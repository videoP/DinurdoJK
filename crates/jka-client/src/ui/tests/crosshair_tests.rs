use super::*;

#[test]
fn dot_crosshair_is_circular_and_exactly_centered() {
    for (width, height) in [(1280_u32, 720_u32), (1279_u32, 799_u32)] {
        let mut vertices = Vec::new();
        build_crosshair(
            &mut vertices,
            CrosshairSettings {
                style: 2,
                size: 24.0,
                color: [255, 255, 255, 255],
                ..CrosshairSettings::default()
            },
            None,
            1.0,
            width,
            height,
        );
        assert_eq!(vertices.len(), 16 * 3);

        let to_pixels = |vertex: &UiVertex| {
            (
                (vertex.position[0] + 1.0) * 0.5 * width as f32,
                (1.0 - vertex.position[1]) * 0.5 * height as f32,
            )
        };
        let points = vertices.iter().map(to_pixels).collect::<Vec<_>>();
        let min_x = points.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
        let max_x = points.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max);
        let min_y = points.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
        let max_y = points.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);
        let center_x = (min_x + max_x) * 0.5;
        let center_y = (min_y + max_y) * 0.5;

        assert!((center_x - width as f32 * 0.5).abs() < 0.001);
        assert!((center_y - height as f32 * 0.5).abs() < 0.001);
        assert!(((max_x - min_x) - (max_y - min_y)).abs() < 0.001);

        // Every non-center perimeter vertex lies on one radius in pixel
        // space, so the dot cannot regress to the old axis-aligned square.
        let expected_radius = (max_x - min_x) * 0.5;
        for &(x, y) in points
            .iter()
            .filter(|&&(x, y)| (x - center_x).abs() > 0.001 || (y - center_y).abs() > 0.001)
        {
            let radius = ((x - center_x).powi(2) + (y - center_y).powi(2)).sqrt();
            assert!((radius - expected_radius).abs() < 0.002);
        }
    }
}

fn pixel_bounds(vertices: &[UiVertex], width: u32, height: u32) -> [f32; 4] {
    let points = vertices.iter().map(|vertex| {
        (
            (vertex.position[0] + 1.0) * 0.5 * width as f32,
            (1.0 - vertex.position[1]) * 0.5 * height as f32,
        )
    });
    points.fold(
        [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ],
        |b, (x, y)| [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)],
    )
}

#[test]
fn image_crosshair_is_a_centered_atlas_quad() {
    let (width, height) = (1280_u32, 720_u32);
    let mut vertices = Vec::new();
    let crosshair = CrosshairSettings {
        style: 1,
        image: 3,
        size: 32.0,
        ..CrosshairSettings::default()
    };
    build_crosshair(&mut vertices, crosshair, None, None, 1.0, width, height);
    assert_eq!(vertices.len(), 6);
    assert!(vertices
        .iter()
        .all(|vertex| vertex.textured == ICON_TEXTURE_SOURCE));

    let [min_x, min_y, max_x, max_y] = pixel_bounds(&vertices, width, height);
    assert!((max_x - min_x - 32.0).abs() < 0.01 && (max_y - min_y - 32.0).abs() < 0.01);
    assert!(((min_x + max_x) * 0.5 - width as f32 * 0.5).abs() < 0.01);
    assert!(((min_y + max_y) * 0.5 - height as f32 * 0.5).abs() < 0.01);

    // Image 3 is the third crosshair, after the two lagometer cells.
    let (uv0, uv1) = icon_cell_uv(ICON_CROSSHAIR_BASE + 2);
    let u_min = vertices
        .iter()
        .map(|v| v.uv[0])
        .fold(f32::INFINITY, f32::min);
    let u_max = vertices
        .iter()
        .map(|v| v.uv[0])
        .fold(f32::NEG_INFINITY, f32::max);
    assert!((u_min - uv0[0]).abs() < 1e-6 && (u_max - uv1[0]).abs() < 1e-6);
}

#[test]
fn image_zero_and_out_of_range_fall_back_to_the_shape() {
    for image in [0, CROSSHAIR_IMAGE_COUNT + 1] {
        let mut vertices = Vec::new();
        let crosshair = CrosshairSettings {
            style: 2,
            image,
            ..CrosshairSettings::default()
        };
        build_crosshair(&mut vertices, crosshair, None, None, 1.0, 1280, 720);
        assert!(vertices.iter().all(|vertex| vertex.textured == 0.0));
        assert_eq!(vertices.len(), 16 * 3);
    }
}

#[test]
fn line_crosshair_is_a_short_vertical_line() {
    let (width, height) = (1280_u32, 960_u32);
    let mut vertices = Vec::new();
    let crosshair = CrosshairSettings {
        style: CROSSHAIR_STYLE_LINE,
        size: 24.0,
        ..CrosshairSettings::default()
    };
    build_crosshair(&mut vertices, crosshair, None, None, 2.0, width, height);
    assert_eq!(vertices.len(), 6);
    let [min_x, min_y, max_x, max_y] = pixel_bounds(&vertices, width, height);
    // 2 units wide at 2 px per unit; 1.25x the plus (24 * 2/3 = 16 px) tall.
    assert!((max_x - min_x - 4.0).abs() < 0.01);
    assert!((max_y - min_y - 20.0).abs() < 0.01);
    assert!(((min_x + max_x) * 0.5 - width as f32 * 0.5).abs() < 0.01);
    assert!(((min_y + max_y) * 0.5 - height as f32 * 0.5).abs() < 0.01);

    // The width follows jaPRO's 0.25..=5 clamp.
    let mut thick = Vec::new();
    build_crosshair(&mut thick, crosshair, None, None, 50.0, width, height);
    let [min_x, _, max_x, _] = pixel_bounds(&thick, width, height);
    assert!((max_x - min_x - 10.0).abs() < 0.01);
}
