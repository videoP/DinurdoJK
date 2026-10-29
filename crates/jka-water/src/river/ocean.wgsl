// Grid2D_OceanPatch stages 4/5/6, selected 512x512 / 256 threads.
// Copyright Epic Games, Inc. All Rights Reserved. See ocean-reference.json.
const OCEAN_N:u32=512u;const OCEAN_HALF:u32=256u;
const OCEAN_PI:f32=3.14159265358979323846;
var<workgroup> fft_shared:array<vec2<f32>,512>;
struct OceanControls { physical:vec4<f32>, wind:vec4<f32>, frame:vec4<f32> }
// physical: gravity, PhillipsAmplitude, TimeScale, OceanLengthScale
// wind: windSpeed, windAngle, alignment (integer value), Slosh
// frame: age, unused, unused, unused
@group(0) @binding(0) var<uniform> ocean:OceanControls;
@group(0) @binding(1) var noise_tex:texture_2d<f32>;
@group(0) @binding(2) var noise_sampler:sampler;
@group(0) @binding(3) var fft_input:texture_2d<f32>;
@group(0) @binding(4) var fft_output:texture_storage_2d<rgba16float,write>;
@group(0) @binding(5) var detail_output:texture_storage_2d<rgba16float,write>;
fn ocean_fold(v:i32)->i32 {if v>256 {return 512-v;}return v;}
fn ocean_noise_uv(scanline:u32,thread:u32)->array<vec2<f32>,4> {
    let a=vec2<f32>(f32(ocean_fold(i32(scanline))),f32(ocean_fold(i32(thread))))/256.0;
    let b=vec2<f32>(f32(ocean_fold(i32(scanline)+256))/256.0,a.y);
    return array<vec2<f32>,4>(a,b,a+0.25,b+0.25);
}
fn ocean_spectrum(scanline:u32,thread:u32,time:f32,p:OceanControls,noise:array<vec4<f32>,4>)->array<vec2<f32>,2> {
    let two_pi=2.0*OCEAN_PI;let factor=two_pi/p.physical.w;let speed=sqrt(factor*p.physical.x);let scaled_time=p.physical.z*time;
    var angle=p.wind.y%360.0;if angle<0.0 {angle+=360.0;}
    let radians= -(OCEAN_PI/180.0)*angle;let wind=vec2<f32>(cos(radians),sin(radians));
    var principal=1.0;if abs(p.wind.x)>0.001 {principal=p.wind.x*p.wind.x*two_pi/p.physical.x;}
    let alignment=clamp(i32(p.wind.z),1,20);
    let forward=fract(20.0*vec2<f32>(length(noise[0]),length(noise[1])));
    let backward=fract(20.0*vec2<f32>(length(noise[2]),length(noise[3])));
    var sign=1.0;if p.wind.y>=180.0 {sign*= -1.0;}if p.wind.x<0.0 {sign*= -1.0;}
    var mask=1.0;var k2=i32(scanline);
    if k2==256 {mask=0.0;}else if k2>256 {k2= -(512-k2);sign*= -1.0;}
    var v:array<vec2<f32>,2>;
    for(var r=0u;r<2u;r++) {
        var k1=i32(thread+r*OCEAN_HALF);
        if k1==256 {mask=0.0;}
        if k1>256 {k1= -(512-k1);if k2==0 {sign*= -1.0;}}
        let k=vec2<f32>(f32(k1)*factor,f32(k2)*factor);let magnitude=length(k);var amplitude=0.0;
        if !(k1==0 && k2==0) {
            let kl=magnitude*principal;let direction=k/magnitude;let wind_cos=dot(direction,wind);
            let power=pow(wind_cos*wind_cos,f32(alignment));let ksqr=f32(k1*k1+k2*k2);let k4=ksqr*ksqr;
            amplitude=sqrt(abs(p.physical.y)*power/(exp(1.0/(kl*kl))*k4));
        }
        let dispersion=sign*speed*sqrt(magnitude)*scaled_time+two_pi*forward[r];
        let dispersion2=sign*speed*sqrt(magnitude)*scaled_time+two_pi*backward[r];
        let a=vec2<f32>(cos(dispersion),sin(dispersion));var b=vec2<f32>(cos(dispersion2),sin(dispersion2));b.y*= -1.0;
        let other=amplitude*p.wind.w;v[r]=mask*(other*b+amplitude*a);
    }
    return v;
}
fn ocean_twiddle(v:vec2<f32>,theta:f32)->vec2<f32> {
    let c=cos(theta);let s=sin(theta);return vec2<f32>(v.x*c-v.y*s,v.x*s+v.y*c);
}
fn ocean_fft(thread:u32,initial:array<vec2<f32>,2>)->array<vec2<f32>,2> {
    var v=initial;
    for(var ns=1u;ns<OCEAN_HALF;ns*=2u) {
        let d=(thread/ns)*ns*2u+(thread%ns);
        let theta=2.0*OCEAN_PI*f32(thread%ns)/f32(ns*2u);
        v[1]=ocean_twiddle(v[1],theta);let a=v[0];v[0]=a+v[1];v[1]=a-v[1];
        workgroupBarrier();fft_shared[d]=v[0];fft_shared[d+ns]=v[1];workgroupBarrier();
        v[0]=fft_shared[thread];v[1]=fft_shared[thread+OCEAN_HALF];
    }
    let theta=2.0*OCEAN_PI*f32(thread%OCEAN_HALF)/f32(OCEAN_N);
    v[1]=ocean_twiddle(v[1],theta);let a=v[0];v[0]=a+v[1];v[1]=a-v[1];return v;
}
@compute @workgroup_size(256)
fn ocean_x(@builtin(workgroup_id) group:vec3<u32>,@builtin(local_invocation_index) t:u32) {
    let uv=ocean_noise_uv(group.x,t);var noise:array<vec4<f32>,4>;
    for(var i=0u;i<4u;i++){noise[i]=textureSampleLevel(noise_tex,noise_sampler,uv[i],0.0);}
    let v=ocean_fft(t,ocean_spectrum(group.x,t,ocean.frame.x,ocean,noise));
    textureStore(fft_output,vec2<u32>(t,group.x),vec4<f32>(v[0],0.0,0.0));
    textureStore(fft_output,vec2<u32>(t+OCEAN_HALF,group.x),vec4<f32>(v[1],0.0,0.0));
}
@compute @workgroup_size(256)
fn ocean_y(@builtin(workgroup_id) group:vec3<u32>,@builtin(local_invocation_index) t:u32) {
    let initial=array<vec2<f32>,2>(textureLoad(fft_input,vec2<u32>(group.x,t),0).xy,textureLoad(fft_input,vec2<u32>(group.x,t+OCEAN_HALF),0).xy);
    let v=ocean_fft(t,initial);
    textureStore(fft_output,vec2<u32>(group.x,t),vec4<f32>(v[0],0.0,0.0));
    textureStore(fft_output,vec2<u32>(group.x,t+OCEAN_HALF),vec4<f32>(v[1],0.0,0.0));
}
@compute @workgroup_size(8,8)
fn ocean_reset(@builtin(global_invocation_id) id:vec3<u32>) {
    if any(id.xy>=vec2<u32>(OCEAN_N)){return;}
    textureStore(fft_output,id.xy,vec4<f32>(0.0));
}
@compute @workgroup_size(8,8)
fn ocean_normals(@builtin(global_invocation_id) id:vec3<u32>) {
    if any(id.xy>=vec2<u32>(OCEAN_N)){return;}
    let height=textureLoad(fft_input,id.xy,0).x;
    let right=textureLoad(fft_input,vec2<u32>((id.x+1u)%OCEAN_N,id.y),0).x;
    let up=textureLoad(fft_input,vec2<u32>(id.x,(id.y+1u)%OCEAN_N),0).x;
    let dx=150.0/f32(OCEAN_N);let world=vec3<f32>(0.0,0.0,height);
    let a=vec3<f32>(dx,0.0,right)-world;let b=vec3<f32>(0.0,dx,up)-world;
    let cross_product=cross(a,b);let magnitude=length(cross_product);var normal=vec3<f32>(0.0,0.0,1.0);
    if !(magnitude<1e-10){normal=cross_product/magnitude;}
    textureStore(detail_output,id.xy,vec4<f32>(normal,height));
}
