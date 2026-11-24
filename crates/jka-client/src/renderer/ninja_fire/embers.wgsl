// Fire_Smoke_Explosive GPU sprite simulation: one pool per source (turret
// embers: kinds 0 and 1; Burning Man sparks: kind 2). Tick structure as the
// Coast whitewater port of a Niagara GPU emitter: em_begin (counts, spawn
// budget, indirect arguments), em_update_main (surviving particles appended
// to the other buffer), em_spawn_main (SimulateMapSpawn then the same tick's
// update), em_end (alive count, draw arguments). Prepended: ember_common.wgsl.

struct EmberParams {
    // Previous anchor (pool frame, cm), tick dt.
    previous: vec4<f32>,
    // Anchor (pool frame, cm), Engine.Time at the end of the tick.
    anchor: vec4<f32>,
    // AdvectionVelocity (cm/s).
    air: vec4<f32>,
    // CurlNoiseForce RandomizationOffset; Noise Frequency.
    curl_offset: vec4<f32>,
    // CurlNoiseForce Pan Noise Field.
    curl_pan: vec4<f32>,
    // Spawn rate per second of slots 0 and 1 (scaled and fuelled by the host); their kinds.
    rates: vec4<f32>,
    // Capacity, source seed, brush count, tick counter.
    misc: vec4<u32>,
}

@group(0) @binding(0) var<uniform> em: EmberParams;
@group(0) @binding(1) var<storage, read> em_src: array<EmberParticle>;
@group(0) @binding(2) var<storage, read_write> em_dst: array<EmberParticle>;
// 0 alive, 1 append, 2 spawn total, 3 unique-ID base, 4/5 spawn remainders
// (f32 bits), 6 slot-0 spawns.
@group(0) @binding(3) var<storage, read_write> em_counters: array<atomic<u32>, 8>;
// brush_collision.rs pack_brushes: per brush two headers (min.xyz + first
// plane index bits, max.xyz + plane count bits), then planes (normal, distance).
@group(0) @binding(4) var<storage, read> em_brushes: array<vec4<f32>>;
// Emitter vector field: TilingCurl32 (embers) or TilingCurl64 (sparks), RGBA16F.
@group(0) @binding(5) var em_curl: texture_3d<f32>;
@group(0) @binding(6) var em_wrap: sampler;
// Bound only for em_begin/em_end.
@group(1) @binding(0) var<storage, read_write> em_args: array<u32, 10>;

const EM_SPHERE_RADIUS: f32 = 25.0;
const EM_RESTITUTION: f32 = 0.2;
// Valid Collision Plane Normal For Rest (0,0,1) at 50% alignment.
const EM_REST_ALIGNMENT: f32 = 0.5;
// DinurdoJK: normal speed (cm/s) below which a floor contact rests (UE uses a
// penetration rest counter).
const EM_REST_SPEED: f32 = 100.0;

// NiagaraEmitterInstanceShader.usf rand3.x structure with a host emitter seed.
fn em_random(a: u32, b: u32, c: u32, counter: u32) -> f32 {
    var x = a * 1664525u + 1013904223u;
    var y = b * 1664525u + 1013904223u;
    var z = (counter | (c << 16u)) * 1664525u + 1013904223u;
    for (var i = 0; i < 2; i++) {
        x = x + y * z;
        y = y + z * x;
        z = z + x * y;
    }
    return f32(x >> 8u) / 16777216.0;
}

fn em_curl_force(sample_position: vec3<f32>, time: f32, strength: f32) -> vec3<f32> {
    let lookup = ((em.curl_offset.xyz + sample_position) * (em.curl_offset.w / 1000.0)
        - em.curl_pan.xyz * time) * 125.0;
    let uvw = (lookup + vec3<f32>(500.0, 500.0, 0.0)) / 1000.0;
    return textureSampleLevel(em_curl, em_wrap, uvw, 0.0).xyz * strength;
}

// Deepest sphere contact against the convex brushes: normal and depth (w <= 0: none).
fn em_collide(position: vec3<f32>, radius: f32) -> vec4<f32> {
    var best = vec4<f32>(0.0, 0.0, 1.0, 0.0);
    for (var b = 0u; b < em.misc.z; b++) {
        let low = em_brushes[2u * b];
        let high = em_brushes[2u * b + 1u];
        if (any(position + radius < low.xyz) || any(position - radius > high.xyz)) { continue; }
        let first = bitcast<u32>(low.w);
        let count = bitcast<u32>(high.w);
        var nearest = -1e30;
        var normal = vec3<f32>(0.0, 0.0, 1.0);
        for (var i = 0u; i < count; i++) {
            let plane = em_brushes[first + i];
            let along = dot(plane.xyz, position) - plane.w;
            if (along > nearest) { nearest = along; normal = plane.xyz; }
        }
        let depth = radius - nearest;
        if (depth > best.w) { best = vec4<f32>(normal, depth); }
    }
    return best;
}

// Turret update stack: ParticleState, GravityForce, Drag, Collision,
// CurlNoiseForce, SolveForcesAndVelocity (drag ignores mass; no clamps).
fn em_advance_ember(particle: ptr<function, EmberParticle>, recipe: EmberRecipe, dt: f32, time: f32) {
    let size = (*particle).size_mass_drag_kind.x;
    let mass = max((*particle).size_mass_drag_kind.y, 1e-6);
    var position = (*particle).position_age.xyz;
    var velocity = (*particle).velocity_lifetime.xyz;
    var force = vec3<f32>(0.0, 0.0, -recipe.gravity * mass);
    // CalculateRadiusFromSpriteSize: |SpriteSize| / 2 with the current speed stretch.
    let stretch = select(1.0, ember_stretch(length(velocity), recipe), recipe.aligned > 0.5);
    let radius = 0.5 * size * sqrt(1.0 + stretch * stretch);
    // DinurdoJK: GPU Distance Fields -> solid BSP brushes near the source.
    let contact = em_collide(position + velocity * dt, radius);
    if (contact.w > 0.0) {
        // Push out by the predicted penetration; the solver below moves on.
        position = position + contact.xyz * contact.w;
        let normal_speed = dot(velocity, contact.xyz);
        if (normal_speed < 0.0) { velocity -= contact.xyz * (normal_speed * (1.0 + EM_RESTITUTION)); }
        if (contact.z > EM_REST_ALIGNMENT && -normal_speed < EM_REST_SPEED) { velocity = vec3<f32>(0.0); }
        // A contact replaces the accumulated force.
        force = vec3<f32>(0.0);
    }
    force += em_curl_force(position, time, recipe.curl_strength);
    let solved = velocity + force * (dt / mass);
    velocity = em.air.xyz + (solved - em.air.xyz) / (1.0 + (*particle).size_mass_drag_kind.z * dt);
    (*particle).position_age = vec4<f32>(position + velocity * dt, (*particle).position_age.w);
    (*particle).velocity_lifetime = vec4<f32>(velocity, (*particle).velocity_lifetime.w);
}

// Burning Man sparks (unchanged from the previous CPU port): AddVelocity
// force Z*1000, CurlNoiseForce001 (TilingCurl64 x 1200), point attraction
// towards (0,0,250) within 500 cm, SolveForcesAndVelocity with mass =
// width/10 and drag relative to AdvectionVelocity.
fn em_advance_spark(particle: ptr<function, EmberParticle>, recipe: EmberRecipe, dt: f32, time: f32) {
    let local = (*particle).position_age.xyz;
    let velocity = (*particle).velocity_lifetime.xyz;
    var force = vec3<f32>(0.0, 0.0, 1000.0) + em_curl_force(local, time, recipe.curl_strength);
    let to_point = vec3<f32>(0.0, 0.0, 250.0) - local - (force * dt + velocity) * dt;
    let reach = length(to_point);
    if (reach <= 500.0) {
        let direction = select(vec3<f32>(0.0, 0.0, 1.0), to_point / reach, reach > 0.00001);
        force += direction * (50.0 * sqrt(reach));
    }
    let solved = velocity + force * (dt / max((*particle).size_mass_drag_kind.y, 1e-6));
    let drag = (*particle).size_mass_drag_kind.z;
    let next = em.air.xyz + (solved - em.air.xyz) / (1.0 + drag * dt);
    (*particle).position_age = vec4<f32>(local + next * dt, (*particle).position_age.w);
    (*particle).velocity_lifetime = vec4<f32>(next, (*particle).velocity_lifetime.w);
}

fn em_advance(p: ptr<function, EmberParticle>, dt: f32, time: f32) -> bool {
    var particle = *p;
    let age = particle.position_age.w + dt;
    particle.position_age.w = age;
    if (age >= particle.velocity_lifetime.w) { return false; }
    if (dt > 0.0) {
        let kind = u32(particle.size_mass_drag_kind.w);
        let recipe = ember_recipe(kind);
        if (kind == 2u) {
            em_advance_spark(&particle, recipe, dt, time);
        } else {
            em_advance_ember(&particle, recipe, dt, time);
        }
    }
    *p = particle;
    return true;
}

fn em_append(particle: EmberParticle) {
    let i = atomicAdd(&em_counters[1], 1u);
    if (i < em.misc.x) { em_dst[i] = particle; }
}

@compute @workgroup_size(1)
fn em_begin() {
    let capacity = em.misc.x;
    let source = min(atomicLoad(&em_counters[0]), capacity);
    atomicStore(&em_counters[1], 0u);
    // SpawnRate: rate * dt with the fractional remainder carried.
    let dt = em.previous.w;
    let want0 = max(em.rates.x * dt + bitcast<f32>(atomicLoad(&em_counters[4])), 0.0);
    let want1 = max(em.rates.y * dt + bitcast<f32>(atomicLoad(&em_counters[5])), 0.0);
    let whole0 = floor(want0);
    let whole1 = floor(want1);
    atomicStore(&em_counters[4], bitcast<u32>(select(0.0, want0 - whole0, em.rates.x > 0.0)));
    atomicStore(&em_counters[5], bitcast<u32>(select(0.0, want1 - whole1, em.rates.y > 0.0)));
    let room = capacity - source;
    let spawn0 = min(u32(whole0), room);
    let spawn1 = min(u32(whole1), room - spawn0);
    atomicStore(&em_counters[6], spawn0);
    atomicStore(&em_counters[2], spawn0 + spawn1);
    em_args[0] = (source + 63u) / 64u; em_args[1] = 1u; em_args[2] = 1u;
    em_args[3] = (spawn0 + spawn1 + 63u) / 64u; em_args[4] = 1u; em_args[5] = 1u;
}

@compute @workgroup_size(64)
fn em_update_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= min(atomicLoad(&em_counters[0]), em.misc.x)) { return; }
    var particle = em_src[id.x];
    if (em_advance(&particle, em.previous.w, em.anchor.w)) { em_append(particle); }
}

// Turret particle appearance with launch directions tuned for flames.
fn em_spawn_ember(kind: u32, uid: u32, origin: vec3<f32>) -> EmberParticle {
    let recipe = ember_recipe(kind);
    let seed = em.misc.y;
    let sizes = recipe.size;
    let size = mix(sizes.x, sizes.y, em_random(uid, seed, kind + 1u, 1u));
    // InitializeParticle colour: Random Range, Link RGBA or Link RGB / Link A.
    let rgb = em_random(uid, seed, kind + 1u, 2u);
    let alpha = select(rgb, em_random(uid, seed, kind + 1u, 3u), recipe.separate_alpha > 0.5);
    let color = vec4<f32>(mix(recipe.color_low.rgb, recipe.color_high.rgb, rgb),
        mix(recipe.color_low.a, recipe.color_high.a, alpha));
    // SphereLocation r 25, NonUniformScale (1,1,0): uniform volume sample on the emitter plane.
    let around = 6.2831853 * em_random(uid, seed, kind + 1u, 4u);
    let cos_theta = 2.0 * em_random(uid, seed, kind + 1u, 5u) - 1.0;
    let radius = EM_SPHERE_RADIUS * pow(em_random(uid, seed, kind + 1u, 6u), 1.0 / 3.0)
        * sqrt(1.0 - cos_theta * cos_theta);
    // Sample around the full flame, with a bias toward upward launches.
    let spin = 6.2831853 * em_random(uid, seed, kind + 1u, 7u);
    let vertical = mix(-0.35, 0.95, em_random(uid, seed, kind + 1u, 11u));
    let horizontal = sqrt(1.0 - vertical * vertical);
    let direction = vec3<f32>(horizontal * cos(spin), horizontal * sin(spin), vertical);
    let speed = mix(recipe.launch_speed.x, recipe.launch_speed.y, em_random(uid, seed, kind + 1u, 8u));
    var particle: EmberParticle;
    particle.position_age = vec4<f32>(origin + vec3<f32>(cos(around) * radius, sin(around) * radius, 0.0), 0.0);
    particle.velocity_lifetime = vec4<f32>(direction * speed,
        mix(recipe.lifetime.x, recipe.lifetime.y, em_random(uid, seed, kind + 1u, 9u)));
    particle.color = color;
    // Particles.Mass = SpriteSize.x / 10.
    particle.size_mass_drag_kind = vec4<f32>(size, size / 10.0,
        mix(recipe.drag.x, recipe.drag.y, em_random(uid, seed, kind + 1u, 10u)), f32(kind));
    particle.anchor = vec4<f32>(0.0);
    return particle;
}

// Burning Man sparks: lifetime 0.5-1.5 s, width 0.5-2, SphereLocation r 50
// flattened by NonUniformScale (1,1,0) at -160 cm, drag 20-60; each keeps its
// spawn anchor.
fn em_spawn_spark(uid: u32, origin: vec3<f32>) -> EmberParticle {
    let recipe = ember_recipe(2u);
    let seed = em.misc.y;
    let size = mix(recipe.size.x, recipe.size.y, em_random(uid, seed, 3u, 2u));
    let around = 6.2831853 * em_random(uid, seed, 3u, 3u);
    let cos_theta = 2.0 * em_random(uid, seed, 3u, 4u) - 1.0;
    let radius = 50.0 * pow(em_random(uid, seed, 3u, 5u), 1.0 / 3.0) * sqrt(1.0 - cos_theta * cos_theta);
    var particle: EmberParticle;
    particle.position_age = vec4<f32>(cos(around) * radius, sin(around) * radius, -160.0, 0.0);
    particle.velocity_lifetime = vec4<f32>(0.0, 0.0, 0.0,
        mix(recipe.lifetime.x, recipe.lifetime.y, em_random(uid, seed, 3u, 1u)));
    particle.color = recipe.color_low;
    particle.size_mass_drag_kind = vec4<f32>(size, size / 10.0,
        mix(recipe.drag.x, recipe.drag.y, em_random(uid, seed, 3u, 6u)), 2.0);
    particle.anchor = vec4<f32>(origin, 0.0);
    return particle;
}

@compute @workgroup_size(64)
fn em_spawn_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let total = atomicLoad(&em_counters[2]);
    if (id.x >= total) { return; }
    let first_slot = atomicLoad(&em_counters[6]);
    let slot0 = id.x < first_slot;
    let kind = u32(select(em.rates.w, em.rates.z, slot0));
    let index = select(id.x - first_slot, id.x, slot0);
    let count = select(total - first_slot, first_slot, slot0);
    let uid = atomicLoad(&em_counters[3]) + id.x;
    // Interpolated spawn: births spread through the tick.
    let birth = (f32(index) + 0.5) / f32(max(count, 1u));
    let origin = mix(em.previous.xyz, em.anchor.xyz, birth);
    var particle: EmberParticle;
    if (kind == 2u) {
        particle = em_spawn_spark(uid, origin);
    } else {
        particle = em_spawn_ember(kind, uid, origin);
    }
    if (em_advance(&particle, em.previous.w * (1.0 - birth), em.anchor.w)) { em_append(particle); }
}

@compute @workgroup_size(1)
fn em_end() {
    let alive = min(atomicLoad(&em_counters[1]), em.misc.x);
    atomicStore(&em_counters[0], alive);
    atomicStore(&em_counters[3], atomicLoad(&em_counters[3]) + atomicLoad(&em_counters[2]));
    em_args[6] = 6u; em_args[7] = alive; em_args[8] = 0u; em_args[9] = 0u;
}
