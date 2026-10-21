// godot-water-shader (vendor/godot-water-shader, MIT, Emil Jerkovic): the `gerstner` and `wave` functions of
// `shaders/g_wave.gdshader` and `shaders/foam_calc.gdshader`, in the project's own metre frame (FoamScene: the sea at
// y = 0). `jka_godot_noise(uv)` is defined by the includer: the red channel of the project's seamless cellular
// NoiseTexture2D (`textures/ocean_noise.tres`), repeating. Upstream's quirks are kept as written (see the port notes in
// `Development Docs/Gerstner ocean.md`): the horizontal motion is `steepness * cos(phase)` regardless of `amplitude`,
// and `TIME * speed + dot(direction, pos) * frequency` travels against `direction`.
//
// Upstream draws a 10 m plane at 4 cm cells, so it never undersamples its waves. Hybrid's clipmap rings and far pixels
// do: each wave here fades out where the mesh cell (vertex stage) or the pixel's footprint (fragment) can no longer
// resolve it, as Blue Water's own `displacement(grid, spacing)` does for its cascades, and the fragment takes the
// normal analytically per pixel (upstream's finite-difference vertex normal is exact only on its dense plane).

// The highest crest above the plane: the summed amplitudes plus the noise's half range (upstream metres).
const JKA_GODOT_CREST: f32 = 0.73;
const JKA_GODOT_TAU: f32 = 6.283185307179586;
// The noise's feature size (upstream metres): cellular cells about 100 of the 512 texels a 5 m repeat holds.
const JKA_GODOT_NOISE_FEATURE: f32 = 1.0;

// Upstream's five waves: xy direction (normalized), z amplitude, w frequency; and speed, steepness.
const JKA_GODOT_WAVES: array<vec4<f32>, 5> = array<vec4<f32>, 5>(
    vec4<f32>(0.0, 1.0, 0.4, 1.0),
    vec4<f32>(0.7071068, 0.7071068, 0.15, 1.3),
    vec4<f32>(0.4472136, 0.8944272, 0.12, 2.0),
    vec4<f32>(-0.3713907, 0.9284767, 0.01, 3.0),
    vec4<f32>(-0.2747211, 0.9615239, 0.025, 4.0),
);
const JKA_GODOT_MOTION: array<vec2<f32>, 5> = array<vec2<f32>, 5>(
    vec2<f32>(1.3, 0.2), vec2<f32>(1.6, 0.12), vec2<f32>(2.0, 0.17), vec2<f32>(3.0, 0.1), vec2<f32>(4.0, 0.08),
);

// How much of a wave of `frequency` (rad per upstream metre) a sampling `cell` (upstream metres) resolves: whole at four
// samples a wavelength, gone at two.
fn jka_godot_resolved(frequency: f32, cell: f32) -> f32 {
    return 1.0 - smoothstep(0.25, 0.5, cell * frequency / JKA_GODOT_TAU);
}

// Upstream's `wave` (`streaks`: foam_calc's `noise_intensity_x/z`, zero for g_wave), each wave and the height noise
// faded to what `cell` resolves (0 keeps everything, as upstream).
fn jka_godot_wave_filtered(pos: vec2<f32>, time: f32, streaks: vec2<f32>, cell: f32) -> vec3<f32> {
    var displacement = vec3<f32>(0.0);
    let noise_kept = 1.0 - smoothstep(0.25, 0.5, cell / JKA_GODOT_NOISE_FEATURE);
    for (var i = 0; i < 5; i++) {
        let wave = JKA_GODOT_WAVES[i];
        let motion = JKA_GODOT_MOTION[i];
        let direction = wave.xy;
        let kept = jka_godot_resolved(wave.w, cell);
        let noise_value = jka_godot_noise(pos * 0.2 + time * direction * 0.07);
        let phase = time * motion.x + dot(direction, pos) * wave.w;
        let xz = direction * cos(phase) * motion.y * kept;
        displacement += vec3<f32>(
            xz.x + noise_value * streaks.x - streaks.x / 2.0,
            sin(phase) * wave.z * kept + (noise_value * 0.05 - 0.05 / 2.0) * noise_kept,
            xz.y + noise_value * streaks.y - streaks.y / 2.0);
    }
    return displacement;
}

fn jka_godot_wave_streaked(pos: vec2<f32>, time: f32, streaks: vec2<f32>) -> vec3<f32> {
    return jka_godot_wave_filtered(pos, time, streaks, 0.0);
}

// `r_gerstnerWaveScale`: the upstream sea `scale` times larger, Froude-similar (lengths times `scale`, time divided by
// its square root), so a larger sea keeps upstream's shape and steepness and slows as real swell does. 1 is upstream.
fn jka_godot_time(seconds: f32, scale: f32) -> f32 {
    return seconds / sqrt(scale);
}

// Displacement (m) of the grid point `grid` (m) at `scale`, for a mesh `spacing` metres apart there.
fn jka_godot_displacement(grid: vec2<f32>, spacing: f32, scale: f32, seconds: f32) -> vec3<f32> {
    return jka_godot_wave_filtered(grid / scale, jka_godot_time(seconds, scale), vec2<f32>(0.0), spacing / scale) * scale;
}

struct JkaGodotSurface {
    // Unnormalized normal of the displaced surface; y is the horizontal map's Jacobian (upstream's `jacobian_d`).
    normal: vec3<f32>,
    // Slope variance of the waves the footprint averaged away (for the reflection lobe).
    variance: f32,
}

// The waves' surface at `pos` (upstream metres) for a pixel `footprint` (upstream metres) wide: the analytic tangent
// frame of upstream's displacement without its height noise, each wave faded as the footprint averages it and its slope
// energy kept as variance.
fn jka_godot_surface(pos: vec2<f32>, time: f32, footprint: f32) -> JkaGodotSurface {
    var jacobian = mat2x2<f32>(1.0, 0.0, 0.0, 1.0);
    var slope = vec2<f32>(0.0);
    var variance = 0.0;
    for (var i = 0; i < 5; i++) {
        let wave = JKA_GODOT_WAVES[i];
        let motion = JKA_GODOT_MOTION[i];
        let direction = wave.xy;
        let kept = jka_godot_resolved(wave.w, footprint);
        let phase = time * motion.x + dot(direction, pos) * wave.w;
        // d(direction * cos(phase) * steepness) / d(pos) and d(sin(phase) * amplitude) / d(pos).
        let stretch = -sin(phase) * motion.y * wave.w * kept;
        jacobian += mat2x2<f32>(direction * direction.x * stretch, direction * direction.y * stretch);
        slope += direction * (cos(phase) * wave.z * wave.w * kept);
        let height_slope = wave.z * wave.w;
        variance += (1.0 - kept * kept) * height_slope * height_slope * 0.5;
    }
    // Tangents along x and z: (J[0][0], slope.x, J[0][1]) and (J[1][0], slope.y, J[1][1]); normal = cross(tz, tx).
    let tx = vec3<f32>(jacobian[0][0], slope.x, jacobian[0][1]);
    let tz = vec3<f32>(jacobian[1][0], slope.y, jacobian[1][1]);
    return JkaGodotSurface(cross(tz, tx), variance);
}
