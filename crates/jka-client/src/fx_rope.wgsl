// Visual-only grapple rope: dedicated, lazy GPU vertex-shader path.
// 24 joined segments x 6 vertices, two instances (halo and cable core).
// This shader does not run in the regular FX sprite or Minimal pipelines.
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
override ENABLE_LEGACY_FOG: bool = false;
override FORCE_FOG_BLACK: bool = false;
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var base_sampler: sampler;
@group(1) @binding(2) var<uniform> entity_lighting: EntityLightingSettings;

struct InstanceIn {
    // start.xyz, layer (2 = halo, 3 = core)
    @location(0) origin: vec4<f32>,
    // hook.xyz, visual spring phase
    @location(1) left: vec4<f32>,
    // visual spring offset.xyz, observed hook speed
    @location(2) up: vec4<f32>,
    @location(3) color: vec4<f32>,
};
struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) world_position: vec3<f32>,
    @location(3) layer: f32,
};

const PI: f32 = 3.141592653589793;
const TAU: f32 = 6.283185307179586;
const SEGMENTS: f32 = 24.0;

// The position is analytic in t, avoiding per-section position uploads and
// cracks. Both endpoints are returned exactly without any oscillator offset.
fn rope_point(input: InstanceIn, t: f32) -> vec3<f32> {
    let a = input.origin.xyz;
    let b = input.left.xyz;
    if (t <= 0.0) { return a; }
    if (t >= 1.0) { return b; }
    let ray = b - a;
    let size = max(length(ray), 0.001);
    let direction = ray / size;
    // render-space +Y = JKA +Z; match the CPU reference basis.
    let reference = select(vec3<f32>(0.0, 1.0, 0.0),
        vec3<f32>(0.0, 0.0, -1.0), abs(direction.y) > 0.94);
    let side = normalize(cross(direction, reference));
    let normal = cross(direction, side);
    let spring = input.up.xyz - direction * dot(input.up.xyz, direction);
    let flight = clamp(input.up.w / 650.0, 0.0, 1.0);
    let wave = (0.12 + 2.8 * flight) * clamp(size / 240.0, 0.0, 1.0);
    let droop = min(size * (0.008 + 0.007 * flight), 12.0);
    let packet = TAU * (2.25 * t) - input.left.w;
    let transverse = side * (wave * sin(packet)) +
        normal * (0.55 * wave * sin(packet * 0.76 + 0.75));
    return a + ray * t + (spring * 0.85 + transverse) * sin(PI * t) -
        vec3<f32>(0.0, 4.0 * droop * t * (1.0 - t), 0.0);
}

@vertex
fn vs_main(input: InstanceIn, @builtin(vertex_index) vertex_index: u32) -> VertexOut {
    // Each adjacent quad evaluates the exact same node positions AND tangent
    // on the GPU. No independently aligned strip corners / hard V joins.
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
        vec2<f32>(1.0, -1.0), vec2<f32>(1.0, -1.0),
        vec2<f32>(-1.0, 1.0), vec2<f32>(-1.0, -1.0));
    let corner = corners[vertex_index % 6u];
    let segment = vertex_index / 6u;
    let node = segment + select(1u, 0u, corner.y > 0.0);
    let t = f32(node) / SEGMENTS;
    let center = rope_point(input, t);
    let earlier = rope_point(input, max(0.0, t - 1.0 / SEGMENTS));
    let later = rope_point(input, min(1.0, t + 1.0 / SEGMENTS));
    let tangent = normalize(later - earlier);
    let to_view = camera.camera_pos_time.xyz - center;
    var across = cross(tangent, to_view);
    if (dot(across, across) < 0.0001) {
        let reference = select(vec3<f32>(0.0, 1.0, 0.0),
            vec3<f32>(0.0, 0.0, -1.0), abs(tangent.y) > 0.94);
        across = cross(tangent, reference);
    }
    across = normalize(across);
    let width = select(0.95, 2.05, input.origin.w < 2.5);
    let world = center + across * corner.x * width;
    var out: VertexOut;
    out.clip_position = camera.view_proj * vec4<f32>(world, 1.0);
    out.uv = vec2<f32>(select(1.0, 0.0, corner.x > 0.0), t);
    out.color = input.color;
    out.world_position = world;
    out.layer = input.origin.w;
    return out;
}

fn rope_fog(rgb: vec3<f32>, point: vec3<f32>, fog_black: bool) -> vec3<f32> {
    if (!ENABLE_LEGACY_FOG) { return rgb; }
    let mode = entity_lighting.legacy_fog_params.x;
    if (mode < 0.5) { return rgb; }
    let authored_depth = max(entity_lighting.legacy_fog_color_depth.a, 0.001);
    let scale = entity_lighting.legacy_fog_params.y;
    var amount = 0.0;
    if (mode < 1.5) {
        let distance = max(dot(point - camera.camera_pos_time.xyz,
            normalize(camera.camera_forward.xyz)), 0.0);
        let scaled = distance / authored_depth * scale;
        amount = 1.0 - exp(-5.5412635 * scaled * scaled);
    } else {
        let radial = distance(point, camera.camera_pos_time.xyz) / authored_depth;
        amount = 1.0 - exp(-radial * scale * 0.26);
    }
    let fog_color = select(entity_lighting.legacy_fog_color_depth.rgb,
        vec3<f32>(0.0), fog_black || FORCE_FOG_BLACK);
    return mix(rgb, fog_color, clamp(amount, 0.0, 1.0));
}

fn rope_color(input: VertexOut, fog_black: bool) -> vec4<f32> {
    let tex = textureSample(base_texture, base_sampler, input.uv);
    let radial = abs(input.uv.x * 2.0 - 1.0);
    let halo = input.layer < 2.5;
    // Smooth, feathered glow and a reasonably opaque cable center. Alpha
    // fades to zero at the same continuous ribbon boundaries in both layers.
    let feather = select(1.0 - smoothstep(0.76, 1.0, radial),
        exp(-radial * radial * 3.4) * (1.0 - smoothstep(0.94, 1.0, radial)), halo);
    let rgb = rope_fog(tex.rgb * input.color.rgb, input.world_position, fog_black);
    return vec4<f32>(rgb, tex.a * input.color.a * feather);
}

@fragment
fn fs_unlit(input: VertexOut) -> @location(0) vec4<f32> {
    return rope_color(input, false);
}
@fragment
fn fs_unlit_zero_alpha(input: VertexOut) -> @location(0) vec4<f32> {
    let color = rope_color(input, false);
    if (color.a <= 0.0) { discard; }
    return color;
}
@fragment
fn fs_unlit_fog_black(input: VertexOut) -> @location(0) vec4<f32> {
    return rope_color(input, true);
}

@fragment
fn fs_wireframe() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 1.0, 0.0, 1.0);
}
