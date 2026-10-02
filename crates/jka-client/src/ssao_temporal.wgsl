// Half-resolution optimized uniformly weighted GT-VBAO.
//
// Ported from Mirko Salm's optimized bidirectional GT-VBAO reference. The hot
// horizon-to-bitmask path uses the reference's no-acos uniform slice CDF,
// perspective-correct slice orientation, point-sample/group jitter and
// per-sample view-ray thickness. The Shadertoy reference uses 32 samples per
// direction to validate against a ray marcher; this real-time path deliberately
// uses two slices x two samples per side = 8 AO depth taps.
//
// Temporal reprojection and the renderer's existing full-resolution bilateral
// upsample remain in place because the reference shader's simple static-frame
// accumulation is not suitable for a moving game camera.

struct SsaoTemporalSettings {
    viewport_history: vec4<f32>, // full width, full height, history valid, frame index
    camera_pos_time: vec4<f32>,
    previous_camera_pos: vec4<f32>,
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    prev_view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var linear_depth_texture: texture_2d<f32>;
@group(0) @binding(1) var history_texture: texture_2d<f32>;
@group(0) @binding(2) var history_sampler: sampler;
@group(0) @binding(3) var<uniform> settings: SsaoTemporalSettings;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

const AO_RADIUS_WORLD: f32 = 96.0;
const AO_STRENGTH: f32 = 0.88;
const AO_MAX_PIXEL_RADIUS: f32 = 96.0;
const SLICE_COUNT: u32 = 2u;
const SAMPLES_PER_SIDE: u32 = 2u;
const SECTOR_COUNT: u32 = 32u;
const PI: f32 = 3.14159265358979323846;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(3.0, 1.0)
    );
    let p = positions[vertex_index];
    var out: VertexOut;
    out.position = vec4<f32>(p, 0.0, 1.0);
    out.uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    return out;
}

fn valid_depth(depth: f32) -> bool {
    return depth > 0.0 && depth < 999999.0;
}

fn clamp_uv(uv: vec2<f32>) -> vec2<f32> {
    return clamp(uv, vec2<f32>(0.000001), vec2<f32>(0.999999));
}

fn depth_at_pixel(pixel: vec2<i32>) -> f32 {
    let dims = textureDimensions(linear_depth_texture, 0);
    let max_pixel = vec2<i32>(i32(dims.x) - 1, i32(dims.y) - 1);
    return textureLoad(linear_depth_texture, clamp(pixel, vec2<i32>(0), max_pixel), 0).x;
}

fn depth_at_uv(uv: vec2<f32>) -> f32 {
    let dims = textureDimensions(linear_depth_texture, 0);
    let dims_f = vec2<f32>(f32(dims.x), f32(dims.y));
    let pixel = vec2<i32>(floor(clamp_uv(uv) * dims_f));
    return depth_at_pixel(pixel);
}

fn world_ray(uv: vec2<f32>) -> vec3<f32> {
    let safe_uv = clamp_uv(uv);
    // Reversed-Z: unproject on the far plane (see post.wgsl world_ray).
    let ndc = vec4<f32>(safe_uv.x * 2.0 - 1.0, 1.0 - safe_uv.y * 2.0, 0.0, 1.0);
    var world_far = settings.inv_view_proj * ndc;
    world_far = world_far / max(abs(world_far.w), 1e-6);
    return normalize(world_far.xyz - settings.camera_pos_time.xyz);
}

fn world_position(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    return settings.camera_pos_time.xyz + world_ray(uv) * depth;
}

fn interleaved_gradient_noise(pixel: vec2<f32>, frame: f32) -> f32 {
    // Cheap spatiotemporal rotation. The irrational frame offset prevents a
    // stationary two-slice pattern while temporal accumulation integrates it.
    let p = pixel + vec2<f32>(frame * 17.0, frame * 29.0);
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

fn choose_depth_neighbor(
    centre_pixel: vec2<i32>,
    axis: vec2<i32>,
    centre_depth: f32
) -> vec3<f32> {
    let d_negative = depth_at_pixel(centre_pixel - axis);
    let d_positive = depth_at_pixel(centre_pixel + axis);
    var sign = 1.0;
    var chosen_depth = d_positive;
    if (!valid_depth(d_positive) ||
        (valid_depth(d_negative) && abs(d_negative - centre_depth) < abs(d_positive - centre_depth))) {
        sign = -1.0;
        chosen_depth = d_negative;
    }
    if (!valid_depth(chosen_depth)) {
        chosen_depth = centre_depth;
    }
    return vec3<f32>(sign, chosen_depth, 0.0);
}

struct SurfaceFrame {
    centre_world: vec3<f32>,
    centre_depth: f32,
    view_ray: vec3<f32>,
    normal: vec3<f32>,
    screen_x_world: vec3<f32>,
    screen_y_world: vec3<f32>,
    world_per_pixel: f32,
};

fn reconstruct_surface_frame(uv: vec2<f32>, centre_depth: f32) -> SurfaceFrame {
    let full_dims = max(settings.viewport_history.xy, vec2<f32>(1.0));
    let texel = 1.0 / full_dims;
    let centre_pixel = vec2<i32>(floor(clamp_uv(uv) * full_dims));
    let x_pick = choose_depth_neighbor(centre_pixel, vec2<i32>(1, 0), centre_depth);
    let y_pick = choose_depth_neighbor(centre_pixel, vec2<i32>(0, 1), centre_depth);

    let centre_ray = world_ray(uv);
    let x_uv = uv + vec2<f32>(x_pick.x * texel.x, 0.0);
    let y_uv = uv + vec2<f32>(0.0, y_pick.x * texel.y);
    let x_ray = world_ray(x_uv);
    let y_ray = world_ray(y_uv);

    let camera = settings.camera_pos_time.xyz;
    let centre_world = camera + centre_ray * centre_depth;
    let x_world = camera + x_ray * x_pick.y;
    let y_world = camera + y_ray * y_pick.y;

    var dx = x_world - centre_world;
    var dy = y_world - centre_world;
    // Preserve +screen-axis orientation even when the closest depth neighbor is
    // the negative pixel. This keeps the slice frame stable across depth edges.
    dx *= x_pick.x;
    dy *= y_pick.x;

    var normal = normalize(cross(dx, dy));
    let to_camera = -centre_ray;
    if (dot(normal, to_camera) < 0.0) {
        normal = -normal;
    }

    let same_depth_x = (camera + x_ray * centre_depth - centre_world) * x_pick.x;
    let same_depth_y = (camera + y_ray * centre_depth - centre_world) * y_pick.x;
    let world_per_pixel = max(0.5 * (length(same_depth_x) + length(same_depth_y)), 1e-4);

    var frame: SurfaceFrame;
    frame.centre_world = centre_world;
    frame.centre_depth = centre_depth;
    frame.view_ray = centre_ray;
    frame.normal = normal;
    frame.screen_x_world = same_depth_x;
    frame.screen_y_world = same_depth_y;
    frame.world_per_pixel = world_per_pixel;
    return frame;
}

fn quantized_edge(cdf: f32, group_jitter: f32) -> u32 {
    // The reference jitters the continuous horizon by rnd / 32 before
    // quantization. Adding rnd before floor() after multiplying by 32 is the
    // same operation and saves one multiply.
    let scaled = clamp(cdf, 0.0, 1.0) * f32(SECTOR_COUNT);
    return u32(clamp(floor(scaled + group_jitter), 0.0, f32(SECTOR_COUNT)));
}

fn sector_interval_mask(start_edge: u32, end_edge: u32) -> u32 {
    // Preserve the reference interval orientation. Do not sort the endpoints:
    // an inverted interval is empty, not an occluded arc.
    if (start_edge >= end_edge) {
        return 0u;
    }

    var from_start = 0u;
    if (start_edge < 32u) {
        from_start = 0xffffffffu << start_edge;
    }

    var to_end = 0u;
    if (end_edge >= 32u) {
        to_end = 0xffffffffu;
    } else if (end_edge > 0u) {
        to_end = 0xffffffffu >> (32u - end_edge);
    }

    return from_start & to_end;
}

fn slice_axis_to_screen_direction(frame: SurfaceFrame, slice_axis: vec3<f32>) -> vec2<f32> {
    // GT-VBAO samples slices uniformly in the plane around the view vector and
    // then projects those slices into screen space. Invert the local
    // screen-to-world differential here rather than choosing a uniform screen
    // angle (which is perspective-biased away from the image centre).
    let dx = frame.screen_x_world;
    let dy = frame.screen_y_world;
    let xx = dot(dx, dx);
    let xy = dot(dx, dy);
    let yy = dot(dy, dy);
    let det = xx * yy - xy * xy;
    if (abs(det) <= 1e-12) {
        return vec2<f32>(1.0, 0.0);
    }

    let rhs_x = dot(slice_axis, dx);
    let rhs_y = dot(slice_axis, dy);
    let px = (rhs_x * yy - rhs_y * xy) / det;
    let py = (rhs_y * xx - rhs_x * xy) / det;
    let screen = vec2<f32>(px, py);
    let len = length(screen);
    if (len <= 1e-6) {
        return vec2<f32>(1.0, 0.0);
    }
    return screen / len;
}

fn sample_slice_axis(view_to_camera: vec3<f32>, angle: f32) -> vec3<f32> {
    // Build an orthonormal basis around V and sample a slice direction in that
    // local frame, matching the perspective-correct construction in GT-VBAO.
    let helper = select(
        vec3<f32>(0.0, 0.0, 1.0),
        vec3<f32>(0.0, 1.0, 0.0),
        abs(view_to_camera.z) > 0.95
    );
    let tangent = normalize(cross(helper, view_to_camera));
    let bitangent = cross(view_to_camera, tangent);
    return normalize(tangent * cos(angle) + bitangent * sin(angle));
}

fn sample_visibility_slice(
    frame: SurfaceFrame,
    uv: vec2<f32>,
    slice_axis: vec3<f32>,
    screen_direction: vec2<f32>,
    group_jitter: f32
) -> f32 {
    let view_to_camera = -frame.view_ray;
    let slice_plane_normal = normalize(cross(view_to_camera, slice_axis));
    let projected_normal = frame.normal -
        slice_plane_normal * dot(frame.normal, slice_plane_normal);
    let projected_len = length(projected_normal);
    if (projected_len <= 1e-5) {
        return 1.0;
    }

    // This is the sinN term from the optimized uniformly weighted reference:
    // T = cross(sliceN, projN), sinN = dot(T, V) / |projN|.
    let angular_tangent = cross(slice_plane_normal, projected_normal);
    let sin_n = clamp(dot(angular_tangent, view_to_camera) / projected_len, -1.0, 1.0);

    let full_dims = max(settings.viewport_history.xy, vec2<f32>(1.0));
    let centre_pixel = clamp_uv(uv) * full_dims;
    let radius_pixels = clamp(AO_RADIUS_WORLD / frame.world_per_pixel, 2.0, AO_MAX_PIXEL_RADIUS);

    var sectors = 0u;
    for (var sample_index = 0u; sample_index < SAMPLES_PER_SIDE; sample_index = sample_index + 1u) {
        // Practical low-sample distribution. The original reference uses 32
        // exponentially spaced samples per side; at two samples per side this
        // stratified distribution preserves a near and far horizon probe
        // without pow/log instructions in the hot loop.
        let ordinal = (f32(sample_index) + 0.5 + group_jitter * 0.35) /
            f32(SAMPLES_PER_SIDE);
        let radial_fraction = 0.12 + 0.88 * ordinal;
        let pixel_distance = max(radius_pixels * radial_fraction, 1.25);

        for (var side = 0u; side < 2u; side = side + 1u) {
            let d = select(-1.0, 1.0, side == 1u);
            let signed_pixels = screen_direction * (pixel_distance * d);
            let sample_full_pixel = centre_pixel + signed_pixels;
            if (sample_full_pixel.x < 0.0 || sample_full_pixel.x >= full_dims.x ||
                sample_full_pixel.y < 0.0 || sample_full_pixel.y >= full_dims.y) {
                continue;
            }

            let sample_depth = depth_at_pixel(vec2<i32>(floor(sample_full_pixel)));
            if (!valid_depth(sample_depth)) {
                continue;
            }

            // Reconstruct the sample ray from the centre ray plus the local
            // screen-space differential. This avoids an inverse-matrix multiply
            // for every AO tap while preserving the perspective-correct slice.
            let same_depth_offset =
                frame.screen_x_world * signed_pixels.x +
                frame.screen_y_world * signed_pixels.y;
            let sample_ray = normalize(
                frame.view_ray * frame.centre_depth + same_depth_offset
            );
            let sample_world = settings.camera_pos_time.xyz + sample_ray * sample_depth;
            let delta_front = sample_world - frame.centre_world;
            let sample_distance = length(delta_front);
            if (sample_distance <= 1e-4 || sample_distance > AO_RADIUS_WORLD * 1.35) {
                continue;
            }

            // Our linear depth is camera-ray distance, so adding thickness along
            // each sample's own ray is the equivalent of the reference's
            // perspective-correct sampleDepth + Thickness reconstruction.
            let thickness = clamp(8.0 + sample_depth * 0.002, 8.0, 28.0);
            let delta_back =
                (sample_world + sample_ray * thickness) - frame.centre_world;

            var horizon_cos = vec2<f32>(
                dot(normalize(delta_front), view_to_camera),
                dot(normalize(delta_back), view_to_camera)
            );

            // Sampling direction flips the front/back horizon ordering.
            if (d < 0.0) {
                horizon_cos = horizon_cos.yx;
            }

            // Mirko Salm's optimized uniformly weighted GT-VBAO mapping:
            //
            // hor01 = ((0.5 + 0.5*sinN) + d/2) - (d/2)*horCos
            //
            // This is the key no-acos optimization from the supplied shader.
            let d05 = d * 0.5;
            var horizon_01 =
                vec2<f32>((0.5 + 0.5 * sin_n) + d05) -
                vec2<f32>(d05) * horizon_cos;
            horizon_01 = clamp(horizon_01, vec2<f32>(0.0), vec2<f32>(1.0));

            let front_edge = quantized_edge(horizon_01.x, group_jitter);
            let back_edge = quantized_edge(horizon_01.y, group_jitter);
            sectors |= sector_interval_mask(front_edge, back_edge);
        }
    }

    let occluded = f32(countOneBits(sectors)) / f32(SECTOR_COUNT);
    return 1.0 - occluded;
}

fn current_ssao(uv: vec2<f32>, centre_depth: f32) -> f32 {
    if (!valid_depth(centre_depth)) {
        return 1.0;
    }

    let frame = reconstruct_surface_frame(uv, centre_depth);
    let full_dims = max(settings.viewport_history.xy, vec2<f32>(1.0));
    let pixel = floor(clamp_uv(uv) * full_dims);
    let frame_index = settings.viewport_history.w;
    let noise = interleaved_gradient_noise(pixel, frame_index);
    let view_to_camera = -frame.view_ray;

    // A slice is unoriented, so PI covers the full set of slice planes. Rotate
    // the two planes over time; temporal reprojection integrates the residual
    // low-sample noise while keeping the per-frame kernel at eight depth taps.
    let base_angle = fract(noise + frame_index * 0.61803398875) * PI;

    var visibility = 0.0;
    for (var slice_index = 0u; slice_index < SLICE_COUNT; slice_index = slice_index + 1u) {
        let angle = base_angle + f32(slice_index) * (PI / f32(SLICE_COUNT));
        let slice_axis = sample_slice_axis(view_to_camera, angle);
        let screen_direction = slice_axis_to_screen_direction(frame, slice_axis);
        let jitter = fract(noise + f32(slice_index) * 0.38196601125);
        visibility += sample_visibility_slice(
            frame,
            uv,
            slice_axis,
            screen_direction,
            jitter
        );
    }

    visibility /= f32(SLICE_COUNT);
    return clamp(1.0 - (1.0 - visibility) * AO_STRENGTH, 0.0, 1.0);
}

fn previous_uv(world: vec3<f32>) -> vec3<f32> {
    let clip = settings.prev_view_proj * vec4<f32>(world, 1.0);
    if (clip.w <= 1e-5) {
        return vec3<f32>(-1.0, -1.0, -1.0);
    }
    let ndc = clip.xyz / clip.w;
    return vec3<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5, ndc.z);
}

fn temporal_ssao(uv: vec2<f32>, centre_depth: f32, current: f32) -> f32 {
    if (settings.viewport_history.z <= 0.5 || !valid_depth(centre_depth)) {
        return current;
    }

    let world = world_position(uv, centre_depth);
    let reprojection = previous_uv(world);
    if (reprojection.x <= 0.0 || reprojection.x >= 1.0 ||
        reprojection.y <= 0.0 || reprojection.y >= 1.0) {
        return current;
    }

    let previous = textureSample(history_texture, history_sampler, reprojection.xy);
    if (!valid_depth(previous.g)) {
        return current;
    }

    let expected_previous_depth = distance(settings.previous_camera_pos.xyz, world);
    let depth_tolerance = max(8.0, expected_previous_depth * 0.015);
    if (abs(previous.g - expected_previous_depth) > depth_tolerance) {
        return current;
    }

    let history_dims = vec2<f32>(textureDimensions(history_texture));
    let motion_pixels = length((reprojection.xy - uv) * history_dims);
    let history_weight = clamp(0.90 - motion_pixels * 0.060, 0.0, 0.90);
    // Keep temporal accumulation from smearing a newly exposed contact shadow.
    let previous_ao = clamp(previous.r, current - 0.16, current + 0.16);
    return mix(current, previous_ao, history_weight);
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let centre_depth = depth_at_uv(input.uv);
    if (!valid_depth(centre_depth)) {
        return vec4<f32>(1.0, 0.0, 0.0, 1.0);
    }

    let current = current_ssao(input.uv, centre_depth);
    let ao = temporal_ssao(input.uv, centre_depth, current);
    return vec4<f32>(ao, centre_depth, 0.0, 1.0);
}
