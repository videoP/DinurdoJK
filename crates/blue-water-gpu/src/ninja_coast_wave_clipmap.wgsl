
struct NinjaWaveClipmap {
    window: vec4<f32>, // camera xz in metres, finest extent in metres, level count
    detail: vec4<f32>, // x FFT detail applied at bake time
}

@group(0) @binding(0) var<uniform> ninja_wave_bake: NinjaWaveClipmap;
@group(0) @binding(1) var ninja_wave_output: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(2) var ninja_wave_previous_mip: texture_2d_array<f32>;

@group(5) @binding(0) var<uniform> ninja_wave_clip: NinjaWaveClipmap;
@group(5) @binding(1) var ninja_wave_cache: texture_2d_array<f32>;
@group(5) @binding(2) var ninja_wave_sampler: sampler;

fn ninja_wave_center(window: vec4<f32>, cell: f32) -> vec2<f32> {
    return floor(window.xy / cell + 0.5) * cell;
}

fn ninja_wave_slope(sample: JkaFastWave) -> vec2<f32> {
    return -sample.normal.xz / max(sample.normal.y, 0.05);
}

@compute @workgroup_size(8, 8)
fn ninja_wave_bake_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let width = f32(textureDimensions(ninja_wave_output).x);
    let window = ninja_wave_bake.window;
    let levels = u32(window.w);
    let extent = window.z * exp2(f32(id.z % levels));
    let cell = extent / width;
    let xz = ninja_wave_center(window, cell) + (vec2<f32>(id.xy) + 0.5) * cell - 0.5 * extent;
    let dx = vec2<f32>(cell, 0.0);
    let dy = vec2<f32>(0.0, cell);
    if id.z >= levels {
        let slope = ninja_coast_wave_slope_grad(xz, xz, 0.0, true, 0u, dx, dy, ninja_wave_bake.detail.x);
        textureStore(ninja_wave_output, vec2<i32>(id.xy), id.z, vec4<f32>(slope, 0.0, 0.0));
        return;
    }
    let wave = jka_fast_wave_grad(xz, 0.0, dx, dy, ninja_wave_bake.detail.x);
    let slope = ninja_wave_slope(wave);
    // Store a second moment so filtering resolved slopes adds back roughness.
    textureStore(ninja_wave_output, vec2<i32>(id.xy), id.z,
        vec4<f32>(slope, wave.variance + dot(slope, slope), wave.foam));
}

@compute @workgroup_size(8, 8)
fn ninja_wave_mip(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= textureDimensions(ninja_wave_output)) { return; }
    let lo = vec2<i32>(id.xy * 2u);
    let sum = textureLoad(ninja_wave_previous_mip, lo, id.z, 0)
        + textureLoad(ninja_wave_previous_mip, lo + vec2<i32>(1, 0), id.z, 0)
        + textureLoad(ninja_wave_previous_mip, lo + vec2<i32>(0, 1), id.z, 0)
        + textureLoad(ninja_wave_previous_mip, lo + vec2<i32>(1, 1), id.z, 0);
    textureStore(ninja_wave_output, vec2<i32>(id.xy), id.z, sum * 0.25);
}

fn ninja_wave_read(xz: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>, layer: u32, offset: u32) -> vec4<f32> {
    let window = ninja_wave_clip.window;
    let extent = window.z * exp2(f32(layer));
    let width = f32(textureDimensions(ninja_wave_cache).x);
    let uv = (xz - ninja_wave_center(window, extent / width)) / extent + 0.5;
    return textureSampleGrad(ninja_wave_cache, ninja_wave_sampler, uv, layer + offset, dx / extent, dy / extent);
}

fn ninja_wave_bound(xz: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>) -> f32 {
    let delta = abs(xz - ninja_wave_clip.window.xy);
    return max(delta.x, delta.y) + max(length(dx), length(dy));
}

fn ninja_wave_level(bound: f32, extent: f32) -> u32 {
    return u32(ceil(log2(max(bound / (extent * 0.48), 1.0))));
}

fn ninja_cached_wave(xz: vec2<f32>, calm: f32, dx: vec2<f32>, dy: vec2<f32>, fft_detail: f32) -> JkaFastWave {
    let window = ninja_wave_clip.window;
    let bound = ninja_wave_bound(xz, dx, dy);
    let layer = ninja_wave_level(bound, window.z);
    if calm > 0.0 || layer >= u32(window.w) {
        return jka_fast_wave_grad(xz, calm, dx, dy, fft_detail);
    }
    // The outer 12.5% of each level blends into its next coarser neighbour.
    // At the last edge, fade to the unbounded FFT field.
    let extent = window.z * exp2(f32(layer));
    let share = smoothstep(0.42, 0.48, bound / extent);
    var packed = ninja_wave_read(xz, dx, dy, layer, 0u);
    if share > 0.0 && layer + 1u < u32(window.w) {
        packed = mix(packed, ninja_wave_read(xz, dx, dy, layer + 1u, 0u), share);
    }
    let normal = normalize(vec3<f32>(-packed.x, 1.0, -packed.y));
    let variance = max(packed.z - dot(packed.xy, packed.xy), 0.0);
    // Foam coverage includes the baked Jacobian correction. Windrow gathering
    // uses unit density here to keep the four filterable channels in one read.
    if share > 0.0 && layer + 1u == u32(window.w) {
        let direct = jka_fast_wave_grad(xz, calm, dx, dy, fft_detail);
        return JkaFastWave(normalize(mix(normal, direct.normal, share)),
            mix(packed.w, direct.foam, share), mix(variance, direct.variance, share),
            mix(1.0, direct.jacobian, share), 1.0);
    }
    return JkaFastWave(normal, packed.w, variance, 1.0, 1.0);
}

fn ninja_cached_shore_slope(xz: vec2<f32>, at: vec2<f32>, probe2: u32, dx: vec2<f32>, dy: vec2<f32>) -> vec2<f32> {
    let window = ninja_wave_clip.window;
    let levels = u32(window.w);
    let bound = ninja_wave_bound(xz, dx, dy);
    let layer = ninja_wave_level(bound, window.z);
    let fft_detail = ninja_wave_clip.detail.x;
    if layer >= levels || (probe2 & 511u) != 0u || hull_sea_active() {
        return ninja_coast_wave_slope_grad(xz, at, 0.0, true, probe2, dx, dy, fft_detail);
    }
    let slope = ninja_wave_read(xz, dx, dy, layer, levels).xy;
    let extent = window.z * exp2(f32(layer));
    let share = smoothstep(0.42, 0.48, bound / extent);
    if share <= 0.0 { return slope; }
    if layer + 1u < levels {
        return mix(slope, ninja_wave_read(xz, dx, dy, layer + 1u, levels).xy, share);
    }
    return mix(slope, ninja_coast_wave_slope_grad(xz, at, 0.0, true, probe2, dx, dy, fft_detail), share);
}
