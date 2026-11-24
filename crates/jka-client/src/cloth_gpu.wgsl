// GPU cloth: WGSL port of gpu-cloth-sim (addons/godot_gpu_cloth/shaders/compute).
//
// Reference sources:
//   cs_skin           cloth_skin.glsl       (Ghoul2 weighting, per render vertex)
//   cs_warm_start     cloth_warm_start.glsl
//   predict_particle  cloth_predict.glsl
//   solve_constraint  cloth_solve.glsl
//   collide_particle  cloth_collide.glsl
//   update_particle   cloth_update.glsl
//   cs_face_normals   cloth_normals.glsl
//   cs_vertex_normals cloth_output.glsl     (normal accumulation; buffer, not image)
//   cs_output         cloth_surface.gdshader vertex stage -> DynamicModelVertex words
//
// cs_tick is the reference `_gpu_do_simulate` substep loop in ONE workgroup:
// each reference dispatch boundary is a workgroup storage barrier, in the
// same order (predict; per iteration every colour group, then collide;
// update). Constraints inside a colour group share no particle, so the result
// is the reference's; only ~600 dispatch/barrier round trips per tick go away.
//
// cs_drivers/cs_gather build the animated target the reference calls
// `skinned_targets`. Here that is DinurdoJK's garment target: welded copies
// averaged, then free fabric blended toward its sewn attachments' motion by
// r_clothAnimationInfluence (cloth.rs `garment_targets`).
//
// The solver frame is the model (skeleton) frame in metres, as in the
// reference. JKA model space is Z-up, so the reference's solver-local
// `vec3(0, gravity, 0)` is `vec3(0, 0, gravity)` here.

struct Params {
    dt: f32,
    gravity: f32,
    particle_count: u32,
    constraint_count: u32,
    damping: f32,
    max_speed: f32,
    collider_count: u32,
    render_count: u32,
    inertia: vec3<f32>,
    max_travel: f32,
    wind: vec3<f32>,
    inv_substeps: f32,
    tri_count: u32,
    bone_count: u32,
    flip: f32,
    blend: f32,
    substeps: u32,
    iterations: u32,
    group_count: u32,
    driver_count: u32,
    influence: f32,
    pad0: f32,
    pad1: f32,
    pad2: f32,
};

struct ClothDraw {
    axis0: vec4<f32>,
    axis1: vec4<f32>,
    axis2: vec4<f32>,
    origin: vec4<f32>,
    color: vec4<f32>,
    uv_xform: vec4<f32>,
    uv_cross: vec4<f32>,
    spec_light: vec4<f32>,
    spec_viewer: vec4<f32>,
    classic_ambient: vec4<f32>,
    classic_directed: vec4<f32>,
    classic_direction: vec4<f32>,
    classic_sun_directed: vec4<f32>,
    classic_sun_direction: vec4<f32>,
    water_bands0: vec4<f32>,
    water_bands1: vec4<f32>,
    water_base: vec4<f32>,
    // x: output base vertex, y: first garment render vertex, z: render count,
    // w: bit0 classic light, bit1 JK2 encoded light multiply.
    params: vec4<u32>,
};

struct RenderVertex {
    position_uv_x: vec4<f32>,
    normal_uv_y: vec4<f32>,
    bone_indices: vec4<u32>,
    weights: vec4<f32>,
    params: vec4<u32>,
};

struct BoneMatrix {
    row0: vec4<f32>,
    row1: vec4<f32>,
    row2: vec4<f32>,
};

const METRES_PER_JKA_UNIT: f32 = 0.0254;
const NO_PARTICLE: u32 = 0xffffffffu;
const TICK_THREADS: u32 = 256u;

@group(0) @binding(0) var<storage, read_write> render_in: array<RenderVertex>;
@group(0) @binding(1) var<storage, read_write> bones: array<BoneMatrix>;
// Two vec4 per render vertex: skinned position (metres), skinned normal.
@group(0) @binding(2) var<storage, read_write> skinned_render: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> member_offsets: array<u32>;
@group(0) @binding(4) var<storage, read_write> member_list: array<u32>;
@group(0) @binding(5) var<storage, read_write> skinned_targets: array<vec4<f32>>;
@group(0) @binding(6) var<storage, read_write> positions: array<vec4<f32>>;
@group(0) @binding(7) var<storage, read_write> predicted: array<vec4<f32>>;
@group(0) @binding(8) var<storage, read_write> velocities: array<vec4<f32>>;
@group(0) @binding(9) var<storage, read_write> constraints: array<vec4<f32>>;
@group(0) @binding(10) var<storage, read_write> colliders: array<vec4<f32>>;
@group(0) @binding(11) var<storage, read_write> cloth_weights: array<vec4<f32>>;
@group(0) @binding(12) var<storage, read_write> indices: array<u32>;
@group(0) @binding(13) var<storage, read_write> face_normals: array<vec4<f32>>;
@group(0) @binding(14) var<storage, read_write> vert_tri_counts: array<u32>;
@group(0) @binding(15) var<storage, read_write> vert_tri_offsets: array<u32>;
@group(0) @binding(16) var<storage, read_write> vert_tri_list: array<u32>;
@group(0) @binding(17) var<storage, read_write> particle_normals: array<vec4<f32>>;
@group(0) @binding(18) var<storage, read_write> prev_positions: array<vec4<f32>>;
@group(0) @binding(19) var<storage, read_write> render_to_particle: array<u32>;
// Packed DynamicModelVertex words (15 per vertex), same as ghoul2_skin_compute.wgsl.
@group(0) @binding(20) var<storage, read_write> output_words: array<u32>;
// Two per particle: (driver0, driver1, driver2, count), bitcast weights.
@group(0) @binding(21) var<storage, read_write> particle_drivers: array<vec4<u32>>;
// Three rows per attachment driver: its mean skin transform this frame.
@group(0) @binding(22) var<storage, read_write> driver_mats: array<vec4<f32>>;
// Bind-pose particle position (metres) and attachment freedom.
@group(0) @binding(23) var<storage, read_write> particle_rest: array<vec4<f32>>;
// Driver -> its attachment particle.
@group(0) @binding(24) var<storage, read_write> driver_particles: array<u32>;
// Colour groups: (offset, count, -, -).
@group(0) @binding(25) var<storage, read_write> constraint_groups: array<vec4<u32>>;

@group(1) @binding(0) var<uniform> params: Params;
@group(2) @binding(1) var<uniform> draw: ClothDraw;

// ---------------------------------------------------------------------------
// cloth_skin.glsl
// ---------------------------------------------------------------------------

fn transform_point(bone: BoneMatrix, point: vec3<f32>) -> vec3<f32> {
    let p = vec4<f32>(point, 1.0);
    return vec3<f32>(dot(bone.row0, p), dot(bone.row1, p), dot(bone.row2, p));
}

fn transform_vector(bone: BoneMatrix, vector: vec3<f32>) -> vec3<f32> {
    let v = vec4<f32>(vector, 0.0);
    return vec3<f32>(dot(bone.row0, v), dot(bone.row1, v), dot(bone.row2, v));
}

fn fetch_bone(index: u32) -> BoneMatrix {
    // Reference guard: out-of-range bone = no contribution. Ghoul2 always
    // carries the weight, so clamp to a valid matrix instead of dropping it.
    return bones[min(index, params.bone_count - 1u)];
}

@compute @workgroup_size(64, 1, 1)
fn cs_skin(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= params.render_count) {
        return;
    }
    let input = render_in[i];
    let rp = input.position_uv_x.xyz;
    let w = input.weights;
    let count = input.params.x;
    // Linear blend skinning with the engine's (OpenJK RB_SurfaceGhoul) weight
    // convention: the final weight is the residual, as ghoul2_skin.wgsl does.
    let p0 = transform_point(fetch_bone(input.bone_indices.x), rp);
    var pos = p0;
    if (count == 2u) {
        let p1 = transform_point(fetch_bone(input.bone_indices.y), rp);
        pos = w.x * (p0 - p1) + p1;
    } else if (count == 3u) {
        let p1 = transform_point(fetch_bone(input.bone_indices.y), rp);
        let p2 = transform_point(fetch_bone(input.bone_indices.z), rp);
        pos = w.x * p0 + w.y * p1 + (1.0 - w.x - w.y) * p2;
    } else if (count >= 4u) {
        let p1 = transform_point(fetch_bone(input.bone_indices.y), rp);
        let p2 = transform_point(fetch_bone(input.bone_indices.z), rp);
        let p3 = transform_point(fetch_bone(input.bone_indices.w), rp);
        pos = w.x * p0 + w.y * p1 + w.z * p2 + (1.0 - w.x - w.y - w.z) * p3;
    }
    let normal = transform_vector(fetch_bone(input.bone_indices.x), input.normal_uv_y.xyz);
    skinned_render[i * 2u] = vec4<f32>(pos * METRES_PER_JKA_UNIT, 1.0);
    skinned_render[i * 2u + 1u] = vec4<f32>(normal, 0.0);
}

// ---------------------------------------------------------------------------
// Garment target (cloth.rs garment_targets / compute_attachment_drivers)
// ---------------------------------------------------------------------------

// One thread per attachment driver: the mean blended skin transform of the
// driver particle's welded render copies (ClothSurfaceFrame.skin_transforms).
@compute @workgroup_size(64, 1, 1)
fn cs_drivers(@builtin(global_invocation_id) gid: vec3<u32>) {
    let d = gid.x;
    if (d >= params.driver_count) {
        return;
    }
    let p = driver_particles[d];
    let start = member_offsets[p];
    let end = member_offsets[p + 1u];
    var r0 = vec4<f32>(0.0);
    var r1 = vec4<f32>(0.0);
    var r2 = vec4<f32>(0.0);
    for (var m = start; m < end; m++) {
        let input = render_in[member_list[m]];
        let w = input.weights;
        let idx = input.bone_indices;
        let count = min(input.params.x, 4u);
        for (var k = 0u; k < count; k++) {
            let bone = fetch_bone(idx[k]);
            r0 += bone.row0 * w[k];
            r1 += bone.row1 * w[k];
            r2 += bone.row2 * w[k];
        }
    }
    let inv = 1.0 / f32(max(end - start, 1u));
    driver_mats[d * 3u] = r0 * inv;
    driver_mats[d * 3u + 1u] = r1 * inv;
    driver_mats[d * 3u + 2u] = r2 * inv;
}

// Welded render copies share one particle (mean, like `particle_targets`);
// free fabric then follows its nearest sewn attachments' skin transforms.
@compute @workgroup_size(64, 1, 1)
fn cs_gather(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = gid.x;
    if (p >= params.particle_count) {
        return;
    }
    let start = member_offsets[p];
    let end = member_offsets[p + 1u];
    var sum = vec3<f32>(0.0);
    for (var m = start; m < end; m++) {
        sum += skinned_render[member_list[m] * 2u].xyz;
    }
    let authored = sum / f32(max(end - start, 1u));

    let rest = particle_rest[p];
    let freedom = rest.w * (1.0 - clamp(params.influence, 0.0, 1.0));
    let drivers = particle_drivers[p * 2u];
    let weights = bitcast<vec4<f32>>(particle_drivers[p * 2u + 1u]);
    var target_pos = authored;
    if (freedom > 0.0 && drivers.w > 0u) {
        let bind = vec4<f32>(rest.xyz, METRES_PER_JKA_UNIT);
        var attached = vec3<f32>(0.0);
        for (var k = 0u; k < min(drivers.w, 3u); k++) {
            let d = drivers[k];
            attached += vec3<f32>(
                dot(driver_mats[d * 3u], bind),
                dot(driver_mats[d * 3u + 1u], bind),
                dot(driver_mats[d * 3u + 2u], bind),
            ) * weights[k];
        }
        target_pos = mix(authored, attached, freedom);
    }
    skinned_targets[p] = vec4<f32>(target_pos, 1.0);
}

// ---------------------------------------------------------------------------
// cloth_warm_start.glsl
// ---------------------------------------------------------------------------

@compute @workgroup_size(64, 1, 1)
fn cs_warm_start(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= params.particle_count) {
        return;
    }
    let w = positions[idx].w;   // preserve inv_mass
    let sk = skinned_targets[idx].xyz;
    positions[idx] = vec4<f32>(sk, w);
    predicted[idx] = vec4<f32>(sk, w);
    velocities[idx] = vec4<f32>(0.0);
    prev_positions[idx] = vec4<f32>(sk, w);
}

// ---------------------------------------------------------------------------
// cloth_predict.glsl
// ---------------------------------------------------------------------------

fn predict_particle(idx: u32) {
    let w = positions[idx].w;

    // Anchored particle (inverse_mass == 0): track the skeleton-driven position
    // from the skin compute pass. Zero velocity so constraints don't pull it.
    if (w < 0.001) {
        let target_pos = skinned_targets[idx].xyz;
        positions[idx] = vec4<f32>(target_pos, 0.0);
        predicted[idx] = vec4<f32>(target_pos, 0.0);
        velocities[idx] = vec4<f32>(0.0);
        return;
    }

    var pos = positions[idx].xyz;

    // Reference-frame correction: compensate for parent node movement so free
    // particles feel inertia rather than teleporting with the solver node.
    pos -= params.inertia;

    var vel = velocities[idx].xyz;

    vel += vec3<f32>(0.0, 0.0, params.gravity) * params.dt;
    vel += params.wind * params.dt;

    let speed = length(vel);
    if (speed > params.max_speed) {
        vel *= params.max_speed / speed;
    }

    let p = pos + vel * params.dt;

    predicted[idx] = vec4<f32>(p, w);
    velocities[idx] = vec4<f32>(vel, 0.0);
}

// ---------------------------------------------------------------------------
// cloth_solve.glsl
// ---------------------------------------------------------------------------

fn solve_constraint(cidx: u32) {
    let a = u32(constraints[cidx].x);
    let b = u32(constraints[cidx].y);
    let rest = constraints[cidx].z;
    let stiffness = constraints[cidx].w;

    let pa = predicted[a].xyz;
    let pb = predicted[b].xyz;
    let wa = predicted[a].w;
    let wb = predicted[b].w;

    let delta = pb - pa;
    let dist = length(delta);

    if (dist < 1e-7) {
        return;
    }

    let w_sum = wa + wb;
    if (w_sum < 1e-7) {
        return;
    }

    let err = (dist - rest) / dist;
    let correction = delta * err * stiffness;

    predicted[a] = vec4<f32>(pa + correction * (wa / w_sum), wa);
    predicted[b] = vec4<f32>(pb - correction * (wb / w_sum), wb);
}

// ---------------------------------------------------------------------------
// cloth_collide.glsl
// ---------------------------------------------------------------------------

fn collide_particle(idx: u32) {
    let w = predicted[idx].w;
    if (w < 0.001) {
        return;
    }

    var pos = predicted[idx].xyz;
    let skin_pos = skinned_targets[idx].xyz;

    for (var c = 0u; c < params.collider_count; c++) {
        let shape_type = colliders[c * 4u + 1u].w;

        if (shape_type < 0.5) {
            // Capsule / Sphere
            let a = colliders[c * 4u].xyz;
            let b = colliders[c * 4u + 1u].xyz;
            let r = colliders[c * 4u].w;
            let ab = b - a;
            let ab2 = dot(ab, ab);

            var t_sk = 0.0;
            if (ab2 > 1e-12) {
                t_sk = clamp(dot(skin_pos - a, ab) / ab2, 0.0, 1.0);
            }
            let closest_sk = a + ab * t_sk;
            let skin_diff = skin_pos - closest_sk;
            let skin_dist = length(skin_diff);

            // Step 1 - hemisphere plane: ALWAYS run, regardless of whether pos
            // is inside or outside the collider.  The plane sits at r*0.5 from
            // the axis (halfway inside the surface) so it never crosses the
            // symmetry axis even when skin_pos clips slightly inside at a joint.
            // This establishes the correct side BEFORE the surface push, which
            // would otherwise blindly push toward the nearest surface.
            if (skin_dist > 1e-7) {
                let skin_normal = skin_diff / skin_dist;
                let proj = dot(pos - closest_sk, skin_normal);
                if (proj < r * 0.5) {
                    pos += skin_normal * (r * 0.5 - proj);
                }
            }

            // Step 2 - surface push: pos is now on the correct hemisphere.
            // Push it outside the collider surface if it is still inside.
            var t = 0.0;
            if (ab2 > 1e-12) {
                t = clamp(dot(pos - a, ab) / ab2, 0.0, 1.0);
            }
            let closest = a + ab * t;
            let diff = pos - closest;
            let dist = length(diff);
            if (dist < r && dist > 1e-7) {
                pos = closest + (diff / dist) * r;
            }
        } else {
            // Box (OBB)
            let center = colliders[c * 4u].xyz;
            let half_ext = colliders[c * 4u + 1u].xyz;
            let right = colliders[c * 4u + 2u].xyz;
            let up_dir = colliders[c * 4u + 3u].xyz;
            let fwd = cross(right, up_dir);

            let d = pos - center;
            var local_pos = vec3<f32>(dot(d, right), dot(d, up_dir), dot(d, fwd));

            if (all(abs(local_pos) < half_ext)) {
                let dist_to_face = half_ext - abs(local_pos);
                if (dist_to_face.x <= dist_to_face.y && dist_to_face.x <= dist_to_face.z) {
                    local_pos.x = sign(local_pos.x) * half_ext.x;
                } else if (dist_to_face.y <= dist_to_face.z) {
                    local_pos.y = sign(local_pos.y) * half_ext.y;
                } else {
                    local_pos.z = sign(local_pos.z) * half_ext.z;
                }
                pos = center + right * local_pos.x + up_dir * local_pos.y + fwd * local_pos.z;
            }
        }
    }

    predicted[idx] = vec4<f32>(pos, w);
}

// ---------------------------------------------------------------------------
// cloth_update.glsl
// ---------------------------------------------------------------------------

fn update_particle(idx: u32) {
    let old_pos = positions[idx].xyz;
    var new_pos = predicted[idx].xyz;
    let w = positions[idx].w;
    let cloth_w = cloth_weights[idx].x;

    // Anchored particles (w == 0) are already snapped to skinned_targets in the
    // predict pass, so skip velocity recovery for them.
    if (w < 0.001) {
        return;
    }

    // Max travel: hard limit on how far each particle can stray from its
    // animated skin position. Fully simulated particles get the full budget;
    // blend-zone particles get a proportionally tighter limit.
    if (params.max_travel > 0.0) {
        let diff = new_pos - skinned_targets[idx].xyz;
        let dist = length(diff);
        let lim = cloth_w * params.max_travel;
        if (lim > 1e-7 && dist > lim) {
            new_pos = skinned_targets[idx].xyz + (diff / dist) * lim;
        }
    }

    // Recover velocity from position delta.
    var vel = (new_pos - old_pos) / max(params.dt, 1e-7);
    vel *= params.damping;

    // Blend-zone: lerp simulated position toward skeleton-driven position.
    // cloth_weight == 1.0  -> fully simulated (no lerp)
    // cloth_weight in (0,1) -> partial skeleton influence
    if (cloth_w < 0.999) {
        let sub_w = pow(cloth_w, params.inv_substeps);
        new_pos = mix(skinned_targets[idx].xyz, new_pos, sub_w);
        vel *= sub_w;
    }

    positions[idx] = vec4<f32>(new_pos, w);
    velocities[idx] = vec4<f32>(vel, 0.0);
}

// ---------------------------------------------------------------------------
// _gpu_do_simulate: one cloth tick, every reference dispatch in order.
// ---------------------------------------------------------------------------

@compute @workgroup_size(256, 1, 1)
fn cs_tick(@builtin(local_invocation_index) lid: u32) {
    for (var s = 0u; s < params.substeps; s++) {
        // PREDICT
        for (var i = lid; i < params.particle_count; i += TICK_THREADS) {
            predict_particle(i);
        }
        storageBarrier();

        // SOLVE + COLLIDE interleaved
        for (var iter = 0u; iter < params.iterations; iter++) {
            for (var g = 0u; g < params.group_count; g++) {
                let range = constraint_groups[g];
                for (var c = lid; c < range.y; c += TICK_THREADS) {
                    solve_constraint(range.x + c);
                }
                storageBarrier();
            }
            if (params.collider_count > 0u) {
                for (var i = lid; i < params.particle_count; i += TICK_THREADS) {
                    collide_particle(i);
                }
                storageBarrier();
            }
        }

        // UPDATE
        for (var i = lid; i < params.particle_count; i += TICK_THREADS) {
            update_particle(i);
        }
        storageBarrier();
    }
}

// ---------------------------------------------------------------------------
// cloth_normals.glsl (one thread per triangle)
// ---------------------------------------------------------------------------

@compute @workgroup_size(64, 1, 1)
fn cs_face_normals(@builtin(global_invocation_id) gid: vec3<u32>) {
    let t = gid.x;
    if (t >= params.tri_count) {
        return;
    }

    let i0 = indices[t * 3u + 0u];
    let i1 = indices[t * 3u + 1u];
    let i2 = indices[t * 3u + 2u];

    let v0 = positions[i0].xyz;
    let n = cross(positions[i1].xyz - v0, positions[i2].xyz - v0) * params.flip;
    face_normals[t] = vec4<f32>(n, 0.0);
}

// ---------------------------------------------------------------------------
// cloth_output.glsl normal accumulation (one thread per particle)
// ---------------------------------------------------------------------------

@compute @workgroup_size(64, 1, 1)
fn cs_vertex_normals(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= params.particle_count) {
        return;
    }

    let count = vert_tri_counts[idx];
    let offset = vert_tri_offsets[idx];
    var normal = vec3<f32>(0.0);
    for (var t = 0u; t < count; t++) {
        normal += face_normals[vert_tri_list[offset + t]].xyz;
    }
    let nl = dot(normal, normal);
    if (nl > 1e-8) {
        normal = normal * inverseSqrt(nl);
    } else {
        normal = vec3<f32>(0.0, 0.0, 1.0);
    }
    particle_normals[idx] = vec4<f32>(normal, 0.0);
}

// ---------------------------------------------------------------------------
// Output: cloth_surface.gdshader's vertex stage (simulated positions/normals
// replace the skinned ones), written as this renderer's DynamicModelVertex.
// Material terms mirror ghoul2_skin_compute.wgsl.
// ---------------------------------------------------------------------------

fn model_to_world(model_position: vec3<f32>) -> vec3<f32> {
    return draw.origin.xyz
        + draw.axis0.xyz * model_position.x
        + draw.axis1.xyz * model_position.y
        + draw.axis2.xyz * model_position.z;
}

fn model_vector_to_world(v: vec3<f32>) -> vec3<f32> {
    return draw.axis0.xyz * v.x + draw.axis1.xyz * v.y + draw.axis2.xyz * v.z;
}

fn jka_to_render(world: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(world.x, world.z, -world.y);
}

fn srgb_decode(v: f32) -> f32 {
    if (v <= 0.04045) {
        return v / 12.92;
    }
    return pow((v + 0.055) / 1.055, 2.4);
}

// q3 RB_CalcSpecularAlpha: (reflected light . viewer)^4, all in JKA world space.
fn specular_alpha(world: vec3<f32>, world_normal: vec3<f32>) -> f32 {
    if (draw.spec_light.w == 0.0) {
        return 1.0;
    }
    let normal = normalize(world_normal);
    let light_dir = normalize(draw.spec_light.xyz - world);
    let reflected = normal * (2.0 * dot(normal, light_dir)) - light_dir;
    let viewer = normalize(draw.spec_viewer.xyz - world);
    let l = dot(reflected, viewer);
    if (l < 0.0) {
        return 0.0;
    }
    let l2 = l * l;
    return min(l2 * l2, 1.0);
}

fn write_f32(word: u32, value: f32) {
    output_words[word] = bitcast<u32>(value);
}

@compute @workgroup_size(64, 1, 1)
fn cs_output(@builtin(global_invocation_id) gid: vec3<u32>) {
    let local = gid.x;
    if (local >= draw.params.z) {
        return;
    }
    let global = draw.params.y + local;
    let particle = render_to_particle[global];

    var model_m: vec3<f32>;
    var model_normal: vec3<f32>;
    if (particle == NO_PARTICLE) {
        model_m = skinned_render[global * 2u].xyz;
        model_normal = skinned_render[global * 2u + 1u].xyz;
    } else {
        let current = positions[particle];
        if (current.w < 0.001) {
            // Attachments follow this frame's pose, not the last fixed tick.
            model_m = skinned_targets[particle].xyz;
        } else {
            model_m = mix(prev_positions[particle].xyz, current.xyz, params.blend);
        }
        model_normal = particle_normals[particle].xyz;
    }

    let world = model_to_world(model_m / METRES_PER_JKA_UNIT);
    let world_normal = model_vector_to_world(model_normal);
    let render_position = jka_to_render(world);
    var render_normal = jka_to_render(world_normal);
    let nl = dot(render_normal, render_normal);
    render_normal = select(vec3<f32>(0.0, 1.0, 0.0), render_normal * inverseSqrt(nl), nl > 1e-12);

    var color = draw.color;
    if ((draw.params.w & 1u) != 0u) {
        var incoming = 0.0;
        let light_dir = draw.classic_direction.xyz;
        let light_len_sq = dot(light_dir, light_dir);
        if (light_len_sq > 1.0e-8) {
            incoming = max(dot(render_normal, light_dir * inverseSqrt(light_len_sq)), 0.0);
        }
        var sun_incoming = 0.0;
        let sun_dir = draw.classic_sun_direction.xyz;
        let sun_len_sq = dot(sun_dir, sun_dir);
        if (sun_len_sq > 1.0e-8) {
            sun_incoming = max(dot(render_normal, sun_dir * inverseSqrt(sun_len_sq)), 0.0);
        }
        var lighting = clamp(
            (draw.classic_ambient.xyz
                + incoming * draw.classic_directed.xyz
                + sun_incoming * draw.classic_sun_directed.xyz) / 255.0,
            vec3<f32>(0.0),
            vec3<f32>(1.0),
        );
        if ((draw.params.w & 2u) != 0u) {
            lighting = vec3<f32>(srgb_decode(lighting.x), srgb_decode(lighting.y), srgb_decode(lighting.z));
        }
        color = vec4<f32>(color.rgb * lighting, color.a);
    }
    color.a = color.a * specular_alpha(world, world_normal);

    let input = render_in[global];
    let original_uv = vec2<f32>(input.position_uv_x.w, input.normal_uv_y.w);
    let uv = original_uv * draw.uv_xform.xy + draw.uv_xform.zw
        + vec2<f32>(original_uv.y * draw.uv_cross.x, original_uv.x * draw.uv_cross.y);

    var wetness = 0.0;
    if (draw.water_base.w > 0.5) {
        let band = clamp((render_position.y - draw.water_base.x) * 0.125, 0.0, 7.0);
        let levels = array<f32, 8>(
            draw.water_bands0.x, draw.water_bands0.y, draw.water_bands0.z, draw.water_bands0.w,
            draw.water_bands1.x, draw.water_bands1.y, draw.water_bands1.z, draw.water_bands1.w
        );
        let lo = u32(band);
        wetness = mix(levels[lo], levels[min(lo + 1u, 7u)], fract(band));
    }

    let base = (draw.params.x + local) * 15u;
    write_f32(base + 0u, render_position.x);
    write_f32(base + 1u, render_position.y);
    write_f32(base + 2u, render_position.z);
    write_f32(base + 3u, render_normal.x);
    write_f32(base + 4u, render_normal.y);
    write_f32(base + 5u, render_normal.z);
    write_f32(base + 6u, uv.x);
    write_f32(base + 7u, uv.y);
    write_f32(base + 8u, color.x);
    write_f32(base + 9u, color.y);
    write_f32(base + 10u, color.z);
    write_f32(base + 11u, color.w);
    write_f32(base + 12u, 0.0);
    write_f32(base + 13u, wetness);
    write_f32(base + 14u, 0.0);
}
