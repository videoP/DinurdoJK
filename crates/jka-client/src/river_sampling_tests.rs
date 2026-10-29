//! Independent hardware-filter oracle for the CPU hydraulic reference. This
//! shader performs only SampleLevel, without the production cubic/branch logic.
use jka_water::river::Water;
use wgpu::util::DeviceExt;

pub(super) struct Oracle<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
}

impl<'a> Oracle<'a> {
    pub(super) fn new(device: &'a wgpu::Device, queue: &'a wgpu::Queue) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("independent R32f SampleLevel oracle"),
            source: wgpu::ShaderSource::Wgsl(r#"
                @group(0) @binding(0) var field: texture_2d_array<f32>;
                @group(0) @binding(1) var bilinear_clamp: sampler;
                @group(0) @binding(2) var<storage, read> queries: array<vec4<f32>>;
                @group(0) @binding(3) var<storage, read_write> result: array<vec4<f32>>;
                @compute @workgroup_size(64)
                fn probe(@builtin(global_invocation_id) id: vec3<u32>) {
                    if id.x >= arrayLength(&queries) { return; }
                    let q = queries[id.x];
                    result[id.x] = vec4<f32>(textureSampleLevel(field, bilinear_clamp, q.xy, i32(q.z), 0.0).x, 0.0, 0.0, 0.0);
                }
            "#.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("independent R32f SampleLevel oracle"),
            layout: None,
            module: &shader,
            entry_point: Some("probe"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self {
            device,
            queue,
            pipeline,
        }
    }

    pub(super) fn samples(
        &self,
        dimensions: [usize; 2],
        water: &[Water],
        queries: &[[f32; 4]],
    ) -> Vec<f32> {
        if queries.is_empty() {
            return Vec::new();
        }
        let device = self.device;
        let queue = self.queue;
        // Deliberately use independent queue uploads rather than the production
        // planar staging/first-encoder path: detect attribute/row packing errors.
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("independent R32f reference attributes"),
            size: wgpu::Extent3d {
                width: dimensions[0] as u32,
                height: dimensions[1] as u32,
                depth_or_array_layers: 4,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for channel in 0..4 {
            let data: Vec<_> = water
                .iter()
                .flat_map(|w| {
                    [w.height, w.depth, w.velocity[0], w.velocity[1]][channel].to_le_bytes()
                })
                .collect();
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: channel as u32,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(dimensions[0] as u32 * 4),
                    rows_per_image: Some(dimensions[1] as u32),
                },
                wgpu::Extent3d {
                    width: dimensions[0] as u32,
                    height: dimensions[1] as u32,
                    depth_or_array_layers: 1,
                },
            );
        }
        let view = texture.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let query_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("reference UV/layer queries"),
            contents: bytemuck::cast_slice(queries),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let result = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("reference hardware-filtered values"),
            size: queries.len() as u64 * 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("independent sampling inputs"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: query_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: result.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups((queries.len() as u32).div_ceil(64), 1, 1);
        }
        queue.submit(Some(encoder.finish()));
        super::read_buffer(device, queue, &result)
            .into_iter()
            .map(|v| v[0])
            .collect()
    }

    pub(super) fn check_exact_centres_edges_and_layers(&self) {
        let water: Vec<_> = (0..64)
            .map(|i| Water {
                height: i as f32,
                depth: 1024.0 + i as f32 * 2.0,
                velocity: [-512.0 + i as f32 * 3.0, 100000.0 + i as f32 * 4.0],
            })
            .collect();
        let mut queries = Vec::new();
        let mut expected = Vec::new();
        for channel in 0..4 {
            let value = |i: usize| {
                let w = water[i];
                [w.height, w.depth, w.velocity[0], w.velocity[1]][channel]
            };
            for (uv, indices) in [
                ([-1.0, -2.0], [0; 4]),
                ([2.0, 3.0], [63; 4]),
                ([(3.0 + 0.5) / 8.0, (4.0 + 0.5) / 8.0], [35; 4]),
                ([2.0 / 8.0, 3.0 / 8.0], [17, 18, 25, 26]),
            ] {
                queries.push([uv[0], uv[1], channel as f32, 0.0]);
                expected.push(indices.map(value).iter().sum::<f32>() * 0.25);
            }
        }
        assert_eq!(self.samples([8, 8], &water, &queries), expected);
    }
}
