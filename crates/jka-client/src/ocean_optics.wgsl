// Bounded Beer-Lambert medium, captured before water and diagnostic overlays.
struct SkyFogRegion {
    bounds: vec4<f32>,
    plane: vec4<f32>,
    segments: array<vec4<f32>, 16>,
};
struct Optics {
    inverse_view_projection: mat4x4<f32>,
    eye: vec4<f32>,
    forward: vec4<f32>,
    fog: vec4<f32>,
    params: vec4<f32>,
    minimum: array<vec4<f32>, 8>,
    maximum: array<vec4<f32>, 8>,
    sky_regions: array<SkyFogRegion, 8>,
    sky_info: vec4<f32>,
    caustic_tuning: vec4<f32>,
    procedural_params: vec4<f32>,
    eye_wave: vec4<f32>,
};
COLOR_DECLARATION
@group(0) @binding(1) var scene_sampler: sampler;
DEPTH_DECLARATION
@group(0) @binding(3) var<uniform> optics: Optics;
@group(0) @binding(4) var caustic_patterns: texture_2d_array<f32>;
@group(0) @binding(5) var caustic_pattern_sampler: sampler;
// OCEAN_STRUCT
// Existing Blue Water adapter displacement. Used ONLY in the specialized
// Hybrid mode-4 optics shader, never in ordinary Godot/native capture.
@group(1) @binding(0) var wave_displacement: texture_2d_array<f32>;
@group(1) @binding(1) var wave_normal: texture_2d_array<f32>;
@group(1) @binding(2) var wave_sampler: sampler;
@group(1) @binding(3) var<uniform> ocean: OceanRenderSettings;

// Depth-unprojected receivers can fall just outside their water bounds. Pool
// walls commonly lie exactly on those bounds (e.g. hothchallenges y=7728), so
// strict containment made caustics toggle per pixel like a coplanar overlay.
// Match the outline's existing tolerance, in map units. Only receiver tests
// use this margin; ray intervals and absorption retain the actual volume.
const CAUSTIC_RECEIVER_MARGIN: f32 = 0.75;
// Replaced at pipeline specialization time; false compiles all extra wave
// reads and classification out of non-Hybrid optics.
const JKA_HYBRID_WAVE: bool = false;
// Replaced for submerged Hybrid mode 4 (`Resources::ensure_submerged_fog_capture`):
// the eye is in water, so a pixel is fogged over its ray up to the water top
// over the eye (`optics.eye.w`), and the per-volume span solve compiles out.
const JKA_SUBMERGED_FOG: bool = false;

// Read the SAME Blue Water simulation adapter passed to the world water
// material. Sampling varies with the ray's WORLD XZ, NOT one camera height
// pasted onto a whole frame. Away from the eye this reads the two large
// cascades only (1024 m and 181 m tiles); the 31 m detail cascade and the
// horizontal-displacement inversion that the mesh applies are folded into the
// eye term: the CPU probe (see ocean/eye_wave.rs) solves the exact rasterized
// surface over the eye, and `optics.eye_wave.x` is its difference from this
// function's naive value there. That makes the waterline AT THE EYE (the tint
// switch) agree with the mesh and with the CPU cull, fading out over ~30 m.
fn jka_wave_height(position_xz: vec2<f32>, flat_plane: f32) -> f32 {
    if (!JKA_HYBRID_WAVE) { return flat_plane; }
    let p = position_xz / max(ocean.ocean_info.z, 0.001);
    var displacement_y = 0.0;
    if ocean.map_scales[0].z > 0.0 {
    for (var i = 0u; i < 2u; i += 1u) {
        let scale = ocean.map_scales[i];
        let uv = p * vec2<f32>(1.0, -1.0) * scale.xy;
        displacement_y += textureSampleLevel(wave_displacement, wave_sampler, uv, i32(i), 0.0).y * scale.z;
    }
    }
    // The same analytic swell as the Hybrid Godot mesh and native geometry.
    // The eye-probe correction below accounts for native stochastic FFT tiling,
    // the third cascade and displaced-grid inversion near the camera.
    let phase = dot(p, ocean.swell.xy) * ocean.swell.z - ocean.swell_motion.x * ocean.swell_motion.w;
    var height = flat_plane + (displacement_y + ocean.swell.w * sin(phase)) * ocean.ocean_info.z;
    if (optics.eye_wave.y > 0.5) {
        let near = 1.0 - smoothstep(0.0, 1200.0, distance(position_xz, optics.eye.xz));
        height += optics.eye_wave.x * near;
    }
    return height;
}

fn jka_wave_signed_depth(t: f32, delta: vec3<f32>, flat_plane: f32) -> f32 {
    let p = optics.eye.xyz + delta * t;
    return jka_wave_height(p.xz, flat_plane) - p.y;
}

// Clip a ray against the animated upper boundary, sampling along THIS ray.
// A secant estimate (not midpoint bisection!) is essential: the ray may travel
// thousands of map units while the crest is only tens of units above the eye.
// Taking the midpoint would create precisely the giant optical walls that the
// earlier broken fix produced. One refinement catches local wave curvature.
fn jka_wave_wet_span(span: vec2<f32>, delta: vec3<f32>, flat_plane: f32) -> vec2<f32> {
    if (!JKA_HYBRID_WAVE) {
        return sky_fog_clip(span, optics.eye.y, delta.y, flat_plane);
    }
    if (span.y <= span.x) { return vec2<f32>(1.0, 0.0); }
    var a = span.x;
    var b = span.y;
    var da = jka_wave_signed_depth(a, delta, flat_plane);
    var db = jka_wave_signed_depth(b, delta, flat_plane);
    if (da >= 0.0 && db >= 0.0) { return span; }
    if (da < 0.0 && db < 0.0) { return vec2<f32>(1.0, 0.0); }
    let start_wet = da >= 0.0;
    // Two secant steps, bounded and GPU-only (four wave reads including ends).
    // Compared with sampling a wave at every screen ray step, this has fixed
    // work while keeping the critical camera-side crossing precise.
    for (var i = 0u; i < 2u; i += 1u) {
        let weight = abs(da) / max(abs(da) + abs(db), 1.0e-5);
        let t = mix(a, b, weight);
        let dt = jka_wave_signed_depth(t, delta, flat_plane);
        if ((dt >= 0.0) == start_wet) {
            a = t;
            da = dt;
        } else {
            b = t;
            db = dt;
        }
    }
    let crossing = mix(a, b, abs(da) / max(abs(da) + abs(db), 1.0e-5));
    return select(vec2<f32>(crossing, span.y), vec2<f32>(span.x, crossing), start_wet);
}

@vertex fn vs_main(@builtin(vertex_index) id: u32) -> @builtin(position) vec4<f32> {
    let xy = vec2<f32>(f32((id << 1u) & 2u), f32(id & 2u));
    return vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0);
}
fn interval(a: vec3<f32>, delta: vec3<f32>, lo: vec3<f32>, hi: vec3<f32>) -> vec2<f32> {
    var near = 0.0;
    var far = 1.0;
    for (var axis = 0u; axis < 3u; axis += 1u) {
        if (abs(delta[axis]) < 0.00001) {
            if (a[axis] < lo[axis] || a[axis] > hi[axis]) { return vec2<f32>(1.0, 0.0); }
        } else {
            let t0 = (lo[axis] - a[axis]) / delta[axis];
            let t1 = (hi[axis] - a[axis]) / delta[axis];
            near = max(near, min(t0, t1));
            far = min(far, max(t0, t1));
        }
    }
    return vec2<f32>(near, far);
}

// The surface outline (see OceanMasks): a box is only the bounding rectangle of
// its water, so an island ring's hollow must not count as water. A volume's
// `minimum.w` carries its mask slot + 1, 0 meaning "the box is the water".
fn ocean_mask_contains(slot: i32, p: vec2<f32>) -> bool {
    if (slot < 0 || slot >= 8) {
        return true;
    }
    let range = ocean.mask_ranges[slot];
    let count = u32(range.y);
    if (count == 0u) {
        return true;
    }
    let bounds = ocean.mask_bounds[slot];
    let outside_bounds = any(p < bounds.xy) || any(p > bounds.zw);
    if (any(p < bounds.xy - vec2<f32>(CAUSTIC_RECEIVER_MARGIN))
        || any(p > bounds.zw + vec2<f32>(CAUSTIC_RECEIVER_MARGIN))) {
        return false;
    }
    let tile_xy = vec2<u32>(clamp(
        floor((p - bounds.xy) / max(bounds.zw - bounds.xy, vec2<f32>(1.0e-4)) * 8.0),
        vec2<f32>(0.0), vec2<f32>(7.0)));
    let tile = tile_xy.y * 8u + tile_xy.x;
    let code = (ocean.mask_tiles[slot][tile >> 4u] >> ((tile & 15u) * 2u)) & 3u;
    // Cached tile classifications cover only the exact bounds. Points in the
    // margin must still pass the tolerant triangle test, including hole edges.
    if (!outside_bounds && code == 1u) { return false; }
    if (!outside_bounds && code == 2u) { return true; }
    let first = u32(range.x);
    let tolerance = 0.75;
    for (var i = 0u; i < count; i = i + 1u) {
        let t0 = ocean.mask_triangles[(first + i) * 2u];
        let t1 = ocean.mask_triangles[(first + i) * 2u + 1u];
        let a = t0.xy;
        let b = t0.zw;
        let c = t1.xy;
        let lo = min(a, min(b, c)) - vec2<f32>(tolerance);
        let hi = max(a, max(b, c)) + vec2<f32>(tolerance);
        if (any(p < lo) || any(p > hi)) {
            continue;
        }
        let ab = b - a;
        let bc = c - b;
        let ca = a - c;
        let e0 = (ab.x * (p.y - a.y) - ab.y * (p.x - a.x)) / max(length(ab), 1.0e-4);
        let e1 = (bc.x * (p.y - b.y) - bc.y * (p.x - b.x)) / max(length(bc), 1.0e-4);
        let e2 = (ca.x * (p.y - c.y) - ca.y * (p.x - c.x)) / max(length(ca), 1.0e-4);
        if ((e0 >= -tolerance && e1 >= -tolerance && e2 >= -tolerance)
            || (e0 <= tolerance && e1 <= tolerance && e2 <= tolerance)) {
            return true;
        }
    }
    return false;
}

// Water spans along the current ray, as fractions of its eye->surface delta,
// sorted by entry. Private state so masked volumes can add several spans.
// The capacity is deliberately small. This is a `var<private>` array indexed by
// a runtime value, so it cannot live in registers: every invocation reserves
// scratch memory for the whole array, which costs occupancy for the entire
// shader whether or not the spans are used. Measured on hothchallenges looking
// across water, capacity 16 vs 8 was 1.85ms vs 1.33ms in this pass, and 4 was
// no better than 8, so 8 is the knee. Eight also covers reality comfortably: an
// unmasked volume contributes one span, a ray crossing a ring-shaped outline two.
const OCEAN_MAX_SPANS: u32 = 8u;
var<private> g_spans: array<vec2<f32>, OCEAN_MAX_SPANS>;
var<private> g_span_count: u32;
var<private> g_eye_depth: f32;

// Water deeper than this below the surface counts as water even inside an
// outline hollow (see capture_sample).
const MASK_DRY_DEPTH: f32 = 128.0;

// Depth of the water entry point that sets the in-scattered light colour. From
// above the surface the nearest entry governs: the 128-unit hollow fallback span
// starts deeper by construction and must not override a ray that already met the
// wet surface (that made fog darker only inside the original BSP footprint).
// A submerged eye keeps the deepest entry, as before.
fn note_eye_depth(top: f32, delta_y: f32, span_x: f32, first: bool) {
    let depth = max(top - (optics.eye.y + delta_y * span_x), 0.0);
    if (optics.eye.y > top) {
        g_eye_depth = select(min(g_eye_depth, depth), depth, first);
    } else {
        g_eye_depth = max(g_eye_depth, depth);
    }
}

fn add_span(span: vec2<f32>, top: f32, delta_y: f32) {
    if (span.y <= span.x) {
        return;
    }
    if (g_span_count >= OCEAN_MAX_SPANS) {
        // Overflow widens the last span rather than discarding this one, so a
        // ray crossing more than eight water pieces over-estimates absorption
        // slightly instead of silently losing a layer. `add_masked_spans`
        // already folds its own runs the same way.
        g_spans[OCEAN_MAX_SPANS - 1u] = vec2<f32>(
            min(g_spans[OCEAN_MAX_SPANS - 1u].x, span.x),
            max(g_spans[OCEAN_MAX_SPANS - 1u].y, span.y),
        );
        note_eye_depth(top, delta_y, span.x, false);
        return;
    }
    // Insertion sort allows exact union of overlapping bounds (no double fog).
    let first_span = g_span_count == 0u;
    var j = g_span_count;
    loop {
        if (j == 0u) { break; }
        if (g_spans[j - 1u].x <= span.x) { break; }
        g_spans[j] = g_spans[j - 1u];
        j -= 1u;
    }
    g_spans[j] = span;
    g_span_count += 1u;
    note_eye_depth(top, delta_y, span.x, first_span);
}

// Part of `span` where the ray's xz line is inside one outline triangle (grown
// by the surface mask's 0.75 unit tolerance). x >= y means the ray misses it.
fn mask_triangle_interval(a: vec2<f32>, b: vec2<f32>, c: vec2<f32>, origin: vec2<f32>, direction: vec2<f32>, span: vec2<f32>) -> vec2<f32> {
    let miss = vec2<f32>(1.0, 0.0);
    let tolerance = 0.75;
    let p0 = origin + direction * span.x;
    let p1 = origin + direction * span.y;
    let lo = min(a, min(b, c)) - vec2<f32>(tolerance);
    let hi = max(a, max(b, c)) + vec2<f32>(tolerance);
    if (any(max(p0, p1) < lo) || any(min(p0, p1) > hi)) {
        return miss;
    }
    let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    if (abs(area) < 1.0e-4) {
        return miss;
    }
    let winding = select(-1.0, 1.0, area > 0.0);
    var t = span;
    for (var edge_index = 0u; edge_index < 3u; edge_index += 1u) {
        var start = a;
        var finish = b;
        if (edge_index == 1u) {
            start = b;
            finish = c;
        } else if (edge_index == 2u) {
            start = c;
            finish = a;
        }
        let edge = finish - start;
        let inv_length = 1.0 / max(length(edge), 1.0e-4);
        // Signed distance to the edge (positive inside) is linear in t.
        let at_origin = winding * (edge.x * (origin.y - start.y) - edge.y * (origin.x - start.x)) * inv_length + tolerance;
        let slope = winding * (edge.x * direction.y - edge.y * direction.x) * inv_length;
        if (abs(slope) < 1.0e-9) {
            if (at_origin < 0.0) {
                return miss;
            }
        } else {
            let bound = -at_origin / slope;
            if (slope > 0.0) {
                t.x = max(t.x, bound);
            } else {
                t.y = min(t.y, bound);
            }
        }
    }
    return t;
}

// Clip one interval against p(t) <= limit; reversed inputs express >=.
// No arbitrary ocean extent: every XZ constraint is a half-plane.
fn sky_fog_clip(span: vec2<f32>, origin: f32, delta: f32, limit: f32) -> vec2<f32> {
    if (abs(delta) < 1.0e-7) {
        return select(span, vec2<f32>(1.0, 0.0), origin > limit);
    }
    let t = (limit - origin) / delta;
    if (delta > 0.0) {
        return vec2<f32>(span.x, min(span.y, t));
    }
    return vec2<f32>(max(span.x, t), span.y);
}

fn sky_fog_column(span: vec2<f32>, origin: vec2<f32>, delta: vec2<f32>,
    wall: vec4<f32>) -> vec2<f32> {
    let side = u32(round(wall.w));
    var result = span;
    if (side == 1u || side == 2u) {
        // -X / +X half-strip with bounded tangential Z.
        result = sky_fog_clip(result, origin.x * select(1.0, -1.0, side == 2u),
            delta.x * select(1.0, -1.0, side == 2u),
            wall.x * select(1.0, -1.0, side == 2u) + 2.0);
        result = sky_fog_clip(result, -origin.y, -delta.y, -wall.y);
        result = sky_fog_clip(result, origin.y, delta.y, wall.z);
    } else if (side == 3u || side == 4u) {
        // -Z / +Z half-strip with bounded tangential X.
        result = sky_fog_clip(result, origin.y * select(1.0, -1.0, side == 4u),
            delta.y * select(1.0, -1.0, side == 4u),
            wall.x * select(1.0, -1.0, side == 4u) + 2.0);
        result = sky_fog_clip(result, -origin.x, -delta.x, -wall.y);
        result = sky_fog_clip(result, origin.x, delta.x, wall.z);
    } else {
        return vec2<f32>(1.0, 0.0);
    }
    return result;
}

// The optical medium must agree with the extended clipmap mask. Original
// bounded water boxes above remain unchanged; only the added sky-contact
// columns keep their source pool's authored floor. Adjacent quadrants
// are admitted only when both touched sky walls meet at a real corner.
fn add_sky_ocean_spans(delta: vec3<f32>) {
    if (JKA_SUBMERGED_FOG) { return; }
    let origin = optics.eye.xz;
    let direction = delta.xz;
    for (var i = 0u; i < u32(optics.sky_info.x); i += 1u) {
        let top = optics.sky_regions[i].plane.x;
        let above_floor = sky_fog_clip(vec2<f32>(0.0, 1.0), -optics.eye.y, -delta.y, -optics.sky_regions[i].plane.w);
        let wet = jka_wave_wet_span(above_floor, delta, top);
        if (wet.y <= wet.x) { continue; }
        for (var j = 0u; j < u32(optics.sky_regions[i].plane.y); j += 1u) {
            let seg = optics.sky_regions[i].segments[j];
            let span = sky_fog_column(wet, origin, direction, seg);
            if (span.y > span.x) { add_span(span, top, delta.y); }
        }
        let corners = u32(round(optics.sky_regions[i].plane.z));
        for (var c = 0u; c < 4u; c += 1u) {
            if ((corners & (1u << c)) == 0u) { continue; }
            // Bit order: (-X,-Z), (+X,-Z), (-X,+Z), (+X,+Z).
            let right = (c & 1u) != 0u;
            let far = (c & 2u) != 0u;
            // Explicit second constraint avoids an XZ rectangle or finite radius.
            let x_edge = select(optics.sky_regions[i].bounds.x, optics.sky_regions[i].bounds.z, right);
            let z_edge = select(optics.sky_regions[i].bounds.y, optics.sky_regions[i].bounds.w, far);
            var span = sky_fog_clip(wet, origin.x * select(1.0, -1.0, right),
                direction.x * select(1.0, -1.0, right), x_edge * select(1.0, -1.0, right));
            span = sky_fog_clip(span, origin.y * select(1.0, -1.0, far),
                direction.y * select(1.0, -1.0, far), z_edge * select(1.0, -1.0, far));
            if (span.y > span.x) { add_span(span, top, delta.y); }
        }
    }
}

const MASK_FINE_AXIS: i32 = 32;

// Classifies the tiles of the outline's 32x32 grid (`mask_fine_tiles`, exact
// against the coastline grown by the mask tolerance) that the ray's xz segment
// crosses: 1 all outside, 2 all inside, 0 when it touches a coastline tile,
// both kinds, or more tiles than the walk allows.
fn mask_segment_class(slot: i32, origin: vec2<f32>, direction: vec2<f32>, span: vec2<f32>) -> u32 {
    let bounds = ocean.mask_bounds[slot];
    let axis = f32(MASK_FINE_AXIS);
    let size = max((bounds.zw - bounds.xy) / axis, vec2<f32>(1.0e-4));
    let p0 = (origin + direction * span.x - bounds.xy) / size;
    let p1 = (origin + direction * span.y - bounds.xy) / size;
    var cell = vec2<i32>(clamp(floor(p0), vec2<f32>(0.0), vec2<f32>(axis - 1.0)));
    let last = vec2<i32>(clamp(floor(p1), vec2<f32>(0.0), vec2<f32>(axis - 1.0)));
    let d = p1 - p0;
    let step = vec2<i32>(select(vec2<f32>(-1.0), vec2<f32>(1.0), d >= vec2<f32>(0.0)));
    // Segment fraction to the next tile edge on each axis, and per tile.
    let next_edge = vec2<f32>(cell) + select(vec2<f32>(0.0), vec2<f32>(1.0), d >= vec2<f32>(0.0));
    let inv = 1.0 / max(abs(d), vec2<f32>(1.0e-6));
    var t_max = abs(next_edge - p0) * inv;
    let t_delta = inv;
    var seen = 0u;
    for (var i = 0u; i < 64u; i += 1u) {
        let tile = u32(cell.y * MASK_FINE_AXIS + cell.x);
        let word = u32(slot) * 64u + (tile >> 4u);
        let code = (ocean.mask_fine_tiles[word >> 2u][word & 3u] >> ((tile & 15u) * 2u)) & 3u;
        if (code == 0u) { return 0u; }
        seen |= code;
        if (seen == 3u) { return 0u; }
        if (all(cell == last)) { return seen; }
        if (t_max.x < t_max.y) {
            cell.x += step.x;
            t_max.x += t_delta.x;
        } else {
            cell.y += step.y;
            t_max.y += t_delta.y;
        }
        if (any(cell < vec2<i32>(0)) || any(cell >= vec2<i32>(MASK_FINE_AXIS))) { return 0u; }
    }
    return 0u;
}

// Adds the runs of `span` that lie inside the volume's real outline.
fn add_masked_spans(slot: i32, span: vec2<f32>, top: f32, delta: vec3<f32>) {
    let range = ocean.mask_ranges[slot];
    let count = u32(range.y);
    let first = u32(range.x);
    // Outside its bounds a masked volume holds no water (as `ocean_mask_contains`).
    let bounds = ocean.mask_bounds[slot];
    let origin = optics.eye.xz;
    var inner = sky_fog_clip(span, -origin.x, -delta.x, -bounds.x);
    inner = sky_fog_clip(inner, origin.x, delta.x, bounds.z);
    inner = sky_fog_clip(inner, -origin.y, -delta.z, -bounds.y);
    inner = sky_fog_clip(inner, origin.y, delta.z, bounds.w);
    if (inner.y <= inner.x) { return; }
    // Most rays only cross tiles wholly in or out of the outline; only one that
    // touches a boundary tile walks every outline triangle.
    let crossed = mask_segment_class(slot, origin, delta.xz, inner);
    if (crossed == 2u) { add_span(inner, top, delta.y); return; }
    if (crossed == 1u) { return; }
    var runs: array<vec2<f32>, 8>;
    var run_count = 0u;
    for (var i = 0u; i < count; i = i + 1u) {
        let t0 = ocean.mask_triangles[(first + i) * 2u];
        let t1 = ocean.mask_triangles[(first + i) * 2u + 1u];
        let piece = mask_triangle_interval(t0.xy, t0.zw, t1.xy, optics.eye.xz, delta.xz, span);
        if (piece.y <= piece.x) {
            continue;
        }
        // Neighbouring triangles abut, so fold every run this piece touches
        // into one. Overflow past eight runs widens the last one.
        var merged = piece;
        var kept = 0u;
        for (var k = 0u; k < run_count; k = k + 1u) {
            let run = runs[k];
            if (run.y >= merged.x - 1.0e-4 && run.x <= merged.y + 1.0e-4) {
                merged = vec2<f32>(min(merged.x, run.x), max(merged.y, run.y));
            } else {
                runs[kept] = run;
                kept += 1u;
            }
        }
        if (kept < 8u) {
            runs[kept] = merged;
            kept += 1u;
        } else {
            runs[7u] = vec2<f32>(min(runs[7u].x, merged.x), max(runs[7u].y, merged.y));
        }
        run_count = kept;
    }
    for (var k = 0u; k < run_count; k = k + 1u) {
        add_span(runs[k], top, delta.y);
    }
}

fn slopes(p: vec2<f32>) -> vec2<f32> {
    let scale = ocean.map_scales[1];
    let uv = p / ocean.ocean_info.z * vec2<f32>(1.0, -1.0) * scale.xy;
    return textureSampleLevel(wave_normal, wave_sampler, uv, 1, 0.0).xy * vec2<f32>(1.0, -1.0) * scale.w;
}
struct Capture {
    @location(0) color: vec4<f32>,
    @location(1) depth: f32,
};
struct OpticalSample {
    color: vec4<f32>,
    depth: f32,
    water_distance: f32,
    receiver: vec4<f32>,
};
// 32 seamless phases generated once on the GPU. Consecutive phases change
// actual light-web shapes (interference and evolving warp), not just UV offsets.
// Two level-zero filtered taps interpolate continuously at any render FPS.
fn animated_caustics(position: vec2<f32>) -> f32 {
    let phase = fract(optics.procedural_params.x * optics.procedural_params.z / 3.2) * 32.0;
    let slice = i32(floor(phase));
    let blend = fract(phase);
    // Slow non-looping advection prevents the precomputed temporal cycle from
    // obviously repeating. The field still deforms independently of drift.
    let advection = optics.procedural_params.x * optics.procedural_params.z * vec2<f32>(0.005, -0.004);
    let uv = fract(position * (optics.procedural_params.y / 1024.0) + advection);
    let next = (slice + 1) % 32;
    let a = textureSampleLevel(caustic_patterns, caustic_pattern_sampler, uv, slice / 4, 0.0)[u32(slice % 4)];
    let b = textureSampleLevel(caustic_patterns, caustic_pattern_sampler, uv, next / 4, 0.0)[u32(next % 4)];
    return mix(a, b, blend);
}
fn capture_sample(uv: vec2<f32>, depth: f32, sample_color: vec3<f32>) -> OpticalSample {
    let h = optics.inverse_view_projection * vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), depth, 1.0);
    let world = h.xyz / h.w;
    let delta = world - optics.eye.xyz;
    let ray_length = length(delta);
    // Derivatives must remain outside pixel-dependent branches for WGSL
    // derivative uniformity; other caustics work is gated below.
    let world_dx = dpdx(world);
    let world_dy = dpdy(world);
    var color = sample_color;
    g_span_count = 0u;
    g_eye_depth = 0.0;
    // Distance gate runs before footprint classification and all caustic
    // slope samples. No extra pass, texture, FFT work or CPU readback.
    let caustic_limit = optics.caustic_tuning.w;
    // A zero/near-zero sun strength cannot illuminate Godot-style caustics.
    // Do not compute their footprint and FFT derivatives just to multiply the
    // result by zero later. Procedural Hybrid uses a deliberate ambient floor.
    let caustic_sun_eligible = ocean.sun_direction_intensity.w > 0.001 || optics.procedural_params.w > 0.5;
    let caustic_receiver_in_range = optics.params.z > 0.0 && depth > 0.0 && caustic_sun_eligible &&
        (caustic_limit <= 0.0 || ray_length < caustic_limit);
    var bottom_depth = 0.0;
    for (var i = 0u; i < select(min(u32(optics.params.w), 8u), 0u, JKA_SUBMERGED_FOG); i += 1u) {
        let lo = optics.minimum[i].xyz;
        let hi = optics.maximum[i].xyz;
        let slot = i32(round(optics.minimum[i].w)) - 1;
        // For Hybrid allow the ray to start above the UNDISTURBED plane while
        // still inside a crest. The XZ box and floor remain exact. The upper
        // Y bound is only a conservative candidate envelope: the final span
        // is clipped individually against the sampled animated wave.
        let candidate_hi = select(hi, vec3<f32>(hi.x, hi.y + 4096.0, hi.z), JKA_HYBRID_WAVE);
        let candidate = interval(optics.eye.xyz, delta, lo, candidate_hi);
        let span = jka_wave_wet_span(candidate, delta, hi.y);
        if (span.y > span.x) {
            let local_top = jka_wave_height(optics.eye.xz, hi.y);
            if (slot >= 0 && slot < 8) {
                add_masked_spans(slot, span, local_top, delta);
                // Hollows remain dry near the shoreline, even when the wave
                // rises above the original plane. Keep the deep fallback.
                add_span(sky_fog_clip(span, optics.eye.y, delta.y, hi.y - MASK_DRY_DEPTH), local_top, delta.y);
            } else {
                add_span(span, local_top, delta.y);
            }
        }
        let receiver_margin = vec3<f32>(CAUSTIC_RECEIVER_MARGIN);
        if (caustic_receiver_in_range && all(world >= lo - receiver_margin)
            && all(world <= candidate_hi + receiver_margin)
            && ocean_mask_contains(slot, world.xz)) {
            bottom_depth = max(bottom_depth, max(jka_wave_height(world.xz, hi.y) - world.y, 0.0));
        }
    }
    // The continuation exists under the rendered ocean, NOT as fog-painted
    // background geometry. A depth=0 sample is sky (no finite receiver):
    // marching that far-plane ray through semi-infinite sky columns creates
    // large triangular fog wedges, including on the underwater skybox.
    // Finite world/model receivers still receive the continued absorption.
    if (depth > 0.0) {
        add_sky_ocean_spans(delta);
    }
    var eye_depth = g_eye_depth;
    var water_fraction = 0.0;
    var end = 0.0;
    for (var i = 0u; i < g_span_count; i += 1u) {
        water_fraction += max(g_spans[i].y - max(g_spans[i].x, end), 0.0);
        end = max(end, g_spans[i].y);
    }
    if (JKA_SUBMERGED_FOG) {
        // Water from the eye to the receiver, or to where the ray rises through
        // the water top; what lies past it is behind the drawn surface.
        let top = optics.eye.w;
        water_fraction = select(1.0, clamp((top - optics.eye.y) / delta.y, 0.0, 1.0), delta.y > 0.0);
        let floor_span = sky_fog_clip(vec2<f32>(0.0, water_fraction), -optics.eye.y, -delta.y, -optics.forward.w);
        water_fraction = max(floor_span.y - floor_span.x, 0.0);
        eye_depth = max(top - optics.eye.y, 0.0);
        if (caustic_receiver_in_range && world.y >= optics.forward.w - CAUSTIC_RECEIVER_MARGIN) {
            bottom_depth = max(top - world.y, 0.0);
        }
    }
    // Optional low-cost wave-curvature caustics. Approximation, not photon tracing.
    // No caustic-specific normal reconstruction or wave samples when disabled.
    if (caustic_receiver_in_range && bottom_depth > 2.0) {
        // Scope to the bound simulation; distinct authored oceans never borrow its waves.
        let own_margin = vec2<f32>(CAUSTIC_RECEIVER_MARGIN);
        let own_water = ocean.authored_plane.x < 0.5 || (all(world.xz >= ocean.authored_bounds.xy - own_margin) && all(world.xz <= ocean.authored_bounds.zw + own_margin));
        let sun = normalize(-ocean.sun_direction_intensity.xyz);
        if (own_water && sun.y > 0.05) {
            let normal_cross = cross(world_dx, world_dy);
            let normal_length = length(normal_cross);
            // No invented world-up normal when gradients are degenerate.
            // In that case the receiver's orientation is unknown: skip caustics.
            if (normal_length > 0.000001) {
                let scene_normal = normal_cross / normal_length;
                let world_pixel_span = max(length(world_dx), length(world_dy));
                let viewport = vec2<f32>(textureDimensions(scene_color));
                let expected_pixel_span = max(ray_length * 4.0 / max(min(viewport.x, viewport.y), 1.0), 0.25);
                let caustic_surface_confidence = 1.0 - smoothstep(
                    expected_pixel_span * 4.0,
                    expected_pixel_span * 12.0,
                    world_pixel_span,
                );
                let refracted = refract(-sun, vec3<f32>(0.0, 1.0, 0.0), 1.0 / 1.333);
                // Hybrid samples Blue Water's 181m mid-cascade while Godot uses
                // a 57m tile. Repeating the native field more densely restores
                // roughly the Godot pattern size without another FFT/capture.
                var focus = 0.0;
                if (optics.procedural_params.w > 0.5) {
                    // Independent animated interference field: no FFT normal
                    // texture samples or per-fragment wave-curvature derivative.
                    let surface = world.xz - refracted.xz * bottom_depth / max(-refracted.y, 0.1);
                    focus = (animated_caustics(surface) - 0.26) * 1.75 * optics.procedural_params.w;
                } else {
                    let density = max(optics.caustic_tuning.x, 1.0);
                    let surface = (world.xz - refracted.xz * bottom_depth / max(-refracted.y, 0.1)) * density;
                    let step = 8.0 / density;
                    var divergence = 0.0;
                    if (optics.caustic_tuning.z > 0.5) {
                        let c = slopes(surface);
                        let sx = slopes(surface + vec2<f32>(step, 0.0));
                        let sz = slopes(surface + vec2<f32>(0.0, step));
                        divergence = ((sx.x - c.x) + (sz.y - c.y)) / step;
                    } else {
                        let dx = (slopes(surface + vec2<f32>(step,0.0)) - slopes(surface - vec2<f32>(step,0.0))) / (2.0*step);
                        let dz = (slopes(surface + vec2<f32>(0.0,step)) - slopes(surface - vec2<f32>(0.0,step))) / (2.0*step);
                        divergence = dx.x + dz.y;
                    }
                    focus = clamp(1.0 / max(0.25, 1.0 + bottom_depth * 0.25 * divergence) - 1.0, -0.4, 2.0);
                }
                let fade = exp2(-4.321928 * bottom_depth / max(optics.fog.w, 1.0));
                // Enhance bright focus lines without making unfocused regions
                // excessively dark, as scaling both signed lobes would do.
                let tuned_focus = min(focus, 0.0) + max(focus, 0.0) * optics.caustic_tuning.y;
                // Fade during the outer quarter of the receiver range to avoid
                // a moving cutoff boundary as the player flies over the water.
                var range_fade = 1.0;
                if (caustic_limit > 0.0) {
                    range_fade = 1.0 - smoothstep(caustic_limit * 0.75, caustic_limit, ray_length);
                }
                let caustic = tuned_focus * optics.params.z * fade * range_fade * abs(dot(scene_normal, sun)) * select(min(ocean.sun_direction_intensity.w, 1.0), max(min(ocean.sun_direction_intensity.w, 1.0), 0.25), optics.procedural_params.w > 0.5);
                // Preserve the existing discontinuity fade and brightness clamp.
                let caustic_multiplier = max(1.0 + caustic, 0.65);
                color *= mix(1.0, caustic_multiplier, caustic_surface_confidence);
            }
        }
    }
    // BlueWater applies its own column absorption. Give its surface the same
    // Godot caustics before fog, so that transmission is not attenuated twice.
    let receiver_color = vec4<f32>(color, 1.0);
    // No-receiver pixels keep the original box fog only (identical to OFF);
    // sky columns never paint them (see above). The exception is a downward
    // ray seen from above the surface of an extended ocean: it can only end in
    // an unbounded water column, so a ray clipped by the original box edge
    // must not leave a dark void-coloured border there.
    if (depth <= 0.0 && delta.y < 0.0) {
        for (var i = 0u; i < u32(optics.sky_info.x); i += 1u) {
            if (optics.eye.y > jka_wave_height(optics.eye.xz, optics.sky_regions[i].plane.x)) {
                water_fraction = 1.0;
                // The unbounded column starts at the surface. Do not inherit the
                // original box's 128-unit hollow span depth: that darkened only the
                // no-receiver pixels inside the BSP footprint (red fog -> black).
                eye_depth = 0.0;
            }
        }
    }
    if (water_fraction > 0.0) {
        // log2(20) corresponds to FIVE percent transmission, not four.
        let ratios = vec3<f32>(38.0/7.5, 38.0/22.0, 1.0);
        let transmission = exp2(-4.321928 * ratios * ray_length * water_fraction / max(optics.fog.w, 1.0));
        let downwelling = exp2(-4.321928 * ratios * eye_depth / max(optics.params.x, 1.0));
        color = color * transmission + optics.fog.rgb * downwelling * (1.0 - transmission);
    }
    var out: OpticalSample;
    out.color = vec4<f32>(color, 1.0);
    out.depth = select(max(dot(delta, optics.forward.xyz), 0.0), 1000000.0, depth <= 0.0);
    out.water_distance = ray_length * water_fraction;
    out.receiver = receiver_color;
    return out;
}

fn capture_pixel(p: vec4<f32>) -> OpticalSample {
    let pixel = vec2<i32>(p.xy);
    let uv = p.xy / vec2<f32>(textureDimensions(scene_color));
    CAPTURE_BODY
}
@fragment fn fs_capture(@builtin(position) p: vec4<f32>) -> Capture {
    let sample = capture_pixel(p);
    return Capture(sample.color, sample.depth);
}
struct CaptureReceiver {
    @location(0) color: vec4<f32>,
    @location(1) depth: f32,
    @location(2) receiver: vec4<f32>,
};
@fragment fn fs_capture_receiver(@builtin(position) p: vec4<f32>) -> CaptureReceiver {
    let sample = capture_pixel(p);
    return CaptureReceiver(sample.color, sample.depth, sample.receiver);
}
@fragment fn fs_water_distance(@builtin(position) p: vec4<f32>) -> @location(0) f32 {
    return capture_pixel(p).water_distance / 40.0;
}
@fragment fn fs_glass_background(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let sample=capture_pixel(p);
    // Keep world depth with color to distinguish a recoverable model occlusion
    // from an opaque world wall in front of the pane.
    return vec4<f32>(sample.color.rgb,min(sample.depth,60000.0));
}
