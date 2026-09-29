
// Baked lightgrid is already in input.color. Local light uses the original
// material tint, so a dark lightgrid cannot suppress a nearby saber or lamp.
fn rt_model_color(input: VertexOut, tex: vec4<f32>) -> vec4<f32> {
    let base = lit_color(input.normal, tex) * input.color;
    if (camera.render_flags.x != 0u || lighting_settings.values.x == 0u || lighting_settings.values.y == 0u) {
        return base;
    }
    let normal = input.normal * inverseSqrt(max(dot(input.normal, input.normal), 1e-12));
    let cluster = cluster_for_fragment(input);
    var irradiance = vec3<f32>(0.0);
    for (var i = 0u; i < min(cluster.count, 32u); i += 1u) {
        let light = dynamic_lights[cluster.indices[i]];
        let delta = light.position_radius.xyz - input.world_position;
        let distance_squared = dot(delta, delta);
        let radius = max(light.position_radius.w, 0.0);
        if (distance_squared <= 1e-8 || distance_squared >= radius * radius) { continue; }
        let distance_to_light = sqrt(distance_squared);
        let direction = delta / distance_to_light;
        let ndotl = max(dot(normal, direction), 0.0);
        var scalar = light.color_intensity.a * local_light_attenuation(light, distance_to_light)
            * ndotl * local_light_surface_scale(light);
        if (ENABLE_MAP_LIGHT_SIMULATION && light.shadow.y > 0.5) {
            scalar = q3map_light_scalar(light, distance_to_light, q3map_surface_angle(light, ndotl));
            if (!q3map_fast_contribution_visible(scalar)) { continue; }
        }
        scalar *= emitter_visibility(light, direction);
        if (scalar <= 0.0) { continue; }
        irradiance += light.color_intensity.rgb * scalar
            * ray_traced_local_shadow_visibility(light, input.world_position, normal);
    }
    return vec4<f32>(base.rgb + tex.rgb * input.raw_color.rgb * irradiance, base.a);
}
