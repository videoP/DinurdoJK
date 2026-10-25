//! Overlay.
use crate::renderer::{
    append_vignette_vertices, create_menu_backdrop_resources, create_screen_fx_pipeline,
    load_ui_key_atlas, pipeline_hash, try_load_texture_asset,
    try_load_texture_asset_from_search_path, try_load_ui_font_asset, ui, upload_texture, Arc,
    AssetSearchPath, EguiRenderData, FxBlend, GpuImage, HashMap, Instant, Mat4,
    MenuBackdropResources, Path, PathBuf, PipelineJobKey, Renderer, TextureData, UiSnapshot,
    UiVertex, Vec3, SCREEN_FX_BUFFER_BYTES, UI_BUFFER_BYTES, UI_DYNAMIC_BUFFER_BYTES,
    UI_TRANSIENT_BUFFER_BYTES,
};

pub(in crate::renderer) struct PendingUiLevelshot {
    pub(in crate::renderer) key: String,
    pub(in crate::renderer) map_name: String,
    pub(in crate::renderer) game: Option<PathBuf>,
}

pub(in crate::renderer) struct ScreenFxTexture {
    pub(in crate::renderer) _image: GpuImage,
    pub(in crate::renderer) bind_group: wgpu::BindGroup,
}

impl Renderer {
    pub(in crate::renderer) fn set_ui(&mut self, mut ui_state: UiSnapshot) {
        let needs_key_atlas = !self.ui_keys_loaded && ui_state.movement_keys.mode != 0;
        // The app is not the owner of pipeline jobs; preserve the render-thread
        // status when a gameplay/menu UI snapshot arrives between samples.
        // The app never owns GPU atlas placement. Do not overwrite the UV
        // lookup after a scroll/HUD snapshot arrives on the render thread.
        ui_state.icon_atlas_uvs.clone_from(&self.ui_state.icon_atlas_uvs);
        ui_state.shader_compile_progress = self.ui_state.shader_compile_progress;
        ui_state.shader_compile_stage = self.ui_state.shader_compile_stage.clone();
        self.ui_state = ui_state;
        if needs_key_atlas {
            self.load_ui_key_atlas_now();
        }
        self.load_ui_icons_if_needed();
        self.sync_ui_team_icons();
        self.sync_ui_extension_images();
        self.sync_ui_loading_background();
        self.rebuild_ui();
    }

    pub(in crate::renderer) fn set_extension_hud(
        &mut self,
        widgets: Vec<crate::extensions::PlacedHudWidget>,
        hidden: crate::extensions::NativeHudMask,
        images: Vec<String>,
    ) {
        if self.ui_state.extension_hud == widgets
            && self.ui_state.native_hidden == hidden
            && self.ui_state.extension_images == images
        {
            return;
        }
        self.ui_state.extension_hud = widgets;
        self.ui_state.native_hidden = hidden;
        self.ui_state.extension_images = images;
        self.sync_ui_extension_images();
        // Uses the existing dynamic HUD buffer/pass. No new egui pass or pipeline.
        self.rebuild_dynamic_ui();
    }

    /// Poll background shader jobs at UI cadence, not at render frame rate.
    /// Upload only the small retained HUD batch when the integer count changes.
    pub(in crate::renderer) fn refresh_shader_compile_progress(&mut self) {
        let status = self.pipeline_jobs.progress().map(|progress| ui::MapLoadingBar {
            label: "COMPILING SHADERS",
            completed: progress.finished,
            total: progress.total,
            skipped: false,
        });
        let stage = if status.is_some() {
            crate::pipeline_jobs::world_pipeline_stage().map(|stage| format!(
                "BSP {}/{} {} ({}s)", stage.pipeline, stage.total,
                stage.description, stage.elapsed_seconds
            ))
        } else {
            None
        };
        // Compile queue just drained: persist the new pipelines now, so a crash
        // or kill later in the session does not lose them.
        if status.is_none() && self.ui_state.shader_compile_progress.is_some() {
            crate::pipeline_cache::save_in_background();
        }
        let signature = |bar: &Option<ui::MapLoadingBar>| {
            bar.as_ref().map(|bar| (bar.completed, bar.total))
        };
        if signature(&self.ui_state.shader_compile_progress) != signature(&status)
            || self.ui_state.shader_compile_stage != stage
        {
            self.ui_state.shader_compile_progress = status;
            self.ui_state.shader_compile_stage = stage;
            self.rebuild_retained_ui();
        }
    }

    pub(in crate::renderer) fn set_player_names(
        &mut self,
        player_names: Option<ui::UiWorldPlayerNames>,
    ) {
        self.player_names = player_names;
    }

    /// Rebuild the UI bind group from the current font/splash/key textures.
    pub(in crate::renderer) fn rebuild_ui_bind_group(&mut self) {
        let splash = if self.ui_binding_is_unknown_map {
            &self._ui_unknown_map
        } else {
            &self._ui_splash
        };
        self.ui_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("JKA UI font bind group"),
            layout: &self.ui_texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self._ui_font.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self._ui_font_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&self._ui_small_font.view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&splash.view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&self._ui_keys.view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&self._ui_icons.view),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&self._ui_team_icons.view),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(&self._ui_ext_images.view),
                },
            ],
        });
    }

    /// Bind the icon atlas the first time a draw list wants it: the lagometer's
    /// own frame / phone jack, the old speed graph's frame, an image crosshair,
    /// or a weapon / Force selection strip.
    pub(in crate::renderer) fn load_ui_icons_if_needed(&mut self) {
        if self.ui_icons_loaded {
            return;
        }
        let wanted = self
            .ui_state
            .lagometer
            .as_ref()
            .is_some_and(|ui| !ui.pics.is_empty())
            || self
                .ui_state
                .speedometer
                .as_ref()
                .is_some_and(|ui| !ui.graph_pics.is_empty())
            || self.ui_state.crosshair.image != 0
            || self.ui_state.force_select.is_some()
            || self.ui_state.weapon_select.is_some();
        if !wanted {
            return;
        }
        let started = Instant::now();
        let (data, uvs) = load_ui_icon_atlas(
            &self.base,
            self.detail_texture_game.as_deref(),
            self.device.limits().max_texture_dimension_2d,
        );
        self._ui_icons = upload_texture(&self.device, &self.queue, &data);
        // Keep UV metadata and the uploaded pixels in sync. Incoming app
        // snapshots preserve this render-thread-owned lookup in set_ui().
        self.ui_state.icon_atlas_uvs = uvs;
        self.ui_icons_loaded = true;
        self.rebuild_ui_bind_group();
        rverbose!(
            1,
            "UI icon atlas: loaded on first use in {:.1} ms",
            started.elapsed().as_secs_f64() * 1000.0
        );
    }

    /// Repack only when the tinfo/model/item icon set changes. The UI snapshot
    /// carries qpath candidate lists, so all VFS/image work stays on the render
    /// thread and the normal disabled-overlay path remains a 1x1 placeholder.
    pub(in crate::renderer) fn sync_ui_team_icons(&mut self) {
        // One atlas backs every player/class/powerup icon consumer. Keep all
        // active lists in deterministic order so the retained scoreboard and
        // transient team/duel HUD can coexist without rebinding source 6 to a
        // different layout between batches.
        let wanted = ui::shared_player_icon_paths(&self.ui_state);
        if wanted == self.ui_team_icon_key {
            return;
        }
        self.ui_team_icon_key = wanted.clone();
        let data = if wanted.is_empty() {
            empty_ui_font_texture("JKA UI team overlay atlas (not loaded)")
        } else {
            load_ui_team_icon_atlas(&self.base, self.detail_texture_game.as_deref(), &wanted)
        };
        self._ui_team_icons = upload_texture(&self.device, &self.queue, &data);
        self.rebuild_ui_bind_group();
    }

    /// Keep the extension image atlas in step with `ui_state.extension_images`.
    ///
    /// The slot list only grows while an extension is loaded, so this loads just
    /// the images that are new and writes each into its own cell: nothing already
    /// on screen is touched, and no frame ever re-reads the whole set from the
    /// game files. A list that is not an extension of the last one (extensions
    /// unloaded, or reloaded with different art) starts over with a blank atlas.
    pub(in crate::renderer) fn sync_ui_extension_images(&mut self) {
        if self.ui_state.extension_images == self.ui_ext_image_key {
            return;
        }
        let started = Instant::now();
        let wanted = self.ui_state.extension_images.clone();
        if wanted.is_empty() {
            self._ui_ext_images = upload_texture(
                &self.device,
                &self.queue,
                &empty_ui_font_texture("JKA UI extension image atlas (not loaded)"),
            );
            self.ui_ext_image_key.clear();
            self.rebuild_ui_bind_group();
            return;
        }
        if self.ui_ext_image_key.is_empty() || !wanted.starts_with(&self.ui_ext_image_key) {
            self._ui_ext_images =
                upload_texture(&self.device, &self.queue, &blank_ui_extension_atlas());
            self.ui_ext_image_key.clear();
            self.rebuild_ui_bind_group();
        }
        let first_new = self.ui_ext_image_key.len();
        let cells = load_ui_extension_images(
            &self.base,
            self.detail_texture_game.as_deref(),
            &wanted[first_new..],
        );
        let cell = ui::EXT_IMAGE_CELL;
        for (offset, pixels) in cells.iter().enumerate() {
            let Some(pixels) = pixels else { continue };
            let index = (first_new + offset) as u32;
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self._ui_ext_images._texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: (index % ui::EXT_IMAGE_COLUMNS) * cell,
                        y: (index / ui::EXT_IMAGE_COLUMNS) * cell,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(cell * 4),
                    rows_per_image: Some(cell),
                },
                wgpu::Extent3d { width: cell, height: cell, depth_or_array_layers: 1 },
            );
        }
        rverbose!(
            1,
            "UI extension images: {} new in {:.1} ms ({} total)",
            wanted.len() - first_new,
            started.elapsed().as_secs_f64() * 1000.0,
            wanted.len()
        );
        self.ui_ext_image_key = wanted;
    }

    /// Read the movement key images and bind them. Runs once, the first time the
    /// movement keys are switched on, instead of at every startup.
    pub(in crate::renderer) fn load_ui_key_atlas_now(&mut self) {
        let started = Instant::now();
        let data = load_ui_key_atlas(&self.base, self.detail_texture_game.as_deref());
        self._ui_keys = upload_texture(&self.device, &self.queue, &data);
        self.ui_keys_loaded = true;
        self.rebuild_ui_bind_group();
        rverbose!(
            1,
            "UI movement keys: atlas loaded on first use in {:.1} ms",
            started.elapsed().as_secs_f64() * 1000.0
        );
    }

    pub(in crate::renderer) fn sync_ui_loading_background(&mut self) {
        let request = self.ui_state.loading.as_ref().and_then(|loading| {
            let stem = loading_levelshot_stem(&loading.map_name)?;
            Some((stem, loading.active_game_dir.as_ref().map(PathBuf::from)))
        });
        let desired_key = request.as_ref().map(|(stem, game)| {
            format!(
                "{}\n{}",
                game.as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
                stem
            )
        });
        if self.ui_loading_image_key == desired_key {
            return;
        }

        self.ui_loading_image_key = desired_key.clone();
        self.ui_pending_levelshot = None;

        let Some((map_name, game)) = request else {
            // Binding 3 is not drawn outside the retained loading screen. Avoid
            // hitting the VFS just to restore menu/splash when loading ends.
            return;
        };
        let key = desired_key.expect("loading request must have a key");

        // First present the already-resident OpenJK MP unknown-map art. The real
        // levelshot is deliberately resolved only after this frame reaches present().
        self.ui_splash_size = self.ui_unknown_map_size;
        self.ui_binding_is_unknown_map = true;
        self.rebuild_ui_bind_group();
        self.ui_pending_levelshot = Some(PendingUiLevelshot {
            key,
            map_name,
            game,
        });
    }

    /// Resolve the real map levelshot only after at least one fallback frame has
    /// already reached `present()`. If disk/decode is slow, the compositor keeps
    /// showing `unknownmap_mp` instead of exposing the renderer clear color.
    pub(in crate::renderer) fn resolve_pending_ui_levelshot(&mut self) {
        // The startup splash deliberately owns the very first presented frame.
        // Do not consume a queued map levelshot until the actual loading UI
        // (and therefore unknownmap_mp) has itself reached present() once.
        if self.ui_state.startup_splash || self.ui_state.loading.is_none() {
            return;
        }
        let Some(pending) = self.ui_pending_levelshot.take() else {
            return;
        };
        if self.ui_loading_image_key.as_deref() != Some(pending.key.as_str()) {
            return;
        }

        let path = format!("levelshots/{}", pending.map_name);
        let Some(data) = try_load_texture_asset(
            &self.base,
            pending.game.as_deref(),
            &path,
            true,
            false,
            true,
            "UI levelshot",
        ) else {
            eprintln!("UI levelshot: no image resolved for {path}; keeping menu/art/unknownmap_mp");
            return;
        };

        let size = [data.width, data.height];
        let image = upload_texture(&self.device, &self.queue, &data);
        self.ui_splash_size = Some(size);
        self._ui_splash = image;
        self.ui_binding_is_unknown_map = false;
        self.rebuild_ui_bind_group();
        // Cover-mode geometry depends on the source dimensions.
        self.rebuild_retained_ui();
    }

    pub(in crate::renderer) fn ensure_screen_fx_pipeline(&mut self, blend: FxBlend) {
        if self.screen_fx_pipelines.contains_key(&blend) {
            return;
        }
        let key = PipelineJobKey::new(
            "screen-fx",
            blend as u32,
            pipeline_hash(&(self.config.format, blend)),
        );
        if let Some(pipeline) = self.pipeline_jobs.take_ready(key) {
            self.screen_fx_pipelines.insert(blend, pipeline);
            return;
        }
        let device = self.device.clone();
        let layout = self.screen_fx_pipeline_layout.clone();
        let shader = self.screen_fx_shader.clone();
        let format = self.config.format;
        self.pipeline_jobs
            .request(key, "screen FX blend variant", move || {
                create_screen_fx_pipeline(&device, &layout, &shader, format, blend)
            });
    }

    pub(in crate::renderer) fn ensure_screen_fx_texture(
        &mut self,
        texture: &Arc<TextureData>,
    ) -> String {
        let key = format!(
            "{}|{}|{}x{}|{}",
            texture.label,
            texture
                .source
                .as_ref()
                .map_or_else(String::new, |path| path.display().to_string()),
            texture.width,
            texture.height,
            texture.mip_level_count,
        );
        if !self.screen_fx_textures.contains_key(&key) {
            let image = upload_texture(&self.device, &self.queue, texture);
            self.baked_brightness.register(&image._texture);
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("JKA CGame screen FX bind group"),
                layout: &self.screen_fx_texture_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&image.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.screen_fx_sampler),
                    },
                ],
            });
            self.screen_fx_textures.insert(
                key.clone(),
                ScreenFxTexture {
                    _image: image,
                    bind_group,
                },
            );
        }
        key
    }

    pub(in crate::renderer) fn set_screen_fx(&mut self, draws: &[crate::fx::draw::ScreenFxDraw]) {
        self.screen_fx_batches.clear();
        self.screen_fx_vertex_count = 0;
        if draws.is_empty() {
            return;
        }

        let max_vertices = SCREEN_FX_BUFFER_BYTES as usize / std::mem::size_of::<UiVertex>();
        let mut vertices = Vec::with_capacity((draws.len() * 6).min(max_vertices));
        for draw in draws {
            if vertices.len().saturating_add(6) > max_vertices {
                break;
            }
            self.ensure_screen_fx_pipeline(draw.material.blend);
            let texture_key = draw
                .material
                .texture
                .as_ref()
                .map(|texture| self.ensure_screen_fx_texture(texture));

            let [mut x, y, mut w, h] = draw.rect;
            if draw.anchor != crate::fx::draw::ScreenFxAnchor::Stretch {
                // widthRatioCoef = (640 * height) / (480 * width)
                let coef =
                    (640.0 * self.config.height as f32) / (480.0 * self.config.width.max(1) as f32);
                match draw.anchor {
                    crate::fx::draw::ScreenFxAnchor::Center => x = 320.0 + (x - 320.0) * coef,
                    crate::fx::draw::ScreenFxAnchor::Right => x = 640.0 - (640.0 - x) * coef,
                    crate::fx::draw::ScreenFxAnchor::Stretch => {}
                }
                w *= coef;
            }
            let x0 = x / 640.0 * 2.0 - 1.0;
            let x1 = (x + w) / 640.0 * 2.0 - 1.0;
            let y0 = 1.0 - y / 480.0 * 2.0;
            let y1 = 1.0 - (y + h) / 480.0 * 2.0;
            let color = draw.material.float_vertex_color(draw.color);
            let [u0, v0, u1, v1] = draw.uv_rect;
            let vertex = |position, uv| UiVertex {
                position,
                uv,
                color,
                textured: 1.0,
            };
            let start = vertices.len() as u32;
            vertices.extend_from_slice(&[
                vertex([x0, y0], [u0, v0]),
                vertex([x0, y1], [u0, v1]),
                vertex([x1, y1], [u1, v1]),
                vertex([x0, y0], [u0, v0]),
                vertex([x1, y1], [u1, v1]),
                vertex([x1, y0], [u1, v0]),
            ]);
            self.screen_fx_batches.push(ScreenFxBatch {
                range: start..start + 6,
                blend: draw.material.blend,
                texture_key,
            });
        }
        self.screen_fx_vertex_count = vertices.len() as u32;
        if !vertices.is_empty() {
            self.queue.write_buffer(
                &self.screen_fx_vertex_buffer,
                0,
                bytemuck::cast_slice(&vertices),
            );
        }
    }

    pub(in crate::renderer) fn set_dynamic_hud(
        &mut self,
        hud: Option<ui::HudState>,
        movement_hud: ui::MovementHudState,
    ) {
        let hud_changed = self.ui_state.hud != hud;
        let movement_changed = self.ui_state.movement_hud != movement_hud;
        if !hud_changed && !movement_changed {
            return;
        }
        self.ui_state.hud = hud;
        self.ui_state.movement_hud = movement_hud;
        if hud_changed {
            // HUD text/geometry can change size, so rebuild the stable prefix and
            // then append the current movement tail.
            self.rebuild_dynamic_ui();
        } else {
            // The common path: only velocity/input/view state changed. Preserve
            // the stable prefix and rewrite the tiny movement tail in place.
            self.rebuild_movement_ui();
        }
    }

    pub(in crate::renderer) fn set_transient_ui(
        &mut self,
        chat_lines: Vec<ui::UiChatLine>,
        console_notify_lines: Vec<ui::UiConsoleNotifyLine>,
        center_print: Option<ui::UiCenterPrint>,
        demo_timeline: Option<ui::DemoTimelineUi>,
        prediction_debug: Option<ui::PredictionDebugUi>,
        crosshair_target: ui::UiCrosshairTarget,
        force_select: Option<ui::UiForceSelect>,
        weapon_select: Option<ui::UiWeaponSelect>,
        follow_name: Option<String>,
        game_timer: Option<String>,
        mini_scores: Option<ui::UiMiniScores>,
        race_timer: Option<crate::japro_cg::RaceTimerUi>,
        vote_line: Option<String>,
        scoreboard: Option<ui::UiScoreboard>,
        scoreboard_focus_client: Option<i32>,
        speedometer: Option<crate::speedometer::Ui>,
        lagometer: Option<crate::lagometer::Ui>,
        team_overlay: Option<ui::TeamOverlayUi>,
    ) {
        self.ui_state.chat_lines = chat_lines;
        self.ui_state.console_notify_lines = console_notify_lines;
        self.ui_state.center_print = center_print;
        self.ui_state.demo_timeline = demo_timeline;
        self.ui_state.prediction_debug = prediction_debug;
        self.ui_state.crosshair_target = crosshair_target;
        self.ui_state.force_select = force_select;
        self.ui_state.weapon_select = weapon_select;
        self.ui_state.follow_name = follow_name;
        self.ui_state.game_timer = game_timer;
        self.ui_state.mini_scores = mini_scores;
        self.ui_state.race_timer = race_timer;
        self.ui_state.vote_line = vote_line;
        let scoreboard_changed = self.ui_state.scoreboard != scoreboard
            || self.ui_state.scoreboard_focus_client != scoreboard_focus_client;
        self.ui_state.scoreboard = scoreboard;
        self.ui_state.scoreboard_focus_client = scoreboard_focus_client;
        self.ui_state.speedometer = speedometer;
        self.ui_state.lagometer = lagometer;
        self.ui_state.team_overlay = team_overlay;
        self.load_ui_icons_if_needed();
        self.sync_ui_team_icons();
        self.sync_ui_extension_images();
        if scoreboard_changed {
            // Scoreboard visibility and focus are transient state. Rebuild its
            // retained batch only when either input changes.
            self.rebuild_retained_ui();
        }
        self.rebuild_transient_ui();
    }

    pub(in crate::renderer) fn set_ui_telemetry(
        &mut self,
        perf: ui::PerfStats,
        threads: std::sync::Arc<[ui::ThreadPerfStats]>,
    ) {
        self.ui_state.perf = perf;
        self.ui_state.threads = threads;
        // Telemetry is part of the small volatile overlay batch. Rebuilding it
        // never touches retained console/menu/scoreboard geometry.
        if self.ui_state.video.draw_fps != 0 {
            self.rebuild_transient_ui();
        }
    }

    pub(in crate::renderer) fn set_egui(&mut self, frame: Option<EguiRenderData>) {
        let Some(mut frame) = frame else {
            self.egui_active = false;
            self.egui_present_token = None;
            self.egui_paint_jobs.clear();
            for texture_id in self.egui_pending_free.drain(..) {
                self.egui_renderer.free_texture(&texture_id);
            }
            return;
        };

        for (texture_id, image_deltas) in &frame.textures_delta.set {
            for image_delta in image_deltas {
                self.egui_renderer
                    .update_texture(&self.device, &self.queue, *texture_id, image_delta);
            }
        }
        self.egui_pending_free.extend(frame.textures_delta.free.iter().copied());
        frame.textures_delta.clear();
        self.egui_paint_jobs = std::mem::take(&mut frame.paint_jobs);
        self.egui_pixels_per_point = frame.pixels_per_point.max(0.25);
        self.egui_active = true;
        self.egui_present_token = frame.present_token;
    }

    pub(in crate::renderer) fn submit_egui(&mut self, frame_view: &wgpu::TextureView) {
        if !self.egui_active || self.egui_paint_jobs.is_empty() {
            return;
        }

        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width.max(1), self.config.height.max(1)],
            pixels_per_point: self.egui_pixels_per_point,
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("JKA egui overlay encoder"),
            });
        let callback_command_buffers = self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &self.egui_paint_jobs,
            &screen,
        );
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("JKA egui overlay"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: self.hdr_output.as_ref().map_or(frame_view, |output| &output.ui_view),
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if self.hdr_output.is_some() { wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT) } else { wgpu::LoadOp::Load },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            self.egui_renderer
                .render(&mut pass, &self.egui_paint_jobs, &screen);
        }
        if let Some(output) = &self.hdr_output {
            output.composite_ui(&mut encoder, frame_view);
        }
        let egui_command_buffer = encoder.finish();
        self.queue.submit(
            callback_command_buffers
                .into_iter()
                .chain(std::iter::once(egui_command_buffer)),
        );

        for texture_id in self.egui_pending_free.drain(..) {
            self.egui_renderer.free_texture(&texture_id);
        }
    }

    pub(in crate::renderer) fn menu_backdrop_requested(&self) -> bool {
        // The frontend Main scene is artwork, not a page that needs a blurred
        // and desaturated background. Avoid both the grading and its extra pass.
        // It does use the same lazily allocated scene capture as the source of
        // the renderer-drawn main page (neon_menu.rs).
        self.neon_menu_wanted()
            || (!self.ui_state.frontend_main_scene
                && !self.ui_state.video.skip_ui
                && self.ui_state.loading.is_none()
                && (self.ui_state.mode == ui::OverlayMode::Game
                    || (self.ui_state.mode == ui::OverlayMode::Video
                        && self.ui_state.setup_selected != 1)))
    }

    /// The scene may render into the backdrop source this frame. The main page
    /// waits for its own pipelines too, rendering straight to the frame until then.
    pub(in crate::renderer) fn menu_backdrop_ready(&self) -> bool {
        self.menu_backdrop.is_some() && (!self.neon_menu_wanted() || self.neon_menu_ready())
    }

    pub(in crate::renderer) fn ensure_menu_backdrop_resources(&mut self) {
        let width = self.config.width.max(1);
        let height = self.config.height.max(1);
        let format = self.config.format;
        if self
            .menu_backdrop
            .as_ref()
            .is_some_and(|resources| resources.width == width && resources.height == height)
        {
            return;
        }
        // Never use an old-size backdrop while its replacement is compiling.
        self.menu_backdrop = None;
        let key = PipelineJobKey::new("menu-backdrop", 0, pipeline_hash(&(width, height, format)));
        if let Some(resources) = self.pipeline_jobs.take_ready::<MenuBackdropResources>(key) {
            self.menu_backdrop = Some(resources);
            return;
        }
        let device = self.device.clone();
        let sampler = self.post_sampler.clone();
        self.pipeline_jobs.request(key, "menu backdrop", move || {
            create_menu_backdrop_resources(&device, &sampler, width, height, format)
        });
    }

    /// `plain_scene`: the frame of a cached-scene capture, which must not
    /// contain the main page's overlays.
    pub(in crate::renderer) fn submit_menu_backdrop(&self, frame_view: &wgpu::TextureView, plain_scene: bool) {
        if self.neon_menu_wanted() && self.submit_neon_menu(frame_view, plain_scene) {
            return;
        }
        let Some(backdrop) = &self.menu_backdrop else {
            return;
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("JKA menu backdrop encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("JKA menu Gaussian backdrop"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: frame_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&backdrop.pipeline);
            pass.set_bind_group(0, &backdrop.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
    }

    /// The renderer-drawn overlay (frame rate readout, perf graphs, surface
    /// inspector, background bake progress) as its own submit.
    ///
    pub(in crate::renderer) fn draw_ui_batches<'a>(
        pass: &mut wgpu::RenderPass<'a>,
        ui_pipeline: &'a wgpu::RenderPipeline,
        ui_bind_group: &'a wgpu::BindGroup,
        dynamic_buffer: &'a wgpu::Buffer,
        dynamic_vertex_count: u32,
        transient_buffer: &'a wgpu::Buffer,
        transient_vertex_count: u32,
        retained_buffer: &'a wgpu::Buffer,
        retained_vertex_count: u32,
    ) {
        pass.set_pipeline(ui_pipeline);
        pass.set_bind_group(0, ui_bind_group, &[]);
        // Draw the gameplay HUD, transient chat/center-print and telemetry, then
        // retained foreground UI such as the scoreboard, console, and inspector.
        if dynamic_vertex_count != 0 {
            pass.set_vertex_buffer(0, dynamic_buffer.slice(..));
            pass.draw(0..dynamic_vertex_count, 0..1);
        }
        if transient_vertex_count != 0 {
            pass.set_vertex_buffer(0, transient_buffer.slice(..));
            pass.draw(0..transient_vertex_count, 0..1);
        }
        if retained_vertex_count != 0 {
            pass.set_vertex_buffer(0, retained_buffer.slice(..));
            pass.draw(0..retained_vertex_count, 0..1);
        }
    }

    pub(in crate::renderer) fn draw_screen_fx_batches<'a>(
        pass: &mut wgpu::RenderPass<'a>,
        vertex_buffer: &'a wgpu::Buffer,
        vertex_count: u32,
        batches: &'a [ScreenFxBatch],
        pipelines: &'a HashMap<FxBlend, wgpu::RenderPipeline>,
        textures: &'a HashMap<String, ScreenFxTexture>,
        white_bind_group: &'a wgpu::BindGroup,
    ) {
        if vertex_count == 0 {
            return;
        }
        pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        for batch in batches {
            let Some(pipeline) = pipelines.get(&batch.blend) else {
                continue;
            };
            let bind_group = batch
                .texture_key
                .as_ref()
                .and_then(|key| textures.get(key))
                .map_or(white_bind_group, |texture| &texture.bind_group);
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind_group, &[]);
            pass.draw(batch.range.clone(), 0..1);
        }
    }

    /// With a menu open this runs *after* egui so the readout stays on top of
    /// the menu instead of being covered by it: these are status displays the
    /// player is usually watching while changing a setting.
    pub(in crate::renderer) fn submit_ui_overlay(&self, frame_view: &wgpu::TextureView) {
        if self.screen_fx_vertex_count == 0
            && self.ui_vertex_count == 0
            && self.ui_dynamic_vertex_count == 0
            && self.ui_transient_vertex_count == 0
            && !(self.frame_time_graph.draw_ready() && !self.egui_active)
        {
            return;
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("JKA UI overlay encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("JKA UI overlay"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: frame_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            // OpenJK draws CG_SaberClashFlare in CG_Draw2D before the ordinary
            // HUD status elements. Keep that ordering and its authored shader
            // blend, while leaving retained UI/console on top.
            Self::draw_screen_fx_batches(
                &mut pass,
                &self.screen_fx_vertex_buffer,
                self.screen_fx_vertex_count,
                &self.screen_fx_batches,
                &self.screen_fx_pipelines,
                &self.screen_fx_textures,
                &self.screen_fx_white_bind_group,
            );

            Self::draw_ui_batches(
                &mut pass,
                &self.ui_pipeline,
                &self.ui_bind_group,
                &self.ui_dynamic_vertex_buffer,
                self.ui_dynamic_vertex_count,
                &self.ui_transient_vertex_buffer,
                self.ui_transient_vertex_count,
                &self.ui_vertex_buffer,
                self.ui_vertex_count,
            );
            if !self.egui_active {
                self.frame_time_graph.draw(&mut pass, &self.ui_pipeline, &self.ui_bind_group);
            }
        }
        self.queue.submit([encoder.finish()]);
    }

    pub(in crate::renderer) fn rebuild_ui(&mut self) {
        self.rebuild_retained_ui();
        self.rebuild_dynamic_ui();
        self.rebuild_transient_ui();
    }

    pub(in crate::renderer) fn rebuild_retained_ui(&mut self) {
        let trace_started = self.ui_trace.then(Instant::now);
        if self.ui_state.video.skip_ui {
            self.ui_vertex_count = 0;
            return;
        }
        let mut vertices = ui::build_vertices(
            &self.ui_state,
            self.config.width,
            self.config.height,
            self.ui_small_font_metrics.as_ref(),
            self.ui_splash_size,
        );
        let max_vertices = UI_BUFFER_BYTES as usize / std::mem::size_of::<UiVertex>();
        if vertices.len() > max_vertices {
            vertices.truncate(max_vertices - (max_vertices % 3));
        }
        self.ui_vertex_count = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
        if self.ui_vertex_count != 0 {
            self.queue
                .write_buffer(&self.ui_vertex_buffer, 0, bytemuck::cast_slice(&vertices));
        }
        if let Some(started) = trace_started {
            rverbose!(
                2,
                "[JKA UI] retained rebuild {:.3} ms, {} vertices ({} triangles)",
                started.elapsed().as_secs_f64() * 1000.0,
                self.ui_vertex_count,
                self.ui_vertex_count / 3
            );
        }
    }

    pub(in crate::renderer) fn rebuild_dynamic_ui(&mut self) {
        let trace_started = self.ui_trace.then(Instant::now);
        self.ui_dynamic_vertices.clear();
        self.ui_dynamic_stable_vertex_count = 0;
        if self.ui_state.video.skip_ui {
            self.ui_dynamic_vertex_count = 0;
            return;
        }

        // Vignette-only mode is a few quads in the same gameplay batch. Keeping
        // it in the stable prefix preserves the original painter order.
        if self.vignette_enabled && !self.frame_plan.use_post {
            append_vignette_vertices(&mut self.ui_dynamic_vertices);
        }
        ui::build_dynamic_static_vertices(
            &mut self.ui_dynamic_vertices,
            &self.ui_state,
            self.config.width,
            self.config.height,
        );
        let max_vertices = UI_DYNAMIC_BUFFER_BYTES as usize / std::mem::size_of::<UiVertex>();
        if self.ui_dynamic_vertices.len() > max_vertices {
            self.ui_dynamic_vertices
                .truncate(max_vertices - (max_vertices % 3));
        }
        self.ui_dynamic_stable_vertex_count = self.ui_dynamic_vertices.len();
        ui::build_movement_vertices(
            &mut self.ui_dynamic_vertices,
            &self.ui_state,
            self.config.width,
            self.config.height,
        );
        if self.ui_dynamic_vertices.len() > max_vertices {
            self.ui_dynamic_vertices
                .truncate(max_vertices - (max_vertices % 3));
        }
        self.ui_dynamic_vertex_count =
            u32::try_from(self.ui_dynamic_vertices.len()).unwrap_or(u32::MAX);
        if self.ui_dynamic_vertex_count != 0 {
            self.queue.write_buffer(
                &self.ui_dynamic_vertex_buffer,
                0,
                bytemuck::cast_slice(&self.ui_dynamic_vertices),
            );
        }
        if let Some(started) = trace_started {
            rverbose!(
                2,
                "[JKA UI] gameplay rebuild {:.3} ms, {} stable + {} hot vertices",
                started.elapsed().as_secs_f64() * 1000.0,
                self.ui_dynamic_stable_vertex_count,
                self.ui_dynamic_vertices
                    .len()
                    .saturating_sub(self.ui_dynamic_stable_vertex_count)
            );
        }
    }

    pub(in crate::renderer) fn rebuild_movement_ui(&mut self) {
        let trace_started = self.ui_trace.then(Instant::now);
        if self.ui_state.video.skip_ui {
            self.ui_dynamic_vertex_count = 0;
            return;
        }
        let max_vertices = UI_DYNAMIC_BUFFER_BYTES as usize / std::mem::size_of::<UiVertex>();
        let stable = self
            .ui_dynamic_stable_vertex_count
            .min(self.ui_dynamic_vertices.len())
            .min(max_vertices);
        self.ui_dynamic_vertices.truncate(stable);
        ui::build_movement_vertices(
            &mut self.ui_dynamic_vertices,
            &self.ui_state,
            self.config.width,
            self.config.height,
        );
        if self.ui_dynamic_vertices.len() > max_vertices {
            self.ui_dynamic_vertices
                .truncate(max_vertices - (max_vertices % 3));
        }
        self.ui_dynamic_vertex_count =
            u32::try_from(self.ui_dynamic_vertices.len()).unwrap_or(u32::MAX);

        // The stable prefix is already resident on the GPU. Upload only the tail
        // that changed with this input/simulation snapshot.
        let tail = &self.ui_dynamic_vertices[stable..];
        if !tail.is_empty() {
            let byte_offset = (stable * std::mem::size_of::<UiVertex>()) as u64;
            self.queue.write_buffer(
                &self.ui_dynamic_vertex_buffer,
                byte_offset,
                bytemuck::cast_slice(tail),
            );
        }
        if let Some(started) = trace_started {
            rverbose!(
                2,
                "[JKA UI] movement tail {:.3} ms, {} vertices / {} bytes",
                started.elapsed().as_secs_f64() * 1000.0,
                tail.len(),
                tail.len() * std::mem::size_of::<UiVertex>()
            );
        }
    }

    pub(in crate::renderer) fn rebuild_transient_ui(&mut self) {
        let trace_started = self.ui_trace.then(Instant::now);
        self.ui_transient_vertices.clear();
        if self.ui_state.video.skip_ui {
            self.ui_transient_vertex_count = 0;
            self.ui_transient_stable_vertex_count = 0;
            return;
        }
        ui::build_transient_vertices(
            &mut self.ui_transient_vertices,
            &self.ui_state,
            self.ui_small_font_metrics.as_ref(),
            self.config.width,
            self.config.height,
        );
        let max_vertices = UI_TRANSIENT_BUFFER_BYTES as usize / std::mem::size_of::<UiVertex>();
        if self.ui_transient_vertices.len() > max_vertices {
            self.ui_transient_vertices
                .truncate(max_vertices - (max_vertices % 3));
        }
        self.ui_transient_stable_vertex_count = self.ui_transient_vertices.len();
        self.ui_transient_vertex_count =
            u32::try_from(self.ui_transient_vertices.len()).unwrap_or(u32::MAX);
        // The stable part changed, so the tail's resident copy is no longer known.
        self.ui_transient_tail_uploaded = None;
        if self.ui_transient_vertex_count != 0 {
            self.queue.write_buffer(
                &self.ui_transient_vertex_buffer,
                0,
                bytemuck::cast_slice(&self.ui_transient_vertices),
            );
        }
        if let Some(started) = trace_started {
            rverbose!(
                2,
                "[JKA UI] transient rebuild {:.3} ms, {} vertices",
                started.elapsed().as_secs_f64() * 1000.0,
                self.ui_transient_vertex_count
            );
        }
    }

    pub(in crate::renderer) fn rebuild_player_name_tail(
        &mut self,
        view_proj: Mat4,
        dynamic_crosshair_world: Option<[f32; 3]>,
    ) {
        self.ui_transient_vertices
            .truncate(self.ui_transient_stable_vertex_count);
        if self.ui_state.video.skip_ui {
            self.ui_transient_vertex_count = 0;
            return;
        }

        // TaystJK keeps the crosshair hit in world space until draw time. Do the
        // same here, after the renderer samples the newest subframe view. The endpoint
        // and aim angles arrive as one LatestViewState sample, so this projection uses
        // the exact camera that produced that hit rather than a previous-frame point.
        let tail_start = self.ui_transient_vertices.len();
        // Projected player names and the late-latched crosshair are screen-space
        // CG_Draw2D, even though their vertices are appended after HUD publishing.
        // Check the snapshot boolean once; no cvar lookup in the render loop.
        if self.ui_state.video.draw_2d {
            if ui::gameplay_hud_visible(&self.ui_state, self.config.width, self.config.height) {
                if let Some(anchor) = dynamic_crosshair_world {
                    let clip = view_proj * Vec3::from_array(anchor).extend(1.0);
                    if clip.w > 0.001 {
                        let ndc = clip.truncate() / clip.w;
                        if (0.0..=1.0).contains(&ndc.z) && ndc.x.abs() <= 1.15 && ndc.y.abs() <= 1.15 {
                            let x = (ndc.x * 0.5 + 0.5) * self.config.width.max(1) as f32;
                            let y = (0.5 - ndc.y * 0.5) * self.config.height.max(1) as f32;
                            ui::build_crosshair_vertices(
                                &mut self.ui_transient_vertices,
                                &self.ui_state,
                                Some([x, y]),
                                self.config.width,
                                self.config.height,
                            );
                        }
                    }
                }
            }

            if let Some(player_names) = self.player_names.as_ref() {
                for label in player_names.labels.iter() {
                    let clip = view_proj * Vec3::from_array(label.anchor).extend(1.0);
                    if clip.w <= 0.001 {
                        continue;
                    }
                    let ndc = clip.truncate() / clip.w;
                    if !(0.0..=1.0).contains(&ndc.z) || ndc.x.abs() > 1.15 || ndc.y.abs() > 1.15 {
                        continue;
                    }
                    let x = (ndc.x * 0.5 + 0.5) * self.config.width.max(1) as f32;
                    let y = (0.5 - ndc.y * 0.5) * self.config.height.max(1) as f32;
                    ui::build_world_player_label_vertices(
                        &mut self.ui_transient_vertices,
                        label,
                        x,
                        y,
                        player_names.scale,
                        self.ui_small_font_metrics.as_ref(),
                        self.config.width,
                        self.config.height,
                    );
                }
            }
        }

        let max_vertices = UI_TRANSIENT_BUFFER_BYTES as usize / std::mem::size_of::<UiVertex>();
        if self.ui_transient_vertices.len() > max_vertices {
            self.ui_transient_vertices
                .truncate(max_vertices - (max_vertices % 3));
        }
        self.ui_transient_vertex_count =
            u32::try_from(self.ui_transient_vertices.len()).unwrap_or(u32::MAX);
        let tail = &self.ui_transient_vertices[tail_start.min(self.ui_transient_vertices.len())..];
        if !tail.is_empty() {
            let byte_offset = (tail_start * std::mem::size_of::<UiVertex>()) as u64;
            let bytes: &[u8] = bytemuck::cast_slice(tail);
            // A static crosshair re-sends the same vertices every frame; each
            // queue write is its own GPU copy (~9 us on a UHD 630).
            let unchanged = self.ui_transient_tail_uploaded.as_ref()
                .is_some_and(|(offset, uploaded)| *offset == byte_offset && uploaded.as_slice() == bytes);
            if !unchanged {
                self.queue.write_buffer(&self.ui_transient_vertex_buffer, byte_offset, bytes);
                let (offset, uploaded) = self.ui_transient_tail_uploaded
                    .get_or_insert_with(|| (byte_offset, Vec::new()));
                *offset = byte_offset;
                uploaded.clear();
                uploaded.extend_from_slice(bytes);
            }
        }
    }
}

/// Place images on shelves, largest-first. Each icon gets a one-texel
/// extruded border, which gives linear filtering the same edge clamp as a
/// standalone GL texture. No fixed 64/256 cell or resampling is involved.
fn pack_native_icon_shelves(
    images: &[Option<image::RgbaImage>],
    width: u32,
    max_height: u32,
) -> Option<(u32, Vec<(u32, u32)>)> {
    let mut order: Vec<usize> = (0..images.len())
        .filter(|&i| images[i].is_some())
        .collect();
    order.sort_unstable_by_key(|&i| {
        let im = images[i].as_ref().unwrap();
        (std::cmp::Reverse(im.height()), std::cmp::Reverse(im.width()), i)
    });
    let mut coords = vec![(0, 0); images.len()];
    let mut x: u32 = 2;
    let mut y: u32 = 2;
    let mut row_height: u32 = 0;
    for i in order {
        let image = images[i].as_ref().unwrap();
        let (rw, rh) = (image.width() + 2, image.height() + 2);
        if rw + 4 > width || rh + 4 > max_height { return None; }
        if x + rw + 2 > width {
            y += row_height;
            x = 2;
            row_height = 0;
        }
        if y + rh + 2 > max_height { return None; }
        coords[i] = (x + 1, y + 1);
        x += rw;
        row_height = row_height.max(rh);
    }
    Some(((y + row_height + 2).max(4), coords))
}

/// Keep every HUD icon at its decoded native width and height. Only shrink an
/// exceptional oversized image when GPU texture-size or memory limits force it;
/// unlike the old fixed-cell atlas, ordinary 32px, 128px, 512px and HD art
/// are copied byte-for-byte into this texture. Returns both pixels and per-icon
/// UVs so weapons, Force icons, crosshairs and the lagometer share one binding.
fn assemble_native_icon_atlas(
    mut images: Vec<Option<image::RgbaImage>>,
    max_texture_size: u32,
) -> (TextureData, Vec<([f32; 2], [f32; 2])>) {
    // Cap to a broadly supported size and to 128MiB of uncompressed atlas
    // memory. Individual images remain native unless packing cannot fit.
    let limit = max_texture_size.min(8192).max(8);
    const MAX_PIXELS: u64 = 32 * 1024 * 1024;
    let mut reductions = 0;
    for (i, opt) in images.iter_mut().enumerate() {
        let Some(image) = opt.as_mut() else { continue; };
        let (w, h) = image.dimensions();
        if w + 6 <= limit && h + 6 <= limit { continue; }
        let factor = ((limit - 6) as f64 / w as f64)
            .min((limit - 6) as f64 / h as f64)
            .min(1.0);
        let (nw, nh) = (((w as f64 * factor).floor() as u32).max(1),
                        ((h as f64 * factor).floor() as u32).max(1));
        eprintln!("UI icon {}: {}x{} exceeds GPU atlas limit, shrinking to {}x{}", i, w, h, nw, nh);
        *image = image::imageops::resize(&*image, nw, nh, image::imageops::FilterType::Lanczos3);
        reductions += 1;
    }
    let (width, height, positions) = loop {
        let image_area = images.iter().flatten().map(|im| {
            u64::from(im.width() + 2) * u64::from(im.height() + 2)
        }).sum::<u64>();
        let widest = images.iter().flatten().map(|im| im.width() + 6).max().unwrap_or(4);
        let desired = (image_area as f64).sqrt().ceil() as u32;
        let mut width = desired.max(widest).max(4).next_power_of_two().min(limit);
        let mut packed = None;
        loop {
            if let Some((height, positions)) = pack_native_icon_shelves(&images, width, limit) {
                if u64::from(width) * u64::from(height) <= MAX_PIXELS {
                    packed = Some((width, height, positions));
                    break;
                }
            }
            if width == limit { break; }
            width = width.saturating_mul(2).min(limit);
        }
        if let Some(layout) = packed { break layout; }
        // Real texture constraints, not an arbitrary icon-size target, are the
        // only reason we ever resample. Reduce the largest icon first.
        let Some((i, _)) = images.iter().enumerate().filter_map(|(i, im)| {
            im.as_ref().map(|im| (i, u64::from(im.width()) * u64::from(im.height())))
        }).filter(|&(_, area)| area > 1).max_by_key(|&(_, area)| area) else {
            eprintln!("UI icon atlas: cannot pack icons within texture limits; using transparent placeholder");
            break (4, 4, vec![(0, 0); images.len()]);
        };
        let im = images[i].as_ref().unwrap();
        let (w, h) = im.dimensions();
        let (nw, nh) = ((w * 3 / 4).max(1), (h * 3 / 4).max(1));
        eprintln!("UI icon {}: atlas capacity exceeded, shrinking {}x{} to {}x{}", i, w, h, nw, nh);
        images[i] = Some(image::imageops::resize(im, nw, nh, image::imageops::FilterType::Lanczos3));
        reductions += 1;
    };

    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    let mut uvs = vec![([0.5 / width as f32, 0.5 / height as f32],
                        [0.5 / width as f32, 0.5 / height as f32]); images.len()];
    for (i, opt) in images.iter().enumerate() {
        let Some(im) = opt.as_ref() else { continue; };
        let (x, y) = positions[i];
        if x == 0 || y == 0 { continue; }
        let (w, h) = im.dimensions();
        // Duplicate the outside texel into a one-pixel gutter on all four
        // sides. Bilinear filtering can't bleed from neighbouring artwork.
        for dy in 0..(h + 2) {
            for dx in 0..(w + 2) {
                let src_x = dx.saturating_sub(1).min(w - 1);
                let src_y = dy.saturating_sub(1).min(h - 1);
                let px = im.get_pixel(src_x, src_y);
                let offset = (((y - 1 + dy) * width + (x - 1 + dx)) * 4) as usize;
                rgba[offset..offset + 4].copy_from_slice(&px.0);
            }
        }
        uvs[i] = ([x as f32 / width as f32, y as f32 / height as f32],
                  [(x + w) as f32 / width as f32, (y + h) as f32 / height as f32]);
    }
    rverbose!(1, "UI icon atlas: {}x{}, {} native-size icons, {} GPU-limit reductions",
        width, height, images.iter().flatten().count(), reductions);
    (TextureData {
        label: "JKA UI native-resolution icon atlas".into(),
        source: None, width, height, rgba, rgba16f: None,
        mip_level_count: 1, clamp: true, srgb: true,
    }, uvs)
}

pub(in crate::renderer) fn load_ui_icon_atlas(
    base: &Path,
    game: Option<&Path>,
    max_texture_size: u32,
) -> (TextureData, Vec<([f32; 2], [f32; 2])>) {
    let mut images = vec![None; ui::ICON_NAMES.len()];
    match AssetSearchPath::open_game(base, game) {
        Ok(mut assets) => {
            for (index, name) in ui::ICON_NAMES.iter().enumerate() {
                if name.is_empty() { continue; }
                let requested = if (ui::ICON_WEAPON_NO_AMMO_BASE..ui::ICON_WEAPON_STAFF).contains(&index) {
                    format!("{name}_na")
                } else {
                    (*name).to_owned()
                };
                let mut image = try_load_texture_asset_from_search_path(
                    &mut assets, &requested, true, false, true, "UI icon",
                );
                if image.is_none() && index >= ui::ICON_WEAPON_NO_AMMO_BASE {
                    let fallback = if index >= ui::ICON_WEAPON_STAFF {
                        "gfx/hud/w_icon_lightsaber"
                    } else { name };
                    image = try_load_texture_asset_from_search_path(
                        &mut assets, fallback, true, false, true, "UI icon fallback",
                    );
                }
                let Some(image) = image else {
                    eprintln!("UI icon {requested}: not found");
                    continue;
                };
                images[index] = image::RgbaImage::from_raw(image.width, image.height, image.rgba);
                if images[index].is_none() {
                    eprintln!("UI icon {requested}: unexpected pixel format");
                }
            }
        }
        Err(error) => eprintln!("UI icon asset search: {error}"),
    }
    assemble_native_icon_atlas(images, max_texture_size)
}

#[cfg(test)]
mod native_icon_atlas_tests {
    use super::*;

    #[test]
    fn different_resolution_icons_are_copied_exactly_without_resampling() {
        let sizes = [(32, 16), (512, 256), (93, 71), (64, 64)];
        let mut icons = vec![None; sizes.len() + 1];
        for (i, (w, h)) in sizes.iter().copied().enumerate() {
            icons[i] = Some(image::RgbaImage::from_fn(w, h, |x, y| {
                image::Rgba([i as u8 * 13 + (x % 37) as u8,
                             (y % 251) as u8, (x % 253) as u8, 255])
            }));
        }
        let (atlas, uvs) = assemble_native_icon_atlas(icons.clone(), 2048);
        assert_eq!(uvs.len(), icons.len());
        assert_eq!(atlas.mip_level_count, 1);
        assert!(atlas.width <= 2048 && atlas.height <= 2048);
        for (i, icon) in icons.iter().enumerate() {
            let Some(im) = icon.as_ref() else { continue; };
            let (uv0, uv1) = uvs[i];
            let (x, y) = ((uv0[0] * atlas.width as f32).round() as u32,
                          (uv0[1] * atlas.height as f32).round() as u32);
            assert_eq!(((uv1[0] - uv0[0]) * atlas.width as f32).round() as u32, im.width());
            assert_eq!(((uv1[1] - uv0[1]) * atlas.height as f32).round() as u32, im.height());
            for yy in 0..im.height() {
                for xx in 0..im.width() {
                    let o = (((y + yy) * atlas.width + x + xx) * 4) as usize;
                    assert_eq!(&atlas.rgba[o..o + 4], &im.get_pixel(xx, yy).0[..]);
                }
            }
            // The extruded border is an exact copy of the first texel.
            let gutter = (((y - 1) * atlas.width + x - 1) * 4) as usize;
            assert_eq!(&atlas.rgba[gutter..gutter + 4], &im.get_pixel(0, 0).0[..]);
        }
        assert_eq!(uvs[icons.len() - 1].0, uvs[icons.len() - 1].1);
    }

    #[test]
    fn scales_only_when_hardware_constraints_prevent_native_packing() {
        let images = vec![Some(image::RgbaImage::new(256, 128))];
        let (atlas, uvs) = assemble_native_icon_atlas(images, 128);
        let size = ((uvs[0].1[0] - uvs[0].0[0]) * atlas.width as f32).round() as u32;
        assert!(size < 256);
        assert!(atlas.width <= 128 && atlas.height <= 128);
    }
}

/// Full-size, fully transparent extension image atlas: `MAX_HUD_IMAGES` cells of
/// `ui::EXT_IMAGE_CELL` squares, `ui::EXT_IMAGE_COLUMNS` per row. Allocated once;
/// images are written into their cells as they are first needed.
pub(in crate::renderer) fn blank_ui_extension_atlas() -> TextureData {
    let cell = ui::EXT_IMAGE_CELL;
    let rows = (crate::extensions::MAX_HUD_IMAGES as u32).div_ceil(ui::EXT_IMAGE_COLUMNS);
    let (width, height) = (ui::EXT_IMAGE_COLUMNS * cell, rows * cell);
    TextureData {
        label: "JKA UI extension image atlas".into(),
        source: None,
        width,
        height,
        rgba: vec![0u8; (width * height * 4) as usize],
        rgba16f: None,
        mip_level_count: 1,
        clamp: true,
        srgb: true,
    }
}

/// Decode extension `hud_image()` pictures into `ui::EXT_IMAGE_CELL` squares
/// (RGBA8, `None` when the image is missing). Images are stretched to the cell and
/// drawn with the whole cell as UVs, so their aspect ratio survives.
pub(in crate::renderer) fn load_ui_extension_images(
    base: &Path,
    game: Option<&Path>,
    names: &[String],
) -> Vec<Option<Vec<u8>>> {
    let cell = ui::EXT_IMAGE_CELL;
    let mut assets = match AssetSearchPath::open_game(base, game) {
        Ok(assets) => assets,
        Err(error) => {
            eprintln!("Extension HUD image asset search: {error}");
            return vec![None; names.len()];
        }
    };
    names
        .iter()
        .map(|name| {
            let Some(image) = try_load_texture_asset_from_search_path(
                &mut assets,
                name,
                true,
                false,
                true,
                "extension HUD image",
            ) else {
                eprintln!("Extension HUD image {name}: not found");
                return None;
            };
            let (source_w, source_h) = (image.width, image.height);
            let Some(source) = image::RgbaImage::from_raw(source_w, source_h, image.rgba) else {
                eprintln!("Extension HUD image {name}: unexpected pixel format");
                return None;
            };
            Some(if source_w == cell && source_h == cell {
                source.into_raw()
            } else {
                image::imageops::resize(&source, cell, cell, image::imageops::FilterType::Triangle)
                    .into_raw()
            })
        })
        .collect()
}

/// Dynamic atlas for CG_DrawTeamOverlay model, weapon and powerup icons.
/// Each cell key may contain newline-separated candidate qpaths; the first
/// existing image wins (team-skin icon -> team fallback -> defer art).
pub(in crate::renderer) fn load_ui_team_icon_atlas(
    base: &Path,
    game: Option<&Path>,
    keys: &[String],
) -> TextureData {
    let cell = ui::TEAM_ICON_CELL;
    let columns = ui::TEAM_ICON_COLUMNS.max(1);
    let rows = ((keys.len() as u32 + columns - 1) / columns).max(1);
    let (width, height) = (columns * cell, rows * cell);
    let mut rgba = vec![0u8; (width * height * 4) as usize];

    match AssetSearchPath::open_game(base, game) {
        Ok(mut assets) => {
            for (index, key) in keys.iter().enumerate() {
                let mut loaded = None;
                for name in key.split('\n').filter(|name| !name.is_empty()) {
                    let resolved;
                    let qpath = if let Some(spec) = name.strip_prefix("@team-model-icon\t") {
                        let mut fields = spec.splitn(5, '\t');
                        let (
                            Some(model),
                            Some(skin),
                            Some(team),
                            Some(gametype),
                            Some(jedi_v_merc),
                        ) = (
                            fields.next(),
                            fields.next(),
                            fields.next(),
                            fields.next(),
                            fields.next(),
                        )
                        else {
                            continue;
                        };
                        let (Ok(team), Ok(gametype)) =
                            (team.parse::<i32>(), gametype.parse::<i32>())
                        else {
                            continue;
                        };
                        let mut final_skin = skin.to_owned();
                        if gametype >= crate::cgame::GT_TEAM
                            && gametype != crate::cgame::GT_SIEGE
                            && jedi_v_merc != "1"
                        {
                            let _ = crate::cgame::validate_skin_for_team(
                                model,
                                &mut final_skin,
                                team,
                                |candidate| assets.contains_qpath(candidate),
                            );
                        }
                        let icon_skin = final_skin.split('|').next().unwrap_or("default");
                        resolved = format!("models/players/{model}/icon_{icon_skin}");
                        resolved.as_str()
                    } else {
                        name
                    };
                    if let Some(image) = try_load_texture_asset_from_search_path(
                        &mut assets,
                        qpath,
                        true,
                        false,
                        true,
                        "team overlay icon",
                    ) {
                        loaded = Some(image);
                        break;
                    }
                }
                let Some(image) = loaded else { continue };
                let (source_w, source_h) = (image.width, image.height);
                let Some(source) = image::RgbaImage::from_raw(source_w, source_h, image.rgba)
                else {
                    continue;
                };
                let cell_image = if source_w == cell && source_h == cell {
                    source
                } else {
                    image::imageops::resize(
                        &source,
                        cell,
                        cell,
                        image::imageops::FilterType::Triangle,
                    )
                };
                let col = index as u32 % columns;
                let row = index as u32 / columns;
                let pixels = cell_image.as_raw();
                let row_bytes = (cell * 4) as usize;
                for y in 0..cell {
                    let src = y as usize * row_bytes;
                    let dst = ((((row * cell + y) * width) + col * cell) * 4) as usize;
                    rgba[dst..dst + row_bytes].copy_from_slice(&pixels[src..src + row_bytes]);
                }
            }
        }
        Err(error) => eprintln!("UI team overlay asset search: {error}"),
    }
    TextureData {
        label: "JKA UI team overlay atlas".into(),
        source: None,
        width,
        height,
        rgba,
        rgba16f: None,
        mip_level_count: 1,
        clamp: true,
        srgb: true,
    }
}

pub(in crate::renderer) fn load_ui_font_texture(base: &Path, game: Option<&Path>) -> TextureData {
    match AssetSearchPath::open_game(base, game) {
        Ok(mut assets) => {
            // Jedi Academy's native fixed charset. It is a 16x16 logical grid,
            // but OpenJK samples only the left half of each horizontal slot
            // (8x16 glyphs on a 16x16 pitch); ui::glyph_quad mirrors that layout.
            if let Some(data) = try_load_ui_font_asset(&mut assets, "gfx/2d/charsgrid_med") {
                return data;
            }
        }
        Err(error) => eprintln!("UI font asset search: {error}"),
    }

    let (width, height, rgba) = ui::fallback_font_rgba();
    rverbose!(1, "UI font: using built-in fallback charset");
    TextureData {
        label: "JKA UI fallback charset".into(),
        source: None,
        width,
        height,
        rgba,
        rgba16f: None,
        mip_level_count: 1,
        clamp: true,
        srgb: false,
    }
}

pub(in crate::renderer) fn loading_levelshot_stem(map_name: &str) -> Option<String> {
    let trimmed = map_name.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Keep map subdirectories intact (e.g. mp/ffa3 -> levelshots/mp/ffa3).
    // Only remove the map-source extension; Path::file_stem would incorrectly
    // collapse mp/ffa3.bsp to just ffa3.
    let lower = trimmed.to_ascii_lowercase();
    let qpath = if lower.ends_with(".bsp") || lower.ends_with(".map") {
        &trimmed[..trimmed.len().saturating_sub(4)]
    } else {
        trimmed
    };
    Some(qpath.replace('\\', "/"))
}

pub(in crate::renderer) fn load_ui_loading_texture(
    base: &Path,
    game: Option<&Path>,
    levelshot_map_name: Option<&str>,
) -> (TextureData, Option<[u32; 2]>) {
    if let Some(map_name) = levelshot_map_name {
        // Use the renderer's normal VFS/image resolver. Passing an extensionless
        // qpath intentionally gives levelshots the same format fallback rules as
        // every other generic texture load instead of maintaining a private list.
        let path = format!("levelshots/{map_name}");
        if let Some(data) =
            try_load_texture_asset(base, game, &path, true, false, true, "UI levelshot")
        {
            let size = [data.width, data.height];
            return (data, Some(size));
        }
        eprintln!(
            "UI levelshot: no image resolved for {path}; falling back to menu/art/unknownmap_mp"
        );
        return load_ui_unknown_map_texture(base, game);
    }

    if let Some(data) =
        try_load_texture_asset(base, game, "menu/splash", true, false, true, "UI splash")
    {
        let size = [data.width, data.height];
        return (data, Some(size));
    }

    (
        TextureData {
            label: "JKA missing splash".into(),
            source: None,
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 255],
            rgba16f: None,
            mip_level_count: 1,
            clamp: true,
            srgb: true,
        },
        None,
    )
}

pub(in crate::renderer) fn load_ui_unknown_map_texture(
    base: &Path,
    game: Option<&Path>,
) -> (TextureData, Option<[u32; 2]>) {
    if let Some(data) = try_load_texture_asset(
        base,
        game,
        "menu/art/unknownmap_mp",
        true,
        false,
        true,
        "UI unknown map",
    ) {
        let size = [data.width, data.height];
        return (data, Some(size));
    }

    // Last-resort compatibility for incomplete/custom data installs.
    if let Some(data) = try_load_texture_asset(
        base,
        game,
        "menu/splash",
        true,
        false,
        true,
        "UI splash fallback",
    ) {
        let size = [data.width, data.height];
        return (data, Some(size));
    }

    (
        TextureData {
            label: "JKA missing unknown-map fallback".into(),
            source: None,
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 255],
            rgba16f: None,
            mip_level_count: 1,
            clamp: true,
            srgb: true,
        },
        None,
    )
}

pub(in crate::renderer) fn empty_ui_font_texture(label: &str) -> TextureData {
    TextureData {
        label: label.into(),
        source: None,
        width: 1,
        height: 1,
        rgba: vec![255, 255, 255, 0],
        rgba16f: None,
        mip_level_count: 1,
        clamp: true,
        srgb: false,
    }
}

pub(in crate::renderer) fn load_ui_proportional_font(
    base: &Path,
    game: Option<&Path>,
    name: &str,
) -> (TextureData, Option<ui::ProportionalFont>) {
    let mut assets = match AssetSearchPath::open_game(base, game) {
        Ok(assets) => assets,
        Err(error) => {
            eprintln!("UI proportional font asset search: {error}");
            return (empty_ui_font_texture("JKA missing proportional font"), None);
        }
    };

    let texture_path = format!("fonts/{name}");
    let texture = try_load_ui_font_asset(&mut assets, &texture_path)
        .unwrap_or_else(|| empty_ui_font_texture("JKA missing proportional font"));

    let metrics_path = format!("fonts/{name}.fontdat");
    let metrics = match assets.read(&metrics_path, 64 * 1024) {
        Ok(Some(asset)) => match ui::parse_fontdat(&asset.bytes) {
            Ok(font) => {
                if crate::logging::renderer_verbose_enabled(2) {
                    crate::logging::write_line_with_path(
                        crate::logging::Level::Info,
                        format_args!(
                            "UI proportional font: {} point / {} px, {} from {}",
                            font.point_size,
                            font.height,
                            metrics_path,
                            asset.source.display()
                        ),
                        asset.source.clone(),
                    );
                }
                Some(font)
            }
            Err(error) => {
                eprintln!("UI proportional font {metrics_path}: {error}");
                None
            }
        },
        Ok(None) => {
            eprintln!("UI proportional font: {metrics_path} not found");
            None
        }
        Err(error) => {
            eprintln!("UI proportional font {metrics_path}: {error}");
            None
        }
    };

    (texture, metrics)
}

#[derive(Clone)]
pub(in crate::renderer) struct ScreenFxBatch {
    pub(in crate::renderer) range: std::ops::Range<u32>,
    pub(in crate::renderer) blend: FxBlend,
    pub(in crate::renderer) texture_key: Option<String>,
}

#[cfg(test)]
mod extension_image_atlas_tests {
    use super::*;

    #[test]
    fn blank_atlas_has_room_for_every_slot() {
        let atlas = blank_ui_extension_atlas();
        let cells = (atlas.width / ui::EXT_IMAGE_CELL) * (atlas.height / ui::EXT_IMAGE_CELL);
        assert!(cells as usize >= crate::extensions::MAX_HUD_IMAGES);
        assert!(atlas.rgba.iter().all(|&b| b == 0));
    }

    #[test]
    #[ignore = "requires JKA_TEST_BASE with stock assets"]
    fn stock_hud_art_decodes_into_cells() {
        let base = std::path::PathBuf::from(std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE"));
        let names: Vec<String> = [
            "gfx/2d/numbers/t_one", "gfx/hud/armor_tic_1", "gfx/hud/hudleft", "gfx/hud/health_tic_1",
            "gfx/hud/nonexistent_image",
        ].iter().map(|n| (*n).to_owned()).collect();
        let cells = load_ui_extension_images(&base, None, &names);
        let opaque = |cell: &Vec<u8>| cell.chunks_exact(4).filter(|p| p[3] > 16).count();
        for cell in &cells[..4] {
            let cell = cell.as_ref().expect("stock image missing");
            assert_eq!(cell.len() as u32, ui::EXT_IMAGE_CELL * ui::EXT_IMAGE_CELL * 4);
            assert!(opaque(cell) > 500);
        }
        assert!(cells[4].is_none(), "a missing image must stay transparent");
    }
}
