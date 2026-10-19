#import naval_clouds::shadow::{CloudShadow, cloud_shadow}
#import naval_sky::environment::{SkyEnvironment, env_irradiance}
struct Frame { inverse_vp:mat4x4<f32>,eye:vec4<f32>,viewport:vec4<f32>,options:vec4<f32> }
@group(0) @binding(0) var<uniform> frame:Frame;
@group(0) @binding(1) var scene_depth:texture_depth_2d;
@group(0) @binding(2) var scene_color:texture_2d<f32>;
@group(1) @binding(0) var shadow_map:texture_2d<f32>;
@group(1) @binding(1) var shadow_sampler:sampler;
@group(1) @binding(2) var<uniform> shadow:CloudShadow;
@group(2) @binding(0) var env_map:texture_2d<f32>;
@group(2) @binding(1) var env_sampler:sampler;
@group(2) @binding(2) var<uniform> env:SkyEnvironment;
@vertex fn vertex(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32> {
 let p=array<vec2<f32>,3>(vec2<f32>(-1.,-1.),vec2<f32>(3.,-1.),vec2<f32>(-1.,3.));return vec4<f32>(p[i],0.,1.);
}
// World position of a pixel of the single-sample copy; w is 0 where there is no surface.
fn world_at(pixel:vec2<i32>)->vec4<f32> {
 let q=clamp(pixel,vec2<i32>(0),vec2<i32>(textureDimensions(scene_depth))-1);
 let d=textureLoad(scene_depth,q,0);
 let uv=(vec2<f32>(q)+0.5)*frame.viewport.zw;
 let clip=frame.inverse_vp*vec4<f32>(uv.x*2.-1.,1.-uv.y*2.,d,1.);
 return vec4<f32>(clip.xyz/clip.w,select(0.,1.,d>0.));
}
// The step to the neighbour on the side of a depth discontinuity that stays on this pixel's surface. dpdx/dpdy mix the
// foreground and background of a 2 x 2 quad at a silhouette, which gave the lit gain a wrong normal round every model.
// Both candidates point the way the pixel index grows, so the cross product keeps its handedness.
fn step_across(centre:vec3<f32>,forward:vec4<f32>,back:vec4<f32>)->vec3<f32> {
 let a=forward.xyz-centre;let b=centre-back.xyz;
 let la=select(1e9,length(a),forward.w>0.&&length(a)>1e-6);
 let lb=select(1e9,length(b),back.w>0.&&length(b)>1e-6);
 return select(b,a,la<=lb);
}
@fragment fn light(@builtin(position) p:vec4<f32>)->@location(0) vec4<f32> {
 let pixel=vec2<i32>(p.xy);let color=textureLoad(scene_color,pixel,0);let depth=textureLoad(scene_depth,pixel,0);
 let uv=p.xy*frame.viewport.zw;let clip=frame.inverse_vp*vec4<f32>(uv.x*2.-1.,1.-uv.y*2.,depth,1.);
 let world=clip.xyz/clip.w;
 let dx=step_across(world,world_at(pixel+vec2<i32>(1,0)),world_at(pixel-vec2<i32>(1,0)));
 let dy=step_across(world,world_at(pixel+vec2<i32>(0,1)),world_at(pixel-vec2<i32>(0,1)));
 let cross_n=cross(dx,dy);
 if depth<=0. || dot(cross_n,cross_n)<1e-12 {return color;}
 var normal=normalize(cross_n);if dot(normal,frame.eye.xyz-world)<0. {normal=-normal;}
 let incidence=max(dot(normal,env.light_direction),0.);
 let shade=select(1.,cloud_shadow(shadow_map,shadow_sampler,shadow,world),frame.options.y>0.5);
 var gain=vec3<f32>(mix(1.,shade,incidence*0.7));
 if frame.options.x>0.5 {
  // The input holds baked illumination. This opt-in estimate retains its
  // material detail and dark interiors; it cannot recover separate albedo/GI.
  let ambient=env_irradiance(env_map,env_sampler,env,normal)/3.14159265;
  let direct=env.light_color*env.light_intensity*incidence*shade;
  gain=clamp(ambient*0.5+direct*0.2,vec3<f32>(0.03),vec3<f32>(4.));
 }
 let footprint=max(length(dx),length(dy));let expected=max(distance(world,frame.eye.xyz)*4./min(frame.viewport.x,frame.viewport.y),0.01);
 gain=mix(vec3<f32>(1.),gain,1.-smoothstep(expected*4.,expected*12.,footprint));
 return vec4<f32>(color.rgb*gain*(1.+frame.options.z),color.a);
}
