//! The game-side half of the solo shim for brush entities: a port of the
//! OpenJK MP server logic that makes doors, plats, buttons and their triggers
//! work (g_mover.c, g_trigger.c, g_target.c, G_TouchTriggers, TryUse).
//!
//! `movers.rs` spawns each brush entity in its rest state; this module owns it
//! afterwards. It runs on the local player's fixed Pmove timeline through the
//! [`SoloGame`] hook, so a mover, the player it carries and the Pmove step that
//! traces against it always agree on where everything is:
//!
//! * `G_RunFrame`: every mover evaluates its trajectory at the new time and
//!   `G_MoverPush`es the player out of the way (or carries it), blocked movers
//!   stall and reverse, and think functions fire.
//! * `ClientThink_real` after Pmove: entities the player bumped, the use key,
//!   and `G_TouchTriggers` contact with trigger brushes and the boxes doors
//!   and plats spawn around themselves.
//!
//! Open doors also open their areaportals, which the snapshot's areamask
//! reports to the renderer.
//!
//! Door and plat sounds follow `G_PlayDoorSound` / `G_PlayDoorLoopSound`: the
//! map's `soundSet` is registered as a `CS_AMBIENT_SET` configstring, movers
//! post `EV_PLAYDOORSOUND` and loop the set's middle sound while they travel.
//!
//! Left out because the solo game has no damage model: crush and pain damage,
//! train sounds, and the ICARUS script hooks.

use jka_assets::bsp::AreaLocator;
use jka_movement::{
    CollisionWorld, EntityClip, PlayerState, PlayerView, TraceQuery, TraceWorld,
};
use jka_protocol::{gamestate::EntityState, server::PlayerState as ProtocolPlayerState};

use jka_protocol::entity_event::{EntityEvent, EVENT_VALID_MSEC, EV_EVENT_BITS};

use super::{
    movers::{angle_forward, set_movedir, spawns_in_local_gametype, sscanf_vec3, SpawnVars, Train},
    set_entity_i32, set_entity_vec3,
};
use crate::{
    cgame::Trajectory,
    net::{PredictionWorld, SolidEntity},
    player::SoloGame,
    scene::MapBrushEntity,
};

type V3 = [f32; 3];

const MAX_GENTITIES: usize = 1024;
const ENTITYNUM_NONE: i32 = 1023;
/// Number given to game entities the client never sees.
const LOGIC_ENTITY_NUMBER: i32 = ENTITYNUM_NONE - 1;
/// g_local.h FRAMETIME.
const FRAMETIME: i32 = 100;
const USE_DISTANCE: f32 = 64.0;

/// `BMS_START` / `BMS_MID` / `BMS_END`: the stage of a brush-model sound set.
const BMS_START: i32 = 0;
const BMS_MID: i32 = 1;
const BMS_END: i32 = 2;

const TR_STATIONARY: i32 = 0;
const TR_LINEAR_STOP: i32 = 3;
const TR_NONLINEAR_STOP: i32 = 4;
const TR_SINE: i32 = 5;

const MASK_PLAYERSOLID: i32 = 0x1 | 0x10 | 0x100 | 0x1000;
/// SOLID | BODY | CORPSE: what TryUse's trace stops at.
const MASK_USE: i32 = 0x1 | 0x100 | 0x200;
const SOLID_BMODEL: i32 = 0x00ff_ffff;

const BUTTON_ATTACK: i32 = 1;
const BUTTON_ALT_ATTACK: i32 = 128;
const BUTTON_USE: i32 = 32;

// func_door / func_plat / func_button spawnflags.
const MOVER_START_OPEN: i32 = 1;
const MOVER_FORCE_ACTIVATE: i32 = 2;
const MOVER_CRUSHER: i32 = 4;
const MOVER_TOGGLE: i32 = 8;
const MOVER_LOCKED: i32 = 16;
const MOVER_PLAYER_USE: i32 = 64;
const MOVER_INACTIVE: i32 = 128;

// trigger_multiple spawnflags.
const TRIGGER_CLIENTONLY: i32 = 1;
const TRIGGER_FACING: i32 = 2;
const TRIGGER_USE_BUTTON: i32 = 4;
const TRIGGER_FIRE_BUTTON: i32 = 8;
const TRIGGER_NPCONLY: i32 = 16;
const TRIGGER_INACTIVE: i32 = 128;
const TRIGGER_MULTIPLE: i32 = 2048;

const TARGET_RELAY_TEAM_ONLY: i32 = 1 | 2;
const TARGET_RELAY_RANDOM: i32 = 4;
const TARGET_DELAY_NO_RETRIGGER: i32 = 1;

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn length(a: V3) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// RadiusFromBounds.
fn radius_from_bounds(mins: V3, maxs: V3) -> f32 {
    length(std::array::from_fn(|axis| mins[axis].abs().max(maxs[axis].abs())))
}

fn overlaps(a_min: V3, a_max: V3, b_min: V3, b_max: V3) -> bool {
    (0..3).all(|axis| a_min[axis] <= b_max[axis] && a_max[axis] >= b_min[axis])
}

/// q_math AngleVectors.
fn angle_vectors(angles: V3) -> [V3; 3] {
    let (sy, cy) = angles[1].to_radians().sin_cos();
    let (sp, cp) = angles[0].to_radians().sin_cos();
    let (sr, cr) = angles[2].to_radians().sin_cos();
    [
        [cp * cy, cp * sy, -sp],
        [-sr * sp * cy + cr * sy, -sr * sp * sy - cr * cy, -sr * cp],
        [cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp],
    ]
}

/// The rotation G_TryPushingEntity applies to a rider's offset from the pusher
/// (G_CreateRotationMatrix, transposed by G_TransposeMatrix).
fn rotate_by_pusher(amove: V3, point: V3) -> V3 {
    let [forward, right, up] = angle_vectors(amove);
    let matrix = [forward, right.map(|v| -v), up];
    std::array::from_fn(|k| matrix[0][k] * point[0] + matrix[1][k] * point[1] + matrix[2][k] * point[2])
}

fn angle_to_short(angle: f32) -> i32 {
    ((angle * (65536.0 / 360.0)) as i32) & 65535
}

/// `r.absmin` / `r.absmax` of a linked entity: the one-unit pad, or the bounding
/// sphere's box when the entity is rotated (SV_LinkEntity).
fn abs_bounds(origin: V3, angles: V3, mins: V3, maxs: V3) -> (V3, V3) {
    if angles != [0.0; 3] {
        let radius = radius_from_bounds(mins, maxs);
        (origin.map(|v| v - radius - 1.0), origin.map(|v| v + radius + 1.0))
    } else {
        (
            std::array::from_fn(|axis| origin[axis] + mins[axis] - 1.0),
            std::array::from_fn(|axis| origin[axis] + maxs[axis] + 1.0),
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    Door,
    Plat,
    Button,
    Train,
    /// A brush entity with a trajectory but no game logic (func_static, ...).
    Brush,
    TriggerMulti,
    /// The box `Think_SpawnNewDoorTrigger` puts around a door.
    TriggerDoor,
    /// The box `SpawnPlatTrigger` puts over a plat's rest position.
    TriggerPlat,
    Relay,
    Delay,
    Activate,
    Deactivate,
    Counter,
    Always,
}

impl Class {
    fn is_binary(self) -> bool {
        matches!(self, Class::Door | Class::Plat | Class::Button)
    }

    fn is_mover(self) -> bool {
        matches!(self, Class::Door | Class::Plat | Class::Button | Class::Train | Class::Brush)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MoverState {
    Pos1,
    Pos2,
    OneToTwo,
    TwoToOne,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Think {
    None,
    ReturnToPos1,
    UseBinaryMoverGo,
    MatchTeam,
    SpawnDoorTrigger,
    MultiWait,
    MultiTriggerRun,
    ClearedFire,
    TargetDelay,
    Always,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Touch {
    None,
    Button,
    Plat,
    PlatCenter,
    DoorTrigger,
    Multi,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Activator {
    None,
    Player,
    Entity(usize),
}

/// One `gentity_t`.
struct GEnt {
    number: i32,
    class: Class,
    targetname: Option<String>,
    target: Option<String>,
    target2: Option<String>,
    opentarget: Option<String>,
    closetarget: Option<String>,
    team: Option<String>,
    spawnflags: i32,
    /// FL_INACTIVE.
    inactive: bool,
    /// `ent->use` is set.
    can_use: bool,
    /// SVF_PLAYER_USABLE.
    player_usable: bool,
    touch: Touch,
    // Geometry (`r.*`).
    model: u32,
    mins: V3,
    maxs: V3,
    origin: V3,
    angles: V3,
    /// `r.contents & CONTENTS_TRIGGER`.
    trigger: bool,
    /// Index into `MoverGame::solids` while the brush model is solid.
    solid: Option<usize>,
    /// Trigger `movedir`, for FACING.
    movedir: V3,
    // Trajectories (`s.pos` / `s.apos`).
    pos: Trajectory,
    apos: Trajectory,
    pos1: V3,
    pos2: V3,
    mover_state: MoverState,
    speed: f32,
    wait: f32,
    random: f32,
    delay: f32,
    /// `ent->count`: a door trigger's thinnest axis, a counter's remaining uses.
    count: i32,
    bounce_count: i32,
    initial_count: i32,
    /// The "linear" key: constant-speed movement.
    linear: bool,
    // Teams.
    team_master: usize,
    team_chain: Option<usize>,
    slave: bool,
    parent: Option<usize>,
    /// SV_LinkEntity's `areanum` / `areanum2` at the rest position.
    areas: [Option<usize>; 2],
    // Thinking.
    think: Think,
    nextthink: i32,
    /// Offset from the first frame's time for a think set up at spawn.
    spawn_think: i32,
    activator: Activator,
    pain_debounce: i32,
    aim_debounce: i32,
    /// `usetime`: how long a USE_BUTTON trigger must be held, and since when.
    use_time: i32,
    hold_until: Option<i32>,
    hold_touch: i32,
    state: Option<EntityState>,
    train: Option<Train>,
    /// The `soundSet` key: a `bmodelSet` of sound/sound.txt.
    sound_set: Option<String>,
    /// `eventTime`: when `G_AddEvent` last posted an event on `state`.
    event_time: i32,
}

impl GEnt {
    fn blank(number: i32, class: Class) -> Self {
        Self {
            number,
            class,
            targetname: None,
            target: None,
            target2: None,
            opentarget: None,
            closetarget: None,
            team: None,
            spawnflags: 0,
            inactive: false,
            can_use: false,
            player_usable: false,
            touch: Touch::None,
            model: 0,
            mins: [0.0; 3],
            maxs: [0.0; 3],
            origin: [0.0; 3],
            angles: [0.0; 3],
            trigger: false,
            solid: None,
            movedir: [0.0; 3],
            pos: Trajectory::default(),
            apos: Trajectory::default(),
            pos1: [0.0; 3],
            pos2: [0.0; 3],
            mover_state: MoverState::Pos1,
            speed: 0.0,
            wait: 0.0,
            random: 0.0,
            delay: 0.0,
            count: 0,
            bounce_count: 0,
            initial_count: 0,
            linear: false,
            team_master: 0,
            team_chain: None,
            slave: false,
            parent: None,
            areas: [None, None],
            think: Think::None,
            nextthink: 0,
            spawn_think: 0,
            activator: Activator::None,
            pain_debounce: 0,
            aim_debounce: 0,
            use_time: 0,
            hold_until: None,
            hold_touch: 0,
            state: None,
            train: None,
            sound_set: None,
            event_time: 0,
        }
    }

    fn is_moving(&self) -> bool {
        self.pos.kind != TR_STATIONARY || self.apos.kind != TR_STATIONARY
    }

    fn abs(&self) -> (V3, V3) {
        abs_bounds(self.origin, self.angles, self.mins, self.maxs)
    }
}

/// The player as G_MoverPush sees it: an entity with a box, standing on
/// something, that can be pushed.
struct Actor {
    origin: V3,
    mins: V3,
    maxs: V3,
    ground: i32,
    delta_yaw: i32,
    dirty: bool,
}

impl Actor {
    fn read(state: &PlayerState) -> Self {
        let view = state.view();
        Self {
            origin: view.origin,
            mins: view.mins,
            maxs: view.maxs,
            ground: view.ground_entity,
            delta_yaw: view.delta_angles[1],
            dirty: false,
        }
    }

    fn abs(&self) -> (V3, V3) {
        (
            std::array::from_fn(|axis| self.origin[axis] + self.mins[axis] - 1.0),
            std::array::from_fn(|axis| self.origin[axis] + self.maxs[axis] + 1.0),
        )
    }

    /// Hand the pushed origin (and the view turn a rotating mover imparts)
    /// back to Pmove's playerState.
    fn write_back(&self, state: &mut PlayerState) {
        let mut network = state.network();
        let mut player = ProtocolPlayerState::default();
        player.fields = std::mem::take(&mut network.fields);
        for axis in 0..3 {
            player.set_field_bits(&format!("origin[{axis}]"), self.origin[axis].to_bits());
        }
        player.set_field_bits("delta_angles[1]", self.delta_yaw as u32);
        network.fields = player.fields;
        if let Err(error) = state.set_network(&network) {
            eprintln!("LOCAL SERVER MOVERS: could not move the player with a mover: {error}");
        }
    }
}

fn write_trajectory(state: &mut EntityState, prefix: &str, tr: &Trajectory) {
    set_entity_i32(state, &format!("{prefix}.trType"), tr.kind);
    set_entity_i32(state, &format!("{prefix}.trTime"), tr.time);
    set_entity_i32(state, &format!("{prefix}.trDuration"), tr.duration);
    set_entity_vec3(state, &format!("{prefix}.trBase"), tr.base);
    set_entity_vec3(state, &format!("{prefix}.trDelta"), tr.delta);
}

fn nonzero(value: f32, default: f32) -> f32 {
    if value == 0.0 {
        default
    } else {
        value
    }
}

fn keyed(vars: &SpawnVars, key: &str) -> Option<String> {
    vars.spawn(key).filter(|value| !value.is_empty()).map(str::to_owned)
}

/// All the game entities of one map, and the level clock they run on.
pub(super) struct MoverGame {
    ents: Vec<GEnt>,
    solids: Vec<SolidEntity>,
    /// entity number -> index into `ents`.
    by_number: Vec<i32>,
    time: i32,
    previous_time: i32,
    started: bool,
    /// The server timeline is Pmove's command time plus this.
    time_offset: i32,
    use_delay: i32,
    rng: u32,
    depth: u32,
    areas: Option<AreaLocator>,
    area_count: usize,
    /// cm.areaPortals: open-count per ordered area pair.
    portals: Vec<i32>,
    /// G_SoundSetIndex: the `soundSet` names; entry `n` is `CS_AMBIENT_SET + n + 1`.
    sound_sets: Vec<String>,
}

impl MoverGame {
    /// Take over the entities `movers.rs` spawned (`entities`, `sources` and
    /// `trains` are its `BrushMovers`) and create the ones that are never
    /// published: triggers and target entities.
    pub(super) fn new(
        entities: Vec<EntityState>,
        sources: Vec<usize>,
        trains: Vec<Train>,
        brush_entities: &[MapBrushEntity],
        areas: Option<AreaLocator>,
    ) -> Self {
        let area_count = areas.as_ref().map_or(0, AreaLocator::area_count);
        let mut game = Self {
            ents: Vec::new(),
            solids: Vec::new(),
            by_number: vec![-1; MAX_GENTITIES],
            time: 0,
            previous_time: 0,
            started: false,
            time_offset: 0,
            use_delay: 0,
            rng: 0x2545_f491,
            depth: 0,
            areas,
            area_count,
            portals: vec![0; area_count * area_count],
            sound_sets: Vec::new(),
        };
        let mut published = vec![false; brush_entities.len()];
        for (state, &source) in entities.into_iter().zip(&sources) {
            published[source] = true;
            game.spawn_published(state, &brush_entities[source]);
        }
        for train in trains {
            if let Some(ent) = game.ents.get_mut(train.entity) {
                ent.train = Some(train);
            }
        }
        for (source, entity) in brush_entities.iter().enumerate() {
            if !published[source] {
                game.spawn_logic(entity);
            }
        }
        game.find_teams();
        game.init_areas();
        game.spawn_plat_triggers();
        game
    }

    pub(super) fn set_time_offset(&mut self, offset: i32) {
        self.time_offset = offset;
    }

    /// The sound sets movers use, in `CS_AMBIENT_SET + 1 + n` order.
    pub(super) fn sound_sets(&self) -> &[String] {
        &self.sound_sets
    }

    /// The mover entities clients see.
    pub(super) fn published(&self) -> impl Iterator<Item = &EntityState> {
        self.ents.iter().filter_map(|ent| ent.state.as_ref())
    }

    pub(super) fn summary(&self) -> String {
        let count = |class| self.ents.iter().filter(|ent| ent.class == class).count();
        format!(
            "{} door(s), {} plat(s), {} button(s), {} trigger(s), {} relay/target(s)",
            count(Class::Door),
            count(Class::Plat),
            count(Class::Button),
            count(Class::TriggerMulti),
            count(Class::Relay) + count(Class::Delay) + count(Class::Counter) + count(Class::Activate) + count(Class::Deactivate)
        )
    }

    fn push_ent(&mut self, ent: GEnt) -> usize {
        let index = self.ents.len();
        // Only entities clients can see are ever named by number (Pmove's
        // touch list, TryUse's trace); the rest are found by targetname.
        if ent.state.is_some() {
            if let Some(slot) = usize::try_from(ent.number).ok().and_then(|n| self.by_number.get_mut(n)) {
                *slot = index as i32;
            }
        }
        self.ents.push(ent);
        index
    }

    fn index_of(&self, number: i32) -> Option<usize> {
        usize::try_from(number)
            .ok()
            .and_then(|n| self.by_number.get(n))
            .and_then(|&index| usize::try_from(index).ok())
    }

    // ------------------------------------------------------------ spawning --

    fn spawn_published(&mut self, state: EntityState, source: &MapBrushEntity) {
        let vars = SpawnVars(&source.spawn_vars);
        let classname = vars.spawn("classname").unwrap_or_default().to_ascii_lowercase();
        let class = match classname.as_str() {
            "func_door" => Class::Door,
            "func_plat" => Class::Plat,
            "func_button" => Class::Button,
            "func_train" => Class::Train,
            _ => Class::Brush,
        };
        let mut ent = GEnt::blank(i32::from(state.number), class);
        ent.spawnflags = vars.field_i32("spawnflags");
        ent.targetname = keyed(&vars, "targetname");
        ent.target = keyed(&vars, "target");
        ent.target2 = keyed(&vars, "target2");
        ent.opentarget = keyed(&vars, "opentarget");
        ent.closetarget = keyed(&vars, "closetarget");
        ent.team = keyed(&vars, "team");
        ent.sound_set = keyed(&vars, "soundset");
        ent.model = source.model;
        ent.mins = source.mins;
        ent.maxs = source.maxs;
        ent.pos = Trajectory::of(&state, "pos");
        ent.apos = Trajectory::of(&state, "apos");
        ent.origin = ent.pos.base;
        ent.angles = ent.apos.base;
        ent.linear = vars.field_i32("linear") != 0;
        if class.is_binary() {
            Self::init_binary_mover(&mut ent, &vars);
        }
        let solid = state.field_i32("solid") == Some(SOLID_BMODEL);
        ent.state = Some(state);
        let index = self.push_ent(ent);
        // G_PrecacheSoundsets.
        if let Some(name) = self.ents[index].sound_set.clone() {
            let slot = self.sound_set_index(&name);
            if let Some(state) = self.ents[index].state.as_mut() {
                set_entity_i32(state, "soundSetIndex", slot);
            }
        }
        if solid {
            self.solids.push(SolidEntity {
                number: self.ents[index].number,
                generic_enemy_index: 0,
                skip_movement: false,
                clip: EntityClip::InlineModel {
                    index: source.model as i32,
                    origin: self.ents[index].origin,
                    angles: self.ents[index].angles,
                },
                bounds: None,
            });
            self.ents[index].solid = Some(self.solids.len() - 1);
            self.link(index);
        }
    }

    /// SP_func_door / SP_func_plat / SP_func_button, then InitMover.
    fn init_binary_mover(ent: &mut GEnt, vars: &SpawnVars) {
        let origin = vars.field("origin").map_or([0.0; 3], sscanf_vec3);
        let size = sub(ent.maxs, ent.mins);
        let movedir = set_movedir(vars.field_angles());
        let along = |lip: f32| dot(movedir.map(f32::abs), size) - lip;
        match ent.class {
            Class::Door => {
                ent.speed = nonzero(vars.field_f32("speed"), 400.0);
                ent.wait = nonzero(vars.field_f32("wait"), 2.0) * 1000.0;
                ent.delay = (vars.field_i32("delay") * 1000) as f32;
                let far = add(origin, movedir.map(|v| v * along(vars.spawn_f32("lip", 8.0).1)));
                (ent.pos1, ent.pos2) = if ent.spawnflags & MOVER_START_OPEN != 0 { (far, origin) } else { (origin, far) };
                // Locked doors, and any without a targetname/health/use key,
                // are wired up by their first think.
                let targeted = ent.targetname.is_some()
                    || vars.field_i32("health") != 0
                    || ent.spawnflags & (MOVER_PLAYER_USE | MOVER_FORCE_ACTIVATE) != 0;
                ent.think = if ent.spawnflags & MOVER_LOCKED == 0 && targeted {
                    Think::MatchTeam
                } else {
                    Think::SpawnDoorTrigger
                };
                ent.spawn_think = FRAMETIME;
            }
            Class::Plat => {
                ent.speed = vars.spawn_f32("speed", 200.0).1;
                ent.wait = 1000.0;
                ent.delay = vars.field_i32("delay") as f32;
                let (height_set, height) = vars.spawn_f32("height", 0.0);
                let lip = vars.spawn_f32("lip", 8.0).1;
                let height = if height_set { height } else { size[2] - lip };
                ent.pos2 = origin;
                ent.pos1 = [origin[0], origin[1], origin[2] - height];
                ent.touch = Touch::Plat;
                ent.parent = None;
            }
            _ => {
                ent.speed = nonzero(vars.field_f32("speed"), 40.0);
                ent.wait = nonzero(vars.field_f32("wait"), 1.0) * 1000.0;
                ent.delay = vars.field_i32("delay") as f32;
                ent.pos1 = origin;
                ent.pos2 = add(origin, movedir.map(|v| v * along(vars.spawn_f32("lip", 4.0).1)));
                if vars.field_i32("health") == 0 {
                    ent.touch = Touch::Button;
                }
            }
        }
        // InitMover / InitMoverTrData.
        ent.can_use = true;
        ent.inactive = ent.spawnflags & MOVER_INACTIVE != 0;
        ent.player_usable = ent.spawnflags & MOVER_PLAYER_USE != 0;
        ent.mover_state = MoverState::Pos1;
        ent.origin = ent.pos1;
        if ent.speed == 0.0 {
            ent.speed = 100.0;
        }
        let distance = length(sub(ent.pos2, ent.pos1));
        ent.pos = Trajectory {
            kind: TR_STATIONARY,
            time: 0,
            duration: ((distance * 1000.0 / ent.speed) as i32).max(1),
            base: ent.pos1,
            delta: [0.0; 3],
        };
        ent.apos = Trajectory::default();
    }

    /// The entities that are never sent to clients.
    fn spawn_logic(&mut self, source: &MapBrushEntity) {
        let vars = SpawnVars(&source.spawn_vars);
        if !spawns_in_local_gametype(&vars) {
            return;
        }
        let classname = vars.spawn("classname").unwrap_or_default().to_ascii_lowercase();
        let class = match classname.as_str() {
            "trigger_multiple" | "trigger_once" => Class::TriggerMulti,
            "trigger_always" => Class::Always,
            "target_relay" => Class::Relay,
            "target_delay" => Class::Delay,
            "target_activate" => Class::Activate,
            "target_deactivate" => Class::Deactivate,
            "target_counter" => Class::Counter,
            _ => return,
        };
        let mut ent = GEnt::blank(LOGIC_ENTITY_NUMBER, class);
        ent.spawnflags = vars.field_i32("spawnflags");
        ent.targetname = keyed(&vars, "targetname");
        ent.target = keyed(&vars, "target");
        ent.target2 = keyed(&vars, "target2");
        ent.wait = vars.field_f32("wait");
        ent.random = vars.field_f32("random");
        ent.origin = vars.field("origin").map_or([0.0; 3], sscanf_vec3);
        ent.can_use = true;
        match class {
            Class::TriggerMulti => {
                let once = classname == "trigger_once";
                ent.model = source.model;
                ent.mins = source.mins;
                ent.maxs = source.maxs;
                ent.delay = (vars.field_i32("delay") * 1000) as f32;
                ent.use_time = vars.field_i32("usetime");
                ent.wait = if once { -1.0 } else { ent.wait };
                if !once && ent.wait > 0.0 && ent.random >= ent.wait {
                    ent.random = ent.wait - FRAMETIME as f32 / 1000.0;
                }
                let speed = vars.field_f32("speed");
                ent.speed = if speed == 0.0 && ent.target2.is_some() { 1000.0 } else { speed * 1000.0 };
                ent.touch = Touch::Multi;
                ent.trigger = source.model > 0;
                ent.inactive = ent.spawnflags & TRIGGER_INACTIVE != 0;
                let angles = vars.field_angles();
                if angles != [0.0; 3] {
                    ent.movedir = set_movedir(angles);
                }
            }
            Class::Always => {
                ent.think = Think::Always;
                ent.spawn_think = 300;
                ent.can_use = false;
            }
            Class::Relay => ent.inactive = ent.spawnflags & 128 != 0,
            Class::Delay => {
                // "delay" is the old spelling of "wait" (default 1 second).
                ent.wait = match vars.spawn("delay") {
                    Some(delay) => super::movers::atof(delay),
                    None => vars.spawn_f32("wait", 1.0).1,
                };
                if ent.wait == 0.0 {
                    ent.wait = 1.0;
                }
            }
            Class::Counter => {
                ent.wait = -1.0;
                ent.count = vars.field_i32("count");
                if ent.count == 0 {
                    ent.count = 2;
                }
                ent.initial_count = ent.count;
                ent.bounce_count = vars.field_i32("bouncecount");
                ent.inactive = ent.spawnflags & 128 != 0;
            }
            _ => {}
        }
        self.push_ent(ent);
    }

    /// SpawnPlatTrigger for every untargeted plat, once the whole map exists.
    fn spawn_plat_triggers(&mut self) {
        let plats: Vec<usize> = (0..self.ents.len())
            .filter(|&i| self.ents[i].class == Class::Plat && self.ents[i].targetname.is_none())
            .collect();
        for plat in plats {
            let (pos1, mins, maxs) = (self.ents[plat].pos1, self.ents[plat].mins, self.ents[plat].maxs);
            let mut tmin = [pos1[0] + mins[0] + 33.0, pos1[1] + mins[1] + 33.0, pos1[2] + mins[2]];
            let mut tmax = [pos1[0] + maxs[0] - 33.0, pos1[1] + maxs[1] - 33.0, pos1[2] + maxs[2] + 8.0];
            for axis in 0..2 {
                if tmax[axis] <= tmin[axis] {
                    tmin[axis] = pos1[axis] + (mins[axis] + maxs[axis]) * 0.5;
                    tmax[axis] = tmin[axis] + 1.0;
                }
            }
            let mut trigger = GEnt::blank(LOGIC_ENTITY_NUMBER, Class::TriggerPlat);
            trigger.mins = tmin;
            trigger.maxs = tmax;
            trigger.trigger = true;
            trigger.touch = Touch::PlatCenter;
            trigger.parent = Some(plat);
            self.push_ent(trigger);
            self.ents[plat].parent = Some(plat);
        }
    }

    /// G_FindTeams: movers sharing a `team` key move as one.
    fn find_teams(&mut self) {
        for i in 0..self.ents.len() {
            self.ents[i].team_master = i;
        }
        for i in 0..self.ents.len() {
            if !self.ents[i].class.is_mover() || self.ents[i].slave {
                continue;
            }
            let Some(team) = self.ents[i].team.clone() else { continue };
            let mut chain = i;
            for j in i + 1..self.ents.len() {
                if !self.ents[j].class.is_mover() || self.ents[j].slave || self.ents[j].team.as_deref() != Some(&team) {
                    continue;
                }
                self.ents[chain].team_chain = Some(j);
                self.ents[j].team_master = i;
                self.ents[j].team_chain = None;
                self.ents[j].slave = true;
                chain = j;
                if self.ents[i].class != Class::Train {
                    if let Some(name) = self.ents[j].targetname.take() {
                        self.ents[i].targetname = Some(name);
                    }
                }
            }
        }
    }

    /// SV_LinkEntity's areanum/areanum2 for every binary mover at rest.
    fn init_areas(&mut self) {
        let Some(locator) = &self.areas else { return };
        for ent in self.ents.iter_mut().filter(|ent| ent.class.is_binary()) {
            let (low, high) = abs_bounds(ent.pos1, [0.0; 3], ent.mins, ent.maxs);
            ent.areas = locator.box_areas(low, high);
        }
    }

    // --------------------------------------------------------------- areas --

    /// CM_AdjustAreaPortalState; only a team's master opens the portal.
    fn adjust_area_portal(&mut self, i: usize, open: bool) {
        let [Some(a), Some(b)] = self.ents[i].areas else { return };
        let n = self.area_count;
        if a >= n || b >= n {
            return;
        }
        let delta = if open { 1 } else { -1 };
        for index in [a * n + b, b * n + a] {
            self.portals[index] = (self.portals[index] + delta).max(0);
        }
    }

    /// CM_WriteAreaBits as refdef's areamask: a set bit hides an area. The view
    /// sees its own area and every area an open areaportal chain reaches; a
    /// view outside any area sees everything.
    pub(super) fn area_mask(&self, area: Option<usize>) -> [u8; 32] {
        let Some(area) = area.filter(|&area| area < 256) else {
            return [0; 32];
        };
        let n = self.area_count;
        let mut visible = vec![false; n.max(area + 1)];
        let mut stack = vec![area];
        visible[area] = true;
        while let Some(current) = stack.pop() {
            if current >= n {
                continue;
            }
            for other in 0..n {
                if !visible[other] && self.portals[current * n + other] > 0 {
                    visible[other] = true;
                    stack.push(other);
                }
            }
        }
        let mut mask = [0xff; 32];
        for (area, _) in visible.iter().enumerate().filter(|(_, visible)| **visible) {
            if area < 256 {
                mask[area / 8] &= !(1 << (area % 8));
            }
        }
        mask
    }

    // ------------------------------------------------------------- linking --

    /// SV_LinkEntity for a solid brush model: where Pmove finds it.
    fn link(&mut self, i: usize) {
        let ent = &self.ents[i];
        let Some(slot) = ent.solid else { return };
        let (low, high) = ent.abs();
        let solid = &mut self.solids[slot];
        solid.clip = EntityClip::InlineModel { index: ent.model as i32, origin: ent.origin, angles: ent.angles };
        // Room for the trace-box epsilon.
        solid.bounds = Some((low.map(|v| v - 2.0), high.map(|v| v + 2.0)));
    }

    fn sync(&mut self, i: usize) {
        let ent = &mut self.ents[i];
        if let Some(state) = ent.state.as_mut() {
            write_trajectory(state, "pos", &ent.pos);
            write_trajectory(state, "apos", &ent.apos);
        }
    }

    // ------------------------------------------------------ G_FindTargets --

    fn find_targets(&self, name: &str) -> Vec<usize> {
        (0..self.ents.len())
            .filter(|&i| self.ents[i].targetname.as_deref().is_some_and(|t| t.eq_ignore_ascii_case(name)))
            .collect()
    }

    fn random(&mut self) -> u32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng
    }

    /// Q_flrand(-1, 1).
    fn crandom(&mut self) -> f32 {
        (self.random() as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    /// G_UseTargets.
    fn use_targets(&mut self, i: usize, activator: Activator) {
        let target = self.ents[i].target.clone();
        self.use_targets2(i, activator, target.as_deref());
    }

    /// G_UseTargets2: `use` every entity whose targetname matches `name`.
    fn use_targets2(&mut self, i: usize, activator: Activator, name: Option<&str>) {
        let Some(name) = name.filter(|name| !name.is_empty()) else { return };
        if self.depth > 16 {
            return;
        }
        self.depth += 1;
        for target in self.find_targets(name) {
            if target != i {
                self.global_use(target, i, activator);
            }
        }
        self.depth -= 1;
    }

    /// GlobalUse.
    fn global_use(&mut self, i: usize, other: usize, activator: Activator) {
        let ent = &self.ents[i];
        if ent.inactive || !ent.can_use {
            return;
        }
        match ent.class {
            Class::Door | Class::Plat | Class::Button => self.use_binary_mover(i, Some(other), activator),
            Class::TriggerMulti => self.multi_trigger(i, activator),
            Class::Relay => self.target_relay_use(i, activator),
            Class::Delay => self.target_delay_use(i, activator),
            Class::Activate | Class::Deactivate => {
                let active = self.ents[i].class == Class::Activate;
                if let Some(name) = self.ents[i].target.clone() {
                    for target in self.find_targets(&name) {
                        self.ents[target].inactive = !active;
                    }
                }
            }
            Class::Counter => self.target_counter_use(i, activator),
            _ => {}
        }
    }

    fn target_relay_use(&mut self, i: usize, activator: Activator) {
        let ent = &self.ents[i];
        // The player is TEAM_FREE in the local FFA game, never red or blue.
        if ent.spawnflags & TARGET_RELAY_TEAM_ONLY != 0 && activator == Activator::Player {
            return;
        }
        if ent.wait == -1.0 {
            self.ents[i].can_use = false;
        }
        if self.ents[i].spawnflags & TARGET_RELAY_RANDOM != 0 {
            if let Some(name) = self.ents[i].target.clone() {
                let choices = self.find_targets(&name);
                if !choices.is_empty() {
                    let pick = choices[self.random() as usize % choices.len()];
                    self.global_use(pick, i, activator);
                }
            }
            return;
        }
        self.use_targets(i, activator);
    }

    fn target_delay_use(&mut self, i: usize, activator: Activator) {
        let ent = &self.ents[i];
        if ent.nextthink > self.time && ent.spawnflags & TARGET_DELAY_NO_RETRIGGER != 0 {
            return;
        }
        let jitter = self.crandom();
        let ent = &mut self.ents[i];
        ent.nextthink = self.time + ((ent.wait + ent.random * jitter) * 1000.0) as i32;
        ent.think = Think::TargetDelay;
        ent.activator = activator;
    }

    fn target_counter_use(&mut self, i: usize, activator: Activator) {
        if self.ents[i].count == 0 {
            return;
        }
        self.ents[i].count -= 1;
        if self.ents[i].count != 0 {
            let target2 = self.ents[i].target2.clone();
            self.use_targets2(i, activator, target2.as_deref());
            return;
        }
        if self.ents[i].spawnflags & 128 != 0 {
            self.ents[i].inactive = true;
        }
        self.ents[i].activator = activator;
        self.use_targets(i, activator);
        let ent = &mut self.ents[i];
        if ent.count == 0 && ent.bounce_count != 0 {
            ent.count = ent.initial_count;
            if ent.bounce_count > 0 {
                ent.bounce_count -= 1;
            }
        }
    }

    // ---------------------------------------------------------------- sounds --

    /// G_SoundSetIndex: 1-based, because slot 0 means "no sound set".
    fn sound_set_index(&mut self, name: &str) -> i32 {
        let position = self
            .sound_sets
            .iter()
            .position(|known| known.eq_ignore_ascii_case(name))
            .unwrap_or_else(|| {
                self.sound_sets.push(name.to_owned());
                self.sound_sets.len() - 1
            });
        position as i32 + 1
    }

    /// G_AddEvent on a published entity. The two high event bits flip so a
    /// repeat of the same event is a new one to the client.
    fn add_event(&mut self, i: usize, event: EntityEvent, parm: i32) {
        let time = self.time;
        let ent = &mut self.ents[i];
        ent.event_time = time;
        if let Some(state) = ent.state.as_mut() {
            let previous = state.field_i32("event").unwrap_or(0);
            let toggled = ((previous & EV_EVENT_BITS) ^ EV_EVENT_BITS) | event.as_i32();
            set_entity_i32(state, "event", toggled);
            set_entity_i32(state, "eventParm", parm);
        }
    }

    /// G_PlayDoorSound.
    fn play_door_sound(&mut self, i: usize, stage: i32) {
        if self.ents[i].sound_set.is_some() {
            self.add_event(i, EntityEvent::EV_PLAYDOORSOUND, stage);
        }
    }

    /// Sets or clears the looping sound-set sound (`loopSound` / `loopIsSoundset`).
    fn set_door_loop(&mut self, i: usize, on: bool) {
        let ent = &mut self.ents[i];
        if on && ent.sound_set.is_none() {
            return;
        }
        if let Some(state) = ent.state.as_mut() {
            set_entity_i32(state, "loopSound", if on { BMS_MID } else { 0 });
            set_entity_i32(state, "loopIsSoundset", i32::from(on));
        }
    }

    /// G_RunFrame: events older than EVENT_VALID_MSEC are cleared.
    fn expire_events(&mut self) {
        let time = self.time;
        for ent in &mut self.ents {
            if time - ent.event_time <= EVENT_VALID_MSEC {
                continue;
            }
            if let Some(state) = ent.state.as_mut().filter(|state| state.field_i32("event").unwrap_or(0) != 0) {
                set_entity_i32(state, "event", 0);
            }
        }
    }

    // -------------------------------------------------------- binary movers --

    /// SetMoverState.
    fn set_mover_state(&mut self, i: usize, state: MoverState, time: i32) {
        let time_now = self.time;
        let ent = &mut self.ents[i];
        ent.mover_state = state;
        ent.pos.time = time;
        if ent.pos.duration <= 0 {
            ent.pos.duration = 1;
        }
        let moving = if ent.linear { TR_LINEAR_STOP } else { TR_NONLINEAR_STOP };
        let scale = 1000.0 / ent.pos.duration as f32;
        match state {
            MoverState::Pos1 => {
                ent.pos.base = ent.pos1;
                ent.pos.kind = TR_STATIONARY;
            }
            MoverState::Pos2 => {
                ent.pos.base = ent.pos2;
                ent.pos.kind = TR_STATIONARY;
            }
            MoverState::OneToTwo => {
                ent.pos.base = ent.pos1;
                ent.pos.delta = sub(ent.pos2, ent.pos1).map(|v| v * scale);
                ent.pos.kind = moving;
            }
            MoverState::TwoToOne => {
                ent.pos.base = ent.pos2;
                ent.pos.delta = sub(ent.pos1, ent.pos2).map(|v| v * scale);
                ent.pos.kind = moving;
            }
        }
        ent.origin = ent.pos.evaluate(time_now);
        self.link(i);
        self.sync(i);
    }

    /// MatchTeam.
    fn match_team(&mut self, leader: usize, state: MoverState, time: i32) {
        let mut slave = Some(leader);
        while let Some(i) = slave {
            self.set_mover_state(i, state, time);
            slave = self.ents[i].team_chain;
        }
    }

    fn return_to_pos1(&mut self, i: usize) {
        self.ents[i].think = Think::None;
        self.ents[i].nextthink = 0;
        let time = self.time;
        self.match_team(i, MoverState::TwoToOne, time);
        self.set_door_loop(i, true);
        self.play_door_sound(i, BMS_START);
    }

    /// Reached_BinaryMover.
    fn reached_binary_mover(&mut self, i: usize) {
        let time = self.time;
        self.set_door_loop(i, false);
        match self.ents[i].mover_state {
            MoverState::OneToTwo => {
                self.set_mover_state(i, MoverState::Pos2, time);
                self.play_door_sound(i, BMS_END);
                if self.ents[i].wait < 0.0 {
                    let ent = &mut self.ents[i];
                    ent.think = Think::None;
                    ent.nextthink = 0;
                    ent.can_use = false;
                } else {
                    let ent = &mut self.ents[i];
                    ent.think = Think::ReturnToPos1;
                    // TOGGLE movers wait for the next use.
                    ent.nextthink = if ent.spawnflags & MOVER_TOGGLE != 0 { -1 } else { time + ent.wait as i32 };
                }
                let activator = match self.ents[i].activator {
                    Activator::None => Activator::Entity(i),
                    other => other,
                };
                let opentarget = self.ents[i].opentarget.clone();
                self.use_targets2(i, activator, opentarget.as_deref());
            }
            MoverState::TwoToOne => {
                self.set_mover_state(i, MoverState::Pos1, time);
                self.play_door_sound(i, BMS_END);
                if self.ents[i].team_master == i {
                    self.adjust_area_portal(i, false);
                }
                let closetarget = self.ents[i].closetarget.clone();
                let activator = self.ents[i].activator;
                self.use_targets2(i, activator, closetarget.as_deref());
            }
            MoverState::Pos1 | MoverState::Pos2 => {}
        }
    }

    /// Use_BinaryMover_Go.
    fn use_binary_mover_go(&mut self, i: usize) {
        let time = self.time;
        match self.ents[i].mover_state {
            MoverState::Pos1 => {
                // Start 50 msec later: a player-triggered use has not advanced the clock yet.
                self.match_team(i, MoverState::OneToTwo, time + 50);
                self.set_door_loop(i, true);
                self.play_door_sound(i, BMS_START);
                if self.ents[i].team_master == i {
                    self.adjust_area_portal(i, true);
                }
                let activator = self.ents[i].activator;
                self.use_targets(i, activator);
            }
            MoverState::Pos2 => {
                // All the way open: just delay before coming down.
                let ent = &mut self.ents[i];
                ent.think = Think::ReturnToPos1;
                ent.nextthink = if ent.spawnflags & MOVER_TOGGLE != 0 { time + FRAMETIME } else { time + ent.wait as i32 };
                let (activator, target2) = (ent.activator, ent.target2.clone());
                self.use_targets2(i, activator, target2.as_deref());
            }
            state @ (MoverState::TwoToOne | MoverState::OneToTwo) => {
                // Only partway before reversing: restart at the matching point of the return leg.
                let ent = &self.ents[i];
                let (total, partial) = if ent.pos.kind == TR_NONLINEAR_STOP {
                    let total = ent.pos.duration - 50;
                    let anchor = if state == MoverState::TwoToOne { ent.pos1 } else { ent.pos2 };
                    let mut fraction = length(sub(ent.origin, anchor)) / length(ent.pos.delta);
                    fraction /= ent.pos.duration as f32;
                    fraction /= 0.001;
                    let degrees = fraction.clamp(-1.0, 1.0).acos().to_degrees();
                    let elapsed = (90.0 - degrees) / 90.0 * ent.pos.duration as f32;
                    (total, total - elapsed.floor() as i32)
                } else {
                    (ent.pos.duration, time - ent.pos.time)
                };
                let partial = partial.min(total);
                let start = time - (total - partial);
                self.ents[i].pos.time = start;
                let next = if state == MoverState::TwoToOne { MoverState::OneToTwo } else { MoverState::TwoToOne };
                self.match_team(i, next, start);
                self.play_door_sound(i, BMS_START);
            }
        }
    }

    fn lock_doors(&mut self, leader: usize, locked: bool) {
        let mut slave = Some(leader);
        while let Some(i) = slave {
            let ent = &mut self.ents[i];
            if locked {
                ent.spawnflags |= MOVER_LOCKED;
            } else {
                // Locked toggle doors keep their targetname.
                if ent.spawnflags & MOVER_TOGGLE == 0 {
                    ent.targetname = None;
                }
                ent.spawnflags &= !MOVER_LOCKED;
            }
            if let Some(state) = ent.state.as_mut() {
                set_entity_i32(state, "frame", i32::from(!locked));
            }
            slave = self.ents[i].team_chain;
        }
    }

    /// Use_BinaryMover.
    fn use_binary_mover(&mut self, i: usize, other: Option<usize>, activator: Activator) {
        let _ = other;
        if !self.ents[i].can_use {
            return;
        }
        // Only the master is used.
        if self.ents[i].slave {
            let master = self.ents[i].team_master;
            self.use_binary_mover(master, other, activator);
            return;
        }
        if self.ents[i].inactive {
            return;
        }
        if self.ents[i].spawnflags & MOVER_LOCKED != 0 {
            self.lock_doors(i, false);
            return;
        }
        self.ents[i].activator = activator;
        if self.ents[i].delay != 0.0 {
            let time = self.time;
            let ent = &mut self.ents[i];
            ent.think = Think::UseBinaryMoverGo;
            ent.nextthink = time + ent.delay as i32;
        } else {
            self.use_binary_mover_go(i);
        }
    }

    /// Blocked_Door (doors and plats): reverse unless a crusher.
    fn blocked_door(&mut self, i: usize) {
        let relock = self.ents[i].spawnflags & MOVER_LOCKED != 0;
        if self.ents[i].spawnflags & MOVER_CRUSHER != 0 {
            return;
        }
        self.use_binary_mover(i, Some(i), Activator::Player);
        if relock {
            self.lock_doors(i, true);
        }
    }

    // ---------------------------------------------------------- G_MoverPush --

    /// G_TestEntityPosition for the player.
    fn test_position(&self, actor: &Actor, world: &mut CollisionWorld) -> bool {
        let mut maxs = actor.maxs;
        maxs[2] = maxs[2].max(1.0);
        let trace = PredictionWorld { world, solids: &self.solids, client_num: 0 }.trace(TraceQuery {
            start: actor.origin,
            mins: actor.mins,
            maxs,
            end: actor.origin,
            pass_entity: 0,
            mask: MASK_PLAYERSOLID,
        });
        trace.start_solid != 0
    }

    /// G_MoverPush for one part of a team: move it and carry or shove the
    /// player. `pushed` remembers where the player was before the team's
    /// first push so a blocked move can put it back. Returns false if blocked.
    fn mover_push(
        &mut self,
        part: usize,
        mv: V3,
        amove: V3,
        actor: Option<&mut Actor>,
        pushed: &mut Vec<(V3, i32)>,
        world: &mut CollisionWorld,
    ) -> bool {
        let (mins, maxs, total_min, total_max) = {
            let ent = &self.ents[part];
            if ent.angles != [0.0; 3] || amove != [0.0; 3] {
                let radius = radius_from_bounds(ent.mins, ent.maxs);
                let mins = ent.origin.map(|v| v - radius);
                let maxs = ent.origin.map(|v| v + radius);
                let (mins, maxs) = (add(mins, mv), add(maxs, mv));
                (mins, maxs, sub(mins, mv), sub(maxs, mv))
            } else {
                let (low, high) = ent.abs();
                let (mut total_min, mut total_max) = (low, high);
                for axis in 0..3 {
                    if mv[axis] > 0.0 {
                        total_max[axis] += mv[axis];
                    } else {
                        total_min[axis] += mv[axis];
                    }
                }
                (add(low, mv), add(high, mv), total_min, total_max)
            }
        };
        // Move the pusher to its final position.
        self.ents[part].origin = add(self.ents[part].origin, mv);
        self.ents[part].angles = add(self.ents[part].angles, amove);
        self.link(part);

        let Some(actor) = actor else { return true };
        let (low, high) = actor.abs();
        if !overlaps(low, high, total_min, total_max) {
            return true;
        }
        let number = self.ents[part].number;
        if actor.ground != number {
            // Not riding it: only pushed if its box ends up inside the mover.
            if (0..3).any(|axis| low[axis] >= maxs[axis] || high[axis] <= mins[axis]) {
                return true;
            }
            if !self.test_position(actor, world) {
                return true;
            }
        }

        // G_TryPushingEntity.
        let (old_origin, old_yaw) = (actor.origin, actor.delta_yaw);
        pushed.push((old_origin, old_yaw));
        let offset = sub(actor.origin, self.ents[part].origin);
        let turned = sub(rotate_by_pusher(amove, offset), offset);
        actor.origin = add(add(actor.origin, mv), turned);
        actor.delta_yaw += angle_to_short(amove[1]);
        actor.dirty = true;
        if actor.ground != number {
            actor.ground = ENTITYNUM_NONE;
        }
        if !self.test_position(actor, world) {
            return true;
        }
        // Blocked: leaving it where it was is fine for a rider (a sliding trapdoor).
        actor.origin = old_origin;
        if !self.test_position(actor, world) {
            actor.ground = ENTITYNUM_NONE;
            pushed.pop();
            return true;
        }

        // Bobbing entities are instant-kill and never get blocked; with no damage
        // model the solo player is left where it was and the mover carries on.
        let ent = &self.ents[part];
        if ent.pos.kind == TR_SINE || ent.apos.kind == TR_SINE {
            return true;
        }
        // Move back everything already moved.
        if let Some(&(origin, yaw)) = pushed.first() {
            actor.origin = origin;
            actor.delta_yaw = yaw;
        }
        false
    }

    /// G_MoverTeam.
    fn mover_team(&mut self, leader: usize, mut actor: Option<&mut Actor>, world: &mut CollisionWorld) {
        let mut chain = Vec::new();
        let mut part = Some(leader);
        while let Some(i) = part {
            chain.push(i);
            part = self.ents[i].team_chain;
        }
        let time = self.time;
        let mut pushed = Vec::new();
        let mut blocked = false;
        for &part in &chain {
            let ent = &self.ents[part];
            let mv = sub(ent.pos.evaluate(time), ent.origin);
            let amove = sub(ent.apos.evaluate(time), ent.angles);
            if (mv != [0.0; 3] || amove != [0.0; 3])
                && !self.mover_push(part, mv, amove, actor.as_deref_mut(), &mut pushed, world)
            {
                blocked = true;
                break;
            }
        }
        if blocked {
            // Go back to the previous position: the movers lose this frame.
            let lost = time - self.previous_time;
            for &part in &chain {
                let ent = &mut self.ents[part];
                ent.pos.time += lost;
                ent.apos.time += lost;
                ent.origin = ent.pos.evaluate(time);
                ent.angles = ent.apos.evaluate(time);
                if let Some(train) = ent.train.as_mut() {
                    train.delay(lost);
                }
                self.link(part);
                self.sync(part);
            }
            if matches!(self.ents[leader].class, Class::Door | Class::Plat) {
                self.blocked_door(leader);
            }
            return;
        }
        for &part in &chain {
            let ent = &self.ents[part];
            let stops = matches!(ent.pos.kind, TR_LINEAR_STOP | TR_NONLINEAR_STOP);
            if stops && time >= ent.pos.time + ent.pos.duration && ent.class.is_binary() {
                self.reached_binary_mover(part);
            }
        }
    }

    /// G_RunMover.
    fn run_mover(&mut self, i: usize, actor: Option<&mut Actor>, world: &mut CollisionWorld) {
        if self.ents[i].slave {
            return;
        }
        if self.ents[i].is_moving() {
            self.mover_team(i, actor, world);
        }
        let time = self.time;
        let ent = &mut self.ents[i];
        if let (Some(state), Some(train)) = (ent.state.as_mut(), ent.train.as_mut()) {
            if train.advance(state, time) {
                ent.pos = Trajectory::of(state, "pos");
                ent.apos = Trajectory::of(state, "apos");
            }
        }
        self.run_think(i);
    }

    // ------------------------------------------------------------- thinking --

    /// G_RunThink.
    fn run_think(&mut self, i: usize) {
        let ent = &self.ents[i];
        if ent.nextthink <= 0 || ent.nextthink > self.time {
            return;
        }
        let think = ent.think;
        self.ents[i].nextthink = 0;
        match think {
            Think::None => {}
            Think::ReturnToPos1 => self.return_to_pos1(i),
            Think::UseBinaryMoverGo => self.use_binary_mover_go(i),
            Think::MatchTeam => {
                let (state, time) = (self.ents[i].mover_state, self.time);
                self.match_team(i, state, time);
            }
            Think::SpawnDoorTrigger => self.spawn_door_trigger(i),
            Think::MultiWait => {}
            Think::MultiTriggerRun => self.multi_trigger_run(i),
            Think::ClearedFire => self.trigger_cleared_fire(i),
            Think::TargetDelay => {
                let activator = self.ents[i].activator;
                self.use_targets(i, activator);
            }
            Think::Always => {
                self.use_targets(i, Activator::Entity(i));
                self.ents[i].can_use = false;
            }
        }
    }

    /// Think_SpawnNewDoorTrigger: a touch box around the whole team.
    fn spawn_door_trigger(&mut self, i: usize) {
        let (mut mins, mut maxs) = self.ents[i].abs();
        let mut other = self.ents[i].team_chain;
        while let Some(part) = other {
            let (low, high) = self.ents[part].abs();
            for axis in 0..3 {
                mins[axis] = mins[axis].min(low[axis]).min(high[axis]);
                maxs[axis] = maxs[axis].max(low[axis]).max(high[axis]);
            }
            other = self.ents[part].team_chain;
        }
        // Expand the thinnest axis.
        let mut best = 0;
        for axis in 1..3 {
            if maxs[axis] - mins[axis] < maxs[best] - mins[best] {
                best = axis;
            }
        }
        maxs[best] += 120.0;
        mins[best] -= 120.0;
        let mut trigger = GEnt::blank(LOGIC_ENTITY_NUMBER, Class::TriggerDoor);
        trigger.mins = mins;
        trigger.maxs = maxs;
        trigger.trigger = true;
        trigger.touch = Touch::DoorTrigger;
        trigger.parent = Some(i);
        trigger.count = best as i32;
        self.push_ent(trigger);
        let (state, time) = (self.ents[i].mover_state, self.time);
        self.match_team(i, state, time);
    }

    // ------------------------------------------------------------- triggers --

    fn touch_button(&mut self, i: usize) {
        if self.ents[i].mover_state == MoverState::Pos1 {
            self.use_binary_mover(i, Some(i), Activator::Player);
        }
    }

    /// Touch_Plat: hold off the return while a live player stands on it.
    fn touch_plat(&mut self, i: usize) {
        if self.ents[i].mover_state == MoverState::Pos2 {
            self.ents[i].nextthink = self.time + 1000;
        }
    }

    fn touch_plat_center(&mut self, i: usize) {
        let Some(plat) = self.ents[i].parent else { return };
        if self.ents[plat].mover_state == MoverState::Pos1 {
            self.use_binary_mover(plat, Some(i), Activator::Player);
        }
    }

    /// Touch_DoorTrigger.
    fn touch_door_trigger(&mut self, i: usize) {
        let Some(parent) = self.ents[i].parent else { return };
        if self.ents[i].inactive || self.ents[parent].spawnflags & MOVER_LOCKED != 0 {
            return;
        }
        if self.ents[parent].mover_state != MoverState::OneToTwo {
            self.use_binary_mover(parent, Some(i), Activator::Player);
        }
    }

    /// Touch_Multi.
    fn touch_multi(&mut self, i: usize, view: &PlayerView, buttons: i32) {
        let ent = &self.ents[i];
        if ent.inactive {
            return;
        }
        // Only NPCs can set off an NPCONLY trigger, unless CLIENTONLY is also set.
        if ent.spawnflags & TRIGGER_CLIENTONLY == 0 && ent.spawnflags & TRIGGER_NPCONLY != 0 {
            return;
        }
        if ent.spawnflags & TRIGGER_FACING != 0 {
            let forward = angle_vectors(view.view_angles)[0];
            if dot(ent.movedir, forward) < 0.5 {
                return;
            }
        }
        if ent.spawnflags & TRIGGER_USE_BUTTON != 0 {
            if buttons & BUTTON_USE == 0 {
                self.ents[i].hold_until = None;
                return;
            }
            if ent.use_time > 0 {
                // The use key must be held for `usetime` ms inside the trigger.
                let (time, use_time) = (self.time, ent.use_time);
                let ent = &mut self.ents[i];
                if time - ent.hold_touch > 200 {
                    ent.hold_until = None;
                }
                ent.hold_touch = time;
                match ent.hold_until {
                    None => {
                        ent.hold_until = Some(time + use_time.min(60_000));
                        return;
                    }
                    Some(until) if until < time => ent.hold_until = None,
                    Some(_) => return,
                }
            }
        }
        if self.ents[i].spawnflags & TRIGGER_FIRE_BUTTON != 0 && buttons & (BUTTON_ATTACK | BUTTON_ALT_ATTACK) == 0 {
            return;
        }
        if self.ents[i].think == Think::ClearedFire {
            // Still waiting to fire target2: keep the clear timer running.
            self.ents[i].nextthink = self.time + self.ents[i].speed as i32;
            return;
        }
        self.multi_trigger(i, Activator::Player);
    }

    /// multi_trigger.
    fn multi_trigger(&mut self, i: usize, activator: Activator) {
        let time = self.time;
        let ent = &self.ents[i];
        if ent.think == Think::MultiTriggerRun {
            return; // already triggered, just waiting to run
        }
        if ent.nextthink > time {
            if ent.spawnflags & TRIGGER_MULTIPLE != 0 {
                if ent.pain_debounce != 0 && ent.pain_debounce != time {
                    return;
                }
            } else {
                return;
            }
        }
        if activator == Activator::Player && ent.aim_debounce == time {
            return; // the player already set this off this frame
        }
        if ent.inactive {
            return;
        }
        let ent = &mut self.ents[i];
        ent.activator = activator;
        if ent.delay > 0.0 && (ent.pain_debounce as f32) < time as f32 + ent.delay {
            ent.think = Think::MultiTriggerRun;
            ent.nextthink = time + ent.delay as i32;
            ent.pain_debounce = time;
        } else {
            self.multi_trigger_run(i);
        }
    }

    /// multi_trigger_run.
    fn multi_trigger_run(&mut self, i: usize) {
        let time = self.time;
        self.ents[i].think = Think::None;
        let activator = self.ents[i].activator;
        self.use_targets(i, activator);
        let (has_target2, wait) = (self.ents[i].target2.is_some(), self.ents[i].wait);
        if has_target2 && wait >= 0.0 {
            self.ents[i].think = Think::ClearedFire;
            self.ents[i].nextthink = time + self.ents[i].speed as i32;
        } else if wait > 0.0 {
            if self.ents[i].pain_debounce != time {
                let jitter = self.crandom();
                let ent = &mut self.ents[i];
                ent.nextthink = time + ((ent.wait + ent.random * jitter) * 1000.0) as i32;
                ent.think = Think::MultiWait;
                ent.pain_debounce = time;
            }
        } else if wait < 0.0 {
            // A one-shot trigger stops being touchable and usable.
            let ent = &mut self.ents[i];
            ent.trigger = false;
            ent.can_use = false;
            ent.think = Think::None;
        }
        if activator == Activator::Player {
            self.ents[i].aim_debounce = time;
        }
    }

    /// trigger_cleared_fire.
    fn trigger_cleared_fire(&mut self, i: usize) {
        let (activator, target2) = (self.ents[i].activator, self.ents[i].target2.clone());
        self.use_targets2(i, activator, target2.as_deref());
        self.ents[i].think = Think::None;
        // The wait timer starts now that the trigger has been cleared.
        if self.ents[i].wait > 0.0 {
            let jitter = self.crandom();
            let ent = &mut self.ents[i];
            ent.nextthink = self.time + ((ent.wait + ent.random * jitter) * 1000.0) as i32;
        }
    }

    fn touch(&mut self, i: usize, view: &PlayerView, buttons: i32) {
        match self.ents[i].touch {
            Touch::None => {}
            Touch::Button => self.touch_button(i),
            Touch::Plat => self.touch_plat(i),
            Touch::PlatCenter => self.touch_plat_center(i),
            Touch::DoorTrigger => self.touch_door_trigger(i),
            Touch::Multi => self.touch_multi(i, view, buttons),
        }
    }

    /// trap->EntityContact: does the player's box touch this trigger?
    fn contacts(&self, i: usize, view: &PlayerView, world: &mut CollisionWorld) -> bool {
        let ent = &self.ents[i];
        if ent.model > 0 {
            let contact = world.trace_entity(
                TraceQuery {
                    start: view.origin,
                    mins: view.mins,
                    maxs: view.maxs,
                    end: view.origin,
                    pass_entity: 0,
                    mask: -1,
                },
                EntityClip::InlineModel { index: ent.model as i32, origin: ent.origin, angles: ent.angles },
            );
            contact.start_solid != 0 || contact.all_solid != 0 || contact.fraction < 1.0
        } else {
            (0..3).all(|axis| {
                view.origin[axis] + view.mins[axis] < ent.origin[axis] + ent.maxs[axis]
                    && view.origin[axis] + view.maxs[axis] > ent.origin[axis] + ent.mins[axis]
            })
        }
    }

    /// G_TouchTriggers.
    fn touch_triggers(&mut self, view: &PlayerView, buttons: i32, world: &mut CollisionWorld) {
        const RANGE: V3 = [40.0, 40.0, 52.0];
        let (low, high) = (sub(view.origin, RANGE), add(view.origin, RANGE));
        let candidates: Vec<usize> = (0..self.ents.len())
            .filter(|&i| {
                let ent = &self.ents[i];
                let (ent_low, ent_high) = ent.abs();
                ent.trigger && ent.touch != Touch::None && overlaps(low, high, ent_low, ent_high)
            })
            .collect();
        for i in candidates {
            if self.contacts(i, view, world) {
                self.touch(i, view, buttons);
            }
        }
    }

    /// TryUse: the entity 64 units ahead of the eyes.
    fn try_use(&mut self, view: &PlayerView, world: &mut CollisionWorld) {
        let src = view.eye_origin();
        let dest = add(src, angle_forward(view.view_angles).map(|v| v * USE_DISTANCE));
        let trace = PredictionWorld { world, solids: &self.solids, client_num: 0 }.trace(TraceQuery {
            start: src,
            mins: [0.0; 3],
            maxs: [0.0; 3],
            end: dest,
            pass_entity: 0,
            mask: MASK_USE,
        });
        if trace.fraction == 1.0 || trace.entity == ENTITYNUM_NONE {
            return;
        }
        let Some(target) = self.index_of(trace.entity) else { return };
        let ent = &self.ents[target];
        // ValidUseTarget.
        if !ent.can_use || ent.inactive || !ent.player_usable {
            return;
        }
        if ent.touch == Touch::Button {
            self.touch_button(target);
        } else {
            self.global_use(target, target, Activator::Player);
        }
    }

    /// `/trace` menu's "USE / TOGGLE": `try_use`'s tail, keyed by entity
    /// number instead of a crosshair trace. Deliberately skips the
    /// `player_usable` (proximity/facing) gate since this is an explicit dev
    /// action, not a simulated player interaction; `can_use`/`inactive` still
    /// apply so it can't use something that was never usable or is mid-cooldown.
    pub(super) fn use_entity(&mut self, entity_num: u16) -> Result<(), String> {
        let Some(target) = self.index_of(i32::from(entity_num)) else {
            return Err("not a solo-tracked entity".to_owned());
        };
        let ent = &self.ents[target];
        if !ent.can_use || ent.inactive {
            return Err("entity is not usable right now".to_owned());
        }
        if ent.touch == Touch::Button {
            self.touch_button(target);
        } else {
            self.global_use(target, target, Activator::Player);
        }
        Ok(())
    }
}

impl SoloGame for MoverGame {
    fn solids(&self) -> &[SolidEntity] {
        &self.solids
    }

    fn run_frame(&mut self, raw_time: i32, mut player: Option<&mut PlayerState>, world: &mut CollisionWorld) {
        let time = raw_time.saturating_add(self.time_offset);
        if !self.started {
            self.started = true;
            self.time = time;
            self.previous_time = time;
            // Thinks set up at spawn count from the first frame, and movers that
            // are already in motion start where their trajectory has them.
            for i in 0..self.ents.len() {
                let ent = &mut self.ents[i];
                if ent.spawn_think != 0 {
                    ent.nextthink = time + ent.spawn_think;
                }
                if ent.is_moving() {
                    ent.origin = ent.pos.evaluate(time);
                    ent.angles = ent.apos.evaluate(time);
                    self.link(i);
                }
            }
        } else if time < self.time {
            return;
        } else {
            self.previous_time = self.time;
            self.time = time;
        }
        self.expire_events();
        let mut actor = player
            .as_deref()
            .filter(|_| self.ents.iter().any(|ent| ent.class.is_mover() && ent.is_moving()))
            .map(Actor::read);
        let mut i = 0;
        // Thinks may spawn entities (door triggers), which then run too.
        while i < self.ents.len() {
            if self.ents[i].class.is_mover() {
                self.run_mover(i, actor.as_mut(), world);
            } else {
                self.run_think(i);
            }
            i += 1;
        }
        if let (Some(actor), Some(state)) = (actor.filter(|actor| actor.dirty), player.as_deref_mut()) {
            actor.write_back(state);
        }
    }

    fn after_pmove(&mut self, _raw_time: i32, player: Option<(&PlayerView, i32)>, world: &mut CollisionWorld) {
        let Some((view, buttons)) = player else { return };
        // Entities Pmove ran into (Touch_Button, Touch_Plat).
        let count = view.touch_count.clamp(0, view.touches.len() as i32) as usize;
        for (n, &number) in view.touches[..count].iter().enumerate() {
            if view.touches[..n].contains(&number) {
                continue;
            }
            if let Some(i) = self.index_of(number) {
                self.touch(i, view, buttons);
            }
        }
        if buttons & BUTTON_USE != 0 && self.use_delay < self.time {
            self.try_use(view, world);
            self.use_delay = self.time + 100;
        }
        self.touch_triggers(view, buttons, world);
    }
}

#[cfg(test)]
#[path = "game_tests.rs"]
mod tests;
