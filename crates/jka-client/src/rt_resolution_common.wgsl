struct RtSunShadowSettings {
    inverse_view_proj: mat4x4<f32>,
    dimensions: vec4<u32>, // width, height, frame index, dither phase (frame index % 4)
    flags: vec4<u32>, // enabled, history valid, reserved, reserved
    prev_camera_pos: vec4<f32>, // previous frame's eye position
};

fn rt_shadow_world_position(pixel: vec2<f32>, depth: f32) -> vec3<f32> {
    let uv = pixel / vec2<f32>(rt_shadow_settings.dimensions.xy);
    let h = rt_shadow_settings.inverse_view_proj * vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.5, 1.0);
    let direction = normalize(h.xyz / h.w - camera.camera_pos_time.xyz);
    return camera.camera_pos_time.xyz + direction * depth;
}
