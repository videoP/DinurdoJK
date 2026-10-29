// Port of UE 5.8.1 WaterAdvanced Grid2D_SW_River cached shader stages 6..14.
// Original algorithms: Copyright Epic Games, Inc. All Rights Reserved.
// See scripts/ue-water-inspect.py and the river provenance document.
// This file supplies individual stages, not a complete hydraulic solver.
// Water/velocity attributes use the source's Float override: R32f array layers.
// Geometry/output format parity remains separate.

struct Parameters {
    dimensions: vec4<u32>, // nx, ny, extrapolation radius, extrapolate into dry
    stepping: vec4<f32>,   // dx (cm), dt (s), gravity (cm/s2), max velocity (cm/s)
    limits: vec4<f32>,     // Manning friction, velocity dissipation, max depth, integration depth max
    fade: vec4<f32>,       // shallow fade start/speed, advection multiplier, minimum advected depth
}
@group(0) @binding(0) var<uniform> params: Parameters;
@group(0) @binding(1) var water: texture_2d_array<f32>; // height, depth, vx, vy layers
@group(0) @binding(2) var<storage, read> geometry: array<vec4<f32>>; // bottom, collider, collision vx/vy
@group(0) @binding(3) var output_water: texture_storage_2d_array<r32float, write>;
@group(0) @binding(4) var<storage, read_write> previous_bottom: array<f32>;
@group(0) @binding(5) var<uniform> presentation: vec4<f32>; // radius, height cutoff, method, border
@group(0) @binding(6) var<storage, read_write> output_normals: array<vec4<f32>>;
@group(0) @binding(7) var<storage, read_write> output_surface: array<vec4<f32>>;

struct SourceBox { position_angle: vec4<f32>, size_sink: vec4<f32> }
struct Boundary {
    times: vec4<f32>,  // Engine.DeltaTime, sink rate, height-match amount, outside removal rate
    origin: vec4<f32>, // first grid corner relative to simulation origin
    flags: vec4<u32>,  // box count, match spline, unused, unused
}
@group(0) @binding(8) var<storage, read> water_info: array<vec4<f32>>; // local height, depth, flow XY
@group(0) @binding(9) var<storage, read> source_boxes: array<SourceBox>;
@group(0) @binding(10) var<uniform> boundary: Boundary;
@group(0) @binding(11) var grid_sampler: sampler;

fn dt_over_dx() -> f32 { return 1.0 / (params.stepping.x / params.stepping.y); }

// Integer Niagara grid loads outside the resource return zero. Sampling and
// extrapolation stages have their own policies and must not reuse this as a
// substitute for filtered texture sampling.
fn in_bounds(at: vec2<i32>) -> bool {
    return all(at >= vec2<i32>(0)) && all(at < vec2<i32>(params.dimensions.xy));
}
fn index(at: vec2<i32>) -> u32 { return u32(at.y) * params.dimensions.x + u32(at.x); }
fn read_water(at: vec2<i32>) -> vec4<f32> {
    if !in_bounds(at) { return vec4<f32>(0.0); }
    return vec4<f32>(textureLoad(water, at, 0, 0).x, textureLoad(water, at, 1, 0).x,
                     textureLoad(water, at, 2, 0).x, textureLoad(water, at, 3, 0).x);
}
fn write_water(at: vec2<i32>, value: vec4<f32>) {
    textureStore(output_water, at, 0, vec4<f32>(value.x));
    textureStore(output_water, at, 1, vec4<f32>(value.y));
    textureStore(output_water, at, 2, vec4<f32>(value.z));
    textureStore(output_water, at, 3, vec4<f32>(value.w));
}
fn read_geometry(at: vec2<i32>) -> vec4<f32> {
    if !in_bounds(at) { return vec4<f32>(0.0); }
    return geometry[index(at)];
}

// PreSim's PrevBottomContour = BottomContour. For fixed captured geometry,
// only the first step uses the reset zero; later cycles snapshot this bottom.
@compute @workgroup_size(8,8)
fn snapshot_bottom(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= params.dimensions.xy) { return; }
    let i = id.y * params.dimensions.x + id.x;
    previous_bottom[i] = geometry[i].x;
}

fn clamp_cell(at: vec2<i32>) -> vec2<i32> {
    return clamp(at, vec2<i32>(0), vec2<i32>(params.dimensions.xy) - 1);
}

fn bilinear(unit: vec2<f32>, channel: u32) -> f32 {
    // Niagara's SF_Bilinear / AM_Clamp SampleLevel uses the original UV, not
    // a UV reconstructed from GridPos. Let the GPU perform texture filtering.
    return textureSampleLevel(water, grid_sampler, unit, i32(channel), 0.0).x;
}

// NiagaraDataInterfaceGrid2DCollection.ush::Basic1DCubic, not Catmull-Rom.
fn basic_cubic(v: vec4<f32>, t: f32) -> f32 {
    if abs(v.z - v.y) < 1.1754943508e-38 { return v.y; }
    let a1 = -1.0 / 3.0 * v.x - 0.5 * v.y + v.z - 1.0 / 6.0 * v.w;
    let a2 = 0.5 * v.x - v.y + 0.5 * v.z;
    let a3 = -1.0 / 6.0 * v.x + 0.5 * v.y - 0.5 * v.z + 1.0 / 6.0 * v.w;
    return a3 * t * t * t + a2 * t * t + a1 * t + v.y;
}

fn sample_cubic(sample_index: vec2<f32>, channel: u32) -> f32 {
    // Keep Niagara's IndexToUnit -> Basic2DCubic GridPos round trip. Its f32
    // rounding matters near integer cell boundaries; do not cancel it.
    let dimensions = vec2<f32>(params.dimensions.xy);
    let unit = (sample_index + 0.5) * (vec2<f32>(1.0) / dimensions);
    let at = unit * dimensions - 0.5;
    let cell = vec2<i32>(floor(at));
    if any(cell < vec2<i32>(2)) || any(cell >= vec2<i32>(params.dimensions.xy) - 3) {
        return bilinear(unit, channel);
    }
    let t = fract(at);
    var rows = vec4<f32>(0.0);
    var minimum = 3.402823466e+38;
    var maximum = -3.402823466e+38;
    for (var j = 0; j < 4; j++) {
        var values = vec4<f32>(0.0);
        for (var i = 0; i < 4; i++) {
            let value = read_water(cell + vec2<i32>(i - 1, j - 1))[channel];
            values[u32(i)] = value;
            minimum = min(value, minimum);
            maximum = max(value, maximum);
        }
        rows[u32(j)] = basic_cubic(values, t.x);
    }
    let value = basic_cubic(rows, t.y);
    if value < minimum || value > maximum { return bilinear(unit, channel); }
    return value;
}

@compute @workgroup_size(8, 8)
fn source_and_sink(@builtin(global_invocation_id) id: vec3<u32>) {
    let at = vec2<i32>(id.xy);
    if !in_bounds(at) { return; }
    var water = read_water(at);
    let geo = read_geometry(at);
    let position = boundary.origin.xyz + vec3<f32>((vec2<f32>(at) + 0.5) * params.stepping.x, 0.0);
    for (var sink = 0u; sink < 2u; sink++) {
        for (var i = 0u; i < boundary.flags.x; i++) {
            let box = source_boxes[i];
            if u32(box.size_sink.w) != sink { continue; }
            let local = position - box.position_angle.xyz;
            let axis = vec3<f32>(0.0, 0.0, sin(box.position_angle.w / 2.0));
            let rotation_w = -cos(box.position_angle.w / 2.0);
            let rotated = local + cross(axis, local) * 2.0 * rotation_w
                + cross(axis, cross(axis, local) * 2.0);
            if sink == 0u && all(rotated >= -0.5 * box.size_sink.xyz) && all(rotated <= 0.5 * box.size_sink.xyz) {
                water.x = box.position_angle.z + 0.5 * box.size_sink.z;
                water.y = water.x - geo.x;
                // Cached source velocity is zero and nonadditive: never replaces
                // an existing velocity under the source's magnitude comparison.
            }
            if sink != 0u && water.y > 1e-5 {
                let distance = abs(rotated.y);
                let radius = box.size_sink.x;
                if distance <= radius {
                    let info = water_info[index(at)];
                    let target_height = select(geo.x, info.x, info.y > 1e-5);
                    let blend = smoothstep(0.0, radius, distance);
                    let goal = mix(target_height, water.x, blend);
                    water.x += boundary.times.y * (goal - water.x) * boundary.times.x;
                    water.y = water.x - geo.x;
                    water.z += 0.1 * boundary.times.y * (blend * water.z - water.z) * boundary.times.x;
                    water.w += 0.1 * boundary.times.y * (blend * water.w - water.w) * boundary.times.x;
                }
            }
        }
    }
    write_water(at, water);
}

@compute @workgroup_size(8, 8)
fn distribute_overlap(@builtin(global_invocation_id) id: vec3<u32>) {
    let at = vec2<i32>(id.xy);
    if !in_bounds(at) { return; }
    let input = read_water(at);
    // The selected cached shader passes half-width 0 and overlap multiplier 0.
    // Its remaining operation is depth clamping and height reconstruction.
    let depth = min(max(input.y, 0.0), params.limits.z);
    write_water(at, vec4<f32>(read_geometry(at).x + depth, depth, input.zw));
}

@compute @workgroup_size(8, 8)
fn extrapolate_velocity(@builtin(global_invocation_id) id: vec3<u32>) {
    let at = vec2<i32>(id.xy);
    if !in_bounds(at) { return; }
    var w = read_water(at);
    if (w.y < 1e-5 && params.dimensions.w != 0u) || read_geometry(at).y > 1.0 - 1e-5 {
        var distance = 65500.0;
        let radius = i32(params.dimensions.z);
        for (var x = -radius; x <= radius; x++) { for (var y = -radius; y <= radius; y++) {
            let n = read_water(at + vec2<i32>(x, y));
            let d = length(vec2<f32>(f32(x), f32(y)));
            // The source fetches neighbor collider data but does not filter on it.
            if d < distance && n.y > 1e-5 && n.x > w.x && length(n.zw) > 1e-5 {
                w.z = n.z;
                w.w = n.w;
                distance = d;
            }
        }}
    }
    write_water(at, w);
}

@compute @workgroup_size(8, 8)
fn advect_water(@builtin(global_invocation_id) id: vec3<u32>) {
    let at = vec2<i32>(id.xy);
    if !in_bounds(at) { return; }
    let input = read_water(at);
    let geo = read_geometry(at);
    var w = input;
    if geo.y < 1.0 - 1e-5 && input.y > params.fade.w {
        let scale = dt_over_dx() * params.fade.z;
        let vy = 0.25 * (read_water(at + vec2<i32>(0, 1)).w + read_water(at + vec2<i32>(-1, 1)).w
                       + read_water(at + vec2<i32>(-1, 0)).w + input.w);
        w.z = sample_cubic(vec2<f32>(at) - scale * vec2<f32>(input.z, vy), 2u);
        let vx = 0.25 * (read_water(at + vec2<i32>(1, 0)).z + read_water(at + vec2<i32>(1, -1)).z
                       + read_water(at + vec2<i32>(0, -1)).z + input.z);
        w.w = sample_cubic(vec2<f32>(at) - scale * vec2<f32>(vx, input.w), 3u);
        let velocity = 0.5 * (vec2<f32>(read_water(at + vec2<i32>(1, 0)).z,
                                       read_water(at + vec2<i32>(0, 1)).w) + input.zw);
        w.y = sample_cubic(vec2<f32>(at) - scale * velocity, 1u);
    }
    w.x = geo.x + w.y;
    if abs(geo.x - previous_bottom[index(at)]) > params.stepping.x && w.y > 1e-5 {
        w = vec4<f32>(geo.x, 0.0, 0.0, 0.0);
    }
    write_water(at, w);
}

@compute @workgroup_size(8, 8)
fn extrapolate_height(@builtin(global_invocation_id) id: vec3<u32>) {
    let at = vec2<i32>(id.xy);
    if !in_bounds(at) { return; }
    var w = read_water(at);
    let geo = read_geometry(at);
    if geo.y > 1.0 - 1e-5 {
        var distance = 65500.0;
        let radius = i32(params.dimensions.z);
        for (var x = -radius; x <= radius; x++) { for (var y = -radius; y <= radius; y++) {
            let position = at + vec2<i32>(x, y);
            let n = read_water(position);
            let d = length(vec2<f32>(f32(x), f32(y)));
            if d < distance && read_geometry(position).y < 1.0 - 1e-5 && n.y > 1e-5 {
                w.x = n.x;
                distance = d;
            }
        }}
        w.y = w.x - geo.x;
    }
    write_water(at, w);
}

@compute @workgroup_size(8, 8)
fn update_velocity(@builtin(global_invocation_id) id: vec3<u32>) {
    let at = vec2<i32>(id.xy);
    if !in_bounds(at) { return; }
    let input = read_water(at);
    let geo = read_geometry(at);
    let left = read_water(at + vec2<i32>(-1, 0));
    let down = read_water(at + vec2<i32>(0, -1));
    let left_geo = read_geometry(at + vec2<i32>(-1, 0));
    let down_geo = read_geometry(at + vec2<i32>(0, -1));
    let eps = 1e-8;
    var masks = vec2<f32>(left_geo.y, down_geo.y);
    var collision = vec2<f32>(left_geo.z, down_geo.w);
    if (left_geo.x > input.x && left.y < eps)
        || (geo.x > left.x && input.y < eps) || (left.y < eps && input.y < eps) {
        collision.x = 0.0;
        masks.x = 1.0;
    }
    if (down_geo.x > input.x && down.y < eps)
        || (geo.x > down.x && input.y < eps) || (down.y < eps && input.y < eps) {
        collision.y = 0.0;
        masks.y = 1.0;
    }
    let dt = params.stepping.y;
    let scale = dt_over_dx() * params.stepping.z;
    var velocity = input.zw - scale * (input.xx - vec2<f32>(left.x, down.x));
    velocity = mix(velocity, collision, masks);
    velocity = mix(velocity, geo.zw, vec2<f32>(geo.y));
    var speed = length(velocity);
    if speed > eps {
        let depth = select(input.yy, vec2<f32>(left.y, down.y), velocity >= vec2<f32>(0.0));
        var force = -velocity * params.limits.x * speed
            / max(vec2<f32>(eps), pow(depth / 100.0, vec2<f32>(4.0 / 3.0)));
        let magnitude = length(force);
        if magnitude > eps {
            force = clamp(magnitude, 0.0, speed / dt) * force / magnitude;
        }
        velocity += dt * force;
    }
    velocity *= params.limits.y;
    speed = length(velocity);
    if speed > eps {
        velocity = clamp(speed, 0.0, params.stepping.w) * velocity / speed;
    }
    write_water(at, vec4<f32>(input.xy, velocity));
}

fn limited_slope(left: f32, right: f32) -> f32 {
    return sign(left) * max(0.0, min(abs(left), sign(left) * right));
}

@compute @workgroup_size(8, 8)
fn update_depth(@builtin(global_invocation_id) id: vec3<u32>) {
    let at = vec2<i32>(id.xy);
    if !in_bounds(at) { return; }
    let input = read_water(at);
    let geo = read_geometry(at);
    let d = input.y;
    let left = read_water(at + vec2<i32>(-1, 0)).y;
    let right = read_water(at + vec2<i32>(1, 0)).y;
    let down = read_water(at + vec2<i32>(0, -1)).y;
    let up = read_water(at + vec2<i32>(0, 1)).y;
    let left2 = read_water(at + vec2<i32>(-2, 0)).y;
    let right2 = read_water(at + vec2<i32>(2, 0)).y;
    let down2 = read_water(at + vec2<i32>(0, -2)).y;
    let up2 = read_water(at + vec2<i32>(0, 2)).y;
    let vx = input.z;
    let vy = input.w;
    let vr = read_water(at + vec2<i32>(1, 0)).z;
    let vu = read_water(at + vec2<i32>(0, 1)).w;
    var il: f32;
    var ir: f32;
    var ib: f32;
    var iu: f32;
    if vr > 0.0 { ir = d + 0.5 * limited_slope(d - left, right - d); }
    else { ir = right - 0.5 * limited_slope(right - d, right2 - right); }
    if vx > 0.0 { il = left + 0.5 * limited_slope(left - left2, d - left); }
    else { il = d - 0.5 * limited_slope(d - left, right - d); }
    if vu > 0.0 { iu = d + 0.5 * limited_slope(d - down, up - d); }
    else { iu = up - 0.5 * limited_slope(up - d, up2 - up); }
    if vy > 0.0 { ib = down + 0.5 * limited_slope(down - down2, d - down); }
    else { ib = d - 0.5 * limited_slope(d - down, up - d); }
    // Source TotalWeight accumulates four scalar integer ternaries into f32.
    // Keep that form: a vector dot/select rewrite emitted invalid dot4.f64
    // through the bundled DXC, although FXC and Vulkan accepted it.
    var wet: f32 = 0.0;
    wet += f32(select(0i, 1i, left > 1e-8));
    wet += f32(select(0i, 1i, right > 1e-8));
    wet += f32(select(0i, 1i, down > 1e-8));
    wet += f32(select(0i, 1i, up > 1e-8));
    // Preserve the source expression; all-dry behavior needs backend validation.
    let average = (left + right + down + up) / wet;
    let adjustment = max(0.0, average - params.limits.w * params.stepping.x
        / (params.stepping.z * params.stepping.y));
    il -= adjustment;
    ir -= adjustment;
    ib -= adjustment;
    iu -= adjustment;
    var depth = d - dt_over_dx()
        * ((ir * vr - il * vx) + (iu * vu - ib * vy));
    depth -= params.stepping.y * params.fade.y
        * (1.0 - smoothstep(0.0, params.stepping.x * params.fade.x, depth));
    depth = min(max(mix(depth, d, geo.y), 0.0), params.limits.z);
    write_water(at, vec4<f32>(geo.x + depth, depth, input.zw));
}

@compute @workgroup_size(8, 8)
fn match_spline(@builtin(global_invocation_id) id: vec3<u32>) {
    let at = vec2<i32>(id.xy);
    if !in_bounds(at) { return; }
    let input = read_water(at);
    var water = input;
    if boundary.flags.y != 0u {
        let info = water_info[index(at)];
        let bottom = read_geometry(at).x;
        water.y = max(input.y - params.stepping.y * boundary.times.w
            * (1.0 - smoothstep(0.0, 0.1, info.y)), 0.0);
        water.x = bottom + input.y;
        if water.x < info.x && water.y > 1e-5 && info.y > 1e-5 {
            water.x = min(water.x + boundary.times.z, info.x);
            water.y = water.x - bottom;
        }
    }
    write_water(at, water);
}

@compute @workgroup_size(8, 8)
fn prepare_surface(@builtin(global_invocation_id) id: vec3<u32>) {
    let at = vec2<i32>(id.xy);
    if !in_bounds(at) { return; }
    let input = read_water(at);
    let border = i32(presentation.w);
    let maximum = vec2<i32>(params.dimensions.xy) - 1;
    let is_border = any(at < vec2<i32>(border)) || any(at > maximum - border);
    let extrap = !is_border && (read_geometry(at).y > 1.0 - 1e-5 || input.y < 1e-5);
    var height = select(input.x, 0.0, extrap);
    var depth = select(input.y, 0.0, extrap);
    var weight_height = select(1.0, 0.0, extrap);
    var weight_depth = weight_height;
    let radius = i32(presentation.x);
    let depth_radius = max(radius - 2, 1);
    let method = select(i32(presentation.z), 3, presentation.y < 1e-8);
    for (var x = -radius; x <= radius; x++) { for (var y = -radius; y <= radius; y++) {
        // Exact source predicate: neither the center row nor column contributes.
        if x != 0 && y != 0 {
            let position = clamp_cell(at + vec2<i32>(x, y));
            let n = read_water(position);
            if read_geometry(position).y < 1.0 - 1e-5 && n.y > 1e-5 {
                let distance = length(vec2<f32>(position - at));
                var weight = 0.0;
                if extrap { weight = distance; }
                else if method != 0 {
                    weight = 1.0 - smoothstep(0.0, f32(radius), distance);
                    if method == 1 { weight *= 1.0 - smoothstep(0.0, presentation.y, abs(n.x - input.x)); }
                    else if method == 2 { weight *= smoothstep(0.0, presentation.y, abs(n.x - input.x)); }
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
    var velocity = vec2<f32>(0.0);
    var weight = 0.0;
    // Source's radius-one velocity blur is for SimGridRT only.
    for (var x = -1; x <= 1; x++) { for (var y = -1; y <= 1; y++) {
        let kernel = length(vec2<f32>(1.0) - abs(vec2<f32>(f32(x), f32(y))) / 2.0);
        velocity += read_water(clamp_cell(at + vec2<i32>(x, y))).zw * kernel;
        weight += kernel;
    }}
    velocity /= weight;
    write_water(at, vec4<f32>(height, input.yzw));
    output_surface[index(at)] = vec4<f32>(height, depth, velocity);
}

@compute @workgroup_size(8, 8)
fn compute_normals_and_reset(@builtin(global_invocation_id) id: vec3<u32>) {
    let at = vec2<i32>(id.xy);
    if !in_bounds(at) { return; }
    let input = read_water(at);
    let dx = params.stepping.x;
    var normal = vec3<f32>(0.0, 0.0, 1.0);
    if input.y > 1e-5 {
        let right = vec3<f32>(dx, 0.0, read_water(at + vec2<i32>(1, 0)).x - input.x);
        let up = vec3<f32>(0.0, dx, read_water(at + vec2<i32>(0, 1)).x - input.x);
        let product = cross(right, up);
        let magnitude = length(product);
        if magnitude >= 1e-5 { normal = product / magnitude; }
    }
    output_normals[index(at)] = vec4<f32>(normal, 0.0);
    write_water(at, vec4<f32>(read_geometry(at).x + input.y, input.yzw));
}
