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

struct Camera {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
    clip_plane: vec4<f32>,
    render_flags: vec4<u32>,
    camera_forward: vec4<f32>,
    unjittered_view_proj: mat4x4<f32>,
    previous_unjittered_view_proj: mat4x4<f32>,
    jump_shade: vec4<f32>,
    foliage_wind: vec4<f32>,
};
struct Material {
    header: vec4<u32>,
    vector_s: vec4<f32>,
    vector_t: vec4<f32>,
    mods: array<vec4<f32>, 8>,
    color: vec4<f32>,
    params: vec4<f32>,
    pbr_params0: vec4<f32>,
    pbr_params1: vec4<f32>,
    reflection_probe: vec4<f32>,
    planar_plane: vec4<f32>,
    wave_rgb: vec4<f32>,
    wave_alpha: vec4<f32>,
    wave_funcs: vec4<u32>,
    ocean_color: vec4<f32>, // linear authored texture tint, opacity; negative alpha = default
    ocean_material: vec4<f32>, // xyz reflectance, w roughness; negative = default
    ocean_waves: vec4<f32>, // xy reserved/amplitude hint, z/w authored blend factors
    ocean_blend_color: vec4<f32>, // analyzed mean color/brightness; never a texture lookup
};
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var base_sampler: sampler;
@group(1) @binding(2) var lightmap_texture: texture_2d<f32>;
@group(1) @binding(3) var lightmap_sampler: sampler;
@group(1) @binding(4) var<uniform> material: Material;
@group(1) @binding(5) var sky_rt: texture_2d<f32>;
@group(1) @binding(6) var sky_bk: texture_2d<f32>;
@group(1) @binding(7) var sky_lf: texture_2d<f32>;
@group(1) @binding(8) var sky_ft: texture_2d<f32>;
@group(1) @binding(9) var sky_up: texture_2d<f32>;
@group(1) @binding(10) var sky_dn: texture_2d<f32>;
@group(1) @binding(11) var sky_sampler: sampler;
// Fused additive siblings share the base draw geometry, but NOT material state.
// These are the same MaterialUniform blocks as their original separate passes.
@group(1) @binding(32) var<uniform> additive_stage1: Material;
@group(1) @binding(33) var<uniform> additive_stage2: Material;
@group(1) @binding(24) var additive_clamp_sampler: sampler;

@group(1) @binding(31) var additive_repeat_sampler: sampler;


const CLASSIC_FULLBRIGHT: u32 = 1u;
const CLASSIC_VERTEX_LIGHT: u32 = 2u;
const CLASSIC_LIGHTMAP_ONLY: u32 = 4u;
const MATERIAL_EXPLICIT_LIGHTMAP: u32 = 8388608u;
const MATERIAL_HAS_LIGHTMAP: u32 = 16777216u;
const MATERIAL_OPAQUE_STAGE: u32 = 33554432u;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) lightmap_uv: vec2<f32>,
    @location(5) normal: vec3<f32>,
    @location(3) color: vec4<f32>,
    @location(4) static_ao: f32,
};
struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) lightmap_uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    // xyz: sky direction, w: cached static BSP AO. Packing AO here avoids
    // allocating another interpolator slot.
    @location(3) sky_dir_ao: vec4<f32>,
    @location(4) world_position: vec3<f32>,
    @location(5) world_normal: vec3<f32>,
    @location(6) @interpolate(flat) shell_kind: u32,
    @location(7) shell_coverage: f32,
};

// Fused additive stages only. Reuse locations unused by this shader family;
// ordinary world vertex/fragment entry points keep their original varyings.
// Affine tcMods commute with perspective-correct interpolation, so calculate
// each stage's coordinates per vertex as the original JKA stage path did.
struct AdditiveVertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) additive_uv1: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) additive_uv2: vec2<f32>,
    @location(4) world_position: vec3<f32>,
    @location(5) world_normal: vec3<f32>,
    @location(6) @interpolate(flat) shell_kind: u32,
    @location(7) shell_coverage: f32,
};


// Waveform generators (rgbGen wave / alphaGen wave). Same functions as id Tech 3's
// tables: value = base + func(phase + time * frequency) * amplitude.
fn wave_noise(t: f32) -> f32 {
    let i = floor(t);
    let u = fract(t);
    let s = u * u * (3.0 - 2.0 * u);
    let a = fract(sin(i * 127.1) * 43758.5453) * 2.0 - 1.0;
    let b = fract(sin((i + 1.0) * 127.1) * 43758.5453) * 2.0 - 1.0;
    return mix(a, b, s);
}

fn wave_value(func: u32, p: vec4<f32>, time: f32) -> f32 {
    let x = fract(p.z + time * p.w);
    var v = 0.0;
    if (func == 0u) {
        v = sin(x * 6.28318530718);
    } else if (func == 1u) {
        v = select(select(2.0 - 4.0 * x, 4.0 * x - 4.0, x >= 0.75), 4.0 * x, x < 0.25);
    } else if (func == 2u) {
        v = select(-1.0, 1.0, x < 0.5);
    } else if (func == 3u) {
        v = x;
    } else if (func == 4u) {
        v = 1.0 - x;
    } else {
        v = wave_noise((time + p.z) * p.w);
    }
    return p.x + v * p.y;
}

// JaPro: subtract the remap offset, then apply a positive clampTime.
// vector_t.w is reserved for the limit; zero leaves the clock unbounded.
fn shader_stage_time() -> f32 {
    let elapsed = camera.camera_pos_time.w - material.params.w;
    return select(elapsed, min(elapsed, material.vector_t.w), material.vector_t.w > 0.0);
}

fn apply_wave_gens(color: vec4<f32>) -> vec4<f32> {
    var out = color;
    let time = shader_stage_time();
    if ((material.header.z & 134217728u) != 0u) {
        let g = clamp(wave_value(material.wave_funcs.x, material.wave_rgb, time), 0.0, 1.0);
        out = vec4<f32>(vec3<f32>(g), out.a);
    }
    if ((material.header.z & 268435456u) != 0u) {
        out.a = clamp(wave_value(material.wave_funcs.y, material.wave_alpha, time), 0.0, 1.0);
    }
    return out;
}

// Stage tcMods (scroll/scale/rotate/transform/turb) applied to base coordinates.
// Shared by the per-vertex generated_uv and the per-pixel sky cloud layer.
// `world_pos` is the vertex position in this renderer's [x, z, -y] render
// space (JKA [x, y, z]); it only matters for tcMod turb (kind 5), which
// OpenJK's RB_CalcTurbulentTexCoords drives from (JKA x + JKA z) for S and
// JKA y for T, scaled by 1/128 * 0.125, NOT from the surface's own UV -
// using UV there aliases badly on any surface that tiles many times.
fn apply_tc_mods(base_uv: vec2<f32>, world_pos: vec3<f32>) -> vec2<f32> {
    var uv = base_uv;
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
            let now = a.w + time * b.x;
            let wave = vec2<f32>(
                sin((now + (world_pos.x + world_pos.y) * 0.0009765625) * 6.28318530718) * a.z,
                sin((now - world_pos.z * 0.0009765625) * 6.28318530718) * a.z
            );
            uv = uv + wave;
        }
    }
    return uv;
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
        // Q3/JKA sphere-style environment mapping: view direction reflected by
        // the surface normal. Coordinates are in this renderer's transformed
        // [x,z,-y] space, so use render-space Y/Z for the two lookup axes.
        let n = normalize(input.normal);
        let view = normalize(camera.camera_pos_time.xyz - input.position);
        let reflected = reflect(-view, n);
        // OpenJK computes s from JKA Y and t from JKA Z. The renderer uses
        // [x,z,-y], therefore JKA Y = -render Z and JKA Z = render Y.
        uv = vec2<f32>(0.5 - reflected.z * 0.5, 0.5 - reflected.y * 0.5);
    }

    return apply_tc_mods(uv, input.position);
}

fn fast_world_vertex(input: VertexIn, instance_index: u32) -> VertexOut {
    var output: VertexOut;
    let deformation = deform_surface_deformation_vertex(
        input.position,
        input.normal,
        material.header.w,
        instance_index,
        true
    );
    let world_position = foliage_wind_position(deformation.xyz, material.header.w, camera.foliage_wind);
    output.clip_position = camera.view_proj * vec4<f32>(world_position, 1.0);
    output.uv = generated_uv(input);
    output.lightmap_uv = input.lightmap_uv;
    output.color = input.color;
    // Keep the known-fast fragment path free of the camera bind group.
    // When cached BSP AO is disabled, feed neutral AO (1.0) through the
    // existing interpolator so fs_main never needs camera.render_flags.
    let cached_static_ao = select(1.0, input.static_ao, (camera.render_flags.y & 1u) != 0u);
    output.sky_dir_ao = vec4<f32>(world_position - camera.camera_pos_time.xyz, cached_static_ao);
    output.world_position = world_position;
    output.world_normal = surface_deformation_vertex_normal(
        input.position, input.normal, material.header.w, instance_index, true
    );
    // The fast pipeline intentionally exposes the camera uniform to the vertex
    // stage only. Pack the three classic-lighting flags into the unused upper
    // bits of the already-flat shell_kind varying rather than adding fragment
    // camera visibility or another interpolator. Bit 0 remains shell_kind.
    let shell_kind = select(0u, 1u, instance_index != 0u);
    output.shell_kind = shell_kind | (camera.render_flags.z << 1u);
    output.shell_coverage = deformation.w;
    return output;
}

// TaystJK rd-vanilla Autosprite2Deform, prepared per authored quad at map load.
// position = midpoint of its short edge, normal = shared long axis,
// lightmap_uv.x = signed half-width; y = -1337 is the encoding marker.
// Unmodified geometry (including source .map) keeps its original positions.
// This entry exists only for the autoSprite2 pipeline key; ordinary BSP vertex
// shaders have no billboard branch, uniforms or per-vertex overhead.
fn autosprite2_input(original: VertexIn) -> VertexIn {
    var input = original;
    let half_width = original.lightmap_uv.x;
    if (original.lightmap_uv.y == -1337.0) {
        // BackEnd view axis[0] is the camera forward direction (not the
        // position-to-eye vector). A single long-axis rotation, not full XYZ.
        let minor = cross(original.normal, camera.camera_forward.xyz);
        let length_sq = dot(minor, minor);
        if (length_sq > 0.000001) {
            input.position = original.position + minor * (half_width * inverseSqrt(length_sq));
            // Deformed surface normal for lighting passes that use it.
            input.normal = normalize(cross(original.normal, minor));
        }
    }
    return input;
}

@vertex fn vs_autosprite2(input: VertexIn, @builtin(instance_index) instance_index: u32) -> VertexOut {
    return fast_world_vertex(autosprite2_input(input), instance_index);
}

@vertex fn vs_main(input: VertexIn, @builtin(instance_index) instance_index: u32) -> VertexOut {
    return fast_world_vertex(input, instance_index);
}

// Match the full world path/OpenJK sky semantics: authored sky BSP polygons are
// an angular visibility mask, while the visible sky itself lives at zFar. In
// reversed-Z, depth 0 is the camera far plane.
@vertex fn vs_sky(input: VertexIn, @builtin(instance_index) instance_index: u32) -> VertexOut {
    var output = fast_world_vertex(input, instance_index);
    output.clip_position.z = 0.0;
    return output;
}


// Unified legacy RGB policy: the opaque lightmap/overlay fusion below can
// compose its known inputs exactly in encoded sRGB. Additive BSP passes have
// an unknown framebuffer destination, so their encoded source is a no-pass
// approximation while blending remains linear in the scene target. Both use
// the SAME transfer function. Keep alpha and authored blend factors unchanged.
const MATERIAL_LEGACY_ADDITIVE_SRGB: u32 = 536870912u;
// Approximate TaystJK's additive SOURCE modulation: its 8-bit rgbGen wave
// multiplies the encoded texture sample before the blend. The scene target
// remains linear, so this does not emulate encoded-framebuffer destination
// blending (which would require access to the destination color).
fn legacy_additive_source(source: vec4<f32>, waveform: f32, has_wave: bool) -> vec4<f32> {
    if (!has_wave) {
        return source;
    }
    if (waveform <= 0.0) {
        return vec4<f32>(vec3<f32>(0.0), source.a);
    }
    // Textures were sRGB-decoded by sampling and modulated by rgbGen wave.
    // Recover their linear sample, perform the legacy modulation in encoded
    // space with the reference 8-bit wave, then decode for WGPU blending.
    let texel_linear = max(source.rgb / waveform, vec3<f32>(0.0));
    let vertex_wave_byte = round(clamp(waveform, 0.0, 1.0) * 255.0) / 255.0;
    return vec4<f32>(legacy_rgb_decode(legacy_rgb_encode(texel_linear) * vertex_wave_byte), source.a);
}
fn legacy_rgb_output(color: vec4<f32>) -> vec4<f32> {
    if ((material.header.z & MATERIAL_LEGACY_ADDITIVE_SRGB) != 0u) {
        let has_wave = (material.header.z & 134217728u) != 0u;
        var wave = 1.0;
        if (has_wave) {
            wave = clamp(wave_value(material.wave_funcs.x, material.wave_rgb, shader_stage_time()), 0.0, 1.0);
        }
        return legacy_additive_source(color, wave, has_wave);
    }
    return color;
}

fn legacy_rgb_encode(value: vec3<f32>) -> vec3<f32> {
    let v = max(value, vec3<f32>(0.0));
    return select(1.055 * pow(v, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055),
                  12.92 * v, v <= vec3<f32>(0.0031308));
}
fn legacy_rgb_decode(value: vec3<f32>) -> vec3<f32> {
    let v = max(value, vec3<f32>(0.0));
    return select(pow((v + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4)),
                  v / 12.92, v <= vec3<f32>(0.04045));
}
fn classic_three_stage_rgb(input: VertexOut) -> vec4<f32> {
    // 0: base diffuse; 2: baked lightmap; 5: stage-three overlay (non-sky).
    let diffuse = textureSample(base_texture, base_sampler, input.uv).rgb;
    let lightmap = textureSample(lightmap_texture, lightmap_sampler, input.lightmap_uv).rgb;
    let overlay_sample = textureSample(sky_rt, sky_sampler, input.uv);
    // alphaGen vertex multiplies the authored image alpha, not replaces it.
    let vertex_alpha = clamp(input.color.a * overlay_sample.a, 0.0, 1.0);
    let encoded = mix(
        legacy_rgb_encode(diffuse) * legacy_rgb_encode(lightmap),
        legacy_rgb_encode(overlay_sample.rgb), vertex_alpha);
    return vec4<f32>(legacy_rgb_decode(encoded), 1.0);
}

@fragment fn fs_legacy_srgb_three_stage(input: VertexOut) -> @location(0) vec4<f32> {
    let shell_kind = input.shell_kind & 1u;
    if (surface_deformation_should_discard(input.world_position, material.header.w,
        input.world_normal, shell_kind, input.shell_coverage, input.clip_position.xy, true)) {
        discard;
    }
    return classic_three_stage_rgb(input);
}


// Source additive draws previously computed each of these transformations in
// its own vertex shader. Reversing the primary affine tcMods gives the exact
// common pre-modified coordinates; applying each sibling's own tcMods then
// evaluates the same independent animated UV in one fragment shader.
fn additive_time(m: Material) -> f32 {
    let elapsed = camera.camera_pos_time.w - m.params.w;
    return select(elapsed, min(elapsed, m.vector_t.w), m.vector_t.w > 0.0);
}
fn additive_transform_uv(raw: vec2<f32>, m: Material) -> vec2<f32> {
    var uv = raw;
    let time = additive_time(m);
    for (var i = 0u; i < 4u; i = i + 1u) {
        if (i >= m.header.y) { break; }
        let a = m.mods[i * 2u];
        let b = m.mods[i * 2u + 1u];
        let kind = u32(a.x + 0.5);
        if (kind == 1u) {
            let scroll = a.yz * time;
            uv += scroll - floor(scroll);
        } else if (kind == 2u) {
            uv *= a.yz;
        } else if (kind == 3u) {
            let radians = -a.y * time * 0.017453292519943295;
            let c = cos(radians);
            let sn = sin(radians);
            let p = uv - vec2<f32>(0.5);
            uv = vec2<f32>(p.x * c - p.y * sn, p.x * sn + p.y * c) + vec2<f32>(0.5);
        } else if (kind == 4u) {
            uv = vec2<f32>(uv.x * a.y + uv.y * a.z + a.w,
                           uv.x * b.x + uv.y * b.y + b.z);
        }
    }
    return uv;
}
fn additive_untransform_uv(modified: vec2<f32>, m: Material) -> vec2<f32> {
    var uv = modified;
    let time = additive_time(m);
    for (var j = 0u; j < 4u; j = j + 1u) {
        if (j >= m.header.y) { break; }
        let i = m.header.y - 1u - j;
        let a = m.mods[i * 2u];
        let b = m.mods[i * 2u + 1u];
        let kind = u32(a.x + 0.5);
        if (kind == 1u) {
            let scroll = a.yz * time;
            uv -= scroll - floor(scroll);
        } else if (kind == 2u) {
            uv /= a.yz;
        } else if (kind == 3u) {
            let radians = a.y * time * 0.017453292519943295;
            let c = cos(radians);
            let sn = sin(radians);
            let p = uv - vec2<f32>(0.5);
            uv = vec2<f32>(p.x * c - p.y * sn, p.x * sn + p.y * c) + vec2<f32>(0.5);
        } else if (kind == 4u) {
            let determinant = a.y * b.y - a.z * b.x;
            let p = uv - vec2<f32>(a.w, b.z);
            uv = vec2<f32>((p.x * b.y - p.y * a.z) / determinant,
                           (p.y * a.y - p.x * b.x) / determinant);
        }
    }
    return uv;
}
// The source stage tcMods run on vertices in JKA. A fused fragment can use
// interpolated UVs for its companions instead of rebuilding/inverting tcMods
// for each covered pixel (especially expensive on screen-filling water).
fn additive_vertex_output(base: VertexOut, count: u32) -> AdditiveVertexOut {
    var out: AdditiveVertexOut;
    out.clip_position = base.clip_position;
    out.uv = base.uv;
    out.color = base.color;
    out.world_position = base.world_position;
    out.world_normal = base.world_normal;
    out.shell_kind = base.shell_kind;
    out.shell_coverage = base.shell_coverage;
    let raw_uv = additive_untransform_uv(base.uv, material);
    out.additive_uv1 = additive_transform_uv(raw_uv, additive_stage1);
    out.additive_uv2 = vec2<f32>(0.0);
    if (count == 3u) {
        out.additive_uv2 = additive_transform_uv(raw_uv, additive_stage2);
    }
    return out;
}

@vertex fn vs_additive_fused2(input: VertexIn, @builtin(instance_index) instance_index: u32) -> AdditiveVertexOut {
    return additive_vertex_output(fast_world_vertex(input, instance_index), 2u);
}
@vertex fn vs_additive_fused3(input: VertexIn, @builtin(instance_index) instance_index: u32) -> AdditiveVertexOut {
    return additive_vertex_output(fast_world_vertex(input, instance_index), 3u);
}

fn additive_stage_tint(m: Material, vertex_color: vec4<f32>) -> vec4<f32> {
    var color = m.color;
    if ((m.header.z & 1u) != 0u) {
        color = vec4<f32>(color.rgb * vertex_color.rgb, color.a);
    } else if ((m.header.z & 524288u) != 0u) {
        color = vec4<f32>(color.rgb * (vec3<f32>(1.0) - vertex_color.rgb), color.a);
    }
    if ((m.header.z & 1048576u) != 0u) {
        color.a *= vertex_color.a;
    } else if ((m.header.z & 2097152u) != 0u) {
        color.a *= (1.0 - vertex_color.a);
    }
    if ((m.header.z & 134217728u) != 0u) {
        let wave = clamp(wave_value(m.wave_funcs.x, m.wave_rgb, additive_time(m)), 0.0, 1.0);
        color = vec4<f32>(vec3<f32>(wave), color.a);
    }
    if ((m.header.z & 268435456u) != 0u) {
        color.a = clamp(wave_value(m.wave_funcs.y, m.wave_alpha, additive_time(m)), 0.0, 1.0);
    }
    return color;
}
fn additive_weighted_sample(texel: vec4<f32>, m: Material, vertex_color: vec4<f32>) -> vec4<f32> {
    var sample = texel * additive_stage_tint(m, vertex_color);
    if ((m.header.z & MATERIAL_LEGACY_ADDITIVE_SRGB) != 0u) {
        let has_wave = (m.header.z & 134217728u) != 0u;
        var wave = 1.0;
        if (has_wave) {
            wave = clamp(wave_value(m.wave_funcs.x, m.wave_rgb, additive_time(m)), 0.0, 1.0);
        }
        sample = legacy_additive_source(sample, wave, has_wave);
    }
    // Authored source blend is GL_SRC_ALPHA GL_ONE. Fused fixed blend is ONE/ONE,
    // so preweight RGB and alpha by the source alpha as before.
    return vec4<f32>(sample.rgb * sample.a, sample.a * sample.a);
}
fn additive_fused_contributions(input: AdditiveVertexOut, count: u32) -> vec4<f32> {
    let base = textureSample(base_texture, base_sampler, input.uv);
    var total = additive_weighted_sample(base, material, input.color);
    // Every companion shares the same primary inverse tcMods. Evaluate that
    // inverse once per fragment, then apply independent authored sibling tcMods.
    // Three-stage fusion previously recomputed the inverse twice; no sampling,
    // stage ordering, blending or shader permutation changes.
    let uv1 = input.additive_uv1;
    if ((additive_stage1.header.w & 65536u) != 0u) {
        total += additive_weighted_sample(textureSample(sky_rt, additive_clamp_sampler, uv1), additive_stage1, input.color);
    } else {
        total += additive_weighted_sample(textureSample(sky_rt, additive_repeat_sampler, uv1), additive_stage1, input.color);
    }
    if (count >= 3u) {
        let uv2 = input.additive_uv2;
        if ((additive_stage2.header.w & 65536u) != 0u) {
            total += additive_weighted_sample(textureSample(sky_bk, additive_clamp_sampler, uv2), additive_stage2, input.color);
        } else {
            total += additive_weighted_sample(textureSample(sky_bk, additive_repeat_sampler, uv2), additive_stage2, input.color);
        }
    }
    return total;
}


@fragment fn fs_additive_fused2(input: AdditiveVertexOut) -> @location(0) vec4<f32> {
    if (surface_deformation_should_discard(input.world_position, material.header.w,
        input.world_normal, input.shell_kind & 1u, input.shell_coverage, input.clip_position.xy,
        true)) { discard; }
    return additive_fused_contributions(input, 2u);
}
@fragment fn fs_additive_fused3(input: AdditiveVertexOut) -> @location(0) vec4<f32> {
    if (surface_deformation_should_discard(input.world_position, material.header.w, input.world_normal, input.shell_kind & 1u, input.shell_coverage, input.clip_position.xy, true)) { discard; }
    return additive_fused_contributions(input, 3u);
}
@fragment fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let shell_kind = input.shell_kind & 1u;
    let classic_flags = input.shell_kind >> 1u;
    if (surface_deformation_should_discard(
        input.world_position, material.header.w, input.world_normal, shell_kind, input.shell_coverage, input.clip_position.xy, true
    )) {
        discard;
    }
    if ((material.header.w & (1u << 17u)) != 0u) {
        let to_eye = normalize(camera.camera_pos_time.xyz - input.world_position);
        let facing = abs(dot(normalize(input.world_normal), to_eye));
        let f0 = clamp(material.ocean_color.a, 0.02, 0.18);
        let fresnel = f0 + (1.0 - f0) * pow(1.0 - clamp(facing, 0.0, 1.0), 5.0);
        return vec4<f32>(mix(vec3<f32>(0.65), material.ocean_color.rgb, 0.35), 0.08 + 0.55 * fresnel);
    }
    let classic_fullbright = (classic_flags & CLASSIC_FULLBRIGHT) != 0u;
    let classic_vertex_light = (classic_flags & CLASSIC_VERTEX_LIGHT) != 0u;
    let classic_lightmap_only = (classic_flags & CLASSIC_LIGHTMAP_ONLY) != 0u;
    let explicit_lightmap_stage = (material.header.z & MATERIAL_EXPLICIT_LIGHTMAP) != 0u;
    let has_lightmap = (material.header.z & MATERIAL_HAS_LIGHTMAP) != 0u;
    let opaque_stage = (material.header.z & MATERIAL_OPAQUE_STAGE) != 0u;

    var stage_color = material.color;
    if ((material.header.z & 1u) != 0u) {
        stage_color = vec4<f32>(stage_color.rgb * input.color.rgb, stage_color.a);
    } else if ((material.header.z & 524288u) != 0u) {
        stage_color = vec4<f32>(
            stage_color.rgb * (vec3<f32>(1.0) - input.color.rgb),
            stage_color.a,
        );
    }
    if ((material.header.z & 1048576u) != 0u) {
        stage_color.a = stage_color.a * input.color.a;
    } else if ((material.header.z & 2097152u) != 0u) {
        stage_color.a = stage_color.a * (1.0 - input.color.a);
    }
    stage_color = apply_wave_gens(stage_color);

    var source_sample = textureSample(base_texture, base_sampler, input.uv);

    if (explicit_lightmap_stage) {
        if (classic_fullbright || (classic_vertex_light && classic_lightmap_only)) {
            source_sample = vec4<f32>(1.0);
        } else if (classic_vertex_light) {
            source_sample = vec4<f32>(input.color.rgb, source_sample.a);
        }
    }

    if (classic_lightmap_only && has_lightmap
        && !explicit_lightmap_stage
        && (material.header.z & 2u) == 0u) {
        if (opaque_stage) {
            source_sample = vec4<f32>(1.0);
            stage_color = vec4<f32>(1.0);
        } else {
            discard;
        }
    }

    var base = source_sample * stage_color;
    // An authored masked base with BSP vertex alpha 127/255 and GE128 cannot
    // seed depth. For the narrowly identified multi-stage depth-equal case,
    // test the source texture's alpha while retaining the vertex alpha for
    // the actual stage output and for all sibling shaders.
    let mask_alpha = select(base.a, source_sample.a * material.color.a,
        (material.header.z & 1073741824u) != 0u);
    if ((material.params.x > 0.0 && mask_alpha < material.params.x || material.params.x < 0.0 && mask_alpha >= -material.params.x)) {
        discard;
    }

    // Only the synthesized implicit material uses a single-pass
    // base-texture × lightmap path. Explicit JKA shader scripts render their
    // `$lightmap` as its own ordered blend stage instead.
    if ((material.header.z & 2u) != 0u && input.lightmap_uv.x >= 0.0) {
        var lightmap_color = textureSample(lightmap_texture, lightmap_sampler, input.lightmap_uv).rgb;
        if (classic_fullbright) {
            lightmap_color = vec3<f32>(1.0);
        } else if (classic_vertex_light) {
            lightmap_color = select(input.color.rgb, vec3<f32>(1.0), classic_lightmap_only);
        }
        if (classic_lightmap_only) {
            base = vec4<f32>(lightmap_color, base.a);
        } else {
            base = vec4<f32>(base.rgb * lightmap_color, base.a);
        }
    }

    if (classic_lightmap_only && (material.header.z & 131072u) != 0u && !has_lightmap) {
        base = vec4<f32>(select(input.color.rgb, vec3<f32>(1.0), classic_vertex_light), base.a);
    }

    // Cached static BSP AO only modulates the normal baked-light contribution.
    if (classic_flags == 0u
        && (material.header.x == 1u || (material.header.z & 2u) != 0u || (material.header.z & 131072u) != 0u)) {
        let static_ao = clamp(input.sky_dir_ao.w, 0.0, 1.0);
        base = vec4<f32>(base.rgb * static_ao, base.a);
    }
    let result = shade_surface_deformation(
        input.world_position,
        input.world_normal,
        material.header.w,
        material.header.x,
        shell_kind,
        input.shell_coverage,
        base
    );
    return legacy_rgb_output(result);
}

fn sky_uv(s: f32, t: f32) -> vec2<f32> {
    // Quake 3 MakeSkyVec maps [-1,1] to [0,1] and flips T. Keep a tiny
    // inset to avoid bilinear/trilinear sampling across a face edge.
    return clamp(vec2<f32>((s + 1.0) * 0.5, (1.0 - t) * 0.5), vec2<f32>(0.001), vec2<f32>(0.999));
}

// Cloud layer of a sky shader (R_InitSkyTexCoords). The view direction `d`
// (JKA axes, z up) is projected onto a spherical shell `height` above a
// 4096-unit-radius ground sphere, and the direction from the sphere's centre to
// that point gives the base coordinates as two angles in radians. The stage's
// tcMods are then applied on top, exactly like any other stage.
fn sky_cloud_uv(d: vec3<f32>, height: f32) -> vec2<f32> {
    let radius = 4096.0;
    let dd = dot(d, d);
    let discriminant = d.z * d.z * radius * radius + dd * (2.0 * radius * height + height * height);
    let p = (-2.0 * d.z * radius + 2.0 * sqrt(discriminant)) / (2.0 * dd);
    var v = d * p;
    v.z = v.z + radius;
    v = normalize(v);
    return vec2<f32>(acos(clamp(v.x, -1.0, 1.0)), acos(clamp(v.y, -1.0, 1.0)));
}

fn sky_cloud_color(input: VertexOut, d: vec3<f32>) -> vec4<f32> {
    // The engine never draws clouds on the bottom face of the sky box.
    let a = abs(d);
    if (d.z < 0.0 && a.z >= a.x && a.z >= a.y) {
        discard;
    }
    let uv = apply_tc_mods(sky_cloud_uv(d, material.params.z), vec3<f32>(0.0));
    var stage_color = material.color;
    if ((material.header.z & 1u) != 0u) {
        stage_color = vec4<f32>(stage_color.rgb * input.color.rgb, stage_color.a);
    } else if ((material.header.z & 524288u) != 0u) {
        stage_color = vec4<f32>(stage_color.rgb * (vec3<f32>(1.0) - input.color.rgb), stage_color.a);
    }
    if ((material.header.z & 1048576u) != 0u) {
        stage_color.a = stage_color.a * input.color.a;
    } else if ((material.header.z & 2097152u) != 0u) {
        stage_color.a = stage_color.a * (1.0 - input.color.a);
    }
    stage_color = apply_wave_gens(stage_color);
    let base = textureSample(base_texture, base_sampler, uv) * stage_color;
    if ((material.params.x > 0.0 && base.a < material.params.x || material.params.x < 0.0 && base.a >= -material.params.x)) {
        discard;
    }
    return base;
}

@fragment fn fs_sky(input: VertexOut) -> @location(0) vec4<f32> {
    // Convert the renderer's [x,z,-y] direction back to JKA coordinates.
    let d = normalize(vec3<f32>(input.sky_dir_ao.x, -input.sky_dir_ao.z, input.sky_dir_ao.y));
    // The runtime override replaces sky cubemaps AND authored sky-cloud stages.
    // World geometry is never touched. Restore the OpenJK sky on the next frame
    // by switching this bit off; this requires no pipeline rebuild.
    if ((camera.render_flags.y & 2u) != 0u) {
        if (material.header.x == 4u) { discard; }
        return experimental_star_nest_sky(d);
    }
    if (material.header.x == 4u) {
        // A cloud-layer stage of the sky shader (tcGen sky cloud).
        return sky_cloud_color(input, d);
    }
    if ((material.header.w & 1u) == 0u) {
        // skyParms "-" has no outerbox. OpenJK draws no skybox here, so leave
        // the scene clear color visible (global-fog color when the BSP has one).
        discard;
    }

    let a = abs(d);
    if (a.x >= a.y && a.x >= a.z) {
        if (d.x >= 0.0) {
            let m = a.x;
            return textureSample(sky_rt, sky_sampler, sky_uv(-d.y / m, d.z / m));
        }
        let m = a.x;
        return textureSample(sky_lf, sky_sampler, sky_uv(d.y / m, d.z / m));
    }
    if (a.y >= a.x && a.y >= a.z) {
        if (d.y >= 0.0) {
            let m = a.y;
            return textureSample(sky_bk, sky_sampler, sky_uv(d.x / m, d.z / m));
        }
        let m = a.y;
        return textureSample(sky_ft, sky_sampler, sky_uv(-d.x / m, d.z / m));
    }
    if (d.z >= 0.0) {
        let m = a.z;
        return textureSample(sky_up, sky_sampler, sky_uv(-d.y / m, -d.x / m));
    }
    let m = a.z;
    return textureSample(sky_dn, sky_sampler, sky_uv(-d.y / m, d.x / m));
}


// Diagnostic-only line overlay. The pipeline reuses vs_main so procedural
// vertex motion (Snowflow/ocean) stays identical to the filled geometry.
@fragment
fn fs_wireframe() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 1.0, 0.0, 1.0);
}
