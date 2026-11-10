//! Ocean cvars: declarative runtime and config bindings.
use super::cvar_prelude::*;

use crate::console::cvars::CvarScalar;

video_scalar_cvars! { SCALAR_DEFINITIONS, commit_ocean_settings;
    APP_R_BLUEWATERCOASTSHELTER {
        name: "r_blueWaterCoastShelter", default: "0",
        range: "0|1", description: "Upstream coast: 1 includes land/wind-fetch shelter; 0 keeps depth breaking and moving surf only. Shelter can remove long waves under weak wind. Use bluewater_coast_debug to inspect local attenuation.",
        kind: CvarType::Bool, field: ocean_settings.blue_water_coast_shelter, archive: 27345
    }
    APP_R_BLUEWATERCOAST {
        name: "r_blueWaterCoast", default: "0",
        range: "0|1", description: "Original Blue Water coastal waves/surf on native or Hybrid: depth breaking, land fetch shelter and moving surf bands. Separate from the r_coastal shallow-bank solver; 0 restores open-ocean waves.",
        kind: CvarType::Bool, field: ocean_settings.blue_water_coast, archive: 27344
    }
    APP_R_LAKESOURCEHINTS {
        name: "r_lakeSourceHints", default: "1",
        range: "0|1", description: "Use each promoted JKA water shader's source hue, alpha-blend clarity, authored deformation, roughness and tcMod scroll heading in Lake mode. 0 restores the global Lake material.",
        kind: CvarType::Bool, field: ocean_settings.lake.source_hints, archive: 27377
    }
    APP_R_LAKEWINDSPEED {
        name: "r_lakeWindSpeed", default: "0",
        range: "0..40", description: "Lake ripple wind speed in metres per second; independent of ocean spectrum.",
        kind: CvarType::Float { min: 0.0, max: 40.0 }, field: ocean_settings.lake.wind_speed, archive: 27340
    }
    APP_R_LAKEDIRECTION {
        name: "r_lakeDirection", default: "52",
        range: "-180..180", description: "Lake ripple direction in degrees from +X toward world +Z.",
        kind: CvarType::Float { min: -180.0, max: 180.0 }, field: ocean_settings.lake.direction, archive: 27341
    }
    APP_R_LAKETRANSPARENCY {
        name: "r_lakeTransparency", default: "0.5",
        range: "0.1..10", description: "Lake optical clarity multiplier; higher values reveal more of the bed.",
        kind: CvarType::Float { min: 0.1, max: 10.0 }, field: ocean_settings.lake.transparency, archive: 27342
    }
    APP_R_LAKEREFLECTIONS {
        name: "r_lakeReflections", default: "0.525",
        range: "0..1", description: "Lake world/sky reflection and sun glitter strength; 0 removes reflections.",
        kind: CvarType::Float { min: 0.0, max: 1.0 }, field: ocean_settings.lake.reflections, archive: 27363
    }
    APP_R_LAKEAMPLITUDE {
        name: "r_lakeAmplitude", default: "6.65",
        range: "0..12", description: "Lake geometric amplitude in inches above/below the map plane; 0 is flat.",
        kind: CvarType::Float { min: 0.0, max: 12.0 }, field: ocean_settings.lake.amplitude, archive: 27360
    }
    APP_R_LAKESPEED {
        name: "r_lakeSpeed", default: "1.45",
        range: "0..3", description: "Lake deformation animation speed; 0 freezes it, 1 is normal.",
        kind: CvarType::Float { min: 0.0, max: 3.0 }, field: ocean_settings.lake.speed, archive: 27364
    }
    APP_R_LAKETRANSMISSION {
        name: "r_lakeTransmission", default: "1",
        range: "0..1.0", description: "Cheapwater transmission: 0 preserves opaque shading, 1 reveals the bed with depth absorption.",
        kind: CvarType::Float { min: 0.0, max: 1.0 }, field: ocean_settings.lake.cheap_transmission, archive: 27369
    }
    APP_R_LAKEREFRACTION {
        name: "r_lakeRefraction", default: "0.026",
        range: "0..0.05", description: "Cheapwater screen-space refraction strength; reuses its moving normal layers. Zero disables distortion.",
        kind: CvarType::Float { min: 0.0, max: 0.05 }, field: ocean_settings.lake.cheap_refraction, archive: 27370
    }
    APP_R_LAKEDEPTHCOLOR {
        name: "r_lakeDepthColor", default: "0.52",
        range: "0.0..1.0", description: "Cheapwater shallow/deep color blend. Zero preserves the base material.",
        kind: CvarType::Float { min: 0.0, max: 1.0 }, field: ocean_settings.lake.depth_color_strength, archive: 27371
    }
    APP_R_LAKECOLORDEPTH {
        name: "r_lakeColorDepth", default: "52",
        range: "0.1..100.0", description: "Vertical bed depth in metres at which Cheapwater reaches its deep color.",
        kind: CvarType::Float { min: 0.1, max: 100.0 }, field: ocean_settings.lake.color_depth, archive: 27372
    }
    APP_R_LAKEOPAQUEDEPTH {
        name: "r_lakeOpaqueDepth", default: "8",
        range: "0.25..100 feet", description: "Cheapwater vertical bed depth in feet where nonlinear transparency reaches zero. 3-inch water remains almost fully clear at the default 8 feet.",
        kind: CvarType::Float { min: 0.25, max: 100.0 }, field: ocean_settings.lake.opaque_depth, archive: 27375
    }
    APP_R_LAKESHADER {
        name: "r_lakeShader", default: "1",
        range: "0|1", description: "Lake material A/B: 0 original optical water, 1 Cheapwater material with neutral water reflections. Geometry and reflection selection are shared.",
        kind: CvarType::Int { min: 0, max: 1 }, field: ocean_settings.lake.shader, archive: 27367
    }
    APP_R_LAKEREFLECTIONMODE {
        name: "r_lakeReflectionMode", default: "2",
        range: "0..2", description: "Lake-local reflections: 0 Sky, 1 SSR, 2 Planar. Independent of global reflection mode and quality.",
        kind: CvarType::Int { min: 0, max: 2 }, field: ocean_settings.lake.reflection_mode, archive: 27368
    }
    APP_R_HYBRIDGERSTNERAMPLITUDE {
        name: "r_hybridGerstnerAmplitude", default: "128",
        range: "0.0..16384.0", description: "Hybrid-only directional Gerstner amplitude; independent of Godot and Blue Water FFT.",
        kind: CvarType::Float { min: 0.0, max: 16384.0 }, field: ocean_settings.hybrid_swell.amplitude, archive: 25180
    }
    APP_R_HYBRIDGERSTNERWAVELENGTH {
        name: "r_hybridGerstnerWavelength", default: "1400",
        range: "64.0..32768.0", description: "Hybrid-only directional Gerstner wavelength; independent of Godot and Blue Water FFT.",
        kind: CvarType::Float { min: 64.0, max: 32768.0 }, field: ocean_settings.hybrid_swell.wavelength, archive: 25181
    }
    APP_R_HYBRIDGERSTNERSPEED {
        name: "r_hybridGerstnerSpeed", default: "180",
        range: "-4096.0..4096.0", description: "Hybrid-only directional Gerstner speed; independent of Godot and Blue Water FFT.",
        kind: CvarType::Float { min: -4096.0, max: 4096.0 }, field: ocean_settings.hybrid_swell.speed, archive: 25183
    }
    APP_R_HYBRIDGERSTNERSTEEPNESS {
        name: "r_hybridGerstnerSteepness", default: "0.65",
        range: "0.0..1.0", description: "Hybrid-only directional Gerstner steepness; independent of Godot and Blue Water FFT.",
        kind: CvarType::Float { min: 0.0, max: 1.0 }, field: ocean_settings.hybrid_swell.steepness, archive: 25184
    }
    APP_R_HYBRIDFFTBLEND {
        name: "r_hybridFftBlend", default: "1",
        range: "0..1", description: "Hybrid ocean: 1 pure Blue Water FFT; 0 authored Gerstner; intermediate values mix FFT and analytical displacement. Native and Godot unaffected.",
        kind: CvarType::Float { min: 0.0, max: 1.0 }, field: ocean_settings.hybrid_fft_blend, archive: 25189
    }
    APP_R_OCEANLONGWAVES {
        name: "r_oceanLongWaves", default: "0",
        range: "0..4096", description: "Blue Water / Hybrid: rebuild the FFT's long waves (its largest tile) from this many stronger waves with the same total height, wavelength and direction spread, instead of thousands of weak ones. 0 = original spectrum. Shorter waves are unchanged. Look test for a wave count the server could sum.",
        kind: CvarType::Int { min: 0, max: 4096 }, field: ocean_settings.long_waves, archive: 27390
    }
    APP_R_OCEANMEDIUMWAVES {
        name: "r_oceanMediumWaves", default: "0",
        range: "0..4096", description: "Blue Water / Hybrid: rebuild the FFT's medium waves (its second tile) from this many stronger waves with the same total height and spread. 0 = original spectrum. Pair with r_oceanLongWaves.",
        kind: CvarType::Int { min: 0, max: 4096 }, field: ocean_settings.medium_waves, archive: 27391
    }
    APP_R_OCEANSMALLWAVEDETAIL {
        name: "r_oceanSmallWaveDetail", default: "1",
        range: "0..16", description: "Blue Water / Hybrid: height multiplier for the FFT's fine chop (its smallest tile). Raise it for more small surface variation. Visual only: physics never sums these waves. 1 = Blue Water's own spectrum.",
        kind: CvarType::Float { min: 0.0, max: 16.0 }, field: ocean_settings.small_wave_detail, archive: 27392
    }
    APP_R_OCEANMEDIUMWAVEDETAIL {
        name: "r_oceanMediumWaveDetail", default: "1",
        range: "0..16", description: "Blue Water / Hybrid: height multiplier for the FFT's medium waves (its second tile, a few metres to ~20 m). Raise it for more mid-size surface variation. 1 = Blue Water's own spectrum.",
        kind: CvarType::Float { min: 0.0, max: 16.0 }, field: ocean_settings.medium_wave_detail, archive: 27393
    }
    APP_R_HYBRIDBREAKER {
        name: "r_hybridBreaker", default: "0",
        range: "0..2", description: "Hybrid crest A/B: 0 original; 1 conservative horizontal compression limit; 2 bounded crest folds, signed normals and spilling foam. Preserves vertical wave height. Applies live.",
        kind: CvarType::Int { min: 0, max: 2 }, field: ocean_settings.hybrid_breaker, archive: 25221
    }
    APP_R_BLUEWATERMESHLEVELS {
        name: "r_blueWaterMeshLevels", default: "6",
        range: "6..9", description: "Blue Water clipmap level count; 9 is native reference. Rebuilds geometry.",
        kind: CvarType::Int { min: 6, max: 9 }, field: ocean_settings.blue_water_mesh_levels, archive: 25191
    }
    APP_R_BLUEWATERSSRSTEPS {
        name: "r_blueWaterSSRSteps", default: "16",
        range: "4..32", description: "Native Blue Water reflection ray steps (compile-time specialization; rebuilds only its surface pipeline).",
        kind: CvarType::Int { min: 4, max: 32 }, field: ocean_settings.blue_water_ssr_steps, archive: 25201
    }
    APP_R_OCEANWAKE {
        name: "r_oceanWake", default: "0",
        range: "0..1", description: "Native / Hybrid ocean: 0 JKA's own water interaction only; 1 Blue Water's ship wake (dispersive wake field, trail foam, bow waves, hull foam, impact rings) for A/B. Drive it with the oceanwake command.",
        kind: CvarType::Int { min: 0, max: 1 }, field: ocean_settings.blue_wake, archive: 27330
    }
    APP_R_OCEANSPINDRIFT {
        name: "r_oceanSpindrift", default: "0",
        range: "0..1", description: "Native / Hybrid ocean: Blue Water's spindrift, wisps of spray torn off breaking crests from a gale's wind (13 m/s, set the wind in Water settings). 0 off.",
        kind: CvarType::Int { min: 0, max: 1 }, field: ocean_settings.spindrift, archive: 27331
    }
    APP_R_HYBRIDREFRACTIONMODE {
        name: "r_hybridRefractionMode", default: "1",
        range: "0..2", description: "Hybrid only: 0 off, 1 cheap screen offset, 2 ray trace.",
        kind: CvarType::Int { min: 0, max: 2 }, field: ocean_settings.hybrid_refraction_mode, archive: 25202
    }
    APP_R_HYBRIDCAUSTICSMODE {
        name: "r_hybridCausticsMode", default: "3",
        range: "0..4", description: "Hybrid: 0 native, 1 native FFT only, 2 off, 3 Godot wave curvature, 4 animated procedural.",
        kind: CvarType::Int { min: 0, max: 4 }, field: ocean_settings.hybrid_caustics_mode, archive: 25203
    }
    APP_R_HYBRIDUNDERWATERMODE {
        name: "r_hybridUnderwaterMode", default: "4",
        range: "0..4", description: "Hybrid only: 4 Godot surface+optics when submerged, 0/1/3 native, 2 composite off.",
        kind: CvarType::Int { min: 0, max: 4 }, field: ocean_settings.hybrid_underwater_mode, archive: 25204
    }
    APP_R_HYBRIDMATERIAL {
        name: "r_hybridMaterial", default: "1",
        range: "0..3", description: "Hybrid ocean quality: 0 original Bluewater material; 1 compact full-resolution GGX/foam (default, honors r_hybridSSR); 2 compact with SSR disabled; 3 experimental adaptive compact: restores resolved near ripples, skips unresolvable FFT cascade reads and draws inexpensive stable world-space sun glints. Material 3 honors r_hybridSSR. All fast modes shade at native pixel resolution, independent of r_oceanPixelDetail; native FFT geometry, sky admission, and underwater behavior are preserved.",
        kind: CvarType::Int { min: 0, max: 3 }, field: ocean_settings.hybrid_material, archive: 25206
    }
    APP_R_HYBRIDSSR {
        name: "r_hybridSSR", default: "0",
        range: "0|1", description: "Hybrid only: native screen-space reflection tracing (sky/env reflection remains).",
        kind: CvarType::Bool, field: ocean_settings.hybrid_ssr, archive: 25205
    }
    APP_R_HYBRIDCAUSTICSFAST {
        name: "r_hybridCausticsFast", default: "0",
        range: "0|1", description: "Hybrid A/B: 3-tap forward-difference caustics versus reference 4-tap central differences.",
        kind: CvarType::Bool, field: ocean_settings.hybrid_caustics_fast, archive: 25215
    }
    APP_R_BLUEWATERBOUNDARY {
        name: "r_blueWaterBoundaryMode", default: "3",
        range: "0..3", description: "0 exact JKA footprint+PVS; 1 bounds/PVS only; 2 no boundary/PVS test (diagnostic); 3 exact with per-tile candidate masks.",
        kind: CvarType::Int { min: 0, max: 3 }, field: ocean_settings.blue_water_boundary_mode, archive: 25192
    }
    APP_R_BLUEWATERCAUSTICMODE {
        name: "r_blueWaterCausticsMode", default: "0",
        range: "0..3", description: "Native: 0 compute every frame, 1 on FFT updates, 2 disabled. Legacy 3 (Godot) now belongs to r_hybridCausticsMode.",
        kind: CvarType::Int { min: 0, max: 3 }, field: ocean_settings.blue_water_caustics_mode, archive: 25193
    }
    APP_R_BLUEWATERUNDERWATERMODE {
        name: "r_blueWaterUnderwaterMode", default: "0",
        range: "0..4", description: "Native: 0 full, 1 no motes, 2 no composite, 3 submerged-only. Legacy 4 now belongs to r_hybridUnderwaterMode.",
        kind: CvarType::Int { min: 0, max: 4 }, field: ocean_settings.blue_water_underwater_mode, archive: 25197
    }
    APP_R_BLUEWATERPREPAREGODOTMESH {
        name: "r_blueWaterPrepareGodotMesh", default: "0",
        range: "0|1", description: "A/B: update Godot ocean clipmap indices above water in Blue Water mode. Hybrid mode always prepares them for submersion.",
        kind: CvarType::Bool, field: ocean_settings.blue_water_prepare_godot_mesh, archive: 25194
    }
    APP_R_BLUEWATERADAPTER {
        name: "r_blueWaterAdapter", default: "1",
        range: "0|1", description: "A/B: generate legacy displacement/normal textures for JKA water consumers. 0 freezes these textures, Blue Water surface unaffected.",
        kind: CvarType::Bool, field: ocean_settings.blue_water_adapter, archive: 25195
    }
    APP_R_BLUEWATERSSR {
        name: "r_blueWaterSSR", default: "1",
        range: "0|1", description: "A/B: native screen-space reflection rays only; sky/environment reflection remains.",
        kind: CvarType::Bool, field: ocean_settings.blue_water_ssr, archive: 25196
    }
    APP_R_BLUEWATERREFRACTIONMODE {
        name: "r_blueWaterRefractionMode", default: "2",
        range: "0..2", description: "A/B: 0 off; 1 inexpensive Godot-style screen offset; 2 physical scene ray trace.",
        kind: CvarType::Int { min: 0, max: 2 }, field: ocean_settings.blue_water_refraction_mode, archive: 25199
    }
    APP_R_GERSTNERWAVESCALE {
        name: "r_gerstnerWaveScale", default: "1",
        range: "0.1..20", description: "Gerstner ocean (r_oceanType 5): godot-water-shader's sea this many times larger. Lengths scale by it and time by its square root, so a larger sea keeps the same shape and slows as real swell does. 1 is the original (0.4 m, 6 m waves). Applies live.",
        kind: CvarType::Float { min: 0.1, max: 20.0 }, field: ocean_settings.gerstner_wave_scale, archive: 25231
    }
    APP_R_GERSTNERFOAM {
        name: "r_gerstnerFoam", default: "7",
        range: "0..7", description: "Gerstner ocean (r_oceanType 5) foam bits: 1 near persistent cascade (15 m), 2 far persistent cascade (30 m), 4 distant compression foam past them (per pixel, no persistence). 3 is the original; 7 all. Distances scale with r_gerstnerWaveScale. Applies live.",
        kind: CvarType::Int { min: 0, max: 7 }, field: ocean_settings.gerstner_foam, archive: 25232
    }
    APP_R_GERSTNERFOAMRATE {
        name: "r_gerstnerFoamRate", default: "60",
        range: "0..60", description: "Gerstner ocean (r_oceanType 5): persistent foam simulation steps per second for each cascade, staggered so the near and far cascades rarely step in the same frame. 0 steps every rendered frame (the original). Applies live.",
        kind: CvarType::Float { min: 0.0, max: 60.0 }, field: ocean_settings.gerstner_foam_rate, archive: 25233
    }
    APP_R_OCEANSCREENLODPIXELS {
        name: "r_oceanScreenLodPixels", default: "0",
        range: "0..16", description: "Native/Hybrid screen-space ring LOD. Protects closest 3 rings with adaptive mesh Off; with curvature-adaptive On, coarsens selected rings as requested instead of being overridden. Boundary vertices remain stitched.",
        kind: CvarType::Float { min: 0.0, max: 16.0 }, field: ocean_settings.screen_lod_pixels, archive: 25227
    }
    APP_R_OCEANPIXELDETAIL {
        name: "r_oceanPixelDetail", default: "0",
        range: "0..5", description: "Native Blue Water: 0 full resolution; 1 true 1/2 resolution (2x2); 2 true 1/4 resolution (4x4); 3 distance-aware 2x2; 4 distance-aware 4x4; 5 adaptive 2x2 (full shading only on 2x2 blocks the 2x2 shading cannot reconstruct, see r_oceanPixelContrast). Full-res wave geometry/depth remains.",
        kind: CvarType::Int { min: 0, max: 5 }, field: ocean_settings.pixel_detail_mode, archive: 25228
    }
    APP_R_OCEANPIXELDISTANCE {
        name: "r_oceanPixelDistance", default: "100",
        range: "1..2000", description: "Native/Hybrid metres from camera where full-resolution near-water shading transitions to true reduced-resolution material.",
        kind: CvarType::Float { min: 1.0, max: 2000.0 }, field: ocean_settings.pixel_detail_distance, archive: 25229
    }
    APP_R_OCEANPIXELCONTRAST {
        name: "r_oceanPixelContrast", default: "0.1",
        range: "0.001..0.5", description: "Native/Hybrid r_oceanPixelDetail 5: how far the 2x2-shaded water's luminance may curve across a 2x2 block (share of its level); a 16x16 tile with 8+ such blocks gets full shading. Lower = more full-shaded tiles (closer to Full, slower). Costa: 0.1 = +20% FPS, 0.05 = break-even.",
        kind: CvarType::Float { min: 0.001, max: 0.5 }, field: ocean_settings.pixel_detail_contrast, archive: 25230
    }
    APP_R_OCEANVISUALFIXES {
        name: "r_oceanVisualFixes", default: "0",
        range: "0..2", description: "0 original; 1 balanced whitecaps + lit foam + promoted-water body; 2 stronger Godot-like look. No FFT reset.",
        kind: CvarType::Int { min: 0, max: 2 }, field: ocean_settings.visual_fixes, archive: 25018
    }
    APP_R_OCEANFOAMSOURCE {
        name: "r_oceanFoamSource", default: "0",
        range: "0..2", description: "FFT Jacobian foam generation: 0 original, 1 slightly earlier crests and 1.3x growth, 2 stronger earlier crests and 1.6x growth. Bundle selects minimum level.",
        kind: CvarType::Int { min: 0, max: 2 }, field: ocean_settings.foam_source_mode, archive: 25019
    }
    APP_R_OCEANLINKS {
        name: "r_oceanLinks", default: "127",
        range: "0..127", description: "Bitmask of ocean controls that follow the swell amplitude: 1 wavelength, 2 speed, 4 choppiness, 8 wind chop, 16 foam, 32 spray, 64 roughness. 127 all linked (default), 0 none.",
        kind: CvarType::Int { min: 0, max: 127 }, field: ocean_settings.links, archive: 25020
    }
    APP_R_OCEANFOAMDETAIL {
        name: "r_oceanFoamDetail", default: "1",
        range: "0|1", description: "0 keep whitecaps when normals LOD out, 1 (default) sample all cascades at any distance like Godot and keep far normals.",
        kind: CvarType::Int { min: 0, max: 1 }, field: ocean_settings.foam_detail_mode, archive: 25010
    }
    APP_R_OCEANFOAMACTIVITY {
        name: "r_oceanFoamActivity", default: "0",
        range: "0..3", description: "Promoted water foam attenuation: 0 original, 1 omit texture heuristic, 2 omit amplitude gate, 3 omit both. No FFT reset.",
        kind: CvarType::Int { min: 0, max: 3 }, field: ocean_settings.foam_activity_mode, archive: 25011
    }
    APP_R_OCEANCOLORMODE {
        name: "r_oceanColorMode", default: "0",
        range: "0..3", description: "Color A/B: 0 original, 1 override authored base with configured ocean color, 2 opaque lit water, 3 both.",
        kind: CvarType::Int { min: 0, max: 3 }, field: ocean_settings.color_mode, archive: 25014
    }
    APP_R_OCEANFOAMDEBUG {
        name: "r_oceanFoamDebug", default: "0",
        range: "0..3", description: "Foam diagnostic: 0 shaded water, 1 raw sampled FFT foam, 2 after promoted gates, 3 final breaker + windrows.",
        kind: CvarType::Int { min: 0, max: 3 }, field: ocean_settings.foam_debug, archive: 25017
    }
    APP_R_OCEANDYNAMICUPDATES {
        name: "r_oceanDynamicUpdates", default: "0",
        range: "0|1", description: "0 original; 1 near two ocean mesh LODs sample the live 48 Hz FFT, third LOD uses a 32 Hz cached displacement, outer LODs 24 Hz; wind foam appearance follows the same ring rates. FFT cascades remain synchronized.",
        kind: CvarType::Bool, field: ocean_settings.dynamic_update_rates, archive: 25030
    }
    APP_R_OCEANWINDFOAMSYNC {
        name: "r_oceanWindFoamSync", default: "1",
        range: "0|1", description: "1 update wind-foam transport only when the finest FFT cascade publishes its new normal/foam field; 0 legacy every-render-frame update.",
        kind: CvarType::Bool, field: ocean_settings.wind_foam_sync, archive: 25031
    }
}

app_cvars! {
    APP_R_LAKEWATERCOLOR {
        runtime: CvarDef::new("r_lakeWaterColor", "0.41818184 0.9619174 1", "r g b (0..1)",
            "Lake optical color for absorption and scattering; independent of surface tint and fog.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c = app.video.ocean_settings.lake.water_color;
                CvarValue::String(format!("{} {} {}", c[0], c[1], c[2])) },
            |app, value| App::cvar_set_lake_water_color(app, APP_R_LAKEWATERCOLOR.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_lake_water_color, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(c) = parse_vec3(value) {
                    if c.iter().all(|v| v.is_finite()) { settings.ocean_settings.lake.water_color = c.map(|v| v.clamp(0.0, 1.0)); }
                }
            }),
            archive: Some((27361, |state| { let c = state.video.ocean_settings.lake.water_color;
                format!("{} {} {}", c[0], c[1], c[2]) })),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_lake_water_color(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.lake.water_color = Self::parse_cvar_floats(name, value)?.map(|v| v.clamp(0.0, 1.0));
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_LAKESHALLOWCOLOR {
        runtime: CvarDef::new("r_lakeShallowColor", "0 0.043898553 0.025940072", "r g b (0..1)", "Cheapwater shallow color for depth coloring.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c=app.video.ocean_settings.lake.shallow_color; CvarValue::String(format!("{} {} {}",c[0],c[1],c[2])) },
            |app,value| App::cvar_set_lake_shallow_color(app, APP_R_LAKESHALLOWCOLOR.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_lake_shallow_color, None),
        config: CvarBinding {
            load_video: Some(|settings,_name,value,_context| { if let Some(c)=parse_vec3(value) { if c.iter().all(|v| v.is_finite()) { settings.ocean_settings.lake.shallow_color=c.map(|v| v.clamp(0.0,1.0)); } } }),
            archive: Some((27373, |state| { let c=state.video.ocean_settings.lake.shallow_color; format!("{} {} {}",c[0],c[1],c[2]) })),
            ..CvarBinding::NONE
        }, implementations: [impl App {
            fn cvar_set_lake_shallow_color(&mut self,name:&str,value:&str)->Result<(),String> {
                self.video.ocean_settings.lake.shallow_color=Self::parse_cvar_floats(name,value)?.map(|v| v.clamp(0.0,1.0));
                self.commit_ocean_settings();Ok(())
            }
        }]
    }
    APP_R_LAKEDEEPCOLOR {
        runtime: CvarDef::new("r_lakeDeepColor", "0 0.08160421 0.061574105", "r g b (0..1)", "Cheapwater deep color for depth coloring.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c=app.video.ocean_settings.lake.deep_color; CvarValue::String(format!("{} {} {}",c[0],c[1],c[2])) },
            |app,value| App::cvar_set_lake_deep_color(app, APP_R_LAKEDEEPCOLOR.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_lake_deep_color, None),
        config: CvarBinding {
            load_video: Some(|settings,_name,value,_context| { if let Some(c)=parse_vec3(value) { if c.iter().all(|v| v.is_finite()) { settings.ocean_settings.lake.deep_color=c.map(|v| v.clamp(0.0,1.0)); } } }),
            archive: Some((27374, |state| { let c=state.video.ocean_settings.lake.deep_color; format!("{} {} {}",c[0],c[1],c[2]) })),
            ..CvarBinding::NONE
        }, implementations: [impl App {
            fn cvar_set_lake_deep_color(&mut self,name:&str,value:&str)->Result<(),String> {
                self.video.ocean_settings.lake.deep_color=Self::parse_cvar_floats(name,value)?.map(|v| v.clamp(0.0,1.0));
                self.commit_ocean_settings();Ok(())
            }
        }]
    }
    APP_R_LAKEWAKE {
        // Compatibility console alias only; never load a second independent
        // wake strength from an old video.cfg or archive it a second time.
        runtime: CvarDef::new("r_lakeWake", "1", "0..10",
            "Legacy alias of r_oceanInteractionStrength. Player deformation strength is now global across ocean and lake renderers.",
            CvarType::Float { min: 0.0, max: 10.0 }, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.interaction_strength.clamp(0.0, 10.0)),
            |app,value| { let CvarValue::Float(v)=value else {unreachable!()};
                app.video.ocean_settings.interaction_strength=v;app.commit_ocean_settings();Ok(()) }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_LAKEPLANAR {
        runtime: CvarDef::new("r_lakePlanar", "1", "0|1", "Compatibility alias: 1 selects lake Planar, 0 selects Sky. Use r_lakeReflectionMode for Sky/SSR/Planar comparisons.",
            CvarType::Bool, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.video.ocean_settings.lake.reflection_mode == 2),
            |app,value| { let CvarValue::Bool(v)=value else {unreachable!()};
                app.video.ocean_settings.lake.reflection_mode=if v {2} else {0};app.commit_ocean_settings();Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings,_name,value,_context| { settings.ocean_settings.lake.reflection_mode=if value=="1" {2} else {0}; }),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_LAKETINT {
        runtime: CvarDef::new("r_lakeTint", "0.09994107 0.09994107 0.09994107", "r g b (0..1)",
            "Lake surface tint, independent of optical color and fog; white is neutral.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c = app.video.ocean_settings.lake.tint_color;
                CvarValue::String(format!("{} {} {}", c[0], c[1], c[2])) },
            |app, value| App::cvar_set_lake_tint(app, APP_R_LAKETINT.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_lake_tint, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(c) = parse_vec3(value) {
                    if c.iter().all(|v| v.is_finite()) { settings.ocean_settings.lake.tint_color = c.map(|v| v.clamp(0.0, 1.0)); }
                }
            }),
            archive: Some((27365, |state| { let c = state.video.ocean_settings.lake.tint_color;
                format!("{} {} {}", c[0], c[1], c[2]) })),
            ..CvarBinding::NONE
        }, implementations: [impl App {
            fn cvar_set_lake_tint(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.lake.tint_color = Self::parse_cvar_floats(name, value)?.map(|v| v.clamp(0.0, 1.0));
                self.commit_ocean_settings(); Ok(())
            }
        }]
    }
    APP_R_LAKEFOGCOLOR {
        runtime: CvarDef::new("r_lakeFogColor", "0 0.00003021201 0.01662098", "r g b (0..1)",
            "Lake underwater fog color, independent of the surface color.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c = app.video.ocean_settings.lake.fog_color;
                CvarValue::String(format!("{} {} {}", c[0], c[1], c[2])) },
            |app, value| App::cvar_set_lake_fog_color(app, APP_R_LAKEFOGCOLOR.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_lake_fog_color, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(c) = parse_vec3(value) {
                    if c.iter().all(|v| v.is_finite()) { settings.ocean_settings.lake.fog_color = c.map(|v| v.clamp(0.0, 1.0)); }
                }
            }),
            archive: Some((27362, |state| { let c = state.video.ocean_settings.lake.fog_color;
                format!("{} {} {}", c[0], c[1], c[2]) })),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_lake_fog_color(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.lake.fog_color = Self::parse_cvar_floats(name, value)?.map(|v| v.clamp(0.0, 1.0));
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_BLUEWATERMESHSEGMENTS {
        runtime: CvarDef::new("r_blueWaterMeshSegments", "64", "64|128|256",
            "Blue Water native clipmap segments per level: 64/128/256. Independent of r_oceanMeshQuality.", CvarType::Int { min: 64, max: 256 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.ocean_settings.blue_water_mesh_segments as i32),
            |app, value| {
                let CvarValue::Int(value) = value else { unreachable!() };
                if !matches!(value, 64 | 128 | 256) { return Err("r_blueWaterMeshSegments: use 64, 128, or 256".into()); }
                app.video.ocean_settings.blue_water_mesh_segments = value as u32;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<u32>() {
                    if matches!(v, 64 | 128 | 256) { settings.ocean_settings.blue_water_mesh_segments = v; }
                }
            }),
            archive: Some((25190, |state| state.video.ocean_settings.blue_water_mesh_segments.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_HYBRIDWATERLINEDEBUG {
        runtime: CvarDef::new("r_hybridWaterlineDebug", "0", "0..3",
            "Hybrid mode 4 debug: 0 off; 1 CPU/optics log; 2 color Godot mesh; 3 CAMERA-XZ GPU height gauge drawn 100u forward: yellow displaced wave, orange raw wave, magenta native eye probe, cyan flat, green/red eye tick. No GPU readback.",
            CvarType::Int { min: 0, max: 3 }, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Int(i32::from(app.video.ocean_settings.hybrid_waterline_debug)),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!() };
                app.video.ocean_settings.hybrid_waterline_debug = v as u8;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_HYBRIDFASTFOAM {
        runtime: CvarDef::new("r_hybridFastFoam", "1", "0..3",
            "Hybrid compact material only (r_hybridMaterial 1/2), session A/B: foam opacity gain. 0 no foam, 1 reference, up to 3 stronger. Applies live without recompiling or changing the FFT.",
            CvarType::Float { min: 0.0, max: 3.0 }, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.hybrid_fast_foam),
            |app, value| {
                let CvarValue::Float(v) = value else { unreachable!() };
                app.video.ocean_settings.hybrid_fast_foam = v;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_HYBRIDFASTFOAMDETAIL {
        runtime: CvarDef::new("r_hybridFastFoamDetail", "1", "0|1",
            "Hybrid compact material only, session A/B: 1 keep the native foam pattern layout; 0 one-pattern approximation (more visibly different). Applies live without recompiling. Does not change wave geometry or simulation.",
            CvarType::Bool, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.video.ocean_settings.hybrid_fast_foam_detail),
            |app, value| {
                let CvarValue::Bool(v) = value else { unreachable!() };
                app.video.ocean_settings.hybrid_fast_foam_detail = v;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_HYBRIDTRANSPARENCY {
        runtime: CvarDef::new("r_hybridTransparency", "1", "0.1..10",
            "Hybrid clarity multiplier for both Godot submerged absorption distance and Blue Water surface.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.hybrid_transparency.to_string()),
            |app, value| App::cvar_set_hybrid_transparency(app, APP_R_HYBRIDTRANSPARENCY.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_hybrid_transparency, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.hybrid_transparency = v.clamp(0.1, 10.0); } }
            }),
            archive: Some((25210, |state| state.video.ocean_settings.hybrid_transparency.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_hybrid_transparency(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.hybrid_transparency = Self::parse_cvar_finite(name, value)?.clamp(0.1, 10.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_HYBRIDFOGCOLOR {
        runtime: CvarDef::new("r_hybridFogColor", "", "r g b (0..1)",
            "Hybrid-only fog color shared by Godot submerged optics and native scattering.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c = app.video.ocean_settings.hybrid_fog_color;
                CvarValue::String(format!("{} {} {}", c[0], c[1], c[2])) },
            |app, value| App::cvar_set_hybrid_fog_color(app, APP_R_HYBRIDFOGCOLOR.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_hybrid_fog_color, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(c) = parse_vec3(value) {
                    if c.iter().all(|v| v.is_finite()) { settings.ocean_settings.hybrid_fog_color = c.map(|v| v.clamp(0.0, 1.0)); }
                }
            }),
            archive: Some((25211, |state| { let c = state.video.ocean_settings.hybrid_fog_color;
                format!("{} {} {}", c[0], c[1], c[2]) })),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_hybrid_fog_color(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.hybrid_fog_color = Self::parse_cvar_floats(name, value)?.map(|v| v.clamp(0.0, 1.0));
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_HYBRIDWATERCOLOR {
        runtime: CvarDef::new("r_hybridWaterColor", "", "r g b (0..1)",
            "Hybrid-only water body color; native Blue Water is unaffected.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c = app.video.ocean_settings.hybrid_water_color;
                CvarValue::String(format!("{} {} {}", c[0], c[1], c[2])) },
            |app, value| App::cvar_set_hybrid_water_color(app, APP_R_HYBRIDWATERCOLOR.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_hybrid_water_color, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(c) = parse_vec3(value) {
                    if c.iter().all(|v| v.is_finite()) { settings.ocean_settings.hybrid_water_color = c.map(|v| v.clamp(0.0, 1.0)); }
                }
            }),
            archive: Some((25212, |state| { let c = state.video.ocean_settings.hybrid_water_color;
                format!("{} {} {}", c[0], c[1], c[2]) })),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_hybrid_water_color(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.hybrid_water_color = Self::parse_cvar_floats(name, value)?.map(|v| v.clamp(0.0, 1.0));
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_HYBRIDCAUSTICSSCALE {
        runtime: CvarDef::new("r_hybridCausticsScale", "3", "1..6",
            "Hybrid Godot-receiver caustic pattern density: 3x matches 181m Blue Water to 57m Godot scale.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.hybrid_caustics_scale.to_string()),
            |app, value| App::cvar_set_hybrid_caustics_scale(app, APP_R_HYBRIDCAUSTICSSCALE.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_hybrid_caustics_scale, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.hybrid_caustics_scale = v.clamp(1.0, 6.0); } }
            }),
            archive: Some((25213, |state| state.video.ocean_settings.hybrid_caustics_scale.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_hybrid_caustics_scale(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.hybrid_caustics_scale = Self::parse_cvar_finite(name, value)?.clamp(1.0, 6.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_HYBRIDCAUSTICSGAIN {
        runtime: CvarDef::new("r_hybridCausticsGain", "1.5", "0..4",
            "Hybrid-only caustic focus contrast multiplier, applied after Godot receiver calculation.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.hybrid_caustics_gain.to_string()),
            |app, value| App::cvar_set_hybrid_caustics_gain(app, APP_R_HYBRIDCAUSTICSGAIN.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_hybrid_caustics_gain, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.hybrid_caustics_gain = v.clamp(0.0, 4.0); } }
            }),
            archive: Some((25214, |state| state.video.ocean_settings.hybrid_caustics_gain.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_hybrid_caustics_gain(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.hybrid_caustics_gain = Self::parse_cvar_finite(name, value)?.clamp(0.0, 4.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_HYBRIDPROCEDURALSCALE {
        runtime: CvarDef::new("r_hybridProceduralScale", "1", "0.25..4",
            "Hybrid animated caustic pattern density. Does not increase FFT or GPU generation cost.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.hybrid_procedural_scale.to_string()),
            |app, value| App::cvar_set_hybrid_procedural_scale(app, APP_R_HYBRIDPROCEDURALSCALE.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_hybrid_procedural_scale, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.hybrid_procedural_scale = v.clamp(0.25, 4.0); } }
            }),
            archive: Some((25217, |state| state.video.ocean_settings.hybrid_procedural_scale.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_hybrid_procedural_scale(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.hybrid_procedural_scale = Self::parse_cvar_finite(name, value)?.clamp(0.25, 4.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_HYBRIDPROCEDURALSPEED {
        runtime: CvarDef::new("r_hybridProceduralSpeed", "1", "0..3",
            "Hybrid animated caustic evolution speed, modulated by ocean roughness.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.hybrid_procedural_speed.to_string()),
            |app, value| App::cvar_set_hybrid_procedural_speed(app, APP_R_HYBRIDPROCEDURALSPEED.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_hybrid_procedural_speed, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.hybrid_procedural_speed = v.clamp(0.0, 3.0); } }
            }),
            archive: Some((25218, |state| state.video.ocean_settings.hybrid_procedural_speed.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_hybrid_procedural_speed(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.hybrid_procedural_speed = Self::parse_cvar_finite(name, value)?.clamp(0.0, 3.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_BLUEWATERREFRACTION {
        runtime: CvarDef::new("r_blueWaterRefraction", "0.1", "0..0.15",
            "Blue Water refraction strength; 0 disables. Select cheap screen offset or physical trace with r_blueWaterRefractionMode.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.blue_water_refraction.to_string()),
            |app, value| App::cvar_set_blue_water_refraction(app, APP_R_BLUEWATERREFRACTION.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_blue_water_refraction, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.blue_water_refraction = v.clamp(0.0, 0.15); }
                }
            }),
            archive: Some((25198, |state| state.video.ocean_settings.blue_water_refraction.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_blue_water_refraction(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.blue_water_refraction = Self::parse_cvar_finite(name, value)?.clamp(0.0, 0.15);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_BLUEWATERTRANSPARENCY {
        runtime: CvarDef::new("r_blueWaterTransparency", "1", "0.1..10",
            "Optical travel distance multiplier for Blue Water; higher is clearer, 1 preserves upstream calibration.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.blue_water_optics.transparency.to_string()),
            |app, value| App::cvar_set_blue_water_transparency(app, APP_R_BLUEWATERTRANSPARENCY.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_blue_water_transparency, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.blue_water_optics.transparency = v.clamp(0.1, 10.0); }
                }
            }),
            archive: Some((25155, |state| state.video.ocean_settings.blue_water_optics.transparency.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_blue_water_transparency(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.blue_water_optics.transparency = Self::parse_cvar_finite(name, value)?.clamp(0.1, 10.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_BLUEWATERWATERCOLOR {
        runtime: CvarDef::new("r_blueWaterWaterColor", "", "r g b (0..1)",
            "sRGB tint of the Blue Water body and transmitted crest light.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c = app.video.ocean_settings.blue_water_optics.water_color;
                CvarValue::String(format!("{} {} {}", c[0], c[1], c[2])) },
            |app, value| App::cvar_set_blue_water_water_color(app, APP_R_BLUEWATERWATERCOLOR.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_blue_water_water_color, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(c) = parse_vec3(value) {
                    if c.iter().all(|v| v.is_finite()) { settings.ocean_settings.blue_water_optics.water_color = c.map(|v| v.clamp(0.0, 1.0)); }
                }
            }),
            archive: Some((25156, |state| { let c = state.video.ocean_settings.blue_water_optics.water_color;
                format!("{} {} {}", c[0], c[1], c[2]) })),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_blue_water_water_color(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.blue_water_optics.water_color = Self::parse_cvar_floats(name, value)?;
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_BLUEWATERFOGCOLOR {
        runtime: CvarDef::new("r_blueWaterFogColor", "", "r g b (0..1)",
            "sRGB color of Blue Water underwater scattering.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c = app.video.ocean_settings.blue_water_optics.fog_color;
                CvarValue::String(format!("{} {} {}", c[0], c[1], c[2])) },
            |app, value| App::cvar_set_blue_water_fog_color(app, APP_R_BLUEWATERFOGCOLOR.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_blue_water_fog_color, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(c) = parse_vec3(value) {
                    if c.iter().all(|v| v.is_finite()) { settings.ocean_settings.blue_water_optics.fog_color = c.map(|v| v.clamp(0.0, 1.0)); }
                }
            }),
            archive: Some((25157, |state| { let c = state.video.ocean_settings.blue_water_optics.fog_color;
                format!("{} {} {}", c[0], c[1], c[2]) })),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_blue_water_fog_color(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.blue_water_optics.fog_color = Self::parse_cvar_floats(name, value)?;
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }

    APP_R_BLUEWATERHEIGHT {
        runtime: CvarDef::new("r_blueWaterHeight", "", "0.0..=60.0",
            "Significant wave height in metres (4 times the surface height standard deviation). Zero is flat.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.blue_water_waves.height.to_string()),
            |app, value| App::cvar_set_blue_water_height(app, APP_R_BLUEWATERHEIGHT.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_blue_water_height, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.blue_water_waves.height = v.clamp(0.0, 60.0); }
                }
            }),
            archive: Some((25150, |state| state.video.ocean_settings.blue_water_waves.height.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_blue_water_height(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.blue_water_waves.height = Self::parse_cvar_finite(name, value)?.clamp(0.0, 60.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_BLUEWATERDIRECTION {
        runtime: CvarDef::new("r_blueWaterDirection", "", "-180.0..=180.0",
            "Direction waves travel, in degrees from +X toward world +Z.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.blue_water_waves.direction.to_string()),
            |app, value| App::cvar_set_blue_water_direction(app, APP_R_BLUEWATERDIRECTION.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_blue_water_direction, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.blue_water_waves.direction = v.clamp(-180.0, 180.0); }
                }
            }),
            archive: Some((25151, |state| state.video.ocean_settings.blue_water_waves.direction.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_blue_water_direction(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.blue_water_waves.direction = Self::parse_cvar_finite(name, value)?.clamp(-180.0, 180.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_BLUEWATERWAVELENGTH {
        runtime: CvarDef::new("r_blueWaterWavelength", "", "2.0..=512.0",
            "Dominant crest spacing in metres. Controls wave speed through the native dispersion relation.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.blue_water_waves.wavelength.to_string()),
            |app, value| App::cvar_set_blue_water_wavelength(app, APP_R_BLUEWATERWAVELENGTH.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_blue_water_wavelength, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.blue_water_waves.wavelength = v.clamp(2.0, 512.0); }
                }
            }),
            archive: Some((25152, |state| state.video.ocean_settings.blue_water_waves.wavelength.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_blue_water_wavelength(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.blue_water_waves.wavelength = Self::parse_cvar_finite(name, value)?.clamp(2.0, 512.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_BLUEWATERCHOPPINESS {
        runtime: CvarDef::new("r_blueWaterChoppiness", "", "0.0..=4.0",
            "Horizontal displacement strength. Zero keeps only vertical waves.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.blue_water_waves.choppiness.to_string()),
            |app, value| App::cvar_set_blue_water_choppiness(app, APP_R_BLUEWATERCHOPPINESS.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_blue_water_choppiness, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.blue_water_waves.choppiness = v.clamp(0.0, 4.0); }
                }
            }),
            archive: Some((25153, |state| state.video.ocean_settings.blue_water_waves.choppiness.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_blue_water_choppiness(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.blue_water_waves.choppiness = Self::parse_cvar_finite(name, value)?.clamp(0.0, 4.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_BLUEWATERWINDSPEED {
        runtime: CvarDef::new("r_blueWaterWindSpeed", "", "0.1..=40.0",
            "Reference wind speed in metres per second. Changes the native spectral shape and fine surface roughness; independent of weather wind.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.blue_water_waves.wind_speed.to_string()),
            |app, value| App::cvar_set_blue_water_wind_speed(app, APP_R_BLUEWATERWINDSPEED.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_blue_water_wind_speed, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.blue_water_waves.wind_speed = v.clamp(0.1, 40.0); }
                }
            }),
            archive: Some((25154, |state| state.video.ocean_settings.blue_water_waves.wind_speed.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_blue_water_wind_speed(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.blue_water_waves.wind_speed = Self::parse_cvar_finite(name, value)?.clamp(0.1, 40.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }

    APP_CG_WATERWAKE {
        runtime: CvarDef::new("cg_waterwake", "1", "0|1|2",
            "Global player water wake: 0 off; 1 regular JKA shader on unpromoted water; 2 enhanced JKA wake plus configured Godot and Cheapwater player interaction. Independent of r_oceanType.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.water_wake.to_string()),
            |app, value| App::cvar_set_water_wake(app, APP_CG_WATERWAKE.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_water_wake, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(mode) = value.trim().parse::<u8>() {
                    settings.water_wake = mode.min(2);
                }
            }),
            archive: Some((25007, |state| state.video.water_wake.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_water_wake(&mut self, name: &str, value: &str) -> Result<(), String> {
                let mode: u8 = value.trim().parse()
                    .map_err(|_| format!("{name} expects 0, 1 or 2"))?;
                if mode > 2 { return Err(format!("{name} expects 0, 1 or 2")); }
                self.video.water_wake = mode;
                if let Some(session) = self.game_session.as_mut() {
                    session.water_wake_mode = mode;
                }
                // Keep saved Godot/Cheapwater preferences untouched. Only the
                // renderer-facing copies are gated by the global wake mode.
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_OCEANTYPE {
        runtime: CvarDef::new("r_oceanType", "4", "0..5",
            "Water promotion: 0 stock JKA, 1 Godot ocean, 2 native Blue Water ocean, 3 Hybrid ocean, 4 Blue Water lakes/ponds (subtle relief, no FFT), 5 Gerstner ocean (Hybrid with godot-water-shader's waves, colour and foam, lit by Blue Water).",
            CvarType::Int { min: 0, max: 5 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(i32::from(app.ocean_type())),
            |app, value| {
                let CvarValue::Int(value) = value else { unreachable!() };
                app.set_ocean_type(value as u8);
                Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<u8>() {
                    if v <= 5 {
                        settings.ocean = v != 0;
                        if v != 0 {
                            // 5 is Hybrid (backend 2) with the Gerstner surface.
                            settings.ocean_settings.backend = if v == 5 { 2 } else { v - 1 };
                            settings.ocean_settings.gerstner_water = v == 5;
                        }
                    }
                }
            }),
            archive: Some((25225, |state| if !state.video.ocean {
                "0".to_owned()
            } else if state.video.ocean_settings.gerstner_active() {
                "5".to_owned()
            } else {
                (state.video.ocean_settings.backend + 1).to_string()
            })),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_GERSTNERWATERCOLOR {
        runtime: CvarDef::new("r_gerstnerWaterColor", "0 0.21568628 0.25490198", "r g b (0..1)",
            "Gerstner ocean (r_oceanType 5) water color, sRGB (the original's 0, 55, 65). Crests blend toward r_gerstnerCrestColor.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c = app.video.ocean_settings.gerstner_water_color;
                CvarValue::String(format!("{} {} {}", c[0], c[1], c[2])) },
            |app, value| App::cvar_set_gerstner_water_color(app, APP_R_GERSTNERWATERCOLOR.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_gerstner_water_color, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(c) = parse_vec3(value) {
                    if c.iter().all(|v| v.is_finite()) { settings.ocean_settings.gerstner_water_color = c.map(|v| v.clamp(0.0, 1.0)); }
                }
            }),
            archive: Some((25234, |state| { let c = state.video.ocean_settings.gerstner_water_color;
                format!("{} {} {}", c[0], c[1], c[2]) })),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_gerstner_water_color(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.gerstner_water_color = Self::parse_cvar_floats(name, value)?.map(|v| v.clamp(0.0, 1.0));
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_GERSTNERCRESTCOLOR {
        runtime: CvarDef::new("r_gerstnerCrestColor", "0 0.3133 0.1517", "r g b (0..1)",
            "Gerstner ocean (r_oceanType 5) crest color, sRGB: wave tops blend from r_gerstnerWaterColor toward it (the original's green).", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| { let c = app.video.ocean_settings.gerstner_crest_color;
                CvarValue::String(format!("{} {} {}", c[0], c[1], c[2])) },
            |app, value| App::cvar_set_gerstner_crest_color(app, APP_R_GERSTNERCRESTCOLOR.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_gerstner_crest_color, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(c) = parse_vec3(value) {
                    if c.iter().all(|v| v.is_finite()) { settings.ocean_settings.gerstner_crest_color = c.map(|v| v.clamp(0.0, 1.0)); }
                }
            }),
            archive: Some((25235, |state| { let c = state.video.ocean_settings.gerstner_crest_color;
                format!("{} {} {}", c[0], c[1], c[2]) })),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_gerstner_crest_color(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.gerstner_crest_color = Self::parse_cvar_floats(name, value)?.map(|v| v.clamp(0.0, 1.0));
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_OCEANSURFACEPIXELS {
        runtime: CvarDef::new("r_oceanSurfacePixels", "1", "0|1",
            "Diagnostic A/B: 0 skips promoted ocean surface draws (Godot, native Blue Water, Hybrid and infinity); FFT, foam and mesh updates continue. 1 renders normally.",
            CvarType::Bool, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.video.ocean_settings.surface_pixels),
            |app, value| {
                let CvarValue::Bool(enabled) = value else { unreachable!() };
                if app.video.ocean_settings.surface_pixels != enabled {
                    app.video.ocean_settings.surface_pixels = enabled;
                    app.commit_ocean_settings();
                }
                Ok(())
            }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Some(enabled) = parse_bool(value) { settings.ocean_settings.surface_pixels = enabled; }
            }),
            archive: Some((25226, |state| (state.video.ocean_settings.surface_pixels as u8).to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_OCEANADAPTIVEMESH {
        runtime: CvarDef::new("r_oceanAdaptiveMesh", "0", "0..2",
            "0 native clipmap; 1 whole-ring curvature; 2 experimental stitched per-tile LOD with temporal vertex geomorph. Independent screen-space LOD still applies.",
            CvarType::Int { min: 0, max: 2 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Int(i32::from(app.video.ocean_settings.adaptive_mesh)),
            |app, value| { let CvarValue::Int(v) = value else { unreachable!() };
                app.video.ocean_settings.adaptive_mesh = v.clamp(0, 2) as u8;
                app.commit_ocean_settings(); Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<u8>() { settings.ocean_settings.adaptive_mesh = v.min(2); }
            }),
            archive: Some((25375, |state| state.video.ocean_settings.adaptive_mesh.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_OCEANADAPTIVEMESHTOLERANCE {
        runtime: CvarDef::new("r_oceanAdaptiveMeshTolerance", "1", "0.25..16",
            "Mode 2 local mesh error tolerance (1 original, 2 twice the allowed projected/normal error, 4 more aggressive). Larger values remove more triangles but may reduce crest fidelity. Modes 0/1 unaffected.",
            CvarType::Float { min: 0.25, max: 16.0 }, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::Float(app.video.ocean_settings.adaptive_mesh_tolerance),
            |app, value| { let CvarValue::Float(v) = value else { unreachable!() };
                app.video.ocean_settings.adaptive_mesh_tolerance = if v.is_finite() { v.clamp(0.25, 16.0) } else { 1.0 };
                app.commit_ocean_settings(); Ok(()) }),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() { if v.is_finite() {
                    settings.ocean_settings.adaptive_mesh_tolerance = v.clamp(0.25, 16.0);
                } }
            }),
            archive: Some((25376, |state| state.video.ocean_settings.adaptive_mesh_tolerance.to_string())),
            ..CvarBinding::NONE
        }, implementations: []
    }
    APP_R_OCEANADAPTIVEMESHDEBUG {
        runtime: CvarDef::new("r_oceanAdaptiveMeshDebug", "0", "0|1",
            "Print actual native ocean triangles vs identically PVS-pruned full-detail reference every second. Debug only.",
            CvarType::Bool, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.video.ocean_settings.adaptive_mesh_debug),
            |app, value| { let CvarValue::Bool(v) = value else { unreachable!() };
                app.video.ocean_settings.adaptive_mesh_debug = v;
                app.commit_ocean_settings(); Ok(()) }),
        config: CvarBinding::NONE, implementations: []
    }
    APP_R_OCEANCOSTPROBE {
        runtime: CvarDef::new("r_oceanCostProbe", "0", "0..67108863",
            "Diagnostic A/B, not archived: bitmask stubbing Blue Water surface features to size their GPU cost (changes the image). 1 SSR, 2 foam, 4 glints, 8 wake, 16 rain, 32 refraction, 64 FFT waves, 128 fog, 256 shadows, 512 sky lobe, 1024 hull shelter, 2048 underside, 4096 admission floor only; equivalent-math experiments 8192 defer water body until after SSR, 32768 serial SSR refinement; 16384 native FFT single-copy tiling (changes wave pattern); foam parts 131072 spread reads, 262144 breaking groups, 524288 gale streaks, 1048576 pattern reads; 2097152 value_noise reduced to a fract (noise cost bound); 4194304 wave sample + foam patterns as 8 reads (baked-cache bound); register packing across the reflection march: 8388608 fp16 pack2x16float, 16777216 fp16 wider set (sky, daylight, variances), 33554432 bfloat16 by shifts (not folded by the driver).",
            CvarType::Int { min: 0, max: 67108863 }, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.ocean_settings.surface_cost_probe as i32),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!() };
                app.video.ocean_settings.surface_cost_probe = v as u32;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_OCEANSKIPFADEDCASCADES {
        runtime: CvarDef::new("r_oceanSkipFadedCascades", "0", "0|1",
            "Native Blue Water A/B, not archived: 1 skips a wave cascade's texture reads at pixels where it has fully faded with distance (lossless: every use is scaled by its zero detail).",
            CvarType::Bool, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.video.ocean_settings.skip_faded_cascades),
            |app, value| {
                let CvarValue::Bool(v) = value else { unreachable!() };
                app.video.ocean_settings.skip_faded_cascades = v;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_OCEANWAVESTATISTICS {
        runtime: CvarDef::new("r_oceanWaveStatistics", "0", "0|1|2|3",
            "Native Blue Water A/B, not archived (lossy): 1 draws a wave cascade's slopes as their spectral statistics where a pixel's long side has averaged it (grazing far water), skipping its derivative reads; 2 also folds its foam spread read into the foam read; 3 debug: water shows how statistical each cascade is (red finest, green middle, blue largest).",
            CvarType::Int { min: 0, max: 3 }, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Int(i32::from(app.video.ocean_settings.wave_statistics)),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!() };
                app.video.ocean_settings.wave_statistics = v.clamp(0, 3) as u8;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_OCEANPIXELDEBUG {
        runtime: CvarDef::new("r_oceanPixelDebug", "0", "0|1",
            "Native Blue Water diagnostic, not archived: with r_oceanPixelDetail 5, tint the 2x2 blocks that get full shading by why (yellow water edge, blue depth step, red luminance).",
            CvarType::Bool, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.video.ocean_settings.pixel_detail_debug),
            |app, value| {
                let CvarValue::Bool(v) = value else { unreachable!() };
                app.video.ocean_settings.pixel_detail_debug = v;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_BLUEWATERSSRTILES {
        runtime: CvarDef::new("r_blueWaterSSRTiles", "1", "0|1",
            "Native Blue Water, not archived: 1 builds upstream's reflection tile table so reflection rays whose screen path holds only sky (or seabed below the wave envelope) skip the march, image-identical; 0 traces every ray (A/B).",
            CvarType::Bool, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.video.ocean_settings.blue_water_ssr_tiles),
            |app, value| {
                let CvarValue::Bool(v) = value else { unreachable!() };
                app.video.ocean_settings.blue_water_ssr_tiles = v;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_OCEANOPTICSRECEIVERONLY {
        runtime: CvarDef::new("r_oceanOpticsReceiverOnly", "1", "0|1",
            "Native Blue Water with Godot caustics, not archived: 1 runs the optical capture for its caustic receiver output alone when nothing else reads the fogged capture (image-identical); 0 runs the full capture (A/B).",
            CvarType::Bool, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.video.ocean_settings.optics_receiver_only),
            |app, value| {
                let CvarValue::Bool(v) = value else { unreachable!() };
                app.video.ocean_settings.optics_receiver_only = v;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_OCEANFARSHADER {
        runtime: CvarDef::new("r_oceanFarShader", "0", "0..134217727",
            "Native Blue Water A/B, not archived: 0 one shader everywhere; 1 water past the sky walls uses a slimmer shader (no screen-space reflection, refraction, wake or hull shelter; foam at its far-field average); above 1 an explicit stub mask (r_oceanCostProbe bits).",
            CvarType::Int { min: 0, max: 134217727 }, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Int(app.video.ocean_settings.far_shader as i32),
            |app, value| {
                let CvarValue::Int(v) = value else { unreachable!() };
                app.video.ocean_settings.far_shader = v as u32;
                app.commit_ocean_settings();
                Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_OCEANBACKEND {
        runtime: CvarDef::new("r_oceanBackend", "0", "0|1",
            "Legacy compatibility alias: 0 Godot; 1 native Blue Water. Use r_oceanType for Off/Hybrid.",
            CvarType::Int { min: 0, max: 1 }, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Int(i32::from(app.video.ocean_settings.backend != 0)),
            |app, value| {
                let CvarValue::Int(backend) = value else { unreachable!() };
                let backend = backend as u8;
                if app.video.ocean_settings.backend != backend || !app.video.ocean {
                    app.set_ocean_type(backend + 1);
                }
                Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_OCEANSKYEXTEND {
        runtime: CvarDef::new("r_oceanSkyExtend", "2", "0|1|2",
            "0 original water; 1 camera-relative 3D sky extension; 2 3D clipmap plus mesh-free far ocean. Coastal unchanged.",
            CvarType::Int { min: 0, max: 2 }, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Int(app.ocean_sky_extend as i32),
            |app, value| {
                let CvarValue::Int(mode) = value else { unreachable!() };
                let mode = mode as u8;
                if app.ocean_sky_extend != mode {
                    app.ocean_sky_extend = mode;
                    app.render_command(RenderCommand::SetOceanSkyExtend(mode));
                }
                Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_COASTAL {
        runtime: CvarDef::new("r_coastal", "0", "0|1",
            "Opt-in GPU shallow-bank solver for one promoted pool; r_coastalPool selects its 1-based pool number (0 automatic).",
            CvarType::Bool, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Bool(app.coastal_enabled),
            |app, value| {
                let CvarValue::Bool(enabled)=value else {unreachable!()};
                app.coastal_enabled=enabled;app.coastal_live_sent=None;Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_COASTALPOOL {
        runtime: CvarDef::new("r_coastalPool", "0", "0..8",
            "Live coastal pool: 0 selects an eligible shallow-bank pool automatically; 1..8 match the debug Pool numbers.",
            CvarType::Int {min:0,max:8}, CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::Int(app.coastal_pool.into()),
            |app, value| {
                let CvarValue::Int(pool)=value else {unreachable!()};
                app.coastal_pool=pool as u8;app.coastal_live_sent=None;Ok(())
            }),
        config: CvarBinding::NONE,
        implementations: []
    }
    APP_R_COASTALDEBUG {
        runtime: CvarDef::new(
                "r_coastalDebug",
                "0",
                "0..3",
                "Phase 2.5 promoted-water evaluation: 0 off, 1 treatment tint, 2 boundary faces, 3 bathymetry/depth cells.",
                CvarType::Int { min: 0, max: 3 },
                CvarFlags::NONE,
                LatchScope::None,
                |app: &App| CvarValue::Int(app.coastal_debug_mode.into()),
                |app, value| {
                    let CvarValue::Int(mode) = value else { unreachable!() };
                    app.coastal_debug_mode = mode as u8;
                    app.coastal_debug_sent = None;
                    Ok(())
                },
            ),
        config: CvarBinding::NONE,
        implementations: []
    }
    // A single switch for the opt-in visual corrections; does not mutate
    // the individual A/B CVar values or the simulation state.
    APP_R_OCEANFOAMDRIFT {
        runtime: CvarDef::new("r_oceanFoamDrift", "0", "0..2",
            "Rolls the persistent FFT whitecap foam downwind at this fraction of wind speed, so foam is not pinned to the tile that formed it (the visible 88 m repeat). 0 off, 0.2 is the Unity port's default.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.foam_drift.to_string()),
            |app, value| App::cvar_set_foam_drift(app, APP_R_OCEANFOAMDRIFT.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_foam_drift, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.foam_drift = v.clamp(0.0, 2.0); }
                }
            }),
            archive: Some((25034, |state| state.video.ocean_settings.foam_drift.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_foam_drift(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.foam_drift = Self::parse_cvar_finite(name, value)?.clamp(0.0, 2.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_OCEANFOAMGAIN {
        runtime: CvarDef::new("r_oceanFoamGain", "1", "0..16",
            "Post-simulation breaker foam strength before smoothstep. 1 unchanged, 4 or 8 shows marginal foam.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.foam_gain.to_string()),
            |app, value| App::cvar_set_ab_foam_gain(app, APP_R_OCEANFOAMGAIN.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_ab_foam_gain, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.foam_gain = v.clamp(0.0, 16.0); }
                }
            }),
            archive: Some((25012, |state| state.video.ocean_settings.foam_gain.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_ab_foam_gain(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.foam_gain = Self::parse_cvar_finite(name, value)?.clamp(0.0, 16.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_OCEANFOAMBRIGHTNESS {
        runtime: CvarDef::new("r_oceanFoamBrightness", "1", "0..8",
            "Multiplier on linear foam RGB in ocean surface illumination, 1 unchanged.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.foam_brightness.to_string()),
            |app, value| App::cvar_set_ab_foam_brightness(app, APP_R_OCEANFOAMBRIGHTNESS.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_ab_foam_brightness, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.foam_brightness = v.clamp(0.0, 8.0); }
                }
            }),
            archive: Some((25015, |state| state.video.ocean_settings.foam_brightness.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_ab_foam_brightness(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.foam_brightness = Self::parse_cvar_finite(name, value)?.clamp(0.0, 8.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_OCEANSUNGAIN {
        runtime: CvarDef::new("r_oceanSunGain", "1", "0..8",
            "Multiplier on direct sun diffuse/specular contribution (not sky ambient), 1 unchanged.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.sun_gain.to_string()),
            |app, value| App::cvar_set_ab_sun_gain(app, APP_R_OCEANSUNGAIN.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_ab_sun_gain, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.sun_gain = v.clamp(0.0, 8.0); }
                }
            }),
            archive: Some((25016, |state| state.video.ocean_settings.sun_gain.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_ab_sun_gain(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.sun_gain = Self::parse_cvar_finite(name, value)?.clamp(0.0, 8.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_OCEANINTERACTION {
        runtime: CvarDef::new("r_oceanInteraction", "", "0|1",
            "Nearby player water deformation; inherits r_oceanUpdates.", CvarType::String,
            CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(bool_value(app.video.ocean_settings.interaction)),
            |app, value| App::cvar_set_ocean_interaction(app, APP_R_OCEANINTERACTION.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_ocean_interaction, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                settings.ocean_settings.interaction = parse_bool(value).unwrap_or(settings.ocean_settings.interaction);
            }),
            archive: Some((25000, |state| bool_value(state.video.ocean_settings.interaction))),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_ocean_interaction(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.interaction = Self::parse_cvar_bool(name, value)?;
                self.commit_ocean_settings(); Ok(())
            }
        }]
    }
    APP_R_OCEANINTERACTIONSTRENGTH {
        runtime: CvarDef::new("r_oceanInteractionStrength", "", "nonnegative finite number",
            "Nearby water interaction force scale.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.interaction_strength.to_string()),
            |app, value| App::cvar_set_ocean_interaction_strength(app, APP_R_OCEANINTERACTIONSTRENGTH.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_ocean_interaction_strength, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.interaction_strength = v.max(0.0); } }
            }),
            archive: Some((25001, |state| state.video.ocean_settings.interaction_strength.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_ocean_interaction_strength(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.interaction_strength = Self::parse_cvar_finite(name,value)?.max(0.0);
                self.commit_ocean_settings(); Ok(())
            }
        }]
    }
    APP_R_OCEANINTERACTIONFX {
        runtime: CvarDef::new("r_oceanInteractionFx", "", "0..5",
            "Player water FX: 0 off, 1 entry droplets, 2 wake spray, 3 both, 4 entry jets, 5 all.", CvarType::String,
            CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.interaction_splash_mode.to_string()),
            |app, value| App::cvar_set_ocean_interaction_fx(app, APP_R_OCEANINTERACTIONFX.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_ocean_interaction_fx, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(mode) = value.parse::<u8>() {
                    settings.ocean_settings.interaction_splash_mode = mode.min(5);
                }
            }),
            archive: Some((25006, |state| state.video.ocean_settings.interaction_splash_mode.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_ocean_interaction_fx(&mut self, name: &str, value: &str) -> Result<(), String> {
                let mode = value.parse::<u8>()
                    .map_err(|_| format!("{name} must be in the range 0..5"))?;
                if mode > 5 {
                    return Err(format!("{name} must be in the range 0..5"));
                }
                self.video.ocean_settings.interaction_splash_mode = mode;
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_OCEANINTERACTIONLIFETIME {
        runtime: CvarDef::new("r_oceanInteractionLifetime", "", "0.1..60 seconds",
            "Seconds after the last moving input before retiring the local field.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.interaction_lifetime.to_string()),
            |app, value| App::cvar_set_ocean_interaction_falloff(app, APP_R_OCEANINTERACTIONLIFETIME.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_ocean_interaction_falloff, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.interaction_lifetime = v.clamp(0.1,60.0); } }
            }),
            archive: Some((25002, |state| state.video.ocean_settings.interaction_lifetime.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_ocean_interaction_falloff(&mut self, name: &str, value: &str) -> Result<(), String> {
                let v = Self::parse_cvar_finite(name,value)?;
                match name.to_ascii_lowercase().as_str() {
                    "r_oceaninteractionlifetime" => self.video.ocean_settings.interaction_lifetime = v.clamp(0.1,60.0),
                    "r_oceaninteractionfade" => self.video.ocean_settings.interaction_fade = v.clamp(0.0,60.0),
                    "r_oceaninteractiondamping" => self.video.ocean_settings.interaction_damping = v.max(0.0),
                    "r_oceaninteractionedgefade" => self.video.ocean_settings.interaction_edge_fade = v.clamp(0.0,256.0),
                    _ => unreachable!(),
                }
                self.commit_ocean_settings(); Ok(())
            }
        }]
    }
    APP_R_OCEANINTERACTIONFADE {
        runtime: CvarDef::new("r_oceanInteractionFade", "", "0..60 seconds",
            "Duration of the final smooth fade before field retirement; zero disables temporal fading.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.interaction_fade.to_string()),
            |app, value| App::cvar_set_ocean_interaction_falloff(app, APP_R_OCEANINTERACTIONFADE.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_ocean_interaction_falloff, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.interaction_fade = v.clamp(0.0,60.0); } }
            }),
            archive: Some((25003, |state| state.video.ocean_settings.interaction_fade.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_OCEANINTERACTIONDAMPING {
        runtime: CvarDef::new("r_oceanInteractionDamping", "", "nonnegative finite number",
            "Velocity decay per second; zero disables bulk damping; height decay is 0.175 times this rate.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.interaction_damping.to_string()),
            |app, value| App::cvar_set_ocean_interaction_falloff(app, APP_R_OCEANINTERACTIONDAMPING.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_ocean_interaction_falloff, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.interaction_damping = v.max(0.0); } }
            }),
            archive: Some((25004, |state| state.video.ocean_settings.interaction_damping.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_OCEANINTERACTIONEDGEFADE {
        runtime: CvarDef::new("r_oceanInteractionEdgeFade", "", "0..256 world units",
            "Width of the local-window edge fade; sponge width is 1.5 times this value; zero disables both.", CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.interaction_edge_fade.to_string()),
            |app, value| App::cvar_set_ocean_interaction_falloff(app, APP_R_OCEANINTERACTIONEDGEFADE.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_ocean_interaction_falloff, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.interaction_edge_fade = v.clamp(0.0,256.0); } }
            }),
            archive: Some((25005, |state| state.video.ocean_settings.interaction_edge_fade.to_string())),
            ..CvarBinding::NONE
        },
        implementations: []
    }
    APP_R_OCEANINTERACTIONDEBUG {
        runtime: CvarDef::new("r_oceanInteractionDebug", "", "0|1",
            "Show nearby interaction height (red/blue) and slopes (green).", CvarType::String,
            CvarFlags::NONE, LatchScope::None,
            |app: &App| CvarValue::String(bool_value(app.video.ocean_settings.interaction_debug)),
            |app, value| App::cvar_set_ocean_interaction_debug(app, APP_R_OCEANINTERACTIONDEBUG.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_ocean_interaction_debug, None),
        config: CvarBinding::NONE,
        implementations: [impl App {
            fn cvar_set_ocean_interaction_debug(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.interaction_debug = Self::parse_cvar_bool(name,value)?;
                self.commit_ocean_settings(); Ok(())
            }
        }]
    }
    APP_R_OCEAN {
        runtime: CvarDef::new(
                "r_ocean",
                "",
                "0|1",
                "GPU ocean. Enabling on a world loaded without ocean requires applying video settings.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_ocean(app)),
                |app, value| App::cvar_set_r_ocean(app, APP_R_OCEAN.entry.name, &value.to_string()),
            )
            .with_text_setter(App::cvar_set_r_ocean, None)
            .with_restart_requirement(|app| app.video.ocean && !app.applied_ocean),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.ocean = parse_bool(value).unwrap_or(settings.ocean)
                }),
                archive: Some((93, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.ocean))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_ocean(&self) -> String {
                    bool_value(self.video.ocean)
                }
                fn cvar_set_r_ocean(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ocean = Self::parse_cvar_bool(name, value)?;
                        if let Some(session) = self.game_session.as_mut() {
                            session.ocean_interaction_enabled = self.video.ocean && self.video.ocean_settings.interaction && self.video.water_wake == 2;
                            session.lake_wakes_enabled = self.video.water_wake == 2 && self.video.ocean
                                && self.video.ocean_settings.backend == 3 && self.video.ocean_settings.lake.shader == 1
                                && self.video.ocean_settings.interaction && self.video.ocean_settings.interaction_strength > 0.0;
                        }
                        // OFF releases the FFT resources immediately, and ON can be
                        // restored live only when the loaded world already carries its
                        // promoted ocean clipmap.
                        if !self.video.ocean || self.applied_ocean {
                            self.render_command(RenderCommand::SetOceanEnabled(self.video.ocean));
                        }
                        self.mark_config_dirty();
                        self.publish_ui();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANAUTHORING {
        runtime: CvarDef::new(
                "r_oceanAuthoring",
                "",
                "custom",
                "Controls r_oceanAuthoring.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanauthoring(app)),
                |app, value| {
                    App::cvar_set_r_oceanauthoring(app, APP_R_OCEANAUTHORING.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceanauthoring, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    let v: Vec<_> = value.split_whitespace().collect();
                    if v.len() == 11 {
                        let floats: Option<Vec<f32>> = v[..10].iter().map(|s| s.parse().ok()).collect();
                        if let (Some(f), Ok(seed)) = (floats, v[10].parse()) {
                            settings.ocean_settings.authored = crate::ocean::OceanAuthoring {
                                amplitude: f[0],
                                wavelength: f[1],
                                speed: f[2],
                                direction: f[3],
                                steepness: f[4],
                                slosh: f[5],
                                wind_chop: f[6],
                                foam: f[7],
                                foam_lifetime: f[8],
                                spray: f[9],
                                seed,
                            }
                            .sanitize();
                        }
                    }
                }),
                archive: Some((10419, |state| {
                    let settings = state.video;
                    let a = settings.ocean_settings.authored;
                    format!(
                        "{} {} {} {} {} {} {} {} {} {} {}",
                        a.amplitude,
                        a.wavelength,
                        a.speed,
                        a.direction,
                        a.steepness,
                        a.slosh,
                        a.wind_chop,
                        a.foam,
                        a.foam_lifetime,
                        a.spray,
                        a.seed
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanauthoring(&self) -> String {
                    {
                        let a = self.video.ocean_settings.authored;
                        format!(
                            "{} {} {} {} {} {} {} {} {} {} {}",
                            a.amplitude,
                            a.wavelength,
                            a.speed,
                            a.direction,
                            a.steepness,
                            a.slosh,
                            a.wind_chop,
                            a.foam,
                            a.foam_lifetime,
                            a.spray,
                            a.seed,
                        )
                    }
                }
                fn cvar_set_r_oceanauthoring(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let words: Vec<&str> = value.split_whitespace().collect();
                        let expected = || format!("{name}: expected ten numbers and a seed");
                        if words.len() != 11 {
                            return Err(expected());
                        }
                        let mut f = [0.0_f32; 10];
                        for (slot, word) in f.iter_mut().zip(&words) {
                            *slot = word
                                .parse::<f32>()
                                .ok()
                                .filter(|number| number.is_finite())
                                .ok_or_else(expected)?;
                        }
                        let seed = words[10].parse().map_err(|_| expected())?;
                        self.video.ocean_settings.authored = crate::ocean::OceanAuthoring {
                            amplitude: f[0],
                            wavelength: f[1],
                            speed: f[2],
                            direction: f[3],
                            steepness: f[4],
                            slosh: f[5],
                            wind_chop: f[6],
                            foam: f[7],
                            foam_lifetime: f[8],
                            spray: f[9],
                            seed,
                        }
                        .sanitize();
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANCASCADE1 {
        runtime: CvarDef::new(
                "r_oceanCascade1",
                "",
                "custom",
                "Controls r_oceanCascade1.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceancascade1(app)),
                |app, value| {
                    App::cvar_set_r_oceancascade1(app, APP_R_OCEANCASCADE1.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceancascade1, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.ocean_settings.cascades[0] =
                            parse_ocean_cascade(value, settings.ocean_settings.cascades[0])
                    }
                }),
                archive: Some((103, |state| {
                    let settings = state.video;
                    format!(
                        "{:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4}",
                        settings.ocean_settings.cascades[0].tile_length[0],
                        settings.ocean_settings.cascades[0].tile_length[1],
                        settings.ocean_settings.cascades[0].displacement_scale,
                        settings.ocean_settings.cascades[0].normal_scale,
                        settings.ocean_settings.cascades[0].wind_speed,
                        settings.ocean_settings.cascades[0].wind_direction,
                        settings.ocean_settings.cascades[0].fetch_length,
                        settings.ocean_settings.cascades[0].swell,
                        settings.ocean_settings.cascades[0].spread,
                        settings.ocean_settings.cascades[0].detail,
                        settings.ocean_settings.cascades[0].whitecap,
                        settings.ocean_settings.cascades[0].foam_amount
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceancascade1(&self) -> String {
                    let name = "r_oceancascade1";
                    {
                        let digit = name.as_bytes()[name.len() - 1];
                        let c = self.video.ocean_settings.cascades[usize::from(digit - b'1')];
                        format!(
                            "{:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4}",
                            c.tile_length[0],
                            c.tile_length[1],
                            c.displacement_scale,
                            c.normal_scale,
                            c.wind_speed,
                            c.wind_direction,
                            c.fetch_length,
                            c.swell,
                            c.spread,
                            c.detail,
                            c.whitecap,
                            c.foam_amount,
                        )
                    }
                }
                fn cvar_set_r_oceancascade1(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_oceancascade1".to_owned();
                    {
                        let v: [f32; 12] = Self::parse_cvar_floats(name, value)?;
                        let index = usize::from(lower.as_bytes()[lower.len() - 1] - b'1');
                        self.video.ocean_settings.cascades[index] = crate::ocean::CascadeSettings {
                            tile_length: [v[0], v[1]],
                            displacement_scale: v[2],
                            normal_scale: v[3],
                            wind_speed: v[4],
                            wind_direction: v[5],
                            fetch_length: v[6],
                            swell: v[7],
                            spread: v[8],
                            detail: v[9],
                            whitecap: v[10],
                            foam_amount: v[11],
                        };
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANCASCADE2 {
        runtime: CvarDef::new(
                "r_oceanCascade2",
                "",
                "custom",
                "Controls r_oceanCascade2.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceancascade2(app)),
                |app, value| {
                    App::cvar_set_r_oceancascade2(app, APP_R_OCEANCASCADE2.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceancascade2, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.ocean_settings.cascades[1] =
                            parse_ocean_cascade(value, settings.ocean_settings.cascades[1])
                    }
                }),
                archive: Some((104, |state| {
                    let settings = state.video;
                    format!(
                        "{:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4}",
                        settings.ocean_settings.cascades[1].tile_length[0],
                        settings.ocean_settings.cascades[1].tile_length[1],
                        settings.ocean_settings.cascades[1].displacement_scale,
                        settings.ocean_settings.cascades[1].normal_scale,
                        settings.ocean_settings.cascades[1].wind_speed,
                        settings.ocean_settings.cascades[1].wind_direction,
                        settings.ocean_settings.cascades[1].fetch_length,
                        settings.ocean_settings.cascades[1].swell,
                        settings.ocean_settings.cascades[1].spread,
                        settings.ocean_settings.cascades[1].detail,
                        settings.ocean_settings.cascades[1].whitecap,
                        settings.ocean_settings.cascades[1].foam_amount
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceancascade2(&self) -> String {
                    let name = "r_oceancascade2";
                    {
                        let digit = name.as_bytes()[name.len() - 1];
                        let c = self.video.ocean_settings.cascades[usize::from(digit - b'1')];
                        format!(
                            "{:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4}",
                            c.tile_length[0],
                            c.tile_length[1],
                            c.displacement_scale,
                            c.normal_scale,
                            c.wind_speed,
                            c.wind_direction,
                            c.fetch_length,
                            c.swell,
                            c.spread,
                            c.detail,
                            c.whitecap,
                            c.foam_amount,
                        )
                    }
                }
                fn cvar_set_r_oceancascade2(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_oceancascade2".to_owned();
                    {
                        let v: [f32; 12] = Self::parse_cvar_floats(name, value)?;
                        let index = usize::from(lower.as_bytes()[lower.len() - 1] - b'1');
                        self.video.ocean_settings.cascades[index] = crate::ocean::CascadeSettings {
                            tile_length: [v[0], v[1]],
                            displacement_scale: v[2],
                            normal_scale: v[3],
                            wind_speed: v[4],
                            wind_direction: v[5],
                            fetch_length: v[6],
                            swell: v[7],
                            spread: v[8],
                            detail: v[9],
                            whitecap: v[10],
                            foam_amount: v[11],
                        };
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANCASCADE3 {
        runtime: CvarDef::new(
                "r_oceanCascade3",
                "",
                "custom",
                "Controls r_oceanCascade3.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceancascade3(app)),
                |app, value| {
                    App::cvar_set_r_oceancascade3(app, APP_R_OCEANCASCADE3.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceancascade3, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.ocean_settings.cascades[2] =
                            parse_ocean_cascade(value, settings.ocean_settings.cascades[2])
                    }
                }),
                archive: Some((105, |state| {
                    let settings = state.video;
                    format!(
                        "{:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4}",
                        settings.ocean_settings.cascades[2].tile_length[0],
                        settings.ocean_settings.cascades[2].tile_length[1],
                        settings.ocean_settings.cascades[2].displacement_scale,
                        settings.ocean_settings.cascades[2].normal_scale,
                        settings.ocean_settings.cascades[2].wind_speed,
                        settings.ocean_settings.cascades[2].wind_direction,
                        settings.ocean_settings.cascades[2].fetch_length,
                        settings.ocean_settings.cascades[2].swell,
                        settings.ocean_settings.cascades[2].spread,
                        settings.ocean_settings.cascades[2].detail,
                        settings.ocean_settings.cascades[2].whitecap,
                        settings.ocean_settings.cascades[2].foam_amount
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceancascade3(&self) -> String {
                    let name = "r_oceancascade3";
                    {
                        let digit = name.as_bytes()[name.len() - 1];
                        let c = self.video.ocean_settings.cascades[usize::from(digit - b'1')];
                        format!(
                            "{:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4}",
                            c.tile_length[0],
                            c.tile_length[1],
                            c.displacement_scale,
                            c.normal_scale,
                            c.wind_speed,
                            c.wind_direction,
                            c.fetch_length,
                            c.swell,
                            c.spread,
                            c.detail,
                            c.whitecap,
                            c.foam_amount,
                        )
                    }
                }
                fn cvar_set_r_oceancascade3(&mut self, name: &str, value: &str) -> Result<(), String> {
                    let lower = "r_oceancascade3".to_owned();
                    {
                        let v: [f32; 12] = Self::parse_cvar_floats(name, value)?;
                        let index = usize::from(lower.as_bytes()[lower.len() - 1] - b'1');
                        self.video.ocean_settings.cascades[index] = crate::ocean::CascadeSettings {
                            tile_length: [v[0], v[1]],
                            displacement_scale: v[2],
                            normal_scale: v[3],
                            wind_speed: v[4],
                            wind_direction: v[5],
                            fetch_length: v[6],
                            swell: v[7],
                            spread: v[8],
                            detail: v[9],
                            whitecap: v[10],
                            foam_amount: v[11],
                        };
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANCAUSTICSMAXDISTANCE {
        runtime: CvarDef::new("r_oceanCausticsMaxDistance", "80", "0..500 metres",
            "Godot and Hybrid Godot caustics: camera-to-receiver range with fade over outer 25%; 0 disables. Native Blue Water has its own height cull.",
            CvarType::String, CvarFlags::ARCHIVE, LatchScope::None,
            |app: &App| CvarValue::String(app.video.ocean_settings.optics.caustic_max_distance_m.to_string()),
            |app, value| App::cvar_set_r_oceancausticsmaxdistance(app, APP_R_OCEANCAUSTICSMAXDISTANCE.entry.name, &value.to_string()))
            .with_text_setter(App::cvar_set_r_oceancausticsmaxdistance, None),
        config: CvarBinding {
            load_video: Some(|settings, _name, value, _context| {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() { settings.ocean_settings.optics.caustic_max_distance_m = v.clamp(0.0, 500.0); }
                }
            }),
            archive: Some((25216, |state| state.video.ocean_settings.optics.caustic_max_distance_m.to_string())),
            ..CvarBinding::NONE
        },
        implementations: [impl App {
            fn cvar_set_r_oceancausticsmaxdistance(&mut self, name: &str, value: &str) -> Result<(), String> {
                self.video.ocean_settings.optics.caustic_max_distance_m = Self::parse_cvar_finite(name, value)?.clamp(0.0, 500.0);
                self.commit_ocean_settings();
                Ok(())
            }
        }]
    }
    APP_R_OCEANCAUSTICS {
        runtime: CvarDef::new(
                "r_oceanCaustics",
                "",
                "0..8",
                "Experimental FFT curvature caustics; not shadow-aware yet.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceancaustics(app)),
                |app, value| {
                    App::cvar_set_r_oceancaustics(app, APP_R_OCEANCAUSTICS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceancaustics, None),
        config: CvarBinding {
                load_video: Some(|settings, name, value, _context| {
                    if let Ok(v) = value.parse::<f32>() {
                        if v.is_finite() {
                            let o = &mut settings.ocean_settings.optics;
                            match name.to_ascii_lowercase().as_str() {
                                "r_oceantransparency" => o.transparency = v,
                                "r_oceandepthdarkening" => o.depth_darkening = v,
                                "r_oceanrefraction" => o.refraction = v,
                                "r_oceancaustics" => o.caustics = v,
                                _ => o.underwater_cull = v,
                            }
                            *o = o.sanitize();
                        }
                    }
                }),
                archive: Some((10417, |state| {
                    let settings = state.video;
                    let o = settings.ocean_settings.optics;
                    format!("{}", o.caustics)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceancaustics(&self) -> String {
                    self.video.ocean_settings.optics.caustics.to_string()
                }
                fn cvar_set_r_oceancaustics(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let v = value
                            .parse::<f32>()
                            .map_err(|_| "Expected a finite number".to_owned())?;
                        if !v.is_finite() {
                            return Err("Expected a finite number".to_owned());
                        }
                        let o = &mut self.video.ocean_settings.optics;
                        match name.to_ascii_lowercase().as_str() {
                            "r_oceantransparency" => o.transparency = v,
                            "r_oceandepthdarkening" => o.depth_darkening = v,
                            "r_oceanrefraction" => o.refraction = v,
                            "r_oceancaustics" => o.caustics = v,
                            _ => o.underwater_cull = v,
                        }
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANDEPTHDARKENING {
        runtime: CvarDef::new(
                "r_oceanDepthDarkening",
                "",
                "0.01..32",
                "Underwater sunlight penetration multiplier.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceandepthdarkening(app)),
                |app, value| {
                    App::cvar_set_r_oceandepthdarkening(
                        app,
                        APP_R_OCEANDEPTHDARKENING.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_oceandepthdarkening, None),
        config: CvarBinding {
                load_video: Some(|settings, name, value, _context| {
                    if let Ok(v) = value.parse::<f32>() {
                        if v.is_finite() {
                            let o = &mut settings.ocean_settings.optics;
                            match name.to_ascii_lowercase().as_str() {
                                "r_oceantransparency" => o.transparency = v,
                                "r_oceandepthdarkening" => o.depth_darkening = v,
                                "r_oceanrefraction" => o.refraction = v,
                                "r_oceancaustics" => o.caustics = v,
                                _ => o.underwater_cull = v,
                            }
                            *o = o.sanitize();
                        }
                    }
                }),
                archive: Some((10415, |state| {
                    let settings = state.video;
                    let o = settings.ocean_settings.optics;
                    format!("{}", o.depth_darkening)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceandepthdarkening(&self) -> String {
                    self.video.ocean_settings.optics.depth_darkening.to_string()
                }
                fn cvar_set_r_oceandepthdarkening(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        let v = value
                            .parse::<f32>()
                            .map_err(|_| "Expected a finite number".to_owned())?;
                        if !v.is_finite() {
                            return Err("Expected a finite number".to_owned());
                        }
                        let o = &mut self.video.ocean_settings.optics;
                        match name.to_ascii_lowercase().as_str() {
                            "r_oceantransparency" => o.transparency = v,
                            "r_oceandepthdarkening" => o.depth_darkening = v,
                            "r_oceanrefraction" => o.refraction = v,
                            "r_oceancaustics" => o.caustics = v,
                            _ => o.underwater_cull = v,
                        }
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANFOAMCOLOR {
        runtime: CvarDef::new(
                "r_oceanFoamColor",
                "",
                "custom",
                "Controls r_oceanFoamColor.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanfoamcolor(app)),
                |app, value| {
                    App::cvar_set_r_oceanfoamcolor(app, APP_R_OCEANFOAMCOLOR.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceanfoamcolor, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.ocean_settings.foam_color =
                        parse_vec3(value).unwrap_or(settings.ocean_settings.foam_color);
                }),
                archive: Some((102, |state| {
                    let settings = state.video;
                    format!(
                        "{:.4} {:.4} {:.4}",
                        settings.ocean_settings.foam_color[0],
                        settings.ocean_settings.foam_color[1],
                        settings.ocean_settings.foam_color[2]
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanfoamcolor(&self) -> String {
                    {
                        let c = self.video.ocean_settings.foam_color;
                        format!("{:.4} {:.4} {:.4}", c[0], c[1], c[2])
                    }
                }
                fn cvar_set_r_oceanfoamcolor(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ocean_settings.foam_color = Self::parse_cvar_floats(name, value)?;
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANFOGCOLOR {
        runtime: CvarDef::new(
                "r_oceanFogColor",
                "",
                "R G B (0..1)",
                "Water scattering tint.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanfogcolor(app)),
                |app, value| {
                    App::cvar_set_r_oceanfogcolor(app, APP_R_OCEANFOGCOLOR.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceanfogcolor, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    {
                        settings.ocean_settings.optics.fog_color =
                            parse_vec3(value).unwrap_or(settings.ocean_settings.optics.fog_color)
                    }
                }),
                archive: Some((10412, |state| {
                    let settings = state.video;
                    let o = settings.ocean_settings.optics;
                    format!("{} {} {}", o.fog_color[0], o.fog_color[1], o.fog_color[2])
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanfogcolor(&self) -> String {
                    {
                        let c = self.video.ocean_settings.optics.fog_color;
                        format!("{} {} {}", c[0], c[1], c[2])
                    }
                }
                fn cvar_set_r_oceanfogcolor(&mut self, _name: &str, value: &str) -> Result<(), String> {
                    {
                        let values = value
                            .split_whitespace()
                            .map(str::parse::<f32>)
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(|_| "Expected three finite color components".to_owned())?;
                        if values.len() != 3 || !values.iter().all(|v| v.is_finite()) {
                            return Err("Expected three finite color components".to_owned());
                        }
                        self.video.ocean_settings.optics.fog_color = [values[0], values[1], values[2]];
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANTRANSPARENCY {
        runtime: CvarDef::new(
                "r_oceanTransparency",
                "",
                "1..65536",
                "Water transparency: how far light travels through the water before it is absorbed, in game units. Lower is murkier, above and below the surface.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceantransparency(app)),
                |app, value| {
                    App::cvar_set_r_oceantransparency(
                        app,
                        APP_R_OCEANTRANSPARENCY.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_oceantransparency, None),
        config: CvarBinding {
                load_video: Some(|settings, name, value, _context| {
                    if let Ok(v) = value.parse::<f32>() {
                        if v.is_finite() {
                            let o = &mut settings.ocean_settings.optics;
                            match name.to_ascii_lowercase().as_str() {
                                "r_oceantransparency" => o.transparency = v,
                                "r_oceandepthdarkening" => o.depth_darkening = v,
                                "r_oceanrefraction" => o.refraction = v,
                                "r_oceancaustics" => o.caustics = v,
                                _ => o.underwater_cull = v,
                            }
                            *o = o.sanitize();
                        }
                    }
                }),
                archive: Some((10413, |state| {
                    let settings = state.video;
                    let o = settings.ocean_settings.optics;
                    format!("{}", o.transparency)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceantransparency(&self) -> String {
                    self.video.ocean_settings.optics.transparency.to_string()
                }
                fn cvar_set_r_oceantransparency(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let v = value
                            .parse::<f32>()
                            .map_err(|_| "Expected a finite number".to_owned())?;
                        if !v.is_finite() {
                            return Err("Expected a finite number".to_owned());
                        }
                        let o = &mut self.video.ocean_settings.optics;
                        match name.to_ascii_lowercase().as_str() {
                            "r_oceantransparency" => o.transparency = v,
                            "r_oceandepthdarkening" => o.depth_darkening = v,
                            "r_oceanrefraction" => o.refraction = v,
                            "r_oceancaustics" => o.caustics = v,
                            _ => o.underwater_cull = v,
                        }
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANMAPSIZE {
        runtime: CvarDef::new(
                "r_oceanMapSize",
                "",
                "custom",
                "Controls r_oceanMapSize.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanmapsize(app)),
                |app, value| {
                    App::cvar_set_r_oceanmapsize(app, APP_R_OCEANMAPSIZE.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceanmapsize, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(v) = value.parse::<u32>() {
                        settings.ocean_settings.map_size = v;
                    }
                }),
                archive: Some((94, |state| {
                    let settings = state.video;
                    format!("{}", settings.ocean_settings.map_size)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanmapsize(&self) -> String {
                    self.video.ocean_settings.map_size.to_string()
                }
                fn cvar_set_r_oceanmapsize(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let requested = value
                            .trim()
                            .parse::<u32>()
                            .map_err(|_| format!("{name}: expected 128|256|512|1024"))?;
                        if !matches!(requested, 128 | 256 | 512 | 1024) {
                            return Err(format!("{name}: expected 128|256|512|1024"));
                        }
                        self.video.ocean_settings.map_size = requested;
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANMESHQUALITY {
        runtime: CvarDef::new(
                "r_oceanMeshQuality",
                "",
                "custom",
                "Controls r_oceanMeshQuality.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanmeshquality(app)),
                |app, value| {
                    App::cvar_set_r_oceanmeshquality(
                        app,
                        APP_R_OCEANMESHQUALITY.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_oceanmeshquality, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(v) = value.parse::<u8>() {
                        settings.ocean_settings.mesh_quality = v;
                    }
                }),
                archive: Some((95, |state| {
                    let settings = state.video;
                    format!("{}", settings.ocean_settings.mesh_quality)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanmeshquality(&self) -> String {
                    self.video.ocean_settings.mesh_quality.to_string()
                }
                fn cvar_set_r_oceanmeshquality(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ocean_settings.mesh_quality =
                            match value.trim().to_ascii_lowercase().as_str() {
                                "0" | "low" => 0,
                                "1" | "high" => 1,
                                _ => return Err(format!("{name}: expected 0|1 or low|high")),
                            };
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANNORMALSTRENGTH {
        runtime: CvarDef::new(
                "r_oceanNormalStrength",
                "",
                "custom",
                "Controls r_oceanNormalStrength.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceannormalstrength(app)),
                |app, value| {
                    App::cvar_set_r_oceannormalstrength(
                        app,
                        APP_R_OCEANNORMALSTRENGTH.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_oceannormalstrength, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(v) = value.parse::<f32>() {
                        if v.is_finite() {
                            settings.ocean_settings.normal_strength = v;
                        }
                    }
                }),
                archive: Some((98, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.ocean_settings.normal_strength)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceannormalstrength(&self) -> String {
                    format!("{:.3}", self.video.ocean_settings.normal_strength)
                }
                fn cvar_set_r_oceannormalstrength(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        self.video.ocean_settings.normal_strength = Self::parse_cvar_finite(name, value)?;
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANREFRACTION {
        runtime: CvarDef::new(
                "r_oceanRefraction",
                "",
                "0..0.15",
                "Wave distortion of transmitted scene color.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanrefraction(app)),
                |app, value| {
                    App::cvar_set_r_oceanrefraction(
                        app,
                        APP_R_OCEANREFRACTION.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_oceanrefraction, None),
        config: CvarBinding {
                load_video: Some(|settings, name, value, _context| {
                    if let Ok(v) = value.parse::<f32>() {
                        if v.is_finite() {
                            let o = &mut settings.ocean_settings.optics;
                            match name.to_ascii_lowercase().as_str() {
                                "r_oceantransparency" => o.transparency = v,
                                "r_oceandepthdarkening" => o.depth_darkening = v,
                                "r_oceanrefraction" => o.refraction = v,
                                "r_oceancaustics" => o.caustics = v,
                                _ => o.underwater_cull = v,
                            }
                            *o = o.sanitize();
                        }
                    }
                }),
                archive: Some((10416, |state| {
                    let settings = state.video;
                    let o = settings.ocean_settings.optics;
                    format!("{}", o.refraction)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanrefraction(&self) -> String {
                    self.video.ocean_settings.optics.refraction.to_string()
                }
                fn cvar_set_r_oceanrefraction(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        let v = value
                            .parse::<f32>()
                            .map_err(|_| "Expected a finite number".to_owned())?;
                        if !v.is_finite() {
                            return Err("Expected a finite number".to_owned());
                        }
                        let o = &mut self.video.ocean_settings.optics;
                        match name.to_ascii_lowercase().as_str() {
                            "r_oceantransparency" => o.transparency = v,
                            "r_oceandepthdarkening" => o.depth_darkening = v,
                            "r_oceanrefraction" => o.refraction = v,
                            "r_oceancaustics" => o.caustics = v,
                            _ => o.underwater_cull = v,
                        }
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANROUGHNESS {
        runtime: CvarDef::new(
                "r_oceanRoughness",
                "",
                "custom",
                "Controls r_oceanRoughness.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanroughness(app)),
                |app, value| {
                    App::cvar_set_r_oceanroughness(app, APP_R_OCEANROUGHNESS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceanroughness, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(v) = value.parse::<f32>() {
                        if v.is_finite() {
                            settings.ocean_settings.roughness = v;
                        }
                    }
                }),
                archive: Some((97, |state| {
                    let settings = state.video;
                    format!("{:.3}", settings.ocean_settings.roughness)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanroughness(&self) -> String {
                    format!("{:.3}", self.video.ocean_settings.roughness)
                }
                fn cvar_set_r_oceanroughness(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ocean_settings.roughness = Self::parse_cvar_finite(name, value)?;
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANSEASPRAY {
        runtime: CvarDef::new(
                "r_oceanSeaSpray",
                "",
                "custom",
                "Controls r_oceanSeaSpray.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanseaspray(app)),
                |app, value| {
                    App::cvar_set_r_oceanseaspray(app, APP_R_OCEANSEASPRAY.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceanseaspray, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.ocean_settings.sea_spray =
                        parse_bool(value).unwrap_or(settings.ocean_settings.sea_spray);
                }),
                archive: Some((99, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.ocean_settings.sea_spray))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanseaspray(&self) -> String {
                    bool_value(self.video.ocean_settings.sea_spray)
                }
                fn cvar_set_r_oceanseaspray(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ocean_settings.sea_spray = Self::parse_cvar_bool(name, value)?;
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANUNDERWATERCULL {
        runtime: CvarDef::new(
                "r_oceanUnderwaterCull",
                "",
                "0..32",
                "Cull geometry that water absorption hides, as an absorption-distance multiple (floored at 2): everything past it (min 256) when underwater, and anything under that much water when viewed from above. Zero disables.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanunderwatercull(app)),
                |app, value| {
                    App::cvar_set_r_oceanunderwatercull(
                        app,
                        APP_R_OCEANUNDERWATERCULL.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_oceanunderwatercull, None),
        config: CvarBinding {
                load_video: Some(|settings, name, value, _context| {
                    if let Ok(v) = value.parse::<f32>() {
                        if v.is_finite() {
                            let o = &mut settings.ocean_settings.optics;
                            match name.to_ascii_lowercase().as_str() {
                                "r_oceantransparency" => o.transparency = v,
                                "r_oceandepthdarkening" => o.depth_darkening = v,
                                "r_oceanrefraction" => o.refraction = v,
                                "r_oceancaustics" => o.caustics = v,
                                _ => o.underwater_cull = v,
                            }
                            *o = o.sanitize();
                        }
                    }
                }),
                archive: Some((10418, |state| {
                    let settings = state.video;
                    let o = settings.ocean_settings.optics;
                    format!("{}", o.underwater_cull)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanunderwatercull(&self) -> String {
                    self.video.ocean_settings.optics.underwater_cull.to_string()
                }
                fn cvar_set_r_oceanunderwatercull(
                    &mut self,
                    name: &str,
                    value: &str,
                ) -> Result<(), String> {
                    {
                        let v = value
                            .parse::<f32>()
                            .map_err(|_| "Expected a finite number".to_owned())?;
                        if !v.is_finite() {
                            return Err("Expected a finite number".to_owned());
                        }
                        let o = &mut self.video.ocean_settings.optics;
                        match name.to_ascii_lowercase().as_str() {
                            "r_oceantransparency" => o.transparency = v,
                            "r_oceandepthdarkening" => o.depth_darkening = v,
                            "r_oceanrefraction" => o.refraction = v,
                            "r_oceancaustics" => o.caustics = v,
                            _ => o.underwater_cull = v,
                        }
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANUPDATES {
        runtime: CvarDef::new(
                "r_oceanUpdates",
                "",
                "custom",
                "Controls r_oceanUpdates.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanupdates(app)),
                |app, value| {
                    App::cvar_set_r_oceanupdates(app, APP_R_OCEANUPDATES.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceanupdates, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    if let Ok(v) = value.parse::<f32>() {
                        if v.is_finite() {
                            settings.ocean_settings.updates_per_second = v;
                        }
                    }
                }),
                archive: Some((96, |state| {
                    let settings = state.video;
                    format!("{:.2}", settings.ocean_settings.updates_per_second)
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanupdates(&self) -> String {
                    format!("{:.3}", self.video.ocean_settings.updates_per_second)
                }
                fn cvar_set_r_oceanupdates(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ocean_settings.updates_per_second =
                            Self::parse_cvar_finite(name, value)?.clamp(0.0, 60.0);
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANWATERCOLOR {
        runtime: CvarDef::new(
                "r_oceanWaterColor",
                "",
                "custom",
                "Controls r_oceanWaterColor.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanwatercolor(app)),
                |app, value| {
                    App::cvar_set_r_oceanwatercolor(
                        app,
                        APP_R_OCEANWATERCOLOR.entry.name,
                        &value.to_string(),
                    )
                },
            )
            .with_text_setter(App::cvar_set_r_oceanwatercolor, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.ocean_settings.water_color =
                        parse_vec3(value).unwrap_or(settings.ocean_settings.water_color);
                }),
                archive: Some((101, |state| {
                    let settings = state.video;
                    format!(
                        "{:.4} {:.4} {:.4}",
                        settings.ocean_settings.water_color[0],
                        settings.ocean_settings.water_color[1],
                        settings.ocean_settings.water_color[2]
                    )
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanwatercolor(&self) -> String {
                    {
                        let c = self.video.ocean_settings.water_color;
                        format!("{:.4} {:.4} {:.4}", c[0], c[1], c[2])
                    }
                }
                fn cvar_set_r_oceanwatercolor(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ocean_settings.water_color = Self::parse_cvar_floats(name, value)?;
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANWINDFOAM {
        runtime: CvarDef::new(
                "r_oceanWindFoam",
                "",
                "custom",
                "Controls r_oceanWindFoam.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanwindfoam(app)),
                |app, value| {
                    App::cvar_set_r_oceanwindfoam(app, APP_R_OCEANWINDFOAM.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceanwindfoam, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.ocean_settings.wind_foam_streaks =
                        parse_bool(value).unwrap_or(settings.ocean_settings.wind_foam_streaks);
                }),
                archive: Some((100, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.ocean_settings.wind_foam_streaks))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanwindfoam(&self) -> String {
                    bool_value(self.video.ocean_settings.wind_foam_streaks)
                }
                fn cvar_set_r_oceanwindfoam(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ocean_settings.wind_foam_streaks = Self::parse_cvar_bool(name, value)?;
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANOPTICSCULL {
        runtime: CvarDef::new(
                "r_oceanOpticsCull",
                "",
                "0|1",
                "Skip the underwater optics capture entirely when the camera is dry and no ocean surface is on screen. Nothing can sample it then.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanopticscull(app)),
                |app, value| {
                    App::cvar_set_r_oceanopticscull(app, APP_R_OCEANOPTICSCULL.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceanopticscull, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.ocean_settings.optics.cull =
                        parse_bool(value).unwrap_or(settings.ocean_settings.optics.cull);
                }),
                archive: Some((10424, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.ocean_settings.optics.cull))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanopticscull(&self) -> String {
                    bool_value(self.video.ocean_settings.optics.cull)
                }
                fn cvar_set_r_oceanopticscull(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ocean_settings.optics.cull = Self::parse_cvar_bool(name, value)?;
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANOPTICSSCISSOR {
        runtime: CvarDef::new(
                "r_oceanOpticsScissor",
                "",
                "0|1",
                "Restrict the underwater optics capture to the screen rectangle the visible water covers instead of the whole framebuffer.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanopticsscissor(app)),
                |app, value| {
                    App::cvar_set_r_oceanopticsscissor(app, APP_R_OCEANOPTICSSCISSOR.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceanopticsscissor, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.ocean_settings.optics.scissor =
                        parse_bool(value).unwrap_or(settings.ocean_settings.optics.scissor);
                }),
                archive: Some((10425, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.ocean_settings.optics.scissor))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanopticsscissor(&self) -> String {
                    bool_value(self.video.ocean_settings.optics.scissor)
                }
                fn cvar_set_r_oceanopticsscissor(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ocean_settings.optics.scissor = Self::parse_cvar_bool(name, value)?;
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

    APP_R_OCEANSIMWHENHIDDEN {
        runtime: CvarDef::new(
                "r_oceanSimWhenHidden",
                "",
                "0|1",
                "Keep advancing the FFT wave simulation even while no water is visible. Off freezes the field, which costs nothing and cannot be seen.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceansimwhenhidden(app)),
                |app, value| {
                    App::cvar_set_r_oceansimwhenhidden(app, APP_R_OCEANSIMWHENHIDDEN.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceansimwhenhidden, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.ocean_settings.simulate_when_hidden =
                        parse_bool(value).unwrap_or(settings.ocean_settings.simulate_when_hidden);
                }),
                archive: Some((10426, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.ocean_settings.simulate_when_hidden))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceansimwhenhidden(&self) -> String {
                    bool_value(self.video.ocean_settings.simulate_when_hidden)
                }
                fn cvar_set_r_oceansimwhenhidden(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ocean_settings.simulate_when_hidden = Self::parse_cvar_bool(name, value)?;
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }


    APP_R_OCEANOPTICSMASKS {
        runtime: CvarDef::new(
                "r_oceanOpticsMasks",
                "",
                "0|1",
                "Honour water outline masks in the underwater optics capture. Off is visually wrong on hollow water; it exists to measure the mask cost.",
                CvarType::String,
                CvarFlags::ARCHIVE,
                LatchScope::None,
                |app| CvarValue::String(App::cvar_get_r_oceanopticsmasks(app)),
                |app, value| {
                    App::cvar_set_r_oceanopticsmasks(app, APP_R_OCEANOPTICSMASKS.entry.name, &value.to_string())
                },
            )
            .with_text_setter(App::cvar_set_r_oceanopticsmasks, None),
        config: CvarBinding {
                load_video: Some(|settings, _name, value, _context| {
                    settings.ocean_settings.optics.masks =
                        parse_bool(value).unwrap_or(settings.ocean_settings.optics.masks);
                }),
                archive: Some((10427, |state| {
                    let settings = state.video;
                    format!("{}", u8::from(settings.ocean_settings.optics.masks))
                })),
                ..CvarBinding::NONE
            },
        implementations: [
            impl App {
                fn cvar_get_r_oceanopticsmasks(&self) -> String {
                    bool_value(self.video.ocean_settings.optics.masks)
                }
                fn cvar_set_r_oceanopticsmasks(&mut self, name: &str, value: &str) -> Result<(), String> {
                    {
                        self.video.ocean_settings.optics.masks = Self::parse_cvar_bool(name, value)?;
                        self.commit_ocean_settings();
                    }
                    Ok(())
                }
            }
        ]
    }

}
