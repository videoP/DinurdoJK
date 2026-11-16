// TaystJK RB_CalcStretchTexCoords: reciprocal waveform scaled about (0.5, 0.5).
// Only evaluated for kind 6 (tcMod stretch).
fn tc_stretch_wave(kind: u32, phase: f32) -> f32 {
    let x = fract(phase);
    if (kind == 0u) { return sin(x * 6.28318530718); }
    if (kind == 1u) { return select(select(2.0 - 4.0*x, 4.0*x - 4.0, x >= 0.75), 4.0*x, x < 0.25); }
    if (kind == 2u) { return select(-1.0, 1.0, x < 0.5); }
    if (kind == 3u) { return x; }
    if (kind == 4u) { return 1.0 - x; }
    // Stable noise approximation for GF_NOISE, where the reference uses a LUT.
    let n0 = floor(phase);
    let t = fract(phase);
    let a = fract(sin(n0 * 127.1) * 43758.5453) * 2.0 - 1.0;
    let b = fract(sin((n0 + 1.0) * 127.1) * 43758.5453) * 2.0 - 1.0;
    return mix(a, b, t*t*(3.0 - 2.0*t));
}

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

// JaPro: subtract the remap offset, then apply a positive clampTime.
// vector_t.w is reserved for the limit; zero leaves the clock unbounded.
fn shader_stage_time() -> f32 {
    let elapsed = shadow.camera_pos_time.w - material.params.w;
    return select(elapsed, min(elapsed, material.vector_t.w), material.vector_t.w > 0.0);
}

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

    let time = shader_stage_time();
    for (var i = 0u; i < 4u; i = i + 1u) {
        if (i >= material.header.y) {
            break;
        }
        let a = material.mods[i * 2u];
        let b = material.mods[i * 2u + 1u];
        let kind = u32(a.x + 0.5);
        if (kind == 1u) {
            // TaystJK RB_CalcScrollTexCoords wraps the offset before later tcMods.
            // This matters when rotate follows scroll: a whole-texture offset
            // cannot be discarded after the rotation.
            let scroll = a.yz * time;
            uv = uv + scroll - floor(scroll);
        }
        if (kind == 2u) {
            uv = uv * a.yz;
        }
        if (kind == 3u) {
            // TaystJK RB_CalcRotateTexCoords negates degrees-per-second.
            let radians = -a.y * time * 0.017453292519943295;
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
        if (kind == 6u) {
            let value = a.y + a.z * tc_stretch_wave(u32(b.y + 0.5), select(a.w + time*b.x, (time+a.w)*b.x, u32(b.y + 0.5) == 5u));
            if (abs(value) > 1.0e-6) {
                uv = (uv - vec2<f32>(0.5)) / value + vec2<f32>(0.5);
            }
        }
        if (kind == 5u) {
            // Matches OpenJK's RB_CalcTurbulentTexCoords: driven by vertex
            // world position (scaled), not by the surface's own UV, which
            // aliases badly on any surface that tiles many times.
            let now = a.w + time * b.x;
            let wave = vec2<f32>(
                sin((now + (input.position.x + input.position.y) * 0.0009765625) * 6.28318530718) * a.z,
                sin((now - input.position.z * 0.0009765625) * 6.28318530718) * a.z
            );
            uv = uv + wave;
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
    if ((material.header.z & MATERIAL_FLAG_RGB_VERTEX) != 0u) {
        output.alpha_multiplier *= input.color.a;
    }
    return output;
}

@fragment fn fs_mask(input: MaskVertexOut) {
    let alpha = textureSample(base_texture, base_sampler, input.uv).a * input.alpha_multiplier;
    if ((material.params.x > 0.0 && alpha < material.params.x || material.params.x < 0.0 && alpha >= -material.params.x)) {
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
