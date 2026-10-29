// The same immutable GodotGrass wind image used by grass_prepare.wgsl.
// It rides with the camera so world and model materials share one texture.
@group(0) @binding(1) var foliage_wind_noise: texture_2d<f32>;
@group(0) @binding(2) var foliage_wind_sampler: sampler;

fn foliage_noise_position(position: vec3<f32>, wind: vec4<f32>, variation: f32) -> vec3<f32> {
    let speed = length(wind.xy);
    if (speed <= 0.00001) { return position; }
    let direction = wind.xy / speed;
    let root_m = position.xz / 64.0;
    let advected = root_m - wind.zw;
    // Broad gusts follow the exact grass noise scale and transport. The second
    // tap is finer, with a stable per-plant offset, for independent leaf motion.
    let broad = textureSampleLevel(foliage_wind_noise, foliage_wind_sampler,
        advected * 0.025, 0.0).r * 2.0 - 1.0;
    let detail_uv = advected * 0.075
        + vec2<f32>(variation * 19.17, variation * 7.31)
        + position.y * vec2<f32>(0.00017, 0.00011);
    let detail = textureSampleLevel(foliage_wind_noise, foliage_wind_sampler,
        detail_uv, 0.0).r * 2.0 - 1.0;
    // Inherited speed matches grass; card travel is only a few map units.
    // Strong wind still transports noise faster after amplitude reaches its
    // limit. Maximum displacement stays below foliage::BOUNDS_PADDING (8u).
    let response = min(speed * 0.85, 1.4);
    let along = (0.5 + 1.8 * broad + 1.9 * detail) * response;
    let across = 0.75 * detail * response;
    let horizontal = direction * along + vec2<f32>(-direction.y, direction.x) * across;
    return position + vec3<f32>(horizontal.x, 0.25 * detail * response, horizontal.y);
}

// Compiled world geometry has no per-model pivot: continuous world-space noise
// preserves shared vertices and material-stage alignment without guessed UV roots.
fn foliage_wind_position(position: vec3<f32>, material_word: u32, wind: vec4<f32>) -> vec3<f32> {
    if ((material_word & 262144u) == 0u) { return position; }
    return foliage_noise_position(position, wind, 0.0);
}

// Zero is ineligible; [1,2) encodes a stable origin-derived variation seed.
// Every vertex/stage of a model gets the same seed on CPU and GPU-instance paths.
fn foliage_model_position(position: vec3<f32>, tag: f32, wind: vec4<f32>) -> vec3<f32> {
    if (tag < 0.5) { return position; }
    return foliage_noise_position(position, wind, max(tag - 1.0, 0.0));
}

