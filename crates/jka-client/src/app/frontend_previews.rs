//! Frontend previews.
use crate::app::{
    local_player_alpha, offset_third_person_view, presented_openjk_player, rendering_third_person,
    scene, ui_catalog, App, Arc, AssetDetail, AssetKind, Camera, ClientInfo, Duration,
    DynamicModelSurface, EntityPresenter, FrontendPage, Ghoul2SkinningMode, Instant, JoinMode,
    PlayerFxRequest, PlayerPresenter, PlayerViewPolicyState, ProfileModelEntry, ProfileSaberEntry,
    RenderCommand, ThirdPersonViewInput, ViewLatchMode, ET_PLAYER, FRONTEND_CAMERA_EYE_HEIGHT,
    FRONTEND_FOREGROUND_MODEL, FRONTEND_FOREGROUND_MODEL_DISTANCE,
    FRONTEND_FOREGROUND_MODEL_HEAD_HEIGHT, FRONTEND_FOREGROUND_MODEL_SIDE_OFFSET,
};

/// Dedicated, silent FX scheduler used only by Developer Tools. Keeping this
/// separate from CGame's weapon/map FX means previewing an .efx file can never
/// perturb a live/demo effect timeline.
pub(in crate::app) struct AssetPreviewFxState {
    pub(in crate::app) qpath: String,
    pub(in crate::app) zoom_key: i32,
    pub(in crate::app) system: crate::fx::system::FxSystem,
    pub(in crate::app) effect_id: crate::fx::system::EffectId,
    pub(in crate::app) started: Instant,
    pub(in crate::app) next_play_ms: i32,
    pub(in crate::app) cycle_ms: i32,
}

impl App {
    pub(in crate::app) fn ensure_local_player_presenter(&mut self) -> Result<(), String> {
        if self.local_player_presenter.is_some() {
            return Ok(());
        }
        let mut assets =
            jka_assets::pk3::AssetSearchPath::open_game(&self.base, self.game.as_deref())
                .map_err(|error| format!("SOLO PLAYER ASSET PATH ERROR: {error}"))?;
        assets.set_allow_asset_overrides(self.video.allow_asset_overrides);
        self.local_player_presenter = Some(
            PlayerPresenter::new(assets, self.video.pbr)
                .map_err(|error| format!("SOLO PLAYER ASSET ERROR: {error}"))?,
        );
        Ok(())
    }

    pub(in crate::app) fn asset_preview_camera(&self) -> Camera {
        if self.profile_preview_active {
            let saber = self.profile_selected_section == 3;
            // Lower the studio camera and give it a slight upward pitch. The
            // old origin-level camera made the portrait read as if it were
            // looking down from above the character's chest/head line.
            let mut camera =
                Camera::new_with_fov([0.0, -11.0, 0.0], 0.0, if saber { 52.0 } else { 58.0 });
            camera.pitch = 5.0_f32.to_radians();
            camera
        } else {
            Camera::new_with_fov([0.0, 0.0, 0.0], 0.0, 65.0)
        }
    }

    pub(in crate::app) fn ensure_profile_catalog(&mut self) {
        if self.profile_catalog_loaded {
            return;
        }
        self.profile_catalog_loaded = true;
        self.ui_catalog.pending.profile = true;
        self.ui_catalog.request(
            &self.base,
            self.game.as_deref(),
            ui_catalog::CatalogRequest::Profile,
        );
    }

    pub(in crate::app) fn apply_profile_catalog(
        &mut self,
        result: Result<(Vec<ProfileModelEntry>, Vec<ProfileSaberEntry>), String>,
    ) {
        match result {
            Ok((models, mut sabers)) => {
                for current in [&self.network.saber1, &self.network.saber2] {
                    if !current.is_empty()
                        && !current.eq_ignore_ascii_case("none")
                        && !current.eq_ignore_ascii_case("remove")
                        && !sabers
                            .iter()
                            .any(|entry| entry.name.eq_ignore_ascii_case(current))
                    {
                        let fallback = jka_assets::saber::SaberDefinition::openjk_default(current);
                        sabers.push(ProfileSaberEntry {
                            display_name: fallback.proper_name.clone(),
                            saber_type: fallback.saber_type.clone(),
                            name: fallback.name,
                            model: fallback.model,
                            custom_skin: fallback.custom_skin,
                            num_blades: fallback.num_blades,
                        });
                    }
                }
                sabers.sort_by(|a, b| {
                    a.display_name
                        .to_ascii_lowercase()
                        .cmp(&b.display_name.to_ascii_lowercase())
                        .then_with(|| {
                            a.name
                                .to_ascii_lowercase()
                                .cmp(&b.name.to_ascii_lowercase())
                        })
                });
                self.profile_models = models;
                self.profile_sabers = sabers;
                self.profile_catalog_error = None;
                // Force the preview to rebuild now that saber types are known.
                self.profile_preview_key = None;
            }
            Err(error) => {
                self.profile_catalog_error = Some(error);
            }
        }
    }

    pub(in crate::app) fn update_profile_preview(&mut self) {
        if !self.profile_preview_active {
            return;
        }

        let force_page = self.profile_selected_section == 2;
        let saber_page = self.profile_selected_section == 3;
        if !force_page {
            self.profile_force_preview_pending = None;
            self.profile_force_preview_active = None;
        } else {
            if let Some((power, changed_at)) = self.profile_force_preview_pending {
                if changed_at.elapsed() >= Duration::from_millis(140) {
                    self.profile_force_preview_active = Some(power);
                    self.profile_force_preview_started = Instant::now();
                    self.profile_force_preview_pending = None;
                    self.profile_preview_key = None;
                }
            }
            // One-shot menu demonstration, then settle back to the neutral idle.
            if self.profile_force_preview_active.is_some()
                && self.profile_force_preview_started.elapsed() >= Duration::from_millis(1800)
            {
                self.profile_force_preview_active = None;
                self.profile_preview_started = Instant::now();
                self.profile_preview_key = None;
            }
        }

        let force_power = if force_page {
            self.profile_force_preview_active
        } else {
            None
        };
        let force_animation = force_power.map(|power| match power {
            0 => "BOTH_FORCEHEAL_QUICK",
            1 => "BOTH_FORCEJUMP1",
            2 => "BOTH_RUN1",
            3 => "BOTH_FORCEPUSH",
            4 => "BOTH_FORCEPULL",
            5 => "BOTH_FORCEPUSH",
            6 => "BOTH_FORCEGRIP_HOLD",
            7 => "BOTH_FORCELIGHTNING_HOLD",
            8 => "BOTH_FORCE_RAGE",
            9 => "BOTH_FORCE_PROTECT",
            10 => "BOTH_FORCE_ABSORB",
            11 => "BOTH_FORCEHEAL_QUICK",
            12 => "BOTH_FORCEPUSH",
            13 => "BOTH_FORCE_DRAIN_HOLD",
            // Seeing has no distinct stock activation character pose; saber
            // skill ranks are demonstrated in the stock saber-ready stance.
            14 => "BOTH_STAND1",
            15..=17 => "BOTH_STAND2",
            _ => "BOTH_STAND1",
        });

        let secondary_saber = !self.network.saber2.is_empty()
            && !self.network.saber2.eq_ignore_ascii_case("none")
            && !self.network.saber2.eq_ignore_ascii_case("remove");
        let primary_staff = self.profile_sabers.iter().any(|entry| {
            entry.name.eq_ignore_ascii_case(&self.network.saber1)
                && entry.saber_type.eq_ignore_ascii_case("SABER_STAFF")
        });
        let saber_stance = if secondary_saber {
            "BOTH_SABERDUAL_STANCE"
        } else if primary_staff {
            "BOTH_SABERSTAFF_STANCE"
        } else {
            "BOTH_STAND2"
        };
        let animation_name = force_animation.unwrap_or(if saber_page {
            saber_stance
        } else {
            "BOTH_STAND1"
        });
        let with_sabers = saber_page || matches!(force_power, Some(15) | Some(16) | Some(17));

        // 30 Hz is ample for the isolated Profile character and keeps its CPU
        // skinning/studio-lighting work independent of an uncapped render loop.
        let animation_elapsed = if force_power.is_some() {
            self.profile_force_preview_started.elapsed()
        } else {
            self.profile_preview_started.elapsed()
        };
        let animation_tick = (animation_elapsed.as_millis() / 33) as i32;
        let current_time = animation_tick.saturating_mul(33);
        let key = (
            format!(
                "{}:{}:{}:{}:{}:{}:{}:{}:{}:{:?}:{}",
                animation_name,
                self.solo_client_info.model_cvar(),
                self.network.saber1,
                self.network.saber2,
                with_sabers,
                self.network.color1,
                self.network.color2,
                self.network.sb_rgb1,
                self.network.sb_rgb2,
                self.network.char_color,
                self.network.cosmetics,
            ),
            (self.profile_preview_yaw * 10.0).round() as i32,
            (self.profile_preview_zoom * 1000.0).round() as i32,
            animation_tick,
        );
        if self.profile_preview_key.as_ref() == Some(&key) {
            return;
        }

        let yaw = self.profile_preview_yaw.to_radians();
        let (s, c) = yaw.sin_cos();
        let axis = [[c, s, 0.0], [-s, c, 0.0], [0.0, 0.0, 1.0]];
        let zoom = self.profile_preview_zoom.clamp(0.45, 2.5);
        let mut info = self.solo_client_info.clone();
        info.saber_name = self.network.saber1.clone();
        info.saber2_name = self.network.saber2.clone();
        // Preview what a jaPRO / JA+ server would show, RGB included.
        info.saber_color = crate::cgame::resolve_saber_color(
            i32::from(self.network.color1),
            self.network.sb_rgb1 as i32,
            true,
        );
        info.saber2_color = crate::cgame::resolve_saber_color(
            i32::from(self.network.color2),
            self.network.sb_rgb2 as i32,
            true,
        );
        info.plugin_disable = self.network.plugin_disable;

        let result = (|| -> Result<Vec<DynamicModelSurface>, String> {
            self.ensure_asset_preview_player_presenter()?;
            let cosmetics = self.network.cosmetics;
            if with_sabers || cosmetics != 0 {
                self.ensure_asset_preview_entity_presenter()?;
            }
            let origin = if saber_page {
                [72.0 * zoom, 0.0, -20.0]
            } else {
                [88.0 * zoom, 0.0, -22.0]
            };
            let (mut draws, fx_requests) = {
                let presenter = self
                    .asset_preview_player_presenter
                    .as_mut()
                    .expect("profile Ghoul2 presenter initialized");
                presenter.set_skinning_mode(Ghoul2SkinningMode::Cpu);
                presenter.set_early_frustum_cull(false);
                presenter.set_lod_bias(self.video.ghoul2_lod_bias);
                presenter.set_lod_scale(self.video.lod_scale);
                // No stale requests from a prior Profile frame should leak into
                // the current isolated saber preview.
                let _ = presenter.drain_fx_requests();
                let draws = presenter.present_profile_player_preview(
                    &info,
                    origin,
                    axis,
                    current_time,
                    animation_name,
                    with_sabers,
                    self.network
                        .char_color
                        .map(|channel| f32::from(channel) / 255.0),
                    cosmetics,
                )?;
                let requests = presenter.drain_fx_requests();
                (draws, requests)
            };

            // Hats and capes the player has picked, live as they click.
            let worn = self
                .asset_preview_player_presenter
                .as_mut()
                .expect("profile Ghoul2 presenter initialized")
                .drain_cosmetic_draws();
            if !worn.is_empty() {
                let mut hats = self
                    .asset_preview_entity_presenter
                    .as_mut()
                    .expect("profile MD3 presenter initialized")
                    .present_cosmetics(worn);
                crate::cgame::player_presenter::apply_profile_studio_light(&mut hats);
                draws.append(&mut hats);
            }

            if with_sabers {
                let mut blade_draws = Vec::new();
                // Re-seeded per frame so the preview blade flickers like in game.
                let mut flicker_rng = crate::fx::template::Rng::new(
                    (current_time as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15),
                );
                for request in fx_requests {
                    if let PlayerFxRequest::SaberBlade {
                        origin,
                        direction,
                        length,
                        length_max,
                        radius,
                        color,
                        entity_alpha,
                        ..
                    } = request
                    {
                        blade_draws.extend(crate::cgame::weapon_fx::profile_saber_blade_draws(
                            origin,
                            direction,
                            length,
                            length_max,
                            radius,
                            color,
                            entity_alpha,
                            self.video.modern_sabers,
                            &mut flicker_rng,
                        ));
                    }
                }
                if !blade_draws.is_empty() {
                    let view = crate::fx::draw::FxView::from_camera(&self.asset_preview_camera());
                    let presenter = self
                        .asset_preview_entity_presenter
                        .as_mut()
                        .expect("profile FX presenter initialized");
                    let mut blade_surfaces =
                        crate::fx::draw::tessellate(&blade_draws, &view, &mut |shader| {
                            presenter.fx_material_stages(shader)
                        });
                    draws.append(&mut blade_surfaces);
                }
            }
            Ok(draws)
        })();

        match result {
            Ok(draws) => {
                self.profile_dynamic_models = Arc::new(draws);
                self.profile_preview_key = Some(key);
            }
            Err(error) => {
                self.profile_dynamic_models = Arc::new(Vec::new());
                self.profile_preview_key = Some(key);
                let message = format!("PROFILE PREVIEW: {error}");
                if self.console_status != message {
                    self.console_status = message.clone();
                    self.push_console_line(format!("^1{message}"));
                }
            }
        }
    }

    pub(in crate::app) fn ensure_asset_preview_entity_presenter(&mut self) -> Result<(), String> {
        if self.asset_preview_entity_presenter.is_some() {
            return Ok(());
        }
        let mut assets =
            jka_assets::pk3::AssetSearchPath::open_game(&self.base, self.game.as_deref())
                .map_err(|error| format!("ASSET VIEWER VFS ERROR: {error}"))?;
        assets.set_allow_asset_overrides(self.video.allow_asset_overrides);
        self.asset_preview_entity_presenter = Some(
            EntityPresenter::new(assets, self.video.pbr)
                .map_err(|error| format!("ASSET VIEWER MD3 ERROR: {error}"))?,
        );
        Ok(())
    }

    pub(in crate::app) fn ensure_asset_preview_player_presenter(&mut self) -> Result<(), String> {
        if self.asset_preview_player_presenter.is_some() {
            return Ok(());
        }
        let mut assets =
            jka_assets::pk3::AssetSearchPath::open_game(&self.base, self.game.as_deref())
                .map_err(|error| format!("ASSET VIEWER VFS ERROR: {error}"))?;
        assets.set_allow_asset_overrides(self.video.allow_asset_overrides);
        self.asset_preview_player_presenter = Some(
            PlayerPresenter::new(assets, self.video.pbr)
                .map_err(|error| format!("ASSET VIEWER GLM ERROR: {error}"))?,
        );
        Ok(())
    }

    pub(in crate::app) fn clear_asset_preview_runtime(&mut self) {
        self.solo_dynamic_models = Arc::new(Vec::new());
        self.asset_preview_model_key = None;
        self.asset_preview_shader_key = None;
        self.asset_preview_fx = None;
    }

    pub(in crate::app) fn set_asset_preview_viewport(
        &mut self,
        rect: egui::Rect,
        pixels_per_point: f32,
    ) {
        let scale = pixels_per_point.max(0.01);
        let viewport = [
            (rect.min.x * scale).round().max(0.0) as u32,
            (rect.min.y * scale).round().max(0.0) as u32,
            (rect.width() * scale).round().max(1.0) as u32,
            (rect.height() * scale).round().max(1.0) as u32,
        ];
        if self.asset_preview_viewport_key != Some(viewport) {
            self.asset_preview_viewport_key = Some(viewport);
            self.render_command(RenderCommand::SetAssetPreviewViewport(Some(viewport)));
        }
    }

    pub(in crate::app) fn update_asset_preview_content(&mut self) {
        if self.frontend_page != FrontendPage::AssetViewer {
            return;
        }
        let Some(detail) = self
            .asset_viewer_detail
            .as_ref()
            .and_then(|detail| detail.as_ref().ok())
            .cloned()
        else {
            self.clear_asset_preview_runtime();
            return;
        };

        match detail.kind {
            AssetKind::Md3 | AssetKind::Glm => self.update_asset_model_preview(&detail),
            AssetKind::Shader => self.update_asset_shader_preview(&detail),
            AssetKind::Efx => self.update_asset_efx_preview(&detail),
        }
    }

    pub(in crate::app) fn update_asset_model_preview(&mut self, detail: &AssetDetail) {
        let key = (
            detail.qpath.clone(),
            (self.asset_viewer_model_yaw * 10.0).round() as i32,
            (self.asset_viewer_model_pitch * 10.0).round() as i32,
            (self.asset_viewer_model_zoom * 1000.0).round() as i32,
        );
        if self.asset_preview_model_key.as_ref() == Some(&key) {
            return;
        }

        let center = detail.model_center.unwrap_or([0.0; 3]);
        let radius = detail.model_radius.unwrap_or(32.0).max(1.0);
        let distance = (radius * 2.45 * self.asset_viewer_model_zoom).clamp(24.0, 7500.0);
        let yaw = self.asset_viewer_model_yaw.to_radians();
        let pitch = self.asset_viewer_model_pitch.to_radians();
        let (sy, cy) = yaw.sin_cos();
        let (sp, cp) = pitch.sin_cos();
        // Match the existing yaw convention, then pitch around the model's
        // horizontal axis. This keeps drag rotation centered on the asset rather
        // than pitching the viewer camera away from the preview target.
        let axis = [
            [cy * cp, sy * cp, -sp],
            [-sy, cy, 0.0],
            [cy * sp, sy * sp, cp],
        ];
        let rotated_center = [
            axis[0][0] * center[0] + axis[1][0] * center[1] + axis[2][0] * center[2],
            axis[0][1] * center[0] + axis[1][1] * center[1] + axis[2][1] * center[2],
            axis[0][2] * center[0] + axis[1][2] * center[1] + axis[2][2] * center[2],
        ];
        let target = [distance, 0.0, 0.0];
        let origin = [
            target[0] - rotated_center[0],
            target[1] - rotated_center[1],
            target[2] - rotated_center[2],
        ];
        let qpath = detail.qpath.clone();
        let kind = detail.kind;

        let result = match kind {
            AssetKind::Md3 => self.ensure_asset_preview_entity_presenter().and_then(|_| {
                self.asset_preview_entity_presenter
                    .as_mut()
                    .expect("asset MD3 presenter initialized")
                    .present_static_md3(0, &qpath, origin, axis, [1.0; 4], 0)
            }),
            AssetKind::Glm => self.ensure_asset_preview_player_presenter().and_then(|_| {
                let presenter = self
                    .asset_preview_player_presenter
                    .as_mut()
                    .expect("asset GLM presenter initialized");
                presenter.set_skinning_mode(self.video.ghoul2_skinning);
                presenter.set_early_frustum_cull(false);
                presenter.set_lod_bias(self.video.ghoul2_lod_bias);
                presenter.set_lod_scale(self.video.lod_scale);
                presenter.present_static_glm_preview(0, &qpath, origin, axis, [1.0; 4], 0)
            }),
            AssetKind::Efx | AssetKind::Shader => unreachable!(),
        };
        match result {
            Ok(draws) => {
                self.solo_dynamic_models = Arc::new(draws);
                self.asset_preview_model_key = Some(key);
            }
            Err(error) => {
                self.solo_dynamic_models = Arc::new(Vec::new());
                self.asset_preview_model_key = Some(key);
                let message = format!("ASSET VIEWER PREVIEW: {qpath}: {error}");
                if self.console_status != message {
                    self.console_status = message.clone();
                    self.push_console_line(format!("^1{message}"));
                }
            }
        }
    }

    pub(in crate::app) fn update_asset_shader_preview(&mut self, detail: &AssetDetail) {
        let Some(shader_name) = self
            .asset_viewer_shader_name
            .clone()
            .or_else(|| detail.shader_names.first().cloned())
        else {
            self.solo_dynamic_models = Arc::new(Vec::new());
            self.asset_preview_shader_key = None;
            return;
        };
        let zoom_key = (self.asset_viewer_model_zoom * 1000.0).round() as i32;
        let key = (detail.qpath.clone(), shader_name.clone(), zoom_key);
        if self.asset_preview_shader_key.as_ref() == Some(&key) {
            return;
        }
        if let Err(error) = self.ensure_asset_preview_entity_presenter() {
            self.solo_dynamic_models = Arc::new(Vec::new());
            self.asset_preview_shader_key = Some(key);
            self.push_console_line(format!("^1ASSET VIEWER SHADER PREVIEW: {error}"));
            return;
        }

        // A material browser is most useful on a neutral, camera-facing card.
        // Use the existing FX material resolver so blend modes and multi-stage
        // JKA shaders are represented by the same textures/pipelines as gameplay.
        let distance = (112.0 * self.asset_viewer_model_zoom).clamp(32.0, 3000.0);
        let half = 48.0;
        // Put a neutral checker behind the authored material. Besides making
        // alpha obvious, this gives filter/modulate stages something sensible
        // to composite against instead of the viewer's black clear color.
        let mut draws = Vec::with_capacity(65);
        let cells = 8_u32;
        let cell = half * 2.0 / cells as f32;
        for y in 0..cells {
            for z in 0..cells {
                let y0 = -half + y as f32 * cell;
                let y1 = y0 + cell;
                let z0 = -half + z as f32 * cell;
                let z1 = z0 + cell;
                let shade = if (y + z) & 1 == 0 { 96 } else { 144 };
                draws.push(crate::fx::system::FxDraw::Quad {
                    positions: [
                        [distance + 0.5, y0, z1],
                        [distance + 0.5, y1, z1],
                        [distance + 0.5, y1, z0],
                        [distance + 0.5, y0, z0],
                    ],
                    uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                    rgba: [shade, shade, shade, 255],
                    shader: "$whiteimage".to_owned(),
                });
            }
        }
        draws.push(crate::fx::system::FxDraw::Quad {
            positions: [
                [distance, -half, half],
                [distance, half, half],
                [distance, half, -half],
                [distance, -half, -half],
            ],
            uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            rgba: [255, 255, 255, 255],
            shader: shader_name.clone(),
        });
        let view = crate::fx::draw::FxView::from_camera(&self.asset_preview_camera());
        let presenter = self
            .asset_preview_entity_presenter
            .as_mut()
            .expect("asset shader presenter initialized");
        let surfaces = crate::fx::draw::tessellate(&draws, &view, &mut |shader| {
            presenter.fx_material_stages(shader)
        });
        self.solo_dynamic_models = Arc::new(surfaces);
        self.asset_preview_shader_key = Some(key);
    }

    pub(in crate::app) fn ensure_asset_preview_fx(
        &mut self,
        detail: &AssetDetail,
    ) -> Result<(), String> {
        let zoom_key = (self.asset_viewer_model_zoom * 1000.0).round() as i32;
        if self
            .asset_preview_fx
            .as_ref()
            .is_some_and(|state| state.qpath == detail.qpath && state.zoom_key == zoom_key)
        {
            return Ok(());
        }

        let mut assets =
            jka_assets::pk3::AssetSearchPath::open_game(&self.base, self.game.as_deref())
                .map_err(|error| format!("ASSET VIEWER EFX VFS ERROR: {error}"))?;
        assets.set_allow_asset_overrides(self.video.allow_asset_overrides);
        let mut system = crate::fx::system::FxSystem::new();
        let effect_id = system.register(&detail.qpath, &mut |qpath| {
            assets
                .read(qpath, 32 * 1024 * 1024)
                .ok()
                .flatten()
                .map(|asset| asset.bytes)
        });
        if effect_id == 0 {
            return Err(format!("could not register {}", detail.qpath));
        }
        system.reset_time(0);
        self.asset_preview_fx = Some(AssetPreviewFxState {
            qpath: detail.qpath.clone(),
            zoom_key,
            system,
            effect_id,
            started: Instant::now(),
            next_play_ms: 0,
            cycle_ms: detail.preview_cycle_ms.unwrap_or(1500).clamp(250, 15_000),
        });
        Ok(())
    }

    pub(in crate::app) fn update_asset_efx_preview(&mut self, detail: &AssetDetail) {
        if let Err(error) = self.ensure_asset_preview_entity_presenter() {
            self.solo_dynamic_models = Arc::new(Vec::new());
            self.push_console_line(format!("^1ASSET VIEWER EFX PREVIEW: {error}"));
            return;
        }
        if let Err(error) = self.ensure_asset_preview_fx(detail) {
            self.solo_dynamic_models = Arc::new(Vec::new());
            let message = format!("ASSET VIEWER EFX PREVIEW: {error}");
            if self.console_status != message {
                self.console_status = message.clone();
                self.push_console_line(format!("^1{message}"));
            }
            return;
        }

        let distance = (112.0 * self.asset_viewer_model_zoom).clamp(24.0, 8000.0);
        let frame = {
            let state = self
                .asset_preview_fx
                .as_mut()
                .expect("asset EFX state initialized");
            let now_ms = state.started.elapsed().as_millis().min(i32::MAX as u128) as i32;
            let now_ms = now_ms.max(1);
            state.system.adjust_time(now_ms);
            if now_ms >= state.next_play_ms {
                state.system.play_effect_dir(
                    state.effect_id,
                    [distance, 0.0, 0.0],
                    [1.0, 0.0, 0.0],
                );
                state.next_play_ms = now_ms.saturating_add(state.cycle_ms);
            }
            state.system.frame()
        };

        // The viewer is intentionally silent; Sound primitives remain metadata.
        // Visual primitives are run by the real FX scheduler and material path.
        let view = crate::fx::draw::FxView::from_camera(&self.asset_preview_camera());
        let presenter = self
            .asset_preview_entity_presenter
            .as_mut()
            .expect("asset EFX presenter initialized");
        let surfaces = crate::fx::draw::tessellate(&frame.draws, &view, &mut |shader| {
            presenter.fx_material_stages(shader)
        });
        self.solo_dynamic_models = Arc::new(surfaces);
    }

    pub(in crate::app) fn update_frontend_foreground_player(&mut self) {
        let Some(cinematic) = self.frontend_cinematic else {
            self.solo_dynamic_models = Arc::new(Vec::new());
            return;
        };
        if let Err(error) = self.ensure_local_player_presenter() {
            if self.console_status != error {
                self.console_status = error.clone();
                self.push_console_line(format!("^1{error}"));
            }
            self.solo_dynamic_models = Arc::new(Vec::new());
            return;
        }

        let Some(idle_anim) =
            jka_assets::animation::animation_index("BOTH_STAND1").map(|index| index as i32)
        else {
            self.solo_dynamic_models = Arc::new(Vec::new());
            return;
        };

        let forward = self.camera.forward();
        let right = glam::Vec3::new(-forward.z, 0.0, forward.x).normalize_or_zero();
        let model_origin = self.camera.position
            + forward * FRONTEND_FOREGROUND_MODEL_DISTANCE
            + right * FRONTEND_FOREGROUND_MODEL_SIDE_OFFSET;
        let model_feet_render = [
            model_origin.x,
            model_origin.y - FRONTEND_FOREGROUND_MODEL_HEAD_HEIGHT,
            model_origin.z,
        ];
        let model_feet_jka = scene::jka_position(model_feet_render);
        let camera_jka = scene::jka_position(self.camera.position.to_array());
        let to_camera_x = camera_jka[0] - model_feet_jka[0];
        let to_camera_y = camera_jka[1] - model_feet_jka[1];
        let face_camera_yaw = to_camera_y.atan2(to_camera_x).to_degrees();

        let mut entity_view = jka_movement::PlayerEntityView::default();
        entity_view.number = 0;
        entity_view.entity_type = ET_PLAYER;
        entity_view.client_num = 0;
        // Camera/world rendering uses Y-up render coordinates, while the shared
        // OpenJK player presentation path consumes native JKA Z-up coordinates.
        entity_view.origin = model_feet_jka;
        entity_view.angles = [0.0, face_camera_yaw, 0.0];
        entity_view.legs_anim = idle_anim;
        entity_view.torso_anim = idle_anim;
        entity_view.health = 100;
        entity_view.view_height = FRONTEND_CAMERA_EYE_HEIGHT as i32;
        entity_view.ground_entity_num = jka_movement::ENTITY_WORLD;

        let Some(entity) = presented_openjk_player(entity_view) else {
            self.solo_dynamic_models = Arc::new(Vec::new());
            return;
        };

        let current_time = cinematic
            .started
            .elapsed()
            .as_millis()
            .min(i32::MAX as u128) as i32;
        let presenter = self
            .local_player_presenter
            .as_mut()
            .expect("local player presenter initialized");
        presenter.set_skinning_mode(self.video.ghoul2_skinning);
        presenter.set_early_frustum_cull(self.video.ghoul2_early_cull);
        presenter.set_lod_bias(self.video.ghoul2_lod_bias);
        presenter.set_ghoul2_anim_smooth(self.video.ghoul2_anim_smooth);
        presenter.set_lod_scale(self.video.lod_scale);
        let info = ClientInfo::solo_model(FRONTEND_FOREGROUND_MODEL);
        match presenter.present_player_entity(
            &entity,
            &info,
            current_time,
            1.0,
            Some(scene::jka_position(self.camera.position.to_array())),
            false,
            true,
            false,
            None,
        ) {
            Ok(draws) => self.solo_dynamic_models = Arc::new(draws),
            Err(error) => {
                self.solo_dynamic_models = Arc::new(Vec::new());
                if self.console_status != error {
                    self.console_status = error.clone();
                    self.push_console_line(format!("^1FRONTEND PLAYER PRESENTATION: {error}"));
                }
            }
        }
    }

    pub(in crate::app) fn update_solo_player_view_and_presentation(&mut self) {
        self.render_view_latch = ViewLatchMode::Disabled;
        if self
            .game_session
            .as_ref()
            .is_some_and(|session| session.local)
        {
            // `/map` and `/devmap` are presented exclusively by the shared
            // snapshot/CGame path. Keeping the legacy direct presenter active
            // here would double-submit the player and, more importantly, let
            // local-only FX behavior drift from joined servers again.
            self.solo_dynamic_models = Arc::new(Vec::new());
            return;
        }
        let mut first_person_camera = self.camera;
        let Some((
            mode,
            player_view,
            entity_view,
            view_angles,
            presentation_origin,
            presentation_time,
        )) = self.local_server.as_ref().map(|player| {
            player.camera_subframe(&mut first_person_camera);
            let player_view = player.view();
            let view_angles = player.subframe_view_angles();
            (
                player.mode,
                player_view,
                player.entity_view(),
                view_angles,
                player.presentation_origin(),
                player.presentation_time(),
            )
        })
        else {
            if self.front_end {
                if self.frontend_page != FrontendPage::AssetViewer {
                    self.update_frontend_foreground_player();
                }
            } else {
                self.solo_dynamic_models = Arc::new(Vec::new());
            }
            return;
        };
        let policy = PlayerViewPolicyState::from_entity_view(entity_view, player_view.legs_timer);
        let render_third_person = mode == JoinMode::Player
            && rendering_third_person(self.third_person, self.first_person_lightsaber, policy);
        let first_person_saber = mode == JoinMode::Player
            && !render_third_person
            && self.first_person_lightsaber
            && matches!(policy.weapon, 2 | 3);
        let latch_input_available =
            mode == JoinMode::Player && policy.health > 0 && policy.vehicle_num == 0;

        if render_third_person {
            if let Some(world) = self.map_collision.as_mut() {
                let view = offset_third_person_view(
                    world,
                    self.third_person,
                    &mut self.third_person_camera,
                    ThirdPersonViewInput {
                        origin: presentation_origin,
                        view_angles,
                        velocity: entity_view.velocity,
                        view_height: entity_view.view_height,
                        health: entity_view.health,
                        dead_yaw: entity_view.dead_yaw as f32,
                        client_num: entity_view.client_num,
                        time: f64::from(presentation_time),
                        teleported: false,
                    },
                );
                self.camera.position = glam::Vec3::from_array(scene::render_position(view.origin));
                self.camera.yaw = view.angles[1].to_radians();
                self.camera.pitch = -view.angles[0].to_radians();
                if latch_input_available {
                    if let Some(latch) = view.late_latch {
                        self.render_view_latch = ViewLatchMode::ThirdPerson(latch);
                    }
                }
            } else {
                self.camera = first_person_camera;
                if latch_input_available {
                    self.render_view_latch = ViewLatchMode::Direct;
                }
            }
        } else {
            self.camera = first_person_camera;
            if latch_input_available {
                self.render_view_latch = ViewLatchMode::Direct;
            }
        }

        // As in OpenJK CG_Player, keep the local player animation/Ghoul2 state
        // running even in first person. Geometry submission is the only thing
        // suppressed there (RF_THIRD_PERSON in OpenJK).
        if mode == JoinMode::Player && entity_view.entity_type == ET_PLAYER {
            if let Err(error) = self.ensure_local_player_presenter() {
                if self.console_status != error {
                    self.console_status = error.clone();
                    self.push_console_line(format!("^1{error}"));
                }
                self.solo_dynamic_models = Arc::new(Vec::new());
                return;
            }
            let mut render_entity_view = entity_view;
            render_entity_view.origin = presentation_origin;
            render_entity_view.angles = view_angles;
            if let Some(entity) = presented_openjk_player(render_entity_view) {
                let player_presentation_time = presentation_time;
                let ghoul2_view = self.current_ghoul2_view();
                let presenter = self
                    .local_player_presenter
                    .as_mut()
                    .expect("local player presenter initialized");
                presenter.set_skinning_mode(self.video.ghoul2_skinning);
                presenter.set_early_frustum_cull(self.video.ghoul2_early_cull);
                presenter.set_lod_bias(self.video.ghoul2_lod_bias);
                presenter.set_ghoul2_anim_smooth(self.video.ghoul2_anim_smooth);
                presenter.set_lod_scale(self.video.lod_scale);
                let result = presenter.present_player_entity(
                    &entity,
                    &self.solo_client_info,
                    player_presentation_time,
                    local_player_alpha(self.third_person.alpha),
                    None,
                    false,
                    render_third_person || first_person_saber,
                    first_person_saber,
                    ghoul2_view,
                );
                match result {
                    Ok(draws) => self.solo_dynamic_models = Arc::new(draws),
                    Err(error) => {
                        self.solo_dynamic_models = Arc::new(Vec::new());
                        if self.console_status != error {
                            self.console_status = error.clone();
                            self.push_console_line(format!("^1SOLO PLAYER PRESENTATION: {error}"));
                        }
                    }
                }
            } else {
                self.solo_dynamic_models = Arc::new(Vec::new());
            }
        } else {
            self.solo_dynamic_models = Arc::new(Vec::new());
        }
    }
}
