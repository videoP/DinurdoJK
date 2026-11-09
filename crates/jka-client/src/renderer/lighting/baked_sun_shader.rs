const SKY_AND_AUX_WGSL: &str = r#"
@group(0) @binding(7) var sun_aux:texture_storage_2d<rgba8unorm,write>;
@group(0) @binding(8) var sky_normals:texture_2d<u32>;
@group(0) @binding(9) var sky_atlas:texture_2d<f32>;
@group(0) @binding(10) var sky_atlas_sampler:sampler;
@group(0) @binding(11) var<uniform> sky_environment:SkyEnvironment;
fn sky_octahedral_decode(e:vec2<f32>)->vec3<f32> {
    var n=vec3(e.x,e.y,1.0-abs(e.x)-abs(e.y));
    let t=max(-n.z,0.0);
    n.x+=select(t,-t,n.x>=0.0);
    n.y+=select(t,-t,n.y>=0.0);
    return normalize(n);
}
// The Blue Water sky world illumination of blue_water_scene.wgsl `light`,
// evaluated once per luxel with the bake's smoothed normal (and optional sky
// visibility) instead of a per-pixel depth-derived facet normal.
fn sky_world_gain(at:vec2<i32>)->vec3<f32> {
    if parameters.sky.y<0.5 { return vec3(1.0); }
    let packed=textureLoad(sky_normals,at,0).r;
    if (packed & 0x80000000u)==0u { return vec3(1.0); }
    let e=vec2(f32((packed>>11u)&1023u),f32((packed>>21u)&1023u))/1023.0*2.0-1.0;
    let n=sky_octahedral_decode(e);
    let ambient_visibility=f32(packed&63u)/63.0;
    if parameters.sky.y>1.5 {
        return vanilla_sky_gain(n,parameters.moon.xyz,ambient_visibility,parameters.sky.x);
    }
    // Reuse the completed, edge-antialiased sun/moon bake at its native
    // resolution. Do not introduce a second shadow edge from the original
    // lightmap grid or quantize this visibility into the packed sky normal.
    let incident=max(textureLoad(sun_flux,at,0).r,0.0);
    let cosine=max(textureLoad(sun_flux,at+vec2(0,i32(textureDimensions(sun_flux).y/2u)),0).r,0.0);
    let direct_visibility=clamp(incident/max(cosine,1e-5),0.0,1.0);
    let ambient=env_irradiance(sky_atlas,sky_atlas_sampler,sky_environment,n)/3.14159265*ambient_visibility;
    // Exactly match blue_water_scene.wgsl and model_sky_light: at night,
    // sky_environment.light_* can still refer to the solar key.
    let moon_active=parameters.sky.w>0.5;
    let direction=select(sky_environment.light_direction,parameters.moon.xyz,moon_active);
    let color=select(sky_environment.light_color,vec3<f32>(0.70,0.82,1.0),moon_active);
    let intensity=select(sky_environment.light_intensity,parameters.moon.w,moon_active);
    let incidence=max(dot(n,direction),0.0);
    let direct=color*intensity*incidence*direct_visibility;
    let full_gain=clamp(ambient*0.5+direct*0.2,vec3(0.03),vec3(4.0));
    // Existing lightmaps combine sky/direct GI with compiled point lights.
    // Never darken a sealed room merely because its surface faces upward.
    // This geometric weight is a conservative approximation of the DAYLIGHT
    // share, not a claim to reconstruct individual q3map2 light sources.
    let exposure=clamp(max(ambient_visibility,direct_visibility*incidence),0.0,1.0);
    let weight=select(1.0,exposure,parameters.smoothing.w>0.5);
    return mix(vec3(1.0),full_gain,parameters.sky.x*weight);
}
"#;

pub(crate) fn cache_shader() -> String {
    // Both the live fallback and the bake execute the same subtraction/tone
    // functions. There is one compositor regardless of which input changed.
    let world = include_str!("../../bsp.wgsl");
    let tone = world.find("fn baked_indirect_tone(").unwrap();
    let start = world.find("fn baked_file_to_linear(").unwrap();
    let end = world.find("fn sample_authored_baked_sun(").unwrap();
    let shader = format!(
        r#"#import naval_sky::environment::{{SkyEnvironment, env_irradiance}}
struct Settings {{ baked_sun_direction_strength:vec4<f32>, baked_sun_radiance:vec4<f32>, baked_sun_debug_mode:vec4<u32>, baked_indirect_adjust:vec4<f32>, baked_indirect_curve:vec4<f32> }}
var<private> lighting_settings:Settings;
struct VertexOut {{baked_sun_fit:vec4<f32>,world_normal:vec3<f32>}}
struct Parameters {{fit:vec4<f32>,radiance_strength:vec4<f32>,adjust:vec4<f32>,curve:vec4<f32>,sun:vec4<f32>,direction_strength:vec4<f32>, smoothing:vec4<f32>, sky:vec4<f32>, moon:vec4<f32>}}
@group(0) @binding(0) var source:texture_2d<f32>;
@group(0) @binding(1) var corrected:texture_storage_2d<rgba16float,write>;
@group(0) @binding(2) var<uniform> parameters:Parameters;
@group(0) @binding(3) var sun_flux:texture_2d<f32>;
@group(0) @binding(4) var baked_local_texture:texture_2d_array<f32>;
@group(0) @binding(5) var<storage,read> local_colors:array<vec4<f32>>;
@group(0) @binding(6) var charts:texture_2d<u32>;
fn baked_luminance(rgb:vec3<f32>)->f32 {{return dot(rgb,vec3(0.2126,0.7152,0.0722));}}
{}
{}
fn source_luxel(at:vec2<i32>)->vec4<f32> {{
    let texel=textureLoad(source,clamp(at,vec2(0),vec2<i32>(textureDimensions(source))-1),0);
    var input:VertexOut; input.baked_sun_fit=parameters.fit;input.world_normal=vec3(0.0,1.0,0.0);
    var rgb=texel.rgb;
    if parameters.fit.z==4.0 {{rgb=subtract_authored_baked_sun(rgb,texel.a,input);}}
    return vec4(rgb,texel.a);
}}

// Reconstruct from immutable luxels, not from previously smoothed output.
// Both kernels use a 4x4 neighborhood only during the change-triggered bake.
fn cubic_weight(d:f32, mode:f32)->f32 {{
    let x=abs(d);
    if mode > 1.5 {{
        // Cubic B-spline: gentle low-pass with continuous gradients.
        if x < 1.0 {{ return (4.0-6.0*x*x+3.0*x*x*x)/6.0; }}
        if x < 2.0 {{ let t=2.0-x; return t*t*t/6.0; }}
    }} else {{
        // Catmull-Rom: interpolate the authored luxels without softening them.
        if x < 1.0 {{ return 1.0-2.5*x*x+1.5*x*x*x; }}
        if x < 2.0 {{ return 2.0-4.0*x+2.5*x*x-0.5*x*x*x; }}
    }}
    return 0.0;
}}
fn chart_at(at:vec2<i32>)->u32 {{
    if parameters.smoothing.z < 0.5 {{ return 0u; }}
    return textureLoad(charts,clamp(at,vec2(0),vec2<i32>(textureDimensions(charts))-1),0).r;
}}
fn filtered_source(at:vec2<f32>)->vec4<f32> {{
    let p=vec2<i32>(floor(at));
    let f=fract(at);
    let c00=source_luxel(p);
    let c10=source_luxel(p+vec2(1,0));
    let c01=source_luxel(p+vec2(0,1));
    let c11=source_luxel(p+vec2(1,1));
    let bilinear=mix(mix(c00,c10,f.x),mix(c01,c11,f.x),f.y);
    if parameters.smoothing.x < 0.5 {{ return bilinear; }}
    // The chart of the nearest original luxel owns this output luxel. Taps
    // from any other atlas chart are dropped and the kernel renormalized, so
    // neighbouring islands in the packed lightmap can never bleed in.
    let nearest=p+select(vec2(0),vec2(1),f>=vec2(0.5));
    let owner=chart_at(nearest);
    var value=vec4(0.0);
    var weights=0.0;
    var lo=vec4(1e30);
    var hi=vec4(-1e30);
    for (var y=0; y<4; y+=1) {{
        for (var x=0; x<4; x+=1) {{
            let offset=vec2<i32>(x-1,y-1);
            if owner != 0u && chart_at(p+offset) != owner {{ continue; }}
            let luxel=source_luxel(p+offset);
            let weight=cubic_weight(f.x-f32(offset.x),parameters.smoothing.x)
                       *cubic_weight(f.y-f32(offset.y),parameters.smoothing.x);
            value+=luxel*weight;
            weights+=weight;
            // Limit halo/overshoot to the same-chart 2x2 footprint.
            if all(offset>=vec2(0)) && all(offset<=vec2(1)) {{ lo=min(lo,luxel); hi=max(hi,luxel); }}
        }}
    }}
    if weights <= 1e-4 {{ return source_luxel(nearest); }}
    return clamp(value/weights,lo,hi);
}}
fn raw_bilinear(at:vec2<f32>)->vec4<f32> {{
    let p=vec2<i32>(floor(at));
    let f=fract(at);
    let size=vec2<i32>(textureDimensions(source))-1;
    let a=textureLoad(source,clamp(p,vec2(0),size),0);
    let b=textureLoad(source,clamp(p+vec2(1,0),vec2(0),size),0);
    let c=textureLoad(source,clamp(p+vec2(0,1),vec2(0),size),0);
    let d=textureLoad(source,clamp(p+vec2(1,1),vec2(0),size),0);
    return mix(mix(a,b,f.x),mix(c,d,f.x),f.y);
}}
fn signal_hue(h:f32)->vec3<f32> {{
    return clamp(abs(fract(h+vec3(0.0,2.0/3.0,1.0/3.0))*6.0-3.0)-1.0,vec3(0.0),vec3(1.0));
}}
// r_lightmapSignal: show ONE input of the composition in place of the result.
fn lightmap_signal(which:u32,id:vec2<u32>,at:vec2<f32>,base:vec4<f32>,dims:vec2<u32>)->vec3<f32> {{
    if which==1u {{ return raw_bilinear(at).rgb; }}
    if which==2u {{ return baked_indirect_tone(base.rgb); }}
    if which==3u {{ return vec3(raw_bilinear(at).a); }}
    // New-sun layers exist only once the static sun bake is composed.
    if parameters.sun.w<=0.0 {{ return vec3(1.0,0.0,1.0); }}
    let at_flux=vec2<i32>(id);
    let layer_a=textureLoad(sun_flux,at_flux,0).r;
    let layer_b=textureLoad(sun_flux,at_flux+vec2(0,i32(dims.y)),0).r;
    if which==4u {{ return vec3(max(layer_a,0.0)); }}
    if which==5u {{ return vec3(max(layer_b,0.0)); }}
    if which==6u {{
        if layer_b<=0.001 {{ return vec3(0.02,0.05,0.6); }}
        return mix(vec3(1.0,0.03,0.02),vec3(0.02,1.0,0.03),clamp(layer_a/layer_b,0.0,1.0));
    }}
    if layer_a<=0.0 && layer_b<=0.0 {{ return vec3(0.0); }}
    // Triangle boundary: the receiver-triangle hash differs from a neighbour.
    var offsets=array<vec2<i32>,4>(vec2(1,0),vec2(-1,0),vec2(0,1),vec2(0,-1));
    var edge=false;
    for (var i=0; i<4; i+=1) {{
        let n=clamp(at_flux+offsets[i],vec2(0),vec2<i32>(dims)-1);
        let other=textureLoad(sun_flux,n+vec2(0,i32(dims.y)),0).r;
        if other>0.0 && abs(other-layer_b)>1e-5 {{ edge=true; }}
    }}
    var color=signal_hue(layer_a)*0.75+0.1;
    if which==8u {{ color=mix(vec3(0.08),vec3(1.0,0.75,0.1),clamp(layer_a,0.0,1.0)); }}
    return select(color,vec3(1.0),edge);
}}
@compute @workgroup_size(8,8)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {{
    let dims=textureDimensions(corrected);if any(id.xy>=dims) {{return;}}
    lighting_settings.baked_sun_direction_strength=vec4(0.0,1.0,0.0,parameters.radiance_strength.w);
    lighting_settings.baked_sun_radiance=vec4(parameters.radiance_strength.rgb,1.0);
    lighting_settings.baked_sun_debug_mode=vec4<u32>(0u);
    lighting_settings.baked_indirect_adjust=parameters.adjust;
    lighting_settings.baked_indirect_curve=parameters.curve;
    // Subtract each original luxel BEFORE resampling, matching normal filtering.
    let at=(vec2<f32>(id.xy)+0.5)*vec2<f32>(textureDimensions(source))/vec2<f32>(dims)-0.5;
    let base=filtered_source(at);
    var result=base;
    if parameters.sun.w>0.0 {{
        let incident=max(textureLoad(sun_flux,vec2<i32>(id.xy),0).r,0.0);
        let indirect=baked_indirect_tone(base.rgb);
        if parameters.sun.w<1.5 {{
            result=vec4(indirect+parameters.sun.rgb*incident,incident);
        }} else {{
            // Keep static-world darkening separate from moving actors: baked
            // world shadows already have contrast in the original lightmap.
            let facing=parameters.direction_strength.w;
            // Flux already includes cosine; the unoccluded facing is stored
            // separately in the second half of the incident image (below).
            let cosine=max(textureLoad(sun_flux,vec2<i32>(id.xy)+vec2(0,i32(dims.y)),0).r,0.0);
            let factor=max(1.0-facing*(cosine-incident),1e-5);
            result=vec4(indirect*factor,clamp(parameters.curve.w*incident/factor,0.0,1.0));
        }}
    }}
    if parameters.smoothing.y > 0.5 {{
        textureStore(corrected,vec2<i32>(id.xy),vec4(lightmap_signal(u32(parameters.smoothing.y),id.xy,at,base,dims),0.0));
        return;
    }}
    // Keep newly promoted map lamps independent of the sky tint in isolation
    // mode. The legacy A/B still applies the old full-lightmap multiplier.
    var local=vec3<f32>(0.0);
    // Existing sky-isolation behavior adds promoted map lamps after tint.
    // Vanilla sky similarly preserves promoted lights irrespective of toggle.
    let keep_local=parameters.smoothing.w>0.5 || parameters.sky.y>1.5;
    if parameters.curve.z > 0.0 {{
        if parameters.sun.w <= 0.0 {{ result = vec4(baked_indirect_tone(base.rgb),0.0); }}
        let uv=(vec2<f32>(id.xy)+0.5)/vec2<f32>(dims);
        for (var i=0u;i<u32(parameters.curve.z);i+=1u) {{ local += local_colors[i].rgb*sample_baked_local(uv,i32(i)); }}
        if !keep_local {{ result = vec4(result.rgb + local, result.a); }}
    }}
    let explicit_sun=parameters.sun.w>0.0 && parameters.sun.w<1.5;
    let gain=sky_world_gain(vec2<i32>(id.xy));
    let gain_y=max(baked_luminance(gain),1e-4);
    result=vec4(result.rgb*gain,result.a*select(1.0,gain_y,explicit_sun));
    if keep_local {{ result = vec4(result.rgb + local, result.a); }}
    if parameters.sky.z>0.5 || parameters.sky.x>0.0 {{
        // Unshadowed minus baked static sunlight, in alpha units: inside
        // cascade range the static-world CSM re-applies visibility to it.
        var shadowed=0.0;
        if explicit_sun {{
            let incident=max(textureLoad(sun_flux,vec2<i32>(id.xy),0).r,0.0);
            let cosine=max(textureLoad(sun_flux,vec2<i32>(id.xy)+vec2(0,i32(dims.y)),0).r,0.0);
            shadowed=max(cosine-incident,0.0)*gain_y;
        }}
        // R remains the static-world CSM shadow amount. G/B/A hold the
        // sky multiplier used above (neutral when sky normals are absent); no re-tracing or live sky lookup
        // is required for post-lightmap alpha/additive material overlays.
        // Encoding g/(1+g) preserves both dark night gain and values >1 in
        // this existing rgba8unorm allocation. A neutral gain encodes 0.5.
        let packed_gain=gain/(vec3(1.0)+gain);
        textureStore(sun_aux,vec2<i32>(id.xy),vec4(clamp(shadowed*0.25,0.0,1.0),packed_gain));
    }}
    textureStore(corrected,vec2<i32>(id.xy),result);
}}
"#,
        &world[tone..start],
        &world[start..end]
    );
    let helpers = include_str!("lightmap_local.wgsl")
        .split("// Runtime receiver helpers.")
        .next()
        .unwrap();
    shader.replace(
        "@compute @workgroup_size",
        &format!("{helpers}\n{}\n{SKY_AND_AUX_WGSL}\n@compute @workgroup_size", include_str!("vanilla_sky_lighting.wgsl")),
    )
}

