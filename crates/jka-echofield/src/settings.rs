use crate::{excitation::Kind, procedural::Category};

/// Live controls for the room solver and the three soundscape layers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub enhancements: bool,
    pub master_volume: f32,
    pub emitters: bool,
    pub emitter_density: f32,
    pub emitter_mask: u32,
    pub excitation_mask: u32,
    pub excitation_volume: f32,
    pub terrain: bool,
    pub terrain_range: u32,
    pub terrain_intensity: f32,
    pub reflection_intensity: f32,
    pub per_source_dr: bool,
    pub diffraction: bool,
    pub doppler: bool,
    pub transmission: bool,
    pub air_absorption: bool,
    /// Zero selects weather-derived relative humidity.
    pub humidity: f32,
    pub resample_moving_sources: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enhancements: true,
            master_volume: 0.5,
            emitters: true,
            emitter_density: 0.75,
            emitter_mask: ((1 << 15) - 1) & !FAUNA_MASK,
            excitation_mask: (1 << Kind::ALL.len()) - 1,
            excitation_volume: 0.5,
            terrain: true,
            terrain_range: 24,
            terrain_intensity: 0.5,
            reflection_intensity: 0.3,
            per_source_dr: false,
            diffraction: true,
            doppler: true,
            transmission: true,
            air_absorption: true,
            humidity: 0.0,
            resample_moving_sources: false,
        }
    }
}

impl Settings {
    pub fn emitter(&self, category: Category) -> bool {
        self.enhancements
            && self.emitters
            && !is_fauna(category)
            && self.emitter_mask & (1 << category as u32) != 0
    }

    pub fn excitation(&self, kind: Kind) -> bool {
        self.enhancements && self.excitation_mask & (1 << kind as u32) != 0
    }
}

/// Birds, insects, frogs and bats are excluded from the DinurdoJK soundscape.
const FAUNA_MASK: u32 = 1 << Category::Bird as u32
    | 1 << Category::Insect as u32
    | 1 << Category::Frog as u32
    | 1 << Category::Bat as u32;

pub(crate) fn is_fauna(category: Category) -> bool {
    matches!(
        category,
        Category::Bird | Category::Insect | Category::Frog | Category::Bat
    )
}
