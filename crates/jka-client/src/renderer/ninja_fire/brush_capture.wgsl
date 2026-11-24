// Collision alpha of Fluid Ninja's MeshField capture from solid BSP brushes,
// the GPU substitute for UE's Ninja_SDF_ReadAndPaint distance-field read. One
// invocation per capture texel of the camera-facing flame plane.

struct CaptureParams {
    // Plane centre (render cm); PainterEdgeMaskWidth.
    centre: vec4<f32>,
    // View right; field width (cm).
    right: vec4<f32>,
    // View up; field height (cm).
    up: vec4<f32>,
    // Columns, rows, brush count.
    size: vec4<u32>,
}

@group(0) @binding(0) var<uniform> capture: CaptureParams;
// brush_collision.rs pack_brushes: per brush two headers (min.xyz + first
// plane index bits, max.xyz + plane count bits), then planes (normal, distance).
@group(0) @binding(1) var<storage, read> capture_brushes: array<vec4<f32>>;
@group(0) @binding(2) var capture_out: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8)
fn capture_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let columns = capture.size.x;
    let rows = capture.size.y;
    if (id.x >= columns || id.y >= rows) { return; }
    let u = (f32(id.x) + 0.5) / f32(columns);
    let v = (f32(id.y) + 0.5) / f32(rows);
    let width = capture.right.w;
    let height = capture.up.w;
    let point = capture.centre.xyz + capture.up.xyz * (height * 0.5 - v * height)
        + capture.right.xyz * (u * width - width * 0.5);
    // SDF_ReadAndPaint: abs(min(distance, 0)), default multiplier 1 and
    // offset 0, multiplied by the capture-stage EdgeMask.
    let edge_width = capture.centre.w;
    let edge_scale = 1.0 / max(edge_width, 0.001);
    let axis = max(abs(2.0 * edge_scale * u - edge_scale), abs(2.0 * edge_scale * v - edge_scale));
    let raw_edge = clamp(edge_scale - 0.01 - axis, 0.0, 1.0);
    let edge = select(pow(raw_edge, 0.1), select(0.0, 1.0, raw_edge >= 1.0), edge_width <= 0.05);
    var alpha = 0.0;
    for (var b = 0u; b < capture.size.z; b++) {
        let low = capture_brushes[2u * b];
        let high = capture_brushes[2u * b + 1u];
        if (any(point < low.xyz) || any(point > high.xyz)) { continue; }
        let first = bitcast<u32>(low.w);
        let count = bitcast<u32>(high.w);
        // For an interior point of a convex brush, the maximum signed plane
        // distance is its signed distance to the brush boundary.
        var nearest = -1e30;
        for (var i = 0u; i < count; i++) {
            let plane = capture_brushes[first + i];
            nearest = max(nearest, dot(plane.xyz, point) - plane.w);
        }
        if (nearest < 0.0) { alpha = max(alpha, min(-nearest * edge, 65504.0)); }
    }
    textureStore(capture_out, vec2<i32>(id.xy), vec4<f32>(0.0, 0.0, 0.0, alpha));
}
