//! Native WaterInfo ground/colour/dilation captures feeding GPU merge/decode.
//! Evaluated ribbon geometry and map ground classification are explicit inputs.
//! No procedural substitute spline, guessed mask or CPU capture readback.
use crate::{
    river_capture::Sides,
    river_water_info::{self, Field, Settings},
};
use jka_water::river::water_body::{Mesh, Meshes};
use sha2::{Digest, Sha256};
use wgpu::util::DeviceExt;

pub const SHADER: &str = include_str!("river_water_info_capture.wgsl");
/// EngineDefines.h UE_FLOAT_HUGE_DISTANCE / 4, WaterInfoRendering::BuildOrthoMatrix.
pub const FAR_CM: f32 = 1048576.0 / 4.0;
const CAPTURE_VERSION: u32 = 1;

pub struct Ground<'a> {
    /// Capture-local centimetres, with the explicit world origin below.
    pub positions: &'a [[f32; 3]],
    pub indices: &'a [u32],
    pub sides: Sides,
}
pub struct Body<'a> {
    pub mesh: &'a Meshes,
    /// Affine local-spline -> capture-local transform, evaluated in doubles
    /// before upload. Translation includes subtracting the capture world origin.
    pub local_to_capture: [[f64; 4]; 3],
}
pub const IDENTITY: [[f64; 4]; 3] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
];

struct Draw<'a> {
    pass: usize,
    sides: Sides,
    vertices: Vec<u8>,
    indices: &'a [u32],
}
fn body_draw<'a>(
    mesh: &'a Mesh,
    transform: [[f64; 4]; 3],
    pass: usize,
    buffer_limit: u64,
) -> Result<Draw<'a>, String> {
    let determinant = transform[0][0]
        * (transform[1][1] * transform[2][2] - transform[1][2] * transform[2][1])
        - transform[0][1] * (transform[1][0] * transform[2][2] - transform[1][2] * transform[2][0])
        + transform[0][2] * (transform[1][0] * transform[2][1] - transform[1][1] * transform[2][0]);
    if !determinant.is_finite() {
        return Err("water-body transform determinant overflow".into());
    }
    if mesh.colors.len() != mesh.positions.len()
        || mesh
            .positions
            .len()
            .checked_mul(16)
            .is_none_or(|n| n as u64 > buffer_limit)
    {
        return Err("water-body vertex colour count or buffer size differs".into());
    }
    let mut vertices = Vec::with_capacity(mesh.positions.len() * 16);
    for (p, c) in mesh.positions.iter().zip(&mesh.colors) {
        if p.iter().any(|x| !x.is_finite()) {
            return Err("nonfinite water-body position".into());
        }
        for row in transform {
            let value = (row[0] * p[0] + row[1] * p[1] + row[2] * p[2] + row[3]) as f32;
            if !value.is_finite() {
                return Err("water-body transform overflow".into());
            }
            vertices.extend(value.to_le_bytes());
        }
        vertices.extend(c);
    }
    // Native ribbon indices are clockwise in local XY. StaticMeshSceneProxy
    // reverses culling for a negative local-to-world determinant.
    Ok(Draw {
        pass,
        sides: if determinant < 0.0 {
            Sides::CounterClockwise
        } else {
            Sides::Clockwise
        },
        vertices,
        indices: &mesh.indices,
    })
}

/// Default native WaterInfo method 2: three reverse-Z scene passes, then
/// point-pixel scene-depth resolve and the existing native merge/finalize/decode.
/// All stages may precede bed preparation/solving in the same command buffer.
pub fn encode(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    s: Settings,
    world_origin: [f64; 3],
    ground: &[Ground<'_>],
    bodies: &[Body<'_>],
) -> Result<Field, String> {
    river_water_info::validate_settings(device, s)?;
    let limits = device.limits();
    let projection = [
        (f64::from(s.zone_min[0]) + f64::from(s.zone_size[0]) * 0.5 - world_origin[0]) as f32,
        (f64::from(s.zone_min[1]) + f64::from(s.zone_size[1]) * 0.5 - world_origin[1]) as f32,
        (f64::from(s.zone.capture_z) - world_origin[2]) as f32,
        FAR_CM,
    ];
    if world_origin.iter().any(|x| !x.is_finite())
        || world_origin[2] as f32 != s.origin_z
        || projection.iter().any(|x| !x.is_finite())
        || limits.max_texture_array_layers < 3
    {
        return Err("invalid WaterInfo capture origin/projection or GPU limits".into());
    }
    let camera = [projection, [s.zone_size[0], s.zone_size[1], 1024.0, 0.0]];
    let mut draws = Vec::new();
    for g in ground {
        if g.positions
            .len()
            .checked_mul(16)
            .is_none_or(|n| n as u64 > limits.max_buffer_size)
        {
            return Err("WaterInfo ground vertex buffer exceeds GPU limits".into());
        }
        let mut vertices = Vec::with_capacity(g.positions.len() * 16);
        for p in g.positions {
            if p.iter().any(|x| !x.is_finite()) {
                return Err("nonfinite WaterInfo ground".into());
            }
            for v in p {
                vertices.extend(v.to_le_bytes());
            }
            vertices.extend([0u8; 4]);
        }
        draws.push(Draw {
            pass: 0,
            sides: g.sides,
            vertices,
            indices: g.indices,
        });
    }
    for body in bodies {
        if body
            .local_to_capture
            .iter()
            .flatten()
            .any(|x| !x.is_finite())
        {
            return Err("nonfinite WaterInfo body transform".into());
        }
        draws.push(body_draw(
            &body.mesh.surface,
            body.local_to_capture,
            1,
            limits.max_buffer_size,
        )?);
        draws.push(body_draw(
            &body.mesh.dilation,
            body.local_to_capture,
            2,
            limits.max_buffer_size,
        )?);
    }
    let mut hash = Sha256::new();
    hash.update(CAPTURE_VERSION.to_le_bytes());
    hash.update(SHADER.as_bytes());
    hash.update(bytemuck::bytes_of(&camera));
    for d in &draws {
        if !d.indices.len().is_multiple_of(3)
            || d.indices.len() > u32::MAX as usize
            || d.vertices.len() as u64 > limits.max_buffer_size
            || std::mem::size_of_val(d.indices) as u64 > limits.max_buffer_size
            || d.indices
                .iter()
                .any(|i| *i as usize >= d.vertices.len() / 16)
        {
            return Err("invalid WaterInfo capture triangle indices or GPU buffer limits".into());
        }
        hash.update([
            d.pass as u8,
            match d.sides {
                Sides::TwoSided => 0,
                Sides::CounterClockwise => 1,
                Sides::Clockwise => 2,
            },
        ]);
        hash.update((d.vertices.len() as u64).to_le_bytes());
        hash.update(&d.vertices);
        hash.update((d.indices.len() as u64).to_le_bytes());
        hash.update(bytemuck::cast_slice(d.indices));
    }
    let identity = hash.finalize().into();
    let camera = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("native WaterInfo orthographic camera"),
        contents: bytemuck::bytes_of(&camera),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let texture = |label, format, layers, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: s.dimensions[0],
                height: s.dimensions[1],
                depth_or_array_layers: layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let depths = texture(
        "WaterInfo reverse-Z ground/water/dilation",
        wgpu::TextureFormat::Depth32Float,
        3,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
    );
    let colors = texture(
        "native DrawWaterInfo vertex-interpolated flow",
        wgpu::TextureFormat::Rgba32Float,
        1,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
    );
    let depth_view = depths.create_view(&Default::default());
    let color_view = colors.create_view(&Default::default());
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("native WaterInfo scene capture and DrawWaterInfo"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let uniform_entry = wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let raster_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("WaterInfo capture camera"),
        entries: &[uniform_entry],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("WaterInfo capture"),
        bind_group_layouts: &[Some(&raster_layout)],
        immediate_size: 0,
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("WaterInfo capture camera"),
        layout: &raster_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: camera.as_entire_binding(),
        }],
    });
    let mut pipelines = Vec::new();
    for color in [false, true] {
        for cull in [None, Some(wgpu::Face::Back), Some(wgpu::Face::Front)] {
            pipelines.push(
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("native WaterInfo mesh pass"),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("capture_vertex"),
                        compilation_options: Default::default(),
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: 16,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &wgpu::vertex_attr_array![0=>Float32x3,1=>Unorm8x4],
                        })],
                    },
                    primitive: wgpu::PrimitiveState {
                        front_face: wgpu::FrontFace::Cw,
                        cull_mode: cull,
                        ..Default::default()
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: wgpu::TextureFormat::Depth32Float,
                        depth_write_enabled: Some(true),
                        depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                        stencil: Default::default(),
                        bias: Default::default(),
                    }),
                    multisample: Default::default(),
                    fragment: if color {
                        Some(wgpu::FragmentState {
                            module: &shader,
                            entry_point: Some("capture_flow"),
                            compilation_options: Default::default(),
                            targets: &[Some(wgpu::ColorTargetState {
                                format: wgpu::TextureFormat::Rgba32Float,
                                blend: None,
                                write_mask: wgpu::ColorWrites::ALL,
                            })],
                        })
                    } else {
                        None
                    },
                    multiview_mask: None,
                    cache: None,
                }),
            );
        }
    }
    let uploaded: Vec<_> = draws
        .iter()
        .filter(|d| !d.indices.is_empty())
        .map(|d| {
            (
                d,
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("WaterInfo mesh vertices"),
                    contents: &d.vertices,
                    usage: wgpu::BufferUsages::VERTEX,
                }),
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("WaterInfo mesh indices"),
                    contents: bytemuck::cast_slice(d.indices),
                    usage: wgpu::BufferUsages::INDEX,
                }),
            )
        })
        .collect();
    for layer in 0..3 {
        let view = depths.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2),
            base_array_layer: layer,
            array_layer_count: Some(1),
            ..Default::default()
        });
        let attachment = Some(wgpu::RenderPassColorAttachment {
            view: &color_view,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native WaterInfo ground/colour/dilation capture"),
            color_attachments: if layer == 1 {
                std::slice::from_ref(&attachment)
            } else {
                &[]
            },
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pass.set_bind_group(0, &group, &[]);
        for (d, vertices, indices) in &uploaded {
            if d.pass != layer as usize {
                continue;
            }
            let side = match d.sides {
                Sides::TwoSided => 0,
                Sides::CounterClockwise => 1,
                Sides::Clockwise => 2,
            };
            pass.set_pipeline(&pipelines[if layer == 1 { 3 + side } else { side }]);
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..d.indices.len() as u32, 0, 0..1);
        }
    }
    let raw = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("GPU resolved WaterInfo capture pixels"),
        size: u64::from(s.dimensions[0]) * u64::from(s.dimensions[1]) * 32,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    let resolve_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("native WaterInfo point resolve"),
        entries: &[
            uniform_entry,
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                count: None,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                count: None,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::COMPUTE,
                count: None,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            },
        ],
    });
    let resolve_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("native WaterInfo point resolve"),
        layout: &resolve_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: camera.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&depth_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&color_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: raw.as_entire_binding(),
            },
        ],
    });
    let resolve_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("WaterInfo scene depth resolve"),
        layout: Some(
            &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[Some(&resolve_layout)],
                immediate_size: 0,
            }),
        ),
        module: &shader,
        entry_point: Some("resolve"),
        compilation_options: Default::default(),
        cache: None,
    });
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&resolve_pipeline);
        pass.set_bind_group(0, &resolve_group, &[]);
        pass.dispatch_workgroups(s.dimensions[0].div_ceil(8), s.dimensions[1].div_ceil(8), 1);
    }
    river_water_info::encode_buffer(device, encoder, s, &raw, identity)
}
