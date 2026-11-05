// Orthographic R32f scene-depth capture for WaterAdvanced's top/under inputs.
// XY is stored canonically (rows increase with local Y); underside culling is
// reversed by the pipeline so it sees the other side of single-sided meshes.
struct Camera { projection: vec4<f32>, clip: vec4<f32> };
@group(0) @binding(0) var<uniform> camera: Camera;
struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) depth: f32,
};
@vertex fn capture_vertex(@location(0) position: vec3<f32>) -> Vertex {
    let depth = (position.z - camera.projection.y) * camera.projection.z;
    let xy = position.xy / camera.projection.x * vec2<f32>(2.0, -2.0);
    var out: Vertex;
    out.position = vec4<f32>(xy, (depth - camera.clip.x) / (camera.clip.y - camera.clip.x), 1.0);
    out.depth = depth;
    return out;
}
@fragment fn capture_depth(in: Vertex) -> @location(0) f32 {
    return in.depth;
}
