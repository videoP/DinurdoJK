use super::*;

#[test]
fn contact_blobs_supplement_maps_without_replacing_classic_blob_mode() {
    let mut video = VideoSettings::default();
    for mode in DynamicShadowsMode::ALL {
        video.dynamic_shadows = mode;
        video.player_contact_shadows = true;
        assert_eq!(video.player_blob_shadow_opacity(), if mode == DynamicShadowsMode::Blob { 1.0 } else { 0.5 });
        video.player_contact_shadows = false;
        assert_eq!(video.player_blob_shadow_opacity(), if mode == DynamicShadowsMode::Blob { 1.0 } else { 0.0 });
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
fn hybrid_sun_shadows_have_distinct_persistent_name() {
    assert_eq!(
        DynamicShadowsMode::from_config("hybrid_csm_rt_entities"),
        Some(DynamicShadowsMode::HybridCsmRtEntities)
    );
    assert_eq!(
        DynamicShadowsMode::from_config("3"),
        Some(DynamicShadowsMode::RayTraced),
        "legacy RT numeric alias must not change"
    );
    assert_eq!(
        DynamicShadowsMode::from_config("2"),
        Some(DynamicShadowsMode::CascadedShadowMaps),
        "legacy CSM numeric alias must not change"
    );
}
