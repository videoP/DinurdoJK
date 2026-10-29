//! Native default live-grid output textures and stage-17 normal blur.
//! No water optical shader or substitute material is supplied here.
use crate::river_gpu::SurfaceFields;
use jka_water::river::material::OUTPUT_SHADER;
use wgpu::util::DeviceExt;

pub struct Textures {
    pub dimensions: [u32; 2],
    pub surface: wgpu::Texture,
    pub normals: wgpu::Texture,
    /// Native newly-created UTextureRenderTarget2D address bytes are zero:
    /// TA_Wrap. Desktop RenderTarget group has linear min/mag, point mip.
    pub normal_sampler: wgpu::Sampler,
}
impl Textures {
    pub fn logical_bytes(&self) -> u64 {
        let mut size = self.dimensions;
        let mut total = u64::from(size[0]) * u64::from(size[1]) * 8;
        for _ in 0..self.surface.mip_level_count() {
            total += u64::from(size[0]) * u64::from(size[1]) * 8;
            size = size.map(|n| (n >> 1).max(1));
        }
        total
    }
}

struct Pass {
    pipeline: wgpu::ComputePipeline,
    group: wgpu::BindGroup,
    dimensions: [u32; 2],
}
/// Allocate per river, encode after the hydraulic/output stages, and retain
/// across frames. Bind groups and controls are immutable; encoding creates no
/// resources or CPU uploads. Freeze drops input bindings and the half NormalGrid
/// scratch; pending commands keep their own references without a blocking wait.
pub struct Presentation {
    textures: Textures,
    passes: Vec<Pass>,
    ready: bool,
}

fn texture(device: &wgpu::Device, dimensions: [u32; 2], mips: u32, label: &str) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: dimensions[0],
            height: dimensions[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}
fn view(texture: &wgpu::Texture, mip: u32) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        base_mip_level: mip,
        mip_level_count: Some(1),
        ..Default::default()
    })
}

impl Presentation {
    /// `fields` must be the current stage-13/14 outputs. Their buffer handles
    /// remain stable while solving; Simulation::output_fields gates initial
    /// access on completion of those encoded stages, without CPU readback.
    pub fn new(device: &wgpu::Device, fields: &SurfaceFields) -> Result<Self, String> {
        let dimensions = fields.dimensions;
        let limits = device.limits();
        let bytes = (u64::from(dimensions[0]) * u64::from(dimensions[1]))
            .checked_mul(16)
            .ok_or("native river material output size overflow")?;
        if dimensions.contains(&0)
            || dimensions.iter().any(|n| {
                *n > limits.max_texture_dimension_2d
                    || n.div_ceil(8) > limits.max_compute_workgroups_per_dimension
            })
            || bytes > u64::from(limits.max_storage_buffer_binding_size)
            || [&fields.surface, &fields.normals]
                .iter()
                .any(|b| b.size() != bytes || !b.usage().contains(wgpu::BufferUsages::STORAGE))
        {
            return Err("invalid native river material output buffers or GPU limits".into());
        }
        let mips = u32::BITS - dimensions[0].max(dimensions[1]).leading_zeros();
        let surface = texture(
            device,
            dimensions,
            mips,
            "native SimGridRT RGBA16f with post-simulate mips",
        );
        let normals = texture(device, dimensions, 1, "native NormalsRT RGBA16f");
        // Equivalent half precision per component; three native R16f attribute
        // layers share RGBA16f here. Alpha is unused and remains zero.
        let grid = texture(device, dimensions, 1, "native half NormalGrid scratch");
        let normal_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("native normal RT resource sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            min_filter: wgpu::FilterMode::Linear,
            mag_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let mip_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("native NiagaraGenerateMips SF_Bilinear AM_Clamp"),
            min_filter: wgpu::FilterMode::Linear,
            mag_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native river final fields and mips"),
            source: wgpu::ShaderSource::Wgsl(OUTPUT_SHADER.into()),
        });
        let mut passes = Vec::new();
        let mut add =
            |entry: &str, size: [u32; 2], resources: &[(u32, wgpu::BindingResource<'_>)]| {
                let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(entry),
                    layout: None,
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                });
                let data = [
                    size[0],
                    size[1],
                    (1.0 / size[0] as f32).to_bits(),
                    (1.0 / size[1] as f32).to_bits(),
                ];
                let controls = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("native river material pass dimensions"),
                    contents: bytemuck::cast_slice(&data),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
                let mut entries = vec![wgpu::BindGroupEntry {
                    binding: 0,
                    resource: controls.as_entire_binding(),
                }];
                entries.extend(
                    resources
                        .iter()
                        .map(|(binding, resource)| wgpu::BindGroupEntry {
                            binding: *binding,
                            resource: resource.clone(),
                        }),
                );
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(entry),
                    layout: &pipeline.get_bind_group_layout(0),
                    entries: &entries,
                });
                passes.push(Pass {
                    pipeline,
                    group,
                    dimensions: size,
                });
            };
        let surface0 = view(&surface, 0);
        let grid0 = view(&grid, 0);
        let normal0 = view(&normals, 0);
        add(
            "write_fields",
            dimensions,
            &[
                (1, fields.surface.as_entire_binding()),
                (2, fields.normals.as_entire_binding()),
                (3, wgpu::BindingResource::TextureView(&surface0)),
                (4, wgpu::BindingResource::TextureView(&grid0)),
            ],
        );
        add(
            "blur_normals",
            dimensions,
            &[
                (5, wgpu::BindingResource::TextureView(&grid0)),
                (6, wgpu::BindingResource::TextureView(&normal0)),
            ],
        );
        for mip in 1..mips {
            let size = dimensions.map(|n| (n >> mip).max(1));
            let input = view(&surface, mip - 1);
            let output = view(&surface, mip);
            add(
                "generate_mip",
                size,
                &[
                    (7, wgpu::BindingResource::TextureView(&input)),
                    (8, wgpu::BindingResource::Sampler(&mip_sampler)),
                    (9, wgpu::BindingResource::TextureView(&output)),
                ],
            );
        }
        Ok(Self {
            textures: Textures {
                dimensions,
                surface,
                normals,
                normal_sampler,
            },
            passes,
            ready: false,
        })
    }

    pub fn encode(&mut self, encoder: &mut wgpu::CommandEncoder) {
        for p in &self.passes {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("native river material output"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&p.pipeline);
            pass.set_bind_group(0, &p.group, &[]);
            pass.dispatch_workgroups(p.dimensions[0].div_ceil(8), p.dimensions[1].div_ceil(8), 1);
        }
        self.ready = true;
    }
    pub fn textures(&self) -> Option<&Textures> {
        self.ready.then_some(&self.textures)
    }
    pub fn freeze(self) -> Result<Textures, Self> {
        if self.ready {
            Ok(self.textures)
        } else {
            Err(self)
        }
    }
}
