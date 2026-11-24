// A single GPU-resident particle record is uploaded at birth; one u32 draw
// indirection per *visible material stage* plus a 16-byte FX clock are the
// only per-presentation transfers. The sampler, fog and blend variants are
// intentionally the same as fx_sprite.wgsl.
struct Camera {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
    clip_plane: vec4<f32>,
    render_flags: vec4<u32>,
    camera_forward: vec4<f32>,
};
struct EntityLightingSettings {
    values: vec4<u32>,
    legacy_fog_color_depth: vec4<f32>,
    legacy_fog_params: vec4<f32>,
};
struct ResidentSpark {
    origin_birth: vec4<f32>,
    velocity_death: vec4<f32>,
    acceleration_rotation: vec4<f32>,
    rotation_alpha: vec4<f32>,
    size_rgb_parm: vec4<f32>,
    rgb_start_flags: vec4<f32>,
    rgb_end_class: vec4<f32>,
};
struct FxClock {
    now_ms: f32,
    show_saber_impact: u32,
    unused: vec2<u32>,
};

override ENABLE_LEGACY_FOG: bool = false;
override FORCE_FOG_BLACK: bool = false;
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var base_sampler: sampler;
@group(1) @binding(2) var<uniform> entity_lighting: EntityLightingSettings;
// Own group. Do NOT share it with Ghoul2, glass or weather bind groups.
@group(2) @binding(0) var<storage, read> residents: array<ResidentSpark>;
@group(2) @binding(1) var<storage, read> draw_slot_indices: array<u32>;
@group(2) @binding(2) var<uniform> fx_clock: FxClock;

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) world_position: vec3<f32>,
    @location(3) @interpolate(flat) alive: u32,
};

fn percent(flags: u32, parm: f32, birth: f32, death: f32, now: f32) -> f32 {
    let duration = max(death - birth, 1.0);
    var linear = 1.0;
    if ((flags & 1u) != 0u) { linear = 1.0 - (now - birth) / duration; }
    var other = 1.0;
    let curve = flags & 12u;
    if (curve == 4u && now > parm) {
        other = 1.0 - (now - parm) / max(death - parm, 0.001);
    } else if (curve == 12u) {
        if (now < parm) { other = (parm - now) / max(parm - birth, 0.001); }
        else { other = 0.0; }
    } else if (curve != 4u) {
        return linear;
    }
    if ((flags & 1u) != 0u) { return (linear + other) * 0.5; }
    return other;
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VertexOut {
    let corners = array<vec2<f32>, 6>(
        vec2<f32>( 1.0,  1.0), vec2<f32>(-1.0,  1.0),
        vec2<f32>( 1.0, -1.0), vec2<f32>( 1.0, -1.0),
        vec2<f32>(-1.0,  1.0), vec2<f32>(-1.0, -1.0));
    let uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0));
    // The two high bits encode the per-stage identityLight multiplier.
    // They are draw metadata, not part of the persistent particle slot ID.
    let stage_slot = draw_slot_indices[ii];
    let p = residents[stage_slot & 0x3fffffffu];
    let identity_light = select(select(1.0, 0.5, (stage_slot & 0x40000000u) != 0u),
        0.21404114, (stage_slot & 0x80000000u) != 0u);
    let birth = p.origin_birth.w;
    let death = p.velocity_death.w;
    let alive = fx_clock.now_ms >= birth && fx_clock.now_ms <= death &&
        !(p.rgb_end_class.w > 0.5 && fx_clock.show_saber_impact == 0u);
    let dt = max(fx_clock.now_ms - birth, 0.0) * 0.001;
    // Analytic presentation has no GPU compute dispatch. The reference FX
    // CPU uses semi-implicit Euler, so accelerating particles may differ by
    // a fraction of a unit. The control stays opt-in until A/B validation.
    let center = p.origin_birth.xyz + p.velocity_death.xyz * dt +
        0.5 * p.acceleration_rotation.xyz * dt * dt;
    let flags = bitcast<u32>(p.rgb_start_flags.w);
    let birth_now = clamp(fx_clock.now_ms, birth, death);
    let size_pct = percent((flags >> 8u) & 15u, p.size_rgb_parm.z, birth, death, birth_now);
    let radius = mix(p.size_rgb_parm.y, p.size_rgb_parm.x, size_pct);
    let alpha_pct = percent(flags & 15u, p.rotation_alpha.w, birth, death, birth_now);
    let alpha = clamp(mix(p.rotation_alpha.z, p.rotation_alpha.y, alpha_pct), 0.0, 1.0);
    let rgb_pct = percent((flags >> 4u) & 15u, p.size_rgb_parm.w, birth, death, birth_now);
    let rgb = mix(p.rgb_end_class.xyz, p.rgb_start_flags.xyz, rgb_pct);
    // The CPU rotate() step applies 0.01*frame_ms*delta and damps delta by
    // 1-0.0007*frame_ms. Continuous-time integral approximates that at high FPS.
    let angle = radians(p.acceleration_rotation.w +
        p.rotation_alpha.x * (10.0 / 0.7) * (1.0 - exp(-0.7 * dt)));
    let s = sin(angle);
    let c = cos(angle);
    let forward = normalize(camera.camera_forward.xyz);
    // render-space +Y is JKA +Z; the camera has no roll.
    let left_axis = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), forward));
    let up_axis = normalize(cross(forward, left_axis));
    let left = radius * (left_axis * c - up_axis * s);
    let up = radius * (up_axis * c + left_axis * s);
    let world = center + left * corners[vi].x + up * corners[vi].y;
    var output: VertexOut;
    output.clip_position = camera.view_proj * vec4<f32>(world, 1.0);
    output.uv = uvs[vi];
    // The CPU FX emitter quantizes vertex color into bytes. The shader uses
    // matching 8-bit floors. Without useAlpha, the alpha value tints RGB.
    let rgba_rgb = floor(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0);
    let quant_alpha = floor(alpha * 255.0);
    let use_alpha = (flags & 0x08000000u) != 0u;
    let color_rgb = select(floor(rgba_rgb * quant_alpha / 256.0), rgba_rgb, use_alpha) / 255.0;
    output.color = vec4<f32>(color_rgb * identity_light, select(0.0, quant_alpha / 255.0, use_alpha));
    output.world_position = world;
    output.alive = select(0u, 1u, alive);
    return output;
}

fn apply_self_legacy_fog(rgb: vec3<f32>, world_position: vec3<f32>, fog_to_black: bool) -> vec3<f32> {
    if (!ENABLE_LEGACY_FOG) { return rgb; }
    let mode = entity_lighting.legacy_fog_params.x;
    if (mode < 0.5) { return rgb; }
    let authored_depth = max(entity_lighting.legacy_fog_color_depth.a, 0.001);
    let scale = entity_lighting.legacy_fog_params.y;
    var amount = 0.0;
    if (mode < 1.5) {
        let forward_distance = max(dot(world_position - camera.camera_pos_time.xyz,
            normalize(camera.camera_forward.xyz)), 0.0);
        let scaled = forward_distance / authored_depth * scale;
        amount = 1.0 - exp(-5.5412635 * scaled * scaled);
    } else {
        let radial = distance(world_position, camera.camera_pos_time.xyz) / authored_depth;
        amount = 1.0 - exp(-radial * scale * 0.26);
    }
    let fog_color = select(entity_lighting.legacy_fog_color_depth.rgb, vec3<f32>(0.0), fog_to_black);
    return mix(rgb, fog_color, clamp(amount, 0.0, 1.0));
}

fn fogged(color: vec4<f32>, world_position: vec3<f32>, fog_to_black: bool) -> vec4<f32> {
    return vec4<f32>(apply_self_legacy_fog(color.rgb, world_position, fog_to_black || FORCE_FOG_BLACK), color.a);
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    if (input.alive == 0u) { discard; }
    return fogged(textureSample(base_texture, base_sampler, input.uv) * input.color, input.world_position, false);
}
@fragment
fn fs_mask(input: VertexOut) -> @location(0) vec4<f32> {
    if (input.alive == 0u) { discard; }
    let tex = textureSample(base_texture, base_sampler, input.uv);
    if (tex.a < 0.5) { discard; }
    return fogged(tex * input.color, input.world_position, false);
}
@fragment
fn fs_unlit(input: VertexOut) -> @location(0) vec4<f32> {
    if (input.alive == 0u) { discard; }
    return fogged(textureSample(base_texture, base_sampler, input.uv) * input.color, input.world_position, false);
}
@fragment
fn fs_unlit_zero_alpha(input: VertexOut) -> @location(0) vec4<f32> {
    if (input.alive == 0u) { discard; }
    let color = textureSample(base_texture, base_sampler, input.uv) * input.color;
    if (color.a <= 0.0) { discard; }
    return fogged(color, input.world_position, false);
}
@fragment
fn fs_unlit_fog_black(input: VertexOut) -> @location(0) vec4<f32> {
    if (input.alive == 0u) { discard; }
    return fogged(textureSample(base_texture, base_sampler, input.uv) * input.color, input.world_position, true);
}
