//! Opt-in, model-only authored fixture accent. No resources or shader changes in A.
use super::pipelines::create_ghoul2_skin_pipeline;
use crate::renderer::*;
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(in crate::renderer) struct FixtureLight {
    position_radius: [f32; 4],
    normal_power: [f32; 4],
    color_side: [f32; 4],
    shadow: [f32; 4],
}

pub(in crate::renderer) struct FixtureLighting {
    key: (wgpu::TextureFormat, u32, bool, bool),
    shadow_view: wgpu::TextureView,
    pub bind_group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    previous: [FixtureLight; 4],
    model_layout: wgpu::PipelineLayout,
    skin_layout: wgpu::PipelineLayout,
    model_shader: wgpu::ShaderModule,
    skin_shader: wgpu::ShaderModule,
    // Mode and Source-material variants are lazy and never replace control PSOs.
    pipelines: HashMap<(DynamicModelAlphaMode, bool, bool), wgpu::RenderPipeline>,
    pub raw_colors: Vec<[f32; 4]>,
    pub raw_buffer: Option<wgpu::Buffer>,
    raw_capacity: usize,
}

fn enabled(relight: bool, ab: bool, csm: bool, hardware_rt: bool) -> bool {
    // Hardware RT already has a separate per-pixel model receiver. Keep its
    // material/transient-light behavior intact during this raster experiment.
    relight && ab && csm && !hardware_rt
}

pub(in crate::renderer) fn eligible(mode: DynamicModelAlphaMode) -> bool {
    matches!(
        mode,
        DynamicModelAlphaMode::Opaque
            | DynamicModelAlphaMode::Mask
            | DynamicModelAlphaMode::Blend
            | DynamicModelAlphaMode::MaskBlend
            | DynamicModelAlphaMode::BlendDepthWrite
    )
}

impl Renderer {
    pub(in crate::renderer) fn entity_fixture_active(&self) -> bool {
        // In the ordinary control configuration, avoid even querying the
        // current RT/world state at each existing shadow-selection gate.
        if !self.entity_fixture_lighting_ab || !self.sun_override {
            return false;
        }
        enabled(
            self.sun_override,
            self.entity_fixture_lighting_ab,
            self.cascaded_shadow_mode.is_csm(),
            self.hardware_rt_active(),
        )
    }

    pub(in crate::renderer) fn set_entity_fixture_lighting_ab(&mut self, value: bool) {
        if self.entity_fixture_lighting_ab == value {
            return;
        }
        self.entity_fixture_lighting_ab = value;
        if let Some(world) = &mut self.world {
            world.local_shadows.selection_position = None;
        }
        if !value {
            self.dynamic_model_renderer.fixture_lighting = None;
        }
        self.history_valid = false;
    }

    pub(in crate::renderer) fn sync_entity_fixture_lighting(&mut self) {
        if !self.entity_fixture_active() {
            self.dynamic_model_renderer.fixture_lighting = None;
            return;
        }
        let Some(world) = &self.world else {
            self.dynamic_model_renderer.fixture_lighting = None;
            return;
        };
        let format = self.scene_format();
        let moving = self.baked_local_moving_casters();
        let models = &mut self.dynamic_model_renderer;
        let key = (
            format,
            self.msaa_samples,
            models.legacy_fog_compiled,
            models.model_wet_compiled,
        );
        if models
            .fixture_lighting
            .as_ref()
            .is_none_or(|f| f.key != key || f.shadow_view != world.local_shadows.cube_view)
        {
            models.fixture_lighting = Some(FixtureLighting::new(
                &self.device,
                models,
                &world.local_shadows.cube_view,
                &world.local_shadows.sampler,
                key,
            ));
        }
        let mut lights = [FixtureLight::zeroed(); 4];
        for (out, &index) in lights
            .iter_mut()
            .zip(&world.local_shadows.active_light_indices)
        {
            let Some((_, light)) = world.static_bake_sources.iter().find(|(i, l)| {
                *i == index
                    && crate::renderer::lighting::lightmap_local::source_enabled(
                        l,
                        crate::renderer::lighting::lightmap_local::AUTHORED,
                    )
            }) else {
                continue;
            };
            let Some(slot) = world
                .local_shadows
                .cache_entries
                .iter()
                .position(|e| e.light_index == Some(index))
            else {
                continue;
            };
            if world.local_shadows.pending_slots.contains(&slot) {
                continue;
            }
            *out = FixtureLight {
                position_radius: [
                    light.position[0],
                    light.position[1],
                    light.position[2],
                    light.radius,
                ],
                normal_power: [
                    light.emitter_normal[0],
                    light.emitter_normal[1],
                    light.emitter_normal[2],
                    light.shadow_power,
                ],
                color_side: [
                    light.color[0],
                    light.color[1],
                    light.color[2],
                    u8::from(light.emitter_two_sided) as f32,
                ],
                // Moving maps exist only while the existing inferred-shadow
                // feature has moving casters; never sample an unrendered layer.
                shadow: [
                    slot as f32,
                    if moving {
                        (slot + LOCAL_SHADOW_CACHE_SLOTS) as f32
                    } else {
                        -1.0
                    },
                    0.0,
                    0.0,
                ],
            };
        }
        let state = models.fixture_lighting.as_mut().unwrap();
        if state.previous != lights {
            self.queue
                .write_buffer(&state.uniform, 0, bytemuck::cast_slice(&lights));
            state.previous = lights;
            rverbose!(
                1,
                "[ENTITY FIXTURE A/B] B active: {} authored fixtures; world lightmaps unchanged",
                lights.iter().filter(|l| l.normal_power[3] > 0.0).count()
            );
        }
    }
}

impl FixtureLighting {
    fn new(
        device: &wgpu::Device,
        models: &DynamicModelRenderer,
        shadow: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        key: (wgpu::TextureFormat, u32, bool, bool),
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Entity fixture B bindings"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::FRAGMENT),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::CubeArray,
                        multisampled: false,
                    },
                    count: None,
                },
                comparison_sampler_entry(2),
            ],
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Entity fixture B lights"),
            contents: bytemuck::cast_slice(&[FixtureLight::zeroed(); 4]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Entity fixture B lights"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(shadow),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        let model_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Entity fixture B MD3 layout"),
            bind_group_layouts: &[
                Some(&models.camera_layout),
                Some(&models.texture_layout),
                Some(&layout),
            ],
            immediate_size: 0,
        });
        let skin_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Entity fixture B Ghoul2 layout"),
            bind_group_layouts: &[
                Some(&models.camera_layout),
                Some(&models.texture_layout),
                Some(&models.skin_layout),
                Some(&layout),
            ],
            immediate_size: 0,
        });
        let shader = |skin| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Entity fixture B shader"),
                source: wgpu::ShaderSource::Wgsl(
                    shader_source(skin).expect("fixture shader anchors").into(),
                ),
            })
        };
        Self {
            key,
            shadow_view: shadow.clone(),
            bind_group,
            uniform,
            previous: [FixtureLight::zeroed(); 4],
            model_layout,
            skin_layout,
            model_shader: shader(false),
            skin_shader: shader(true),
            pipelines: HashMap::new(),
            raw_colors: Vec::new(),
            raw_buffer: None,
            raw_capacity: 0,
        }
    }

    pub fn pipeline(
        &self,
        mode: DynamicModelAlphaMode,
        skin: bool,
        source: bool,
    ) -> Option<&wgpu::RenderPipeline> {
        self.pipelines.get(&(mode, skin, source))
    }

    pub fn ensure(
        &mut self,
        device: &wgpu::Device,
        jobs: &mut PipelineJobManager,
        mode: DynamicModelAlphaMode,
        skin: bool,
        source: bool,
    ) {
        if !eligible(mode) || self.pipeline(mode, skin, source).is_some() {
            return;
        }
        let config = pipeline_hash(&(self.key, skin, source));
        let job_key = PipelineJobKey::new("entity-fixture-B", mode.index() as u32, config);
        if let Some(pipeline) = jobs.take_ready(job_key) {
            self.pipelines.insert((mode, skin, source), pipeline);
            return;
        }
        let device = device.clone();
        let layout = if skin {
            self.skin_layout.clone()
        } else {
            self.model_layout.clone()
        };
        let shader = if skin {
            self.skin_shader.clone()
        } else {
            self.model_shader.clone()
        };
        let key = self.key;
        jobs.request(job_key, "Entity fixture B model variant", move || {
            if skin {
                create_ghoul2_skin_pipeline(
                    &device, &layout, &shader, key.0, key.1, mode, key.2, key.3, false, source,
                )
            } else {
                // Same raw-tint stream as the RT receiver, without ray tracing.
                create_dynamic_model_pipeline_inner(
                    &device, &layout, &shader, key.0, key.1, mode, key.2, key.3, true, false, false,
                )
            }
        });
    }

    pub fn upload_raw(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.raw_colors.is_empty() {
            return;
        }
        let bytes = bytemuck::cast_slice(&self.raw_colors);
        if bytes.len() > self.raw_capacity {
            self.raw_capacity = bytes.len().next_power_of_two();
            self.raw_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Entity fixture B raw model tint"),
                size: self.raw_capacity as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        queue.write_buffer(self.raw_buffer.as_ref().unwrap(), 0, bytes);
    }
}

fn shader_source(skin: bool) -> Result<String, String> {
    let base = if skin {
        include_str!("../../ghoul2_skin.wgsl")
    } else {
        include_str!("../../md3.wgsl")
    };
    let mut source = crate::weather::with_model_weather(&base.replace("\r\n", "\n"));
    source.push_str(include_str!("../../studio_lighting.wgsl"));
    for anchor in ["@fragment\nfn fs_glass(", "@fragment fn fs_glass_mask("] {
        let declaration = crate::renderer::rt_models::declaration(&source, anchor)?.to_owned();
        source = source.replacen(&declaration, "", 1);
    }
    if skin {
        // The draw table retains the original tint even after the vertex
        // shader folds classic light into input.color. Source materials retain
        // their own albedo/normal-map behavior.
        source = source.replace("shaded_color(input, tex)", "fixture_color(input, tex)");
        source.push_str("\nfn fixture_color(input: VertexOut, tex: vec4<f32>) -> vec4<f32> {\n let base = shaded_color(input,tex);\n let tint = skin_draws[input.draw_index].color;\n return vec4(base.rgb + tex.rgb*tint.rgb*model_brightness()*fixture_irradiance(input.world_position,input.normal),base.a);\n}\n");
    } else {
        source = source.replacen(
            "struct VertexIn {",
            "struct VertexIn {\n @location(7) fixture_raw: vec4<f32>,",
            1,
        );
        source = source.replacen(
            "struct VertexOut {",
            // MD3 outputs 6/7 carry the Predator cloak varyings. Vertex
            // inputs use a separate location namespace, so fixture_raw's
            // input stays at 7 while its vertex-to-fragment output uses 8.
            "struct VertexOut {\n @location(8) fixture_raw: vec4<f32>,",
            1,
        );
        source = source.replacen(
            "output.color = input.color;",
            "output.color = input.color;\n output.fixture_raw = input.fixture_raw;",
            1,
        );
        source = source.replace(
            "lit_color(input.normal, tex) * input.color",
            "fixture_color(input, tex)",
        );
        source.push_str("\nfn fixture_color(input: VertexOut, tex: vec4<f32>) -> vec4<f32> {\n let base = lit_color(input.normal,tex)*input.color;\n return vec4(base.rgb + tex.rgb*input.fixture_raw.rgb*bitcast<f32>(entity_lighting.values.y)*fixture_irradiance(input.world_position,input.normal),base.a);\n}\n");
    }
    source.push_str(
        &include_str!("fixture_lighting.wgsl")
            .replace("FIXTURE_GROUP", if skin { "3" } else { "2" }),
    );
    Ok(source)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn entity_fixture_gate_requires_explicit_opt_in_and_relight() {
        for relight in [false, true] {
            for ab in [false, true] {
                for csm in [false, true] {
                    for rt in [false, true] {
                        assert_eq!(enabled(relight, ab, csm, rt), relight && ab && csm && !rt);
                    }
                }
            }
        }
    }
    #[test]
    fn entity_fixture_model_shaders_validate() {
        for skin in [false, true] {
            let source = shader_source(skin).unwrap();
            let module = wgpu::naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
            wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("skin={skin}: {e:?}"));
        }
        assert!(!include_str!("../../md3.wgsl").contains("fixture_lights"));
        assert!(!include_str!("../../ghoul2_skin.wgsl").contains("fixture_lights"));
    }

    #[test]
    #[ignore = "requires a Vulkan GPU"]
    fn entity_fixture_direction_and_occlusion_on_gpu() {
        block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::VULKAN,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter = instance.request_adapter(&Default::default()).await.unwrap();
            let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let cube = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("fixture visibility test"),
                size: wgpu::Extent3d {
                    width: 8,
                    height: 8,
                    depth_or_array_layers: 12,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            for layer in 0..12 {
                let view = cube.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                });
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(if layer < 6 { 1.0 } else { 0.0 }),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });
            }
            let mut lights = [FixtureLight::zeroed(); 4];
            for (i, (position, normal, color, slot)) in [
                ([-64.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 0.0], 0.0),
                ([64.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], 0.0),
                ([0.0, 0.0, 64.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0], 1.0),
            ]
            .into_iter()
            .enumerate()
            {
                lights[i] = FixtureLight {
                    position_radius: [position[0], position[1], position[2], 512.0],
                    normal_power: [normal[0], normal[1], normal[2], 10000.0],
                    color_side: [color[0], color[1], color[2], 0.0],
                    shadow: [slot, -1.0, 0.0, 0.0],
                };
            }
            let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&lights),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let source = format!(
                "struct Camera {{ render_flags:vec4<u32> }}; var<private> camera:Camera;\n{}\n@group(0) @binding(3) var<storage,read_write> output:array<vec4<f32>>;\n@compute @workgroup_size(1) fn main() {{\n output[0]=vec4(fixture_irradiance(vec3(0.0),vec3(-1.0,0.0,0.0)),1.0);\n output[1]=vec4(fixture_irradiance(vec3(0.0),vec3(1.0,0.0,0.0)),1.0);\n output[2]=vec4(fixture_irradiance(vec3(0.0),vec3(0.0,1.0,0.0)),1.0);\n output[3]=vec4(fixture_irradiance(vec3(0.0),vec3(0.0,0.0,1.0)),1.0);\n output[4]=vec4(fixture_irradiance(vec3(1000.0),vec3(-1.0,0.0,0.0)),1.0); }}",
                include_str!("fixture_lighting.wgsl").replace("FIXTURE_GROUP", "0")
            );
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: None,
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: None,
                layout: None,
                module: &shader,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            });
            let output = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 80,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 80,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let view = cube.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::CubeArray),
                ..Default::default()
            });
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                compare: Some(wgpu::CompareFunction::LessEqual),
                ..Default::default()
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: output.as_entire_binding(),
                    },
                ],
            });
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.dispatch_workgroups(1, 1, 1);
            }
            encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, 80);
            queue.submit([encoder.finish()]);
            readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            assert!(scope.pop().await.is_none(), "fixture GPU validation");
            let data = readback.slice(..).get_mapped_range().unwrap();
            let colors: &[[f32; 4]] = bytemuck::cast_slice(&data);
            assert!(
                colors[0][0] > 0.05 && colors[0][1] == 0.0 && colors[0][2] == 0.0,
                "left-facing must be red: {colors:?}"
            );
            assert!(
                colors[1][2] > 0.05 && colors[1][0] == 0.0,
                "right-facing must be blue: {colors:?}"
            );
            for i in [2, 3, 4] {
                assert_eq!(
                    &colors[i][..3],
                    &[0.0; 3],
                    "wrong-facing, wall-blocked or distant receiver: {colors:?}"
                );
            }
            drop(data);
            // Also compile the real MD3/Ghoul2 render PSOs against their real
            // layouts, including the Source-material specialization. Naga
            // validation alone cannot catch vertex/bind-group mismatches.
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries: &crate::foliage::camera_layout_entries(),
            });
            let glass_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries: &[],
            });
            let white = upload_texture(
                &device,
                &queue,
                &TextureData {
                    label: "fixture test white".into(),
                    source: None,
                    width: 1,
                    height: 1,
                    rgba: (vec![255; 4]).into(),
                    rgba16f: None,
                    mip_level_count: 1,
                    clamp: true,
                    srgb: true,
                },
            );
            let models = DynamicModelRenderer::new(
                &device,
                &queue,
                &camera_layout,
                &glass_layout,
                wgpu::TextureFormat::Rgba16Float,
                1,
                &white,
            );
            assert!(
                models.fixture_lighting.is_none(),
                "control must allocate no fixture resources"
            );
            let fixtures = FixtureLighting::new(
                &device,
                &models,
                &view,
                &sampler,
                (wgpu::TextureFormat::Rgba16Float, 1, false, false),
            );
            for mode in [
                DynamicModelAlphaMode::Opaque,
                DynamicModelAlphaMode::Mask,
                DynamicModelAlphaMode::BlendDepthWrite,
            ] {
                let _md3 = create_dynamic_model_pipeline_inner(
                    &device,
                    &fixtures.model_layout,
                    &fixtures.model_shader,
                    wgpu::TextureFormat::Rgba16Float,
                    1,
                    mode,
                    false,
                    false,
                    true,
                    false,
                    false,
                );
                for source in [false, true] {
                    let _skin = create_ghoul2_skin_pipeline(
                        &device,
                        &fixtures.skin_layout,
                        &fixtures.skin_shader,
                        wgpu::TextureFormat::Rgba16Float,
                        1,
                        mode,
                        false,
                        false,
                        false,
                        source,
                    );
                }
            }
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let validation = scope.pop().await;
            assert!(
                validation.is_none(),
                "fixture production model pipeline validation: {validation:?}"
            );
        });
    }
}
