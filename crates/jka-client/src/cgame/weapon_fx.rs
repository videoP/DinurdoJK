//! OpenJK CG_Missile (cg_ents.c), the missile/impact visuals registered by
//! CG_RegisterWeapon (cg_weaponinit.c) and fx_*.c, and the effect-playing
//! entity events: EV_MISSILE_HIT/MISS/MISS_METAL and EV_PLAY_EFFECT[_ID].
//! Vanilla branches only (JA+/JAPRO "tribes" substitutions are not ported).

use super::{
    custom_saber_rgb, event_presenter::EventDispatchResult, ClientGameState, PresentationEvent,
    PresentedEntity, SABER_BLACK,
};
use super::footsteps::{self, FootstepImpact, FootstepStages};
use super::player_presenter::PlayerFxRequest;
use super::saber_melt::SaberMelt;
use crate::{
    fx::{
        system::{EffectId, FxBlade, FxClass, FxDraw, FxFrame, FxLight, FxLightKind, FxLightSegment, FxSound, FxStats, FxSystem},
        template::Rng,
    },
    ui::{FootprintMode, SaberMarkMode},
};
use jka_assets::{
    pk3::AssetSearchPath,
    saber::{load_saber_definitions, SaberDefinition, SaberDefinitions},
    siege::SiegeClassVisual,
};
use jka_protocol::entity_event::EntityEvent;
use jka_movement::{CollisionWorld, TraceQuery, TraceWorld, ENTITY_WORLD};
use std::{
    collections::{HashMap, HashSet},
    sync::mpsc,
    thread::{self, JoinHandle},
};

const WP_SABER: i32 = 3;
const WP_BRYAR_PISTOL: i32 = 4;
const WP_BLASTER: i32 = 5;
const WP_DISRUPTOR: i32 = 6;
const WP_BOWCASTER: i32 = 7;
const WP_REPEATER: i32 = 8;
const WP_DEMP2: i32 = 9;
const WP_FLECHETTE: i32 = 10;
const WP_ROCKET_LAUNCHER: i32 = 11;
const WP_THERMAL: i32 = 12;
const WP_DET_PACK: i32 = 14;
const WP_CONCUSSION: i32 = 15;
const WP_BRYAR_OLD: i32 = 16;
const WP_EMPLACED_GUN: i32 = 17;
const WP_TURRET: i32 = 18;
const G2_MODEL_PART: i32 = 50;

const EF_ALT_FIRING: i32 = 1 << 10;
const EF_JETPACK_ACTIVE: i32 = 1 << 11;
const EF_MISSILE_STICK: i32 = 1 << 22;

const TR_STATIONARY: i32 = 0;
const TR_INTERPOLATE: i32 = 1;


const MAX_EFX_BYTES: usize = 65536;
const CONTINUOUS_FX_BACKFILL_MAX_MS: i32 = 250;
// bg_public.h ET_FX states, carried in entityState.modelindex2.
const FX_STATE_OFF: i32 = 0;
const FX_STATE_ONE_SHOT_LIMIT: i32 = 10;

// OpenJK MASK_SOLID is CONTENTS_SOLID|CONTENTS_TERRAIN. Saber contact is kept
// on the presentation collision clone so it never mutates gameplay/prediction.
const CONTENTS_SOLID: i32 = 0x0000_0001;
const CONTENTS_TERRAIN: i32 = 0x0000_1000;
const SABER_CONTACT_MASK: i32 = CONTENTS_SOLID | CONTENTS_TERRAIN;
const SURF_NOIMPACT: i32 = 0x0008_0000;
const SURF_NOMARKS: i32 = 0x0010_0000;
// OpenJK cg_marks.cpp: MARK_TOTAL_TIME=10000, MARK_FADE_TIME=1000.
// CG_CreateSaberMarks backdates the glow by 8500 ms, giving it 1500 ms total
// (500 ms full intensity + the final 1000 ms fade). Keep Legacy exact.
const SABER_MARK_LIFETIME_MS: i32 = 10_000;
const SABER_MARK_FADE_MS: i32 = 1_000;
const SABER_GLOW_LIFETIME_MS: i32 = 1_500;
const SABER_MARK_MAX: usize = 768;
/// Enhanced mode: minimum spacing of the stock contact spark burst per blade.
const ENHANCED_SPARK_INTERVAL_MS: i32 = 110;
/// Enhanced mode: minimum spacing of bubble-pop / droplet-landing sparks.
const ENHANCED_MELT_SPARK_INTERVAL_MS: i32 = 80;
const MAX_SABER_BLADES: usize = 8;

// Enhanced marks deliberately remain presentation-only. Rapier is a rigid-body
// solver, not a viscous/soft-body surface solver; using it for attached melt
// would produce detached beads/chunks rather than a coherent molten wall. If
// cooled slag later breaks free, those detached pieces can become Rapier bodies.

#[derive(Clone, Copy, Debug, Default)]
struct EntityFxState { next_time: i32, one_shot_sequence: i32 }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MissileFxGeneration {
    trajectory_time: i32,
    weapon: i32,
    custom_effect: i32,
    alt_fire: bool,
}

#[derive(Clone, Debug)]
struct MissileFxState {
    generation: MissileFxGeneration,
    last_time: i32,
    last_origin: [f32; 3],
    next_sample_time: f64,
}

enum FxWorkerCommand {
    AdjustTime(i32),
    ResetTime(i32),
    /// `fx_physics` mode (see `crate::fx::FX_PHYSICS_*`).
    SetPhysics(u32),
    /// `fx_lod` mode and `fx_countScale` (see `crate::fx::FX_LOD_*`).
    SetLod { mode: u32, count_scale: f32, lod_scale: f32 },
    /// Render view for LOD: origin and projected pixels per unit at depth 1.
    SetLodView { origin: [f32; 3], px_per_unit: f32 },
    /// Worker-owned clone of the map CM world for FX particle traces.
    SetCollision(Option<CollisionWorld>),
    PlayDir { name: String, origin: [f32; 3], dir: [f32; 3] },
    PlayDirClass { name: String, origin: [f32; 3], dir: [f32; 3], class: FxClass },
    PlayAxis { name: String, origin: [f32; 3], axis: [[f32; 3]; 3] },
    Frame { saber_impact_fx: bool, reply: mpsc::SyncSender<(FxFrame, FxStats)> },
    RefreshAssets { reply: mpsc::SyncSender<Result<usize, String>> },
    Shutdown,
}

struct FxWorker { fx: FxSystem, assets: AssetSearchPath, ids: HashMap<String, EffectId>, missing: HashSet<String> }

impl FxWorker {
    fn effect(&mut self, name: &str) -> EffectId {
        if let Some(id) = self.ids.get(name) { return *id; }
        let assets = &mut self.assets;
        let id = self.fx.register(name, &mut |path| assets.read(path, MAX_EFX_BYTES).ok().flatten().map(|a| a.bytes));
        if id == 0 && self.missing.insert(name.to_ascii_lowercase()) { println!("FX: effect {name} unavailable"); }
        self.ids.insert(name.to_owned(), id);
        id
    }
}

fn fx_worker_loop(rx: mpsc::Receiver<FxWorkerCommand>, assets: AssetSearchPath) {
    let mut worker = FxWorker { fx: FxSystem::new(), assets, ids: HashMap::new(), missing: HashSet::new() };
    while let Ok(command) = rx.recv() {
        match command {
            FxWorkerCommand::AdjustTime(time) => worker.fx.adjust_time(time),
            FxWorkerCommand::ResetTime(time) => worker.fx.reset_time(time),
            FxWorkerCommand::SetPhysics(mode) => worker.fx.set_physics_mode(mode),
            FxWorkerCommand::SetLod { mode, count_scale, lod_scale } => worker.fx.set_lod(mode, count_scale, lod_scale),
            FxWorkerCommand::SetLodView { origin, px_per_unit } => worker.fx.set_lod_view(origin, px_per_unit),
            FxWorkerCommand::SetCollision(world) => {
                worker.fx.set_collision(world.map(|world| Box::new(world) as Box<dyn TraceWorld + Send>));
            }
            FxWorkerCommand::PlayDir { name, origin, dir } => { let id = worker.effect(&name); if id != 0 { worker.fx.play_effect_dir(id, origin, dir); } }
            FxWorkerCommand::PlayDirClass { name, origin, dir, class } => { let id = worker.effect(&name); if id != 0 { worker.fx.play_effect_dir_class(id, origin, dir, class); } }
            FxWorkerCommand::PlayAxis { name, origin, axis } => { let id = worker.effect(&name); if id != 0 { worker.fx.play_effect(id, origin, axis); } }
            FxWorkerCommand::Frame { saber_impact_fx, reply } => { let frame = worker.fx.frame_with_visibility(saber_impact_fx); let _ = reply.send((frame, worker.fx.stats())); }
            FxWorkerCommand::RefreshAssets { reply } => {
                let retried = worker.missing.len();
                let result = worker
                    .assets
                    .refresh()
                    .map(|()| {
                        // Existing FX instances/templates may finish naturally.
                        // Future effect lookups re-register through the refreshed
                        // VFS, including names that previously resolved missing.
                        worker.ids.clear();
                        worker.missing.clear();
                        retried
                    })
                    .map_err(|error| format!("FX ASSET REFRESH ERROR: {error}"));
                let _ = reply.send(result);
            }
            FxWorkerCommand::Shutdown => break,
        }
    }
}

fn saber_shaders(color: i32) -> (&'static str, &'static str) {
    if custom_saber_rgb(color).is_some() {
        // jaPRO's tintable white glow/core (assets/japro sbRGB.shader); the
        // entity presenter falls back to desaturated stock images without it.
        return ("gfx/effects/sabers/RGBglow1", "gfx/effects/sabers/RGBcore1");
    }
    match color {
        0 => ("gfx/effects/sabers/red_glow", "gfx/effects/sabers/red_line"),
        1 => ("gfx/effects/sabers/orange_glow", "gfx/effects/sabers/orange_line"),
        2 => ("gfx/effects/sabers/yellow_glow", "gfx/effects/sabers/yellow_line"),
        3 => ("gfx/effects/sabers/green_glow", "gfx/effects/sabers/green_line"),
        4 => ("gfx/effects/sabers/blue_glow", "gfx/effects/sabers/blue_line"),
        5 => ("gfx/effects/sabers/purple_glow", "gfx/effects/sabers/purple_line"),
        SABER_BLACK => ("gfx/effects/sabers/blackglow", "gfx/effects/sabers/blackcore"),
        _ => ("gfx/effects/sabers/blue_glow", "gfx/effects/sabers/blue_line"),
    }
}

/// OpenJK CG_RGBForSaberColor. Keep this separate from the authored glow
/// shader lookup because the dynamic-light tint intentionally uses softened
/// channel values (for example blue = 0.2, 0.4, 1.0).
fn saber_light_rgb(color: i32) -> Option<[f32; 3]> {
    if let Some(rgb) = custom_saber_rgb(color) {
        return Some(rgb.map(|c| f32::from(c) / 255.0));
    }
    Some(match color {
        0 => [1.0, 0.2, 0.2],
        1 => [1.0, 0.5, 0.1],
        2 => [1.0, 1.0, 0.2],
        3 => [0.2, 1.0, 0.2],
        4 => [0.2, 0.4, 1.0],
        5 => [0.9, 0.2, 1.0],
        SABER_BLACK => [1.0, 1.0, 1.0],
        _ => return None,
    })
}

/// Vertex colour of a glow/halo draw: white for the stock colours (their
/// textures carry the hue), the blade RGB for jaPRO custom colours.
fn saber_glow_rgba(color: i32, intensity: u8) -> [u8; 4] {
    let rgb = custom_saber_rgb(color).unwrap_or([255; 3]);
    let scaled = rgb.map(|c| (f32::from(c) * f32::from(intensity) / 255.0).round() as u8);
    [scaled[0], scaled[1], scaled[2], 255]
}

/// CG_DoSaber's per-frame blade flicker. Returns `(glow_radius, core_radius)`.
/// `glow_rand` / `core_rand` are `Q_flrand(-1, 1)` rolls: the glow radius is
/// `radius * 0.925 +/- 7.5%`, the hot core `radius / 3 +/- 7.5% of radius`, and
/// both are widened by `1 + 2 / length` while the blade is shorter than
/// `lengthMax` (the ignition halo).
fn saber_flicker_radii(
    radius: f32,
    length: f32,
    length_max: f32,
    glow_rand: f32,
    core_rand: f32,
) -> (f32, f32) {
    let radius_range = radius * 0.075;
    let radius_mult = if length < length_max { 1.0 + 2.0 / length } else { 1.0 };
    (
        (radius - radius_range + glow_rand * radius_range) * radius_mult,
        (radius / 3.0 + core_rand * radius_range) * radius_mult,
    )
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length > 1.0e-6 {
        [v[0] / length, v[1] / length, v[2] / length]
    } else {
        [1.0, 0.0, 0.0]
    }
}

fn madd3(origin: [f32; 3], direction: [f32; 3], distance: f32) -> [f32; 3] {
    [
        origin[0] + direction[0] * distance,
        origin[1] + direction[1] * distance,
        origin[2] + direction[2] * distance,
    ]
}

/// Profile-only blade geometry using the exact authored OpenJK saber
/// glow/core shaders. This deliberately skips world contact, marks, trails and
/// dynamic-light state; those belong to live gameplay, not the isolated menu.
pub fn profile_saber_blade_draws(
    origin: [f32; 3],
    direction: [f32; 3],
    length: f32,
    length_max: f32,
    radius: f32,
    color: i32,
    entity_alpha: f32,
    modern_sabers: bool,
    rng: &mut Rng,
) -> Vec<FxDraw> {
    if length < 0.5 || radius <= 0.0 {
        return Vec::new();
    }
    // Same per-frame CG_DoSaber flicker as the in-game blade.
    let glow_rand = rng.flrand(-1.0, 1.0);
    let core_rand = rng.flrand(-1.0, 1.0);
    let (radius, core_radius) = saber_flicker_radii(radius, length, length_max, glow_rand, core_rand);
    let direction = normalize3(direction);
    let tip = madd3(origin, direction, length);
    let line_base = madd3(origin, direction, -1.0);
    let (glow_shader, line_shader) = saber_shaders(color);
    let intensity = (entity_alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    let white = [intensity, intensity, intensity, 255];
    let glow_full = saber_glow_rgba(color, intensity);
    let mut draws = Vec::new();

    if modern_sabers {
        let soft = (f32::from(intensity) * 0.55).round() as u8;
        let glow_soft = saber_glow_rgba(color, soft);
        draws.push(FxDraw::Line {
            start: line_base,
            end: tip,
            width: radius * 1.8,
            rgba: glow_soft,
            shader: glow_shader.to_owned(),
        });
        draws.push(FxDraw::Line {
            start: line_base,
            end: tip,
            width: radius * 0.95,
            rgba: glow_full,
            shader: glow_shader.to_owned(),
        });
        draws.push(FxDraw::Sprite {
            origin,
            radius: (radius * 1.5).max(5.5),
            rotation: 0.0,
            rgba: glow_soft,
            shader: glow_shader.to_owned(),
        });
        draws.push(FxDraw::Sprite {
            origin: tip,
            radius: radius * 0.9,
            rotation: 0.0,
            rgba: glow_soft,
            shader: glow_shader.to_owned(),
        });
    } else {
        // UI_DoSaber submits one RT_SABER_GLOW. Its renderer-side hilt pulse
        // is Q_flrand(0, 1) * 0.25 on top of 5.5 units.
        draws.push(FxDraw::SaberGlow {
            origin,
            direction,
            length,
            radius,
            hilt_radius: 5.5 + rng.flrand(0.0, 1.0) * 0.25,
            rgba: glow_full,
            shader: glow_shader.to_owned(),
        });
    }
    draws.push(FxDraw::Line {
        start: tip,
        end: line_base,
        width: core_radius,
        rgba: white,
        shader: line_shader.to_owned(),
    });
    draws
}

fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale3(v: [f32; 3], scale: f32) -> [f32; 3] {
    [v[0] * scale, v[1] * scale, v[2] * scale]
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn length3(v: [f32; 3]) -> f32 {
    dot3(v, v).sqrt()
}

/// Geometry of one OpenJK CG_CreateSaberMarks slash, before projection.
struct SaberMarkFrame {
    /// `originalPoints[4]` in the engine's winding: start-a1-a2, end+a1-a2,
    /// end+a1+a2, start-a1+a2 with axis1 along the slash and axis2 = axis1 x normal.
    quad: [[f32; 3]; 4],
    mid: [f32; 3],
    tangent: [f32; 3],
    side: [f32; 3],
}

fn saber_mark_frame(start: [f32; 3], end: [f32; 3], normal: [f32; 3], half_width: f32) -> Option<SaberMarkFrame> {
    let delta = sub3(end, start);
    let distance = length3(delta);
    if distance < 1.0e-4 {
        return None;
    }
    let tangent = scale3(delta, 1.0 / distance);
    let side_raw = cross3(tangent, normal);
    let side_length = length3(side_raw);
    if side_length < 1.0e-4 {
        return None;
    }
    let side = scale3(side_raw, 1.0 / side_length);
    let tangent_cap = scale3(tangent, half_width);
    let side_cap = scale3(side, half_width);
    let start_cap = sub3(start, tangent_cap);
    let end_cap = add3(end, tangent_cap);
    Some(SaberMarkFrame {
        quad: [
            sub3(start_cap, side_cap),
            sub3(end_cap, side_cap),
            add3(end_cap, side_cap),
            add3(start_cap, side_cap),
        ],
        mid: scale3(add3(start, end), 0.5),
        tangent,
        side,
    })
}

/// CG_CreateSaberMarks texture projection. The stock rivet/glow images are
/// authored around UV 0.5,0.5; mapping a slash to 0..1 made short sampled
/// segments look like isolated dots. OpenJK randomizes the scales within
/// 0.05..0.08 and 0.15..0.20; midpoints keep the same visual frequency without
/// frame-rate RNG.
fn saber_mark_uv(frame: &SaberMarkFrame, point: [f32; 3]) -> [f32; 2] {
    const U_SCALE: f32 = 0.065;
    const V_SCALE: f32 = 0.175;
    let d = sub3(point, frame.mid);
    [0.5 + dot3(d, frame.tangent) * U_SCALE, 0.5 + dot3(d, frame.side) * V_SCALE]
}

/// The small normal lift substitutes for q3 polygonOffset in the transient FX path.
const SABER_MARK_LIFT: f32 = 0.24;

/// The refEntity CG_Missile submits for a model-carrying missile.
#[derive(Clone, Debug, PartialEq)]
pub struct MissileModel {
    pub qpath: &'static str,
    pub origin: [f32; 3],
    pub axis: [[f32; 3]; 3],
}

/// weaponInfo_t missileTrailFunc effect (and its alt-fire twin).
fn trail_effect(weapon: i32, alt: bool) -> Option<&'static str> {
    Some(match (weapon, alt) {
        (WP_BRYAR_PISTOL | WP_BRYAR_OLD, _) => "bryar/shot",
        (WP_BLASTER | WP_EMPLACED_GUN, _) => "blaster/shot",
        (WP_BOWCASTER, _) => "bowcaster/shot",
        (WP_REPEATER, false) => "repeater/projectile",
        (WP_REPEATER, true) => "repeater/alt_projectile",
        (WP_DEMP2, false) => "demp2/projectile",
        (WP_FLECHETTE, false) => "flechette/shot",
        (WP_FLECHETTE, true) => "flechette/alt_shot",
        (WP_ROCKET_LAUNCHER, _) => "rocket/shot",
        (WP_CONCUSSION, _) => "concussion/shot",
        (WP_TURRET, _) => "turret/shot",
        _ => return None,
    })
}

/// weaponInfo_t missileModel / altMissileModel.
fn missile_model(weapon: i32, alt: bool) -> Option<&'static str> {
    Some(match (weapon, alt) {
        (WP_FLECHETTE, false) => "models/weapons2/golan_arms/projectileMain.md3",
        (WP_FLECHETTE, true) => "models/weapons2/golan_arms/projectile.md3",
        (WP_ROCKET_LAUNCHER, _) => "models/weapons2/merr_sonn/projectile.md3",
        (WP_THERMAL, _) => "models/weapons2/thermal/thermal_proj.md3",
        (WP_DET_PACK, _) => "models/weapons2/detpack/det_pack.md3",
        _ => return None,
    })
}

/// CG_MissileHitWall -> FX_*HitWall. The second field marks effects that
/// play along world up instead of the impact normal.
fn wall_impacts(weapon: i32, alt: bool, charge: i32) -> &'static [(&'static str, bool)] {
    match (weapon, alt) {
        (WP_BRYAR_PISTOL | WP_BRYAR_OLD, true) => match charge {
            4 | 5 => &[("bryar/wall_impact3", false)],
            2 | 3 => &[("bryar/wall_impact2", false)],
            _ => &[("bryar/wall_impact", false)],
        },
        (WP_BRYAR_PISTOL | WP_BRYAR_OLD | WP_TURRET, _) => &[("bryar/wall_impact", false)],
        (WP_CONCUSSION, _) => &[("concussion/explosion", false)],
        (WP_BLASTER | WP_EMPLACED_GUN, _) => &[("blaster/wall_impact", false)],
        (WP_DISRUPTOR, _) => &[("disruptor/alt_miss", false)],
        (WP_BOWCASTER, _) => &[("bowcaster/explosion", false)],
        (WP_REPEATER, true) => &[("repeater/concussion", false)],
        (WP_REPEATER, false) => &[("repeater/wall_impact", false)],
        (WP_DEMP2, true) => &[("demp2/altDetonate", false)],
        (WP_DEMP2, false) => &[("demp2/wall_impact", false)],
        (WP_FLECHETTE, false) => &[("flechette/wall_impact", false)],
        (WP_ROCKET_LAUNCHER, _) => &[("rocket/explosion", false)],
        (WP_THERMAL, _) => &[("thermal/explosion", false), ("thermal/shockwave", true)],
        _ => &[],
    }
}

/// CG_MissileHitPlayer -> FX_*HitPlayer (humanoid targets).
fn player_impacts(weapon: i32, alt: bool) -> &'static [(&'static str, bool)] {
    match (weapon, alt) {
        (WP_BRYAR_PISTOL | WP_BRYAR_OLD | WP_TURRET, _) => &[("bryar/flesh_impact", false)],
        (WP_CONCUSSION, _) => &[("concussion/explosion", false)],
        (WP_BLASTER | WP_EMPLACED_GUN, _) => &[("blaster/flesh_impact", false)],
        (WP_DISRUPTOR, _) => &[("disruptor/alt_hit", false)],
        (WP_BOWCASTER, _) => &[("bowcaster/explosion", false)],
        (WP_REPEATER, true) => &[("repeater/concussion", false)],
        (WP_REPEATER, false) => &[("repeater/flesh_impact", false)],
        (WP_DEMP2, true) => &[("demp2/altDetonate", false)],
        (WP_DEMP2, false) => &[("demp2/flesh_impact", false)],
        (WP_FLECHETTE, _) => &[("flechette/flesh_impact", false)],
        (WP_ROCKET_LAUNCHER, _) => &[("rocket/explosion", false)],
        (WP_THERMAL, _) => &[("thermal/explosion", false), ("thermal/shockwave", true)],
        _ => &[],
    }
}

/// EV_PLAY_EFFECT's effectTypes_t table (cgs.effects registrations).
fn play_effect_type(parm: i32) -> Option<&'static str> {
    Some(match parm {
        1 => "emplaced/dead_smoke",
        2 => "emplaced/explode",
        3 | 10 => "turret/explode",
        4 => "sparks/spark_explosion",
        5 => "tripMine/explosion",
        6 => "detpack/explosion",
        7 => "flechette/alt_blow",
        8 => "stunBaton/flesh_impact",
        9 => "demp2/altDetonate",
        11 => "sparks/spark_exp_nosnd",
        12 => "env/water_impact",
        13 => "env/acid_splash",
        14 => "env/lava_splash",
        15 => "materials/mud_large",
        16 => "materials/sand_large",
        17 => "materials/dirt_large",
        18 => "materials/snow_large",
        19 => "materials/gravel_large",
        _ => return None,
    })
}

/// cg_localents.c LE_PUFF: a sprite drifting on TR_LINEAR that fades and
/// grows over its life.
/// Stable per-blade id for the melt simulation's contact strokes.
fn melt_stroke_id(key: SaberTrailKey) -> u32 {
    (u32::from(key.entity_num) << 16) | (u32::from(key.saber_num) << 8) | u32::from(key.blade_num)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct SaberTrailKey {
    entity_num: u16,
    saber_num: u8,
    blade_num: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct SaberLightKey {
    entity_num: u16,
    saber_num: u8,
}

/// Per-frame OpenJK CG_DoSaberLight accumulator for sabers with 3+ blades.
/// Fixed storage avoids a per-frame heap allocation for staff lights.
#[derive(Clone, Copy, Debug)]
struct SaberLightAggregate {
    segments: [FxLightSegment; MAX_SABER_BLADES],
    tips: [[f32; 3]; MAX_SABER_BLADES],
    count: usize,
    first_midpoint: [f32; 3],
    first_rgb: [f32; 3],
    weighted_rgb: [f32; 3],
    total_length: f32,
    diameter: f32,
}

impl Default for SaberLightAggregate {
    fn default() -> Self {
        Self {
            segments: [FxLightSegment::default(); MAX_SABER_BLADES],
            tips: [[0.0; 3]; MAX_SABER_BLADES],
            count: 0,
            first_midpoint: [0.0; 3],
            first_rgb: [0.0; 3],
            weighted_rgb: [0.0; 3],
            total_length: 0.0,
            diameter: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct SaberTrailHistory {
    base: [f32; 3],
    tip: [f32; 3],
    last_time: i32,
}

#[derive(Clone, Debug)]
struct SaberTrailSegment {
    start_time: i32,
    end_time: i32,
    positions: [[f32; 3]; 4],
    start_uvs: [[f32; 2]; 4],
    rgba: [u8; 4],
    shader: &'static str,
}

#[derive(Clone, Copy, Debug)]
struct SaberContactHistory {
    /// Previous emitted wall-mark sample. Under the default scope this is the
    /// previous presentation-frame contact, matching OpenJK. Under the extended
    /// cg_fxFPSScope it advances only on fixed-rate samples.
    have_old_pos: bool,
    old_pos: [f32; 3],
    /// Previous presentation-frame contact used to interpolate fixed-rate
    /// samples even when cg_fxFPS is higher than the render rate.
    last_contact_pos: [f32; 3],
    last_contact_normal: [f32; 3],
    last_time: i32,
    next_fx_sample_time: f64,
    last_sound_time: i32,
    last_spark_time: i32,
}

impl Default for SaberContactHistory {
    fn default() -> Self {
        Self {
            have_old_pos: false,
            old_pos: [0.0; 3],
            last_contact_pos: [0.0; 3],
            last_contact_normal: [0.0, 1.0, 0.0],
            last_time: i32::MIN / 2,
            next_fx_sample_time: f64::NEG_INFINITY,
            last_sound_time: i32::MIN / 2,
            last_spark_time: i32::MIN / 2,
        }
    }
}

#[derive(Clone, Debug)]
struct SaberWallMark {
    start_time: i32,
    end_time: i32,
    /// Triangle-fan fragments clipped onto the drawn world surfaces (or one
    /// quad when no surface index is available), already lifted off the surface.
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
    normal: [f32; 3],
}

/// One CG_ImpactMark footprint, already projected onto the world.
#[derive(Clone, Debug)]
struct FootMark {
    end_time: i32,
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
    /// `footstep_heavy_*` (a run step) rather than `footstep_*`.
    heavy: bool,
}

/// The prints kept at once; the oldest give way.
const FOOT_MARK_MAX: usize = 512;

#[derive(Clone, Debug)]
struct Puff {
    start_time: i32,
    end_time: i32,
    base: [f32; 3],
    delta: [f32; 3],
    radius: f32,
    rotation: f32,
    color: [f32; 3],
    shader: &'static str,
}

/// A `trap->FX_AddLine` beam: width `size1 -> size2` and alpha `alpha1 -> 0`
/// linearly over `kill_time` (FX_SIZE_LINEAR | FX_ALPHA_LINEAR).
#[derive(Clone, Debug)]
pub(crate) struct BeamSpec {
    start: [f32; 3],
    end: [f32; 3],
    size1: f32,
    size2: f32,
    kill_time: i32,
    rgb: [f32; 3],
    shader: &'static str,
}

#[derive(Clone, Debug)]
struct Beam {
    spec: BeamSpec,
    start_time: i32,
}

/// One `CG_Chunks` LE_FRAGMENT before it is added to the scene.
#[derive(Clone, Debug)]
pub(crate) struct ChunkSpec {
    qpath: String,
    origin: [f32; 3],
    velocity: [f32; 3],
    angles: [f32; 3],
    angle_velocity: [f32; 3],
    life_ms: i32,
    bounce_factor: f32,
    scale: f32,
}

/// A live fragment: `pos` is TR_GRAVITY from (`base`, `trajectory_time`) until it
/// comes to rest.
#[derive(Clone, Debug)]
struct Chunk {
    spec: ChunkSpec,
    start_time: i32,
    end_time: i32,
    base: [f32; 3],
    velocity: [f32; 3],
    trajectory_time: i32,
    origin: [f32; 3],
    axis: [[f32; 3]; 3],
    last_time: i32,
    stationary: bool,
}

/// The refEntity a fragment submits this frame.
#[derive(Clone, Debug, PartialEq)]
pub struct ChunkModel {
    pub qpath: String,
    pub origin: [f32; 3],
    /// Axis rows already multiplied by the fragment's scale (ScaleModelAxis).
    pub axis: [[f32; 3]; 3],
    pub alpha: f32,
}

const DEFAULT_GRAVITY: f32 = 800.0;
const MAX_CHUNKS: usize = 96;
/// cg_localents.c SINK_TIME: stationary fragments fade over the last 2 * SINK_TIME.
const CHUNK_FADE_MS: i32 = 2000;

/// Enough randomness for debris; deterministic per event so replays match.
struct SmallRng(u32);

impl SmallRng {
    fn new(seed: u32) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B1) | 1)
    }

    fn unit(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }

    fn range(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.unit()
    }

    fn irand(&mut self, low: i32, high: i32) -> i32 {
        low + (self.unit() * (high - low + 1) as f32) as i32
    }
}

/// One glass shard from `CG_DoGlassQuad` (an FX_AddPoly with FX_APPLY_PHYSICS).
#[derive(Clone, Debug)]
pub(crate) struct ShardSpec {
    positions: [[f32; 3]; 4],
    uvs: [[f32; 2]; 4],
    velocity: [f32; 3],
    accel_z: f32,
    /// Degrees per second, pitch and yaw only.
    rotation: [f32; 2],
    bounce: f32,
    /// Ticks the shard stays put before it starts to move.
    delay_ms: i32,
}

#[derive(Clone, Debug)]
struct Shard {
    spec: ShardSpec,
    centre0: [f32; 3],
    centre: [f32; 3],
    velocity: [f32; 3],
    angles: [f32; 2],
    start_time: i32,
    last_time: i32,
    stationary: bool,
}

const SHARD_LIFE_MS: i32 = 6000;
const MAX_SHARDS: usize = 1024;

#[derive(Clone, Copy, Debug)]
struct Plum {
    start_time: i32,
    base: [f32; 3],
    score: i32,
}

#[derive(Clone, Copy, Debug)]
struct SaberClashFlareState {
    flash_time: i32,
    position: [f32; 3],
}

/// OpenJK CG_SaberClashFlare output after its visibility trace/projection.
/// Coordinates remain in the original 640x480 CGame virtual space.
#[derive(Clone, Copy, Debug)]
pub struct SaberClashFlare {
    pub rect: [f32; 4],
    pub color: [f32; 4],
}

pub struct WeaponFx {
    saber_definitions: SaberDefinitions,
    worker_tx: mpsc::Sender<FxWorkerCommand>,
    worker: Option<JoinHandle<()>>,
    cached_stats: FxStats,
    entity_fx: HashMap<u16, EntityFxState>,
    missile_fx: HashMap<u16, MissileFxState>,
    /// `0` is stock JKA: invoke continuous projectile FX every presentation
    /// frame. Non-zero values sample them at a fixed rate independent of FPS.
    continuous_fx_fps: u32,
    fx_fps_scope: u32,
    /// Mirrors the worker's `fx_physics` mode (worker starts at the default).
    fx_physics: u32,
    /// Mirrors the worker's `fx_lod` mode and `fx_countScale`.
    fx_lod: (u32, f32, f32),
    runner_rng: Rng,
    /// RT_SABER_GLOW is expanded by the renderer, not CGame. Keep its hilt
    /// pulse RNG separate so it cannot perturb CG_DoSaber's blade/core/light
    /// random sequence for this blade or the next one.
    saber_renderer_rng: Rng,
    puffs: Vec<Puff>,
    beams: Vec<Beam>,
    chunks: Vec<Chunk>,
    plums: Vec<Plum>,
    last_plum_z: f32,
    score_plums: bool,
    /// jaPRO `cg_blood` (0 none, 1 skull/brain, 2 full gibs).
    gib_level: u8,
    shards: Vec<Shard>,
    /// `(mins, maxs)` per inline model, for glass tessellation.
    inline_model_bounds: std::sync::Arc<[([f32; 3], [f32; 3])]>,
    /// Per-frame presentation primitives that do not live in the FX scheduler
    /// (notably CG_DoSaber's RT_SABER_GLOW / RT_LINE submissions).
    immediate_draws: Vec<FxDraw>,
    immediate_sounds: Vec<FxSound>,
    immediate_lights: Vec<FxLight>,
    immediate_blades: Vec<FxBlade>,
    /// OpenJK combines 3+ blade sabers into one dynamic light per saber.
    saber_multi_lights: HashMap<SaberLightKey, SaberLightAggregate>,
    modern_sabers: bool,
    rt_lighting: bool,
    /// Presentation-only A/B switch for authored EV_SABER_HIT /
    /// saber-on-saber EV_SABER_BLOCK EFX. Existing tagged primitives remain
    /// alive so a paused demo can hide/show the exact same population.
    saber_impact_fx: bool,
    saber_marks: SaberMarkMode,
    collision_world: Option<CollisionWorld>,
    /// Drawn, markable world surfaces (R_MarkFragments). `None` keeps the plain
    /// collision-plane mark, which is what source-.map previews use.
    mark_surfaces: Option<std::sync::Arc<jka_assets::bsp::MarkSurfaces>>,
    saber_contact_history: HashMap<SaberTrailKey, SaberContactHistory>,
    saber_wall_marks: Vec<SaberWallMark>,
    /// Footprints (`_PlayerFootStep`'s CG_ImpactMark) and how they are styled.
    foot_marks: Vec<FootMark>,
    footprints: FootprintMode,
    melt: SaberMelt,
    last_melt_spark_ms: i32,
    /// OpenJK cg_saberTrail: 0=off, 1=normal, 2=high-frequency/special mode.
    /// WGPU currently renders value 2 with the normal authored blur material
    /// rather than the legacy stencil-refraction path.
    saber_trail: i32,
    saber_trail_history: HashMap<SaberTrailKey, SaberTrailHistory>,
    saber_trail_segments: Vec<SaberTrailSegment>,
    /// OpenJK cg_saberFlashTime / cg_saberFlashPos. This is a 2D CGame flare,
    /// not an FX world sprite, so it is projected after the final camera is known.
    saber_clash_flare: Option<SaberClashFlareState>,
    time: i32,
    /// True when the current presentation frame runs at the same FX time as the
    /// previous one (paused demo, or >1000 fps within one millisecond). Per-frame
    /// spawns whose lifetime is measured in FX time must not repeat on such a
    /// frame or they pile up and never expire while the clock is frozen.
    repeated_time: bool,
    /// The next `begin_frame` follows a seek and is a fresh frame even though
    /// `reset_for_seek` already stored its time.
    fresh_time: bool,
    /// cg.refdef.vieworg / viewaxis[1] of the last rendered view.
    view_origin: [f32; 3],
    view_left: [f32; 3],
}

impl WeaponFx {
    pub fn new(mut assets: AssetSearchPath) -> Self {
        let saber_definitions = load_saber_definitions(&mut assets).unwrap_or_else(|error| {
            eprintln!("FX SABER DEFINITIONS UNAVAILABLE (stock impact FX only): {error}");
            SaberDefinitions::default()
        });
        let (worker_tx, rx) = mpsc::channel();
        let worker = thread::Builder::new().name("jka-fx".to_owned()).spawn(move || fx_worker_loop(rx, assets)).expect("failed to start JKA FX worker");
        Self {
            saber_definitions,
            worker_tx,
            worker: Some(worker),
            cached_stats: FxStats::default(),
            entity_fx: HashMap::new(),
            missile_fx: HashMap::new(),
            continuous_fx_fps: crate::fx::FX_FPS_DEFAULT,
            fx_fps_scope: crate::fx::FX_FPS_SCOPE_DEFAULT,
            fx_physics: crate::fx::FX_PHYSICS_DEFAULT,
            fx_lod: (crate::fx::FX_LOD_DEFAULT, 1.0, crate::fx::LOD_SCALE_DEFAULT),
            runner_rng: Rng::new(0x4658_5255_4E4E_4552),
            saber_renderer_rng: Rng::new(0x5341_4245_525F_474C),
            puffs: Vec::new(),
            beams: Vec::new(),
            chunks: Vec::new(),
            plums: Vec::new(),
            last_plum_z: 0.0,
            score_plums: true,
            gib_level: 0,
            shards: Vec::new(),
            inline_model_bounds: std::sync::Arc::from(Vec::new()),
            immediate_draws: Vec::new(),
            immediate_sounds: Vec::new(),
            immediate_lights: Vec::new(),
            immediate_blades: Vec::new(),
            saber_multi_lights: HashMap::new(),
            modern_sabers: false,
            rt_lighting: false,
            saber_impact_fx: true,
            saber_marks: SaberMarkMode::Legacy,
            collision_world: None,
            saber_contact_history: HashMap::new(),
            saber_wall_marks: Vec::new(),
            foot_marks: Vec::new(),
            footprints: FootprintMode::default(),
            mark_surfaces: None,
            melt: SaberMelt::new(),
            last_melt_spark_ms: i32::MIN / 2,
            saber_trail: 1,
            saber_trail_history: HashMap::new(),
            saber_trail_segments: Vec::new(),
            saber_clash_flare: None,
            time: 0,
            repeated_time: false,
            fresh_time: true,
            view_origin: [0.0; 3],
            view_left: [0.0, 1.0, 0.0],
        }
    }

    pub fn retry_failed_assets(&mut self) -> Result<usize, String> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.worker_tx
            .send(FxWorkerCommand::RefreshAssets { reply: reply_tx })
            .map_err(|_| "FX ASSET REFRESH ERROR: worker is unavailable".to_owned())?;
        reply_rx
            .recv()
            .map_err(|_| "FX ASSET REFRESH ERROR: worker did not reply".to_owned())?
    }

    pub fn stats(&self) -> crate::fx::system::FxStats {
        self.cached_stats
    }

    pub(crate) fn saber_definitions(&self) -> &SaberDefinitions {
        &self.saber_definitions
    }

    /// Select the optional continuous-ribbon saber presentation. The legacy
    /// path remains OpenJK-compatible and is the default.
    pub fn set_modern_sabers(&mut self, enabled: bool) {
        self.modern_sabers = enabled;
    }

    pub fn set_rt_lighting(&mut self, enabled: bool) {
        self.rt_lighting = enabled;
    }

    pub fn set_saber_impact_fx(&mut self, enabled: bool) {
        self.saber_impact_fx = enabled;
    }

    pub fn set_saber_marks(&mut self, mode: SaberMarkMode) {
        if self.saber_marks != mode {
            self.saber_marks = mode;
            self.saber_contact_history.clear();
            if !matches!(mode, SaberMarkMode::Legacy) {
                self.saber_wall_marks.clear();
            }
            if !matches!(mode, SaberMarkMode::Enhanced) {
                self.melt.clear();
            }
        }
    }

    /// Presentation-only collision clone used by the saber/world contact pass.
    pub fn set_collision_world(&mut self, world: Option<CollisionWorld>) {
        // The FX thread traces `fx_physics` particles against its own clone.
        let _ = self.worker_tx.send(FxWorkerCommand::SetCollision(world.clone()));
        self.collision_world = world;
        self.saber_contact_history.clear();
        self.saber_wall_marks.clear();
        self.melt.clear();
    }

    pub fn set_mark_surfaces(&mut self, surfaces: Option<std::sync::Arc<jka_assets::bsp::MarkSurfaces>>) {
        self.mark_surfaces = surfaces;
        self.saber_wall_marks.clear();
        self.foot_marks.clear();
    }

    pub fn set_saber_trail(&mut self, value: i32) {
        self.saber_trail = value.clamp(0, 2);
    }

    /// `fx_physics`: stock TaystJK particle physics on the CM world.
    pub fn set_fx_physics(&mut self, mode: u32) {
        let mode = mode.min(crate::fx::FX_PHYSICS_ALL);
        if self.fx_physics != mode {
            self.fx_physics = mode;
            let _ = self.worker_tx.send(FxWorkerCommand::SetPhysics(mode));
        }
    }

    /// `fx_lod` and stock `fx_countScale`: spawn-time EFX density control.
    pub fn set_fx_lod(&mut self, mode: u32, count_scale: f32, lod_scale: f32) {
        let value = (
            mode.min(crate::fx::FX_LOD_ADAPTIVE),
            count_scale.clamp(0.0, 1.0),
            lod_scale.clamp(crate::fx::LOD_SCALE_MIN, crate::fx::LOD_SCALE_MAX),
        );
        if self.fx_lod != value {
            self.fx_lod = value;
            let _ = self.worker_tx.send(FxWorkerCommand::SetLod { mode: value.0, count_scale: value.1, lod_scale: value.2 });
        }
    }

    /// Final render view for LOD (`px_per_unit` = viewport height / (2 tan(fov_y / 2))).
    pub fn set_lod_view(&mut self, origin: [f32; 3], px_per_unit: f32) {
        if self.fx_lod.0 != crate::fx::FX_LOD_OFF {
            let _ = self.worker_tx.send(FxWorkerCommand::SetLodView { origin, px_per_unit });
        }
    }

    pub fn set_continuous_fx_fps(&mut self, value: u32) {
        let value = if value == crate::fx::FX_FPS_LEGACY_JKA {
            crate::fx::FX_FPS_LEGACY_JKA
        } else {
            value.clamp(crate::fx::FX_FPS_MIN, crate::fx::FX_FPS_MAX)
        };
        if self.continuous_fx_fps != value {
            self.continuous_fx_fps = value;
            // Start the new cadence from the next presented sample instead of
            // inheriting phase from the previous rate/mode.
            self.missile_fx.clear();
            self.saber_contact_history.clear();
        }
    }

    pub fn set_fx_fps_scope(&mut self, value: u32) {
        let value = value.min(crate::fx::FX_FPS_SCOPE_FRAME_DRIVEN);
        if self.fx_fps_scope != value {
            self.fx_fps_scope = value;
            // Do not connect a newly fixed-rate stroke to history accumulated
            // under presentation-frame cadence (or vice versa).
            self.saber_contact_history.clear();
        }
    }

    /// Clear time-dependent presentation while retaining registered effect assets.
    /// Used by demo seeking so replayed events do not leak across the seek boundary.
    pub fn reset_for_seek(&mut self, time: i32) {
        let _ = self.worker_tx.send(FxWorkerCommand::ResetTime(time));
        self.entity_fx.clear();
        self.missile_fx.clear();
        self.puffs.clear();
        self.beams.clear();
        self.chunks.clear();
        self.foot_marks.clear();
        self.plums.clear();
        self.shards.clear();
        self.immediate_draws.clear();
        self.immediate_sounds.clear();
        self.immediate_lights.clear();
        self.immediate_blades.clear();
        self.saber_multi_lights.clear();
        self.saber_trail_history.clear();
        self.saber_trail_segments.clear();
        self.saber_clash_flare = None;
        self.saber_contact_history.clear();
        self.saber_wall_marks.clear();
        self.melt.clear();
        self.time = time;
        self.fresh_time = true;
    }

    /// FX_AdjustTime for this presentation frame (`cg.time`).
    pub fn begin_frame(&mut self, time: i32) {
        self.immediate_draws.clear();
        self.immediate_sounds.clear();
        self.immediate_lights.clear();
        self.immediate_blades.clear();
        self.saber_multi_lights.clear();
        if time < self.time {
            self.puffs.clear();
            self.beams.clear();
        self.chunks.clear();
            self.entity_fx.clear();
            self.missile_fx.clear();
            self.saber_trail_history.clear();
            self.saber_trail_segments.clear();
            self.saber_clash_flare = None;
            self.saber_contact_history.clear();
            self.saber_wall_marks.clear();
            self.melt.clear();
        }
        self.missile_fx.retain(|_, state| {
            time <= state.last_time.saturating_add(CONTINUOUS_FX_BACKFILL_MAX_MS * 4)
        });
        self.repeated_time = time == self.time && !self.fresh_time;
        self.fresh_time = false;
        self.time = time;
        let _ = self.worker_tx.send(FxWorkerCommand::AdjustTime(time));
    }

    /// The view used for puff drift and near-kill (previous render frame).
    pub fn set_view(&mut self, origin: [f32; 3], left: [f32; 3]) {
        self.view_origin = origin;
        self.view_left = left;
    }

    /// Port of OpenJK codemp/cgame/cg_draw.c CG_SaberClashFlare. The event
    /// owns only time/position; visibility and projection use the final CGame
    /// view so third-person/demo cameras match the stock client.
    pub fn saber_clash_flare(
        &mut self,
        view: &crate::fx::draw::FxView,
        fov_x_degrees: f32,
        fov_y_degrees: f32,
    ) -> Option<SaberClashFlare> {
        const MAX_TIME: i32 = 150;
        let flare = self.saber_clash_flare?;
        let t = self.time - flare.flash_time;
        if t <= 0 || t >= MAX_TIME {
            return None;
        }

        let dif = sub3(flare.position, view.origin);
        let z = dot3(dif, view.axis[0]);
        // OpenJK first rejects anything behind/at the camera with a slightly
        // stronger 0.2 forward-dot threshold than the projection's 0.001.
        if z < 0.2 {
            return None;
        }

        let world = self.collision_world.as_mut()?;
        let trace = world.trace(TraceQuery {
            start: view.origin,
            mins: [0.0; 3],
            maxs: [0.0; 3],
            end: flare.position,
            pass_entity: -1,
            // CG_SaberClashFlare uses CONTENTS_SOLID, not MASK_SOLID.
            mask: CONTENTS_SOLID,
        });
        if trace.fraction < 1.0 {
            return None;
        }

        let len = length3(dif);
        if len > 1200.0 {
            return None;
        }
        let mut v = (1.0 - t as f32 / MAX_TIME as f32)
            * ((1.0 - len / 800.0) * 2.0 + 0.35);
        if v < 0.001 {
            v = 0.001;
        }

        let px = (fov_x_degrees.to_radians() * 0.5).tan();
        let py = (fov_y_degrees.to_radians() * 0.5).tan();
        if px.abs() <= f32::EPSILON || py.abs() <= f32::EPSILON || z <= 0.001 {
            return None;
        }
        let x = 320.0 - dot3(dif, view.axis[1]) * 320.0 / (z * px);
        let y = 240.0 - dot3(dif, view.axis[2]) * 240.0 / (z * py);
        let size = v * 600.0;
        Some(SaberClashFlare {
            rect: [x - v * 300.0, y - v * 300.0, size, size],
            color: [0.8, 0.8, 0.8, 1.0],
        })
    }

    pub fn set_score_plums(&mut self, enabled: bool) {
        self.score_plums = enabled;
    }

    /// `CG_ScorePlum`: a floating score number (4 s) at `origin`.
    pub fn add_score_plum(&mut self, origin: [f32; 3], score: i32) {
        if !self.score_plums || self.repeated_time {
            return;
        }
        let mut base = origin;
        // Successive plums at the same height are stacked 20 units apart.
        if origin[2] >= self.last_plum_z - 20.0 && origin[2] <= self.last_plum_z + 20.0 {
            base[2] -= 20.0;
        }
        self.last_plum_z = origin[2];
        if self.plums.len() >= 16 {
            self.plums.remove(0);
        }
        self.plums.push(Plum { start_time: self.time, base, score });
    }

    /// CG_AddScorePlum for every live plum.
    fn append_score_plums(&mut self, frame: &mut FxFrame) {
        const NUMBER_SIZE: f32 = 8.0;
        const LIFE_MS: i32 = 4000;
        const NAMES: [&str; 11] = ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "minus"];
        let time = self.time;
        let view = self.view_origin;
        self.plums.retain(|plum| time - plum.start_time < LIFE_MS && time >= plum.start_time);
        for plum in &self.plums {
            let c = (LIFE_MS - (time - plum.start_time)) as f32 / LIFE_MS as f32;
            let score = plum.score;
            let rgb: [u8; 3] = if score < 0 {
                [0xff, 0x11, 0x11]
            } else if score >= 50 {
                [0xff, 0, 0xff]
            } else if score >= 20 {
                [0, 0, 0xff]
            } else if score >= 10 {
                [0xff, 0xff, 0]
            } else if score >= 2 {
                [0, 0xff, 0]
            } else {
                [0xff; 3]
            };
            let alpha = if c < 0.25 { (255.0 * 4.0 * c) as u8 } else { 255 };
            let mut origin = plum.base;
            origin[2] += 110.0 - c * 100.0;
            let dir = sub3(view, origin);
            let side = normalize3(cross3(dir, [0.0, 0.0, 1.0]));
            origin = madd3(origin, side, -10.0 + 20.0 * (c * 2.0 * std::f32::consts::PI).sin());
            if length3(sub3(origin, view)) < 20.0 {
                continue; // the view would sit inside the sprite
            }
            let mut digits: Vec<usize> = Vec::new();
            let mut value = score.unsigned_abs();
            loop {
                digits.push((value % 10) as usize);
                value /= 10;
                if value == 0 {
                    break;
                }
            }
            if score < 0 {
                digits.push(10);
            }
            let count = digits.len();
            for i in 0..count {
                let position = madd3(origin, side, (count as f32 / 2.0 - i as f32) * NUMBER_SIZE);
                frame.draws.push(FxDraw::Sprite {
                    origin: position,
                    radius: NUMBER_SIZE / 2.0,
                    rotation: 0.0,
                    rgba: [rgb[0], rgb[1], rgb[2], alpha],
                    shader: format!("gfx/2d/numbers/{}", NAMES[digits[count - 1 - i]]),
                });
            }
        }
    }

    pub fn set_gib_level(&mut self, level: u8) {
        self.gib_level = level.min(2);
    }

    fn add_chunks(&mut self, specs: &[ChunkSpec]) {
        for spec in specs {
            if self.chunks.len() >= MAX_CHUNKS {
                self.chunks.remove(0);
            }
            self.chunks.push(Chunk {
                start_time: self.time,
                end_time: self.time + spec.life_ms,
                base: spec.origin,
                velocity: spec.velocity,
                trajectory_time: self.time,
                origin: spec.origin,
                axis: scale_axis(angles_to_axis(spec.angles), spec.scale),
                last_time: self.time,
                stationary: false,
                spec: spec.clone(),
            });
        }
    }

    pub fn set_inline_model_bounds(&mut self, bounds: std::sync::Arc<[([f32; 3], [f32; 3])]>) {
        self.inline_model_bounds = bounds;
    }

    /// Glass shards are FX polys: advance them (delayed start, gravity, tumble,
    /// bounce) and append their quads to this frame's draws.
    fn append_glass_shards(&mut self, frame: &mut FxFrame) {
        let time = self.time;
        self.shards.retain(|shard| time - shard.start_time < SHARD_LIFE_MS && time >= shard.start_time);
        let mut world = self.collision_world.take();
        for shard in &mut self.shards {
            if time > shard.last_time && !shard.stationary {
                let dt = (time - shard.last_time) as f32 * 0.001;
                shard.last_time = time;
                shard.velocity[2] += shard.spec.accel_z * dt;
                let target: [f32; 3] = std::array::from_fn(|i| shard.centre[i] + shard.velocity[i] * dt);
                let trace = world.as_mut().map(|world| {
                    world.trace(TraceQuery {
                        start: shard.centre,
                        mins: [0.0; 3],
                        maxs: [0.0; 3],
                        end: target,
                        pass_entity: -1,
                        mask: CONTENTS_SOLID,
                    })
                });
                match trace.filter(|trace| trace.fraction < 1.0) {
                    None => {
                        shard.centre = target;
                        shard.angles[0] += shard.spec.rotation[0] * dt;
                        shard.angles[1] += shard.spec.rotation[1] * dt;
                    }
                    Some(trace) => {
                        let v = shard.velocity;
                        let dot = v[0] * trace.normal[0] + v[1] * trace.normal[1] + v[2] * trace.normal[2];
                        shard.velocity = std::array::from_fn(|i| (v[i] - 2.0 * dot * trace.normal[i]) * shard.spec.bounce);
                        shard.centre = trace.end;
                        let speed = (shard.velocity[0].powi(2) + shard.velocity[1].powi(2) + shard.velocity[2].powi(2)).sqrt();
                        if trace.all_solid != 0 || speed < 10.0 {
                            shard.stationary = true;
                        }
                    }
                }
            }
            // FX_ALPHA_NONLINEAR with alphaParm 85: hold 0.15 until 85% of life, then fade out.
            let life = (time - shard.start_time) as f32 / SHARD_LIFE_MS as f32;
            let alpha = if life < 0.85 { 0.15 } else { 0.15 * (1.0 - (life - 0.85) / 0.15) };
            if alpha <= 0.0 {
                continue;
            }
            let axis = angles_to_axis([shard.angles[0], shard.angles[1], 0.0]);
            let positions = shard.spec.positions.map(|point| {
                let rel: [f32; 3] = std::array::from_fn(|i| point[i] - shard.centre0[i]);
                std::array::from_fn(|i| {
                    shard.centre[i] + axis[0][i] * rel[0] + axis[1][i] * rel[1] + axis[2][i] * rel[2]
                })
            });
            frame.draws.push(FxDraw::Quad {
                positions,
                uvs: shard.spec.uvs,
                rgba: [255, 255, 255, (alpha * 255.0) as u8],
                shader: "gfx/misc/test_crackle".to_owned(),
            });
        }
        self.collision_world = world;
    }

    /// CG_AddLocalEntities for LE_FRAGMENT: advance every live chunk to this
    /// frame's time (gravity, bounce off solids, fade once at rest) and return
    /// the models to submit.
    pub fn chunk_models(&mut self) -> Vec<ChunkModel> {
        let time = self.time;
        self.chunks.retain(|chunk| time < chunk.end_time && time >= chunk.start_time);
        let mut models = Vec::with_capacity(self.chunks.len());
        let mut world = self.collision_world.take();
        for chunk in &mut self.chunks {
            if chunk.stationary {
                let remaining = chunk.end_time - time;
                let alpha = if remaining < CHUNK_FADE_MS {
                    (((remaining as f32 / CHUNK_FADE_MS as f32) * 255.0) as i32).clamp(1, 255) as f32 / 255.0
                } else {
                    1.0
                };
                models.push(ChunkModel { qpath: chunk.spec.qpath.clone(), origin: chunk.origin, axis: chunk.axis, alpha });
                continue;
            }
            let dt = (time - chunk.trajectory_time) as f32 * 0.001;
            let mut new_origin: [f32; 3] = std::array::from_fn(|i| chunk.base[i] + chunk.velocity[i] * dt);
            new_origin[2] -= 0.5 * DEFAULT_GRAVITY * dt * dt;
            let trace = world.as_mut().map(|world| {
                world.trace(TraceQuery {
                    start: chunk.origin,
                    mins: [0.0; 3],
                    maxs: [0.0; 3],
                    end: new_origin,
                    pass_entity: -1,
                    mask: CONTENTS_SOLID,
                })
            });
            let frame_ms = (time - chunk.last_time).max(0) as f32;
            match trace.filter(|trace| trace.fraction < 1.0) {
                None => {
                    chunk.origin = new_origin;
                    // LEF_TUMBLE: angles are a linear trajectory from the spawn time.
                    let elapsed = (time - chunk.start_time) as f32 * 0.001;
                    let angles = std::array::from_fn(|i| chunk.spec.angles[i] + chunk.spec.angle_velocity[i] * elapsed);
                    chunk.axis = scale_axis(angles_to_axis(angles), chunk.spec.scale);
                }
                Some(trace) if trace.start_solid == 0 => {
                    // CG_ReflectVelocity.
                    let hit_time = chunk.last_time as f32 + frame_ms * trace.fraction;
                    let mut velocity = chunk.velocity;
                    velocity[2] -= DEFAULT_GRAVITY * (hit_time - chunk.trajectory_time as f32) * 0.001;
                    let dot = velocity[0] * trace.normal[0] + velocity[1] * trace.normal[1] + velocity[2] * trace.normal[2];
                    chunk.velocity = std::array::from_fn(|i| (velocity[i] - 2.0 * dot * trace.normal[i]) * chunk.spec.bounce_factor);
                    chunk.base = trace.end;
                    chunk.trajectory_time = time;
                    if trace.all_solid != 0
                        || (trace.normal[2] > 0.0
                            && (chunk.velocity[2] < 40.0 || chunk.velocity[2] < -frame_ms * chunk.velocity[2]))
                    {
                        chunk.stationary = true;
                    }
                }
                Some(_) => {} // starts inside a solid: leave it where it is
            }
            chunk.last_time = time;
            models.push(ChunkModel { qpath: chunk.spec.qpath.clone(), origin: chunk.origin, axis: chunk.axis, alpha: 1.0 });
        }
        self.collision_world = world;
        models
    }

    /// FX_AddScheduledEffects + FX_Add, then CG_AddLocalEntities' puffs.
    pub fn end_frame(&mut self) -> FxFrame {
        let (reply, recv) = mpsc::sync_channel(1);
        let mut frame = if self.worker_tx.send(FxWorkerCommand::Frame { saber_impact_fx: self.saber_impact_fx, reply }).is_ok() {
            match recv.recv() { Ok((frame, stats)) => { self.cached_stats = stats; frame }, Err(_) => FxFrame::default() }
        } else { FxFrame::default() };
        let time = self.time;
        self.saber_trail_segments.retain(|segment| {
            if time >= segment.end_time {
                return false;
            }
            // OpenJK CTrail::Update linearly advances each vertex ST toward
            // destST (which CG_AddSaberBlade sets to start U + 1), clamping
            // U at 1.0. Preserve that texture sweep over the segment life.
            let life = (segment.end_time - segment.start_time).max(1) as f32;
            let progress = ((time - segment.start_time).max(0) as f32 / life).clamp(0.0, 1.0);
            let mut uvs = segment.start_uvs;
            for uv in &mut uvs {
                uv[0] = (uv[0] + progress).min(1.0);
            }
            frame.draws.push(FxDraw::Quad {
                positions: segment.positions,
                uvs,
                rgba: segment.rgba,
                shader: segment.shader.to_owned(),
            });
            true
        });
        // Histories only need to bridge nearby samples; discard entities that
        // have not submitted a blade for several seconds.
        self.saber_trail_history
            .retain(|_, history| time <= history.last_time.saturating_add(2500));
        self.saber_contact_history
            .retain(|_, history| time <= history.last_time.saturating_add(2500));
        self.append_saber_wall_marks(&mut frame);
        self.append_foot_marks(&mut frame);
        self.append_glass_shards(&mut frame);
        self.append_score_plums(&mut frame);
        if matches!(self.saber_marks, SaberMarkMode::Enhanced) {
            self.append_enhanced_melt(&mut frame);
        }
        self.flush_saber_multi_lights();
        frame.draws.append(&mut self.immediate_draws);
        frame.sounds.append(&mut self.immediate_sounds);
        frame.lights.append(&mut self.immediate_lights);
        frame.blades.append(&mut self.immediate_blades);
        self.beams.retain(|beam| {
            let life = beam.spec.kill_time.max(1);
            let age = time - beam.start_time;
            if age > life {
                return false;
            }
            let t = (age.max(0) as f32 / life as f32).clamp(0.0, 1.0);
            let spec = &beam.spec;
            // No FX_USE_ALPHA: alpha fades the colour and the alpha byte stays 0
            // (these shaders are GL_ONE GL_ONE with rgbGen vertex).
            let fade = 1.0 - t;
            let byte = |v: f32| (v * fade * 255.0).clamp(0.0, 255.0) as u8;
            frame.draws.push(FxDraw::Line {
                start: spec.start,
                end: spec.end,
                width: spec.size1 + (spec.size2 - spec.size1) * t,
                rgba: [byte(spec.rgb[0]), byte(spec.rgb[1]), byte(spec.rgb[2]), 0],
                shader: spec.shader.to_owned(),
            });
            true
        });
        let view = self.view_origin;
        self.puffs.retain(|puff| {
            if time >= puff.end_time {
                return false;
            }
            // CG_AddPuff.
            let c = (puff.end_time - time) as f32 / (puff.end_time - puff.start_time) as f32;
            let seconds = (time - puff.start_time) as f32 * 0.001;
            let origin = std::array::from_fn(|i| puff.base[i] + puff.delta[i] * seconds);
            let offset: [f32; 3] = std::array::from_fn(|i| origin[i] - view[i]);
            if (offset[0] * offset[0] + offset[1] * offset[1] + offset[2] * offset[2]).sqrt() < puff.radius {
                return false; // the view is inside the sprite
            }
            let rgb = puff.color.map(|value| (value * c).clamp(0.0, 255.0) as u8);
            frame.draws.push(FxDraw::Sprite {
                origin,
                radius: puff.radius * (1.0 - c) + 8.0,
                rotation: puff.rotation,
                rgba: [rgb[0], rgb[1], rgb[2], 0],
                shader: puff.shader.to_owned(),
            });
            true
        });
        frame
    }

    /// How footprints are drawn (`r_footprints`): off, or 2D prints, which the
    /// 3D mode also leaves everywhere its snow field cannot.
    pub fn set_footprint_mode(&mut self, mode: FootprintMode) {
        if self.footprints != mode {
            self.footprints = mode;
            if mode == FootprintMode::Off {
                self.foot_marks.clear();
            }
        }
    }

    /// The effect and print halves of `_PlayerFootStep` for one footfall; the
    /// sound is the caller's. `snowflow_owner` marks a player whose snow the
    /// Snowflow field deforms in 3D mode, where a flat print would hover over
    /// the dent.
    pub fn footstep(&mut self, impact: &FootstepImpact, stages: FootstepStages, snowflow_owner: bool) {
        let step = footsteps::material_step(impact.material);
        if stages.effects {
            if let Some(effect) = step.effect {
                // FX_PlayEffectID(effect, trace.endpos, trace.plane.normal)
                self.play(effect, impact.position, normalize_or_up(impact.normal));
            }
        }
        if !stages.marks || !step.mark || self.footprints == FootprintMode::Off || self.repeated_time {
            return;
        }
        let snow = impact.material & 0x1f == footsteps::MATERIAL_SNOW;
        if snow && snowflow_owner && self.footprints == FootprintMode::ThreeD {
            return;
        }
        self.push_footprint(impact);
    }

    /// CG_ImpactMark(footMarkShader, pos, normal, yaw, 1, 1, 1, 1, qfalse, 6, qfalse):
    /// a 12 unit square around the step, its `footstep_*` image turned to the
    /// legs' yaw and projected onto the drawn world surfaces below it.
    fn push_footprint(&mut self, impact: &FootstepImpact) -> bool {
        let axis0 = normalize3(impact.normal);
        if impact.normal.iter().all(|&c| c == 0.0) {
            return false;
        }
        // PerpendicularVector, then RotatePointAroundVector(axis[2], axis[0], axis[1], orientation).
        let axis2 = rotate_about3(perpendicular3(axis0), axis0, impact.yaw);
        let axis1 = cross3(axis0, axis2);
        let radius = footsteps::FOOTPRINT_RADIUS;
        let corner = |a: f32, b: f32| add3(impact.position, add3(scale3(axis1, a * radius), scale3(axis2, b * radius)));
        let quad = [corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)];
        // The `_l` shaders are the `_r` image mirrored across u (`tcMod transform -1 0 0 1 1 0`).
        let mirror = !impact.foot.right_foot();
        let texcoord_scale = 0.5 / radius;
        let uv_of = |point: [f32; 3]| {
            let delta = sub3(point, impact.position);
            let u = 0.5 + dot3(delta, axis1) * texcoord_scale;
            [if mirror { 1.0 - u } else { u }, 0.5 + dot3(delta, axis2) * texcoord_scale]
        };
        let lift = scale3(axis0, SABER_MARK_LIFT);
        let (mut positions, mut uvs, mut indices) = (Vec::new(), Vec::new(), Vec::new());
        if let Some(surfaces) = &self.mark_surfaces {
            let mut projected = jka_assets::bsp::MarkBuffer::default();
            surfaces.project(&quad, scale3(axis0, -20.0), &mut projected);
            for (fragment, _) in projected.iter() {
                let base = positions.len() as u32;
                for point in fragment {
                    uvs.push(uv_of(*point));
                    positions.push(add3(*point, lift));
                }
                for i in 1..fragment.len() as u32 - 1 {
                    indices.extend_from_slice(&[base, base + i, base + i + 1]);
                }
            }
            if positions.is_empty() {
                return false;
            }
        } else {
            for point in quad {
                uvs.push(uv_of(point));
                positions.push(add3(point, lift));
            }
            indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
        }
        self.foot_marks.push(FootMark {
            end_time: self.time + SABER_MARK_LIFETIME_MS,
            positions,
            uvs,
            indices,
            heavy: impact.foot.heavy(),
        });
        if self.foot_marks.len() > FOOT_MARK_MAX {
            let excess = self.foot_marks.len() - FOOT_MARK_MAX;
            self.foot_marks.drain(..excess);
        }
        true
    }

    /// Every live print folded into two meshes (light and heavy steps), so a
    /// crowd's tracks cost two draws. They fade over their last second the way
    /// `CG_AddMarks` does without `alphaFade`: the colour, not the alpha.
    fn append_foot_marks(&mut self, frame: &mut FxFrame) {
        #[derive(Default)]
        struct Batch {
            positions: Vec<[f32; 3]>,
            uvs: Vec<[f32; 2]>,
            rgba: Vec<[u8; 4]>,
            indices: Vec<u32>,
        }
        let time = self.time;
        let mut light = Batch::default();
        let mut heavy = Batch::default();
        self.foot_marks.retain(|mark| {
            if time >= mark.end_time {
                return false;
            }
            let remaining = mark.end_time - time;
            let fade = if remaining < SABER_MARK_FADE_MS {
                remaining as f32 / SABER_MARK_FADE_MS as f32
            } else {
                1.0
            };
            let channel = (255.0 * fade.clamp(0.0, 1.0)).round() as u8;
            let batch = if mark.heavy { &mut heavy } else { &mut light };
            let base = batch.positions.len() as u32;
            batch.positions.extend_from_slice(&mark.positions);
            batch.uvs.extend_from_slice(&mark.uvs);
            batch.rgba.resize(batch.positions.len(), [channel, channel, channel, 255]);
            batch.indices.extend(mark.indices.iter().map(|index| base + index));
            true
        });
        for (batch, shader) in [(light, "footstep_r"), (heavy, "footstep_heavy_r")] {
            if !batch.indices.is_empty() {
                frame.draws.push(FxDraw::Mesh {
                    positions: batch.positions,
                    uvs: batch.uvs,
                    rgba: batch.rgba,
                    indices: batch.indices,
                    shader: shader.to_owned(),
                });
            }
        }
    }

    /// CG_Item's `FX_PlayEffectID(cgs.effects.itemCone, origin, up)` under a
    /// placed weapon or powerup. The stock client replays the 50 ms effect every
    /// frame; outside legacy mode it is sampled at the continuous-FX rate, which
    /// keeps its additive brightness independent of the frame rate.
    pub fn item_cone(&mut self, entity_number: u16, origin: [f32; 3]) {
        if self.repeated_time {
            return;
        }
        if self.continuous_fx_fps != crate::fx::FX_FPS_LEGACY_JKA {
            let period = (1000 / self.continuous_fx_fps.max(1)).max(1) as i32;
            let runtime = self.entity_fx.entry(entity_number).or_default();
            if runtime.next_time > self.time {
                return;
            }
            runtime.next_time = self.time.saturating_add(period);
        }
        self.play("mp/itemcone", origin, [0.0, 0.0, 1.0]);
    }

    /// OpenJK CG_FX: consume the fx_runner state carried by an ET_FX snapshot.
    pub fn entity_fx(&mut self, entity: &PresentedEntity, game: &ClientGameState) {
        let state = &entity.state;
        let fx_state = state.field_i32("modelindex2").unwrap_or(FX_STATE_OFF);
        if fx_state == FX_STATE_OFF { return; }
        {
            let runtime = self.entity_fx.entry(entity.number).or_default();
            if runtime.next_time > self.time { return; }
            if fx_state < FX_STATE_ONE_SHOT_LIMIT {
                if runtime.one_shot_sequence == fx_state { return; }
                runtime.one_shot_sequence = fx_state;
            }
        }
        let delay = state.field_f32("speed").unwrap_or(0.0).max(0.0) as i32;
        let random = state.field_i32("time").unwrap_or(0).max(0);
        let jitter = (self.runner_rng.flrand(0.0, 1.0) * random as f32) as i32;
        self.entity_fx.entry(entity.number).or_default().next_time = self.time.saturating_add(delay).saturating_add(jitter);
        let effect_index = state.field_i32("modelindex").unwrap_or(0);
        let Some(name) = game.effect_qpath(effect_index) else { return; };
        let mut dir = angles_to_axis(super::entity_vec3(state, "angles").unwrap_or([0.0; 3]))[0];
        if dir == [0.0; 3] { dir[1] = 1.0; }
        let _ = self.play(&name, entity.origin, dir);
    }

    /// OpenJK CG_DoSaber / CG_DoSaberLight dynamic-light submission. Sabers
    /// with fewer than three blades emit one light per visible blade; staffs
    /// and other 3+ blade sabers accumulate into one light for the whole saber.
    fn saber_dynamic_light(
        &mut self,
        key: SaberTrailKey,
        origin: [f32; 3],
        direction: [f32; 3],
        visible_length: f32,
        authored_length: f32,
        color: i32,
        num_blades: u8,
        no_dlight: bool,
    ) {
        // CG_DoSaber disables the per-blade light for black sabers. TaystJK's
        // separate 3+ blade CG_DoSaberLight path still includes black as white
        // when it builds the combined multiblade light, so keep that distinction.
        if no_dlight || (num_blades < 3 && color == SABER_BLACK) {
            return;
        }

        let direction = normalize3(direction);
        if num_blades < 3 {
            if visible_length < 0.5 {
                return;
            }
            // CG_DoSaber initializes rgb to white before CG_RGBForSaberColor.
            let rgb = saber_light_rgb(color).unwrap_or([1.0; 3]);
            // OpenJK CG_DoSaber: midpoint of the actually rendered/clipped
            // blade, radius = length * 1.4 + Q_flrand(0, 1) * 3.
            let midpoint = madd3(origin, direction, visible_length * 0.5);
            let radius = visible_length * 1.4 + self.runner_rng.flrand(0.0, 1.0) * 3.0;
            self.immediate_lights.push(FxLight {
                kind: FxLightKind::Saber,
                origin: midpoint, radius, rgb,
                segment: Some([origin, madd3(origin, direction, visible_length)]),
                blade_segments: None,
            });
            return;
        }

        // OpenJK CG_DoSaberLight reads saberInfo_t blade lengths/bolts rather
        // than CG_AddSaberBlade's wall-clipped local saberLen. Mirror that for
        // 3+ blade sabers and combine them once at end_frame.
        let length = authored_length.max(0.0);
        if length < 0.5 {
            return;
        }
        // CG_DoSaberLight's rgbs array is zero-initialized before the helper,
        // so an unknown color remains black there (unlike CG_DoSaber above).
        let rgb = saber_light_rgb(color).unwrap_or([0.0; 3]);
        let tip = madd3(origin, direction, length);
        let light_key = SaberLightKey { entity_num: key.entity_num, saber_num: key.saber_num };
        let aggregate = self.saber_multi_lights.entry(light_key).or_default();
        if aggregate.count >= MAX_SABER_BLADES {
            return;
        }
        if aggregate.count == 0 {
            aggregate.first_midpoint = madd3(origin, direction, length * 0.5);
            aggregate.first_rgb = rgb;
        }
        aggregate.tips[aggregate.count] = tip;
        if self.rt_lighting {
            aggregate.segments[aggregate.count] = FxLightSegment {
                endpoints: [origin, madd3(origin, direction, visible_length.max(0.0))],
                rgb,
                weight: length,
            };
        }
        aggregate.count += 1;
        aggregate.total_length += length;
        for channel in 0..3 {
            aggregate.weighted_rgb[channel] += rgb[channel] * length;
        }
        aggregate.diameter = aggregate.diameter.max(length * 2.0);
    }

    fn flush_saber_multi_lights(&mut self) {
        // Drain rather than replace the map so staff-heavy scenes retain the
        // small allocation across frames instead of churning the heap.
        let lights = &mut self.immediate_lights;
        let rng = &mut self.runner_rng;
        for (_, aggregate) in self.saber_multi_lights.drain() {
            if aggregate.count == 0 || aggregate.total_length <= 0.0 {
                continue;
            }

            let (origin, rgb) = if aggregate.count == 1 {
                (aggregate.first_midpoint, aggregate.first_rgb)
            } else {
                let mut midpoint = [0.0; 3];
                for tip in &aggregate.tips[..aggregate.count] {
                    midpoint = add3(midpoint, *tip);
                }
                let inv_count = 1.0 / aggregate.count as f32;
                midpoint = midpoint.map(|component| component * inv_count);
                let inv_length = 1.0 / aggregate.total_length;
                let rgb = aggregate.weighted_rgb.map(|component| component * inv_length);
                (midpoint, rgb)
            };

            // OpenJK starts with the longest blade diameter, then expands to
            // the farthest pair of blade tips.
            let mut diameter = aggregate.diameter;
            for i in 0..aggregate.count {
                for j in 0..aggregate.count {
                    diameter = diameter.max(length3(sub3(aggregate.tips[i], aggregate.tips[j])));
                }
            }
            let radius = diameter + rng.flrand(0.0, 1.0) * 8.0;
            let blade_segments = self.rt_lighting.then(|| {
                aggregate.segments[..aggregate.count].iter().copied().map(|mut blade| {
                    blade.weight /= aggregate.total_length;
                    // Clipped/hidden blades emit no RT light; do not redistribute
                    // their energy to the remaining blades.
                    if length3(sub3(blade.endpoints[1], blade.endpoints[0])) < 0.5 {
                        blade.weight = 0.0;
                    }
                    blade
                }).collect::<Vec<_>>().into()
            });
            lights.push(FxLight { kind: FxLightKind::Saber, origin, radius, rgb, segment: None, blade_segments });
        }
    }

    /// Force-power visuals requested by CG_Player.
    pub fn player_fx(&mut self, request: &PlayerFxRequest) {
        match request {
            PlayerFxRequest::Effect { name, origin, axis } => {
                let _ = self.worker_tx.send(FxWorkerCommand::PlayAxis { name: (*name).to_owned(), origin: *origin, axis: *axis });
            }
            PlayerFxRequest::EffectDir { name, origin, dir } => {
                let _ = self.worker_tx.send(FxWorkerCommand::PlayDir { name: name.clone(), origin: *origin, dir: *dir });
            }
            PlayerFxRequest::SaberBlade {
                origin,
                direction,
                length,
                length_max,
                radius,
                color,
                entity_alpha,
                entity_num,
                saber_num,
                blade_num,
                saber_move,
                torso_anim,
                saber_in_flight,
                trail_style,
                num_blades,
                no_dlight,
                no_wall_marks,
            } => {
                let key = SaberTrailKey {
                    entity_num: *entity_num,
                    saber_num: *saber_num,
                    blade_num: *blade_num,
                };
                let clipped_length = self.saber_world_contact(
                    key,
                    *origin,
                    *direction,
                    *length,
                    *no_wall_marks,
                );
                self.saber_dynamic_light(
                    key,
                    *origin,
                    *direction,
                    clipped_length,
                    *length,
                    *color,
                    *num_blades,
                    *no_dlight,
                );
                self.saber_trail(
                    key,
                    *origin,
                    *direction,
                    clipped_length,
                    *color,
                    *entity_alpha,
                    *saber_move,
                    *torso_anim,
                    *saber_in_flight,
                    *trail_style,
                );
                self.saber_blade(
                    *origin,
                    *direction,
                    clipped_length,
                    *length_max,
                    *radius,
                    *color,
                    *entity_alpha,
                );
            }
            PlayerFxRequest::PushPuffs { origin } => {
                // CG_ForcePushBlur (LE_PUFF path).
                self.puff(*origin, 55.0, 0.0, [24.0, 32.0, 40.0], "gfx/effects/forcePush");
                self.puff(*origin, -55.0, 180.0, [24.0, 32.0, 40.0], "gfx/effects/forcePush");
            }
            PlayerFxRequest::HeadSprite { origin, shader } => {
                // CG_PlayerFloatSprite: RT_SPRITE, radius 10, opaque white.
                self.immediate_draws.push(FxDraw::Sprite {
                    origin: *origin,
                    radius: 10.0,
                    rotation: 0.0,
                    rgba: [255; 4],
                    shader: (*shader).to_owned(),
                });
            }
            PlayerFxRequest::GripPuffs { origin } => {
                // CG_ForceGripEffect.
                let wv = (self.time as f32 * 0.004).sin() * 0.08 + 0.1;
                self.puff(*origin, 55.0, 0.0, [(200.0 + wv * 255.0).min(255.0), 0.0, 0.0], "gfx/effects/forcePush");
                self.puff(*origin, -55.0, 180.0, [255.0; 3], "gfx/effects/sabers/red_glow");
            }
        }
    }

    fn finish_melt_stroke(&mut self, key: SaberTrailKey) {
        self.melt.end_stroke(melt_stroke_id(key));
    }

    fn saber_world_contact(
        &mut self,
        key: SaberTrailKey,
        origin: [f32; 3],
        direction: [f32; 3],
        length: f32,
        no_wall_marks: bool,
    ) -> f32 {
        if matches!(self.saber_marks, SaberMarkMode::Off) || length < 0.5 {
            self.saber_contact_history.remove(&key);
            self.finish_melt_stroke(key);
            return length;
        }
        let direction = normalize3(direction);
        let end = madd3(origin, direction, length + 1.0);
        let Some(world) = self.collision_world.as_mut() else {
            self.finish_melt_stroke(key);
            return length;
        };
        let hit = world.trace(TraceQuery {
            start: origin,
            mins: [0.0; 3],
            maxs: [0.0; 3],
            end,
            pass_entity: i32::from(key.entity_num),
            mask: SABER_CONTACT_MASK,
        });
        if hit.fraction >= 1.0 {
            if let Some(history) = self.saber_contact_history.get_mut(&key) {
                history.have_old_pos = false;
                history.last_time = self.time;
                history.next_fx_sample_time = f64::NEG_INFINITY;
            }
            self.finish_melt_stroke(key);
            return length;
        }

        let clipped_length = (length * hit.fraction).max(0.1);
        let hit_normal = normalize3(hit.normal);
        let can_impact = hit.surface_flags & SURF_NOIMPACT == 0;
        let can_mark = can_impact
            && hit.surface_flags & SURF_NOMARKS == 0
            && hit.entity == ENTITY_WORLD
            && !no_wall_marks;
        let previous = self.saber_contact_history.get(&key).copied().unwrap_or_default();
        let mut next = previous;
        next.last_time = self.time;

        // Extended scope: preserve OpenJK's geometry/contact semantics, but
        // sample the presentation-frame-driven legacy saber sparks/marks at the
        // same fixed cadence as continuous EFX. Interpolate between consecutive
        // render-frame contacts so 90/120/250 Hz remains spatially continuous
        // even when the renderer itself is presenting more slowly. Authored EFX
        // timing is not rewritten.
        let fixed_legacy_contact = self.fx_fps_scope == crate::fx::FX_FPS_SCOPE_FRAME_DRIVEN
            && self.continuous_fx_fps != crate::fx::FX_FPS_LEGACY_JKA
            && matches!(self.saber_marks, SaberMarkMode::Legacy);
        if fixed_legacy_contact {
            let now = f64::from(self.time);
            let period = 1000.0 / f64::from(self.continuous_fx_fps.max(1));
            let continuous = previous.last_time > i32::MIN / 4
                && self.time >= previous.last_time
                && self.time.saturating_sub(previous.last_time) <= CONTINUOUS_FX_BACKFILL_MAX_MS;

            if !continuous || !previous.next_fx_sample_time.is_finite() {
                // First contact arms the sample history immediately, just like
                // OpenJK arms trail.haveOldPos on its first touching frame.
                if !no_wall_marks && can_impact {
                    self.play("sparks/spark_nosnd", hit.end, hit_normal);
                    next.last_spark_time = self.time;
                }
                next.have_old_pos = can_mark;
                next.old_pos = hit.end;
                next.next_fx_sample_time = now + period;
            } else {
                let frame_dt = f64::from((self.time - previous.last_time).max(1));
                let mut sample_time = previous.next_fx_sample_time;
                let mut emitted = 0usize;
                while sample_time <= now + f64::EPSILON && emitted < 64 {
                    let t = ((sample_time - f64::from(previous.last_time)) / frame_dt)
                        .clamp(0.0, 1.0) as f32;
                    let sample_pos = [
                        previous.last_contact_pos[0] + (hit.end[0] - previous.last_contact_pos[0]) * t,
                        previous.last_contact_pos[1] + (hit.end[1] - previous.last_contact_pos[1]) * t,
                        previous.last_contact_pos[2] + (hit.end[2] - previous.last_contact_pos[2]) * t,
                    ];
                    let sample_normal = normalize3([
                        previous.last_contact_normal[0] + (hit_normal[0] - previous.last_contact_normal[0]) * t,
                        previous.last_contact_normal[1] + (hit_normal[1] - previous.last_contact_normal[1]) * t,
                        previous.last_contact_normal[2] + (hit_normal[2] - previous.last_contact_normal[2]) * t,
                    ]);
                    if !no_wall_marks && can_impact {
                        self.play("sparks/spark_nosnd", sample_pos, sample_normal);
                        next.last_spark_time = self.time;
                    }
                    if can_mark && next.have_old_pos
                        && length3(sub3(sample_pos, next.old_pos)) > 1.0e-4
                    {
                        self.push_legacy_saber_wall_mark(next.old_pos, sample_pos, sample_normal);
                    }
                    if can_mark {
                        next.old_pos = sample_pos;
                        next.have_old_pos = true;
                    } else {
                        next.have_old_pos = false;
                    }
                    sample_time += period;
                    emitted += 1;
                }
                // A hitch should not cause an unbounded burst. If the safety
                // cap was reached, resume from the first future sample.
                if sample_time <= now {
                    let skipped = ((now - sample_time) / period).floor() + 1.0;
                    sample_time += skipped * period;
                }
                next.next_fx_sample_time = sample_time;

                // Audio is not an EFX density artifact. Keep OpenJK's 100 ms
                // wall-hit debounce independent of the visual sampling rate.
                if can_mark && previous.have_old_pos
                    && self.time.saturating_sub(previous.last_sound_time) >= 100
                {
                    let variant = self.runner_rng.irand(1, 3);
                    self.immediate_sounds.push(FxSound {
                        origin: hit.end,
                        qpath: format!("sound/weapons/saber/saberhitwall{variant}.wav"),
                    });
                    next.last_sound_time = self.time;
                }
            }
            next.last_contact_pos = hit.end;
            next.last_contact_normal = hit_normal;
            if !can_mark {
                next.have_old_pos = false;
            }
            self.saber_contact_history.insert(key, next);
            return clipped_length;
        }

        // OpenJK performs saber/world contact once per presentation frame. The
        // cg_fxFPS knob is for scheduled/continuous .efx emitters, not marks.
        // Legacy keeps the stock burst every frame. Enhanced lets the melt carry
        // the scene (glow, bubbling, dripping) and only fires an occasional spark.
        if !no_wall_marks && can_impact {
            let enhanced = matches!(self.saber_marks, SaberMarkMode::Enhanced);
            if !enhanced || self.time.saturating_sub(previous.last_spark_time) >= ENHANCED_SPARK_INTERVAL_MS {
                self.play("sparks/spark_nosnd", hit.end, hit_normal);
                next.last_spark_time = self.time;
            }
        }

        match self.saber_marks {
            SaberMarkMode::Legacy => {
                self.finish_melt_stroke(key);
                if can_mark && previous.have_old_pos {
                    if length3(sub3(hit.end, previous.old_pos)) > 1.0e-4 {
                        self.push_legacy_saber_wall_mark(previous.old_pos, hit.end, hit_normal);
                    }
                    // OpenJK uses blade.hitWallDebounceTime and allows another
                    // wall-hit sound every 100 ms while continuous contact lasts.
                    if self.time.saturating_sub(previous.last_sound_time) >= 100 {
                        let variant = self.runner_rng.irand(1, 3);
                        self.immediate_sounds.push(FxSound {
                            origin: hit.end,
                            qpath: format!("sound/weapons/saber/saberhitwall{variant}.wav"),
                        });
                        next.last_sound_time = self.time;
                    }
                }
            }
            SaberMarkMode::Enhanced => {
                // The melt only builds on surfaces the stock mark projector would
                // also mark (not nomarks/noimpact/fog, not compiled-in model soup).
                let markable = self
                    .mark_surfaces
                    .as_ref()
                    .is_none_or(|surfaces| surfaces.has_markable_surface(hit.end, hit_normal));
                if can_mark && markable {
                    let heat = self.melt.contact(
                        melt_stroke_id(key),
                        hit.end,
                        hit_normal,
                        self.time,
                        self.collision_world.as_mut(),
                    );
                    if previous.have_old_pos && self.time.saturating_sub(previous.last_sound_time) >= 100 {
                        let variant = self.runner_rng.irand(1, 3);
                        self.immediate_sounds.push(FxSound {
                            origin: hit.end,
                            qpath: format!("sound/weapons/saber/saberhitwall{variant}.wav"),
                        });
                        next.last_sound_time = self.time;
                    }
                    let flicker = 0.94 + (self.time as f32 * 0.029).sin() * 0.06;
                    let heat01 = (heat / 1.25).clamp(0.0, 1.0);
                    self.immediate_lights.push(FxLight {
                        kind: FxLightKind::SaberMark,
                        origin: madd3(hit.end, hit_normal, 1.1),
                        segment: None,
                        blade_segments: None,
                        radius: 48.0 + 72.0 * heat01,
                        rgb: [1.0 * flicker, (0.24 + 0.24 * heat01) * flicker, (0.025 + 0.055 * heat01) * flicker],
                    });
                } else {
                    self.finish_melt_stroke(key);
                }
            }
            SaberMarkMode::Off => {}
        }

        next.have_old_pos = can_mark;
        next.old_pos = hit.end;
        next.last_contact_pos = hit.end;
        next.last_contact_normal = hit_normal;
        self.saber_contact_history.insert(key, next);
        clipped_length
    }

    /// OpenJK CG_CreateSaberMarks. The slash quad is projected onto the drawn world
    /// surfaces by R_MarkFragments: nothing is produced on nomarks/noimpact/fog
    /// shaders, on compiled-in model triangle soup, or where no world surface is
    /// drawn (a hidden clip brush, an entity model's hull).
    fn push_legacy_saber_wall_mark(&mut self, start: [f32; 3], end: [f32; 3], normal: [f32; 3]) -> bool {
        let Some(frame) = saber_mark_frame(start, end, normal, 0.65) else {
            return false;
        };
        let lift = scale3(normal, SABER_MARK_LIFT);
        let mut positions = Vec::new();
        let mut uvs = Vec::new();
        let mut indices = Vec::new();
        if let Some(surfaces) = &self.mark_surfaces {
            let mut projected = jka_assets::bsp::MarkBuffer::default();
            surfaces.project(&frame.quad, scale3(normal, -1.0), &mut projected);
            for (fragment, _) in projected.iter() {
                let base = positions.len() as u32;
                for point in fragment {
                    uvs.push(saber_mark_uv(&frame, *point));
                    positions.push(add3(*point, lift));
                }
                for i in 1..fragment.len() as u32 - 1 {
                    indices.extend_from_slice(&[base, base + i, base + i + 1]);
                }
            }
            if positions.is_empty() {
                return false;
            }
        } else {
            for point in frame.quad {
                uvs.push(saber_mark_uv(&frame, point));
                positions.push(add3(point, lift));
            }
            indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
        }
        self.saber_wall_marks.push(SaberWallMark {
            start_time: self.time,
            end_time: self.time + SABER_MARK_LIFETIME_MS,
            positions,
            uvs,
            indices,
            normal,
        });
        if self.saber_wall_marks.len() > SABER_MARK_MAX {
            let excess = self.saber_wall_marks.len() - SABER_MARK_MAX;
            self.saber_wall_marks.drain(..excess);
        }
        true
    }

    /// Every live mark is folded into two meshes (burn, then glow) with per-vertex
    /// fade, so a long saber fight costs two draws however many marks it left.
    fn append_saber_wall_marks(&mut self, frame: &mut FxFrame) {
        #[derive(Default)]
        struct Batch {
            positions: Vec<[f32; 3]>,
            uvs: Vec<[f32; 2]>,
            rgba: Vec<[u8; 4]>,
            indices: Vec<u32>,
        }
        impl Batch {
            fn push(&mut self, positions: impl Iterator<Item = [f32; 3]>, mark: &SaberWallMark, rgba: [u8; 4]) {
                let base = self.positions.len() as u32;
                self.positions.extend(positions);
                self.uvs.extend_from_slice(&mark.uvs);
                self.rgba.resize(self.positions.len(), rgba);
                self.indices.extend(mark.indices.iter().map(|index| base + index));
            }
        }
        let time = self.time;
        let mut burn_batch = Batch::default();
        let mut glow_batch = Batch::default();
        self.saber_wall_marks.retain(|mark| {
            if time >= mark.end_time {
                return false;
            }
            let remaining = mark.end_time - time;
            let burn_fade = if remaining < SABER_MARK_FADE_MS {
                remaining as f32 / SABER_MARK_FADE_MS as f32
            } else {
                1.0
            };
            let burn = (255.0 * burn_fade.clamp(0.0, 1.0)).round() as u8;
            burn_batch.push(mark.positions.iter().copied(), mark, [burn, burn, burn, burn]);

            let age = (time - mark.start_time).max(0);
            if age < SABER_GLOW_LIFETIME_MS {
                let fade = if age <= 500 {
                    1.0
                } else {
                    1.0 - (age - 500) as f32 / (SABER_GLOW_LIFETIME_MS - 500) as f32
                };
                let lift = scale3(mark.normal, 0.035);
                glow_batch.push(
                    mark.positions.iter().map(|point| add3(*point, lift)),
                    mark,
                    [(235.0 * fade.max(0.0)) as u8, (118.0 * fade.max(0.0)) as u8, (12.0 * fade.max(0.0)) as u8, 255],
                );
            }
            true
        });
        for (batch, shader) in [(burn_batch, "gfx/damage/rivetmark"), (glow_batch, "gfx/effects/saberDamageGlow")] {
            if !batch.indices.is_empty() {
                frame.draws.push(FxDraw::Mesh {
                    positions: batch.positions,
                    uvs: batch.uvs,
                    rgba: batch.rgba,
                    indices: batch.indices,
                    shader: shader.to_owned(),
                });
            }
        }
    }

    fn append_enhanced_melt(&mut self, frame: &mut FxFrame) {
        let time = self.time;
        let sparks = self.melt.update(time, self.collision_world.as_mut());
        for spark in sparks {
            if time.saturating_sub(self.last_melt_spark_ms) < ENHANCED_MELT_SPARK_INTERVAL_MS {
                break;
            }
            self.last_melt_spark_ms = time;
            self.play("sparks/spark_nosnd", spark.origin, spark.normal);
        }
        self.melt.emit(time, self.view_origin, &mut frame.draws, &mut self.immediate_lights);
    }

    #[allow(clippy::too_many_arguments)]
    fn saber_trail(
        &mut self,
        key: SaberTrailKey,
        origin: [f32; 3],
        direction: [f32; 3],
        length: f32,
        color: i32,
        entity_alpha: f32,
        saber_move: i32,
        torso_anim: i32,
        saber_in_flight: bool,
        trail_style: i32,
    ) {
        if self.saber_trail == 0 || trail_style > 1 || self.repeated_time {
            return;
        }
        // A blade that is off (holstered staff/dual blade, or still ramping up)
        // is CG_AddSaberBlade's dontDraw case: no quad, but base/tip/lastTime
        // are still stored every sample. Skipping that leaves the position from
        // when the blade went out, and relighting it mid-swing would stretch a
        // trail quad from there to the current blade.
        let blade_off = length < 0.5;

        let direction = normalize3(direction);
        let tip = madd3(origin, direction, length + 3.0);
        let now = self.time;
        let previous = self.saber_trail_history.get(&key).copied();
        let sample_due = previous
            .map(|history| self.saber_trail == 2 || now > history.last_time.saturating_add(2))
            .unwrap_or(true);
        if !sample_due {
            return;
        }

        if let Some(history) = previous.filter(|_| !blade_off) {
            let move_trail_length = jka_movement::saber_move_trail_length(saber_move);
            let super_break = jka_movement::super_break_win_anim(torso_anim);
            let active = super_break || move_trail_length > 0 || saber_in_flight;
            let fresh = now < history.last_time.saturating_add(2000);
            let diff = now.saturating_sub(history.last_time);
            if active && fresh && diff <= 10_000 {
                // OpenJK: duration = saberMoveData[].trailLength / 5, with a
                // 150 ms super-break fallback and SABER_TRAIL_TIME (40 ms)
                // otherwise. trailStyle 1 uses swordTrail for twice as long.
                let mut duration = if move_trail_length > 0 {
                    move_trail_length as f32 / 5.0
                } else if super_break {
                    150.0
                } else {
                    40.0
                };
                let (shader, rgb) = if trail_style == 1 {
                    duration *= 2.0;
                    ("gfx/effects/sabers/swordTrail", [32u8, 32, 32])
                } else if color == SABER_BLACK {
                    ("gfx/effects/sabers/blacktrail", saber_trail_rgb(color))
                } else {
                    ("gfx/effects/sabers/saberBlur", saber_trail_rgb(color))
                };
                let duration = duration.max(1.0);
                // Exact CG_AddSaberBlade winding/UV progression: new base,
                // new tip, old tip, old base. clampmap handles U beyond 1.
                let old_u = diff as f32 / duration;
                let intensity = entity_alpha.clamp(0.0, 1.0);
                let rgb = rgb.map(|c| (f32::from(c) * intensity).round().clamp(0.0, 255.0) as u8);
                self.saber_trail_segments.push(SaberTrailSegment {
                    start_time: now,
                    end_time: now.saturating_add(duration.round() as i32),
                    positions: [origin, tip, history.tip, history.base],
                    start_uvs: [[0.0, 1.0], [0.0, 0.0], [old_u, 0.0], [old_u, 1.0]],
                    rgba: [rgb[0], rgb[1], rgb[2], 255],
                    shader,
                });
            }
        }

        // OpenJK stores the newest base/tip even when the current move does
        // not draw a trail, so the next attack connects from a fresh sample.
        self.saber_trail_history.insert(
            key,
            SaberTrailHistory {
                base: origin,
                tip,
                last_time: now,
            },
        );
    }

    fn saber_blade(
        &mut self,
        origin: [f32; 3],
        direction: [f32; 3],
        length: f32,
        length_max: f32,
        radius: f32,
        color: i32,
        entity_alpha: f32,
    ) {
        if length < 0.5 || radius <= 0.0 {
            return;
        }
        // Rolled per frame like CG_DoSaber, so the glow and core shimmer. The
        // lighting-side blade keeps the steady authored radius.
        let glow_rand = self.runner_rng.flrand(-1.0, 1.0);
        let core_rand = self.runner_rng.flrand(-1.0, 1.0);
        let (flicker_radius, core_radius) =
            saber_flicker_radii(radius, length, length_max, glow_rand, core_rand);
        let steady_radius = radius;
        let radius = flicker_radius;
        let direction = normalize3(direction);
        let tip = madd3(origin, direction, length);
        let line_base = madd3(origin, direction, -1.0);
        let (glow_shader, line_shader) = saber_shaders(color);
        // GL_ONE GL_ONE has no source-alpha weighting. RF_FORCE_ENT_ALPHA-like
        // fades therefore scale vertex RGB, which is what rgbGen vertex uses.
        let intensity = (entity_alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
        let white = [intensity, intensity, intensity, 255];
        let glow_full = saber_glow_rgba(color, intensity);
        if intensity >= 8 {
            self.immediate_blades.push(FxBlade { start: line_base, end: tip, radius: steady_radius });
        }

        if self.modern_sabers {
            // Continuous camera-facing capsule: smoother than the legacy bead
            // chain while still using the stock authored saber materials.
            let soft = (f32::from(intensity) * 0.55).round() as u8;
            let glow_soft = saber_glow_rgba(color, soft);
            self.immediate_draws.push(FxDraw::Line {
                start: line_base,
                end: tip,
                width: radius * 1.8,
                rgba: glow_soft,
                shader: glow_shader.to_owned(),
            });
            self.immediate_draws.push(FxDraw::Line {
                start: line_base,
                end: tip,
                width: radius * 0.95,
                rgba: glow_full,
                shader: glow_shader.to_owned(),
            });
            self.immediate_draws.push(FxDraw::Sprite {
                origin,
                radius: (radius * 1.5).max(5.5),
                rotation: 0.0,
                rgba: glow_soft,
                shader: glow_shader.to_owned(),
            });
            self.immediate_draws.push(FxDraw::Sprite {
                origin: tip,
                radius: radius * 0.9,
                rotation: 0.0,
                rgba: glow_soft,
                shader: glow_shader.to_owned(),
            });
        } else {
            // CG_DoSaber submits exactly one RT_SABER_GLOW. The renderer owns
            // the bead-chain expansion and its independent 5.5..5.75 hilt
            // pulse, matching TaystJK RB_SurfaceSaberGlow instead of flattening
            // the entity into generic CGame sprites here.
            let hilt_radius = 5.5 + self.saber_renderer_rng.flrand(0.0, 1.0) * 0.25;
            self.immediate_draws.push(FxDraw::SaberGlow {
                origin,
                direction,
                length,
                radius,
                hilt_radius,
                rgba: glow_full,
                shader: glow_shader.to_owned(),
            });
        }

        // OpenJK CG_DoSaber uses an RT_LINE from blade tip to one unit behind
        // the hilt origin, at radius / 3 (flickered), with the color-specific
        // line shader.
        self.immediate_draws.push(FxDraw::Line {
            start: tip,
            end: line_base,
            width: core_radius,
            rgba: white,
            shader: line_shader.to_owned(),
        });
    }

    fn puff(&mut self, origin: [f32; 3], speed: f32, rotation: f32, color: [f32; 3], shader: &'static str) {
        // Puffs live 120 ms of FX time. A frame at an unchanged time (paused
        // demo) would add another copy every frame that can never expire.
        if self.repeated_time {
            return;
        }
        self.puffs.push(Puff {
            start_time: self.time,
            end_time: self.time + 120,
            base: origin,
            delta: self.view_left.map(|v| v * speed),
            radius: 2.0,
            rotation,
            color,
            shader,
        });
    }

    fn play(&mut self, name: &str, origin: [f32; 3], dir: [f32; 3]) -> bool {
        self.worker_tx
            .send(FxWorkerCommand::PlayDir { name: name.to_owned(), origin, dir })
            .is_ok()
    }

    fn play_saber_impact(&mut self, name: &str, origin: [f32; 3], dir: [f32; 3]) -> bool {
        self.worker_tx
            .send(FxWorkerCommand::PlayDirClass {
                name: name.to_owned(),
                origin,
                dir,
                class: FxClass::SaberImpact,
            })
            .is_ok()
    }

    fn missile_fx_samples(
        &mut self,
        entity_number: u16,
        generation: MissileFxGeneration,
        time: i32,
        origin: [f32; 3],
    ) -> Vec<[f32; 3]> {
        if self.continuous_fx_fps == crate::fx::FX_FPS_LEGACY_JKA {
            return vec![origin];
        }

        let period = 1000.0 / f64::from(self.continuous_fx_fps.max(1));
        use std::collections::hash_map::Entry;
        let state = match self.missile_fx.entry(entity_number) {
            Entry::Vacant(entry) => {
                entry.insert(MissileFxState {
                    generation,
                    last_time: time,
                    last_origin: origin,
                    next_sample_time: f64::from(time) + period,
                });
                return vec![origin];
            }
            Entry::Occupied(entry) => entry.into_mut(),
        };

        let elapsed = time.saturating_sub(state.last_time);
        if state.generation != generation
            || time < state.last_time
            || elapsed > CONTINUOUS_FX_BACKFILL_MAX_MS
        {
            *state = MissileFxState {
                generation,
                last_time: time,
                last_origin: origin,
                next_sample_time: f64::from(time) + period,
            };
            return vec![origin];
        }

        if time == state.last_time {
            state.last_origin = origin;
            return Vec::new();
        }

        let start_time = f64::from(state.last_time);
        let end_time = f64::from(time);
        let duration = end_time - start_time;
        let previous_origin = state.last_origin;
        let mut samples = Vec::new();
        while state.next_sample_time <= end_time + 1.0e-6 {
            let alpha = ((state.next_sample_time - start_time) / duration).clamp(0.0, 1.0) as f32;
            let sample: [f32; 3] = std::array::from_fn(|axis| {
                previous_origin[axis] + (origin[axis] - previous_origin[axis]) * alpha
            });
            samples.push(sample);
            state.next_sample_time += period;
        }
        state.last_time = time;
        state.last_origin = origin;
        samples
    }

    /// CG_Missile: play the weapon's trail effect for this frame and return
    /// the missile model refEntity, if the weapon has one.
    pub fn missile(&mut self, entity: &PresentedEntity, game: &ClientGameState, time: i32) -> Option<MissileModel> {
        let state = &entity.state;
        let weapon = state.field_i32("weapon").unwrap_or(0);
        if weapon == WP_SABER || weapon == G2_MODEL_PART {
            return None; // thrown sabers / G2 parts: owned by the player presenter
        }
        let e_flags = state.field_i32("eFlags").unwrap_or(0);
        let alt = e_flags & EF_ALT_FIRING != 0;
        let delta = super::entity_vec3(state, "pos.trDelta").unwrap_or([0.0; 3]);
        let forward = normalize_or_up(delta);

        let other2 = state.field_i32("otherEntityNum2").unwrap_or(0);
        let generation = MissileFxGeneration {
            trajectory_time: state.field_i32("pos.trTime").unwrap_or(0),
            weapon,
            custom_effect: other2,
            alt_fire: alt,
        };
        if other2 != 0 {
            // An over-ridden trail effect (cgs.gameEffects = CS_EFFECTS).
            if e_flags & EF_JETPACK_ACTIVE == 0 {
                if let Some(name) = game.effect_qpath(other2) {
                    let samples =
                        self.missile_fx_samples(entity.number, generation, time, entity.origin);
                    for origin in samples {
                        self.play(&name, origin, forward);
                    }
                }
            }
            return None;
        }

        if let Some(name) = trail_effect(weapon, alt) {
            let samples = self.missile_fx_samples(entity.number, generation, time, entity.origin);
            for origin in samples {
                if alt && (weapon == WP_BRYAR_PISTOL || weapon == WP_BRYAR_OLD) {
                    // FX_BryarAltProjectileThink: one crackle per charge level.
                    for _ in 1..state.field_i32("generic1").unwrap_or(0) {
                        self.play("bryar/crackleShot", origin, forward);
                    }
                }
                self.play(name, origin, forward);
            }
        }

        let qpath = missile_model(weapon, alt)?;
        let axis = if state.field_i32("apos.trType").unwrap_or(0) != TR_INTERPOLATE {
            let stick = e_flags & EF_MISSILE_STICK != 0;
            let yaw = if state.field_i32("pos.trType").unwrap_or(0) != TR_STATIONARY {
                time as f32 * if stick { 0.5 } else { 0.25 }
            } else if stick {
                state.field_i32("pos.trTime").unwrap_or(0) as f32 * 0.5
            } else {
                state.field_i32("time").unwrap_or(0) as f32
            };
            jka_movement::rotate_around_direction(forward, yaw)
        } else {
            // lerpAngles is a copy of s1->angles for missiles.
            angles_to_axis(super::entity_vec3(state, "angles").unwrap_or([0.0; 3]))
        };
        Some(MissileModel { qpath, origin: entity.origin, axis })
    }

    /// The FX part of CG_EntityEvent. The semantic decode is deliberately
    /// split from side effects so the optional event-worker path can prepare
    /// events off-thread while preserving the exact same ordered application.
    #[cfg(test)]
    pub fn entity_event(
        &mut self,
        event: &PresentationEvent,
        game: &ClientGameState,
        siege_classes: &[SiegeClassVisual],
    ) -> Option<EventDispatchResult> {
        let prepared = prepare_entity_event(
            event,
            game,
            siege_classes,
            &self.saber_definitions,
        );
        self.entity_event_prepared(event, &prepared)
    }

    pub(crate) fn entity_event_prepared(
        &mut self,
        event: &PresentationEvent,
        prepared: &PreparedFxEvent,
    ) -> Option<EventDispatchResult> {
        match prepared {
            PreparedFxEvent::None => None,
            PreparedFxEvent::SaberHit { effect, count, origin, dir } => {
                let mut played = false;
                for _ in 0..*count {
                    played |= self.play_saber_impact(effect, *origin, *dir);
                }
                Some(if played {
                    EventDispatchResult::Handled("FX_SABER_HIT_OPENJK")
                } else {
                    EventDispatchResult::Partial("FX_SABER_HIT_MISSING")
                })
            }
            PreparedFxEvent::SaberBlock {
                effect,
                origin,
                dir,
                clash_flare,
            } => {
                if *clash_flare {
                    self.saber_clash_flare = Some(SaberClashFlareState {
                        flash_time: self.time - 50,
                        position: *origin,
                    });
                }
                let played = self.play_saber_impact(effect, *origin, *dir);
                Some(if played {
                    EventDispatchResult::Handled("FX_SABER_BLOCK_OPENJK")
                } else {
                    EventDispatchResult::Partial("FX_SABER_BLOCK_MISSING")
                })
            }
            PreparedFxEvent::SaberClashFlare { origin } => {
                self.saber_clash_flare = Some(SaberClashFlareState {
                    flash_time: self.time - 50,
                    position: *origin,
                });
                Some(EventDispatchResult::Partial("FX_SABER_CLASH_FLARE_OPENJK"))
            }
            PreparedFxEvent::MissileImpact {
                dir,
                custom_requested,
                custom_effect,
                vehicle_pending,
                weapon,
                alt,
                charge,
            } => {
                if *custom_requested {
                    let played = custom_effect
                        .as_deref()
                        .is_some_and(|name| self.play(name, event.position, *dir));
                    return Some(if played { EventDispatchResult::Handled("FX_CUSTOM_IMPACT") } else { EventDispatchResult::Partial("FX_CUSTOM_IMPACT_MISSING") });
                }
                if *vehicle_pending {
                    return Some(EventDispatchResult::Partial("FX_VEHICLE_WEAPON_IMPACT_PENDING"));
                }
                let effects = if event.event == EntityEvent::EV_MISSILE_HIT {
                    player_impacts(*weapon, *alt)
                } else {
                    wall_impacts(*weapon, *alt, *charge)
                };
                let mut played = false;
                for &(name, up) in effects {
                    played |= self.play(name, event.position, if up { [0.0, 0.0, 1.0] } else { *dir });
                }
                Some(if played { EventDispatchResult::Handled("FX_MISSILE_IMPACT") } else { EventDispatchResult::Partial("FX_MISSILE_IMPACT_NONE") })
            }
            PreparedFxEvent::GlassBreak { model } => {
                if self.repeated_time {
                    return Some(EventDispatchResult::Handled("FX_GLASS_SHARDS"));
                }
                let specs = glass_shards(event, *model, &self.inline_model_bounds);
                for spec in specs {
                    if self.shards.len() >= MAX_SHARDS {
                        self.shards.remove(0);
                    }
                    let centre = std::array::from_fn(|axis| spec.positions.iter().map(|p| p[axis]).sum::<f32>() * 0.25);
                    self.shards.push(Shard {
                        centre0: centre,
                        centre,
                        velocity: spec.velocity,
                        angles: [0.0; 2],
                        start_time: self.time,
                        last_time: self.time + spec.delay_ms,
                        stationary: false,
                        spec,
                    });
                }
                Some(EventDispatchResult::Handled("FX_GLASS_SHARDS"))
            }
            PreparedFxEvent::Gibs { origin, seed } => {
                let specs = gib_specs(*origin, *seed, self.gib_level);
                if specs.is_empty() || self.repeated_time {
                    return Some(EventDispatchResult::Handled("FX_GIBS"));
                }
                self.add_chunks(&specs);
                Some(EventDispatchResult::Handled("FX_GIBS"))
            }
            PreparedFxEvent::Chunks(specs) => {
                if self.repeated_time {
                    return Some(EventDispatchResult::Handled("FX_CHUNKS"));
                }
                for spec in specs {
                    if self.chunks.len() >= MAX_CHUNKS {
                        self.chunks.remove(0);
                    }
                    self.chunks.push(Chunk {
                        start_time: self.time,
                        end_time: self.time + spec.life_ms,
                        base: spec.origin,
                        velocity: spec.velocity,
                        trajectory_time: self.time,
                        origin: spec.origin,
                        axis: scale_axis(angles_to_axis(spec.angles), spec.scale),
                        last_time: self.time,
                        stationary: false,
                        spec: spec.clone(),
                    });
                }
                Some(EventDispatchResult::Handled("FX_CHUNKS"))
            }
            PreparedFxEvent::PlayEffects { effects, beams, status } => {
                let mut played = false;
                for (name, origin, dir) in effects {
                    played |= self.play(name, *origin, *dir);
                }
                if !self.repeated_time {
                    for spec in beams {
                        self.beams.push(Beam { spec: spec.clone(), start_time: self.time });
                        played = true;
                    }
                }
                Some(if played { EventDispatchResult::Handled(status) } else { EventDispatchResult::Partial("FX_EFFECT_MISSING") })
            }
            PreparedFxEvent::GroundEffect { name, entity, position } => {
                let Some(world) = self.collision_world.as_mut() else {
                    return Some(EventDispatchResult::Partial("FX_NO_COLLISION_WORLD"));
                };
                let mut end = *position;
                end[2] -= 4096.0;
                let trace = world.trace(TraceQuery {
                    start: *position,
                    mins: PLAYER_HULL_MINS,
                    maxs: PLAYER_HULL_MAXS,
                    end,
                    pass_entity: *entity,
                    mask: SABER_CONTACT_MASK, // MASK_SOLID
                });
                if trace.fraction >= 1.0 {
                    return Some(EventDispatchResult::Partial("FX_GROUND_EFFECT_NO_FLOOR"));
                }
                let played = self.play(name, trace.end, [0.0, 0.0, 1.0]);
                Some(if played { EventDispatchResult::Handled("FX_GROUND_EFFECT") } else { EventDispatchResult::Partial("FX_EFFECT_MISSING") })
            }
            PreparedFxEvent::PlayEffect { name, origin, dir } => {
                let Some(name) = name else {
                    return Some(EventDispatchResult::Partial("FX_PLAY_EFFECT_TYPE_UNKNOWN"));
                };
                Some(if self.play(name, *origin, *dir) { EventDispatchResult::Handled("FX_PLAY_EFFECT") } else { EventDispatchResult::Partial("FX_EFFECT_MISSING") })
            }
            PreparedFxEvent::PlayEffectId { name, origin, dir } => {
                let Some(name) = name.as_deref() else {
                    return Some(EventDispatchResult::Partial("EFFECT_RESOURCE_MISSING"));
                };
                // Portal effects belong to the sky-portal scene, which is not
                // rendered separately; they play in the main scene.
                Some(if self.play(name, *origin, *dir) { EventDispatchResult::Handled("FX_PLAY_EFFECT_ID") } else { EventDispatchResult::Partial("FX_EFFECT_MISSING") })
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum PreparedFxEvent {
    None,
    SaberHit {
        effect: String,
        count: usize,
        origin: [f32; 3],
        dir: [f32; 3],
    },
    SaberBlock {
        effect: String,
        origin: [f32; 3],
        dir: [f32; 3],
        clash_flare: bool,
    },
    SaberClashFlare {
        origin: [f32; 3],
    },
    MissileImpact {
        dir: [f32; 3],
        custom_requested: bool,
        custom_effect: Option<String>,
        vehicle_pending: bool,
        weapon: i32,
        alt: bool,
        charge: i32,
    },
    PlayEffect {
        name: Option<&'static str>,
        origin: [f32; 3],
        dir: [f32; 3],
    },
    PlayEffectId {
        name: Option<String>,
        origin: [f32; 3],
        dir: [f32; 3],
    },
    /// `CG_Chunks` fragments (EV_DEBRIS).
    Chunks(Vec<ChunkSpec>),
    /// `CG_GibPlayer` at `origin`; the gib count depends on `cg_blood` at dispatch.
    Gibs { origin: [f32; 3], seed: u32 },
    /// `CG_GlassShatter` (EV_GLASS_SHATTER). `model` is the glass entity's inline
    /// model when its state is still known; shards are built on dispatch, where
    /// the map's inline model bounds live.
    GlassBreak { model: Option<usize> },
    /// A batch of effects for one event (concussion alt-fire rings + impact).
    PlayEffects {
        effects: Vec<(String, [f32; 3], [f32; 3])>,
        beams: Vec<BeamSpec>,
        status: &'static str,
    },
    /// An effect placed where a hull dropped from `position` lands (teleport
    /// and Jedi Master spawn effects: `CG_Trace(position -> position - 4096)`).
    GroundEffect {
        name: &'static str,
        entity: i32,
        position: [f32; 3],
    },
}

const WHITE: [f32; 3] = [1.0; 3];
const PLAYER_HULL_MINS: [f32; 3] = [-15.0, -15.0, -24.0 + 8.0]; // DEFAULT_MINS_2 + 8
const PLAYER_HULL_MAXS: [f32; 3] = [15.0, 15.0, 40.0]; // DEFAULT_MAXS_2

fn impact_saber_definition(
    saber_definitions: &SaberDefinitions,
    event: &PresentationEvent,
    game: &ClientGameState,
    siege_classes: &[SiegeClassVisual],
) -> Option<(SaberDefinition, bool)> {
    let owner = event.state.field_i32("otherEntityNum2")?;
    if !(0..1023).contains(&owner) {
        return None;
    }
    let owner = u16::try_from(owner).ok()?;
    let info = game
        .entity_state(owner)
        .and_then(|state| {
            if state.field_i32("eType").unwrap_or(0) == super::ET_NPC {
                game.npc_client_info(state).ok()
            } else {
                game.client_info(usize::from(owner), siege_classes)
            }
        })
        .or_else(|| game.client_info(usize::from(owner), siege_classes))?;
    let saber_num = event.state.field_i32("weapon").unwrap_or(0);
    let saber_name = match saber_num {
        0 => &info.saber_name,
        1 => &info.saber2_name,
        _ => return None,
    };
    let definition = saber_definitions.get(saber_name)?.clone();
    let blade_num = event.state.field_i32("legsAnim").unwrap_or(0).max(0) as usize;
    let second_style = definition.blade_style2_start > 0
        && blade_num >= definition.blade_style2_start;
    Some((definition, second_style))
}

fn saber_event_direction(event: &PresentationEvent) -> [f32; 3] {
    let mut dir = super::entity_vec3(&event.state, "angles").unwrap_or([0.0; 3]);
    if dir == [0.0; 3] {
        dir[1] = 1.0;
    }
    dir
}

fn saber_event_origin(event: &PresentationEvent) -> [f32; 3] {
    super::entity_vec3(&event.state, "origin").unwrap_or(event.position)
}

pub(crate) fn prepare_entity_event(
    event: &PresentationEvent,
    game: &ClientGameState,
    siege_classes: &[SiegeClassVisual],
    saber_definitions: &SaberDefinitions,
) -> PreparedFxEvent {
    let state = &event.state;
    match event.event {
        EntityEvent::EV_SABER_HIT => {
            // OpenJK codemp/cgame/cg_event.c EV_SABER_HIT.
            let mut hit_person = "saber/blood_sparks_mp.efx".to_owned();
            let mut hit_person_small = "saber/blood_sparks_25_mp.efx".to_owned();
            let mut hit_person_mid = "saber/blood_sparks_50_mp.efx".to_owned();
            let mut hit_other = "saber/saber_cut.efx".to_owned();
            if let Some((definition, second_style)) =
                impact_saber_definition(saber_definitions, event, game, siege_classes)
            {
                let person = if second_style {
                    definition.hit_person_effect2.as_ref()
                } else {
                    definition.hit_person_effect.as_ref()
                };
                if let Some(effect) = person {
                    hit_person.clone_from(effect);
                    hit_person_small.clone_from(effect);
                    hit_person_mid.clone_from(effect);
                }
                if let Some(effect) = if second_style {
                    definition.hit_other_effect2.as_ref()
                } else {
                    definition.hit_other_effect.as_ref()
                } {
                    hit_other.clone_from(effect);
                }
            }
            let origin = saber_event_origin(event);
            let dir = saber_event_direction(event);
            let (effect, count) = match event.parm {
                16 => (hit_person, 6),
                3 => (hit_person_small, 1),
                2 => (hit_person_mid, 1),
                value if value != 0 => (hit_person, 3),
                _ => (hit_other, 1),
            };
            PreparedFxEvent::SaberHit { effect, count, origin, dir }
        }
        EntityEvent::EV_SABER_BLOCK => {
            // OpenJK: eventParm != 0 is saber-on-saber; zero is a projectile deflect.
            let origin = saber_event_origin(event);
            let saber = (event.parm != 0)
                .then(|| impact_saber_definition(saber_definitions, event, game, siege_classes))
                .flatten();
            let mut effect = if event.parm != 0 {
                "saber/saber_block.efx".to_owned()
            } else {
                "blaster/deflect.efx".to_owned()
            };
            if event.parm != 0 {
                if let Some((definition, second_style)) = saber.as_ref() {
                    if let Some(custom) = if *second_style {
                        definition.block_effect2.as_ref()
                    } else {
                        definition.block_effect.as_ref()
                    } {
                        effect.clone_from(custom);
                    }
                }
            }
            // OpenJK MP checks the selected saber's primary-style
            // SFL2_NO_CLASH_FLARE flag even when bladeStyle2 is active.
            let clash_flare = event.parm != 0
                && !saber
                    .as_ref()
                    .is_some_and(|(definition, _)| definition.no_clash_flare);
            PreparedFxEvent::SaberBlock {
                effect,
                origin,
                dir: saber_event_direction(event),
                clash_flare,
            }
        }
        EntityEvent::EV_SABER_CLASHFLARE => PreparedFxEvent::SaberClashFlare {
            origin: saber_event_origin(event),
        },
        EntityEvent::EV_MISSILE_HIT | EntityEvent::EV_MISSILE_MISS | EntityEvent::EV_MISSILE_MISS_METAL => {
            let dir = jka_movement::byte_to_dir(event.parm);
            let custom = state.field_i32("emplacedOwner").unwrap_or(0);
            let custom_requested = custom != 0;
            let custom_effect = if custom_requested { game.effect_qpath(custom) } else { None };
            let flags = state.field_i32("eFlags").unwrap_or(0);
            let vehicle_pending = flags & EF_JETPACK_ACTIVE != 0
                && state.field_i32("otherEntityNum2").unwrap_or(0) != 0;
            let weapon = state.field_i32("weapon").unwrap_or(0);
            let alt = flags & EF_ALT_FIRING != 0;
            let charge = if alt { state.field_i32("generic1").unwrap_or(0) } else { 0 };
            PreparedFxEvent::MissileImpact {
                dir,
                custom_requested,
                custom_effect,
                vehicle_pending,
                weapon,
                alt,
                charge,
            }
        }
        EntityEvent::EV_PLAYER_TELEPORT_IN | EntityEvent::EV_PLAYER_TELEPORT_OUT => {
            if super::sound_presenter::duel_hides(game, state.field_i32("clientNum").unwrap_or(-1)) {
                return PreparedFxEvent::None;
            }
            PreparedFxEvent::GroundEffect {
                name: "mp/spawn.efx",
                entity: i32::from(event.entity_num),
                position: event.position,
            }
        }
        EntityEvent::EV_BECOME_JEDIMASTER => PreparedFxEvent::GroundEffect {
            name: "mp/jedispawn.efx",
            entity: i32::from(event.entity_num),
            position: event.position,
        },
        EntityEvent::EV_CONC_ALT_IMPACT => {
            // Rings every 64 units along the shot, then the wall impact and the
            // borrowed disruptor alt-miss burst. (The beam trail itself needs
            // FX_ConcAltShot's line primitive and is not drawn.)
            let origin2 = super::entity_vec3(state, "origin2").unwrap_or([0.0; 3]);
            let mut shot_dir = super::entity_vec3(state, "angles").unwrap_or([0.0; 3]);
            let shot_dist = length3(shot_dir);
            if shot_dist > 0.0 {
                shot_dir = shot_dir.map(|v| v / shot_dist);
            }
            let ring_dir = super::entity_vec3(state, "angles2").unwrap_or([0.0; 3]);
            let dir = jka_movement::byte_to_dir(event.parm);
            let mut effects = Vec::new();
            let mut travelled = 0.0;
            // FX_ConcAltShot ends at the last ring `spot`, not at the impact.
            let mut last_spot = origin2;
            while travelled < shot_dist && effects.len() < 256 {
                let spot = std::array::from_fn(|i| origin2[i] + shot_dir[i] * travelled);
                effects.push(("concussion/alt_ring".to_owned(), spot, ring_dir));
                last_spot = spot;
                travelled += 64.0;
            }
            effects.push(("concussion/explosion".to_owned(), event.position, dir));
            effects.push(("disruptor/alt_miss".to_owned(), event.position, dir));
            const BRIGHT: [f32; 3] = [0.75, 0.5, 1.0];
            let beams = vec![
                BeamSpec { start: origin2, end: last_spot, size1: 0.1, size2: 10.0, kill_time: 175, rgb: WHITE, shader: "gfx/effects/blueLine" },
                BeamSpec { start: origin2, end: last_spot, size1: 0.1, size2: 7.0, kill_time: 150, rgb: BRIGHT, shader: "gfx/misc/whiteline2" },
            ];
            PreparedFxEvent::PlayEffects { effects, beams, status: "FX_CONC_ALT_IMPACT" }
        }
        EntityEvent::EV_DISRUPTOR_MAIN_SHOT | EntityEvent::EV_DISRUPTOR_SNIPER_SHOT => {
            // The reference starts the beam at the shooter's weapon muzzle bolt
            // (first person: last frame's flash point); the server's origin2 is
            // the muzzle it fired from, used here for every viewpoint.
            let start = super::entity_vec3(state, "origin2").unwrap_or([0.0; 3]);
            let end = event.position;
            let beams = if event.event == EntityEvent::EV_DISRUPTOR_MAIN_SHOT {
                // FX_DisruptorMainShot (cg_disruptorMainTime default 150).
                vec![BeamSpec { start, end, size1: 0.1, size2: 6.0, kill_time: 150, rgb: WHITE, shader: "gfx/effects/redLine" }]
            } else {
                // FX_DisruptorAltShot (cg_disruptorAltTime default 175); shouldtarget = full charge.
                let mut beams = vec![BeamSpec { start, end, size1: 0.1, size2: 10.0, kill_time: 175, rgb: WHITE, shader: "gfx/effects/redLine" }];
                if state.field_i32("shouldtarget").unwrap_or(0) != 0 {
                    beams.push(BeamSpec { start, end, size1: 0.1, size2: 7.0, kill_time: 175, rgb: [0.8, 0.7, 0.0], shader: "gfx/misc/whiteline2" });
                }
                beams
            };
            PreparedFxEvent::PlayEffects { effects: Vec::new(), beams, status: "FX_DISRUPTOR_BEAM" }
        }
        EntityEvent::EV_DISRUPTOR_SNIPER_MISS | EntityEvent::EV_DISRUPTOR_HIT => {
            let dir = jka_movement::byte_to_dir(event.parm);
            let weapon = state.field_i32("weapon").unwrap_or(0) != 0;
            let name = match (event.event, weapon) {
                (EntityEvent::EV_DISRUPTOR_SNIPER_MISS, true) | (EntityEvent::EV_DISRUPTOR_HIT, false) => "disruptor/wall_impact",
                (EntityEvent::EV_DISRUPTOR_SNIPER_MISS, false) => "disruptor/alt_miss",
                _ => "disruptor/flesh_impact",
            };
            PreparedFxEvent::PlayEffects {
                effects: vec![(name.to_owned(), event.position, dir)],
                beams: Vec::new(),
                status: "FX_DISRUPTOR_IMPACT",
            }
        }
        EntityEvent::EV_DEBRIS => prepare_chunks(event, game),
        EntityEvent::EV_GIB_PLAYER => PreparedFxEvent::Gibs {
            origin: event.position,
            seed: (event.receive_sequence as u32) ^ (event.server_time as u32).rotate_left(5),
        },
        EntityEvent::EV_GLASS_SHATTER => {
            let entity = state.field_i32("genericenemyindex").unwrap_or(-1);
            let model = u16::try_from(entity)
                .ok()
                .and_then(|number| game.entity_state(number))
                .and_then(|glass| usize::try_from(glass.field_i32("modelindex").unwrap_or(0)).ok());
            PreparedFxEvent::GlassBreak { model }
        }
        EntityEvent::EV_MISC_MODEL_EXP => {
            // CG_MiscModelExplosion: effects scattered through the breakable's bbox.
            let maxs = super::entity_vec3(state, "origin2").unwrap_or([0.0; 3]);
            let mins = super::entity_vec3(state, "angles2").unwrap_or([0.0; 3]);
            let size = state.field_i32("time").unwrap_or(0);
            let chunk_type = event.parm;
            // (effect, optional second effect, base chunk count)
            let (effect, effect2, base): (&str, Option<&str>, i32) = match chunk_type {
                1 => ("chunks/glassbreak", None, 5),
                6 => ("chunks/glassbreak", Some("chunks/metalexplode"), 5),
                2 | 3 => ("chunks/sparkexplode", None, 5),
                0 | 7 | 10 | 11 | 14 => ("chunks/metalexplode", None, 2),
                12 => ("chunks/grateexplode", None, 8),
                13 => ("chunks/ropebreak", None, 20),
                15 | 4 | 5 | 9 | 16 => (if size == 2 { "chunks/rockbreaklg" } else { "chunks/rockbreakmed" }, None, 13),
                _ => return PreparedFxEvent::None,
            };
            let count = (base + 7 * size).clamp(0, 128);
            let mut rng = (event.receive_sequence as u32).wrapping_mul(0x9E37_79B1) | 1;
            let mut next = move || {
                rng ^= rng << 13;
                rng ^= rng >> 17;
                rng ^= rng << 5;
                (rng >> 8) as f32 / (1u32 << 24) as f32
            };
            let mid: [f32; 3] = std::array::from_fn(|i| (mins[i] + maxs[i]) * 0.5);
            let mut effects = Vec::with_capacity(count as usize);
            for _ in 0..count {
                let org: [f32; 3] = std::array::from_fn(|i| {
                    let r = next() * 0.8 + 0.1;
                    r * mins[i] + (1.0 - r) * maxs[i]
                });
                let dir = normalize_or_up([org[0] - mid[0], org[1] - mid[1], org[2] - mid[2]]);
                let name = match effect2 {
                    Some(second) if next() >= 0.5 => second,
                    _ => effect,
                };
                effects.push((name.to_owned(), org, dir));
            }
            PreparedFxEvent::PlayEffects { effects, beams: Vec::new(), status: "FX_MISC_MODEL_EXPLOSION" }
        }
        EntityEvent::EV_PLAY_EFFECT => {
            let mut dir = super::entity_vec3(state, "angles").unwrap_or([0.0; 3]);
            if dir == [0.0; 3] { dir[1] = 1.0; }
            let origin = super::entity_vec3(state, "origin").unwrap_or(event.position);
            PreparedFxEvent::PlayEffect { name: play_effect_type(event.parm), origin, dir }
        }
        EntityEvent::EV_PLAY_EFFECT_ID | EntityEvent::EV_PLAY_PORTAL_EFFECT_ID => {
            let mut dir = angles_to_axis(super::entity_vec3(state, "angles").unwrap_or([0.0; 3]))[0];
            if dir == [0.0; 3] { dir[1] = 1.0; }
            PreparedFxEvent::PlayEffectId {
                name: game.effect_qpath(event.parm),
                origin: event.position,
                dir,
            }
        }
        _ => PreparedFxEvent::None,
    }
}

/// `CG_GibPlayer`: skull or brain at level 1, plus nine body parts at level 2.
fn gib_specs(origin: [f32; 3], seed: u32, level: u8) -> Vec<ChunkSpec> {
    if level == 0 {
        return Vec::new();
    }
    const GIB_VELOCITY: f32 = 250.0;
    const GIB_JUMP: f32 = 250.0;
    let mut rng = SmallRng::new(seed);
    let gib = |model: &str, rng: &mut SmallRng| ChunkSpec {
        qpath: format!("models/gibs/{model}.md3"),
        origin,
        velocity: [
            rng.range(-1.0, 1.0) * GIB_VELOCITY,
            rng.range(-1.0, 1.0) * GIB_VELOCITY,
            GIB_JUMP + rng.range(-1.0, 1.0) * GIB_VELOCITY,
        ],
        angles: [0.0; 3],
        angle_velocity: [0.0; 3],
        life_ms: 5000 + (rng.range(0.0, 1.0) * 3000.0) as i32,
        bounce_factor: 0.6,
        scale: 1.0,
    };
    let mut specs = vec![gib(if rng.irand(0, 1) != 0 { "skull" } else { "brain" }, &mut rng)];
    if level >= 2 {
        for part in ["abdomen", "arm", "chest", "fist", "foot", "forearm", "intestine", "leg", "leg"] {
            specs.push(gib(part, &mut rng));
        }
    }
    specs
}

/// CG_Chunks: the LE_FRAGMENT spray for a broken brush/model (material_t in
/// `trickedentindex`). Glass, sparks, grates and rope only play effects/sounds.
fn prepare_chunks(event: &PresentationEvent, game: &ClientGameState) -> PreparedFxEvent {
    let state = &event.state;
    let chunk_type = state.field_i32("trickedentindex").unwrap_or(8);
    let num_chunks = state.field_i32("eventParm").unwrap_or(0).clamp(0, MAX_CHUNKS as i32);
    let (speed_mod, has_models) = match chunk_type {
        4 | 5 | 9 | 15 | 16 => (0.5, true),
        0 | 3 | 7 | 10 => (0.8, true),
        6 | 11 | 14 => (1.0, true),
        _ => (1.0, false), // MAT_GLASS, ELECTRICAL, GRATE1, ROPE, NONE
    };
    if !has_models || num_chunks == 0 {
        return PreparedFxEvent::None;
    }
    let custom = state.field_i32("modelindex").unwrap_or(0);
    let custom_model = (custom > 0).then(|| game.model_qpath(custom)).flatten();
    let origin = super::entity_vec3(state, "origin").unwrap_or(event.position);
    let mins = super::entity_vec3(state, "angles2").unwrap_or([0.0; 3]);
    let maxs = super::entity_vec3(state, "origin2").unwrap_or([0.0; 3]);
    let speed = state.field_f32("speed").unwrap_or(0.0);
    let mut base_scale = state.field_f32("apos.trBase[0]").unwrap_or(0.0);
    if base_scale <= 0.0 || base_scale.is_nan() {
        base_scale = 1.0;
    }
    let mut rng = SmallRng::new((event.receive_sequence as u32) ^ (event.server_time as u32).rotate_left(11));
    let mut specs = Vec::with_capacity(num_chunks as usize);
    for _ in 0..num_chunks {
        let family = match chunk_type {
            7 => "metal/metal1",
            9 => "rock/rock1",
            5 => "rock/rock2",
            4 => "rock/rock3",
            16 => if rng.irand(0, 1) != 0 { "rock/rock1" } else { "rock/rock3" },
            15 => "metal/wmetal1",
            11 => "crate/crate1",
            14 => "crate/crate2",
            10 => if rng.irand(0, 1) != 0 { "metal/metal2" } else { "metal/metal1" },
            _ => "metal/metal2", // METAL, ELEC_METAL, GLASS_METAL
        };
        let qpath = custom_model
            .clone()
            .unwrap_or_else(|| format!("models/chunks/{family}_{}.md3", rng.irand(1, 4)));
        let spot: [f32; 3] = std::array::from_fn(|i| {
            let r = rng.range(0.0, 1.0) * 0.8 + 0.1;
            r * mins[i] + (1.0 - r) * maxs[i]
        });
        let dir = normalize_or_up([spot[0] - origin[0], spot[1] - origin[1], spot[2] - origin[2]]);
        let chunk_speed = rng.range(speed * 0.5, speed * 1.25) * speed_mod;
        let angles = [rng.range(0.0, 360.0), rng.range(0.0, 360.0), rng.range(0.0, 360.0)];
        let spin = rng.range(0.0, 1.0) * 600.0 + 200.0;
        specs.push(ChunkSpec {
            qpath,
            origin: spot,
            velocity: dir.map(|c| c * chunk_speed),
            angles,
            angle_velocity: [rng.range(-1.0, 1.0) * spin, rng.range(-1.0, 1.0) * spin, 0.0],
            life_ms: 1300 + (rng.range(0.0, 1.0) * 900.0) as i32,
            bounce_factor: 0.2 + rng.range(0.0, 1.0) * 0.2,
            scale: rng.range(base_scale * 0.75, base_scale * 1.25),
        });
    }
    PreparedFxEvent::Chunks(specs)
}

/// CG_GlassShatter + CG_DoGlass. The reference fetches the brush face from the
/// renderer (`R_GetBModelVerts`); the glass is a thin box, so its face is the
/// bounds' plane across the thinnest axis (width along the horizontal axis,
/// height along Z when the glass is upright).
fn glass_shards(
    event: &PresentationEvent,
    model: Option<usize>,
    bounds: &[([f32; 3], [f32; 3])],
) -> Vec<ShardSpec> {
    let state = &event.state;
    let dmg_pt = super::entity_vec3(state, "origin").unwrap_or(event.position);
    let dmg_dir = super::entity_vec3(state, "angles").unwrap_or([0.0; 3]);
    let dmg_radius = state.field_i32("trickedentindex").unwrap_or(0) as f32;
    let max_shards = state.field_i32("pos.trTime").unwrap_or(0).max(0) as usize;

    // The glass entity's own model, else the smallest inline model around the damage point.
    let by_entity = model.and_then(|index| bounds.get(index).copied()).filter(|_| model != Some(0));
    let by_point = || {
        bounds
            .iter()
            .skip(1) // model 0 is the world
            .filter(|(mins, maxs)| (0..3).all(|i| dmg_pt[i] >= mins[i] - 16.0 && dmg_pt[i] <= maxs[i] + 16.0))
            .min_by(|a, b| {
                let volume = |(mins, maxs): &([f32; 3], [f32; 3])| (0..3).map(|i| maxs[i] - mins[i]).product::<f32>();
                volume(a).total_cmp(&volume(b))
            })
            .copied()
    };
    let Some((mins, maxs)) = by_entity.or_else(by_point) else {
        return Vec::new();
    };

    let extent: [f32; 3] = std::array::from_fn(|i| maxs[i] - mins[i]);
    let thin = (0..3).min_by(|&a, &b| extent[a].total_cmp(&extent[b])).unwrap_or(0);
    let (u_axis, v_axis) = match thin {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    let plane = (mins[thin] + maxs[thin]) * 0.5;
    let corner = |u: f32, v: f32| {
        let mut point = [0.0; 3];
        point[thin] = plane;
        point[u_axis] = u;
        point[v_axis] = v;
        point
    };
    let verts = [
        corner(mins[u_axis], mins[v_axis]),
        corner(maxs[u_axis], mins[v_axis]),
        corner(maxs[u_axis], maxs[v_axis]),
        corner(mins[u_axis], maxs[v_axis]),
    ];
    let width = extent[u_axis];
    let height = extent[v_axis];

    let mut rng = SmallRng::new((event.receive_sequence as u32) ^ (event.server_time as u32).rotate_left(7));
    let mut off_x = [[0.0f32; 20]; 20];
    let mut off_z = [[0.0f32; 20]; 20];
    for i in 0..20 {
        for t in 0..20 {
            off_x[t][i] = rng.range(-1.0, 1.0) * 0.03;
            off_z[i][t] = rng.range(-1.0, 1.0) * 0.03;
        }
    }

    const TIME_DECAY_SLOW: f32 = 0.1;
    const TIME_DECAY_MED: f32 = 0.04;
    const TIME_DECAY_FAST: f32 = 0.009;
    let (step_height, mx_height, time_decay) = if height < 100.0 {
        (0.2, 5, TIME_DECAY_SLOW)
    } else if height > 220.0 {
        (0.05, 20, TIME_DECAY_FAST)
    } else {
        (0.1, 10, TIME_DECAY_MED)
    };
    let step_width = (0.25 - width * 0.0002).max(0.01);
    let mx_width = ((width * 0.2) as i32).max(5);
    let time_decay = (time_decay + TIME_DECAY_FAST) * 0.5;

    let offx = |i: i32, t: i32| off_x[i.rem_euclid(20) as usize][t.rem_euclid(20) as usize];
    let offz = |t: i32, i: i32| off_z[t.rem_euclid(20) as usize][i.rem_euclid(20) as usize];
    let bilerp = |uv: [f32; 2]| -> [f32; 3] {
        // CG_CalcBiLerp: rows (v0,v1) and (v3,v2), mixed by uv[1].
        std::array::from_fn(|axis| {
            let bottom = verts[0][axis] * (1.0 - uv[0]) + verts[1][axis] * uv[0];
            let top = verts[3][axis] * (1.0 - uv[0]) + verts[2][axis] * uv[0];
            bottom * (1.0 - uv[1]) + top * uv[1]
        })
    };

    let mut specs = Vec::new();
    let mut z = 0.0f32;
    let mut i = 0i32;
    'rows: while z < 1.0 {
        let mut x = 0.0f32;
        let mut t = 0i32;
        while x < 1.0 {
            let jitter_x = |t_index: i32, i_index: i32, base: f32| {
                if t_index > 0 && t_index < mx_width { base - offx(i_index, t_index) } else { base }
            };
            let jitter_z = |t_index: i32, i_index: i32, base: f32| {
                if i_index > 0 && i_index < mx_height { base - offz(t_index, i_index) } else { base }
            };
            let uv = [
                [jitter_x(t, i, x), jitter_z(t, i, z)],
                [jitter_x(t + 1, i, x) + step_width, jitter_z(t + 1, i, z)],
                [jitter_x(t + 1, i + 1, x) + step_width, jitter_z(t + 1, i + 1, z) + step_height],
                [jitter_x(t, i + 1, x), jitter_z(t, i + 1, z) + step_height],
            ];
            let sub: [[f32; 3]; 4] = uv.map(bilerp);

            let mut dif = (0..3).map(|k| (sub[0][k] - dmg_pt[k]).powi(2)).sum::<f32>() * time_decay
                - rng.range(0.0, 1.0) * 32.0;
            dif -= dmg_radius * dmg_radius;
            let (stick, delay_ms) = if dif > 1.0 {
                (true, (dif + rng.range(0.0, 1.0) * 200.0) as i32)
            } else {
                (false, 0)
            };

            // CG_DoGlassQuad.
            let mut velocity = [rng.range(-12.0, 12.0), rng.range(-12.0, 12.0), -1.0];
            if !stick {
                for k in 0..3 {
                    velocity[k] += 0.3 * dmg_dir[k];
                }
            }
            specs.push(ShardSpec {
                positions: sub,
                uvs: uv,
                velocity,
                accel_z: -(600.0 + rng.range(0.0, 1.0) * 100.0),
                rotation: [rng.range(-40.0, 40.0), rng.range(-40.0, 40.0)],
                bounce: rng.range(0.0, 1.0) * 0.2 + 0.15,
                delay_ms,
            });
            if max_shards != 0 && specs.len() >= max_shards {
                break 'rows;
            }
            x += step_width;
            t += 1;
        }
        z += step_height;
        i += 1;
    }
    specs
}

fn saber_trail_rgb(color: i32) -> [u8; 3] {
    if let Some(rgb) = custom_saber_rgb(color) {
        return rgb;
    }
    match color {
        0 => [255, 0, 0],
        1 => [255, 64, 0],
        2 => [255, 255, 0],
        3 => [0, 255, 0],
        4 => [0, 64, 255],
        5 => [220, 0, 255],
        _ => [0, 64, 255],
    }
}

impl Drop for WeaponFx {
    fn drop(&mut self) {
        let _ = self.worker_tx.send(FxWorkerCommand::Shutdown);
        if let Some(worker) = self.worker.take() { let _ = worker.join(); }
    }
}

/// q_math.c PerpendicularVector: the axis `src` is least aligned with,
/// projected onto the plane across `src`.
fn perpendicular3(src: [f32; 3]) -> [f32; 3] {
    let mut pos = 0;
    let mut smallest = 1.0_f32;
    for (i, component) in src.iter().enumerate() {
        if component.abs() < smallest {
            smallest = component.abs();
            pos = i;
        }
    }
    let mut unit = [0.0; 3];
    unit[pos] = 1.0;
    let d = dot3(src, unit) / dot3(src, src);
    normalize3(sub3(unit, scale3(src, d)))
}

/// q_math.c RotatePointAroundVector: `point` turned `degrees` counterclockwise
/// (right-handed) about the unit vector `dir`.
fn rotate_about3(point: [f32; 3], dir: [f32; 3], degrees: f32) -> [f32; 3] {
    let (sin, cos) = degrees.to_radians().sin_cos();
    add3(
        add3(scale3(point, cos), scale3(cross3(dir, point), sin)),
        scale3(dir, dot3(dir, point) * (1.0 - cos)),
    )
}

fn normalize_or_up(v: [f32; 3]) -> [f32; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length == 0.0 { [0.0, 0.0, 1.0] } else { v.map(|c| c / length) }
}

fn scale_axis(axis: [[f32; 3]; 3], scale: f32) -> [[f32; 3]; 3] {
    axis.map(|row| row.map(|value| value * scale))
}

fn angles_to_axis(angles: [f32; 3]) -> [[f32; 3]; 3] {
    let (sp, cp) = angles[0].to_radians().sin_cos();
    let (sy, cy) = angles[1].to_radians().sin_cos();
    let (sr, cr) = angles[2].to_radians().sin_cos();
    [
        [cp * cy, cp * sy, -sp],
        [sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, sr * cp],
        [cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_saber_uses_tayst_assets_and_has_no_dynamic_light() {
        assert_eq!(
            saber_shaders(SABER_BLACK),
            ("gfx/effects/sabers/blackglow", "gfx/effects/sabers/blackcore")
        );
        // TaystJK's trail-colour switch has no black case, so black falls through
        // to the same blue vertex colour as its default while using blacktrail.
        assert_eq!(saber_trail_rgb(SABER_BLACK), [0, 64, 255]);

        let assets = AssetSearchPath::open_search_dirs(Vec::<std::path::PathBuf>::new()).unwrap();
        let mut fx = WeaponFx::new(assets);
        fx.begin_frame(1_000);
        fx.saber_dynamic_light(
            SaberTrailKey { entity_num: 1, saber_num: 0, blade_num: 0 },
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            32.0,
            32.0,
            SABER_BLACK,
            1,
            false,
        );
        assert!(fx.end_frame().lights.is_empty());

        // Its 3+ blade path is a separate combined-light routine and does include
        // black blades as white, matching TaystJK CG_DoSaberLight.
        let assets = AssetSearchPath::open_search_dirs(Vec::<std::path::PathBuf>::new()).unwrap();
        let mut fx = WeaponFx::new(assets);
        fx.begin_frame(1_001);
        fx.saber_dynamic_light(
            SaberTrailKey { entity_num: 2, saber_num: 0, blade_num: 0 },
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            32.0,
            32.0,
            SABER_BLACK,
            4,
            false,
        );
        let frame = fx.end_frame();
        assert_eq!(frame.lights.len(), 1);
        assert_eq!(frame.lights[0].rgb, [1.0, 1.0, 1.0]);
    }

    fn missile_generation() -> MissileFxGeneration {
        MissileFxGeneration {
            trajectory_time: 900,
            weapon: WP_BLASTER,
            custom_effect: 0,
            alt_fire: false,
        }
    }

    #[test]
    fn fixed_continuous_fx_rate_is_independent_of_presentation_frames() {
        let assets = AssetSearchPath::open_search_dirs(Vec::<std::path::PathBuf>::new()).unwrap();
        let mut fx = WeaponFx::new(assets);
        fx.set_continuous_fx_fps(90);
        let generation = missile_generation();

        assert_eq!(
            fx.missile_fx_samples(7, generation, 1000, [0.0, 0.0, 0.0]),
            vec![[0.0, 0.0, 0.0]],
            "first presentation seeds one visible sample"
        );
        assert!(
            fx.missile_fx_samples(7, generation, 1005, [5.0, 0.0, 0.0]).is_empty(),
            "rendering again before the 90 Hz tick must not add density"
        );

        let sample = fx.missile_fx_samples(7, generation, 1012, [12.0, 0.0, 0.0]);
        assert_eq!(sample.len(), 1);
        assert!((sample[0][0] - 11.111_111).abs() < 0.001);

        let samples = fx.missile_fx_samples(7, generation, 1035, [35.0, 0.0, 0.0]);
        assert_eq!(samples.len(), 2);
        assert!((samples[0][0] - 22.222_221).abs() < 0.001);
        assert!((samples[1][0] - 33.333_332).abs() < 0.001);
    }

    fn footprint_fx() -> WeaponFx {
        WeaponFx::new(AssetSearchPath::open_search_dirs(Vec::<std::path::PathBuf>::new()).unwrap())
    }

    fn footprint_at(yaw: f32, foot: jka_assets::animevents::FootstepType) -> FootstepImpact {
        FootstepImpact { entity: 3, foot, material: 8, position: [100.0, 50.0, 10.0], normal: [0.0, 0.0, 1.0], yaw }
    }

    fn mesh_of(frame: &FxFrame, shader: &str) -> Option<(Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[u8; 4]>)> {
        frame.draws.iter().find_map(|draw| match draw {
            FxDraw::Mesh { positions, uvs, rgba, shader: name, .. } if name == shader => {
                Some((positions.clone(), uvs.clone(), rgba.clone()))
            }
            _ => None,
        })
    }

    #[test]
    fn a_footprint_points_its_toes_along_the_legs_yaw() {
        use jka_assets::animevents::FootstepType;
        let mut fx = footprint_fx();
        fx.begin_frame(1000);
        // Yaw 90 on flat ground: the toes point along +Y, so the quad's +v edge is at y = 56.
        fx.footstep(&footprint_at(90.0, FootstepType::Right), FootstepStages::from_level(3), false);
        let frame = fx.end_frame();
        let (positions, uvs, rgba) = mesh_of(&frame, "footstep_r").expect("a print");
        assert_eq!(positions.len(), 4);
        for (position, uv) in positions.iter().zip(&uvs) {
            assert!((uv[1] - (0.5 + (position[1] - 50.0) / 12.0)).abs() < 1e-4, "v follows the toe direction: {position:?} {uv:?}");
            assert!((position[2] - 10.24).abs() < 1e-4, "lifted off the floor");
        }
        assert!(rgba.iter().all(|c| *c == [255, 255, 255, 255]));
    }

    #[test]
    fn left_feet_mirror_the_right_print_and_heavy_steps_use_their_own_shader() {
        use jka_assets::animevents::FootstepType;
        let mut fx = footprint_fx();
        fx.begin_frame(1000);
        fx.footstep(&footprint_at(0.0, FootstepType::Left), FootstepStages::from_level(3), false);
        fx.footstep(&footprint_at(0.0, FootstepType::HeavyRight), FootstepStages::from_level(3), false);
        let frame = fx.end_frame();
        let (positions, uvs, _) = mesh_of(&frame, "footstep_r").expect("the light left print");
        // At yaw 0 axis1 = Z x axis2 = +Y, so u grows with y for a right foot and falls for a left one.
        for (position, uv) in positions.iter().zip(&uvs) {
            assert!((uv[0] - (0.5 - (position[1] - 50.0) / 12.0)).abs() < 1e-4);
        }
        assert!(mesh_of(&frame, "footstep_heavy_r").is_some());
    }

    #[test]
    fn footprints_need_the_marks_stage_soft_ground_and_a_style() {
        use jka_assets::animevents::FootstepType;
        let draws = |fx: &mut WeaponFx, impact: &FootstepImpact, level: u8, owner: bool| {
            fx.foot_marks.clear();
            fx.footstep(impact, FootstepStages::from_level(level), owner);
            fx.foot_marks.len()
        };
        let mut fx = footprint_fx();
        fx.begin_frame(1000);
        let sand = footprint_at(0.0, FootstepType::Right);
        assert_eq!(draws(&mut fx, &sand, 3, false), 1);
        assert_eq!(draws(&mut fx, &sand, 2, false), 0, "effects but no graphics");
        let concrete = FootstepImpact { material: 11, ..sand.clone() };
        assert_eq!(draws(&mut fx, &concrete, 3, false), 0, "hard ground keeps no print");
        fx.set_footprint_mode(FootprintMode::Off);
        assert_eq!(draws(&mut fx, &sand, 3, false), 0, "r_footprints off");
        // In 3D mode the local player's snow is the Snowflow field's; everyone else's gets a print.
        fx.set_footprint_mode(FootprintMode::ThreeD);
        let snow = FootstepImpact { material: 14, ..sand };
        assert_eq!(draws(&mut fx, &snow, 3, true), 0);
        assert_eq!(draws(&mut fx, &snow, 3, false), 1);
        fx.set_footprint_mode(FootprintMode::TwoD);
        assert_eq!(draws(&mut fx, &snow, 3, true), 1);
    }

    #[test]
    fn prints_fade_their_colour_over_the_last_second() {
        use jka_assets::animevents::FootstepType;
        let mut fx = footprint_fx();
        fx.begin_frame(1000);
        fx.footstep(&footprint_at(0.0, FootstepType::Right), FootstepStages::from_level(3), false);
        fx.begin_frame(1000 + SABER_MARK_LIFETIME_MS - 500);
        let (_, _, rgba) = mesh_of(&fx.end_frame(), "footstep_r").unwrap();
        assert!(rgba.iter().all(|c| *c == [128, 128, 128, 255]), "half faded: {rgba:?}");
        fx.begin_frame(1000 + SABER_MARK_LIFETIME_MS);
        assert!(mesh_of(&fx.end_frame(), "footstep_r").is_none(), "gone after 10 s");
    }

    #[test]
    fn rotating_about_the_up_axis_turns_counterclockwise() {
        let forward = rotate_about3([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], 90.0);
        assert!((forward[1] - 1.0).abs() < 1e-5 && forward[0].abs() < 1e-5);
        assert_eq!(perpendicular3([0.0, 0.0, 1.0]), [1.0, 0.0, 0.0]);
    }

    #[test]
    fn item_cone_is_sampled_at_the_continuous_fx_rate() {
        let assets = AssetSearchPath::open_search_dirs(Vec::<std::path::PathBuf>::new()).unwrap();
        let mut fx = WeaponFx::new(assets);
        fx.set_continuous_fx_fps(100);
        let due = |fx: &WeaponFx| fx.entity_fx.get(&5).map(|state| state.next_time);
        fx.begin_frame(1000);
        fx.item_cone(5, [0.0; 3]);
        assert_eq!(due(&fx), Some(1010));
        fx.begin_frame(1004);
        fx.item_cone(5, [0.0; 3]);
        assert_eq!(due(&fx), Some(1010), "rendering again before the tick adds nothing");
        fx.begin_frame(1010);
        fx.item_cone(5, [0.0; 3]);
        assert_eq!(due(&fx), Some(1020));
    }

    /// The real `mp/itemcone.efx` (a cylinder plus flares) must produce geometry.
    #[test]
    #[ignore = "requires JKA_TEST_BASE with the stock PK3s"]
    fn stock_item_cone_effect_draws_a_cylinder() {
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let mut fx = WeaponFx::new(AssetSearchPath::open(std::path::Path::new(&base)).unwrap());
        fx.set_continuous_fx_fps(crate::fx::FX_FPS_LEGACY_JKA);
        fx.begin_frame(1000);
        fx.item_cone(5, [0.0, 0.0, 32.0]);
        let frame = fx.end_frame();
        let cylinders = frame.draws.iter().filter(|draw| matches!(draw, FxDraw::Cylinder { .. })).count();
        println!("item cone draws: {} ({} cylinder)", frame.draws.len(), cylinders);
        assert_eq!(cylinders, 1, "the light cone");
        assert!(frame.draws.len() > 1, "and its flares");
    }

    #[test]
    fn legacy_jka_continuous_fx_samples_every_presentation() {
        let assets = AssetSearchPath::open_search_dirs(Vec::<std::path::PathBuf>::new()).unwrap();
        let mut fx = WeaponFx::new(assets);
        fx.set_continuous_fx_fps(crate::fx::FX_FPS_LEGACY_JKA);
        let generation = missile_generation();

        for (time, x) in [(1000, 0.0), (1001, 1.0), (1001, 1.25), (1002, 2.0)] {
            assert_eq!(
                fx.missile_fx_samples(9, generation, time, [x, 0.0, 0.0]),
                vec![[x, 0.0, 0.0]],
            );
        }
    }

    #[test]
    fn push_puffs_follow_cg_add_puff() {
        let assets = AssetSearchPath::open_search_dirs(Vec::<std::path::PathBuf>::new()).unwrap();
        let mut fx = WeaponFx::new(assets);
        fx.set_view([1000.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        fx.begin_frame(1000);
        fx.player_fx(&PlayerFxRequest::PushPuffs { origin: [0.0; 3] });
        let frame = fx.end_frame();
        assert_eq!(frame.draws.len(), 2);
        let FxDraw::Sprite { radius, rgba, .. } = &frame.draws[0] else { panic!() };
        assert_eq!((*radius, *rgba), (8.0, [24, 32, 40, 0]), "c = 1 at spawn");
        fx.begin_frame(1060);
        let frame = fx.end_frame();
        let FxDraw::Sprite { origin, radius, rgba, .. } = &frame.draws[0] else { panic!() };
        assert!((origin[1] - 3.3).abs() < 1e-4, "drifts along the view's left at 55 u/s");
        assert!((radius - 9.0).abs() < 1e-5 && rgba[0] == 12, "half faded, half grown");
        fx.begin_frame(1120);
        assert!(fx.end_frame().draws.is_empty(), "120 ms life");
    }

    #[test]
    fn saber_blade_uses_stock_openjk_materials_and_optional_ribbon() {
        let assets = AssetSearchPath::open_search_dirs(Vec::<std::path::PathBuf>::new()).unwrap();
        let mut fx = WeaponFx::new(assets);
        let blade = PlayerFxRequest::SaberBlade {
            origin: [1.0, 2.0, 3.0],
            direction: [1.0, 0.0, 0.0],
            length: 10.0,
            length_max: 10.0,
            radius: 2.0,
            color: 0,
            entity_alpha: 1.0,
            entity_num: 7,
            saber_num: 0,
            blade_num: 0,
            saber_move: 0,
            torso_anim: 0,
            saber_in_flight: false,
            trail_style: 0,
            num_blades: 1,
            no_dlight: false,
            no_wall_marks: false,
        };

        fx.begin_frame(1000);
        fx.player_fx(&blade);
        let classic = fx.end_frame();
        assert_eq!(classic.lights.len(), 1, "CG_DoSaber emits one dlight for a single blade");
        assert_eq!(classic.lights[0].origin, [6.0, 2.0, 3.0]);
        assert_eq!(classic.lights[0].segment, Some([[1.0, 2.0, 3.0], [11.0, 2.0, 3.0]]));
        assert_eq!(classic.lights[0].rgb, [1.0, 0.2, 0.2]);
        assert!(classic.lights[0].radius >= 14.0 && classic.lights[0].radius <= 17.0);
        assert!(classic.draws.iter().any(|draw| matches!(
            draw,
            FxDraw::SaberGlow { shader, hilt_radius, .. }
                if shader == "gfx/effects/sabers/red_glow"
                    && (5.5..=5.75).contains(hilt_radius)
        )));
        assert!(classic.draws.iter().any(|draw| matches!(
            draw,
            FxDraw::Line { shader, .. } if shader == "gfx/effects/sabers/red_line"
        )));
        assert!(classic.draws.iter().all(|draw| match draw {
            FxDraw::SaberGlow { shader, .. } => shader == "gfx/effects/sabers/red_glow",
            FxDraw::Line { shader, .. } => shader == "gfx/effects/sabers/red_line",
            _ => false,
        }));
        assert_eq!(classic.draws.len(), 2, "one RT_SABER_GLOW plus one RT_LINE core");

        fx.set_modern_sabers(true);
        fx.begin_frame(1001);
        fx.player_fx(&blade);
        let modern = fx.end_frame();
        assert_eq!(modern.draws.len(), 5, "two glow ribbons, two caps, one hot core");
        assert_eq!(
            modern.draws.iter().filter(|draw| matches!(draw, FxDraw::Line { .. })).count(),
            3
        );
    }

    #[test]
    fn saber_flicker_matches_cg_dosaber_radius_math() {
        // Fully extended: glow spans radius * [0.85, 1.0], core radius/3 +/- 7.5%.
        let (glow_lo, core_lo) = saber_flicker_radii(2.0, 40.0, 40.0, -1.0, -1.0);
        let (glow_hi, core_hi) = saber_flicker_radii(2.0, 40.0, 40.0, 1.0, 1.0);
        assert!((glow_lo - 1.7).abs() < 1.0e-5 && (glow_hi - 2.0).abs() < 1.0e-5);
        assert!((core_lo - (2.0 / 3.0 - 0.15)).abs() < 1.0e-5);
        assert!((core_hi - (2.0 / 3.0 + 0.15)).abs() < 1.0e-5);

        // Still extending: both radii pick up the 1 + 2 / length halo.
        let (glow, core) = saber_flicker_radii(2.0, 4.0, 40.0, 1.0, 0.0);
        assert!((glow - 2.0 * 1.5).abs() < 1.0e-5);
        assert!((core - (2.0 / 3.0) * 1.5).abs() < 1.0e-5);
    }

    #[test]
    fn saber_no_dlight_suppresses_stock_blade_light() {
        let assets = AssetSearchPath::open_search_dirs(Vec::<std::path::PathBuf>::new()).unwrap();
        let mut fx = WeaponFx::new(assets);
        let blade = PlayerFxRequest::SaberBlade {
            origin: [0.0; 3],
            direction: [1.0, 0.0, 0.0],
            length: 40.0,
            length_max: 40.0,
            radius: 3.0,
            color: 4,
            entity_alpha: 1.0,
            entity_num: 3,
            saber_num: 0,
            blade_num: 0,
            saber_move: 0,
            torso_anim: 0,
            saber_in_flight: false,
            trail_style: 0,
            num_blades: 1,
            no_dlight: true,
            no_wall_marks: false,
        };
        fx.begin_frame(1000);
        fx.player_fx(&blade);
        let frame = fx.end_frame();
        assert!(frame.lights.is_empty());
        assert!(!frame.draws.is_empty(), "noDlight must not suppress the blade itself");
    }

    #[test]
    fn three_blade_saber_combines_to_one_openjk_light() {
        let assets = AssetSearchPath::open_search_dirs(Vec::<std::path::PathBuf>::new()).unwrap();
        let mut fx = WeaponFx::new(assets);
        fx.begin_frame(1000);
        for (blade_num, direction, color) in [
            (0, [1.0, 0.0, 0.0], 0),
            (1, [-1.0, 0.0, 0.0], 3),
            (2, [0.0, 1.0, 0.0], 4),
        ] {
            fx.player_fx(&PlayerFxRequest::SaberBlade {
                origin: [0.0; 3],
                direction,
                length: 10.0,
                length_max: 10.0,
                radius: 2.0,
                color,
                entity_alpha: 1.0,
                entity_num: 9,
                saber_num: 0,
                blade_num,
                saber_move: 0,
                torso_anim: 0,
                saber_in_flight: false,
                trail_style: 0,
                num_blades: 3,
                no_dlight: false,
                no_wall_marks: false,
            });
        }
        let frame = fx.end_frame();
        assert_eq!(frame.lights.len(), 1, "OpenJK CG_DoSaberLight emits one staff light");
        let light = &frame.lights[0];
        assert!((light.origin[0] - 0.0).abs() < 1e-5);
        assert!((light.origin[1] - (10.0 / 3.0)).abs() < 1e-5);
        assert!((light.origin[2] - 0.0).abs() < 1e-5);
        let expected = [
            (1.0 + 0.2 + 0.2) / 3.0,
            (0.2 + 1.0 + 0.4) / 3.0,
            (0.2 + 0.2 + 1.0) / 3.0,
        ];
        for (actual, expected) in light.rgb.iter().zip(expected) {
            assert!((*actual - expected).abs() < 1e-5);
        }
        assert!(light.radius >= 20.0 && light.radius <= 28.0);
        assert!(light.blade_segments.is_none());
    }

    #[test]
    fn rt_multiblade_preserves_color_weights_and_clipped_endpoints() {
        let assets = AssetSearchPath::open_search_dirs(Vec::<std::path::PathBuf>::new()).unwrap();
        let mut fx = WeaponFx::new(assets);
        fx.set_rt_lighting(true);
        fx.begin_frame(1000);
        for (blade_num, authored, visible, no_dlight) in [
            (0, 10.0, 5.0, false), (1, 20.0, 20.0, false),
            (2, 30.0, 0.0, false), (3, 40.0, 40.0, true),
        ] {
            fx.saber_dynamic_light(SaberTrailKey { entity_num: 9, saber_num: 0, blade_num },
                [0.0; 3], [1.0, 0.0, 0.0], visible, authored, blade_num as i32, 4, no_dlight);
        }
        let frame = fx.end_frame();
        assert_eq!(frame.lights.len(), 1);
        let segments = frame.lights[0].blade_segments.as_ref().unwrap();
        assert_eq!(segments.len(), 3, "noDlight blades must never enter the aggregate");
        assert_eq!(segments[0].endpoints, [[0.0; 3], [5.0, 0.0, 0.0]]);
        assert!((segments[0].weight - 1.0 / 6.0).abs() < 1e-6);
        assert!((segments[1].weight - 1.0 / 3.0).abs() < 1e-6);
        assert_eq!(segments[2].weight, 0.0, "fully clipped blades emit no light");
        for (i, segment) in segments.iter().enumerate() {
            assert_eq!(segment.rgb, saber_light_rgb(i as i32).unwrap());
        }
    }

    #[test]
    fn saber_trail_connects_consecutive_blade_samples_with_stock_shader() {
        let assets = AssetSearchPath::open_search_dirs(Vec::<std::path::PathBuf>::new()).unwrap();
        let mut fx = WeaponFx::new(assets);
        // A thrown saber is always trail-active in OpenJK and avoids depending
        // on a specific saberMove enum value in this focused renderer test.
        let mut blade = PlayerFxRequest::SaberBlade {
            origin: [0.0, 0.0, 0.0], direction: [1.0, 0.0, 0.0], length: 10.0, length_max: 10.0, radius: 2.0,
            color: 4, entity_alpha: 1.0, entity_num: 2, saber_num: 0, blade_num: 0,
            saber_move: 0, torso_anim: 0, saber_in_flight: true, trail_style: 0,
            num_blades: 1, no_dlight: false, no_wall_marks: false,
        };
        fx.begin_frame(1000);
        fx.player_fx(&blade);
        let _ = fx.end_frame();
        if let PlayerFxRequest::SaberBlade { origin, direction, .. } = &mut blade {
            *origin = [0.0, 1.0, 0.0];
            *direction = [0.0, 1.0, 0.0];
        }
        fx.begin_frame(1010);
        fx.player_fx(&blade);
        let frame = fx.end_frame();
        let quad = frame.draws.iter().find_map(|draw| match draw {
            FxDraw::Quad { positions, uvs, rgba, shader } => Some((positions, uvs, rgba, shader)),
            _ => None,
        }).expect("second sample should emit a saber trail quad");
        assert_eq!(quad.3, "gfx/effects/sabers/saberBlur");
        assert_eq!(quad.2[..3], [0, 64, 255]);
        assert_eq!(quad.0[0], [0.0, 1.0, 0.0]);
        assert_eq!(quad.1[2][0], 0.25, "10 ms / 40 ms stock fallback duration");

        fx.set_saber_trail(0);
        fx.begin_frame(1060);
        fx.player_fx(&blade);
        assert!(!fx.end_frame().draws.iter().any(|draw| matches!(draw, FxDraw::Quad { .. })));
    }

    #[test]
    fn relit_blade_trail_starts_from_the_off_sample_not_the_last_lit_one() {
        let assets = AssetSearchPath::open_search_dirs(Vec::<std::path::PathBuf>::new()).unwrap();
        let mut fx = WeaponFx::new(assets);
        let blade = |origin: [f32; 3], length: f32| PlayerFxRequest::SaberBlade {
            origin, direction: [1.0, 0.0, 0.0], length, length_max: 40.0, radius: 2.0,
            color: 4, entity_alpha: 1.0, entity_num: 2, saber_num: 0, blade_num: 1,
            saber_move: 0, torso_anim: 0, saber_in_flight: true, trail_style: 0,
            num_blades: 2, no_dlight: false, no_wall_marks: false,
        };
        let has_quad = |frame: &FxFrame| frame.draws.iter().any(|draw| matches!(draw, FxDraw::Quad { .. }));

        fx.begin_frame(1000);
        fx.player_fx(&blade([0.0, 0.0, 0.0], 10.0));
        let _ = fx.end_frame();

        // The blade is switched off mid-swing while the hilt keeps moving.
        fx.begin_frame(1010);
        fx.player_fx(&blade([100.0, 0.0, 0.0], 0.0));
        assert!(!has_quad(&fx.end_frame()), "an off blade draws no trail");

        // Relit: the quad must connect to the off sample, not to x = 0.
        fx.begin_frame(1020);
        fx.player_fx(&blade([100.0, 1.0, 0.0], 10.0));
        let frame = fx.end_frame();
        let positions = frame.draws.iter().find_map(|draw| match draw {
            FxDraw::Quad { positions, .. } => Some(*positions),
            _ => None,
        }).expect("relit blade should trail from the off sample");
        assert_eq!(positions[3], [100.0, 0.0, 0.0], "old base is where the blade was switched off");
    }

    #[test]
    fn vanilla_weapon_tables_match_cg_register_weapon() {
        assert_eq!(trail_effect(WP_BLASTER, false), Some("blaster/shot"));
        assert_eq!(trail_effect(WP_DEMP2, true), None, "DEMP2 alt has no trail func");
        assert_eq!(trail_effect(WP_THERMAL, false), None);
        assert_eq!(missile_model(WP_ROCKET_LAUNCHER, true), Some("models/weapons2/merr_sonn/projectile.md3"));
        assert_eq!(missile_model(WP_BLASTER, false), None);
        assert_eq!(wall_impacts(WP_THERMAL, false, 0).len(), 2);
        assert_eq!(wall_impacts(WP_FLECHETTE, true, 0).len(), 0, "flechette alt impact is its own blow effect");
        assert_eq!(wall_impacts(WP_BRYAR_PISTOL, true, 4), [("bryar/wall_impact3", false)]);
        assert_eq!(play_effect_type(12), Some("env/water_impact"));
    }
}

#[cfg(test)]
mod event_beam_tests {
    use super::*;
    use jka_protocol::gamestate::{EntityState, ENTITY_FIELDS};

    fn event(kind: EntityEvent, parm: i32, fields: &[(&str, u32)]) -> PresentationEvent {
        let mut state = EntityState { number: 40, fields: [0; ENTITY_FIELDS.len()] };
        for (name, value) in fields {
            let index = ENTITY_FIELDS.iter().position(|(field, _)| field == name).unwrap();
            state.fields[index] = *value;
        }
        PresentationEvent {
            receive_sequence: 7,
            source_entity_num: 40,
            entity_num: 40,
            event: kind,
            raw_event: kind.as_i32(),
            parm,
            position: [100.0, 0.0, 0.0],
            event_only_entity: true,
            server_time: 5_000,
            state,
        }
    }

    fn prepare(event: &PresentationEvent) -> PreparedFxEvent {
        prepare_entity_event(event, &ClientGameState::new(), &[], &SaberDefinitions::default())
    }

    #[test]
    fn disruptor_alt_shot_adds_beef_beam_only_at_full_charge() {
        let full = event(EntityEvent::EV_DISRUPTOR_SNIPER_SHOT, 0, &[("shouldtarget", 1)]);
        let PreparedFxEvent::PlayEffects { beams, .. } = prepare(&full) else { panic!("expected beams") };
        assert_eq!(beams.len(), 2);
        assert_eq!(beams[0].end, [100.0, 0.0, 0.0]);
        let partial = event(EntityEvent::EV_DISRUPTOR_SNIPER_SHOT, 0, &[]);
        let PreparedFxEvent::PlayEffects { beams, .. } = prepare(&partial) else { panic!("expected beams") };
        assert_eq!(beams.len(), 1);
        assert_eq!((beams[0].size2, beams[0].kill_time), (10.0, 175));
    }

    #[test]
    fn disruptor_impacts_pick_the_reference_effect() {
        let name = |kind, weapon| {
            let e = event(kind, 0, &[("weapon", weapon)]);
            let PreparedFxEvent::PlayEffects { effects, .. } = prepare(&e) else { panic!("expected effect") };
            effects[0].0.clone()
        };
        assert_eq!(name(EntityEvent::EV_DISRUPTOR_HIT, 1), "disruptor/flesh_impact");
        assert_eq!(name(EntityEvent::EV_DISRUPTOR_HIT, 0), "disruptor/wall_impact");
        assert_eq!(name(EntityEvent::EV_DISRUPTOR_SNIPER_MISS, 1), "disruptor/wall_impact");
        assert_eq!(name(EntityEvent::EV_DISRUPTOR_SNIPER_MISS, 0), "disruptor/alt_miss");
    }

    #[test]
    fn misc_model_explosion_scatters_inside_the_box() {
        let e = event(EntityEvent::EV_MISC_MODEL_EXP, 12, &[("time", 1)]);
        let PreparedFxEvent::PlayEffects { effects, .. } = prepare(&e) else { panic!("expected effects") };
        assert_eq!(effects.len(), 8 + 7); // grate: 8 + 7 * size
        assert!(effects.iter().all(|(name, ..)| name == "chunks/grateexplode"));
        let none = event(EntityEvent::EV_MISC_MODEL_EXP, 8, &[]); // MAT_NONE
        assert!(matches!(prepare(&none), PreparedFxEvent::None));
    }

    #[test]
    fn beams_expire_after_their_kill_time() {
        let mut fx = WeaponFx::new(AssetSearchPath::open_search_dirs(Vec::<std::path::PathBuf>::new()).unwrap());
        fx.begin_frame(1_000);
        let shot = event(EntityEvent::EV_DISRUPTOR_MAIN_SHOT, 0, &[]);
        let prepared = prepare(&shot);
        fx.entity_event_prepared(&shot, &prepared);
        let lines = |frame: &FxFrame| frame.draws.iter().filter(|draw| matches!(draw, FxDraw::Line { .. })).count();
        assert_eq!(lines(&fx.end_frame()), 1);
        fx.begin_frame(1_151);
        assert_eq!(lines(&fx.end_frame()), 0);
    }

    #[test]
    fn debris_spawns_tumbling_fragments_that_fall_and_expire() {
        let bits = |v: f32| v.to_bits();
        let debris = event(
            EntityEvent::EV_DEBRIS,
            0,
            &[
                ("trickedentindex", 9), // MAT_GREY_STONE
                ("eventParm", 6),
                ("speed", bits(100.0)),
                ("origin2[0]", bits(16.0)),
                ("origin2[1]", bits(16.0)),
                ("origin2[2]", bits(16.0)),
            ],
        );
        let PreparedFxEvent::Chunks(specs) = prepare(&debris) else { panic!("expected chunks") };
        assert_eq!(specs.len(), 6);
        assert!(specs.iter().all(|spec| spec.qpath.starts_with("models/chunks/rock/rock1_")));
        assert!(specs.iter().all(|spec| (1300..=2200).contains(&spec.life_ms)));

        let mut fx = WeaponFx::new(AssetSearchPath::open_search_dirs(Vec::<std::path::PathBuf>::new()).unwrap());
        fx.begin_frame(1_000);
        fx.entity_event_prepared(&debris, &PreparedFxEvent::Chunks(specs));
        let first = fx.chunk_models();
        assert_eq!(first.len(), 6);
        fx.begin_frame(1_500);
        let later = fx.chunk_models();
        // No collision world in this test: pure gravity, so z has dropped 0.5 g t^2 more than velocity gain.
        assert!(later.iter().zip(&first).all(|(a, b)| a.origin != b.origin));
        fx.begin_frame(3_300);
        assert!(fx.chunk_models().is_empty(), "fragments live at most 2.2 s");
    }

    #[test]
    fn glass_shatter_tessellates_the_thin_face_and_delays_far_shards() {
        // A 200 x 4 x 120 window with the damage point at its centre.
        let bounds = [([-1.0e4; 3], [1.0e4; 3]), ([0.0, 0.0, 0.0], [200.0, 4.0, 120.0])];
        let bits = |v: f32| v.to_bits();
        let shatter = event(
            EntityEvent::EV_GLASS_SHATTER,
            0,
            &[("origin[0]", bits(100.0)), ("origin[1]", bits(2.0)), ("origin[2]", bits(60.0))],
        );
        let specs = glass_shards(&shatter, Some(1), &bounds);
        assert!(specs.len() > 20, "a window this size breaks into many shards: {}", specs.len());
        assert!(specs.iter().all(|shard| shard.positions.iter().all(|p| (p[1] - 2.0).abs() < 1e-3)), "shards lie in the thin axis' mid-plane");
        let near = specs.iter().filter(|shard| shard.delay_ms == 0).count();
        assert!(near > 0 && near < specs.len(), "the impact area falls at once, the rest is delayed");
        // Entity model unknown: falls back to the box around the damage point.
        assert_eq!(glass_shards(&shatter, None, &bounds).len(), specs.len());
        // Capped by pos.trTime.
        let capped = event(EntityEvent::EV_GLASS_SHATTER, 0, &[("origin[0]", bits(100.0)), ("origin[1]", bits(2.0)), ("origin[2]", bits(60.0)), ("pos.trTime", 7)]);
        assert_eq!(glass_shards(&capped, Some(1), &bounds).len(), 7);
    }

    #[test]
    fn gib_counts_follow_cg_blood_and_launch_upwards() {
        assert!(gib_specs([0.0; 3], 1, 0).is_empty());
        let skull_or_brain = gib_specs([0.0; 3], 1, 1);
        assert_eq!(skull_or_brain.len(), 1);
        assert!(skull_or_brain[0].qpath.ends_with("skull.md3") || skull_or_brain[0].qpath.ends_with("brain.md3"));
        let full = gib_specs([0.0; 3], 1, 2);
        assert_eq!(full.len(), 10);
        assert!(full.iter().all(|gib| gib.velocity[2] >= 0.0 && gib.bounce_factor == 0.6));
        let mut fx = WeaponFx::new(AssetSearchPath::open_search_dirs(Vec::<std::path::PathBuf>::new()).unwrap());
        fx.set_gib_level(2);
        fx.begin_frame(1_000);
        let gib = event(EntityEvent::EV_GIB_PLAYER, 0, &[]);
        fx.entity_event_prepared(&gib, &prepare(&gib));
        assert_eq!(fx.chunk_models().len(), 10);
    }
}
