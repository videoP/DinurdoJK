use super::*;

#[test]
fn contact_blobs_supplement_maps_without_replacing_classic_blob_mode() {
    let mut video = VideoSettings::default();
    for mode in DynamicShadowsMode::ALL {
        video.dynamic_shadows = mode;
        video.player_contact_shadows = 2.0;
        for strength in [0.0, 0.25, 0.6, 1.0] {
            video.sun_shadow_strength = strength;
            let settings = video.player_blob_shadow_settings();
            assert_eq!(settings.opacity, if mode == DynamicShadowsMode::Blob { 1.0 } else { 2.0 * strength });
            assert_eq!(settings.contact, mode != DynamicShadowsMode::Blob);
        }
        video.player_contact_shadows = 0.0;
        assert_eq!(video.player_blob_shadow_settings().opacity, if mode == DynamicShadowsMode::Blob { 1.0 } else { 0.0 });
    }
}

#[test]
fn dynamic_shadow_modes_round_trip_and_keep_old_stencil_configs() {
    for mode in DynamicShadowsMode::ALL {
        assert_eq!(
            DynamicShadowsMode::from_config(mode.config_value()),
            Some(mode)
        );
    }
    // `stencil` configs predate the Entity map and must land on it, not Off.
    for legacy in ["stencil", "stencil_legacy", "entity_map"] {
        assert_eq!(
            DynamicShadowsMode::from_config(legacy),
            Some(DynamicShadowsMode::EntityMap)
        );
    }
    for source in EntityShadowLight::ALL {
        assert_eq!(
            EntityShadowLight::from_config(source.config_value()),
            Some(source)
        );
    }
}

#[test]
fn removed_rt_sun_modes_migrate_to_csm() {
    assert_eq!(DynamicShadowsMode::ALL, [DynamicShadowsMode::Off, DynamicShadowsMode::Blob,
        DynamicShadowsMode::EntityMap, DynamicShadowsMode::CascadedShadowMapsEntities]);
    for legacy in ["hybrid_csm_rt_entities", "hybrid", "csm_rt_entities", "5",
        "ray_traced", "raytraced", "3", "4", "2"] {
        assert_eq!(DynamicShadowsMode::from_config(legacy), Some(DynamicShadowsMode::CascadedShadowMaps));
    }
    assert_eq!(DynamicLightsMode::from_config("ray_traced"), Some(DynamicLightsMode::RayTracedHardware));
}
