//! Independent full-quality scenes sharing one GPU device/queue. As with the
//! companion framework, only the final presenter owns/acquires a native surface.
use super::studio_transition::StudioTransitionGpu;
use crate::renderer::{
    FrameInfo, Instant, RenderError, Renderer, StudioSceneFrame, StudioSourceFrame, ViewLatchMode,
};
use crate::renderer::capture::{tight_bgra, ScreenshotPixelFormat};

impl Renderer {
    pub(in crate::renderer) fn park_inactive_studio_views(&mut self, active: &[u64]) {
        let parked = winit::dpi::PhysicalSize::new(1, 1);
        for (id, source) in &mut self.studio_scenes {
            if active.contains(id) { continue; }
            if source.size != parked {
                source.studio_lens_gpu=None;
                source.studio_entity_mask = None;
                source.resize(parked);
            }
            // No-op once parked; new scenes start at 1x1 with full arrays.
            source.set_shadow_arrays_parked(true);
            source.set_local_shadow_cache_parked(true);
        }
    }
    pub(in crate::renderer) fn frame_time(&self) -> f32 {
        self.scene_time
            .unwrap_or_else(|| self.started.elapsed().as_secs_f32())
    }

    pub(in crate::renderer) fn render_studio_source(
        &mut self,
        sample: &StudioSourceFrame,
        video: &crate::ui::VideoSettings,
        size: winit::dpi::PhysicalSize<u32>,
        depth: bool,
        identity: bool,
    ) -> Result<(), RenderError> {
        let signature = format!("{video:?}");
        if signature != self.studio_settings_signature {
            super::super::render_thread::apply_scene_video_settings(self, video);
            self.studio_settings_signature = signature;
        }
        if self.size != size {
            self.resize(size);
        }
        self.set_shadow_arrays_parked(false);
        self.set_local_shadow_cache_parked(false);
        if self.depth_of_field_strength>0.0 {self.depth_of_field_strength=0.0;self.dof_mode=crate::ui::DofMode::Off;self.request_scene_rebuild();}
        self.apply_pending_scene_rebuild();
        if self.studio_revision != sample.revision {
            self.history_valid = false;
            self.camera_history_valid = false;
            self.cloud_history_valid = false;
            self.ssao_history_valid = false;
            self.ssr_history_valid = false;
            self.studio_revision = sample.revision;
        }
        self.scene_time = Some(sample.time);
        self.update_inline_models(Some(sample.inline_models.as_slice()));
        self.set_screen_fx(&sample.screen_fx);
        self.set_cloud_foreground(&sample.cloud_foreground);
        if let Some(ocean) = self.ocean.as_mut() {
            ocean.set_lake_actors(&sample.ocean_actors);
        }
        // Depth capture enables the normal radial-depth prepass. Its readback
        // is owned by the presenter, so no per-source disk output is created.
        self.studio_lens=Some(sample.lens);
        self.scene_depth_capture = depth || sample.lens.aperture.is_some();
        self.render(
            &sample.camera,
            sample.player_position,
            sample.area_mask.as_ref(),
            &sample.dynamic_models,
            &[],
            &sample.lights,
            sample.environment,
            None,
            128.0,
            None,
            &sample.camera,
            0.0,
            0.0,
            ViewLatchMode::Disabled,
            None,
        )?;
        if sample.lens.aperture.is_some() {
            let format=self.offscreen_output.as_ref().unwrap().format();
            let dimensions=[self.config.width.max(1),self.config.height.max(1)];
            if self.studio_lens_gpu.as_ref().is_none_or(|g|!g.matches(dimensions,format)){
                self.studio_lens_gpu=Some(super::super::studio_lens::StudioLensGpu::new(&self.device,dimensions,format));
            }
            let gpu=self.studio_lens_gpu.as_ref().unwrap();
            let input=self.offscreen_output.as_ref().unwrap().create_view(&Default::default());
            let mut encoder=self.device.create_command_encoder(&Default::default());
            gpu.encode(&self.device,&self.queue,&mut encoder,&input,&self.targets.linear_depth_view,&gpu.view,&sample.camera,sample.lens,[0,0,dimensions[0],dimensions[1]]);
            self.queue.submit(Some(encoder.finish()));
        } else {self.studio_lens_gpu=None;}
        if let Some(hud)=sample.hud.as_deref() {
            self.submit_studio_source_hud(hud);
        }
        if identity {
            let width=self.config.width.max(1);
            let height=self.config.height.max(1);
            let samples=self.msaa_samples;
            if self.studio_entity_mask.as_ref().is_none_or(|mask|
                !mask.matches(width,height,samples)) {
                self.studio_entity_mask=Some(super::super::studio_entity_mask::StudioEntityMask::new(
                    &self.device,&self.dynamic_model_renderer,width,height,samples));
            }
            self.studio_entity_mask.as_ref().unwrap().encode(
                &self.device,&self.queue,&self.dynamic_model_renderer,
                &self.camera_bind_group,&self.targets.depth_view);
        }
        Ok(())
    }

    /// Paint the source's own native JKA HUD into its final color buffer,
    /// *before* Studio transitions blend two recordings. Preview and clean
    /// frame readbacks therefore contain the same per-demo HUD pixels.
    fn submit_studio_source_hud(&mut self,hud:&crate::ui::UiSnapshot) {
        use crate::ui;
        let (w,h)=(self.config.width.max(1),self.config.height.max(1));
        // The key-art atlas is lazily resident in ordinary gameplay. Studio
        // can be opened directly from the main menu, so initialize it here too.
        if hud.movement_keys.mode != 0 && !self.ui_keys_loaded {
            self.load_ui_key_atlas_now();
        }
        let mut scaled_hud;
        let hud=if hud.studio_hud_virtual_layout {
            scaled_hud=hud.clone();
            scaled_hud.hud_layout=hud.hud_layout.studio_for_frame(w,h);
            &scaled_hud
        }else{hud};
        // Studio standings use a separate sans-serif atlas; gameplay keeps its
        // OCR HUD font. Both batches still land in preview and exported frames.
        let wanted_font=hud.studio_race.as_ref().map(|race|race.font.as_str());
        if wanted_font.is_some_and(|font|self.studio_race_font.as_ref().is_none_or(|(key,_)|key!=font)) {
            let choice=wanted_font.unwrap_or_default().to_owned();
            let (data,metrics)=crate::renderer::load_studio_overlay_font(&self.base,self.detail_texture_game.as_deref(),&choice);
            self.studio_race_font=Some((choice,None));
            if let Some(metrics)=metrics {
                let image=crate::renderer::textures::upload_cached_texture(&self.device,&self.queue,&data);
                let views=[&self._ui_font.view,&image.view,&self._ui_splash.view,&self._ui_keys.view,&self._ui_icons.view,&self._ui_team_icons.view,&self._ui_ext_images.view,&self._ui_clean_font.view,&self._ui_avatars.view];
                let mut entries=Vec::new();
                for (index,view) in views.iter().enumerate() {
                    entries.push(wgpu::BindGroupEntry {binding:if index==0 {0}else{index as u32+1},resource:wgpu::BindingResource::TextureView(view)});
                }
                entries.push(wgpu::BindGroupEntry {binding:1,resource:wgpu::BindingResource::Sampler(&self._ui_font_sampler)});
                let binding=self.device.create_bind_group(&wgpu::BindGroupDescriptor {label:Some("Studio standings font"),layout:&self.ui_texture_layout,entries:&entries});
                if let Some((_,font))=self.studio_race_font.as_mut() {*font=Some((image,metrics,binding));}
            }
        }
        let mut native_hud=hud.clone();
        let studio_font=self.studio_race_font.as_ref().and_then(|(_,font)|font.as_ref()).filter(|_|hud.studio_race.is_some());
        if studio_font.is_some() {
            native_hud.studio_race=None;
            // Comparison name tags share the overlay font.
            native_hud.studio_world_labels.clear();
        }
        let mut vertices=Vec::new();
        ui::build_dynamic_static_vertices(&mut vertices,&native_hud,w,h);
        ui::build_movement_vertices(&mut vertices,&native_hud,w,h);
        ui::build_transient_vertices(&mut vertices,&native_hud,self.ui_small_font_metrics.as_ref(),w,h);
        let native_count=vertices.len() as u32;
        if let (Some((_,font,_)),Some(race))=(studio_font,&hud.studio_race) {
            ui::draw_studio_race(&mut vertices,race,Some(font),w,h);
            ui::draw_studio_labels(&mut vertices,&hud.studio_world_labels,Some(font),w,h);
        }
        let limit=crate::renderer::UI_DYNAMIC_BUFFER_BYTES as usize / std::mem::size_of::<crate::renderer::UiVertex>();
        if vertices.len()>limit {vertices.truncate(limit-(limit%3));}
        if vertices.is_empty(){return;}
        self.queue.write_buffer(&self.ui_dynamic_vertex_buffer,0,bytemuck::cast_slice(&vertices));
        let target=self.studio_lens_gpu.as_ref().map_or_else(
            ||self.offscreen_output.as_ref().expect("Studio source output").create_view(&Default::default()),
            |gpu|gpu.view.clone());
        let mut encoder=self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label:Some("Studio native source HUD")});
        {
            let mut pass=encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label:Some("Studio HUD over source color"),
                color_attachments:&[Some(wgpu::RenderPassColorAttachment {
                    view:&target,depth_slice:None,resolve_target:None,
                    ops:wgpu::Operations{load:wgpu::LoadOp::Load,store:wgpu::StoreOp::Store},
                })],depth_stencil_attachment:None,timestamp_writes:None,
                occlusion_query_set:None,multiview_mask:None,
            });
            pass.set_pipeline(&self.ui_pipeline);
            pass.set_bind_group(0,&self.ui_bind_group,&[]);
            pass.set_vertex_buffer(0,self.ui_dynamic_vertex_buffer.slice(..));
            pass.draw(0..native_count.min(vertices.len() as u32),0..1);
            if vertices.len() as u32>native_count {
                if let Some((_,_,binding))=studio_font {pass.set_bind_group(0,binding,&[]);}
                pass.draw(native_count..vertices.len() as u32,0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
    }

    /// Render (or, for an unchanged offline scene, reuse) both source worlds of
    /// `scene` at `size`. `None` when a source world is not loaded.
    fn render_studio_sources(
        &mut self,
        scene: &std::sync::Arc<StudioSceneFrame>,
        size: winit::dpi::PhysicalSize<u32>,
        depth: bool,
        allow_cache: bool,
    ) -> Result<Option<StudioSourceTextures>, RenderError> {
        let mut active = vec![scene.outgoing.id];
        if let Some(incoming) = scene.incoming.as_ref() { active.push(incoming.id); }
        self.park_inactive_studio_views(&active);
        // A depth reveal needs a fresh linear-depth prepass in BOTH source
        // worlds, even when the user is not exporting an EXR depth channel.
        let transition_depth = scene.incoming.is_some() && matches!(scene.effect, 8 | 9 | 10);
        let transition_identity = scene.incoming.is_some() && matches!(scene.effect, 15 | 16);
        let cached = allow_cache && scene.offline_capture
            && self.studio_last_scene.as_ref().is_some_and(|last| std::sync::Arc::ptr_eq(last, scene))
            && (!depth || self.studio_last_depth)
            && std::iter::once(&scene.outgoing).chain(scene.incoming.as_ref()).all(|sample|
                self.studio_scenes.get(&sample.id).is_some_and(|s| s.size == size));
        if !cached {
            let samples = std::iter::once(&scene.outgoing).chain(scene.incoming.as_ref());
            for sample in samples {
                let Some(source) = self.studio_scenes.get_mut(&sample.id) else { return Ok(None); };
                source.render_studio_source(sample, &scene.video, size, depth || transition_depth, transition_identity)?;
            }
            self.studio_last_scene = (allow_cache && scene.offline_capture).then(|| std::sync::Arc::clone(scene));
            self.studio_last_depth = depth;
        }
        Ok(self.studio_source_textures(scene))
    }

    /// Owned handles to the sources' current output (no rendering).
    fn studio_source_textures(&self, scene: &StudioSceneFrame) -> Option<StudioSourceTextures> {
        let outgoing = self.studio_scenes.get(&scene.outgoing.id)?;
        let incoming = match scene.incoming.as_ref() {
            Some(sample) => self.studio_scenes.get(&sample.id)?,
            None => outgoing,
        };
        let output = |source: &Renderer| source.studio_lens_gpu.as_ref().map_or_else(
            || source.offscreen_output.as_ref().map(wgpu::Texture::clone), |g| Some(g.texture.clone()));
        Some(StudioSourceTextures {
            color: [output(outgoing)?, output(incoming)?],
            depth: [outgoing.targets._linear_depth_texture.clone(), incoming.targets._linear_depth_texture.clone()],
            identity: [
                outgoing.studio_entity_mask.as_ref().map(|mask| mask.view.clone()),
                scene.incoming.as_ref().and(incoming.studio_entity_mask.as_ref()).map(|mask| mask.view.clone()),
            ],
        })
    }

    pub(in crate::renderer) fn queue_studio_capture(
        &mut self,
        frame_index: u64,
        depth: bool,
        scene: std::sync::Arc<StudioSceneFrame>,
    ) {
        self.studio_capture_queue.push_back(StudioCapture { frame_index, depth, received: Instant::now(), scene });
    }

    pub(in crate::renderer) fn render_studio_scene(
        &mut self,
        scene: &std::sync::Arc<StudioSceneFrame>,
    ) -> Result<FrameInfo, RenderError> {
        let started = Instant::now();
        let composite_rect = self.studio_world_viewport.and_then(|viewport|
            Self::clamped_studio_composite_rect(viewport, self.config.width, self.config.height));
        let exporting = self.studio_export_size.is_some();
        let (scene, textures) = if exporting {
            // Export preview: a ~30 Hz view of the last captured frame. The
            // sources are rendered only by captures, never a second time here.
            let Some(_) = composite_rect else { return Ok(FrameInfo::default()); };
            if self.studio_export_preview_at
                .is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(33)) {
                return Ok(FrameInfo::default());
            }
            let Some(shown) = self.studio_export_shown.clone() else { return Ok(FrameInfo::default()); };
            let Some(textures) = self.studio_source_textures(&shown) else { return Ok(FrameInfo::default()); };
            self.studio_export_preview_at = Some(Instant::now());
            (shown, textures)
        } else {
            let Some([_, _, w, h]) = composite_rect else {
                return self.render_asset_preview(&scene.outgoing.camera, &[]);
            };
            let depth = matches!(self.screenshot_requested,
                Some(crate::renderer::ScreenshotDestination::VideoFrame { depth: true, .. }));
            match self.render_studio_sources(scene, winit::dpi::PhysicalSize::new(w, h), depth, true)? {
                Some(textures) => (std::sync::Arc::clone(scene), textures),
                None => return self.render_asset_preview(&scene.outgoing.camera, &[]),
            }
        };
        let [x, y, w, h] = composite_rect.expect("checked above");
        let mut info = FrameInfo::default();
        let frame = self.acquire_render_frame(&mut info)?;
        let surface_view = self.surface_texture_view(&frame);
        let view = self
            .hdr_output
            .as_ref()
            .map_or_else(|| surface_view.clone(), |o| o.view.clone());
        let mut readback = self.prepare_screenshot_readback();
        let clean = readback.as_ref().is_some_and(|r| {
            matches!(
                r.destination,
                crate::renderer::ScreenshotDestination::VideoFrame { .. }
            )
        });
        let depth = !exporting && readback.as_ref().is_some_and(|r| r.depth_requested);
        let extent = frame.texture.size();
        // Presentation may go through the FP16 native-HDR compositor instead
        // of the SDR surface. The transition target must match its actual view.
        let composite_format=self.preview_scene_format();
        if self
            .studio_transition_gpu
            .as_ref()
            .is_none_or(|g| g.size != extent || g.format != composite_format)
        {
            self.studio_transition_gpu = Some(StudioTransitionGpu::new(
                &self.device,
                extent,
                composite_format,
            ));
        }
        let gpu = self.studio_transition_gpu.as_ref().unwrap();
        let bind = Self::studio_composite_bind_group(&self.device, gpu, &textures);
        let values = [
            (x as f32).to_bits(),
            (y as f32).to_bits(),
            (w as f32).to_bits(),
            (h as f32).to_bits(),
            scene.progress.to_bits(),
            scene.effect,
            0,
            0,
        ];
        self.queue
            .write_buffer(&gpu.params, 0, bytemuck::cast_slice(&values));
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Studio scene presentation"),
            });
        if depth {
            {
                let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Studio depth backing"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.targets.linear_depth_render_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 1_000_000.0,
                                g: 0.0,
                                b: 0.0,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
            }
            encoder.copy_texture_to_texture(
                textures.depth[0].as_image_copy(),
                wgpu::TexelCopyTextureInfo {
                    texture: &self.targets._linear_depth_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x, y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Studio independent world composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.039,
                            g: 0.063,
                            b: 0.098,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_scissor_rect(x, y, w, h);
            pass.set_pipeline(&gpu.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        if !clean {
            self.submit_egui(&view);
            if self.egui_active {
                self.submit_ui_overlay(&view);
            }
        }
        if let Some(output) = &self.hdr_output {
            output.submit(&self.device, &self.queue, &surface_view, readback.is_some());
        }
        if let Some(readback) = &mut readback {
            self.submit_screenshot_copy(
                self.hdr_output
                    .as_ref()
                    .map_or(&frame.texture, |o| &o.capture),
                readback,
            );
        }
        self.present_render_frame(frame);
        if let Some(readback) = readback {
            self.finish_screenshot_readback(readback);
        }
        info.frame_ms = started.elapsed().as_secs_f64() * 1000.0;
        info.present_call_completed_at = Some(Instant::now());
        Ok(info)
    }

    fn studio_composite_bind_group(
        device: &wgpu::Device,
        gpu: &StudioTransitionGpu,
        textures: &StudioSourceTextures,
    ) -> wgpu::BindGroup {
        let view = |texture: &wgpu::Texture| texture.create_view(&Default::default());
        let (color_a, color_b) = (view(&textures.color[0]), view(&textures.color[1]));
        let (depth_a, depth_b) = (view(&textures.depth[0]), view(&textures.depth[1]));
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Independent Studio source views"),
            layout: &gpu.bindings,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&color_a) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&color_b) },
                wgpu::BindGroupEntry { binding: 2, resource: gpu.params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&depth_a) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&depth_b) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(
                    textures.identity[0].as_ref().unwrap_or(&gpu.empty_entity_mask_view)) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(
                    textures.identity[1].as_ref().unwrap_or(&gpu.empty_entity_mask_view)) },
            ],
        })
    }

    /// One queued export/still frame: render its own scene's sources at the
    /// output size, composite them into a dedicated texture and copy that into
    /// a readback slot. Nothing waits for the GPU here: the slot is mapped
    /// asynchronously and collected by `finish_studio_readbacks`, so the next
    /// frame is recorded while the GPU still works on this one. No swapchain
    /// acquire/present either, so VSync never paces it and nothing from the
    /// editor can leak in.
    pub(in crate::renderer) fn render_studio_capture(&mut self, capture: StudioCapture) -> Result<FrameInfo, RenderError> {
        let started = Instant::now();
        let Some([width, height]) = self.studio_export_size else {
            self.studio_capture_results.push_back(Err("Studio capture requested without an export size".into()));
            return Ok(FrameInfo::default());
        };
        let max = self.device.limits().max_texture_dimension_2d;
        let size = winit::dpi::PhysicalSize::new(width.clamp(16, max), height.clamp(16, max));
        let format = self.config.format;
        let pixel_format = match format {
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => ScreenshotPixelFormat::Bgra,
            wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => ScreenshotPixelFormat::Rgba,
            other => {
                self.studio_capture_results.push_back(Err(format!("unsupported Studio export format {other:?}")));
                return Ok(FrameInfo::default());
            }
        };
        let Some(textures) = self.render_studio_sources(&capture.scene, size, capture.depth, false)? else {
            self.studio_capture_results.push_back(Err("Studio export source world is not loaded".into()));
            return Ok(FrameInfo::default());
        };
        let color_format = textures.color[0].format();
        if color_format != format || textures.color[0].width() != size.width || textures.color[0].height() != size.height {
            self.studio_capture_results.push_back(Err(format!(
                "Studio export source is {}x{} {color_format:?}, expected {}x{} {format:?}",
                textures.color[0].width(), textures.color[0].height(), size.width, size.height)));
            return Ok(FrameInfo::default());
        }
        self.studio_export_shown = Some(std::sync::Arc::clone(&capture.scene));
        let extent = wgpu::Extent3d { width: size.width, height: size.height, depth_or_array_layers: 1 };
        if self.studio_export_target.as_ref().is_none_or(|(gpu, target)| gpu.size != extent || target.format() != color_format) {
            let target = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Studio offscreen export composite"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: color_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            self.studio_export_target = Some((StudioTransitionGpu::new(&self.device, extent, color_format), target));
        }
        let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded = (size.width * 4).div_ceil(alignment) * alignment;
        let buffer_size = u64::from(padded) * u64::from(size.height);
        let slot = self.studio_readback_slot(buffer_size, capture.depth);
        let (gpu, target) = self.studio_export_target.as_ref().unwrap();
        let bind = Self::studio_composite_bind_group(&self.device, gpu, &textures);
        let values = [
            0.0_f32.to_bits(),
            0.0_f32.to_bits(),
            (size.width as f32).to_bits(),
            (size.height as f32).to_bits(),
            capture.scene.progress.to_bits(),
            capture.scene.effect,
            0,
            0,
        ];
        self.queue.write_buffer(&gpu.params, 0, bytemuck::cast_slice(&values));
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Studio offscreen export composite"),
        });
        {
            let view = target.create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Studio offscreen export composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
            pass.set_pipeline(&gpu.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        let readback = &self.studio_readback_slots[slot];
        let copy = |encoder: &mut wgpu::CommandEncoder, texture: &wgpu::Texture, buffer: &wgpu::Buffer| {
            encoder.copy_texture_to_buffer(
                texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded),
                        rows_per_image: Some(size.height),
                    },
                },
                extent,
            );
        };
        copy(&mut encoder, target, &readback.color);
        // The outgoing source's own radial depth is already at output size.
        let depth_buffer = readback.depth.as_ref().filter(|_| capture.depth);
        if let Some(buffer) = depth_buffer {
            copy(&mut encoder, &textures.depth[0], buffer);
        }
        let submission = self.queue.submit([encoder.finish()]);
        let submitted = Instant::now();
        // Map state per buffer (color, depth): 0 pending, 1 mapped, 2 failed.
        let mapped = std::sync::Arc::new([
            std::sync::atomic::AtomicU8::new(0),
            std::sync::atomic::AtomicU8::new(if depth_buffer.is_some() { 0 } else { 1 }),
        ]);
        for (index, buffer) in [Some(&readback.color), depth_buffer].into_iter().enumerate() {
            let Some(buffer) = buffer else { continue };
            let flags = std::sync::Arc::clone(&mapped);
            buffer.slice(0..buffer_size).map_async(wgpu::MapMode::Read, move |result| {
                flags[index].store(if result.is_ok() { 1 } else { 2 }, std::sync::atomic::Ordering::Release);
            });
        }
        self.studio_readbacks.push_back(PendingStudioReadback {
            slot, frame_index: capture.frame_index, received: capture.received, started, submitted,
            submission, width: size.width, height: size.height, padded, buffer_size, pixel_format,
            depth: depth_buffer.is_some(), mapped,
        });
        let mut info = FrameInfo::default();
        info.frame_ms = started.elapsed().as_secs_f64() * 1000.0;
        Ok(info)
    }

    /// A free readback slot large enough for `size` bytes (and depth). With
    /// every slot in flight, the oldest frame is completed first.
    fn studio_readback_slot(&mut self, size: u64, depth: bool) -> usize {
        loop {
            let free = (0..self.studio_readback_slots.len()).find(|slot|
                !self.studio_readbacks.iter().any(|p| p.slot == *slot) && !self.studio_unpacking.contains(slot));
            let create = |device: &wgpu::Device| StudioReadbackSlot {
                color: Self::studio_readback_buffer(device, size, "Studio export color readback"),
                depth: depth.then(|| Self::studio_readback_buffer(device, size, "Studio export depth readback")),
                size,
            };
            if let Some(slot) = free {
                let current = &self.studio_readback_slots[slot];
                if current.size < size || (depth && current.depth.is_none()) {
                    self.studio_readback_slots[slot] = create(&self.device);
                }
                return slot;
            }
            if self.studio_readback_slots.len() < STUDIO_READBACK_SLOTS {
                self.studio_readback_slots.push(create(&self.device));
                return self.studio_readback_slots.len() - 1;
            }
            if self.studio_readbacks.is_empty() {
                self.collect_studio_unpacked(true);
            } else {
                self.finish_studio_readbacks(true);
            }
        }
    }

    fn studio_readback_buffer(device: &wgpu::Device, size: u64, label: &str) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        })
    }

    /// Hand mapped export readbacks to the unpack worker in frame order and
    /// collect finished frames. `wait` blocks until the oldest one is mapped;
    /// otherwise this only polls (every render loop).
    pub(in crate::renderer) fn finish_studio_readbacks(&mut self, wait: bool) {
        if let Some(front) = self.studio_readbacks.front() {
            let poll = if wait {
                wgpu::PollType::Wait { submission_index: Some(front.submission.clone()), timeout: None }
            } else {
                wgpu::PollType::Poll
            };
            let _ = self.device.poll(poll);
            while let Some(front) = self.studio_readbacks.front() {
                let state = front.mapped.each_ref().map(|flag| flag.load(std::sync::atomic::Ordering::Acquire));
                if state.contains(&0) { break; }
                let pending = self.studio_readbacks.pop_front().expect("front exists");
                self.queue_studio_unpack(pending, state);
            }
        }
        self.collect_studio_unpacked(false);
    }

    /// The repack (one full-frame copy out of mapped memory) runs on a
    /// persistent worker so the render thread can record the next frame.
    fn queue_studio_unpack(&mut self, pending: PendingStudioReadback, state: [u8; 2]) {
        let slot_index = pending.slot;
        let slot = &self.studio_readback_slots[slot_index];
        let job = StudioUnpackJob {
            generation: self.studio_readback_generation,
            slot: pending.slot,
            color: slot.color.clone(),
            depth: slot.depth.clone().filter(|_| pending.depth),
            state,
            queued: Instant::now(),
            pending,
        };
        if self.studio_unpacker.is_none() {
            self.studio_unpacker = StudioUnpacker::spawn();
        }
        let job = match self.studio_unpacker.as_ref() {
            Some(unpacker) => match unpacker.jobs.send(job) {
                Ok(()) => {
                    self.studio_unpacking.push(slot_index);
                    return;
                }
                Err(std::sync::mpsc::SendError(job)) => job,
            },
            None => job,
        };
        // No worker: unpack inline rather than lose the frame.
        let done = unpack_studio_readback(job);
        self.studio_capture_results.push_back(done.result);
    }

    /// Finished frames from the unpack worker, oldest first. `block` waits for one.
    fn collect_studio_unpacked(&mut self, block: bool) {
        let Some(unpacker) = self.studio_unpacker.as_ref() else { return; };
        let mut block = block && !self.studio_unpacking.is_empty();
        loop {
            let done = if block { unpacker.done.recv().ok() } else { unpacker.done.try_recv().ok() };
            block = false;
            let Some(done) = done else { break; };
            // Results of a cancelled export (older generation) are dropped.
            if done.generation != self.studio_readback_generation { continue; }
            self.studio_unpacking.retain(|slot| *slot != done.slot);
            self.studio_capture_results.push_back(done.result);
        }
    }
}

/// Persistent export readback unpacker (one channel-fed thread).
pub(in crate::renderer) struct StudioUnpacker {
    jobs: std::sync::mpsc::Sender<StudioUnpackJob>,
    done: std::sync::mpsc::Receiver<StudioUnpacked>,
}
impl StudioUnpacker {
    fn spawn() -> Option<Self> {
        let (jobs, job_rx) = std::sync::mpsc::channel::<StudioUnpackJob>();
        let (done_tx, done) = std::sync::mpsc::channel();
        std::thread::Builder::new().name("studio-readback-unpack".into()).spawn(move || {
            for job in job_rx {
                if done_tx.send(unpack_studio_readback(job)).is_err() { break; }
            }
        }).ok()?;
        Some(Self { jobs, done })
    }
}

struct StudioUnpackJob {
    generation: u64,
    slot: usize,
    color: wgpu::Buffer,
    depth: Option<wgpu::Buffer>,
    state: [u8; 2],
    queued: Instant,
    pending: PendingStudioReadback,
}

struct StudioUnpacked {
    generation: u64,
    slot: usize,
    result: Result<crate::renderer::ScreenshotOutput, String>,
}

/// Copy a mapped export readback into tight BGRA (and depth) and unmap it.
fn unpack_studio_readback(job: StudioUnpackJob) -> StudioUnpacked {
    let worker_started = Instant::now();
    let pending = &job.pending;
    let row_bytes = pending.width as usize * 4;
    let (padded, height) = (pending.padded as usize, pending.height as usize);
    let read = |buffer: &wgpu::Buffer, mapped: u8, format: ScreenshotPixelFormat| -> Result<Vec<u8>, String> {
        if mapped != 1 { return Err("GPU readback mapping failed".into()); }
        let view = buffer.slice(0..pending.buffer_size).get_mapped_range();
        let pixels = match &view {
            Ok(view) => Ok(tight_bgra(view, row_bytes, padded, height, format)),
            Err(error) => Err(format!("GPU readback range failed: {error}")),
        };
        drop(view);
        buffer.unmap();
        pixels
    };
    // Read (and unmap) both before reporting either error.
    let color = read(&job.color, job.state[0], pending.pixel_format);
    let depth = job.depth.as_ref().map(|buffer| read(buffer, job.state[1], ScreenshotPixelFormat::Bgra));
    let result = color.and_then(|bgra| {
        let depth_f32le = depth.transpose()?;
        let now = Instant::now();
        let timing = crate::runtime::VideoFrameTiming {
            received: pending.received, frame_started: pending.started, presented: pending.submitted,
            gpu_wait_ms: job.queued.saturating_duration_since(pending.submitted).as_secs_f64() * 1000.0,
            readback_copy_ms: now.saturating_duration_since(worker_started).as_secs_f64() * 1000.0,
            queued: job.queued, worker_started, worker_done: now,
        };
        Ok(crate::renderer::ScreenshotOutput::VideoFrame {
            frame_index: pending.frame_index, width: pending.width, height: pending.height,
            bgra, depth_f32le, timing,
        })
    });
    StudioUnpacked { generation: job.generation, slot: job.slot, result }
}

/// Export readback buffers in flight at once (one per pipelined frame).
const STUDIO_READBACK_SLOTS: usize = 3;

pub(in crate::renderer) struct StudioReadbackSlot {
    color: wgpu::Buffer,
    depth: Option<wgpu::Buffer>,
    size: u64,
}

/// A submitted export frame whose readback buffers are mapping.
pub(in crate::renderer) struct PendingStudioReadback {
    slot: usize,
    frame_index: u64,
    received: Instant,
    started: Instant,
    submitted: Instant,
    submission: wgpu::SubmissionIndex,
    width: u32,
    height: u32,
    padded: u32,
    buffer_size: u64,
    pixel_format: ScreenshotPixelFormat,
    depth: bool,
    /// Color, depth: 0 pending, 1 mapped, 2 failed (depth is 1 when absent).
    mapped: std::sync::Arc<[std::sync::atomic::AtomicU8; 2]>,
}

/// A queued offscreen Render Studio capture. It carries its own scene, so
/// frames can be in flight while the app builds and publishes the next one.
pub(in crate::renderer) struct StudioCapture {
    pub frame_index: u64,
    pub depth: bool,
    pub received: Instant,
    pub scene: std::sync::Arc<StudioSceneFrame>,
}

/// Owned handles to a scene's source outputs (outgoing, incoming).
struct StudioSourceTextures {
    color: [wgpu::Texture; 2],
    depth: [wgpu::Texture; 2],
    identity: [Option<wgpu::TextureView>; 2],
}
