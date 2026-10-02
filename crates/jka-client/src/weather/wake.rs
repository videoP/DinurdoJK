//! Footsteps, drags and landings in standing water.
//!
//! The tracker watches moving people and emits short-lived disturbance events at
//! their feet, but only where the CPU copy of the puddle shape says there is
//! standing water. The events drive the ripples the surface shaders draw, and the
//! footsteps and landings among them also come back out as [`WaterSplash`]es for
//! the FX system to play the game's own splash effects.

use super::cpu_field::PuddleSample;
use glam::Vec3;
use std::collections::HashMap;

pub(crate) const WAKE_MAX_EVENTS: usize = 24;
/// Seconds a disturbance keeps rippling.
pub(crate) const WAKE_LIFETIME: f32 = 1.8;
/// Ripple front speed in world units per second.
pub(crate) const WAKE_RING_SPEED: f32 = 60.0;
/// A footstep only disturbs water at least this deep into a puddle.
const MIN_WATER: f32 = 0.15;
const MAX_PENDING_SPLASHES: usize = 16;

/// A person is a wake source only while moving at least this fast (units/s).
const MIN_SPEED: f32 = 40.0;
/// Full-strength wake at this speed; JKA running speed is about 250.
const FULL_SPEED: f32 = 260.0;
const STRIDE_DISTANCE: f32 = 48.0;
const DRAG_DISTANCE: f32 = 14.0;
const FOOT_OFFSET: f32 = 9.0;
/// Standing player origin to sole (the player hull's mins Z).
pub(crate) const FEET_BELOW_ORIGIN: f32 = 24.0;
const TELEPORT_DISTANCE: f32 = 192.0;
/// An event this far from the camera can never be seen.
const MAX_CAMERA_DISTANCE: f32 = 2600.0;
/// Downward speed (units/s) above which touching ground counts as a landing.
const LANDING_SPEED: f32 = 170.0;
const ACTOR_FORGET_SECONDS: f32 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum WakeKind {
    /// A foot planted in the water.
    Step = 0,
    /// A body pushing through it.
    Drag = 1,
    /// Touching down after a fall or jump.
    Landing = 2,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct WakeEvent {
    /// Render-space position at the sole.
    pub position: [f32; 3],
    pub birth: f32,
    /// 0..1.
    pub strength: f32,
    /// Render-space XZ unit direction of travel.
    pub direction: [f32; 2],
    pub kind: WakeKind,
}

impl WakeEvent {
    /// `(x, y, z, birth)` as the shaders read it.
    pub(crate) fn packed_position(&self) -> [f32; 4] {
        [self.position[0], self.position[1], self.position[2], self.birth]
    }

    /// `(strength, direction x, direction z, kind)` as the shaders read it.
    pub(crate) fn packed_shape(&self) -> [f32; 4] {
        [self.strength, self.direction[0], self.direction[1], self.kind as u32 as f32]
    }
}

/// A footstep or landing in water, for the FX system to splash.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WaterSplash {
    /// Render-space position at the sole.
    pub position: [f32; 3],
    pub landing: bool,
}

struct Actor {
    feet: Vec3,
    travel: f32,
    drag_travel: f32,
    next_foot: f32,
    vertical_speed: f32,
    last_seen: f32,
}

#[derive(Default)]
pub(crate) struct WakeTracker {
    events: Vec<WakeEvent>,
    splashes: Vec<WaterSplash>,
    actors: HashMap<u32, Actor>,
}

impl WakeTracker {
    pub(crate) fn events(&self) -> &[WakeEvent] {
        &self.events
    }

    pub(crate) fn clear(&mut self) {
        self.events.clear();
        self.splashes.clear();
        self.actors.clear();
    }

    pub(crate) fn take_splashes(&mut self) -> Vec<WaterSplash> {
        std::mem::take(&mut self.splashes)
    }

    /// Advances the tracker to `now` seconds. `people` yields `(id, sole position)`
    /// in render space for everyone who may wade; `camera` culls far events and
    /// `water` reports the standing water at a position.
    pub(crate) fn update(
        &mut self,
        now: f32,
        camera: Vec3,
        people: impl IntoIterator<Item = (u32, Vec3)>,
        water: &dyn Fn(Vec3) -> PuddleSample,
    ) {
        self.events.retain(|event| {
            let age = now - event.birth;
            (0.0..WAKE_LIFETIME).contains(&age)
        });

        for (id, feet) in people {
            if !feet.is_finite() || feet.distance_squared(camera) > MAX_CAMERA_DISTANCE * MAX_CAMERA_DISTANCE {
                self.actors.remove(&id);
                continue;
            }
            let actor = self.actors.entry(id).or_insert(Actor {
                feet,
                travel: 0.0,
                drag_travel: 0.0,
                next_foot: 1.0,
                vertical_speed: 0.0,
                last_seen: now,
            });
            let dt = (now - actor.last_seen).max(0.0);
            actor.last_seen = now;
            let delta = feet - actor.feet;
            actor.feet = feet;
            if dt <= 1.0e-4 {
                continue;
            }
            let horizontal = (delta.x * delta.x + delta.z * delta.z).sqrt();
            if horizontal > TELEPORT_DISTANCE || dt > 0.5 {
                actor.travel = 0.0;
                actor.drag_travel = 0.0;
                actor.vertical_speed = 0.0;
                continue;
            }
            let speed = horizontal / dt;
            let vertical_speed = delta.y / dt;
            let previous_vertical = actor.vertical_speed;
            actor.vertical_speed = vertical_speed;

            let direction = if horizontal > 1.0e-3 {
                [delta.x / horizontal, delta.z / horizontal]
            } else {
                [0.0, 1.0]
            };

            // Touch-down: falling fast a moment ago, now stopped by the ground.
            if previous_vertical < -LANDING_SPEED && vertical_speed > -LANDING_SPEED * 0.4 {
                let strength = (-previous_vertical / 520.0).clamp(0.35, 1.0);
                emit(&mut self.events, &mut self.splashes, water, WakeEvent { position: feet.to_array(), birth: now, strength, direction, kind: WakeKind::Landing });
            }

            if speed < MIN_SPEED || vertical_speed.abs() > 140.0 {
                continue;
            }
            let strength = (speed / FULL_SPEED).clamp(0.25, 1.0);

            actor.drag_travel += horizontal;
            while actor.drag_travel >= DRAG_DISTANCE {
                actor.drag_travel -= DRAG_DISTANCE;
                emit(&mut self.events, &mut self.splashes, water, WakeEvent { position: feet.to_array(), birth: now, strength: strength * 0.8, direction, kind: WakeKind::Drag });
            }

            actor.travel += horizontal;
            while actor.travel >= STRIDE_DISTANCE {
                actor.travel -= STRIDE_DISTANCE;
                actor.next_foot = -actor.next_foot;
                // Feet fall either side of the line of travel.
                let side = [-direction[1] * actor.next_foot, direction[0] * actor.next_foot];
                let position = [
                    feet.x + side[0] * FOOT_OFFSET,
                    feet.y,
                    feet.z + side[1] * FOOT_OFFSET,
                ];
                emit(&mut self.events, &mut self.splashes, water, WakeEvent { position, birth: now, strength, direction, kind: WakeKind::Step });
            }
        }

        self.actors.retain(|_, actor| now - actor.last_seen < ACTOR_FORGET_SECONDS);
    }

}

/// Records an event if it lies in water, deepening it with the depth of the
/// water, and queues a splash for footsteps and landings.
fn emit(
    events: &mut Vec<WakeEvent>,
    splashes: &mut Vec<WaterSplash>,
    water: &dyn Fn(Vec3) -> PuddleSample,
    mut event: WakeEvent,
) {
    let sample = water(Vec3::from_array(event.position));
    if sample.coverage < MIN_WATER {
        return;
    }
    event.strength = (event.strength * (0.6 + 0.4 * sample.depth)).min(1.0);
    if event.kind != WakeKind::Drag && splashes.len() < MAX_PENDING_SPLASHES {
        splashes.push(WaterSplash {
            position: event.position,
            landing: event.kind == WakeKind::Landing,
        });
    }
    push_event(events, event);
}

/// Appends an event, dropping the oldest when the buffer is full.
fn push_event(events: &mut Vec<WakeEvent>, event: WakeEvent) {
    if events.len() == WAKE_MAX_EVENTS {
        events.remove(0);
    }
    events.push(event);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deep(_: Vec3) -> PuddleSample {
        PuddleSample { coverage: 1.0, depth: 1.0 }
    }

    fn dry(_: Vec3) -> PuddleSample {
        PuddleSample::default()
    }

    fn walk(tracker: &mut WakeTracker, speed: f32, seconds: f32, water: &dyn Fn(Vec3) -> PuddleSample) {
        let steps = (seconds * 60.0) as usize;
        for i in 0..steps {
            let now = i as f32 / 60.0;
            let feet = Vec3::new(speed * now, 0.0, 0.0);
            tracker.update(now, Vec3::ZERO, [(1, feet)], water);
        }
    }

    #[test]
    fn running_through_water_leaves_alternating_steps_a_drag_trail_and_splashes() {
        let mut tracker = WakeTracker::default();
        walk(&mut tracker, 250.0, 0.5, &deep);
        let steps: Vec<WakeEvent> =
            tracker.events().iter().filter(|e| e.kind == WakeKind::Step).copied().collect();
        let step_count = steps.len();
        let drags = tracker.events().iter().filter(|e| e.kind == WakeKind::Drag).count();
        assert!(step_count >= 2, "steps {step_count}");
        assert!(drags >= step_count, "drags {drags}");
        // Consecutive steps land on opposite sides of the line of travel.
        assert!(steps[0].position[2] * steps[1].position[2] < 0.0);
        assert!(tracker.events().iter().all(|e| e.strength > 0.0 && e.strength <= 1.0));
        // Only steps splash, never the continuous drag.
        let splashes = tracker.take_splashes();
        assert_eq!(splashes.len(), step_count);
        assert!(splashes.iter().all(|splash| !splash.landing));
        assert!(tracker.take_splashes().is_empty());
    }

    #[test]
    fn dry_ground_leaves_nothing() {
        let mut tracker = WakeTracker::default();
        walk(&mut tracker, 250.0, 1.0, &dry);
        assert!(tracker.events().is_empty());
        assert!(tracker.take_splashes().is_empty());
    }

    #[test]
    fn shallow_water_is_gentler_than_deep_water() {
        let shallow = |_: Vec3| PuddleSample { coverage: 0.5, depth: 0.0 };
        let mut a = WakeTracker::default();
        let mut b = WakeTracker::default();
        walk(&mut a, 250.0, 0.4, &shallow);
        walk(&mut b, 250.0, 0.4, &deep);
        let strongest = |t: &WakeTracker| t.events().iter().map(|e| e.strength).fold(0.0, f32::max);
        assert!(strongest(&a) < strongest(&b));
    }

    #[test]
    fn standing_still_and_teleporting_leave_nothing() {
        let mut tracker = WakeTracker::default();
        walk(&mut tracker, 5.0, 1.0, &deep);
        assert!(tracker.events().is_empty());
        tracker.update(2.0, Vec3::ZERO, [(1, Vec3::new(0.0, 0.0, 0.0))], &deep);
        tracker.update(2.02, Vec3::ZERO, [(1, Vec3::new(900.0, 0.0, 0.0))], &deep);
        assert!(tracker.events().is_empty());
    }

    #[test]
    fn events_expire_and_the_buffer_never_overflows() {
        let mut tracker = WakeTracker::default();
        walk(&mut tracker, 260.0, 3.0, &deep);
        assert!(tracker.events().len() <= WAKE_MAX_EVENTS);
        tracker.update(10.0, Vec3::ZERO, std::iter::empty(), &deep);
        assert!(tracker.events().is_empty());
    }

    #[test]
    fn a_hard_landing_makes_a_big_splash() {
        let mut tracker = WakeTracker::default();
        tracker.update(0.00, Vec3::ZERO, [(2, Vec3::new(0.0, 100.0, 0.0))], &deep);
        tracker.update(0.05, Vec3::ZERO, [(2, Vec3::new(0.0, 88.0, 0.0))], &deep); // -240 u/s
        tracker.update(0.10, Vec3::ZERO, [(2, Vec3::new(0.0, 88.0, 0.0))], &deep); // stopped
        let landing = tracker.events().iter().find(|e| e.kind == WakeKind::Landing);
        assert!(landing.is_some_and(|event| event.strength >= 0.35));
        assert!(tracker.take_splashes().iter().any(|splash| splash.landing));
    }

    #[test]
    fn people_out_of_view_are_ignored() {
        let mut tracker = WakeTracker::default();
        for i in 0..60 {
            let now = i as f32 / 60.0;
            tracker.update(now, Vec3::ZERO, [(3, Vec3::new(5000.0 + 250.0 * now, 0.0, 0.0))], &deep);
        }
        assert!(tracker.events().is_empty());
    }
}
