//! Port of the OpenJK MP spawn functions that give map entities an inline BSP
//! model (codemp/game/g_mover.c, g_spawn.c). Each entity is built in its
//! spawn-time state: doors, buttons and plats rest at `pos1`,
//! rotating/bobbing/pendulum movers keep the trajectory CGame evaluates every
//! frame, and START_OFF walls/usables stay out of the snapshot (SVF_NOCLIENT)
//! exactly like on a real server. A func_train that starts moving (no
//! targetname, or START_ON) runs Reached_Train/Think_BeginMoving over its
//! path_corners. What happens to them afterwards (use, touch, push, block) is
//! `game.rs`.

use jka_protocol::gamestate::{EntityState, ENTITY_FIELDS};

use super::{set_entity_i32, set_entity_vec3};
use crate::{
    cgame::ET_MOVER,
    scene::{MapBrushEntity, TrainCorner},
};

const SOLID_BMODEL: i32 = 0x00ff_ffff;
const TR_STATIONARY: i32 = 0;
const TR_LINEAR: i32 = 2;
const TR_NONLINEAR_STOP: i32 = 4;
const TR_SINE: i32 = 5;
const EF_RADAROBJECT: i32 = 1 << 2;
const EF_SHADER_ANIM: i32 = 1 << 4;
const EF2_HYPERSPACE: i32 = 1 << 5;
const MOVER_LOCKED: i32 = 16;
/// g_gravity default, used by SP_func_pendulum's frequency.
const G_GRAVITY: f32 = 800.0;
/// MAX_MODELS is 512; leave room for the other CS_MODELS users.
const MAX_MOVER_MODELS: usize = 256;
/// GT_FFA entry of G_SpawnGEntityFromSpawnVars' gametypeNames[].
const LOCAL_GAMETYPE_NAME: &str = "ffa";

/// Spawn variables with OpenJK's two lookup rules: G_SpawnString returns the
/// first matching key, while G_ParseField applies every key in order so the
/// last one wins for fields such as origin/angles/spawnflags/speed.
pub(super) struct SpawnVars<'a>(pub(super) &'a [(String, String)]);

impl SpawnVars<'_> {
    pub(super) fn spawn(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.as_str())
    }

    pub(super) fn field(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .rev()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.as_str())
    }

    pub(super) fn spawn_f32(&self, key: &str, default: f32) -> (bool, f32) {
        match self.spawn(key) {
            Some(value) => (true, atof(value)),
            None => (false, default),
        }
    }

    pub(super) fn spawn_vec3(&self, key: &str, default: [f32; 3]) -> (bool, [f32; 3]) {
        match self.spawn(key) {
            Some(value) => (true, sscanf_vec3(value)),
            None => (false, default),
        }
    }

    pub(super) fn field_i32(&self, key: &str) -> i32 {
        self.field(key).map_or(0, atoi)
    }

    pub(super) fn field_f32(&self, key: &str) -> f32 {
        self.field(key).map_or(0.0, atof)
    }

    /// F_VECTOR `origin`/`angles`, then F_ANGLEHACK `angle` if it came later.
    pub(super) fn field_angles(&self) -> [f32; 3] {
        let mut angles = [0.0; 3];
        for (name, value) in self.0 {
            if name.eq_ignore_ascii_case("angles") {
                angles = sscanf_vec3(value);
            } else if name.eq_ignore_ascii_case("angle") {
                angles = [0.0, atof(value), 0.0];
            }
        }
        angles
    }
}

/// C atof/atoi: parse the longest numeric prefix, 0 when there is none.
pub(super) fn atof(text: &str) -> f32 {
    let text = text.trim_start();
    let end = text
        .char_indices()
        .take_while(|&(index, c)| {
            c.is_ascii_digit() || c == '.' || ((c == '-' || c == '+') && index == 0)
        })
        .map(|(index, c)| index + c.len_utf8())
        .last()
        .unwrap_or(0);
    text[..end].parse::<f32>().ok().filter(|value| value.is_finite()).unwrap_or(0.0)
}

pub(super) fn atoi(text: &str) -> i32 {
    atof(text) as i32
}

pub(super) fn sscanf_vec3(text: &str) -> [f32; 3] {
    let mut values = text.split_whitespace().map(atof);
    std::array::from_fn(|_| values.next().unwrap_or(0.0))
}

/// q_math AngleVectors forward vector.
pub(super) fn angle_forward(angles: [f32; 3]) -> [f32; 3] {
    let (sp, cp) = angles[0].to_radians().sin_cos();
    let (sy, cy) = angles[1].to_radians().sin_cos();
    [cp * cy, cp * sy, -sp]
}

/// G_SetMovedir: the editor's -1/-2 `angle` shortcuts mean up/down.
pub(super) fn set_movedir(angles: [f32; 3]) -> [f32; 3] {
    if angles == [0.0, -1.0, 0.0] {
        [0.0, 0.0, 1.0]
    } else if angles == [0.0, -2.0, 0.0] {
        [0.0, 0.0, -1.0]
    } else {
        angle_forward(angles)
    }
}

/// InitMover / InitBBrush constantLight from the `light` and `color` keys.
fn constant_light(vars: &SpawnVars) -> i32 {
    let (light_set, light) = vars.spawn_f32("light", 100.0);
    let (color_set, color) = vars.spawn_vec3("color", [1.0, 1.0, 1.0]);
    if !light_set && !color_set {
        return 0;
    }
    let channel = |value: f32| ((value * 255.0) as i32).min(255);
    let intensity = ((light / 4.0) as i32).min(255);
    channel(color[0]) | (channel(color[1]) << 8) | (channel(color[2]) << 16) | (intensity << 24)
}

/// The spawn-time ET_MOVER state of one brush entity.
struct Mover {
    pos_type: i32,
    pos_base: [f32; 3],
    pos_delta: [f32; 3],
    pos_duration: i32,
    pos_time: i32,
    apos_type: i32,
    apos_base: [f32; 3],
    apos_delta: [f32; 3],
    apos_duration: i32,
    apos_time: i32,
    e_flags: i32,
    e_flags2: i32,
    constant_light: i32,
    speed: f32,
    /// `model2` (InitMover: `s.modelindex2 = G_ModelIndex(model2)`).
    model2: Option<String>,
    /// Present for a func_train that starts moving on its own.
    train: Option<TrainSpec>,
}

struct TrainSpec {
    speed: f32,
    corners: Vec<TrainCorner>,
}

impl Mover {
    fn stationary(origin: [f32; 3], vars: &SpawnVars) -> Self {
        Self {
            pos_type: TR_STATIONARY,
            pos_base: origin,
            pos_delta: [0.0; 3],
            pos_duration: 0,
            pos_time: 0,
            apos_type: TR_STATIONARY,
            apos_base: [0.0; 3],
            apos_delta: [0.0; 3],
            apos_duration: 0,
            apos_time: 0,
            e_flags: 0,
            e_flags2: 0,
            constant_light: constant_light(vars),
            speed: 0.0,
            model2: vars.spawn("model2").map(str::to_owned),
            train: None,
        }
    }
}

/// SP_func_door / SP_func_button: `pos2` is `pos1` moved along movedir by the
/// model size minus lip; START_OPEN (spawnflag 1) swaps the two.
fn binary_mover_pos1(entity: &MapBrushEntity, vars: &SpawnVars, origin: [f32; 3], lip_default: f32, start_open: bool) -> [f32; 3] {
    if !start_open {
        return origin;
    }
    let movedir = set_movedir(vars.field_angles());
    let (_, lip) = vars.spawn_f32("lip", lip_default);
    let distance = (0..3)
        .map(|axis| movedir[axis].abs() * (entity.maxs[axis] - entity.mins[axis]))
        .sum::<f32>()
        - lip;
    std::array::from_fn(|axis| origin[axis] + distance * movedir[axis])
}

/// G_CallSpawn for the brush classes. `None` means the entity would be freed
/// or linked with SVF_NOCLIENT, so it never reaches a snapshot.
fn spawn_mover(entity: &MapBrushEntity) -> Option<Mover> {
    let vars = SpawnVars(&entity.spawn_vars);
    let classname = vars.spawn("classname")?.to_ascii_lowercase();
    let origin = vars.field("origin").map_or([0.0; 3], sscanf_vec3);
    let spawnflags = vars.field_i32("spawnflags");

    let mover = match classname.as_str() {
        "func_door" => {
            let pos1 = binary_mover_pos1(entity, &vars, origin, 8.0, spawnflags & 1 != 0);
            let mut mover = Mover::stationary(pos1, &vars);
            if spawnflags & MOVER_LOCKED != 0 {
                mover.e_flags |= EF_SHADER_ANIM;
            }
            mover
        }
        "func_button" => {
            Mover::stationary(binary_mover_pos1(entity, &vars, origin, 4.0, false), &vars)
        }
        "func_plat" => {
            // pos1 is the rest (bottom) position, pos2 the authored origin.
            let (height_set, height) = vars.spawn_f32("height", 0.0);
            let height = if height_set {
                height
            } else {
                let (_, lip) = vars.spawn_f32("lip", 8.0);
                (entity.maxs[2] - entity.mins[2]) - lip
            };
            Mover::stationary([origin[0], origin[1], origin[2] - height], &vars)
        }
        "func_static" => {
            let mut mover = Mover::stationary(origin, &vars);
            mover.apos_base = vars.field_angles();
            if spawnflags & 4 != 0 {
                mover.e_flags |= EF_SHADER_ANIM;
            }
            if vars.spawn("hyperspace").map_or(0, atoi) != 0 {
                mover.e_flags2 |= EF2_HYPERSPACE;
            }
            mover
        }
        "func_rotating" => {
            // With health it is spawned through SP_func_breakable first; both
            // paths leave a stationary brush at its origin before the spin.
            let mut mover = Mover::stationary(origin, &vars);
            let (spin_set, spin) = vars.spawn_vec3("spinangles", [0.0; 3]);
            if spin_set {
                mover.apos_delta = spin;
            } else {
                let speed = match vars.field_f32("speed") {
                    speed if speed != 0.0 => speed,
                    _ => 100.0,
                };
                let axis = if spawnflags & 4 != 0 {
                    2
                } else if spawnflags & 8 != 0 {
                    0
                } else {
                    1
                };
                mover.apos_delta[axis] = speed;
            }
            mover.apos_type = TR_LINEAR;
            if spawnflags & 2 != 0 {
                let diagonal = (0..3)
                    .map(|axis| (entity.maxs[axis] - entity.mins[axis]).powi(2))
                    .sum::<f32>()
                    .sqrt();
                mover.speed = diagonal * 0.5;
                mover.e_flags |= EF_RADAROBJECT;
            }
            mover
        }
        "func_bobbing" => {
            let (_, speed) = vars.spawn_f32("speed", 4.0);
            let (_, height) = vars.spawn_f32("height", 32.0);
            let (_, phase) = vars.spawn_f32("phase", 0.0);
            let mut mover = Mover::stationary(origin, &vars);
            mover.pos_type = TR_SINE;
            mover.pos_duration = (speed * 1000.0) as i32;
            mover.pos_time = (mover.pos_duration as f32 * phase) as i32;
            let axis = if spawnflags & 1 != 0 {
                0
            } else if spawnflags & 2 != 0 {
                1
            } else {
                2
            };
            mover.pos_delta[axis] = height;
            mover
        }
        "func_pendulum" => {
            let (_, speed) = vars.spawn_f32("speed", 30.0);
            let (_, phase) = vars.spawn_f32("phase", 0.0);
            let length = entity.mins[2].abs().max(8.0);
            let frequency = 1.0 / (std::f32::consts::PI * 2.0) * (G_GRAVITY / (3.0 * length)).sqrt();
            let mut mover = Mover::stationary(origin, &vars);
            mover.pos_duration = (1000.0 / frequency) as i32;
            mover.apos_base = vars.field_angles();
            mover.apos_duration = (1000.0 / frequency) as i32;
            mover.apos_time = (mover.apos_duration as f32 * phase) as i32;
            mover.apos_type = TR_SINE;
            mover.apos_delta[2] = speed;
            mover
        }
        "func_train" => {
            // SP_func_train frees a train without a target. Think_SetupTrainTargets
            // only sends it off along its path_corners when it has no
            // targetname or START_ON; otherwise it rests at its own origin
            // (no `use` function exists to start it later).
            vars.spawn("target")?;
            let mut mover = Mover::stationary(origin, &vars);
            if vars.spawn("targetname").is_none() || spawnflags & 1 != 0 {
                let speed = match vars.field_f32("speed") {
                    speed if speed != 0.0 => speed,
                    _ => 100.0,
                };
                mover.train = Some(TrainSpec { speed, corners: entity.train_corners.clone() });
            }
            mover
        }
        "func_glass" | "func_breakable" => Mover::stationary(origin, &vars),
        "func_usable" | "func_wall" => {
            if spawnflags & 1 != 0 {
                // START_OFF: SVF_NOCLIENT + EF_NODRAW until used.
                return None;
            }
            Mover::stationary(origin, &vars)
        }
        _ => return None,
    };
    Some(mover)
}

/// G_SpawnGEntityFromSpawnVars' gametype filters for the local GT_FFA game.
pub(super) fn spawns_in_local_gametype(vars: &SpawnVars) -> bool {
    if vars.spawn("notfree").map_or(0, atoi) != 0 {
        return false;
    }
    vars.spawn("gametype")
        .is_none_or(|gametypes| gametypes.contains(LOCAL_GAMETYPE_NAME))
}

/// Everything the shim keeps for the brush entities it spawned.
pub(super) struct BrushMovers {
    pub entities: Vec<EntityState>,
    /// Index into the brush entity list of the source of each `entities` item.
    pub sources: Vec<usize>,
    /// Moving trains; `Train::entity` indexes `entities`.
    pub trains: Vec<Train>,
    /// `model2` names; `modelindex2` N refers to `models[N - 1]`.
    pub models: Vec<String>,
}

/// Runtime state of one func_train (`ent->nextTrain` and its think).
pub(super) struct Train {
    pub entity: usize,
    speed: f32,
    corners: Vec<TrainCorner>,
    next: Option<usize>,
    duration: i32,
    phase: TrainPhase,
}

#[derive(Clone, Copy)]
enum TrainPhase {
    /// Think_SetupTrainTargets has not run yet.
    Setup,
    Moving { end: i32 },
    Waiting { until: i32 },
    /// Reached_Train found no further corner: "just stop".
    Stopped,
}

/// Bound on catch-up legs in one call (a long stall over a tight cycle).
const MAX_TRAIN_STEPS: usize = 4096;

impl Train {
    /// Run every train think due at `time` against its published state.
    /// Returns whether the published trajectory changed.
    pub(super) fn advance(&mut self, state: &mut EntityState, time: i32) -> bool {
        let mut changed = false;
        for _ in 0..MAX_TRAIN_STEPS {
            match self.phase {
                TrainPhase::Setup => self.reached(state, time),
                TrainPhase::Moving { end } if time >= end => self.reached(state, end),
                TrainPhase::Waiting { until } if time >= until => self.begin_moving(state, until),
                _ => return changed,
            }
            changed = true;
        }
        // Still behind after a pathological stall: resume from now.
        if let TrainPhase::Moving { .. } = self.phase {
            self.begin_moving(state, time);
        }
        true
    }

    /// G_MoverTeam pushed `pos.trTime` back by `delta` because something
    /// blocked the train; its pending think moves with it.
    pub(super) fn delay(&mut self, delta: i32) {
        match &mut self.phase {
            TrainPhase::Moving { end } => *end = end.saturating_add(delta),
            TrainPhase::Waiting { until } => *until = until.saturating_add(delta),
            TrainPhase::Setup | TrainPhase::Stopped => {}
        }
    }

    /// Reached_Train: aim at the corner after the one just reached.
    fn reached(&mut self, state: &mut EntityState, at: i32) {
        let Some(corner) = self.next.and_then(|index| self.corners.get(index)) else {
            self.phase = TrainPhase::Stopped;
            return;
        };
        let Some(following) = corner.next.filter(|&index| index < self.corners.len()) else {
            self.phase = TrainPhase::Stopped;
            return;
        };
        let pos1 = corner.origin;
        let pos2 = self.corners[following].origin;
        let speed = if corner.speed != 0.0 { corner.speed } else { self.speed }.max(1.0);
        let wait = corner.wait;
        let length = (0..3).map(|axis| (pos2[axis] - pos1[axis]).powi(2)).sum::<f32>().sqrt();
        // SetMoverState never leaves a zero duration.
        self.duration = ((length * 1000.0 / speed) as i32).max(1);
        self.next = Some(following);

        let scale = 1000.0 / self.duration as f32;
        set_entity_vec3(state, "pos.trBase", pos1);
        set_entity_vec3(state, "origin", pos1);
        set_entity_vec3(state, "pos.trDelta", std::array::from_fn(|axis| (pos2[axis] - pos1[axis]) * scale));
        set_entity_i32(state, "pos.trDuration", self.duration);
        set_entity_i32(state, "pos.trTime", at);
        if wait != 0.0 {
            set_entity_i32(state, "pos.trType", TR_STATIONARY);
            self.phase = TrainPhase::Waiting { until: at.saturating_add((wait * 1000.0) as i32) };
        } else {
            set_entity_i32(state, "pos.trType", TR_NONLINEAR_STOP);
            self.phase = TrainPhase::Moving { end: at.saturating_add(self.duration) };
        }
    }

    /// Think_BeginMoving: the corner wait is over.
    fn begin_moving(&mut self, state: &mut EntityState, at: i32) {
        set_entity_i32(state, "pos.trTime", at);
        set_entity_i32(state, "pos.trType", TR_NONLINEAR_STOP);
        self.phase = TrainPhase::Moving { end: at.saturating_add(self.duration) };
    }
}

/// Spawn every drawable brush entity. `first_number` is the first free
/// entity slot and `max_entities` the exclusive limit.
pub(super) fn build_brush_mover_entities(
    brush_entities: &[MapBrushEntity],
    first_number: u16,
    max_entities: u16,
) -> BrushMovers {
    let mut entities = Vec::new();
    let mut sources = Vec::new();
    let mut trains = Vec::new();
    let mut models = Vec::<String>::new();
    for (source, entity) in brush_entities.iter().enumerate() {
        let vars = SpawnVars(&entity.spawn_vars);
        if !spawns_in_local_gametype(&vars) {
            continue;
        }
        let Some(mover) = spawn_mover(entity) else {
            continue;
        };
        let number = first_number.saturating_add(entities.len() as u16);
        if number >= max_entities {
            eprintln!("LOCAL SERVER MOVERS: entity limit reached; remaining brush entities skipped");
            break;
        }
        let mut state = EntityState {
            number,
            fields: [0; ENTITY_FIELDS.len()],
        };
        set_entity_i32(&mut state, "eType", ET_MOVER);
        // SV_SetBrushModel + SV_LinkEntity: modelindex = N, solid = SOLID_BMODEL.
        set_entity_i32(&mut state, "modelindex", entity.model as i32);
        set_entity_i32(&mut state, "solid", SOLID_BMODEL);
        set_entity_i32(&mut state, "pos.trType", mover.pos_type);
        set_entity_vec3(&mut state, "pos.trBase", mover.pos_base);
        set_entity_vec3(&mut state, "pos.trDelta", mover.pos_delta);
        set_entity_i32(&mut state, "pos.trDuration", mover.pos_duration);
        set_entity_i32(&mut state, "pos.trTime", mover.pos_time);
        set_entity_vec3(&mut state, "origin", mover.pos_base);
        set_entity_i32(&mut state, "apos.trType", mover.apos_type);
        set_entity_vec3(&mut state, "apos.trBase", mover.apos_base);
        set_entity_vec3(&mut state, "apos.trDelta", mover.apos_delta);
        set_entity_i32(&mut state, "apos.trDuration", mover.apos_duration);
        set_entity_i32(&mut state, "apos.trTime", mover.apos_time);
        set_entity_vec3(&mut state, "angles", mover.apos_base);
        set_entity_i32(&mut state, "eFlags", mover.e_flags);
        set_entity_i32(&mut state, "eFlags2", mover.e_flags2);
        set_entity_i32(&mut state, "constantLight", mover.constant_light);
        if mover.speed != 0.0 {
            super::set_entity_f32(&mut state, "speed", mover.speed);
        }
        if let Some(name) = &mover.model2 {
            let index = match models.iter().position(|known| known.eq_ignore_ascii_case(name)) {
                Some(index) => Some(index),
                None if models.len() < MAX_MOVER_MODELS => {
                    models.push(name.clone());
                    Some(models.len() - 1)
                }
                None => {
                    eprintln!("LOCAL SERVER MOVERS: model2 limit reached; {name} not drawn");
                    None
                }
            };
            if let Some(index) = index {
                set_entity_i32(&mut state, "modelindex2", index as i32 + 1);
            }
        }
        if let Some(spec) = mover.train.filter(|spec| !spec.corners.is_empty()) {
            trains.push(Train {
                entity: entities.len(),
                speed: spec.speed,
                next: Some(0),
                corners: spec.corners,
                duration: 1,
                phase: TrainPhase::Setup,
            });
        }
        entities.push(state);
        sources.push(source);
    }
    BrushMovers { entities, sources, trains, models }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brush(model: u32, vars: &[(&str, &str)]) -> MapBrushEntity {
        MapBrushEntity {
            model,
            mins: [0.0, 0.0, 0.0],
            maxs: [64.0, 16.0, 128.0],
            train_corners: Vec::new(),
            spawn_vars: vars.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect(),
        }
    }

    fn field(state: &EntityState, name: &str) -> u32 {
        let index = ENTITY_FIELDS.iter().position(|(n, _)| *n == name).unwrap();
        state.fields[index]
    }

    #[test]
    fn stained_glass_door_is_a_visible_bmodel_mover() {
        let entities = build_brush_mover_entities(
            &[brush(204, &[("classname", "func_door"), ("spawnflags", "8"), ("angle", "-1"), ("model", "*204")])],
            100,
            1024,
        )
        .entities;
        assert_eq!(entities.len(), 1);
        assert_eq!(field(&entities[0], "eType") as i32, ET_MOVER);
        assert_eq!(field(&entities[0], "modelindex"), 204);
        assert_eq!(field(&entities[0], "solid") as i32, SOLID_BMODEL);
    }

    #[test]
    fn start_open_door_rests_at_the_open_position() {
        let entities = build_brush_mover_entities(
            &[brush(3, &[("classname", "func_door"), ("spawnflags", "1"), ("angle", "-1")])],
            100,
            1024,
        )
        .entities;
        // Up by 128 height minus the default 8 unit lip.
        let z = f32::from_bits(field(&entities[0], "pos.trBase[2]"));
        assert_eq!(z, 120.0);
    }

    #[test]
    fn start_off_usables_triggers_and_filtered_entities_are_not_sent() {
        let entities = build_brush_mover_entities(
            &[
                brush(1, &[("classname", "func_usable"), ("spawnflags", "1")]),
                brush(2, &[("classname", "trigger_multiple")]),
                brush(3, &[("classname", "func_door"), ("notfree", "1")]),
                brush(4, &[("classname", "func_door"), ("gametype", "siege ctf")]),
                brush(5, &[("classname", "func_usable")]),
            ],
            100,
            1024,
        )
        .entities;
        assert_eq!(entities.len(), 1);
        assert_eq!(field(&entities[0], "modelindex"), 5);
    }

    fn corner(origin: [f32; 3], speed: f32, wait: f32, next: Option<usize>) -> TrainCorner {
        TrainCorner { origin, speed, wait, next }
    }

    fn train_entity(vars: &[(&str, &str)], corners: Vec<TrainCorner>) -> MapBrushEntity {
        let mut entity = brush(9, vars);
        entity.train_corners = corners;
        entity
    }

    fn pos_type(state: &EntityState) -> i32 {
        field(state, "pos.trType") as i32
    }

    fn base_x(state: &EntityState) -> f32 {
        f32::from_bits(field(state, "pos.trBase[0]"))
    }

    #[test]
    fn model2_is_published_as_a_shared_modelindex2() {
        let movers = build_brush_mover_entities(
            &[
                brush(1, &[("classname", "func_door"), ("model2", "models/a.md3")]),
                brush(2, &[("classname", "func_static"), ("model2", "MODELS/A.md3")]),
                brush(3, &[("classname", "func_static"), ("model2", "models/b.md3")]),
                brush(4, &[("classname", "func_static")]),
            ],
            100,
            1024,
        );
        assert_eq!(movers.models, ["models/a.md3", "models/b.md3"]);
        let indices: Vec<u32> = movers.entities.iter().map(|e| field(e, "modelindex2")).collect();
        assert_eq!(indices, [1, 1, 2, 0]);
    }

    #[test]
    fn trains_with_a_targetname_only_move_when_start_on() {
        let corners = || vec![corner([0.0; 3], 0.0, 0.0, Some(1)), corner([64.0, 0.0, 0.0], 0.0, 0.0, None)];
        let idle = build_brush_mover_entities(
            &[train_entity(&[("classname", "func_train"), ("target", "c0"), ("targetname", "t"), ("spawnflags", "2"), ("origin", "5 6 7")], corners())],
            100,
            1024,
        );
        assert!(idle.trains.is_empty());
        // Rests at its own origin, not at the first corner.
        assert_eq!(f32::from_bits(field(&idle.entities[0], "pos.trBase[2]")), 7.0);
        let started = build_brush_mover_entities(
            &[train_entity(&[("classname", "func_train"), ("target", "c0"), ("targetname", "t"), ("spawnflags", "1")], corners())],
            100,
            1024,
        );
        assert_eq!(started.trains.len(), 1);
        let untargeted = build_brush_mover_entities(
            &[train_entity(&[("classname", "func_train"), ("target", "c0")], corners())],
            100,
            1024,
        );
        assert_eq!(untargeted.trains.len(), 1);
    }

    #[test]
    fn train_follows_its_path_and_stops_at_the_last_corner() {
        let corners = vec![
            corner([0.0; 3], 200.0, 0.0, Some(1)),
            corner([200.0, 0.0, 0.0], 0.0, 0.0, Some(2)),
            corner([200.0, 100.0, 0.0], 0.0, 0.0, None),
        ];
        let mut movers = build_brush_mover_entities(
            &[train_entity(&[("classname", "func_train"), ("target", "c0"), ("speed", "100")], corners)],
            100,
            1024,
        );
        let (train, state) = (&mut movers.trains[0], &mut movers.entities[0]);
        assert_eq!(pos_type(state), TR_STATIONARY);

        // First think: leave corner 0 for corner 1 at that corner's speed.
        train.advance(state, 1000);
        assert_eq!(pos_type(state), TR_NONLINEAR_STOP);
        assert_eq!(field(state, "pos.trTime"), 1000);
        assert_eq!(field(state, "pos.trDuration"), 1000);
        assert_eq!(f32::from_bits(field(state, "pos.trDelta[0]")), 200.0);
        train.advance(state, 1999);
        assert_eq!(field(state, "pos.trTime"), 1000);

        // Reaching corner 1 uses the train speed (100) toward corner 2.
        train.advance(state, 2000);
        assert_eq!(base_x(state), 200.0);
        assert_eq!(field(state, "pos.trTime"), 2000);
        assert_eq!(field(state, "pos.trDuration"), 1000);
        assert_eq!(f32::from_bits(field(state, "pos.trDelta[1]")), 100.0);

        // Corner 2 has no successor: the train just stops there.
        train.advance(state, 3000);
        train.advance(state, 9000);
        assert_eq!(field(state, "pos.trTime"), 2000);
        assert_eq!(base_x(state), 200.0);
    }

    #[test]
    fn train_waits_at_a_corner_before_moving_on() {
        let corners = vec![
            corner([0.0; 3], 100.0, 2.0, Some(1)),
            corner([100.0, 0.0, 0.0], 0.0, 0.0, None),
        ];
        let mut movers = build_brush_mover_entities(
            &[train_entity(&[("classname", "func_train"), ("target", "c0")], corners)],
            100,
            1024,
        );
        let (train, state) = (&mut movers.trains[0], &mut movers.entities[0]);
        train.advance(state, 1000);
        assert_eq!(pos_type(state), TR_STATIONARY);
        train.advance(state, 2999);
        assert_eq!(pos_type(state), TR_STATIONARY);
        train.advance(state, 3000);
        assert_eq!(pos_type(state), TR_NONLINEAR_STOP);
        assert_eq!(field(state, "pos.trTime"), 3000);
    }

    #[test]
    fn looping_train_survives_a_long_stall() {
        let corners = vec![
            corner([0.0; 3], 500.0, 0.0, Some(1)),
            corner([1.0, 0.0, 0.0], 500.0, 0.0, Some(0)),
        ];
        let mut movers = build_brush_mover_entities(
            &[train_entity(&[("classname", "func_train"), ("target", "c0")], corners)],
            100,
            1024,
        );
        let (train, state) = (&mut movers.trains[0], &mut movers.entities[0]);
        train.advance(state, 0);
        train.advance(state, 1_000_000_000);
        assert_eq!(pos_type(state), TR_NONLINEAR_STOP);
    }

    #[test]
    fn rotating_mover_spins_about_yaw_by_default() {
        let entities =
            build_brush_mover_entities(&[brush(7, &[("classname", "func_rotating")])], 100, 1024).entities;
        assert_eq!(field(&entities[0], "apos.trType") as i32, TR_LINEAR);
        assert_eq!(f32::from_bits(field(&entities[0], "apos.trDelta[1]")), 100.0);
    }
}
