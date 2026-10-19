// Host implementation of the published native eye policy in upstream
// docs/lighting.md, "Realism" and "Into the sun". The original renderer's eye
// shader is not published. The reference itself is the unchanged sky_mean.wgsl.
@group(1) @binding(0) var<storage, read> sky_mean: vec4<f32>;
var<workgroup> native_samples: array<f32, 256>;
var<workgroup> native_sum: array<f32, 256>;

@compute @workgroup_size(16, 16, 1)
fn cs_native(@builtin(local_invocation_id) local: vec3<u32>,
             @builtin(local_invocation_index) index: u32) {
    let dimensions = textureDimensions(scene_texture);
    let pixel = min((local.xy * dimensions + dimensions / 2u) / 16u, dimensions - 1u);
    let sample_log = log2(max(luminance(textureLoad(scene_texture, vec2<i32>(pixel), 0).rgb), 1e-6));
    let own_sky = max(luminance(sky_mean.rgb), 1e-6);
    let clear_sky = select(own_sky, sky_mean.w, sky_mean.w > 0.0);
    let sky_log = log2(own_sky);
    let clear_log = log2(max(clear_sky, 1e-6));
    native_samples[index] = sample_log;
    // Bright glare cannot dominate the mean; its brightest tenth is metered
    // separately and can close the eye instead of being thrown away.
    native_sum[index] = min(sample_log, sky_log + 0.5);
    workgroupBarrier();
    for (var stride = 128u; stride > 0u; stride /= 2u) {
        if index < stride { native_sum[index] += native_sum[index + stride]; }
        workgroupBarrier();
    }
    // Bitonic sort in shared memory, ascending, to meter the brightest tenth.
    for (var width = 2u; width <= 256u; width *= 2u) {
        for (var stride = width / 2u; stride > 0u; stride /= 2u) {
            let other = index ^ stride;
            let value = native_samples[index];
            let pair = native_samples[other];
            let ascending = (index & width) == 0u;
            let take_min = (index < other) == ascending;
            let sorted = select(max(value, pair), min(value, pair), take_min);
            workgroupBarrier();
            native_samples[index] = sorted;
            workgroupBarrier();
        }
    }
    if index == 0u {
        let mean_log = native_sum[0] / 256.0;
        var bright_log = 0.0;
        for (var i = 230u; i < 256u; i++) { bright_log += native_samples[i]; }
        bright_log /= 26.0;
        let day = smoothstep(-5.5, -3.0, clear_log);
        let bright_over_clear = bright_log - clear_log;
        let opening = min(clamp(-0.37 - (mean_log - clear_log), 0.0, 2.0),
            max(1.5 - bright_over_clear, 0.0));
        let closing = clamp(max(mean_log - sky_log, bright_over_clear - 1.5), 0.0, 1.0);
        let available = settings.params.x > 0.5 && luminance(sky_mean.rgb) > 0.0;
        // Underwater eyes ease to fixed exposure, matching upstream Session.
        let desired_ev = select(0.0, (opening - closing) * day,
            available && settings.adaptation.w < 0.5);
        let speed = select(settings.adaptation.y, settings.adaptation.z, desired_ev < state.exposure_ev);
        let blend = 1.0 - exp(-max(speed, 0.0) * max(settings.params.y, 0.0));
        state.exposure_ev = mix(state.exposure_ev, desired_ev, clamp(blend, 0.0, 1.0));
        state.average_luminance = exp2(mean_log);
        state.target_ev = desired_ev;
        // Display-only night readability; never feeds back into the meter.
        state.reserved = select(0.0, 1.0 - day, available);
    }
}
