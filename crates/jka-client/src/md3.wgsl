struct Camera {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
    clip_plane: vec4<f32>,
    render_flags: vec4<u32>,
    camera_forward: vec4<f32>,
};

// Pipeline-specialized: false compiles the self-applied Legacy fog out entirely.
// Rebuilt only when FogSystem::legacy_self_fog toggles, never checked per frame.
override ENABLE_LEGACY_FOG: bool = false;

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var base_sampler: sampler;

struct EntityLightingSettings {
    values: vec4<u32>,
    // Self-applied Legacy fog: linear RGB, depthForOpaque.
    legacy_fog_color_depth: vec4<f32>,
    // x: 0 off, 1 authored global EXP2, 2 manual fog; y: strength scale.
    legacy_fog_params: vec4<f32>,
};
@group(1) @binding(2) var<uniform> entity_lighting: EntityLightingSettings;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) world_position: vec3<f32>,
};

@vertex
fn vs_main(input: VertexIn) -> VertexOut {
    var output: VertexOut;
    output.clip_position = camera.view_proj * vec4<f32>(input.position, 1.0);
    output.uv = input.uv;
    output.normal = normalize(input.normal);
    output.color = input.color;
    output.world_position = input.position;
    return output;
}

fn lit_color(normal: vec3<f32>, tex: vec4<f32>) -> vec4<f32> {
    // BSP LIGHTGRID is pre-applied to the transient vertex colors while the
    // CPU packs each dynamic surface. Preserve the previous fragment-lighting
    // placeholder only for irradiance-volume mode until dynamic models sample
    // the actual ambient-cube data.
    if (entity_lighting.values.x != 2u) {
        return tex;
    }
    let light_dir = normalize(vec3<f32>(0.35, 0.75, 0.55));
    let diffuse = max(dot(normalize(normal), light_dir), 0.0);
    let lighting = 0.32 + 0.68 * diffuse;
    return vec4<f32>(tex.rgb * lighting, tex.a);
}

// OpenJK r_drawfog 2 applies GL_EXP2 hardware fog to entities too: their fog
// num is the world's global fog, whose bounds cover the whole world. This is
// the same curve and colour decode as bsp.wgsl legacy_separate_fog, so a
// player and the floor at the same distance receive identical fog.
// Manual fog (no authored map fog) uses the same path in both Legacy modes;
// Legacy 1 authored global fog is instead applied in post from the prepass.
fn apply_self_legacy_fog(rgb: vec3<f32>, world_position: vec3<f32>, fog_to_black: bool) -> vec3<f32> {
    if (!ENABLE_LEGACY_FOG) {
        return rgb;
    }
    let mode = entity_lighting.legacy_fog_params.x;
    if (mode < 0.5) {
        return rgb;
    }
    let authored_depth = max(entity_lighting.legacy_fog_color_depth.a, 0.001);
    let scale = entity_lighting.legacy_fog_params.y;
    var amount = 0.0;
    if (mode < 1.5) {
        // GL_EXP2 density ln(255)^0.5 / depthForOpaque over eye-forward depth.
        let forward_distance = max(
            dot(world_position - camera.camera_pos_time.xyz, normalize(camera.camera_forward.xyz)),
            0.0
        );
        let scaled = forward_distance / authored_depth * scale;
        amount = 1.0 - exp(-5.5412635 * scaled * scaled);
    } else {
        let radial = distance(world_position, camera.camera_pos_time.xyz) / authored_depth;
        amount = 1.0 - exp(-radial * scale * 0.26);
    }
    // OpenJK mGLFogColorOverride: GL_ONE GL_ONE and GL_DST_COLOR GL_ZERO stages
    // fog toward black so the blend equation does not add/multiply fog colour.
    let fog_color = select(entity_lighting.legacy_fog_color_depth.rgb, vec3<f32>(0.0), fog_to_black);
    return mix(rgb, fog_color, clamp(amount, 0.0, 1.0));
}

fn fogged(color: vec4<f32>, world_position: vec3<f32>, fog_to_black: bool) -> vec4<f32> {
    return vec4<f32>(apply_self_legacy_fog(color.rgb, world_position, fog_to_black), color.a);
}

@fragment
fn fs_wireframe() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 1.0, 0.0, 1.0);
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let tex = textureSample(base_texture, base_sampler, input.uv);
    return fogged(lit_color(input.normal, tex) * input.color, input.world_position, false);
}

@fragment
fn fs_mask(input: VertexOut) -> @location(0) vec4<f32> {
    let tex = textureSample(base_texture, base_sampler, input.uv);
    if tex.a < 0.5 {
        discard;
    }
    return fogged(lit_color(input.normal, tex) * input.color, input.world_position, false);
}

@fragment
fn fs_unlit(input: VertexOut) -> @location(0) vec4<f32> {
    let tex = textureSample(base_texture, base_sampler, input.uv);
    return fogged(tex * input.color, input.world_position, false);
}

@fragment
fn fs_unlit_fog_black(input: VertexOut) -> @location(0) vec4<f32> {
    let tex = textureSample(base_texture, base_sampler, input.uv);
    return fogged(tex * input.color, input.world_position, true);
}

// Scene depth prepass. Opaque/alpha-tested entities join the BSP prepass so
// linear depth is the real nearest-opaque scene depth for fog, SSAO, DOF, etc.
// OpenJK gives every entity the world's global fog (its bounds cover the whole
// world), so entities always set the global-fog policy bit.
struct PrepassVertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_position: vec3<f32>,
};

struct PrepassOut {
    @location(0) linear_depth: f32,
    // Write-masked off by the pipeline: TAA keeps its existing entity behavior.
    @location(1) motion_vector: vec2<f32>,
    // No SSR/planar/probe; alpha packs bit 1 (global fog) as 2/3.
    @location(2) reflection_policy: vec4<f32>,
};

fn prepass_output(world_position: vec3<f32>) -> PrepassOut {
    var out: PrepassOut;
    out.linear_depth = distance(world_position, camera.camera_pos_time.xyz);
    out.motion_vector = vec2<f32>(0.0);
    out.reflection_policy = vec4<f32>(0.0, 0.0, 1.0, 2.0 / 3.0);
    return out;
}

@vertex
fn vs_prepass(input: VertexIn) -> PrepassVertexOut {
    var output: PrepassVertexOut;
    output.clip_position = camera.view_proj * vec4<f32>(input.position, 1.0);
    output.uv = input.uv;
    output.world_position = input.position;
    return output;
}

@fragment
fn fs_prepass(input: PrepassVertexOut) -> PrepassOut {
    return prepass_output(input.world_position);
}

@fragment
fn fs_prepass_mask(input: PrepassVertexOut) -> PrepassOut {
    if textureSample(base_texture, base_sampler, input.uv).a < 0.5 {
        discard;
    }
    return prepass_output(input.world_position);
}
