// Cached Grid2D_SW_River stages 4 (Init Foam) and 16 (Foam), FoamGrid dispatch.
// Copyright Epic Games, Inc. All Rights Reserved. See foam_field.rs.
// FoamGrid is the native half grid; its R attribute lives in RGBA16f here.
// The stage-16 output is also FoamRT (same cells, same half value).
struct RiverFoamField {
    unit_to_world:mat4x4<f32>, // columns hold the native row-vector matrix rows
    seed:vec4<f32>,            // curl OffsetFromSeed xyz, noise strength
    noise:vec4<f32>,           // frequency, time scale, advection multiplier, dissipation rate
    grid:vec4<f32>,            // FoamDx, FoamGrid UnitToUV xy, unused
    sim:vec4<f32>,             // User.SimRes xy, Emitter.dx, unused
    frame:vec4<f32>,           // Engine.DeltaTime, Engine.Time, unused, unused
    emission:vec4<f32>,        // authored depth start/full (cm); defaults 150/500
}
@group(0) @binding(0) var<uniform> foam:RiverFoamField;
@group(0) @binding(1) var water:texture_2d_array<f32>; // height, depth, vx, vy (R32f layers)
@group(0) @binding(2) var normal_grid:texture_2d<f32>; // half NormalGrid xyz
@group(0) @binding(3) var foam_previous:texture_2d<f32>;
@group(0) @binding(4) var foam_output:texture_storage_2d<rgba16float,write>;
@group(0) @binding(5) var grid_sampler:sampler; // Niagara SF_Bilinear, AM_Clamp

@compute @workgroup_size(8,8)
fn init_foam(@builtin(global_invocation_id) id:vec3<u32>) {
    if any(id.xy>=textureDimensions(foam_output)) {return;}
    textureStore(foam_output,id.xy,vec4<f32>(0.0));
}

fn river_foam_velocity(unit:vec2<f32>)->vec2<f32> {
    return vec2<f32>(textureSampleLevel(water,grid_sampler,unit,2,0.0).x,textureSampleLevel(water,grid_sampler,unit,3,0.0).x);
}

// Grid2D_Secondary_EmissionPoints001 with SampleFromGrid. The velocity
// 800..1000 and depth-difference criteria have constant zero multipliers;
// under D3D semantics both are exactly zero inside a max of non-negative
// criteria, so their samples are omitted. foam_field.rs keeps the full graph.
fn river_foam_emit(unit:vec2<f32>)->f32 {
    let udx=vec2<f32>(1.0/foam.sim.x,1.0/foam.sim.y);
    let valid=2.0*udx.x;
    if !(unit.x>valid&&unit.x<1.0-valid&&unit.y>valid&&unit.y<1.0-valid) {return 0.0;}
    let depth=textureSampleLevel(water,grid_sampler,unit,1,0.0).x;
    if !(depth>1e-5) {return 0.0;}
    let vx_1=river_foam_velocity(unit+vec2<f32>(0.0,1.0)*udx).x;
    let vx_m1=river_foam_velocity(unit+vec2<f32>(0.0,-1.0)*udx).x;
    let vy_1=river_foam_velocity(unit+vec2<f32>(1.0,0.0)*udx).y;
    let vy_m1=river_foam_velocity(unit+vec2<f32>(-1.0,0.0)*udx).y;
    let curl=abs(((vx_1-vx_m1)-(vy_1-vy_m1))/(2.0*foam.sim.z));
    let velocity=vec2<f32>(0.5*(vx_1+vx_m1),0.5*(vy_1+vy_m1));
    let n=textureSampleLevel(normal_grid,grid_sampler,unit,0.0).xyz;
    let right=textureSampleLevel(normal_grid,grid_sampler,unit+vec2<f32>(1.0,0.0)*udx,0.0).xyz;
    let up=textureSampleLevel(normal_grid,grid_sampler,unit+vec2<f32>(0.0,1.0)*udx,0.0).xyz;
    let curvature=abs(((1.0-dot(n,right))+(1.0-dot(n,up)))*0.5);
    let speed=length(velocity);
    let strongest=max(river_foam_criterion(curl,1.0,3.0,1.0),river_foam_criterion(curvature,0.002,0.05,5.0));
    // Zero minimum explicitly disables this gate; equal-edge smoothstep is
    // undefined in WGSL. Positive wet depth was already checked above.
    var depth_gate=1.0;
    if foam.emission.y>foam.emission.x {
        depth_gate=river_foam_criterion(depth,foam.emission.x,foam.emission.y,1.0);
    }
    let amount=1.0*strongest*depth_gate*river_foam_criterion(speed,50.0,200.0,1.0);
    return clamp(amount,0.0,1.0);
}

@compute @workgroup_size(8,8)
fn foam_stage(@builtin(global_invocation_id) id:vec3<u32>) {
    if any(id.xy>=textureDimensions(foam_output)) {return;}
    let unit=(vec2<f32>(id.xy)+0.5)*foam.grid.yz;
    // Grid2D_Foam_Advect: UnitToWorld world position, curl noise, advection.
    let world=(foam.unit_to_world*vec4<f32>(unit,0.0,1.0)).xyz;
    let xyz=world*foam.noise.x+foam.noise.y*foam.frame.y;
    let noise=(river_foam_curl(xyz,foam.seed.xyz)*foam.seed.w).xy;
    let velocity=river_foam_velocity(unit)+noise;
    let scale=foam.noise.z*foam.frame.x/foam.grid.x;
    let sample_unit=(vec2<f32>(id.xy)-scale*velocity+0.5)*foam.grid.yz;
    let advected=textureSampleLevel(foam_previous,grid_sampler,sample_unit,0.0).x;
    let total=advected+river_foam_emit(unit);
    // DissipateFloat: rate User.FoamDissipationRate, subtraction amount zero.
    let value=clamp(total/(foam.frame.x*foam.noise.w+1.0)-0.0*foam.frame.x,0.0,1e9);
    textureStore(foam_output,id.xy,vec4<f32>(value,0.0,0.0,0.0));
}
