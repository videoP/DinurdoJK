#import naval_sky::environment::{SkyEnvironment, env_irradiance}
// Change-triggered vertex-lit BSP sky bake. This uses the same Blue Water
// environment/normal/exposure equation as the lightmap compositor, but never
// runs in a frame's draw shaders. GpuVertex is 15 packed f32 (60 bytes); RGB
// begins at word 10. Only the statically collected receiver indices are written.
struct SkyVertex {
    gpu_index: u32,
    packed: u32,
    reserved: vec2<u32>,
    authored_color: vec4<f32>,
}
struct Settings {
    moon: vec4<f32>,
    flags: vec4<u32>, // x active, y moon, z strength bits; w bits 0 vanilla, 1 isolation, 2+ receiver offset
}
@group(0) @binding(0) var<storage, read> receivers: array<SkyVertex>;
@group(0) @binding(1) var<storage, read_write> world_vertex_words: array<u32>;
@group(0) @binding(2) var sky_atlas: texture_2d<f32>;
@group(0) @binding(3) var sky_atlas_sampler: sampler;
@group(0) @binding(4) var<uniform> sky_environment: SkyEnvironment;
@group(0) @binding(5) var<uniform> settings: Settings;

fn sky_octahedral_decode(e: vec2<f32>) -> vec3<f32> {
    var n = vec3(e.x, e.y, 1.0 - abs(e.x) - abs(e.y));
    let t = max(-n.z, 0.0);
    n.x += select(t, -t, n.x >= 0.0);
    n.y += select(t, -t, n.y >= 0.0);
    return normalize(n);
}
// Shared analytic sky gain is included in the Rust shader builder.
fn vertex_sky_gain(packed: u32) -> vec3<f32> {
    if (settings.flags.x == 0u || (packed & 0x80000000u) == 0u) { return vec3(1.0); }
    let e = vec2(f32((packed >> 11u) & 1023u), f32((packed >> 21u) & 1023u)) / 1023.0 * 2.0 - 1.0;
    let n = sky_octahedral_decode(e);
    let ambient_visibility = f32(packed & 63u) / 63.0;
    if (settings.flags.w & 1u) != 0u {
        return vanilla_sky_gain(n, settings.moon.xyz, ambient_visibility,
            clamp(bitcast<f32>(settings.flags.z), 0.0, 1.0));
    }
    let direct_visibility = f32((packed >> 6u) & 31u) / 31.0;
    let ambient = env_irradiance(sky_atlas, sky_atlas_sampler, sky_environment, n) / 3.14159265 * ambient_visibility;
    let moon_active = settings.flags.y != 0u;
    let direction = select(sky_environment.light_direction, settings.moon.xyz, moon_active);
    let color = select(sky_environment.light_color, vec3<f32>(0.70, 0.82, 1.0), moon_active);
    let intensity = select(sky_environment.light_intensity, settings.moon.w, moon_active);
    let direct = color * intensity * max(dot(n, direction), 0.0) * direct_visibility;
    let full_gain = clamp(ambient * 0.5 + direct * 0.2, vec3(0.03), vec3(4.0));
    let incidence = max(dot(n, direction), 0.0);
    let exposure = clamp(max(ambient_visibility, direct_visibility * incidence), 0.0, 1.0);
    let isolation = (settings.flags.w & 2u) != 0u;
    let weight = select(1.0, exposure, isolation);
    return mix(vec3(1.0), full_gain,
        clamp(bitcast<f32>(settings.flags.z), 0.0, 1.0) * weight);
}
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = id.x + (settings.flags.w >> 2u);
    if (index >= arrayLength(&receivers)) { return; }
    let receiver = receivers[index];
    let gain = vertex_sky_gain(receiver.packed);
    let offset = receiver.gpu_index * 15u + 10u;
    world_vertex_words[offset] = bitcast<u32>(receiver.authored_color.x * gain.x);
    world_vertex_words[offset + 1u] = bitcast<u32>(receiver.authored_color.y * gain.y);
    world_vertex_words[offset + 2u] = bitcast<u32>(receiver.authored_color.z * gain.z);
}
