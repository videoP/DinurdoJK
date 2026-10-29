//! Context.
use crate::renderer::{
    create_targets, create_texture_sampler, rebuild_world_bind_groups, Mat4, PhysicalSize,
    Renderer, TextureFilter, VsyncMode, DEPTH_FORMAT,
};

pub(in crate::renderer) struct RenderFrame {
    pub texture: wgpu::Texture,
    surface: Option<wgpu::SurfaceTexture>,
}

impl Renderer {
    pub(in crate::renderer) fn acquire_render_frame(&mut self, info: &mut crate::renderer::FrameInfo)
        -> Result<RenderFrame, crate::renderer::RenderError> {
        if let Some(texture) = &self.offscreen_output {
            return Ok(RenderFrame { texture: texture.clone(), surface: None });
        }
        let surface = self.acquire_surface_frame(info)?;
        Ok(RenderFrame { texture: surface.texture.clone(), surface: Some(surface) })
    }

    pub(in crate::renderer) fn present_render_frame(&self, frame: RenderFrame) {
        if let Some(surface) = frame.surface {
            self.window.pre_present_notify();
            self.queue.present(surface);
        }
    }

    pub(in crate::renderer) fn make_offscreen_output(&self) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Independent Studio scene output"),
            size: wgpu::Extent3d { width: self.config.width.max(1), height: self.config.height.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }
}

/// The configuration actually handed to `Surface::configure`.
///
/// DXGI flip-model swapchains do not accept sRGB formats, and Intel's Windows
/// Vulkan driver presents through DXGI: with an sRGB swapchain it falls back to
/// "Composed: Copy with GPU GDI" (PresentMon), an extra GPU copy plus a blocking
/// GDI hand-off every frame. TaystJK's Vulkan renderer uses B8G8R8A8_UNORM. Keep
/// `Renderer::config.format` as the sRGB render format (all pipelines and views
/// use it) but present a UNORM swapchain that allows that sRGB view.
/// `JKA_SURFACE_SRGB_DIRECT=1` restores the sRGB swapchain for A/B tests.
pub(in crate::renderer) fn presentation_config(
    config: &wgpu::SurfaceConfiguration,
    linear_present_supported: bool,
) -> wgpu::SurfaceConfiguration {
    let mut config = config.clone();
    let linear = config.format.remove_srgb_suffix();
    let srgb_direct = std::env::var_os("JKA_SURFACE_SRGB_DIRECT").is_some_and(|v| v == "1");
    if linear != config.format && linear_present_supported && !srgb_direct {
        config.view_formats = vec![config.format];
        config.format = linear;
    }
    config
}

/// The scene/compositor format differs from the swapchain format when native
/// HDR output is actually available. On SDR fallback there is no compositor. A surface texture view must use the format
/// selected for the *presentation surface*, not the FP16 compositor.
#[inline]
fn surface_view_format(
    scene_format: wgpu::TextureFormat,
    hdr_surface_format: Option<wgpu::TextureFormat>,
) -> wgpu::TextureFormat {
    hdr_surface_format.unwrap_or(scene_format)
}

impl Renderer {
    /// Texture views in bind groups retain the old textures after target replacement.
    /// Refresh only the inputs; the resolve pipeline and layout remain compatible.
    fn rebind_scene_depth_resolve(&mut self) {
        // A pending MSAA change can also replace the single/multisampled layout.
        // The scene rebuild requested below recreates that pipeline and its binding
        // together before the next frame, so do not bind it to incompatible targets.
        if self.scene_pipeline_compat.1 != self.msaa_samples {
            return;
        }
        self.ssr_visibility_bind_group = self._ssr_visibility_layout.as_ref().map(|layout| {
            super::post::pipelines::create_ssr_visibility_bind_group(
                &self.device,
                layout,
                &self.targets,
            )
        });
    }

    /// View of the acquired swapchain image. In the normal SDR path this can
    /// be an sRGB view of a UNORM swapchain (see `presentation_config`). Native
    /// HDR output instead uses the selected native HDR surface format,
    /// never the internal Rgba16Float compositor format.
    pub(in crate::renderer) fn surface_texture_view(&self, frame: &RenderFrame) -> wgpu::TextureView {
        let format = surface_view_format(
            self.config.format,
            self.hdr_output.as_ref().map(|output| output.selection.format),
        );
        frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(format),
            ..Default::default()
        })
    }

    #[inline]
    pub(in crate::renderer) fn note_surface_configured(&mut self, reason: &'static str) {
        self.surface_config_generation = self.surface_config_generation.saturating_add(1);
        self.surface_config_reason = reason;
        self.surface_configured_at = std::time::Instant::now();
    }

    pub(in crate::renderer) fn resize(&mut self, size: PhysicalSize<u32>) {
        // Windows can repeat a resize notification without changing dimensions.
        // Reallocating every target then briefly retains two full target sets.
        if self.size == size { return; }
        self.size = size;
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        if self.offscreen_output.is_some() {
            self.offscreen_output = Some(self.make_offscreen_output());
        }
        self.configure_surface();
        self.note_surface_configured("resize");
        self.resize_hdr_output();
        self.targets = create_targets(
            &self.device,
            size.width,
            size.height,
            self.scene_format(),
            self.msaa_samples,
            self.glow_mode.enabled() || self.halation_enabled,
            self.glow_mode.authored(),
            self.depth_of_field_strength > 0.001,
            self.ssao_enabled,
            self.ssr_enabled,
            self.needs_resolved_depth_target(),
            self.taa_enabled || self.rt_taa_mode > 0,
            self.clouds_enabled && self.cloud_backend == 0,
            self.cloud_render_resolution,
        );
        self.world_flares.invalidate_depth_binding();
        self.world_flares_sky.invalidate_depth_binding();
        self.rebind_scene_depth_resolve();
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
        // If resize recreated the target while a format/MSAA setting was still
        // pending, schedule the matching pipeline rebuild before the next frame.
        if self.scene_pipeline_compat != (self.scene_format(), self.msaa_samples) {
            self.request_scene_rebuild();
        }
        self.rebuild_ui();
    }

    pub(in crate::renderer) fn reconfigure(&mut self, reason: &'static str) {
        if self.size.width == 0 || self.size.height == 0 {
            return;
        }
        self.configure_surface();
        self.note_surface_configured(reason);
        self.resize_hdr_output();
        self.targets = create_targets(
            &self.device,
            self.config.width,
            self.config.height,
            self.scene_format(),
            self.msaa_samples,
            self.glow_mode.enabled() || self.halation_enabled,
            self.glow_mode.authored(),
            self.depth_of_field_strength > 0.001,
            self.ssao_enabled,
            self.ssr_enabled,
            self.needs_resolved_depth_target(),
            self.taa_enabled || self.rt_taa_mode > 0,
            self.clouds_enabled && self.cloud_backend == 0,
            self.cloud_render_resolution,
        );
        self.world_flares.invalidate_depth_binding();
        self.world_flares_sky.invalidate_depth_binding();
        self.rebind_scene_depth_resolve();
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
        // Surface loss/reconfiguration is also allowed to recreate textures;
        // never mistake that for recompiling pipelines for a new scene format.
        if self.scene_pipeline_compat != (self.scene_format(), self.msaa_samples) {
            self.request_scene_rebuild();
        }
    }

    /// Reconfigure only the presentation surface. Present mode and frame-queue
    /// changes do not alter render-target formats, sample counts, shaders, or
    /// pipelines, so keep this cold path deliberately smaller than `reconfigure`.
    pub(in crate::renderer) fn reconfigure_presentation_surface(&mut self, reason: &'static str) {
        if self.size.width == 0 || self.size.height == 0 {
            return;
        }
        self.configure_surface();
        self.note_surface_configured(reason);
    }

    pub(in crate::renderer) fn set_vsync(&mut self, vsync: VsyncMode) {
        if self.vsync == vsync {
            return;
        }
        let previous_present_mode = self.config.present_mode;
        let configure_started = std::time::Instant::now();
        self.vsync = vsync;
        self.config.present_mode = present_mode_for_vsync(vsync, &self.present_modes);
        self.reconfigure_presentation_surface("vsync");
        // This is wgpu's actual configured surface state (not just our mirror
        // in self.config). It cannot reveal any OS compositor/driver override.
        let applied = self.surface.get_configuration();
        if self.size.width > 0 && self.size.height > 0 && applied.as_ref().is_some_and(|config| {
            config.present_mode != self.config.present_mode
                || config.desired_maximum_frame_latency
                    != self.config.desired_maximum_frame_latency
        }) {
            eprintln!("[JKA PRESENT] WARNING: requested presentation configuration differs from surface configuration: {applied:?}");
        }
        rverbose!(
            1,
            "VSync: {:?} -> {:?} (requested {}, surface {:?}, queue {}, configure {:.3} ms, surface generation {})",
            previous_present_mode,
            self.config.present_mode,
            vsync.label(),
            applied.as_ref().map(|config| config.present_mode),
            self.config.desired_maximum_frame_latency,
            configure_started.elapsed().as_secs_f64() * 1000.0,
            self.surface_config_generation
        );
    }

    pub(in crate::renderer) fn set_max_frame_latency(&mut self, latency: u32) {
        let latency = latency.clamp(1, 3);
        if self.config.desired_maximum_frame_latency == latency {
            return;
        }
        self.config.desired_maximum_frame_latency = latency;
        self.reconfigure_presentation_surface("max_frame_latency");
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

#[cfg(test)]
mod presentation_view_tests {
    use super::surface_view_format;
    use wgpu::TextureFormat as Format;

    #[test]
    fn native_hdr_and_sdr_fallback_use_the_actual_swapchain_view_format() {
        // The internal compositor remains FP16 in all three cases.
        let scene = Format::Rgba16Float;
        assert_eq!(surface_view_format(scene, Some(Format::Rgb10a2Unorm)), Format::Rgb10a2Unorm);
        assert_eq!(surface_view_format(scene, Some(Format::Rgba16Float)), Format::Rgba16Float);
        assert_eq!(surface_view_format(scene, Some(Format::Bgra8Unorm)), Format::Bgra8Unorm);
        assert_eq!(surface_view_format(scene, Some(Format::Bgra8UnormSrgb)), Format::Bgra8UnormSrgb);
    }

    #[test]
    fn ordinary_sdr_retains_its_srgb_render_view() {
        // SDR uses an sRGB render view of a UNORM presentation surface.
        assert_eq!(surface_view_format(Format::Bgra8UnormSrgb, None), Format::Bgra8UnormSrgb);
    }
}
