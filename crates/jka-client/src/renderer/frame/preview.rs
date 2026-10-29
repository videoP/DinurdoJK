//! Frame preview.
use crate::renderer::{
    camera_depth_clear, Camera, CameraUniform, DynamicModelSurface, FrameInfo, Instant,
    RenderError, Renderer,
};

impl Renderer {
    pub(in crate::renderer) fn render_asset_preview(
        &mut self,
        camera: &Camera,
        dynamic_models: &[DynamicModelSurface],
    ) -> Result<FrameInfo, RenderError> {
        // The editor can route an ESC/menu frame here immediately after an
        // FP16 demo scene. SetAssetPreviewMode only *queues* a target/PSO
        // rebuild, so using the old MSAA view would attempt to resolve
        // Rgba16Float into Bgra8UnormSrgb and panic at encoder.finish().
        // Also covers Studio's empty-scene fallback, which calls this method
        // directly even when the explicit asset-preview toggle is off.
        let preview_format = self.preview_scene_format();
        if self.targets._scene_texture.format() != preview_format
            || self.scene_pipeline_compat != (preview_format, self.msaa_samples)
        {
            if !self.asset_preview_mode {
                self.set_asset_preview_mode(true);
            }
            self.request_scene_rebuild();
        }
        self.apply_pending_scene_rebuild();
        let frame_started = Instant::now();
        let full_width = self.config.width.max(1);
        let full_height = self.config.height.max(1);
        let draw_preview = self.asset_preview_viewport.is_some();
        let mut viewport = self.asset_preview_viewport.unwrap_or([0, 0, 1, 1]);
        viewport[0] = viewport[0].min(full_width.saturating_sub(1));
        viewport[1] = viewport[1].min(full_height.saturating_sub(1));
        viewport[2] = viewport[2]
            .min(full_width.saturating_sub(viewport[0]))
            .max(1);
        viewport[3] = viewport[3]
            .min(full_height.saturating_sub(viewport[1]))
            .max(1);
        let view_proj = camera.view_projection(viewport[2], viewport[3]);
        if let Some(background)=&mut self.asset_preview_background{
            background.prepare(&self.device,&self.queue,preview_format,self.msaa_samples,[viewport[2],viewport[3]]);
        }
        let uniform = CameraUniform {
            view_proj: view_proj.to_cols_array_2d(),
            camera_pos_time: [
                camera.position.x,
                camera.position.y,
                camera.position.z,
                self.frame_time(),
            ],
            clip_plane: [0.0; 4],
            render_flags: [0, 0, 0, 0],
            camera_forward: camera.forward().extend(0.0).to_array(),
            unjittered_view_proj: view_proj.to_cols_array_2d(),
            previous_unjittered_view_proj: view_proj.to_cols_array_2d(),
            jump_shade: [0.0; 4],
            foliage_wind: [0.0; 4],
        };
        self.queue
            .write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&uniform));

        let mut info = FrameInfo::default();
        let prepare_started = Instant::now();
        self.dynamic_model_renderer.prepare(
            &mut self.baked_brightness,
            &mut self.pipeline_jobs,
            &self.device,
            &self.queue,
            dynamic_models,
            None,
            false,
            false,
            false,
            false,
        );
        self.dynamic_model_renderer.prepare_model_bundles(&self.device, &self.camera_bind_group);
        info.dynamic_model_prepare_ms = prepare_started.elapsed().as_secs_f64() * 1000.0;
        let (ghoul2_gpu_instances, ghoul2_gpu_draw_calls) =
            self.dynamic_model_renderer.ghoul2_draw_stats();
        info.ghoul2_gpu_instances = ghoul2_gpu_instances;
        info.ghoul2_gpu_draw_calls = ghoul2_gpu_draw_calls;
        info.dynamic_model_surfaces = u32::try_from(dynamic_models.len()).unwrap_or(u32::MAX);
        info.dynamic_model_vertices = dynamic_models
            .iter()
            .map(|surface| surface.vertex_count() as u64)
            .sum();
        info.dynamic_model_indices = dynamic_models
            .iter()
            .map(|surface| surface.index_count() as u64)
            .sum();
        info.cpu_prepare_ms = frame_started.elapsed().as_secs_f64() * 1000.0;

        let frame = self.acquire_render_frame(&mut info)?;
        let surface_view = self.surface_texture_view(&frame);
        let presentation_view = self.hdr_output.as_ref().map_or_else(|| surface_view.clone(), |output| output.view.clone());
        let lens=self.studio_lens.filter(|l|l.aperture.is_some());
        if lens.is_some() {
            let size=[full_width,full_height];
            if self.studio_lens_gpu.as_ref().is_none_or(|g|!g.matches(size,preview_format)) {
                self.studio_lens_gpu=Some(super::super::studio_lens::StudioLensGpu::new(&self.device,size,preview_format));
            }
        } else {self.studio_lens_gpu=None;}
        let frame_view=if lens.is_some(){self.studio_lens_gpu.as_ref().unwrap().view.clone()}else{presentation_view.clone()};
        let mut screenshot_readback=self.prepare_screenshot_readback();
        let clean_video_frame = screenshot_readback.as_ref().is_some_and(|readback|
            matches!(&readback.destination, crate::renderer::ScreenshotDestination::VideoFrame { .. }));
        let transparent_capture = screenshot_readback.as_ref().is_some_and(|readback|
            matches!(&readback.destination, crate::renderer::ScreenshotDestination::VideoFrame { alpha: true, .. }));
        let depth_capture = screenshot_readback.as_ref().is_some_and(|readback| readback.depth_requested);
        if transparent_capture {self.dynamic_model_renderer.prepare_studio_capture(&self.device);}

        let encode_started = Instant::now();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("JKA asset viewer encoder"),
            });
        let (color_view, resolve_target) = if let Some(msaa_view) = &self.targets.msaa_view {
            (msaa_view, Some(&frame_view))
        } else {
            (&frame_view, None)
        };
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("JKA asset viewer model preview"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color_view,
                    depth_slice: None,
                    resolve_target,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(if transparent_capture {
                            wgpu::Color::TRANSPARENT
                        } else if self.studio_world_viewport.is_some() && !clean_video_frame {
                            // Match the Studio world's editor backing color.
                            // Switching between model preview and demo playback
                            // must not flash black against the editor chrome.
                            wgpu::Color {r:0.039,g:0.063,b:0.098,a:1.0}
                        } else { wgpu::Color::BLACK }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(camera_depth_clear()),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if draw_preview {
                pass.set_viewport(
                    viewport[0] as f32,
                    viewport[1] as f32,
                    viewport[2] as f32,
                    viewport[3] as f32,
                    0.0,
                    1.0,
                );
                pass.set_scissor_rect(viewport[0], viewport[1], viewport[2], viewport[3]);
                if !transparent_capture {
                    if let Some(background)=&self.asset_preview_background {
                        if background.visible_in_capture(clean_video_frame) {background.draw(&mut pass);}
                    }
                }
                if transparent_capture {
                    self.dynamic_model_renderer.draw_studio_capture(&mut pass,&self.camera_bind_group);
                } else {
                    self.dynamic_model_renderer.draw(&mut pass,&self.camera_bind_group);
                }
            }
        }
        // Single-sampled radial-depth prepass for isolated studio exports.
        // Reuse the existing skinned Ghoul2/MD3 pipelines and the same
        // 1,000,000-unit background convention used by demo depth EXRs.
        if depth_capture || lens.is_some() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Studio radial depth"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets.linear_depth_render_view,
                    depth_slice: None, resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 1_000_000.0, g: 0.0, b: 0.0, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.ao_depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(camera_depth_clear()),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            });
            pass.set_viewport(viewport[0] as f32, viewport[1] as f32, viewport[2] as f32, viewport[3] as f32, 0.0, 1.0);
            pass.set_scissor_rect(viewport[0], viewport[1], viewport[2], viewport[3]);
            if draw_preview { self.dynamic_model_renderer.draw_rain_prepass(&mut pass, &self.camera_bind_group); }
        }
        if let Some(lens)=lens {
            self.studio_lens_gpu.as_ref().unwrap().encode(&self.device,&self.queue,&mut encoder,
                &frame_view,&self.targets.linear_depth_view,&presentation_view,camera,lens,viewport);
        }
        info.cpu_encode_ms = encode_started.elapsed().as_secs_f64() * 1000.0;
        let submit_started = Instant::now();
        self.queue.submit([encoder.finish()]);
        let frame_view=presentation_view;
        if !clean_video_frame {
            self.submit_egui(&frame_view);
            if self.egui_active {
                self.submit_ui_overlay(&frame_view);
            }
        }
        info.cpu_submit_ms = submit_started.elapsed().as_secs_f64() * 1000.0;
        let present_started = Instant::now();
        if let Some(output) = &self.hdr_output {
            output.submit_with_alpha(&self.device, &self.queue, &surface_view, screenshot_readback.is_some(), transparent_capture);
        }
        // After the HDR output pass: its capture texture holds the final image.
        if let Some(readback)=&mut screenshot_readback{self.submit_screenshot_copy(self.hdr_output.as_ref().map_or(&frame.texture, |output| &output.capture),readback);}
        self.present_render_frame(frame);
        let present_completed_at = Instant::now();
        self.model_frame_log.presented_without_scene(
            present_completed_at,
            camera.position,
            [self.config.width, self.config.height],
        );
        info.cpu_present_ms = present_completed_at
            .saturating_duration_since(present_started)
            .as_secs_f64()
            * 1000.0;
        info.present_call_completed_at = Some(present_completed_at);
        if let Some(readback)=screenshot_readback{self.finish_screenshot_readback(readback);}
        info.frame_ms = frame_started.elapsed().as_secs_f64() * 1000.0;
        Ok(info)
    }
}
