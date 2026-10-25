// Shared bilinear lookup for the compositor and moving-shadow correction.
fn sample_baked_local(uv: vec2<f32>, layer: i32) -> f32 {
    let dims = vec2<i32>(textureDimensions(baked_local_texture));
    let at = uv * vec2<f32>(dims) - 0.5;
    let p = vec2<i32>(floor(at)); let f = fract(at);
    let a = textureLoad(baked_local_texture, clamp(p,vec2(0),dims-1),layer,0).r;
    let b = textureLoad(baked_local_texture, clamp(p+vec2(1,0),vec2(0),dims-1),layer,0).r;
    let c = textureLoad(baked_local_texture, clamp(p+vec2(0,1),vec2(0),dims-1),layer,0).r;
    let d = textureLoad(baked_local_texture, clamp(p+vec2(1,1),vec2(0),dims-1),layer,0).r;
    return max(mix(mix(a,b,f.x),mix(c,d,f.x),f.y),0.0);
}
// Runtime receiver helpers.
fn composed_lightmap_receiver(input: VertexOut) -> bool {
    return (lighting_settings.baked_sun_debug_mode.z & 3u) != 0u && lightmapped_shadow_receiver(input);
}
fn lightmapped_shadow_receiver(input: VertexOut) -> bool {
    return input.shell_kind == 0u
        && ((material_flags() & (2u | 8388608u | 16777216u)) != 0u || material_tc_gen() == 1u);
}
fn baked_local_receiver(input: VertexOut) -> bool {
    return (lighting_settings.baked_sun_debug_mode.z & 2u) != 0u && composed_lightmap_receiver(input);
}
fn local_light_already_baked(light: PointLight, input: VertexOut) -> bool {
    return light.shape.w > 0.5 && baked_local_receiver(input) && !dynamic_light_debug_enabled();
}
// Estimate a lamp's incident energy, not its share of the selected lamp set.
// Original BSP RGB has no per-source decomposition, so this remains an estimate.
fn inferred_lamp_weight(light: PointLight, position: vec3<f32>, normal: vec3<f32>) -> f32 {
    let delta=light.position_radius.xyz-position;
    let d2=dot(delta,delta); let radius=max(light.position_radius.w,1.0);
    if d2>=radius*radius || d2<0.0001 {return 0.0;}
    let toward=delta*inverseSqrt(d2);
    let receive=max(dot(normalize(normal),toward),0.0);
    var emission=1.0;
    if light.emitter.w>=0.5 && light.emitter.w<2.5 {
        let cosine=dot(normalize(light.emitter.xyz),-toward);
        emission=select(max(cosine,0.0),abs(cosine),light.emitter.w>1.5);
    }
    let window=max(1.0-d2/(radius*radius),0.0);
    return max(light.shape.y,0.0)*receive*emission*window*window/(d2+256.0);
}
fn shade_original_lamp_shadows(lightmap: vec3<f32>, input: VertexOut) -> vec3<f32> {
    let vertex_lit = input.shell_kind == 0u && (material_flags() & 131073u) == 131073u;
    if !lightmapped_shadow_receiver(input) && !vertex_lit {return lightmap;}
    let ids=vec2<u32>(lighting_settings.baked_indirect_curve.zw);
    // A/B=4: before doing expensive per-light attribution or any PCF,
    // prove that at least one lamp has a moving caster along this receiver
    // direction. The test is conservative: failing means output is *exactly*
    // the original lightmap.
    // Inferred lamp visibility uses raster cubemaps in every lighting mode,
    // so their face/octant occupancy also bounds RT-enabled receivers.
    if (lighting_settings.baked_sun_debug_mode.w & 4u) != 0u && lamp_raster_receiver_culling_safe() {
        var shadow_possible = false;
        for (var j=0u; j<4u; j+=1u) {
            let encoded=(ids[j/2u]>>((j%2u)*8u))&255u;
            if encoded == 0u { continue; }
            let lamp=dynamic_lights[encoded-1u];
            // Conservative cheap sphere bound before the face/octant test.
            let delta=input.world_position-lamp.position_radius.xyz;
            if dot(delta,delta) > lamp.position_radius.w*lamp.position_radius.w {continue;}
            if baked_local_moving_maybe(lamp,input.world_position) { shadow_possible = true; break; }
        }
        if !shadow_possible {return lightmap;}
    }
    var blocked=0.0;
    for (var i=0u;i<4u;i+=1u) {
        let encoded=(ids[i/2u]>>((i%2u)*8u))&255u;
        if encoded==0u {continue;}
        let light=dynamic_lights[encoded-1u];
        // Reuse existing face/octant metadata before the irradiance math.
        // This adds no texture reads and also applies with transient RT enabled.
        if lamp_raster_receiver_culling_safe() && !baked_local_moving_maybe(light,input.world_position) {continue;}
        let weight=inferred_lamp_weight(light,input.world_position,input.world_normal);
        if weight<=0.0 || light.shadow.x<0.5 {continue;}
        let moving=baked_local_moving_visibility(light,input.world_position,input.world_normal);
        if moving>=0.9999 {continue;}
        // A lamp behind static geometry must not darken the original map here.
        let stationary=sample_local_shadow_visibility(light,input.world_position,input.world_normal);
        blocked+=weight*stationary*(1.0-moving);
    }
    if blocked<=0.0 {return lightmap;}
    // Remove absolute blocked energy from the available baked illumination.
    // Dividing by the selected lamps' sum cancelled distance/power falloff and
    // let unrelated (including wall-hidden) lamps change shadow contrast.
    // Keep the existing 0.65 gain and 52% maximum removal, preserving hue and
    // a residual for indirect/unidentified lighting. No extra visibility taps.
    let baseline=max(dot(max(lightmap,vec3(0.0)),vec3(0.2126,0.7152,0.0722)),1e-6);
    let fraction=min(0.52,0.65*blocked/baseline);
    return lightmap*(1.0-fraction);
}
fn shade_baked_local(lightmap: vec3<f32>, input: VertexOut) -> vec3<f32> {
    if (lighting_settings.baked_sun_debug_mode.z & 4u) != 0u {
        if (lighting_settings.baked_sun_debug_mode.w & 1u)==0u {return lightmap;}
        return shade_original_lamp_shadows(lightmap,input);
    }
    if !baked_local_receiver(input) || (lighting_settings.baked_sun_debug_mode.w & 1u) == 0u { return lightmap; }
    var blocked = vec3<f32>(0.0);
    let ids=vec2<u32>(lighting_settings.baked_indirect_curve.zw);
    for (var i=0u;i<4u;i+=1u) {
        let encoded=(ids[i/2u] >> ((i%2u)*8u)) & 255u;
        if encoded==0u {continue;}
        let light=dynamic_lights[encoded-1u];
        if light.shape.w < 0.5 || light.shadow.x < 0.5 { continue; }
        let delta = light.position_radius.xyz - input.world_position;
        if dot(delta,delta) >= light.position_radius.w*light.position_radius.w { continue; }
        let visibility = baked_local_moving_visibility(light,input.world_position,input.world_normal);
        if visibility >= 0.9999 { continue; }
        blocked += light.color_intensity.rgb * sample_baked_local(input.lightmap_uv,i32(light.shape.w-1.0)) * (1.0-visibility);
    }
    return max(lightmap-blocked,vec3(0.0));
}

fn baked_local_total(input: VertexOut) -> vec3<f32> {
    if !baked_local_receiver(input) { return vec3(0.0); }
    // The worker stores the sum once. Cost must not scale with bake sources.
    let first = i32(textureNumLayers(baked_local_texture)) - 3;
    if first < 0 { return vec3(0.0); }
    return vec3(sample_baked_local(input.lightmap_uv,first),
        sample_baked_local(input.lightmap_uv,first+1),
        sample_baked_local(input.lightmap_uv,first+2));
}
