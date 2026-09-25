//! OpenJK humanoid player animation state, downstream of decoded `entityState_t`.
//!
//! This is a direct Rust port of the active animation-control path in
//! `codemp/cgame/cg_players.c`: `CG_PlayerAnimation`, `CG_RunLerpFrame`,
//! `CG_FirstAnimFrame`, and `CG_SetLerpFrameAnimation`, plus the
//! `BG_SaberStartTransAnim` speed modifiers from `codemp/game/bg_panimate.c`.
//! It drives the Ghoul2 bone-animation implementation in `jka-assets`; it does
//! not invent a second animation system.

use jka_assets::{
    animation::{
        is_death_animation, is_flipping_animation, is_running_animation,
        is_saber_animation, is_saber_transition_animation, is_walking_animation,
        uses_saber_animation_speed_scale, Animation, AnimationSet,
    },
    ghoul2::{
        GlaAnimation, Ghoul2Animator, Matrix3x4, BONE_ANIM_BLEND,
        BONE_ANIM_OVERRIDE_FREEZE, BONE_ANIM_OVERRIDE_LOOP,
    },
    saber::SaberAnimationScales,
};
use jka_protocol::gamestate::EntityState;

use crate::cgame::ClientInfo;

// OpenJK codemp/qcommon/q_shared.h forcePowers_t.
const FP_SPEED: i32 = 2;
const FP_RAGE: i32 = 8;
// OpenJK codemp/game/bg_weapons.h weapon_t.
const WP_SABER: i32 = 3;
// OpenJK codemp/game/bg_public.h brokenLimb_t.
const BROKENLIMB_LARM: i32 = 1;
const BROKENLIMB_RARM: i32 = 2;
const CLASS_VEHICLE: i32 = 53;

#[derive(Debug, Clone)]
pub struct LerpFrameState {
    pub old_frame: i32,
    pub frame: i32,
    pub old_frame_time: i32,
    pub frame_time: i32,
    pub backlerp: f32,
    pub animation_number: i32,
    pub animation_time: i32,
    pub animation_speed: f32,
    pub animation_torso_speed: f32,
    pub last_forced_frame: i32,
    pub last_flip: bool,
    has_animation: bool,
}

impl Default for LerpFrameState {
    fn default() -> Self {
        Self {
            old_frame: 0,
            frame: 0,
            old_frame_time: 0,
            frame_time: 0,
            backlerp: 0.0,
            animation_number: -1,
            animation_time: 0,
            animation_speed: 0.0,
            animation_torso_speed: 0.0,
            last_forced_frame: -1,
            last_flip: false,
            has_animation: false,
        }
    }
}

/// Per-centity humanoid animation state. OpenJK stores the lerp frames on
/// `cent->pe`, Ghoul2 state on `cent->ghoul2`, and remembered animation values
/// on clientInfo; this Rust type owns exactly those animation responsibilities.
pub struct PlayerAnimationState {
    pub animator: Ghoul2Animator,
    pub legs: LerpFrameState,
    pub torso: LerpFrameState,
    ci_legs_anim: i32,
    ci_torso_anim: i32,
    ci_broken_limbs: i32,
    no_lumbar: bool,
}

impl PlayerAnimationState {
    /// OpenJK `CG_RegisterClientModelname` seeds model_root with frames 0..12 at
    /// speed 1.0 before cgame starts applying network animation IDs.
    pub fn new(gla: &GlaAnimation, current_time: i32) -> Result<Self, String> {
        let mut animator = Ghoul2Animator::new(gla);
        animator.set_bone_anim(
            gla,
            "model_root",
            0,
            12,
            BONE_ANIM_OVERRIDE_LOOP,
            1.0,
            current_time,
            None,
            -1,
        )?;
        Ok(Self {
            animator,
            legs: LerpFrameState::default(),
            torso: LerpFrameState::default(),
            ci_legs_anim: 0,
            ci_torso_anim: 0,
            ci_broken_limbs: 0,
            no_lumbar: Ghoul2Animator::bone_index(gla, "lower_lumbar").is_none(),
        })
    }

    /// Port of OpenJK `CG_PlayerAnimation` for the stock humanoid path.
    pub fn update(
        &mut self,
        entity: &EntityState,
        animations: &AnimationSet,
        gla: &GlaAnimation,
        client_info: &ClientInfo,
        saber_scales: &SaberAnimationScales,
        current_time: i32,
        anim_blend: bool,
    ) -> Result<(), String> {
        let legs_anim = field_i32(entity, "legsAnim");
        let torso_anim = field_i32(entity, "torsoAnim");
        let force_active = field_i32(entity, "forcePowersActive");

        let legs_speed = if !is_running_animation(legs_anim) && !is_walking_animation(legs_anim) {
            1.0
        } else if force_active & (1 << FP_RAGE) != 0 {
            1.3
        } else if force_active & (1 << FP_SPEED) != 0 {
            1.7
        } else {
            1.0
        };

        run_lerp_frame(
            &mut self.animator,
            &mut self.legs,
            &mut self.ci_legs_anim,
            &mut self.ci_torso_anim,
            &mut self.ci_broken_limbs,
            self.no_lumbar,
            entity,
            animations,
            gla,
            client_info,
            saber_scales,
            field_i32(entity, "legsFlip") != 0,
            legs_anim,
            legs_speed,
            false,
            current_time,
            anim_blend,
        )?;

        if field_i32(entity, "NPC_class") != CLASS_VEHICLE {
            let torso_speed = if force_active & (1 << FP_RAGE) != 0 {
                1.7
            } else {
                1.0
            };
            run_lerp_frame(
                &mut self.animator,
                &mut self.torso,
                &mut self.ci_legs_anim,
                &mut self.ci_torso_anim,
                &mut self.ci_broken_limbs,
                self.no_lumbar,
                entity,
                animations,
                gla,
                client_info,
                saber_scales,
                field_i32(entity, "torsoFlip") != 0,
                torso_anim,
                torso_speed,
                true,
                current_time,
                anim_blend,
            )?;
        }

        // CG_TriggerAnimSounds also refreshes cent->pe frame values from the
        // authoritative Ghoul2 bones. Preserve that state update even though
        // sound events are a later presentation layer.
        refresh_lerp_frames_from_ghoul2(
            &mut self.animator,
            &mut self.legs,
            &mut self.torso,
            self.no_lumbar,
            gla,
            current_time,
        )?;
        Ok(())
    }

    pub fn evaluate_pose_openjk_root(
        &mut self,
        gla: &GlaAnimation,
        current_time: i32,
    ) -> Result<Vec<Matrix3x4>, String> {
        self.animator.evaluate_pose_openjk_root(gla, current_time)
    }

    /// OpenJK `CG_ResetPlayerEntity` clears both lerp-frame structs after
    /// seeding the current animations.  On the next `CG_PlayerAnimation` call
    /// those animations are installed again from the authoritative
    /// `entityState_t`.  Resetting the Rust lerp bookkeeping here preserves
    /// that behavior without inventing a second animation transition path.
    pub fn reset_player_entity(&mut self) {
        self.legs = LerpFrameState::default();
        self.torso = LerpFrameState::default();
        self.animator.clear_bone_angle_overrides();
    }

    pub fn client_animation_ids(&self) -> (i32, i32) {
        (self.ci_legs_anim, self.ci_torso_anim)
    }
}

#[allow(clippy::too_many_arguments)]
fn run_lerp_frame(
    animator: &mut Ghoul2Animator,
    lf: &mut LerpFrameState,
    ci_legs_anim: &mut i32,
    ci_torso_anim: &mut i32,
    ci_broken_limbs: &mut i32,
    no_lumbar: bool,
    entity: &EntityState,
    animations: &AnimationSet,
    gla: &GlaAnimation,
    client_info: &ClientInfo,
    saber_scales: &SaberAnimationScales,
    flip_state: bool,
    new_animation: i32,
    speed_scale: f32,
    torso_only: bool,
    current_time: i32,
    anim_blend: bool,
) -> Result<(), String> {
    let force_frame = field_i32(entity, "forceFrame");
    if force_frame != 0 {
        if lf.last_forced_frame != force_frame {
            let flags = BONE_ANIM_OVERRIDE_FREEZE | BONE_ANIM_BLEND;
            for bone in ["lower_lumbar", "model_root", "Motion"] {
                animator.set_bone_anim(
                    gla,
                    bone,
                    force_frame,
                    force_frame + 1,
                    flags,
                    1.0,
                    current_time,
                    None,
                    150,
                )?;
            }
        }
        lf.last_forced_frame = force_frame;
        lf.animation_number = 0;
    } else {
        lf.last_forced_frame = -1;
        let speed_changed = if torso_only {
            lf.animation_torso_speed != speed_scale
        } else {
            lf.animation_speed != speed_scale
        };
        if new_animation != lf.animation_number
            || field_i32(entity, "brokenLimbs") != *ci_broken_limbs
            || lf.last_flip != flip_state
            || !lf.has_animation
            || speed_changed
        {
            set_lerp_frame_animation(
                animator,
                lf,
                ci_legs_anim,
                ci_torso_anim,
                ci_broken_limbs,
                no_lumbar,
                entity,
                animations,
                gla,
                client_info,
                saber_scales,
                new_animation,
                speed_scale,
                torso_only,
                flip_state,
                current_time,
                anim_blend,
            )?;
        }
    }

    lf.last_flip = flip_state;
    if lf.frame_time > current_time + 200 {
        lf.frame_time = current_time;
    }
    if lf.old_frame_time > current_time {
        lf.old_frame_time = current_time;
    }
    if lf.frame_time != 0 {
        lf.backlerp = if lf.frame_time == lf.old_frame_time {
            0.0
        } else {
            1.0
                - (current_time - lf.old_frame_time) as f32
                    / (lf.frame_time - lf.old_frame_time) as f32
        };
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn set_lerp_frame_animation(
    animator: &mut Ghoul2Animator,
    lf: &mut LerpFrameState,
    ci_legs_anim: &mut i32,
    ci_torso_anim: &mut i32,
    ci_broken_limbs: &mut i32,
    no_lumbar: bool,
    entity: &EntityState,
    animations: &AnimationSet,
    gla: &GlaAnimation,
    client_info: &ClientInfo,
    saber_scales: &SaberAnimationScales,
    new_animation: i32,
    anim_speed_mult: f32,
    torso_only: bool,
    flip_state: bool,
    current_time: i32,
    anim_blend: bool,
) -> Result<(), String> {
    let old_anim = lf.animation_number;
    let old_speed = lf.animation_speed;
    lf.animation_number = new_animation;
    let anim = *animations
        .get(new_animation)
        .ok_or_else(|| format!("Bad animation number: {new_animation}"))?;
    lf.has_animation = true;
    lf.animation_time = lf.frame_time + i32::from(anim.frame_lerp).abs();

    // The currently-supported player model path is humanoid (localAnimIndex 0),
    // so OpenJK's non-humanoid broken-limb bookkeeping does not run here.
    let _ = ci_broken_limbs;

    if anim.frame_lerp == 0 {
        return Err(format!("animation {new_animation} has zero frameLerp"));
    }
    let mut anim_speed = 50.0 / f32::from(anim.frame_lerp);
    let mut flags = if anim.loop_frames != -1 {
        BONE_ANIM_OVERRIDE_LOOP
    } else {
        BONE_ANIM_OVERRIDE_FREEZE
    };
    let (first_frame, last_frame) = animation_frame_range(anim, anim_speed);
    if anim_blend {
        flags |= BONE_ANIM_BLEND;
    }
    if is_death_animation(new_animation)
        || (old_anim != -1 && is_death_animation(old_anim))
    {
        flags &= !BONE_ANIM_BLEND;
    }
    let blend_time = if flags & BONE_ANIM_BLEND != 0
        && (is_flipping_animation(new_animation)
            || (old_anim != -1 && is_flipping_animation(old_anim)))
    {
        200
    } else {
        100
    };

    anim_speed *= anim_speed_mult;
    apply_saber_start_transition_speed(
        entity,
        client_info,
        saber_scales,
        new_animation,
        &mut anim_speed,
    );

    let resume_frame = if torso_only {
        let resume = lf.animation_torso_speed != anim_speed_mult
            && new_animation == old_anim
            && flip_state == lf.last_flip;
        lf.animation_torso_speed = anim_speed_mult;
        resume
    } else {
        let resume = lf.animation_speed != anim_speed_mult
            && new_animation == old_anim
            && flip_state == lf.last_flip;
        lf.animation_speed = anim_speed_mult;
        resume
    };

    if field_i32(entity, "NPC_class") == CLASS_VEHICLE {
        animator.set_bone_anim(
            gla,
            "model_root",
            first_frame,
            last_frame,
            flags,
            anim_speed,
            current_time,
            None,
            blend_time,
        )?;
        return Ok(());
    }

    let mut begin_frame: Option<f32> = None;
    if torso_only && !no_lumbar {
        if resume_frame {
            begin_frame = animator.bone_frame(
                gla,
                require_bone(gla, "lower_lumbar")?,
                current_time,
            )?;
        }

        // OpenJK always checks model_root after the optional lower_lumbar resume
        // and gives matching legs/torso animations priority to avoid a wobbly spine.
        if let Some(root_frame) = animator.bone_frame(
            gla,
            require_bone(gla, "model_root")?,
            current_time,
        )? {
            if field_i32(entity, "torsoAnim") == field_i32(entity, "legsAnim")
                && root_frame >= f32::from(anim.first_frame)
                && root_frame <= (i32::from(anim.first_frame) + i32::from(anim.num_frames)) as f32
            {
                begin_frame = Some(root_frame);
            }
        }

        if first_frame > last_frame || *ci_torso_anim == new_animation {
            begin_frame = None;
        }
        animator.set_bone_anim(
            gla,
            "lower_lumbar",
            first_frame,
            last_frame,
            flags,
            anim_speed,
            current_time,
            begin_frame,
            blend_time,
        )?;
        lf.frame = first_frame;
        *ci_torso_anim = new_animation;
    } else {
        if resume_frame {
            begin_frame = animator.bone_frame(
                gla,
                require_bone(gla, "model_root")?,
                current_time,
            )?;
        }
        if begin_frame.is_some_and(|frame| frame < first_frame as f32 || frame > last_frame as f32)
        {
            begin_frame = None;
        }

        if field_i32(entity, "torsoAnim") == field_i32(entity, "legsAnim")
            && (*ci_legs_anim != new_animation || old_speed != anim_speed)
        {
            let old_begin = begin_frame;
            if let Some(torso_frame) = animator.bone_frame(
                gla,
                require_bone(gla, "lower_lumbar")?,
                current_time,
            )? {
                begin_frame = Some(torso_frame);
                if torso_frame < first_frame as f32 || torso_frame > last_frame as f32 {
                    begin_frame = old_begin;
                }
            }
        }

        animator.set_bone_anim(
            gla,
            "model_root",
            first_frame,
            last_frame,
            flags,
            anim_speed,
            current_time,
            begin_frame,
            blend_time,
        )?;
        *ci_legs_anim = new_animation;
    }

    // localAnimIndex <= 1 for supported humanoid player models.
    if field_i32(entity, "torsoAnim") == new_animation && !no_lumbar {
        animator.set_bone_anim(
            gla,
            "Motion",
            first_frame,
            last_frame,
            flags,
            anim_speed,
            current_time,
            begin_frame,
            blend_time,
        )?;
    }
    Ok(())
}

fn animation_frame_range(anim: Animation, anim_speed: f32) -> (i32, i32) {
    if anim_speed < 0.0 {
        (
            i32::from(anim.first_frame) + i32::from(anim.num_frames),
            i32::from(anim.first_frame),
        )
    } else {
        (
            i32::from(anim.first_frame),
            i32::from(anim.first_frame) + i32::from(anim.num_frames),
        )
    }
}

fn apply_saber_start_transition_speed(
    entity: &EntityState,
    client_info: &ClientInfo,
    saber_scales: &SaberAnimationScales,
    animation: i32,
    speed: &mut f32,
) {
    if uses_saber_animation_speed_scale(animation) && field_i32(entity, "weapon") == WP_SABER {
        *speed *= saber_scales.get(&client_info.saber_name);
        *speed *= saber_scales.get(&client_info.saber2_name);
    }

    let broken = field_i32(entity, "brokenLimbs");
    if is_saber_transition_animation(animation) {
        match field_i32(entity, "fireflag") {
            1 => *speed *= 1.5, // FORCE_LEVEL_1
            3 => *speed *= 0.75, // FORCE_LEVEL_3
            _ => {}
        }
        apply_broken_arm_speed(broken, speed);
    } else if broken != 0 && is_saber_animation(animation) {
        apply_broken_arm_speed(broken, speed);
    }
}

fn apply_broken_arm_speed(broken: i32, speed: &mut f32) {
    // OpenJK checks right arm first, then left arm.
    if broken & (1 << BROKENLIMB_RARM) != 0 {
        *speed *= 0.5;
    } else if broken & (1 << BROKENLIMB_LARM) != 0 {
        *speed *= 0.65;
    }
}

fn refresh_lerp_frames_from_ghoul2(
    animator: &mut Ghoul2Animator,
    legs: &mut LerpFrameState,
    torso: &mut LerpFrameState,
    no_lumbar: bool,
    gla: &GlaAnimation,
    current_time: i32,
) -> Result<(), String> {
    if let Some(frame) = animator.bone_frame(gla, require_bone(gla, "model_root")?, current_time)? {
        let current = frame.floor() as i32;
        legs.old_frame = legs.frame;
        legs.frame = current;
    }
    if no_lumbar {
        torso.old_frame = legs.old_frame;
        torso.frame = legs.frame;
        return Ok(());
    }
    if let Some(frame) = animator.bone_frame(gla, require_bone(gla, "lower_lumbar")?, current_time)? {
        let current = frame.floor() as i32;
        torso.old_frame = torso.frame;
        torso.frame = current;
        torso.backlerp = 1.0 - (frame - current as f32);
    }
    Ok(())
}

fn require_bone(gla: &GlaAnimation, name: &str) -> Result<usize, String> {
    Ghoul2Animator::bone_index(gla, name)
        .ok_or_else(|| format!("unsupported humanoid model: missing Ghoul2 bone {name:?}"))
}

fn field_i32(entity: &EntityState, name: &str) -> i32 {
    entity.field_i32(name).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broken_arm_speed_order_matches_bg_saber_start_trans_anim() {
        let mut speed = 1.0;
        apply_broken_arm_speed((1 << BROKENLIMB_LARM) | (1 << BROKENLIMB_RARM), &mut speed);
        assert_eq!(speed, 0.5);
        let mut speed = 1.0;
        apply_broken_arm_speed(1 << BROKENLIMB_LARM, &mut speed);
        assert_eq!(speed, 0.65);
    }
}
