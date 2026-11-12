// Sparse-only stage variants with workgroup-memory neighbourhoods (not
// Unreal; r_riverStorageMode 1/2). Appended after shallow_sparse.wgsl and an
// invocation file. Every 8x8 workgroup lies inside one tile, so its
// neighbourhood is staged once instead of re-read per cell through the tile
// table. Reads, values and arithmetic order are those of the shallow.wgsl
// stage they replace; sparse_river_storage_modes_match_native (jka-river)
// requires a bitwise identical pool.

// ---- PrepareSurface (shallow.wgsl prepare_surface) ----
// The native smoothing radius (Presentation::half_width) is 5. Neighbours past
// SURFACE_LDS_RADIUS fall back to global reads, so larger radii stay exact.
const SURFACE_LDS_RADIUS: i32 = 5;
const SURFACE_LDS_SIDE: i32 = 18; // 8 + 2 * SURFACE_LDS_RADIUS
// Smoothing neighbours at clamp_cell(p): (height, depth), the depth zeroed
// where the neighbour is a collider. The stage tests
// `collider < 1 - 1e-5 && depth > 1e-5`; a zeroed depth fails it just the same.
var<workgroup> surface_hd: array<vec2<f32>, 324>;
// Velocity at clamp_cell(p) for the radius-1 blur (10 x 10).
var<workgroup> surface_velocity: array<vec2<f32>, 100>;

fn surface_masked(c: vec2<i32>) -> vec2<f32> {
    let n = read_water(c);
    return vec2<f32>(n.x, select(0.0, n.y, read_geometry(c).y < 1.0 - 1e-5));
}
// Neighbour at clamp_cell(p) for a cell of the workgroup starting at `origin`.
fn surface_neighbour(origin: vec2<i32>, p: vec2<i32>) -> vec2<f32> {
    let u = p - origin + vec2<i32>(SURFACE_LDS_RADIUS);
    if all(u >= vec2<i32>(0)) && all(u < vec2<i32>(SURFACE_LDS_SIDE)) {
        return surface_hd[u.y * SURFACE_LDS_SIDE + u.x];
    }
    return surface_masked(clamp_cell(p));
}

@compute @workgroup_size(8, 8)
fn prepare_surface_shared(
    @builtin(global_invocation_id) id: vec3<u32>,
    @builtin(local_invocation_id) local: vec3<u32>,
    @builtin(local_invocation_index) lane: u32,
) {
    let at = invocation_cell(id);
    let origin = at - vec2<i32>(local.xy);
    for (var k = i32(lane); k < SURFACE_LDS_SIDE * SURFACE_LDS_SIDE; k += 64) {
        let p = origin - vec2<i32>(SURFACE_LDS_RADIUS) + vec2<i32>(k % SURFACE_LDS_SIDE, k / SURFACE_LDS_SIDE);
        surface_hd[k] = surface_masked(clamp_cell(p));
    }
    for (var k = i32(lane); k < 100; k += 64) {
        let p = origin - vec2<i32>(1) + vec2<i32>(k % 10, k / 10);
        surface_velocity[k] = read_water(clamp_cell(p)).zw;
    }
    workgroupBarrier();
    if !invocation_active(id, at) { return; }
    let input = read_water(at);
    let border = i32(presentation.smoothing.w);
    let maximum = vec2<i32>(params.dimensions.xy) - 1;
    let is_border = any(at < vec2<i32>(border)) || any(at > maximum - border);
    let extrap = !is_border && (read_geometry(at).y > 1.0 - 1e-5 || input.y < 1e-5);
    var height = select(input.x, 0.0, extrap);
    var depth = select(input.y, 0.0, extrap);
    var weight_height = select(1.0, 0.0, extrap);
    var weight_depth = weight_height;
    let radius = i32(presentation.smoothing.x);
    let depth_radius = max(radius - 2, 1);
    let method = select(i32(presentation.smoothing.z), 3, presentation.smoothing.y < 1e-8);
    for (var x = -radius; x <= radius; x++) { for (var y = -radius; y <= radius; y++) {
        // Exact source predicate: neither the center row nor column contributes.
        if x != 0 && y != 0 {
            let position = clamp_cell(at + vec2<i32>(x, y));
            let n = surface_neighbour(origin, at + vec2<i32>(x, y));
            if n.y > 1e-5 {
                let distance = length(vec2<f32>(position - at));
                var weight = 0.0;
                if extrap { weight = distance; }
                else if method != 0 {
                    weight = 1.0 - smoothstep(0.0, f32(radius), distance);
                    if method == 1 { weight *= 1.0 - smoothstep(0.0, presentation.smoothing.y, abs(n.x - input.x)); }
                    else if method == 2 { weight *= smoothstep(0.0, presentation.smoothing.y, abs(n.x - input.x)); }
                }
                height += n.x * weight;
                weight_height += weight;
                if abs(x) <= depth_radius && abs(y) <= depth_radius {
                    depth += n.y * weight;
                    weight_depth += weight;
                }
            }
        }
    }}
    if weight_height > 1e-8 { height /= weight_height; }
    else if extrap { height = input.x; }
    if weight_depth > 1e-8 { depth /= weight_depth; }
    else if extrap { depth = input.y; }
    if is_border { depth = 0.0; }
    if presentation.extension.x > 0.5 && !extrap {
        height = max(height, read_geometry(at).x);
    }
    var velocity = vec2<f32>(0.0);
    var weight = 0.0;
    let ring = vec2<i32>(local.xy) + vec2<i32>(1);
    for (var x = -1; x <= 1; x++) { for (var y = -1; y <= 1; y++) {
        let kernel = length(vec2<f32>(1.0) - abs(vec2<f32>(f32(x), f32(y))) / 2.0);
        velocity += surface_velocity[(ring.y + y) * 10 + ring.x + x] * kernel;
        weight += kernel;
    }}
    velocity /= weight;
    write_water(at, vec4<f32>(height, input.yzw));
    output_surface[index(at)] = vec4<f32>(height, depth, velocity);
}

