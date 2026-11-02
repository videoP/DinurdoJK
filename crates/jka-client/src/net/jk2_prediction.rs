//! Live MVSDK prediction. Only free spectators are admitted until native remote
//! combat context and playable command selection have been validated.
use super::{movement_cmd, prediction_solid_entities, NetworkSettings, PredictionWorld};
use jka_movement::{jk2 as native, CollisionWorld, TraceWorld};
use jka_protocol::{
    commands::{atoi, info_value},
    jk2::{self, presentation, Version},
    netchan::UserCmd,
    server::{PlayerState, Snapshot},
    session::ClientSession,
};
use std::collections::{BTreeMap, VecDeque};

#[cfg(test)]
mod tests;

pub(crate) struct Frame {
    pub display: PlayerState,
    pub committed: PlayerState,
    pub previous: PlayerState,
    pub angles: Option<[i32; 3]>,
}

#[derive(Default)]
pub(crate) struct Predictor {
    player: Option<native::Player>,
    version: Option<Version>,
    animations: Option<Vec<u8>>,
    previous: Option<PlayerState>,
    boundary: Option<(i32, i32, i32, i32)>,
    last_command: i32,
    last_time: i32,
    regime: Option<(i32, i32, i32, i32, i32)>,
    records: VecDeque<(i32, native::State)>,
    compared_snapshot: Option<i32>,
    matched: u32,
    compared: u32,
    diverged: u32,
    max_error: [f32; 3], // absolute component errors: origin, velocity, view angles
    last_report: Option<i32>,
    pub messages: Vec<String>,
}

fn native_version(version: Version) -> native::Version {
    match version {
        Version::V102 => native::Version::V102,
        Version::V104 => native::Version::V104,
    }
}

fn import(version: Version, ps: &jk2::PlayerState) -> Result<native::State, String> {
    let mut state = native::State::zero(native_version(version));
    for (name, value) in ps.fields.iter() {
        state.set_field_bits(name, value)?;
    }
    state.stats = ps.stats;
    state.persistant = ps.persistant;
    state.ammo = ps.ammo;
    state.powerups = ps.powerups;
    Ok(state)
}

fn present(state: &native::State) -> Result<PlayerState, String> {
    // Player::normalized() has native 1.04 JK2 identifiers, even for 1.02 wire.
    // Map those identifiers exactly once, with the existing renderer adapter.
    let version = match state.version {
        native::Version::V102 => Version::V102,
        native::Version::V104 => Version::V104,
    };
    let mut raw = jk2::PlayerState::zero(version);
    for (&name, &bits) in native::field_names(state.version).iter().zip(&state.fields) {
        if !raw.fields.set_field_bits(name, bits) {
            return Err(format!("Unknown JK2 presentation field {name}"));
        }
    }
    raw.stats = state.stats;
    raw.persistant = state.persistant;
    raw.ammo = state.ammo;
    raw.powerups = state.powerups;
    presentation::player_state(version, &raw)
}

fn free_spectator(ps: &jk2::PlayerState, client: i32) -> bool {
    ps.fields.field_i32("pm_type") == Some(3) // JK2 PM_SPECTATOR, JKA is 4
        && ps.persistant[3] == 3 // PERS_TEAM / TEAM_SPECTATOR
        && ps.fields.field_i32("pm_flags").unwrap_or(0) & 4096 == 0
        && ps.fields.field_i32("clientNum") == Some(client)
}

fn boundary(snapshot: &jk2::Snapshot) -> (i32, i32, i32, i32) {
    (
        i32::from(snapshot.snap_flags & 4),
        snapshot
            .player_state
            .fields
            .field_i32("eFlags")
            .unwrap_or(0)
            & 4,
        snapshot
            .player_state
            .fields
            .field_i32("clientNum")
            .unwrap_or(-1),
        snapshot
            .player_state
            .fields
            .field_i32("pm_type")
            .unwrap_or(-1),
    )
}

fn settings(
    version: Version,
    config: &BTreeMap<u16, Vec<u8>>,
    client: i32,
) -> Result<native::Settings, String> {
    let local = config
        .get(&(544 + client as u16))
        .map(Vec::as_slice)
        .unwrap_or_default();
    let gameplay = match info_value(local, b"mvgp")
        .filter(|value| !value.is_empty())
        .map(atoi)
    {
        None => native_version(version),
        Some(2) => native::Version::V102,
        Some(3) => return Err("JK2 1.03 gameplay prediction is deferred".into()),
        Some(_) => native::Version::V104, // pinned CG_NewClientInfo fallback
    };
    let info = config.get(&0).map(Vec::as_slice).unwrap_or_default();
    let value = |key| info_value(info, key).map(atoi);
    let system = config.get(&1).map(Vec::as_slice).unwrap_or_default();
    let mut settings = native::Settings::new(gameplay);
    settings.pmove_fixed = info_value(system, b"pmove_fixed").map_or(0, atoi);
    settings.pmove_msec = info_value(system, b"pmove_msec")
        .map_or(8, atoi)
        .clamp(8, 33);
    settings.gametype = value(b"g_gametype").unwrap_or(0);
    settings.no_footsteps = i32::from(value(b"dmflags").unwrap_or(0) & 32 != 0);
    settings.tracemask &= !0x0200_0000; // CONTENTS_BODY: spectators pass through players
    settings.spectator_server_timing = 1;
    Ok(settings)
}

// Compare the client-predicted spectator contract, not server-owned score,
// ping, combat and event fields which Pmove cannot author for a spectator.
const COMPARE: &[&str] = &[
    "origin[0]",
    "origin[1]",
    "origin[2]",
    "velocity[0]",
    "velocity[1]",
    "velocity[2]",
    "viewangles[0]",
    "viewangles[1]",
    "viewangles[2]",
    "pm_type",
    "pm_flags",
    "pm_time",
    "groundEntityNum",
    "viewheight",
    "bobCycle",
    "legsAnim",
    "torsoAnim",
    "legsTimer",
    "torsoTimer",
];
// Validation bound only; never quantizes, snaps, smooths or changes prediction.
// The private server runs a compiled 32-bit QVM; native host math need not be
// bit-identical. Keep raw exact-match counts and first differences separately.
const FLOAT_ERROR_LIMIT: f32 = 1.0 / 1024.0;

fn error_bounds(predicted: &native::State, server: &native::State) -> ([f32; 3], bool) {
    let mut errors = [0.0_f32; 3];
    let mut discrete = false;
    for &name in COMPARE {
        let (Some(a), Some(b)) = (predicted.field_bits(name), server.field_bits(name)) else {
            continue;
        };
        let group = ["origin[", "velocity[", "viewangles["]
            .iter()
            .position(|&prefix| name.starts_with(prefix));
        if let Some(group) = group {
            let (a, b) = (f32::from_bits(a), f32::from_bits(b));
            let error = if a.is_finite() && b.is_finite() {
                (a - b).abs()
            } else {
                f32::INFINITY
            };
            errors[group] = errors[group].max(error);
        } else {
            discrete |= a != b;
        }
    }
    (errors, discrete)
}
fn first_difference(
    predicted: &native::State,
    server: &native::State,
) -> Option<(&'static str, u32, u32)> {
    COMPARE.iter().find_map(|&name| {
        let (a, b) = (predicted.field_bits(name)?, server.field_bits(name)?);
        (a != b).then_some((name, a, b))
    })
}

impl Predictor {
    fn invalidate(&mut self) {
        self.previous = None;
        self.boundary = None;
        self.records.clear();
        self.compared_snapshot = None;
        self.matched = 0;
        self.compared = 0;
        self.diverged = 0;
        self.max_error = [0.0; 3];
        self.last_report = None;
    }

    fn compare(
        &mut self,
        snapshot: &jk2::Snapshot,
        version: Version,
        debug: bool,
    ) -> Result<(), String> {
        if self.compared_snapshot == Some(snapshot.message_num) {
            return Ok(());
        }
        self.compared_snapshot = Some(snapshot.message_num);
        let time = snapshot
            .player_state
            .fields
            .field_i32("commandTime")
            .unwrap_or(0);
        if let Some((number, predicted)) = self
            .records
            .iter()
            .find(|(_, state)| state.field_bits("commandTime") == Some(time as u32))
        {
            let server = import(version, &snapshot.player_state)?;
            let difference = first_difference(predicted, &server);
            self.compared += 1;
            self.matched += u32::from(difference.is_none());
            let (errors, discrete) = error_bounds(predicted, &server);
            self.diverged +=
                u32::from(discrete || errors.iter().any(|&error| error > FLOAT_ERROR_LIMIT));
            for (max, error) in self.max_error.iter_mut().zip(errors) {
                *max = (*max).max(error);
            }
            if debug
                && (difference.is_some()
                    || self
                        .last_report
                        .is_none_or(|last| snapshot.server_time - last >= 1000))
            {
                let delta: [f32; 3] = std::array::from_fn(|axis| {
                    let name = format!("origin[{axis}]");
                    f32::from_bits(predicted.field_bits(&name).unwrap())
                        - f32::from_bits(server.field_bits(&name).unwrap())
                });
                self.messages.push(format!(
                    "JK2-PRED-SERVER version={} cmd={} commandTime={} snap={} matched={}/{} first={:?} originDelta={delta:?} maxAbsOriginVelocityAngles={errors:?} discreteMismatch={discrete}",
                    version.name(), number, time, snapshot.message_num, self.matched, self.compared, difference));
                self.last_report = Some(snapshot.server_time);
            }
        }
        self.records.retain(|(_, state)| {
            state
                .field_bits("commandTime")
                .is_some_and(|value| value as i32 > time)
        });
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn predict(
        &mut self,
        session: &ClientSession,
        snap: &Snapshot,
        next: Option<&Snapshot>,
        time: i32,
        world: &mut CollisionWorld,
        entities: &[crate::cgame::PresentedEntity],
        options: &NetworkSettings,
        provisional: Option<UserCmd>,
        animations: impl FnOnce() -> Result<Vec<u8>, String>,
    ) -> Result<Option<Frame>, String> {
        let version = session
            .jk2_version()
            .ok_or("JK2 prediction on JKA session")?;
        let wire = session.jk2_decoder().ok_or("Missing JK2 wire decoder")?;
        let Some(current) = wire.snapshot(snap.message_num) else {
            self.invalidate();
            return Ok(None);
        };
        if options.no_predict || !free_spectator(&current.player_state, session.client_num()) {
            self.invalidate();
            return Ok(None);
        }
        let next_raw = next.and_then(|next| wire.snapshot(next.message_num));
        let discontinuity = self.boundary.is_some_and(|old| old != boundary(current))
            || session.cmd_number() < self.last_command
            || time < self.last_time
            || self.version != Some(version);
        if discontinuity {
            self.invalidate();
        }
        self.boundary = Some(boundary(current));
        self.last_command = session.cmd_number();
        self.last_time = time;
        let base = next_raw
            .filter(|next| {
                !discontinuity
                    && boundary(next) == boundary(current)
                    && free_spectator(&next.player_state, session.client_num())
            })
            .unwrap_or(current);
        let settings = settings(version, &wire.configstrings, session.client_num())?;
        let regime = (
            settings.gameplay_version as i32,
            settings.pmove_fixed,
            settings.pmove_msec,
            settings.gametype,
            settings.no_footsteps,
        );
        if self.regime != Some(regime) {
            self.invalidate();
            self.boundary = Some(boundary(current));
            self.regime = Some(regime);
            self.messages.push(format!(
                "JK2-PRED native free spectator version={} gameplay={} fixed={} msec={}",
                version.name(),
                regime.0,
                regime.1,
                regime.2
            ));
        }
        self.compare(
            base,
            version,
            options.prediction_debug || options.show_miss || options.physics_diag != 0,
        )?;
        let commands: Vec<_> = ((session.cmd_number() - session.command_window() as i32 + 1).max(1)
            ..=session.cmd_number())
            .filter_map(|number| session.command(number).map(|cmd| (number, cmd)))
            .collect();
        let ack = base
            .player_state
            .fields
            .field_i32("commandTime")
            .unwrap_or(0);
        if commands.first().is_some_and(|(number, cmd)| {
            *number > 1 && cmd.server_time > ack && cmd.server_time < time
        }) {
            // CG_PredictPlayerState: command backup exhausted, freeze the last
            // valid prediction rather than replay a stream with a missing prefix.
            self.records.clear();
            return Ok(self.previous.clone().map(|ps| Frame {
                display: ps.clone(),
                committed: ps.clone(),
                previous: ps,
                angles: None,
            }));
        }
        if self.animations.is_none() {
            self.animations = Some(animations()?);
        }
        // Raw and mapped histories share message numbers, never field IDs.
        let solid_snapshot = next
            .filter(|next| next.message_num == base.message_num)
            .unwrap_or(snap);
        let solids = prediction_solid_entities(solid_snapshot, entities, base.server_time);
        let mut trace = PredictionWorld {
            world,
            solids: &solids,
            client_num: session.client_num(),
        };
        let result = self.replay(version, base, &commands, provisional, &settings, &mut trace);
        if result.is_err() {
            self.invalidate();
        }
        result.map(Some)
    }

    fn replay<W: TraceWorld>(
        &mut self,
        version: Version,
        base: &jk2::Snapshot,
        commands: &[(i32, UserCmd)],
        provisional: Option<UserCmd>,
        settings: &native::Settings,
        world: &mut W,
    ) -> Result<Frame, String> {
        let state = import(version, &base.player_state)?;
        if self.version != Some(version) || self.player.is_none() {
            self.player = Some(native::Player::new(
                native_version(version),
                self.animations.as_deref().ok_or("Missing JK2 animations")?,
                &state,
            )?);
            self.version = Some(version);
        }
        let player = self.player.as_mut().unwrap();
        player.set_network(&state)?;
        let latest = commands.last().map_or(0, |(_, cmd)| cmd.server_time);
        let mut angles = commands.last().map(|(_, cmd)| cmd.angles);
        let mut replay_time = state.field_bits("commandTime");
        for &(number, cmd) in commands {
            if cmd.server_time > latest {
                continue;
            } // previous map_restart
              // Nonfixed acknowledged commands have no side effects. Fixed mode
              // must still enter the bridge for its pre-skip view-angle update.
            if settings.pmove_fixed == 0
                && replay_time.is_some_and(|time| cmd.server_time <= time as i32)
            {
                continue;
            }
            let before = replay_time;
            player.predict(movement_cmd(&cmd), settings, world)?;
            let network = player.network();
            let command_time = network.field_bits("commandTime");
            replay_time = command_time;
            if command_time != before {
                angles = Some(cmd.angles);
                if !self
                    .records
                    .iter()
                    .any(|(_, state)| state.field_bits("commandTime") == command_time)
                {
                    self.records.push_back((number, network));
                }
            }
        }
        while self.records.len() > 512 {
            self.records.pop_front();
        }
        let committed = present(&player.normalized())?;
        let previous = self.previous.clone().unwrap_or_else(|| committed.clone());
        let mut display = committed.clone();
        if let Some(mut cmd) = provisional.filter(|cmd| cmd.server_time > latest) {
            // Match ClientSession::create_command_at's JK2 wire selection gate.
            cmd.weapon = 0;
            cmd.force_selection = 0;
            cmd.inventory_selection = 0;
            cmd.generic_command = 0;
            let mut preview = player.try_clone()?;
            preview.predict(movement_cmd(&cmd), settings, world)?;
            display = present(&preview.normalized())?;
            angles = Some(cmd.angles);
        }
        self.previous = Some(committed.clone());
        Ok(Frame {
            display,
            committed,
            previous,
            angles,
        })
    }
}
