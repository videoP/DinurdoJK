//! Context.
use crate::renderer::{
    create_targets, create_texture_sampler, rebuild_world_bind_groups, Mat4, PhysicalSize,
    Renderer, TextureFilter, VsyncMode, DEPTH_FORMAT,
};

impl Renderer {
    pub(in crate::renderer) fn resize(&mut self, size: PhysicalSize<u32>) {
        self.invalidate_rt_sun_shadow_depth();
        self.size = size;
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
        self.targets = create_targets(
            &self.device,
            size.width,
            size.height,
            self.scene_format(),
            self.msaa_samples,
            self.bloom_enabled || self.halation_enabled,
            self.depth_of_field_strength > 0.001,
            self.ssao_enabled,
            self.ssr_enabled,
            self.cloud_render_resolution,
        );
        self.rebuild_planar_reflection_resources();
        self.rebuild_post_bind_group();
        self.rebuild_rain_haze_mask_bind_group();
        if let Some(target) = &mut self.smaa_target {
            target.resize(&self.device, size.width.max(1), size.height.max(1));
        }
        self.rebuild_gpu_visibility_resources();
        self.history_valid = false;
        self.camera_history_valid = false;
        self.dof_focus_valid = false;
        self.previous_frame_time = 0.0;
        self.previous_unjittered_view_proj = Mat4::IDENTITY;
        self.previous_cloud_view_proj = Mat4::IDENTITY;
        self.cloud_history_valid = false;
        self.cloud_history_read_index = 0;
        self.cloud_temporal_frame_index = 0;
        self.taa_frame_index = 0;
        self.history_read_index = 0;
        self.ssao_history_valid = false;
        self.ssao_history_read_index = 0;
        self.ssr_history_valid = false;
        self.ssr_history_read_index = 0;
        self.ssr_frame_index = 0;
        self.update_post_uniform();
        self.update_lighting_settings();
        self.rebuild_ui();
    }

    pub(in crate::renderer) fn reconfigure(&mut self) {
        self.invalidate_rt_sun_shadow_depth();
        if self.size.width == 0 || self.size.height == 0 {
            return;
        }
        self.surface.configure(&self.device, &self.config);
        self.targets = create_targets(
            &self.device,
            self.config.width,
            self.config.height,
            self.scene_format(),
            self.msaa_samples,
            self.bloom_enabled || self.halation_enabled,
            self.depth_of_field_strength > 0.001,
            self.ssao_enabled,
            self.ssr_enabled,
            self.cloud_render_resolution,
        );
        self.rebuild_planar_reflection_resources();
        self.rebuild_post_bind_group();
        self.rebuild_rain_haze_mask_bind_group();
        if let Some(target) = &mut self.smaa_target {
            target.resize(
                &self.device,
                self.config.width.max(1),
                self.config.height.max(1),
            );
        }
        self.rebuild_gpu_visibility_resources();
        self.history_valid = false;
        self.camera_history_valid = false;
        self.dof_focus_valid = false;
        self.previous_frame_time = 0.0;
        self.previous_unjittered_view_proj = Mat4::IDENTITY;
        self.previous_cloud_view_proj = Mat4::IDENTITY;
        self.cloud_history_valid = false;
        self.cloud_history_read_index = 0;
        self.cloud_temporal_frame_index = 0;
        self.taa_frame_index = 0;
        self.history_read_index = 0;
        self.ssao_history_valid = false;
        self.ssao_history_read_index = 0;
        self.ssr_history_valid = false;
        self.ssr_history_read_index = 0;
        self.ssr_frame_index = 0;
        self.update_post_uniform();
        self.update_lighting_settings();
    }

    /// Reconfigure only the presentation surface. Present mode and frame-queue
    /// changes do not alter render-target formats, sample counts, shaders, or
    /// pipelines, so keep this cold path deliberately smaller than `reconfigure`.
    pub(in crate::renderer) fn reconfigure_presentation_surface(&mut self) {
        if self.size.width == 0 || self.size.height == 0 {
            return;
        }
        self.surface.configure(&self.device, &self.config);
    }

    pub(in crate::renderer) fn set_vsync(&mut self, vsync: VsyncMode) {
        if self.vsync == vsync {
            return;
        }
        self.vsync = vsync;
        self.config.present_mode = present_mode_for_vsync(vsync, &self.present_modes);
        self.reconfigure_presentation_surface();
        rverbose!(
            1,
            "VSync: {} -> present mode {:?}",
            vsync.label(),
            self.config.present_mode
        );
    }

    pub(in crate::renderer) fn set_max_frame_latency(&mut self, latency: u32) {
        let latency = latency.clamp(1, 3);
        if self.config.desired_maximum_frame_latency == latency {
            return;
        }
        self.config.desired_maximum_frame_latency = latency;
        self.reconfigure_presentation_surface();
        rverbose!(1, "Maximum frame latency: {latency}");
    }

    pub(in crate::renderer) fn set_msaa(&mut self, samples: u32) {
        if self.msaa_samples == samples || !self.supported_msaa.contains(&samples) {
            return;
        }
        let samples = if self.gameplay_scene_format() == wgpu::TextureFormat::Rgba16Float
            && !self.hdr_supported_msaa.contains(&samples)
        {
            1
        } else {
            samples
        };
        self.msaa_samples = samples;
        self.request_scene_rebuild();
    }

    pub(in crate::renderer) fn set_texture_filter(&mut self, filter: TextureFilter) {
        if self.texture_filter == filter {
            return;
        }
        self.texture_filter = filter;
        self.repeat_sampler = create_texture_sampler(&self.device, false, filter);
        self.clamp_sampler = create_texture_sampler(&self.device, true, filter);
        if let Some(world) = &mut self.world {
            rebuild_world_bind_groups(
                &self.device,
                &self.surface_layout,
                &self.fast_surface_layout,
                &self.white,
                &self.missing,
                &self.flat_normal,
                &self.repeat_sampler,
                &self.clamp_sampler,
                &self.pbr_repeat_sampler,
                &self.pbr_clamp_sampler,
                &self.lightmap_sampler,
                &self.sky_sampler,
                &self.detail_texture,
                self.detail_texture_auto,
                &self.detail_auto_textures,
                self.surface_deformation.buffer(),
                self.surface_deformation.field_views(),
                self.surface_deformation.field_sampler(),
                world,
            );
            for effect in &mut world.surface_sprite_effects {
                if let Some(texture) = world.textures.get(effect.source.texture) {
                    let sampler = if effect.source.clamp {
                        &self.clamp_sampler
                    } else {
                        &self.repeat_sampler
                    };
                    effect.bind_group = self
                        .surface_sprite_effect_renderer
                        .create_texture_bind_group(&self.device, &texture.view, sampler);
                }
            }
        }
    }
}

pub(in crate::renderer) fn temporal_jitter(frame_index: u32) -> [f32; 2] {
    // Bevy main: Halton (2,3) - 0.5, including the zero-jitter reset phase.
    const BEVY_TAA_HALTON: [[f32; 2]; 8] = [
        [0.0, 0.0],
        [0.0, -0.16666666],
        [-0.25, 0.16666669],
        [0.25, -0.3888889],
        [-0.375, -0.055555552],
        [0.125, 0.2777778],
        [-0.125, -0.2777778],
        [0.375, 0.055555582],
    ];
    BEVY_TAA_HALTON[(frame_index as usize) % BEVY_TAA_HALTON.len()]
}

pub(in crate::renderer) fn supported_msaa(
    adapter: &wgpu::Adapter,
    surface_format: wgpu::TextureFormat,
) -> Vec<u32> {
    let color = adapter.get_texture_format_features(surface_format).flags;
    let depth = adapter.get_texture_format_features(DEPTH_FORMAT).flags;
    let can_resolve = color.contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_RESOLVE);
    let mut result = vec![1];
    if can_resolve {
        for samples in [2_u32, 4, 8, 16] {
            if color.sample_count_supported(samples) && depth.sample_count_supported(samples) {
                result.push(samples);
            }
        }
    }
    result
}

pub(in crate::renderer) fn no_vsync_mode(modes: &[wgpu::PresentMode]) -> wgpu::PresentMode {
    if modes.contains(&wgpu::PresentMode::Immediate) {
        wgpu::PresentMode::Immediate
    } else {
        wgpu::PresentMode::AutoNoVsync
    }
}

pub(in crate::renderer) fn present_mode_for_vsync(
    mode: VsyncMode,
    supported: &[wgpu::PresentMode],
) -> wgpu::PresentMode {
    match mode {
        VsyncMode::Off => no_vsync_mode(supported),
        VsyncMode::On => wgpu::PresentMode::Fifo,
        VsyncMode::Fast => {
            if supported.contains(&wgpu::PresentMode::Mailbox) {
                wgpu::PresentMode::Mailbox
            } else {
                wgpu::PresentMode::Fifo
            }
        }
        VsyncMode::Adaptive => {
            if supported.contains(&wgpu::PresentMode::FifoRelaxed) {
                wgpu::PresentMode::FifoRelaxed
            } else {
                wgpu::PresentMode::Fifo
            }
        }
    }
}
