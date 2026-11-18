// JKA screen-space scene transmission through the upstream wave surface.
// The preserved upstream shader supplies its FFT normals, water IOR, optical
// attenuation, Fresnel and refracted-sky fallback. Only finite scene hits are added.
fn jka_refraction_depth(uv:vec2<f32>)->f32 {
    let size=vec2<i32>(textureDimensions(scene_depth));
    return textureLoad(scene_depth,clamp(vec2<i32>(uv*vec2<f32>(size)),vec2<i32>(0),size-1),0);
}
fn jka_refraction_uv(point:vec3<f32>)->vec3<f32> {
    let clip=world_to_clip(point);
    let ndc=clip.xy/max(clip.w,1e-6);
    return vec3<f32>(ndc.x*0.5+0.5,0.5-ndc.y*0.5,clip.w);
}
fn jka_refracted_scene(origin:vec3<f32>,incident:vec3<f32>,normal:vec3<f32>,eta:f32,reach:f32,below:bool)->vec4<f32> {
    if surface.reflections.w < 0.5 || surface.reflections.z <= 0.0 {
        return vec4<f32>(0.0,0.0,0.0,-1.0);
    }
    if dot(incident,normal)>=-1e-4 || -(camera.view*vec4<f32>(curved(origin),1.0)).z>=camera.far*0.999 {
        return vec4<f32>(0.0,0.0,0.0,-1.0);
    }
    if surface.reflections.w < 1.5 {
        // Godot-style screen offset: a normal-projected warp and one receiver
        // depth rejection, rather than the 24-step physical trace below.
        let size = vec2<f32>(textureDimensions(scene_depth));
        let uv = jka_refraction_uv(origin).xy;
        let edge = smoothstep(0.0, 0.06, min(min(uv.x, uv.y), min(1.0-uv.x, 1.0-uv.y)));
        let vertical = vec3<f32>(0.0, select(-1.0, 1.0, below), 0.0);
        let tilt = (camera.view_projection * vec4<f32>(normal-vertical, 0.0)).xy * vec2<f32>(1.0,-1.0);
        let column = select(reach, reach*0.5, below);
        let offset = clamp(tilt, vec2<f32>(-1.0), vec2<f32>(1.0)) * surface.reflections.z * edge * min(column/1.6, 1.0);
        var bent_uv = clamp(uv+offset, 0.5/size, 1.0-0.5/size);
        let depth = jka_refraction_depth(bent_uv);
        let receiver = camera.near*camera.far/(depth*(camera.far-camera.near)+camera.near);
        let surface_depth = -(camera.view*vec4<f32>(curved(origin),1.0)).z;
        if depth > 0.0 && receiver <= surface_depth+0.025 { bent_uv = uv; }
        return vec4<f32>(textureSampleLevel(scene_color,wake_sampler,bent_uv,0.0).rgb,column);
    }
    let bend=refract(incident,normal,eta);
    if dot(bend,bend)<1e-6 {return vec4<f32>(0.0,0.0,0.0,-1.0);}
    let direction=normalize(mix(incident,bend,clamp(surface.reflections.z/0.1,0.0,1.0)));
    let limit=min(max(reach,2.0),min(camera.far,100.0));
    var previous=0.02;
    for(var i=1u;i<=24u;i+=1u) {
        let fraction=f32(i)/24.0;
        let along=max(fraction*fraction*limit,0.03);
        let point=origin+direction*along;
        let projected=jka_refraction_uv(point);
        if projected.z<=0.0 || projected.z>=camera.far || any(projected.xy<vec2<f32>(0.002)) || any(projected.xy>vec2<f32>(0.998)) {break;}
        let depth=jka_refraction_depth(projected.xy);
        let scene=camera.near*camera.far/(depth*(camera.far-camera.near)+camera.near);
        let ray=-(camera.view*vec4<f32>(curved(point),1.0)).z;
        if depth>0.0 && ray>=scene {
            var low=previous;var high=along;
            for(var step=0u;step<5u;step+=1u) {
                let middle=(low+high)*0.5;
                let probe=origin+direction*middle;
                let screen=jka_refraction_uv(probe);
                let d=jka_refraction_depth(screen.xy);
                let receiver=camera.near*camera.far/(d*(camera.far-camera.near)+camera.near);
                let probe_depth=-(camera.view*vec4<f32>(curved(probe),1.0)).z;
                if d>0.0 && probe_depth>=receiver {high=middle;} else {low=middle;}
            }
            let hit=origin+direction*high;
            let screen=jka_refraction_uv(hit);
            let d=jka_refraction_depth(screen.xy);
            let receiver=camera.near*camera.far/(d*(camera.far-camera.near)+camera.near);
            let actual=-(camera.view*vec4<f32>(curved(hit),1.0)).z;
            // Reject foreground silhouettes and rays that jump across a depth discontinuity.
            if abs(actual-receiver)>max(0.15,receiver*0.005) {return vec4<f32>(0.0,0.0,0.0,-1.0);}
            let ndc=vec4<f32>(screen.x*2.0-1.0,1.0-screen.y*2.0,d,1.0);
            let unprojected=camera.inverse_view_projection*ndc;
            var world=unprojected.xyz/unprojected.w;
            let across=world.xz-camera.position.xz;
            world.y+=camera.curvature*dot(across,across);
            if below && (!jka_water_contains(world.xz) || world.y>origin.y+0.1) {return vec4<f32>(0.0,0.0,0.0,-1.0);}
            if !below && world.y<origin.y-0.1 {return vec4<f32>(0.0,0.0,0.0,-1.0);}
            // Keep the original source pixel/sky fallback at the screen boundary.
            let edge=min(min(screen.x,screen.y),min(1.0-screen.x,1.0-screen.y));
            if edge<0.02 {return vec4<f32>(0.0,0.0,0.0,-1.0);}
            return vec4<f32>(textureSampleLevel(scene_color,wake_sampler,screen.xy,0.0).rgb,distance(origin,world));
        }
        previous=along;
    }
    return vec4<f32>(0.0,0.0,0.0,-1.0);
}

// r_underwaterReflections (surface.shadows.y, unused upstream): seen from
// below, the surface mirrors the submerged scene: totally past Snell's window,
// by the water-to-air Fresnel inside it. Traced in the same opaque scene copy
// as the transmission; rgb the mirrored scene, a its weight (Fresnel times hit
// confidence; 0 when off or missed, keeping upstream's mirrored water light).
// `up` is the surface normal toward the air; the camera is below it.
fn jka_underwater_reflection(origin:vec3<f32>,view:vec3<f32>,up:vec3<f32>)->vec4<f32> {
    if surface.shadows.y < 0.5 {return vec4<f32>(0.0);}
    let facing=clamp(abs(dot(view,up)),0.00002,1.0);
    var fresnel=0.02+0.98*pow(1.0-facing,5.0);
    if 1.333*1.333*(1.0-facing*facing)>1.0 {fresnel=1.0;}
    let direction=reflect(-view,up);
    // Only rays back down into the water reach the submerged scene.
    if direction.y>-1e-3 || -(camera.view*vec4<f32>(curved(origin),1.0)).z>=camera.far*0.999 {return vec4<f32>(0.0);}
    let limit=min(camera.far,100.0);
    var previous=0.02;
    for(var i=1u;i<=24u;i+=1u) {
        let fraction=f32(i)/24.0;
        let along=max(fraction*fraction*limit,0.03);
        let point=origin+direction*along;
        let projected=jka_refraction_uv(point);
        if projected.z<=0.0 || projected.z>=camera.far || any(projected.xy<vec2<f32>(0.002)) || any(projected.xy>vec2<f32>(0.998)) {break;}
        let depth=jka_refraction_depth(projected.xy);
        let scene=camera.near*camera.far/(depth*(camera.far-camera.near)+camera.near);
        let ray=-(camera.view*vec4<f32>(curved(point),1.0)).z;
        if depth>0.0 && ray>=scene {
            var low=previous;var high=along;
            for(var step=0u;step<5u;step+=1u) {
                let middle=(low+high)*0.5;
                let probe=origin+direction*middle;
                let screen=jka_refraction_uv(probe);
                let d=jka_refraction_depth(screen.xy);
                let receiver=camera.near*camera.far/(d*(camera.far-camera.near)+camera.near);
                let probe_depth=-(camera.view*vec4<f32>(curved(probe),1.0)).z;
                if d>0.0 && probe_depth>=receiver {high=middle;} else {low=middle;}
            }
            let screen=jka_refraction_uv(origin+direction*high);
            let d=jka_refraction_depth(screen.xy);
            let receiver=camera.near*camera.far/(d*(camera.far-camera.near)+camera.near);
            let actual=-(camera.view*vec4<f32>(curved(origin+direction*high),1.0)).z;
            // Reject foreground silhouettes and rays that jump across a depth discontinuity.
            if abs(actual-receiver)>max(0.15,receiver*0.005) {return vec4<f32>(0.0);}
            let edge=min(min(screen.x,screen.y),min(1.0-screen.x,1.0-screen.y));
            let confidence=smoothstep(0.02,0.08,edge);
            return vec4<f32>(textureSampleLevel(scene_color,wake_sampler,screen.xy,0.0).rgb,fresnel*confidence);
        }
        previous=along;
    }
    return vec4<f32>(0.0);
}
