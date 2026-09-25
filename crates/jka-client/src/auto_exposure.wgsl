struct AutoExposureSettings {
    params: vec4<f32>, // x enabled, y dt, z min EV, w max EV
    adaptation: vec4<f32>, // x middle gray, y brighten speed, z darken speed, w reserved
};

struct AutoExposureState {
    exposure_ev: f32,
    average_luminance: f32,
    target_ev: f32,
    reserved: f32,
};

@group(0) @binding(0) var scene_texture: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> state: AutoExposureState;
@group(0) @binding(2) var<uniform> settings: AutoExposureSettings;

var<workgroup> log_luminance: array<f32, 256>;

fn luminance(color: vec3<f32>) -> f32 {
    return dot(max(color, vec3<f32>(0.0)), vec3<f32>(0.2126, 0.7152, 0.0722));
}

@compute @workgroup_size(16, 16, 1)
fn cs_main(
    @builtin(local_invocation_id) local_id: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    let dimensions = textureDimensions(scene_texture);
    let sample_x = min(
        (local_id.x * dimensions.x + dimensions.x / 2u) / 16u,
        dimensions.x - 1u,
    );
    let sample_y = min(
        (local_id.y * dimensions.y + dimensions.y / 2u) / 16u,
        dimensions.y - 1u,
    );
    let color = textureLoad(scene_texture, vec2<i32>(i32(sample_x), i32(sample_y)), 0).rgb;
    log_luminance[local_index] = log2(max(luminance(color), 1.0e-5));
    workgroupBarrier();

    var stride = 128u;
    loop {
        if (local_index < stride) {
            log_luminance[local_index] += log_luminance[local_index + stride];
        }
        workgroupBarrier();
        if (stride == 1u) {
            break;
        }
        stride = stride / 2u;
    }

    if (local_index == 0u) {
        if (settings.params.x < 0.5) {
            state.exposure_ev = 0.0;
            state.average_luminance = 1.0;
            state.target_ev = 0.0;
            state.reserved = 0.0;
            return;
        }

        let average = exp2(log_luminance[0] / 256.0);
        let middle_gray = max(settings.adaptation.x, 1.0e-4);
        let target_exposure_ev = clamp(
            log2(middle_gray / max(average, 1.0e-5)),
            settings.params.z,
            settings.params.w,
        );

        // Negative delta means the scene suddenly became brighter and the
        // exposure must come down. Keep that adaptation a little faster than
        // opening up into darkness to avoid blown-out flashes while retaining
        // the slower eye/camera adaptation users expect after entering a dark room.
        let speed = select(settings.adaptation.y, settings.adaptation.z, target_exposure_ev < state.exposure_ev);
        let blend = 1.0 - exp(-max(speed, 0.0) * max(settings.params.y, 0.0));
        let adapted = mix(state.exposure_ev, target_exposure_ev, clamp(blend, 0.0, 1.0));

        state.exposure_ev = adapted;
        state.average_luminance = average;
        state.target_ev = target_exposure_ev;
        state.reserved = 0.0;
    }
}
