// SW_River_Secondary_Emitter ParticleSpawnUpdate stage, one thread per
// particle: the cached HLSL's module order with the saved asset's values
// (secondary.rs). Copyright Epic Games, Inc. All Rights Reserved.
// Positions are river centimetres, Z up (Unreal's world axes and units).
struct RiverSprayView {
    position: vec3<f32>,
    right: vec3<f32>,
    up: vec3<f32>,
    forward: vec3<f32>,
    m00: f32,    // ViewToClip[0][0] = 1 / tan(horizontal half FOV)
    m11: f32,    // ViewToClip[1][1]
    aspect: f32, // ViewSizeAndInverseSize.x * .w
};
struct RiverSprayControls {
    grid: vec4<f32>,      // WorldToUnit: grid origin XY (cm), 1 / world extent XY
    sim: vec4<f32>,       // User.SimRes xy, Emitter.dx, Emitter.CellSize.x
    water: vec4<f32>,     // LocalToWorldTranslation.z, RestrictEmissionDepthRange xy, unused
    frame: vec4<f32>,     // Engine.DeltaTime, Emitter.Age, unused, unused
    counters: vec4<u32>,  // System.TickCount, Emitter.InstanceSeed (i32 bits), spawn burst now, particle count
    view: array<vec4<f32>, 4>,     // position; right, M00; up, M11; forward, aspect
    previous: array<vec4<f32>, 4>, // the same for the previous frame
};
@group(0) @binding(0) var<uniform> spray: RiverSprayControls;
@group(0) @binding(1) var water: texture_2d_array<f32>; // height, depth, vx, vy (R32f layers)
@group(0) @binding(2) var normal_grid: texture_2d<f32>;
@group(0) @binding(3) var grid_sampler: sampler;        // Niagara reader SF_Bilinear, AM_Clamp
@group(0) @binding(4) var<storage, read_write> particles: array<RiverSprayParticle>;
@group(0) @binding(5) var<storage, read_write> probe_slots: array<vec4<f32>>;

var<private> river_spray_thread: i32;
var<private> river_spray_det: i32;
var<private> river_spray_nondet: i32;

// rand(x, Seed1, Seed2, Seed3): RandomCounterDeterministic increments per call.
fn river_spray_rand_det(x: f32, s1: i32, s2: i32, s3: i32) -> f32 {
    river_spray_det += 1;
    return river_spray_rand3(s1, s2, s3, river_spray_det).x * x;
}
fn river_spray_rand_det2(x: vec2<f32>, s1: i32, s2: i32, s3: i32) -> vec2<f32> {
    river_spray_det += 1;
    return river_spray_rand3(s1, s2, s3, river_spray_det).xy * x;
}
// GPU rand(x): rand4(GLinearThreadId, EmitterTickCounter, GLinearThreadId, counter).
fn river_spray_rand(x: f32) -> f32 {
    river_spray_nondet -= 1;
    let tick = bitcast<i32>(spray.counters.x);
    return river_spray_rand4(river_spray_thread, tick, river_spray_thread, river_spray_nondet).x * x;
}

fn river_spray_modulo(x: f32, y: f32) -> f32 {
    return x - y * trunc(x / y); // ModuloPrecise
}

fn river_spray_view(v: array<vec4<f32>, 4>) -> RiverSprayView {
    return RiverSprayView(v[0].xyz, v[1].xyz, v[2].xyz, v[3].xyz, v[1].w, v[2].w, v[3].w);
}
// TranslatedWorldToClip of a camera-relative point (reversed-Z infinite
// perspective: x*M00, y*M11, near, view depth). Only x, y and w are read.
fn river_spray_clip(view: RiverSprayView, p: vec3<f32>) -> vec4<f32> {
    return vec4<f32>(dot(view.right, p) * view.m00, dot(view.up, p) * view.m11, 0.0, dot(view.forward, p));
}
fn river_spray_to_view(view: RiverSprayView, p: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(view.right, p), dot(view.up, p), dot(view.forward, p));
}
fn river_spray_from_view(view: RiverSprayView, p: vec3<f32>) -> vec3<f32> {
    return view.right * p.x + view.up * p.y + view.forward * p.z;
}

// Grid2D_SW_EmissionCriteria; a zero-width range takes D3D's saturate(NaN)=0
// / saturate(+inf)=1 result explicitly (foam_field::criterion).
fn river_spray_criterion(value: f32, range: vec2<f32>, multiplier: f32) -> f32 {
    if range.y == range.x {
        return select(0.0, 1.0, value - range.x > 0.0) * multiplier;
    }
    return smoothstep(range.x, range.y, value) * multiplier;
}

struct RiverSpraySample {
    curl: f32,
    curvature: f32,
    velocity: vec2<f32>,
    depth_difference: f32,
    water_depth: f32,
    water_height: f32,
    height_difference: f32,
    normal: vec3<f32>,
};

fn river_spray_velocity(unit: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(textureSampleLevel(water, grid_sampler, unit, 2, 0.0).x, textureSampleLevel(water, grid_sampler, unit, 3, 0.0).x);
}

// Sample_Water: WorldToUnit, Grid2D_SW_SampleSecondaryEmission (SampleFromGrid,
// the native X-only ValidRange), then WaterHeight + LocalToWorldTranslation.z.
fn river_spray_sample(position: vec3<f32>, dt: f32) -> RiverSpraySample {
    var out = RiverSpraySample(0.0, 0.0, vec2<f32>(0.0), 0.0, 0.0, 0.0, 0.0, vec3<f32>(0.0));
    let unit = (position.xy - spray.grid.xy) * spray.grid.zw + 0.5;
    let udx = vec2<f32>(1.0 / spray.sim.x, 1.0 / spray.sim.y);
    let valid = 2.0 * udx.x;
    if river_spray_finite3(position) && unit.x > valid && unit.x < 1.0 - valid && unit.y > valid && unit.y < 1.0 - valid {
        out.water_depth = textureSampleLevel(water, grid_sampler, unit, 1, 0.0).x;
        out.water_height = textureSampleLevel(water, grid_sampler, unit, 0, 0.0).x;
        if out.water_depth > 1e-5 {
            let vx_1 = river_spray_velocity(unit + vec2<f32>(0.0, 1.0) * udx).x;
            let vx_m1 = river_spray_velocity(unit + vec2<f32>(0.0, -1.0) * udx).x;
            let vy_1 = river_spray_velocity(unit + vec2<f32>(1.0, 0.0) * udx).y;
            let vy_m1 = river_spray_velocity(unit + vec2<f32>(-1.0, 0.0) * udx).y;
            out.curl = abs(((vx_1 - vx_m1) - (vy_1 - vy_m1)) / (2.0 * spray.sim.z));
            out.velocity = vec2<f32>(0.5 * (vx_1 + vx_m1), 0.5 * (vy_1 + vy_m1));
            let n = textureSampleLevel(normal_grid, grid_sampler, unit, 0.0).xyz;
            let n_right = textureSampleLevel(normal_grid, grid_sampler, unit + vec2<f32>(1.0, 0.0) * udx, 0.0).xyz;
            let n_up = textureSampleLevel(normal_grid, grid_sampler, unit + vec2<f32>(0.0, 1.0) * udx, 0.0).xyz;
            out.normal = n;
            out.curvature = abs(((1.0 - dot(n, n_right)) + (1.0 - dot(n, n_up))) * 0.5);
            let next = unit + out.velocity * dt * udx / spray.sim.z;
            out.depth_difference = textureSampleLevel(water, grid_sampler, next, 1, 0.0).x - out.water_depth;
            out.height_difference = textureSampleLevel(water, grid_sampler, next, 0, 0.0).x - out.water_height;
        }
    }
    out.water_height += spray.water.x;
    return out;
}

// Emission_Criterias' EmitAmount (sink-box culling not ported).
fn river_spray_emit_amount(s: RiverSpraySample) -> f32 {
    let speed = length(s.velocity);
    let c = RIVER_SPRAY_CRITERIA;
    let strongest = max(max(max(river_spray_criterion(s.curl, c[0].xy, c[0].z), river_spray_criterion(speed, c[1].xy, c[1].z)),
        river_spray_criterion(s.curvature, c[2].xy, c[2].z)), river_spray_criterion(s.depth_difference, c[3].xy, c[3].z));
    let amount = RIVER_SPRAY_EMISSION.x * strongest * river_spray_criterion(s.water_depth, spray.water.yz, 1.0)
        * river_spray_criterion(speed, RIVER_SPRAY_SPEED_GATE, 1.0);
    return clamp(amount, 0.0, RIVER_SPRAY_EMISSION.y);
}

// Emission_Criterias' custom HLSL (RandomVelocityMagnitude 0).
fn river_spray_spawn_velocity(s: RiverSpraySample) -> vec3<f32> {
    var v = vec3<f32>(s.velocity, 0.0);
    v = v - dot(v, s.normal) * s.normal;
    v *= RIVER_SPRAY_SPAWN_VELOCITY.y;
    v += RIVER_SPRAY_SPAWN_VELOCITY.x * s.normal;
    return v * RIVER_SPRAY_SPAWN_VELOCITY.z;
}

// SimulateMapSpawn: InitializeParticle (Lifetime 0.5, Random Uniform sprite
// size, Random rotation, owner position) and SetVariables MaterialRandom.
fn river_spray_spawn(index: u32) -> RiverSprayParticle {
    let initialize_random = river_spray_rand(1.0);
    let size = RIVER_SPRAY_SIZE_RANGE.x + river_spray_rand(RIVER_SPRAY_SIZE_RANGE.y - RIVER_SPRAY_SIZE_RANGE.x);
    let rotation = RIVER_SPRAY_ROTATION_RANGE.x + river_spray_rand(RIVER_SPRAY_ROTATION_RANGE.y - RIVER_SPRAY_ROTATION_RANGE.x);
    let material_random = river_spray_rand(1.0);
    _ = initialize_random;
    let owner = vec3<f32>(spray.grid.xy, spray.water.x);
    return RiverSprayParticle(
        vec4<f32>(owner, 0.0),
        vec4<f32>(0.0, 0.0, 0.0, RIVER_SPRAY_LIFETIME),
        vec4<f32>(size, size, size, size),
        vec4<f32>(rotation, 0.0, material_random, 0.0),
        vec4<i32>(0, 0, 0, i32(index)),
    );
}

@compute @workgroup_size(64)
fn update(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = id.x;
    if index >= spray.counters.w {
        return;
    }
    river_spray_thread = i32(index);
    river_spray_det = 0;
    river_spray_nondet = -1;
    let dt = spray.frame.x;
    var p: RiverSprayParticle;
    if spray.counters.z != 0u {
        p = river_spray_spawn(index); // TransferAttributes: spawn feeds update
    } else {
        p = particles[index];
    }
    let unique = p.state.w;
    let seed = bitcast<i32>(spray.counters.y);
    let tick = bitcast<i32>(spray.counters.x);
    let previous_age = p.position_age.w; // Particles.Previous.Age

    // ParticleState (LoopParticlesLifetime, never killed).
    var age = p.position_age.w;
    var lifetime = p.velocity_lifetime.w;
    let first_frame = age <= dt;
    let lifetime_safe = max(lifetime, 1e-5);
    age = river_spray_modulo(age + dt, lifetime_safe);
    var normalized_age = age / lifetime_safe;

    // RandomizeStartAgeOverLifetime (first frame only).
    let start = river_spray_rand_det(1.0, unique, RIVER_SPRAY_SEED_FLOAT, seed);
    if first_frame {
        let a = river_spray_modulo(age + lifetime * start, lifetime);
        age = a;
        normalized_age = a / lifetime;
    }

    // RandomRangeVector2D -> ViewRecycler Near / Far Distance.
    let near_far = river_spray_rand_det2(RIVER_SPRAY_NEAR_FAR, unique, RIVER_SPRAY_SEED_VECTOR2, seed);

    // ---- ViewRecycler ----
    var position = p.position_age.xyz;
    var velocity = p.velocity_lifetime.xyz;
    var sprite_size = p.size.xy;
    let initial_size = p.size.zw;
    var flags = p.state.z;
    var tag = p.state.x;
    var tag_cache = p.state.y;
    let backward = -velocity;
    let backward_squared = dot(backward, backward);
    let below_threshold = backward_squared < 1e-5 * 1e-5;
    var trace_direction = vec3<f32>(0.0, 0.0, 1.0);
    if !below_threshold {
        trace_direction = backward * inverseSqrt(backward_squared);
    }
    let do_trace = !below_threshold;
    let emitter_first_frame = spray.frame.y - dt <= dt;
    let stored_do_recycle = (flags & RIVER_SPRAY_DO_RECYCLE) != 0;
    flags = flags & ~RIVER_SPRAY_DO_RECYCLE;
    let recycle_from_previous = stored_do_recycle || previous_age == 0.0 || previous_age > age;

    let view = river_spray_view(spray.view);
    // Per-particle caches equal the previous frame's view except on a
    // particle's first frame, which uses the current one.
    var previous = river_spray_view(spray.previous);
    if first_frame {
        previous = view;
    }
    let travel_camera_forward = dot(view.position - previous.position, view.forward);
    let radius = max(abs(sprite_size.x), abs(sprite_size.y)) * 0.5;
    let near = max(near_far.x, 0.0);
    let far = near_far.y;
    let reference = RIVER_SPRAY_REFERENCE_FOV;
    let width_coverage = view.m00 * reference;
    let height_coverage = view.m00 * reference;
    let width_coverage_previous = previous.m00 * reference;
    let height_coverage_previous = previous.m00 * reference;

    let current_clip = river_spray_clip(view, position - view.position);
    let pad = radius * reference * 2.0;
    let outside_current = abs(current_clip.x) > (current_clip.w + pad) * width_coverage
        || abs(current_clip.y) > (current_clip.w + pad * view.aspect) * height_coverage
        || current_clip.w > far + 2.0 * radius
        || current_clip.w < near - 2.0 * radius;
    let previous_clip = river_spray_clip(previous, position - previous.position);
    let outside_previous = abs(previous_clip.x) > (previous_clip.w + pad) * width_coverage_previous
        || abs(previous_clip.y) > (previous_clip.w + pad * view.aspect) * height_coverage_previous
        || previous_clip.w > far + 2.0 * radius
        || previous_clip.w < near - 2.0 * radius;

    let travel_step = (abs(travel_camera_forward) - near) / (far - near);
    var travel = fract(p.misc.w + sign(travel_camera_forward) * travel_step * river_spray_rand(3234.0));
    if !(abs(travel) <= 1.0) {
        travel = 0.0; // far == near guard (not upstream)
    }

    // Custom HLSL: volume-distributed frustum sample, rejection against the
    // previous frustum, then a backward trace to the frustum border.
    let offset = view.position - previous.position;
    let width = 2.0 * (far / view.m00) * width_coverage;
    let height = 2.0 * (far / view.m11) * height_coverage;
    let comp_x = radius * reference;
    let comp_y = comp_x * view.aspect;
    let base = width * height;
    let volume = base * far * (1.0 / 3.0);
    let small_volume = (near * near * near * base) / (3.0 * far * far);
    let cube_root_three = 1.44224957031;
    let far_two_thirds = pow(far, 2.0 / 3.0);
    let inv_base_cube_root = 1.0 / pow(base, 1.0 / 3.0);
    var relative = vec3<f32>(0.0);
    var random_outside_previous = false;
    for (var i = 0; i < RIVER_SPRAY_REJECTION_SAMPLES; i++) {
        let random2 = river_spray_rand_det(1.0, unique, RIVER_SPRAY_SEED_RECYCLER, 11000 + i);
        let random = fract(random2 - travel);
        let z = pow((volume - small_volume) * random + small_volume, 1.0 / 3.0);
        let spawn_depth = cube_root_three * far_two_thirds * z * inv_base_cube_root;
        var rand_x = river_spray_rand_det(2.0, unique, tick, 0 + i) - 1.0;
        var rand_y = river_spray_rand_det(2.0, unique, tick, 1 + i) - 1.0;
        rand_x *= width_coverage;
        rand_y *= height_coverage;
        rand_x *= comp_x + spawn_depth;
        rand_y *= comp_y + spawn_depth;
        // ClipToTranslatedWorld of (x, y, 0, depth): the w = 0 direction.
        relative = river_spray_from_view(view, vec3<f32>(rand_x / view.m00, rand_y / view.m11, spawn_depth));
        let projected = river_spray_clip(previous, relative + offset);
        if abs(projected.x / (projected.w + comp_x)) > width_coverage_previous
            || abs(projected.y / (projected.w + comp_y)) > height_coverage_previous
            || projected.w < near || projected.w > far {
            random_outside_previous = true;
            break;
        }
    }
    let corner_near = vec3<f32>(near * width_coverage / view.m00, -near * height_coverage / view.m11, near);
    let corner_far = vec3<f32>(far * width_coverage / view.m00, far * height_coverage / view.m11, far);
    var view_position = river_spray_to_view(view, relative);
    let trace = river_spray_to_view(view, trace_direction);
    let near_to_far = normalize(corner_far - corner_near);
    let left_down = vec3<f32>(-corner_near.x, -corner_near.y, corner_near.z);
    var plane_normal: array<vec3<f32>, 5>;
    var plane_position: array<vec3<f32>, 5>;
    if trace.z < 0.0 {
        plane_normal[0] = vec3<f32>(0.0, 0.0, -1.0);
        plane_position[0] = corner_near;
    } else {
        plane_normal[0] = vec3<f32>(0.0, 0.0, 1.0);
        plane_position[0] = corner_far;
    }
    plane_normal[1] = cross(vec3<f32>(0.0, 1.0, 0.0), near_to_far);
    plane_position[1] = corner_far;
    plane_normal[2] = plane_normal[1] * vec3<f32>(-1.0, 1.0, 1.0);
    plane_position[2] = left_down;
    plane_normal[3] = -cross(vec3<f32>(1.0, 0.0, 0.0), near_to_far);
    plane_position[3] = corner_far;
    plane_normal[4] = plane_normal[3] * vec3<f32>(1.0, -1.0, 1.0);
    plane_position[4] = left_down;
    var t = 99999999.0;
    var border_normal = vec3<f32>(0.0, 0.0, 1.0);
    for (var i = 0; i < 5; i++) {
        let n_dot_dp = dot(plane_normal[i], plane_position[i] - view_position);
        if n_dot_dp <= 0.0 {
            t = 0.0;
        } else {
            let t_current = n_dot_dp / dot(plane_normal[i], trace);
            if t_current < t && t_current >= 0.000001 {
                t = t_current;
                border_normal = plane_normal[i];
            }
        }
    }
    view_position += trace * t;
    view_position += border_normal * radius;
    let border = river_spray_from_view(view, view_position);

    var do_rejection = false;
    var hide = false;
    var do_recycle = false;
    var border_spawn = false;
    var rejection_valid = false;
    if outside_current || (flags & RIVER_SPRAY_DO_REJECTION) != 0 || recycle_from_previous {
        hide = true;
        if recycle_from_previous {
            do_recycle = true;
            hide = false;
            border_spawn = true;
        }
        if random_outside_previous {
            do_recycle = true;
            hide = false;
            rejection_valid = true;
        }
        if outside_current && outside_previous {
            do_recycle = true;
            hide = false;
            border_spawn = true;
        }
        if !do_recycle {
            do_rejection = true;
            hide = true;
        }
    }
    if border_spawn && !rejection_valid && do_trace && !emitter_first_frame {
        relative = border;
    }
    let spawn_position = view.position + relative;
    flags = (flags & ~RIVER_SPRAY_DO_REJECTION) | select(0, RIVER_SPRAY_DO_REJECTION, do_rejection);

    // Visibility tag (renderer visibility 0; hidden -999, cached for unhide).
    if tag == RIVER_SPRAY_HIDDEN {
        tag = tag_cache;
    }
    var visible = true;
    hide = visible && hide;
    if hide {
        visible = false;
        tag_cache = tag;
        tag = RIVER_SPRAY_HIDDEN;
    }
    if do_recycle {
        velocity = vec3<f32>(0.0); // Velocity Mode Set, Recycle Velocity 0
    }
    if do_recycle || hide {
        position = spawn_position;
    }
    if do_recycle && !first_frame {
        lifetime = RIVER_SPRAY_LIFETIME; // Lifetime Mode Initial Value
        age = dt * 3.0;
        normalized_age = 0.0;
    }
    if do_recycle {
        sprite_size = initial_size; // Sprite Size Mode Initial Value
    }
    let recycle_valid = do_recycle || first_frame;

    // GravityForce (mass 1).
    var force = RIVER_SPRAY_GRAVITY;
    // RandomRangeFloat005 -> SpriteRotationRate001.
    let rotation_rate = RIVER_SPRAY_ROTATION_RATE.x
        + river_spray_rand_det(RIVER_SPRAY_ROTATION_RATE.y - RIVER_SPRAY_ROTATION_RATE.x, unique, RIVER_SPRAY_SEED_FLOAT, seed);
    let rotation = p.misc.x + dt * rotation_rate;

    // Sample_Water and Emission_Criterias.
    let s = river_spray_sample(position, dt);
    let amount = river_spray_emit_amount(s);
    let respawned = recycle_valid || first_frame;
    let recycle = s.water_depth < RIVER_SPRAY_EMISSION.w || (respawned && amount < RIVER_SPRAY_EMISSION.z);
    if respawned {
        position = vec3<f32>(position.xy, s.water_height);
        velocity = river_spray_spawn_velocity(s);
    }
    if visible && recycle {
        tag_cache = tag;
        tag = RIVER_SPRAY_HIDDEN;
    }
    flags = (flags & ~RIVER_SPRAY_DO_RECYCLE) | select(0, RIVER_SPRAY_DO_RECYCLE, recycle);

    // WhiteWaterVelocityAndForce: under the surface ride the projected flow
    // with bubble buoyancy instead of the accumulated force.
    if position.z < s.water_height {
        let surface = vec3<f32>(s.velocity, 0.0);
        velocity = surface - dot(surface, s.normal) * s.normal;
        force = RIVER_SPRAY_BUOYANCY;
    }
    // SolveForcesAndVelocity (mass 1, drag 0, no limits).
    velocity += force * dt;
    position += velocity * dt;
    // ScaleSpriteSize002 (ScaleColor001's alpha is evaluated by the draw).
    sprite_size = initial_size * river_spray_size_scale(normalized_age);

    if !river_spray_finite3(position) || !river_spray_finite3(velocity) {
        // Degenerate frustum samples (near > far) produce NaN upstream too and
        // never pass emission; keep them hidden and retrying.
        position = view.position;
        velocity = vec3<f32>(0.0);
        flags = flags | RIVER_SPRAY_DO_RECYCLE;
        if tag != RIVER_SPRAY_HIDDEN {
            tag_cache = tag;
            tag = RIVER_SPRAY_HIDDEN;
        }
    }
    particles[index] = RiverSprayParticle(
        vec4<f32>(position, age),
        vec4<f32>(velocity, lifetime),
        vec4<f32>(sprite_size, initial_size),
        vec4<f32>(rotation, normalized_age, p.misc.z, travel),
        vec4<i32>(tag, tag_cache, flags, unique),
    );
}

// Diagnostic probe for parity tests: slot 3i holds (world xyz, dt) and
// receives (depth, height, curvature, EmitAmount); 3i+1 (spawn velocity,
// curl); 3i+2 (velocity xy, depth difference, recycle-on-spawn).
@compute @workgroup_size(64)
fn probe(@builtin(global_invocation_id) id: vec3<u32>) {
    let count = arrayLength(&probe_slots) / 3u;
    if id.x >= count {
        return;
    }
    let input = probe_slots[id.x * 3u];
    let s = river_spray_sample(input.xyz, input.w);
    let amount = river_spray_emit_amount(s);
    let recycle = s.water_depth < RIVER_SPRAY_EMISSION.w || amount < RIVER_SPRAY_EMISSION.z;
    probe_slots[id.x * 3u] = vec4<f32>(s.water_depth, s.water_height, s.curvature, amount);
    probe_slots[id.x * 3u + 1u] = vec4<f32>(river_spray_spawn_velocity(s), s.curl);
    probe_slots[id.x * 3u + 2u] = vec4<f32>(s.velocity, s.depth_difference, select(0.0, 1.0, recycle));
}
