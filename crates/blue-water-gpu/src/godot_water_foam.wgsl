// godot-water-shader's persistent foam (`r_oceanType 5`): one cascade's `shaders/foam_calc.gdshader` (Jacobian and
// intersection foam over a capture centred on the camera) and `shaders/history.gdshader` (the previous frame's foam,
// shifted with the capture, eroded and decayed; the larger kept) in one full-screen pass. Upstream's foam viewport is
// 8-bit, so the fresh foam is clamped to 0..1 before the history takes the maximum.
//
// Upstream's two orthographic depth cameras (the highest and lowest surfaces within 10 m of the water) are a static
// contact field here (`ocean::gerstner_contact`): per column the highest and lowest static solid within the same band.

struct FoamPass {
    // xy capture centre (grid m), z coverage (m), w 1 / resolution.
    capture: vec4<f32>,
    // xy history offset (uv), z frame delta (s), w the wave clock (s).
    history: vec4<f32>,
    // x wave scale, y decay rate, z erosion multiplier, w 1 while the history is this capture's.
    params: vec4<f32>,
    // The contact field: xy its first texel's corner (grid m), zw 1 / its extent (m).
    contact_frame: vec4<f32>,
    // x 1 with a contact field, y the water plane above the field's level (m), zw foam_calc's noise_intensity_x/z.
    contact: vec4<f32>,
}

@group(0) @binding(0) var<uniform> foam_pass: FoamPass;
@group(0) @binding(1) var history_foam: texture_2d<f32>;
@group(0) @binding(2) var patterns: texture_2d<f32>;
@group(0) @binding(3) var repeat_sampler: sampler;
@group(0) @binding(4) var clamp_sampler: sampler;
// rg: the highest and lowest solid within the band, relative to the field's level (m).
@group(0) @binding(5) var contact_field: texture_2d<f32>;

fn jka_godot_noise(uv: vec2<f32>) -> f32 {
    return textureSampleLevel(patterns, repeat_sampler, uv, 0.0).g;
}

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let scale = foam_pass.params.x;
    let time = jka_godot_time(foam_pass.history.w, scale);
    let uv = position.xy * foam_pass.capture.w;
    // foam_calc, in upstream metres.
    let capture_center = foam_pass.capture.xy / scale;
    let capture_coverage = foam_pass.capture.z / scale;
    let sampling_distance = foam_pass.capture.w * capture_coverage;
    let world_xz = capture_center + (uv - 0.5) * capture_coverage;
    let streaks = foam_pass.contact.zw;
    let disp = jka_godot_wave_streaked(world_xz, time, streaks);
    // Upstream evaluates the waves twice more, one texel (`sampling_distance`) along x and z. This pass covers its
    // capture one texel per pixel, so the neighbouring pixels' own evaluations are those samples: fine derivatives
    // give the same one-texel differences (forward or backward by the pixel's quad side) for one evaluation, not three.
    let dpos_x = vec2<f32>(sampling_distance, 0.0) + dpdxFine(disp.xz);
    let dpos_z = vec2<f32>(0.0, sampling_distance) + dpdyFine(disp.xz);
    let sa = dpos_x.x * dpos_z.y - dpos_x.y * dpos_z.x;
    let jacobian_d = sa / (sampling_distance * sampling_distance);
    let compression = 1.0 - smoothstep(0.0, 0.7, jacobian_d);
    var noise_v = jka_godot_noise(world_xz / 15.0);
    noise_v = smoothstep(0.2, 0.7, noise_v);
    noise_v *= 8.0;
    let foam = compression * noise_v;
    // Intersection foam where the displaced water lies inside a solid's vertical extent (m).
    var i_foam = 0.0;
    if foam_pass.contact.x > 0.5 {
        let at = (world_xz + disp.xz) * scale;
        let field_uv = (at - foam_pass.contact_frame.xy) * foam_pass.contact_frame.zw;
        let span = textureSampleLevel(contact_field, clamp_sampler, field_uv, 0.0).rg - foam_pass.contact.y;
        let y_water = disp.y * scale;
        let over = smoothstep(0.0, 0.15 * scale, span.x - y_water);
        let under = smoothstep(0.0, 0.15 * scale, y_water - span.y);
        i_foam = over * under;
    }
    let current = clamp(vec2<f32>(foam, i_foam), vec2<f32>(0.0), vec2<f32>(1.0));
    // history
    let history_uv = uv + foam_pass.history.xy;
    var previous = vec2<f32>(0.0);
    if foam_pass.params.w > 0.5 && all(history_uv >= vec2<f32>(0.0)) && all(history_uv <= vec2<f32>(1.0)) {
        previous = textureSampleLevel(history_foam, clamp_sampler, history_uv, 0.0).rg;
    }
    let delta = foam_pass.history.z;
    let decay_rate = foam_pass.params.y;
    let erosion_multiplier = foam_pass.params.z;
    let noise_erosion = jka_godot_noise(world_xz / 15.0 * 24.0);
    previous.x -= noise_erosion * delta * erosion_multiplier;
    previous.y -= noise_erosion * delta * erosion_multiplier * 0.1;
    previous.x *= exp(-decay_rate * delta);
    previous.y *= exp(-decay_rate * delta * 0.3);
    previous = max(previous, vec2<f32>(0.0));
    return vec4<f32>(max(current, previous), 0.0, 1.0);
}
