// Surface deformation integration.
// 2D keeps the stock JKA footprint marks. 3D reads Snowflow's persistent
// toroidal RGBA16F state field: R depression, G displaced berm mass,
// B compression, A ice, and also keeps the stock 2D boot mark layered over
// the deformed snow so the two modes are additive rather than mutually exclusive.
// The state field is written by snowflow_deformation_sim.wgsl.

const MAX_SURFACE_DEFORMATION_STAMPS: u32 = 32u;
const FOOTPRINT_MODE_2D: u32 = 1u;
const FOOTPRINT_MODE_3D: u32 = 2u;
const MATERIAL_SNOW_WORD: u32 = 14u;
const JKA_MARK_RADIUS: f32 = 6.0;

struct LegacyStampGpu {
    position_half_width: vec4<f32>,
    forward_half_length: vec4<f32>,
    normal_depth: vec4<f32>,
    foot_kind: vec4<u32>,
};
struct SurfaceDeformationUniform {
    header: vec4<u32>,
    snowflow_header: vec4<u32>,
    snowflow_params: vec4<f32>,
    snowflow_params2: vec4<f32>,
    legacy_bounds: vec4<f32>,
    stamps: array<LegacyStampGpu, 32>,
};

@group(1) @binding(21) var<uniform> surface_deformation: SurfaceDeformationUniform;
@group(1) @binding(22) var stock_footstep_l: texture_2d<f32>;
@group(1) @binding(23) var stock_footstep_r: texture_2d<f32>;
@group(1) @binding(24) var stock_footstep_sampler: sampler;
@group(1) @binding(25) var snowflow_field_0: texture_2d<f32>;
@group(1) @binding(26) var snowflow_field_1: texture_2d<f32>;
@group(1) @binding(27) var snowflow_field_sampler: sampler;

fn surface_material_id(material_word: u32) -> u32 { return (material_word >> 8u) & 31u; }
fn is_authored_snow(material_word: u32) -> bool { return surface_material_id(material_word) == MATERIAL_SNOW_WORD; }
fn xz_inside_bounds(position: vec2<f32>, bounds: vec4<f32>) -> bool {
    return position.x >= bounds.x && position.y >= bounds.y && position.x <= bounds.z && position.y <= bounds.w;
}

// ------------------------------------------------------------------------- 2D
fn footprint_basis(stamp: LegacyStampGpu) -> mat3x3<f32> {
    let n = normalize(stamp.normal_depth.xyz);
    var forward = stamp.forward_half_length.xyz - n * dot(stamp.forward_half_length.xyz, n);
    if (dot(forward, forward) < 1e-5) {
        let helper = select(vec3<f32>(1.0,0.0,0.0), vec3<f32>(0.0,0.0,1.0), abs(n.x) > 0.8);
        forward = cross(n, helper);
    }
    forward = normalize(forward);
    return mat3x3<f32>(normalize(cross(n, forward)), forward, n);
}
fn sample_stock_footprint(kind: u32, uv: vec2<f32>) -> vec4<f32> {
    if (kind == 0u) { return textureSampleLevel(stock_footstep_l, stock_footstep_sampler, uv, 0.0); }
    return textureSampleLevel(stock_footstep_r, stock_footstep_sampler, uv, 0.0);
}
fn shade_stock_2d_footprints(world_position: vec3<f32>, color: vec4<f32>) -> vec4<f32> {
    if (surface_deformation.header.x == 0u || !xz_inside_bounds(world_position.xz, surface_deformation.legacy_bounds)) {
        return color;
    }
    var out_color = color;
    let count = min(surface_deformation.header.x, MAX_SURFACE_DEFORMATION_STAMPS);
    let available = surface_deformation.header.z;
    for (var i = 0u; i < count; i = i + 1u) {
        let stamp = surface_deformation.stamps[i];
        let coarse = world_position.xz - stamp.position_half_width.xz;
        if (abs(coarse.x) > 8.5 || abs(coarse.y) > 12.5 || abs(world_position.y - stamp.position_half_width.y) > 5.0) { continue; }
        let kind = min(stamp.foot_kind.x, 1u);
        if ((available & (1u << kind)) == 0u) { continue; }
        let basis = footprint_basis(stamp);
        let delta = world_position - stamp.position_half_width.xyz;
        if (abs(dot(delta, basis[2])) > 3.0) { continue; }
        let local = vec2<f32>(dot(delta,basis[0]), dot(delta,basis[1])) / JKA_MARK_RADIUS;
        if (abs(local.x) > 1.0 || abs(local.y) > 1.0) { continue; }
        let mark = sample_stock_footprint(kind, vec2<f32>(0.5 + local.x*0.5, 0.5 - local.y*0.5));
        let blend_mode = (surface_deformation.header.w >> (kind * 4u)) & 0x0fu;
        if (blend_mode == 1u) { out_color = vec4<f32>(out_color.rgb * mark.rgb, out_color.a); }
        else if (blend_mode == 2u) { out_color = vec4<f32>(out_color.rgb * (vec3<f32>(1.0)-mark.rgb), out_color.a); }
        else if (blend_mode == 3u) { out_color = vec4<f32>(out_color.rgb + mark.rgb, out_color.a); }
        else { out_color = vec4<f32>(mix(out_color.rgb, mark.rgb, clamp(mark.a,0.0,1.0)), out_color.a); }
    }
    return out_color;
}

// --------------------------------------------------------------- Snowflow read
fn snowflow_uv(world_xz: vec2<f32>) -> vec2<f32> {
    return fract(world_xz / surface_deformation.snowflow_params.z);
}
fn snowflow_falloff(world_xz: vec2<f32>) -> f32 {
    if (surface_deformation.snowflow_header.y == 0u) { return 0.0; }
    let centre = surface_deformation.snowflow_params.xy;
    let size = surface_deformation.snowflow_params.z;
    let d = abs(world_xz - centre) / (size * 0.5);
    return 1.0 - smoothstep(0.80, 0.96, max(d.x, d.y));
}
fn snowflow_sample_uv(uv: vec2<f32>) -> vec4<f32> {
    if (surface_deformation.snowflow_header.x == 0u) {
        return textureSampleLevel(snowflow_field_0, snowflow_field_sampler, uv, 0.0);
    }
    return textureSampleLevel(snowflow_field_1, snowflow_field_sampler, uv, 0.0);
}
fn snowflow_sample(world_xz: vec2<f32>) -> vec4<f32> {
    return snowflow_sample_uv(snowflow_uv(world_xz));
}

// Snowflow's source gradient noise, namespaced to avoid colliding with JKA shaders.
fn snowflow_hash21(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}
fn snowflow_grad2(i: vec2<f32>) -> vec2<f32> {
    let a = snowflow_hash21(i) * 6.28318530718;
    return vec2<f32>(cos(a), sin(a));
}
fn snowflow_noise2(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = p - i;
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let ga = snowflow_grad2(i + vec2<f32>(0.0, 0.0));
    let gb = snowflow_grad2(i + vec2<f32>(1.0, 0.0));
    let gc = snowflow_grad2(i + vec2<f32>(0.0, 1.0));
    let gd = snowflow_grad2(i + vec2<f32>(1.0, 1.0));
    let va = dot(ga, f - vec2<f32>(0.0, 0.0));
    let vb = dot(gb, f - vec2<f32>(1.0, 0.0));
    let vc = dot(gc, f - vec2<f32>(0.0, 1.0));
    let vd = dot(gd, f - vec2<f32>(1.0, 1.0));
    return va + (vb - va) * u.x + (vc - va) * u.y
        + (va - vb - vc + vd) * u.x * u.y;
}

// Exact Snowflow deformHeight() read filter, with the current local overlay
// lattice spacing supplied by Rust. JKA's only geometry deviation is the max(0)
// at the call site so a footprint cannot sink beneath the authored BSP floor.
fn snowflow_filtered_height(world_xz: vec2<f32>) -> f32 {
    let w = snowflow_falloff(world_xz);
    if (w <= 0.0) { return 0.0; }
    let size = surface_deformation.snowflow_params.z;
    let spacing = surface_deformation.snowflow_params2.y;
    let base = snowflow_uv(world_xz);
    let r = spacing / size;
    var acc = 0.0;
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            let wt = f32((2 - abs(i)) * (2 - abs(j))) * (1.0 / 16.0);
            let s = snowflow_sample_uv(base + vec2<f32>(f32(i), f32(j)) * r);
            acc += (s.g - s.r) * wt;
        }
    }
    return acc * w;
}

fn snowflow_deformed_normal(world_position: vec3<f32>, base_normal: vec3<f32>) -> vec3<f32> {
    let w = snowflow_falloff(world_position.xz);
    if (w <= 0.0) { return normalize(base_normal); }
    let step = surface_deformation.snowflow_params.w * 2.0;
    let a = snowflow_sample(world_position.xz + vec2<f32>(step,0.0));
    let b = snowflow_sample(world_position.xz - vec2<f32>(step,0.0));
    let c = snowflow_sample(world_position.xz + vec2<f32>(0.0,step));
    let d = snowflow_sample(world_position.xz - vec2<f32>(0.0,step));
    let gx = ((a.g-a.r) - (b.g-b.r)) / (2.0*step) * w;
    let gz = ((c.g-c.r) - (d.g-d.r)) / (2.0*step) * w;
    // Snow ground is deliberately limited to upward-facing authored surfaces.
    // Add the field gradient to the authored normal rather than replacing it.
    return normalize(base_normal + vec3<f32>(-gx, 0.0, -gz));
}

fn deform_surface_deformation_vertex(
    position: vec3<f32>, normal: vec3<f32>, material_word: u32,
    instance_index: u32, allow_shell: bool,
) -> vec4<f32> {
    if (!allow_shell || instance_index == 0u || surface_deformation.header.y != FOOTPRINT_MODE_3D ||
        !is_authored_snow(material_word) || normalize(normal).y <= 0.35) {
        return vec4<f32>(position, 0.0);
    }
    let height = max(snowflow_filtered_height(position.xz), 0.0);
    return vec4<f32>(position + normalize(normal) * (height + 0.025), 1.0);
}

fn surface_deformation_vertex_normal(
    position: vec3<f32>, normal: vec3<f32>, material_word: u32,
    instance_index: u32, allow_shell: bool,
) -> vec3<f32> {
    if (!allow_shell || instance_index == 0u || surface_deformation.header.y != FOOTPRINT_MODE_3D ||
        !is_authored_snow(material_word) || normalize(normal).y <= 0.35) {
        return normalize(normal);
    }
    return snowflow_deformed_normal(position, normal);
}

fn surface_deformation_should_discard(
    world_position: vec3<f32>, material_word: u32, world_normal: vec3<f32>,
    shell_kind: u32, shell_coverage: f32, pixel_position: vec2<f32>, allow_shell: bool,
) -> bool {
    if (shell_kind == 0u) { return false; }
    if (!allow_shell || surface_deformation.header.y != FOOTPRINT_MODE_3D ||
        !is_authored_snow(material_word) || normalize(world_normal).y <= 0.25 ||
        snowflow_falloff(world_position.xz) <= 0.0) { return true; }
    // Do not cut the overlay at the centre texel's state boundary. deformHeight()
    // deliberately samples a wider 3x3 neighbourhood, so doing that would clip
    // the filtered berm fringe and recreate the jagged edge the source filter is
    // designed to remove. Local chunk culling keeps the flat zero-state overlay
    // cheap; its 0.025u bias simply covers the authored BSP underneath.
    return false;
}

fn shade_snowflow_state(world_position: vec3<f32>, color: vec4<f32>) -> vec4<f32> {
    let weight = snowflow_falloff(world_position.xz);
    if (weight <= 0.001) { return color; }

    // Match Snowflow snow.fragment.wgsl: derive a world-space pixel footprint,
    // widen the deformation-normal baseline as pixels get larger, and reuse the
    // four neighbours to low-pass the state channels at distance.
    let d_uv = snowflow_uv(world_position.xz);
    let center = snowflow_sample_uv(d_uv);
    if (max(max(center.r, center.g), center.b) < 0.00001) { return color; }
    let ddx_w = dpdx(world_position);
    let ddy_w = dpdy(world_position);
    let footprint_min = max(min(length(ddx_w.xz), length(ddy_w.xz)), 1e-4);
    let deform_texel = surface_deformation.snowflow_params.w;
    let step = max(deform_texel * 2.0, footprint_min * 1.4);
    let e_uv = step / surface_deformation.snowflow_params.z;
    let dx_a = snowflow_sample_uv(d_uv + vec2<f32>(e_uv, 0.0));
    let dx_b = snowflow_sample_uv(d_uv - vec2<f32>(e_uv, 0.0));
    let dz_a = snowflow_sample_uv(d_uv + vec2<f32>(0.0, e_uv));
    let dz_b = snowflow_sample_uv(d_uv - vec2<f32>(0.0, e_uv));
    let wide = clamp(footprint_min / (deform_texel * 4.0), 0.0, 1.0) * 0.8;
    let state = mix(center, (center + dx_a + dx_b + dz_a + dz_b) * 0.2, wide);

    let metres_per_unit = surface_deformation.snowflow_params2.x;
    let deform_depth = state.r * metres_per_unit * weight;
    let deform_berm = state.g * metres_per_unit * weight;
    let compression = clamp(state.b, 0.0, 1.0) * weight;

    // Source material state, applied relative to the authored JKA base texture.
    // This preserves map art while retaining Snowflow's packed-vs-loose contrast.
    var rgb = color.rgb;
    // Broader packed-snow darkening so the trough and inner berm wall read as
    // churned/compacted snow, while the strongest compression in the actual
    // foot plants still remains darker than the rest.
    let trough_dark = clamp(deform_depth * 1.15 + compression * 0.20, 0.0, 1.0);
    rgb *= mix(vec3<f32>(1.0), vec3<f32>(0.84, 0.87, 0.92), trough_dark * 0.40);
    if (deform_berm > 0.002) {
        let loose = clamp(deform_berm * 5.0, 0.0, 1.0);
        rgb *= mix(vec3<f32>(1.0), vec3<f32>(1.047, 1.040, 1.021), loose * 0.55);
        // Make the berm readable from its actual shape rather than merely from
        // displaced-mass amount. Reuse the four state taps already fetched above
        // to estimate d(berm - depression)/dxz, so this adds no texture samples.
        // Both the inner and outer berm faces darken as they get steeper, while
        // the flat crest remains relatively bright/loose like Snowflow.
        let slope_x = ((dx_a.g - dx_a.r) - (dx_b.g - dx_b.r)) / max(2.0 * step, 1e-4);
        let slope_z = ((dz_a.g - dz_a.r) - (dz_b.g - dz_b.r)) / max(2.0 * step, 1e-4);
        let berm_slope = smoothstep(0.06, 0.62, length(vec2<f32>(slope_x, slope_z))) * weight;
        rgb *= mix(vec3<f32>(1.0), vec3<f32>(0.72, 0.77, 0.86), berm_slope * 0.68);

        // The inside wall additionally carries packed/churned snow from the trough.
        let inner_berm = clamp(loose * (deform_depth * 2.2 + compression * 0.9), 0.0, 1.0);
        rgb *= mix(vec3<f32>(1.0), vec3<f32>(0.86, 0.89, 0.94), inner_berm * 0.34);
        let world_m = world_position.xz * metres_per_unit;
        let chunk = snowflow_noise2(world_m * 34.0) * 0.5 + 0.5;
        rgb *= 1.0 - loose * 0.10 * chunk;
    }
    // Keep actual boot plants darker than the surrounding trough / berm.
    rgb *= mix(vec3<f32>(1.0), vec3<f32>(0.725, 0.751, 0.799), compression * 0.85);
    let ao = 1.0 - clamp(deform_depth * 1.9, 0.0, 1.0) * 0.38;
    let cave_tint = mix(vec3<f32>(1.0), vec3<f32>(0.55,0.72,1.0), (1.0-ao)*0.95);
    rgb *= ao * cave_tint;
    return vec4<f32>(rgb, color.a);
}

fn shade_surface_deformation(
    world_position: vec3<f32>, world_normal: vec3<f32>, material_word: u32,
    tc_gen: u32, shell_kind: u32, shell_darkness: f32, color: vec4<f32>,
) -> vec4<f32> {
    if (tc_gen == 1u) { return color; }
    if (surface_deformation.header.y == FOOTPRINT_MODE_2D) {
        if (!is_authored_snow(material_word)) { return color; }
        return shade_stock_2d_footprints(world_position, color);
    }
    if (surface_deformation.header.y == FOOTPRINT_MODE_3D && is_authored_snow(material_word)) {
        // 3D mode is intentionally "3D + 2D": keep Snowflow's persistent
        // deformation/material response, then stamp the original JKA footstep
        // texture on the visible snow surface as well. Legacy stamps are already
        // recorded for Foot contacts in 3D mode, so this adds no CPU-side work.
        return shade_stock_2d_footprints(world_position, shade_snowflow_state(world_position, color));
    }
    return color;
}
