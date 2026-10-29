// Native live-grid material-function boundaries. Full water shading and the
// upstream MF_TextureAdvection wave height are separate ports. No fallback.
// Copyright Epic Games, Inc. All Rights Reserved. presentation-reference.json.
fn river_zero_load(tex:texture_2d<f32>,cell:vec2<i32>) -> vec4<f32> {
    if any(cell<vec2<i32>(0)) || any(cell>=vec2<i32>(textureDimensions(tex,0))) { return vec4<f32>(0.0); }
    return textureLoad(tex,cell,0);
}
fn river_live_field(tex:texture_2d<f32>,uv:vec2<f32>) -> vec4<f32> {
    let position=uv*vec2<f32>(textureDimensions(tex,0))-vec2<f32>(0.5);
    let cell=vec2<i32>(floor(position)); let fraction=position-vec2<f32>(cell);
    let a=river_zero_load(tex,cell); let b=river_zero_load(tex,cell+vec2<i32>(1,0));
    let c=river_zero_load(tex,cell+vec2<i32>(0,1)); let d=river_zero_load(tex,cell+vec2<i32>(1,1));
    return mix(mix(a,b,fraction.x),mix(c,d,fraction.x),fraction.y);
}
struct RiverMotion {
    // effective material LowVelocityWaveScale, WaveAttenMin/Max, WaveAmount
    attenuation:vec4<f32>,
    // PreviousFrameVelocityMult, WaveSpeed, Engine.DeltaTime, unused
    velocity:vec4<f32>,
}
// Evaluate separately with the current/previous phase's upstream inputs. Time
// in the native advection graph changes to PrevFrameGameTime in previous WPO.
fn river_grid_displacement(fluid:vec4<f32>,normal:vec3<f32>,advected_height:f32,incoming:vec3<f32>,local_z_to_world:vec3<f32>,motion:RiverMotion,previous_frame:bool) -> vec3<f32> {
    let current=local_z_to_world*(advected_height+fluid.x);
    if !previous_frame { return incoming+current; }
    let magnitude=length(fluid.zw);
    let attenuation=mix(1.0,smoothstep(motion.attenuation.y,motion.attenuation.z,magnitude),1.0-clamp(motion.attenuation.x,0.0,1.0))*motion.attenuation.w;
    let velocity=(((vec3<f32>(fluid.zw,0.0)*motion.velocity.z)*motion.velocity.x)*clamp(attenuation,0.0,1.0))*motion.velocity.y;
    let projected=velocity-dot(velocity,normal)*normal;
    return incoming+(current-projected);
}
