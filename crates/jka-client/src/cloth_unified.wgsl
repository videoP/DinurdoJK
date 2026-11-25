// Port of Johann Hotzel, unified-solver, commit 5e00799568ff9249bfe3c809818cbdcf93a79619.
// MIT; see cloth_unified.LICENSE. Source: Runtime/Shaders/UnifiedSolver.compute.
// WGSL storage alignment differs from HLSL. JKA is Z-up. Particle display
// color/visibility are not solver inputs. Pack/scatter/snapshot are JKA adapters.
struct Particle {
    position: vec3<f32>, inv_mass: f32,
    velocity: vec3<f32>, phase: i32,
    prev_position: vec3<f32>, pad: f32,
};
struct Constraint {
    a: u32, b: u32, rest_length: f32, compliance: f32,
    break_force: f32, damping: f32,
};
struct Delta { x: atomic<i32>, y: atomic<i32>, z: atomic<i32>, count: atomic<i32> };
// Sphere/capsule/box arrays occupy contiguous ranges in one buffer.
struct Collider { a: vec4<f32>, b: vec4<f32>, x: vec4<f32>, y: vec4<f32>, z: vec4<f32> };
struct Params {
    particle_count: u32, constraint_count: u32, table_size: u32, sphere_count: u32,
    capsule_count: u32, box_count: u32, dt: f32, sor: f32,
    gravity: vec3<f32>, damping: f32,
    cell_size: f32, radius: f32, friction_static: f32, friction_kinetic: f32,
    max_depenetration_speed: f32, ground_z: f32, rigid_count: u32, pad1: f32,
};
struct Transfer {
    new_x: vec4<f32>, new_y: vec4<f32>, new_z: vec4<f32>, new_origin: vec4<f32>,
    inv_x: vec4<f32>, inv_y: vec4<f32>, inv_z: vec4<f32>,
    offset: u32, count: u32, phase: i32, reset: u32,
};
@group(0) @binding(0) var<storage, read_write> particles: array<Particle>;
@group(0) @binding(1) var<storage, read_write> constraints: array<Constraint>;
@group(0) @binding(2) var<storage, read_write> deltas: array<Delta>;
@group(0) @binding(3) var<storage, read_write> hash_head: array<atomic<i32>>;
@group(0) @binding(4) var<storage, read_write> hash_next: array<i32>;
@group(0) @binding(5) var<storage, read_write> colliders: array<Collider>;
@group(0) @binding(6) var<storage, read_write> render_previous: array<vec4<f32>>;
@group(0) @binding(7) var<storage, read_write> local_positions: array<vec4<f32>>;
@group(0) @binding(9) var<storage, read_write> local_previous: array<vec4<f32>>;
@group(0) @binding(10) var<storage, read_write> local_targets: array<vec4<f32>>;
// World-space state belongs to the garment even if pool ranges are reordered.
@group(0) @binding(11) var<storage, read_write> instance_particles: array<Particle>;
@group(0) @binding(12) var<storage, read_write> instance_previous: array<vec4<f32>>;
// Shape-matching buffers: the cloth integration registers no rigid bodies.
struct RigidBody {
    particle_offset: u32, particle_count: u32, pad0: u32, pad1: u32,
    quaternion: vec4<f32>, xcm: vec3<f32>, pad2: f32,
};
@group(0) @binding(13) var<storage, read_write> rigid_bodies: array<RigidBody>;
@group(0) @binding(14) var<storage, read_write> rigid_particle_indices: array<u32>;
@group(0) @binding(15) var<storage, read_write> rigid_rest_offsets: array<vec4<f32>>;
@group(1) @binding(0) var<uniform> params: Params;
@group(2) @binding(0) var<uniform> transfer: Transfer;
const FP_SCALE: f32 = 100000.0;
const FP_INV: f32 = 1.0 / FP_SCALE;

// Predict: semi-implicit Euler; fixed particles are not integrated.
@compute @workgroup_size(256)
fn predict(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.particle_count) { return; }
    var p = particles[id.x];
    if (p.inv_mass == 0.0) { return; }
    p.prev_position = p.position;
    p.velocity += params.dt * params.gravity;
    p.position += params.dt * p.velocity;
    particles[id.x] = p;
}
@compute @workgroup_size(256)
fn clear_deltas(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.particle_count) { return; }
    atomicStore(&deltas[id.x].x, 0);
    atomicStore(&deltas[id.x].y, 0);
    atomicStore(&deltas[id.x].z, 0);
    atomicStore(&deltas[id.x].count, 0);
}
// HLSL round() is nearest-even. Spell it explicitly rather than rely on WGSL
// implementation latitude at ties.
fn round_even(x: f32) -> f32 {
    let lo = floor(x);
    let frac = x - lo;
    if (frac == 0.5) { return select(lo, lo + 1.0, (i32(lo) & 1) != 0); }
    return select(lo, lo + 1.0, frac > 0.5);
}
fn accumulate(i: u32, d: vec3<f32>) {
    atomicAdd(&deltas[i].x, i32(round_even(d.x * FP_SCALE)));
    atomicAdd(&deltas[i].y, i32(round_even(d.y * FP_SCALE)));
    atomicAdd(&deltas[i].z, i32(round_even(d.z * FP_SCALE)));
    atomicAdd(&deltas[i].count, 1);
}
@compute @workgroup_size(256)
fn solve_distance(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.constraint_count) { return; }
    let c = constraints[id.x];
    if (c.rest_length < 0.0) { return; }
    let pa = particles[c.a];
    let pb = particles[c.b];
    let w_sum = pa.inv_mass + pb.inv_mass;
    if (w_sum == 0.0) { return; }
    let diff = pb.position - pa.position;
    let dist = length(diff);
    if (dist < 1e-7) { return; }
    let n = diff / dist;
    let alpha_tilde = c.compliance / (params.dt * params.dt);
    let gamma = (c.compliance * c.damping) / params.dt;
    let constraint = dist - c.rest_length;
    let damping_term = gamma * dot(n, (pb.position - pb.prev_position) - (pa.position - pa.prev_position));
    let dlambda = (-constraint - damping_term) / ((1.0 + gamma) * w_sum + alpha_tilde);
    if (c.break_force > 0.0) {
        let force = abs(dlambda) / params.dt;
        if (force > c.break_force) {
            constraints[id.x].rest_length = -1.0;
            return;
        }
    }
    accumulate(c.a, -pa.inv_mass * dlambda * n);
    accumulate(c.b, pb.inv_mass * dlambda * n);
}
@compute @workgroup_size(256)
fn apply_deltas(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.particle_count) { return; }
    var p = particles[id.x];
    if (p.inv_mass == 0.0) { return; }
    let count = atomicLoad(&deltas[id.x].count);
    if (count == 0) { return; }
    let delta = vec3<f32>(f32(atomicLoad(&deltas[id.x].x)), f32(atomicLoad(&deltas[id.x].y)), f32(atomicLoad(&deltas[id.x].z))) * FP_INV;
    p.position += params.sor * delta / f32(count);
    particles[id.x] = p;
}
fn world_to_cell(pos: vec3<f32>) -> vec3<i32> {
    return vec3<i32>(floor(pos / params.cell_size));
}
fn cell_hash(cell: vec3<i32>) -> u32 {
    // Bitcast before multiplying to preserve HLSL's wrapping 32-bit bits.
    // WGSL forbids mixing `*` and `^` unparenthesized; same grouping as HLSL precedence.
    let h = (bitcast<u32>(cell.x) * 92837111u) ^ (bitcast<u32>(cell.y) * 689287499u) ^ (bitcast<u32>(cell.z) * 283923481u);
    return h % params.table_size;
}
@compute @workgroup_size(256)
fn clear_hash(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.table_size) { return; }
    atomicStore(&hash_head[id.x], -1);
}
@compute @workgroup_size(256)
fn build_hash(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.particle_count) { return; }
    let hash = cell_hash(world_to_cell(particles[id.x].position));
    hash_next[id.x] = atomicExchange(&hash_head[hash], i32(id.x));
}
@compute @workgroup_size(256)
fn solve_contact(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if (i >= params.particle_count) { return; }
    let pi = particles[i];
    if (pi.inv_mass == 0.0) { return; }
    let cell_i = world_to_cell(pi.position);
    for (var dx = -1; dx <= 1; dx++) {
        for (var dy = -1; dy <= 1; dy++) {
            for (var dz = -1; dz <= 1; dz++) {
                let hash = cell_hash(cell_i + vec3<i32>(dx, dy, dz));
                var j = atomicLoad(&hash_head[hash]);
                while (j >= 0) {
                    let ju = u32(j);
                    if (ju != i) {
                        let pj = particles[ju];
                        if (pi.phase != 0 && pi.phase == pj.phase) {
                            j = hash_next[ju];
                            continue;
                        }
                        let delta = pi.position - pj.position;
                        let dist2 = dot(delta, delta);
                        let min_d = params.radius + params.radius;
                        if (dist2 < min_d * min_d && dist2 > 1e-10) {
                            let dist = sqrt(dist2);
                            let n = delta / dist;
                            var penetration = min_d - dist;
                            let max_corr = params.max_depenetration_speed * params.dt;
                            penetration = min(penetration, max_corr);
                            let w_sum = pi.inv_mass + pj.inv_mass;
                            if (w_sum > 0.0) {
                                let s = penetration / w_sum;
                                accumulate(i, n * (s * pi.inv_mass));
                                accumulate(ju, -n * (s * pj.inv_mass));
                                let dx_rel = (pi.position - pi.prev_position) - (pj.position - pj.prev_position);
                                let dxt = dx_rel - dot(dx_rel, n) * n;
                                let len = length(dxt);
                                if (len > 1e-7) {
                                    let limit = select(params.friction_kinetic * penetration, len, len < params.friction_static * penetration);
                                    let scale = min(len, limit);
                                    let t_dir = dxt / len;
                                    accumulate(i, -t_dir * scale * (pi.inv_mass / w_sum));
                                    accumulate(ju, t_dir * scale * (pj.inv_mass / w_sum));
                                }
                            }
                        }
                    }
                    j = hash_next[ju];
                }
            }
        }
    }
}
// Upstream ground plane, with Y converted to JKA Z. Not dispatched for BSP
// maps: no map-wide ground plane exists in DinurdoJK.
@compute @workgroup_size(256)
fn solve_ground(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.particle_count) { return; }
    var p = particles[id.x];
    if (p.inv_mass == 0.0) { return; }
    let min_z = params.ground_z + params.radius;
    if (p.position.z < min_z) {
        let penetration = min_z - p.position.z;
        p.position.z = min_z;
        let dx = p.position - p.prev_position;
        let dxt = dx.xy;
        let len = length(dxt);
        if (len > 1e-7) {
            let limit = select(params.friction_kinetic * penetration, len, len < params.friction_static * penetration);
            let correction = dxt / len * min(len, limit);
            p.position.x -= correction.x;
            p.position.y -= correction.y;
        }
    }
    particles[id.x] = p;
}
// Identical normal/previous-position/friction projection shared by upstream
// sphere, capsule and box kernels. Capture displacement before projection.
fn primitive_contact(p_in: Particle, closest: vec3<f32>, n: vec3<f32>, min_dist: f32, penetration: f32) -> Particle {
    var p = p_in;
    let dx = p.position - p.prev_position;
    p.position = closest + n * min_dist;
    let prev_diff = p.prev_position - closest;
    let prev_proj = dot(prev_diff, n);
    if (prev_proj < min_dist) { p.prev_position += (min_dist - prev_proj) * n; }
    let dx_t = dx - dot(dx, n) * n;
    let len = length(dx_t);
    if (len > 1e-7) {
        let limit = select(params.friction_kinetic * penetration, len, len < params.friction_static * penetration);
        p.position -= (dx_t / len) * min(len, limit);
    }
    return p;
}
@compute @workgroup_size(256)
fn solve_sphere(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.particle_count) { return; }
    var p = particles[id.x];
    if (p.inv_mass == 0.0) { return; }
    for (var i = 0u; i < params.sphere_count; i++) {
        let sc = colliders[i];
        let diff = p.position - sc.a.xyz;
        let dist = length(diff);
        let min_dist = sc.a.w + params.radius;
        if (dist < min_dist && dist > 1e-7) {
            p = primitive_contact(p, sc.a.xyz, diff / dist, min_dist, min_dist - dist);
        }
    }
    particles[id.x] = p;
}
@compute @workgroup_size(256)
fn solve_capsule(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.particle_count) { return; }
    var p = particles[id.x];
    if (p.inv_mass == 0.0) { return; }
    for (var i = 0u; i < params.capsule_count; i++) {
        let cc = colliders[params.sphere_count + i];
        let ab = cc.b.xyz - cc.a.xyz;
        let ab_len2 = dot(ab, ab);
        var closest = cc.a.xyz;
        if (ab_len2 >= 1e-10) {
            let t = clamp(dot(p.position - cc.a.xyz, ab) / ab_len2, 0.0, 1.0);
            closest += t * ab;
        }
        let diff = p.position - closest;
        let dist = length(diff);
        let min_dist = cc.a.w + params.radius;
        if (dist < min_dist && dist > 1e-7) {
            p = primitive_contact(p, closest, diff / dist, min_dist, min_dist - dist);
        }
    }
    particles[id.x] = p;
}
@compute @workgroup_size(256)
fn solve_box(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.particle_count) { return; }
    var p = particles[id.x];
    if (p.inv_mass == 0.0) { return; }
    for (var i = 0u; i < params.box_count; i++) {
        let bc = colliders[params.sphere_count + params.capsule_count + i];
        let rel = p.position - bc.a.xyz;
        let local = vec3<f32>(dot(rel, bc.x.xyz), dot(rel, bc.y.xyz), dot(rel, bc.z.xyz));
        let clamped = clamp(local, -bc.b.xyz, bc.b.xyz);
        let diff_local = local - clamped;
        let dist = length(diff_local);
        var n: vec3<f32>;
        var closest: vec3<f32>;
        var penetration: f32;
        if (dist > 1e-7) {
            if (dist >= params.radius) { continue; }
            let n_local = diff_local / dist;
            n = n_local.x * bc.x.xyz + n_local.y * bc.y.xyz + n_local.z * bc.z.xyz;
            closest = bc.a.xyz + clamped.x * bc.x.xyz + clamped.y * bc.y.xyz + clamped.z * bc.z.xyz;
            penetration = params.radius - dist;
        } else {
            let dist_to_face = bc.b.xyz - abs(local);
            var n_local: vec3<f32>;
            if (dist_to_face.x <= dist_to_face.y && dist_to_face.x <= dist_to_face.z) {
                n_local = vec3<f32>(select(-1.0, 1.0, local.x >= 0.0), 0.0, 0.0);
            } else if (dist_to_face.y <= dist_to_face.z) {
                n_local = vec3<f32>(0.0, select(-1.0, 1.0, local.y >= 0.0), 0.0);
            } else {
                n_local = vec3<f32>(0.0, 0.0, select(-1.0, 1.0, local.z >= 0.0));
            }
            let min_face_dist = min(dist_to_face.x, min(dist_to_face.y, dist_to_face.z));
            n = n_local.x * bc.x.xyz + n_local.y * bc.y.xyz + n_local.z * bc.z.xyz;
            penetration = params.radius + min_face_dist;
            let face_local = local + n_local * min_face_dist;
            closest = bc.a.xyz + face_local.x * bc.x.xyz + face_local.y * bc.y.xyz + face_local.z * bc.z.xyz;
        }
        p = primitive_contact(p, closest, n, params.radius, penetration);
    }
    particles[id.x] = p;
}
@compute @workgroup_size(256)
fn update_velocity(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.particle_count) { return; }
    var p = particles[id.x];
    if (p.inv_mass == 0.0) { return; }
    p.velocity = (p.position - p.prev_position) / params.dt;
    p.velocity *= max(0.0, 1.0 - params.damping * params.dt);
    particles[id.x] = p;
}
// SolveRigidBody: direct shape-matching kernel port, included for source
// coverage. Not dispatched by the cloth-only host (zero registered bodies).
fn quat_to_matrix(q: vec4<f32>) -> mat3x3<f32> {
    let xx = q.x*q.x; let yy = q.y*q.y; let zz = q.z*q.z;
    let xy = q.x*q.y; let xz = q.x*q.z; let yz = q.y*q.z;
    let wx = q.w*q.x; let wy = q.w*q.y; let wz = q.w*q.z;
    // WGSL matrices are constructed from columns, unlike HLSL's row order.
    return mat3x3<f32>(
        vec3<f32>(1.0-2.0*(yy+zz), 2.0*(xy+wz), 2.0*(xz-wy)),
        vec3<f32>(2.0*(xy-wz), 1.0-2.0*(xx+zz), 2.0*(yz+wx)),
        vec3<f32>(2.0*(xz+wy), 2.0*(yz-wx), 1.0-2.0*(xx+yy)));
}
fn quat_mul(a: vec4<f32>, b: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(
        a.w*b.x + a.x*b.w + a.y*b.z - a.z*b.y,
        a.w*b.y - a.x*b.z + a.y*b.w + a.z*b.x,
        a.w*b.z + a.x*b.y - a.y*b.x + a.z*b.w,
        a.w*b.w - a.x*b.x - a.y*b.y - a.z*b.z);
}
@compute @workgroup_size(64)
fn solve_rigid_body(@builtin(global_invocation_id) id: vec3<u32>) {
    let rigid_id = id.x;
    if (rigid_id >= params.rigid_count) { return; }
    let rb = rigid_bodies[rigid_id];
    let n = rb.particle_count;
    let base_idx = rb.particle_offset;
    var xcm = vec3<f32>(0.0);
    var total_mass = 0.0;
    for (var i = 0u; i < n; i++) {
        let p = particles[rigid_particle_indices[base_idx+i]];
        var m = 0.0;
        if (p.inv_mass > 0.0) { m = 1.0 / p.inv_mass; }
        xcm += m*p.position;
        total_mass += m;
    }
    if (total_mass < 1e-9) { return; }
    xcm /= total_mass;
    var ac0 = vec3<f32>(0.0);
    var ac1 = vec3<f32>(0.0);
    var ac2 = vec3<f32>(0.0);
    for (var i = 0u; i < n; i++) {
        let p = particles[rigid_particle_indices[base_idx+i]];
        var m = 0.0;
        if (p.inv_mass > 0.0) { m = 1.0 / p.inv_mass; }
        let p_rel = p.position - xcm;
        let q_rest = rigid_rest_offsets[base_idx+i].xyz;
        ac0 += m*q_rest.x*p_rel;
        ac1 += m*q_rest.y*p_rel;
        ac2 += m*q_rest.z*p_rel;
    }
    var q = rb.quaternion;
    for (var iter = 0; iter < 20; iter++) {
        let r = quat_to_matrix(q);
        let r0 = r[0]; let r1 = r[1]; let r2 = r[2];
        let num = cross(r0, ac0) + cross(r1, ac1) + cross(r2, ac2);
        let den = abs(dot(r0, ac0) + dot(r1, ac1) + dot(r2, ac2)) + 1e-12;
        let omega = num / den;
        let w = length(omega);
        if (w < 1e-12) { break; }
        let axis = omega / w;
        let half_w = w * 0.5;
        let dq = vec4<f32>(axis*sin(half_w), cos(half_w));
        q = normalize(quat_mul(dq, q));
    }
    rigid_bodies[rigid_id].quaternion = q;
    rigid_bodies[rigid_id].xcm = xcm;
    let r_final = quat_to_matrix(q);
    for (var i = 0u; i < n; i++) {
        let p_idx = rigid_particle_indices[base_idx+i];
        var p = particles[p_idx];
        if (p.inv_mass == 0.0) { continue; }
        p.position = r_final * rigid_rest_offsets[base_idx+i].xyz + xcm;
        particles[p_idx] = p;
    }
}

// DinurdoJK integration only: keep state in world metres on the GPU. A
// moving model never repeatedly transforms its simulated state through a
// local-space round trip; only the presentation output is transformed back.
@compute @workgroup_size(256)
fn pack(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= transfer.count) { return; }
    let i = id.x;
    let j = transfer.offset + i;
    let new_axis = mat3x3<f32>(transfer.new_x.xyz, transfer.new_y.xyz, transfer.new_z.xyz);
    var p: Particle;
    var previous: vec4<f32>;
    if (transfer.reset != 0u) {
        let q = local_positions[i];
        let world = new_axis * q.xyz + transfer.new_origin.xyz;
        p = Particle(world, q.w, vec3<f32>(0.0), transfer.phase, world, 0.0);
        previous = vec4<f32>(world, q.w);
    } else {
        p = instance_particles[i];
        p.phase = transfer.phase;
        previous = instance_previous[i];
    }
    if (p.inv_mass == 0.0) {
        p.position = new_axis * local_targets[i].xyz + transfer.new_origin.xyz;
        p.velocity = vec3<f32>(0.0);
        // Upstream fixed particles do not integrate; only the game supplies
        // the current kinematic anchor position before each world solve.
        p.prev_position = p.position;
    }
    particles[j] = p;
    render_previous[j] = previous;
}
@compute @workgroup_size(256)
fn snapshot(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.particle_count) { return; }
    let p = particles[id.x];
    render_previous[id.x] = vec4<f32>(p.position, p.inv_mass);
}
@compute @workgroup_size(256)
fn scatter(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= transfer.count) { return; }
    let i = id.x;
    let j = transfer.offset + i;
    let p = particles[j];
    let previous = render_previous[j];
    instance_particles[i] = p;
    instance_previous[i] = previous;
    let inv = mat3x3<f32>(transfer.inv_x.xyz, transfer.inv_y.xyz, transfer.inv_z.xyz);
    local_positions[i] = vec4<f32>(inv * (p.position - transfer.new_origin.xyz), p.inv_mass);
    local_previous[i] = vec4<f32>(inv * (previous.xyz - transfer.new_origin.xyz), p.inv_mass);
}
