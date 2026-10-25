//! Local RT light receivers for the existing transient model geometry stream.
use super::*;

struct CompiledRtModelReceivers {
    key: (wgpu::TextureFormat, u32, bool, bool),
    lights_layout: wgpu::BindGroupLayout,
    opaque: wgpu::RenderPipeline,
    mask: wgpu::RenderPipeline,
    blend: wgpu::RenderPipeline,
    mask_blend: wgpu::RenderPipeline,
}

pub(super) struct RtModelReceivers {
    pub key: (wgpu::TextureFormat, u32, bool, bool),
    pub lights: wgpu::BindGroup,
    pub opaque: wgpu::RenderPipeline,
    pub mask: wgpu::RenderPipeline,
    pub blend: wgpu::RenderPipeline,
    pub mask_blend: wgpu::RenderPipeline,
}

impl Renderer {
    pub(super) fn ensure_rt_model_receivers(&mut self) {
        if self.dynamic_lights_mode != DynamicLightsMode::RayTracedHardware
            || !self.hardware_rt_active()
        {
            return;
        }
        let key = (
            self.scene_format(),
            self.msaa_samples,
            self.dynamic_model_renderer.legacy_fog_compiled,
            self.map_light_simulation_active(),
        );
        if let Some(rt) = self.ray_traced_shadows.as_mut() {
            discard_incompatible_receivers(&mut rt.model_receivers, key);
            if rt.model_receivers.is_some() {
                return;
            }
        }
        let Some(rt) = self.ray_traced_shadows.as_ref() else {
            return;
        };
        if self.world.is_none() {
            return;
        }

        let job_key = PipelineJobKey::new("rt-model-receivers", 0, pipeline_hash(&key));
        if let Some(compiled) = self
            .pipeline_jobs
            .take_ready::<CompiledRtModelReceivers>(job_key)
        {
            // Bind groups carry map-owned light buffers, so build this cheap piece
            // on the render thread at install time. A worker result can therefore
            // survive a map transition without retaining or rebinding stale buffers.
            let Some(world) = self.world.as_ref() else {
                return;
            };
            let lights = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("RT model local lights"),
                layout: &compiled.lights_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: world.light_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: world._cluster_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.lighting_settings_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&world.local_shadows.cube_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::Sampler(&world.local_shadows.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 17,
                        resource: world.local_shadows.glass_buffer.as_entire_binding(),
                    },
                ],
            });
            if let Some(rt) = self.ray_traced_shadows.as_mut() {
                rt.model_receivers = Some(RtModelReceivers {
                    key: compiled.key,
                    lights,
                    opaque: compiled.opaque,
                    mask: compiled.mask,
                    blend: compiled.blend,
                    mask_blend: compiled.mask_blend,
                });
            }
            return;
        }

        let device = self.device.clone();
        let camera_layout = self.camera_layout.clone();
        let texture_layout = self.dynamic_model_renderer.texture_layout.clone();
        let receiver_layout = rt.receiver_layout.clone();
        self.pipeline_jobs
            .request(job_key, "RT dynamic-model receiver pipelines", move || {
                let lights_layout = light_layout(&device);
                let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("RT model receiver layout"),
                    bind_group_layouts: &[
                        Some(&camera_layout),
                        Some(&texture_layout),
                        Some(&lights_layout),
                        Some(&receiver_layout),
                    ],
                    immediate_size: 0,
                });
                let source = model_shader_source(key.3).expect("RT model shader anchors");
                crate::renderer::lighting::rt_diagnostics::dump_shader("model-rt", &source);
                let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("RT model receivers"),
                    source: wgpu::ShaderSource::Wgsl(source.into()),
                });
                let pipeline = |alpha| {
                    create_dynamic_model_pipeline_inner(
                        &device, &layout, &shader, key.0, key.1, alpha, key.2, true, true, false, false,
                    )
                };
                CompiledRtModelReceivers {
                    key,
                    lights_layout,
                    opaque: pipeline(DynamicModelAlphaMode::Opaque),
                    mask: pipeline(DynamicModelAlphaMode::Mask),
                    blend: pipeline(DynamicModelAlphaMode::Blend),
                    mask_blend: pipeline(DynamicModelAlphaMode::MaskBlend),
                }
            });
    }
}

fn discard_incompatible_receivers(
    receivers: &mut Option<RtModelReceivers>,
    key: (wgpu::TextureFormat, u32, bool, bool),
) {
    // A worker may take several frames. Old pipelines cannot bridge a change
    // in attachment format/MSAA; normal model lighting is the safe fallback.
    if receivers.as_ref().is_some_and(|models| models.key != key) {
        *receivers = None;
    }
}

fn light_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let mut entries = std::array::from_fn::<_, 3, _>(|binding| wgpu::BindGroupLayoutEntry {
        binding: binding as u32,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: if binding == 2 {
                wgpu::BufferBindingType::Uniform
            } else {
                wgpu::BufferBindingType::Storage { read_only: true }
            },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }).to_vec();
    entries.extend([
        wgpu::BindGroupLayoutEntry {
            binding: 4,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::CubeArray,
                multisampled: false,
            },
            count: None,
        },
        comparison_sampler_entry(5),
        wgpu::BindGroupLayoutEntry {
            binding: 17,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
    ]);
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("RT model local lights"),
        entries: &entries,
    })
}

// Reuse the authoritative BSP attenuation, segment sampling and alpha-query
// helpers. Checked anchors fail loudly if their declarations change.
pub(super) fn declaration<'a>(source: &'a str, anchor: &str) -> Result<&'a str, String> {
    let start = source
        .find(anchor)
        .ok_or_else(|| format!("missing {anchor}"))?;
    let body = source[start..]
        .find('{')
        .ok_or_else(|| format!("missing body: {anchor}"))?
        + start;
    let mut depth = 0;
    for (offset, byte) in source.as_bytes()[body..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(&source[start..body + offset + 1]);
                }
            }
            _ => {}
        }
    }
    Err(format!("unterminated {anchor}"))
}

// The receiver variant appends one vertex attribute to md3.wgsl's VertexIn, which
// declares DynamicModelVertex's own attributes by literal location. It must land
// past all of them, and on the same location the pipeline's second vertex buffer
// binds; a new vertex field otherwise silently collides with it.
const _: () = assert!(
    RT_RECEIVER_RAW_COLOR_LOCATION == 6,
    "update the injected VertexIn raw_color location in model_shader_source"
);

fn model_shader_source(map_lights: bool) -> Result<String, String> {
    let donor = ray_traced_world_shader_source(include_str!("../../bsp.wgsl"))?;
    let mut source = format!(
        "enable wgpu_ray_query;\n{}\n",
        crate::weather::with_model_weather(
            &include_str!("../../md3.wgsl").replace("\r\n", "\n"),
        )
    );
    // RT model receivers only compile the opaque, mask, and blend entry points.
    // The regular model's glass entry points need a separate glass resource
    // group, which is absent from this receiver layout.
    for anchor in ["@fragment\nfn fs_glass(", "@fragment fn fs_glass_mask("] {
        let glass_fragment = declaration(&source, anchor)?.to_owned();
        source = source.replacen(&glass_fragment, "", 1);
    }
    for anchor in [
        "struct PointLight ",
        "struct ClusterRecord ",
        "struct LightingSettings ",
    ] {
        source.push_str(declaration(&donor, anchor)?);
        source.push_str(";\n");
    }
    let start = donor
        .find("@group(3) @binding(8)")
        .ok_or("missing RT bindings")?;
    let last = donor
        .find("@group(3) @binding(14)")
        .ok_or("missing RT alpha bindings")?;
    let end = last
        + donor[last..]
            .find(';')
            .ok_or("missing RT binding terminator")?
        + 1;
    source.push_str(&donor[start..end]);
    source.push_str(
        "\n@group(2) @binding(0) var<storage, read> dynamic_lights: array<PointLight>;\n\
        @group(2) @binding(1) var<storage, read> light_clusters: array<ClusterRecord>;\n\
        @group(2) @binding(2) var<uniform> lighting_settings: LightingSettings;\n\
        @group(2) @binding(4) var local_shadow_texture: texture_depth_cube_array;\n\
        @group(2) @binding(5) var local_shadow_sampler: sampler_comparison;\n\
        const ENABLE_LOCAL_SHADOWS: bool = true;\n\
        const ENABLE_RAY_TRACED_SHADOWS: bool = true;\n\
        const ENABLE_CLUSTERED_LITE_DLIGHTS: bool = false;\n",
    );
    source.push_str(&format!(
        "const ENABLE_MAP_LIGHT_SIMULATION: bool = {map_lights};\n"
    ));
    for line in donor
        .lines()
        .filter(|line| line.starts_with("const CLUSTER_") || line.starts_with("const LOCAL_SHADOW_NEAR:"))
    {
        source.push_str(line);
        source.push('\n');
    }
    for name in [
        "rt_rand_f",
        "rt_sample_count",
        "rt_local_sample_count",
        "rt_sample_local_emitter",
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
        "rt_trace_shadow_throughput",
        "rt_trace_shadow_query",
        "rt_local_shadow_rgb",
        "rt_trace_shadow_visibility",
        "ray_traced_local_shadow_visibility",
        "rt_trace_local_visibility",
        "sample_local_shadow_visibility",
        "baked_local_moving_maybe",
        "baked_local_moving_visibility",
        "local_light_shadow_visibility",
        "local_light_shadow_rgb",
        "q3map_effective_distance",
        "q3map_surface_angle",
        "local_light_attenuation",
        "q3map_light_scalar",
        "q3map_fast_contribution_visible",
        "local_light_surface_scale",
        "emitter_visibility",
        "cluster_for_fragment",
    ] {
        if !crate::renderer::lighting::rt_diagnostics::config().receiver.glass
            && matches!(name, "rt_candidate_glass_throughput" | "rt_trace_shadow_throughput") {
            continue;
        }
        source.push_str(declaration(&donor, &format!("fn {name}("))?);
        source.push('\n');
    }
    if crate::renderer::lighting::rt_diagnostics::config().receiver.glass {
        for name in ["RasterGlassTriangle", "RasterGlassSlot"] {
            source.push_str(declaration(&donor, &format!("struct {name} "))?);
            source.push_str(";\n");
        }
        source.push_str("@group(2) @binding(17) var<storage, read> raster_glass_slots: array<RasterGlassSlot>;\n");
        source.push_str(declaration(&donor, "fn raster_glass_throughput(")?);
        source.push('\n');
    }
    for (from, to, count) in [
        (
            "@location(3) color: vec4<f32>,",
            "@location(3) color: vec4<f32>,\n    @location(6) raw_color: vec4<f32>,",
            1,
        ),
        (
            "@location(3) world_position: vec3<f32>,",
            "@location(3) world_position: vec3<f32>,\n    @location(5) raw_color: vec4<f32>,",
            1,
        ),
        (
            "output.color = input.color;",
            "output.color = input.color;\n    output.raw_color = input.raw_color;",
            1,
        ),
        (
            "lit_color(input.normal, tex) * input.color, input.normal, input.world_position, input.water_wetness)",
            "rt_model_color(input, tex), input.normal, input.world_position, input.water_wetness)",
            2,
        ),
    ] {
        if source.matches(from).count() != count {
            return Err(format!("model shader anchor changed: {from}"));
        }
        source = source.replace(from, to);
    }
    source.push_str(include_str!("../../rt_receiver_flags.wgsl"));
    source.push_str(include_str!("../../rt_local_rate_surface.wgsl"));
    source.push_str(include_str!("../../rt_local_rate_read.wgsl"));
    source.push_str(include_str!("../../rt_solari_ris.wgsl"));
    source.push_str(declaration(&donor, "fn rt_rate_local_shadow_rgb(")?);
    source.push_str(&rt_sampled_light_loops(
        &include_str!("../../rt_model_lighting.wgsl").replace("\r\n", "\n"),
    )?);
    Ok(source)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires a Vulkan GPU"]
    fn rt_model_cache_drops_old_msaa_before_replacement_is_ready() {
        block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::VULKAN,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter = instance.request_adapter(&Default::default()).await.unwrap();
            let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
                required_features: wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES,
                ..Default::default()
            }).await.unwrap();
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: None,
                source: wgpu::ShaderSource::Wgsl(r#"
@vertex fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(p[i], 0.0, 1.0);
}
@fragment fn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(0.25, 0.5, 0.75, 1.0); }
"#.into()),
            });
            let make = |key: (wgpu::TextureFormat, u32, bool, bool)| {
                let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("JKA dynamic model opaque pipeline"), layout: None,
                    vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[] },
                    fragment: Some(wgpu::FragmentState {
                        module: &shader, entry_point: Some("fs_main"), compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState { format: key.0, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                    }),
                    primitive: Default::default(), depth_stencil: None,
                    multisample: wgpu::MultisampleState { count: key.1, ..Default::default() },
                    multiview_mask: None, cache: crate::pipeline_cache::get().as_ref(),
                });
                let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[] });
                let lights = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &layout, entries: &[] });
                RtModelReceivers { key, lights, opaque: pipeline.clone(), mask: pipeline.clone(), blend: pipeline.clone(), mask_blend: pipeline }
            };
            let encode = |pipeline: &wgpu::RenderPipeline, key: (wgpu::TextureFormat, u32, bool, bool)| {
                let target = device.create_texture(&wgpu::TextureDescriptor {
                    label: None, size: wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 },
                    mip_level_count: 1, sample_count: key.1, dimension: wgpu::TextureDimension::D2,
                    format: key.0, usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[],
                });
                let view = target.create_view(&Default::default());
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("JKA frame encoder") });
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: None,
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, depth_slice: None, resolve_target: None,
                            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                        depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
                    });
                    pass.set_pipeline(pipeline);
                    pass.draw(0..3, 0..1);
                }
                encoder.finish()
            };
            let key2 = (wgpu::TextureFormat::Bgra8UnormSrgb, 2, false, false);
            let key1 = (key2.0, 1, false, false);
            let mut cached = Some(make(key2));
            // Reproduce the reported validation failure before applying the
            // cache fix. This requires no world assets or RT driver compiler.
            let guard = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let _invalid = encode(&cached.as_ref().unwrap().opaque, key1);
            let error = guard.pop().await.expect("old 2x pipeline unexpectedly accepted in a 1x pass");
            assert!(error.to_string().contains("sample count"), "{error}");
            discard_incompatible_receivers(&mut cached, key1);
            assert!(cached.is_none(), "a pending async replacement must leave no incompatible receiver");
            // While the RT worker is pending, the ordinary 1x model pipeline
            // can render. Repeated MSAA/format/fog/source transitions invalidate
            // the same cache contract and cannot reintroduce the stale pipeline.
            for key in [key1, key2, key1, (wgpu::TextureFormat::Rgba16Float, 1, false, false),
                (wgpu::TextureFormat::Rgba16Float, 1, true, false), (wgpu::TextureFormat::Rgba16Float, 1, true, true)] {
                discard_incompatible_receivers(&mut cached, key);
                assert!(cached.is_none());
                let replacement = make(key);
                let guard = device.push_error_scope(wgpu::ErrorFilter::Validation);
                queue.submit([encode(&replacement.opaque, key)]);
                assert!(guard.pop().await.is_none());
                cached = Some(replacement);
                discard_incompatible_receivers(&mut cached, key);
                assert!(cached.is_some(), "matching receiver must remain installed");
            }
            println!("reproduced 2x-to-1x validation error; six replacement/fallback transitions passed");
        });
    }

    #[test]
    #[ignore = "requires a Vulkan GPU with hardware ray queries"]
    fn model_receiver_gpu_pipelines() {
        block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::VULKAN,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .unwrap();
            println!("RT model pipeline validation: {:?}", adapter.get_info());
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("RT model pipeline validation"),
                    required_features: wgpu::Features::EXPERIMENTAL_RAY_QUERY,
                    required_limits: adapter.limits(),
                    experimental_features: unsafe { wgpu::ExperimentalFeatures::enabled() },
                    ..Default::default()
                })
                .await
                .unwrap();
            check_emitter_sampling(&device, &queue);
            let camera = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT)],
            });
            let texture = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries: &[
                    texture_entry(0),
                    sampler_entry(1),
                    uniform_entry(2, wgpu::ShaderStages::FRAGMENT),
                ],
            });
            let lights = light_layout(&device);
            let scene = create_ray_traced_shadow_receiver_layout(&device);
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[Some(&camera), Some(&texture), Some(&lights), Some(&scene)],
                immediate_size: 0,
            });
            for map in [false, true] {
                let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: None,
                    source: wgpu::ShaderSource::Wgsl(model_shader_source(map).unwrap().into()),
                });
                for (format, samples, fog) in [
                    (wgpu::TextureFormat::Bgra8UnormSrgb, 1, false),
                    (wgpu::TextureFormat::Rgba16Float, 4, true),
                ] {
                    for alpha in [
                        DynamicModelAlphaMode::Opaque,
                        DynamicModelAlphaMode::Mask,
                        DynamicModelAlphaMode::Blend,
                        DynamicModelAlphaMode::MaskBlend,
                    ] {
                        create_dynamic_model_pipeline_inner(
                            &device, &layout, &shader, format, samples, alpha, fog, true, true, false, false,
                        );
                    }
                }
            }
        });
    }
    fn check_emitter_sampling(device: &wgpu::Device, queue: &wgpu::Queue) {
        let donor = ray_traced_world_shader_source(include_str!("../../bsp.wgsl")).unwrap();
        let mut source = String::from(
            r#"
struct VertexOut { clip_position: vec4<f32>, world_position: vec3<f32> };
struct Camera { render_flags: vec4<u32> };
const camera = Camera(vec4<u32>(0u));
struct Settings { values: vec4<u32>, map_ambient: vec4<f32> };
var<private> lighting_settings: Settings;
@group(0) @binding(0) var<storage, read_write> results: array<vec4<f32>>;
"#,
        );
        source.push_str(declaration(&donor, "struct PointLight ").unwrap());
        source.push_str(";\n");
        for name in [
            "rt_rand_f",
            "rt_sample_count",
            "rt_local_sample_count",
            "rt_sample_local_emitter",
            "rt_emitter_seed",
            "rt_sample_local_emitter_seed",
        ] {
            source.push_str(declaration(&donor, &format!("fn {name}(")).unwrap());
            source.push('\n');
        }
        source.push_str(
            r#"
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let count = 1u << (id.x / 256u);
    lighting_settings = Settings(vec4<u32>(1u, 1u, 256u, 1u), vec4<f32>(0.0, 0.0, 0.0, f32(count)));
    let input = VertexOut(vec4<f32>(f32(id.x % 256u), 0.0, 0.0, 1.0), vec3<f32>(0.0, 0.0, -10.0));
    var light = PointLight(vec4<f32>(0.0, 0.0, 0.0, 10.0), vec4<f32>(3.0, 2.0, 1.0, 2.0),
        vec4<f32>(1.0, 0.0, 0.0, -1.0), vec4<f32>(0.0), vec4<f32>(0.0, 0.0, 0.0, 1.0));
    var sum = vec3<f32>(0.0);
    var failures = select(1.0, 0.0, rt_local_sample_count(light) == count);
    for (var i = 0u; i < count; i += 1u) {
        let sample = rt_sample_local_emitter(light, input, 0u, i, count);
        let t = (sample.position_radius.x + 1.0) * 0.5;
        if (t < f32(i) / f32(count) || t > f32(i + 1u) / f32(count)) { failures += 1.0; }
        sum += sample.color_intensity.rgb * sample.color_intensity.a;
    }
    // A finite point emitter should use the same quality count, preserve total
    // radiance, move within its projected source disk, and resolve to a point.
    light.emitter = vec4<f32>(0.0);
    light.shadow.z = 2.0;
    var sphere_sum = vec3<f32>(0.0);
    if (rt_local_sample_count(light) != count) { failures += 1.0; }
    for (var i = 0u; i < count; i += 1u) {
        let sample = rt_sample_local_emitter(light, input, 0u, i, count);
        let offset = sample.position_radius.xyz - light.position_radius.xyz;
        if (length(offset) > 2.0001 || sample.shadow.z != 0.0 || sample.emitter.w != 0.0) { failures += 1.0; }
        sphere_sum += sample.color_intensity.rgb * sample.color_intensity.a;
    }
    if (any(abs(sphere_sum - vec3<f32>(6.0, 4.0, 2.0)) > vec3<f32>(0.0001))) { failures += 1.0; }

    // Negative q3map angle metadata must remain untouched at every quality level.
    light.shadow.z = 0.0;
    light.shadow.w = 0.0;
    light.emitter.w = -2.5;
    let point = rt_sample_local_emitter(light, input, 0u, 0u, rt_local_sample_count(light));
    if (rt_local_sample_count(light) != 1u || any(point.emitter != light.emitter)
        || any(point.color_intensity != light.color_intensity)
        || any(point.position_radius != light.position_radius)) { failures += 1.0; }
    results[id.x] = vec4<f32>(sum, failures);
}
"#,
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("RT sampling checks"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: crate::pipeline_cache::get().as_ref(),
        });
        let size = 768 * 16;
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: output.as_entire_binding(),
            }],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(12, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, size);
        let submission = queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .unwrap();
        rx.recv().unwrap().unwrap();
        let data = readback.slice(..).get_mapped_range().expect("GPU readback range");
        for (i, result) in bytemuck::cast_slice::<u8, [f32; 4]>(&data)
            .iter()
            .enumerate()
        {
            assert_eq!(
                *result,
                [6.0, 4.0, 2.0, 0.0],
                "GPU sample energy/strata/point metadata at seed {i}"
            );
        }
    }
    #[test]
    fn model_receivers_validate_and_emit_spirv() {
        use wgpu::naga;
        for map in [false, true] {
            let source = model_shader_source(map).unwrap();
            let module = naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
            let info = naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap();
            for fog in [0.0, 1.0] {
                let constants = [("ENABLE_LEGACY_FOG".into(), fog)].into_iter().collect();
                let (specialized, info) = naga::back::pipeline_constants::process_overrides(
                    &module, &info, None, &constants,
                )
                .unwrap();
                naga::back::spv::write_vec(
                    &specialized,
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
    }
}
