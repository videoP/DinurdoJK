// Near-only depth caster for GodotGrass.
// Reuses the already-computed visible-blade record, but rasterizes the source
// one-triangle low blade so close contact shadows cost only three vertices/blade.

struct ShadowCaster {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
};

@group(0) @binding(0) var<uniform> shadow: ShadowCaster;

struct PreparedWords { words: array<u32>, };
@group(1) @binding(0) var<storage, read> prepared_instances: PreparedWords;

const PI: f32 = 3.14159265358979323846;
const PREPARED_WORDS: u32 = 11u;
const FULL_SHADOW_METERS: f32 = 4.0;
const MAX_SHADOW_METERS: f32 = 6.0;
const WORLD_SCALE: f32 = 1.0 / 64.0;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @builtin(instance_index) instance_index: u32,
};

@vertex
fn vs_main(input: VertexInput) -> @builtin(position) vec4<f32> {
    let base = input.instance_index * PREPARED_WORDS;
    let camera_crush = unpack2x16float(prepared_instances.words[base + 4u]);
    let camera_distance_m = camera_crush.x;

    // Avoid all remaining storage loads and transform work for high-LOD blades
    // outside the tiny useful contact-shadow radius.
    if (camera_distance_m >= MAX_SHADOW_METERS) {
        return vec4<f32>(2.0, 2.0, 2.0, 1.0);
    }

    let root_world = bitcast<vec3<f32>>(vec3<u32>(
        prepared_instances.words[base + 0u],
        prepared_instances.words[base + 1u],
        prepared_instances.words[base + 2u],
    ));
    let authored_height_scale = bitcast<f32>(prepared_instances.words[base + 3u] & 0xffffff00u);
    let hw = unpack2x16float(prepared_instances.words[base + 5u]);
    let turn = unpack2x16float(prepared_instances.words[base + 6u]);
    let bend0 = unpack2x16float(prepared_instances.words[base + 7u]);
    let bend1 = unpack2x16float(prepared_instances.words[base + 8u]);

    let height_factor = 1.0 - input.uv.y;
    let h2 = height_factor * height_factor;
    let turbulence = mix(bend0.y, bend1.x, h2);
    let uncrushed_bend = bend0.x * height_factor + turbulence;
    let bend_angle = mix(0.63 * PI, uncrushed_bend, camera_crush.y);
    let sin_bend = sin(bend_angle);
    let cos_bend = cos(bend_angle);

    // Collapse the proxy toward its root across the last two metres so the
    // deliberately short caster radius disappears without a hard popping ring.
    let range_fade = 1.0 - smoothstep(FULL_SHADOW_METERS, MAX_SHADOW_METERS, camera_distance_m);
    var vertex_m = input.position * authored_height_scale;
    vertex_m.x *= hw.y * range_fade;
    vertex_m.y *= hw.x * 1.20 * range_fade;

    let bent_vertex = vec3<f32>(
        vertex_m.x,
        cos_bend * vertex_m.y - sin_bend * vertex_m.z,
        sin_bend * vertex_m.y + cos_bend * vertex_m.z,
    );
    let vertex_m_turned = vec3<f32>(
        turn.y * bent_vertex.x + turn.x * bent_vertex.z,
        bent_vertex.y,
        -turn.x * bent_vertex.x + turn.y * bent_vertex.z,
    );
    let world_position = root_world + vertex_m_turned / WORLD_SCALE;
    return shadow.view_proj * vec4<f32>(world_position, 1.0);
}
