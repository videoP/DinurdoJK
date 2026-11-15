// Vanilla changes only the visible sky, not the time-of-day world exposure.
// Use the SAME calibrated night/day response as Blue Water, without Blue
// Water's atmospheric tint or moonlight. Previously the vanilla-only
// sky-open heuristic preserved ~60% of baked daylight on half-open surfaces
// at midnight, so switching from Blue Water brightened the map dramatically.
// The optional authored-room protection still uses the sky openness estimate.
fn vanilla_sky_gain(_normal: vec3<f32>, toward_sun: vec3<f32>, sky_open: f32,
                    strength: f32, isolate: bool, calibration: vec4<f32>) -> vec3<f32> {
    let calibrated = sky_gain_calibrated(vec3<f32>(0.03), vec3<f32>(1.0),
        clamp(toward_sun.y, -1.0, 1.0), calibration);
    let authored_share = select(1.0,
        sky_transport_daylight_share(vec2<f32>(clamp(sky_open, 0.0, 1.0), 0.0)), isolate);
    return mix(vec3<f32>(1.0), calibrated, clamp(strength * authored_share, 0.0, 1.0));
}

// Room protection uses a fixed sky-transport response, not the live moon's
// visibility. Small but sustained indirect sky access can account for nearly
// all the authored illumination in an outdoor shadow. Ignore the weakest
// transport/noise, then smoothly reach full daylight dependence at 8% response
// to the unit white sky. This is an estimated share, not source reconstruction.
fn sky_transport_daylight_share(response: vec2<f32>) -> f32 {
    return smoothstep(0.01, 0.08, max(response.x + response.y, 0.0));
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
