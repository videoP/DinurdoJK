// Port: UE 5.8.1 SingleLayerWaterComposite.usf, legacy GBuffer path, above
// water, no separated main light, no reflection captures. Copyright Epic Games,
// Inc. All Rights Reserved. Rerasterizing the identical material replaces UE's
// GBuffer read. Active map/Blue Water sky replaces Unreal's skylight resource.
// SSR is SSRTReflections.usf quality 2 (one mirror ray, 16 steps) and
// SSRTRayCast.ush, with current scene color and fixed frame-0 jitter instead
// of previous-frame color/reprojection. No temporal history or denoiser.
@group(5) @binding(0) var river_sky_rt: texture_2d<f32>;
@group(5) @binding(1) var river_sky_bk: texture_2d<f32>;
@group(5) @binding(2) var river_sky_lf: texture_2d<f32>;
@group(5) @binding(3) var river_sky_ft: texture_2d<f32>;
@group(5) @binding(4) var river_sky_up: texture_2d<f32>;
@group(5) @binding(5) var river_sky_dn: texture_2d<f32>;
@group(5) @binding(6) var river_sky_sampler: sampler;

// JKA's existing sky-face orientation and mip adapter, in river Z-up axes.
fn river_sky_uv(x:f32,y:f32)->vec2<f32> {return clamp(vec2<f32>(0.5*x+0.5,0.5-0.5*y),vec2<f32>(0.001),vec2<f32>(0.999));}
fn river_map_radiance(d:vec3<f32>,roughness:f32)->vec3<f32> {
    let a=abs(d);
    let lod=roughness*f32(textureNumLevels(river_sky_rt)-1u);
    if a.x>=a.y&&a.x>=a.z {
        if d.x>=0.0 {return textureSampleLevel(river_sky_rt,river_sky_sampler,river_sky_uv(-d.y/a.x,d.z/a.x),lod).rgb;}
        return textureSampleLevel(river_sky_lf,river_sky_sampler,river_sky_uv(d.y/a.x,d.z/a.x),lod).rgb;
    }
    if a.y>=a.x&&a.y>=a.z {
        if d.y>=0.0 {return textureSampleLevel(river_sky_bk,river_sky_sampler,river_sky_uv(d.x/a.y,d.z/a.y),lod).rgb;}
        return textureSampleLevel(river_sky_ft,river_sky_sampler,river_sky_uv(-d.x/a.y,d.z/a.y),lod).rgb;
    }
    if d.z>=0.0 {return textureSampleLevel(river_sky_up,river_sky_sampler,river_sky_uv(-d.y/a.z,-d.x/a.z),lod).rgb;}
    return textureSampleLevel(river_sky_dn,river_sky_sampler,river_sky_uv(-d.y/a.z,d.x/a.z),lod).rgb;
}

// SingleLayerWaterComposite.usf: SSR over environment, EnvBRDF and shoreline
// fade, Src=One/Dst=SrcAlpha. Above water TransmittanceToScene is exactly one.
@fragment fn fs_composite(input:RiverOut,@builtin(front_facing) front:bool)->@location(0) vec4<f32> {
    if input.eye_water.z > 0.5 { discard; }
    let eye = river_view.eye.xyz;
    let pixel_depth = dot(input.world - eye, river_view.forward.xyz); // GetPixelDepth
    // The bilateral slope tolerance (ddx_fine/ddy_fine), taken before the clip.
    let slope = max(abs(dpdxFine(pixel_depth)), abs(dpdyFine(pixel_depth)));

    // OpacityMask: attribute default 1, Opacity 1, SingleRiverRendering; JKA
    // has no CustomDepth, so it reads as cleared (beyond every pixel).
    let fluid = river_live_field(sim_grid, input.uv);
    let masks = river_surface_masks(fluid.y, 1.0, input.uv, vec2<f32>(textureDimensions(sim_grid, 0)), 1.0, pixel_depth, 3.0e38);

    // Primary foam and the detail-normal graph, both before the clip: their
    // advected samples take fragment derivatives. Without the foam detail
    // resource foam is absent (not substituted): zero total and opacity.
    let world_xy = input.world.xy;
    let time = river_view.time.x;
    var foam = RiverFoam(vec3<f32>(0.0, 0.0, 1.0), 0.0, 0.0, 0.0, 0.0, vec3<f32>(0.0));
    if river_view.time.y > 0.5 {
        foam = river_live_grid_foam(sim_grid, normals_rt, normal_sampler, foam_rt, foam_rt_sampler, foam_detail, foam_detail_sampler,
            input.uv, world_xy, time, river_foam_advection(), river_foam_settings());
    }
    // FoamOpacity 1 (live instance); the normal gate takes the simulation mask.
    let graph_normal = river_live_grid_normal(sim_grid, normals_rt, normal_sampler, ocean_detail, ocean_sampler, input.uv, world_xy, time,
        river_motion(river_view.time.z), river_wpo_advection(), river_detail_advection(), river_wave_height(),
        foam.normal, foam.total, river_foam_settings().shading.x, masks.simulation_mask);
    if masks.material_mask < 0.3333 {
        discard;
    }
    return river_composite_pass(input.clip, input.world, front, graph_normal, foam, pixel_depth, slope);
}

// SingleLayerWaterComposite after the material graph, shared by the camera
// window (fs_composite) and the water-mesh quadtree tiles (fs_composite_far).
fn river_composite_pass(clip: vec4<f32>, world: vec3<f32>, front: bool, graph_normal: vec3<f32>, foam: RiverFoam,
                        pixel_depth: f32, slope: f32) -> vec4<f32> {
    let eye = river_view.eye.xyz;
    // Two-sided: WorldNormal flips on the back face.
    let two_sided_sign = select(-1.0, 1.0, front);
    let n = graph_normal * two_sided_sign;
    let v = normalize(eye - world);
    let n_dot_v = dot(n, v);
    let p = river_slw_parameters();
    let surface = river_slw_surface(p, n_dot_v, two_sided_sign, pixel_depth);
    // WaterAttributes, Water_Underside, then MF_ShallowWaterRiver's foam lerps.
    let attributes = river_foam_attributes(
        RiverFoamAttributes(surface.base_color, surface.roughness, surface.specular, surface.opacity), foam, river_foam_settings());
    // The foam scattering only enters through WaterInfo velocity, which is
    // zero for spline-less (simulation-first) rivers.



    let size=vec2<f32>(textureDimensions(scene_depth));
    let uv=clamp(clip.xy/size,0.55/size,(size-0.55)/size);
    let behind=river_gather_behind(uv,size,pixel_depth,slope);
    let delta_depth=behind.scene_depth-pixel_depth;
    if delta_depth<=0.0 {discard;}
    let roughness=attributes.roughness;
    let mirror=reflect(-v,n);
    let a=roughness*roughness;
    let off_peak=mix(n,mirror,(1.0-a)*(sqrt(1.0-a)+a));
    var environment=river_map_radiance(off_peak,roughness);
    if river_view.sun_illuminance.w>0.5 {
        environment=env_radiance(env_map,env_sampler,env,river_slw_render_vector(off_peak),roughness);
    }
    let ssr=river_ssr(world,mirror,pixel_depth,roughness,clip.xy);
    let reflected=ssr.rgb+environment*(1.0-ssr.a);
    let gf=textureSampleLevel(preintegrated_gf,gf_sampler,vec2<f32>(max(0.0,n_dot_v),roughness),0.0).rg;
    let brdf=river_slw_env_brdf(vec3<f32>(0.08*attributes.specular),gf);
    return vec4<f32>(reflected*brdf*clamp(delta_depth*0.02,0.0,1.0),1.0);
}
