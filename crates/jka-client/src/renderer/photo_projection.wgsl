// Local cube coordinates: +X right, +Y up, +Z forward.
const PI: f32 = 3.141592653589793;
const SAMPLES: u32 = SAMPLES_VALUE;
// One pass may cover a strip/segment of a larger virtual output. Samples are
// placed from GLOBAL pixel coordinates, never from local derivatives, so a
// strip is identical to the same pixels of a full-image pass.
struct Params { origin: vec2f, full: vec2f, mode: u32, _pad0: u32, _pad1: u32, _pad2: u32 };
@group(0) @binding(0) var cube: texture_cube<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> params: Params;
@vertex fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    let uv = vec2f(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4f(uv.x*2.0-1.0, 1.0-uv.y*2.0, 0.0, 1.0);
}
fn face_direction(face: u32, uv: vec2f) -> vec3f {
    let p = uv*2.0-1.0;
    switch face {
        case 0u: { return vec3f(1.0,-p.y,-p.x); }
        case 1u: { return vec3f(-1.0,-p.y,p.x); }
        case 2u: { return vec3f(p.x,1.0,p.y); }
        case 3u: { return vec3f(p.x,-1.0,-p.y); }
        case 4u: { return vec3f(p.x,-p.y,1.0); }
        default: { return vec3f(-p.x,-p.y,-1.0); }
    }
}
fn sample_projection(uv: vec2f) -> vec4f {
    let mode = params.mode;
    var direction = vec3f(0.0,0.0,1.0);
    if mode == 0u || mode == 5u {
        let lon = (uv.x-0.5)*2.0*PI;
        if mode == 0u {
            let lat = (0.5-uv.y)*PI;
            direction = vec3f(sin(lon)*cos(lat),sin(lat),cos(lon)*cos(lat));
        } else {
            direction = normalize(vec3f(sin(lon),1.0-2.0*uv.y,cos(lon)));
        }
    } else if mode == 1u {
        let grid = uv*vec2f(4.0,3.0);
        let cell = vec2u(floor(grid));
        var face = 6u;
        if cell.y == 1u {
            switch cell.x { case 0u: {face=1u;} case 1u: {face=4u;} case 2u: {face=0u;} default: {face=5u;} }
        } else if cell.x == 1u && cell.y == 0u {face=2u;}
        else if cell.x == 1u && cell.y == 2u {face=3u;}
        if face == 6u {return vec4f(0.0,0.0,0.0,1.0);}
        direction = face_direction(face,fract(grid));
    } else if mode == 3u || mode == 4u {
        // Stereographic plane centered on the nadir/zenith; horizon at
        // half the image radius, with finite cropping near opposite pole.
        let p = (uv*2.0-1.0)*2.0;
        let r2 = dot(p,p);
        let sign_y = select(-1.0,1.0,mode==4u);
        direction = vec3f(2.0*p.x,sign_y*(1.0-r2),-2.0*p.y)/(1.0+r2);
    } else if mode == 6u {
        let back = uv.x >= 0.5;
        let p = vec2f(fract(uv.x*2.0)*2.0-1.0,1.0-2.0*uv.y);
        let radius = length(p);
        if radius > 1.0 {return vec4f(0.0,0.0,0.0,1.0);}
        let theta = radius*PI*0.5;
        let radial = p/max(radius,0.000001)*sin(theta);
        direction = vec3f(select(radial.x,-radial.x,back),radial.y,select(cos(theta),-cos(theta),back));
    } else {
        direction = face_direction(mode-7u,uv);
    }
    return vec4f(textureSampleLevel(cube,samp,direction,0.0).rgb,1.0);
}

@fragment fn fs_main(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    // Integer global pixel; subsample centers (s+0.5)/SAMPLES inside it.
    let pixel = params.origin + floor(pos.xy);
    var color = vec4f(0.0);
    for (var y = 0u; y < SAMPLES; y++) {
        for (var x = 0u; x < SAMPLES; x++) {
            let offset = (vec2f(f32(x),f32(y))+0.5)/f32(SAMPLES);
            color += sample_projection((pixel+offset)/params.full);
        }
    }
    return color/f32(SAMPLES*SAMPLES);
}
