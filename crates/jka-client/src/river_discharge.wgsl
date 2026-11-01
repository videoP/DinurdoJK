// Host volumetric source adapter. The Unreal hydraulic stages are unchanged.
// Normalize Q*dt over the actual open source cells, independently of resolution.
struct Source {rect:vec4<u32>,metrics:vec4<f32>} // cell area cm2, inlet Z, volume cm3, unused
@group(0) @binding(0) var<uniform> source:Source;
@group(0) @binding(1) var<storage,read> geometry:array<vec4<f32>>;
@group(0) @binding(2) var<storage,read_write> count:atomic<u32>;
@group(0) @binding(3) var water:texture_2d_array<f32>;
@group(0) @binding(4) var output_water:texture_storage_2d_array<r32float,write>;

fn eligible(p:vec2<u32>,width:u32)->bool {
    if any(p<source.rect.xy)||any(p>=source.rect.zw) {return false;}
    let bed=geometry[p.y*width+p.x];
    return bed.y<1.0-1e-5&&bed.x<source.metrics.y;
}
@compute @workgroup_size(8,8)
fn count_cells(@builtin(global_invocation_id) id:vec3<u32>) {
    // Dispatch only the bounded inlet rectangle. Width comes from the geometry
    // allocation via a separate value in metrics.w, avoiding hydraulic reads.
    let p=id.xy+source.rect.xy;
    if any(p>=source.rect.zw) {return;}
    if eligible(p,u32(source.metrics.w)) {atomicAdd(&count,1u);}
}
@compute @workgroup_size(8,8)
fn inject(@builtin(global_invocation_id) id:vec3<u32>) {
    let dims=textureDimensions(water);
    if any(id.xy>=dims) {return;}
    let p=vec2<i32>(id.xy);
    var h=textureLoad(water,p,0,0).r;
    var depth=textureLoad(water,p,1,0).r;
    let cells=atomicLoad(&count);
    if cells>0u&&eligible(id.xy,dims.x) {
        depth+=source.metrics.z/(f32(cells)*source.metrics.x);
        h=geometry[id.y*dims.x+id.x].x+depth;
    }
    textureStore(output_water,p,0,vec4<f32>(h));
    textureStore(output_water,p,1,vec4<f32>(depth));
    textureStore(output_water,p,2,vec4<f32>(textureLoad(water,p,2,0).r));
    textureStore(output_water,p,3,vec4<f32>(textureLoad(water,p,3,0).r));
}
