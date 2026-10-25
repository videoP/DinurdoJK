//! One selection per movement-HUD component. Legacy cvars remain the archive
//! format, but menus, drawing and the editor resolve them through this module.
use super::*;
use crate::speedometer::flag as speed;

pub const NATIVE_READOUTS: u32 = speed::ENABLE | speed::GROUNDSPEED | speed::JUMPHEIGHT
    | speed::JUMPDISTANCE | speed::VERTICALSPEED | speed::YAWSPEED | speed::ACCELMETER
    | speed::SPEEDGRAPH | speed::SPEEDGRAPHOLD | speed::JUMPS;

pub const GUIDE_CHOICES: &[(u32, &str)] = &[
    (0, "Off"), (SHELPER_ORIGINAL, "TaystJK Original"), (SHELPER_UPDATED, "TaystJK Updated"),
    (SHELPER_CGAZ, "TaystJK CGAZ"), (SHELPER_CINEMATIC, "Cinematic (world guide)"),
    (SHELPER_XONOTIC, "Xonotic bands"), (SHELPER_Q3DF, "Q3DF CGaz bands"),
    (SHELPER_WARSOW_HUD, "Warsow triangles"),
];
pub const READOUT_CHOICES: &[(u32, &str)] = &[(0, "Off"), (1, "jaPRO"), (2, "Q3DF")];
pub const NATIVE_CHOICES: &[(u32, &str)] = &[(0, "Off"), (1, "jaPRO")];
pub const COMPASS_CHOICES: &[(u32, &str)] = &[(0, "Off"), (1, "Q3DF")];
pub const GRAPH_CHOICES: &[(u32, &str)] = &[(0, "Off"), (1, "jaPRO graph"), (2, "jaPRO classic graph")];

pub fn native_bit(id: HudElementId) -> u32 {
    match id {
        HudElementId::Speedometer => speed::ENABLE,
        HudElementId::AccelMeter => speed::ACCELMETER,
        HudElementId::GroundSpeed => speed::GROUNDSPEED,
        HudElementId::JumpHeight => speed::JUMPHEIGHT,
        HudElementId::JumpDistance => speed::JUMPDISTANCE,
        HudElementId::VerticalSpeed => speed::VERTICALSPEED,
        HudElementId::YawSpeed => speed::YAWSPEED,
        HudElementId::SpeedometerJumps => speed::JUMPS,
        _ => 0,
    }
}

pub fn selected(id: HudElementId, helper: u32, native: u32) -> u32 {
    match id {
        HudElementId::StrafeGuide => [SHELPER_WARSOW_HUD, SHELPER_Q3DF, SHELPER_XONOTIC,
            SHELPER_CINEMATIC, SHELPER_CGAZ, SHELPER_UPDATED, SHELPER_ORIGINAL]
            .into_iter().find(|bit| helper & bit != 0).unwrap_or(0),
        HudElementId::Speedometer if helper & SHELPER_Q3DF_SPEED != 0 => 2,
        HudElementId::AccelMeter if helper & SHELPER_Q3DF_ACCEL != 0 => 2,
        HudElementId::AccelMeter if helper & SHELPER_ACCELMETER != 0 => 1,
        HudElementId::VelocityCompass => u32::from(helper & SHELPER_Q3DF_COMPASS != 0),
        HudElementId::SpeedGraph => if native & speed::SPEEDGRAPH != 0 { 1 }
            else if native & speed::SPEEDGRAPHOLD != 0 { 2 } else { 0 },
        _ => u32::from(native & native_bit(id) != 0),
    }
}

pub fn select(id: HudElementId, choice: u32, helper: &mut u32, native: &mut u32) {
    match id {
        HudElementId::StrafeGuide => *helper = (*helper & !SHELPER_STYLE_MASK) | (choice & SHELPER_STYLE_MASK),
        HudElementId::Speedometer => {
            *helper &= !SHELPER_Q3DF_SPEED;
            *native &= !speed::ENABLE;
            if choice == 1 { *native |= speed::ENABLE; }
            if choice == 2 { *helper |= SHELPER_Q3DF_SPEED; }
        }
        HudElementId::AccelMeter => {
            *helper &= !(SHELPER_ACCELMETER | SHELPER_Q3DF_ACCEL);
            *native &= !speed::ACCELMETER;
            if choice == 1 { *native |= speed::ACCELMETER; }
            if choice == 2 { *helper |= SHELPER_Q3DF_ACCEL; }
        }
        HudElementId::VelocityCompass => {
            *helper &= !SHELPER_Q3DF_COMPASS;
            if choice != 0 { *helper |= SHELPER_Q3DF_COMPASS; }
        }
        HudElementId::SpeedGraph => {
            *native &= !(speed::SPEEDGRAPH | speed::SPEEDGRAPHOLD);
            if choice == 1 { *native |= speed::SPEEDGRAPH; }
            if choice == 2 { *native |= speed::SPEEDGRAPHOLD; }
        }
        _ => {
            let bit = native_bit(id);
            *native &= !bit;
            if choice != 0 { *native |= bit; }
        }
    }
}

/// Deterministic migration for old configs with overlapping providers.
pub fn normalize(helper: &mut u32, native: &mut u32) {
    let guide = selected(HudElementId::StrafeGuide, *helper, *native);
    select(HudElementId::StrafeGuide, guide, helper, native);
    for id in [HudElementId::Speedometer, HudElementId::AccelMeter, HudElementId::SpeedGraph] {
        let choice = selected(id, *helper, *native);
        select(id, choice, helper, native);
    }
}

pub fn is_movement_component(id: HudElementId) -> bool {
    matches!(id, HudElementId::StrafeGuide | HudElementId::VelocityCompass | HudElementId::SpeedGraph)
        || native_bit(id) != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn component_choices_are_exclusive_and_independent() {
        let mut helper = SHELPER_Q3DF | SHELPER_Q3DF_SPEED | SHELPER_Q3DF_ACCEL | SHELPER_ACCELMETER;
        let mut native = speed::ENABLE | speed::ACCELMETER | speed::GROUNDSPEED;
        normalize(&mut helper, &mut native);
        assert_eq!(native & (speed::ENABLE | speed::ACCELMETER), 0);
        select(HudElementId::AccelMeter, 1, &mut helper, &mut native);
        assert_eq!(selected(HudElementId::Speedometer, helper, native), 2);
        assert_eq!(helper & (SHELPER_Q3DF_ACCEL | SHELPER_ACCELMETER), 0);
        assert_ne!(native & speed::ACCELMETER, 0);
        select(HudElementId::Speedometer, 0, &mut helper, &mut native);
        assert_eq!(selected(HudElementId::Speedometer, helper, native), 0);
        assert_eq!(selected(HudElementId::AccelMeter, helper, native), 1);
        assert_ne!(native & speed::GROUNDSPEED, 0);
    }
}
