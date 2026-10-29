// MF_TextureAdvection Custom 5 and WaterAdvanced.ush selected three-phase path.
// Copyright Epic Games, Inc. All Rights Reserved. wave-advection-reference.json.
const RIVER_PI:f32=3.1415926535897932;
const RIVER_TWO_PI:f32=2.0*RIVER_PI;
struct RiverAdvection { brackets:vec4<f32>, wave:vec4<f32> }
// brackets: angle degrees, magnitude delta, pulse length, texture scale
// wave: speed, vertex mip bias, unused, unused
struct RiverWavePlan {
    velocities:array<vec2<f32>,4>, angle_blend:f32,magnitude_blend:f32,
    times:vec3<f32>,weights:vec3<f32>,cycles:vec3<f32>,offsets:vec3<f32>,
}
fn river_pseudo_random(cycle:f32)->f32 {
    let pos=fract(vec2<f32>(cycle,0.0)/128.0)*128.0+vec2<f32>(-64.340622,-72.465622);
    return fract(dot(pos.xyx*pos.xyy,vec3<f32>(20.390625,60.703125,2.4281209)));
}
fn river_wave_plan(velocity:vec2<f32>,normal:vec3<f32>,time:f32,s:RiverAdvection)->RiverWavePlan {
    let scaled=vec3<f32>(velocity*s.wave.x,0.0);
    let projected=(scaled-dot(scaled,normal)*normal).xy;
    var d0:vec2<f32>;var d1:vec2<f32>;var angle_alpha=0.0;
    if s.brackets.x>1e-5 {
        let delta=s.brackets.x*RIVER_PI/180.0;
        var angle=atan2(projected.y,projected.x);
        if angle<0.0 {angle+=2.0*RIVER_PI;}
        let fractional=angle%delta;let angle0=angle-fractional;
        d0=vec2<f32>(cos(angle0),sin(angle0));let c=cos(delta);let sn=sin(delta);
        d1=vec2<f32>(d0.x*c-d0.y*sn,d0.x*sn+d0.y*c);
        angle_alpha=fractional/delta;
    } else {d0=normalize(projected);d1=d0;}
    let magnitude=length(projected);var m0=magnitude;var m1=magnitude;var magnitude_alpha=0.0;
    if s.brackets.y>1e-5 {
        let fractional=magnitude%s.brackets.y;m0=magnitude-fractional;m1=m0+s.brackets.y;magnitude_alpha=fractional/s.brackets.y;
    }
    var p:RiverWavePlan;p.velocities=array<vec2<f32>,4>(d0*m0,d0*m1,d1*m0,d1*m1);
    p.angle_blend=angle_alpha;p.magnitude_blend=magnitude_alpha;
    let phase=time+s.brackets.z*vec3<f32>(0.0,1.0,2.0)/3.0;
    p.times=phase%vec3<f32>(s.brackets.z);p.cycles=floor(phase/s.brackets.z);
    let timescale=RIVER_TWO_PI/s.brackets.z;
    p.weights=(vec3<f32>(0.5)-0.5*cos(timescale*time+vec3<f32>(0.0,1.0,2.0)*RIVER_TWO_PI/3.0))/1.5;
    p.offsets=vec3<f32>(river_pseudo_random(p.cycles.x),river_pseudo_random(p.cycles.y),river_pseudo_random(p.cycles.z))*1024.0;
    return p;
}
fn river_wave_uv(p:RiverWavePlan,position:vec2<f32>,scale:f32,bracket:u32,phase:u32)->vec2<f32> {
    return ((position-p.velocities[bracket]*p.times[phase])+p.offsets[phase])*scale;
}
fn river_wave_sample(tex:texture_2d<f32>,tex_sampler:sampler,p:RiverWavePlan,position:vec2<f32>,scale:f32,mips:vec3<f32>)->vec4<f32> {
    var values:array<vec4<f32>,4>;
    for (var i=0u;i<4u;i++) {
        let a=textureSampleLevel(tex,tex_sampler,river_wave_uv(p,position,scale,i,0u),mips.x);
        let b=textureSampleLevel(tex,tex_sampler,river_wave_uv(p,position,scale,i,1u),mips.y);
        let c=textureSampleLevel(tex,tex_sampler,river_wave_uv(p,position,scale,i,2u),mips.z);
        values[i]=a*p.weights.x+b*p.weights.y+c*p.weights.z;
    }
    return mix(mix(values[0],values[1],p.magnitude_blend),mix(values[2],values[3],p.magnitude_blend),p.angle_blend);
}
fn river_wave_vertex(tex:texture_2d<f32>,tex_sampler:sampler,position:vec2<f32>,velocity:vec2<f32>,normal:vec3<f32>,time:f32,s:RiverAdvection)->vec4<f32> {
    let p=river_wave_plan(velocity,normal,time,s);
    return river_wave_sample(tex,tex_sampler,p,position,s.brackets.w,vec3<f32>(s.wave.y));
}
fn river_wave_mip(dx:vec2<f32>,dy:vec2<f32>,dimensions:vec2<f32>)->f32 {
    let a=dx*dimensions;let b=dy*dimensions;
    return 0.5*log2(max(max(dot(a,a),dot(b,b)),1e-8));
}
// Fragment derivatives use the original velocity, deliberately without Speed
// or projection. Native vertex samples above use the supplied mip bias instead.
fn river_wave_fragment(tex:texture_2d<f32>,tex_sampler:sampler,position:vec2<f32>,velocity:vec2<f32>,normal:vec3<f32>,time:f32,s:RiverAdvection)->vec4<f32> {
    let p=river_wave_plan(velocity,normal,time,s);
    let uv0=(position-velocity*p.times.x)*s.brackets.w;
    let uv1=(position-velocity*p.times.y)*s.brackets.w;
    let uv2=(position-velocity*p.times.z)*s.brackets.w;
    let dims=vec2<f32>(textureDimensions(tex,0));let levels=f32(textureNumLevels(tex));
    let mips=clamp(vec3<f32>(river_wave_mip(dpdx(uv0),dpdy(uv0),dims),river_wave_mip(dpdx(uv1),dpdy(uv1),dims),river_wave_mip(dpdx(uv2),dpdy(uv2),dims))+s.wave.y,vec3<f32>(0.0),vec3<f32>(levels));
    return river_wave_sample(tex,tex_sampler,p,position,s.brackets.w,mips);
}
struct RiverWaveHeight { attenuation:vec4<f32>, camera:vec4<f32> }
// attenuation: low velocity scale, min/max speed, power
// camera: height multiplier, camera falloff, unused, unused
fn river_advected_height(alpha:f32,magnitude:f32,distance:f32,s:RiverWaveHeight)->f32 {
    let a=mix(1.0,smoothstep(s.attenuation.y,s.attenuation.z,magnitude),1.0-clamp(s.attenuation.x,0.0,1.0));
    var power=0.0;if a>2.980233e-8 {power=pow(a,s.attenuation.w);}
    return (alpha*(power*s.camera.x))*smoothstep(s.camera.y,s.camera.y+50.0,distance);
}
