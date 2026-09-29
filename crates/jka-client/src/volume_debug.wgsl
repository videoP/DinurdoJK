// Trigger / clip-brush debug overlay. Static geometry, alpha-blended, depth
// tested without writes; positions arrive already in renderer coordinates.
struct CameraUniform {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: CameraUniform;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec3<f32>,
};

@vertex
fn vs_main(input: VsIn) -> VsOut {
    var out: VsOut;
    var clip = camera.view_proj * vec4<f32>(input.position, 1.0);
    // Brush faces are usually coplanar with the visible world. Pull them 0.2%
    // toward the camera (reverse-Z: larger is nearer) so they do not z-fight;
    // hardware depth bias is not allowed on line topologies.
    clip.z = clip.z * 1.002;
    out.position = clip;
    out.color = input.color.rgb;
    return out;
}

@fragment
fn fs_fill(input: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(input.color, 0.20);
}

@fragment
fn fs_line(input: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(input.color, 0.90);
}
