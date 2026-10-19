// Diagnostic only: sample exactly under the camera, DISPLAY the result 100
// JKA units in front. Same wave bind groups as the native Blue Water surface.
// All wave coordinates and heights are METRES relative to this pool's BSP plane.
// GPU-only, no readback, no change to optics, mesh selection, or game state.
#import naval::camera::{camera, world_to_clip, view_ray}
#import naval_ocean::waves::{displacement, height_at}
#import naval_ocean::wake::{wake_height, mesh_spacing}

struct MarkerVertex {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) sample_grid: vec2<f32>,
};

// All heights below are sampled at camera.position.xz, never at display_grid.
// Six 5px screen-facing line segments (36 vertices) form ONE camera gauge:
//   RED: eye to horizontally-corrected displaced water height
//   YELLOW: displaced surface estimate with horizontal choppiness inverted
//   ORANGE: native displacement at camera grid, before horizontal inversion
//   MAGENTA: exact native underwater EYE probe height_at(camera.xz) (no wake)
//   CYAN: original flat plane (height=0 in Blue Water metres)
//   GREEN/RED: camera eye height; green=under yellow estimate, red=above
// The inverse estimate is a visual diagnostic, not an authoritative mesh raycast;
// clipmap interpolation / LOD morph can still differ from the rasterized mesh.
@vertex
fn marker_vertex(@builtin(vertex_index) vertex_id: u32) -> MarkerVertex {
    var out: MarkerVertex;
    let sample_grid = camera.position.xz;
    let ray_forward = view_ray(vec2<f32>(0.0, 0.0)).xz;
    let forward = select(vec2<f32>(1.0, 0.0), ray_forward / max(length(ray_forward), 0.00001),
                         length(ray_forward) > 0.00001);
    let display_grid = sample_grid + forward * 2.5; // 100 JKA units / 40 units per metre

    // Raw native vertex displacement at CAMERA XZ, with native mesh LOD filtering.
    let raw = jka_hybrid_displacement(sample_grid, mesh_spacing(sample_grid));
    let raw_height = raw.y + wake_height(sample_grid);

    // The visible vertices are moved horizontally by displacement.xz. Invert
    // that mapping (three fixed-point iterations) to estimate the wave surface
    // whose final rasterized horizontal XZ is directly under the camera.
    var unmoved_grid = sample_grid;
    for (var step: u32 = 0u; step < 3u; step = step + 1u) {
        let d = jka_hybrid_displacement(unmoved_grid, mesh_spacing(unmoved_grid));
        unmoved_grid = sample_grid - d.xz;
    }
    let sampled = jka_hybrid_displacement(unmoved_grid, mesh_spacing(unmoved_grid));
    let wave_height = sampled.y + wake_height(unmoved_grid);

    // This is the *native probe eye channel* in shader_composer.rs, so if
    // this differs from yellow, the native probe and raster wave disagree.
    var probe_grid = sample_grid;
    var probe_d = jka_hybrid_displacement(probe_grid, mesh_spacing(probe_grid));
    for (var step: u32 = 0u; step < 6u; step = step + 1u) {
        probe_grid = sample_grid - probe_d.xz;
        probe_d = jka_hybrid_displacement(probe_grid, mesh_spacing(probe_grid));
    }
    let probe_height = probe_d.y;
    let eye_height = camera.position.y;
    let at = vec3<f32>(display_grid.x, 0.0, display_grid.y);
    let series = vertex_id / 6u;
    var a: vec3<f32>;
    var b: vec3<f32>;
    var color: vec4<f32>;
    if series == 0u {
        a = at + vec3<f32>(0.0, eye_height, 0.0);
        b = at + vec3<f32>(0.0, wave_height, 0.0);
        color = vec4<f32>(1.0, 0.06, 0.06, 1.0);
    } else if series == 1u {
        a = at + vec3<f32>(-0.33, wave_height, 0.0);
        b = at + vec3<f32>( 0.33, wave_height, 0.0);
        color = vec4<f32>(1.0, 0.95, 0.07, 1.0);
    } else if series == 2u {
        a = at + vec3<f32>(-0.22, raw_height, 0.0);
        b = at + vec3<f32>( 0.22, raw_height, 0.0);
        color = vec4<f32>(1.0, 0.44, 0.02, 1.0);
    } else if series == 3u {
        a = at + vec3<f32>(-0.27, probe_height, 0.0);
        b = at + vec3<f32>( 0.27, probe_height, 0.0);
        color = vec4<f32>(1.0, 0.05, 0.98, 1.0);
    } else if series == 4u {
        a = at + vec3<f32>(-0.30, 0.0, 0.0);
        b = at + vec3<f32>( 0.30, 0.0, 0.0);
        color = vec4<f32>(0.05, 0.91, 1.0, 1.0);
    } else {
        a = at + vec3<f32>(-0.25, eye_height, 0.0);
        b = at + vec3<f32>( 0.25, eye_height, 0.0);
        color = select(vec4<f32>(1.0, 0.05, 0.05, 1.0),
                       vec4<f32>(0.05, 1.0, 0.20, 1.0), wave_height > eye_height);
    }

    // World-space vertical placement; display-only shift in XZ above.
    // Thick lines stay visible behind water geometry (depth compare Always).
    let clip_a = world_to_clip(a);
    let clip_b = world_to_clip(b);
    let pa = clip_a.xy / max(clip_a.w, 0.00001);
    let pb = clip_b.xy / max(clip_b.w, 0.00001);
    let delta = pb - pa;
    let tangent = select(vec2<f32>(0.0, 1.0), delta / max(length(delta), 1e-4), dot(delta, delta) > 1e-8);
    let perpendicular = vec2<f32>(-tangent.y, tangent.x);
    let half_width = vec2<f32>(5.0 / max(camera.viewport.x, 1.0),
                              5.0 / max(camera.viewport.y, 1.0));
    let corner = vertex_id % 6u;
    let at_b = corner == 1u || corner == 2u || corner == 4u;
    let side = select(-1.0, 1.0, corner == 2u || corner == 4u || corner == 5u);
    var clip = select(clip_a, clip_b, at_b);
    // Naga WGSL disallows writing clip.xy as a swizzle lvalue.
    clip = vec4<f32>(clip.xy + perpendicular * half_width * side * clip.w, clip.z, clip.w);
    if clip_a.w <= 0.0 || clip_b.w <= 0.0 {
        clip = vec4<f32>(-10.0, -10.0, 0.0, 1.0);
    }
    out.clip = clip;
    out.color = color;
    out.sample_grid = sample_grid;
    return out;
}

@fragment
fn marker_fragment(input: MarkerVertex) -> @location(0) vec4<f32> {
    // Host admission mask only allows valid water pools. Fragment stage owns it.
    if !jka_water_contains(input.sample_grid) { discard; }
    return input.color;
}
