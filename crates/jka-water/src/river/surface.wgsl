// Selected live MF_ShallowWaterRiver normal/mask pins; surface-reference.json.
// Copyright Epic Games, Inc. All Rights Reserved.
// Requires material.wgsl and wave_advection.wgsl. Foam and scene inputs are
// upstream dependencies, not defaults or a substitute Single Layer Water shader.
fn river_blend_normals(n1:vec3<f32>,n2:vec3<f32>)->vec3<f32> {
    let a=1.0/(1.0+n1.z);let b=-n1.x*n1.y*a;
    var b1=vec3<f32>(1.0-n1.x*n1.x*a,b,-n1.x);
    var b2=vec3<f32>(b,1.0-n1.y*n1.y*a,-n1.y);
    if n1.z < -0.9999999 {b1=vec3<f32>(0.0,-1.0,0.0);b2=vec3<f32>(-1.0,0.0,0.0);}
    return n2.x*b1+n2.y*b2+n2.z*n1;
}
fn river_safe_normalize(n:vec3<f32>)->vec3<f32> {
    let squared=dot(n,n);
    // Native If equality pin is connected, with the default EqualsThreshold.
    if abs(squared-1e-6)>1e-5 && squared>=1e-6 {return normalize(n);}
    return vec3<f32>(0.0,0.0,1.0);
}
fn river_surface_normal(base:vec3<f32>,wpo_rgb:vec3<f32>,detail_rgb:vec3<f32>,magnitude:f32,motion:RiverMotion,height:RiverWaveHeight,foam_normal:vec3<f32>,total_foam:f32,foam_opacity:f32,opacity_mask:f32)->vec3<f32> {
    let atten=mix(1.0,smoothstep(height.attenuation.y,height.attenuation.z,magnitude),1.0-clamp(height.attenuation.x,0.0,1.0));
    var power=0.0;if atten>2.980233e-8 {power=pow(atten,height.attenuation.w);}
    let with_wpo=mix(base,river_blend_normals(base,wpo_rgb),clamp(power*height.camera.x,0.0,1.0));
    let detail_atten=mix(1.0,smoothstep(motion.attenuation.y,motion.attenuation.z,magnitude),1.0-clamp(motion.attenuation.x,0.0,1.0))*motion.attenuation.w;
    let with_detail=mix(with_wpo,river_blend_normals(with_wpo,detail_rgb),clamp(detail_atten,0.0,1.0));
    let foam=river_blend_normals(with_wpo,foam_normal)*foam_opacity;
    var result=mix(with_detail,foam,total_foam);
    if opacity_mask<1e-8 {result=vec3<f32>(0.0);}
    return river_safe_normalize(result);
}
struct RiverMasks {water_opacity:f32,simulation_mask:f32,material_mask:f32}
fn river_surface_masks(depth:f32,opacity:f32,uv:vec2<f32>,dimensions:vec2<f32>,incoming_mask:f32,pixel_depth:f32,custom_depth:f32)->RiverMasks {
    let water_opacity=select(0.0,1.0,depth>1e-8)*opacity;
    let dx=1.0/dimensions;
    let inside=all(uv>=dx)&&all(uv<=vec2<f32>(1.0)-dx);
    let sim_mask=select(0.0,1.0,inside&&water_opacity>1e-8);
    let custom_visible=select(0.0,1.0,abs(pixel_depth-custom_depth)>1e-5&&pixel_depth<custom_depth);
    return RiverMasks(water_opacity,sim_mask,(incoming_mask*sim_mask)*custom_visible);
}
// Fragment-only selected live texture path. Advection's footprint uses original
// fluid velocity derivatives, not its bracketed/projected/scaled velocities.
// Both FFT RGB samples stay linear, without a normal-map decode or early normalize.
fn river_live_grid_normal(fluid_tex:texture_2d<f32>,normal_tex:texture_2d<f32>,normal_sampler:sampler,detail_tex:texture_2d<f32>,detail_sampler:sampler,uv:vec2<f32>,world_xy:vec2<f32>,time:f32,motion:RiverMotion,wpo_advection:RiverAdvection,detail_advection:RiverAdvection,height:RiverWaveHeight,foam_normal:vec3<f32>,total_foam:f32,foam_opacity:f32,opacity_mask:f32)->vec3<f32> {
    let fluid=river_live_field(fluid_tex,uv);
    let base=textureSampleLevel(normal_tex,normal_sampler,uv,0.0).xyz;
    let wpo=river_wave_fragment(detail_tex,detail_sampler,world_xy,fluid.zw,base,time,wpo_advection).xyz;
    let detail=river_wave_fragment(detail_tex,detail_sampler,world_xy,fluid.zw,base,time,detail_advection).xyz;
    return river_surface_normal(base,wpo,detail,length(fluid.zw),motion,height,foam_normal,total_foam,foam_opacity,opacity_mask);
}
