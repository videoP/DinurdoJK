// Shared by the visible native mesh and native eye probe. Bounds include
// the three FFT bands and the analytic swell, before applying ONE constant
// horizontal scale. Height, phase, foam history and spectrum are preserved.
@group(2) @binding(9) var<storage, read> jka_crest_bounds: vec4<f32>;

fn jka_crest_scale(wave: vec4<f32>, motion: vec4<f32>, mode: f32) -> f32 {
    return jka_crest_scale_from_bounds(jka_crest_bounds, motion.z,
        abs(wave.w * wave.z * motion.y), mode);
}
