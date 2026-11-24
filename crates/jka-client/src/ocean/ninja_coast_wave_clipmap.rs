use blue_water_gpu::shader_composer;
use jka_river::river_ninja::checked_scope;

use super::ninja_ocean::{b, buf, s, sampler, t, uniform};

const SIZE: u32 = 256;
const LEVELS: u32 = 4;
const EXTENT: f32 = 30.0;
const MIPS: u32 = SIZE.ilog2() + 1;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

pub(super) struct Pipeline {
    pub draw: wgpu::RenderPipeline,
    pub shore: wgpu::RenderPipeline,
    read: wgpu::BindGroupLayout,
    write: wgpu::BindGroupLayout,
    hybrid: wgpu::BindGroupLayout,
    mip: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    bake: wgpu::ComputePipeline,
    downsample: wgpu::ComputePipeline,
}

impl Pipeline {
    pub fn new(
        device: &wgpu::Device,
        surface_groups: &[wgpu::BindGroupLayout; 5],
        (format, samples, steps, ssr): (wgpu::TextureFormat, u32, u32, bool),
    ) -> Result<Self, String> {
        if device.limits().max_bind_groups < 6 {
            return Err("needs six render bind groups".into());
        }
        let material = jka_ninja::coast_material_wgsl().ok_or("Fluid Ninja private provider not included")?;
        let module = shader_composer::ninja_coast_clipmap_module(&material, jka_water::river::slw::SHADER, steps, ssr)?;
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Coast deep-water FFT clipmap"),
            source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(module)),
        });
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let compute = wgpu::ShaderStages::COMPUTE;
        let sampled = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        };
        let layout = |label, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(label), entries })
        };
        let output = wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: compute,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: FORMAT,
                view_dimension: wgpu::TextureViewDimension::D2Array,
            },
            count: None,
        };
        let read = layout("Coast FFT clipmap read", &[uniform(0, fragment), sampled(1, fragment), sampler(2, fragment)]);
        let write = layout("Coast FFT clipmap write", &[uniform(0, compute), output]);
        let hybrid = layout("Coast FFT clipmap swell", &[uniform(17, compute)]);
        let mip = layout("Coast FFT clipmap mip", &[output, sampled(2, compute)]);
        let [camera, material, waves, environment, coast] = surface_groups;
        let render_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Coast deep-water clipmap layout"),
            bind_group_layouts: &[Some(camera), Some(material), Some(waves), Some(environment), Some(coast), Some(&read)],
            immediate_size: 0,
        });
        let compute_pipeline = |entry, bind_group_layouts: &[Option<&wgpu::BindGroupLayout>]| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(entry), bind_group_layouts, immediate_size: 0,
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry), layout: Some(&layout), module: &shader, entry_point: Some(entry),
                compilation_options: Default::default(), cache: None,
            })
        };
        let bake = compute_pipeline("ninja_wave_bake_main", &[Some(&write), Some(&hybrid), Some(waves)]);
        let downsample = compute_pipeline("ninja_wave_mip", &[Some(&mip)]);
        let draw = super::ninja_coast::split_pipeline(device, &render_layout, &shader, format, samples, true);
        let shore = super::ninja_coast::split_pipeline(device, &render_layout, &shader, format, samples, false);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Coast FFT clipmap filter"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 4,
            ..Default::default()
        });
        let valid = checked_scope(validation);
        let enough_memory = checked_scope(memory);
        valid?;
        enough_memory?;
        Ok(Self { draw, shore, read, write, hybrid, mip, sampler, bake, downsample })
    }
}

pub(super) struct Slot {
    pub draw: wgpu::BindGroup,
    params: wgpu::Buffer,
    write: wgpu::BindGroup,
    hybrid: wgpu::BindGroup,
    mips: Vec<wgpu::BindGroup>,
    last: Option<(u64, wgpu::BindGroup, [f32; 2], (f32, bool))>,
}

impl Slot {
    pub fn new(device: &wgpu::Device, pipe: &Pipeline, swell: &wgpu::Buffer) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Coast FFT clipmap"),
            size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: LEVELS * 2 },
            mip_level_count: MIPS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let view = |level, count| texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            base_mip_level: level,
            mip_level_count: Some(count),
            ..Default::default()
        });
        let params = buf(device, "Coast FFT clipmap window", 32,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
        let group = |layout, entries: &[wgpu::BindGroupEntry<'_>]| {
            device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("Coast FFT clipmap"), layout, entries })
        };
        let draw = group(&pipe.read, &[b(0, &params), t(1, &view(0, MIPS)), s(2, &pipe.sampler)]);
        let write = group(&pipe.write, &[b(0, &params), t(1, &view(0, 1))]);
        let hybrid = group(&pipe.hybrid, &[b(17, swell)]);
        let mips = (1..MIPS).map(|level| {
            group(&pipe.mip, &[t(1, &view(level, 1)), t(2, &view(level - 1, 1))])
        }).collect();
        Self { draw, params, write, hybrid, mips, last: None }
    }

    pub fn encode(&mut self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder,
        pipe: &Pipeline, waves: &wgpu::BindGroup, revision: u64, center: [f32; 2], detail: (f32, bool),
    ) {
        if self.last.as_ref().is_some_and(|(last_revision, last_waves, last_center, last_detail)| {
            *last_revision == revision && last_waves == waves && *last_detail == detail
                && (last_center[0] - center[0]).abs().max((last_center[1] - center[1]).abs()) < EXTENT / 8.0
        }) {
            return;
        }
        queue.write_buffer(&self.params, 0, bytemuck::cast_slice(&[
            [center[0], center[1], EXTENT, LEVELS as f32], [detail.0, 0.0, 0.0, 0.0],
        ]));
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Coast FFT clipmap bake"), timestamp_writes: None,
        });
        pass.set_pipeline(&pipe.bake);
        pass.set_bind_group(0, &self.write, &[]);
        pass.set_bind_group(1, &self.hybrid, &[]);
        pass.set_bind_group(2, waves, &[]);
        let layers = if detail.1 { LEVELS * 2 } else { LEVELS };
        pass.dispatch_workgroups(SIZE / 8, SIZE / 8, layers);
        pass.set_pipeline(&pipe.downsample);
        for (index, group) in self.mips.iter().enumerate() {
            let size = (SIZE >> (index + 1)).div_ceil(8);
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups(size, size, layers);
        }
        self.last = Some((revision, waves.clone(), center, detail));
    }
}
