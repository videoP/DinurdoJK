use super::cvar_prelude::*;
use crate::console::cvars::CvarScalar;

macro_rules! audio_cvars {
    ($($id:ident, $field:ident, $name:literal, $range:literal, $kind:expr, $order:literal, $help:literal;)*) => {
        app_cvars! {
            $($id {
                runtime: CvarDef::new(
                    $name, "", $range, $help, $kind, CvarFlags::ARCHIVE, LatchScope::None,
                    |app: &App| CvarScalar::cvar_value(app.audio.echofield_settings.$field),
                    |app, value| {
                        app.audio.echofield_settings.$field = CvarScalar::from_cvar_value(value)?;
                        app.apply_echofield_settings();
                        Ok(())
                    },
                ),
                config: CvarBinding {
                    load_audio: Some(|settings, _, text| {
                        if let Ok(value) = $id.parse(text).and_then(CvarScalar::from_cvar_value) {
                            settings.echofield_settings.$field = value;
                        }
                    }),
                    archive: Some(($order, |state| CvarScalar::cvar_value(state.audio.echofield_settings.$field).to_string())),
                    ..CvarBinding::NONE
                },
                implementations: []
            })*
        }
    };
}

audio_cvars! {
    ECHO_ENHANCEMENTS, enhancements, "s_echoFieldEnhancements", "0..1", CvarType::Bool, 43000,
        "Enable the Echofield soundscape and direct-path supplements. Off keeps the map's authored EAGLE path or the base BSP propagation.";
    ECHO_VOLUME, master_volume, "s_echoFieldVolume", "0..1", CvarType::Float { min: 0.0, max: 1.0 }, 43001,
        "Overall level of Echofield ambience and supplemental early reflections.";
    ECHO_EMITTERS, emitters, "s_echoFieldEmitters", "0..1", CvarType::Bool, 43002,
        "Enable positioned environmental loops and triggered emitters.";
    ECHO_DENSITY, emitter_density, "s_echoFieldEmitterDensity", "0.25..1", CvarType::Float { min: 0.25, max: 1.0 }, 43003,
        "Density of emitters beyond half their audible range.";
    ECHO_EMITTER_MASK, emitter_mask, "s_echoFieldEmitterMask", "0..32766", CvarType::Int { min: 0, max: 32766 }, 43004,
        "Add enabled emitter bits: leaves 16, grass 32, whistle 64, flow 128, drip 256, rain 512, still water 1024, lava 2048, cave events 4096, cave drone 8192, mechanical 16384.";
    ECHO_EXCITATION_MASK, excitation_mask, "s_echoFieldExcitationMask", "0..31", CvarType::Int { min: 0, max: 31 }, 43007,
        "Add enabled excitation bits: wind 1, foliage 2, grass 4, water 8, lava 16.";
    ECHO_EXCITATION, excitation_volume, "s_echoFieldExcitationVolume", "0..1", CvarType::Float { min: 0.0, max: 1.0 }, 43008,
        "Level of wind and other continuous environmental excitation.";
    ECHO_TERRAIN, terrain, "s_echoFieldTerrain", "0..1", CvarType::Bool, 43009,
        "Enable positional sounds that reveal terrain features.";
    ECHO_TERRAIN_RANGE, terrain_range, "s_echoFieldTerrainRange", "8..32", CvarType::Int { min: 8, max: 32 }, 43010,
        "Terrain scan radius in metres.";
    ECHO_TERRAIN_LEVEL, terrain_intensity, "s_echoFieldTerrainIntensity", "0..1", CvarType::Float { min: 0.0, max: 1.0 }, 43011,
        "Level of Echofield terrain sounds.";
    ECHO_REFLECTION, reflection_intensity, "s_echoFieldEarlyReflections", "0..1", CvarType::Float { min: 0.0, max: 1.0 }, 43012,
        "Level of delayed reflections for the BSP headphone rendering path.";
    ECHO_DR, per_source_dr, "s_echoFieldDirectReverb", "0..1", CvarType::Bool, 43013,
        "Adjust each BSP source's direct-to-reverberant ratio using critical distance.";
    ECHO_DIFFRACTION, diffraction, "s_echoFieldDiffraction", "0..1", CvarType::Bool, 43014,
        "Allow directional rerouting of sound around obstacles.";
    ECHO_TRANSMISSION, transmission, "s_echoFieldTransmission", "0..1", CvarType::Bool, 43015,
        "Enable Echofield material and mover attenuation.";
    ECHO_AIR, air_absorption, "s_echoFieldAirAbsorption", "0..1", CvarType::Bool, 43016,
        "Enable frequency-dependent atmospheric absorption.";
    ECHO_HUMIDITY, humidity, "s_echoFieldHumidity", "0..100", CvarType::Float { min: 0.0, max: 100.0 }, 43017,
        "Relative humidity in percent. Zero derives humidity from weather and local liquid.";
    ECHO_MOVING, resample_moving_sources, "s_echoFieldUpdateMoving", "0..1", CvarType::Bool, 43018,
        "Refresh acoustic results when a sound source moves.";
    ECHO_DOPPLER, doppler, "s_echoFieldDoppler", "0..1", CvarType::Bool, 43019,
        "Shift positional sound pitch with listener and source motion.";
}
