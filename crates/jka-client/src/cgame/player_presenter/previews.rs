//! Previews.
use crate::cgame::player_presenter::{
    animation_index, apply_profile_studio_light, blade_color, cosmetic_draws_for_mask,
    model_bolt_matrix_timed, normalize_vec3, record_pose_eval, saber_blade_fx_request,
    saber_name_is_removed, sibling_default_skin_qpath, transform_jka_model_point,
    transform_jka_model_vector, Arc, DynamicModelSurface, Ghoul2Animator, Instant, PlayerPresenter,
    BONE_ANIM_OVERRIDE_FREEZE, BONE_ANIM_OVERRIDE_LOOP,
};

impl PlayerPresenter {
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
            Ghoul2Animator::new(&model.gla).evaluate_pose_openjk_root(&model.gla, current_time)?;
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
            Ghoul2Animator::new(&model.gla).evaluate_pose_openjk_root(&model.gla, current_time)?;
        record_pose_eval(&mut self.perf, pose_started);
        self.render_glm_surfaces(
            0,
            &info.model_qpath(),
            &model.glm,
            &model.gla,
            &model.surfaces,
            model.jiggle.as_deref(),
            &pose,
            0,
            axis,
            origin,
            [1.0; 4],
            None,
            true,
        )
    }

    /// Modern Profile viewport: stock humanoid stand animation plus the actual
    /// selected saber hilts bolted to the same hand tags used by CG_Player.
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
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let model = self.load_model(info)?;
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

        let mut animator = Ghoul2Animator::new(&model.gla);
        // CG_Player uses model_root for legs and lower_lumbar for torso. For a
        // full-body menu stand both use the same authored animation; Motion is
        // updated too, matching the same-animation branch in CG_PlayerAnimation.
        for bone in ["model_root", "lower_lumbar", "Motion"] {
            if Ghoul2Animator::bone_index(&model.gla, bone).is_some() {
                animator.set_bone_anim(
                    &model.gla,
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
        let pose_started = Instant::now();
        let pose = animator.evaluate_pose_openjk_root(&model.gla, current_time.max(0))?;
        record_pose_eval(&mut self.perf, pose_started);
        let mut draws = self.render_glm_surfaces_tinted(
            0,
            &info.model_qpath(),
            &model.glm,
            &model.gla,
            &model.surfaces,
            model.jiggle.as_deref(),
            &pose,
            0,
            axis,
            origin,
            [1.0; 4],
            skin_tint,
            None,
            true,
        )?;

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
                let mut hilt_animator = Ghoul2Animator::new(&hilt.gla);
                let pose_started = Instant::now();
                let hilt_pose = hilt_animator.evaluate_pose(&hilt.gla, current_time, hand_bolt)?;
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

        apply_profile_studio_light(&mut draws);
        Ok(draws)
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
            Ghoul2Animator::new(&model.gla).evaluate_pose_openjk_root(&model.gla, current_time)?;
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
            Ghoul2Animator::new(&model.gla).evaluate_pose_openjk_root(&model.gla, current_time)?;
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
