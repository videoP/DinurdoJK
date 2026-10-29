// The clouds' temporal upscaling (`cloudsResolve.frag` of @takram/three-clouds), made to converge: the march gives
// one pixel of each 4 × 4 block a sample per update, and every pixel of the drawing buffer keeps the running mean of
// its own samples and how many it holds (up to MOST_SAMPLES, or MOST_MOVED_SAMPLES where the view moved it; takram,
// and this port until 2026-10-03, kept little more than the latest, so one sample's noise stood in every still as
// grain). Its history is reprojected by the motion of the march pixels about it (their clouds' drift included) and
// clipped to the variance of the march around it, both weighted smoothly by distance, so neither steps at the blocks'
// edges (under a moving view, and binoculars above all, the steps printed the 4 × 4 blocks). A history the box finds
// well out loses samples by how far, and one whose surface changed (a hull moved off the sky, or onto it) starts
// again. The light shafts' lengths go the same way; the composite draws a pixel holding few samples through a small
// spatial filter (`composite.wgsl`).
//
// A pixel with few samples of its own (the sky a turning view has just brought in, a cut's first second, a history the
// box threw out) leans on a prior: the march about it, the same smooth weighting, gathered over the updates until it
// holds PRIOR_WEIGHT samples' worth, and giving way as the pixel's own samples arrive. Until 2026-10-04 such a pixel
// followed the march buffer upsampled, half a new estimate every update, and then jumped to its first sample as it
// was: a view turning at a mouse's pace holds most of its sky at three samples or fewer (the march reaches a pixel
// once in 16 updates, and the sky crosses the screen in one or two seconds), so the clouds boiled as it turned.
//
// Where a pixel's 3 × 3 march neighbourhood straddles the opaque scene's silhouette (a mast or a funnel against the
// clouds), only the march pixels that saw what the pixel sees count: a ray stopped at a ship carries no cloud and the
// ship's motion, and taken into a sky pixel's prior, motion or clipping box it left a cloud-free halo along the
// silhouette, widest while the view turns (takram's "disocclusion errors on scene objects").
//
// Where the view slides near cloud across what lies behind it (a camera travelling a few hundred metres from a cumulus:
// an aircraft's, the free camera's), a pixel's clouds are no one surface that moved: the near part of the volume
// passes the far part, the cloud's fringe over the sky, and no one motion carries the history. The reprojected mean
// then held another place's clouds, a few samples of its own beside them (the march reaches a pixel once in 16 updates,
// and a travelling view keeps bringing in what it has not seen), and the cloud showed as a snow of samples and a torn
// fringe. Such a pixel (the march about it moving SLIDE_PARALLAX against the view's turn alone) takes SLIDE_BLEND of
// the march about it each update, through a Gaussian of SLIDE_DEVIATION march texels, and its mean holds at most
// SLIDE_SAMPLES of its own: the near cloud is drawn from the march as it stands, softer and steady. A turning view, a
// ship's camera and the clouds' drift move nothing near enough to count.
//
// The shafts' history carries the pixel's state beside the length (r, km): g the samples of its own its mean holds, b
// the view depth (km) of the opaque scene it saw (0 for none), so a reprojected history that saw another surface is
// dropped, a the prior's weight in the mean.
//
// MIT License, Copyright (c) 2024 Shota Matsuda (https://github.com/takram-design-engineering/three-geospatial). The
// full notice is in `naval-clouds/THIRD_PARTY.md`. The variance clipping follows Salvi (GDC 2016) and Playdead's
// "temporal" (MIT).

#import naval_clouds::common::{clouds, clamp_sampler, METER_TO_KM}

// The opaque scene's reversed depth, the composite's (binding 1, the underwater probe, is not read here).
@group(1) @binding(0) var opaque_depth: texture_depth_2d;
@group(3) @binding(0) var march_color: texture_2d<f32>;
@group(3) @binding(1) var march_depth_velocity: texture_2d<f32>;
@group(3) @binding(2) var march_shadow_length: texture_2d<f32>;
@group(3) @binding(3) var history_color: texture_2d<f32>;
@group(3) @binding(4) var history_shadow_length: texture_2d<f32>;
@group(3) @binding(5) var color_out: texture_storage_2d<rgba16float, write>;
@group(3) @binding(6) var shadow_length_out: texture_storage_2d<rgba16float, write>;

const VARIANCE_GAMMA: f32 = 2.0;
// The variance gamma where the reprojection moved the pixel less than a quarter of a pixel: wide enough that a still
// pixel keeps what it gathered, narrow enough that a changed cloud is clipped out of a uniform neighbourhood.
const STILL_GAMMA: f32 = 8.0;
// The most samples a pixel's mean holds: 16 updates each, about four seconds at 60 frames a second, where the view
// has not moved the pixel (a held or paused picture, whose samples step through the jitter evenly); half that where it
// has (the view turning, a ship's camera, the clouds' drift). A moved history is resampled every update and its
// samples come from other pixels' jitter; over the longer span it gathered the blur of those resamplings and of the
// clouds' evolving shapes, and measured against the clouds marched at 32 jitters the shorter one is as near or nearer
// in every view and keeps the cloud edges a tenth sharper.
const MOST_SAMPLES: f32 = 16.0;
const MOST_MOVED_SAMPLES: f32 = 8.0;
// How far apart (a share of the farther) two view depths of the opaque scene may be and still be one surface.
const SURFACE_TOLERANCE: f32 = 0.2;
// The prior: the march about a pixel counts as PRIOR_WEIGHT samples in the mean of a pixel that has none of its own
// (one update's is about three samples' worth of noise, but a quarter-resolution picture: weighed by its noise alone it
// would hold the clouds soft for seconds), less as the pixel's own arrive, none from PRIOR_SAMPLES on; and it gathers
// PRIOR_STEP of weight an update, so a new pixel's prior is the mean of eight updates' marches, each on another cell.
const PRIOR_WEIGHT: f32 = 2.0;
const PRIOR_SAMPLES: f32 = 4.0;
const PRIOR_STEP: f32 = 0.25;
// The sliding view (above): from SLIDE_PARALLAX.x to .y pixels an update of parallax a pixel goes from none of it to
// all; the share of the march's Gaussian (SLIDE_DEVIATION texels across, in a 5 × 5 window) taken an update, and the
// samples of its own a mean keeps. Measured flying 100 m/s past a cumulus 300 m off (Medium, 1120 × 630): the frames'
// mean error against the clouds converged at the same place 2.97 → 1.87 levels with the near step and the composite's
// filter, their grain a few pixels across 1.62 → 1.13 times the converged clouds'. A wider Gaussian or a larger share
// measured the same.
const SLIDE_PARALLAX: vec2<f32> = vec2<f32>(0.25, 1.5);
const SLIDE_BLEND: f32 = 0.25;
const SLIDE_DEVIATION: f32 = 1.25;
const SLIDE_SAMPLES: f32 = 2.0;

// Metres along the view of a reversed depth (`naval::camera::view_depth`).
fn view_depth(depth: f32) -> f32 {
    let near = clouds.planes.x;
    let far = clouds.planes.y;
    return far * near / (depth * (far - near) + near);
}

// Metres along the view to the opaque scene through a pixel of the drawing buffer; 0 where there is none (sky or sea).
fn pixel_scene_view_z(pixel: vec2<u32>) -> f32 {
    let size = textureDimensions(opaque_depth);
    let uv = (vec2<f32>(pixel) + 0.5) / clouds.screen.xy;
    let at = min(vec2<u32>(uv * vec2<f32>(size)), size - 1u);
    let depth = textureLoad(opaque_depth, at, 0);
    if depth <= 0.0 {
        return 0.0;
    }
    return view_depth(depth);
}

// Whether a surface `texel_z` along the view (0: none) is the one a pixel at `pixel_z` (the same unit) sees.
fn same_surface(texel_z: f32, pixel_z: f32) -> bool {
    if texel_z <= 0.0 || pixel_z <= 0.0 {
        return texel_z <= 0.0 && pixel_z <= 0.0;
    }
    return abs(texel_z - pixel_z) <= SURFACE_TOLERANCE * max(texel_z, pixel_z);
}

// The quadratic B-spline of radius 1.5 march texels: the 3 × 3 window holds all of it but the corners' last eighth of
// a texel, so the weights hardly change where the window steps from one block to the next.
fn spline(t: f32) -> f32 {
    let a = abs(t);
    if a < 0.5 {
        return 0.75 - a * a;
    }
    let b = max(1.5 - a, 0.0);
    return 0.5 * b * b;
}

// Weighted sums over march pixels.
struct Moments {
    weight: f32,
    color: vec4<f32>,
    color_moment2: vec4<f32>,
    shadow_length: f32,
    length_moment2: f32,
    velocity: vec2<f32>,
}

fn add(moments: ptr<function, Moments>, color: vec4<f32>, shadow_length: f32, depth_velocity: vec4<f32>, weight: f32) {
    (*moments).weight += weight;
    (*moments).color += color * weight;
    (*moments).color_moment2 += color * color * weight;
    (*moments).shadow_length += shadow_length * weight;
    (*moments).length_moment2 += shadow_length * shadow_length * weight;
    (*moments).velocity += depth_velocity.yz * weight;
}

// The march pixels of the 3 × 3 neighbourhood that saw the drawing-buffer pixel's surface (all of them where none did):
// their colour and shafts' length with those values' second moments, and their motion, weighted by the spline about
// the pixel. takram reprojects by the nearest march pixel's motion
// (`getClosestFragment`), which steps from one 4 × 4 block to the next; where the motion varies over the view (a
// drifting cloud, a moving camera's parallax: both go as one over the distance) a long history sheared along those steps
// until the clipping tore it up.
struct Neighbourhood {
    color: vec4<f32>,
    color_moment2: vec4<f32>,
    shadow_length: f32,
    length_moment2: f32,
    velocity: vec2<f32>,
    // The most any of those march pixels moved against the view's turn alone through its own ray (the turn's motion
    // varies over the view: a fast turn's differs by most of a pixel across a march pixel near the edges): pixels an
    // update.
    parallax: f32,
}

fn neighbourhood(pixel: vec2<u32>, low: vec2<i32>, pixel_z: f32) -> Neighbourhood {
    let top = vec2<i32>(clouds.screen.zw) - 1;
    // The pixel in the march buffer's texels, their centres at whole numbers.
    let position = (vec2<f32>(pixel) + 0.5) / 4.0 - 0.5;
    let empty = Moments(0.0, vec4<f32>(0.0), vec4<f32>(0.0), 0.0, 0.0, vec2<f32>(0.0));
    var every = empty;
    var same = empty;
    var every_parallax = 0.0;
    var same_parallax = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let texel = clamp(low + vec2<i32>(x, y), vec2<i32>(0), top);
            let depth_velocity = textureLoad(march_depth_velocity, texel, 0);
            let color = textureLoad(march_color, texel, 0);
            let shadow_length = textureLoad(march_shadow_length, texel, 0).r;
            let offset = vec2<f32>(low + vec2<i32>(x, y)) - position;
            let weight = spline(offset.x) * spline(offset.y) + 1e-4;
            // The drawing-buffer pixel this update marched for the texel (`march_main`).
            let marched = min(vec2<u32>(texel) * 4u + vec2<u32>(clouds.frame.yz), vec2<u32>(clouds.screen.xy) - 1u);
            let turn = turn_velocity((vec2<f32>(marched) + 0.5) / clouds.screen.xy);
            let parallax = length((depth_velocity.yz - turn) * clouds.screen.xy);
            add(&every, color, shadow_length, depth_velocity, weight);
            every_parallax = max(every_parallax, parallax);
            if same_surface(depth_velocity.a, pixel_z) {
                add(&same, color, shadow_length, depth_velocity, weight);
                same_parallax = max(same_parallax, parallax);
            }
        }
    }
    var chosen = every;
    var parallax = every_parallax;
    if same.weight > 0.0 {
        chosen = same;
        parallax = same_parallax;
    }
    let scale = 1.0 / chosen.weight;
    return Neighbourhood(
        chosen.color * scale,
        chosen.color_moment2 * scale,
        chosen.shadow_length * scale,
        chosen.length_moment2 * scale,
        chosen.velocity * scale,
        parallax,
    );
}

// The motion (uv an update) of the sky at infinity through a pixel at `uv`: the view's turn alone, as the march takes
// a ray that met nothing (`march_pass.wgsl`).
fn turn_velocity(uv: vec2<f32>) -> vec2<f32> {
    let size = clouds.screen.xy;
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let tangent = 1.0 / clouds.forward.w;
    let view_ray = vec3<f32>(ndc.x * tangent * clouds.viewport.x / clouds.viewport.y, ndc.y * tangent, -1.0);
    let rotation = mat3x3<f32>(clouds.view[0].xyz, clouds.view[1].xyz, clouds.view[2].xyz);
    let previous = clouds.previous_view_projection * vec4<f32>(transpose(rotation) * view_ray, 0.0);
    let previous_ndc = previous.xy / previous.w;
    return uv - vec2<f32>(previous_ndc.x * 0.5 + 0.5, 0.5 - previous_ndc.y * 0.5);
}

// The march about a pixel through a Gaussian of SLIDE_DEVIATION march texels in a 5 × 5 window, the march pixels that
// saw the pixel's surface (all of them where none did).
fn march_smooth(pixel: vec2<u32>, low: vec2<i32>, pixel_z: f32) -> vec4<f32> {
    let top = vec2<i32>(clouds.screen.zw) - 1;
    let position = (vec2<f32>(pixel) + 0.5) / 4.0 - 0.5;
    let falloff = -0.5 / (SLIDE_DEVIATION * SLIDE_DEVIATION);
    var every = vec4<f32>(0.0);
    var every_weight = 0.0;
    var same = vec4<f32>(0.0);
    var same_weight = 0.0;
    for (var y = -2; y <= 2; y++) {
        for (var x = -2; x <= 2; x++) {
            let texel = clamp(low + vec2<i32>(x, y), vec2<i32>(0), top);
            let offset = vec2<f32>(low + vec2<i32>(x, y)) - position;
            let weight = exp(dot(offset, offset) * falloff);
            let color = textureLoad(march_color, texel, 0);
            every += color * weight;
            every_weight += weight;
            if same_surface(textureLoad(march_depth_velocity, texel, 0).a, pixel_z) {
                same += color * weight;
                same_weight += weight;
            }
        }
    }
    if same_weight > 0.0 {
        return same / same_weight;
    }
    return every / every_weight;
}

// The history at `uv` through a Catmull–Rom filter in nine bilinear reads (takram's `textureCatmullRom`, after MJP's
// gist, which takram leaves commented out for a bilinear read): a moving view resamples its history every update, and
// a bilinear read low-passes it each time, softening the clouds while the camera moves.
fn history_catmull_rom(uv: vec2<f32>) -> vec4<f32> {
    let size = clouds.screen.xy;
    let sample_position = uv * size;
    let position1 = floor(sample_position - 0.5) + 0.5;
    let f = sample_position - position1;
    let w0 = f * (-0.5 + f * (1.0 - 0.5 * f));
    let w1 = 1.0 + f * f * (-2.5 + 1.5 * f);
    let w2 = f * (0.5 + f * (2.0 - 1.5 * f));
    let w3 = f * f * (-0.5 + 0.5 * f);
    let w12 = w1 + w2;
    let offset12 = w2 / w12;
    let p0 = (position1 - 1.0) / size;
    let p3 = (position1 + 2.0) / size;
    let p12 = (position1 + offset12) / size;
    var result = vec4<f32>(0.0);
    result += textureSampleLevel(history_color, clamp_sampler, vec2<f32>(p0.x, p0.y), 0.0) * w0.x * w0.y;
    result += textureSampleLevel(history_color, clamp_sampler, vec2<f32>(p12.x, p0.y), 0.0) * w12.x * w0.y;
    result += textureSampleLevel(history_color, clamp_sampler, vec2<f32>(p3.x, p0.y), 0.0) * w3.x * w0.y;
    result += textureSampleLevel(history_color, clamp_sampler, vec2<f32>(p0.x, p12.y), 0.0) * w0.x * w12.y;
    result += textureSampleLevel(history_color, clamp_sampler, vec2<f32>(p12.x, p12.y), 0.0) * w12.x * w12.y;
    result += textureSampleLevel(history_color, clamp_sampler, vec2<f32>(p3.x, p12.y), 0.0) * w3.x * w12.y;
    result += textureSampleLevel(history_color, clamp_sampler, vec2<f32>(p0.x, p3.y), 0.0) * w0.x * w3.y;
    result += textureSampleLevel(history_color, clamp_sampler, vec2<f32>(p12.x, p3.y), 0.0) * w12.x * w3.y;
    result += textureSampleLevel(history_color, clamp_sampler, vec2<f32>(p3.x, p3.y), 0.0) * w3.x * w3.y;
    // The filter's negative lobes can overshoot below zero at hard edges.
    return max(result, vec4<f32>(0.0));
}

// A history clipped into the box mean ± gamma × deviation (Playdead's clip toward the box's centre: the radiance sets
// how far, the opacity follows), and how far out it was: 1 or less inside, 2 at twice the box's half-width. The box is
// at least a hundredth of the mean wide, so a flat neighbourhood (a clear sky, a deck's flat base) does not read the
// last bits of a history as far out.
struct Clipped {
    value: vec4<f32>,
    out: f32,
}

fn clip_to_variance(mean: vec4<f32>, moment2: vec4<f32>, history: vec4<f32>, gamma: f32) -> Clipped {
    let deviation = sqrt(max(moment2.rgb - mean.rgb * mean.rgb, vec3<f32>(0.0)));
    let extent = max(deviation * gamma, abs(mean.rgb) * 0.01) + 1e-7;
    let offset = history - mean;
    let ratios = abs(offset.rgb) / extent;
    let out = max(ratios.x, max(ratios.y, ratios.z));
    if out > 1.0 {
        return Clipped(mean + offset / out, out);
    }
    return Clipped(history, out);
}

fn clip_length(mean: f32, moment2: f32, history: f32, gamma: f32) -> f32 {
    let extent = sqrt(max(moment2 - mean * mean, 0.0)) * gamma + 1e-7;
    return clamp(history, mean - extent, mean + extent);
}

fn store(pixel: vec2<u32>, color: vec4<f32>, shadow_length: f32, samples: f32, prior: f32, pixel_z: f32) {
    textureStore(color_out, pixel, color);
    textureStore(shadow_length_out, pixel, vec4<f32>(shadow_length, samples, pixel_z * METER_TO_KM, prior));
}

// The weight the prior holds in the mean of a pixel with `samples` of its own.
fn prior_share(samples: f32) -> f32 {
    return PRIOR_WEIGHT * max(1.0 - samples / PRIOR_SAMPLES, 0.0);
}

@compute @workgroup_size(8, 8, 1)
fn resolve_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pixel = id.xy;
    if any(pixel >= vec2<u32>(clouds.screen.xy)) {
        return;
    }
    let low = pixel / 4u;
    let uv = (vec2<f32>(pixel) + 0.5) / clouds.screen.xy;
    let pixel_z = pixel_scene_view_z(pixel);
    // The pixel marched this update saw its surface by construction.
    let current_frame = all(pixel % 4u == vec2<u32>(clouds.frame.yz));
    let near = neighbourhood(pixel, vec2<i32>(low), pixel_z);

    // The history: its mean, the samples of the pixel's own it holds and the prior's weight beside them. None without
    // one to reproject (a cut), off its edge, or where it saw another surface (a hull that moved off this sky, or onto
    // it).
    var history = vec4<f32>(0.0);
    var history_length = 0.0;
    var samples = 0.0;
    var prior = 0.0;
    var most = MOST_SAMPLES;
    let previous_uv = uv - near.velocity;
    if clouds.frame.w > 0.5 && all(previous_uv >= vec2<f32>(0.0)) && all(previous_uv <= vec2<f32>(1.0)) {
        // A large variance gamma works for upsampling; it ghosts more, which clouds hide. Where the view has not moved
        // the pixel (under a pixel of motion: a steady or slowly moving camera, the clouds far off) the box opens wider
        // still: the march buffer's neighbourhood is a quarter-resolution picture, and clipping the history to it drags
        // the detail the samples gathered back toward that picture.
        let motion = length(near.velocity * clouds.screen.xy);
        // A lightning flash changes the clouds' light from one update to the next: clipped as a moving view's, the
        // history takes the flash and lets it go with the march.
        let gamma = mix(STILL_GAMMA, VARIANCE_GAMMA, max(smoothstep(0.25, 1.0, motion), clouds.flash.x));
        // A pixel the view did not move reads its own history texel; one moved under half a pixel (a drifting cloud, a
        // slow turn), bilinearly; one moved further, through the Catmull–Rom filter, which keeps the clouds crisper
        // while the camera moves. Under a slow drift the Catmull–Rom read keeps the noise of samples the history
        // carried in from other pixels (and prints the order the cells were marched in as a faint 4 × 4 lattice), where
        // the bilinear read smooths it; measured against the clouds marched at 32 jitters, it is the nearer below half
        // a pixel and the Catmull–Rom read above.
        var reprojected = textureLoad(history_color, pixel, 0);
        if motion >= 0.01 {
            reprojected = textureSampleLevel(history_color, clamp_sampler, previous_uv, 0.0);
            let sharp = smoothstep(0.3, 0.7, motion);
            if sharp > 0.0 {
                reprojected = mix(reprojected, history_catmull_rom(previous_uv), sharp);
            }
            most = MOST_MOVED_SAMPLES;
        }
        let state = textureSampleLevel(history_shadow_length, clamp_sampler, previous_uv, 0.0);
        if same_surface(state.b, pixel_z * METER_TO_KM) {
            let clipped = clip_to_variance(near.color, near.color_moment2, reprojected, gamma);
            // A history well out of the box keeps less of its weight: all to half its half-width out (the box's own
            // noise), none from two and a half, so a changed cloud takes the new samples at once.
            let kept = clamp(2.5 - clipped.out, 0.0, 1.0);
            history = clipped.value;
            history_length = clip_length(near.shadow_length, near.length_moment2, state.r, gamma);
            samples = state.g * kept;
            prior = state.a * kept;
        }
    }

    // A pixel the view slides near cloud across takes the march about it as it stands, and keeps few samples of its own.
    let sliding = smoothstep(SLIDE_PARALLAX.x, SLIDE_PARALLAX.y, near.parallax);
    if sliding > 0.0 {
        history = mix(history, march_smooth(pixel, vec2<i32>(low), pixel_z), SLIDE_BLEND * sliding);
        samples = min(samples, mix(most, SLIDE_SAMPLES, sliding));
    }

    // The prior joins the mean of a pixel that holds less of it than its samples call for (all of it, where the pixel
    // holds nothing).
    let fill = clamp(prior_share(samples) - prior, 0.0, PRIOR_STEP);
    if fill > 0.0 {
        let weight = fill / (samples + prior + fill);
        history = mix(history, near.color, weight);
        history_length = mix(history_length, near.shadow_length, weight);
        prior += fill;
    }
    if current_frame {
        // The new sample into the pixel's running mean. The prior's weight then falls to its share for the new count
        // (the mean keeps what it gathered): the samples that follow weigh the more, and the prior fades out.
        samples = min(samples + 1.0, most);
        let weight = 1.0 / (samples + prior);
        history = mix(history, textureLoad(march_color, low, 0), weight);
        history_length = mix(history_length, textureLoad(march_shadow_length, low, 0).r, weight);
        prior = min(prior, prior_share(samples));
    }
    store(pixel, history, history_length, samples, prior, pixel_z);
}
