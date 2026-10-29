//! Native WaterInfo merge/finalize/decode. Raw inputs must come from actual
//! ground/water/dilated-water captures; no inferred spline or discharge field.
use bytemuck::{Pod, Zeroable};
use jka_water::river::{
    boundary::WaterInfo,
    water_info::{self, Pixel, Zone, MAX_FLOW_VELOCITY},
};
use sha2::{Digest, Sha256};
use wgpu::util::DeviceExt;

#[derive(Clone, Copy, Debug)]
pub enum Precision {
    Half,
    Full,
}
impl Precision {
    fn format(self) -> wgpu::TextureFormat {
        match self {
            Self::Half => wgpu::TextureFormat::Rgba16Float,
            Self::Full => wgpu::TextureFormat::Rgba32Float,
        }
    }
    pub fn shader(self) -> String {
        water_info::SHADER.replace(
            "FIELD_FORMAT",
            match self {
                Self::Half => "rgba16float",
                Self::Full => "rgba32float",
            },
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub dimensions: [u32; 2],
    pub simulation_dimensions: [u32; 2],
    pub zone: Zone,
    pub origin_z: f32,
    pub grid_min: [f32; 2],
    pub grid_size: [f32; 2],
    pub zone_min: [f32; 2],
    pub zone_size: [f32; 2],
    pub blur_radius: i32,
    pub precision: Precision,
}

/// Immutable input identity includes source inputs/settings/shader version; it
/// is NOT a claimed hash of GPU output bytes. Clones share the same GPU buffer.
#[derive(Clone)]
pub struct Field {
    pub(crate) dimensions: [u32; 2],
    pub(crate) buffer: wgpu::Buffer,
    pub(crate) identity: [u8; 32],
}
impl Field {
    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }
    pub fn from_samples(
        device: &wgpu::Device,
        dimensions: [u32; 2],
        samples: &[WaterInfo],
    ) -> Result<Self, String> {
        let size = (u64::from(dimensions[0]) * u64::from(dimensions[1]))
            .checked_mul(16)
            .ok_or("WaterInfo field size overflow")?;
        if dimensions.contains(&0)
            || size / 16 != samples.len() as u64
            || size > u64::from(device.limits().max_storage_buffer_binding_size)
            || size > device.limits().max_buffer_size
            || samples.iter().any(|w| {
                [w.height, w.depth, w.flow_velocity[0], w.flow_velocity[1]]
                    .iter()
                    .any(|v| !v.is_finite())
            })
        {
            return Err("invalid WaterInfo samples/dimensions or GPU binding size".into());
        }
        let bytes: Vec<_> = samples
            .iter()
            .flat_map(|w| [w.height, w.depth, w.flow_velocity[0], w.flow_velocity[1]])
            .flat_map(f32::to_le_bytes)
            .collect();
        Ok(Self {
            dimensions,
            identity: Sha256::digest(&bytes).into(),
            buffer: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("immutable river WaterInfo samples"),
                contents: &bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            }),
        })
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniform {
    dimensions: [u32; 4],
    simulation: [u32; 4],
    heights: [f32; 4],
    controls: [f32; 4],
    grid: [f32; 4],
    zone: [f32; 4],
}

/// Three source operations in one command stream. Encoded intermediate
/// textures quantize at BOTH writes, according to the zone's chosen format.
pub fn encode(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    s: Settings,
    pixels: &[Pixel],
) -> Result<Field, String> {
    validate_settings(device, s)?;
    if u64::from(s.dimensions[0]) * u64::from(s.dimensions[1]) != pixels.len() as u64
        || pixels.iter().any(|v| {
            [
                v.ground_depth,
                v.water_depth,
                v.dilation_depth,
                v.encoded_velocity[0],
                v.encoded_velocity[1],
                s.zone.capture_z - v.ground_depth,
                s.zone.capture_z - v.water_depth,
                s.zone.capture_z - v.dilation_depth,
            ]
            .iter()
            .any(|x| !x.is_finite())
                || v.encoded_velocity.iter().any(|x| {
                    !(*x * 1089.0).is_finite()
                        || !((*x - 0.5) * 2048.0).is_finite()
                        || (matches!(s.precision, Precision::Half) && x.abs() > 65504.0)
                })
        })
    {
        return Err("invalid WaterInfo capture pixels".into());
    }
    let bytes: Vec<_> = pixels
        .iter()
        .flat_map(|v| {
            [
                v.ground_depth,
                v.water_depth,
                v.dilation_depth,
                0.0,
                v.encoded_velocity[0],
                v.encoded_velocity[1],
                0.0,
                0.0,
            ]
        })
        .flat_map(f32::to_le_bytes)
        .collect();
    let data = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("point sampled WaterInfo capture inputs"),
        contents: &bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });
    encode_buffer(device, encoder, s, &data, Sha256::digest(&bytes).into())
}

pub(crate) fn validate_settings(device: &wgpu::Device, s: Settings) -> Result<(), String> {
    s.zone.validate()?;
    let count = u64::from(s.dimensions[0]) * u64::from(s.dimensions[1]);
    let out = u64::from(s.simulation_dimensions[0]) * u64::from(s.simulation_dimensions[1]);
    let limits = device.limits();
    if s.dimensions.contains(&0)
        || s.simulation_dimensions.contains(&0)
        || s.dimensions
            .iter()
            .any(|n| *n > limits.max_texture_dimension_2d)
        || s.dimensions
            .iter()
            .chain(s.simulation_dimensions.iter())
            .any(|n| n.div_ceil(8) > limits.max_compute_workgroups_per_dimension)
        || count.checked_mul(32).is_none_or(|n| {
            n > u64::from(limits.max_storage_buffer_binding_size) || n > limits.max_buffer_size
        })
        || out.checked_mul(16).is_none_or(|n| {
            n > u64::from(limits.max_storage_buffer_binding_size) || n > limits.max_buffer_size
        })
        || [
            s.origin_z,
            s.zone.water_min - s.origin_z,
            s.zone.water_max - s.origin_z,
            s.zone.ground_min - 256.0 - s.origin_z,
        ]
        .iter()
        .any(|v| !v.is_finite())
        || s.grid_min
            .iter()
            .chain(s.zone_min.iter())
            .any(|v| !v.is_finite())
        || s.grid_size
            .iter()
            .chain(s.zone_size.iter())
            .any(|v| !v.is_finite() || *v <= 0.0)
    {
        return Err("invalid WaterInfo captures, mapping or GPU limits".into());
    }
    if matches!(s.precision, Precision::Full)
        && !device
            .features()
            .contains(wgpu::Features::FLOAT32_FILTERABLE)
    {
        return Err("full precision WaterInfo sampling requires enabled FLOAT32_FILTERABLE".into());
    }
    Ok(())
}

/// Same operations with GPU-produced point captures. The identity describes
/// the capture inputs/version, not a hash of unread GPU output bytes.
pub(crate) fn encode_buffer(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    s: Settings,
    data: &wgpu::Buffer,
    source_identity: [u8; 32],
) -> Result<Field, String> {
    validate_settings(device, s)?;
    if data.size() != u64::from(s.dimensions[0]) * u64::from(s.dimensions[1]) * 32 {
        return Err("WaterInfo raw GPU capture buffer size differs".into());
    }
    let out = u64::from(s.simulation_dimensions[0]) * u64::from(s.simulation_dimensions[1]);
    let uniform = Uniform {
        dimensions: [
            s.dimensions[0],
            s.dimensions[1],
            s.blur_radius.clamp(0, 16) as u32,
            0,
        ],
        simulation: [s.simulation_dimensions[0], s.simulation_dimensions[1], 0, 0],
        heights: [
            s.zone.water_min,
            s.zone.water_max,
            s.zone.ground_min,
            s.zone.capture_z,
        ],
        controls: [
            s.zone.underground_offset,
            MAX_FLOW_VELOCITY,
            s.origin_z,
            0.0,
        ],
        grid: [s.grid_min[0], s.grid_min[1], s.grid_size[0], s.grid_size[1]],
        zone: [s.zone_min[0], s.zone_min[1], s.zone_size[0], s.zone_size[1]],
    };
    let mut hash = Sha256::new();
    hash.update(water_info::SHADER.as_bytes());
    hash.update(bytemuck::bytes_of(&uniform));
    hash.update([matches!(s.precision, Precision::Full) as u8]);
    hash.update(source_identity);
    let identity = hash.finalize().into();
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("native WaterInfo zone/mapping"),
        contents: bytemuck::bytes_of(&uniform),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let texture = |label| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: s.dimensions[0],
                height: s.dimensions[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: s.precision.format(),
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        })
    };
    let merged = texture("native merged WaterInfo");
    let finalized = texture("native finalized WaterInfo");
    let views = [
        merged.create_view(&Default::default()),
        finalized.create_view(&Default::default()),
    ];
    let decoded = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("decoded local river WaterInfo"),
        size: out * 16,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("WaterInfo resource wrap LOD-zero sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        min_filter: wgpu::FilterMode::Linear,
        mag_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("native WaterInfo operations"),
        entries: &(0..6)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                count: None,
                ty: match binding {
                    2 => wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    3 => wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: s.precision.format(),
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    4 => wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    _ => wgpu::BindingType::Buffer {
                        ty: if binding == 0 {
                            wgpu::BufferBindingType::Uniform
                        } else {
                            wgpu::BufferBindingType::Storage {
                                read_only: binding == 1,
                            }
                        },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                },
            })
            .collect::<Vec<_>>(),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("native WaterInfo pipeline"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("native WaterInfo source port"),
        source: wgpu::ShaderSource::Wgsl(s.precision.shader().into()),
    });
    // Input/output views are distinct even for stages that do not use one side.
    for (entry, input, output, dimensions) in [
        ("merge", 1, 0, s.dimensions),
        ("finalize", 0, 1, s.dimensions),
        ("decode", 1, 0, s.simulation_dimensions),
    ] {
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(entry),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: data.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&views[input]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&views[output]),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: decoded.as_entire_binding(),
                },
            ],
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            cache: None,
        });
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(dimensions[0].div_ceil(8), dimensions[1].div_ceil(8), 1);
    }
    Ok(Field {
        dimensions: s.simulation_dimensions,
        buffer: decoded,
        identity,
    })
}
