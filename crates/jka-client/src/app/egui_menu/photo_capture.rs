//! In-game capture controls; never change the actual gameplay HUD or camera.
use super::App;
use crate::renderer::{PhotoOptions, PhotoProjection, ScreenshotFormat};

impl App {
    pub(super) fn egui_photo_capture_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Screenshot");
        ui.label("Capture the current viewpoint without menu chrome.");
        ui.separator();
        let mut options = self.video.photo;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut options.panorama, false, "Standard");
            ui.selectable_value(&mut options.panorama, true, "360° Panorama");
        });
        if options.panorama {
            egui::ComboBox::from_id_salt("photo_projection").selected_text(options.projection.label()).show_ui(ui, |ui| {
                for projection in PhotoProjection::ALL {
                    ui.selectable_value(&mut options.projection, projection, projection.label());
                }
            });
            ui.label(if matches!(options.projection, PhotoProjection::Cylindrical) {
                "360° horizontal; 90° vertical (excludes poles)."
            } else if matches!(options.projection, PhotoProjection::Planet | PhotoProjection::Tunnel) {
                "Stereographic artistic view; crops near the opposite pole."
            } else { "Full 360° × 180° spherical coverage." });
        }
        let equi = options.panorama && options.projection == PhotoProjection::Equirectangular;
        if options.panorama && !equi && !PhotoOptions::WIDTHS.contains(&options.panorama_width) { options.panorama_width = 8192; }
        let width = if options.panorama { &mut options.panorama_width } else { &mut options.width };
        egui::ComboBox::from_id_salt("photo_resolution").selected_text(if *width == 0 { "Current resolution".to_owned() } else { format!("{} px wide", *width) }).show_ui(ui, |ui| {
            if !options.panorama { ui.selectable_value(width, 0, "Current resolution"); }
            for &value in if equi { &PhotoOptions::EQUIRECT_WIDTHS[..] } else { &PhotoOptions::WIDTHS[..] } { ui.selectable_value(width, value, format!("{} px", value)); }
        });
        if equi {
            ui.horizontal(|ui| {
                ui.label("Custom width (2:1)");
                if ui.add(egui::DragValue::new(width).range(2048..=131072).speed(2)).changed() { *width = (*width).div_ceil(2)*2; }
            });
        }
        let jpeg_allowed = !equi || options.panorama_width <= PhotoOptions::MAX_JPEG_PANORAMA_WIDTH;
        if !jpeg_allowed { options.format = ScreenshotFormat::Png; }
        ui.horizontal(|ui| {
            ui.label("Format");
            ui.add_enabled_ui(jpeg_allowed, |ui| {
                ui.selectable_value(&mut options.format, ScreenshotFormat::Jpeg, "JPEG");
            }).response.on_disabled_hover_text("JPEG is limited to 65,500 pixels per axis (libjpeg); the largest JPEG panorama is 65,500 × 32,750.");
            ui.selectable_value(&mut options.format, ScreenshotFormat::Png, "PNG");
        });
        if !jpeg_allowed { ui.small("PNG selected: JPEG cannot store this image width."); }
        ui.separator();
        ui.label("Screenshot antialiasing");
        ui.horizontal(|ui| {
            ui.label("MSAA");
            ui.selectable_value(&mut options.max_msaa, false, "Gameplay");
            ui.selectable_value(&mut options.max_msaa, true, "Maximum supported");
        });
        ui.horizontal(|ui| {
            ui.label("Supersampling");
            for factor in [1, 2, 4] {
                ui.selectable_value(&mut options.supersample, factor, format!("{factor}× linear"));
            }
        });
        let display = self.window.as_ref().map(|window| {
            let size = window.inner_size(); [size.width.max(1), size.height.max(1)]
        }).unwrap_or([1,1]);
        let [w,h] = options.output_size(display);
        if options.panorama && options.projection == PhotoProjection::Faces { ui.label(format!("Six {w} × {h} images")); }
        else { ui.label(format!("Output: {w} × {h}")); }
        if let Some([rw,rh]) = options.render_size(display) {
            ui.small(format!("Internal render: {rw} × {rh}{}", if options.panorama { " per face" } else { "" }));
            match options.estimated_plan() {
                Some(Ok(plan)) => {
                    ui.small(format!("Estimated method: {}.", plan.describe()));
                    ui.small("Chosen automatically when the capture starts, from this GPU's limits and free VRAM.");
                }
                Some(Err(error)) => { ui.small(error); }
                None => {}
            }
        }
        ui.small("4× linear supersampling renders 16 subpixels per regular output pixel; high MSAA and large images can exhaust GPU memory.");
        ui.separator();
        let mut excluded = true;
        ui.add_enabled(false, egui::Checkbox::new(&mut excluded, "Exclude menus and console"));
        if options.panorama {
            let mut hidden = true;
            ui.add_enabled(false, egui::Checkbox::new(&mut hidden, "Hide all 2D UI"));
            ui.add_enabled(false, egui::Checkbox::new(&mut hidden, "Hide first-person weapon"));
        } else {
            ui.checkbox(&mut options.hide_ui, "Hide all 2D UI");
            ui.checkbox(&mut options.hide_weapon, "Hide first-person weapon");
        }
        ui.checkbox(&mut options.hide_player, "Hide viewed player model");
        ui.separator();
        ui.checkbox(&mut options.embed_game_metadata, "Embed game metadata")
            .on_hover_text("Save server, player names, scores, pings and match timers in the image's XMP. Map, mod, position and view are always saved so Go To Spot works. Also applies to /screenshot and /screenshot_png.");
        if options != self.video.photo {
            self.video.photo = options;
            self.mark_config_dirty();
        }
        ui.separator();
        let ready = !self.front_end && self.prepared_map_cache.is_some() && self.render.is_some();
        if self.pending_photo_capture.is_some() || self.photo_cancel.is_some() {
            let cancelling = self.photo_cancel.as_ref().is_some_and(|c| c.load(std::sync::atomic::Ordering::Acquire));
            if ui.add_enabled(!cancelling, egui::Button::new(if cancelling { "Cancelling…" } else { "Cancel screenshot" })).clicked() {
                self.cancel_photo_capture();
            }
            if let Some(progress) = &self.photo_progress {
                if progress.total > 0 {
                    let fraction = (progress.completed as f64 / progress.total as f64).clamp(0.0, 1.0) as f32;
                    ui.add(egui::ProgressBar::new(fraction).show_percentage());
                    if let Some(seconds) = progress.eta_seconds {
                        ui.small(format!("Current phase ETA: {}m {:02}s", seconds / 60, seconds % 60));
                    }
                }
                ui.small(&progress.status);
            }
        } else if ui.add_enabled(ready, egui::Button::new("Take screenshot")).clicked() {
            self.request_photo_capture(options);
        }
        ui.label(&self.console_status);
        ui.small(format!("Save folder: {}", self.game_write_path("screenshots").display()));
        ui.small("Large captures allocate independent render targets. Gameplay settings are not changed.");
    }
}
