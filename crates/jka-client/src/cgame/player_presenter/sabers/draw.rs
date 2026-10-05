//! Sabers draw.
use crate::cgame::player_presenter::{
    angles_to_axis, blade_angles, blade_color, is_asset_pending, model_bolt_matrix_timed,
    normalize_vec3, record_pose_eval, saber_blade_fx_request, transform_jka_model_point,
    transform_jka_model_vector, Arc, ClientGameState, DynamicModelSurface, Ghoul2Animator, Instant,
    Matrix3x4, PlayerModelAsset, PlayerPresenter, PresentedEntity, SaberBladeLengthKey,
    SaberBladeLengthState, SaberDefinition, SaberThrowState, EF_DEAD, MAX_CLIENTS, WP_SABER,
};

impl PlayerPresenter {
    /// The `clientInfo_t::saber[]` pair `WP_SetSaber` produces for this client.
    pub(in crate::cgame::player_presenter) fn equipped_sabers(
        &self,
        info: &crate::cgame::ClientInfo,
    ) -> [Option<SaberDefinition>; 2] {
        self.saber_definitions.equip(
            [&info.saber_name, &info.saber2_name],
            info.client_num < MAX_CLIENTS,
        )
    }

    /// The equipped sabers as Pmove sees them through BG_MySaber.
    pub fn saber_movement_loadout(
        &self,
        saber_names: [&str; 2],
        player_client: bool,
    ) -> [jka_movement::SaberMovementInfo; 2] {
        crate::local_server::saber_movement_loadout(
            &self.saber_definitions,
            saber_names,
            player_client,
        )
    }

    pub(in crate::cgame::player_presenter) fn report_saber_warning_once(
        &mut self,
        entity_num: u16,
        error: &str,
    ) {
        if is_asset_pending(error) {
            return;
        }
        let key = error.to_ascii_lowercase();
        if self.reported_saber_warnings.insert(key) {
            println!("PLAYER SABER WARNING ent={entity_num}: {error}");
        }
    }

    pub(in crate::cgame::player_presenter) fn report_saber_diagnostic_once(
        &mut self,
        key: String,
        message: String,
    ) {
        if crate::logging::developer_enabled(2) && self.reported_saber_diagnostics.insert(key) {
            println!("{message}");
        }
    }

    pub(in crate::cgame::player_presenter) fn presented_saber_blade_length(
        &mut self,
        client_num: usize,
        saber_num: usize,
        blade_num: usize,
        authored_length: f32,
        desired_length: f32,
        current_time: i32,
    ) -> f32 {
        let key = SaberBladeLengthKey {
            client_num,
            saber_num: saber_num as u8,
            blade_num: blade_num as u8,
        };
        let state = self.saber_blade_lengths.entry(key).or_insert_with(|| {
            SaberBladeLengthState::new(authored_length, desired_length, current_time)
        });
        state.set_desired_and_update(desired_length, authored_length, current_time)
    }

    pub(in crate::cgame::player_presenter) fn append_player_sabers(
        &mut self,
        draws: &mut Vec<DynamicModelSurface>,
        entity: &PresentedEntity,
        info: &crate::cgame::ClientInfo,
        player_model: &PlayerModelAsset,
        player_pose: &[Matrix3x4],
        player_axis: [[f32; 3]; 3],
        player_origin: [f32; 3],
        current_time: i32,
        entity_alpha: f32,
        attached_sabers: [bool; 2],
        process_blades: bool,
    ) -> Result<(), String> {
        let dead = entity.state.field_i32("eFlags").unwrap_or(0) & EF_DEAD != 0;
        let weapon = entity.state.field_i32("weapon").unwrap_or(0);
        let holstered = entity.state.field_i32("saberHolstered").unwrap_or(0);
        let in_flight = entity.state.field_i32("saberInFlight").unwrap_or(0) != 0;
        let saber_move = entity.state.field_i32("saberMove").unwrap_or(0);
        let torso_anim = entity.state.field_i32("torsoAnim").unwrap_or(0);
        let mut sabers = Vec::with_capacity(2);
        // These booleans are the Rust equivalent of actual Ghoul2 model
        // indices 1/2. Hilt visibility follows those slots, not holster/death
        // guesses from currentState. A slot must also exist in the client's
        // WP_SetSaber result.
        let [primary, secondary] = self.equipped_sabers(info);
        if let (true, Some(definition)) = (attached_sabers[0], primary) {
            sabers.push((
                0usize,
                info.saber_name.as_str(),
                info.saber_color,
                "*r_hand",
                definition,
            ));
        }
        if let (true, Some(definition)) = (attached_sabers[1], secondary) {
            sabers.push((
                1usize,
                info.saber2_name.as_str(),
                info.saber2_color,
                "*l_hand",
                definition,
            ));
        }

        for (saber_num, saber_name, saber_color, hand_name, definition) in sabers {
            let Some(hand_bolt) = model_bolt_matrix_timed(
                &mut self.perf,
                &player_model.glm,
                &player_model.gla,
                player_pose,
                hand_name,
            )?
            else {
                return Err(format!(
                    "{} has no Ghoul2 {} bolt",
                    player_model.key, hand_name
                ));
            };
            let hilt = self.load_saber_model_in_game(&definition)?;
            let mut animator = Ghoul2Animator::new(&hilt.gla);
            let pose_started = Instant::now();
            let hilt_pose = animator.evaluate_pose(&hilt.gla, current_time, hand_bolt)?;
            record_pose_eval(&mut self.perf, pose_started);
            if i32::from(entity.number) == self.viewer_client {
                if let Some(debug) = self.viewer_anim_debug.as_mut() {
                    debug.hilt_world[saber_num] = Some(transform_jka_model_point(
                        [hand_bolt[0][3], hand_bolt[1][3], hand_bolt[2][3]],
                        player_axis,
                        player_origin,
                    ));
                }
            }
            let mut hilt_draws = self.render_glm_surfaces(
                entity.number,
                &definition.model,
                &hilt.glm,
                &hilt.gla,
                &hilt.surfaces,
                None,
                &hilt_pose,
                0,
                player_axis,
                player_origin,
                [1.0, 1.0, 1.0, entity_alpha],
                None,
                false,
            )?;
            if i32::from(entity.number) == self.viewer_client {
                if let Some(debug) = self.viewer_anim_debug.as_mut() {
                    debug.hilt_submitted[saber_num] =
                        hilt_draws.iter().any(|draw| draw.raster_visible);
                }
            }
            // Two saber slots may legitimately use the exact same hilt model.
            // Keep their RT BLAS identities distinct while material-shell clones
            // of one physical hilt continue sharing the same caster identity.
            if self.rt_shadow_casters_enabled {
                for surface in &mut hilt_draws {
                    if let Some(key) = surface.rt_skinned_key.take() {
                        surface.rt_skinned_key = Some(Arc::<str>::from(format!(
                            "{}#held-saber{}",
                            key.as_ref(),
                            saber_num,
                        )));
                    }
                }
            }
            draws.append(&mut hilt_draws);
            // ET_BODY goes through CG_General in OpenJK. The copied hilt model
            // remains, but CG_Player/CG_AddSaberBlade never runs for the body.
            if !process_blades {
                continue;
            }

            // Process every authored blade, including blades that are turning
            // off. OpenJK still advances those lengths and calls the saber-blade
            // path with dontDraw once they reach zero; our zero-length request
            // is what clears WeaponFx trail/contact history.
            for blade_index in 0..definition.num_blades {
                let tag_name = format!("*blade{}", blade_index + 1);
                let (blade_bolt, resolved_tag) = if let Some(matrix) = model_bolt_matrix_timed(
                    &mut self.perf,
                    &hilt.glm,
                    &hilt.gla,
                    &hilt_pose,
                    &tag_name,
                )? {
                    (matrix, tag_name.as_str())
                } else if blade_index == 0 {
                    if let Some(matrix) = model_bolt_matrix_timed(
                        &mut self.perf,
                        &hilt.glm,
                        &hilt.gla,
                        &hilt_pose,
                        "*flash",
                    )? {
                        (matrix, "*flash")
                    } else {
                        self.report_saber_diagnostic_once(
                            format!(
                                "held:{}:{}:{}:missing-bolt",
                                entity.number, saber_num, blade_index
                            ),
                            format!(
                                "SABER PRESENT FAILED ent={} saber={} name={} stage=bladeBolt tried={},*flash",
                                entity.number, saber_num, saber_name, tag_name
                            ),
                        );
                        continue;
                    }
                } else {
                    self.report_saber_diagnostic_once(
                        format!(
                            "held:{}:{}:{}:missing-bolt",
                            entity.number, saber_num, blade_index
                        ),
                        format!(
                            "SABER PRESENT FAILED ent={} saber={} name={} stage=bladeBolt tried={}",
                            entity.number, saber_num, saber_name, tag_name
                        ),
                    );
                    continue;
                };
                let origin_model = [blade_bolt[0][3], blade_bolt[1][3], blade_bolt[2][3]];
                // Public G2API_GetBoltMatrix applies its historical 90-degree
                // column fix.  CG_Player then requests NEGATIVE_Y, which reduces
                // to -column0 of the low/internal bolt matrix used here.
                let dir_model =
                    normalize_vec3([-blade_bolt[0][0], -blade_bolt[1][0], -blade_bolt[2][0]]);
                let origin_world =
                    transform_jka_model_point(origin_model, player_axis, player_origin);
                let dir_world = normalize_vec3(transform_jka_model_vector(dir_model, player_axis));
                let blade = definition.blade(blade_index);
                // stillDoSaber in OpenJK:
                // - EF_DEAD + WP_SABER => both sabers desired 0, gradual
                // - saberHolstered==1 => primary blade0 on, extra/second off
                // - saberHolstered>=2 => both desired 0, gradual
                // - non-saber weapon => lengths are immediately zeroed
                let desired_length = if weapon != WP_SABER || dead || holstered >= 2 {
                    0.0
                } else if holstered == 1 && (saber_num > 0 || blade_index > 0) {
                    0.0
                } else {
                    -1.0
                };
                let presented_length = if weapon == WP_SABER {
                    self.presented_saber_blade_length(
                        info.client_num,
                        saber_num,
                        blade_index,
                        blade.length,
                        desired_length,
                        current_time,
                    )
                } else {
                    let key = SaberBladeLengthKey {
                        client_num: info.client_num,
                        saber_num: saber_num as u8,
                        blade_num: blade_index as u8,
                    };
                    self.saber_blade_lengths.insert(
                        key,
                        SaberBladeLengthState::new(blade.length, 0.0, current_time),
                    );
                    0.0
                };
                let client_color =
                    if saber_num == 0 && self.saber_staff_multi_color && blade_index >= 1 {
                        info.saber2_color
                    } else {
                        saber_color
                    };
                let saber_color =
                    blade_color(info, client_color, &blade, Some(self.saber_team_colors));
                if i32::from(entity.number) == self.viewer_client && blade_index < 8 {
                    if let Some(debug) = self.viewer_anim_debug.as_mut() {
                        debug.blade_world[saber_num][blade_index] = Some(origin_world);
                        debug.blade_core_world[saber_num][blade_index] =
                            Some(std::array::from_fn(|i| origin_world[i] - dir_world[i]));
                        debug.blade_length[saber_num][blade_index] = presented_length;
                        debug.blade_submitted[saber_num][blade_index] =
                            process_blades && presented_length > 0.0 && entity_alpha >= 8.0 / 255.0;
                    }
                }
                let secondary_style = definition.blade_style2_start > 0
                    && blade_index >= definition.blade_style2_start;
                let trail_style = if secondary_style {
                    definition.trail_style2
                } else {
                    definition.trail_style
                };
                let no_wall_marks = if secondary_style {
                    definition.no_wall_marks2
                } else {
                    definition.no_wall_marks
                };
                if let Some(request) = saber_blade_fx_request(
                    origin_world,
                    dir_world,
                    presented_length,
                    blade.length,
                    blade.radius,
                    saber_color,
                    entity_alpha,
                    entity.number,
                    saber_num as u8,
                    blade_index as u8,
                    saber_move,
                    torso_anim,
                    in_flight,
                    trail_style,
                    definition.num_blades as u8,
                    definition.no_dlight,
                    no_wall_marks,
                ) {
                    self.fx_requests.push(request);
                }
                self.report_saber_diagnostic_once(
                    format!("held:{}:{}:{}:submitted", entity.number, saber_num, blade_index),
                    format!(
                        "SABER PRESENT ent={} saber={} name={} handBolt={} blade={} tag={} origin=({:.1},{:.1},{:.1}) dir=({:.3},{:.3},{:.3}) length={:.1} radius={:.1} color={} submitted=1",
                        entity.number,
                        saber_num,
                        saber_name,
                        hand_name,
                        blade_index,
                        resolved_tag,
                        origin_world[0], origin_world[1], origin_world[2],
                        dir_world[0], dir_world[1], dir_world[2],
                        presented_length,
                        blade.radius,
                        saber_color,
                    ),
                );
            }
        }
        Ok(())
    }

    /// OpenJK's thrown saber is not an ordinary generic-model submission.
    /// `CG_Player` follows `saberEntityNum`, manually renders that Ghoul2 hilt,
    /// and evaluates the primary saber's blade bolts from the thrown entity.
    pub fn present_thrown_saber_for_player(
        &mut self,
        owner: &PresentedEntity,
        info: &crate::cgame::ClientInfo,
        entities: &[PresentedEntity],
        game: &ClientGameState,
        current_time: i32,
        entity_alpha: f32,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let mut draws = Vec::new();
        // CG_Player only runs the thrown-saber render inside its
        // `weapon == WP_SABER && saberHolstered < 2` branch.
        if owner.state.field_i32("weapon").unwrap_or(0) != WP_SABER
            || owner.state.field_i32("saberHolstered").unwrap_or(0) >= 2
            || owner.state.field_i32("saberInFlight").unwrap_or(0) == 0
            || owner.state.field_i32("saberEntityNum").unwrap_or(0) == 0
        {
            self.thrown_sabers.remove(&owner.number);
            return Ok(draws);
        }

        let saber_entity_num = owner.state.field_i32("saberEntityNum").unwrap_or(-1);
        let Ok(saber_entity_num_u16) = u16::try_from(saber_entity_num) else {
            self.thrown_sabers.remove(&owner.number);
            self.report_saber_diagnostic_once(
                format!("throw:{}:bad-entity", owner.number),
                format!(
                    "SABER THROW FAILED owner={} stage=saberEntityNum value={saber_entity_num}",
                    owner.number
                ),
            );
            return Ok(draws);
        };
        let Some(saber_entity) = entities
            .iter()
            .find(|entity| entity.number == saber_entity_num_u16)
        else {
            self.thrown_sabers.remove(&owner.number);
            self.report_saber_diagnostic_once(
                format!("throw:{}:{}:missing-entity", owner.number, saber_entity_num),
                format!(
                    "SABER THROW FAILED owner={} saberEnt={} stage=entityLookup",
                    owner.number, saber_entity_num
                ),
            );
            return Ok(draws);
        };

        let [primary, _] = self.equipped_sabers(info);
        let Some(mut definition) = primary else {
            self.thrown_sabers.remove(&owner.number);
            return Ok(draws);
        };
        let model_index = saber_entity.state.field_i32("modelindex").unwrap_or(0);
        let network_model = game.model_qpath(model_index);
        if let Some(qpath) = network_model
            .as_ref()
            .filter(|qpath| qpath.to_ascii_lowercase().ends_with(".glm"))
        {
            // The thrown entity's network-visible model is authoritative, just
            // like OpenJK's CS_MODELS lookup before G2API_InitGhoul2Model.
            definition.model.clone_from(qpath);
        }
        let hilt = self.load_saber_model_in_game(&definition)?;
        let mut animator = Ghoul2Animator::new(&hilt.gla);
        let pose_started = Instant::now();
        let hilt_pose = animator.evaluate_pose_openjk_root(&hilt.gla, current_time)?;
        record_pose_eval(&mut self.perf, pose_started);
        let snapshot_num = game
            .current_snapshot()
            .map_or(0, |snapshot| snapshot.message_num);
        let throw_state = self.thrown_sabers.entry(owner.number).or_insert_with(|| {
            SaberThrowState::new(owner, saber_entity, current_time, snapshot_num)
        });
        let hilt_angles = throw_state.angles(
            owner,
            saber_entity,
            current_time,
            snapshot_num,
            definition.return_damage,
        )?;
        let saber_axis = angles_to_axis(hilt_angles);
        let blade_axis = angles_to_axis(blade_angles(hilt_angles));
        let outbound = saber_entity.state.field_i32("saberInFlight").unwrap_or(0) != 0;
        self.report_saber_diagnostic_once(
            format!("throw-axis:{}:{}:{outbound}", owner.number, saber_entity.number),
            format!(
                "SABER THROW AXIS owner={} saberEnt={} outbound={} angles={:?} aposType={} yawVelocity={} returnDamage={}",
                owner.number, saber_entity.number, outbound, hilt_angles,
                saber_entity.state.field_i32("apos.trType").unwrap_or(0),
                saber_entity.state.field_f32("apos.trDelta[1]").unwrap_or(0.0),
                definition.return_damage,
            ),
        );

        let mut hilt_draws = self.render_glm_surfaces(
            saber_entity.number,
            &definition.model,
            &hilt.glm,
            &hilt.gla,
            &hilt.surfaces,
            None,
            &hilt_pose,
            0,
            saber_axis,
            saber_entity.origin,
            [1.0, 1.0, 1.0, entity_alpha],
            None,
            false,
        )?;
        if i32::from(owner.number) == self.viewer_client {
            if let Some(debug) = self.viewer_anim_debug.as_mut() {
                debug.hilt_world[0] = Some(saber_entity.origin);
                debug.hilt_submitted[0] = hilt_draws.iter().any(|draw| draw.raster_visible);
            }
        }
        draws.append(&mut hilt_draws);

        let holstered = owner.state.field_i32("saberHolstered").unwrap_or(0);
        for blade_index in 0..definition.num_blades {
            if holstered == 1 && blade_index > 0 {
                // A staff thrown in single-blade mode: the extra blades get a
                // desired length of 0 and CG_AddSaberBlade(dontDraw). Keep
                // their persistent length state advancing without drawing.
                let authored = definition.blade(blade_index).length;
                self.presented_saber_blade_length(
                    info.client_num,
                    0,
                    blade_index,
                    authored,
                    0.0,
                    current_time,
                );
                continue;
            }
            let tag_name = format!("*blade{}", blade_index + 1);
            let (blade_bolt, resolved_tag) = if let Some(matrix) = model_bolt_matrix_timed(
                &mut self.perf,
                &hilt.glm,
                &hilt.gla,
                &hilt_pose,
                &tag_name,
            )? {
                (matrix, tag_name.as_str())
            } else if blade_index == 0 {
                if let Some(matrix) = model_bolt_matrix_timed(
                    &mut self.perf,
                    &hilt.glm,
                    &hilt.gla,
                    &hilt_pose,
                    "*flash",
                )? {
                    (matrix, "*flash")
                } else {
                    self.report_saber_diagnostic_once(
                        format!("throw:{}:{}:missing-bolt", owner.number, blade_index),
                        format!(
                            "SABER THROW FAILED owner={} saberEnt={} model={} stage=bladeBolt tried={},*flash",
                            owner.number, saber_entity.number, definition.model, tag_name
                        ),
                    );
                    continue;
                }
            } else {
                self.report_saber_diagnostic_once(
                    format!("throw:{}:{}:missing-bolt", owner.number, blade_index),
                    format!(
                        "SABER THROW FAILED owner={} saberEnt={} model={} stage=bladeBolt tried={}",
                        owner.number, saber_entity.number, definition.model, tag_name
                    ),
                );
                continue;
            };

            let origin_model = [blade_bolt[0][3], blade_bolt[1][3], blade_bolt[2][3]];
            let dir_model =
                normalize_vec3([-blade_bolt[0][0], -blade_bolt[1][0], -blade_bolt[2][0]]);
            let origin_world =
                transform_jka_model_point(origin_model, blade_axis, saber_entity.origin);
            let dir_world = normalize_vec3(transform_jka_model_vector(dir_model, blade_axis));
            let blade = definition.blade(blade_index);
            let presented_length = self.presented_saber_blade_length(
                info.client_num,
                0,
                blade_index,
                blade.length,
                -1.0,
                current_time,
            );
            let client_color = if self.saber_staff_multi_color && blade_index >= 1 {
                info.saber2_color
            } else {
                info.saber_color
            };
            let saber_color = blade_color(info, client_color, &blade, Some(self.saber_team_colors));
            if i32::from(owner.number) == self.viewer_client && blade_index < 8 {
                if let Some(debug) = self.viewer_anim_debug.as_mut() {
                    debug.blade_world[0][blade_index] = Some(origin_world);
                    debug.blade_core_world[0][blade_index] =
                        Some(std::array::from_fn(|i| origin_world[i] - dir_world[i]));
                    debug.blade_length[0][blade_index] = presented_length;
                    debug.blade_submitted[0][blade_index] =
                        presented_length > 0.0 && entity_alpha >= 8.0 / 255.0;
                }
            }
            let secondary_style =
                definition.blade_style2_start > 0 && blade_index >= definition.blade_style2_start;
            let trail_style = if secondary_style {
                definition.trail_style2
            } else {
                definition.trail_style
            };
            let no_wall_marks = if secondary_style {
                definition.no_wall_marks2
            } else {
                definition.no_wall_marks
            };
            if let Some(request) = saber_blade_fx_request(
                origin_world,
                dir_world,
                presented_length,
                blade.length,
                blade.radius,
                saber_color,
                entity_alpha,
                owner.number,
                0,
                blade_index as u8,
                owner.state.field_i32("saberMove").unwrap_or(0),
                owner.state.field_i32("torsoAnim").unwrap_or(0),
                true,
                trail_style,
                definition.num_blades as u8,
                definition.no_dlight,
                no_wall_marks,
            ) {
                self.fx_requests.push(request);
            }
            self.report_saber_diagnostic_once(
                format!("throw:{}:{}:submitted", owner.number, blade_index),
                format!(
                    "SABER THROW owner={} saberEnt={} name={} model={} modelindex={} blade={} tag={} origin=({:.1},{:.1},{:.1}) dir=({:.3},{:.3},{:.3}) length={:.1} radius={:.1} color={} submitted=1",
                    owner.number,
                    saber_entity.number,
                    info.saber_name,
                    definition.model,
                    model_index,
                    blade_index,
                    resolved_tag,
                    origin_world[0], origin_world[1], origin_world[2],
                    dir_world[0], dir_world[1], dir_world[2],
                    presented_length,
                    blade.radius,
                    saber_color,
                ),
            );
        }

        Ok(draws)
    }
}
