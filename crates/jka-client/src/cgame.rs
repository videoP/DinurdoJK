//! Faithful client-game snapshot/entity state translated from OpenJK `codemp/cgame/cg_snapshot.c`
//! and `codemp/cgame/cg_ents.c`.
//!
//! This module intentionally sits downstream of the protocol decoder. Demo playback and a future
//! network source both feed the same decoded `Snapshot` values into this layer.

mod player_animation;
pub(crate) mod ragdoll;
mod saber_throw;
pub mod item_presenter;
pub mod weapon_fx;
pub(crate) mod saber_melt;
pub mod sound_presenter;
pub mod stringed;
pub mod entity_presenter;
pub mod event_debug;
pub mod event_presenter;
pub(crate) mod event_workers;
pub mod player_presenter;
pub mod view;

use std::collections::{BTreeMap, VecDeque};

use jka_assets::siege::{find_siege_class_visual, SiegeClassVisual};
use jka_movement::PlayerEntityView;
use jka_protocol::{
    entity_event::{EntityEvent, EV_EVENT_BITS},
    gamestate::{EntityState, ENTITY_FIELDS},
    server::{PlayerState, ServerCommand, Snapshot},
};


// OpenJK `bg_public.h` / q_shared configstring layout. CS_PLAYERS evaluates to 1131 in
// protocol 26 (CS_ICONS + MAX_ICONS). Keep these wire-visible indexes fixed.
pub const CS_SERVERINFO: u16 = 0;
pub const CS_ITEMS: u16 = 27; // OpenJK: server-built item precache bitstring
// OpenJK/TaystJK protocol-26 configstring layout from bg_public.h. Keeping these
// in the cgame layer lets demos and a future live net source resolve the same
// model/sound/effect indexes without renderer-owned lookup tables.
pub const CS_MODELS: u16 = 298;
pub const CS_SOUNDS: u16 = 811;
pub const CS_PLAYERS: u16 = 1131;
pub const CS_EFFECTS: u16 = 1355;
pub const GT_SIEGE: i32 = 7;

/// Shared OpenJK `CG_SetNextSnap` interpolation break conditions. Camera sampling and entity
/// presentation must agree on these or the body/camera can interpolate across different players,
/// teleports, or map_restart boundaries.
pub fn snapshot_discontinuity(current: &Snapshot, next: &Snapshot) -> bool {
    let current_eflags = current.player_state.field_i32("eFlags").unwrap_or(0);
    let next_eflags = next.player_state.field_i32("eFlags").unwrap_or(0);
    let current_client = current.player_state.field_i32("clientNum").unwrap_or(-1);
    let next_client = next.player_state.field_i32("clientNum").unwrap_or(-1);
    ((current_eflags ^ next_eflags) & EF_TELEPORT_BIT) != 0
        || current_client != next_client
        || ((current.snap_flags ^ next.snap_flags) & SNAPFLAG_SERVERCOUNT) != 0
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInfo {
    pub client_num: usize,
    pub name: String,
    pub team: i32,
    pub gametype: i32,
    /// CG_NewClientInfo's `ds` gender hint selects the missing-model fallback.
    pub female: bool,
    pub model_name: String,
    pub skin_name: String,
    pub siege_class: String,
    pub saber_name: String,
    pub saber2_name: String,
    /// OpenJK `CG_NewClientInfo` stores the protocol `c1`/`c2` values in
    /// `icolor1`/`icolor2`, and `CG_AddSaberBlade` uses those integers as the
    /// multiplayer saber colors (invalid/non-saber values fall back to blue).
    pub saber_color: i32,
    pub saber2_color: i32,
    /// CG_AddSaberBlade for an NPC without `boltToPlayer` colors draws each
    /// blade in its saber definition's authored color instead of c1/c2.
    pub definition_saber_colors: bool,
}

impl ClientInfo {
    /// Offline/local default matching the requested stock Kyle presentation.
    /// The presenter still consumes an ordinary ClientInfo so solo does not get
    /// a separate model-rendering path.
    pub fn solo_kyle() -> Self {
        Self {
            client_num: 0,
            name: "Kyle".to_owned(),
            team: 0,
            gametype: 0,
            female: false,
            model_name: "kyle".to_owned(),
            skin_name: "default".to_owned(),
            siege_class: String::new(),
            saber_name: String::new(),
            saber2_name: String::new(),
            saber_color: 4, // SABER_BLUE
            saber2_color: 4,
            definition_saber_colors: false,
        }
    }

    /// Offline/local model selection using the same model/skin split as the
    /// OpenJK clientinfo path below. Empty input retains the requested Kyle default.
    pub fn solo_model(value: &str) -> Self {
        let value = value.trim();
        let (model_name, skin_name) = if value.is_empty() {
            ("kyle".to_owned(), "default".to_owned())
        } else {
            split_model_skin(value)
        };
        Self { model_name, skin_name, ..Self::solo_kyle() }
    }

    pub fn model_cvar(&self) -> String {
        if self.skin_name.eq_ignore_ascii_case("default") {
            self.model_name.clone()
        } else {
            format!("{}/{}", self.model_name, self.skin_name)
        }
    }
    /// The exact skin-name construction used by OpenJK `CG_RegisterClientModelname`.
    pub fn skin_qpath(&self) -> String {
        if self.skin_name.contains('|')
            && self.skin_name.contains("head")
            && self.skin_name.contains("torso")
            && self.skin_name.contains("lower")
        {
            format!("models/players/{}/|{}", self.model_name, self.skin_name)
        } else {
            format!(
                "models/players/{}/model_{}.skin",
                self.model_name, self.skin_name
            )
        }
    }

    pub fn default_skin_qpath(&self) -> String {
        format!("models/players/{}/model_default.skin", self.model_name)
    }

    /// CG_LoadClientInfo registration failure policy (GT_TEAM == 6).
    /// Preserve client identity and equipment; only replace the visual asset choice.
    pub fn missing_model_fallback(&self) -> Self {
        Self {
            model_name: if self.female { "jan" } else { "kyle" }.to_owned(),
            skin_name: if self.gametype >= 6 {
                self.skin_name.clone()
            } else {
                "default".to_owned()
            },
            ..self.clone()
        }
    }

    pub fn model_qpath(&self) -> String {
        format!("models/players/{}/model.glm", self.model_name)
    }
}

const MAX_GENTITIES: usize = 1024;
const MAX_CLIENTS: usize = 32;
const EF_TELEPORT_BIT: i32 = 1 << 3;
const SNAPFLAG_SERVERCOUNT: u8 = 1 << 2;
const DEFAULT_GRAVITY: f32 = 800.0;
const CLASS_VEHICLE: i32 = 53;
const GIB_HEALTH: i32 = -40;
const PM_SPECTATOR: i32 = 4;
const PM_INTERMISSION: i32 = 7;
const PERS_TEAM: usize = 3;
const TEAM_SPECTATOR: i32 = 3;
const EF_DEAD: i32 = 1 << 1;
const EF_SEEKERDRONE: i32 = 1 << 21;

pub const ET_GENERAL: i32 = 0;
pub const ET_PLAYER: i32 = 1;
pub const ET_ITEM: i32 = 2;
pub const ET_MISSILE: i32 = 3;
pub const ET_SPECIAL: i32 = 4;
pub const ET_HOLOCRON: i32 = 5;
pub const ET_MOVER: i32 = 6;
pub const ET_BEAM: i32 = 7;
pub const ET_PORTAL: i32 = 8;
pub const ET_SPEAKER: i32 = 9;
pub const ET_PUSH_TRIGGER: i32 = 10;
pub const ET_TELEPORT_TRIGGER: i32 = 11;
pub const ET_INVISIBLE: i32 = 12;
pub const ET_NPC: i32 = 13;
pub const ET_TEAM: i32 = 14;
pub const ET_BODY: i32 = 15;
pub const ET_TERRAIN: i32 = 16;
pub const ET_FX: i32 = 17;
pub const ET_EVENTS: i32 = 18;

const MAX_PS_EVENTS: i32 = 2;
const MAX_PREDICTED_EVENTS: usize = 16;
const EVENT_VALID_MSEC: i32 = 300;
const EF_PLAYER_EVENT: i32 = 1 << 5;

const TR_STATIONARY: i32 = 0;
const TR_INTERPOLATE: i32 = 1;
const TR_LINEAR: i32 = 2;
const TR_LINEAR_STOP: i32 = 3;
const TR_NONLINEAR_STOP: i32 = 4;
const TR_SINE: i32 = 5;
const TR_GRAVITY: i32 = 6;

#[derive(Debug, Clone)]
pub struct CEntity {
    pub current_state: Option<EntityState>,
    pub next_state: Option<EntityState>,
    pub current_valid: bool,
    pub interpolate: bool,
    pub snapshot_time: i32,
    pub previous_event: i32,
    pub lerp_origin: [f32; 3],
    pub lerp_angles: [f32; 3],
    /// OpenJK `cent->miscTime`; EV_ITEM_RESPAWN stamps it so CG_Item can
    /// fade the item back in.
    pub misc_time: i32,
}

impl Default for CEntity {
    fn default() -> Self {
        Self {
            current_state: None,
            next_state: None,
            current_valid: false,
            interpolate: false,
            snapshot_time: 0,
            previous_event: 0,
            lerp_origin: [0.0; 3],
            lerp_angles: [0.0; 3],
            misc_time: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PresentedEntity {
    pub number: u16,
    pub entity_type: i32,
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    pub state: EntityState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityPresentationKind {
    General,
    Player,
    Item,
    Missile,
    Special,
    Holocron,
    Mover,
    Beam,
    Portal,
    Speaker,
    PushTrigger,
    TeleportTrigger,
    Invisible,
    Npc,
    Team,
    Body,
    Terrain,
    Fx,
    Event,
    Unknown,
}

impl PresentedEntity {
    pub fn presentation_kind(&self) -> EntityPresentationKind {
        match self.entity_type {
            ET_GENERAL => EntityPresentationKind::General,
            ET_PLAYER => EntityPresentationKind::Player,
            ET_ITEM => EntityPresentationKind::Item,
            ET_MISSILE => EntityPresentationKind::Missile,
            ET_SPECIAL => EntityPresentationKind::Special,
            ET_HOLOCRON => EntityPresentationKind::Holocron,
            ET_MOVER => EntityPresentationKind::Mover,
            ET_BEAM => EntityPresentationKind::Beam,
            ET_PORTAL => EntityPresentationKind::Portal,
            ET_SPEAKER => EntityPresentationKind::Speaker,
            ET_PUSH_TRIGGER => EntityPresentationKind::PushTrigger,
            ET_TELEPORT_TRIGGER => EntityPresentationKind::TeleportTrigger,
            ET_INVISIBLE => EntityPresentationKind::Invisible,
            ET_NPC => EntityPresentationKind::Npc,
            ET_TEAM => EntityPresentationKind::Team,
            ET_BODY => EntityPresentationKind::Body,
            ET_TERRAIN => EntityPresentationKind::Terrain,
            ET_FX => EntityPresentationKind::Fx,
            e if e >= ET_EVENTS => EntityPresentationKind::Event,
            _ => EntityPresentationKind::Unknown,
        }
    }
}

/// Renderer/audio-independent equivalent of the event identity produced by
/// OpenJK `CG_CheckEvents`.  The semantic `event` has EV_EVENT_BITS removed;
/// `raw_event` is retained for diagnostics and exact transition verification.
#[derive(Debug, Clone)]
pub struct PresentationEvent {
    /// Monotonic client receive-queue sequence. Zero means the event has been
    /// constructed but has not yet crossed the receive queue boundary.
    pub receive_sequence: u64,
    pub source_entity_num: u16,
    pub entity_num: u16,
    pub event: EntityEvent,
    pub raw_event: i32,
    pub parm: i32,
    pub position: [f32; 3],
    pub event_only_entity: bool,
    pub server_time: i32,
    pub state: EntityState,
}

/// Client-only receive queue for events that OpenJK-compatible snapshot /
/// playerstate discovery has already accepted. This does not change protocol
/// semantics: it only separates discovery from deterministic presentation.
#[derive(Debug)]
struct PresentationEventQueue {
    next_sequence: u64,
    events: VecDeque<PresentationEvent>,
}

impl Default for PresentationEventQueue {
    fn default() -> Self {
        Self {
            next_sequence: 1,
            events: VecDeque::new(),
        }
    }
}

impl PresentationEventQueue {
    fn clear(&mut self) {
        self.events.clear();
        self.next_sequence = 1;
    }

    fn push(&mut self, mut event: PresentationEvent) {
        event.receive_sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        if self.next_sequence == 0 {
            self.next_sequence = 1;
        }
        self.events.push_back(event);
    }

    fn pop_front(&mut self) -> Option<PresentationEvent> {
        self.events.pop_front()
    }
}

/// Diagnostic result from the OpenJK-style `CG_CheckEvents` gate.  This is
/// deliberately separate from [`PresentationEvent`]: accepted events continue
/// into presentation/audio dispatch, while duplicates/zero events can be
/// surfaced by `cg_debugEvents` without changing game behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventCheckDisposition {
    Accepted,
    Duplicate,
    Zero,
    NoEvent,
}

#[derive(Debug, Clone)]
pub struct EventCheckTrace {
    pub source_entity_num: u16,
    pub entity_num: u16,
    pub entity_type: i32,
    pub server_time: i32,
    pub raw_event: i32,
    pub event: i32,
    pub previous_event: i32,
    pub event_only_entity: bool,
    pub disposition: EventCheckDisposition,
}

struct EventCheckOutcome {
    event: Option<PresentationEvent>,
    trace: EventCheckTrace,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EntityTypeSummary {
    pub total: usize,
    pub players: usize,
    pub npcs: usize,
    pub movers: usize,
    pub missiles: usize,
    pub items: usize,
    pub event_entities: usize,
    pub other: usize,
}

/// One OpenJK `scores` client record. The wire command carries 14 integers
/// per client; keep the complete record here so presentation can evolve without
/// reparsing or changing protocol semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoreEntry {
    pub client: i32,
    pub score: i32,
    pub ping: i32,
    pub time: i32,
    pub score_flags: i32,
    pub powerups: i32,
    pub accuracy: i32,
    pub impressive_count: i32,
    pub excellent_count: i32,
    pub gauntlet_count: i32,
    pub defend_count: i32,
    pub assist_count: i32,
    pub perfect: i32,
    pub captures: i32,
    pub name: String,
    pub team: i32,
}

/// Reliable Ghoul2 lifecycle commands emitted by the OpenJK server.
///
/// These are presentation state, not UI notices: `ircg` is the authoritative
/// body-queue copy signal and `rcg` forces the live client Ghoul2 weapon state
/// to be reinitialized on respawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ghoul2ServerCommand {
    BodyQueueCopy {
        source_client: u16,
        body_entity: u16,
        known_weapon: i32,
        light_side: bool,
    },
    RestoreClient {
        source_client: u16,
    },
}

/// Text/output state from CG_ServerCommand for the console and HUD UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CgameNotice {
    /// `print`: console text (may contain @@@ StringEd references).
    Print(Vec<u8>),
    /// `chat`/`tchat`/`lchat`/`ltchat`: a chat-box line.
    Chat { team: bool, text: Vec<u8> },
    /// `cp`: CG_CenterPrint.
    CenterPrint(Vec<u8>),
    /// `scores`: OpenJK scoreboard response.
    Scores {
        team_scores: [i32; 2],
        entries: Vec<ScoreEntry>,
    },
    /// `map_restart`.
    MapRestart,
}

pub struct ClientGameState {
    entities: Vec<CEntity>,
    notices: VecDeque<CgameNotice>,
    big_config: jka_protocol::commands::BigConfigString,
    configstrings: BTreeMap<u16, Vec<u8>>,
    queued_server_commands: VecDeque<ServerCommand>,
    executed_server_command: i32,
    current_snapshot: Option<Snapshot>,
    next_snapshot: Option<Snapshot>,
    frame_interpolation: f32,
    /// OpenJK cg.eventSequence / cg.predictableEvents. These track events
    /// already played from committed local prediction so later predicted
    /// corrections can be recognized without replaying every render frame.
    predicted_event_sequence: i32,
    predictable_events: [i32; MAX_PREDICTED_EVENTS],
    presentation_events: PresentationEventQueue,
    pending_event_traces: VecDeque<EventCheckTrace>,
    pending_ghoul2_commands: VecDeque<Ghoul2ServerCommand>,
}

impl Default for ClientGameState {
    fn default() -> Self {
        Self {
            entities: vec![CEntity::default(); MAX_GENTITIES],
            notices: VecDeque::new(),
            big_config: jka_protocol::commands::BigConfigString::default(),
            configstrings: BTreeMap::new(),
            queued_server_commands: VecDeque::new(),
            executed_server_command: 0,
            current_snapshot: None,
            next_snapshot: None,
            frame_interpolation: 0.0,
            predicted_event_sequence: 0,
            predictable_events: [0; MAX_PREDICTED_EVENTS],
            presentation_events: PresentationEventQueue::default(),
            pending_event_traces: VecDeque::new(),
            pending_ghoul2_commands: VecDeque::new(),
        }
    }
}

impl ClientGameState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset_gamestate(
        &mut self,
        configstrings: &BTreeMap<u16, Vec<u8>>,
        server_command_sequence: i32,
    ) {
        self.entities.fill(CEntity::default());
        self.big_config = jka_protocol::commands::BigConfigString::default();
        self.configstrings.clone_from(configstrings);
        self.queued_server_commands.clear();
        self.executed_server_command = server_command_sequence;
        self.current_snapshot = None;
        self.next_snapshot = None;
        self.frame_interpolation = 0.0;
        self.predicted_event_sequence = 0;
        self.predictable_events = [0; MAX_PREDICTED_EVENTS];
        self.presentation_events.clear();
        self.pending_event_traces.clear();
        self.pending_ghoul2_commands.clear();
    }

    pub fn drain_ghoul2_commands(&mut self) -> Vec<Ghoul2ServerCommand> {
        self.pending_ghoul2_commands.drain(..).collect()
    }

    pub fn drain_notices(&mut self) -> Vec<CgameNotice> {
        self.notices.drain(..).collect()
    }

    pub fn queue_server_command(&mut self, command: ServerCommand) {
        if command.sequence > self.executed_server_command {
            self.queued_server_commands.push_back(command);
        }
    }

    pub fn configstring(&self, index: u16) -> Option<&[u8]> {
        self.configstrings.get(&index).map(Vec::as_slice)
    }

    /// Semantic configstring update used by non-wire sources such as the
    /// in-process local server. Network/demo paths still arrive through the
    /// normal `cs` server-command machinery.
    pub fn set_configstring(&mut self, index: u16, value: Vec<u8>) {
        self.configstrings.insert(index, value);
    }

    pub fn model_qpath(&self, model_index: i32) -> Option<String> {
        configstring_resource(&self.configstrings, CS_MODELS, 512, model_index)
    }

    pub fn sound_qpath(&self, sound_index: i32) -> Option<String> {
        configstring_resource(&self.configstrings, CS_SOUNDS, 256, sound_index)
    }

    pub fn effect_qpath(&self, effect_index: i32) -> Option<String> {
        configstring_resource(&self.configstrings, CS_EFFECTS, 64, effect_index)
    }

    /// OpenJK `CG_TransitionPlayerState` event portion for the local predicted
    /// player. `ps` and `ops` must be committed predicted states, never the
    /// display-only provisional command, or high render FPS would replay an
    /// event before the corresponding usercmd is committed.
    pub fn transition_predicted_player_state(
        &mut self,
        ps: &PlayerState,
        ops: &PlayerState,
        server_time: i32,
    ) -> Result<(), String> {
        if ps.field_i32("clientNum") != ops.field_i32("clientNum") {
            // CG_TransitionPlayerState sets ops = ps when the followed client
            // changes, preventing old-player events from leaking across.
            return Ok(());
        }

        self.check_playerstate_events(ps, ops, server_time)?;
        // Port the correction gate as requested by the live handoff. Current
        // OpenJK keeps this routine available even where a callsite may be
        // disabled; running it after the ordinary transition cannot duplicate
        // events just stored above, but can replace a recently mispredicted one.
        self.check_changed_predictable_events(ps, server_time)
    }

    /// OpenJK `CG_CheckPlayerstateEvents`.
    fn check_playerstate_events(
        &mut self,
        ps: &PlayerState,
        ops: &PlayerState,
        server_time: i32,
    ) -> Result<(), String> {
        let external = ps.field_i32("externalEvent").unwrap_or(0);
        if external != 0 && external != ops.field_i32("externalEvent").unwrap_or(0) {
            let parm = ps.field_i32("externalEventParm").unwrap_or(0);
            self.push_predicted_player_event(ps, external, parm, server_time)?;
        }

        let sequence = ps.field_i32("eventSequence").unwrap_or(0);
        let old_sequence = ops.field_i32("eventSequence").unwrap_or(0);
        for index in sequence.saturating_sub(MAX_PS_EVENTS)..sequence {
            let slot = index & (MAX_PS_EVENTS - 1);
            let event = ps.field_i32(&format!("events[{slot}]")).unwrap_or(0);
            let old_event = ops.field_i32(&format!("events[{slot}]")).unwrap_or(0);
            if index >= old_sequence
                || (index > old_sequence.saturating_sub(MAX_PS_EVENTS) && event != old_event)
            {
                let parm = ps.field_i32(&format!("eventParms[{slot}]")).unwrap_or(0);
                self.push_predicted_player_event(ps, event, parm, server_time)?;
                self.predictable_events[index as usize & (MAX_PREDICTED_EVENTS - 1)] = event;
                self.predicted_event_sequence = self.predicted_event_sequence.saturating_add(1);
            }
        }
        Ok(())
    }

    /// OpenJK `CG_CheckChangedPredictableEvents`: if the authoritative/replayed
    /// committed prediction changes one of the recent predictable events,
    /// dispatch the corrected event once and replace the ring entry.
    fn check_changed_predictable_events(
        &mut self,
        ps: &PlayerState,
        server_time: i32,
    ) -> Result<(), String> {
        let sequence = ps.field_i32("eventSequence").unwrap_or(0);
        for index in sequence.saturating_sub(MAX_PS_EVENTS)..sequence {
            if index >= self.predicted_event_sequence {
                continue;
            }
            if index <= self.predicted_event_sequence - MAX_PREDICTED_EVENTS as i32 {
                continue;
            }
            let slot = index & (MAX_PS_EVENTS - 1);
            let event = ps.field_i32(&format!("events[{slot}]")).unwrap_or(0);
            let ring = index as usize & (MAX_PREDICTED_EVENTS - 1);
            if event != self.predictable_events[ring] {
                let parm = ps.field_i32(&format!("eventParms[{slot}]")).unwrap_or(0);
                self.push_predicted_player_event(ps, event, parm, server_time)?;
                self.predictable_events[ring] = event;
            }
        }
        Ok(())
    }

    fn push_predicted_player_event(
        &mut self,
        ps: &PlayerState,
        raw_event: i32,
        parm: i32,
        server_time: i32,
    ) -> Result<(), String> {
        let event_num = raw_event & !EV_EVENT_BITS;
        if event_num == EntityEvent::EV_NONE.as_i32() {
            return Ok(());
        }
        let event = EntityEvent::try_from(event_num)
            .map_err(|unknown| format!("CG_EntityEvent: unknown event {unknown}"))?;
        let mut state = player_state_to_entity_state(ps)
            .ok_or_else(|| "CG_TransitionPlayerState: invalid predicted clientNum".to_owned())?;
        set_entity_i32(&mut state, "event", raw_event);
        set_entity_i32(&mut state, "eventParm", parm);
        let position = player_state_vec3(ps, "origin").unwrap_or([0.0; 3]);
        self.presentation_events.push(PresentationEvent {
            receive_sequence: 0,
            source_entity_num: state.number,
            entity_num: state.number,
            event,
            raw_event,
            parm,
            position,
            event_only_entity: false,
            server_time,
            state,
        });
        Ok(())
    }

    /// TaystJK `EV_VOICECMD_SOUND`: the server only sends the accepted voice
    /// event.  The cgame turns that event into the teammate chat-box line; it
    /// is not a separate `chat`/`tchat` server command.
    fn vgs_chat_notice(&self, event: &PresentationEvent) -> Option<CgameNotice> {
        if event.event != EntityEvent::EV_VOICECMD_SOUND {
            return None;
        }

        let speaker = usize::try_from(event.state.field_i32("groundEntityNum")?).ok()?;
        if speaker >= MAX_CLIENTS {
            return None;
        }

        let sound = self.sound_qpath(event.parm)?;
        let description = crate::vgs::description_for_sound(&sound)?;

        let snapshot = self.current_snapshot.as_ref()?;
        let local_client = usize::try_from(snapshot.player_state.field_i32("clientNum")?).ok()?;
        if local_client >= MAX_CLIENTS {
            return None;
        }

        let speaker_info = self.configstring(CS_PLAYERS.checked_add(u16::try_from(speaker).ok()?)?)?;
        let local_info = self.configstring(CS_PLAYERS.checked_add(u16::try_from(local_client).ok()?)?)?;
        let speaker_team = info_value(speaker_info, b"t").and_then(parse_i32_ascii)?;
        let local_team = info_value(local_info, b"t").and_then(parse_i32_ascii)?;
        if speaker_team != local_team {
            return None;
        }

        let speaker_name = info_value(speaker_info, b"n")?;
        if speaker_name.is_empty() {
            return None;
        }

        let mut text = Vec::with_capacity(speaker_name.len() + description.len() + 4);
        text.extend_from_slice(speaker_name);
        text.extend_from_slice(b"^7: ");
        text.extend_from_slice(description.as_bytes());
        Some(CgameNotice::Chat { team: true, text })
    }

    pub fn presentation_event_count(&self) -> usize {
        self.presentation_events.events.len()
    }

    /// Pop one accepted event from the client receive queue in discovery order.
    /// Snapshot/playerstate parsing never performs presentation side effects;
    /// the caller drains this queue after state transitions are complete.
    pub fn pop_presentation_event(&mut self) -> Option<PresentationEvent> {
        let event = self.presentation_events.pop_front()?;
        if let Some(notice) = self.vgs_chat_notice(&event) {
            self.notices.push_back(notice);
        }
        Some(event)
    }

    /// Convenience bulk drain retained for tests/tools. The live app uses
    /// `pop_presentation_event` so no temporary Vec is allocated each frame.
    pub fn drain_presentation_events(&mut self) -> Vec<PresentationEvent> {
        let mut events = Vec::new();
        while let Some(event) = self.pop_presentation_event() {
            events.push(event);
        }
        events
    }

    pub fn drain_event_check_traces(&mut self) -> Vec<EventCheckTrace> {
        self.pending_event_traces.drain(..).collect()
    }

    pub fn gametype(&self) -> i32 {
        self.configstring(CS_SERVERINFO)
            .and_then(|info| info_value(info, b"g_gametype"))
            .and_then(parse_i32_ascii)
            .unwrap_or(0)
    }

    /// OpenJK `CS_ITEMS`: the server/game module tells cgame which item visuals
    /// are present on this level. `CG_RegisterItemVisuals` uses this bitstring
    /// during precache rather than inventing a client-side availability list.
    pub fn server_registers_item(&self, item_index: usize) -> bool {
        self.configstring(CS_ITEMS)
            .and_then(|items| items.get(item_index))
            .is_some_and(|value| *value == b'1')
    }

    /// Server-side weapon-disable rules determine what can be granted/spawned.
    /// The client should still trust snapshot/playerState weapon fields for what
    /// is actually equipped. This is exposed for OpenJK-compatible UI/precache
    /// decisions, not as a render-time visibility gate.
    pub fn weapon_disable_mask(&self) -> i32 {
        let info = match self.configstring(CS_SERVERINFO) {
            Some(info) => info,
            None => return 0,
        };
        let key: &[u8] = if matches!(self.gametype(), 3 | 4) {
            b"g_duelWeaponDisable"
        } else {
            b"g_weaponDisable"
        };
        info_value(info, key)
            .or_else(|| info_value(info, b"wdisable"))
            .and_then(parse_i32_ascii)
            .unwrap_or(0)
    }

    /// Visual subset of OpenJK `CG_NewClientInfo` with `cg_forceModel == 0`. Siege model/skin
    /// overrides are applied from the same class definitions loaded by `BG_SiegeLoadClasses`.
    pub fn client_info(
        &self,
        client_num: usize,
        siege_classes: &[SiegeClassVisual],
    ) -> Option<ClientInfo> {
        let index = usize::from(CS_PLAYERS).checked_add(client_num)?;
        let index = u16::try_from(index).ok()?;
        let configstring = self.configstring(index)?;
        if configstring.is_empty() {
            return None;
        }

        let name = info_value(configstring, b"n")
            .map(bytes_to_lossless_ascii)
            .unwrap_or_default();
        let team = info_value(configstring, b"t")
            .and_then(parse_i32_ascii)
            .unwrap_or(0);
        let model_value = info_value(configstring, b"model")
            .map(bytes_to_lossless_ascii)
            .unwrap_or_default();
        let (mut model_name, mut skin_name) = split_model_skin(&model_value);
        let siege_class = info_value(configstring, b"siegeclass")
            .map(bytes_to_lossless_ascii)
            .unwrap_or_default();
        // OpenJK CG_NewClientInfo reads the equipped saber definition names from
        // the protocol-visible `st` / `st2` info-string keys before WP_SetSaber.
        let saber_name = info_value(configstring, b"st")
            .map(bytes_to_lossless_ascii)
            .unwrap_or_default();
        let saber2_name = info_value(configstring, b"st2")
            .map(bytes_to_lossless_ascii)
            .unwrap_or_default();
        let saber_color = info_value(configstring, b"c1")
            .and_then(parse_i32_ascii)
            .unwrap_or(4);
        let saber2_color = info_value(configstring, b"c2")
            .and_then(parse_i32_ascii)
            .unwrap_or(4);

        if self.gametype() == GT_SIEGE {
            if let Some(class) = find_siege_class_visual(siege_classes, &siege_class) {
                if !class.forced_model.is_empty() {
                    model_name.clone_from(&class.forced_model);
                }
                if !class.forced_skin.is_empty() {
                    skin_name.clone_from(&class.forced_skin);
                }
            }
        }

        Some(ClientInfo {
            client_num,
            name,
            team,
            gametype: self.gametype(),
            female: info_value(configstring, b"ds")
                .is_some_and(|value| value.first() == Some(&b'f')),
            model_name,
            skin_name,
            siege_class,
            saber_name,
            saber2_name,
            saber_color,
            saber2_color,
            definition_saber_colors: false,
        })
    }

    /// OpenJK `cent->npcClient` as filled by CG_G2AnimEntModelLoad and
    /// CG_Player: the Ghoul2 model and appended skin come from
    /// `CS_MODELS + modelindex` ("models/players/<model>/model.glm*<skin>"),
    /// sabers from the `@name` configstrings at npcSaber1/npcSaber2, team is
    /// TEAM_FREE. Vehicles and non-player-model NPCs are not humanoid clients.
    pub fn npc_client_info(&self, npc: &EntityState) -> Result<ClientInfo, &'static str> {
        if field_i32(npc, "NPC_class") == CLASS_VEHICLE {
            return Err("npc-vehicle-uses-static-presenter");
        }
        let model_index = field_i32(npc, "modelindex");
        let model = self.model_qpath(model_index).ok_or("npc-model-configstring-missing")?;
        // CG_HandleAppendedSkin: the text after the last '*' is the skin.
        let (glm, skin) = match model.rsplit_once('*') {
            Some((glm, skin)) if !skin.is_empty() => (glm, skin),
            Some((glm, _)) => (glm, "default"),
            None => (model.as_str(), "default"),
        };
        let folder = glm
            .strip_prefix("models/players/")
            .and_then(|rest| rest.strip_suffix("/model.glm"))
            .filter(|folder| !folder.is_empty() && !folder.contains('/'))
            .ok_or("npc-model-not-player-glm")?;
        let saber = |field: &str| -> String {
            // npcSaber* index CS_MODELS entries of the form "@saberName".
            let index = field_i32(npc, field);
            if index == 0 {
                return "none".to_owned();
            }
            self.model_qpath(index)
                .and_then(|name| name.strip_prefix('@').map(str::to_owned))
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "none".to_owned())
        };
        let bolt_colors = field_i32(npc, "boltToPlayer");
        Ok(ClientInfo {
            client_num: usize::from(npc.number),
            name: String::new(),
            team: 0, // TEAM_FREE
            gametype: self.gametype(),
            female: false,
            model_name: folder.to_owned(),
            skin_name: skin.to_owned(),
            siege_class: String::new(),
            saber_name: saber("npcSaber1"),
            saber2_name: saber("npcSaber2"),
            saber_color: (bolt_colors & 0x07) - 1,
            saber2_color: ((bolt_colors & 0x38) >> 3) - 1,
            definition_saber_colors: bolt_colors == 0,
        })
    }

    /// OpenJK `CG_SetInitialSnapshot`, limited to the snapshot/entity responsibilities that exist
    /// in this Rust client today. Rendering/Ghoul2 side effects are deliberately not fabricated.
    pub fn set_initial_snapshot(&mut self, snapshot: &Snapshot) -> Result<(), String> {
        self.current_snapshot = Some(snapshot.clone());
        self.next_snapshot = None;
        self.execute_server_commands(snapshot.server_command_num)?;

        for entity in &snapshot.entities {
            let outcome = {
                let cent = self.entity_mut(entity.number)?;
                cent.current_state = Some(entity.clone());
                cent.next_state = None;
                cent.interpolate = false;
                cent.current_valid = true;
                reset_entity_lerp(cent, snapshot.server_time, snapshot.server_time)?;
                check_entity_event(cent, entity, snapshot.server_time)?
            };
            self.pending_event_traces.push_back(outcome.trace);
            if let Some(event) = outcome.event {
                self.presentation_events.push(event);
            }
        }
        Ok(())
    }

    /// OpenJK `CG_SetNextSnap`: copy entity nextState and decide whether interpolation is legal.
    pub fn set_next_snapshot(&mut self, snapshot: Option<&Snapshot>) -> Result<(), String> {
        self.next_snapshot = snapshot.cloned();
        let Some(snapshot) = snapshot else {
            for cent in &mut self.entities {
                cent.next_state = None;
                cent.interpolate = false;
            }
            return Ok(());
        };

        for entity in &snapshot.entities {
            let cent = self.entity_mut(entity.number)?;
            let teleport = cent
                .current_state
                .as_ref()
                .is_some_and(|current| {
                    ((field_i32(current, "eFlags") ^ field_i32(entity, "eFlags"))
                        & EF_TELEPORT_BIT)
                        != 0
                });
            cent.next_state = Some(entity.clone());
            cent.interpolate = cent.current_valid && !teleport;
        }
        Ok(())
    }

    /// OpenJK `CG_TransitionSnapshot` + `CG_TransitionEntity` state movement.
    pub fn transition_snapshot(&mut self, render_time: i32) -> Result<(), String> {
        let next = self
            .next_snapshot
            .clone()
            .ok_or_else(|| "CG_TransitionSnapshot: NULL next snapshot".to_owned())?;
        let current = self
            .current_snapshot
            .clone()
            .ok_or_else(|| "CG_TransitionSnapshot: NULL current snapshot".to_owned())?;
        if next.server_time < current.server_time {
            return Err("CG_ProcessSnapshots: server time went backwards".to_owned());
        }

        self.execute_server_commands(next.server_command_num)?;

        // OpenJK clears currentValid for every entity in the old snapshot first.
        for entity in &current.entities {
            self.entity_mut(entity.number)?.current_valid = false;
        }

        self.current_snapshot = Some(next.clone());
        self.next_snapshot = None;

        for state in &next.entities {
            let outcome = {
                let cent = self.entity_mut(state.number)?;
                let next_state = cent
                    .next_state
                    .take()
                    .unwrap_or_else(|| state.clone());
                cent.current_state = Some(next_state.clone());
                cent.current_valid = true;
                if !cent.interpolate {
                    reset_entity_lerp(cent, next.server_time, render_time)?;
                }
                cent.interpolate = false;
                cent.snapshot_time = next.server_time;
                check_entity_event(cent, &next_state, next.server_time)?
            };
            self.pending_event_traces.push_back(outcome.trace);
            if let Some(event) = outcome.event {
                self.presentation_events.push(event);
            }
        }
        Ok(())
    }

    /// `cg_entities[number].miscTime` (0 when never stamped).
    pub fn entity_misc_time(&self, number: u16) -> i32 {
        self.entities.get(usize::from(number)).map_or(0, |cent| cent.misc_time)
    }

    /// Current centity state for presentation-side systems such as custom
    /// player/NPC sound resolution.
    pub fn entity_state(&self, number: u16) -> Option<&EntityState> {
        self.entities
            .get(usize::from(number))
            .and_then(|cent| cent.current_state.as_ref())
    }

    pub fn current_snapshot(&self) -> Option<&Snapshot> {
        self.current_snapshot.as_ref()
    }

    /// OpenJK `CG_AddPacketEntities` frame interpolation + `CG_CalcEntityLerpPositions`.
    #[cfg(test)]
    pub fn present_entities(&mut self, time: i32) -> Result<Vec<PresentedEntity>, String> {
        self.present_entities_at(f64::from(time))
    }

    /// High-resolution presentation variant. Snapshot transitions/events remain
    /// on integer JKA server time, while visual interpolation may sample between
    /// integer milliseconds (important when rendering demos at very high FPS).
    pub fn present_entities_at(&mut self, time_ms: f64) -> Result<Vec<PresentedEntity>, String> {
        let current = self
            .current_snapshot
            .as_ref()
            .ok_or_else(|| "CG_AddPacketEntities: no current snapshot".to_owned())?
            .clone();
        let next = self.next_snapshot.clone();

        self.frame_interpolation = match &next {
            Some(next) if next.server_time != current.server_time => {
                ((time_ms - f64::from(current.server_time))
                    / f64::from(next.server_time - current.server_time)) as f32
            }
            _ => 0.0,
        };

        let frame_interpolation = self.frame_interpolation;
        let mut presented = Vec::with_capacity(current.entities.len());
        for state in &current.entities {
            // OpenJK skips the local player entity here because predictedPlayerEntity is added
            // separately. Demo POV is already handled from playerState in app.rs.
            let local_client = current.player_state.field_i32("clientNum").unwrap_or(-1);
            if i32::from(state.number) == local_client {
                continue;
            }
            let cent = self.entity_mut(state.number)?;
            calc_entity_lerp_positions(cent, time_ms, frame_interpolation, current.server_time, next.as_ref().map(|s| s.server_time))?;
            let current_state = cent.current_state.clone().unwrap_or_else(|| state.clone());
            let entity_type = field_i32(&current_state, "eType");
            // As in CG_AddCEntity, event-only entities have already served their transition/event
            // purpose and are not persistent scene geometry.
            if entity_type >= ET_EVENTS {
                continue;
            }
            presented.push(PresentedEntity {
                number: current_state.number,
                entity_type,
                origin: cent.lerp_origin,
                angles: cent.lerp_angles,
                state: current_state,
            });
        }
        Ok(presented)
    }

    /// Demo/follow-camera equivalent of OpenJK's predictedPlayerEntity path.
    /// The caller decides whether the local/followed body is rendered (OpenJK
    /// suppresses it in ordinary first person); this method only produces the
    /// player entity state that the shared player presenter consumes.
    #[cfg(test)]
    pub fn present_followed_player(&self, time: i32) -> Option<PresentedEntity> {
        self.present_followed_player_at(f64::from(time))
    }

    pub fn present_followed_player_at(&self, time_ms: f64) -> Option<PresentedEntity> {
        let current = self.current_snapshot.as_ref()?;
        let next = self.next_snapshot.as_ref();
        let discontinuity = next.is_some_and(|next| snapshot_discontinuity(current, next));
        let alpha = if discontinuity {
            0.0
        } else {
            next.filter(|next| next.server_time > current.server_time)
                .map(|next| {
                    ((time_ms - f64::from(current.server_time))
                        / f64::from(next.server_time - current.server_time)) as f32
                })
                .unwrap_or(0.0)
                .clamp(0.0, 1.0)
        };
        presented_player_state(
            &current.player_state,
            next.map(|snapshot| &snapshot.player_state),
            alpha,
        )
    }
    pub fn summarize_entities(&self) -> EntityTypeSummary {
        let mut summary = EntityTypeSummary::default();
        let Some(snapshot) = self.current_snapshot.as_ref() else {
            return summary;
        };
        for entity in &snapshot.entities {
            summary.total += 1;
            match field_i32(entity, "eType") {
                ET_PLAYER => summary.players += 1,
                ET_NPC => summary.npcs += 1,
                ET_MOVER => summary.movers += 1,
                ET_MISSILE => summary.missiles += 1,
                ET_ITEM => summary.items += 1,
                e if e >= ET_EVENTS => summary.event_entities += 1,
                _ => summary.other += 1,
            }
        }
        summary
    }

    fn entity_mut(&mut self, number: u16) -> Result<&mut CEntity, String> {
        self.entities
            .get_mut(usize::from(number))
            .ok_or_else(|| format!("entity number {} outside MAX_GENTITIES", number))
    }

    fn execute_server_commands(&mut self, target_sequence: i32) -> Result<(), String> {
        while self
            .queued_server_commands
            .front()
            .is_some_and(|command| command.sequence <= target_sequence)
        {
            let command = self.queued_server_commands.pop_front().expect("front checked");
            if command.sequence <= self.executed_server_command {
                continue;
            }
            self.execute_server_command(&command)?;
            self.executed_server_command = command.sequence;
        }
        Ok(())
    }

    fn execute_server_command(&mut self, command: &ServerCommand) -> Result<(), String> {
        // CL_GetServerCommand reassembles bcs0/bcs1/bcs2 into one `cs`.
        use jka_protocol::commands::BigConfigOutcome;
        let assembled;
        let text: &[u8] = match self.big_config.feed(&command.text) {
            BigConfigOutcome::PassThrough => &command.text,
            BigConfigOutcome::Pending => return Ok(()),
            BigConfigOutcome::Complete(text) => {
                assembled = text;
                &assembled
            }
            BigConfigOutcome::Overflow => {
                return Err(format!("bcs exceeded BIG_INFO_STRING at server command #{}", command.sequence))
            }
        };
        // OpenJK routes this through CG_ServerCommand. `cs` updates must not
        // become visible until the snapshot's serverCommandSequence reaches them.
        // JKA network strings are byte strings, not UTF-8. Chat and names
        // commonly contain legacy high bytes. Only the command/index are ASCII.
        let parts = server_command_tokens(text);
        let name = parts.first().copied().unwrap_or_default();
        let arg = |index: usize| parts.get(index).copied().unwrap_or_default().to_vec();
        // CG_RemoveChatEscapeChar
        let strip_escape = |mut text: Vec<u8>| {
            text.retain(|&byte| byte != 0x19);
            text
        };
        if name.eq_ignore_ascii_case(b"print") {
            self.notices.push_back(CgameNotice::Print(arg(1)));
            return Ok(());
        }
        if name.eq_ignore_ascii_case(b"cp") {
            self.notices.push_back(CgameNotice::CenterPrint(arg(1)));
            return Ok(());
        }
        if name.eq_ignore_ascii_case(b"chat") || name.eq_ignore_ascii_case(b"tchat") {
            let team = name.eq_ignore_ascii_case(b"tchat");
            self.notices.push_back(CgameNotice::Chat { team, text: strip_escape(arg(1)) });
            return Ok(());
        }
        if name.eq_ignore_ascii_case(b"lchat") || name.eq_ignore_ascii_case(b"ltchat") {
            if parts.len() < 4 {
                return Ok(());
            }
            // "%s^7<%s> ^%s%s": name, location, colour, message.
            let text = [arg(1), b"^7<".to_vec(), arg(2), b"> ^".to_vec(), arg(3), arg(4)].concat();
            let team = name.eq_ignore_ascii_case(b"ltchat");
            self.notices.push_back(CgameNotice::Chat { team, text: strip_escape(text) });
            return Ok(());
        }
        if name.eq_ignore_ascii_case(b"scores") {
            // OpenJK CG_ParseScores / g_cmds.c DeathmatchScoreboardMessage:
            // argv(1) count, argv(2..=3) red/blue totals, then SCORE_OFFSET
            // (=14) integer fields for each client.
            const SCORE_OFFSET: usize = 14;
            const MAX_SCORE_CLIENTS: usize = 32;
            let integer = |index: usize| {
                parts.get(index).and_then(|value| parse_i32_ascii(value)).unwrap_or(0)
            };
            let count = integer(1).clamp(0, MAX_SCORE_CLIENTS as i32) as usize;
            let team_scores = [integer(2), integer(3)];
            let mut entries = Vec::with_capacity(count);
            for score_index in 0..count {
                let base = 4 + score_index * SCORE_OFFSET;
                if parts.len() < base + SCORE_OFFSET {
                    break;
                }
                let client = integer(base).clamp(0, (MAX_SCORE_CLIENTS - 1) as i32);
                let player_info = u16::try_from(client)
                    .ok()
                    .and_then(|client| CS_PLAYERS.checked_add(client))
                    .and_then(|index| self.configstring(index));
                let name = player_info
                    .and_then(|info| info_value(info, b"n"))
                    .map(bytes_to_lossless_ascii)
                    .unwrap_or_else(|| format!("CLIENT {client}"));
                let team = player_info
                    .and_then(|info| info_value(info, b"t"))
                    .and_then(parse_i32_ascii)
                    .unwrap_or(0);
                entries.push(ScoreEntry {
                    client,
                    score: integer(base + 1),
                    ping: integer(base + 2),
                    time: integer(base + 3),
                    score_flags: integer(base + 4),
                    powerups: integer(base + 5),
                    accuracy: integer(base + 6),
                    impressive_count: integer(base + 7),
                    excellent_count: integer(base + 8),
                    gauntlet_count: integer(base + 9),
                    defend_count: integer(base + 10),
                    assist_count: integer(base + 11),
                    perfect: integer(base + 12),
                    captures: integer(base + 13),
                    name,
                    team,
                });
            }
            self.notices.push_back(CgameNotice::Scores { team_scores, entries });
            return Ok(());
        }
        if name == b"map_restart" {
            self.notices.push_back(CgameNotice::MapRestart);
            return Ok(());
        }
        if name.eq_ignore_ascii_case(b"ircg") {
            // OpenJK CG_RestoreClientGhoul_f: ircg <client> <body> <weapon> <side>.
            // Keep this byte/token parser aligned with the protocol command; do
            // not reconstruct corpse equipment from snapshot guesses.
            let source_client = parts.get(1).and_then(|v| parse_i32_ascii(v));
            let body_entity = parts.get(2).and_then(|v| parse_i32_ascii(v));
            let known_weapon = parts.get(3).and_then(|v| parse_i32_ascii(v));
            let light_side = parts.get(4).and_then(|v| parse_i32_ascii(v)).unwrap_or(0) != 0;
            let (Some(source_client), Some(body_entity), Some(known_weapon)) =
                (source_client, body_entity, known_weapon)
            else {
                return Ok(());
            };
            if !(0..MAX_CLIENTS as i32).contains(&source_client)
                || !(0..MAX_GENTITIES as i32).contains(&body_entity)
            {
                return Ok(());
            }
            self.pending_ghoul2_commands.push_back(Ghoul2ServerCommand::BodyQueueCopy {
                source_client: source_client as u16,
                body_entity: body_entity as u16,
                known_weapon,
                light_side,
            });
            return Ok(());
        }
        if name.eq_ignore_ascii_case(b"rcg") {
            // Same OpenJK handler without a body-copy target. It still resets
            // clent->weapon / clent->ghoul2weapon so respawn reattaches cleanly.
            let Some(source_client) = parts.get(1).and_then(|v| parse_i32_ascii(v)) else {
                return Ok(());
            };
            if !(0..MAX_CLIENTS as i32).contains(&source_client) {
                return Ok(());
            }
            self.pending_ghoul2_commands.push_back(Ghoul2ServerCommand::RestoreClient {
                source_client: source_client as u16,
            });
            return Ok(());
        }
        if !name.eq_ignore_ascii_case(b"cs") {
            return Ok(());
        }
        let index_text = parts
            .get(1)
            .ok_or_else(|| format!("malformed cs server command #{}", command.sequence))?;
        let index = parse_i32_ascii(index_text)
            .filter(|index| (0..i32::from(jka_protocol::gamestate::MAX_CONFIGSTRINGS)).contains(index))
            .ok_or_else(|| format!("invalid cs index in server command #{}", command.sequence))? as u16;
        // CL_ConfigstringModified consumes Cmd_ArgsFrom(2): join tokens with a
        // single space while preserving every byte inside quoted arguments.
        let value = parts.get(2..).unwrap_or_default().join(&b' ');
        self.configstrings.insert(index, value);
        Ok(())
    }
}

fn info_value<'a>(info: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let mut fields = info.split(|&byte| byte == b'\\');
    if info.first() == Some(&b'\\') {
        let _ = fields.next();
    }
    loop {
        let candidate = fields.next()?;
        let value = fields.next()?;
        if candidate.eq_ignore_ascii_case(key) {
            return Some(value);
        }
    }
}

fn configstring_resource(
    configstrings: &BTreeMap<u16, Vec<u8>>,
    base: u16,
    count: i32,
    index: i32,
) -> Option<String> {
    if index <= 0 || index >= count {
        return None;
    }
    let key = u16::try_from(i32::from(base) + index).ok()?;
    let value = configstrings.get(&key)?;
    if value.is_empty() {
        return None;
    }
    Some(bytes_to_lossless_ascii(value).replace('\\', "/"))
}

fn parse_i32_ascii(value: &[u8]) -> Option<i32> {
    std::str::from_utf8(value).ok()?.parse().ok()
}

pub fn bytes_to_lossless_ascii(value: &[u8]) -> String {
    value.iter().map(|&byte| byte as char).collect()
}

fn split_model_skin(value: &str) -> (String, String) {
    if let Some((model, skin)) = value.split_once('/') {
        (model.to_owned(), skin.to_owned())
    } else {
        (value.to_owned(), "default".to_owned())
    }
}


/// DinurdoJK compact forced-player-model setting. This deliberately compresses
/// TaystJK's cg_forceModel/cg_forceAllyModel/cg_forceEnemyModel UX into one cvar:
/// `0` = off, `model[/skin]` = force every *other* player to one model,
/// `allyModel,enemyModel` = use separate same-team/enemy models.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForcedPlayerModels {
    pub ally: String,
    pub enemy: String,
    /// Preserve whether the user selected the explicit ally/enemy form even
    /// when both sides currently happen to name the same model.
    pub split: bool,
}

impl ForcedPlayerModels {
    pub fn parse(value: &str) -> Result<Option<Self>, String> {
        let value = value.trim();
        if value.is_empty() || value == "0" {
            return Ok(None);
        }
        if value.chars().any(|ch| ch == '\\' || ch == '\n' || ch == '\r' || ch == '"') {
            return Err("cg_forceModel: model names may not contain quotes, backslashes, or newlines".to_owned());
        }
        let mut parts = value.split(',').map(str::trim);
        let first = parts.next().unwrap_or_default();
        let second = parts.next();
        if parts.next().is_some() {
            return Err("cg_forceModel: expected 0, model[/skin], or allyModel,enemyModel".to_owned());
        }
        if first.is_empty() || second.is_some_and(str::is_empty) {
            return Err("cg_forceModel: forced model names may not be empty".to_owned());
        }
        for spec in [Some(first), second].into_iter().flatten() {
            let (model, skin) = spec.split_once('/').unwrap_or((spec, "default"));
            if model.trim().is_empty() || skin.trim().is_empty() || skin.contains('/') {
                return Err("cg_forceModel: expected model or model/skin on each side".to_owned());
            }
        }
        Ok(Some(match second {
            Some(enemy) => Self {
                ally: first.to_owned(),
                enemy: enemy.to_owned(),
                split: true,
            },
            None => Self {
                ally: first.to_owned(),
                enemy: first.to_owned(),
                split: false,
            },
        }))
    }

    pub fn serialize(&self) -> String {
        if self.split {
            format!("{},{}", self.ally, self.enemy)
        } else {
            self.ally.clone()
        }
    }

    /// Apply TaystJK-style ally/enemy model forcing at the client-presentation
    /// layer without mutating the protocol CS_PLAYERS identity. The local/viewed
    /// player keeps their own model; in non-team modes every other player is an
    /// enemy. In team modes, TEAM_RED/TEAM_BLUE equality determines allies.
    pub fn apply_to(&self, info: &mut ClientInfo, viewer: Option<&ClientInfo>) {
        if viewer.is_some_and(|viewer| viewer.client_num == info.client_num) {
            return;
        }
        let ally = viewer.is_some_and(|viewer| {
            info.gametype >= 6
                && matches!(viewer.team, 1 | 2)
                && viewer.team == info.team
        });
        let forced = if ally { &self.ally } else { &self.enemy };
        let (model_name, skin_name) = if let Some((model, skin)) = forced.split_once('/') {
            (model.trim().to_owned(), skin.trim().to_owned())
        } else {
            // TaystJK keeps the target's team skin when cg_forceModel is active,
            // then the ally/enemy override replaces only modelName. Preserve that
            // useful behavior in team games; the compact arbitrary-model form uses
            // default outside team modes because it has no separate base model skin.
            let skin = if info.gametype >= 6 {
                info.skin_name.clone()
            } else {
                "default".to_owned()
            };
            (forced.trim().to_owned(), skin)
        };
        info.model_name = model_name;
        info.skin_name = skin_name;
    }
}

/// Bounded byte-oriented Cmd_TokenizeString2 semantics, including comments
/// and quoted strings. Like OpenJK, backslash does not escape a quote.
fn server_command_tokens(mut text: &[u8]) -> Vec<&[u8]> {
    text = &text[..text.iter().position(|&byte| byte == 0).unwrap_or(text.len())];
    let mut tokens = Vec::new();
    while !text.is_empty() && tokens.len() < 1024 {
        let whitespace = text.iter().take_while(|&&byte| byte <= b' ').count();
        text = &text[whitespace..];
        if text.is_empty() || text.starts_with(b"//") { break; }
        if text.starts_with(b"/*") {
            let Some(end) = text.windows(2).position(|pair| pair == b"*/") else { break; };
            text = &text[end + 2..];
            continue;
        }
        if text[0] == b'"' {
            text = &text[1..];
            let end = text.iter().position(|&byte| byte == b'"').unwrap_or(text.len());
            tokens.push(&text[..end]);
            text = &text[(end + 1).min(text.len())..];
        } else {
            let end = (0..text.len()).find(|&index| {
                text[index] <= b' ' || text[index] == b'"'
                    || text[index..].starts_with(b"//") || text[index..].starts_with(b"/*")
            }).unwrap_or(text.len());
            tokens.push(&text[..end]);
            text = &text[end..];
        }
    }
    tokens
}

fn reset_entity_lerp(
    cent: &mut CEntity,
    server_time: i32,
    render_time: i32,
) -> Result<(), String> {
    // OpenJK CG_ResetEntity allows an old entity slot to fire the same event
    // again after its EVENT_VALID_MSEC window has expired.
    if cent.snapshot_time < render_time.saturating_sub(EVENT_VALID_MSEC) {
        cent.previous_event = 0;
    }
    let state = cent
        .current_state
        .as_ref()
        .ok_or_else(|| "CG_ResetEntity without currentState".to_owned())?;
    cent.lerp_origin = entity_vec3(state, "origin").unwrap_or_else(|| trajectory(state, "pos").base);
    cent.lerp_angles = entity_vec3(state, "angles").unwrap_or_else(|| trajectory(state, "apos").base);
    cent.snapshot_time = server_time;
    Ok(())
}

fn check_entity_event(
    cent: &mut CEntity,
    state: &EntityState,
    server_time: i32,
) -> Result<EventCheckOutcome, String> {
    let entity_type = field_i32(state, "eType");
    let previous_event = cent.previous_event;
    let event_only_entity = entity_type > ET_EVENTS;
    let raw_event = if event_only_entity {
        entity_type - ET_EVENTS
    } else {
        field_i32(state, "event")
    };
    let event = raw_event & !EV_EVENT_BITS;
    let entity_num = if event_only_entity && (field_i32(state, "eFlags") & EF_PLAYER_EVENT) != 0 {
        u16::try_from(field_i32(state, "otherEntityNum")).unwrap_or(state.number)
    } else {
        state.number
    };

    let trace = |disposition| EventCheckTrace {
        source_entity_num: state.number,
        entity_num,
        entity_type,
        server_time,
        raw_event,
        event,
        previous_event,
        event_only_entity,
        disposition,
    };

    if event_only_entity {
        // OpenJK event-only entities fire exactly once for that centity slot.
        if cent.previous_event != 0 {
            return Ok(EventCheckOutcome {
                event: None,
                trace: trace(EventCheckDisposition::Duplicate),
            });
        }
        cent.previous_event = 1;
    } else {
        if raw_event == 0 && cent.previous_event == 0 {
            return Ok(EventCheckOutcome {
                event: None,
                trace: trace(EventCheckDisposition::NoEvent),
            });
        }
        if raw_event == cent.previous_event {
            return Ok(EventCheckOutcome {
                event: None,
                trace: trace(EventCheckDisposition::Duplicate),
            });
        }
        cent.previous_event = raw_event;
    }

    if event == EntityEvent::EV_NONE.as_i32() {
        return Ok(EventCheckOutcome {
            event: None,
            trace: trace(EventCheckDisposition::Zero),
        });
    }
    let event = EntityEvent::try_from(event)
        .map_err(|unknown| format!("CG_EntityEvent: unknown event {unknown}"))?;
    // CG_EntityEvent EV_ITEM_RESPAWN: `cent->miscTime = cg.time`.
    if event == EntityEvent::EV_ITEM_RESPAWN && !event_only_entity {
        cent.misc_time = server_time;
    }
    let position = evaluate_trajectory(trajectory(state, "pos"), server_time)?;
    Ok(EventCheckOutcome {
        event: Some(PresentationEvent {
            receive_sequence: 0,
            source_entity_num: state.number,
            entity_num,
            event,
            raw_event,
            parm: field_i32(state, "eventParm"),
            position,
            event_only_entity,
            server_time,
            state: state.clone(),
        }),
        trace: trace(EventCheckDisposition::Accepted),
    })
}

fn calc_entity_lerp_positions(
    cent: &mut CEntity,
    time_ms: f64,
    frame_interpolation: f32,
    current_server_time: i32,
    next_server_time: Option<i32>,
) -> Result<(), String> {
    let current = cent
        .current_state
        .as_ref()
        .ok_or_else(|| "CG_CalcEntityLerpPositions without currentState".to_owned())?;
    let current_pos = trajectory(current, "pos");
    let current_apos = trajectory(current, "apos");

    if cent.interpolate && current_pos.kind == TR_INTERPOLATE {
        interpolate_entity_position(
            cent,
            frame_interpolation,
            current_server_time,
            next_server_time.ok_or_else(|| "interpolating entity without next snapshot".to_owned())?,
        )?;
        return Ok(());
    }

    let number = i32::from(current.number);
    let entity_type = field_i32(current, "eType");
    let is_remote_client = number < 32;

    // OpenJK's stock cg_smoothClients default is 0. In that mode
    // CG_CalcEntityLerpPositions forcibly treats remote clients and NPCs as
    // TR_INTERPOLATE before the normal trajectory branches. This prevents
    // extrapolating player positions when a newer snapshot exists.
    if cent.interpolate && (is_remote_client || entity_type == ET_NPC) {
        interpolate_entity_position(
            cent,
            frame_interpolation,
            current_server_time,
            next_server_time.ok_or_else(|| "interpolating entity without next snapshot".to_owned())?,
        )?;
        return Ok(());
    }

    let is_vehicle_npc = entity_type == ET_NPC && field_i32(current, "NPC_class") == CLASS_VEHICLE;
    if cent.interpolate
        && (current_pos.kind == TR_LINEAR_STOP && (is_remote_client || entity_type == ET_NPC)
            || is_vehicle_npc)
    {
        interpolate_entity_position(
            cent,
            frame_interpolation,
            current_server_time,
            next_server_time.ok_or_else(|| "interpolating entity without next snapshot".to_owned())?,
        )?;
        return Ok(());
    }

    cent.lerp_origin = evaluate_trajectory_at(current_pos, time_ms)?;
    cent.lerp_angles = evaluate_trajectory_at(current_apos, time_ms)?;
    Ok(())
}

fn interpolate_entity_position(
    cent: &mut CEntity,
    fraction: f32,
    current_server_time: i32,
    next_server_time: i32,
) -> Result<(), String> {
    let current = cent
        .current_state
        .as_ref()
        .ok_or_else(|| "CG_InterpolateEntityPosition without currentState".to_owned())?;
    let next = cent
        .next_state
        .as_ref()
        .ok_or_else(|| "CG_InterpolateEntityPosition without nextState".to_owned())?;
    let current_origin = evaluate_trajectory(trajectory(current, "pos"), current_server_time)?;
    let next_origin = evaluate_trajectory(trajectory(next, "pos"), next_server_time)?;
    let current_angles = evaluate_trajectory(trajectory(current, "apos"), current_server_time)?;
    let next_angles = evaluate_trajectory(trajectory(next, "apos"), next_server_time)?;
    for axis in 0..3 {
        cent.lerp_origin[axis] = current_origin[axis] + fraction * (next_origin[axis] - current_origin[axis]);
        cent.lerp_angles[axis] = lerp_angle(current_angles[axis], next_angles[axis], fraction);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct Trajectory {
    kind: i32,
    time: i32,
    duration: i32,
    base: [f32; 3],
    delta: [f32; 3],
}

fn trajectory(entity: &EntityState, prefix: &str) -> Trajectory {
    Trajectory {
        kind: field_i32(entity, &format!("{prefix}.trType")),
        time: field_i32(entity, &format!("{prefix}.trTime")),
        duration: field_i32(entity, &format!("{prefix}.trDuration")),
        base: entity_vec3(entity, &format!("{prefix}.trBase")).unwrap_or([0.0; 3]),
        delta: entity_vec3(entity, &format!("{prefix}.trDelta")).unwrap_or([0.0; 3]),
    }
}

fn evaluate_trajectory(tr: Trajectory, at_time: i32) -> Result<[f32; 3], String> {
    evaluate_trajectory_at(tr, f64::from(at_time))
}

fn evaluate_trajectory_at(tr: Trajectory, mut at_time_ms: f64) -> Result<[f32; 3], String> {
    let mut result = tr.base;
    let tr_time = f64::from(tr.time);
    let tr_duration = f64::from(tr.duration);
    match tr.kind {
        TR_STATIONARY | TR_INTERPOLATE => {}
        TR_LINEAR => {
            let delta_time = ((at_time_ms - tr_time) * 0.001) as f32;
            vector_ma(&mut result, delta_time, tr.delta);
        }
        TR_SINE => {
            if tr.duration == 0 {
                return Err("BG_EvaluateTrajectory: TR_SINE with zero duration".to_owned());
            }
            let delta_time = ((at_time_ms - tr_time) / tr_duration) as f32;
            let phase = (delta_time * std::f32::consts::TAU).sin();
            vector_ma(&mut result, phase, tr.delta);
        }
        TR_LINEAR_STOP => {
            let stop_time = f64::from(tr.time.saturating_add(tr.duration));
            if at_time_ms > stop_time {
                at_time_ms = stop_time;
            }
            let delta_time = (((at_time_ms - tr_time) * 0.001) as f32).max(0.0);
            vector_ma(&mut result, delta_time, tr.delta);
        }
        TR_NONLINEAR_STOP => {
            let stop_time = f64::from(tr.time.saturating_add(tr.duration));
            if at_time_ms > stop_time {
                at_time_ms = stop_time;
            }
            let elapsed = at_time_ms - tr_time;
            let delta_time = if elapsed > tr_duration || elapsed <= 0.0 || tr.duration == 0 {
                0.0
            } else {
                let degrees = 90.0 - 90.0 * elapsed as f32 / tr.duration as f32;
                tr.duration as f32 * 0.001 * degrees.to_radians().cos()
            };
            vector_ma(&mut result, delta_time, tr.delta);
        }
        TR_GRAVITY => {
            let delta_time = ((at_time_ms - tr_time) * 0.001) as f32;
            vector_ma(&mut result, delta_time, tr.delta);
            result[2] -= 0.5 * DEFAULT_GRAVITY * delta_time * delta_time;
        }
        other => return Err(format!("BG_EvaluateTrajectory: unknown trType {other}")),
    }
    Ok(result)
}

fn vector_ma(base: &mut [f32; 3], scale: f32, delta: [f32; 3]) {
    for axis in 0..3 {
        base[axis] += scale * delta[axis];
    }
}

fn lerp_angle(from: f32, to: f32, fraction: f32) -> f32 {
    let mut delta = to - from;
    if delta > 180.0 {
        delta -= 360.0;
    }
    if delta < -180.0 {
        delta += 360.0;
    }
    from + fraction * delta
}

/// Presentation-relevant port of OpenJK `BG_PlayerStateToEntityState` from
/// codemp/game/bg_misc.c. Predictable-event ring consumption is intentionally
/// left to the later CG_CheckEvents port because protocol PlayerState does not
/// carry cgame's local `entityEventSequence` cursor.
/// Local/offline equivalent of the network player bridge. The native
/// `PlayerEntityView` was produced by OpenJK `BG_PlayerStateToEntityState`;
/// this function only maps that already-computed state into the protocol
/// EntityState container consumed by the shared player presenter.
pub fn presented_openjk_player(view: PlayerEntityView) -> Option<PresentedEntity> {
    let number = u16::try_from(view.number).ok()?;
    let mut state = EntityState { number, fields: [0; ENTITY_FIELDS.len()] };
    set_entity_i32(&mut state, "eType", view.entity_type);
    set_entity_i32(&mut state, "pos.trType", TR_INTERPOLATE);
    set_entity_vec3(&mut state, "pos.trBase", view.origin);
    set_entity_vec3(&mut state, "pos.trDelta", view.velocity);
    set_entity_i32(&mut state, "apos.trType", TR_INTERPOLATE);
    set_entity_vec3(&mut state, "apos.trBase", view.angles);
    set_entity_i32(&mut state, "trickedentindex", view.trickedentindex);
    set_entity_i32(&mut state, "trickedentindex2", view.trickedentindex2);
    set_entity_i32(&mut state, "trickedentindex3", view.trickedentindex3);
    set_entity_i32(&mut state, "trickedentindex4", view.trickedentindex4);
    set_entity_i32(&mut state, "forceFrame", view.force_frame);
    set_entity_i32(&mut state, "emplacedOwner", view.emplaced_owner);
    set_entity_f32(&mut state, "speed", view.speed);
    set_entity_i32(&mut state, "genericenemyindex", view.generic_enemy_index);
    set_entity_i32(&mut state, "activeForcePass", view.active_force_pass);
    set_entity_f32(&mut state, "angles2[1]", view.movement_dir as f32);
    set_entity_i32(&mut state, "legsAnim", view.legs_anim);
    set_entity_i32(&mut state, "torsoAnim", view.torso_anim);
    set_entity_i32(&mut state, "legsFlip", view.legs_flip);
    set_entity_i32(&mut state, "torsoFlip", view.torso_flip);
    set_entity_i32(&mut state, "clientNum", view.client_num);
    set_entity_i32(&mut state, "eFlags", view.e_flags);
    set_entity_i32(&mut state, "eFlags2", view.e_flags2);
    set_entity_i32(&mut state, "saberInFlight", view.saber_in_flight);
    set_entity_i32(&mut state, "saberEntityNum", view.saber_entity_num);
    set_entity_i32(&mut state, "saberMove", view.saber_move);
    set_entity_i32(&mut state, "forcePowersActive", view.force_powers_active);
    set_entity_i32(&mut state, "bolt1", view.bolt1);
    set_entity_i32(&mut state, "otherEntityNum2", view.other_entity_num2);
    set_entity_i32(&mut state, "saberHolstered", view.saber_holstered);
    set_entity_i32(&mut state, "event", view.event);
    set_entity_i32(&mut state, "eventParm", view.event_parm);
    set_entity_i32(&mut state, "weapon", view.weapon);
    set_entity_i32(&mut state, "groundEntityNum", view.ground_entity_num);
    set_entity_i32(&mut state, "powerups", view.powerups);
    set_entity_i32(&mut state, "loopSound", view.loop_sound);
    set_entity_i32(&mut state, "generic1", view.generic1);
    set_entity_i32(&mut state, "modelindex2", view.modelindex2);
    set_entity_i32(&mut state, "constantLight", view.constant_light);
    set_entity_vec3(&mut state, "origin2", view.origin2);
    set_entity_i32(&mut state, "isJediMaster", view.is_jedi_master);
    set_entity_i32(&mut state, "time2", view.time2);
    set_entity_i32(&mut state, "fireflag", view.fireflag);
    set_entity_i32(&mut state, "heldByClient", view.held_by_client);
    set_entity_i32(&mut state, "ragAttach", view.rag_attach);
    set_entity_i32(&mut state, "iModelScale", view.model_scale);
    set_entity_i32(&mut state, "brokenLimbs", view.broken_limbs);
    set_entity_i32(&mut state, "hasLookTarget", view.has_look_target);
    set_entity_i32(&mut state, "lookTarget", view.look_target);
    for index in 0..4 {
        set_entity_i32(&mut state, &format!("customRGBA[{index}]"), view.custom_rgba[index]);
    }
    set_entity_i32(&mut state, "m_iVehicleNum", view.vehicle_num);
    Some(PresentedEntity {
        number,
        entity_type: view.entity_type,
        origin: view.origin,
        angles: view.angles,
        state,
    })
}

pub fn player_state_to_entity_state(ps: &PlayerState) -> Option<EntityState> {
    let client_num = ps.field_i32("clientNum")?;
    let number = u16::try_from(client_num).ok()?;
    let mut state = EntityState {
        number,
        fields: [0; ENTITY_FIELDS.len()],
    };

    let pm_type = ps.field_i32("pm_type").unwrap_or(0);
    let health = ps.stats[0];
    let entity_type = if pm_type == PM_INTERMISSION
        || pm_type == PM_SPECTATOR
        || health <= GIB_HEALTH
    {
        ET_INVISIBLE
    } else {
        ET_PLAYER
    };
    set_entity_i32(&mut state, "eType", entity_type);
    set_entity_i32(&mut state, "pos.trType", TR_INTERPOLATE);
    copy_player_vec3(ps, &mut state, "origin", "pos.trBase");
    copy_player_vec3(ps, &mut state, "velocity", "pos.trDelta");
    set_entity_i32(&mut state, "apos.trType", TR_INTERPOLATE);
    copy_player_vec3(ps, &mut state, "viewangles", "apos.trBase");

    copy_player_i32(ps, &mut state, "fd.forceMindtrickTargetIndex", "trickedentindex");
    copy_player_i32(ps, &mut state, "fd.forceMindtrickTargetIndex2", "trickedentindex2");
    copy_player_i32(ps, &mut state, "fd.forceMindtrickTargetIndex3", "trickedentindex3");
    copy_player_i32(ps, &mut state, "fd.forceMindtrickTargetIndex4", "trickedentindex4");
    copy_player_i32(ps, &mut state, "saberLockFrame", "forceFrame");
    copy_player_i32(ps, &mut state, "electrifyTime", "emplacedOwner");
    copy_player_f32(ps, &mut state, "speed", "speed");
    copy_player_i32(ps, &mut state, "genericEnemyIndex", "genericenemyindex");
    copy_player_i32(ps, &mut state, "activeForcePass", "activeForcePass");
    if let Some(movement_dir) = ps.field_i32("movementDir") {
        set_entity_f32(&mut state, "angles2[1]", movement_dir as f32);
    }
    copy_player_i32(ps, &mut state, "legsAnim", "legsAnim");
    copy_player_i32(ps, &mut state, "torsoAnim", "torsoAnim");
    copy_player_i32(ps, &mut state, "legsFlip", "legsFlip");
    copy_player_i32(ps, &mut state, "torsoFlip", "torsoFlip");
    set_entity_i32(&mut state, "clientNum", client_num);

    let mut eflags = ps.field_i32("eFlags").unwrap_or(0);
    if ps.field_i32("genericEnemyIndex").unwrap_or(-1) != -1 {
        eflags |= EF_SEEKERDRONE;
    }
    if health <= 0 {
        eflags |= EF_DEAD;
    } else {
        eflags &= !EF_DEAD;
    }
    set_entity_i32(&mut state, "eFlags", eflags);
    copy_player_i32(ps, &mut state, "eFlags2", "eFlags2");

    copy_player_i32(ps, &mut state, "saberInFlight", "saberInFlight");
    copy_player_i32(ps, &mut state, "saberEntityNum", "saberEntityNum");
    copy_player_i32(ps, &mut state, "saberMove", "saberMove");
    copy_player_i32(ps, &mut state, "fd.forcePowersActive", "forcePowersActive");
    set_entity_i32(
        &mut state,
        "bolt1",
        i32::from(ps.field_i32("duelInProgress").unwrap_or(0) != 0),
    );
    copy_player_i32(ps, &mut state, "emplacedIndex", "otherEntityNum2");
    copy_player_i32(ps, &mut state, "saberHolstered", "saberHolstered");

    if ps.field_i32("externalEvent").unwrap_or(0) != 0 {
        copy_player_i32(ps, &mut state, "externalEvent", "event");
        copy_player_i32(ps, &mut state, "externalEventParm", "eventParm");
    }

    copy_player_i32(ps, &mut state, "weapon", "weapon");
    copy_player_i32(ps, &mut state, "groundEntityNum", "groundEntityNum");
    let mut powerups = 0i32;
    for (index, &expiry) in ps.powerups.iter().enumerate() {
        if expiry != 0 {
            powerups |= 1i32 << index;
        }
    }
    set_entity_i32(&mut state, "powerups", powerups);
    copy_player_i32(ps, &mut state, "loopSound", "loopSound");
    copy_player_i32(ps, &mut state, "generic1", "generic1");
    copy_player_i32(ps, &mut state, "weaponstate", "modelindex2");
    copy_player_i32(ps, &mut state, "weaponChargeTime", "constantLight");
    copy_player_vec3(ps, &mut state, "lastHitLoc", "origin2");
    copy_player_i32(ps, &mut state, "isJediMaster", "isJediMaster");
    copy_player_i32(ps, &mut state, "holocronBits", "time2");
    copy_player_i32(ps, &mut state, "fd.saberAnimLevel", "fireflag");
    copy_player_i32(ps, &mut state, "heldByClient", "heldByClient");
    copy_player_i32(ps, &mut state, "ragAttach", "ragAttach");
    copy_player_i32(ps, &mut state, "iModelScale", "iModelScale");
    copy_player_i32(ps, &mut state, "brokenLimbs", "brokenLimbs");
    copy_player_i32(ps, &mut state, "hasLookTarget", "hasLookTarget");
    copy_player_i32(ps, &mut state, "lookTarget", "lookTarget");
    for index in 0..4 {
        copy_player_i32(
            ps,
            &mut state,
            &format!("customRGBA[{index}]"),
            &format!("customRGBA[{index}]"),
        );
    }
    copy_player_i32(ps, &mut state, "m_iVehicleNum", "m_iVehicleNum");
    Some(state)
}

/// cg.predictedPlayerEntity: CG_PlayerStateToEntityState of the predicted state.
pub fn presented_player_state_entity(ps: &PlayerState) -> Option<PresentedEntity> {
    presented_player_state(ps, None, 0.0)
}

fn presented_player_state(
    current: &PlayerState,
    next: Option<&PlayerState>,
    alpha: f32,
) -> Option<PresentedEntity> {
    // OpenJK CG_AddCEntity: the locally predicted free spectator is not
    // presented at all when PERS_TEAM == TEAM_SPECTATOR. Follow mode is
    // unaffected because SpectatorClientEndFrame copies the followed player's
    // playerState (including its non-spectator PERS_TEAM) and adds PMF_FOLLOW.
    if current.persistant[PERS_TEAM] == TEAM_SPECTATOR {
        return None;
    }
    let mut state = player_state_to_entity_state(current)?;
    let mut origin = player_state_vec3(current, "origin")?;
    let mut angles = player_state_vec3(current, "viewangles")?;
    if let Some(next) = next {
        if next.field_i32("clientNum") == current.field_i32("clientNum") {
            if let Some(next_origin) = player_state_vec3(next, "origin") {
                for axis in 0..3 {
                    origin[axis] += (next_origin[axis] - origin[axis]) * alpha;
                }
            }
            if let Some(next_angles) = player_state_vec3(next, "viewangles") {
                for axis in 0..3 {
                    angles[axis] = lerp_angle(angles[axis], next_angles[axis], alpha);
                }
            }
        }
    }
    set_entity_vec3(&mut state, "pos.trBase", origin);
    set_entity_vec3(&mut state, "apos.trBase", angles);
    Some(PresentedEntity {
        number: state.number,
        entity_type: field_i32(&state, "eType"),
        origin,
        angles,
        state,
    })
}

fn player_state_vec3(state: &PlayerState, prefix: &str) -> Option<[f32; 3]> {
    Some([
        state.field_f32(&format!("{prefix}[0]"))?,
        state.field_f32(&format!("{prefix}[1]"))?,
        state.field_f32(&format!("{prefix}[2]"))?,
    ])
}

fn entity_field_index(name: &str) -> Option<usize> {
    ENTITY_FIELDS
        .iter()
        .position(|(candidate, _)| *candidate == name)
}

fn set_entity_i32(entity: &mut EntityState, name: &str, value: i32) {
    if let Some(index) = entity_field_index(name) {
        entity.fields[index] = value as u32;
    }
}

fn set_entity_f32(entity: &mut EntityState, name: &str, value: f32) {
    if let Some(index) = entity_field_index(name) {
        entity.fields[index] = value.to_bits();
    }
}

fn set_entity_vec3(entity: &mut EntityState, prefix: &str, value: [f32; 3]) {
    for (axis, value) in value.into_iter().enumerate() {
        set_entity_f32(entity, &format!("{prefix}[{axis}]"), value);
    }
}

fn copy_player_i32(ps: &PlayerState, entity: &mut EntityState, source: &str, target: &str) {
    if let Some(value) = ps.field_i32(source) {
        set_entity_i32(entity, target, value);
    }
}

fn copy_player_f32(ps: &PlayerState, entity: &mut EntityState, source: &str, target: &str) {
    if let Some(value) = ps.field_f32(source) {
        set_entity_f32(entity, target, value);
    }
}

fn copy_player_vec3(ps: &PlayerState, entity: &mut EntityState, source: &str, target: &str) {
    if let Some(value) = player_state_vec3(ps, source) {
        set_entity_vec3(entity, target, value);
    }
}
fn field_i32(entity: &EntityState, name: &str) -> i32 {
    entity.field_i32(name).unwrap_or(0)
}

fn entity_vec3(entity: &EntityState, prefix: &str) -> Option<[f32; 3]> {
    Some([
        entity.field_f32(&format!("{prefix}[0]"))?,
        entity.field_f32(&format!("{prefix}[1]"))?,
        entity.field_f32(&format!("{prefix}[2]"))?,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_server_command_bytes_preserve_configstrings_and_sequence() {
        let mut game = ClientGameState::new();
        game.queue_server_command(ServerCommand { sequence: 72, text: b"chat \"^1\xd8hello\xff\"".to_vec() });
        game.queue_server_command(ServerCommand { sequence: 73, text: b"  cs\t 1133  \"\\n\\\xd8Player\\model\\kyle\"".to_vec() });
        game.execute_server_commands(72).unwrap();
        assert_eq!(game.executed_server_command, 72);
        assert!(game.configstring(1133).is_none());
        game.execute_server_commands(73).unwrap();
        assert_eq!(game.configstring(1133), Some(b"\\n\\\xd8Player\\model\\kyle".as_slice()));
        assert_eq!(game.executed_server_command, 73);
        game.execute_server_command(&ServerCommand { sequence: 74, text: b"cs 1133 \"\"".to_vec() }).unwrap();
        assert_eq!(game.configstring(1133), Some(b"".as_slice()));
        assert!(game.execute_server_command(&ServerCommand { sequence: 75, text: b"cs 1700 invalid".to_vec() }).is_err());
    }

    #[test]
    fn command_tokenization_uses_openjk_byte_and_quote_rules() {
        assert_eq!(server_command_tokens(b" /* comment */ cs 1 \"a  \xff // b\" // end"),
            vec![b"cs".as_slice(), b"1", b"a  \xff // b"]);
        assert_eq!(server_command_tokens(b"cs 1 \"a\\\"b"), vec![b"cs".as_slice(), b"1", b"a\\", b"b"]);
        assert_eq!(server_command_tokens(b"cs 1 abc\0ignored"), vec![b"cs".as_slice(), b"1", b"abc"]);
    }

    #[test]
    fn ghoul2_body_queue_server_commands_follow_openjk_ircg_rcg() {
        let mut game = ClientGameState::new();
        game.execute_server_command(&ServerCommand {
            sequence: 76,
            text: b"ircg 7 71 3 1".to_vec(),
        })
        .unwrap();
        game.execute_server_command(&ServerCommand {
            sequence: 77,
            text: b"rcg 7".to_vec(),
        })
        .unwrap();

        assert_eq!(
            game.drain_ghoul2_commands(),
            vec![
                Ghoul2ServerCommand::BodyQueueCopy {
                    source_client: 7,
                    body_entity: 71,
                    known_weapon: 3,
                    light_side: true,
                },
                Ghoul2ServerCommand::RestoreClient { source_client: 7 },
            ]
        );
    }

    #[test]
    fn scores_server_command_parses_openjk_14_int_records() {
        let mut game = ClientGameState::new();
        game.configstrings.insert(
            CS_PLAYERS + 2,
            br"\n\^1Red Jawa\t\1\model\jawa/default".to_vec(),
        );
        game.execute_server_command(&ServerCommand {
            sequence: 80,
            text: b"scores 1 17 9 2 42 55 7 3 4 88 5 6 7 8 9 1 11".to_vec(),
        })
        .unwrap();

        let notices = game.drain_notices();
        let CgameNotice::Scores { team_scores, entries } = &notices[0] else {
            panic!("expected scores notice");
        };
        assert_eq!(*team_scores, [17, 9]);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].client, 2);
        assert_eq!(entries[0].name, "^1Red Jawa");
        assert_eq!(entries[0].team, 1);
        assert_eq!(entries[0].score, 42);
        assert_eq!(entries[0].ping, 55);
        assert_eq!(entries[0].time, 7);
        assert_eq!(entries[0].captures, 11);
    }

    #[test]
    #[ignore = "requires JKA_TEST_BASE and demos/cheezyVsource.dm_26"]
    fn cheezy_demo_server_commands_reach_eof() {
        use jka_protocol::{demo::DemoReader, server::{Decoder, Event}};
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let mut assets = jka_assets::pk3::AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let bytes = assets.read("demos/cheezyVsource.dm_26", 64 * 1024 * 1024).unwrap().unwrap().bytes;
        let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
        let mut decoder = Decoder::new();
        let mut game = ClientGameState::new();
        let mut snapshots = 0;
        let mut saw_72 = false;
        let mut legacy_commands = 0;
        let mut last_time = 0;
        while let Some(record) = reader.next_record().unwrap() {
            for event in decoder.parse_packet(record.sequence, &record.payload).unwrap().events {
                match event {
                    Event::Gamestate { server_command_sequence, .. } => {
                        game.reset_gamestate(&decoder.configstrings, server_command_sequence);
                    }
                    Event::ServerCommand(command) => {
                        if std::str::from_utf8(&command.text).is_err() { legacy_commands += 1; }
                        if command.sequence == 72 {
                            saw_72 = true;
                            println!("COMMAND 72 bytes={:?}", command.text);
                        }
                        game.queue_server_command(command);
                    }
                    Event::Snapshot { .. } => {
                        let snapshot = decoder.latest_snapshot().unwrap();
                        if snapshots == 0 {
                            game.set_initial_snapshot(snapshot).unwrap();
                        } else {
                            game.set_next_snapshot(Some(snapshot)).unwrap();
                            game.transition_snapshot(snapshot.server_time).unwrap();
                        }
                        snapshots += 1;
                        last_time = snapshot.server_time;
                        game.drain_presentation_events();
                        game.drain_event_check_traces();
                    }
                    _ => {}
                }
            }
        }
        println!("DEMO EOF snapshots={snapshots} lastTime={last_time} executedCommand={} legacyCommands={legacy_commands}", game.executed_server_command);
        assert!(saw_72 && legacy_commands > 0 && snapshots > 100);
        assert!(game.executed_server_command > 72);
    }
    use jka_protocol::gamestate::ENTITY_FIELDS;

    fn entity(number: u16, values: &[(&str, u32)]) -> EntityState {
        let mut entity = EntityState { number, fields: [0; ENTITY_FIELDS.len()] };
        for (name, value) in values {
            let index = ENTITY_FIELDS.iter().position(|(candidate, _)| candidate == name).unwrap();
            entity.fields[index] = *value;
        }
        entity
    }

    fn predicted_ps(client: i32, sequence: i32, event0: i32, parm0: i32, event1: i32, parm1: i32) -> PlayerState {
        // Protocol-26 PLAYER_FIELDS indices. The field_i32 assertions make this
        // helper fail loudly if the schema order ever changes underneath it.
        const EVENT_SEQUENCE: usize = 19;
        const EVENTS_0: usize = 27;
        const EVENTS_1: usize = 28;
        const CLIENT_NUM: usize = 43;
        const EVENT_PARMS_1: usize = 60;
        const EVENT_PARMS_0: usize = 65;
        let mut ps = PlayerState::default();
        ps.fields[EVENT_SEQUENCE] = sequence as u32;
        ps.fields[EVENTS_0] = event0 as u32;
        ps.fields[EVENTS_1] = event1 as u32;
        ps.fields[CLIENT_NUM] = client as u32;
        ps.fields[EVENT_PARMS_0] = parm0 as u32;
        ps.fields[EVENT_PARMS_1] = parm1 as u32;
        assert_eq!(ps.field_i32("eventSequence"), Some(sequence));
        assert_eq!(ps.field_i32("events[0]"), Some(event0));
        assert_eq!(ps.field_i32("events[1]"), Some(event1));
        assert_eq!(ps.field_i32("clientNum"), Some(client));
        assert_eq!(ps.field_i32("eventParms[0]"), Some(parm0));
        assert_eq!(ps.field_i32("eventParms[1]"), Some(parm1));
        ps
    }

    #[test]
    fn free_spectator_has_no_predicted_player_presentation() {
        let mut ps = PlayerState::default();
        assert!(presented_player_state_entity(&ps).is_some());
        ps.persistant[PERS_TEAM] = TEAM_SPECTATOR;
        assert!(presented_player_state_entity(&ps).is_none());
    }

    #[test]
    fn presentation_event_queue_preserves_same_snapshot_discovery_order() {
        let player_state = PlayerState::default();
        let first = entity(70, &[("eType", (ET_EVENTS + EntityEvent::EV_JUMP.as_i32()) as u32)]);
        let second = entity(71, &[("eType", (ET_EVENTS + EntityEvent::EV_ROLL.as_i32()) as u32)]);
        let snapshot = Snapshot {
            server_time: 1_000,
            message_num: 1,
            delta_num: -1,
            snap_flags: 0,
            server_command_num: 0,
            area_mask: [0; 32],
            player_state,
            vehicle_player_state: None,
            entities: vec![first, second],
        };

        let mut game = ClientGameState::new();
        game.set_initial_snapshot(&snapshot).unwrap();

        let first = game.pop_presentation_event().expect("first queued event");
        let second = game.pop_presentation_event().expect("second queued event");
        assert_eq!(first.receive_sequence, 1);
        assert_eq!(first.source_entity_num, 70);
        assert_eq!(first.event, EntityEvent::EV_JUMP);
        assert_eq!(second.receive_sequence, 2);
        assert_eq!(second.source_entity_num, 71);
        assert_eq!(second.event, EntityEvent::EV_ROLL);
        assert!(game.pop_presentation_event().is_none());
    }

    #[test]
    fn predicted_events_share_the_same_monotonic_receive_queue() {
        let mut game = ClientGameState::new();
        let old = predicted_ps(3, 0, 0, 0, 0, 0);
        let next = predicted_ps(
            3,
            2,
            EntityEvent::EV_JUMP.as_i32(),
            7,
            EntityEvent::EV_ROLL.as_i32(),
            9,
        );

        game.transition_predicted_player_state(&next, &old, 1_000).unwrap();
        let first = game.pop_presentation_event().expect("first predicted event");
        let second = game.pop_presentation_event().expect("second predicted event");
        assert_eq!((first.receive_sequence, first.event, first.parm), (1, EntityEvent::EV_JUMP, 7));
        assert_eq!((second.receive_sequence, second.event, second.parm), (2, EntityEvent::EV_ROLL, 9));
        assert!(game.pop_presentation_event().is_none());
    }

    #[test]
    fn predicted_player_event_is_emitted_once_from_committed_transition() {
        let mut game = ClientGameState::new();
        let old = predicted_ps(3, 0, 0, 0, 0, 0);
        let next = predicted_ps(3, 1, EntityEvent::EV_JUMP.as_i32(), 7, 0, 0);

        game.transition_predicted_player_state(&next, &old, 1_000).unwrap();
        let events = game.drain_presentation_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, EntityEvent::EV_JUMP);
        assert_eq!(events[0].parm, 7);

        // Re-presenting the same committed state must not replay its event.
        game.transition_predicted_player_state(&next, &next, 1_008).unwrap();
        assert!(game.drain_presentation_events().is_empty());
    }

    #[test]
    fn changed_predicted_event_replaces_ring_without_double_dispatch() {
        let mut game = ClientGameState::new();
        let old = predicted_ps(3, 0, 0, 0, 0, 0);
        let first = predicted_ps(3, 1, EntityEvent::EV_JUMP.as_i32(), 1, 0, 0);
        game.transition_predicted_player_state(&first, &old, 1_000).unwrap();
        let first_events = game.drain_presentation_events();
        assert_eq!(first_events.len(), 1);
        assert_eq!(first_events[0].event, EntityEvent::EV_JUMP);

        // Same eventSequence, but replay/authority corrected the recent slot.
        let corrected = predicted_ps(3, 1, EntityEvent::EV_ROLL.as_i32(), 3, 0, 0);
        game.transition_predicted_player_state(&corrected, &first, 1_008).unwrap();
        let corrected_events = game.drain_presentation_events();
        assert_eq!(corrected_events.len(), 1);
        assert_eq!(corrected_events[0].event, EntityEvent::EV_ROLL);
        assert_eq!(corrected_events[0].parm, 3);

        game.transition_predicted_player_state(&corrected, &corrected, 1_016).unwrap();
        assert!(game.drain_presentation_events().is_empty());
    }

    #[test]
    fn predicted_transition_does_not_leak_events_across_followed_clients() {
        let mut game = ClientGameState::new();
        let old = predicted_ps(2, 0, 0, 0, 0, 0);
        let next = predicted_ps(4, 1, EntityEvent::EV_JUMP.as_i32(), 0, 0, 0);
        game.transition_predicted_player_state(&next, &old, 1_000).unwrap();
        assert!(game.drain_presentation_events().is_empty());
    }

    #[test]
    fn trajectory_linear_matches_openjk_units() {
        let tr = Trajectory {
            kind: TR_LINEAR,
            time: 1_000,
            duration: 0,
            base: [10.0, 20.0, 30.0],
            delta: [100.0, -50.0, 20.0],
        };
        assert_eq!(evaluate_trajectory(tr, 1_500).unwrap(), [60.0, -5.0, 40.0]);
    }

    #[test]
    fn configstring_resources_use_protocol_ranges() {
        let mut cg = ClientGameState::new();
        cg.configstrings
            .insert(CS_MODELS + 7, b"models/map_objects/test.md3".to_vec());
        cg.configstrings
            .insert(CS_SOUNDS + 3, b"sound/test.wav".to_vec());
        cg.configstrings
            .insert(CS_EFFECTS + 2, b"effects/test.efx".to_vec());
        assert_eq!(cg.model_qpath(7).as_deref(), Some("models/map_objects/test.md3"));
        assert_eq!(cg.sound_qpath(3).as_deref(), Some("sound/test.wav"));
        assert_eq!(cg.effect_qpath(2).as_deref(), Some("effects/test.efx"));
        assert!(cg.model_qpath(0).is_none());
    }

    #[test]
    fn entity_event_is_emitted_once_and_masks_sequence_bits() {
        let mut cent = CEntity::default();
        let state = entity(
            12,
            &[
                ("eType", ET_GENERAL as u32),
                ("event", (0x200 | EntityEvent::EV_DISRUPTOR_SNIPER_MISS.as_i32()) as u32),
                ("eventParm", 9),
            ],
        );
        let outcome = check_entity_event(&mut cent, &state, 1000).unwrap();
        assert_eq!(outcome.trace.disposition, EventCheckDisposition::Accepted);
        let event = outcome.event.unwrap();
        assert_eq!(event.event, EntityEvent::EV_DISRUPTOR_SNIPER_MISS);
        assert_eq!(event.raw_event, 0x200 | EntityEvent::EV_DISRUPTOR_SNIPER_MISS.as_i32());
        assert_eq!(event.parm, 9);
        let duplicate = check_entity_event(&mut cent, &state, 1000).unwrap();
        assert!(duplicate.event.is_none());
        assert_eq!(duplicate.trace.disposition, EventCheckDisposition::Duplicate);
    }

    #[test]
    fn player_event_entity_reports_other_entity_num() {
        let mut cent = CEntity::default();
        let state = entity(
            70,
            &[
                ("eType", (ET_EVENTS + EntityEvent::EV_WATER_CLEAR.as_i32()) as u32),
                ("eFlags", EF_PLAYER_EVENT as u32),
                ("otherEntityNum", 4),
            ],
        );
        let outcome = check_entity_event(&mut cent, &state, 2500).unwrap();
        assert_eq!(outcome.trace.disposition, EventCheckDisposition::Accepted);
        let event = outcome.event.unwrap();
        assert!(event.event_only_entity);
        assert_eq!(event.source_entity_num, 70);
        assert_eq!(event.entity_num, 4);
        assert_eq!(event.event, EntityEvent::EV_WATER_CLEAR);
        let duplicate = check_entity_event(&mut cent, &state, 2500).unwrap();
        assert!(duplicate.event.is_none());
        assert_eq!(duplicate.trace.disposition, EventCheckDisposition::Duplicate);
    }

    #[test]
    fn set_next_snap_disables_interpolation_on_teleport_toggle() {
        let first_entity = entity(4, &[("eType", ET_PLAYER as u32), ("eFlags", 0)]);
        let next_entity = entity(4, &[("eType", ET_PLAYER as u32), ("eFlags", EF_TELEPORT_BIT as u32)]);
        let player_state = jka_protocol::server::PlayerState::default();
        let first = Snapshot {
            server_time: 100,
            message_num: 1,
            delta_num: -1,
            snap_flags: 0,
            server_command_num: 0,
            area_mask: [0; 32],
            player_state: player_state.clone(),
            vehicle_player_state: None,
            entities: vec![first_entity],
        };
        let next = Snapshot { server_time: 133, message_num: 2, entities: vec![next_entity], ..first.clone() };
        let mut cg = ClientGameState::new();
        cg.set_initial_snapshot(&first).unwrap();
        cg.set_next_snapshot(Some(&next)).unwrap();
        assert!(!cg.entities[4].interpolate);
    }
}

#[cfg(test)]
mod client_info_tests {
    use super::*;

    #[test]
    fn missing_model_fallback_uses_gender_and_gametype_without_changing_identity() {
        let mut game = ClientGameState::new();
        game.configstrings.insert(
            CS_PLAYERS + 2,
            br"\n\Opponent\t\2\ds\f\model\missing/blue\st\dual_2\st2\none".to_vec(),
        );
        let info = game.client_info(2, &[]).unwrap();
        let fallback = info.missing_model_fallback();
        assert_eq!(fallback.model_cvar(), "jan");
        assert_eq!(fallback.client_num, 2);
        assert_eq!(fallback.name, "Opponent");
        assert_eq!(fallback.saber_name, "dual_2");
        assert_eq!(fallback.saber2_name, "none");
        assert_eq!(info.model_cvar(), "missing/blue");

        game.configstrings.insert(CS_SERVERINFO, br"\g_gametype\6".to_vec());
        let mut info = game.client_info(2, &[]).unwrap();
        assert_eq!(info.missing_model_fallback().model_cvar(), "jan/blue");
        info.female = false;
        assert_eq!(info.missing_model_fallback().model_cvar(), "kyle/blue");
        info.gametype = 3; // Duel is not a team mode, even with a team hint.
        assert_eq!(info.missing_model_fallback().model_cvar(), "kyle");
    }

    #[test]
    fn new_client_info_model_and_skin_match_openjk_split() {
        let mut game = ClientGameState::new();
        game.configstrings.insert(
            CS_PLAYERS,
            br"\n\Player\t\1\model\jedi_hm/head_a1|torso_a1|lower_a1".to_vec(),
        );
        let info = game.client_info(0, &[]).unwrap();
        assert_eq!(info.model_name, "jedi_hm");
        assert_eq!(info.skin_name, "head_a1|torso_a1|lower_a1");
    }

    #[test]
    fn compact_force_model_parses_single_and_team_split_forms() {
        assert!(ForcedPlayerModels::parse("0").unwrap().is_none());
        let all = ForcedPlayerModels::parse("kyle").unwrap().unwrap();
        assert_eq!(all.ally, "kyle");
        assert_eq!(all.enemy, "kyle");
        assert!(!all.split);
        assert_eq!(all.serialize(), "kyle");
        let split = ForcedPlayerModels::parse("rebel/default,stormtrooper/default").unwrap().unwrap();
        assert_eq!(split.ally, "rebel/default");
        assert_eq!(split.enemy, "stormtrooper/default");
        assert!(split.split);
        assert_eq!(split.serialize(), "rebel/default,stormtrooper/default");
        assert_eq!(ForcedPlayerModels::parse("kyle,kyle").unwrap().unwrap().serialize(), "kyle,kyle");
        assert!(ForcedPlayerModels::parse("rebel,,stormtrooper").is_err());
        assert!(ForcedPlayerModels::parse("/default").is_err());
        assert!(ForcedPlayerModels::parse("kyle/").is_err());
    }

    #[test]
    fn compact_force_model_uses_team_relation_and_preserves_viewer_model() {
        let forced = ForcedPlayerModels::parse("rebel,stormtrooper").unwrap().unwrap();
        let viewer = ClientInfo { client_num: 1, team: 1, gametype: 6, ..ClientInfo::solo_kyle() };
        let ally = ClientInfo { client_num: 2, team: 1, gametype: 6, skin_name: "red".to_owned(), ..ClientInfo::solo_kyle() };
        let enemy = ClientInfo { client_num: 3, team: 2, gametype: 6, skin_name: "blue".to_owned(), ..ClientInfo::solo_kyle() };
        let mut preserved_viewer = viewer.clone();
        forced.apply_to(&mut preserved_viewer, Some(&viewer));
        assert_eq!(preserved_viewer.model_cvar(), "kyle");

        let mut forced_ally = ally;
        forced.apply_to(&mut forced_ally, Some(&viewer));
        assert_eq!(forced_ally.model_cvar(), "rebel/red");

        let mut forced_enemy = enemy;
        forced.apply_to(&mut forced_enemy, Some(&viewer));
        assert_eq!(forced_enemy.model_cvar(), "stormtrooper/blue");

        let mut ffa_enemy = ClientInfo { client_num: 4, team: 0, gametype: 0, ..ClientInfo::solo_kyle() };
        forced.apply_to(&mut ffa_enemy, Some(&viewer));
        assert_eq!(ffa_enemy.model_cvar(), "stormtrooper");
    }

    #[test]
    fn siege_class_forces_model_and_skin() {
        let mut game = ClientGameState::new();
        game.configstrings
            .insert(CS_SERVERINFO, br"\g_gametype\7".to_vec());
        game.configstrings.insert(
            CS_PLAYERS + 3,
            br"\n\Player\t\2\model\kyle/default\siegeclass\Jedi Guardian".to_vec(),
        );
        let classes = vec![SiegeClassVisual {
            name: "jedi guardian".to_owned(),
            forced_model: "jedi_hm".to_owned(),
            forced_skin: "head_a1|torso_a1|lower_a1".to_owned(),
        }];
        let info = game.client_info(3, &classes).unwrap();
        assert_eq!(info.model_name, "jedi_hm");
        assert_eq!(info.skin_name, "head_a1|torso_a1|lower_a1");
        assert_eq!(
            info.skin_qpath(),
            "models/players/jedi_hm/|head_a1|torso_a1|lower_a1"
        );
    }
}
