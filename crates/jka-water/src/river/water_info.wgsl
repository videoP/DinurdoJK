// Source: WaterInfoMerge.usf / WaterInfoFinalize.usf / WaterDataFunctions.ush.
// Original algorithms Copyright Epic Games, Inc. All Rights Reserved.
// FIELD_FORMAT is instantiated to the selected native RGBA16f / RGBA32f.
struct Parameters {
    dimensions: vec4<u32>, // input width/height, clamped blur radius, unused
    simulation: vec4<u32>, // output grid width/height
    heights: vec4<f32>, // water min/max, ground min, capture Z
    controls: vec4<f32>, // underground offset, max flow (1024), origin Z
    grid: vec4<f32>, // world min XY, size XY
    zone: vec4<f32>, // world min XY, size XY
}
struct Pixel { depths: vec4<f32>, velocity: vec4<f32> }
@group(0) @binding(0) var<uniform> p: Parameters;
@group(0) @binding(1) var<storage,read> input_pixels: array<Pixel>;
@group(0) @binding(2) var encoded_input: texture_2d<f32>;
@group(0) @binding(3) var encoded_output: texture_storage_2d<FIELD_FORMAT,write>;
@group(0) @binding(4) var zone_sampler: sampler;
@group(0) @binding(5) var<storage,read_write> decoded: array<vec4<f32>>;
fn normalized(height:f32, minimum:f32, maximum:f32)->f32 {
    if minimum==maximum { return 1.0; }
    return clamp((height-minimum)/(maximum-minimum),0.0,1.0);
}
@compute @workgroup_size(8,8)
fn merge(@builtin(global_invocation_id) id:vec3<u32>) {
    if any(id.xy>=p.dimensions.xy) { return; }
    let input=input_pixels[id.y*p.dimensions.x+id.x];
    let ground=max(p.heights.w-input.depths.x,p.heights.z);
    let water=p.heights.w-input.depths.y;
    var dilation=p.heights.w-input.depths.z;
    let has_dilation=dilation<=p.heights.y && dilation>=p.heights.x;
    var valid_water=water>=p.heights.x;
    if has_dilation && water+128.0<ground { valid_water=false; }
    let g=normalized(ground,p.heights.z,p.heights.y);
    var result=vec4<f32>(0.0,0.0,0.0,g);
    if valid_water { result=vec4<f32>(input.velocity.xy,normalized(water,p.heights.x,p.heights.y),g); }
    else if has_dilation {
        if dilation>ground { dilation=ground-p.controls.x; }
        result=vec4<f32>(0.0,0.0,normalized(dilation,p.heights.x,p.heights.y),g);
    }
    textureStore(encoded_output,vec2<i32>(id.xy),result);
}
fn load_encoded(at:vec2<i32>)->vec4<f32> {
    if any(at<vec2<i32>(0)) || any(at>=vec2<i32>(p.dimensions.xy)) { return vec4<f32>(0.0); }
    return textureLoad(encoded_input,at,0);
}
@compute @workgroup_size(8,8)
fn finalize(@builtin(global_invocation_id) id:vec3<u32>) {
    if any(id.xy>=p.dimensions.xy) { return; }
    var accum=vec2<f32>(0.0); var neighbours:f32=0.0;
    let radius=i32(p.dimensions.z);
    for(var x=-radius;x<=radius;x++) { for(var y=-radius;y<=radius;y++) {
        let v=load_encoded(vec2<i32>(id.xy)+vec2<i32>(x,y));
        let water=v.z*(p.heights.y-p.heights.x)+p.heights.x;
        let ground=v.w*(p.heights.y-p.heights.z)+p.heights.z;
        if water>ground { accum+=v.xy; neighbours+=1.0; }
    }}
    if neighbours>0.0 { accum/=neighbours; }
    let v=load_encoded(vec2<i32>(id.xy));
    textureStore(encoded_output,vec2<i32>(id.xy),vec4<f32>(accum,v.zw));
}
@compute @workgroup_size(8,8)
fn decode(@builtin(global_invocation_id) id:vec3<u32>) {
    if any(id.xy>=p.simulation.xy) { return; }
    let unit=(vec2<f32>(id.xy)+0.5)/vec2<f32>(p.simulation.xy);
    let world=unit*p.grid.zw+p.grid.xy;
    let uv=(world-p.zone.xy)/p.zone.zw;
    // RenderTarget2DArray's resource sampler wraps. It is NOT the hydraulic
    // Grid2DCollection's clamp sampler. Explicit LOD zero has linear filtering.
    let v=textureSampleLevel(encoded_input,zone_sampler,uv,0.0);
    let ground=v.w*(p.heights.y-p.heights.z)+p.heights.z;
    var water=v.z*(p.heights.y-p.heights.x)+p.heights.x;
    if v.z<=0.0 { water=ground-256.0; }
    let velocity=(v.xy-0.5)*2.0*p.controls.y;
    decoded[id.y*p.simulation.x+id.x]=vec4<f32>(water-p.controls.z,water-ground,velocity);
}
