// Selected native live primary foam graph. Copyright Epic Games, Inc.
// All Rights Reserved. foam-reference.json; no replacement foam/noise producer.
struct RiverFoamSettings {
    fits:vec4<f32>, // mask fit/power, scattering fit/power
    velocity:vec4<f32>, // low scale, min/max, normal flatness
    presentation:vec4<f32>, // gain, power, black-point/mult lerp, render fit
    shading:vec4<f32>, // foam opacity, roughness, specular, unused
    base_color:vec4<f32>,scattering_color:vec4<f32>,scattering_velocity:vec4<f32>,
}
struct RiverFoam {normal:vec3<f32>,mask:f32,scattering_mask:f32,total:f32,opacity:f32,scattering:vec3<f32>}
fn river_foam_power(base:f32,exponent:f32)->f32 {
    if base>2.980233e-8 {return pow(base,exponent);}return 0.0;
}
fn river_foam_present(field:f32,detail:vec4<f32>,magnitude:f32,s:RiverFoamSettings)->RiverFoam {
    let low=mix(1.0,smoothstep(s.velocity.y,s.velocity.z,magnitude),1.0-clamp(s.velocity.x,0.0,1.0));
    let mask=(river_foam_power(clamp(field*s.fits.x,0.0,1.0),s.fits.y)/s.fits.x)*low;
    let scatter_mask=river_foam_power(clamp(field*s.fits.z,0.0,1.0),s.fits.w)/s.fits.z;
    let black=smoothstep(1.0-clamp(mask,0.0,1.0),1.0,clamp(detail.a,0.0,1.0));
    let combined=mix(black,mask*detail.a,s.presentation.z);
    let primary=river_foam_power(combined,s.presentation.y)*s.presentation.x;
    let total=smoothstep(0.0,s.presentation.w,clamp(max(primary,0.0),0.0,1.0));
    let normal=normalize(mix(detail.xyz,vec3<f32>(0.0,0.0,1.0),s.velocity.w));
    let scatter_alpha=max(smoothstep(s.scattering_velocity.x,s.scattering_velocity.y,magnitude),scatter_mask);
    return RiverFoam(normal,mask,scatter_mask,total,total*s.shading.x,(s.scattering_color.xyz*s.scattering_color.w)*scatter_alpha);
}
fn river_foam_cubic_weights(d:vec2<f32>)->array<vec2<f32>,4> {
    return array<vec2<f32>,4>(-1.0/3.0*d+0.5*d*d-1.0/6.0*d*d*d,
        vec2<f32>(1.0)-d*d+0.5*(d*d*d-d),d+0.5*(d*d-d*d*d),1.0/6.0*(d*d*d-d));
}
fn river_foam_field(tex:texture_2d<f32>,clamped_sampler:sampler,uv:vec2<f32>)->vec4<f32> {
    let dims=vec2<i32>(textureDimensions(tex,0));let position=uv*vec2<f32>(dims)-vec2<f32>(0.5);
    let cell=vec2<i32>(floor(position));
    if any(cell<vec2<i32>(1))||any(cell>=dims-vec2<i32>(2)) {return textureSampleLevel(tex,clamped_sampler,uv,0.0);}
    let weights=river_foam_cubic_weights(position-vec2<f32>(cell));
    var minv=vec4<f32>(3.402823466e+38);var maxv=vec4<f32>(-3.402823466e+38);var value=vec4<f32>(0.0);
    for(var j=0u;j<4u;j++) {
        var row=vec4<f32>(0.0);
        for(var i=0u;i<4u;i++) {
            let v=textureLoad(tex,cell+vec2<i32>(i32(i)-1,i32(j)-1),0);
            row+=weights[i].x*v;minv=min(v,minv);maxv=max(v,maxv);
        }
        value+=weights[j].y*row;
    }
    return min(max(value,minv),maxv);
}
struct RiverFoamAttributes {base_color:vec3<f32>,roughness:f32,specular:f32,opacity:f32}
fn river_foam_attributes(incoming:RiverFoamAttributes,foam:RiverFoam,s:RiverFoamSettings)->RiverFoamAttributes {
    return RiverFoamAttributes(mix(incoming.base_color,s.base_color.xyz,foam.opacity),mix(incoming.roughness,s.shading.y,foam.opacity),mix(incoming.specular,s.shading.z,foam.opacity),incoming.opacity+foam.opacity);
}
fn river_live_grid_foam(fluid_tex:texture_2d<f32>,normal_tex:texture_2d<f32>,normal_sampler:sampler,foam_field:texture_2d<f32>,field_sampler:sampler,foam_detail:texture_2d<f32>,detail_sampler:sampler,uv:vec2<f32>,world_xy:vec2<f32>,time:f32,advection:RiverAdvection,s:RiverFoamSettings)->RiverFoam {
    let fluid=river_live_field(fluid_tex,uv);let base=textureSampleLevel(normal_tex,normal_sampler,uv,0.0).xyz;
    let detail=river_wave_fragment(foam_detail,detail_sampler,world_xy,fluid.zw,base,time,advection);
    let field=river_foam_field(foam_field,field_sampler,uv).r;
    return river_foam_present(field,detail,length(fluid.zw),s);
}

// Screen-space foam A/B: keep the native bicubic filter when a field cell
// covers at least one screen pixel; bilinear when foam varies subpixel.
// The original filtered field remains the mode 0 reference.
fn river_foam_field_lod(tex:texture_2d<f32>,clamped_sampler:sampler,uv:vec2<f32>,lod:vec2<f32>)->vec4<f32> {
    let dims=vec2<f32>(textureDimensions(tex,0));
    let dx=dpdx(uv)*dims;
    let dy=dpdy(uv)*dims;
    let pixel_cells=max(length(dx),length(dy));
    // Cutoff 4 = 1.5 cells per pixel; the slider shifts it in mips, floored at 1 cell/pixel.
    if lod.x >= 1.5 && log2(max(pixel_cells,1e-8)) >= max(lod.y-3.415,0.0) {
        return textureSampleLevel(tex,clamped_sampler,uv,0.0);
    }
    return river_foam_field(tex,clamped_sampler,uv);
}
// Same authored foam graph, with wave and field LOD. Mode 1 changes wave
// samples only; mode 2 adds adaptive foam-field reconstruction.
fn river_live_grid_foam_lod(fluid_tex:texture_2d<f32>,normal_tex:texture_2d<f32>,normal_sampler:sampler,foam_field:texture_2d<f32>,field_sampler:sampler,foam_detail:texture_2d<f32>,detail_sampler:sampler,uv:vec2<f32>,world_xy:vec2<f32>,time:f32,advection:RiverAdvection,s:RiverFoamSettings,material_lod:vec2<f32>)->RiverFoam {
    let fluid=river_live_field(fluid_tex,uv);
    let base=textureSampleLevel(normal_tex,normal_sampler,uv,0.0).xyz;
    let detail=river_wave_fragment_lod(foam_detail,detail_sampler,world_xy,fluid.zw,base,time,advection,material_lod);
    let field=river_foam_field_lod(foam_field,field_sampler,uv,material_lod).r;
    return river_foam_present(field,detail,length(fluid.zw),s);
}
