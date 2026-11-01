// Reversed-Z furthest-depth HZB, as UE HZBReduce.usf: min keeps a conservative
// occluder at coarse mips. The first level covers 2x2 full-resolution pixels.
@group(0) @binding(0) var depth_in: texture_depth_2d;
@group(0) @binding(1) var previous: texture_2d<f32>;
@group(0) @binding(2) var output: texture_storage_2d<r32float,write>;
@compute @workgroup_size(8,8)
fn first(@builtin(global_invocation_id) id:vec3<u32>) {
    if any(id.xy>=textureDimensions(output)) {return;}
    if any(id.xy*2u>=textureDimensions(depth_in)) {textureStore(output,id.xy,vec4<f32>(0.0));return;}
    let hi=vec2<i32>(textureDimensions(depth_in))-1;
    let p=vec2<i32>(id.xy)*2;
    let d=min(min(textureLoad(depth_in,min(p,hi),0),textureLoad(depth_in,min(p+vec2<i32>(1,0),hi),0)),
              min(textureLoad(depth_in,min(p+vec2<i32>(0,1),hi),0),textureLoad(depth_in,min(p+vec2<i32>(1,1),hi),0)));
    textureStore(output,id.xy,vec4<f32>(d));
}
@compute @workgroup_size(8,8)
fn reduce(@builtin(global_invocation_id) id:vec3<u32>) {
    if any(id.xy>=textureDimensions(output)) {return;}
    let hi=vec2<i32>(textureDimensions(previous))-1;
    let p=vec2<i32>(id.xy)*2;
    let d=min(min(textureLoad(previous,min(p,hi),0).x,textureLoad(previous,min(p+vec2<i32>(1,0),hi),0).x),
              min(textureLoad(previous,min(p+vec2<i32>(0,1),hi),0).x,textureLoad(previous,min(p+vec2<i32>(1,1),hi),0).x));
    textureStore(output,id.xy,vec4<f32>(d));
}
