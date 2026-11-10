//! cg_specHud card bars: health and Force ease to each new value, and a lost
//! chunk lingers briefly, then drains (fighting-game damage trail). Numbers on
//! the cards stay exact; only the bar fills animate. Pure functions of time,
//! so any publish rate shows the same motion.
use std::time::{Duration, Instant};

const EASE: Duration = Duration::from_millis(180);
const TRAIL_HOLD: Duration = Duration::from_millis(380);
const TRAIL_DRAIN: Duration = Duration::from_millis(420);

pub(in crate::app) const BAR_HEALTH: usize = 0;
pub(in crate::app) const BAR_FORCE: usize = 1;

#[derive(Clone, Copy, Debug)]
struct Tween {
    from: f32,
    to: f32,
    start: Instant,
    duration: Duration,
}

impl Tween {
    fn still(value: f32, now: Instant) -> Self {
        Self { from: value, to: value, start: now, duration: Duration::ZERO }
    }

    fn value(&self, now: Instant) -> f32 {
        // Before a delayed start this is 0, so the tween holds `from`.
        let elapsed = now.saturating_duration_since(self.start).as_secs_f32();
        let t = if self.duration.is_zero() { 1.0 } else { (elapsed / self.duration.as_secs_f32()).clamp(0.0, 1.0) };
        let eased = 1.0 - (1.0 - t).powi(3);
        self.from + (self.to - self.from) * eased
    }

    fn done(&self, now: Instant) -> bool {
        now >= self.start + self.duration
    }
}

#[derive(Clone, Copy, Debug)]
struct Bar {
    fill: Tween,
    trail: Tween,
}

impl Bar {
    fn new(value: f32, now: Instant) -> Self {
        Self { fill: Tween::still(value, now), trail: Tween::still(value, now) }
    }

    fn set(&mut self, target: f32, now: Instant) {
        if (target - self.fill.to).abs() < 1e-4 {
            return;
        }
        let shown = self.fill.value(now);
        let trail_shown = self.trail.value(now).max(shown);
        self.fill = Tween { from: shown, to: target, start: now, duration: EASE };
        self.trail = if target < shown {
            // Loss: the chunk holds (stacking with any chunk still showing),
            // then drains down to the new value.
            Tween { from: trail_shown, to: target, start: now + TRAIL_HOLD, duration: TRAIL_DRAIN }
        } else {
            // Gain: no chunk.
            self.fill
        };
    }

    fn sample(&self, now: Instant) -> (f32, f32) {
        let fill = self.fill.value(now);
        (fill, self.trail.value(now).max(fill))
    }

    fn animating(&self, now: Instant) -> bool {
        !self.fill.done(now) || !self.trail.done(now)
    }
}

#[derive(Default)]
pub(in crate::app) struct BarAnimations {
    bars: [[Option<Bar>; 2]; 32],
}

impl BarAnimations {
    /// Moves bar `kind` of `client` toward `target` (0..1) and returns the
    /// shown (fill, trail) as per-mille. `None` clears it: a bar that comes
    /// back (respawn, new sample) starts at its value without animating.
    pub(in crate::app) fn sample(&mut self, client: usize, kind: usize, target: Option<f32>, now: Instant) -> Option<(u16, u16)> {
        let slot = self.bars.get_mut(client)?.get_mut(kind)?;
        let Some(target) = target.filter(|t| t.is_finite()).map(|t| t.clamp(0.0, 1.0)) else {
            *slot = None;
            return None;
        };
        let bar = slot.get_or_insert_with(|| Bar::new(target, now));
        bar.set(target, now);
        let (fill, trail) = bar.sample(now);
        let mille = |v: f32| (v * 1000.0).round() as u16;
        Some((mille(fill), mille(trail)))
    }

    pub(in crate::app) fn animating(&self, now: Instant) -> bool {
        self.bars.iter().flatten().flatten().any(|bar| bar.animating(now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loss_eases_fill_and_holds_then_drains_trail() {
        let now = Instant::now();
        let mut bars = BarAnimations::default();
        assert_eq!(bars.sample(1, BAR_HEALTH, Some(1.0), now), Some((1000, 1000)));
        let (fill, trail) = bars.sample(1, BAR_HEALTH, Some(0.4), now).unwrap();
        assert_eq!((fill, trail), (1000, 1000));
        let later = now + EASE;
        let (fill, trail) = bars.sample(1, BAR_HEALTH, Some(0.4), later).unwrap();
        assert_eq!(fill, 400);
        assert_eq!(trail, 1000, "chunk still held");
        let drained = now + TRAIL_HOLD + TRAIL_DRAIN;
        assert_eq!(bars.sample(1, BAR_HEALTH, Some(0.4), drained), Some((400, 400)));
        assert!(!bars.animating(drained));
    }

    #[test]
    fn gain_has_no_chunk_and_none_resets() {
        let now = Instant::now();
        let mut bars = BarAnimations::default();
        bars.sample(2, BAR_FORCE, Some(0.2), now);
        let (fill, trail) = bars.sample(2, BAR_FORCE, Some(0.9), now + EASE / 2).unwrap();
        assert_eq!(fill, trail);
        assert_eq!(bars.sample(2, BAR_FORCE, None, now), None);
        assert_eq!(bars.sample(2, BAR_FORCE, Some(0.5), now), Some((500, 500)));
    }
}
