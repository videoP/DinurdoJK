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

// r_skyWorldGainMode calibration of the Blue Water world gain `full`.
// params: x mode (0 A original, 1 B daylight fades to `neutral`, 2 C ratio to the
// map's authored time), y toward-sun Y of the authored (q3map_sun or detected)
// sun, z 1 when that reference exists. `now_y` is the live key light's toward-
// light Y, -1 when the moon is the key. Keep identical to blue_water_scene.wgsl
// and the model sky compute.
fn sky_gain_calibrated(full: vec3<f32>, neutral: vec3<f32>, now_y: f32, params: vec4<f32>) -> vec3<f32> {
    let mode = u32(params.x + 0.5);
    if mode == 0u { return full; }
    let day_t = clamp((now_y + 0.10) / 0.30, 0.0, 1.0);
    let day = day_t * day_t * (3.0 - 2.0 * day_t);
    if mode == 1u { return mix(full, neutral, day); }
    // C: no authored sun known -> neutral at all times (no time-of-day scaling).
    if params.z < 0.5 { return neutral; }
    let ref_t = clamp((params.y + 0.10) / 0.30, 0.0, 1.0);
    let ref_day = ref_t * ref_t * (3.0 - 2.0 * ref_t);
    let ratio = clamp(mix(0.10, 1.0, day) / mix(0.10, 1.0, ref_day), 0.15, 2.0);
    // Blue Water supplies only the twilight/moon hue (luminance-normalised).
    let tint = full / max(dot(full, vec3<f32>(0.2126, 0.7152, 0.0722)), 1e-4);
    return neutral * ratio * mix(tint, vec3<f32>(1.0), day);
}
