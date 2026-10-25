override ENABLE_CSM_COVERAGE: bool = false;

// Shared by both world shader families. Static receivers already contain the
// composed sunlight; only the moving-caster footprint needs a live comparison.
fn static_sun_lightmap_receiver(input: VertexOut) -> bool {
    return (lighting_settings.baked_sun_debug_mode.z & 1u) != 0u && input.shell_kind == 0u
        && ((material_flags() & (2u | 8388608u | 16777216u)) != 0u || material_tc_gen() == 1u);
}

fn sample_dynamic_cascade(world_position: vec3<f32>, normal: vec3<f32>, cascade: i32) -> f32 {
    if textureNumLayers(shadow_texture) < 9u { return 1.0; }
    let coverage = shadow_settings.dynamic_coverage[u32(cascade)];
    if ENABLE_CSM_COVERAGE && all(coverage == vec4<u32>(0u)) { return 1.0; }
    let texel = shadow_settings.cascade_texel_sizes[u32(cascade)];
    let offset = normal * (shadow_settings.bevy_params.y * texel)
        - normalize(shadow_settings.light_direction_enabled.xyz) * shadow_settings.bevy_params.x;
    let clip = shadow_settings.view_proj[u32(cascade)] * vec4(world_position + offset, 1.0);
    let ndc = clip.xyz / clip.w;
    if ENABLE_CSM_COVERAGE {
        if !shadow_coverage_contains(coverage, ndc.xy) { return 1.0; }
    } else {
        let region = bitcast<vec4<f32>>(coverage);
        if any(ndc.xy < region.xy) || any(ndc.xy > region.zw) { return 1.0; }
    }
    if ndc.z < 0.0 || ndc.z > 1.0 {
        return 1.0;
    }
    let uv = ndc.xy * vec2(0.5,-0.5) + vec2(0.5);
    return sample_shadow_map_bevy_castano(uv, ndc.z, cascade + 5i);
}

fn lightmap_moving_sun_visibility(input: VertexOut) -> f32 {
    let depth = max(dot(input.world_position-camera.camera_pos_time.xyz,
        normalize(shadow_settings.camera_forward.xyz)),0.0);
    var cascade = 0i;
    if depth > shadow_settings.split_depths.x { cascade = 1i; }
    if depth > shadow_settings.split_depths.y { cascade = 2i; }
    if depth > shadow_settings.split_depths.z { cascade = 3i; }
    if depth > shadow_settings.split_depths[u32(cascade)] { return 1.0; }
    var visibility = sample_dynamic_cascade(input.world_position,input.world_normal,cascade);
    if cascade < 3i {
        let far = shadow_settings.split_depths[u32(cascade)];
        let near = (1.0-shadow_settings.bevy_params.z)*far;
        if depth >= near {
            let next = sample_dynamic_cascade(input.world_position,input.world_normal,cascade+1i);
            visibility = mix(visibility,next,(depth-near)/(far-near));
        }
    }
    return visibility;
}

fn shade_composed_sun(lightmap: vec3<f32>, flux: f32, input: VertexOut) -> vec3<f32> {
    if flux <= 0.0 { return lightmap; }
    let visibility = lightmap_moving_sun_visibility(input);
    if lighting_settings.runtime_sun_radiance.w > 0.5 {
        return max(lightmap-lighting_settings.runtime_sun_radiance.rgb*flux*(1.0-visibility),vec3(0.0));
    }
    if visibility >= 0.9999 { return lightmap; }
    return lightmap-max(lightmap-baked_local_total(input),vec3(0.0))*flux*(1.0-visibility);
}
