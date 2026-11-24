//! Direct ports of conflict-free EchoField files RT60Estimator.java,
//! RoomGeometry.java, AirAbsorptionProcessor.java (math), ReflectionTap.java.
//! Distances are upstream metres/blocks, NOT JKA units. No unit conversion here.

pub fn estimate_rt60(times: &[f64], energies: &[f64]) -> f64 {
    assert_eq!(times.len(), energies.len());
    const MAX_BINS: usize = 500;
    const BIN_WIDTH: f64 = 0.01;
    let mut sums = [0.0; MAX_BINS];
    let mut counts = [0u32; MAX_BINS];
    for (&time, &energy) in times.iter().zip(energies) {
        if energy <= 0.0 {
            continue;
        }
        let bin = (time / BIN_WIDTH) as i32;
        if bin < 0 || bin >= MAX_BINS as i32 {
            continue;
        }
        sums[bin as usize] += energy;
        counts[bin as usize] += 1;
    }
    let (mut sum_t, mut sum_ln_e, mut sum_t_ln_e, mut sum_t2) = (0.0, 0.0, 0.0, 0.0);
    let mut n = 0;
    for bin in 0..MAX_BINS {
        if counts[bin] == 0 {
            continue;
        }
        let mean = sums[bin] / f64::from(counts[bin]);
        if mean > 0.0 {
            let t = (bin as f64 + 0.5) * BIN_WIDTH;
            let ln_e = mean.ln();
            n += 1;
            sum_t += t;
            sum_ln_e += ln_e;
            sum_t_ln_e += t * ln_e;
            sum_t2 += t * t;
        }
    }
    if n < 3 {
        return 0.0;
    }
    let n = f64::from(n);
    let denom = n * sum_t2 - sum_t * sum_t;
    if denom.abs() < 1e-15 {
        return 0.0;
    }
    let alpha = -(n * sum_t_ln_e - sum_t * sum_ln_e) / denom;
    if alpha <= 0.0 {
        return 0.0;
    }
    1000_f64.ln() / alpha
}

pub fn estimate_volume(enclosure: f32, avg_return_dist: f32) -> f32 {
    if enclosure < 0.01 {
        return 100_000.0;
    }
    let side = 2.0 * avg_return_dist;
    side * side * side / enclosure
}
pub fn critical_distance(volume_m3: f32, rt60: f32) -> f32 {
    0.057 * (f64::from(volume_m3 / rt60.max(0.05)).sqrt() as f32)
}
pub fn reverb_send_multiplier(source_distance: f32, critical_dist: f32) -> f32 {
    if critical_dist <= 0.01 {
        return 1.0;
    }
    f64::from((source_distance / critical_dist).min(1.0)).sqrt() as f32
}

pub fn compute_band_gains(distance_m: f32, humidity_rh: f32) -> [f32; 3] {
    const ALPHA_30RH: [f32; 3] = [0.1, 2.0, 15.0];
    const ALPHA_70RH: [f32; 3] = [0.05, 1.0, 8.0];
    let t = ((humidity_rh - 30.0) / 40.0).clamp(0.0, 1.0);
    std::array::from_fn(|i| {
        let alpha = ALPHA_30RH[i] + (ALPHA_70RH[i] - ALPHA_30RH[i]) * t;
        let attenuation_db = alpha * distance_m / 100.0;
        (10_f64.powf(-f64::from(attenuation_db) / 20.0) as f32).clamp(0.0, 1.0)
    })
}
/// OpenAL AL_LOWPASS_GAIN and AL_LOWPASS_GAINHF. The owner of the native
/// context must apply these; this function never writes to OpenAL itself.
pub fn lowpass_parameters(distance_m: f32, humidity_rh: f32) -> [f32; 2] {
    let g = compute_band_gains(distance_m, humidity_rh);
    [
        ((g[0] + g[1] + g[2]) / 3.0).clamp(0.0, 1.0),
        if g[0] < 0.001 {
            0.0
        } else {
            (g[2] / g[0]).clamp(0.0, 1.0)
        },
    ]
}
/// Upstream biome-key mapping, retained as a reference. JKA has no biome registry.
pub fn biome_humidity(biome_key: Option<&str>) -> f32 {
    let Some(key) = biome_key else {
        return 50.0;
    };
    let lower = key.to_lowercase();
    let has = |keys: &[&str]| keys.iter().any(|k| lower.contains(*k));
    if has(&["desert", "badlands", "mesa"]) {
        15.0
    } else if has(&["savanna"]) {
        25.0
    } else if has(&["taiga", "snowy", "frozen", "ice"]) {
        35.0
    } else if has(&["plains", "forest", "birch", "mountain", "meadow"]) {
        50.0
    } else if has(&["jungle", "mangrove"]) {
        85.0
    } else if has(&["swamp", "mushroom"]) {
        90.0
    } else if has(&["ocean", "river", "beach"]) {
        75.0
    } else {
        50.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ReflectionTap<M> {
    pub position: [f64; 3],
    pub distance: f64,
    pub delay: f64,
    pub energy: f64,
    pub material: M,
    pub direction: [f64; 3],
    pub order: i32,
}
impl<M> ReflectionTap<M> {
    pub fn new(
        position: [f64; 3],
        distance: f64,
        energy: f64,
        material: M,
        direction: [f64; 3],
        order: i32,
    ) -> Self {
        Self {
            position,
            distance,
            delay: distance / 343.0,
            energy,
            material,
            direction,
            order,
        }
    }
}
