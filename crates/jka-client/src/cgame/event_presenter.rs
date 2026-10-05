//! Presentation-side equivalent of OpenJK `CG_EntityEvent`.
//!
//! Snapshot/event recognition stays in `cgame.rs`.  This module converts an
//! accepted JKA event into renderer/audio-facing presentation work.  The Rust
//! SoundPresenter handles supported audio events before this visual dispatcher.
//! Its fallback sound cases remain PARTIAL when audio is unavailable.

use std::{sync::Arc, time::Instant};

use jka_protocol::entity_event::EntityEvent;

use crate::{
    camera::CameraShake,
    renderer::{DynamicModelAlphaMode, DynamicModelSurface, DynamicWireframeClass, DynamicModelVertex},
    scene,
};

use super::{ClientGameState, PresentationEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventDispatchResult {
    Handled(&'static str),
    Partial(&'static str),
    Unhandled,
}

impl EventDispatchResult {
    pub fn status(self) -> String {
        match self {
            Self::Handled(reason) => format!("HANDLED:{reason}"),
            Self::Partial(reason) => format!("PARTIAL:{reason}"),
            Self::Unhandled => "UNHANDLED".to_owned(),
        }
    }

    pub fn class(self) -> EventDispatchClass {
        match self {
            Self::Handled(_) => EventDispatchClass::Handled,
            Self::Partial(_) => EventDispatchClass::Partial,
            Self::Unhandled => EventDispatchClass::Unhandled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventDispatchClass {
    Handled,
    Partial,
    Unhandled,
}

#[derive(Debug, Clone)]
struct EventFlash {
    entity_num: u16,
    origin: [f32; 3],
    start_time: i32,
    duration_ms: i32,
    radius: f32,
    color: [f32; 3],
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PreparedEventVisual {
    result: EventDispatchResult,
    flash: Option<PreparedFlash>,
    shake: Option<PreparedShake>,
}

/// `CGCam_Shake(es->angles[0], es->time)` after the `CG_EntityEvent` gates.
#[derive(Debug, Clone, Copy)]
struct PreparedShake {
    intensity: f32,
    duration_ms: i32,
}

#[derive(Debug, Clone, Copy)]
struct PreparedFlash {
    duration_ms: i32,
    radius: f32,
    color: [f32; 3],
}

pub(crate) fn prepare_event_visual(
    event: &PresentationEvent,
    game: &ClientGameState,
) -> PreparedEventVisual {
    let mut shake = None;
    let (result, flash) = match event.event {
        EntityEvent::EV_SABER_ATTACK => (EventDispatchResult::Partial("SABER_ATTACK_NO_AUDIO"), None),
        // OpenJK EV_SABER_HIT / BLOCK / CLASHFLARE are handled by WeaponFx
        // using the authored .efx path plus CG_SaberClashFlare. Do not fall
        // back to the old hand-made crossed-billboard flash.
        EntityEvent::EV_SABER_HIT
        | EntityEvent::EV_SABER_BLOCK
        | EntityEvent::EV_SABER_CLASHFLARE => (EventDispatchResult::Unhandled, None),
        EntityEvent::EV_SABER_UNHOLSTER => (EventDispatchResult::Partial("SABER_UNHOLSTER_NO_AUDIO"), None),
        EntityEvent::EV_SHIELD_HIT => (
            EventDispatchResult::Handled("SHIELD_HIT_FLASH"),
            Some(PreparedFlash { duration_ms: 120, radius: 13.0, color: [0.25, 0.55, 1.0] }),
        ),
        EntityEvent::EV_GENERAL_SOUND | EntityEvent::EV_GLOBAL_SOUND | EntityEvent::EV_ENTITY_SOUND => {
            let result = if game.sound_qpath(event.parm).is_some() {
                EventDispatchResult::Partial("SOUND_NO_AUDIO_BACKEND")
            } else {
                EventDispatchResult::Partial("SOUND_RESOURCE_MISSING")
            };
            (result, None)
        }
        EntityEvent::EV_GLOBAL_TEAM_SOUND => (EventDispatchResult::Partial("GLOBAL_TEAM_SOUND_NO_AUDIO_BACKEND"), None),
        EntityEvent::EV_PREDEFSOUND => (EventDispatchResult::Partial("PREDEFINED_SOUND_NO_AUDIO_BACKEND"), None),
        EntityEvent::EV_PLAY_EFFECT => (EventDispatchResult::Partial("EFFECT_RUNTIME_PENDING"), None),
        EntityEvent::EV_PLAY_EFFECT_ID | EntityEvent::EV_PLAY_PORTAL_EFFECT_ID => {
            let result = if game.effect_qpath(event.parm).is_some() {
                EventDispatchResult::Partial("EFFECT_RUNTIME_PENDING")
            } else {
                EventDispatchResult::Partial("EFFECT_RESOURCE_MISSING")
            };
            (result, None)
        }
        // Sound-only in OpenJK (or sound plus a HUD icon): with no audio backend
        // there is nothing else to present.
        EntityEvent::EV_FOOTSTEP
        | EntityEvent::EV_FOOTSTEP_METAL
        | EntityEvent::EV_FOOTSPLASH
        | EntityEvent::EV_FOOTWADE
        | EntityEvent::EV_SWIM
        | EntityEvent::EV_FALL
        | EntityEvent::EV_JUMP
        | EntityEvent::EV_ROLL
        | EntityEvent::EV_PAIN
        | EntityEvent::EV_DEATH1
        | EntityEvent::EV_DEATH2
        | EntityEvent::EV_DEATH3
        | EntityEvent::EV_TAUNT
        | EntityEvent::EV_WATER_TOUCH
        | EntityEvent::EV_WATER_LEAVE
        | EntityEvent::EV_WATER_UNDER
        | EntityEvent::EV_WATER_CLEAR
        | EntityEvent::EV_CHANGE_WEAPON
        | EntityEvent::EV_FIRE_WEAPON
        | EntityEvent::EV_ALT_FIRE
        | EntityEvent::EV_WEAPON_CHARGE
        | EntityEvent::EV_WEAPON_CHARGE_ALT
        | EntityEvent::EV_DISRUPTOR_ZOOMSOUND
        | EntityEvent::EV_ITEM_POP
        | EntityEvent::EV_MUTE_SOUND
        | EntityEvent::EV_STARTLOOPINGSOUND
        | EntityEvent::EV_STOPLOOPINGSOUND
        | EntityEvent::EV_PLAYDOORSOUND
        | EntityEvent::EV_PLAYDOORLOOPSOUND
        | EntityEvent::EV_BMODEL_SOUND
        | EntityEvent::EV_VOICECMD_SOUND
        | EntityEvent::EV_CLIENTJOIN => (EventDispatchResult::Partial("SOUND_NO_AUDIO_BACKEND"), None),
        // CG_EntityEvent has an empty body (or only a commented-out call) for these.
        EntityEvent::EV_JUMP_PAD
        | EntityEvent::EV_GRENADE_BOUNCE
        | EntityEvent::EV_MISSILE_STICK
        | EntityEvent::EV_BULLET => (EventDispatchResult::Handled("NO_CLIENT_PRESENTATION"), None),
        EntityEvent::EV_STEP_8 | EntityEvent::EV_STEP_12 | EntityEvent::EV_STEP_16 => {
            (EventDispatchResult::Handled("STEP_SMOOTHING"), None)
        }
        EntityEvent::EV_STEP_4 => (EventDispatchResult::Handled("STEP_SMOOTHING"), None),
        EntityEvent::EV_ITEM_PICKUP | EntityEvent::EV_GLOBAL_ITEM_PICKUP | EntityEvent::EV_ITEM_RESPAWN => {
            (EventDispatchResult::Partial("ITEM_HUD_PENDING"), None)
        }
        EntityEvent::EV_FORCE_DRAINED | EntityEvent::EV_TEAM_POWER => {
            (EventDispatchResult::Handled("TEAM_POWER_SHELL"), None)
        }
        // The gib models (models/gibs/*) are not shipped with JKA, so only the
        // gib sound exists to present.
        EntityEvent::EV_GIB_PLAYER => (EventDispatchResult::Handled("GIB_SOUND_ONLY"), None),
        EntityEvent::EV_DISRUPTOR_MAIN_SHOT
        | EntityEvent::EV_DISRUPTOR_SNIPER_SHOT
        | EntityEvent::EV_DISRUPTOR_SNIPER_MISS
        | EntityEvent::EV_DISRUPTOR_HIT => (EventDispatchResult::Partial("BEAM_FX_PENDING"), None),
        EntityEvent::EV_GLASS_SHATTER | EntityEvent::EV_DEBRIS | EntityEvent::EV_MISC_MODEL_EXP => {
            (EventDispatchResult::Handled("CHUNK_FX"), None) // built by WeaponFx
        }
        EntityEvent::EV_SCREENSHAKE => {
            shake = screen_shake(event, game);
            (EventDispatchResult::Handled("SCREEN_SHAKE"), None)
        }
        EntityEvent::EV_GHOUL2_MARK => (EventDispatchResult::Handled("G2_BURN_MARK"), None),
        EntityEvent::EV_PLAY_ROFF => (EventDispatchResult::Partial("ROFF_PENDING"), None),
        EntityEvent::EV_BODYFADE => (EventDispatchResult::Handled("BODY_FADE"), None),
        // Generic (non-player) Ghoul2 models are not presented, so there is no instance to destroy.
        EntityEvent::EV_DESTROY_GHOUL2_INSTANCE => (EventDispatchResult::Handled("NO_G2_INSTANCE"), None),
        EntityEvent::EV_VEH_FIRE => (EventDispatchResult::Handled("VEHICLE_MUZZLE_FX"), None),
        // CG_OutOfAmmoChange (`note_out_of_ammo`) and the saber force-HUD flash
        // (`note_hud_events`) are applied by the app.
        EntityEvent::EV_NOAMMO => (EventDispatchResult::Handled("NOAMMO_AUTOSWITCH"), None),
        // Text messages are queued by the app (`note_stringed_messages`); the
        // Q3 powerup flash state drives nothing in JKA's HUD.
        EntityEvent::EV_ITEMUSEFAIL | EntityEvent::EV_CTFMESSAGE => {
            (EventDispatchResult::Handled("STRINGED_MESSAGE"), None)
        }
        EntityEvent::EV_POWERUP_QUAD | EntityEvent::EV_POWERUP_BATTLESUIT => {
            (EventDispatchResult::Handled("NO_JKA_PRESENTATION"), None)
        }
        // HUD timer bar and score plums are queued by the app (`note_hud_events`).
        EntityEvent::EV_LOCALTIMER | EntityEvent::EV_SCOREPLUM => (EventDispatchResult::Handled("HUD_EVENT"), None),
        EntityEvent::EV_SIEGE_ROUNDOVER
        | EntityEvent::EV_SIEGE_OBJECTIVECOMPLETE
        | EntityEvent::EV_SIEGESPEC
        | EntityEvent::EV_GIVE_NEW_RANK
        | EntityEvent::EV_SET_FREE_SABER
        | EntityEvent::EV_SET_FORCE_DISABLE
        | EntityEvent::EV_DEBUG_LINE
        | EntityEvent::EV_TESTLINE => (EventDispatchResult::Partial("HUD_UI_EVENT_PENDING"), None),
        EntityEvent::EV_PLAYER_TELEPORT_IN
        | EntityEvent::EV_PLAYER_TELEPORT_OUT
        | EntityEvent::EV_BECOME_JEDIMASTER
        | EntityEvent::EV_CONC_ALT_IMPACT => (EventDispatchResult::Partial("FX_RUNTIME_PENDING"), None),
        EntityEvent::EV_GLOBAL_DUEL | EntityEvent::EV_PRIVATE_DUEL => {
            (EventDispatchResult::Handled("DUEL_ANNOUNCE"), None)
        }
        EntityEvent::EV_USE => (EventDispatchResult::Handled("NO_CLIENT_PRESENTATION"), None),
        other if (EntityEvent::EV_USE_ITEM0.as_i32()..=EntityEvent::EV_USE_ITEM15.as_i32()).contains(&other.as_i32()) => {
            (EventDispatchResult::Partial("USE_ITEM_SOUND_ONLY"), None)
        }
        other if super::sound_presenter::voice_event_custom_sound(other).is_some() => {
            (EventDispatchResult::Partial("SOUND_NO_AUDIO_BACKEND"), None)
        }
        _ => (EventDispatchResult::Unhandled, None),
    };
    PreparedEventVisual { result, flash, shake }
}

/// `cg.landChange/landTime` and `cg.stepChange/stepTime`: vertical view offsets
/// after a hard landing and while smoothing stair steps.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ViewKick {
    land_change: f32,
    land_time: i32,
    step_change: f32,
    step_time: i32,
    /// Local server only: length of the physics tick the eye interpolates over.
    /// Its step events are stamped with the tick's command time, so the render
    /// clock can trail them by up to this much. 0 = events never lead the clock.
    step_lead_ms: i32,
}

const LAND_DEFLECT_TIME: i32 = 150;
const LAND_RETURN_TIME: i32 = 300;
const STEP_TIME: i32 = 200;
const MAX_STEP_CHANGE: f32 = 32.0;

impl ViewKick {
    /// DoFall for the viewer: `delta` is the event's fall speed.
    pub fn land(&mut self, delta: i32, time: i32) {
        self.land_change = (-(delta as f32)).clamp(-32.0, 32.0);
        self.land_time = time;
    }

    pub fn set_step_lead_ms(&mut self, tick_msec: i32) {
        self.step_lead_ms = tick_msec.max(0);
    }

    /// EV_STEP_4/8/12/16 for the viewer (`steps` is 1..=4, four units each).
    pub fn step(&mut self, steps: i32, time: i32) {
        // cg.time never ran backwards in the original; a solo step event can lead it.
        let elapsed = (time - self.step_time).max(0);
        let old = if elapsed < STEP_TIME {
            self.step_change * (STEP_TIME - elapsed) as f32 / STEP_TIME as f32
        } else {
            0.0
        };
        self.step_change = (old + 4.0 * steps as f32).min(MAX_STEP_CHANGE);
        self.step_time = time;
    }

    /// `CG_CalculateWeaponPosition` landing-only weapon drop. OpenJK applies
    /// exactly one quarter of cg.landChange and does not include stair smoothing.
    pub fn weapon_land_offset(&self, time: i32) -> f32 {
        let delta = time - self.land_time;
        if (0..LAND_DEFLECT_TIME).contains(&delta) {
            self.land_change * 0.25 * delta as f32 / LAND_DEFLECT_TIME as f32
        } else if (LAND_DEFLECT_TIME..LAND_DEFLECT_TIME + LAND_RETURN_TIME).contains(&delta) {
            self.land_change
                * 0.25
                * (LAND_DEFLECT_TIME + LAND_RETURN_TIME - delta) as f32
                / LAND_RETURN_TIME as f32
        } else {
            0.0
        }
    }

    /// Vertical eye offset at `time` (CG_OffsetFirstPersonView / CG_StepOffset).
    /// The landing dip is a first-person-only effect; step smoothing also
    /// applies to the third-person camera.
    pub fn offset(&self, time: i32, first_person: bool) -> f32 {
        let mut offset = 0.0;
        let step_elapsed = time - self.step_time;
        if (0..STEP_TIME).contains(&step_elapsed) {
            offset -= self.step_change * (STEP_TIME - step_elapsed) as f32 / STEP_TIME as f32;
        } else if step_elapsed < 0 && -step_elapsed < self.step_lead_ms {
            // The eye is still rising to the stepped height across the tick that
            // stepped (interpolation), so cancel exactly that much of the rise.
            // Skipping this popped the eye up, then snapped it back down.
            let progress = 1.0 - (-step_elapsed) as f32 / self.step_lead_ms as f32;
            offset -= self.step_change * progress;
        }
        let land_elapsed = time - self.land_time;
        if first_person && land_elapsed >= 0 {
            if land_elapsed < LAND_DEFLECT_TIME {
                offset += self.land_change * land_elapsed as f32 / LAND_DEFLECT_TIME as f32;
            } else if land_elapsed < LAND_DEFLECT_TIME + LAND_RETURN_TIME {
                let f = 1.0 - (land_elapsed - LAND_DEFLECT_TIME) as f32 / LAND_RETURN_TIME as f32;
                offset += self.land_change * f;
            }
        }
        offset
    }
}

#[derive(Default)]
pub struct EventPresenter {
    flashes: Vec<EventFlash>,
    camera_shake: Option<CameraShake>,
    view_kick: ViewKick,
}

impl EventPresenter {
    pub(crate) fn dispatch_prepared(
        &mut self,
        event: &PresentationEvent,
        prepared: PreparedEventVisual,
    ) -> EventDispatchResult {
        if let Some(flash) = prepared.flash {
            self.flash(event, flash.duration_ms, flash.radius, flash.color);
        }
        if let Some(shake) = prepared.shake {
            // CGCam_Shake replaces any shake in progress.
            self.camera_shake = CameraShake::new(shake.intensity, Instant::now(), shake.duration_ms);
        }
        prepared.result
    }

    /// `CG_DoCameraShake`: an effect-driven shake, weaker with distance from the view.
    pub fn do_camera_shake(&mut self, view_origin: [f32; 3], origin: [f32; 3], intensity: f32, radius: i32, time_ms: i32) {
        let dist = (0..3).map(|i| (view_origin[i] - origin[i]).powi(2)).sum::<f32>().sqrt();
        if radius <= 0 || dist > radius as f32 {
            return;
        }
        let scale = 1.0 - dist / radius as f32;
        self.camera_shake = CameraShake::new(intensity * scale, Instant::now(), time_ms);
    }

    /// `CGCam_Shake` from the viewer's own weapon fire.
    pub fn add_camera_shake(&mut self, intensity: f32, time_ms: i32) {
        self.camera_shake = CameraShake::new(intensity, Instant::now(), time_ms);
    }

    pub fn view_kick_mut(&mut self) -> &mut ViewKick {
        &mut self.view_kick
    }

    pub fn view_kick(&self) -> ViewKick {
        self.view_kick
    }

    /// The shake still running, for the render snapshot.
    pub fn camera_shake(&self) -> Option<CameraShake> {
        self.camera_shake.filter(|shake| !shake.expired(Instant::now()))
    }

    fn flash(&mut self, event: &PresentationEvent, duration_ms: i32, radius: f32, color: [f32; 3]) {
        self.flashes.push(EventFlash {
            entity_num: event.entity_num,
            origin: event.position,
            start_time: event.server_time,
            duration_ms,
            radius,
            color,
        });
    }

    pub fn present(&mut self, current_time: i32) -> Vec<DynamicModelSurface> {
        self.flashes.retain(|flash| {
            current_time.saturating_sub(flash.start_time) <= flash.duration_ms
        });
        self.flashes
            .iter()
            .filter_map(|flash| flash_surface(flash, current_time))
            .collect()
    }

    pub fn clear(&mut self) {
        self.flashes.clear();
        self.camera_shake = None;
        self.view_kick = ViewKick::default();
    }
}

/// CG_EntityEvent EV_SCREENSHAKE: `modelindex` 0 shakes everyone, otherwise
/// only client `modelindex - 1`; ignored while the viewer is in a duel.
fn screen_shake(event: &PresentationEvent, game: &ClientGameState) -> Option<PreparedShake> {
    if game.japro_racemode() || super::sound_presenter::local_ps(game, "duelInProgress") != 0 {
        return None;
    }
    let target = event.state.field_i32("modelindex").unwrap_or(0);
    if target != 0 && super::sound_presenter::local_client(game).map(i32::from) != Some(target - 1) {
        return None;
    }
    Some(PreparedShake {
        intensity: event.state.field_f32("angles[0]").unwrap_or(0.0),
        duration_ms: event.state.field_i32("time").unwrap_or(0),
    })
}

fn flash_surface(flash: &EventFlash, current_time: i32) -> Option<DynamicModelSurface> {
    let age = current_time.saturating_sub(flash.start_time).max(0) as f32;
    let duration = flash.duration_ms.max(1) as f32;
    let alpha = (1.0 - age / duration).clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return None;
    }
    let radius = flash.radius * (0.75 + 0.35 * (age / duration));
    let axes = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let mut vertices = Vec::with_capacity(12);
    let mut indices = Vec::with_capacity(36);
    for pair in [(0usize, 1usize), (0, 2), (1, 2)] {
        let a = axes[pair.0];
        let b = axes[pair.1];
        let points = [
            add_scaled(flash.origin, a, -radius, b, -radius),
            add_scaled(flash.origin, a, radius, b, -radius),
            add_scaled(flash.origin, a, radius, b, radius),
            add_scaled(flash.origin, a, -radius, b, radius),
        ];
        let base = vertices.len() as u32;
        for (index, point) in points.into_iter().enumerate() {
            vertices.push(DynamicModelVertex {
                position: scene::render_position(point),
                normal: scene::render_position(a),
                uv: match index {
                    0 => [0.0, 0.0],
                    1 => [1.0, 0.0],
                    2 => [1.0, 1.0],
                    _ => [0.0, 1.0],
                },
                color: [flash.color[0], flash.color[1], flash.color[2], alpha * 0.55],
                depth_hack: 0.0,
            });
        }
        indices.extend_from_slice(&[
            base, base + 1, base + 2,
            base, base + 2, base + 3,
            base + 2, base + 1, base,
            base + 3, base + 2, base,
        ]);
    }
    Some(DynamicModelSurface {
        entity_num: flash.entity_num,
        wireframe_class: DynamicWireframeClass::Effect,
        raster_visible: true,
        vertices: Arc::new(vertices),
        indices: Arc::new(indices),
        lighting_origin: None,
        rt_rigid: None,
        rt_skinned_key: None,
        ghoul2_gpu: None,
        fx_gpu_sprites: None,
        texture: None,
        alpha_mode: DynamicModelAlphaMode::Additive,
    })
}

fn add_scaled(
    origin: [f32; 3],
    a: [f32; 3],
    sa: f32,
    b: [f32; 3],
    sb: f32,
) -> [f32; 3] {
    [
        origin[0] + a[0] * sa + b[0] * sb,
        origin[1] + a[1] * sa + b[1] * sb,
        origin[2] + a[2] * sa + b[2] * sb,
    ]
}

#[cfg(test)]
mod view_kick_tests {
    use super::*;

    #[test]
    fn landing_dips_then_returns_and_only_in_first_person() {
        let mut kick = ViewKick::default();
        kick.land(20, 1_000);
        assert_eq!(kick.offset(1_000, true), 0.0);
        assert!((kick.offset(1_075, true) + 10.0).abs() < 1e-4, "halfway down the deflect");
        assert!((kick.offset(1_150, true) + 20.0).abs() < 1e-4, "full dip");
        assert!((kick.offset(1_300, true) + 10.0).abs() < 1e-4, "halfway back");
        assert_eq!(kick.offset(1_450, true), 0.0);
        assert_eq!(kick.offset(1_150, false), 0.0, "third person has no landing dip");
        kick.land(500, 2_000);
        assert_eq!(kick.offset(2_150, true), -32.0, "clamped to 32 units");
    }

    #[test]
    fn steps_accumulate_and_decay_over_step_time() {
        let mut kick = ViewKick::default();
        kick.step(2, 1_000); // 8 units
        assert!((kick.offset(1_000, false) + 8.0).abs() < 1e-4);
        assert!((kick.offset(1_100, false) + 4.0).abs() < 1e-4);
        kick.step(4, 1_100); // 16 more on top of the remaining 4
        assert!((kick.offset(1_100, false) + 20.0).abs() < 1e-4);
        assert_eq!(kick.offset(1_300, false), 0.0);
        kick.step(4, 5_000);
        kick.step(4, 5_000);
        kick.step(4, 5_000);
        kick.step(4, 5_000);
        kick.step(4, 5_000);
        assert_eq!(kick.offset(5_000, false), -32.0, "MAX_STEP_CHANGE");
    }

    #[test]
    fn step_stamped_after_the_render_clock_cancels_the_interpolated_rise() {
        // Local server: the event carries the tick's command time, the eye is
        // drawn up to one tick earlier.
        let mut kick = ViewKick::default();
        kick.step(2, 1_008);
        kick.set_step_lead_ms(8);
        assert_eq!(kick.offset(1_000, false), 0.0, "eye has not started rising yet");
        assert_eq!(kick.offset(1_004, false), -4.0, "half the rise cancelled");
        assert_eq!(kick.offset(1_008, false), -8.0);
        assert!(kick.offset(1_108, false) > -8.0);
        assert_eq!(kick.offset(1_208, false), 0.0);
        // A clock that rebased far behind the event must not resurrect it.
        assert_eq!(kick.offset(100, false), 0.0);
    }

    #[test]
    fn effect_shake_needs_the_view_inside_its_radius_and_scales_with_distance() {
        let mut presenter = EventPresenter::default();
        presenter.do_camera_shake([0.0; 3], [1000.0, 0.0, 0.0], 8.0, 500, 1000);
        assert!(presenter.camera_shake().is_none(), "out of range");
        presenter.do_camera_shake([0.0; 3], [250.0, 0.0, 0.0], 8.0, 500, 1000);
        let shake = presenter.camera_shake().expect("in range");
        assert!((shake.intensity - 4.0).abs() < 1e-4, "half way out: half intensity");
    }
}
