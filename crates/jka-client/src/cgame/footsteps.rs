//! Player footsteps: `CG_PlayerAnimEvents` (the AEV_FOOTSTEP half),
//! `CG_PlayerFootsteps` and `_PlayerFootStep` from `codemp/cgame/cg_players.c`.
//!
//! Stock humanoid players never ask the server for footsteps. The animation
//! file keys them to frames of the walk/run cycles, the client notices the
//! legs bone crossing one, traces down from the foot bolt and lets the surface's
//! material pick the sound, a puff of dust and a print. This module is the pure
//! part of that: which frames fire, what a material does, and the two traces.
//! Playing the result belongs to the presenters.
//!
//! `cg_footsteps` gates it the way jaPRO does: 0 nothing, 1 sounds, 2 adds the
//! material effects, 3 adds the prints, 4 is the debugging "always" level.

use jka_assets::{
    animation::AnimationSet,
    animevents::{AnimEvent, AnimEventType, FootstepType},
};
use jka_movement::{CollisionWorld, TraceQuery, TraceResult, TraceWorld, ENTITY_NONE};
use crate::net::{PredictionWorld, SolidEntity};

// surfaceflags.h material ids.
const MATERIAL_SOLIDWOOD: u32 = 1;
const MATERIAL_HOLLOWWOOD: u32 = 2;
const MATERIAL_SOLIDMETAL: u32 = 3;
const MATERIAL_HOLLOWMETAL: u32 = 4;
const MATERIAL_SHORTGRASS: u32 = 5;
const MATERIAL_LONGGRASS: u32 = 6;
const MATERIAL_DIRT: u32 = 7;
const MATERIAL_SAND: u32 = 8;
const MATERIAL_GRAVEL: u32 = 9;
pub const MATERIAL_SNOW: u32 = 14;
const MATERIAL_MUD: u32 = 17;
const MATERIAL_FABRIC: u32 = 21;
const MATERIAL_CANVAS: u32 = 22;
const MATERIAL_RUBBER: u32 = 24;
const MATERIAL_PLASTIC: u32 = 25;
const MATERIAL_CARPET: u32 = 27;
const MATERIAL_MASK: u32 = 0x1f;

const CONTENTS_LAVA: i32 = 0x2;
const CONTENTS_WATER: i32 = 0x4;
/// `MASK_WATER` (there is no slime in JKA maps).
const MASK_WATER: i32 = CONTENTS_WATER | CONTENTS_LAVA;
/// `MASK_PLAYERSOLID`: solid, player clip, body and terrain.
const MASK_PLAYERSOLID: i32 = 0x1 | 0x10 | 0x100 | 0x1000;
/// `FOOTSTEP_DISTANCE`: how far below the foot a surface still counts.
const FOOTSTEP_DISTANCE: f32 = 32.0;
/// `CG_PlayerFootsteps` lifts the bolt "a bit for coplanar".
const FOOT_LIFT: f32 = 15.0;
/// The radius of the print `_PlayerFootStep` leaves.
pub const FOOTPRINT_RADIUS: f32 = 6.0;

/// `cg_footsteps` levels.
pub const FOOTSTEPS_SOUNDS: u8 = 1;
pub const FOOTSTEPS_EFFECTS: u8 = 2;
pub const FOOTSTEPS_MARKS: u8 = 3;

/// Which `cg_footsteps` stages are on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FootstepStages {
    pub sounds: bool,
    pub effects: bool,
    pub marks: bool,
}

impl FootstepStages {
    pub fn from_level(level: u8) -> Self {
        Self {
            sounds: level >= FOOTSTEPS_SOUNDS,
            effects: level >= FOOTSTEPS_EFFECTS,
            marks: level >= FOOTSTEPS_MARKS,
        }
    }

    pub fn any(self) -> bool {
        self.sounds || self.effects || self.marks
    }
}

/// `CG_PlayerAnimEvents` for the AEV_FOOTSTEP events of one list: the feet
/// whose key frame the bone crossed going from `old_frame` to `frame` of
/// `anim`. `chance` is `Q_irand(0, 99)`.
///
/// A jump of more than one frame (a slow frame, or the speed force) fires the
/// keys it passed, but only within 3 frames of either end and only inside the
/// same animation, exactly as the engine does; a looping animation also fires
/// keys it wrapped past.
pub fn footsteps_between(
    events: &[AnimEvent],
    animations: &AnimationSet,
    anim: i32,
    old_frame: i32,
    frame: i32,
    mut chance: impl FnMut() -> i32,
) -> Vec<FootstepType> {
    // CG_TriggerAnimSounds only looks when the bone frame changed.
    if old_frame == frame {
        return Vec::new();
    }
    let jumped = (old_frame - frame).abs() > 1;
    let (mut backward, mut looping, mut first, mut last) = (false, false, 0, 0);
    if jumped {
        if let Some(animation) = animations.get(anim) {
            backward = animation.frame_lerp < 0;
            if animation.loop_frames != -1 {
                looping = true;
                first = i32::from(animation.first_frame);
                last = first + i32::from(animation.num_frames);
            }
        }
    }
    let mut steps = Vec::new();
    for event in events {
        let Some((foot, probability)) = event.footstep.filter(|_| event.event_type == AnimEventType::Footstep) else {
            continue;
        };
        let key = event.key_frame;
        let mut hit = key == frame;
        if !hit && jumped && ((old_frame - key).abs() <= 3 || (frame - key).abs() <= 3) {
            let in_anim = key >= first && key < last;
            hit = if backward {
                (old_frame > key && frame < key) || (looping && in_anim && old_frame > key && frame > old_frame)
            } else {
                (old_frame < key && frame > key) || (looping && in_anim && old_frame < key && frame < old_frame)
            };
        }
        if hit && (probability == 0 || probability > chance()) {
            steps.push(foot);
        }
    }
    steps
}

/// Which pool of `sound/player/footsteps/` a surface uses (`footstep_t`, minus
/// the walk/run split that the foot type decides).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepSound {
    Stone,
    Metal,
    Pipe,
    Snow,
    Sand,
    Grass,
    Dirt,
    Mud,
    Gravel,
    Rug,
    Wood,
}

impl StepSound {
    /// `sound/player/footsteps/<stem><1-4>.wav`: the `_PlayerFootStep` /
    /// EV_FOOTSTEP sound for a walk or (heavy) run step. `variant` is 0..4.
    pub fn path(self, heavy: bool, variant: usize) -> String {
        let (walk, run) = match self {
            Self::Stone => ("stone_step", "stone_run"),
            Self::Metal => ("metal_step", "metal_run"),
            Self::Pipe => ("pipe_step", "pipe_run"),
            Self::Snow => ("snow_step", "snow_run"),
            Self::Sand => ("sand_walk", "sand_run"),
            Self::Grass => ("grass_step", "grass_run"),
            Self::Dirt => ("dirt_step", "dirt_run"),
            Self::Mud => ("mud_walk", "mud_run"),
            Self::Gravel => ("gravel_walk", "gravel_run"),
            Self::Rug => ("rug_step", "rug_run"),
            Self::Wood => ("wood_walk", "wood_run"),
        };
        format!("sound/player/footsteps/{}{}.wav", if heavy { run } else { walk }, (variant & 3) + 1)
    }
}

/// What walking on a material does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialStep {
    pub sound: StepSound,
    /// The `cgs.effects.footstep*` effect.
    pub effect: Option<&'static str>,
    /// Soft ground that keeps a print (`bMark`).
    pub mark: bool,
}

/// The `switch ( trace.surfaceFlags & MATERIAL_MASK )` of `_PlayerFootStep`.
/// Everything unlisted (glass, water, tiles, concrete, rock, ...) is stone.
pub fn material_step(material: u32) -> MaterialStep {
    let (sound, effect, mark) = match material & MATERIAL_MASK {
        MATERIAL_MUD => (StepSound::Mud, Some("materials/mud"), true),
        MATERIAL_DIRT => (StepSound::Dirt, Some("materials/sand"), true),
        MATERIAL_SAND => (StepSound::Sand, Some("materials/sand"), true),
        MATERIAL_SNOW => (StepSound::Snow, Some("materials/snow"), true),
        MATERIAL_SHORTGRASS | MATERIAL_LONGGRASS => (StepSound::Grass, None, false),
        MATERIAL_SOLIDMETAL => (StepSound::Metal, None, false),
        MATERIAL_HOLLOWMETAL => (StepSound::Pipe, None, false),
        MATERIAL_GRAVEL => (StepSound::Gravel, Some("materials/gravel"), false),
        MATERIAL_CARPET | MATERIAL_FABRIC | MATERIAL_CANVAS | MATERIAL_RUBBER | MATERIAL_PLASTIC => {
            (StepSound::Rug, None, false)
        }
        MATERIAL_SOLIDWOOD | MATERIAL_HOLLOWWOOD => (StepSound::Wood, None, false),
        _ => (StepSound::Stone, None, false),
    };
    MaterialStep { sound, effect, mark }
}

/// `rand() & 3` of `cgs.media.footsteps[soundType][rand()&3]`: a variation
/// picked from what is unique to this step.
pub fn sound_variant(impact: &FootstepImpact, time: i32) -> usize {
    let mixed = (time as u32).wrapping_mul(0x9E37_79B9)
        ^ impact.position[0].to_bits()
        ^ impact.position[1].to_bits().rotate_left(13)
        ^ u32::from(impact.entity).wrapping_mul(0x85EB_CA6B);
    ((mixed >> 13) & 3) as usize
}

/// `CG_PlayerFootsteps` skips droids, creatures and machines (AT-ST, claw
/// monster, fish, glider, probe droids, sentries, swamp trooper, ...).
pub fn npc_class_steps(npc_class: i32) -> bool {
    !matches!(npc_class, 1 | 4 | 7 | 8 | 10 | 16 | 30 | 32 | 34 | 35 | 39 | 41 | 42 | 45)
}

/// One footfall that found ground, ready to be sounded and drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct FootstepImpact {
    /// The player entity (`S_StartSound(NULL, clientNum, CHAN_BODY, ...)`).
    pub entity: u16,
    pub foot: FootstepType,
    /// `trace.surfaceFlags & MATERIAL_MASK`.
    pub material: u32,
    /// `trace.endpos`, in JKA units.
    pub position: [f32; 3],
    /// `trace.plane.normal`.
    pub normal: [f32; 3],
    /// `cent->pe.legs.yawAngle`: the way the toes point.
    pub yaw: f32,
}

/// `_PlayerFootStep`'s traces from a foot bolt: first `MASK_WATER` (a foot in
/// water makes no footstep at all), then the ground up to 32 units below.
/// Returns `(end, normal, surfaceFlags)`.
pub fn trace_footstep(world: &mut impl TraceWorld, foot_bolt: [f32; 3]) -> Option<([f32; 3], [f32; 3], i32)> {
    let start = [foot_bolt[0], foot_bolt[1], foot_bolt[2] + FOOT_LIFT];
    let end = [start[0], start[1], start[2] - FOOTSTEP_DISTANCE];
    let query = |mask| TraceQuery {
        start,
        mins: [-7.0, -7.0, 0.0],
        maxs: [7.0, 7.0, 2.0],
        end,
        pass_entity: ENTITY_NONE,
        mask,
    };
    if world.trace(query(MASK_WATER)).fraction < 1.0 {
        return None;
    }
    let ground = world.trace(query(MASK_PLAYERSOLID));
    // "no shadow if too high"
    (ground.fraction < 1.0).then_some((ground.end, ground.normal, ground.surface_flags))
}

/// Apply the TaystJK `CG_Trace` footstep fix to the *ground* probe alone.
/// The initial MASK_WATER test remains the legacy world CM trace, matching
/// `_PlayerFootStep`'s original first trace. The player's own entity number is
/// excluded from the solid list, like `CG_Trace(..., cent->currentState.number)`.
pub struct FootstepEntityTrace<'a> {
    world: &'a mut CollisionWorld,
    solids: &'a [SolidEntity],
    viewer_client: i32,
    skip_entity: i32,
}

impl<'a> FootstepEntityTrace<'a> {
    pub fn new(world: &'a mut CollisionWorld, solids: &'a [SolidEntity], viewer_client: i32, skip_entity: i32) -> Self {
        Self { world, solids, viewer_client, skip_entity }
    }
}

impl TraceWorld for FootstepEntityTrace<'_> {
    fn trace(&mut self, mut query: TraceQuery) -> TraceResult {
        if query.mask != MASK_PLAYERSOLID {
            return self.world.trace(query);
        }
        query.pass_entity = self.skip_entity;
        PredictionWorld {
            world: &mut *self.world,
            solids: self.solids,
            client_num: self.viewer_client,
        }.trace(query)
    }

    fn point_contents(&mut self, point: [f32; 3], pass_entity: i32) -> i32 {
        self.world.point_contents(point, pass_entity)
    }
}

/// Traces each of `steps` (the feet that fell this frame) from the bolt
/// `foot_bolt` names and keeps the ones that found ground.
pub fn push_footsteps(
    world: &mut impl TraceWorld,
    out: &mut Vec<FootstepImpact>,
    entity: u16,
    yaw: f32,
    steps: &[FootstepType],
    mut foot_bolt: impl FnMut(FootstepType) -> [f32; 3],
) {
    for &foot in steps {
        if let Some((position, normal, flags)) = trace_footstep(world, foot_bolt(foot)) {
            out.push(FootstepImpact { entity, foot, material: flags as u32 & MATERIAL_MASK, position, normal, yaw });
        }
    }
}

/// Where a foot is when the model has not been posed: straight below the
/// entity's origin (players stand 24 units off the ground, boots a little above it).
pub fn unposed_foot(entity_origin: [f32; 3]) -> [f32; 3] {
    [entity_origin[0], entity_origin[1], entity_origin[2] - 22.0]
}

// ---------------------------------------------------------------------------
// Jedi Outcast. JK2 has no animevents.cfg: `CG_RunLerpFrame` (mvsdk
// cgame/cg_players.c) calls `CG_FootStep` when the legs frame crosses a
// hard-coded step frame of the walking animations, and `CG_FootstepForSurface`
// traces from the lower-lumbar bolt to pick `boot` or `clank`. No dust, no prints.
// ---------------------------------------------------------------------------

/// JK2 `SURF_METALSTEPS` / `SURF_NOSTEPS` (JK2 surfaceflags.h).
const JK2_SURF_METALSTEPS: i32 = 0x0000_8000;
const JK2_SURF_NOSTEPS: i32 = 0x0040_0000;
/// `DEFAULT_MINS_2` / `DEFAULT_MAXS_2` and the 64 unit probe of `CG_FootstepForSurface`.
const JK2_HULL_MINS: [f32; 3] = [-15.0, -15.0, -24.0];
const JK2_HULL_MAXS: [f32; 3] = [15.0, 15.0, 40.0];
const JK2_PROBE_LENGTH: f32 = 64.0;

/// `CG_InWalkingAnim`: the last step frame of a legs animation (relative to its
/// first frame), 0 for animations that make no steps. JK2 steps on every
/// `BOTH_WALK1..BOTH_RUNAWAY1` entry, turns and strafes included.
fn jk2_final_step_frame(name: &str) -> i32 {
    match name {
        "BOTH_WALL_RUN_RIGHT" | "BOTH_WALL_RUN_LEFT" => 8,
        "BOTH_WALK1" | "BOTH_WALK2" | "BOTH_WALK3" | "BOTH_WALK4" | "BOTH_WALK5" | "BOTH_WALK6"
        | "BOTH_WALK7" | "BOTH_WALK8" | "BOTH_WALK9" | "BOTH_WALK10" | "BOTH_WALKTORUN1"
        | "BOTH_RUN1" | "BOTH_RUN1START" | "BOTH_RUN1STOP" | "BOTH_RUN2" | "BOTH_RUNINJURED1"
        | "BOTH_STRAFE_LEFT1" | "BOTH_STRAFE_RIGHT1" | "BOTH_RUNSTRAFE_LEFT1" | "BOTH_RUNSTRAFE_RIGHT1"
        | "BOTH_TURN_LEFT1" | "BOTH_TURN_RIGHT1" | "BOTH_TURNSTAND1" | "BOTH_TURNSTAND2"
        | "BOTH_TURNSTAND3" | "BOTH_TURNSTAND4" | "BOTH_TURNSTAND5" | "BOTH_TURNCROUCH1"
        | "BOTH_RUNAWAY1" | "BOTH_WALKBACK1" | "LEGS_WALKBACK1" => 18,
        "BOTH_WALKBACK2" | "BOTH_RUNBACK1" | "BOTH_RUNBACK2" | "LEGS_WALKBACK2" | "LEGS_RUNBACK1"
        | "LEGS_RUNBACK2" => 9,
        _ => 0,
    }
}

/// Whether the legs bone going from `old_frame` to `frame` of `anim` (a
/// `CG_RunLerpFrame` advance) is a JK2 footfall, and if the animation steps at
/// all, whether it is a wall run (which needs no ground under it).
pub fn jk2_step_due(animations: &AnimationSet, anim: i32, old_frame: i32, frame: i32) -> Option<bool> {
    let name = AnimationSet::name(anim)?;
    let last = jk2_final_step_frame(name);
    if last == 0 || old_frame == frame {
        return None;
    }
    let first = i32::from(animations.get(anim)?.first_frame);
    let crossed = |at: i32| frame >= first + at && old_frame < first + at;
    let wall_run = name.starts_with("BOTH_WALL_RUN");
    let due = if wall_run {
        crossed(2) || crossed(last) || crossed(12) || crossed(16)
    } else {
        crossed(3) || crossed(last)
    } || (old_frame > frame && frame > first + 1); // "missed one": the loop wrapped
    due.then_some(wall_run)
}

/// A JK2 footfall that found a surface that makes steps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Jk2Footstep {
    pub entity: u16,
    pub position: [f32; 3],
    /// `SURF_METALSTEPS`: `clank` instead of `boot`.
    pub metal: bool,
}

/// `CG_FootstepForSurface`: a player-sized hull from the lumbar bolt, 64 units
/// along the bolt's down axis. Nothing under the player, or `SURF_NOSTEPS`, is silence.
pub fn jk2_trace_footstep(
    world: &mut impl TraceWorld,
    entity: u16,
    origin: [f32; 3],
    down: [f32; 3],
) -> Option<Jk2Footstep> {
    let end = [
        origin[0] + down[0] * JK2_PROBE_LENGTH,
        origin[1] + down[1] * JK2_PROBE_LENGTH,
        origin[2] + down[2] * JK2_PROBE_LENGTH,
    ];
    let hit = world.trace(TraceQuery {
        start: origin,
        mins: JK2_HULL_MINS,
        maxs: JK2_HULL_MAXS,
        end,
        pass_entity: i32::from(entity),
        mask: MASK_PLAYERSOLID,
    });
    if hit.fraction >= 1.0 || hit.surface_flags & JK2_SURF_NOSTEPS != 0 {
        return None;
    }
    Some(Jk2Footstep { entity, position: origin, metal: hit.surface_flags & JK2_SURF_METALSTEPS != 0 })
}

/// `cgs.media.footsteps[type][rand()&3]` for a JK2 step: `boot`, or `clank` on
/// metal, or `splash` for the water events.
pub fn jk2_step_sound(kind: Jk2StepKind, variant: usize) -> String {
    let stem = match kind {
        Jk2StepKind::Boot => "boot",
        Jk2StepKind::Metal => "clank",
        Jk2StepKind::Splash => "splash",
    };
    format!("sound/player/footsteps/{stem}{}.wav", (variant & 3) + 1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Jk2StepKind {
    Boot,
    Metal,
    Splash,
}

/// `rand() & 3` for a JK2 step, from what is unique to it.
pub fn jk2_sound_variant(entity: u16, time: i32) -> usize {
    let mixed = (time as u32).wrapping_mul(0x9E37_79B9) ^ u32::from(entity).wrapping_mul(0x85EB_CA6B);
    ((mixed >> 13) & 3) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use jka_assets::{
        animation::{animation_index, parse_animation_cfg},
        animevents::parse_animation_events,
    };
    use jka_movement::TraceResult;

    fn animations() -> AnimationSet {
        // A walk (looping) and a death (no loop, plays backwards in the last test).
        parse_animation_cfg(b"BOTH_WALK1 100 30 0 20\nBOTH_DEATH1 200 20 -1 -20\n").unwrap()
    }

    fn walk() -> i32 {
        animation_index("BOTH_WALK1").unwrap() as i32
    }

    fn events(cfg: &str) -> Vec<AnimEvent> {
        parse_animation_events(&format!("LOWEREVENTS\n{{\n{cfg}\n}}\n"), &animations()).legs
    }

    #[test]
    fn a_step_fires_when_its_frame_is_reached() {
        let steps = events("BOTH_WALK1 AEV_FOOTSTEP 4 footstep_r 0\nBOTH_WALK1 AEV_FOOTSTEP 19 footstep_l 0");
        let mut never = || -> i32 { panic!("probability 0 never rolls") };
        assert_eq!(footsteps_between(&steps, &animations(), walk(), 103, 104, &mut never), vec![FootstepType::Right]);
        assert!(footsteps_between(&steps, &animations(), walk(), 104, 104, &mut never).is_empty());
        assert_eq!(footsteps_between(&steps, &animations(), walk(), 118, 119, &mut never), vec![FootstepType::Left]);
    }

    #[test]
    fn a_slow_frame_still_fires_the_keys_it_skipped() {
        let steps = events("BOTH_WALK1 AEV_FOOTSTEP 4 footstep_r 0");
        // 102 -> 106 passes 104 (and is within 3 frames of it).
        assert_eq!(footsteps_between(&steps, &animations(), walk(), 102, 106, || 0), vec![FootstepType::Right]);
        // 100 -> 108 passes it too, but neither end is within 3 frames: the engine drops it.
        assert!(footsteps_between(&steps, &animations(), walk(), 100, 108, || 0).is_empty());
    }

    #[test]
    fn a_looping_walk_fires_a_key_it_passed_before_wrapping() {
        let steps = events("BOTH_WALK1 AEV_FOOTSTEP 28 footstep_l 0");
        // 126 -> 101: the cycle wrapped after passing 128, which is within 3 of the old frame.
        assert_eq!(footsteps_between(&steps, &animations(), walk(), 126, 101, || 0), vec![FootstepType::Left]);
        // 110 -> 101 is a backwards jump of a forwards clip: nothing lies between.
        assert!(footsteps_between(&steps, &animations(), walk(), 110, 101, || 0).is_empty());
    }

    #[test]
    fn a_backwards_animation_fires_keys_going_down() {
        let steps = events("BOTH_DEATH1 AEV_FOOTSTEP 5 footstep_r 0");
        let death = animation_index("BOTH_DEATH1").unwrap() as i32;
        assert_eq!(footsteps_between(&steps, &animations(), death, 208, 203, || 0), vec![FootstepType::Right]);
        // Both ends further than 3 frames from the key: the engine drops it.
        assert!(footsteps_between(&steps, &animations(), death, 212, 200, || 0).is_empty());
    }

    #[test]
    fn a_percentage_step_rolls_against_the_chance() {
        let steps = events("BOTH_WALK1 AEV_FOOTSTEP 4 footstep_heavy_r 40");
        assert_eq!(footsteps_between(&steps, &animations(), walk(), 103, 104, || 39), vec![FootstepType::HeavyRight]);
        assert!(footsteps_between(&steps, &animations(), walk(), 103, 104, || 40).is_empty());
    }

    #[test]
    fn materials_pick_the_stock_sound_effect_and_print() {
        let sand = material_step(MATERIAL_SAND);
        assert_eq!((sand.sound, sand.effect, sand.mark), (StepSound::Sand, Some("materials/sand"), true));
        // Dirt puffs sand dust, gravel puffs but leaves no print, grass and metal do neither.
        assert_eq!(material_step(MATERIAL_DIRT).effect, Some("materials/sand"));
        assert!(!material_step(MATERIAL_GRAVEL).mark && material_step(MATERIAL_GRAVEL).effect.is_some());
        assert_eq!(material_step(MATERIAL_LONGGRASS), MaterialStep { sound: StepSound::Grass, effect: None, mark: false });
        assert_eq!(material_step(MATERIAL_HOLLOWMETAL).sound, StepSound::Pipe);
        assert_eq!(material_step(MATERIAL_CARPET).sound, StepSound::Rug);
        assert_eq!(material_step(0).sound, StepSound::Stone, "unset material");
        assert_eq!(material_step(26).sound, StepSound::Stone, "tiles");
        // surfaceFlags carry other bits above the material.
        assert_eq!(material_step(MATERIAL_MUD | 0x4000_0000).sound, StepSound::Mud);
    }

    #[test]
    fn sound_paths_use_the_walk_or_run_stem() {
        assert_eq!(StepSound::Sand.path(false, 0), "sound/player/footsteps/sand_walk1.wav");
        assert_eq!(StepSound::Sand.path(true, 3), "sound/player/footsteps/sand_run4.wav");
        assert_eq!(StepSound::Stone.path(false, 5), "sound/player/footsteps/stone_step2.wav", "variant wraps to 0..4");
        assert_eq!(StepSound::Metal.path(true, 1), "sound/player/footsteps/metal_run2.wav");
    }

    #[test]
    fn cg_footsteps_levels_enable_their_stages_cumulatively() {
        let stages = |level| {
            let s = FootstepStages::from_level(level);
            (s.sounds, s.effects, s.marks)
        };
        assert_eq!(stages(0), (false, false, false));
        assert_eq!(stages(1), (true, false, false));
        assert_eq!(stages(2), (true, true, false));
        assert_eq!(stages(3), (true, true, true));
        assert_eq!(stages(4), (true, true, true));
    }

    struct Floor {
        water: bool,
        ground_at: Option<f32>,
    }

    impl TraceWorld for Floor {
        fn trace(&mut self, query: TraceQuery) -> TraceResult {
            let mut result = TraceResult::clear(query.end);
            if query.mask == MASK_WATER {
                if self.water {
                    result.fraction = 0.5;
                }
            } else if let Some(z) = self.ground_at.filter(|&z| z >= query.end[2] && z <= query.start[2]) {
                result.fraction = (query.start[2] - z) / (query.start[2] - query.end[2]);
                result.end = [query.end[0], query.end[1], z];
                result.normal = [0.0, 0.0, 1.0];
                result.surface_flags = MATERIAL_SAND as i32;
            }
            result
        }

        fn point_contents(&mut self, _point: [f32; 3], _pass_entity: i32) -> i32 {
            0
        }
    }

    #[test]
    fn the_foot_traces_down_from_a_lifted_bolt() {
        let hit = trace_footstep(&mut Floor { water: false, ground_at: Some(3.0) }, [10.0, 20.0, 5.0]).unwrap();
        assert_eq!(hit, ([10.0, 20.0, 3.0], [0.0, 0.0, 1.0], MATERIAL_SAND as i32));
        // The floor is 32 below the lifted start (z 20): a foot well above it finds nothing.
        assert!(trace_footstep(&mut Floor { water: false, ground_at: Some(-40.0) }, [0.0, 0.0, 5.0]).is_none());
        assert!(trace_footstep(&mut Floor { water: false, ground_at: None }, [0.0, 0.0, 5.0]).is_none());
    }

    #[test]
    fn push_footsteps_keeps_the_feet_that_found_ground() {
        let mut out = Vec::new();
        let mut world = Floor { water: false, ground_at: Some(0.0) };
        // The left foot is over a pit, the right one on the floor.
        push_footsteps(&mut world, &mut out, 7, 90.0, &[FootstepType::Left, FootstepType::HeavyRight], |foot| {
            [0.0, 0.0, if foot == FootstepType::Left { -80.0 } else { 4.0 }]
        });
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].entity, out[0].foot, out[0].material, out[0].yaw), (7, FootstepType::HeavyRight, MATERIAL_SAND, 90.0));
        assert_eq!(out[0].position, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn machines_and_creatures_leave_no_footsteps() {
        assert!(npc_class_steps(0) && npc_class_steps(2));
        assert!(!npc_class_steps(1) && !npc_class_steps(34) && !npc_class_steps(45));
        assert!(unposed_foot([1.0, 2.0, 30.0])[2] < 30.0);
    }

    #[test]
    fn a_foot_in_water_makes_no_step() {
        assert!(trace_footstep(&mut Floor { water: true, ground_at: Some(3.0) }, [0.0, 0.0, 5.0]).is_none());
    }
}

#[cfg(test)]
mod jk2_tests {
    use super::*;
    use jka_assets::animation::{animation_index, parse_animation_cfg};

    fn animations() -> AnimationSet {
        parse_animation_cfg(
            b"BOTH_RUN1 100 30 0 20
BOTH_RUNBACK2 200 30 0 20
BOTH_WALL_RUN_RIGHT 300 30 0 20
BOTH_STAND1 400 30 0 20
",
        )
        .unwrap()
    }

    fn due(name: &str, old_frame: i32, frame: i32) -> Option<bool> {
        jk2_step_due(&animations(), animation_index(name).unwrap() as i32, old_frame, frame)
    }

    #[test]
    fn a_run_steps_at_frame_three_and_eighteen() {
        assert_eq!(due("BOTH_RUN1", 102, 103), Some(false));
        assert_eq!(due("BOTH_RUN1", 103, 104), None);
        assert_eq!(due("BOTH_RUN1", 117, 118), Some(false));
        assert_eq!(due("BOTH_RUN1", 118, 119), None);
    }

    #[test]
    fn running_backwards_steps_at_nine() {
        assert_eq!(due("BOTH_RUNBACK2", 208, 209), Some(false));
        assert_eq!(due("BOTH_RUNBACK2", 217, 218), None);
    }

    #[test]
    fn a_wrapped_loop_counts_as_a_missed_step() {
        assert_eq!(due("BOTH_RUN1", 129, 102), Some(false));
        assert_eq!(due("BOTH_RUN1", 129, 100), None, "back on the first frame is not past firstFrame+1");
    }

    #[test]
    fn a_wall_run_steps_four_times_and_needs_no_ground() {
        for (old, new) in [(301, 302), (307, 308), (311, 312), (315, 316)] {
            assert_eq!(due("BOTH_WALL_RUN_RIGHT", old, new), Some(true), "{old}->{new}");
        }
        assert_eq!(due("BOTH_WALL_RUN_RIGHT", 303, 304), None);
    }

    #[test]
    fn standing_and_a_held_frame_make_no_step() {
        assert_eq!(due("BOTH_STAND1", 402, 403), None);
        assert_eq!(due("BOTH_RUN1", 103, 103), None);
    }

    #[test]
    fn step_sounds_use_the_jk2_pools() {
        assert_eq!(jk2_step_sound(Jk2StepKind::Boot, 0), "sound/player/footsteps/boot1.wav");
        assert_eq!(jk2_step_sound(Jk2StepKind::Metal, 3), "sound/player/footsteps/clank4.wav");
        assert_eq!(jk2_step_sound(Jk2StepKind::Splash, 5), "sound/player/footsteps/splash2.wav");
    }
}

