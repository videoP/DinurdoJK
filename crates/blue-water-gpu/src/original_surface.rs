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

pub struct OriginalSurfacePipeline {
    pub camera_layout: wgpu::BindGroupLayout,
    pub surface_layout: wgpu::BindGroupLayout,
    pub environment_layout: wgpu::BindGroupLayout,
    pub pipeline: wgpu::RenderPipeline,
    pub reduced_pipeline: Option<wgpu::RenderPipeline>,
    pub near_pipeline: Option<wgpu::RenderPipeline>,
    pub reconstruct_pipeline: Option<wgpu::RenderPipeline>,
    pub reconstruct_layout: Option<wgpu::BindGroupLayout>,
    pub wireframe: Option<wgpu::RenderPipeline>,
    pub waterline_marker: Option<wgpu::RenderPipeline>,
    // Keep the existing compiled vertex entry to compile the debug overlay
    // lazily; no wireframe PSO when r_showtris ocean is disabled.
    vertex_shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
}

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
    ring_lods: Vec<[std::ops::Range<u32>; 5]>,
    // Independent centre-fill LODs; without these the nearest flat-water
    // rectangle would remain fully tessellated even as its rings simplify.
    fill_lods: Vec<[std::ops::Range<u32>; 5]>,
    // CPU copy of upstream lattice/index variants for footprint/PVS pruning.
    // The GPU vertex buffer and its wave displacement shader stay unchanged.
    source_vertices: Vec<geometry::OceanVertex>,
    source_indices: Vec<u32>,
    prune_capacity: usize,
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
            ring_lods,
            fill_lods,
            source_indices,
            source_vertices,
            prune_capacity,
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
        let first = first.min(self.clipmap.levels - 1);
        let last = if horizon { self.clipmap.levels - 1 }
            else { outer.max(first).min(self.clipmap.levels - 1) };
        let mut out = Vec::with_capacity((self.prune_capacity / 8).max(4096));
        // Reconstruct the upstream vertex shader's snapped grid once per
        // lattice crossing, not three times for every triangle. The shader's
        // continuous 0..1 geomorph moves vertices by at most one cell, which
        // is covered by the conservative bounds expansion below.
        let positions: Vec<[f32; 2]> = self.source_vertices.iter().map(|vertex| {
            let level = (vertex.level[0] as usize).min(self.clipmap.levels - 1);
            let frame = frames[level];
            let inner = level > 0 && vertex.position.iter().all(|&v|
                v.abs() <= self.clipmap.segments as f32 * frame[2] / 4.0 + 1.0e-3);
            let centre = if inner { frames[level - 1] } else { frame };
            [centre[0] + vertex.position[0], centre[1] + vertex.position[1]]
        }).collect();
        let mut prune_range = |range: &std::ops::Range<u32>, original_quads: bool| {
            // The upstream fill and stride-1 rings are six indices per cell.
            // Godot admits the entire cell conservatively; test once per
            // quad, instead of separately discarding half of a wet quad.
            // Stitched outer LOD variants contain triangle fans, so their
            // groups must instead be examined triangle-by-triangle.
            let stride = if original_quads { 6 } else { 3 };
            for indices in self.source_indices[range.start as usize..range.end as usize]
                .chunks_exact(stride)
            {
                let mut lo = [f32::INFINITY; 2];
                let mut hi = [f32::NEG_INFINITY; 2];
                for &vertex_index in indices {
                    let point = positions[vertex_index as usize];
                    for axis in 0..2 {
                        lo[axis] = lo[axis].min(point[axis]);
                        hi[axis] = hi[axis].max(point[axis]);
                    }
                }
                let level = (self.source_vertices[indices[0] as usize].level[0] as usize)
                    .min(self.clipmap.levels - 1);
                let pad = frames[level][2] * 1.05 + extra_pad;
                for axis in 0..2 { lo[axis] -= pad; hi[axis] += pad; }
                if admit(lo, hi) { out.extend_from_slice(indices); }
            }
        };
        let fill_variant = fill_step.clamp(1, 16).ilog2().min(4) as usize;
        prune_range(&self.fill_lods[first][fill_variant], fill_variant == 0);
        for level in first..=last {
            let variant = steps[level].clamp(1, 16).ilog2().min(4) as usize;
            prune_range(&self.ring_lods[level][variant], variant == 0);
        }
        // Horizon/skirt is valid only under the sky-contact gate; the finite
        // footprint callback cannot bound its far-plane vertices conservatively.
        if horizon {
            out.extend_from_slice(&self.source_indices[self.horizon_start as usize..self.index_count as usize]);
        }
        out
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
    ) {
        if index_count == 0 { return; }
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..index_count, 0, 0..1);
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
    pub counts: [u32; 4],
    pub scale: [f32; 4],
    pub tiles: [u32; 4],
    pub pvs: [[u32; 4]; 8],
    pub triangles: [[f32; 4]; 512],
    pub sky: [[f32; 4]; 16],
    /// Eight 32-bit candidate masks per 8x8 tile; exact edge tests still decide coverage.
    pub candidates: [[u32; 4]; 128],
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
        let hybrid_swell = buffer(device, "JKA Hybrid Gerstner", 32, false);
        let pure_fft: [[f32; 4]; 2] = [[0.0; 4], [0.0, 0.0, 1.0, 0.0]];
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
            ],
        })
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
            color,
            depth,
        );
        self.current_scene = Some(next);
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
        Self::new_with_wave_layout(device, waves.layout(), color_format, samples, reflection_steps, pixel_detail_mode)
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
        let mut composer = shader_composer::composer().expect("Blue Water shader modules");
        let source = shader_composer::jka_surface_module(&mut composer, reflection_steps, pixel_detail_mode)
            .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&composer)));
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
                targets: &[Some(color_format.into())],
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
                    targets: &[Some(wgpu::TextureFormat::Rgba16Float.into())],
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
            let reconstruction = shader_composer::jka_reconstruct_module(&mut composer)
                .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&composer)));
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
                let near_module = shader_composer::jka_near_surface_module(&mut composer, reflection_steps)
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
            wireframe: None,
            waterline_marker: None,
            vertex_shader: shader,
            pipeline_layout: layout,
        }
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
                    if id.x < arrayLength(&points) {{ results[id.x] = u32(jka_water_contains(points[id.x])); }}\n\
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
            for case in 0..5 {
                let mut boundary = SurfaceBoundary::zeroed();
                boundary.bounds = [0.0, 0.0, 100.0, 100.0];
                boundary.scale = [40.0, 0.75, 0.0, 0.0];
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
                    entries: &[entry_buffer(16, &uniform)],
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
                        assert_eq!(actual != 0, expected, "case {case}, point ({x}, {z})");
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
