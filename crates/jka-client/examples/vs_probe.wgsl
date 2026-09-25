struct OceanRenderSettings {
    map_scales: array<vec4<f32>, 3>,
    water_color: vec4<f32>,
    foam_color: vec4<f32>,
    ocean_info: vec4<f32>,
    surface: vec4<f32>,
};
@group(0) @binding(0) var ocean_displacement: texture_2d_array<f32>;
@group(0) @binding(1) var ocean_sampler: sampler;
@group(0) @binding(2) var<uniform> ocean: OceanRenderSettings;
@group(0) @binding(3) var<storage, read_write> out_data: array<vec4<f32>>;
@group(0) @binding(4) var<uniform> probe: vec4<f32>; // origin x, origin z, step, camera-relative flag

@compute @workgroup_size(8,8,1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= 128u || gid.y >= 128u) { return; }
    let position = vec3<f32>(
        probe.x + f32(gid.x) * probe.z,
        0.0,
        probe.y + f32(gid.y) * probe.z
    );
    let units_per_meter = max(ocean.ocean_info.z, 0.001);
    let world_m = position.xz / units_per_meter;
    var displacement_m = vec3<f32>(0.0);
    for (var i = 0u; i < 3u; i = i + 1u) {
        let scale = ocean.map_scales[i];
        displacement_m += textureSampleLevel(
            ocean_displacement, ocean_sampler, world_m * scale.xy, i32(i), 0.0
        ).xyz * scale.z;
    }
    out_data[gid.y * 128u + gid.x] = vec4<f32>(displacement_m, ocean.map_scales[0].z);
}
