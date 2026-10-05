//! Environment.
use crate::scene::{
    Bsp, SurfaceKind, Vec3, WeatherOcclusionBrush, WeatherOcclusionSide, WeatherOcclusionSource,
    WeatherOcclusionTriangle,
};

/// Whether rain can pool on a surface with these shader contents/flags.
/// `MATERIAL_*` ids come from JKA `surfaceflags.h` (see `jka_assets::bsp`).
pub(in crate::scene) fn surface_holds_puddles(contents: u32, surface_flags: u32) -> bool {
    const CONTENTS_LAVA: u32 = 0x0000_0002;
    const CONTENTS_WATER: u32 = 0x0000_0004;
    const ABSORBENT_MATERIALS: [u32; 13] = [
        5,  // short grass
        6,  // long grass
        8,  // sand
        9,  // gravel
        13, // water
        14, // snow
        15, // ice
        16, // flesh
        19, // dry leaves
        20, // green leaves
        21, // fabric
        22, // canvas
        27, // carpet
    ];
    if contents & (CONTENTS_LAVA | CONTENTS_WATER) != 0 {
        return false;
    }
    let material = surface_flags & jka_assets::bsp::MATERIAL_MASK;
    !ABSORBENT_MATERIALS.contains(&material)
}

pub(in crate::scene) fn bsp_weather_occlusion_source(
    bsp: &Bsp,
    mesh: &jka_assets::bsp::Mesh,
) -> Option<WeatherOcclusionSource> {
    const CONTENTS_SOLID: u32 = 0x0000_0001;
    const CONTENTS_TERRAIN: u32 = 0x0000_1000;
    const SURF_SKY: u32 = 0x0000_2000;
    const SURF_NODRAW: u32 = 0x0020_0000;
    let world = bsp.models.first()?;
    let mut brushes = Vec::new();
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;

    for brush_index in world.brushes.clone() {
        let Some(brush) = bsp.brushes.get(brush_index) else {
            continue;
        };
        let Some(brush_shader) = bsp.shaders.get(brush.shader) else {
            continue;
        };
        if brush_shader.contents & (CONTENTS_SOLID | CONTENTS_TERRAIN) == 0 {
            continue;
        }
        let mut sides = Vec::with_capacity(brush.sides.len());
        let mut has_sky_side = false;
        for side_index in brush.sides.clone() {
            let Some(side) = bsp.brush_sides.get(side_index) else {
                continue;
            };
            let Some(plane) = bsp.planes.get(side.plane) else {
                continue;
            };
            let surface_flags = bsp
                .shaders
                .get(side.shader)
                .map_or(0, |shader| shader.surface_flags);
            has_sky_side |= surface_flags & SURF_SKY != 0;
            sides.push(WeatherOcclusionSide {
                normal: plane.normal,
                distance: plane.distance,
            });
        }
        if has_sky_side || sides.len() < 6 {
            continue;
        }
        let brush_min_x = -sides[0].distance;
        let brush_max_x = sides[1].distance;
        let brush_min_y = -sides[2].distance;
        let brush_max_y = sides[3].distance;
        if ![brush_min_x, brush_max_x, brush_min_y, brush_max_y]
            .into_iter()
            .all(f32::is_finite)
            || brush_min_x >= brush_max_x
            || brush_min_y >= brush_max_y
        {
            continue;
        }
        min_x = min_x.min(brush_min_x);
        min_y = min_y.min(brush_min_y);
        max_x = max_x.max(brush_max_x);
        max_y = max_y.max(brush_max_y);
        brushes.push(WeatherOcclusionBrush {
            mins_xy: [brush_min_x, brush_min_y],
            maxs_xy: [brush_max_x, brush_max_y],
            sides,
        });
    }

    let mut triangles = Vec::new();
    let mut topography_triangles = Vec::new();
    for batch in &mesh.batches {
        let Some(surface) = bsp.surfaces.get(batch.surface) else {
            continue;
        };
        if surface.kind == SurfaceKind::Flare {
            continue;
        }
        let Some(shader) = bsp.shaders.get(batch.shader) else {
            continue;
        };
        if shader.surface_flags & (SURF_SKY | SURF_NODRAW) != 0 {
            continue;
        }
        let blocks_rain = shader.contents & (CONTENTS_SOLID | CONTENTS_TERRAIN) != 0;
        let holds_puddles = surface_holds_puddles(shader.contents, shader.surface_flags);
        for indices in mesh.indices[batch.indices.clone()].chunks_exact(3) {
            let (Some(a), Some(b), Some(c)) = (
                mesh.vertices.get(indices[0] as usize).map(|v| v.position),
                mesh.vertices.get(indices[1] as usize).map(|v| v.position),
                mesh.vertices.get(indices[2] as usize).map(|v| v.position),
            ) else {
                continue;
            };
            let ab = Vec3::from_array(b) - Vec3::from_array(a);
            let ac = Vec3::from_array(c) - Vec3::from_array(a);
            // Vertical/degenerate triangles have no XZ footprint for standing water
            // and cannot block vertically falling precipitation in the heightfield.
            if ab.cross(ac).z.abs() <= 1.0e-4 {
                continue;
            }
            for point in [a, b, c] {
                min_x = min_x.min(point[0]);
                min_y = min_y.min(point[1]);
                max_x = max_x.max(point[0]);
                max_y = max_y.max(point[1]);
            }
            let triangle = WeatherOcclusionTriangle {
                positions: [a, b, c],
                holds_puddles,
            };
            topography_triangles.push(triangle);
            if blocks_rain && matches!(surface.kind, SurfaceKind::Patch | SurfaceKind::Triangles) {
                // Brush solids already provide their ordinary planar blocker faces.
                // Keep tessellated patches/triangle soups as blocker supplements.
                triangles.push(triangle);
            }
        }
    }
    if (brushes.is_empty() && triangles.is_empty() && topography_triangles.is_empty())
        || ![min_x, min_y, max_x, max_y].into_iter().all(f32::is_finite)
    {
        return None;
    }
    Some(WeatherOcclusionSource {
        min_xz: [min_x, -max_y],
        max_xz: [max_x, -min_y],
        brushes,
        triangles,
        topography_triangles,
    })
}

// 2Retr0/GodotGrass uses 5 m tiles and 10 blades per meter at density 1.0.
// JKA map units are treated as 64 units / Godot meter in grass.rs. The common
// stock Yavin grass sprite density is 42; preserve authored relative density
// while calibrating that value to the GodotGrass demo's full-density spacing.
pub(in crate::scene) const GRASS_PATCH_SIZE: f32 = 5.0 * 64.0;

pub(in crate::scene) const GRASS_REFERENCE_SPRITE_DENSITY: f32 = 42.0;

pub(in crate::scene) const GRASS_FULL_DENSITY_SPACING: f32 = 64.0 / 10.0;

pub(in crate::scene) const GRASS_MIN_SPACING: f32 = 3.2;

pub(in crate::scene) const GRASS_MAX_SPACING: f32 = 96.0;

pub(in crate::scene) const GRASS_MAX_BLADES_PER_TRIANGLE: usize = 32768;
