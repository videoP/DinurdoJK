//! Competitive world A/B. Shitter reuses the normal world BSP GPU geometry,
//! PVS, index ranges and indirect draw calls, with a flat no-texture shader.
//! Steam Audio and gameplay collision are independent of this path.
use super::{depth_format, PvsMode, Renderer};
use crate::scene;
use std::sync::{mpsc, Arc};
use std::path::PathBuf;
use std::collections::BTreeMap;
use std::ops::Range;
use wgpu::util::DeviceExt;

/// One tiny indirect stream for the native BSP geometry. Allocated lazily only
/// while Shitter is active. No duplicate vertex/index mesh is ever uploaded.
pub(in crate::renderer) struct NativeFlatDraw {
    pub buffer: wgpu::Buffer,
    pub capacity: usize,
    pub count: u32,
    uploaded: Vec<u8>,
}

/// The original BSP PVS/area/frustum-selected draw list is already available.
/// Only the draw arguments are compacted, never the geometry itself. Skip the
/// queue upload when the list is unchanged, including stationary-camera frames.
pub(in crate::renderer) fn update_native_flat_draw(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    cached: &mut Option<NativeFlatDraw>,
    draws: &[super::DrawIndexedIndirectArgs],
) {
    let Ok(count) = u32::try_from(draws.len()) else { return; };
    if count == 0 {
        if let Some(cached) = cached.as_mut() { cached.count = 0; }
        return;
    }
    let bytes = bytemuck::cast_slice(draws);
    if cached.as_ref().is_none_or(|cached| cached.capacity < draws.len()) {
        let capacity = draws.len().next_power_of_two();
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Native BSP Shitter multi-draw arguments"),
            size: (capacity * std::mem::size_of::<super::DrawIndexedIndirectArgs>()) as u64,
            usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        *cached = Some(NativeFlatDraw { buffer, capacity, count: 0, uploaded: Vec::new() });
    }
    let cached = cached.as_mut().expect("native flat indirect buffer allocated");
    if cached.uploaded.as_slice() != bytes {
        queue.write_buffer(&cached.buffer, 0, bytes);
        cached.uploaded.clear();
        cached.uploaded.extend_from_slice(bytes);
    }
    cached.count = count;
}

/// A spatially bounded contiguous index range. Culling it costs no GPU memory
/// copies and never uploads a new per-frame index buffer.
pub(in crate::renderer) struct CollisionChunk {
    pub range: Range<u32>,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    /// Conservative RBSP bounds, converted once from renderer Y-up to BSP Z-up.
    pub bsp_bounds_min: [f32; 3],
    pub bsp_bounds_max: [f32; 3],
}

/// Inverse of `scene::render_position`: [x, z, -y] back to JKA [x, -z, y].
/// Use the extremal coordinates, not the center: brush faces can touch both
/// sides of a BSP split/areaportal, and culling those faces would leave holes.
fn collision_bsp_bounds(min: [f32; 3], max: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    (
        [min[0], -max[2], min[1]],
        [max[0], -min[2], max[1]],
    )
}

pub(in crate::renderer) struct CollisionGpuMesh {
    pub vertices: wgpu::Buffer,
    pub indices: wgpu::Buffer,
    pub index_count: u32,
    pub gpu_bytes: usize,
    pub chunks: Vec<CollisionChunk>,
    /// Cached against (PVS mode, camera cluster, server areamask). No BSP tree
    /// traversal at all during ordinary frames in the same cluster/area state.
    selection_key: Option<(PvsMode, Option<usize>, Option<[u8; 32]>)>,
    visible_chunks: Vec<bool>,
    pub pvs_rejected_chunks: u32,
}

impl CollisionGpuMesh {
    /// Reuse the exact BSP visibility/areaportal primitive used by the native
    /// inline-model renderer. It walks all touched leaves, not a triangle
    /// centroid (which may sit inside a solid brush).
    pub fn refresh_pvs(
        &mut self,
        visibility: Option<&jka_assets::bsp::Visibility>,
        mode: PvsMode,
        camera_cluster: Option<usize>,
        area_mask: Option<&[u8; 32]>,
    ) {
        let cluster = if mode == PvsMode::Off { None } else { camera_cluster };
        let mask = if cluster.is_some() { area_mask.copied() } else { None };
        let key = (mode, cluster, mask);
        if self.selection_key == Some(key) { return; }
        self.selection_key = Some(key);
        let mut rejected = 0_u32;
        for (chunk, allowed) in self.chunks.iter().zip(&mut self.visible_chunks) {
            *allowed = visibility.is_none_or(|vis| vis.box_visible(
                cluster, chunk.bsp_bounds_min, chunk.bsp_bounds_max, mask.as_ref(),
            ));
            rejected += u32::from(!*allowed);
        }
        self.pvs_rejected_chunks = rejected;
    }

    /// The world batches and collision indices are intentionally immutable.
    /// Only contiguous, PVS- and frustum-accepted index ranges are submitted.
    /// Combining neighbors avoids the draw-call overhead of one draw per cell.
    pub fn draw_visible(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        frustum: &super::lighting::local_shadows::ClipFrustum,
    ) {
        let mut run: Option<Range<u32>> = None;
        for (chunk, &pvs_visible) in self.chunks.iter().zip(&self.visible_chunks) {
            if !pvs_visible || !frustum.intersects_aabb(chunk.bounds_min, chunk.bounds_max) {
                if let Some(indices) = run.take() {
                    pass.draw_indexed(indices, 0, 0..1);
                }
                continue;
            }
            match &mut run {
                Some(indices) if indices.end == chunk.range.start => {
                    indices.end = chunk.range.end;
                }
                _ => {
                    if let Some(indices) = run.take() {
                        pass.draw_indexed(indices, 0, 0..1);
                    }
                    run = Some(chunk.range.clone());
                }
            }
        }
        if let Some(indices) = run {
            pass.draw_indexed(indices, 0, 0..1);
        }
    }
}

struct IndexChunk {
    indices: Vec<u32>,
    bounds_min: [f32; 3],
    bounds_max: [f32; 3],
}

/// Uploaded only when competitive world rendering is requested. The normal
/// OpenJK player/prediction collision and Steam Audio remain independent.
///
/// A spatial cell stores complete triangles and their *true* bounds. Large
/// triangles are never incorrectly culled simply because their centroid lies
/// outside the camera: their bounds encompass every corner.
pub(in crate::renderer) fn upload(
    device: &wgpu::Device,
    mesh: &jka_assets::bsp::CompetitiveMesh,
) -> Option<CollisionGpuMesh> {
    if mesh.vertices.is_empty() || mesh.triangles.is_empty() {
        return None;
    }
    // World-space coordinates are the exact same transformation as the normal
    // renderer. A light facing up gets the highest intensity; wall directions
    // get lower ones. Faces remain visible from either side with cull_mode=None.
    let positions: Vec<[f32; 3]> = mesh.vertices.iter()
        .copied().map(crate::scene::render_position).collect();
    let mut shade_sum = vec![0.0_f32; positions.len()];
    let mut weight_sum = vec![0.0_f32; positions.len()];
    // Do not split triangles or introduce per-fragment normal calculation.
    // Shading is assigned once to compacted position-only vertices.
    // Smaller than the old 1536-unit cells: on corridor maps those broad
    // AABBs intersected far too many PVS leaves and were rarely rejected.
    // Indexed runs are coalesced at draw time, keeping submission cheap.
    const CELL_SIZE: f32 = 512.0;
    let mut cells: BTreeMap<(i32, i32, i32), IndexChunk> = BTreeMap::new();
    for &tri in &mesh.triangles {
        let [a, b, c] = tri.map(|index| usize::try_from(index).ok())
            .map(|index| index.and_then(|index| positions.get(index).copied()));
        let (Some(a), Some(b), Some(c)) = (a, b, c) else { return None; };
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let area2 = n[0].hypot(n[1]).hypot(n[2]);
        if !area2.is_finite() || area2 < 1.0e-8 { continue; }
        // Restrained grays in linear scene color: the old 0.43..0.99
        // appeared near-white after output transfer, hiding the player.
        // Keep this entirely in the one-time mesh generation step.
        let brightness = 0.16 + 0.20 * (n[1].abs() / area2)
            + 0.06 * (n[2].abs() / area2);
        for &i in &tri {
            let index = i as usize;
            shade_sum[index] += brightness * area2;
            weight_sum[index] += area2;
        }
        let centroid = [
            (a[0] + b[0] + c[0]) / 3.0,
            (a[1] + b[1] + c[1]) / 3.0,
            (a[2] + b[2] + c[2]) / 3.0,
        ];
        let key = (
            (centroid[0] / CELL_SIZE).floor() as i32,
            (centroid[1] / CELL_SIZE).floor() as i32,
            (centroid[2] / CELL_SIZE).floor() as i32,
        );
        let cell = cells.entry(key).or_insert_with(|| IndexChunk {
            indices: Vec::new(),
            bounds_min: [f32::INFINITY; 3],
            bounds_max: [f32::NEG_INFINITY; 3],
        });
        cell.indices.extend_from_slice(&tri);
        for point in [a, b, c] {
            for axis in 0..3 {
                cell.bounds_min[axis] = cell.bounds_min[axis].min(point[axis]);
                cell.bounds_max[axis] = cell.bounds_max[axis].max(point[axis]);
            }
        }
    }
    if cells.is_empty() { return None; }
    let vertices: Vec<[f32; 4]> = positions.into_iter().enumerate().map(|(i, p)| {
        let shade = if weight_sum[i] > 0.0 {
            (shade_sum[i] / weight_sum[i]).clamp(0.0, 1.0)
        } else { 0.65 };
        [p[0], p[1], p[2], shade]
    }).collect();
    let total_indices = cells.values().try_fold(0_usize, |sum, cell| {
        sum.checked_add(cell.indices.len())
    })?;
    let index_count = u32::try_from(total_indices).ok()?;
    let mut indices = Vec::with_capacity(total_indices);
    let mut chunks = Vec::with_capacity(cells.len());
    for (_, cell) in cells {
        let first = u32::try_from(indices.len()).ok()?;
        indices.extend_from_slice(&cell.indices);
        let end = u32::try_from(indices.len()).ok()?;
        let (bsp_bounds_min, bsp_bounds_max) =
            collision_bsp_bounds(cell.bounds_min, cell.bounds_max);
        chunks.push(CollisionChunk {
            range: first..end,
            bounds_min: cell.bounds_min,
            bounds_max: cell.bounds_max,
            bsp_bounds_min,
            bsp_bounds_max,
        });
    }
    let gpu_bytes = vertices.len() * std::mem::size_of::<[f32; 4]>()
        + indices.len() * std::mem::size_of::<u32>();
    let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Competitive visible BSP position + precomputed shade"),
        contents: bytemuck::cast_slice(&vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Spatially grouped competitive BSP indices"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    let visible_chunks = vec![true; chunks.len()];
    Some(CollisionGpuMesh {
        vertices, indices, index_count, gpu_bytes, chunks, selection_key: None,
        visible_chunks, pvs_rejected_chunks: 0,
    })
}

/// Contiguous runs of triangles with the same authored face slope. All run
/// boundaries are in the already-resident native BSP index buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::renderer) struct ShitterSlopeRun {
    pub first_index: u32,
    pub index_count: u32,
    pub gray: bool,
}

/// Created only on entering textured ShitteR for a map. The normal renderer
/// stores NONE of this cache. AUTO4 collapsed ranges are cached when installed
/// (not reclassified every frame); all original ranges use the immutable table.
pub(in crate::renderer) struct ShitterSlopeCache {
    static_runs: Vec<ShitterSlopeRun>,
    dynamic_runs: BTreeMap<u32, Vec<ShitterSlopeRun>>,
    static_end: u32,
}

fn shitter_face_gray(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> bool {
    let ab = [b[0]-a[0], b[1]-a[1], b[2]-a[2]];
    let ac = [c[0]-a[0], c[1]-a[1], c[2]-a[2]];
    let cross = [
        ab[1]*ac[2] - ab[2]*ac[1],
        ab[2]*ac[0] - ab[0]*ac[2],
        ab[0]*ac[1] - ab[1]*ac[0],
    ];
    let area2 = cross[0].hypot(cross[1]).hypot(cross[2]);
    // OpenJK PM_ClipVelocity's walkability threshold MIN_WALK_NORMAL = 0.7,
    // evaluated in renderer Y-up. Use the *triangle plane* (not smooth shared
    // vertex normals), which prevents adjacent faces changing each other's grid.
    area2 > 1.0e-7 && cross[1] / area2 >= 0.7
}

fn shitter_runs_from_indices(
    indices: &[u32], first_index: u32,
    mut position: impl FnMut(u32) -> Option<[f32; 3]>,
) -> Vec<ShitterSlopeRun> {
    let mut runs: Vec<ShitterSlopeRun> = Vec::new();
    for (triangle, corners) in indices.chunks_exact(3).enumerate() {
        let gray = match (position(corners[0]), position(corners[1]), position(corners[2])) {
            (Some(a), Some(b), Some(c)) => shitter_face_gray(a, b, c),
            _ => false,
        };
        let start = first_index + (triangle as u32)*3;
        if let Some(last) = runs.last_mut() {
            if last.gray == gray && last.first_index + last.index_count == start {
                last.index_count += 3;
                continue;
            }
        }
        runs.push(ShitterSlopeRun { first_index: start, index_count: 3, gray });
    }
    runs
}

fn world_position(world: &super::WorldGpu, index: u32) -> Option<[f32; 3]> {
    if let Some(vertex) = world.static_ao_gpu_vertices.get(index as usize) {
        return Some(vertex.position);
    }
    for model in &world.inline_models {
        let Some(offset) = index.checked_sub(model.gpu_vertex_start) else { continue; };
        if let Some(vertex) = model.base.get(offset as usize) {
            return Some(vertex.position);
        }
    }
    None
}

impl ShitterSlopeCache {
    pub(in crate::renderer) fn for_world(world: &super::WorldGpu) -> Self {
        let indices = world.remap_index_source.as_slice();
        let static_runs = shitter_runs_from_indices(indices, 0, |index| world_position(world, index));
        let gray = static_runs.iter().filter(|run| run.gray).map(|run| run.index_count/3).sum::<u32>();
        println!("[SHITTER] cached triangle slope classifications: {} triangles ({} walkable), {} runs, ShitteR-only", indices.len()/3, gray, static_runs.len());
        Self { static_runs, dynamic_runs: BTreeMap::new(), static_end: indices.len() as u32 }
    }

    pub(in crate::renderer) fn cache_collapsed(
        &mut self, start: u32, indices: &[u32], mut position: impl FnMut(u32) -> Option<[f32; 3]>,
    ) {
        self.dynamic_runs.insert(start, shitter_runs_from_indices(indices, start, |index| position(index)));
    }

    pub(in crate::renderer) fn forget_collapsed(&mut self, start: u32) {
        self.dynamic_runs.remove(&start);
    }

    /// Project cached triangle runs into this draw's slice without any face
    /// slope calculation. The material selection changes only at triangle
    /// boundaries, and each triangle is submitted exactly once.
    pub(in crate::renderer) fn append_draws(
        &self, first: u32, count: u32,
        orange: &mut Vec<super::DrawIndexedIndirectArgs>,
        gray: &mut Vec<super::DrawIndexedIndirectArgs>,
    ) {
        let end = first.saturating_add(count);
        let override_runs = self.dynamic_runs.range(..=first).next_back()
            .map(|(_, runs)| runs.as_slice())
            .filter(|runs| runs.last().is_some_and(|run|
                run.first_index + run.index_count > first));
        let runs = override_runs.unwrap_or_else(|| if first < self.static_end {
            &self.static_runs
        } else {
            &[]
        });
        let mut at = runs.partition_point(|run| run.first_index + run.index_count <= first);
        let mut covered = first;
        while let Some(run) = runs.get(at) {
            if run.first_index >= end { break; }
            let start = run.first_index.max(first);
            let stop = (run.first_index + run.index_count).min(end);
            if start > covered { // unknown AUTO4 region: do not drop triangles
                push_shitter_draw(orange, covered, start - covered);
            }
            if stop > start {
                push_shitter_draw(if run.gray { &mut *gray } else { &mut *orange }, start, stop-start);
                covered = stop;
            }
            at += 1;
        }
        if covered < end {
            push_shitter_draw(orange, covered, end - covered);
        }
    }
}

fn push_shitter_draw(draws: &mut Vec<super::DrawIndexedIndirectArgs>, first: u32, count: u32) {
    if count == 0 { return; }
    if let Some(last) = draws.last_mut() {
        if last.first_index + last.index_count == first {
            last.index_count += count;
            return;
        }
    }
    draws.push(super::DrawIndexedIndirectArgs {
        first_index: first, index_count: count,
        instance_count: 1, base_vertex: 0, first_instance: 0,
    });
}

/// Register any already-resident patches/AUTO4 recipes when the user activates
/// textured ShitteR after previously playing the map in the normal renderer.
/// These are not rebuilt or even inspected while ShitteR is disabled.
pub(in crate::renderer) fn cache_active_shitter_lod(world: &mut super::WorldGpu) {
    let ranges = world.patch_lod.shitter_active_ranges(
        &world.bsp_model_lod, &world.auto4_collapse_cache.states, &world.auto4_geometry_members, &world.full_batches,
        &world.remap_index_source,
    );
    for (start, indices) in ranges {
        let (cache, vertices, models) = (&mut world.shitter_slope_cache,
            &world.static_ao_gpu_vertices, &world.inline_models);
        if let Some(cache) = cache.as_mut() {
            cache.cache_collapsed(start, &indices, |index| {
                if let Some(vertex) = vertices.get(index as usize) { return Some(vertex.position); }
                models.iter().find_map(|model| index.checked_sub(model.gpu_vertex_start)
                    .and_then(|offset| model.base.get(offset as usize).map(|v| v.position)))
            });
        }
    }
}

// The original flat pipeline remains completely separate from the optional
// textured variant, so /r_shitterTexture 0 executes no texture sampling.
pub(in crate::renderer) struct CollisionWorldTexturedPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub gray_pipeline: wgpu::RenderPipeline,
    pub bind_group: wgpu::BindGroup,
    // Retain the layout and images so changing r_textureMode can rebind the
    // live sampler without reloading textures or recompiling the pipeline.
    image_layout: wgpu::BindGroupLayout,
    _images: [super::GpuImage; 2],
}

pub(in crate::renderer) struct CollisionWorldPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub textured: Option<CollisionWorldTexturedPipeline>,
    format: wgpu::TextureFormat,
    samples: u32,
    game: Option<PathBuf>,
    texture_requested: bool,
    texture_job: Option<mpsc::Receiver<Result<[(crate::materials::TextureData, String); 2], String>>>,
}

const SHITTER_ORANGE_IMAGE: &str = "textures/japro/orange_grid";
const SHITTER_GRAY_IMAGE: &str = "textures/japro/gray_grid";

// Both images are resolved once by the background worker. An authored shader
// name can be a script-only alias; only its static diffuse image is used.
// Missing assets remain conspicuously magenta, never silently substituted.
fn load_shitter_image(
    assets: &mut jka_assets::pk3::AssetSearchPath,
    qpath: &'static str,
) -> Result<(crate::materials::TextureData, String), String> {
    if let Some(data) = super::try_load_texture_asset_from_search_path(
        assets, qpath, false, true, true, "Shitter raw world image",
    ) {
        return Ok((data, qpath.to_owned()));
    }
    let (shaders, _) = crate::materials::shader_library(assets, &mut Vec::new(), false)?;
    let shader = jka_assets::shader::find_shader(&shaders, qpath)
        .ok_or_else(|| format!("{qpath} has no raw image and no shader definition"))?;
    for stage in &shader.stages {
        if stage.video || stage.image.is_empty() || stage.image.starts_with('$')
            || !matches!(stage.tc_gen, jka_assets::shader::TcGen::Base)
        {
            continue;
        }
        if let Some(data) = super::try_load_texture_asset_from_search_path(
            assets, &stage.image, false, true, true, "Shitter shader diffuse image",
        ) {
            return Ok((data, stage.image.clone()));
        }
    }
    Err(format!("{qpath} shader defines no usable diffuse image"))
}

fn load_shitter_images(
    base: &std::path::Path,
    game: Option<&std::path::Path>,
) -> Result<[(crate::materials::TextureData, String); 2], String> {
    let mut assets = jka_assets::pk3::AssetSearchPath::open_game(base, game)
        .map_err(|error| format!("Shitter image asset search: {error}"))?;
    Ok([SHITTER_ORANGE_IMAGE, SHITTER_GRAY_IMAGE].map(|name| {
        load_shitter_image(&mut assets, name).unwrap_or_else(|error| {
            eprintln!("[SHITTER TEXTURE] {error}; showing missing-texture checker");
            (crate::materials::missing_texture_data(), format!("MISSING {name}"))
        })
    }))
}

// Called when the live r_textureMode / anisotropy sampler changes. The two
// resident grid textures and pipeline are retained; rebinding is O(1) and
// never occurs in a normal frame.
fn shitter_grid_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    images: &[super::GpuImage; 2],
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Shitter orange/gray grid image bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0, resource: wgpu::BindingResource::TextureView(&images[0].view),
            },
            wgpu::BindGroupEntry {
                binding: 1, resource: wgpu::BindingResource::TextureView(&images[1].view),
            },
            wgpu::BindGroupEntry {
                binding: 2, resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

fn create_shitter_world_pipeline(
    device: &wgpu::Device,
    camera_layout: &wgpu::BindGroupLayout,
    image_layout: Option<&wgpu::BindGroupLayout>,
    format: wgpu::TextureFormat,
    samples: u32,
    shader_source: &str,
    label: &'static str,
    gray: bool,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(shader_source.into()),
    });
    let mut layouts = vec![Some(camera_layout)];
    if let Some(layout) = image_layout {
        layouts.push(Some(layout));
    }
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &layouts,
        immediate_size: 0,
    });
    // Hold specialization constants until create_render_pipeline has consumed the descriptor.
    // An array literal inside the conditional constants field would be dropped too early.
    let grid_constants = [("GRID_GRAY", if gray { 1.0 } else { 0.0 })];
    let fragment_constants: &[(&str, f64)] = if image_layout.is_some() {
        &grid_constants
    } else {
        &[]
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            // Both shaders read the original BSP buffers, never upload a new mesh.
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<scene::GpuVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &[
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x3,
                        offset: 0,
                        shader_location: 0,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x3,
                        offset: std::mem::offset_of!(scene::GpuVertex, normal) as u64,
                        shader_location: 1,
                    },
                ],
            })],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None, // keep two-sided BSP/patch visibility
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: depth_format(),
            depth_write_enabled: Some(true),
            // Strict reversed-Z: coincident authored triangles cannot keep
            // repainting each other in arbitrary order. Gray is drawn first.
            depth_compare: Some(wgpu::CompareFunction::Greater),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: fragment_constants,
                ..Default::default()
            },
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: crate::pipeline_cache::get().as_ref(),
    })
}

impl Renderer {
    pub(in crate::renderer) fn ensure_collision_world_pipeline(&mut self) {
        let format = self.scene_format();
        let samples = self.msaa_samples;
        let reusable = self.collision_world_pipeline.as_ref().is_some_and(|cached|
            cached.format == format && cached.samples == samples
                && cached.game == self.detail_texture_game);
        if !reusable {
            let pipeline = create_shitter_world_pipeline(
                &self.device, &self.camera_layout, None, format, samples,
                include_str!("../collision_world.wgsl"), "Native BSP flat Shitter pipeline", false,
            );
            self.collision_world_pipeline = Some(CollisionWorldPipeline {
                pipeline, textured: None, format, samples,
                game: self.detail_texture_game.clone(),
                texture_requested: false, texture_job: None,
            });
        }
        if super::shitter_texture_enabled() {
            self.ensure_shitter_textured_pipeline();
        }
    }

    fn ensure_shitter_textured_pipeline(&mut self) {
        let loaded = {
            let Some(cached) = self.collision_world_pipeline.as_mut() else { return; };
            if cached.textured.is_some() { return; }
            match cached.texture_job.as_ref().map(|job| job.try_recv()) {
                Some(Ok(result)) => {
                    cached.texture_job = None;
                    Some(result)
                }
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    cached.texture_job = None;
                    Some(Err("Shitter texture worker disconnected".to_owned()))
                }
                _ => None,
            }
        };
        if let Some(result) = loaded {
            self.finish_shitter_textured_pipeline(result);
            return;
        }
        let Some(cached) = self.collision_world_pipeline.as_mut() else { return; };
        if cached.texture_requested { return; }
        cached.texture_requested = true;
        let root = self.base.clone();
        let game = self.detail_texture_game.clone();
        let (tx, rx) = mpsc::channel();
        match std::thread::Builder::new().name("shitter-world-image".into()).spawn(move || {
            let _ = tx.send(load_shitter_images(&root, game.as_deref()));
        }) {
            Ok(_) => cached.texture_job = Some(rx),
            Err(error) => self.finish_shitter_textured_pipeline(Err(format!(
                "could not spawn Shitter image worker: {error}"
            ))),
        }
    }

    fn finish_shitter_textured_pipeline(
        &mut self,
        result: Result<[(crate::materials::TextureData, String); 2], String>,
    ) {
        let images = match result {
            Ok(images) => images,
            Err(error) => {
                eprintln!("[SHITTER TEXTURE] {error}; showing missing-texture checkers");
                [SHITTER_ORANGE_IMAGE, SHITTER_GRAY_IMAGE].map(|name| {
                    (crate::materials::missing_texture_data(), format!("MISSING {name}"))
                })
            }
        };
        let [(orange_data, orange_path), (gray_data, gray_path)] = images;
        let (orange_width, orange_height) = (orange_data.width, orange_data.height);
        let (gray_width, gray_height) = (gray_data.width, gray_data.height);
        let orange_image = super::upload_texture_picmip(&self.device, &self.queue, &orange_data, self.picmip);
        let gray_image = super::upload_texture_picmip(&self.device, &self.queue, &gray_data, self.picmip);
        let image_layout = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Shitter orange/gray grid image layout"),
            entries: &[
                super::bindings::texture_entry(0),
                super::bindings::texture_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let images = [orange_image, gray_image];
        let bind_group = shitter_grid_bind_group(
            &self.device, &image_layout, &images, &self.repeat_sampler,
        );
        let format = self.scene_format();
        let pipeline = create_shitter_world_pipeline(
            &self.device, &self.camera_layout, Some(&image_layout), format, self.msaa_samples,
            include_str!("../collision_world_textured.wgsl"),
            "Native BSP orange-grid Shitter pipeline", false,
        );
        let gray_pipeline = create_shitter_world_pipeline(
            &self.device, &self.camera_layout, Some(&image_layout), format, self.msaa_samples,
            include_str!("../collision_world_textured.wgsl"),
            "Native BSP gray-grid Shitter pipeline", true,
        );
        if let Some(cached) = self.collision_world_pipeline.as_mut() {
            cached.textured = Some(CollisionWorldTexturedPipeline {
                pipeline, gray_pipeline, bind_group, image_layout, _images: images,
            });
        }
        println!("[SHITTER TEXTURE] orange={} ({}x{}), walkable-gray={} ({}x{}), picmip {}, one bind group, one sample per world fragment",
            orange_path, orange_width, orange_height, gray_path, gray_width, gray_height, self.picmip);
    }

    pub(in crate::renderer) fn rebind_shitter_texture_sampler(&mut self) {
        if let Some(textured) = self.collision_world_pipeline.as_mut()
            .and_then(|cached| cached.textured.as_mut())
        {
            textured.bind_group = shitter_grid_bind_group(
                &self.device, &textured.image_layout, &textured._images, &self.repeat_sampler,
            );
        }
    }
}

/// Only allocated on demand. `WorldGpu` does not hold an acoustic mesh or
/// another persistent map-load product when this feature has never been used.
pub(in crate::renderer) struct CollisionWorldJob {
    map_hash: u64,
    receiver: mpsc::Receiver<Result<Arc<jka_assets::bsp::CompetitiveMesh>, String>>,
}

impl Renderer {
    pub(in crate::renderer) fn request_collision_world(
        &mut self,
        map_hash: u64,
        root: PathBuf,
        game: Option<PathBuf>,
        source: scene::MapSource,
        allow_asset_overrides: bool,
    ) {
        if !super::collision_world_only()
            || !self.world.as_ref().is_some_and(|world|
                world.remap_asset_hash == map_hash && world.collision_mesh.is_none())
            || self.collision_world_job.as_ref().is_some_and(|job| job.map_hash == map_hash)
        {
            return;
        }
        // A stale map's pending receiver is never allowed to upload here.
        self.collision_world_job = None;
        let (tx, receiver) = mpsc::channel();
        match std::thread::Builder::new()
            .name("collision-world-build".into())
            .spawn(move || {
                let result = scene::prepare_collision_render_mesh(
                    &root, game.as_deref(), &source, allow_asset_overrides, map_hash,
                );
                let _ = tx.send(result);
            })
        {
            Ok(_) => {
                self.collision_world_job = Some(CollisionWorldJob { map_hash, receiver });
                println!("[COLLISION WORLD] preparing compiled visual surfaces on demand (background worker)");
            }
            Err(error) => println!("[COLLISION WORLD] worker spawn failed: {error}"),
        }
    }

    pub(in crate::renderer) fn poll_collision_world_job(&mut self) {
        let Some(job) = &self.collision_world_job else { return; };
        let result = match job.receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("worker disconnected".to_owned()),
        };
        let map_hash = job.map_hash;
        self.collision_world_job = None;
        if !super::collision_world_only()
            || !self.world.as_ref().is_some_and(|world| world.remap_asset_hash == map_hash)
        {
            return;
        }
        match result {
            Ok(mesh) => {
                let gpu = upload(&self.device, &mesh);
                if let Some(world) = self.world.as_mut() {
                    if world.remap_asset_hash == map_hash {
                        world.collision_mesh = gpu;
                        if let Some(mesh) = &world.collision_mesh {
                            println!("[COLLISION WORLD] compiled visual mesh: {} triangles, {} spatial chunks, {:.2} MiB GPU vertex/index buffers", mesh.index_count / 3, mesh.chunks.len(), mesh.gpu_bytes as f64 / 1_048_576.0);
                        } else {
                            println!("[COLLISION WORLD] empty competitive mesh; using authored world");
                        }
                    }
                }
            }
            Err(error) => println!("[COLLISION WORLD] preparation failed: {error}"),
        }
    }
}

#[cfg(test)]
mod collision_pvs_tests {
    use super::collision_bsp_bounds;

    #[test]
    fn renderer_aabb_round_trips_to_jka_bsp_bounds() {
        // renderer Y-up [x,z,-y] => JKA Z-up [x,y,z]
        let (min, max) = collision_bsp_bounds(
            [10.0, -80.0, -400.0], [30.0, 100.0, -200.0],
        );
        assert_eq!(min, [10.0, 200.0, -80.0]);
        assert_eq!(max, [30.0, 400.0, 100.0]);
    }
}

#[cfg(test)]
mod shitter_grid_tests {
    use super::{ShitterSlopeCache, shitter_face_gray, shitter_runs_from_indices};

    #[test]
    fn true_triangle_planes_define_walkable_grids() {
        assert!(shitter_face_gray([0.,0.,0.], [0.,0.,1.], [1.,0.,0.]));
        assert!(!shitter_face_gray([0.,0.,0.], [1.,0.,0.], [0.,0.,1.]));
        assert!(!shitter_face_gray([0.,0.,0.], [0.,1.,0.], [0.,0.,1.]));
    }

    #[test]
    fn cached_triangle_runs_are_exclusive_and_can_be_sliced() {
        // Two triangles share edge vertices yet have different plane normals.
        let points = [[0.,0.,0.], [0.,0.,1.], [1.,0.,0.], [0.,1.,0.]];
        let indexes = [0,1,2,0,3,1];
        let runs = shitter_runs_from_indices(&indexes, 0, |i| points.get(i as usize).copied());
        assert_eq!(runs.len(), 2);
        assert!(runs[0].gray);
        assert!(!runs[1].gray);
        let cache = ShitterSlopeCache { static_runs: runs, dynamic_runs: Default::default(), static_end: 6 };
        let (mut orange, mut gray) = (Vec::new(), Vec::new());
        cache.append_draws(0,6,&mut orange,&mut gray);
        assert_eq!((gray[0].first_index,gray[0].index_count), (0,3));
        assert_eq!((orange[0].first_index,orange[0].index_count), (3,3));
        orange.clear();gray.clear();
        cache.append_draws(3,3,&mut orange,&mut gray);
        assert_eq!(orange.len(),1);
        assert!(gray.is_empty());
    }
}
