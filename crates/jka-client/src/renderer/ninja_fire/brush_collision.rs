//! BSP provider for the collision alpha of Fluid Ninja's MeshField capture.
//! UE reads the scene distance field on the GPU (Ninja_SDF_ReadAndPaint); JKA
//! has no distance field, so the solid brushes stand in for it and the
//! per-texel work runs in a compute pass (`brush_capture.wgsl`). The CPU only
//! picks the brushes near the field, when the field moves. `pack_brushes`
//! and `BrushBuffer` are shared with the particle pools (`embers.rs`).
use std::mem::size_of;

use glam::Vec3;
use jka_ninja::{FLAME_CAPTURE_DOWNSCALE, FlameBrushMask, FlameDesc};
use jka_river::river_ninja::checked_scope;

use crate::ninja_fire_debug::WorldBrush;

/// GPU brush list: per brush two headers (min.xyz + first plane index bits,
/// max.xyz + plane count bits), then planes (normal, distance).
pub(super) fn pack_brushes<'a>(
    brushes: impl IntoIterator<Item = &'a WorldBrush>,
    bounds: impl Fn(&WorldBrush) -> (Vec3, Vec3),
    plane: impl Fn(Vec3, f32) -> [f32; 4],
) -> (Vec<[f32; 4]>, u32) {
    let brushes: Vec<&WorldBrush> = brushes.into_iter().collect();
    let headers = 2 * brushes.len();
    let mut packed = Vec::with_capacity(headers + brushes.iter().map(|brush| brush.planes.len()).sum::<usize>());
    packed.resize(headers, [0.0; 4]);
    for (index, brush) in brushes.iter().enumerate() {
        let first = packed.len() as u32;
        packed.extend(brush.planes.iter().map(|&(normal, distance)| plane(normal, distance)));
        let (low, high) = bounds(brush);
        packed[2 * index] = [low.x, low.y, low.z, f32::from_bits(first)];
        packed[2 * index + 1] = [high.x, high.y, high.z, f32::from_bits(brush.planes.len() as u32)];
    }
    (packed, brushes.len() as u32)
}

/// A storage buffer for packed brushes, grown on demand.
pub(super) struct BrushBuffer {
    pub(super) buffer: wgpu::Buffer,
    capacity: u64,
}

impl BrushBuffer {
    pub(super) fn new(device: &wgpu::Device, label: &'static str) -> Self {
        Self { buffer: Self::create(device, label, 16), capacity: 16 }
    }

    fn create(device: &wgpu::Device, label: &'static str, size: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Returns true when the buffer was replaced (bind groups must be rebuilt).
    pub(super) fn write(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue, label: &'static str, packed: &[[f32; 4]],
    ) -> bool {
        let bytes = (packed.len() * size_of::<[f32; 4]>()) as u64;
        let grown = bytes > self.capacity;
        if grown {
            self.capacity = bytes.next_power_of_two();
            self.buffer = Self::create(device, label, self.capacity);
        }
        if !packed.is_empty() {
            queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(packed));
        }
        grown
    }
}

/// The capture compute pipeline, shared by every flame field.
pub(super) struct CapturePipeline {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
}

impl CapturePipeline {
    pub(super) fn new(device: &wgpu::Device) -> Result<Self, String> {
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let c = wgpu::ShaderStages::COMPUTE;
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: c, ty, count: None };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Ninja flame brush capture"),
            entries: &[
                entry(0, wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None,
                }),
                entry(1, wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false,
                    min_binding_size: None,
                }),
                entry(2, wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format: wgpu::TextureFormat::Rgba16Float,
                    view_dimension: wgpu::TextureViewDimension::D2,
                }),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Ninja flame brush capture"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Ninja flame brush capture"),
            source: wgpu::ShaderSource::Wgsl(include_str!("brush_capture.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Ninja flame brush capture"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("capture_main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let memory = checked_scope(memory);
        let validation = checked_scope(validation);
        validation.and(memory)?;
        Ok(Self { layout, pipeline })
    }
}

struct CaptureGpu {
    size: [u32; 2],
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    params: wgpu::Buffer,
    brushes: BrushBuffer,
    group: wgpu::BindGroup,
}

impl CaptureGpu {
    fn new(device: &wgpu::Device, pipeline: &CapturePipeline, size: [u32; 2]) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Ninja flame brush capture"),
            size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Ninja flame brush capture"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let brushes = BrushBuffer::new(device, "Ninja flame capture brushes");
        let group = Self::group(device, pipeline, &params, &brushes.buffer, &view);
        Self { size, texture, view, params, brushes, group }
    }

    fn group(
        device: &wgpu::Device, pipeline: &CapturePipeline, params: &wgpu::Buffer, brushes: &wgpu::Buffer,
        view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Ninja flame brush capture"),
            layout: &pipeline.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: brushes.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(view) },
            ],
        })
    }
}

/// GPU encode context for a capture.
pub(super) struct CaptureEncode<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub pipeline: &'a CapturePipeline,
}

#[derive(Default)]
pub(super) struct BrushCapture {
    previous_height: Option<f32>,
    frame: Option<[Vec3; 3]>,
    /// Brushes near the field (indices into the map list), with the map key
    /// and the centre they were chosen around.
    candidates: Vec<usize>,
    chosen: Option<(usize, Vec3)>,
    gpu: Option<CaptureGpu>,
    revision: u64,
    has_collision: bool,
}

impl BrushCapture {
    pub(super) fn sample(
        &mut self,
        gpu: CaptureEncode<'_>,
        brushes: Option<&[WorldBrush]>,
        centre: Vec3,
        right: Vec3,
        up: Vec3,
        desc: &FlameDesc,
    ) -> Option<FlameBrushMask<'_>> {
        // Ninja_SDF_ReadAndPaint Op_34 subtracts the vertical simulation motion
        // from the query position to align with the source's previous frame.
        let previous_height = self.previous_height.replace(centre.y);
        let brushes = brushes?;
        let centre = Vec3::new(centre.x, previous_height.unwrap_or(centre.y), centre.z);
        let frame = [centre, right, up];
        let size = desc.resolution.map(|n| (n / FLAME_CAPTURE_DOWNSCALE).max(1));
        if self.gpu.as_ref().is_none_or(|capture| capture.size != size) {
            self.gpu = Some(CaptureGpu::new(gpu.device, gpu.pipeline, size));
            self.frame = None;
        }
        if self.frame != Some(frame) {
            let [width, height, _] = desc.extent_cm;
            self.choose_candidates(gpu.device, gpu.queue, gpu.pipeline, brushes, centre, width.hypot(height) * 0.5);
            self.has_collision = self.candidates.iter()
                .any(|&index| touches_plane(&brushes[index], frame, width, height));
            let capture = self.gpu.as_ref().expect("capture GPU");
            if self.has_collision {
                let floats: [f32; 12] = [
                    centre.x, centre.y, centre.z, desc.painter_edge_mask_width,
                    right.x, right.y, right.z, width,
                    up.x, up.y, up.z, height,
                ];
                let mut bytes = Vec::with_capacity(64);
                bytes.extend_from_slice(bytemuck::cast_slice(&floats));
                bytes.extend_from_slice(bytemuck::cast_slice(&[size[0], size[1], self.candidates.len() as u32, 0]));
                gpu.queue.write_buffer(&capture.params, 0, &bytes);
                let mut pass = gpu.encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("Ninja flame brush capture"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&gpu.pipeline.pipeline);
                pass.set_bind_group(0, &capture.group, &[]);
                pass.dispatch_workgroups(size[0].div_ceil(8), size[1].div_ceil(8), 1);
            }
            self.frame = Some(frame);
            self.revision = self.revision.wrapping_add(1);
        }
        let capture = self.gpu.as_ref()?;
        self.has_collision.then_some(FlameBrushMask { texture: &capture.texture, revision: self.revision })
    }

    /// Re-pick the brushes within reach of the field when the map changes or
    /// the field moves out of the area chosen before. View rotation alone
    /// keeps the list.
    fn choose_candidates(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue, pipeline: &CapturePipeline,
        brushes: &[WorldBrush], centre: Vec3, radius: f32,
    ) {
        let key = brushes.as_ptr() as usize ^ brushes.len();
        if self.chosen.is_some_and(|(chosen, around)| chosen == key && around.distance(centre) <= radius * 0.5) {
            return;
        }
        let reach = Vec3::splat(radius * 1.5);
        let (min, max) = (centre - reach, centre + reach);
        self.candidates.clear();
        self.candidates.extend(brushes.iter().enumerate()
            .filter(|(_, brush)| brush.mins.cmple(max).all() && brush.maxs.cmpge(min).all())
            .map(|(index, _)| index));
        let (packed, _) = pack_brushes(
            self.candidates.iter().map(|&index| &brushes[index]),
            |brush| (brush.mins, brush.maxs),
            |normal, distance| [normal.x, normal.y, normal.z, distance],
        );
        let capture = self.gpu.as_mut().expect("capture GPU");
        if capture.brushes.write(device, queue, "Ninja flame capture brushes", &packed) {
            capture.group = CaptureGpu::group(device, pipeline, &capture.params, &capture.brushes.buffer, &capture.view);
        }
        self.chosen = Some((key, centre));
    }
}

/// The brush overlaps the field's box and crosses its plane.
fn touches_plane(brush: &WorldBrush, [centre, right, up]: [Vec3; 3], width: f32, height: f32) -> bool {
    let half = (right.abs() * width + up.abs() * height) * 0.5;
    if brush.mins.cmpgt(centre + half).any() || brush.maxs.cmplt(centre - half).any() {
        return false;
    }
    let normal = right.cross(up);
    let offset = (brush.maxs + brush.mins) * 0.5 - centre;
    let bounds_half = (brush.maxs - brush.mins) * 0.5;
    offset.dot(normal).abs() <= bounds_half.dot(normal.abs())
}
