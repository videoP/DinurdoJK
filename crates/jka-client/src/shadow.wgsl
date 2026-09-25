struct ShadowCaster {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
};

struct Material {
    header: vec4<u32>,
    vector_s: vec4<f32>,
    vector_t: vec4<f32>,
    mods: array<vec4<f32>, 8>,
    color: vec4<f32>,
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> shadow: ShadowCaster;
@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var base_sampler: sampler;
@group(1) @binding(4) var<uniform> material: Material;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) lightmap_uv: vec2<f32>,
    @location(5) normal: vec3<f32>,
    @location(3) color: vec4<f32>,
};

struct MaskVertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) alpha_multiplier: f32,
};

fn generated_uv(input: VertexIn) -> vec2<f32> {
    var uv = input.uv;
    if (material.header.x == 1u) {
        uv = input.lightmap_uv;
    }
    if (material.header.x == 2u) {
        uv = vec2<f32>(
            dot(input.position, material.vector_s.xyz),
            dot(input.position, material.vector_t.xyz)
        );
    }
    if (material.header.x == 3u) {
        let n = normalize(input.normal);
        let view = normalize(shadow.camera_pos_time.xyz - input.position);
        let reflected = reflect(-view, n);
        uv = vec2<f32>(0.5 - reflected.z * 0.5, 0.5 - reflected.y * 0.5);
    }

    let time = shadow.camera_pos_time.w;
    for (var i = 0u; i < 4u; i = i + 1u) {
        if (i >= material.header.y) {
            break;
        }
        let a = material.mods[i * 2u];
        let b = material.mods[i * 2u + 1u];
        let kind = u32(a.x + 0.5);
        if (kind == 1u) {
            uv = uv + a.yz * time;
        }
        if (kind == 2u) {
            uv = uv * a.yz;
        }
        if (kind == 3u) {
            let radians = a.y * time * 0.017453292519943295;
            let c = cos(radians);
            let sn = sin(radians);
            let p = uv - vec2<f32>(0.5);
            uv = vec2<f32>(p.x * c - p.y * sn, p.x * sn + p.y * c) + vec2<f32>(0.5);
        }
        if (kind == 4u) {
            uv = vec2<f32>(
                uv.x * a.y + uv.y * a.z + a.w,
                uv.x * b.x + uv.y * b.y + b.z
            );
        }
        if (kind == 5u) {
            let wave = a.y
                + sin((a.w + time * b.x + uv.x + uv.y) * 6.28318530718) * a.z;
            uv = uv + vec2<f32>(wave);
        }
    }
    return uv;
}

fn encode_sky_depth(z: f32) -> f32 {
    return 0.5 + atan(z) / 3.14159265358979323846;
}

@vertex fn vs_main(input: VertexIn) -> @builtin(position) vec4<f32> {
    return shadow.view_proj * vec4<f32>(input.position, 1.0);
}

@vertex fn vs_sky(input: VertexIn) -> @builtin(position) vec4<f32> {
    let clip = shadow.view_proj * vec4<f32>(input.position, 1.0);
    let safe_abs_w = max(abs(clip.w), 1.0e-6);
    let safe_w = select(-safe_abs_w, safe_abs_w, clip.w >= 0.0);
    let ndc_z = clip.z / safe_w;
    let encoded = encode_sky_depth(ndc_z);
    // Preserve the cascade's X/Y projection but force Z into the legal clip
    // interval. The encoded depth is monotonic, so the depth test still picks
    // the nearest sky surface along the directional-light ray even when that
    // surface lies beyond the ordinary CSM receiver depth range.
    return vec4<f32>(clip.x, clip.y, encoded * clip.w, clip.w);
}

@vertex fn vs_mask(input: VertexIn) -> MaskVertexOut {
    var output: MaskVertexOut;
    output.position = shadow.view_proj * vec4<f32>(input.position, 1.0);
    output.uv = generated_uv(input);
    output.alpha_multiplier = material.color.a;
    if ((material.header.z & 1u) != 0u) {
        output.alpha_multiplier *= input.color.a;
    }
    return output;
}

@fragment fn fs_mask(input: MaskVertexOut) {
    let alpha = textureSample(base_texture, base_sampler, input.uv).a * input.alpha_multiplier;
    if (alpha < material.params.x) {
        discard;
    }
}

@fragment fn fs_translucent_shadow(input: MaskVertexOut) {
    let alpha = clamp(textureSample(base_texture, base_sampler, input.uv).a * input.alpha_multiplier, 0.0, 1.0);
    // Stable 4x4 Bayer coverage converts authored translucency into fractional
    // directional-shadow coverage. PCF then resolves it into smooth partial
    // transmission without needing an additional full-resolution color atlas.
    let x = u32(input.position.x) & 3u;
    let y = u32(input.position.y) & 3u;
    let index = y * 4u + x;
    let bayer = array<f32, 16>(
         0.0,  8.0,  2.0, 10.0,
        12.0,  4.0, 14.0,  6.0,
         3.0, 11.0,  1.0,  9.0,
        15.0,  7.0, 13.0,  5.0
    );
    let threshold = (bayer[index] + 0.5) / 16.0;
    if (alpha < threshold) {
        discard;
    }
}
