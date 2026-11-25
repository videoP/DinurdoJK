//! Unified Solver cloth backend: one world-space XPBD/Jacobi particle pool,
//! native WGPU dispatches following SolverManager.FixedUpdate, plus Ghoul2
//! pack/scatter adapters. Source revision: 5e00799568ff9249bfe3c809818cbdcf93a79619.
use super::{
    groups_of, pass_index, storage_zeroed, ClothGpuFrame, ClothGpuRenderer,
    Dispatch, EncodeContext, InstanceGpu, Pass, PendingDraw, PipelineJobKey,
    PipelineJobManager, Arc, HashMap, Pod, Zeroable, Vec3, MAX_DRAW_SLOTS,
    MAX_FRAME_GAP_MS, MAX_TICKS, METRES_PER_JKA_UNIT, PARAM_SLOTS,
};
use glam::Mat3;

// SolverManager defaults. The table is fixed for the entire active pool, not
// allocated separately per character. No hash is cleared on render-only frames.
const TABLE_SIZE: u32 = 262139;
const THREADS: u32 = 256;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SolverParams {
    particle_count: u32, constraint_count: u32, table_size: u32, sphere_count: u32,
    capsule_count: u32, box_count: u32, dt: f32, sor: f32,
    gravity: [f32; 3], damping: f32,
    cell_size: f32, radius: f32, friction_static: f32, friction_kinetic: f32,
    max_depenetration_speed: f32, ground_z: f32, rigid_count: u32, pad: f32,
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Constraint {
    a: u32, b: u32, rest_length: f32, compliance: f32, break_force: f32, damping: f32,
}
#[repr(C)]
#[derive(Clone, Copy, Default, Pod, Zeroable)]
struct Collider { a: [f32; 4], b: [f32; 4], x: [f32; 4], y: [f32; 4], z: [f32; 4] }
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TransferUniform {
    new_axis: [[f32; 4]; 3], new_origin: [f32; 4],
    inverse_axis: [[f32; 4]; 3],
    offset: u32, count: u32, phase: i32, reset: u32,
}
const _: () = {
    assert!(std::mem::size_of::<SolverParams>() == 80);
    assert!(std::mem::size_of::<Constraint>() == 24);
    assert!(std::mem::size_of::<Collider>() == 80);
    assert!(std::mem::size_of::<TransferUniform>() == 128);
};

fn vec4(v: Vec3, w: f32) -> [f32; 4] { [v.x, v.y, v.z, w] }
fn axis4(m: Mat3) -> [[f32; 4]; 3] {
    [vec4(m.x_axis, 0.0), vec4(m.y_axis, 0.0), vec4(m.z_axis, 0.0)]
}

/// The authority has world-space spheres/capsules/OBBs. A non-uniform or
/// sheared entity matrix would transform a sphere into an unsupported
/// ellipsoid; reject that frame instead of silently approximating its shape.
#[derive(Clone, Copy)]
pub(super) struct Pose {
    axis: Mat3,
    origin: Vec3,
    scale: f32,
}
impl Pose {
    pub(super) fn from_frame(frame: &ClothGpuFrame) -> Option<Self> {
        let axis = Mat3::from_cols_array_2d(&frame.axis);
        let origin = Vec3::from_array(frame.origin) * METRES_PER_JKA_UNIT;
        let s = axis.x_axis.length();
        if !origin.is_finite() || !(axis.x_axis.is_finite() && axis.y_axis.is_finite() && axis.z_axis.is_finite()) || s <= 1e-6 { return None; }
        let y = axis.y_axis.length();
        let z = axis.z_axis.length();
        let eps = s * 1e-3;
        if (s - y).abs() > eps || (s - z).abs() > eps
            || axis.x_axis.dot(axis.y_axis).abs() > s * eps
            || axis.x_axis.dot(axis.z_axis).abs() > s * eps
            || axis.y_axis.dot(axis.z_axis).abs() > s * eps {
            return None;
        }
        Some(Self { axis, origin, scale: s })
    }
    fn point(self, p: [f32; 4]) -> Vec3 { self.axis * Vec3::new(p[0], p[1], p[2]) + self.origin }
    fn direction(self, p: [f32; 4]) -> Vec3 { self.axis * Vec3::new(p[0], p[1], p[2]) / self.scale }
    fn transfer(self, offset: u32, count: u32, phase: i32, reset: bool) -> TransferUniform {
        TransferUniform {
            new_axis: axis4(self.axis), new_origin: vec4(self.origin, 0.0),
            inverse_axis: axis4(self.axis.inverse()), offset, count, phase, reset: u32::from(reset),
        }
    }
    pub(super) fn changed_scale(self, old: Self) -> bool {
        (self.scale - old.scale).abs() > self.scale * 1e-3
    }
}

#[derive(Clone, Copy)]
pub(super) struct Timing { pub(super) ticks: u32, pub(super) blend: f32, pub(super) reset: bool }
#[derive(Default)]
struct Clock { last_time: Option<i32>, hz: u32, accumulator: f64 }
impl Clock {
    fn advance(&mut self, frame: &ClothGpuFrame) -> Timing {
        let hz = frame.tuning.hz.clamp(15, 240);
        let h = 1.0 / f64::from(hz);
        let max_ticks = frame.tuning.max_ticks.clamp(1, MAX_TICKS as u32);
        let delta = self.last_time.map(|last| frame.time_ms.saturating_sub(last));
        let reset = self.hz != hz || !delta.is_some_and(|d| (0..=MAX_FRAME_GAP_MS).contains(&d));
        self.last_time = Some(frame.time_ms);
        self.hz = hz;
        if reset {
            self.accumulator = 0.0;
            return Timing { ticks: 0, blend: 1.0, reset };
        }
        self.accumulator += f64::from(delta.unwrap()) * 0.001;
        let mut ticks = 0;
        while self.accumulator + 1e-12 >= h && ticks < max_ticks {
            self.accumulator = (self.accumulator - h).max(0.0);
            ticks += 1;
        }
        if ticks == max_ticks && self.accumulator >= h { self.accumulator %= h; }
        Timing { ticks, blend: (self.accumulator / h).clamp(0.0, 1.0) as f32, reset }
    }
}

#[derive(Clone, Copy)]
#[repr(usize)]
enum Kernel {
    ClearHash, BuildHash, Predict, ClearDeltas, SolveDistance, SolveContact,
    ApplyDeltas, SolveGround, SolveSphere, SolveCapsule, SolveBox, UpdateVelocity,
    Pack, Snapshot, Scatter,
}
const KERNELS: [Kernel; 15] = [
    Kernel::ClearHash, Kernel::BuildHash, Kernel::Predict, Kernel::ClearDeltas,
    Kernel::SolveDistance, Kernel::SolveContact, Kernel::ApplyDeltas, Kernel::SolveGround,
    Kernel::SolveSphere, Kernel::SolveCapsule, Kernel::SolveBox, Kernel::UpdateVelocity,
    Kernel::Pack, Kernel::Snapshot, Kernel::Scatter,
];
impl Kernel {
    fn entry(self) -> &'static str {
        match self {
            Self::ClearHash => "clear_hash", Self::BuildHash => "build_hash",
            Self::Predict => "predict", Self::ClearDeltas => "clear_deltas",
            Self::SolveDistance => "solve_distance", Self::SolveContact => "solve_contact",
            Self::ApplyDeltas => "apply_deltas", Self::SolveGround => "solve_ground",
            Self::SolveSphere => "solve_sphere", Self::SolveCapsule => "solve_capsule",
            Self::SolveBox => "solve_box", Self::UpdateVelocity => "update_velocity",
            Self::Pack => "pack", Self::Snapshot => "snapshot", Self::Scatter => "scatter",
        }
    }
    fn bindings(self) -> &'static [u32] {
        match self {
            Self::ClearHash => &[3], Self::BuildHash => &[0, 3, 4],
            Self::Predict | Self::SolveGround | Self::UpdateVelocity => &[0],
            Self::ClearDeltas => &[2], Self::SolveDistance => &[0, 1, 2],
            Self::SolveContact => &[0, 2, 3, 4], Self::ApplyDeltas => &[0, 2],
            Self::SolveSphere | Self::SolveCapsule | Self::SolveBox => &[0, 5],
            Self::Pack => &[0, 6, 7, 10, 11, 12], Self::Snapshot => &[0, 6],
            Self::Scatter => &[0, 6, 7, 9, 11, 12],
        }
    }
    fn transfer(self) -> bool { matches!(self, Self::Pack | Self::Scatter) }
}
struct Pipelines {
    params_layout: wgpu::BindGroupLayout,
    transfer_layout: wgpu::BindGroupLayout,
    layouts: Vec<wgpu::BindGroupLayout>,
    pipelines: Vec<wgpu::ComputePipeline>,
}
fn uniform_layout(device: &wgpu::Device, size: u64) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("JKA Unified cloth uniform layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0, visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(size),
            }, count: None,
        }],
    })
}
fn build_pipelines(device: &wgpu::Device) -> Pipelines {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("JKA Unified Solver cloth"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../../cloth_unified.wgsl").into()),
    });
    let params_layout = uniform_layout(device, std::mem::size_of::<SolverParams>() as u64);
    let transfer_layout = uniform_layout(device, std::mem::size_of::<TransferUniform>() as u64);
    let mut layouts = Vec::with_capacity(KERNELS.len());
    let mut pipelines = Vec::with_capacity(KERNELS.len());
    for kernel in KERNELS {
        let entries = kernel.bindings().iter().map(|&binding| {
            crate::renderer::storage_buffer_entry(binding, false, wgpu::ShaderStages::COMPUTE)
        }).collect::<Vec<_>>();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(kernel.entry()), entries: &entries,
        });
        let mut groups = vec![Some(&layout), Some(&params_layout)];
        if kernel.transfer() { groups.push(Some(&transfer_layout)); }
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(kernel.entry()), bind_group_layouts: &groups, immediate_size: 0,
        });
        pipelines.push(device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(kernel.entry()), layout: Some(&pipeline_layout), module: &shader,
            entry_point: Some(kernel.entry()), compilation_options: Default::default(),
            cache: crate::pipeline_cache::get().as_ref(),
        }));
        layouts.push(layout);
    }
    Pipelines { params_layout, transfer_layout, layouts, pipelines }
}
fn bind_uniform(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, buffer: &wgpu::Buffer) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA Unified cloth uniform"), layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
    })
}
fn bind_storage(device: &wgpu::Device, pipelines: &Pipelines, buffers: &Buffers,
    kernel: Kernel, instance: Option<&InstanceGpu>) -> wgpu::BindGroup {
    let entries = kernel.bindings().iter().map(|&binding| {
        let buffer = match binding {
            0 => &buffers.particles, 1 => &buffers.constraints, 2 => &buffers.deltas,
            3 => &buffers.hash_head, 4 => &buffers.hash_next, 5 => &buffers.colliders,
            6 => &buffers.render_previous,
            7 => &instance.unwrap().positions,
            9 => &instance.unwrap().prev_positions, 10 => &instance.unwrap().skinned_targets,
            11 => &instance.unwrap().unified_state.as_ref().unwrap().particles,
            12 => &instance.unwrap().unified_state.as_ref().unwrap().previous,
            _ => unreachable!(),
        };
        wgpu::BindGroupEntry { binding, resource: buffer.as_entire_binding() }
    }).collect::<Vec<_>>();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(kernel.entry()), layout: &pipelines.layouts[kernel as usize], entries: &entries,
    })
}

struct Buffers {
    capacity: [u32; 3],
    particles: wgpu::Buffer, constraints: wgpu::Buffer, deltas: wgpu::Buffer,
    hash_head: wgpu::Buffer, hash_next: wgpu::Buffer, colliders: wgpu::Buffer,
    render_previous: wgpu::Buffer, params: wgpu::Buffer, params_group: wgpu::BindGroup,
    groups: Vec<Option<wgpu::BindGroup>>,
}
impl Buffers {
    fn new(device: &wgpu::Device, pipelines: &Pipelines, capacity: [u32; 3]) -> Self {
        let [p, c, k] = capacity.map(u64::from);
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA Unified cloth params"), size: std::mem::size_of::<SolverParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
        });
        let params_group = bind_uniform(device, &pipelines.params_layout, &params);
        let mut out = Self {
            capacity,
            particles: storage_zeroed(device, "Unified particles", p * 48),
            constraints: storage_zeroed(device, "Unified distance constraints", c * 24),
            deltas: storage_zeroed(device, "Unified Jacobi delta/count", p * 16),
            hash_head: storage_zeroed(device, "Unified hash heads", u64::from(TABLE_SIZE) * 4),
            hash_next: storage_zeroed(device, "Unified hash links", p * 4),
            colliders: storage_zeroed(device, "Unified world colliders", k * 80),
            render_previous: storage_zeroed(device, "Unified interpolation", p * 16),
            params, params_group, groups: Vec::new(),
        };
        out.groups = KERNELS.iter().map(|&k| (!k.transfer()).then(|| bind_storage(device, pipelines, &out, k, None))).collect();
        out
    }
    fn fits(&self, requested: [u32; 3]) -> bool {
        self.capacity.iter().zip(requested).all(|(&a,b)| a >= b)
    }
}

struct TransferBindings { pack: wgpu::BindGroup, scatter: wgpu::BindGroup }
/// Per-instance GPU state and cached bridge groups. Global pool growth only
/// invalidates the bridge groups; the world-space particles survive.
pub(super) struct InstanceState {
    particles: wgpu::Buffer,
    previous: wgpu::Buffer,
    uniform_buffer: wgpu::Buffer,
    uniform: wgpu::BindGroup,
    bindings: Option<TransferBindings>,
}
impl InstanceState {
    fn new(device: &wgpu::Device, pipelines: &Pipelines, count: u32) -> Self {
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Unified cloth world transform"), size: std::mem::size_of::<TransferUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
        });
        let uniform = bind_uniform(device, &pipelines.transfer_layout, &uniform_buffer);
        Self {
            particles: storage_zeroed(device, "Unified persistent garment particles", u64::from(count.max(1))*48),
            previous: storage_zeroed(device, "Unified persistent garment interpolation", u64::from(count.max(1))*16),
            uniform_buffer, uniform, bindings: None,
        }
    }
}
struct TransferJob { key: (u16, Arc<str>), count: u32 }
struct Frame {
    transfers: Vec<TransferJob>, params: SolverParams, timing: Timing, substeps: u32, particle_collision: bool,
}
#[derive(PartialEq)]
struct ConstraintSignature {
    garments: Vec<((u16, Arc<str>), u32, f32)>, compliance: f32, damping: f32,
}
#[derive(Default)]
pub(super) struct UnifiedGpu {
    pipelines: Option<Pipelines>, buffers: Option<Buffers>,
    frame: Option<Frame>, clock: Clock, signature: Option<ConstraintSignature>,
    reported_capacity: bool,
}
impl UnifiedGpu {
    pub(super) fn begin_frame(&mut self) { self.frame = None; }
    pub(super) fn reset_clock(&mut self) { self.clock = Clock::default(); self.frame = None; }
    pub(super) fn ready(&mut self, jobs: &mut PipelineJobManager, device: &wgpu::Device) -> bool {
        if self.pipelines.is_none() {
            let key = PipelineJobKey::new("unified-cloth", 0, 0);
            if let Some(pipelines) = jobs.take_ready::<Pipelines>(key) {
                self.pipelines = Some(pipelines);
            } else {
                let device = device.clone();
                jobs.request(key, "Unified Solver cloth pipelines", move || build_pipelines(&device));
            }
        }
        self.pipelines.is_some()
    }
    pub(super) fn timing(&mut self, frame: &ClothGpuFrame) -> Timing { self.clock.advance(frame) }

    /// Build one world particle range per visible garment, in deterministic
    /// entity/model order. Particle state persists in per-instance world-space
    /// buffers; the shared pool can grow/reorder without resetting survivors.
    pub(super) fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue,
        pending: &[PendingDraw], dispatches: &[Dispatch], instances: &mut HashMap<(u16, Arc<str>), InstanceGpu>,
        timing: Timing) -> bool {
        let Some(pipelines) = self.pipelines.as_ref() else { return false; };
        let mut active = dispatches.iter().filter(|d| d.unified).collect::<Vec<_>>();
        if active.is_empty() { self.reset_clock(); return true; }
        active.sort_unstable_by(|a,b| a.key.cmp(&b.key));
        let frames = active.iter().map(|dispatch| {
            let frame = pending.iter().find(|draw| {
                let f = &draw.surface.frame;
                f.entity_num == dispatch.key.0 && f.garment.key == dispatch.key.1
            }).map(|draw| draw.surface.frame.as_ref())?;
            let pose = Pose::from_frame(frame)?;
            Some((*dispatch, frame, pose))
        }).collect::<Option<Vec<_>>>();
        let Some(frames) = frames else { return false; };
        let first = frames[0].1;
        let tuning = first.tuning.unified;
        // A single XPBD world has one fixed timestep/material parameter set.
        if frames.iter().any(|(_, f, _)| f.tuning != first.tuning || f.time_ms != first.time_ms) {
            return false;
        }
        let mut p_count = 0u32;
        let mut c_count = 0u32;
        let mut sig = ConstraintSignature { garments: Vec::with_capacity(frames.len()), compliance: tuning.compliance, damping: tuning.constraint_damping };
        for (d, f, pose) in &frames {
            sig.garments.push((d.key.clone(), f.garment.particle_count() as u32, pose.scale));
            let Some(p) = p_count.checked_add(f.garment.particle_count() as u32) else { return false; };
            let Some(c) = c_count.checked_add(f.garment.constraints.len() as u32) else { return false; };
            p_count = p; c_count = c;
        }
        // Reuse the existing body's primitive fit. Transform every primitive
        // into world metres, once per entity (not once per cloth draw).
        let mut spheres = Vec::new();
        let mut capsules = Vec::new();
        let mut boxes = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (_, f, pose) in &frames {
            if f.colliders.is_empty() || !seen.insert(f.entity_num) { continue; }
            for c in &f.colliders {
                if c[1][3] < 0.5 {
                    let a = pose.point(c[0]);
                    let b = pose.point(c[1]);
                    let item = Collider { a: vec4(a, c[0][3] * pose.scale), b: vec4(b, 0.0), ..Default::default() };
                    if a == b { spheres.push(item); } else { capsules.push(item); }
                } else {
                    let x = pose.direction(c[2]);
                    let y = pose.direction(c[3]);
                    // Same third-axis construction as the game's existing OBB packing.
                    let z = x.cross(y);
                    boxes.push(Collider {
                        a: vec4(pose.point(c[0]), 0.0),
                        b: vec4(Vec3::new(c[1][0], c[1][1], c[1][2]) * pose.scale, 0.0),
                        x: vec4(x, 0.0), y: vec4(y, 0.0), z: vec4(z, 0.0),
                    });
                }
            }
        }
        let sphere_count = spheres.len() as u32;
        let capsule_count = capsules.len() as u32;
        let box_count = boxes.len() as u32;
        spheres.extend(capsules);
        spheres.extend(boxes);
        let k_count = spheres.len() as u32;
        let limits = device.limits();
        let limit = u64::from(limits.max_storage_buffer_binding_size).min(limits.max_buffer_size);
        let requested = [p_count.max(1), c_count.max(1), k_count.max(1)];
        let strides = [48u64, 24, 80];
        if requested.iter().zip(strides).any(|(&n,s)| u64::from(n)*s > limit)
            || u64::from(TABLE_SIZE)*4 > limit
            || [p_count, c_count, TABLE_SIZE].into_iter().any(|n| n.div_ceil(THREADS) > limits.max_compute_workgroups_per_dimension)
            || p_count > i32::MAX as u32 {
            if !self.reported_capacity {
                log::warn!("Unified cloth pool exceeds WGPU storage/dispatch limits; drawing skinned pose instead of truncating particles");
                self.reported_capacity = true;
            }
            return false;
        }
        if self.buffers.as_ref().is_none_or(|b| !b.fits(requested)) {
            let capacity = std::array::from_fn(|i| {
                let old = self.buffers.as_ref().map_or(1, |b| b.capacity[i]);
                let n = requested[i].max(old);
                let grown = n.checked_next_power_of_two().unwrap_or(n);
                if u64::from(grown) * strides[i] <= limit { grown } else { n }
            });
            // Drop cached old pool bindings before replacing the large buffers.
            for instance in instances.values_mut() {
                if let Some(state) = &mut instance.unified_state { state.bindings = None; }
            }
            self.buffers = Some(Buffers::new(device, pipelines, capacity));
            self.signature = None;
        }
        let buffers = self.buffers.as_ref().unwrap();
        if self.signature.as_ref() != Some(&sig) {
            let mut constraints = Vec::with_capacity(c_count as usize);
            let mut offset = 0;
            for (_, f, pose) in &frames {
                constraints.extend(f.garment.constraints.iter().map(|&(a,b,rest)| Constraint {
                    a: offset + a, b: offset + b, rest_length: rest * pose.scale,
                    compliance: tuning.compliance, break_force: 0.0, damping: tuning.constraint_damping,
                }));
                offset += f.garment.particle_count() as u32;
            }
            if !constraints.is_empty() { queue.write_buffer(&buffers.constraints, 0, bytemuck::cast_slice(&constraints)); }
            self.signature = Some(sig);
        }
        if !spheres.is_empty() { queue.write_buffer(&buffers.colliders, 0, bytemuck::cast_slice(&spheres)); }
        let substeps = tuning.substeps.clamp(1, 128);
        let params = SolverParams {
            particle_count: p_count, constraint_count: c_count, table_size: TABLE_SIZE,
            sphere_count, capsule_count, box_count,
            dt: (1.0 / first.tuning.hz.clamp(15, 240) as f32) / substeps as f32,
            sor: tuning.sor,
            // Same external-acceleration slot as upstream gravity; JKA is Z-up.
            // The existing weather input is already in world metres/s^2.
            gravity: (Vec3::new(0.0, 0.0, -9.81) + Vec3::from_array(first.wind)).to_array(),
            damping: first.tuning.damping,
            radius: tuning.particle_radius, cell_size: 2.0 * tuning.particle_radius,
            friction_static: tuning.friction_static,
            friction_kinetic: tuning.friction_kinetic,
            max_depenetration_speed: tuning.max_depenetration_speed,
            ground_z: 0.0, rigid_count: 0, pad: 0.0,
        };
        queue.write_buffer(&buffers.params, 0, bytemuck::bytes_of(&params));
        let mut transfers = Vec::with_capacity(frames.len());
        let mut offset = 0;
        for (index, (d, f, pose)) in frames.into_iter().enumerate() {
            let instance = instances.get_mut(&d.key).expect("scheduled instance");
            let count = f.garment.particle_count() as u32;
            let phase = if tuning.self_collision { 0 } else { (index + 1) as i32 };
            if instance.unified_state.is_none() {
                instance.unified_state = Some(InstanceState::new(device, pipelines, count));
            }
            let transfer = pose.transfer(offset, count, phase, d.warm_start);
            if instance.unified_state.as_ref().unwrap().bindings.is_none() {
                let pack = bind_storage(device, pipelines, buffers, Kernel::Pack, Some(instance));
                let scatter = bind_storage(device, pipelines, buffers, Kernel::Scatter, Some(instance));
                instance.unified_state.as_mut().unwrap().bindings = Some(TransferBindings { pack, scatter });
            }
            let state = instance.unified_state.as_ref().unwrap();
            queue.write_buffer(&state.uniform_buffer, 0, bytemuck::bytes_of(&transfer));
            transfers.push(TransferJob { key: d.key.clone(), count });
            instance.unified_pose = Some(pose);
            offset += count;
        }
        self.frame = Some(Frame { transfers, params, timing, substeps, particle_collision: tuning.particle_collision });
        true
    }
}

struct Context<'a> { pipelines: &'a Pipelines, buffers: &'a Buffers }
impl Context<'_> {
    fn dispatch(&self, pass: &mut wgpu::ComputePass<'_>, kernel: Kernel, count: u32) {
        if count == 0 { return; }
        pass.set_pipeline(&self.pipelines.pipelines[kernel as usize]);
        pass.set_bind_group(0, self.buffers.groups[kernel as usize].as_ref().unwrap(), &[]);
        pass.set_bind_group(1, &self.buffers.params_group, &[]);
        pass.dispatch_workgroups(count.div_ceil(THREADS), 1, 1);
    }
    fn transfer(&self, pass: &mut wgpu::ComputePass<'_>, kernel: Kernel, job: &TransferJob, instance: &InstanceGpu) {
        if job.count == 0 { return; }
        pass.set_pipeline(&self.pipelines.pipelines[kernel as usize]);
        let state = instance.unified_state.as_ref().unwrap();
        let bindings = state.bindings.as_ref().unwrap();
        let group = if matches!(kernel, Kernel::Pack) { &bindings.pack } else { &bindings.scatter };
        pass.set_bind_group(0, group, &[]);
        pass.set_bind_group(1, &self.buffers.params_group, &[]);
        pass.set_bind_group(2, &state.uniform, &[]);
        pass.dispatch_workgroups(job.count.div_ceil(THREADS), 1, 1);
    }
    /// Exact cloth dispatch sequence of SolverManager.FixedUpdate (hash built
    /// once before all substeps). Each dispatch boundary is global, not a
    /// workgroup-local barrier. No Gauss-Seidel or legacy collision code.
    fn tick(&self, pass: &mut wgpu::ComputePass<'_>, f: &Frame) {
        let p = f.params.particle_count;
        if f.particle_collision {
            self.dispatch(pass, Kernel::ClearHash, f.params.table_size);
            self.dispatch(pass, Kernel::BuildHash, p);
        }
        for _ in 0..f.substeps {
            self.dispatch(pass, Kernel::Predict, p);
            self.dispatch(pass, Kernel::ClearDeltas, p);
            self.dispatch(pass, Kernel::SolveDistance, f.params.constraint_count);
            if f.particle_collision { self.dispatch(pass, Kernel::SolveContact, p); }
            self.dispatch(pass, Kernel::ApplyDeltas, p);
            // SolveGround is a Unity demo plane, not a BSP collision model.
            if f.params.sphere_count > 0 { self.dispatch(pass, Kernel::SolveSphere, p); }
            if f.params.capsule_count > 0 { self.dispatch(pass, Kernel::SolveCapsule, p); }
            if f.params.box_count > 0 { self.dispatch(pass, Kernel::SolveBox, p); }
            // No rigid-particle bodies are registered by cloth. Rapier is not replaced.
            self.dispatch(pass, Kernel::UpdateVelocity, p);
        }
    }
}

impl ClothGpuRenderer {
    pub(super) fn encode_unified(&self, encoder: &mut wgpu::CommandEncoder) {
        let (Some(frame), Some(pipelines), Some(buffers), Some(skin_pipelines)) = (
            &self.unified.frame, &self.unified.pipelines, &self.unified.buffers, &self.pipelines,
        ) else { return; };
        let context = Context { pipelines, buffers };
        // Every garment's skin/anchors must be ready before any world solve.
        // The same dynamic model buffer is consumed by raster, depth, shadow, RT.
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("JKA Unified cloth world solve"), timestamp_writes: None,
        });
        for job in &frame.transfers {
            let instance = &self.instances[&job.key];
            let garment = &self.garments[&job.key.1].garment;
            let dispatch = self.dispatches.iter().find(|d| d.key == job.key).unwrap();
            let skin = EncodeContext { pipelines: skin_pipelines, instance, param_stride: instance.params.size() / PARAM_SLOTS as u64 };
            skin.bind(&mut pass, Pass::Skin, 0);
            pass.dispatch_workgroups(groups_of(garment.render_vertices.len() as u32), 1, 1);
            if dispatch.drivers {
                skin.bind(&mut pass, Pass::Drivers, 0);
                pass.dispatch_workgroups(groups_of(garment.driver_particles.len() as u32), 1, 1);
            }
            skin.bind(&mut pass, Pass::Gather, 0);
            pass.dispatch_workgroups(groups_of(job.count), 1, 1);
            if dispatch.warm_start {
                skin.bind(&mut pass, Pass::WarmStart, 0);
                pass.dispatch_workgroups(groups_of(job.count), 1, 1);
            }
            context.transfer(&mut pass, Kernel::Pack, job, instance);
        }
        for tick in 0..frame.timing.ticks {
            if tick + 1 == frame.timing.ticks {
                context.dispatch(&mut pass, Kernel::Snapshot, frame.params.particle_count);
            }
            context.tick(&mut pass, frame);
        }
        for job in &frame.transfers {
            let instance = &self.instances[&job.key];
            context.transfer(&mut pass, Kernel::Scatter, job, instance);
            let garment = &self.garments[&job.key.1].garment;
            let dispatch = self.dispatches.iter().find(|d| d.key == job.key).unwrap();
            let skin = EncodeContext { pipelines: skin_pipelines, instance, param_stride: instance.params.size() / PARAM_SLOTS as u64 };
            // Local frame/pin position can change even on a render-only frame.
            // Recompute normals in the new local frame before output.
            skin.bind(&mut pass, Pass::FaceNormals, 0);
            pass.dispatch_workgroups(groups_of(garment.triangles.len() as u32), 1, 1);
            skin.bind(&mut pass, Pass::VertexNormals, 0);
            pass.dispatch_workgroups(groups_of(job.count), 1, 1);
            if let Some((_, output_group)) = &instance.output_bind_group {
                let stride = instance.draws.size() / MAX_DRAW_SLOTS as u64;
                pass.set_pipeline(&skin_pipelines.pipelines[pass_index(Pass::Output)]);
                pass.set_bind_group(0, output_group, &[]);
                pass.set_bind_group(1, &instance.params_bind_group, &[0]);
                for &(slot, count) in &dispatch.outputs {
                    pass.set_bind_group(2, &instance.draw_bind_group, &[(u64::from(slot) * stride) as u32]);
                    pass.dispatch_workgroups(groups_of(count), 1, 1);
                }
            }
        }
    }
}
