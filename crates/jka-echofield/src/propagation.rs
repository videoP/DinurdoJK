//! SPR SoundPhysics.java / ReflectedAudio.java, clean 4a595e7. All distances
//! in upstream metres; source category/block variation supplied by host.
use crate::{geometry::Vec3, profile::*};
#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub rays: usize,
    pub bounces: usize,
    pub block_absorption: f32,
    pub variation: f64,
    pub max_occlusion: f64,
    pub max_occlusion_steps: usize,
    pub strict: bool,
    pub underwater_filter: f32,
    pub reverb_attenuation_distance: f32,
    pub reverb_distance: f32,
    pub max_processing_distance: f64,
    pub attenuation_factor: f64,
    pub direction_evaluation: bool,
    pub redirect_non_occluded_sounds: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            rays: 32,
            bounces: 4,
            block_absorption: 1.,
            variation: 0.35,
            max_occlusion: 64.,
            max_occlusion_steps: 16,
            strict: false,
            underwater_filter: 0.9,
            reverb_attenuation_distance: 0.,
            reverb_distance: 1.5,
            max_processing_distance: 512.,
            attenuation_factor: 1.,
            direction_evaluation: true,
            redirect_non_occluded_sounds: true,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Source {
    pub position: Vec3,
    pub max_distance: f32,
    pub underwater: bool,
    pub block_sound: bool,
    pub voicechat: bool,
    pub aux_only: bool,
}
#[derive(Clone, Copy, Debug)]
pub struct Environment {
    pub direct_gain: f32,
    pub direct_cutoff: f32,
    pub send_gain: [f32; 4],
    pub send_cutoff: [f32; 4],
    pub redirected_position: Option<Vec3>,
    pub occlusion: f64,
}
impl Default for Environment {
    fn default() -> Self {
        Self {
            direct_gain: 1.,
            direct_cutoff: 1.,
            send_gain: [0.; 4],
            send_cutoff: [1.; 4],
            redirected_position: None,
            occlusion: 0.,
        }
    }
}
fn shared(world: &mut impl AcousticWorld, pos: Vec3, listener: Vec3) -> Option<Vec3> {
    if world.cast(pos, listener).is_none() {
        Some(sub(pos, listener))
    } else {
        None
    }
}
struct Reflected {
    sum: Vec3,
    count: usize,
    directions: usize,
    evaluate: bool,
    max_length: f64,
}
impl Reflected {
    fn add(&mut self, vector: Vec3, distance: f64) {
        let len = distance + length(vector);
        if len <= 0. || len > self.max_length {
            return;
        }
        self.count += 1;
        if self.evaluate {
            self.sum = add(self.sum, scale(normalize(vector), 1. / (len * len)));
            self.directions += 1;
        }
    }
}
pub fn evaluate(
    world: &mut impl AcousticWorld,
    s: Source,
    listener: Vec3,
    listener_underwater: bool,
    c: Config,
) -> Environment {
    let mut result = Environment::default();
    if c.rays == 0
        || c.bounces < 2
        || s.max_distance <= 0.
        || length(sub(listener, s.position)) > c.max_processing_distance
    {
        if s.aux_only {
            result.direct_gain = 0.;
        }
        return result;
    }
    let mut occ = world.occlusion(s.position, listener, c.max_occlusion, c.max_occlusion_steps);
    let variation = if s.block_sound {
        c.variation.max(0.49)
    } else {
        c.variation
    };
    if !c.strict && variation > 0. {
        for x in [-1., 1.] {
            for y in [-1., 1.] {
                for z in [-1., 1.] {
                    let offset = scale([x, y, z], variation);
                    occ = occ.min(world.occlusion(
                        add(s.position, offset),
                        add(listener, offset),
                        c.max_occlusion,
                        c.max_occlusion_steps,
                    ));
                }
            }
        }
    }
    occ = occ.min(c.max_occlusion);
    result.occlusion = occ;
    let absorption = c.block_absorption * 3.;
    let base = (-occ * f64::from(absorption)).exp() as f32;
    let underwater = listener_underwater || s.underwater;
    let mut direct = base;
    if underwater {
        direct *= 1. - c.underwater_filter;
    }
    let evaluate =
        c.direction_evaluation && (occ > 0. || !c.redirect_non_occluded_sounds) && !s.voicechat;
    let initial = shared(world, s.position, listener)
        .map(normalize)
        .unwrap_or([0.; 3]);
    let mut reflected = Reflected {
        sum: initial,
        count: 0,
        directions: 0,
        evaluate,
        max_length: c.max_processing_distance / c.attenuation_factor,
    };
    let mut ratio = vec![0_f32; c.bounces];
    let mut gains = [0_f32; 4];
    let rcp = 1. / (c.rays * c.bounces) as f32;
    for i in 0..c.rays {
        let ray = golden_direction(i, c.rays, false);
        let Some(mut hit) = world.cast(s.position, add(s.position, scale(ray, MAX_DISTANCE)))
        else {
            continue;
        };
        let mut total = length(sub(hit.position, s.position)) as f32;
        let mut dir = ray;
        if let Some(v) = shared(world, add(hit.position, scale(hit.normal, 0.001)), listener) {
            reflected.add(v, f64::from(total));
        }
        for r in &mut ratio {
            let next_dir = reflect(dir, hit.normal);
            let next = world.cast(
                add(hit.position, scale(hit.normal, 0.001)),
                add(hit.position, scale(next_dir, MAX_DISTANCE)),
            );
            let reflectivity = hit.material.reflectivity;
            let energy = 0.25 * (reflectivity * 0.75 + 0.25);
            let miss = next.is_none();
            if let Some(new) = next {
                total = (f64::from(total) + length(sub(hit.position, new.position))) as f32;
                *r += reflectivity;
                hit = new;
                dir = next_dir;
                if let Some(v) =
                    shared(world, add(hit.position, scale(hit.normal, 0.001)), listener)
                {
                    reflected.add(v, f64::from(total));
                }
            } else {
                total = (f64::from(total) + length(sub(hit.position, listener))) as f32;
            }
            // Upstream checks attenuation before breaking a missed bounce.
            if total < c.reverb_attenuation_distance {
                continue;
            }
            let delay = total.max(0.) * 0.12 * reflectivity;
            for (j, g) in gains.iter_mut().enumerate() {
                let cross = if j < 3 {
                    1. - (delay - j as f32).abs().clamp(0., 1.)
                } else {
                    (delay - 2.).clamp(0., 1.)
                };
                *g += cross * energy * if j == 0 { 6.4 * rcp } else { 12.8 * rcp };
            }
            if miss {
                break;
            }
        }
    }
    for r in &mut ratio {
        *r /= c.rays as f32;
    }
    if reflected.evaluate && reflected.directions > 0 {
        let n = normalize(reflected.sum);
        if length(n) >= 0.5 {
            result.redirected_position =
                Some(add(listener, scale(n, length(sub(listener, s.position)))));
        }
    }
    let air = reflected.count as f32 * 64. * rcp;
    let weights = [20., 15., 10., 10.].map(|d| (air / d).clamp(0., 1.));
    let mut cutoff = weights.map(|w| base * (1. - w) + w);
    direct = direct.max((weights.iter().sum::<f32>() * 0.25).sqrt() * 0.2);
    result.direct_cutoff = direct;
    result.direct_gain = if s.aux_only { 0. } else { direct.powf(0.1) };
    gains[1] *= ratio[1];
    if ratio.len() > 2 {
        gains[2] *= ratio[2].powi(3);
    }
    if ratio.len() > 3 {
        gains[3] *= ratio[3].powi(4);
    }
    let distance = length(sub(
        listener,
        result.redirected_position.unwrap_or(s.position),
    )) as f32;
    let multiplier = 1. - (distance / (s.max_distance * c.reverb_distance)).min(1.);
    for j in 0..4 {
        gains[j] = if j >= 2 {
            (gains[j] * 1.05 - 0.05).clamp(0., 1.)
        } else {
            gains[j].clamp(0., 1.)
        };
        gains[j] *= cutoff[j].powf(0.1);
        gains[j] *= multiplier;
        if underwater {
            cutoff[j] *= 0.4;
        }
    }
    result.send_gain = gains;
    result.send_cutoff = cutoff;
    result
}

/// Later, conflict-free SoundPhysics.java change on main; applied before EFX writes.
pub fn blend_profile(e: &mut Environment, p: &EnvironmentProfile) {
    let late = (p.rt60 / 2.).min(1.) * 0.3 * 0.5;
    e.send_gain[2] += late;
    e.send_gain[3] += late;
    let hf = 0.7 + 0.3 * (1. - p.spectral[2]);
    let enclosure = p.enclosure * 0.3 * 0.2;
    e.send_gain[0] += enclosure;
    e.send_gain[1] += enclosure;
    let scatter = p.scatter * 0.3 * 0.15;
    e.send_gain[1] += scatter;
    e.send_gain[2] += scatter;
    e.send_gain = e.send_gain.map(|g| g.min(1.));
    e.send_cutoff = e.send_cutoff.map(|c| (c * hf).clamp(0., 1.));
}
