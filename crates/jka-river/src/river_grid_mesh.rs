//! Reset-only native river grid geometry. Water displacement/shading are
//! separate material operations and are deliberately not baked into positions.
use jka_water::river::grid_mesh::{GridMesh, SHADER, VERTEX_STRIDE};
use wgpu::util::DeviceExt;

pub struct Mesh {
    pub geometry: GridMesh,
    pub vertices: wgpu::Buffer,
    pub indices: wgpu::Buffer,
}

pub fn vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
        0 => Float32x3, 1 => Snorm8x4, 2 => Snorm8x4, 3 => Float32x2, 4 => Unorm8x4
    ];
    wgpu::VertexBufferLayout {
        array_stride: VERTEX_STRIDE,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &ATTRIBUTES,
    }
}

/// Allocate and initialize a single native material section. Call on reset,
/// retain across hydraulic frames and freeze; no per-frame mesh regeneration.
/// No CPU vertex generation/upload, readback or duplicated mesh streams.
pub fn encode(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    geometry: GridMesh,
) -> Result<Mesh, String> {
    let limits = device.limits();
    if [geometry.vertex_bytes(), geometry.index_bytes()]
        .iter()
        .any(|n| {
            *n > limits.max_buffer_size || *n > u64::from(limits.max_storage_buffer_binding_size)
        })
        || geometry
            .vertex_dimensions()
            .iter()
            .any(|n| n.div_ceil(8) > limits.max_compute_workgroups_per_dimension)
    {
        return Err("native river mesh exceeds GPU buffer/dispatch limits".into());
    }
    let vertices = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("native river grid vertices"),
        size: geometry.vertex_bytes(),
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::VERTEX
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let indices = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("native river grid indices"),
        size: geometry.index_bytes(),
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::INDEX
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let cells = geometry.cells();
    let dimensions = geometry.vertex_dimensions();
    let size = geometry.size();
    let words = [
        cells[0],
        cells[1],
        dimensions[0],
        dimensions[1],
        size[0].to_bits(),
        size[1].to_bits(),
        0,
        0,
    ];
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("native river mesh reset controls"),
        contents: bytemuck::cast_slice(&words),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("native river mesh reset layout"),
        entries: &std::array::from_fn::<_, 3, _>(|i| wgpu::BindGroupLayoutEntry {
            binding: i as u32,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: if i == 0 {
                    wgpu::BufferBindingType::Uniform
                } else {
                    wgpu::BufferBindingType::Storage { read_only: false }
                },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }),
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("native river mesh reset bindings"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: vertices.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: indices.as_entire_binding(),
            },
        ],
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("native river mesh reset stages"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("native river mesh reset pipeline layout"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    for (entry, dimensions) in [("create_vertices", dimensions), ("create_triangles", cells)] {
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            cache: None,
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some(entry),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(dimensions[0].div_ceil(8), dimensions[1].div_ceil(8), 1);
    }
    Ok(Mesh {
        geometry,
        vertices,
        indices,
    })
}
