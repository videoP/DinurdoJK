//! Sky appearance and celestial positions. Map illumination lives in Relighting.
use crate::app::egui_settings::{
    color_row, segmented_row, theme, ui, App, VideoSection, VideoSettings,
};

/// Float settings count as changed once they differ by more than a slider's
/// display precision.
fn differs(a: f32, b: f32) -> bool {
    (a - b).abs() > 1e-4
}

impl App {
    /// How many settings on the Sky page differ from their defaults. The custom
    /// sun axes count whenever custom sky sun or world relighting is active.
    pub(in crate::app) fn sun_page_changes(&self) -> usize {
        let v = &self.video;
        let d = VideoSettings::default();
        let s = &v.blue_water_environment;
        let e = crate::blue_water_environment::Settings::default();
        let mut flags = vec![
            v.sky_backend != d.sky_backend,
            v.blue_sky_cube_ab != d.blue_sky_cube_ab,
            v.environment_mode != d.environment_mode,
            v.sky_sun_custom != d.sky_sun_custom,
            v.sun_time_locks != d.sun_time_locks,
            differs(v.relight_time, d.relight_time),
            v.relight_match_system_time != d.relight_match_system_time,
            v.fast_sky != d.fast_sky,
            v.sky_fractal_override != d.sky_fractal_override,
            v.sun_visibility != d.sun_visibility,
            s.moon_custom != e.moon_custom,
            s.quality != e.quality,
            differs(s.moon_phase, e.moon_phase),
            s.moon_intensity_custom != e.moon_intensity_custom,
            s.moon_intensity_custom && differs(s.moon_intensity, e.moon_intensity),
            differs(s.moon_elevation, e.moon_elevation),
            differs(s.moon_azimuth, e.moon_azimuth),
            differs(s.rayleigh, e.rayleigh),
            differs(s.turbidity, e.turbidity),
            differs(s.mie, e.mie),
            differs(s.mie_g, e.mie_g),
            differs(s.multiple, e.multiple),
            differs(s.night_sky, e.night_sky),
            differs(s.sun_size, e.sun_size),
            differs(s.moon_size, e.moon_size),
        ];
        if v.sun_override || v.sky_sun_custom {
            flags.extend([
                differs(v.sun_yaw, d.sun_yaw),
                differs(v.sun_pitch, d.sun_pitch),
                differs(v.sun_intensity, d.sun_intensity),
                v.sun_color != d.sun_color,
            ]);
        }
        flags.into_iter().filter(|changed| *changed).count()
    }

    /// Restores every setting the Sky page shows. The Blue Water environment
    /// struct also holds clouds and weather, so only the sky fields are copied back.
    pub(in crate::app) fn reset_sun_page(&mut self) {
        let d = VideoSettings::default();
        let e = crate::blue_water_environment::Settings::default();
        let authored = self.map_sun_editor_values();
        let v = &mut self.video;
        v.sky_backend = d.sky_backend;
        v.environment_mode = d.environment_mode;
        v.relighting_mode = if v.sun_override { 1 } else { 0 };
        v.relight_time = d.relight_time;
        v.relight_match_system_time = d.relight_match_system_time;
        v.sky_sun_custom = d.sky_sun_custom;
        v.sun_time_locks = d.sun_time_locks;
        let reset_sky_cube = v.blue_sky_cube_ab != d.blue_sky_cube_ab;
        // r_fastsky also needs its dedicated render-thread command.
        let reset_fastsky = v.fast_sky != d.fast_sky;
        v.sky_fractal_override = d.sky_fractal_override;
        let (yaw, pitch, intensity, color) = authored.unwrap_or((d.sun_yaw, d.sun_pitch, d.sun_intensity, d.sun_color));
        v.sun_yaw = yaw;
        v.sun_pitch = pitch;
        v.sun_intensity = intensity;
        v.sun_color = color;
        v.sun_visibility = d.sun_visibility;
        let s = &mut v.blue_water_environment;
        s.quality = e.quality;
        s.moon_phase = e.moon_phase;
        s.moon_intensity = e.moon_intensity;
        s.moon_intensity_custom = e.moon_intensity_custom;
        s.moon_custom = e.moon_custom;
        s.moon_elevation = e.moon_elevation;
        s.moon_azimuth = e.moon_azimuth;
        s.rayleigh = e.rayleigh;
        s.turbidity = e.turbidity;
        s.mie = e.mie;
        s.mie_g = e.mie_g;
        s.multiple = e.multiple;
        s.night_sky = e.night_sky;
        s.sun_size = e.sun_size;
        s.moon_size = e.moon_size;
        // Reset appearance without changing the Relight map master switch.
        if reset_fastsky { let _ = self.set_console_cvar("r_fastsky", "0"); }
        // Unlike the other sky settings, this A/B has a dedicated render-thread
        // command. Reset through its cvar so the renderer updates immediately.
        if reset_sky_cube { let _ = self.set_console_cvar("r_blueSkyCubeAB", "1"); }
        self.relight_clock_effective = None;
        self.render_command(crate::renderer::RenderCommand::SetBlueSkyCubeAB(d.blue_sky_cube_ab));
        self.sun_custom_initialized = self.video.sun_override;
        self.baked_sun_debug_view = 0;
        self.sun_ray_until = None;
        self.sync_sun_ray();
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    pub(in crate::app::egui_settings) fn egui_sun(&mut self, ui: &mut egui::Ui) {
        // A held slider that isn't moving produces no change events, so keep its
        // direction/diagnostic preview alive for as long as the drag continues.
        if self.sun_ray_until.is_some() && ui.ctx().dragged_id().is_some() {
            match self.environment_selected {
                ui::ENV_ROW_SUN_YAW | ui::ENV_ROW_SUN_PITCH => self.touch_sun_ray(),

                _ => {}
            }
        }
        let defaults = VideoSettings::default();
        let changed = self.sun_page_changes();
        theme::row(
            ui,
            "Sky settings",
            "These settings are saved normally unless a relighting profile is active. Use Looks & profiles to save named looks or choose a map default. Reset all returns this whole page to its shipped values; double-click a slider or label to reset one setting.",
            theme::Reset::None,
            |ui| {
                ui.add_enabled_ui(changed != 0, |ui| {
                    if theme::ghost_button(ui, "RESET ALL SKY SETTINGS").clicked() {
                        self.reset_sun_page();
                    }
                });
                ui.add_space(10.0);
                let (text, color) = if changed == 0 {
                    ("All at defaults".to_owned(), theme::TEXT_FAINT())
                } else {
                    (format!("{changed} changed from default"), theme::WARNING())
                };
                theme::glow_label(ui, &text, 12.5, color);
            },
        );

        // ----------------------------------------------------- rendering --
        theme::section(ui, "ENVIRONMENT RENDERING", "One shared quality/update mode for the visible sky and world relighting (if enabled).");
        if let Some(mode) = segmented_row(ui, "Rendering mode",
            "Baked caches clear Blue Water sky faces and cached world illumination; editing time automatically refreshes affected caches. Realtime evaluates sky and enabled relighting live. Baked system time is sampled once per map load.",
            theme::Reset::None, self.video.environment_mode,
            &[(0u8, "Baked"), (1u8, "Realtime")]) {
            self.set_environment_mode(mode);
        }
        theme::hint(ui, if self.video.sun_override {
            "World relighting is enabled: it follows this shared environment mode."
        } else {
            "World relighting is disabled: this mode still controls the sky. Authored map illumination remains unchanged."
        }, theme::TEXT_DIM());

        // ------------------------------------------------------------ sky --
        theme::section(ui, "SKY", "Which sky is drawn behind the map.");
        let sky_mode = if self.video.fast_sky != 0 {
            0 // r_fastsky wins over authored/Blue Water rendering
        } else if self.video.sky_fractal_override {
            3
        } else if self.video.sky_backend == 1 {
            2
        } else {
            1
        };
        theme::row(ui, "Sky mode", "Fastsky uses r_fastsky 1. Vanilla draws authored BSP sky stages. Blue Water adds its atmosphere; Star Nest replaces BSP sky faces with the fractal renderer. These modes are exclusive in the UI; clouds are chosen separately.", theme::Reset::None, |ui| {
            for (mode, name) in [(0, "Fastsky"), (1, "Vanilla"), (2, "Blue Water"), (3, "Star Nest")] {
                if theme::chip(ui, name, sky_mode == mode).clicked() && (sky_mode != mode || (mode == 0 && self.video.fast_sky != 1)) {
                    // The fastsky cvar also sends a specialized render-thread command.
                    let fast = if mode == 0 { "1" } else { "0" };
                    let _ = self.set_console_cvar("r_fastsky", fast);
                    self.video.sky_backend = u8::from(mode == 2);
                    self.video.sky_fractal_override = mode == 3;
                    self.sync_post_effects();
                    self.mark_config_dirty();
                    self.publish_ui();
                }
                ui.add_space(3.0);
            }
        });
        if self.video.sky_backend == 1 && self.video.fast_sky == 0 {
            theme::row(ui, "Sky quality", "Blue Water sky lookup tables, environment bake, and native weather budget. Cloud quality is independent on the Clouds page.", theme::Reset::None, |ui| {
                for (i, name) in ["Low", "Medium", "High", "Ultra"].iter().enumerate() {
                    if theme::chip(ui, name, self.video.blue_water_environment.quality == i as u8).clicked() {
                        if self.video.blue_water_environment.quality != i as u8 {
                            self.video.blue_water_environment.quality = i as u8;
                            self.sync_post_effects();
                            self.mark_config_dirty();
                            self.publish_ui();
                        }
                    }
                }
            });
        }

        // ------------------------------------------------------------ time --
        theme::section(ui, "TIME OF DAY", "Artistic 24-hour cycle; sunrise 6 AM, noon 12 PM, sunset 6 PM. Sun and moon positions update together; lunar phase and original map lighting stay independently adjustable.");
        theme::row(ui, "Time", "3 AM to the following 3 AM. Match system time uses the PC's local clock; Realtime follows it, Baked captures it at map load. Manual slider and shadow/sky selection are otherwise unchanged.", theme::Reset::None, |ui| {
            let matching = self.video.relight_match_system_time;
            // The disabled slider is a clock readout, not the archived
            // manual position. Baked mode may still be using a sample
            // captured at map load; display the current local time without
            // changing that sample or triggering another bake.
            let mut time = if matching {
                use chrono::Timelike;
                let now = chrono::Local::now();
                crate::app::settings::graphics::system_clock_relighting_hour(now.hour(), now.minute())
            } else {
                self.video.relight_time
            };
            let minutes = ((time.rem_euclid(24.0) * 60.0).round() as u32) % (24 * 60);
            let hours = minutes / 60;
            let hh = hours % 12;
            let readout = format!("{}:{:02} {}", if hh == 0 { 12 } else { hh }, minutes % 60, if hours < 12 { "AM" } else { "PM" });
            ui.horizontal(|ui| {
                // Reserve room for the adjacent toggle: the theme's slider
                // uses its available width to size the Taffy track.
                let slider_width = (ui.available_width() - 150.0).max(210.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(slider_width, 24.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add_enabled_ui(!matching, |ui| {
                            if theme::slider_default(ui, &mut time, 3.0..=27.0, &readout, 15.0) {
                                self.set_relighting_time((time * 4.0).round() * 0.25);
                            }
                        });
                    },
                );
                if theme::chip(ui, "MATCH SYSTEM TIME", matching).clicked() {
                    self.set_relighting_match_system_time(!matching);
                }
            });
        });
        if self.video.relight_match_system_time && self.video.environment_mode == 0 {
            let hint = if self.relight_clock_effective.is_some() {
                "Baked: map lighting uses the time captured at map load. The disabled slider shows the current PC clock, which may have moved on."
            } else {
                "Baked: the system clock will first be applied at the next map load. The disabled slider shows the current PC clock."
            };
            theme::hint(ui, hint, theme::TEXT_DIM());
        }
        if self.video.sky_backend != 1 || self.video.fast_sky != 0 {
            theme::hint(ui, "Blue Water Sky gives the clock a matching sunrise, sunset and night sky; other sky modes retain their own appearance.", theme::TEXT_DIM());
        }
        // ------------------------------------------------------------ sun --
        theme::section(ui, "SUN", "Yaw, pitch and intensity follow the time of day when locked. Unlock any axis for a persistent absolute override, independent of world relighting.");
        let authored = self.map_sun_editor_values();
        if !self.video.sky_sun_custom && !self.video.sun_override {
            theme::hint(ui, "Showing the map's authored sun. Adjust time or a slider to enable a custom visual sun without relighting the world.", theme::TEXT_DIM());
        }
        if theme::ghost_button(ui, "RESTORE MAP SUN").clicked() {
            let (yaw, pitch, intensity, color) = authored.unwrap_or((
                defaults.sun_yaw, defaults.sun_pitch, defaults.sun_intensity, defaults.sun_color,
            ));
            self.video.sun_yaw = yaw;
            self.video.sun_pitch = pitch;
            self.video.sun_intensity = intensity;
            self.video.sun_color = color;
            self.video.sun_time_locks = 7;
            self.video.sky_sun_custom = false;
            self.sync_post_effects();
            self.mark_config_dirty();
            self.publish_ui();
        }
        let (auto_yaw, auto_pitch, auto_intensity) = self.automatic_relighting_sun(
            self.relight_clock_effective.unwrap_or(self.video.relight_time)
        );
        const AXES: [(u8, &str, &str, f32, f32); 3] = [
            (VideoSettings::SUN_LOCK_YAW, "Yaw / azimuth", "q3map sun azimuth: 0° east, 90° north.", 0.0, 360.0),
            (VideoSettings::SUN_LOCK_PITCH, "Pitch / elevation", "Sun elevation, -90° to +90°. Negative values are below the horizon.", -90.0, 90.0),
            (VideoSettings::SUN_LOCK_INTENSITY, "Intensity", "Sun strength. Locked follows daylight, fading to zero after sunset; unlocked holds a fixed value.", 0.0, ui::SUN_INTENSITY_MAX),
        ];
        for (bit, label, tip, lo, hi) in AXES {
            let locked = self.video.sun_time_locks & bit != 0;
            let (current, automatic, default) = match bit {
                VideoSettings::SUN_LOCK_YAW => (self.video.sun_yaw, auto_yaw, authored.map_or(defaults.sun_yaw, |s| s.0)),
                VideoSettings::SUN_LOCK_PITCH => (self.video.sun_pitch, auto_pitch, authored.map_or(defaults.sun_pitch, |s| s.1)),
                _ => (self.video.sun_intensity, auto_intensity, authored.map_or(defaults.sun_intensity, |s| s.2)),
            };
            theme::row(ui, label, tip, theme::Reset::None, |ui| {
                let mut shown = current;
                let readout = if bit == VideoSettings::SUN_LOCK_INTENSITY {
                    format!("{shown:.0}{}", if locked { " (Auto)" } else { "" })
                } else {
                    format!("{shown:.1}°{}", if locked { " (Auto)" } else { "" })
                };
                let moved = theme::slider_default(ui, &mut shown, lo..=hi, &readout, default);
                ui.add_space(8.0);
                let toggle = theme::lock_toggle(ui, locked);
                if moved {
                    self.environment_selected = match bit {
                        VideoSettings::SUN_LOCK_YAW => ui::ENV_ROW_SUN_YAW,
                        VideoSettings::SUN_LOCK_PITCH => ui::ENV_ROW_SUN_PITCH,
                        _ => ui::ENV_ROW_SUN_INTENSITY,
                    };
                    match bit {
                        VideoSettings::SUN_LOCK_YAW => self.set_sun_yaw(shown),
                        VideoSettings::SUN_LOCK_PITCH => self.set_sun_pitch(shown),
                        _ => self.set_sun_intensity(shown),
                    }
                } else if let Some(now_locked) = toggle {
                    self.set_sun_time_lock(bit, now_locked);
                }
                if !locked {
                    ui.label(egui::RichText::new(format!("Auto {:.0}", automatic)).small().color(theme::TEXT_FAINT()));
                }
            });
        }
        let mut color = self.video.sun_color;
        let mut color_changed = false;
        color_row(ui, "Sun color", "Sun chromaticity, normalized like q3map_sun. This affects the visual sun even without relighting.",
            theme::Reset::Environment(ui::ENV_ROW_SUN_COLOR), &mut color, &mut color_changed);
        if color_changed {
            self.environment_selected = ui::ENV_ROW_SUN_COLOR;
            self.set_sun_color(color);
        }
        ui.horizontal_wrapped(|ui| {
            if theme::ghost_button(ui, "OPEN RELIGHTING").clicked() {
                self.video_section = VideoSection::Relighting;
            }
            if theme::ghost_button(ui, "SAVE / LOAD LOOK").clicked() {
                self.video_section = VideoSection::EnvironmentLooks;
            }
        });

        // ------------------------------------------- moon and atmosphere --
        if self.video.sky_backend == 1 {
            self.egui_native_moon(ui);
            self.egui_native_atmosphere(ui);
        }
    }
}
