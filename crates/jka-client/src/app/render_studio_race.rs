//! One retained ghost race, with an output clock that can pause and rewind.
//! Native demo/world time never runs backward: only indexed ghost poses do.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::app) enum RacePhaseKind {
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
pub(in crate::app) struct RaceRunner {
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

fn timing_bounds<'a>(
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
fn standings(race: &RaceComparison, sample: &RaceSample<'_>) -> Vec<(usize, Option<f64>)> {
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
pub(in crate::app) struct RacePhase {
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
pub(in crate::app) struct RaceReplay {
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
pub(in crate::app) struct RaceComparison {
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
pub(in crate::app) struct RaceOverlayStyle {
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

pub(in crate::app) struct RaceSample<'a> {
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
pub(in crate::app) fn runner_color(index: usize, count: usize, reference: usize) -> [u8; 3] {
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
pub(in crate::app) struct RaceSection {
    pub start_ms: i32,
    pub end_ms: i32,
    pub entries: Vec<i32>,
    pub exits: Vec<i32>,
    pub routes: Vec<RaceRouteGroup>,
}
#[derive(Clone, Debug)]
pub(in crate::app) struct RaceRouteGroup {
    pub focus: usize,
    pub members: Vec<usize>,
}

pub(in crate::app) fn build_sequence(
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

/// Interpolate the decoder's indexed path without seeking a native session.
pub(in crate::app) fn track_position(
    track: &super::race_ghost::RaceGhostTrack,
    ms: i32,
) -> glam::Vec3 {
    let i = track
        .frames
        .partition_point(|f| f.elapsed_ms <= ms)
        .saturating_sub(1)
        .min(track.frames.len() - 1);
    let a = &track.frames[i];
    let p = glam::Vec3::from_array(
        super::playerstate_vec3(&a.player_state, "origin").unwrap_or([0.0; 3]),
    );
    let Some(b) = track.frames.get(i + 1) else {
        return p;
    };
    if (a.player_state.field_i32("eFlags").unwrap_or(0)
        ^ b.player_state.field_i32("eFlags").unwrap_or(0))
        & 8
        != 0
    {
        return p;
    }
    let q = glam::Vec3::from_array(
        super::playerstate_vec3(&b.player_state, "origin").unwrap_or(p.to_array()),
    );
    p.lerp(
        q,
        ((ms - a.elapsed_ms) as f32 / (b.elapsed_ms - a.elapsed_ms).max(1) as f32).clamp(0.0, 1.0),
    )
}

/// Each runner's indexed track, in runner order (None while still loading).
pub(in crate::app) fn runner_tracks<'a>(
    race: &RaceComparison,
    runtime: &'a [super::render_studio_demo::StudioRaceRuntime],
) -> Vec<Option<&'a super::race_ghost::RaceGhostTrack>> {
    race.runners
        .iter()
        .map(|runner| {
            runtime
                .iter()
                .find(|r| r.key.0 == runner.name && r.key.1 == runner.game)
                .and_then(|r| r.ghost.as_ref())
                .map(|g| &g.track)
        })
        .collect()
}

fn track_poses(
    tracks: &[Option<&super::race_ghost::RaceGhostTrack>],
    sample: &RaceSample<'_>,
) -> Vec<glam::Vec3> {
    tracks
        .iter()
        .zip(&sample.clocks)
        .map(|(track, &ms)| track.map_or(glam::Vec3::ZERO, |t| track_position(t, ms)))
        .collect()
}

pub(in crate::app) fn poses(
    race: &RaceComparison,
    sample: &RaceSample<'_>,
    runtime: &[super::render_studio_demo::StudioRaceRuntime],
) -> Vec<glam::Vec3> {
    track_poses(&runner_tracks(race, runtime), sample)
}

/// Upgrade timing data in existing saved comparisons once indexed tracks are
/// ready. This leaves the saved edit and its clock phases intact.
pub(in crate::app) fn ensure_progress(
    race: &mut RaceComparison,
    runtime: &[super::render_studio_demo::StudioRaceRuntime],
) {
    if race.runners.iter().all(|r| !r.progress.is_empty()) {
        return;
    }
    let reference = &race.runners[race.reference];
    let track_for = |runner: &RaceRunner| {
        runtime
            .iter()
            .find(|r| r.key.0 == runner.name && r.key.1 == runner.game)
            .and_then(|r| r.ghost.as_ref())
            .map(|g| &g.track)
    };
    let Some(track) = track_for(reference) else {
        return;
    };
    let reference_duration = reference.race_ms;
    let path = |track: &super::race_ghost::RaceGhostTrack, duration: i32| {
        (0..=duration / 120)
            .map(|i| track_position(track, i * 120).to_array())
            .collect::<Vec<_>>()
    };
    let fast = path(track, reference_duration);
    for (i, runner) in race.runners.iter_mut().enumerate() {
        if !runner.progress.is_empty() {
            continue;
        }
        if i == race.reference {
            runner.progress = vec![[0, 0], [runner.race_ms, reference_duration]];
        } else if let Some(track) = track_for(runner) {
            runner.progress = super::render_studio_director::course_progress(
                &fast,
                &path(track, runner.race_ms),
                runner.race_ms,
                reference_duration,
            );
        }
    }
}

fn heading(
    sample: &RaceSample<'_>,
    tracks: &[Option<&super::race_ghost::RaceGhostTrack>],
    focus: usize,
    replay: Option<&RaceReplay>,
) -> glam::Vec3 {
    let track = tracks[focus];
    let ms = sample.clocks[focus];
    let delta = track.map_or(glam::Vec3::X, |t| {
        if let Some(route) = replay {
            track_position(t, route.exit_ms) - track_position(t, route.entry_ms)
        } else {
            track_position(t, ms + 180) - track_position(t, (ms - 180).max(0))
        }
    });
    let mut direction = glam::Vec3::new(delta.x, delta.y, 0.0).normalize_or_zero();
    if replay.is_none() {
        direction = track.map_or(glam::Vec3::X, |t| travel_heading(t, ms));
    }
    if direction.length_squared() < 0.5 && replay.is_some() {
        // A vertical section or a loop can rejoin at the same XY position.
        // Its detached camera must not fall back to the recorded POV yaw.
        direction = glam::Vec3::X;
    }
    if let Some(route) = replay {
        // A world-space side view of the section, independent of recorded
        // view angles. A small pan follows progress along the full route.
        let u = ((ms - route.entry_ms) as f32 / (route.exit_ms - route.entry_ms).max(1) as f32)
            .clamp(0.0, 1.0);
        let angle = std::f32::consts::FRAC_PI_2 + (u - 0.5) * 0.25;
        let (sn, cs) = angle.sin_cos();
        direction = glam::Vec3::new(
            cs * direction.x - sn * direction.y,
            sn * direction.x + cs * direction.y,
            0.0,
        );
    }
    direction
}

/// Keep the approach heading through a vertical climb. Tiny XY motion and
/// wall-contact view yaw do not define a useful chase direction. Looking back
/// in source time also makes reverse playback, seeks and export agree.
fn travel_heading(track: &super::race_ghost::RaceGhostTrack, ms: i32) -> glam::Vec3 {
    let motion = |time: i32| {
        let d = track_position(track, time + 180) - track_position(track, (time - 180).max(0));
        glam::Vec3::new(d.x, d.y, 0.0)
    };
    let current = motion(ms);
    const RELIABLE: f32 = 24.0;
    if current.length() >= RELIABLE { return current.normalize(); }
    let previous = (0..=ms.max(0) / 50).find_map(|step| {
        let d = motion(ms - step * 50);
        (d.length() >= RELIABLE).then(|| d.normalize())
    }).unwrap_or(glam::Vec3::X);
    if current.length_squared() < 1e-4 { return previous; }
    let a = previous.y.atan2(previous.x);
    let b = current.y.atan2(current.x);
    let weight = (current.length() / RELIABLE).clamp(0.0, 1.0);
    let weight = weight * weight * (3.0 - 2.0 * weight);
    let angle = a + wrap_angle(b - a) * weight;
    glam::Vec3::new(angle.cos(), angle.sin(), 0.0)
}

pub(in crate::app) const CAMERA_VERSION: u32 = 2;

/// Rebuild camera data from older project files once all source tracks arrive.
pub(in crate::app) fn ensure_camera(
    race: &mut RaceComparison,
    runtime: &[super::render_studio_demo::StudioRaceRuntime],
    collision: Option<&mut jka_movement::CollisionWorld>,
    viewport: [u32; 2],
) {
    if race.camera_version == CAMERA_VERSION && !race.camera.is_empty() { return; }
    let Some(world) = collision else { return };
    let tracks = runner_tracks(race, runtime);
    if tracks.iter().any(Option::is_none) { return; }
    race.camera = bake_camera(race, &tracks, world, viewport);
    race.camera_version = CAMERA_VERSION;
}

/// Studio seconds between baked chase-camera keys.
pub(in crate::app) const CAMERA_KEY_STEP: f64 = 0.05;
/// The baked solve judges each candidate by its worst moment this many keys
/// ahead and behind, so a move around an obstruction starts before it arrives.
const CAMERA_CHOICE_RADIUS: usize = 10;
/// Boom and lens easing radius in keys (see `ease_above`).
const CAMERA_EASE_RADIUS: usize = 5;
const CAMERA_MASK: i32 = 0x1 | 0x10 | 0x1000;
/// Yaw offsets from the chase heading, best first.
const CHASE_ANGLES: [f32; 12] = [
    0.0,
    0.4,
    -0.4,
    0.8,
    -0.8,
    1.3,
    -1.3,
    1.9,
    -1.9,
    2.5,
    -2.5,
    std::f32::consts::PI,
];
/// Wider lenses tried after the wanted one.
const CHASE_LENSES: [f32; 5] = [105.0, 120.0, 135.0, 150.0, 160.0];

/// One baked chase-camera key. Key `k` is at output `k * CAMERA_KEY_STEP`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(in crate::app) struct RaceCameraKey {
    /// Map-space position.
    pub position: [f32; 3],
    pub fov: f32,
}

/// Per-phase adjustments to the planned race camera, persisted with its shot.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(in crate::app) struct RaceCameraEdit {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub height: f32,
    pub side: f32,
    pub fov: Option<f32>,
    pub lens: Option<crate::renderer::StudioLens>,
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

    fn position(&self, position: glam::Vec3, target: glam::Vec3) -> glam::Vec3 {
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

/// What the chase wants at one output time, before collision.
struct ChaseFraming {
    body_origin: glam::Vec3,
    /// Look target (eases between runners while aligning).
    center: glam::Vec3,
    body_center: glam::Vec3,
    direction: glam::Vec3,
    distance: f32,
    fov: f32,
    desired_height: f32,
    orbit: f32,
    replay_weight: f32,
}

fn chase_framing(
    race: &RaceComparison,
    sample: &RaceSample<'_>,
    tracks: &[Option<&super::race_ghost::RaceGhostTrack>],
    output: f64,
) -> ChaseFraming {
    use glam::Vec3;
    let points = track_poses(tracks, sample);
    let focus = race.focus(sample.phase);
    let body_origin = points[focus];
    let u = (sample.elapsed / sample.phase.duration).clamp(0.0, 1.0) as f32;
    let ease = u * u * (3.0 - 2.0 * u);
    let returning = sample.phase.kind == RacePhaseKind::Return;
    let replay = sample.phase.replay.as_ref();
    let mut direction = heading(sample, tracks, focus, if returning { None } else { replay });
    let near = points
        .iter()
        .filter(|p| p.distance(body_origin) < 550.0)
        .count();
    let cycle = output.rem_euclid(16.0);
    let mut orbit = if replay.is_none() && race.orbits && near > 1 && (8.0..14.0).contains(&cycle) {
        (((cycle - 8.0) / 6.0) * std::f64::consts::PI).sin().powi(2) as f32 * 0.85
    } else {
        0.0
    };
    let mut origin = body_origin;
    let mut free_weight = if replay.is_some() { 1.0 } else { 0.0 };
    let mut spread = 0.0;
    if let Some(route) = replay {
        if sample.phase.kind == RacePhaseKind::Align || returning {
            let old_focus = if returning {
                route.focus
            } else {
                route.previous_focus
            };
            let previous = race
                .phases
                .iter()
                .take_while(|p| p.start < sample.phase.start)
                .last()
                .and_then(|p| p.replay.as_ref());
            let old_route = if returning { Some(route) } else { previous };
            let old = heading(sample, tracks, old_focus, old_route);
            let a = old.y.atan2(old.x);
            let b = direction.y.atan2(direction.x);
            let angle = a
                + ((b - a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
                    - std::f32::consts::PI)
                    * ease;
            direction = glam::Vec3::new(angle.cos(), angle.sin(), 0.0);
            origin = points[old_focus].lerp(body_origin, ease);
            spread = points[old_focus].distance(body_origin) * 4.0 * ease * (1.0 - ease);
            free_weight = if returning {
                1.0 - ease
            } else {
                if old_route.is_some() {
                    1.0
                } else {
                    ease
                }
            };
            if returning && race.orbits && near > 1 && (8.0..14.0).contains(&cycle) {
                orbit = (((cycle - 8.0) / 6.0) * std::f64::consts::PI).sin().powi(2) as f32
                    * 0.85
                    * ease;
            }
        }
    }
    let (sn, cs) = orbit.sin_cos();
    direction = Vec3::new(
        cs * direction.x - sn * direction.y,
        sn * direction.x + cs * direction.y,
        0.0,
    );
    let replay_weight = match sample.phase.kind {
        RacePhaseKind::Race => 0.0,
        RacePhaseKind::Slowdown => ease,
        RacePhaseKind::Resume => 1.0 - ease,
        _ => 1.0,
    };
    let distance = 220.0
        + 100.0 * replay_weight
        + 70.0 * free_weight
        + 18.0 * (output as f32 * 0.6).sin()
        + spread;
    let base_fov = 88.0 + 4.0 * (output as f32 * 0.45).sin();
    ChaseFraming {
        body_origin,
        center: origin + Vec3::Z * 22.0,
        body_center: body_origin + Vec3::Z * 22.0,
        direction,
        distance,
        fov: base_fov + (96.0 - base_fov) * replay_weight,
        desired_height: 55.0 + orbit * 30.0 + 55.0 * free_weight,
        orbit,
        replay_weight,
    }
}

/// Candidate heights: the wanted one, then level/lower/higher positions for
/// a ceiling or wall that would force a raised camera into the runner.
fn chase_elevation(framing: &ChaseFraming, slot: usize) -> f32 {
    [framing.desired_height, 0.0, -50.0, 120.0][slot]
}

/// The unobstructed boom end for a yaw offset from the chase heading.
fn chase_wanted(framing: &ChaseFraming, angle: f32, elevation: f32) -> glam::Vec3 {
    let (sn, cs) = angle.sin_cos();
    let d = framing.direction;
    let dir = glam::Vec3::new(cs * d.x - sn * d.y, sn * d.x + cs * d.y, 0.0);
    framing.center - dir * framing.distance + glam::Vec3::Z * elevation
}

/// Clear fraction of the boom from the body to `wanted`. The boom is the
/// camera's line of sight, so any point short of it sees the body. None
/// when the body itself starts in solid.
fn chase_reach(
    world: &mut jka_movement::CollisionWorld,
    framing: &ChaseFraming,
    wanted: glam::Vec3,
) -> Option<f32> {
    use jka_movement::TraceWorld;
    let trace = world.trace(jka_movement::TraceQuery {
        start: framing.body_center.to_array(),
        end: wanted.to_array(),
        mins: [-5.0; 3],
        maxs: [5.0; 3],
        pass_entity: -1,
        mask: CAMERA_MASK,
    });
    (trace.start_solid == 0).then_some(trace.fraction)
}

fn chase_view(position: glam::Vec3, target: glam::Vec3, fov: f32) -> crate::camera::Camera {
    let mut key = super::render_studio_timeline::StudioCameraKey {
        position: super::scene::render_position(position.to_array()),
        fov,
        ..Default::default()
    };
    super::render_studio_director::look_at(
        &mut key,
        glam::Vec3::from_array(super::scene::render_position(target.to_array())),
    );
    let mut camera = super::render_studio_spatial::camera_from_key(key);
    camera.set_presentation_fov(fov);
    camera
}

/// Feet and head inside the 8% screen margins.
fn frames_body(camera: &crate::camera::Camera, body_origin: glam::Vec3, viewport: [u32; 2]) -> bool {
    body_inside_margins(camera, body_origin, viewport, 0.08)
}

fn body_inside_margins(camera: &crate::camera::Camera, body_origin: glam::Vec3, viewport: [u32; 2], margin: f32) -> bool {
    [-24.0, 64.0].into_iter().all(|height| {
        let p = glam::Vec3::from_array(super::scene::render_position(
            (body_origin + glam::Vec3::Z * height).to_array(),
        ));
        camera
            .project_to_screen(viewport[0], viewport[1], p)
            .is_some_and(|p| {
                p.x > viewport[0] as f32 * margin
                    && p.x < viewport[0] as f32 * (1.0 - margin)
                    && p.y > viewport[1] as f32 * margin
                    && p.y < viewport[1] as f32 * (1.0 - margin)
            })
    })
}

/// The wanted lens, else the first wider one that frames the body.
fn fitted_lens(framing: &ChaseFraming, position: glam::Vec3, viewport: [u32; 2]) -> Option<f32> {
    let mut camera = chase_view(position, framing.center, framing.fov);
    std::iter::once(framing.fov).chain(CHASE_LENSES).find(|&lens| {
        camera.set_presentation_fov(lens);
        frames_body(&camera, framing.body_origin, viewport)
    })
}

/// Narrowest lens with room for interpolation between the baked keys. Solving
/// exactly at the live 8% cutoff lets subpixel movement alternate between a
/// valid planned frame and a completely different fallback camera.
fn minimum_fov(framing: &ChaseFraming, position: glam::Vec3, viewport: [u32; 2]) -> f32 {
    let mut camera = chase_view(position, framing.center, framing.fov);
    let mut fits = |lens: f32| {
        camera.set_presentation_fov(lens);
        body_inside_margins(&camera, framing.body_origin, viewport, 0.10)
    };
    let (mut low, mut high) = (framing.fov, CHASE_LENSES[CHASE_LENSES.len() - 1]);
    if fits(low) || !fits(high) {
        return if fits(low) { low } else { high };
    }
    for _ in 0..10 {
        let mid = 0.5 * (low + high);
        if fits(mid) {
            high = mid;
        } else {
            low = mid;
        }
    }
    high
}

/// Lower is better: valid frames first, then the wanted lens, angle, height
/// and boom length.
fn chase_score(framing: &ChaseFraming, angle: f32, elevation: f32, lens: f32, position: glam::Vec3) -> f32 {
    (lens - framing.fov).max(0.0) * 2.0
        + angle.abs() * 8.0
        + (elevation - framing.desired_height).abs() * 0.035
        + (framing.distance - position.distance(framing.center)).max(0.0) * 0.02
}

/// Per-frame solve, kept for comparisons saved before cameras were baked.
/// Each frame decides alone, so an obstruction swaps the camera instantly.
fn live_chase(
    framing: &ChaseFraming,
    mut collision: Option<&mut jka_movement::CollisionWorld>,
    viewport: [u32; 2],
) -> (crate::camera::Camera, glam::Vec3) {
    let mut chosen: Option<(f32, glam::Vec3, f32)> = None;
    // Never retain a candidate that clips the body; rank valid frames before
    // resorting to a wide lens.
    'candidates: for slot in 0..4 {
        let elevation = chase_elevation(framing, slot);
        for angle in CHASE_ANGLES {
            let wanted = chase_wanted(framing, angle, elevation);
            let position = match collision.as_deref_mut() {
                Some(world) => match chase_reach(world, framing, wanted) {
                    Some(reach) => framing.body_center.lerp(wanted, reach),
                    None => continue,
                },
                None => wanted,
            };
            let Some(lens) = fitted_lens(framing, position, viewport) else {
                continue;
            };
            let score = chase_score(framing, angle, elevation, lens, position);
            if chosen.as_ref().is_none_or(|(best, _, _)| score < *best) {
                chosen = Some((score, position, lens));
            }
            if score < 0.01 {
                break 'candidates;
            }
        }
    }
    match chosen {
        Some((_, position, lens)) => (chase_view(position, framing.center, lens), position),
        None => {
            // Degenerate geometry still gets a fitted, readable silhouette
            // rather than a known out-of-frame shot.
            let position = framing.body_center - framing.direction * 220.0;
            (chase_view(position, framing.body_center, 110.0), position)
        }
    }
}

/// Mean over the window k-radius..=k+radius, clamped to the ends.
fn box_blur(values: &[f32], radius: usize) -> Vec<f32> {
    let mut prefix = Vec::with_capacity(values.len() + 1);
    prefix.push(0.0f64);
    for &v in values {
        prefix.push(prefix[prefix.len() - 1] + f64::from(v));
    }
    (0..values.len())
        .map(|k| {
            let (a, b) = (k.saturating_sub(radius), (k + radius + 1).min(values.len()));
            ((prefix[b] - prefix[a]) / (b - a) as f64) as f32
        })
        .collect()
}

fn window_max(values: &[f32], radius: usize) -> Vec<f32> {
    (0..values.len())
        .map(|k| {
            values[k.saturating_sub(radius)..(k + radius + 1).min(values.len())]
                .iter()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max)
        })
        .collect()
}

/// A smooth curve never below `values`: a 2r max filter, then two r means.
/// Each mean only averages maxima whose windows include the key itself, so
/// the curve rises ahead of a peak and holds through it.
fn ease_above(values: &[f32], radius: usize) -> Vec<f32> {
    box_blur(&box_blur(&window_max(values, 2 * radius), radius), radius)
}

fn ease_below(values: &[f32], radius: usize) -> Vec<f32> {
    let negated: Vec<f32> = values.iter().map(|v| -v).collect();
    ease_above(&negated, radius).into_iter().map(|v| -v).collect()
}

fn wrap_angle(a: f32) -> f32 {
    (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

/// Solve the whole chase once against map collision, as keys. Unlike the
/// per-frame solve, each choice sees half a second ahead and behind: the
/// camera holds one angle while it stays good, swings around an obstruction
/// before it arrives, and eases the boom in and the lens wider ahead of
/// time. Every key keeps its line of sight to the body.
pub(in crate::app) fn bake_camera(
    race: &RaceComparison,
    tracks: &[Option<&super::race_ghost::RaceGhostTrack>],
    world: &mut jka_movement::CollisionWorld,
    viewport: [u32; 2],
) -> Vec<RaceCameraKey> {
    const BLOCKED: f32 = 1000.0;
    let duration = race.duration();
    if race.phases.is_empty() || tracks.len() != race.runners.len() || !(duration > 0.0) {
        return Vec::new();
    }
    let count = (duration / CAMERA_KEY_STEP).ceil() as usize + 1;
    let frames: Vec<_> = (0..count)
        .map(|k| {
            let output = (k as f64 * CAMERA_KEY_STEP).min(duration);
            chase_framing(race, &race.sample(output), tracks, output)
        })
        .collect();
    let angles = CHASE_ANGLES.len();
    let states = 4 * angles;
    let candidate = |framing: &ChaseFraming, state: usize| {
        (CHASE_ANGLES[state % angles], chase_elevation(framing, state / angles))
    };
    // Every candidate's live-solve cost at every key.
    let mut cost = vec![BLOCKED; count * states];
    for (k, framing) in frames.iter().enumerate() {
        for state in 0..states {
            let (angle, elevation) = candidate(framing, state);
            let wanted = chase_wanted(framing, angle, elevation);
            let Some(reach) = chase_reach(world, framing, wanted) else {
                continue;
            };
            let position = framing.body_center.lerp(wanted, reach);
            if let Some(lens) = fitted_lens(framing, position, viewport) {
                cost[k * states + state] = chase_score(framing, angle, elevation, lens, position);
            }
        }
    }
    // A candidate is only as good as its worst moment nearby.
    let mut ahead = vec![0.0; count * states];
    for state in 0..states {
        let column: Vec<f32> = (0..count).map(|k| cost[k * states + state]).collect();
        for (k, v) in window_max(&column, CAMERA_CHOICE_RADIUS).into_iter().enumerate() {
            ahead[k * states + state] = v;
        }
    }
    // Cheapest path through time. A change must buy back about a second of a
    // one-point-worse view, plus its swing.
    let switch: Vec<f32> = (0..states * states)
        .map(|i| {
            let (from, to) = (i / states, i % states);
            if from == to {
                0.0
            } else {
                20.0 + 20.0 * wrap_angle(CHASE_ANGLES[from % angles] - CHASE_ANGLES[to % angles]).abs()
                    + if from / angles != to / angles { 10.0 } else { 0.0 }
            }
        })
        .collect();
    let mut total = ahead[..states].to_vec();
    let mut back = vec![0u8; count * states];
    for k in 1..count {
        let mut next = vec![0.0; states];
        for to in 0..states {
            let (from, best) = (0..states)
                .map(|from| (from, total[from] + switch[from * states + to]))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap();
            next[to] = best + ahead[k * states + to];
            back[k * states + to] = from as u8;
        }
        total = next;
    }
    let mut choice = vec![0; count];
    choice[count - 1] = (0..states).min_by(|&a, &b| total[a].total_cmp(&total[b])).unwrap();
    for k in (1..count).rev() {
        choice[k - 1] = usize::from(back[k * states + choice[k]]);
    }
    // Ease the chosen world yaw and height: a change becomes a swing.
    let heading_of = |f: &ChaseFraming| f.direction.y.atan2(f.direction.x);
    let mut yaw: Vec<f32> = Vec::with_capacity(count);
    for (framing, &state) in frames.iter().zip(&choice) {
        let raw = heading_of(framing) + candidate(framing, state).0;
        yaw.push(yaw.last().map_or(raw, |&prev| prev + wrap_angle(raw - prev)));
    }
    let yaw = box_blur(&box_blur(&yaw, CAMERA_CHOICE_RADIUS), CAMERA_CHOICE_RADIUS);
    let height: Vec<f32> = frames.iter().zip(&choice).map(|(f, &s)| candidate(f, s).1).collect();
    let height = box_blur(&box_blur(&height, CAMERA_CHOICE_RADIUS), CAMERA_CHOICE_RADIUS);
    // Pull the boom in ahead of anything that would cut the line of sight,
    // never past the clear length at any key.
    let wanted: Vec<_> = frames
        .iter()
        .enumerate()
        .map(|(k, f)| chase_wanted(f, yaw[k] - heading_of(f), height[k]))
        .collect();
    let reach: Vec<f32> = frames
        .iter()
        .zip(&wanted)
        .map(|(f, &w)| chase_reach(world, f, w).unwrap_or(0.0))
        .collect();
    let reach = ease_below(&reach, CAMERA_EASE_RADIUS);
    let positions: Vec<_> = frames
        .iter()
        .zip(&wanted)
        .zip(&reach)
        .map(|((f, &w), &r)| f.body_center.lerp(w, r.clamp(0.0, 1.0)))
        .collect();
    // Widen the lens ahead of any moment the body would leave the margins.
    let widen: Vec<f32> = frames
        .iter()
        .zip(&positions)
        .map(|(f, &p)| minimum_fov(f, p, viewport) - f.fov)
        .collect();
    let widen = ease_above(&widen, CAMERA_EASE_RADIUS);
    frames
        .iter()
        .zip(positions)
        .zip(widen)
        .map(|((f, p), w)| RaceCameraKey { position: p.to_array(), fov: f.fov + w })
        .collect()
}

/// A deterministic chase with smooth, bounded orbit excursions. The camera
/// follows the indexed reference pose, including pause/reverse, never a stale
/// authoritative demo player. Collision and full-body screen margins win over
/// zoom, aperture and orbit styling. Baked keys (see `bake_camera`) replace
/// the per-frame solve when present.
pub(in crate::app) fn camera(
    race: &RaceComparison,
    sample: &RaceSample<'_>,
    runtime: &[super::render_studio_demo::StudioRaceRuntime],
    mut collision: Option<&mut jka_movement::CollisionWorld>,
    output: f64,
    viewport: [u32; 2],
    edit: Option<&RaceCameraEdit>,
) -> (crate::camera::Camera, crate::renderer::StudioLens) {
    let tracks = runner_tracks(race, runtime);
    let framing = chase_framing(race, sample, &tracks, output);
    let (mut camera, mut position) = match race.camera_key_at(output) {
        Some((position, fov)) => (chase_view(position, framing.center, fov), position),
        None => live_chase(&framing, collision.as_deref_mut(), viewport),
    };
    // Interpolation and anticipatory boom easing must never outrank the
    // subject's visibility. Recheck the actual frame, including between keys.
    let clear=collision.as_deref_mut().is_none_or(|world| {
        use jka_movement::TraceWorld;
        let eye=super::scene::jka_position(camera.position.to_array());
        let sight=world.trace(jka_movement::TraceQuery {
            start:eye,end:framing.body_center.to_array(),mins:[0.0;3],maxs:[0.0;3],pass_entity:-1,mask:CAMERA_MASK,
        });
        world.point_contents(eye,-1)&CAMERA_MASK==0 && sight.start_solid==0 && sight.fraction>0.995
    });
    if !clear {
        (camera,position)=live_chase(&framing,collision.as_deref_mut(),viewport);
    } else if !frames_body(&camera,framing.body_origin,viewport) {
        // Keep the planned position and angle for a framing miss. In Echidna's
        // tight passages, the head can cross the margin by a fraction of a
        // pixel between keys; selecting another chase angle causes a flash.
        camera.set_presentation_fov(camera.cg_fov().max(minimum_fov(&framing,position,viewport)));
        if !frames_body(&camera,framing.body_origin,viewport) {
            (camera,position)=live_chase(&framing,collision.as_deref_mut(),viewport);
        }
    }
    if let Some(edit) = edit.filter(|e| e.valid()) {
        let wanted = edit.position(position, framing.body_center);
        let reach = collision.as_deref_mut().map_or(1.0, |world| chase_reach(world, &framing, wanted).unwrap_or(0.0));
        position = framing.body_center.lerp(wanted, reach);
        camera = chase_view(position, framing.center, edit.fov.unwrap_or(camera.cg_fov()));
    }
    smooth_rewind_view(&mut camera, race, sample, &tracks, output, edit);
    let mut lens = crate::renderer::StudioLens::default();
    lens.focus_distance = position.distance(framing.body_center).clamp(1.0, 65536.0);
    // Use depth sparingly: explanatory replays stay sharp across every route.
    if framing.replay_weight < 0.95 && framing.orbit > 0.0 {
        lens.aperture =
            Some(5.6 + 50.0 * (1.0 - framing.orbit / 0.85) + framing.replay_weight * 48.0);
    }
    if let Some(authored) = edit.and_then(|e| e.lens) {
        lens = authored;
        lens.focus_distance = position.distance(framing.body_center).clamp(1.0, 65536.0);
    }
    (camera, lens)
}

/// Average view angles over a short OUTPUT-time window during fast rewinds.
/// Sampling the stored camera and indexed runner makes this independent of
/// frame rate, playback order and whether the frame is previewed or exported.
/// Positions and race clocks keep their original timing.
fn smooth_rewind_view(
    camera: &mut crate::camera::Camera,
    race: &RaceComparison,
    sample: &RaceSample<'_>,
    tracks: &[Option<&super::race_ghost::RaceGhostTrack>],
    output: f64,
    edit: Option<&RaceCameraEdit>,
) {
    if sample.phase.kind != RacePhaseKind::Rewind { return; }
    let focus = race.focus(sample.phase);
    let Some(track) = tracks[focus] else { return };
    const WINDOW: f64 = 0.18;
    let start = sample.phase.start;
    let end = start + sample.phase.duration;
    let blend = ((output - start).min(end - output) / WINDOW).clamp(0.0, 1.0) as f32;
    let blend = blend * blend * (3.0 - 2.0 * blend);
    if blend == 0.0 { return; }
    let mut previous = None::<f32>;
    let (mut yaw_sum, mut pitch_sum, mut weight_sum) = (0.0, 0.0, 0.0);
    for tap in -6i32..=6 {
        let time = (output + f64::from(tap) * WINDOW / 6.0).clamp(start, end - 1e-6);
        let Some((position, fov)) = race.camera_key_at(time) else { return };
        let target = track_position(track, race.sample(time).clocks[focus]) + glam::Vec3::Z * 22.0;
        let position = edit.filter(|e| e.valid()).map_or(position, |e| e.position(position, target));
        let view = chase_view(position, target, fov);
        let yaw = previous.map_or(view.yaw, |last| last + wrap_angle(view.yaw - last));
        previous = Some(yaw);
        let weight = (7 - tap.abs()) as f32;
        yaw_sum += yaw * weight;
        pitch_sum += view.pitch * weight;
        weight_sum += weight;
    }
    camera.yaw += wrap_angle(yaw_sum / weight_sum - camera.yaw) * blend;
    camera.pitch += (pitch_sum / weight_sum - camera.pitch) * blend;
}

/// Flat colored RF_NODEPTH shells reuse the existing GPU/CPU skinning path.
/// These are silent, non-shadow-casting Studio presentation geometry only.
pub(in crate::app) fn silhouettes(
    draws: &[crate::renderer::DynamicModelSurface],
    color: [u8; 3],
) -> Vec<crate::renderer::DynamicModelSurface> {
    use std::{collections::HashSet, sync::Arc};
    let mut seen = HashSet::new();
    let tint = [
        f32::from(color[0]) / 255.0,
        f32::from(color[1]) / 255.0,
        f32::from(color[2]) / 255.0,
        0.14,
    ];
    draws
        .iter()
        .filter_map(|body| {
            let id = body.ghoul2_gpu.as_ref().map_or_else(
                || format!("{:p}", Arc::as_ptr(&body.indices)),
                |g| g.mesh_key.to_string(),
            );
            if !seen.insert(id) {
                return None;
            }
            let mut shell = body.clone();
            shell.alpha_mode = crate::renderer::DynamicModelAlphaMode::SightShellNoDepth;
            shell.texture = None;
            shell.lighting_origin = None;
            shell.rt_rigid = None;
            shell.rt_skinned_key = None;
            shell.authored_glow = false;
            if let Some(gpu) = shell.ghoul2_gpu.as_mut() {
                gpu.color = tint;
                gpu.source_material = None;
            } else {
                shell.vertices = Arc::new(
                    body.vertices
                        .iter()
                        .map(|v| crate::renderer::DynamicModelVertex { color: tint, ..*v })
                        .collect(),
                );
            }
            Some(shell)
        })
        .collect()
}

pub(in crate::app) fn hud(
    race: &RaceComparison,
    sample: &RaceSample<'_>,
    runtime: &[super::render_studio_demo::StudioRaceRuntime],
    camera: &crate::camera::Camera,
    viewport: [u32; 2],
    map: Option<std::sync::Arc<crate::ui::StudioRaceMap>>,
    output_time: f64,
    ui: &mut crate::ui::UiSnapshot,
) {
    let points = poses(race, sample, runtime);
    ui.video.skip_ui = false;
    ui.video.draw_2d = true;
    ui.video.draw_fps = 0;
    let reference = &race.runners[race.reference];
    let track = runtime
        .iter()
        .find(|r| r.key.0 == reference.name && r.key.1 == reference.game)
        .and_then(|r| r.ghost.as_ref())
        .map(|g| &g.track);
    let route = track.filter(|_| map.is_none()).map_or_else(Vec::new, |t| {
        t.frames
            .iter()
            .step_by((t.frames.len() / 300).max(1))
            .filter_map(|f| super::playerstate_vec3(&f.player_state, "origin"))
            .collect()
    });
    ui.studio_world_labels.clear();
    let focus = race.focus(sample.phase);
    let standings = standings(race, sample);
    let bounds = timing_bounds(race, sample.phase);
    let runners = race
        .runners
        .iter()
        .enumerate()
        .map(|(i, runner)| {
            let color = [
                f32::from(runner.color[0]) / 255.0,
                f32::from(runner.color[1]) / 255.0,
                f32::from(runner.color[2]) / 255.0,
                1.0,
            ];
            let name = runner
                .name
                .split('-')
                .next()
                .unwrap_or(&runner.name)
                .to_owned();
            let head = glam::Vec3::from_array(super::scene::render_position(
                (points[i] + glam::Vec3::Z * 70.0).to_array(),
            ));
            if let Some(p) = camera.project_to_screen(viewport[0], viewport[1], head) {
                let reserved = crate::ui::leaderboard_bounds(
                    race.runners.len(),
                    viewport[0],
                    viewport[1],
                    race.style.readout_scale,
                );
                if !(p.x < reserved[0] + reserved[2] + 45.0 && p.y < reserved[1] + reserved[3] + 12.0) {
                ui.studio_world_labels.push(crate::ui::UiStudioLabel {
                    color,
                    pos: [p.x / viewport[0] as f32, p.y / viewport[1] as f32],
                    text: format!(
                        "{}{}",
                        if i == focus && focus != race.reference {
                            "FOCUS \u{00b7} "
                        } else if i == race.reference {
                            "REFERENCE · "
                        } else {
                            ""
                        },
                        name.chars().take(16).collect::<String>()
                    ),
                    alpha: if i == focus || i == race.reference {
                        1.0
                    } else {
                        race.style.label_alpha.clamp(0.0, 1.0)
                    },
                    scale: race.style.label_scale.clamp(0.3, 2.0)
                        * if i == focus || i == race.reference { 1.15 } else { 1.0 },
                });
            }
                }
            crate::ui::StudioRaceRunnerUi {
                route_index: i,
                rank: standings[i].0,
                gap: standings[i].1,
                focused: i == focus,
                name,
                color: runner.color,
                position: points[i].to_array(),
                elapsed: f64::from(
                    (sample.clocks[i].min(bounds.map_or(runner.race_ms, |b| b.1[i]))
                        - bounds.map_or(0, |b| b.0[i]))
                    .max(0),
                ) / 1000.0,
                finished: sample.clocks[i] >= bounds.map_or(runner.race_ms, |b| b.1[i]),
                reference: i == race.reference,
            }
        })
        .collect();
    // Keep the reference label anchored; spread nearby labels into separate
    // rows instead of printing overlapping names on a stacked starting pack.
    ui.studio_world_labels
        .sort_by_key(|label| !label.text.starts_with("REFERENCE"));
    for i in 0..ui.studio_world_labels.len() {
        let (placed, rest) = ui.studio_world_labels.split_at_mut(i);
        let label = &mut rest[0];
        let original = label.pos[1];
        label.pos[0] = label.pos[0].clamp(0.08, 0.92);
        for step in 0..16 {
            let offset = if step % 2 == 1 {
                -((step + 1) / 2) as f32
            } else {
                (step / 2) as f32
            };
            let y = (original + offset * 0.035).clamp(0.03, 0.94);
            if placed.iter().all(|p| {
                (p.pos[1] - y).abs() > 0.03
                    || (p.pos[0] - label.pos[0]).abs()
                        > (p.text.len() + label.text.len()) as f32 * 0.0065
            }) {
                label.pos[1] = y;
                break;
            }
        }
    }
    let mut runners: Vec<_> = runners;
    runners.sort_by_key(|r| r.rank);
    ui.studio_race = Some(crate::ui::StudioRaceUi {
        section_clock: bounds.is_some(),
        phase: race.phase_label(sample.phase),
        section: sample.phase.section,
        runners,
        route,
        map,
        output_time,
        font: race.style.font.clone(),
        readout_scale: race.style.readout_scale.clamp(0.4, 2.5),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn race_camera_edits_round_trip_and_change_framing() {
        use super::super::render_studio_director::{StudioDirectorShot, StudioDirectorPreset};
        let mut shot = StudioDirectorShot::new(1, StudioDirectorPreset::Chase, -1, 0.0, 2.0);
        shot.comparison_label = Some("Race".into());
        let edit = RaceCameraEdit { yaw: 90.0, distance: 2.0, height: 30.0,
            fov: Some(75.0), ..Default::default() };
        shot.comparison_camera = Some(edit);
        let text = serde_json::to_string(&shot).unwrap();
        let restored: StudioDirectorShot = serde_json::from_str(&text).unwrap();
        assert_eq!(restored, shot);
        assert!(restored.valid());
        let target = glam::Vec3::new(10.0, 20.0, 30.0);
        let position = edit.position(target + glam::Vec3::X * 100.0, target);
        assert!(position.distance(target + glam::Vec3::Y * 200.0 + glam::Vec3::Z * 30.0) < 0.001);
        shot.comparison_camera.as_mut().unwrap().distance = f32::NAN;
        assert!(!shot.valid());
        // Old phase markers remain loadable and acquire controls without migration.
        let mut legacy = serde_json::to_value(&restored).unwrap();
        legacy.as_object_mut().unwrap().remove("comparison_camera");
        assert!(serde_json::from_value::<StudioDirectorShot>(legacy).unwrap().comparison_camera.is_none());
    }

    #[test]
    #[ignore = "Requires local Echidna project, demos and Ventara map; set DJK_STUDIO_TEST_BASE"]
    fn echidna_wallrun_camera_stays_continuous_and_editable() {
        use super::super::{race_ghost, render_studio_demo::StudioRaceRuntime, render_studio_timeline::StudioDemoClip};
        let base = std::path::PathBuf::from(std::env::var_os("DJK_STUDIO_TEST_BASE").unwrap());
        jka_assets::pk3::configure_filesystem_roots(&base, crate::filesystem_paths::load(&base));
        let project = std::env::var_os("DJK_STUDIO_TEST_PROJECT").map(std::path::PathBuf::from)
            .unwrap_or_else(|| base.join("render-studio/Echidna.json"));
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(project).unwrap()).unwrap();
        let clips: Vec<StudioDemoClip> = serde_json::from_value(json["demos"].clone()).unwrap();
        let mut race = *clips[0].comparison.clone().unwrap();
        let runtime: Vec<_> = race.runners.iter().enumerate().map(|(i, runner)| {
            let bytes = std::fs::read(base.parent().unwrap().join(&runner.game).join("demos")
                .join(format!("{}.dm_26", runner.name))).unwrap();
            let ghost = race_ghost::RaceGhost::from_track(race_ghost::parse_track(runner.name.clone(), bytes).unwrap(), &[], 60000+i as u16).unwrap();
            StudioRaceRuntime { key: (runner.name.clone(),runner.game.clone()), ghost: Some(ghost), worker: None, pending_track: None, error: None }
        }).collect();
        let mut assets = jka_assets::pk3::AssetSearchPath::open_game(&base, Some(&base.parent().unwrap().join("japro")))
            .expect("map search path");
        let bsp = assets.read("maps/ventara.bsp",512*1024*1024).expect("read Ventara").expect("installed Ventara");
        let mut world = jka_movement::CollisionWorld::from_bsp(&bsp.bytes).expect("Ventara collision");
        // Upgrade the saved path; a second evaluation must reuse it.
        race.camera_version = 0;
        ensure_camera(&mut race, &runtime, Some(&mut world), [1280,720]);
        assert_eq!(race.camera_version, CAMERA_VERSION);
        let cache = race.camera.as_ptr();
        ensure_camera(&mut race, &runtime, Some(&mut world), [1280,720]);
        assert_eq!(cache, race.camera.as_ptr());
        let mut previous = None::<(glam::Vec3,f32)>;
        let mut max_step = 0.0f32;
        let mut max_angle = 0.0f32;
        let mut max_fov = 0.0f32;
        for frame in 0..=240 {
            let time = 12.5 + frame as f64 / 120.0;
            let sample = race.sample(time);
            let tracks = runner_tracks(&race,&runtime);
            let body = track_poses(&tracks,&sample)[race.focus(sample.phase)];
            let (cam,_) = camera(&race,&sample,&runtime,Some(&mut world),time,[1280,720],None);
            let offset = glam::Vec3::from_array(super::super::scene::jka_position(cam.position.to_array())) - body;
            if let Some((last,yaw)) = previous {
                max_step = max_step.max(offset.distance(last));
                max_angle = max_angle.max(wrap_angle(cam.yaw-yaw).abs().to_degrees());
            }
            max_fov = max_fov.max(cam.cg_fov());
            previous = Some((offset,cam.yaw));
            assert!(offset.length() > 60.0,"camera collapsed into runner at {time}: {offset}");
            assert!(frames_body(&cam,body,[1280,720]),"body clipped at {time}");
            if frame % 30 == 0 { eprintln!("{time:.2}: offset={offset:.1} fov={:.1}",cam.cg_fov()); }
            let edit = RaceCameraEdit { fov: Some(75.0), lens: Some(crate::renderer::StudioLens { aperture: Some(2.8), ..Default::default() }), ..Default::default() };
            let (edited,lens) = camera(&race,&sample,&runtime,Some(&mut world),time,[1280,720],Some(&edit));
            assert!((edited.cg_fov()-75.0).abs()<0.001);
            assert_eq!(lens.aperture,Some(2.8));
            // Seeking away and back uses exactly the same sampled camera.
            let _ = camera(&race,&race.sample(2.0),&runtime,Some(&mut world),2.0,[1280,720],None);
            let (again,_) = camera(&race,&sample,&runtime,Some(&mut world),time,[1280,720],None);
            assert_eq!(cam.position,again.position);
            assert_eq!(cam.yaw,again.yaw);
        }
        eprintln!("Wallrun max per 120 Hz frame: {max_step:.2} units, {max_angle:.2} degrees; max FOV {max_fov:.1}");
        assert!(max_step < 12.0,"camera position snapped: {max_step}");
        assert!(max_angle < 5.0,"camera yaw snapped: {max_angle}");
        assert!(max_fov < 120.0,"wallrun forced a distorted lens: {max_fov}");
    }

    #[test]
    fn live_gaps_compare_crossing_times_and_finish_times() {
        let runners = [1000, 2000]
            .into_iter()
            .enumerate()
            .map(|(i, duration)| RaceRunner {
                name: format!("runner{i}"),
                game: String::new(),
                duration_ms: duration + 5000,
                race_ms: duration,
                color: [255; 3],
                progress: vec![[0, 0], [duration, 1000]],
            })
            .collect();
        let race = build_sequence(runners, 0, 0, 10000, vec![], false, false);
        let a = standings(&race, &race.sample(0.5));
        assert_eq!(a[0], (1, Some(0.0)));
        assert!((a[1].1.unwrap() - 0.25).abs() < 1e-9);
        let b = standings(&race, &race.sample(2.0));
        assert_eq!(b[1], (2, Some(1.0)));
        let text = serde_json::to_string(&race).unwrap();
        let restored: RaceComparison = serde_json::from_str(&text).unwrap();
        assert_eq!(standings(&restored, &restored.sample(0.5)), a);
    }

    #[test]
    fn section_gaps_remove_each_runners_fork_entry_time() {
        let runners = [1000, 2000]
            .into_iter()
            .enumerate()
            .map(|(i, duration)| RaceRunner {
                name: format!("runner{i}"),
                game: String::new(),
                duration_ms: duration + 5000,
                race_ms: duration,
                color: [255; 3],
                progress: vec![[0, 0], [duration, 1000]],
            })
            .collect();
        let mut race = build_sequence(runners, 0, 0, 10000, vec![], false, false);
        race.phases = vec![RacePhase {
            kind: RacePhaseKind::Replay,
            start: 0.0,
            duration: 1.25,
            from: vec![500, 1000],
            to: vec![1000, 2000],
            world_from: 1000,
            world_to: 1000,
            section: 1,
            replay: Some(RaceReplay {
                focus: 1,
                previous_focus: 0,
                members: vec![1],
                ordinal: 2,
                total: 2,
                entry_ms: 1000,
                exit_ms: 2000,
                exits: vec![1000, 2000],
            }),
        }];
        let a = standings(&race, &race.sample(0.75));
        assert_eq!(a[0].0, 1);
        assert!((a[1].1.unwrap() - 0.25).abs() < 1e-9);
        assert_eq!(standings(&race, &race.sample(1.25))[1], (2, Some(0.5)));
        let mut pause = race.phases[0].clone();
        pause.kind = RacePhaseKind::Pause;
        pause.start = 1.25;
        pause.duration = 0.5;
        pause.from = pause.to.clone();
        race.phases.push(pause);
        assert_eq!(
            standings(&race, &race.sample(1.5))[1],
            (2, Some(0.5)),
            "pausing a section must preserve its timing basis"
        );
        let bounds = timing_bounds(&race, race.sample(1.5).phase).unwrap();
        assert_eq!(bounds.0, &[500, 1000]);
    }
    #[test]
    fn comparison_palette_is_unique_for_all_thirty_two_runners() {
        for count in 2..=32 {
            let colors: std::collections::HashSet<_> =
                (0..count).map(|i| runner_color(i, count, 1)).collect();
            assert_eq!(colors.len(), count);
        }
    }

    #[test]
    fn camera_easing_moves_ahead_of_an_obstruction_and_never_past_it() {
        // A boom that is fully clear except for a short obstruction.
        let mut reach = vec![1.0f32; 80];
        reach[40..44].fill(0.3);
        let eased = ease_below(&reach, CAMERA_EASE_RADIUS);
        assert!(eased.iter().zip(&reach).all(|(e, r)| *e <= *r + 1e-6), "never past the clear boom");
        assert!(eased[40] <= 0.3 + 1e-6, "fully pulled in when it arrives");
        assert!(eased[40 - CAMERA_EASE_RADIUS] < 0.5, "well underway before it arrives");
        assert!(eased[40 - 4 * CAMERA_EASE_RADIUS + 1] < 1.0, "starts moving early");
        assert!(eased.windows(2).all(|w| (w[0] - w[1]).abs() < 0.15), "no snap");
        let widen = ease_above(&reach.iter().map(|r| 1.0 - r).collect::<Vec<_>>(), 2);
        assert!(widen.iter().zip(&reach).all(|(w, r)| *w >= 1.0 - r - 1e-6));
    }

    #[test]
    fn baked_camera_keys_interpolate_smoothly_and_cut_on_teleports() {
        let runners = [1000, 2000]
            .into_iter()
            .enumerate()
            .map(|(i, duration)| RaceRunner {
                name: format!("runner{i}"),
                game: String::new(),
                duration_ms: duration + 5000,
                race_ms: duration,
                color: [255; 3],
                progress: vec![[0, 0], [duration, 1000]],
            })
            .collect();
        let mut race = build_sequence(runners, 0, 0, 10000, vec![], false, false);
        assert!(race.camera_key_at(0.3).is_none(), "unbaked comparisons keep the live solve");
        race.camera = (0..=20)
            .map(|k| RaceCameraKey {
                position: [k as f32 * 10.0 + if k >= 15 { 5000.0 } else { 0.0 }, 0.0, 0.0],
                fov: 90.0,
            })
            .collect();
        assert!(race.valid(10000));
        let (p, fov) = race.camera_key_at(0.125).unwrap();
        assert!((p.x - 25.0).abs() < 1e-3 && fov == 90.0, "{p}");
        let (before, _) = race.camera_key_at(14.4 * CAMERA_KEY_STEP).unwrap();
        let (after, _) = race.camera_key_at(14.6 * CAMERA_KEY_STEP).unwrap();
        assert!(before.x < 200.0 && after.x > 5000.0, "teleport is a cut: {before} {after}");
        assert!(race.camera_key_at(99.0).is_some_and(|(p, _)| (p.x - 5200.0).abs() < 1e-2));
    }
}
