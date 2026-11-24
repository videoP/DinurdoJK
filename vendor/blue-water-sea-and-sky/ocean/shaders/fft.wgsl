// Ports the transform passes of the web game's `ocean/waves/field.ts` (`evolve`, `pass`, `stockham`) and the layout of
// the web game's `ocean/waves/fft.ts`: the spectrum evolved to the current time and inverse-transformed with Stockham passes.
//
// The frequency-domain atlas holds the cascades side by side (N·count × N texels), two packed complex signals per
// texel (`Pair`): (Dx + i·Dy, Dz + i·∂Dx/∂z) and (∂y/∂x + i·∂y/∂z, ∂Dx/∂x + i·∂Dz/∂z). For a pass of radix R after
// sub-transforms of length p, output j is
//
//   y[j] = Σ_{r<R} x[i + r·N/R] · e^{2πi·r·q/(R·p)},  q = j mod R·p,  i = ⌊j/(R·p)⌋·p + j mod p,
//
// and the passes run with p = 1, R₁, R₁R₂, … so the result lands in natural order. Rows first (the first row pass
// evolves the spectrum as its input), then columns; the last column pass is `resolve.wgsl`.

struct CascadeParams {
    // x: wavenumber step 2π/size, y: foam e-folding lifetime (s), z: breaking threshold, w: bubbles' lifetime (s).
    a: vec4<f32>,
    // xy: breaking indicator weights (compression, forward face), zw: this update's spreading gain along x and z.
    b: vec4<f32>,
    // Gale foam streaks (resolve.wgsl): xy the surface's drift (texels/s), z their e-folding life (s, 0 none), w how fast
    // whitecap foam feeds them (1/s).
    c: vec4<f32>,
    // xy the lines they gather into (cycles per tile along x and z), z how fast they gather (texels/s), w the lines'
    // meander (radians).
    d: vec4<f32>,
}

struct FieldParams {
    // x: phase (time / FOLD_PERIOD, 0–1), y: choppiness, z: elapsed seconds (0 on a paused frame), w: the streak lines'
    // meander phase (radians).
    time: vec4<f32>,
    // xy: unit vector the wind blows toward, z: N, w: cascade count.
    wind: vec4<f32>,
    cascades: array<CascadeParams, 3>,
    // DinurdoJK: xyz = 1 when cascade 0/1/2 carries visible waves. An inactive cascade's FFT is skipped and resolve
    // writes zeros for it (WaveField::rebuild_spectrum).
    cascade_on: vec4<f32>,
}

struct PassParams {
    radix: u32,
    span: u32,
    // 1 for a row pass, 0 for a column pass.
    horizontal: u32,
    // 1 when the pass evolves the spectrum as its input.
    evolve: u32,
}

struct Pair {
    a: vec4<f32>,
    b: vec4<f32>,
}

@group(0) @binding(0) var<uniform> field: FieldParams;
@group(0) @binding(1) var<uniform> pass_params: PassParams;
// Per atlas texel (Re a, Im a, ±m, 0): the texel's wave is a·e^{∓iωt}, ω = 2π·m/FOLD_PERIOD.
@group(0) @binding(2) var<storage, read> spectrum: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> source: array<Pair>;
@group(0) @binding(4) var<storage, read_write> destination: array<Pair>;

const TAU: f32 = 6.283185307179586;

// e^{iθ}-rotation of two packed complex numbers (xy, zw).
fn rotate(value: vec4<f32>, c: f32, s: f32) -> vec4<f32> {
    return value * c + vec4<f32>(-value.y, value.x, -value.w, value.z) * s;
}

// The eight fields' spectra at one atlas texel, packed as four complex signals. Choppy displacement is +i·k̂·λ·H so
// crests sharpen as in Gerstner waves.
fn evolve(column: i32, row: i32, tile: i32) -> Pair {
    let n = i32(field.wind.z);
    let width = n * i32(field.wind.w);
    let half = n / 2;
    let a = spectrum[row * width + tile * n + column];
    // H = a·e^{−2πi·m·phase}; m·phase is split as 256·high + low so float32 keeps ~1e-5 of a turn.
    let phase = field.time.x;
    let high = floor(a.z / 256.0);
    let low = a.z - high * 256.0;
    let turn = fract(high * fract(phase * 256.0) + low * phase) * TAU;
    let c = cos(turn);
    let s = sin(turn);
    let h = vec2<f32>(a.x * c + a.y * s, a.y * c - a.x * s);
    let ih = vec2<f32>(-h.y, h.x);
    let wavenumber = field.cascades[tile].a.x;
    let kx = f32(select(column - n, column, column < half)) * wavenumber;
    let kz = f32(select(row - n, row, row < half)) * wavenumber;
    let k = max(sqrt(kx * kx + kz * kz), 1e-6);
    let lambda = field.time.y;
    let x = lambda * kx / k;
    let z = lambda * kz / k;
    return Pair(
        vec4<f32>(ih * (x + 1.0), ih * (z * (1.0 - kx))),
        vec4<f32>(ih * kx - h * kz, (h * (kx * kx) + ih * (kz * kz)) * (-lambda / k)),
    );
}

@compute @workgroup_size(16, 16, 1)
fn transform(@builtin(global_invocation_id) id: vec3<u32>) {
    let n = i32(field.wind.z);
    let width = n * i32(field.wind.w);
    let pixel = vec2<i32>(id.xy);
    if pixel.x >= width || pixel.y >= n {
        return;
    }
    let radix = i32(pass_params.radix);
    let span = i32(pass_params.span);
    let horizontal = pass_params.horizontal == 1u;
    let tile = pixel.x / n;
    if field.cascade_on[tile] < 0.5 {
        return;
    }
    let j = select(pixel.y, pixel.x - tile * n, horizontal);
    let block = radix * span;
    let first = (j / block) * span + j % span;
    let angle = f32(j % block) * (TAU / f32(block));
    var sum_a = vec4<f32>(0.0);
    var sum_b = vec4<f32>(0.0);
    for (var r = 0; r < radix; r++) {
        let index = first + r * (n / radix);
        var value: Pair;
        if pass_params.evolve == 1u {
            value = evolve(index, pixel.y, tile);
        } else if horizontal {
            value = source[pixel.y * width + tile * n + index];
        } else {
            value = source[index * width + pixel.x];
        }
        let turn = angle * f32(r);
        let c = cos(turn);
        let s = sin(turn);
        sum_a += rotate(value.a, c, s);
        sum_b += rotate(value.b, c, s);
    }
    destination[pixel.y * width + pixel.x] = Pair(sum_a, sum_b);
}
