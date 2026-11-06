//! Competitive world A/B. Shitter reuses the normal world BSP GPU geometry,
//! PVS, index ranges and indirect draw calls, with a flat no-texture shader.
//! Steam Audio and gameplay collision are independent of this path.
use super::{camera_depth_compare, depth_format, PvsMode, Renderer};
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

// The original flat pipeline remains completely separate from the optional
// textured variant, so /r_shitterTexture 0 executes no texture sampling.
pub(in crate::renderer) struct CollisionWorldTexturedPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub bind_group: wgpu::BindGroup,
    _image: super::GpuImage,
}

pub(in crate::renderer) struct CollisionWorldPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub textured: Option<CollisionWorldTexturedPipeline>,
    format: wgpu::TextureFormat,
    samples: u32,
    game: Option<PathBuf>,
    texture_requested: bool,
    texture_job: Option<mpsc::Receiver<Result<(crate::materials::TextureData, String), String>>>,
}

const SHITTER_IMAGE: &str = "textures/factory/base";

// CPU-only work; runs once on a background thread, never in frame encoding.
// Prefer the image at the shader qpath. If the qpath is script-only, resolve
// one ordinary diffuse image from that script; do NOT execute its stages.
fn load_shitter_image(
    base: &std::path::Path,
    game: Option<&std::path::Path>,
) -> Result<(crate::materials::TextureData, String), String> {
    let mut assets = jka_assets::pk3::AssetSearchPath::open_game(base, game)
        .map_err(|error| format!("Shitter image asset search: {error}"))?;
    if let Some(data) = super::try_load_texture_asset_from_search_path(
        &mut assets, SHITTER_IMAGE, false, true, true, "Shitter raw world image",
    ) {
        return Ok((data, SHITTER_IMAGE.to_owned()));
    }
    // Resolve script-only aliases only on a background worker. The canonical
    // shader library parser already handles .shader/.oshader precedence.
    let (shaders, _) = crate::materials::shader_library(&mut assets, &mut Vec::new(), false)?;
    let shader = jka_assets::shader::find_shader(&shaders, SHITTER_IMAGE)
        .ok_or_else(|| format!("{SHITTER_IMAGE} has no raw image and no shader definition"))?;
    for stage in &shader.stages {
        if stage.video || stage.image.is_empty() || stage.image.starts_with('$')
            || !matches!(stage.tc_gen, jka_assets::shader::TcGen::Base)
        {
            continue;
        }
        if let Some(data) = super::try_load_texture_asset_from_search_path(
            &mut assets, &stage.image, false, true, true, "Shitter shader diffuse image",
        ) {
            return Ok((data, stage.image.clone()));
        }
    }
    Err(format!("{SHITTER_IMAGE} shader defines no usable diffuse image"))
}

fn create_shitter_world_pipeline(
    device: &wgpu::Device,
    camera_layout: &wgpu::BindGroupLayout,
    image_layout: Option<&wgpu::BindGroupLayout>,
    format: wgpu::TextureFormat,
    samples: u32,
    shader_source: &str,
    label: &'static str,
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
            depth_compare: Some(camera_depth_compare()),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
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
                include_str!("../collision_world.wgsl"), "Native BSP flat Shitter pipeline",
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
            let _ = tx.send(load_shitter_image(&root, game.as_deref()));
        }) {
            Ok(_) => cached.texture_job = Some(rx),
            Err(error) => self.finish_shitter_textured_pipeline(Err(format!(
                "could not spawn Shitter image worker: {error}"
            ))),
        }
    }

    fn finish_shitter_textured_pipeline(
        &mut self,
        result: Result<(crate::materials::TextureData, String), String>,
    ) {
        let (data, path) = match result {
            Ok((data, path)) => (data, path),
            Err(error) => {
                eprintln!("[SHITTER TEXTURE] {error}; showing missing-texture checker");
                (crate::materials::missing_texture_data(), "MISSING".to_owned())
            }
        };
        let width = data.width;
        let height = data.height;
        let image = super::upload_texture_picmip(&self.device, &self.queue, &data, self.picmip);
        let image_layout = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Shitter single raw image layout"),
            entries: &[
                super::bindings::texture_entry(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Shitter single raw image bind group"),
            layout: &image_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0, resource: wgpu::BindingResource::TextureView(&image.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1, resource: wgpu::BindingResource::Sampler(&self.repeat_sampler),
                },
            ],
        });
        let format = self.scene_format();
        let pipeline = create_shitter_world_pipeline(
            &self.device, &self.camera_layout, Some(&image_layout), format, self.msaa_samples,
            include_str!("../collision_world_textured.wgsl"),
            "Native BSP one-image Shitter pipeline",
        );
        if let Some(cached) = self.collision_world_pipeline.as_mut() {
            cached.textured = Some(CollisionWorldTexturedPipeline {
                pipeline, bind_group, _image: image,
            });
        }
        println!("[SHITTER TEXTURE] sampled {} ({}x{}, picmip {}, one bind group, one texture fetch per world fragment)",
            path, width, height, self.picmip);
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
