@group(0) @binding(3) var ninja_fog_scene_depth: texture_depth_multisampled_2d;

fn ninja_fog_device_depth(pixel: vec2<i32>) -> f32 {
    var nearest = 0.0;
    let sample_count = textureNumSamples(ninja_fog_scene_depth);
    for (var sample = 0u; sample < sample_count; sample += 1u) {
        nearest = max(nearest, textureLoad(ninja_fog_scene_depth, pixel, i32(sample)));
    }
    return nearest;
}
