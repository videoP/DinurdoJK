//! Vehicles.
use crate::cgame::player_presenter::{
    angles_to_axis, ghoul2_entity_radius, ghoul2_lod_for_view, ghoul2_model_scale,
    ghoul2_render_origin, ghoul2_render_transform, is_asset_pending, record_pose_eval,
    resolve_vehicle_model_request, Arc, ClientGameState, DynamicModelSurface, Ghoul2Animator,
    Ghoul2PresentationView, Instant, PlayerPresenter, PresentedEntity, VehicleSnap,
    ViewerAnimDebug,
};

impl PlayerPresenter {
    /// Minimal OpenJK-compatible vehicle presentation while the full vehicle
    /// animation/effects path is still being ported. Vehicle NPCs may send a
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
        let vehicle_name = requested.strip_prefix('$').map(str::to_ascii_lowercase);
        let lod = if let Some(view) = view {
            if self.early_frustum_cull {
                self.perf.frustum_tests = self.perf.frustum_tests.saturating_add(1);
                let radius = ghoul2_entity_radius(entity) * ghoul2_model_scale(entity);
                if view.sphere_outside(render_origin, radius) {
                    self.perf.frustum_culled = self.perf.frustum_culled.saturating_add(1);
                    if !self.rt_shadow_casters_enabled {
                        return Ok(Vec::new());
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

        // With no bone animation commands set, Ghoul2Animator evaluates the
        // registered model in its base/static pose using OpenJK's Ghoul2 root
        // matrix. This is intentionally the minimum safe vehicle path;
        // vehicle-specific animation comes later.
        let pose_started = Instant::now();
        let pose =
            Ghoul2Animator::new(&model.gla).evaluate_pose_openjk_root(&model.gla, current_time)?;
        record_pose_eval(&mut self.perf, pose_started);
        let axis = angles_to_axis(entity.angles);
        let (axis, origin) = ghoul2_render_transform(entity, axis);
        if let Some(vehicle) = vehicle_name {
            self.vehicle_snaps.insert(
                entity.number,
                VehicleSnap {
                    vehicle,
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

    /// See `viewer_foot_bolts`; `None` until the viewer's model has been posed.
    pub fn viewer_foot_bolts(&self) -> Option<[[f32; 3]; 2]> {
        self.viewer_foot_bolts
    }

    /// The viewer's animation input and lerp frames from its last posed frame.
    pub fn viewer_anim_debug(&self) -> Option<ViewerAnimDebug> {
        self.viewer_anim_debug
    }
}
