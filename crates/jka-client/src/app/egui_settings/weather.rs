//! Weather.
use crate::app::egui_settings::{
    index_of, percent, segmented_row, theme, ui, App, PuddleQuality, RainIntensity,
};

impl App {
    pub(in crate::app::egui_settings) fn egui_weather(&mut self, ui: &mut egui::Ui) {
        theme::section(
            ui,
            "WIND",
            "One authoritative atmospheric wind shared by clouds, rain, grass, ocean chop, spray and foam streaks.",
        );
        let wind = self.video.weather_wind;
        theme::row(
            ui,
            "Wind speed",
            "Base atmospheric wind speed in JKA map units per second.",
            theme::Reset::Environment(ui::ENV_ROW_WEATHER_WIND_SPEED),
            |ui| {
                let mut value = wind.speed;
                let readout = format!("{value:.0} u/s");
                if theme::slider(ui, &mut value, 0.0..=8192.0, &readout) {
                    self.set_weather_wind_speed(value);
                }
            },
        );
        theme::row(
            ui,
            "Wind direction",
            "Compass heading the wind blows toward, in degrees.",
            theme::Reset::Environment(ui::ENV_ROW_WEATHER_WIND_DIRECTION),
            |ui| {
                let mut value = wind.direction.rem_euclid(360.0);
                let readout = format!("{value:.0}°");
                if theme::slider(ui, &mut value, 0.0..=360.0, &readout) {
                    self.set_weather_wind_direction(value);
                }
            },
        );
        theme::row(
            ui,
            "Gust strength",
            "Amount of continuous speed variation applied to the shared weather wind.",
            theme::Reset::Environment(ui::ENV_ROW_WEATHER_GUST_STRENGTH),
            |ui| {
                let mut value = wind.gust;
                let readout = percent(value);
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    self.set_weather_gust_strength(value);
                }
            },
        );
        theme::row(
            ui,
            "Direction variation",
            "Maximum continuous angular wandering of the shared weather wind.",
            theme::Reset::Environment(ui::ENV_ROW_WEATHER_DIRECTION_VARIATION),
            |ui| {
                let mut value = wind.shift;
                let readout = format!("{value:.0}°");
                if theme::slider(ui, &mut value, 0.0..=180.0, &readout) {
                    self.set_weather_direction_variation(value);
                }
            },
        );
        theme::row(
            ui,
            "Mapper export",
            "Copy the shared weather wind as map entity keys.",
            theme::Reset::None,
            |ui| {
                if theme::ghost_button(ui, "COPY WIND KEYS").clicked() {
                    let w = self.video.weather_wind;
                    ui.ctx().copy_text(format!(
                        "\"windSpeed\" \"{}\"\n\"windAngle\" \"{}\"\n\"windGust\" \"{}\"\n\"windShift\" \"{}\"\n",
                        w.speed, w.direction, w.gust, w.shift
                    ));
                }
            },
        );

        theme::section(ui, "FOG", "Map fog and volumetric atmospheric scattering.");
        self.egui_fog(ui);

        theme::section(
            ui,
            "PRECIPITATION",
            "Rainfall, wetness and precipitation haze.",
        );
        self.egui_environment_toggle(
            ui,
            "Rain",
            "Simulated rainfall with splashes, wetness on surfaces and haze. \
             Follows the shared weather wind.",
            ui::ENV_ROW_RAIN,
            self.video.rain,
        );
        ui.add_enabled_ui(self.video.rain, |ui| {
            const RAIN: [(RainIntensity, &str); 3] = [
                (RainIntensity::Light, "Light"),
                (RainIntensity::Rain, "Rain"),
                (RainIntensity::Heavy, "Heavy"),
            ];
            if let Some(target) = segmented_row(
                ui,
                "Rain intensity",
                "Particle count, streak length and how much haze the rain adds. \
             Heavy is noticeably more expensive.",
                theme::Reset::Environment(ui::ENV_ROW_RAIN_INTENSITY),
                self.video.rain_intensity,
                &RAIN,
            ) {
                let current = index_of(&RAIN, self.video.rain_intensity, 1);
                let next = index_of(&RAIN, target, current);
                self.environment_selected = ui::ENV_ROW_RAIN_INTENSITY;
                self.change_environment_setting(next as i32 - current as i32);
            }

            const PUDDLE_WATER: [(PuddleQuality, &str); 2] = [
                (PuddleQuality::Standard, "Standard"),
                (PuddleQuality::High, "High"),
            ];
            if let Some(target) = segmented_row(
                ui,
                "Puddle water",
                "How puddles and wet ground are shaded. High uses the ocean's water \
             response (Fresnel, sun and light glints) and streaked reflections \
             of neon and lit surfaces. The cost is only paid on wet pixels.",
                theme::Reset::Environment(ui::ENV_ROW_PUDDLE_WATER),
                self.video.puddle_quality,
                &PUDDLE_WATER,
            ) {
                let current = index_of(&PUDDLE_WATER, self.video.puddle_quality, 1);
                let next = index_of(&PUDDLE_WATER, target, current);
                self.environment_selected = ui::ENV_ROW_PUDDLE_WATER;
                self.change_environment_setting(next as i32 - current as i32);
            }
            theme::row(
                ui,
                "Scattered puddles",
                "How readily rain collects in small puddles on large, level ground \
             such as plazas and roads. Enclosed basins flood regardless.",
                theme::Reset::Environment(ui::ENV_ROW_PUDDLE_SCATTER),
                |ui| {
                    let mut value = self.video.puddle_scatter;
                    let readout = percent(value);
                    if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                        self.set_puddle_scatter(value);
                    }
                },
            );
            theme::row(
                ui,
                "Rain color grade",
                "A wet-weather grade that fades in with the rain: cooler shadows, \
             warmer highlights and richer neon. 0% leaves the picture untouched.",
                theme::Reset::Environment(ui::ENV_ROW_RAIN_GRADE),
                |ui| {
                    let mut value = self.video.rain_grade;
                    let readout = percent(value);
                    if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                        self.set_rain_grade(value);
                    }
                },
            );
        });
    }
}
