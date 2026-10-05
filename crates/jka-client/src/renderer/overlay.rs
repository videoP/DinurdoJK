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
    pub(in crate::renderer) fn set_ui(&mut self, ui_state: UiSnapshot) {
        let needs_key_atlas = !self.ui_keys_loaded && ui_state.movement_keys.mode != 0;
        self.ui_state = ui_state;
        if needs_key_atlas {
            self.load_ui_key_atlas_now();
        }
        self.load_ui_icons_if_needed();
        self.sync_ui_team_icons();
        self.sync_ui_loading_background();
        self.rebuild_ui();
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
            ],
        });
    }

    /// Bind the icon atlas the first time a draw list wants it: the lagometer's
    /// own frame / phone jack, the old speed graph's frame, or an image crosshair.
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
            || self.ui_state.force_select.is_some();
        if !wanted {
            return;
        }
        let started = Instant::now();
        let data = load_ui_icon_atlas(&self.base, self.detail_texture_game.as_deref());
        self._ui_icons = upload_texture(&self.device, &self.queue, &data);
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
        let wanted = self
            .ui_state
            .team_overlay
            .as_ref()
            .map(|overlay| overlay.icon_paths.clone())
            .or_else(|| match self.ui_state.mini_scores.as_ref() {
                Some(ui::UiMiniScores::Duel { icon_paths, .. }) => Some(icon_paths.clone()),
                _ => None,
            })
            .unwrap_or_default();
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
        center_print: Option<ui::UiCenterPrint>,
        demo_timeline: Option<ui::DemoTimelineUi>,
        prediction_debug: Option<ui::PredictionDebugUi>,
        crosshair_target: ui::UiCrosshairTarget,
        force_select: Option<ui::UiForceSelect>,
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
        self.ui_state.center_print = center_print;
        self.ui_state.demo_timeline = demo_timeline;
        self.ui_state.prediction_debug = prediction_debug;
        self.ui_state.crosshair_target = crosshair_target;
        self.ui_state.force_select = force_select;
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
        threads: [ui::ThreadPerfStats; crate::thread_activity::SLOT_COUNT],
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
        let Some(frame) = frame else {
            self.egui_active = false;
            self.egui_paint_jobs.clear();
            for texture_id in self.egui_pending_free.drain(..) {
                self.egui_renderer.free_texture(&texture_id);
            }
            return;
        };

        for (texture_id, image_delta) in &frame.textures_delta.set {
            self.egui_renderer
                .update_texture(&self.device, &self.queue, *texture_id, image_delta);
        }
        self.egui_pending_free.extend(frame.textures_delta.free);
        self.egui_paint_jobs = frame.paint_jobs;
        self.egui_pixels_per_point = frame.pixels_per_point.max(0.25);
        self.egui_active = true;
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
            let mut pass = pass.forget_lifetime();
            self.egui_renderer
                .render(&mut pass, &self.egui_paint_jobs, &screen);
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
        !self.ui_state.video.skip_ui
            && self.ui_state.loading.is_none()
            && (self.ui_state.mode == ui::OverlayMode::Game
                || (self.ui_state.mode == ui::OverlayMode::Video
                    && self.ui_state.setup_selected != 1))
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

    pub(in crate::renderer) fn submit_menu_backdrop(&self, frame_view: &wgpu::TextureView) {
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
            self.queue.write_buffer(
                &self.ui_transient_vertex_buffer,
                byte_offset,
                bytemuck::cast_slice(tail),
            );
        }
    }
}

/// Packs the stock 2D images in `ui::ICON_NAMES` (the lagometer's graph frame and phone
/// jack, then the `crosshaira..j` image crosshairs) into one row of `ui::ICON_CELL`
/// squares. The retail images are 32x32, so they are resampled up to the cell; a missing
/// image (`crosshairj` needs japro-assets.pk3) leaves its cell transparent.
pub(in crate::renderer) fn load_ui_icon_atlas(base: &Path, game: Option<&Path>) -> TextureData {
    let cell = ui::ICON_CELL;
    let (width, height) = (ui::ICON_NAMES.len() as u32 * cell, cell);
    let mut rgba = vec![0u8; (width * height * 4) as usize];

    match AssetSearchPath::open_game(base, game) {
        Ok(mut assets) => {
            for (index, name) in ui::ICON_NAMES.iter().enumerate() {
                let Some(image) = try_load_texture_asset_from_search_path(
                    &mut assets,
                    name,
                    true,
                    false,
                    true,
                    "UI icon",
                ) else {
                    eprintln!("UI icon {name}: not found");
                    continue;
                };
                let (source_w, source_h) = (image.width, image.height);
                let Some(source) = image::RgbaImage::from_raw(source_w, source_h, image.rgba)
                else {
                    eprintln!("UI icon {name}: unexpected pixel format");
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
                let pixels = cell_image.as_raw();
                let row_bytes = (cell * 4) as usize;
                for y in 0..cell {
                    let src = y as usize * row_bytes;
                    let dst = (((y * width) + index as u32 * cell) * 4) as usize;
                    rgba[dst..dst + row_bytes].copy_from_slice(&pixels[src..src + row_bytes]);
                }
            }
        }
        Err(error) => eprintln!("UI icon asset search: {error}"),
    }
    TextureData {
        label: "JKA UI icon atlas".into(),
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
