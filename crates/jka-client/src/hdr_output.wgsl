// Input is linear BT.709 in units of paper white, after scene/UI composition.
struct OutputSettings { values: vec4<f32> }; // encoding, paper white nits, peak nits, rolloff
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var<uniform> settings: OutputSettings;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -3.0), vec2<f32>(-1.0, 1.0), vec2<f32>(3.0, 1.0));
    return vec4<f32>(p[index], 0.0, 1.0);
}

fn pq(nits: vec3<f32>) -> vec3<f32> {
    let p = pow(clamp(nits / 10000.0, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(2610.0 / 16384.0));
    return pow((vec3<f32>(3424.0 / 4096.0) + (2413.0 / 128.0) * p)
        / (vec3<f32>(1.0) + (2392.0 / 128.0) * p), vec3<f32>(2523.0 / 32.0));
}

fn srgb(linear: vec3<f32>) -> vec3<f32> {
    let c = max(linear, vec3<f32>(0.0));
    return select(1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055), 12.92 * c, c <= vec3<f32>(0.0031308));
}

fn sdr_tonemap(c: vec3<f32>) -> vec3<f32> {
    return clamp((c * (2.51 * c + vec3<f32>(0.03)))
        / (c * (2.43 * c + vec3<f32>(0.59)) + vec3<f32>(0.14)), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn hdr_rolloff(c: vec3<f32>, headroom: f32) -> vec3<f32> {
    // Preserve diffuse/reference white. Compress only highlights, without
    // per-channel clipping that would change their hue before gamut conversion.
    let brightest = max(c.r, max(c.g, c.b));
    if (brightest <= 1.0) { return c; }
    let shoulder = max(headroom - 1.0, 0.0001);
    let mapped = 1.0 + shoulder * (1.0 - exp(-(brightest - 1.0) / shoulder));
    return c * mapped / brightest;
}

fn encode_output(position: vec4<f32>) -> vec4<f32> {
    var color = max(textureLoad(image, vec2<i32>(position.xy), 0).rgb, vec3<f32>(0.0));
    let encoding = u32(settings.values.x);
    if (encoding == 0u || encoding == 3u) {
        let strength = clamp(settings.values.w, 0.0, 1.0);
        if (strength > 0.0) {
            color = mix(color, sdr_tonemap(color), strength);
        }
        if (encoding == 3u) { color = srgb(color); }
        return vec4<f32>(color, 1.0);
    }
    let white = max(settings.values.y, 80.0);
    let peak = max(settings.values.z, white);
    let rolloff = clamp(settings.values.w, 0.0, 1.0);
    if (rolloff > 0.0) {
        color = mix(color, hdr_rolloff(color, peak / white), rolloff);
    }
    color = min(color * white, vec3<f32>(peak));
    if (encoding == 1u) {
        // Windows scRGB defines linear 1.0 as 80 cd/m^2.
        return vec4<f32>(color / 80.0, 1.0);
    }
    // Linear BT.709/D65 -> BT.2020/D65, then ST-2084 (absolute 10,000-nit PQ).
    let rec2020 = mat3x3<f32>(
        vec3<f32>(0.6274040, 0.0690970, 0.0163916),
        vec3<f32>(0.3292820, 0.9195400, 0.0880132),
        vec3<f32>(0.0433136, 0.0113612, 0.8955950)) * color;
    return vec4<f32>(pq(rec2020), 1.0);
}

@fragment
fn fs_output(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return encode_output(position);
}

@fragment
fn fs_ui(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // Egui blends its preferred gamma-space framebuffer using premultiplied
    // alpha. Unpremultiply BEFORE decoding, then premultiply in linear light;
    // decoding an already-premultiplied white incorrectly darkens translucent UI.
    let ui = textureLoad(image, vec2<i32>(position.xy), 0);
    if (ui.a <= 0.00001) { return vec4<f32>(0.0); }
    let gamma = clamp(ui.rgb / ui.a, vec3<f32>(0.0), vec3<f32>(1.0));
    let linear = select(pow((gamma + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4)), gamma / 12.92, gamma <= vec3<f32>(0.04045));
    return vec4<f32>(linear * ui.a, ui.a);
}

// Render Studio's isolated character capture retains the scene alpha instead
// of forcing it opaque like the normal desktop HDR output. The RGB transfer
// curve is identical to SDR screenshot capture.
@fragment
fn fs_capture_alpha(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let scene = textureLoad(image, vec2<i32>(position.xy), 0);
    let coverage = clamp(scene.a, 0.0, 1.0);
    // Tone-map normal translucent material in straight light. Uncovered
    // additive emission instead uses its actual intensity, not a division by
    // zero coverage. Then convert the displayed light to ordinary RGBA.
    let reference_alpha = select(1.0,coverage,coverage > 0.00001);
    let rgb = max(scene.rgb,vec3<f32>(0.0))/reference_alpha;
    let strength = clamp(settings.values.w, 0.0, 1.0);
    let premult = clamp(mix(rgb,sdr_tonemap(rgb),strength)*reference_alpha,vec3<f32>(0.0),vec3<f32>(1.0));
    let minimum_alpha = max(coverage,max(premult.r,max(premult.g,premult.b)));
    let alpha = ceil(minimum_alpha*255.0)/255.0;
    return vec4<f32>(premult/max(alpha,0.00001),alpha);
}
