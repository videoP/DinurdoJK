//! Optional compute posing into the same private world-vertex regions as the CPU path.
use crate::renderer::{GpuPass, GpuVertex, InlineModelGpu, InlineModelInstance, Renderer};
use crate::ui::InlineTransformMode;
use wgpu::util::DeviceExt;

const SHADER: &str = include_str!("inline_transform.wgsl");

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuPose {
    axis: [[f32; 4]; 3],
    origin: [f32; 4],
    params: [u32; 4],
}

pub(in crate::renderer) struct InlineTransformModel {
    bind_group: wgpu::BindGroup,
    pose_buffer: wgpu::Buffer,
    bounds: ([f32; 3], [f32; 3]),
    prefix_words: u32,
    count: u32,
    pending: bool,
}

pub(in crate::renderer) struct InlineTransformGpu {
    pipeline: wgpu::ComputePipeline,
    pub(in crate::renderer) models: Vec<Option<InlineTransformModel>>,
}

impl InlineTransformGpu {
    pub(in crate::renderer) fn new(
        device: &wgpu::Device,
        vertices: &wgpu::Buffer,
        models: &[InlineModelGpu],
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Inline mover compute transform"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Inline mover compute transform"),
            layout: None,
            module: &shader,
            entry_point: Some("transform"),
            compilation_options: Default::default(),
            cache: None,
        });
        let layout = pipeline.get_bind_group_layout(0);
        let models = models
            .iter()
            .map(|model| InlineTransformModel::new(device, &layout, vertices, model))
            .collect();
        Self { pipeline, models }
    }
}

impl InlineTransformModel {
    fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        vertices: &wgpu::Buffer,
        model: &InlineModelGpu,
    ) -> Option<Self> {
        let stride = std::mem::size_of::<GpuVertex>() as u64;
        let offset = u64::from(model.gpu_vertex_start) * stride;
        let alignment = u64::from(device.limits().min_storage_buffer_offset_alignment);
        let aligned = offset / alignment * alignment;
        let prefix = offset - aligned;
        let bytes = model.base.len() as u64 * stride;
        // Bind only this mover's region, avoiding the storage-binding limit on
        // large world buffers. Oversized/empty movers retain the CPU fallback.
        if bytes == 0
            || prefix + bytes > u64::from(device.limits().max_storage_buffer_binding_size)
            || (model.base.len() as u64).div_ceil(64)
                > u64::from(device.limits().max_compute_workgroups_per_dimension)
        {
            return None;
        }
        let base: Vec<[f32; 6]> = model
            .base
            .iter()
            .map(|v| {
                [
                    v.position[0],
                    v.position[1],
                    v.position[2],
                    v.normal[0],
                    v.normal[1],
                    v.normal[2],
                ]
            })
            .collect();
        let base_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Inline mover immutable positions and normals"),
            contents: bytemuck::cast_slice(&base),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let pose_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Inline mover pose"),
            size: std::mem::size_of::<GpuPose>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Inline mover compute transform"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: base_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: vertices,
                        offset: aligned,
                        size: wgpu::BufferSize::new(prefix + bytes),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: pose_buffer.as_entire_binding(),
                },
            ],
        });
        let mut minimum = [f32::INFINITY; 3];
        let mut maximum = [f32::NEG_INFINITY; 3];
        for vertex in &model.base {
            for axis in 0..3 {
                minimum[axis] = minimum[axis].min(vertex.position[axis]);
                maximum[axis] = maximum[axis].max(vertex.position[axis]);
            }
        }
        Some(Self {
            bind_group,
            pose_buffer,
            bounds: (minimum, maximum),
            prefix_words: (prefix / 4) as u32,
            count: model.base.len() as u32,
            pending: false,
        })
    }

    pub(in crate::renderer) fn update(
        &mut self,
        queue: &wgpu::Queue,
        pose: InlineModelInstance,
    ) -> ([f32; 3], [f32; 3]) {
        let packed = GpuPose {
            axis: pose.axis.map(|v| [v[0], v[1], v[2], 0.0]),
            origin: [pose.origin[0], pose.origin[1], pose.origin[2], 0.0],
            params: [
                self.count,
                self.prefix_words,
                (std::mem::size_of::<GpuVertex>() / 4) as u32,
                (std::mem::offset_of!(GpuVertex, normal) / 4) as u32,
            ],
        };
        queue.write_buffer(&self.pose_buffer, 0, bytemuck::bytes_of(&packed));
        self.pending = true;
        posed_bounds(self.bounds, pose)
    }

    pub(in crate::renderer) fn cancel(&mut self) {
        self.pending = false;
    }
}

/// Transform the eight corners, not every vertex. Conservative for rotation
/// and editor scale; CPU visibility/light selection never rejects real geometry.
fn posed_bounds(bounds: ([f32; 3], [f32; 3]), pose: InlineModelInstance) -> ([f32; 3], [f32; 3]) {
    let mut minimum = [f32::INFINITY; 3];
    let mut maximum = [f32::NEG_INFINITY; 3];
    for corner in 0..8 {
        let p = pose.transform_point(std::array::from_fn(|axis| {
            if corner & (1 << axis) == 0 {
                bounds.0[axis]
            } else {
                bounds.1[axis]
            }
        }));
        for axis in 0..3 {
            // Cover CPU/GPU rounding differences at culling boundaries.
            let margin = 1.0e-5 * p[axis].abs().max(1.0);
            minimum[axis] = minimum[axis].min(p[axis] - margin);
            maximum[axis] = maximum[axis].max(p[axis] + margin);
        }
    }
    (minimum, maximum)
}

impl Renderer {
    pub(in crate::renderer) fn set_inline_transform_mode(&mut self, mode: InlineTransformMode) {
        if self.inline_transform_mode == mode {
            return;
        }
        if mode == InlineTransformMode::CpuWorkers && self.inline_transform_workers.is_none() {
            let threads = std::thread::available_parallelism()
                .map_or(1, usize::from)
                .saturating_sub(1)
                .clamp(1, 4);
            match rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .thread_name(|index| format!("inline-pose-{index}"))
                .build()
            {
                Ok(pool) => self.inline_transform_workers = Some(pool),
                Err(error) => eprintln!(
                    "Inline mover workers unavailable; using serial CPU transforms: {error}"
                ),
            }
        }
        self.inline_transform_mode = mode;
        if let Some(world) = self.world.as_mut() {
            // Force an authoritative rewrite even for stationary movers when
            // switching modes, including pending work from an unsubmitted frame.
            for model in &mut world.inline_models {
                if matches!(model.uploaded, Some(Some(_))) {
                    model.uploaded = None;
                }
            }
            if let Some(gpu) = world.inline_transform.as_mut() {
                for model in gpu.models.iter_mut().flatten() {
                    model.cancel();
                }
            }
        }
    }

    pub(in crate::renderer) fn encode_inline_transforms(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        let Some(gpu) = self
            .world
            .as_mut()
            .and_then(|w| w.inline_transform.as_mut())
        else {
            return;
        };
        if self.inline_transform_mode != InlineTransformMode::Gpu
            || !gpu.models.iter().flatten().any(|m| m.pending)
        {
            return;
        }
        self.gpu_profiler
            .write_encoder_timestamp(encoder, GpuPass::InlineTransform, false);
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Inline mover transforms"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&gpu.pipeline);
            for model in gpu.models.iter_mut().flatten().filter(|m| m.pending) {
                pass.set_bind_group(0, &model.bind_group, &[]);
                pass.dispatch_workgroups(model.count.div_ceil(64), 1, 1);
                model.pending = false;
            }
        }
        self.gpu_profiler
            .write_encoder_timestamp(encoder, GpuPass::InlineTransform, true);
    }

    /// A failed frame may have dropped its encoder before submission. Retry
    /// active poses even when the following snapshot has identical transforms.
    pub(in crate::renderer) fn retry_inline_transforms(&mut self) {
        if self.inline_transform_mode != InlineTransformMode::Gpu {
            return;
        }
        if let Some(world) = self.world.as_mut() {
            if let Some(gpu) = world.inline_transform.as_mut() {
                for (model, resource) in world.inline_models.iter().zip(&mut gpu.models) {
                    if let Some(resource) = resource {
                        resource.pending = matches!(model.uploaded, Some(Some(_)));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "inline_transform_tests.rs"]
mod tests;
