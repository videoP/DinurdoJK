// Fire_Smoke_Explosive Smoke2 instance for the shared ninja_hvol.wgsl stage.
// The box is the Niagara volume renderer's component bounds after
// CameraFacingFunction: local X = view right, Y = view down, Z = toward the
// camera, pivot .5. The private material (FLAME_VOLUME_CONTRACT) takes uvw.
struct NinjaFlameVolumeSample {
    extinction: vec3<f32>,
    emission: vec3<f32>,
    albedo: vec3<f32>,
};

struct FlameSmokeVolume {
    centre_time: vec4<f32>, // box centre (map units), game seconds
    axis_x: vec4<f32>,      // local axis, half extent (map units)
    axis_y: vec4<f32>,
    axis_z: vec4<f32>,
    resolution: vec4<f32>,  // voxel counts along the local axes, EFX extinction scale
    sky_rgb: vec4<f32>,
    baked_ambient: vec4<f32>,
    plume_flow: vec4<f32>,  // source UV, rise per roll, phase in cycles
};

@group(1) @binding(0) var<uniform> flame_smoke: FlameSmokeVolume;
@group(1) @binding(1) var flame_velocity_density: texture_2d<f32>;
@group(1) @binding(2) var flame_volume_noise: texture_3d<f32>;
@group(1) @binding(3) var flame_clamp: sampler;
@group(1) @binding(4) var flame_wrap: sampler;

fn hv_box_centre() -> vec3<f32> { return flame_smoke.centre_time.xyz; }
fn hv_box_axes() -> mat3x3<f32> {
    return mat3x3<f32>(flame_smoke.axis_x.xyz, flame_smoke.axis_y.xyz, flame_smoke.axis_z.xyz);
}
fn hv_box_half_extent() -> vec3<f32> {
    return vec3<f32>(flame_smoke.axis_x.w, flame_smoke.axis_y.w, flame_smoke.axis_z.w);
}
fn hv_box_resolution() -> vec3<f32> { return flame_smoke.resolution.xyz; }
// Same march budget and trace distance as the fog HVOL instance.
fn hv_max_steps() -> f32 { return 196.0; }
fn hv_max_distance() -> f32 { return 30000.0 / HV_CM_PER_MAP_UNIT; }
fn hv_support(world: vec3<f32>, pad: f32) -> bool { return true; }

fn rising_smoke_uv(uv: vec2<f32>) -> vec2<f32> {
    let flow = flame_smoke.plume_flow;
    let away_from_root = smoothstep(0.02, 0.14, flow.y - uv.y);
    if (away_from_root == 0.0) { return uv; }
    let aspect = flame_smoke.axis_y.w / flame_smoke.axis_x.w;
    let world_aspect = vec2<f32>(1.0, aspect);
    var warped = uv;
    for (var i = 0; i < 2; i += 1) {
        let age = fract(flow.w) + f32(i);
        let side = select(-1.0, 1.0, ((i32(floor(flow.w)) - i) & 1) == 0);
        let centre = flow.xy + vec2<f32>(side * (0.10 + 0.05 * age), -flow.z * age);
        let radius = 0.19 + 0.08 * age;
        let offset = (warped - centre) * world_aspect;
        let radial = dot(offset, offset) / (radius * radius);
        if (radial < 1.0) {
            let strength = smoothstep(0.1, 0.65, age) * (1.0 - smoothstep(1.25, 2.0, age));
            let angle = -side * 5.0 * strength * away_from_root * (1.0 - radial) * (1.0 - radial);
            let s = sin(angle);
            let c = cos(angle);
            let rolled = vec2<f32>(c * offset.x - s * offset.y, s * offset.x + c * offset.y);
            warped = centre + rolled / world_aspect;
        }
    }
    return warped;
}

fn hv_sample(world: vec3<f32>, eye: vec3<f32>, ray_dx: vec3<f32>, ray_dy: vec3<f32>) -> HvSample {
    let uvw = 0.5 * hv_to_local(world) / hv_box_half_extent() + 0.5;
    var weight = 1.0;
    var smoke_uv = uvw;
    if (flame_smoke.resolution.w > 0.0) {
        // The ray box bounds the work. The EFX density tapers to zero
        // on an ellipsoid inside it, including along the viewing axis.
        let centred = 2.0 * uvw - vec3<f32>(1.0);
        let envelope = 1.0 - dot(centred, centred);
        if (envelope <= 0.0) {
            return HvSample(vec3<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0));
        }
        weight = envelope * envelope * flame_smoke.resolution.w;
        smoke_uv = vec3<f32>(rising_smoke_uv(uvw.xy), uvw.z);
    }
    let s = ninja_flame_volume(smoke_uv, flame_smoke.centre_time.w, flame_smoke.sky_rgb.rgb);
    let extinction = s.extinction * weight;
    let baked_scatter = extinction * s.albedo * flame_smoke.baked_ambient.rgb;
    return HvSample(extinction, s.emission * weight + baked_scatter, s.albedo);
}
