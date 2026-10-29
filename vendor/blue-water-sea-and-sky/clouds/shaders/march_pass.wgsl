// The clouds' march (`clouds.frag`'s `main` of @takram/three-clouds) as compute over the march buffer, a quarter of the
// drawing buffer on each axis: every update marches one pixel of each 4 × 4 block, the block's cell taken in Bayer
// order over 16 updates, and the reconstruction (`resolve.wgsl`) fills the rest from history. The ray stops at the
// opaque scene (`scene.opaque.depth`) and at the sea. Writes the clouds' premultiplied radiance and opacity with the
// haze over them, the front depth, the motion and the opaque scene's depth for the reconstruction, and the light shaft's
// length. A ray that starts
// under the sea (a camera under water) meets the sea's surface first and marches nothing.
//
// MIT License, Copyright (c) 2024 Shota Matsuda (https://github.com/takram-design-engineering/three-geospatial). The
// full notice is in `naval-clouds/THIRD_PARTY.md`.

#import naval_clouds::common::{clouds, scene_depth, camera_planet, ray_starts_under_water, METER_TO_KM}
#import naval_clouds::field::{march_jitter, weather_mip_level}
#import naval_clouds::march::{frame_light, march_clouds, march_shadow_length, approximate_haze, ray_ranges, clouds_near_far, shadow_near_far, haze_near_far}
#import naval_sky::aerial::aerial_along

// The ocean's underwater probe (`ray_starts_under_water`).
@group(2) @binding(12) var underwater_probe: texture_2d<f32>;
@group(3) @binding(0) var color_out: texture_storage_2d<rgba16float, write>;
@group(3) @binding(1) var depth_velocity_out: texture_storage_2d<rgba32float, write>;
@group(3) @binding(2) var shadow_length_out: texture_storage_2d<rgba16float, write>;

// Metres along the view of a reversed depth (`naval::camera::view_depth`).
fn view_depth(depth: f32) -> f32 {
    let near = clouds.planes.x;
    let far = clouds.planes.y;
    return far * near / (depth * (far - near) + near);
}

// A clip position's texture coordinates (y down).
fn clip_to_uv(clip: vec4<f32>) -> vec2<f32> {
    let ndc = clip.xy / clip.w;
    return vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
}

// What one ray gives: the premultiplied radiance and opacity (haze included), the front depth (m), the motion and the
// opaque scene's view depth (m, 0 where the ray met none) for the reconstruction, and the light shaft's length (m).
struct Marched {
    color: vec4<f32>,
    front_depth: f32,
    velocity: vec2<f32>,
    scene_view_z: f32,
    shadow_length: f32,
}

// The march of the ray through a pixel of the drawing buffer, `jitter` its blue noise.
fn march_pixel(pixel: vec2<u32>, jitter: f32) -> Marched {
    let size = clouds.screen.xy;
    let uv = (vec2<f32>(pixel) + 0.5) / size;
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    // The ray from the lens and the view's rotation alone, as takram's vertex stage builds it: a point of the near plane
    // less the camera would lose the direction to f32 steps of the camera's position (2.4e-4 m at 2.6 km up, a fifth of
    // a pixel), which reprojection would read as the view moving.
    let tangent = 1.0 / clouds.forward.w;
    let view_ray = vec3<f32>(ndc.x * tangent * clouds.viewport.x / clouds.viewport.y, ndc.y * tangent, -1.0);
    let rotation = mat3x3<f32>(clouds.view[0].xyz, clouds.view[1].xyz, clouds.view[2].xyz);
    let ray_direction = normalize(transpose(rotation) * view_ray);
    let camera = camera_planet();
    let cos_theta = dot(clouds.sun.xyz, ray_direction);

    let ranges = ray_ranges(camera, ray_direction);
    var ray_near_far = clouds_near_far(ranges);
    var shadow_ray_near_far = shadow_near_far(ranges);
    var haze_ray_near_far = haze_near_far(ranges);

    // The opaque scene ends the rays (`getRayDistanceToScene`).
    let depth_size = textureDimensions(scene_depth);
    let depth_pixel = min(vec2<u32>(uv * vec2<f32>(depth_size)), depth_size - 1u);
    let depth = textureLoad(scene_depth, depth_pixel, 0);
    var scene_view_z = 0.0;
    var ray_distance_to_scene = 0.0;
    if depth > 0.0 {
        scene_view_z = view_depth(depth);
        ray_distance_to_scene = scene_view_z / max(dot(ray_direction, clouds.forward.xyz), 1e-4);
        ray_near_far.y = min(ray_near_far.y, ray_distance_to_scene);
        shadow_ray_near_far.y = min(shadow_ray_near_far.y, ray_distance_to_scene);
        haze_ray_near_far.y = min(haze_ray_near_far.y, ray_distance_to_scene);
    }
    // From under the water every ray meets the sea's surface first.
    let submerged = ray_starts_under_water(underwater_probe, uv);
    let intersects_ground = submerged || any(ray_near_far < vec2<f32>(0.0));
    let intersects_scene = ray_near_far.y < ray_near_far.x;

    let light = frame_light(camera);
    let pixel_position = vec2<f32>(pixel);
    // Radians one pixel of the drawing buffer spans.
    let pixel_angle = 2.0 / (clouds.forward.w * size.y);

    var color = vec4<f32>(0.0);
    var front_depth = ray_near_far.y;
    var velocity = vec2<f32>(0.0);
    var shadow_length = 0.0;
    var hit_clouds = false;

    if !intersects_ground && !intersects_scene {
        let ray_origin = ray_near_far.x * ray_direction + camera;
        let slant = abs(dot(normalize(ray_origin), ray_direction));
        var mip_level = weather_mip_level(ray_near_far.x, slant, pixel_angle) * clouds.planes.w;
        mip_level = mix(0.0, mip_level, min(1.0, 0.2 * clouds.camera.w / clouds.heights.y));
        let marched = march_clouds(ray_origin, ray_direction, ray_near_far, cos_theta, jitter, pow(2.0, mip_level), light, pixel_position);
        color = marched.color;

        // The front depth is −1 when nothing was accumulated.
        hit_clouds = marched.front_depth >= 0.0;
        if hit_clouds {
            front_depth = ray_near_far.x + marched.front_depth;
            if clouds.shafts.w > 0.5 {
                // The shaft's ray ends at the clouds, smoothly by their opacity.
                shadow_ray_near_far.y = mix(shadow_ray_near_far.y, min(front_depth, shadow_ray_near_far.y), color.a);
                // The shadow length comes before the aerial perspective, which takes it.
                if all(shadow_ray_near_far >= vec2<f32>(0.0)) {
                    shadow_length = march_shadow_length(
                        shadow_ray_near_far.x * ray_direction + camera,
                        ray_direction,
                        shadow_ray_near_far,
                        jitter,
                        pixel_position,
                    );
                }
            }
            if clouds.switches.z > 0.5 {
                haze_ray_near_far.y = mix(haze_ray_near_far.y, min(front_depth, haze_ray_near_far.y), color.a);
            }
            // The air between the eye and the clouds' front, as every material takes it (the aerial-perspective volume:
            // the atmosphere and the weather's haze).
            if clouds.experiments.x < 0.5 || clouds.experiments.x > 4.5 {
                let air = aerial_along(uv, front_depth);
                color = vec4<f32>(color.rgb * air.transmittance + air.inscatter * color.a, color.a);
            }
            // Motion of the clouds' front for the reconstruction, from the camera in the world: the planet frame's
            // heights carry the planet's radius, and its half-metre steps in f32 would move a still view's history by a
            // tenth of a pixel an update, which bilinear reads of the history smear into streaks. The front stood where
            // the wind has carried it from since the previous update, so a drifting cloud keeps its history (binoculars
            // magnify a 12 m/s wind 20 km off to a fifth of a pixel an update).
            let front_world = clouds.camera.xyz + ray_direction * front_depth;
            let previous = clouds.previous_view_projection * vec4<f32>(front_world - clouds.drift_step.xyz, 1.0);
            velocity = uv - clip_to_uv(previous);
        }
    }

    if !hit_clouds {
        if clouds.shafts.w > 0.5 && !submerged && all(shadow_ray_near_far >= vec2<f32>(0.0)) {
            shadow_length = march_shadow_length(
                shadow_ray_near_far.x * ray_direction + camera,
                ray_direction,
                shadow_ray_near_far,
                jitter,
                pixel_position,
            );
        }
        // Motion of the scene, or of the sky at infinity, where no cloud was marched.
        var previous: vec4<f32>;
        if scene_view_z > 0.0 {
            front_depth = scene_view_z;
            previous = clouds.previous_view_projection * vec4<f32>(clouds.camera.xyz + ray_direction * ray_distance_to_scene, 1.0);
        } else {
            front_depth = clouds.planes.y;
            previous = clouds.previous_view_projection * vec4<f32>(ray_direction, 0.0);
        }
        velocity = uv - clip_to_uv(previous);
    }

    if clouds.switches.z > 0.5 && !submerged && (clouds.experiments.x < 0.5 || clouds.experiments.x > 4.5) {
        let haze = approximate_haze(
            clouds.planes.x * ray_direction + camera,
            ray_direction,
            haze_ray_near_far.y - haze_ray_near_far.x,
            cos_theta,
            shadow_length,
            light,
        );
        color = vec4<f32>(mix(color.rgb, haze.rgb, haze.a), color.a * (1.0 - haze.a) + haze.a);
    }
    return Marched(color, front_depth, velocity, scene_view_z, shadow_length);
}

// The update's march: one pixel of each 4 × 4 block, into the march buffer.
@compute @workgroup_size(8, 8, 1)
fn march_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let low = id.xy;
    if any(low >= vec2<u32>(clouds.screen.zw)) {
        return;
    }
    let pixel = min(low * 4u + vec2<u32>(clouds.frame.yz), vec2<u32>(clouds.screen.xy) - 1u);
    // The blue noise of the drawing buffer's pixel, not the march buffer's: takram reads its STBN at the march buffer's
    // texel, which works because each of STBN's 64 frames is a fresh pattern; this crate's blue noise is one pattern
    // stepped by the golden ratio, so all 16 cells of a block would share it, offset only by the update each is marched
    // in, and print the same 4-pixel pattern into every block. It steps once per march of the pixel (`march_jitter`).
    let marched = march_pixel(pixel, march_jitter(pixel, u32(clouds.frame.x) / 16u));
    textureStore(color_out, low, marched.color);
    textureStore(depth_velocity_out, low, vec4<f32>(marched.front_depth, marched.velocity, marched.scene_view_z));
    textureStore(shadow_length_out, low, vec4<f32>(marched.shadow_length * METER_TO_KM, 0.0, 0.0, 1.0));
}

// The first update after a cut marches every pixel, straight into the history the composite reads (here `color_out`
// and `shadow_length_out` are the drawing buffer's), so the first picture of a new view is whole rather than the
// march buffer upsampled (as the game's clouds did, which marched every pixel for two updates after a cut); the
// reconstruction then refines it as usual. The motion is not written: the next update's march writes its own.
@compute @workgroup_size(8, 8, 1)
fn march_full_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pixel = id.xy;
    if any(pixel >= vec2<u32>(clouds.screen.xy)) {
        return;
    }
    // The jitter one step before the updates' own (k − 1 of 256), so no pixel's next march repeats it.
    let marched = march_pixel(pixel, march_jitter(pixel, u32(clouds.frame.x) / 16u + 255u));
    textureStore(color_out, pixel, marched.color);
    // The reconstruction's state beside the shafts' length: one sample of the pixel's own, the opaque scene it saw (km)
    // and no prior (`resolve.wgsl`).
    textureStore(
        shadow_length_out,
        pixel,
        vec4<f32>(marched.shadow_length * METER_TO_KM, 1.0, marched.scene_view_z * METER_TO_KM, 0.0),
    );
}
