//! Renderer world-effect compatibility glue.

use crate::{
    app::App,
    ui::RainIntensity,
    world_effects::{ApplyResult, WeatherType},
};

impl App {
    pub(in crate::app) fn sync_echofield_weather(&mut self) {
        if !self.audio.echofield { return; }
        let weather = self.effective_weather_type();
        let wind = self.world_effects.wind_override().unwrap_or(self.video.weather_wind);
        let temperature = match weather {
            WeatherType::Snow => 0.1,
            WeatherType::Sand => 1.5,
            _ => 0.7,
        };
        if let Some(sound) = self.game_session.as_mut().and_then(|session| session.sound_presenter.as_mut()) {
            sound.set_echofield_weather(
                self.video.relight_time, weather.is_rain(),
                matches!(weather, WeatherType::HeavyRain | WeatherType::HeavyRainFog),
                wind.speed / crate::ocean::OCEAN_UNITS_PER_METER, temperature,
            );
        }
    }
    pub(in crate::app) fn reset_world_effects(&mut self) {
        self.world_effects = crate::world_effects::LegacyWorldEffects::default();
        let _ = self.world_effects.set_weather_type(self.video.weather_type);
        self.sync_post_effects();
    }

    /// Shared equivalent of TaystJK/OpenJK `RE_WorldEffectCommand`. Manual
    /// `/r_we` and game-provided `CS_EFFECTS "*..."` strings both enter here.
    pub(in crate::app) fn apply_world_effect_command(&mut self, command: &str, echo: bool) {
        match self.world_effects.apply(command) {
            Ok(ApplyResult::Applied) => {
                if echo {
                    self.console_status = format!("WORLD EFFECT: {}", command.trim());
                }
            }
            Ok(ApplyResult::Placeholder(kind)) => {
                if echo {
                    self.console_status = format!(
                        "WORLD EFFECT: {} ({kind} placeholder wired)",
                        command.trim()
                    );
                }
            }
            Ok(ApplyResult::Capacity(kind)) => {
                if echo {
                    self.console_status = format!("WORLD EFFECT: maximum {kind} reached");
                }
            }
            Err(error) => {
                if echo {
                    for line in error.lines() {
                        self.push_console_line(format!("^3{line}"));
                    }
                    self.console_status = "WORLD EFFECT COMMAND ERROR".to_owned();
                } else {
                    devprintln!(1, "WORLD EFFECT command ignored: {error}");
                }
                return;
            }
        }
        self.sync_post_effects();
    }

    /// World-effect state drives on-screen weather; the archived preference is
    /// restored on startup and when leaving a server.
    pub(in crate::app) fn effective_weather_type(&self) -> WeatherType {
        self.world_effects.weather_type()
    }

    /// Select one user-facing weather type. This uses the same world-effect state
    /// as `r_we`, but replaces only particle weather so changing precipitation in
    /// the menu does not accidentally erase an authored/shared wind zone.
    pub(in crate::app) fn set_weather_type(&mut self, weather: WeatherType) {
        if let Err(error) = self.world_effects.set_weather_type(weather) {
            self.push_console_line(format!("^3r_we: {error}"));
            return;
        }

        self.video.weather_type = weather;
        self.video.rain = weather.is_rain();
        self.video.rain_intensity = match weather {
            WeatherType::LightRain => RainIntensity::Light,
            WeatherType::HeavyRain => RainIntensity::Heavy,
            WeatherType::Rain | WeatherType::AcidRain => RainIntensity::Rain,
            _ => self.video.rain_intensity,
        };
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    /// Called after CGame has executed reliable commands for a frame. This is
    /// the Rust equivalent of CG_ParseWeatherEffect -> trap->R_WorldEffectCommand.
    pub(in crate::app) fn process_server_world_effects(&mut self) {
        let commands = match self.game_session.as_mut() {
            Some(session) => session.client_game.drain_world_effect_commands(),
            None => return,
        };
        if commands.is_empty() {
            return;
        }
        for command in commands {
            self.apply_world_effect_command(&command, false);
        }
    }
}
