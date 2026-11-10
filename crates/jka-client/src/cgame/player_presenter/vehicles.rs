//! Vehicles.
use crate::cgame::player_presenter::{
    angles_to_axis, ghoul2_entity_radius, ghoul2_lod_for_view, ghoul2_model_scale,
    ghoul2_render_origin, ghoul2_render_transform, is_asset_pending, record_pose_eval,
    resolve_vehicle_model_request, model_bolt_matrix_timed, transform_jka_model_point,
    Arc, ClientGameState, DynamicModelSurface, Ghoul2Animator,
    Ghoul2PresentationView, Instant, PlayerPresenter, PresentedEntity, VehicleSnap,
    ViewerAnimDebug,
};

use crate::cgame::player_animation::PlayerAnimationState;
use jka_assets::animation::AnimationSet;
use jka_assets::ghoul2::GlaAnimation;

// PlayerAnimationState::update uses the clientInfo fields only for saber
// transition overrides. CLASS_VEHICLE model_root animations do not have any;
// keep the inert value initialized once instead of allocating model strings
// for every mounted animal on every rendered frame.
static VEHICLE_ANIMATION_INFO: std::sync::OnceLock<crate::cgame::ClientInfo> =
    std::sync::OnceLock::new();

pub(in crate::cgame::player_presenter) struct VehicleAnimationRuntime {
    model_request: String,
    animation: PlayerAnimationState,
    // Print an incompatible animation once per state change, not every frame.
    last_rejected_animation: Option<String>,
}

/// Ghoul2's valid frame indices come from the actual referenced GLA, not from
/// humanoid animation.cfg or the network animation index. TaystJK's vehicle
/// model still draws when `G2_SetBoneAnim` cannot install an invalid override.
/// Check *before* mutating the live lerp state so one bad entry cannot stop
/// the vehicle, muzzle attachments and pilot attachment from rendering.
fn incompatible_vehicle_animation(
    animations: &AnimationSet,
    gla: &GlaAnimation,
    legs_anim: i32,
    force_frame: i32,
) -> Option<String> {
    if force_frame != 0 {
        if force_frame < 0 || (force_frame as usize) >= gla.num_frames {
            return Some(format!("forceFrame={force_frame} outside GLA frames={}", gla.num_frames));
        }
        return None;
    }
    let Some(anim) = animations.get(legs_anim) else {
        return Some(format!("legsAnim={legs_anim} absent from animation table"));
    };
    // Non-humanoid 0/0 entries are intentional no-ops in CG_SetLerpFrameAnimation.
    if anim.first_frame == 0 && anim.num_frames == 0 {
        return None;
    }
    let first = usize::from(anim.first_frame);
    let end = first + usize::from(anim.num_frames);
    if end > gla.num_frames {
        return Some(format!(
            "legsAnim={legs_anim} range={first}..{end} outside GLA frames={}",
            gla.num_frames
        ));
    }
    if Ghoul2Animator::bone_index(gla, "model_root").is_none() {
        return Some(format!("legsAnim={legs_anim}: model_root bone missing"));
    }
    None
}

/// TaystJK `CG_G2PlayerAngles` (cg_players.c) for non-humanoid Ghoul2.
///
/// The network lerpAngles are NOT the model's world rotation for most
/// vehicles: only fighters use full pitch/yaw/roll; speeders and walkers
/// preserve roll but discard pitch; animals and all other types use yaw only.
/// This applies just as much to unoccupied vehicles as to mounted ones.
fn vehicle_render_angles(lerp_angles: [f32; 3], vehicle_type: Option<&str>) -> [f32; 3] {
    match vehicle_type {
        Some(kind) if kind.eq_ignore_ascii_case("VH_FIGHTER") => lerp_angles,
        Some(kind) if kind.eq_ignore_ascii_case("VH_SPEEDER")
            || kind.eq_ignore_ascii_case("VH_WALKER") =>
        {
            [0.0, lerp_angles[1], lerp_angles[2]]
        }
        _ => [0.0, lerp_angles[1], 0.0],
    }
}

/// Exact TaystJK `BG_G2ATSTAngles` (bg_pmove.c): walkers retain yaw/roll
/// on the model axis, while the incoming pitch is applied to `thoracic`.
/// `CG_G2PlayerAngles` calls this before reconstructing the Ghoul2 pose.
///
/// G2_SetBoneAngles returns false when an optional bone is absent. Mirror
/// that behavior rather than rejecting custom walker GLAs without `thoracic`.
fn apply_walker_thoracic_pitch(
    animator: &mut Ghoul2Animator,
    gla: &GlaAnimation,
    lerp_angles: [f32; 3],
) -> Result<(), String> {
    if Ghoul2Animator::bone_index(gla, "thoracic").is_some() {
        animator.set_bone_angles_postmult(
            gla,
            "thoracic",
            [lerp_angles[0], 0.0, 0.0],
            jka_movement::G2_POSITIVE_X,
            jka_movement::G2_NEGATIVE_Y,
            jka_movement::G2_NEGATIVE_Z,
        )?;
    }
    Ok(())
}

impl PlayerPresenter {
    /// OpenJK-compatible Ghoul2 vehicle and animated model_root presentation.
    /// Non-animation vehicle FX and prediction remain separate systems. Vehicle NPCs may send a
    /// `$name` through CS_MODELS/modelindex; resolve that through the winning
    /// ext_data/vehicles definition before registering the Ghoul2 model/skin.
    /// Genuine registration failures still use stock swoop as a visible fallback.
    pub(in crate::cgame::player_presenter) fn present_vehicle_static(
        &mut self,
        entity: &PresentedEntity,
        game: &ClientGameState,
        current_time: i32,
        view: Option<Ghoul2PresentationView>,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let model_index = entity.state.field_i32("modelindex").unwrap_or(0);
        let requested = game
            .model_qpath(model_index)
            .ok_or_else(|| format!("vehicle modelindex {model_index} has no CS_MODELS entry"))?;
        let resolved = resolve_vehicle_model_request(&self.vehicle_definitions, &requested);
        let model = match resolved {
            Ok((requested_glm, requested_skin, requested_label)) => {
                match self.load_static_glm_in_game(
                    &requested_glm,
                    requested_skin.as_deref(),
                    &requested_label,
                ) {
                    Ok(model) => model,
                    Err(pending) if is_asset_pending(&pending) => return Ok(Vec::new()),
                    Err(primary_error) => self.vehicle_fallback(
                        entity.number,
                        &requested,
                        Some(&requested_glm),
                        requested_skin.as_deref(),
                        &primary_error,
                    )?,
                }
            }
            Err(primary_error) => {
                self.vehicle_fallback(entity.number, &requested, None, None, &primary_error)?
            }
        };

        let render_origin = ghoul2_render_origin(entity);
        let mut raster_visible = true;
        // An optional `*skin` suffix is not part of the `.veh` lookup name.
        let vehicle_name = requested.split('*').next()
            .and_then(|name| name.strip_prefix('$'))
            .map(str::to_ascii_lowercase);
        let lod = if let Some(view) = view {
            if self.early_frustum_cull {
                self.perf.frustum_tests = self.perf.frustum_tests.saturating_add(1);
                let radius = ghoul2_entity_radius(entity) * ghoul2_model_scale(entity);
                if view.sphere_outside(render_origin, radius) {
                    self.perf.frustum_culled = self.perf.frustum_culled.saturating_add(1);
                    if !self.rt_shadow_casters_enabled {
                        // A visible rider can extend outside the vehicle's
                        // sphere. Its *driver bolt still needs the current pose.
                        let owner = entity.state.field_i32("owner").unwrap_or(1023);
                        if !(0..1023).contains(&owner) {
                            return Ok(Vec::new());
                        }
                    }
                    raster_visible = false;
                }
            }
            let lod = ghoul2_lod_for_view(
                view,
                entity,
                render_origin,
                model.glm.lods.len(),
                self.lod_bias,
                self.lod_scale,
            );
            let bucket = lod.min(self.perf.lod_counts.len() - 1);
            self.perf.lod_counts[bucket] = self.perf.lod_counts[bucket].saturating_add(1);
            lod
        } else {
            self.perf.lod_counts[0] = self.perf.lod_counts[0].saturating_add(1);
            0
        };

        // TaystJK CG_G2PlayerAngles -> BG_G2ATSTAngles acts on the VEHICLE's
        // thoracic bone, not the driver's skeleton and not the model axis.
        // Look this up once before mutably borrowing the animation runtime.
        let vehicle_type = self.vehicle_definition_for_model_request(&requested)
            .and_then(|definition| definition.vehicle_type.as_deref());
        let is_walker = vehicle_type.is_some_and(|kind| kind.eq_ignore_ascii_case("VH_WALKER"));
        let render_angles = vehicle_render_angles(entity.angles, vehicle_type);

        // TaystJK CG_PlayerAnimation -> CG_RunLerpFrame uses the authoritative
        // legsAnim/legsFlip for CLASS_VEHICLE, driving *only* model_root. Reuse
        // our existing direct port of that state machine, not a guessed
        // stand/walk/run animation selected from client-side velocity.
        let pose_started = Instant::now();
        let pose = if let Some(animations) = model.animations.as_deref() {
            let needs_new = self.vehicle_animations.get(&entity.number)
                .is_none_or(|runtime| runtime.model_request != requested);
            if needs_new {
                self.vehicle_animations.insert(entity.number, VehicleAnimationRuntime {
                    model_request: requested.clone(),
                    animation: PlayerAnimationState::new_vehicle(&model.animation_gla),
                    last_rejected_animation: None,
                });
            }
            let runtime = self.vehicle_animations.get_mut(&entity.number).unwrap();
            let anim_issue = incompatible_vehicle_animation(
                animations,
                &model.animation_gla,
                entity.state.field_i32("legsAnim").unwrap_or(0),
                entity.state.field_i32("forceFrame").unwrap_or(0),
            );
            if let Some(reason) = anim_issue {
                if runtime.last_rejected_animation.as_deref() != Some(reason.as_str()) {
                    println!(
                        "VEHICLE ANIMATION SKIPPED: entity={} model={} gla={} reason={}; retaining last valid pose",
                        entity.number, requested, model.animation_gla.name, reason,
                    );
                    runtime.last_rejected_animation = Some(reason);
                }
            } else {
                runtime.last_rejected_animation = None;
                runtime.animation.update(
                    &entity.state,
                    animations,
                    &model.animation_gla,
                    VEHICLE_ANIMATION_INFO.get_or_init(crate::cgame::ClientInfo::solo_kyle),
                    &self.saber_scales,
                    current_time,
                    true,
                    false,
                    None,
                )?;
            }
            // Apply the current interpolated pitch EVERY frame, including
            // a return to zero. No stale previous-frame override survives.
            if is_walker {
                apply_walker_thoracic_pitch(
                    &mut runtime.animation.animator,
                    &model.animation_gla,
                    entity.angles,
                )?;
            }
            model.bind_animation_pose(
                runtime.animation.evaluate_pose_openjk_root(&model.animation_gla, current_time)?,
            )?
        } else {
            // A non-vehicle fallback/static GLM has no local animation table.
            // The walker pitch override is still applied if its GLA has the bone.
            let mut animator = Ghoul2Animator::new(&model.animation_gla);
            if is_walker {
                apply_walker_thoracic_pitch(&mut animator, &model.animation_gla, entity.angles)?;
            }
            model.bind_animation_pose(
                animator.evaluate_pose_openjk_root(&model.animation_gla, current_time)?,
            )?
        };
        record_pose_eval(&mut self.perf, pose_started);
        // CG_G2PlayerAngles does not put the Tauntaun's interpolated pitch
        // on its model root. Previously we fed all three angles to the world
        // axis, leaving an unoccupied Tauntaun tipped backwards on dismount.
        // Use the authored `.veh` type, never infer from the model name or
        // mounting/owner state (which changes while dismounting).
        let axis = angles_to_axis(render_angles);
        let (axis, origin) = ghoul2_render_transform(entity, axis);
        if let Some(vehicle) = vehicle_name {
            self.vehicle_snaps.insert(
                entity.number,
                VehicleSnap {
                    vehicle,
                    frame_time: current_time,
                    owner: entity.state.field_i32("owner").unwrap_or(-1),
                    yaw: entity.angles[1],
                    model: Arc::clone(&model),
                    pose: pose.clone(),
                    axis,
                    origin,
                },
            );
        }
        let mut draws = self.render_glm_surfaces(
            entity.number,
            &requested,
            &model.glm,
            &model.gla,
            &model.surfaces,
            None,
            &pose,
            lod,
            axis,
            origin,
            [1.0, 1.0, 1.0, 1.0],
            None,
            true,
        )?;
        if !raster_visible {
            for surface in &mut draws {
                surface.raster_visible = false;
            }
        }
        Ok(draws)
    }

    /// TaystJK CG_Player -> AttachRidersGeneric: the pilot is placed at the
    /// animated *driver bolt on the vehicle's Ghoul2 instance and its render
    /// yaw follows the vehicle. Only use a pose from the same presentation
    /// frame, never yesterday's cached bolt; check owner just as CG_Player does.
    pub(in crate::cgame::player_presenter) fn vehicle_attached_entity(
        &mut self,
        rider: &PresentedEntity,
        current_time: i32,
    ) -> Result<Option<PresentedEntity>, String> {
        let vehicle_num = rider.state.field_i32("m_iVehicleNum").unwrap_or(0);
        let Ok(vehicle_num) = u16::try_from(vehicle_num) else {
            return Ok(None);
        };
        if vehicle_num == 0 || rider.state.field_i32("NPC_class").unwrap_or(0) == crate::cgame::CLASS_VEHICLE {
            return Ok(None);
        }
        let Some(vehicle) = self.vehicle_snaps.get(&vehicle_num) else {
            return Ok(None);
        };
        let client_num = rider.state.field_i32("clientNum").unwrap_or(i32::from(rider.number));
        if vehicle.frame_time != current_time {
            return Ok(None);
        }
        let mut result = rider.clone();
        // CG_Player copies vehicle lerpAngles[YAW] even when the actual
        // rider attachment cannot run (droid, passenger, predicted local).
        result.angles[1] = vehicle.yaw;
        // TaystJK intentionally does not re-bolt the locally predicted player;
        // BG prediction supplies that position. Non-pilots take the droid/
        // passenger path, which cannot safely use the pilot's *driver bolt.
        if vehicle.owner != client_num || client_num == self.viewer_client {
            return Ok(Some(result));
        }
        if let Some(bolt) = model_bolt_matrix_timed(
            &mut self.perf, &vehicle.model.glm, &vehicle.model.gla,
            &vehicle.pose, "*driver",
        )? {
            result.origin = transform_jka_model_point(
                [bolt[0][3], bolt[1][3], bolt[2][3]],
                vehicle.axis, vehicle.origin,
            );
        }
        Ok(Some(result))
    }

    /// See `viewer_foot_bolts`; `None` until the viewer's model has been posed.
    pub fn viewer_foot_bolts(&self) -> Option<[[f32; 3]; 2]> {
        self.viewer_foot_bolts
    }

    /// The viewer's animation input and lerp frames from its last posed frame.
    pub fn viewer_anim_debug(&self) -> Option<ViewerAnimDebug> {
        self.viewer_anim_debug
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_walker_thoracic_pitch, incompatible_vehicle_animation, vehicle_render_angles};
    use jka_assets::animation::{Animation, AnimationSet};
    use jka_assets::ghoul2::{openjk_default_gla, Ghoul2Animator};
    use jka_assets::skeleton::{humanoid_reference, HumanoidRig};

    #[test]
    fn two_frame_vehicle_skips_only_out_of_bounds_animation_entries() {
        let mut gla = openjk_default_gla().unwrap();
        gla.skeleton[0].name = "model_root".into();
        gla.num_frames = 2;
        let mut animations = AnimationSet { animations: vec![Animation::default(); 4] };
        assert!(incompatible_vehicle_animation(&animations, &gla, 0, 0).is_none());
        animations.animations[1] = Animation {
            first_frame: 0, num_frames: 2, frame_lerp: 100, loop_frames: -1,
        };
        assert!(incompatible_vehicle_animation(&animations, &gla, 1, 0).is_none());
        animations.animations[2] = Animation {
            first_frame: 1, num_frames: 12, frame_lerp: 100, loop_frames: -1,
        };
        assert!(incompatible_vehicle_animation(&animations, &gla, 2, 0).unwrap().contains("outside GLA"));
        assert!(incompatible_vehicle_animation(&animations, &gla, 5000, 0).is_some());
        assert!(incompatible_vehicle_animation(&animations, &gla, 1, 2).unwrap().contains("forceFrame"));
    }

    #[test]
    fn tauntaun_animal_ignores_pilot_pitch_and_roll_after_dismount() {
        assert_eq!(
            vehicle_render_angles([47.0, 172.0, -18.0], Some("VH_ANIMAL")),
            [0.0, 172.0, 0.0],
        );
    }

    #[test]
    fn walker_pitch_applies_to_thoracic_not_model_root_and_resets_to_zero() {
        let gla = humanoid_reference(HumanoidRig::Jka53);
        let thoracic = Ghoul2Animator::bone_index(&gla, "thoracic").unwrap();
        let mut animator = Ghoul2Animator::new(&gla);
        let neutral = animator.evaluate_pose_openjk_root(&gla, 0).unwrap();
        apply_walker_thoracic_pitch(&mut animator, &gla, [32.0, 140.0, 11.0]).unwrap();
        let pitched = animator.evaluate_pose_openjk_root(&gla, 0).unwrap();
        assert_ne!(pitched[thoracic], neutral[thoracic]);
        // TaystJK BG_G2ATSTAngles supplies just pitch; yaw/roll must not
        // enter the override, even when the vehicle's world axis uses them.
        let mut pitch_only = Ghoul2Animator::new(&gla);
        apply_walker_thoracic_pitch(&mut pitch_only, &gla, [32.0, 0.0, 0.0]).unwrap();
        assert_eq!(pitch_only.evaluate_pose_openjk_root(&gla, 0).unwrap(), pitched);
        apply_walker_thoracic_pitch(&mut animator, &gla, [0.0, 140.0, 11.0]).unwrap();
        let reset = animator.evaluate_pose_openjk_root(&gla, 0).unwrap();
        // The reference zero-degree POSTMULT matrix is the neutral bone
        // transform; account for floating-point bind/inverse multiplication.
        for (a, b) in reset.iter().zip(neutral.iter()) {
            for row in 0..3 {
                for col in 0..4 {
                    assert!((a[row][col] - b[row][col]).abs() < 0.001);
                }
            }
        }
    }

    #[test]
    fn walker_pitch_silently_skips_a_model_without_thoracic() {
        let gla = jka_assets::ghoul2::openjk_default_gla().unwrap();
        let mut animator = Ghoul2Animator::new(&gla);
        apply_walker_thoracic_pitch(&mut animator, &gla, [20.0, 30.0, 0.0]).unwrap();
        animator.evaluate_pose_openjk_root(&gla, 0).unwrap();
    }

    #[test]
    fn vehicle_orientation_matches_cg_g2playerangles_by_type() {
        let angles = [33.0, 121.0, -24.0];
        assert_eq!(vehicle_render_angles(angles, Some("VH_FIGHTER")), angles);
        assert_eq!(vehicle_render_angles(angles, Some("vh_speeder")), [0.0, 121.0, -24.0]);
        assert_eq!(vehicle_render_angles(angles, Some("VH_WALKER")), [0.0, 121.0, -24.0]);
        assert_eq!(vehicle_render_angles(angles, Some("VH_ANIMAL")), [0.0, 121.0, 0.0]);
        assert_eq!(vehicle_render_angles(angles, None), [0.0, 121.0, 0.0]);
        assert_eq!(vehicle_render_angles(angles, Some("VH_UNKNOWN")), [0.0, 121.0, 0.0]);
    }
}
