//! Authored water-brush bounds retained for promoted-water optics.
use super::{WaterFootprint, WaterVolume};
use jka_assets::bsp::{bathymetry::CONTENTS_WATER, Bsp};

pub fn from_jka_bounds(minimum: [f32; 3], maximum: [f32; 3]) -> WaterVolume {
    WaterVolume {
        minimum: [minimum[0], minimum[2], -maximum[1]],
        maximum: [maximum[0], maximum[2], -minimum[1]],
        mask_slot: None,
    }
}

pub fn from_bsp(bsp: &Bsp) -> Vec<WaterVolume> {
    let Some(world) = bsp.models.first() else {
        return Vec::new();
    };
    world
        .brushes
        .clone()
        .filter_map(|id| {
            let brush = &bsp.brushes[id];
            if bsp.shaders[brush.shader].contents & CONTENTS_WATER == 0 {
                return None;
            }
            let mut minimum = [f32::NEG_INFINITY; 3];
            let mut maximum = [f32::INFINITY; 3];
            // BSP collision brushes include axial bounding/bevel planes, even for
            // sloped brushes. Do not infer depth from rendered water undersides:
            // q3map2 may remove those faces or merge them with other water.
            for side in &bsp.brush_sides[brush.sides.clone()] {
                let plane = bsp.planes[side.plane];
                for axis in 0..3 {
                    if plane.normal[axis] > 0.999 {
                        maximum[axis] = maximum[axis].min(plane.distance);
                    } else if plane.normal[axis] < -0.999 {
                        minimum[axis] = minimum[axis].max(-plane.distance);
                    }
                }
            }
            (0..3)
                .all(|a| {
                    minimum[a].is_finite() && maximum[a].is_finite() && minimum[a] < maximum[a]
                })
                .then(|| from_jka_bounds(minimum, maximum))
        })
        .collect()
}

fn overlaps(triangle: super::MaskTriangle, minimum: [f32; 2], maximum: [f32; 2]) -> bool {
    let edges = [0, 1, 2].map(|i| {
        let a = triangle[i];
        let b = triangle[(i + 1) % 3];
        [a[1] - b[1], b[0] - a[0]]
    });
    let corners = [
        minimum,
        [minimum[0], maximum[1]],
        maximum,
        [maximum[0], minimum[1]],
    ];
    [[1.0, 0.0], [0.0, 1.0]]
        .into_iter()
        .chain(edges)
        .all(|axis| {
            let length = axis[0].hypot(axis[1]);
            if length < 1e-6 {
                return true;
            }
            let project = |p: [f32; 2]| (p[0] * axis[0] + p[1] * axis[1]) / length;
            let a = triangle.map(project);
            let b = corners.map(project);
            let lo_a = a.into_iter().fold(f32::INFINITY, f32::min);
            let hi_a = a.into_iter().fold(f32::NEG_INFINITY, f32::max);
            let lo_b = b.into_iter().fold(f32::INFINITY, f32::min);
            let hi_b = b.into_iter().fold(f32::NEG_INFINITY, f32::max);
            hi_a > lo_b + 0.001 && hi_b > lo_a + 0.001
        })
}

/// Conservative floor of the brushes supporting this surface. Other floors,
/// disconnected pools and brushes inside footprint holes cannot deepen it.
/// A sheet with no supporting volume supplies no invented underwater medium.
pub fn promoted_bottom(footprint: &WaterFootprint, brushes: &[WaterVolume]) -> f32 {
    brushes
        .iter()
        .filter(|brush| {
            (brush.maximum[1] - footprint.plane).abs() <= 1.0
                && footprint.triangles.iter().any(|&triangle| {
                    overlaps(
                        triangle,
                        [brush.minimum[0], brush.minimum[2]],
                        [brush.maximum[0], brush.maximum[2]],
                    )
                })
        })
        .map(|brush| brush.minimum[1])
        .fold(footprint.plane, f32::min)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocean::{build_water_footprints, OceanMasks, WaterFace};

    fn footprint() -> WaterFootprint {
        build_water_footprints(
            &[WaterFace {
                plane: -2011.0,
                minimum: [1383.0, 723.75],
                maximum: [3399.0, 8402.25],
                authored_ocean: None,
                material: 0,
            }],
            &[vec![
                [[1383.0, 723.75], [3399.0, 723.75], [1383.0, 8402.25]],
                [[3399.0, 723.75], [3399.0, 8402.25], [1383.0, 8402.25]],
            ]],
            256.0,
        )
        .remove(0)
    }

    #[test]
    fn promotion_preserves_the_brush_bottom_and_does_not_borrow_a_lower_pool() {
        let lake = footprint();
        let brush = from_jka_bounds([1383.0, -8402.25, -2067.0], [3399.0, -723.75, -2011.0]);
        let basement = from_jka_bounds([1383.0, -8402.25, -4000.0], [3399.0, -723.75, -3000.0]);
        let bottom = promoted_bottom(&lake, &[brush, basement]);
        assert_eq!(bottom, -2067.0);
        let volume = WaterVolume {
            minimum: [lake.minimum[0], bottom, lake.minimum[1]],
            maximum: [lake.maximum[0], lake.plane, lake.maximum[1]],
            mask_slot: None,
        };
        let masks = OceanMasks::default();
        assert!(volume.contains([1725.0, -2040.0, 6485.0], &masks));
        assert!(!volume.contains([1725.0, -3273.0, 6485.0], &masks));
        assert_eq!(promoted_bottom(&lake, &[basement]), lake.plane);
    }

    #[test]
    fn a_brush_touching_the_outline_or_inside_a_hole_cannot_deepen_water() {
        assert!(!overlaps(
            [[0.0, 0.0], [100.0, 0.0], [0.0, 100.0]],
            [100.0, 0.0],
            [200.0, 100.0]
        ));
        assert!(!overlaps(
            [[0.0, 0.0], [100.0, 0.0], [0.0, 100.0]],
            [60.0, 60.0],
            [80.0, 80.0]
        ));
        assert!(overlaps(
            [[0.0, 0.0], [100.0, 0.0], [0.0, 100.0]],
            [10.0, 10.0],
            [20.0, 20.0]
        ));
    }
}
