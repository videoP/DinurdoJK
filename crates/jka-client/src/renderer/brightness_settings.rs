//! Scene ownership and cold brightness state transitions.
//! JK2MV encoded-space gamma LUT. Built only for a settings transition, never
//! for a rendered frame. 1024 samples in RGBA16F (8 KiB), bilinear filtered.
use super::{GammaPostUniform, Renderer};

const JK2_GAMMA_LUT_SIZE: u32 = 1024;

pub(in crate::renderer) fn srgb_encode(v: f32) -> f32 {
    if v <= 0.0031308 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

pub(in crate::renderer) fn srgb_decode(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

// JK2MV's encoded-space gamma and output overbright, reused when composing
// it into the grading cube. Both LUT paths must use exactly the same transfer.
pub(in crate::renderer) fn jk2_output_transfer(linear: f32, gamma: f32, overbright_bits: u8) -> f32 {
    let gain = if overbright_bits > 0 { 2.0 } else { 1.0 };
    let encoded = srgb_encode(linear.clamp(0.0, 1.0)).powf(1.0 / gamma.clamp(0.05, 3.0));
    srgb_decode((encoded * gain).min(1.0))
}

fn jk2_gamma_lut_bytes(gamma: f32, overbright_bits: u8) -> Vec<u8> {
    let mut samples = Vec::with_capacity((JK2_GAMMA_LUT_SIZE * 8) as usize);
    for index in 0..JK2_GAMMA_LUT_SIZE {
        // Square the lookup coordinate: more precision where gamma > 1
        // steepens the near-black response. The GPU only pays a sqrt.
        let coordinate = index as f32 / (JK2_GAMMA_LUT_SIZE - 1) as f32;
        let linear = coordinate * coordinate;
        let corrected = jk2_output_transfer(linear, gamma, overbright_bits);
        let sample = crate::materials::f32_to_f16_bits(corrected);
        for component in [sample, sample, sample, crate::materials::f32_to_f16_bits(1.0)] {
            samples.extend_from_slice(&component.to_le_bytes());
        }
    }
    samples
}

pub(in crate::renderer) fn create_jk2_gamma_lut(
    device: &wgpu::Device, queue: &wgpu::Queue, gamma: f32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("JK2MV gamma/overbright lookup (1024 RGBA16F)"),
        size: wgpu::Extent3d { width: JK2_GAMMA_LUT_SIZE, height: 1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    upload_jk2_gamma_lut(queue, &texture, gamma, 0);
    let view = texture.create_view(&Default::default());
    (texture, view)
}

fn upload_jk2_gamma_lut(queue: &wgpu::Queue, texture: &wgpu::Texture, gamma: f32, bits: u8) {
    queue.write_texture(
        texture.as_image_copy(),
        &jk2_gamma_lut_bytes(gamma, bits),
        wgpu::TexelCopyBufferLayout {
            offset: 0, bytes_per_row: Some(JK2_GAMMA_LUT_SIZE * 8), rows_per_image: Some(1),
        },
        wgpu::Extent3d { width: JK2_GAMMA_LUT_SIZE, height: 1, depth_or_array_layers: 1 },
    );
}

impl Renderer {
    /// Math-only HDR changes scene precision, not JK2's SDR color transfer.
    /// Only native HDR presentation uses the existing linear HDR policy.
    pub(in crate::renderer) fn jk2_sdr_color(&self) -> bool {
        self.jk2_legacy_color && self.hdr_output.is_none()
    }

    /// Shared by compact and full/temporal post (0 = linear, 1 = legacy
    /// transfer LUT, 2 = legacy transfer fused into the grading cube).
    pub(in crate::renderer) fn jk2_output_lut_mode(&self) -> f32 {
        if !self.jk2_sdr_color() { 0.0 }
        else if self.jk2_gamma_baked { 2.0 }
        else { 1.0 }
    }

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
        // Do not generate a LUT for JKA, or regenerate one for unrelated UI
        // and color-grade changes. Only a JK2 gamma/overbright transition uploads.
        let mut output_changed = false;
        if self.jk2_legacy_color {
            let key = (self.output_gamma().to_bits(), self.overbright_bits);
            if self.jk2_gamma_lut_key != key {
                upload_jk2_gamma_lut(&self.queue, &self._jk2_gamma_lut_texture,
                    self.output_gamma(), self.overbright_bits);
                self.jk2_gamma_lut_key = key;
                output_changed = true;
            }
        }
        let wants_fused = self.should_fuse_jk2_output_lut();
        // Also rebuild on color-policy transitions: the same grading settings
        // require a different texture when JK2's output transfer is fused.
        if ((wants_fused && output_changed)
            || wants_fused != self.jk2_gamma_baked
            || (self.split_toning.active() && !wants_fused))
            && self.rebuild_color_grading_lut()
        {
            self.rebuild_color_lut_bind_groups();
        }
        self.queue.write_buffer(
            &self.gamma_post_buffer,
            0,
            bytemuck::bytes_of(&GammaPostUniform {
                values: [
                    self.output_gamma(),
                    if self.jk2_gamma_baked { 1.0 } else { self.color_lut_effective_strength },
                    if self.vignette_enabled { 1.0 } else { 0.0 },
                    self.color_grade.exposure_ev,
                ],
                flags: [if self.hdr_enabled { 1.0 } else { 0.0 }, if self.hdr_output.is_some() { 1.0 } else { 0.0 }, f32::from(self.overbright_bits), self.jk2_output_lut_mode()],
            }),
        );
        self.rebuild_frame_plan();
        self.sync_ui_gamma_uniforms();
        self.update_post_uniform();
        self.rebuild_ui();
    }
}

#[cfg(test)]
mod jk2_gamma_lut_tests {
    use super::*;

    #[test]
    fn jk2_gamma_lut_has_correct_endpoints_and_neutral_identity() {
        let bytes = jk2_gamma_lut_bytes(1.0, 0);
        assert_eq!(bytes.len(), (JK2_GAMMA_LUT_SIZE * 8) as usize);
        assert_eq!(&bytes[0..2], &[0, 0]);
        assert_eq!(&bytes[bytes.len() - 8..bytes.len() - 6],
            &crate::materials::f32_to_f16_bits(1.0).to_le_bytes());
    }
    #[test]
    fn legacy_overbright_saturates_at_one() {
        let bytes = jk2_gamma_lut_bytes(1.0, 1);
        let last = bytes.len() - 8;
        assert_eq!(&bytes[last..last + 2],
            &crate::materials::f32_to_f16_bits(1.0).to_le_bytes());
        assert_ne!(&bytes[1024..1026],
            &jk2_gamma_lut_bytes(1.0, 0)[1024..1026]);
    }
}
