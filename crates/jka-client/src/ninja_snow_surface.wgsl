// Humpy1 retains its legacy shell. Composite uses the authored BSP ground
// receiver and separate velocity-density export (density is inverted in A).
fn ninja_snow_ue(v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(v.x, -v.z, v.y);
}

fn ninja_snow_jka(v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(v.x, v.z, -v.y);
}

fn ninja_snow_vertex_offset(position: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    let world_cm = ninja_snow_ue(position) * 2.54;
    let n = ninja_snow_ue(normal);
    if (ninja_snow_composite_mode()) {
        // MI_Snow_Composite's tile-only WPO is world-space and independent of
        // the moving velocity-density window. Do not use Humpy1 displacement.
        let tile = ninja_snow_composite_tile_uvs(world_cm.xy);
        let offset_cm = ninja_snow_composite_wpo_cm(
            snowflow_field_1, snowflow_field_sampler, tile.xy, n);
        return ninja_snow_jka(offset_cm) / 2.54;
    }
    let tile = ninja_snow_humpy1_tile_uvs(world_cm.xy);
    let offset_cm = ninja_snow_humpy1_wpo_cm(snowflow_field_1, snowflow_field_sampler, tile.xy, n);
    return ninja_snow_jka(offset_cm) / 2.54;
}

@diagnostic(off, derivative_uniformity)
fn ninja_snow_material(position: vec3<f32>, normal: vec3<f32>, eye: vec3<f32>, direction: vec3<f32>) -> NinjaSnowShading {
    let world_cm = ninja_snow_ue(position) * 2.54;
    let n = normalize(ninja_snow_ue(normal));
    // Keep the world X basis where it is well-defined; use Y only at the
    // degenerate axis. This is the host's geometric tangent construction.
    let axis = select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), abs(n.x) > 0.9999);
    let tangent = normalize(axis - n * dot(axis, n));
    let frame = mat3x3<f32>(tangent, cross(n, tangent), n);
    let world_to_tangent = transpose(frame);
    let view = normalize(ninja_snow_ue(eye - position));
    let sim = (position.xz - surface_deformation.snowflow_params.xy) / surface_deformation.snowflow_params.z;
    let sim_uv = vec2<f32>(0.5 + sim.x, 0.5 - sim.y);
    let composite = ninja_snow_composite_mode();
    let tile = select(ninja_snow_humpy1_tile_uvs(world_cm.xy), ninja_snow_composite_tile_uvs(world_cm.xy), composite);
    var profile = NINJA_SNOW_HUMPY1;
    if (composite) { profile = NINJA_SNOW_COMPOSITE; }
    let input = NinjaSnowHumpy1Inputs(
        sim_uv, dpdx(sim_uv), dpdy(sim_uv),
        tile.xy, dpdx(tile.xy), dpdy(tile.xy),
        tile.zw, dpdx(tile.zw), dpdy(tile.zw),
        world_cm, dpdx(world_cm), dpdy(world_cm),
        view, ninja_snow_ue(direction), n, world_to_tangent * view,
        world_to_tangent, 1.0, select(0.0, 1.0, composite), 1.0,
        composite, surface_deformation.snowflow_header.y != 0u
    );
    var snow = ninja_snow_surface_shade(
        snowflow_field_0, stock_footstep_sampler,
        snowflow_field_1, snowflow_field_sampler,
        stock_footstep_l, snowflow_field_sampler,
        stock_footstep_r, snowflow_field_sampler, input, profile
    );
    if (composite) {
        snow = ninja_snow_composite_material(snow, n);
    }
    let perturbed = normalize(ninja_snow_jka(frame * snow.normal_raw));
    if (composite) {
        // Preserve the JKA map's authored diffuse. The UE material returns
        // absolute color, so use the same reference-tint normalization already
        // used by the Humpy1 JKA receiver, instead of replacing the BSP image.
        return NinjaSnowShading(snow.base_color / profile.color_high, perturbed,
            clamp(snow.roughness_raw, 0.02, 1.0));
    }
    // The legacy Humpy1 overlay remains unchanged.
    let weight = ninja_snow_weight(position);
    let color_scale = mix(vec3<f32>(1.0), snow.base_color / profile.color_high, weight);
    return NinjaSnowShading(color_scale, normalize(mix(normal, perturbed, weight)), clamp(snow.roughness_raw, 0.02, 1.0));
}
