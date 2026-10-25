// Port of Bevy Solari's realtime/initial_path.wesl::sample_light_ris.
// Source: D:/Code/bevy-main/crates/bevy_solari (MIT, see vendor/bevy-solari-ris).
// Keep Solari's weighted reservoir selection and weight_sum / selected_target
// normalization. Our candidates enumerate a small group of existing JKA light
// contributions, already divided by emitter sample count, instead of drawing
// random candidates from a light tile. Consequently the inverse candidate PDF
// cancels the 1/N MIS weight: the reservoir estimates the SUM of that group.
// Evaluate the authoritative material first, then trace the selected sample.

struct RtRisReservoir {
    light: PointLight,
    contribution: vec3<f32>,
    specular: vec3<f32>,
    weight_sum: f32,
    selected_target_function: f32,
    count: u32,
};

fn rt_ray_rate() -> u32 {
    return clamp(rt_shadow_flags.flags.w, 1u, 4u);
}


fn rt_ris_update(reservoir: ptr<function, RtRisReservoir>, light: PointLight,
    contribution: vec3<f32>, specular: vec3<f32>, rng: ptr<function, u32>) {
    (*reservoir).count += 1u;
    if (all(contribution == vec3<f32>(0.0)) && all(specular == vec3<f32>(0.0))) { return; }
    if ((*reservoir).weight_sum == 0.0) {
        // The first nonzero candidate is selected with probability one. Defer
        // its target calculation until another candidate actually competes.
        // Full and a lone point light thus do no luminance or reservoir RNG
        // work in any budget. Negative weight_sum marks this exact singleton.
        (*reservoir).selected_target_function = 1.0;
        (*reservoir).weight_sum = -1.0;
        (*reservoir).light = light;
        (*reservoir).contribution = contribution;
        (*reservoir).specular = specular;
        return;
    }
    if ((*reservoir).weight_sum < 0.0) {
        let first_target = dot(abs((*reservoir).contribution) + abs((*reservoir).specular), vec3<f32>(0.2126, 0.7152, 0.0722));
        (*reservoir).weight_sum = first_target;
        (*reservoir).selected_target_function = first_target;
    }
    // Absolute channels also support JKA subtractive lights. Separate diffuse
    // and wet specular must not cancel in the sampling target.
    let target_function = dot(abs(contribution) + abs(specular), vec3<f32>(0.2126, 0.7152, 0.0722));
    if (target_function <= 0.0) { return; }
    let resampling_weight = target_function;
    (*reservoir).weight_sum += resampling_weight;
    if (rt_rand_f(rng) * (*reservoir).weight_sum < resampling_weight) {
        (*reservoir).selected_target_function = target_function;
        (*reservoir).light = light;
        (*reservoir).contribution = contribution;
        (*reservoir).specular = specular;
    }
}

fn rt_ris_unbiased_weight(reservoir: RtRisReservoir) -> f32 {
    if (reservoir.selected_target_function <= 0.0) { return 0.0; }
    if (reservoir.weight_sum < 0.0) { return 1.0; }
    return reservoir.weight_sum / reservoir.selected_target_function;
}

fn rt_ris_candidate_shadow_rgb(light: PointLight, position: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    // Static map lights keep their existing raster shadows and never enter RIS.
    if (ENABLE_RAY_TRACED_SHADOWS && light.shadow.w >= 0.5) { return vec3<f32>(1.0); }
    return local_light_shadow_rgb(light, position, normal);
}
