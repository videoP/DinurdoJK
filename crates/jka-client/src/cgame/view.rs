//! OpenJK/TaystJK client-view policy that sits between player state and camera/rendering.
//!
//! This module deliberately contains no WGPU code and no snapshot ownership. Demo playback,
//! local play, and a future live network client all reduce their player state to
//! `PlayerViewPolicyState`, then use the same third-person decision and local-player alpha rules.

use super::PM_INTERMISSION;
use crate::camera::ThirdPersonSettings;
use jka_assets::animation::AnimationSet;
use jka_movement::{PlayerEntityView, PM_SPECTATOR};
use jka_protocol::server::PlayerState;

#[derive(Debug, Clone, Copy)]
pub struct PlayerViewPolicyState {
    pub health: i32,
    pub pm_type: i32,
    pub weapon: i32,
    pub legs_anim: i32,
    pub legs_timer: i32,
    pub torso_anim: i32,
    pub emplaced_index: i32,
    pub force_hand_extend: i32,
    pub falling_to_death: i32,
    pub vehicle_num: i32,
    pub zoom_mode: i32,
    pub team: i32,
}

impl PlayerViewPolicyState {
    /// Build policy state from a decoded network/demo playerState. Keeping this projection here
    /// prevents demo playback and live networking from independently reimplementing CG view rules.
    pub fn from_player_state(state: &PlayerState) -> Self {
        Self {
            health: state.stats[0],
            pm_type: state.field_i32("pm_type").unwrap_or(0),
            weapon: state.field_i32("weapon").unwrap_or(0),
            legs_anim: state.field_i32("legsAnim").unwrap_or(0),
            legs_timer: state.field_i32("legsTimer").unwrap_or(0),
            torso_anim: state.field_i32("torsoAnim").unwrap_or(0),
            emplaced_index: state.field_i32("emplacedIndex").unwrap_or(0),
            force_hand_extend: state.field_i32("forceHandExtend").unwrap_or(0),
            falling_to_death: state.field_i32("fallingToDeath").unwrap_or(0),
            vehicle_num: state.field_i32("m_iVehicleNum").unwrap_or(0),
            zoom_mode: state.field_i32("zoomMode").unwrap_or(0),
            team: state.persistant.get(3).copied().unwrap_or(0),
        }
    }

    /// Build the presentation-policy state from the native/OpenJK local-player bridge.
    /// `legs_timer` lives in `PlayerView` rather than `PlayerEntityView`, so the caller supplies it.
    pub fn from_entity_view(state: PlayerEntityView, legs_timer: i32) -> Self {
        Self {
            health: state.health,
            pm_type: state.pm_type,
            weapon: state.weapon,
            legs_anim: state.legs_anim,
            legs_timer,
            torso_anim: state.torso_anim,
            emplaced_index: state.emplaced_index,
            force_hand_extend: state.force_hand_extend,
            falling_to_death: state.falling_to_death,
            vehicle_num: state.vehicle_num,
            zoom_mode: state.zoom_mode,
            team: state.team,
        }
    }
}

fn bg_in_grapple_move(anim: i32) -> bool {
    matches!(
        AnimationSet::name(anim),
        Some(
            "BOTH_KYLE_GRAB"
                | "BOTH_KYLE_MISS"
                | "BOTH_KYLE_PA_1"
                | "BOTH_KYLE_PA_2"
                | "BOTH_PLAYER_PA_1"
                | "BOTH_PLAYER_PA_2"
                | "BOTH_PLAYER_PA_FLY"
        )
    )
}

fn pm_in_knockdown(legs_anim: i32, legs_timer: i32) -> bool {
    match AnimationSet::name(legs_anim) {
        Some(
            "BOTH_KNOCKDOWN1"
                | "BOTH_KNOCKDOWN2"
                | "BOTH_KNOCKDOWN3"
                | "BOTH_KNOCKDOWN4"
                | "BOTH_KNOCKDOWN5"
        ) => true,
        Some(
            "BOTH_GETUP1"
                | "BOTH_GETUP2"
                | "BOTH_GETUP3"
                | "BOTH_GETUP4"
                | "BOTH_GETUP5"
                | "BOTH_FORCE_GETUP_F1"
                | "BOTH_FORCE_GETUP_F2"
                | "BOTH_FORCE_GETUP_B1"
                | "BOTH_FORCE_GETUP_B2"
                | "BOTH_FORCE_GETUP_B3"
                | "BOTH_FORCE_GETUP_B4"
                | "BOTH_FORCE_GETUP_B5"
                | "BOTH_GETUP_BROLL_B"
                | "BOTH_GETUP_BROLL_F"
                | "BOTH_GETUP_BROLL_L"
                | "BOTH_GETUP_BROLL_R"
                | "BOTH_GETUP_FROLL_B"
                | "BOTH_GETUP_FROLL_F"
                | "BOTH_GETUP_FROLL_L"
                | "BOTH_GETUP_FROLL_R"
        ) => legs_timer != 0,
        _ => false,
    }
}

/// OpenJK `CG_DrawActiveFrame` third-person policy for the currently viewed player.
/// Camera placement itself stays in `camera.rs`; this function only decides whether the
/// local/followed body and third-person camera are active.
pub fn rendering_third_person(
    settings: ThirdPersonSettings,
    first_person_lightsaber: bool,
    state: PlayerViewPolicyState,
) -> bool {
    const WP_MELEE: i32 = 2;
    const WP_SABER: i32 = 3;
    const WP_EMPLACED_GUN: i32 = 17;
    const HANDEXTEND_KNOCKDOWN: i32 = 8;
    const TEAM_SPECTATOR: i32 = 3;

    // TaystJK CG_CalcViewValues handles PM_INTERMISSION before any ordinary
    // first/third-person offsets. Never let saber/melee/death policy turn the
    // fixed intermission viewpoint into a chase camera.
    if state.pm_type == PM_INTERMISSION {
        return false;
    }

    let mut third_person = settings.enabled || state.health <= 0;
    if state.health > 0 {
        if state.weapon == WP_EMPLACED_GUN && state.emplaced_index != 0 {
            third_person = true;
        } else if state.weapon == WP_SABER
            || state.weapon == WP_MELEE
            || bg_in_grapple_move(state.torso_anim)
            || bg_in_grapple_move(state.legs_anim)
            || state.force_hand_extend == HANDEXTEND_KNOCKDOWN
            || state.falling_to_death != 0
            || state.vehicle_num != 0
            || pm_in_knockdown(state.legs_anim, state.legs_timer)
        {
            // TaystJK/OpenJK cg_fpls: saber/melee normally force third person,
            // but cg_fpls allows first person for those two weapon classes when
            // cg_thirdPerson itself is disabled.
            third_person = !(!settings.enabled
                && first_person_lightsaber
                && (state.weapon == WP_SABER || state.weapon == WP_MELEE));
        } else if state.zoom_mode != 0 {
            third_person = false;
        }
    }
    if state.pm_type == PM_SPECTATOR || state.team == TEAM_SPECTATOR {
        third_person = false;
    }
    third_person
}

/// OpenJK `CG_CheckThirdPersonAlpha` writes `cg_thirdPersonAlpha` through the
/// refEntity shaderRGBA byte. Match the original 8-bit alpha quantization.
pub fn local_player_alpha(alpha: f32) -> f32 {
    if alpha < 1.0 {
        ((alpha * 255.0) as u8) as f32 / 255.0
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ordinary() -> PlayerViewPolicyState {
        PlayerViewPolicyState {
            health: 100,
            pm_type: 0,
            weapon: 0,
            legs_anim: 0,
            legs_timer: 0,
            torso_anim: 0,
            emplaced_index: 0,
            force_hand_extend: 0,
            falling_to_death: 0,
            vehicle_num: 0,
            zoom_mode: 0,
            team: 0,
        }
    }

    #[test]
    fn explicit_setting_controls_ordinary_player() {
        let mut settings = ThirdPersonSettings::default();
        assert!(!rendering_third_person(settings, false, ordinary()));
        settings.enabled = true;
        assert!(rendering_third_person(settings, false, ordinary()));
    }

    #[test]
    fn zoom_overrides_explicit_third_person_for_living_ordinary_player() {
        let mut settings = ThirdPersonSettings::default();
        settings.enabled = true;
        let mut state = ordinary();
        state.zoom_mode = 1;
        assert!(!rendering_third_person(settings, false, state));
    }

    #[test]
    fn saber_forces_third_person() {
        let mut state = ordinary();
        state.weapon = 3;
        assert!(rendering_third_person(ThirdPersonSettings::default(), false, state));
    }

    #[test]
    fn fpls_allows_saber_and_melee_first_person() {
        for weapon in [2, 3] {
            let mut state = ordinary();
            state.weapon = weapon;
            assert!(!rendering_third_person(
                ThirdPersonSettings::default(),
                true,
                state,
            ));
        }
    }

    #[test]
    fn intermission_bypasses_third_person_policy() {
        let mut settings = ThirdPersonSettings::default();
        settings.enabled = true;
        let mut state = ordinary();
        state.pm_type = PM_INTERMISSION;
        state.weapon = 3;
        assert!(!rendering_third_person(settings, false, state));
    }
}
