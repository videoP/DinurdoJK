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
    /// MVSDK `lf->animation`: the installed animation, kept while forceFrame
    /// zeroes `animationNumber`. Only the JK2 path uses it.
    jk2_animation: Option<Animation>,
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
            jk2_animation: None,
        }
    }
}

/// Per-centity humanoid animation state. OpenJK stores the lerp frames on
/// `cent->pe`, Ghoul2 state on `cent->ghoul2`, and remembered animation values
/// on clientInfo; this Rust type owns exactly those animation responsibilities.
#[derive(Clone)]
pub struct PlayerAnimationState {
    pub animator: Ghoul2Animator,
    pub legs: LerpFrameState,
    pub torso: LerpFrameState,
    ci_legs_anim: i32,
    ci_torso_anim: i32,
    ci_broken_limbs: i32,
    no_lumbar: bool,
    /// Whether `update` has run since the last reset, so the first frame an
    /// entity is seen does not look like a jump through the cycle.
    frames_seen: bool,
    /// The legs bone frames `CG_TriggerAnimSounds` compared on the last update.
    legs_step: Option<LegsFrameStep>,
}

/// The legs bone went from `old_frame` to `frame` while playing `anim`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegsFrameStep {
    pub old_frame: i32,
    pub frame: i32,
    pub anim: i32,
}

impl PlayerAnimationState {
    /// `CG_RegisterClientModelname` seeds the *humanoid player* model_root with
    /// 0..12. That setup is not used by TaystJK's `CG_G2AnimEntModelLoad` path
    /// for CLASS_VEHICLE; a valid fighter GLA can contain only two frames.
    pub fn new(gla: &GlaAnimation, current_time: i32) -> Result<Self, String> {
        let mut state = Self::new_vehicle(gla);
        if Ghoul2Animator::bone_index(gla, "model_root").is_some() {
            state.animator.set_bone_anim(
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
        }
        Ok(state)
    }

    /// The non-humanoid vehicle starts in its authored frame-0 pose. The
    /// authoritative `legsAnim` is applied by `update` only when defined in
    /// the vehicle's actual animation.cfg; no humanoid 12-frame pre-roll.
    pub fn new_vehicle(gla: &GlaAnimation) -> Self {
        Self {
            animator: Ghoul2Animator::new(gla),
            legs: LerpFrameState::default(),
            torso: LerpFrameState::default(),
            ci_legs_anim: 0,
            ci_torso_anim: 0,
            ci_broken_limbs: 0,
            no_lumbar: Ghoul2Animator::bone_index(gla, "lower_lumbar").is_none(),
            frames_seen: false,
            legs_step: None,
        }
    }

    /// The legs frame change of the last `update`, the input of the animation
    /// events (footsteps). `None` when the frame did not move.
    pub fn legs_frame_step(&self) -> Option<LegsFrameStep> {
        self.legs_step
    }

    /// Port of OpenJK `CG_PlayerAnimation` for the stock humanoid path.
    /// `jk2` (`2` = 1.02, `4` = 1.04) selects the MVSDK cgame port instead.
    pub fn update(
        &mut self,
        entity: &EntityState,
        animations: &AnimationSet,
        gla: &GlaAnimation,
        client_info: &ClientInfo,
        saber_scales: &SaberAnimationScales,
        current_time: i32,
        anim_blend: bool,
        humanoid: bool,
        jk2: Option<u8>,
    ) -> Result<(), String> {
        if let Some(version) = jk2.filter(|_| humanoid && !self.no_lumbar) {
            return self.update_jk2(version, entity, animations, gla, current_time, anim_blend);
        }
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
            humanoid,
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
                humanoid,
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
            "lower_lumbar",
            gla,
            current_time,
        )?;
        self.finish_frame_step();
        Ok(())
    }

    fn finish_frame_step(&mut self) {
        self.legs_step = (self.frames_seen && self.legs.frame != self.legs.old_frame).then_some(LegsFrameStep {
            old_frame: self.legs.old_frame,
            frame: self.legs.frame,
            anim: self.legs.animation_number,
        });
        self.frames_seen = true;
    }

    /// MVSDK `CG_PlayerAnimation` (code/cgame/cg_players.c). Unlike OpenJK,
    /// Force Rage/Speed scale *every* legs animation, and there is no
    /// broken-limb or sabers.cfg speed input.
    fn update_jk2(
        &mut self,
        version: u8,
        entity: &EntityState,
        animations: &AnimationSet,
        gla: &GlaAnimation,
        current_time: i32,
        anim_blend: bool,
    ) -> Result<(), String> {
        let force_active = field_i32(entity, "forcePowersActive");
        let legs_speed = if force_active & (1 << FP_RAGE) != 0 {
            1.3
        } else if force_active & (1 << FP_SPEED) != 0 {
            1.7
        } else {
            1.0
        };
        let torso_speed = if force_active & (1 << FP_RAGE) != 0 { 1.7 } else { 1.0 };
        let mut jk2 = Jk2Lerp {
            version,
            animator: &mut self.animator,
            ci_legs_anim: &mut self.ci_legs_anim,
            ci_torso_anim: &mut self.ci_torso_anim,
            entity,
            animations,
            gla,
            current_time,
            anim_blend,
        };
        // CG_RunLerpFrame advances JK2's own frame counter (lf->frame), which
        // CG_FirstAnimFrame, the 1.04 torso/legs sync and footsteps read.
        self.legs_step = jk2.run(&mut self.legs, field_i32(entity, "legsAnim"),
            field_i32(entity, "legsFlip") != 0, legs_speed, false, 0)?;
        let legs_frame = self.legs.frame;
        jk2.run(&mut self.torso, field_i32(entity, "torsoAnim"),
            field_i32(entity, "torsoFlip") != 0, torso_speed, true, legs_frame)?;
        self.frames_seen = true;
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
        self.frames_seen = false;
        self.legs_step = None;
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
    humanoid: bool,
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
                if Ghoul2Animator::bone_index(gla, bone).is_none() {
                    continue;
                }
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
                humanoid,
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
    humanoid: bool,
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

    // TaystJK CG_SetLerpFrameAnimation allows undefined (0-frame) entries for
    // non-humanoid animation sets. They are valid no-ops, not registration errors.
    if !humanoid && anim.first_frame == 0 && anim.num_frames == 0 {
        return Ok(());
    }

    // Broken-limb pose overrides are humanoid-only in the original path.
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

        if !no_lumbar
            && field_i32(entity, "torsoAnim") == field_i32(entity, "legsAnim")
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

    // Non-humanoid NPC skeletons do not require the humanoid Motion chain.
    if humanoid && field_i32(entity, "torsoAnim") == new_animation && !no_lumbar {
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
    torso_bone: &str,
    gla: &GlaAnimation,
    current_time: i32,
) -> Result<(), String> {
    // G2API_GetBoneFrame simply returns false when a creature lacks a queried
    // bone. Mirror that instead of turning optional NPC bones into fatal errors.
    if let Some(root) = Ghoul2Animator::bone_index(gla, "model_root") {
        if let Some(frame) = animator.bone_frame(gla, root, current_time)? {
            let current = frame.floor() as i32;
            legs.old_frame = legs.frame;
            legs.frame = current;
        }
    }
    if no_lumbar {
        torso.old_frame = legs.old_frame;
        torso.frame = legs.frame;
        return Ok(());
    }
    if let Some(frame) = animator.bone_frame(gla, require_bone(gla, torso_bone)?, current_time)? {
        let current = frame.floor() as i32;
        torso.old_frame = torso.frame;
        torso.frame = current;
        torso.backlerp = 1.0 - (frame - current as f32);
    }
    Ok(())
}

/// MVSDK installs torso-only animations on `upper_lumbar` for 1.02 clients
/// and `lower_lumbar` for 1.03/1.04 (`ci->jk2gameplay == VERSION_1_02`).
fn jk2_torso_bone(version: u8) -> &'static str {
    if version == 2 { "upper_lumbar" } else { "lower_lumbar" }
}

/// JK2 `ANIM_TOGGLEBIT`; the presentation layer carries it as legs/torsoFlip.
const JK2_ANIM_TOGGLEBIT: i32 = 2048;

/// MVSDK `BG_FlippingAnim`/`BG_InDeathAnim` (the JKA lists are supersets).
/// `anim` is a presentation ID; `toggle` re-adds the JK2 toggle bit, which
/// 1.02 `BG_FlippingAnim` does not strip.
fn jk2_anim_flags(version: u8, anim: i32, toggle: bool) -> (bool, bool) {
    let Some(wire) = jka_protocol::jk2::presentation::wire_animation(anim) else {
        return (false, false);
    };
    let raw = if toggle { wire | JK2_ANIM_TOGGLEBIT } else { wire };
    jka_movement::jk2_animation_flags(version, raw)
}

/// MVSDK `CG_RunLerpFrame`/`CG_SetLerpFrameAnimation` for one player.
struct Jk2Lerp<'a> {
    version: u8,
    animator: &'a mut Ghoul2Animator,
    ci_legs_anim: &'a mut i32,
    ci_torso_anim: &'a mut i32,
    entity: &'a EntityState,
    animations: &'a AnimationSet,
    gla: &'a GlaAnimation,
    current_time: i32,
    anim_blend: bool,
}

impl Jk2Lerp<'_> {
    fn torso_bone(&self) -> &'static str {
        jk2_torso_bone(self.version)
    }

    /// `CG_RunLerpFrame`. The animation is (re)installed on a new number or
    /// toggle, or by `CG_FirstAnimFrame`: a speed change only takes effect
    /// once the current animation is back on its first frame. Returns the
    /// legs frame pair `CG_FootStep` tests at this advance.
    fn run(
        &mut self,
        lf: &mut LerpFrameState,
        new_animation: i32,
        flip: bool,
        speed_scale: f32,
        torso_only: bool,
        legs_frame: i32,
    ) -> Result<Option<LegsFrameStep>, String> {
        let now = self.current_time;
        if !lf.has_animation {
            // CG_ResetPlayerEntity -> CG_ClearLerpFrame (speed 1).
            lf.frame_time = now;
            lf.old_frame_time = now;
            lf.last_flip = flip;
            self.set(lf, new_animation, 1.0, torso_only, legs_frame)?;
            let first = lf.jk2_animation.map_or(0, |anim| i32::from(anim.first_frame));
            lf.old_frame = first;
            lf.frame = first;
        }
        let force_frame = field_i32(self.entity, "forceFrame");
        if force_frame != 0 {
            if lf.last_forced_frame != force_frame {
                // MVSDK: BONE_ANIM_OVERRIDE_FREEZE only (no blend), 150 ms.
                for bone in [self.torso_bone(), "model_root", "Motion"] {
                    self.animator.set_bone_anim(
                        self.gla, bone, force_frame, force_frame + 1,
                        BONE_ANIM_OVERRIDE_FREEZE, 1.0, now, None, 150,
                    )?;
                }
            }
            lf.last_forced_frame = force_frame;
            lf.animation_number = 0;
        } else {
            lf.last_forced_frame = -1;
            // CG_FirstAnimFrame.
            let first_anim_frame = {
                let same_speed = if torso_only {
                    lf.animation_torso_speed == speed_scale
                } else {
                    lf.animation_speed == speed_scale
                };
                !same_speed && lf.jk2_animation.is_some_and(|anim| {
                    anim.num_frames < 2 || i32::from(anim.first_frame) == lf.frame
                })
            };
            if new_animation != lf.animation_number
                || lf.last_flip != flip
                || lf.jk2_animation.is_none()
                || first_anim_frame
            {
                self.set(lf, new_animation, speed_scale, torso_only, legs_frame)?;
            }
        }
        lf.last_flip = flip;

        let mut step = None;
        if now >= lf.frame_time {
            // CG_FootStep tests the pair from the *previous* advance.
            if !torso_only && lf.old_frame != lf.frame {
                step = Some(LegsFrameStep {
                    old_frame: lf.old_frame,
                    frame: lf.frame,
                    anim: lf.animation_number,
                });
            }
            lf.old_frame = lf.frame;
            lf.old_frame_time = lf.frame_time;
            let Some(anim) = lf.jk2_animation.filter(|anim| anim.frame_lerp != 0) else {
                return Ok(step); // "shouldn't happen": MVSDK returns before the clamps
            };
            let frame_lerp = i32::from(anim.frame_lerp);
            lf.frame_time = if now < lf.animation_time {
                lf.animation_time // initial lerp
            } else {
                lf.old_frame_time + frame_lerp
            };
            // C int arithmetic: truncating division, then `f *= speedScale`.
            let mut f = (lf.frame_time - lf.animation_time) / frame_lerp;
            f = (f as f32 * speed_scale) as i32;
            let num_frames = i32::from(anim.num_frames);
            if f >= num_frames {
                f -= num_frames;
                if anim.loop_frames != -1 {
                    let loop_frames = i32::from(anim.loop_frames);
                    let span = if num_frames - loop_frames == 0 {
                        num_frames
                    } else {
                        num_frames - loop_frames
                    };
                    f = f.checked_rem(span).unwrap_or(0) + loop_frames;
                } else {
                    f = num_frames - 1;
                    // Stuck at the end: can transition immediately.
                    lf.frame_time = now;
                }
            }
            lf.frame = i32::from(anim.first_frame) + f;
            if now > lf.frame_time {
                lf.frame_time = now;
            }
        }
        if lf.frame_time > now + 200 {
            lf.frame_time = now;
        }
        if lf.old_frame_time > now {
            lf.old_frame_time = now;
        }
        lf.backlerp = if lf.frame_time == lf.old_frame_time {
            0.0
        } else {
            1.0 - (now - lf.old_frame_time) as f32 / (lf.frame_time - lf.old_frame_time) as f32
        };
        Ok(step)
    }

    /// `CG_SetLerpFrameAnimation` (non-ATST humanoid branch).
    fn set(
        &mut self,
        lf: &mut LerpFrameState,
        new_animation: i32,
        anim_speed_mult: f32,
        torso_only: bool,
        legs_frame: i32,
    ) -> Result<(), String> {
        let version = self.version;
        let old_anim = lf.animation_number;
        // lf->animationNumber kept the toggle bit of the previous install.
        let old_toggle = lf.last_flip;
        lf.animation_number = new_animation;
        let anim = *self.animations
            .get(new_animation)
            .ok_or_else(|| format!("Bad animation number: {new_animation}"))?;
        lf.has_animation = true;
        lf.jk2_animation = Some(anim);
        // initialLerp = ceil(1000 / |fps|) = |frameLerp|.
        lf.animation_time = lf.frame_time + i32::from(anim.frame_lerp).abs();
        if anim.frame_lerp == 0 {
            return Err(format!("animation {new_animation} has zero frameLerp"));
        }

        let mut anim_speed = 50.0 / f32::from(anim.frame_lerp);
        let mut flags = if anim.loop_frames != -1 {
            BONE_ANIM_OVERRIDE_LOOP
        } else {
            BONE_ANIM_OVERRIDE_FREEZE
        };
        if self.anim_blend {
            flags |= BONE_ANIM_BLEND;
        }
        // newAnimation was masked before these tests; oldAnim was not.
        let (new_flip, new_death) = jk2_anim_flags(version, new_animation, false);
        let (old_flip, old_death) = if old_anim != -1 {
            jk2_anim_flags(version, old_anim, old_toggle)
        } else {
            (false, false)
        };
        if new_death || old_death {
            flags &= !BONE_ANIM_BLEND;
        }
        let blend_time = if flags & BONE_ANIM_BLEND != 0 && (new_flip || old_flip) { 200 } else { 150 };

        anim_speed *= anim_speed_mult;
        // 1.02 skips BG_SaberStartTransAnim.
        if version != 2 && is_saber_transition_animation(new_animation) {
            match field_i32(self.entity, "fireflag") {
                1 => anim_speed *= 1.5,  // FORCE_LEVEL_1
                3 => anim_speed *= 0.75, // FORCE_LEVEL_3
                _ => {}
            }
        }
        if torso_only {
            lf.animation_torso_speed = anim_speed_mult;
        } else {
            lf.animation_speed = anim_speed_mult;
        }

        // MVSDK never swaps the range for a negative frameLerp (OpenJK does);
        // JK2 Ghoul2 then holds a reversed FREEZE animation on endFrame + 1.
        // Only keep that where the held frame exists in this GLA.
        let first = i32::from(anim.first_frame);
        let last = first + i32::from(anim.num_frames);
        let (start, end) = if anim_speed < 0.0 && last + 1 >= self.gla.num_frames as i32 {
            (last, first)
        } else {
            (first, last)
        };

        let torso_anim = field_i32(self.entity, "torsoAnim");
        let mut begin_frame = start;
        if torso_only {
            // cent->pe.legs.frame: the legs lerp frame advanced just before.
            let legs_frame = (version != 2
                && torso_anim == field_i32(self.entity, "legsAnim")
                && legs_frame >= first
                && legs_frame <= last)
                .then_some(legs_frame);
            if let Some(legs_frame) = legs_frame {
                self.animator.set_bone_anim(
                    self.gla, "lower_lumbar", start, end, flags, anim_speed,
                    self.current_time, Some(legs_frame as f32), blend_time,
                )?;
                begin_frame = legs_frame;
            } else {
                self.animator.set_bone_anim(
                    self.gla, self.torso_bone(), start, end, flags, anim_speed,
                    self.current_time, None, blend_time,
                )?;
            }
            *self.ci_torso_anim = new_animation;
        } else {
            self.animator.set_bone_anim(
                self.gla, "model_root", start, end, flags, anim_speed,
                self.current_time, None, blend_time,
            )?;
            if version == 2 {
                *self.ci_torso_anim = new_animation;
            }
            *self.ci_legs_anim = new_animation;
        }

        if torso_anim == new_animation {
            // 1.02 always starts Motion at its first frame; 1.03/1.04 follow
            // the legs-synchronised torso frame.
            let set_frame = (version != 2 && begin_frame != start).then_some(begin_frame as f32);
            self.animator.set_bone_anim(
                self.gla, "Motion", start, end, flags, anim_speed,
                self.current_time, set_frame, blend_time,
            )?;
        }
        Ok(())
    }
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

    /// The two-frame TIE fighter GLA from `oktry2.dm_26` must not be treated
    /// as a humanoid with an unconditional 0..12 root animation.
    #[test]
    fn vehicle_animation_initialization_does_not_seed_humanoid_frames() {
        let mut gla = jka_assets::ghoul2::openjk_default_gla().unwrap();
        gla.skeleton[0].name = "model_root".into();
        gla.num_frames = 2;
        assert!(PlayerAnimationState::new(&gla, 1401575).is_err());
        let vehicle = PlayerAnimationState::new_vehicle(&gla);
        assert!(!vehicle.animator.bone_anim(0).unwrap().is_animating());
        assert!(!vehicle.legs.has_animation);
    }

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
