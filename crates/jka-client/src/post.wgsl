@group(0) @binding(0) var scene_texture: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;
@group(0) @binding(2) var<uniform> settings: PostSettings;
@group(0) @binding(3) var linear_depth_texture: texture_2d<f32>;
@group(0) @binding(4) var history_texture: texture_2d<f32>;
@group(0) @binding(5) var<storage, read> integrated_fog: array<vec4<f32>>;
@group(0) @binding(6) var bloom_half_texture: texture_2d<f32>;
@group(0) @binding(7) var bloom_quarter_texture: texture_2d<f32>;
@group(0) @binding(8) var bloom_eighth_texture: texture_2d<f32>;
@group(0) @binding(9) var color_lut_texture: texture_3d<f32>;
@group(0) @binding(10) var color_lut_sampler: sampler;
@group(0) @binding(11) var ssao_history_texture: texture_2d<f32>;
@group(0) @binding(12) var ssr_history_texture: texture_2d<f32>;
@group(0) @binding(13) var ssr_depth_texture: texture_2d<f32>;
@group(0) @binding(14) var dof_horizontal_texture: texture_2d<f32>;
@group(0) @binding(15) var cloud_detail_texture: texture_3d<f32>;
@group(0) @binding(16) var cloud_noise_sampler: sampler;
@group(0) @binding(17) var cloud_transfer_texture: texture_2d<f32>;
@group(0) @binding(18) var rain_occlusion_height: texture_2d<f32>;
@group(0) @binding(19) var rain_haze_mask: texture_2d<f32>;
@group(0) @binding(20) var cloud_weather_texture: texture_2d<f32>;
@group(0) @binding(21) var taa_motion_vectors: texture_2d<f32>;
@group(0) @binding(22) var taa_depth: texture_depth_2d;
@group(0) @binding(23) var reflection_policy_texture: texture_2d<f32>;
struct AutoExposureState {
    exposure_ev: f32,
    average_luminance: f32,
    target_ev: f32,
    reserved: f32,
};
@group(0) @binding(24) var<storage, read> auto_exposure_state: AutoExposureState;
@group(0) @binding(25) var ssr_visibility_texture: texture_2d<f32>;
// Sparse march output, read only by fs_cloud_resolve. It lives in its own bind
// group because the march pipeline renders into this texture and so must not
// have it bound; per-entry-point reachability keeps group 1 out of that
// pipeline layout entirely.
@group(1) @binding(0) var cloud_march_texture: texture_2d<f32>;

const FROXEL_X: u32 = 80u;
const FROXEL_Y: u32 = 45u;
const FROXEL_Z: u32 = 48u;
const FROXEL_NEAR: f32 = 4.0;
const FROXEL_FAR: f32 = 16384.0;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(3.0, 1.0)
    );
    let p = positions[vertex_index];
    var out: VertexOut;
    out.position = vec4<f32>(p, 0.0, 1.0);
    out.uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    return out;
}

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn aces_fitted(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp(
        (x * (a * x + vec3<f32>(b))) / (x * (c * x + vec3<f32>(d)) + vec3<f32>(e)),
        vec3<f32>(0.0),
        vec3<f32>(1.0)
    );
}

fn scene(uv: vec2<f32>) -> vec3<f32> {
    return textureSample(
        scene_texture,
        scene_sampler,
        clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0))
    ).rgb;
}
fn display_scene(uv: vec2<f32>) -> vec3<f32> {
    var color = scene(uv) * exp2(settings.grain.w + auto_exposure_state.exposure_ev);
    if (settings.color.y > 0.5) {
        color = aces_fitted(color);
    }
    return pow(max(color, vec3<f32>(0.0)), vec3<f32>(1.0 / max(settings.color.x, 0.05)));
}

fn purple_fringe_color(uv: vec2<f32>, color: vec3<f32>) -> vec3<f32> {
    // Highlight/lens fringing rather than the usual radial RGB split. Keep this
    // deliberately optional: it costs two scene samples only when enabled.
    let strength = max(settings.film.y, 0.0);
    if (strength <= 0.001) {
        return color;
    }

    let luma = luminance(color);
    let gradient = vec2<f32>(dpdx(luma), dpdy(luma));
    let edge = length(gradient);
    let edge_dir = gradient / max(edge, 0.00001);
    let texel = vec2<f32>(
        1.0 / max(settings.aa.y, 1.0),
        1.0 / max(settings.aa.z, 1.0)
    );

    // Two low-cost scales turn the old hard one-pixel outline into a narrow,
    // fading halo. Normal strengths remain around a pixel; deliberately huge
    // console values still widen the effect for debugging.
    let near_radius_px = 0.55 + strength * 0.020;
    let far_radius_px = 1.45 + strength * 0.050;
    let near_color = display_scene(uv + edge_dir * texel * near_radius_px);
    let far_color = display_scene(uv + edge_dir * texel * far_radius_px);
    let near_luma = luminance(near_color);
    let far_luma = luminance(far_color);
    let near_delta = max(near_luma - luma, 0.0);
    let far_delta = max(far_luma - luma, 0.0);
    let bright_luma = max(near_luma, far_luma);

    // Broad smooth gates avoid the binary-looking edge threshold from the first
    // version. The far contribution is weaker, so the fringe naturally decays
    // away from the high-contrast boundary instead of reading as an outline.
    let spill = near_delta * 0.72 + far_delta * 0.28;
    let edge_gate = smoothstep(0.005, 0.080, max(edge, spill));
    let highlight_gate = smoothstep(0.38, 0.92, bright_luma);
    let dark_side_gate = smoothstep(0.003, 0.20, spill);
    let fringe = edge_gate * highlight_gate * dark_side_gate * strength * 0.060;

    // Much cooler than magenta: blue-violet with only enough red to keep it from
    // reading as a pure blue glow. Do not subtract green; that was pushing the
    // previous version toward hot pink.
    let violet = vec3<f32>(0.16, 0.035, 0.92);
    return color + violet * fringe;
}
// Wet-weather grade: a faint cool cast in the shadows and mids, like the blue-grey
// of rain-washed air, a little more contrast, and extra pop only for colours that
// are already vivid (neon, signs, lit windows). It never invents a warm colour:
// any red or orange in the picture has to come from the lights themselves.
// `amount` is the wet-weather strength, already faded by how exposed the camera is
// to the rain, so a dry scene or a sheltered camera is untouched. Display-referred,
// so it runs after tone mapping/gamma and before the user's LUT.
fn apply_rain_grade(color: vec3<f32>, amount: f32) -> vec3<f32> {
    if (amount <= 0.001) {
        return color;
    }
    let luma = luminance(color);
    // Contrast about a dark mid-grey deepens the blacks a little.
    var graded = max((color - vec3<f32>(0.40)) * (1.0 + 0.10 * amount) + vec3<f32>(0.40), vec3<f32>(0.0));
    // Cool cast, strongest in the shadows and fading out toward the highlights.
    let coolness = 1.0 - smoothstep(0.30, 0.95, luma);
    graded += vec3<f32>(-0.006, 0.004, 0.016) * (coolness * amount);
    // Vividness: only genuinely saturated light gets a boost.
    let chroma = max(graded.r, max(graded.g, graded.b)) - min(graded.r, min(graded.g, graded.b));
    let vivid = smoothstep(0.55, 0.85, chroma) * smoothstep(0.20, 0.50, luma);
    graded = mix(vec3<f32>(luminance(graded)), graded, 1.0 + 0.25 * amount * vivid);
    return clamp(graded, vec3<f32>(0.0), vec3<f32>(1.0));
}

const COLOR_LUT_SIZE: f32 = 33.0;

fn apply_color_lut(color: vec3<f32>) -> vec3<f32> {
    let strength = clamp(settings.film.w, 0.0, 1.0);
    if (strength <= 0.001) {
        return color;
    }
    let input = clamp(color, vec3<f32>(0.0), vec3<f32>(1.0));
    // Sample texel centres so 0 and 1 map exactly to the endpoints of the
    // baked 33^3 cube while hardware trilinear filtering handles interpolation.
    let coord = (input * (COLOR_LUT_SIZE - 1.0) + vec3<f32>(0.5)) / COLOR_LUT_SIZE;
    let graded = textureSample(color_lut_texture, color_lut_sampler, coord).rgb;
    return mix(color, graded, strength);
}

fn pcg_hash(input: u32) -> u32 {
    let state = input * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn grain_hash(cell: vec2<u32>, frame: u32, salt: u32) -> f32 {
    let seed = cell.x * 1597334677u
        ^ cell.y * 3812015801u
        ^ frame * 2798796415u
        ^ salt;
    return f32(pcg_hash(seed)) * (1.0 / 4294967295.0);
}

fn film_grain(color: vec3<f32>, uv: vec2<f32>) -> vec3<f32> {
    let strength = clamp(settings.grain.x, 0.0, 1.0);
    if (strength <= 0.001) {
        return color;
    }

    // Keep grain tied to output pixels instead of world/source resolution. A
    // discrete film-frame seed prevents the pattern from turning into high-rate
    // electronic noise when the renderer is running at thousands of FPS.
    let viewport = max(settings.aa.yz, vec2<f32>(1.0));
    let size = max(settings.grain.y, 0.5);
    let pixel = floor(uv * viewport / size);
    let cell = vec2<u32>(pixel);
    let coarse_cell = vec2<u32>(floor(pixel * 0.47));
    let film_frame = u32(floor(max(settings.grain.z, 0.0) * 24.0));

    // Cheap band-pass grain: subtract a lower-frequency component instead of
    // adding it. This avoids the cloudy/blotchy low-frequency noise common to
    // game grain while retaining a visible grain size with only a few integer
    // hashes and no grain texture/sample.
    let fine = grain_hash(cell, film_frame, 0x68bc21ebu) - 0.5;
    let coarse = grain_hash(coarse_cell, film_frame, 0x02e5be93u) - 0.5;
    let band = (fine - coarse * 0.58) * 1.28;

    // Real colour-film dye layers do not share perfectly identical grain. Add a
    // restrained decorrelated chroma component while keeping most energy in
    // luminance so it never turns into RGB television noise.
    let dye = grain_hash(cell, film_frame, 0x967a889bu) - 0.5;
    let grain_rgb = vec3<f32>(
        band + dye * 0.075,
        band - dye * 0.035,
        band + dye * 0.095
    );

    // Density-weighted approximation: grain becomes more apparent as image
    // density rises (toward shadows), then rolls away near paper-white/highlights.
    // This intentionally avoids per-channel log/exp density conversion to keep
    // the full-screen effect cheap enough for the high-FPS renderer.
    let luma = clamp(luminance(color), 0.0, 1.0);
    let density = sqrt(max(1.0 - luma, 0.0));
    let highlight_rolloff = 1.0 - smoothstep(0.72, 0.985, luma);
    let black_guard = mix(0.74, 1.0, smoothstep(0.01, 0.10, luma));
    let amount = strength * 0.18 * density * highlight_rolloff * black_guard;

    return max(color + grain_rgb * amount, vec3<f32>(0.0));
}

fn depth_at_pixel(pixel: vec2<i32>) -> f32 {
    let dims = textureDimensions(linear_depth_texture);
    let max_pixel = vec2<i32>(i32(dims.x) - 1, i32(dims.y) - 1);
    return textureLoad(linear_depth_texture, clamp(pixel, vec2<i32>(0), max_pixel), 0).x;
}

fn depth_at_uv(uv: vec2<f32>) -> f32 {
    let dims = textureDimensions(linear_depth_texture);
    let dims_f = vec2<f32>(f32(dims.x), f32(dims.y));
    let pixel = vec2<i32>(
        i32(clamp(uv.x, 0.0, 0.999999) * dims_f.x),
        i32(clamp(uv.y, 0.0, 0.999999) * dims_f.y)
    );
    return depth_at_pixel(pixel);
}

fn valid_depth(depth: f32) -> bool {
    return depth > 0.0 && depth < 999999.0;
}

fn world_ray(uv: vec2<f32>) -> vec3<f32> {
    // Camera depth is reversed-Z (near -> 1, far -> 0). Unproject on the FAR
    // plane: at z = 1 the point is ~1 unit from the camera, so subtracting the
    // camera position (map coordinates in the thousands, f32 spacing ~5e-4)
    // left ~1e-3 rad of per-pixel direction noise. That noise is ~0.5 world
    // units at 500 units of depth, the same size as a one-pixel tangent, so
    // depth-derived normals (contact shadows, puddles) flickered.
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    var world_far = settings.inv_view_proj * ndc;
    world_far = world_far / max(abs(world_far.w), 1e-6);
    return normalize(world_far.xyz - settings.camera_pos_time.xyz);
}

fn cloud_world_ray(uv: vec2<f32>) -> vec3<f32> {
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 1.0, 1.0);
    var world_far = settings.cloud_inv_view_proj * ndc;
    world_far = world_far / max(abs(world_far.w), 1e-6);
    return normalize(world_far.xyz - settings.camera_pos_time.xyz);
}

fn world_position(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    return settings.camera_pos_time.xyz + world_ray(uv) * depth;
}

fn srgb_to_linear_channel(c: f32) -> f32 {
    let value = clamp(c, 0.0, 1.0);
    return select(
        pow((value + 0.055) / 1.055, 2.4),
        value / 12.92,
        value <= 0.04045
    );
}

fn linear_to_srgb_channel(c: f32) -> f32 {
    let value = max(c, 0.0);
    return select(
        1.055 * pow(value, 1.0 / 2.4) - 0.055,
        12.92 * value,
        value <= 0.0031308
    );
}

fn srgb_to_linear(color: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        srgb_to_linear_channel(color.r),
        srgb_to_linear_channel(color.g),
        srgb_to_linear_channel(color.b)
    );
}

fn linear_to_srgb(color: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        linear_to_srgb_channel(color.r),
        linear_to_srgb_channel(color.g),
        linear_to_srgb_channel(color.b)
    );
}

fn openjk_fog_texel_alpha(texel_x: f32) -> f32 {
    let x = clamp(texel_x, 0.0, 255.0);
    let table_index = floor(clamp(x / 32.0, 0.0, 1.0) * 255.0);
    let table_value = sqrt(table_index / 255.0);
    return floor(table_value * 255.0) / 255.0;
}

fn openjk_global_fog_alpha(forward_normalized: f32) -> f32 {
    // Matches the filtered 256x32 *fog texture used by RB_FogPass. For the
    // global row, OpenJK's +1/512 S bias cancels the texture half-texel.
    let texel = max(forward_normalized, 0.0) * 32.0;
    let x0 = floor(texel);
    let frac_x = fract(texel);
    return mix(
        openjk_fog_texel_alpha(x0),
        openjk_fog_texel_alpha(x0 + 1.0),
        frac_x
    );
}

fn policy_bits(pixel: vec2<i32>) -> u32 {
    let dims = textureDimensions(reflection_policy_texture);
    let maximum = vec2<i32>(i32(dims.x) - 1, i32(dims.y) - 1);
    let policy = textureLoad(
        reflection_policy_texture,
        clamp(pixel, vec2<i32>(0), maximum),
        0
    );
    return u32(round(clamp(policy.a, 0.0, 1.0) * 3.0));
}

fn ssr_receiver_at_pixel(pixel: vec2<i32>) -> bool {
    let dims = textureDimensions(reflection_policy_texture);
    let maximum = vec2<i32>(i32(dims.x) - 1, i32(dims.y) - 1);
    return textureLoad(reflection_policy_texture, clamp(pixel, vec2<i32>(0), maximum), 0).r >= 0.5;
}

fn apply_legacy1_global_fog(
    color: vec3<f32>,
    uv: vec2<f32>,
    pixel: vec2<i32>,
    radial_depth: f32
) -> vec3<f32> {
    let scale = settings.taa_params.w;
    let depth_for_opaque = settings.legacy_fog.w;
    if (scale <= 0.0 || depth_for_opaque <= 0.001 || !valid_depth(radial_depth)) {
        return color;
    }
    // Bit 1 says the frontmost opaque surface participates in global fog. The
    // prepass writes it for fogged BSP surfaces (not q3map_nofog/sky) and for
    // every depth-writing entity, which OpenJK assigns the world's global fog.
    // radial_depth already is that surface's own depth, entity or BSP.
    if ((policy_bits(pixel) & 2u) == 0u) {
        return color;
    }

    let ray = world_ray(uv);
    let camera_forward = world_ray(vec2<f32>(0.5));
    let forward_distance = radial_depth * max(dot(ray, camera_forward), 0.0);
    let amount = openjk_global_fog_alpha(
        (forward_distance / depth_for_opaque) * scale
    );

    // OpenJK's RB_FogPass blended into the legacy gamma-encoded LDR
    // framebuffer. WGPU sRGB attachments blend in linear space, which made
    // the same alpha look much denser in dark interiors. Emulate that old
    // framebuffer blend explicitly, then return to linear for the sRGB target.
    let encoded_scene = linear_to_srgb(color);
    let encoded_fog = clamp(settings.legacy_fog.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    return srgb_to_linear(mix(encoded_scene, encoded_fog, amount));
}

fn camera_motion_blur(uv: vec2<f32>, color: vec3<f32>, depth: f32) -> vec3<f32> {
    let shutter_scale = max(settings.camera_fx.x, 0.0);
    if (shutter_scale <= 0.001 || !valid_depth(depth)) {
        return color;
    }

    // MiniEngine-style camera reprojection: reconstruct the static world point
    // from linear depth, project it through the previous camera, then integrate
    // along that screen-space velocity. The CPU scales the per-frame vector by
    // a fixed exposure interval so 2000+ FPS does not make the blur disappear.
    let world = world_position(uv, depth);
    let prev_clip = settings.motion_prev_view_proj * vec4<f32>(world, 1.0);
    if (prev_clip.w <= 1e-5) {
        return color;
    }
    let prev_ndc = prev_clip.xy / prev_clip.w;
    let prev_uv = vec2<f32>(prev_ndc.x * 0.5 + 0.5, 0.5 - prev_ndc.y * 0.5);
    if (any(prev_uv < vec2<f32>(-0.10)) || any(prev_uv > vec2<f32>(1.10))) {
        return color;
    }

    let viewport = max(settings.aa.yz, vec2<f32>(1.0));
    var motion_px = (prev_uv - uv) * viewport * shutter_scale;
    let length_px = length(motion_px);
    if (length_px < 0.35) {
        return color;
    }
    motion_px *= min(1.0, 40.0 / max(length_px, 0.0001));
    let motion_uv = motion_px / viewport;

    // Bevy's common quality setting uses two samples in each direction plus the
    // centre (five total). Use the same sample-count shape without allocating a
    // dedicated velocity texture for static BSP/camera motion.
    let a = scene(uv - motion_uv * 0.50);
    let b = scene(uv - motion_uv * 0.25);
    let c = scene(uv + motion_uv * 0.25);
    let d = scene(uv + motion_uv * 0.50);
    return (a + d) * 0.12 + (b + c) * 0.23 + color * 0.30;
}

fn dof_radius_px(depth: f32) -> f32 {
    let strength = clamp(settings.camera_fx.y, 0.0, 1.0);
    if (strength <= 0.001 || !valid_depth(depth)) {
        return 0.0;
    }
    let focus = max(settings.camera_fx.z, 32.0);
    let relative_defocus = abs(depth - focus) / max(depth, 32.0);
    let max_radius = mix(2.0, 14.0, strength);
    return smoothstep(0.015, 0.95, relative_defocus) * max_radius;
}

fn dof_use_high_quality(radius: f32) -> bool {
    let quality = settings.camera_fx.w;
    return quality > 1.5 || (quality > 0.5 && radius >= 6.0);
}

fn depth_of_field(uv: vec2<f32>, color: vec3<f32>, depth: f32) -> vec3<f32> {
    let radius = dof_radius_px(depth);
    if (radius < 0.20) {
        return color;
    }

    // Second half of the separable Gaussian. Adaptive mode follows the CoC:
    // five bilinear samples for modest blur, nine only when wide CoC would
    // otherwise reveal the sample spacing.
    let texel_y = 1.0 / max(settings.aa.z, 1.0);
    let center = textureSample(dof_horizontal_texture, scene_sampler, uv).rgb;
    var blurred: vec3<f32>;
    if (dof_use_high_quality(radius)) {
        let scale = radius / 7.30294072;
        let o1 = 1.45842952 * scale * texel_y;
        let o2 = 3.40398481 * scale * texel_y;
        let o3 = 5.35180578 * scale * texel_y;
        let o4 = 7.30294072 * scale * texel_y;
        blurred = center * 0.13357122;
        blurred += (textureSample(dof_horizontal_texture, scene_sampler, uv + vec2<f32>(0.0, o1)).rgb + textureSample(dof_horizontal_texture, scene_sampler, uv - vec2<f32>(0.0, o1)).rgb) * 0.23330843;
        blurred += (textureSample(dof_horizontal_texture, scene_sampler, uv + vec2<f32>(0.0, o2)).rgb + textureSample(dof_horizontal_texture, scene_sampler, uv - vec2<f32>(0.0, o2)).rgb) * 0.13592781;
        blurred += (textureSample(dof_horizontal_texture, scene_sampler, uv + vec2<f32>(0.0, o3)).rgb + textureSample(dof_horizontal_texture, scene_sampler, uv - vec2<f32>(0.0, o3)).rgb) * 0.05138318;
        blurred += (textureSample(dof_horizontal_texture, scene_sampler, uv + vec2<f32>(0.0, o4)).rgb + textureSample(dof_horizontal_texture, scene_sampler, uv - vec2<f32>(0.0, o4)).rgb) * 0.01259497;
    } else {
        let scale = radius / 3.23076923;
        let o1 = 1.38461538 * scale * texel_y;
        let o2 = 3.23076923 * scale * texel_y;
        blurred = center * 0.22702703;
        blurred += (textureSample(dof_horizontal_texture, scene_sampler, uv + vec2<f32>(0.0, o1)).rgb + textureSample(dof_horizontal_texture, scene_sampler, uv - vec2<f32>(0.0, o1)).rgb) * 0.31621622;
        blurred += (textureSample(dof_horizontal_texture, scene_sampler, uv + vec2<f32>(0.0, o2)).rgb + textureSample(dof_horizontal_texture, scene_sampler, uv - vec2<f32>(0.0, o2)).rgb) * 0.07027027;
    }
    return mix(color, blurred, smoothstep(0.20, 1.25, radius));
}

fn project_uv(world: vec3<f32>) -> vec3<f32> {
    let clip = settings.view_proj * vec4<f32>(world, 1.0);
    if (clip.w <= 1e-5) {
        return vec3<f32>(-10.0, -10.0, -1.0);
    }
    let ndc = clip.xyz / clip.w;
    return vec3<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5, ndc.z);
}

fn surface_normal(pixel: vec2<i32>) -> vec3<f32> {
    let dims = textureDimensions(linear_depth_texture);
    let dims_f = vec2<f32>(f32(dims.x), f32(dims.y));
    let d0 = depth_at_pixel(pixel);
    if (!valid_depth(d0)) {
        return vec3<f32>(0.0, 1.0, 0.0);
    }
    let uv0 = (vec2<f32>(pixel) + vec2<f32>(0.5)) / dims_f;
    let px = pixel + vec2<i32>(1, 0);
    let py = pixel + vec2<i32>(0, 1);
    let dx_depth = depth_at_pixel(px);
    let dy_depth = depth_at_pixel(py);
    if (!valid_depth(dx_depth) || !valid_depth(dy_depth)) {
        return normalize(-world_ray(uv0));
    }
    let uvx = (vec2<f32>(px) + vec2<f32>(0.5)) / dims_f;
    let uvy = (vec2<f32>(py) + vec2<f32>(0.5)) / dims_f;
    // Camera-relative positions: adding camera_pos (map coordinates in the
    // thousands) before subtracting quantises the one-pixel tangents to the f32
    // spacing there, which stair-steps the normal across small surfaces.
    let r0 = world_ray(uv0) * d0;
    let r1 = world_ray(uvx) * dx_depth;
    let r2 = world_ray(uvy) * dy_depth;
    var n = normalize(cross(r1 - r0, r2 - r0));
    if (dot(n, -r0) < 0.0) {
        n = -n;
    }
    return n;
}

const TAA_DEFAULT_HISTORY_BLEND_RATE: f32 = 0.1;
const TAA_MIN_HISTORY_BLEND_RATE: f32 = 0.015;

fn taa_rcp(x: f32) -> f32 {
    return 1.0 / x;
}

fn taa_max3(x: vec3<f32>) -> f32 {
    return max(x.r, max(x.g, x.b));
}

fn taa_tonemap(color: vec3<f32>) -> vec3<f32> {
    return color * taa_rcp(taa_max3(color) + 1.0);
}

fn taa_reverse_tonemap(color: vec3<f32>) -> vec3<f32> {
    return color * taa_rcp(1.0 - taa_max3(color));
}

// Playdead YCoCg helpers used by Bevy's TAA resolve.
fn taa_rgb_to_ycocg(rgb: vec3<f32>) -> vec3<f32> {
    let y = (rgb.r / 4.0) + (rgb.g / 2.0) + (rgb.b / 4.0);
    let co = (rgb.r / 2.0) - (rgb.b / 2.0);
    let cg = (-rgb.r / 4.0) + (rgb.g / 2.0) - (rgb.b / 4.0);
    return vec3<f32>(y, co, cg);
}

fn taa_ycocg_to_rgb(ycocg: vec3<f32>) -> vec3<f32> {
    let r = ycocg.x + ycocg.y - ycocg.z;
    let g = ycocg.x + ycocg.z;
    let b = ycocg.x - ycocg.y - ycocg.z;
    return clamp(vec3<f32>(r, g, b), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn taa_clip_towards_aabb_center(
    history_color: vec3<f32>,
    current_color: vec3<f32>,
    aabb_min: vec3<f32>,
    aabb_max: vec3<f32>
) -> vec3<f32> {
    let p_clip = 0.5 * (aabb_max + aabb_min);
    let e_clip = 0.5 * (aabb_max - aabb_min) + vec3<f32>(0.00000001);
    let v_clip = history_color - p_clip;
    let v_unit = v_clip / e_clip;
    let a_unit = abs(v_unit);
    let ma_unit = taa_max3(a_unit);
    if (ma_unit > 1.0) {
        return p_clip + (v_clip / ma_unit);
    }
    return history_color;
}

fn taa_pixel_from_uv(uv: vec2<f32>, dims: vec2<u32>) -> vec2<i32> {
    let p = vec2<i32>(vec2<f32>(dims) * clamp(uv, vec2<f32>(0.0), vec2<f32>(0.99999994)));
    return clamp(p, vec2<i32>(0), vec2<i32>(dims) - vec2<i32>(1));
}

fn taa_sample_current_nearest(uv: vec2<f32>) -> vec3<f32> {
    let dims = textureDimensions(scene_texture);
    var sample = textureLoad(scene_texture, taa_pixel_from_uv(uv, dims), 0).rgb;
    if (settings.taa_params.x > 0.5) {
        sample = taa_tonemap(sample);
    }
    return taa_rgb_to_ycocg(sample);
}

fn taa_sample_history_linear(u: f32, v: f32) -> vec3<f32> {
    return textureSample(history_texture, scene_sampler, vec2<f32>(u, v)).rgb;
}

fn taa_depth_nearest(uv: vec2<f32>) -> f32 {
    let dims = textureDimensions(taa_depth);
    return textureLoad(taa_depth, taa_pixel_from_uv(uv, dims), 0);
}

fn taa_motion_nearest(uv: vec2<f32>) -> vec2<f32> {
    let dims = textureDimensions(taa_motion_vectors);
    return textureLoad(taa_motion_vectors, taa_pixel_from_uv(uv, dims), 0).rg;
}

fn taa_history_confidence_nearest(uv: vec2<f32>) -> f32 {
    let dims = textureDimensions(history_texture);
    return textureLoad(history_texture, taa_pixel_from_uv(uv, dims), 0).a;
}

struct TaaResolve {
    color: vec3<f32>,
    history_confidence: f32,
};

fn taa_resolve_bevy(uv: vec2<f32>) -> TaaResolve {
    let texture_size = vec2<f32>(textureDimensions(scene_texture));
    let texel_size = 1.0 / texture_size;
    var current_color = textureLoad(
        scene_texture,
        taa_pixel_from_uv(uv, textureDimensions(scene_texture)),
        0
    ).rgb;
    if (settings.taa_params.x > 0.5) {
        current_color = taa_tonemap(current_color);
    }

    var out: TaaResolve;

    // Bevy's RESET specialization: current frame becomes history immediately,
    // with maximum stationary confidence for the next frame.
    if (settings.scene.w <= 0.5) {
        out.color = current_color;
        out.history_confidence = 1.0 / TAA_MIN_HISTORY_BLEND_RATE;
        return out;
    }

    // Bevy: pick the closest motion vector from the centre plus four diagonal
    // samples two texels away. Reversed-Z means the greatest depth is closest.
    let offset = texel_size * 2.0;
    let d_uv_tl = uv + vec2<f32>(-offset.x, offset.y);
    let d_uv_tr = uv + vec2<f32>( offset.x, offset.y);
    let d_uv_bl = uv + vec2<f32>(-offset.x,-offset.y);
    let d_uv_br = uv + vec2<f32>( offset.x,-offset.y);
    var closest_uv = uv;
    let d_tl = taa_depth_nearest(d_uv_tl);
    let d_tr = taa_depth_nearest(d_uv_tr);
    var closest_depth = taa_depth_nearest(uv);
    let d_bl = taa_depth_nearest(d_uv_bl);
    let d_br = taa_depth_nearest(d_uv_br);
    if (d_tl > closest_depth) {
        closest_uv = d_uv_tl;
        closest_depth = d_tl;
    }
    if (d_tr > closest_depth) {
        closest_uv = d_uv_tr;
        closest_depth = d_tr;
    }
    if (d_bl > closest_depth) {
        closest_uv = d_uv_bl;
        closest_depth = d_bl;
    }
    if (d_br > closest_depth) {
        closest_uv = d_uv_br;
    }
    let closest_motion_vector = taa_motion_nearest(closest_uv);
    let history_uv = uv - closest_motion_vector;

    // Bevy's five-tap Catmull-Rom history reconstruction (corner taps omitted).
    let sample_position = history_uv * texture_size;
    let texel_center = floor(sample_position - 0.5) + 0.5;
    let f = sample_position - texel_center;
    let w0 = f * (-0.5 + f * (1.0 - 0.5 * f));
    let w1 = 1.0 + f * f * (-2.5 + 1.5 * f);
    let w2 = f * (0.5 + f * (2.0 - 1.5 * f));
    let w3 = f * f * (-0.5 + 0.5 * f);
    let w12 = w1 + w2;
    let texel_position_0 = (texel_center - 1.0) * texel_size;
    let texel_position_3 = (texel_center + 2.0) * texel_size;
    let texel_position_12 = (texel_center + (w2 / w12)) * texel_size;
    var history_color = taa_sample_history_linear(texel_position_12.x, texel_position_0.y) * w12.x * w0.y;
    history_color += taa_sample_history_linear(texel_position_0.x, texel_position_12.y) * w0.x * w12.y;
    history_color += taa_sample_history_linear(texel_position_12.x, texel_position_12.y) * w12.x * w12.y;
    history_color += taa_sample_history_linear(texel_position_3.x, texel_position_12.y) * w3.x * w12.y;
    history_color += taa_sample_history_linear(texel_position_12.x, texel_position_3.y) * w12.x * w3.y;

    // Bevy's 3x3 YCoCg variance clipping.
    let s_tl = taa_sample_current_nearest(uv + vec2<f32>(-texel_size.x,  texel_size.y));
    let s_tm = taa_sample_current_nearest(uv + vec2<f32>( 0.0,           texel_size.y));
    let s_tr = taa_sample_current_nearest(uv + vec2<f32>( texel_size.x,  texel_size.y));
    let s_ml = taa_sample_current_nearest(uv + vec2<f32>(-texel_size.x,  0.0));
    let s_mm = taa_rgb_to_ycocg(current_color);
    let s_mr = taa_sample_current_nearest(uv + vec2<f32>( texel_size.x,  0.0));
    let s_bl = taa_sample_current_nearest(uv + vec2<f32>(-texel_size.x, -texel_size.y));
    let s_bm = taa_sample_current_nearest(uv + vec2<f32>( 0.0,          -texel_size.y));
    let s_br = taa_sample_current_nearest(uv + vec2<f32>( texel_size.x, -texel_size.y));
    let moment_1 = s_tl + s_tm + s_tr + s_ml + s_mm + s_mr + s_bl + s_bm + s_br;
    let moment_2 = (s_tl * s_tl) + (s_tm * s_tm) + (s_tr * s_tr) +
        (s_ml * s_ml) + (s_mm * s_mm) + (s_mr * s_mr) +
        (s_bl * s_bl) + (s_bm * s_bm) + (s_br * s_br);
    let mean = moment_1 / 9.0;
    let variance = (moment_2 / 9.0) - (mean * mean);
    let std_deviation = sqrt(max(variance, vec3<f32>(0.0)));
    history_color = taa_rgb_to_ycocg(history_color);
    history_color = taa_clip_towards_aabb_center(
        history_color,
        s_mm,
        mean - std_deviation,
        mean + std_deviation
    );
    history_color = taa_ycocg_to_rgb(history_color);

    var history_confidence = taa_history_confidence_nearest(uv);
    let pixel_motion_vector = abs(closest_motion_vector) * texture_size;
    if (pixel_motion_vector.x < 0.01 && pixel_motion_vector.y < 0.01) {
        history_confidence += 10.0;
    } else {
        history_confidence = 1.0;
    }

    var current_color_factor = clamp(
        1.0 / history_confidence,
        TAA_MIN_HISTORY_BLEND_RATE,
        TAA_DEFAULT_HISTORY_BLEND_RATE
    );

    if (any(clamp(history_uv, vec2<f32>(0.0), vec2<f32>(1.0)) != history_uv)) {
        current_color_factor = 1.0;
        history_confidence = 1.0;
    }

    current_color = mix(history_color, current_color, current_color_factor);
    out.color = current_color;
    out.history_confidence = history_confidence;
    return out;
}

fn taa_display_color(color: vec3<f32>) -> vec3<f32> {
    if (settings.taa_params.x > 0.5) {
        return taa_reverse_tonemap(color);
    }
    return color;
}

fn fxaa_color(uv: vec2<f32>, texel: vec2<f32>) -> vec3<f32> {

    let rgb_m = scene(uv);
    let rgb_nw = scene(uv + texel * vec2<f32>(-1.0, -1.0));
    let rgb_ne = scene(uv + texel * vec2<f32>( 1.0, -1.0));
    let rgb_sw = scene(uv + texel * vec2<f32>(-1.0,  1.0));
    let rgb_se = scene(uv + texel * vec2<f32>( 1.0,  1.0));

    let luma_m = luminance(rgb_m);
    let luma_nw = luminance(rgb_nw);
    let luma_ne = luminance(rgb_ne);
    let luma_sw = luminance(rgb_sw);
    let luma_se = luminance(rgb_se);
    let luma_min = min(luma_m, min(min(luma_nw, luma_ne), min(luma_sw, luma_se)));
    let luma_max = max(luma_m, max(max(luma_nw, luma_ne), max(luma_sw, luma_se)));
    if (luma_max - luma_min < max(0.0312, luma_max * 0.125)) {
        return rgb_m;
    }

    var dir = vec2<f32>(
        -((luma_nw + luma_ne) - (luma_sw + luma_se)),
         ((luma_nw + luma_sw) - (luma_ne + luma_se))
    );
    let reduce = max(
        (luma_nw + luma_ne + luma_sw + luma_se) * (0.25 * 0.125),
        1.0 / 128.0
    );
    let reciprocal_min = 1.0 / (min(abs(dir.x), abs(dir.y)) + reduce);
    dir = clamp(dir * reciprocal_min, vec2<f32>(-8.0), vec2<f32>(8.0)) * texel;

    let rgb_a = 0.5 * (
        scene(uv + dir * (1.0 / 3.0 - 0.5)) +
        scene(uv + dir * (2.0 / 3.0 - 0.5))
    );
    let rgb_b = rgb_a * 0.5 + 0.25 * (
        scene(uv + dir * -0.5) +
        scene(uv + dir * 0.5)
    );
    let luma_b = luminance(rgb_b);
    if (luma_b < luma_min || luma_b > luma_max) {
        return rgb_a;
    }
    return rgb_b;
}

struct BloomLevels {
    half: vec3<f32>,
    quarter: vec3<f32>,
    eighth: vec3<f32>,
};

fn bloom_levels(uv: vec2<f32>) -> BloomLevels {
    var levels: BloomLevels;
    levels.half = textureSample(bloom_half_texture, scene_sampler, uv).rgb;
    levels.quarter = textureSample(bloom_quarter_texture, scene_sampler, uv).rgb;
    levels.eighth = textureSample(bloom_eighth_texture, scene_sampler, uv).rgb;
    return levels;
}

fn bloom_color(levels: BloomLevels) -> vec3<f32> {
    // Keep bloom distinct from film halation: bloom is a faint, neutral optical
    // veil spread over a much wider radius, while halation owns the tighter
    // warm/red highlight halo. Reweighting the existing pyramid costs nothing.
    return levels.half * 0.18 + levels.quarter * 0.28 + levels.eighth * 0.54;
}

fn halation_red_levels(levels: BloomLevels) -> f32 {
    // Keep halation tighter than the deliberately broadened neutral bloom.
    return levels.half.r * 0.72 + levels.quarter.r * 0.28;
}

fn halation_red(uv: vec2<f32>) -> f32 {
    let half = textureSample(bloom_half_texture, scene_sampler, uv).r;
    let quarter = textureSample(bloom_quarter_texture, scene_sampler, uv).r;
    return half * 0.72 + quarter * 0.28;
}

fn apply_film_halation(color: vec3<f32>, red_return: f32) -> vec3<f32> {
    // Halation is red-sensitive emulsion/base scatter, not a second neutral
    // bloom. Suppress the direct highlight core a little so the low-resolution
    // return reads more like a red halo around bright sources than a red tint
    // painted onto the source itself.
    let source_luma = luminance(color);
    let core_suppression = 1.0 - smoothstep(0.72, 1.20, source_luma) * 0.52;
    let halo = red_return * core_suppression;
    return color + vec3<f32>(halo, halo * 0.11, halo * 0.018) * 0.40;
}

fn ssao_bilateral_tap(
    history_pixel: vec2<i32>,
    bilinear_weight: f32,
    centre_depth: f32
) -> vec2<f32> {
    let dims = textureDimensions(ssao_history_texture);
    let max_pixel = vec2<i32>(i32(dims.x) - 1, i32(dims.y) - 1);
    let sample_value = textureLoad(
        ssao_history_texture,
        clamp(history_pixel, vec2<i32>(0), max_pixel),
        0
    );
    if (!valid_depth(sample_value.g)) {
        return vec2<f32>(0.0, 0.0);
    }

    // Depth-aware bilateral upsample. This is intentionally algebraic rather
    // than exp() based because it runs at full output resolution.
    let depth_range = max(16.0, centre_depth * 0.025);
    let relative_delta = abs(sample_value.g - centre_depth) / depth_range;
    var depth_weight = max(1.0 - relative_delta, 0.0);
    depth_weight *= depth_weight;
    let weight = bilinear_weight * depth_weight;
    return vec2<f32>(sample_value.r * weight, weight);
}

fn ssao_factor(pixel: vec2<i32>, centre_depth: f32) -> f32 {
    if (!valid_depth(centre_depth)) {
        return 1.0;
    }

    let full_viewport = max(settings.aa.yz, vec2<f32>(1.0));
    let uv = (vec2<f32>(pixel) + vec2<f32>(0.5)) / full_viewport;
    let history_dims_u = textureDimensions(ssao_history_texture);
    let history_dims = vec2<f32>(f32(history_dims_u.x), f32(history_dims_u.y));
    let history_coord = uv * history_dims - vec2<f32>(0.5);
    let base = vec2<i32>(floor(history_coord));
    let f = fract(history_coord);

    let w00 = (1.0 - f.x) * (1.0 - f.y);
    let w10 = f.x * (1.0 - f.y);
    let w01 = (1.0 - f.x) * f.y;
    let w11 = f.x * f.y;

    let a = ssao_bilateral_tap(base, w00, centre_depth);
    let b = ssao_bilateral_tap(base + vec2<i32>(1, 0), w10, centre_depth);
    let c = ssao_bilateral_tap(base + vec2<i32>(0, 1), w01, centre_depth);
    let d = ssao_bilateral_tap(base + vec2<i32>(1, 1), w11, centre_depth);
    let accumulated = a + b + c + d;
    if (accumulated.y <= 1e-5) {
        return 1.0;
    }
    return clamp(accumulated.x / accumulated.y, 0.0, 1.0);
}

// Contact shadows: a screen-space depth march toward the map's sun, after Bevy's
// `ContactShadows` (bevy_pbr contact_shadows.rs / ssr raymarch.wesl).
//
// - The ray has a fixed WORLD length, so the shadow is the same size wherever the
//   camera is. It is marched in SCREEN space (uniform pixel steps), clipped to the
//   viewport rather than abandoned when a sample leaves it.
// - `march_behind_surfaces`: a surface further in front of the ray than the
//   thickness is not an occluder (the ray passes behind it and keeps going), and
//   a hit that penetrates most of the thickness fades out instead of popping.
// - Start jitter (interleaved gradient noise) turns step banding into fine noise.
// - The receiver itself is rejected geometrically: a sample only occludes if it
//   stands clear of the receiver's tangent plane (CONTACT_SHADOW_MIN_HEIGHT), so one
//   depth tap per step is enough (Bevy takes a bilinear and a nearest tap).
// - `r_contactShadowDebug 1` shows why the pass returned for each pixel.
const CONTACT_SHADOW_STEPS: i32 = 16;
// Bevy's defaults are length 0.3 m / thickness 0.1 m; at ~32 JKA units per metre
// (a 56-unit player is ~1.75 m) that is ~10 and ~3.2 units.
const CONTACT_SHADOW_LENGTH: f32 = 10.0; // world units toward the sun
const CONTACT_SHADOW_THICKNESS: f32 = 3.2;
// A sample is only an occluder if it stands at least this far above the receiver's
// tangent plane (plus a little per unit of depth). On baronshed_share (map origin
// ~43k units out) the ray's radial depth and the stored depth disagree by a few
// units that grow with range, so the floor sampled a few pixels along the ray read
// as "in front of" the ray and shadowed itself. Those false hits sat within ~1 unit
// of the receiver plane (measured p99 1.15); real occluders stand clear of it. The
// depth disagreement itself was never explained.
const CONTACT_SHADOW_MIN_HEIGHT: f32 = 1.5;
const CONTACT_SHADOW_STRENGTH: f32 = 0.5; // darkest multiplier is 1 - this

// Interleaved gradient noise (Jimenez 2014), static per pixel.
fn contact_shadow_noise(pixel: vec2<i32>) -> f32 {
    return fract(52.9829189 * fract(dot(vec2<f32>(pixel), vec2<f32>(0.06711056, 0.00583715))));
}

// Normal from depth that does not smear across silhouettes: on each axis take the
// neighbour with the smaller depth step, so a floor texel beside a wall base is
// not tilted by the wall.
fn contact_shadow_normal(pixel: vec2<i32>, p0: vec3<f32>, d0: f32) -> vec3<f32> {
    let dims = textureDimensions(linear_depth_texture);
    let dims_f = vec2<f32>(f32(dims.x), f32(dims.y));
    let dl = depth_at_pixel(pixel + vec2<i32>(-1, 0));
    let dr = depth_at_pixel(pixel + vec2<i32>(1, 0));
    let du = depth_at_pixel(pixel + vec2<i32>(0, -1));
    let dd = depth_at_pixel(pixel + vec2<i32>(0, 1));
    let ok_l = valid_depth(dl);
    let ok_r = valid_depth(dr);
    let ok_u = valid_depth(du);
    let ok_d = valid_depth(dd);
    if (!(ok_l || ok_r) || !(ok_u || ok_d)) {
        return normalize(settings.camera_pos_time.xyz - p0);
    }
    let use_left = ok_l && (!ok_r || abs(dl - d0) < abs(dr - d0));
    let use_up = ok_u && (!ok_d || abs(du - d0) < abs(dd - d0));
    let px = select(pixel + vec2<i32>(1, 0), pixel + vec2<i32>(-1, 0), use_left);
    let py = select(pixel + vec2<i32>(0, 1), pixel + vec2<i32>(0, -1), use_up);
    // Camera-relative so the one-pixel tangents are not quantised by the f32
    // spacing at map coordinates (see surface_normal).
    let r0 = world_ray((vec2<f32>(pixel) + vec2<f32>(0.5)) / dims_f) * d0;
    let rel_x = world_ray((vec2<f32>(px) + vec2<f32>(0.5)) / dims_f) * select(dr, dl, use_left);
    let rel_y = world_ray((vec2<f32>(py) + vec2<f32>(0.5)) / dims_f) * select(dd, du, use_up);
    let tx = select(rel_x - r0, r0 - rel_x, use_left);
    let ty = select(rel_y - r0, r0 - rel_y, use_up);
    var n = normalize(cross(tx, ty));
    if (dot(n, -r0) < 0.0) {
        n = -n;
    }
    return n;
}

// Diagnostic (r_contactShadowDebug 1): why the pass returned for a pixel.
// 0 marched (grey = the factor), 1 invalid depth (magenta), 2 faces away from the
// sun (blue), 3 origin behind the camera (yellow), 4 ray sub-pixel or off screen (red).
var<private> contact_shadow_reason: f32;

fn contact_shadow_debug_color(factor: f32) -> vec3<f32> {
    let r = contact_shadow_reason;
    if (r > 3.5) { return vec3<f32>(1.0, 0.0, 0.0); }
    if (r > 2.5) { return vec3<f32>(1.0, 1.0, 0.0); }
    if (r > 1.5) { return vec3<f32>(0.0, 0.2, 1.0); }
    if (r > 0.5) { return vec3<f32>(1.0, 0.0, 1.0); }
    return vec3<f32>(factor);
}

fn contact_shadow_factor(pixel: vec2<i32>, world: vec3<f32>, centre_depth: f32) -> f32 {
    contact_shadow_reason = 0.0;
    if (settings.scene.x <= 0.5 || !valid_depth(centre_depth)) {
        contact_shadow_reason = 1.0;
        return 1.0;
    }
    let camera = settings.camera_pos_time.xyz;
    // cloud_sun_direction is the direction the light travels (sun -> world).
    let toward_sun = -normalize(settings.cloud_sun_direction.xyz);
    let n = contact_shadow_normal(pixel, world, centre_depth);
    // Surfaces turned from the sun are already unlit; darkening them again is
    // the double-shadowing that made the old pass look dirty.
    let facing = smoothstep(0.02, 0.30, dot(n, toward_sun));
    if (facing <= 0.0) {
        contact_shadow_reason = 2.0;
        return 1.0;
    }

    // Ray endpoints. Lift the origin off the receiver, and pull the far end in
    // front of the camera plane so the projection below is well defined.
    let p0 = world + n * (1.0 + centre_depth * 0.001);
    var p1 = p0 + toward_sun * CONTACT_SHADOW_LENGTH;
    let c0 = settings.view_proj * vec4<f32>(p0, 1.0);
    if (c0.w <= 1.0) {
        contact_shadow_reason = 3.0;
        return 1.0;
    }
    var c1 = settings.view_proj * vec4<f32>(p1, 1.0);
    if (c1.w < 1.0) {
        p1 = mix(p0, p1, (c0.w - 1.0) / (c0.w - c1.w));
        c1 = settings.view_proj * vec4<f32>(p1, 1.0);
    }
    let ndc0 = c0.xy / c0.w;
    let ndc1 = c1.xy / c1.w;
    let inv_w0 = 1.0 / c0.w;
    let inv_w1 = 1.0 / max(c1.w, 1.0e-3);

    // Clip the ray to the viewport in screen space; s is the screen-space
    // parameter (0 at the origin, 1 at the far end).
    let delta = ndc1 - ndc0;
    let safe_delta = select(delta, vec2<f32>(1.0e-6), abs(delta) < vec2<f32>(1.0e-6));
    let edge = select(vec2<f32>(-1.0), vec2<f32>(1.0), safe_delta > vec2<f32>(0.0));
    let to_edge = (edge - ndc0) / safe_delta;
    let s_end = clamp(min(to_edge.x, to_edge.y), 0.0, 1.0);

    let dims = textureDimensions(linear_depth_texture);
    let dims_f = vec2<f32>(f32(dims.x), f32(dims.y));
    let length_px = length(delta * 0.5 * dims_f) * s_end;
    if (length_px < 2.0) {
        contact_shadow_reason = 4.0;
        return 1.0; // sub-pixel at this distance, or the ray is off screen
    }
    let steps = clamp(i32(length_px), 2, CONTACT_SHADOW_STEPS);
    let step_s = s_end / f32(steps);

    // Perspective-correct world interpolation: world/w is affine in screen space.
    let a0 = p0 * inv_w0;
    let a1 = p1 * inv_w1;
    // The distance term covers the ray-vs-surface depth offset, which grows with range.
    let thickness = CONTACT_SHADOW_THICKNESS + centre_depth * 0.015;
    let min_height = CONTACT_SHADOW_MIN_HEIGHT + centre_depth * 0.004;
    let jitter = 0.25 + 0.75 * contact_shadow_noise(pixel);

    var amount = 0.0;
    for (var i: i32 = 0; i < steps; i = i + 1) {
        let s = (f32(i) + jitter) * step_s;
        let ndc = mix(ndc0, ndc1, s);
        let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        let surface = depth_at_uv(uv);
        if (!valid_depth(surface)) {
            continue; // sky never occludes
        }
        let ray_world = mix(a0, a1, s) / mix(inv_w0, inv_w1, s);
        let to_ray = ray_world - camera;
        let ray_depth = length(to_ray);
        let penetration = ray_depth - surface;
        if (penetration <= 0.0 || penetration >= thickness) {
            continue; // in front of the surface, or passing well behind it
        }
        // Is this depth texel just the receiver itself? A genuine occluder stands
        // clear of the receiver's tangent plane; the floor seen a few pixels along
        // the ray does not.
        let sample_world = camera + world_ray(uv) * surface;
        if (dot(n, sample_world - world) < min_height) {
            continue;
        }
        // Bevy: shallow hits are fully dark, hits near the thickness limit fade out.
        let visibility = clamp((penetration / thickness - 0.5) * 2.0, 0.0, 1.0);
        // Ease out toward the end of the ray so the shadow has no hard cut-off.
        let along = clamp(distance(ray_world, p0) / CONTACT_SHADOW_LENGTH, 0.0, 1.0);
        amount = (1.0 - visibility) * (1.0 - smoothstep(0.55, 1.0, along));
        break;
    }
    return 1.0 - amount * facing * CONTACT_SHADOW_STRENGTH;
}

// Standing-water coverage at a world position, from the same field, shape and
// exposure rules as the material pass (weather_surface.wgsl). Walls and ceilings
// are rejected before the field is fetched.
fn post_puddle_amount(world: vec3<f32>, normal_y: f32) -> f32 {
    let accumulation = clamp(settings.rain.w, 0.0, 1.0);
    if (accumulation <= 0.001 || normal_y < 0.75
        || settings.rain_occlusion.z <= 0.0 || settings.rain_occlusion.w <= 0.0) {
        return 0.0;
    }
    let field = weather_sample_field(
        rain_occlusion_height,
        world,
        settings.rain_occlusion.xy,
        settings.rain_occlusion.zw,
        normal_y
    );
    if (field.exposure <= 0.001) {
        return 0.0;
    }
    let shape = weather_puddle_shape(field, world.xz, accumulation, settings.weather_look.x);
    return shape.coverage * field.exposure * smoothstep(0.75, 0.95, normal_y);
}

fn finalize_puddle_film(color: vec3<f32>, world: vec3<f32>, puddle: f32) -> vec3<f32> {
    let water = smoothstep(0.04, 0.78, puddle);
    if (water <= 0.001) {
        return color;
    }

    // This is intentionally a final clear-water film, not another material pass.
    // It runs after every authored JKA stage has already composited, so a glow or
    // filter stage can no longer paint a dry surface back over the puddle.
    let luma = luminance(color);
    let saturated = max(mix(vec3<f32>(luma), color, 1.045), vec3<f32>(0.0));
    let clear_wet = saturated * 0.80;
    let view_dir = normalize(settings.camera_pos_time.xyz - world);
    let grazing = 1.0 - clamp(abs(view_dir.y), 0.0, 1.0);
    let film_strength = min(water * mix(0.48, 0.30, grazing) * 2.0, 1.0);
    return mix(color, clear_wet, film_strength);
}

fn resolved_scene_radial_depth(pixel: vec2<i32>) -> f32 {
    let dims = textureDimensions(ssr_visibility_texture);
    let maximum = vec2<i32>(i32(dims.x) - 1, i32(dims.y) - 1);
    let clamped_pixel = clamp(pixel, vec2<i32>(0), maximum);
    let device_depth = textureLoad(ssr_visibility_texture, clamped_pixel, 0).x;
    if (device_depth <= 0.0) {
        return 1.0e30;
    }

    let dims_f = vec2<f32>(f32(dims.x), f32(dims.y));
    let uv = (vec2<f32>(clamped_pixel) + vec2<f32>(0.5)) / dims_f;
    let ndc = vec4<f32>(
        uv.x * 2.0 - 1.0,
        1.0 - uv.y * 2.0,
        device_depth,
        1.0
    );
    var world = settings.inv_view_proj * ndc;
    if (abs(world.w) <= 1.0e-6) {
        return 1.0e30;
    }
    world = world / world.w;
    return distance(world.xyz, settings.camera_pos_time.xyz);
}

fn bsp_frontmost_at_pixel(pixel: vec2<i32>) -> bool {
    let bsp_depth = depth_at_pixel(pixel);
    if (!valid_depth(bsp_depth)) {
        return false;
    }
    let scene_depth = resolved_scene_radial_depth(pixel);
    if (!valid_depth(scene_depth)) {
        return false;
    }

    // The old gate compared two reverse-Z samples with an absolute 1e-7
    // epsilon. That was unstable across camera jitter, MSAA coverage and target
    // recreation (alt-tab/resize), sometimes rejecting the entire fog post pass.
    // Compare in world-space distance instead. A later dynamic owner is normally
    // tens/hundreds of units closer; this small tolerance only absorbs raster
    // and polygon-offset differences for the same BSP surface.
    let tolerance = max(2.0, bsp_depth * 0.002);
    return scene_depth + tolerance >= bsp_depth;
}

struct SsrBilateralTap {
    value: vec4<f32>,
    weight: f32,
}

fn ssr_bilateral_tap(
    history_pixel: vec2<i32>,
    bilinear_weight: f32,
    centre_depth: f32
) -> SsrBilateralTap {
    let dims = textureDimensions(ssr_history_texture);
    let max_pixel = vec2<i32>(i32(dims.x) - 1, i32(dims.y) - 1);
    let pixel = clamp(history_pixel, vec2<i32>(0), max_pixel);
    let reflection = textureLoad(ssr_history_texture, pixel, 0);
    let sample_depth = textureLoad(ssr_depth_texture, pixel, 0).x;

    var result: SsrBilateralTap;
    result.value = vec4<f32>(0.0);
    result.weight = 0.0;
    if (!valid_depth(sample_depth)) {
        return result;
    }

    let depth_range = max(18.0, centre_depth * 0.025);
    let relative_delta = abs(sample_depth - centre_depth) / depth_range;
    var depth_weight = max(1.0 - relative_delta, 0.0);
    depth_weight *= depth_weight;
    result.weight = bilinear_weight * depth_weight;
    result.value = reflection * result.weight;
    return result;
}

fn temporal_ssr_color(pixel: vec2<i32>, centre_depth: f32, color: vec3<f32>, puddle: f32) -> vec3<f32> {
    if (settings.scene.z <= 0.5 || !valid_depth(centre_depth)) {
        return color;
    }
    // Gate again at full display resolution. The SSR history is half-res and
    // bilinear upsampling can otherwise pull a neighboring BSP reflection over
    // a player/ocean silhouette even when the half-res source texel was cleared.
    // Entities write SSR-ineligible policy in the prepass; the frontmost test
    // still covers non-prepass geometry such as grass and promoted ocean.
    if (!ssr_receiver_at_pixel(pixel) || !bsp_frontmost_at_pixel(pixel)) {
        return color;
    }

    let full_viewport = max(settings.aa.yz, vec2<f32>(1.0));
    let uv = (vec2<f32>(pixel) + vec2<f32>(0.5)) / full_viewport;
    let history_dims_u = textureDimensions(ssr_history_texture);
    let history_dims = vec2<f32>(f32(history_dims_u.x), f32(history_dims_u.y));
    let history_coord = uv * history_dims - vec2<f32>(0.5);
    let base = vec2<i32>(floor(history_coord));
    let f = fract(history_coord);

    let taps = array<SsrBilateralTap, 4>(
        ssr_bilateral_tap(base, (1.0 - f.x) * (1.0 - f.y), centre_depth),
        ssr_bilateral_tap(base + vec2<i32>(1, 0), f.x * (1.0 - f.y), centre_depth),
        ssr_bilateral_tap(base + vec2<i32>(0, 1), (1.0 - f.x) * f.y, centre_depth),
        ssr_bilateral_tap(base + vec2<i32>(1, 1), f.x * f.y, centre_depth)
    );

    var accumulated = vec4<f32>(0.0);
    var total_weight = 0.0;
    for (var i = 0u; i < 4u; i = i + 1u) {
        accumulated += taps[i].value;
        total_weight += taps[i].weight;
    }
    if (total_weight <= 1e-5) {
        return color;
    }

    let resolved = accumulated / total_weight;
    let water = smoothstep(0.04, 0.78, puddle);
    let reflection_weight = clamp(resolved.a, 0.0, mix(0.60, 0.94, water));
    return mix(color, max(resolved.rgb, vec3<f32>(0.0)), reflection_weight);
}

fn reflection_debug_color(pixel: vec2<i32>, centre_depth: f32, _base: vec3<f32>) -> vec3<f32> {
    if (!valid_depth(centre_depth) || !bsp_frontmost_at_pixel(pixel)) {
        return vec3<f32>(0.0);
    }

    let dims_u = textureDimensions(reflection_policy_texture);
    let maximum = vec2<i32>(i32(dims_u.x) - 1, i32(dims_u.y) - 1);
    let policy = textureLoad(
        reflection_policy_texture,
        clamp(pixel, vec2<i32>(0), maximum),
        0
    );
    let quality = u32(settings.taa_params.z + 0.5);

    // G is not merely "planar capable": the depth prepass writes it only when
    // this surface actually won one of the finite realtime planar slots.
    if (quality >= 3u && policy.g >= 0.5) {
        return vec3<f32>(1.0, 0.05, 0.85);
    }

    // SSR is a current-frame/temporal result, so only promote the candidate to
    // green when the depth-aware history resolve carries a valid hit weight.
    if (quality >= 2u && policy.r >= 0.5 && settings.scene.z > 0.5) {
        let full_viewport = max(settings.aa.yz, vec2<f32>(1.0));
        let uv = (vec2<f32>(pixel) + vec2<f32>(0.5)) / full_viewport;
        let history_dims_u = textureDimensions(ssr_history_texture);
        let history_dims = vec2<f32>(f32(history_dims_u.x), f32(history_dims_u.y));
        let history_coord = uv * history_dims - vec2<f32>(0.5);
        let base_pixel = vec2<i32>(floor(history_coord));
        let f = fract(history_coord);
        let taps = array<SsrBilateralTap, 4>(
            ssr_bilateral_tap(base_pixel, (1.0 - f.x) * (1.0 - f.y), centre_depth),
            ssr_bilateral_tap(base_pixel + vec2<i32>(1, 0), f.x * (1.0 - f.y), centre_depth),
            ssr_bilateral_tap(base_pixel + vec2<i32>(0, 1), (1.0 - f.x) * f.y, centre_depth),
            ssr_bilateral_tap(base_pixel + vec2<i32>(1, 1), f.x * f.y, centre_depth)
        );
        var accumulated_alpha = 0.0;
        var total_weight = 0.0;
        for (var i = 0u; i < 4u; i = i + 1u) {
            accumulated_alpha += taps[i].value.a;
            total_weight += taps[i].weight;
        }
        if (total_weight > 1e-5 && accumulated_alpha / total_weight > 0.01) {
            return vec3<f32>(0.05, 1.0, 0.18);
        }
    }

    // A valid local cubemap/probe is the stable fallback after a failed SSR ray.
    let packed_policy_bits = u32(round(clamp(policy.a, 0.0, 1.0) * 3.0));
    if (quality >= 1u && (packed_policy_bits & 1u) != 0u) {
        return vec3<f32>(0.06, 0.20, 1.0);
    }
    if (quality >= 2u && policy.r >= 0.5) {
        return vec3<f32>(0.18, 0.18, 0.18);
    }
    return vec3<f32>(0.0);
}

fn froxel_value(x: u32, y: u32, z: u32) -> vec4<f32> {
    let cx = min(x, FROXEL_X - 1u);
    let cy = min(y, FROXEL_Y - 1u);
    let cz = min(z, FROXEL_Z - 1u);
    let index = cz * FROXEL_X * FROXEL_Y + cy * FROXEL_X + cx;
    return integrated_fog[index];
}

fn sample_integrated_fog(uv: vec2<f32>, depth: f32) -> vec4<f32> {
    let fx = clamp(uv.x, 0.0, 0.999999) * f32(FROXEL_X - 1u);
    let fy = clamp(uv.y, 0.0, 0.999999) * f32(FROXEL_Y - 1u);
    let normalized_z = clamp(
        log(max(depth, FROXEL_NEAR) / FROXEL_NEAR) / log(FROXEL_FAR / FROXEL_NEAR),
        0.0,
        1.0
    );
    let fz = clamp(
        normalized_z * f32(FROXEL_Z) - 1.0,
        0.0,
        f32(FROXEL_Z - 1u)
    );

    let x0 = u32(floor(fx));
    let y0 = u32(floor(fy));
    let z0 = u32(floor(fz));
    let x1 = min(x0 + 1u, FROXEL_X - 1u);
    let y1 = min(y0 + 1u, FROXEL_Y - 1u);
    let z1 = min(z0 + 1u, FROXEL_Z - 1u);
    let tx = fract(fx);
    let ty = fract(fy);
    let tz = fract(fz);

    let c000 = froxel_value(x0, y0, z0);
    let c100 = froxel_value(x1, y0, z0);
    let c010 = froxel_value(x0, y1, z0);
    let c110 = froxel_value(x1, y1, z0);
    let c001 = froxel_value(x0, y0, z1);
    let c101 = froxel_value(x1, y0, z1);
    let c011 = froxel_value(x0, y1, z1);
    let c111 = froxel_value(x1, y1, z1);
    let c00 = mix(c000, c100, tx);
    let c10 = mix(c010, c110, tx);
    let c01 = mix(c001, c101, tx);
    let c11 = mix(c011, c111, tx);
    let c0 = mix(c00, c10, ty);
    let c1 = mix(c01, c11, ty);
    return mix(c0, c1, tz);
}

// Volumetric cloud shaping is a port of evroon/bevy-volumetric-clouds, which
// implements the Horizon Zero Dawn cloudscape method with Frostbite's
// energy-conserving scattering integral. Silhouette comes from a tileable
// Perlin-Worley weather map (base field, remap contrast floor, per-region cloud
// height) eroded at its edges by a tileable Worley detail volume. Both are baked
// once on the CPU in cloud_noise.rs.

// World units spanned by one repeat of the weather map, per cloud preset. The
// reference layer geometry (1250..2400) is close enough to JKA's default cloud
// height and thickness to reuse its ~13.3k tile directly.
const CLOUD_WEATHER_TILE_CUMULUS: f32 = 13333.0;
const CLOUD_WEATHER_TILE_STRATUS: f32 = 22000.0;
const CLOUD_WEATHER_TILE_STORM: f32 = 17000.0;
// The reference derives detail frequency from base frequency by this ratio, so
// resizing cloud cells rescales their erosion along with them.
const CLOUD_DETAIL_TILE_RATIO: f32 = 42.0;

const CLOUD_DETAIL_STRENGTH: f32 = 0.27;
const CLOUD_BASE_EDGE_SOFTNESS: f32 = 0.10;
const CLOUD_BOTTOM_SOFTNESS: f32 = 0.25;
// Extinction per world unit at full shaped density. The previous value was
// roughly fifteen times lower, which is most of why the deck read as haze
// rather than as cloud mass.
const CLOUD_DENSITY_SCALE: f32 = 0.03;
// Mean density of the unresolved far-field medium, as a fraction of in-cloud
// density. Tuned so the horizon keeps the opacity the previous far-field
// integration produced with its much weaker extinction constant.
const CLOUD_FAR_MEAN_DENSITY: f32 = 0.027;

// Frostbite dual-lobe phase parameters.
const CLOUD_FORWARD_G: f32 = 0.8;
const CLOUD_BACKWARD_G: f32 = -0.2;
const CLOUD_SCATTERING_LERP: f32 = 0.5;
const CLOUD_MIN_TRANSMITTANCE: f32 = 0.05;

// Self-shadow march. The first step is a fraction of layer thickness so the
// geometric growth below reaches about a tenth of the layer in six taps,
// matching the reference at its default 1150-unit layer.
const CLOUD_SHADOW_STEP_FRACTION: f32 = 0.0087;
const CLOUD_SHADOW_STEP_MULTIPLY: f32 = 1.3;

// Two-point ambient sky gradient. The ratio between these is the depth cue a
// single flat ambient term cannot provide; the gains are the exposure knobs
// that match the reference's absolute levels to JKA's.
const CLOUD_AMBIENT_TOP: vec3<f32> = vec3<f32>(0.993, 1.113, 1.333);
const CLOUD_AMBIENT_BOTTOM: vec3<f32> = vec3<f32>(0.260, 0.447, 0.580);
const CLOUD_AMBIENT_GAIN: f32 = 0.42;
const CLOUD_SUN_GAIN: f32 = 0.50;
// Gains that put the map's averaged skybox colour on the same level as the
// fixed gradient above, preserving its roughly 2.7:1 top-to-bottom ratio.
const CLOUD_SKY_AMBIENT_TOP_GAIN: f32 = 1.60;
const CLOUD_SKY_AMBIENT_BOTTOM_GAIN: f32 = 0.60;

// Horizontal displacement of the weather lookup across the full layer, per unit
// of thickness, at shear 1.0. Real cumulus lean with height; more importantly
// the base field is sampled from world.xz alone, so without this the cloud is a
// vertically extruded 2D image.
const CLOUD_SHEAR_SCALE: f32 = 1.0;
// Largest share of layer thickness the base-offset field may raise a cloud base
// by. Bases only ever rise, so the layer stays inside its authored slab.
const CLOUD_BASE_VARIATION_RANGE: f32 = 0.35;
// Measured centre and usable width of the baked cloud-height channel, used to
// normalise it before it drives thickness variation.
const CLOUD_HEIGHT_FIELD_MEAN: f32 = 0.328;
const CLOUD_HEIGHT_FIELD_SPREAD: f32 = 0.37;
// Largest share of the slab that thickness variation may take off a column.
const CLOUD_THICKNESS_VARIATION_RANGE: f32 = 0.70;
// Layer depth the weather tiles above were authored against. A cloud mass has
// to widen with the deck it sits in or its aspect changes every time the
// thickness slider moves: at a fixed tile a 2430-unit deck gives 5.5:1 masses
// where the reference had 11.6:1, which reads as tall spikes rather than as
// larger clouds. Tying the two keeps a cloud the same shape at any thickness
// and leaves Cloud size to change only its scale.
const CLOUD_REFERENCE_THICKNESS: f32 = 1150.0;
// Multiplier applied on top, at each end of the size control.
const CLOUD_SIZE_MIN: f32 = 0.5;
const CLOUD_SIZE_MAX: f32 = 4.0;

// Logarithmic, so a step of the slider is the same proportional change
// anywhere along it.
fn cloud_size_scale() -> f32 {
    return CLOUD_SIZE_MIN
        * pow(CLOUD_SIZE_MAX / CLOUD_SIZE_MIN, clamp(settings.cloud_variation.y, 0.0, 1.0));
}
// Extinction per world unit for aerial perspective on distant cloud.
const CLOUD_AERIAL_FALLOFF: f32 = 1.0e-4;

fn cloud_linearstep(s: f32, e: f32, v: f32) -> f32 {
    return clamp((v - s) / (e - s), 0.0, 1.0);
}

fn cloud_linearstep0(e: f32, v: f32) -> f32 {
    return min(v / max(e, 0.0001), 1.0);
}

fn cloud_remap(v: f32, s: f32, e: f32) -> f32 {
    return (v - s) / (e - s);
}

fn cloud_weather_tile(kind: f32) -> f32 {
    if (kind > 1.5) {
        return CLOUD_WEATHER_TILE_STORM;
    }
    if (kind > 0.5) {
        return CLOUD_WEATHER_TILE_STRATUS;
    }
    return CLOUD_WEATHER_TILE_CUMULUS;
}

// r: base Perlin-Worley field. g: spatially varying contrast floor that the
// density remap divides by, which is what stops every cloud in the sky from
// sharing one edge hardness. b: per-region cloud height, so towering and flat
// masses can coexist in a single frame. a: low-frequency base-offset field.
fn cloud_weather(world: vec3<f32>, tile: f32) -> vec4<f32> {
    return textureSampleLevel(
        cloud_weather_texture,
        cloud_noise_sampler,
        world.xz / tile,
        0.0
    );
}

fn cloud_detail(world: vec3<f32>, tile: f32) -> f32 {
    return textureSampleLevel(
        cloud_detail_texture,
        cloud_noise_sampler,
        world / tile,
        0.0
    ).r;
}

// The wind's direction, advection and erosion drift depend only on the frame
// time, so cloud_wind.rs evaluates them once per frame on the CPU and they
// arrive in the uniform (cloud_wind_dir, cloud_wind_offset, cloud_wind_delta,
// cloud_detail_slip, cloud_detail_billow). They were previously rebuilt from
// sin/cos in every density sample.

// Advect the sampling position with the prevailing wind, then shear it with
// height. The shear term is what stops a thick deck from reading as vertical
// curtains: the weather map is a 2D field sampled from world.xz, so at every
// altitude the silhouette is identical unless the lookup itself moves.
fn cloud_advected_noise_position(world: vec3<f32>, h: f32, thickness: f32) -> vec3<f32> {
    let wind_dir = settings.cloud_wind_dir.xyz;
    let wind_offset = settings.cloud_wind_offset.xyz;
    let shear = settings.cloud_shaping.x * CLOUD_SHEAR_SCALE * thickness * (h - 0.5);
    return world - wind_offset + wind_dir * shear;
}

// Let only the high-frequency erosion volume drift relative to the macro
// weather field. This slowly changes billows and edges without changing the
// identity or path of the large cloud masses and costs no extra texture fetch.
fn cloud_evolved_detail_position(p: vec3<f32>, tile: f32) -> vec3<f32> {
    if (settings.cloud_temporal_tuning.w <= 0.5) {
        return p;
    }
    // The macro weather field keeps the same path and identity; only the Worley
    // erosion volume slips through it. A small along-wind difference plus the
    // larger cross-wind slip prevents the erosion from looking like a second
    // conveyor belt pasted over the first one. Both terms are per-frame values
    // from cloud_wind.rs; only the tile scale of the wander is per sample.
    return p - settings.cloud_detail_slip.xyz + settings.cloud_detail_billow.xyz * (tile * 0.035);
}

fn cloud_terrain_lift_fraction(world: vec3<f32>, thickness: f32) -> f32 {
    if (settings.cloud_shadow.w <= 0.5 || settings.rain_occlusion.z <= 0.0 || settings.rain_occlusion.w <= 0.0) {
        return 0.0;
    }
    let uv = (world.xz - settings.rain_occlusion.xy) * settings.rain_occlusion.zw;
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        return 0.0;
    }
    let dims_u = textureDimensions(rain_occlusion_height);
    let dims = vec2<f32>(f32(dims_u.x), f32(dims_u.y));
    let px = vec2<i32>(clamp(uv * dims, vec2<f32>(0.0), dims - vec2<f32>(1.0)));
    let height_sample = textureLoad(rain_occlusion_height, px, 0);
    // R is the highest rain-blocking solid/patch; G is the highest visible
    // upward-facing topography. Use either so the experiment responds on maps
    // where only one of those representations covers a given feature.
    let terrain_y = max(height_sample.x, height_sample.y);
    if (terrain_y <= -1.0e19) {
        return 0.0;
    }

    // Begin bending the lower cloud before an actual intersection so the result
    // reads as an updraft, then increase the lift when geometry penetrates the
    // deck. This is still just one heightfield lookup: no rays or BSP traversal.
    let base = settings.cloud_layer.x;
    let below = max(base - terrain_y, 0.0);
    let proximity = 1.0 - smoothstep(0.0, max(thickness * 1.6, 1.0), below);
    let penetration = clamp((terrain_y - base) / max(thickness, 1.0), 0.0, 1.0);
    return proximity * mix(0.30, 0.55, penetration);
}

const CLOUD_HORIZON_RADIUS: f32 = 524288.0;
const CLOUD_SHELL_EPSILON: f32 = 4.0;
// Angular width, in cosine of incidence at the cloud base, over which a ray
// that grazes the underside of the deck is blended into one that punches
// through it. See cloud_shell_exit().
const CLOUD_BASE_TANGENT_FADE: f32 = 0.045;
// Height above the cloud base over which the below-deck horizon fade is handed
// back to the inside-the-deck case. Switching it at a four-unit boundary popped
// on the frame the camera crossed the base.
const CLOUD_HORIZON_FADE_DEPTH: f32 = 64.0;

fn cloud_shell_center(ro: vec3<f32>) -> vec3<f32> {
    // A camera-local tangent sphere keeps JKA's ordinary map coordinates while
    // giving the cloud layer gentle planetary curvature. Keep this radius much
    // larger than a normal JKA map: a visibly small sphere produces a bowl/ring
    // silhouette instead of a believable distant cloud horizon.
    return vec3<f32>(ro.x, settings.cloud_layer.x - CLOUD_HORIZON_RADIUS, ro.z);
}

fn ray_sphere_roots(ro: vec3<f32>, rd: vec3<f32>, center: vec3<f32>, radius: f32) -> vec2<f32> {
    let oc = ro - center;
    let b = dot(oc, rd);
    let c = dot(oc, oc) - radius * radius;
    let discriminant = b * b - c;
    if (discriminant < 0.0) {
        return vec2<f32>(1.0e30, -1.0e30);
    }
    let root = sqrt(max(discriminant, 0.0));
    return vec2<f32>(-b - root, -b + root);
}

fn ray_sphere_roots_valid(roots: vec2<f32>) -> bool {
    // ray_sphere_roots() encodes a miss as (+large, -large). Never let one
    // of those sentinels become a real cloud march entry/exit distance.
    return roots.x <= roots.y;
}

// Where a ray that is already inside the deck stops.
//
// A ray that only grazes the underside of the cloud base keeps travelling
// through cloud and leaves through the top; one that genuinely penetrates the
// base leaves into clear air below. At the tangent elevation those two exit
// distances differ several-fold (roughly 4.8x just inside the lower boundary),
// so choosing between them with a hard test drew a sharp horizontal line across
// the horizon at exactly that elevation, with saturated cloud above it and thin
// cloud below. Blending across the tangency removes the line: at grazing
// incidence this returns the top exit, which is precisely what the no-hit case
// returns, so the interval is continuous through the transition.
fn cloud_shell_exit(
    ro: vec3<f32>,
    rd: vec3<f32>,
    center: vec3<f32>,
    inner_roots: vec2<f32>,
    inner_hit: bool,
    outer_exit: f32,
    min_t: f32
) -> f32 {
    if (!inner_hit || inner_roots.x <= min_t) {
        return outer_exit;
    }
    let entry = ro + rd * inner_roots.x;
    let penetration = abs(dot(normalize(entry - center), rd));
    return mix(
        outer_exit,
        inner_roots.x,
        smoothstep(0.0, CLOUD_BASE_TANGENT_FADE, penetration)
    );
}

fn cloud_shell_interval(ro: vec3<f32>, rd: vec3<f32>) -> vec2<f32> {
    let inner_radius = CLOUD_HORIZON_RADIUS;
    let outer_radius = inner_radius + max(settings.cloud_layer.y, 1.0);
    let center = cloud_shell_center(ro);
    let radial = ro - center;
    let camera_radius = length(radial);
    let radial_dir = radial / max(camera_radius, 1.0);
    let radial_motion = dot(radial_dir, rd);
    let outer_roots = ray_sphere_roots(ro, rd, center, outer_radius);
    if (!ray_sphere_roots_valid(outer_roots) || outer_roots.y <= CLOUD_SHELL_EPSILON) {
        return vec2<f32>(1.0, 0.0);
    }

    let inner_roots = ray_sphere_roots(ro, rd, center, inner_radius);
    let inner_hit = ray_sphere_roots_valid(inner_roots);
    var t0 = 0.0;
    var t1 = 0.0;

    if (camera_radius <= inner_radius + CLOUD_SHELL_EPSILON) {
        // Below the cloud deck (or exactly on its lower boundary), never march
        // inward through the fake planet and pick the cloud shell up again on
        // the far side. In the real atmosphere the terrain/planet would occlude
        // that segment; JKA skyboxes do not provide such an occluder.
        //
        // Keep a narrow below-horizon allowance so render_clouds() can fade
        // the deck smoothly through the local horizon instead of cutting it
        // off on one exact scanline. Rays farther downward are still rejected
        // so they cannot wrap through the fake planet to the far-side shell.
        if (radial_motion < -0.035) {
            return vec2<f32>(1.0, 0.0);
        }
        t0 = max(inner_roots.y, 0.0);
        t1 = outer_roots.y;
    } else if (camera_radius < outer_radius - CLOUD_SHELL_EPSILON) {
        // Camera is genuinely inside the cloud shell. March only the connected
        // piece of cloud volume containing the camera; a ray toward the clear
        // inner atmosphere leaves at the near inner-sphere intersection.
        t0 = 0.0;
        t1 = cloud_shell_exit(
            ro,
            rd,
            center,
            inner_roots,
            inner_hit,
            outer_roots.y,
            CLOUD_SHELL_EPSILON
        );
    } else if (camera_radius <= outer_radius + CLOUD_SHELL_EPSILON) {
        // On the top boundary: inward rays enter the cloud deck; outward rays
        // immediately leave it. Handling the boundary explicitly avoids the
        // same far-side wraparound that occurred at the lower boundary.
        if (radial_motion >= 0.0) {
            return vec2<f32>(1.0, 0.0);
        }
        t0 = 0.0;
        t1 = cloud_shell_exit(
            ro,
            rd,
            center,
            inner_roots,
            inner_hit,
            outer_roots.y,
            CLOUD_SHELL_EPSILON
        );
    } else {
        // Above the cloud deck. Downward-looking rays are valid here: this is
        // the case where clouds really should appear below the local horizon.
        // Take only the first connected shell segment, never the far side.
        if (outer_roots.x <= CLOUD_SHELL_EPSILON) {
            return vec2<f32>(1.0, 0.0);
        }
        t0 = outer_roots.x;
        t1 = cloud_shell_exit(
            ro,
            rd,
            center,
            inner_roots,
            inner_hit,
            outer_roots.y,
            t0 + CLOUD_SHELL_EPSILON
        );
    }

    if (t1 <= t0) {
        return vec2<f32>(1.0, 0.0);
    }

    // The lower hemisphere is the fake planet-side continuation of the shell,
    // not usable sky. This is a final guard against any numerical far-side hit.
    let midpoint = ro + rd * ((t0 + t1) * 0.5);
    if ((midpoint - center).y <= 0.0) {
        return vec2<f32>(1.0, 0.0);
    }
    return vec2<f32>(t0, t1);
}

fn cloud_normalized_height(world: vec3<f32>) -> f32 {
    let thickness = max(settings.cloud_layer.y, 1.0);
    let center = cloud_shell_center(settings.camera_pos_time.xyz);
    return (length(world - center) - CLOUD_HORIZON_RADIUS) / thickness;
}

// Erodes the top and bottom boundaries of the layer. Kept per preset so stratus
// stays a broad sheet while storm cloud fills nearly the whole slab.
fn cloud_vertical_gradient(h: f32, kind: f32) -> f32 {
    if (kind > 1.5) {
        return cloud_linearstep(0.0, 0.06, h) - cloud_linearstep(0.85, 1.15, h);
    }
    if (kind > 0.5) {
        return cloud_linearstep(0.0, 0.04, h) - cloud_linearstep(0.88, 1.10, h);
    }
    return cloud_linearstep(0.0, 0.1, h) - cloud_linearstep(0.8, 1.2, h);
}

fn cloud_density_gain(kind: f32) -> f32 {
    if (kind > 1.5) {
        return 1.38;
    }
    if (kind > 0.5) {
        return 0.82;
    }
    return 1.0;
}

fn cloud_far_vertical_profile(world: vec3<f32>) -> f32 {
    // The far field deliberately stops resolving individual cloud cells. Keep
    // only the layer's vertical envelope so a very long grazing ray can be
    // integrated analytically into a smooth cloud bank instead of turning
    // sparse raymarch samples into screen-space grain.
    let h = cloud_normalized_height(world);
    if (h <= 0.0 || h >= 1.0) {
        return 0.0;
    }
    let kind = settings.clouds.y;
    return max(cloud_vertical_gradient(h, kind) * cloud_density_gain(kind), 0.0);
}

// Shaped cloud fraction in [0, 1], before extinction scaling. `detail` scales
// how much Worley erosion is applied: distant samples drop it so the silhouette
// stays stable instead of dissolving into per-sample noise. The second component
// is the pre-detail macro edge: values <= 0 are guaranteed empty because Worley
// detail only erodes density. The primary marcher can use that margin for
// conservative-ish larger steps when r_cloudEmptySkip is enabled.
fn cloud_shape_sample_impl(world: vec3<f32>, h: f32, detail: f32, use_terrain: bool) -> vec2<f32> {
    let thickness = max(settings.cloud_layer.y, 1.0);
    var terrain_lift = 0.0;
    if (use_terrain) {
        terrain_lift = cloud_terrain_lift_fraction(world, thickness);
    }
    let shaped_h = h - terrain_lift;
    if (shaped_h <= 0.0 || shaped_h >= 1.0) {
        return vec2<f32>(0.0, -0.01);
    }

    let kind = settings.clouds.y;
    let coverage = clamp(settings.clouds.w, 0.0, 1.0);
    let tile = cloud_weather_tile(kind)
        * (thickness / CLOUD_REFERENCE_THICKNESS)
        * cloud_size_scale();
    let p = cloud_advected_noise_position(world, shaped_h, thickness);
    let weather = cloud_weather(p, tile);

    // Shift this column's base by the low-frequency offset field and rescale so
    // the top of the layer stays put. The field is centred here so bases both
    // rise and fall: only ever raising them reads as a uniform lift of the whole
    // deck rather than as variation. Without this every term acting on the
    // bottom of the deck is a pure function of h, putting every cloud base on
    // one exact plane.
    let base_offset = (clamp(weather.a, 0.0, 1.0) * 2.0 - 1.0)
        * clamp(settings.cloud_shaping.y, 0.0, 1.0)
        * CLOUD_BASE_VARIATION_RANGE;
    let height_norm = clamp(
        (weather.b - CLOUD_HEIGHT_FIELD_MEAN) / CLOUD_HEIGHT_FIELD_SPREAD + 0.5,
        0.0,
        1.0
    );
    let top_limit = 1.0 - height_norm
        * clamp(settings.cloud_variation.x, 0.0, 1.0)
        * CLOUD_THICKNESS_VARIATION_RANGE;

    let hb = (shaped_h - base_offset) / max(top_limit - base_offset, 0.08);
    if (hb <= 0.0 || hb >= 1.0) {
        return vec2<f32>(0.0, -0.01);
    }

    let n = hb * hb * weather.b + pow(1.0 - hb, 16.0);
    var m = cloud_remap(weather.r - n, weather.g, 1.0) * cloud_vertical_gradient(hb, kind);
    let macro_edge = m + coverage - 1.0;

    // Under the A/B optimization gate, a non-positive macro edge is exact empty
    // space: the detail field can only subtract from m, never create density.
    // Returning here both avoids the 3D detail fetch and exposes the margin to
    // the primary march so it can skip more than one dt through very clear air.
    if (settings.cloud_shaping.z > 0.5 && macro_edge <= 0.0) {
        return vec2<f32>(0.0, macro_edge);
    }

    let erosion = smoothstep(1.0, 0.5, m) * clamp(detail, 0.0, 1.0);
    if (erosion > 0.0) {
        let detail_p = cloud_evolved_detail_position(p, tile);
        m -= cloud_detail(detail_p, tile / CLOUD_DETAIL_TILE_RATIO) * erosion * CLOUD_DETAIL_STRENGTH;
    }

    m = smoothstep(0.0, CLOUD_BASE_EDGE_SOFTNESS, m + coverage - 1.0);
    m *= cloud_linearstep0(CLOUD_BOTTOM_SOFTNESS, hb);

    return vec2<f32>(clamp(m * cloud_density_gain(kind), 0.0, 1.0), macro_edge);
}

fn cloud_shape_sample(world: vec3<f32>, h: f32, detail: f32) -> vec2<f32> {
    return cloud_shape_sample_impl(world, h, detail, true);
}

fn cloud_shape_without_terrain(world: vec3<f32>, h: f32, detail: f32) -> f32 {
    return cloud_shape_sample_impl(world, h, detail, false).x;
}

fn cloud_density_sample(world: vec3<f32>, h: f32, detail: f32) -> vec2<f32> {
    let sample = cloud_shape_sample(world, h, detail);
    return vec2<f32>(
        sample.x * CLOUD_DENSITY_SCALE * max(settings.cloud_sun_color.w, 0.0),
        sample.y
    );
}

fn cloud_density_without_terrain(world: vec3<f32>, h: f32, detail: f32) -> f32 {
    return cloud_shape_without_terrain(world, h, detail)
        * CLOUD_DENSITY_SCALE
        * max(settings.cloud_sun_color.w, 0.0);
}

fn cloud_henyey_greenstein(mu: f32, g: f32) -> f32 {
    let g2 = g * g;
    let denom = max(1.0 + g2 - 2.0 * g * mu, 0.0001);
    return (1.0 - g2) / (denom * sqrt(denom));
}

// Marched self-shadowing, as the reference does: the step size grows
// geometrically so a handful of taps still reach a useful fraction of the layer.
// Terrain lift is intentionally omitted from these secondary taps: the visible
// primary march pays the heightfield lookup, while internal shadowing remains an
// inexpensive approximation instead of multiplying that lookup by every tap.
fn cloud_volumetric_shadow(
    origin: vec3<f32>,
    toward_sun: vec3<f32>,
    thickness: f32,
    steps: u32,
    detail: f32
) -> f32 {
    var step_size = thickness * CLOUD_SHADOW_STEP_FRACTION;
    var travelled = step_size * 0.5;
    var transmittance = 1.0;

    for (var i = 0u; i < 8u; i = i + 1u) {
        if (i >= steps) {
            break;
        }
        let pos = origin + toward_sun * travelled;
        let h = cloud_normalized_height(pos);
        if (h > 1.0) {
            break;
        }
        transmittance *= exp(-cloud_density_without_terrain(pos, h, detail) * step_size);
        step_size *= CLOUD_SHADOW_STEP_MULTIPLY;
        travelled += step_size;
    }

    return transmittance;
}

// Ground cloud shadows are resolved here, after the complete JKA material has
// been composited. Doing this in an individual BSP shader stage made projected
// shadows depend on how a material happened to be split into legacy passes: an
// opaque base/lightmap pair could behave differently from its neighbours even
// though both occupy the same world space. Depth reconstruction gives every
// visible receiver the same world-space cloud mask, independent of material
// stage ordering and receiver normals.
fn projected_cloud_shadow_interval(ro: vec3<f32>, rd: vec3<f32>) -> vec2<f32> {
    let inner_radius = CLOUD_HORIZON_RADIUS;
    let thickness = max(settings.cloud_layer.y, 1.0);
    let outer_radius = inner_radius + thickness;
    // Match the visible cloud deck: one camera-local tangent sphere for the
    // whole frame, not a new tangent sphere centred on each receiver.
    let center = cloud_shell_center(settings.camera_pos_time.xyz);
    let receiver_radius = length(ro - center);
    let outer = ray_sphere_roots(ro, rd, center, outer_radius);
    if (!ray_sphere_roots_valid(outer) || outer.y <= 0.0) {
        return vec2<f32>(1.0, 0.0);
    }
    let inner = ray_sphere_roots(ro, rd, center, inner_radius);
    let inner_hit = ray_sphere_roots_valid(inner);
    var t0 = 0.0;
    var t1 = 0.0;

    if (receiver_radius < inner_radius) {
        if (!inner_hit || inner.y <= 0.0) {
            return vec2<f32>(1.0, 0.0);
        }
        t0 = inner.y;
        t1 = outer.y;
    } else if (receiver_radius <= outer_radius) {
        t0 = 0.0;
        t1 = outer.y;
        if (inner_hit && inner.x > 0.0) {
            t1 = min(t1, inner.x);
        }
    } else {
        if (outer.x <= 0.0) {
            return vec2<f32>(1.0, 0.0);
        }
        t0 = outer.x;
        t1 = outer.y;
        if (inner_hit && inner.x > t0) {
            t1 = min(t1, inner.x);
        }
    }

    if (t1 <= t0) {
        return vec2<f32>(1.0, 0.0);
    }
    let midpoint = ro + rd * ((t0 + t1) * 0.5);
    if ((midpoint - center).y <= 0.0) {
        return vec2<f32>(1.0, 0.0);
    }
    return vec2<f32>(t0, t1);
}

fn projected_cloud_shadow_density(world: vec3<f32>) -> f32 {
    // Ground shadows only need the silhouette, so the Worley erosion pass is
    // skipped here. Terrain lift is also omitted from these four secondary
    // probes to keep the experimental interaction from multiplying its map
    // heightfield fetch cost. The macro weather field still matches the deck.
    // This returns the shaped fraction rather than an extinction coefficient so
    // the attenuation constant below stays independent of CLOUD_DENSITY_SCALE.
    return cloud_shape_without_terrain(world, cloud_normalized_height(world), 0.0);
}

// Entities publish their smooth sun-facing value in the prepass policy G channel
// as code 26..126 of 255 (md3.wgsl cloud_shadow_facing_code); the BSP prepass
// writes exactly 0 or 1 there. Returns -1 when the pixel did not provide one.
fn entity_cloud_shadow_facing(pixel: vec2<i32>) -> f32 {
    let dims = textureDimensions(reflection_policy_texture);
    let maximum = vec2<i32>(i32(dims.x) - 1, i32(dims.y) - 1);
    let code = round(
        textureLoad(reflection_policy_texture, clamp(pixel, vec2<i32>(0), maximum), 0).g * 255.0
    );
    if (code < 26.0 || code > 126.0) {
        return -1.0;
    }
    return (code - 26.0) / 100.0;
}

fn projected_cloud_shadow_factor(
    world_position: vec3<f32>,
    normal: vec3<f32>,
    pixel: vec2<i32>
) -> f32 {
    if (settings.cloud_shadow.x <= 0.5) {
        return 1.0;
    }

    // cloud_sun_direction stores the direction the light travels (sun -> world).
    let toward_sun = -normalize(settings.cloud_sun_direction.xyz);
    if (toward_sun.y <= 0.02) {
        return 1.0;
    }
    let interval = projected_cloud_shadow_interval(world_position, toward_sun);
    if (interval.y <= interval.x) {
        return 1.0;
    }

    let segment = interval.y - interval.x;
    var density_sum = 0.0;
    for (var i = 0u; i < 4u; i = i + 1u) {
        let f = (f32(i) + 0.5) * 0.25;
        density_sum += projected_cloud_shadow_density(
            world_position + toward_sun * (interval.x + segment * f)
        );
    }
    let mean_density = density_sum * 0.25;
    let optical_distance = min(segment, max(settings.cloud_layer.y, 1.0) * 4.0);
    let visibility = exp(-mean_density * optical_distance * 0.00115);

    // 1.0 means the projected mask is allowed to reach its full Beer-Lambert
    // attenuation. The previous BSP path had a hidden second limiter (the CPU
    // supplied at most 0.55 here) on top of the visible 0.60 multiplier, which
    // is why even the nominal 60% setting looked much weaker than expected.
    // This mask multiplies the finished pixel, so it must only bite where the
    // sun was actually contributing. A surface turned away from the sun gets no
    // direct light for a cloud to take away, and dimming its ambient/emissive
    // colour (a wall, an overhang's underside) was the visible error; ramp the
    // strength in with how squarely the surface faces the sun.
    // Models use the facing their shader computed from smooth vertex normals;
    // a normal rebuilt from depth is flat per triangle and shows as blocks.
    var sun_facing = smoothstep(0.0, 0.3, dot(normal, toward_sun));
    let entity_facing = entity_cloud_shadow_facing(pixel);
    if (entity_facing >= 0.0) {
        sun_facing = entity_facing;
    }
    let strength = clamp(settings.cloud_shadow.y, 0.0, 1.0) * sun_facing;
    return 1.0 - strength * (1.0 - visibility);
}

fn cloud_scene_limit(uv: vec2<f32>, ro: vec3<f32>, rd: vec3<f32>) -> f32 {
    let depth = depth_at_uv(uv);
    if (!valid_depth(depth)) {
        return 1.0e30;
    }

    // The scene depth was generated with the current (possibly TAA-jittered)
    // camera matrix, while cloud rays intentionally use an unjittered matrix
    // for stable low-resolution/temporal rendering. Reconstruct the actual
    // visible scene point and project it onto the cloud ray instead of assuming
    // both rays are bit-identical.
    let scene_world = world_position(uv, depth);
    return max(dot(scene_world - ro, rd), 0.0);
}

fn current_cloud_transfer(uv: vec2<f32>, animate_jitter: bool) -> vec4<f32> {
    // RGB is in-scattered cloud light; A is the amount of background light
    // transmitted through the cloud layer. Keeping the transfer independent of
    // scene color lets us raymarch it once at a lower resolution and composite
    // it over the full-resolution sky later without baking the sky into history.
    if (settings.clouds.x <= 0.5) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let ro = settings.camera_pos_time.xyz;
    let rd = cloud_world_ray(uv);
    let interval = cloud_shell_interval(ro, rd);
    let t0 = interval.x;
    let t1 = min(interval.y, cloud_scene_limit(uv, ro, rd));
    if (t1 <= t0 + CLOUD_SHELL_EPSILON) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }

    let center = cloud_shell_center(ro);
    let inner_radius = CLOUD_HORIZON_RADIUS;
    let outer_radius = inner_radius + max(settings.cloud_layer.y, 1.0);
    let camera_radius = length(ro - center);
    let camera_radial = normalize(ro - center);

    // When the camera is below the deck there is no real JKA planet/terrain
    // sphere to occlude the mathematical far side of our curved cloud shell.
    // Fade through a small angular band around the local horizontal instead
    // of switching the shell on/off on one exact scanline.
    // How much of the below-deck horizon fade applies. It has to reach zero
    // before the camera is properly inside the layer, where dimming the horizon
    // would darken cloud the camera is standing in, but gating it on a
    // four-unit boundary switched the whole fade on in a single frame as the
    // camera crossed the base. Hand it over across a short height band instead.
    let below_amount = 1.0 - smoothstep(
        0.0,
        CLOUD_HORIZON_FADE_DEPTH,
        camera_radius - inner_radius
    );
    var horizon_fade = 1.0;
    if (below_amount > 0.0) {
        let radial_motion = dot(camera_radial, rd);
        horizon_fade = mix(1.0, smoothstep(-0.015, 0.035, radial_motion), below_amount);
    }

    // Tangent fading is only for rays that ENTER the shell from outside.
    // Applying it at t0 == 0 was wrong: when the camera is inside the cloud
    // volume, dot(camera_up, view_ray) reaches zero at the local horizon and
    // created the two hard cloud bands above and below the player.
    var grazing_fade = 1.0;
    if (t0 > CLOUD_SHELL_EPSILON &&
        (camera_radius < inner_radius - CLOUD_SHELL_EPSILON || camera_radius > outer_radius + CLOUD_SHELL_EPSILON)) {
        let entry_pos = ro + rd * t0;
        let entry_normal = normalize(entry_pos - center);
        grazing_fade = smoothstep(0.004, 0.060, abs(dot(entry_normal, rd)));
    }
    let shell_visibility = horizon_fade * grazing_fade;

    let quality = clamp(settings.clouds.z, 0.0, 1.0);
    let thickness = max(settings.cloud_layer.y, 1.0);

    // Do not spend the detailed stochastic raymarch on the entire tangent
    // length of a near-horizontal cloud ray. At low quality the detailed range
    // intentionally ends sooner; everything beyond it is integrated below as a
    // smooth bulk medium. This turns far-field salt-and-pepper noise into the
    // visually expected opaque cloud bank and makes the worst-case horizon path
    // cheaper rather than more expensive.
    let far_detail_distance = mix(16384.0, 32768.0, quality);
    let detailed_t1 = min(t1, max(t0, far_detail_distance));
    let detailed_length = max(detailed_t1 - t0, 0.0);

    // Fewer primary steps than the previous march used: each sample now costs a
    // weather fetch, a detail fetch and a multi-tap shadow march. The animated
    // jitter below lets temporal accumulation supply the convergence, which is
    // how the reference gets a clean image out of only twelve steps.
    let desired_steps = 10u + u32(round(quality * 30.0));
    let dt = detailed_length / max(f32(desired_steps), 1.0);
    let shadow_steps = 3u + u32(round(quality * 3.0));

    // Animate the jitter only where history is actually integrating it. The
    // bevy reference re-randomises every frame (hash13(ray_dir + fract(time)))
    // because it always accumulates when the camera is still; where history is
    // rejected there is nothing to average the variation away, and a frozen
    // per-pixel offset reads as a stable pattern instead of per-frame flicker.
    var jitter_frame = 0u;
    if (animate_jitter) {
        jitter_frame = u32(settings.camera_pos_time.w * 1000.0);
    }
    let pixel = vec2<u32>(uv * max(settings.aa.yz, vec2<f32>(1.0)));
    let offset = grain_hash(pixel, jitter_frame, 0x51f15e5du);
    var t = t0 + dt * offset;
    var transmittance = 1.0;
    var scattering = vec3<f32>(0.0);

    // DirectionalSun stores the direction the light rays travel (sun -> world),
    // while cloud lighting needs the direction from the sample back toward the sun.
    let light_direction = normalize(settings.cloud_sun_direction.xyz);
    let toward_sun = -light_direction;
    let sun_color = max(settings.cloud_sun_color.rgb, vec3<f32>(0.0));
    let sun_elevation = clamp(toward_sun.y, -1.0, 1.0);
    // Keep direct light alive right down to the horizon so low-angle sunlight
    // can actually paint the clouds instead of disappearing before sunset.
    let direct_sun_amount = smoothstep(-0.06, 0.035, sun_elevation);
    let low_sun_amount = 1.0 - smoothstep(0.12, 0.72, sun_elevation);

    // Preserve authored q3map sun chromaticity, then push the low sun toward a
    // warmer spectrum without changing its overall vector magnitude. Shadowed
    // cloud mass gets a subtle cool skylight bias for warm/cool sunset contrast.
    let sunset_tint = mix(vec3<f32>(1.0), vec3<f32>(1.55, 0.72, 0.32), low_sun_amount);
    let warm_sun = max(sun_color * sunset_tint, vec3<f32>(0.0001));
    let tinted_sun_color = normalize(warm_sun) * max(length(sun_color), 0.0001);
    let ambient_tint = mix(vec3<f32>(1.0), vec3<f32>(0.90, 0.94, 1.08), low_sun_amount);

    let kind = settings.clouds.y;
    var base_color = vec3<f32>(0.72, 0.76, 0.80);
    if (kind > 1.5) {
        base_color = vec3<f32>(0.38, 0.42, 0.48);
    } else if (kind > 0.5) {
        base_color = vec3<f32>(0.68, 0.71, 0.74);
    }
    // base_color is a preset tint, not a brightness: renormalising it around
    // cumulus keeps the ambient gradient's absolute level under CLOUD_AMBIENT_GAIN.
    let preset_tint = base_color / 0.76;
    let ambient_level = CLOUD_AMBIENT_GAIN * ambient_tint * preset_tint;
    // The ported gradient constants are tuned for the reference's own blue
    // procedural sky. On a desert, sunset or night skybox they light the clouds
    // the wrong colour entirely, so blend toward the map's averaged sky.
    let sky_amount = clamp(settings.cloud_sky_ambient.w, 0.0, 1.0);
    let sky_rgb = max(settings.cloud_sky_ambient.rgb, vec3<f32>(0.0));
    let ambient_top = mix(
        CLOUD_AMBIENT_TOP,
        sky_rgb * CLOUD_SKY_AMBIENT_TOP_GAIN,
        sky_amount
    ) * ambient_level;
    let ambient_bottom = mix(
        CLOUD_AMBIENT_BOTTOM,
        sky_rgb * CLOUD_SKY_AMBIENT_BOTTOM_GAIN,
        sky_amount
    ) * ambient_level;
    let sun_radiance = tinted_sun_color * CLOUD_SUN_GAIN * direct_sun_amount;

    // Frostbite dual-lobe phase function. mu is the cosine between the direction
    // the light travels and the direction it must leave in to reach the eye, so
    // the forward lobe peaks when looking into the sun. That peak is roughly
    // twenty times the backward value and is what produces silver-lined edges;
    // the single cosine power this replaces spanned less than a factor of two.
    let mu = dot(rd, toward_sun);
    let phase = mix(
        cloud_henyey_greenstein(mu, CLOUD_FORWARD_G),
        cloud_henyey_greenstein(mu, CLOUD_BACKWARD_G),
        CLOUD_SCATTERING_LERP
    );

    // r_cloudEmptySkip is a real primary-ray optimization, not merely a guard
    // around the tiny Worley texture fetch. cloud_density_sample() exposes the
    // pre-detail macro edge. Because detail only erodes, a strongly negative
    // edge is known-empty space and can use a larger dt. When a coarse endpoint
    // approaches a possible cloud region we backtrack to the first skipped fine
    // step before shading, which protects most thin towers and edge features.
    let empty_skip = settings.cloud_shaping.z > 0.5;
    var previous_advance = 1.0;
    var first_hit = -1.0;

    for (var i = 0u; i < 96u; i = i + 1u) {
        if (detailed_length <= CLOUD_SHELL_EPSILON ||
            (!empty_skip && i >= desired_steps) || t > detailed_t1 ||
            transmittance < CLOUD_MIN_TRANSMITTANCE) {
            break;
        }

        let pos = ro + rd * t;
        let h = cloud_normalized_height(pos);

        // Drop the high-frequency erosion with distance. The silhouette stays
        // intact because the same weather map drives it at every LOD.
        let detail_lod = 1.0 - smoothstep(10000.0, 30000.0, t);
        let density_sample = cloud_density_sample(pos, h, detail_lod);

        // We arrived here by a coarse jump and the macro field is no longer
        // confidently empty. Revisit the skipped interval at normal dt before
        // accumulating any opacity at this endpoint.
        if (empty_skip && previous_advance > 1.0 && density_sample.y > -0.04) {
            t -= dt * (previous_advance - 1.0);
            previous_advance = 1.0;
            continue;
        }

        let sigma = density_sample.x;
        if (sigma > 0.0) {
            if (first_hit < 0.0) {
                first_hit = t;
            }
            let shadow = cloud_volumetric_shadow(
                pos,
                toward_sun,
                thickness,
                shadow_steps,
                min(detail_lod, 0.35)
            );
            let ambient = mix(ambient_bottom, ambient_top, clamp(h, 0.0, 1.0));
            // Frostbite energy-conserving integration: S * (1 - dT) / sigma with
            // S = sigma * radiance, so the scattering coefficient cancels and the
            // result stops drifting with step count.
            let radiance = ambient + sun_radiance * phase * shadow;
            let delta_transmittance = exp(-sigma * dt);
            scattering += transmittance * radiance * (1.0 - delta_transmittance);
            transmittance *= delta_transmittance;
        }

        var advance = 1.0;
        if (empty_skip && density_sample.y < -0.12) {
            advance = 2.0;
            if (density_sample.y < -0.40) {
                advance = 4.0;
            }
        }
        t += dt * advance;
        previous_advance = advance;
    }

    // Past the detailed range, integrate a smooth statistical cloud medium in
    // one shot. A long tangent ray crosses many unresolved cloud cells, so its
    // physically useful result is the accumulated optical depth, not whichever
    // handful of high-frequency cells a coarse jittered march happened to hit.
    let far_t0 = detailed_t1;
    let far_length = max(t1 - far_t0, 0.0);
    if (far_length > CLOUD_SHELL_EPSILON && transmittance >= CLOUD_MIN_TRANSMITTANCE) {
        let far_p0 = ro + rd * (far_t0 + far_length * 0.25);
        let far_p1 = ro + rd * (far_t0 + far_length * 0.75);
        let far_profile = 0.5 * (
            cloud_far_vertical_profile(far_p0) +
            cloud_far_vertical_profile(far_p1)
        );
        let coverage = clamp(settings.clouds.w, 0.0, 1.0);
        // Squaring coverage keeps sparse-cloud presets from becoming an opaque
        // wall too quickly while normal/overcast coverage still converges fast
        // over the enormous path length of a horizon ray. The leading constant
        // is chosen so this matches the horizon opacity the previous, much
        // weaker extinction produced, now that CLOUD_DENSITY_SCALE is physical.
        let far_sigma = far_profile * CLOUD_FAR_MEAN_DENSITY * coverage * coverage
            * CLOUD_DENSITY_SCALE * max(settings.cloud_sun_color.w, 0.0);
        let far_alpha = 1.0 - exp(-far_sigma * far_length);
        if (far_alpha > 0.0001) {
            // Individual self-shadow probes are intentionally gone here. Their
            // high-frequency variation is exactly what should disappear at this
            // distance; use a stable skylight/sun mixture instead.
            let far_ambient = mix(ambient_bottom, ambient_top, 0.5);
            let far_radiance = far_ambient + sun_radiance * phase * 0.70;
            scattering += transmittance * far_radiance * far_alpha;
            transmittance *= 1.0 - far_alpha;
        }
    }

    // Aerial perspective. Without it a bank at sixty thousand units renders at
    // the same contrast as one overhead, which is a large part of why the
    // horizon reads as a wall. The reference fades toward its own sky colour by
    // first-hit distance; here the same falloff fades toward the ambient sky
    // already being used to light the clouds.
    let aerial = clamp(settings.cloud_shaping.w, 0.0, 1.0);
    if (aerial > 0.0 && first_hit > 0.0) {
        let haze_amount = clamp(0.8 - exp(-CLOUD_AERIAL_FALLOFF * first_hit), 0.0, 1.0) * aerial;
        let haze = ambient_top * (1.0 - transmittance);
        scattering = mix(scattering, haze, haze_amount);
    }

    let effective_transmittance = mix(1.0, transmittance, shell_visibility);
    return vec4<f32>(scattering * shell_visibility, effective_transmittance);
}

fn cloud_history_uv(uv: vec2<f32>) -> vec2<f32> {
    let ro = settings.camera_pos_time.xyz;
    let rd = cloud_world_ray(uv);
    let interval = cloud_shell_interval(ro, rd);
    if (interval.y <= interval.x) {
        return vec2<f32>(-1.0);
    }

    // Reproject a representative point halfway through the connected cloud
    // segment. The layer is thin compared with its viewing distance, so this is
    // stable while avoiding a second density-weighted march just for history.
    var world = ro + rd * ((interval.x + interval.y) * 0.5);

    // Follow the same advected cloud feature backward to the previous frame.
    // Use the exact displacement delta so the optional varying wind remains
    // temporally stable instead of assuming constant velocity.
    world -= settings.cloud_wind_delta.xyz;

    let prev_clip = settings.cloud_prev_view_proj * vec4<f32>(world, 1.0);
    if (prev_clip.w <= 1e-5) {
        return vec2<f32>(-1.0);
    }
    let prev_ndc = prev_clip.xy / prev_clip.w;
    return vec2<f32>(prev_ndc.x * 0.5 + 0.5, 0.5 - prev_ndc.y * 0.5);
}

@fragment
fn fs_cloud_march(input: VertexOut) -> @location(0) vec4<f32> {
    // Sparse march. With the interleave running, the pass is drawn into a
    // viewport 1/grid the size of the target and each invocation IS one block:
    // it marches the block's fresh texel and fs_cloud_resolve reconstructs every
    // other pixel from those samples. Skipping the other pixels inside a
    // full-size pass instead left one working lane in every 2x2 quad, so every
    // wave ran the whole march loop at a quarter of its lanes and saved almost
    // no time. The CPU picks the viewport from the same enabled && history-valid
    // condition as `temporal` here.
    let temporal = settings.cloud_temporal.x > 0.5 && settings.cloud_temporal.y > 0.5;
    var uv = input.uv;
    if (temporal) {
        let grid = max(u32(round(settings.cloud_temporal.w)), 1u);
        let pattern = u32(round(settings.cloud_temporal.z)) % (grid * grid);
        let fresh_offset = vec2<u32>(pattern % grid, pattern / grid);
        let size = vec2<u32>(textureDimensions(cloud_transfer_texture));
        let texel = min(vec2<u32>(input.position.xy) * grid + fresh_offset, size - vec2<u32>(1u));
        // input.uv spans the small viewport; the ray belongs to the full-size texel.
        uv = (vec2<f32>(texel) + vec2<f32>(0.5)) / vec2<f32>(size);
    }

    // Freeze the jitter the moment the camera moves. The resolve rejects
    // history at that point, so nothing is left to average a fresh random
    // offset away and it reads as flicker; a per-pixel offset that is constant
    // in time is a stable pattern instead.
    var animate = false;
    if (temporal) {
        let prev_uv = cloud_history_uv(uv);
        if (all(prev_uv >= vec2<f32>(0.0)) && all(prev_uv <= vec2<f32>(1.0))) {
            let viewport = max(settings.aa.yz, vec2<f32>(1.0));
            animate = length((prev_uv - uv) * viewport) < 0.25;
        }
    }
    return current_cloud_transfer(uv, animate);
}

fn cloud_lattice_texel(block: vec2<i32>, size: vec2<i32>, grid: i32, fresh: vec2<i32>) -> vec2<i32> {
    let max_block = (size - vec2<i32>(1)) / grid;
    return min(clamp(block, vec2<i32>(0), max_block) * grid + fresh, size - vec2<i32>(1));
}

struct CloudLattice {
    value: vec4<f32>,
    minimum: vec4<f32>,
    maximum: vec4<f32>,
};

// The interleaved march is stored compactly: texel (bx, by) of the march target
// is the fresh sample of block (bx, by), i.e. full-size texel
// (bx * grid + fresh.x, by * grid + fresh.y). Interpolating the four
// surrounding valid samples gives every pixel a
// current-frame estimate, which is what the single-pass resolve lacked: it
// returned stale history verbatim for three pixels in four, so those pixels had
// nothing pulling them back toward what was on screen and drifted until their
// turn came round again.
//
// The same four samples bound how far history may disagree with the current
// frame. They sit `grid` texels apart, so this box is looser than a full 3x3
// clamp: it catches gross disagreement reliably and subtle disagreement partly.
fn cloud_sample_lattice(
    texel: vec2<i32>,
    size: vec2<i32>,
    grid: i32,
    fresh: vec2<i32>,
    depth_aware: bool
) -> CloudLattice {
    let lattice = (vec2<f32>(texel) - vec2<f32>(fresh)) / f32(grid);
    let base = vec2<i32>(floor(lattice));
    let frac = fract(lattice);

    // Full-size texel positions of the four samples (for depth lookups) and
    // their compact block addresses (for the march fetch).
    let max_block = (size - vec2<i32>(1)) / grid;
    let b00 = clamp(base, vec2<i32>(0), max_block);
    let b10 = clamp(base + vec2<i32>(1, 0), vec2<i32>(0), max_block);
    let b01 = clamp(base + vec2<i32>(0, 1), vec2<i32>(0), max_block);
    let b11 = clamp(base + vec2<i32>(1, 1), vec2<i32>(0), max_block);
    let t00 = cloud_lattice_texel(base, size, grid, fresh);
    let t10 = cloud_lattice_texel(base + vec2<i32>(1, 0), size, grid, fresh);
    let t01 = cloud_lattice_texel(base + vec2<i32>(0, 1), size, grid, fresh);
    let t11 = cloud_lattice_texel(base + vec2<i32>(1, 1), size, grid, fresh);

    let c00 = textureLoad(cloud_march_texture, b00, 0);
    let c10 = textureLoad(cloud_march_texture, b10, 0);
    let c01 = textureLoad(cloud_march_texture, b01, 0);
    let c11 = textureLoad(cloud_march_texture, b11, 0);

    var out: CloudLattice;
    out.minimum = min(min(c00, c10), min(c01, c11));
    out.maximum = max(max(c00, c10), max(c01, c11));
    out.value = mix(mix(c00, c10, frac.x), mix(c01, c11, frac.x), frac.y);

    if (depth_aware) {
        // Contributors that marched against different geometry must not be
        // averaged together, or cloud from open sky bleeds across every
        // silhouette. Where they disagree, take the nearest-depth one whole.
        let inv = 1.0 / vec2<f32>(size);
        let centre = depth_at_uv((vec2<f32>(texel) + 0.5) * inv);
        let texels = array<vec2<i32>, 4>(t00, t10, t01, t11);
        let colors = array<vec4<f32>, 4>(c00, c10, c01, c11);
        var worst = 0.0;
        var nearest = c00;
        var nearest_delta = 1.0e30;
        for (var i = 0; i < 4; i = i + 1) {
            let d = depth_at_uv((vec2<f32>(texels[i]) + 0.5) * inv);
            let delta = abs(d - centre);
            worst = max(worst, delta);
            if (delta < nearest_delta) {
                nearest_delta = delta;
                nearest = colors[i];
            }
        }
        // Relative, because depth precision and scene scale both vary hugely.
        if (worst > max(centre, 1.0) * 0.02) {
            out.value = nearest;
        }
    }

    return out;
}

@fragment
fn fs_cloud_resolve(input: VertexOut) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(cloud_march_texture));
    let texel = clamp(vec2<i32>(input.position.xy), vec2<i32>(0), size - vec2<i32>(1));

    let temporal_enabled = settings.cloud_temporal.x > 0.5;
    let history_valid = settings.cloud_temporal.y > 0.5;
    if (!temporal_enabled || !history_valid) {
        // Nothing was interleaved, so every pixel already holds its own march.
        return textureLoad(cloud_march_texture, texel, 0);
    }

    let grid = i32(max(round(settings.cloud_temporal.w), 1.0));
    let pattern = i32(round(settings.cloud_temporal.z)) % (grid * grid);
    let fresh = vec2<i32>(pattern % grid, pattern / grid);
    let lattice = cloud_sample_lattice(
        texel,
        size,
        grid,
        fresh,
        settings.cloud_shadow.z > 0.5
    );

    let prev_uv = cloud_history_uv(input.uv);
    if (any(prev_uv < vec2<f32>(0.0)) || any(prev_uv > vec2<f32>(1.0))) {
        return lattice.value;
    }

    var confidence = 1.0;

    // The reference's rule, softened. It discards history on any camera change
    // at all because it has no motion vector to correct the sample with; this
    // port does reproject, so confidence falls off with how far the
    // reprojection had to reach. Measured in screen pixels: expressed in UV
    // this gate needed a pan of roughly fifty pixels per frame before it
    // engaged, so slow panning kept full history wherever the slider sat. At
    // full strength a single pixel of movement now rejects outright.
    let motion_reject = clamp(settings.cloud_temporal_tuning.y, 0.0, 1.0);
    if (motion_reject > 0.0) {
        let viewport = max(settings.aa.yz, vec2<f32>(1.0));
        let motion_px = length((prev_uv - input.uv) * viewport);
        confidence = confidence - smoothstep(0.05, 0.75, motion_px) * motion_reject;
    }

    // cloud_history_uv() follows one point at the middle of the shell, the only
    // depth it can know. Where solid geometry sits in front of that point the
    // march was clamped to the geometry instead, so this pixel's content belongs
    // to a surface a few units away while its motion vector was computed for one
    // tens of thousands of units away. Blending history into it drags that
    // surface's silhouette across the sky for as many frames as the blend keeps.
    if (settings.cloud_temporal_tuning.z > 0.5) {
        let ro = settings.camera_pos_time.xyz;
        let rd = cloud_world_ray(input.uv);
        let interval = cloud_shell_interval(ro, rd);
        if (interval.y > interval.x) {
            let shell_t = (interval.x + interval.y) * 0.5;
            if (cloud_scene_limit(input.uv, ro, rd) < shell_t * 0.98) {
                confidence = 0.0;
            }
        }
    }

    if (confidence <= 0.02) {
        return lattice.value;
    }

    // Clipping history into the range the current frame produced nearby is what
    // lets the blend run high without trails: a sample that survived the tests
    // above but still disagrees with what is on screen now decays in one frame
    // instead of over 1/(1-blend) of them. This replaces the delta-threshold
    // rejection the single-pass resolve used, which could not tell jitter noise
    // apart from genuine disocclusion and so had to be detuned until it caught
    // neither.
    var history = textureSample(cloud_transfer_texture, scene_sampler, prev_uv);
    history = clamp(history, lattice.minimum, lattice.maximum);

    let blend = clamp(settings.cloud_temporal_tuning.x, 0.0, 0.98);
    return mix(lattice.value, history, blend * confidence);
}

// The light a lit saber blade adds to this pixel, so the clouds can be
// composited under it instead of over it. The blade is additive FX light drawn
// over the sky and it writes no depth, so the cloud composite cannot tell it
// from sky: it would dim the blade by the cloud's transmittance and bury it
// under cloud radiance. The CPU projects each blade to a screen-space capsule
// (see cloud_foreground_uniform).
//
// The blade's own contribution is the pixel minus the sky beside the blade, so
// the estimate samples the scene just outside the glow. Removing the cloud
// around the blade instead would open a window onto the raw skybox behind it.
fn saber_glow_over_clouds(uv: vec2<f32>, scene_depth: f32, background: vec3<f32>) -> vec3<f32> {
    let count = u32(settings.cloud_foreground.x);
    if (count == 0u) {
        return vec3<f32>(0.0);
    }
    let viewport = max(settings.aa.yz, vec2<f32>(1.0));
    let pixel = uv * viewport;
    var weight = 0.0;
    var sky_centre = pixel;
    var sky_side = vec2<f32>(1.0, 0.0);
    var sky_reach = 0.0;
    for (var i = 0u; i < count; i = i + 1u) {
        let segment = settings.cloud_blades[i * 2u];
        let extent = settings.cloud_blades[i * 2u + 1u];
        // A blade behind solid geometry is not seen; leave the clouds alone.
        if (valid_depth(scene_depth) && scene_depth < extent.y - 4.0) {
            continue;
        }
        let along = segment.zw - segment.xy;
        let from_start = pixel - segment.xy;
        let h = clamp(dot(from_start, along) / max(dot(along, along), 1.0e-4), 0.0, 1.0);
        let closest = segment.xy + along * h;
        let offset = pixel - closest;
        let distance_px = length(offset);
        // The core is fully the blade's, the halo fades out over the glow radius.
        let w = 1.0 - smoothstep(extent.x * 0.35, extent.x, distance_px);
        if (w > weight) {
            weight = w;
            sky_centre = closest;
            sky_side = vec2<f32>(-along.y, along.x) / max(length(along), 1.0e-3);
            sky_reach = extent.x * 1.5;
        }
    }
    if (weight <= 0.0) {
        return vec3<f32>(0.0);
    }
    // Sky reference: just past the glow on both sides of the blade, averaged so
    // a sky gradient across the blade cancels instead of tinting one side. A
    // reference that lands on geometry is not sky and is left out.
    var sky = vec3<f32>(0.0);
    var sky_samples = 0.0;
    for (var side = 0u; side < 2u; side = side + 1u) {
        let direction = select(-1.0, 1.0, side == 0u);
        let reference_uv = clamp(
            (sky_centre + sky_side * sky_reach * direction) / viewport,
            vec2<f32>(0.0),
            vec2<f32>(1.0)
        );
        if (!valid_depth(depth_at_uv(reference_uv))) {
            sky += textureSampleLevel(scene_texture, scene_sampler, reference_uv, 0.0).rgb;
            sky_samples += 1.0;
        }
    }
    if (sky_samples <= 0.0) {
        return vec3<f32>(0.0);
    }
    return max(background - sky / sky_samples, vec3<f32>(0.0)) * weight;
}

// Projected cloud shadows are evaluated after additive FX have already entered
// scene_texture. Multiplying the whole pixel would therefore shadow the saber
// itself. Estimate only the blade's additive foreground contribution by
// sampling the same receiver just outside its screen-space capsule; callers can
// shadow the receiver normally and add this contribution back unshadowed.
fn saber_glow_over_receiver(uv: vec2<f32>, scene_depth: f32) -> vec3<f32> {
    let count = u32(settings.cloud_foreground.x);
    if (count == 0u || !valid_depth(scene_depth)) {
        return vec3<f32>(0.0);
    }
    let viewport = max(settings.aa.yz, vec2<f32>(1.0));
    let pixel = uv * viewport;
    var weight = 0.0;
    var blade_centre = pixel;
    var blade_side = vec2<f32>(1.0, 0.0);
    var blade_reach = 0.0;
    for (var i = 0u; i < count; i = i + 1u) {
        let segment = settings.cloud_blades[i * 2u];
        let extent = settings.cloud_blades[i * 2u + 1u];
        // Blade segment is behind the visible receiver at this pixel.
        if (scene_depth < extent.y - 4.0) {
            continue;
        }
        let along = segment.zw - segment.xy;
        let from_start = pixel - segment.xy;
        let h = clamp(dot(from_start, along) / max(dot(along, along), 1.0e-4), 0.0, 1.0);
        let closest = segment.xy + along * h;
        let distance_px = length(pixel - closest);
        let w = 1.0 - smoothstep(extent.x * 0.35, extent.x, distance_px);
        if (w > weight) {
            weight = w;
            blade_centre = closest;
            blade_side = vec2<f32>(-along.y, along.x) / max(length(along), 1.0e-3);
            blade_reach = extent.x * 1.5;
        }
    }
    if (weight <= 0.0) {
        return vec3<f32>(0.0);
    }

    var receiver = vec3<f32>(0.0);
    var receiver_samples = 0.0;
    for (var side = 0u; side < 2u; side = side + 1u) {
        let direction = select(-1.0, 1.0, side == 0u);
        let reference_uv = clamp(
            (blade_centre + blade_side * blade_reach * direction) / viewport,
            vec2<f32>(0.0),
            vec2<f32>(1.0)
        );
        let reference_depth = depth_at_uv(reference_uv);
        // Only compare against the same local receiver. This rejects samples
        // that crossed a silhouette/edge while stepping outside the glow.
        if (valid_depth(reference_depth)
            && abs(reference_depth - scene_depth) <= max(scene_depth, 1.0) * 0.02) {
            receiver += textureSampleLevel(scene_texture, scene_sampler, reference_uv, 0.0).rgb;
            receiver_samples += 1.0;
        }
    }
    if (receiver_samples <= 0.0) {
        return vec3<f32>(0.0);
    }
    let centre = textureSampleLevel(scene_texture, scene_sampler, uv, 0.0).rgb;
    return max(centre - receiver / receiver_samples, vec3<f32>(0.0)) * weight;
}

fn render_clouds(background: vec3<f32>, uv: vec2<f32>) -> vec3<f32> {
    if (settings.clouds.x <= 0.5) {
        return background;
    }

    // The cloud transfer buffer may be half/quarter resolution, so bilinear
    // upsampling can straddle a thin opaque or alpha-tested silhouette. Gate
    // the composite again at full display resolution: if the visible scene
    // point is in front of the cloud volume, no neighboring cloud texel is
    // allowed to wash over that pixel.
    let scene_depth = depth_at_uv(uv);
    if (valid_depth(scene_depth)) {
        let ro = settings.camera_pos_time.xyz;
        let rd = cloud_world_ray(uv);
        let interval = cloud_shell_interval(ro, rd);
        if (interval.y <= interval.x || cloud_scene_limit(uv, ro, rd) <= interval.x + CLOUD_SHELL_EPSILON) {
            return background;
        }
    }

    var transfer = textureSample(cloud_transfer_texture, scene_sampler, uv);
    if (settings.underwater.x > -1.0e20) {
        // The camera is under the ocean, so the sky (and any cloud in it) is seen
        // through the surface. The background already carries the water's
        // absorption from the optics pass; the clouds are composited after it
        // and would otherwise punch through at full strength.
        let seen = cloud_seen_through_water(cloud_world_ray(uv));
        transfer = vec4<f32>(transfer.rgb * seen.rgb * seen.a, mix(1.0, transfer.a, seen.a));
    }
    // The blade sits in front of the cloud: everything else is dimmed by the
    // cloud, but the blade's own light is not.
    let transmittance = clamp(transfer.a, 0.0, 1.0);
    let blade = saber_glow_over_clouds(uv, scene_depth, background);
    return background * transmittance + max(transfer.rgb, vec3<f32>(0.0))
        + blade * (1.0 - transmittance);
}

// How much of a cloud in direction `rd` reaches a camera under the water surface:
// Beer-Lambert absorption over the path up to the surface (same channel ratios as
// ocean_optics.wgsl), and the window of sky that is visible at all. Beyond the
// critical angle (asin(1/1.333), 48.6 degrees off vertical) the surface is a mirror
// showing the underwater scene, not the sky. rgb: transmission, a: window.
fn cloud_seen_through_water(rd: vec3<f32>) -> vec4<f32> {
    if (rd.y <= 0.0) {
        return vec4<f32>(0.0);
    }
    let rise = max(settings.underwater.x - settings.camera_pos_time.y, 0.0);
    let path = rise / max(rd.y, 0.02);
    let ratios = vec3<f32>(38.0 / 7.5, 38.0 / 22.0, 1.0);
    let transmission = exp2(-4.321928 * ratios * path / max(settings.underwater.y, 1.0));
    let window = smoothstep(0.58, 0.72, rd.y);
    return vec4<f32>(transmission, window);
}

fn apply_volumetric_fog(
    color: vec3<f32>,
    uv: vec2<f32>,
    depth: f32
) -> vec3<f32> {
    if (settings.scene.y <= 0.5 || !valid_depth(depth)) {
        return color;
    }
    // linear_depth includes depth-writing entities, so a player integrates fog
    // only up to itself rather than through to the BSP behind it.
    let volume = sample_integrated_fog(uv, min(depth, FROXEL_FAR));
    let transmittance = clamp(volume.a, 0.0, 1.0);
    return color * transmittance + max(volume.rgb, vec3<f32>(0.0));
}

fn rain_haze_field(uv: vec2<f32>) -> vec2<f32> {
    let mask_dims = textureDimensions(rain_haze_mask, 0);
    if (mask_dims.x == 0u || mask_dims.y == 0u) {
        return vec2<f32>(1.0, 0.5);
    }
    let field = textureSampleLevel(
        rain_haze_mask,
        scene_sampler,
        clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)),
        0.0
    ).rg;
    return clamp(field, vec2<f32>(0.0), vec2<f32>(1.0));
}

fn apply_rain_haze(color: vec3<f32>, uv: vec2<f32>, world: vec3<f32>, depth: f32) -> vec3<f32> {
    if (settings.rain.x <= 0.5 || !valid_depth(depth)) {
        return color;
    }

    // Individual streaks only need to sell the near/mid field. At distance, cheap
    // atmospheric extinction conveys the enormous number of unresolved droplets.
    // Tint from the authored sun color so sunset/storm lighting is preserved instead
    // of turning the world into generic gray fog.
    let intensity = clamp(settings.rain.y, 0.0, 1.0);
    // Haze is a property of the authored rain strength, not a separate user
    // control. Rust supplies the preset-derived value in settings.rain.z.
    let haze_strength = clamp(settings.rain.z, 0.0, 1.0);
    let haze_field = rain_haze_field(uv);
    let exposure = haze_field.x;
    let gust = haze_field.y;
    if (exposure <= 0.001) {
        return color;
    }
    // Still intentionally cheap depth haze, not the froxel volumetric system.
    // Heavy rain starts a little nearer and ramps density non-linearly so it
    // reads as a real downpour without adding another volumetric pass.
    let heavy = smoothstep(0.72, 1.0, intensity);
    let haze_start = mix(768.0, 620.0, heavy);
    let distance = max(depth - haze_start, 0.0);
    let density = mix(0.000110, 0.000335, intensity) * mix(1.0, 1.34, heavy) * haze_strength;
    // Replace the old independent sine curtain with the same low-frequency
    // GodotGrass gust field that bends grass and angles the rain. The field was
    // already evaluated in the 64-wide haze pass and packed into haze_field.y.
    let curtain = mix(0.78, 1.0, gust);
    let extinction = 1.0 - exp(-distance * density * curtain);
    let maximum = mix(0.31, 0.74, intensity) * mix(1.0, 1.10, heavy) * haze_strength;
    let amount = clamp(extinction, 0.0, maximum) * exposure;

    // Rain air is a cool blue-grey. It only borrows the sun's brightness, never its
    // colour: a warm sun must not turn distant walls pink.
    let sun_tint = max(settings.cloud_sun_color.rgb, vec3<f32>(0.04));
    let sun_brightness = clamp(luminance(sun_tint), 0.0, 1.0);
    let rain_air = vec3<f32>(0.39, 0.47, 0.57) * mix(0.94, 1.0, sun_brightness);
    let scene_luma = luminance(color);
    let haze_color = rain_air * mix(0.62, 1.08, clamp(scene_luma, 0.0, 1.0));
    return mix(color, haze_color, amount);
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let gamma = max(settings.color.x, 0.05);
    let tonemap_enabled = settings.color.y > 0.5;
    let bloom_enabled = settings.color.z > 0.5;
    let ssao_enabled = settings.color.w > 0.5;
    let halation_enabled = settings.film.x > 0.5;
    let fxaa_enabled = settings.aa.x > 0.5;
    let viewport = max(settings.aa.yz, vec2<f32>(1.0));
    let texel = 1.0 / viewport;

    let dims = textureDimensions(linear_depth_texture);
    let dims_f = vec2<f32>(f32(dims.x), f32(dims.y));
    let max_pixel = vec2<i32>(i32(dims.x) - 1, i32(dims.y) - 1);
    let raw_pixel = vec2<i32>(
        i32(input.uv.x * dims_f.x),
        i32(input.uv.y * dims_f.y)
    );
    let pixel = clamp(raw_pixel, vec2<i32>(0), max_pixel);
    let centre_depth = depth_at_pixel(pixel);

    var color = scene(input.uv);

    if (settings.taa_params.y > 0.5) {
        color = reflection_debug_color(pixel, centre_depth, color);
        return vec4<f32>(color, 1.0);
    }

    if (fxaa_enabled) {
        color = fxaa_color(input.uv, texel);
    }

    color = camera_motion_blur(input.uv, color, centre_depth);
    color = depth_of_field(input.uv, color, centre_depth);

    if (valid_depth(centre_depth)) {
        let world = world_position(input.uv, centre_depth);
        let normal = surface_normal(pixel);
        let cloud_shadow = projected_cloud_shadow_factor(world, normal, pixel);
        let saber_foreground = saber_glow_over_receiver(input.uv, centre_depth);
        color = color * cloud_shadow + saber_foreground * (1.0 - cloud_shadow);
        let puddle = post_puddle_amount(world, normal.y);
        color = finalize_puddle_film(color, world, puddle);
        color = temporal_ssr_color(pixel, centre_depth, color, puddle);
        let contact_factor = contact_shadow_factor(pixel, world, centre_depth);
        if (settings.scene.x > 1.5) {
            return vec4<f32>(contact_shadow_debug_color(contact_factor), 1.0);
        }
        color *= contact_factor;
        if (ssao_enabled) {
            color *= ssao_factor(pixel, centre_depth);
        }
        color = apply_volumetric_fog(color, input.uv, centre_depth);
    }

    // The transfer buffer is depth-clipped, so the same composite now handles
    // sky pixels, clouds behind scene geometry, and low clouds physically in
    // front of opaque/alpha-tested surfaces.
    color = render_clouds(color, input.uv);

    if (bloom_enabled) {
        let levels = bloom_levels(input.uv);
        // Rain scatters light: lamps and signs bloom wider in wet air.
        color += bloom_color(levels) * (0.32 * (1.0 + 0.6 * settings.weather_look.y));
        if (halation_enabled) {
            let red_return = halation_red_levels(levels);
            color = apply_film_halation(color, red_return);
        }
    } else if (halation_enabled) {
        let red_return = halation_red(input.uv);
        color = apply_film_halation(color, red_return);
    }

    color *= exp2(settings.grain.w + auto_exposure_state.exposure_ev);
    if (tonemap_enabled) {
        color = aces_fitted(color);
    }

    color = apply_legacy1_global_fog(color, input.uv, pixel, centre_depth);
    color = pow(max(color, vec3<f32>(0.0)), vec3<f32>(1.0 / gamma));
    color = purple_fringe_color(input.uv, color);
    color = apply_rain_grade(color, settings.weather_look.y);
    color = apply_color_lut(color);
    if (settings.film.z > 0.5) {
        let edge = smoothstep(0.28, 0.78, distance(input.uv, vec2<f32>(0.5)));
        color *= 1.0 - edge * 0.28;
    }
    color = film_grain(color, input.uv);
    return vec4<f32>(color, 1.0);
}

struct TaaFragmentOut {
    @location(0) display: vec4<f32>,
    @location(1) history: vec4<f32>,
};

@fragment
fn fs_main_taa(input: VertexOut) -> TaaFragmentOut {
    let gamma = max(settings.color.x, 0.05);
    let tonemap_enabled = settings.color.y > 0.5;
    let bloom_enabled = settings.color.z > 0.5;
    let ssao_enabled = settings.color.w > 0.5;
    let halation_enabled = settings.film.x > 0.5;
    let fxaa_enabled = settings.aa.x > 0.5;
    let viewport = max(settings.aa.yz, vec2<f32>(1.0));
    let texel = 1.0 / viewport;

    let dims = textureDimensions(linear_depth_texture);
    let dims_f = vec2<f32>(f32(dims.x), f32(dims.y));
    let max_pixel = vec2<i32>(i32(dims.x) - 1, i32(dims.y) - 1);
    let raw_pixel = vec2<i32>(
        i32(input.uv.x * dims_f.x),
        i32(input.uv.y * dims_f.y)
    );
    let pixel = clamp(raw_pixel, vec2<i32>(0), max_pixel);
    let centre_depth = depth_at_pixel(pixel);

    if (settings.taa_params.y > 0.5) {
        let debug_color = reflection_debug_color(pixel, centre_depth, scene(input.uv));
        var debug_out: TaaFragmentOut;
        debug_out.display = vec4<f32>(debug_color, 1.0);
        debug_out.history = vec4<f32>(debug_color, 1.0);
        return debug_out;
    }

    let taa = taa_resolve_bevy(input.uv);
    let temporal_history = taa.color;
    var color = taa_display_color(taa.color);

    if (fxaa_enabled) {
        color = fxaa_color(input.uv, texel);
    }

    color = camera_motion_blur(input.uv, color, centre_depth);
    color = depth_of_field(input.uv, color, centre_depth);

    if (valid_depth(centre_depth)) {
        let world = world_position(input.uv, centre_depth);
        let normal = surface_normal(pixel);
        let cloud_shadow = projected_cloud_shadow_factor(world, normal, pixel);
        let saber_foreground = saber_glow_over_receiver(input.uv, centre_depth);
        color = color * cloud_shadow + saber_foreground * (1.0 - cloud_shadow);
        let puddle = post_puddle_amount(world, normal.y);
        color = finalize_puddle_film(color, world, puddle);
        color = temporal_ssr_color(pixel, centre_depth, color, puddle);
        let contact_factor = contact_shadow_factor(pixel, world, centre_depth);
        if (settings.scene.x > 1.5) {
            let debug_color = vec4<f32>(contact_shadow_debug_color(contact_factor), 1.0);
            return TaaFragmentOut(debug_color, debug_color);
        }
        color *= contact_factor;
        if (ssao_enabled) {
            color *= ssao_factor(pixel, centre_depth);
        }
        color = apply_volumetric_fog(color, input.uv, centre_depth);
    }

    // The transfer buffer is depth-clipped, so the same composite now handles
    // sky pixels, clouds behind scene geometry, and low clouds physically in
    // front of opaque/alpha-tested surfaces.
    color = render_clouds(color, input.uv);

    if (bloom_enabled) {
        let levels = bloom_levels(input.uv);
        // Rain scatters light: lamps and signs bloom wider in wet air.
        color += bloom_color(levels) * (0.32 * (1.0 + 0.6 * settings.weather_look.y));
        if (halation_enabled) {
            let red_return = halation_red_levels(levels);
            color = apply_film_halation(color, red_return);
        }
    } else if (halation_enabled) {
        let red_return = halation_red(input.uv);
        color = apply_film_halation(color, red_return);
    }

    color *= exp2(settings.grain.w + auto_exposure_state.exposure_ev);
    if (tonemap_enabled) {
        color = aces_fitted(color);
    }

    color = apply_legacy1_global_fog(color, input.uv, pixel, centre_depth);
    color = pow(max(color, vec3<f32>(0.0)), vec3<f32>(1.0 / gamma));
    color = purple_fringe_color(input.uv, color);
    color = apply_rain_grade(color, settings.weather_look.y);
    color = apply_color_lut(color);
    if (settings.film.z > 0.5) {
        let edge = smoothstep(0.28, 0.78, distance(input.uv, vec2<f32>(0.5)));
        color *= 1.0 - edge * 0.28;
    }
    color = film_grain(color, input.uv);
    var out: TaaFragmentOut;
    out.display = vec4<f32>(color, 1.0);
    out.history = vec4<f32>(temporal_history, taa.history_confidence);
    return out;
}
