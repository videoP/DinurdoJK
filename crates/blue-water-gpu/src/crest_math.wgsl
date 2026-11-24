// Horizontal mapping only. Modes 0-2 retain the existing A/B behavior.
// Mode 3 uses the existing conservative budget and the larger measured
// bound; it never deliberately budgets an overhang. The reduction is still
// a sampled estimate for the stochastic/coastal surface, not a proof.
fn jka_crest_scale_from_bounds(bounds: vec4<f32>, fft_mix: f32, analytic: f32, mode: f32) -> f32 {
    if mode < 0.5 { return 1.0; }
    let band_bound = dot(bounds.xyz, vec3<f32>(1.732051, 1.732051, 1.0));
    var bound = band_bound;
    var budget = 0.9;
    if mode > 2.5 {
        bound = max(band_bound, bounds.w);
        // Off->on can happen before an FFT job publishes its first reduction.
        // Do not draw unbounded horizontal chop from a not-yet-bounded field.
        // Keep the negative scale tag representable in the RGBA16F adapter.
        if fft_mix > 0.0 && bound <= 0.0 { return 0.00006103515625; }
    } else if mode > 1.5 {
        if bounds.w > 0.0 { bound = bounds.w; }
        budget = 1.15;
    }
    return min(1.0, budget / max(bound * fft_mix + analytic, 1e-5));
}

// The original spilling cue (shader_composer.rs) is shared with compact
// material. For mode 3 feed the *unlimited* strain: limiting geometry must
// not extinguish whitewater where the original wave would pinch. Use
// existing coverage, height gate and authored foam textures; not an FNL port.
fn jka_crest_jacobian(strain: vec3<f32>) -> f32 {
    return (1.0 + strain.x) * (1.0 + strain.y) - strain.z * strain.z;
}
fn jka_crest_spilling_cue(jacobian: f32, height_metres: f32, significant_height: f32) -> f32 {
    let high = smoothstep(0.0, 0.22, height_metres / max(significant_height, 0.1));
    return (1.0 - smoothstep(0.15, 0.7, jacobian)) * high;
}
