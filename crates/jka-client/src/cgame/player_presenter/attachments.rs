//! Attachments.
use crate::cgame::player_presenter::{
    model_bolt_matrix_timed, normalize_vec3, record_pose_eval, transform_jka_model_point,
    transform_jka_model_vector, weapon_world_model, DynamicModelSurface, Ghoul2Animator, Instant,
    Matrix3x4, PlayerModelAsset, PlayerPresenter, PresentedEntity, WP_BRYAR_PISTOL, WP_SABER,
};

/// One cosmetic MD3 (`CG_DrawCosmeticOnPlayer`): a hat or cape bolted to a player.
#[derive(Clone, Debug, PartialEq)]
pub struct CosmeticDraw {
    pub entity_num: u16,
    pub model: &'static str,
    pub origin: [f32; 3],
    pub axis: [[f32; 3]; 3],
    pub rgba: [f32; 4],
    /// The parent's customShader (ghost racers carry their cosmetics with them).
    pub custom_shader: Option<&'static str>,
}

/// Force-power visuals CG_Player hands to the FX system / local entities.
#[derive(Clone, Debug, PartialEq)]
pub enum PlayerFxRequest {
    /// FX_PlayEntityEffectID at a hand bolt (lightning, drain).
    Effect {
        name: &'static str,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
    },
    /// An authored effect (by name) played along `dir` (vehicle muzzle flashes).
    EffectDir {
        name: String,
        origin: [f32; 3],
        dir: [f32; 3],
    },
    /// CG_DoSaber blade presentation. The FX layer owns the view-facing geometry
    /// so authored saber shaders go through the same material path as other FX.
    SaberBlade {
        origin: [f32; 3],
        direction: [f32; 3],
        length: f32,
        /// Authored `lengthMax`; CG_DoSaber widens the blade while `length`
        /// is still below it (ignition / retraction halo).
        length_max: f32,
        radius: f32,
        color: i32,
        entity_alpha: f32,
        /// Identity/state needed by OpenJK CG_AddSaberBlade's persistent
        /// per-blade trail history.
        entity_num: u16,
        saber_num: u8,
        blade_num: u8,
        saber_move: i32,
        torso_anim: i32,
        saber_in_flight: bool,
        trail_style: i32,
        /// Number of authored blades on this saber. OpenJK uses one combined
        /// dynamic light for sabers with 3+ blades instead of one per blade.
        num_blades: u8,
        /// Authored OpenJK `noDlight` / SFL2_NO_DLIGHT.
        no_dlight: bool,
        /// Authored OpenJK noWallMarks/noWallMarks2 for the active blade style.
        no_wall_marks: bool,
    },
    /// CG_ForcePushBlur's LE_PUFF path (two sprites drifting sideways).
    PushPuffs { origin: [f32; 3] },
    /// CG_ForceGripEffect (a red puff and a red saber-glow puff).
    GripPuffs { origin: [f32; 3] },
    /// CG_PlayerFloatSprite: an icon above a player's head (chat balloon, voice
    /// chat, connection trouble), white and 10 units in radius.
    HeadSprite {
        origin: [f32; 3],
        shader: &'static str,
    },
}

pub(in crate::cgame::player_presenter) const EF_BODYPUSH: i32 = 1 << 19;

pub(in crate::cgame::player_presenter) const EF_JETPACK_ACTIVE: i32 = 1 << 11;

pub(in crate::cgame::player_presenter) const EF_JETPACK: i32 = 1 << 29;

pub(in crate::cgame::player_presenter) const EF_JETPACK_FLAMING: i32 = 1 << 30;

pub(in crate::cgame::player_presenter) const JAPRO_CINFO2_WTTRIBES: i32 = 1 << 4;

pub(in crate::cgame::player_presenter) const JETPACK_MODEL: &str =
    "models/weapons2/jetpack/model.glm";

pub(in crate::cgame::player_presenter) const PW_DISINT_4: i32 = 9;

pub(in crate::cgame::player_presenter) const FP_GRIP: i32 = 6;

pub(in crate::cgame::player_presenter) const MAX_GRIP_DISTANCE: f32 = 256.0;

pub(in crate::cgame::player_presenter) const DEFAULT_VIEWHEIGHT: f32 = 26.0;

pub(in crate::cgame::player_presenter) const DEFAULT_PLAYER_MINS: [f32; 3] = [-15.0, -15.0, -24.0];

pub(in crate::cgame::player_presenter) const DEFAULT_PLAYER_MAXS: [f32; 3] = [15.0, 15.0, 40.0];

pub(in crate::cgame::player_presenter) const FP_RAGE: i32 = 8;

pub(in crate::cgame::player_presenter) const FP_PROTECT: i32 = 9;

pub(in crate::cgame::player_presenter) const FORCE_LEVEL_2: i32 = 2;

pub(in crate::cgame::player_presenter) const FORCE_LEVEL_3: i32 = 3;

/// cg_pushBoneNames for CG_ForcePushBodyBlur.
pub(in crate::cgame::player_presenter) const PUSH_BONE_NAMES: [&str; 8] = [
    "cranium",
    "lower_lumbar",
    "rhand",
    "lhand",
    "ltibia",
    "rtibia",
    "lradius",
    "rradius",
];

pub(in crate::cgame::player_presenter) const MAX_CLIENTS: usize = 32;

pub(in crate::cgame::player_presenter) const MAX_SABER_BLADES: usize = 8;

/// WP_SaberSetDefaults: `blade[].lengthMax` before a definition overrides it.
pub(in crate::cgame::player_presenter) const DEFAULT_SABER_BLADE_LENGTH_MAX: f32 = 32.0;

/// weapon_t values used by the held-weapon attachment rules.
pub(in crate::cgame::player_presenter) const WP_MELEE: i32 = 2;

pub(in crate::cgame::player_presenter) const WP_EMPLACED_GUN: i32 = 17;

pub(in crate::cgame::player_presenter) const TEAM_SPECTATOR: i32 = 3;

/// The saber-specific weapon bootstrap at the end of OpenJK's
/// CG_ResetPlayerEntity. A remote player entering the PVS (including every
/// entity in an initial demo snapshot) copies the complete WP_SABER Ghoul2
/// weapon instance before CG_Player applies saberInFlight. The copy itself
/// runs CG_CopyG2WeaponInstance, which places saber 0 at model index 1 and
/// saber 1 at model index 2. This is deliberately separate from the normal
/// per-frame weapon-pointer comparison below: ghoul2weapon identifies the
/// primary weapon instance and is not evidence that model index 2 was copied.
#[allow(clippy::too_many_arguments)]
pub(in crate::cgame::player_presenter) fn reset_remote_player_saber_attachment(
    ghoul2_weapon: &mut Option<i32>,
    cent_weapon: &mut i32,
    attached_weapon: &mut Option<i32>,
    primary_saber_attached: &mut bool,
    secondary_saber_attached: &mut bool,
    requested_weapon: i32,
    remote_from_predicted_player: bool,
    has_primary_saber: bool,
    has_secondary_saber: bool,
) {
    // cg_players.c::CG_ResetPlayerEntity:
    //   currentState.number != predictedPlayerState.clientNum
    //   && currentState.weapon == WP_SABER
    //   && cent->weapon != currentState.weapon
    if !remote_from_predicted_player
        || requested_weapon != WP_SABER
        || *cent_weapon == requested_weapon
    {
        return;
    }

    *cent_weapon = requested_weapon;

    // CG_CopyG2WeaponInstance(WP_SABER) copies both configured saber slots.
    // In this Rust presenter these booleans are the actual model-index state.
    *attached_weapon = None;
    *primary_saber_attached = has_primary_saber;
    *secondary_saber_attached = has_secondary_saber;
    *ghoul2_weapon = Some(WP_SABER);
}

/// OpenJK CG_Player + CG_CopyG2WeaponInstance. `ghoul2_weapon` is the last
/// g2WeaponInstances pointer identity, `cent_weapon` is the persistent centity
/// weapon identity, and the three model-slot arguments are the actual Ghoul2
/// attachment state. Death clears only ghoul2weapon; it does not
/// manufacture/remove held models.
#[allow(clippy::too_many_arguments)]
pub(in crate::cgame::player_presenter) fn update_weapon_attachment(
    ghoul2_weapon: &mut Option<i32>,
    cent_weapon: &mut i32,
    attached_weapon: &mut Option<i32>,
    primary_saber_attached: &mut bool,
    secondary_saber_attached: &mut bool,
    requested_weapon: i32,
    instance: Option<i32>,
    saber_in_flight: bool,
    dead: bool,
    spectator: bool,
    has_primary_saber: bool,
    has_secondary_saber: bool,
) {
    // CG_Player recomputes whether Ghoul2 model index 1 actually exists every
    // frame. If it does not, OpenJK forcibly clears both tracking fields so a
    // weapon can be recopied later (cg_players.c: g2HasWeapon check immediately
    // before the weapon update block). This is essential for saber return: the
    // in-flight path removes model index 1 while leaving the logical saber
    // instance selected, and the missing-slot check is what makes the catch
    // reattach it on the next non-flight frame.
    let g2_has_weapon = attached_weapon.is_some() || *primary_saber_attached;
    if !g2_has_weapon {
        *ghoul2_weapon = None;
        *cent_weapon = 0;
    }

    // CG_Player does this after the actual model-slot test and before its
    // death/spectator tests so an in-flight primary saber cannot be
    // copied back into the hand until the server says it has returned.
    if saber_in_flight {
        *ghoul2_weapon = Some(WP_SABER);
    }

    if dead {
        // CG_Player / CG_CheckPlayerG2Weapons: "no updating weapons when dead". The
        // copied models stay exactly as they were until an explicit event or
        // body-copy operation changes them.
        *ghoul2_weapon = None;
        return;
    }

    if spectator {
        // OpenJK resets the tracking pointers for spectators, but does not
        // issue a Ghoul2 RemoveGhoul2Model here. Preserve the actual slots.
        *ghoul2_weapon = None;
        *cent_weapon = 0;
        return;
    }

    if *ghoul2_weapon == instance {
        return;
    }

    match instance {
        Some(WP_SABER) => {
            // CG_CopyG2WeaponInstance copies saber 0 to model index 1 and, for
            // dual sabers, saber 1 to model index 2.
            *attached_weapon = None;
            *primary_saber_attached = has_primary_saber;
            *secondary_saber_attached = has_secondary_saber;
        }
        Some(WP_MELEE | WP_EMPLACED_GUN) => {
            // These weapon instances deliberately remove held Ghoul2 models.
            *attached_weapon = None;
            *primary_saber_attached = false;
            *secondary_saber_attached = false;
        }
        Some(other) => {
            *attached_weapon = Some(other);
            *primary_saber_attached = false;
            *secondary_saber_attached = false;
        }
        // No g2WeaponInstances entry means CG_CopyG2WeaponInstance does
        // nothing; the previous actual model slots remain bolted on.
        None => {}
    }

    *cent_weapon = requested_weapon;
    *ghoul2_weapon = instance;
}

/// OpenJK CG_BodyQueueCopy first duplicates the source Ghoul2 instance and
/// only then applies this correction to model index 1. Crucially, a missing
/// source model is never manufactured from knownWeapon.
pub(in crate::cgame::player_presenter) fn body_queue_model1_weapon(
    source_model1: Option<i32>,
    known_weapon: i32,
) -> Option<i32> {
    let source_model1 = source_model1?;
    if known_weapon > WP_BRYAR_PISTOL {
        return None;
    }
    if known_weapon == WP_SABER {
        return Some(WP_SABER);
    }
    // CG_G2WeaponInstance must exist for CopySpecificGhoul2Model to replace
    // model index 1. If it does not, retain the duplicated source slot.
    weapon_world_model(known_weapon).map_or(Some(source_model1), |_| Some(known_weapon))
}

impl PlayerPresenter {
    /// TaystJK/OpenJK CG_Player jetpack model-slot-3 presentation.
    ///
    /// The legacy renderer copies a preloaded Ghoul2 jetpack instance into
    /// model index 3, whose parent bolt is player bolt 2 (`*chestg`). This
    /// renderer has no mutable multi-model Ghoul2 instance, so the equivalent
    /// is a child GLM pose rooted directly at the chest bolt.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::cgame::player_presenter) fn append_jetpack(
        &mut self,
        draws: &mut Vec<DynamicModelSurface>,
        entity: &PresentedEntity,
        player_model: &PlayerModelAsset,
        player_pose: &[Matrix3x4],
        player_axis: [[f32; 3]; 3],
        player_origin: [f32; 3],
        current_time: i32,
        entity_alpha: f32,
    ) -> Result<(), String> {
        let Some(chest_bolt) = model_bolt_matrix_timed(
            &mut self.perf,
            &player_model.glm,
            &player_model.gla,
            player_pose,
            "*chestg",
        )?
        else {
            return Err(format!("{} has no Ghoul2 *chestg bolt", player_model.key));
        };

        let jetpack = self.load_static_glm_in_game(JETPACK_MODEL, None, JETPACK_MODEL)?;
        let pose_started = Instant::now();
        let jetpack_pose = Ghoul2Animator::new(&jetpack.gla).evaluate_pose(
            &jetpack.gla,
            current_time,
            chest_bolt,
        )?;
        record_pose_eval(&mut self.perf, pose_started);

        let mut jetpack_draws = self.render_glm_surfaces(
            entity.number,
            JETPACK_MODEL,
            &jetpack.glm,
            &jetpack.gla,
            &jetpack.surfaces,
            None,
            &jetpack_pose,
            0,
            player_axis,
            player_origin,
            [1.0, 1.0, 1.0, entity_alpha],
            None,
            true,
        )?;
        draws.append(&mut jetpack_draws);

        let e_flags = entity.state.field_i32("eFlags").unwrap_or(0);
        if e_flags & EF_JETPACK_ACTIVE == 0 {
            return Ok(());
        }

        if self.japro_cinfo2 & JAPRO_CINFO2_WTTRIBES != 0 {
            // TaystJK's WTTRIBES branch does not use the model nozzle bolts:
            // two vertical effects are placed around lerpOrigin using
            // AngleVectors(turAngles).
            // cent->turAngles is the legs angle output of BG_G2PlayerAngles;
            // player_axis is the same AnglesToAxis result (possibly uniformly
            // model-scaled), so normalize its basis back to directions.
            let forward = normalize_vec3(player_axis[0]);
            // AnglesToAxis stores model-left in axis[1]; AngleVectors returns
            // right, so negate it to match the C code.
            let model_left = normalize_vec3(player_axis[1]);
            let right = [-model_left[0], -model_left[1], -model_left[2]];
            let mut flame_pos = entity.origin;
            for i in 0..3 {
                flame_pos[i] -= 6.0 * forward[i];
            }
            flame_pos[2] += 22.0;
            for i in 0..3 {
                flame_pos[i] -= 6.0 * right[i];
            }
            self.fx_requests.push(PlayerFxRequest::EffectDir {
                name: "effects/tribes/jet.efx".to_owned(),
                origin: flame_pos,
                dir: [0.0, 0.0, 1.0],
            });
            for i in 0..3 {
                flame_pos[i] += 10.0 * right[i];
            }
            self.fx_requests.push(PlayerFxRequest::EffectDir {
                name: "effects/tribes/jet.efx".to_owned(),
                origin: flame_pos,
                dir: [0.0, 0.0, 1.0],
            });
            return Ok(());
        }

        // G2API_GetBoltMatrix applies the legacy Ghoul2 270-degree yaw fixup
        // before BG_GiveMeVectorFromMatrix sees the matrix. Our helper exposes
        // the lower/raw matrix, so reconstruct the same public X/Y axes here:
        // public +X = -raw column 1, public +Y = raw column 0.
        for (index, tag) in ["torso_ljet", "torso_rjet"].into_iter().enumerate() {
            let Some(matrix) = model_bolt_matrix_timed(
                &mut self.perf,
                &jetpack.glm,
                &jetpack.gla,
                &jetpack_pose,
                tag,
            )?
            else {
                continue;
            };
            let mut origin = transform_jka_model_point(
                [matrix[0][3], matrix[1][3], matrix[2][3]],
                player_axis,
                player_origin,
            );
            let positive_x = normalize_vec3(transform_jka_model_vector(
                [-matrix[0][1], -matrix[1][1], -matrix[2][1]],
                player_axis,
            ));
            let negative_y = normalize_vec3(transform_jka_model_vector(
                [-matrix[0][0], -matrix[1][0], -matrix[2][0]],
                player_axis,
            ));
            let (first_dir, final_dir) = if index == 0 {
                (negative_y, positive_x)
            } else {
                (positive_x, negative_y)
            };
            for i in 0..3 {
                origin[i] -= 9.5 * first_dir[i];
                origin[i] -= 13.5 * final_dir[i];
            }
            let request = || PlayerFxRequest::EffectDir {
                name: "effects/boba/jet.efx".to_owned(),
                origin,
                dir: final_dir,
            };
            self.fx_requests.push(request());
            // EF_JETPACK_FLAMING deliberately submits the same authored EFX
            // twice, matching TaystJK's FIXME-era behavior exactly.
            if e_flags & EF_JETPACK_FLAMING != 0 {
                self.fx_requests.push(request());
            }
        }
        Ok(())
    }

    /// Ghoul2 model index 1: the weapon's world GLM bolted to the player's
    /// right hand (G2API_SetBoltInfo(instance, 0, 0) -> bolt 0 = *r_hand).
    #[allow(clippy::too_many_arguments)]
    pub(in crate::cgame::player_presenter) fn append_held_weapon(
        &mut self,
        draws: &mut Vec<DynamicModelSurface>,
        entity: &PresentedEntity,
        weapon: i32,
        player_model: &PlayerModelAsset,
        player_pose: &[Matrix3x4],
        player_axis: [[f32; 3]; 3],
        player_origin: [f32; 3],
        current_time: i32,
        entity_alpha: f32,
    ) -> Result<(), String> {
        let qpath = weapon_world_model(weapon).ok_or("weapon has no Ghoul2 world model")?;
        let Some(hand_bolt) = model_bolt_matrix_timed(
            &mut self.perf,
            &player_model.glm,
            &player_model.gla,
            player_pose,
            "*r_hand",
        )?
        else {
            return Err(format!("{} has no Ghoul2 *r_hand bolt", player_model.key));
        };
        let weapon_model = self.load_static_glm_in_game(&qpath, None, &qpath)?;
        let pose_started = Instant::now();
        let pose = Ghoul2Animator::new(&weapon_model.gla).evaluate_pose(
            &weapon_model.gla,
            current_time,
            hand_bolt,
        )?;
        record_pose_eval(&mut self.perf, pose_started);
        let mut weapon_draws = self.render_glm_surfaces(
            entity.number,
            &qpath,
            &weapon_model.glm,
            &weapon_model.gla,
            &weapon_model.surfaces,
            None,
            &pose,
            0,
            player_axis,
            player_origin,
            [1.0, 1.0, 1.0, entity_alpha],
            None,
            true,
        )?;
        draws.append(&mut weapon_draws);
        Ok(())
    }
}
