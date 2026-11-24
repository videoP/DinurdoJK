@group(0) @binding(3) var ninja_fog_scene_depth: texture_depth_2d;

fn ninja_fog_device_depth(pixel: vec2<i32>) -> f32 {
    return textureLoad(ninja_fog_scene_depth, pixel, 0);
}
