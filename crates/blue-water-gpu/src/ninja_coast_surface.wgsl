// Host bridge for the private Fluid Ninja Coast material (Water_Dense_Sea /
// NinjaLive_Water_100m, MI_Water_DENSE_Coast_shiny_cyan_wavetile_v2). The
// private material owns WPO and surface attributes; the existing UE SLW port
// owns optics. Native mesh, FFT, camera and footprint admission are host inputs.
struct NinjaCoastHost {
    domain: vec4<f32>, // patch centre in native metres xz, full extent m, absolute plane m
    config: vec4<f32>, // FFT on, RVT clamp (UE cm), solver SimZ datum (UE cm), debug view
    mesh: vec4<f32>,   // x: flat output mesh Z (UE cm, pawn-attached system position); yz: swell travel (render x, z);
                       // w: host options, bit 1 r_ninjaCoastTileMap 0, bit 2 r_ninjaCoastTileNormals 0, bit 4 r_ninjaCoastWakeFoam
    look: vec4<f32>,   // rgb: Hybrid water color / default (scattering tint), a: Hybrid transparency (extinction divisor)
    // Host LOD A/B (all 0 = authored): x r_ninjaCoastFarLod start m (far material
    // past 1.5x), y r_ninjaCoastFarDetail, z r_ninjaCoastSSRDistance m, w r_ninjaCoastCostProbe bits.
    lod: vec4<f32>,
    // x r_ninjaCoastFftDistance m (full FFT slope fades out by it; 0 = unlimited),
    // y r_ninjaCoastFftLite (what replaces it past there: 0 nothing, 1-3 lite slope),
    // z r_ninjaCoastDeepWater (> 0: water past the shore band drawn by `deep_fragment`; JKA_NINJA_OPEN_SEA),
    // w r_ninjaCoastCostProbe2 bits (per-operation ablation; see the cvar).
    lod2: vec4<f32>,
    // x r_ninjaCoastShoreDepth in UE cm (0 = sim detail over all water), y r_ninjaCoastOpenFoam strength.
    shore: vec4<f32>,
    detail: vec4<f32>, // shore blend endpoint, FFT detail, Coast foam lace, additive deep red
};
@group(4) @binding(0) var<uniform> ninja_host: NinjaCoastHost;
@group(4) @binding(1) var ninja_vd: texture_2d<f32>;
@group(4) @binding(2) var ninja_pd: texture_2d<f32>;
@group(4) @binding(3) var ninja_painter: texture_2d<f32>;
@group(4) @binding(4) var ninja_terrain: texture_2d<f32>;
@group(4) @binding(5) var ninja_flow: texture_2d<f32>;
@group(4) @binding(6) var ninja_rough: texture_2d<f32>;
@group(4) @binding(7) var ninja_tile1: texture_2d<f32>;
@group(4) @binding(8) var ninja_tile2: texture_2d<f32>;
@group(4) @binding(9) var ninja_clamp: sampler;
@group(4) @binding(10) var ninja_repeat: sampler;
@group(4) @binding(11) var ninja_gf: texture_2d<f32>;
@group(4) @binding(12) var ninja_capture: texture_2d<f32>;
@group(4) @binding(13) var ninja_wet_noise: texture_2d<f32>; // T_Noise3_128px (MF_WetMapFunction TextureSample_1)
@group(4) @binding(14) var ninja_caustics: texture_3d<f32>; // CausticsTileComponentVolumeTexture (T_Caustics_Animated_02)
// r_ninjaCoastDebug: unlit views of the simulation inputs/outputs.
fn ninja_debug(uv: vec2<f32>, mode: u32) -> vec3<f32> {
    let vd = textureSampleLevel(ninja_vd, ninja_clamp, uv, 0.0);
    let pd = textureSampleLevel(ninja_pd, ninja_clamp, uv, 0.0);
    let cap = textureSampleLevel(ninja_capture, ninja_clamp, uv, 0.0);
    let bed = textureSampleLevel(ninja_terrain, ninja_clamp, uv, 0.0);
    switch mode {
        case 1u: { return vec3<f32>(saturate(vd.b), saturate(vd.a), 0.0); }
        case 2u: { return vec3<f32>(0.5 + 0.25 * vd.r, 0.5 + 0.25 * vd.g, 0.5); }
        case 3u: { return vec3<f32>(saturate(pd.r * 0.25), saturate(pd.g * 4.0), saturate(-pd.g * 4.0)); }
        case 4u: { return vec3<f32>(0.5 + 0.5 * cap.x, 0.5 + 0.5 * cap.y, saturate(cap.w)); }
        default: {
            if bed.w < 0.5 { return vec3<f32>(1.0, 0.0, 1.0); }
            return select(vec3<f32>(0.0, 0.1, saturate(-bed.x / 500.0)), vec3<f32>(0.1, saturate(bed.x / 200.0), 0.0), bed.x >= 0.0);
        }
    }
}
fn ninja_uv(xz: vec2<f32>) -> vec2<f32> {
    let uv = (xz - ninja_host.domain.xy) / ninja_host.domain.z;
    return vec2<f32>(uv.x, -uv.y) + 0.5;
}
fn ninja_domain_share(uv: vec2<f32>) -> f32 {
    // Host finite-domain handoff, not an authored FNL material parameter.
    return 1.0 - smoothstep(0.45, 0.5, max(abs(uv.x - 0.5), abs(uv.y - 0.5)));
}
// Sim weight of a point (native metres, plane-relative): the window edge
// handoff times a fade by 3D distance from the eye. The 100 m window follows
// the camera in XY only (UE's rides on the pawn), so without this its square
// showed under a high camera; with it the sim contribution blends out
// radially (25-45 m, inside the 50 m half extent) and is gone when the eye is
// that far from the water, leaving the passive surface everywhere.
const NINJA_COAST_EYE_FADE_M: vec2<f32> = vec2<f32>(25.0, 45.0);
fn ninja_share(uv: vec2<f32>, world: vec3<f32>) -> f32 {
    let eye = 1.0 - smoothstep(NINJA_COAST_EYE_FADE_M.x, NINJA_COAST_EYE_FADE_M.y, distance(world, camera.position));
    let share = ninja_domain_share(uv) * eye;
    if share <= 0.0 { return 0.0; }
    return share * ninja_shore_weight(uv);
}
// Host option r_ninjaCoastShoreDepth: the sim's surface detail (ripples, foam, swash WPO,
// wet sand) only over a captured bed shallower than this; unknown bed is open sea.
// A 50% band past the depth fades it out.
// Anywhere (not only in the sim window): the window's bed raster inside it, the
// Blue Water shore depth outside (r_blueWaterCoast; without it, open sea).
fn ninja_shore_weight_at(uv: vec2<f32>, xz: vec2<f32>) -> f32 {
    let depth_cm = ninja_host.shore.x;
    if depth_cm <= 0.0 { return 1.0; }
    if all(uv > vec2<f32>(0.0)) && all(uv < vec2<f32>(1.0)) {
        let bed = textureSampleLevel(ninja_terrain, ninja_clamp, uv, 0.0);
        if bed.w > 0.5 { return 1.0 - smoothstep(depth_cm, depth_cm * ninja_host.detail.x, -bed.x); }
    }
    if waves.shore.x < 0.5 { return 0.0; }
    let depth = shore_at(xz).depth * 100.0;
    return 1.0 - smoothstep(depth_cm, depth_cm * ninja_host.detail.x, depth);
}
fn ninja_shore_weight(uv: vec2<f32>) -> f32 {
    let depth_cm = ninja_host.shore.x;
    if depth_cm <= 0.0 { return 1.0; }
    let bed = textureSampleLevel(ninja_terrain, ninja_clamp, uv, 0.0);
    if bed.w < 0.5 { return 0.0; }
    return 1.0 - smoothstep(depth_cm, depth_cm * ninja_host.detail.x, -bed.x);
}
// Sky admission for a point `height_m` native metres above the plane.
// `jka_water_sky_admission` compares the rain roof field (first hit from the
// sky, nearest texel) with the flat plane, so on a beach the sand itself reads
// as a roof once it rises 8 units above the plane: Coast swash and wet sand
// lost sun and ambient in blocky texel steps along that contour (the item-3
// "waterline stair steps"). Compare with the local surface instead; the 40
// unit (1 m) slack absorbs the nearest texel's slope step while umbrellas,
// decks and roofs (well above that) still shade.
fn ninja_coast_sky_admission(grid: vec2<f32>, height_m: f32) -> f32 {
    if jka_boundary.sky_uv.z <= 0.0 { return 1.0; }
    let uv = (grid * jka_boundary.scale.x - jka_boundary.sky_uv.xy) * jka_boundary.sky_uv.zw;
    if any(uv < vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0)) { return 1.0; }
    let roof = textureLoad(jka_sky_roof, vec2<i32>(uv * vec2<f32>(textureDimensions(jka_sky_roof))), 0).r;
    let ground = jka_boundary.sky_plane.x + max(height_m, 0.0) * jka_boundary.scale.x;
    return select(1.0, 0.0, roof > ground + 40.0);
}
// Coast admission. The authored footprint (`jka_water_contains`) is the map's
// water brushes; inside the patch the Coast surface follows the captured bed
// and its swash runs past that outline. Inside the patch, a texel whose bed
// lies in the run-up band (bed <= 150 cm above the clamp, the capture's mask-G
// run-up limit) under open sky is admitted too; the swash then ends where the
// terrain-following surface sinks under the sand (depth test) or thins out.
// CPU pruning keeps mesh cells within `COAST_ADMISSION_MARGIN` of the footprint.
const NINJA_COAST_RUNUP_CM: f32 = 150.0;
// Scene-light calibration for the Coast diffuse terms (see the fragment).
const NINJA_COAST_SCENE_LIGHT: f32 = 0.25;
// Column behind open-sea pixels (sky / no receiver), UE cm: opaque at any
// r_hybridTransparency, finite so behind_z and exp() stay well defined.
const NINJA_COAST_OPEN_DEEP_CM: f32 = 1.0e6;
// Bed height above the plane (UE cm) past which no water reaches: the host
// run-up limit (collision.w = 2) caps swash at 100-150 cm.
const NINJA_COAST_DRY_BED_CM: f32 = 250.0;
// Dry vertices sink this far below the plane (UE cm).
const NINJA_COAST_DRY_SINK_CM: f32 = 100.0;
struct NinjaCoastBehind { depth: f32, weights: vec4<f32>, received: bool }
// Scene depth behind the water at `screen` (UE cm). Sky (cleared reversed-Z
// depth) is not a receiver: an open-sea column, as Hybrid's OPEN_DEEP;
// linearised it read as a floor on the far plane and the sky showed through
// as a cyan band at a fixed view depth. When every texel lies in front of the
// water (sky walls with water continued past them) it is open sea too.
fn ninja_coast_behind(screen: vec2<f32>, size: vec2<f32>, pixel_depth: f32, open_deep: f32) -> NinjaCoastBehind {
    let raw = textureGather(scene_depth, wake_sampler, screen);
    let linear = select(100.0 * camera.near * camera.far / (raw * (camera.far - camera.near) + camera.near),
        vec4<f32>(open_deep), raw <= vec4<f32>(0.0));
    let gathered = river_slw_bilateral(screen, size, linear, vec4<f32>(1.0), pixel_depth, 0.0);
    let weights = gathered.weights * gathered.refraction_mask;
    let received = dot(weights, vec4<f32>(1.0)) > 0.0;
    return NinjaCoastBehind(select(open_deep, gathered.scene_depth, received), weights, received);
}
fn ninja_coast_contains(grid: vec2<f32>) -> bool {
    if jka_water_contains(grid) { return true; }
    let uv = ninja_uv(grid);
    if any(uv <= vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0)) { return false; }
    let bed = textureSampleLevel(ninja_terrain, ninja_clamp, uv, 0.0);
    return bed.w > 0.5 && bed.x <= NINJA_COAST_RUNUP_CM
        && ninja_coast_sky_admission(grid, bed.x * 0.01) > 0.5;
}
fn ninja_frame() -> NinjaCoastFrame {
    return NinjaCoastFrame(ninja_host.config.y, ninja_host.config.z, ninja_coast_swell_rotation());
}
// Rotation (cos, sin) taking UE's authored tile-1 travel direction, (1, -1)/sqrt2
// in UE XY (the pan (-.03, .03) moves features along -pan), onto the authored
// swell (r_blueWaterDirection via the host uniform: render (x, z) travel
// vector; UE Y = -render Z). waves.wind is not stepped in r_hybridMaterial 5.
fn ninja_coast_swell_rotation() -> vec2<f32> {
    let travel = ninja_host.mesh.yz;
    let w = vec2<f32>(travel.x, -travel.y);
    if dot(w, w) < 1e-8 { return vec2<f32>(1.0, 0.0); }
    let s = normalize(w);
    let r = vec2<f32>(0.70710678, -0.70710678);
    return vec2<f32>(dot(r, s), r.x * s.y - r.y * s.x);
}

// Native plane-relative metres (x, y up, z = -JKA y) to the UE reference frame.
fn ninja_cm(p: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(p.x * 100.0, -p.z * 100.0, p.y * 100.0 + ninja_host.config.y);
}
fn ninja_displacement(grid: vec2<f32>, spacing: f32) -> vec3<f32> {
    var d = vec3<f32>(0.0);
    let uv0 = ninja_uv(grid);
    let share0 = ninja_domain_share(uv0);
    // r_ninjaCoastCostProbe 2048 (diagnostic): flat mesh, no FFT displacement reads.
    if ninja_host.config.x > 0.5 && (u32(ninja_host.lod.w + 0.5) & 2048u) == 0u {
        let raw = displacement(grid, spacing);
        let scale = jka_crest_scale_from_bounds(jka_crest_bounds, 1.0, 0.0, 3.0);
        // Host composition: FFT swell does not run up dry bed inside the patch.
        let bed = textureSampleLevel(ninja_terrain, ninja_clamp, uv0, 0.0);
        let wet = mix(1.0, select(1.0, smoothstep(0.0, 100.0, -bed.x), bed.w > 0.5), share0);
        d = vec3<f32>(raw.x * scale, raw.y, raw.z * scale) * wet;
    }
    let p = vec3<f32>(grid.x + d.x, d.y, grid.y + d.z);
    let uv = ninja_uv(p.xz);
    // Everything below is scaled by the sim share: past the eye fade / window it adds
    // exactly 0, so skip its texture reads (most of the mesh from a distance).
    let share = ninja_share(uv, p);
    if share <= 0.0 { return d; }
    let f = ninja_frame();
    // UE evaluates WPO on the undisplaced output mesh at the system position,
    // which rides on the pawn (ninja_coast.rs `mesh_z_cm`).
    let mesh = vec3<f32>(p.x * 100.0, -p.z * 100.0, ninja_host.mesh.x);
    let wpo = ninja_coast_wpo_cm(ninja_vd, ninja_pd, ninja_painter, ninja_terrain, ninja_tile1, ninja_tile2,
        ninja_clamp, ninja_repeat, uv, mesh, ninja_cm(camera.position), camera.time, f,
        (u32(ninja_host.mesh.w + 0.5) & 1u) == 0u);
    var rise = mesh.z + wpo.z - f.clamp_cm;
    // Host: EnableHeightAlignment drapes the dry sheet onto HeightField, the
    // captured bed. UE's bed is a smooth Landscape; ours includes buildings,
    // so cells straddling a roof edge sloped through the walls (costadelsol
    // inland houses). Known bed above the swash run-up limit holds no water:
    // sink those vertices below the plane, under every dry surface.
    let bed = textureSampleLevel(ninja_terrain, ninja_clamp, uv, 0.0);
    if bed.w > 0.5 && bed.x > NINJA_COAST_DRY_BED_CM {
        rise = min(rise, -NINJA_COAST_DRY_SINK_CM);
    }
    return d + vec3<f32>(wpo.x, rise, -wpo.y) * (0.01 * share);
}
// `wave_surface` (vendor surface.wgsl) restricted to the slope Coast reads, calm 0:
// the same expressions in the same order without the foam, bubble, surf and
// variance reads, and a cascade whose detail weight is 0 (footprint past its
// last level) is not read: it adds exactly 0 to the slope and strain.
// Lite far variant (lossy, r_ninjaCoastFftLite): `cap` > 0 limits the
// anisotropy to cap:1 by widening the minor footprint axis (as a lower sampler
// clamp would; the cascades' detail fade follows the wider footprint), and
// `with_strain` false skips the extras reads (Jacobian of the horizontal map = 1).
fn ninja_coast_wave_slope_grad(xz: vec2<f32>, at: vec2<f32>, cap_in: f32, strain_in: bool, probe2: u32,
        gradient_x: vec2<f32>, gradient_y: vec2<f32>, fft_detail: f32) -> vec2<f32> {
    // r_ninjaCoastCostProbe2 (diagnostic): 1 isotropic reads, 2 no strain reads, 4 one scatter copy,
    // 8/16/32 skip cascade 0/1/2, 64 no ripple read, 128 no gust noise, 256 no shore lookup.
    var cap = cap_in;
    if (probe2 & 1u) != 0u { cap = 1.0; }
    let with_strain = strain_in && (probe2 & 2u) == 0u;
    var ddx = gradient_x;
    var ddy = gradient_y;
    var aniso = ANISOTROPY;
    if cap > 0.0 {
        aniso = cap;
        let lx = length(ddx);
        let ly = length(ddy);
        if lx > ly * cap { ddy = ddy * (lx / max(ly * cap, 1e-9)); }
        else if ly > lx * cap { ddx = ddx * (ly / max(lx * cap, 1e-9)); }
    }
    let across = length(ddx);
    let down = length(ddy);
    let footprint = max(min(across, down), max(across, down) / aniso);
    let wide = max(across, down);
    var gain = 1.0;
    if (probe2 & 128u) == 0u { gain = gust_gain(xz, wide); }
    let top = waves.info.x;
    var shore = Shore(vec4<f32>(1.0), 1.0e4, 0.0);
    if (probe2 & 256u) == 0u { shore = shore_at(xz); }
    var hulls = HullSurface(vec2<f32>(0.0), vec3<f32>(0.0), vec2<f32>(0.0), 0.0);
    if hull_sea_active() {
        hulls = hull_surface(xz, at, footprint, shore);
    }
    var slope = vec2<f32>(0.0);
    var strain = vec3<f32>(0.0);
    for (var i = 0; i < CASCADES; i++) {
        let level = log2(footprint * waves.texels[i]);
        let detail = (1.0 - smoothstep(top - 1.0, top, level))
            * clamp(fft_detail * f32(CASCADES) - f32(i), 0.0, 1.0);
        if detail > 0.0 && ((probe2 >> (3u + u32(i))) & 1u) == 0u {
            let tile = waves.tile[i];
            var copies = scatter(xz, i);
            if (probe2 & 4u) != 0u { copies.weights = vec3<f32>(1.0, 0.0, 0.0); }
            let read_derivatives = copies_grad(field_derivatives, copies, i, ddx / tile, ddy / tile);
            let kept = shore.scales[i];
            let derivatives = mix_zero(read_derivatives, copies) * kept;
            var extras_x = 0.0;
            var strain_zw = vec2<f32>(0.0);
            if with_strain {
                let read_extras = copies_grad(field_extras, copies, i, ddx / tile, ddy / tile);
                extras_x = mix_zero(read_extras, copies).x * kept;
                strain_zw = derivatives.zw;
            }
            let roughened = 1.0 + (gain - 1.0) * waves.gust_shares[i];
            let stilled = 1.0 - 0.0 * waves.slick_shares[i];
            slope += derivatives.xy * detail * stilled * sqrt(roughened);
            strain += vec3<f32>(strain_zw, extras_x) * detail;
        }
        if i == 0 {
            slope += hulls.slope * detail;
            strain += hulls.strain * detail;
        }
    }
    let xx = strain.x + 1.0;
    let zz = strain.y + 1.0;
    let cross_strain = strain.z;
    let jacobian = xx * zz - cross_strain * cross_strain;
    let world = vec2<f32>(zz * slope.x - cross_strain * slope.y, xx * slope.y - cross_strain * slope.x) / max(jacobian, MIN_JACOBIAN);
    var ripple_slope = vec2<f32>(0.0);
    if CASCADES > 1 && (probe2 & 64u) == 0u {
        let finest = CASCADES - 1;
        let ripple_level = max(log2(footprint * waves.tail.w), 0.0);
        let ripple_shown = (1.0 - smoothstep(top - 1.0, top, ripple_level))
            * clamp(fft_detail * f32(CASCADES) - f32(finest), 0.0, 1.0);
        if ripple_shown > 0.0 {
            let ripple_uv = xz / waves.tail.z + 0.5 / waves.info.z;
            let read = textureSampleGrad(field_derivatives, field_sampler, ripple_uv, finest, ddx / waves.tail.z, ddy / waves.tail.z);
            ripple_slope = read.xy * ripple_shown * sqrt(gain) * shore.scales[finest];
        }
    }
    return world + ripple_slope + hulls.world;
}

@diagnostic(off, derivative_uniformity)
fn ninja_coast_wave_slope(xz: vec2<f32>, at: vec2<f32>, cap: f32, strain: bool, probe2: u32) -> vec2<f32> {
    return ninja_coast_wave_slope_grad(xz, at, cap, strain, probe2, dpdx(xz), dpdy(xz), ninja_host.detail.y);
}
// r_ninjaCoastFarDetail 0: flat far material (FFT slope only, mean roughness).
const NINJA_COAST_FAR_ROUGHNESS: f32 = 0.12;
fn ninja_coast_far_flat(sign_face: f32) -> NinjaCoastMaterial {
    return NinjaCoastMaterial(vec3<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0, 0.0, sign_face),
        NINJA_COAST_FAR_ROUGHNESS, 0.0, 0.5, 1.0, vec2<f32>(0.0), 0.0);
}
fn ninja_coast_mix_material(a: NinjaCoastMaterial, b: NinjaCoastMaterial, t: f32) -> NinjaCoastMaterial {
    return NinjaCoastMaterial(mix(a.base_color, b.base_color, t), mix(a.emissive, b.emissive, t),
        mix(a.normal, b.normal, t), mix(a.roughness, b.roughness, t), mix(a.opacity, b.opacity, t),
        mix(a.specular, b.specular, t), mix(a.refraction, b.refraction, t), mix(a.sim_velocity, b.sim_velocity, t),
        mix(a.tile_density, b.tile_density, t));
}
@diagnostic(off, derivative_uniformity)
fn ninja_coast_normal(input: VertexOut, probe: u32, probe2: u32, fft_share: f32, fft_lite: u32) -> vec3<f32> {
    var up = vec3<f32>(0.0, 1.0, 0.0);
    let statistics = fft_lite == 4u;
    if ninja_host.config.x > 0.5 && (probe & 4u) == 0u && (fft_share > 0.0 || (fft_lite > 0u && !statistics)) {
        var ws_slope = vec2<f32>(0.0);
        if fft_share > 0.0 {
            if (probe & 512u) != 0u { ws_slope = wave_surface(input.grid, 0.0, input.world.xz).slope; }
            else {
#ifdef JKA_NINJA_WAVE_CLIPMAP
                ws_slope = ninja_cached_shore_slope(input.grid, input.world.xz, probe2, dpdx(input.grid), dpdy(input.grid));
#else
                ws_slope = ninja_coast_wave_slope(input.grid, input.world.xz, 0.0, true, probe2);
#endif
            }
            ws_slope *= fft_share;
        }
        if fft_share < 1.0 && fft_lite > 0u && !statistics {
            // 1: 4:1 anisotropy; 2: 4:1 without strain reads; 3: 2:1 without strain reads.
            let lite = ninja_coast_wave_slope(input.grid, input.world.xz, select(4.0, 2.0, fft_lite >= 3u), fft_lite == 1u, probe2);
            ws_slope += lite * (1.0 - fft_share);
        }
        let uv0 = ninja_uv(input.grid);
        let domain = ninja_domain_share(uv0);
        var wet = 1.0;
        if domain > 0.0 {
            let bed = textureSampleLevel(ninja_terrain, ninja_clamp, uv0, 0.0);
            wet = mix(1.0, select(1.0, smoothstep(0.0, 100.0, -bed.x), bed.w > 0.5), domain);
        }
        up = normalize(vec3<f32>(-ws_slope.x * wet, 1.0, -ws_slope.y * wet));
    }
    return up;
}

// The near/far material split is per pixel; derivative-taking samples inside
// it are only wrong on quads straddling a band edge, where that side's weight is ~0.
@diagnostic(off, derivative_uniformity)
@fragment
fn fragment(input: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // r_ninjaCoastCostProbe 4096 (diagnostic): vertex + raster cost only.
    if (u32(ninja_host.lod.w + 0.5) & 4096u) != 0u { return vec4<f32>(0.0, 0.0, 0.0, 1.0); }
    let to_eye = camera.position - input.world;
    let v = normalize(to_eye);
    let p = ninja_cm(input.world);
    let eye = ninja_cm(camera.position);
    let uv = ninja_uv(input.world.xz);
    let pixel_depth = max(-(camera.view * vec4<f32>(curved(input.world), 1.0)).z * 100.0, 0.001);
    let sign_face = select(-1.0, 1.0, front);
    // Scene behind the water at this pixel (SceneDepthWithoutWater), gathered
    // before the discard so the caustics UV derivatives stay in uniform flow.
    let size = vec2<f32>(textureDimensions(scene_depth));
    let uv_min = 0.55 / size;
    let uv_max = 1.0 - 0.55 / size;
    let screen = clamp(input.clip.xy / size, uv_min, uv_max);
    let open_deep = pixel_depth + NINJA_COAST_OPEN_DEEP_CM;
    let probe = u32(ninja_host.lod.w + 0.5);
    let probe2 = u32(ninja_host.lod2.w + 0.5);
    let eye_m = length(to_eye);
    // r_ninjaCoastTileMap 0 / r_ninjaCoastTileNormals 0 as material lite bits.
    let options = u32(ninja_host.mesh.w + 0.5);
    // r_ninjaCoastDeepWater split: `deep_fragment` (its own pipeline, drawn first) shades the
    // front faces of water past the shore band; this pass shades only shore water and blends
    // over the deep pass across the band (alpha = shore weight). Off: Coast everywhere.
    let split = coast_deep_split(front);
    // Passive tile map only over shore water (r_ninjaCoastShoreDepth); off: no reads.
    // r_ninjaCoastOpenFoam (unsplit only): over open sea the Coast foam shows on the FFT crests
    // instead (upper part of the wave height; no tile map mask, no camera fades).
    let tile_on = (options & 1u) == 0u;
    let open_foam_on = !split && ninja_host.shore.y > 0.0 && ninja_host.config.x > 0.5 && front;
    var shore_w = 1.0;
    if tile_on || open_foam_on || split { shore_w = ninja_shore_weight_at(uv, input.grid); }
    if split && shore_w <= 0.0 { discard; }
    // UE shades the output mesh with its flat VertexNormal (WPO does not
    // rewrite normals). The host FFT (mode 4) tilts it by the smooth wave
    // slope with the same dry-bed attenuation as the displacement; no
    // per-triangle derivative normal (that faceted the open water).
    // Evaluated first, before the scene gathers and the material: only `up`
    // stays live across the rest, so the FFT reads do not compete with the
    // material's state for registers (same image).
    // r_ninjaCoastFftDistance: the full slope hands over to the lite one
    // (r_ninjaCoastFftLite; 0 = none) over the last quarter of the range.
    var fft_share = 1.0;
    if ninja_host.lod2.x > 0.0 { fft_share = 1.0 - smoothstep(ninja_host.lod2.x * 0.75, ninja_host.lod2.x, eye_m); }
    var fft_lite = u32(ninja_host.lod2.y + 0.5);
    if (probe & 1024u) != 0u { fft_share = 0.0; fft_lite = max(fft_lite, 1u); }
    let statistics = fft_lite == 4u;
#ifndef JKA_NINJA_LATE_FFT
    let up = ninja_coast_normal(input, probe, probe2, fft_share, fft_lite);
#endif
    var tile_gain = 0.0;
    if tile_on { tile_gain = shore_w; }
    var open_crest = 0.0;
    if open_foam_on {
        let hs = max(waves.sea.x, 0.05);
        // Crest band: from the mean level, full at 30% of the significant height (whole
        // wave fronts); r_ninjaCoastOpenFoam scales it to the tile map's foam density range.
        open_crest = smoothstep(0.0, 0.3 * hs, input.world.y) * (1.0 - shore_w) * ninja_host.shore.y;
    }
    let tile_lite = select(0u, 2048u, tile_gain <= 0.0) | select(0u, 256u, (options & 2u) != 0u)
        | select(0u, 4096u, (options & 4u) != 0u); // NINJA_COAST_LAKE_WAKE_FOAM
    var direct_pixel = NinjaCoastBehind(open_deep, vec4<f32>(0.0), false);
    if (probe & 8u) == 0u { direct_pixel = ninja_coast_behind(screen, size, pixel_depth, open_deep); }
    var far = 0.0;
    if ninja_host.lod.x > 0.0 { far = smoothstep(ninja_host.lod.x, ninja_host.lod.x * 1.5, eye_m); }
    if (probe & 2u) != 0u { far = 1.0; }
    let far_detail = u32(ninja_host.lod.y + 0.5);
    var far_attrs = ninja_coast_far_flat(sign_face);
    if far > 0.0 && far_detail > 0u && (probe & 2u) == 0u {
        // Past the sim fade share is 0; the lite material keeps the tile pattern.
        far_attrs = ninja_coast_shade(ninja_vd, ninja_pd, ninja_terrain, ninja_flow, ninja_rough, ninja_tile1,
            ninja_tile2, ninja_clamp, ninja_repeat, uv, p, eye, pixel_depth, camera.time, sign_face,
            ninja_frame(), 0.0, select(384u, 128u, far_detail >= 2u) | tile_lite, tile_gain, open_crest);
    }
    let share = ninja_share(uv, input.world);
    var attrs = far_attrs;
    if far < 1.0 {
        let near_attrs = ninja_coast_shade(ninja_vd, ninja_pd, ninja_terrain, ninja_flow, ninja_rough, ninja_tile1,
            ninja_tile2, ninja_clamp, ninja_repeat, uv, p, eye, pixel_depth, camera.time, sign_face,
            ninja_frame(), share, (probe & 384u) | ((probe2 >> 8u) & 1536u) | tile_lite, tile_gain, open_crest);
        attrs = ninja_coast_mix_material(near_attrs, far_attrs, far);
    }
#ifdef JKA_NINJA_LATE_FFT
    let up = ninja_coast_normal(input, probe, probe2, fft_share, fft_lite);
#endif
    let tangent = normalize(vec3<f32>(1.0, 0.0, 0.0) - up * up.x);
    let bitangent = cross(up, tangent);
    // MaterialTemplate.ush: WorldNormal *= TwoSidedSign after the tangent
    // transform (on top of the graph's own Multiply_168 flip). Without it the
    // back faces of folded swash fronts faced away from the eye (N.V <= 0,
    // Fresnel 1) and drew as hard white sky-reflection lines.
    let n = normalize(tangent * attrs.normal.x + bitangent * attrs.normal.y + up * attrs.normal.z) * sign_face;
    let caustics_in = ninja_coast_caustics_inputs(ninja_pd, ninja_clamp, uv, p, eye, pixel_depth,
        direct_pixel.depth, ninja_host.domain.z * 100.0, attrs, share);
    let caustics_dx = dpdx(caustics_in.flow_uv);
    let caustics_dy = dpdy(caustics_in.flow_uv);
    if !ninja_coast_contains(input.grid) { discard; }
    let debug = u32(ninja_host.config.w + 0.5);
    if debug >= 1u && debug <= 5u {
        return vec4<f32>(ninja_debug(uv, debug) * ninja_domain_share(uv), 1.0);
    }
    var rough = max(attrs.roughness, 0.001);
    if statistics && fft_share < 1.0 && ninja_host.config.x > 0.5 && (probe & 4u) == 0u {
        // The unresolved slope energy broadens the far lobe as the sampled
        // normals fade. Preserve the authored roughness at the start of the band.
        let scales = shore_at(input.grid).scales.xyz;
        let unresolved = dot(waves.slopes.xyz, scales * scales) * (1.0 - fft_share * fft_share);
        rough = min(1.0, sqrt(rough * rough + unresolved));
    }
    let coverage = saturate(attrs.opacity);
    let f0 = vec3<f32>(0.08 * attrs.specular);
    // probe2: 512 no scene colour reads, 1024 no cloud shadow, 2048 no sun shadow map, 4096 no sky
    // irradiance, 8192 no sky radiance, 16384 no BRDF LUT, 32768 no sky admission, 65536 no fog.
    var gf = vec2<f32>(0.5, 0.02);
    if (probe2 & 16384u) == 0u { gf = textureSampleLevel(ninja_gf, ninja_clamp, vec2<f32>(max(dot(n,v),0.0), rough), 0.0).rg; }
    let brdf = river_slw_env_brdf(f0, gf);
    let l = sky_env.light_direction;
    var admission = 1.0;
    if (probe2 & 32768u) == 0u { admission = ninja_coast_sky_admission(input.grid, input.world.y); }
    let sun = sky_env.light_color * sky_env.light_intensity * admission;
    var shadow = 1.0;
    if (probe & 16u) == 0u {
        if (probe2 & 2048u) == 0u { shadow = water_sun_shadow(input.world, u32(surface.shadows.x)); }
        if (probe2 & 1024u) == 0u { shadow *= cloud_shadow(cloud_map, cloud_sampler, cloud_params, input.world); }
    }
    var ambient = vec3<f32>(0.05) * admission;
    if (probe & 32u) == 0u && (probe2 & 4096u) == 0u { ambient = env_irradiance(sky_atlas, sky_sampler, sky_env, n) * admission; }
    // Host calibration (not a UE term): Blue Water's sun (~6 sea units at
    // noon) and sky irradiance are ~4x brighter than the light JKA's baked
    // lightmaps put on the same beach. UE lights sand and water with one model,
    // so its deep-water in-scatter is .26x the dry-sand luminance (PIE linear
    // capture, tools/coast/capture_coast_light.py), i.e. in-scattered light ~.85x
    // the sand's; ours was 1.9x the sand's at the open-sea view -3268 8172 6730. The diffuse
    // terms (SLW in-scatter, BaseColor x Opacity layer) take the scene's scale;
    // reflections and specular keep the sky's.
    let scene_light = NINJA_COAST_SCENE_LIGHT;
    let diffuse = attrs.base_color * coverage * scene_light;
    let direct = river_slw_direct(n, v, l, diffuse, f0, rough);
    // SLW refraction (DistortionCommon IndexOfRefraction) from the material's
    // Refraction attribute, as the river SLW port: offset the scene lookup by
    // the view normal; keep the undistorted sample when every distorted
    // texel lies in front of the water.
    // Zero distortion (ior exactly 1: the material's refraction fade reaches 0
    // past 40 m) reads the same texels as `direct_pixel`; skip that gather.
    var behind_pixel = direct_pixel;
    var behind_screen = screen;
    if (probe & 8u) == 0u && attrs.refraction != 1.0 {
        let view_normal = normalize((camera.view * vec4<f32>(n, 0.0)).xyz);
        let aspect = camera.viewport.x * camera.viewport.w;
        let distortion_params = vec4<f32>(1.0 / (aspect * camera.tan_half_fov_y), aspect, camera.viewport.x, camera.viewport.y);
        let distortion = river_slw_ior_uv_distortion(view_normal, attrs.refraction, distortion_params,
            direct_pixel.depth, pixel_depth);
        if any(distortion != vec2<f32>(0.0)) {
            let distorted_screen = clamp(screen + distortion, uv_min, uv_max);
            let distorted = ninja_coast_behind(distorted_screen, size, pixel_depth, open_deep);
            if distorted.received {
                behind_pixel = distorted;
                behind_screen = distorted_screen;
            }
        }
    }
    let weights = behind_pixel.weights;
    // No receiver (sky / open-sea column): every weight is 0, so the colour
    // gathers would contribute exactly 0.
    var behind = vec3<f32>(0.0);
    if behind_pixel.received && (probe2 & 512u) == 0u {
        behind = vec3<f32>(dot(textureGather(0, scene_color, wake_sampler, behind_screen), weights),
            dot(textureGather(1, scene_color, wake_sampler, behind_screen), weights),
            dot(textureGather(2, scene_color, wake_sampler, behind_screen), weights));
    }
    let behind_depth = behind_pixel.depth;
    let behind_z = eye.z + (p.z - eye.z) * behind_depth / pixel_depth;
    let c = ninja_coast_volume();
    // WATER ColorScale: ScaleColorBehindWater 1.1 plus the caustics branch.
    let color_scale = ninja_coast_color_scale(ninja_caustics, ninja_repeat, caustics_in, attrs, share,
        camera.time, caustics_dx, caustics_dy);
    // Hybrid Water color / Transparency (host controls; defaults leave UE's coefficients).
    let clarity = max(ninja_host.look.w, 0.1);
    let coeff = RiverSlwCoefficients(c.scattering * ninja_host.look.rgb / clarity, c.absorption / clarity, c.phase, color_scale);
    var vol_luminance = vec3<f32>(0.0);
    if (probe & 64u) == 0u {
        vol_luminance = river_slw_volume(coeff, pixel_depth, behind_depth, behind, 1.0, p.z, behind_z,
            v, n, river_slw_ior(attrs.specular), l, sun * scene_light, ambient * scene_light, brdf, shadow, vec3<f32>(1.0), 1.0-coverage).luminance;
    }
    let ray = reflect(-v,n);
    var sky = vec3<f32>(0.1) * admission;
    if (probe & 32u) == 0u && (probe2 & 8192u) == 0u { sky = env_radiance(sky_atlas, sky_sampler, sky_env, ray, rough) * admission; }
    var reflected = sky;
#ifdef JKA_NINJA_SSR
    // r_ninjaCoastSSRDistance: trace fades out over the last quarter of the range.
    var ssr_share = 1.0;
    if ninja_host.lod.z > 0.0 { ssr_share = 1.0 - smoothstep(ninja_host.lod.z * 0.75, ninja_host.lod.z, eye_m); }
    if (probe & 1u) != 0u { ssr_share = 0.0; }
    if ssr_share > 0.0 {
        let trace = screen_space_reflection(scene_color, wake_sampler, scene_depth,
            (camera.view * vec4<f32>(curved(input.world),1.0)).xyz,
            (camera.view * vec4<f32>(ray,0.0)).xyz, surface.reflections.x > 0.5,
            surface.reflections.y, REFLECTION_STEPS, (camera.view * vec4<f32>(n,0.0)).xyz, rough, TRACE_BELOW);
        reflected = mix(sky, trace.color, trace.confidence * ssr_share);
    }
#endif
    let layer = diffuse * ambient / 3.141592653589793 + direct.diffuse * sun * shadow + attrs.emissive;
    let color = layer + direct.specular * sun * shadow + vol_luminance + reflected * brdf;
    // r_ninjaCoastDebug 6..8: lit components, same fog path as the surface.
    if debug == 6u { return vec4<f32>(jka_surface_legacy_fog(apply_fog(vol_luminance,input.world),input.world),1.0); }
    if debug == 7u { return vec4<f32>(jka_surface_legacy_fog(apply_fog(layer,input.world),input.world),1.0); }
    if debug == 8u {
        return vec4<f32>(saturate((behind_depth - pixel_depth) / 2000.0),
            select(0.0, 1.0, dot(weights, vec4<f32>(1.0)) > 0.0), saturate(coverage * 4.0), 1.0);
    }
    var final_color = color;
    if (probe2 & 65536u) == 0u { final_color = jka_surface_legacy_fog(apply_fog(color,input.world),input.world); }
    return vec4<f32>(final_color, select(1.0, saturate(shore_w), split));
}
// r_ninjaCoastDeepWater > 0 (JKA_NINJA_OPEN_SEA builds): front faces in the shore band
// blend over the deep pass. Debug views and back faces stay Coast-only and opaque.
fn coast_deep_split(front: bool) -> bool {
#ifdef JKA_NINJA_OPEN_SEA
    return ninja_host.lod2.z > 0.0 && front && ninja_host.config.w < 0.5;
#else
    return false;
#endif
}
#ifdef JKA_NINJA_OPEN_SEA
fn ninja_deep_visible(input: VertexOut, front: bool) -> bool {
    if !coast_deep_split(front) { return false; }
    let probe = u32(ninja_host.lod.w + 0.5);
    if (probe & 8192u) != 0u { return true; }
    return ninja_shore_weight_at(ninja_uv(input.world.xz), input.grid) < 1.0
        && ninja_coast_contains(input.grid);
}

#ifdef JKA_NINJA_REDUCED
fn ninja_coarse_scene_pixel(pixel: vec2<f32>) -> vec2<i32> {
    let full = vec2<i32>(camera.viewport.xy);
    let coarse = (full + 1) / 2;
    return min(vec2<i32>(pixel * camera.viewport.xy / vec2<f32>(coarse)), full - 1);
}
#endif

// Deep-water pass (r_ninjaCoastDeepWater): its own pipeline, drawn before the Coast pass, so
// the compact adaptive Hybrid material keeps its own register budget (inside the Coast
// shader it ran at Coast's occupancy and saved only ~6%). Front faces with any open-sea
// share; the Coast pass covers the shore band and everything else.
@diagnostic(off, derivative_uniformity)
@fragment
fn deep_fragment(input: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // r_ninjaCoastCostProbe (diagnostic): 8192 skip the shore/coverage gates, 16384 return
    // flat colour after them (gate cost only).
    let probe = u32(ninja_host.lod.w + 0.5);
    if !ninja_deep_visible(input, front) { discard; }
#ifdef JKA_NINJA_ADAPTIVE
    if ninja_reconstructible(input) { return ninja_reconstruct(input); }
#endif
#ifdef JKA_NINJA_REDUCED
    if input.clip.z < textureLoad(scene_depth, ninja_coarse_scene_pixel(input.clip.xy), 0) { discard; }
#endif
    if (probe & 16384u) != 0u { return vec4<f32>(0.0, 0.0, 0.1, 1.0); }
#ifdef JKA_NINJA_ADAPTIVE
    return ninja_adaptive_debug(jka_fast_fragment(input, front), vec3<f32>(0.9, 0.05, 0.35));
#else
    return jka_fast_fragment(input, front);
#endif
}
// Colour match of the compact Hybrid water body (upwelling light, composed into
// `jka_fast_fragment`) to the Coast SLW open water: linear gain from same-launch
// costadelsol open-sea shots (foam-free median, 2026-11-22). 1 = Blue Water's own colour.
// Iterated through the tonemapper: (2.1, 3.4, 2.2) left the screen 2.0/2.4/1.55x short,
// (4.4, 8.1, 3.4) 1.9/1.54/1.15x (open sea; the user viewpoint already matched).
const NINJA_COAST_DEEP_TINT: vec3<f32> = vec3<f32>(8.4, 12.5, 3.9);
#endif
// Landscape receiver: FNL Water_Dense_Sea draws its wet sand with the landscape
// material MI_Sandy_SeaCoastMountain_Composite1_WetMapped_v2, whose
// MF_WetMapFunction reads the water sim's RT_VelocityDensity
// (NinjaVelocityDensityBuffer): BaseColor *= Color, Roughness = min(base, R),
// MI WetMapDarkening .7, WaterEdgeSoftness 1, switches SimpleMode /
// KillWetmapUnderWater / SubtractDensityFromWetmap off. JKA keeps baked
// lighting, so this pass modulates the lit opaque scene: out = add + dst * mul
// (blend One, SrcAlpha). `mul` is the Color output; `add` is the sky
// reflection the wet roughness adds over the dry surface (host: JKA receivers
// have no roughness, the dry sand is taken as fully rough). Caustics output and
// the separate NinjaLive_Area_Sand SURFACE sim are not part of this pass.
@vertex
fn ninja_wet_vertex(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(p[i], 0.0, 1.0);
}
fn ninja_wet_env(n: vec3<f32>, v: vec3<f32>, rough: f32) -> vec3<f32> {
    let gf = textureSampleLevel(ninja_gf, ninja_clamp, vec2<f32>(max(dot(n, v), 0.0), rough), 0.0).rg;
    let ray = reflect(-v, n);
    // MI Specular .5 -> F0 .04 (UE DefaultLit 0.08 * Specular).
    return env_radiance(sky_atlas, sky_sampler, sky_env, ray, rough) * (0.04 * gf.x + gf.y);
}
@fragment
fn ninja_wet_fragment(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let px = vec2<i32>(pixel.xy);
    let depth = textureLoad(scene_depth, px, 0);
    let ndc = pixel.xy * camera.viewport.zw;
    let clip = camera.inverse_view_projection * vec4<f32>(ndc.x * 2.0 - 1.0, 1.0 - ndc.y * 2.0, depth, 1.0);
    var world = clip.xyz / clip.w;
    let across = world.xz - camera.position.xz;
    world.y += camera.curvature * dot(across, across);
    // Derivatives precede every discard.
    let dx = dpdx(world);
    let dy = dpdy(world);
    if depth <= 0.0 { discard; }
    let uv = ninja_uv(world.xz);
    let share = ninja_share(uv, world);
    if share <= 0.0 { discard; }
    var n = cross(dx, dy);
    let nl = length(n);
    if nl < 1.0e-7 { discard; }
    n = n / nl;
    let v = normalize(camera.position - world);
    n = select(-n, n, dot(n, v) >= 0.0);
    // UE applies the function to the Landscape only: admit receivers on the
    // captured bed surface (bed.x is UE cm above the clamp; world.y is native
    // metres above the plane = clamp).
    let bed = textureSampleLevel(ninja_terrain, ninja_clamp, uv, 0.0);
    let above_bed = world.y * 100.0 - bed.x;
    let receiver = select(0.0, 1.0 - smoothstep(15.0, 40.0, abs(above_bed)), bed.w > 0.5) * share;
    if receiver <= 0.0 { discard; }
    let vd = textureSampleLevel(ninja_vd, ninja_clamp, uv, 0.0);
    let density = vd.b;
    let wet = vd.a;
    // Color (FunctionOutput_2): Max_3(Clamp_0(1 - wet, WetMapDarkening, 1),
    // Saturate_0(Power_6(1 - dot(VertexNormalWS, +Z), .35))).
    let slope = saturate(pow(max(1.0 - n.y, 0.0), 0.35));
    let color = max(clamp(1.0 - wet, 0.7, 1.0), slope);
    // Roughness (FunctionOutput_1): Max_2(T_Noise3.R(WorldPos.xy * .01 cm) * .2,
    // Max_1(1 - Min_0((1 - density)^WaterEdgeSoftness, wet), .05)).
    let world_cm = ninja_cm(world);
    let noise = textureSampleLevel(ninja_wet_noise, ninja_repeat, world_cm.xy * 0.01, 0.0).r;
    let rough = max(noise * 0.2, max(1.0 - min(1.0 - density, wet), 0.05));
    let gloss = max(ninja_wet_env(n, v, rough) - ninja_wet_env(n, v, 1.0), vec3<f32>(0.0))
        * ninja_coast_sky_admission(world.xz, world.y) * (1.0 - slope);
    return vec4<f32>(gloss * receiver, mix(1.0, color, receiver));
}
@fragment
fn ninja_wire(input: VertexOut) -> @location(0) vec4<f32> {
    if !ninja_coast_contains(input.grid) { discard; }
    return vec4<f32>(0.0, 0.9, 1.0, 1.0);
}
// Whitewater sprites ("PARTICLES 100m simdriven - whitewater"): the particle
// state is simulated by the private coast_whitewater_sim.wgsl in UE cm of the
// Coast frame. Sprite renderer: Alignment VelocityAligned, FacingMode
// FaceCamera (NiagaraSpriteVertexFactory: up = -velocity direction, right =
// facing x up), SpriteSize x along right, y along up, UV 0..1. The group-4
// layout of this pass differs from the surface's (binding 16/17 only).
struct NinjaWhitewaterDraw { clamp_cm: vec4<f32> } // x clamp (UE cm), y r_ninjaCoastDebug, z r_ninjaCoastWhitewaterOpacity
@group(4) @binding(16) var<storage, read> ninja_ww_particles: array<NinjaWhitewaterParticle>;
@group(4) @binding(17) var<uniform> ninja_ww_draw: NinjaWhitewaterDraw;
struct NinjaWhitewaterOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) world: vec3<f32>,
}
@vertex
fn ninja_ww_vertex(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> NinjaWhitewaterOut {
    let p = ninja_ww_particles[instance];
    var corners = array<vec2<f32>, 6>(vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0));
    let corner = corners[vertex];
    // UE cm (x, y, z up; z = clamp at the host plane) -> native metres (x, up, -y).
    let centre = vec3<f32>(p.position.x, p.position.z - ninja_ww_draw.clamp_cm.x, -p.position.y) * 0.01;
    let velocity = vec3<f32>(p.velocity.x, p.velocity.z, -p.velocity.y);
    let facing = normalize(camera.position - centre);
    let camera_up = (camera.inverse_view * vec4<f32>(0.0, 1.0, 0.0, 0.0)).xyz;
    let up = select(-velocity * inverseSqrt(max(dot(velocity, velocity), 1e-12)), camera_up, dot(velocity, velocity) < 1e-8);
    let right_raw = cross(facing, up);
    let right = select(normalize(right_raw), normalize(cross(facing, camera_up)), dot(right_raw, right_raw) < 1e-10);
    let size = p.sprite_size * 0.01;
    let world = centre + right * ((corner.x - 0.5) * size.x) + up * ((corner.y - 0.5) * size.y);
    return NinjaWhitewaterOut(world_to_clip(world), corner, p.color, world);
}
@fragment
fn ninja_ww_fragment(input: NinjaWhitewaterOut) -> @location(0) vec4<f32> {
    // Camera-facing sprites: TwoSidedSign is +1 (front) in UE.
    let distance_cm = distance(input.world, camera.position) * 100.0;
    // AtmosphericLightColor: the sun's illuminance on the ground (clamped to 0..1 by the material).
    let shade = ninja_ww_shade(input.color, input.uv, distance_cm, sky_env.light_color * sky_env.light_intensity, true);
    // r_ninjaCoastDebug 9: particle colour / 300; 10: light term / 4 (red = NaN); 11: emissive. Opaque.
    let debug = u32(ninja_ww_draw.clamp_cm.y + 0.5);
    if debug == 9u { return vec4<f32>(saturate(input.color.rgb / 300.0), 1.0); }
    if debug == 10u {
        let light = sky_env.light_color * sky_env.light_intensity;
        if any(light != light) { return vec4<f32>(1.0, 0.0, 0.0, 1.0); }
        return vec4<f32>(saturate(light / 4.0), 1.0);
    }
    if debug == 11u { return vec4<f32>(saturate(shade.emissive), 1.0); }
    if shade.opacity <= 0.0 { discard; }
    // Host approximation of the reference level's display of these HDR sprites,
    // inside this pass: PostProcess_Warm/Cold pre-exposure (Basic auto exposure,
    // min = max brightness 1 EV100 -> 1 / (1.2 * 2)) and UE's filmic tonemap,
    // applied to the sprite's own premultiplied contribution so its core rolls
    // off to white instead of clipping into a hard disc (JKA has no tonemap).
    let added = shade.emissive * (NINJA_WW_DISPLAY * shade.opacity);
    let shown = (added * (2.51 * added + 0.03)) / (added * (2.43 * added + 0.59) + 0.14);
    let coverage = saturate(max(shown.r, max(shown.g, shown.b)));
    if coverage <= 0.0 { discard; }
    let color = jka_surface_legacy_fog(apply_fog(shown / coverage, input.world), input.world);
    // Host control (r_ninjaCoastWhitewaterOpacity): scales the sprite's coverage only.
    let alpha = coverage * saturate(ninja_ww_draw.clamp_cm.z);
    if alpha <= 0.0 { discard; }
    return vec4<f32>(color, alpha);
}
// Measured display calibration (tools/coast/capture_whitewater_hdr.py, analysis/
// whitewater_hdr.py): in UE's linear HDR the sprites add ~1.2x the dry-sand
// luminance at the 99th percentile pixel and ~5x at the 99.9th. The level's
// pre-exposure (Basic AE, min = max 1 EV100 -> .417) alone left the sprite
// cores ~10x brighter than that here; .07 places a median sprite's peak near
// 1.2x our sand (JKA display-referred, no exposure).
const NINJA_WW_DISPLAY: f32 = 0.07;
