// Cosmetic, map-geometry-aware Force Lightning. No gameplay decisions or GPU readback.
// One invocation owns an entire conductor; branches share its traced endpoints.
// This is dispatched after CPU snapshot submission and before FX rasterization.
struct Camera {
    view_proj: mat4x4<f32>,
    camera_pos_time: vec4<f32>,
    clip_plane: vec4<f32>,
    render_flags: vec4<u32>,
    camera_forward: vec4<f32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;

struct BvhNode {
    minimum: vec4<f32>,
    maximum: vec4<f32>,
    // x=left, y=right, z=first sorted triangle, w=count (0 = interior)
    node_data: vec4<u32>,
};
struct Triangle { a: vec4<f32>, b: vec4<f32>, c: vec4<f32> };
struct Sprite {
    origin: vec4<f32>,
    left: vec4<f32>,
    up: vec4<f32>,
    color: vec4<f32>,
};
struct Bolt {
    start: vec4<f32>,
    end: vec4<f32>,
    // radius, chaos, seed, quantized fx time
    shape: vec4<f32>,
    rgba: vec4<f32>,
};
struct Params { count: u32, first_sprite: u32, node_count: u32, triangle_count: u32 };
@group(1) @binding(0) var<storage, read> nodes: array<BvhNode>;
@group(1) @binding(1) var<storage, read> triangles: array<Triangle>;
@group(1) @binding(2) var<storage, read> bolts: array<Bolt>;
@group(1) @binding(3) var<storage, read_write> sprites: array<Sprite>;
@group(1) @binding(4) var<uniform> params: Params;

const SEG: u32 = 24u;
const BRANCH: u32 = 12u;
const BRANCHES: u32 = 2u;
const SPARK: u32 = 8u;
const STRIDE: u32 = SEG * 3u + BRANCH * BRANCHES + SPARK + 1u;
const PI: f32 = 3.14159265359;

fn rnd(v: f32) -> f32 { return fract(sin(v * 127.1 + 78.233) * 43758.5453); }
fn safe_normalize(v: vec3<f32>, backup: vec3<f32>) -> vec3<f32> {
    return select(backup, v * inverseSqrt(max(dot(v,v), 0.000001)), dot(v,v) > 0.000001);
}
fn basis(dir: vec3<f32>) -> vec3<f32> {
    return safe_normalize(cross(dir, select(vec3<f32>(0.0,1.0,0.0), vec3<f32>(0.0,0.0,1.0), abs(dir.y) > 0.90)), vec3<f32>(1.0,0.0,0.0));
}
struct Hit { distance: f32, normal: vec3<f32>, valid: bool };
fn slab(a: vec3<f32>, inv: vec3<f32>, lo: vec3<f32>, hi: vec3<f32>, max_distance: f32) -> bool {
    let t0 = (lo-a)*inv;
    let t1 = (hi-a)*inv;
    let minimum = min(t0,t1);
    let maximum = max(t0,t1);
    return max(max(minimum.x,minimum.y), max(minimum.z,0.0)) <=
        min(min(maximum.x,maximum.y), min(maximum.z,max_distance));
}
fn collide(a: vec3<f32>, b: vec3<f32>) -> Hit {
    let delta = b-a;
    let distance = length(delta);
    var result = Hit(distance, vec3<f32>(0.0,1.0,0.0), false);
    if (distance < 0.01 || params.node_count == 0u) { return result; }
    let dir = delta / distance;
    let inv = vec3<f32>(1.0) / select(vec3<f32>(0.00001), dir, abs(dir) > vec3<f32>(0.00001));
    var stack: array<u32, 48>;
    var top = 1u;
    stack[0] = 0u;
    var visits = 0u;
    // Bounding both stack and visits makes worst-case GPU cost predictable.
    loop {
        if (top == 0u || visits >= 320u) { break; }
        visits += 1u;
        top -= 1u;
        let index = stack[top];
        if (index >= params.node_count) { continue; }
        let n = nodes[index];
        if (!slab(a, inv, n.minimum.xyz, n.maximum.xyz, result.distance)) { continue; }
        if (n.node_data.w != 0u) {
            for (var i = 0u; i < n.node_data.w; i++) {
                let ti = n.node_data.z + i;
                if (ti >= params.triangle_count) { break; }
                let tri = triangles[ti];
                let e1 = tri.b.xyz-tri.a.xyz;
                let e2 = tri.c.xyz-tri.a.xyz;
                let p = cross(dir,e2);
                let det = dot(e1,p);
                if (abs(det) < 0.000001) { continue; }
                let invdet = 1.0 / det;
                let t = a-tri.a.xyz;
                let u = dot(t,p)*invdet;
                if (u < 0.0 || u > 1.0) { continue; }
                let q = cross(t,e1);
                let v = dot(dir,q)*invdet;
                if (v < 0.0 || u+v > 1.0) { continue; }
                let hit_dist = dot(e2,q)*invdet;
                if (hit_dist > 0.035 && hit_dist < result.distance - 0.01) {
                    result.distance = hit_dist;
                    result.normal = safe_normalize(cross(e1,e2), vec3<f32>(0.0,1.0,0.0));
                    if (dot(result.normal,dir)>0.0) { result.normal *= -1.0; }
                    result.valid = true;
                }
            }
        } else if (top + 2u <= 48u) {
            stack[top] = n.node_data.x;
            stack[top+1u] = n.node_data.y;
            top += 2u;
        }
    }
    return result;
}
// A lightning channel is a piecewise-angular, fractal displacement field,
// NOT a sine curve / smoothly interpolated noodle. Three broad anchor bends,
// eight medium bends and an independent node crackle share a stable FX tick.
// CPU lighting probes implement precisely the same path (arc_point in Rust).
fn noise2(value: f32, key: f32) -> vec2<f32> {
    return vec2<f32>(rnd(value*17.71+key),rnd(value*23.19+key+3.7))*2.0-1.0;
}
fn large_anchor(anchor_index: u32, key: f32) -> vec2<f32> {
    if (anchor_index == 0u || anchor_index >= 3u) { return vec2<f32>(0.0); }
    return noise2(f32(anchor_index)*3.17,key+2.3);
}
fn arc(a: vec3<f32>, b: vec3<f32>, index: u32, seed: f32, clock: f32, power: f32) -> vec3<f32> {
    if (index == 0u) { return a; }
    if (index >= SEG) { return b; }
    let t = f32(index)/f32(SEG);
    let ray = b-a;
    let length_world = length(ray);
    let direction = safe_normalize(ray,vec3<f32>(1.0,0.0,0.0));
    let right = basis(direction);
    let other = cross(direction,right);
    let key = seed*31.79 + floor(clock*34.0)*113.17;
    let interval = t*3.0;
    let anchor_index = min(u32(floor(interval)),2u);
    let broad = mix(large_anchor(anchor_index,key),large_anchor(anchor_index+1u,key),fract(interval));
    // Linear intervals are intentional: changing directions sharply at the
    // anchors makes forked discharges look electrical rather than parabolic.
    let medium_interval = f32(index)/3.0;
    let medium_index = floor(medium_interval);
    let medium = mix(noise2(medium_index,key+17.9),
                     noise2(medium_index+1.0,key+17.9),fract(medium_interval));
    let sharp = noise2(f32(index)*1.41,key+53.7);
    let envelope = pow(max(sin(PI*t),0.0),0.50);
    let amplitude = min(length_world*0.24+2.0,53.0)*power;
    let displacement = (broad*0.68 + medium*0.33 + sharp*0.22)*amplitude*envelope;
    return mix(a,b,t) + (right*displacement.x + other*displacement.y);
}
fn hidden() -> Sprite {
    return Sprite(vec4<f32>(0.0),vec4<f32>(0.0),vec4<f32>(0.0),vec4<f32>(0.0));
}
fn quad(a: vec3<f32>, b: vec3<f32>, radius: f32, rgb: vec3<f32>, opacity: f32, layer: f32) -> Sprite {
    let center = 0.5*(a+b);
    let view = safe_normalize(camera.camera_pos_time.xyz-center,vec3<f32>(0.0,0.0,1.0));
    let axis = safe_normalize(b-a,vec3<f32>(0.0,1.0,0.0));
    // Small overlap hides hard rectangular caps at sharp changes in tangent.
    // Bound by segment length to avoid drawing long streaks through surfaces.
    let overlap = min(radius*0.28, length(b-a)*0.11);
    var side = cross(axis, view);
    if (dot(side,side)<0.00001) { side = basis(axis); }
    side = safe_normalize(side,vec3<f32>(1.0,0.0,0.0));
    return Sprite(vec4<f32>(center,-layer),vec4<f32>(side*radius,0.0),
                  vec4<f32>((a-b)*0.5-axis*overlap,0.0),vec4<f32>(rgb,opacity));
}
fn flash(position: vec3<f32>, radius: f32, rgb: vec3<f32>, opacity: f32) -> Sprite {
    let eye = safe_normalize(camera.camera_pos_time.xyz-position,vec3<f32>(0.0,0.0,1.0));
    let right = basis(eye);
    return Sprite(vec4<f32>(position,-5.0),vec4<f32>(right*radius,0.0),
        vec4<f32>(cross(eye,right)*radius,0.0),vec4<f32>(rgb,opacity));
}

// Every output slot is overwritten every dispatch, including collapsed arcs.
// Otherwise a prior frame's sprite survives when an arc collides early.
@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let bid = id.x;
    if (bid >= params.count) { return; }
    let bolt = bolts[bid];
    let base = params.first_sprite + bid*STRIDE;
    let source = bolt.start.xyz;
    let endpoint = bolt.end.xyz;
    let thick = max(bolt.shape.x,0.15);
    let tint = vec3<f32>(0.10,0.32,1.0)*max(bolt.rgba.rgb,vec3<f32>(0.3));
    let inner = vec3<f32>(0.40,0.75,1.0);
    let white_core = vec3<f32>(0.75,0.91,1.0);
    let clock = bolt.shape.w;
    let seed = bolt.shape.z;
    let pulse = 0.78+0.22*rnd(seed*13.19+floor(clock*34.0)*5.31);
    var previous = source;
    var strike = endpoint;
    var normal = vec3<f32>(0.0,1.0,0.0);
    var incoming = safe_normalize(endpoint-source,vec3<f32>(0.0,0.0,1.0));
    var contact = false;
    var trunk_live = true;
    var contact_step = SEG+1u;
    var history: array<vec3<f32>,25>;
    history[0] = source;
    for (var i=0u; i<SEG; i++) {
        var next_point = previous;
        var visible = false;
        if (trunk_live) {
            let goal = arc(source,endpoint,i+1u,seed,clock,1.0);
            let edge = goal-previous;
            let hit = collide(previous,goal);
            next_point = goal;
            if (hit.valid) {
                next_point = previous+safe_normalize(edge,incoming)*hit.distance;
                strike = next_point;
                normal = hit.normal;
                contact = true;
                contact_step = i+1u;
                trunk_live = false;
            }
            incoming = safe_normalize(next_point-previous,incoming);
            visible = distance(previous,next_point) > 0.04;
        }
        history[i+1u] = next_point;
        if (!visible) {
            sprites[base+i] = hidden();
            sprites[base+SEG+i] = hidden();
            sprites[base+SEG*2u+i] = hidden();
        } else {
            let position = (f32(i)+0.5)/f32(SEG);
            let taper = 0.50+0.50*pow(max(sin(PI*position),0.0),0.45);
            let strength = bolt.rgba.a*pulse;
            // Broad faint scattering field, saturated blue plasma, fine hot
            // filament. These are intentionally different widths and energies.
            sprites[base+i] = quad(previous,next_point,thick*2.9*taper,tint,strength*0.075,1.0);
            sprites[base+SEG+i] = quad(previous,next_point,thick*0.75*taper,inner,strength*0.48,2.0);
            sprites[base+SEG*2u+i] = quad(previous,next_point,thick*0.13*taper,white_core,strength*0.62,3.0);
        }
        previous = next_point;
    }
    if (!contact) {
        let ray = safe_normalize(endpoint-source,vec3<f32>(0.0,0.0,1.0));
        let terminal = collide(endpoint-ray*1.5,endpoint+ray*2.5);
        if (terminal.valid) {
            contact = true;
            strike = endpoint;
            normal = terminal.normal;
        }
    }
    // Two true forks, each born on a different point of the parent trunk.
    // One may extinguish on a given FX sample; neither is a parallel duplicate.
    let trunk_direction = safe_normalize(endpoint-source,vec3<f32>(0.0,0.0,1.0));
    let lateral = basis(trunk_direction);
    let vertical = cross(trunk_direction,lateral);
    for (var fork_id=0u; fork_id<BRANCHES; fork_id++) {
        let attach = select(7u,15u,fork_id == 1u);
        let fork_begin = history[attach];
        let available = attach < contact_step && (!contact || distance(fork_begin,strike)>2.0);
        let polarity = select(-1.0,1.0,rnd(seed+f32(fork_id)*17.7)>0.5);
        let diversion = min(length(endpoint-source)*0.19+3.0,43.0);
        let branch_end = mix(fork_begin,strike,select(0.38,0.57,fork_id==1u))
             + lateral*polarity*diversion
             + vertical*(rnd(seed*7.7+f32(fork_id)*9.1)*2.0-1.0)*diversion*0.62;
        var fork_previous = fork_begin;
        var branch_live = available;
        // Gate entire fork per FX frame to avoid a static, symmetrical tree.
        if (rnd(floor(clock*34.0)*2.71+seed+f32(fork_id)*14.11) < 0.14) {
            branch_live = false;
        }
        for (var j=0u; j<BRANCH; j++) {
            let slot = base+SEG*3u+fork_id*BRANCH+j;
            if (!branch_live) { sprites[slot] = hidden(); continue; }
            let goal = arc(fork_begin,branch_end,(j+1u)*2u,seed+f32(fork_id)*31.0+19.0,clock,0.47);
            let hit = collide(fork_previous,goal);
            var next_point = goal;
            if (hit.valid) {
                next_point = fork_previous+safe_normalize(goal-fork_previous,trunk_direction)*hit.distance;
                branch_live = false;
            }
            if (distance(next_point,fork_previous)<0.05) {
                sprites[slot] = hidden();
            } else {
                let fade = 1.0-f32(j)/f32(BRANCH);
                sprites[slot] = quad(fork_previous,next_point,thick*0.24*fade+0.04,
                                     tint*1.45,bolt.rgba.a*pulse*0.35*fade,2.0);
            }
            fork_previous = next_point;
        }
    }
    // Four short, 2-link surface crawls. Each begins at the actual solid-BSP
    // contact and stays just outside the plane (not 8 radial straight spokes).
    let t1 = safe_normalize(cross(normal,incoming),basis(normal));
    let t2 = safe_normalize(cross(normal,t1),basis(normal));
    for (var arm=0u; arm<4u; arm++) {
        let angle = f32(arm)*2.39996+seed;
        let tangent = t1*cos(angle)+t2*sin(angle);
        let sideways = safe_normalize(cross(normal,tangent),t1);
        let length_arm = (3.5+7.0*rnd(seed+f32(arm)*9.1))*select(0.6,1.0,arm<2u);
        let start_pos = strike+normal*0.45;
        let halfway = start_pos+tangent*length_arm*0.48+sideways*(rnd(seed+f32(arm)*4.3)-0.5)*3.0;
        let final_pos = start_pos+tangent*length_arm+sideways*(rnd(seed+f32(arm)*6.7)-0.5)*4.0;
        var crawl_previous = start_pos;
        for (var j=0u; j<2u; j++) {
            let slot = base+SEG*3u+BRANCH*BRANCHES+arm*2u+j;
            if (!contact) { sprites[slot] = hidden(); continue; }
            let goal = select(halfway,final_pos,j==1u);
            let hit = collide(crawl_previous,goal);
            let edge = goal-crawl_previous;
            let next_point = select(goal,crawl_previous+safe_normalize(edge,tangent)*hit.distance,hit.valid);
            sprites[slot] = quad(crawl_previous,next_point,thick*0.13,
                tint*1.7,bolt.rgba.a*pulse*0.40*(1.0-f32(j)*0.36),2.0);
            crawl_previous = next_point;
        }
    }
    if (contact) {
        sprites[base+STRIDE-1u] = flash(strike,thick*6.0,inner,bolt.rgba.a*pulse*0.28);
    } else {
        sprites[base+STRIDE-1u] = hidden();
    }
}
