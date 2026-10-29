// The clouds over the scene (`cloudsEffect.frag` of @takram/three-clouds): one full-screen triangle in the `Sky` phase
// after the dome, untested against depth, blending the reconstructed clouds (premultiplied radiance, opacity) over
// whatever is there: scene × (1 − opacity) + radiance. The march already stopped every ray at the opaque scene and the
// sea, but only one pixel in 16 is marched an update: the others are reconstructed from the history and the
// quarter-resolution neighbourhood, which can carry a cloud onto a hull's or a mast's edge while it moves (takram's
// "disocclusion errors on scene objects"). So where the opaque scene stands nearer than the bottom of the lowest
// layer along a pixel's ray (a ship, an island or an aircraft seen from under the clouds) nothing but haze can lie
// in front of it, and the composite keeps at most the haze's opacity there. Nothing at all where the ray starts under
// the sea: the sea's surface stands between the camera and every cloud (the march gives those pixels nothing, but the
// reconstruction can carry a cloud across the waterline from the history).
//
// A pixel whose mean holds few samples of its own (the first after a cut, a capture's full march, a sky a hull just
// uncovered or a turning view just brought in, a near cloud a travelling view slides across) still carries their
// noise, so it is drawn through a Gaussian over its neighbours that saw its surface, FILTER_DEVIATION wide for one
// sample and narrower as the samples gather (1 / √n), none from FILTER_SAMPLES on, each neighbour weighed by the
// samples its own mean holds: the filter pools the samples about the pixel rather than the pixels. The march's jitter
// is blue noise, whose error a few pixels across cancels. A pixel holding the reconstruction's prior alone counts as
// FILTER_FLOOR of a sample. The history itself is never filtered.
//
// MIT License, Copyright (c) 2024 Shota Matsuda (https://github.com/takram-design-engineering/three-geospatial). The
// full notice is in `naval-clouds/THIRD_PARTY.md`.

#import naval_clouds::common::{clouds, camera_planet, ray_sphere_second, ray_starts_under_water}

@group(1) @binding(0) var cloud_color: texture_2d<f32>;
// The reconstruction's state (`resolve.wgsl`): r the shafts' length, g the samples of its own a pixel's mean holds, b
// the view depth (km) of the opaque scene it saw, 0 for none, a the prior's weight in the mean.
@group(1) @binding(1) var cloud_state: texture_2d<f32>;
// The opaque scene's reversed depth (`scene.opaque.depth`; the layer's own empty depth without it), and the ocean's
// underwater probe (`ray_starts_under_water`; the layer's own 1 × 1 zero without an ocean).
@group(3) @binding(0) var opaque_depth: texture_depth_2d;
@group(3) @binding(1) var underwater_probe: texture_2d<f32>;

// The spatial filter's deviation (px) over a pixel holding one sample, the samples from which it draws a pixel as it is,
// the farthest neighbour it reads (px each way) and the weight of a pixel holding no sample of its own. Until
// 2026-10-05 the deviation was 1 px, unweighted, and a pixel with no sample was drawn as it was: a camera travelling
// past a near cloud left a snow of single samples on it (`resolve.wgsl`, the sliding view). 7 × 7 at most measured a
// little smoother and cost 0.2 ms more while a view moves (1120 × 630).
const FILTER_DEVIATION: f32 = 2.5;
const FILTER_SAMPLES: f32 = 8.0;
const FILTER_REACH: i32 = 2;
const FILTER_FLOOR: f32 = 0.25;
// How far apart (a share of the farther) two view depths of the opaque scene may be and still be one surface.
const SURFACE_TOLERANCE: f32 = 0.2;

struct CompositeVertex {
    @builtin(position) position: vec4<f32>,
}

@vertex
fn composite_vertex(@builtin(vertex_index) index: u32) -> CompositeVertex {
    let clip = vec2<f32>(select(-1.0, 3.0, index == 1u), select(-1.0, 3.0, index == 2u));
    return CompositeVertex(vec4<f32>(clip, 0.0, 1.0));
}

// Metres along the view of a reversed depth (`naval::camera::view_depth`).
fn view_depth(depth: f32) -> f32 {
    let near = clouds.planes.x;
    let far = clouds.planes.y;
    return far * near / (depth * (far - near) + near);
}

// The ray through a fragment of the drawing buffer, built as the march builds it (`march_pixel`).
fn pixel_ray(position: vec2<f32>) -> vec3<f32> {
    let size = clouds.viewport.xy;
    let uv = position / size;
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let tangent = 1.0 / clouds.forward.w;
    let view_ray = vec3<f32>(ndc.x * tangent * size.x / size.y, ndc.y * tangent, -1.0);
    let rotation = mat3x3<f32>(clouds.view[0].xyz, clouds.view[1].xyz, clouds.view[2].xyz);
    return normalize(transpose(rotation) * view_ray);
}

// Metres along the ray to the bottom of the lowest layer from under it; 0 from inside or above the layers.
fn cloud_entry(origin: vec3<f32>, direction: vec3<f32>) -> f32 {
    if clouds.camera.w >= clouds.heights.x {
        return 0.0;
    }
    return max(ray_sphere_second(origin, direction, clouds.sun.w + clouds.heights.x), 0.0);
}

// The haze's opacity over `distance` metres of the ray (`approximate_haze`'s, which needs only its density).
fn haze_opacity(origin: vec3<f32>, direction: vec3<f32>, distance: f32) -> f32 {
    if clouds.switches.z < 0.5 {
        return 0.0;
    }
    let camera_height = clouds.camera.w;
    let modulation = clamp((clouds.medium.z - 0.2) / 0.2, 0.0, 1.0);
    if camera_height * modulation < 0.0 {
        return 0.0;
    }
    let density = modulation * clouds.haze.x * exp(-camera_height * clouds.haze.y);
    if density < 1e-7 {
        return 0.0;
    }
    let normal_at_origin = normalize(origin);
    let normal_at_horizon = (origin - dot(origin, direction) * direction) / clouds.sun.w;
    let blend = clamp((dot(normal_at_origin, normal_at_horizon) - 0.9) / 0.1, 0.0, 1.0);
    let normal = mix(normal_at_origin, normal_at_horizon, blend);
    let angle = max(dot(normal, direction), 1e-5);
    let optical_depth = (1.0 - exp(-distance * angle * clouds.haze.y)) * density / clouds.haze.y / angle;
    return clamp(1.0 - exp(-optical_depth), 0.0, 1.0);
}

// Whether a pixel that saw the opaque scene `other` km along the view (0: none) saw the surface of one at `own`.
fn same_surface(other: f32, own: f32) -> bool {
    if other <= 0.0 || own <= 0.0 {
        return other <= 0.0 && own <= 0.0;
    }
    return abs(other - own) <= SURFACE_TOLERANCE * max(other, own);
}

// The reconstructed clouds at a pixel, through the spatial filter while its mean holds few samples.
fn cloud_at(pixel: vec2<u32>) -> vec4<f32> {
    let color = textureLoad(cloud_color, pixel, 0);
    let state = textureLoad(cloud_state, pixel, 0);
    let samples = state.g;
    if samples >= FILTER_SAMPLES {
        return color;
    }
    let deviation = FILTER_DEVIATION / sqrt(max(samples, 1.0));
    let reach = clamp(i32(ceil(deviation * 1.5)), 1, FILTER_REACH);
    let falloff = -0.5 / (deviation * deviation);
    let top = vec2<i32>(textureDimensions(cloud_color)) - 1;
    var weight = max(samples, FILTER_FLOOR);
    var sum = color * weight;
    for (var y = -reach; y <= reach; y++) {
        for (var x = -reach; x <= reach; x++) {
            if x == 0 && y == 0 {
                continue;
            }
            let at = clamp(vec2<i32>(pixel) + vec2<i32>(x, y), vec2<i32>(0), top);
            let other = textureLoad(cloud_state, at, 0);
            if !same_surface(other.b, state.b) {
                continue;
            }
            let w = exp(f32(x * x + y * y) * falloff) * max(other.g, FILTER_FLOOR);
            sum += textureLoad(cloud_color, at, 0) * w;
            weight += w;
        }
    }
    return sum / weight;
}

// Bilinear upscaling with the reconstruction's scene-depth classification. A sky
// pixel must not blend in a neighbour whose cloud ray stopped at foreground geometry.
// The full-resolution path retains its existing sample-count spatial filter.
fn cloud_upscaled(uv: vec2<f32>, scene_z: f32) -> vec4<f32> {
    let size = textureDimensions(cloud_color);
    let position = uv * vec2<f32>(size) - 0.5;
    let base = vec2<i32>(floor(position));
    let fraction = fract(position);
    var sum = vec4<f32>(0.0);
    var total = 0.0;
    for (var y = 0; y < 2; y++) {
        for (var x = 0; x < 2; x++) {
            let at = clamp(base + vec2<i32>(x, y), vec2<i32>(0), vec2<i32>(size) - 1);
            let state = textureLoad(cloud_state, at, 0);
            if !same_surface(state.b, scene_z * 0.001) {
                continue;
            }
            let wx = select(1.0 - fraction.x, fraction.x, x == 1);
            let wy = select(1.0 - fraction.y, fraction.y, y == 1);
            let weight = wx * wy;
            sum += textureLoad(cloud_color, at, 0) * weight;
            total += weight;
        }
    }
    if total > 1e-5 {
        return sum / total;
    }
    let pixel = min(vec2<u32>(uv * vec2<f32>(size)), size - 1u);
    return cloud_at(pixel);
}

@fragment
fn composite_fragment(in: CompositeVertex) -> @location(0) vec4<f32> {
    let uv = in.position.xy / clouds.viewport.xy;
    if clouds.experiments.x > 4.5 {
        let size = textureDimensions(opaque_depth);
        let pixel = min(vec2<u32>(uv * vec2<f32>(size)), size - 1u);
        let depth = textureLoad(opaque_depth, pixel, 0);
        if depth <= 0.0 { return vec4<f32>(0.0, 0.05, 0.3, 1.0); }
        let distance = view_depth(depth);
        return vec4<f32>(vec3<f32>(1.0 - exp(-distance / 100.0)), 1.0);
    }
    if ray_starts_under_water(underwater_probe, uv) {
        return vec4<f32>(0.0);
    }
    let depth_size = textureDimensions(opaque_depth);
    let depth_pixel = min(vec2<u32>(uv * vec2<f32>(depth_size)), depth_size - 1u);
    let depth = textureLoad(opaque_depth, depth_pixel, 0);
    var scene_z = 0.0;
    var maximum_opacity = 1e20;
    if depth > 0.0 {
        scene_z = view_depth(depth);
        let direction = pixel_ray(in.position.xy);
        let near = clouds.planes.x;
        let origin = camera_planet() + direction * near;
        let entry = cloud_entry(origin, direction);
        let distance = scene_z / max(dot(direction, clouds.forward.xyz), 1e-4) - near;
        if entry > 0.0 && distance < entry {
            maximum_opacity = haze_opacity(origin, direction, max(distance, 0.0));
            // Most foreground geometry cannot have cloud in front of it. Reject
            // before the upscaling / up-to-5x5 spatial filter, including at 100%.
            if maximum_opacity <= 0.0 {
                return vec4<f32>(0.0);
            }
        }
    }
    let size = textureDimensions(cloud_color);
    var color: vec4<f32>;
    if all(vec2<f32>(size) == clouds.viewport.xy) {
        color = cloud_at(min(vec2<u32>(in.position.xy), size - 1u));
    } else {
        color = cloud_upscaled(uv, scene_z);
    }
    if color.a <= 0.0 {
        return color;
    }
    return color * min(maximum_opacity / color.a, 1.0);
}
