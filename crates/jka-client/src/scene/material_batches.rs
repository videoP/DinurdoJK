//! Materials.
use crate::scene::{
    bsp_fog_params, material_legacy2_in_stage_safe, materials, stage_fog_color_override, AlphaGen,
    BTreeMap, BlendFactor, BlendMode, Bsp, CullMode, DrawBatch, DrawClass, MaterialDebugEntry,
    MaterialDebugInfo, MaterialStage, PipelineKey, Range, RgbGen, Shader, StageTexture,
    SurfaceMaterial, TcGen, Textures,
};

pub(in crate::scene) fn stage_is_opaque_replacement(stage: &MaterialStage) -> bool {
    stage.blend
        == Some(materials::BlendFunc {
            src: BlendFactor::One,
            dst: BlendFactor::Zero,
        })
}

pub(in crate::scene) fn stage_class(stage: &MaterialStage) -> DrawClass {
    // GL_ONE GL_ZERO is authored through blendFunc, but it does not actually
    // blend with the framebuffer: src * 1 + dst * 0 is a full replacement.
    // Keep world-material semantics aligned with the FX/player paths so a
    // replacement base is an opaque/depth-seeding pass instead of letting
    // later translucent surfaces show through it.
    if stage.blend.is_some() && !stage_is_opaque_replacement(stage) {
        DrawClass::Transparent
    } else if stage.alpha_cutoff > 0.0 {
        DrawClass::Mask
    } else {
        DrawClass::Opaque
    }
}

pub(in crate::scene) fn class_for(material: &SurfaceMaterial) -> DrawClass {
    if material.sky {
        DrawClass::Sky
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
        && stage.alpha_cutoff <= 0.0
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
        texture,
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
        alpha_shadow: material.alpha_shadow,
        light_filter: material.light_filter,
        water_primary: material.water && first,
        authored_ocean: None,
        planar_environment_candidate: matches!(stage.tc_gen, TcGen::Environment),
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

pub(in crate::scene) fn build_material_debug_entry(
    name: &str,
    definition: Option<&Shader>,
    origin: Option<&materials::ShaderDefinitionOrigin>,
    material: &SurfaceMaterial,
    textures: &Textures,
) -> MaterialDebugEntry {
    let mut lines = Vec::new();
    if let Some(definition) = definition {
        let stage_unsupported = definition
            .stages
            .iter()
            .map(|stage| stage.unsupported.len())
            .sum::<usize>();
        let unsupported = definition.unsupported.len() + stage_unsupported;
        lines.push(format!(
            "parsed shader: stages={} portal={} sky={} nodraw={} translucent={} nofog={} cull={} unsupported={} (shader={} stage={})",
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
            "resolved grass emitter: GodotGrass per-blade path height={:.1} JKA-width-hint={:.1} density={:.1} fade={:.1}..{:.1} wind={:.2}; legacy sprite card suppressed",
            grass.height,
            grass.authored_width,
            grass.density,
            grass.fade_dist,
            grass.fade_max,
            grass.wind,
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
            "{} surface emitter: value={:.1} color={:.3},{:.3},{:.3} subdivide={:.1}",
            if surface_light.inferred_from_emissive {
                "emissive-texture inferred"
            } else {
                "q3map authored"
            },
            surface_light.value,
            surface_light.color[0],
            surface_light.color[1],
            surface_light.color[2],
            surface_light.subdivide,
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
        image_sources,
        lines,
    }
}

pub(in crate::scene) fn material_debug_info(
    library_debug: &materials::ShaderLibraryDiagnostics,
    names_and_materials: impl IntoIterator<Item = (String, SurfaceMaterial)>,
    library: &BTreeMap<String, Shader>,
    textures: &Textures,
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
    // `dlight_in_lightmap_stage` is derived from the authored stages, but the
    // `$lightmap` stage can be folded into the diffuse pass (GL_DST_COLOR pair)
    // or replaced by vertex color. With no lightmap-stage batch left, the
    // shader would skip the base-stage light and nothing would add it: the
    // surface would ignore every runtime/per-pixel light.
    let emitted = &mut out[start..];
    if !emitted.iter().any(|batch| batch.texture_is_lightmap) {
        for batch in emitted {
            batch.dlight_in_lightmap_stage = false;
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
        out.push(batch);

        for stage in stages.iter().skip(2) {
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

    // Other explicit shader scripts remain true ordered passes.
    for (stage_index, stage) in stages.iter().enumerate() {
        out.push(stage_batch(
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
        ));
    }
}
