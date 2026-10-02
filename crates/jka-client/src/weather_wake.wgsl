// Ripples from footsteps, drags and landings (weather/wake.rs). Appended only to
// shaders that declare the module-scope `weather_surface` uniform.

// Height gradient of every live disturbance at a world position. Each is an
// expanding packet of a couple of crests that settles as it ages; a disturbance
// only touches water at its own height, so a jump over a puddle leaves nothing.
fn weather_wake_gradient(world: vec3<f32>) -> vec2<f32> {
    let count = min(u32(weather_surface.wake_info.x), 24u);
    var gradient = vec2<f32>(0.0);
    if (count == 0u) {
        return gradient;
    }
    let lifetime = max(weather_surface.wake_info.y, 0.01);
    let full_radius = weather_surface.wake_info.z * lifetime * 0.5;
    let now = weather_surface.wake_info.w;
    for (var i = 0u; i < count; i = i + 1u) {
        let position = weather_surface.wake_a[i];
        let shape = weather_surface.wake_b[i];
        let age = now - position.w;
        if (age < 0.0 || age > lifetime || abs(world.y - position.y) > 24.0) {
            continue;
        }
        let scale = weather_wake_kind_scale(shape.w);
        let progress = age / lifetime;
        let ease = 1.0 - (1.0 - progress) * (1.0 - progress);
        let radius = full_radius * scale * ease;
        let offset = world.xz - position.xz;
        let squared = dot(offset, offset);
        let reach = radius + 6.0;
        if (squared > reach * reach) {
            continue;
        }
        let dist = max(sqrt(squared), 0.001);
        let behind = radius - dist;
        let wavelength = 16.0 * sqrt(scale);
        let packet = smoothstep(-3.0, 2.0, behind)
            * (1.0 - smoothstep(wavelength * 1.2, wavelength * 3.2, behind));
        let fade = 1.0 - progress;
        let envelope = shape.x * fade * fade * smoothstep(0.0, 0.05, progress);
        gradient += (offset / dist) * (cos(behind * 6.28318530718 / wavelength) * packet * envelope * 2.6);
    }
    return gradient;
}
