//! Environment snow.
use crate::renderer::{
    aabb_intersects_clip_frustum, mpsc, Arc, BTreeMap, DrawBatch, DrawClass, Duration, GpuVertex,
    HashMap, HashSet, Instant, Mat4, Mutex, PipelineKey, Vec2, Vec3, WorldGpu, MATERIAL_SNOW,
    SNOW_SHELL_CHUNK_SIZE, SNOW_SHELL_DRAW_RADIUS, SNOW_SHELL_TESSELLATION,
};
use wgpu::util::DeviceExt;

pub(in crate::renderer) struct SnowShellDraw {
    pub(in crate::renderer) coarse_batch_index: usize,
    /// Vertex range inside the owning chunk's buffer.
    pub(in crate::renderer) vertices: std::ops::Range<u32>,
    pub(in crate::renderer) bounds_min: [f32; 3],
    pub(in crate::renderer) bounds_max: [f32; 3],
}

/// One up-facing snow triangle as authored, before tessellation.
pub(in crate::renderer) struct SnowSourceTriangle {
    pub(in crate::renderer) coarse_batch_index: usize,
    pub(in crate::renderer) vertices: [GpuVertex; 3],
}

/// One resident chunk of tessellated snow: every snow sub-triangle whose centroid
/// falls in a `SNOW_SHELL_CHUNK_SIZE` cell, with one draw per source batch.
pub(in crate::renderer) struct SnowShellChunk {
    /// `None` when the cell turned out to hold no sub-triangle centroid.
    pub(in crate::renderer) vertex_buffer: Option<wgpu::Buffer>,
    pub(in crate::renderer) draws: Vec<SnowShellDraw>,
    /// XZ extent of the chunk cell, for a cheap whole-chunk distance reject.
    pub(in crate::renderer) min: Vec2,
    pub(in crate::renderer) max: Vec2,
}

impl SnowShellChunk {
    pub(in crate::renderer) fn near_center(&self, center: Vec2) -> bool {
        let closest = center.clamp(self.min, self.max);
        // The draw bounds can poke a few units past the cell edge.
        (closest - center).length_squared()
            <= (SNOW_SHELL_DRAW_RADIUS + 8.0) * (SNOW_SHELL_DRAW_RADIUS + 8.0)
    }
}

pub(in crate::renderer) struct SnowShellGpu {
    // The shell is the snow surface tessellated down to `SNOW_SHELL_TESSELLATION`
    // so the Snowflow field can move vertices on the GPU. Only the area around the
    // field centre is ever drawn, so instead of tessellating the whole map up
    // front (hundreds of MiB), keep the cheap source triangles and tessellate a
    // chunk when the field centre approaches it. Runtime footsteps never rebuild
    // a chunk; the persistent Snowflow field moves its vertices on the GPU.
    pub(in crate::renderer) source: Arc<SnowSource>,
    pub(in crate::renderer) chunks: BTreeMap<(i32, i32), SnowShellChunk>,
    /// Cells handed to the worker pool whose result has not been collected yet.
    pub(in crate::renderer) pending: HashSet<(i32, i32)>,
    pub(in crate::renderer) results_tx: mpsc::Sender<SnowChunkBuild>,
    /// Only the render thread reads it (`stream` has `&mut self`); the mutex just
    /// keeps `WorldGpu` `Sync`.
    pub(in crate::renderer) results_rx: Mutex<mpsc::Receiver<SnowChunkBuild>>,
    /// A shared per-vertex `u32::MAX` stream; every chunk binds it as slot 1.
    pub(in crate::renderer) legacy_dlight_surface_id_buffer: wgpu::Buffer,
    pub(in crate::renderer) id_capacity: usize,
    pub(in crate::renderer) stats: SnowShellStats,
}

/// The map's snow triangles and which chunk cells they overlap; immutable after
/// load and shared with the tessellation workers.
pub(in crate::renderer) struct SnowSource {
    pub(in crate::renderer) triangles: Vec<SnowSourceTriangle>,
    /// Source-triangle indices (ascending) overlapping each chunk cell in XZ.
    pub(in crate::renderer) cells: HashMap<(i32, i32), Vec<u32>>,
}

/// A chunk tessellated on a worker, waiting to become a GPU buffer.
pub(in crate::renderer) struct SnowChunkBuild {
    pub(in crate::renderer) cell: (i32, i32),
    pub(in crate::renderer) vertices: Vec<GpuVertex>,
    pub(in crate::renderer) draws: Vec<SnowShellDraw>,
    pub(in crate::renderer) build_ms: f64,
}

/// Counters for the `[SNOW STREAM]` log line; reset whenever the queue drains.
#[derive(Clone, Copy, Default)]
pub(in crate::renderer) struct SnowShellStats {
    pub(in crate::renderer) chunks: u32,
    pub(in crate::renderer) build_ms_sum: f64,
    pub(in crate::renderer) build_ms_max: f64,
    pub(in crate::renderer) upload_ms_sum: f64,
    pub(in crate::renderer) upload_ms_max: f64,
}

pub(in crate::renderer) fn gpu_vertex_key(vertex: &GpuVertex) -> [u32; 15] {
    [
        vertex.position[0].to_bits(),
        vertex.position[1].to_bits(),
        vertex.position[2].to_bits(),
        vertex.uv[0].to_bits(),
        vertex.uv[1].to_bits(),
        vertex.lightmap_uv[0].to_bits(),
        vertex.lightmap_uv[1].to_bits(),
        vertex.normal[0].to_bits(),
        vertex.normal[1].to_bits(),
        vertex.normal[2].to_bits(),
        vertex.color[0].to_bits(),
        vertex.color[1].to_bits(),
        vertex.color[2].to_bits(),
        vertex.color[3].to_bits(),
        vertex.alpha_cutoff.to_bits(),
    ]
}

pub(in crate::renderer) fn midpoint_gpu_vertex(a: GpuVertex, b: GpuVertex) -> GpuVertex {
    let midpoint3 = |a: [f32; 3], b: [f32; 3]| {
        [
            (a[0] + b[0]) * 0.5,
            (a[1] + b[1]) * 0.5,
            (a[2] + b[2]) * 0.5,
        ]
    };
    let midpoint2 = |a: [f32; 2], b: [f32; 2]| [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
    let midpoint4 = |a: [f32; 4], b: [f32; 4]| {
        [
            (a[0] + b[0]) * 0.5,
            (a[1] + b[1]) * 0.5,
            (a[2] + b[2]) * 0.5,
            (a[3] + b[3]) * 0.5,
        ]
    };
    let mut normal = Vec3::from_array(midpoint3(a.normal, b.normal)).normalize_or_zero();
    if normal == Vec3::ZERO {
        normal = Vec3::from_array(a.normal).normalize_or_zero();
    }
    GpuVertex {
        position: midpoint3(a.position, b.position),
        uv: midpoint2(a.uv, b.uv),
        lightmap_uv: midpoint2(a.lightmap_uv, b.lightmap_uv),
        normal: normal.to_array(),
        color: midpoint4(a.color, b.color),
        alpha_cutoff: (a.alpha_cutoff + b.alpha_cutoff) * 0.5,
    }
}

/// Tessellate `triangle` down to the shell lattice, appending only the leaf
/// triangles whose centroid lies in `cell`. Sub-triangles whose bounding box
/// misses the cell cannot contain such a leaf (every leaf centroid is inside its
/// ancestor's box), so they are skipped; the leaves and their order are exactly
/// what an unpruned walk would assign to this cell.
pub(in crate::renderer) fn append_snow_triangle_in_cell(
    out: &mut Vec<GpuVertex>,
    cell: (i32, i32),
    triangle: [GpuVertex; 3],
    depth: u32,
) {
    let positions = triangle.map(|vertex| Vec3::from_array(vertex.position));
    // Margin keeps float rounding at a cell edge from pruning a leaf that the
    // exact centroid test below would still assign here.
    let lo_x = cell.0 as f32 * SNOW_SHELL_CHUNK_SIZE - 0.01;
    let hi_x = (cell.0 + 1) as f32 * SNOW_SHELL_CHUNK_SIZE + 0.01;
    let lo_z = cell.1 as f32 * SNOW_SHELL_CHUNK_SIZE - 0.01;
    let hi_z = (cell.1 + 1) as f32 * SNOW_SHELL_CHUNK_SIZE + 0.01;
    let min_x = positions[0].x.min(positions[1].x).min(positions[2].x);
    let max_x = positions[0].x.max(positions[1].x).max(positions[2].x);
    let min_z = positions[0].z.min(positions[1].z).min(positions[2].z);
    let max_z = positions[0].z.max(positions[1].z).max(positions[2].z);
    if max_x < lo_x || min_x > hi_x || max_z < lo_z || min_z > hi_z {
        return;
    }

    let max_edge_sq = (positions[0] - positions[1])
        .length_squared()
        .max((positions[1] - positions[2]).length_squared())
        .max((positions[2] - positions[0]).length_squared());
    let target_sq = SNOW_SHELL_TESSELLATION * SNOW_SHELL_TESSELLATION;
    if max_edge_sq <= target_sq || depth >= 12 {
        let centroid = (positions[0] + positions[1] + positions[2]) / 3.0;
        let chunk_x = (centroid.x / SNOW_SHELL_CHUNK_SIZE).floor() as i32;
        let chunk_z = (centroid.z / SNOW_SHELL_CHUNK_SIZE).floor() as i32;
        if (chunk_x, chunk_z) == cell {
            out.extend_from_slice(&triangle);
        }
        return;
    }

    let ab = midpoint_gpu_vertex(triangle[0], triangle[1]);
    let bc = midpoint_gpu_vertex(triangle[1], triangle[2]);
    let ca = midpoint_gpu_vertex(triangle[2], triangle[0]);
    append_snow_triangle_in_cell(out, cell, [triangle[0], ab, ca], depth + 1);
    append_snow_triangle_in_cell(out, cell, [ab, triangle[1], bc], depth + 1);
    append_snow_triangle_in_cell(out, cell, [ca, bc, triangle[2]], depth + 1);
    append_snow_triangle_in_cell(out, cell, [ab, bc, ca], depth + 1);
}

pub(in crate::renderer) fn snow_id_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    let ids = vec![u32::MAX; capacity.max(1)];
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("JKA Snowflow Legacy dlight surface-id fallback"),
        contents: bytemuck::cast_slice(&ids),
        usage: wgpu::BufferUsages::VERTEX,
    })
}

/// Index the map's snow triangles. This is a pass over the snow batches only and
/// builds no tessellation; chunks are tessellated on demand by `SnowShellGpu::stream`.
pub(in crate::renderer) fn create_snow_shell_gpu(
    device: &wgpu::Device,
    vertices: &[GpuVertex],
    coarse_sources: &[DrawBatch],
) -> SnowShellGpu {
    let mut source_triangles = Vec::<SnowSourceTriangle>::new();
    let mut cells = HashMap::<(i32, i32), Vec<u32>>::new();
    for (coarse_batch_index, source) in coarse_sources.iter().enumerate() {
        if u32::from(source.surface_material) != MATERIAL_SNOW
            || source.pipeline.class == DrawClass::Sky
        {
            continue;
        }
        let start = usize::try_from(source.vertices.start).unwrap_or(usize::MAX);
        let end = usize::try_from(source.vertices.end)
            .unwrap_or(usize::MAX)
            .min(vertices.len());
        if start >= end {
            continue;
        }
        for tri in vertices[start..end].chunks_exact(3) {
            let triangle = [tri[0], tri[1], tri[2]];
            let normal = (Vec3::from_array(triangle[0].normal)
                + Vec3::from_array(triangle[1].normal)
                + Vec3::from_array(triangle[2].normal))
            .normalize_or_zero();
            if normal.y <= 0.35 {
                continue;
            }
            let index = source_triangles.len() as u32;
            let xs = triangle.map(|vertex| vertex.position[0]);
            let zs = triangle.map(|vertex| vertex.position[2]);
            let cell_of = |value: f32| (value / SNOW_SHELL_CHUNK_SIZE).floor() as i32;
            let (min_x, max_x) = (xs[0].min(xs[1]).min(xs[2]), xs[0].max(xs[1]).max(xs[2]));
            let (min_z, max_z) = (zs[0].min(zs[1]).min(zs[2]), zs[0].max(zs[1]).max(zs[2]));
            // Every leaf centroid lies inside the source triangle's box, so these
            // are all the cells that can receive a leaf of this triangle.
            for cell_x in cell_of(min_x)..=cell_of(max_x) {
                for cell_z in cell_of(min_z)..=cell_of(max_z) {
                    cells.entry((cell_x, cell_z)).or_default().push(index);
                }
            }
            source_triangles.push(SnowSourceTriangle {
                coarse_batch_index,
                vertices: triangle,
            });
        }
    }

    let id_capacity = 1 << 16;
    let (results_tx, results_rx) = mpsc::channel();
    rverbose!(
        1,
        "Snowflow overlay: {} source triangle(s) in {} chunk cell(s); {:.0}u chunks tessellated to a {:.1}u lattice on demand within {:.0}u of the field centre",
        source_triangles.len(),
        cells.len(),
        SNOW_SHELL_CHUNK_SIZE,
        SNOW_SHELL_TESSELLATION,
        SNOW_SHELL_DRAW_RADIUS,
    );
    SnowShellGpu {
        source: Arc::new(SnowSource {
            triangles: source_triangles,
            cells,
        }),
        chunks: BTreeMap::new(),
        pending: HashSet::new(),
        results_tx,
        results_rx: Mutex::new(results_rx),
        legacy_dlight_surface_id_buffer: snow_id_buffer(device, id_capacity),
        id_capacity,
        stats: SnowShellStats::default(),
    }
}

/// How far beyond the draw radius chunks are prepared, so a moving player finds
/// them already built, and how much farther they must drift before eviction.
pub(in crate::renderer) const SNOW_SHELL_PREFETCH_MARGIN: f32 = 160.0;

pub(in crate::renderer) const SNOW_SHELL_EVICT_MARGIN: f32 = 352.0;

/// Chunks tessellating on the worker pool at once. Requests are re-sorted
/// nearest-first every frame, so a small window keeps the queue from going
/// stale when the field centre moves quickly.
pub(in crate::renderer) const SNOW_SHELL_MAX_IN_FLIGHT: usize = 12;

/// Render-thread budget per frame for turning finished chunks into GPU buffers
/// (one is always taken so progress is guaranteed).
pub(in crate::renderer) const SNOW_SHELL_UPLOAD_BUDGET: Duration = Duration::from_micros(600);

pub(in crate::renderer) static SNOW_SHELL_WORKER_POOL: std::sync::OnceLock<
    Option<rayon::ThreadPool>,
> = std::sync::OnceLock::new();

/// Run `job` off the render thread. The shell tessellation is the only user, so
/// it gets its own small pool instead of competing with frame-critical pools.
pub(in crate::renderer) fn spawn_snow_shell_job(job: impl FnOnce() + Send + 'static) {
    let pool = SNOW_SHELL_WORKER_POOL
        .get_or_init(|| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(2)
                .thread_name(|index| format!("jka-snow-shell-{index}"))
                .build()
                .map_err(|error| eprintln!("Snow shell worker pool unavailable: {error}"))
                .ok()
        })
        .as_ref();
    match pool {
        Some(pool) => pool.spawn(job),
        None => {
            std::thread::spawn(job);
        }
    }
}

/// Tessellate one chunk cell. Pure function of the shared source, so it runs on
/// a worker; the render thread only has to create the GPU buffer.
pub(in crate::renderer) fn tessellate_snow_cell(
    source: &SnowSource,
    cell: (i32, i32),
) -> (Vec<GpuVertex>, Vec<SnowShellDraw>) {
    let mut packed = Vec::<GpuVertex>::new();
    let mut draws = Vec::<SnowShellDraw>::new();
    let mut open: Option<(usize, usize)> = None;
    let close = |packed: &Vec<GpuVertex>,
                 open: &mut Option<(usize, usize)>,
                 draws: &mut Vec<SnowShellDraw>| {
        let Some((batch, start)) = open.take() else {
            return;
        };
        if start >= packed.len() {
            return;
        }
        let mut bounds_min = Vec3::splat(f32::INFINITY);
        let mut bounds_max = Vec3::splat(f32::NEG_INFINITY);
        for vertex in &packed[start..] {
            let p = Vec3::from_array(vertex.position);
            bounds_min = bounds_min.min(p);
            bounds_max = bounds_max.max(p);
        }
        // Snowflow source allows +0.34 m of berm; keep conservative JKA-unit padding.
        bounds_max.y += 16.0;
        draws.push(SnowShellDraw {
            coarse_batch_index: batch,
            vertices: u32::try_from(start).unwrap_or(u32::MAX)
                ..u32::try_from(packed.len()).unwrap_or(u32::MAX),
            bounds_min: bounds_min.to_array(),
            bounds_max: bounds_max.to_array(),
        });
    };
    if let Some(indices) = source.cells.get(&cell) {
        // Source triangles are stored batch by batch, so each batch's leaves are
        // contiguous and one draw covers them.
        for &index in indices {
            let triangle = &source.triangles[index as usize];
            let before = packed.len();
            if open.is_some_and(|(batch, _)| batch != triangle.coarse_batch_index) {
                close(&packed, &mut open, &mut draws);
            }
            if open.is_none() {
                open = Some((triangle.coarse_batch_index, before));
            }
            append_snow_triangle_in_cell(&mut packed, cell, triangle.vertices, 0);
        }
        close(&packed, &mut open, &mut draws);
    }
    (packed, draws)
}

impl SnowShellGpu {
    pub(in crate::renderer) fn has_snow(&self) -> bool {
        !self.source.triangles.is_empty()
    }

    /// Drop every resident chunk (footprints are not in 3D mode, so the shell is
    /// not drawn). Chunks still in flight are collected and discarded or reused
    /// by the next `stream`.
    pub(in crate::renderer) fn release(&mut self) {
        self.chunks.clear();
    }

    pub(in crate::renderer) fn cell_distance(cell: (i32, i32), center: Vec2) -> f32 {
        let min = Vec2::new(cell.0 as f32, cell.1 as f32) * SNOW_SHELL_CHUNK_SIZE;
        let max = min + Vec2::splat(SNOW_SHELL_CHUNK_SIZE);
        (center.clamp(min, max) - center).length()
    }

    /// Keep the chunks around `center` resident without touching the frame time
    /// much: finished worker results become GPU buffers (within a small budget),
    /// far chunks are dropped, and missing near ones are queued nearest-first.
    pub(in crate::renderer) fn stream(&mut self, device: &wgpu::Device, center: Vec2) {
        if !self.has_snow() {
            return;
        }
        let evict_distance = SNOW_SHELL_DRAW_RADIUS + SNOW_SHELL_EVICT_MARGIN;
        self.chunks
            .retain(|&cell, _| Self::cell_distance(cell, center) <= evict_distance);

        // Finished chunks -> GPU buffers.
        let upload_started = Instant::now();
        let mut uploaded = 0_u32;
        loop {
            if uploaded > 0 && upload_started.elapsed() >= SNOW_SHELL_UPLOAD_BUDGET {
                break;
            }
            let receiver = self
                .results_rx
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Ok(built) = receiver.try_recv() else {
                break;
            };
            self.pending.remove(&built.cell);
            // The player may have moved on while the worker was busy.
            if Self::cell_distance(built.cell, center) > evict_distance {
                continue;
            }
            let chunk_started = Instant::now();
            if built.vertices.len() > self.id_capacity {
                self.id_capacity = built.vertices.len().next_power_of_two();
                self.legacy_dlight_surface_id_buffer = snow_id_buffer(device, self.id_capacity);
            }
            let vertex_buffer = (!built.vertices.is_empty()).then(|| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("JKA Snowflow snow overlay chunk"),
                    contents: bytemuck::cast_slice(&built.vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                })
            });
            let min = Vec2::new(built.cell.0 as f32, built.cell.1 as f32) * SNOW_SHELL_CHUNK_SIZE;
            self.chunks.insert(
                built.cell,
                SnowShellChunk {
                    vertex_buffer,
                    draws: built.draws,
                    min,
                    max: min + Vec2::splat(SNOW_SHELL_CHUNK_SIZE),
                },
            );
            let upload_ms = chunk_started.elapsed().as_secs_f64() * 1000.0;
            self.stats.chunks += 1;
            self.stats.build_ms_sum += built.build_ms;
            self.stats.build_ms_max = self.stats.build_ms_max.max(built.build_ms);
            self.stats.upload_ms_sum += upload_ms;
            self.stats.upload_ms_max = self.stats.upload_ms_max.max(upload_ms);
            uploaded += 1;
        }

        // Queue missing chunks near the centre, nearest first.
        if self.pending.len() < SNOW_SHELL_MAX_IN_FLIGHT {
            let reach = SNOW_SHELL_DRAW_RADIUS + SNOW_SHELL_PREFETCH_MARGIN;
            let first_x = ((center.x - reach) / SNOW_SHELL_CHUNK_SIZE).floor() as i32;
            let last_x = ((center.x + reach) / SNOW_SHELL_CHUNK_SIZE).floor() as i32;
            let first_z = ((center.y - reach) / SNOW_SHELL_CHUNK_SIZE).floor() as i32;
            let last_z = ((center.y + reach) / SNOW_SHELL_CHUNK_SIZE).floor() as i32;
            let mut missing = Vec::<(f32, (i32, i32))>::new();
            for cell_x in first_x..=last_x {
                for cell_z in first_z..=last_z {
                    let cell = (cell_x, cell_z);
                    if self.chunks.contains_key(&cell)
                        || self.pending.contains(&cell)
                        || !self.source.cells.contains_key(&cell)
                    {
                        continue;
                    }
                    let distance = Self::cell_distance(cell, center);
                    if distance <= reach {
                        missing.push((distance, cell));
                    }
                }
            }
            missing.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (_, cell) in missing {
                if self.pending.len() >= SNOW_SHELL_MAX_IN_FLIGHT {
                    break;
                }
                let source = Arc::clone(&self.source);
                let results = self.results_tx.clone();
                self.pending.insert(cell);
                spawn_snow_shell_job(move || {
                    let started = Instant::now();
                    let (vertices, draws) = tessellate_snow_cell(&source, cell);
                    let build_ms = started.elapsed().as_secs_f64() * 1000.0;
                    // The shell is gone (map unloaded) when the receiver is.
                    let _ = results.send(SnowChunkBuild {
                        cell,
                        vertices,
                        draws,
                        build_ms,
                    });
                });
            }
        }

        // One summary line per fill (spawn, teleport, a burst) or any slow chunk;
        // a chunk border crossed while walking stays quiet.
        let stats = self.stats;
        if self.pending.is_empty()
            && stats.chunks > 0
            && (stats.chunks >= 8 || stats.build_ms_max >= 2.0 || stats.upload_ms_max >= 1.0)
        {
            rverbose!(
                2,
                "[SNOW STREAM] {} chunk(s) | worker build avg {:.2} max {:.2} ms (off the render thread) | render-thread buffer create avg {:.2} max {:.2} ms | {} resident",
                stats.chunks,
                stats.build_ms_sum / f64::from(stats.chunks),
                stats.build_ms_max,
                stats.upload_ms_sum / f64::from(stats.chunks),
                stats.upload_ms_max,
                self.chunks.len(),
            );
        }
        if self.pending.is_empty() {
            self.stats = SnowShellStats::default();
        }
    }
}

pub(in crate::renderer) fn snow_shell_draw_near_center(draw: &SnowShellDraw, center: Vec2) -> bool {
    let minimum = Vec2::new(draw.bounds_min[0], draw.bounds_min[2]);
    let maximum = Vec2::new(draw.bounds_max[0], draw.bounds_max[2]);
    let closest = center.clamp(minimum, maximum);
    (closest - center).length_squared() <= SNOW_SHELL_DRAW_RADIUS * SNOW_SHELL_DRAW_RADIUS
}

pub(in crate::renderer) fn draw_snow_shell_fast<'a>(
    pass: &mut wgpu::RenderPass<'a>,
    world: &'a WorldGpu,
    view_proj: Mat4,
    deform_center: Vec2,
    current_pipeline: &mut Option<PipelineKey>,
) {
    if world.snow_shell.chunks.is_empty() {
        return;
    }
    pass.set_vertex_buffer(
        1,
        world.snow_shell.legacy_dlight_surface_id_buffer.slice(..),
    );
    for chunk in world.snow_shell.chunks.values() {
        let Some(vertex_buffer) = &chunk.vertex_buffer else {
            continue;
        };
        if !chunk.near_center(deform_center) {
            continue;
        }
        pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        for draw in &chunk.draws {
            if !snow_shell_draw_near_center(draw, deform_center)
                || !aabb_intersects_clip_frustum(draw.bounds_min, draw.bounds_max, view_proj)
            {
                continue;
            }
            let Some(batch) = world.coarse_batches.get(draw.coarse_batch_index) else {
                continue;
            };
            if *current_pipeline != Some(batch.source.pipeline) {
                pass.set_pipeline(&world.fast_pipelines[&batch.source.pipeline]);
                *current_pipeline = Some(batch.source.pipeline);
            }
            pass.set_bind_group(1, &batch.fast_bind_group, &[]);
            pass.draw(draw.vertices.clone(), 1..2);
        }
    }
    pass.set_vertex_buffer(0, world.vertex_buffer.slice(..));
    pass.set_vertex_buffer(1, world.legacy_dlight_surface_id_buffer.slice(..));
    pass.set_index_buffer(world.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
    *current_pipeline = None;
}

pub(in crate::renderer) fn draw_snow_shell<'a>(
    pass: &mut wgpu::RenderPass<'a>,
    world: &'a WorldGpu,
    pipelines: &'a BTreeMap<PipelineKey, wgpu::RenderPipeline>,
    view_proj: Mat4,
    deform_center: Vec2,
    current_pipeline: &mut Option<PipelineKey>,
) {
    if world.snow_shell.chunks.is_empty() {
        return;
    }
    pass.set_vertex_buffer(
        1,
        world.snow_shell.legacy_dlight_surface_id_buffer.slice(..),
    );
    for chunk in world.snow_shell.chunks.values() {
        let Some(vertex_buffer) = &chunk.vertex_buffer else {
            continue;
        };
        if !chunk.near_center(deform_center) {
            continue;
        }
        pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        for draw in &chunk.draws {
            if !snow_shell_draw_near_center(draw, deform_center)
                || !aabb_intersects_clip_frustum(draw.bounds_min, draw.bounds_max, view_proj)
            {
                continue;
            }
            let Some(batch) = world.coarse_batches.get(draw.coarse_batch_index) else {
                continue;
            };
            if *current_pipeline != Some(batch.source.pipeline) {
                pass.set_pipeline(&pipelines[&batch.source.pipeline]);
                *current_pipeline = Some(batch.source.pipeline);
            }
            pass.set_bind_group(1, &batch.bind_group, &[]);
            pass.draw(draw.vertices.clone(), 1..2);
        }
    }
    pass.set_vertex_buffer(0, world.vertex_buffer.slice(..));
    pass.set_vertex_buffer(1, world.legacy_dlight_surface_id_buffer.slice(..));
    pass.set_index_buffer(world.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
    *current_pipeline = None;
}
