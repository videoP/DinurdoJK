// Ported from Bevy Solari directional-light sampling:
// - bevy_solari/src/scene/sampling.wesl::resolve_light_sample
// - bevy_pbr/src/render/utils.wesl::{rand_f, rand_vec2f}
// - bevy_render/src/maths.wesl::orthonormalize
//
// The sampler itself is generic for finite angular directional emitters. The
// current RT Shadows caller supplies the JKA authored sun direction and an
// Earth-like angular extent through shadow_settings.bevy_params.x.
fn rt_rand_f(state: ptr<function, u32>) -> f32 {
    *state = *state * 747796405u + 2891336453u;
    let word = ((*state >> ((*state >> 28u) + 4u)) ^ *state) * 277803737u;
    return f32((word >> 22u) ^ word) * bitcast<f32>(0x2f800004u);
}

fn rt_rand_vec2f(state: ptr<function, u32>) -> vec2<f32> {
    return vec2<f32>(rt_rand_f(state), rt_rand_f(state));
}

fn rt_sample_count() -> u32 {
    return u32(clamp(lighting_settings.map_ambient.w, 1.0, 4.0));
}

fn rt_local_sample_count(light: PointLight) -> u32 {
    // Point lights and q3map metadata need one visibility query, regardless of quality.
    return select(1u, rt_sample_count(), light.emitter.w < -0.5 && light.shadow.w >= 0.5);
}

fn rt_sample_local_emitter(light: PointLight, input: VertexOut, light_index: u32, sample_index: u32, sample_count: u32) -> PointLight {
    // Negative emitter.w also encodes authored q3map angle attenuation. Only
    // transient lights may use it as a finite-emitter marker.
    if (light.emitter.w >= -0.5 || light.shadow.w < 0.5) {
        return light;
    }
    // Uniform line-source integration: use a normalized distribution along the
    // authored visible blade, preserving the existing total light strength.
    // The entire lighting evaluation (distance, BRDF and visibility) consumes
    // this same sample. Sampling only visibility would leave a point-light BRDF.
    let pixel = vec2<u32>(max(input.clip_position.xy, vec2<f32>(0.0)));
    let width = max(lighting_settings.values.z, 1u);
    var rng = (pixel.x + pixel.y * width) ^ (light_index * 2246822519u) ^ (camera.render_flags.w * 3266489917u);
    rng ^= sample_index * 668265263u;
    // One jittered sample per equal interval (PBRT stratified sampling).
    let sample_t = (f32(sample_index) + rt_rand_f(&rng)) / f32(sample_count);
    var sampled = light;
    sampled.position_radius = vec4<f32>(
        light.position_radius.xyz + light.emitter.xyz * (2.0 * sample_t - 1.0),
        light.position_radius.w
    );
    sampled.emitter = vec4<f32>(0.0);
    sampled.color_intensity = vec4<f32>(light.color_intensity.rgb / f32(sample_count), light.color_intensity.a);
    return sampled;
}

fn rt_copysign(a: f32, b: f32) -> f32 {
    return bitcast<f32>((bitcast<u32>(a) & 0x7fffffffu) | (bitcast<u32>(b) & 0x80000000u));
}

fn rt_orthonormalize(z_basis: vec3<f32>) -> mat3x3<f32> {
    let sign = rt_copysign(1.0, z_basis.z);
    let a = -1.0 / (sign + z_basis.z);
    let b = z_basis.x * z_basis.y * a;
    let x_basis = vec3<f32>(1.0 + sign * z_basis.x * z_basis.x * a, sign * b, -sign * z_basis.x);
    let y_basis = vec3<f32>(b, sign + z_basis.y * z_basis.y * a, -z_basis.y);
    return mat3x3<f32>(x_basis, y_basis, z_basis);
}

fn rt_sample_directional_emitter(
    direction_to_light: vec3<f32>,
    cos_theta_max: f32,
    rng: ptr<function, u32>,
    sample_index: u32,
    sample_count: u32,
) -> vec3<f32> {
    // Bevy Solari: sample uniformly in solid angle inside the directional
    // light's cone, then rotate that local +Z cone onto the light direction.
    var random = rt_rand_vec2f(rng);
    let columns = select(1u, 2u, sample_count > 1u);
    let rows = max(sample_count / columns, 1u);
    random = (vec2<f32>(f32(sample_index % columns), f32(sample_index / columns)) + random)
        / vec2<f32>(f32(columns), f32(rows));
    let cos_theta = (1.0 - random.x) + random.x * cos_theta_max;
    let sin_theta = sqrt(max(1.0 - cos_theta * cos_theta, 0.0));
    let phi = random.y * 6.28318530717958647692;
    let local_direction = vec3<f32>(cos(phi) * sin_theta, sin(phi) * sin_theta, cos_theta);
    return rt_orthonormalize(direction_to_light) * local_direction;
}

fn rt_alpha_repeat_coord(value: i32, size: i32) -> i32 {
    let remainder = value % size;
    return select(remainder + size, remainder, remainder >= 0);
}

fn rt_alpha_texel(texture: RtAlphaTexture, x: i32, y: i32) -> f32 {
    let width = max(i32(texture.values.y), 1);
    let height = max(i32(texture.values.z), 1);
    var sx = x;
    var sy = y;
    if (texture.values.w != 0u) {
        sx = clamp(sx, 0, width - 1);
        sy = clamp(sy, 0, height - 1);
    } else {
        sx = rt_alpha_repeat_coord(sx, width);
        sy = rt_alpha_repeat_coord(sy, height);
    }
    let linear_index = u32(sy * width + sx);
    let packed = rt_alpha_texels[texture.values.x + linear_index / 4u];
    let shift = (linear_index & 3u) * 8u;
    return f32((packed >> shift) & 0xffu) * (1.0 / 255.0);
}

fn rt_sample_alpha(texture_index: u32, uv: vec2<f32>) -> f32 {
    let texture = rt_alpha_textures[texture_index];
    let size = vec2<f32>(f32(max(texture.values.y, 1u)), f32(max(texture.values.z, 1u)));
    let sample_position = uv * size - vec2<f32>(0.5);
    let base = vec2<i32>(floor(sample_position));
    let fraction = fract(sample_position);
    let a00 = rt_alpha_texel(texture, base.x, base.y);
    let a10 = rt_alpha_texel(texture, base.x + 1, base.y);
    let a01 = rt_alpha_texel(texture, base.x, base.y + 1);
    let a11 = rt_alpha_texel(texture, base.x + 1, base.y + 1);
    return mix(mix(a00, a10, fraction.x), mix(a01, a11, fraction.x), fraction.y);
}

fn rt_alpha_generated_uv(
    material: RtAlphaMaterial,
    base_uv: vec2<f32>,
    lightmap_uv: vec2<f32>,
    world_position: vec3<f32>,
    world_normal: vec3<f32>,
) -> vec2<f32> {
    var uv = base_uv;
    if (material.header.x == 1u) {
        uv = lightmap_uv;
    }
    if (material.header.x == 2u) {
        uv = vec2<f32>(
            dot(world_position, material.vector_s.xyz),
            dot(world_position, material.vector_t.xyz)
        );
    }
    if (material.header.x == 3u) {
        let n = normalize(world_normal);
        let view = normalize(camera.camera_pos_time.xyz - world_position);
        let reflected = reflect(-view, n);
        uv = vec2<f32>(0.5 - reflected.z * 0.5, 0.5 - reflected.y * 0.5);
    }

    let time = camera.camera_pos_time.w;
    for (var i = 0u; i < 4u; i = i + 1u) {
        if (i >= material.header.y) {
            break;
        }
        let a = material.mods[i * 2u];
        let b = material.mods[i * 2u + 1u];
        let kind = u32(a.x + 0.5);
        if (kind == 1u) {
            uv = uv + a.yz * time;
        }
        if (kind == 2u) {
            uv = uv * a.yz;
        }
        if (kind == 3u) {
            let radians = a.y * time * 0.017453292519943295;
            let c = cos(radians);
            let sn = sin(radians);
            let p = uv - vec2<f32>(0.5);
            uv = vec2<f32>(p.x * c - p.y * sn, p.x * sn + p.y * c) + vec2<f32>(0.5);
        }
        if (kind == 4u) {
            uv = vec2<f32>(
                uv.x * a.y + uv.y * a.z + a.w,
                uv.x * b.x + uv.y * b.y + b.z
            );
        }
        if (kind == 5u) {
            // Matches OpenJK's RB_CalcTurbulentTexCoords: driven by vertex
            // world position (scaled), not by the surface's own UV, which
            // aliases badly on any surface that tiles many times.
            let now = a.w + time * b.x;
            let wave = vec2<f32>(
                sin((now + (world_position.x + world_position.y) * 0.0009765625) * 6.28318530718) * a.z,
                sin((now - world_position.z * 0.0009765625) * 6.28318530718) * a.z
            );
            uv = uv + wave;
        }
    }
    return uv;
}

fn rt_alpha_candidate_blocks(candidate: RayIntersection) -> bool {
    let geometry_index = candidate.instance_custom_data + candidate.geometry_index;
    let geometry = rt_alpha_geometries[geometry_index];
    let material = rt_alpha_materials[geometry.values.z];
    let first_index = geometry.values.y + candidate.primitive_index * 3u;
    let first_vertex = geometry.values.x;
    let i0 = first_vertex + rt_alpha_indices[first_index];
    let i1 = first_vertex + rt_alpha_indices[first_index + 1u];
    let i2 = first_vertex + rt_alpha_indices[first_index + 2u];
    let v0 = rt_alpha_vertices[i0];
    let v1 = rt_alpha_vertices[i1];
    let v2 = rt_alpha_vertices[i2];
    let bary = vec3<f32>(
        1.0 - candidate.barycentrics.x - candidate.barycentrics.y,
        candidate.barycentrics.x,
        candidate.barycentrics.y
    );
    let object_position = v0.position_u.xyz * bary.x
        + v1.position_u.xyz * bary.y
        + v2.position_u.xyz * bary.z;
    let object_normal = v0.normal_v.xyz * bary.x
        + v1.normal_v.xyz * bary.y
        + v2.normal_v.xyz * bary.z;
    let base_uv = vec2<f32>(
        v0.position_u.w * bary.x + v1.position_u.w * bary.y + v2.position_u.w * bary.z,
        v0.normal_v.w * bary.x + v1.normal_v.w * bary.y + v2.normal_v.w * bary.z
    );
    let lightmap_uv = v0.lightmap_alpha.xy * bary.x
        + v1.lightmap_alpha.xy * bary.y
        + v2.lightmap_alpha.xy * bary.z;
    let vertex_alpha = v0.lightmap_alpha.z * bary.x
        + v1.lightmap_alpha.z * bary.y
        + v2.lightmap_alpha.z * bary.z;
    let world_position = candidate.object_to_world * vec4<f32>(object_position, 1.0);
    let world_normal = candidate.object_to_world * vec4<f32>(object_normal, 0.0);
    let uv = rt_alpha_generated_uv(material, base_uv, lightmap_uv, world_position, world_normal);
    var alpha_multiplier = material.color.a;
    // Match the existing BSP shader's alphaGen semantics exactly. RGB generation
    // uses different bits; RT candidate rejection must only follow alphaGen.
    if ((material.header.z & 1048576u) != 0u) {
        alpha_multiplier *= vertex_alpha;
    } else if ((material.header.z & 2097152u) != 0u) {
        alpha_multiplier *= (1.0 - vertex_alpha);
    }
    let alpha = rt_sample_alpha(material.texture.x, uv) * alpha_multiplier;
    return alpha >= material.params.x;
}

fn rt_trace_shadow_visibility(origin: vec3<f32>, direction_to_light: vec3<f32>, max_distance: f32) -> f32 {
    // RayDesc requires t_max >= t_min. Very close emitters have no traceable
    // segment after the receiver and emitter endpoint biases are applied.
    if (max_distance <= 0.25) {
        return 1.0;
    }
    var query: ray_query;
    // TERMINATE_ON_FIRST_HIT | SKIP_AABBS. Opaque BLAS triangles auto-commit;
    // non-opaque mask triangles interrupt traversal as candidates so their
    // authored alpha test can decide whether traversal continues.
    rayQueryInitialize(
        &query,
        rt_shadow_scene,
        RayDesc(0x204u, 0xffu, 0.25, max_distance, origin, direction_to_light)
    );
    loop {
        let has_candidate = rayQueryProceed(&query);
        if (!has_candidate) {
            break;
        }
        let candidate = rayQueryGetCandidateIntersection(&query);
        if (candidate.kind == 1u && rt_alpha_candidate_blocks(candidate)) {
            rayQueryConfirmIntersection(&query);
            rayQueryTerminate(&query);
        }
    }
    let hit = rayQueryGetCommittedIntersection(&query);
    return select(1.0, 0.0, hit.kind != 0u);
}

fn ray_traced_local_shadow_visibility(light: PointLight, world_position: vec3<f32>, normal: vec3<f32>) -> f32 {
    // Local shadows remain independently switchable. The cubemap slot/count
    // are deliberately irrelevant here, including for transient FX lights.
    if (!ENABLE_LOCAL_SHADOWS || camera.render_flags.x != 0u) {
        return 1.0;
    }
    let delta = light.position_radius.xyz - world_position;
    let distance_squared = dot(delta, delta);
    let radius = max(light.position_radius.w, 0.0);
    if (distance_squared <= 1e-8 || distance_squared >= radius * radius) {
        return 1.0;
    }
    let direction = delta * inverseSqrt(distance_squared);
    // Match the contribution rules shared by the existing clustered callers.
    // Ordinary lights contribute neither diffuse nor specular on the back side.
    // Authored q3map lighting has different angle rules, so preserve that path.
    let ndotl = max(dot(normal, direction), 0.0);
    if (ENABLE_MAP_LIGHT_SIMULATION && light.shadow.y > 0.5) {
        let scalar = q3map_light_scalar(light, sqrt(distance_squared), q3map_surface_angle(light, ndotl));
        if (!q3map_fast_contribution_visible(scalar)) {
            return 1.0;
        }
    } else if (ndotl <= 0.0 || light.color_intensity.a == 0.0) {
        return 1.0;
    }
    if (emitter_visibility(light, direction) <= 0.0) {
        return 1.0;
    }
    return rt_trace_local_visibility(light, world_position, normal);
}

fn rt_trace_local_visibility(light: PointLight, world_position: vec3<f32>, normal: vec3<f32>) -> f32 {
    let delta = light.position_radius.xyz - world_position;
    let distance_squared = dot(delta, delta);
    let radius = max(light.position_radius.w, 0.0);
    if (distance_squared <= 1e-8 || distance_squared >= radius * radius) { return 1.0; }
    let direction = delta * inverseSqrt(distance_squared);
    let surface_normal = normal * inverseSqrt(max(dot(normal, normal), 1e-12));
    let oriented_normal = select(-surface_normal, surface_normal, dot(surface_normal, direction) >= 0.0);
    // Limit the offset near an emitter, and recompute the direction from the
    // biased origin so the finite segment still ends exactly at that emitter.
    let origin = world_position + oriented_normal * min(0.5, sqrt(distance_squared) * 0.25);
    let to_emitter = light.position_radius.xyz - origin;
    let distance = length(to_emitter);
    if (distance <= 0.5) {
        return 1.0;
    }
    return rt_trace_shadow_visibility(origin, to_emitter / distance, distance - 0.25);
}

fn ray_traced_shadow_visibility(input: VertexOut, normal: vec3<f32>) -> f32 {
    if (!ENABLE_RAY_TRACED_SUN || camera.render_flags.x != 0u || shadow_settings.light_direction_enabled.w < 0.5) { return 1.0; }
    let cached = rt_cached_sun_visibility(input);
    if (cached >= 0.0) { return cached; }
    return rt_uncached_sun_visibility(input, normal);
}

fn rt_uncached_sun_visibility(input: VertexOut, normal: vec3<f32>) -> f32 {
    if (!ENABLE_RAY_TRACED_SUN || camera.render_flags.x != 0u || shadow_settings.light_direction_enabled.w < 0.5) {
        return 1.0;
    }

    // ShadowSettings stores the direction the authored light travels. Receiver
    // rays travel in the opposite direction, toward the finite emitter.
    let toward_light = -shadow_settings.light_direction_enabled.xyz;
    let light_len_sq = dot(toward_light, toward_light);
    if (light_len_sq < 1e-8) {
        return 1.0;
    }
    let direction_to_light = toward_light * inverseSqrt(light_len_sq);
    let normal_len_sq = max(dot(normal, normal), 1e-12);
    let surface_normal = normal * inverseSqrt(normal_len_sq);
    let oriented_normal = select(-surface_normal, surface_normal, dot(surface_normal, direction_to_light) >= 0.0);
    let origin = input.world_position + oriented_normal * 0.5;

    // Seed layout follows Bevy's per-pixel/per-frame stochastic approach. With
    // TAA enabled render_flags.w advances every frame so the existing temporal
    // resolve integrates a new sun-disc sample. Without TAA the frame term is
    // held at zero: each pixel keeps one deterministic sample, avoiding temporal
    // sparkle without multiplying hardware ray traversal cost.
    let px = u32(max(input.clip_position.x, 0.0));
    let py = u32(max(input.clip_position.y, 0.0));
    let viewport_width = max(lighting_settings.values.z, 1u);
    let pixel_index = px + py * viewport_width;
    var rng = pixel_index + camera.render_flags.w;
    let cos_theta_max = clamp(shadow_settings.bevy_params.x, 0.0, 1.0);
    let samples = rt_sample_count();
    var visibility = 0.0;
    for (var sample_index = 0u; sample_index < samples; sample_index += 1u) {
        let sampled_direction = rt_sample_directional_emitter(direction_to_light, cos_theta_max, &rng, sample_index, samples);
        visibility += rt_trace_shadow_visibility(origin, sampled_direction, 1000000.0);
    }
    return visibility / f32(samples);
}

