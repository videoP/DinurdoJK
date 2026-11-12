// Host presentation adapter: Hybrid/Godot submerged optics over the native
// solved river mesh. Above-water SLW and the hydraulic/foam producers are intact.

fn river_vertex_height(uv: vec2<f32>, eye: vec3<f32>) -> f32 {
    let local = vec3<f32>((uv - 0.5) * body.grid.xy, 0.0);
    let world = body.origin.xyz + local;
    return body.origin.z + river_live_grid_wpo(sim_grid, normals_rt, normal_sampler,
        ocean_detail, ocean_sampler, uv, world.xy, distance(world, eye),
        river_view.time.x, local, vec3<f32>(0.0, 0.0, 1.0), river_motion(river_view.time.z),
        river_wpo_advection(), river_wave_height(), false).z;
}

// Match the actual grid's a-b-c / c-d-a diagonal, including vertex WPO.
// Called in the vertex stage, so the submerged fullscreen pass only evaluates
// this three times, with no CPU/GPU readback or per-pixel wave reconstruction.
fn river_eye_water_at(eye: vec3<f32>) -> vec4<f32> {
    let uv = (eye.xy - body.origin.xy) / body.grid.xy + 0.5;
    if any(uv <= vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0)) {
        return vec4<f32>(0.0);
    }
    let f = river_live_field(sim_grid, uv);
    let masks = river_surface_masks(f.y, 1.0, uv, vec2<f32>(textureDimensions(sim_grid)), 1.0, 0.0, 3.0e38);
    let bed = body.origin.z + f.x - f.y;
    if masks.material_mask < 0.3333 || eye.z < bed {
        return vec4<f32>(0.0);
    }
    let cell = floor(uv * body.grid.zw);
    let fraction = uv * body.grid.zw - cell;
    let lo = cell / body.grid.zw;
    let hi = (cell + 1.0) / body.grid.zw;
    let c = river_vertex_height(lo, eye);
    let a = river_vertex_height(hi, eye);
    var top: f32;
    if fraction.x >= fraction.y {
        let b = river_vertex_height(vec2<f32>(hi.x, lo.y), eye);
        top = c * (1.0 - fraction.x) + b * (fraction.x - fraction.y) + a * fraction.y;
    } else {
        let d = river_vertex_height(vec2<f32>(lo.x, hi.y), eye);
        top = c * (1.0 - fraction.y) + d * (fraction.y - fraction.x) + a * fraction.x;
    }
    return vec4<f32>(top, bed, select(0.0, 1.0, eye.z < top), 0.0);
}

fn river_hybrid_underside(input: RiverOut, graph_normal: vec3<f32>, foam: RiverFoam) -> vec4<f32> {
    let n = -graph_normal;
    let v = normalize(river_view.eye.xyz - input.world);
    let no_v = max(dot(n, v), 0.00002);
    // Same water-to-air Fresnel and total internal reflection as Hybrid.
    let f0 = 0.08 * river_slw_parameters().shading.x;
    var fresnel = f0 + (1.0 - f0) * pow(1.0 - no_v, 5.0);
    if 1.333 * 1.333 * (1.0 - no_v * no_v) > 1.0 { fresnel = 1.0; }
    let size = vec2<f32>(textureDimensions(scene_color));
    let uv = input.clip.xy / size;
    let offset = river_to_view(n).xy * (river_slw_parameters().refraction.x - 1.0) * 0.025;
    var refracted = clamp(uv + offset, 0.5 / size, 1.0 - 0.5 / size);
    let receiver = river_scene_depth(textureLoad(scene_depth, vec2<i32>(refracted * size), 0));
    if receiver <= dot(input.world - river_view.eye.xyz, river_view.forward.xyz) { refracted = uv; }
    let transmission = textureSampleLevel(scene_color, normal_sampler, refracted, 0.0).rgb;
    // Reflection points into the water. Screen tracing uses the pre-river scene;
    // misses see the underwater body color rather than an air-side sky capture.
    let roughness = mix(river_slw_parameters().shading.z, river_foam_settings().shading.y, saturate(foam.opacity));
    let reflected = river_ssr(input.world, reflect(-v, n), dot(input.world - river_view.eye.xyz, river_view.forward.xyz), roughness, input.clip.xy);
    let environment = reflected.rgb + authored.underwater.rgb * (1.0 - reflected.a);
    let foam_amount = saturate(foam.opacity);
    let color = mix(transmission, environment, fresnel);
    return vec4<f32>(mix(color, river_foam_settings().base_color.rgb * authored.underwater.rgb, foam_amount), 1.0);
}

// Listener probe: an isolated 1x1 pass, after the solver, not a CPU
// approximation of the river source bounds. Alpha is always opaque so the
// MAX-blended red channel ORs overlapping rivers without extra readbacks.
struct RiverAudioListener {
    eye: vec4<f32>,
    foot: vec4<f32>,
};
@group(6) @binding(4) var<uniform> river_audio_listener: RiverAudioListener;
@vertex fn vs_river_audio(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let xy = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0);
}
@fragment fn fs_river_audio() -> @location(0) vec4<f32> {
    let eye = river_eye_water_at(river_audio_listener.eye.xyz);
    // Separate foot contact from eye submersion: JKA EV_WATER_TOUCH fires
    // when feet enter water, before the eye goes below the surface.
    let foot = river_eye_water_at(river_audio_listener.foot.xyz);
    return vec4<f32>(eye.z, foot.z, 0.0, 1.0);
}

struct RiverSubmergedOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) eye_water: vec4<f32>,
};
@vertex fn vs_submerged(@builtin(vertex_index) index: u32) -> RiverSubmergedOut {
    var out: RiverSubmergedOut;
    let xy = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    out.clip = vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0);
    out.eye_water = river_eye_water_at(river_view.eye.xyz);
    return out;
}

// Final color/depth has the underside and opaque bed already rasterized. Its
// nearest depth terminates the water path at that exact surface/receiver.
// The patch footprint clips sideways escape into clear sky at the grid edge.
@fragment fn fs_submerged(input: RiverSubmergedOut) -> @location(0) vec4<f32> {
    if input.eye_water.z < 0.5 { discard; }
    let size = vec2<f32>(textureDimensions(scene_depth));
    let uv = input.clip.xy / size;
    let ndc = uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
    let ray = river_view.forward.xyz + river_view.right.xyz * ndc.x / river_view.distortion.x
        + river_view.up.xyz * ndc.y / (river_view.distortion.x * river_view.distortion.y);
    var end = river_scene_depth(textureLoad(scene_depth, vec2<i32>(input.clip.xy), 0));
    for (var axis = 0u; axis < 2u; axis += 1u) {
        if abs(ray[axis]) > 1e-8 {
            let side = body.origin[axis] + sign(ray[axis]) * body.grid[axis] * 0.5;
            end = min(end, max((side - river_view.eye[axis]) / ray[axis], 0.0));
        }
    }
    // Hybrid's wavelength-dependent Beer-Lambert attenuation and downwelling.
    // Distances and tint are independent authored values for this river.
    let eye_depth = max(input.eye_water.x - river_view.eye.z, 0.0);
    let path = end * length(ray);
    let scene = textureLoad(scene_color, vec2<i32>(input.clip.xy), 0);
    // Both modes share the exact spectral attenuation equations. The old
    // renderer already used these; duplicating them was not a useful A/B.
    let medium = jka_underwater_spectral_medium(
        scene.rgb, authored.underwater.rgb, path,
        authored.underwater.w, eye_depth, authored.underwater_depth.x);
    // B adds only a narrow waterline tint; A retains the old appearance.
    if authored.underwater_depth.y > 0.5 {
        let near_surface = 1.0 - smoothstep(0.0, 24.0, eye_depth);
        return vec4<f32>(mix(medium, medium * 0.88 + authored.underwater.rgb * 0.12,
            near_surface), scene.a);
    }
    return vec4<f32>(medium, scene.a);
}

// r_riverWire 1: unmasked x-ray overlay of the whole displaced grid,
// including dry and never-loaded cells.
@fragment fn fs_river_wire(input: RiverOut) -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 1.0, 0.7, 1.0);
}

// r_riverWire 0: only where the water itself draws (fs's material mask).
@fragment fn fs_river_wire_masked(input: RiverOut) -> @location(0) vec4<f32> {
    let fluid = river_live_field(sim_grid, input.uv);
    let masks = river_surface_masks(fluid.y, 1.0, input.uv, vec2<f32>(textureDimensions(sim_grid, 0)), 1.0, 0.0, 3.0e38);
    if masks.material_mask < 0.3333 {
        discard;
    }
    return vec4<f32>(0.0, 1.0, 0.7, 1.0);
}
