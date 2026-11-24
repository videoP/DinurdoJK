// LightScatteringCS port (UE 5.6 VolumetricFog.usf) for the Ninja fog volume.
// Appended to froxel_fog.wgsl (group 0: host lights and shadows),
// ninja_fog_froxel.wgsl and the private fog material (group 1).
//
// Host integration: UE's directional/sky/local light injection is replaced by
// the host froxel lighting terms (ambient, CSM sun, clustered local lights), so
// map lights light the fog. Everything else follows the UE source.

@group(2) @binding(0) var<uniform> nv: NinjaVolumeParams;
@group(2) @binding(1) var nv_history: texture_3d<f32>;
@group(2) @binding(2) var nv_history_sampler: sampler;
@group(2) @binding(3) var nv_light_scattering: texture_storage_3d<rgba16float, write>;

// The host froxel lighting per unit albedo, standing in for LightScattering.
fn nv_host_lighting(world: vec3<f32>, grid_coordinate: vec3<u32>) -> vec3<f32> {
    let eye = nv.view.xyz;
    let to_point = world - eye;
    let radial = length(to_point);
    let ray = to_point / max(radial, 1.0e-4);
    let g = nv.misc.w;
    let ambient_scale = select(0.28, 1.0, settings.fog_params.w > 0.001);
    var light = vec3<f32>(ambient_scale);
    if (settings.sun_direction_enabled.w > 0.5) {
        let sun_direction = normalize(settings.sun_direction_enabled.xyz);
        let sun_intensity = clamp(settings.sun_color_intensity.w / 250.0, 0.0, 2.0);
        let phase = phase_henyey_greenstein(dot(ray, -sun_direction), g);
        light += settings.sun_color_intensity.rgb * phase
            * shadow_visibility(world, radial) * (0.42 * sun_intensity);
    }
    let froxel = vec3<u32>(
        min(grid_coordinate.x * FROXEL_X / nv.grid.x, FROXEL_X - 1u),
        min(grid_coordinate.y * FROXEL_Y / nv.grid.y, FROXEL_Y - 1u),
        0u,
    );
    light += local_light_scattering(froxel, world, radial, ray, vec3<f32>(1.0), g);
    return light;
}

@compute @workgroup_size(4, 4, 4)
fn ninja_light_scattering(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (any(gid >= nv.grid.xyz)) {
        return;
    }
    let cell = vec3<f32>(gid);

    // ComputeHistoryVolumeUVFromTranslatedPos on the unjittered cell centre.
    let centre = nv_cell_world(cell, vec3<f32>(0.5));
    let prev_clip = nv.prev_view_proj * vec4<f32>(centre, 1.0);
    let prev_ndc = prev_clip.xy / prev_clip.w;
    let history_uv = vec3<f32>(
        prev_ndc * vec2<f32>(0.5, -0.5) + 0.5,
        nv_normalized_zslice(prev_clip.w),
    );
    var history_alpha = nv.grid_z.w;
    if (prev_clip.w <= 0.0 || any(history_uv < vec3<f32>(0.0)) || any(history_uv >= vec3<f32>(1.0))) {
        history_alpha = 0.0;
    }
    // Supersample if the history was outside the camera frustum.
    let samples = select(1u, nv.grid.w, history_alpha < 0.001);

    var lighting = vec3<f32>(0.0);
    for (var sample_index = 0u; sample_index < samples; sample_index = sample_index + 1u) {
        let world = nv_cell_world(cell, nv.jitter[sample_index].xyz);
        lighting += nv_host_lighting(world, gid);
    }
    lighting /= f32(samples);

    // VBufferA/B from VoxelizePS: the slice is rasterized at the jittered
    // slice depth (FrameJitterOffset0.z) and evaluated at pixel centres.
    let voxel_offset = vec3<f32>(0.5, 0.5, nv.jitter[0].z);
    let voxel = nv_cell_world(cell, voxel_offset);
    let ddx = nv_cell_world(cell + vec3<f32>(1.0, 0.0, 0.0), voxel_offset) - voxel;
    let ddy = nv_cell_world(cell + vec3<f32>(0.0, 1.0, 0.0), voxel_offset) - voxel;
    let eye = nv.view.xyz;
    let sun_direction = normalize(settings.sun_direction_enabled.xyz);
    let material = ninja_fog_sample(voxel, eye, sun_direction, ddx, ddy);
    // Scale = UnitScale (1/100, per cm) * SliceFadeAlpha^3; converted to map units.
    let view_depth = dot(voxel - eye, nv_forward());
    let fade_start = 0.6;
    let slice_fade = 1.0 - clamp((view_depth / nv.view.w - fade_start) / (1.0 - fade_start), 0.0, 1.0);
    let scale = (NINJA_CM_PER_MAP_UNIT / 100.0) * slice_fade * slice_fade * slice_fade;
    let extinction = material.extinction * scale;
    let scattering = material.albedo * extinction;
    let emissive = material.emission * scale;

    var scattering_and_extinction = vec4<f32>(lighting * scattering + emissive, extinction);
    if (history_alpha > 0.0) {
        let history = textureSampleLevel(nv_history, nv_history_sampler, history_uv, 0.0);
        scattering_and_extinction = mix(scattering_and_extinction, history, history_alpha);
    }
    // MakePositiveFinite.
    let finite = (scattering_and_extinction == scattering_and_extinction)
        & (abs(scattering_and_extinction) < vec4<f32>(65000.0));
    textureStore(nv_light_scattering, gid, select(vec4<f32>(0.0), max(scattering_and_extinction, vec4<f32>(0.0)), finite));
}
