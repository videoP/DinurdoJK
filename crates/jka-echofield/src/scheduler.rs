//! EchoField SampleEmitterScheduler.java: original 20 Hz countdowns at 4a595e7.
//! IDs must be unique per emitter lifetime.
use crate::{environment::EnvironmentConditions, random::JavaRandom};
use std::collections::HashMap;
#[derive(Clone, Copy, Debug)]
pub enum TriggerKind {
    Bird,
    Insect,
    Frog,
    Bat,
    WaterDrip,
    WaterRain,
    CaveAmbient,
    Mechanical,
    WaterStill,
    Other,
}
#[derive(Default)]
pub struct SampleScheduler {
    countdowns: HashMap<u64, i32>,
}
impl SampleScheduler {
    pub fn should_trigger(
        &mut self,
        id: u64,
        kind: TriggerKind,
        cond: &EnvironmentConditions,
        rng: &mut JavaRandom,
    ) -> bool {
        let mut remaining = self.countdowns.get(&id).copied().unwrap_or(-1);
        if remaining < 0 {
            remaining = initial_interval(kind, rng);
        }
        remaining -= 1;
        if remaining <= 0 {
            self.countdowns.insert(id, interval(kind, cond, rng));
            true
        } else {
            self.countdowns.insert(id, remaining);
            false
        }
    }
    pub fn remove(&mut self, id: u64) {
        self.countdowns.remove(&id);
    }
    pub fn clear(&mut self) {
        self.countdowns.clear();
    }
}
fn initial_interval(kind: TriggerKind, rng: &mut JavaRandom) -> i32 {
    use TriggerKind::*;
    match kind {
        Bird => 5 + rng.next_int(30),
        Insect => 5 + rng.next_int(20),
        WaterDrip => 3 + rng.next_int(15),
        CaveAmbient => 5 + rng.next_int(25),
        Mechanical => 5 + rng.next_int(20),
        _ => 5 + rng.next_int(30),
    }
}
fn interval(kind: TriggerKind, c: &EnvironmentConditions, rng: &mut JavaRandom) -> i32 {
    use TriggerKind::*;
    match kind {
        Bird => {
            if c.dawn_chorus > 0.5 {
                10 + rng.next_int(25)
            } else if c.dawn_chorus > 0.1 {
                20 + rng.next_int(40)
            } else {
                60 + rng.next_int(140)
            }
        }
        Insect => {
            if 1.0 - c.daylight > 0.5 {
                10 + rng.next_int(35)
            } else {
                40 + rng.next_int(80)
            }
        }
        Frog => 60 + rng.next_int(120),
        Bat => 80 + rng.next_int(120),
        WaterDrip => 20 + rng.next_int(80),
        WaterRain => {
            if c.is_raining {
                10 + rng.next_int(20)
            } else {
                200
            }
        }
        CaveAmbient => {
            let scale = (1.0 + (c.room_size - 5.0) * 0.06).max(1.0).min(1.6);
            let base = (100.0 * scale) as i32;
            let range = (200.0 * scale) as i32;
            base + rng.next_int(range.max(1))
        }
        Mechanical => 60 + rng.next_int(100),
        WaterStill => 100 + rng.next_int(200),
        Other => 100,
    }
}
