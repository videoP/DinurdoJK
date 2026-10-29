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
};

struct PlanarReflectionSettings {
    planes: array<vec4<f32>, 4>,
    viewport: vec4<f32>,
    debug: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var base_sampler: sampler;
@group(1) @binding(4) var<uniform> material: Material;
@group(2) @binding(2) var<uniform> planar_reflection: PlanarReflectionSettings;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) lightmap_uv: vec2<f32>,
    @location(5) normal: vec3<f32>,
    @location(3) color: vec4<f32>,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
};

struct MaskVertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) alpha_multiplier: f32,
    @location(2) world_position: vec3<f32>,
};

// JaPro: subtract the remap offset, then apply a positive clampTime.
// vector_t.w is reserved for the limit; zero leaves the clock unbounded.
fn shader_stage_time() -> f32 {
    let elapsed = camera.camera_pos_time.w - material.params.w;
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
        let view = normalize(camera.camera_pos_time.xyz - input.position);
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

@vertex
fn vs_main(input: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.clip_position = camera.view_proj * vec4<f32>(input.position, 1.0);
    out.world_position = input.position;
    return out;
}

@vertex
fn vs_mask(input: VertexIn) -> MaskVertexOut {
    var out: MaskVertexOut;
    out.clip_position = camera.view_proj * vec4<f32>(input.position, 1.0);
    out.uv = generated_uv(input);
    out.world_position = input.position;
    out.alpha_multiplier = material.color.a;
    if ((material.header.z & 1u) != 0u) {
        out.alpha_multiplier *= input.color.a;
    }
    return out;
}

struct FragmentOut {
    @location(0) linear_depth: f32,
    @location(1) motion_vector: vec2<f32>,
    // R: SSR eligible; G: active planar winner this frame;
    // B: cached roughness hint; A: probe available.
    @location(2) reflection_policy: vec4<f32>,
};

fn material_matches_planar_plane(selected_plane: vec4<f32>) -> bool {
    let material_len2 = dot(material.planar_plane.xyz, material.planar_plane.xyz);
    if (material_len2 <= 0.5) {
        return false;
    }
    let material_n = normalize(material.planar_plane.xyz);
    let selected_n = normalize(selected_plane.xyz);
    let alignment = dot(material_n, selected_n);
    let plane_delta = select(
        abs(material.planar_plane.w + selected_plane.w),
        abs(material.planar_plane.w - selected_plane.w),
        alignment >= 0.0
    );
    return abs(alignment) > 0.999 && plane_delta < 2.0;
}

fn active_planar_for_surface(world_position: vec3<f32>) -> bool {
    let planar_candidate = (material.header.z & (4096u | 8192u)) != 0u;
    if (!planar_candidate || planar_reflection.viewport.z <= 0.5) {
        return false;
    }
    let promoted_environment = (material.header.z & 8192u) != 0u;
    let active_count = min(u32(planar_reflection.viewport.z + 0.5), 4u);
    for (var slot = 0u; slot < 4u; slot = slot + 1u) {
        if (slot >= active_count) {
            break;
        }
        let selected_plane = planar_reflection.planes[slot];
        if (dot(selected_plane.xyz, selected_plane.xyz) <= 0.5) {
            continue;
        }
        if (promoted_environment) {
            let selected_n = normalize(selected_plane.xyz);
            if (abs(dot(selected_n, world_position) + selected_plane.w) < 0.5) {
                return true;
            }
        } else if (material_matches_planar_plane(selected_plane)) {
            return true;
        }
    }
    return false;
}

fn reflection_policy(world_position: vec3<f32>) -> vec4<f32> {
    let ssr_eligible = select(0.0, 1.0, (material.header.z & 4194304u) != 0u);
    let active_planar = select(0.0, 1.0, active_planar_for_surface(world_position));
    // Alpha is an Rgba8Unorm channel. Pack two exact bits into its four
    // representable thirds: bit 0 = reflection probe, bit 1 = BSP global fog.
    // This avoids allocating another full-resolution policy target merely to
    // reproduce OpenJK's legacy framebuffer-space global fog pass.
    let probe_bit = select(0u, 1u, (material.header.z & 2048u) != 0u);
    let global_fog_bit = select(0u, 2u, (material.header.z & 67108864u) != 0u);
    let packed_policy = f32(probe_bit | global_fog_bit) / 3.0;
    return vec4<f32>(
        ssr_eligible,
        active_planar,
        clamp(material.pbr_params0.w, 0.0, 1.0),
        packed_policy
    );
}

fn bevy_motion_vector(world_position: vec3<f32>) -> vec2<f32> {
    // Faithful to Bevy's MOTION_VECTOR_PREPASS convention: motion is computed
    // from unjittered current/previous clip positions and stored as a UV offset.
    let current_clip_t = camera.unjittered_view_proj * vec4<f32>(world_position, 1.0);
    let current_clip = current_clip_t.xy / current_clip_t.w;
    let previous_clip_t = camera.previous_unjittered_view_proj * vec4<f32>(world_position, 1.0);
    let previous_clip = previous_clip_t.xy / previous_clip_t.w;
    return (current_clip - previous_clip) * vec2<f32>(0.5, -0.5);
}

// Radial distance is not affine across a triangle, so interpolating a
// per-vertex distance overestimates it inside large polygons (a floor triangle
// under a third-person camera could read several times too far). World
// position is affine and interpolates exactly; take the distance per fragment.
fn radial_depth(world_position: vec3<f32>) -> f32 {
    return distance(world_position, camera.camera_pos_time.xyz);
}

// Only the thermal-specific prepass PSOs call this classifier.  The regular
// prepass retains roughness in policy.b for SSR and does no thermal work.
fn thermal_world_material() -> f32 {
    let kind = (material.header.w >> 8u) & 31u;
    var heat = 0.15; // unknown concrete/stone
    switch (kind) {
        case 1u, 2u: { heat = 0.24; }  // wood
        case 3u, 4u: { heat = 0.19; }  // metal: variable with heating, not automatically hot
        case 5u, 6u: { heat = 0.11; }  // grass / vegetation
        case 7u: { heat = 0.22; }      // dirt
        case 8u: { heat = 0.30; }      // sand
        case 9u: { heat = 0.20; }      // gravel
        case 10u: { heat = 0.07; }     // glass
        case 14u: { heat = 0.025; }   // snow
        case 17u: { heat = 0.13; }    // mud
        case 21u, 22u: { heat = 0.22; } // fabric/canvas
        case 24u, 25u: { heat = 0.23; } // rubber/plastic
        case 27u: { heat = 0.18; }    // carpet
        default: {}
    }
    if ((material.header.w & (1u << 17u)) != 0u) {
        heat = 0.065; // JKA glass material override
    }
    if ((material.header.z & 65536u) != 0u) {
        heat = 0.055; // promoted JKA water stage
    }
    // An additive or glow stage does NOT heat its entire parent material:
    // landing pads, consoles and similar shaders have a cool opaque base and
    // small animated emissive overlays. Those stages are replayed at their
    // authored UVs/rgbGen/blend into the thermal-only glow capture and sampled
    // per pixel in thermal_glow_heat (post.wgsl). Giving the base draw a heat
    // floor here would falsely heat the entire surface.
    // q3map_surfacelight is an explicit authored whole-surface emitter.
    if ((material.header.w & (1u << 5u)) != 0u) {
        heat = max(heat, 0.76);
    }
    // Authored surfaceparm lava and compiled CONTENTS_LAVA take priority over
    // generic water, glass and material hints, including emissive stage flags.
    if ((material.header.w & (1u << 3u)) != 0u) { return 0.80; }
    // BSP shader emission and stage glow are explicit hints; only the
    // active-only depth variant reads them. This is not a true heat source.
    if ((material.header.z & 256u) != 0u) {
        heat = max(heat, 0.72);
    }
    return heat;
}

@fragment
fn fs_main(input: VertexOut) -> FragmentOut {
    var out: FragmentOut;
    out.linear_depth = radial_depth(input.world_position);
    // SSR, fog, AO and depth consumers do not need Bevy motion vectors.
    // Only TAA samples this attachment. Avoid two extra 4x4 matrix transforms
    // per BSP pixel when TAA is off (the attachment remains bound for the
    // other prepass consumers; full MRT specialization can follow separately).
    out.motion_vector = vec2<f32>(0.0);
    if ((camera.render_flags.y & 4u) != 0u) {
        out.motion_vector = bevy_motion_vector(input.world_position);
    }
    out.reflection_policy = reflection_policy(input.world_position);
    return out;
}

@fragment
fn fs_thermal(input: VertexOut) -> FragmentOut {
    var out: FragmentOut;
    out.linear_depth = radial_depth(input.world_position);
    // SSR, fog, AO and depth consumers do not need Bevy motion vectors.
    // Only TAA samples this attachment. Avoid two extra 4x4 matrix transforms
    // per BSP pixel when TAA is off (the attachment remains bound for the
    // other prepass consumers; full MRT specialization can follow separately).
    out.motion_vector = vec2<f32>(0.0);
    if ((camera.render_flags.y & 4u) != 0u) {
        out.motion_vector = bevy_motion_vector(input.world_position);
    }
    out.reflection_policy = reflection_policy(input.world_position);
    out.reflection_policy.b = thermal_world_material();
    return out;
}

@fragment
fn fs_mask(input: MaskVertexOut) -> FragmentOut {
    let alpha = textureSample(base_texture, base_sampler, input.uv).a * input.alpha_multiplier;
    if ((material.params.x > 0.0 && alpha < material.params.x || material.params.x < 0.0 && alpha >= -material.params.x)) {
        discard;
    }
    var out: FragmentOut;
    out.linear_depth = radial_depth(input.world_position);
    // SSR, fog, AO and depth consumers do not need Bevy motion vectors.
    // Only TAA samples this attachment. Avoid two extra 4x4 matrix transforms
    // per BSP pixel when TAA is off (the attachment remains bound for the
    // other prepass consumers; full MRT specialization can follow separately).
    out.motion_vector = vec2<f32>(0.0);
    if ((camera.render_flags.y & 4u) != 0u) {
        out.motion_vector = bevy_motion_vector(input.world_position);
    }
    out.reflection_policy = reflection_policy(input.world_position);
    return out;
}

@fragment
fn fs_thermal_mask(input: MaskVertexOut) -> FragmentOut {
    let alpha = textureSample(base_texture, base_sampler, input.uv).a * input.alpha_multiplier;
    if ((material.params.x > 0.0 && alpha < material.params.x || material.params.x < 0.0 && alpha >= -material.params.x)) {
        discard;
    }
    var out: FragmentOut;
    out.linear_depth = radial_depth(input.world_position);
    // SSR, fog, AO and depth consumers do not need Bevy motion vectors.
    // Only TAA samples this attachment. Avoid two extra 4x4 matrix transforms
    // per BSP pixel when TAA is off (the attachment remains bound for the
    // other prepass consumers; full MRT specialization can follow separately).
    out.motion_vector = vec2<f32>(0.0);
    if ((camera.render_flags.y & 4u) != 0u) {
        out.motion_vector = bevy_motion_vector(input.world_position);
    }
    out.reflection_policy = reflection_policy(input.world_position);
    out.reflection_policy.b = thermal_world_material();
    return out;
}

// Rain-only linear-depth prepass. Unlike the general spatial-post prepass,
// these entry points write only radial depth (plus hardware depth).
@fragment
fn fs_rain(input: VertexOut) -> @location(0) f32 {
    return radial_depth(input.world_position);
}

@fragment
fn fs_rain_mask(input: MaskVertexOut) -> @location(0) f32 {
    let alpha = textureSample(base_texture, base_sampler, input.uv).a * input.alpha_multiplier;
    if ((material.params.x > 0.0 && alpha < material.params.x || material.params.x < 0.0 && alpha >= -material.params.x)) {
        discard;
    }
    return radial_depth(input.world_position);
}

// Depth-only alpha test for the Hi-Z early pass. Opaque surfaces need no
// fragment stage at all (`vs_main` alone); this is only for alpha-tested ones.
@fragment
fn fs_depth_mask(input: MaskVertexOut) {
    let alpha = textureSample(base_texture, base_sampler, input.uv).a * input.alpha_multiplier;
    if ((material.params.x > 0.0 && alpha < material.params.x || material.params.x < 0.0 && alpha >= -material.params.x)) {
        discard;
    }
}
