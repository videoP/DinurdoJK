// Diagnostic shader, never concatenated into ordinary BSP shader modules.
// Its VS comes from the existing BSP shader; only this FS is diagnostic.
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
    deform_move_vector: vec4<f32>,
    deform_move_wave: vec4<f32>,
};

@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var base_sampler: sampler;
@group(1) @binding(4) var<uniform> material: Material;

struct HeatmapVertexInput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

@fragment fn fs_cost(input: HeatmapVertexInput) -> @location(0) vec4<f32> {
    let sampled = textureSample(base_texture, base_sampler, input.uv);
    var stage_alpha = material.color.a;
    if ((material.header.z & 1048576u) != 0u) {
        stage_alpha *= input.color.a;
    } else if ((material.header.z & 2097152u) != 0u) {
        stage_alpha *= 1.0 - input.color.a;
    }
    let alpha = sampled.a * stage_alpha;
    let source_alpha = sampled.a * material.color.a;
    let mask_alpha = select(alpha, source_alpha, (material.header.z & 1073741824u) != 0u);
    if ((material.params.x > 0.0 && mask_alpha < material.params.x)
        || (material.params.x < 0.0 && mask_alpha >= -material.params.x)) {
        discard;
    }
    if (alpha <= 0.001) { discard; }
    // One additive contribution per *surviving fragment*.
    // No blend-alpha attenuation: even a very translucent pixel executes FS work.
    return vec4<f32>(max(material.params.z, 0.0), 0.0, 0.0, 0.0);
}

@vertex fn vs_composite(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

// Unreal Engine's documented ShaderComplexityColors (BaseEngine.ini).
// This is palette-only: the existing 0..24 diagnostic cost scale, Naga
// estimate and accumulated fragment counts are intentionally unchanged.
const UNREAL_SHADER_COMPLEXITY_COLORS: array<vec3<f32>, 9> = array<vec3<f32>, 9>(
    vec3<f32>(0.0,   1.0,   0.127),
    vec3<f32>(0.0,   1.0,   0.0),
    vec3<f32>(0.046, 0.52,  0.0),
    vec3<f32>(0.215, 0.215, 0.0),
    vec3<f32>(0.52,  0.046, 0.0),
    vec3<f32>(0.7,   0.0,   0.0),
    vec3<f32>(1.0,   0.0,   0.0),
    vec3<f32>(1.0,   0.0,   0.5),
    vec3<f32>(1.0,   0.9,   0.9),
);

@group(0) @binding(0) var cost_buffer: texture_2d<f32>;
@fragment fn fs_composite(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let cost = textureLoad(cost_buffer, vec2<i32>(pos.xy), 0).r;
    if (cost <= 0.001) { discard; }
    // Eight equal intervals over the pre-existing 0..24 heatmap cost range.
    let segment = clamp(cost / 3.0, 0.0, 8.0);
    let i = min(u32(segment), 7u);
    let fraction = segment - f32(i);
    let rgb = mix(UNREAL_SHADER_COMPLEXITY_COLORS[i],
                  UNREAL_SHADER_COMPLEXITY_COLORS[i + 1u], fraction);
    // Replace the scene color, as Unreal's debug view does, rather than
    // tinting it at 97% opacity (which changes the diagnostic colors).
    return vec4<f32>(rgb, 1.0);
}
