// Stable entry points; extension WGSL never owns texture or vertex bindings.
@fragment fn fs_main(input: HudVertexOutput) -> @location(0) vec4<f32> {
    let shaded = hud_pixel(input.uv) * input.tint;
    return hud_compensate(shaded);
}
