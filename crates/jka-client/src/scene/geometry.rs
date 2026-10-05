//! Geometry.
use crate::scene::{jka_position, Arc, BTreeMap, DrawBatch, DrawClass, GpuVertex, Vec3};

#[derive(Default)]
pub(in crate::scene) struct Geometry {
    /// Exact PVS pieces inside one coarse material/lightmap batch, keyed by
    /// camera-cluster visibility and area membership. Pieces are laid out
    /// contiguously so FULL and AUTO 4 reference subranges of the coarse range
    /// without duplicating world vertices.
    pub(in crate::scene) by_pvs_signature: BTreeMap<(Vec<u64>, [u64; 4]), WorldGeometryChunk>,
}

#[derive(Default)]
pub(in crate::scene) struct WorldGeometryChunk {
    pub(in crate::scene) vertices: Vec<GpuVertex>,
    pub(in crate::scene) surface_ids: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::scene) struct GrassPatchKey {
    pub(in crate::scene) cell_x: i32,
    pub(in crate::scene) cell_z: i32,
    pub(in crate::scene) pvs_signature: Vec<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::scene) struct GrassWorkerPatchKey {
    pub(in crate::scene) cell_x: i32,
    pub(in crate::scene) cell_z: i32,
    pub(in crate::scene) signature_id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::scene) struct GroupKey {
    pub(in crate::scene) class: DrawClass,
    pub(in crate::scene) shader: usize,
    pub(in crate::scene) lightmap: Option<usize>,
    pub(in crate::scene) vertex_lit: bool,
    pub(in crate::scene) color_slot: u8,
    pub(in crate::scene) fog_num: i32,
    pub(in crate::scene) transparent_order: usize,
    pub(in crate::scene) planar_group: Option<PlanarGroupKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::scene) struct PlanarGroupKey {
    pub(in crate::scene) normal_x: i32,
    pub(in crate::scene) normal_y: i32,
    pub(in crate::scene) normal_z: i32,
    pub(in crate::scene) distance: i32,
}

/// Markable-surface index for a source-.map world, which has no BSP surface
/// table to read shader flags from: every opaque/mask render triangle that faces
/// up enough is treated like an `SF_FACE`, as the old renderer-side gather did.
pub(in crate::scene) fn mark_surfaces_from_batches(
    vertices: &[GpuVertex],
    batches: &[DrawBatch],
) -> Arc<jka_assets::bsp::MarkSurfaces> {
    let mut triangles = Vec::new();
    for batch in batches {
        if !matches!(batch.pipeline.class, DrawClass::Opaque | DrawClass::Mask) {
            continue;
        }
        let start = batch.vertices.start as usize;
        let end = (batch.vertices.end as usize).min(vertices.len());
        if start >= end {
            continue;
        }
        for triangle in vertices[start..end].chunks_exact(3) {
            let normal = Vec3::from_array(triangle[0].normal)
                + Vec3::from_array(triangle[1].normal)
                + Vec3::from_array(triangle[2].normal);
            let normal = normal.normalize_or_zero();
            if normal == Vec3::ZERO {
                continue;
            }
            triangles.push((
                [
                    triangle[0].position,
                    triangle[1].position,
                    triangle[2].position,
                ]
                .map(jka_position),
                jka_position(normal.to_array()),
            ));
        }
    }
    Arc::new(jka_assets::bsp::MarkSurfaces::from_world_triangles(
        triangles,
    ))
}
