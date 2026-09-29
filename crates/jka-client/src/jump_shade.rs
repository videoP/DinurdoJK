//! jaPRO SP-physics jump-height helper.
//!
//! In jaPRO's SP movement style a landing at or above the height the jump
//! started from halves horizontal speed (bg_pmove.c, `PM_CrashLand`). While
//! airborne this tints every flat, walkable BSP surface by how it relates to
//! that height: green just under the jump line (best landing), through yellow
//! to red further down, and a dim blue-to-cyan zone above it that is still
//! reachable with the current force-jump level (the speed-penalty zone).
//!
//! This module only derives the tint parameters from the predicted player
//! state. The renderer draws them in the world shader, compiled in only while
//! the helper is engaged (`ENABLE_JUMP_SHADE`), so it is free when disabled.

use jka_protocol::server::PlayerState;

/// `STAT_MOVEMENTSTYLE` in jaPRO's `statIndex_t` (bg_public.h).
const STAT_MOVEMENTSTYLE: usize = 13;
/// `MV_SP` in jaPRO's `movementStyle_e` (bg_public.h, `_SPPHYSICS` build).
const MV_SP: i32 = 12;
const ENTITYNUM_NONE: i32 = 1023;
/// `forceJumpZStart` is 0 when grounded and -65536 after a teleport.
const TELEPORT_JUMP_Z: f32 = -65000.0;
/// `forceJumpHeightMax` per FP_LEVITATION level (bg_pmove.c).
const FORCE_JUMP_HEIGHT_MAX: [f32; 4] = [66.0, 130.0, 226.0, 418.0];
/// Player origin to the floor it stands on (mins.z is -24, plus a unit of slack).
const ORIGIN_TO_FLOOR: f32 = 25.0;
/// Distance below the jump line spanned by the green-to-red gradient. Past it
/// the shader continues with alternating red/pink 16-unit depth bands.
const GRADIENT_RANGE: f32 = 64.0;
/// Tint opacity over the surface's own lighting.
const STRENGTH: f32 = 0.85;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum JumpShadeState {
    /// Setting off, not a jaPRO server, or not in the SP movement style.
    #[default]
    Off,
    /// SP physics with the helper enabled, but no jump in progress.
    Idle,
    /// Airborne after a jump. Heights are floor levels in world units.
    Airborne {
        /// Floor height of the jump start: at or above it is penalised.
        threshold: f32,
        /// Highest floor still reachable with the current force-jump level.
        apex: f32,
    },
}

impl JumpShadeState {
    /// True while the world shader variant with the tint must be compiled in.
    pub fn engaged(self) -> bool {
        self != Self::Off
    }

    /// `[threshold, apex, gradient range, strength]` for the camera uniform.
    /// Zero strength makes the shader skip the tint entirely.
    pub fn uniform(self) -> [f32; 4] {
        match self {
            Self::Airborne { threshold, apex } => [threshold, apex, GRADIENT_RANGE, STRENGTH],
            Self::Off | Self::Idle => [0.0; 4],
        }
    }

    /// `ps` must be the local player's state on a jaPRO server.
    pub fn from_player_state(ps: &PlayerState) -> Self {
        Self::from_motion(
            ps.stats[STAT_MOVEMENTSTYLE],
            ps.field_i32("groundEntityNum").unwrap_or(0),
            ps.field_f32("fd.forceJumpZStart").unwrap_or(0.0),
            ps.field_i32("fd.forcePowerLevel[FP_LEVITATION]").unwrap_or(0),
        )
    }

    fn from_motion(
        movement_style: i32,
        ground_entity: i32,
        jump_z_start: f32,
        levitation_level: i32,
    ) -> Self {
        if movement_style != MV_SP {
            return Self::Off;
        }
        if ground_entity != ENTITYNUM_NONE || jump_z_start == 0.0 || jump_z_start <= TELEPORT_JUMP_Z
        {
            return Self::Idle;
        }
        let level = levitation_level.clamp(0, FORCE_JUMP_HEIGHT_MAX.len() as i32 - 1) as usize;
        let threshold = jump_z_start - ORIGIN_TO_FLOOR;
        Self::Airborne {
            threshold,
            apex: threshold + FORCE_JUMP_HEIGHT_MAX[level],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_sp_movement_style_engages() {
        assert_eq!(JumpShadeState::from_motion(1, ENTITYNUM_NONE, 100.0, 1), JumpShadeState::Off);
        assert!(!JumpShadeState::from_motion(0, 5, 0.0, 0).engaged());
        assert!(JumpShadeState::from_motion(MV_SP, 5, 0.0, 0).engaged());
    }

    #[test]
    fn grounded_or_teleported_players_are_idle() {
        assert_eq!(JumpShadeState::from_motion(MV_SP, 7, 100.0, 1), JumpShadeState::Idle);
        assert_eq!(JumpShadeState::from_motion(MV_SP, ENTITYNUM_NONE, 0.0, 1), JumpShadeState::Idle);
        assert_eq!(
            JumpShadeState::from_motion(MV_SP, ENTITYNUM_NONE, -65536.0, 1),
            JumpShadeState::Idle
        );
    }

    #[test]
    fn airborne_heights_follow_jump_start_and_force_level() {
        assert_eq!(
            JumpShadeState::from_motion(MV_SP, ENTITYNUM_NONE, 125.0, 2),
            JumpShadeState::Airborne { threshold: 100.0, apex: 326.0 }
        );
        // Out-of-range levels clamp instead of indexing past the table.
        assert_eq!(
            JumpShadeState::from_motion(MV_SP, ENTITYNUM_NONE, 125.0, 9),
            JumpShadeState::Airborne { threshold: 100.0, apex: 518.0 }
        );
        assert_eq!(
            JumpShadeState::from_motion(MV_SP, ENTITYNUM_NONE, 125.0, -1),
            JumpShadeState::Airborne { threshold: 100.0, apex: 166.0 }
        );
    }

    #[test]
    fn uniform_is_zero_unless_airborne() {
        assert_eq!(JumpShadeState::Off.uniform(), [0.0; 4]);
        assert_eq!(JumpShadeState::Idle.uniform(), [0.0; 4]);
        assert_eq!(
            JumpShadeState::Airborne { threshold: 1.0, apex: 2.0 }.uniform(),
            [1.0, 2.0, GRADIENT_RANGE, STRENGTH]
        );
    }
}
