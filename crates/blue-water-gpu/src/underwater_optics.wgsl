// Shared JKA spectral underwater optics for Blue Water lake and native river.
// Matches the legacy Hybrid ratios (38 / 7.5, 38 / 22, 1) and the five-percent
// Beer-Lambert normalization. Callers supply their own physical path length,
// submerged test, waterline treatment, and world-unit conversion.
fn jka_underwater_spectral_transmission(path: f32, range: f32) -> vec3<f32> {
    let ratios = vec3<f32>(38.0 / 7.5, 38.0 / 22.0, 1.0);
    return exp2(-4.321928 * ratios * max(path, 0.0) / max(range, 0.001));
}

fn jka_underwater_spectral_medium(
    scene: vec3<f32>, tint: vec3<f32>, path: f32, visibility: f32,
    eye_depth: f32, darkening: f32,
) -> vec3<f32> {
    let transmission = jka_underwater_spectral_transmission(path, visibility);
    let downwelling = jka_underwater_spectral_transmission(eye_depth, darkening);
    return scene * transmission + tint * downwelling * (1.0 - transmission);
}
