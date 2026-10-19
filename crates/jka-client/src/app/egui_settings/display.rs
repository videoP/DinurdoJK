//! Display.
use crate::app::egui_settings::{
    config, index_of, pending_video_reset, preview_quality_preset_row, segmented_row, theme, ui,
    App, FullscreenMode, QualityPreset, RendererBackend, VideoSection, VsyncMode,
};

#[derive(Clone, Copy)]
struct PendingPresetDialog {
    preset: QualityPreset,
    section: VideoSection,
}

fn pending_preset_dialog_id() -> egui::Id {
    egui::Id::new("jka_quality_preset_scope_dialog")
}

fn video_section_label(section: VideoSection) -> &'static str {
    match section {
        VideoSection::Display => "Display & graphics",
        VideoSection::Geometry => "Geometry",
        VideoSection::Lighting => "Lighting",
        VideoSection::Effects => "Effects",
        VideoSection::Shadows => "Shadows",
        VideoSection::Color => "Color & HDR",
        VideoSection::PostProcessing => "Post processing",
        VideoSection::DebugTools => "Debug & tools",
        VideoSection::Physics => "Physics",
        VideoSection::Sun => "Sky",
        VideoSection::BlueWater => "Ocean",
        VideoSection::Clouds => "Clouds",
        VideoSection::Weather => "Weather",
        VideoSection::Surface => "Surface interaction",
        VideoSection::Water => "Ocean",
        VideoSection::Glass => "Glass",
        VideoSection::Optimizations => "Optimizations",
    }
}

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
            VideoSection::Display => (
                "DISPLAY & GRAPHICS",
                "Window mode, frame pacing, anti-aliasing and texture quality.",
            ),
            VideoSection::Geometry => (
                "GEOMETRY",
                "World visibility, curved surfaces and model rendering.",
            ),
            VideoSection::Lighting => ("LIGHTING", "Ambient, dynamic and indirect lighting."),
            VideoSection::Effects => (
                "EFFECTS",
                "Particles, saber effects, flares and reflections.",
            ),
            VideoSection::Shadows => ("SHADOWS", "Dynamic and local shadowing."),
            VideoSection::Color => (
                "COLOR & HDR",
                "HDR precision, grading, split toning and output brightness.",
            ),
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
                "SKY",
                "Sky renderer, sun, baked lighting, moon and atmosphere.",
            ),
            VideoSection::BlueWater => (
                "OCEAN",
                "Choose Godot, native Blue Water or Hybrid in the single Ocean page.",
            ),
            VideoSection::Clouds => ("CLOUDS", "Cloud renderer and shaping controls."),
            VideoSection::Weather => (
                "WEATHER",
                "Shared world wind, fog, precipitation and atmospheric effects.",
            ),
            VideoSection::Surface => (
                "SURFACE INTERACTION",
                "Footprints and procedural ground detail.",
            ),
            VideoSection::Water => ("OCEAN", "Stock JKA, Godot, Blue Water or Hybrid promoted water."),
            VideoSection::Glass => ("GLASS", "Glass promotion, transmission, frosting and refractive edges."),
            VideoSection::Optimizations => (
                "OPTIMIZATIONS",
                "Speed-ups and A/B switches. No quality preset changes these.",
            ),
        };
        // The preset toolbar is global to Video, not part of any individual
        // page title or scrollable setting group.
        egui::Frame::new()
            .fill(theme::RAIL_FILL)
            .stroke(egui::Stroke::new(1.0, theme::LINE))
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| self.egui_preset_header(ui));
        ui.add_space(10.0);
        theme::page_title(ui, title, detail);

        egui::ScrollArea::vertical()
            .id_salt(("jka_video_section", title))
            .auto_shrink([false, false])
            .show(ui, |ui| match self.video_section {
                VideoSection::Display => {
                    self.egui_display_settings(ui);
                    self.egui_image_quality(ui);
                }
                VideoSection::Geometry => {
                    self.egui_visibility(ui);
                    self.egui_patches(ui);
                    self.egui_models(ui);
                }
                VideoSection::Lighting => self.egui_lighting(ui),
                VideoSection::Effects => {
                    self.egui_effects(ui);
                    self.egui_reflections(ui);
                }
                VideoSection::Shadows => self.egui_shadows(ui),
                VideoSection::PostProcessing => self.egui_post_processing(ui),
                VideoSection::Color => self.egui_color_settings(ui),
                VideoSection::DebugTools => self.egui_debug_tools(ui),
                VideoSection::Physics => self.egui_physics(ui),
                VideoSection::Sun => self.egui_sun(ui),
                VideoSection::BlueWater => self.egui_water(ui),
                VideoSection::Clouds => self.egui_clouds(ui),
                VideoSection::Weather => self.egui_weather(ui),
                VideoSection::Surface => self.egui_surface(ui),
                VideoSection::Water => self.egui_water(ui),
                VideoSection::Glass => self.egui_glass(ui),
                VideoSection::Optimizations => self.egui_optimizations(ui),
            });

        // Drained after the page is built: a label click posts its tag while
        // the rows are being laid out, and applying it there would mutate the
        // settings mid-frame.
        if let Some(reset) = theme::take_reset(ui.ctx()) {
            self.apply_setting_reset(reset);
        }
        theme::clear_pending_resets(ui.ctx());
        theme::clear_preset_preview(ui.ctx());
        // Foreground overlay: never consumes vertical space in the settings
        // column and can be dismissed without changing any cvars.
        self.egui_preset_scope_popup(ui);
    }

    fn egui_preset_scope_popup(&mut self, ui: &mut egui::Ui) {
        let Some(pending) = ui.ctx().data(|data| data.get_temp::<PendingPresetDialog>(pending_preset_dialog_id())) else {
            return;
        };
        #[derive(Clone, Copy)]
        enum Choice { Page, All, Cancel }
        let mut choice = None;
        let has_page_settings = pending.preset.settings().iter().any(|(name, _)| {
            super::preset_preview::preset_cvar_section(name) == Some(pending.section)
        });
        egui::Area::new(egui::Id::new("quality_preset_scope_popup"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(theme::PANEL_FILL)
                    .stroke(egui::Stroke::new(1.0, theme::LINE_STRONG))
                    .inner_margin(egui::Margin::same(22))
                    .show(ui, |ui| {
                        ui.set_width(390.0);
                        theme::page_title(ui, "APPLY QUALITY PRESET", pending.preset.label());
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new(format!(
                            "Current page: {}", video_section_label(pending.section)
                        )).color(theme::TEXT_DIM));
                        ui.add_space(12.0);
                        ui.add_enabled_ui(has_page_settings, |ui| {
                            if theme::primary_button(ui, "APPLY TO THIS PAGE").clicked() {
                                choice = Some(Choice::Page);
                            }
                        });
                        if !has_page_settings {
                            ui.label(egui::RichText::new(
                                "This preset has no settings on this page."
                            ).small().color(theme::TEXT_FAINT));
                        }
                        ui.add_space(8.0);
                        if theme::primary_button(ui, "APPLY TO ALL PAGES").clicked() {
                            choice = Some(Choice::All);
                        }
                        ui.add_space(12.0);
                        if theme::ghost_button(ui, "CANCEL").clicked() {
                            choice = Some(Choice::Cancel);
                        }
                    });
            });
        if choice.is_none() && ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            choice = Some(Choice::Cancel);
        }
        if let Some(choice) = choice {
            ui.ctx().data_mut(|data| data.remove::<PendingPresetDialog>(pending_preset_dialog_id()));
            match choice {
                Choice::Page => self.apply_quality_preset_to_page(pending.preset, pending.section),
                Choice::All => self.apply_quality_preset(pending.preset),
                Choice::Cancel => {}
            }
        }
    }

    fn egui_preset_header(&mut self, ui: &mut egui::Ui) {
        const PRESETS: [(QualityPreset, &str); 6] = [
            (QualityPreset::Minimal, "Minimal (Legacy)"),
            (QualityPreset::MinimalUnified, "Minimal (Unified)"),
            (QualityPreset::Low, "Low"),
            (QualityPreset::Medium, "Medium"),
            (QualityPreset::High, "High"),
            (QualityPreset::Ultra, "Ultra"),
        ];
        let (selected, hovered) = preview_quality_preset_row(ui, self.active_quality_preset(), &PRESETS);
        if let Some(index) = selected {
            // A preset click selects a scope, it does not apply any settings.
            ui.ctx().data_mut(|data| data.insert_temp(
                pending_preset_dialog_id(),
                PendingPresetDialog { preset: PRESETS[index].0, section: self.video_section },
            ));
        }
        // Keep preview visible after opening the dialog even though the pointer
        // has left the bar. On Cancel it disappears without touching settings.
        let pending = ui.ctx().data(|data| data.get_temp::<PendingPresetDialog>(pending_preset_dialog_id()));
        let hovered_preset = pending.map(|dialog| dialog.preset).or_else(|| hovered.map(|index| PRESETS[index].0));
        if let Some(preset) = hovered_preset {
            theme::set_preset_preview(ui.ctx(), self.preset_row_preview(preset));
        } else {
            theme::set_preset_preview(ui.ctx(), Vec::new());
        }
    }

    // ------------------------------------------------------------ rendering --

    pub(in crate::app::egui_settings) fn egui_display_settings(&mut self, ui: &mut egui::Ui) {
        theme::section(
            ui,
            "DISPLAY & FRAME PACING",
            "Window mode, timing, output and presentation.",
        );
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
        theme::row(
            ui,
            "Graphics processor",
            "Select which WGPU adapter runs Unified. Automatic prefers the system's high-performance GPU. Dedicated or Integrated selects a matching compatible adapter. Software requires a CPU adapter exposed by the chosen graphics backend. Switching devices requires Apply; it does not move individual draws onto the CPU.",
            theme::Reset::Video(ui::VIDEO_ROW_GPU_ADAPTER),
            |ui| {
                egui::ComboBox::from_id_salt("video_gpu_adapter")
                    .selected_text(crate::renderer::gpu_adapter::label(self.video.gpu_adapter))
                    .width(190.0)
                    .show_ui(ui, |ui| {
                        for (index, label) in crate::renderer::gpu_adapter::LABELS.iter().enumerate() {
                            if ui.selectable_label(self.video.gpu_adapter == index as u8, *label).clicked() {
                                self.video.gpu_adapter = index as u8;
                                self.video_selected = ui::VIDEO_ROW_GPU_ADAPTER;
                                self.mark_config_dirty();
                            }
                        }
                    });
            },
        );
        ui.label(format!("Currently rendering on: {} ({})", self.active_gpu_name, self.active_gpu_type));
        if self.video.gpu_adapter != self.applied_gpu_adapter {
            theme::hint(ui, "Graphics processor changed: needs Apply", theme::WARNING);
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
