//! UE 5.6 Niagara Heterogeneous Volumes live shading (lighting cache, ray
//! march at LightingDownsampleFactor, HeterogeneousVolumesComposite), shared
//! by the Fluid Ninja fog HVOL and the Fire_Smoke_Explosive smoke volume.
//! Each instance supplies a shader module built from the froxel lighting
//! library (group 0), its material bindings (group 1), the instance hooks
//! documented in `ninja_hvol.wgsl`, and that stage (`STAGE_WGSL`).
use super::ninja_fog::{VolumeParams, VOLUME_FORMAT};

/// The selected EffectsQuality@3 downsample (UE LightingDownsampleFactor 2).
pub(in crate::renderer) const DOWNSAMPLE: u32 = 2;
pub(in crate::renderer) const STAGE_WGSL: &str = include_str!("ninja_hvol.wgsl");
/// Shared `NinjaVolumeParams` struct and view-ray helpers.
pub(in crate::renderer) const COMMON_WGSL: &str = include_str!("ninja_fog_volume_common.wgsl");

pub(in crate::renderer) struct Pipeline {
    cache_pipeline: wgpu::ComputePipeline,
    march_pipeline: wgpu::ComputePipeline,
    composite: wgpu::RenderPipeline,
    cache_layout: wgpu::BindGroupLayout,
    march_layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
    blue_noise: wgpu::TextureView,
    upsample: wgpu::Sampler,
    cache_sampler: wgpu::Sampler,
    cache_size: [u32; 3],
}

pub(in crate::renderer) struct Target {
    size: [u32; 2],
    cache: wgpu::TextureView,
    radiance: wgpu::TextureView,
    cache_group: wgpu::BindGroup,
    composite_group: wgpu::BindGroup,
}

impl Pipeline {
    /// `source` is the complete instance module ending with `STAGE_WGSL`.
    /// `cache_size` is the lighting cache: the voxel resolution over the
    /// lighting downsample at LOD 0.
    pub(in crate::renderer) fn new(
        device: &wgpu::Device,
        lights: &wgpu::BindGroupLayout,
        material: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        blue_noise: wgpu::TextureView,
        source: String,
        cache_size: [u32; 3],
    ) -> Self {
        let compute = wgpu::ShaderStages::COMPUTE;
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let d2 = wgpu::TextureViewDimension::D2;
        let d3 = wgpu::TextureViewDimension::D3;
        let uniform = wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: compute,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<VolumeParams>() as u64),
            },
            count: None,
        };
        let read = |binding, visibility, view_dimension, filterable| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension,
                multisampled: false,
            },
            count: None,
        };
        let write = |binding, view_dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: compute,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: VOLUME_FORMAT,
                view_dimension,
            },
            count: None,
        };
        let layout = |label, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries,
            })
        };
        let cache_layout = layout("Ninja HVOL lighting cache", &[uniform, write(3, d3)]);
        let march_layout = layout("Ninja HVOL ray march", &[
            uniform, read(1, compute, d2, false), read(2, compute, d2, false),
            read(4, compute, d3, true), write(5, d2),
            wgpu::BindGroupLayoutEntry {
                binding: 6,
                visibility: compute,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ]);
        let composite_layout = layout("Ninja HVOL composite", &[
            read(0, fragment, d2, true),
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: fragment,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ]);
        let upsample = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Ninja HVOL bilinear upsample"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // Trilinear clamp lighting-cache lookups.
        let cache_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Ninja HVOL lighting cache sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Ninja HVOL live shading"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline_layout = |label, groups: &[Option<&wgpu::BindGroupLayout>]| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: groups,
                immediate_size: 0,
            })
        };
        let pipeline = |entry, layout| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout(entry, &[Some(lights), Some(material), Some(layout)])),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: crate::pipeline_cache::get().as_ref(),
            })
        };
        let cache_pipeline = pipeline("hv_build_lighting_cache", &cache_layout);
        let march_pipeline = pipeline("hv_render", &march_layout);
        let apply = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Ninja HVOL scene composite"),
            source: wgpu::ShaderSource::Wgsl(include_str!("ninja_hvol_composite.wgsl").into()),
        });
        let composite = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Ninja HVOL scene composite"),
            layout: Some(&pipeline_layout("Ninja HVOL scene composite", &[Some(&composite_layout)])),
            vertex: wgpu::VertexState {
                module: &apply,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &apply,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(super::ninja_fog::volume_composite_target(format))],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: crate::pipeline_cache::get().as_ref(),
        });
        Self {
            cache_pipeline, march_pipeline, composite, cache_layout, march_layout, composite_layout,
            blue_noise, upsample, cache_sampler, cache_size,
        }
    }

    pub(in crate::renderer) fn create_target(&self, device: &wgpu::Device, params: &wgpu::Buffer, viewport: [u32; 2]) -> Target {
        let size = viewport.map(|n| n.div_ceil(DOWNSAMPLE));
        let texture = |label, [width, height, depth_or_array_layers]: [u32; 3], dimension| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width, height, depth_or_array_layers },
                mip_level_count: 1,
                sample_count: 1,
                dimension,
                format: VOLUME_FORMAT,
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            }).create_view(&Default::default())
        };
        let cache = texture("Ninja HVOL lighting cache", self.cache_size, wgpu::TextureDimension::D3);
        let radiance = texture("Ninja HVOL radiance and transmittance", [size[0], size[1], 1], wgpu::TextureDimension::D2);
        let cache_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Ninja HVOL lighting cache"),
            layout: &self.cache_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
                view_entry(3, &cache),
            ],
        });
        let composite_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Ninja HVOL composite"),
            layout: &self.composite_layout,
            entries: &[
                view_entry(0, &radiance),
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.upsample) },
            ],
        });
        Target { size, cache, radiance, cache_group, composite_group }
    }

    pub(in crate::renderer) fn encode(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        params: &wgpu::Buffer,
        target: &Target,
        lights: &wgpu::BindGroup,
        material: &wgpu::BindGroup,
        depth: &wgpu::TextureView,
        scene: &wgpu::TextureView,
    ) {
        let march_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Ninja HVOL ray march"),
            layout: &self.march_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
                view_entry(1, depth),
                view_entry(2, &self.blue_noise),
                view_entry(4, &target.cache),
                view_entry(5, &target.radiance),
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::Sampler(&self.cache_sampler) },
            ],
        });
        {
            let [x, y, z] = self.cache_size.map(|axis| axis.div_ceil(4));
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Ninja HVOL lighting cache"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.cache_pipeline);
            pass.set_bind_group(0, lights, &[]);
            pass.set_bind_group(1, material, &[]);
            pass.set_bind_group(2, &target.cache_group, &[]);
            pass.dispatch_workgroups(x, y, z);
        }
        {
            let [x, y] = target.size.map(|axis| axis.div_ceil(8));
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Ninja HVOL ray march"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.march_pipeline);
            pass.set_bind_group(0, lights, &[]);
            pass.set_bind_group(1, material, &[]);
            pass.set_bind_group(2, &march_group, &[]);
            pass.dispatch_workgroups(x, y, 1);
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Ninja HVOL scene composite"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: scene,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.composite);
        pass.set_bind_group(0, &target.composite_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn view_entry(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::TextureView(view) }
}
