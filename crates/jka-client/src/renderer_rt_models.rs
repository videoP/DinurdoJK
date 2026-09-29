//! Local RT light receivers for the existing transient model geometry stream.
use super::*;

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
        if self.dynamic_lights_mode != DynamicLightsMode::RayTracedHardware || !self.hardware_rt_active() {
            return;
        }
        let key = (self.scene_format(), self.msaa_samples,
            self.dynamic_model_renderer.legacy_fog_compiled, self.map_light_simulation_active());
        let Some(rt) = self.ray_traced_shadows.as_mut() else { return };
        if rt.model_receivers.as_ref().is_some_and(|models| models.key == key) { return; }
        let Some(world) = self.world.as_ref() else { return };
        let lights_layout = light_layout(&self.device);
        let lights = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("RT model local lights"), layout: &lights_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: world.light_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: world._cluster_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: self.lighting_settings_buffer.as_entire_binding() },
            ],
        });
        let layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("RT model receiver layout"),
            bind_group_layouts: &[Some(&self.camera_layout), Some(&self.dynamic_model_renderer.texture_layout),
                Some(&lights_layout), Some(&rt.receiver_layout)],
            immediate_size: 0,
        });
        let source = model_shader_source(key.3).expect("RT model shader anchors");
        let shader = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("RT model receivers"), source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = |alpha| create_dynamic_model_pipeline_inner(
            &self.device, &layout, &shader, key.0, key.1, alpha, key.2, true);
        rt.model_receivers = Some(RtModelReceivers {
            key, lights,
            opaque: pipeline(DynamicModelAlphaMode::Opaque),
            mask: pipeline(DynamicModelAlphaMode::Mask),
            blend: pipeline(DynamicModelAlphaMode::Blend),
            mask_blend: pipeline(DynamicModelAlphaMode::MaskBlend),
        });
    }
}

fn light_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("RT model local lights"),
            entries: &std::array::from_fn::<_, 3, _>(|binding| wgpu::BindGroupLayoutEntry {
                binding: binding as u32,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: if binding == 2 { wgpu::BufferBindingType::Uniform }
                        else { wgpu::BufferBindingType::Storage { read_only: true } },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }),
        })
}

// Reuse the authoritative BSP attenuation, segment sampling and alpha-query
// helpers. Checked anchors fail loudly if their declarations change.
pub(super) fn declaration<'a>(source: &'a str, anchor: &str) -> Result<&'a str, String> {
    let start = source.find(anchor).ok_or_else(|| format!("missing {anchor}"))?;
    let body = source[start..].find('{').ok_or_else(|| format!("missing body: {anchor}"))? + start;
    let mut depth = 0;
    for (offset, byte) in source.as_bytes()[body..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => { depth -= 1; if depth == 0 { return Ok(&source[start..body + offset + 1]); } }
            _ => {},
        }
    }
    Err(format!("unterminated {anchor}"))
}

fn model_shader_source(map_lights: bool) -> Result<String, String> {
    let donor = ray_traced_world_shader_source(include_str!("bsp.wgsl"))?;
    let mut source = format!("enable wgpu_ray_query;\n{}\n", include_str!("md3.wgsl").replace("\r\n", "\n"));
    for anchor in ["struct PointLight ", "struct ClusterRecord ", "struct LightingSettings "] {
        source.push_str(declaration(&donor, anchor)?);
        source.push_str(";\n");
    }
    let start = donor.find("@group(3) @binding(8)").ok_or("missing RT bindings")?;
    let last = donor.find("@group(3) @binding(14)").ok_or("missing RT alpha bindings")?;
    let end = last + donor[last..].find(';').ok_or("missing RT binding terminator")? + 1;
    source.push_str(&donor[start..end]);
    source.push_str("\n@group(2) @binding(0) var<storage, read> dynamic_lights: array<PointLight>;\n\
        @group(2) @binding(1) var<storage, read> light_clusters: array<ClusterRecord>;\n\
        @group(2) @binding(2) var<uniform> lighting_settings: LightingSettings;\n\
        const ENABLE_LOCAL_SHADOWS: bool = true;\n\
        const ENABLE_CLUSTERED_LITE_DLIGHTS: bool = false;\n");
    source.push_str(&format!("const ENABLE_MAP_LIGHT_SIMULATION: bool = {map_lights};\n"));
    for line in donor.lines().filter(|line| line.starts_with("const CLUSTER_")) {
        source.push_str(line); source.push('\n');
    }
    for name in ["rt_rand_f", "rt_sample_count", "rt_local_sample_count", "rt_sample_local_emitter", "rt_alpha_repeat_coord", "rt_alpha_texel",
        "rt_sample_alpha", "rt_alpha_generated_uv", "rt_alpha_candidate_blocks", "rt_trace_shadow_visibility",
        "ray_traced_local_shadow_visibility", "rt_trace_local_visibility", "q3map_effective_distance", "q3map_surface_angle",
        "local_light_attenuation", "q3map_light_scalar", "q3map_fast_contribution_visible",
        "local_light_surface_scale", "emitter_visibility", "cluster_for_fragment"] {
        source.push_str(declaration(&donor, &format!("fn {name}("))?);
        source.push('\n');
    }
    for (from, to, count) in [
        ("@location(3) color: vec4<f32>,", "@location(3) color: vec4<f32>,\n    @location(4) raw_color: vec4<f32>,", 1),
        ("@location(3) world_position: vec3<f32>,", "@location(3) world_position: vec3<f32>,\n    @location(4) raw_color: vec4<f32>,", 1),
        ("output.color = input.color;", "output.color = input.color;\n    output.raw_color = input.raw_color;", 1),
        ("lit_color(input.normal, tex) * input.color, input.world_position, false", "rt_model_color(input, tex), input.world_position, false", 2),
    ] {
        if source.matches(from).count() != count { return Err(format!("model shader anchor changed: {from}")); }
        source = source.replace(from, to);
    }
    source.push_str(&rt_sampled_light_loops(&include_str!("rt_model_lighting.wgsl").replace("\r\n", "\n"))?);
    Ok(source)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires a Vulkan GPU with hardware ray queries"]
    fn model_receiver_gpu_pipelines() {
        block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::VULKAN, ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions::default()).await.unwrap();
            println!("RT model pipeline validation: {:?}", adapter.get_info());
            let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("RT model pipeline validation"),
                required_features: wgpu::Features::EXPERIMENTAL_RAY_QUERY,
                required_limits: adapter.limits(),
                experimental_features: unsafe { wgpu::ExperimentalFeatures::enabled() },
                ..Default::default()
            }).await.unwrap();
            check_emitter_sampling(&device, &queue);
            rt_resolution::validate_gpu(&device, &queue);
            let camera = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None, entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT)],
            });
            let texture = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None, entries: &[texture_entry(0), sampler_entry(1), uniform_entry(2, wgpu::ShaderStages::FRAGMENT)],
            });
            let lights = light_layout(&device);
            let scene = create_ray_traced_shadow_receiver_layout(&device);
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None, bind_group_layouts: &[Some(&camera), Some(&texture), Some(&lights), Some(&scene)], immediate_size: 0,
            });
            for map in [false, true] {
                let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: None, source: wgpu::ShaderSource::Wgsl(model_shader_source(map).unwrap().into()),
                });
                for (format, samples, fog) in [(wgpu::TextureFormat::Bgra8UnormSrgb, 1, false), (wgpu::TextureFormat::Rgba16Float, 4, true)] {
                    for alpha in [DynamicModelAlphaMode::Opaque, DynamicModelAlphaMode::Mask, DynamicModelAlphaMode::Blend, DynamicModelAlphaMode::MaskBlend] {
                        create_dynamic_model_pipeline_inner(&device, &layout, &shader, format, samples, alpha, fog, true);
                    }
                }
            }
        });
    }
    fn check_emitter_sampling(device: &wgpu::Device, queue: &wgpu::Queue) {
        let donor = ray_traced_world_shader_source(include_str!("bsp.wgsl")).unwrap();
        let mut source = String::from(r#"
struct VertexOut { clip_position: vec4<f32> };
struct Camera { render_flags: vec4<u32> };
const camera = Camera(vec4<u32>(0u));
struct Settings { values: vec4<u32>, map_ambient: vec4<f32> };
var<private> lighting_settings: Settings;
@group(0) @binding(0) var<storage, read_write> results: array<vec4<f32>>;
"#);
        source.push_str(declaration(&donor, "struct PointLight ").unwrap());
        source.push_str(";\n");
        for name in ["rt_rand_f", "rt_sample_count", "rt_local_sample_count", "rt_sample_local_emitter"] {
            source.push_str(declaration(&donor, &format!("fn {name}(" )).unwrap());
            source.push('\n');
        }
        source.push_str(r#"
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let count = 1u << (id.x / 256u);
    lighting_settings = Settings(vec4<u32>(1u, 1u, 256u, 1u), vec4<f32>(0.0, 0.0, 0.0, f32(count)));
    let input = VertexOut(vec4<f32>(f32(id.x % 256u), 0.0, 0.0, 1.0));
    var light = PointLight(vec4<f32>(0.0, 0.0, 0.0, 10.0), vec4<f32>(3.0, 2.0, 1.0, 2.0),
        vec4<f32>(1.0, 0.0, 0.0, -1.0), vec4<f32>(0.0, 0.0, 0.0, 1.0));
    var sum = vec3<f32>(0.0);
    var failures = select(1.0, 0.0, rt_local_sample_count(light) == count);
    for (var i = 0u; i < count; i += 1u) {
        let sample = rt_sample_local_emitter(light, input, 0u, i, count);
        let t = (sample.position_radius.x + 1.0) * 0.5;
        if (t < f32(i) / f32(count) || t > f32(i + 1u) / f32(count)) { failures += 1.0; }
        sum += sample.color_intensity.rgb * sample.color_intensity.a;
    }
    // Negative q3map angle metadata must remain untouched at every quality level.
    light.shadow.w = 0.0;
    light.emitter.w = -2.5;
    let point = rt_sample_local_emitter(light, input, 0u, 0u, rt_local_sample_count(light));
    if (rt_local_sample_count(light) != 1u || any(point.emitter != light.emitter)
        || any(point.color_intensity != light.color_intensity)
        || any(point.position_radius != light.position_radius)) { failures += 1.0; }
    results[id.x] = vec4<f32>(sum, failures);
}
"#);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("RT sampling checks"), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None, layout: None, module: &shader, entry_point: Some("main"), compilation_options: Default::default(), cache: None,
        });
        let size = 768 * 16;
        let output = device.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
        let readback = device.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &pipeline.get_bind_group_layout(0), entries: &[wgpu::BindGroupEntry { binding: 0, resource: output.as_entire_binding() }] });
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
        readback.slice(..).map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
        device.poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: Some(std::time::Duration::from_secs(30)) }).unwrap();
        rx.recv().unwrap().unwrap();
        let data = readback.slice(..).get_mapped_range();
        for (i, result) in bytemuck::cast_slice::<u8, [f32; 4]>(&data).iter().enumerate() {
            assert_eq!(*result, [6.0, 4.0, 2.0, 0.0], "GPU sample energy/strata/point metadata at seed {i}");
        }
    }
    #[test]
    fn model_receivers_validate_and_emit_spirv() {
        use wgpu::naga;
        for map in [false, true] {
            let source = model_shader_source(map).unwrap();
            let module = naga::front::wgsl::parse_str(&source).unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
            let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
                .validate(&module).unwrap();
            for fog in [0.0, 1.0] {
                let constants = [("ENABLE_LEGACY_FOG".into(), fog)].into_iter().collect();
                let (specialized, info) = naga::back::pipeline_constants::process_overrides(&module, &info, None, &constants).unwrap();
                naga::back::spv::write_vec(&specialized, &info,
                    &naga::back::spv::Options { lang_version: (1, 4), ..Default::default() }, None).unwrap();
            }
        }
    }
}
