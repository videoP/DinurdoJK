//! Scene ownership and cold brightness state transitions.
use super::{GammaPostUniform, Renderer};

impl Renderer {
    pub(super) fn output_gamma(&self) -> f32 {
        if self.gamma_method.is_hardware()
            || self.baked_brightness.overrides_output()
        {
            1.0
        } else {
            self.gamma
        }
    }

    /// Reuse existing textures/views/bind groups; exclude UI, lighting/data maps,
    /// and videoMap textures that are overwritten by streaming frame uploads.
    pub(super) fn sync_baked_brightness_assets(&mut self) {
        if !self.baked_brightness.enabled && !self.baked_brightness.busy {
            return;
        }
        let old_output_gamma = self.output_gamma();
        // Baked gamma writes texels. Detach shared immutable color images
        // before registering them, and rebuild bindings to those private copies.
        if self.baked_brightness.enabled {
            if let Some(mut world) = self.world.take() {
                let mut changed = false;
                for image in &mut world.textures {
                    if image._texture.format() == wgpu::TextureFormat::Rgba8UnormSrgb && image.is_shared() {
                        changed |= image.detach_for_write(&self.device, &self.queue);
                    }
                }
                if changed { self.rebuild_world_surface_bind_groups(&mut world); }
                self.world = Some(world);
            }
        }
        let mut textures = Vec::new();
        if let Some(world) = &self.world {
            textures.extend(
                world
                    .textures
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| {
                        !self
                            .world_videos
                            .iter()
                            .any(|video| video.texture == *index)
                    })
                    .map(|(_, image)| image._texture.clone()),
            );
        }
        textures.extend(
            self.dynamic_model_renderer
                .textures
                .values()
                .map(|value| value._image._texture.clone()),
        );
        textures.extend(
            self.detail_auto_textures
                .values()
                .map(|image| image._texture.clone()),
        );
        textures.extend(
            self.screen_fx_textures
                .values()
                .map(|value| value._image._texture.clone()),
        );
        textures.push(self.detail_texture._texture.clone());
        textures.push(self.ocean_spray_albedo._texture.clone());
        self.baked_brightness.retain_live(&textures);
        for texture in textures {
            self.baked_brightness.register(&texture);
        }
        if self.output_gamma() != old_output_gamma {
            self.refresh_output_gamma();
        }
    }

    pub(super) fn set_gamma_method(&mut self, method: crate::gamma::GammaMethod) {
        if method == crate::gamma::GammaMethod::Baked && self.dynamic_model_renderer.textures.values().any(|t| t._image.is_shared()) {
            self.dynamic_model_renderer.textures.clear();
            self.dynamic_model_renderer.frame_texture_keys.clear();
            self.dynamic_model_renderer.model_bundles.clear();
        }
        self.gamma_method = method;
        if method == crate::gamma::GammaMethod::HardwareSceneOnly {
            self.ensure_native_ui_gamma();
        } else {
            // The large UI intermediate is only needed for this strategy.
            self.ui_gamma_egui = None;
        }
        self.baked_brightness
            .set_target(method == crate::gamma::GammaMethod::Baked, self.gamma);
        self.sync_baked_brightness_assets();
        self.refresh_output_gamma();
    }

    #[cold]
    pub(super) fn tick_baked_brightness(&mut self) {
        if self
            .baked_brightness
            .tick(&self.device, &self.queue, &mut self.pipeline_jobs)
        {
            self.refresh_output_gamma();
        }
    }

    pub(super) fn refresh_output_gamma(&mut self) {
        if self.split_toning.active() && self.rebuild_color_grading_lut() {
            self.rebuild_color_lut_bind_groups();
        }
        self.queue.write_buffer(
            &self.gamma_post_buffer,
            0,
            bytemuck::bytes_of(&GammaPostUniform {
                values: [
                    self.output_gamma(),
                    self.color_lut_effective_strength,
                    if self.vignette_enabled { 1.0 } else { 0.0 },
                    self.color_grade.exposure_ev,
                ],
                flags: [if self.hdr_enabled { 1.0 } else { 0.0 }, if self.hdr_output.is_some() { 1.0 } else { 0.0 }, 0.0, 0.0],
            }),
        );
        self.rebuild_frame_plan();
        self.sync_ui_gamma_uniforms();
        self.update_post_uniform();
        self.rebuild_ui();
    }
}
