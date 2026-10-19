//! Weather.
use crate::{
    app::egui_settings::{
        index_of, percent, quality_table_row, segmented_row, theme, ui, App, FootprintMode,
        PuddleQuality,
    },
    world_effects::WeatherType,
};

impl App {
    pub(in crate::app::egui_settings) fn egui_weather(&mut self, ui: &mut egui::Ui) {
        self.egui_native_weather(ui);
        theme::section(
            ui,
            "WIND",
            "One authoritative atmospheric wind shared by clouds, rain, grass, ocean chop, spray and foam streaks.",
        );
        let wind = self.video.weather_wind;
        theme::row(
            ui,
            "Wind speed",
            "Base atmospheric wind speed in knots.",
            theme::Reset::Environment(ui::ENV_ROW_WEATHER_WIND_SPEED),
            |ui| {
                // Stored in map units/s; the ocean scale is 40 units per metre.
                let units_per_knot = 0.514_444 * crate::ocean::OCEAN_UNITS_PER_METER;
                let mut value = (wind.speed / units_per_knot).clamp(0.0, 250.0);
                let readout = format!("{value:.0} kt");
                if theme::slider(ui, &mut value, 0.0..=250.0, &readout) {
                    self.set_weather_wind_speed(value * units_per_knot);
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
            "WEATHER",
            "World weather authored by r_we. Choose one presentation type here; direct r_we/CS_EFFECTS commands still retain OpenJK-style additive compatibility.",
        );

        let current_weather = self.effective_weather_type();
        let mut selected_weather = current_weather;
        theme::row(
            ui,
            "Weather type",
            "Selects the active particle-weather type using the same world-effect state as r_we. Clear really clears rain, snow, sand and other particle weather.",
            theme::Reset::Environment(ui::ENV_ROW_RAIN),
            |ui| {
                egui::ComboBox::from_id_salt("weather_type")
                    .selected_text(current_weather.label())
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for weather in WeatherType::ALL {
                            ui.selectable_value(&mut selected_weather, weather, weather.label());
                        }
                    });
            },
        );
        if selected_weather != current_weather {
            self.environment_selected = ui::ENV_ROW_RAIN;
            self.set_weather_type(selected_weather);
        }

        theme::row(
            ui,
            "Wind visuals",
            "Camera-centred airborne dust specks and sparse gust wisps. Off by default, independent of Weather type and r_we clear (console: r_windVisuals). Driven by the atmospheric wind; sheltered areas are suppressed by the weather heightfield.",
            theme::Reset::Environment(ui::ENV_ROW_WIND_VISUALS),
            |ui| {
                let mut value = self.video.wind_visuals;
                let readout = percent(value);
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    self.set_wind_visuals(value);
                }
            },
        );
        let weather = self.effective_weather_type();
        if weather.is_rain() {
            self.egui_environment_toggle(
                ui,
                "Rain on lens",
                "Droplets and rivulets of water on the camera itself, refracting the finished scene. Fades out under shelter with the rest of the weather. HUD and menus are drawn afterwards and stay undistorted.",
                ui::ENV_ROW_RAIN_GLASS,
                self.video.rain_glass,
            );

            const PUDDLE_WATER: [(PuddleQuality, &str); 2] = [
                (PuddleQuality::Standard, "Standard"),
                (PuddleQuality::High, "High"),
            ];
            if let Some(target) = segmented_row(
                ui,
                "Puddle water",
                "How puddles and wet ground are shaded. High uses the ocean's water response (Fresnel, sun and light glints) and streaked reflections of neon and lit surfaces. The cost is only paid on wet pixels.",
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
                "How readily rain collects in small puddles on large, level ground such as plazas and roads. Enclosed basins flood regardless.",
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
                "A wet-weather grade that fades in with the rain: cooler shadows, warmer highlights and richer neon. 0% leaves the picture untouched.",
                theme::Reset::Environment(ui::ENV_ROW_RAIN_GRADE),
                |ui| {
                    let mut value = self.video.rain_grade;
                    let readout = percent(value);
                    if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                        self.set_rain_grade(value);
                    }
                },
            );
        }

        if weather.uses_footprints() {
            const FOOTPRINTS: [(FootprintMode, &str); 3] = [
                (FootprintMode::Off, "Off"),
                (FootprintMode::TwoD, "2D stamp"),
                (FootprintMode::ThreeD, "3D + 2D stamp"),
            ];
            if let Some(target) = quality_table_row(
                ui,
                "Footprints",
                "Tracks left in snow and sand. The 2D stamp is a decal; 3D also displaces local snow so prints have real depth.",
                theme::Reset::Environment(ui::ENV_ROW_FOOTPRINTS),
                self.video.footprints,
                &FOOTPRINTS,
                2,
            ) {
                self.set_environment_quality_segment(ui::ENV_ROW_FOOTPRINTS, target);
            }
        }

        if matches!(weather, WeatherType::Fog | WeatherType::HeavyRainFog | WeatherType::LightFog) {
            theme::banner(
                ui,
                "This is a legacy r_we particle-cloud mode. Its compatibility state is preserved; use the Fog section above for DinurdoJK's rendered fog controls.",
                theme::TEXT_DIM,
            );
        }

    }
}
