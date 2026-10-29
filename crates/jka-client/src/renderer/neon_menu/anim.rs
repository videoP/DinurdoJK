//! Everything on the main page that moves, simulated on the render thread from
//! the app's `MenuFrame` edges (selection, press, reveal). The app never sends
//! per-frame animation state.
use crate::neon_menu::{MenuFrame, ITEM_COUNT};

/// Lit level of a sign nobody is pointing at: readable through the fog,
/// on a tired transformer.
pub(super) const IDLE: f32 = 0.5;
/// Lit level of the sign under the pointer. Tube gain is set so IDLE sits
/// below the tone-curve shoulder and this blows well past it into bloom.
const LIT: f32 = 2.2;
/// Rollover strike: the tube drops out and catches several times.
const STRIKE: f32 = 0.42;
/// Seconds for the condensation to close a wipe again.
const REFOG: f32 = 6.5;
pub(super) const MAX_DRIPS: usize = 16;
/// "CHAT LOGS": the G has a bad electrode.
pub(super) const FAULTY_ITEM: usize = 4;
pub(super) const FAULTY_GLYPH: u32 = 6;

/// Pixel-space geometry of the page this frame.
pub(super) struct Geometry {
    pub scale: f32,
    /// Wipe area per item: [x0, y0, x1, y1].
    pub wipe: [[f32; 4]; ITEM_COUNT],
    /// Neon ink extent per item, for the power-on fill: [x0, x1].
    pub ink: [[f32; 2]; ITEM_COUNT],
    pub sill: f32,
    pub width: f32,
    pub monitor: [f32; 4],
    pub entry_count: usize,
}

#[derive(Clone, Copy, Default)]
pub(super) struct Wipe {
    pub progress: f32,
    pub clear: f32,
    pub tilt: f32,
    pub seed: f32,
    dripped: bool,
}

#[derive(Clone, Copy, Default)]
pub(super) struct ItemAnim {
    pub level: f32,
    pub fill: f32,
    pub flash: f32,
    pub wipe: Wipe,
}

#[derive(Clone, Copy)]
pub(super) struct Drip {
    pub x: f32,
    pub y: f32,
    pub r: f32,
    target_r: f32,
    pub alpha: f32,
    pub top: f32,
    pub trail: f32,
    pub stretch: f32,
    pub seed: f32,
    vel: f32,
    age: f32,
    wait: f32,
}

pub(super) struct Anim {
    pub time: f32,
    pub revealed_at: Option<f32>,
    pub selected: usize,
    pub select_time: f32,
    active: Option<usize>,
    active_time: f32,
    press_serial: u32,
    pub items: [ItemAnim; ITEM_COUNT],
    pub drips: Vec<Drip>,
    next_ambient: f32,
    rng: u32,
    pub lightbox_item: usize,
    pub lightbox_power: f32,
    lightbox_switch: f32,
    pub monitor_power: f32,
}

impl Default for Anim {
    fn default() -> Self {
        Self {
            time: 0.0,
            revealed_at: None,
            selected: 0,
            select_time: -10.0,
            active: None,
            active_time: -10.0,
            press_serial: 0,
            items: [ItemAnim::default(); ITEM_COUNT],
            drips: Vec::new(),
            next_ambient: 3.0,
            rng: 0x9E37_79B9,
            lightbox_item: 0,
            lightbox_power: 0.0,
            lightbox_switch: -10.0,
            monitor_power: 0.0,
        }
    }
}

pub(super) fn hash(x: f32) -> f32 {
    let v = (x * 12.9898).sin() * 43_758.547;
    v - v.floor()
}

/// A neon tube or fluorescent starter catching: random on/off at `rate` Hz,
/// increasingly likely to be on as `t` approaches `length`.
fn strike(t: f32, length: f32, rate: f32, seed: f32) -> f32 {
    if t >= length {
        return 1.0;
    }
    let chance = (t / length).powf(0.6);
    if hash((t * rate).floor() + seed * 31.7) < chance { 1.0 } else { 0.0 }
}

impl Anim {
    fn random(&mut self) -> f32 {
        // xorshift32
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    fn power_on_time(&self, index: usize) -> Option<f32> {
        self.revealed_at.map(|at| at + 0.03 + index as f32 * 0.065)
    }

    /// Fault level of the bad letter: steady, then a stuttering burst every
    /// few seconds, like a sign nobody has fixed.
    pub fn fault_level(&self) -> f32 {
        let cycle = 6.7;
        let phase = self.time % cycle;
        let burst = (self.time / cycle).floor();
        if phase < 0.5 + hash(burst) * 0.6 {
            if hash((self.time * 26.0).floor()) > 0.45 { 1.0 } else { 0.07 }
        } else {
            1.0
        }
    }

    pub fn update(&mut self, dt: f32, frame: &MenuFrame, geo: &Geometry) {
        let dt = dt.clamp(0.0, 0.1);
        self.time += dt;
        let time = self.time;
        if frame.revealed && self.revealed_at.is_none() {
            self.revealed_at = Some(time);
        }
        let selected = usize::from(frame.selected).min(geo.entry_count - 1);
        if selected != self.selected {
            self.selected = selected;
            self.select_time = time;
            self.lightbox_switch = time;
        }
        let active = frame.active.map(|a| usize::from(a).min(geo.entry_count - 1));
        if active != self.active {
            self.active = active;
            self.active_time = time;
        }
        if frame.press_serial != self.press_serial {
            self.press_serial = frame.press_serial;
            if let Some(item) = self.items.get_mut(usize::from(frame.pressed)) {
                item.flash = 1.0;
            }
        }

        for index in 0..ITEM_COUNT {
            if index >= geo.entry_count {
                self.items[index] = ItemAnim::default();
                continue;
            }
            let power_on = self.power_on_time(index);
            let is_active = Some(index) == self.active;
            let active_time = self.active_time;
            let item = &mut self.items[index];
            item.flash *= (-dt * 9.0).exp();
            let Some(on) = power_on.filter(|on| time >= *on) else {
                item.level = 0.0;
                item.fill = 0.0;
                continue;
            };
            let since_on = time - on;
            // Light runs along the tube as it first strikes.
            item.fill = (since_on / 0.32).min(1.0);
            // Each idle sign breathes a little on its own ballast.
            let idle = IDLE * (1.0 + 0.05 * (time * 0.8 + index as f32 * 1.7).sin());
            let target = if is_active {
                let since = time - active_time.max(on);
                if since < STRIKE {
                    // Drops below idle and catches, faster as it warms up.
                    let rate = 22.0 + since * 60.0;
                    let chance = 0.25 + 0.75 * (since / STRIKE);
                    if hash((since * rate).floor() + index as f32 * 3.1) < chance { LIT * 1.2 } else { idle * 0.2 }
                } else {
                    // Overshoot, then a visible hum while the pointer stays.
                    let hum = 0.07 * (hash((time * 14.0).floor() + index as f32) - 0.5);
                    LIT * (1.0 + 0.3 * (-(since - STRIKE) * 5.0).exp() + hum)
                }
            } else {
                idle
            };
            let start = strike(since_on, 0.3, 28.0, index as f32);
            if is_active || target > item.level {
                item.level = target * start;
            } else {
                // Selected signs fall back gently, like a dimmed ballast.
                item.level = target + (item.level - target) * (-dt * 7.0).exp();
            }
        }

        self.update_wipes(dt, geo);
        self.update_drips(dt, geo);

        // Lightbox: fluorescent start on reveal, a blink while it changes text.
        let lightbox_on = self.revealed_at.map(|at| at + 0.8);
        self.lightbox_power = match lightbox_on {
            Some(on) if time >= on => {
                let start = strike(time - on, 0.55, 14.0, 9.0);
                let since = time - self.lightbox_switch;
                let blink = if since < 0.22 && hash((since * 30.0).floor() + 4.0) < 0.6 { 0.15 } else { 1.0 };
                if since >= 0.11 {
                    self.lightbox_item = self.selected;
                }
                start * blink
            }
            _ => 0.0,
        };
        self.monitor_power = match self.revealed_at {
            Some(at) if time > at + 0.3 => ((time - at - 0.3) / 0.35).min(1.0),
            _ => 0.0,
        };
    }

    fn update_wipes(&mut self, dt: f32, geo: &Geometry) {
        let time = self.time;
        let mut spawns: Vec<(usize, [f32; 4])> = Vec::new();
        let mut hands: Vec<([f32; 4], f32)> = Vec::new();
        for index in 0..geo.entry_count {
            let ready = self.power_on_time(index).is_some_and(|on| time >= on + 0.25);
            let active = Some(index) == self.active && ready;
            let (seed, tilt) = (self.random(), self.random());
            let wipe = &mut self.items[index].wipe;
            if active {
                if wipe.progress <= 0.0 || (wipe.clear < 0.35 && wipe.progress >= 1.0) {
                    *wipe = Wipe { progress: 0.0001, clear: 0.0, tilt: (tilt - 0.5) * 0.07, seed, dripped: false };
                }
                // A hand takes longer across a longer word.
                let width = (geo.wipe[index][2] - geo.wipe[index][0]) / geo.scale.max(0.1);
                let duration = (0.3 + width / 1100.0).clamp(0.35, 0.75);
                wipe.progress = (wipe.progress + dt / duration).min(1.0);
                wipe.clear = (wipe.clear + dt * 7.0).min(1.0);
                let rect = geo.wipe[index];
                let eased = wipe.progress * wipe.progress * (3.0 - 2.0 * wipe.progress);
                hands.push((rect, rect[0] + (rect[2] - rect[0]) * eased));
            } else if wipe.progress > 0.0 {
                // The breath on the glass creeps back in from the edges
                // (the shader shrinks the wipe as `clear` falls).
                wipe.clear -= dt / REFOG;
                if wipe.clear <= 0.0 {
                    *wipe = Wipe::default();
                }
            }
            if wipe.progress >= 1.0 && !wipe.dripped {
                wipe.dripped = true;
                spawns.push((index, geo.wipe[index]));
            }
        }
        // Beads the hand has passed over are gone.
        for drip in &mut self.drips {
            if hands.iter().any(|(rect, front)| {
                drip.x >= rect[0] && drip.x <= *front && drip.y >= rect[1] && drip.y <= rect[3]
            }) {
                drip.alpha = (drip.alpha - dt * 9.0).max(0.0);
                drip.trail = 0.0;
            }
        }
        // Water pushed to the lower edge of a wipe gathers into beads just
        // below it.
        for (index, rect) in spawns {
            let tilt = self.items[index].wipe.tilt;
            let count = 2 + (self.random() * 2.0) as usize;
            for k in 0..count {
                let along = if k == 0 { 0.92 + self.random() * 0.06 } else { 0.1 + self.random() * 0.8 };
                let x = rect[0] + (rect[2] - rect[0]) * along;
                let r = (2.2 + self.random() * 3.2) * geo.scale * if k == 0 { 1.35 } else { 1.0 };
                let lower = rect[3] + r * 1.2 + (x - (rect[0] + rect[2]) * 0.5) * -tilt;
                let wait = 0.4 + self.random() * 2.2;
                self.spawn_drip(x, lower, r, wait);
            }
        }
    }

    fn spawn_drip(&mut self, x: f32, y: f32, r: f32, wait: f32) {
        if self.drips.len() >= MAX_DRIPS {
            self.drips.remove(0);
        }
        let seed = self.random();
        self.drips.push(Drip {
            x, y, r: r * 0.3, target_r: r, alpha: 1.0, top: y, trail: 0.0, stretch: 1.0, seed,
            vel: 0.0, age: 0.0, wait,
        });
    }

    fn update_drips(&mut self, dt: f32, geo: &Geometry) {
        let time = self.time;
        // Condensation keeps forming and running even while nobody touches it.
        if self.revealed_at.is_some() && time >= self.next_ambient {
            self.next_ambient = time + 2.0 + self.random() * 3.5;
            // Many of them settle over the signs, so the menu slowly gets
            // covered until someone wipes it.
            let (x, y) = if self.random() < 0.45 {
                let rect = geo.wipe[((self.random() * geo.entry_count as f32) as usize).min(geo.entry_count - 1)];
                (rect[0] + (rect[2] - rect[0]) * self.random(), rect[1] + (rect[3] - rect[1]) * self.random())
            } else {
                ((0.04 + self.random() * 0.92) * geo.width, (0.06 + self.random() * 0.55) * geo.sill)
            };
            let m = geo.monitor;
            if !(x > m[0] - 20.0 && x < m[2] + 20.0 && y > m[1] - 20.0) {
                let r = (1.6 + self.random() * 3.4) * geo.scale;
                let wait = 1.0 + self.random() * 7.0;
                self.spawn_drip(x, y, r, wait);
            }
        }
        let scale = geo.scale;
        for drip in &mut self.drips {
            drip.age += dt;
            drip.r = (drip.r + dt * drip.target_r * 1.6).min(drip.target_r);
            if drip.wait > 0.0 {
                drip.wait -= dt;
            } else {
                // Stick-slip: drier patches of glass hold the bead, then it
                // breaks free. Heavier beads run faster.
                let slip = value_noise(drip.y * 0.04 + drip.seed * 17.0);
                let heavy = (drip.r / (3.0 * scale)).clamp(0.4, 1.6);
                if slip < 0.32 {
                    drip.vel *= (-dt * 10.0).exp();
                } else {
                    drip.vel = (drip.vel + 300.0 * scale * heavy * dt).min(150.0 * scale * heavy);
                }
                drip.y += drip.vel * dt;
                drip.r = (drip.r - dt * drip.vel * 0.0022).max(1.1 * scale);
                drip.target_r = drip.target_r.min(drip.r);
                drip.x += (value_noise(drip.y * 0.02 + drip.seed * 5.0) - 0.5) * drip.vel * dt * 0.12;
            }
            drip.stretch = 1.0 + (drip.vel / (60.0 * scale)).min(1.0) * 0.9;
            let travelled = drip.y - drip.top;
            if travelled > drip.r {
                drip.trail = (1.0 - (drip.age - 6.0) / 7.0).clamp(0.0, 1.0);
            }
            if drip.age > 24.0 {
                drip.alpha -= dt / 2.5;
            }
        }
        let sill = geo.sill;
        self.drips.retain(|drip| drip.alpha > 0.0 && drip.y - drip.r < sill);
    }
}

fn value_noise(x: f32) -> f32 {
    let i = x.floor();
    let f = x - i;
    let u = f * f * (3.0 - 2.0 * f);
    hash(i) * (1.0 - u) + hash(i + 1.0) * u
}
