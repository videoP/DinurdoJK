//! Small retained ripple stamps from presentation actors. No wave solver.
use super::{
    interaction::ActorSample, sky_extend::SkyExtension, OceanMasks, OceanSurface,
    OCEAN_UNITS_PER_METER,
};
use blue_water_gpu::lake_ripples::{Stamp, MAX_STAMPS, SIZE, SPAN};
use std::collections::{HashMap, VecDeque};

const LIFETIME: f32 = 2.5;
#[derive(Clone, Copy)]
struct Event {
    slot: usize,
    position: [f32; 2],
    born: f32,
    strength: f32,
}
struct History {
    sample: ActorSample,
    emitted_ms: i32,
    seen: f32,
}
#[derive(Default)]
pub(super) struct LakeWakes {
    actors: HashMap<u16, History>,
    events: VecDeque<Event>,
    clock: f32,
}

fn inside(
    sample: ActorSample,
    slot: usize,
    body: OceanSurface,
    sky: Option<SkyExtension>,
    masks: &OceanMasks,
) -> bool {
    let p = sample.origin;
    if !p.iter().all(|v| v.is_finite()) {
        return false;
    }
    let xz = [p[0], p[2]];
    let finite = p[0] >= body.minimum[0]
        && p[0] <= body.maximum[0]
        && p[2] >= body.minimum[1]
        && p[2] <= body.maximum[1]
        && masks.contains(slot, xz);
    finite || sky.is_some_and(|s| s.contains(xz))
}
fn touches(
    sample: ActorSample,
    slot: usize,
    body: OceanSurface,
    sky: Option<SkyExtension>,
    masks: &OceanMasks,
) -> bool {
    sample.origin[1] - 24.0 <= body.plane_height + 1.0
        && sample.origin[1] + sample.head_height >= body.plane_height - 1.0
        && inside(sample, slot, body, sky, masks)
}

impl LakeWakes {
    pub fn clear(&mut self) {
        self.actors.clear();
        self.events.clear();
    }
    pub fn update(
        &mut self,
        samples: &[ActorSample],
        now: f32,
        camera: [f32; 3],
        bodies: &[(usize, OceanSurface, Option<SkyExtension>)],
        masks: &OceanMasks,
    ) {
        if now < self.clock {
            self.clear();
        }
        self.clock = now;
        self.events.retain(|e| now - e.born < LIFETIME);
        self.actors.retain(|_, a| now - a.seen < 0.5);
        for &sample in samples.iter().take(64) {
            if !sample.origin.iter().all(|v| v.is_finite()) {
                continue;
            }
            let distance = (sample.origin[0] - camera[0]).hypot(sample.origin[2] - camera[2]);
            if distance > (SPAN * 0.5 + 4.0) * OCEAN_UNITS_PER_METER {
                continue;
            }
            let Some(history) = self.actors.get_mut(&sample.id) else {
                self.actors.insert(
                    sample.id,
                    History {
                        sample,
                        emitted_ms: sample.time_ms - 100,
                        seen: now,
                    },
                );
                continue;
            };
            history.seen = now;
            let previous = history.sample;
            history.sample = sample;
            if sample.time_ms == previous.time_ms && sample.continuity == previous.continuity {
                continue;
            }
            let Some(velocity) = super::interaction::movement(previous, sample) else {
                history.emitted_ms = sample.time_ms;
                continue;
            };
            let horizontal = velocity.x.hypot(velocity.z);
            for &(slot, body, sky) in bodies {
                let was_touching = touches(previous, slot, body, sky, masks);
                let touching = touches(sample, slot, body, sky, masks);
                let crossed = !was_touching
                    && !touching
                    && previous.origin[1] - 24.0 > body.plane_height
                    && sample.origin[1] + sample.head_height < body.plane_height;
                let mut hit = sample;
                if crossed {
                    let t = ((previous.origin[1] - 24.0 - body.plane_height)
                        / (previous.origin[1] - sample.origin[1]))
                        .clamp(0.0, 1.0);
                    hit.origin = std::array::from_fn(|i| {
                        previous.origin[i] + (sample.origin[i] - previous.origin[i]) * t
                    });
                }
                if !(touching || was_touching || crossed && inside(hit, slot, body, sky, masks)) {
                    continue;
                }
                let entry = !was_touching && (touching || crossed);
                let moving = touching
                    && horizontal > 25.0
                    && sample.time_ms.saturating_sub(history.emitted_ms) >= 100;
                if !(entry || moving) {
                    continue;
                }
                if self.events.len() == MAX_STAMPS {
                    self.events.pop_front();
                }
                let impact_speed = sample
                    .velocity
                    .map_or(velocity.y.abs(), |v| v[1].abs().max(velocity.y.abs()));
                let strength = if entry {
                    0.8 + (impact_speed / 400.0).min(1.5)
                } else {
                    (horizontal / 240.0).clamp(0.15, 1.5)
                };
                self.events.push_back(Event {
                    slot,
                    position: [
                        hit.origin[0] / OCEAN_UNITS_PER_METER,
                        hit.origin[2] / OCEAN_UNITS_PER_METER,
                    ],
                    born: now,
                    strength,
                });
                history.emitted_ms = sample.time_ms;
                break;
            }
        }
    }
    pub fn origin(camera: [f32; 3]) -> [f32; 2] {
        let cell = SPAN / SIZE as f32;
        [camera[0], camera[2]]
            .map(|v| (v / OCEAN_UNITS_PER_METER / cell).round() * cell - SPAN * 0.5)
    }
    pub fn stamps(&self, slot: usize, now: f32, origin: [f32; 2], strength: f32) -> Vec<Stamp> {
        self.events
            .iter()
            .filter(|e| e.slot == slot)
            .filter_map(|e| {
                let age = (now - e.born).max(0.0);
                if age >= LIFETIME {
                    return None;
                }
                let radius = 0.25 + age * 0.85;
                let width = 0.22 + age * 0.04;
                let outer = radius + width;
                let uv = [
                    (e.position[0] - origin[0]) / SPAN,
                    (e.position[1] - origin[1]) / SPAN,
                ];
                if uv
                    .iter()
                    .any(|v| *v < -outer / SPAN || *v > 1.0 + outer / SPAN)
                {
                    return None;
                }
                Some(Stamp {
                    position: [uv[0], uv[1], outer / SPAN, radius],
                    shape: [
                        outer,
                        width,
                        0.015 * strength * e.strength * (1.0 - age / LIFETIME).powi(2),
                        0.0,
                    ],
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn actor(time_ms: i32, x: f32, y: f32) -> ActorSample {
        ActorSample {
            id: 1,
            origin: [x, y, 0.],
            head_height: 26.,
            time_ms,
            continuity: 0,
            velocity: None,
        }
    }
    fn body() -> [(usize, OceanSurface, Option<SkyExtension>); 1] {
        [(
            0,
            OceanSurface {
                plane_height: 0.,
                minimum: [-1000.; 2],
                maximum: [1000.; 2],
            },
            None,
        )]
    }
    #[test]
    fn lake_wakes_follow_motion_expire_and_ignore_idle_repeated_frames_and_teleports() {
        let mut wakes = LakeWakes::default();
        let masks = OceanMasks::default();
        let b = body();
        wakes.update(&[actor(0, 0., 0.)], 0., [0.; 3], &b, &masks);
        wakes.update(&[actor(100, 0., 0.)], 0.1, [0.; 3], &b, &masks);
        assert!(wakes.events.is_empty());
        let moving = actor(200, 30., 0.);
        wakes.update(&[moving], 0.2, [0.; 3], &b, &masks);
        assert_eq!(wakes.events.len(), 1);
        wakes.update(&[moving], 0.21, [0.; 3], &b, &masks);
        assert_eq!(wakes.events.len(), 1);
        wakes.update(&[actor(250, 500., 0.)], 0.25, [0.; 3], &b, &masks);
        assert_eq!(wakes.events.len(), 1);
        let origin = LakeWakes::origin([0.; 3]);
        let first = wakes.stamps(0, 0.2, origin, 1.);
        let later = wakes.stamps(0, 1.0, origin, 1.);
        assert!(
            later[0].position[3] > first[0].position[3] && later[0].shape[2] < first[0].shape[2]
        );
        assert!(
            wakes.stamps(1, 0.2, origin, 1.).is_empty(),
            "ripples must stay on their own pool"
        );
        wakes.update(&[], 3., [0.; 3], &b, &masks);
        assert!(wakes.events.is_empty());
    }
    #[test]
    fn lake_wakes_bound_crowded_player_stamps_and_reset_after_time_reversal() {
        let mut wakes = LakeWakes::default();
        let masks = OceanMasks::default();
        let b = body();
        for t in 0..5 {
            let actors: Vec<_> = (0..64)
                .map(|id| ActorSample {
                    id,
                    ..actor(t * 100, t as f32 * 20., 0.)
                })
                .collect();
            wakes.update(&actors, t as f32 * 0.1, [0.; 3], &b, &masks);
            assert!(wakes.events.len() <= MAX_STAMPS);
        }
        assert_eq!(wakes.events.len(), MAX_STAMPS);
        wakes.update(&[], 0., [0.; 3], &b, &masks);
        assert!(wakes.events.is_empty());
        assert!(wakes.actors.is_empty());
    }
    #[test]
    fn lake_wakes_detect_fast_surface_entries_and_reject_dry_or_remote_motion() {
        let mut wakes = LakeWakes::default();
        let masks = OceanMasks::default();
        let b = body();
        wakes.update(&[actor(0, 0., 80.)], 0., [0.; 3], &b, &masks);
        wakes.update(&[actor(100, 0., -70.)], 0.1, [0.; 3], &b, &masks);
        assert_eq!(
            wakes.events.len(),
            1,
            "swept entry must survive crossing the whole waterline in one sample"
        );
        wakes.clear();
        for t in 0..100 {
            wakes.update(
                &[actor(t * 20, t as f32 * 3., 100.)],
                t as f32 * 0.02,
                [0.; 3],
                &b,
                &masks,
            );
        }
        assert!(
            wakes.events.is_empty(),
            "airborne players must not stamp water"
        );
        for t in 0..100 {
            wakes.update(
                &[actor(t * 20, 2000. + t as f32 * 3., 0.)],
                3. + t as f32 * 0.02,
                [0.; 3],
                &b,
                &masks,
            );
        }
        assert!(
            wakes.events.is_empty(),
            "far-away actors must not consume the local wake budget"
        );
    }
}
