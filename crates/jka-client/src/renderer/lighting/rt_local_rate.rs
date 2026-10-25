//! Full-resolution, reduced-rate scalar local-light visibility. Four sample
//! bitmasks cover all 32 clustered lights without a radiance/BRDF approximation.
use super::*;
use crate::renderer::lighting::ray_tracing::rt_alpha_storage_entry;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Settings {
    inverse_view_proj: [[f32; 4]; 4],
    dimensions: [u32; 4],
    flags: [u32; 4],
    prev_camera_pos: [f32; 4],
}

pub(super) struct RtLocalRateHistory {
    textures: [wgpu::Texture; 3],
    views: [wgpu::TextureView; 2],
    pub published: wgpu::TextureView,
    uniform: wgpu::Buffer,
    size: [u32; 2],
    layers: u32,
    read: usize,
    valid: bool,
    enabled: bool,
    frame: u32,
    generation: u64,
    compute: Option<Compute>,
}

pub(super) fn receiver_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 17,
        visibility: wgpu::ShaderStages::FRAGMENT,
        count: None,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Uint,
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        },
    }
}

fn target(device: &wgpu::Device, size: [u32; 2], layers: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("RT local visibility masks and metadata"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: layers,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Uint,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::STORAGE_BINDING
            | if cfg!(test) {
                wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST
            } else {
                wgpu::TextureUsages::empty()
            },
        view_formats: &[],
    })
}

impl RtLocalRateHistory {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let textures = std::array::from_fn(|_| target(device, [1, 1], 1));
        let views = std::array::from_fn(|i| {
            textures[i].create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
        });
        let published = textures[2].create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        Self {
            textures,
            views,
            published,
            uniform: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("RT local rate settings"),
                contents: bytemuck::bytes_of(&Settings::zeroed()),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            }),
            size: [1, 1],
            layers: 1,
            read: 0,
            valid: false,
            enabled: false,
            frame: 0,
            generation: u64::MAX,
            compute: None,
        }
    }

    pub(super) fn invalidate(&mut self) {
        self.valid = false;
    }

    fn resize(&mut self, device: &wgpu::Device, size: [u32; 2], samples: u32) -> bool {
        let layers = if samples > 1 { 2 } else { 1 };
        if self.size == size && self.layers == layers {
            return false;
        }
        self.textures = std::array::from_fn(|_| target(device, size, layers));
        self.views = std::array::from_fn(|i| {
            self.textures[i].create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
        });
        self.published = self.textures[2].create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        self.size = size;
        self.layers = layers;
        self.valid = false;
        true
    }
}

impl Renderer {
    pub(super) fn rt_local_cache_wanted(&self) -> bool {
        self.rt_trace_rate > 1
            && !self.tinted_glass_enabled
            && self.ray_tracing_supported
            && self.dynamic_lights_mode == DynamicLightsMode::RayTracedHardware
            && !self.asset_preview_mode
            && self.active_transient_light_count() > 0
            && self
                .world
                .as_ref()
                .is_some_and(|w| w.active_pipeline_variant.ray_traced_shadows)
    }

    pub(super) fn prepare_rt_local_rate(&mut self, view_proj: Mat4, eye: Vec3) {
        let enabled = self.rt_local_cache_wanted();
        let map_lights = self
            .world
            .as_ref()
            .is_some_and(|w| w.active_pipeline_variant.map_light_simulation);
        let Some(rt) = self.ray_traced_shadows.as_mut() else {
            return;
        };
        let h = &mut rt.sun_shadow_history.local;
        let resized = enabled
            && h.resize(
                &self.device,
                [self.config.width.max(1), self.config.height.max(1)],
                self.rt_samples,
            );
        if !enabled
            || !h.enabled
            || h.generation != rt.alpha_buffers_generation
            || eye.distance(self.previous_camera_position) > 96.0
        {
            h.valid = false;
        }
        h.enabled = enabled;
        h.generation = rt.alpha_buffers_generation;
        if enabled {
            self.queue.write_buffer(
                &h.uniform,
                0,
                bytemuck::bytes_of(&Settings {
                    inverse_view_proj: view_proj.inverse().to_cols_array_2d(),
                    dimensions: [h.size[0], h.size[1], h.frame, self.rt_trace_rate],
                    flags: [1, u32::from(h.valid), u32::from(map_lights), 0],
                    prev_camera_pos: self.previous_camera_position.extend(0.0).to_array(),
                }),
            );
        }
        if resized {
            self.rebuild_weather_surface_receiver_bind_group();
        }
    }

    pub(super) fn encode_rt_local_rate(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if !self.rt_local_cache_wanted() {
            return;
        }
        let Some(world) = self.world.as_ref() else {
            return;
        };
        let Some(rt) = self.ray_traced_shadows.as_mut() else {
            return;
        };
        let h = &mut rt.sun_shadow_history.local;
        if h.compute.is_none() {
            h.compute = Some(Compute::new(&self.device, &self.camera_layout));
        }
        let compute = h.compute.as_ref().unwrap();
        let inputs = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("RT local rate inputs"),
            layout: &compute.input_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: h.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(
                        &self.targets.linear_depth_render_view,
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&self.targets.motion_vector_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&h.views[h.read]),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&h.views[1 - h.read]),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: self.lighting_settings_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: world.light_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: world._cluster_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::TextureView(&h.published),
                },
            ],
        });
        let scene = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("RT local rate scene"),
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
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("RT local visibility update"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&compute.pipeline);
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            pass.set_bind_group(1, &inputs, &[]);
            pass.set_bind_group(2, &scene, &[]);
            pass.dispatch_workgroups(h.size[0].div_ceil(8), h.size[1].div_ceil(8), 1);
        }
        h.read = 1 - h.read;
        h.valid = true;
        h.frame = h.frame.wrapping_add(1);
    }
}

struct Compute {
    input_layout: wgpu::BindGroupLayout,
    scene_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
}

impl Compute {
    fn new(device: &wgpu::Device, camera: &wgpu::BindGroupLayout) -> Self {
        let uint_texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            count: None,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Uint,
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
        };
        let storage_texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            count: None,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: wgpu::TextureFormat::Rgba32Uint,
                view_dimension: wgpu::TextureViewDimension::D2Array,
            },
        };
        let mut light_entry = rt_alpha_storage_entry::<GpuPointLight>(6);
        light_entry.visibility = wgpu::ShaderStages::COMPUTE;
        // One count plus 32 indices: explicit layouts must advertise at least
        // the shader's complete first ClusterRecord, rather than one u32.
        let mut cluster_entry = rt_alpha_storage_entry::<[u32; 33]>(7);
        cluster_entry.visibility = wgpu::ShaderStages::COMPUTE;
        let input_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("RT local rate inputs"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::COMPUTE),
                unfilterable_texture_entry_stages(1, wgpu::ShaderStages::COMPUTE),
                unfilterable_texture_entry_stages(2, wgpu::ShaderStages::COMPUTE),
                uint_texture(3),
                storage_texture(4),
                uniform_entry(5, wgpu::ShaderStages::COMPUTE),
                light_entry,
                cluster_entry,
                storage_texture(8),
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
        for e in &mut scene_entries {
            e.visibility = wgpu::ShaderStages::COMPUTE;
        }
        let scene_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("RT local rate scene"),
            entries: &scene_entries,
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("RT local rate"),
            bind_group_layouts: &[Some(camera), Some(&input_layout), Some(&scene_layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("RT local rate"),
            source: wgpu::ShaderSource::Wgsl(
                shader_source()
                    .expect("RT local rate shader anchors")
                    .into(),
            ),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("RT local rate"),
            layout: Some(&layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: crate::pipeline_cache::get().as_ref(),
        });
        Self {
            input_layout,
            scene_layout,
            pipeline,
        }
    }
}

fn shader_source() -> Result<String, String> {
    let donor = ray_traced_world_shader_source(include_str!("../../bsp.wgsl"))?;
    let declaration = rt_models::declaration;
    let mut source = String::from("enable wgpu_ray_query;\n");
    for name in ["Camera", "LightingSettings", "PointLight", "ClusterRecord"] {
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
            .ok_or("RT alpha terminator missing")?
        + 1;
    source.push_str(&donor[start..end].replace("@group(3)", "@group(2)"));
    source.push_str("\nconst ENABLE_LOCAL_SHADOWS: bool = true;\n");
    for line in donor.lines().filter(|l| l.starts_with("const CLUSTER_")) {
        source.push_str(line);
        source.push('\n');
    }
    for name in [
        "rt_rand_f",
        "rt_sample_count",
        "rt_local_sample_count",
        "rt_emitter_seed",
        "rt_sample_local_emitter_seed",
        "rt_alpha_repeat_coord",
        "rt_rgba_texel",
        "rt_sample_rgba",
        "rt_sample_alpha",
        "rt_tc_stretch_wave",
        "rt_alpha_generated_uv",
        "rt_alpha_candidate_blocks",
        "rt_candidate_glass_throughput",
        "rt_trace_shadow_query",
        "rt_trace_shadow_query_mask",
        "rt_trace_shadow_visibility",
        "rt_trace_local_visibility",
        "ray_traced_local_shadow_visibility",
        "q3map_effective_distance",
        "q3map_surface_angle",
        "q3map_light_scalar",
        "q3map_fast_contribution_visible",
        "emitter_visibility",
        "cluster_for_fragment",
    ] {
        if name == "rt_candidate_glass_throughput"
            && !crate::renderer::lighting::rt_diagnostics::config()
                .receiver
                .glass
        {
            continue;
        }
        source.push_str(&declaration(&donor, &format!("fn {name}("))?.replace(
            "ENABLE_MAP_LIGHT_SIMULATION",
            "(rt_shadow_settings.flags.z != 0u)",
        ));
        source.push('\n');
    }
    source.push_str(include_str!("../../rt_resolution_common.wgsl"));
    source.push_str(include_str!("../../rt_local_rate_surface.wgsl"));
    source.push_str(include_str!("../../rt_local_rate_trace.wgsl"));
    Ok(source)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rt_local_rate_compute_validates_and_emits_spirv() {
        use wgpu::naga;
        let source = shader_source().unwrap();
        let module = naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
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

#[cfg(test)]
#[path = "rt_local_rate_tests.rs"]
mod gpu_tests;
