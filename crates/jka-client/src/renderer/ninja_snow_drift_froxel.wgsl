// Private snow material is compiled only with ninja_composite. The active
// FVOL 200m passive instance is a separate emitter from the 50m blizzard.
struct NinjaSnowDustUniform {
    centre_extent: vec4<f32>, // map centre XZ; UE FVOL extent in JKA units, height scale
    height_origin: vec4<f32>, // height grid UV origin XZ + physical span
    time_sky: vec4<f32>, // demo/game time, sky RGB
};
struct NinjaSnowDustSegment {
    optical_depth: f32,
    scattering: vec3<f32>,
    emission: vec3<f32>,
};
@group(1) @binding(0) var<uniform> ninja_snow_drift: NinjaSnowDustUniform;
@group(1) @binding(1) var ninja_snow_drift_source: texture_2d<f32>;
@group(1) @binding(2) var ninja_snow_drift_height: texture_2d<f32>;
@group(1) @binding(3) var ninja_snow_drift_volume_noise: texture_3d<f32>;
@group(1) @binding(4) var ninja_snow_drift_repeat: sampler;

fn ninja_snow_dust_height_at(point: vec3<f32>) -> vec2<f32> {
    let uv = (point.xz - ninja_snow_drift.height_origin.xy) / ninja_snow_drift.height_origin.z;
    if (any(uv < vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0))) {
        return vec2<f32>(0.0, 0.0);
    }
    let size = textureDimensions(ninja_snow_drift_height);
    let pos = clamp(vec2<i32>(uv * vec2<f32>(size)), vec2<i32>(0), vec2<i32>(size) - vec2<i32>(1));
    let h = textureLoad(ninja_snow_drift_height, pos, 0);
    return vec2<f32>(h.x, h.w);
}
fn ninja_snow_dust_segment(ray: vec3<f32>, left: f32, right: f32) -> NinjaSnowDustSegment {
    let world = settings.camera_pos_time.xyz + ray * (0.5 * (left + right));
    let ground = ninja_snow_dust_height_at(world);
    if (ground.y < 0.5) {
        return NinjaSnowDustSegment(0.0, vec3<f32>(0.0), vec3<f32>(0.0));
    }
    let s = ninja_snow_dust_density(
        world, ground.x, ninja_snow_drift.centre_extent.xy,
        ninja_snow_drift.centre_extent.z, ninja_snow_drift.time_sky.x,
        ninja_snow_drift.time_sky.yzw,
    );
    let length_metres = (right - left) * (2.54 / 100.0);
    let tau = max(s.extinction, 0.0) * length_metres;
    return NinjaSnowDustSegment(tau, s.albedo * tau, s.emission * length_metres);
}
