//! Full-resolution sun shadow tracing, temporally rate-limited by a rotating
//! 2x2 dither schedule instead of spatial downsampling.
//!
//! A prior implementation traced sun and local-light visibility into a
//! quarter-pixel-count cache and spatially reconstructed it for every
//! full-resolution receiver. Measurement showed that reconstruction cost
//! (an unconditional per-fragment neighbor scan, paid even where no light was
//! nearby) plus a depth prepass forced only for that mode outweighed the rays
//! it saved. This version keeps the same target ray-count reduction (each
//! pixel is freshly traced once every 4 frames) but pays for it with a
//! same-pixel history lookup instead of a spatial search, and removes local
//! lights from the scheme entirely: they are already gated by actual cluster
//! membership in the ordinary ray-traced path, so caching them only added
//! unconditional overhead without a matching miss-rate problem to fix.
use super::*;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RtSunShadowSettings {
    inverse_view_proj: [[f32; 4]; 4],
    dimensions: [u32; 4], // width, height, frame index, dither phase (unused, derived in shader)
    flags: [u32; 4],      // enabled, history valid, reserved, reserved
    prev_camera_pos: [f32; 4], // previous frame's eye, to predict where a history texel's distance should land
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RtShadowReceiverFlags {
    flags: [u32; 4], // enabled, reserved, reserved, reserved
}

pub(super) struct RtSunShadowHistory {
    uniform: wgpu::Buffer,
    pub flags_uniform: wgpu::Buffer,
    pub published: wgpu::TextureView,
    _published_texture: wgpu::Texture,
    _ping_textures: [wgpu::Texture; 2],
    ping_views: [wgpu::TextureView; 2],
    dimensions: [u32; 2],
    read_index: usize,
    history_valid: bool,
    frame_index: u32,
    enabled: bool,
    compute: Option<ComputeResources>,
}

struct ComputeResources {
    input_layout: wgpu::BindGroupLayout,
    scene_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    inputs: [Option<wgpu::BindGroup>; 2],
    scene: Option<wgpu::BindGroup>,
    scene_generation: u64,
}

/// Each texel is (visibility, camera distance of the surface it was traced for).
/// Distance replaces the old full world position: it is what history and the
/// receivers actually validate against, at half the bytes of Rgba32Float and
/// with no per-frame copy of the published result.
const CACHE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg32Float;

fn target(
    device: &wgpu::Device,
    size: [u32; 2],
    label: &str,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: CACHE_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

impl RtSunShadowHistory {
    pub fn new(device: &wgpu::Device) -> Self {
        let (t0, v0) = target(device, [1, 1], "RT sun shadow ping 0");
        let (t1, v1) = target(device, [1, 1], "RT sun shadow ping 1");
        let (published_texture, published) = target(device, [1, 1], "RT sun shadow (published)");
        Self {
            uniform: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("RT sun shadow settings"),
                contents: bytemuck::bytes_of(&RtSunShadowSettings::zeroed()),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            }),
            flags_uniform: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("RT sun shadow receiver flags"),
                contents: bytemuck::bytes_of(&RtShadowReceiverFlags::zeroed()),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            }),
            published,
            _published_texture: published_texture,
            _ping_textures: [t0, t1],
            ping_views: [v0, v1],
            dimensions: [1, 1],
            read_index: 0,
            history_valid: false,
            frame_index: 0,
            enabled: false,
            compute: None,
        }
    }

    fn resize(&mut self, device: &wgpu::Device, size: [u32; 2]) -> bool {
        if self.dimensions == size {
            return false;
        }
        self.dimensions = size;
        let (t0, v0) = target(device, size, "RT sun shadow ping 0");
        let (t1, v1) = target(device, size, "RT sun shadow ping 1");
        self._ping_textures = [t0, t1];
        self.ping_views = [v0, v1];
        let (published_texture, published) = target(device, size, "RT sun shadow (published)");
        self._published_texture = published_texture;
        self.published = published;
        self.read_index = 0;
        self.history_valid = false;
        self.frame_index = 0;
        if let Some(compute) = self.compute.as_mut() {
            compute.inputs = [None, None];
        }
        true
    }
}

pub(super) fn receiver_entries() -> [wgpu::BindGroupLayoutEntry; 2] {
    [
        uniform_entry(15, wgpu::ShaderStages::FRAGMENT),
        unfilterable_texture_entry_stages(16, wgpu::ShaderStages::FRAGMENT),
    ]
}

impl Renderer {
    pub(super) fn invalidate_rt_sun_shadow_depth(&mut self) {
        if let Some(rt) = self.ray_traced_shadows.as_mut() {
            if let Some(compute) = rt.sun_shadow_history.compute.as_mut() {
                compute.inputs = [None, None];
            }
        }
    }

    /// The cache only serves the sun. With RT local lights alone the receivers
    /// never read it (their `ENABLE_RAY_TRACED_SUN` override is off), so tracing
    /// it, and forcing the depth prepass for it, would be pure waste.
    pub(super) fn rt_sun_cache_wanted(&self) -> bool {
        self.rt_reduced_shadows
            && self
                .world
                .as_ref()
                .is_some_and(|world| world.active_pipeline_variant.ray_traced_sun)
    }

    pub(super) fn prepare_rt_sun_shadow(&mut self, view_proj: Mat4) {
        let enabled = self.rt_sun_cache_wanted();
        let previous_eye = self.previous_camera_position;
        let width = self.config.width.max(1);
        let height = self.config.height.max(1);
        let Some(rt) = self.ray_traced_shadows.as_mut() else {
            return;
        };
        let history = &mut rt.sun_shadow_history;
        if !enabled && !history.enabled {
            return;
        }
        let just_enabled = enabled && !history.enabled;
        let resized = enabled && history.resize(&self.device, [width, height]);
        if just_enabled {
            history.history_valid = false;
            history.frame_index = 0;
        }
        let settings = RtSunShadowSettings {
            inverse_view_proj: view_proj.inverse().to_cols_array_2d(),
            dimensions: [width, height, history.frame_index, 0],
            flags: [u32::from(enabled), u32::from(history.history_valid), 0, 0],
            prev_camera_pos: previous_eye.extend(0.0).to_array(),
        };
        self.queue
            .write_buffer(&history.uniform, 0, bytemuck::bytes_of(&settings));
        self.queue.write_buffer(
            &history.flags_uniform,
            0,
            bytemuck::bytes_of(&RtShadowReceiverFlags {
                flags: [u32::from(enabled), 0, 0, 0],
            }),
        );
        history.enabled = enabled;
        if resized {
            self.rebuild_weather_surface_receiver_bind_group();
        }
    }

    pub(super) fn encode_rt_sun_shadow(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if !self.rt_sun_cache_wanted() {
            return;
        }
        let Some(rt) = self.ray_traced_shadows.as_mut() else {
            return;
        };
        let history = &mut rt.sun_shadow_history;
        if !history.enabled {
            return;
        }
        if history.compute.is_none() {
            history.compute = Some(ComputeResources::new(&self.device, &self.camera_layout));
        }
        let compute = history.compute.as_mut().unwrap();
        if compute.inputs[0].is_none() {
            compute.inputs[0] = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("RT sun shadow inputs (read 0)"),
                layout: &compute.input_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: history.uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(
                            &self.targets.linear_depth_render_view,
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(
                            &self.targets.motion_vector_view,
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&history.ping_views[0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&history.ping_views[1]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: self.lighting_settings_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: self.shadow_resources.receiver_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(&history.published),
                    },
                ],
            }));
            compute.inputs[1] = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("RT sun shadow inputs (read 1)"),
                layout: &compute.input_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: history.uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(
                            &self.targets.linear_depth_render_view,
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(
                            &self.targets.motion_vector_view,
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&history.ping_views[1]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&history.ping_views[0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: self.lighting_settings_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: self.shadow_resources.receiver_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(&history.published),
                    },
                ],
            }));
        }
        if compute.scene.is_none() || compute.scene_generation != rt.alpha_buffers_generation {
            compute.scene = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("RT sun shadow scene"),
                layout: &compute.scene_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: rt._tlas.as_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 9,
                        resource: rt.alpha_vertex_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 10,
                        resource: rt.alpha_index_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 11,
                        resource: rt.alpha_geometry_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 12,
                        resource: rt.alpha_material_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 13,
                        resource: rt.alpha_texture_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 14,
                        resource: rt.alpha_texel_buffer.as_entire_binding(),
                    },
                ],
            }));
            compute.scene_generation = rt.alpha_buffers_generation;
        }
        let write_index = 1 - history.read_index;
        self.gpu_profiler
            .write_encoder_timestamp(encoder, GpuPass::RtVisibility, false);
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("RT sun shadow"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&compute.pipeline);
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            pass.set_bind_group(1, compute.inputs[history.read_index].as_ref().unwrap(), &[]);
            pass.set_bind_group(2, compute.scene.as_ref().unwrap(), &[]);
            pass.dispatch_workgroups(
                history.dimensions[0].div_ceil(8),
                history.dimensions[1].div_ceil(8),
                1,
            );
        }
        self.gpu_profiler
            .write_encoder_timestamp(encoder, GpuPass::RtVisibility, true);
        history.read_index = write_index;
        history.history_valid = true;
        history.frame_index = history.frame_index.wrapping_add(1);
    }
}

impl ComputeResources {
    fn new(device: &wgpu::Device, camera: &wgpu::BindGroupLayout) -> Self {
        let storage_texture = |binding, format| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            count: None,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format,
                view_dimension: wgpu::TextureViewDimension::D2,
            },
        };
        let input_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("RT sun shadow inputs"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::COMPUTE),
                unfilterable_texture_entry_stages(1, wgpu::ShaderStages::COMPUTE),
                unfilterable_texture_entry_stages(2, wgpu::ShaderStages::COMPUTE),
                unfilterable_texture_entry_stages(3, wgpu::ShaderStages::COMPUTE),
                storage_texture(4, CACHE_FORMAT),
                uniform_entry(5, wgpu::ShaderStages::COMPUTE),
                uniform_entry(6, wgpu::ShaderStages::COMPUTE),
                storage_texture(7, CACHE_FORMAT),
            ],
        });
        let mut scene_entries = rt_alpha_storage_entries().to_vec();
        scene_entries.push(wgpu::BindGroupLayoutEntry {
            binding: 8,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::AccelerationStructure {
                vertex_return: false,
            },
            count: None,
        });
        for entry in &mut scene_entries {
            entry.visibility = wgpu::ShaderStages::COMPUTE;
        }
        let scene_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("RT sun shadow scene"),
            entries: &scene_entries,
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("RT sun shadow compute"),
            bind_group_layouts: &[Some(camera), Some(&input_layout), Some(&scene_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("RT sun shadow compute"),
            source: wgpu::ShaderSource::Wgsl(
                compute_shader_source()
                    .expect("RT sun shadow anchors")
                    .into(),
            ),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("RT sun shadow"),
            layout: Some(&layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self {
            input_layout,
            scene_layout,
            pipeline,
            inputs: [None, None],
            scene: None,
            scene_generation: u64::MAX,
        }
    }
}

fn compute_shader_source() -> Result<String, String> {
    let donor = ray_traced_world_shader_source(include_str!("../../bsp.wgsl"))?;
    let declaration = rt_models::declaration;
    let mut source = String::from("enable wgpu_ray_query;\n");
    for name in ["Camera", "ShadowSettings", "LightingSettings"] {
        source.push_str(declaration(&donor, &format!("struct {name} "))?);
        source.push_str(";\n");
    }
    source.push_str("struct VertexOut { clip_position: vec4<f32>, world_position: vec3<f32> };\n");
    let start = donor
        .find("@group(3) @binding(8)")
        .ok_or("RT scene binding missing")?;
    let last = donor
        .find("@group(3) @binding(14)")
        .ok_or("RT alpha binding missing")?;
    let end = last
        + donor[last..]
            .find(';')
            .ok_or("RT alpha binding terminator missing")?
        + 1;
    source.push_str(&donor[start..end].replace("@group(3)", "@group(2)"));
    source.push_str("\nconst ENABLE_RAY_TRACED_SUN: bool = true;\n");
    for name in [
        "rt_rand_f",
        "rt_rand_vec2f",
        "rt_sample_count",
        "rt_copysign",
        "rt_orthonormalize",
        "rt_sample_directional_emitter",
        "rt_alpha_repeat_coord",
        "rt_alpha_texel",
        "rt_sample_alpha",
        "rt_alpha_generated_uv",
        "rt_alpha_candidate_blocks",
        "rt_trace_shadow_visibility",
        "rt_uncached_sun_visibility",
    ] {
        source.push_str(declaration(&donor, &format!("fn {name}("))?);
        source.push('\n');
    }
    source.push_str(include_str!("../../rt_resolution_common.wgsl"));
    source.push_str(include_str!("../../rt_resolution_trace.wgsl"));
    Ok(source)
}

#[cfg(test)]
pub(super) fn validate_gpu(device: &wgpu::Device, queue: &wgpu::Queue) {
    // Exercise a real occluder BLAS/TLAS end to end: dispatch the trace pass at
    // odd viewport dimensions, publish the result, and read it back through the
    // same lookup the material shader uses.
    let size = [7u32, 5u32];
    let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[uniform_entry(
            0,
            wgpu::ShaderStages::COMPUTE | wgpu::ShaderStages::FRAGMENT,
        )],
    });
    let compute = ComputeResources::new(device, &camera_layout);
    let mut history = RtSunShadowHistory::new(device);
    history.resize(device, size);
    let eye = Vec3::new(0.0, 0.0, 4.0);
    let vp = Mat4::perspective_rh(90.0_f32.to_radians(), 7.0 / 5.0, 0.1, 100.0)
        * Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y);
    // history_valid = 0 forces every pixel to trace fresh this dispatch, since
    // the ping-pong textures start out uninitialized.
    let settings = RtSunShadowSettings {
        inverse_view_proj: vp.inverse().to_cols_array_2d(),
        dimensions: [7, 5, 0, 0],
        flags: [1, 0, 0, 0],
        prev_camera_pos: eye.extend(0.0).to_array(),
    };
    queue.write_buffer(&history.uniform, 0, bytemuck::bytes_of(&settings));
    queue.write_buffer(
        &history.flags_uniform,
        0,
        bytemuck::bytes_of(&RtShadowReceiverFlags {
            flags: [1, 0, 0, 0],
        }),
    );
    let mut camera = CameraUniform::zeroed();
    camera.view_proj = vp.to_cols_array_2d();
    camera.camera_pos_time = eye.extend(0.0).to_array();
    let buffer = |bytes: &[u8], usage| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytes,
            usage,
        })
    };
    let camera_buffer = buffer(bytemuck::bytes_of(&camera), wgpu::BufferUsages::UNIFORM);
    let camera_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &camera_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: camera_buffer.as_entire_binding(),
        }],
    });
    let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 7,
            height: 5,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut depths = Vec::new();
    let mut positions = Vec::<[f32; 4]>::new();
    for y in 0..size[1] {
        for x in 0..size[0] {
            let h = vp.inverse()
                * Vec4::new(
                    (x as f32 + 0.5) / 7.0 * 2.0 - 1.0,
                    1.0 - (y as f32 + 0.5) / 5.0 * 2.0,
                    0.5,
                    1.0,
                );
            let dir = (h.truncate() / h.w - eye).normalize();
            depths.push(-eye.z / dir.z);
            positions.push((eye + dir * (-eye.z / dir.z)).extend(0.0).to_array());
        }
    }
    queue.write_texture(
        depth_texture.as_image_copy(),
        bytemuck::cast_slice(&depths),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(7 * 4),
            rows_per_image: Some(5),
        },
        depth_texture.size(),
    );
    let depth_view = depth_texture.create_view(&Default::default());
    let motion_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 7,
            height: 5,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rg16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let motion_view = motion_texture.create_view(&Default::default());
    let mut sun = ShadowReceiverUniform::zeroed();
    sun.light_direction_enabled = [0.0, 0.0, -1.0, 1.0];
    sun.bevy_params[0] = 1.0; // zero angular radius for a deterministic oracle
    let sun = buffer(bytemuck::bytes_of(&sun), wgpu::BufferUsages::UNIFORM);
    let mut lighting = LightingSettings::zeroed();
    lighting.map_ambient[3] = 1.0; // one deterministic sample
    let lighting = buffer(bytemuck::bytes_of(&lighting), wgpu::BufferUsages::UNIFORM);
    let inputs = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &compute.input_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: history.uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&depth_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&motion_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&history.ping_views[0]),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(&history.ping_views[1]),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: lighting.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: sun.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(&history.published),
            },
        ],
    });
    let blocker = create_rt_triangle_blas(
        device,
        "RT sun shadow test blocker",
        &[[0.0, -50.0, 2.0], [50.0, -50.0, 2.0], [0.0, 50.0, 2.0]],
        &[0, 1, 2],
        false,
    )
    .unwrap();
    let mut tlas = device.create_tlas(&wgpu::CreateTlasDescriptor {
        label: None,
        max_instances: 1,
        flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
        update_mode: wgpu::AccelerationStructureUpdateMode::Build,
    });
    tlas[0] = Some(wgpu::TlasInstance::new(
        &blocker.blas,
        [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        0,
        255,
    ));
    let alpha = [
        create_rt_alpha_buffer::<RtAlphaVertexGpu>(
            device,
            "test",
            &[],
            wgpu::BufferUsages::empty(),
        ),
        create_rt_alpha_buffer::<u32>(device, "test", &[], wgpu::BufferUsages::empty()),
        create_rt_alpha_buffer::<RtAlphaGeometryGpu>(
            device,
            "test",
            &[],
            wgpu::BufferUsages::empty(),
        ),
        create_rt_alpha_buffer::<RtAlphaMaterialGpu>(
            device,
            "test",
            &[],
            wgpu::BufferUsages::empty(),
        ),
        create_rt_alpha_buffer::<RtAlphaTextureGpu>(
            device,
            "test",
            &[],
            wgpu::BufferUsages::empty(),
        ),
        create_rt_alpha_buffer::<u32>(device, "test", &[], wgpu::BufferUsages::empty()),
    ];
    let mut scene_entries = vec![wgpu::BindGroupEntry {
        binding: 8,
        resource: tlas.as_binding(),
    }];
    scene_entries.extend(alpha.iter().enumerate().map(|(i, b)| wgpu::BindGroupEntry {
        binding: i as u32 + 9,
        resource: b.as_entire_binding(),
    }));
    let scene = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &compute.scene_layout,
        entries: &scene_entries,
    });

    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.build_acceleration_structures([rt_triangle_blas_build_entry(&blocker)].iter(), [&tlas]);
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&compute.pipeline);
        pass.set_bind_group(0, &camera_group, &[]);
        pass.set_bind_group(1, &inputs, &[]);
        pass.set_bind_group(2, &scene, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }

    let donor = ray_traced_world_shader_source(include_str!("../../bsp.wgsl")).unwrap();
    let mut probe =
        String::from("struct VertexOut { clip_position: vec4<f32>, world_position: vec3<f32> };\n");
    probe.push_str(rt_models::declaration(&donor, "struct Camera ").unwrap());
    probe.push_str(";\n");
    probe.push_str("\n@group(0) @binding(0) var<uniform> camera: Camera;\n@group(1) @binding(0) var<storage, read_write> output: array<vec4<f32>>;\n@group(1) @binding(1) var<storage, read> positions: array<vec4<f32>>;\n");
    let reader = include_str!("../../rt_resolution_read.wgsl");
    probe.push_str(&reader[..reader.find("fn rt_cached_sun_visibility").unwrap()]);
    probe.push_str(rt_models::declaration(reader, "fn rt_cached_sun_visibility(").unwrap());
    probe.push_str(r#"
@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = id.y * 7u + id.x;
    let input = VertexOut(vec4<f32>(vec2<f32>(id.xy) + vec2<f32>(0.5), 0.0, 1.0), positions[index].xyz);
    var stray = input;
    stray.world_position += vec3<f32>(0.0, 0.0, 50.0);
    // x: what a receiver on the recorded surface sees; y: a receiver that is
    // not on it (ocean over seabed, MSAA edge) must be rejected with -1.
    output[index] = vec4<f32>(rt_cached_sun_visibility(input), rt_cached_sun_visibility(stray), 0.0, 0.0);
}
"#);
    let storage_entry = |binding, read_only| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        count: None,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
    };
    let storage_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[storage_entry(0, false), storage_entry(1, true)],
    });
    let empty = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[],
    });
    let mut entries = receiver_entries();
    for e in &mut entries {
        e.visibility = wgpu::ShaderStages::COMPUTE;
    }
    let read_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &entries,
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[
            Some(&camera_layout),
            Some(&storage_layout),
            Some(&empty),
            Some(&read_layout),
        ],
        immediate_size: 0,
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("RT sun shadow reconstruction test"),
        source: wgpu::ShaderSource::Wgsl(probe.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 35 * 16,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 35 * 16,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let positions = buffer(
        bytemuck::cast_slice(&positions),
        wgpu::BufferUsages::STORAGE,
    );
    let output_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &storage_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: output.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: positions.as_entire_binding(),
            },
        ],
    });
    let empty_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &empty,
        entries: &[],
    });
    let read_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &read_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 15,
                resource: history.flags_uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 16,
                resource: wgpu::BindingResource::TextureView(&history.published),
            },
        ],
    });
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &camera_group, &[]);
        pass.set_bind_group(1, &output_group, &[]);
        pass.set_bind_group(2, &empty_group, &[]);
        pass.set_bind_group(3, &read_group, &[]);
        pass.dispatch_workgroups(7, 5, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, 35 * 16);
    let submission = queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(std::time::Duration::from_secs(30)),
        })
        .unwrap();
    rx.recv().unwrap().unwrap();
    let data = readback.slice(..).get_mapped_range();
    let pixels = bytemuck::cast_slice::<u8, [f32; 4]>(&data);
    // The blocker triangle covers the right half of the view at this row;
    // column 0 sees past it, column 4 does not (same geometry as the fixture
    // this test was adapted from).
    assert_eq!(pixels[2 * 7][0], 1.0, "unoccluded receiver");
    assert!(pixels[2 * 7 + 4][0] < 0.5, "occluded receiver");
    for (i, pixel) in pixels.iter().enumerate() {
        assert_eq!(
            pixel[1], -1.0,
            "receiver off the cached surface must fall back to a direct trace (pixel {i})"
        );
    }
    drop(data);
    readback.unmap();

    // Second frame: slide the blocker out of view and reuse history. The
    // occluded interior must be carried (history is trusted where a whole
    // neighbourhood agrees), the scheduled dither cell for this phase must trace
    // fresh, and the edge of the old shadow must not be carried.
    tlas[0] = Some(wgpu::TlasInstance::new(
        &blocker.blas,
        [
            1.0, 0.0, 0.0, 1000.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0,
        ],
        0,
        255,
    ));
    let settings = RtSunShadowSettings {
        inverse_view_proj: vp.inverse().to_cols_array_2d(),
        dimensions: [7, 5, 1, 0],
        flags: [1, 1, 0, 0],
        prev_camera_pos: eye.extend(0.0).to_array(),
    };
    queue.write_buffer(&history.uniform, 0, bytemuck::bytes_of(&settings));
    let inputs_next = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &compute.input_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: history.uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&depth_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&motion_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&history.ping_views[1]),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(&history.ping_views[0]),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: lighting.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: sun.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(&history.published),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.build_acceleration_structures([rt_triangle_blas_build_entry(&blocker)].iter(), [&tlas]);
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&compute.pipeline);
        pass.set_bind_group(0, &camera_group, &[]);
        pass.set_bind_group(1, &inputs_next, &[]);
        pass.set_bind_group(2, &scene, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &camera_group, &[]);
        pass.set_bind_group(1, &output_group, &[]);
        pass.set_bind_group(2, &empty_group, &[]);
        pass.set_bind_group(3, &read_group, &[]);
        pass.dispatch_workgroups(7, 5, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, 35 * 16);
    let submission = queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(std::time::Duration::from_secs(30)),
        })
        .unwrap();
    rx.recv().unwrap().unwrap();
    let data = readback.slice(..).get_mapped_range();
    let pixels = bytemuck::cast_slice::<u8, [f32; 4]>(&data);
    assert_eq!(
        pixels[2 * 7 + 5][0],
        0.0,
        "uniformly shadowed history away from an edge is carried, not retraced"
    );
    assert_eq!(
        pixels[7 + 5][0],
        1.0,
        "the scheduled dither cell traces fresh against the current scene"
    );
    assert_eq!(
        pixels[2 * 7 + 3][0],
        1.0,
        "a shadow edge is retraced every frame, so the departed caster is gone there"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rt_sun_shadow_compute_validates_and_emits_spirv() {
        use wgpu::naga;
        let source = compute_shader_source().unwrap();
        let module = naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
        naga::back::spv::write_vec(
            &module,
            &info,
            &naga::back::spv::Options {
                lang_version: (1, 4),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    }
}
