//! Faithful client-game snapshot/entity state translated from OpenJK `codemp/cgame/cg_snapshot.c`
//! and `codemp/cgame/cg_ents.c`.
//!
//! This module intentionally sits downstream of the protocol decoder. Demo playback and a future
//! network source both feed the same decoded `Snapshot` values into this layer.

mod player_animation;
pub(crate) mod footsteps;
pub(crate) mod ragdoll;
pub(crate) mod cloth;
pub(crate) mod cloth_body;
pub(crate) mod jiggle;
mod saber_throw;
pub mod item_presenter;
pub mod weapon_fx;
pub(crate) mod saber_melt;
pub mod sound_presenter;
pub(crate) mod sound_tables;
pub(crate) mod ambient_sets;
pub mod stringed;
pub mod entity_presenter;
pub mod event_debug;
pub mod event_presenter;
pub(crate) mod event_workers;
pub mod player_presenter;
pub(crate) mod player_gore;
pub mod view;
pub mod crosshair;

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
pub const CS_MUSIC: u16 = 2;
pub const CS_WARMUP: u16 = 5;
pub const CS_SCORES1: u16 = 6;
pub const CS_SCORES2: u16 = 7;
pub const CS_LEVEL_START_TIME: u16 = 21;
pub const CS_INTERMISSION: u16 = 22;
pub const CS_FLAGSTATUS: u16 = 23;
pub const CS_SHADERSTATE: u16 = 24;
pub const CS_ITEMS: u16 = 27;
pub const CS_CLIENT_JEDIMASTER: u16 = 28;
pub const CS_CLIENT_DUELWINNER: u16 = 29;
pub const CS_CLIENT_DUELISTS: u16 = 30;
pub const CS_CLIENT_DUELHEALTHS: u16 = 31;
pub const CS_LEGACY_FIXES: u16 = 36;
pub const CS_SIEGE_STATE: u16 = 293;
pub const CS_SIEGE_OBJECTIVES: u16 = 294;
pub const CS_SIEGE_TIMEOVERRIDE: u16 = 295;
pub const CS_SIEGE_WINTEAM: u16 = 296;
// OpenJK/TaystJK protocol-26 configstring layout from bg_public.h. Keeping these
// in the cgame layer lets demos and a future live net source resolve the same
// model/sound/effect indexes without renderer-owned lookup tables.
pub const CS_MODELS: u16 = 298;
pub const CS_SOUNDS: u16 = 811;
pub const CS_PLAYERS: u16 = 1131;
/// CS_PLAYERS + MAX_CLIENTS + MAX_G2BONES in protocol 26.
pub const CS_LOCATIONS: u16 = 1227;
pub const CS_EFFECTS: u16 = 1355;
pub const CS_LIGHT_STYLES: u16 = CS_EFFECTS + 64;
pub const MAX_LIGHT_STYLE_CONFIGSTRINGS: u16 = 64 * 3;
pub const GT_TEAM: i32 = 6;
pub const GT_SIEGE: i32 = 7;
pub const TEAM_RED: i32 = 1;
pub const TEAM_BLUE: i32 = 2;
pub const SABER_RED: i32 = 0;
pub const SABER_BLUE: i32 = 4;
pub const SABER_PURPLE: i32 = 5;
const MAX_QPATH: usize = 64;

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
    /// TaystJK serverinfo exception: team skin/saber forcing is disabled in Jedi-vs-Merc mode.
    pub jedi_v_merc: bool,
    /// BG_ValidateSkinForTeam's RGB/Jedi-model fallback tint. Zero/None means use customRGBA.
    pub team_color_override: Option<[u8; 3]>,
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
    /// Local cp_pluginDisable used when the final authored/client saber colour is
    /// chosen. Keeping it on ClientInfo also covers NPC definition blade colours.
    pub plugin_disable: i32,
    /// CG_AddSaberBlade for an NPC without `boltToPlayer` colors draws each
    /// blade in its saber definition's authored color instead of c1/c2.
    pub definition_saber_colors: bool,
    /// jaPRO `c5` (relayed `cp_cosmetics`): bit mask of worn cosmetics.
    pub cosmetics: u32,
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
            jedi_v_merc: false,
            team_color_override: None,
            female: false,
            model_name: "kyle".to_owned(),
            skin_name: "default".to_owned(),
            siege_class: String::new(),
            saber_name: String::new(),
            saber2_name: String::new(),
            saber_color: 4, // SABER_BLUE
            saber2_color: 4,
            plugin_disable: 1536,
            definition_saber_colors: false,
            cosmetics: 0,
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
pub(crate) const PM_INTERMISSION: i32 = 7;
const PERS_TEAM: usize = 3;
const PERS_PLAYEREVENTS: usize = 5;
const PERS_IMPRESSIVE_COUNT: usize = 9;
const PERS_EXCELLENT_COUNT: usize = 10;
const PERS_DEFEND_COUNT: usize = 11;
const PERS_ASSIST_COUNT: usize = 12;
const PERS_GAUNTLET_FRAG_COUNT: usize = 13;
const PERS_CAPTURES: usize = 14;
const PLAYEREVENT_DENIEDREWARD: i32 = 0x0001;
const PLAYEREVENT_GAUNTLETREWARD: i32 = 0x0002;
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
    /// jaPRO/JA+ cjp_client extension; absent on stock 14-field scores records.
    pub deaths: Option<i32>,
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
    /// TaystJK/OpenJK `kg2`: destroy a non-client Ghoul2 instance.
    KillEntity { entity: u16 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamInfoEntry {
    pub client: u16,
    pub location: i32,
    pub health: i32,
    pub armor: i32,
    pub weapon: i32,
    pub powerups: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CosmeticUnlock {
    pub bitvalue: i32,
    pub mapname: String,
    pub style: i32,
    pub duration: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShaderRemap {
    pub from: String,
    pub to: String,
    pub time_offset: String,
}

/// Reliable cgame side effects which belong outside pure snapshot state. The
/// first four compatibility passes intentionally preserve these commands even
/// where the final Siege/UI consumer is implemented later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CgameServerAction {
    KillLoopSounds { entities: Vec<u16> },
    ShaderRemap(ShaderRemap),
    RestartMapMusic,
    LightStyleChanged { index: u16 },
    NewForceRank { rank: i32, open_menu: bool, team: i32 },
    SiegeBriefing { team: i32 },
    SiegeClassSelect,
    SiegeProfile,
    SiegeExtendedData(Vec<u8>),
    ClientLevelShot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConfigStringState {
    jedi_master: i32,
    duel_winner: i32,
    duelists: [i32; 3],
    duelist_healths: [i32; 3],
    intermission: bool,
    flag_status: [i32; 2],
    siege_state: Vec<u8>,
    siege_objectives: Vec<u8>,
    siege_time_override: i32,
    siege_win_team: i32,
    shader_remaps: BTreeMap<String, ShaderRemap>,
}

impl Default for ConfigStringState {
    fn default() -> Self {
        Self {
            jedi_master: -1,
            duel_winner: -1,
            duelists: [-1; 3],
            duelist_healths: [-1; 3],
            intermission: false,
            flag_status: [-1; 2],
            siege_state: Vec::new(),
            siege_objectives: Vec::new(),
            siege_time_override: 0,
            siege_win_team: 0,
            shader_remaps: BTreeMap::new(),
        }
    }
}

/// Which server command produced a chat line; jaPRO's `cg_chatSounds` picks the
/// beep from it (CG_Chat_f).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatKind {
    /// `chat`
    Say,
    /// `tchat`
    Team,
    /// `lchat` / `ltchat`
    Located,
    /// A line synthesised client-side from a VGS event; the game makes no sound.
    Voice,
}

/// Text/output state from CG_ServerCommand for the console and HUD UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewardKind {
    Capture,
    Impressive,
    Excellent,
    Humiliation,
    Defend,
    Assist,
    Denied,
    GauntletEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RewardBaseline {
    client_num: i32,
    team: i32,
    counts: [i32; 6],
    player_events: i32,
}

impl RewardBaseline {
    fn from_player_state(ps: &PlayerState) -> Self {
        Self {
            client_num: ps.field_i32("clientNum").unwrap_or(-1),
            team: ps.persistant[PERS_TEAM],
            counts: [
                ps.persistant[PERS_CAPTURES],
                ps.persistant[PERS_IMPRESSIVE_COUNT],
                ps.persistant[PERS_EXCELLENT_COUNT],
                ps.persistant[PERS_GAUNTLET_FRAG_COUNT],
                ps.persistant[PERS_DEFEND_COUNT],
                ps.persistant[PERS_ASSIST_COUNT],
            ],
            player_events: ps.persistant[PERS_PLAYEREVENTS],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CgameNotice {
    /// `print`: console text (may contain @@@ StringEd references).
    Print(Vec<u8>),
    /// `chat`/`tchat`/`lchat`/`ltchat`: a chat-box line.
    Chat { team: bool, kind: ChatKind, text: Vec<u8> },
    /// `cp`: CG_CenterPrint.
    CenterPrint(Vec<u8>),
    /// TaystJK/jaPRO `cg_killMessage`, generated from EV_OBITUARY for the
    /// viewer's own non-suicide kills. Localisation/presentation stays app-side.
    KillMessage {
        target: String,
        /// Only available when the viewed client owns the recorded playerState.
        rank: Option<i32>,
        /// Only available when the viewed client owns the recorded playerState.
        score: Option<i32>,
        gametype: i32,
    },
    /// OpenJK `CG_Obituary` console line generated directly from EV_OBITUARY.
    /// Demos carry the event, not a separate server `print`, so this must live
    /// in cgame presentation to work identically live and during playback.
    Obituary {
        target: String,
        attacker: Option<String>,
        key: &'static str,
    },
    /// JKA/TaystJK playerState reward counter transition. The app chooses
    /// cg_drawRewards mode assets and owns the visual FIFO.
    Reward { kind: RewardKind, count: i32 },
    /// `scores`: OpenJK scoreboard response.
    Scores {
        team_scores: [i32; 2],
        entries: Vec<ScoreEntry>,
    },
    /// `map_restart`.
    MapRestart,
    /// A client-generated StringEd message (`CG_GetStringEdString`), e.g. the
    /// CTF messages or BEGIN DUEL. `key` is `FILE_REFERENCE`.
    /// `CG_ItemPickup`'s "Picked up <item>" console line, for a bg_itemlist classname.
    ItemPickupLine { classname: String },
    StringEd {
        key: &'static str,
        /// Prefixed as `name^7 ` when set.
        player: Option<String>,
        /// Substituted for `%s` when set.
        team: Option<&'static str>,
        center: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationViewer {
    /// Ordinary OpenJK ownership: the current snapshot/playerState client.
    Snapshot,
    /// Presentation is following another client whose full playerState is not recorded.
    Client(i32),
    /// Free camera / no player owns local-only presentation.
    None,
}

pub struct ClientGameState {
    entities: Vec<CEntity>,
    /// jaPRO `cg_entities[client].vChatTime`: until when a client's voice-chat
    /// icon shows (`EV_VOICECMD_SOUND` time + 1000).
    vchat_until: [i32; MAX_CLIENTS],
    notices: VecDeque<CgameNotice>,
    big_config: jka_protocol::commands::BigConfigString,
    configstrings: BTreeMap<u16, Vec<u8>>,
    /// Effective player whose local-only HUD/audio/event feedback is being viewed.
    /// Demos may follow a client other than the one whose playerState was recorded.
    presentation_viewer: PresentationViewer,
    /// Local `cp_pluginDisable`, needed by client-only jaPRO presentation rules
    /// such as bit 3 (black-saber suppression). Server-consumed bits still
    /// travel through userinfo in `NetworkSettings`.
    plugin_disable: i32,
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
    /// Highest playerState eventSequence already played. Predicted events come
    /// from the displayed state, which is re-predicted every frame with a
    /// provisional command, so the previous frame's state alone is not a
    /// reliable "already played" marker.
    predicted_event_high: Option<i32>,
    /// Last authoritative/predicted playerState reward counters seen. This
    /// de-duplicates the same transition when both snapshot and prediction
    /// paths observe it while still making demos/follow views work.
    reward_baseline: Option<RewardBaseline>,
    presentation_events: PresentationEventQueue,
    pending_event_traces: VecDeque<EventCheckTrace>,
    pending_ghoul2_commands: VecDeque<Ghoul2ServerCommand>,
    pending_server_actions: VecDeque<CgameServerAction>,
    pending_audio_actions: VecDeque<CgameServerAction>,
    team_info: Vec<TeamInfoEntry>,
    cosmetic_unlocks: Vec<CosmeticUnlock>,
    force_rank_change: Option<(i32, bool, i32)>,
    config_state: ConfigStringState,
}

impl Default for ClientGameState {
    fn default() -> Self {
        Self {
            entities: vec![CEntity::default(); MAX_GENTITIES],
            vchat_until: [0; MAX_CLIENTS],
            notices: VecDeque::new(),
            big_config: jka_protocol::commands::BigConfigString::default(),
            configstrings: BTreeMap::new(),
            presentation_viewer: PresentationViewer::Snapshot,
            plugin_disable: 1536,
            queued_server_commands: VecDeque::new(),
            executed_server_command: 0,
            current_snapshot: None,
            next_snapshot: None,
            frame_interpolation: 0.0,
            predicted_event_sequence: 0,
            predictable_events: [0; MAX_PREDICTED_EVENTS],
            predicted_event_high: None,
            reward_baseline: None,
            presentation_events: PresentationEventQueue::default(),
            pending_event_traces: VecDeque::new(),
            pending_ghoul2_commands: VecDeque::new(),
            pending_server_actions: VecDeque::new(),
            pending_audio_actions: VecDeque::new(),
            team_info: Vec::new(),
            cosmetic_unlocks: Vec::new(),
            force_rank_change: None,
            config_state: ConfigStringState::default(),
        }
    }
}

impl ClientGameState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_plugin_disable(&mut self, bits: i32) {
        self.plugin_disable = bits;
    }

    pub fn set_presentation_viewer(&mut self, viewer: PresentationViewer) {
        self.presentation_viewer = viewer;
    }

    /// Effective local/viewed client for presentation-only behavior. Live/local
    /// sessions use cg.snap->ps.clientNum; demos can explicitly follow a remote
    /// entity or have no player owner while in free camera.
    pub fn presentation_client_num(&self) -> Option<i32> {
        match self.presentation_viewer {
            PresentationViewer::Snapshot => self
                .current_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.player_state.field_i32("clientNum")),
            PresentationViewer::Client(client) => Some(client),
            PresentationViewer::None => None,
        }
    }

    /// A complete playerState exists only when the effective viewer is the
    /// playerState stored in the snapshot. Never expose the demo recorder's
    /// playerState while presentation is following a different client.
    pub fn presentation_player_state(&self) -> Option<&PlayerState> {
        let snapshot = self.current_snapshot.as_ref()?;
        let client = snapshot.player_state.field_i32("clientNum")?;
        (self.presentation_client_num() == Some(client)).then_some(&snapshot.player_state)
    }

    pub fn reset_gamestate(
        &mut self,
        configstrings: &BTreeMap<u16, Vec<u8>>,
        server_command_sequence: i32,
    ) {
        self.entities.fill(CEntity::default());
        self.vchat_until = [0; MAX_CLIENTS];
        self.big_config = jka_protocol::commands::BigConfigString::default();
        self.configstrings.clone_from(configstrings);
        self.presentation_viewer = PresentationViewer::Snapshot;
        self.queued_server_commands.clear();
        self.executed_server_command = server_command_sequence;
        self.current_snapshot = None;
        self.next_snapshot = None;
        self.frame_interpolation = 0.0;
        self.predicted_event_sequence = 0;
        self.predictable_events = [0; MAX_PREDICTED_EVENTS];
        self.predicted_event_high = None;
        self.reward_baseline = None;
        self.presentation_events.clear();
        self.pending_event_traces.clear();
        self.pending_ghoul2_commands.clear();
        self.pending_server_actions.clear();
        self.pending_audio_actions.clear();
        self.team_info.clear();
        self.cosmetic_unlocks.clear();
        self.force_rank_change = None;
        self.rebuild_configstring_state();
    }

    pub fn drain_ghoul2_commands(&mut self) -> Vec<Ghoul2ServerCommand> {
        self.pending_ghoul2_commands.drain(..).collect()
    }

    pub fn drain_server_actions(&mut self) -> Vec<CgameServerAction> {
        self.pending_server_actions.drain(..).collect()
    }

    /// Audio side effects have their own queue so unconsumed Siege/UI actions
    /// never get rescanned every frame.
    pub fn drain_audio_server_actions(&mut self) -> Vec<CgameServerAction> {
        self.pending_audio_actions.drain(..).collect()
    }

    pub fn team_info(&self) -> &[TeamInfoEntry] {
        &self.team_info
    }

    pub fn cosmetic_unlocks(&self) -> &[CosmeticUnlock] {
        &self.cosmetic_unlocks
    }

    pub fn force_rank_change(&self) -> Option<(i32, bool, i32)> {
        self.force_rank_change
    }

    pub fn shader_remaps(&self) -> &BTreeMap<String, ShaderRemap> {
        &self.config_state.shader_remaps
    }

    pub fn duelists(&self) -> [i32; 3] { self.config_state.duelists }
    pub fn duelist_healths(&self) -> [i32; 3] { self.config_state.duelist_healths }
    pub fn jedi_master(&self) -> i32 { self.config_state.jedi_master }
    pub fn duel_winner(&self) -> i32 { self.config_state.duel_winner }
    pub fn flag_status(&self) -> [i32; 2] { self.config_state.flag_status }
    pub fn intermission_started(&self) -> bool { self.config_state.intermission }
    pub fn siege_state(&self) -> &[u8] { &self.config_state.siege_state }
    pub fn siege_objectives(&self) -> &[u8] { &self.config_state.siege_objectives }
    pub fn siege_time_override(&self) -> i32 { self.config_state.siege_time_override }
    pub fn siege_win_team(&self) -> i32 { self.config_state.siege_win_team }

    pub fn push_notice(&mut self, notice: CgameNotice) {
        self.notices.push_back(notice);
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

    /// Client-visible configstring table, in protocol index order. This mirrors
    /// the client `gameState_t` backing TaystJK/OpenJK's `configstrings` command.
    pub fn configstrings(&self) -> &BTreeMap<u16, Vec<u8>> {
        &self.configstrings
    }

    /// Semantic configstring update used by non-wire sources such as the
    /// in-process local server. Network/demo paths still arrive through the
    /// normal `cs` server-command machinery.
    pub fn set_configstring(&mut self, index: u16, value: Vec<u8>) {
        self.configstrings.insert(index, value);
        self.configstring_modified(index);
    }

    /// Rust equivalent of TaystJK/OpenJK `CG_ConfigStringModified`: the wire
    /// table is already current when this runs, and this method performs the
    /// configstrings that have cgame side effects instead of treating every
    /// entry as passive lookup data. Asset registration remains lazy in this
    /// client, so model/sound/effect ranges need no synchronous registration.
    fn configstring_modified(&mut self, index: u16) {
        self.apply_configstring_state(index, true);
    }

    fn rebuild_configstring_state(&mut self) {
        self.config_state = ConfigStringState::default();
        for index in [
            CS_CLIENT_JEDIMASTER,
            CS_CLIENT_DUELWINNER,
            CS_CLIENT_DUELISTS,
            CS_CLIENT_DUELHEALTHS,
            CS_INTERMISSION,
            CS_FLAGSTATUS,
            CS_SHADERSTATE,
            CS_SIEGE_STATE,
            CS_SIEGE_OBJECTIVES,
            CS_SIEGE_TIMEOVERRIDE,
            CS_SIEGE_WINTEAM,
        ] {
            self.apply_configstring_state(index, false);
        }
    }

    fn apply_configstring_state(&mut self, index: u16, emit_actions: bool) {
        let value = self.configstring(index).unwrap_or_default().to_vec();
        let integer = parse_i32_ascii(&value).unwrap_or(0);
        match index {
            CS_MUSIC => {
                if emit_actions {
                    self.pending_audio_actions.push_back(CgameServerAction::RestartMapMusic);
                }
            }
            CS_CLIENT_JEDIMASTER => self.config_state.jedi_master = integer,
            CS_CLIENT_DUELWINNER => self.config_state.duel_winner = integer,
            CS_CLIENT_DUELISTS => self.config_state.duelists = parse_pipe_numbers::<3>(&value),
            CS_CLIENT_DUELHEALTHS => self.config_state.duelist_healths = parse_duelist_healths(&value),
            CS_INTERMISSION => self.config_state.intermission = integer != 0,
            CS_FLAGSTATUS => {
                // g_team.c transmits 0=at base, 1=taken, 2=dropped. CGame
                // remaps the compact wire digit back to flagStatus_t, where
                // FLAG_DROPPED is 4 because the one-flag states occupy 2/3.
                let decode = |byte: Option<&u8>| match byte.copied() {
                    Some(b'0') => 0,
                    Some(b'1') => 1,
                    Some(b'2') => 4,
                    _ => -1,
                };
                self.config_state.flag_status = [decode(value.first()), decode(value.get(1))];
            }
            CS_SHADERSTATE => {
                self.config_state.shader_remaps.clear();
                for remap in parse_shader_state(&value) {
                    if emit_actions {
                        self.pending_server_actions.push_back(CgameServerAction::ShaderRemap(remap.clone()));
                    }
                    self.config_state.shader_remaps.insert(remap.from.to_ascii_lowercase(), remap);
                }
            }
            CS_SIEGE_STATE => self.config_state.siege_state = value,
            CS_SIEGE_OBJECTIVES => self.config_state.siege_objectives = value,
            CS_SIEGE_TIMEOVERRIDE => self.config_state.siege_time_override = integer,
            CS_SIEGE_WINTEAM => self.config_state.siege_win_team = integer,
            index if (CS_LIGHT_STYLES..CS_LIGHT_STYLES + MAX_LIGHT_STYLE_CONFIGSTRINGS).contains(&index) => {
                if emit_actions {
                    self.pending_server_actions.push_back(CgameServerAction::LightStyleChanged {
                        index: index - CS_LIGHT_STYLES,
                    });
                }
            }
            // Serverinfo, warmup, scores, votes, player configstrings and
            // resource configstrings are read directly from the current table
            // by their consumers. This is equivalent to TaystJK's assignment /
            // registration side effects without duplicating cache ownership.
            _ => {}
        }
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
    /// player. `ps` and `ops` are the presented state and the previous frame's
    /// presented state (stock cgame runs a real usercmd every frame, so its
    /// predicted state is the presented one). Events must fire with the state
    /// the eye and model are drawn from; firing them from the trailing
    /// committed state delayed every predicted effect, and made step smoothing
    /// cancel a rise that had already been shown. `predicted_event_high` stops
    /// the per-frame re-prediction from replaying an event.
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

        self.check_reward_notices(ps);
        self.check_playerstate_events(ps, ops, server_time)?;
        // Port the correction gate as requested by the live handoff. Current
        // OpenJK keeps this routine available even where a callsite may be
        // disabled; running it after the ordinary transition cannot duplicate
        // events just stored above, but can replace a recently mispredicted one.
        self.check_changed_predictable_events(ps, server_time)
    }

    /// TaystJK `CG_CheckLocalSounds` reward-counter portion. Award ownership is
    /// server authoritative (`playerState.persistant[]`), so Excellent/spree
    /// timing is never guessed client-side.
    fn check_reward_notices(&mut self, ps: &PlayerState) {
        let next = RewardBaseline::from_player_state(ps);
        let Some(previous) = self.reward_baseline.replace(next) else {
            return;
        };

        // CG_TransitionPlayerState suppresses local sounds when changing the
        // followed client/team, in intermission, or while spectating. Keep the
        // baseline current but do not replay historical counters afterward.
        if next.client_num != previous.client_num
            || next.team != previous.team
            || next.team == TEAM_SPECTATOR
            || ps.field_i32("pm_type").unwrap_or(0) == PM_INTERMISSION
        {
            return;
        }

        let kinds = [
            RewardKind::Capture,
            RewardKind::Impressive,
            RewardKind::Excellent,
            RewardKind::Humiliation,
            RewardKind::Defend,
            RewardKind::Assist,
        ];
        for (index, kind) in kinds.into_iter().enumerate() {
            // Counter decreases are respawn/map/reset bookkeeping, not a new
            // award. Normal TaystJK transitions only ever increment here.
            if next.counts[index] > previous.counts[index] {
                self.notices.push_back(CgameNotice::Reward { kind, count: next.counts[index] });
            }
        }

        if next.player_events != previous.player_events {
            if (next.player_events & PLAYEREVENT_DENIEDREWARD)
                != (previous.player_events & PLAYEREVENT_DENIEDREWARD)
            {
                self.notices.push_back(CgameNotice::Reward { kind: RewardKind::Denied, count: 0 });
            } else if (next.player_events & PLAYEREVENT_GAUNTLETREWARD)
                != (previous.player_events & PLAYEREVENT_GAUNTLETREWARD)
            {
                self.notices.push_back(CgameNotice::Reward { kind: RewardKind::GauntletEvent, count: 0 });
            }
        }
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
        // The provisional command can add or drop an event between frames, so
        // never replay below what was already played. A larger drop than the
        // ring holds is a real discontinuity (respawn, new map), not jitter.
        let played = match self.predicted_event_high {
            Some(high) if high <= sequence.saturating_add(MAX_PS_EVENTS) => high.max(old_sequence),
            _ => old_sequence,
        };
        self.predicted_event_high = Some(played.max(sequence));
        for index in sequence.saturating_sub(MAX_PS_EVENTS)..sequence {
            let slot = index & (MAX_PS_EVENTS - 1);
            let event = ps.field_i32(&format!("events[{slot}]")).unwrap_or(0);
            let old_event = ops.field_i32(&format!("events[{slot}]")).unwrap_or(0);
            if index >= played
                || (index < old_sequence
                    && index > old_sequence.saturating_sub(MAX_PS_EVENTS)
                    && event != old_event)
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

    /// jaPRO CG_EntityEvent `ci->team == ourTeam || isGlobalVGS(s)`: ourTeam is
    /// PERS_TEAM, but a follow-cam viewer counts as TEAM_SPECTATOR (the snapshot
    /// carries the followed player's PERS_TEAM and clientNum). Global VGS families
    /// bypass the team test. Both the chat line and the voice icon use it.
    fn voice_command_shown(&self, speaker: usize, sound: &str) -> bool {
        const PMF_FOLLOW: i32 = 4096;
        let Some(snapshot) = self.current_snapshot.as_ref() else { return false };
        let Some(speaker_team) = u16::try_from(speaker)
            .ok()
            .and_then(|speaker| CS_PLAYERS.checked_add(speaker))
            .and_then(|index| self.configstring(index))
            .and_then(|info| info_value(info, b"t"))
            .and_then(parse_i32_ascii)
        else {
            return false;
        };
        let player_state = &snapshot.player_state;
        let local_team = if player_state.field_i32("pm_flags").unwrap_or(0) & PMF_FOLLOW != 0 {
            TEAM_SPECTATOR
        } else {
            player_state.persistant[PERS_TEAM]
        };
        speaker_team == local_team || crate::vgs::is_global_vgs(sound)
    }

    /// jaPRO `vChatEnt->vChatTime = cg.time + 1000` for an audible voice command.
    fn note_voice_command(&mut self, event: &PresentationEvent) {
        if event.event != EntityEvent::EV_VOICECMD_SOUND {
            return;
        }
        let Some(speaker) = event.state.field_i32("groundEntityNum").and_then(|n| usize::try_from(n).ok()) else {
            return;
        };
        if speaker >= MAX_CLIENTS {
            return;
        }
        let Some(sound) = self.sound_qpath(event.parm) else { return };
        if self.voice_command_shown(speaker, &sound) {
            self.vchat_until[speaker] = event.server_time.saturating_add(1000);
        }
    }

    /// `cent->vChatTime` for a client; the icon shows while it is `> cg.time`.
    pub fn voice_chat_until(&self, client: i32) -> i32 {
        usize::try_from(client).ok().and_then(|client| self.vchat_until.get(client)).copied().unwrap_or(0)
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

        if !self.voice_command_shown(speaker, &sound) {
            return None;
        }
        let speaker_info = self.configstring(CS_PLAYERS.checked_add(u16::try_from(speaker).ok()?)?)?;

        let speaker_name = info_value(speaker_info, b"n")?;
        if speaker_name.is_empty() {
            return None;
        }

        let mut text = Vec::with_capacity(speaker_name.len() + description.len() + 4);
        text.extend_from_slice(speaker_name);
        text.extend_from_slice(b"^7: ");
        text.extend_from_slice(description.as_bytes());
        Some(CgameNotice::Chat { team: true, kind: ChatKind::Voice, text })
    }

    pub fn presentation_event_count(&self) -> usize {
        self.presentation_events.events.len()
    }

    /// Pop one accepted event from the client receive queue in discovery order.
    /// Snapshot/playerstate parsing never performs presentation side effects;
    /// the caller drains this queue after state transitions are complete.
    pub fn pop_presentation_event(&mut self) -> Option<PresentationEvent> {
        let event = self.presentation_events.pop_front()?;
        self.note_voice_command(&event);
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

    /// True on a jaPRO server (`gamename`), where the jaPRO-only client rules apply.
    pub fn is_japro(&self) -> bool {
        self.configstring(CS_SERVERINFO).is_some_and(|info| {
            crate::net::mod_support::ServerMod::detect(info) == crate::net::mod_support::ServerMod::Japro
        })
    }

    /// TaystJK advertises cjp_client to both JA+ and jaPRO; those servers then
    /// append a 15th `deaths` integer to every scores record.
    pub fn supports_score_deaths(&self) -> bool {
        self.configstring(CS_SERVERINFO).is_some_and(|info| {
            matches!(
                crate::net::mod_support::ServerMod::detect(info),
                crate::net::mod_support::ServerMod::Japro | crate::net::mod_support::ServerMod::Japlus
            )
        })
    }

    /// TaystJK accepts RGB sabers on JA+/jaPRO and on any server explicitly
    /// advertising the RGB capability in `taystJKinfo`.
    pub fn rgb_sabers_supported(&self) -> bool {
        self.configstring(CS_SERVERINFO)
            .is_some_and(crate::net::mod_support::supports_rgb_sabers)
    }

    /// `cgs.jcinfo2`.
    pub fn japro_cinfo2(&self) -> i32 {
        self.configstring(CS_SERVERINFO)
            .and_then(|info| info_value(info, b"jcinfo2"))
            .and_then(parse_i32_ascii)
            .unwrap_or(0)
    }

    /// jaPRO `IsRacemode(&cg.predictedPlayerState)`: the viewer is in a timed race
    /// course (`STAT_RACEMODE`), where cgame skips shake, duel cues and similar.
    pub fn japro_racemode(&self) -> bool {
        const STAT_RACEMODE: usize = 11;
        self.is_japro()
            && self
                .current_snapshot()
                .is_some_and(|snapshot| snapshot.player_state.stats[STAT_RACEMODE] != 0)
    }

    /// `cgs.svfps`: `sv_fps` from serverinfo, 20 when it is absent or zero
    /// (`CG_ParseServerinfo`).
    pub fn server_fps(&self) -> i32 {
        self.configstring(CS_SERVERINFO)
            .and_then(|info| info_value(info, b"sv_fps"))
            .and_then(parse_i32_ascii)
            .filter(|fps| *fps != 0)
            .unwrap_or(20)
    }

    /// `g_synchronousClients`, a systeminfo cvar cgame mirrors (the lagometer labels it "snc").
    pub fn synchronous_clients(&self) -> bool {
        self.configstring(jka_protocol::session::CS_SYSTEMINFO)
            .and_then(|info| info_value(info, b"g_synchronousClients"))
            .and_then(parse_i32_ascii)
            .is_some_and(|value| value != 0)
    }

    pub fn gametype(&self) -> i32 {
        self.configstring(CS_SERVERINFO)
            .and_then(|info| info_value(info, b"g_gametype"))
            .and_then(parse_i32_ascii)
            .unwrap_or(0)
    }

    /// TaystJK `cgs.jediVmerc`, parsed from CS_SERVERINFO. Team presentation forcing is disabled there.
    pub fn jedi_v_merc(&self) -> bool {
        self.configstring(CS_SERVERINFO)
            .and_then(|info| info_value(info, b"g_jediVmerc"))
            .and_then(parse_i32_ascii)
            .unwrap_or(0)
            != 0
    }

    /// The `cgs.timelimit` / `fraglimit` / `levelStartTime` / `scores1` / `scores2`
    /// values `CG_ParseServerinfo` and `CG_ConfigStringModified` keep.
    pub fn match_limits(&self) -> MatchLimits {
        let info = |key: &[u8]| {
            self.configstring(CS_SERVERINFO)
                .and_then(|info| info_value(info, key))
                .and_then(parse_i32_ascii)
                .unwrap_or(0)
        };
        let cs = |index: u16| self.configstring(index).and_then(parse_i32_ascii).unwrap_or(0);
        MatchLimits {
            timelimit: info(b"timelimit"),
            fraglimit: info(b"fraglimit"),
            level_start_time: cs(CS_LEVEL_START_TIME),
            scores1: cs(CS_SCORES1),
            scores2: cs(CS_SCORES2),
        }
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

    /// `cgs.clientinfo[client].name`: CG_NewClientInfo keeps the `n` key of the
    /// client's `CS_PLAYERS` configstring, which is all this reads.
    pub fn client_name(&self, client: usize) -> Option<String> {
        let index = CS_PLAYERS.checked_add(u16::try_from(client).ok()?)?;
        let name = info_value(self.configstring(index)?, b"n")?;
        Some(bytes_to_lossless_ascii(name))
    }

    /// OpenJK `clientInfo_t.gender` source used by CG_Obituary. The protocol
    /// exposes it as the `ds` client-info key (`f`, `n`, otherwise male).
    pub fn client_gender(&self, client: usize) -> i32 {
        let Some(index) = u16::try_from(client).ok().and_then(|client| CS_PLAYERS.checked_add(client)) else {
            return 0;
        };
        match self
            .configstring(index)
            .and_then(|info| info_value(info, b"ds"))
            .and_then(|value| value.first().copied())
            .map(|byte| byte.to_ascii_lowercase())
        {
            Some(b'f') => 1,
            Some(b'n') => 2,
            _ => 0,
        }
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
        // jaPRO relays `cp_sbRGB1/2` as c3/c4; they only apply to SABER_RGB.
        let rgb_capable = self.rgb_sabers_supported();
        let mut saber_color = resolve_saber_color(
            info_value(configstring, b"c1").and_then(parse_i32_ascii).unwrap_or(4),
            info_value(configstring, b"c3").and_then(parse_i32_ascii).unwrap_or(0),
            rgb_capable,
        );
        let mut saber2_color = resolve_saber_color(
            info_value(configstring, b"c2").and_then(parse_i32_ascii).unwrap_or(4),
            info_value(configstring, b"c4").and_then(parse_i32_ascii).unwrap_or(0),
            rgb_capable,
        );

        if self.gametype() == GT_SIEGE {
            if let Some(class) = find_siege_class_visual(siege_classes, &siege_class) {
                if !class.forced_model.is_empty() {
                    model_name.clone_from(&class.forced_model);
                }
                if !class.forced_skin.is_empty() {
                    skin_name.clone_from(&class.forced_skin);
                }
                if let Some(color) = class.forced_saber_color {
                    saber_color = color;
                }
                if let Some(color) = class.forced_saber2_color {
                    saber2_color = color;
                }
            }
        }

        Some(ClientInfo {
            client_num,
            name,
            team,
            gametype: self.gametype(),
            jedi_v_merc: self.jedi_v_merc(),
            team_color_override: None,
            female: info_value(configstring, b"ds")
                .is_some_and(|value| value.first() == Some(&b'f')),
            model_name,
            skin_name,
            siege_class,
            saber_name,
            saber2_name,
            saber_color,
            saber2_color,
            plugin_disable: self.plugin_disable,
            definition_saber_colors: false,
            cosmetics: info_value(configstring, b"c5")
                .and_then(parse_i32_ascii)
                .map_or(0, |bits| bits as u32),
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
            jedi_v_merc: self.jedi_v_merc(),
            team_color_override: None,
            female: false,
            model_name: folder.to_owned(),
            skin_name: skin.to_owned(),
            siege_class: String::new(),
            saber_name: saber("npcSaber1"),
            saber2_name: saber("npcSaber2"),
            saber_color: (bolt_colors & 0x07) - 1,
            saber2_color: ((bolt_colors & 0x38) >> 3) - 1,
            plugin_disable: self.plugin_disable,
            definition_saber_colors: bolt_colors == 0,
            cosmetics: 0,
        })
    }

    /// OpenJK `CG_SetInitialSnapshot`, limited to the snapshot/entity responsibilities that exist
    /// in this Rust client today. Rendering/Ghoul2 side effects are deliberately not fabricated.
    pub fn set_initial_snapshot(&mut self, snapshot: &Snapshot) -> Result<(), String> {
        self.reward_baseline = Some(RewardBaseline::from_player_state(&snapshot.player_state));
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
        self.check_reward_notices(&next.player_state);

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

    /// TaystJK `CG_AddCEntity` keys its intermission presentation gate from
    /// `cg.predictedPlayerState.pm_type`. The current snapshot is the
    /// authoritative base for that predicted state in this client.
    pub(crate) fn rendering_intermission(&self) -> bool {
        self.current_snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.player_state.field_i32("pm_type").unwrap_or(0) == PM_INTERMISSION
        })
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
            let kind = if team { ChatKind::Team } else { ChatKind::Say };
            self.notices.push_back(CgameNotice::Chat { team, kind, text: strip_escape(arg(1)) });
            return Ok(());
        }
        if name.eq_ignore_ascii_case(b"lchat") || name.eq_ignore_ascii_case(b"ltchat") {
            if parts.len() < 4 {
                return Ok(());
            }
            // "%s^7<%s> ^%s%s": name, location, colour, message.
            let text = [arg(1), b"^7<".to_vec(), arg(2), b"> ^".to_vec(), arg(3), arg(4)].concat();
            let team = name.eq_ignore_ascii_case(b"ltchat");
            self.notices.push_back(CgameNotice::Chat { team, kind: ChatKind::Located, text: strip_escape(text) });
            return Ok(());
        }
        if name.eq_ignore_ascii_case(b"cps") {
            // CG_CenterPrintSE_f takes a direct StringEd key (optionally with
            // one leading @). Re-express it through the same @@@ translation
            // path already used by ordinary server text.
            let key_arg = arg(1);
            let key = key_arg.as_slice().strip_prefix(b"@").unwrap_or(key_arg.as_slice());
            self.notices.push_back(CgameNotice::CenterPrint([b"@@@".as_slice(), key].concat()));
            return Ok(());
        }
        if name.eq_ignore_ascii_case(b"tinfo") {
            // TaystJK CG_ParseTeamInfo: six integers per sorted team member.
            const FIELDS: usize = 6;
            let count = parts.get(1).copied().and_then(parse_i32_ascii).unwrap_or(0).clamp(0, MAX_CLIENTS as i32) as usize;
            let mut entries = Vec::with_capacity(count);
            for i in 0..count {
                let base = 2 + i * FIELDS;
                if parts.len() < base + FIELDS { break; }
                let number = |offset: usize| parts.get(base + offset).copied().and_then(parse_i32_ascii).unwrap_or(0);
                let client = number(0);
                if !(0..MAX_CLIENTS as i32).contains(&client) { continue; }
                entries.push(TeamInfoEntry {
                    client: client as u16,
                    location: number(1),
                    health: number(2),
                    armor: number(3),
                    weapon: number(4),
                    powerups: number(5),
                });
            }
            self.team_info = entries;
            return Ok(());
        }
        if name.eq_ignore_ascii_case(b"cosmetics") {
            // jaPRO/TaystJK sends colon/newline/tab delimited quadruples in
            // argv(1): bitvalue, mapname, style, duration.
            let text = arg(1);
            let fields: Vec<&[u8]> = text
                .split(|b| matches!(*b, b':' | b'\n' | b'\t'))
                .filter(|field| !field.is_empty())
                .collect();
            self.cosmetic_unlocks.clear();
            for row in fields.chunks_exact(4).take(64) {
                self.cosmetic_unlocks.push(CosmeticUnlock {
                    bitvalue: parse_i32_ascii(row[0]).unwrap_or(0),
                    mapname: bytes_to_lossless_ascii(row[1]),
                    style: parse_i32_ascii(row[2]).unwrap_or(0),
                    duration: parse_i32_ascii(row[3]).unwrap_or(0),
                });
            }
            return Ok(());
        }
        if name.eq_ignore_ascii_case(b"scores") {
            // OpenJK CG_ParseScores / g_cmds.c DeathmatchScoreboardMessage:
            // argv(1) count, argv(2..=3) red/blue totals, then a fixed number
            // of integer fields for each client. Stock is 14; jaPRO appends a
            // 15th (deaths) for clients that send cjp_client. DinurdoJK, like
            // TaystJK, advertises that userinfo key to JA+/jaPRO. A wrong stride
            // shifts every record and scrambles the client numbers (hence names).
            const MAX_SCORE_CLIENTS: usize = 32;
            let integer = |index: usize| {
                parts.get(index).copied().and_then(parse_i32_ascii).unwrap_or(0)
            };
            let count = integer(1).clamp(0, MAX_SCORE_CLIENTS as i32) as usize;
            let team_scores = [integer(2), integer(3)];
            let score_offset = score_record_stride(self.supports_score_deaths(), count, parts.len());
            let mut entries = Vec::with_capacity(count);
            for score_index in 0..count {
                let base = 4 + score_index * score_offset;
                if parts.len() < base + SCORE_FIELDS {
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
                    deaths: (score_offset == SCORE_FIELDS_JAPRO).then(|| integer(base + 14)),
                    name,
                    team,
                });
            }
            self.notices.push_back(CgameNotice::Scores { team_scores, entries });
            return Ok(());
        }
        if name.eq_ignore_ascii_case(b"clientLevelShot") {
            self.pending_server_actions.push_back(CgameServerAction::ClientLevelShot);
            return Ok(());
        }
        if name.eq_ignore_ascii_case(b"loaddefered") {
            // Intentionally a no-op: DinurdoJK never defers player models; its
            // async asset workers are the equivalent of the completed load.
            return Ok(());
        }
        if name.eq_ignore_ascii_case(b"nfr") {
            // TaystJK checks for rank + menu and reads the team with CG_Argv(3),
            // which yields an empty/zero argument when older servers omit it.
            if parts.len() >= 3 {
                let rank = parts.get(1).copied().and_then(parse_i32_ascii).unwrap_or(0);
                let open_menu = parts.get(2).copied().and_then(parse_i32_ascii).unwrap_or(0) != 0;
                let team = parts.get(3).copied().and_then(parse_i32_ascii).unwrap_or(0);
                self.force_rank_change = Some((rank, open_menu, team));
                self.pending_server_actions.push_back(CgameServerAction::NewForceRank { rank, open_menu, team });
            }
            return Ok(());
        }
        if name.eq_ignore_ascii_case(b"kg2") {
            for value in parts.iter().skip(1).copied() {
                let Some(entity) = parse_i32_ascii(value) else { continue; };
                if entity < MAX_CLIENTS as i32 {
                    // CG_KillGhoul2_f refuses to destroy client Ghoul2.
                    return Ok(());
                }
                if (0..MAX_GENTITIES as i32).contains(&entity) {
                    self.pending_ghoul2_commands.push_back(Ghoul2ServerCommand::KillEntity { entity: entity as u16 });
                }
            }
            return Ok(());
        }
        if name.eq_ignore_ascii_case(b"kls") {
            let entities = parts
                .iter()
                .skip(1)
                .take(2)
                .copied()
                .filter_map(parse_i32_ascii)
                .filter(|entity| (0..MAX_GENTITIES as i32).contains(entity))
                .map(|entity| entity as u16)
                .collect::<Vec<_>>();
            if !entities.is_empty() {
                self.pending_audio_actions.push_back(CgameServerAction::KillLoopSounds { entities });
            }
            return Ok(());
        }
        if name.eq_ignore_ascii_case(b"remapShader") {
            if parts.len() == 4 {
                let remap = ShaderRemap {
                    from: bytes_to_lossless_ascii(parts[1]),
                    to: bytes_to_lossless_ascii(parts[2]),
                    time_offset: bytes_to_lossless_ascii(parts[3]),
                };
                self.config_state.shader_remaps.insert(remap.from.to_ascii_lowercase(), remap.clone());
                self.pending_server_actions.push_back(CgameServerAction::ShaderRemap(remap));
            }
            return Ok(());
        }
        if name.eq_ignore_ascii_case(b"sb") {
            let team = parts.get(1).copied().and_then(parse_i32_ascii).unwrap_or(0);
            self.pending_server_actions.push_back(CgameServerAction::SiegeBriefing { team });
            return Ok(());
        }
        if name.eq_ignore_ascii_case(b"scl") {
            self.pending_server_actions.push_back(CgameServerAction::SiegeClassSelect);
            return Ok(());
        }
        if name.eq_ignore_ascii_case(b"spc") {
            self.pending_server_actions.push_back(CgameServerAction::SiegeProfile);
            return Ok(());
        }
        if name.eq_ignore_ascii_case(b"sxd") {
            self.pending_server_actions.push_back(CgameServerAction::SiegeExtendedData(
                parts.get(1..).unwrap_or_default().join(&b' '),
            ));
            return Ok(());
        }
        if name.eq_ignore_ascii_case(b"map_restart") {
            self.notices.push_back(CgameNotice::MapRestart);
            return Ok(());
        }
        if name.eq_ignore_ascii_case(b"ircg") {
            // OpenJK CG_RestoreClientGhoul_f: ircg <client> <body> <weapon> <side>.
            // Keep this byte/token parser aligned with the protocol command; do
            // not reconstruct corpse equipment from snapshot guesses.
            let source_client = parts.get(1).copied().and_then(parse_i32_ascii);
            let body_entity = parts.get(2).copied().and_then(parse_i32_ascii);
            let known_weapon = parts.get(3).copied().and_then(parse_i32_ascii);
            let light_side = parts.get(4).copied().and_then(parse_i32_ascii).unwrap_or(0) != 0;
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
            let Some(source_client) = parts.get(1).copied().and_then(parse_i32_ascii) else {
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
        self.set_configstring(index, value);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MatchLimits {
    pub timelimit: i32,
    pub fraglimit: i32,
    pub level_start_time: i32,
    pub scores1: i32,
    pub scores2: i32,
}

fn parse_pipe_numbers<const N: usize>(value: &[u8]) -> [i32; N] {
    let mut out = [-1; N];
    for (slot, field) in out.iter_mut().zip(value.split(|byte| *byte == b'|')) {
        if let Some(number) = parse_i32_ascii(field) {
            *slot = number;
        }
    }
    out
}

fn parse_duelist_healths(value: &[u8]) -> [i32; 3] {
    let mut fields = value.split(|byte| *byte == b'|');
    let first = fields.next().and_then(parse_i32_ascii).unwrap_or(0);
    let second = fields.next().and_then(parse_i32_ascii).unwrap_or(0);
    let third = fields.next().map_or(-1, |field| {
        if field.first() == Some(&b'!') { -1 } else { parse_i32_ascii(field).unwrap_or(0) }
    });
    [first, second, third]
}

fn parse_shader_state(value: &[u8]) -> Vec<ShaderRemap> {
    // CS_SHADERSTATE is `old=new:time@old2=new2:time2@...`. Shader qpaths and
    // time offsets are protocol ASCII; preserving malformed entries by simply
    // ignoring them matches CG_ShaderStateChanged's break/continue behavior.
    value
        .split(|byte| *byte == b'@')
        .filter_map(|entry| {
            if entry.is_empty() { return None; }
            let equals = entry.iter().position(|byte| *byte == b'=')?;
            let tail = &entry[equals + 1..];
            let colon = tail.iter().position(|byte| *byte == b':')?;
            let from = bytes_to_lossless_ascii(&entry[..equals]);
            let to = bytes_to_lossless_ascii(&tail[..colon]);
            let time_offset = bytes_to_lossless_ascii(&tail[colon + 1..]);
            (!from.is_empty() && !to.is_empty()).then_some(ShaderRemap { from, to, time_offset })
        })
        .collect()
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

/// jaPRO `SABER_RGB`: the blade colour comes from `cp_sbRGB1/2` (`c3/c4`).
pub const SABER_RGB: i32 = 6;
/// jaPRO/TaystJK `SABER_BLACK`.
pub const SABER_BLACK: i32 = 11;
/// Set on a resolved blade colour that carries a packed `0xBBGGRR` in its low 24
/// bits, so every blade/trail/light path can take one integer as before.
pub const SABER_CUSTOM_RGB: i32 = 1 << 24;

/// RGB of a resolved custom blade colour (`SABER_CUSTOM_RGB`), if it is one.
pub fn custom_saber_rgb(color: i32) -> Option<[u8; 3]> {
    (color & SABER_CUSTOM_RGB != 0).then(|| [(color & 255) as u8, ((color >> 8) & 255) as u8, ((color >> 16) & 255) as u8])
}

/// jaPRO ClampSaberColor + CG_NewClientInfo's rgb handling: the effective blade
/// colour for the protocol `c1`/`c2` value and its `c3`/`c4` RGB.
pub fn resolve_saber_color(color: i32, packed_rgb: i32, rgb_capable: bool) -> i32 {
    let color = color.rem_euclid(12);
    match color {
        // Flame/electric variants draw as tinted RGB blades in jaPRO too.
        SABER_RGB..=10 if rgb_capable => {
            // CG_NewClientInfo: an unset colour is 255, i.e. pure red.
            let packed = if packed_rgb & 0xFF_FFFF == 0 { 255 } else { packed_rgb & 0xFF_FFFF };
            SABER_CUSTOM_RGB | packed
        }
        // Servers without RGB support roll the extended colours back to the base six.
        SABER_RGB..=10 => color - SABER_RGB,
        _ => color,
    }
}

/// TaystJK `ClampSaberColor`: jaPRO plugin-disable bit 3 makes a black saber
/// render as orange. The server does not consume this bit; it is purely a
/// client presentation preference.
pub fn apply_plugin_saber_color(color: i32, plugin_disable: i32) -> i32 {
    if color == SABER_BLACK
        && plugin_disable & crate::japro_cg::plugin_disable::BLACK_SABERS_DISABLE != 0
    {
        1 // SABER_ORANGE
    } else {
        color
    }
}

/// OpenJK/TaystJK `BG_IsValidCharacterModel`: reject bundled non-MP skins.
pub fn is_valid_character_model(model_name: &str, skin_name: &str) -> bool {
    if skin_name.eq_ignore_ascii_case("menu") {
        return false;
    }
    if model_name.eq_ignore_ascii_case("kyle")
        && matches!(skin_name.to_ascii_lowercase().as_str(), "fpls" | "fpls2" | "fpls3")
    {
        return false;
    }
    true
}

/// Port of TaystJK/OpenJK `BG_ValidateSkinForTeam`.
///
/// `file_exists` is the JKA virtual filesystem check used for custom `_red`/`_blue` skins.
/// The returned RGB value is `clientInfo_t::colorOverride` for `jedi_*` / RGB models.
pub fn validate_skin_for_team<F>(
    model_name: &str,
    skin_name: &mut String,
    team: i32,
    mut file_exists: F,
) -> Option<[u8; 3]>
where
    F: FnMut(&str) -> bool,
{
    let red_path = format!("models/players/{model_name}/model_red.skin");
    let blue_path = format!("models/players/{model_name}/model_blue.skin");
    let jedi_custom = model_name.len() > 5
        && model_name.as_bytes().get(..5).is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"jedi_"));
    let rgb_without_team_variants = skin_name.as_bytes()
        .get(..3)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"rgb"))
        && (!file_exists(&red_path) || !file_exists(&blue_path));

    if jedi_custom || rgb_without_team_variants {
        return match team {
            TEAM_RED => Some([255, 0, 0]),
            TEAM_BLUE => Some([0, 0, 255]),
            _ => None,
        };
    }

    let (team_name, opponent_name, suffix) = match team {
        TEAM_RED => ("red", "blue", "_red"),
        TEAM_BLUE => ("blue", "red", "_blue"),
        _ => return None,
    };
    if skin_name.eq_ignore_ascii_case(team_name) {
        return None;
    }

    if skin_name.eq_ignore_ascii_case(opponent_name)
        || skin_name.eq_ignore_ascii_case("default")
        || skin_name.contains('|')
        || !is_valid_character_model(model_name, skin_name)
    {
        *skin_name = team_name.to_owned();
        return None;
    }

    let suffix_word = suffix.trim_start_matches('_');
    if !skin_name.ends_with(suffix_word) {
        if skin_name.len() + suffix.len() >= MAX_QPATH {
            *skin_name = team_name.to_owned();
            return None;
        }
        skin_name.push_str(suffix);
    }
    let candidate = format!("models/players/{model_name}/model_{skin_name}.skin");
    if !file_exists(&candidate) {
        *skin_name = team_name.to_owned();
    }
    None
}

/// TaystJK `CG_AddSaberBlade` team-color normalization. With cg_saberTeamColors=0,
/// vanilla still rejects the opposing base color and extended/custom colors.
pub fn team_saber_color(info: &ClientInfo, color: i32, saber_team_colors: bool) -> i32 {
    if info.gametype < GT_TEAM || info.gametype == GT_SIEGE || info.jedi_v_merc {
        return color;
    }
    match info.team {
        TEAM_RED if saber_team_colors || color == SABER_BLUE || color > SABER_PURPLE => SABER_RED,
        TEAM_BLUE if saber_team_colors || color == SABER_RED || color > SABER_PURPLE => SABER_BLUE,
        _ => color,
    }
}

/// Integer fields every `scores` record carries (stock OpenJK layout).
const SCORE_FIELDS: usize = 14;
/// jaPRO's extended record adds a trailing deaths field.
const SCORE_FIELDS_JAPRO: usize = 15;

/// Per-client stride of a `scores` command (`token_count` includes the command
/// name). The mod decides it, as in jaPRO's CG_ParseScores. When the token
/// count exactly fits only the other layout - e.g. a mod that sends the extended
/// record to a client it did not detect as jaPRO - trust the wire instead.
fn score_record_stride(extended_deaths: bool, count: usize, token_count: usize) -> usize {
    let (preferred, other) = if extended_deaths {
        (SCORE_FIELDS_JAPRO, SCORE_FIELDS)
    } else {
        (SCORE_FIELDS, SCORE_FIELDS_JAPRO)
    };
    let body = token_count.saturating_sub(4);
    if count == 0 || body == count * preferred || body != count * other {
        preferred
    } else {
        other
    }
}

pub fn bytes_to_lossless_ascii(value: &[u8]) -> String {
    value.iter().map(|&byte| byte as char).collect()
}

/// Convert Dinurdo's lossless one-byte JKA text representation back to protocol/
/// font bytes. Rust strings are UTF-8, but JKA text and its 256-glyph fonts are
/// byte indexed; using `str::as_bytes()` turns e.g. byte 0xA4 into UTF-8 C2 A4
/// and renders/sends two bogus glyphs. Characters outside the legacy 0..255
/// range have no stock JKA glyph and are replaced rather than corrupting the
/// following text.
pub fn text_to_jka_bytes(value: &str) -> Vec<u8> {
    value
        .chars()
        .map(|ch| u8::try_from(ch as u32).unwrap_or(b'?'))
        .collect()
}

/// Unicode characters Windows produces for the old no-leading-zero ALT codes.
/// JKA itself is byte-indexed, so map those semantic CP437 characters back to
/// the exact 0x80..0xFF byte the original client would have queued. This is
/// deliberately only for *typed/pasted input*; strings decoded from the JKA
/// protocol already use U+00xx as a lossless raw-byte container.
const CP437_HIGH_UNICODE: [u32; 128] = [
    0x00C7, 0x00FC, 0x00E9, 0x00E2, 0x00E4, 0x00E0, 0x00E5, 0x00E7, 0x00EA, 0x00EB, 0x00E8, 0x00EF, 0x00EE, 0x00EC, 0x00C4, 0x00C5,
    0x00C9, 0x00E6, 0x00C6, 0x00F4, 0x00F6, 0x00F2, 0x00FB, 0x00F9, 0x00FF, 0x00D6, 0x00DC, 0x00A2, 0x00A3, 0x00A5, 0x20A7, 0x0192,
    0x00E1, 0x00ED, 0x00F3, 0x00FA, 0x00F1, 0x00D1, 0x00AA, 0x00BA, 0x00BF, 0x2310, 0x00AC, 0x00BD, 0x00BC, 0x00A1, 0x00AB, 0x00BB,
    0x2591, 0x2592, 0x2593, 0x2502, 0x2524, 0x2561, 0x2562, 0x2556, 0x2555, 0x2563, 0x2551, 0x2557, 0x255D, 0x255C, 0x255B, 0x2510,
    0x2514, 0x2534, 0x252C, 0x251C, 0x2500, 0x253C, 0x255E, 0x255F, 0x255A, 0x2554, 0x2569, 0x2566, 0x2560, 0x2550, 0x256C, 0x2567,
    0x2568, 0x2564, 0x2565, 0x2559, 0x2558, 0x2552, 0x2553, 0x256B, 0x256A, 0x2518, 0x250C, 0x2588, 0x2584, 0x258C, 0x2590, 0x2580,
    0x03B1, 0x00DF, 0x0393, 0x03C0, 0x03A3, 0x03C3, 0x00B5, 0x03C4, 0x03A6, 0x0398, 0x03A9, 0x03B4, 0x221E, 0x03C6, 0x03B5, 0x2229,
    0x2261, 0x00B1, 0x2265, 0x2264, 0x2320, 0x2321, 0x00F7, 0x2248, 0x00B0, 0x2219, 0x00B7, 0x221A, 0x207F, 0x00B2, 0x25A0, 0x00A0,
];

pub fn unicode_input_char_to_jka_char(ch: char) -> char {
    let code = ch as u32;
    if code < 0x80 {
        return ch;
    }
    if let Some(index) = CP437_HIGH_UNICODE.iter().position(|&candidate| candidate == code) {
        return char::from(0x80u8 + index as u8);
    }
    // Leading-zero Windows ALT codes and Latin-1 paste paths can already carry
    // the desired raw 8-bit value. Preserve those when CP437 has no match.
    char::from(u8::try_from(code).unwrap_or(b'?'))
}

pub fn unicode_input_to_jka_text(value: &str) -> String {
    value.chars().map(unicode_input_char_to_jka_char).collect()
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

/// A `trajectory_t`.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Trajectory {
    pub(crate) kind: i32,
    pub(crate) time: i32,
    pub(crate) duration: i32,
    pub(crate) base: [f32; 3],
    pub(crate) delta: [f32; 3],
}

impl Trajectory {
    /// BG_EvaluateTrajectory; a trajectory OpenJK would `Com_Error` on stays at its base.
    pub(crate) fn evaluate(self, at_time: i32) -> [f32; 3] {
        evaluate_trajectory(self, at_time).unwrap_or(self.base)
    }

    /// The `pos` or `apos` trajectory of an entity state.
    pub(crate) fn of(entity: &EntityState, prefix: &str) -> Self {
        trajectory(entity, prefix)
    }
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

/// BG_EvaluateTrajectory of an entity's `pos` or `apos` at `time`. `None` for a
/// trajectory type OpenJK would `Com_Error` on.
pub(crate) fn evaluate_entity_trajectory(state: &EntityState, prefix: &str, time: i32) -> Option<[f32; 3]> {
    evaluate_trajectory(trajectory(state, prefix), time).ok()
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

/// TaystJK `CG_AddCEntity` intermission visibility gate. Keep this shared by
/// the generic entity and Ghoul2 player presenters so the Rust split does not
/// accidentally render something that the original single dispatcher skipped.
pub(crate) fn suppressed_during_intermission(
    intermission: bool,
    entity: &PresentedEntity,
) -> bool {
    intermission
        && match entity.entity_type {
            ET_GENERAL | ET_PLAYER | ET_INVISIBLE => true,
            ET_NPC => entity.state.field_i32("NPC_class").unwrap_or(0) == CLASS_VEHICLE,
            _ => false,
        }
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

/// Presentation-only interpolation for auxiliary playerState sources such as
/// race ghosts. This intentionally reuses the exact predicted-player conversion
/// instead of maintaining a second animation/state translation table.
pub(crate) fn presented_player_state_interpolated(
    current: &PlayerState,
    next: Option<&PlayerState>,
    alpha: f32,
) -> Option<PresentedEntity> {
    presented_player_state(current, next, alpha)
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
    fn tayst_server_command_table_preserves_team_ghoul_audio_and_remap_state() {
        let mut game = ClientGameState::new();
        game.execute_server_command(&ServerCommand {
            sequence: 78,
            text: b"tinfo 2 3 11 100 50 3 4 7 22 75 25 5 8".to_vec(),
        })
        .unwrap();
        assert_eq!(
            game.team_info(),
            &[
                TeamInfoEntry { client: 3, location: 11, health: 100, armor: 50, weapon: 3, powerups: 4 },
                TeamInfoEntry { client: 7, location: 22, health: 75, armor: 25, weapon: 5, powerups: 8 },
            ]
        );

        game.execute_server_command(&ServerCommand { sequence: 79, text: b"kg2 71 72".to_vec() }).unwrap();
        assert_eq!(
            game.drain_ghoul2_commands(),
            vec![Ghoul2ServerCommand::KillEntity { entity: 71 }, Ghoul2ServerCommand::KillEntity { entity: 72 }]
        );

        game.execute_server_command(&ServerCommand { sequence: 80, text: b"kls 71 72".to_vec() }).unwrap();
        assert_eq!(
            game.drain_audio_server_actions(),
            vec![CgameServerAction::KillLoopSounds { entities: vec![71, 72] }]
        );

        game.execute_server_command(&ServerCommand {
            sequence: 81,
            text: b"remapShader textures/old textures/new 1250".to_vec(),
        })
        .unwrap();
        assert_eq!(game.shader_remaps()["textures/old"].to, "textures/new");
        assert_eq!(
            game.drain_server_actions(),
            vec![CgameServerAction::ShaderRemap(ShaderRemap {
                from: "textures/old".to_owned(),
                to: "textures/new".to_owned(),
                time_offset: "1250".to_owned(),
            })]
        );
    }

    #[test]
    fn configstring_modified_tracks_duel_siege_shader_and_music_state() {
        let mut game = ClientGameState::new();
        game.set_configstring(CS_CLIENT_JEDIMASTER, b"9".to_vec());
        game.set_configstring(CS_CLIENT_DUELWINNER, b"4".to_vec());
        game.set_configstring(CS_CLIENT_DUELISTS, b"4|8|12".to_vec());
        game.set_configstring(CS_CLIENT_DUELHEALTHS, b"100|67|!".to_vec());
        game.set_configstring(CS_FLAGSTATUS, b"20".to_vec());
        game.set_configstring(CS_SIEGE_STATE, b"round-live".to_vec());
        game.set_configstring(CS_SIEGE_OBJECTIVES, b"obj-data".to_vec());
        game.set_configstring(CS_SIEGE_TIMEOVERRIDE, b"45000".to_vec());
        game.set_configstring(CS_SIEGE_WINTEAM, b"2".to_vec());
        game.set_configstring(
            CS_SHADERSTATE,
            b"textures/a=textures/b:0@textures/c=textures/d:1.5@".to_vec(),
        );
        game.set_configstring(CS_MUSIC, b"music/intro.mp3 music/loop.mp3".to_vec());

        assert_eq!(game.jedi_master(), 9);
        assert_eq!(game.duel_winner(), 4);
        assert_eq!(game.duelists(), [4, 8, 12]);
        assert_eq!(game.duelist_healths(), [100, 67, -1]);
        assert_eq!(game.flag_status(), [4, 0]);
        assert_eq!(game.siege_state(), b"round-live");
        assert_eq!(game.siege_objectives(), b"obj-data");
        assert_eq!(game.siege_time_override(), 45000);
        assert_eq!(game.siege_win_team(), 2);
        assert_eq!(game.shader_remaps().len(), 2);
        assert_eq!(game.shader_remaps()["textures/c"].time_offset, "1.5");

        let audio = game.drain_audio_server_actions();
        assert_eq!(audio, vec![CgameServerAction::RestartMapMusic]);
        let remaining = game.drain_server_actions();
        assert_eq!(
            remaining.iter().filter(|action| matches!(action, CgameServerAction::ShaderRemap(_))).count(),
            2
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
        assert_eq!(entries[0].deaths, None);
    }

    #[test]
    fn japro_scores_use_15_int_records_with_trailing_deaths() {
        let mut game = ClientGameState::new();
        game.configstrings.insert(CS_SERVERINFO, br"\gamename\japro 1.4".to_vec());
        game.configstrings.insert(CS_PLAYERS + 2, br"\n\Alice\t\0".to_vec());
        game.configstrings.insert(CS_PLAYERS + 5, br"\n\Bob\t\0".to_vec());
        game.execute_server_command(&ServerCommand {
            sequence: 81,
            text: b"scores 2 0 0 \
                    2 42 55 7 0 0 0 0 0 0 0 0 0 0 9 \
                    5 10 30 3 0 0 0 0 0 0 0 0 0 0 4"
                .to_vec(),
        })
        .unwrap();

        let notices = game.drain_notices();
        let CgameNotice::Scores { entries, .. } = &notices[0] else {
            panic!("expected scores notice");
        };
        assert_eq!(entries.len(), 2);
        assert_eq!((entries[0].client, entries[0].name.as_str(), entries[0].score), (2, "Alice", 42));
        assert_eq!((entries[1].client, entries[1].name.as_str(), entries[1].score), (5, "Bob", 10));
        assert_eq!(entries[0].deaths, Some(9));
        assert_eq!(entries[1].deaths, Some(4));
    }

    #[test]
    fn rgb_saber_colours_resolve_like_japro_client_info() {
        // 0x0000FF packed = pure red channel 255.
        let rgb = SABER_CUSTOM_RGB | 0x00_FF80;
        assert_eq!(resolve_saber_color(6, 0x00_FF80, true), rgb);
        assert_eq!(custom_saber_rgb(rgb), Some([0x80, 0xFF, 0x00]));
        // An unset colour reads as pure red, like CG_NewClientInfo.
        assert_eq!(custom_saber_rgb(resolve_saber_color(6, 0, true)), Some([255, 0, 0]));
        // Stock colours pass through; servers without RGB roll back to the base six.
        assert_eq!(resolve_saber_color(3, 123, true), 3);
        assert_eq!(resolve_saber_color(6, 123, false), 0);
        assert_eq!(resolve_saber_color(9, 123, false), 3);

        let mut game = ClientGameState::new();
        game.configstrings.insert(CS_SERVERINFO, br"\gamename\japro 1.4".to_vec());
        game.configstrings.insert(CS_PLAYERS, br"\n\Rgb\t\0\c1\6\c2\4\c3\65280\c4\0".to_vec());
        let info = game.client_info(0, &[]).unwrap();
        assert_eq!(custom_saber_rgb(info.saber_color), Some([0, 255, 0]));
        assert_eq!(info.saber2_color, 4);
        game.configstrings.insert(CS_SERVERINFO, br"\gamename\basejka".to_vec());
        assert_eq!(game.client_info(0, &[]).unwrap().saber_color, 0);
    }

    #[test]
    fn client_name_tracks_the_players_configstring() {
        let mut game = ClientGameState::new();
        assert_eq!(game.client_name(3), None);
        game.configstrings.insert(CS_PLAYERS + 3, br"\n\^1Old\t\0".to_vec());
        assert_eq!(game.client_name(3).as_deref(), Some("^1Old"));
        // A configstring update is visible on the next read, no rescan needed.
        game.configstrings.insert(CS_PLAYERS + 3, br"\n\New\t\0".to_vec());
        assert_eq!(game.client_name(3).as_deref(), Some("New"));
    }

    #[test]
    fn score_stride_follows_wire_when_mod_guess_does_not_fit() {
        assert_eq!(score_record_stride(true, 3, 4 + 3 * 15), 15);
        assert_eq!(score_record_stride(false, 3, 4 + 3 * 14), 14);
        // Extended records from a server that was not detected as jaPRO.
        assert_eq!(score_record_stride(false, 3, 4 + 3 * 15), 15);
        // Truncated command: fall back to the mod's layout.
        assert_eq!(score_record_stride(true, 20, 4 + 7 * 15), 15);
        assert_eq!(score_record_stride(false, 0, 4), 14);
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
    fn intermission_entity_suppression_matches_taystjk_dispatch() {
        let presented = |entity_type: i32, npc_class: i32| {
            let state = entity(
                1,
                &[("eType", entity_type as u32), ("NPC_class", npc_class as u32)],
            );
            PresentedEntity {
                number: 1,
                entity_type,
                origin: [0.0; 3],
                angles: [0.0; 3],
                state,
            }
        };

        assert!(suppressed_during_intermission(true, &presented(ET_GENERAL, 0)));
        assert!(suppressed_during_intermission(true, &presented(ET_PLAYER, 0)));
        assert!(suppressed_during_intermission(true, &presented(ET_INVISIBLE, 0)));
        assert!(suppressed_during_intermission(true, &presented(ET_NPC, CLASS_VEHICLE)));
        assert!(!suppressed_during_intermission(true, &presented(ET_NPC, 0)));
        assert!(!suppressed_during_intermission(true, &presented(ET_MOVER, 0)));
        assert!(!suppressed_during_intermission(false, &presented(ET_PLAYER, 0)));
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
            ping: 0,
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
    fn reprediction_jitter_does_not_replay_a_played_event() {
        let mut game = ClientGameState::new();
        let base = predicted_ps(3, 0, 0, 0, 0, 0);
        let stepped = predicted_ps(3, 1, EntityEvent::EV_STEP_8.as_i32(), 0, 0, 0);
        game.transition_predicted_player_state(&stepped, &base, 1_000).unwrap();
        assert_eq!(game.drain_presentation_events().len(), 1);

        // The next provisional command does not reproduce the event, the one
        // after does again: the presented state flickers, the event played once.
        game.transition_predicted_player_state(&base, &stepped, 1_004).unwrap();
        game.transition_predicted_player_state(&stepped, &base, 1_008).unwrap();
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
            ping: 0,
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
    fn plugin_disable_black_saber_clamps_only_black_to_orange() {
        use crate::japro_cg::plugin_disable;

        assert_eq!(apply_plugin_saber_color(SABER_BLACK, 0), SABER_BLACK);
        assert_eq!(
            apply_plugin_saber_color(SABER_BLACK, plugin_disable::BLACK_SABERS_DISABLE),
            1
        );
        assert_eq!(
            apply_plugin_saber_color(2, plugin_disable::BLACK_SABERS_DISABLE),
            2
        );
    }

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
    fn team_skin_validation_matches_openjk_fallbacks() {
        let mut skin = "default".to_owned();
        assert_eq!(validate_skin_for_team("kyle", &mut skin, TEAM_RED, |_| true), None);
        assert_eq!(skin, "red");

        let mut skin = "custom".to_owned();
        assert_eq!(validate_skin_for_team("reelo", &mut skin, TEAM_BLUE, |path| {
            path.ends_with("model_custom_blue.skin")
        }), None);
        assert_eq!(skin, "custom_blue");

        let mut skin = "rgb_default".to_owned();
        assert_eq!(
            validate_skin_for_team("custom", &mut skin, TEAM_BLUE, |_| false),
            Some([0, 0, 255])
        );
        assert_eq!(skin, "rgb_default");

        let mut skin = "head_a1|torso_a1|lower_a1".to_owned();
        assert_eq!(
            validate_skin_for_team("jedi_hm", &mut skin, TEAM_RED, |_| false),
            Some([255, 0, 0])
        );
        assert_eq!(skin, "head_a1|torso_a1|lower_a1");
    }

    #[test]
    fn team_saber_colors_match_taystjk_rules_and_exceptions() {
        let red = ClientInfo {
            team: TEAM_RED,
            gametype: GT_TEAM,
            ..ClientInfo::solo_kyle()
        };
        assert_eq!(team_saber_color(&red, SABER_BLUE, true), SABER_RED);
        assert_eq!(team_saber_color(&red, 3, true), SABER_RED);
        assert_eq!(team_saber_color(&red, SABER_BLUE, false), SABER_RED);
        assert_eq!(team_saber_color(&red, 3, false), 3);

        let siege = ClientInfo { gametype: GT_SIEGE, ..red.clone() };
        assert_eq!(team_saber_color(&siege, SABER_BLUE, true), SABER_BLUE);
        let jedi_v_merc = ClientInfo { jedi_v_merc: true, ..red };
        assert_eq!(team_saber_color(&jedi_v_merc, SABER_BLUE, true), SABER_BLUE);
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
            forced_saber_color: Some(0),
            forced_saber2_color: Some(4),
        }];
        let info = game.client_info(3, &classes).unwrap();
        assert_eq!(info.model_name, "jedi_hm");
        assert_eq!(info.skin_name, "head_a1|torso_a1|lower_a1");
        assert_eq!(info.saber_color, SABER_RED);
        assert_eq!(info.saber2_color, SABER_BLUE);
        assert_eq!(
            info.skin_qpath(),
            "models/players/jedi_hm/|head_a1|torso_a1|lower_a1"
        );
    }
}
