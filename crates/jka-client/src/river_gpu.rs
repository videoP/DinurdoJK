//! WGPU scheduling/resources for the source-backed Unreal river stages.
//!
//! This executes the currently ported hydraulic and output stages and can
//! consume river_capture's prepared GPU bed. Dynamic collisions, foam and full
//! renderer integration are still required for the complete river system.

use bytemuck::{Pod, Zeroable};
use jka_water::river::boundary::{Controls, SourceBox, WaterInfo};
use jka_water::river::control::Frame;
use jka_water::river::{Geometry, Parameters, Presentation, Transport, Water, SHALLOW_SHADER};
use sha2::{Digest, Sha256};
use wgpu::util::DeviceExt;

use crate::river_capture::PreparedBed;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kernel {
    SourceAndSink,
    DistributeOverlap,
    ExtrapolateVelocity,
    AdvectWater,
    ExtrapolateHeight,
    UpdateVelocity,
    UpdateDepth,
    MatchSpline,
    PrepareSurface,
    ComputeNormalsAndReset,
}

impl Kernel {
    pub const ALL: [Self; 10] = [
        Self::SourceAndSink,
        Self::DistributeOverlap,
        Self::ExtrapolateVelocity,
        Self::AdvectWater,
        Self::ExtrapolateHeight,
        Self::UpdateVelocity,
        Self::UpdateDepth,
        Self::MatchSpline,
        Self::PrepareSurface,
        Self::ComputeNormalsAndReset,
    ];

    pub fn entry(self) -> &'static str {
        match self {
            Self::SourceAndSink => "source_and_sink",
            Self::DistributeOverlap => "distribute_overlap",
            Self::ExtrapolateVelocity => "extrapolate_velocity",
            Self::AdvectWater => "advect_water",
            Self::ExtrapolateHeight => "extrapolate_height",
            Self::UpdateVelocity => "update_velocity",
            Self::UpdateDepth => "update_depth",
            Self::MatchSpline => "match_spline",
            Self::PrepareSurface => "prepare_surface",
            Self::ComputeNormalsAndReset => "compute_normals_and_reset",
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniform {
    dimensions: [u32; 4],
    stepping: [f32; 4],
    limits: [f32; 4],
    fade: [f32; 4],
}

impl Uniform {
    fn with_parameters(mut self, p: Parameters) -> Self {
        self.stepping = [p.dx, p.dt, p.gravity, p.max_velocity];
        self.limits = [
            p.friction,
            p.velocity_dissipation,
            p.max_depth,
            p.integration_depth_max,
        ];
        self.fade[0] = p.fade_start;
        self.fade[1] = p.fade_speed;
        self
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BoundaryUniform {
    times: [f32; 4],
    origin: [f32; 4],
    flags: [u32; 4],
}

#[derive(Clone, Copy)]
pub struct BoundaryInput<'a> {
    pub water_info: &'a [WaterInfo],
    pub boxes: &'a [SourceBox],
    pub controls: Controls,
}

enum InitialWater<'a> {
    Samples(&'a [Water]),
    Reset,
}

enum GeometryInput<'a> {
    Samples(&'a [Geometry], &'a [f32]),
    Prepared(PreparedBed),
}

/// Both water and velocity DI instances override Niagara's half-float default
/// with ENiagaraGpuBufferFormat::Float (PF_R32_FLOAT). Four separate attribute
/// layers share an array here; sampling never interpolates between layers.
pub const WATER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;

fn state_texture(device: &wgpu::Device, dimensions: [u32; 2], label: &str) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: dimensions[0],
            height: dimensions[1],
            depth_or_array_layers: 4,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: WATER_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

struct InitialUpload {
    buffer: wgpu::Buffer,
    bytes_per_row: u32,
}

fn geometry_bytes(geometry: &[Geometry]) -> Vec<u8> {
    geometry
        .iter()
        .flat_map(|g| {
            [
                g.bottom,
                g.collider,
                g.collision_velocity[0],
                g.collision_velocity[1],
            ]
        })
        .flat_map(f32::to_le_bytes)
        .collect()
}

/// Render outputs own their buffers independently of the simulation. Consuming
/// the simulation with freeze releases its ping-pong/geometry/history resources
/// and bind groups; submitted GPU commands retain their own references safely.
#[derive(Clone)]
pub struct SurfaceFields {
    pub dimensions: [u32; 2],
    pub surface: wgpu::Buffer,
    pub normals: wgpu::Buffer,
}

impl SurfaceFields {
    pub fn logical_bytes(&self) -> u64 {
        u64::from(self.dimensions[0]) * u64::from(self.dimensions[1]) * 32
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Hydraulic,
    Presentation,
    Ready,
}

pub struct Simulation {
    dimensions: [u32; 2],
    state: [wgpu::Texture; 2],
    initial_upload: Option<InitialUpload>,
    groups: [wgpu::BindGroup; 2],
    pipelines: Vec<wgpu::ComputePipeline>,
    snapshot_pipeline: wgpu::ComputePipeline,
    surface: wgpu::Buffer,
    normals: wgpu::Buffer,
    current: usize,
    phase: Phase,
    history_needs_snapshot: bool,
    parameters: Parameters,
    uniform: wgpu::Buffer,
    boundary_uniform: wgpu::Buffer,
    stage_template: Uniform,
    boundary_template: BoundaryUniform,
    #[cfg(test)]
    trace: Option<(wgpu::Device, Vec<(Kernel, wgpu::Texture)>)>,
    bytes: u64,
}

impl Simulation {
    pub fn new(
        device: &wgpu::Device,
        dimensions: [u32; 2],
        water: &[Water],
        geometry: &[Geometry],
        previous_bottom: &[f32],
        p: Parameters,
        t: Transport,
        render: Presentation,
        boundary: BoundaryInput<'_>,
    ) -> Result<Self, String> {
        Self::new_fields(
            device,
            dimensions,
            InitialWater::Samples(water),
            GeometryInput::Samples(geometry, previous_bottom),
            p,
            t,
            render,
            boundary,
            None,
        )
    }

    /// Start a fresh reference solve directly from a GPU-captured bed. Niagara
    /// resets these grids to zero; there is no invented initial fill/velocity.
    /// Capture preparation and this solve can share one encoder/submission.
    pub fn from_prepared_bed(
        device: &wgpu::Device,
        bed: PreparedBed,
        p: Parameters,
        t: Transport,
        render: Presentation,
        boundary: BoundaryInput<'_>,
    ) -> Result<Self, String> {
        let mut info_hash = Sha256::new();
        for w in boundary.water_info {
            for v in [w.height, w.depth, w.flow_velocity[0], w.flow_velocity[1]] {
                info_hash.update(v.to_le_bytes());
            }
        }
        let hash: [u8; 32] = info_hash.finalize().into();
        if hash != bed.water_info_hash
            || p.dx != bed.settings.dx
            || (0..2).any(|axis| {
                let size = bed.dimensions[axis] as f32 * p.dx;
                (size - bed.settings.world_size[axis]).abs() > size * 1e-6
                    || (boundary.controls.grid_min[axis] + size * 0.5).abs() > size * 1e-6
            })
            || boundary.controls.grid_min[2] != 0.0
        {
            return Err("prepared river bed and solver WaterInfo/coordinates differ".into());
        }
        Self::new_fields(
            device,
            bed.dimensions,
            InitialWater::Reset,
            GeometryInput::Prepared(bed),
            p,
            t,
            render,
            boundary,
            None,
        )
    }

    /// GPU WaterInfo preparation and bed selection share one immutable input
    /// identity. No GPU-to-CPU readback/hash or duplicate field upload is needed.
    pub fn from_prepared_field(
        device: &wgpu::Device,
        bed: PreparedBed,
        info: crate::river_water_info::Field,
        p: Parameters,
        t: Transport,
        render: Presentation,
        boxes: &[SourceBox],
        controls: Controls,
    ) -> Result<Self, String> {
        if info.identity != bed.water_info_hash
            || info.dimensions != bed.dimensions
            || p.dx != bed.settings.dx
            || controls.grid_min[2] != 0.0
            || (0..2).any(|i| {
                let size = bed.dimensions[i] as f32 * p.dx;
                (size - bed.settings.world_size[i]).abs() > size * 1e-6
                    || (controls.grid_min[i] + size * 0.5).abs() > size * 1e-6
            })
        {
            return Err("prepared river bed and immutable GPU WaterInfo/coordinates differ".into());
        }
        Self::new_fields(
            device,
            bed.dimensions,
            InitialWater::Reset,
            GeometryInput::Prepared(bed),
            p,
            t,
            render,
            BoundaryInput {
                water_info: &[],
                boxes,
                controls,
            },
            Some(info),
        )
    }

    fn new_fields(
        device: &wgpu::Device,
        dimensions: [u32; 2],
        initial_water: InitialWater<'_>,
        input_geometry: GeometryInput<'_>,
        p: Parameters,
        t: Transport,
        render: Presentation,
        boundary: BoundaryInput<'_>,
        prepared_info: Option<crate::river_water_info::Field>,
    ) -> Result<Self, String> {
        let count = u64::from(dimensions[0]) * u64::from(dimensions[1]);
        let size = count.checked_mul(16).ok_or("river buffer size overflow")?;
        let limits = device.limits();
        if !device
            .features()
            .contains(wgpu::Features::FLOAT32_FILTERABLE)
        {
            return Err(
                "Unreal river R32f grids require enabled FLOAT32_FILTERABLE hardware sampling"
                    .into(),
            );
        }
        let fields_match = match &input_geometry {
            GeometryInput::Samples(geometry, previous_bottom) => {
                count == geometry.len() as u64 && count == previous_bottom.len() as u64
            }
            GeometryInput::Prepared(bed) => {
                dimensions == bed.dimensions
                    && bed.geometry.size() == size
                    && bed.previous_bottom.size() == count * 4
            }
        } && match &initial_water {
            InitialWater::Samples(water) => count == water.len() as u64,
            InitialWater::Reset => true,
        };
        if dimensions.contains(&0)
            || !fields_match
            || match &prepared_info {
                Some(info) => info.dimensions != dimensions || info.buffer.size() != size,
                None => count != boundary.water_info.len() as u64,
            }
            || size > u64::from(limits.max_storage_buffer_binding_size)
            || size > limits.max_buffer_size
            || limits.max_storage_buffers_per_shader_stage < 6
            || limits.max_texture_array_layers < 4
            || dimensions
                .iter()
                .any(|n| *n > limits.max_texture_dimension_2d)
            || dimensions
                .iter()
                .any(|n| n.div_ceil(8) > limits.max_compute_workgroups_per_dimension)
        {
            return Err(
                "river grid does not fit the supplied fields or GPU buffer/dispatch limits".into(),
            );
        }
        let controls = boundary.controls;
        let box_bytes = boundary.boxes.len().max(1) as u64 * 32;
        if box_bytes > u64::from(limits.max_storage_buffer_binding_size)
            || box_bytes > limits.max_buffer_size
            || !controls.frame_dt.is_finite()
            || controls.frame_dt <= 0.0
            || [
                controls.sink_dissipation,
                controls.height_matching_amount,
                controls.remove_outside_amount,
            ]
            .iter()
            .any(|v| !v.is_finite() || *v < 0.0)
            || controls.grid_min.iter().any(|v| !v.is_finite())
            || boundary.water_info.iter().any(|w| {
                [w.height, w.depth, w.flow_velocity[0], w.flow_velocity[1]]
                    .iter()
                    .any(|v| !v.is_finite())
            })
            || boundary.boxes.iter().any(|b| {
                b.position.iter().any(|v| !v.is_finite())
                    || !b.angle.is_finite()
                    || b.size.iter().any(|v| !v.is_finite() || *v <= 0.0)
            })
        {
            return Err("invalid river WaterInfo/source boxes or boundary controls".into());
        }
        let positive = [
            p.dx,
            p.dt,
            p.gravity,
            p.max_depth,
            p.max_velocity,
            p.fade_start,
        ];
        let nonnegative = [
            p.friction,
            p.velocity_dissipation,
            p.integration_depth_max,
            p.fade_speed,
            t.advection_multiplier,
            t.minimum_advection_depth,
            render.height_cutoff,
        ];
        if positive.iter().any(|v| !v.is_finite() || *v <= 0.0)
            || nonnegative.iter().any(|v| !v.is_finite() || *v < 0.0)
            || t.extrapolation_half_width > 32
            || render.half_width > 32
            || render.smoothing_method > 3
            || render.border_width > dimensions[0].min(dimensions[1])
        {
            return Err("invalid river stage parameters".into());
        }
        let invalid_water = match &initial_water {
            InitialWater::Reset => false,
            InitialWater::Samples(water) => water.iter().any(|w| {
                [w.height, w.depth, w.velocity[0], w.velocity[1]]
                    .iter()
                    .any(|v| !v.is_finite())
            }),
        };
        let invalid_geometry = match &input_geometry {
            GeometryInput::Prepared(_) => false,
            GeometryInput::Samples(geometry, previous_bottom) => {
                geometry.iter().any(|g| {
                    [
                        g.bottom,
                        g.collider,
                        g.collision_velocity[0],
                        g.collision_velocity[1],
                    ]
                    .iter()
                    .any(|v| !v.is_finite())
                        || !(0.0..=1.0).contains(&g.collider)
                }) || previous_bottom.iter().any(|v| !v.is_finite())
            }
        };
        if invalid_water || invalid_geometry {
            return Err("nonfinite river fields or collider outside 0..1".into());
        }
        let stage_template = Uniform {
            dimensions: [
                dimensions[0],
                dimensions[1],
                t.extrapolation_half_width,
                u32::from(t.extrapolate_into_dry),
            ],
            stepping: [p.dx, p.dt, p.gravity, p.max_velocity],
            limits: [
                p.friction,
                p.velocity_dissipation,
                p.max_depth,
                p.integration_depth_max,
            ],
            fade: [
                p.fade_start,
                p.fade_speed,
                t.advection_multiplier,
                t.minimum_advection_depth,
            ],
        };
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("UE river stage controls"),
            contents: bytemuck::bytes_of(&stage_template),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let presentation = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("UE river presentation controls"),
            contents: bytemuck::cast_slice(&[
                render.half_width as f32,
                render.height_cutoff,
                render.smoothing_method as f32,
                render.border_width as f32,
            ]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let boundary_template = BoundaryUniform {
            times: [
                controls.frame_dt,
                controls.sink_dissipation,
                controls.height_matching_amount,
                controls.remove_outside_amount,
            ],
            origin: [
                controls.grid_min[0],
                controls.grid_min[1],
                controls.grid_min[2],
                0.0,
            ],
            flags: [
                boundary.boxes.len() as u32,
                u32::from(controls.match_spline),
                0,
                0,
            ],
        };
        let boundary_params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("UE river source sink and spline controls"),
            contents: bytemuck::bytes_of(&boundary_template),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let water_info = if let Some(info) = prepared_info {
            info.buffer
        } else {
            let info_bytes: Vec<u8> = boundary
                .water_info
                .iter()
                .flat_map(|w| [w.height, w.depth, w.flow_velocity[0], w.flow_velocity[1]])
                .flat_map(f32::to_le_bytes)
                .collect();
            let water_info = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("UE river local WaterInfo field"),
                contents: &info_bytes,
                usage: wgpu::BufferUsages::STORAGE,
            });
            water_info
        };
        let mut volumes: Vec<u8> = boundary
            .boxes
            .iter()
            .flat_map(|b| {
                [
                    b.position[0],
                    b.position[1],
                    b.position[2],
                    b.angle,
                    b.size[0],
                    b.size[1],
                    b.size[2],
                    if b.sink { 1.0 } else { 0.0 },
                ]
            })
            .flat_map(f32::to_le_bytes)
            .collect();
        if volumes.is_empty() {
            volumes.resize(32, 0);
        }
        let boxes = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("UE river source and sink box arrays"),
            contents: &volumes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let initial_upload = if let InitialWater::Samples(water) = initial_water {
            let bytes_per_row = (dimensions[0] * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
                * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
            let upload_size = u64::from(bytes_per_row) * u64::from(dimensions[1]) * 4;
            if upload_size > limits.max_buffer_size || upload_size > usize::MAX as u64 {
                return Err("river initial texture upload exceeds GPU/host buffer limits".into());
            }
            let mut bytes = vec![0u8; upload_size as usize];
            for (i, w) in water.iter().enumerate() {
                let x = i % dimensions[0] as usize;
                let y = i / dimensions[0] as usize;
                for (channel, value) in [w.height, w.depth, w.velocity[0], w.velocity[1]]
                    .into_iter()
                    .enumerate()
                {
                    let offset =
                        (channel * dimensions[1] as usize + y) * bytes_per_row as usize + x * 4;
                    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
                }
            }
            Some(InitialUpload {
                buffer: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("UE river initial R32f attributes"),
                    contents: &bytes,
                    usage: wgpu::BufferUsages::COPY_SRC,
                }),
                bytes_per_row,
            })
        } else {
            None
        };
        let state = [
            state_texture(device, dimensions, "UE river R32f state A"),
            state_texture(device, dimensions, "UE river R32f state B"),
        ];
        let (geo, history) = match input_geometry {
            GeometryInput::Prepared(bed) => (bed.geometry, bed.previous_bottom),
            GeometryInput::Samples(geometry, previous_bottom) => {
                let geo = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("UE river geometry"),
                    contents: &geometry_bytes(geometry),
                    usage: wgpu::BufferUsages::STORAGE,
                });
                let old_bottom: Vec<u8> = previous_bottom
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect();
                let history = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("UE river previous bottom"),
                    contents: &old_bottom,
                    usage: wgpu::BufferUsages::STORAGE,
                });
                (geo, history)
            }
        };
        let output = |label| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        };
        let surface = output("UE river rendered height depth and blurred flow");
        let normals = output("UE river surface normals");
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("UE river stage layout"),
            entries: &(0..12)
                .map(|binding| wgpu::BindGroupLayoutEntry {
                    binding,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    count: None,
                    ty: match binding {
                        1 => wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2Array,
                            multisampled: false,
                        },
                        3 => wgpu::BindingType::StorageTexture {
                            access: wgpu::StorageTextureAccess::WriteOnly,
                            format: WATER_FORMAT,
                            view_dimension: wgpu::TextureViewDimension::D2Array,
                        },
                        11 => wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        _ => wgpu::BindingType::Buffer {
                            ty: match binding {
                                0 | 5 | 10 => wgpu::BufferBindingType::Uniform,
                                4 | 6 | 7 => wgpu::BufferBindingType::Storage { read_only: false },
                                _ => wgpu::BufferBindingType::Storage { read_only: true },
                            },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                    },
                })
                .collect::<Vec<_>>(),
        });
        let views = state
            .each_ref()
            .map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()));
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Niagara SF_Bilinear AM_Clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let groups = std::array::from_fn(|i| {
            let mut entries: Vec<_> = [
                (0, &uniform),
                (2, &geo),
                (4, &history),
                (5, &presentation),
                (6, &normals),
                (7, &surface),
                (8, &water_info),
                (9, &boxes),
                (10, &boundary_params),
            ]
            .into_iter()
            .map(|(binding, buffer)| wgpu::BindGroupEntry {
                binding,
                resource: buffer.as_entire_binding(),
            })
            .collect();
            entries.extend([
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&views[i]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&views[1 - i]),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ]);
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("UE river ping-pong stage bindings"),
                layout: &layout,
                entries: &entries,
            })
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("UE river pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("UE river source port"),
            source: wgpu::ShaderSource::Wgsl(SHALLOW_SHADER.into()),
        });
        let pipelines = Kernel::ALL
            .into_iter()
            .map(|kernel| {
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(kernel.entry()),
                    layout: Some(&pipeline_layout),
                    module: &shader,
                    entry_point: Some(kernel.entry()),
                    compilation_options: Default::default(),
                    cache: None,
                })
            })
            .collect();
        let snapshot_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("UE river PreSim bottom history"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("snapshot_bottom"),
            compilation_options: Default::default(),
            cache: None,
        });
        Ok(Self {
            dimensions,
            state,
            initial_upload,
            groups,
            pipelines,
            snapshot_pipeline,
            surface,
            normals,
            current: 0,
            phase: Phase::Hydraulic,
            history_needs_snapshot: false,
            parameters: p,
            uniform,
            boundary_uniform: boundary_params,
            stage_template,
            boundary_template,
            #[cfg(test)]
            trace: None,
            bytes: count * 100 + 128 + box_bytes,
        })
    }

    pub fn encode_kernel(&mut self, encoder: &mut wgpu::CommandEncoder, kernel: Kernel) {
        // Samples supplied to new() are staged once in the first command stream.
        // Fresh prepared-bed solves instead use WebGPU's reset-zero textures.
        if let Some(upload) = self.initial_upload.take() {
            encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo {
                    buffer: &upload.buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(upload.bytes_per_row),
                        rows_per_image: Some(self.dimensions[1]),
                    },
                },
                self.state[0].as_image_copy(),
                self.state[0].size(),
            );
        }
        if kernel == Kernel::SourceAndSink && self.history_needs_snapshot {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("UE river PreSim snapshot bottom"),
                ..Default::default()
            });
            pass.set_pipeline(&self.snapshot_pipeline);
            pass.set_bind_group(0, &self.groups[self.current], &[]);
            pass.dispatch_workgroups(
                self.dimensions[0].div_ceil(8),
                self.dimensions[1].div_ceil(8),
                1,
            );
            self.history_needs_snapshot = false;
        }
        let pipeline = Kernel::ALL.iter().position(|&k| k == kernel).unwrap();
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(kernel.entry()),
                ..Default::default()
            });
            pass.set_pipeline(&self.pipelines[pipeline]);
            pass.set_bind_group(0, &self.groups[self.current], &[]);
            pass.dispatch_workgroups(
                self.dimensions[0].div_ceil(8),
                self.dimensions[1].div_ceil(8),
                1,
            );
        }
        self.current = 1 - self.current;
        #[cfg(test)]
        if let Some((device, trace)) = &mut self.trace {
            // Test-only snapshots are GPU copies; readback happens after the
            // whole submission, preserving the production ordering under test.
            let copy = state_texture(device, self.dimensions, "river stage trace");
            encoder.copy_texture_to_texture(
                self.state[self.current].as_image_copy(),
                copy.as_image_copy(),
                copy.size(),
            );
            trace.push((kernel, copy));
        }
        self.phase = match kernel {
            Kernel::PrepareSurface => Phase::Presentation,
            Kernel::ComputeNormalsAndReset if self.phase == Phase::Presentation => Phase::Ready,
            _ => Phase::Hydraulic,
        };
        if self.phase == Phase::Ready {
            self.history_needs_snapshot = true;
        }
    }

    /// Execute the selected stages 6..14 against prepared geometry/WaterInfo.
    /// Static history snapshots are included; emitter/substep policy, dynamic
    /// collision updates and foam remain separate.
    pub fn encode_hydraulics_and_surface(&mut self, encoder: &mut wgpu::CommandEncoder) {
        for kernel in Kernel::ALL {
            self.encode_kernel(encoder, kernel);
        }
    }

    /// Main emitter's actual execution loops: stages 6/7 once, 8..12 repeated,
    /// 13/14 once. Control uploads are recorded GPU copies, so multiple frames
    /// with different timestep/sink deltas can share a submission safely.
    pub fn encode_reference_frame(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        frame: Frame,
    ) -> Result<(), &'static str> {
        if frame.parameters().dx != self.parameters.dx {
            return Err("river frame cell size differs from the captured simulation grid");
        }
        let stage = self.stage_template.with_parameters(frame.parameters());
        let mut boundary = self.boundary_template;
        boundary.times[0] = frame.engine_delta();
        let mut bytes = [0u8; 112];
        bytes[..64].copy_from_slice(bytemuck::bytes_of(&stage));
        bytes[64..].copy_from_slice(bytemuck::bytes_of(&boundary));
        let upload = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("UE river queued frame controls"),
            contents: &bytes,
            usage: wgpu::BufferUsages::COPY_SRC,
        });
        encoder.copy_buffer_to_buffer(&upload, 0, &self.uniform, 0, 64);
        encoder.copy_buffer_to_buffer(&upload, 64, &self.boundary_uniform, 0, 48);
        self.encode_kernel(encoder, Kernel::SourceAndSink);
        self.encode_kernel(encoder, Kernel::DistributeOverlap);
        for _ in 0..frame.substeps() {
            for kernel in &Kernel::ALL[2..8] {
                self.encode_kernel(encoder, *kernel);
            }
        }
        self.encode_kernel(encoder, Kernel::PrepareSurface);
        self.encode_kernel(encoder, Kernel::ComputeNormalsAndReset);
        Ok(())
    }

    /// Hydraulic attributes; initial sample uploads are recorded by the first
    /// encode call. Fresh captured-bed solves have reset-zero initial state.
    pub fn state(&self) -> &wgpu::Texture {
        &self.state[self.current]
    }

    #[cfg(test)]
    pub fn enable_trace(&mut self, device: &wgpu::Device) {
        self.trace = Some((device.clone(), Vec::new()));
    }

    #[cfg(test)]
    pub fn take_trace(&mut self) -> Vec<(Kernel, wgpu::Texture)> {
        self.trace.take().map(|(_, v)| v).unwrap_or_default()
    }
    pub fn surface(&self) -> &wgpu::Buffer {
        &self.surface
    }
    pub fn normals(&self) -> &wgpu::Buffer {
        &self.normals
    }
    pub fn surface_ready(&self) -> bool {
        self.phase == Phase::Ready
    }

    /// Shared output handles for material texture generation while solving.
    /// Geometry and hydraulic ping-pong resources are not retained by this view.
    pub fn output_fields(&self) -> Option<SurfaceFields> {
        self.surface_ready().then(|| SurfaceFields {
            dimensions: self.dimensions,
            surface: self.surface.clone(),
            normals: self.normals.clone(),
        })
    }
    pub fn logical_bytes(&self) -> u64 {
        self.bytes + self.initial_upload.as_ref().map_or(0, |u| u.buffer.size())
    }

    /// Freeze is legal only after the output stage pair. On an invalid phase
    /// return the original simulation so callers cannot lose an evolving solve.
    pub fn freeze(self) -> Result<SurfaceFields, Self> {
        if !self.surface_ready() {
            return Err(self);
        }
        Ok(SurfaceFields {
            dimensions: self.dimensions,
            surface: self.surface,
            normals: self.normals,
        })
    }
}
