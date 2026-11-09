// Analytic day/night gain for the *vanilla* JKA sky backend. Unlike the Blue
// Water path this never samples an atmosphere atlas or changes the sky art.
// A deliberately conservative approximation: preserve authored daytime
// lightmaps, dim their sky-dependent share toward night, and retain more light
// inside sheltered spaces. Local promoted lights are composed separately.
fn vanilla_sky_gain(normal: vec3<f32>, toward_sun: vec3<f32>, sky_open: f32, strength: f32) -> vec3<f32> {
    let elevation = clamp(toward_sun.y, -1.0, 1.0);
    let t = clamp((elevation + 0.10) / 0.30, 0.0, 1.0);
    let day = t * t * (3.0 - 2.0 * t);
    let openness = clamp(sky_open, 0.0, 1.0);
    // Sheltered receivers retain baked interior illumination; exposed
    // receivers can approach genuine night darkness. Up-facing surfaces
    // receive somewhat more sky contribution than walls/undersides.
    let facing = 0.70 + 0.30 * max(normal.y, 0.0);
    let darkening = (1.0 - day) * (0.08 + 0.87 * openness * facing);
    return vec3<f32>(mix(1.0, clamp(1.0 - darkening, 0.05, 1.0), clamp(strength, 0.0, 1.0)));
}
