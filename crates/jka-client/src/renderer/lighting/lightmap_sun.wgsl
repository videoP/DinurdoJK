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

// r_staticWorldCsm with a composed static sun: inside cascade range the full
// static+moving cascades replace the bake's (softer) visibility. The composer
// stores the unshadowed-minus-baked sun amount in static_sun_aux.r * 4, so
// unshadowed = flux + that. Beyond range the baked lightmap stays as is.
// Returns w = 0 when not applicable.
fn static_csm_composed_sun(lightmap: vec3<f32>, flux: f32, input: VertexOut) -> vec4<f32> {
    if (lighting_settings.baked_sun_debug_mode.z & 8u) == 0u || lighting_settings.runtime_sun_radiance.w < 0.5 {
        return vec4(0.0);
    }
    let depth = max(dot(input.world_position - camera.camera_pos_time.xyz,
        normalize(shadow_settings.camera_forward.xyz)), 0.0);
    let far = shadow_settings.split_depths[u32(clamp(i32(shadow_settings.bevy_params.w + 0.5), 1, 4) - 1)];
    let inside = 1.0 - smoothstep(far * 0.85, far, depth);
    if inside <= 0.0 { return vec4(0.0); }
    let aux = textureSampleLevel(static_sun_aux, lightmap_sampler, input.lightmap_uv, 0.0);
    if aux.g > 0.5 { return vec4(0.0); }
    let unshadowed = flux + aux.r * 4.0;
    let radiance = lighting_settings.runtime_sun_radiance.rgb;
    let csm = max(lightmap + radiance * (unshadowed * sun_shadow_transmission(input, input.world_normal) - flux), vec3(0.0));
    if inside >= 1.0 { return vec4(csm, 1.0); }
    var baked = lightmap;
    if flux > 0.0 {
        baked = max(lightmap - radiance * flux * (1.0 - lightmap_moving_sun_visibility(input)) * shadow_settings.params.z, vec3(0.0));
    }
    return vec4(mix(baked, csm, inside), 1.0);
}

fn shade_composed_sun(lightmap: vec3<f32>, flux: f32, input: VertexOut) -> vec3<f32> {
    let static_csm = static_csm_composed_sun(lightmap, flux, input);
    if static_csm.w > 0.5 { return static_csm.rgb; }
    if flux <= 0.0 { return lightmap; }
    let visibility = lightmap_moving_sun_visibility(input);
    if lighting_settings.runtime_sun_radiance.w > 0.5 {
        return max(lightmap-lighting_settings.runtime_sun_radiance.rgb*flux*(1.0-visibility)*shadow_settings.params.z,vec3(0.0));
    }
    if visibility >= 0.9999 { return lightmap; }
    // The compositor applies the user strength to legacy alpha. Facing and
    // static sun visibility remain geometric, independent of sun intensity.
    let moving_fraction = clamp(flux, 0.0, 1.0);
    return lightmap-max(lightmap-baked_local_total(input),vec3(0.0))*moving_fraction*(1.0-visibility);
}
