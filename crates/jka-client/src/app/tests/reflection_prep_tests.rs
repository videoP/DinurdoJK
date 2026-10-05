use super::*;

fn prepared(quality: ReflectionQuality) -> scene::MapPrepareOptions {
    let (planar_reflections, planar_environment, omit_environment_stages) =
        App::reflection_prep_flags(quality);
    scene::MapPrepareOptions {
        planar_reflections,
        planar_environment,
        omit_environment_stages,
        ..Default::default()
    }
}

fn serves(prepared_as: ReflectionQuality, requested: ReflectionQuality) -> bool {
    App::reflection_prep_serves(prepared(prepared_as), App::reflection_prep_flags(requested))
}

#[test]
fn legacy_low_and_medium_share_one_map_shape() {
    for a in [
        ReflectionQuality::Legacy,
        ReflectionQuality::Low,
        ReflectionQuality::Medium,
    ] {
        for b in [
            ReflectionQuality::Legacy,
            ReflectionQuality::Low,
            ReflectionQuality::Medium,
        ] {
            assert!(serves(a, b));
        }
    }
}

#[test]
fn quality_can_drop_live_but_not_grow_the_map_topology() {
    assert!(serves(ReflectionQuality::Ultra, ReflectionQuality::Medium));
    assert!(serves(ReflectionQuality::High, ReflectionQuality::Legacy));
    assert!(!serves(ReflectionQuality::Medium, ReflectionQuality::High));
    assert!(!serves(ReflectionQuality::Legacy, ReflectionQuality::Ultra));
}

#[test]
fn crossing_off_needs_the_map_prepared_again() {
    assert!(serves(ReflectionQuality::Off, ReflectionQuality::Off));
    assert!(!serves(ReflectionQuality::Ultra, ReflectionQuality::Off));
    assert!(!serves(ReflectionQuality::Off, ReflectionQuality::Legacy));
}
