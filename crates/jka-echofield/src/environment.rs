//! EchoField EnvironmentConditions.java. The host supplies world inputs;
//! no fabricated biome classifications or default weather lookup in this crate.
use std::f64::consts::PI;

#[derive(Debug, Clone, Copy)]
pub struct EnvironmentConditions {
    pub daylight: f32,
    pub dawn_chorus: f32,
    pub dusk_factor: f32,
    pub is_raining: bool,
    pub is_thundering: bool,
    pub temperature: f32,
    pub is_forest: bool,
    pub is_swamp: bool,
    pub is_desert: bool,
    pub wind_exposure: f32,
    pub enclosure: f32,
    pub room_size: f32,
}
#[derive(Debug, Clone, Copy)]
pub struct EnvironmentInputs {
    /// Minecraft-compatible time: 0 dawn, 6000 noon, 12000 dusk, 18000 midnight.
    pub day_time: i64,
    pub is_raining: bool,
    pub is_thundering: bool,
    pub temperature: f32,
    pub is_forest: bool,
    pub is_swamp: bool,
    pub is_desert: bool,
    pub wind_exposure: f32,
    pub enclosure: f32,
    pub room_size: f32,
}
impl From<EnvironmentInputs> for EnvironmentConditions {
    fn from(x: EnvironmentInputs) -> Self {
        let day = x.day_time % 24000;
        Self {
            daylight: daylight(day),
            dawn_chorus: dawn_chorus(day),
            dusk_factor: dusk_factor(day),
            is_raining: x.is_raining,
            is_thundering: x.is_thundering,
            temperature: x.temperature.clamp(0.0, 2.0),
            is_forest: x.is_forest,
            is_swamp: x.is_swamp,
            is_desert: x.is_desert,
            wind_exposure: x.wind_exposure,
            enclosure: x.enclosure,
            room_size: x.room_size,
        }
    }
}
pub fn daylight(day_time: i64) -> f32 {
    (0.5 + 0.5 * (2.0 * PI * (day_time - 6000) as f64 / 24000.0).cos() as f32).clamp(0.0, 1.0)
}
fn pulse(mut dist: f32) -> f32 {
    if dist > 12000.0 {
        dist = 24000.0 - dist;
    }
    if dist < 1000.0 {
        0.5 + 0.5 * (PI * f64::from(dist) / 1000.0).cos() as f32
    } else {
        0.0
    }
}
pub fn dawn_chorus(day_time: i64) -> f32 {
    pulse(day_time.abs() as f32)
}
pub fn dusk_factor(day_time: i64) -> f32 {
    pulse((day_time - 12000).abs() as f32)
}
