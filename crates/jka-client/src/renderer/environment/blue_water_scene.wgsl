#import naval_clouds::shadow::{CloudShadow, cloud_shadow}
#import naval_sky::environment::{SkyEnvironment, env_irradiance}
struct Frame { inverse_vp:mat4x4<f32>,eye:vec4<f32>,viewport:vec4<f32>,options:vec4<f32>,moon_light:vec4<f32>,moon_mode:vec4<f32> }
@group(0) @binding(0) var<uniform> frame:Frame;
@group(0) @binding(1) var scene_depth:texture_depth_2d;
@group(0) @binding(2) var scene_color:texture_2d<f32>;
// Depth-prepass policy mask: R 0.25 / 0.75 = lightmapped world, 0.125 = model.
@group(0) @binding(3) var world_mask:texture_2d<f32>;
// Prepass radial depth (JKA units): grass, water and other main-pass-only
// surfaces in front of the marked world must still receive the live light.
@group(0) @binding(4) var world_mask_depth:texture_2d<f32>;
@group(1) @binding(0) var shadow_map:texture_2d<f32>;
@group(1) @binding(1) var shadow_sampler:sampler;
@group(1) @binding(2) var<uniform> shadow:CloudShadow;
@group(2) @binding(0) var env_map:texture_2d<f32>;
@group(2) @binding(1) var env_sampler:sampler;
@group(2) @binding(2) var<uniform> env:SkyEnvironment;
// Same uniform ABI and depth array as the JKA cascaded-shadow receiver.
struct CsmFrame {
 view_proj:array<mat4x4<f32>,4>,
 split_depths:vec4<f32>,
 light_direction_enabled:vec4<f32>,
 params:vec4<f32>,
 camera_forward:vec4<f32>,
 cascade_texel_sizes:vec4<f32>,
 bevy_params:vec4<f32>,
 dynamic_coverage:array<vec4<u32>,4>,
};
@group(3) @binding(0) var csm_depth:texture_depth_2d_array;
@group(3) @binding(1) var csm_sampler:sampler_comparison;
@group(3) @binding(2) var<uniform> csm:CsmFrame;

fn moon_csm_cascade(world:vec3<f32>, normal:vec3<f32>, index:i32)->f32 {
 let n=normalize(normal);
 let light=normalize(csm.light_direction_enabled.xyz);
 let offset=n*(csm.bevy_params.y*csm.cascade_texel_sizes[u32(index)])
           -light*csm.bevy_params.x;
 let clip=csm.view_proj[u32(index)]*vec4<f32>(world+offset,1.);
 if clip.w<=0. {return 1.;}
 let ndc=clip.xyz/clip.w;
 if any(abs(ndc.xy)>vec2<f32>(1.)) || ndc.z<=0. || ndc.z>=1. {return 1.;}
 let uv=ndc.xy*vec2<f32>(0.5,-0.5)+vec2<f32>(0.5);
 // One hardware-filtered depth compare; this is only sampled for moonlit pixels.
 let static_visibility=textureSampleCompareLevel(csm_depth,csm_sampler,uv,index,ndc.z);
 if csm.params.w<=1.5 {return static_visibility;}
 // Entity-caster CSM stores movers in slices 5..8. When static-world
 // casters are enabled, they remain in slices 0..3 as well.
 let moving_visibility=textureSampleCompareLevel(csm_depth,csm_sampler,uv,index+5,ndc.z);
 return min(static_visibility,moving_visibility);
}
fn moon_csm_visibility(world:vec3<f32>,normal:vec3<f32>)->f32 {
 if csm.light_direction_enabled.w<0.5 {return 1.;}
 let depth=max(dot(world-frame.eye.xyz,normalize(csm.camera_forward.xyz)),0.);
 var i=0i;
 if depth>csm.split_depths.x {i=1i;}
 if depth>csm.split_depths.y {i=2i;}
 if depth>csm.split_depths.z {i=3i;}
 if depth>csm.split_depths[u32(i)] {return 1.;}
 var visibility=moon_csm_cascade(world,normal,i);
 if i<3 {
  let end=csm.split_depths[u32(i)];
  let start=(1.-csm.bevy_params.z)*end;
  if depth>=start {
   let next=moon_csm_cascade(world,normal,i+1);
   visibility=mix(visibility,next,clamp((depth-start)/max(end-start,1e-5),0.,1.));
  }
 }
 return mix(1.,visibility,clamp(csm.params.z,0.,1.));
}
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
 // At night the direct key is the authored or phase-derived moon, not
 // Sky::light's selected solar source. The environment remains indirect fill.
 let moon_active=frame.moon_mode.x>0.5;
 let light_direction=select(env.light_direction,frame.moon_light.xyz,moon_active);
 let light_color=select(env.light_color,vec3<f32>(0.70,0.82,1.),moon_active);
 let light_intensity=select(env.light_intensity,frame.moon_light.w,moon_active);
 let incidence=max(dot(normal,light_direction),0.);
 let shade=select(1.,cloud_shadow(shadow_map,shadow_sampler,shadow,world),frame.options.y>0.5);
 // Fullscreen lighting always uses its own original screen-space policy.
 // The BSP lightmap bake does not suppress or supply its intensity.
 let mask_pixel=clamp(pixel,vec2<i32>(0),vec2<i32>(textureDimensions(world_mask))-1);
 let policy=textureLoad(world_mask,mask_pixel,0);
 let mask=policy.r;
 // Models lit themselves (md3/ghoul2 sky_world_model_gain). No depth match:
 // the view weapon's depth hack moves its depth-buffer position, not its mask.
 let lit_model=frame.moon_mode.w>0.5 && abs(mask-0.125)<0.06;
 // A model's prepass G carries smoothstep(0, 0.3, N.L) from its smooth normal
 // as code 26..126 (md3.wgsl cloud_shadow_facing_code); prefer it to the facet.
 let facing_code=policy.g*255.;
 let facing=select(incidence,clamp((facing_code-26.)/100.,0.,1.),abs(mask-0.125)<0.06 && facing_code>25.5 && facing_code<126.5);
 var gain=vec3<f32>(mix(1.,shade,facing*0.7));
 if frame.options.x>0. && !lit_model {
  // The input holds baked illumination. This opt-in estimate retains its
  // material detail and dark interiors; it cannot recover separate albedo/GI.
  let ambient=env_irradiance(env_map,env_sampler,env,normal)/3.14159265;
  // Realtime sky lighting uses the existing cascades for its sun/moon key.
  // Baked mode never runs this sky gain; its shadow filtering is unchanged.
  var moon_visibility=1.;
  if frame.options.w>0.5 && incidence>0. {
   moon_visibility=moon_csm_visibility(world,normal);
  }
  let direct=light_color*light_intensity*incidence*shade*moon_visibility;
  // options.x is the strength: 0 keeps the cloud-shadow-only gain, 1 replaces it.
  gain=mix(gain,clamp(ambient*0.5+direct*0.2,vec3<f32>(0.03),vec3<f32>(4.)),frame.options.x);
 }
 let footprint=max(length(dx),length(dy));let expected=max(distance(world,frame.eye.xyz)*4./min(frame.viewport.x,frame.viewport.y),0.01);
 gain=mix(vec3<f32>(1.),gain,1.-smoothstep(expected*4.,expected*12.,footprint));
 return vec4<f32>(color.rgb*gain*(1.+frame.options.z),color.a);
}
