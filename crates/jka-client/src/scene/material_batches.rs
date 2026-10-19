//! Materials.
use crate::scene::{
    bsp_fog_params, material_legacy2_in_stage_safe, materials, stage_fog_color_override, AlphaGen,
    AssetSearchPath, BTreeMap, BlendFactor, BlendMode, Bsp, CullMode, DrawBatch, DrawClass, MaterialDebugEntry,
    MaterialDebugInfo, MaterialStage, PipelineKey, Range, RgbGen, Shader, StageTexture,
    SurfaceMaterial, TcGen, TcMod, Textures, GpuVertex,
};

pub(in crate::scene) fn stage_is_opaque_replacement(stage: &MaterialStage) -> bool {
    stage.blend
        == Some(materials::BlendFunc {
            src: BlendFactor::One,
            dst: BlendFactor::Zero,
        })
}

/// Is this stage part of a composed, authored environment effect rather than
/// a free-standing surface reflection that we can replace with a planar view?
///
/// This is *not* a name/texture blacklist. Some custom glass uses the same
/// image both for the background-filter pass and its additive environment pass;
/// other materials use different images with similarly interdependent blends.
/// In both cases, keep TaystJK's original stage sampling and framebuffer math.
///
/// A blended environment overlay over an ordinary opaque base is still eligible
/// for the existing enhanced-planar path (e.g. polished floors). Mirrors remain
/// explicitly controlled by `portal`, never by this inference.
pub(in crate::scene) fn authored_environment_composite(
    material: &SurfaceMaterial,
    stage: &MaterialStage,
) -> bool {
    if !material.explicit || material.planar_reflection || material.water {
        return false;
    }
    // Explicit glass may consist of this environment contribution alone.
    // Keep its authored framebuffer blend when glass promotion is disabled;
    // the separate reflection-quality resolver must not replace it either.
    if material.surface_material == jka_assets::bsp::MATERIAL_GLASS as u8
        && matches!(stage.tc_gen, TcGen::Environment)
    {
        return true;
    }
    // Share the exact classification with parsed-shader preparation. Off must
    // not delete a stage that the Ultra guard considers part of the material.
    materials::authored_environment_composite_stage(
        &material.stages,
        stage,
        |stage| matches!(stage.tc_gen, TcGen::Environment),
        |stage| stage.blend,
        |a, b| matches!(a.texture, StageTexture::Image(_)) && a.texture == b.texture,
    )
}

pub(in crate::scene) fn material_is_authored_environment_composite(
    material: &SurfaceMaterial,
) -> bool {
    material.stages.iter().any(|stage| authored_environment_composite(material, stage))
}

pub(in crate::scene) fn material_has_promotable_environment_stage(
    material: &SurfaceMaterial,
) -> bool {
    !material_is_authored_environment_composite(material)
        && material.stages.iter().any(|stage| matches!(stage.tc_gen, TcGen::Environment))
}

pub(in crate::scene) fn stage_class(stage: &MaterialStage) -> DrawClass {
    // GL_ONE GL_ZERO is authored through blendFunc, but it does not actually
    // blend with the framebuffer: src * 1 + dst * 0 is a full replacement.
    // Keep world-material semantics aligned with the FX/player paths so a
    // replacement base is an opaque/depth-seeding pass instead of letting
    // later translucent surfaces show through it.
    if stage.blend.is_some() && !stage_is_opaque_replacement(stage) {
        DrawClass::Transparent
    } else if stage.alpha_cutoff != 0.0 {
        DrawClass::Mask
    } else {
        DrawClass::Opaque
    }
}

/// A `surfaceparm water` material whose authored colour or alpha generator can
/// reach zero (e.g. costadelsol's shore-surf sheet `seph_surf`, `rgbGen wave sin
/// 0 0.6 0.5 0.09`, additive) is an overlay that fades itself out, not a body of
/// water. Promoting it into the FFT ocean multiplies the sea body colour by that
/// wave, so the sheet turned into an opaque black water patch for roughly half of
/// every 11 s cycle. Such materials keep their authored draw.
pub(in crate::scene) fn water_fades_to_nothing(material: &SurfaceMaterial) -> bool {
    use jka_assets::shader::{AlphaGen, RgbGen, WaveFunc};
    let reaches_zero = |wave: jka_assets::shader::Wave| {
        let low = match wave.func {
            WaveFunc::Sin | WaveFunc::Triangle | WaveFunc::Square | WaveFunc::Noise => {
                wave.base - wave.amplitude.abs()
            }
            WaveFunc::Sawtooth | WaveFunc::InverseSawtooth => wave.base.min(wave.base + wave.amplitude),
        };
        low <= 0.05
    };
    material.water
        && material.stages.iter().any(|stage| {
            matches!(stage.rgb_gen, RgbGen::Wave(wave) if reaches_zero(wave))
                || matches!(stage.alpha_gen, AlphaGen::Wave(wave) if reaches_zero(wave))
        })
}

/// Computed during map/shader preparation, not in the per-frame render loop.
/// TaystJK framebuffer blendFunc is stage-local: an opaque material may use
/// alpha/additive blending for later stages (lightmaps, terrain splats,
/// caustics, depthfade). Those siblings must not be composited over an already
/// cloud-shadowed base. Genuine translucent shaders remain deferred.
pub(in crate::scene) fn opaque_composite_before_clouds(material: &SurfaceMaterial) -> bool {
    if material.sky
        || material.water
        || material.planar_reflection
        || material.authored_translucent
        || material.auto_sprite2
        // Explicit translucent sort classes start above the opaque/decal range.
        || material.sort.is_some_and(|sort| !sort.is_finite() || sort >= 5.0)
    {
        return false;
    }
    material.stages.first().is_some_and(|base| {
        let first = stage_pipeline(material, base, true);
        first.depth_write && matches!(first.class, DrawClass::Opaque | DrawClass::Mask)
    })
}

/// A late-sorted shader (`sort additive`) that nevertheless starts with an
/// alpha-tested, depth-writing stage, so its geometry is a cutout that
/// occupies the scene like an opaque surface. Seaweed and fish: they draw after
/// the water by sort, yet sit underneath it and must be absorbed with the rest.
fn late_sorted_depth_seed(material: &SurfaceMaterial) -> bool {
    if material.sky
        || material.water
        || material.planar_reflection
        || material.auto_sprite2
        || material.glass_promotion
        || !material.sort.is_some_and(|sort| sort.is_finite() && sort >= 5.0)
    {
        return false;
    }
    material.stages.first().is_some_and(|base| {
        let first = stage_pipeline(material, base, true);
        first.depth_write && matches!(first.class, DrawClass::Opaque | DrawClass::Mask)
    })
}

/// Per-stage refinement of `opaque_composite_before_clouds`. An alpha-blended
/// stage AFTER the material's lightmap filter is an overlay on the already lit
/// result (e.g. costadelsol's `depthfade.tga` height haze), not albedo or
/// sunlight. Darkening it with the cloud multiplier turned the authored haze
/// into hard blotches, so it and the stages after it go behind the multiplier.
/// Albedo splats before the lightmap and additive caustics stay in front.
/// Returns how many authored stages, counted from the end, start at the first
/// such overlay (the overlay itself plus everything after it).
fn lit_overlay_trailing_stages(material: &SurfaceMaterial) -> Option<usize> {
    if !opaque_composite_before_clouds(material) {
        return None;
    }
    let lightmap = material
        .stages
        .iter()
        .position(|s| matches!(s.texture, StageTexture::Lightmap))?;
    let overlay = lightmap
        + 1
        + material.stages[lightmap + 1..].iter().position(|s| {
            s.blend
                == Some(materials::BlendFunc {
                    src: BlendFactor::SrcAlpha,
                    dst: BlendFactor::OneMinusSrcAlpha,
                })
        })?;
    Some(material.stages.len() - overlay)
}

const LIT_OVERLAY_BLEND: BlendMode =
    BlendMode::Custom(BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha);

/// Stable fixed-point JKA material sort bucket.  Deliberately store it on each
/// authored stage: runtime submission cannot infer this from the stage's
/// pipeline class (e.g. a late-sorted alpha-tested depth seed).
pub(in crate::scene) fn effective_material_sort(material: &SurfaceMaterial) -> i32 {
    // Match FinishShader: polygonOffset defaults to SS_DECAL, even when its
    // single stage uses destination-color blending (e.g. mp_imperial_symbol).
    let default = if material.sky { 2.0 } else if material.offset { 4.0 }
        else if class_for(material) == DrawClass::Transparent { 14.0 } else { 3.0 };
    (material.sort.unwrap_or(default) * 1024.0) as i32
}

pub(in crate::scene) fn class_for(material: &SurfaceMaterial) -> DrawClass {
    if material.sky {
        DrawClass::Sky
    } else if material.sort.is_some_and(|sort| sort >= 5.0) {
        // This is the geometry/PVS *ordering* class, not a per-stage GPU
        // pipeline class. TaystJK sorts the whole shader at SS_SEE_THROUGH
        // or later, even when its first stage is an alpha-tested depth seed.
        // Keep that stage's Mask pipeline unchanged for GE128/depthWrite.
        DrawClass::Transparent
    } else {
        material
            .stages
            .first()
            .map(stage_class)
            .unwrap_or(DrawClass::Opaque)
    }
}

/// True when the authored alpha-test stage can never survive its own alpha
/// cutoff. Texture alpha is normalized to [0, 1], so a constant alpha below
/// the cutoff remains below it after multiplication by any sampled texel.
///
/// Keep this deliberately narrow: it recognizes a mathematically guaranteed
/// discard, not merely a stage that "looks invisible" for common content.
pub(in crate::scene) fn stage_is_guaranteed_alpha_discard(stage: &MaterialStage) -> bool {
    matches!(stage.alpha_gen, AlphaGen::Const)
        && stage.alpha_cutoff > 0.0
        && stage.opacity < stage.alpha_cutoff
}

/// True for the common q3map light-brush idiom that emits light at compile
/// time but contributes no visible RGB at runtime: a single `$whiteimage`
/// stage, black `rgbGen const`, and additive `GL_ONE GL_ONE` blending.
///
/// Restrict this to authored surface lights rather than treating every black
/// additive stage as disposable. OpenGL blend state also touches destination
/// alpha, and arbitrary non-light shaders can intentionally depend on that.
pub(in crate::scene) fn stage_is_light_only_black_additive(
    stage: &MaterialStage,
    has_authored_surface_light: bool,
) -> bool {
    has_authored_surface_light
        && matches!(stage.texture, StageTexture::White)
        && matches!(stage.rgb_gen, RgbGen::Const)
        && stage
            .color
            .iter()
            .all(|channel| channel.abs() <= f32::EPSILON)
        && stage.blend
            == Some(materials::BlendFunc {
                src: BlendFactor::One,
                dst: BlendFactor::One,
            })
        && !stage.depth_write
        && stage.alpha_cutoff == 0.0
}

/// Map-load-only render cull for explicit shader geometry whose ordinary
/// stages are guaranteed not to contribute visible RGB. The BSP surface itself
/// remains intact so compile/runtime semantics that consume its triangles first
/// (for example q3map_surfacelight extraction) are preserved.
pub(in crate::scene) fn material_render_is_guaranteed_discarded(
    material: &SurfaceMaterial,
    vertex_lit: bool,
) -> bool {
    if !material.explicit
        || material.sky
        || material.water
        || material.planar_reflection
        || material.grass.is_some()
        || !material.surface_sprite_effects.is_empty()
        || material.stages.is_empty()
    {
        return false;
    }

    let stages = prepared_stages(material, vertex_lit);
    let authored_light_only_candidate = material
        .surface_light
        .is_some_and(|light| !light.inferred_from_emissive)
        && stages.len() == 1;
    stages.iter().all(|stage| {
        stage_is_guaranteed_alpha_discard(stage)
            || stage_is_light_only_black_additive(stage, authored_light_only_candidate)
    })
}

pub(in crate::scene) fn blend_for(stage: &MaterialStage) -> BlendMode {
    if stage_is_opaque_replacement(stage) {
        BlendMode::Opaque
    } else {
        stage
            .blend
            .map(|blend| BlendMode::Custom(blend.src, blend.dst))
            .unwrap_or(BlendMode::Opaque)
    }
}

pub(in crate::scene) fn effective_world_cull(material: &SurfaceMaterial) -> CullMode {
    // Raven's quick-sprite renderer ends a surfaceSprites group by enabling
    // face culling. On a parent shader authored `cull twosided`, that leaves
    // retail JKA effectively one-sided after the sprite path. wgpu pipelines
    // do not have that kind of global mutable raster state, so reproduce the
    // retail-visible result directly and deterministically on the world draw.
    //
    // Do not touch already-one-sided authored modes, and do not touch the
    // procedural grass pipelines themselves (they remain explicitly two-sided).
    if material.surface_sprite_cull_quirk && material.cull == CullMode::None {
        CullMode::Back
    } else {
        material.cull
    }
}

pub(in crate::scene) fn stage_pipeline(
    material: &SurfaceMaterial,
    stage: &MaterialStage,
    first: bool,
) -> PipelineKey {
    PipelineKey {
        legacy_srgb_three_stage: false,
        additive_fusion_count: 0,
        auto_sprite2: material.auto_sprite2,
        sky_outer: material.sky && material.skybox.is_some()
            && !matches!(stage.tc_gen, TcGen::SkyCloud(_)),
        class: if material.sky {
            DrawClass::Sky
        } else {
            stage_class(stage)
        },
        blend: blend_for(stage),
        cull: if material.sky {
            CullMode::None
        } else {
            effective_world_cull(material)
        },
        offset: material.offset,
        // OpenJK's default opaque first stage writes depth. Treat GL_ONE GL_ZERO
        // as that same opaque replacement semantic; later genuinely blended
        // stages still need an explicit depthWrite.
        depth_write: stage.depth_write
            || (first && (stage.blend.is_none() || stage_is_opaque_replacement(stage))),
        depth_equal: stage.depth_equal,
    }
}

pub(in crate::scene) fn resolved_bsp_surface_fog(
    bsp: &Bsp,
    library: &BTreeMap<String, Shader>,
    material: &SurfaceMaterial,
    fog_num: i32,
    global_fog_num: Option<i32>,
    global_fog: Option<[f32; 4]>,
) -> ([f32; 4], bool) {
    // TaystJK `noglfog` disables runtime GL fog for this material even when
    // the compiled BSP provides a valid fog index. q3map_nofog has already
    // opted out at compile time, but is also safe to respect at runtime.
    if material.no_fog {
        return ([0.0; 4], false);
    }
    let authored = bsp_fog_params(bsp, library, fog_num);
    if authored[3] > 0.001 {
        return (authored, global_fog_num == Some(fog_num));
    }

    // q3map2 normally writes the default/global fog index into every eligible
    // draw surface. Some legacy/custom BSPs leave fogNum == -1 instead. OpenJK
    // can inherit the world's global fog for BSP-model surfaces; recover the
    // same visual intent here for any missing assignment, but never override
    // q3map_nofog authored by the material. Local brush fog remains strictly
    // driven by the compiled per-surface fog index.
    if fog_num < 0 && !material.no_fog {
        if let Some(global) = global_fog {
            return (global, true);
        }
    }

    (authored, false)
}

/// See `DrawBatch::dlight_in_lightmap_stage`. Materials with any PBR companion
/// keep the base-stage light so their microfacet response is unchanged.
pub(in crate::scene) fn material_dlight_in_lightmap_stage(material: &SurfaceMaterial) -> bool {
    material
        .stages
        .iter()
        .any(|stage| matches!(stage.texture, StageTexture::Lightmap))
        && !material.stages.iter().any(|stage| {
            let e = &stage.enhancements;
            e.normal_texture.is_some()
                || e.roughness_texture.is_some()
                || e.height_texture.is_some()
                || e.metallic_texture.is_some()
                || e.specular_texture.is_some()
                || e.emissive_texture.is_some()
                || e.roughness_override.is_some()
                || e.specular_reflectance.is_some()
        })
}

pub(in crate::scene) fn stage_batch(
    material: &SurfaceMaterial,
    stage: &MaterialStage,
    first: bool,
    vertex_lit: bool,
    vertices: Range<u32>,
    bsp_shader_index: Option<usize>,
    material_debug_index: Option<usize>,
    lightmap: Option<usize>,
    pvs_signature: &[u64],
    area_signature: [u64; 4],
    fog: [f32; 4],
    fog_is_global: bool,
) -> DrawBatch {
    let authored_environment_composite = material_is_authored_environment_composite(material);
    let (texture, texture_is_lightmap, texture_is_white) = match stage.texture {
        StageTexture::Image(index) => (Some(index), false, false),
        StageTexture::Lightmap => (None, true, false),
        StageTexture::White => (None, false, true),
    };
    let pipeline = stage_pipeline(material, stage, first);
    let detail_texture_eligible = !material.authored_detail
        && !material.water
        && !material.planar_reflection
        && matches!(stage.texture, StageTexture::Image(_))
        && matches!(stage.tc_gen, TcGen::Base)
        && stage.tc_mods.is_empty()
        && pipeline.class == DrawClass::Opaque
        && pipeline.blend == BlendMode::Opaque;
    DrawBatch {
        vertices,
        bsp_shader_index: bsp_shader_index.map_or(u32::MAX, |index| index as u32),
        material_debug_index: material_debug_index.map_or(u32::MAX, |index| index as u32),
        surface_material: material.surface_material,
        glass_primary: material.glass_promotion && first,
        glass_tint: material.glass_tint,
        glass_reflectance: material.glass_reflectance,
        opaque_composite_before_clouds: opaque_composite_before_clouds(material),
        lit_overlay: false,
        late_depth_seed: late_sorted_depth_seed(material),
        late_sorted_material: material.sort.is_some_and(|sort| sort >= 5.0),
        authored_sort_priority: effective_material_sort(material),
        texture,
        legacy_srgb_overlay: None,
        additive_companions: Vec::new(),
        animation: stage.animation.clone(),
        texture_is_lightmap,
        texture_is_white,
        detail_texture_eligible,
        normal_texture: stage.enhancements.normal_texture,
        roughness_texture: stage.enhancements.roughness_texture,
        height_texture: stage.enhancements.height_texture,
        metallic_texture: stage.enhancements.metallic_texture,
        specular_texture: stage.enhancements.specular_texture,
        emissive_texture: stage.enhancements.emissive_texture,
        height_from_alpha: stage.enhancements.height_from_alpha,
        rmo_packed: stage.enhancements.rmo_packed,
        rmo_specular_alpha: stage.enhancements.rmo_specular_alpha,
        normal_scale: stage.enhancements.normal_scale,
        roughness_override: stage.enhancements.roughness_override,
        specular_reflectance: stage.enhancements.specular_reflectance,
        parallax_depth: stage.enhancements.parallax_depth,
        lightmap,
        modulate_lightmap: false,
        reverse_lightmap_fold_role: 0,
        reverse_lightmap_fold_detail: false,
        dlight_in_lightmap_stage: material_dlight_in_lightmap_stage(material),
        vertex_lit,
        pipeline,
        tc_gen: stage.tc_gen,
        tc_mods: stage.tc_mods.clone(),
        rgb_gen: stage.rgb_gen,
        alpha_gen: stage.alpha_gen,
        color: [
            stage.color[0],
            stage.color[1],
            stage.color[2],
            stage.opacity,
        ],
        alpha_cutoff: stage.alpha_cutoff,
        alpha_test_texture_only: false,
        shader_time_offset: 0.0,
        shader_clamp_time: material.clamp_time,
        no_picmip: material.no_picmip,
        authored_glow: stage.glow,
        fog,
        fog_is_global,
        fog_color_override: stage_fog_color_override(stage, first),
        // A sky's stages are cloud layers drawn as extra batches; they must not
        // change the fog eligibility the sky had when it was a single batch.
        legacy2_fog_in_stage_safe: material_legacy2_in_stage_safe(if material.sky {
            &[]
        } else {
            &material.stages
        }),
        global_fog_post_eligible: !material.sky
            && material
                .stages
                .iter()
                .enumerate()
                .any(|(index, candidate)| {
                    stage_pipeline(material, candidate, index == 0).depth_write
                }),
        planar_reflection: material.planar_reflection && first,
        water: material.water,
        water_amplitude: material.water_amplitude,
        water_deformed: material.water_deformed,
        alpha_shadow: material.alpha_shadow,
        baked_shadow_texture: material.baked_shadow_texture,
        light_filter: material.light_filter,
        water_primary: material.water && first && !water_fades_to_nothing(material),
        water_overlay: material.water && water_fades_to_nothing(material),
        authored_ocean: None,
        planar_environment_candidate: matches!(stage.tc_gen, TcGen::Environment)
            && !authored_environment_composite,
        // Record at material scope, so the Trace Inspector reports it even
        // when the user selects the base (non-environment) stage.
        authored_environment_composite,
        planar_plane: [0.0; 4],
        planar_pvs_origin: [0.0; 3],
        skybox: material.skybox,
        reflection_probe: None,
        reflection_probe_position_radius: [0.0; 4],
        reflection_cache_flags: 0,
        reflection_roughness_hint: 1.0,
        pvs_signature: pvs_signature.to_vec(),
        area_signature,
    }
}

pub(in crate::scene) fn prepared_stages(
    material: &SurfaceMaterial,
    vertex_lit: bool,
) -> Vec<MaterialStage> {
    let mut stages = material.stages.clone();
    if material.glass_promotion {
        let Some(mut stage) = stages.into_iter().next() else { return Vec::new(); };
        // The source image only informs the tint. Its low-resolution reflection
        // pattern and framebuffer blend no longer appear in the promoted pass.
        stage.texture = StageTexture::White;
        stage.animation = None;
        stage.enhancements = Default::default();
        stage.blend = Some(materials::BlendFunc {
            src: BlendFactor::SrcAlpha,
            dst: BlendFactor::OneMinusSrcAlpha,
        });
        stage.alpha_cutoff = 0.0;
        stage.opacity = 1.0;
        stage.color = [1.0; 3];
        stage.rgb_gen = RgbGen::Identity;
        stage.alpha_gen = AlphaGen::Identity;
        stage.tc_gen = TcGen::Base;
        stage.tc_mods.clear();
        stage.depth_write = false;
        stage.depth_equal = false;
        stage.glow = false;
        return vec![stage];
    }
    if !vertex_lit {
        return stages;
    }

    // q3map static models and other LIGHTMAP_BY_VERTEX surfaces store baked
    // lighting in BSP vertex colors. OpenJK substitutes that vertex color for a
    // `$lightmap` stage instead of trying to sample a nonexistent lightmap page.
    if let Some(index) = stages
        .iter()
        .position(|stage| matches!(stage.texture, StageTexture::Lightmap))
    {
        if index == 0 {
            // An initial lightmap stage is folded into the next pass. This mirrors
            // OpenJK's "move lightmap stage down" treatment for vertex-lit data.
            stages.remove(0);
            if let Some(first) = stages.first_mut() {
                first.rgb_gen = RgbGen::ExactVertex;
                first.alpha_gen = AlphaGen::Identity;
                first.blend = None;
            }
        } else if let Some(stage) = stages.get_mut(index) {
            stage.texture = StageTexture::White;
            stage.rgb_gen = RgbGen::ExactVertex;
            stage.alpha_gen = AlphaGen::Identity;
        }
    } else if !material.explicit {
        // Implicit materials normally render base texture × lightmap. For a
        // vertex-lit BSP surface the corresponding operation is base × vertex.
        if let Some(first) = stages.first_mut() {
            first.rgb_gen = RgbGen::ExactVertex;
            first.alpha_gen = AlphaGen::Identity;
        }
    } else if !stages.iter().any(|stage| stage.rgb_gen.uses_vertex_color()) {
        // Rend2 .mtr overrides often replace the classic JKA diffuse stage with
        // a PBR-enhanced diffuse stage (normal/RMO/specular companions) but may
        // omit the legacy rgbGen that consumed q3map's LIGHTMAP_BY_VERTEX data.
        // Do not make every explicit shader vertex-lit: classic explicit shaders
        // can intentionally author identity/fullbright passes. Restrict this
        // compatibility fallback to the primary opaque PBR-enhanced base stage,
        // and leave additive/glow sibling stages untouched.
        if let Some(base) = stages
            .iter_mut()
            .find(|stage| enhanced_identity_base_stage(stage))
        {
            base.rgb_gen = RgbGen::ExactVertex;
            base.alpha_gen = AlphaGen::Identity;
        }
    }
    stages
}

pub(in crate::scene) fn has_stage_enhancements(stage: &MaterialStage) -> bool {
    let e = &stage.enhancements;
    e.normal_texture.is_some()
        || e.roughness_texture.is_some()
        || e.height_texture.is_some()
        || e.metallic_texture.is_some()
        || e.specular_texture.is_some()
        || e.emissive_texture.is_some()
        || e.roughness_override.is_some()
        || e.specular_reflectance.is_some()
}

pub(in crate::scene) fn enhanced_identity_base_stage(stage: &MaterialStage) -> bool {
    matches!(stage.texture, StageTexture::Image(_))
        && stage.blend.is_none()
        && matches!(stage.tc_gen, TcGen::Base)
        && matches!(stage.rgb_gen, RgbGen::Identity)
        && has_stage_enhancements(stage)
}

pub(in crate::scene) fn debug_texture_label(index: Option<usize>, textures: &Textures) -> String {
    index
        .and_then(|index| textures.images.get(index))
        .map(|image| image.label.clone())
        .unwrap_or_else(|| "-".into())
}

pub(in crate::scene) fn debug_texture_source(index: Option<usize>, textures: &Textures) -> String {
    index
        .and_then(|index| textures.images.get(index))
        .map(|image| {
            let provider = image.source.as_deref().map_or_else(
                || "<generated / unknown>".to_owned(),
                |path| path.display().to_string(),
            );
            format!("{} @ {provider}", image.label)
        })
        .unwrap_or_else(|| "-".into())
}

/// Map-load-only audit classification; never changes stage emission, blending,
/// depth, fog or authored shader state. Used for ranking potential work.

pub(in crate::scene) fn stage_audit_class(material: &SurfaceMaterial) -> &'static str {
    if material.sky { return "sky_ordered"; }
    if !material.explicit { return "implicit_base"; }
    if material.stages.len() <= 1 { return "single_stage"; }
    if can_fold_jka_lightmap_pair(&material.stages[0], &material.stages[1]) {
        return "existing_lightmap_pair_when_eligible";
    }
    if material.water || material.planar_reflection { return "water_or_reflection"; }
    if can_fold_openjk_reverse_lightmap_modulate_pair(&material.stages[0], &material.stages[1]) {
        return "optional_reverse_lightmap_pair_when_no_fog";
    }
    if material.stages.iter().any(|s| s.glow) { return "glow_requires_parity"; }
    if material.stages.iter().any(|s| s.alpha_cutoff != 0.0 || s.depth_equal || s.depth_write) {
        return "depth_or_alpha_test";
    }
    if material.stages.iter().any(|s| !s.tc_mods.is_empty() || !matches!(s.tc_gen, TcGen::Base | TcGen::Lightmap)) {
        return "dynamic_or_special_texcoords";
    }
    if material.stages.iter().any(has_stage_enhancements) { return "enhanced_stage_companion"; }
    if material.stages.iter().any(|s| s.blend.is_some()) { return "ordered_framebuffer_blend"; }
    "multiple_opaque_stages_review"
}

pub(in crate::scene) fn build_material_debug_entry(
    name: &str,
    definition: Option<&Shader>,
    origin: Option<&materials::ShaderDefinitionOrigin>,
    material: &SurfaceMaterial,
    textures: &Textures,
    assets: &AssetSearchPath,
) -> MaterialDebugEntry {
    let mut lines = Vec::new();
    let mut resolution_lines = Vec::new();
    resolution_lines.push(format!(
        "SHADER LOOKUP: {} ({})",
        if origin.is_some() { "FOUND" } else { "NO MATCH" },
        if origin.is_some() {
            "exact name in parsed shader library"
        } else {
            "no exact name in parsed shader library; this is not a disk search"
        }
    ));
    if let Some(origin) = origin {
        resolution_lines.push(format!(
            "SHADER DEFINITION: {} @ {}",
            origin.file, origin.source.display()
        ));
    }
    if !material.explicit {
        // This is the exact fallback path in materials::describe(), not an
        // approximation of authored stages. Only probe the already indexed VFS.
        resolution_lines.push(format!("IMPLICIT DIFFUSE REQUEST: {name} (clamp=false, sRGB=true)"));
        resolution_lines.push(format!(
            "ASSET OVERRIDES: {} | candidate probe is INDEX ONLY (no read/decode)",
            if assets.asset_overrides_allowed() { "ENABLED" } else { "DISABLED (stock-first)" }
        ));
        let candidates = materials::texture_lookup_candidates(name);
        let mut first_indexed = None;
        // Decode tries every stock candidate first when overrides are disabled,
        // then falls back to normal VFS resolution in candidate order.
        if !assets.asset_overrides_allowed() {
            for candidate in &candidates {
                if let Some(provider) = assets.indexed_provider_path(candidate, true) {
                    first_indexed = Some(format!("{candidate} @ {} (stock)", provider.display()));
                    break;
                }
            }
        }
        for (index, candidate) in candidates.iter().enumerate() {
            let provider = assets.indexed_provider_path(candidate, false);
            if first_indexed.is_none() {
                if let Some(provider) = provider {
                    first_indexed = Some(format!("{candidate} @ {}", provider.display()));
                }
            }
            resolution_lines.push(format!(
                "IMAGE CANDIDATE {index}: {candidate} -> {}",
                provider.map_or_else(|| "NOT INDEXED".to_owned(), |p| p.display().to_string())
            ));
        }
        resolution_lines.push(format!(
            "FIRST INDEXED IMAGE (load order): {}",
            first_indexed.as_deref().unwrap_or("NONE")
        ));
        match material.stages.first().map(|stage| stage.texture) {
            Some(StageTexture::Image(index)) => {
                resolution_lines.push(format!(
                    "IMPLICIT IMAGE RESULT: LOADED {}",
                    debug_texture_source(Some(index), textures)
                ));
                resolution_lines.push("IMPLICIT RECIPE: decoded diffuse * BSP lightmap when surface is lightmapped".into());
            }
            Some(StageTexture::White) => {
                resolution_lines.push("IMPLICIT IMAGE RESULT: FALLBACK $whiteimage (not authored)".into());
                if let Some(error) = textures.load_failure(name, false, true) {
                    let kind = if error.starts_with("Missing texture:") {
                        "MISSING ASSET"
                    } else if error.starts_with("negative cached") {
                        "NEGATIVE CACHE"
                    } else {
                        "READ/DECODE FAILURE"
                    };
                    resolution_lines.push(format!("FALLBACK CAUSE: {kind}: {error}"));
                } else {
                    resolution_lines.push("FALLBACK CAUSE: no recorded texture-load error (inspect cache/resolution path)".into());
                }
            }
            Some(StageTexture::Lightmap) => resolution_lines.push("IMPLICIT IMAGE RESULT: $lightmap only".into()),
            None => resolution_lines.push("IMPLICIT IMAGE RESULT: no render stages".into()),
        }
    } else {
        resolution_lines.push("IMPLICIT IMAGE LOOKUP: NOT USED (authored shader stages)".into());
    }
    if let Some(definition) = definition {
        let stage_unsupported = definition
            .stages
            .iter()
            .map(|stage| stage.unsupported.len())
            .sum::<usize>();
        let unsupported = definition.unsupported.len() + stage_unsupported;
        lines.push(format!(
            "parsed shader: stages={} portal={} sky={} nodraw={} translucent={} nofog={} cull={} sort={:?} unsupported={} (shader={} stage={})",
            definition.stages.len(),
            definition.portal,
            definition.sky,
            definition.nodraw,
            definition.translucent,
            definition.no_fog,
            if definition.cull.is_empty() {
                "default"
            } else {
                &definition.cull
            },
            definition.sort,
            unsupported,
            definition.unsupported.len(),
            stage_unsupported
        ));
        for (index, stage) in definition.stages.iter().enumerate() {
            let color = stage.color.unwrap_or([1.0; 3]);
            lines.push(format!(
                "authored stage {index}: map={} blend={} rgbGen={:?} rgbConst={:.3},{:.3},{:.3} alphaGen={:?} normalMap={} normalHeightMap={} normalScale={} rmoMap={} rmos={} specMap={} roughness={} specularReflectance={} parallaxDepth={}",
                if stage.image.is_empty() { "-" } else { &stage.image },
                if stage.blend.is_empty() { "opaque" } else { &stage.blend },
                stage.rgb_gen,
                color[0],
                color[1],
                color[2],
                stage.alpha_gen,
                stage.normal_map.as_deref().unwrap_or("-"),
                stage.normal_height_map.as_deref().unwrap_or("-"),
                stage.normal_scale
                    .map(|value| format!("{:.3},{:.3}", value[0], value[1]))
                    .unwrap_or_else(|| "-".into()),
                stage.rmo_map.as_deref().unwrap_or("-"),
                stage.rmo_specular_alpha,
                stage.specular_map.as_deref().unwrap_or("-"),
                stage.roughness
                    .map(|value| format!("{value:.4}"))
                    .unwrap_or_else(|| "-".into()),
                stage.specular_reflectance
                    .map(|value| format!("{:.3},{:.3},{:.3}", value[0], value[1], value[2]))
                    .unwrap_or_else(|| "-".into()),
                stage.parallax_depth
                    .map(|value| format!("{value:.4}"))
                    .unwrap_or_else(|| "-".into())
            ));
            if let Some(sprite) = stage.surface_sprite {
                lines.push(format!(
                    "authored stage {index}: surfaceSprites={:?} width={:.1} height={:.1} density={:.1} fade={:.1}..{:.1} fadeScale={:.2} variance={:.2},{:.2} wind={:.2} idle={:.2} vertSkew={:.2} facing={:?}",
                    sprite.kind,
                    sprite.width,
                    sprite.height,
                    sprite.density,
                    sprite.fade_dist,
                    sprite.fade_max,
                    sprite.fade_scale,
                    sprite.variance[0],
                    sprite.variance[1],
                    sprite.wind,
                    sprite.wind_idle,
                    sprite.vert_skew,
                    sprite.facing,
                ));
            }
            for unsupported in &stage.unsupported {
                lines.push(format!("authored stage {index}: UNSUPPORTED {unsupported}"));
            }
        }
    } else {
        lines.push("parsed shader: none (implicit material)".into());
    }

    if let Some(grass) = material.grass {
        lines.push(format!(
            "resolved grass emitter: r_grass 1=GodotGrass, 0=native JKA sprites; height={:.1} JKA-width-hint={:.1} density={:.1} fade={:.1}..{:.1} wind={:.2}",
            grass.height,
            grass.authored_width,
            grass.density,
            grass.fade_dist,
            grass.fade_max,
            grass.wind,
        ));
    }

    for sprite in &material.surface_sprite_effects {
        lines.push(format!(
            "resolved native surfaceSprite: {:?} texture={} size={:.1}x{:.1} density={:.1} alphaCut={:.3} depthWrite={} GodotCandidate={} (r_grass 0 always uses native)",
            sprite.sprite.kind,
            sprite.texture,
            sprite.sprite.width,
            sprite.sprite.height,
            sprite.sprite.density,
            sprite.alpha_cutoff,
            sprite.depth_write,
            sprite.godot_candidate,
        ));
    }

    if material.surface_sprite_cull_quirk && material.cull == CullMode::None {
        lines.push(
            "retail surfaceSprites cull quirk: authored twosided -> effective world cull=Back; procedural grass remains two-sided"
                .into(),
        );
    }

    if let Some(surface_light) = material.surface_light {
        lines.push(format!(
            "q3map authored surface emitter: value={:.1} color={:.3},{:.3},{:.3} subdivide={:.1}",
            surface_light.value,
            surface_light.color[0],
            surface_light.color[1],
            surface_light.color[2],
            surface_light.subdivide,
        ));
    }
    if let Some(promoted) = material.shader_light {
        let mut reasons = Vec::new();
        if promoted.reasons & materials::SHADER_LIGHT_GLOW != 0 { reasons.push("glow"); }
        if promoted.reasons & materials::SHADER_LIGHT_ADDITIVE != 0 { reasons.push("additive"); }
        if promoted.reasons & materials::SHADER_LIGHT_EMISSIVE_COMPANION != 0 { reasons.push("emissive-map"); }
        lines.push(format!(
            "shader-promoted surface emitter: reasons={} value={:.1} color={:.3},{:.3},{:.3} subdivide={:.1}",
            reasons.join("+"),
            promoted.value,
            promoted.color[0],
            promoted.color[1],
            promoted.color[2],
            promoted.subdivide,
        ));
    }

    if material.stages.len() >= 2
        && can_fold_jka_lightmap_pair(&material.stages[0], &material.stages[1])
    {
        let stage = &material.stages[1];
        let pbr = has_stage_enhancements(stage);
        let pom = stage.enhancements.height_texture.is_some();
        lines.push(format!(
            "effective render: OpenJK-style $lightmap + GL_DST_COLOR/GL_ZERO pair folds to ONE opaque base*lightmap batch; pbr_eligible={pbr} pom_eligible={pom}"
        ));
    }

    if material.stages.len() >= 2
        && can_fold_openjk_reverse_lightmap_modulate_pair(&material.stages[0], &material.stages[1])
    {
        lines.push(format!(
            "optional OpenJK GL_ZERO/GL_SRC_COLOR modulation: r_stageCollapseExtra (requires BSP lightmap, no BSP fog, no water/reflection; default OFF)",
        ));
    }

    lines.push(format!(
        "performance stage audit: authored={} resolved={} class={} (classification only; actual draw recipe depends on LM/vertex/fog/visibility)",
        definition.map_or(material.stages.len(), |d| d.stages.len()),
        material.stages.len(),
        stage_audit_class(material),
    ));
    let mut enhanced = false;
    let mut image_sources = Vec::new();
    for (index, stage) in material.stages.iter().enumerate() {
        let base = match stage.texture {
            StageTexture::Image(texture) => textures
                .images
                .get(texture)
                .map(|image| image.label.clone())
                .unwrap_or_else(|| format!("<missing texture #{texture}>")),
            StageTexture::Lightmap => "$lightmap".into(),
            StageTexture::White => "$whiteimage".into(),
        };
        let e = &stage.enhancements;
        if let StageTexture::Image(texture) = stage.texture {
            image_sources.push(format!(
                "STAGE {index} BASE IMAGE: {}",
                debug_texture_source(Some(texture), textures)
            ));
        }
        for (kind, texture) in [
            ("NORMAL", e.normal_texture),
            ("ROUGHNESS", e.roughness_texture),
            ("METALLIC", e.metallic_texture),
            ("SPECULAR", e.specular_texture),
            ("EMISSIVE", e.emissive_texture),
            ("HEIGHT", e.height_texture),
        ] {
            if texture.is_some() {
                image_sources.push(format!(
                    "STAGE {index} {kind} IMAGE: {}",
                    debug_texture_source(texture, textures)
                ));
            }
        }
        let stage_enhanced = has_stage_enhancements(stage);
        enhanced |= stage_enhanced;
        let pbr_eligible = matches!(stage.texture, StageTexture::Image(_))
            && stage.blend.is_none()
            && stage.alpha_cutoff == 0.0
            && matches!(stage.tc_gen, TcGen::Base);
        lines.push(format!(
            "resolved stage {index}: base={base} blend={:?} alpha_cutoff={:.3} color={:.3},{:.3},{:.3},{:.3} tcGen={:?} rgbGen={:?} alphaGen={:?} pbr_eligible={} normal={} roughness={} metallic={} specular={} emissive={} height={} height_from_alpha={} rmo_packed={} rmos_alpha={} normalScale={:.3},{:.3} roughnessOverride={} specularReflectance={} parallaxDepth={:.4}",
            stage.blend,
            stage.alpha_cutoff,
            stage.color[0],
            stage.color[1],
            stage.color[2],
            stage.opacity,
            stage.tc_gen,
            stage.rgb_gen,
            stage.alpha_gen,
            pbr_eligible,
            debug_texture_label(e.normal_texture, textures),
            debug_texture_label(e.roughness_texture, textures),
            debug_texture_label(e.metallic_texture, textures),
            debug_texture_label(e.specular_texture, textures),
            debug_texture_label(e.emissive_texture, textures),
            debug_texture_label(e.height_texture, textures),
            e.height_from_alpha,
            e.rmo_packed,
            e.rmo_specular_alpha,
            e.normal_scale[0],
            e.normal_scale[1],
            e.roughness_override
                .map(|value| format!("{value:.4}"))
                .unwrap_or_else(|| "-".into()),
            e.specular_reflectance
                .map(|value| format!("{:.3},{:.3},{:.3}", value[0], value[1], value[2]))
                .unwrap_or_else(|| "-".into()),
            e.parallax_depth,
        ));
        if stage_enhanced && !pbr_eligible {
            lines.push(format!(
                "resolved stage {index}: NOTE companion maps loaded but current stage is not eligible for the PBR/POM shader path"
            ));
        }
    }

    MaterialDebugEntry {
        name: name.to_owned(),
        definition_file: origin.map(|origin| origin.file.clone()),
        definition_source: origin.map(|origin| origin.source.clone()),
        source: origin
            .map(|origin| format!("{} @ {}", origin.file, origin.source.display()))
            .unwrap_or_else(|| "<implicit/no shader definition>".into()),
        mtr_override: origin.is_some_and(|origin| origin.mtr_override),
        definition_text: definition.and_then(|definition| definition.definition_text.clone()),
        enhanced,
        authored_stage_count: definition.map_or(material.stages.len(), |d| d.stages.len()),
        resolved_stage_count: material.stages.len(),
        stage_audit_class: stage_audit_class(material),
        stage_sequence: material.stages.iter().map(|stage| format!(
            "{:?} / {:?} / rgb={:?} / alpha={:?} / tc={:?} / mods={} / glow={} / depth_equal={} / depth_write={} / alpha_cutoff={}",
            stage.texture, stage.blend, stage.rgb_gen, stage.alpha_gen,
            stage.tc_gen, stage.tc_mods.len(), stage.glow,
            stage.depth_equal, stage.depth_write, stage.alpha_cutoff,
        )).collect(),
        image_sources,
        resolution_lines,
        lines,
    }
}

pub(in crate::scene) fn material_debug_info(
    library_debug: &materials::ShaderLibraryDiagnostics,
    names_and_materials: impl IntoIterator<Item = (String, SurfaceMaterial)>,
    library: &BTreeMap<String, Shader>,
    textures: &Textures,
    assets: &AssetSearchPath,
) -> MaterialDebugInfo {
    let mut entries = names_and_materials
        .into_iter()
        .map(|(name, material)| {
            build_material_debug_entry(
                &name,
                library.get(&name),
                library_debug.origins.get(&name),
                &material,
                textures,
                assets,
            )
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    MaterialDebugInfo {
        shader_files: library_debug.shader_files,
        mtr_files: library_debug.mtr_files,
        shader_definitions: library_debug.shader_definitions,
        mtr_definitions: library_debug.mtr_definitions,
        entries,
    }
}

pub(in crate::scene) fn can_fold_jka_lightmap_pair(
    lightmap_stage: &MaterialStage,
    material_stage: &MaterialStage,
) -> bool {
    // OpenJK/JKA treats an opaque $lightmap pass followed by a diffuse pass
    // using GL_DST_COLOR/GL_ZERO as ordinary modulation. Its multitexture
    // collapse swaps the lightmap to bundle 1 and renders diffuse * lightmap.
    // Use our existing one-pass base*lightmap path for the same semantics.
    // This is a legacy shader rule, not a PBR-only optimization.
    matches!(lightmap_stage.texture, StageTexture::Lightmap)
        && lightmap_stage.blend.is_none()
        && lightmap_stage.alpha_cutoff == 0.0
        && lightmap_stage.opacity == 1.0
        && lightmap_stage.color == [1.0; 3]
        && matches!(lightmap_stage.rgb_gen, RgbGen::Identity)
        && matches!(lightmap_stage.alpha_gen, AlphaGen::Identity)
        && matches!(lightmap_stage.tc_gen, TcGen::Lightmap)
        && lightmap_stage.tc_mods.is_empty()
        && !lightmap_stage.depth_equal
        && matches!(material_stage.texture, StageTexture::Image(_))
        && material_stage.blend
            == Some(materials::BlendFunc {
                src: BlendFactor::DstColor,
                dst: BlendFactor::Zero,
            })
        && material_stage.alpha_cutoff == 0.0
        && material_stage.opacity == 1.0
        && material_stage.color == [1.0; 3]
        && matches!(material_stage.rgb_gen, RgbGen::Identity)
        && matches!(material_stage.alpha_gen, AlphaGen::Identity)
        && matches!(material_stage.tc_gen, TcGen::Base)
        && !material_stage.depth_equal
}

/// OpenJK `CollapseMultitexture` explicitly permits the companion modulation
/// case: opaque `$lightmap`, then `blendFunc GL_ZERO GL_SRC_COLOR`. Both RGB
/// blend equations yield `diffuse * lightmap`. Use the *existing* fused WGPU
/// base*lightmap path; don't author a second rendering implementation.
///
/// This eligibility test is intentionally *stricter* than OpenJK: no authored
/// glow (separate bloom extraction), secondary depth writes, PBR companion maps,
/// or dynamically transformed texcoords. Caller also rules out BSP fog, water
/// and special reflections so this opt-in cannot silently change those paths.
/// Conservatively recognize a prefix of the commonplace TaystJK idiom:
/// opaque $lightmap; DST_COLOR/ZERO diffuse; SRC_ALPHA/ONE_MINUS_SRC_ALPHA
/// vertex-alpha overlay. Its blending occurs in encoded RGB in the original
/// framebuffer. The separate WGPU draws instead blend in linear light.
///
/// This does not pattern-match shader names or pixel colors. We reject any
/// stage needing a distinct sampler, animation, texcoord generation, depth
/// operation, enhanced lighting, custom tint, glow, or special environment.
/// Unsupported materials retain the original authored stage recipe.
pub(in crate::scene) fn legacy_srgb_three_stage_overlay(
    material: &SurfaceMaterial,
    stages: &[MaterialStage],
    fog: [f32; 4],
) -> Option<usize> {
    if stages.len() < 3
        || !material.explicit
        || material.sky || material.water || material.planar_reflection
        || material.offset || material.auto_sprite2 || material.water_deformed
        || !material.surface_sprite_effects.is_empty()
        || material.authored_detail || material.sort.is_some()
        || fog[3] > 0.001
        || !can_fold_jka_lightmap_pair(&stages[0], &stages[1])
        // Only the first three passes are fused. Later authored stages remain
        // independent, retaining their original animation, blending and glow.
        || stages[..3].iter().any(|stage| stage.animation.is_some()
            || stage.glow || has_stage_enhancements(stage)
            || !stage.tc_mods.is_empty() || stage.depth_equal
            || stage.alpha_cutoff != 0.0)
        || stages[0].depth_write || stages[1].depth_write
        || stages[2].depth_write
    {
        return None;
    }
    let overlay = &stages[2];
    if overlay.blend != Some(materials::BlendFunc {
        src: BlendFactor::SrcAlpha,
        dst: BlendFactor::OneMinusSrcAlpha,
    }) || !matches!(overlay.tc_gen, TcGen::Base)
        || !matches!(overlay.rgb_gen, RgbGen::Identity)
        || !matches!(overlay.alpha_gen, AlphaGen::Vertex)
        || overlay.opacity != 1.0 || overlay.color != [1.0; 3]
    {
        return None;
    }
    if let StageTexture::Image(image) = overlay.texture {
        Some(image)
    } else {
        None
    }
}

pub(in crate::scene) fn can_fold_openjk_reverse_lightmap_modulate_pair(
    lightmap_stage: &MaterialStage,
    material_stage: &MaterialStage,
) -> bool {
    if material_stage.blend
        != Some(materials::BlendFunc {
            src: BlendFactor::Zero,
            dst: BlendFactor::SrcColor,
        })
    {
        return false;
    }
    let mut forward_modulate = material_stage.clone();
    forward_modulate.blend = Some(materials::BlendFunc {
        src: BlendFactor::DstColor,
        dst: BlendFactor::Zero,
    });
    can_fold_jka_lightmap_pair(lightmap_stage, &forward_modulate)
        && !lightmap_stage.glow
        && !material_stage.glow
        && !lightmap_stage.depth_write
        && !material_stage.depth_write
        && !has_stage_enhancements(lightmap_stage)
        && !has_stage_enhancements(material_stage)
        && material_stage.tc_mods.is_empty()
}

/// A sky surface is the outer box (discarded by the shader when the sky has
/// none) followed by one cloud batch per stage of the sky shader. The engine
/// draws them in that order, with each stage's coordinates generated from the
/// view direction. Ordinary same-surface batch ordering keeps the box first
/// and the stages in authored order.
pub(crate) fn append_sky_batches(
    out: &mut Vec<DrawBatch>,
    material: &SurfaceMaterial,
    bsp_shader_index: Option<usize>,
    material_debug_index: Option<usize>,
    vertex_lit: bool,
    range: Range<u32>,
    lightmap: Option<usize>,
    pvs_signature: &[u64],
    area_signature: [u64; 4],
    fog: [f32; 4],
    fog_is_global: bool,
) {
    let box_stage = MaterialStage {
        texture: StageTexture::White,
        animation: None,
        enhancements: Default::default(),
        blend: None,
        alpha_cutoff: 0.0,
        opacity: 1.0,
        color: [1.0; 3],
        rgb_gen: RgbGen::Identity,
        alpha_gen: AlphaGen::Identity,
        tc_gen: TcGen::Base,
        tc_mods: Vec::new(),
        depth_write: true,
        depth_equal: false,
            glow: false,
    };
    out.push(stage_batch(
        material,
        &box_stage,
        true,
        vertex_lit,
        range.clone(),
        bsp_shader_index,
        material_debug_index,
        lightmap,
        pvs_signature,
        area_signature,
        fog,
        fog_is_global,
    ));
    for (stage_index, stage) in material.stages.iter().enumerate() {
        let mut cloud = stage.clone();
        cloud.tc_gen = TcGen::SkyCloud(material.sky_cloud_height);
        out.push(stage_batch(
            material,
            &cloud,
            stage_index == 0,
            vertex_lit,
            range.clone(),
            bsp_shader_index,
            material_debug_index,
            lightmap,
            pvs_signature,
            area_signature,
            fog,
            fog_is_global,
        ));
    }
}


/// Recovery for BSPs whose *entire* masked base is one 8-bit color step below
/// GE128, yet whose later authored stages require depthFunc equal. Without a
/// depth seed, the complete material is guaranteed invisible, regardless of
/// its valid texture alpha. Keep vertex RGB and all subsequent stage alphas.
///
/// This is deliberately NOT an alphaGen parser override. Explicit alphaGen
/// vertex and all ordinary vertex-alpha blends remain intact; this path is
/// selected for a whole BSP draw group only when the masked base otherwise
/// cannot pass, and only with dependent depth-equal stages.
pub(in crate::scene) fn recover_impossible_masked_depth_seed(
    batches: &mut [DrawBatch], geometry: &[GpuVertex],
) {
    if geometry.is_empty() || !batches.iter().any(|batch| batch.pipeline.depth_equal) {
        return;
    }
    for (index, stage) in batches.iter_mut().enumerate() {
        // All authored stages in this group share the same BSP geometry. Only
        // the depth-writing base can require rescue, never its blended siblings.
        if index != 0 || !stage.pipeline.depth_write
            || stage.pipeline.class != DrawClass::Mask
            || stage.alpha_cutoff != 0.5
            || stage.rgb_gen != RgbGen::Vertex
            || !matches!(stage.alpha_gen, AlphaGen::Vertex | AlphaGen::Identity)
            || stage.color[3] < 0.999
        {
            continue;
        }
        // 127/255 versus GE128 is the characteristic 8-bit boundary case.
        // Do not 'fix' meaningful fades (0.0, 0.25, etc.) or mixed alpha draws.
        let near_boundary = geometry.iter().all(|vertex| {
            let alpha = vertex.color[3];
            alpha >= (127.0 / 255.0) - 1e-5 && alpha < 0.5
        });
        if near_boundary {
            stage.alpha_test_texture_only = true;
        }
    }
}


fn additive_affine_tcmods(batch: &DrawBatch) -> bool {
    batch.tc_mods.len() <= 4 && batch.tc_mods.iter().all(|m| match *m {
        TcMod::Scroll(x, y) => x.is_finite() && y.is_finite(),
        TcMod::Scale(x, y) => x.is_finite() && y.is_finite() && x.abs() > 0.00001 && y.abs() > 0.00001,
        TcMod::Rotate(x) => x.is_finite(),
        TcMod::Transform(v) => v.iter().all(|x| x.is_finite())
            && (v[0] * v[3] - v[1] * v[2]).abs() > 0.00001,
        _ => false,
    })
}

fn additive_same_tcgen(a: &DrawBatch, b: &DrawBatch) -> bool {
    match (a.tc_gen, b.tc_gen) {
        (TcGen::Base, TcGen::Base) => true,
        (TcGen::Vector(sa, ta), TcGen::Vector(sb, tb)) => sa == sb && ta == tb,
        _ => false,
    }
}

fn additive_candidate(b: &DrawBatch) -> bool {
    b.pipeline.blend == BlendMode::Custom(BlendFactor::SrcAlpha, BlendFactor::One)
        && b.pipeline.class == DrawClass::Transparent
        && !b.pipeline.depth_write && !b.pipeline.offset && !b.pipeline.auto_sprite2
        && !b.pipeline.legacy_srgb_three_stage
        // Never re-fuse an already fused primary: that would overwrite its
        // companions rather than extend the authored stage sequence.
        && b.pipeline.additive_fusion_count == 0 && b.additive_companions.is_empty()
        && b.alpha_cutoff == 0.0 && b.animation.is_none()
        && b.texture.is_some() && !b.texture_is_white && !b.texture_is_lightmap
        // Automatic Rend2 companion-map discovery does not change transparent
        // JKA stages: material_uniform() emits the PBR map flags only for
        // opaque, unblended surfaces. In this SRC_ALPHA/ONE path the companion
        // textures are NOT sampled. Their mere presence must not veto fusion.
        // Preserve all actually-used state tests below.
        && b.legacy_srgb_overlay.is_none()
        && b.fog[3] <= 0.001 && !b.planar_reflection
        && !b.planar_environment_candidate && !b.modulate_lightmap
        && !b.dlight_in_lightmap_stage && !b.detail_texture_eligible
        && additive_affine_tcmods(b)
}

/// Exact eligibility explanation shared by preparation and Trace Inspector.
/// None means the pair is eligible; a separate max-three-stage limit may still
/// split an eligible run. Keep this in one place to avoid misleading diagnostics.
pub(crate) fn additive_fusion_rejection_reason(a: &DrawBatch, b: &DrawBatch) -> Option<&'static str> {
    if !additive_candidate(a) || !additive_candidate(b) {
        return Some("non-additive blend, depth/alpha state, texture animation, fog, PBR or non-affine tcMod");
    }
    if a.authored_glow != b.authored_glow {
        return Some("authored glow boundary (glow-only framebuffer replay)");
    }
    if a.vertices != b.vertices || a.bsp_shader_index != b.bsp_shader_index {
        return Some("different BSP surface geometry or shader");
    }
    if a.pipeline.cull != b.pipeline.cull || a.pipeline.depth_equal != b.pipeline.depth_equal {
        return Some("different culling or depth-test state");
    }
    if a.fog_color_override != b.fog_color_override
        || a.shader_time_offset != b.shader_time_offset
        || a.shader_clamp_time != b.shader_clamp_time
        || a.water != b.water || a.vertex_lit != b.vertex_lit
    {
        return Some("different fog, shader time, water or vertex-lighting state");
    }
    if !additive_same_tcgen(a, b) {
        return Some("incompatible texture coordinate generators");
    }
    None
}

fn additive_compatible(a: &DrawBatch, b: &DrawBatch) -> bool {
    additive_fusion_rejection_reason(a, b).is_none()
}

fn fuse_adjacent_additive_batches(out: &mut Vec<DrawBatch>, start: usize) {
    let mut i = start;
    while i + 1 < out.len() {
        if !additive_compatible(&out[i], &out[i + 1]) {
            i += 1;
            continue;
        }
        let mut end = i + 2;
        while end < out.len() && end - i < 3 && additive_compatible(&out[i], &out[end]) {
            end += 1;
        }
        let companions: Vec<_> = out.drain(i + 1..end).collect();
        out[i].pipeline.additive_fusion_count = (companions.len() + 1) as u8;
        out[i].additive_companions = companions;
        i += 1;
    }
}

/// A cheap cold-path gate: ordinary prepared maps are already fused, and
/// must not clone their huge PVS batch vectors merely to rediscover that.
pub(in crate::scene) fn has_unfused_additive_batches(source: &[DrawBatch]) -> bool {
    source.windows(2).any(|pair| additive_compatible(&pair[0], &pair[1]))
}

/// Cold-path recovery for maps or remap caches holding unfused stages even
/// though the live setting asks for fusion. Do not modify already fused draws.
/// No shader-name cases and no work in the renderer's frame loop.
pub(in crate::scene) fn fuse_prepared_additive_batches(source: &[DrawBatch]) -> (Vec<DrawBatch>, usize) {
    let mut draws = source.to_vec();
    fuse_adjacent_additive_batches(&mut draws, 0);
    let saved = source.len() - draws.len();
    (draws, saved)
}

pub(in crate::scene) fn append_material_batches(
    out: &mut Vec<DrawBatch>,
    material: &SurfaceMaterial,
    bsp_shader_index: usize,
    material_debug_index: Option<usize>,
    vertex_lit: bool,
    range: Range<u32>,
    lightmap: Option<usize>,
    pvs_signature: &[u64],
    area_signature: [u64; 4],
    fog: [f32; 4],
    fog_is_global: bool,
) {
    let start = out.len();
    append_material_batches_unflagged(
        out,
        material,
        bsp_shader_index,
        material_debug_index,
        vertex_lit,
        range,
        lightmap,
        pvs_signature,
        area_signature,
        fog,
        fog_is_global,
    );
    // Fold only strictly equivalent adjacent SRC_ALPHA/ONE passes: no depth writes,
    // no alpha discard, identical depth/glow/fog and affine texture generators.
    // This is material-agnostic: ocean, fish caustics and terrain overlays all
    // use the same eligibility test. The world fragment shader evaluates every
    // member in original order in one draw, preserving independent tcMods.
    fuse_adjacent_additive_batches(out, start);
    // `dlight_in_lightmap_stage` is derived from the authored stages, but the
    // `$lightmap` stage can be folded into the diffuse pass (GL_DST_COLOR pair)
    // or replaced by vertex color. With no lightmap-stage batch left, the
    // shader would skip the base-stage light and nothing would add it: the
    // surface would ignore every runtime/per-pixel light.
    let emitted = &mut out[start..];
    if !emitted.iter().any(|batch| batch.texture_is_lightmap) {
        for batch in emitted.iter_mut() {
            batch.dlight_in_lightmap_stage = false;
        }
    }
    // Lightmap folding only merges stages at the front, so the authored stages
    // after the overlay map 1:1 onto the tail of the emitted batches. Confirm
    // the batch really is the alpha overlay before moving it behind clouds.
    if let Some(trailing) = lit_overlay_trailing_stages(material) {
        if trailing <= emitted.len() {
            let tail = emitted.len() - trailing;
            if emitted[tail].pipeline.blend == LIT_OVERLAY_BLEND {
                for batch in &mut emitted[tail..] {
                    batch.opaque_composite_before_clouds = false;
                    batch.lit_overlay = true;
                }
            }
        }
    }
}

pub(in crate::scene) fn append_material_batches_unflagged(
    out: &mut Vec<DrawBatch>,
    material: &SurfaceMaterial,
    bsp_shader_index: usize,
    material_debug_index: Option<usize>,
    vertex_lit: bool,
    range: Range<u32>,
    lightmap: Option<usize>,
    pvs_signature: &[u64],
    area_signature: [u64; 4],
    fog: [f32; 4],
    fog_is_global: bool,
) {
    if material.sky {
        append_sky_batches(
            out,
            material,
            Some(bsp_shader_index),
            material_debug_index,
            vertex_lit,
            range,
            lightmap,
            pvs_signature,
            area_signature,
            fog,
            fog_is_global,
        );
        return;
    }

    let stages = prepared_stages(material, vertex_lit);
    if stages.is_empty() {
        return;
    }

    // Keep the overwhelmingly common implicit BSP material in one draw call.
    if !material.explicit && !vertex_lit {
        let mut batch = stage_batch(
            material,
            &stages[0],
            true,
            vertex_lit,
            range,
            Some(bsp_shader_index),
            material_debug_index,
            lightmap,
            pvs_signature,
            area_signature,
            fog,
            fog_is_global,
        );
        batch.modulate_lightmap = lightmap.is_some();
        out.push(batch);
        return;
    }

    // A Rend2/PBR override can replace the original lightmapped diffuse stage
    // without carrying an explicit `$lightmap` pass. Preserve classic explicit
    // shader semantics in general, but for a PBR-enhanced identity base stage
    // the BSP lightmap is still the map's baked diffuse lighting source.
    if !vertex_lit
        && lightmap.is_some()
        && material.explicit
        && !stages
            .iter()
            .any(|stage| matches!(stage.texture, StageTexture::Lightmap))
        && stages.iter().any(enhanced_identity_base_stage)
    {
        for (stage_index, stage) in stages.iter().enumerate() {
            let mut batch = stage_batch(
                material,
                stage,
                stage_index == 0,
                vertex_lit,
                range.clone(),
                Some(bsp_shader_index),
                material_debug_index,
                lightmap,
                pvs_signature,
                area_signature,
                fog,
                fog_is_global,
            );
            if enhanced_identity_base_stage(stage) {
                batch.modulate_lightmap = true;
            }
            out.push(batch);
        }
        return;
    }

    if !vertex_lit
        && lightmap.is_some()
        && stages.len() >= 2
        && can_fold_jka_lightmap_pair(&stages[0], &stages[1])
    {
        let mut material_stage = stages[1].clone();
        material_stage.blend = None;
        let mut batch = stage_batch(
            material,
            &material_stage,
            true,
            vertex_lit,
            range.clone(),
            Some(bsp_shader_index),
            material_debug_index,
            lightmap,
            pvs_signature,
            area_signature,
            fog,
            fog_is_global,
        );
        batch.modulate_lightmap = true;
        // Three authored stages, but one opaque draw: reproduce the exact
        // encoded-RGB stage equation (validated against TaystJK with #12).
        // Ordinary materials stay on their original pipeline, with no pow().
        if let Some(overlay_image) = legacy_srgb_three_stage_overlay(material, &stages, fog) {
            batch.legacy_srgb_overlay = Some(overlay_image);
            batch.pipeline.legacy_srgb_three_stage = true;
            // The composite is not a single diffuse stage; automatic detail
            // promotion would alter the authored third-stage semantics.
            batch.detail_texture_eligible = false;
        }
        let fused = batch.pipeline.legacy_srgb_three_stage;
        out.push(batch);

        // The prefix (lightmap + diffuse + vertex-alpha overlay) is now one
        // draw, but an arbitrary fourth or later stage must still execute.
        // This generalizes the existing three-stage fast path without changing
        // any unsupported additive/animated/depth-equal/glow passes.
        for stage in stages.iter().skip(if fused { 3 } else { 2 }) {
            out.push(stage_batch(
                material,
                stage,
                false,
                vertex_lit,
                range.clone(),
                Some(bsp_shader_index),
                material_debug_index,
                lightmap,
                pvs_signature,
                area_signature,
                fog,
                fog_is_global,
            ));
        }
        return;
    }

    // Retain the two authored passes in the pristine cached PreparedMap.
    // A map-local reverse-modulation marker allows a *runtime* cvar change to
    // derive a one-pass recipe without reparsing shaders or modifying the
    // original cache. Do not fold on the map worker: it cannot know the
    // live cvar and the unfused variant must remain recoverable.
    let reverse_foldable = !vertex_lit
        && lightmap.is_some()
        && stages.len() >= 2
        && !material.sky
        && !material.water
        && !material.planar_reflection
        && fog[3] <= 0.001
        && can_fold_openjk_reverse_lightmap_modulate_pair(&stages[0], &stages[1]);
    for (stage_index, stage) in stages.iter().enumerate() {
        let mut batch = stage_batch(
            material,
            stage,
            stage_index == 0,
            vertex_lit,
            range.clone(),
            Some(bsp_shader_index),
            material_debug_index,
            lightmap,
            pvs_signature,
            area_signature,
            fog,
            fog_is_global,
        );
        if reverse_foldable && stage_index < 2 {
            batch.reverse_lightmap_fold_role = (stage_index + 1) as u8;
            if stage_index == 1 {
                batch.reverse_lightmap_fold_detail = !material.authored_detail;
            }
        }
        out.push(batch);
    }
}

#[cfg(test)]
mod cloud_composite_order_tests {
    use super::*;

    fn base_stage() -> MaterialStage {
        MaterialStage {
            texture: StageTexture::White,
            animation: None,
            enhancements: Default::default(),
            blend: None,
            alpha_cutoff: 0.0,
            opacity: 1.0,
            color: [1.0; 3],
            rgb_gen: RgbGen::Identity,
            alpha_gen: AlphaGen::Identity,
            tc_gen: TcGen::Base,
            tc_mods: Vec::new(),
            depth_write: false,
            depth_equal: false,
            glow: false,
        }
    }

    #[test]
    fn shore_surf_sheet_is_not_an_ocean_body_but_the_sea_is() {
        use jka_assets::shader::{Wave, WaveFunc};
        let wave = |base, amplitude| Wave {
            func: WaveFunc::Sin, base, amplitude, phase: 0.0, frequency: 0.09,
        };
        let stage = |rgb: RgbGen| MaterialStage { rgb_gen: rgb, ..base_stage() };
        // textures/sephtestmap/seph_surf: rgbGen wave sin 0 0.6 0.5 0.09
        let surf = SurfaceMaterial {
            water: true,
            stages: vec![stage(RgbGen::Wave(wave(0.0, 0.6))), stage(RgbGen::Wave(wave(0.0, 0.3)))],
            ..Default::default()
        };
        assert!(water_fades_to_nothing(&surf));
        let batch = stage_batch(&surf, &surf.stages[0], true, false, 0..3,
            Some(1), None, None, &[], [0; 4], [0.0; 4], false);
        assert!(!batch.water_primary, "overlay sheets keep their authored draw");
        // textures/sephtestmap/oceanwater: rgbGen wave sin 1 0.05 (never near zero),
        // and plain sea shaders with no generator at all.
        let sea = SurfaceMaterial {
            water: true,
            stages: vec![stage(RgbGen::Wave(wave(1.0, 0.05))), stage(RgbGen::Identity)],
            ..Default::default()
        };
        assert!(!water_fades_to_nothing(&sea));
        let batch = stage_batch(&sea, &sea.stages[0], true, false, 0..3,
            Some(2), None, None, &[], [0; 4], [0.0; 4], false);
        assert!(batch.water_primary);
        // Non-water materials are never reclassified.
        let wall = SurfaceMaterial { stages: surf.stages.clone(), ..Default::default() };
        assert!(!water_fades_to_nothing(&wall));
    }

    #[test]
    fn ge128_vertex_127_depth_equal_material_recovers_only_mask() {
        let mut base = base_stage();
        base.alpha_cutoff = 0.5;
        base.rgb_gen = RgbGen::Vertex;
        base.blend = Some(materials::BlendFunc {
            src: BlendFactor::One, dst: BlendFactor::Zero,
        });
        let mut caustic = base_stage();
        caustic.blend = Some(materials::BlendFunc {
            src: BlendFactor::SrcAlpha, dst: BlendFactor::One,
        });
        caustic.depth_equal = true;
        let material = SurfaceMaterial {
            stages: vec![base.clone(), caustic.clone()],
            sort: Some(15.0), explicit: true,
            ..Default::default()
        };
        let make_batch = |stage: &MaterialStage, first: bool| {
            stage_batch(&material, stage, first, false, 0..3,
                Some(114), None, None, &[], [0; 4], [0.0; 4], false)
        };
        let mut batches = vec![make_batch(&base, true), make_batch(&caustic, false)];
        let vertex = GpuVertex {
            position: [0.0; 3], uv: [0.0; 2], lightmap_uv: [0.0; 2],
            normal: [0.0, 1.0, 0.0], color: [1.0, 1.0, 1.0, 127.0 / 255.0],
            alpha_cutoff: 1.0,
        };
        recover_impossible_masked_depth_seed(&mut batches, &[vertex; 3]);
        assert!(batches[0].alpha_test_texture_only);
        assert!(!batches[1].alpha_test_texture_only);

        // Normal/intentional fades must retain their authored vertex alpha.
        batches[0].alpha_test_texture_only = false;
        let mut intentional_fade = vertex;
        intentional_fade.color[3] = 0.25;
        recover_impossible_masked_depth_seed(&mut batches, &[intentional_fade; 3]);
        assert!(!batches[0].alpha_test_texture_only);

        // The recovery is not applied when there is no dependent depth-equal pass.
        recover_impossible_masked_depth_seed(&mut batches[0..1], &[vertex; 3]);
        assert!(!batches[0].alpha_test_texture_only);
    }

    #[test]
    fn two_additive_wave_layers_with_one_minus_vertex_alpha_fuse() {
        // Models seph_surf without depending on the material name. Keep the
        // per-stage waveform, opposite scrolling, and vertex inverse alpha.
        let mut first = base_stage();
        first.texture = StageTexture::Image(14);
        first.blend = Some(materials::BlendFunc {
            src: BlendFactor::SrcAlpha, dst: BlendFactor::One,
        });
        first.alpha_gen = AlphaGen::OneMinusVertex;
        first.rgb_gen = RgbGen::Wave(jka_assets::shader::Wave {
            func: jka_assets::shader::WaveFunc::Sin,
            base: 0.0, amplitude: 0.6, phase: 0.5, frequency: 0.09,
        });
        first.tc_mods = vec![TcMod::Scale(3.0, 1.0), TcMod::Scroll(1.0, 0.0)];
        let mut second = first.clone();
        second.rgb_gen = RgbGen::Wave(jka_assets::shader::Wave {
            func: jka_assets::shader::WaveFunc::Sin,
            base: 0.0, amplitude: 0.3, phase: 0.0, frequency: 0.09,
        });
        second.tc_mods = vec![TcMod::Scale(3.0, 1.0), TcMod::Scroll(-0.6, 0.0)];
        let material = SurfaceMaterial {
            explicit: true, water: true, stages: vec![first, second],
            ..Default::default()
        };
        let mut draws = Vec::new();
        append_material_batches(&mut draws, &material, 10, None, false,
            0..48, None, &[], [0; 4], [0.0; 4], false);
        assert_eq!(draws.len(), 1);
        assert_eq!(draws[0].pipeline.additive_fusion_count, 2);
        assert_eq!(draws[0].additive_companions.len(), 1);
        assert_eq!(draws[0].additive_companions[0].alpha_gen, AlphaGen::OneMinusVertex);
        assert_ne!(draws[0].rgb_gen, draws[0].additive_companions[0].rgb_gen);
        assert_ne!(draws[0].tc_mods, draws[0].additive_companions[0].tc_mods);
    }

    #[test]
    fn additive_ocean_fuses_two_regular_and_three_glow_layers() {
        let mut stages = (0..5).map(|i| {
            let mut stage = base_stage();
            stage.texture = StageTexture::Image(i);
            stage.blend = Some(materials::BlendFunc {
                src: BlendFactor::SrcAlpha, dst: BlendFactor::One,
            });
            stage.tc_mods = vec![TcMod::Scroll(0.01 * (i as f32 + 1.0), 0.025)];
            stage.glow = i >= 2;
            stage
        }).collect::<Vec<_>>();
        stages[3].tc_mods.push(TcMod::Scale(0.4, 0.4));
        stages[3].tc_mods.push(TcMod::Rotate(4.5));
        // This is loaded automatically for seph_water2.jpg, but transparent
        // material_uniform never samples it. It must not prevent fusion.
        stages[0].enhancements.emissive_texture = Some(77);
        let material = SurfaceMaterial {
            explicit: true, water: true, stages,
            ..Default::default()
        };
        let mut out = Vec::new();
        append_material_batches(&mut out, &material, 8, None, false, 0..6,
            None, &[], [0; 4], [0.0; 4], false);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].pipeline.additive_fusion_count, 2);
        assert_eq!(out[1].pipeline.additive_fusion_count, 3);
        assert!(!out[0].authored_glow);
        assert!(out[1].authored_glow);
        assert_eq!(
            additive_fusion_rejection_reason(&out[0].additive_companions[0], &out[1]),
            Some("authored glow boundary (glow-only framebuffer replay)"),
        );
        let restored = super::super::shader_remap::expand_additive_batches(&out);
        assert_eq!(restored.len(), 5);
        assert!(restored.iter().all(|b| b.pipeline.additive_fusion_count == 0));
        assert_eq!(restored.iter().map(|b| b.texture).collect::<Vec<_>>(),
            (0..5).map(Some).collect::<Vec<_>>());
        assert!(!restored[0].authored_glow && !restored[1].authored_glow);
        assert!(restored[2].authored_glow && restored[3].authored_glow && restored[4].authored_glow);
        // The cold-path re-fuser restores an expanded cached map, then becomes
        // a no-op on an already-fused copy (idempotent / no stage loss).
        assert!(has_unfused_additive_batches(&restored));
        let (refused, saved) = fuse_prepared_additive_batches(&restored);
        assert!(!has_unfused_additive_batches(&refused));
        assert_eq!(saved, 3);
        assert_eq!(refused.len(), 2);
        assert_eq!(refused[0].pipeline.additive_fusion_count, 2);
        assert_eq!(refused[1].pipeline.additive_fusion_count, 3);
        let (unchanged, saved_again) = fuse_prepared_additive_batches(&refused);
        assert_eq!(saved_again, 0);
        assert_eq!(unchanged.len(), 2);
        assert_eq!(unchanged[0].additive_companions.len(), 1);
        assert_eq!(unchanged[1].additive_companions.len(), 2);
    }

    #[test]
    fn additive_fish_respects_depth_and_glow_boundaries() {
        let mut base = base_stage();
        base.texture = StageTexture::Image(1);
        base.alpha_cutoff = 0.5;
        base.depth_write = true;
        let mut caustic = base_stage();
        caustic.texture = StageTexture::Image(2);
        caustic.blend = Some(materials::BlendFunc {
            src: BlendFactor::SrcAlpha, dst: BlendFactor::One,
        });
        caustic.depth_equal = true;
        caustic.tc_gen = TcGen::Vector([0.00390625, 0.0, 0.0], [0.0, 0.00390625, 0.0]);
        caustic.tc_mods = vec![TcMod::Scroll(-0.0075, 0.005)];
        let mut caustic2 = caustic.clone();
        caustic2.texture = StageTexture::Image(3);
        caustic2.tc_mods = vec![TcMod::Scroll(-0.0175, 0.025)];
        let mut caustic_glow = caustic.clone();
        caustic_glow.texture = StageTexture::Image(4);
        caustic_glow.glow = true;
        let mut fade = base_stage();
        fade.texture = StageTexture::Image(5);
        fade.depth_equal = true;
        fade.blend = Some(materials::BlendFunc {
            src: BlendFactor::SrcAlpha, dst: BlendFactor::OneMinusSrcAlpha,
        });
        let material = SurfaceMaterial {
            explicit: true, stages: vec![base, caustic, caustic2, caustic_glow, fade],
            ..Default::default()
        };
        let mut out = Vec::new();
        append_material_batches(&mut out, &material, 12, None, false, 0..6,
            None, &[], [0; 4], [0.0; 4], false);
        assert_eq!(out.len(), 4);
        assert_eq!(out[1].pipeline.additive_fusion_count, 2);
        assert!(out[1].pipeline.depth_equal);
        assert_eq!(out[2].pipeline.additive_fusion_count, 0);
        assert!(out[2].authored_glow);
        let restored = super::super::shader_remap::expand_additive_batches(&out);
        assert_eq!(restored.len(), 5);
        assert_eq!(restored[0].pipeline.depth_write, true);
        assert_eq!(restored[1].texture, Some(2));
        assert_eq!(restored[2].texture, Some(3));
        assert_eq!(restored[3].texture, Some(4));
        assert_eq!(restored[4].texture, Some(5));
        assert!(restored[1].pipeline.depth_equal && restored[2].pipeline.depth_equal);
        assert!(restored[3].authored_glow && !restored[2].authored_glow);
    }

    #[test]
    fn authored_opaque_terrain_stays_together_but_water_does_not() {
        let base = base_stage();
        let mut wet_sand = base.clone();
        wet_sand.blend = Some(materials::BlendFunc {
            src: BlendFactor::SrcAlpha,
            dst: BlendFactor::OneMinusSrcAlpha,
        });
        let mut caustics = base.clone();
        caustics.blend = Some(materials::BlendFunc {
            src: BlendFactor::SrcAlpha,
            dst: BlendFactor::One,
        });
        let terrain = SurfaceMaterial {
            stages: vec![base.clone(), wet_sand, caustics.clone()],
            ..Default::default()
        };
        assert!(opaque_composite_before_clouds(&terrain));
        for stage in &terrain.stages {
            assert!(stage_batch(&terrain, stage, false, false, 0..3,
                Some(109), None, None, &[], [0; 4], [0.0; 4], false)
                .opaque_composite_before_clouds);
        }

        let water = SurfaceMaterial { water: true, ..terrain.clone() };
        assert!(!opaque_composite_before_clouds(&water));
        let trans = SurfaceMaterial { authored_translucent: true, ..terrain.clone() };
        assert!(!opaque_composite_before_clouds(&trans));
        let sorted = SurfaceMaterial { sort: Some(9.0), ..terrain.clone() };
        assert!(!opaque_composite_before_clouds(&sorted));
        let fully_blended = SurfaceMaterial { stages: vec![caustics], ..terrain };
        assert!(!opaque_composite_before_clouds(&fully_blended));
    }

    #[test]
    fn explicit_late_sort_controls_surface_order_not_stage_alpha_mask() {
        let mut base = base_stage();
        base.alpha_cutoff = 0.5;
        base.blend = Some(materials::BlendFunc {
            src: BlendFactor::One, dst: BlendFactor::Zero,
        });
        let sorted = SurfaceMaterial {
            stages: vec![base.clone()],
            sort: Some(15.0),
            ..Default::default()
        };
        assert_eq!(class_for(&sorted), DrawClass::Transparent);
        assert_eq!(effective_material_sort(&sorted), 15 * 1024);
        // Per-stage pipeline still alpha-tests, writes depth and preserves
        // the authored GL_ONE GL_ZERO replacement blend.
        let pipeline = stage_pipeline(&sorted, &base, true);
        assert_eq!(pipeline.class, DrawClass::Mask);
        assert!(pipeline.depth_write);
        assert_eq!(pipeline.blend, BlendMode::Opaque);
        let unsorted = SurfaceMaterial { sort: None, ..sorted };
        assert_eq!(class_for(&unsorted), DrawClass::Mask);
        assert_eq!(effective_material_sort(&unsorted), 3 * 1024);
    }

    #[test]
    fn late_sorted_cutouts_are_surfaces_but_late_blends_are_not() {
        // costadelsol seaweed/fish: `sort additive`, GE128 depthWrite base.
        let mut cutout = base_stage();
        cutout.alpha_cutoff = 0.5;
        cutout.depth_write = true;
        let seaweed = SurfaceMaterial {
            sort: Some(15.0), stages: vec![cutout.clone()], ..Default::default()
        };
        assert!(late_sorted_depth_seed(&seaweed));
        let unsorted = SurfaceMaterial { sort: None, ..seaweed.clone() };
        assert!(!late_sorted_depth_seed(&unsorted));
        let additive_fx = MaterialStage {
            blend: Some(materials::BlendFunc { src: BlendFactor::SrcAlpha, dst: BlendFactor::One }),
            alpha_cutoff: 0.0,
            depth_write: false,
            ..cutout
        };
        let glow = SurfaceMaterial { sort: Some(15.0), stages: vec![additive_fx], ..Default::default() };
        assert!(!late_sorted_depth_seed(&glow));
    }

    #[test]
    fn lit_alpha_overlay_after_lightmap_goes_behind_clouds() {
        // costadelsol terrain_sand2wetsand: sand, wet-sand splat, lightmap,
        // caustics, depthfade height haze.
        let base = base_stage();
        let alpha = Some(materials::BlendFunc {
            src: BlendFactor::SrcAlpha,
            dst: BlendFactor::OneMinusSrcAlpha,
        });
        let wet_sand = MaterialStage { blend: alpha, ..base.clone() };
        let lightmap = MaterialStage {
            texture: StageTexture::Lightmap,
            blend: Some(materials::BlendFunc {
                src: BlendFactor::DstColor,
                dst: BlendFactor::Zero,
            }),
            ..base.clone()
        };
        let caustics = MaterialStage {
            blend: Some(materials::BlendFunc {
                src: BlendFactor::SrcAlpha,
                dst: BlendFactor::One,
            }),
            ..base.clone()
        };
        let depthfade = MaterialStage { blend: alpha, ..base.clone() };
        let terrain = SurfaceMaterial {
            stages: vec![base, wet_sand.clone(), lightmap.clone(), caustics, depthfade],
            ..Default::default()
        };
        assert_eq!(lit_overlay_trailing_stages(&terrain), Some(1));
        // Only the haze batch is flagged: it follows the cloud multiplier but
        // must still draw before the ocean optics capture with its surface.
        let mut out = Vec::new();
        let explicit_terrain = SurfaceMaterial { explicit: true, ..terrain.clone() };
        append_material_batches(&mut out, &explicit_terrain, 12, None, false, 0..6,
            None, &[], [0; 4], [0.0; 4], false);
        let last = out.len() - 1;
        assert!(out[last].lit_overlay && !out[last].opaque_composite_before_clouds);
        assert!(out[..last].iter().all(|b| !b.lit_overlay && b.opaque_composite_before_clouds));
        // The pre-lightmap wet-sand splat is albedo and must stay in front.
        let splat_only = SurfaceMaterial {
            stages: vec![base_stage(), wet_sand, lightmap],
            ..Default::default()
        };
        assert_eq!(lit_overlay_trailing_stages(&splat_only), None);
    }
}
