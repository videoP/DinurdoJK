//! Pipeline layout for the preserved Blue Water surface shader.
//! Bind groups 0, 1 and 3 are supplied by the JKA render bridge; group 2 is
//! `SurfaceWaves`. The upstream files stay unchanged; the JKA variant adds a
//! fragment admission test for the host's footprint and PVS boundaries.
use crate::{geometry, shader_composer, surface_waves::SurfaceWaves};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

pub(crate) fn uniform(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

pub(crate) fn storage(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

pub(crate) fn sampled(
    binding: u32,
    visibility: wgpu::ShaderStages,
    dimension: wgpu::TextureViewDimension,
    depth: bool,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type: if depth {
                wgpu::TextureSampleType::Depth
            } else {
                wgpu::TextureSampleType::Float { filterable: true }
            },
            view_dimension: dimension,
            multisampled: false,
        },
        count: None,
    }
}

pub(crate) fn roof_sampled(visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    let mut entry = sampled(22, visibility, wgpu::TextureViewDimension::D2, false);
    entry.ty = wgpu::BindingType::Texture {
        sample_type: wgpu::TextureSampleType::Float { filterable: false },
        view_dimension: wgpu::TextureViewDimension::D2,
        multisampled: false,
    };
    entry
}

pub(crate) fn sampler(
    binding: u32,
    visibility: wgpu::ShaderStages,
    comparison: bool,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Sampler(if comparison {
            wgpu::SamplerBindingType::Comparison
        } else {
            wgpu::SamplerBindingType::Filtering
        }),
        count: None,
    }
}

/// `r_oceanPixelDetail` 5's tile classifier: one 8x8 workgroup per 16x16-pixel tile over the reduced shading.
fn adaptive_classifier(device: &wgpu::Device) -> (wgpu::ComputePipeline, wgpu::BindGroupLayout) {
    let compute = wgpu::ShaderStages::COMPUTE;
    let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry {
        binding, visibility: compute,
        ty: wgpu::BindingType::Texture {
            sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false,
        }, count: None,
    };
    let buffer = |binding, ty| wgpu::BindGroupLayoutEntry {
        binding, visibility: compute,
        ty: wgpu::BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Blue Water adaptive tiles"),
        entries: &[
            texture(0, wgpu::TextureSampleType::Float { filterable: true }),
            texture(1, wgpu::TextureSampleType::Depth),
            buffer(2, wgpu::BufferBindingType::Storage { read_only: false }),
            buffer(3, wgpu::BufferBindingType::Uniform),
        ],
    });
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Blue Water adaptive tiles"),
        source: wgpu::ShaderSource::Wgsl(shader_composer::ADAPTIVE_TILES_WGSL.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Blue Water adaptive tiles"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("Blue Water adaptive tiles"),
        layout: Some(&pipeline_layout),
        module: &module,
        entry_point: Some("classify"),
        compilation_options: Default::default(),
        cache: None,
    });
    (pipeline, layout)
}

pub struct OriginalSurfacePipeline {
    pub camera_layout: wgpu::BindGroupLayout,
    pub surface_layout: wgpu::BindGroupLayout,
    pub environment_layout: wgpu::BindGroupLayout,
    pub pipeline: wgpu::RenderPipeline,
    pub reduced_pipeline: Option<wgpu::RenderPipeline>,
    pub near_pipeline: Option<wgpu::RenderPipeline>,
    pub reconstruct_pipeline: Option<wgpu::RenderPipeline>,
    pub reconstruct_layout: Option<wgpu::BindGroupLayout>,
    /// `r_oceanPixelDetail` 5: the tile classifier (`shader_composer::ADAPTIVE_TILES_WGSL`) and its bind group layout
    /// (reduced color, reduced depth, tile buffer, parameters).
    pub adaptive_classify: Option<(wgpu::ComputePipeline, wgpu::BindGroupLayout)>,
    /// `r_oceanFarShader`: the expensive material's twin for water past the sky walls (`with_far_variant`), drawn over
    /// the pruned mesh's far range: of `pipeline` in full-resolution mode, of `reduced_pipeline` otherwise.
    pub far_pipeline: Option<wgpu::RenderPipeline>,
    pub wireframe: Option<wgpu::RenderPipeline>,
    pub waterline_marker: Option<wgpu::RenderPipeline>,
    // Keep the existing compiled vertex entry to compile the debug overlay
    // lazily; no wireframe PSO when r_showtris ocean is disabled.
    vertex_shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
}

/// Cell classes for `OriginalClipmap::pruned_indices_classified`.
pub const CELL_REJECTED: u8 = 0;
pub const CELL_NEAR: u8 = 1;
pub const CELL_FAR: u8 = 2;

/// Preserved High-tier clipmap vertex and index data. JKA finite pools omit
/// the upstream horizon/skirt: its exterior belongs to JKA's sky extension
/// gate, not to an unbounded sea in every BSP room.
pub struct OriginalClipmap {
    clipmap: geometry::Clipmap,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    fills: Vec<std::ops::Range<u32>>,
    rings: Vec<u32>,
    horizon_start: u32,
    index_count: u32,
    // Ring index variants for stride 1, 2, 4, 8, 16; all preserve both
    // original inner/outer edge vertices to stitch without T-junctions.
    ring_lods: std::sync::Arc<[[std::ops::Range<u32>; 5]]>,
    // Independent centre-fill LODs; without these the nearest flat-water
    // rectangle would remain fully tessellated even as its rings simplify.
    fill_lods: std::sync::Arc<[[std::ops::Range<u32>; 5]]>,
    // CPU copy of upstream lattice/index variants for footprint/PVS pruning.
    // The GPU vertex buffer and its wave displacement shader stay unchanged.
    // Shared so `PruneSource` can prune on a worker thread.
    source_vertices: std::sync::Arc<[geometry::OceanVertex]>,
    source_indices: std::sync::Arc<[u32]>,
    prune_capacity: usize,
}

/// CPU-only view of the clipmap's index variants for footprint/PVS pruning.
/// Cloning shares the source buffers, so a prune can run on a worker thread.
#[derive(Clone)]
pub struct PruneSource {
    levels: usize,
    segments: u32,
    horizon_start: u32,
    index_count: u32,
    prune_capacity: usize,
    ring_lods: std::sync::Arc<[[std::ops::Range<u32>; 5]]>,
    fill_lods: std::sync::Arc<[[std::ops::Range<u32>; 5]]>,
    source_vertices: std::sync::Arc<[geometry::OceanVertex]>,
    source_indices: std::sync::Arc<[u32]>,
}

/// One pruned segment (the centre fill or one ring variant) and its inputs.
struct PrunedSegment {
    context: u64,
    range: std::ops::Range<u32>,
    extra_pad: u32,
    /// The segment's vertices read `frames[frame_base..frame_base + frames.len()]`.
    frame_base: usize,
    frames: Vec<[u32; 4]>,
    near: Vec<u32>,
    far: Vec<u32>,
}

impl PrunedSegment {
    fn matches(&self, context: u64, range: &std::ops::Range<u32>, extra_pad: f32, frames: &[[f32; 4]; 10]) -> bool {
        self.context == context
            && self.range == *range
            && self.extra_pad == extra_pad.to_bits()
            && frames.get(self.frame_base..self.frame_base + self.frames.len()).is_some_and(|current| {
                current.iter().zip(&self.frames).all(|(frame, bits)| frame.map(f32::to_bits) == *bits)
            })
    }
}

/// Memo for `PruneSource::pruned_indices_classified_cached`. A camera move
/// that re-snaps one clipmap level re-prunes only the segments whose vertices
/// read that level's frame; the rest are reused verbatim. `context` names the
/// admission callback: callers change it whenever `classify` could answer
/// differently for the same cell.
#[derive(Default)]
pub struct PruneSegmentCache {
    segments: Vec<Option<PrunedSegment>>,
}

impl PruneSource {
    /// `OriginalClipmap::pruned_indices_classified`, reusing every segment in
    /// `cache` whose frames, LOD variant, padding and `context` are unchanged.
    /// The result is identical to an uncached prune. Also returns how many
    /// segments were re-pruned.
    #[allow(clippy::too_many_arguments)]
    pub fn pruned_indices_classified_cached(
        &self, cache: &mut PruneSegmentCache, context: u64,
        first: usize, outer: usize, steps: &[u8; 10], fill_step: u8,
        horizon: bool, horizon_far: bool, frames: &[[f32; 4]; 10], extra_pad: f32,
        mut classify: impl FnMut([f32; 2], [f32; 2]) -> u8,
    ) -> (Vec<u32>, u32, usize) {
        let first = first.min(self.levels - 1);
        let last = if horizon { self.levels - 1 }
            else { outer.max(first).min(self.levels - 1) };
        if cache.segments.len() < self.levels + 1 {
            cache.segments.resize_with(self.levels + 1, || None);
        }
        // Slot 0 is the centre fill, slot 1 + level that level's ring.
        let fill_variant = fill_step.clamp(1, 16).ilog2().min(4) as usize;
        let mut wanted = Vec::with_capacity(self.levels + 1);
        wanted.push((0, self.fill_lods[first][fill_variant].clone(), fill_variant == 0));
        for level in first..=last {
            let variant = steps[level].clamp(1, 16).ilog2().min(4) as usize;
            wanted.push((1 + level, self.ring_lods[level][variant].clone(), variant == 0));
        }
        let mut rebuilt = 0;
        for (slot, range, original_quads) in &wanted {
            let entry = &mut cache.segments[*slot];
            if !entry.as_ref().is_some_and(|segment| segment.matches(context, range, extra_pad, frames)) {
                *entry = Some(self.prune_segment(context, range, *original_quads, frames, extra_pad, &mut classify));
                rebuilt += 1;
            }
        }
        let segments = || wanted.iter().filter_map(|(slot, ..)| cache.segments[*slot].as_ref());
        // Horizon/skirt is valid only under the sky-contact gate; the finite
        // footprint callback cannot bound its far-plane vertices conservatively.
        let skirt = &self.source_indices[self.horizon_start as usize..self.index_count as usize];
        let total = segments().map(|segment| segment.near.len() + segment.far.len()).sum::<usize>()
            + if horizon { skirt.len() } else { 0 };
        let mut out = Vec::with_capacity(total);
        for segment in segments() {
            out.extend_from_slice(&segment.near);
        }
        if horizon && !horizon_far {
            out.extend_from_slice(skirt);
        }
        let far_start = out.len() as u32;
        for segment in segments() {
            out.extend_from_slice(&segment.far);
        }
        if horizon && horizon_far {
            out.extend_from_slice(skirt);
        }
        (out, far_start, rebuilt)
    }

    fn prune_segment(
        &self, context: u64, range: &std::ops::Range<u32>, original_quads: bool,
        frames: &[[f32; 4]; 10], extra_pad: f32,
        classify: &mut impl FnMut([f32; 2], [f32; 2]) -> u8,
    ) -> PrunedSegment {
        // CPU positions match the upstream snapped frame and inner-edge shift.
        // The shader's continuous 0..1 geomorph moves vertices by at most one
        // cell, which is covered by the conservative bounds expansion below.
        let levels = self.levels;
        let segments = self.segments as f32;
        let position = |vertex: &geometry::OceanVertex| {
            let level = (vertex.level[0] as usize).min(levels - 1);
            let frame = frames[level];
            let inner = level > 0 && vertex.position.iter().all(|&v|
                v.abs() <= segments * frame[2] / 4.0 + 1.0e-3);
            let centre = if inner { frames[level - 1] } else { frame };
            [centre[0] + vertex.position[0], centre[1] + vertex.position[1]]
        };
        // The upstream fill and stride-1 rings are six indices per cell.
        // Godot admits the entire cell conservatively; test once per
        // quad, instead of separately discarding half of a wet quad.
        // Stitched outer LOD variants contain triangle fans, so their
        // groups must instead be examined triangle-by-triangle.
        let stride = if original_quads { 6 } else { 3 };
        let (mut near, mut far) = (Vec::new(), Vec::new());
        let (mut lowest, mut highest) = (usize::MAX, 0);
        for indices in self.source_indices[range.start as usize..range.end as usize].chunks_exact(stride) {
            let mut lo = [f32::INFINITY; 2];
            let mut hi = [f32::NEG_INFINITY; 2];
            for &vertex_index in indices {
                let vertex = &self.source_vertices[vertex_index as usize];
                let level = (vertex.level[0] as usize).min(levels - 1);
                lowest = lowest.min(level);
                highest = highest.max(level);
                let point = position(vertex);
                for axis in 0..2 {
                    lo[axis] = lo[axis].min(point[axis]);
                    hi[axis] = hi[axis].max(point[axis]);
                }
            }
            let level = (self.source_vertices[indices[0] as usize].level[0] as usize).min(levels - 1);
            let pad = frames[level][2] * 1.05 + extra_pad;
            for axis in 0..2 { lo[axis] -= pad; hi[axis] += pad; }
            match classify(lo, hi) {
                CELL_NEAR => near.extend_from_slice(indices),
                CELL_FAR => far.extend_from_slice(indices),
                _ => {}
            }
        }
        // Inner vertices also read the next finer level's frame.
        let (frame_base, frame_end) = if lowest == usize::MAX { (0, 0) } else { (lowest.saturating_sub(1), highest + 1) };
        PrunedSegment {
            context,
            range: range.clone(),
            extra_pad: extra_pad.to_bits(),
            frame_base,
            frames: frames[frame_base..frame_end].iter().map(|frame| frame.map(f32::to_bits)).collect(),
            near,
            far,
        }
    }

    /// Maximum indices across the active fill, per-ring LOD variants and sky horizon.
    pub fn pruned_index_capacity(&self) -> usize { self.prune_capacity }
}

/// Material values in the original surface uniform. Colors are linear RGB.
/// Scene-specific colors can be supplied by JKA's promoted water appearance.
#[derive(Clone, Copy, Debug)]
pub struct SurfaceAppearance {
    pub absorption: [f32; 3],
    pub transmission: [f32; 3],
    pub pigment: [f32; 3],
    pub crest_foam: [f32; 4],
    pub surface_foam: [f32; 4],
    pub shoreline_foam: [f32; 4],
    pub wind_stretch: f32,
    pub windrow_coverage: f32,
    pub foam_streak_mean: f32,
    pub rain: f32,
}

impl Default for SurfaceAppearance {
    fn default() -> Self {
        // Original calibration::NORTH_ATLANTIC / scene_parameters, linear RGB.
        // The temporary bridge's cyan tint and clear-blue coefficients were
        // not the upstream optical calibration, especially below the surface.
        let linear = |hex: u32| [16, 8, 0].map(|shift| {
            let srgb = ((hex >> shift) & 255) as f32 / 255.0;
            if srgb <= 0.04045 { srgb / 12.92 } else { ((srgb + 0.055) / 1.055).powf(2.4) }
        });
        Self {
            absorption: linear(0x945b57),
            transmission: linear(0x49575e),
            pigment: linear(0x2d373c),
            crest_foam: [1.0, 1.0, 1.0, 1.0],
            surface_foam: [1.0, 1.0, 1.0, 0.6],
            shoreline_foam: [0.85, 0.94, 0.98, 0.8],
            wind_stretch: 0.5,
            windrow_coverage: 0.2,
            foam_streak_mean: 0.5,
            rain: 0.0,
        }
    }
}

/// Reindex the *existing* upstream grid, without moving vertices or changing
/// the shader's clipmap frame/morph logic. The annulus is four non-overlapping
/// rectangles from upstream geometry::build: north/south, east/west.
///
/// Each interior block gets a coarse centre fan. At the annulus borders (both
/// the hole and the exterior) ALL original grid-edge vertices are included,
/// avoiding T-junctions against the independently snapped neighbouring ring.
/// This is built once per mesh quality, not uploaded per frame.
fn append_stitched_ring(indices: &mut Vec<u32>, segments: u32, level: usize, stride: u32) {
    let half = (segments / 2) as i32;
    let hole = (segments / 4) as i32;
    let step = stride as i32;
    debug_assert!(stride.is_power_of_two() && stride <= segments / 4);
    let per_level = (segments + 1) * (segments + 1);
    let at = |x: i32, y: i32| -> u32 {
        level as u32 * per_level + ((y + half) * (segments as i32 + 1) + x + half) as u32
    };
    // Squares completely in the ring, with the hole excluded exactly as in
    // geometry::build. Rectangle boundaries coincide on the same lattice.
    let rects = [
        (-half, half, -half, -hole),
        (-half, half, hole, half),
        (-half, -hole, -hole, hole),
        (hole, half, -hole, hole),
    ];
    for (left, right, bottom, top) in rects {
        for y0 in (bottom..top).step_by(stride as usize) {
            for x0 in (left..right).step_by(stride as usize) {
                let x1 = x0 + step;
                let y1 = y0 + step;
                // An edge touching the true hole or exterior must include
                // every source grid vertex. Adjacent rectangles use identical
                // strides at their internal borders.
                let fine_left = x0 == -half || (x0 == hole && y0 >= -hole && y1 <= hole);
                let fine_right = x1 == half || (x1 == -hole && y0 >= -hole && y1 <= hole);
                let fine_bottom = y0 == -half || (y0 == hole && x0 >= -hole && x1 <= hole);
                let fine_top = y1 == half || (y1 == -hole && x0 >= -hole && x1 <= hole);
                // Traverse clockwise viewed from above, matching the upstream
                // OceanVertex winding (geometry::build's triangle closure).
                // At stride 16 the perimeter has at most 64 samples.
                // No per-block heap allocation during clipmap construction.
                let mut edge = [0u32; 64];
                let mut count = 0usize;
                let mut push = |vertex| { edge[count] = vertex; count += 1; };
                push(at(x0, y0));
                let delta = if fine_left { 1 } else { step };
                for y in (y0 + delta..=y1).step_by(delta as usize) { push(at(x0, y)); }
                let delta = if fine_top { 1 } else { step };
                for x in (x0 + delta..=x1).step_by(delta as usize) { push(at(x, y1)); }
                let delta = if fine_right { 1 } else { step };
                for y in (y0..y1).rev() {
                    // start at y1 - delta, not one below y1 when decimated
                    if (y1 - y) % delta == 0 { push(at(x1, y)); }
                }
                let delta = if fine_bottom { 1 } else { step };
                for x in (x0..x1).rev() {
                    if (x1 - x) % delta == 0 && x > x0 { push(at(x, y0)); }
                }
                // The fan apex is an EXISTING interior lattice vertex;
                // no extra vertex attributes or GPU pipeline changes.
                drop(push);
                let centre = at(x0 + step / 2, y0 + step / 2);
                for i in 0..count {
                    indices.extend_from_slice(&[centre, edge[i], edge[(i + 1) % count]]);
                }
            }
        }
    }
}

/// A decimated centre fill with every boundary lattice vertex retained.
/// The fan/edge winding matches append_stitched_ring. As with the rings,
/// this retains the *original* vertices and only substitutes indices.
fn append_stitched_fill(indices: &mut Vec<u32>, segments: u32, level: usize, stride: u32) {
    let half = (segments / 4) as i32;
    let step = stride as i32;
    debug_assert!(stride.is_power_of_two() && stride >= 2 && stride <= segments / 4);
    let per_level = (segments + 1) * (segments + 1);
    let at = |x: i32, y: i32| -> u32 {
        level as u32 * per_level + ((y + segments as i32 / 2) * (segments as i32 + 1)
            + x + segments as i32 / 2) as u32
    };
    for y0 in (-half..half).step_by(stride as usize) {
        for x0 in (-half..half).step_by(stride as usize) {
            let x1 = x0 + step;
            let y1 = y0 + step;
            let mut edge = [0u32; 64];
            let mut count = 0;
            let mut push = |v| { edge[count] = v; count += 1; };
            push(at(x0, y0));
            let delta = if x0 == -half { 1 } else { step };
            for y in (y0 + delta..=y1).step_by(delta as usize) { push(at(x0, y)); }
            let delta = if y1 == half { 1 } else { step };
            for x in (x0 + delta..=x1).step_by(delta as usize) { push(at(x, y1)); }
            let delta = if x1 == half { 1 } else { step };
            for y in (y0..y1).rev() {
                if (y1 - y) % delta == 0 { push(at(x1, y)); }
            }
            let delta = if y0 == -half { 1 } else { step };
            for x in (x0..x1).rev() {
                if (x1 - x) % delta == 0 && x > x0 { push(at(x, y0)); }
            }
            drop(push);
            let centre = at(x0 + step / 2, y0 + step / 2);
            for i in 0..count {
                indices.extend_from_slice(&[centre, edge[i], edge[(i + 1) % count]]);
            }
        }
    }
}

impl OriginalClipmap {
    pub fn new(device: &wgpu::Device, segments: u32, levels: usize) -> Self {
        let clipmap = geometry::Clipmap::new(segments, levels);
        let mesh = geometry::build(clipmap, 512.0);
        let outer_ring_cells = clipmap.segments * clipmap.segments * 3 / 4;
        let horizon_start = mesh.rings[clipmap.levels - 1] + outer_ring_cells * 6;
        let index_count = mesh.indices.len() as u32;
        let mut indices = mesh.indices.clone();
        let ring_lods: Vec<[std::ops::Range<u32>; 5]> = (0..clipmap.levels).map(|level| {
            let base_start = mesh.rings[level];
            let base_end = mesh.rings.get(level + 1).copied().unwrap_or(horizon_start);
            let mut ranges: [std::ops::Range<u32>; 5] = std::array::from_fn(|_| base_start..base_end);
            for (variant, range) in ranges.iter_mut().enumerate().skip(1) {
                let start = indices.len() as u32;
                append_stitched_ring(&mut indices, clipmap.segments, level,
                    (1u32 << variant).min(clipmap.segments / 4));
                *range = start..indices.len() as u32;
            }
            ranges
        }).collect();
        let fill_lods: Vec<[std::ops::Range<u32>; 5]> = (0..clipmap.levels).map(|level| {
            let base = mesh.fills[level].clone();
            let mut variants: [std::ops::Range<u32>; 5] = std::array::from_fn(|_| base.clone());
            for (variant, range) in variants.iter_mut().enumerate().skip(1) {
                let start = indices.len() as u32;
                append_stitched_fill(&mut indices, clipmap.segments, level,
                    (1u32 << variant).min(clipmap.segments / 4));
                *range = start..indices.len() as u32;
            }
            variants
        }).collect();
        let prune_capacity = (0..clipmap.levels).map(|level| {
            ring_lods[level].iter().map(|r| (r.end - r.start) as usize).max().unwrap_or(0)
        }).sum::<usize>() + fill_lods.iter().flat_map(|variants| variants.iter())
            .map(|r| (r.end - r.start) as usize).max().unwrap_or(0)
            + (index_count - horizon_start) as usize;
        let source_indices = indices.clone();
        let source_vertices = mesh.vertices.clone();
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Blue Water original clipmap vertices"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Blue Water original clipmap indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        Self {
            clipmap,
            vertices,
            indices,
            fills: mesh.fills,
            rings: mesh.rings,
            horizon_start,
            index_count,
            ring_lods: ring_lods.into(),
            fill_lods: fill_lods.into(),
            source_indices: source_indices.into(),
            source_vertices: source_vertices.into(),
            prune_capacity,
        }
    }

    /// Shared CPU data for pruning off the render thread.
    pub fn prune_source(&self) -> PruneSource {
        PruneSource {
            levels: self.clipmap.levels,
            segments: self.clipmap.segments,
            horizon_start: self.horizon_start,
            index_count: self.index_count,
            prune_capacity: self.prune_capacity,
            ring_lods: self.ring_lods.clone(),
            fill_lods: self.fill_lods.clone(),
            source_vertices: self.source_vertices.clone(),
            source_indices: self.source_indices.clone(),
        }
    }

    /// Maximum indices across the active fill, per-ring LOD variants and sky horizon.
    pub fn pruned_index_capacity(&self) -> usize { self.prune_capacity }

    /// Prune the original Blue Water indices with the SAME conservative cell
    /// admission callback used by Godot. The submitted triangles are genuinely
    /// removed; fragment masking remains solely for shoreline precision.
    ///
    /// CPU positions match the upstream snapped frame and inner-edge shift.
    /// `extra_pad` (metres) widens every test for callers that pass frames
    /// quantized coarser than the real snap, so one prune stays valid over a
    /// longer camera move.
    /// Pad by the maximum one-cell geomorph to avoid losing triangles when
    /// `detail.morph` changes between lattice crossings (no per-frame rebuild).
    pub fn pruned_indices(
        &self,
        first: usize, outer: usize, steps: &[u8; 10], horizon: bool,
        frames: &[[f32; 4]; 10],
        extra_pad: f32,
        admit: impl FnMut([f32; 2], [f32; 2]) -> bool,
    ) -> Vec<u32> {
        self.pruned_indices_with_fill(first, outer, steps, 1, horizon, frames, extra_pad, admit)
    }

    /// Curvature-aware geometry option: individually decimate the centre fill
    /// as well as the annular rings, without changing the upstream vertex shader.
    pub fn pruned_indices_with_fill(
        &self,
        first: usize, outer: usize, steps: &[u8; 10], fill_step: u8,
        horizon: bool, frames: &[[f32; 4]; 10], extra_pad: f32,
        mut admit: impl FnMut([f32; 2], [f32; 2]) -> bool,
    ) -> Vec<u32> {
        self.pruned_indices_classified(first, outer, steps, fill_step, horizon, false, frames, extra_pad,
            |lo, hi| if admit(lo, hi) { CELL_NEAR } else { CELL_REJECTED }).0
    }

    /// `pruned_indices_with_fill` with each admitted cell classed near or far (`classify` returns a `CELL_*`): the
    /// near cells first, then the far ones (and the horizon/skirt when `horizon_far`), and the index where the far
    /// part starts. With no far cells the indices are exactly `pruned_indices_with_fill`'s.
    #[allow(clippy::too_many_arguments)]
    pub fn pruned_indices_classified(
        &self,
        first: usize, outer: usize, steps: &[u8; 10], fill_step: u8,
        horizon: bool, horizon_far: bool, frames: &[[f32; 4]; 10], extra_pad: f32,
        classify: impl FnMut([f32; 2], [f32; 2]) -> u8,
    ) -> (Vec<u32>, u32) {
        let mut cache = PruneSegmentCache::default();
        let (indices, far_start, _) = self.prune_source().pruned_indices_classified_cached(
            &mut cache, 0, first, outer, steps, fill_step, horizon, horizon_far, frames, extra_pad, classify);
        (indices, far_start)
    }

    pub fn clipmap(&self) -> geometry::Clipmap {
        self.clipmap
    }

    /// Draw the finest needed middle and rings through `outer_level`. The
    /// caller must also bind the exact JKA footprint/PVS fragment mask.
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, first: usize, outer_level: usize) {
        let first = first.min(self.clipmap.levels - 1);
        let last = outer_level.max(first).min(self.clipmap.levels - 1);
        let end = self
            .rings
            .get(last + 1)
            .copied()
            .unwrap_or(self.horizon_start);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(self.fills[first].clone(), 0, 0..1);
        pass.draw_indexed(self.rings[first]..end, 0, 0..1);
    }

    /// Use the original vertex shader and frames verbatim; only ring *interior*
    /// indices become sparse. Both ring borders retain every original vertex,
    /// so separate LOD levels meet exactly as before, including odd-vertex
    /// morphing at the outer edge. The already allocated variants are immutable.
    pub fn draw_lod<'a>(
        &'a self, pass: &mut wgpu::RenderPass<'a>, first: usize,
        outer_level: usize, ring_steps: &[u8; 10], horizon: bool,
    ) {
        let first = first.min(self.clipmap.levels - 1);
        let last = if horizon { self.clipmap.levels - 1 }
            else { outer_level.max(first).min(self.clipmap.levels - 1) };
        if (first..=last).all(|level| ring_steps[level] == 1) {
            if horizon { self.draw_horizon(pass, first); }
            else { self.draw(pass, first, last); }
            return;
        }
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(self.fills[first].clone(), 0, 0..1);
        for level in first..=last {
            let variant = ring_steps[level].clamp(1, 16).ilog2().min(4) as usize;
            pass.draw_indexed(self.ring_lods[level][variant].clone(), 0, 0..1);
        }
        if horizon {
            pass.draw_indexed(self.horizon_start..self.index_count, 0, 0..1);
        }
    }

    /// Draw the exact selected pool indices for all Blue Water passes.
    pub fn draw_pruned<'a>(
        &'a self, pass: &mut wgpu::RenderPass<'a>,
        index_buffer: &'a wgpu::Buffer, index_count: u32,
        local_vertices: Option<&'a wgpu::Buffer>,
    ) {
        self.draw_pruned_range(pass, index_buffer, 0..index_count, local_vertices);
    }

    /// `draw_pruned` over `range` of the pruned indices (the near or far part of `pruned_indices_classified`).
    pub fn draw_pruned_range<'a>(
        &'a self, pass: &mut wgpu::RenderPass<'a>,
        index_buffer: &'a wgpu::Buffer, range: std::ops::Range<u32>,
        local_vertices: Option<&'a wgpu::Buffer>,
    ) {
        if range.is_empty() { return; }
        pass.set_vertex_buffer(0, local_vertices.unwrap_or(&self.vertices).slice(..));
        pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(range, 0, 0..1);
    }

    pub fn triangle_count_lod(
        &self, first: usize, outer_level: usize, ring_steps: &[u8; 10], horizon: bool,
    ) -> u64 {
        let first = first.min(self.clipmap.levels - 1);
        let last = if horizon { self.clipmap.levels - 1 }
            else { outer_level.max(first).min(self.clipmap.levels - 1) };
        let mut indices = self.fills[first].end - self.fills[first].start;
        for level in first..=last {
            let variant = ring_steps[level].clamp(1, 16).ilog2().min(4) as usize;
            let range = &self.ring_lods[level][variant];
            indices += range.end - range.start;
        }
        if horizon { indices += self.index_count - self.horizon_start; }
        u64::from(indices) / 3
    }

    pub fn triangle_count(&self, first: usize, outer_level: usize) -> u64 {
        let first = first.min(self.clipmap.levels - 1);
        let last = outer_level.max(first).min(self.clipmap.levels - 1);
        let end = self
            .rings
            .get(last + 1)
            .copied()
            .unwrap_or(self.horizon_start);
        u64::from(self.fills[first].end - self.fills[first].start + end - self.rings[first]) / 3
    }

    /// The original horizon and skirt are admitted only for a pool whose JKA
    /// sky contacts permit continuation. Their fragments use the same boundary.
    pub fn draw_horizon<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, first: usize) {
        let first = first.min(self.clipmap.levels - 1);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(self.fills[first].clone(), 0, 0..1);
        pass.draw_indexed(self.rings[first]..self.index_count, 0, 0..1);
    }

    pub fn horizon_triangle_count(&self, first: usize) -> u64 {
        let first = first.min(self.clipmap.levels - 1);
        u64::from(
            self.fills[first].end - self.fills[first].start + self.index_count - self.rings[first],
        ) / 3
    }

    pub fn outer_level_for_radius(&self, radius_metres: f64) -> usize {
        (0..self.clipmap.levels)
            .find(|&level| self.clipmap.half(level) >= radius_metres)
            .unwrap_or(self.clipmap.levels - 1)
    }

    /// The first 25 vec4s of the upstream `Surface` block. The point-light
    /// and geographic water-field tail remains zero when those effects are off.
    pub fn surface_prefix(
        &self,
        centre: [f64; 2],
        standoff_metres: f64,
        appearance: SurfaceAppearance,
    ) -> [[f32; 4]; 25] {
        let mut values = [[0.0; 4]; 25];
        values[..10].copy_from_slice(&self.clipmap.frames(centre[0], centre[1]));
        values[10] = [
            self.clipmap.segments as f32,
            self.clipmap.levels as f32,
            centre[0] as f32,
            centre[1] as f32,
        ];
        let detail = self.clipmap.detail(standoff_metres);
        values[11] = [detail.first as f32, detail.morph, 0.0, 0.0];
        values[13] = [
            appearance.absorption[0],
            appearance.absorption[1],
            appearance.absorption[2],
            0.0,
        ];
        values[14] = [
            appearance.transmission[0],
            appearance.transmission[1],
            appearance.transmission[2],
            0.0,
        ];
        values[15] = appearance.crest_foam;
        values[16] = appearance.surface_foam;
        values[17] = appearance.shoreline_foam;
        values[18] = [
            appearance.wind_stretch,
            appearance.windrow_coverage,
            appearance.foam_streak_mean,
            0.0,
        ];
        // The host enables aerial perspective and fills legacy fog parameters
        // when preparing this surface. Clear by default for standalone callers.
        values[23] = [
            appearance.pigment[0],
            appearance.pigment[1],
            appearance.pigment[2],
            0.0,
        ];
        values[24][0] = appearance.rain.clamp(0.0, 1.0);
        values
    }
}

/// `naval::camera::Camera` in the preserved camera.wgsl. JKA supplies scaled
/// metre-space matrices; its ordinary world and depth attachments stay in
/// native map units.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct SurfaceCamera {
    pub view: [[f32; 4]; 4],
    pub projection: [[f32; 4]; 4],
    pub view_projection: [[f32; 4]; 4],
    pub inverse_view: [[f32; 4]; 4],
    pub inverse_projection: [[f32; 4]; 4],
    pub inverse_view_projection: [[f32; 4]; 4],
    pub position_time: [f32; 4],
    pub viewport: [f32; 4],
    pub near: f32,
    pub far: f32,
    pub tan_half_fov_y: f32,
    pub aspect: f32,
    pub dt: f32,
    pub frame: u32,
    pub curvature: f32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct SurfaceBoundary {
    pub bounds: [f32; 4],
    /// Bottom relative to the water plane in metres; y enables floor clipping.
    pub depth: [f32; 4],
    pub counts: [u32; 4],
    pub scale: [f32; 4],
    pub tiles: [u32; 4],
    pub pvs: [[u32; 4]; 8],
    pub triangles: [[f32; 4]; 512],
    pub sky: [[f32; 4]; 16],
    /// Eight 32-bit candidate masks per 8x8 tile; exact edge tests still decide coverage.
    pub candidates: [[u32; 4]; 128],
    /// Shared weather roof texture transform in JKA units; zero extent disables it.
    pub sky_uv: [f32; 4],
    /// Water plane height in JKA units (the native material uses a local Y).
    pub sky_plane: [f32; 4],
}

impl SurfaceBoundary {
    pub fn prepare_candidates(&mut self) {
        self.candidates = [[0; 4]; 128];
        let width = (self.bounds[2] - self.bounds[0]).max(0.0);
        let height = (self.bounds[3] - self.bounds[1]).max(0.0);
        let tolerance = self.scale[1];
        for tile in 0..64 {
            let x = tile % 8;
            let y = tile / 8;
            let lo = [self.bounds[0] + width * x as f32 / 8.0,
                self.bounds[1] + height * y as f32 / 8.0];
            let hi = [self.bounds[0] + width * (x + 1) as f32 / 8.0,
                self.bounds[1] + height * (y + 1) as f32 / 8.0];
            for index in 0..(self.counts[0] as usize).min(256) {
                let ab = self.triangles[index * 2];
                let c = self.triangles[index * 2 + 1];
                let minimum = [ab[0].min(ab[2]).min(c[0]), ab[1].min(ab[3]).min(c[1])];
                let maximum = [ab[0].max(ab[2]).max(c[0]), ab[1].max(ab[3]).max(c[1])];
                if maximum[0] + tolerance >= lo[0] && minimum[0] - tolerance <= hi[0]
                    && maximum[1] + tolerance >= lo[1] && minimum[1] - tolerance <= hi[1] {
                    let word = tile * 8 + index / 32;
                    self.candidates[word / 4][word % 4] |= 1 << (index % 32);
                }
            }
        }
    }
}

pub(crate) fn buffer(device: &wgpu::Device, label: &'static str, size: u64, storage: bool) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(16),
        usage: (if storage {
            wgpu::BufferUsages::STORAGE
        } else {
            wgpu::BufferUsages::UNIFORM
        }) | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

pub(crate) fn placeholder(
    device: &wgpu::Device,
    label: &'static str,
    format: wgpu::TextureFormat,
    dimension: wgpu::TextureViewDimension,
    layers: u32,
) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: layers,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: if dimension == wgpu::TextureViewDimension::D3 {
            wgpu::TextureDimension::D3
        } else {
            wgpu::TextureDimension::D2
        },
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(dimension),
        ..Default::default()
    })
}

pub(crate) fn size_of_binding(module: &wgpu::naga::Module, group: u32, binding: u32) -> u64 {
    let (_, global) = module
        .global_variables
        .iter()
        .find(|(_, global)| {
            global
                .binding
                .as_ref()
                .is_some_and(|at| at.group == group && at.binding == binding)
        })
        .unwrap_or_else(|| panic!("Blue Water original surface binding {group}:{binding}"));
    match &module.types[global.ty].inner {
        wgpu::naga::TypeInner::Struct { span, .. } => u64::from(*span),
        _ => panic!("Blue Water original surface uniform type"),
    }
}

pub(crate) fn entry_buffer(binding: u32, value: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: value.as_entire_binding(),
    }
}

pub(crate) fn entry_texture(binding: u32, value: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(value),
    }
}

pub(crate) fn entry_sampler(binding: u32, value: &wgpu::Sampler) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::Sampler(value),
    }
}

/// `r_oceanType 5`'s textures (group 1, bindings 18-21, `hybrid_gerstner.wgsl`): the packed pattern texture with its
/// repeating sampler and the two persistent foam maps (`crate::godot_water`). Cheap to clone (handles).
#[derive(Clone)]
pub struct GodotWaterViews {
    pub patterns: wgpu::TextureView,
    pub repeat: wgpu::Sampler,
    pub foam_near: wgpu::TextureView,
    pub foam_far: wgpu::TextureView,
}

/// Size of the `JkaHybridSwell` uniform (group 1, binding 17): swell, motion, breaker, Godot water, its foam centres
/// and its water and crest colours.
pub const HYBRID_SWELL_BYTES: u64 = 112;

/// The wake sampler's inputs (`naval_ocean::wake`): the `Wake` uniform, the field's two display textures, the trail atlas
/// and the torpedo field. Cheap to clone (handles).
#[derive(Clone)]
pub struct WakeResources {
    pub uniform: wgpu::Buffer,
    pub displays: [wgpu::TextureView; 2],
    pub trail_atlas: wgpu::TextureView,
    pub torpedo_field: wgpu::TextureView,
}

/// Live bindings for the original material. Neutral shadow, wake, cloud and
/// probe resources keep their corresponding upstream branches disabled until
/// JKA has supplied them. Scene copies can be rebound after resize/capture.
pub struct OriginalSurfaceBindings {
    pub camera_group: wgpu::BindGroup,
    pub material_group: wgpu::BindGroup,
    pub environment_group: wgpu::BindGroup,
    pub(crate) camera: wgpu::Buffer,
    camera_aux: CameraAux,
    current_aerial: Option<wgpu::TextureView>,
    current_cloud: Option<wgpu::TextureView>,
    current_scene: Option<(wgpu::TextureView, wgpu::TextureView)>,
    neutral_cloud: (wgpu::TextureView, wgpu::Sampler, wgpu::Buffer),
    surface: wgpu::Buffer,
    hybrid_swell: wgpu::Buffer,
    /// `r_oceanType 5`'s textures; 1x1 placeholders until `set_godot_water`.
    godot: GodotWaterViews,
    neutral_godot: GodotWaterViews,
    environment: wgpu::Buffer,
    ambient_atlas: wgpu::Texture,
    ambient: [f32; 3],
    foam: crate::foam_texture::FoamMap,
    pub(crate) calm_wake: wgpu::Buffer,
    pub(crate) calm_view: wgpu::TextureView,
    pub(crate) wake_sampler: wgpu::Sampler,
    /// Live ship-wake resources (group 1, bindings 5-10); calm until `set_wake` supplies the effects crate's.
    wake: WakeResources,
    wake_generation: u64,
    cloud_view: wgpu::TextureView,
    cloud_sampler: wgpu::Sampler,
    cloud_uniform: wgpu::Buffer,
    pub(crate) probe_view: wgpu::TextureView,
    reflection_table: wgpu::Buffer,
    pub(crate) boundary: wgpu::Buffer,
    sky_roof: wgpu::TextureView,
    pub surface_bytes: usize,
}

struct CameraAux {
    shadows: wgpu::Buffer,
    motion: wgpu::Buffer,
    occluder: wgpu::Buffer,
    occluder_uniform: wgpu::Buffer,
    shadow_array: wgpu::TextureView,
    shadow_wide: wgpu::TextureView,
    shadow_sampler: wgpu::Sampler,
    aerial: wgpu::TextureView,
    aerial_sampler: wgpu::Sampler,
}
impl OriginalSurfaceBindings {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipeline: &OriginalSurfacePipeline,
    ) -> Self {
        Self::with_foam(
            device,
            queue,
            pipeline,
            crate::foam_texture::FoamMap::new(device, queue),
        )
    }

    pub fn with_foam(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipeline: &OriginalSurfacePipeline,
        foam: crate::foam_texture::FoamMap,
    ) -> Self {
        let mut composer = shader_composer::composer().expect("Blue Water modules");
        let module = shader_composer::surface_module(&mut composer, false)
            .expect("original Blue Water surface");
        let camera = buffer(
            device,
            "Blue Water camera",
            std::mem::size_of::<SurfaceCamera>() as u64,
            false,
        );
        let shadows = buffer(
            device,
            "Blue Water disabled shadows",
            size_of_binding(&module, 0, 1),
            false,
        );
        // The color entry point does not import motion; keep the published
        // camera-layout slot for the later motion pipeline.
        let motion = buffer(device, "Blue Water camera motion", 144, false);
        let occluder = buffer(
            device,
            "Blue Water disabled occluder data",
            256 * 256 * 8,
            true,
        );
        let occluder_uniform = buffer(
            device,
            "Blue Water disabled occluders",
            size_of_binding(&module, 0, 9),
            false,
        );
        let shadow_array = placeholder(
            device,
            "Blue Water blank shadow array",
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureViewDimension::D2Array,
            7,
        );
        let shadow_wide = placeholder(
            device,
            "Blue Water blank wide shadow",
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureViewDimension::D2,
            1,
        );
        let aerial = placeholder(
            device,
            "Blue Water blank aerial volume",
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureViewDimension::D3,
            1,
        );
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Blue Water disabled shadow comparison"),
            compare: Some(wgpu::CompareFunction::GreaterEqual),
            ..Default::default()
        });
        let aerial_sampler = device.create_sampler(&Default::default());
        let camera_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Blue Water original camera"),
            layout: &pipeline.camera_layout,
            entries: &[
                entry_buffer(0, &camera),
                entry_buffer(1, &shadows),
                entry_texture(2, &shadow_array),
                entry_texture(3, &shadow_wide),
                entry_sampler(4, &shadow_sampler),
                entry_buffer(5, &motion),
                entry_texture(6, &aerial),
                entry_sampler(7, &aerial_sampler),
                entry_buffer(8, &occluder),
                entry_buffer(9, &occluder_uniform),
            ],
        });
        let surface_bytes = size_of_binding(&module, 1, 0) as usize;
        let surface = buffer(
            device,
            "Blue Water original surface parameters",
            surface_bytes as u64,
            false,
        );
        let hybrid_swell = buffer(device, "JKA Hybrid Gerstner and crest control", HYBRID_SWELL_BYTES, false);
        let pure_fft: [[f32; 4]; 3] = [[0.0; 4], [0.0, 0.0, 1.0, 0.0], [0.0; 4]];
        queue.write_buffer(&hybrid_swell, 0, bytemuck::bytes_of(&pure_fft));
        let calm_wake = buffer(
            device,
            "Blue Water calm wake",
            size_of_binding(&module, 1, 5),
            false,
        );
        let calm_view = placeholder(
            device,
            "Blue Water calm wake texture",
            wgpu::TextureFormat::Rgba16Float,
            wgpu::TextureViewDimension::D2,
            1,
        );
        let wake_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let cloud_view = placeholder(
            device,
            "Blue Water disabled cloud shadow",
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureViewDimension::D2,
            1,
        );
        let cloud_sampler = device.create_sampler(&Default::default());
        let cloud_uniform = buffer(
            device,
            "Blue Water disabled cloud shadow uniform",
            size_of_binding(&module, 1, 13),
            false,
        );
        let probe_view = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("Blue Water original underwater probe"),
                size: wgpu::Extent3d {
                    width: 128,
                    height: 72,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let reflection_table = buffer(device, "Blue Water unpruned reflection table", 4096, true);
        let boundary = buffer(
            device,
            "Blue Water JKA admission boundary",
            std::mem::size_of::<SurfaceBoundary>() as u64,
            false,
        );
        let mut admission = SurfaceBoundary::zeroed();
        // Standalone BlueWater has no BSP; preserve its original visible sky.
        admission.bounds = [-1e8, -1e8, 1e8, 1e8];
        admission.scale = [1.0, 0.75, 0.0, 0.0];
        queue.write_buffer(&boundary, 0, bytemuck::bytes_of(&admission));
        let color = placeholder(
            device,
            "Blue Water blank opaque scene color",
            wgpu::TextureFormat::Rgba16Float,
            wgpu::TextureViewDimension::D2,
            1,
        );
        let depth = placeholder(
            device,
            "Blue Water blank opaque scene depth",
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureViewDimension::D2,
            1,
        );
        let blank_godot = placeholder(device, "Godot water blank texture", wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureViewDimension::D2, 1);
        let neutral_godot = GodotWaterViews {
            patterns: blank_godot.clone(),
            repeat: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("Godot water blank repeat"),
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::Repeat,
                ..Default::default()
            }),
            foam_near: blank_godot.clone(),
            foam_far: blank_godot,
        };
        let sky_roof = placeholder(device, "no BSP roof", wgpu::TextureFormat::Rgba32Float,
            wgpu::TextureViewDimension::D2, 1);
        let material_group = Self::make_material_group(
            device,
            &pipeline.surface_layout,
            &surface,
            &foam,
            &WakeResources {
                uniform: calm_wake.clone(),
                displays: [calm_view.clone(), calm_view.clone()],
                trail_atlas: calm_view.clone(),
                torpedo_field: calm_view.clone(),
            },
            &wake_sampler,
            &cloud_view,
            &cloud_sampler,
            &cloud_uniform,
            &probe_view,
            &reflection_table,
            &boundary,
            &hybrid_swell,
            &neutral_godot,
            &sky_roof,
            &color,
            &depth,
        );
        let environment = buffer(device, "Blue Water environment", 80, false);
        let atlas_size = wgpu::Extent3d {
            width: 384,
            height: 512,
            depth_or_array_layers: 1,
        };
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Blue Water temporary sky environment atlas"),
            size: atlas_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let mut pixels = vec![0u8; atlas_size.width as usize * atlas_size.height as usize * 4];
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[117, 154, 186, 255]);
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &atlas,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(atlas_size.width * 4),
                rows_per_image: Some(atlas_size.height),
            },
            atlas_size,
        );
        let atlas_view = atlas.create_view(&Default::default());
        let atlas_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let environment_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Blue Water temporary environment"),
            layout: &pipeline.environment_layout,
            entries: &[
                entry_texture(0, &atlas_view),
                entry_sampler(1, &atlas_sampler),
                entry_buffer(2, &environment),
            ],
        });
        // SkyEnvironment: atlas texels, max mip, generation, light, sun and moon.
        let mut environment_values = [0.0f32; 20];
        environment_values[0] = 1.0 / atlas_size.width as f32;
        environment_values[1] = 1.0 / atlas_size.height as f32;
        environment_values[2] = 7.0;
        environment_values[5] = 1.0;
        environment_values[7] = 5.0;
        environment_values[8..11].copy_from_slice(&[1.0, 0.95, 0.85]);
        environment_values[13] = 1.0;
        queue.write_buffer(&environment, 0, bytemuck::cast_slice(&environment_values));
        Self {
            camera_group,
            material_group,
            environment_group,
            camera,
            camera_aux: CameraAux {
                shadows,
                motion,
                occluder,
                occluder_uniform,
                shadow_array,
                shadow_wide,
                shadow_sampler,
                aerial,
                aerial_sampler,
            },
            current_aerial: None,
            current_cloud: None,
            current_scene: None,
            neutral_cloud: (
                cloud_view.clone(),
                cloud_sampler.clone(),
                cloud_uniform.clone(),
            ),
            surface,
            hybrid_swell,
            godot: neutral_godot.clone(),
            neutral_godot,
            environment,
            ambient_atlas: atlas,
            ambient: [117.0 / 255.0, 154.0 / 255.0, 186.0 / 255.0],
            foam,
            wake: WakeResources {
                uniform: calm_wake.clone(),
                displays: [calm_view.clone(), calm_view.clone()],
                trail_atlas: calm_view.clone(),
                torpedo_field: calm_view.clone(),
            },
            wake_generation: 0,
            calm_wake,
            calm_view,
            wake_sampler,
            cloud_view,
            cloud_sampler,
            cloud_uniform,
            probe_view,
            reflection_table,
            boundary,
            sky_roof,
            surface_bytes,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn make_material_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        surface: &wgpu::Buffer,
        foam: &crate::foam_texture::FoamMap,
        wake: &WakeResources,
        wake_sampler: &wgpu::Sampler,
        cloud_view: &wgpu::TextureView,
        cloud_sampler: &wgpu::Sampler,
        cloud_uniform: &wgpu::Buffer,
        probe_view: &wgpu::TextureView,
        reflection_table: &wgpu::Buffer,
        boundary: &wgpu::Buffer,
        hybrid_swell: &wgpu::Buffer,
        godot: &GodotWaterViews,
        sky_roof: &wgpu::TextureView,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Blue Water original surface material"),
            layout,
            entries: &[
                entry_buffer(0, surface),
                entry_texture(1, &foam.view),
                entry_sampler(2, &foam.sampler),
                entry_texture(3, color),
                entry_texture(4, depth),
                entry_buffer(5, &wake.uniform),
                entry_texture(6, &wake.displays[0]),
                entry_texture(7, &wake.displays[1]),
                entry_sampler(8, wake_sampler),
                entry_texture(9, &wake.trail_atlas),
                entry_texture(10, &wake.torpedo_field),
                entry_texture(11, cloud_view),
                entry_sampler(12, cloud_sampler),
                entry_buffer(13, cloud_uniform),
                entry_texture(14, probe_view),
                entry_buffer(15, reflection_table),
                entry_buffer(16, boundary),
                entry_buffer(17, hybrid_swell),
                entry_texture(18, &godot.patterns),
                entry_sampler(19, &godot.repeat),
                entry_texture(20, &godot.foam_near),
                entry_texture(21, &godot.foam_far),
                entry_texture(22, sky_roof),
            ],
        })
    }

    /// Read the depth copy's tiles (`crate::reflection_tiles`) instead of the zeroed table that traces every ray.
    /// Rebuilds the material group against the scene copies already bound.
    pub fn set_reflection_table(&mut self, device: &wgpu::Device, pipeline: &OriginalSurfacePipeline, table: &wgpu::Buffer) {
        if self.reflection_table == *table { return; }
        self.reflection_table = table.clone();
        if let Some((color, depth)) = self.current_scene.take() {
            self.set_scene(device, pipeline, &color, &depth);
        }
    }

    pub fn set_sky_roof(&mut self, device: &wgpu::Device, pipeline: &OriginalSurfacePipeline, roof: &wgpu::TextureView) {
        if self.sky_roof == *roof { return; }
        self.sky_roof = roof.clone();
        if let Some((color, depth)) = self.current_scene.take() {
            self.set_scene(device, pipeline, &color, &depth);
        }
    }

    pub fn set_scene(
        &mut self,
        device: &wgpu::Device,
        pipeline: &OriginalSurfacePipeline,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
    ) {
        let next = (color.clone(), depth.clone());
        if self.current_scene.as_ref() == Some(&next) { return; }
        self.material_group = Self::make_material_group(
            device,
            &pipeline.surface_layout,
            &self.surface,
            &self.foam,
            &self.wake,
            &self.wake_sampler,
            &self.cloud_view,
            &self.cloud_sampler,
            &self.cloud_uniform,
            &self.probe_view,
            &self.reflection_table,
            &self.boundary,
            &self.hybrid_swell,
            &self.godot,
            &self.sky_roof,
            color,
            depth,
        );
        self.current_scene = Some(next);
    }

    /// Bind `r_oceanType 5`'s textures (or the neutral ones again for `None`). Rebuilds the material group against the
    /// scene copies already bound.
    pub fn set_godot_water(&mut self, device: &wgpu::Device, pipeline: &OriginalSurfacePipeline,
        views: Option<&GodotWaterViews>) {
        let next = views.unwrap_or(&self.neutral_godot);
        if self.godot.patterns == next.patterns && self.godot.foam_near == next.foam_near
            && self.godot.foam_far == next.foam_far {
            return;
        }
        self.godot = next.clone();
        if let Some((color, depth)) = self.current_scene.take() {
            self.set_scene(device, pipeline, &color, &depth);
        }
    }

    /// Bind live ship-wake resources (or calm ones again for `None`). Rebuilds the material group against the scene
    /// copies already bound; `wake_generation` moves so dependent bind groups (underwater, rain heights) can follow.
    pub fn set_wake(
        &mut self,
        device: &wgpu::Device,
        pipeline: &OriginalSurfacePipeline,
        wake: Option<WakeResources>,
    ) {
        let next = wake.unwrap_or_else(|| WakeResources {
            uniform: self.calm_wake.clone(),
            displays: [self.calm_view.clone(), self.calm_view.clone()],
            trail_atlas: self.calm_view.clone(),
            torpedo_field: self.calm_view.clone(),
        });
        let same = self.wake.uniform == next.uniform
            && self.wake.displays == next.displays
            && self.wake.trail_atlas == next.trail_atlas
            && self.wake.torpedo_field == next.torpedo_field;
        if same { return; }
        self.wake = next;
        self.wake_generation += 1;
        if let Some((color, depth)) = self.current_scene.take() {
            self.set_scene(device, pipeline, &color, &depth);
        }
    }

    pub fn wake_resources(&self) -> &WakeResources { &self.wake }
    /// The pool's `JkaWaterBoundary` uniform, for effects that must stay inside its footprint.
    pub fn boundary(&self) -> &wgpu::Buffer { &self.boundary }
    /// The cloud-shadow resources currently bound (neutral when clouds are off).
    pub fn cloud_resources(&self) -> (&wgpu::TextureView, &wgpu::Sampler, &wgpu::Buffer) {
        (&self.cloud_view, &self.cloud_sampler, &self.cloud_uniform)
    }
    pub fn wake_generation(&self) -> u64 { self.wake_generation }

    /// Bind the original atmosphere's froxels and cloud shadow resources, restoring clear air when absent.
    pub fn set_atmosphere_resources(
        &mut self,
        device: &wgpu::Device,
        pipeline: &OriginalSurfacePipeline,
        aerial: Option<(&wgpu::TextureView, &wgpu::Sampler)>,
        cloud: Option<(&wgpu::TextureView, &wgpu::Sampler, &wgpu::Buffer)>,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
    ) {
        let next_aerial = aerial.map(|(view, _)| view.clone());
        if self.current_aerial != next_aerial {
            let aux = &self.camera_aux;
            let (view, sampler) = aerial.unwrap_or((&aux.aerial, &aux.aerial_sampler));
            self.camera_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Blue Water live atmosphere camera"),
                layout: &pipeline.camera_layout,
                entries: &[
                    entry_buffer(0, &self.camera),
                    entry_buffer(1, &aux.shadows),
                    entry_texture(2, &aux.shadow_array),
                    entry_texture(3, &aux.shadow_wide),
                    entry_sampler(4, &aux.shadow_sampler),
                    entry_buffer(5, &aux.motion),
                    entry_texture(6, view),
                    entry_sampler(7, sampler),
                    entry_buffer(8, &aux.occluder),
                    entry_buffer(9, &aux.occluder_uniform),
                ],
            });
            self.current_aerial = next_aerial;
        }
        let next_cloud = cloud.map(|(view, _, _)| view.clone());
        if self.current_cloud != next_cloud {
            let (view, sampler, uniform) = cloud.unwrap_or((
                &self.neutral_cloud.0,
                &self.neutral_cloud.1,
                &self.neutral_cloud.2,
            ));
            self.cloud_view = view.clone();
            self.cloud_sampler = sampler.clone();
            self.cloud_uniform = uniform.clone();
            self.current_scene = None;
            self.set_scene(device, pipeline, color, depth);
            self.current_cloud = next_cloud;
        }
    }
    pub fn underwater_probe(&self) -> &wgpu::TextureView {
        &self.probe_view
    }

    pub fn write_camera(&self, queue: &wgpu::Queue, camera: &SurfaceCamera) {
        queue.write_buffer(&self.camera, 0, bytemuck::bytes_of(camera));
    }

    pub fn write_boundary(&self, queue: &wgpu::Queue, boundary: &SurfaceBoundary) {
        assert!(boundary.counts[0] <= 256 && boundary.counts[1] <= 16);
        queue.write_buffer(&self.boundary, 0, bytemuck::bytes_of(boundary));
    }

    pub fn foam_streak_mean(&self) -> f32 {
        self.foam.streak_mean
    }

    pub fn set_environment_atlas(
        &mut self,
        device: &wgpu::Device,
        pipeline: &OriginalSurfacePipeline,
        atlas: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
    ) {
        self.environment_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Blue Water live sky environment"),
            layout: &pipeline.environment_layout,
            entries: &[
                entry_texture(0, atlas),
                entry_sampler(1, sampler),
                entry_buffer(2, &self.environment),
            ],
        });
    }

    pub fn use_ambient_atlas(&mut self, device: &wgpu::Device, pipeline: &OriginalSurfacePipeline) {
        let view = self.ambient_atlas.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        self.set_environment_atlas(device, pipeline, &view, &sampler);
    }

    /// Use the original atmosphere's atlas and complete sun/moon lighting block.
    pub fn set_sky_environment(
        &mut self,
        device: &wgpu::Device,
        pipeline: &OriginalSurfacePipeline,
        atlas: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        uniforms: &wgpu::Buffer,
    ) {
        self.environment_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Blue Water original atmosphere lighting"),
            layout: &pipeline.environment_layout,
            entries: &[
                entry_texture(0, atlas),
                entry_sampler(1, sampler),
                entry_buffer(2, uniforms),
            ],
        });
    }

    /// Map ambient fallback until the directional sky atlas is available.
    pub fn write_ambient(&mut self, queue: &wgpu::Queue, ambient: [f32; 3]) {
        if self.ambient == ambient {
            return;
        }
        self.ambient = ambient;
        let rgb = ambient.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
        let mut pixels = vec![0u8; 384 * 512 * 4];
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.ambient_atlas,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(384 * 4),
                rows_per_image: Some(512),
            },
            wgpu::Extent3d {
                width: 384,
                height: 512,
                depth_or_array_layers: 1,
            },
        );
    }

    pub fn write_hybrid_swell(&self, queue: &wgpu::Queue, two_vec4: &[[f32; 4]; 2]) {
        queue.write_buffer(&self.hybrid_swell, 0, bytemuck::bytes_of(two_vec4));
    }

    pub fn write_breaker_mode(&self, queue: &wgpu::Queue, mode: u8, fast_foam: f32, fast_foam_detail: bool) {
        queue.write_buffer(&self.hybrid_swell, 32, bytemuck::bytes_of(&[f32::from(mode), fast_foam,
            if fast_foam_detail { 1.0 } else { 0.0 }, 0.0]));
    }

    /// `r_oceanType 5`: [1 on, wave scale, seconds, foam bits], the foam maps' centres [near x, z, far x, z] (m) and the
    /// linear water and crest colours. Zero turns the Godot waves off.
    pub fn write_godot_water(&self, queue: &wgpu::Queue, godot: &[[f32; 4]; 4]) {
        queue.write_buffer(&self.hybrid_swell, 48, bytemuck::bytes_of(godot));
    }

    pub fn write_surface_prefix(&self, queue: &wgpu::Queue, first_25_vec4: &[[f32; 4]; 25]) {
        assert!(self.surface_bytes >= 25 * 16);
        queue.write_buffer(&self.surface, 0, bytemuck::bytes_of(first_25_vec4));
    }

    pub fn write_sun(
        &self,
        queue: &wgpu::Queue,
        direction: [f32; 3],
        color: [f32; 3],
        intensity: f32,
    ) {
        let mut data = [0.0f32; 20];
        data[0] = 1.0 / 384.0;
        data[1] = 1.0 / 512.0;
        data[2] = 7.0;
        data[4..7].copy_from_slice(&direction);
        data[7] = intensity;
        data[8..11].copy_from_slice(&color);
        data[12..15].copy_from_slice(&direction);
        data[15] = 0.5;
        data[17] = -1.0;
        data[19] = 1.0;
        queue.write_buffer(&self.environment, 0, bytemuck::cast_slice(&data));
    }
}

impl OriginalSurfacePipeline {
    pub fn new(
        device: &wgpu::Device,
        waves: &SurfaceWaves,
        color_format: wgpu::TextureFormat,
        samples: u32,
        reflection_steps: u32,
    ) -> Self {
        Self::new_with_pixel_detail(device, waves, color_format, samples, reflection_steps, 0)
    }

    /// Separate program for sparse-shading A/B. Only the selected mode is
    /// compiled: there is no runtime per-fragment mode check in default mode.
    pub fn new_with_pixel_detail(
        device: &wgpu::Device,
        waves: &SurfaceWaves,
        color_format: wgpu::TextureFormat,
        samples: u32,
        reflection_steps: u32,
        pixel_detail_mode: u8,
    ) -> Self {
        Self::new_with_wave_layout(device, waves.layout(), color_format, samples, reflection_steps, pixel_detail_mode, 0)
    }

    /// Pipeline construction needs only the immutable wave layout. Hosts can
    /// clone that handle into their build queue without sharing wave state.
    pub fn new_with_wave_layout(
        device: &wgpu::Device,
        wave_layout: &wgpu::BindGroupLayout,
        color_format: wgpu::TextureFormat,
        samples: u32,
        reflection_steps: u32,
        pixel_detail_mode: u8,
        // Main full-resolution program variant: `shader_composer::COST_PROBE_*` stubs (A/B cost measurement) and
        // the lossless `SURFACE_SKIP_FADED_CASCADES`; 0 is the original.
        cost_probe: u32,
    ) -> Self {
        let vertex = wgpu::ShaderStages::VERTEX;
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let both = vertex | fragment;
        let t2 = wgpu::TextureViewDimension::D2;
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Blue Water original camera layout"),
            entries: &[
                // Vertex visibility too: native spindrift lights and hazes each wisp in its vertex stage.
                uniform(0, both),
                uniform(1, both),
                sampled(2, both, wgpu::TextureViewDimension::D2Array, true),
                sampled(3, both, t2, true),
                sampler(4, both, true),
                uniform(5, both),
                sampled(6, both, wgpu::TextureViewDimension::D3, false),
                sampler(7, both, false),
                storage(8, both),
                uniform(9, both),
            ],
        });
        let surface_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Blue Water original material layout"),
            entries: &[
                uniform(0, both),
                sampled(1, fragment, t2, false),
                sampler(2, fragment, false),
                sampled(3, fragment, t2, false),
                sampled(4, fragment, t2, true),
                uniform(5, both),
                sampled(6, both, t2, false),
                sampled(7, both, t2, false),
                sampler(8, both, false),
                sampled(9, fragment, t2, false),
                sampled(10, fragment, t2, false),
                sampled(11, fragment, t2, false),
                sampler(12, fragment, false),
                uniform(13, fragment),
                sampled(14, fragment, t2, false),
                storage(15, fragment),
                uniform(16, fragment),
                uniform(17, both),
                sampled(18, both, t2, false),
                sampler(19, both, false),
                sampled(20, fragment, t2, false),
                sampled(21, fragment, t2, false),
                roof_sampled(fragment),
            ],
        });
        let environment_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Blue Water original environment layout"),
                entries: &[
                    sampled(0, both, t2, false),
                    sampler(1, both, false),
                    uniform(2, both),
                ],
            });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Blue Water original surface"),
            bind_group_layouts: &[
                Some(&camera_layout),
                Some(&surface_layout),
                Some(wave_layout),
                Some(&environment_layout),
            ],
            immediate_size: 0,
        });
        // Reduced modes shade at the full-resolution footprint (`full_resolution_footprint`), modules included.
        let mut composer = shader_composer::composer_with_surface_probes(
            cost_probe,
            shader_composer::pixel_scale(pixel_detail_mode)).expect("Blue Water shader modules");
        let source = shader_composer::jka_surface_module_with_probe(&mut composer, reflection_steps, pixel_detail_mode, cost_probe)
            .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&composer)));
        let reduced_targets: [Option<wgpu::ColorTargetState>; 3] =
            std::array::from_fn(|_| Some(wgpu::TextureFormat::Rgba16Float.into()));
        let main_targets = [Some(color_format.into())];
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Blue Water original surface shader"),
            source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(source)),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Blue Water original surface"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[Some(geometry::OceanVertex::LAYOUT)],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                // Reduced modes compose the material with the glint outputs (`jka_reduced_glints`); this pipeline is
                // then unused but must match them.
                targets: if pixel_detail_mode == 0 { &main_targets[..] } else { &reduced_targets[..] },
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });
        let (reduced_pipeline, near_pipeline, reconstruct_pipeline, reconstruct_layout) = if pixel_detail_mode == 0 {
            (None, None, None, None)
        } else {
            // Render the complete surface graph at true reduced raster resolution.
            // The expensive shader does not execute at intermediate pixels.
            let reduced = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Blue Water true reduced-resolution material"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader, entry_point: Some("vertex"),
                    compilation_options: Default::default(),
                    buffers: &[Some(geometry::OceanVertex::LAYOUT)],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader, entry_point: Some("fragment"),
                    compilation_options: Default::default(),
                    // Colour without glints, glitter mean/density, glint share (`jka_reduced_glints`).
                    targets: &reduced_targets,
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw, cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                    stencil: Default::default(), bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState { count: 1, ..Default::default() },
                multiview_mask: None, cache: None,
            });
            let reduced_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Blue Water reduced shaded color/depth"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2, multisampled: false,
                        }, count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Depth,
                            view_dimension: wgpu::TextureViewDimension::D2, multisampled: false,
                        }, count: None,
                    },
                    // The glitter's mean/density and the glint share (`jka_reduced_glints`).
                    wgpu::BindGroupLayoutEntry {
                        binding: 3, visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2, multisampled: false,
                        }, count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 4, visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2, multisampled: false,
                        }, count: None,
                    },
                    // Mode 5's tile decisions (`adaptive_tiles.wgsl`); bound but unread in modes 1-4.
                    wgpu::BindGroupLayoutEntry {
                        binding: 2, visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false, min_binding_size: None,
                        }, count: None,
                    },
                ],
            });
            let reconstruction_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Blue Water reduced reconstruction layout"),
                bind_group_layouts: &[
                    Some(&camera_layout), Some(&surface_layout), Some(wave_layout),
                    Some(&environment_layout), Some(&reduced_layout),
                ], immediate_size: 0,
            });
            let mut composer = shader_composer::composer().expect("Blue Water composer");
            let adaptive = pixel_detail_mode == 5;
            let reconstruction = if adaptive {
                shader_composer::jka_adaptive_reconstruct_module(&mut composer)
            } else {
                shader_composer::jka_reconstruct_module(&mut composer)
            }.unwrap_or_else(|error| panic!("{}", error.emit_to_string(&composer)));
            let reconstruction_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Blue Water full-resolution depth-aware upscaler"),
                source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(reconstruction)),
            });
            let reconstructed = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Blue Water full-resolution geometry, depth and color reconstruction"),
                layout: Some(&reconstruction_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &reconstruction_shader, entry_point: Some("vertex"),
                    compilation_options: Default::default(),
                    buffers: &[Some(geometry::OceanVertex::LAYOUT)],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &reconstruction_shader, entry_point: Some("fragment"),
                    compilation_options: Default::default(), targets: &[Some(color_format.into())],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw, cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                    stencil: Default::default(), bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
                multiview_mask: None, cache: None,
            });
            let near = if matches!(pixel_detail_mode, 3 | 4) {
                let mut composer = shader_composer::composer().expect("Blue Water composer");
                let near_module = shader_composer::jka_near_surface_module(&mut composer, reflection_steps, cost_probe)
                    .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&composer)));
                let near_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("Blue Water distance-aware near detail"),
                    source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(near_module)),
                });
                Some(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("Blue Water full-resolution near detail / feathered overlay"),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &near_shader, entry_point: Some("vertex"),
                        compilation_options: Default::default(),
                        buffers: &[Some(geometry::OceanVertex::LAYOUT)],
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &near_shader, entry_point: Some("fragment"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: color_format,
                            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleList,
                        front_face: wgpu::FrontFace::Ccw, cull_mode: None, ..Default::default()
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: wgpu::TextureFormat::Depth32Float,
                        depth_write_enabled: Some(false),
                        depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                        stencil: Default::default(), bias: Default::default(),
                    }),
                    multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
                    multiview_mask: None, cache: None,
                }))
            } else if adaptive {
                // Mode 5: the full material on the blocks the reconstruction discards, opaque and depth-writing like
                // it, so each pixel is drawn by exactly one of the two.
                let mut composer = shader_composer::composer().expect("Blue Water composer");
                let full_module = shader_composer::jka_adaptive_surface_module(&mut composer, reflection_steps, cost_probe)
                    .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&composer)));
                let full_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("Blue Water adaptive full-resolution detail"),
                    source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(full_module)),
                });
                Some(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("Blue Water adaptive full-resolution detail"),
                    layout: Some(&reconstruction_pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &full_shader, entry_point: Some("vertex"),
                        compilation_options: Default::default(),
                        buffers: &[Some(geometry::OceanVertex::LAYOUT)],
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &full_shader, entry_point: Some("fragment"),
                        compilation_options: Default::default(), targets: &[Some(color_format.into())],
                    }),
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleList,
                        front_face: wgpu::FrontFace::Ccw, cull_mode: None, ..Default::default()
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: wgpu::TextureFormat::Depth32Float,
                        depth_write_enabled: Some(true),
                        depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                        stencil: Default::default(), bias: Default::default(),
                    }),
                    multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
                    multiview_mask: None, cache: None,
                }))
            } else { None };
            (Some(reduced), near, Some(reconstructed), Some(reduced_layout))
        };
        Self {
            camera_layout,
            surface_layout,
            environment_layout,
            pipeline,
            reduced_pipeline,
            near_pipeline,
            reconstruct_pipeline,
            reconstruct_layout,
            adaptive_classify: (pixel_detail_mode == 5).then(|| adaptive_classifier(device)),
            far_pipeline: None,
            wireframe: None,
            waterline_marker: None,
            vertex_shader: shader,
            pipeline_layout: layout,
        }
    }

    /// Builds `far_pipeline`: the same material composed with `far_variant` stubs on top of `cost_probe`, for the
    /// pixel-detail mode's expensive pass (full resolution in mode 0, the reduced target otherwise). `far_variant` 0
    /// leaves it unbuilt.
    #[allow(clippy::too_many_arguments)]
    pub fn with_far_variant(mut self, device: &wgpu::Device, color_format: wgpu::TextureFormat, samples: u32,
        reflection_steps: u32, pixel_detail_mode: u8, cost_probe: u32, far_variant: u32) -> Self {
        if far_variant == 0 {
            return self;
        }
        let variant = cost_probe | far_variant;
        let mut composer = shader_composer::composer_with_surface_probes(
            variant,
            shader_composer::pixel_scale(pixel_detail_mode)).expect("Blue Water shader modules");
        let source = shader_composer::jka_surface_module_with_probe(&mut composer, reflection_steps, pixel_detail_mode, variant)
            .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&composer)));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Blue Water far-water surface shader"),
            source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(source)),
        });
        let reduced_targets: [Option<wgpu::ColorTargetState>; 3] =
            std::array::from_fn(|_| Some(wgpu::TextureFormat::Rgba16Float.into()));
        let main_targets = [Some(color_format.into())];
        let (targets, samples): (&[Option<wgpu::ColorTargetState>], u32) =
            if pixel_detail_mode == 0 { (&main_targets, samples) } else { (&reduced_targets, 1) };
        self.far_pipeline = Some(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Blue Water far-water surface"),
            layout: Some(&self.pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[Some(geometry::OceanVertex::LAYOUT)],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets,
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
            multiview_mask: None,
            cache: None,
        }));
        self
    }

    /// Explicit debug mode 3 only: draw the GPU-evaluated wave at a fixed 100 JKA
    /// units forward, but heights sampled under the camera. No new bindings/readback.
    pub fn ensure_waterline_marker(&mut self, device: &wgpu::Device,
        color_format: wgpu::TextureFormat, samples: u32) {
        if self.waterline_marker.is_some() { return; }
        let mut composer = shader_composer::composer().expect("Blue Water diagnostic composer");
        let module = shader_composer::jka_waterline_marker_module(&mut composer)
            .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&composer)));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("JKA native waterline marker"),
            source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(module)),
        });
        self.waterline_marker = Some(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("JKA native waterline diagnostic overlay"),
            layout: Some(&self.pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("marker_vertex"),
                buffers: &[], compilation_options: Default::default() },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("marker_fragment"),
                targets: &[Some(color_format.into())], compilation_options: Default::default() }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(), bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
            multiview_mask: None, cache: None,
        }));
    }

    /// Compile this diagnostic variant only when r_showtris enables ocean lines.
    pub fn ensure_wireframe(&mut self, device: &wgpu::Device, color_format: wgpu::TextureFormat, samples: u32) {
        if self.wireframe.is_some() || !device.features().contains(wgpu::Features::POLYGON_MODE_LINE) {
            return;
        }
        let mut composer = shader_composer::composer().expect("Blue Water wireframe composer");
        let module = shader_composer::jka_wireframe_module(&mut composer)
            .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&composer)));
        let line_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Blue Water PVS/footprint-masked native wireframe"),
            source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(module)),
        });
        self.wireframe = Some(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Blue Water original native wireframe"),
            layout: Some(&self.pipeline_layout),
            vertex: wgpu::VertexState {
                module: &self.vertex_shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[Some(geometry::OceanVertex::LAYOUT)],
            },
            fragment: Some(wgpu::FragmentState {
                module: &line_shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(color_format.into())],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                polygon_mode: wgpu::PolygonMode::Line,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
            multiview_mask: None,
            cache: None,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blue_water_core::QualityTier;

    #[test]
    fn decimated_rings_keep_exact_dense_boundaries_and_watertight_indices() {
        use std::collections::HashMap;
        for segments in [32u32, 64, 128] {
            let half = (segments / 2) as i32;
            let hole = (segments / 4) as i32;
            for stride in [2, 4, 8, 16] {
                let mut indices = Vec::new();
                append_stitched_ring(&mut indices, segments, 0, stride.min(segments / 4));
                assert_eq!(indices.len() % 3, 0);
                assert!(indices.len() < (segments * segments * 3 / 4 * 6) as usize);
                let point = |id: u32| {
                    let span = segments + 1;
                    ((id % span) as i32 - half, (id / span) as i32 - half)
                };
                let mut edges: HashMap<((i32, i32), (i32, i32)), u32> = HashMap::new();
                let mut doubled_area = 0i64;
                for triangle in indices.chunks_exact(3) {
                    let [a, b, c] = [point(triangle[0]), point(triangle[1]), point(triangle[2])];
                    let cross = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
                    assert!(cross < 0, "zero/inverted triangle, stride={stride}");
                    doubled_area += i64::from(-cross);
                    for (a, b) in [(a, b), (b, c), (c, a)] {
                        *edges.entry(if a < b { (a, b) } else { (b, a) }).or_default() += 1;
                    }
                }
                assert_eq!(doubled_area, i64::from(segments * segments * 3 / 2));
                let boundary: Vec<_> = edges.iter().filter(|edge| *edge.1 == 1).collect();
                assert_eq!(boundary.len(), (6 * segments) as usize);
                for (edge, _) in boundary {
                    let (a, b) = *edge;
                    // Every seam segment is exactly one ORIGINAL grid cell.
                    assert_eq!((a.0 - b.0).abs() + (a.1 - b.1).abs(), 1);
                    assert!(a.0.abs() == half || b.0.abs() == half
                        || a.1.abs() == half || b.1.abs() == half
                        || a.0.abs() == hole || b.0.abs() == hole
                        || a.1.abs() == hole || b.1.abs() == hole);
                }
                assert!(edges.values().all(|&n| n == 1 || n == 2), "nonmanifold index edges");
            }
        }
    }

    #[test]
    fn decimated_fill_keeps_dense_ring_join_without_nonmanifold_edges() {
        use std::collections::HashMap;
        for segments in [64u32, 128, 256] {
            let half = (segments / 2) as i32;
            let hole = (segments / 4) as i32;
            for stride in [2, 4, 8, 16] {
                let mut indices = Vec::new();
                append_stitched_fill(&mut indices, segments, 0, stride);
                let point = |id: u32| {
                    let span = segments + 1;
                    ((id % span) as i32 - half, (id / span) as i32 - half)
                };
                let mut edges: HashMap<((i32, i32), (i32, i32)), u32> = HashMap::new();
                let mut doubled_area = 0i64;
                for tri in indices.chunks_exact(3) {
                    let [a, b, c] = [point(tri[0]), point(tri[1]), point(tri[2])];
                    let cross = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
                    assert!(cross < 0, "inverted/degenerate fill triangle: stride={stride}");
                    doubled_area += i64::from(-cross);
                    for (a, b) in [(a, b), (b, c), (c, a)] {
                        *edges.entry(if a < b { (a, b) } else { (b, a) }).or_default() += 1;
                    }
                }
                assert_eq!(doubled_area, i64::from(segments * segments / 2));
                let boundary: Vec<_> = edges.iter().filter(|edge| *edge.1 == 1).collect();
                assert_eq!(boundary.len(), (2 * segments) as usize);
                for (edge, _) in boundary {
                    let (a, b) = *edge;
                    assert_eq!((a.0 - b.0).abs() + (a.1 - b.1).abs(), 1);
                    assert!(a.0.abs() == hole || b.0.abs() == hole
                        || a.1.abs() == hole || b.1.abs() == hole);
                }
                assert!(edges.values().all(|&n| n == 1 || n == 2));
            }
        }
    }

    #[test]
    fn jka_fragment_boundary_preserves_outline_pvs_and_sky_contacts() {
        futures_lite::future::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance
                .request_adapter(&Default::default())
                .await
                .expect("GPU adapter");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    required_limits: adapter.limits(),
                    ..Default::default()
                })
                .await
                .expect("GPU device");
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let points: Vec<[f32; 2]> = (-2..=12)
                .flat_map(|z| (-2..=12).map(move |x| [x as f32 * 10.0, z as f32 * 10.0]))
                .collect();
            let metres: Vec<_> = points.iter().map(|p| [p[0] / 40.0, p[1] / 40.0]).collect();
            let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("JKA boundary sample points"),
                contents: bytemuck::cast_slice(&metres),
                usage: wgpu::BufferUsages::STORAGE,
            });
            let bytes = (points.len() * 4) as u64;
            let output = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("JKA boundary results"),
                size: bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let read = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("JKA boundary readback"),
                size: bytes,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let source = format!(
                "{}\n\
                @group(0) @binding(0) var<storage, read> points: array<vec2<f32>>;\n\
                @group(0) @binding(1) var<storage, read_write> results: array<u32>;\n\
                @compute @workgroup_size(64) fn test_boundary(@builtin(global_invocation_id) id: vec3<u32>) {{\n\
                    if id.x < arrayLength(&points) {{ results[id.x] = u32(jka_water_contains(points[id.x])) | (u32(jka_water_sky_admission(points[id.x])) << 1u) | (u32(jka_water_sun_admission(points[id.x])) << 2u); }}\n\
                }}",
                include_str!("surface_boundary.wgsl")
            );
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("JKA boundary test"),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("JKA boundary test"),
                layout: None,
                module: &shader,
                entry_point: Some("test_boundary"),
                compilation_options: Default::default(),
                cache: None,
            });
            let inputs = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[entry_buffer(0, &input), entry_buffer(1, &output)],
            });
            // A narrow roof occupying half the texture. Test nonzero world origin,
            // above-roof water, disabled/missing admission, and out-of-map samples.
            let roof = device.create_texture_with_data(&queue, &wgpu::TextureDescriptor {
                label: Some("BSP roof admission regression"),
                size: wgpu::Extent3d { width: 2, height: 2, depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
            }, wgpu::util::TextureDataOrder::LayerMajor, bytemuck::cast_slice(&[
                [-1e19f32, 0.0, 0.0, 0.0], [100.0, 0.0, 0.0, 0.0],
                [-1e19, 0.0, 0.0, 0.0], [100.0, 0.0, 0.0, 0.0],
            ]));
            let roof = roof.create_view(&Default::default());
            for case in 0..8 {
                let mut boundary = SurfaceBoundary::zeroed();
                boundary.bounds = [0.0, 0.0, 100.0, 100.0];
                boundary.scale = [40.0, 0.75, 0.0, 0.0];
                if case == 5 || case == 6 {
                    boundary.sky_uv = [10.0, 10.0, 0.01, 0.01];
                    boundary.sky_plane[0] = if case == 6 { 200.0 } else { 0.0 };
                }
                if case == 1 || case == 4 {
                    boundary.counts[0] = 4;
                    boundary.triangles[..8].copy_from_slice(&[
                        [0.0, 0.0, 40.0, 0.0],
                        [40.0, 100.0, 0.0, 0.0],
                        [0.0, 0.0, 40.0, 100.0],
                        [0.0, 100.0, 0.0, 0.0],
                        [40.0, 0.0, 100.0, 0.0],
                        [100.0, 40.0, 0.0, 0.0],
                        [40.0, 0.0, 100.0, 40.0],
                        [40.0, 40.0, 0.0, 0.0],
                    ]);
                    if case == 4 {
                        boundary.counts[3] = 3;
                        boundary.prepare_candidates();
                    }
                } else if case == 2 {
                    boundary.counts[2] = 1;
                    boundary.pvs = [[0xffff0000; 4]; 8];
                } else if case == 3 {
                    boundary.counts[1] = 2;
                    boundary.sky[0] = [0.0, 0.0, 100.0, 1.0];
                    boundary.sky[1] = [0.0, 0.0, 100.0, 3.0];
                }
                let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::bytes_of(&boundary),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &pipeline.get_bind_group_layout(1),
                    entries: &[entry_buffer(16, &uniform), entry_texture(22, &roof)],
                });
                let mut encoder = device.create_command_encoder(&Default::default());
                {
                    let mut pass = encoder.begin_compute_pass(&Default::default());
                    pass.set_pipeline(&pipeline);
                    pass.set_bind_group(0, &inputs, &[]);
                    pass.set_bind_group(1, &group, &[]);
                    pass.dispatch_workgroups((points.len() as u32).div_ceil(64), 1, 1);
                }
                encoder.copy_buffer_to_buffer(&output, 0, &read, 0, bytes);
                queue.submit(Some(encoder.finish()));
                read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: None,
                    })
                    .unwrap();
                {
                    let mapped = read.slice(..).get_mapped_range().unwrap();
                    let actual: &[u32] = bytemuck::cast_slice(&mapped);
                    for (&[x, z], &actual) in points.iter().zip(actual) {
                        let inside = x >= 0.0 && z >= 0.0 && x <= 100.0 && z <= 100.0;
                        let expected = match case {
                            1 | 4 => inside && (x <= 40.0 || z <= 40.0),
                            2 => inside && x >= 50.0,
                            3 => x <= 100.0 && z <= 100.0,
                            _ => inside,
                        };
                        assert_eq!(actual & 1 != 0, expected, "case {case}, point ({x}, {z})");
                        let covered = case == 5 && x >= 60.0 && x < 110.0 && z >= 10.0 && z < 110.0;
                        assert_eq!(actual >> 1, if covered { 0 } else { 3 },
                            "sky/sun admission case {case}, point ({x}, {z})");
                    }
                }
                read.unmap();
            }
            assert!(scope.pop().await.is_none(), "JKA boundary GPU validation");
        });
    }

    #[test]
    fn pruned_indices_keep_the_selected_lod_draw_when_everything_is_admitted() {
        futures_lite::future::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("GPU adapter");
            let (device, _queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .expect("GPU device");
            for (segments, levels) in [(64, 6), (256, 9)] {
                let clipmap = OriginalClipmap::new(&device, segments, levels);
                let frames = clipmap.clipmap().frames(123.4, -56.7);
                for steps in [[1u8; 10], [1, 1, 2, 4, 8, 16, 1, 2, 4, 8]] {
                    for (first, outer, horizon) in [(0, levels - 1, false), (2, levels - 2, false), (1, levels - 1, true)] {
                        let kept = clipmap.pruned_indices(first, outer, &steps, horizon, &frames, 0.0, |_, _| true);
                        assert_eq!(
                            kept.len() as u64 / 3,
                            clipmap.triangle_count_lod(first, outer, &steps, horizon),
                            "segments {segments} levels {levels} first {first} outer {outer} horizon {horizon}"
                        );
                        assert!(kept.len() <= clipmap.pruned_index_capacity());
                    }
                }
                let started = std::time::Instant::now();
                let mut kept = 0;
                for _ in 0..10 {
                    kept = clipmap.pruned_indices(0, levels - 1, &[1; 10], false, &frames, 0.0,
                        |lo, hi| hi[0] > 0.0 && lo[0] < 3000.0 && hi[1] > 0.0 && lo[1] < 3000.0).len();
                }
                eprintln!("prune {segments}x{levels}: {:.2} ms per full rebuild, {kept} indices kept",
                    started.elapsed().as_secs_f64() * 100.0);
            }
        });
    }

    #[test]
    fn cached_prune_matches_a_full_prune_and_repeats_only_moved_levels() {
        futures_lite::future::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions::default()).await.expect("GPU adapter");
            let (device, _queue) = adapter.request_device(&wgpu::DeviceDescriptor::default()).await.expect("GPU device");
            let (segments, levels) = (256u32, 6usize);
            let clipmap = OriginalClipmap::new(&device, segments, levels);
            let source = clipmap.prune_source();
            let classify = |lo: [f32; 2], hi: [f32; 2]| {
                if hi[0] < 0.0 || lo[0] > 900.0 || hi[1] < -300.0 || lo[1] > 400.0 { CELL_REJECTED }
                else if lo[0] > 600.0 { CELL_FAR } else { CELL_NEAR }
            };
            let steps = [1u8, 1, 2, 4, 8, 16, 1, 1, 1, 1];
            let mut cache = PruneSegmentCache::default();
            let (mut full_ms, mut cached_ms, mut partial) = (0.0, 0.0, 0);
            // Walk the camera so different levels re-snap on different steps.
            for step in 0..40 {
                let x = 120.0 + f64::from(step) * 3.7;
                let z = -40.0 + f64::from(step) * 1.9;
                let frames = clipmap.clipmap().frames(x, z);
                for horizon_far in [false, true] {
                    let started = std::time::Instant::now();
                    let expected = clipmap.pruned_indices_classified(0, levels - 1, &steps, 1, true, horizon_far, &frames, 16.0, classify);
                    full_ms += started.elapsed().as_secs_f64() * 1000.0;
                    let started = std::time::Instant::now();
                    let (indices, far_start, rebuilt) = source.pruned_indices_classified_cached(
                        &mut cache, 7, 0, levels - 1, &steps, 1, true, horizon_far, &frames, 16.0, classify);
                    cached_ms += started.elapsed().as_secs_f64() * 1000.0;
                    assert_eq!((indices, far_start), expected, "step {step} horizon_far {horizon_far}");
                    if step > 0 && rebuilt < levels + 1 { partial += 1; }
                }
            }
            // A different admission context must not reuse segments.
            let frames = clipmap.clipmap().frames(120.0, -40.0);
            let (_, _, rebuilt) = source.pruned_indices_classified_cached(
                &mut cache, 8, 0, levels - 1, &steps, 1, true, false, &frames, 16.0, classify);
            assert_eq!(rebuilt, levels + 1);
            assert!(partial > 0, "no step reused any segment");
            eprintln!("prune {segments}x{levels}: full {:.2} ms, cached {:.2} ms per call; {partial} partial rebuilds",
                full_ms / 80.0, cached_ms / 80.0);
        });
    }

    #[test]
    fn pruned_indices_keep_every_triangle_the_shader_places_over_the_region() {
        futures_lite::future::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions::default()).await.expect("GPU adapter");
            let (device, _queue) = adapter.request_device(&wgpu::DeviceDescriptor::default()).await.expect("GPU device");
            let (segments, levels) = (64u32, 6usize);
            let clipmap = OriginalClipmap::new(&device, segments, levels);
            // A 90 m x 40 m pool 300 m from the origin; camera inside it.
            let (lo_r, hi_r) = ([300.0f32, -120.0], [390.0f32, -80.0]);
            let frames = clipmap.clipmap().frames(331.3, -97.9);
            let steps = [1u8; 10];
            let kept = clipmap.pruned_indices(0, levels - 1, &steps, false, &frames, 0.0, |lo, hi| {
                hi[0] >= lo_r[0] && lo[0] <= hi_r[0] && hi[1] >= lo_r[1] && lo[1] <= hi_r[1]
            });
            let kept: std::collections::HashSet<[u32; 3]> = kept.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
            // The vertex shader's snapped lattice position, written independently.
            let shader_position = |vertex: &geometry::OceanVertex| -> [f32; 2] {
                let level = vertex.level[0] as usize;
                let frame = frames[level];
                let lattice = [vertex.position[0] * frame[3], vertex.position[1] * frame[3]];
                let inner = lattice[0].abs().max(lattice[1].abs()) <= segments as f32 / 4.0 && level > 0;
                let finer = frames[level.saturating_sub(1)];
                let shift = if inner { [(finer[0] - frame[0]) * frame[3], (finer[1] - frame[1]) * frame[3]] } else { [0.0; 2] };
                [frame[0] + (lattice[0] + shift[0]) * frame[2], frame[1] + (lattice[1] + shift[1]) * frame[2]]
            };
            let mut ranges = vec![clipmap.fills[0].clone()];
            for level in 0..levels { ranges.push(clipmap.ring_lods[level][0].clone()); }
            let (mut over, mut missing) = (0, 0);
            for range in ranges {
                for tri in clipmap.source_indices[range.start as usize..range.end as usize].chunks_exact(3) {
                    let points: Vec<[f32; 2]> = tri.iter().map(|&i| shader_position(&clipmap.source_vertices[i as usize])).collect();
                    let lo = [points.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min), points.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min)];
                    let hi = [points.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max), points.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max)];
                    if hi[0] >= lo_r[0] && lo[0] <= hi_r[0] && hi[1] >= lo_r[1] && lo[1] <= hi_r[1] {
                        over += 1;
                        if !kept.contains(&[tri[0], tri[1], tri[2]]) { missing += 1; }
                    }
                }
            }
            eprintln!("triangles over the pool: {over}, missing from the pruned set: {missing}");
            assert!(over > 100 && missing == 0);
        });
    }

    /// Every `r_oceanCostProbe` variant must create a valid JKA surface pipeline on the real adapter.
    #[test]
    fn cost_probe_pipelines_create_on_gpu() {
        futures_lite::future::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("GPU adapter");
            eprintln!("adapter: {:?}", adapter.get_info());
            let (device, _queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    required_limits: adapter.limits(),
                    ..Default::default()
                })
                .await
                .expect("GPU device");
            let field = crate::WaveField::new(&device, QualityTier::High);
            let waves = SurfaceWaves::new(&device, &field);
            let mut failures = Vec::new();
            let masks = (0..13).map(|bit| 1u32 << bit)
                .chain([shader_composer::SURFACE_SKIP_FADED_CASCADES, shader_composer::COST_PROBE_PACK_HALF,
                    shader_composer::COST_PROBE_PACK_HALF_WIDE, shader_composer::SURFACE_WAVE_STATISTICS,
                    shader_composer::SURFACE_WAVE_STATISTICS | shader_composer::SURFACE_WAVE_STATISTICS_FOAM]);
            for mask in masks {
                let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
                let _pipeline = OriginalSurfacePipeline::new_with_wave_layout(
                    &device, waves.layout(), wgpu::TextureFormat::Bgra8UnormSrgb, 1, 16, 0, mask);
                device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
                if let Some(error) = scope.pop().await {
                    eprintln!("probe {mask:#x}: {error}");
                    failures.push(mask);
                }
            }
            assert!(failures.is_empty(), "cost probe pipelines failed: {failures:x?}");
            // Every `r_oceanPixelDetail` mode's reduced, reconstruction and detail pipelines.
            for mode in 1..=5u8 {
                let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
                let pipeline = OriginalSurfacePipeline::new_with_wave_layout(
                    &device, waves.layout(), wgpu::TextureFormat::Rgba16Float, 1, 16, mode, 0);
                device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
                assert!(scope.pop().await.is_none(), "pixel detail {mode} pipelines invalid");
                assert_eq!(pipeline.near_pipeline.is_some(), matches!(mode, 3..=5), "pixel detail {mode} detail pass");
                assert_eq!(pipeline.adaptive_classify.is_some(), mode == 5, "pixel detail {mode} tile classifier");
            }
        });
    }

    /// Driver register counts for surface variants, read through the vendored wgpu-hal's `DINURDO_VK_SHADER_STATS`
    /// diagnostic (VK_KHR_pipeline_executable_properties). Prints only; skipped unless that variable is 1. Masks
    /// beyond the defaults come from `JKA_SURFACE_STATS_MASKS` (comma-separated, hex with 0x or decimal).
    #[test]
    fn surface_register_stats() {
        if std::env::var_os("DINURDO_VK_SHADER_STATS").is_none_or(|v| v != "1") {
            return;
        }
        let parse = |text: &str| match text.trim().strip_prefix("0x") {
            Some(hex) => u32::from_str_radix(hex, 16).ok(),
            None => text.trim().parse().ok(),
        };
        let masks: Vec<u32> = std::env::var("JKA_SURFACE_STATS_MASKS").ok()
            .map(|list| list.split(',').filter_map(parse).collect())
            .unwrap_or_else(|| vec![0, shader_composer::COST_PROBE_SSR, shader_composer::COST_PROBE_FOAM,
                shader_composer::COST_PROBE_WAVES, shader_composer::COST_PROBE_GLINTS, shader_composer::COST_PROBE_REFRACTION,
                shader_composer::COST_PROBE_SKY, shader_composer::COST_PROBE_FLOOR]);
        futures_lite::future::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::VULKAN,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions::default()).await.expect("Vulkan adapter");
            eprintln!("adapter: {:?}", adapter.get_info());
            let (device, _queue) = adapter.request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(),
                ..Default::default()
            }).await.expect("GPU device");
            let field = crate::WaveField::new(&device, QualityTier::High);
            let waves = SurfaceWaves::new(&device, &field);
            for mask in masks {
                eprintln!("[SURFACE MASK] {mask:#x}");
                let _pipeline = OriginalSurfacePipeline::new_with_wave_layout(
                    &device, waves.layout(), wgpu::TextureFormat::Rgba16Float, 1, 16, 0, mask);
                device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
            }
        });
    }

    #[test]
    fn original_surface_pipeline_accepts_explicit_jka_bind_group_layouts() {
        futures_lite::future::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("GPU adapter");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    required_limits: adapter.limits(),
                    ..Default::default()
                })
                .await
                .expect("GPU device");
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let field = crate::WaveField::new(&device, QualityTier::High);
            let waves = SurfaceWaves::new(&device, &field);
            let pipeline =
                OriginalSurfacePipeline::new(&device, &waves, wgpu::TextureFormat::Rgba16Float, 1, 16);
            let bindings = OriginalSurfaceBindings::new(&device, &queue, &pipeline);
            let clipmap = OriginalClipmap::new(&device, 128, 9);
            let mut camera = SurfaceCamera::zeroed();
            for matrix in [
                &mut camera.view,
                &mut camera.projection,
                &mut camera.view_projection,
                &mut camera.inverse_view,
                &mut camera.inverse_projection,
                &mut camera.inverse_view_projection,
            ] {
                for (axis, row) in matrix.iter_mut().enumerate() {
                    row[axis] = 1.0;
                }
            }
            camera.position_time = [0.0, 1.0, 0.0, 0.0];
            camera.viewport = [16.0, 16.0, 1.0 / 16.0, 1.0 / 16.0];
            camera.near = 0.025;
            camera.far = 100.0;
            camera.tan_half_fov_y = 0.5;
            camera.aspect = 1.0;
            bindings.write_camera(&queue, &camera);
            bindings.write_surface_prefix(
                &queue,
                &clipmap.surface_prefix([0.0, 0.0], 1.0, SurfaceAppearance::default()),
            );
            let target = |format| {
                device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("Blue Water surface draw validation"),
                        size: wgpu::Extent3d {
                            width: 16,
                            height: 16,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    })
                    .create_view(&Default::default())
            };
            let color = target(wgpu::TextureFormat::Rgba16Float);
            let depth = target(wgpu::TextureFormat::Depth32Float);
            let water_distance = target(wgpu::TextureFormat::R32Float);
            let scene = crate::scene_copy::SceneCopy::new(&device, 16, 16, 1);
            let underwater_pipelines = crate::underwater::UnderwaterPipelines::new(
                &device,
                waves.layout(),
                wgpu::TextureFormat::Rgba16Float,
                1,
            );
            let underwater = crate::underwater::UnderwaterBindings::new(
                &device,
                &underwater_pipelines,
                &bindings,
                &scene.depth,
                &water_distance,
            );
            underwater.write(
                &queue,
                &camera,
                SurfaceAppearance::default(),
                [0.0, 1.0, 0.0],
                [1.0; 3],
                false,
            );
            let mut encoder = device.create_command_encoder(&Default::default());
            underwater.probe(&mut encoder, &underwater_pipelines, waves.group(&field));
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Blue Water original clipmap draw validation"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &color,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(0.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });
                pass.set_pipeline(&pipeline.pipeline);
                pass.set_bind_group(0, &bindings.camera_group, &[]);
                pass.set_bind_group(1, &bindings.material_group, &[]);
                pass.set_bind_group(2, waves.group(&field), &[]);
                pass.set_bind_group(3, &bindings.environment_group, &[]);
                clipmap.draw(&mut pass, 0, 0);
            }
            scene.capture(&device, &mut encoder, &color, &depth);
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Original underwater dry-view draw validation"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &color,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });
                underwater.draw(&mut pass, &underwater_pipelines, true);
            }
            queue.submit(Some(encoder.finish()));
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            assert!(
                scope.pop().await.is_none(),
                "original Blue Water surface layout"
            );
        });
    }
}

/// One lattice tile may choose a different density from the tile beside it.
/// Existing Blue Water GPU displacement is used for *both* the old and new
/// tessellation during a transition; the CPU never attempts to read back FFT.
#[derive(Clone, Copy, Debug)]
pub struct LocalWaveHint {
    pub height: f32,
    pub wavelength: f32,
    pub choppiness: f32,
    pub wind_speed: f32,
    pub fft_weight: f32,
    pub swell: [f32; 4],
    /// Multiplies both geometry-error limits; 1.0 preserves original mode 2.
    pub tolerance: f32,
    pub seconds: f32,
    pub focal_pixels: f32,
    pub eye: [f32; 3],
}

#[derive(Default)]
pub struct LocalTileState {
    epoch: i64,
    tiles: std::collections::BTreeMap<(usize, i32, i32), (u8, u8)>,
}

#[derive(Clone, Copy)]
struct LocalTile {
    level: usize,
    x: i32,
    y: i32,
    from: u8,
    to: u8,
}

impl OriginalClipmap {
    /// CPU prototype of adaptive tiled tessellation. Each 16x16 cell tile is
    /// independently selected, with 2:1 neighbour enforcement. Patch boundaries
    /// use the finer of their two steps. A vertex encodes its *coarse* parent
    /// step and morph progress in the normally unused clipmap `level.y`; the
    /// unchanged JKA vertex layout is shared by all native surface pipelines.
    ///
    /// IMPORTANT: the selection is a phase-aware *spectrum estimate*, not a
    /// GPU FFT curvature readback. The shader performs the actual wave-aware
    /// geomorph and all JKA PVS/footprint tests are retained.
    pub fn local_adaptive_mesh(
        &self, first: usize, outer: usize, horizon: bool,
        frames: &[[f32; 4]; 10], screen_steps: &[u8; 10],
        hint: LocalWaveHint, state: &mut LocalTileState,
        mut admit: impl FnMut([f32; 2], [f32; 2]) -> bool,
    ) -> (Vec<geometry::OceanVertex>, Vec<u32>, [u32; 5]) {
        const TILE: i32 = 16;
        const TRANSITION: f32 = 0.5;
        let clip = self.clipmap;
        let first = first.min(clip.levels - 1);
        let last = if horizon { clip.levels - 1 } else { outer.max(first).min(clip.levels - 1) };
        let epoch = (hint.seconds.max(0.0) / TRANSITION).floor() as i64;
        let phase = ((hint.seconds.max(0.0) / TRANSITION).fract()).clamp(0.0, 1.0);
        let progress = phase * phase * (3.0 - 2.0 * phase);
        let mut candidate = std::collections::BTreeMap::<(usize,i32,i32),u8>::new();
        let half = (clip.segments / 2) as i32;
        let hole = (clip.segments / 4) as i32;
        for level in first..=last {
            for y in (-half..half).step_by(TILE as usize) {
                for x in (-half..half).step_by(TILE as usize) {
                    if level != first && x >= -hole && x < hole && y >= -hole && y < hole { continue; }
                    let frame = frames[level];
                    let cell = frame[2];
                    let cx = frame[0] + (x as f32 + TILE as f32 * 0.5) * cell;
                    let cy = frame[1] + (y as f32 + TILE as f32 * 0.5) * cell;
                    let d = ((cx - hint.eye[0]).hypot(cy - hint.eye[2]))
                        .hypot(hint.eye[1].abs()).max(cell);
                    let wl = hint.wavelength.max(2.0) / (1.0 + 0.8 *
                        (hint.wind_speed.max(0.0) * hint.choppiness.max(0.0) / 12.0).sqrt());
                    let k = std::f32::consts::TAU / wl;
                    // This local phase is an inexpensive curvature proxy, NOT
                    // a claim that we recovered the stochastic FFT phases.
                    let angle = k * (cx * 0.73 + cy * 0.68) -
                        hint.seconds * (9.81 * k).sqrt();
                    let coverage = (k * cell * TILE as f32 * 0.7).min(1.0);
                    let fft = hint.height.max(0.0) * hint.fft_weight.max(0.0)
                        * k*k * (angle.sin().abs() + coverage).min(1.0);
                    let ak = hint.swell[2].abs();
                    let aphase = ak * (cx * hint.swell[0] + cy * hint.swell[1]) -
                        hint.seconds * (9.81 * ak).sqrt();
                    let analytic = hint.swell[3].abs() * ak*ak
                        * (aphase.sin().abs() + (ak*cell*TILE as f32*0.7).min(1.0)).min(1.0);
                    let curvature = fft + analytic;
                    // Expose the existing mode-2 error limits as one predictable
                    // control. Increasing tolerance chooses coarser quads.
                    let tolerance = if hint.tolerance.is_finite() {
                        hint.tolerance.clamp(0.25, 16.0)
                    } else { 1.0 };
                    let mut step = screen_steps[level].clamp(1, 16);
                    if curvature.is_finite() {
                        while step < 16 {
                            let test = step * 2;
                            let width = test as f32 * cell;
                            let projected_error = hint.focal_pixels * curvature * width * width / (8.0 * d);
                            let normal_error = curvature * width;
                            if projected_error > 3.0 * tolerance || normal_error > 0.35 * tolerance { break; }
                            step = test;
                        }
                    }
                    candidate.insert((level,x,y),step);
                }
            }
        }
        // Bound neighbouring tessellation steps to 2:1. This guarantees that
        // the border adapter never has to stitch radically different grids.
        for _ in 0..5 {
            let mut changed = false;
            let old = candidate.clone();
            for (&(l,x,y), step) in &mut candidate {
                for nb in [(l,x-TILE,y),(l,x+TILE,y),(l,x,y-TILE),(l,x,y+TILE)] {
                    if let Some(&n) = old.get(&nb) {
                        let limited = n.saturating_mul(2).min(16);
                        if *step > limited { *step = limited; changed = true; }
                    }
                }
            }
            if !changed { break; }
        }
        if epoch != state.epoch || state.tiles.len() != candidate.len() {
            let mut next = std::collections::BTreeMap::new();
            for (key, want) in candidate {
                let from = state.tiles.get(&key).map_or(want, |&(_, to)| to);
                // Only move one subdivision grade per transition. This avoids
                // large bursts of new vertices and is more stable on waves.
                let to = if want > from { (from * 2).min(want) }
                    else if want < from { (from / 2).max(want) } else { want };
                next.insert(key, (from, to));
            }
            state.tiles = next;
            state.epoch = epoch;
        }
        let tiles: Vec<_> = state.tiles.iter().map(|(&(l,x,y), &(from,to))|
            LocalTile { level:l, x, y, from, to }).collect();
        let mut vertices: Vec<geometry::OceanVertex> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut counts = [0u32; 5];
        for tile in &tiles {
            let frame = frames[tile.level];
            let cell = frame[2];
            let x = tile.x;
            let y = tile.y;
            // Conservatively admit the whole tile. Actual coast/PVS shape is
            // still evaluated in the fragment shader as before.
            let bounds_pad = cell * 2.0 + 16.0;
            let lo = [frame[0] + x as f32 * cell - bounds_pad,
                frame[1] + y as f32 * cell - bounds_pad];
            let hi = [frame[0] + (x+TILE) as f32 * cell + bounds_pad,
                frame[1] + (y+TILE) as f32 * cell + bounds_pad];
            if !admit(lo, hi) { continue; }
            let key = |nx,ny| (tile.level, nx, ny);
            let neighbor = |nx,ny| state.tiles.get(&key(nx,ny)).copied();
            let edges = [neighbor(x-TILE,y), neighbor(x+TILE,y),
                neighbor(x,y-TILE), neighbor(x,y+TILE)];
            let edge_levels = edges.map(|other| match other {
                Some((from,to)) => (tile.from.min(from), tile.to.min(to)),
                None => (1,1), // clipmap holes / exterior must retain every border vertex
            });
            let active = tile.from.min(tile.to) as i32;
            let parent = tile.from.max(tile.to);
            counts[(active as u8).ilog2().min(4) as usize] += 1;
            let blend = if tile.from > tile.to { 1.0-progress }
                else if tile.from < tile.to { progress } else { 0.0 };
            // Tile-local vertices: independently varying morph weights must
            // never be shared between adjacent patches on the GPU.
            let mut local = std::collections::BTreeMap::<(i32,i32),u32>::new();
            let mut push_vertex = |vx: i32, vy: i32| -> u32 {
                if let Some(&id) = local.get(&(vx,vy)) { return id; }
                let (mut parent_step, mut coarse_blend) = (parent, blend);
                // Both patches touching an edge encode the *same* old/new
                // edge step and interpolation, keeping the seam watertight.
                for (edge, on) in [vy == y, vx == x+TILE, vy == y+TILE, vx == x].iter().enumerate() {
                    if *on {
                        let (a,b) = edge_levels[[2,1,3,0][edge]];
                        let p = a.max(b);
                        let factor = if a > b { 1.0-progress }
                            else if a < b { progress } else { 0.0 };
                        if p > parent_step || parent_step == tile.from.max(tile.to) {
                            parent_step = p; coarse_blend = factor;
                        }
                    }
                }
                // Tile corners exist at every supported LOD (1, 2, 4, 8,
                // 16).  They must therefore NEVER be geomorphed: a corner
                // is duplicated by up to four independently transitioning
                // tiles, and their edge-specific parent steps can differ.
                // Moving that shared corner separately opens cracks even
                // though all four indices refer to the same grid location.
                // Keeping it at its exact displaced position is both seamless
                // and pop-free, since no LOD ever removes a tile corner.
                let permanent_corner = (vx == x || vx == x + TILE)
                    && (vy == y || vy == y + TILE);
                let meta = if !permanent_corner && parent_step > 1 && coarse_blend > 0.0 {
                    1024.0 + 2.0 * parent_step as f32 + coarse_blend
                } else { 0.0 };
                let id = vertices.len() as u32;
                vertices.push(geometry::OceanVertex {
                    position:[vx as f32 * cell, vy as f32 * cell],
                    level:[tile.level as f32, meta],
                });
                local.insert((vx,vy), id);
                id
            };
            for by in (y..y+TILE).step_by(active as usize) {
                for bx in (x..x+TILE).step_by(active as usize) {
                    if active == 1 {
                        let a=push_vertex(bx,by);
                        let b=push_vertex(bx+1,by);
                        let c=push_vertex(bx+1,by+1);
                        let d=push_vertex(bx,by+1);
                        // Match the original upstream clockwise winding.
                        indices.extend_from_slice(&[a,c,b,a,d,c]);
                        continue;
                    }
                    let left = if bx==x {edge_levels[0].0.min(edge_levels[0].1) as i32} else {active};
                    let right = if bx+active==x+TILE {edge_levels[1].0.min(edge_levels[1].1) as i32} else {active};
                    let bottom = if by==y {edge_levels[2].0.min(edge_levels[2].1) as i32} else {active};
                    let top = if by+active==y+TILE {edge_levels[3].0.min(edge_levels[3].1) as i32} else {active};
                    // Coarse fan with independently stitched edges, never T-junctions.
                    let mut edge = Vec::<u32>::with_capacity(4*active as usize+1);
                    edge.push(push_vertex(bx,by));
                    for py in (by+left..=by+active).step_by(left as usize) {edge.push(push_vertex(bx,py));}
                    for px in (bx+top..=bx+active).step_by(top as usize) {edge.push(push_vertex(px,by+active));}
                    for py in (by..by+active).rev() {
                        if (by+active-py)%right==0 {edge.push(push_vertex(bx+active,py));}
                    }
                    for px in (bx..bx+active).rev() {
                        if (bx+active-px)%bottom==0 && px>bx {edge.push(push_vertex(px,by));}
                    }
                    let centre = push_vertex(bx+active/2,by+active/2);
                    for k in 0..edge.len() {
                        indices.extend_from_slice(&[centre,edge[k],edge[(k+1)%edge.len()]]);
                    }
                }
            }
        }
        if horizon {
            // Keep the untouched vendor horizon/skirt vertex geometry.
            let mut remap = std::collections::BTreeMap::<u32, u32>::new();
            for &old in &self.source_indices[self.horizon_start as usize..self.index_count as usize] {
                let new = *remap.entry(old).or_insert_with(|| {
                    let id = vertices.len() as u32;
                    vertices.push(self.source_vertices[old as usize]);
                    id
                });
                indices.push(new);
            }
        }
        (vertices, indices, counts)
    }
}
