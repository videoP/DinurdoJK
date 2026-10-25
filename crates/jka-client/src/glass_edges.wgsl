@group(0) @binding(0) var mask: texture_2d<f32>;
@group(0) @binding(1) var previous: texture_2d<f32>;
@group(0) @binding(2) var next: texture_storage_2d<rgba16float,write>;
@group(0) @binding(3) var<uniform> params: vec4<u32>;
// glass_bounds.wgsl result; [1] is the half-res region [x0, y0, x1, y1).
@group(0) @binding(5) var<uniform> bounds: array<vec4<u32>, 4>;
fn region_pixel(gid: vec3<u32>) -> vec2<u32> { return bounds[1].xy + gid.xy; }
fn outside_region(p: vec2<u32>) -> bool { return any(p >= min(bounds[1].zw, textureDimensions(next))); }
fn mask_at(p: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(next));
    let full = vec2<i32>(textureDimensions(mask));
    let uv = (vec2<f32>(clamp(p,vec2<i32>(0),size-1)) + 0.5) / vec2<f32>(size);
    return textureLoad(mask,clamp(vec2<i32>(uv*vec2<f32>(full)),vec2<i32>(0),full-1),0);
}
fn normal_of(value: vec4<f32>) -> vec3<f32> {
    let oct = value.xy / max(value.w,0.001);
    var n = vec3<f32>(oct,1.0-abs(oct.x)-abs(oct.y));
    if (n.z < 0.0) { n = vec3<f32>((1.0-abs(n.yx))*select(vec2<f32>(-1.0),vec2<f32>(1.0),n.xy>=vec2<f32>(0.0)),n.z); }
    return normalize(n);
}
@group(0) @binding(4) var scene_depth: texture_2d<f32>;
fn foreground_at(p: vec2<i32>) -> bool {
    let value=mask_at(p);
    let size=vec2<i32>(textureDimensions(next));
    let full=vec2<i32>(textureDimensions(scene_depth));
    let uv=(vec2<f32>(clamp(p,vec2<i32>(0),size-1))+0.5)/vec2<f32>(size);
    let depth=textureLoad(scene_depth,clamp(vec2<i32>(uv*vec2<f32>(full)),vec2<i32>(0),full-1),0).r;
    return value.w>0.25 && depth<=value.z/max(value.w,0.001)+0.5;
}
@compute @workgroup_size(8,8)
fn seed_edges(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(next);
    let pixel = region_pixel(gid);
    if (outside_region(pixel)) { return; }
    let p = vec2<i32>(pixel);
    let center = mask_at(p);
    let inside = center.w > 0.25;
    let foreground=foreground_at(p);
    var boundary = false;
    var foreground_boundary=false;
    let offsets = array<vec2<i32>,4>(vec2<i32>(1,0),vec2<i32>(-1,0),vec2<i32>(0,1),vec2<i32>(0,-1));
    for (var i=0u;i<4u;i+=1u) {
        let other = mask_at(p+offsets[i]);
        boundary = boundary || (inside != (other.w>0.25));
        if (inside && other.w>0.25) {
            boundary = boundary || dot(normal_of(center),normal_of(other))<0.9;
            foreground_boundary=foreground_boundary || foreground!=foreground_at(p+offsets[i]);
        }
    }
    let uv = (vec2<f32>(pixel)+0.5)/vec2<f32>(size);
    textureStore(next,p,vec4<f32>(select(vec2<f32>(-1.0),uv,boundary),select(vec2<f32>(-1.0),uv,foreground_boundary)));
}
@compute @workgroup_size(8,8)
fn jump_edges(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(next);
    let pixel = region_pixel(gid);
    if (outside_region(pixel)) { return; }
    let p = vec2<i32>(pixel);
    let uv = (vec2<f32>(pixel)+0.5)/vec2<f32>(size);
    var closest = textureLoad(previous,p,0);
    var best = vec2<f32>(1e20);
    if (closest.x>=0.0) { let delta=(uv-closest.xy)*vec2<f32>(params.yz); best.x=dot(delta,delta); }
    if (closest.z>=0.0) { let delta=(uv-closest.zw)*vec2<f32>(params.yz); best.y=dot(delta,delta); }
    for (var y=-1;y<=1;y+=1) { for (var x=-1;x<=1;x+=1) {
        let q=p+vec2<i32>(x,y)*i32(params.x);
        if (any(q<vec2<i32>(0)) || any(q>=vec2<i32>(size))) { continue; }
        let candidate=textureLoad(previous,q,0);
        if (candidate.x>=0.0) {
            let delta=(uv-candidate.xy)*vec2<f32>(params.yz); let distance=dot(delta,delta);
            if (distance<best.x) { best.x=distance; closest=vec4<f32>(candidate.xy,closest.zw); }
        }
        if (candidate.z>=0.0) {
            let delta=(uv-candidate.zw)*vec2<f32>(params.yz); let distance=dot(delta,delta);
            if (distance<best.y) { best.y=distance; closest=vec4<f32>(closest.xy,candidate.zw); }
        }
    } }
    textureStore(next,p,closest);
}
