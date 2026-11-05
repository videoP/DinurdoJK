//! Ghost-race comparison data: runners, phases, replays, the baked chase
//! camera keys and the phase sequence builder. Track sampling, the camera
//! bake and drawing stay in the client.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RacePhaseKind {
    Race,
    Slowdown,
    Pause,
    Rewind,
    Align,
    Replay,
    Return,
    Resume,
}

impl RacePhaseKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Race => "RACE · shared start clock",
            Self::Slowdown => "SLOWING · route difference ahead",
            Self::Pause => "PAUSED · compare these lines",
            Self::Rewind => "REWIND · returning to the fork",
            Self::Align => "ALIGNING SECTION STARTS",
            Self::Replay => "SECTION REPLAY · starts aligned · actual speeds",
            Self::Return => "RETURNING TO RACE",
            Self::Resume => "RESUMING RACE",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RaceRunner {
    pub name: String,
    pub game: String,
    pub duration_ms: i32,
    pub race_ms: i32,
    pub color: [u8; 3],
    /// (runner elapsed ms, common course progress). Persisted so live gaps
    /// need only interpolation, never path matching on the render thread.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub progress: Vec<[i32; 2]>,
}

impl RaceRunner {
    fn progress_at(&self, clock: i32) -> Option<f64> {
        let i = self.progress.partition_point(|p| p[0] <= clock);
        if i == 0 {
            return self.progress.first().map(|p| f64::from(p[1]));
        }
        let a = self.progress[i - 1];
        let Some(b) = self.progress.get(i) else {
            return Some(f64::from(a[1]));
        };
        let u = f64::from(clock - a[0]) / f64::from((b[0] - a[0]).max(1));
        Some(f64::from(a[1]) + f64::from(b[1] - a[1]) * u)
    }
    fn crossing_at(&self, progress: f64) -> Option<f64> {
        let i = self
            .progress
            .partition_point(|p| f64::from(p[1]) < progress);
        if i == 0 {
            return self.progress.first().map(|p| f64::from(p[0]));
        }
        let a = self.progress[i - 1];
        let Some(b) = self.progress.get(i) else {
            return Some(f64::from(a[0]));
        };
        let u = (progress - f64::from(a[1])) / f64::from((b[1] - a[1]).max(1));
        Some(f64::from(a[0]) + f64::from(b[0] - a[0]) * u)
    }
}

pub fn timing_bounds<'a>(
    race: &'a RaceComparison,
    phase: &'a RacePhase,
) -> Option<(&'a [i32], &'a [i32])> {
    if phase.kind == RacePhaseKind::Return {
        return None;
    }
    let replay = if phase.kind == RacePhaseKind::Replay {
        phase
    } else {
        let route = phase.replay.as_ref()?;
        race.phases.iter().find(|p| {
            p.kind == RacePhaseKind::Replay
                && p.section == phase.section
                && p.replay
                    .as_ref()
                    .is_some_and(|r| r.focus == route.focus && r.ordinal == route.ordinal)
        })?
    };
    let exits = replay
        .replay
        .as_ref()
        .filter(|r| r.exits.len() == race.runners.len())
        .map_or(replay.to.as_slice(), |r| r.exits.as_slice());
    Some((&replay.from, exits))
}

/// Race position and seconds behind the current leader at the same course
/// progress. During section replays, each clock starts at its own fork gate.
pub fn standings(race: &RaceComparison, sample: &RaceSample<'_>) -> Vec<(usize, Option<f64>)> {
    let bounds = timing_bounds(race, sample.phase);
    let entry = |i: usize| bounds.map_or(0, |b| b.0[i]);
    let exit = |i: usize| bounds.map_or(race.runners[i].race_ms, |b| b.1[i]);
    let elapsed = |i: usize| (sample.clocks[i].min(exit(i)) - entry(i)).max(0);
    let progress = |i: usize| race.runners[i].progress_at(sample.clocks[i]);
    let mut order: Vec<_> = (0..race.runners.len()).collect();
    order.sort_by(|&a, &b| {
        let done_a = sample.clocks[a] >= exit(a);
        let done_b = sample.clocks[b] >= exit(b);
        done_b
            .cmp(&done_a)
            .then_with(|| {
                if done_a && done_b {
                    elapsed(a).cmp(&elapsed(b))
                } else {
                    progress(b)
                        .unwrap_or(0.0)
                        .total_cmp(&progress(a).unwrap_or(0.0))
                }
            })
            .then_with(|| race.runners[a].race_ms.cmp(&race.runners[b].race_ms))
            .then(a.cmp(&b))
    });
    let leader = order[0];
    let mut result = vec![(0, None); order.len()];
    for (rank, &i) in order.iter().enumerate() {
        let gap = if i == leader {
            Some(0.0)
        } else if sample.clocks[i] >= exit(i) && sample.clocks[leader] >= exit(leader) {
            Some(f64::from(elapsed(i) - elapsed(leader)).max(0.0) / 1000.0)
        } else {
            progress(i)
                .and_then(|p| race.runners[leader].crossing_at(p))
                .map(|crossing| {
                    (f64::from(elapsed(i)) - (crossing - f64::from(entry(leader))).max(0.0))
                        .max(0.0)
                        / 1000.0
                })
        };
        result[i] = (rank + 1, gap);
    }
    result
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RacePhase {
    pub kind: RacePhaseKind,
    pub start: f64,
    pub duration: f64,
    /// Race elapsed clocks of every actor, in the same stable runner order.
    pub from: Vec<i32>,
    pub to: Vec<i32>,
    /// Native scene clock on the reference recording's source timeline.
    pub world_from: i32,
    pub world_to: i32,
    pub section: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay: Option<RaceReplay>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RaceReplay {
    pub focus: usize,
    pub previous_focus: usize,
    pub members: Vec<usize>,
    pub ordinal: usize,
    pub total: usize,
    pub entry_ms: i32,
    pub exit_ms: i32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exits: Vec<i32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RaceComparison {
    pub source_start_ms: i32,
    pub reference: usize,
    pub runners: Vec<RaceRunner>,
    pub phases: Vec<RacePhase>,
    pub orbits: bool,
    pub trails: bool,
    /// Baked chase camera (`bake_camera`). Empty in comparisons saved before
    /// it existed: those keep the per-frame solve.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub camera: Vec<RaceCameraKey>,
    #[serde(default)]
    pub camera_version: u32,
    #[serde(default)]
    pub style: RaceOverlayStyle,
}

/// Look of the standings and name tags, baked into preview and export.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RaceOverlayStyle {
    /// Empty: a packaged `fonts/studio_overlay.ttf`, else a system UI font.
    /// `crate::ui::STUDIO_FONT_CLASSIC`: JKA's ergoec. Otherwise a font path.
    pub font: String,
    pub readout_scale: f32,
    pub label_scale: f32,
    /// Name-tag opacity of runners other than the focus and reference.
    pub label_alpha: f32,
}

impl Default for RaceOverlayStyle {
    fn default() -> Self {
        Self { font: String::new(), readout_scale: 1.0, label_scale: 0.75, label_alpha: 0.55 }
    }
}

pub struct RaceSample<'a> {
    pub phase: &'a RacePhase,
    pub clocks: Vec<i32>,
    pub world_ms: i32,
    pub elapsed: f64,
}

impl RaceComparison {
    pub fn focus(&self, phase: &RacePhase) -> usize {
        if phase.kind == RacePhaseKind::Return {
            self.reference
        } else {
            phase.replay.as_ref().map_or(self.reference, |r| r.focus)
        }
    }
    /// Sound follows the displayed runner's clock, including alternate replays.
    /// The native world/reference clock can be held at its finish during these.
    pub fn audio_active(&self, sample: &RaceSample<'_>) -> bool {
        let focus = self.focus(sample.phase);
        matches!(sample.phase.kind, RacePhaseKind::Race | RacePhaseKind::Replay
            | RacePhaseKind::Slowdown | RacePhaseKind::Resume)
            && sample.clocks[focus] < self.runners[focus].race_ms
    }
    pub fn phase_label(&self, phase: &RacePhase) -> String {
        if let Some(route) = &phase.replay {
            let name = self.runners[route.focus]
                .name
                .split('-')
                .next()
                .unwrap_or(&self.runners[route.focus].name);
            if phase.kind == RacePhaseKind::Replay {
                format!(
                    "SECTION REPLAY {}/{} | {} | best of {} | real speed",
                    route.ordinal,
                    route.total,
                    name,
                    route.members.len()
                )
            } else {
                format!(
                    "{} | route {}/{}: {}",
                    phase.kind.label(),
                    route.ordinal,
                    route.total,
                    name
                )
            }
        } else {
            phase.kind.label().into()
        }
    }
    pub fn duration(&self) -> f64 {
        self.phases.last().map_or(0.0, |p| p.start + p.duration)
    }
    pub fn sample(&self, output: f64) -> RaceSample<'_> {
        let i = self
            .phases
            .partition_point(|p| p.start <= output)
            .saturating_sub(1)
            .min(self.phases.len() - 1);
        let phase = &self.phases[i];
        let elapsed = (output - phase.start).clamp(0.0, phase.duration);
        let u = (elapsed / phase.duration).clamp(0.0, 1.0);
        let progress = match phase.kind {
            RacePhaseKind::Race => u,
            RacePhaseKind::Slowdown => 2.0 * u - u * u,
            RacePhaseKind::Resume => u * u,
            _ => u * u * (3.0 - 2.0 * u),
        };
        let clocks: Vec<i32> = phase
            .from
            .iter()
            .zip(&phase.to)
            .map(|(&a, &b)| {
                if phase.kind == RacePhaseKind::Replay {
                    // A short acceleration into real 1x playback; never normalize
                    // runners to the same duration, which would hide speed gains.
                    let seconds = if elapsed < 0.5 {
                        elapsed * elapsed
                    } else {
                        elapsed - 0.25
                    };
                    (a + (seconds * 1000.0).round() as i32).min(b)
                } else {
                    (f64::from(a) + f64::from(b - a) * progress).round() as i32
                }
            })
            .collect();
        let world_ms = (self.source_start_ms
            + clocks[self.reference].min(self.runners[self.reference].race_ms))
        .clamp(phase.world_from, phase.world_to);
        RaceSample {
            phase,
            clocks,
            world_ms,
            elapsed,
        }
    }
    pub fn valid(&self, source_limit: i32) -> bool {
        (2..=32).contains(&self.runners.len())
            && self.reference < self.runners.len()
            && (-36_000_000..=36_000_000).contains(&self.source_start_ms)
            && !self.phases.is_empty()
            && self.phases.len() <= 512
            && self.duration() <= 36000.0
            && self.runners.iter().all(|r| {
                r.race_ms > 0
                    && r.race_ms <= 36_000_000
                    && r.duration_ms > 0
                    && r.progress.len() <= 4096
                    && r.progress
                        .iter()
                        .all(|p| p[0] >= 0 && p[0] <= r.race_ms && p[1] >= 0 && p[1] <= 36_000_000)
                    && r.progress
                        .windows(2)
                        .all(|p| p[0][0] < p[1][0] && p[0][1] <= p[1][1])
                    && !r.name.is_empty()
                    && !r.name.contains("..")
                    && !r.name.contains(['\\', ':'])
                    && !r.game.contains(['/', '\\', ':'])
                    && !r.game.contains("..")
            })
            && self.runners.iter().enumerate().all(|(i, r)| {
                !self.runners[..i]
                    .iter()
                    .any(|p| p.name == r.name && p.game == r.game)
            })
            && self.phases.iter().all(|p| {
                p.start.is_finite()
                    && p.start >= 0.0
                    && p.duration.is_finite()
                    && p.duration > 0.0
                    && p.replay.as_ref().is_none_or(|r| {
                        r.focus < self.runners.len()
                            && r.previous_focus < self.runners.len()
                            && r.members.contains(&r.focus)
                            && r.members.iter().all(|&i| i < self.runners.len())
                            && r.ordinal > 0
                            && r.ordinal <= r.total
                            && r.total <= 32
                            && r.entry_ms >= 0
                            && r.exit_ms > r.entry_ms
                            && (r.exits.is_empty()
                                || (r.exits.len() == self.runners.len()
                                    && r.exits.iter().all(|&t| (1..=36_000_000).contains(&t))))
                    })
                    && p.from.len() == self.runners.len()
                    && p.to.len() == self.runners.len()
                    && p.from
                        .iter()
                        .chain(&p.to)
                        .all(|&t| t >= 0 && t <= 36_000_000)
                    && p.world_from >= 0
                    && p.world_from <= p.world_to
                    && p.world_to <= source_limit
            })
            && self.camera.len() <= (self.duration() / CAMERA_KEY_STEP) as usize + 2
            && self.camera.iter().all(|k| {
                k.position.iter().all(|v| v.is_finite()) && (1.0..=170.0).contains(&k.fov)
            })
            && self.phases.first().is_some_and(|p| p.start == 0.0)
            && self.phases.windows(2).all(|w| {
                (w[0].start + w[0].duration - w[1].start).abs() < 1e-6
                    && w[0].to == w[1].from
                    && w[0].world_to == w[1].world_from
            })
    }
}

/// Spread comparison colors across the group instead of hashing into a small
/// repeated palette. Keep the reference white and highest contrast.
pub fn runner_color(index: usize, count: usize, reference: usize) -> [u8; 3] {
    if index == reference {
        return [245, 245, 245];
    }
    let slot = if index > reference { index - 1 } else { index };
    let h = slot as f32 / (count - 1).max(1) as f32 * 6.0;
    let sector = h.floor() as usize;
    let x = 1.0 - (h.rem_euclid(2.0) - 1.0).abs();
    let rgb = match sector {
        0 => [1.0, x, 0.0],
        1 => [x, 1.0, 0.0],
        2 => [0.0, 1.0, x],
        3 => [0.0, x, 1.0],
        4 => [x, 0.0, 1.0],
        _ => [1.0, 0.0, x],
    };
    rgb.map(|v| (65.0 + 190.0 * v).round() as u8)
}

/// A local comparison window, with each runner's own section entry and exit.
pub struct RaceSection {
    pub start_ms: i32,
    pub end_ms: i32,
    pub entries: Vec<i32>,
    pub exits: Vec<i32>,
    pub routes: Vec<RaceRouteGroup>,
}
#[derive(Clone, Debug)]
pub struct RaceRouteGroup {
    pub focus: usize,
    pub members: Vec<usize>,
}

pub fn build_sequence(
    runners: Vec<RaceRunner>,
    reference: usize,
    source_start: i32,
    source_limit: i32,
    sections: Vec<RaceSection>,
    orbits: bool,
    trails: bool,
) -> RaceComparison {
    use RacePhaseKind as K;
    let mut race = RaceComparison {
        source_start_ms: source_start,
        reference,
        runners,
        phases: Vec::new(),
        orbits,
        trails,
        camera: Vec::new(),
        camera_version: 0,
        style: RaceOverlayStyle::default(),
    };
    let count = race.runners.len();
    let finish = race.runners.iter().map(|r| r.race_ms).max().unwrap_or(1);
    let reference_finish = race.runners[reference].race_ms;
    let world_at = move |t: i32| (source_start + t.min(reference_finish)).clamp(0, source_limit);
    let mut cursor = 0;
    let mut add = |kind,
                   duration: f64,
                   from: Vec<i32>,
                   to: Vec<i32>,
                   world_from,
                   world_to,
                   section,
                   replay: Option<RaceReplay>| {
        if duration <= 0.0 {
            return;
        }
        let start = race.phases.last().map_or(0.0, |p| p.start + p.duration);
        race.phases.push(RacePhase {
            kind,
            start,
            duration,
            from,
            to,
            world_from,
            world_to,
            section,
            replay,
        });
    };
    for (index, section) in sections.into_iter().enumerate() {
        // The main chase already showed this line. Keep the reference as a
        // companion in every alternate pass, without repeating its own pass.
        let routes: Vec<_> = section
            .routes
            .iter()
            .filter(|g| !g.members.contains(&reference))
            .collect();
        let pause = section.end_ms.min(reference_finish);
        let slow = pause - 500;
        if slow < cursor + 500 || section.start_ms >= slow || routes.is_empty() {
            continue;
        }
        let world = world_at(pause);
        add(
            K::Race,
            f64::from(slow - cursor) / 1000.0,
            vec![cursor; count],
            vec![slow; count],
            world_at(cursor),
            world_at(slow),
            0,
            None,
        );
        add(
            K::Slowdown,
            1.0,
            vec![slow; count],
            vec![pause; count],
            world_at(slow),
            world,
            index + 1,
            None,
        );
        add(
            K::Pause,
            0.7,
            vec![pause; count],
            vec![pause; count],
            world,
            world,
            index + 1,
            None,
        );
        add(
            K::Rewind,
            (f64::from(pause - section.start_ms) / 6500.0).clamp(0.35, 1.5),
            vec![pause; count],
            vec![section.start_ms; count],
            world,
            world,
            index + 1,
            None,
        );
        let mut clocks = vec![section.start_ms; count];
        let mut previous = None::<RaceReplay>;
        for (ordinal, group) in routes.iter().enumerate() {
            let meta = RaceReplay {
                focus: group.focus,
                previous_focus: previous.as_ref().map_or(reference, |r| r.focus),
                members: group.members.clone(),
                ordinal: ordinal + 1,
                total: routes.len(),
                entry_ms: section.entries[group.focus],
                exit_ms: section.exits[group.focus],
                exits: section.exits.clone(),
            };
            if let Some(last) = &previous {
                add(
                    K::Pause,
                    0.6,
                    clocks.clone(),
                    clocks.clone(),
                    world,
                    world,
                    index + 1,
                    Some(last.clone()),
                );
                let span = clocks
                    .iter()
                    .zip(&section.entries)
                    .map(|(a, b)| (a - b).abs())
                    .max()
                    .unwrap_or(0);
                add(
                    K::Rewind,
                    (f64::from(span) / 6500.0).clamp(0.35, 1.5),
                    clocks.clone(),
                    section.entries.clone(),
                    world,
                    world,
                    index + 1,
                    Some(last.clone()),
                );
                clocks = section.entries.clone();
            }
            add(
                K::Align,
                0.9,
                clocks,
                section.entries.clone(),
                world,
                world,
                index + 1,
                Some(meta.clone()),
            );
            let span = (section.exits[group.focus] - section.entries[group.focus])
                .max(section.exits[reference] - section.entries[reference]);
            clocks = section
                .entries
                .iter()
                .zip(&section.exits)
                .map(|(&a, &b)| (a + span).min(b))
                .collect();
            add(
                K::Replay,
                f64::from(span) / 1000.0 + 0.25,
                section.entries.clone(),
                clocks.clone(),
                world,
                world,
                index + 1,
                Some(meta.clone()),
            );
            previous = Some(meta);
        }
        add(
            K::Return,
            1.5,
            clocks,
            vec![pause; count],
            world,
            world,
            index + 1,
            previous,
        );
        cursor = (pause + 500).min(finish);
        if cursor > pause {
            add(
                K::Resume,
                2.0 * f64::from(cursor - pause) / 1000.0,
                vec![pause; count],
                vec![cursor; count],
                world,
                world_at(cursor),
                index + 1,
                None,
            );
        }
    }
    if finish > cursor {
        add(
            K::Race,
            f64::from(finish - cursor) / 1000.0,
            vec![cursor; count],
            vec![finish; count],
            world_at(cursor),
            world_at(finish),
            0,
            None,
        );
    }
    race
}

pub const CAMERA_VERSION: u32 = 2;

/// Studio seconds between baked chase-camera keys.
pub const CAMERA_KEY_STEP: f64 = 0.05;

/// One baked chase-camera key. Key `k` is at output `k * CAMERA_KEY_STEP`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RaceCameraKey {
    /// Map-space position.
    pub position: [f32; 3],
    pub fov: f32,
}

/// Per-phase adjustments to the planned race camera, persisted with its shot.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RaceCameraEdit {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub height: f32,
    pub side: f32,
    pub fov: Option<f32>,
    pub lens: Option<crate::lens::StudioLens>,
}

impl Default for RaceCameraEdit {
    fn default() -> Self {
        Self { yaw: 0.0, pitch: 0.0, distance: 1.0, height: 0.0, side: 0.0, fov: None, lens: None }
    }
}

impl RaceCameraEdit {
    pub fn valid(&self) -> bool {
        self.yaw.is_finite() && self.yaw.abs() <= 180.0
            && self.pitch.is_finite() && self.pitch.abs() <= 80.0
            && self.distance.is_finite() && (0.1..=4.0).contains(&self.distance)
            && self.height.is_finite() && self.height.abs() <= 512.0
            && self.side.is_finite() && self.side.abs() <= 512.0
            && self.fov.is_none_or(|f| f.is_finite() && (10.0..=140.0).contains(&f))
            && self.lens.is_none_or(|lens| lens.valid())
    }

    pub fn position(&self, position: glam::Vec3, target: glam::Vec3) -> glam::Vec3 {
        let boom = position - target;
        let yaw = boom.y.atan2(boom.x) + self.yaw.to_radians();
        let pitch = (boom.z.atan2(boom.truncate().length()) + self.pitch.to_radians())
            .clamp(-1.5, 1.5);
        let (sn, cs) = yaw.sin_cos();
        let radius = boom.length() * self.distance;
        target + glam::Vec3::new(cs * pitch.cos(), sn * pitch.cos(), pitch.sin()) * radius
            + glam::Vec3::Z * self.height + glam::Vec3::new(-sn, cs, 0.0) * self.side
    }
}

impl RaceComparison {
    /// The baked chase camera at `output`: map-space position and lens.
    /// None for comparisons saved before cameras were baked.
    pub fn camera_key_at(&self, output: f64) -> Option<(glam::Vec3, f32)> {
        use glam::Vec3;
        let keys = &self.camera;
        if keys.len() < 2 {
            return None;
        }
        let x = (output / CAMERA_KEY_STEP).max(0.0);
        let i = (x.floor() as usize).min(keys.len() - 2);
        let u = (x - i as f64).clamp(0.0, 1.0) as f32;
        let key = |j: usize| keys[j.min(keys.len() - 1)];
        let (a, b) = (key(i), key(i + 1));
        let (p1, p2) = (Vec3::from_array(a.position), Vec3::from_array(b.position));
        // A teleport is a cut, not a flight across the map.
        const CUT: f32 = 600.0;
        if p1.distance(p2) > CUT {
            return Some(if u < 0.5 { (p1, a.fov) } else { (p2, b.fov) });
        }
        let p0 = Vec3::from_array(key(i.saturating_sub(1)).position);
        let p3 = Vec3::from_array(key(i + 2).position);
        let p0 = if p0.distance(p1) > CUT { p1 } else { p0 };
        let p3 = if p3.distance(p2) > CUT { p2 } else { p3 };
        // Catmull-Rom through the keys.
        let (u2, u3) = (u * u, u * u * u);
        let position = 0.5
            * (2.0 * p1
                + (p2 - p0) * u
                + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * u2
                + (3.0 * p1 - p0 - 3.0 * p2 + p3) * u3);
        Some((position, a.fov + (b.fov - a.fov) * u))
    }
}

