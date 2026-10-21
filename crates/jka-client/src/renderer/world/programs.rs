//! Stage program policies. Classification happens during compilation, never per pixel.
use crate::renderer::{
    MaterialFragmentKey, MaterialUniform, PipelineKey, WorldShaderFamily, WorldShaderVariantKey,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(in crate::renderer) enum WorldProgramMode {
    #[default]
    Specialized,
    Shared,
    Generated,
}

impl WorldProgramMode {
    pub fn from_console(mode: u8) -> Self {
        match mode {
            3 => Self::Shared,
            4 => Self::Generated,
            _ => Self::Specialized,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(in crate::renderer) enum StageProgram {
    #[default]
    Specialized,
    /// Mode-1 specialization compiled from bsp_lean.wgsl for a dry stage while
    /// Hybrid Ocean holds the variant in the Enhanced family (see `dry_variant`).
    SpecializedLean,
    Minimal,
    SharedBasic,
    SharedEnhanced,
    GeneratedBasic,
    GeneratedEnhanced,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(in crate::renderer) struct VertexRecipe {
    pub tc_mods: [u8; 4],
    pub stretch_funcs: [u8; 4],
    pub wave_funcs: [u8; 2],
    pub surface_material: u8,
}

impl VertexRecipe {
    pub fn from_uniform(material: &MaterialUniform) -> Self {
        let mut recipe = Self {
            surface_material: ((material.header[3] >> 8) & 31) as u8,
            ..Self::default()
        };
        for i in 0..(material.header[1] as usize).min(4) {
            recipe.tc_mods[i] = (material.mods[i * 2][0] + 0.5) as u8;
            if recipe.tc_mods[i] == 6 {
                recipe.stretch_funcs[i] = (material.mods[i * 2 + 1][1] + 0.5) as u8;
            }
        }
        if material.header[2] & 134217728 != 0 {
            recipe.wave_funcs[0] = material.wave_funcs[0] as u8;
        }
        if material.header[2] & 268435456 != 0 {
            recipe.wave_funcs[1] = material.wave_funcs[1] as u8;
        }
        recipe
    }
}

/// The scoped experimental feature envelope. Other configurations retain mode 1.
pub(in crate::renderer) fn supported_variant(v: WorldShaderVariantKey) -> bool {
    v.family() == WorldShaderFamily::Lean
        && !v.source_map_world
        && !v.map_light_simulation
        && !v.vertex_dlights
        && !v.clustered_lite_dlights
        && !v.local_shadows
        && !v.cascaded_shadows
        && !v.ray_traced_shadows
        && !v.ray_traced_sun
        && !v.weather_surface
        && !v.classic_render_flags
        && !v.planar_debug
        && !v.dynamic_light_debug
        && !v.sky_debug
}

/// Hybrid Ocean promotes the whole variant to the Enhanced family (bsp.wgsl and
/// its pipeline layout), yet only the reserved ocean pipeline shades ocean
/// geometry: bsp.wgsl recognises it per vertex (`is_ocean_vertex`) and world
/// batches never contain clipmap vertices. Every other material stage can use
/// the Lean family's equations with the ocean compiled out. The Lean bindings are
/// a subset of the Enhanced layout (identical groups 0/1/3/4, group 2 entries
/// 0-9/16/17), so those programs are built against the Enhanced layout and draw
/// with the frame's existing bind groups. RT variants keep their own modules.
pub(in crate::renderer) fn dry_variant(v: WorldShaderVariantKey) -> Option<WorldShaderVariantKey> {
    if !v.ocean || v.ray_traced_shadows {
        return None;
    }
    let dry = WorldShaderVariantKey { ocean: false, ..v };
    (dry.family() == WorldShaderFamily::Lean).then_some(dry)
}

/// Whether a stage can reach the baked-sun/indirect relight code: an explicit
/// `$lightmap` stage, the implicit lightmap multiply, or a vertex-lit stage
/// using vertex colour. Everything else compiles ENABLE_BAKED_SUN_RELIGHT out.
fn baked_relight_live(m: MaterialFragmentKey) -> bool {
    m.flags & (2 | 8388608) != 0 || (m.flags & 131072 != 0 && m.flags & 1 != 0)
}

/// Exact material-side eliminations in the main base pass. Reflection/fog/dlight
/// redraws retain their original pipelines, including clipping and deformation.
pub(in crate::renderer) fn effective_stage_variant(
    mut v: WorldShaderVariantKey,
    m: MaterialFragmentKey,
) -> WorldShaderVariantKey {
    v = v.base_pass_key();
    if m.flags & (8 | 16 | 32 | 64 | 128 | 256 | 512 | 1024 | 2048 | 262144) == 0 {
        v.pbr = false;
        v.pom = false;
        v.deluxe = false;
        v.deluxe_specular = false;
        v.pbr_shared_material_eval = false;
        v.pbr_shared_tangent_frame = false;
        v.pom_mip_aware = false;
        v.pom_adaptive_steps = false;
        v.pbr_companion_sampler = false;
        v.pbr_vertex_lightgrid = false;
    }
    if m.flags_w & 2 == 0 {
        v.detail_textures_enabled = false;
    }
    if m.flags & 4096 == 0 && m.tc_gen != 3 {
        v.planar_reflections = false;
    }
    if m.vertex_recipe.surface_material != 14 {
        v.surface_deformation = false;
    }
    if !baked_relight_live(m) {
        v.baked_sun_relight = false;
    }
    v
}

fn basic(v: WorldShaderVariantKey) -> bool {
    !v.pbr
        && !v.pom
        && !v.deluxe
        && !v.detail_textures_enabled
        && !v.planar_reflections
        && !v.surface_deformation
        && !v.baked_sun_relight
}

pub(in crate::renderer) fn program_key(
    v: WorldShaderVariantKey,
    p: PipelineKey,
    m: MaterialFragmentKey,
) -> MaterialFragmentKey {
    if let Some(dry) = dry_variant(v) {
        let mut key = program_key(dry, p, m);
        if key.program == StageProgram::Specialized {
            key.program = StageProgram::SpecializedLean;
        }
        return key;
    }
    let alpha = p.class != crate::renderer::DrawClass::Opaque;
    // Glass shading exists only in fs_main; the minimal, shared and generated
    // entries would draw it as an opaque stage.
    let glass = m.flags_w & super::variants::MATERIAL_FLAGS_W_GLASS != 0;
    let ordinary = !p.auto_sprite2 && !p.legacy_srgb_three_stage && m.tc_gen <= 3 && !glass;
    // A uniform-driven shared relight measured 4-13% slower than mode-1
    // specialization (RTX 3060, costadelsol/ffa3): keep it specialized in mode 3.
    let shared_relight = v.program_mode == WorldProgramMode::Shared
        && v.baked_sun_relight && baked_relight_live(m);
    if !supported_variant(v) || !ordinary || shared_relight
        || v.program_mode == WorldProgramMode::Specialized
    {
        let mut key = m.for_variant(v, alpha);
        if !glass && super::pipelines::material_minimal_entry(v, &p) {
            key.program = StageProgram::Minimal;
        }
        return key;
    }
    let effective = effective_stage_variant(v, m);
    match v.program_mode {
        WorldProgramMode::Shared => MaterialFragmentKey {
            // Zero masks leave every material option uniform-driven. Families
            // differ only by the operations their global variant compiles in.
            tc_gen: u32::MAX,
            program: if basic(effective) {
                StageProgram::SharedBasic
            } else {
                StageProgram::SharedEnhanced
            },
            ..MaterialFragmentKey::default()
        },
        WorldProgramMode::Generated => {
            let mut key = m.for_variant(effective, alpha);
            key.tc_gen = m.tc_gen;
            key.vertex_recipe = m.vertex_recipe;
            if !effective.surface_deformation {
                key.vertex_recipe.surface_material = 0;
            }
            key.program = if basic(effective) {
                StageProgram::GeneratedBasic
            } else {
                StageProgram::GeneratedEnhanced
            };
            key
        }
        WorldProgramMode::Specialized => unreachable!(),
    }
}

/// Diagnostic counterpart to the actual mode selection predicates. This runs
/// only for `/trace` and the opt-in FPS graph, never in the pixel/draw hot path.
/// All applicable blockers are returned, rather than the first generic reason.
/// `mode` is an explicit hypothetical 2/3/4; the caller may inspect eligibility
/// for all three without recompiling or changing the active renderer state.
pub(in crate::renderer) fn stage_rejection_reasons(
    v: WorldShaderVariantKey,
    p: PipelineKey,
    m: MaterialFragmentKey,
    mode: u8,
) -> Vec<&'static str> {
    let mut reasons = Vec::new();
    if !matches!(mode, 2..=4) {
        return reasons;
    }
    // Same delegation as program_key: dry stages are judged without the ocean.
    let v = dry_variant(v).unwrap_or(v);
    // These batches never enter the per-material PSO table at all, even in
    // mode 1. Check these before the global variant for useful map summaries.
    if p.class == crate::renderer::DrawClass::Sky {
        reasons.push("sky uses its own shader path");
    }
    if p.additive_fusion_count >= 2 {
        reasons.push("fused additive stage uses its own shader path");
    }
    if p == crate::renderer::ocean_pipeline_key() {
        reasons.push("reserved ocean pipeline uses its own shader path");
    }
    if mode == 2 {
        if !v.material_specialization || !v.minimal_entries {
            reasons.push("mode 2 / material specialization not enabled on this variant");
        }
        if v.family() != WorldShaderFamily::Lean {
            reasons.push("world shader family is Enhanced (not Lean)");
        }
        macro_rules! exclude { ($field:ident, $reason:literal) => {
            if v.$field { reasons.push($reason); }
        }; }
        exclude!(pbr, "PBR enabled");
        exclude!(pom, "POM enabled");
        exclude!(point_lights, "point lighting enabled");
        exclude!(map_light_simulation, "map light simulation enabled");
        exclude!(source_map_world, "Source-style map world enabled");
        exclude!(vertex_dlights, "vertex dynamic lighting enabled");
        exclude!(clustered_lite_dlights, "clustered-lite dynamic lighting enabled");
        exclude!(area_lights, "area lighting enabled");
        exclude!(irradiance_volume, "irradiance volume enabled");
        exclude!(voxel_gi, "voxel GI enabled");
        exclude!(local_shadows, "local shadows enabled");
        exclude!(cascaded_shadows, "cascaded shadows enabled");
        exclude!(ray_traced_shadows, "ray-traced shadows enabled");
        exclude!(planar_reflections, "planar reflections enabled");
        exclude!(ocean, "global ocean shader feature enabled");
        exclude!(weather_surface, "weather surface shading enabled");
        exclude!(baked_sun_relight, "baked-sun relighting enabled");
        exclude!(classic_render_flags, "classic render flags enabled");
        exclude!(planar_debug, "planar reflection debug enabled");
        exclude!(deluxe, "deluxe mapping enabled");
        exclude!(detail_textures_enabled, "detail textures enabled");
        exclude!(surface_deformation, "surface deformation enabled");
        exclude!(dynamic_light_debug, "dynamic-light debug enabled");
    } else {
        if v.family() != WorldShaderFamily::Lean {
            reasons.push("world shader family is Enhanced (not Lean)");
            // `supported_variant` only tests family(), so report *which*
            // feature promoted the family. This is especially important for
            // Hybrid Ocean: the promoted world family covers dry BSP too.
            if v.ocean { reasons.push("global ocean shading forces Enhanced family"); }
            if v.point_lights { reasons.push("point lighting forces Enhanced family"); }
            if v.area_lights { reasons.push("area lighting forces Enhanced family"); }
            if v.irradiance_volume { reasons.push("irradiance volume forces Enhanced family"); }
            if v.voxel_gi { reasons.push("voxel GI forces Enhanced family"); }
        }
        macro_rules! exclude { ($field:ident, $reason:literal) => {
            if v.$field { reasons.push($reason); }
        }; }
        // Must stay in sync with supported_variant(), not a broader list of
        // theoretically expensive features. PBR itself is allowed in 3/4.
        exclude!(source_map_world, "Source-style map world enabled");
        exclude!(map_light_simulation, "map light simulation enabled");
        exclude!(vertex_dlights, "vertex dynamic lighting enabled");
        exclude!(clustered_lite_dlights, "clustered-lite dynamic lighting enabled");
        exclude!(local_shadows, "local shadows enabled");
        exclude!(cascaded_shadows, "cascaded shadows enabled");
        exclude!(ray_traced_shadows, "ray-traced shadows enabled");
        exclude!(ray_traced_sun, "ray-traced sun enabled");
        exclude!(weather_surface, "weather surface shading enabled");
        exclude!(classic_render_flags, "classic render flags enabled");
        exclude!(planar_debug, "planar reflection debug enabled");
        exclude!(dynamic_light_debug, "dynamic-light debug enabled");
        exclude!(sky_debug, "sky debug shader enabled");
    }
    if p.auto_sprite2 {
        reasons.push("autosprite2 deformation uses its own entry point");
    }
    if p.legacy_srgb_three_stage {
        reasons.push("authored three-stage legacy sRGB composite");
    }
    if m.flags_w & super::variants::MATERIAL_FLAGS_W_GLASS != 0 {
        reasons.push("glass stage requires fs_main");
    }
    if mode >= 3 && m.tc_gen > 3 {
        reasons.push("special tcGen unsupported by shared/generated recipes");
    }
    if mode == 3 && v.baked_sun_relight && baked_relight_live(m) {
        reasons.push("baked relight on a lightmapped stage stays specialized in mode 3");
    }
    reasons
}

/// Static map-batch eligibility, *not* visibility-weighted actual draw counts.
/// One first/primary rejection per batch makes categories sum to blocked.
/// 0 total, 1 eligible, 2 global features, 3 sky, 4 fused, 5 ocean stage,
/// 6 autosprite2, 7 RGB3, 8 special tcGen, 9 glass, 10 other.
pub(in crate::renderer) fn map_eligibility_counts(
    world: &crate::renderer::WorldGpu,
) -> Option<[u32; 11]> {
    let variant = world.active_pipeline_variant;
    let mode = active_mode(variant);
    if !(2..=4).contains(&mode) { return None; }
    let mut counts = [0_u32; 11];
    for batch in &world.coarse_batches {
        counts[0] += 1;
        let p = batch.source.pipeline;
        let m = batch.fragment_key;
        let reasons = stage_rejection_reasons(variant, p, m, mode);
        if reasons.is_empty() {
            counts[1] += 1;
            continue;
        }
        let judged = dry_variant(variant).unwrap_or(variant);
        let category = if p.class == crate::renderer::DrawClass::Sky { 3 }
            else if p.additive_fusion_count >= 2 { 4 }
            else if p == crate::renderer::ocean_pipeline_key() { 5 }
            else if (mode == 2 && !judged.minimal_entry_compatible())
                || (mode >= 3 && !supported_variant(judged)) { 2 }
            else if p.auto_sprite2 { 6 }
            else if p.legacy_srgb_three_stage { 7 }
            else if mode >= 3 && m.tc_gen > 3 { 8 }
            else if m.flags_w & super::variants::MATERIAL_FLAGS_W_GLASS != 0 { 9 }
            else { 10 };
        counts[category] += 1;
    }
    Some(counts)
}

/// Whether a narrowed key's program uses vs/fs_world_minimal (decided by `program_key`).
pub(in crate::renderer) fn uses_minimal_entry(m: MaterialFragmentKey) -> bool {
    matches!(m.program, StageProgram::Minimal | StageProgram::SharedBasic)
}

pub(in crate::renderer) fn active_mode(v: WorldShaderVariantKey) -> u8 {
    if !v.material_specialization {
        0
    } else if v.minimal_entries {
        2
    } else {
        match v.program_mode {
            WorldProgramMode::Shared => 3,
            WorldProgramMode::Generated => 4,
            _ => 1,
        }
    }
}

pub(in crate::renderer) fn fallback_reason(
    v: WorldShaderVariantKey,
    p: PipelineKey,
    m: MaterialFragmentKey,
) -> &'static str {
    let v = dry_variant(v).unwrap_or(v);
    if !supported_variant(v) {
        "features outside Minimal/Low/Medium envelope"
    } else if p.auto_sprite2 {
        "autosprite2 vertex deformation"
    } else if p.legacy_srgb_three_stage {
        "authored three-stage RGB composite"
    } else if m.tc_gen > 3 {
        "special texture-coordinate generator"
    } else if m.flags_w & super::variants::MATERIAL_FLAGS_W_GLASS != 0 {
        "glass primary stage"
    } else if v.program_mode == WorldProgramMode::Shared && v.baked_sun_relight && baked_relight_live(m) {
        "baked relight on a lightmapped stage (mode 3)"
    } else {
        "existing special effect entry"
    }
}

pub(in crate::renderer) fn program_variant(
    v: WorldShaderVariantKey,
    m: MaterialFragmentKey,
) -> WorldShaderVariantKey {
    // Every program but the Enhanced-module specialization is Lean source.
    let v = if m.program == StageProgram::Specialized { v } else { dry_variant(v).unwrap_or(v) };
    match m.program {
        StageProgram::SharedBasic => {
            let empty = MaterialFragmentKey {
                tc_gen: 0,
                ..MaterialFragmentKey::default()
            };
            effective_stage_variant(v, empty)
        }
        StageProgram::GeneratedBasic | StageProgram::GeneratedEnhanced => {
            effective_stage_variant(v, m)
        }
        _ => v.base_pass_key(),
    }
}

/// Balanced-brace extraction keeps generated entries on the checked-in shading
/// equations. Missing anchors fail loudly instead of silently changing output.
fn function_body<'a>(source: &'a str, name: &str) -> &'a str {
    let start = source
        .find(&format!("fn {name}("))
        .expect("world shader function anchor");
    let open = start
        + source[start..]
            .find('{')
            .expect("world shader function body");
    let mut depth = 1;
    for (offset, byte) in source.as_bytes()[open + 1..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            return &source[open + 1..open + 1 + offset];
        }
    }
    panic!("unterminated world shader function {name}");
}

fn replace_function_body(source: &mut String, name: &str, replacement: &str) {
    let body = function_body(source, name);
    let start = body.as_ptr() as usize - source.as_ptr() as usize;
    let end = start + body.len();
    source.replace_range(start..end, replacement);
}

/// Reuse each authored tcMod equation but emit the actual sequence without a
/// uniform loop or opcode interpreter. Parameters stay in the material buffer.
fn specialized_tc_mods(source: &str, recipe: VertexRecipe) -> String {
    use std::fmt::Write;
    let original = function_body(source, "apply_tc_mods");
    let begin = original
        .find("        let a = material.mods")
        .expect("tcMod operation anchor");
    let end = original
        .rfind("    }\n    return uv;")
        .expect("tcMod loop end");
    let operation = &original[begin..end];
    let mut body = String::from("\n    var uv = base_uv;\n    let time = shader_stage_time();\n");
    for (i, kind) in recipe.tc_mods.into_iter().enumerate() {
        if kind == 0 {
            continue;
        }
        let anchor = format!("if (kind == {kind}u) {{");
        let start = operation.find(&anchor).expect("authored tcMod opcode");
        let open = start + anchor.len();
        let mut depth = 1;
        let mut close = open;
        for (offset, byte) in operation.as_bytes()[open..].iter().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                close = open + offset;
                break;
            }
        }
        let fixed = operation[open..close]
            .replace("u32(b.y + 0.5)", &format!("{}u", recipe.stretch_funcs[i]));
        let _ = writeln!(
            body,
            "    {{\nlet a = material.mods[{}u];\nlet b = material.mods[{}u];\n{fixed}\n    }}",
            i * 2,
            i * 2 + 1
        );
    }
    body.push_str("    return uv;\n");
    body
}

/// Source generation is bounded by encountered normalized recipes. The module
/// retains reusable helper definitions; only the generated entries are compiled.
pub(in crate::renderer) fn generated_source(
    v: WorldShaderVariantKey,
    m: MaterialFragmentKey,
) -> String {
    use std::fmt::Write;
    let mut source = crate::renderer::compose_world_shader(include_str!("../../bsp_lean.wgsl"))
        .replace("\r\n", "\n");
    let mods = specialized_tc_mods(&source, m.vertex_recipe);
    replace_function_body(&mut source, "apply_tc_mods", &mods);
    let uv = match m.tc_gen {
        0 => "input.uv".to_owned(),
        1 => "input.lightmap_uv".to_owned(),
        2 => "vec2<f32>(dot(input.position, material.vector_s.xyz), dot(input.position, material.vector_t.xyz))".to_owned(),
        3 => "vec2<f32>(0.5 - reflected.z * 0.5, 0.5 - reflected.y * 0.5)".to_owned(),
        _ => panic!("unsupported generated tcGen"),
    };
    let environment = if m.tc_gen == 3 {
        "let n = normalize(input.normal);\nlet view = normalize(camera.camera_pos_time.xyz - input.position);\nlet reflected = reflect(-view, n);"
    } else {
        ""
    };
    replace_function_body(
        &mut source,
        "generated_uv",
        &format!("\n{environment}\nreturn apply_tc_mods({uv}, input.position);\n"),
    );
    source = source
        .replace(
            "material.wave_funcs.x",
            &format!("{}u", m.vertex_recipe.wave_funcs[0]),
        )
        .replace(
            "material.wave_funcs.y",
            &format!("{}u", m.vertex_recipe.wave_funcs[1]),
        );

    let enhanced = m.program == StageProgram::GeneratedEnhanced;
    let has_wave = m.flags & (134217728 | 268435456) != 0;
    let needs_color = (m.flags & (1 | 524288) != 0 && m.flags & 134217728 == 0)
        || (m.flags & (1048576 | 2097152) != 0 && m.flags & 268435456 == 0);
    let needs_lm = enhanced || m.flags & 2 != 0;
    let needs_position = enhanced || v.legacy_fog || v.jump_shade;
    let mut fields = vec![("uv", "vec2<f32>")];
    if needs_lm {
        fields.push(("lightmap_uv", "vec2<f32>"));
    }
    if needs_color {
        fields.push(("color", "vec4<f32>"));
    }
    if needs_position {
        fields.push(("world_position", "vec3<f32>"));
    }
    if v.static_bsp_ao {
        fields.push(("sky_dir_ao", "vec4<f32>"));
    }
    if enhanced {
        fields.push(("world_normal", "vec3<f32>"));
    }
    if v.surface_deformation {
        fields.extend([("shell_kind", "u32"), ("shell_coverage", "f32")]);
    }
    // Baked relight reads the per-surface sun fit resolved in the vertex stage
    // from the surface-id stream (`world_vertex`), exactly like vs_main_legacy.
    let baked = enhanced && v.baked_sun_relight;
    if baked {
        fields.push(("baked_sun_fit", "vec4<f32>"));
    }
    if v.pbr && v.deluxe && v.pbr_vertex_lightgrid {
        fields.extend([
            ("pbr_lightgrid_direction", "vec4<f32>"),
            ("pbr_lightgrid_lighting", "vec4<f32>"),
        ]);
    }
    if has_wave {
        fields.push(("wave_color", "vec4<f32>"));
    }
    if has_wave {
        // Additive source modulation uses the same rgbGen waveform already
        // carried from the vertex shader. Do not re-run wave_value in the FS:
        // generated stage programs intentionally evaluate it only in the VS.
        let additive_wave_eval = format!(
            "wave = clamp(wave_value({}u, material.wave_rgb, shader_stage_time()), 0.0, 1.0);",
            m.vertex_recipe.wave_funcs[0],
        );
        assert!(source.contains(&additive_wave_eval), "additive waveform must remain vertex-only");
        source = source.replace(&additive_wave_eval, "wave = generated_wave_color.x;");
        // Keep the exact wave evaluator, including remap/clamp time, in the VS.
        // A flat varying carries its per-stage result to the existing FS helper.
        source = source.replacen("fn apply_wave_gens(", "fn vertex_wave_gens(", 1);
        source.push_str("\nvar<private> generated_wave_color: vec4<f32>;\nfn apply_wave_gens(color: vec4<f32>) -> vec4<f32> {\nvar out = color;\nif ((material_flags() & 134217728u) != 0u) { out = vec4<f32>(generated_wave_color.rgb, out.a); }\nif ((material_flags() & 268435456u) != 0u) { out.a = generated_wave_color.a; }\nreturn out;\n}\n");
    }
    let mut generated = String::from(
        "\nstruct GeneratedVertexOut {\n@builtin(position) @invariant clip_position: vec4<f32>,\n",
    );
    for (location, (name, ty)) in fields.iter().enumerate() {
        let flat = if *ty == "u32" || *name == "wave_color" || *name == "baked_sun_fit" {
            "@interpolate(flat) "
        } else {
            ""
        };
        let _ = writeln!(generated, "@location({location}) {flat}{name}: {ty},");
    }
    let surface_id_input = if baked { "@location(6) legacy_dlight_surface_id: u32, " } else { "" };
    let _ = writeln!(generated, "}};\n@vertex fn vs_generated(input: VertexIn, {surface_id_input}@builtin(instance_index) instance_index: u32) -> GeneratedVertexOut {{\nvar output: GeneratedVertexOut;");
    if enhanced {
        let surface_id = if baked { "legacy_dlight_surface_id" } else { "0xffffffffu" };
        let _ = writeln!(generated, "let surface = world_vertex(input, instance_index, {surface_id});\noutput.clip_position = surface.clip_position;");
        for (name, _) in &fields {
            if *name != "wave_color" {
                let _ = writeln!(generated, "output.{name} = surface.{name};");
            }
        }
    } else {
        generated.push_str("output.clip_position = camera.view_proj * vec4<f32>(input.position, 1.0);\noutput.uv = generated_uv(input);\n");
        if needs_lm {
            generated.push_str("output.lightmap_uv = input.lightmap_uv;\n");
        }
        if needs_color {
            generated.push_str("output.color = input.color;\n");
        }
        if needs_position {
            generated.push_str("output.world_position = input.position;\n");
        }
        if v.static_bsp_ao {
            generated.push_str("output.sky_dir_ao = vec4<f32>(vec3<f32>(0.0), input.static_ao);\n");
        }
    }
    if has_wave {
        generated.push_str("output.wave_color = vertex_wave_gens(vec4<f32>(1.0));\n");
    }
    generated.push_str("return output;\n}\n@fragment fn fs_generated(input: GeneratedVertexOut) -> @location(0) vec4<f32> {\n");
    if has_wave {
        generated.push_str("generated_wave_color = input.wave_color;\n");
    }
    if enhanced {
        generated
            .push_str("var surface: VertexOut;\nsurface.clip_position = input.clip_position;\n");
        for (name, _) in &fields {
            if *name != "wave_color" {
                let _ = writeln!(generated, "surface.{name} = input.{name};");
            }
        }
        let entry = if v.legacy_fog {
            "fs_main_legacy_fog"
        } else {
            "fs_main"
        };
        generated.push_str(
            &function_body(&source, entry)
                .replace("input.", "surface.")
                .replace("shade_surface(input)", "shade_surface(surface)"),
        );
    } else {
        let mut body = function_body(&source, "fs_world_minimal").to_owned();
        body = body
            .replace(
                "input.world_ao.xyz",
                if needs_position {
                    "input.world_position"
                } else {
                    "vec3<f32>(0.0)"
                },
            )
            .replace(
                "input.world_ao.w",
                if v.static_bsp_ao {
                    "input.sky_dir_ao.w"
                } else {
                    "1.0"
                },
            );
        if !needs_color {
            body = body.replace("input.color", "vec4<f32>(1.0)");
        }
        if !needs_lm {
            body = body.replace("input.lightmap_uv", "vec2<f32>(-1.0)");
        }
        generated.push_str(&body);
    }
    generated.push_str("\n}\n");
    source.push_str(&generated);
    source
}

#[derive(Default)]
pub(in crate::renderer) struct ProgramModules {
    modules: std::sync::Mutex<
        std::collections::HashMap<(WorldShaderVariantKey, MaterialFragmentKey), wgpu::ShaderModule>,
    >,
}

impl ProgramModules {
    /// `original` is the variant family's module; `lean` is bsp_lean.wgsl. Only
    /// `Specialized` uses the family module: every other program is Lean source
    /// (identical to `original` unless Hybrid Ocean selected the Enhanced family).
    pub fn shader(
        &self,
        device: &wgpu::Device,
        original: &wgpu::ShaderModule,
        lean: &wgpu::ShaderModule,
        v: WorldShaderVariantKey,
        m: MaterialFragmentKey,
    ) -> wgpu::ShaderModule {
        match m.program {
            StageProgram::Specialized => return original.clone(),
            StageProgram::GeneratedBasic | StageProgram::GeneratedEnhanced => {}
            _ => return lean.clone(),
        }
        let mut modules = self.modules.lock().unwrap();
        modules
            .entry((v, m))
            .or_insert_with(|| {
                device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("JKA generated world stage recipe"),
                    source: wgpu::ShaderSource::Wgsl(generated_source(v, m).into()),
                })
            })
            .clone()
    }

    pub fn pipeline(
        &self,
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        original: &wgpu::ShaderModule,
        lean: &wgpu::ShaderModule,
        format: wgpu::TextureFormat,
        samples: u32,
        variant: WorldShaderVariantKey,
        key: PipelineKey,
        material: MaterialFragmentKey,
    ) -> wgpu::RenderPipeline {
        let effective = program_variant(variant, material);
        let shader = self.shader(device, original, lean, effective, material);
        let minimal = uses_minimal_entry(material);
        crate::renderer::create_world_pipeline_specialized(
            device,
            layout,
            &shader,
            format,
            key,
            samples,
            effective.legacy_fog,
            false,
            Some(effective),
            false,
            None,
            Some(material),
            minimal,
        )
    }
}

#[cfg(test)]
#[path = "program_tests.rs"]
mod tests;
