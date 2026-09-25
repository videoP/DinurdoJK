struct CameraUniform {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: CameraUniform;

struct VsIn {
    @location(0) position: vec3<f32>,
};

struct VsOut {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs_main(input: VsIn) -> VsOut {
    var out: VsOut;
    out.position = camera.view_proj * vec4<f32>(input.position, 1.0);
    return out;
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 1.0, 0.0, 1.0);
}
