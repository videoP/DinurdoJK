//! Pure CPU packing work. Workers never touch WGPU, presenter state or draw order.
use crate::renderer::{classic_vertex_illumination, scene, DynamicModelVertex, Vec3};
use std::ops::Range;

#[derive(Default)]
pub(in crate::renderer) struct BatchKeyCache {
    keys: std::collections::HashMap<(usize, u64, bool, Option<bool>), u64>,
    // Permanent material/mesh variant identities, allocated only on first use.
    group_ids: std::collections::HashMap<(usize, u64, bool, Option<bool>), u64>,
}

impl BatchKeyCache {
    /// The mesh owns this cache; texture IDs are monotonic for this renderer.
    /// Keep the exact legacy hash so sorting and grouping order remain identical.
    pub fn resolve(
        &mut self,
        draw: &crate::renderer::Ghoul2PendingDraw,
        texture_id: u64,
        glow: bool,
    ) -> (u64, bool) {
        let key = (
            draw.alpha_mode.index(),
            texture_id,
            draw.source_material,
            glow.then_some(draw.authored_glow),
        );
        if let Some(&hash) = self.keys.get(&key) {
            return (hash, true);
        }
        let hash = crate::renderer::DynamicModelRenderer::ghoul2_batch_hash(draw, glow);
        self.keys.insert(key, hash);
        (hash, false)
    }
    /// A collision-free ID for the exact mesh + material/pipeline variant.
    /// The renderer owns the monotonically increasing ID space; `BatchKeyCache`
    /// is owned by the already-resident Ghoul2 mesh. No string/Arc hashing or
    /// cross-frame snapshot dependency is needed to group visible instances.
    pub fn resolve_group_id(
        &mut self,
        draw: &crate::renderer::Ghoul2PendingDraw,
        texture_id: u64,
        glow: bool,
        next_id: &mut u64,
    ) -> (u64, bool) {
        let key = (
            draw.alpha_mode.index(), texture_id, draw.source_material,
            glow.then_some(draw.authored_glow),
        );
        if let Some(&id) = self.group_ids.get(&key) {
            return (id, true);
        }
        let id = *next_id;
        *next_id = next_id.checked_add(1).expect("Ghoul2 group ID overflow");
        self.group_ids.insert(key, id);
        (id, false)
    }

}

/// Reused per-frame bucket. The indices keep Ghoul2PendingDraw in its original
/// storage; only *distinct* groups need sorting. Group vectors are retained
/// across frames rather than allocating one Vec per visible player surface.
#[derive(Default)]
pub(in crate::renderer) struct Ghoul2FrameGroup {
    pub sort_key: (u8, u32, bool, u64, u64),
    pub indices: Vec<usize>,
}

#[derive(Clone, Copy, Default)]
pub(in crate::renderer) struct GeometryCounts {
    pub surfaces: u32,
    pub vertices: u64,
    pub indices: u64,
}

#[derive(Clone, Copy, Default)]
pub(in crate::renderer) struct PreparePerf {
    pub cpu_sources: [GeometryCounts; 5],
    pub gather_ms: f64,
    pub cpu_pack_ms: f64,
    pub ghoul2_group_ms: f64,
    pub upload_ms: f64,
    pub parallel_used: bool,
    pub vertex_upload_bytes: u64,
    pub md3_cache_hits: u32,
    pub md3_cache_misses: u32,
    pub md3_reused_vertices: u64,
    pub md3_vertex_skipped_bytes: u64,
    pub md3_index_skipped_bytes: u64,
    pub index_upload_bytes: u64,
    pub index_skipped_bytes: u64,
    pub ghoul2_instanced_groups: u32,
    pub ghoul2_singleton_groups: u32,
    pub ghoul2_ordered_instances: u32,
    pub ghoul2_excluded_instances: u32,
    pub ghoul2_largest_group: u32,
    pub batch_key_hits: u32,
    pub batch_key_misses: u32,
}

pub(in crate::renderer) struct CpuPackJob {
    pub vertices: Range<usize>,
    pub indices: Range<usize>,
    pub light: Option<scene::ClassicEntityLight>,
    pub water: Option<crate::water_wetness::WaterProfile>,
}

/// Split only at surface boundaries, retaining exact primitive order. Gaps may
/// contain already-packed blob geometry and are deliberately left untouched.
pub(in crate::renderer) fn pack_jobs(
    jobs: &[CpuPackJob],
    vertices: &mut [DynamicModelVertex],
    indices: &mut [u32],
    vertex_base: usize,
    index_base: usize,
    splits: u32,
) {
    if jobs.len() >= 2 && splits != 0 && (vertices.len() >= 4096 || indices.len() >= 8192) {
        let midpoint = vertex_base + vertices.len() / 2;
        let split = jobs
            .partition_point(|job| job.vertices.start < midpoint)
            .clamp(1, jobs.len() - 1);
        let v = jobs[split].vertices.start;
        let i = jobs[split].indices.start;
        let (left_vertices, right_vertices) = vertices.split_at_mut(v - vertex_base);
        let (left_indices, right_indices) = indices.split_at_mut(i - index_base);
        rayon::join(
            || {
                pack_jobs(
                    &jobs[..split],
                    left_vertices,
                    left_indices,
                    vertex_base,
                    index_base,
                    splits - 1,
                )
            },
            || {
                pack_jobs(
                    &jobs[split..],
                    right_vertices,
                    right_indices,
                    v,
                    i,
                    splits - 1,
                )
            },
        );
        return;
    }
    for job in jobs {
        if let Some(water) = job.water {
            for vertex in &mut vertices[job.vertices.start - vertex_base..job.vertices.end - vertex_base] {
                vertex.water_wetness = water.wetness_at_z(vertex.position[1]);
            }
        }
        if let Some(light) = &job.light {
            for vertex in
                &mut vertices[job.vertices.start - vertex_base..job.vertices.end - vertex_base]
            {
                let normal = Vec3::from_array(vertex.normal).normalize_or_zero();
                let illumination = classic_vertex_illumination(light, normal);
                for channel in 0..3 {
                    vertex.color[channel] *= illumination[channel];
                }
            }
        }
        let base_vertex = job.vertices.start as u32;
        for index in &mut indices[job.indices.start - index_base..job.indices.end - index_base] {
            *index = base_vertex.saturating_add(*index);
        }
    }
}

#[derive(Default)]
pub(in crate::renderer) struct IndexUploadCache {
    resident: Vec<u32>,
}

impl IndexUploadCache {
    pub fn clear(&mut self) {
        self.resident.clear();
    }

    /// Compare in memcmp-sized blocks before narrowing to individual indices.
    /// A shorter matching prefix requires no GPU write: trailing indices are
    /// unreachable by this frame's draws. The returned span is in index units.
    pub fn update(&mut self, incoming: &[u32]) -> Option<Range<usize>> {
        const BLOCK: usize = 1024;
        let common = self.resident.len().min(incoming.len());
        let mut first = 0;
        while first + BLOCK <= common
            && self.resident[first..first + BLOCK] == incoming[first..first + BLOCK]
        {
            first += BLOCK;
        }
        while first < common && self.resident[first] == incoming[first] {
            first += 1;
        }
        if first == incoming.len() {
            self.resident.truncate(incoming.len());
            return None;
        }
        let mut end = incoming.len();
        if end <= self.resident.len() {
            while end >= first + BLOCK
                && self.resident[end - BLOCK..end] == incoming[end - BLOCK..end]
            {
                end -= BLOCK;
            }
            while end > first && self.resident[end - 1] == incoming[end - 1] {
                end -= 1;
            }
        }
        self.resident.resize(incoming.len(), 0);
        self.resident[first..end].copy_from_slice(&incoming[first..end]);
        Some(first..end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    #[test]
    fn cached_batch_keys_match_legacy_keys_across_texture_alpha_and_glow_variants() {
        use crate::renderer::{
            DynamicModelAlphaMode, DynamicModelRenderer, DynamicWireframeClass,
            Ghoul2GpuDrawUniform, Ghoul2PendingDraw,
        };
        let mut cache = BatchKeyCache::default();
        let mut draw = Ghoul2PendingDraw {
            entity_num: 7, portal_only: false,
            mesh_key: "model#lod0#surface1".into(),
            texture_key: Some("skin1".into()),
            alpha_mode: DynamicModelAlphaMode::Opaque,
            wireframe_class: DynamicWireframeClass::Player,
            authored_glow: false,
            source_material: false,
            texture_binding_id: 1,
            uniform: Ghoul2GpuDrawUniform::zeroed(),
            sequence: 0,
            batch_hash: None,
            batch_group_id: None,
        };
        for (texture, texture_id, alpha, authored_glow, prepare_glow) in [
            ("skin1", 1, DynamicModelAlphaMode::Opaque, false, false),
            ("skin1", 1, DynamicModelAlphaMode::Opaque, true, false),
            ("skin1", 1, DynamicModelAlphaMode::Opaque, true, true),
            ("skin1", 1, DynamicModelAlphaMode::Opaque, false, true),
            ("skin2", 2, DynamicModelAlphaMode::Opaque, false, false),
            ("skin2", 2, DynamicModelAlphaMode::Mask, false, false),
        ] {
            draw.texture_key = Some(texture.into());
            draw.alpha_mode = alpha;
            draw.authored_glow = authored_glow;
            let expected = DynamicModelRenderer::ghoul2_batch_hash(&draw, prepare_glow);
            assert_eq!(cache.resolve(&draw, texture_id, prepare_glow).0, expected);
            assert_eq!(
                cache.resolve(&draw, texture_id, prepare_glow),
                (expected, true)
            );
        }
        draw.source_material = true;
        let expected = DynamicModelRenderer::ghoul2_batch_hash(&draw, true);
        assert_eq!(cache.resolve(&draw, 2, true), (expected, false));
        assert_eq!(cache.resolve(&draw, 2, true), (expected, true));
    }

    #[test]
    fn group_ids_are_persistent_unique_across_mesh_and_material_variants() {
        use crate::renderer::{
            DynamicModelAlphaMode, DynamicWireframeClass, Ghoul2GpuDrawUniform, Ghoul2PendingDraw,
        };
        use bytemuck::Zeroable;
        let mut next_id = 1;
        let mut mesh_a = BatchKeyCache::default();
        let mut mesh_b = BatchKeyCache::default();
        let mut draw = Ghoul2PendingDraw {
            entity_num: 1, portal_only: false, mesh_key: "model#surface1".into(),
            texture_key: Some("skin".into()), alpha_mode: DynamicModelAlphaMode::Opaque,
            wireframe_class: DynamicWireframeClass::Player,
            authored_glow: false, source_material: false, texture_binding_id: 9,
            uniform: Ghoul2GpuDrawUniform::zeroed(), sequence: 0,
            batch_hash: None, batch_group_id: None,
        };
        assert_eq!(mesh_a.resolve_group_id(&draw, 9, false, &mut next_id), (1, false));
        assert_eq!(mesh_a.resolve_group_id(&draw, 9, false, &mut next_id), (1, true));
        assert_eq!(mesh_b.resolve_group_id(&draw, 9, false, &mut next_id), (2, false));
        draw.source_material = true;
        assert_eq!(mesh_a.resolve_group_id(&draw, 9, false, &mut next_id), (3, false));
        draw.source_material = false;
        draw.authored_glow = true;
        assert_eq!(mesh_a.resolve_group_id(&draw, 9, false, &mut next_id), (1, true));
        assert_eq!(mesh_a.resolve_group_id(&draw, 9, true, &mut next_id), (4, false));
        draw.alpha_mode = DynamicModelAlphaMode::Mask;
        assert_eq!(mesh_a.resolve_group_id(&draw, 9, true, &mut next_id), (5, false));
        draw.alpha_mode = DynamicModelAlphaMode::Opaque;
        assert_eq!(mesh_a.resolve_group_id(&draw, 10, true, &mut next_id), (6, false));
        assert_eq!(next_id, 7);
    }

    #[test]
    fn index_updates_preserve_the_active_buffer_after_growth_shrink_and_interior_changes() {
        let mut cache = IndexUploadCache::default();
        let mut gpu = Vec::<u32>::new();
        let cases = [
            vec![1, 2, 3, 4],
            vec![1, 2, 3, 4],
            vec![1, 8, 3, 4],
            vec![1, 8],
            vec![1, 8, 9, 10, 11],
            vec![],
            vec![7, 6, 5],
        ];
        for input in cases {
            if let Some(span) = cache.update(&input) {
                gpu.resize(gpu.len().max(input.len()), 0);
                gpu[span.clone()].copy_from_slice(&input[span]);
            }
            assert_eq!(&gpu[..input.len()], input.as_slice());
        }
        cache.clear();
        assert_eq!(cache.update(&[7, 6, 5]), Some(0..3));
    }

    #[test]
    fn index_updates_narrow_changes_across_block_boundaries() {
        let mut cache = IndexUploadCache::default();
        let mut input = vec![3; 4096];
        assert_eq!(cache.update(&input), Some(0..4096));
        assert_eq!(cache.update(&input), None);
        input[1023] = 8;
        input[2048] = 9;
        assert_eq!(cache.update(&input), Some(1023..2049));
        assert_eq!(cache.update(&input), None);
    }

    #[test]
    fn bounded_parallel_packing_matches_serial_bytes_and_preserves_gaps() {
        let mut input = vec![DynamicModelVertex::zeroed(); 9000];
        for (i, vertex) in input.iter_mut().enumerate() {
            vertex.normal = [1.0, 0.0, 0.0];
            vertex.color = [0.2, 0.4, 0.8, 0.5];
            vertex.position = [i as f32, 1.0, 2.0];
            vertex.depth_hack = 1.0;
        }
        let light = scene::ClassicEntityLight {
            ambient: [20.0, 40.0, 60.0],
            directed: [30.0; 3],
            direction: [1.0, 0.0, 0.0],
            sun_directed: [0.0; 3],
            sun_direction: [0.0; 3],
            ..Default::default()
        };
        let jobs = [
            CpuPackJob {
                vertices: 4..4000,
                indices: 3..4000,
                light: Some(light),
                water: None,
            },
            CpuPackJob {
                vertices: 4004..9000,
                indices: 4003..9000,
                light: None,
                water: None,
            },
        ];
        let mut serial_vertices = input.clone();
        let mut parallel_vertices = input;
        let mut serial_indices = vec![2; 9000];
        let mut parallel_indices = serial_indices.clone();
        pack_jobs(&jobs, &mut serial_vertices, &mut serial_indices, 0, 0, 0);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .unwrap();
        pool.install(|| {
            pack_jobs(
                &jobs,
                &mut parallel_vertices,
                &mut parallel_indices,
                0,
                0,
                2,
            )
        });
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&serial_vertices),
            bytemuck::cast_slice::<_, u8>(&parallel_vertices)
        );
        assert_eq!(serial_indices, parallel_indices);
        assert_eq!(parallel_indices[4000..4003], [2; 3]);
        assert_eq!(parallel_vertices[4000].color, [0.2, 0.4, 0.8, 0.5]);
    }
}
