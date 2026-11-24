// Host prelude for the Fluid Ninja river's submerged post material
// (jka_ninja::underwater_wgsl) over the Ninja ocean. Names and units match
// the river host (river_surface.wgsl / river_underwater.wgsl): JKA axes,
// centimetres, `river_view` built by jka_river::river_surface::ViewUniform.

struct NinjaOceanView {
    eye: vec4<f32>,          // xyz cm, w far (JKA units)
    forward: vec4<f32>,      // w near (JKA units)
    right: vec4<f32>,        // w Ninja material time
    up: vec4<f32>,           // w 1 = Ninja underwater, 2 = with caustics
    distortion: vec4<f32>,   // x ViewToClip M[0][0], y aspect
    sun_direction: vec4<f32>,
    mask: vec4<f32>,         // x 0 = Blue Water probe, 1 = flat-volume water path (lakes); yz host fade radial cm
};
struct NinjaOceanAuthored {
    ninja_optics: vec4<f32>, // distance density, distortion, brightness, caustics gain
    ninja_distance_color: vec4<f32>,
    ninja_caustics_color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> river_view: NinjaOceanView;
@group(0) @binding(1) var<uniform> authored: NinjaOceanAuthored;
@group(0) @binding(2) var scene_color: texture_2d<f32>;
@group(0) @binding(3) var scene_depth: texture_depth_2d;
// Blue Water's underwater probe: metres below the surface of each pixel's
// near-plane point, negative above it (same mask as the native composite).
@group(0) @binding(4) var depth_probe: texture_2d<f32>;
@group(0) @binding(5) var normal_sampler: sampler;
@group(0) @binding(6) var ninja_wrap: sampler;
@group(0) @binding(7) var ninja_underwater_noise: texture_2d<f32>;
@group(0) @binding(8) var ninja_underwater_gradient: texture_2d<f32>;
@group(0) @binding(9) var ninja_underwater_volume: texture_3d<f32>;
// The JKA flat-volume bounded ray length in water (metres; 0 = dry), for
// surfaces without a Blue Water probe.
@group(0) @binding(10) var water_path: texture_2d<f32>;

const RIVER_CM_PER_UNIT: f32 = 2.5;
// M_UnderWaterPostProcess distance curve (ninja_underwater.rs DISTANCE_*).
const NINJA_DISTANCE_EXPONENT: f32 = 0.305;
const NINJA_DISTANCE_OFFSET: f32 = -0.8;
// Wave allowance when classifying surface pixels (cm).
const NINJA_SURFACE_MARGIN_CM: f32 = 250.0;

fn river_scene_depth(device_z: f32) -> f32 {
    let near = river_view.forward.w;
    let far = river_view.eye.w;
    return near * far / (device_z * (far - near) + near) * RIVER_CM_PER_UNIT;
}

fn river_underwater_ray(uv: vec2<f32>) -> vec3<f32> {
    let ndc = uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
    return river_view.forward.xyz + river_view.right.xyz * ndc.x / river_view.distortion.x
        + river_view.up.xyz * ndc.y / (river_view.distortion.x * river_view.distortion.y);
}

struct NinjaOceanOut {
    @builtin(position) clip: vec4<f32>,
};

@vertex fn vs_ninja_ocean_underwater(@builtin(vertex_index) index: u32) -> NinjaOceanOut {
    let xy = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: NinjaOceanOut;
    out.clip = vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

// Same body as the river's Ninja branch of fs_submerged, gated per pixel by
// the probe (or water path) so a camera straddling the waterline keeps its dry half.
@fragment fn fs_ninja_ocean_underwater(input: NinjaOceanOut) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(scene_depth));
    let uv = input.clip.xy / size;
    let pixel = vec2<i32>(input.clip.xy);
    let scene = textureLoad(scene_color, pixel, 0);
    let probe_texel = textureSampleLevel(depth_probe, normal_sampler, uv, 0.0);
    let probe = probe_texel.r;
    let path = textureLoad(water_path, min(pixel, vec2<i32>(textureDimensions(water_path)) - 1), 0).r;
    if select(probe, path, river_view.mask.x > 0.5) <= 0.0 {
        return scene;
    }
    let distorted_uv = clamp(uv + ninja_underwater_distortion(uv, river_view.right.w),
        0.5 / size, 1.0 - 0.5 / size);
    let distorted_pixel = vec2<i32>(distorted_uv * size);
    let distorted_scene = textureSampleLevel(scene_color, normal_sampler, distorted_uv, 0.0).rgb;
    let distorted_depth = river_scene_depth(textureLoad(scene_depth, distorted_pixel, 0));
    var color = ninja_underwater_color(distorted_scene, distorted_depth, uv, pixel);
    let ray = river_underwater_ray(distorted_uv);
    // Host: the water surface seen from below (Snell's window and the
    // reflecting underside) carries light that crossed only the water path to
    // it. The look's grade (CloseColor add, channel tint) clipped the sky in
    // the window to flat cyan, so surface pixels take
    // the distance fog alone: the scene attenuated by the path, plus the look's
    // own pure-fog colour (the same material over a black scene). Surface
    // pixels: at least the eye's depth below the waves (probe .g, metres) above
    // the eye, less a wave margin. Probe mask only (the Blue Water surfaces).
    let eye_depth_cm = probe_texel.g * 100.0;
    let rise_cm = distorted_depth * ray.z;
    let surface = select(0.0, smoothstep(eye_depth_cm - NINJA_SURFACE_MARGIN_CM, eye_depth_cm - 0.4 * NINJA_SURFACE_MARGIN_CM,
        rise_cm), river_view.mask.x < 0.5 && eye_depth_cm > 0.0);
    let radial = distorted_depth * length(ray);
    let fade = smoothstep(river_view.mask.y, river_view.mask.z, radial);
    if surface > 0.0 || fade > 0.0 {
        let fog = ninja_underwater_color(vec3<f32>(0.0), distorted_depth, uv, pixel);
        if surface > 0.0 {
            let fogged = saturate(pow(max(distorted_depth, 1.0), NINJA_DISTANCE_EXPONENT) * authored.ninja_optics.x
                + NINJA_DISTANCE_OFFSET);
            color = mix(color, distorted_scene * (1.0 - fogged) + fog, surface);
        }
        // Host fade (not in M_UnderWaterPostProcess, whose CloseColor term keeps
        // the scene visible at any depth): past full DistanceColor the scene
        // blends into the pure-fog colour, so geometry beyond mask.z (radial cm)
        // is invisible and the underwater cull distance stops there without popping.
        color = mix(color, fog, fade);
    }
    return vec4<f32>(color, scene.a);
}

