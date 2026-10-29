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
    composer_with_noise(None)
}

/// `COST_PROBE_CHEAP_NOISE`: `value_noise` reduced to one fract of its inputs (the lattice hashes and their smoothstep
/// mix gone), to bound what a baked noise texture could save. Changes the image.
pub const COST_PROBE_CHEAP_NOISE: u32 = 1 << 21;
/// The wave sample and foam patterns replaced by eight anisotropic reads: the per-pixel side of a baked cache.
pub const COST_PROBE_BAKED_READS: u32 = 1 << 22;

/// Register-pressure probe (DOOM: The Dark Ages, GDC 2026 slide 55): the scalars computed before the screen-space
/// reflection and read only after it (foam inputs of the wave sample and wake, light shares, the water body) held as
/// pairs of halves in one `u32` each (`pack2x16float`) across the reflection march, unpacked where the foam starts.
/// Lossy (fp16 rounding of those values only).
pub const COST_PROBE_PACK_HALF: u32 = 1 << 23;
/// `COST_PROBE_PACK_HALF` plus the reflected sky and the daylight (radiance, clamped to fp16's range) and the facet
/// variances.
pub const COST_PROBE_PACK_HALF_WIDE: u32 = 1 << 24;

/// `COST_PROBE_PACK_HALF`'s set held as two bfloat16 halves (each f32's high 16 bits, rounded) by integer shifts and
/// masks, which a compiler cannot fold back to the f32 the way it may fold `unpack2x16float(pack2x16float(x))`.
/// 8-bit mantissa (0.4 % relative).
pub const COST_PROBE_PACK_BITS: u32 = 1 << 25;

/// The modules with `naval_ocean::noise`'s `value_noise` body replaced (`cheap_noise`), for the cost probe only.
pub fn composer_with_noise(cheap_noise: Option<()>) -> Result<Composer, Box<ComposerError>> {
    composer_with_footprint(cheap_noise, 1)
}

/// Pixels per reduced pixel edge for an `r_oceanPixelDetail` mode (1 at full resolution).
pub fn pixel_scale(pixel_detail_mode: u8) -> u32 {
    match pixel_detail_mode { 1 | 3 | 5 => 2, 2 | 4 => 4, _ => 1 }
}

/// The reduced-resolution material shades one pixel per `scale`² block, but at the full-resolution pixel's footprint:
/// every screen derivative (`dpdx`, `dpdy`, `fwidth`) divided by `scale` and every implicit-mip read biased
/// `-log2(scale)` (`textureSampleBias`). The reduced image is then point samples of the full-resolution one: the
/// filtered roughness, glint density and foam mips match, so `r_oceanPixelDetail` 5's full-resolution tiles join the
/// reconstruction without seams, and 1-4 keep Full's glints and foam detail instead of 2x2-wide versions.
fn full_resolution_footprint(source: &str, scale: u32) -> String {
    if scale <= 1 {
        return source.to_owned();
    }
    let mut out = String::with_capacity(source.len() + 256);
    let mut rest = source;
    const CALLS: &[&str] = &["dpdx(", "dpdy(", "fwidth(", "textureSample("];
    loop {
        // The next call of one of CALLS that starts an identifier (not `textureSampleGrad(`, not `my_dpdx(`).
        let next = CALLS.iter().filter_map(|call| {
            let mut from = 0;
            while let Some(at) = rest[from..].find(call).map(|at| at + from) {
                let boundary = rest[..at].chars().next_back().is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
                if boundary { return Some((at, *call)); }
                from = at + call.len();
            }
            None
        }).min_by_key(|&(at, _)| at);
        let Some((at, call)) = next else { break; };
        // The call's closing parenthesis.
        let open = at + call.len();
        let mut depth = 1;
        let close = open + rest[open..].char_indices().find(|&(_, c)| {
            depth += match c { '(' => 1, ')' => -1, _ => 0 };
            depth == 0
        }).expect("unbalanced call in shader source").0;
        let arguments = full_resolution_footprint(&rest[open..close], scale);
        out.push_str(&rest[..at]);
        if call == "textureSample(" {
            out.push_str(&format!("textureSampleBias({arguments}, {:.1})", -(scale as f32).log2()));
        } else {
            out.push_str(&format!("({:.2} * {call}{arguments}))", 1.0 / scale as f32));
        }
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

/// `composer_with_noise` with every module at the full-resolution footprint of a material shaded at `1 / scale`
/// resolution (`full_resolution_footprint`); 1 leaves the modules unchanged.
pub fn composer_with_footprint(cheap_noise: Option<()>, scale: u32) -> Result<Composer, Box<ComposerError>> {
    composer_with_options(cheap_noise, scale, false, false)
}

/// Surface experiments leave the shared vendor shaders untouched.
pub fn composer_with_surface_probes(probe: u32, scale: u32) -> Result<Composer, Box<ComposerError>> {
    composer_with_options((probe & COST_PROBE_CHEAP_NOISE != 0).then_some(()), scale,
        probe & COST_PROBE_SERIAL_REFINEMENT != 0,
        probe & COST_PROBE_SINGLE_WAVE_COPY != 0 && probe & SURFACE_GODOT_WATER == 0)
}

fn composer_with_options(cheap_noise: Option<()>, scale: u32, serial_refinement: bool, single_wave_copy: bool) -> Result<Composer, Box<ComposerError>> {
    const BODY_START: &str = "fn value_noise(p: vec2<f32>, seed: u32) -> f32 {";
    let cheap = cheap_noise.map(|()| {
        let source = MODULES.iter().find(|(path, _)| *path == "naval-ocean/shaders/noise.wgsl").unwrap().1;
        let start = source.find(BODY_START).expect("upstream value_noise signature");
        let end = start + source[start..].find("\n}").expect("value_noise body end") + 2;
        format!("{}{BODY_START}\n    return fract(p.x * 0.618 + p.y * 0.382 + f32(seed) * 0.1);\n}}{}", &source[..start], &source[end..])
    });
    let mut composer = Composer::default();
    for &(file_path, source) in MODULES {
        let source = match &cheap {
            Some(cheap) if file_path == "naval-ocean/shaders/noise.wgsl" => cheap.as_str(),
            _ => source,
        };
        let source = if single_wave_copy && file_path == "naval-ocean/shaders/waves.wgsl" {
            // Change the shared displacement/normal/reflected-hit sampling
            // together. This removes anti-tiling, not the FFT or foam fields.
            replace_once(source, "    if i >= 2 {", "    if true {")
        } else if serial_refinement && file_path == "naval-ocean/shaders/screen_reflection.wgsl" {
            jka_serial_refinement(source)
        } else { source.to_owned() };
        let source = full_resolution_footprint(&source, scale);
        composer
            .add_composable_module(ComposableModuleDescriptor {
                source: &source,
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
    jka_surface_module_with_probe(composer, reflection_steps, pixel_detail_mode, 0)
}

/// `jka_surface_module` with `r_oceanCostProbe` feature stubs (`COST_PROBE_*`). A/B cost measurement only: every
/// Most masks change the image; defer-body and serial-refinement retain the material's math.
pub fn jka_surface_module_with_probe(
    composer: &mut Composer,
    reflection_steps: u32,
    pixel_detail_mode: u8,
    cost_probe: u32,
) -> Result<wgpu::naga::Module, Box<ComposerError>> {
    compose_surface_probed(composer, false, true, reflection_steps.clamp(4, 32), pixel_detail_mode.min(5), false,
        cost_probe, false)
}

/// `r_oceanCostProbe` bits: each stubs one feature of the JKA surface fragment so its share of the pass can be
/// benched. The stubs are not lossless; they exist only to size the tile-classification work.
pub const COST_PROBE_SSR: u32 = 1 << 0;
pub const COST_PROBE_FOAM: u32 = 1 << 1;
pub const COST_PROBE_GLINTS: u32 = 1 << 2;
pub const COST_PROBE_WAKE: u32 = 1 << 3;
pub const COST_PROBE_RAIN: u32 = 1 << 4;
pub const COST_PROBE_REFRACTION: u32 = 1 << 5;
pub const COST_PROBE_WAVES: u32 = 1 << 6;
pub const COST_PROBE_FOG: u32 = 1 << 7;
pub const COST_PROBE_SHADOWS: u32 = 1 << 8;
pub const COST_PROBE_SKY: u32 = 1 << 9;
pub const COST_PROBE_SHELTER: u32 = 1 << 10;
pub const COST_PROBE_UNDERSIDE: u32 = 1 << 11;
/// Admission only: a constant colour after the footprint test (the pass's fixed floor).
pub const COST_PROBE_FLOOR: u32 = 1 << 12;
pub const COST_PROBE_ALL: u32 = (1 << 13) - 1;
/// Same material expressions, computed after SSR to shorten temporary lifetimes.
pub const COST_PROBE_DEFER_BODY: u32 = 1 << 13;
/// Wave-sampling diagnostic: native FFT uses one coherent copy at every scale.
/// The same bit already flattens the pixel wave frame for the Gerstner material;
/// the SURFACE_GODOT_WATER tag distinguishes those mutually exclusive backends.
/// Lossy: allows tile repetition and changes the sampled wave pattern.
pub const COST_PROBE_SINGLE_WAVE_COPY: u32 = 1 << 14;
/// Same eight bisections, reading only the selected quarter instead of both quarters.
pub const COST_PROBE_SERIAL_REFINEMENT: u32 = 1 << 15;
/// Finer foam probes (the parts `COST_PROBE_FOAM` removes together): the wave field's spread reads for the bubble cloud,
/// the breaking groups' noise, the gale streaks, and the foam texture's pattern reads.
pub const COST_PROBE_FOAM_SPREAD: u32 = 1 << 17;
pub const COST_PROBE_FOAM_GROUPS: u32 = 1 << 18;
pub const COST_PROBE_FOAM_GALE: u32 = 1 << 19;
pub const COST_PROBE_FOAM_PATTERNS: u32 = 1 << 20;
/// Lossless variant bit sharing the probe mask (`r_oceanSkipFadedCascades`): a cascade whose `detail` is exactly 0 at
/// a pixel skips its field reads. Every use of those reads is scaled by `detail` (or `shown`, which carries it), and
/// its variance is `mix(slopes, settled, 0) == slopes`, so the result is unchanged for finite field data.
pub const SURFACE_SKIP_FADED_CASCADES: u32 = 1 << 16;
/// Far-water foam (`r_oceanFarShader`): the foam patterns at upstream's own fully filtered values (every blur 1, the
/// windrows' band mask at its mean, gathering at ½). Upstream reaches these by itself once a pixel covers enough of the
/// foam texture, and every pattern read is then weighted by zero, so the texture reads and their noise drop out.
/// Nearer water loses its lace.
pub const FAR_SHADER_FOAM: u32 = 1 << 26;
/// The far-water shader's default stubs: no screen-space reflection (past the walls only the sky and a few far
/// specks lie along a ray), no refraction, wake or hull shelter (nothing stands behind or beside that water), and
/// `FAR_SHADER_FOAM`. Waves, sky lobe, glints, shadows, fog and the underside are kept.
pub const FAR_SHADER_DEFAULT: u32 = COST_PROBE_SSR | COST_PROBE_REFRACTION | COST_PROBE_WAKE | COST_PROBE_SHELTER
    | FAR_SHADER_FOAM;

/// `SURFACE_SKIP_FADED_CASCADES` on the JKA surface source: the cascade body runs only while `detail > 0`; a faded
/// cascade adds just the variance the full body would (`mix(waves.slopes[i], settled, 0.0) * stilled * roughened *
/// kept * kept`, the same expressions in the same order). The hull share after it is scaled by `detail` and stays
/// outside. The reads are explicit-gradient (`copies_grad`), so they are valid in this non-uniform branch.
fn jka_skip_faded_cascades(source: &str) -> String {
    let s = replace_once(source, "let detail = 1.0 - smoothstep(top - 1.0, top, level);",
        "let detail = 1.0 - smoothstep(top - 1.0, top, level);\n        if detail > 0.0 {");
    replace_once(&s, "        // Near hulls the first cascade's long waves give way", concat!(
        "        } else {\n",
        "            let faded = waves.slopes[i] * (1.0 - calm * waves.slick_shares[i])\n",
        "                * (1.0 + (gain - 1.0) * waves.gust_shares[i]) * shore.scales[i] * shore.scales[i];\n",
        "            variance += faded;\n",
        "            unresolved += faded;\n",
        "        }\n",
        "        // Near hulls the first cascade's long waves give way"))
}

/// Lossy variant bits sharing the probe mask (`r_oceanWaveStatistics` 1 and 2): the geometry-to-BRDF handoff of
/// Bruneton, Neyret & Holzschuch 2010 (filtered slope statistics as in LEAN mapping). Where a pixel's long side has
/// averaged a cascade (upstream's own fully settled variance: the long side over ten of its slope lengths, at full
/// grazing anisotropy) the cascade's mean slope and strain give way to its spectral slope variance and the derivative
/// reads are skipped; over five to ten slope lengths per long side they fade. The variance keeps the
/// energy, so sun glitter and the sky lobe widen to match. The filtered foam read is kept.
pub const SURFACE_WAVE_STATISTICS: u32 = 1 << 27;
/// With `SURFACE_WAVE_STATISTICS`: the bubble cloud's wider spread read (and its breaking-group noise) folds into the
/// filtered foam read over the same fade, so a statistical cascade reads its field once.
pub const SURFACE_WAVE_STATISTICS_FOAM: u32 = 1 << 28;
/// Diagnostic with `SURFACE_WAVE_STATISTICS`: the surface returns how statistical each cascade is (red the finest,
/// green the middle, blue the largest) instead of its colour.
pub const SURFACE_WAVE_STATISTICS_DEBUG: u32 = 1 << 29;
/// `r_oceanType 5`: godot-water-shader's surface (`godot_water_surface.wgsl`) in place of Blue Water's FFT wave sample,
/// water body and foam, lit by Blue Water. The vertex stage's waves are the uniform-driven `jka_hybrid.godot` path,
/// shared by every Hybrid-family module.
pub const SURFACE_GODOT_WATER: u32 = 1 << 30;
/// Opt-in FFT Hybrid material with Godot-style GGX, simple foam and filtered
/// environment reflection. Geometry and every displacement consumer stay native.
pub const SURFACE_FAST_HYBRID: u32 = 1 << 31;
/// `SURFACE_GODOT_WATER` cost probe (a bit the Blue Water probes leave free): the per-pixel wave frame stubbed (flat).
pub const COST_PROBE_GODOT_PIXEL: u32 = 1 << 14;
/// The cost probes that rewrite the FFT wave sample or the foam stack, which `SURFACE_GODOT_WATER` has replaced.
const GODOT_REPLACED_PROBES: u32 = COST_PROBE_WAVES | COST_PROBE_BAKED_READS | COST_PROBE_FOAM | COST_PROBE_FOAM_GALE
    | COST_PROBE_PACK_HALF | COST_PROBE_PACK_HALF_WIDE | COST_PROBE_PACK_BITS;

/// `SURFACE_GODOT_WATER` on the JKA surface source (after its JKA replacements, before the cost probes).
fn jka_godot_material(source: &str, probe: u32) -> String {
    // No FFT reads: a flat sample at upstream's ROUGHNESS 0.01 (slope variance ~1e-4); the slope is the vertex normal's.
    let s = replace_once(source, "let sample = wave_surface(xz, wake.slick, input.world.xz);",
        "let sample = WaveSample(vec3<f32>(0.0, 1.0, 0.0), vec2<f32>(0.0), 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1e-4, 1e-4, vec2<f32>(0.0));");
    let s = replace_once(&s, "let legacy_slope = sample.slope * jka_hybrid.motion_mix.z + jka_hybrid_slope(xz);",
        "let jka_godot = jka_godot_pixel(xz);\n    let legacy_slope = jka_godot.slope;");
    // The waves the pixel averaged away widen the reflection lobe, as Blue Water's unresolved cascades do.
    let s = replace_once(&s, "let variances = wind_variances(sample.unresolved_variance + footprint_variance(dry), wind_speed)",
        "let variances = wind_variances(sample.unresolved_variance + jka_godot.variance + footprint_variance(dry), wind_speed)");
    let s = replace_once(&s, "select(legacy_slope, sample.slope, jka_hybrid.breaker.x > 0.5)", "legacy_slope");
    let s = replace_once(&s, "let crest = clamp(input.world.y / max(waves.sea.x, 0.1), 0.0, 1.0);",
        "let crest = clamp(input.world.y / max(JKA_GODOT_CREST * jka_hybrid.godot.y, 0.1), 0.0, 1.0);");
    let s = replace_once(&s, "let body = water_body(transmitted, deep, column_transmittance(attenuation, backscatter, through, view));",
        "let body = jka_godot_body(xz, input.world, transmitted, through, select(ray_view_z * distance - scene_view_z, -1.0, depth <= 0.0),\n        \
         up, sun_direction, sun_radiance, contact.irradiance, lit, jka_godot.jacobian);");
    // Upstream paints its foam into the albedo (`jka_godot_body`); only a wake's churned water is laid over it.
    let s = replace_span(&s, "    // Foam: the texture laid", "#ifdef POINT_LIGHTS", concat!(
        "    // Foam: godot-water-shader's is in the body's albedo; a wake's churned water is laid over it.\n",
        "    let stretch = surface.foam.x * 2.0 + 1.0;\n",
        "    let patterns = foam_patterns(foam_map, foam_sampler, surface.foam.z, xz, heading, stretch, sun_direction);\n",
        "    let lace = patterns.pattern.r;\n",
        "    let sky_up = env_radiance(sky_atlas, sky_sampler, sky_env, normalize(vec3<f32>(up.x, max(up.y, 0.0), up.z)), 1.0);\n",
        "    let foam_light = foam_radiance(sky_up * contact.sky_share, sun_radiance, up, sun_direction, lit);\n",
        "    let churned = churned_water(wake.foam, wake.young, patterns.pattern, patterns.blur);\n",
        "    let aerated = aeration_with_wake(0.0, wake.bubbles);\n",
        "    let water = mix(bubble_cloud(body, aerated, foam_light, absorption), reflected, aerated_reflectance(reflectance, aerated));\n",
        "    var above = water + direct * beam;\n",
        "    let windrows = 0.0;\n",
        "    let shore = 0.0;\n",
        "    let breakers = 0.0;\n",
        "    let white = billows(patterns.pattern.a, patterns.churn_sunward, lace, churned.dense, patterns.blur, absorption);\n",
        "    above = foam_over(above, water, foam_light * surface.crest_foam.rgb * white, reflected, reflectance, churned.opacity);\n"));
    const CREST_COVER: &str = "let crest_cover = clamp(max(max(whitecaps.opacity * surface.crest_foam.a, churned.opacity), breakers), 0.0, 1.0);";
    assert!(s.contains(CREST_COVER), "Godot water anchor changed: {CREST_COVER}");
    let s = s.replace(CREST_COVER, "let crest_cover = clamp(max(churned.opacity, breakers), 0.0, 1.0);");
    // The FFT probe does not know these waves: the host's eye solve classifies the camera (`pigment.w` 2 submerged).
    let s = replace_once(&s,
        "    let submerged = select(surface.pigment.w > 0.5 && textureLoad(underwater_probe, vec2<i32>(0, 0), 0).g > 0.0, surface.pigment.w > 1.5, jka_hybrid.breaker.x > 0.5);",
        "    let submerged = surface.pigment.w > 1.5;");
    let s = if probe & COST_PROBE_GODOT_PIXEL != 0 {
        replace_once(&s, "let jka_godot = jka_godot_pixel(xz);", "let jka_godot = JkaGodotPixel(vec2<f32>(0.0), 1.0, 0.0);")
    } else { s };
    format!("{s}\n{}", include_str!("godot_water_surface.wgsl"))
}

// The spread read's first line, and its groups' call (the vendored source is CRLF: anchors are single lines).
const WAVE_STATS_SPREAD: &str = "        let spread = mix_mean(copies_grad(field_extras, copies, i, crestward * max(wide, CREST_SPREAD) / tile,";
const WAVE_STATS_SPREAD_GROUPS: &str = "        let spread_shown = crests * breaking_group(xz, spread.y, spread.w, i) * detail;";

/// `SURFACE_WAVE_STATISTICS` (and `_FOAM`) on the JKA surface source. Applied after the cost probes; a foam probe that
/// already rewrote the spread read leaves it alone.
fn jka_wave_statistics(source: &str, fold_foam: bool, debug: bool) -> String {
    let s = replace_once(source, "copies_grad, mix_zero,", "copies_grad, Copies, mix_zero,");
    let s = if debug {
        let s = replace_once(&s, "const SETTLE_FULL: f32 = 0.1;", "const SETTLE_FULL: f32 = 0.1;\nvar<private> jka_statistical: vec3<f32>;");
        let s = replace_once(&s, "    return vec4<f32>(color, 1.0);", "    return vec4<f32>(jka_statistical.zyx, 1.0);");
        replace_once(&s, "        let derivatives = mix_zero(read_derivatives, copies) * kept;",
            "        jka_statistical[i] = statistical;\n        let derivatives = mix_zero(read_derivatives, copies) * kept;")
    } else { s };
    let s = replace_once(&s, "const SETTLE_FULL: f32 = 0.1;",
        "const SETTLE_FULL: f32 = 0.1;\n// `SURFACE_WAVE_STATISTICS`: the share (slope length per long side) below which a cascade's mean slope fades out.\nconst STATS_START: f32 = 0.2;");
    let s = replace_once(&s, "        let read_derivatives = copies_grad(field_derivatives, copies, i, ddx / tile, ddy / tile);", concat!(
        "        // Once the pixel's long side has averaged the cascade, its slopes are their statistics (Bruneton 2010).\n",
        "        let statistical = (1.0 - smoothstep(SETTLE_FULL, STATS_START, waves.slope_lengths[i] / max(wide, 1e-3))) * grazing;\n",
        "        var read_derivatives = Copies(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));\n",
        "        if statistical < 1.0 {\n",
        "            read_derivatives = copies_grad(field_derivatives, copies, i, ddx / tile, ddy / tile);\n",
        "        }"));
    let s = replace_once(&s, "        let derivatives = mix_zero(read_derivatives, copies) * kept;",
        "        let derivatives = mix_zero(read_derivatives, copies) * (kept * (1.0 - statistical));");
    let s = replace_once(&s, "        let extras = vec4<f32>(mix_zero(read_extras, copies).x * kept, means.y, means.z, means.w);",
        "        let extras = vec4<f32>(mix_zero(read_extras, copies).x * (kept * (1.0 - statistical)), means.y, means.z, means.w);");
    // The mean slope's resolved share joins the settled variance as it fades (all of the cascade's at `statistical` 1).
    let s = replace_once(&s, "waves.slopes[i] * max(1.0 - share, 0.0), (1.0 - smoothstep(SETTLE_FULL, SETTLE_START, share)) * grazing);",
        "waves.slopes[i] * max(1.0 - share * (1.0 - statistical), 0.0), (1.0 - smoothstep(SETTLE_FULL, SETTLE_START, share)) * grazing);");
    if !fold_foam || !s.contains(WAVE_STATS_SPREAD) || !s.contains(WAVE_STATS_SPREAD_GROUPS) {
        return s;
    }
    // At `statistical` 1 the spread is the foam read and its groups are the foam's (`means.yw`: the same inputs).
    let s = replace_span(&s, WAVE_STATS_SPREAD, WAVE_STATS_SPREAD_GROUPS, concat!(
        "        var spread = means;\n",
        "        var spread_shown = shown;\n",
        "        if statistical < 1.0 {\n",
        "            let read_spread = mix_mean(copies_grad(field_extras, copies, i, crestward * max(wide, CREST_SPREAD) / tile,\n",
        "                wind * max(wide, CROSS_SPREAD) / tile), copies) * (kept * kept);\n",
        "            spread = mix(read_spread, means, statistical);\n",
        "            spread_shown = crests * breaking_group(xz, spread.y, spread.w, i) * detail;\n",
        "        }"));
    replace_once(&s, WAVE_STATS_SPREAD_GROUPS, "")
}

fn replace_once(source: &str, from: &str, to: &str) -> String {
    assert_eq!(source.matches(from).count(), 1, "cost probe anchor changed: {from}");
    source.replace(from, to)
}

/// Replace `[start, end)`, where `end` is its first match after `start`.
fn replace_span(source: &str, start: &str, end: &str, to: &str) -> String {
    assert_eq!(source.matches(start).count(), 1, "cost probe span start changed: {start}");
    let from = source.find(start).unwrap();
    let until = from + source[from..].find(end).unwrap_or_else(|| panic!("cost probe span end missing: {end}"));
    format!("{}{}{}", &source[..from], to, &source[until..])
}

/// The `r_oceanCostProbe` stubs on the JKA surface source (after its JKA replacements, before admission).
fn jka_defer_body(source: &str) -> String {
    let start = source.find("    // The opaque scene behind this pixel").expect("water body start");
    let end = source[start..].find("    // Fresnel between the reflected sky").expect("water body end") + start;
    let body = source[start..end].to_owned();
    let mut source = source.to_owned();
    source.replace_range(start..end, "");
    let after = source.find("    // How far up a wave this point sits").expect("reflection result");
    source.insert_str(after, &body);
    source
}

fn jka_serial_refinement(source: &str) -> String {
    // The original reads middle/lower/upper, then uses only middle and one
    // quarter. These eight sequential bisections visit exactly those same
    // selected points; the unselected four depth reads are unnecessary.
    let start = source.find("                for (var r = 0; r < REFINEMENTS; r += 2)").expect("SSR refinement start");
    let end = source[start..].find("                // Accept a surface within").expect("SSR refinement end") + start;
    let mut result = source.to_owned();
    result.replace_range(start..end, concat!(
        "                for (var r = 0; r < REFINEMENTS; r++) {\n",
        "                    let middle = (low + high) * 0.5;\n",
        "                    let centre = probe_at(ray, middle, depth_along(ray, depth_copy, middle));\n",
        "                    if crossed(centre, edge, front_scene) {\n",
        "                        high = middle;\n",
        "                        refined = centre;\n",
        "                    } else { low = middle; }\n",
        "                }\n"));
    result
}

fn jka_cost_probe_source(source: &str, probe: u32) -> String {
    let mut s = source.to_owned();
    let on = |bit: u32| probe & bit != 0;
    if on(COST_PROBE_WAKE) {
        s = replace_once(&s, "#import naval_ocean::wake::{wake_height,", "#import naval_ocean::wake::{WakeSurface, wake_height,");
        s = replace_once(&s, "let wake = wake_surface(xz, input.world, footprint);",
            "let wake = WakeSurface(vec3<f32>(0.0, 1.0, 0.0), 0.0, 0.0, 0.0, 0.0, 0.0);");
    }
    if on(COST_PROBE_WAVES) {
        // The Hybrid breaker prepends its cofactor normal to the sample.
        let normal = if s.contains("jka_normal: vec3<f32>,") { "vec3<f32>(0.0, 1.0, 0.0), " } else { "" };
        s = replace_once(&s, "let sample = wave_surface(xz, wake.slick, input.world.xz);",
            &format!("let sample = WaveSample({normal}vec2<f32>(0.0), 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.01, 0.01, vec2<f32>(0.0));"));
    }
    if on(COST_PROBE_RAIN) {
        s = replace_once(&s, "#import naval_ocean::rain::rain_surface", "#import naval_ocean::rain::{RainSurface, rain_surface}");
        s = replace_once(&s, "let rain = rain_surface(xz, waves.wind.z, surface.weather.x, pixel_size);",
            "let rain = RainSurface(vec2<f32>(0.0), 0.0);");
    }
    if on(COST_PROBE_SHADOWS) {
        s = replace_span(&s, "let shadow = water_sun_shadow(", "\n", "let shadow = 1.0;");
    }
    if on(COST_PROBE_SHELTER) {
        s = replace_once(&s, "shelter_light(hull_shelter(input.world, lobe.direction, lobe.in_plane), daylight,",
            "shelter_light(vec4<f32>(0.0), daylight,");
    }
    if on(COST_PROBE_REFRACTION) {
        s = replace_once(&s, "let bent = jka_refracted_scene(input.world, -view, up, 1.0 / 1.333, column * 2.0, true);",
            "let bent = vec4<f32>(0.0, 0.0, 0.0, -1.0);");
    }
    if on(COST_PROBE_SKY) {
        s = replace_once(&s, "let sky = shelter_reflect(contact, sky_lobe(sky_atlas, sky_sampler, sky_env, lobe));",
            "let sky = daylight * 0.1;");
    }
    if on(COST_PROBE_SSR) {
        s = replace_span(&s, "let traced = screen_space_reflection(", "    // How far up a wave", "let reflected = sky;\n");
    }
    if on(COST_PROBE_GLINTS) {
        s = replace_once(&s, "let direct = glint + crest_transmission(", "let direct = crest_transmission(");
    }
    if on(COST_PROBE_BAKED_READS) {
        // What a baked world-space wave/foam cache would cost per pixel: the wave sample and the foam patterns each
        // replaced by four anisotropic reads at the pixel's own footprint (field_extras stands in for the cache).
        s = replace_once(&s, "#import naval_ocean::foam::{FOAM_GLOSS,", "#import naval_ocean::foam::{FoamPatterns, FOAM_GLOSS,");
        let normal = if s.contains("jka_normal: vec3<f32>,") { "vec3<f32>(b0.xy, 1.0), " } else { "" };
        s = replace_once(&s, "let sample = wave_surface(xz, wake.slick, input.world.xz);", &format!(concat!(
            "let bake_dx = dpdx(xz) / 64.0;\n",
            "    let bake_dy = dpdy(xz) / 64.0;\n",
            "    let b0 = textureSampleGrad(field_extras, field_sampler, xz / 64.0, 0, bake_dx, bake_dy);\n",
            "    let b1 = textureSampleGrad(field_extras, field_sampler, xz / 64.0 + 0.31, 1, bake_dx, bake_dy);\n",
            "    let b2 = textureSampleGrad(field_extras, field_sampler, xz / 64.0 + 0.57, 2, bake_dx, bake_dy);\n",
            "    let b3 = textureSampleGrad(field_extras, field_sampler, xz / 64.0 + 0.83, 0, bake_dx, bake_dy);\n",
            "    let sample = WaveSample({}b0.xy * 0.01, 1.0 + b0.z * 0.01, abs(b1.x), abs(b1.y), abs(b1.z), abs(b1.w) * 0.1,\n",
            "        abs(b2.x), abs(b2.y) * 0.01 + 0.001, abs(b2.z) * 0.01 + 0.001, abs(b3.xy));"), normal));
        s = replace_span(&s, "let patterns = foam_patterns(", "\n", concat!(
            "let f0 = textureSampleGrad(field_extras, field_sampler, xz / 48.0, 1, bake_dx * 1.3, bake_dy * 1.3);\n",
            "    let f1 = textureSampleGrad(field_extras, field_sampler, xz / 48.0 + 0.2, 2, bake_dx * 1.3, bake_dy * 1.3);\n",
            "    let f2 = textureSampleGrad(field_extras, field_sampler, xz / 48.0 + 0.4, 0, bake_dx * 1.3, bake_dy * 1.3);\n",
            "    let f3 = textureSampleGrad(field_extras, field_sampler, xz / 48.0 + 0.6, 1, bake_dx * 1.3, bake_dy * 1.3);\n",
            "    let patterns = FoamPatterns(abs(f0), abs(f1.x), abs(f1.y), abs(f1.z), abs(f1.w), abs(f2.x), abs(f3), abs(f2.y), abs(f2.z));"));
    }
    if on(COST_PROBE_FOAM_SPREAD) {
        s = replace_span(&s, "let spread = mix_mean(copies_grad(field_extras", "        let spread_shown", "let spread = vec4<f32>(0.0);\n");
    }
    if on(COST_PROBE_FOAM_GROUPS) {
        s = replace_once(&s, "breaking_group(xz, extras.y, extras.w, i)", "1.0");
        s = replace_once(&s, "breaking_group(xz, spread.y, spread.w, i)", "1.0");
    }
    if on(COST_PROBE_FOAM_GALE) {
        s = replace_span(&s, "    // In a gale the whitecaps' old foam drifts", "    windrows *= surface.crest_foam.a;", "");
    }
    if on(FAR_SHADER_FOAM) && !on(COST_PROBE_FOAM_PATTERNS) && !on(COST_PROBE_BAKED_READS) {
        // FoamPatterns(pattern, blur, streaks, streaks_blur, lines, gather, coarse, coarse_blur, churn_sunward): every
        // blur 1, so each pattern value is weighted by zero; `lines` and `gather` are what upstream mixes to at a
        // coarse blur of 1 (`streak_mean`, the `surface.foam.z` it is called with, and ½).
        s = replace_once(&s, "#import naval_ocean::foam::{FOAM_GLOSS,", "#import naval_ocean::foam::{FoamPatterns, FOAM_GLOSS,");
        s = replace_span(&s, "let patterns = foam_patterns(", "\n",
            "let patterns = FoamPatterns(vec4<f32>(0.5), 1.0, 0.5, 1.0, surface.foam.z, 0.5, vec4<f32>(0.5), 1.0, 0.5);");
    }
    if on(COST_PROBE_FOAM_PATTERNS) {
        s = replace_once(&s, "#import naval_ocean::foam::{FOAM_GLOSS,", "#import naval_ocean::foam::{FoamPatterns, FOAM_GLOSS,");
        s = replace_span(&s, "let patterns = foam_patterns(", "\n",
            "let patterns = FoamPatterns(vec4<f32>(0.5), 0.0, 0.5, 0.0, 0.5, 0.5, vec4<f32>(0.5), 0.0, 0.5);");
    }
    if on(COST_PROBE_FOAM) {
        s = replace_span(&s, "    // Foam: the texture laid", "#ifdef POINT_LIGHTS", concat!(
            "    let aerated = 0.0;\n",
            "    let foam_light = vec3<f32>(0.0);\n",
            "    let water = mix(body, reflected, reflectance);\n",
            "    var above = water + direct * beam;\n",
            "    let windrows = 0.0;\n",
            "    let shore = 0.0;\n"));
        s = s.replace("let crest_cover = clamp(max(max(whitecaps.opacity * surface.crest_foam.a, churned.opacity), breakers), 0.0, 1.0);",
            "let crest_cover = 0.0;");
    }
    if on(COST_PROBE_FOG) {
        s = replace_once(&s, "var color = jka_surface_legacy_fog(apply_fog(above, input.world), input.world);", "var color = above;");
    }
    if on(COST_PROBE_UNDERSIDE) {
        s = replace_span(&s, "if !front && (submerged", "{", "if false ");
    }
    if on(COST_PROBE_DEFER_BODY)
        && !on(COST_PROBE_PACK_HALF | COST_PROBE_PACK_HALF_WIDE | COST_PROBE_PACK_BITS) {
        s = jka_defer_body(&s);
    }
    s
}

/// `COST_PROBE_PACK_HALF` (and `_WIDE`) on the JKA surface source. Values are packed once all are produced (before
/// the reflection march) and unpacked in nested blocks that shadow the originals by name, so the material code after
/// them reads the halves unchanged: the foam's inputs after `foam_patterns` (its texture reads), the wide set's sky and
/// variances just after the march. The underside's beam is hoisted (same expression) so `optics` need not stay live.
fn jka_pack_half_source(source: &str, wide: bool, bits: bool) -> String {
    let mut s = replace_once(source, "    let to_camera = camera.position.xyz - input.world;", concat!(
        "    let jka_pk_s0 = pack2x16float(vec2<f32>(sample.foam, sample.fresh));\n",
        "    let jka_pk_s1 = pack2x16float(vec2<f32>(sample.foam_soft, sample.whitecap_share));\n",
        "    let jka_pk_s2 = pack2x16float(vec2<f32>(sample.bubbles, sample.jacobian));\n",
        "    let jka_pk_s3 = pack2x16float(sample.breakers);\n",
        "    let jka_pk_w0 = pack2x16float(vec2<f32>(wake.foam, wake.young));\n",
        "    let to_camera = camera.position.xyz - input.world;"));
    let mut late = String::from(concat!(
        "    let jka_pk_l0 = pack2x16float(vec2<f32>(lit, beam));\n",
        "    let jka_pk_l1 = pack2x16float(vec2<f32>(contact.sky_share, wake.bubbles));\n",
        "    let jka_pk_b0 = pack2x16float(body.rg);\n",
        "    let jka_pk_b1 = pack2x16float(vec2<f32>(body.b, column));\n",
        "    let jka_pk_a0 = pack2x16float(absorption.rg);\n",
        "    let jka_pk_a1 = pack2x16float(vec2<f32>(absorption.b, reflectance));\n",
        "    let jka_under_beam = select(absorption, water_beam(optics), surface.water.absorption.w > 0.5);\n",
        "    let jka_pk_u0 = pack2x16float(pigment.rg);\n",
        "    let jka_pk_u1 = pack2x16float(vec2<f32>(pigment.b, jka_under_beam.r));\n",
        "    let jka_pk_u2 = pack2x16float(jka_under_beam.gb);\n"));
    if wide {
        late.push_str(concat!(
            "    let jka_pk_k0 = pack2x16float(min(sky.rg, vec2<f32>(65000.0)));\n",
            "    let jka_pk_k1 = pack2x16float(vec2<f32>(min(sky.b, 65000.0), variances.x));\n",
            "    let jka_pk_d0 = pack2x16float(min(daylight.rg, vec2<f32>(65000.0)));\n",
            "    let jka_pk_d1 = pack2x16float(vec2<f32>(min(daylight.b, 65000.0), variances.y));\n"));
    }
    s = replace_once(&s, "    let to_view = camera.view;", &format!("{late}    let to_view = camera.view;"));
    let mut closing = 1;
    if wide {
        closing += 1;
        s = replace_once(&s, "    let reflected = mix(sky, traced.color, traced.confidence);", concat!(
            "    {\n",
            "    let jka_k1 = unpack2x16float(jka_pk_k1);\n",
            "    let jka_d1 = unpack2x16float(jka_pk_d1);\n",
            "    let sky = vec3<f32>(unpack2x16float(jka_pk_k0), jka_k1.x);\n",
            "    let variances = vec2<f32>(jka_k1.y, jka_d1.y);\n",
            "    let reflected = mix(sky, traced.color, traced.confidence);"));
    }
    let daylight = if wide {
        "    let daylight = vec3<f32>(unpack2x16float(jka_pk_d0), jka_d1.x);\n"
    } else { "" };
    // With `COST_PROBE_FOAM` the unpack goes where its stub starts (to size the pack across the reflection alone).
    let unpack_at = if s.contains("    let lace = patterns.pattern.r;") { "    let lace = patterns.pattern.r;" } else { "    let aerated = 0.0;" };
    s = replace_once(&s, unpack_at, &format!(concat!(
        "    {{\n",
        "    let jka_s0 = unpack2x16float(jka_pk_s0);\n",
        "    let jka_s1 = unpack2x16float(jka_pk_s1);\n",
        "    let jka_s2 = unpack2x16float(jka_pk_s2);\n",
        "    let sample = JkaPackedSample(jka_s0.x, jka_s0.y, jka_s1.x, jka_s1.y, jka_s2.x, jka_s2.y, unpack2x16float(jka_pk_s3));\n",
        "    let jka_w0 = unpack2x16float(jka_pk_w0);\n",
        "    let jka_l0 = unpack2x16float(jka_pk_l0);\n",
        "    let jka_l1 = unpack2x16float(jka_pk_l1);\n",
        "    let wake = JkaPackedWake(jka_w0.x, jka_l1.y, jka_w0.y);\n",
        "    let contact = JkaPackedContact(jka_l1.x);\n",
        "    let lit = jka_l0.x;\n",
        "    let beam = jka_l0.y;\n",
        "    let jka_b1 = unpack2x16float(jka_pk_b1);\n",
        "    let body = vec3<f32>(unpack2x16float(jka_pk_b0), jka_b1.x);\n",
        "    let column = jka_b1.y;\n",
        "    let jka_a1 = unpack2x16float(jka_pk_a1);\n",
        "    let absorption = vec3<f32>(unpack2x16float(jka_pk_a0), jka_a1.x);\n",
        "    let reflectance = jka_a1.y;\n",
        "    let jka_u1 = unpack2x16float(jka_pk_u1);\n",
        "    let pigment = vec3<f32>(unpack2x16float(jka_pk_u0), jka_u1.x);\n",
        "{}",
        "{}"), daylight, unpack_at));
    s = replace_once(&s, "select(absorption, water_beam(optics), surface.water.absorption.w > 0.5), sun_direction, sky_env.light_color);",
        "vec3<f32>(jka_u1.y, unpack2x16float(jka_pk_u2)), sun_direction, sky_env.light_color);");
    s = replace_once(&s, "    return vec4<f32>(color, 1.0);", &format!("    return vec4<f32>(color, 1.0);{}", "\n    }".repeat(closing)));
    s.push_str(concat!(
        "\nstruct JkaPackedSample { foam: f32, fresh: f32, foam_soft: f32, whitecap_share: f32, bubbles: f32, jacobian: f32, breakers: vec2<f32> }\n",
        "struct JkaPackedWake { foam: f32, bubbles: f32, young: f32 }\n",
        "struct JkaPackedContact { sky_share: f32 }\n"));
    if bits {
        s = s.replace("unpack2x16float(", "jka_unpack_bf16(").replace("pack2x16float(", "jka_pack_bf16(");
        s.push_str(concat!(
            "fn jka_pack_bf16(v: vec2<f32>) -> u32 {\n",
            "    let r = bitcast<vec2<u32>>(v) + vec2<u32>(0x8000u);\n",
            "    return (r.x >> 16u) | (r.y & 0xffff0000u);\n",
            "}\n",
            "fn jka_unpack_bf16(p: u32) -> vec2<f32> {\n",
            "    return bitcast<vec2<f32>>(vec2<u32>(p << 16u, p & 0xffff0000u));\n",
            "}\n"));
    }
    s
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
                  "let depth = select(-1.0, height_at(ray.start.xz) + wake_height(ray.start.xz) - ray.start.y, jka_water_column_contains(ray.start));")
                .replace("vec4<f32>(depth, height_at(eye.xz) - eye.y, 0.0, 1.0)",
                      "vec4<f32>(depth, select(-1.0, height_at(eye.xz) - eye.y, jka_water_column_contains(eye)), 0.0, 1.0)");
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
                  "let pixel = clamp(vec2<i32>(uv * camera.viewport.xy), vec2<i32>(0), vec2<i32>(camera.viewport.xy) - 1);\n    let bounded = select(0.0, step(range, textureLoad(jka_water_distance, pixel, 0).r), jka_water_column_contains(world));\n    let shown = bounded * smoothstep(NEAR.x, NEAR.y, range)");
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
        include_str!("surface_boundary.wgsl"), format!("{}\n{}\n{}", include_str!("crest_control.wgsl"),
            include_str!("hybrid_gerstner.wgsl"), include_str!("godot_water.wgsl")));
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
    // The probe binds no noise texture: `r_oceanType 5`'s waves are read at the noise's mean, which drops only
    // upstream's +-2.5 cm (times the wave scale) height jitter from the eye's waterline.
    let source = format!("{}\n{}\n{}\nfn jka_godot_noise(uv: vec2<f32>) -> f32 {{ return 0.5; }}\n",
        include_str!("eye_height.wgsl"), include_str!("crest_control.wgsl"), include_str!("godot_water.wgsl"));
    composer.make_naga_module(NagaModuleDescriptor {
        source: &source,
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
        &format!("{SKY_BINDING}\n{}\n{}\n{}", include_str!("crest_control.wgsl"), include_str!("hybrid_gerstner.wgsl"),
            include_str!("godot_water.wgsl")));
    let replace = |text: String, from: &str, to: &str| {
        assert_eq!(text.matches(from).count(), 1, "Hybrid breaker anchor changed: {from}");
        text.replace(from, to)
    };
    let source = replace(source, "struct WaveSample {", "struct WaveSample {\n    jka_normal: vec3<f32>,");
    let source = replace(source, "    let xx = strain.x + 1.0;", r#"    if jka_hybrid.breaker.x > 0.5 {
        // Apply the same constant horizontal map as the vertices BEFORE
        // transforming the combined grid slope into a world normal.
        slope = slope * jka_hybrid.motion_mix.z + jka_hybrid_slope(xz);
        strain = (strain * jka_hybrid.motion_mix.z + jka_hybrid_strain(xz)) * jka_hybrid_crest_scale();
    }
    let xx = strain.x + 1.0;"#);
    let source = replace(source, "        world + ripple_slope + hulls.world,", r#"        vec3<f32>(-(zz * slope.x - cross_strain * slope.y), jacobian,
            -(xx * slope.y - cross_strain * slope.x))
            - vec3<f32>(ripple_slope.x + hulls.world.x, 0.0, ripple_slope.y + hulls.world.y) * jacobian,
        world + ripple_slope + hulls.world,"#);
    let source = replace(source, "    let dry = normalize(vec3<f32>(-total.x, 1.0, -total.y));", r#"    var dry = normalize(vec3<f32>(-total.x, 1.0, -total.y));
    if jka_hybrid.breaker.x > 0.5 {
        // The cofactor normal remains finite and retains its orientation as
        // the crest passes vertical. Do not divide by a clamped determinant.
        let wake_slope = -wake.normal.xz / max(wake.normal.y, 0.05);
        let signed = sample.jka_normal - vec3<f32>(wake_slope.x, 0.0, wake_slope.y) * sample.jacobian;
        dry = normalize(signed + vec3<f32>(0.0, 1e-6, 0.0));
        if surface.pigment.w < 1.5 && dot(dry, view) < 0.0 { dry = -dry; }
    }"#);
    let source = replace(source, "    let up = normalize(vec3<f32>(-total.x - rain.slope.x, 1.0, -total.y - rain.slope.y));", r#"    var up = normalize(vec3<f32>(-total.x - rain.slope.x, 1.0, -total.y - rain.slope.y));
    if jka_hybrid.breaker.x > 0.5 {
        up = normalize(dry - vec3<f32>(rain.slope.x, 0.0, rain.slope.y) * abs(dry.y));
    }"#);
    let source = replace(source, "    let breakers = foam_share(sample.breakers.x, lace, patterns.blur, SURF_EDGE) * surface.crest_foam.a;", r#"    var crest_breaking = 0.0;
    if jka_hybrid.breaker.x > 1.5 {
        // Use the existing textured foam and persistent FFT whitecap history;
        // do not paint an opaque stripe or introduce another water surface.
        let high = smoothstep(0.0, 0.22, input.world.y / max(waves.sea.x, 0.1));
        let compressed = 1.0 - smoothstep(0.15, 0.7, sample.jacobian);
        crest_breaking = compressed * high;
    }
    let breakers = foam_share(max(sample.breakers.x, crest_breaking), lace, patterns.blur, SURF_EDGE) * surface.crest_foam.a;"#);
    let source = replace(source,
        "    let submerged = surface.pigment.w > 0.5 && textureLoad(underwater_probe, vec2<i32>(0, 0), 0).g > 0.0;",
        "    let submerged = select(surface.pigment.w > 0.5 && textureLoad(underwater_probe, vec2<i32>(0, 0), 0).g > 0.0, surface.pigment.w > 1.5, jka_hybrid.breaker.x > 0.5);");
    let source = replace(source, "    if !front && (submerged || input.world.y > camera.position.y) {",
        "    if !front && (submerged || (jka_hybrid.breaker.x < 0.5 && input.world.y > camera.position.y)) {");
    // Both regular windrows and the gale's `max(windrows, streaks)` path
    // must obey the same sea-energy gate. Suppress final windrow coverage
    // AFTER the gale maximum, rather than patching one optional branch and
    // accidentally leaving other white patterns fully opaque at 0.1 m/s.
    const WINDROWS_END: &str = "    let white = billows(patterns.pattern.a,";
    const BUBBLES: &str = "let aerated = aeration_with_wake(sample.bubbles * (1.0 - whitecaps.averaged), wake.bubbles);";
    assert_eq!(source.matches(WINDROWS_END).count(), 1,
        "Blue Water windrow gate anchor changed upstream");
    source.replace(VERTEX, "let offset = jka_hybrid_displacement(grid, spacing);")
        .replace("let da = displacement(pa, spacing);", "let da = jka_hybrid_parent_displacement(pa, spacing);")
        .replace("let db = displacement(pb, spacing);", "let db = jka_hybrid_parent_displacement(pb, spacing);")
        .replace("let dc = displacement(pc, spacing);", "let dc = jka_hybrid_parent_displacement(pc, spacing);")
        .replace(NORMAL, "let legacy_slope = sample.slope * jka_hybrid.motion_mix.z + jka_hybrid_slope(xz);\n    let total = select(legacy_slope, sample.slope, jka_hybrid.breaker.x > 0.5) - wake.normal.xz / max(wake.normal.y, 0.05);")
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
    compose_surface_probed(composer, point_lights, jka_boundary, reflection_steps, pixel_detail_mode, near_detail, 0, false)
}

fn compose_surface_probed(
    composer: &mut Composer,
    point_lights: bool,
    jka_boundary: bool,
    reflection_steps: u32,
    pixel_detail_mode: u8,
    near_detail: bool,
    cost_probe: u32,
    // `r_oceanPixelDetail` 5's full-resolution pass: the material only on the 2x2 blocks `jka_adaptive_full` picks.
    adaptive_full: bool,
) -> Result<wgpu::naga::Module, Box<ComposerError>> {
    let mut shader_defs = vec![
        ("CASCADES".to_owned(), ShaderDefValue::UInt(3)),
        ("REFLECTION_STEPS".to_owned(), ShaderDefValue::UInt(reflection_steps)),
    ];
    let fast_hybrid = cost_probe & SURFACE_FAST_HYBRID != 0 && cost_probe & SURFACE_GODOT_WATER == 0;
    if fast_hybrid && cost_probe & COST_PROBE_SSR == 0 {
        shader_defs.push(("JKA_FAST_SSR".to_owned(), ShaderDefValue::UInt(1)));
    }
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
        let godot = cost_probe & SURFACE_GODOT_WATER != 0;
        // Cost probes alone (no lossless/lossy variant bits) decide whether reduced modes split the glints.
        let probe_only = cost_probe & !SURFACE_GODOT_WATER;
        let cost_probe = if fast_hybrid {
            cost_probe & (SURFACE_FAST_HYBRID | COST_PROBE_SSR | COST_PROBE_FLOOR)
        } else if godot { cost_probe & !GODOT_REPLACED_PROBES } else { cost_probe };
        let original = if godot { jka_godot_material(&original, cost_probe) } else { original };
        // Reduced-resolution variants execute the original *entire* material
        // on a smaller attachment. Only the screen-space opaque scene lookup
        // needs remapping back to full-resolution source textures.
        let scale = pixel_scale(pixel_detail_mode);
        // The floor probe returns before the material: the rest of the body is unreachable.
        let admission = if cost_probe & COST_PROBE_FLOOR != 0 {
            "\n    if !jka_water_contains(input.grid) { discard; }\n    return vec4<f32>(0.02, 0.08, 0.12, 1.0);"
        } else {
            "\n    if !jka_water_contains(input.grid) { discard; }"
        };
        let original = if cost_probe & SURFACE_SKIP_FADED_CASCADES != 0 { jka_skip_faded_cascades(&original) } else { original };
        let core = if fast_hybrid {
            let start = original.find(entry).expect("native fragment");
            let start = original[..start].rfind("@fragment").expect("native fragment attribute");
            let end = start + original[start..].find("// The motion pass for temporal anti-aliasing").expect("native motion pass");
            let tint = crate::original_surface::SurfaceAppearance::default().transmission;
            let fast = include_str!("hybrid_fast_surface.wgsl").replace("JKA_FAST_TRANSMISSION",
                &format!("vec3<f32>({}, {}, {})", tint[0], tint[1], tint[2]));
            format!("{}\n{}\n{}", &original[..start], fast, &original[end..])
                .replace("#import naval_ocean::foam::{FOAM_GLOSS,", "#import naval_ocean::foam::{FoamPatterns, FOAM_GLOSS,")
        } else { jka_cost_probe_source(&original, cost_probe) };
        let core = if cost_probe & SURFACE_WAVE_STATISTICS != 0 {
            jka_wave_statistics(&core, cost_probe & SURFACE_WAVE_STATISTICS_FOAM != 0,
                cost_probe & SURFACE_WAVE_STATISTICS_DEBUG != 0)
        } else { core };
        let core = if cost_probe & (COST_PROBE_PACK_HALF | COST_PROBE_PACK_HALF_WIDE | COST_PROBE_PACK_BITS) != 0 {
            // `pack2x16float` is core WGSL; wgpu grants this on every adapter with `DownlevelFlags::SHADER_F16_IN_F32`
            // (all desktop Vulkan/DX12), but naga_oil validates with default capabilities.
            composer.capabilities |= wgpu::naga::valid::Capabilities::SHADER_FLOAT16_IN_FLOAT32;
            jka_pack_half_source(&core, cost_probe & COST_PROBE_PACK_HALF_WIDE != 0, cost_probe & COST_PROBE_PACK_BITS != 0)
        } else { core };
        // Upstream treats the atlas as globally visible. A closed BSP room must
        // not reflect its sky lobe or solar halo. Do this after cost probes:
        // their exact-string anchors still see the unmodified upstream material.
        let core = core.replace(
            "let sun_radiance = sky_env.light_color * sun_intensity;",
            "let sun_radiance = sky_env.light_color * sun_intensity * jka_admission;",
        ).replace(
            "return aerial_apply(color, world);",
            "return mix(color, aerial_apply(color, world), jka_water_sky_admission(world.xz));",
        ).replace(
            "let sky = shelter_reflect(contact, sky_lobe(sky_atlas, sky_sampler, sky_env, lobe));",
            "let sky = shelter_reflect(contact, sky_lobe(sky_atlas, sky_sampler, sky_env, lobe)) * jka_admission;",
        );
        let core = core.replace(entry, &format!("{entry}{admission}\n    let jka_admission = jka_water_sky_admission(input.grid);"));
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
        let core = if adaptive_full {
            // Whole 2x2 quads leave together (the decision is per block), before any derivative is taken.
            // WGSL's `discard` demotes to a helper that runs the rest of the material: the return is what saves the
            // work. The decision is per 2x2 block (one quad), so no live lane loses a neighbour's derivatives; the
            // diagnostic is off for that reason, as in the near-detail variant.
            let edited = jka_adaptive_bindings(&jka_invariant_position(&core)).replace(
                &format!("{entry}{admission}"),
                &format!("{entry}{admission}\n    if !jka_adaptive_full(vec2<i32>(input.clip.xy)) {{ discard; return vec4<f32>(0.0); }}"));
            let edited = replace_once(&edited, "return vec4<f32>(color, 1.0);",
                "return vec4<f32>(jka_adaptive_tint(color, vec2<i32>(input.clip.xy)), 1.0);");
            format!("diagnostic(off, derivative_uniformity);\n{edited}\n{ADAPTIVE_WGSL}")
        } else { core };
        let core = if scale == 1 {
            core
        } else {
            let pixel = "let pixel = vec2<i32>(input.clip.xy);";
            assert_eq!(core.matches(pixel).count(), 1, "upstream pixel coordinate contract changed");
            // The material keeps the full-resolution pixel's footprint (`full_resolution_footprint`).
            let core = core.replace(pixel, &format!("let pixel = vec2<i32>(input.clip.xy * f32({scale}));"));
            let core = jka_reduced_glints(&core, entry, probe_only == 0);
            full_resolution_footprint(&core, scale)
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
    // `SURFACE_GODOT_WATER` or 0, as the main program.
    variant: u32,
) -> Result<wgpu::naga::Module, Box<ComposerError>> {
    compose_surface_probed(composer, false, true, reflection_steps.clamp(4, 32), 0, true, variant & SURFACE_GODOT_WATER, false)
}

/// Full-resolution depth/coverage reconstruction. The original vertex stage
/// is retained: there is NO flat-water proxy, and the JKA footprint test is
/// applied at full resolution. Color comes from actual Blue Water shading at
/// 1/2 or 1/4 resolution, with alpha- and depth-aware reconstruction.
pub fn jka_reconstruct_module(
    composer: &mut Composer,
) -> Result<wgpu::naga::Module, Box<ComposerError>> {
    jka_reconstruct_from_source(composer, include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/surface.wgsl"), false)
}

/// `r_oceanPixelDetail` 5 (adaptive 2x2): the reconstruction, drawn only on the 2x2 blocks `jka_adaptive_full` leaves
/// to it. [`jka_adaptive_surface_module`] draws the rest with the full material, so each pixel is drawn once.
pub fn jka_adaptive_reconstruct_module(
    composer: &mut Composer,
) -> Result<wgpu::naga::Module, Box<ComposerError>> {
    jka_reconstruct_from_source(composer, include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/surface.wgsl"), true)
}

/// `r_oceanPixelDetail` 5: the full-resolution original material on the 2x2 blocks the 2x2 reduced shading cannot
/// reconstruct (see `ADAPTIVE_WGSL`), opaque over the reconstruction's complementary blocks.
pub fn jka_adaptive_surface_module(
    composer: &mut Composer,
    reflection_steps: u32,
    // `SURFACE_GODOT_WATER` or 0, as the main program.
    variant: u32,
) -> Result<wgpu::naga::Module, Box<ComposerError>> {
    compose_surface_probed(composer, false, true, reflection_steps.clamp(4, 32), 0, false, variant & SURFACE_GODOT_WATER, true)
}

/// The reduced-resolution material (`r_oceanPixelDetail` 1-5) without its glints, plus what the full-resolution
/// reconstruction needs to draw them per pixel. A glint is a per-pixel draw (`glitter_glints`: a Poisson count on cells
/// about a pixel across), so a reduced pixel's glint upscaled is a 2x2 (or 4x4) square. Everything else is smooth.
/// Location 0 is the colour with `glint = 0`; 1 the glitter's mean (`soft_knee(sun_glitter)`, rgb) and slope density
/// (a); 2 the share of a glint that reaches the pixel's colour (rgb): every step after `above = water + direct * beam`
/// (the four `foam_over` layers, the spray veil, both fogs) is affine per channel, so a second `above` carrying a unit
/// glint more through the same statements, less the colour, is that share exactly. Back faces seen from below show
/// none. The reconstruction adds `share * luminance_ceiling(mean * glitter_glints(its pixel), peak)`, as the material
/// would. `split` false (cost probes) keeps the glints in the colour and writes zero glitter.
fn jka_reduced_glints(source: &str, entry: &str, split: bool) -> String {
    let mut s = replace_once(source, entry, &entry.replace("-> @location(0) vec4<f32> {", "-> JkaReducedOut {"));
    s.push_str(concat!(
        "\nstruct JkaReducedOut {\n",
        "    @location(0) color: vec4<f32>,\n",
        "    @location(1) glitter: vec4<f32>,\n",
        "    @location(2) transfer: vec4<f32>,\n",
        "}\n"));
    s = s.replace("return vec4<f32>(0.02, 0.08, 0.12, 1.0);",
        "return JkaReducedOut(vec4<f32>(0.02, 0.08, 0.12, 1.0), vec4<f32>(0.0), vec4<f32>(0.0));");
    if !split {
        return replace_once(&s, "return vec4<f32>(color, 1.0);",
            "return JkaReducedOut(vec4<f32>(color, 1.0), vec4<f32>(0.0), vec4<f32>(0.0));");
    }
    s = replace_span(&s, "let glint = luminance_ceiling(", "    let direct = ", concat!(
        "let jka_glitter_mean = soft_knee(sun_glitter(up, view, sun_direction, sun_radiance, variances, downwind), ",
        "sun_intensity * GLITTER_KNEE);\n",
        "    let jka_glitter_density = glitter_density(up, view, sun_direction, variances, downwind);\n",
        "    let glint = vec3<f32>(0.0);\n"));
    s = replace_once(&s, "var above = water + direct * beam;",
        "var above = water + direct * beam;\n    var jka_glint_above = above + vec3<f32>(beam);");
    // Each statement that rewrites `above`, once more on the glint-carrying copy.
    let mut out = String::with_capacity(s.len() + 1024);
    let mut rest = s.as_str();
    let mut statements = 0;
    while let Some(at) = rest.find("above = ") {
        let line_start = rest[..at].rfind('\n').map_or(0, |n| n + 1);
        let indent = &rest[line_start..at];
        let is_statement = indent.chars().all(char::is_whitespace)
            && !rest[..at].ends_with("jka_glint_") && !rest[..at].ends_with("var ");
        let end = at + rest[at..].find(';').expect("statement end") + 1;
        out.push_str(&rest[..end]);
        if is_statement {
            let copy = rest[at..end].replacen("above = ", "jka_glint_above = ", 1)
                .replace("(above,", "(jka_glint_above,").replace(", above,", ", jka_glint_above,");
            assert!(copy.contains("jka_glint_above,"), "glint transfer: `above` statement shape changed: {copy}");
            out.push_str(&format!("\n{indent}{copy}"));
            statements += 1;
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    // Four foam layers and the spray veil (`SURFACE_GODOT_WATER`: the wake's one foam layer and the veil).
    let expected = if out.contains("jka_godot_body(") { 2 } else { 5 };
    assert_eq!(statements, expected, "glint transfer: upstream `above` statements changed");
    let fog = "var color = jka_surface_legacy_fog(apply_fog(above, input.world), input.world);";
    let s = replace_once(&out, fog, &format!("{fog}\n    var jka_glint_transfer = \
        jka_surface_legacy_fog(apply_fog(jka_glint_above, input.world), input.world) - color;"));
    let s = replace_once(&s, "if !front && (submerged || (jka_hybrid.breaker.x < 0.5 && input.world.y > camera.position.y)) {",
        "if !front && (submerged || (jka_hybrid.breaker.x < 0.5 && input.world.y > camera.position.y)) {\n        jka_glint_transfer = vec3<f32>(0.0);");
    replace_once(&s, "return vec4<f32>(color, 1.0);", concat!(
        "return JkaReducedOut(vec4<f32>(color, 1.0), vec4<f32>(jka_glitter_mean, min(jka_glitter_density, 65000.0)), ",
        "vec4<f32>(jka_glint_transfer, 0.0));"))
}

/// Both full-resolution passes of mode 5 rasterize the same mesh from two shader modules; an invariant position makes
/// their coverage identical, so no pixel falls between the complementary discards.
fn jka_invariant_position(source: &str) -> String {
    replace_once(source, "@builtin(position) clip: vec4<f32>,", "@builtin(position) @invariant clip: vec4<f32>,")
}

/// The reduced-shading bindings (`@group(4)`) next to upstream's global bindings: naga-oil rejects an attributed `var`
/// between functions.
fn jka_adaptive_bindings(source: &str) -> String {
    const SKY_BINDING: &str = "@group(3) @binding(2) var<uniform> sky_env: SkyEnvironment;";
    replace_once(source, SKY_BINDING, &format!("{SKY_BINDING}\n{REDUCED_BINDINGS}"))
}

const REDUCED_BINDINGS: &str = "@group(4) @binding(0) var reduced_color: texture_2d<f32>;\n@group(4) @binding(1) var reduced_depth: texture_depth_2d;\n@group(4) @binding(2) var<storage, read> jka_adaptive_tiles: array<u32>;\n\
    @group(4) @binding(3) var reduced_glitter: texture_2d<f32>;\n@group(4) @binding(4) var reduced_transfer: texture_2d<f32>;";

/// `r_oceanPixelDetail` 5's per-tile decision, read identically by both full-resolution passes: `adaptive_tiles.wgsl`
/// classifies each 16x16-pixel tile from the 2x2 reduced shading (0 reconstructed; 1 water edge, 2 depth step, 3 dense
/// luminance detail: full material). Whole tiles keep the GPU's 32-pixel groups on one side of the decision.
const ADAPTIVE_WGSL: &str = "
fn jka_adaptive_tile(pixel: vec2<i32>) -> u32 {
    let across = (i32(textureDimensions(reduced_color).x) + 7) / 8;
    let tile = pixel / 16;
    return jka_adaptive_tiles[tile.y * across + tile.x];
}

fn jka_adaptive_full(pixel: vec2<i32>) -> bool {
    return jka_adaptive_tile(pixel) != 0u;
}

// `r_oceanPixelDebug` (a negative `jka_boundary.scale.w`): full-shaded tiles tinted by reason (yellow water edge, blue
// depth step, red luminance detail).
fn jka_adaptive_tint(color: vec3<f32>, pixel: vec2<i32>) -> vec3<f32> {
    if jka_boundary.scale.w >= 0.0 { return color; }
    let reason = jka_adaptive_tile(pixel);
    let tint = select(select(vec3<f32>(1.0, 0.1, 0.1), vec3<f32>(0.1, 0.3, 1.0), reason == 2u),
        vec3<f32>(1.0, 0.9, 0.1), reason == 1u);
    let level = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
    return mix(color, tint * max(level, 0.05) * 2.0, 0.6);
}
";

/// `r_oceanPixelDetail` 5's tile classifier (compute, after the reduced shading).
pub const ADAPTIVE_TILES_WGSL: &str = include_str!("adaptive_tiles.wgsl");

/// Pixels per edge of an adaptive tile (`adaptive_tiles.wgsl`'s workgroup covers 8x8 reduced texels).
pub const ADAPTIVE_TILE: u32 = 16;

/// Tiles (across, down) over a full-resolution target `width` x `height`, as the classifier dispatches them.
pub fn adaptive_tile_counts(width: u32, height: u32) -> (u32, u32) {
    (width.max(1).div_ceil(2).div_ceil(8), height.max(1).div_ceil(2).div_ceil(8))
}

fn jka_reconstruct_from_source(
    composer: &mut Composer,
    original: &str,
    adaptive: bool,
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
    let original = original.replace(SKY_BINDING, &format!("{SKY_BINDING}\n{REDUCED_BINDINGS}"));
    let original = if adaptive { jka_invariant_position(&original) } else { original };
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
    // This pixel's area on the water, for its own glints (`jka_reduced_glints`), in uniform control flow.
    let grid_dx = dpdx(input.grid);
    let grid_dy = dpdy(input.grid);
    let pixel_area = abs(grid_dx.x * grid_dy.y - grid_dx.y * grid_dy.x);
    if !jka_water_contains(input.grid) { discard; }
    let dim = vec2<i32>(textureDimensions(reduced_color));
    let loc = input.clip.xy / camera.viewport.xy * vec2<f32>(dim) - 0.5;
    let origin = vec2<i32>(floor(loc));
    let fraction = fract(loc);
    var result = vec3<f32>(0.0);
    var weight = 0.0;
    var nearest = vec3<f32>(0.0);
    var nearest_weight = -1.0;
    var glitter = vec4<f32>(0.0);
    var transfer = vec3<f32>(0.0);
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
            glitter += textureLoad(reduced_glitter, coord, 0) * w;
            transfer += textureLoad(reduced_transfer, coord, 0).rgb * w;
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
    // The glints at full resolution: the reduced shading left them out and kept their smooth mean, density and share.
    let mean = glitter / weight;
    let glint = luminance_ceiling(mean.rgb * glitter_glints(input.grid, pixel_area, mean.a, waves.wind.z),
        sky_env.light_intensity * GLINT_PEAK);
    return vec4<f32>(result / weight + transfer / weight * glint, 1.0);
}
"#;
    const FRAGMENT_ATTRIBUTE: &str = "@fragment\n";
    let prefix_end = start.checked_sub(FRAGMENT_ATTRIBUTE.len())
        .expect("upstream fragment attribute missing");
    assert_eq!(&original[prefix_end..start], FRAGMENT_ATTRIBUTE,
        "upstream fragment attribute changed");
    // This source file may be checked out with CRLF line ends.
    let fragment = fragment.replace("\r\n", "\n");
    let (fragment, decision) = if adaptive {
        (replace_once(&fragment, "    if !jka_water_contains(input.grid) { discard; }\n",
            "    if !jka_water_contains(input.grid) { discard; }\n    if jka_adaptive_full(vec2<i32>(input.clip.xy)) { discard; return vec4<f32>(0.0); }\n"),
            ADAPTIVE_WGSL)
    } else { (fragment, "") };
    let source = format!("{}{}{}\n{}\n{decision}", &original[..prefix_end], fragment,
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
    fn equivalent_work_order_variants_validate_with_material_modes() {
        for mask in [COST_PROBE_SINGLE_WAVE_COPY, COST_PROBE_SINGLE_WAVE_COPY | COST_PROBE_SSR,
            COST_PROBE_DEFER_BODY, COST_PROBE_SERIAL_REFINEMENT,
            COST_PROBE_DEFER_BODY | COST_PROBE_SERIAL_REFINEMENT,
            COST_PROBE_DEFER_BODY | COST_PROBE_SSR,
            COST_PROBE_DEFER_BODY | COST_PROBE_FOAM,
            COST_PROBE_DEFER_BODY | COST_PROBE_PACK_HALF] {
            for mode in [0, 1, 2, 5] {
                let mut composer = composer_with_surface_probes(mask, pixel_scale(mode)).expect("modules");
                let module = jka_surface_module_with_probe(&mut composer, 16, mode, mask)
                    .unwrap_or_else(|error| panic!("mask {mask}, mode {mode}: {}", error.emit_to_string(&composer)));
                wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(),
                    wgpu::naga::valid::Capabilities::SHADER_FLOAT16_IN_FLOAT32)
                    .validate(&module).expect("equivalent work ordering validates");
            }
        }
    }

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
    fn fast_hybrid_material_validates_with_and_without_reflections() {
        for probe in [SURFACE_FAST_HYBRID, SURFACE_FAST_HYBRID | COST_PROBE_SSR,
            SURFACE_FAST_HYBRID | FAR_SHADER_DEFAULT, SURFACE_FAST_HYBRID | COST_PROBE_FLOOR] {
            let mut composer = composer_with_surface_probes(probe, 1).expect("modules");
            let module = jka_surface_module_with_probe(&mut composer, 16, 0, probe)
                .unwrap_or_else(|error| panic!("fast Hybrid {probe:#x}: {}", error.emit_to_string(&composer)));
            wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::default()).validate(&module).expect("fast Hybrid validates");
        }
    }

    #[test]
    fn full_resolution_footprint_rewrites_only_derivatives_and_implicit_reads() {
        let source = "let a = dpdx(xz) / max(fwidth(f(b)), 1.0);\nlet c = textureSample(map, s, uv + dpdy(v).x);\n\
            let d = textureSampleGrad(map, s, uv, gx, gy);\nlet e = my_dpdx(x);";
        assert_eq!(full_resolution_footprint(source, 1), source);
        assert_eq!(full_resolution_footprint(source, 2), "let a = (0.50 * dpdx(xz)) / max((0.50 * fwidth(f(b))), 1.0);\n\
            let c = textureSampleBias(map, s, uv + (0.50 * dpdy(v)).x, -1.0);\n\
            let d = textureSampleGrad(map, s, uv, gx, gy);\nlet e = my_dpdx(x);");
        assert!(full_resolution_footprint("textureSample(t, s, uv)", 4).ends_with(", -2.0)"));
    }

    #[test]
    fn jka_reduced_pixel_modes_validate() {
        // The JKA surface differs from the preserved upstream shader:
        // validate its unchanged mode and every specialized cheap mode.
        for mode in 0..=5 {
            let mut composer = composer_with_footprint(None, pixel_scale(mode)).expect("upstream modules");
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
    fn jka_cost_probe_variants_validate() {
        let mut masks: Vec<u32> = (0..13).map(|bit| 1u32 << bit).collect();
        masks.push(COST_PROBE_ALL & !COST_PROBE_FLOOR);
        masks.push(SURFACE_SKIP_FADED_CASCADES);
        masks.push(SURFACE_SKIP_FADED_CASCADES | COST_PROBE_SSR);
        masks.extend([COST_PROBE_FOAM_SPREAD, COST_PROBE_FOAM_GROUPS, COST_PROBE_FOAM_GALE, COST_PROBE_FOAM_PATTERNS,
            COST_PROBE_BAKED_READS, COST_PROBE_PACK_HALF, COST_PROBE_PACK_HALF_WIDE,
            COST_PROBE_PACK_HALF | SURFACE_SKIP_FADED_CASCADES, COST_PROBE_PACK_BITS,
            COST_PROBE_PACK_BITS | COST_PROBE_PACK_HALF_WIDE, FAR_SHADER_FOAM, FAR_SHADER_DEFAULT,
            SURFACE_WAVE_STATISTICS, SURFACE_WAVE_STATISTICS | SURFACE_WAVE_STATISTICS_FOAM,
            SURFACE_WAVE_STATISTICS | SURFACE_WAVE_STATISTICS_FOAM | SURFACE_SKIP_FADED_CASCADES | COST_PROBE_FOAM_SPREAD,
            SURFACE_WAVE_STATISTICS | SURFACE_WAVE_STATISTICS_DEBUG]);
        // The far-water shader in every pixel-detail mode (the reduced modes compose glint outputs and footprints).
        for mode in 0..=5u8 {
            let mut composer = composer_with_footprint(None, pixel_scale(mode)).expect("upstream modules");
            let module = jka_surface_module_with_probe(&mut composer, 16, mode, FAR_SHADER_DEFAULT)
                .unwrap_or_else(|error| panic!("far mode {mode}: {}", error.emit_to_string(&composer)));
            wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::SHADER_FLOAT16_IN_FLOAT32)
                .validate(&module).unwrap_or_else(|error| panic!("far mode {mode} invalid: {error}"));
            let mut composer = composer_with_footprint(None, pixel_scale(mode)).expect("upstream modules");
            let stats = SURFACE_WAVE_STATISTICS | SURFACE_WAVE_STATISTICS_FOAM;
            let module = jka_surface_module_with_probe(&mut composer, 16, mode, stats)
                .unwrap_or_else(|error| panic!("wave statistics mode {mode}: {}", error.emit_to_string(&composer)));
            wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::SHADER_FLOAT16_IN_FLOAT32)
                .validate(&module).unwrap_or_else(|error| panic!("wave statistics mode {mode} invalid: {error}"));
        }
        {
            let mut composer = composer_with_noise(Some(())).expect("cheap noise modules");
            let module = jka_surface_module_with_probe(&mut composer, 16, 0, COST_PROBE_CHEAP_NOISE)
                .unwrap_or_else(|error| panic!("cheap noise: {}", error.emit_to_string(&composer)));
            wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(), wgpu::naga::valid::Capabilities::default())
                .validate(&module).expect("cheap noise validates");
        }
        for mask in masks {
            let mut composer = composer().expect("upstream modules");
            let module = jka_surface_module_with_probe(&mut composer, 16, 0, mask)
                .unwrap_or_else(|error| panic!("probe {mask:#x}: {}", error.emit_to_string(&composer)));
            wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::SHADER_FLOAT16_IN_FLOAT32,
            )
            .validate(&module)
            .unwrap_or_else(|error| panic!("probe {mask:#x} invalid: {error}"));
        }
    }

    /// `r_oceanType 5`: the Godot water material in every pixel-detail mode, with the far shader and the probes it
    /// tolerates, its near and adaptive passes, and the eye and waterline probes that share its waves.
    #[test]
    fn jka_godot_water_variants_validate() {
        let validate = |module: wgpu::naga::Module, what: &str| {
            wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::SHADER_FLOAT16_IN_FLOAT32)
                .validate(&module).unwrap_or_else(|error| panic!("{what} invalid: {error}"));
        };
        for mode in 0..=5u8 {
            for variant in [SURFACE_GODOT_WATER, SURFACE_GODOT_WATER | FAR_SHADER_DEFAULT,
                SURFACE_GODOT_WATER | COST_PROBE_ALL & !COST_PROBE_FLOOR] {
                let mut composer = composer_with_footprint(None, pixel_scale(mode)).expect("upstream modules");
                let module = jka_surface_module_with_probe(&mut composer, 16, mode, variant)
                    .unwrap_or_else(|error| panic!("godot mode {mode} {variant:#x}: {}", error.emit_to_string(&composer)));
                validate(module, &format!("godot mode {mode} {variant:#x}"));
            }
        }
        let mut modules = composer().expect("upstream modules");
        let near = jka_near_surface_module(&mut modules, 16, SURFACE_GODOT_WATER)
            .unwrap_or_else(|error| panic!("godot near: {}", error.emit_to_string(&modules)));
        validate(near, "godot near");
        let mut modules = composer().expect("upstream modules");
        let adaptive = jka_adaptive_surface_module(&mut modules, 16, SURFACE_GODOT_WATER)
            .unwrap_or_else(|error| panic!("godot adaptive: {}", error.emit_to_string(&modules)));
        validate(adaptive, "godot adaptive");
        let mut modules = composer().expect("upstream modules");
        let eye = eye_height_module(&mut modules)
            .unwrap_or_else(|error| panic!("eye height: {}", error.emit_to_string(&modules)));
        validate(eye, "eye height");
        let mut modules = composer().expect("upstream modules");
        let marker = jka_waterline_marker_module(&mut modules)
            .unwrap_or_else(|error| panic!("waterline marker: {}", error.emit_to_string(&modules)));
        validate(marker, "waterline marker");
        let mut modules = composer().expect("upstream modules");
        let wireframe = jka_wireframe_module(&mut modules)
            .unwrap_or_else(|error| panic!("wireframe: {}", error.emit_to_string(&modules)));
        validate(wireframe, "wireframe");
    }

    #[test]
    fn jka_reconstruction_and_near_shaders_validate() {
        for near in [false, true] {
            let mut composer = composer().expect("upstream modules");
            let module = if near {
                jka_near_surface_module(&mut composer, 16, 0)
            } else {
                jka_reconstruct_module(&mut composer)
            }.unwrap_or_else(|error| panic!("{}", error.emit_to_string(&composer)));
            wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::default(),
            ).validate(&module).expect("Blue Water reduced composition validates");
        }
    }

    /// `r_oceanPixelDetail` 5's two full-resolution passes: each takes the decision, discards the other's blocks and
    /// keeps an invariant position.
    #[test]
    fn jka_adaptive_pixel_shaders_validate() {
        for full in [false, true] {
            let mut composer = composer().expect("upstream modules");
            let module = if full {
                jka_adaptive_surface_module(&mut composer, 16, 0)
            } else {
                jka_adaptive_reconstruct_module(&mut composer)
            }.unwrap_or_else(|error| panic!("full {full}: {}", error.emit_to_string(&composer)));
            wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::default(),
            ).validate(&module).unwrap_or_else(|error| panic!("full {full} invalid: {error}"));
            let entry = module.entry_points.iter().find(|entry| entry.name == "vertex").expect("vertex entry");
            let invariant = module.types.iter().any(|(_, ty)| matches!(&ty.inner,
                wgpu::naga::TypeInner::Struct { members, .. } if members.iter().any(|member| matches!(member.binding,
                    Some(wgpu::naga::Binding::BuiltIn(wgpu::naga::BuiltIn::Position { invariant: true }))))));
            assert!(invariant, "full {full}: {} lacks an invariant position", entry.name);
        }
        let classifier = wgpu::naga::front::wgsl::parse_str(ADAPTIVE_TILES_WGSL).expect("adaptive tiles parse");
        wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(), wgpu::naga::valid::Capabilities::default())
            .validate(&classifier).expect("adaptive tiles validate");
        assert_eq!(adaptive_tile_counts(2560, 1440), (160, 90));
        assert_eq!(adaptive_tile_counts(2561, 1441), (161, 91));
        assert!(ADAPTIVE_TILES_WGSL.contains("@workgroup_size(8, 8)") && ADAPTIVE_TILE == 16);
    }

    #[test]
    fn jka_reconstruction_accepts_lf_and_crlf_sources() {
        let lf = include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/surface.wgsl")
            .replace("\r\n", "\n");
        for source in [lf.clone(), lf.replace('\n', "\r\n")] {
            let mut composer = composer().expect("upstream modules");
            let module = jka_reconstruct_from_source(&mut composer, &source, false)
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
