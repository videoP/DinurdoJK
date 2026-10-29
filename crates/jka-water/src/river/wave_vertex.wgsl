// Selected live-grid WPO graph, with native wave advection connected.
// Requires material.wgsl and wave_advection.wgsl. No optical water substitute.
// Copyright Epic Games, Inc. All Rights Reserved.
fn river_live_grid_wpo(fluid_tex:texture_2d<f32>,normal_tex:texture_2d<f32>,normal_sampler:sampler,detail_tex:texture_2d<f32>,detail_sampler:sampler,uv:vec2<f32>,world_xy:vec2<f32>,camera_distance:f32,time:f32,incoming:vec3<f32>,local_z_to_world:vec3<f32>,motion:RiverMotion,advection:RiverAdvection,height:RiverWaveHeight,previous_frame:bool)->vec3<f32> {
    let fluid=river_live_field(fluid_tex,uv);
    let normal=textureSampleLevel(normal_tex,normal_sampler,uv,0.0).xyz;
    let detail=river_wave_vertex(detail_tex,detail_sampler,world_xy,fluid.zw,normal,time,advection);
    let wave_height=river_advected_height(detail.a,length(fluid.zw),camera_distance,height);
    return river_grid_displacement(fluid,normal,wave_height,incoming,local_z_to_world,motion,previous_frame);
}
