struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) ring: vec3<f32>,
}
@vertex
fn vertex(@builtin(vertex_index) i: u32, @location(0) position: vec4<f32>, @location(1) shape: vec4<f32>) -> Out {
    let corners = array<vec2<f32>,6>(vec2(-1.,-1.),vec2(1.,-1.),vec2(-1.,1.),vec2(-1.,1.),vec2(1.,-1.),vec2(1.,1.));
    let p = corners[i];
    let uv = position.xy + p * position.z;
    var out: Out;
    out.clip = vec4<f32>(uv * vec2<f32>(2.,-2.) + vec2<f32>(-1.,1.),0.,1.);
    out.local = p * shape.x;
    out.ring = vec3<f32>(position.w, shape.y, shape.z);
    return out;
}
@fragment
fn fragment(input: Out) -> @location(0) vec4<f32> {
    let distance = length(input.local);
    let t = (distance - input.ring.x) / input.ring.y;
    if abs(t) >= 1.0 { discard; }
    // Derivative of a compact smooth ring; positive and negative slopes balance.
    let slope = -4.0 * t * (1.0-t*t) * input.ring.z / input.ring.y;
    let disturbance = -input.local / max(distance,1e-4) * slope;
    return vec4<f32>(disturbance,0.,0.);
}
