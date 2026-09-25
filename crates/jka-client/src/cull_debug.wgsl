struct Camera {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var<storage, read> rejection_reason: array<u32>;

struct VertexIn {
    @location(0) position: vec3<f32>,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) @interpolate(flat) reason: u32,
};

@vertex
fn vs_main(input: VertexIn, @builtin(instance_index) cull_index: u32) -> VertexOut {
    var output: VertexOut;
    output.clip_position = camera.view_proj * vec4<f32>(input.position, 1.0);
    output.reason = rejection_reason[cull_index];
    return output;
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    // 0 = visible, 1 = frustum rejected, 2 = Hi-Z rejected,
    // 3 = PVS rejected, 4 = snapshot areamask rejected.
    // Visible surfaces are discarded so this pass is purely diagnostic.
    if (input.reason == 0u) {
        discard;
    }
    if (input.reason == 1u) {
        return vec4<f32>(1.0, 0.12, 0.04, 0.44);
    }
    if (input.reason == 2u) {
        return vec4<f32>(1.0, 0.08, 0.82, 0.44);
    }
    if (input.reason == 3u) {
        return vec4<f32>(0.05, 0.86, 1.0, 0.36);
    }
    if (input.reason == 4u) {
        return vec4<f32>(1.0, 0.86, 0.08, 0.40);
    }
    // Unknown rejection reasons should never occur. Return fully transparent
    // instead of ending the function with `discard`; FXC requires an explicit
    // return on every syntactic control-flow path for a value-returning entry point.
    return vec4<f32>(0.0);
}
