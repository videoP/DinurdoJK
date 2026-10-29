// Shared BSP / MD3 / Ghoul2 glass optics. Snell refraction follows Bevy's
// transmission.wesl (MIT; LICENSE-BevyGlass-MIT). Profiles are analytic curves.
struct GlassSettings {
    optical: vec4<f32>, // IOR, thickness, body distortion, blur radius
    color: vec4<f32>, // tint RGB, absorption strength
    effects: vec4<f32>, // dispersion, roughness, reflection, style
    environment: vec4<f32>,
    appearance: vec4<f32>, // frost mix, haze opacity, effect strength, magnification
    edge: vec4<f32>, // width pixels, refraction strength, profile, max displacement
    rim: vec4<f32>, // strength, width, softness, angle radians
    detail: vec4<f32>, // progressive blur, blur mode, waviness, animation speed
    highlights: vec4<f32>, // rim saturation, rim type, dispersion base, background recovery
};

fn glass_mask_value(world: vec3<f32>, normal: vec3<f32>, eye: vec3<f32>, forward: vec3<f32>) -> vec4<f32> {
    let n=select(vec3<f32>(0.0,0.0,1.0),normal/max(length(normal),0.0001),length(normal)>0.0001);
    var oct=n.xy/(abs(n.x)+abs(n.y)+abs(n.z));
    if (n.z<0.0) { oct=(1.0-abs(oct.yx))*select(vec2<f32>(-1.0),vec2<f32>(1.0),oct>=vec2<f32>(0.0)); }
    return vec4<f32>(oct,clamp(dot(world-eye,forward),0.0,60000.0),1.0);
}
fn glass_clamp_uv(uv: vec2<f32>) -> vec2<f32> {
    let pixel=0.5/vec2<f32>(textureDimensions(glass_scene));
    return clamp(uv,pixel,1.0-pixel);
}
fn glass_background(uv: vec2<f32>, fallback: vec2<f32>, surface_depth: f32) -> vec3<f32> {
    let safe=glass_clamp_uv(uv);
    let dims=vec2<f32>(textureDimensions(glass_scene));
    // Reject each bilinear contributor, rather than only the nearest depth.
    // A valid sample beside an actor can still interpolate its foreground color.
    let position=safe*dims-0.5;
    let base=vec2<i32>(floor(position)); let fraction=fract(position);
    var sum=vec3<f32>(0.0); var coverage=0.0;
    for (var y=0;y<2;y+=1) { for (var x=0;x<2;x+=1) {
        let pixel=clamp(base+vec2<i32>(x,y),vec2<i32>(0),vec2<i32>(dims)-1);
        let weight=select(1.0-fraction.x,fraction.x,x==1)*select(1.0-fraction.y,fraction.y,y==1);
        let depth=textureLoad(glass_depth,pixel,0).r;
        if (depth>surface_depth+0.5 && weight>0.0) {
            sum+=textureLoad(glass_scene,pixel,0).rgb*weight; coverage+=weight;
        } else if (weight>0.0 && glass_settings.highlights.w>0.5) {
            let background=textureLoad(glass_background_scene,pixel,0);
            if (background.a>surface_depth+0.5) { sum+=background.rgb*weight; coverage+=weight; }
        }
    } }
    if (coverage>0.001) { return sum/coverage; }
    return textureSampleLevel(glass_scene,glass_sampler,fallback,0.0).rgb;
}
fn glass_blurred(uv: vec2<f32>, fallback: vec2<f32>, depth: f32, radius: f32) -> vec3<f32> {
    let sharp=glass_background(uv,fallback,depth);
    if (radius<0.15 || glass_settings.appearance.x<=0.0) { return sharp; }
    if (textureNumLevels(glass_scene)<2u) { return sharp; }
    // Mip zero contains foreground models. Interpolating it with the first
    // masked mip would reintroduce their silhouettes at small blur radii.
    let lod=clamp(log2(max(radius,2.0)),1.0,f32(textureNumLevels(glass_scene)-1u));
    let blur=textureSampleLevel(glass_scene,glass_sampler,glass_clamp_uv(uv),lod);
    // Premultiplied validity prevents a dark fringe around excluded actors.
    let blurred=blur.rgb/max(blur.a,0.0001);
    return mix(sharp,blurred,glass_settings.appearance.x*min(radius*0.5,1.0)*clamp(blur.a*16.0,0.0,1.0));
}
fn glass_has_edges() -> bool {
    return (glass_settings.edge.x>0.0 && (glass_settings.edge.y>0.0 || glass_settings.detail.x>0.0))
        || glass_settings.rim.x>0.0 || glass_settings.appearance.w!=0.0
        || (glass_settings.environment.w>0.0 && glass_settings.optical.z>0.0);
}
fn glass_field(uv: vec2<f32>) -> vec4<f32> {
    let dims=vec2<i32>(textureDimensions(glass_edges));
    return textureLoad(glass_edges,clamp(vec2<i32>(uv*vec2<f32>(dims)),vec2<i32>(0),dims-1),0);
}
fn glass_mask_at(uv: vec2<f32>) -> vec4<f32> {
    let dims=vec2<i32>(textureDimensions(glass_mask));
    return textureLoad(glass_mask,clamp(vec2<i32>(uv*vec2<f32>(dims)),vec2<i32>(0),dims-1),0);
}
fn glass_edge_distance(uv: vec2<f32>, dims: vec2<f32>) -> f32 {
    let nearest=glass_field(uv);
    if (nearest.x<0.0) { return 1e6; }
    let distance=length((nearest.xy-uv)*dims);
    return select(-distance,distance,glass_mask_at(uv).w>=0.25);
}
// Trace along the visible pane, using the nearest-boundary distance to advance.
// Only evaluated when magnification is enabled; no authored UV seams are used.
fn glass_span(uv: vec2<f32>, direction: vec2<f32>, dims: vec2<f32>) -> f32 {
    var position=uv;
    var distance=0.0;
    for (var i=0u;i<24u;i+=1u) {
        let closest=glass_field(position);
        var step=16.0;
        if (closest.x>=0.0) { step=max(length((position-closest.xy)*dims)*0.8,2.0); }
        let candidate=position+direction*step/dims;
        if (any(candidate<=vec2<f32>(0.0)) || any(candidate>=vec2<f32>(1.0)) || glass_mask_at(candidate).w<0.25) {
            var lo=0.0; var hi=step;
            for (var j=0u;j<5u;j+=1u) {
                let mid=(lo+hi)*0.5;
                let probe=position+direction*mid/dims;
                if (all(probe>vec2<f32>(0.0)) && all(probe<vec2<f32>(1.0)) && glass_mask_at(probe).w>=0.25) { lo=mid; } else { hi=mid; }
            }
            return distance+lo;
        }
        position=candidate; distance+=step;
    }
    return distance;
}
fn glass_profile(t: f32) -> vec2<f32> {
    let x=clamp(t,0.001,0.999);
    let circle=sqrt(max(1.0-(1.0-x)*(1.0-x),0.0001));
    let circle_slope=(1.0-x)/circle;
    let quartic=max(1.0-pow(1.0-x,4.0),0.0001);
    let squircle=pow(quartic,0.25);
    let squircle_slope=pow(1.0-x,3.0)*pow(quartic,-0.75);
    let profile=u32(glass_settings.edge.z+0.5);
    if (profile==1u) { return vec2<f32>(squircle,squircle_slope); }
    if (profile==2u) { return vec2<f32>(1.0-circle,-circle_slope); }
    if (profile==3u) {
        let blend=x*x*(3.0-2.0*x);
        let derivative=6.0*x*(1.0-x);
        return vec2<f32>(mix(squircle,1.0-circle,blend),mix(squircle_slope,-circle_slope,blend)+(1.0-circle-squircle)*derivative);
    }
    return vec2<f32>(circle,circle_slope);
}
fn glass_shade(world: vec3<f32>, world_normal: vec3<f32>, fragment: vec4<f32>,
    view_proj: mat4x4<f32>, eye: vec3<f32>, forward: vec3<f32>, time: f32,
    authored_tint: vec3<f32>, authored_f0: f32) -> vec4<f32> {
    let dims=vec2<f32>(textureDimensions(glass_scene));
    let uv=glass_clamp_uv(fragment.xy/dims);
    let clear=textureSampleLevel(glass_scene,glass_sampler,uv,0.0).rgb;
    if (glass_settings.appearance.z<=0.0) { return vec4<f32>(clear,1.0); }
    let v=normalize(eye-world);
    let original_n=normalize(world_normal);
    var n=select(-original_n,original_n,dot(original_n,v)>=0.0);
    let style=u32(glass_settings.effects.w+0.5);
    if (glass_settings.detail.z>0.0) {
        let axis=select(vec3<f32>(0.0,1.0,0.0),vec3<f32>(1.0,0.0,0.0),abs(n.y)>0.95);
        let tangent=normalize(cross(axis,n)); let bitangent=cross(n,tangent);
        let phase=time*glass_settings.detail.w;
        let p=vec2<f32>(dot(world,tangent),dot(world,bitangent))*0.045;
        let warp=vec2<f32>(sin(p.y+phase)*cos(p.x*0.73),sin(p.x*0.87-phase)*cos(p.y));
        n=normalize(n+(tangent*warp.x+bitangent*warp.y)*0.12*glass_settings.detail.z);
    }
    let surface_depth=max(dot(world-eye,forward),0.0);
    let background_depth=textureLoad(glass_depth,vec2<i32>(fragment.xy),0).r;
    let gap=max(background_depth-surface_depth,0.0);
    let ray=refract(-v,n,1.0/glass_settings.optical.x);
    let exit_clip=view_proj*vec4<f32>(world+ray*glass_settings.optical.y,1.0);
    let exit_uv=exit_clip.xy/max(exit_clip.w,0.001)*vec2<f32>(0.5,-0.5)+0.5;
    var displacement=(exit_uv-uv)*glass_settings.optical.z;
    if (style==3u) {
        let screen_n=(view_proj*vec4<f32>(n,0.0)).xy*vec2<f32>(1.0,-1.0);
        displacement=clamp(screen_n,vec2<f32>(-1.0),vec2<f32>(1.0))*0.012*glass_settings.optical.z*min(gap/48.0,1.0);
    }
    var nearest=vec4<f32>(-1.0);
    var edge_distance=1e6;
    var edge_direction=vec2<f32>(0.0);
    var edge_t=1.0;
    if (glass_has_edges()) {
        nearest=glass_field(uv);
        if (nearest.x>=0.0) {
            let delta=(nearest.xy-uv)*dims;
            edge_distance=length(delta);
            edge_direction=delta/max(edge_distance,0.001);
            // A nearest seed switches along the rasterized staircase. Derive
            // a smooth normal from signed distances instead of those switches.
            if (edge_distance<max(glass_settings.edge.x,glass_settings.rim.y+glass_settings.rim.z*3.0)+4.0) {
                let dx=vec2<f32>(4.0/dims.x,0.0); let dy=vec2<f32>(0.0,4.0/dims.y);
                let gradient=vec2<f32>(glass_edge_distance(uv+dx,dims)-glass_edge_distance(uv-dx,dims),
                    glass_edge_distance(uv+dy,dims)-glass_edge_distance(uv-dy,dims));
                if (length(gradient)>0.001 && length(gradient)<1e5) { edge_direction=-normalize(gradient); }
            }
            edge_t=clamp(edge_distance/max(glass_settings.edge.x,0.001),0.0,1.0);
            if (edge_t<1.0 && glass_settings.edge.y>0.0) {
                let curve=glass_profile(edge_t);
                let bevel_normal=normalize(vec3<f32>(edge_direction*curve.y,-1.0));
                let bevel_ray=refract(vec3<f32>(0.0,0.0,1.0),bevel_normal,1.0/glass_settings.optical.x);
                let height=curve.x*glass_settings.edge.x+glass_settings.optical.y;
                displacement-=bevel_ray.xy/max(abs(bevel_ray.z),0.05)*height*glass_settings.edge.y/dims;
            }
        }
        if (glass_settings.appearance.w!=0.0) {
            let left=glass_span(uv,vec2<f32>(-1.0,0.0),dims); let right=glass_span(uv,vec2<f32>(1.0,0.0),dims);
            let top=glass_span(uv,vec2<f32>(0.0,-1.0),dims); let bottom=glass_span(uv,vec2<f32>(0.0,1.0),dims);
            let center=uv+vec2<f32>(right-left,bottom-top)*0.5/dims;
            displacement+=(center-uv)*glass_settings.appearance.w*0.5;
        }
    }
    let pixels=length(displacement*dims);
    displacement*=min(1.0,glass_settings.edge.w/max(pixels,0.0001));
    // Smoothly suppress offsets near foreground silhouettes, independently of
    // the real pane outline used for bevels/rims. No highlight follows actors.
    let recovered_background=glass_settings.highlights.w>0.5
        && textureLoad(glass_background_scene,vec2<i32>(fragment.xy),0).a>surface_depth+0.5;
    if (glass_settings.environment.w>0.0 && nearest.z>=0.0 && !recovered_background) {
        let foreground_distance=length((uv-nearest.zw)*dims);
        let guarded=smoothstep(0.0,max(length(displacement*dims)*(1.0+glass_settings.effects.x*glass_settings.highlights.z)+2.0,3.0),foreground_distance);
        displacement*=mix(1.0,guarded,glass_settings.environment.w);
    }
    let screen_edge=smoothstep(0.0,0.02,min(min(uv.x,uv.y),min(1.0-uv.x,1.0-uv.y)));
    displacement*=screen_edge*smoothstep(0.0,2.0,gap);
    let sample_uv=glass_clamp_uv(uv+displacement);
    let corrected=glass_settings.effects.y*clamp(glass_settings.optical.x*2.0-2.0,0.0,1.0);
    let rough_radius=corrected*corrected*1600.0/max(surface_depth,32.0);
    let progressive=(1.0-edge_t)*glass_settings.detail.x*select(1.0,abs(edge_direction.y),glass_settings.detail.y>0.5);
    let radius=min(glass_settings.optical.w+rough_radius+progressive,64.0);
    var transmitted=glass_blurred(sample_uv,uv,surface_depth,radius);
    if (glass_settings.effects.x>0.00001) {
        let dispersion=displacement*glass_settings.effects.x*glass_settings.highlights.z;
        let red=glass_blurred(sample_uv-dispersion,uv,surface_depth,radius).r;
        let blue=glass_blurred(sample_uv+dispersion,uv,surface_depth,radius).b;
        transmitted=vec3<f32>(red,transmitted.g,blue);
    }
    let peak=max(max(authored_tint.r,authored_tint.g),authored_tint.b);
    let chroma=authored_tint/max(peak,0.001);
    transmitted*=mix(vec3<f32>(1.0),chroma*glass_settings.color.rgb,glass_settings.color.a);
    transmitted=mix(transmitted,glass_settings.color.rgb,glass_settings.appearance.y);
    let physical_f0=pow((glass_settings.optical.x-1.0)/(glass_settings.optical.x+1.0),2.0);
    let f0=mix(physical_f0,clamp(authored_f0,0.0,0.18),0.25);
    let facing=clamp(dot(n,v),0.0,1.0);
    let fresnel=clamp((f0+(1.0-f0)*pow(1.0-facing,5.0))*glass_settings.effects.z,0.0,1.0);
    var result=mix(transmitted,max(glass_settings.environment.rgb,vec3<f32>(0.08,0.10,0.12)),fresnel);
    if (glass_settings.rim.x>0.0 && edge_distance<1e5) {
        let light=vec2<f32>(cos(glass_settings.rim.w),sin(glass_settings.rim.w));
        let lobe=pow(abs(dot(edge_direction,light)),4.0);
        let rim=1.0-smoothstep(glass_settings.rim.y,glass_settings.rim.y+glass_settings.rim.z,edge_distance);
        let intensity=clamp(rim*lobe*glass_settings.rim.x,0.0,1.0);
        if (glass_settings.highlights.y<0.5) {
            let luminance=dot(result,vec3<f32>(0.2126,0.7152,0.0722));
            let saturated=max(mix(vec3<f32>(luminance),result,glass_settings.highlights.x),vec3<f32>(0.0));
            result=mix(result,saturated,intensity);
        }
        result=mix(result,vec3<f32>(1.0),intensity);
    }
    return vec4<f32>(mix(clear,result,glass_settings.appearance.z),1.0);
}

// TaystJK rd-vanilla/tr_shadows.cpp: RB_CaptureScreenImage / RB_DistortionFill.
// The silhouette masks two screen stretches; neither normals nor skin colour
// affect the result. Reuse the glass capture, including the original centred
// power-of-two crop (which is visibly different from sampling the whole screen).
fn cloak_shade(fragment: vec4<f32>, time: f32) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(glass_scene));
    let uv = fragment.xy / dims;
    let crop = exp2(floor(log2(min(dims, vec2<f32>(2048.0)))));
    let start = floor((dims - crop) * 0.5);
    let stretch0 = abs(sin(time * 0.5)) * vec2<f32>(0.08, 0.2);
    let stretch1 = abs(sin(time * 0.8)) * vec2<f32>(0.2, 0.08);
    let uv0 = (start + (stretch0 + uv * (1.0 - 2.0 * stretch0)) * crop) / dims;
    let uv1 = (start + (stretch1 + uv * (1.0 - 2.0 * stretch1)) * crop) / dims;
    let first = textureSampleLevel(glass_scene, glass_sampler, glass_clamp_uv(uv0), 0.0).rgb;
    let second = textureSampleLevel(glass_scene, glass_sampler, glass_clamp_uv(uv1), 0.0).rgb;
    // The second quad uses SRC_ALPHA,SRC_ALPHA with alpha 0.5.
    return vec4<f32>((first + second) * 0.5, 1.0);
}

// RF_DISTORTION captures a square around the Force half-shield's origin, then
// maps that image with the model UVs. The shader returns a precomposited colour
// because the shared Glass pipeline replaces the destination.
fn force_refraction_shade(fragment: vec4<f32>, st: vec2<f32>, origin: vec3<f32>,
    view_proj: mat4x4<f32>, eye: vec3<f32>, alpha: f32) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(glass_scene));
    let distance = length(origin - eye);
    var radius = 32.0;
    if (distance < 128.0) { radius = 256.0; }
    else if (distance < 256.0) { radius = 128.0; }
    else if (distance < 512.0) { radius = 64.0; }
    let clip = view_proj * vec4<f32>(origin, 1.0);
    let centre = (clip.xy / max(clip.w, 0.0001) * vec2<f32>(0.5, -0.5) + 0.5) * dims;
    let size = min(vec2<f32>(radius), dims);
    let start = clamp(floor(centre - size * 0.5), vec2<f32>(0.0), dims - size);
    // GL screen-copy texture has its V origin at the bottom of the capture.
    let uv = (start + vec2<f32>(fract(st.x), 1.0-fract(st.y)) * size) / dims;
    let refracted = textureSampleLevel(glass_scene, glass_sampler, glass_clamp_uv(uv), 0.0).rgb;
    let background = textureSampleLevel(glass_scene, glass_sampler, glass_clamp_uv(fragment.xy/dims), 0.0).rgb;
    return vec4<f32>(mix(background, refracted, alpha), 1.0);
}
