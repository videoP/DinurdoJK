#import naval_sky::aerial::{aerial_over_sky}

// Cached Blue Water sky through JKA's BSP sky admission, including portals.
// Each face is independent (JKA-style mixed resolution). Direction comes
// from inverse VP, not sky polygon positions, so portal camera turns stay exact.
struct CachedSkyCamera {
    view_projection: mat4x4<f32>,
    inverse_view_projection: mat4x4<f32>,
    viewport: vec4<f32>,
}
@group(0) @binding(0) var<uniform> host: CachedSkyCamera;
@group(0) @binding(1) var sky_faces: texture_2d_array<f32>;
@group(0) @binding(2) var sky_down: texture_2d<f32>;
@group(0) @binding(3) var sky_sampler: sampler;

// These projections are the inverse of blue_water_sky_cube.wgsl::cube_direction.
// Texture coordinates are top-left-origin; array layers skip cube face 3 (-Y).
fn sample_sky_faces(direction: vec3<f32>) -> vec3<f32> {
    let abs_dir = abs(direction);
    var uv: vec2<f32>;
    var layer: i32;
    if (abs_dir.x >= abs_dir.y && abs_dir.x >= abs_dir.z) {
        let inv_major = 1.0 / abs_dir.x;
        if (direction.x >= 0.0) {
            uv = vec2<f32>(-direction.z, -direction.y) * inv_major;
            layer = 0;  // +X
        } else {
            uv = vec2<f32>( direction.z, -direction.y) * inv_major;
            layer = 1;  // -X
        }
    } else if (abs_dir.y >= abs_dir.z) {
        let inv_major = 1.0 / abs_dir.y;
        if (direction.y < 0.0) {
            // -Y / skybox_dn is stored as its own 2x2 sampled texture.
            uv = vec2<f32>(direction.x, -direction.z) * inv_major;
            return textureSampleLevel(sky_down, sky_sampler, uv * 0.5 + 0.5, 0.0).rgb;
        }
        uv = vec2<f32>(direction.x, direction.z) * inv_major;
        layer = 2;      // +Y
    } else {
        let inv_major = 1.0 / abs_dir.z;
        if (direction.z >= 0.0) {
            uv = vec2<f32>( direction.x, -direction.y) * inv_major;
            layer = 3;  // +Z
        } else {
            uv = vec2<f32>(-direction.x, -direction.y) * inv_major;
            layer = 4;  // -Z
        }
    }
    return textureSampleLevel(sky_faces, sky_sampler, uv * 0.5 + 0.5, layer, 0.0).rgb;
}
@vertex fn sky_mask(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    var clip = host.view_projection * vec4<f32>(position, 1.0);
    clip.z = 0.0;
    return clip;
}
@fragment fn sky_color(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let ndc = vec2<f32>(2.0 * pixel.x / host.viewport.x - 1.0,
                        1.0 - 2.0 * pixel.y / host.viewport.y);
    let a = host.inverse_view_projection * vec4<f32>(ndc, 0.75, 1.0);
    let b = host.inverse_view_projection * vec4<f32>(ndc, 0.25, 1.0);
    let direction = normalize(b.xyz / b.w - a.xyz / a.w);
    var radiance = aerial_over_sky(sample_sky_faces(direction), pixel.xy / host.viewport.xy);
#if SKY_SDR == true
    radiance = max(radiance, vec3<f32>(0.0));
    radiance = radiance / (vec3<f32>(1.0) + radiance);
#endif
    return vec4<f32>(radiance, 1.0);
}
