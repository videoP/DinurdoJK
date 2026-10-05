//! Display.
use crate::app::egui_settings::{
    config, index_of, optional_quality_table_row, pending_video_reset, segmented_row, theme, ui,
    App, FullscreenMode, QualityPreset, RendererBackend, VideoSection, VsyncMode,
};

impl App {
    pub(in crate::app) fn egui_video_page(&mut self, ui: &mut egui::Ui) {
        // Install the pending-row palette before building the page. The list is
        // derived from App's authoritative applied/prepared state, not a second
        // UI dirty bit, so orange always means the next Apply will consume it.
        let pending_resets = self
            .pending_video_changes()
            .iter()
            .filter_map(|change| pending_video_reset(&change.key))
            .collect();
        theme::set_pending_resets(ui.ctx(), pending_resets);

        let (title, detail) = match self.video_section {
            VideoSection::Display => ("DISPLAY & FRAME PACING", "Window mode, timing and output."),
            VideoSection::ImageQuality => (
                "IMAGE QUALITY",
                "Edge sampling, texture reconstruction and framebuffer precision.",
            ),
            VideoSection::Visibility => (
                "VISIBILITY & GEOMETRY",
                "World visibility and GPU culling paths.",
            ),
            VideoSection::Models => (
                "MODELS",
                "Ghoul2 skinning, authored GLM detail and model submission.",
            ),
            VideoSection::Lighting => ("LIGHTING", "Ambient, dynamic and indirect lighting."),
            VideoSection::Effects => (
                "EFFECTS",
                "Particle effects, saber presentation, flares and impact marks.",
            ),
            VideoSection::Shadows => ("SHADOWS", "Dynamic and local shadowing."),
            VideoSection::Reflections => (
                "REFLECTIONS",
                "Environment probes, screen-space reflections and dynamically budgeted planar views.",
            ),
            VideoSection::Color => ("COLOR", "Film looks, shadow and highlight tints, and brightness."),
            VideoSection::PostProcessing => (
                "POST PROCESSING",
                "Lens effects, blur and film texture.",
            ),
            VideoSection::DebugTools => (
                "DEBUG & TOOLS",
                "Renderer diagnostics and instrumentation.",
            ),
            VideoSection::Physics => (
                "PHYSICS",
                "Client-side visual simulation for ragdolls, dynamic props and debris.",
            ),
            VideoSection::Sun => (
                "SUN",
                "Runtime q3 shader-sun direction, intensity and chromaticity.",
            ),
            VideoSection::Clouds => ("CLOUDS", "Volumetric cloud deck and shaping controls."),
            VideoSection::Weather => (
                "WEATHER",
                "Shared world wind, fog, precipitation and atmospheric effects.",
            ),
            VideoSection::Surface => (
                "SURFACE INTERACTION",
                "Footprints and procedural ground detail.",
            ),
            VideoSection::Water => ("WATER", "FFT ocean simulation and promoted water surfaces."),
        };
        theme::page_title(ui, title, detail);

        egui::ScrollArea::vertical()
            .id_salt(("jka_video_section", title))
            .auto_shrink([false, false])
            .show(ui, |ui| match self.video_section {
                VideoSection::Display => self.egui_display_settings(ui),
                VideoSection::ImageQuality => self.egui_image_quality(ui),
                VideoSection::Visibility => self.egui_visibility(ui),
                VideoSection::Models => self.egui_models(ui),
                VideoSection::Lighting => self.egui_lighting(ui),
                VideoSection::Effects => self.egui_effects(ui),
                VideoSection::Shadows => self.egui_shadows(ui),
                VideoSection::Reflections => self.egui_reflections(ui),
                VideoSection::PostProcessing => self.egui_post_processing(ui),
                VideoSection::Color => self.egui_color_settings(ui),
                VideoSection::DebugTools => self.egui_debug_tools(ui),
                VideoSection::Physics => self.egui_physics(ui),
                VideoSection::Sun => self.egui_sun(ui),
                VideoSection::Clouds => self.egui_clouds(ui),
                VideoSection::Weather => self.egui_weather(ui),
                VideoSection::Surface => self.egui_surface(ui),
                VideoSection::Water => self.egui_water(ui),
            });

        // Drained after the page is built: a label click posts its tag while
        // the rows are being laid out, and applying it there would mutate the
        // settings mid-frame.
        if let Some(reset) = theme::take_reset(ui.ctx()) {
            self.apply_setting_reset(reset);
        }
        theme::clear_pending_resets(ui.ctx());
    }

    // ------------------------------------------------------------ rendering --

    pub(in crate::app::egui_settings) fn egui_display_settings(&mut self, ui: &mut egui::Ui) {
        const PRESETS: [(QualityPreset, &str); 5] = [
            (QualityPreset::Minimal, "Minimal (Legacy)"),
            (QualityPreset::MinimalUnified, "Minimal (Unified)"),
            (QualityPreset::Low, "Low"),
            (QualityPreset::Medium, "Medium"),
            (QualityPreset::High, "High"),
        ];
        if let Some(index) = optional_quality_table_row(
            ui,
            "Quality preset",
            "Sets every rendering cost lever at once. Resolution, display mode, \
             render backend, vsync, frame queue, FPS cap and the debug toggles are left \
             alone. The bar becomes Custom once you change a preset-controlled setting. \
             Minimal (Legacy) and Minimal (Unified) have identical settings; Legacy uses \
             the known-fast world renderer and Unified the general one, for comparison.",
            theme::Reset::Video(ui::VIDEO_ROW_QUALITY_PRESET),
            self.active_quality_preset(),
            &PRESETS,
        ) {
            self.video_selected = ui::VIDEO_ROW_QUALITY_PRESET;
            self.apply_quality_preset(PRESETS[index].0);
        }

        const BACKENDS: [(RendererBackend, &str); 2] = [
            (RendererBackend::Vulkan, "Vulkan"),
            (RendererBackend::Dx12, "DX12"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "Render backend",
            "Graphics API the renderer runs on. Driver quality differs per \
             vendor, so try the other one if something looks wrong. DX12 caps \
             presentation at three frames per refresh; Vulkan does not. Needs \
             Apply.",
            theme::Reset::Video(ui::VIDEO_ROW_RENDER_BACKEND),
            self.video.renderer_backend,
            &BACKENDS,
        ) {
            let current = usize::from(self.video.renderer_backend == RendererBackend::Dx12);
            let next = usize::from(target == RendererBackend::Dx12);
            self.video_selected = ui::VIDEO_ROW_RENDER_BACKEND;
            self.change_video_setting(next as i32 - current as i32);
        }
        if self.video.renderer_backend != self.applied_renderer_backend {
            theme::hint(ui, "Render backend changed: needs Apply", theme::WARNING);
        }

        const DISPLAY_MODES: [(FullscreenMode, &str); 3] = [
            (FullscreenMode::Windowed, "Windowed"),
            (FullscreenMode::Borderless, "Borderless"),
            (FullscreenMode::Exclusive, "Exclusive"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "Display mode",
            "Windowed keeps the desktop usable. Borderless keeps the desktop video mode. \
             Vulkan Exclusive switches the monitor video mode. Direct3D 12 has no classic \
             fullscreen-exclusive mode; its Exclusive option uses the HWND flip-model/FSO \
             path that Windows can promote to DirectFlip / Independent Flip. Needs Apply.",
            theme::Reset::Video(ui::VIDEO_ROW_FULLSCREEN),
            self.video.fullscreen,
            &DISPLAY_MODES,
        ) {
            let current = index_of(&DISPLAY_MODES, self.video.fullscreen, 0);
            let next = index_of(&DISPLAY_MODES, target, current);
            self.video_selected = ui::VIDEO_ROW_FULLSCREEN;
            self.change_video_setting(next as i32 - current as i32);
        }
        if self.video.fullscreen != self.applied_fullscreen {
            theme::hint(ui, "Display mode changed: needs Apply", theme::WARNING);
        }

        theme::row(
            ui,
            "Companion window",
            "Opens a native second-monitor workspace for persistent Console, Scoreboard and Server info. The auxiliary surface has its own sleeping render worker and only submits when its contents change. This is also the presentation-surface foundation for a later spectator Scene View.",
            theme::Reset::None,
            |ui| {
                if let Some(enabled) = theme::switch(ui, self.companion_enabled()) {
                    self.set_companion_enabled(enabled);
                }
            },
        );
        if self.companion_enabled() && self.applied_fullscreen == FullscreenMode::Exclusive {
            theme::hint(
                ui,
                "Exclusive fullscreen can lose focus when you click the companion; passive display remains safe.",
                theme::TEXT_DIM,
            );
        }

        let resolutions =
            Self::available_resolutions(self.window.as_deref(), self.video.resolution);
        theme::row(
            ui,
            "Resolution",
            "Backbuffer size the world is rendered at. Lower resolutions cost \
             less on the GPU at the price of sharpness. Needs Apply.",
            theme::Reset::Video(ui::VIDEO_ROW_RESOLUTION),
            |ui| {
                egui::ComboBox::from_id_salt("video_resolution")
                    .selected_text(format!(
                        "{} × {}",
                        self.video.resolution[0], self.video.resolution[1]
                    ))
                    .width(190.0)
                    .show_ui(ui, |ui| {
                        for resolution in resolutions {
                            if ui
                                .selectable_label(
                                    self.video.resolution == resolution,
                                    format!("{} × {}", resolution[0], resolution[1]),
                                )
                                .clicked()
                            {
                                self.video.resolution = resolution;
                                self.mark_config_dirty();
                                self.console_status = format!(
                                    "RESOLUTION: {}X{} (PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART)",
                                    resolution[0], resolution[1]
                                );
                            }
                        }
                    });
                if self.video.resolution != self.applied_resolution {
                    theme::hint(ui, "needs apply", theme::WARNING);
                }
            },
        );

        const VSYNC: [(VsyncMode, &str); 4] = [
            (VsyncMode::Off, "Off"),
            (VsyncMode::On, "On"),
            (VsyncMode::Fast, "Fast"),
            (VsyncMode::Adaptive, "Adaptive"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "VSync",
            "Off tears but has the lowest latency. On locks to the refresh rate. \
             Fast presents the newest frame without blocking. Adaptive drops \
             sync only when the frame rate falls below the refresh rate.",
            theme::Reset::Video(ui::VIDEO_ROW_VSYNC),
            self.video.vsync,
            &VSYNC,
        ) {
            let current = index_of(&VSYNC, self.video.vsync, 0);
            let next = index_of(&VSYNC, target, current);
            self.video_selected = ui::VIDEO_ROW_VSYNC;
            self.change_video_setting(next as i32 - current as i32);
        }

        const FRAME_LATENCY: [(u32, &str); 3] =
            [(1, "1 Lowest"), (2, "2 Balanced"), (3, "3 Throughput")];
        if let Some(target) = segmented_row(
            ui,
            "Frame queue",
            "Maximum WGPU presentation frames in flight. 1 favors the lowest \
             presentation/input latency, 2 balances latency and throughput, and 3 \
             favors maximum throughput. Changes apply live through a surface-only \
             reconfigure: no shader or render-pipeline rebuild. Useful for A/B testing.",
            theme::Reset::Video(ui::VIDEO_ROW_MAX_FRAME_LATENCY),
            self.video.max_frame_latency,
            &FRAME_LATENCY,
        ) {
            let current = index_of(&FRAME_LATENCY, self.video.max_frame_latency, 2);
            let next = index_of(&FRAME_LATENCY, target, current);
            self.video_selected = ui::VIDEO_ROW_MAX_FRAME_LATENCY;
            self.change_video_setting(next as i32 - current as i32);
        }

        let fps_cap_tip = match self.present_fps_ceiling() {
            Some(ceiling) => format!(
                "Upper bound on rendered frames per second. DX12 with FAST vsync \
                 presents without the tearing flag, so frames retire at vblank and \
                 stop at {ceiling} fps here. Higher values snap back to it. Switch \
                 VSync to Off for an uncapped, tearing present path."
            ),
            None => "Upper bound on rendered frames per second. 0 is unlimited. A cap a \
                     little under the refresh rate keeps input latency steady."
                .to_owned(),
        };
        theme::row(
            ui,
            "Frame rate cap",
            &fps_cap_tip,
            theme::Reset::Video(ui::VIDEO_ROW_FPS_CAP),
            |ui| {
                let enforced = self.present_fps_ceiling();
                let mut cap = self.effective_fps_cap();
                let changed = ui
                    .add(
                        egui::DragValue::new(&mut cap)
                            .range(0..=config::FPS_CAP_MAX)
                            .speed(10.0)
                            .suffix(" fps")
                            // Typing "142" would otherwise apply 1, then 14, and
                            // a one-frame-per-second cap locks the window up long
                            // before the rest of the number arrives.
                            .update_while_editing(false),
                    )
                    .changed();
                ui.add_space(10.0);
                let state = match enforced {
                    Some(ceiling) if ceiling == cap => "Present limit",
                    _ if cap >= config::FPS_CAP_MAX => "Maximum",
                    _ => "Capped",
                };
                theme::glow_label(ui, state, 12.5, theme::TEXT_FAINT);
                if changed {
                    self.set_fps_cap(cap);
                }
            },
        );
    }
}
