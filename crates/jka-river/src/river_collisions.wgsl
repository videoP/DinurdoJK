// Moving-body collisions for the sparse river pool: UE 5.8.1 WaterAdvanced
// Grid2D_SW_River PreSim (simulation stage 5) Grid2D_SW_UpdateCollisions with
// User.Collide_Meshes (NiagaraDataInterfaceRigidMeshCollisionQuery), as
// cached in river-reference/Grid2D_SW_River_Emitter.*.generated.hlsl:
//   - sample point = cell centre at Z = previous WaterHeight
//     (UseWaterHeightForCollision = true);
//   - Collide_Meshes: closest element by primitive distance (box projection,
//     GetBoxProjection), no smear, no simple path, CollisionOffset 0,
//     SDFSoftCollisionFalloff 0 -> AreaFraction 1 when Dist <= 0;
//   - inside: IsCollider = AreaFraction, CollisionVelocity = element
//     velocity (XY, grid-local) + the border pass's zero velocity.
// Upstream recomputes the grid's IsCollider/CollisionVelocity every frame
// from the border rule. Here the static bed already holds the border and void
// colliders, so only the bodies' footprints are rewritten: restore the
// previous footprints' collider values, save the new ones, stamp.
// JKA players are axis-aligned boxes (their collision hull), so only the box
// branch of the primitive query is needed. Velocity scale (the menu's
// "Deformation strength", 1 = native) multiplies the element velocity.
//
// Sparse-only deviation (documented): a dry cell with no wet 4-neighbour is
// not stamped. Upstream's dense rectangle has no residency; here a dry stamped
// cell would carry the body velocity into its state, read as non-pristine and
// grow residency after a player walking on dry land.

struct Body {
    centre: vec4<f32>,   // lattice-local cm (x, y, z relative to the river origin)
    extent: vec4<f32>,   // full box size cm (BoxExtent; half = 0.5 * extent)
    velocity: vec4<f32>, // cm/s, grid-local
}
struct Collisions {
    counts: vec4<u32>,    // stamped footprints, restored footprints, footprint side, unused
    table: vec4<u32>,     // table width, table height (tiles), tile size, log2 tile size
    dims: vec4<i32>,      // lattice cells x, y
    stepping: vec4<f32>,  // dx cm, velocity scale, unused, unused
    origins: array<vec4<i32>, 16>, // xy stamped footprint corner, zw restored corner
    bodies: array<Body, 16>,
}
@group(0) @binding(0) var<uniform> collisions: Collisions;
@group(0) @binding(1) var<storage, read> tile_slots: array<i32>;
@group(0) @binding(2) var<storage, read_write> geometry: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> saved: array<f32>;
@group(0) @binding(4) var<storage, read> water: array<vec4<f32>>;

fn in_bounds(at: vec2<i32>) -> bool {
    return all(at >= vec2<i32>(0)) && all(at < collisions.dims.xy);
}
// Storage index of a resident cell, or -1.
fn cell_index(at: vec2<i32>) -> i32 {
    if !in_bounds(at) { return -1; }
    let tile = vec2<u32>(at) >> vec2<u32>(collisions.table.w);
    let slot = tile_slots[tile.y * collisions.table.x + tile.x];
    if slot < 0 { return -1; }
    let lane = vec2<u32>(at) & vec2<u32>(collisions.table.z - 1u);
    return slot * i32(collisions.table.z * collisions.table.z) + i32(lane.y * collisions.table.z + lane.x);
}
fn saved_index(body: u32, lane: vec2<u32>) -> u32 {
    let side = collisions.counts.z;
    return (body * side + lane.y) * side + lane.x;
}
fn footprint_lane(id: vec3<u32>) -> bool {
    return all(id.xy < vec2<u32>(collisions.counts.z));
}

// The previous footprints' static collider values (bed collision velocity is
// always zero in this port: capture writes (bottom, collider, 0, 0)).
@compute @workgroup_size(8, 8)
fn restore_bodies(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.z >= collisions.counts.y || !footprint_lane(id) { return; }
    let i = cell_index(collisions.origins[id.z].zw + vec2<i32>(id.xy));
    let value = saved[saved_index(id.z, id.xy)];
    // Negative: the cell was not resident when saved (never stamped). A tile
    // committed since then already holds its own static collider.
    if i < 0 || value < 0.0 { return; }
    let base = geometry[i];
    geometry[i] = vec4<f32>(base.x, value, 0.0, 0.0);
}

@compute @workgroup_size(8, 8)
fn save_bodies(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.z >= collisions.counts.x || !footprint_lane(id) { return; }
    let i = cell_index(collisions.origins[id.z].xy + vec2<i32>(id.xy));
    if i < 0 {
        saved[saved_index(id.z, id.xy)] = -1.0;
        return;
    }
    saved[saved_index(id.z, id.xy)] = geometry[i].y;
}

fn depth_at(at: vec2<i32>) -> f32 {
    let i = cell_index(at);
    if i < 0 { return 0.0; }
    return water[i].y;
}

// GetBoxProjection_User_Collide_Meshes, distance only (an unrotated box: the
// element transform is a translation, LocalPosition = WorldPosition - centre).
fn box_distance(local: vec3<f32>, extent: vec3<f32>) -> f32 {
    let half = 0.5 * extent;
    let delta = abs(local) - half;
    var closest = 2;
    if delta.x > delta.y && delta.x > delta.z { closest = 0; } else if delta.y > delta.z { closest = 1; }
    let outside = length(max(delta, vec3<f32>(0.0)));
    return outside + min(delta[closest], 0.0);
}

@compute @workgroup_size(8, 8)
fn stamp_bodies(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.z >= collisions.counts.x || !footprint_lane(id) { return; }
    let at = collisions.origins[id.z].xy + vec2<i32>(id.xy);
    let i = cell_index(at);
    if i < 0 { return; }
    let state = water[i];
    if state.y <= 1e-5 && depth_at(at + vec2<i32>(1, 0)) <= 1e-5 && depth_at(at - vec2<i32>(1, 0)) <= 1e-5
        && depth_at(at + vec2<i32>(0, 1)) <= 1e-5 && depth_at(at - vec2<i32>(0, 1)) <= 1e-5 {
        return;
    }
    let dx = collisions.stepping.x;
    let position = vec3<f32>((vec2<f32>(at) + 0.5) * dx, state.x);
    // GetClosestElement: the first element with the smallest distance wins.
    var distance = 9999999.0;
    var velocity = vec2<f32>(0.0);
    for (var b = 0u; b < collisions.counts.x; b++) {
        let body = collisions.bodies[b];
        let d = box_distance(position - body.centre.xyz, body.extent.xyz);
        if d < distance {
            distance = d;
            velocity = body.velocity.xy;
        }
    }
    // 1 - smoothstep(-0, 0, Dist) with Falloff 0: inside (Dist <= 0) only.
    if distance > 0.0 { return; }
    let base = geometry[i];
    geometry[i] = vec4<f32>(base.x, 1.0, velocity * collisions.stepping.y);
}
