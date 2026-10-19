//! Independent native Blue Water spectrum controls, shared by promoted pools.
use blue_water_core::{sea_state, WaveParameters};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlueWaterWaves {
    pub height: f32,
    pub direction: f32,
    pub wavelength: f32,
    pub choppiness: f32,
    pub wind_speed: f32,
}
impl Default for BlueWaterWaves {
    fn default() -> Self {
        let p = sea_state::drawn_sea(&WaveParameters::default(), true);
        Self { height: p.significant_height as f32, direction: p.wind_direction.to_degrees() as f32,
            wavelength: p.peak_wavelength as f32, choppiness: p.choppiness as f32,
            wind_speed: p.wind_speed as f32 }
    }
}
impl BlueWaterWaves {
    pub fn sanitize(mut self) -> Self {
        let defaults = Self::default();
        let bounded = |v: f32, fallback: f32, low: f32, high: f32| {
            if v.is_finite() { v.clamp(low, high) } else { fallback }
        };
        self.height = bounded(self.height, defaults.height, 0.0, 60.0);
        self.direction = bounded(self.direction, defaults.direction, -180.0, 180.0);
        self.wavelength = bounded(self.wavelength, defaults.wavelength, 2.0, 512.0);
        self.choppiness = bounded(self.choppiness, defaults.choppiness, 0.0, 4.0);
        self.wind_speed = bounded(self.wind_speed, defaults.wind_speed, 0.1, 40.0);
        self
    }
    pub fn parameters(self) -> WaveParameters {
        let p = self.sanitize();
        // Preserve the current preset's spectrum shape without allowing its
        // automatic sea-state policy to replace the user's wavelength/chop.
        WaveParameters {
            significant_height: f64::from(p.height),
            wind_direction: f64::from(p.direction).to_radians(),
            peak_wavelength: f64::from(p.wavelength),
            choppiness: f64::from(p.choppiness),
            wind_speed: f64::from(p.wind_speed),
            ..sea_state::drawn_sea(&WaveParameters::default(), true)
        }
    }
}
