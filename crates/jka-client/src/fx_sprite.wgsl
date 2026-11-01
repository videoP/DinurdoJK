struct Camera {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
    clip_plane: vec4<f32>,
    render_flags: vec4<u32>,
    camera_forward: vec4<f32>,
};

override ENABLE_LEGACY_FOG: bool = false;
// OpenJK Dynamic Glow temporarily overrides the fog colour to black while
// replaying explicitly-authored `glow` stages. Pipeline-specialize that replay
// rather than changing material classification or blend behavior.
override FORCE_FOG_BLACK: bool = false;

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var base_sampler: sampler;

struct EntityLightingSettings {
    values: vec4<u32>,
    legacy_fog_color_depth: vec4<f32>,
    legacy_fog_params: vec4<f32>,
};
@group(1) @binding(2) var<uniform> entity_lighting: EntityLightingSettings;

struct InstanceIn {
    @location(0) origin: vec4<f32>,
    @location(1) left: vec4<f32>,
    @location(2) up: vec4<f32>,
    @location(3) color: vec4<f32>,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) world_position: vec3<f32>,
    @location(3) lightning_layer: f32,
    @location(4) lightning_time: f32,
};

// GPU-generated Force Lightning: 24 linked view-facing ribbon sections.
// Endpoint and EFX color/texture/alpha are authored by the original effect.
// The four layers are corona, conductor, white-hot filament and one fork.
// Vertex-built geometry avoids a compute pass, transient vertex allocations,
// CPU segment tessellation and per-frame ray/mesh readbacks.
fn bolt_hash(n: f32) -> f32 {
    return fract(sin(n * 127.1 + 78.233) * 43758.5453);
}

fn bolt_point(a: vec3<f32>, b: vec3<f32>, t: f32, seed: f32, chaos: f32, sample_time: f32) -> vec3<f32> {
    let ray = b - a;
    let bolt_len = max(length(ray), 0.001);
    let direction = ray / bolt_len;
    let reference = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0), abs(direction.y) > 0.9);
    let side = normalize(cross(direction, reference));
    let other = normalize(cross(direction, side));
    // Each vertex uses the same integer node and phase hash. Adjacent
    // segments therefore meet exactly; there are no single-frame tears.
    let node = round(t * 24.0);
    let step = sample_time * 34.0;
    let phase = floor(step);
    let blend = smoothstep(0.10, 0.88, fract(step));
    let key = node * 27.113 + seed * 91.39;
    let j0 = vec2<f32>(bolt_hash(key + phase * 11.31), bolt_hash(key + phase * 15.17));
    let j1 = vec2<f32>(bolt_hash(key + (phase + 1.0) * 11.31), bolt_hash(key + (phase + 1.0) * 15.17));
    let jitter = mix(j0, j1, blend) * 2.0 - vec2<f32>(1.0);
    // Pin BOTH endpoints. Clamp displacement in long-range FX so arcs
    // remain legible, don't jump across rooms or disconnect from hands.
    let envelope = pow(max(sin(t * 3.14159265), 0.0), 0.75);
    let amplitude = min(15.0, bolt_len * 0.08 + 1.5) * (1.0 + min(chaos, 10.0) * 0.03);
    return mix(a, b, t) + (side * jitter.x + other * jitter.y) * amplitude * envelope;
}

fn lightning_vertex(input: InstanceIn, vertex_index: u32) -> VertexOut {
    let corner = array<vec2<f32>, 6>(
        vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
        vec2<f32>(1.0, -1.0), vec2<f32>(1.0, -1.0),
        vec2<f32>(-1.0, 1.0), vec2<f32>(-1.0, -1.0));
    let quad = vertex_index / 6u;
    let local = corner[vertex_index % 6u];
    let along = (f32(quad) + select(1.0, 0.0, local.y > 0.0)) / 24.0;
    let branch = input.up.y > 3.5 && input.up.y < 4.5;
    let t = select(along, mix(0.38, 0.78, along), branch);
    let a = input.origin.xyz;
    let b = input.left.xyz;
    var center = bolt_point(a, b, t, input.left.w, input.up.z, input.up.w);
    if (branch) {
        let ray = normalize(b - a);
        let side = normalize(cross(ray, select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0), abs(ray.y) > 0.9)));
        let angle = sin(along * 3.14159265);
        center += side * angle * (1.5 + 0.025 * distance(a, b));
    }
    let direction = normalize(b - a);
    let view = normalize(camera.camera_pos_time.xyz - center);
    var across = cross(direction, view);
    if (dot(across, across) < 0.0001) {
        across = cross(direction, select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0), abs(direction.y) > 0.9));
    }
    across = normalize(across);
    let taper = 0.45 + 0.55 * pow(max(sin(along * 3.14159265), 0.0), 0.3);
    let world = center + across * local.x * input.up.x * taper;
    let pulse = 0.82 + 0.18 * sin(input.up.w * 46.0 + input.left.w * 19.0);
    let filament = input.up.y > 2.5 && input.up.y < 3.5;
    var output: VertexOut;
    output.lightning_layer = input.up.y;
    output.lightning_time = input.up.w;
    if (input.up.y > 4.5) {
        // Endpoint flash: only quad zero is visible. The remaining vertices
        // collapse outside the clip volume, so the 144-vertex bolt pipeline
        // also renders one soft camera-facing impact corona per conductor.
        let flare_view = normalize(camera.camera_pos_time.xyz - b);
        let basis = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 1.0, 0.0), abs(flare_view.z) > 0.9);
        let flare_right = normalize(cross(flare_view, basis));
        let flare_up = cross(flare_right, flare_view);
        let flash = input.up.x * 2.8 * pulse;
        let flare_world = b + flash * (flare_right * local.x + flare_up * local.y);
        output.clip_position = select(camera.view_proj * vec4<f32>(flare_world, 1.0),
            vec4<f32>(-3.0, -3.0, 0.0, 1.0), quad > 0u);
        output.uv = vec2<f32>(0.5) + local * 0.5;
        output.color = vec4<f32>(mix(input.color.rgb, vec3<f32>(1.0), 0.62), input.color.a * pulse);
        output.world_position = flare_world;
        return output;
    }
    output.clip_position = camera.view_proj * vec4<f32>(world, 1.0);
    output.uv = vec2<f32>(select(1.0, 0.0, local.x > 0.0), t);
    output.color = vec4<f32>(mix(input.color.rgb, vec3<f32>(1.0), select(0.0, 0.82, filament)),
        input.color.a * pulse);
    output.world_position = world;
    return output;
}

@vertex
fn vs_main(input: InstanceIn, @builtin(vertex_index) vertex_index: u32) -> VertexOut {
    if (input.origin.w > 0.5) {
        return lightning_vertex(input, vertex_index);
    }

    // CPU RB_AddQuadStamp order: (v0,v1,v3), (v3,v1,v2). Cull is disabled
    // for this pipeline, so the duplicate reverse-winding CPU triangles are
    // unnecessary while preserving the exact visible quad.
    let corners = array<vec2<f32>, 6>(
        vec2<f32>( 1.0,  1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>(-1.0, -1.0),
    );
    let uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vertex_index];
    let world = input.origin.xyz + input.left.xyz * corner.x + input.up.xyz * corner.y;
    var output: VertexOut;
    output.clip_position = camera.view_proj * vec4<f32>(world, 1.0);
    output.uv = uvs[vertex_index];
    output.color = input.color;
    output.world_position = world;
    // Negative origin.w marks one of the compute-generated plasma layers.
    // Regular FX quad instances keep their exact legacy texturing.
    output.lightning_layer = max(-input.origin.w, 0.0);
    output.lightning_time = 0.0;
    return output;
}

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
    let fog_color = select(entity_lighting.legacy_fog_color_depth.rgb, vec3<f32>(0.0), fog_to_black);
    return mix(rgb, fog_color, clamp(amount, 0.0, 1.0));
}

fn fogged(color: vec4<f32>, world_position: vec3<f32>, fog_to_black: bool) -> vec4<f32> {
    return vec4<f32>(apply_self_legacy_fog(color.rgb, world_position, fog_to_black || FORCE_FOG_BLACK), color.a);
}

// A lightning surface is deliberately NOT just a fully opaque line texture.
// Each radial energy field fades continuously, giving a bright conductor,
// white core, and very broad, low-intensity halo without relying on bloom.
// Optional scene bloom picks up the emissive center naturally.
fn energy_color(input: VertexOut, tex: vec4<f32>, fog_black: bool) -> vec4<f32> {
    if (input.lightning_layer < 0.5) {
        return fogged(tex * input.color, input.world_position, fog_black);
    }
    // The narrow hot filament is still a line, not a 2-D circular sprite.
    // Previous code mistakenly treated layer 3 as a flare, visually erasing
    // its angular detail and turning many sections into small glowing blobs.
    let flare = input.lightning_layer > 4.5;
    let radial = select(abs(2.0 * input.uv.x - 1.0),
        length((input.uv - vec2<f32>(0.5)) * 2.0), flare);
    let field = exp(-radial * radial * select(6.8, 5.0, flare));
    let micro = 0.91 + 0.09 * sin(input.uv.y * 71.0 + input.lightning_time * 43.0);
    let center_hot = (input.lightning_layer > 1.5 && input.lightning_layer < 2.5) || (input.lightning_layer > 2.5 && input.lightning_layer < 3.5);
    let rgb = input.color.rgb * field * micro * select(1.0, 1.55, center_hot);
    // Preserve authored texture modulation without reintroducing hard opaque
    // line edges, including under additive blend materials.
    let material = 0.65 + 0.35 * max(tex.r, max(tex.g, tex.b));
    let alpha = input.color.a * field * micro * (0.65 + 0.35 * tex.a);
    return fogged(vec4<f32>(rgb * material, alpha), input.world_position, fog_black);
}

@fragment
fn fs_wireframe() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 1.0, 0.0, 1.0);
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    return energy_color(input, textureSample(base_texture, base_sampler, input.uv), false);
}

@fragment
fn fs_mask(input: VertexOut) -> @location(0) vec4<f32> {
    let tex = textureSample(base_texture, base_sampler, input.uv);
    if tex.a < 0.5 {
        discard;
    }
    return energy_color(input, tex, false);
}

@fragment
fn fs_unlit(input: VertexOut) -> @location(0) vec4<f32> {
    return energy_color(input, textureSample(base_texture, base_sampler, input.uv), false);
}

@fragment
fn fs_unlit_zero_alpha(input: VertexOut) -> @location(0) vec4<f32> {
    let color = energy_color(input, textureSample(base_texture, base_sampler, input.uv), false);
    // Exact-zero only: this is an A/B for guaranteed transparent source pixels,
    // not an alpha cutoff. Bilinear edge samples with any non-zero alpha survive.
    if color.a <= 0.0 {
        discard;
    }
    return color;
}

@fragment
fn fs_unlit_fog_black(input: VertexOut) -> @location(0) vec4<f32> {
    return energy_color(input, textureSample(base_texture, base_sampler, input.uv), true);
}
