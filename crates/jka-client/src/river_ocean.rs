//! Shared native FFT normal/height texture producer, independent of hydraulics.
//! Original algorithms Copyright Epic Games, Inc. All Rights Reserved.
use jka_water::river::{
    material,
    ocean::{self, Parameters, SIZE},
};
use wgpu::util::DeviceExt;

struct Pass {
    pipeline: wgpu::ComputePipeline,
    group: wgpu::BindGroup,
    groups: [u32; 2],
}
fn texture(device: &wgpu::Device, mips: u32, label: &str) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
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
fn pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    entry: &str,
) -> wgpu::ComputePipeline {
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(entry),
        layout: None,
        module,
        entry_point: Some(entry),
        compilation_options: Default::default(),
        cache: None,
    })
}
fn group(
    device: &wgpu::Device,
    pipeline: &wgpu::ComputePipeline,
    resources: &[(u32, wgpu::BindingResource<'_>)],
) -> wgpu::BindGroup {
    let entries: Vec<_> = resources
        .iter()
        .map(|(binding, resource)| wgpu::BindGroupEntry {
            binding: *binding,
            resource: resource.clone(),
        })
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("native ocean resources"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &entries,
    })
}
fn pass(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    entry: &str,
    groups: [u32; 2],
    resources: &[(u32, wgpu::BindingResource<'_>)],
) -> Pass {
    let pipeline = pipeline(device, module, entry);
    let group = group(device, &pipeline, resources);
    Pass {
        pipeline,
        group,
        groups,
    }
}
fn dispatch(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::ComputePipeline,
    group: &wgpu::BindGroup,
    groups: [u32; 2],
) {
    let mut p = encoder.begin_compute_pass(&Default::default());
    p.set_pipeline(pipeline);
    p.set_bind_group(0, group, &[]);
    p.dispatch_workgroups(groups[0], groups[1], 1);
}
/// One owner per world, shared by river materials. Keep it animating after
/// hydraulic presentation freezes. It does not create a replacement material.
pub struct Ocean {
    pub texture: wgpu::Texture,
    pub sampler: wgpu::Sampler,
    // RGBA packs the native two R16f attributes. B/A are unused, not new fields.
    pub(crate) x: wgpu::Texture,
    pub(crate) y: wgpu::Texture,
    pub(crate) noise: wgpu::TextureView,
    pub(crate) noise_sampler: wgpu::Sampler,
    x_view: wgpu::TextureView,
    x_pipeline: wgpu::ComputePipeline,
    resets: Vec<Pass>,
    output: Vec<Pass>,
}
/// Immutable age/settings snapshot. Multiple queued frames cannot alias a
/// queue-written time uniform. Pipelines/textures/views/mip groups are retained;
/// preparation makes just a 48-byte uniform and its X-pass group, encoding none.
pub struct Frame<'a> {
    owner: &'a Ocean,
    group: wgpu::BindGroup,
}
impl Ocean {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Self, String> {
        let limits = device.limits();
        if limits.max_compute_workgroup_size_x < 256
            || limits.max_compute_invocations_per_workgroup < 256
            || limits.max_compute_workgroup_storage_size < 4096
            || limits.max_texture_dimension_2d < SIZE
            || limits.max_compute_workgroups_per_dimension < SIZE
        {
            return Err(
                "native 512-cell ocean FFT requires 256-thread groups and 4096-byte shared storage"
                    .into(),
            );
        }
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native ocean FFT"),
            source: wgpu::ShaderSource::Wgsl(ocean::SHADER.into()),
        });
        let mip_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native ocean post-simulate mips"),
            source: wgpu::ShaderSource::Wgsl(material::OUTPUT_SHADER.into()),
        });
        let x = texture(
            device,
            1,
            "native ocean X grid, two half attributes packed in RG",
        );
        let y = texture(
            device,
            1,
            "native ocean Y grid, two half attributes packed in RG",
        );
        let texture = texture(device, 10, "shared native OceanNormalRT RGBA16f");
        let x_view = view(&x, 0);
        let y_view = view(&y, 0);
        let output_view = view(&texture, 0);
        let noise = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("native Good64x64TilingNoiseHighFreq G8"),
            size: wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            noise.as_image_copy(),
            ocean::NOISE,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(64),
                rows_per_image: Some(64),
            },
            noise.size(),
        );
        let noise = noise.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("native ocean RT repeat SampleLevel"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            min_filter: wgpu::FilterMode::Linear,
            mag_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let noise_sampler = sampler.clone();
        let mip_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("native ocean clamp bilinear mip generation"),
            min_filter: wgpu::FilterMode::Linear,
            mag_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let x_pipeline = pipeline(device, &shader, "ocean_x");
        let resets = [&x_view, &y_view]
            .iter()
            .map(|v| {
                pass(
                    device,
                    &shader,
                    "ocean_reset",
                    [64, 64],
                    &[(4, wgpu::BindingResource::TextureView(v))],
                )
            })
            .collect();
        let mut output = vec![
            pass(
                device,
                &shader,
                "ocean_y",
                [SIZE, 1],
                &[
                    (3, wgpu::BindingResource::TextureView(&x_view)),
                    (4, wgpu::BindingResource::TextureView(&y_view)),
                ],
            ),
            pass(
                device,
                &shader,
                "ocean_normals",
                [64, 64],
                &[
                    (3, wgpu::BindingResource::TextureView(&y_view)),
                    (5, wgpu::BindingResource::TextureView(&output_view)),
                ],
            ),
        ];
        for mip in 1..10 {
            let size = SIZE >> mip;
            let data = [
                size,
                size,
                (1.0 / size as f32).to_bits(),
                (1.0 / size as f32).to_bits(),
            ];
            let controls = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("native ocean mip dimensions"),
                contents: bytemuck::cast_slice(&data),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            output.push(pass(
                device,
                &mip_shader,
                "generate_mip",
                [size.div_ceil(8), size.div_ceil(8)],
                &[
                    (0, controls.as_entire_binding()),
                    (
                        7,
                        wgpu::BindingResource::TextureView(&view(&texture, mip - 1)),
                    ),
                    (8, wgpu::BindingResource::Sampler(&mip_sampler)),
                    (9, wgpu::BindingResource::TextureView(&view(&texture, mip))),
                ],
            ));
        }
        Ok(Self {
            texture,
            sampler,
            x,
            y,
            noise,
            noise_sampler,
            x_view,
            x_pipeline,
            resets,
            output,
        })
    }
    pub fn prepare_frame(
        &self,
        device: &wgpu::Device,
        age: f32,
        p: Parameters,
    ) -> Result<Frame<'_>, String> {
        let data = [
            p.gravity,
            p.amplitude,
            p.time_scale,
            p.length,
            p.wind_speed,
            p.wind_angle,
            p.alignment as f32,
            p.slosh,
            age,
            0.0,
            0.0,
            0.0,
        ];
        if data.iter().any(|v| !v.is_finite()) || p.gravity <= 0.0 || p.length <= 0.0 {
            return Err(
                "native ocean requires finite controls, positive gravity and domain length".into(),
            );
        }
        let controls = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("immutable native ocean age/settings"),
            contents: bytemuck::cast_slice(&data),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let group = group(
            device,
            &self.x_pipeline,
            &[
                (0, controls.as_entire_binding()),
                (1, wgpu::BindingResource::TextureView(&self.noise)),
                (2, wgpu::BindingResource::Sampler(&self.noise_sampler)),
                (4, wgpu::BindingResource::TextureView(&self.x_view)),
            ],
        );
        Ok(Frame { owner: self, group })
    }
    /// Native reset clears complex attributes and emits zero-height/up normals,
    /// without executing the NotOnSimulationReset FFT stages.
    pub fn encode_reset(&self, encoder: &mut wgpu::CommandEncoder) {
        for p in &self.resets {
            dispatch(encoder, &p.pipeline, &p.group, p.groups);
        }
        for p in &self.output[1..] {
            dispatch(encoder, &p.pipeline, &p.group, p.groups);
        }
    }
    pub fn logical_bytes(&self) -> u64 {
        // Includes padded half grids and the seed, excludes driver/in-flight data.
        2 * u64::from(SIZE) * u64::from(SIZE) * 8
            + (0..10)
                .map(|m| u64::from(SIZE >> m).pow(2) * 8)
                .sum::<u64>()
            + 4096
    }
}
impl Frame<'_> {
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder) {
        dispatch(encoder, &self.owner.x_pipeline, &self.group, [SIZE, 1]);
        for p in &self.owner.output {
            dispatch(encoder, &p.pipeline, &p.group, p.groups);
        }
    }
}
