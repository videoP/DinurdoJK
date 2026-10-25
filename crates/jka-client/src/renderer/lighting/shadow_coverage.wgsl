// Matches the CPU's conservative 16 x 8 caster coverage grid.
fn shadow_coverage_contains(mask: vec4<u32>, ndc: vec2<f32>) -> bool {
    let tile = vec2<u32>(clamp(floor((ndc * 0.5 + vec2(0.5)) * vec2(16.0, 8.0)), vec2(0.0), vec2(15.0, 7.0)));
    let bit = tile.y * 16u + tile.x;
    return (mask[bit >> 5u] & (1u << (bit & 31u))) != 0u;
}
