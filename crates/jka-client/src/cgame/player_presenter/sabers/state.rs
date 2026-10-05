//! Sabers state.
use crate::cgame::player_presenter::{
    Arc, Matrix3x4, PlayerAngleState, PlayerAnimationState, PlayerModelAsset,
};

/// OpenJK stores current/desired blade length in clientInfo_t::saber[].blade[].
/// Keep the same persistent presentation state so EF_DEAD can retract a live
/// player's blade instead of snapping it back to its authored full length.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(in crate::cgame::player_presenter) struct SaberBladeLengthKey {
    pub(in crate::cgame::player_presenter) client_num: usize,
    pub(in crate::cgame::player_presenter) saber_num: u8,
    pub(in crate::cgame::player_presenter) blade_num: u8,
}

#[derive(Clone, Copy, Debug)]
pub(in crate::cgame::player_presenter) struct SaberBladeLengthState {
    pub(in crate::cgame::player_presenter) length: f32,
    pub(in crate::cgame::player_presenter) length_max: f32,
    pub(in crate::cgame::player_presenter) desired_length: f32,
    pub(in crate::cgame::player_presenter) extend_debounce: i32,
    pub(in crate::cgame::player_presenter) last_update_time: i32,
}

impl SaberBladeLengthState {
    pub(in crate::cgame::player_presenter) fn new(
        length_max: f32,
        desired_length: f32,
        time: i32,
    ) -> Self {
        let length_max = length_max.max(0.0);
        let length = if desired_length == 0.0 {
            0.0
        } else {
            length_max
        };
        Self {
            length,
            length_max,
            desired_length,
            extend_debounce: time,
            last_update_time: time,
        }
    }

    /// Direct arithmetic port of OpenJK BG_SI_SetLengthGradual for one blade.
    pub(in crate::cgame::player_presenter) fn set_desired_and_update(
        &mut self,
        desired_length: f32,
        length_max: f32,
        time: i32,
    ) -> f32 {
        self.length_max = length_max.max(0.0);
        self.length = self.length.clamp(0.0, self.length_max);
        self.desired_length = desired_length;
        if self.last_update_time == time {
            return self.length;
        }
        self.last_update_time = time;

        let desired = if self.desired_length == -1.0 {
            self.length_max
        } else {
            self.desired_length.clamp(0.0, self.length_max)
        };
        if self.length == desired {
            return self.length;
        }
        if self.length == self.length_max || self.length == 0.0 {
            self.extend_debounce = time;
            if self.length == 0.0 {
                self.length += 1.0;
            } else {
                self.length -= 1.0;
            }
        }
        let mut amount = (time - self.extend_debounce) as f32 * 0.01;
        if amount < 0.2 {
            amount = 0.2;
        }
        if self.length < desired {
            self.length += amount;
            if self.length > desired {
                self.length = desired;
            }
            if self.length > self.length_max {
                self.length = self.length_max;
            }
        } else if self.length > desired {
            self.length -= amount;
            if self.length < desired {
                self.length = desired;
            }
            if self.length < 0.0 {
                self.length = 0.0;
            }
        }
        self.length
    }
}

pub(in crate::cgame::player_presenter) struct EntityPlayerState {
    pub(in crate::cgame::player_presenter) model_key: String,
    /// False while `model` is only a stand-in (previous model or default
    /// fallback) for a requested model that is still loading. A settled state
    /// with matching requested names lets the per-frame lookup be skipped.
    pub(in crate::cgame::player_presenter) model_settled: bool,
    // Cache the resolved model directly on the persistent centity-like state.
    // This avoids rebuilding/lowercasing qpath cache keys and hashing the global
    // model cache for every visible player on every frame.
    pub(in crate::cgame::player_presenter) requested_model_name: String,
    pub(in crate::cgame::player_presenter) requested_skin_name: String,
    pub(in crate::cgame::player_presenter) model: Arc<PlayerModelAsset>,
    pub(in crate::cgame::player_presenter) animation: PlayerAnimationState,
    // OpenJK cent->pe + clientInfo angle state consumed by BG_G2PlayerAngles.
    pub(in crate::cgame::player_presenter) player_angles: PlayerAngleState,
    pub(in crate::cgame::player_presenter) last_angle_time: i32,
    pub(in crate::cgame::player_presenter) last_e_flags: i32,
    pub(in crate::cgame::player_presenter) last_client_num: i32,
    /// `cent->ghoul2weapon`: which g2WeaponInstances entry was last copied.
    pub(in crate::cgame::player_presenter) ghoul2_weapon: Option<i32>,
    /// OpenJK `cent->weapon`: identity of the weapon model(s) currently bolted
    /// into the persistent Ghoul2 instance. This intentionally survives death
    /// even when currentState.weapon changes; EV_DESTROY_WEAPON_MODEL or a
    /// later live weapon swap is what changes the bolted model state.
    pub(in crate::cgame::player_presenter) cent_weapon: i32,
    /// Ghoul2 model index 1 on the player when it is a non-saber weapon.
    pub(in crate::cgame::player_presenter) attached_weapon: Option<i32>,
    /// Ghoul2 model index 1 when occupied by the primary saber hilt.
    pub(in crate::cgame::player_presenter) primary_saber_attached: bool,
    /// Ghoul2 model index 2 when occupied by the second saber hilt.
    pub(in crate::cgame::player_presenter) secondary_saber_attached: bool,
    /// `clientInfo_t::saberName/saber2Name` as of the last frame, so a userinfo
    /// change can force CG_NewClientInfo's weapon-instance refresh.
    pub(in crate::cgame::player_presenter) saber_names: [String; 2],
    /// jaPRO `CBoneCache::mSmoothBones`: the previous frame's filtered final
    /// bone pose, consumed by `r_ghoul2animsmooth`. `None` when there is no
    /// valid continuous history (first frame, a visibility gap, ragdoll, or a
    /// model swap), matching jaPRO's `touch != mLastTouch` reseed case.
    pub(in crate::cgame::player_presenter) bone_smooth_history: Option<Vec<Matrix3x4>>,
    /// `current_time` of the last `bone_smooth_history` update. Frustum
    /// culling returns early without touching the history above, so staleness
    /// is checked by elapsed time rather than relying on every early return to
    /// invalidate it.
    pub(in crate::cgame::player_presenter) bone_smooth_time: i32,
}
