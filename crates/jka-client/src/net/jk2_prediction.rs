//! Live MVSDK prediction of the local client's own playerstate, scheduled as
//! pinned CG_PredictPlayerState. Remote client context (cg_entities
//! currentState) is kept for every snapshot; follow mode is not predicted.
use super::{
    adjust_position_for_mover, movement_cmd, origin, prediction_solid_entities,
    provisional_presentation_state, set_origin, NetworkSettings, PredictionWorld,
};
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
    /// Items whose currentState gained EF_NODRAW from a predicted pickup
    /// (CG_TouchItem), until the server's next transition of that entity.
    pub hidden: Vec<u16>,
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
    regime: Option<(i32, i32, i32, i32, i32, i32, i32)>,
    /// pm_type of the last committed prediction (cg.predictedPlayerState).
    last_pm_type: Option<i32>,
    records: VecDeque<(i32, native::State)>,
    compared_snapshot: Option<i32>,
    matched: u32,
    compared: u32,
    diverged: u32,
    max_error: [f32; 3], // absolute component errors: origin, velocity, view angles
    last_report: Option<i32>,
    /// Last wire snapshot transitioned into the native remote-client context.
    context_message: Option<i32>,
    last_context_error: Option<i32>,
    /// Client centities' lerpOrigin as of the previous frame: CGame predicts
    /// before CG_AddPacketEntities recomputes them for the current cg.time.
    lerp_origins: [Option<[f32; 3]>; 32],
    /// cg_showMiss view trace: the previous frame's time, drawn origin and
    /// committed eventSequence.
    view_trace: Option<(i32, [f32; 3], i32)>,
    pub messages: Vec<String>,
}

fn native_version(version: Version) -> native::Version {
    match version {
        Version::V102 => native::Version::V102,
        Version::V104 => native::Version::V104,
    }
}

pub(crate) fn import(version: Version, ps: &jk2::PlayerState) -> Result<native::State, String> {
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

/// Runs for every trigger-type entity of every snapshot: wire schema
/// positions are resolved to native field positions once per version.
fn import_entity(version: Version, entity: &jk2::EntityState) -> Result<native::EntityState, String> {
    static MAPS: [std::sync::OnceLock<Vec<Option<usize>>>; 2] = [std::sync::OnceLock::new(), std::sync::OnceLock::new()];
    let names = native::entity_field_names(native_version(version));
    let map = MAPS[usize::from(version == Version::V104)].get_or_init(|| {
        entity.fields.iter().map(|(name, _)| names.iter().position(|&native| native == name)).collect()
    });
    let mut state = native::EntityState::zero(native_version(version), i32::from(entity.number));
    for ((name, value), index) in entity.fields.iter().zip(map) {
        let slot = index.and_then(|index| state.fields.get_mut(index));
        *slot.ok_or_else(|| format!("Unknown JK2 entity field {name}"))? = value;
    }
    Ok(state)
}

/// cg_predict.c saber-lock facing inputs: cg.time, cg.snap->ps and the lock
/// opponent centity's lerpOrigin.
fn lock_frame(current: &jk2::Snapshot, time: i32, lerp_origins: &[Option<[f32; 3]>; 32]) -> native::Frame {
    let fields = &current.player_state.fields;
    let lock_enemy = fields.field_i32("saberLockEnemy").unwrap_or(0);
    let lock_origin = usize::try_from(lock_enemy)
        .ok()
        .and_then(|index| lerp_origins.get(index).copied().flatten());
    native::Frame {
        cg_time: time,
        lock_time: fields.field_i32("saberLockTime").unwrap_or(0),
        lock_enemy,
        has_lock_origin: i32::from(lock_origin.is_some()),
        snap_origin: std::array::from_fn(|axis| {
            fields.field_f32(&format!("origin[{axis}]")).unwrap_or(0.0)
        }),
        lock_origin: lock_origin.unwrap_or_default(),
    }
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

// JK2 entityType_t values CG_BuildSolidList collects as triggers.
const ET_ITEM: i32 = 2;
const ET_PUSH_TRIGGER: i32 = 10;
const ET_TELEPORT_TRIGGER: i32 = 11;
const PM_SPECTATOR: i32 = 3; // JK2 pmtype_t (JKA's is 4)
const PM_DEAD: i32 = 4;
const TEAM_SPECTATOR: i32 = 3;
const PMF_FOLLOW: i32 = 4096;
const CONTENTS_BODY: i32 = 0x100; // JK2 surfaceflags.h, same as JKA

/// CG_PredictPlayerState predicts every own playerstate outside follow mode,
/// including dead, frozen and intermission states (Pmove handles those). One
/// host exception: the first active snapshot can be a spectator-team PM_NORMAL
/// state before the server has run a command; it is shown from snapshots
/// until the server applies spectator movement.
fn predictable(ps: &jk2::PlayerState, client: i32) -> bool {
    let fields = &ps.fields;
    fields.field_i32("pm_flags").unwrap_or(0) & PMF_FOLLOW == 0
        && fields.field_i32("clientNum") == Some(client)
        && !(ps.persistant[3] == TEAM_SPECTATOR && fields.field_i32("pm_type") != Some(PM_SPECTATOR))
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

/// The local client's MVSDK gameplay version (CG_NewClientInfo `mvgp`).
pub(crate) fn gameplay(
    version: Version,
    config: &BTreeMap<u16, Vec<u8>>,
    client: i32,
) -> Result<native::Version, String> {
    let local = config
        .get(&(544 + client as u16))
        .map(Vec::as_slice)
        .unwrap_or_default();
    match info_value(local, b"mvgp")
        .filter(|value| !value.is_empty())
        .map(atoi)
    {
        None => Ok(native_version(version)),
        Some(2) => Ok(native::Version::V102),
        Some(3) => Err("JK2 1.03 gameplay prediction is deferred".into()),
        Some(_) => Ok(native::Version::V104), // pinned CG_NewClientInfo fallback
    }
}

/// `mask_pm_type` is the pm_type CG_PredictPlayerState sees in cg_pmove.ps
/// (the previous prediction); `base` is the snapshot state being replayed.
fn settings(
    version: Version,
    config: &BTreeMap<u16, Vec<u8>>,
    client: i32,
    mask_pm_type: i32,
    base: &jk2::PlayerState,
    predict_items: bool,
) -> Result<native::Settings, String> {
    let gameplay = gameplay(version, config, client)?;
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
    // MASK_PLAYERSOLID; dead players and the spectator team pass through bodies.
    if mask_pm_type == PM_DEAD || base.persistant[3] == TEAM_SPECTATOR {
        settings.tracemask &= !CONTENTS_BODY;
    }
    // Only SpectatorThink drops fixed subdivision after G_ClientThink's time
    // rounding; ClientThink_real Pmove matches the CGame contract.
    settings.spectator_server_timing =
        i32::from(base.fields.field_i32("pm_type") == Some(PM_SPECTATOR));
    settings.predict_items = i32::from(predict_items);
    Ok(settings)
}

// Compare the client-predicted spectator contract, not server-owned score,
// ping, combat and event fields which Pmove cannot author for a spectator.
const SPECTATOR_COMPARE: &[&str] = &[
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
// Playing states add Pmove-owned weapon and saber fields. Server combat
// (damage, knockback, Force effects, events) still appears as divergence and
// is reported by first field; it is never folded into the prediction.
const PLAYING_COMPARE: &[&str] = &[
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
    "weapon",
    "weaponstate",
    "weaponTime",
    "saberMove",
];

fn compared_fields(server: &native::State) -> &'static [&'static str] {
    if server.field_bits("pm_type") == Some(PM_SPECTATOR as u32) {
        SPECTATOR_COMPARE
    } else {
        PLAYING_COMPARE
    }
}
// Validation bound only; never quantizes, snaps, smooths or changes prediction.
// The private server runs a compiled 32-bit QVM; native host math need not be
// bit-identical. Keep raw exact-match counts and first differences separately.
const FLOAT_ERROR_LIMIT: f32 = 1.0 / 1024.0;

fn error_bounds(predicted: &native::State, server: &native::State) -> ([f32; 3], bool) {
    let mut errors = [0.0_f32; 3];
    let mut discrete = false;
    for &name in compared_fields(server) {
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
    compared_fields(server).iter().find_map(|&name| {
        let (a, b) = (predicted.field_bits(name)?, server.field_bits(name)?);
        (a != b).then_some((name, a, b))
    })
}

impl Predictor {
    /// cg.predictedPlayerState from committed commands, as a wire state.
    pub(crate) fn predicted_state(&self) -> Option<native::State> {
        self.previous.as_ref()?;
        self.player.as_ref().map(native::Player::network)
    }

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
                // Every differing field on a material miss: one first field
                // cannot tell a step-up from a jump or animation decision.
                let fields = if discrete || errors.iter().any(|&error| error > 0.5) {
                    let mut text = String::new();
                    for &name in compared_fields(&server) {
                        let (Some(a), Some(b)) = (predicted.field_bits(name), server.field_bits(name)) else {
                            continue;
                        };
                        if a == b {
                            continue;
                        }
                        if ["origin[", "velocity[", "viewangles["].iter().any(|p| name.starts_with(p)) {
                            text += &format!(" {name}={}/{}", f32::from_bits(a), f32::from_bits(b));
                        } else {
                            text += &format!(" {name}={}/{}", a as i32, b as i32);
                        }
                    }
                    format!(" predicted/server:{text}")
                } else {
                    String::new()
                };
                self.messages.push(format!(
                    "JK2-PRED-SERVER version={} cmd={} commandTime={} snap={} matched={}/{} first={:?} originDelta={delta:?} maxAbsOriginVelocityAngles={errors:?} discreteMismatch={discrete}{fields}",
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
        let predicting =
            !options.no_predict && predictable(&current.player_state, session.client_num());
        let version_changed = self.version != Some(version);
        // CGame transitions cg_entities whether or not it predicts this frame.
        if let Err(error) = self.update_context(version, wire, current, animations) {
            if self.player.is_none() && predicting {
                return Err(error);
            }
            if self.last_context_error.is_none_or(|last| current.server_time - last >= 1000) {
                self.messages.push(format!("JK2-PRED context reset: {error}"));
                self.last_context_error = Some(current.server_time);
            }
        }
        let frame = lock_frame(current, time, &self.lerp_origins);
        self.lerp_origins = [None; 32];
        for entity in entities.iter().filter(|entity| entity.number < 32) {
            self.lerp_origins[usize::from(entity.number)] = Some(entity.origin);
        }
        if !predicting {
            self.invalidate();
            return Ok(None);
        }
        let next_raw = next.and_then(|next| wire.snapshot(next.message_num));
        let discontinuity = self.boundary.is_some_and(|old| old != boundary(current))
            || session.cmd_number() < self.last_command
            || time < self.last_time
            || version_changed;
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
                    && predictable(&next.player_state, session.client_num())
            })
            .unwrap_or(current);
        // cg_pmove.ps is still the previous prediction when its mask is chosen.
        let mask_pm_type = self
            .last_pm_type
            .unwrap_or_else(|| current.player_state.fields.field_i32("pm_type").unwrap_or(0));
        let settings = settings(
            version,
            &wire.configstrings,
            session.client_num(),
            mask_pm_type,
            &base.player_state,
            options.predict_items,
        )?;
        let regime = (
            settings.gameplay_version as i32,
            settings.pmove_fixed,
            settings.pmove_msec,
            settings.gametype,
            settings.no_footsteps,
            settings.tracemask,
            settings.spectator_server_timing,
        );
        if self.regime != Some(regime) {
            self.invalidate();
            self.boundary = Some(boundary(current));
            self.regime = Some(regime);
            self.messages.push(format!(
                "JK2-PRED native prediction version={} gameplay={} fixed={} msec={} pm_type={} mask={:#x} serverSpectatorTiming={}",
                version.name(),
                regime.0,
                regime.1,
                regime.2,
                base.player_state.fields.field_i32("pm_type").unwrap_or(-1),
                regime.5,
                regime.6,
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
            let hidden = self.player.as_ref().map(hidden_items).unwrap_or_default();
            return Ok(self.previous.clone().map(|ps| Frame {
                display: ps.clone(),
                committed: ps.clone(),
                previous: ps,
                angles: None,
                hidden,
            }));
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
        let result = self.replay(version, base, &commands, provisional, &settings, &frame, &mut trace);
        if result.is_err() {
            self.invalidate();
        }
        result.map(|mut frame| {
            // CG_AdjustPositionForMover from cg.physicsTime to cg.time.
            for ps in [&mut frame.committed, &mut frame.display] {
                let adjusted = adjust_position_for_mover(entities, ps, origin(ps), base.server_time, time);
                set_origin(ps, adjusted);
            }
            if options.show_miss {
                self.trace_view(&frame, base, time);
            }
            Some(frame)
        })
    }

    /// cg_showMiss: report a drawn-position jump of more than 6 units between
    /// consecutive frames, and every new committed step/jump/land event, with
    /// the drawn and committed states and the base snapshot.
    fn trace_view(&mut self, frame: &Frame, base: &jk2::Snapshot, time: i32) {
        use jka_protocol::entity_event::EntityEvent;
        let drawn = origin(&frame.display);
        let committed = origin(&frame.committed);
        let sequence = frame.committed.field_i32("eventSequence").unwrap_or(0);
        let Some((last_time, last_drawn, last_sequence)) = self.view_trace.replace((time, drawn, sequence)) else {
            return;
        };
        let jump = (0..3).map(|k| (drawn[k] - last_drawn[k]).powi(2)).sum::<f32>().sqrt();
        let mut events = Vec::new();
        for slot in last_sequence.max(sequence - 2)..sequence {
            let raw = frame.committed.field_i32(&format!("events[{}]", slot & 1)).unwrap_or(0) & !0x300;
            if let Some(event) = EntityEvent::from_i32(raw) {
                if matches!(
                    event,
                    EntityEvent::EV_STEP_4 | EntityEvent::EV_STEP_8 | EntityEvent::EV_STEP_12 | EntityEvent::EV_STEP_16
                        | EntityEvent::EV_JUMP | EntityEvent::EV_FALL
                ) {
                    events.push(format!("{event:?}"));
                }
            }
        }
        if jump <= 6.0 && events.is_empty() {
            return;
        }
        let bits = |name: &str| f32::from_bits(base.player_state.fields.field_bits(name).unwrap_or(0));
        self.messages.push(format!(
            "JK2-VIEW time={time} dt={} drawnMove={jump:.2} drawn={drawn:?} committed={committed:?} ground={} legsAnim={} vz={} events={events:?} snapCommandTime={} snapOrigin=[{}, {}, {}]",
            time - last_time,
            frame.committed.field_i32("groundEntityNum").unwrap_or(-1),
            frame.committed.field_i32("legsAnim").unwrap_or(-1) & !2048,
            frame.committed.field_f32("velocity[2]").unwrap_or(0.0),
            base.player_state.fields.field_i32("commandTime").unwrap_or(0),
            bits("origin[0]"),
            bits("origin[1]"),
            bits("origin[2]"),
        ));
    }

    /// Create the native player once per version, then apply every wire
    /// snapshot since the last transition, as CG_ProcessSnapshots does.
    fn update_context(
        &mut self,
        version: Version,
        wire: &jk2::Decoder,
        current: &jk2::Snapshot,
        animations: impl FnOnce() -> Result<Vec<u8>, String>,
    ) -> Result<(), String> {
        if self.version != Some(version) {
            self.player = None;
        }
        if self.player.is_none() {
            if self.animations.is_none() {
                self.animations = Some(animations()?);
            }
            self.player = Some(native::Player::new(
                native_version(version),
                self.animations.as_deref().ok_or("Missing JK2 animations")?,
                &import(version, &current.player_state)?,
            )?);
            self.version = Some(version);
            self.context_message = None;
            self.last_pm_type = None;
        }
        let player = self.player.as_mut().unwrap();
        let svflags = info_value(wire.configstrings.get(&0).map(Vec::as_slice).unwrap_or_default(), b"mvsdk_svFlags")
            .map_or(0, atoi);
        let first = match self.context_message {
            Some(last) if last == current.message_num => return Ok(()),
            // The wire ring keeps PACKET_BACKUP snapshots; older ones are gone.
            Some(last) if last < current.message_num => (last + 1).max(current.message_num - 31),
            _ => current.message_num,
        };
        self.context_message = Some(current.message_num);
        let result = (first..=current.message_num)
            .filter_map(|message| wire.snapshot(message))
            .try_for_each(|snapshot| {
                let entities = snapshot
                    .entities
                    .iter()
                    .filter(|entity| entity.number < 32)
                    .map(|entity| import_entity(version, entity))
                    .collect::<Result<Vec<_>, _>>()?;
                player.transition_context(&import(version, &snapshot.player_state)?, &entities)?;
                // Only trigger-type currentState matters to CG_BuildSolidList.
                let (mut triggers, mut others) = (Vec::new(), Vec::new());
                for entity in snapshot.entities.iter().filter(|entity| entity.number >= 32) {
                    let kind = entity.fields.field_i32("eType").unwrap_or(-1);
                    if [ET_ITEM, ET_PUSH_TRIGGER, ET_TELEPORT_TRIGGER].contains(&kind) {
                        triggers.push(import_entity(version, entity)?);
                    } else {
                        others.push(i32::from(entity.number));
                    }
                }
                player.transition_world(&triggers, &others, svflags)
            });
        if result.is_err() {
            player.reset_context();
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn replay<W: TraceWorld>(
        &mut self,
        version: Version,
        base: &jk2::Snapshot,
        commands: &[(i32, UserCmd)],
        provisional: Option<UserCmd>,
        settings: &native::Settings,
        frame: &native::Frame,
        world: &mut W,
    ) -> Result<Frame, String> {
        let state = import(version, &base.player_state)?;
        let mut player = self.player.take().ok_or("Missing JK2 native player")?;
        let latest = commands.last().map_or(0, |(_, cmd)| cmd.server_time);
        let mut angles = commands.last().map(|(_, cmd)| cmd.angles);
        // CG_BuildSolidList: the snapshot prediction starts from.
        let triggers: Vec<i32> = base.entities.iter().map(|entity| i32::from(entity.number)).collect();
        let result = player.set_network(&state).and_then(|()| {
            player.set_frame(frame);
            player.set_triggers(&triggers);
            // Remote writebacks persist across frames like cg_entities, but
            // only when the whole committed replay succeeds.
            player.context_transaction(|player| {
                self.replay_committed(player, &state, commands, latest, settings, world, &mut angles)
            })
        });
        let result =
            result.and_then(|()| self.finish_replay(&player, latest, provisional, settings, world, angles));
        self.player = Some(player);
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn replay_committed<W: TraceWorld>(
        &mut self,
        player: &mut native::Player,
        state: &native::State,
        commands: &[(i32, UserCmd)],
        latest: i32,
        settings: &native::Settings,
        world: &mut W,
        angles: &mut Option<[i32; 3]>,
    ) -> Result<(), String> {
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
                *angles = Some(cmd.angles);
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
        Ok(())
    }

    fn finish_replay<W: TraceWorld>(
        &mut self,
        player: &native::Player,
        latest: i32,
        provisional: Option<UserCmd>,
        settings: &native::Settings,
        world: &mut W,
        mut angles: Option<[i32; 3]>,
    ) -> Result<Frame, String> {
        self.last_pm_type = player.network().field_bits("pm_type").map(|bits| bits as i32);
        let committed = present(&player.normalized())?;
        let previous = self.previous.clone().unwrap_or_else(|| committed.clone());
        let mut display = committed.clone();
        // LiveInput::preview_cmd already carries JK2 wire selections.
        if let Some(cmd) = provisional.filter(|cmd| cmd.server_time > latest) {
            let mut preview = player.try_clone()?;
            preview.predict(movement_cmd(&cmd), settings, world)?;
            // Unsent input moves the view only; animations/events stay committed.
            display = provisional_presentation_state(&present(&preview.normalized())?, &committed);
            angles = Some(cmd.angles);
        }
        self.previous = Some(committed.clone());
        Ok(Frame {
            display,
            committed,
            previous,
            angles,
            hidden: hidden_items(player),
        })
    }
}

fn hidden_items(player: &native::Player) -> Vec<u16> {
    player.predicted_nodraw().into_iter().filter_map(|number| u16::try_from(number).ok()).collect()
}
