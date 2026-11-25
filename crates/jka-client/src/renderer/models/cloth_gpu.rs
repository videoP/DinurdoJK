//! GPU cloth (r_clothSolver 1): gpu-cloth-sim's compute pipeline
//! (godot_gpu_cloth `GPUClothSolver`) on this renderer's device.
//!
//! The reference simulates once per rendered frame and hands positions and
//! normals to the mesh's vertex shader through storage images. Here the same
//! passes run at the fixed cloth rate, and a final pass writes each garment
//! surface straight into its reserved range of the dynamic model vertex
//! buffer, so raster, depth, shadow and RT consumers see the simulated cloth
//! with no CPU readback. Between ticks that pass blends the last two states.
use crate::renderer::{
    classic_vertex_illumination, scene, Arc, ClothGpuFrame, ClothGpuGarment, ClothGpuSurface,
    DynamicModelVertex, Ghoul2GpuBone, Ghoul2RtInputVertex, HashMap, PipelineJobKey,
    PipelineJobManager, Vec3,
};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

#[path = "cloth_unified.rs"]
mod unified;

const WORKGROUP: u32 = 64;
const METRES_PER_JKA_UNIT: f32 = 0.0254;
/// Slot 0 carries the per-frame passes; ticks use 1..=MAX_TICKS.
const MAX_TICKS: usize = 8;
const PARAM_SLOTS: usize = MAX_TICKS + 1;
const MAX_DRAW_SLOTS: usize = 32;
const MAX_COLLIDERS: usize = 64;
/// Unseen garments/instances are released after this long.
const INSTANCE_TTL: std::time::Duration = std::time::Duration::from_secs(5);
/// Same teleport rule as the CPU garment: restart from the skinned pose.
const TELEPORT_METRES: f32 = 1.5;
const MAX_FRAME_GAP_MS: i32 = 250;
/// GLM triangles wind opposite to the rendered index order, so the reference
/// face normal `cross(p1 - p0, p2 - p0)` needs `flip_normals` to face out.
const NORMAL_FLIP: f32 = -1.0;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct Params {
    dt: f32,
    gravity: f32,
    particle_count: u32,
    constraint_count: u32,
    damping: f32,
    max_speed: f32,
    collider_count: u32,
    render_count: u32,
    inertia: [f32; 3],
    max_travel: f32,
    wind: [f32; 3],
    inv_substeps: f32,
    tri_count: u32,
    bone_count: u32,
    flip: f32,
    blend: f32,
    substeps: u32,
    iterations: u32,
    group_count: u32,
    driver_count: u32,
    influence: f32,
    pad: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct DrawUniform {
    axis0: [f32; 4],
    axis1: [f32; 4],
    axis2: [f32; 4],
    origin: [f32; 4],
    color: [f32; 4],
    uv_xform: [f32; 4],
    uv_cross: [f32; 4],
    spec_light: [f32; 4],
    spec_viewer: [f32; 4],
    classic_ambient: [f32; 4],
    classic_directed: [f32; 4],
    classic_direction: [f32; 4],
    classic_sun_directed: [f32; 4],
    classic_sun_direction: [f32; 4],
    water_bands0: [f32; 4],
    water_bands1: [f32; 4],
    water_base: [f32; 4],
    params: [u32; 4],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    Skin,
    Drivers,
    Gather,
    WarmStart,
    /// Every predict/solve/collide/update of one tick, in one workgroup.
    Tick,
    FaceNormals,
    VertexNormals,
    Output,
}

const PASSES: [Pass; 8] = [
    Pass::Skin,
    Pass::Drivers,
    Pass::Gather,
    Pass::WarmStart,
    Pass::Tick,
    Pass::FaceNormals,
    Pass::VertexNormals,
    Pass::Output,
];

impl Pass {
    fn entry(self) -> &'static str {
        match self {
            Pass::Skin => "cs_skin",
            Pass::Drivers => "cs_drivers",
            Pass::Gather => "cs_gather",
            Pass::WarmStart => "cs_warm_start",
            Pass::Tick => "cs_tick",
            Pass::FaceNormals => "cs_face_normals",
            Pass::VertexNormals => "cs_vertex_normals",
            Pass::Output => "cs_output",
        }
    }

    /// Group-0 storage bindings each entry point uses (see cloth_gpu.wgsl).
    fn bindings(self) -> &'static [u32] {
        match self {
            Pass::Skin => &[0, 1, 2],
            Pass::Drivers => &[0, 1, 3, 4, 22, 24],
            Pass::Gather => &[2, 3, 4, 5, 21, 22, 23],
            Pass::WarmStart => &[5, 6, 7, 8, 18],
            Pass::Tick => &[5, 6, 7, 8, 9, 10, 11, 25],
            Pass::FaceNormals => &[6, 12, 13],
            Pass::VertexNormals => &[13, 14, 15, 16, 17],
            Pass::Output => &[0, 2, 5, 6, 17, 18, 19, 20],
        }
    }
}

pub(in crate::renderer) struct ClothGpuPipelines {
    params_layout: wgpu::BindGroupLayout,
    draw_layout: wgpu::BindGroupLayout,
    pass_layouts: Vec<wgpu::BindGroupLayout>,
    pipelines: Vec<wgpu::ComputePipeline>,
}

fn dynamic_uniform_entry(binding: u32, size: u64) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: true,
            min_binding_size: wgpu::BufferSize::new(size),
        },
        count: None,
    }
}

fn build_pipelines(device: &wgpu::Device) -> ClothGpuPipelines {
    let source = include_str!("../../cloth_gpu.wgsl");
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("JKA GPU cloth shader"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let params_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("JKA GPU cloth params layout"),
        entries: &[dynamic_uniform_entry(0, std::mem::size_of::<Params>() as u64)],
    });
    let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("JKA GPU cloth draw layout"),
        entries: &[dynamic_uniform_entry(1, std::mem::size_of::<DrawUniform>() as u64)],
    });
    let mut pass_layouts = Vec::with_capacity(PASSES.len());
    let mut pipelines = Vec::with_capacity(PASSES.len());
    for pass in PASSES {
        let entries = pass
            .bindings()
            .iter()
            .map(|&binding| {
                crate::renderer::storage_buffer_entry(binding, false, wgpu::ShaderStages::COMPUTE)
            })
            .collect::<Vec<_>>();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA GPU cloth pass layout"),
            entries: &entries,
        });
        let third = (pass == Pass::Output).then_some(&draw_layout);
        let mut layouts = vec![Some(&layout), Some(&params_layout)];
        if let Some(third) = third {
            layouts.push(Some(third));
        }
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("JKA GPU cloth pipeline layout"),
            bind_group_layouts: &layouts,
            immediate_size: 0,
        });
        pipelines.push(device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(pass.entry()),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some(pass.entry()),
            compilation_options: Default::default(),
            cache: crate::pipeline_cache::get().as_ref(),
        }));
        pass_layouts.push(layout);
    }
    ClothGpuPipelines {
        params_layout,
        draw_layout,
        pass_layouts,
        pipelines,
    }
}

fn storage_init(device: &wgpu::Device, label: &str, bytes: &[u8]) -> wgpu::Buffer {
    // Zero-length storage bindings are invalid; keep at least one vec4.
    let mut padded;
    let contents = if bytes.len() < 16 {
        padded = bytes.to_vec();
        padded.resize(16, 0);
        padded.as_slice()
    } else {
        bytes
    };
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
    })
}

fn storage_zeroed(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(16),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

fn align(size: u64, alignment: u64) -> u64 {
    size.div_ceil(alignment.max(1)) * alignment.max(1)
}

/// Static per-garment buffers, shared by every player wearing it.
struct GarmentGpu {
    garment: Arc<ClothGpuGarment>,
    render_in: wgpu::Buffer,
    member_offsets: wgpu::Buffer,
    member_list: wgpu::Buffer,
    constraints: wgpu::Buffer,
    cloth_weights: wgpu::Buffer,
    indices: wgpu::Buffer,
    vert_tri_counts: wgpu::Buffer,
    vert_tri_offsets: wgpu::Buffer,
    vert_tri_list: wgpu::Buffer,
    render_to_particle: wgpu::Buffer,
    particle_drivers: wgpu::Buffer,
    particle_rest: wgpu::Buffer,
    driver_particles: wgpu::Buffer,
    constraint_groups: wgpu::Buffer,
    /// Reference `stiffness` is baked into every constraint at build time.
    stiffness: f32,
    last_seen: std::time::Instant,
}

fn constraint_words(garment: &ClothGpuGarment, stiffness: f32) -> Vec<[f32; 4]> {
    // _push_constraint(data, a, b, rest, k)
    garment
        .constraints
        .iter()
        .map(|&(a, b, rest)| [a as f32, b as f32, rest, stiffness])
        .collect()
}

impl GarmentGpu {
    fn new(device: &wgpu::Device, garment: Arc<ClothGpuGarment>, stiffness: f32) -> Self {
        let render_in = garment
            .render_vertices
            .iter()
            .map(Ghoul2RtInputVertex::from)
            .collect::<Vec<_>>();
        let weights = garment
            .cloth_weights
            .iter()
            .map(|&w| [w, 0.0, 0.0, 0.0])
            .collect::<Vec<_>>();
        let triangle_words = garment.triangles.iter().flatten().copied().collect::<Vec<u32>>();
        let groups = garment
            .constraint_groups
            .iter()
            .map(|&(offset, count)| [offset, count, 0, 0])
            .collect::<Vec<[u32; 4]>>();
        // Two vec4<u32> per particle: driver indices + count, then weight bits.
        let drivers = garment
            .attachment_drivers
            .iter()
            .flat_map(|drivers| {
                let mut index = [0u32; 4];
                let mut weight = [0u32; 4];
                for (slot, &(driver, w)) in drivers.iter().take(3).enumerate() {
                    index[slot] = driver;
                    weight[slot] = w.to_bits();
                }
                index[3] = drivers.len().min(3) as u32;
                [index, weight]
            })
            .collect::<Vec<[u32; 4]>>();
        let rest = garment
            .bind_particles
            .iter()
            .zip(&garment.cloth_weights)
            .map(|(p, &w)| [p[0], p[1], p[2], w])
            .collect::<Vec<[f32; 4]>>();
        Self {
            render_in: storage_init(device, "JKA GPU cloth render vertices", bytemuck::cast_slice(&render_in)),
            member_offsets: storage_init(device, "JKA GPU cloth member offsets", bytemuck::cast_slice(&garment.member_offsets)),
            member_list: storage_init(device, "JKA GPU cloth member list", bytemuck::cast_slice(&garment.member_list)),
            constraints: storage_init(
                device,
                "JKA GPU cloth constraints",
                bytemuck::cast_slice(&constraint_words(&garment, stiffness)),
            ),
            cloth_weights: storage_init(device, "JKA GPU cloth weights", bytemuck::cast_slice(&weights)),
            indices: storage_init(device, "JKA GPU cloth triangles", bytemuck::cast_slice(&triangle_words)),
            vert_tri_counts: storage_init(device, "JKA GPU cloth vertex triangle counts", bytemuck::cast_slice(&garment.vert_tri_counts)),
            vert_tri_offsets: storage_init(device, "JKA GPU cloth vertex triangle offsets", bytemuck::cast_slice(&garment.vert_tri_offsets)),
            vert_tri_list: storage_init(device, "JKA GPU cloth vertex triangle list", bytemuck::cast_slice(&garment.vert_tri_list)),
            render_to_particle: storage_init(device, "JKA GPU cloth render to particle", bytemuck::cast_slice(&garment.render_to_particle)),
            particle_drivers: storage_init(device, "JKA GPU cloth particle drivers", bytemuck::cast_slice(&drivers)),
            particle_rest: storage_init(device, "JKA GPU cloth particle rest", bytemuck::cast_slice(&rest)),
            driver_particles: storage_init(device, "JKA GPU cloth driver particles", bytemuck::cast_slice(&garment.driver_particles)),
            constraint_groups: storage_init(device, "JKA GPU cloth constraint groups", bytemuck::cast_slice(&groups)),
            garment,
            stiffness,
            last_seen: std::time::Instant::now(),
        }
    }
}

/// One player's simulation state.
struct InstanceGpu {
    bones: wgpu::Buffer,
    bone_capacity: usize,
    skinned_render: wgpu::Buffer,
    skinned_targets: wgpu::Buffer,
    positions: wgpu::Buffer,
    predicted: wgpu::Buffer,
    velocities: wgpu::Buffer,
    colliders: wgpu::Buffer,
    face_normals: wgpu::Buffer,
    particle_normals: wgpu::Buffer,
    prev_positions: wgpu::Buffer,
    driver_mats: wgpu::Buffer,
    params: wgpu::Buffer,
    draws: wgpu::Buffer,
    params_bind_group: wgpu::BindGroup,
    draw_bind_group: wgpu::BindGroup,
    /// Group-0 bind groups per pass except Output; rebuilt when bones grow.
    pass_bind_groups: Vec<Option<wgpu::BindGroup>>,
    output_bind_group: Option<(wgpu::Buffer, wgpu::BindGroup)>,
    needs_warm_start: bool,
    backend: u8,
    unified_pose: Option<unified::Pose>,
    unified_state: Option<unified::InstanceState>,
    last_unified_frame: Option<u64>,
    accumulator: f32,
    last_time_ms: Option<i32>,
    /// Model origin at the previous simulated tick (reference `_prev_mesh_world_pos`).
    tick_origin: [f32; 3],
    last_seen: std::time::Instant,
}

impl InstanceGpu {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipelines: &ClothGpuPipelines,
        garment: &ClothGpuGarment,
        param_stride: u64,
        draw_stride: u64,
    ) -> Self {
        let particles = garment.particle_count() as u64;
        let render = garment.render_vertices.len() as u64;
        let triangles = garment.triangles.len() as u64;
        let positions = storage_zeroed(device, "JKA GPU cloth positions", particles * 16);
        // inv_mass: 0 for anchored particles (cloth weight < 0.01), else 1.
        let initial = garment
            .cloth_weights
            .iter()
            .map(|&w| [0.0, 0.0, 0.0, if w < 0.01 { 0.0 } else { 1.0 }])
            .collect::<Vec<[f32; 4]>>();
        if !initial.is_empty() {
            queue.write_buffer(&positions, 0, bytemuck::cast_slice(&initial));
        }
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA GPU cloth params"),
            size: param_stride * PARAM_SLOTS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let draws = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA GPU cloth draws"),
            size: draw_stride * MAX_DRAW_SLOTS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let params_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("JKA GPU cloth params"),
            layout: &pipelines.params_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &params,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<Params>() as u64),
                }),
            }],
        });
        let draw_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("JKA GPU cloth draws"),
            layout: &pipelines.draw_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &draws,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<DrawUniform>() as u64),
                }),
            }],
        });
        Self {
            bones: storage_zeroed(device, "JKA GPU cloth bones", 128 * 48),
            bone_capacity: 128,
            skinned_render: storage_zeroed(device, "JKA GPU cloth skinned render", render * 32),
            skinned_targets: storage_zeroed(device, "JKA GPU cloth skinned targets", particles * 16),
            positions,
            predicted: storage_zeroed(device, "JKA GPU cloth predicted", particles * 16),
            velocities: storage_zeroed(device, "JKA GPU cloth velocities", particles * 16),
            colliders: storage_zeroed(device, "JKA GPU cloth colliders", MAX_COLLIDERS as u64 * 64),
            face_normals: storage_zeroed(device, "JKA GPU cloth face normals", triangles * 16),
            particle_normals: storage_zeroed(device, "JKA GPU cloth normals", particles * 16),
            prev_positions: storage_zeroed(device, "JKA GPU cloth previous positions", particles * 16),
            driver_mats: storage_zeroed(
                device,
                "JKA GPU cloth attachment transforms",
                garment.driver_particles.len() as u64 * 48,
            ),
            params,
            draws,
            params_bind_group,
            draw_bind_group,
            pass_bind_groups: vec![None; PASSES.len()],
            output_bind_group: None,
            needs_warm_start: true,
            backend: 0,
            unified_pose: None,
            unified_state: None,
            last_unified_frame: None,
            accumulator: 0.0,
            last_time_ms: None,
            tick_origin: [0.0; 3],
            last_seen: std::time::Instant::now(),
        }
    }

    fn binding<'a>(
        &'a self,
        garment: &'a GarmentGpu,
        binding: u32,
        output: Option<&'a wgpu::Buffer>,
    ) -> &'a wgpu::Buffer {
        match binding {
            0 => &garment.render_in,
            1 => &self.bones,
            2 => &self.skinned_render,
            3 => &garment.member_offsets,
            4 => &garment.member_list,
            5 => &self.skinned_targets,
            6 => &self.positions,
            7 => &self.predicted,
            8 => &self.velocities,
            9 => &garment.constraints,
            10 => &self.colliders,
            11 => &garment.cloth_weights,
            12 => &garment.indices,
            13 => &self.face_normals,
            14 => &garment.vert_tri_counts,
            15 => &garment.vert_tri_offsets,
            16 => &garment.vert_tri_list,
            17 => &self.particle_normals,
            18 => &self.prev_positions,
            19 => &garment.render_to_particle,
            20 => output.expect("output binding needs the target vertex buffer"),
            21 => &garment.particle_drivers,
            22 => &self.driver_mats,
            23 => &garment.particle_rest,
            24 => &garment.driver_particles,
            25 => &garment.constraint_groups,
            _ => unreachable!("cloth_gpu.wgsl has no binding {binding}"),
        }
    }

    fn pass_bind_group(
        &self,
        device: &wgpu::Device,
        pipelines: &ClothGpuPipelines,
        garment: &GarmentGpu,
        pass_index: usize,
        output: Option<&wgpu::Buffer>,
    ) -> wgpu::BindGroup {
        let pass = PASSES[pass_index];
        let entries = pass
            .bindings()
            .iter()
            .map(|&binding| wgpu::BindGroupEntry {
                binding,
                resource: self.binding(garment, binding, output).as_entire_binding(),
            })
            .collect::<Vec<_>>();
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(pass.entry()),
            layout: &pipelines.pass_layouts[pass_index],
            entries: &entries,
        })
    }
}

struct PendingDraw {
    surface: ClothGpuSurface,
    base_vertex: u32,
    classic: Option<scene::ClassicEntityLight>,
    water: Option<crate::water_wetness::WaterProfile>,
}

/// One instance's GPU work this frame.
struct Dispatch {
    key: (u16, Arc<str>),
    warm_start: bool,
    unified: bool,
    ticks: u32,
    drivers: bool,
    /// (draw slot, vertex count) of each output dispatch.
    outputs: Vec<(u32, u32)>,
}

#[derive(Default)]
pub(in crate::renderer) struct ClothGpuRenderer {
    pipelines: Option<ClothGpuPipelines>,
    unified: unified::UnifiedGpu,
    frame_serial: u64,
    garments: HashMap<Arc<str>, GarmentGpu>,
    instances: HashMap<(u16, Arc<str>), InstanceGpu>,
    pending: Vec<PendingDraw>,
    dispatches: Vec<Dispatch>,
}

impl ClothGpuRenderer {
    pub(in crate::renderer) fn begin_frame(&mut self) {
        self.pending.clear();
        self.dispatches.clear();
        self.unified.begin_frame();
        self.frame_serial = self.frame_serial.wrapping_add(1);
    }

    /// GPU work was scheduled this frame (the vertex buffer must be STORAGE).
    pub(in crate::renderer) fn has_dispatches(&self) -> bool {
        self.pipelines.is_some() && !self.dispatches.is_empty()
    }

    /// Called from model preparation after the surface's zeroed vertex range
    /// was reserved at `base_vertex`.
    pub(in crate::renderer) fn record(
        &mut self,
        surface: &ClothGpuSurface,
        base_vertex: u32,
        classic: Option<scene::ClassicEntityLight>,
        water: Option<crate::water_wetness::WaterProfile>,
    ) {
        self.pending.push(PendingDraw {
            surface: surface.clone(),
            base_vertex,
            classic,
            water,
        });
    }

    /// Schedule this frame's simulation and outputs. Surfaces that cannot be
    /// GPU-written yet (pipelines still compiling, slot overflow) are filled
    /// on the CPU with their skinned pose so they never draw empty.
    pub(in crate::renderer) fn prepare(
        &mut self,
        jobs: &mut PipelineJobManager,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vertices: &mut [DynamicModelVertex],
    ) {
        if self.pending.is_empty() {
            self.unified.reset_clock();
            self.evict();
            return;
        }
        if self.pipelines.is_none() {
            let key = PipelineJobKey::new("gpu-cloth", 0, 0);
            if let Some(pipelines) = jobs.take_ready::<ClothGpuPipelines>(key) {
                self.pipelines = Some(pipelines);
            } else {
                let device = device.clone();
                jobs.request(key, "GPU cloth compute pipelines", move || build_pipelines(&device));
            }
        }
        let pending = std::mem::take(&mut self.pending);
        let Some(pipelines) = self.pipelines.as_ref() else {
            for draw in &pending {
                cpu_skinned_fill(draw, vertices);
            }
            self.pending = pending;
            self.pending.clear();
            return;
        };
        // Both GPU modes share the existing skin/normal/output pipelines.
        // Only mode 2 requests its additional global compute pipelines.
        let unified_frame = pending.iter().find(|draw| {
            let sim = &draw.surface.frame;
            sim.tuning.solver == 2 && unified::Pose::from_frame(sim).is_some()
        });
        let unified_timing = if let Some(draw) = unified_frame {
            if self.unified.ready(jobs, device) {
                Some(self.unified.timing(&draw.surface.frame))
            } else {
                self.unified.reset_clock();
                None
            }
        } else {
            self.unified.reset_clock();
            None
        };
        let alignment = u64::from(device.limits().min_uniform_buffer_offset_alignment);
        let param_stride = align(std::mem::size_of::<Params>() as u64, alignment);
        let draw_stride = align(std::mem::size_of::<DrawUniform>() as u64, alignment);
        let now = std::time::Instant::now();

        let mut draw_slots = HashMap::<(u16, Arc<str>), u32>::new();
        let mut draw_bytes = HashMap::<(u16, Arc<str>), Vec<u8>>::new();
        for draw in &pending {
            let sim = &draw.surface.frame;
            let pose = if sim.tuning.solver == 2 {
                let pose = unified::Pose::from_frame(sim);
                if unified_timing.is_none() || pose.is_none() {
                    cpu_skinned_fill(draw, vertices);
                    continue;
                }
                pose
            } else { None };
            let garment_key = Arc::clone(&sim.garment.key);
            let key = (sim.entity_num, Arc::clone(&garment_key));
            let stiffness = sim.tuning.stiffness;

            let garment_gpu = self
                .garments
                .entry(Arc::clone(&garment_key))
                .or_insert_with(|| {
                    GarmentGpu::new(device, Arc::clone(&sim.garment), stiffness)
                });
            garment_gpu.last_seen = now;
            if garment_gpu.stiffness != stiffness {
                queue.write_buffer(
                    &garment_gpu.constraints,
                    0,
                    bytemuck::cast_slice(&constraint_words(&garment_gpu.garment, stiffness)),
                );
                garment_gpu.stiffness = stiffness;
            }

            let instance = self.instances.entry(key.clone()).or_insert_with(|| {
                InstanceGpu::new(device, queue, pipelines, &sim.garment, param_stride, draw_stride)
            });
            instance.last_seen = now;

            // First draw of this instance this frame schedules the simulation.
            if !self.dispatches.iter().any(|dispatch| dispatch.key == key) {
                if instance.backend != sim.tuning.solver {
                    instance.needs_warm_start = true;
                    instance.unified_pose = None;
                    instance.unified_state = None;
                    instance.backend = sim.tuning.solver;
                }
                let timing = if let Some(pose) = pose {
                    // Culled/absent garments missed the global world's ticks.
                    // Restart from current skin, never inject old detached cloth.
                    if instance.last_unified_frame != Some(self.frame_serial.wrapping_sub(1))
                        || instance.unified_pose.is_none_or(|old| pose.changed_scale(old)) {
                        instance.needs_warm_start = true;
                    }
                    instance.last_unified_frame = Some(self.frame_serial);
                    unified_timing
                } else { None };
                self.dispatches.push(schedule(instance, sim, device, queue, param_stride, timing));
            }

            let slot = draw_slots.entry(key.clone()).or_insert(0);
            if *slot as usize >= MAX_DRAW_SLOTS {
                cpu_skinned_fill(draw, vertices);
                continue;
            }
            let Some(&(first, count)) = sim.garment.surface_ranges.get(draw.surface.surface as usize)
            else {
                continue;
            };
            let uniform = draw_uniform(draw, first, count);
            let bytes = draw_bytes.entry(key.clone()).or_default();
            bytes.resize((*slot as u64 * draw_stride) as usize, 0);
            bytes.extend_from_slice(bytemuck::bytes_of(&uniform));
            let dispatch = self
                .dispatches
                .iter_mut()
                .find(|dispatch| dispatch.key == key)
                .expect("scheduled above");
            dispatch.outputs.push((*slot, count));
            *slot += 1;
        }
        for (key, bytes) in draw_bytes {
            if let Some(instance) = self.instances.get(&key) {
                queue.write_buffer(&instance.draws, 0, &bytes);
            }
        }
        // Group-0 bind groups for the simulation passes (output is bound
        // once the target vertex buffer is final).
        for dispatch in &self.dispatches {
            let (Some(instance), Some(garment)) = (
                self.instances.get(&dispatch.key),
                self.garments.get(&dispatch.key.1),
            ) else {
                continue;
            };
            if instance.pass_bind_groups.iter().take(PASSES.len() - 1).any(Option::is_none) {
                let groups = (0..PASSES.len())
                    .map(|index| {
                        (PASSES[index] != Pass::Output)
                            .then(|| instance.pass_bind_group(device, pipelines, garment, index, None))
                    })
                    .collect();
                self.instances.get_mut(&dispatch.key).unwrap().pass_bind_groups = groups;
            }
        }
        if let Some(timing) = unified_timing {
            if !self.unified.prepare(device, queue, &pending, &self.dispatches, &mut self.instances, timing) {
                // No partial/truncated solver world, and no empty vertex ranges.
                for draw in &pending {
                    if draw.surface.frame.tuning.solver == 2 {
                        cpu_skinned_fill(draw, vertices);
                    }
                }
                for dispatch in self.dispatches.iter().filter(|d| d.unified) {
                    if let Some(instance) = self.instances.get_mut(&dispatch.key) {
                        instance.needs_warm_start = true;
                    }
                }
                self.dispatches.retain(|d| !d.unified);
                self.unified.reset_clock();
            }
        }
        self.pending = pending;
        self.pending.clear();
        self.evict();
    }

    /// Bind each scheduled output to the final dynamic vertex buffer.
    pub(in crate::renderer) fn bind_output(&mut self, device: &wgpu::Device, target: &wgpu::Buffer) {
        let Some(pipelines) = self.pipelines.as_ref() else {
            return;
        };
        let output_index = PASSES.iter().position(|&pass| pass == Pass::Output).unwrap();
        for dispatch in &self.dispatches {
            let Some(garment) = self.garments.get(&dispatch.key.1) else {
                continue;
            };
            let Some(instance) = self.instances.get(&dispatch.key) else {
                continue;
            };
            if instance.output_bind_group.as_ref().is_some_and(|(buffer, _)| buffer == target) {
                continue;
            }
            let group = instance.pass_bind_group(device, pipelines, garment, output_index, Some(target));
            self.instances.get_mut(&dispatch.key).unwrap().output_bind_group =
                Some((target.clone(), group));
        }
    }

    pub(in crate::renderer) fn encode(&self, encoder: &mut wgpu::CommandEncoder) {
        let Some(pipelines) = self.pipelines.as_ref() else {
            return;
        };
        self.encode_unified(encoder);
        for dispatch in &self.dispatches {
            if dispatch.unified { continue; }
            let (Some(instance), Some(garment)) = (
                self.instances.get(&dispatch.key),
                self.garments.get(&dispatch.key.1),
            ) else {
                continue;
            };
            if instance.pass_bind_groups.iter().take(PASSES.len() - 1).any(Option::is_none) {
                continue;
            }
            let Some((_, output_group)) = instance.output_bind_group.as_ref() else {
                continue;
            };
            let context = EncodeContext {
                pipelines,
                instance,
                param_stride: instance.params.size() / PARAM_SLOTS as u64,
            };
            let particles = garment.garment.particle_count() as u32;
            let render = garment.garment.render_vertices.len() as u32;
            let triangles = garment.garment.triangles.len() as u32;
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("JKA GPU cloth simulate"),
                    timestamp_writes: None,
                });
                // SKIN every frame, so attachments follow the current pose.
                context.bind(&mut pass, Pass::Skin, 0);
                pass.dispatch_workgroups(groups_of(render), 1, 1);
                if dispatch.drivers {
                    context.bind(&mut pass, Pass::Drivers, 0);
                    pass.dispatch_workgroups(groups_of(garment.garment.driver_particles.len() as u32), 1, 1);
                }
                context.bind(&mut pass, Pass::Gather, 0);
                pass.dispatch_workgroups(groups_of(particles), 1, 1);
                if dispatch.warm_start {
                    context.bind(&mut pass, Pass::WarmStart, 0);
                    pass.dispatch_workgroups(groups_of(particles), 1, 1);
                }
                for tick in 1..dispatch.ticks {
                    context.tick(&mut pass, u64::from(tick));
                }
            }
            if dispatch.ticks > 0 {
                // The output pass blends from the state before the newest tick.
                encoder.copy_buffer_to_buffer(
                    &instance.positions,
                    0,
                    &instance.prev_positions,
                    0,
                    instance.positions.size(),
                );
            }
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("JKA GPU cloth output"),
                timestamp_writes: None,
            });
            if dispatch.ticks > 0 {
                context.tick(&mut pass, u64::from(dispatch.ticks));
            }
            if dispatch.ticks > 0 || dispatch.warm_start {
                // NORMALS
                context.bind(&mut pass, Pass::FaceNormals, 0);
                pass.dispatch_workgroups(groups_of(triangles), 1, 1);
                context.bind(&mut pass, Pass::VertexNormals, 0);
                pass.dispatch_workgroups(groups_of(particles), 1, 1);
            }
            // OUTPUT
            let draw_stride = instance.draws.size() / MAX_DRAW_SLOTS as u64;
            pass.set_pipeline(&pipelines.pipelines[pass_index(Pass::Output)]);
            pass.set_bind_group(0, output_group, &[]);
            pass.set_bind_group(1, &instance.params_bind_group, &[0]);
            for &(slot, count) in &dispatch.outputs {
                pass.set_bind_group(
                    2,
                    &instance.draw_bind_group,
                    &[(u64::from(slot) * draw_stride) as u32],
                );
                pass.dispatch_workgroups(groups_of(count), 1, 1);
            }
        }
    }

    fn evict(&mut self) {
        let now = std::time::Instant::now();
        self.instances
            .retain(|_, instance| now.duration_since(instance.last_seen) <= INSTANCE_TTL);
        let instances = &self.instances;
        self.garments.retain(|key, garment| {
            now.duration_since(garment.last_seen) <= INSTANCE_TTL
                || instances.keys().any(|(_, garment_key)| garment_key == key)
        });
    }
}

fn pass_index(pass: Pass) -> usize {
    PASSES.iter().position(|&p| p == pass).unwrap()
}

fn groups_of(count: u32) -> u32 {
    count.div_ceil(WORKGROUP).max(1)
}

struct EncodeContext<'a> {
    pipelines: &'a ClothGpuPipelines,
    instance: &'a InstanceGpu,
    param_stride: u64,
}

impl EncodeContext<'_> {
    fn bind(&self, pass: &mut wgpu::ComputePass<'_>, p: Pass, slot: u64) {
        pass.set_pipeline(&self.pipelines.pipelines[pass_index(p)]);
        pass.set_bind_group(0, self.instance.pass_bind_groups[pass_index(p)].as_ref().unwrap(), &[]);
        pass.set_bind_group(1, &self.instance.params_bind_group, &[(slot * self.param_stride) as u32]);
    }

    /// One reference `_gpu_do_simulate` frame (= one fixed cloth tick): the
    /// substep loop runs inside cs_tick, one workgroup per garment.
    fn tick(&self, pass: &mut wgpu::ComputePass<'_>, slot: u64) {
        self.bind(pass, Pass::Tick, slot);
        pass.dispatch_workgroups(1, 1, 1);
    }
}

/// Fixed-rate accumulator and per-tick parameters (reference `_simulate`).
fn schedule(
    instance: &mut InstanceGpu,
    frame: &ClothGpuFrame,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    param_stride: u64,
    unified_timing: Option<unified::Timing>,
) -> Dispatch {
    let tuning = frame.tuning;
    let hz = tuning.hz.clamp(15, 240);
    let h = 1.0 / hz as f32;
    let substeps = tuning.substeps.clamp(1, 64);
    let iterations = tuning.solver_iterations.clamp(1, 64);
    let max_ticks = (tuning.max_ticks.clamp(1, MAX_TICKS as u32)) as usize;

    let origin = Vec3::from_array(frame.origin);
    let teleported = instance.last_time_ms.is_some()
        && (origin - Vec3::from_array(instance.tick_origin)).length() * METRES_PER_JKA_UNIT
            > TELEPORT_METRES;
    let mut warm_start = instance.needs_warm_start || teleported;
    let mut ticks = 0usize;
    match instance.last_time_ms {
        Some(last) if !warm_start => {
            let delta = frame.time_ms.saturating_sub(last);
            if delta < 0 || delta > MAX_FRAME_GAP_MS {
                warm_start = true;
            } else {
                instance.accumulator += delta as f32 * 0.001;
                while instance.accumulator + f32::EPSILON >= h && ticks < max_ticks {
                    instance.accumulator -= h;
                    ticks += 1;
                }
                if ticks == max_ticks && instance.accumulator >= h {
                    instance.accumulator %= h;
                }
            }
        }
        _ => warm_start = true,
    }
    if warm_start {
        instance.accumulator = 0.0;
        instance.tick_origin = frame.origin;
        ticks = 0;
    }
    if let Some(timing) = unified_timing {
        warm_start |= timing.reset;
        ticks = timing.ticks as usize;
        instance.accumulator = timing.blend * h;
        if warm_start { instance.tick_origin = frame.origin; }
    }
    instance.last_time_ms = Some(frame.time_ms);
    instance.needs_warm_start = false;

    let garment = &frame.garment;
    let axis = glam::Mat3::from_cols_array_2d(&frame.axis);
    let to_local = if axis.determinant().abs() > 1.0e-6 {
        axis.inverse()
    } else {
        glam::Mat3::IDENTITY
    };
    // Inertia: model movement since the last tick, shared across this frame's ticks.
    let delta_world = (origin - Vec3::from_array(instance.tick_origin)) * METRES_PER_JKA_UNIT;
    let delta_local = to_local * delta_world;
    if ticks > 0 {
        instance.tick_origin = frame.origin;
    }
    let inertia_scale = Vec3::from_array(tuning.inertia_scale);
    let inertia_sub = if ticks > 0 {
        delta_local * inertia_scale / (ticks as f32 * substeps as f32)
    } else {
        Vec3::ZERO
    };

    let bone_count = frame.bones.len().max(1) as u32;
    let collider_count = if frame.colliders.is_empty() {
        0
    } else {
        frame.colliders.len().min(MAX_COLLIDERS)
    };
    let dt = h / substeps as f32;
    let base = Params {
        dt,
        gravity: tuning.gravity_strength,
        particle_count: garment.particle_count() as u32,
        constraint_count: garment.constraints.len() as u32,
        // r_clothDamping is an exponential rate (1/s); the reference damping
        // is the velocity kept per substep.
        damping: (-tuning.damping.max(0.0) * dt).exp(),
        max_speed: tuning.max_speed,
        collider_count: collider_count as u32,
        render_count: garment.render_vertices.len() as u32,
        inertia: [0.0; 3],
        max_travel: garment.max_travel,
        wind: [0.0; 3],
        inv_substeps: 1.0 / substeps as f32,
        tri_count: garment.triangles.len() as u32,
        bone_count,
        flip: NORMAL_FLIP,
        blend: if warm_start && (unified_timing.is_none() || ticks == 0) {
            1.0
        } else { (instance.accumulator / h).clamp(0.0, 1.0) },
        substeps,
        iterations,
        group_count: garment.constraint_groups.len() as u32,
        driver_count: garment.driver_particles.len() as u32,
        influence: if unified_timing.is_some() { 1.0 } else { tuning.animation_influence },
        pad: [0.0; 3],
    };
    let legacy_ticks = if unified_timing.is_some() { 0 } else { ticks };
    let mut bytes = vec![0u8; (param_stride as usize) * (legacy_ticks + 1)];
    bytes[..std::mem::size_of::<Params>()].copy_from_slice(bytemuck::bytes_of(&base));
    let wind = Vec3::from_array(frame.wind);
    for tick in 1..=legacy_ticks {
        // Reference gust: Time.get_ticks_msec() / 1000 * wind_frequency, sampled
        // at this tick's simulated time.
        let tick_time = frame.time_ms as f32 * 0.001
            - instance.accumulator
            - (ticks - tick) as f32 * h;
        let t = tick_time * tuning.wind_frequency;
        let gust = Vec3::new(
            (t * 1.7).sin() + (t * 3.1 + 1.3).sin(),
            (t * 1.3 + 2.0).sin() + (t * 2.7 + 0.7).sin(),
            (t * 2.1 + 4.0).sin() + (t * 1.9 + 3.1).sin(),
        ) * 0.5;
        let effective = wind + wind.length() * gust * tuning.wind_turbulence;
        let params = Params {
            inertia: inertia_sub.to_array(),
            wind: (to_local * effective).to_array(),
            ..base
        };
        let at = tick * param_stride as usize;
        bytes[at..at + std::mem::size_of::<Params>()].copy_from_slice(bytemuck::bytes_of(&params));
    }
    queue.write_buffer(&instance.params, 0, &bytes);

    if frame.bones.len() > instance.bone_capacity {
        // Bones are per instance; growth invalidates the bind groups using them.
        instance.bone_capacity = frame.bones.len().next_power_of_two();
        instance.bones = storage_zeroed(
            device,
            "JKA GPU cloth bones",
            instance.bone_capacity as u64 * 48,
        );
        instance.pass_bind_groups.fill(None);
    }
    if !frame.bones.is_empty() {
        queue.write_buffer(&instance.bones, 0, bytemuck::cast_slice(frame.bones.as_slice()));
    }
    if unified_timing.is_none() && collider_count > 0 {
        queue.write_buffer(
            &instance.colliders,
            0,
            bytemuck::cast_slice(&frame.colliders[..collider_count]),
        );
    }
    Dispatch {
        key: (frame.entity_num, Arc::clone(&garment.key)),
        warm_start,
        unified: unified_timing.is_some(),
        ticks: ticks as u32,
        drivers: unified_timing.is_none() && !garment.driver_particles.is_empty(),
        outputs: Vec::new(),
    }
}

fn draw_uniform(draw: &PendingDraw, first: u32, count: u32) -> DrawUniform {
    let sim = &draw.surface.frame;
    let light = draw.classic;
    let v4 = |v: [f32; 3]| [v[0], v[1], v[2], 0.0];
    let (spec_light, spec_viewer) = draw
        .surface
        .specular
        .map_or(([0.0; 4], [0.0; 4]), |(light, viewer)| {
            ([light[0], light[1], light[2], 1.0], v4(viewer))
        });
    DrawUniform {
        axis0: v4(sim.axis[0]),
        axis1: v4(sim.axis[1]),
        axis2: v4(sim.axis[2]),
        origin: v4(sim.origin),
        color: draw.surface.color,
        uv_xform: draw.surface.uv_xform,
        uv_cross: draw.surface.uv_cross,
        spec_light,
        spec_viewer,
        classic_ambient: light.map_or([0.0; 4], |l| v4(l.ambient)),
        classic_directed: light.map_or([0.0; 4], |l| v4(l.directed)),
        classic_direction: light.map_or([0.0; 4], |l| v4(l.direction)),
        classic_sun_directed: light.map_or([0.0; 4], |l| v4(l.sun_directed)),
        classic_sun_direction: light.map_or([0.0; 4], |l| v4(l.sun_direction)),
        water_bands0: draw.water.map_or([0.0; 4], |p| [p.bands[0], p.bands[1], p.bands[2], p.bands[3]]),
        water_bands1: draw.water.map_or([0.0; 4], |p| [p.bands[4], p.bands[5], p.bands[6], p.bands[7]]),
        water_base: draw.water.map_or([0.0; 4], |p| [p.first_z, 0.0, 0.0, 1.0]),
        params: [
            draw.base_vertex,
            first,
            count,
            u32::from(light.is_some())
                | (u32::from(light.is_some() && super::types::jk2_encoded_entity_light()) << 1),
        ],
    }
}

/// Unsimulated fallback: the skinned pose, exactly as the CPU fills a
/// GPU-skinned surface (Ghoul2 weighting, weight-0 normal).
fn cpu_skinned_fill(draw: &PendingDraw, vertices: &mut [DynamicModelVertex]) {
    let sim = &draw.surface.frame;
    let Some(&(first, count)) = sim.garment.surface_ranges.get(draw.surface.surface as usize) else {
        return;
    };
    let bone = |index: u32| -> Option<&Ghoul2GpuBone> {
        sim.bones.get(index as usize).or_else(|| sim.bones.last())
    };
    let point = |bone: &Ghoul2GpuBone, p: [f32; 3], w: f32| {
        let row = |r: [f32; 4]| r[0] * p[0] + r[1] * p[1] + r[2] * p[2] + r[3] * w;
        Vec3::new(row(bone.row0), row(bone.row1), row(bone.row2))
    };
    let base = draw.base_vertex as usize;
    for local in 0..count as usize {
        let Some(source) = sim.garment.render_vertices.get(first as usize + local) else {
            break;
        };
        let Some(out) = vertices.get_mut(base + local) else {
            break;
        };
        let p = |slot: usize| bone(source.bone_indices[slot]).map_or(Vec3::ZERO, |b| point(b, source.position, 1.0));
        let w = source.weights;
        let model = match source.weight_count {
            0 | 1 => p(0),
            2 => w[0] * (p(0) - p(1)) + p(1),
            3 => w[0] * p(0) + w[1] * p(1) + (1.0 - w[0] - w[1]) * p(2),
            _ => w[0] * p(0) + w[1] * p(1) + w[2] * p(2) + (1.0 - w[0] - w[1] - w[2]) * p(3),
        };
        let normal = bone(source.bone_indices[0]).map_or(Vec3::Z, |b| point(b, source.normal, 0.0));
        let axis = glam::Mat3::from_cols_array_2d(&sim.axis);
        let world = Vec3::from_array(sim.origin) + axis * model;
        let world_normal = axis * normal;
        let render = scene::render_position(world.to_array());
        let render_normal = Vec3::from_array(scene::render_position(world_normal.to_array()))
            .try_normalize()
            .unwrap_or(Vec3::Y);
        let mut color = draw.surface.color;
        if let Some(light) = draw.classic.as_ref() {
            let illumination = classic_vertex_illumination(light, render_normal);
            for channel in 0..3 {
                color[channel] *= illumination[channel];
            }
        }
        let uv = source.uv;
        let x = draw.surface.uv_xform;
        let c = draw.surface.uv_cross;
        *out = DynamicModelVertex {
            position: render,
            normal: render_normal.to_array(),
            uv: [uv[0] * x[0] + x[2] + uv[1] * c[0], uv[1] * x[1] + x[3] + uv[0] * c[1]],
            color,
            depth_hack: 0.0,
            water_wetness: draw.water.map_or(0.0, |water| water.wetness_at_z(render[1])),
            foliage_wind: 0.0,
        };
    }
}
