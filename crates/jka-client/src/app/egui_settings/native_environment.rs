//! Native Blue Water atmosphere, clouds and weather controls.
use super::{App, theme};
/// Double-clicking a slider (or a choice) restores `default`.
fn slider(ui: &mut egui::Ui, label: &str, value: &mut f32, default: f32, low: f32, high: f32) -> bool {
    slider_with(ui, label, value, default, low, high, false)
}
fn slider_with(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    default: f32,
    low: f32,
    high: f32,
    logarithmic: bool,
) -> bool {
    let mut changed = false;
    theme::row(ui, label, label, theme::Reset::None, |ui| {
        let text = format!("{:.2}", *value);
        changed = theme::slider_with(ui, value, low..=high, &text, Some(default), logarithmic);
    });
    changed
}
fn choice(ui: &mut egui::Ui, label: &str, value: &mut u8, default: u8, choices: &[&str]) -> bool {
    let old = *value;
    theme::row(ui, label, label, theme::Reset::None, |ui| {
        let combo = egui::ComboBox::from_id_salt(label)
            .selected_text(choices[(*value as usize).min(choices.len() - 1)])
            .show_ui(ui, |ui| {
                for (i, name) in choices.iter().enumerate() {
                    ui.selectable_value(value, i as u8, *name);
                }
            });
        if combo.response.double_clicked() {
            *value = default;
        }
    });
    old != *value
}
/// A checkbox that restores `default` when double-clicked.
fn toggle(ui: &mut egui::Ui, value: &mut bool, default: bool, label: &str, tip: &str) -> bool {
    let response = ui.checkbox(value, label);
    let response = if tip.is_empty() { response } else { response.on_hover_text(tip) };
    let mut changed = response.changed();
    if response.double_clicked() && *value != default {
        *value = default;
        changed = true;
    }
    changed
}
/// Slider row with a real tooltip; double-click the slider or its label to restore `default`.
fn sky_slider(ui: &mut egui::Ui, label: &str, tip: &str, value: &mut f32, default: f32, low: f32, high: f32) -> bool {
    let mut changed = false;
    theme::row(ui, label, tip, theme::Reset::None, |ui| {
        let text = format!("{:.2}", *value);
        changed = theme::slider_default(ui, value, low..=high, &text, default);
    });
    changed
}
fn sky_switch(ui: &mut egui::Ui, label: &str, tip: &str, value: &mut bool) -> bool {
    let mut changed = false;
    theme::row(ui, label, tip, theme::Reset::None, |ui| {
        if let Some(on) = theme::switch(ui, *value) {
            *value = on;
            changed = true;
        }
    });
    changed
}
/// A slider that stays locked (padlock closed, slider greyed) until unlocked, like
/// the linked ocean controls. `unlocked` is shared by every row using one padlock state.
#[allow(clippy::too_many_arguments)]
fn locked_slider(
    ui: &mut egui::Ui,
    label: &str,
    tip: &str,
    value: &mut f32,
    default: f32,
    low: f32,
    high: f32,
    unlocked: &mut bool,
) -> bool {
    let mut changed = false;
    theme::row(ui, label, tip, theme::Reset::None, |ui| {
        let locked = !*unlocked;
        let mut local = *value;
        let readout = format!("{local:.1}\u{b0}");
        let moved = ui
            .add_enabled_ui(!locked, |ui| theme::slider_default(ui, &mut local, low..=high, &readout, default))
            .inner;
        ui.add_space(8.0);
        if let Some(now_locked) = theme::lock_toggle(ui, locked) {
            *unlocked = !now_locked;
            changed = true;
        } else if moved {
            *value = local;
            changed = true;
        }
    });
    changed
}
impl App {
    pub(super) fn egui_native_clouds(&mut self, ui: &mut egui::Ui) {
        let mut s = self.video.blue_water_environment;
        let d = crate::blue_water_environment::Settings::default();
        let mut changed = false;
        theme::section(
            ui,
            "BLUE WATER CLOUDS",
            "Native cloud controls. Heights use metres above the map's main sea plane.",
        );
        changed |= choice(
            ui,
            "Native quality",
            &mut s.quality,
            d.quality,
            &["Low", "Medium", "High", "Ultra"],
        );
        changed |= choice(
            ui,
            "Native cloud type",
            &mut s.genus,
            d.genus,
            &[
                "Automatic weather",
                "Cumulus",
                "Stratus",
                "Cumulonimbus",
                "Stratocumulus",
                "Nimbostratus",
                "Altocumulus",
                "Altostratus",
            ],
        );
        changed |= slider_with(ui, "Cloud base (m)", &mut s.altitude, d.altitude, -1000.0, 15000.0, true);
        changed |= slider(ui, "Cloud thickness (m)", &mut s.thickness, d.thickness, 50.0, 12000.0);
        changed |= slider(ui, "Horizon coverage", &mut s.horizon, d.horizon, 0.0, 1.0);
        changed |= slider(ui, "Cloud ambient light", &mut s.ambient, d.ambient, 0.0, 3.0);
        changed |= slider(ui, "Cloud base shadow", &mut s.base_shadow, d.base_shadow, 0.0, 1.0);
        changed |= choice(
            ui,
            "High clouds",
            &mut s.high_genus,
            d.high_genus,
            &["Automatic", "Cirrus", "Cirrostratus", "Cirrocumulus"],
        );
        changed |= slider(ui, "High cloud amount", &mut s.high_amount, d.high_amount, 0.0, 1.0);
        changed |= slider(
            ui,
            "High cloud altitude (m)",
            &mut s.high_altitude,
            d.high_altitude,
            2000.0,
            20000.0,
        );
        if changed {
            self.video.blue_water_environment = s.sanitize();
            self.sync_post_effects();
            self.mark_config_dirty();
            self.publish_ui();
        }
    }
    fn commit_sky_environment(&mut self, s: crate::blue_water_environment::Settings) {
        self.video.blue_water_environment = s.sanitize();
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }
    pub(super) fn egui_native_moon(&mut self, ui: &mut egui::Ui) {
        let mut s = self.video.blue_water_environment;
        let d = crate::blue_water_environment::Settings::default();
        let mut changed = false;
        let sun_defaults = super::VideoSettings::default();
        let (sun_yaw, sun_pitch) = if self.video.sun_override {
            (self.video.sun_yaw, self.video.sun_pitch)
        } else {
            self.map_sun_editor_values()
                .map(|(yaw, pitch, _, _)| (yaw, pitch))
                .unwrap_or((sun_defaults.sun_yaw, sun_defaults.sun_pitch))
        };
        theme::section(
            ui,
            "MOON",
            "Blue Water sky only. Phase controls illumination and moves the automatic moon; unlock position to hold it in place.",
        );
        changed |= sky_slider(ui, "Moon phase", "0 and 1 are new moon; 0.5 is full. While position is locked, phase also moves the moon along its automatic sky arc. With manual position unlocked, phase changes illumination only.", &mut s.moon_phase, d.moon_phase, 0.0, 1.0);
        let mut unlocked = s.moon_custom;
        // The native model places an automatic moon on its celestial arc from
        // the active sun angle and phase. Show that derived position while the
        // manual-position lock is closed; unlocking starts from this position.
        let mut celestial = ::blue_water_sky::state::CelestialModel::default();
        celestial.set(
            f64::from(sun_pitch),
            f64::from((sun_yaw + 90.0).rem_euclid(360.0)),
            f64::from(s.moon_phase),
        );
        let (auto_moon_elevation, auto_moon_azimuth) =
            ::blue_water_sky::state::angles_of(celestial.moon);
        if !unlocked {
            s.moon_elevation = auto_moon_elevation as f32;
            s.moon_azimuth = auto_moon_azimuth.rem_euclid(360.0) as f32;
        }
        changed |= locked_slider(ui, "Moon elevation", "Current moon height above the horizon. With the padlock closed this is the computed automatic position; unlock to set it manually.", &mut s.moon_elevation, d.moon_elevation, -90.0, 90.0, &mut unlocked);
        changed |= locked_slider(ui, "Moon azimuth", "Current compass direction of the moon. With the padlock closed this is the computed automatic position; unlock to set it manually.", &mut s.moon_azimuth, d.moon_azimuth, 0.0, 360.0, &mut unlocked);
        s.moon_custom = unlocked;
        if changed {
            self.commit_sky_environment(s);
        }
    }
    pub(super) fn egui_native_atmosphere(&mut self, ui: &mut egui::Ui) {
        let mut s = self.video.blue_water_environment;
        let d = crate::blue_water_environment::Settings::default();
        let mut changed = false;
        theme::section(
            ui,
            "ATMOSPHERE",
            "Blue Water sky only. Uses the sun controls above. Atmosphere changes recompute its lookup tables.",
        );
        changed |= sky_slider(ui, "Rayleigh / blue scattering", "How strongly air scatters blue light. Higher gives a bluer daytime sky and redder sunsets.", &mut s.rayleigh, d.rayleigh, 0.0, 1.5);
        changed |= sky_slider(ui, "Atmospheric turbidity", "Haze and aerosol content. Higher is hazier and whiter.", &mut s.turbidity, d.turbidity, 1.0, 10.0);
        changed |= sky_slider(ui, "Mie / sun aureole", "Strength of the glow around the sun.", &mut s.mie, d.mie, 0.0, 2.0);
        changed |= sky_slider(ui, "Mie directionality", "How tightly the aureole hugs the sun. Higher is a smaller, brighter halo.", &mut s.mie_g, d.mie_g, 0.0, 0.95);
        changed |= sky_slider(ui, "Multiple scattering", "Light scattered more than once; brightens the sky away from the sun.", &mut s.multiple, d.multiple, 0.0, 3.0);
        changed |= sky_slider(ui, "Night sky brightness", "Dims the dome's flat night glow only (1 is upstream). Stars, the Milky Way and the world's night light are unchanged.", &mut s.night_sky, d.night_sky, 0.0, 1.0);
        changed |= sky_switch(ui, "Sky world lighting (approximate)", "Use the native sky's ambient and celestial light on opaque terrain/models. Baked lighting cannot be separated exactly; dark regions are preserved but this is not a replacement for baked indoor/GI lighting.", &mut s.world_lighting);
        changed |= sky_switch(ui, "Native highlight mapping", "Use upstream AgX, sky-referenced eye adaptation and night readability. Limits daylight glare, preserves dusk/night exposure, and gently lifts nearby world shadows at night. Applies while native sky or clouds are selected.", &mut s.filmic);
        if changed {
            self.commit_sky_environment(s);
        }
    }
    pub(super) fn egui_native_weather(&mut self, ui: &mut egui::Ui) {
        let mut s = self.video.blue_water_environment;
        let d = crate::blue_water_environment::Settings::default();
        let mut changed = false;
        theme::section(
            ui,
            "WEATHER RENDERER",
            "Choose one weather renderer. Saved JKA weather settings are retained when switching.",
        );
        changed |= choice(
            ui,
            "Weather renderer",
            &mut s.weather_backend,
            d.weather_backend,
            &["JKA weather", "Blue Water weather", "Off"],
        );
        if s.weather_backend == 1 {
            changed |= slider(ui, "Native rainfall", &mut s.rain, d.rain, 0.0, 1.0);
            changed |= slider(ui, "Rain fall speed (x)", &mut s.rain_speed, d.rain_speed, 0.25, 4.0);
            changed |= slider(
                ui,
                "Lightning strikes / minute",
                &mut s.lightning,
                d.lightning,
                0.0,
                30.0,
            );
            changed |= slider(ui, "Sea fog banks", &mut s.fog, d.fog, 0.0, 1.0);
            changed |= toggle(ui, &mut s.strike_preview, d.strike_preview, "Hold ground lightning preview", "Hold one seeded, branched ground strike 2.6 km in front of your current position. Turn off to resume the natural storm. This diagnostic is not saved.");
            changed |= slider(
                ui,
                "Haze visibility (m)",
                &mut s.visibility,
                d.visibility,
                200.0,
                100000.0,
            );
            ui.label(
                "Set rainfall or lightning to zero to disable each. Fog banks use native clouds.",
            );
        }
        if changed {
            self.video.blue_water_environment = s.sanitize();
            self.sync_post_effects();
            self.mark_config_dirty();
            self.publish_ui();
        }
    }
}
