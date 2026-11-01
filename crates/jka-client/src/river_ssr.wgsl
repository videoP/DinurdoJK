// UE SSRTRayCast.ush / SSRTReflections.usf quality-2 adapter. Copyright Epic Games, Inc. All Rights Reserved.
fn river_project_cm(p:vec3<f32>)->vec4<f32> {
    let q=p/RIVER_CM_PER_UNIT;
    return camera.view_proj*vec4<f32>(q.x,q.z,-q.y,1.0);
}
// SSRTRayCast.ush GetStepScreenFactorToClipAtScreenEdge.
fn river_clip_ray(start:vec2<f32>,step:vec2<f32>)->f32 {
    let inv_factor=0.5*length(step);
    let s=1.0-max(abs(step+start*inv_factor)-inv_factor,vec2<f32>(0.0))/max(abs(step),vec2<f32>(1e-8));
    return min(s.x,s.y)/max(inv_factor,1e-8);
}
// Native SSR hit test, mip progression, line-segment refinement and vignette.
fn river_ssr(origin:vec3<f32>,direction:vec3<f32>,scene_depth_cm:f32,roughness:f32,pixel:vec2<f32>)->vec4<f32> {
    let fade=clamp(roughness*(-2.0/0.8)+2.0,0.0,1.0);
    if fade<=0.0 {return vec4<f32>(0.0);}
    let view_z=dot(direction,river_view.forward.xyz);
    var distance_cm=min(scene_depth_cm,1000000.0);
    if view_z<0.0 {distance_cm=min(-0.95*scene_depth_cm/view_z,distance_cm);}
    let start_clip=river_project_cm(origin);
    let end_clip=river_project_cm(origin+direction*distance_cm);
    let start=start_clip.xyz/start_clip.w;
    let end=end_clip.xyz/end_clip.w;
    var ray=end-start;
    if length(ray.xy)<1e-8 {return vec4<f32>(0.0);}
    ray*=river_clip_ray(start.xy,ray.xy);
    let depth_clip=river_project_cm(origin+river_view.forward.xyz*distance_cm);
    let tolerance=max(abs(ray.z),(start.z-depth_clip.z/depth_clip.w)*4.0)/16.0;
    let step=vec3<f32>(ray.xy*vec2<f32>(0.5,-0.5),ray.z)/16.0;
    let jitter=fract(52.9829189*fract(dot(pixel,vec2<f32>(0.06711056,0.00583715))))-0.5;
    let uvz=vec3<f32>(start.xy*vec2<f32>(0.5,-0.5)+0.5,start.z)+step*jitter;
    let hzb_factor=vec2<f32>(textureDimensions(scene_depth))/(vec2<f32>(textureDimensions(reflection_hzb))*2.0);
    var last_diff=0.0;
    for(var i=0u;i<16u;i++) {
        let sample=uvz+step*f32(i+1u);
        let level=1.0+f32(i/2u)*(8.0/16.0)*roughness;
        let depth=textureSampleLevel(reflection_hzb,scene_sampler,sample.xy*hzb_factor,level).r;
        let diff=sample.z-depth;
        if abs(diff+tolerance)<tolerance&&depth!=0.0 {
            let t=clamp(last_diff/(last_diff-diff),0.0,1.0);
            let hit=uvz+step*(f32(i)+t);
            let screen=hit.xy*2.0-1.0;
            let edge=clamp(abs(screen)*5.0-4.0,vec2<f32>(0.0),vec2<f32>(1.0));
            let weight=clamp(1.0-dot(edge,edge),0.0,1.0)*fade;
            let color=textureSampleLevel(scene_color,gf_sampler,hit.xy,0.0).rgb;
            return vec4<f32>(color*weight,weight);
        }
        last_diff=diff;
    }
    return vec4<f32>(0.0);
}

