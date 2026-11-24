//! EchoField 4a595e7: SpectralCategory, DirectionalAnalysis, EnvironmentProfile,
//! EnvironmentProfiler. World access is the only platform abstraction here.
use crate::acoustic::{critical_distance, estimate_rt60, estimate_volume, ReflectionTap};
use crate::geometry::Vec3;
use std::sync::Arc;

pub(crate) fn add(a: Vec3, b: Vec3) -> Vec3 {
    std::array::from_fn(|i| a[i] + b[i])
}
pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    std::array::from_fn(|i| a[i] - b[i])
}
pub(crate) fn scale(a: Vec3, s: f64) -> Vec3 {
    a.map(|v| v * s)
}
pub(crate) fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn length(a: Vec3) -> f64 {
    dot(a, a).sqrt()
}
pub fn normalize(a: Vec3) -> Vec3 {
    let l = length(a);
    if l < 1e-5 {
        [0.; 3]
    } else {
        scale(a, 1. / l)
    }
}
pub(crate) fn reflect(d: Vec3, n: Vec3) -> Vec3 {
    sub(d, scale(n, 2. * dot(d, n)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpectralCategory {
    Hard,
    Soft,
    Foliage,
    Wood,
    Default,
}
impl SpectralCategory {
    /// low/mid/high multipliers; LF decay ratio, LF gain; physical absorption.
    pub fn coefficients(self) -> ([f32; 3], f32, f32, [f32; 3]) {
        match self {
            Self::Hard => ([1., 1., 1.], 1., 1., [0.02, 0.03, 0.05]),
            Self::Soft => ([0.5, 1., 2.], 0.7, 0.7, [0.25, 0.40, 0.55]),
            Self::Foliage => ([0.3, 1.2, 1.8], 1.1, 0.9, [0.10, 0.20, 0.35]),
            Self::Wood => ([0.7, 1., 1.3], 0.9, 0.85, [0.10, 0.08, 0.12]),
            Self::Default => ([1., 1., 1.], 1., 1., [0.05; 3]),
        }
    }
    pub fn absorption(self) -> [f32; 3] {
        self.coefficients().3
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Material {
    pub spectral: SpectralCategory,
    pub reflectivity: f32,
    pub occlusion: f32,
}
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub position: Vec3,
    pub normal: Vec3,
    pub material: Material,
}
/// All world queries are in upstream metres and Y-up. Implementations must
/// exclude self-hits and return a normal oriented into the incident airspace.
pub trait AcousticWorld {
    fn cast(&mut self, from: Vec3, to: Vec3) -> Option<Hit>;
    fn occlusion(&mut self, from: Vec3, to: Vec3, max: f64, max_steps: usize) -> f64;
}
pub fn symmetric_eigenvalues(
    a00: f64,
    a01: f64,
    a02: f64,
    a11: f64,
    a12: f64,
    a22: f64,
) -> [f64; 3] {
    let p = a00 + a11 + a22;
    let q = a00 * a11 - a01 * a01 + a00 * a22 - a02 * a02 + a11 * a22 - a12 * a12;
    let r = a00 * (a11 * a22 - a12 * a12) - a01 * (a01 * a22 - a12 * a02)
        + a02 * (a01 * a12 - a11 * a02);
    let p3 = p / 3.;
    let pp = (p * p - 3. * q) / 9.;
    let qq = (2. * p * p * p - 9. * p * q + 27. * r) / 54.;
    let result = if pp < 1e-30 {
        [p3; 3]
    } else {
        let disc = qq * qq - pp * pp * pp;
        let eps = pp * pp * pp * 1e-10;
        if disc <= eps {
            let s = pp.sqrt();
            let th = (-qq / (pp * s)).clamp(-1., 1.).acos();
            [
                th,
                th + 2. * std::f64::consts::PI,
                th - 2. * std::f64::consts::PI,
            ]
            .map(|v| -2. * s * (v / 3.).cos() + p3)
        } else {
            let a = -qq.signum() * (qq.abs() + disc.sqrt()).cbrt();
            let b = if a.abs() > 1e-30 { pp / a } else { 0. };
            [a + b + p3, -(a + b) / 2. + p3, -(a + b) / 2. + p3]
        }
    };
    result.map(|x| x.max(0.))
}
pub fn directional_balance(dirs: &[Vec3]) -> f32 {
    if dirs.len() < 3 {
        return 0.;
    }
    let n = dirs.len() as f64;
    let mean = scale(dirs.iter().copied().fold([0.; 3], add), 1. / n);
    let mut c = [0.; 6];
    for &d in dirs {
        let [x, y, z] = sub(d, mean);
        for (a, b) in c.iter_mut().zip([x * x, x * y, x * z, y * y, y * z, z * z]) {
            *a += b;
        }
    }
    let [a, b, c, d, e, f] = c.map(|v| v / n);
    let eig = symmetric_eigenvalues(a, b, c, d, e, f);
    let max = eig.into_iter().fold(f64::NEG_INFINITY, f64::max);
    let min = eig.into_iter().fold(f64::INFINITY, f64::min);
    if max < 1e-15 {
        0.
    } else {
        (min / max).clamp(0., 1.) as f32
    }
}
pub type Tap = ReflectionTap<Material>;
#[derive(Clone, Debug)]
pub struct EnvironmentProfile {
    pub taps: Arc<[Tap]>,
    pub enclosure: f32,
    pub average_return_distance: f32,
    pub scatter: f32,
    pub rt60: f32,
    pub absorption: f32,
    pub spectral: [f32; 3],
    pub balance: f32,
    pub wind_exposure: f32,
    pub canopy: f32,
    pub most_open_sky: Vec3,
}
impl Default for EnvironmentProfile {
    fn default() -> Self {
        Self {
            taps: Arc::from([]),
            enclosure: 0.,
            average_return_distance: 0.,
            scatter: 0.,
            rt60: 0.,
            absorption: 0.,
            spectral: [0.; 3],
            balance: 0.,
            wind_exposure: 1.,
            canopy: 0.,
            most_open_sky: [0., 1., 0.],
        }
    }
}
impl EnvironmentProfile {
    pub fn from_taps(taps: Vec<Tap>, dirs: &[Vec3], returned: &[bool]) -> Self {
        assert_eq!(dirs.len(), returned.len());
        let mut upward = 0;
        let mut escaped = 0;
        let mut sky = [0.; 3];
        let mut ret = Vec::new();
        for (&d, &r) in dirs.iter().zip(returned) {
            if r {
                ret.push(d);
            }
            if d[1] > 0. {
                upward += 1;
                if !r {
                    escaped += 1;
                    sky = add(sky, d);
                }
            }
        }
        let exposure = if upward == 0 {
            0.
        } else {
            escaped as f32 / upward as f32
        };
        let canopy = if upward == 0 {
            0.
        } else {
            (upward - escaped) as f32 / upward as f32
        };
        let most_open_sky = if escaped > 0 {
            let a = scale(sky, 1. / escaped as f64);
            if length(a) > 1e-6 {
                normalize(a)
            } else {
                [0., 1., 0.]
            }
        } else {
            let mut longest = -1.;
            let mut best = [0., 1., 0.];
            for tap in &taps {
                if tap.direction[1] > 0. && tap.distance > longest {
                    longest = tap.distance;
                    best = normalize(tap.direction);
                }
            }
            best
        };
        if taps.is_empty() {
            return Self {
                wind_exposure: exposure,
                canopy,
                most_open_sky,
                ..Self::default()
            };
        }
        let n = taps.len() as f64;
        let avg = (taps.iter().map(|t| t.distance).sum::<f64>() / n) as f32;
        let scatter = if ret.len() < 3 || avg <= 1e-6 {
            0.
        } else {
            ((taps
                .iter()
                .map(|t| (t.distance - f64::from(avg)).powi(2))
                .sum::<f64>()
                / n)
                .sqrt()
                / f64::from(avg)) as f32
        };
        let times: Vec<_> = taps.iter().map(|t| t.delay).collect();
        let energies: Vec<_> = taps.iter().map(|t| t.energy).collect();
        let mut spectral = [0.; 3];
        let mut absorption = 0.;
        for tap in &taps {
            let a = tap.material.spectral.absorption();
            absorption += (a[0] + a[1] + a[2]) / 3.;
            for (s, x) in spectral.iter_mut().zip(a) {
                *s += x;
            }
        }
        let balance = directional_balance(&ret);
        Self {
            taps: Arc::from(taps),
            enclosure: ret.len() as f32 / dirs.len() as f32,
            average_return_distance: avg,
            scatter,
            rt60: estimate_rt60(&times, &energies) as f32,
            absorption: absorption / n as f32,
            spectral: spectral.map(|x| x / n as f32),
            balance,
            wind_exposure: exposure * (0.5 + 0.5 * (1. - balance)),
            canopy,
            most_open_sky,
        }
    }
    pub fn volume(&self) -> f32 {
        estimate_volume(self.enclosure, self.average_return_distance)
    }
    pub fn critical_distance(&self) -> f32 {
        critical_distance(self.volume(), self.rt60)
    }
    pub fn lerp(&self, other: &Self, t: f32) -> Self {
        let c = t.clamp(0., 1.);
        let f = |a, b| (1. - c) * a + c * b;
        Self {
            taps: if c >= 0.5 {
                other.taps.clone()
            } else {
                self.taps.clone()
            },
            enclosure: f(self.enclosure, other.enclosure),
            average_return_distance: f(self.average_return_distance, other.average_return_distance),
            scatter: f(self.scatter, other.scatter),
            rt60: f(self.rt60, other.rt60),
            absorption: f(self.absorption, other.absorption),
            spectral: std::array::from_fn(|i| f(self.spectral[i], other.spectral[i])),
            balance: f(self.balance, other.balance),
            wind_exposure: f(self.wind_exposure, other.wind_exposure),
            canopy: f(self.canopy, other.canopy),
            most_open_sky: if c >= 0.5 {
                other.most_open_sky
            } else {
                self.most_open_sky
            },
        }
    }
}
pub const MAX_DISTANCE: f64 = 256.;
pub const GOLDEN_ANGLE: f32 = 1.618033988_f32 * std::f32::consts::PI * 2.;
pub fn golden_direction(i: usize, n: usize, centered: bool) -> Vec3 {
    let fi = if centered {
        (2. * i as f32 + 1.) / (2. * n as f32)
    } else {
        i as f32 / n as f32
    };
    let longitude = GOLDEN_ANGLE * i as f32;
    let latitude = (f64::from(fi * 2. - 1.).asin()) as f32;
    if centered {
        let c = latitude.cos();
        [
            f64::from(c * longitude.cos()),
            f64::from(c * longitude.sin()),
            f64::from(latitude.sin()),
        ]
    } else {
        let lat = f64::from(latitude);
        let lon = f64::from(longitude);
        [lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin()]
    }
}
/// Geometry snapshot lifetime belongs to the worker, not the audio callback.
pub struct Profiler {
    total: usize,
    per_tick: usize,
    pending: Option<(usize, usize)>,
    index: usize,
    taps: Vec<Tap>,
    directions: Vec<Vec3>,
    returned: Vec<bool>,
    latest: EnvironmentProfile,
    previous: Option<EnvironmentProfile>,
    crossfade_tick: u64,
}
impl Default for Profiler {
    fn default() -> Self {
        Self::new(128, 8)
    }
}
impl Profiler {
    pub fn new(total: usize, per_tick: usize) -> Self {
        assert!(total > 0 && per_tick > 0);
        Self {
            total,
            per_tick,
            pending: None,
            index: 0,
            taps: Vec::with_capacity(total * 3),
            directions: Vec::with_capacity(total),
            returned: Vec::with_capacity(total),
            latest: EnvironmentProfile::default(),
            previous: None,
            crossfade_tick: 0,
        }
    }
    pub fn configure(&mut self, total: usize, per_tick: usize) {
        assert!(total > 0 && per_tick > 0);
        self.pending = Some((total, per_tick));
    }
    pub fn invalidate(&mut self) {
        self.index = 0;
        self.taps.clear();
        self.directions.clear();
        self.returned.clear();
        self.latest = EnvironmentProfile::default();
        self.previous = None;
    }
    pub fn current(&self, tick: u64) -> EnvironmentProfile {
        match &self.previous {
            Some(p) if tick.saturating_sub(self.crossfade_tick) < 25 => p.lerp(
                &self.latest,
                tick.saturating_sub(self.crossfade_tick) as f32 / 25.,
            ),
            _ => self.latest.clone(),
        }
    }
    pub fn latest(&self) -> &EnvironmentProfile {
        &self.latest
    }
    pub fn tick(&mut self, world: &mut impl AcousticWorld, camera: Vec3, tick: u64) -> bool {
        let end = (self.index + self.per_tick).min(self.total);
        for i in self.index..end {
            let ray = golden_direction(i, self.total, true);
            self.directions.push(ray);
            let hit = world.cast(camera, add(camera, scale(ray, MAX_DISTANCE)));
            self.returned.push(hit.is_some());
            let Some(mut hit) = hit else { continue };
            let mut direction = ray;
            let mut distance = length(sub(hit.position, camera));
            let mut energy = 1_f32;
            for bounce in 0..3 {
                energy *= hit.material.reflectivity.clamp(0., 1.);
                self.taps.push(Tap::new(
                    hit.position,
                    distance,
                    f64::from(energy),
                    hit.material,
                    ray,
                    bounce + 1,
                ));
                direction = reflect(direction, hit.normal);
                let start = add(hit.position, scale(hit.normal, 0.001));
                let Some(next) =
                    world.cast(start, add(hit.position, scale(direction, MAX_DISTANCE)))
                else {
                    break;
                };
                distance += length(sub(next.position, hit.position));
                hit = next;
            }
        }
        self.index = end;
        if end == self.total {
            let new = EnvironmentProfile::from_taps(
                std::mem::take(&mut self.taps),
                &self.directions,
                &self.returned,
            );
            self.previous = Some(std::mem::replace(&mut self.latest, new));
            self.crossfade_tick = tick;
            if let Some((n, p)) = self.pending.take() {
                self.total = n;
                self.per_tick = p;
            }
            self.index = 0;
            self.directions.clear();
            self.returned.clear();
            true
        } else {
            false
        }
    }
}
