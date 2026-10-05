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
        let uniform = CameraUniform {
            view_proj: view_proj.to_cols_array_2d(),
            camera_pos_time: [
                camera.position.x,
                camera.position.y,
                camera.position.z,
                self.started.elapsed().as_secs_f32(),
            ],
            clip_plane: [0.0; 4],
            render_flags: [0, 0, 0, 0],
            camera_forward: camera.forward().extend(0.0).to_array(),
            unjittered_view_proj: view_proj.to_cols_array_2d(),
            previous_unjittered_view_proj: view_proj.to_cols_array_2d(),
            jump_shade: [0.0; 4],
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
        );
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

        let acquire_started = Instant::now();
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout => {
                self.flush_staged_queue_writes();
                return Err(RenderError::Timeout);
            }
            wgpu::CurrentSurfaceTexture::Occluded => {
                self.flush_staged_queue_writes();
                return Err(RenderError::Occluded);
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.flush_staged_queue_writes();
                return Err(RenderError::Outdated);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.flush_staged_queue_writes();
                return Err(RenderError::Lost);
            }
            wgpu::CurrentSurfaceTexture::Validation => return Err(RenderError::Validation),
        };
        info.cpu_acquire_ms = acquire_started.elapsed().as_secs_f64() * 1000.0;
        let frame_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

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
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
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
                self.dynamic_model_renderer
                    .draw(&mut pass, &self.camera_bind_group);
            }
        }
        info.cpu_encode_ms = encode_started.elapsed().as_secs_f64() * 1000.0;
        let submit_started = Instant::now();
        self.queue.submit([encoder.finish()]);
        self.submit_egui(&frame_view);
        if self.egui_active {
            self.submit_ui_overlay(&frame_view);
        }
        info.cpu_submit_ms = submit_started.elapsed().as_secs_f64() * 1000.0;
        let present_started = Instant::now();
        self.window.pre_present_notify();
        frame.present();
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
        info.frame_ms = frame_started.elapsed().as_secs_f64() * 1000.0;
        Ok(info)
    }
}
