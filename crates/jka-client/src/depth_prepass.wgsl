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

    let time = camera.camera_pos_time.w;
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

@fragment
fn fs_main(input: VertexOut) -> FragmentOut {
    var out: FragmentOut;
    out.linear_depth = radial_depth(input.world_position);
    out.motion_vector = bevy_motion_vector(input.world_position);
    out.reflection_policy = reflection_policy(input.world_position);
    return out;
}

@fragment
fn fs_mask(input: MaskVertexOut) -> FragmentOut {
    let alpha = textureSample(base_texture, base_sampler, input.uv).a * input.alpha_multiplier;
    if (alpha < material.params.x) {
        discard;
    }
    var out: FragmentOut;
    out.linear_depth = radial_depth(input.world_position);
    out.motion_vector = bevy_motion_vector(input.world_position);
    out.reflection_policy = reflection_policy(input.world_position);
    return out;
}
