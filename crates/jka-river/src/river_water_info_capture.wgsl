// WaterInfoRendering default method 2, DrawWaterInfo's actual vertex graph,
// SceneCapturePixelShader / CreateInvDeviceZToWorldZTransform (orthographic).
// Original algorithms Copyright Epic Games, Inc. All Rights Reserved.
struct Camera { projection: vec4<f32>, extent: vec4<f32> }
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var scene_depth: texture_depth_2d_array;
@group(0) @binding(2) var water_color: texture_2d<f32>;
struct Pixel { depths: vec4<f32>, velocity: vec4<f32> }
@group(0) @binding(3) var<storage,read_write> pixels: array<Pixel>;
struct Vertex { @builtin(position) position: vec4<f32>, @location(0) flow: vec2<f32> }
@vertex fn capture_vertex(@location(0) position:vec3<f32>,@location(1) color:vec4<f32>)->Vertex {
    var v:Vertex;
    let depth=camera.projection.z-position.z;
    let xy=(position.xy-camera.projection.xy)/camera.extent.xy*vec2<f32>(2.0,-2.0);
    v.position=vec4<f32>(xy,1.0-depth/camera.projection.w,1.0);
    // Preserve the material's multiply/trig/divide order. This executes BEFORE
    // interpolation, unlike interpolating packed angles in a fragment shader.
    let magnitude=color.x*camera.extent.z;
    let direction=color.y*2.0*3.14159265358979323846;
    let flow_x=magnitude*cos(direction);
    let flow_y=magnitude*sin(direction);
    v.flow=vec2<f32>(flow_x/(2.0*camera.extent.z)+0.5,flow_y/(2.0*camera.extent.z)+0.5);
    return v;
}
@fragment fn capture_flow(v:Vertex)->@location(0) vec4<f32> {
    return vec4<f32>(v.flow,0.0,0.0);
}
fn linear_depth(at:vec2<i32>,layer:i32)->f32 {
    let device_z=textureLoad(scene_depth,at,layer,0);
    // Native reversed-Z orthographic inverse transform: [-far,far+1,0,1].
    // Clear device Z=0 therefore resolves to far, not zero scene depth.
    return device_z*(-camera.projection.w)+(camera.projection.w+1.0)-1.0;
}
@compute @workgroup_size(8,8)
fn resolve(@builtin(global_invocation_id) id:vec3<u32>) {
    let size=textureDimensions(scene_depth);
    if any(id.xy>=size) { return; }
    let at=vec2<i32>(id.xy);
    let color=textureLoad(water_color,at,0);
    pixels[id.y*size.x+id.x]=Pixel(
        vec4<f32>(linear_depth(at,0),linear_depth(at,1),linear_depth(at,2),0.0),
        vec4<f32>(color.xy,0.0,0.0));
}
