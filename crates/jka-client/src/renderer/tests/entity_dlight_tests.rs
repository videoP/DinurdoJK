use super::*;
use models::types::add_entity_dlights;

fn dlight(position: [f32; 3], color: [f32; 3], radius: f32) -> EntityDlight {
    EntityDlight {
        position: Vec3::from_array(position),
        color: Vec3::from_array(color),
        radius,
        radial_exponent: None,
    }
}

fn assert_close(actual: [f32; 3], expected: [f32; 3]) {
    for axis in 0..3 {
        assert!(
            (actual[axis] - expected[axis]).abs() < 1e-3,
            "{actual:?} != {expected:?}"
        );
    }
}

#[test]
fn dlight_adds_inverse_square_directed_light_toward_itself() {
    let mut light = scene::ClassicEntityLight::default();
    // TaystJK: 16 * r^2 / d^2 = 16 * 100^2 / 64^2.
    let weight = 16.0 * 100.0 * 100.0 / (64.0 * 64.0);
    add_entity_dlights(
        &mut light,
        Vec3::ZERO,
        &[dlight([64.0, 0.0, 0.0], [1.0, 0.5, 0.0], 100.0)],
    );
    assert_close(light.directed, [weight, weight * 0.5, 0.0]);
    assert_close(light.direction, [1.0, 0.0, 0.0]);
}

#[test]
fn dlight_distance_never_drops_below_minimum_radius() {
    let mut light = scene::ClassicEntityLight::default();
    add_entity_dlights(
        &mut light,
        Vec3::ZERO,
        &[dlight([0.0, 4.0, 0.0], [1.0; 3], 40.0)],
    );
    // Clamped to 16 units: 16 * 40^2 / 16^2.
    let weight = 16.0 * 40.0 * 40.0 / (16.0 * 16.0);
    assert_close(light.directed, [weight; 3]);
    assert_close(light.direction, [0.0, 1.0, 0.0]);
}

#[test]
fn dlight_direction_blends_with_lightgrid_by_magnitude() {
    let mut light = scene::ClassicEntityLight {
        ambient: [40.0; 3],
        directed: [30.0, 40.0, 0.0],
        direction: [0.0, 1.0, 0.0],
        ..Default::default()
    };
    // Weight 50 along +X against the grid's |directed| = 50 along +Y.
    let radius = (50.0_f32 * 128.0 * 128.0 / 16.0).sqrt();
    add_entity_dlights(
        &mut light,
        Vec3::ZERO,
        &[dlight([128.0, 0.0, 0.0], [1.0, 0.0, 0.0], radius)],
    );
    assert_close(light.ambient, [40.0; 3]);
    assert_close(light.directed, [80.0, 40.0, 0.0]);
    let diagonal = std::f32::consts::FRAC_1_SQRT_2;
    assert_close(light.direction, [diagonal, diagonal, 0.0]);
}

#[test]
fn no_dlights_leaves_classic_light_untouched() {
    let original = scene::ClassicEntityLight {
        ambient: [10.0; 3],
        directed: [20.0, 30.0, 40.0],
        direction: [0.0, 0.0, 1.0],
        ..Default::default()
    };
    let mut light = original;
    add_entity_dlights(&mut light, Vec3::ONE, &[]);
    assert_eq!(light.directed, original.directed);
    assert_eq!(light.direction, original.direction);
}
