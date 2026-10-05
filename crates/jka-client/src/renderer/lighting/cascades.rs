//! Lighting cascades.
use crate::renderer::{
    Camera, Mat4, Vec3, Vec4, BEVY_CSM_FIRST_FAR_BOUND, BEVY_CSM_MAXIMUM_DISTANCE,
    BEVY_CSM_MINIMUM_DISTANCE, BEVY_CSM_OVERLAP_PROPORTION, SHADOW_CASCADES, SHADOW_MAP_SIZE,
};

pub(in crate::renderer) fn bevy_cascade_bounds() -> [f32; SHADOW_CASCADES] {
    let base = (BEVY_CSM_MAXIMUM_DISTANCE / BEVY_CSM_FIRST_FAR_BOUND)
        .powf(1.0 / (SHADOW_CASCADES - 1) as f32);
    std::array::from_fn(|i| BEVY_CSM_FIRST_FAR_BOUND * base.powf(i as f32))
}

pub(in crate::renderer) fn bevy_cascade_shadow_matrices(
    camera: &Camera,
    width: u32,
    height: u32,
    light_direction: Vec3,
) -> (
    [Mat4; SHADOW_CASCADES],
    [f32; SHADOW_CASCADES],
    [f32; SHADOW_CASCADES],
) {
    // Faithful port of Bevy 0.19.1 bevy_light::cascade::{build_directional_light_cascades,
    // calculate_cascade}, with only the cascade distances scaled to JKA world units.
    let light_direction = light_direction.normalize_or_zero();
    let reference_up = if light_direction.dot(Vec3::Y).abs() > 0.95 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    // Bevy builds cascades from the light transform's pure rotation. JKA stores
    // the sun's forward/travel direction directly, so construct the equivalent
    // zero-translation light view and transpose it to obtain world_from_light.
    let light_from_world = Mat4::look_to_rh(Vec3::ZERO, light_direction, reference_up);
    let world_from_light = light_from_world.transpose();
    let bounds = bevy_cascade_bounds();
    let overlap_factor = 1.0 - BEVY_CSM_OVERLAP_PROPORTION;
    let mut matrices = [Mat4::IDENTITY; SHADOW_CASCADES];
    let mut texel_sizes = [0.0; SHADOW_CASCADES];

    for cascade in 0..SHADOW_CASCADES {
        let near_bound = if cascade == 0 {
            BEVY_CSM_MINIMUM_DISTANCE
        } else {
            overlap_factor * bounds[cascade - 1]
        };
        let far_bound = bounds[cascade];
        let corners = camera.frustum_corners(width, height, near_bound, far_bound);

        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        for corner_world in corners {
            let corner_light_view = light_from_world.transform_point3(corner_world);
            min = min.min(corner_light_view);
            max = max.max(corner_light_view);
        }

        let body_diagonal = corners[0].distance_squared(corners[6]);
        let far_plane_diagonal = corners[4].distance_squared(corners[6]);
        let cascade_diameter = body_diagonal.max(far_plane_diagonal).sqrt().ceil();
        let cascade_texel_size = cascade_diameter / SHADOW_MAP_SIZE as f32;
        let near_plane_center = Vec3::new(
            (0.5 * (min.x + max.x) / cascade_texel_size).floor() * cascade_texel_size,
            (0.5 * (min.y + max.y) / cascade_texel_size).floor() * cascade_texel_size,
            max.z,
        );

        let world_from_light_transpose = world_from_light.transpose();
        let cascade_from_world = Mat4::from_cols(
            world_from_light_transpose.x_axis,
            world_from_light_transpose.y_axis,
            world_from_light_transpose.z_axis,
            (-near_plane_center).extend(1.0),
        );
        let r = (max.z - min.z).recip();
        let clip_from_cascade = Mat4::from_cols(
            Vec4::new(2.0 / cascade_diameter, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 2.0 / cascade_diameter, 0.0, 0.0),
            Vec4::new(0.0, 0.0, r, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 1.0),
        );
        matrices[cascade] = clip_from_cascade * cascade_from_world;
        texel_sizes[cascade] = cascade_texel_size;
    }

    (matrices, bounds, texel_sizes)
}
