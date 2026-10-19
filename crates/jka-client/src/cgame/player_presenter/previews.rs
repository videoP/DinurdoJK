//! Previews.
use crate::cgame::player_presenter::{
    animation_index, apply_profile_studio_light, blade_color, cosmetic_draws_for_mask,
    model_bolt_matrix_timed, normalize_vec3, record_pose_eval, saber_blade_fx_request,
    saber_name_is_removed, sibling_default_skin_qpath, transform_jka_model_point,
    transform_jka_model_vector, Arc, DynamicModelSurface, Ghoul2Animator, Instant, PlayerPresenter,
    BONE_ANIM_OVERRIDE_FREEZE, BONE_ANIM_OVERRIDE_LOOP,
};

impl PlayerPresenter {
    pub(crate) fn profile_body_shape_available(&self, info: &crate::cgame::ClientInfo) -> bool {
        let key = super::player_model_key(info);
        matches!(self.lookup_model(&key), super::assets::loading::ModelLookup::Ready(model) if model.body_shape_available)
    }
    /// Render one externally supplied pose using the model's native mesh rig.
    /// This is the input seam for future animation decoders; sampling, timing
    /// and clip playback remain the caller's responsibility.
    #[allow(dead_code)]
    pub fn present_player_pose_preview(
        &mut self,
        info: &crate::cgame::ClientInfo,
        source_skeleton: &[jka_assets::ghoul2::GlaBone],
        input: jka_assets::skeleton::PoseInput<'_>,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let model = self.load_model(info)?;
        let plan = jka_assets::skeleton::cached_retargeter(source_skeleton, &model.gla.skeleton)?;
        let pose = model.bind_mesh_pose(plan.retarget(input)?)?;
        self.render_glm_surfaces(
            0, &info.model_qpath(), &model.glm, &model.gla, &model.surfaces,
            self.jiggle_profile_for_sex(model.jiggle.as_deref(), model.female),
            &pose, 0, axis, origin, [1.0; 4], None, true,
        )
    }

    pub(in crate::cgame::player_presenter) fn load_emote_gla(
        &mut self, qpath: &str,
    ) -> Result<Arc<jka_assets::ghoul2::GlaAnimation>, String> {
        let cell = {
            let mut cache = self.load_shared.glas.lock()
                .map_err(|_| "Ghoul2 animation cache poisoned".to_owned())?;
            Arc::clone(cache.entry(qpath.to_ascii_lowercase()).or_default())
        };
        cell.get_or_init(|| {
            if let Some(bytes) = crate::emotes::read_asset(qpath, jka_emotes::MAX_MEMBER_BYTES)? {
                jka_assets::ghoul2::parse_gla(&bytes).map(Arc::new)
            } else { self.load_glm_animation(qpath) }
        }).clone()
    }

    /// `TurretClientRun` from TaystJK. This is deliberately separate from
    /// `present_static_glm`: the pickup remains a static registration while
    /// deployed (bolt1) ET_GENERAL entities animate on their own clock.
    #[allow(clippy::too_many_arguments)]
    pub fn present_turret_glm(
        &mut self,
        entity_num: u16,
        model_qpath: &str,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
        rgba: [f32; 4],
        started_at: i32,
        current_time: i32,
        hinge_angles: [f32; 3],
        muzzle_flash: bool,
    ) -> Result<(Vec<DynamicModelSurface>, Option<([f32; 3], [f32; 3])>), String> {
        let model = self.load_static_glm(model_qpath, None, model_qpath)?;
        let mut animator = Ghoul2Animator::new(&model.animation_gla);
        // Exactly CG_TurretClientRun's model_root clip, not a generic GLM idle.
        animator.set_bone_anim(&model.animation_gla, "model_root", 0, 11,
            BONE_ANIM_OVERRIDE_FREEZE, 0.8, started_at, None, 0)?;
        for bone in ["bone_gback", "bone_barrel"] {
            animator.set_bone_angles_postmult(&model.animation_gla, bone,
                [0.0; 3], jka_movement::G2_POSITIVE_Y,
                jka_movement::G2_POSITIVE_Z, jka_movement::G2_POSITIVE_X)?;
        }
        animator.set_bone_angles_replace(&model.animation_gla, "bone_hinge",
            hinge_angles, jka_movement::G2_NEGATIVE_Y,
            jka_movement::G2_NEGATIVE_Z, jka_movement::G2_NEGATIVE_X)?;
        let pose_started = Instant::now();
        let pose = model.bind_animation_pose(
            animator.evaluate_pose_openjk_root(&model.animation_gla, current_time)?)?;
        record_pose_eval(&mut self.perf, pose_started);
        // CG_TurretClientRun takes the origin and NEGATIVE_X of *flash02.
        let muzzle = if muzzle_flash {
            model_bolt_matrix_timed(&mut self.perf, &model.glm, &model.gla, &pose, "*flash02")?
                .map(|bolt| {
                    let local = [bolt[0][3], bolt[1][3], bolt[2][3]];
                    let dir = [-bolt[0][0], -bolt[1][0], -bolt[2][0]];
                    (transform_jka_model_point(local, axis, origin),
                     transform_jka_model_vector(dir, axis))
                })
        } else { None };
        let draws = self.render_glm_surfaces(entity_num, model_qpath,
            &model.glm, &model.gla, &model.surfaces, None, &pose,
            0, axis, origin, rgba, None, true)?;
        Ok((draws, muzzle))
    }

    /// Submit a Ghoul2 model in its default pose as one refEntity: `axis` may
    /// carry a non-uniform scale (CG_Item's 1.5x weapons), `rgba` is the
    /// RF_RGB_TINT/RF_FORCE_ENT_ALPHA shaderRGBA and `custom_shader` replaces
    /// every surface's material like refEntity.customShader.
    pub fn present_static_glm(
        &mut self,
        entity_num: u16,
        model_qpath: &str,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
        rgba: [f32; 4],
        custom_shader: Option<&str>,
        current_time: i32,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let mut model = self.load_static_glm(model_qpath, None, model_qpath)?;
        // TaystJK/OpenJK CG_General initializes the Ghoul2 model first, then
        // checks G2API_SkinlessModel and, when needed, applies the sibling
        // `model_default.skin`. Our no-skin registration yields no drawable
        // surfaces for that same class of GLM, so perform the identical
        // fallback before submitting the entity. A missing default skin is
        // non-fatal in OpenJK (R_RegisterSkin returns handle 0), so retain the
        // original registration if this optional lookup fails.
        if model.surfaces.is_empty() {
            if let Some(default_skin) = sibling_default_skin_qpath(model_qpath) {
                if let Ok(skinned) =
                    self.load_static_glm(model_qpath, Some(&default_skin), model_qpath)
                {
                    model = skinned;
                }
            }
        }
        let pose_started = Instant::now();
        let pose =
            model.bind_animation_pose(Ghoul2Animator::new(&model.animation_gla).evaluate_pose_openjk_root(&model.animation_gla, current_time)?)?;
        record_pose_eval(&mut self.perf, pose_started);
        let custom = custom_shader.map(|shader| self.custom_shader_material(shader));
        let draws = self.render_glm_surfaces(
            entity_num,
            model_qpath,
            &model.glm,
            &model.gla,
            &model.surfaces,
            None,
            &pose,
            0,
            axis,
            origin,
            rgba,
            custom,
            true,
        )?;
        Ok(draws)
    }

    /// Profile preview path for a real JKA player selection. Unlike the generic
    /// Asset Viewer, this resolves the exact model[/skin] through ClientInfo, so
    /// multipart skins and `*off` surface mappings behave exactly like CG_Player.
    // Asset-preview entry point that is not wired into the viewer yet.
    #[allow(dead_code)]
    pub fn present_static_player_preview(
        &mut self,
        info: &crate::cgame::ClientInfo,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
        current_time: i32,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let model = self.load_model(info)?;
        let pose_started = Instant::now();
        let pose =
            model.bind_animation_pose(Ghoul2Animator::new(&model.animation_gla).evaluate_pose_openjk_root(&model.animation_gla, current_time)?)?;
        record_pose_eval(&mut self.perf, pose_started);
        self.render_glm_surfaces(
            0,
            &info.model_qpath(),
            &model.glm,
            &model.gla,
            &model.surfaces,
            self.jiggle_profile_for_sex(model.jiggle.as_deref(), model.female),
            &pose,
            0,
            axis,
            origin,
            [1.0; 4],
            None,
            true,
        )
    }

    /// Modern Profile viewport: stock humanoid walk-in-place animation plus
    /// the selected saber hilts bolted to the same hand tags used by CG_Player.
    /// This is deliberately a thin presentation path over the normal assets;
    /// model/skin/saber semantics remain OpenJK-compatible userinfo values.
    pub fn present_profile_player_preview(
        &mut self,
        info: &crate::cgame::ClientInfo,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
        current_time: i32,
        animation_name: &str,
        with_sabers: bool,
        skin_tint: [f32; 3],
        cosmetics: u32,
        // Normalized cursor offset in the Studio stage, used for the player's
        // authored cervical/cranium Ghoul2 look bones.
        head_look: [f32; 2],
        studio_cinematic: bool,
        // A menu emote is a presentation-only override, never an entityState
        // animation: the playing preview, or a scrubbed one (emote_gallery).
        emote_preview: Option<crate::emotes::Preview>,
    ) -> Result<(Vec<DynamicModelSurface>, [Option<[f32; 3]>; 7]), String> {
        // This presentation route doesn't call present_snapshot_players, which
        // normally advances the stage clock. The async jiggle worker needs a
        // monotonically advancing preview timeline to integrate the walk pose.
        self.stage_time_ms = current_time.max(0);
        let model = self.load_model(info)?;
        // Cache converted GLAs through the existing asset loader.
        let emote_gla = if let Some(active) = emote_preview.as_ref() {
            let clip = self.load_emote_gla(&active.emote.gla)?;
            crate::emotes::pose_plan(&clip, &model.gla)?;
            if active.emote.end_frame as usize > active.emote.pose.as_ref().map_or(clip.num_frames, |p| p.frames) {
                return Err(format!("emote {} ends at frame {}, GLA has {} frames",
                    active.emote.id, active.emote.end_frame, clip.num_frames));
            }
            Some(clip)
        } else {
            None
        };
        let playback_gla = emote_gla.as_ref().unwrap_or(&model.animation_gla);
        let animation_number = animation_index(animation_name)
            .ok_or_else(|| format!("missing animation table entry {animation_name}"))?
            as i32;
        let animation = *self
            .animations
            .get(animation_number)
            .ok_or_else(|| format!("missing humanoid animation {animation_name}"))?;
        if animation.frame_lerp == 0 {
            return Err(format!(
                "humanoid animation {animation_name} has zero frameLerp"
            ));
        }
        let anim_speed = 50.0 / f32::from(animation.frame_lerp);
        let (first_frame, last_frame) = if anim_speed < 0.0 {
            (
                i32::from(animation.first_frame) + i32::from(animation.num_frames),
                i32::from(animation.first_frame),
            )
        } else {
            (
                i32::from(animation.first_frame),
                i32::from(animation.first_frame) + i32::from(animation.num_frames),
            )
        };
        let flags = if animation.loop_frames != -1 {
            BONE_ANIM_OVERRIDE_LOOP
        } else {
            BONE_ANIM_OVERRIDE_FREEZE
        };
        let baked_pose = emote_preview.as_ref().and_then(|active|
            active.emote.pose.as_ref().map(|clip|crate::emotes::sample_body(clip,active)));
        let (first_frame, last_frame, flags, anim_speed, pose_time) =
            if let Some(active) = emote_preview.as_ref() {
                let spec = &active.emote;
                (spec.first_frame, spec.end_frame,
                    if spec.looping { BONE_ANIM_OVERRIDE_LOOP } else { BONE_ANIM_OVERRIDE_FREEZE },
                    spec.fps / 20.0, active.elapsed_ms)
            } else {
                (first_frame, last_frame, flags, anim_speed, current_time.max(0))
            };
        let pose_started = Instant::now();
        // Compact bundles use the GLA only as a skeleton. Their frame ranges
        // belong to animation.pose, so never register them on the one-frame GLA.
        let mut animation_pose = match baked_pose {
            Some(pose) => pose,
            None => {
                let mut animator = Ghoul2Animator::new(playback_gla);
                // CG_Player uses model_root for legs and lower_lumbar for torso. For a
                // full-body menu stand both use the same authored animation; Motion is
                // updated too, matching the same-animation branch in CG_PlayerAnimation.
                for bone in ["model_root", "lower_lumbar", "Motion"] {
                    if Ghoul2Animator::bone_index(playback_gla, bone).is_some() {
                        animator.set_bone_anim(
                            playback_gla,
                            bone,
                            first_frame,
                            last_frame,
                            flags,
                            anim_speed,
                            0,
                            None,
                            0,
                        )?;
                    }
                }
                // The same cranium/cervical postmult conventions as OpenJK
                // BG_G2PlayerAngles (POSITIVE_X, NEGATIVE_Y, NEGATIVE_Z). Apply before
                // pose evaluation, so hats, sabers and the entire model stay coherent.
                let pitch = (head_look[1] * 15.0).clamp(-15.0, 15.0);
                let yaw = (head_look[0] * 24.0).clamp(-24.0, 24.0);
                for (bone, weight) in [("cervical", 0.34), ("cranium", 0.66)] {
                    if Ghoul2Animator::bone_index(playback_gla, bone).is_some() {
                        animator.set_bone_angles_postmult(
                            playback_gla, bone, [pitch * weight, yaw * weight, 0.0],
                            jka_movement::G2_POSITIVE_X,
                            jka_movement::G2_NEGATIVE_Y,
                            jka_movement::G2_NEGATIVE_Z,
                        )?;
                    }
                }
                animator.evaluate_pose_openjk_root(playback_gla, pose_time)?
            }
        };
        if emote_preview.is_none() && self.ghoul2_anim_smooth > 0.0 && self.ghoul2_anim_smooth < 1.0 {
            const MAX_HISTORY_GAP_MS: i32 = 250;
            if let Some((old_model, old_animation, old_time, previous)) =
                self.profile_bone_smooth_history.as_ref()
            {
                if old_model == &model.key
                    && old_animation == animation_name
                    && current_time > *old_time
                    && current_time.saturating_sub(*old_time) <= MAX_HISTORY_GAP_MS
                    && previous.len() == animation_pose.len()
                {
                    animation_pose = super::smooth_ghoul2_pose(
                        &model.animation_gla, previous, &animation_pose,
                        self.ghoul2_anim_smooth,
                    );
                }
            }
            self.profile_bone_smooth_history = Some((
                model.key.clone(), animation_name.to_owned(), current_time,
                animation_pose.clone(),
            ));
        } else {
            self.profile_bone_smooth_history = None;
        }
        let mut pose = if emote_preview.is_some() {
            model.bind_external_pose(playback_gla, animation_pose)?
        } else {
            model.bind_animation_pose(animation_pose)?
        };
        if model.source_jiggles_enabled {
            if let Some(bindings) = model.source_jiggles.as_deref() {
                self.preview_source_jiggles
                    .entry(model.key.clone())
                    .or_default()
                    .update(bindings, model.gla.skeleton.len(), &mut pose,
                            axis, origin, current_time);
            }
        }
        if let Some(active)=emote_preview.as_ref(){model.correct_emote_ground(active,&mut pose)?;}
        if let Some(active)=emote_preview.as_ref(){
            self.cosmetic_draws.extend(Self::emote_prop_draws(active,&model,&pose,axis,origin,0));
            let mut fx=Self::emote_effect_frame(active,&model,&pose,axis,origin,0);
            self.cosmetic_draws.append(&mut fx.meshes);
            self.emote_fx.draws.append(&mut fx.draws);
            self.emote_fx.lights.append(&mut fx.lights);
        }
        // The UI anchors use the *same animated Ghoul2 pose* as skinning,
        // including cursor-driven cranium/cervical edits and model yaw.
        // No fixed screen percentages or arbitrary player-space guesses.
        let mut anchors = [None; 7];
        for (index, tag) in [(0, "*head_top"), (2, "*back"),
            (3, "*r_hand"), (4, "*l_hand")] {
            if let Some(matrix) = model_bolt_matrix_timed(
                &mut self.perf, &model.glm, &model.gla, &pose, tag)? {
                let point = [matrix[0][3], matrix[1][3], matrix[2][3]];
                anchors[index] = Some(transform_jka_model_point(point, axis, origin));
                if index == 0 {
                    // Face follows the exact same head bolt matrix and its
                    // local axes (rather than a fixed screen-space marker).
                    let face = [point[0] - 5.0 * matrix[0][0] - 7.0 * matrix[0][2],
                                point[1] - 5.0 * matrix[1][0] - 7.0 * matrix[1][2],
                                point[2] - 5.0 * matrix[2][0] - 7.0 * matrix[2][2]];
                    anchors[1] = Some(transform_jka_model_point(face, axis, origin));
                }
            }
        }
        if let Some(profile) = model.jiggle.as_deref() {
            anchors[5] = profile.studio_body_anchor(false, &pose, axis, origin);
            anchors[6] = profile.studio_body_anchor(true, &pose, axis, origin);
        }
        record_pose_eval(&mut self.perf, pose_started);
        // The studio is CPU-skinned for cinematic lighting. Use the same
        // promoted bind-pose mesh as gameplay when drawing its body shape.
        self.preview_cpu_remesh = true;
        let body_result = self.render_glm_surfaces_tinted(
            0,
            &info.model_qpath(),
            &model.glm,
            &model.gla,
            &model.surfaces,
            self.jiggle_profile_for_sex(model.jiggle.as_deref(), model.female),
            &pose,
            0,
            axis,
            origin,
            [1.0; 4],
            skin_tint,
            None,
            true,
        );
        self.preview_cpu_remesh = false;
        let mut draws = body_result?;

        if with_sabers {
            for (saber_num, saber_name, hand_name) in [
                (0usize, info.saber_name.as_str(), "*r_hand"),
                (1usize, info.saber2_name.as_str(), "*l_hand"),
            ] {
                if saber_name.is_empty() || saber_name_is_removed(saber_name) {
                    continue;
                }
                let definition = self.saber_definitions.definition_or_default(saber_name);
                let Some(hand_bolt) = model_bolt_matrix_timed(
                    &mut self.perf,
                    &model.glm,
                    &model.gla,
                    &pose,
                    hand_name,
                )?
                else {
                    continue;
                };
                let hilt = self.load_saber_model(&definition)?;
                let mut hilt_animator = Ghoul2Animator::new(&hilt.animation_gla);
                let pose_started = Instant::now();
                let hilt_pose = hilt.bind_animation_pose(hilt_animator.evaluate_pose(&hilt.animation_gla, current_time, hand_bolt)?)?;
                record_pose_eval(&mut self.perf, pose_started);
                let mut hilt_draws = self.render_glm_surfaces(
                    0,
                    &definition.model,
                    &hilt.glm,
                    &hilt.gla,
                    &hilt.surfaces,
                    None,
                    &hilt_pose,
                    0,
                    axis,
                    origin,
                    [1.0; 4],
                    None,
                    false,
                )?;
                if self.rt_shadow_casters_enabled {
                    for surface in &mut hilt_draws {
                        if let Some(key) = surface.rt_skinned_key.take() {
                            surface.rt_skinned_key = Some(Arc::<str>::from(format!(
                                "{}#profile-saber{}",
                                key.as_ref(),
                                saber_num,
                            )));
                        }
                    }
                }
                draws.append(&mut hilt_draws);

                // CG_AddSaberBlade-equivalent preview path. Phase 2 attached the
                // hilt but stopped here, which is why the Profile showed a bare
                // handle. Resolve every authored blade bolt exactly as the live
                // player presenter does and hand the resulting blade to the same
                // WeaponFx material/geometry path used in gameplay.
                let client_color = if saber_num == 0 {
                    info.saber_color
                } else {
                    info.saber2_color
                };
                for blade_index in 0..definition.num_blades {
                    let tag_name = format!("*blade{}", blade_index + 1);
                    let (blade_bolt, tag_hack) = if let Some(matrix) = model_bolt_matrix_timed(
                        &mut self.perf,
                        &hilt.glm,
                        &hilt.gla,
                        &hilt_pose,
                        &tag_name,
                    )? {
                        (matrix, false)
                    } else {
                        // UI_SaberDrawBlade falls back to *flash for every
                        // blade, not just blade 0, so pre-JKA hilts still preview.
                        let Some(matrix) = model_bolt_matrix_timed(
                            &mut self.perf,
                            &hilt.glm,
                            &hilt.gla,
                            &hilt_pose,
                            "*flash",
                        )?
                        else {
                            continue;
                        };
                        (matrix, true)
                    };
                    let mut origin_model = [blade_bolt[0][3], blade_bolt[1][3], blade_bolt[2][3]];
                    let mut dir_model =
                        normalize_vec3([-blade_bolt[0][0], -blade_bolt[1][0], -blade_bolt[2][0]]);
                    // Exact stock UI staff tag-hack: when blade2 has no authored
                    // bolt, reverse the *flash direction and offset sixteen units.
                    if tag_hack
                        && blade_index == 1
                        && definition.saber_type.eq_ignore_ascii_case("SABER_STAFF")
                    {
                        dir_model = [-dir_model[0], -dir_model[1], -dir_model[2]];
                        origin_model = [
                            origin_model[0] + dir_model[0] * 16.0,
                            origin_model[1] + dir_model[1] * 16.0,
                            origin_model[2] + dir_model[2] * 16.0,
                        ];
                    }
                    let origin_world = transform_jka_model_point(origin_model, axis, origin);
                    let dir_world = normalize_vec3(transform_jka_model_vector(dir_model, axis));
                    let blade = definition.blade(blade_index);
                    let color = blade_color(info, client_color, &blade, None);
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
                        blade.length,
                        blade.length,
                        blade.radius,
                        color,
                        1.0,
                        0,
                        saber_num as u8,
                        blade_index as u8,
                        0,
                        animation_number,
                        false,
                        trail_style,
                        definition.num_blades as u8,
                        definition.no_dlight,
                        no_wall_marks,
                    ) {
                        self.fx_requests.push(request);
                    }
                }
            }
        }

        // The worn jaPRO cosmetics ride the same pose; the caller drains them
        // into the MD3 presenter (see `drain_cosmetic_draws`).
        if cosmetics != 0 {
            let worn = cosmetic_draws_for_mask(
                &mut self.perf,
                &model,
                &pose,
                axis,
                origin,
                cosmetics,
                0,
                [1.0; 4],
                None,
            );
            self.cosmetic_draws.extend(worn);
        }

        apply_profile_studio_light(&mut draws, studio_cinematic);
        Ok((draws, anchors))
    }

    /// TaystJK's Profile saber preview resolves the selected saber block to its
    /// authored saberModel and optional customSkin. Keep that exact data path,
    /// but render it through DinurdoJK's normal Ghoul2/WGPU presenter.
    // Asset-preview entry point that is not wired into the viewer yet.
    #[allow(dead_code)]
    pub fn present_static_saber_preview(
        &mut self,
        saber_name: &str,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
        current_time: i32,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let definition = self.saber_definitions.definition_or_default(saber_name);
        let model = self.load_saber_model(&definition)?;
        let pose_started = Instant::now();
        let pose =
            model.bind_animation_pose(Ghoul2Animator::new(&model.animation_gla).evaluate_pose_openjk_root(&model.animation_gla, current_time)?)?;
        record_pose_eval(&mut self.perf, pose_started);
        self.render_glm_surfaces(
            0,
            &definition.model,
            &model.glm,
            &model.gla,
            &model.surfaces,
            None,
            &pose,
            0,
            axis,
            origin,
            [1.0; 4],
            None,
            true,
        )
    }

    /// Developer Asset Viewer GLM path. Player/NPC model.glm files usually
    /// obtain their materials from model_default.skin rather than from the GLM
    /// hierarchy itself. Try that sibling skin automatically. If it is absent
    /// or incomplete, retain renderable geometry with a neutral $whiteimage
    /// material instead of making the model disappear against the black viewer.
    pub fn present_static_glm_preview(
        &mut self,
        entity_num: u16,
        model_qpath: &str,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
        rgba: [f32; 4],
        current_time: i32,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let normalized = model_qpath.replace('\\', "/");
        let inferred_skin = normalized
            .rsplit_once('/')
            .filter(|(_, leaf)| leaf.eq_ignore_ascii_case("model.glm"))
            .map(|(folder, _)| format!("{folder}/model_default.skin"));
        let model = self.load_static_glm_with_options(
            model_qpath,
            inferred_skin.as_deref(),
            model_qpath,
            true,
        )?;
        let pose_started = Instant::now();
        let pose =
            model.bind_animation_pose(Ghoul2Animator::new(&model.animation_gla).evaluate_pose_openjk_root(&model.animation_gla, current_time)?)?;
        record_pose_eval(&mut self.perf, pose_started);
        self.render_glm_surfaces(
            entity_num,
            model_qpath,
            &model.glm,
            &model.gla,
            &model.surfaces,
            None,
            &pose,
            0,
            axis,
            origin,
            rgba,
            None,
            true,
        )
    }
}
