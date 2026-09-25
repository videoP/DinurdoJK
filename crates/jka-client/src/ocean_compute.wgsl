const PI: f32 = 3.141592653589793;
const G: f32 = 9.81;
const MAX_MAP_SIZE: u32 = 1024u;
const NUM_SPECTRA: u32 = 4u;
const NUM_CASCADES: u32 = 3u;

struct CascadeParams {
    tile: vec4<f32>,          // xy tile length (m), zw unused
    spectrum: vec4<f32>,      // alpha, peak omega, wind speed, wind angle radians
    shape: vec4<f32>,         // depth, swell, detail, spread
    foam: vec4<f32>,          // whitecap, grow, decay, unused
    seed: vec4<u32>,          // xy seed, zw unused
    time: vec4<f32>,          // x simulation time
};

struct OceanState {
    values: vec4<u32>,        // active cascade, map size, cascade count, unused
};

@group(0) @binding(0) var<storage, read> cascade_params: array<CascadeParams>;
@group(0) @binding(1) var<storage, read_write> spectrum_data: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> butterfly_data: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> fft_data: array<vec2<f32>>;
@group(0) @binding(4) var<storage, read_write> foam_data: array<f32>;
@group(0) @binding(5) var displacement_map: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(6) var normal_map: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(7) var<uniform> state: OceanState;

fn map_size() -> u32 { return state.values.y; }
fn active_cascade() -> u32 { return state.values.x; }
fn complex_mul(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(a.x*b.x - a.y*b.y, a.x*b.y + a.y*b.x);
}
fn complex_conj(a: vec2<f32>) -> vec2<f32> { return vec2<f32>(a.x, -a.y); }
fn complex_exp(x: f32) -> vec2<f32> { return vec2<f32>(cos(x), sin(x)); }

fn hash2(x: vec2<u32>) -> vec2<f32> {
    var h32 = x.y + 374761393u + x.x*3266489917u;
    h32 = 2246822519u * (h32 ^ (h32 >> 15u));
    h32 = 3266489917u * (h32 ^ (h32 >> 13u));
    let n = h32 ^ (h32 >> 16u);
    let rz = vec2<u32>(n, n*48271u);
    return vec2<f32>((rz >> vec2<u32>(1u)) & vec2<u32>(0x7fffffffu)) / f32(0x7fffffffu);
}
fn gaussian(x: vec2<f32>) -> vec2<f32> {
    let safe_x = max(x.x, 1e-7);
    let r = sqrt(-2.0 * log(safe_x));
    let theta = 2.0*PI*x.y;
    return vec2<f32>(r*cos(theta), r*sin(theta));
}
fn dispersion(k: f32, depth: f32) -> vec2<f32> {
    let a = k*depth;
    let b = tanh(a);
    let w = sqrt(G*k*b);
    let dw = 0.5*G*(b + a*(1.0-b*b))/max(w, 1e-7);
    return vec2<f32>(w, dw);
}
fn lh_norm(s: f32) -> f32 {
    let a = sqrt(max(s, 1e-7));
    if (s < 0.4) {
        return (0.5/PI) + s*(0.220636+s*(-0.109+s*0.090));
    }
    return inverseSqrt(PI)*(a*0.5 + (1.0/a)*0.0625);
}
fn lh_function(s: f32, theta: f32) -> f32 {
    return lh_norm(s) * pow(abs(cos(theta*0.5)), 2.0*s);
}
fn directional_spread(w: f32, wp: f32, wind_speed: f32, theta: f32, angle: f32, swell: f32) -> f32 {
    let p = w/wp;
    var s: f32;
    if (w <= wp) {
        s = 6.97*pow(abs(p), 4.06);
    } else {
        s = 9.77*pow(abs(p), -2.33 - 1.45*(wind_speed*wp/G - 1.17));
    }
    let s_xi = 16.0*tanh(wp/w)*swell*swell;
    return lh_function(s+s_xi, theta-angle);
}
fn tma_spectrum(w: f32, wp: f32, alpha: f32, depth: f32) -> f32 {
    let beta = 1.25;
    let gamma = 3.3;
    var sigma = 0.09;
    if (w <= wp) { sigma = 0.07; }
    let r = exp(-(w-wp)*(w-wp)/(2.0*sigma*sigma*wp*wp));
    let jonswap = (alpha*G*G)/pow(w,5.0) * exp(-beta*pow(wp/w,4.0)) * pow(gamma,r);
    let wh = min(w*sqrt(depth/G),2.0);
    var attenuation = 1.0 - 0.5*(2.0-wh)*(2.0-wh);
    if (wh <= 1.0) { attenuation = 0.5*wh*wh; }
    return jonswap*attenuation;
}
fn spectrum_index(cascade: u32, x: u32, y: u32) -> u32 {
    let n = map_size();
    return cascade*n*n + y*n + x;
}
fn get_spectrum_amplitude(cascade: u32, id: vec2<u32>) -> vec2<f32> {
    let p = cascade_params[cascade];
    let n = map_size();
    let tile = p.tile.xy;
    let dk = vec2<f32>(2.0*PI)/tile;
    let centered = vec2<f32>(id) - vec2<f32>(f32(n)*0.5);
    if (all(centered == vec2<f32>(0.0)) || any(abs(centered) >= vec2<f32>(256.0))) { return vec2<f32>(0.0); }
    let k_vec = centered*dk;
    let k = length(k_vec);
    let theta = atan2(k_vec.x, k_vec.y);
    let disp = dispersion(k,p.shape.x);
    let w = disp.x;
    let w_norm = disp.y/k*dk.x*dk.y;
    let s = tma_spectrum(w,p.spectrum.y,p.spectrum.x,p.shape.x);
    let directed = directional_spread(w,p.spectrum.y,p.spectrum.z,theta,p.spectrum.w,p.shape.y);
    let d = mix(0.5/PI,directed,1.0-p.shape.w) * exp(-(1.0-p.shape.z)*(1.0-p.shape.z)*k*k);
    let parent_id = vec2<u32>(vec2<i32>(centered) + vec2<i32>(256));
    return gaussian(hash2(parent_id+p.seed.xy))*sqrt(max(0.0,2.0*s*d*w_norm));
}

@compute @workgroup_size(16,16,1)
fn spectrum_compute(@builtin(global_invocation_id) gid: vec3<u32>) {
    let n = map_size();
    if (gid.x >= n || gid.y >= n || gid.z >= NUM_CASCADES) { return; }
    let id0 = gid.xy;
    let id1 = vec2<u32>((n-id0.x)%n,(n-id0.y)%n);
    let a = get_spectrum_amplitude(gid.z,id0);
    let b = complex_conj(get_spectrum_amplitude(gid.z,id1));
    spectrum_data[spectrum_index(gid.z,gid.x,gid.y)] = vec4<f32>(a,b);
}

@compute @workgroup_size(64,1,1)
fn fft_butterfly(@builtin(global_invocation_id) gid: vec3<u32>) {
    let n = map_size();
    let col = gid.x;
    let stage = gid.y;
    if (col >= n/2u || stage >= u32(firstLeadingBit(n))) { return; }
    let stride = 1u << stage;
    let mid = n >> (stage+1u);
    let i = col >> stage;
    let j = col & (stride-1u);
    let twiddle = complex_exp(PI/f32(stride)*f32(j));
    let r0 = stride*(i+0u)+j;
    let r1 = stride*(i+mid)+j;
    let w0 = stride*(2u*i+0u)+j;
    let w1 = stride*(2u*i+1u)+j;
    let read_indices = vec2<f32>(bitcast<f32>(r0),bitcast<f32>(r1));
    butterfly_data[stage*n+w0] = vec4<f32>(read_indices,twiddle);
    butterfly_data[stage*n+w1] = vec4<f32>(read_indices,-twiddle);
}

fn fft_index(_cascade: u32, ping: u32, spectrum: u32, x: u32, y: u32) -> u32 {
    // One scratch FFT is reused cascade-by-cascade, matching the upstream
    // staggered update schedule while keeping the storage binding below common
    // WebGPU max-storage-buffer limits.
    let n = map_size();
    return ping*NUM_SPECTRA*n*n + spectrum*n*n + y*n+x;
}

@compute @workgroup_size(16,16,1)
fn spectrum_modulate(@builtin(global_invocation_id) gid: vec3<u32>) {
    let n = map_size();
    if (gid.x >= n || gid.y >= n) { return; }
    let c = active_cascade();
    let p = cascade_params[c];
    let centered = vec2<f32>(gid.xy)-vec2<f32>(f32(n)*0.5);
    let k_vec = centered*vec2<f32>(2.0*PI)/p.tile.xy;
    let k = length(k_vec)+1e-6;
    let k_unit = k_vec/k;
    let h0 = spectrum_data[spectrum_index(c,gid.x,gid.y)];
    let phase = sqrt(G*k*tanh(k*p.shape.x))*p.time.x;
    let modulation = complex_exp(phase);
    let h = complex_mul(h0.xy,modulation)+complex_mul(h0.zw,complex_conj(modulation));
    let h_inv = vec2<f32>(-h.y,h.x);
    let hx = h_inv*k_unit.y;
    let hy = h;
    let hz = h_inv*k_unit.x;
    let dhy_dx = h_inv*k_vec.y;
    let dhy_dz = h_inv*k_vec.x;
    let dhx_dx = -h*k_vec.y*k_unit.y;
    let dhz_dz = -h*k_vec.x*k_unit.x;
    let dhz_dx = -h*k_vec.y*k_unit.x;
    fft_data[fft_index(c,0u,0u,gid.x,gid.y)] = vec2<f32>(hx.x-hy.y,hx.y+hy.x);
    fft_data[fft_index(c,0u,1u,gid.x,gid.y)] = vec2<f32>(hz.x-dhy_dx.y,hz.y+dhy_dx.x);
    fft_data[fft_index(c,0u,2u,gid.x,gid.y)] = vec2<f32>(dhy_dz.x-dhx_dx.y,dhy_dz.y+dhx_dx.x);
    fft_data[fft_index(c,0u,3u,gid.x,gid.y)] = vec2<f32>(dhz_dz.x-dhz_dx.y,dhz_dz.y+dhz_dx.x);
}

var<workgroup> row_shared: array<vec2<f32>, 2048>;
@compute @workgroup_size(1024,1,1)
fn fft_compute(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>
) {
    let n = map_size();
    let col = lid.x;
    let row = gid.y;
    let spec = gid.z;
    let lane_valid = col < n && row < n && spec < NUM_SPECTRA;
    let c = active_cascade();
    if (lane_valid) {
        row_shared[col] = fft_data[fft_index(c,0u,spec,col,row)];
    } else {
        row_shared[col] = vec2<f32>(0.0);
    }
    let stages = u32(firstLeadingBit(n));
    for (var stage=0u; stage<stages; stage=stage+1u) {
        workgroupBarrier();
        if (lane_valid) {
            let read_ping = stage & 1u;
            let write_ping = (stage+1u)&1u;
            let b = butterfly_data[stage*n+col];
            let reads = vec2<u32>(bitcast<u32>(b.x),bitcast<u32>(b.y));
            let upper = row_shared[read_ping*MAX_MAP_SIZE+reads.x];
            let lower = row_shared[read_ping*MAX_MAP_SIZE+reads.y];
            row_shared[write_ping*MAX_MAP_SIZE+col] = upper+complex_mul(lower,b.zw);
        }
    }
    workgroupBarrier();
    if (lane_valid) {
        fft_data[fft_index(c,1u,spec,col,row)] = row_shared[(stages&1u)*MAX_MAP_SIZE+col];
    }
}

var<workgroup> transpose_tile: array<vec2<f32>,1056>;
@compute @workgroup_size(32,32,1)
fn transpose(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(global_invocation_id) gid: vec3<u32>
) {
    let n = map_size();
    let spec = gid.z;
    let input_valid = gid.x < n && gid.y < n && spec < NUM_SPECTRA;
    let c = active_cascade();
    let local_index = lid.y*33u+lid.x;

    // Every invocation in the workgroup must reach the barrier.  Returning
    // early for edge lanes is rejected by DX12/FXC (X4026: sync in varying
    // control flow) and is undefined for a workgroup-wide synchronization.
    // Invalid lanes contribute zero to shared memory instead.
    if (input_valid) {
        transpose_tile[local_index] = fft_data[fft_index(c,1u,spec,gid.x,gid.y)];
    } else {
        transpose_tile[local_index] = vec2<f32>(0.0);
    }
    workgroupBarrier();

    let out_x = wid.y*32u+lid.x;
    let out_y = wid.x*32u+lid.y;
    if (out_x < n && out_y < n && spec < NUM_SPECTRA) {
        fft_data[fft_index(c,0u,spec,out_x,out_y)] = transpose_tile[lid.x*33u+lid.y];
    }
}

@compute @workgroup_size(16,16,2)
fn fft_unpack(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>
) {
    let n = map_size();
    if (gid.x>=n || gid.y>=n) { return; }
    let c = active_cascade();
    let sign_shift = select(1.0,-1.0,((gid.x&1u)^(gid.y&1u)) != 0u);
    let a = fft_data[fft_index(c,1u,lid.z*2u,gid.x,gid.y)];
    let b = fft_data[fft_index(c,1u,lid.z*2u+1u,gid.x,gid.y)];
    if (lid.z==0u) {
        textureStore(displacement_map,vec2<i32>(gid.xy),i32(c),vec4<f32>(a.x,a.y,b.x,0.0)*sign_shift);
    } else {
        let p = cascade_params[c];
        let s1 = fft_data[fft_index(c,1u,1u,gid.x,gid.y)];
        let s2 = fft_data[fft_index(c,1u,2u,gid.x,gid.y)];
        let s3 = fft_data[fft_index(c,1u,3u,gid.x,gid.y)];
        let dhy_dx = s1.y*sign_shift;
        let dhy_dz = s2.x*sign_shift;
        let dhx_dx = s2.y*sign_shift*p.tile.z;
        let dhz_dz = s3.x*sign_shift*p.tile.z;
        let dhz_dx = s3.y*sign_shift*p.tile.z;
        let jacobian = (1.0+dhx_dx)*(1.0+dhz_dz)-dhz_dx*dhz_dx;
        let foam_factor = -min(0.0,jacobian-p.foam.x);
        let fi = c*n*n+gid.y*n+gid.x;
        var foam = foam_data[fi];
        foam = foam*exp(-p.foam.z)+foam_factor*p.foam.y;
        foam = clamp(foam,0.0,1.0);
        foam_data[fi]=foam;
        let gradient = vec2<f32>(dhy_dx,dhy_dz)/(vec2<f32>(1.0)+abs(vec2<f32>(dhx_dx,dhz_dz)));
        textureStore(normal_map,vec2<i32>(gid.xy),i32(c),vec4<f32>(gradient,dhx_dx,foam));
    }
}
