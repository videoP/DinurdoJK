// Fog_Mist HVOL 32m instance for the shared ninja_hvol.wgsl stage.
const HV_EXTENT_CM: vec3<f32> = vec3<f32>(3200.0, 1500.0, 3200.0);
const HV_HALF_EXTENT: vec3<f32> = HV_EXTENT_CM / (2.0 * NINJA_CM_PER_MAP_UNIT);
const HV_RESOLUTION: vec3<f32> = vec3<f32>(512.0, 240.0, 512.0);
// The HVOL material has zero support outside this height even at VD.B = 1.
const HV_HEIGHT_SUPPORT_CM: f32 = 0.01 * 1500.0 + (0.05 + 0.01) * 3200.0;

fn hv_box_centre() -> vec3<f32> { return ninja_fog.sim_centre.xyz; }
fn hv_box_axes() -> mat3x3<f32> {
    return mat3x3<f32>(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0));
}
fn hv_box_half_extent() -> vec3<f32> { return HV_HALF_EXTENT; }
fn hv_box_resolution() -> vec3<f32> { return HV_RESOLUTION; }
fn hv_max_steps() -> f32 { return 196.0; }
fn hv_max_distance() -> f32 { return 30000.0 / NINJA_CM_PER_MAP_UNIT; }

fn hv_support(world: vec3<f32>, pad: f32) -> bool {
    let ground = ninja_fog_ground_at(world);
    return ground.y > 0.0
        && abs(world.y - ground.x) <= HV_HEIGHT_SUPPORT_CM / NINJA_CM_PER_MAP_UNIT + pad;
}

fn hv_sample(world: vec3<f32>, eye: vec3<f32>, ray_dx: vec3<f32>, ray_dy: vec3<f32>) -> HvSample {
    let ground = ninja_fog_ground_at(world);
    let sun = -settings.sun_direction_enabled.xyz;
    let sample = ninja_mist_material(world, ground.x, eye, sun, ray_dx, ray_dy, true);
    return HvSample(vec3<f32>(sample.extinction), sample.emission, sample.albedo);
}
