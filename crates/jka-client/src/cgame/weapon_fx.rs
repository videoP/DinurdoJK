//! OpenJK CG_Missile (cg_ents.c), the missile/impact visuals registered by
//! CG_RegisterWeapon (cg_weaponinit.c) and fx_*.c, and the effect-playing
//! entity events: EV_MISSILE_HIT/MISS/MISS_METAL and EV_PLAY_EFFECT[_ID].
//! Vanilla branches only (JA+/JAPRO "tribes" substitutions are not ported).

use super::{event_presenter::EventDispatchResult, ClientGameState, PresentationEvent, PresentedEntity};
use super::player_presenter::PlayerFxRequest;
use crate::fx::system::{EffectId, FxDraw, FxFrame, FxSystem};
use jka_assets::pk3::AssetSearchPath;
use std::collections::HashMap;

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

const EV_PLAY_EFFECT: i32 = 68;
const EV_PLAY_EFFECT_ID: i32 = 69;
const EV_PLAY_PORTAL_EFFECT_ID: i32 = 70;
const EV_MISSILE_HIT: i32 = 85;
const EV_MISSILE_MISS: i32 = 86;
const EV_MISSILE_MISS_METAL: i32 = 87;

const MAX_EFX_BYTES: usize = 65536;

fn saber_shaders(color: i32) -> (&'static str, &'static str) {
    match color {
        0 => ("gfx/effects/sabers/red_glow", "gfx/effects/sabers/red_line"),
        1 => ("gfx/effects/sabers/orange_glow", "gfx/effects/sabers/orange_line"),
        2 => ("gfx/effects/sabers/yellow_glow", "gfx/effects/sabers/yellow_line"),
        3 => ("gfx/effects/sabers/green_glow", "gfx/effects/sabers/green_line"),
        4 => ("gfx/effects/sabers/blue_glow", "gfx/effects/sabers/blue_line"),
        5 => ("gfx/effects/sabers/purple_glow", "gfx/effects/sabers/purple_line"),
        _ => ("gfx/effects/sabers/blue_glow", "gfx/effects/sabers/blue_line"),
    }
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

pub struct WeaponFx {
    fx: FxSystem,
    assets: AssetSearchPath,
    ids: HashMap<String, EffectId>,
    puffs: Vec<Puff>,
    /// Per-frame presentation primitives that do not live in the FX scheduler
    /// (notably CG_DoSaber's RT_SABER_GLOW / RT_LINE submissions).
    immediate_draws: Vec<FxDraw>,
    modern_sabers: bool,
    time: i32,
    /// cg.refdef.vieworg / viewaxis[1] of the last rendered view.
    view_origin: [f32; 3],
    view_left: [f32; 3],
}

impl WeaponFx {
    pub fn new(assets: AssetSearchPath) -> Self {
        Self {
            fx: FxSystem::new(),
            assets,
            ids: HashMap::new(),
            puffs: Vec::new(),
            immediate_draws: Vec::new(),
            modern_sabers: false,
            time: 0,
            view_origin: [0.0; 3],
            view_left: [0.0, 1.0, 0.0],
        }
    }

    pub fn stats(&self) -> crate::fx::system::FxStats {
        self.fx.stats()
    }

    /// Select the optional continuous-ribbon saber presentation. The legacy
    /// path remains OpenJK-compatible and is the default.
    pub fn set_modern_sabers(&mut self, enabled: bool) {
        self.modern_sabers = enabled;
    }

    /// FX_AdjustTime for this presentation frame (`cg.time`).
    pub fn begin_frame(&mut self, time: i32) {
        self.immediate_draws.clear();
        if time < self.time {
            self.puffs.clear();
        }
        self.time = time;
        self.fx.adjust_time(time);
    }

    /// The view used for puff drift and near-kill (previous render frame).
    pub fn set_view(&mut self, origin: [f32; 3], left: [f32; 3]) {
        self.view_origin = origin;
        self.view_left = left;
    }

    /// FX_AddScheduledEffects + FX_Add, then CG_AddLocalEntities' puffs.
    pub fn end_frame(&mut self) -> FxFrame {
        let mut frame = self.fx.frame();
        frame.draws.append(&mut self.immediate_draws);
        let (time, view) = (self.time, self.view_origin);
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

    /// Force-power visuals requested by CG_Player.
    pub fn player_fx(&mut self, request: &PlayerFxRequest) {
        match request {
            PlayerFxRequest::Effect { name, origin, axis } => {
                let id = self.effect(name);
                if id != 0 {
                    self.fx.play_effect(id, *origin, *axis);
                }
            }
            PlayerFxRequest::SaberBlade {
                origin,
                direction,
                length,
                radius,
                color,
                entity_alpha,
            } => {
                self.saber_blade(
                    *origin,
                    *direction,
                    *length,
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
            PlayerFxRequest::GripPuffs { origin } => {
                // CG_ForceGripEffect.
                let wv = (self.time as f32 * 0.004).sin() * 0.08 + 0.1;
                self.puff(*origin, 55.0, 0.0, [(200.0 + wv * 255.0).min(255.0), 0.0, 0.0], "gfx/effects/forcePush");
                self.puff(*origin, -55.0, 180.0, [255.0; 3], "gfx/effects/sabers/red_glow");
            }
        }
    }

    fn saber_blade(
        &mut self,
        origin: [f32; 3],
        direction: [f32; 3],
        length: f32,
        radius: f32,
        color: i32,
        entity_alpha: f32,
    ) {
        if length < 0.5 || radius <= 0.0 {
            return;
        }
        let direction = normalize3(direction);
        let tip = madd3(origin, direction, length);
        let line_base = madd3(origin, direction, -1.0);
        let (glow_shader, line_shader) = saber_shaders(color);
        // GL_ONE GL_ONE has no source-alpha weighting. RF_FORCE_ENT_ALPHA-like
        // fades therefore scale vertex RGB, which is what rgbGen vertex uses.
        let intensity = (entity_alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
        let white = [intensity, intensity, intensity, 255];

        if self.modern_sabers {
            // Continuous camera-facing capsule: smoother than the legacy bead
            // chain while still using the stock authored saber materials.
            let soft = (f32::from(intensity) * 0.55).round() as u8;
            self.immediate_draws.push(FxDraw::Line {
                start: line_base,
                end: tip,
                width: radius * 1.8,
                rgba: [soft, soft, soft, 255],
                shader: glow_shader.to_owned(),
            });
            self.immediate_draws.push(FxDraw::Line {
                start: line_base,
                end: tip,
                width: radius * 0.95,
                rgba: white,
                shader: glow_shader.to_owned(),
            });
            self.immediate_draws.push(FxDraw::Sprite {
                origin,
                radius: (radius * 1.5).max(5.5),
                rotation: 0.0,
                rgba: [soft, soft, soft, 255],
                shader: glow_shader.to_owned(),
            });
            self.immediate_draws.push(FxDraw::Sprite {
                origin: tip,
                radius: radius * 0.9,
                rotation: 0.0,
                rgba: [soft, soft, soft, 255],
                shader: glow_shader.to_owned(),
            });
        } else {
            // OpenJK RB_SurfaceSaberGlow: march view-facing glow sprites from
            // blade tip to hilt, growing the radius slightly each step.
            let mut distance = length;
            let mut glow_radius = radius;
            while distance > 0.0 {
                self.immediate_draws.push(FxDraw::Sprite {
                    origin: madd3(origin, direction, distance),
                    radius: glow_radius,
                    rotation: 0.0,
                    rgba: white,
                    shader: glow_shader.to_owned(),
                });
                distance -= (glow_radius * 0.65).max(0.05);
                glow_radius += 0.017;
            }
            self.immediate_draws.push(FxDraw::Sprite {
                origin,
                radius: 5.5,
                rotation: 0.0,
                rgba: white,
                shader: glow_shader.to_owned(),
            });
        }

        // OpenJK CG_DoSaber uses an RT_LINE from blade tip to one unit behind
        // the hilt origin, at radius / 3, with the color-specific line shader.
        self.immediate_draws.push(FxDraw::Line {
            start: tip,
            end: line_base,
            width: (radius / 3.0).max(0.01),
            rgba: white,
            shader: line_shader.to_owned(),
        });
    }

    fn puff(&mut self, origin: [f32; 3], speed: f32, rotation: f32, color: [f32; 3], shader: &'static str) {
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

    fn effect(&mut self, name: &str) -> EffectId {
        if let Some(&id) = self.ids.get(name) {
            return id;
        }
        let assets = &mut self.assets;
        let id = self.fx.register(name, &mut |path| {
            assets.read(path, MAX_EFX_BYTES).ok().flatten().map(|asset| asset.bytes)
        });
        if id == 0 {
            println!("FX: effect {name} unavailable");
        }
        self.ids.insert(name.to_owned(), id);
        id
    }

    fn play(&mut self, name: &str, origin: [f32; 3], dir: [f32; 3]) -> bool {
        let id = self.effect(name);
        if id != 0 {
            self.fx.play_effect_dir(id, origin, dir);
        }
        id != 0
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
        if other2 != 0 {
            // An over-ridden trail effect (cgs.gameEffects = CS_EFFECTS).
            if e_flags & EF_JETPACK_ACTIVE == 0 {
                if let Some(name) = game.effect_qpath(other2) {
                    self.play(&name, entity.origin, forward);
                }
            }
            return None;
        }

        if let Some(name) = trail_effect(weapon, alt) {
            if weapon == WP_BRYAR_PISTOL && alt || weapon == WP_BRYAR_OLD && alt {
                // FX_BryarAltProjectileThink: one crackle per charge level.
                for _ in 1..state.field_i32("generic1").unwrap_or(0) {
                    self.play("bryar/crackleShot", entity.origin, forward);
                }
            }
            self.play(name, entity.origin, forward);
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

    /// The FX part of CG_EntityEvent.
    pub fn entity_event(&mut self, event: &PresentationEvent, game: &ClientGameState) -> Option<EventDispatchResult> {
        let state = &event.state;
        match event.event {
            EV_MISSILE_HIT | EV_MISSILE_MISS | EV_MISSILE_MISS_METAL => {
                let dir = jka_movement::byte_to_dir(event.parm);
                let custom = state.field_i32("emplacedOwner").unwrap_or(0);
                if custom != 0 {
                    // Hack in OpenJK too: an index to a custom impact effect.
                    let played = game.effect_qpath(custom).is_some_and(|name| self.play(&name, event.position, dir));
                    return Some(if played { EventDispatchResult::Handled("FX_CUSTOM_IMPACT") } else { EventDispatchResult::Partial("FX_CUSTOM_IMPACT_MISSING") });
                }
                if state.field_i32("eFlags").unwrap_or(0) & EF_JETPACK_ACTIVE != 0 && state.field_i32("otherEntityNum2").unwrap_or(0) != 0 {
                    return Some(EventDispatchResult::Partial("FX_VEHICLE_WEAPON_IMPACT_PENDING"));
                }
                let weapon = state.field_i32("weapon").unwrap_or(0);
                let alt = state.field_i32("eFlags").unwrap_or(0) & EF_ALT_FIRING != 0;
                let effects = if event.event == EV_MISSILE_HIT {
                    player_impacts(weapon, alt)
                } else {
                    wall_impacts(weapon, alt, if alt { state.field_i32("generic1").unwrap_or(0) } else { 0 })
                };
                let mut played = false;
                for &(name, up) in effects {
                    played |= self.play(name, event.position, if up { [0.0, 0.0, 1.0] } else { dir });
                }
                Some(if played { EventDispatchResult::Handled("FX_MISSILE_IMPACT") } else { EventDispatchResult::Partial("FX_MISSILE_IMPACT_NONE") })
            }
            EV_PLAY_EFFECT => {
                let Some(name) = play_effect_type(event.parm) else {
                    return Some(EventDispatchResult::Partial("FX_PLAY_EFFECT_TYPE_UNKNOWN"));
                };
                let mut dir = super::entity_vec3(state, "angles").unwrap_or([0.0; 3]);
                if dir == [0.0; 3] {
                    dir[1] = 1.0;
                }
                let origin = super::entity_vec3(state, "origin").unwrap_or(event.position);
                Some(if self.play(name, origin, dir) { EventDispatchResult::Handled("FX_PLAY_EFFECT") } else { EventDispatchResult::Partial("FX_EFFECT_MISSING") })
            }
            EV_PLAY_EFFECT_ID | EV_PLAY_PORTAL_EFFECT_ID => {
                let mut dir = angles_to_axis(super::entity_vec3(state, "angles").unwrap_or([0.0; 3]))[0];
                if dir == [0.0; 3] {
                    dir[1] = 1.0;
                }
                let Some(name) = game.effect_qpath(event.parm) else {
                    return Some(EventDispatchResult::Partial("EFFECT_RESOURCE_MISSING"));
                };
                // Portal effects belong to the sky-portal scene, which is not
                // rendered separately; they play in the main scene.
                Some(if self.play(&name, event.position, dir) { EventDispatchResult::Handled("FX_PLAY_EFFECT_ID") } else { EventDispatchResult::Partial("FX_EFFECT_MISSING") })
            }
            _ => None,
        }
    }
}

fn normalize_or_up(v: [f32; 3]) -> [f32; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length == 0.0 { [0.0, 0.0, 1.0] } else { v.map(|c| c / length) }
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
            radius: 2.0,
            color: 0,
            entity_alpha: 1.0,
        };

        fx.begin_frame(1000);
        fx.player_fx(&blade);
        let classic = fx.end_frame();
        assert!(classic.draws.iter().any(|draw| matches!(
            draw,
            FxDraw::Sprite { shader, .. } if shader == "gfx/effects/sabers/red_glow"
        )));
        assert!(classic.draws.iter().any(|draw| matches!(
            draw,
            FxDraw::Line { shader, .. } if shader == "gfx/effects/sabers/red_line"
        )));
        assert!(classic.draws.iter().all(|draw| match draw {
            FxDraw::Sprite { shader, .. } => shader == "gfx/effects/sabers/red_glow",
            FxDraw::Line { shader, .. } => shader == "gfx/effects/sabers/red_line",
            _ => false,
        }));

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
