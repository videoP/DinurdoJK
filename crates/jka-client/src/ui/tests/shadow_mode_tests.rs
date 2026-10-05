use super::*;

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
