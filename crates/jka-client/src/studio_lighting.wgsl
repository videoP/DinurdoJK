// Isolated character stage: output-timeline lights, independent of BSP lighting.
fn studio_light_color(base: vec4<f32>, albedo: vec4<f32>, normal: vec3<f32>, position: vec3<f32>) -> vec4<f32> {
    let count = min(entity_lighting.values.z, 32u);
    // values.w is exclusive to the isolated Render Studio character stage.
    // With no environment selected, preserve legacy/gameplay presentation.
    if (entity_lighting.values.w == 0u && count == 0u) { return base; }
    let n = normalize(normal);
    var radiance = vec3<f32>(0.0);
    if (entity_lighting.values.w != 0u) {
        let sunlight = max(dot(n, normalize(entity_lighting.studio_sun_direction.xyz)), 0.0);
        radiance += entity_lighting.studio_ambient.rgb * entity_lighting.studio_ambient.a;
        radiance += entity_lighting.studio_sun.rgb * entity_lighting.studio_sun.a * sunlight;
    }
    for (var i = 0u; i < count; i += 1u) {
        let pr = entity_lighting.studio_lights[i * 4u];
        let ci = entity_lighting.studio_lights[i * 4u + 1u];
        let di = entity_lighting.studio_lights[i * 4u + 2u];
        let shape = entity_lighting.studio_lights[i * 4u + 3u];
        let delta = pr.xyz - position;
        let distance = max(length(delta), 0.0001);
        let direction = delta / distance;
        let attenuation = pow(max(1.0 - distance / max(pr.w, 1.0), 0.0), 2.0);
        var cone = 1.0;
        if (shape.y > 0.5) {
            let cosine = dot(-direction, di.xyz);
            let t = clamp((cosine - shape.x) / max(di.w - shape.x, 0.00001), 0.0, 1.0);
            cone = t * t * (3.0 - 2.0 * t);
        }
        radiance += ci.rgb * ci.a * attenuation * max(dot(n, direction), 0.0) * cone;
    }
    if (entity_lighting.values.w != 0u) {
        // Unlike the additive legacy path, never retain the prelit base.
        // Ambient=Sun=0, no local lights => physically black, even when the
        // renderer previously supplied fullbright vertex colors.
        return vec4<f32>(albedo.rgb * radiance, base.a);
    }
    return vec4<f32>(base.rgb + albedo.rgb * radiance, base.a);
}
