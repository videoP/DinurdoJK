//! Camera-window block culling (not Unreal). The window grid is drawn as
//! instanced BLOCK x BLOCK-cell blocks listed by a compute pass, so dry
//! riverbed and the never-loaded void around a sparse river send no
//! triangles to the vertex or fragment stages. See river_blocks.wgsl for the
//! exactness argument. The list is rebuilt only when SimGridRT is rewritten.
//!
//! r_riverAdaptiveMesh 1 (B, renderer only; the solver is unchanged) gives
//! each live block a mesh density from its water's turbulence: surface
//! curvature and flow speed (the advected wave height grows with speed).
//! There is no distance term, and it only removes triangles: the tile
//! resolution (one vertex per solver cell) is the most any block gets.
//! Each block holds its score (instant rise, slow decay) and coarsens only
//! past a margin, so templates do not flicker; see river_blocks.wgsl.
//! Templates over one block's (BLOCK + 1)^2 cell-corner vertices:
//!   0 FULL   one vertex per cell, 512 tris (A's mesh, unchanged)
//!   1 STEP2  288 tris, 2 STEP4 112 tris, 3 STEP8 72 tris (calmer water)
//! Every template keeps one vertex per cell along the block boundary, so
//! neighbouring blocks of any density meet without T-junctions. A
//! template's index values carry its number (`template * BLOCK_VERTICES +
//! local`) so vs_block finds its block list without a non-zero indirect
//! first_instance, which needs INDIRECT_FIRST_INSTANCE (not requested).
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::OnceLock;
use wgpu::util::DeviceExt;

/// Cells per block side; `RIVER_BLOCK` in river_surface.wgsl, `BLOCK` in
/// river_blocks.wgsl.
pub const BLOCK: u32 = 16;
/// Vertex lattice side and vertex count per block; `RIVER_BLOCK_VERTICES`
/// in river_surface.wgsl.
const LATTICE: u32 = BLOCK + 1;
const BLOCK_VERTICES: u32 = LATTICE * LATTICE;
const TEMPLATES: usize = 4;
/// `Wake` in river_blocks.wgsl: a count vec4 and MAX_WAKE rectangles.
const MAX_WAKE: usize = crate::river_tiles::MAX_BODIES;
const WAKE_BYTES: u64 = 16 * (1 + MAX_WAKE as u64);
const SHADER: &str = include_str!("river_blocks.wgsl");

// r_riverBlockCull (session A/B): 0 lists every block (the full grid).
static CULL: AtomicBool = AtomicBool::new(true);
// r_riverAdaptiveMesh (session A/B): rendering only, no solver change.
static ADAPTIVE: AtomicBool = AtomicBool::new(false);
// r_riverMeshTurbulence: B's sensitivity (f32 bits; higher = more detail).
static SENSITIVITY: AtomicU32 = AtomicU32::new(0x3F80_0000); // 1.0
// r_riverWire: 0 the drawn water only, 1 the full window grid and water-mesh
// tiles including dry and never-loaded cells.
static WIRE: AtomicU8 = AtomicU8::new(0);

pub fn enabled() -> bool {
    CULL.load(Ordering::Relaxed)
}
pub fn set_enabled(enabled: bool) {
    CULL.store(enabled, Ordering::Relaxed);
}
pub fn adaptive() -> bool {
    ADAPTIVE.load(Ordering::Relaxed)
}
pub fn set_adaptive(enabled: bool) {
    ADAPTIVE.store(enabled, Ordering::Relaxed);
}
pub fn sensitivity() -> f32 {
    f32::from_bits(SENSITIVITY.load(Ordering::Relaxed))
}
pub fn set_sensitivity(value: f32) {
    if value.is_finite() {
        SENSITIVITY.store(value.clamp(0.0, 100.0).to_bits(), Ordering::Relaxed);
    }
}
pub fn wire_mode() -> u8 {
    WIRE.load(Ordering::Relaxed)
}
pub fn set_wire_mode(mode: u8) {
    WIRE.store(mode.min(1), Ordering::Relaxed);
}

/// The B compute entry points, compiled on first use only.
struct AdaptivePipelines {
    mark: wgpu::ComputePipeline,
    mark_all: wgpu::ComputePipeline,
    compact: wgpu::ComputePipeline,
}

/// Renderer-wide: the compute pipelines and the block index templates.
pub struct Pipelines {
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    module: wgpu::ShaderModule,
    mark: wgpu::ComputePipeline,
    mark_all: wgpu::ComputePipeline,
    compact: wgpu::ComputePipeline,
    adaptive: OnceLock<AdaptivePipelines>,
    /// The templates back to back; template 0 (A's) first, at offset 0.
    pub indices: wgpu::Buffer,
    /// (first index, index count) of each template in `indices`.
    ranges: [[u32; 2]; TEMPLATES],
}

impl Pipelines {
    pub fn new(device: &wgpu::Device) -> Self {
        let buffer_entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let storage = |binding| buffer_entry(binding, wgpu::BufferBindingType::Storage { read_only: false });
        let uniform = |binding| buffer_entry(binding, wgpu::BufferBindingType::Uniform);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("river window blocks"),
            entries: &[
                uniform(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                storage(2),
                storage(3),
                storage(4),
                // B: turbulence tuning, each block's template and its held
                // score (both persist between lists for the hysteresis).
                uniform(5),
                storage(6),
                storage(7),
                // B: block rectangles around the solver's wake bodies.
                uniform(8),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("river window blocks"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("river window blocks"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let (templates, ranges) = templates();
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("river window block indices"),
            contents: bytemuck::cast_slice(&templates),
            usage: wgpu::BufferUsages::INDEX,
        });
        Self {
            mark: compute_pipeline(device, &pipeline_layout, &module, "mark"),
            mark_all: compute_pipeline(device, &pipeline_layout, &module, "mark_all"),
            compact: compute_pipeline(device, &pipeline_layout, &module, "compact"),
            adaptive: OnceLock::new(),
            layout,
            pipeline_layout,
            module,
            indices,
            ranges,
        }
    }

    fn adaptive(&self, device: &wgpu::Device) -> &AdaptivePipelines {
        self.adaptive.get_or_init(|| AdaptivePipelines {
            mark: compute_pipeline(device, &self.pipeline_layout, &self.module, "mark_adaptive"),
            mark_all: compute_pipeline(device, &self.pipeline_layout, &self.module, "mark_adaptive_all"),
            compact: compute_pipeline(device, &self.pipeline_layout, &self.module, "compact_adaptive"),
        })
    }
}

fn compute_pipeline(device: &wgpu::Device, layout: &wgpu::PipelineLayout, module: &wgpu::ShaderModule, entry: &str) -> wgpu::ComputePipeline {
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(entry),
        layout: Some(layout),
        module,
        entry_point: Some(entry),
        compilation_options: Default::default(),
        cache: None,
    })
}

/// The templates back to back in `TEMPLATE_*` order (river_blocks.wgsl),
/// template `t`'s indices offset by `t * BLOCK_VERTICES`, with each
/// (first, count).
fn templates() -> (Vec<u32>, [[u32; 2]; TEMPLATES]) {
    let lattice = [block_indices(), fan_indices(2), fan_indices(4), fan_indices(8)];
    let mut out = Vec::new();
    let ranges = std::array::from_fn(|t| {
        let first = out.len() as u32;
        out.extend(lattice[t].iter().map(|&v: &u32| v + t as u32 * BLOCK_VERTICES));
        [first, out.len() as u32 - first]
    });
    (out, ranges)
}

/// GridMesh::quad's winding over one block's per-cell vertices.
fn block_indices() -> Vec<u32> {
    let width = BLOCK + 1;
    let mut out = Vec::with_capacity((BLOCK * BLOCK * 6) as usize);
    for y in 0..BLOCK {
        for x in 0..BLOCK {
            let a = (x + 1) + (y + 1) * width;
            let b = (x + 1) + y * width;
            let c = x + y * width;
            let d = x + (y + 1) * width;
            out.extend_from_slice(&[a, b, c, c, d, a]);
        }
    }
    out
}

/// Interior quads of `quad` cells (a power of two from 2), each fanned from
/// its centre vertex with GridMesh::quad's winding. An edge on the block
/// boundary keeps every cell vertex; an interior edge is whole, shared by
/// the two quads beside it.
fn fan_indices(quad: u32) -> Vec<u32> {
    assert!(quad.is_power_of_two() && (2..=BLOCK).contains(&quad));
    let vertex = |x: u32, y: u32| x + y * LATTICE;
    let side = BLOCK;
    let mut out = Vec::new();
    let mut ring = Vec::new();
    for y in (0..side).step_by(quad as usize) {
        for x in (0..side).step_by(quad as usize) {
            let centre = vertex(x + quad / 2, y + quad / 2);
            let edges = [
                ((x, y), (1, 0), y == 0),
                ((x + quad, y), (0, 1), x + quad == side),
                ((x + quad, y + quad), (-1, 0), y + quad == side),
                ((x, y + quad), (0, -1), x == 0),
            ];
            ring.clear();
            for ((sx, sy), (dx, dy), boundary) in edges {
                let segments = if boundary { quad } else { 1 };
                let length = (quad / segments) as i32;
                for i in 0..segments as i32 {
                    ring.push(vertex((sx as i32 + dx * i * length) as u32, (sy as i32 + dy * i * length) as u32));
                }
            }
            for (i, &current) in ring.iter().enumerate() {
                out.extend_from_slice(&[centre, ring[(i + 1) % ring.len()], current]);
            }
        }
    }
    out
}

/// `Tuning` in river_blocks.wgsl.
fn tuning_words(cell_cm: f32, sensitivity: f32) -> [f32; 8] {
    let h = jka_water::river::wave_advection::Height::default();
    [cell_cm, sensitivity, 0.0, 0.0, h.low_velocity_scale, h.velocity_min, h.velocity_max, h.power]
}

/// Per river: the live block list and its indirect draws over one SimGridRT.
pub struct Cull {
    group: wgpu::BindGroup,
    /// Per-block live flag (row-major), also read by the foam stage.
    flags: wgpu::Buffer,
    /// Packed block xy (x | y << 16), read by vs_block: one bucket of
    /// `blocks_xy` product entries per template. A uses bucket 0.
    pub blocks: wgpu::Buffer,
    /// One DrawIndexedIndirect record (index count, live blocks, first
    /// index, 0, 0) per template. A writes only the first.
    pub args: wgpu::Buffer,
    tuning: wgpu::Buffer,
    blocks_xy: [u32; 2],
    cell_cm: f32,
    /// (cull, adaptive, sensitivity bits) of the current list; None before
    /// the first encode.
    listed: Option<(bool, bool, u32)>,
    /// Wake rectangles (block coords, inclusive) last uploaded to `wake`.
    wake: wgpu::Buffer,
    wake_rects: Vec<[i32; 4]>,
    /// `wake_rects` changed since the last list.
    wake_changed: bool,
}

/// Cells around a wake body that keep the tile resolution while it is there
/// (the held score then keeps the trailing wake fine for about a second).
pub const WAKE_MARGIN_CELLS: f32 = 8.0;

impl Cull {
    pub fn new(device: &wgpu::Device, pipelines: &Pipelines, surface: &wgpu::Texture, cells: [u32; 2], cell_cm: f32) -> Result<Self, String> {
        if [surface.width(), surface.height()] != cells {
            return Err(format!("river block cull: SimGridRT {}x{} differs from mesh cells {cells:?}", surface.width(), surface.height()));
        }
        let blocks_xy = cells.map(|n| n.div_ceil(BLOCK));
        if blocks_xy.iter().any(|n| *n > u32::from(u16::MAX)) {
            return Err("river block cull: block grid exceeds 16-bit packing".into());
        }
        let count = u64::from(blocks_xy[0]) * u64::from(blocks_xy[1]);
        let mut settings = vec![cells[0], cells[1], blocks_xy[0], blocks_xy[1]];
        for [first, indices] in pipelines.ranges {
            settings.extend_from_slice(&[indices, first, 0, 0]);
        }
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("river window block settings"),
            contents: bytemuck::cast_slice(&settings),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false })
        };
        let flags = buffer("river window block flags", count * 4, wgpu::BufferUsages::STORAGE);
        let blocks = buffer("river window live blocks", count * 4 * TEMPLATES as u64, wgpu::BufferUsages::STORAGE);
        let template = buffer("river window block templates", count * 4, wgpu::BufferUsages::STORAGE);
        let score = buffer("river window block scores", count * 4, wgpu::BufferUsages::STORAGE);
        let wake = buffer("river window block wake", WAKE_BYTES, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
        let tuning = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("river window block tuning"),
            contents: bytemuck::cast_slice(&tuning_words(cell_cm, sensitivity())),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        // Zero instances in every record until the first compaction lands.
        let mut records = [0u32; 5 * TEMPLATES];
        records[0] = BLOCK * BLOCK * 6;
        let args = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("river window block draw"),
            contents: bytemuck::cast_slice(&records),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDIRECT,
        });
        let surface_view = surface.create_view(&wgpu::TextureViewDescriptor {
            base_mip_level: 0,
            mip_level_count: Some(1),
            ..Default::default()
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("river window blocks"),
            layout: &pipelines.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&surface_view) },
                wgpu::BindGroupEntry { binding: 2, resource: flags.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: blocks.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: args.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: tuning.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 6, resource: template.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 7, resource: score.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 8, resource: wake.as_entire_binding() },
            ],
        });
        Ok(Self { group, flags, blocks, args, tuning, blocks_xy, cell_cm, listed: None, wake, wake_rects: Vec::new(), wake_changed: false })
    }

    pub fn flags(&self) -> &wgpu::Buffer {
        &self.flags
    }

    /// Blocks per axis.
    pub fn grid(&self) -> [u32; 2] {
        self.blocks_xy
    }

    /// The indirect records to draw: all templates (B) or only A's.
    pub fn records(&self) -> u64 {
        if self.listed.is_some_and(|(_, adaptive, _)| adaptive) { TEMPLATES as u64 } else { 1 }
    }

    /// Rebuild the list after SimGridRT was rewritten (`surface_changed`) or
    /// when r_riverBlockCull, r_riverAdaptiveMesh or (B)
    /// r_riverMeshTurbulence changed; otherwise the last list stays valid.
    /// The solver's wake bodies this frame, as window-cell rectangles
    /// [x0, y0, x1, y1] (inclusive, before WAKE_MARGIN_CELLS). B forces their
    /// blocks to the tile resolution. Uploaded only when they change.
    pub fn set_wake(&mut self, queue: &wgpu::Queue, cells: impl Iterator<Item = [i32; 4]>) {
        let block = BLOCK as i32;
        let rects: Vec<[i32; 4]> = cells.take(MAX_WAKE).map(|r| r.map(|v| v.div_euclid(block))).collect();
        if rects == self.wake_rects {
            return;
        }
        let mut words = [0i32; WAKE_BYTES as usize / 4];
        words[0] = rects.len() as i32;
        for (slot, rect) in words[4..].chunks_exact_mut(4).zip(&rects) {
            slot.copy_from_slice(rect);
        }
        queue.write_buffer(&self.wake, 0, bytemuck::cast_slice(&words));
        self.wake_rects = rects;
        self.wake_changed = true;
    }

    pub fn encode(&mut self, pipelines: &Pipelines, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, surface_changed: bool) {
        let (cull, adaptive) = (enabled(), adaptive());
        let sensitivity = if adaptive { sensitivity() } else { 0.0 };
        let mode = (cull, adaptive, sensitivity.to_bits());
        let wake_changed = std::mem::take(&mut self.wake_changed) && adaptive;
        if !surface_changed && !wake_changed && self.listed == Some(mode) {
            return;
        }
        // Slider changes only: upload the new tuning ahead of the classifier.
        if adaptive && self.listed.is_none_or(|(_, _, bits)| bits != mode.2) {
            let words = tuning_words(self.cell_cm, sensitivity);
            let staging = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("river window block tuning upload"),
                contents: bytemuck::cast_slice(&words),
                usage: wgpu::BufferUsages::COPY_SRC,
            });
            encoder.copy_buffer_to_buffer(&staging, 0, &self.tuning, 0, std::mem::size_of_val(&words) as u64);
        }
        self.listed = Some(mode);
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("river window block cull"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, &self.group, &[]);
        if adaptive {
            let b = pipelines.adaptive(device);
            pass.set_pipeline(if cull { &b.mark } else { &b.mark_all });
            pass.dispatch_workgroups(self.blocks_xy[0], self.blocks_xy[1], 1);
            pass.set_pipeline(&b.compact);
        } else {
            if cull {
                pass.set_pipeline(&pipelines.mark);
                pass.dispatch_workgroups(self.blocks_xy[0], self.blocks_xy[1], 1);
            } else {
                pass.set_pipeline(&pipelines.mark_all);
                pass.dispatch_workgroups((self.blocks_xy[0] * self.blocks_xy[1]).div_ceil(64), 1, 1);
            }
            pass.set_pipeline(&pipelines.compact);
        }
        pass.dispatch_workgroups(1, 1, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jka_water::river::grid_mesh::GridMesh;
    use std::collections::HashSet;

    #[test]
    fn block_cull_shader_validates() {
        let module = wgpu::naga::front::wgsl::parse_str(SHADER).unwrap_or_else(|e| panic!("river_blocks.wgsl parses: {}", e.emit_to_string(SHADER)));
        wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(), wgpu::naga::valid::Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| panic!("river_blocks.wgsl validates: {e:?}"));
        for entry in ["mark", "mark_all", "compact", "mark_adaptive", "mark_adaptive_all", "compact_adaptive"] {
            assert!(module.entry_points.iter().any(|e| e.name == entry), "missing entry point {entry}");
        }
    }

    #[test]
    fn block_culled_foam_shader_validates() {
        let source = jka_water::river::foam_field::shader() + &crate::river_foam::block_stage();
        let module = wgpu::naga::front::wgsl::parse_str(&source).unwrap_or_else(|e| panic!("culled foam parses: {}", e.emit_to_string(&source)));
        wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(), wgpu::naga::valid::Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| panic!("culled foam validates: {e:?}"));
        for entry in ["init_foam", "foam_stage", "foam_stage_blocks"] {
            assert!(module.entry_points.iter().any(|e| e.name == entry), "missing entry point {entry}");
        }
    }

    #[test]
    fn block_indices_match_grid_mesh_quads_inside_a_block() {
        // A grid exactly one block wide has the same local vertex numbering.
        let mesh = GridMesh::new([BLOCK, BLOCK], [1.0, 1.0]).unwrap();
        let expected: Vec<u32> = (0..BLOCK * BLOCK).flat_map(|q| mesh.quad(q).unwrap()).collect();
        assert_eq!(block_indices(), expected);
    }

    #[test]
    fn templates_carry_their_number_and_a_is_first() {
        let (all, ranges) = templates();
        assert_eq!(ranges[0], [0, BLOCK * BLOCK * 6]);
        assert_eq!(&all[..ranges[0][1] as usize], &block_indices()[..]);
        for (t, [first, count]) in ranges.into_iter().enumerate() {
            assert!(all[first as usize..(first + count) as usize].iter().all(|&v| v / BLOCK_VERTICES == t as u32), "template {t}");
        }
        assert_eq!(ranges[TEMPLATES - 1][0] + ranges[TEMPLATES - 1][1], all.len() as u32);
    }

    #[test]
    fn templates_share_the_block_boundary_wind_and_tile() {
        let (all, ranges) = templates();
        let side = BLOCK as i32;
        let xy = |v: u32| ((v % BLOCK_VERTICES % LATTICE) as i32, (v % BLOCK_VERTICES / LATTICE) as i32);
        let on_edge = |(x, y): (i32, i32)| x == 0 || y == 0 || x == side || y == side;
        for (t, triangles) in [512, 288, 112, 72].into_iter().enumerate() {
            let [first, count] = ranges[t];
            let indices = &all[first as usize..(first + count) as usize];
            assert_eq!(indices.len(), triangles * 3, "template {t}");
            // Boundary vertices: every cell corner on the boundary.
            let edge: HashSet<(i32, i32)> = indices.iter().map(|&v| xy(v)).filter(|&p| on_edge(p)).collect();
            assert_eq!(edge.len(), (BLOCK * 4) as usize, "boundary vertices of template {t}");
            let mut boundary_segments = HashSet::new();
            let mut area_twice = 0;
            for face in indices.chunks_exact(3) {
                let [a, b, c] = [xy(face[0]), xy(face[1]), xy(face[2])];
                let cross = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
                assert!(cross < 0, "inverted or degenerate triangle in template {t}");
                area_twice -= cross;
                for (p, q) in [(a, b), (b, c), (c, a)] {
                    let along = (p.0 == q.0 && (p.0 == 0 || p.0 == side)) || (p.1 == q.1 && (p.1 == 0 || p.1 == side));
                    if along {
                        assert_eq!((p.0 - q.0).abs() + (p.1 - q.1).abs(), 1, "boundary segment not one cell in template {t}");
                        boundary_segments.insert((p.min(q), p.max(q)));
                    }
                }
            }
            assert_eq!(area_twice, side * side * 2, "template {t} does not tile the block");
            assert_eq!(boundary_segments.len(), (BLOCK * 4) as usize, "boundary segments of template {t}");
        }
    }
}
