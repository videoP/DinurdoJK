use super::*;

fn lamp(position: [f32; 3], intensity: f32) -> scene::DynamicLight {
    scene::DynamicLight {
        position,
        color: [1.0; 3],
        radius: 300.0,
        intensity,
        falloff: scene::DynamicLightFalloff::InverseSquare,
        surface_lighting: true,
        emitter_normal: [0.0; 3],
        emitter_two_sided: false,
        angle_attenuation: true,
        angle_scale: 0.0,
        extra_distance: 0.0,
    }
}

#[test]
fn authored_light_must_agree_with_the_baked_direction() {
    let focus = Vec3::ZERO;
    // The baked direction points up and to +x. A brighter lamp on the opposite
    // side (behind a wall, as far as the bake is concerned) must lose.
    let grid = Vec3::new(1.0, 1.0, 0.0).normalize();
    let lights = [
        lamp([-200.0, 200.0, 0.0], 8.0),
        lamp([200.0, 200.0, 0.0], 1.0),
    ];
    let picked = best_authored_light_direction(&lights, focus, Some(grid)).unwrap();
    assert!(
        picked.dot(Vec3::new(1.0, 1.0, 0.0).normalize()) > 0.99,
        "picked {picked:?}"
    );
    // Without a lightgrid sample there is no visibility proxy: brightest wins.
    let picked = best_authored_light_direction(&lights, focus, None).unwrap();
    assert!(picked.x < 0.0, "picked {picked:?}");
}

#[test]
fn authored_light_rejects_out_of_range_and_perpendicular_lamps() {
    let focus = Vec3::ZERO;
    let grid = Vec3::Y;
    assert!(
        best_authored_light_direction(&[lamp([0.0, 5000.0, 0.0], 8.0)], focus, Some(grid))
            .is_none()
    );
    assert!(
        best_authored_light_direction(&[lamp([0.0, 4.0, 0.0], 8.0)], focus, Some(grid)).is_none()
    );
    assert!(
        best_authored_light_direction(&[lamp([300.0, 0.0, 0.0], 8.0)], focus, Some(grid)).is_none()
    );
    assert!(best_authored_light_direction(&[], focus, None).is_none());
}
