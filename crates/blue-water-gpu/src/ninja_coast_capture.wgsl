#import naval_ocean::waves::{waves, displacement}
#import naval_ocean::underwater_ray::screen_ray
// Coast variant of ninja_ocean_capture.wgsl: same crest/contact/probe
// passes, heights from the private Coast WPO (terrain-aligned surface).
struct CaptureParams {
    domain: vec4<f32>, // centre xz, extent, absolute plane m
    eye_time: vec4<f32>, // relative eye x/y/z and material game time
    config: vec4<f32>, // FFT on, contact count, contact spacing, swell travel render x
    frame: vec4<f32>, // RVT clamp (UE cm), solver SimZ datum (UE cm), flat mesh Z (UE cm), swell travel render z
};
@group(0) @binding(0) var<uniform> capture: CaptureParams;
@group(0) @binding(1) var output_crest: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<storage,read> contact_points: array<vec4<f32>>;
@group(0) @binding(3) var<storage,read_write> contact_flags: array<u32>;
@group(0) @binding(4) var previous_vd: texture_2d<f32>;
@group(0) @binding(5) var previous_pd: texture_2d<f32>;
@group(0) @binding(6) var previous_painter: texture_2d<f32>;
@group(0) @binding(7) var terrain: texture_2d<f32>;
@group(0) @binding(8) var tile1: texture_2d<f32>;
@group(0) @binding(9) var tile2: texture_2d<f32>;
@group(0) @binding(10) var clamp_sampler: sampler;
@group(0) @binding(11) var repeat_sampler: sampler;
fn capture_uv(xz:vec2<f32>) -> vec2<f32> {
    let d=(xz-capture.domain.xy)/capture.domain.z;
    return vec2<f32>(d.x,-d.y)+0.5;
}
fn capture_cm(p:vec3<f32>)->vec3<f32> {
    return vec3<f32>(p.x*100.0,-p.z*100.0,p.y*100.0+capture.frame.x);
}
fn capture_macro(grid:vec2<f32>,spacing:f32)->vec3<f32> {
    if capture.config.x < 0.5 { return vec3<f32>(0.0); }
    let d=displacement(grid,spacing);
    let scale=jka_crest_scale_from_bounds(jka_crest_bounds,1.0,0.0,3.0);
    return vec3<f32>(d.x*scale,d.y,d.z*scale)*capture_wet(capture_uv(grid));
}
// Same dry-bed FFT attenuation as ninja_coast_surface.wgsl.
fn capture_wet(uv:vec2<f32>)->f32 {
    let share=1.0-smoothstep(0.45,0.5,max(abs(uv.x-0.5),abs(uv.y-0.5)));
    let bed=textureSampleLevel(terrain,clamp_sampler,uv,0.0);
    return mix(1.0,select(1.0,smoothstep(0.0,100.0,-bed.x),bed.w>0.5),share);
}
// Share the FFT displacements needed by the central-difference Jacobian
// across each 8x8 output tile. The 1-cell halo preserves both +/- samples.
// Original: five displacement() evaluations per output texel.
// Tiled: 100 displacement() evaluations per 64 output texels, at the same
// spacing and with the same central-difference / spilling equations.
// Workgroup f32 values avoid intermediate f16 texture quantization.
var<workgroup> crest_stencil: array<vec3<f32>, 100>;
@compute @workgroup_size(8,8)
fn crest(
    @builtin(global_invocation_id) id:vec3<u32>,
    @builtin(local_invocation_index) lane:u32,
    @builtin(workgroup_id) tile:vec3<u32>,
) {
    let dims=textureDimensions(output_crest);
    let h=capture.domain.z/f32(max(dims.x,dims.y));
    if capture.config.x > 0.5 {
        // All 64 lanes participate, including at a partial boundary tile.
        // This is required before the workgroup barrier below.
        for (var n=lane; n<100u; n+=64u) {
            let local=vec2<i32>(i32(n%10u)-1,i32(n/10u)-1);
            let sample_pixel=vec2<i32>(tile.xy*8u)+local;
            let uv=(vec2<f32>(sample_pixel)+0.5)/vec2<f32>(dims)-0.5;
            let xz=capture.domain.xy+vec2<f32>(uv.x,-uv.y)*capture.domain.z;
            crest_stencil[n]=displacement(xz,h);
        }
    }
    workgroupBarrier();
    if any(id.xy>=dims) {return;}
    var density=0.0;
    if capture.config.x > 0.5 {
        let cell=(id.y%8u+1u)*10u+(id.x%8u+1u);
        let center=crest_stencil[cell];
        let a=(crest_stencil[cell+1u].xz-crest_stencil[cell-1u].xz)/(2.0*h);
        // Texture V increases toward negative world Z; reverse the Y texel delta.
        let b=(crest_stencil[cell-10u].xz-crest_stencil[cell+10u].xz)/(2.0*h);
        let jac=(1.0+a.x)*(1.0+b.y)-a.y*b.x;
        density=jka_crest_spilling_cue(jac,center.y,waves.sea.x)*capture_wet((vec2<f32>(id.xy)+0.5)/vec2<f32>(dims));
    }
    textureStore(output_crest,vec2<i32>(id.xy),vec4<f32>(0.0,0.0,density,0.0));
}

fn capture_height(world_xz:vec2<f32>)->vec2<f32> {
    let spacing=capture.config.z;
    // Invert the limited continuous horizontal map. Contact is evaluated on
    // the previous published FNL pressure (the current tick consumes flags).
    var grid=world_xz;
    for(var i=0;i<12;i++) {grid=mix(grid,world_xz-capture_macro(grid,spacing).xz,0.6);}
    let d=capture_macro(grid,spacing);
    let w=vec3<f32>(grid.x+d.x,d.y,grid.y+d.z);
    let uv=capture_uv(w.xz);
    let share=1.0-smoothstep(0.45,0.5,max(abs(uv.x-0.5),abs(uv.y-0.5)));
    let f=NinjaCoastFrame(capture.frame.x,capture.frame.y,ninja_coast_swell_rotation());
    let mesh=vec3<f32>(w.x*100.0,-w.z*100.0,capture.frame.z);
    let wpo=ninja_coast_wpo_cm(previous_vd,previous_pd,previous_painter,terrain,tile1,tile2,clamp_sampler,
        repeat_sampler,uv,mesh,capture_cm(capture.eye_time.xyz),capture.eye_time.w,f,true);
    // Horizontal WPO is not inverted (as Ocean2); vertical uses the same share as the draw.
    return vec2<f32>(d.y+(mesh.z+wpo.z-f.clamp_cm)*0.01*share,length(w.xz-world_xz));
}
@compute @workgroup_size(64)
fn contacts(@builtin(global_invocation_id) id:vec3<u32>) {
    if id.x>=u32(capture.config.y) {return;}
    let p=contact_points[id.x];
    let h=capture_height(p.xy);
    contact_flags[id.x]=select(0u,1u,p.z<=h.x && h.x<=p.w && h.y<=0.2);
}
@group(3) @binding(0) var<storage,read_write> eye_result:array<vec4<f32>,2>;
@compute @workgroup_size(1)
fn eye_height() {
    let h=capture_height(capture.eye_time.xz);
    eye_result[1]=vec4<f32>(h,1.0,0.0);
}
struct NinjaProbeParams {
    inverse_projection:mat4x4<f32>, camera_world:mat4x4<f32>, camera:vec4<f32>,
};
// Separate binding numbers from eye result, separate entry-point layouts.
@group(3) @binding(1) var<uniform> water_probe:NinjaProbeParams;
@group(3) @binding(2) var probe_out:texture_storage_2d<rgba16float,write>;
@group(3) @binding(3) var<storage, read_write> probe_eye: array<vec4<f32>>;
// All near-plane pixels have the same camera eye. Compute its inverse FFT
// displacement once, then reuse the exact result for every 128x72 pixel.
@compute @workgroup_size(1)
fn underwater_eye_height() {
    let h = capture_height(water_probe.camera.xz);
    probe_eye[0] = vec4<f32>(h, 0.0, 0.0);
}
@compute @workgroup_size(8,8)
fn underwater_probe(@builtin(global_invocation_id) id:vec3<u32>) {
    let size=textureDimensions(probe_out);
    if any(id.xy>=size) {return;}
    if water_probe.camera.w<=0.0 {
        textureStore(probe_out,id.xy,vec4<f32>(-1.0,-1.0,0.0,1.0));return;
    }
    let ray=screen_ray((vec2<f32>(id.xy)+0.5)/vec2<f32>(size),water_probe.inverse_projection,water_probe.camera_world,water_probe.camera.w);
    let eye=water_probe.camera.xyz;
    let h=capture_height(ray.start.xz);
    let e=probe_eye[0].xy;
    let depth=select(-1.0,h.x-ray.start.y,h.y<=0.2 && jka_water_column_contains(ray.start));
    let eye_depth=select(-1.0,e.x-eye.y,e.y<=0.2 && jka_water_column_contains(eye));
    textureStore(probe_out,id.xy,vec4<f32>(depth,eye_depth,0.0,1.0));
}

// Rotation (cos, sin) taking UE's authored tile-1 travel direction, (1, -1)/sqrt2
// in UE XY (the pan (-.03, .03) moves features along -pan), onto the authored
// swell (r_blueWaterDirection via the host uniform: render (x, z) travel
// vector; UE Y = -render Z). waves.wind is not stepped in r_hybridMaterial 5.
fn ninja_coast_swell_rotation() -> vec2<f32> {
    let travel = vec2<f32>(capture.config.w, capture.frame.w);
    let w = vec2<f32>(travel.x, -travel.y);
    if dot(w, w) < 1e-8 { return vec2<f32>(1.0, 0.0); }
    let s = normalize(w);
    let r = vec2<f32>(0.70710678, -0.70710678);
    return vec2<f32>(dot(r, s), r.x * s.y - r.y * s.x);
}
