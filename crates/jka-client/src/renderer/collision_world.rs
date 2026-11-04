//! Competitive world A/B. Indexed compiled BSP visible-surface mesh,
//! flat directional shading and no BSP material or lightmap bindings.
//! All ordinary dynamic player and weapon presentation stays unchanged.
use super::{camera_depth_compare, depth_format, PvsMode, Renderer};
use crate::scene;
use std::sync::{mpsc, Arc};
use std::path::PathBuf;
use std::collections::BTreeMap;
use std::ops::Range;
use wgpu::util::DeviceExt;

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

pub(in crate::renderer) struct CollisionWorldPipeline {
    pub pipeline: wgpu::RenderPipeline,
    format: wgpu::TextureFormat,
    samples: u32,
}

impl Renderer {
    pub(in crate::renderer) fn ensure_collision_world_pipeline(&mut self) {
        let format = self.scene_format();
        let samples = self.msaa_samples;
        if self.collision_world_pipeline.as_ref().is_some_and(|cached|
            cached.format == format && cached.samples == samples) {
            return;
        }
        let shader = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Collision-only shaded BSP shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../collision_world.wgsl").into()),
        });
        let layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Collision-only BSP pipeline layout"),
            bind_group_layouts: &[Some(&self.camera_layout)],
            immediate_size: 0,
        });
        let pipeline = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Collision-only BSP pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 16,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x4,
                        offset: 0,
                        shader_location: 0,
                    }],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                // The normal BSP renderer uses two-sided faces. Shader-authored
                // culling and patch winding are not represented in our single
                // flat pipeline; one-sided rasterization can remove entire
                // walls, floors or curved surfaces when seen from the back.
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format(),
                depth_write_enabled: Some(true),
                depth_compare: Some(camera_depth_compare()),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples, ..Default::default()
            },
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
        });
        self.collision_world_pipeline = Some(CollisionWorldPipeline {
            pipeline, format, samples,
        });
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
