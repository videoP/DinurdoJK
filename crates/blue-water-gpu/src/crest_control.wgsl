// Shared by the visible native mesh and native eye probe. Bounds include
// the three FFT bands and the analytic swell, before applying ONE constant
// horizontal scale. Height, phase, foam history and spectrum are preserved.
@group(2) @binding(9) var<storage, read> jka_crest_bounds: vec4<f32>;

fn jka_crest_scale(wave: vec4<f32>, motion: vec4<f32>, mode: f32) -> f32 {
    if mode < 0.5 { return 1.0; }
    // The stochastic copies have unit squared weights, not unit sum.
    // sqrt(3) bounds their amplification for the two scattered bands.
    var bound = dot(jka_crest_bounds.xyz, vec3<f32>(1.732051, 1.732051, 1.0));
    if mode > 1.5 && jka_crest_bounds.w > 0.0 { bound = jka_crest_bounds.w; }
    let fft = bound * motion.z;
    let analytic = abs(wave.w * wave.z * motion.y);
    // Mode 1 is a conservative comparison. Mode 2 allows a small crest
    // overhang, instead of the unbounded loops from the original chop.
    let budget = select(0.9, 1.15, mode > 1.5);
    return min(1.0, budget / max(fft + analytic, 1e-5));
}
