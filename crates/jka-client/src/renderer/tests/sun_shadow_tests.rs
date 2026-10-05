use super::*;

fn box_at(z: f32) -> ([f32; 3], [f32; 3]) {
    ([-1.0, -1.0, z - 1.0], [1.0, 1.0, z + 1.0])
}

#[test]
fn sunward_casters_are_kept_for_forward_and_reverse_cascades() {
    // Forward-Z cascade: clip z = -view z, so z < 0 is toward the sun.
    let forward = Mat4::orthographic_rh(-10.0, 10.0, -10.0, 10.0, 0.1, 50.0);
    let (min, max) = box_at(500.0); // 500 units behind the eye, sunward of it
    assert!(aabb_intersects_shadow_frustum(min, max, forward, false));
    let (min, max) = box_at(-500.0); // far beyond the cascade, away from the sun
    assert!(!aabb_intersects_shadow_frustum(min, max, forward, false));

    // Reverse-Z cascade: z = 1 at the near (sunward) plane, 0 at the far plane.
    let reverse = Mat4::from_cols(
        Vec4::new(0.1, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 0.1, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 0.02, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 1.0),
    );
    let (min, max) = box_at(500.0); // clip z >> 1: sunward of the near plane
    assert!(aabb_intersects_shadow_frustum(min, max, reverse, true));
    let (min, max) = box_at(-500.0); // clip z << 0: beyond the far plane
    assert!(!aabb_intersects_shadow_frustum(min, max, reverse, true));
}

#[test]
fn sun_ray_runs_from_the_head_toward_the_sun() {
    let head = Vec3::new(100.0, 40.0, -20.0);
    let toward_sun = Vec3::new(0.0, 0.6, 0.8);
    let vertices = sun_ray_vertices(head, toward_sun, head + Vec3::new(0.0, 0.0, 300.0));
    let sun = head + toward_sun * SUN_RAY_LENGTH;
    let beam_start =
        Vec3::from_array(vertices[0].position).lerp(Vec3::from_array(vertices[1].position), 0.5);
    let beam_end =
        Vec3::from_array(vertices[4].position).lerp(Vec3::from_array(vertices[5].position), 0.5);
    assert!(beam_start.distance(sun) < 1.0);
    assert!(beam_end.distance(head) < 1.0);
    assert!(vertices
        .iter()
        .all(|v| v.position.iter().all(|c| c.is_finite())));
}
