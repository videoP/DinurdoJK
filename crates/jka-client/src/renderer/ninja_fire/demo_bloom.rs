//! Fire Smoke's standard Gaussian bloom at the demo's PostProcessQuality=3.
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt as _;

const LEVELS: usize = 6;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const KERNEL_PERCENT: [f32; LEVELS] = [0.3, 1.0, 2.0, 10.0, 30.0, 64.0];
const TINT: [f32; LEVELS] = [0.3465, 0.138, 0.1176, 0.066, 0.066, 0.061];

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Filter {
    axis_tint: [f32; 4],
    taps: [[f32; 4]; 32],
}

struct Pass {
    pipeline: wgpu::RenderPipeline,
    bindings: wgpu::BindGroup,
    output: wgpu::TextureView,
}

pub(super) struct DemoBloom {
    passes: Vec<Pass>,
}

impl DemoBloom {
    pub(super) fn new(
        device: &wgpu::Device,
        scene: &wgpu::TextureView,
        output: &wgpu::TextureView,
        (width, height): (u32, u32),
    ) -> Result<Self, String> {
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Fire Smoke Gaussian bloom"),
            source: wgpu::ShaderSource::Wgsl(include_str!("demo_bloom.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Fire Smoke Gaussian bloom"),
            entries: &[
                image_layout(0),
                image_layout(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Fire Smoke Gaussian bloom"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(entry),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader, entry_point: Some("fullscreen"),
                buffers: &[], compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader, entry_point: Some(entry), compilation_options: Default::default(),
                targets: &[Some(FORMAT.into())],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let half_pipeline = pipeline("half_res");
        let extract_pipeline = pipeline("extract");
        let down_pipeline = pipeline("downsample");
        let blur_pipeline = pipeline("gaussian");
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Fire Smoke bloom bilinear clamp"),
            min_filter: wgpu::FilterMode::Linear,
            mag_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let size: [[u32; 2]; LEVELS] = std::array::from_fn(|i| {
            let divisor = 1 << (i + 1);
            [width.div_ceil(divisor).max(1), height.div_ceil(divisor).max(1)]
        });
        let half = texture(device, size[0]);
        let down = size.map(|size| texture(device, size));
        let horizontal = size.map(|size| texture(device, size));
        let accumulated: [_; LEVELS] = std::array::from_fn(|i| {
            if i == 0 { output.clone() } else { texture(device, size[i]) }
        });
        let mut passes = Vec::with_capacity(1 + LEVELS * 3);
        let mut pass = |pipeline: &wgpu::RenderPipeline, source, additive, dst: &wgpu::TextureView, filter: Filter| {
            let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Fire Smoke bloom weights"),
                contents: bytemuck::bytes_of(&filter),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Fire Smoke bloom pass"),
                layout: &layout,
                entries: &[
                    texture_entry(0, source),
                    texture_entry(1, additive),
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
                    wgpu::BindGroupEntry { binding: 3, resource: uniform.as_entire_binding() },
                ],
            });
            passes.push(Pass { pipeline: pipeline.clone(), bindings, output: dst.clone() });
        };
        pass(&half_pipeline, scene, scene, &half, Filter::zeroed());
        pass(&extract_pipeline, &half, &half, &down[0], Filter::zeroed());
        for i in 1..LEVELS {
            pass(&down_pipeline, &down[i - 1], &down[i - 1], &down[i], Filter::zeroed());
        }
        for i in (0..LEVELS).rev() {
            let [width, height] = size[i].map(|n| n as f32);
            let radius = width * KERNEL_PERCENT[i] * 0.005;
            let mut filter = Filter {
                axis_tint: [width.recip(), 0.0, 0.0, 0.0],
                taps: gaussian_taps(radius, 1.0),
            };
            pass(&blur_pipeline, &down[i], &down[i], &horizontal[i], filter);
            let additive = accumulated.get(i + 1);
            filter.axis_tint = [0.0, height.recip(), 0.0, f32::from(u8::from(additive.is_some()))];
            filter.taps = gaussian_taps(radius, TINT[i] * (10.0 / LEVELS as f32));
            pass(&blur_pipeline, &horizontal[i], additive.unwrap_or(&horizontal[i]), &accumulated[i], filter);
        }
        super::resolve_scopes(memory, validation)?;
        Ok(Self { passes })
    }

    pub(super) fn encode(&self, encoder: &mut wgpu::CommandEncoder) {
        for stage in &self.passes {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Fire Smoke Gaussian bloom"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &stage.output,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&stage.pipeline);
            pass.set_bind_group(0, &stage.bindings, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

/// PostProcessWeightedSampleSum's 32-tap bilinear kernel, default size scale
/// and zero cross-center weight. The vertical pass applies the source tint.
fn gaussian_taps(radius: f32, tint: f32) -> [[f32; 4]; 32] {
    let radius = radius.clamp(0.00001, 31.0);
    let support = radius.ceil() as i32;
    let weight = |sample: i32| (-16.7 * (sample as f32 / radius).powi(2)).exp();
    let mut taps = [[0.0; 4]; 32];
    let mut sum = 0.0;
    let mut index = 0;
    for sample in (-support..=support).step_by(2) {
        let a = weight(sample);
        let b = if sample == support { 0.0 } else { weight(sample + 1) };
        let w = a + b;
        // Zero-weight pairs underflow at tiny radii and make no contribution.
        if w > 0.0 {
            taps[index] = [sample as f32 + b / w, w, 0.0, 0.0];
            sum += w;
            index += 1;
        }
    }
    for tap in &mut taps[..index] {
        tap[1] *= tint / sum;
    }
    taps
}

pub(super) fn texture(device: &wgpu::Device, [width, height]: [u32; 2]) -> wgpu::TextureView {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Fire Smoke bloom level"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    }).create_view(&Default::default())
}

pub(super) fn image_layout(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
        count: None,
    }
}

pub(super) fn texture_entry(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::TextureView(view) }
}
