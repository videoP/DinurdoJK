//! Compose the preserved upstream ocean surface WGSL using its original modules.
//! This is a shader validation bridge; JKA bind groups and render passes are
//! integrated separately. The JKA variant adds only a fragment admission test;
//! the preserved source and its shading calculations remain unchanged.
use naga_oil::compose::{
    ComposableModuleDescriptor, Composer, ComposerError, NagaModuleDescriptor, ShaderDefValue,
    ShaderLanguage, ShaderType,
};

const MODULES: &[(&str, &str)] = &[
    (
        "naval-render/shaders/camera.wgsl",
        include_str!("../../../vendor/blue-water-sea-and-sky/shared/render/shaders/camera.wgsl"),
    ),
    (
        "naval-render/shaders/shadow.wgsl",
        include_str!("../../../vendor/blue-water-sea-and-sky/shared/render/shaders/shadow.wgsl"),
    ),
    (
        "naval-render/shaders/motion.wgsl",
        include_str!("../../../vendor/blue-water-sea-and-sky/shared/render/shaders/motion.wgsl"),
    ),
    (
        "naval-render/shaders/lights.wgsl",
        include_str!("../../../vendor/blue-water-sea-and-sky/shared/render/shaders/lights.wgsl"),
    ),
    (
        "naval-render/shaders/aerial_volume.wgsl",
        include_str!(
            "../../../vendor/blue-water-sea-and-sky/shared/render/shaders/aerial_volume.wgsl"
        ),
    ),
    (
        "naval-render/shaders/aerial.wgsl",
        include_str!("../../../vendor/blue-water-sea-and-sky/shared/render/shaders/aerial.wgsl"),
    ),
    (
        "naval-clouds/shaders/shadow.wgsl",
        include_str!("../../../vendor/blue-water-sea-and-sky/clouds/shaders/shadow.wgsl"),
    ),
    (
        "naval-ocean/shaders/water_properties.wgsl",
        include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/water_properties.wgsl"),
    ),
    (
        "naval-ocean/shaders/noise.wgsl",
        include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/noise.wgsl"),
    ),
    (
        "naval-ocean/shaders/sky_environment.wgsl",
        include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/sky_environment.wgsl"),
    ),
    (
        "naval-ocean/shaders/reflection.wgsl",
        include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/reflection.wgsl"),
    ),
    (
        "naval-ocean/shaders/water_body.wgsl",
        include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/water_body.wgsl"),
    ),
    (
        "naval-ocean/shaders/foam.wgsl",
        include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/foam.wgsl"),
    ),
    (
        "naval-ocean/shaders/point_lights.wgsl",
        include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/point_lights.wgsl"),
    ),
    (
        "naval-ocean/shaders/mx_noise.wgsl",
        include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/mx_noise.wgsl"),
    ),
    (
        "naval-ocean/shaders/wake.wgsl",
        include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/wake.wgsl"),
    ),
    (
        "naval-ocean/shaders/waves.wgsl",
        include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/waves.wgsl"),
    ),
    (
        "naval-ocean/shaders/screen_reflection.wgsl",
        include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/screen_reflection.wgsl"),
    ),
    (
        "naval-ocean/shaders/underwater_ray.wgsl",
        include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/underwater_ray.wgsl"),
    ),
    (
        "naval-ocean/shaders/rain.wgsl",
        include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/rain.wgsl"),
    ),
    (
        "naval-ocean/shaders/spray_haze.wgsl",
        include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/spray_haze.wgsl"),
    ),
];

/// The JKA pool-admission function (`jka_water_contains`) and its `@group(1) @binding(16)` uniform, for effects that
/// must stay inside promoted water footprints.
pub const SURFACE_BOUNDARY_WGSL: &str = include_str!("surface_boundary.wgsl");

pub fn composer() -> Result<Composer, Box<ComposerError>> {
    let mut composer = Composer::default();
    for &(file_path, source) in MODULES {
        composer
            .add_composable_module(ComposableModuleDescriptor {
                source,
                file_path,
                language: ShaderLanguage::Wgsl,
                ..Default::default()
            })
            .map_err(Box::new)?;
    }
    Ok(composer)
}

pub fn surface_module(
    composer: &mut Composer,
    point_lights: bool,
) -> Result<wgpu::naga::Module, Box<ComposerError>> {
    compose_surface(composer, point_lights, false, 16, 0, false)
}

pub fn jka_surface_module(
    composer: &mut Composer,
    reflection_steps: u32,
    pixel_detail_mode: u8,
) -> Result<wgpu::naga::Module, Box<ComposerError>> {
    compose_surface(composer, false, true, reflection_steps.clamp(4, 32), pixel_detail_mode.min(4), false)
}

/// Original underwater passes with only the JKA pool-admission/path cap added.
pub fn underwater_module(composer: &mut Composer, pass: &str) -> Result<wgpu::naga::Module, Box<ComposerError>> {
    let replace = |source: &str, from: &str, to: &str| {
        assert_eq!(source.matches(from).count(), 1, "upstream underwater contract changed: {from}");
        source.replace(from, to)
    };
    let (source, path) = match pass {
        "probe" => {
            let original = include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/underwater_probe.wgsl");
            let source = replace(original, "let depth = height_at(ray.start.xz) + wake_height(ray.start.xz) - ray.start.y;",
                "let depth = select(-1.0, height_at(ray.start.xz) + wake_height(ray.start.xz) - ray.start.y, jka_water_contains(ray.start.xz));")
                .replace("vec4<f32>(depth, height_at(eye.xz) - eye.y, 0.0, 1.0)",
                    "vec4<f32>(depth, select(-1.0, height_at(eye.xz) - eye.y, jka_water_contains(eye.xz)), 0.0, 1.0)");
            (format!("{source}\n{}", include_str!("surface_boundary.wgsl")), "original underwater probe with JKA boundary")
        }
        "composite" => {
            let original = include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/underwater.wgsl");
            // JKA's short geometry far plane is not the end of an open water
            // column. A missing receiver may reach optically opaque water;
            // the independent JKA path texture still limits finite pools.
            let source = replace(original, "return -near * far / (depth * (far - near) + near);",
                "return select(-near * far / (depth * (far - near) + near), -10000.0, depth <= 0.0);");
            let source = replace(&source, "let distance = max(farthest / ray.view.z - ray.to_near, 0.0) * clarity;",
                "let distance = min(max(farthest / ray.view.z - ray.to_near, 0.0), textureLoad(jka_water_distance, pixel, 0).r) * clarity;");
            let source = replace(&source, "let film = waterline * MENISCUS_TINT;",
                "let path = textureLoad(jka_water_distance, vec2<i32>(position.xy), 0).r;\n    let film = select(0.0, waterline * MENISCUS_TINT, path > 0.0);");
            (format!("{source}\n@group(1) @binding(4) var jka_water_distance: texture_2d<f32>;"), "original underwater composite with JKA bounded path")
        }
        "motes" => {
            let original = include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/underwater_motes.wgsl");
            let source = replace(original, "let shown = smoothstep(NEAR.x, NEAR.y, range)",
                "let pixel = clamp(vec2<i32>(uv * camera.viewport.xy), vec2<i32>(0), vec2<i32>(camera.viewport.xy) - 1);\n    let bounded = select(0.0, step(range, textureLoad(jka_water_distance, pixel, 0).r), jka_water_contains(world.xz));\n    let shown = bounded * smoothstep(NEAR.x, NEAR.y, range)");
            (format!("{source}\n@group(1) @binding(4) var jka_water_distance: texture_2d<f32>;\n{}", include_str!("surface_boundary.wgsl")), "original underwater motes with JKA bounded path")
        }
        _ => panic!("unknown underwater pass {pass}"),
    };
    composer.make_naga_module(NagaModuleDescriptor {
        source: &source, file_path: path, shader_type: ShaderType::Wgsl,
        shader_defs: [("CASCADES".to_owned(), ShaderDefValue::UInt(3))].into_iter().collect(),
        ..Default::default()
    }).map_err(Box::new)
}

/// Debug-only native wave-height marker. The probe uses the *same* native
/// displacement and wake helpers as the visible Blue Water vertex shader.
/// This shader is compiled only on explicit request, not during map load.
pub fn jka_waterline_marker_module(composer: &mut Composer) -> Result<wgpu::naga::Module, Box<ComposerError>> {
    let source = format!("{}\n{}\n{}", include_str!("waterline_marker.wgsl"),
        include_str!("surface_boundary.wgsl"), include_str!("hybrid_gerstner.wgsl"));
    composer.make_naga_module(NagaModuleDescriptor {
        source: &source,
        file_path: "JKA native wave line diagnostic",
        shader_type: ShaderType::Wgsl,
        shader_defs: [("CASCADES".to_owned(), ShaderDefValue::UInt(3))].into_iter().collect(),
        ..Default::default()
    }).map_err(Box::new)
}

/// One-thread compute probe: the height of the original surface mesh over a point,
/// using the preserved upstream `displacement()`. Wave resources bind at group 2.
pub fn eye_height_module(composer: &mut Composer) -> Result<wgpu::naga::Module, Box<ComposerError>> {
    composer.make_naga_module(NagaModuleDescriptor {
        source: include_str!("eye_height.wgsl"),
        file_path: "JKA native eye height probe",
        shader_type: ShaderType::Wgsl,
        shader_defs: [("CASCADES".to_owned(), ShaderDefValue::UInt(3))].into_iter().collect(),
        ..Default::default()
    }).map_err(Box::new)
}

/// Original surface + Hybrid gerstner extension. Keep the Blue Water FFT module
/// untouched; insert only at its single vertex-displacement and normal sites.
fn jka_hybrid_surface_source(source: &str) -> String {
    const SKY_BINDING: &str = "@group(3) @binding(2) var<uniform> sky_env: SkyEnvironment;";
    const VERTEX: &str = "let offset = displacement(grid, spacing);";
    const NORMAL: &str = "let total = sample.slope - wake.normal.xz / max(wake.normal.y, 0.05);";
    assert_eq!(source.matches(SKY_BINDING).count(), 1, "Hybrid source binding changed upstream");
    assert_eq!(source.matches(VERTEX).count(), 1, "Hybrid source vertex changed upstream");
    assert_eq!(source.matches(NORMAL).count(), 1, "Hybrid source normal changed upstream");
    let source = source.replace(SKY_BINDING,
        &format!("{SKY_BINDING}\n{}", include_str!("hybrid_gerstner.wgsl")));
    // Both regular windrows and the gale's `max(windrows, streaks)` path
    // must obey the same sea-energy gate. Suppress final windrow coverage
    // AFTER the gale maximum, rather than patching one optional branch and
    // accidentally leaving other white patterns fully opaque at 0.1 m/s.
    const WINDROWS_END: &str = "    let white = billows(patterns.pattern.a,";
    const BUBBLES: &str = "let aerated = aeration_with_wake(sample.bubbles * (1.0 - whitecaps.averaged), wake.bubbles);";
    assert_eq!(source.matches(WINDROWS_END).count(), 1,
        "Blue Water windrow gate anchor changed upstream");
    source.replace(VERTEX, "let offset = jka_hybrid_displacement(grid, spacing);")
        .replace(NORMAL, "let total = sample.slope * jka_hybrid.motion_mix.z + jka_hybrid_slope(xz) - wake.normal.xz / max(wake.normal.y, 0.05);")
        .replace(WINDROWS_END, "    windrows *= surface.crest_foam.a;\n    let white = billows(patterns.pattern.a,")
        // Keep wake aeration independently; hide only ocean bubbles persisting
        // from earlier high-sea FFT frames.
        .replace(BUBBLES, "let aerated = aeration_with_wake(sample.bubbles * surface.crest_foam.a * (1.0 - whitecaps.averaged), wake.bubbles);")
}

fn compose_surface(
    composer: &mut Composer,
    point_lights: bool,
    jka_boundary: bool,
    reflection_steps: u32,
    pixel_detail_mode: u8,
    near_detail: bool,
) -> Result<wgpu::naga::Module, Box<ComposerError>> {
    let mut shader_defs = vec![
        ("CASCADES".to_owned(), ShaderDefValue::UInt(3)),
        ("REFLECTION_STEPS".to_owned(), ShaderDefValue::UInt(reflection_steps)),
    ];
    if point_lights {
        shader_defs.push(("POINT_LIGHTS".to_owned(), ShaderDefValue::UInt(1)));
    }
    let upstream = include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/surface.wgsl");
    let original = if jka_boundary { jka_hybrid_surface_source(upstream) } else { upstream.to_owned() };
    let source = if jka_boundary {
        let entry = "fn fragment(input: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {";
        assert_eq!(original.matches(entry).count(), 1, "upstream fragment contract changed");
        // Upstream's ordinary wave rings fit inside its 30 km far plane. JKA's
        // distanceCull can end within the first few rings, before the skirt.
        // Fit every admitted infinite-water vertex to the host depth range so
        // the rings join the horizon even as distanceCull changes at runtime.
        let far_guard = "if (horizon || fan) && camera.curvature > 0.0 {";
        assert_eq!(original.matches(far_guard).count(), 1, "upstream far-plane contract changed");
        let original = original.replace(far_guard, "{");
        // JKA exposes the original transmission swatch as its water color.
        // Apply the same relative tint to the upwelling body without tinting
        // scene objects or reflected sky. The calibrated default has gain one.
        let deep = "let deep = upwelling(subsurface_reflectance(absorption, backscatter), sun_direction, sun_radiance, contact.irradiance, lit);";
        assert_eq!(original.matches(deep).count(), 1, "upstream water-body contract changed");
        let tint = crate::original_surface::SurfaceAppearance::default().transmission;
        let original = original.replace(deep, &format!("let deep = upwelling(subsurface_reflectance(absorption, backscatter), sun_direction, sun_radiance, contact.irradiance, lit) * (surface.transmission.rgb / vec3<f32>({}, {}, {}));", tint[0], tint[1], tint[2]));
        // A nearest-depth MSAA scene copy can select the foreground player at a
        // silhouette while the water fragment covers another sample. That is
        // open water, not a zero-depth shoreline with a bright foam outline.
        let column = "let column = select(max(scene_view_z / min(ray_view_z, -1e-4) - distance, 0.0), OPEN_DEEP, depth <= 0.0);";
        assert_eq!(original.matches(column).count(), 1, "upstream water-column contract changed");
        let original = original.replace(column, "let receiver_range = scene_view_z / min(ray_view_z, -1e-4);\n    let column = select(max(receiver_range - distance, 0.0), OPEN_DEEP, depth <= 0.0 || receiver_range + 0.03 < distance);");
        let body = "let body = water_body(textureLoad(scene_color, pixel, 0).rgb, deep, column_transmittance(attenuation, backscatter, column, view));";
        assert_eq!(original.matches(body).count(), 1, "upstream transmission contract changed");
        let original = original.replace(body, "let bent = jka_refracted_scene(input.world, -view, up, 1.0 / 1.333, column * 2.0, true);\n    let transmitted = select(textureLoad(scene_color, pixel, 0).rgb, bent.rgb, bent.a >= 0.0);\n    let through = select(column, bent.a, bent.a >= 0.0);\n    let body = water_body(transmitted, deep, column_transmittance(attenuation, backscatter, through, view));");
        let underside = "color = underside(view, up, pigment, select(absorption, water_beam(optics), surface.water.absorption.w > 0.5), sun_direction, sky_env.light_color);";
        assert_eq!(original.matches(underside).count(), 1, "upstream underside contract changed");
        let original = original.replace(underside, &format!("{underside}\n        let window_ray = refract(-view, -up, 1.333);\n        let window_scene = jka_refracted_scene(input.world, -view, -up, 1.333, 100.0, false);\n        if window_scene.a >= 0.0 {{\n            color += (window_scene.rgb - sky_reflection(window_ray, 0.0)) * WINDOW_GAIN * (1.0 - water_fresnel(max(dot(window_ray, up), 0.0)));\n        }}"));

        let fog = "var color = apply_fog(above, input.world);";
        assert_eq!(original.matches(fog).count(), 1, "upstream surface fog contract changed");
        let original = original.replace(fog, "var color = jka_surface_legacy_fog(apply_fog(above, input.world), input.world);");
        let original = format!("{original}\n{}", include_str!("surface_fog.wgsl"));
        // Reduced-resolution variants execute the original *entire* material
        // on a smaller attachment. Only the screen-space opaque scene lookup
        // needs remapping back to full-resolution source textures.
        let scale = match pixel_detail_mode { 1 | 3 => 2, 2 | 4 => 4, _ => 1 };
        let core = original.replace(entry,
            &format!("{entry}\n    if !jka_water_contains(input.grid) {{ discard; }}"));
        let core = if near_detail {
            // Only near fragments run the original, expensive material. The
            // overlaid color blends from full quality to reconstructed water
            // over a 10%-wide transition instead of visibly hard switching.
            let anchor = "let wake = wake_surface(xz, input.world, footprint);";
            assert_eq!(core.matches(anchor).count(), 1, "upstream early fragment contract changed");
            let early = "let near_distance = distance(input.world, camera.position.xyz);\n    let near_limit = jka_boundary.scale.w;\n    if near_distance >= near_limit * 1.1 { return vec4<f32>(0.0); }";
            let last = "return vec4<f32>(color, 1.0);";
            assert_eq!(core.matches(last).count(), 1, "upstream full color return changed");
            let edited = core.replace(anchor, &format!("{early}\n    {anchor}"))
                .replace(last, "return vec4<f32>(color, 1.0 - smoothstep(near_limit * 0.9, near_limit * 1.1, near_distance));");
            format!("diagnostic(off, derivative_uniformity);\n{edited}")
        } else { core };
        let core = if scale == 1 {
            core
        } else {
            let pixel = "let pixel = vec2<i32>(input.clip.xy);";
            assert_eq!(core.matches(pixel).count(), 1, "upstream pixel coordinate contract changed");
            core.replace(pixel, &format!("let pixel = vec2<i32>(input.clip.xy * f32({scale}));"))
        };
        format!("{}\n{}\n{}", core, include_str!("surface_boundary.wgsl"), include_str!("surface_refraction.wgsl"))
    } else {
        original.to_owned()
    };
    composer
        .make_naga_module(NagaModuleDescriptor {
            source: &source,
            file_path: "naval-ocean/shaders/surface.wgsl",
            shader_type: ShaderType::Wgsl,
            shader_defs: shader_defs.into_iter().collect(),
            ..Default::default()
        })
        .map_err(Box::new)
}

/// Native showtris must use the exact same JKA footprint/PVS/sky fragment
/// admission as the shaded ocean. The diagnostic has its own small fragment,
/// but retains Blue Water's original displaced vertex/clipmap geometry.
/// Compose only on demand (r_showtris OCEAN), not in the normal shader path.
pub fn jka_wireframe_module(
    composer: &mut Composer,
) -> Result<wgpu::naga::Module, Box<ComposerError>> {
    const ENTRY: &str = "@fragment\nfn fragment(input: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {";
    let original = include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/surface.wgsl")
        .replace("\r\n", "\n");
    // Match the shaded surface's JKA far-plane treatment for infinite
    // sky-contact rings. Do not change the original displaced vertex path.
    const FAR_GUARD: &str = "if (horizon || fan) && camera.curvature > 0.0 {";
    assert_eq!(original.matches(FAR_GUARD).count(), 1, "upstream vertex contract changed");
    let original = jka_hybrid_surface_source(&original).replace(FAR_GUARD, "{");
    let start = original.find(ENTRY).expect("upstream wireframe fragment signature");
    let brace = start + ENTRY.len() - 1;
    let mut depth = 0;
    let mut end = brace;
    for (offset, ch) in original[brace..].char_indices() {
        if ch == '{' { depth += 1; }
        if ch == '}' {
            depth -= 1;
            if depth == 0 { end = brace + offset + 1; break; }
        }
    }
    assert!(end > brace, "upstream wireframe fragment body changed");
    let fragment = r#"@fragment
fn fragment(input: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    if !jka_water_contains(input.grid) { discard; }
    return vec4<f32>(1.0, 0.9, 0.1, 1.0);
}"#;
    let source = format!("{}{}{}\n{}", &original[..start], fragment,
        &original[end..], include_str!("surface_boundary.wgsl"));
    composer.make_naga_module(NagaModuleDescriptor {
        source: &source,
        file_path: "JKA Blue Water PVS-masked native wireframe",
        shader_type: ShaderType::Wgsl,
        shader_defs: [("CASCADES".to_owned(), ShaderDefValue::UInt(3)),
            ("REFLECTION_STEPS".to_owned(), ShaderDefValue::UInt(16))].into_iter().collect(),
        ..Default::default()
    }).map_err(Box::new)
}

/// Full-resolution original surface near the camera for distance-aware modes.
/// Blends over the reconstructed far field with no substitute color.
pub fn jka_near_surface_module(
    composer: &mut Composer,
    reflection_steps: u32,
) -> Result<wgpu::naga::Module, Box<ComposerError>> {
    compose_surface(composer, false, true, reflection_steps.clamp(4, 32), 0, true)
}

/// Full-resolution depth/coverage reconstruction. The original vertex stage
/// is retained: there is NO flat-water proxy, and the JKA footprint test is
/// applied at full resolution. Color comes from actual Blue Water shading at
/// 1/2 or 1/4 resolution, with alpha- and depth-aware reconstruction.
pub fn jka_reconstruct_module(
    composer: &mut Composer,
) -> Result<wgpu::naga::Module, Box<ComposerError>> {
    jka_reconstruct_from_source(composer, include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/surface.wgsl"))
}

fn jka_reconstruct_from_source(
    composer: &mut Composer,
    original: &str,
) -> Result<wgpu::naga::Module, Box<ComposerError>> {
    const ENTRY: &str = "fn fragment(input: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {";
    // Git may check out WGSL as CRLF on Windows. Normalize before locating or
    // splicing entry-point attributes so byte offsets have the same meaning.
    let original = original.replace("\r\n", "\n");
    // Keep the host's native far-plane adjustment in the reconstructed vertex
    // path, otherwise the depth and silhouette differ from the shaded pass.
    let original = jka_hybrid_surface_source(&original);
    let original = original.replace(
        "if (horizon || fan) && camera.curvature > 0.0 {",
        "{",
    );
    // Put new resources alongside upstream's global binding declarations,
    // before the first WGSL function. Inserting an attributed `var` between
    // functions made naga-oil reject the generated reconstruction module.
    const SKY_BINDING: &str = "@group(3) @binding(2) var<uniform> sky_env: SkyEnvironment;";
    assert_eq!(original.matches(SKY_BINDING).count(), 1, "upstream binding layout changed");
    let original = original.replace(
        SKY_BINDING,
        &format!("{SKY_BINDING}\n@group(4) @binding(0) var reduced_color: texture_2d<f32>;\n@group(4) @binding(1) var reduced_depth: texture_depth_2d;"),
    );
    let start = original.find(ENTRY).expect("upstream fragment signature");
    let brace = start + ENTRY.len() - 1;
    let mut level = 0;
    let mut end = brace;
    for (i, c) in original[brace..].char_indices() {
        if c == '{' { level += 1; }
        if c == '}' { level -= 1; if level == 0 { end = brace + i + 1; break; } }
    }
    assert!(end > brace, "upstream fragment body changed");
    let fragment = r#"
@fragment
fn fragment(input: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    if !jka_water_contains(input.grid) { discard; }
    let dim = vec2<i32>(textureDimensions(reduced_color));
    let loc = input.clip.xy / camera.viewport.xy * vec2<f32>(dim) - 0.5;
    let origin = vec2<i32>(floor(loc));
    let fraction = fract(loc);
    var result = vec3<f32>(0.0);
    var weight = 0.0;
    var nearest = vec3<f32>(0.0);
    var nearest_weight = -1.0;
    for (var j = 0; j < 2; j++) {
        for (var i = 0; i < 2; i++) {
            let coord = clamp(origin + vec2<i32>(i,j), vec2<i32>(0), dim-1);
            let sample_color = textureLoad(reduced_color, coord, 0);
            let sample_z = textureLoad(reduced_depth, coord, 0);
            let k = select(1.0 - fraction.x, fraction.x, i == 1) *
                    select(1.0 - fraction.y, fraction.y, j == 1);
            // Reject colors from the opposite side of a depth discontinuity.
            let agreement = exp(-abs(input.clip.z - sample_z) * 600.0);
            let w = k * sample_color.a * agreement;
            result += sample_color.rgb * w;
            weight += w;
            if w > nearest_weight { nearest_weight = w; nearest = sample_color.rgb; }
        }
    }
    if weight <= 1e-6 {
        // A thin displaced crest or shoreline may cover a full-res pixel but
        // miss the coarse 2x2/4x4 sample grid. Look for adjacent water before
        // giving up; never substitute an arbitrary water swatch.
        var candidate_distance = 1000.0;
        for (var j = -2; j <= 2; j++) {
            for (var i = -2; i <= 2; i++) {
                let coord = clamp(origin + vec2<i32>(i,j), vec2<i32>(0), dim-1);
                let color = textureLoad(reduced_color, coord, 0);
                let metric = length(vec2<f32>(f32(i), f32(j)) - fraction);
                if color.a > 0.5 && metric < candidate_distance {
                    candidate_distance = metric;
                    nearest = color.rgb;
                }
            }
        }
        if candidate_distance >= 1000.0 {
            return vec4<f32>(textureLoad(scene_color, clamp(vec2<i32>(input.clip.xy), vec2<i32>(0), vec2<i32>(camera.viewport.xy)-1), 0).rgb, 1.0);
        }
        return vec4<f32>(nearest, 1.0);
    }
    return vec4<f32>(result / weight, 1.0);
}
"#;
    const FRAGMENT_ATTRIBUTE: &str = "@fragment\n";
    let prefix_end = start.checked_sub(FRAGMENT_ATTRIBUTE.len())
        .expect("upstream fragment attribute missing");
    assert_eq!(&original[prefix_end..start], FRAGMENT_ATTRIBUTE,
        "upstream fragment attribute changed");
    let source = format!("{}{}{}\n{}", &original[..prefix_end], fragment,
        &original[end..], include_str!("surface_boundary.wgsl"));
    composer.make_naga_module(NagaModuleDescriptor {
        source: &source,
        file_path: "JKA Blue Water reduced reconstruction",
        shader_type: ShaderType::Wgsl,
        shader_defs: [("CASCADES".to_owned(), ShaderDefValue::UInt(3)),
                      ("REFLECTION_STEPS".to_owned(), ShaderDefValue::UInt(16))].into_iter().collect(),
        ..Default::default()
    }).map_err(Box::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_surface_shader_composes_and_validates() {
        for point_lights in [false, true] {
            let mut composer = composer()
                .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&Composer::default())));
            let module = surface_module(&mut composer, point_lights)
                .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&composer)));
            wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .expect("preserved Blue Water surface validates");
        }
    }

    #[test]
    fn jka_reduced_pixel_modes_validate() {
        // The JKA surface differs from the preserved upstream shader:
        // validate its unchanged mode and every specialized cheap mode.
        for mode in 0..=4 {
            let mut composer = composer().expect("upstream modules");
            let module = jka_surface_module(&mut composer, 16, mode)
                .unwrap_or_else(|error| panic!("mode {mode}: {}", error.emit_to_string(&composer)));
            wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .unwrap_or_else(|error| panic!("mode {mode} invalid: {error}"));
        }
    }

    #[test]
    fn jka_reconstruction_and_near_shaders_validate() {
        for near in [false, true] {
            let mut composer = composer().expect("upstream modules");
            let module = if near {
                jka_near_surface_module(&mut composer, 16)
            } else {
                jka_reconstruct_module(&mut composer)
            }.unwrap_or_else(|error| panic!("{}", error.emit_to_string(&composer)));
            wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::default(),
            ).validate(&module).expect("Blue Water reduced composition validates");
        }
    }

    #[test]
    fn jka_reconstruction_accepts_lf_and_crlf_sources() {
        let lf = include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/surface.wgsl")
            .replace("\r\n", "\n");
        for source in [lf.clone(), lf.replace('\n', "\r\n")] {
            let mut composer = composer().expect("upstream modules");
            let module = jka_reconstruct_from_source(&mut composer, &source)
                .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&composer)));
            wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::default(),
            ).validate(&module).expect("reconstructed water validates with either line ending");
        }
    }

    #[test]
    fn original_surface_pipeline_builds_on_gpu() {
        futures_lite::future::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("GPU adapter");
            let (device, _queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    required_limits: adapter.limits(),
                    ..Default::default()
                })
                .await
                .expect("GPU device");
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            for point_lights in [false, true] {
                let mut composer = composer().expect("upstream modules");
                let module = surface_module(&mut composer, point_lights).expect("upstream surface");
                let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("Blue Water original surface"),
                    source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(module)),
                });
                let compile_started = std::time::Instant::now();
                let _pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("Blue Water original surface validation"),
                    layout: None,
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("vertex"),
                        compilation_options: Default::default(),
                        buffers: &[Some(crate::geometry::OceanVertex::LAYOUT)],
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some("fragment"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::TextureFormat::Rgba16Float.into())],
                    }),
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleList,
                        front_face: wgpu::FrontFace::Ccw,
                        cull_mode: None,
                        ..Default::default()
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: wgpu::TextureFormat::Depth32Float,
                        depth_write_enabled: Some(true),
                        depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                        stencil: Default::default(),
                        bias: Default::default(),
                    }),
                    multisample: Default::default(),
                    multiview_mask: None,
                    cache: None,
                });
                eprintln!(
                    "[BLUE WATER COMPILE] original surface point_lights={point_lights}: {:.3} ms",
                    compile_started.elapsed().as_secs_f64() * 1000.0
                );
            }
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            assert!(
                scope.pop().await.is_none(),
                "Blue Water original surface pipeline validation"
            );
        });
    }
}
