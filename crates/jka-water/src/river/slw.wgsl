// Single Layer Water for the selected live river: resolved coefficient/surface
// graph and the engine's FULL-quality water volume lighting. Copyright Epic
// Games, Inc. All Rights Reserved. See slw.rs; scene inputs are explicit.
struct RiverSlwParameters {
    scattering:vec4<f32>,
    absorption:vec4<f32>,
    albedo:vec4<f32>,      // Water Albedo rgb, CausticsMaxIntensity
    shading:vec4<f32>,     // specular, fresnel specular, roughness, fresnel roughness
    fresnel:vec4<f32>,     // fresnel exponent, anisotropy, velocity to mask waves, unused
    refraction:vec4<f32>,  // refraction, far, depth for min, bottom amount
    critical:vec4<f32>,    // angle dot, width, bend width, lip refraction
    depth_bias:vec4<f32>,  // RefractionDepthBias, unused...
}
struct RiverSlwCoefficients {scattering:vec3<f32>,absorption:vec3<f32>,phase_g:f32,color_scale:vec3<f32>}
fn river_slw_coefficients(p:RiverSlwParameters,river_scattering:vec3<f32>,water_info_velocity:vec2<f32>,two_sided_sign:f32)->RiverSlwCoefficients {
    let blend=saturate(length(water_info_velocity)/p.fresnel.z);
    let front=saturate(two_sided_sign);
    let base=0.0+p.scattering.rgb*p.scattering.a;
    let river=0.0+blend*(river_scattering-0.0);
    let scattering=1.0*((base+river)/1000.0*front);
    let absorption=1.0*((1.0/p.absorption.rgb)/p.absorption.a*front);
    return RiverSlwCoefficients(scattering,absorption,p.fresnel.y,vec3<f32>(1.0));
}
fn river_slw_fresnel(n_dot_v:f32,exponent:f32)->f32 {
    let base=max(abs(1.0-max(n_dot_v,0.0)),1.0e-4);
    return pow(max(base,0.0),exponent)*(1.0-0.0)+0.0;
}
struct RiverSlwSurface {base_color:vec3<f32>,metallic:f32,specular:f32,roughness:f32,opacity:f32,refraction:vec2<f32>}
fn river_slw_surface(p:RiverSlwParameters,n_dot_v:f32,two_sided_sign:f32,pixel_depth:f32)->RiverSlwSurface {
    let f=river_slw_fresnel(n_dot_v,p.fresnel.x);
    let specular=mix(p.shading.x,p.shading.y,f);
    let roughness=mix(p.shading.z,p.shading.w,f);
    let front=saturate(two_sided_sign);
    let critical=p.critical.x-n_dot_v;
    let tir=mix(saturate(critical/p.critical.y),0.0,front);
    let refraction=mix(p.refraction.x,p.refraction.y,saturate(pixel_depth/p.refraction.z));
    let bottom=mix(1.0,refraction,p.refraction.w);
    let lip=mix(bottom,p.critical.w,1.0-saturate(critical/p.critical.z));
    return RiverSlwSurface(p.albedo.rgb/p.albedo.w,0.0,mix(specular,1.0,tir),roughness,mix(0.0,1.0,tir),
        vec2<f32>(mix(lip,refraction,front),p.depth_bias.x));
}
fn river_slw_ior(specular:f32)->f32 {
    let f0=0.08*specular;
    return 2.0/(1.0-sqrt(min(f0,0.99)))-1.0;
}
// WaterRefract; w=0 on total internal reflection.
fn river_slw_refract(v:vec3<f32>,n:vec3<f32>,eta:f32)->vec4<f32> {
    let cos_i=dot(v,n);
    let cos_t2=1.0-eta*eta*(1.0-cos_i*cos_i);
    if cos_t2<0.0 {return vec4<f32>(reflect(-v,n),0.0);}
    let cos_t=-select(-1.0,1.0,cos_i>=0.0)*sqrt(cos_t2);
    return vec4<f32>((eta*cos_i+cos_t)*n-eta*v,1.0);
}
fn river_slw_isotropic_phase()->f32 {return 1.0/(4.0*3.1415926535897932);}
fn river_slw_schlick_phase(g:f32,cos_theta:f32)->f32 {
    let k=1.55*g-0.55*g*g*g;
    let factor=1.0+k*cos_theta;
    return (1.0-k*k)/(4.0*3.1415926535897932*factor*factor);
}
fn river_slw_env_brdf(specular_color:vec3<f32>,gf:vec2<f32>)->vec3<f32> {
    return specular_color*gf.x+saturate(50.0*specular_color.g)*gf.y;
}
struct RiverSlwBilateral {scene_depth:f32,weights:vec4<f32>,refraction_mask:vec4<f32>}
// After the gathers (Gather order x,y,z,w = (0,1),(1,1),(1,0),(0,0)).
fn river_slw_bilateral(uv_in:vec2<f32>,size:vec2<f32>,depth4:vec4<f32>,mask4:vec4<f32>,water_depth:f32,slope:f32)->RiverSlwBilateral {
    let uv=uv_in+(0.5/256.0)*(1.0/size);
    let fr=fract(uv*size-0.5);
    let bilinear=vec4<f32>((1.0-fr.x)*fr.y,fr.x*fr.y,fr.x*(1.0-fr.y),(1.0-fr.x)*(1.0-fr.y));
    let is_water=mask4>=vec4<f32>(128.0/255.0);
    let bilateral=select(vec4<f32>(0.0),vec4<f32>(1.0),(depth4+slope)>=vec4<f32>(water_depth));
    var weights=bilinear*(bilateral*select(vec4<f32>(0.0),vec4<f32>(1.0),is_water));
    let sum=weights.x+weights.y+weights.z+weights.w;
    weights*=select(0.0,1.0/sum,sum>1e-5);
    let mask=select(vec4<f32>(0.0),(mask4-128.0/255.0)/(127.0/255.0),is_water);
    return RiverSlwBilateral(dot(depth4,weights),weights,mask);
}
fn river_slw_uv_distortion(view_normal:vec3<f32>,view_vertex_normal:vec3<f32>,refraction:vec2<f32>,view_to_buffer:vec2<f32>,
    distortion:vec4<f32>,behind_depth:f32,surface_depth:f32)->vec2<f32> {
    var uv=(view_vertex_normal.xy-view_normal.xy)*(refraction.x-1.0)*view_to_buffer;
    uv*=distortion.zw*vec2<f32>(0.00023,-0.00023)*vec2<f32>(distortion.x,distortion.y*distortion.x);
    let bias=-refraction.y;
    let range=clamp(abs(bias*0.5),0.0,50.0);
    let inv_width=1.0/max(1.0,range);
    uv*=saturate((behind_depth-surface_depth)*inv_width+bias);
    uv*=4.0;
    return uv*saturate((behind_depth-surface_depth)*1.0/30.0);
}
struct RiverSlwVolume {luminance:vec3<f32>,water_to_scene:vec3<f32>,water_to_scene_to_light:vec3<f32>}
// EvaluateWaterVolumeLighting after its samples; camera above water and the
// main directional light not separated. Depths/heights in Unreal cm.
fn river_slw_volume(c:RiverSlwCoefficients,water_surface_depth:f32,behind_depth:f32,behind_luminance:vec3<f32>,
    one_over_pre_exposure:f32,surface_z:f32,behind_z:f32,camera_vector:vec3<f32>,normal:vec3<f32>,ior:f32,
    light_direction:vec3<f32>,sun:vec3<f32>,ambient:vec3<f32>,env_brdf:vec3<f32>,shadow:f32,light_function:vec3<f32>,
    visibility:f32)->RiverSlwVolume {
    var out=RiverSlwVolume(vec3<f32>(0.0),vec3<f32>(1.0),vec3<f32>(1.0));
    if !(visibility>0.0) {return out;}
    let depth=max(0.0,behind_depth-water_surface_depth);
    let scattering=max(vec3<f32>(0.0),c.scattering);
    let absorption=max(vec3<f32>(0.0),c.absorption);
    let phase_g=clamp(c.phase_g,-1.0,1.0);
    let scale_behind=mix(vec3<f32>(1.0),max(vec3<f32>(0.0),c.color_scale),saturate(depth*0.02));
    let extinction=scattering+absorption;
    let extinction_safe=max(extinction,vec3<f32>(1e-5));
    var phase=0.0;
    let ray=river_slw_refract(camera_vector,normal,1.0/ior);
    if ray.w>0.0 {phase=river_slw_schlick_phase(phase_g,dot(-light_direction,ray.xyz));}
    let to_top=max(0.0,surface_z-behind_z);
    let mean_to_light=exp(-to_top*extinction);
    let behind=behind_luminance*(mean_to_light*one_over_pre_exposure);
    let sun_scatter=phase*sun;
    let amb_scatter=river_slw_isotropic_phase()*ambient;
    let transmittance=exp(-(extinction*depth));
    let incoming=amb_scatter+sun_scatter*light_function*shadow*1.0;
    let amount=saturate(scattering*(1.0-transmittance)/extinction_safe);
    let scattered=incoming*amount*(1.0-env_brdf);
    out.luminance=visibility*(scattered+transmittance*(behind*scale_behind));
    out.water_to_scene=transmittance;
    out.water_to_scene_to_light=transmittance*mean_to_light;
    return out;
}
// ShadingModels.ush DefaultLitBxDF for SLW's forward directional light: no
// anisotropy, zero sphere/line extent (point-like sun disc) and UE with
// r.Shading.EnergyConservation 0 (the energy LUTs are not ported yet).
// Returns diffuse in .xyz-of-a and specular in b, before the light colour.
struct RiverSlwDirect {diffuse:vec3<f32>,specular:vec3<f32>}
fn river_slw_direct(n:vec3<f32>,v:vec3<f32>,l:vec3<f32>,diffuse_color:vec3<f32>,specular_color:vec3<f32>,roughness:f32)->RiverSlwDirect {
    var out=RiverSlwDirect(vec3<f32>(0.0),vec3<f32>(0.0));
    let n_dot_l=saturate(dot(n,l));
    if !(n_dot_l>0.0) {return out;}
    let h=normalize(v+l);
    let n_dot_h=dot(n,h);let v_dot_h=dot(v,h);
    let n_dot_v=saturate(abs(dot(n,v))+1e-5);
    out.diffuse=diffuse_color*(1.0/3.1415926535897932)*(1.0*n_dot_l);
    let a2=roughness*roughness*roughness*roughness;
    let d0=(n_dot_h*a2-n_dot_h)*n_dot_h+1.0;
    let d=a2/(3.1415926535897932*d0*d0);
    let a=sqrt(a2);
    let vis=0.5/(n_dot_l*(n_dot_v*(1.0-a)+a)+n_dot_v*(n_dot_l*(1.0-a)+a));
    let fc=pow(1.0-v_dot_h,5.0);
    let f=saturate(50.0*specular_color.g)*fc+(1.0-fc)*specular_color;
    out.specular=(1.0*n_dot_l)*((d*vis)*f);
    return out;
}
