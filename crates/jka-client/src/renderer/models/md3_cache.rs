//! Reuse unchanged MD3 packing and the corresponding resident stream ranges.
//! Draw order, shared buffers and all existing model shaders stay unchanged.
use crate::renderer::{scene, DynamicModelSurface, DynamicModelVertex};
use std::{collections::HashMap, ops::Range, sync::Arc};

const MAX_CACHE_BYTES: usize = 128 * 1024 * 1024;

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct Key {
    vertices: usize,
    indices: usize,
    vertex_offset: usize,
    index_offset: usize,
}

fn light_key(light: Option<scene::ClassicEntityLight>) -> Option<[u32; 15]> {
    light.map(|light| {
        let values = [light.ambient, light.directed, light.direction,
            light.sun_directed, light.sun_direction];
        std::array::from_fn(|i| values[i / 3][i % 3].to_bits())
    })
}

struct Entry {
    // Keep source allocations alive: pointer recycling must never produce a hit.
    _source_vertices: Arc<Vec<DynamicModelVertex>>,
    _source_indices: Arc<Vec<u32>>,
    light: Option<[u32; 15]>,
    vertices: Vec<DynamicModelVertex>,
    indices: Vec<u32>,
    seen: u64,
}

struct Pending {
    key: Key,
    source_vertices: Arc<Vec<DynamicModelVertex>>,
    source_indices: Arc<Vec<u32>>,
    light: Option<[u32; 15]>,
    vertices: Range<usize>,
    indices: Range<usize>,
}

#[derive(Default)]
pub(in crate::renderer) struct Md3StreamCache {
    entries: HashMap<Key, Entry>,
    pending: Vec<Pending>,
    generation: u64,
    pub vertex_hits: Vec<Range<usize>>,
    pub index_hits: Vec<Range<usize>>,
    pub hits: u32,
    pub misses: u32,
}

impl Md3StreamCache {
    pub fn clear(&mut self) {
        self.entries.clear();
        self.pending.clear();
        self.vertex_hits.clear();
        self.index_hits.clear();
    }

    pub fn begin_frame(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending.clear();
        self.vertex_hits.clear();
        self.index_hits.clear();
        self.hits = 0;
        self.misses = 0;
    }

    /// Hits contain the exact old CPU lighting result and rebased indices.
    /// Misses are packed by the ordinary serial/worker path, then retained once.
    pub fn append(
        &mut self,
        surface: &DynamicModelSurface,
        light: Option<scene::ClassicEntityLight>,
        vertices: &mut Vec<DynamicModelVertex>,
        indices: &mut Vec<u32>,
    ) -> bool {
        let key = Key {
            vertices: Arc::as_ptr(&surface.vertices) as usize,
            indices: Arc::as_ptr(&surface.indices) as usize,
            vertex_offset: vertices.len(),
            index_offset: indices.len(),
        };
        let light = light_key(light);
        if let Some(entry) = self.entries.get_mut(&key).filter(|entry| entry.light == light) {
            entry.seen = self.generation;
            vertices.extend_from_slice(&entry.vertices);
            indices.extend_from_slice(&entry.indices);
            self.vertex_hits.push(key.vertex_offset..vertices.len());
            self.index_hits.push(key.index_offset..indices.len());
            self.hits += 1;
            return true;
        }
        self.misses += 1;
        self.pending.push(Pending {
            key,
            source_vertices: Arc::clone(&surface.vertices),
            source_indices: Arc::clone(&surface.indices),
            light,
            vertices: key.vertex_offset..key.vertex_offset + surface.vertices.len(),
            indices: key.index_offset..key.index_offset + surface.indices.len(),
        });
        false
    }

    pub fn finish_frame(&mut self, vertices: &[DynamicModelVertex], indices: &[u32]) {
        // Keep only this frame's visible working set, including after PVS changes.
        self.entries.retain(|_, entry| entry.seen == self.generation);
        let mut bytes: usize = self.entries.values().map(|entry|
            std::mem::size_of_val(entry.vertices.as_slice()) + std::mem::size_of_val(entry.indices.as_slice())
        ).sum();
        for pending in self.pending.drain(..) {
            let required = pending.vertices.len() * std::mem::size_of::<DynamicModelVertex>()
                + pending.indices.len() * 4;
            if bytes.saturating_add(required) > MAX_CACHE_BYTES { continue; }
            bytes += required;
            self.entries.insert(pending.key, Entry {
                _source_vertices: pending.source_vertices,
                _source_indices: pending.source_indices,
                light: pending.light,
                vertices: vertices[pending.vertices].to_vec(),
                indices: indices[pending.indices].to_vec(),
                seen: self.generation,
            });
        }
    }
}

/// Complement of ordered, disjoint resident ranges. Adjacent dirty surfaces
/// become one upload. Callers can fall back to a full upload for fragmentation.
pub(super) fn dirty_spans<'a>(len: usize, resident: &'a [Range<usize>]) -> impl Iterator<Item = Range<usize>> + 'a {
    let mut cursor = 0;
    let mut next = 0;
    std::iter::from_fn(move || {
        while let Some(range) = resident.get(next) {
            next += 1;
            debug_assert!(range.start >= cursor && range.end <= len);
            let dirty = cursor..range.start;
            cursor = range.end;
            if !dirty.is_empty() { return Some(dirty); }
        }
        let dirty = cursor..len;
        cursor = len;
        (!dirty.is_empty()).then_some(dirty)
    })
}

pub(super) fn upload_stream<T: bytemuck::Pod>(
    queue: &wgpu::Queue,
    buffer: &wgpu::Buffer,
    values: &[T],
    resident: &[Range<usize>],
    buffer_grew: bool,
) -> u64 {
    // Cap queue submissions when moving geometry is interleaved with hits.
    // A recreated buffer has no resident data even when CPU packing hits.
    // One write covering every dirty span: each queue write is its own GPU copy
    // with a fixed cost (~9 us on a UHD 630) that dwarfs the bytes skipped by
    // uploading only the dirty spans separately.
    let stride = std::mem::size_of::<T>() as u64;
    let mut covering: Option<Range<usize>> = None;
    for span in upload_spans(values.len(), resident, buffer_grew) {
        covering = Some(match covering {
            Some(range) => range.start.min(span.start)..range.end.max(span.end),
            None => span,
        });
    }
    let Some(span) = covering else { return 0 };
    let bytes = bytemuck::cast_slice(&values[span.clone()]);
    queue.write_buffer(buffer, span.start as u64 * stride, bytes);
    bytes.len() as u64
}

fn upload_spans<'a>(len: usize, resident: &'a [Range<usize>], buffer_grew: bool) -> impl Iterator<Item = Range<usize>> + 'a {
    let full = buffer_grew || dirty_spans(len, resident).take(33).count() > 32;
    dirty_spans(len, if full { &[] } else { resident })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::{DynamicModelAlphaMode, DynamicWireframeClass, ModelGeometrySource};
    use crate::renderer::models::prepare::{pack_jobs, CpuPackJob};

    fn surface(count: usize) -> DynamicModelSurface {
        DynamicModelSurface {
            geometry_source: ModelGeometrySource::Md3,
            authored_lighting: false,
            material_trace: None,
            entity_num: 3,
            wireframe_class: DynamicWireframeClass::Entity,
            raster_visible: true,
            vertices: Arc::new((0..count).map(|i| DynamicModelVertex { foliage_wind: 0.0,
                position: [i as f32, 2.0, 3.0], normal: [0.0, 2.0, 0.0],
                uv: [0.2, 0.8], color: [0.8, 0.6, 0.4, 0.3], depth_hack: 1.0,
                water_wetness: 0.0,
            }).collect()),
            indices: Arc::new(vec![0, 2, 1]),
            lighting_origin: Some([0.0; 3]),
            rt_rigid: None, rt_skinned_key: None, ghoul2_gpu: None,
            fx_gpu_sprites: None, authored_glow: false, texture: None,
            alpha_mode: DynamicModelAlphaMode::Blend,
        }
    }

    fn light() -> scene::ClassicEntityLight {
        scene::ClassicEntityLight {
            ambient: [30.0, 50.0, 70.0], directed: [50.0, 70.0, 90.0],
            direction: [0.0, 1.0, 0.0], sun_directed: [10.0, 20.0, 30.0],
            sun_direction: [0.0, 0.8, 0.6], ..Default::default()
        }
    }

    fn frame(cache: &mut Md3StreamCache, surfaces: &[&DynamicModelSurface],
        light: Option<scene::ClassicEntityLight>, prefix: usize,
    ) -> (Vec<DynamicModelVertex>, Vec<u32>) {
        cache.begin_frame();
        let mut vertices = vec![bytemuck::Zeroable::zeroed(); prefix];
        let mut indices = vec![0; prefix];
        let mut jobs = Vec::new();
        for surface in surfaces {
            let v = vertices.len();
            let i = indices.len();
            if !cache.append(surface, light, &mut vertices, &mut indices) {
                vertices.extend_from_slice(&surface.vertices);
                indices.extend_from_slice(&surface.indices);
                jobs.push(CpuPackJob { vertices: v..vertices.len(), indices: i..indices.len(), light, water: None });
            }
        }
        // Also verifies worker packing leaves cached gaps untouched.
        pack_jobs(&jobs, &mut vertices, &mut indices, 0, 0, 2);
        cache.finish_frame(&vertices, &indices);
        (vertices, indices)
    }

    fn assert_vertices(a: &[DynamicModelVertex], b: &[DynamicModelVertex]) {
        assert_eq!(bytemuck::cast_slice::<_, u8>(a), bytemuck::cast_slice::<_, u8>(b));
    }

    #[test]
    fn md3_cache_warm_hits_preserve_lit_vertices_indices_and_skip_resident_uploads() {
        let mesh = surface(3);
        let mut cache = Md3StreamCache::default();
        let cold = frame(&mut cache, &[&mesh], Some(light()), 0);
        assert_eq!((cache.hits, cache.misses), (0, 1));
        let warm = frame(&mut cache, &[&mesh], Some(light()), 0);
        assert_eq!((cache.hits, cache.misses), (1, 0));
        assert_vertices(&cold.0, &warm.0);
        assert_eq!(cold.1, warm.1);
        assert_eq!(warm.0[0].depth_hack, 1.0);
        assert_eq!(warm.0[0].color[3], 0.3);
        assert_eq!(upload_spans(warm.0.len(), &cache.vertex_hits, false).count(), 0);
        assert_eq!(upload_spans(warm.1.len(), &cache.index_hits, false).count(), 0);
        assert_eq!(upload_spans(warm.0.len(), &cache.vertex_hits, true).collect::<Vec<_>>(), vec![0..3]);
        cache.clear();
        frame(&mut cache, &[&mesh], Some(light()), 0);
        assert_eq!((cache.hits, cache.misses), (0, 1));
    }

    #[test]
    fn md3_cache_invalidates_changed_lighting_sources_offsets_and_visibility() {
        let mut mesh = surface(3);
        let mut cache = Md3StreamCache::default();
        frame(&mut cache, &[&mesh], Some(light()), 0);
        for variant in 0..6 {
            let mut changed = light();
            match variant {
                0 => changed.ambient[0] += 1.0,
                1 => changed.directed[0] += 1.0,
                2 => changed.direction[1] = 0.5,
                3 => changed.sun_directed[1] += 1.0,
                4 => changed.sun_direction[1] = 0.2,
                _ => {},
            }
            let changed = (variant != 5).then_some(changed);
            let packed = frame(&mut cache, &[&mesh], changed, 0);
            assert_eq!(cache.misses, 1);
            let reference = frame(&mut Md3StreamCache::default(), &[&mesh], changed, 0);
            assert_vertices(&packed.0, &reference.0);
            assert_eq!(packed.1, reference.1);
        }
        Arc::make_mut(&mut mesh.vertices)[0].position[0] = 42.0;
        let moved = frame(&mut cache, &[&mesh], None, 0);
        assert_eq!(cache.misses, 1);
        assert_eq!(moved.0[0].position[0], 42.0);
        Arc::make_mut(&mut mesh.indices).swap(1, 2);
        frame(&mut cache, &[&mesh], None, 0);
        assert_eq!(cache.misses, 1);
        let shifted = frame(&mut cache, &[&mesh], None, 3);
        assert_eq!(cache.misses, 1);
        assert_eq!(shifted.1, [0, 0, 0, 3, 4, 5]);
        frame(&mut cache, &[], None, 0);
        assert!(cache.entries.is_empty());
        frame(&mut cache, &[&mesh], None, 3);
        assert_eq!(cache.misses, 1);
    }

    #[test]
    fn md3_cache_mixed_worker_misses_and_dirty_uploads_match_full_repacking() {
        let a = surface(4500);
        let mut b = surface(4500);
        let c = surface(4500);
        let mut cache = Md3StreamCache::default();
        frame(&mut cache, &[&a, &b, &c], Some(light()), 3);
        let mut gpu_vertices = frame(&mut cache, &[&a, &b, &c], Some(light()), 3).0;
        Arc::make_mut(&mut b.vertices)[3].color[0] = 0.2;
        let packed = frame(&mut cache, &[&a, &b, &c], Some(light()), 3);
        assert_eq!((cache.hits, cache.misses), (2, 1));
        let reference = frame(&mut Md3StreamCache::default(), &[&a, &b, &c], Some(light()), 3);
        assert_vertices(&packed.0, &reference.0);
        assert_eq!(packed.1, reference.1);
        for span in upload_spans(packed.0.len(), &cache.vertex_hits, false) {
            gpu_vertices[span.clone()].copy_from_slice(&packed.0[span]);
        }
        assert_vertices(&gpu_vertices, &reference.0);
        assert_eq!(dirty_spans(10, &[2..4, 4..6, 8..10]).collect::<Vec<_>>(), vec![0..2, 6..8]);
        let fragmented = (0..35).map(|i| i*2+1..i*2+2).collect::<Vec<_>>();
        assert_eq!(upload_spans(70, &fragmented, false).collect::<Vec<_>>(), vec![0..70]);
        assert_eq!(upload_spans(0, &[], true).count(), 0);
    }
}
