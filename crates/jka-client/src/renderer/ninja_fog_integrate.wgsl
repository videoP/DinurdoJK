// FinalIntegrationCS (UE 5.6 VolumetricFog.usf) and the CombineVolumetricFog
// lookup (HeightFogCommon.ush / HeightFogPixelShader.usf) for the Ninja fog volume.
// Prefixed by ninja_fog_volume_common.wgsl.

@group(0) @binding(0) var<uniform> nv: NinjaVolumeParams;
@group(0) @binding(1) var nv_light_scattering: texture_3d<f32>;
@group(0) @binding(2) var nv_integrated_out: texture_storage_3d<rgba16float, write>;

@compute @workgroup_size(8, 8, 1)
fn ninja_final_integration(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= nv.grid.x || gid.y >= nv.grid.y) {
        return;
    }
    var accumulated_lighting = vec3<f32>(0.0);
    var accumulated_transmittance = 1.0;
    var previous = nv_cell_world(vec3<f32>(f32(gid.x), f32(gid.y), 0.0), vec3<f32>(0.5, 0.5, 0.0));
    var accumulated_depth = 0.0;
    for (var layer = 0u; layer < nv.grid.z; layer = layer + 1u) {
        let coordinate = vec3<u32>(gid.xy, layer);
        let scattering_and_extinction = textureLoad(nv_light_scattering, coordinate, 0);
        let position = nv_cell_world(vec3<f32>(coordinate), vec3<f32>(0.5));
        let step_length = length(position - previous);
        previous = position;
        let transmittance = exp(-scattering_and_extinction.w * step_length);
        accumulated_depth += step_length;
        let fade_in = clamp(accumulated_depth * nv.misc.z, 0.0, 1.0);
        // Energy-conserving integration (Frostbite), as UE.
        let integrated = fade_in
            * (scattering_and_extinction.rgb - scattering_and_extinction.rgb * transmittance)
            / max(scattering_and_extinction.w, 0.00001);
        accumulated_lighting += integrated * accumulated_transmittance;
        accumulated_transmittance *= mix(1.0, transmittance, fade_in);
        textureStore(nv_integrated_out, coordinate, vec4<f32>(accumulated_lighting, accumulated_transmittance));
    }
}
