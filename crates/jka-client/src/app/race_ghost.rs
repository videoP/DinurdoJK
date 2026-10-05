//! Visual-only jaPRO race ghosts loaded from native `.dm_26` demos.
//!
//! A ghost deliberately does not own a second CGame. The worker reduces the
//! recorded POV to the last `duelTime` race segment, retains the configstrings
//! needed to rebuild the recorded ClientInfo, and runtime presentation feeds
//! those playerState samples through the ordinary Ghoul2 PlayerPresenter.

use crate::cgame::{
    presented_player_state_interpolated, ClientGameState, ClientInfo, PresentedEntity,
};
use crate::strafe_trail::LoadedTrail;
use jka_assets::siege::SiegeClassVisual;
use jka_protocol::{
    commands::{atoi, info_value, tokenize, BigConfigOutcome, BigConfigString},
    demo::DemoReader,
    server::{Decoder as ServerMessageDecoder, Event as ServerMessageEvent, PlayerState},
};
use std::{collections::BTreeMap, io::Cursor};

const EF_TELEPORT_BIT: i32 = 1 << 3;

#[derive(Debug, Clone)]
pub(super) struct RaceGhostFrame {
    pub elapsed_ms: i32,
    pub player_state: PlayerState,
}

#[derive(Debug)]
pub(super) struct RaceGhostTrack {
    pub demo_name: String,
    /// Stable identity used by the GUI to toggle a local or remote reference.
    pub source_key: String,
    /// Human-facing label (archive `username` for web ghosts, demo name locally).
    pub display_name: String,
    pub map_name: String,
    pub client_num: usize,
    pub frames: Vec<RaceGhostFrame>,
    pub configstrings: BTreeMap<u16, Vec<u8>>,
    /// Pre-indexed copy of the recorded route. Built on the demo worker so
    /// enabling the visualization never performs LOD construction mid-frame.
    pub strafe_trail: LoadedTrail,
}

impl RaceGhostTrack {
    pub fn duration_ms(&self) -> i32 {
        self.frames.last().map_or(0, |frame| frame.elapsed_ms.max(0))
    }
}

#[derive(Debug)]
struct SegmentBuilder {
    duel_time: i32,
    client_num: usize,
    map_name: String,
    frames: Vec<RaceGhostFrame>,
    configstrings: BTreeMap<u16, Vec<u8>>,
}

impl SegmentBuilder {
    fn new(
        duel_time: i32,
        client_num: usize,
        map_name: String,
        configstrings: &BTreeMap<u16, Vec<u8>>,
    ) -> Self {
        Self {
            duel_time,
            client_num,
            map_name,
            frames: Vec::new(),
            configstrings: configstrings.clone(),
        }
    }

    fn push(&mut self, server_time: i32, player_state: PlayerState) {
        self.frames.push(RaceGhostFrame {
            elapsed_ms: server_time.saturating_sub(self.duel_time),
            player_state,
        });
    }
}

fn apply_configstring_command(text: &[u8], configstrings: &mut BTreeMap<u16, Vec<u8>>) {
    let args = tokenize(text);
    if !args.first().is_some_and(|command| command.eq_ignore_ascii_case(b"cs")) {
        return;
    }
    let (Some(index), Some(value)) = (args.get(1), args.get(2)) else {
        return;
    };
    let Ok(index) = u16::try_from(atoi(index)) else {
        return;
    };
    configstrings.insert(index, value.clone());
}

/// Parse exactly the same race selection policy as the supplied JS tool: every
/// snapshot with a positive `duelTime` belongs to a race segment and the final
/// non-empty segment in the demo is selected.
pub(super) fn parse_track(demo_name: String, bytes: Vec<u8>) -> Result<RaceGhostTrack, String> {
    let mut reader = DemoReader::new(Cursor::new(bytes));
    let mut decoder = ServerMessageDecoder::new();
    let mut configstrings = BTreeMap::<u16, Vec<u8>>::new();
    let mut big_config = BigConfigString::default();
    let mut current = None::<SegmentBuilder>;
    let mut last_completed = None::<SegmentBuilder>;
    let mut messages = 0usize;

    let finish = |current: &mut Option<SegmentBuilder>, last_completed: &mut Option<SegmentBuilder>| {
        if let Some(segment) = current.take() {
            if !segment.frames.is_empty() {
                *last_completed = Some(segment);
            }
        }
    };

    while let Some(record) = reader
        .next_record()
        .map_err(|error| format!("RACE GHOST FRAMING ERROR after message {messages}: {error}"))?
    {
        let sequence = record.sequence;
        let packet = decoder.parse_packet(sequence, &record.payload).map_err(|error| {
            format!(
                "RACE GHOST PROTOCOL ERROR at message {messages} sequence {sequence} byte~{} bit {}: {error}",
                error.bit / 8,
                error.bit,
            )
        })?;
        messages += 1;

        for event in packet.events {
            match event {
                ServerMessageEvent::Gamestate { .. } => {
                    configstrings.clone_from(&decoder.configstrings);
                    big_config = BigConfigString::default();
                }
                ServerMessageEvent::ServerCommand(command) => {
                    let config_command = match big_config.feed(&command.text) {
                        BigConfigOutcome::PassThrough => Some(command.text),
                        BigConfigOutcome::Pending => None,
                        BigConfigOutcome::Complete(text) => Some(text),
                        BigConfigOutcome::Overflow => {
                            return Err(format!(
                                "RACE GHOST: oversized bcs configstring at server command {}",
                                command.sequence
                            ));
                        }
                    };
                    if let Some(config_command) = config_command {
                        apply_configstring_command(&config_command, &mut configstrings);
                    }
                }
                ServerMessageEvent::Snapshot { .. } => {
                    let Some(snapshot) = decoder.latest_snapshot().cloned() else { continue };
                    let duel_time = snapshot.player_state.field_i32("duelTime").unwrap_or(0);
                    if duel_time <= 0 || snapshot.server_time < duel_time {
                        finish(&mut current, &mut last_completed);
                        continue;
                    }
                    let raw_client = snapshot
                        .player_state
                        .field_i32("clientNum")
                        .unwrap_or(decoder.client_number);
                    let Ok(client_num) = usize::try_from(raw_client) else {
                        finish(&mut current, &mut last_completed);
                        continue;
                    };
                    let map_name = configstrings
                        .get(&0)
                        .and_then(|serverinfo| info_value(serverinfo, b"mapname"))
                        .and_then(|value| String::from_utf8(value.to_vec()).ok())
                        .ok_or_else(|| "RACE GHOST demo snapshot has no mapname".to_owned())?;
                    let elapsed = snapshot.server_time.saturating_sub(duel_time);
                    let restarted = current.as_ref().is_some_and(|segment| {
                        segment.duel_time != duel_time
                            || segment.client_num != client_num
                            || !segment.map_name.eq_ignore_ascii_case(&map_name)
                            || segment
                                .frames
                                .last()
                                .is_some_and(|last| elapsed < last.elapsed_ms)
                    });
                    if restarted {
                        finish(&mut current, &mut last_completed);
                    }
                    let segment = current.get_or_insert_with(|| {
                        SegmentBuilder::new(duel_time, client_num, map_name, &configstrings)
                    });
                    segment.push(snapshot.server_time, snapshot.player_state);
                }
                _ => {}
            }
        }
    }
    finish(&mut current, &mut last_completed);

    let segment = last_completed
        .take()
        .ok_or_else(|| format!("RACE GHOST: demos/{demo_name}.dm_26 contains no duelTime race segment"))?;
    if segment.frames.len() < 2 {
        return Err(format!(
            "RACE GHOST: demos/{demo_name}.dm_26 race segment has only {} snapshot(s)",
            segment.frames.len()
        ));
    }
    let trail_points = segment
        .frames
        .iter()
        .filter_map(|frame| playerstate_vec3(&frame.player_state, "origin"))
        .collect::<Vec<_>>();
    let strafe_trail = LoadedTrail::from_points(
        format!("race-ghost:{demo_name}"),
        0,
        &trail_points,
    );
    Ok(RaceGhostTrack {
        source_key: demo_name.clone(),
        display_name: demo_name.clone(),
        demo_name,
        map_name: segment.map_name,
        client_num: segment.client_num,
        frames: segment.frames,
        configstrings: segment.configstrings,
        strafe_trail,
    })
}

#[derive(Debug, Clone, Copy)]
pub(super) struct RaceGhostVisualSample {
    pub origin: [f32; 3],
    pub velocity: [f32; 3],
}

pub(super) struct RaceGhost {
    pub track: RaceGhostTrack,
    pub info: ClientInfo,
    pub entity_num: u16,
    last_live_duel_time: Option<i32>,
    last_elapsed_ms: i32,
    pub visual_sample: Option<RaceGhostVisualSample>,
}

impl RaceGhost {
    pub fn from_track(
        track: RaceGhostTrack,
        siege_classes: &[SiegeClassVisual],
        entity_num: u16,
    ) -> Result<Self, String> {
        let mut game = ClientGameState::new();
        game.reset_gamestate(&track.configstrings, 0);
        let mut info = game.client_info(track.client_num, siege_classes).ok_or_else(|| {
            format!(
                "RACE GHOST: recorded client {} has no usable CS_PLAYERS entry",
                track.client_num
            )
        })?;
        // V1 intentionally renders the recorded player body/cosmetics only. Do
        // this once at load time rather than cloning ClientInfo every frame. It
        // also prevents the auxiliary player from touching blade-length runtime
        // keyed by the recorded (real) client number.
        info.saber_name = "none".to_owned();
        info.saber2_name = "none".to_owned();
        let mut track = track;
        track
            .strafe_trail
            .set_slot((entity_num.saturating_sub(60_000) as u8) % 32);
        Ok(Self {
            track,
            info,
            entity_num,
            last_live_duel_time: None,
            last_elapsed_ms: -1,
            visual_sample: None,
        })
    }

    pub fn reset_sync(&mut self) {
        self.last_live_duel_time = None;
        self.last_elapsed_ms = -1;
        self.visual_sample = None;
    }

    /// Return the visual sample synchronized to the live player's own race
    /// clock. The last recorded sample is held after the ghost finishes so a
    /// slower live run can still see where the reference finished.
    pub fn sample(
        &mut self,
        live_duel_time: i32,
        live_race_mode: bool,
        current_time: i32,
    ) -> Option<(PresentedEntity, bool, i32)> {
        let duel_time = live_duel_time;
        if duel_time <= 0 || !live_race_mode {
            self.reset_sync();
            return None;
        }
        // Match jaPRO's race timer exactly: elapsed = cg.time - ps.duelTime.
        // The app passes the same presentation/server clock used for the HUD.
        let mut elapsed = current_time.saturating_sub(duel_time);
        if elapsed < 0 {
            return None;
        }
        elapsed = elapsed.min(self.track.duration_ms());

        let index = self
            .track
            .frames
            .partition_point(|frame| frame.elapsed_ms <= elapsed)
            .saturating_sub(1)
            .min(self.track.frames.len() - 1);
        let current = &self.track.frames[index];
        let next = self.track.frames.get(index + 1);
        let discontinuity = next.is_some_and(|next| {
            let a = current.player_state.field_i32("eFlags").unwrap_or(0);
            let b = next.player_state.field_i32("eFlags").unwrap_or(0);
            ((a ^ b) & EF_TELEPORT_BIT) != 0
        });
        let alpha = next
            .filter(|_| !discontinuity)
            .map(|next| {
                let span = (next.elapsed_ms - current.elapsed_ms).max(1) as f32;
                ((elapsed - current.elapsed_ms) as f32 / span).clamp(0.0, 1.0)
            })
            .unwrap_or(0.0);
        let Some(mut entity) = presented_player_state_interpolated(
            &current.player_state,
            next.filter(|_| !discontinuity).map(|frame| &frame.player_state),
            alpha,
        ) else {
            self.visual_sample = None;
            return None;
        };
        entity.number = self.entity_num;
        entity.state.number = self.entity_num;

        let mut velocity = playerstate_vec3(&current.player_state, "velocity").unwrap_or([0.0; 3]);
        if let Some(next) = next.filter(|_| !discontinuity) {
            if let Some(next_velocity) = playerstate_vec3(&next.player_state, "velocity") {
                for axis in 0..3 {
                    velocity[axis] += (next_velocity[axis] - velocity[axis]) * alpha;
                }
            }
        }
        self.visual_sample = Some(RaceGhostVisualSample {
            origin: entity.origin,
            velocity,
        });

        let force_reset = self.last_live_duel_time != Some(duel_time)
            || elapsed < self.last_elapsed_ms
            || discontinuity;
        self.last_live_duel_time = Some(duel_time);
        self.last_elapsed_ms = elapsed;
        Some((entity, force_reset, elapsed))
    }
}

fn playerstate_vec3(state: &PlayerState, base: &str) -> Option<[f32; 3]> {
    Some([
        state.field_f32(&format!("{base}[0]"))?,
        state.field_f32(&format!("{base}[1]"))?,
        state.field_f32(&format!("{base}[2]"))?,
    ])
}
