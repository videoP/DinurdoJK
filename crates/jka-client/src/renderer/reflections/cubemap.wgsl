// Ports of TaystJK rend2 prefilterEnvMap.glsl and R_CreateEnvBrdfLUT /
// GSmithCorrelated (tr_image.cpp / tr_extramath.cpp). Six render passes replace
// the OpenGL geometry shader. Cube coordinates remain JKA/OpenGL coordinates.
struct FilterParams { size_face_roughness_luma: vec4<f32> };
@group(0) @binding(0) var environment: texture_cube<f32>;
@group(0) @binding(1) var environment_sampler: sampler;
@group(0) @binding(2) var<uniform> params: FilterParams;

@vertex fn vs_fullscreen(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(f32((index << 1u) & 2u) * 2.0 - 1.0, f32(index & 2u) * 2.0 - 1.0, 0.0, 1.0);
}

fn cube_direction(face: u32, p: vec2<f32>) -> vec3<f32> {
    switch face {
        case 0u: { return normalize(vec3(1.0, -p.y, -p.x)); }
        case 1u: { return normalize(vec3(-1.0, -p.y, p.x)); }
        case 2u: { return normalize(vec3(p.x, 1.0, p.y)); }
        case 3u: { return normalize(vec3(p.x, -1.0, -p.y)); }
        case 4u: { return normalize(vec3(p.x, -p.y, 1.0)); }
        default: { return normalize(vec3(-p.x, -p.y, -1.0)); }
    }
}

fn hammersley(i: u32, count: u32) -> vec2<f32> {
    return vec2(f32(i) / f32(count), f32(reverseBits(i)) * 2.3283064365386963e-10);
}

fn importance_sample_ggx(xi: vec2<f32>, roughness: f32, n: vec3<f32>) -> vec3<f32> {
    let a = roughness * roughness;
    let phi = 6.28318530718 * xi.x;
    let cos_theta = sqrt((1.0 - xi.y) / (1.0 + (a * a - 1.0) * xi.y));
    let sin_theta = sqrt(max(1.0 - cos_theta * cos_theta, 0.0));
    let h = vec3(sin_theta * cos(phi), sin_theta * sin(phi), cos_theta);
    let up = select(vec3(1.0, 0.0, 0.0), vec3(0.0, 0.0, 1.0), abs(n.z) < 0.999);
    let tangent = normalize(cross(up, n));
    return tangent * h.x + cross(n, tangent) * h.y + n * h.z;
}

@fragment fn fs_prefilter(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let p = position.xy / params.size_face_roughness_luma.x * 2.0 - vec2(1.0);
    let n = cube_direction(u32(params.size_face_roughness_luma.y), p);
    let roughness = params.size_face_roughness_luma.z;
    if (roughness == 0.0) {
        return vec4(textureSampleLevel(environment, environment_sampler, n, 0.0).rgb, params.size_face_roughness_luma.w);
    }
    var color = vec3(0.0);
    var weight = 0.0;
    let resolution = f32(textureDimensions(environment).x);
    let sa_texel = 12.56637061436 / (6.0 * resolution * resolution);
    for (var i = 0u; i < 256u; i += 1u) {
        let h = importance_sample_ggx(hammersley(i, 256u), roughness, n);
        let l = 2.0 * dot(n, h) * h - n;
        let nl = clamp(dot(n, l), 0.0, 1.0);
        if (nl > 0.0) {
            let nh = max(dot(n, h), 0.0);
            let hv = max(dot(h, n), 0.0);
            let a2 = roughness * roughness;
            let d = (nh * a2 - nh) * nh + 1.0;
            let distribution = a2 / max(3.14159265359 * d * d, 1e-8);
            let pdf = distribution * nh / max(4.0 * hv, 1e-6) + 0.0001;
            let sa_sample = 1.0 / (256.0 * pdf + 0.0001);
            let mip = 0.5 * log2(sa_sample / sa_texel);
            color += textureSampleLevel(environment, environment_sampler, l, mip).rgb * nl;
            weight += nl;
        }
    }
    return vec4(color / max(weight, 1e-6), params.size_face_roughness_luma.w);
}

// Ordinary capture mip chain, used by the reference's solid-angle sampling.
@fragment fn fs_downsample(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let size = params.size_face_roughness_luma.x;
    let face = u32(params.size_face_roughness_luma.y);
    var color = vec3(0.0);
    for (var y = 0u; y < 2u; y += 1u) { for (var x = 0u; x < 2u; x += 1u) {
        let p = (position.xy + vec2(f32(x), f32(y)) * 0.5 - vec2(0.25)) / size * 2.0 - vec2(1.0);
        color += textureSampleLevel(environment, environment_sampler, cube_direction(face, p), 0.0).rgb;
    }}
    return vec4(color * 0.25, 1.0);
}

@fragment fn fs_brdf(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let roughness = position.x / 128.0;
    let nv = position.y / 128.0;
    let v = vec3(sqrt(max(1.0 - nv * nv, 0.0)), 0.0, nv);
    var scale_bias = vec2(0.0);
    let m = roughness * roughness;
    let m2 = m * m;
    for (var i = 0u; i < 1024u; i += 1u) {
        let xi = hammersley(i, 1024u);
        let phi = 6.28318530718 * xi.x;
        let cos_theta = sqrt((1.0 - xi.y) / (1.0 + (m2 - 1.0) * xi.y));
        let sin_theta = sqrt(max(1.0 - cos_theta * cos_theta, 0.0));
        let h = vec3(sin_theta * cos(phi), sin_theta * sin(phi), cos_theta);
        let vh = max(dot(v, h), 0.0);
        let nl = max(2.0 * dot(v, h) * h.z - nv, 0.0);
        if (nl > 0.0) {
            let vis_v = nl * sqrt(nv * (nv - nv * m2) + m2);
            let vis_l = nv * sqrt(nl * (nl - nl * m2) + m2);
            let visibility = 0.5 / max(vis_v + vis_l, 1e-8);
            let weighted_pdf = nl * visibility * (4.0 * vh / max(h.z, 1e-6));
            let fresnel = pow(1.0 - vh, 5.0);
            scale_bias += vec2(1.0 - fresnel, fresnel) * weighted_pdf;
        }
    }
    return vec4(scale_bias / 1024.0, 0.0, 1.0);
}
