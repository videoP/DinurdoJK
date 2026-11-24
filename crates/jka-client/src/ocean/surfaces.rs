//! Shared promotion geometry; map baking and GPU upload use the same selection.
use crate::scene::{DrawBatch, GpuVertex};

pub struct PromotedFace<'a> {
    pub source: &'a DrawBatch,
    pub key: [u32; 6],
    pub face: super::WaterFace,
    pub triangles: Vec<super::MaskTriangle>,
}

pub fn collect<'a>(
    vertices: &[GpuVertex],
    coarse: &'a [DrawBatch],
    full: &'a [DrawBatch],
) -> Vec<PromotedFace<'a>> {
    let mut seen = std::collections::BTreeSet::new();
    coarse
        .iter()
        .chain(full)
        .filter_map(|source| {
            if !source.water_primary || !is_upward_water_face(source, vertices) {
                return None;
            }
            let (plane, minimum, maximum) = water_surface_extent(source, vertices);
            let key = ocean_surface_key(plane, minimum, maximum, source.bsp_shader_index);
            if !seen.insert(key) {
                return None;
            }
            let slice = &vertices[source.vertices.start as usize..source.vertices.end as usize];
            let triangles = slice
                .chunks_exact(3)
                .map(|t| [0, 1, 2].map(|i| [t[i].position[0], t[i].position[2]]))
                .collect();
            Some(PromotedFace {
                source,
                key,
                face: super::WaterFace {
                    plane,
                    minimum,
                    maximum,
                    authored_ocean: source.authored_ocean,
                    material: source.bsp_shader_index,
                },
                triangles,
            })
        })
        .collect()
}

/// Plane height and world-space xz footprint of a promoted water face.
pub fn water_surface_extent(
    source: &DrawBatch,
    vertices: &[GpuVertex],
) -> (f32, [f32; 2], [f32; 2]) {
    let slice = &vertices[source.vertices.start as usize..source.vertices.end as usize];
    let mut minimum = [f32::INFINITY; 3];
    let mut maximum = [f32::NEG_INFINITY; 3];
    for vertex in slice {
        for axis in 0..3 {
            minimum[axis] = minimum[axis].min(vertex.position[axis]);
            maximum[axis] = maximum[axis].max(vertex.position[axis]);
        }
    }
    (
        maximum[1],
        [minimum[0], minimum[2]],
        [maximum[0], maximum[2]],
    )
}

pub fn ocean_surface_key(
    plane: f32,
    minimum: [f32; 2],
    maximum: [f32; 2],
    material: u32,
) -> [u32; 6] {
    [
        plane.to_bits(),
        minimum[0].to_bits(),
        minimum[1].to_bits(),
        maximum[0].to_bits(),
        maximum[1].to_bits(),
        material,
    ]
}

/// A `surfaceparm water` brush contributes every face it has, so its underside
/// and its sides arrive looking exactly like its top. Only the upward-facing
/// horizontal face is the water surface the FFT ocean should replace.
pub fn is_upward_water_face(source: &DrawBatch, vertices: &[GpuVertex]) -> bool {
    let slice = &vertices[source.vertices.start as usize..source.vertices.end as usize];
    let mut lowest = f32::INFINITY;
    let mut highest = f32::NEG_INFINITY;
    let mut facing = 0.0f32;
    for vertex in slice {
        lowest = lowest.min(vertex.position[1]);
        highest = highest.max(vertex.position[1]);
        facing += vertex.normal[1];
    }
    lowest.is_finite() && facing > 0.0 && lowest >= highest - 1.0
}

/// Promoted pools for which every vertex of a water-overlay sheet lies within `PLANE_TOLERANCE`
/// of a promoted ocean plane and inside that ocean's bounds. Such a sheet (e.g.
/// costadelsol's `seph_surf` shore surf at y=2 and y=4 over the y=3 sea) only
/// duplicates the surf and foam the FFT ocean already draws.
pub fn overlay_ocean_slots(slice: &[GpuVertex], surfaces: &[super::OceanSurface]) -> u8 {
    const PLANE_TOLERANCE: f32 = 1.05;
    const EDGE_TOLERANCE: f32 = 1.0;
    if slice.is_empty() {
        return 0;
    }
    let mut low = [f32::INFINITY; 3];
    let mut high = [f32::NEG_INFINITY; 3];
    for vertex in slice {
        for axis in 0..3 {
            low[axis] = low[axis].min(vertex.position[axis]);
            high[axis] = high[axis].max(vertex.position[axis]);
        }
    }
    surfaces.iter().take(super::OCEAN_MAX_SURFACES).enumerate().fold(0, |slots, (slot, ocean)| {
        let covered = low[1] >= ocean.plane_height - PLANE_TOLERANCE
            && high[1] <= ocean.plane_height + PLANE_TOLERANCE
            && low[0] >= ocean.minimum[0] - EDGE_TOLERANCE
            && low[2] >= ocean.minimum[1] - EDGE_TOLERANCE
            && high[0] <= ocean.maximum[0] + EDGE_TOLERANCE
            && high[2] <= ocean.maximum[1] + EDGE_TOLERANCE;
        if covered { slots | (1u8 << slot) } else { slots }
    })
}

#[cfg(test)]
mod overlay_tests {
    use super::*;

    fn vertex(position: [f32; 3]) -> GpuVertex {
        GpuVertex {
            position, uv: [0.0; 2], lightmap_uv: [0.0; 2], normal: [0.0, 1.0, 0.0],
            color: [1.0; 4], alpha_cutoff: 1.0,
        }
    }

    #[test]
    fn surf_sheets_one_unit_off_the_sea_are_covered_but_distant_water_is_not() {
        let sea = [crate::ocean::OceanSurface {
            plane_height: 3.0, minimum: [-1000.0, -1000.0], maximum: [1000.0, 1000.0],
        }];
        let sheet = |y| [vertex([0.0, y, 0.0]), vertex([100.0, y, 0.0]), vertex([0.0, y, 100.0])];
        assert_eq!(overlay_ocean_slots(&sheet(2.0), &sea), 1, "y=2 under the y=3 sea");
        assert_eq!(overlay_ocean_slots(&sheet(4.0), &sea), 1, "y=4 over the y=3 sea");
        assert_eq!(overlay_ocean_slots(&sheet(4.5), &sea), 0, "a waterfall ledge, not a surf sheet");
        assert_eq!(overlay_ocean_slots(&sheet(80.0), &sea), 0);
        let outside = [vertex([5000.0, 3.0, 0.0]), vertex([5100.0, 3.0, 0.0]), vertex([5000.0, 3.0, 50.0])];
        assert_eq!(overlay_ocean_slots(&outside, &sea), 0, "outside the sea's bounds");
        assert_eq!(overlay_ocean_slots(&[], &sea), 0);
    }
}
