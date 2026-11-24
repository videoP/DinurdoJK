// HL2 underwater view for Source lake water (r_lakeUnderwaterMode 6), ported
// from the Source SDK 2013:
//   fog      The lake's Source fog color and distance (shared with the fog
//            seen through the surface from above), applied once to everything
//            drawn, the surface seen from below included, with the other lake
//            underwater modes' falloff: radial eye distance, 1 - exp2(-4.32 t),
//            t = d / distance, 95% at the fog distance. (HL2's curve, a squared
//            linear range factor plus the beneath material's own lerp, left a
//            hard waterline where the double-fogged surface met the sand.)
//            Nothing drawn (sky): full fog, as HL2 clears to the fog color.
//   overlay  $underwateroverlay "effects/water_warp01" (water_coast01_beneath
//            only), drawn by DrawUnderwaterOverlay after DisableFog():
//            Refract, $refractamount .04, $bluramount 1 (BLUR=1),
//            $fadeoutonsilhouette undefined -> 0 (blend 1), $refracttint 1,
//            $normalmap water_coast01_normal, TextureScroll .1 at 45 degrees.
//            It samples _rt_PowerOfTwoFB, a stretched copy of the frame.

struct Params {
    inverse_view_projection: mat4x4<f32>,
    camera: vec4<f32>,   // xyz eye (render units)
    forward: vec4<f32>,  // xyz view direction
    fog_color: vec4<f32>,// linear
    fog: vec4<f32>,      // start, end (units), time (s), unused
    size: vec4<f32>,     // scene width, height, power-of-two size, 0
    caustics: vec4<f32>, // x lake plane height (render units), y r_lakeUnderwaterCaustics on
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var scene_color: texture_2d<f32>;
@group(0) @binding(2) var scene_depth: texture_depth_2d;
@group(0) @binding(3) var pot_color: texture_2d<f32>;
@group(0) @binding(4) var clamp_linear: sampler;
@group(0) @binding(5) var normals: texture_2d_array<f32>;
@group(0) @binding(6) var repeat_linear: sampler;
// Fluid Ninja caustics volume (T_Caustics_Animated_02); W repeats.
@group(0) @binding(7) var caustics_volume: texture_3d<f32>;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
}

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> VertexOut {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return VertexOut(vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0));
}

// The scene texel at `pixel` with the underwater range fog applied.
fn fogged(pixel: vec2<i32>) -> vec3<f32> {
    let size = vec2<i32>(params.size.xy);
    let p = clamp(pixel, vec2<i32>(0), size - 1);
    var color = textureLoad(scene_color, p, 0).rgb;
    let depth = textureLoad(scene_depth, p, 0);
    var f = vec3<f32>(1.0);
    if depth > 0.0 {
        let ndc = (vec2<f32>(p) + 0.5) / params.size.xy * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
        let world_h = params.inverse_view_projection * vec4<f32>(ndc, depth, 1.0);
        let world = world_h.xyz / world_h.w;
        let distance = length(world - params.camera.xyz);
        // r_lakeUnderwaterCaustics (DinurdoJK, not HL2): the lake's Fluid Ninja
        // caustics on submerged geometry, before the range fog dims them. No
        // wake window here (sim off). Units: render units, 2.5 cm each.
        if params.caustics.y > 0.5 && world.y < params.caustics.x {
            let footprint = distance * 2.5 * 2.0 / params.size.y;
            color = color * ninja_lake_caustics(caustics_volume, repeat_linear, scene_color, pot_color,
                clamp_linear, vec2<f32>(0.5), false, vec2<f32>(world.x, -world.z) * 2.5,
                (params.caustics.x - world.y) * 2.5, distance * 2.5, params.fog.z,
                vec2<f32>(footprint, 0.0), vec2<f32>(0.0, footprint));
        }
        let t = max(distance - params.fog.x, 0.0) / max(params.fog.y - params.fog.x, 1.0);
        // Per-channel absorption from the fog colour, as the lake surface
        // (blue-water-gpu source_water.wgsl source_water_fog_mix).
        let c = params.fog_color.rgb;
        let peak = max(max(c.r, c.g), max(c.b, 1e-4));
        let rate = vec3<f32>(1.0) + 4.0 * (vec3<f32>(1.0) - saturate(c / peak));
        f = vec3<f32>(1.0) - exp2(-4.321928 * t * rate);
    }
    return mix(color, params.fog_color.rgb, f);
}

@fragment
fn fog_fragment(input: VertexOut) -> @location(0) vec4<f32> {
    return vec4<f32>(fogged(vec2<i32>(input.clip.xy)), 1.0);
}

// _rt_PowerOfTwoFB: the fogged frame stretched (bilinear) to size.z^2.
@fragment
fn pot_fragment(input: VertexOut) -> @location(0) vec4<f32> {
    let source = input.clip.xy / params.size.z * params.size.xy - 0.5;
    let base = floor(source);
    let w = source - base;
    let b = vec2<i32>(base);
    let top = mix(fogged(b), fogged(b + vec2<i32>(1, 0)), w.x);
    let bottom = mix(fogged(b + vec2<i32>(0, 1)), fogged(b + vec2<i32>(1, 1)), w.x);
    return vec4<f32>(mix(top, bottom, w.y), 1.0);
}

const BLUR_FRACTION: f32 = 1.0 / 512.0;
const HALF_BLUR_FRACTION: f32 = 0.5 / 512.0;

// refract_ps2x.fxc, BLUR == 1 ("polyphase" 3x3 box in four bilinear taps).
@fragment
fn overlay_fragment(input: VertexOut) -> @location(0) vec4<f32> {
    let uv = input.clip.xy / params.size.xy;
    // DrawScreenSpaceRectangle texcoords span the screen; TextureScroll on
    // $bumptransform: time * rate * (cos, sin)(angle).
    let bump = uv + vec2<f32>(0.70710678, 0.70710678) * (0.1 * params.fog.z);
    // The coast normal is the source normal array's last layer.
    let normal = textureSample(normals, repeat_linear, bump, textureNumLayers(normals) - 1u);
    // DecompressNormal (NORM_DECODE_NONE); scale = vNormal.a * $refractamount.
    let n = normal.xyz * 2.0 - 1.0;
    let coord = uv + n.xy * (normal.a * 0.04);
    var result = textureSample(pot_color, clamp_linear, coord - vec2<f32>(HALF_BLUR_FRACTION)).rgb * 0.4444444;
    result += textureSample(pot_color, clamp_linear, coord + vec2<f32>(BLUR_FRACTION, -HALF_BLUR_FRACTION)).rgb * 0.2222222;
    result += textureSample(pot_color, clamp_linear, coord + vec2<f32>(-HALF_BLUR_FRACTION, BLUR_FRACTION)).rgb * 0.2222222;
    result += textureSample(pot_color, clamp_linear, coord + vec2<f32>(BLUR_FRACTION)).rgb * 0.1111111;
    // lerp(unblurred, result * refractTint, blend) with blend = 1, tint = 1.
    return vec4<f32>(result, 1.0);
}
