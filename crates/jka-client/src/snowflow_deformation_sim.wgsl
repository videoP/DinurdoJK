// Direct native-WGPU port of Noniv/snowflow_demo deformSim.fragment.wgsl.
// The only optimization is skipping neighbour reads when dt == 0; Snowflow's
// own comments guarantee every relaxation term is then an exact no-op.

struct Params {
    window0: vec4<f32>, // center.xy, prevCenter.xy
    window1: vec4<f32>, // size, res, dt, brushCount
    state0: vec4<f32>,  // refillRate, maxDepth, maxBerm, windAngle
};
@group(0) @binding(0) var prev_tex: texture_2d<f32>;
@group(0) @binding(1) var prev_sampler: sampler;
@group(0) @binding(2) var brush_tex: texture_2d<f32>;
@group(0) @binding(3) var<uniform> params: Params;

fn hash21(p: vec2f) -> f32 {
    var p3 = fract(vec3f(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}
fn grad2(i: vec2f) -> vec2f {
    let a = hash21(i) * 6.28318530718;
    return vec2f(cos(a), sin(a));
}
fn noised(p: vec2f) -> vec3f {
    let i = floor(p);
    let f = p - i;
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let du = 30.0 * f * f * (f * (f - 2.0) + 1.0);
    let ga = grad2(i + vec2f(0.0, 0.0));
    let gb = grad2(i + vec2f(1.0, 0.0));
    let gc = grad2(i + vec2f(0.0, 1.0));
    let gd = grad2(i + vec2f(1.0, 1.0));
    let va = dot(ga, f - vec2f(0.0, 0.0));
    let vb = dot(gb, f - vec2f(1.0, 0.0));
    let vc = dot(gc, f - vec2f(0.0, 1.0));
    let vd = dot(gd, f - vec2f(1.0, 1.0));
    let k0 = va;
    let k1 = vb - va;
    let k2 = vc - va;
    let k3 = va - vb - vc + vd;
    let value = k0 + k1 * u.x + k2 * u.y + k3 * u.x * u.y;
    let deriv = ga + u.x * (gb - ga) + u.y * (gc - ga)
        + u.x * u.y * (ga - gb - gc + gd)
        + du * (vec2f(u.y, u.x) * k3 + vec2f(k1, k2));
    return vec3f(value, deriv);
}
fn noise2(p: vec2f) -> f32 { return noised(p).x; }

struct VsOut { @builtin(position) position: vec4f, @location(0) uv: vec2f };
@vertex fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    // Match the renderer's existing fullscreen convention: normalized texture Y
    // runs downward while clip-space Y runs upward. Keeping this mapping exact is
    // required because the toroidal state is addressed later with fract(world/size).
    let p = array<vec2f, 3>(vec2f(-1.0,-3.0), vec2f(-1.0,1.0), vec2f(3.0,1.0));
    var out: VsOut;
    out.position = vec4f(p[index], 0.0, 1.0);
    out.uv = vec2f(p[index].x * 0.5 + 0.5, 0.5 - p[index].y * 0.5);
    return out;
}
fn texel_world(uv: vec2f, centre: vec2f, size: f32) -> vec2f {
    let base = uv * size;
    return base + size * round((centre - base) / size);
}

@fragment fn fs_main(input: VsOut) -> @location(0) vec4f {
    let uv = input.uv;
    let center = params.window0.xy;
    let prev_center = params.window0.zw;
    let size = params.window1.x;
    let res = params.window1.y;
    let dt = params.window1.z;
    let brush_count = i32(params.window1.w);
    let refill_rate = params.state0.x;
    let max_depth = params.state0.y;
    let max_berm = params.state0.z;
    let wind_angle = params.state0.w;
    let world = texel_world(uv, center, size);

    var dep = 0.0;
    var berm = 0.0;
    var comp = 0.0;
    var ice = 0.0;
    let was_inside = all(abs(world - prev_center) <= vec2f(size * 0.5));
    if (was_inside) {
        let c = textureSampleLevel(prev_tex, prev_sampler, uv, 0.0);
        dep = c.r; berm = c.g; comp = c.b; ice = c.a;
        if (dt > 0.0) {
            let t = 1.0 / res;
            let xl = textureSampleLevel(prev_tex, prev_sampler, uv - vec2f(t,0.0), 0.0);
            let xr = textureSampleLevel(prev_tex, prev_sampler, uv + vec2f(t,0.0), 0.0);
            let zd = textureSampleLevel(prev_tex, prev_sampler, uv - vec2f(0.0,t), 0.0);
            let zu = textureSampleLevel(prev_tex, prev_sampler, uv + vec2f(0.0,t), 0.0);
            let k = clamp(refill_rate * dt, 0.0, 1.0);
            let k_dep = min(0.22, 0.004 * k);
            let k_berm = min(0.22, 0.012 * k);
            dep += ((xl.r+xr.r+zd.r+zu.r)-4.0*dep) * k_dep;
            berm += ((xl.g+xr.g+zd.g+zu.g)-4.0*berm) * k_berm;
            let wdir = vec2f(sin(wind_angle), cos(wind_angle));
            let uw = textureSampleLevel(prev_tex, prev_sampler, uv - wdir * (t * 1.6), 0.0);
            let k_adv = min(0.2, 0.002 * k);
            dep = mix(dep, uw.r, k_adv * 0.6);
            berm = mix(berm, uw.g, k_adv);
            let slump = min(berm, dep) * min(0.6, 0.002 * refill_rate * dt);
            dep -= slump; berm -= slump;
            dep *= exp(-dt * refill_rate / 400.0);
            berm *= exp(-dt * refill_rate / 250.0);
            comp *= exp(-dt * refill_rate / 300.0);
            ice *= exp(-dt * refill_rate / 900.0);
        }
    }

    for (var i = 0; i < brush_count; i++) {
        let a = textureLoad(brush_tex, vec2i(i,0), 0);
        let b = textureLoad(brush_tex, vec2i(i,1), 0);
        let c = textureLoad(brush_tex, vec2i(i,2), 0);
        let radius = a.z;
        if (radius <= 0.0) { continue; }
        var p = world - a.xy;
        p -= size * round(p / size);
        let reach = radius * max(a.w, 1.0) * 1.6;
        if (abs(p.x) > reach || abs(p.y) > reach) { continue; }
        let q = vec2f((p.x*b.x+p.y*b.y)/(radius*a.w), (-p.x*b.y+p.y*b.x)/radius);
        let d = length(q);
        if (d > 1.55) { continue; }
        let ang = atan2(q.y, q.x);
        let wob = 1.0 + c.z * 0.22 * noise2(vec2f(cos(ang),sin(ang))*2.7 + c.w);
        let dn = d / wob;
        let core = 1.0 - smoothstep(0.42, 1.0, dn);
        let ring_d = (dn - 1.04) * 3.4;
        let ring = exp(-ring_d * ring_d);
        let grain = 0.72 + 0.56 * (noise2(q*7.5 + c.w*3.1)*0.5 + 0.5);
        dep += b.z * core;
        berm += b.w * ring * grain;
        comp += c.x * core;
        ice = max(ice, c.y * core);
    }
    return vec4f(clamp(dep,0.0,max_depth), clamp(berm,0.0,max_berm), clamp(comp,0.0,1.0), clamp(ice,0.0,1.0));
}
