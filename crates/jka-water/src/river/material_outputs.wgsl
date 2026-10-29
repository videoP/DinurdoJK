// Native default NormalGrid R16f quantization, stage-17 normal blur, RGBA16f
// render outputs and Niagara linear post-simulate mips. No sRGB conversion.
// Copyright Epic Games, Inc. All Rights Reserved. presentation-reference.json.
struct Settings { dimensions:vec2<u32>, inverse_size:vec2<f32> }
@group(0) @binding(0) var<uniform> settings:Settings;
@group(0) @binding(1) var<storage,read> raw_surface:array<vec4<f32>>;
@group(0) @binding(2) var<storage,read> raw_normals:array<vec4<f32>>;
@group(0) @binding(3) var surface_out:texture_storage_2d<rgba16float,write>;
@group(0) @binding(4) var normals_grid_out:texture_storage_2d<rgba16float,write>;
@compute @workgroup_size(8,8)
fn write_fields(@builtin(global_invocation_id) id:vec3<u32>) {
    if any(id.xy>=settings.dimensions) { return; }
    let index=id.x+id.y*settings.dimensions.x;
    textureStore(surface_out,id.xy,raw_surface[index]);
    textureStore(normals_grid_out,id.xy,raw_normals[index]);
}
@group(0) @binding(5) var normals_grid:texture_2d<f32>;
@group(0) @binding(6) var normals_out:texture_storage_2d<rgba16float,write>;
@compute @workgroup_size(8,8)
fn blur_normals(@builtin(global_invocation_id) id:vec3<u32>) {
    if any(id.xy>=settings.dimensions) { return; }
    var total=0.0; var sum=vec3<f32>(0.0);
    for (var xx=-1;xx<=1;xx++) { for (var yy=-1;yy<=1;yy++) {
        let at=clamp(vec2<i32>(id.xy)+vec2<i32>(xx,yy),vec2<i32>(0),vec2<i32>(settings.dimensions)-vec2<i32>(1));
        let weight=length(vec2<f32>(1.0)-abs(vec2<f32>(f32(xx),f32(yy)))/2.0);
        total+=weight; sum+=textureLoad(normals_grid,at,0).xyz*weight;
    }}
    if total>1e-5 { sum/=total; }
    let magnitude=length(sum);
    if magnitude>1e-5 { sum/=magnitude; }
    textureStore(normals_out,id.xy,vec4<f32>(sum,0.0));
}
@group(0) @binding(7) var mip_in:texture_2d<f32>;
@group(0) @binding(8) var mip_sampler:sampler;
@group(0) @binding(9) var mip_out:texture_storage_2d<rgba16float,write>;
@compute @workgroup_size(8,8)
fn generate_mip(@builtin(global_invocation_id) id:vec3<u32>) {
    if any(id.xy>=settings.dimensions) { return; }
    let uv=(vec2<f32>(id.xy)+vec2<f32>(0.5))*settings.inverse_size;
    textureStore(mip_out,id.xy,textureSampleLevel(mip_in,mip_sampler,uv,0.0));
}
