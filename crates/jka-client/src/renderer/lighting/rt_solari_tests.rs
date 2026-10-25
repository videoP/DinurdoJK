use super::*;

// Execute the production transform and reservoir with a counted RGB traversal
// oracle. Keep candidate rejection, emitter normalization, and material shading.
fn counted_source() -> String {
    let mut source = rt_models::declaration(include_str!("../../bsp.wgsl"), "struct PointLight ")
        .unwrap().to_owned();
    source.push_str(";\n");
    source.push_str(rt_models::declaration(include_str!("../../rt_core.wgsl"), "fn rt_rand_f(").unwrap());
    source.push_str(r#"
struct VertexOut { clip_position: vec4<f32>, world_position: vec3<f32> };
struct Flags { flags: vec4<u32> };
struct Cluster { count: u32, indices: array<u32, 32> };
struct LegacyDynamicLighting { diffuse_factor: vec3<f32>, wet_specular: vec3<f32> };
struct Settings { values: vec4<u32> };
struct Counts { rays: atomic<u32>, raster: atomic<u32> };
@group(0) @binding(0) var<uniform> settings: Settings;
@group(0) @binding(1) var<storage, read_write> results: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> counts: Counts;
var<private> rt_shadow_flags: Flags;
var<private> dynamic_lights: array<PointLight, 8>;
var<private> test_cluster: Cluster;
const ENABLE_RAY_TRACED_SHADOWS: bool = true;
fn rt_sample_count() -> u32 { return settings.values.z; }
fn rt_local_sample_count(light: PointLight) -> u32 {
    return select(1u, settings.values.z, light.shadow.w >= 0.5);
}
fn rt_emitter_seed(input: VertexOut) -> u32 { return u32(input.clip_position.x) * 3266489917u; }
fn rt_sample_local_emitter_seed(light: PointLight, input: VertexOut, index: u32, sample: u32, count: u32, seed: u32) -> PointLight {
    var sampled = light;
    sampled.color_intensity = vec4<f32>(light.color_intensity.rgb / f32(count), light.color_intensity.a);
    return sampled;
}
fn local_light_shadow_rgb(light: PointLight, p: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    atomicAdd(&counts.raster, 1u); return vec3<f32>(0.25);
}
fn rt_local_shadow_rgb(light: PointLight, p: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    atomicAdd(&counts.rays, 1u); return light.shape.xyz;
}
fn rt_rate_local_shadow_rgb(light: PointLight, input: VertexOut, normal: vec3<f32>, slot: u32, sample: u32) -> vec3<f32> {
    return rt_local_shadow_rgb(light, input.world_position, normal);
}
fn evaluate(input: VertexOut) -> LegacyDynamicLighting {
    var result: LegacyDynamicLighting;
    let cluster = test_cluster;
    let normal = vec3<f32>(0.0, 0.0, 1.0);
    for (var i = 0u; i < min(cluster.count, 32u); i += 1u) {
        let light = dynamic_lights[cluster.indices[i]];
        if (light.color_intensity.a <= 0.0) { continue; }
        let shadow = rt_local_shadow_rgb(light, input.world_position, normal);
        result.diffuse_factor += light.color_intensity.rgb * light.color_intensity.a * shadow;
        if (light.position_radius.x < 0.0) { continue; }
        result.wet_specular += light.color_intensity.rgb * light.color_intensity.a * shadow * 0.25;
    }
    return result;
}
"#);
    source = rt_sampled_light_loops(&source).unwrap();
    source.push_str(include_str!("../../rt_solari_ris.wgsl"));
    source.push_str(r#"
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    rt_shadow_flags.flags.w = settings.values.x;
    test_cluster.count = settings.values.y;
    for (var i = 0u; i < test_cluster.count; i += 1u) {
        test_cluster.indices[i] = i;
        var light: PointLight;
        light.color_intensity = vec4<f32>(1.0);
        light.shadow.w = 1.0;
        light.shape = vec4<f32>(1.0);
        if (settings.values.w != 0u) {
            light.color_intensity = vec4<f32>(f32(i + 1u), f32((i % 3u) + 1u), 0.5, 1.0);
            light.shape = vec4<f32>(select(0.0, 1.0, i % 2u == 0u), 0.5, 0.25, 0.0);
        }
        if (settings.values.w == 2u && (i == 0u || i + 1u == test_cluster.count)) { light.shadow.w = 0.0; }
        if (settings.values.w == 3u && i % 2u == 0u) { light.color_intensity.a = 0.0; }
        if (settings.values.w == 4u) { light.color_intensity.r *= -1.0; }
        dynamic_lights[i] = light;
    }
    var input: VertexOut;
    input.clip_position.x = f32(id.x);
    let lighting = evaluate(input);
    results[id.x] = vec4<f32>(lighting.diffuse_factor + lighting.wet_specular, f32(rt_ray_rate()));
}
"#);
    source
}

#[test]
fn solari_ris_counted_shader_validates() {
    let source = counted_source();
    let module = wgpu::naga::front::wgsl::parse_str(&source)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
    wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(),
        wgpu::naga::valid::Capabilities::all()).validate(&module)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
}

fn readback(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Vec<u8> {
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
    device.poll(wgpu::PollType::Wait {
        submission_index: None, timeout: Some(std::time::Duration::from_secs(30)),
    }).unwrap();
    rx.recv().unwrap().unwrap();
    let bytes = buffer.slice(..).get_mapped_range().unwrap().to_vec();
    buffer.unmap();
    bytes
}

#[test]
#[ignore = "requires a Vulkan GPU"]
fn solari_ris_gpu_reduces_rays_and_preserves_energy() {
    crate::renderer::block_on(async {
        use wgpu::util::DeviceExt;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        println!("Solari RIS GPU: {:?}", adapter.get_info());
        let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("counted production Solari RIS"),
            source: wgpu::ShaderSource::Wgsl(counted_source().into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None, layout: None, module: &shader, entry_point: Some("main"),
            compilation_options: Default::default(), cache: None,
        });
        const PIXELS: u32 = 16384;
        let settings = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None, contents: bytemuck::bytes_of(&[1u32; 4]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: None, size: u64::from(PIXELS) * 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let counts = device.create_buffer(&wgpu::BufferDescriptor {
            label: None, size: 8,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None, size: u64::from(PIXELS) * 16 + 8,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None, layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: settings.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: output.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: counts.as_entire_binding() },
            ],
        });
        for lights in [1u32, 3, 5, 8] {
            for samples in [1u32, 2, 4] {
                for scenario in 0..5u32 {
                    let mut expected = [0.0f64; 3];
                    let mut static_lights = 0;
                    for i in 0..lights {
                        let color = if scenario == 0 { [1.0; 3] }
                            else { [f64::from(i + 1), f64::from(i % 3 + 1), 0.5] };
                        let is_static = scenario == 2 && (i == 0 || i + 1 == lights);
                        static_lights += u32::from(is_static);
                        if scenario == 3 && i % 2 == 0 { continue; }
                        let visibility = if is_static { [0.25; 3] } else if scenario == 0 { [1.0; 3] }
                            else { [if i % 2 == 0 { 1.0 } else { 0.0 }, 0.5, 0.25] };
                        for channel in 0..3 {
                            let sign = if scenario == 4 && channel == 0 { -1.0 } else { 1.0 };
                            expected[channel] += 1.25 * sign * color[channel] * visibility[channel];
                        }
                    }
                    for rate in [1u32, 2, 4] {
                        queue.write_buffer(&settings, 0, bytemuck::bytes_of(&[rate, lights, samples, scenario]));
                        queue.write_buffer(&counts, 0, &[0; 8]);
                        let mut encoder = device.create_command_encoder(&Default::default());
                        {
                            let mut pass = encoder.begin_compute_pass(&Default::default());
                            pass.set_pipeline(&pipeline);
                            pass.set_bind_group(0, &group, &[]);
                            pass.dispatch_workgroups(PIXELS / 64, 1, 1);
                        }
                        let output_bytes = u64::from(PIXELS) * 16;
                        encoder.copy_buffer_to_buffer(&output, 0, &readback_buffer, 0, output_bytes);
                        encoder.copy_buffer_to_buffer(&counts, 0, &readback_buffer, output_bytes, 8);
                        queue.submit([encoder.finish()]);
                        let bytes = readback(&device, &readback_buffer);
                        let mut average = [0.0f64; 3];
                        for pixel in bytes[..output_bytes as usize].chunks_exact(16) {
                            let value = bytemuck::pod_read_unaligned::<[f32; 4]>(pixel);
                            for channel in 0..3 { average[channel] += f64::from(value[channel]) / f64::from(PIXELS); }
                            assert_eq!(value[3], rate as f32);
                        }
                        for channel in 0..3 {
                            let tolerance = if rate == 1 || scenario == 0 { 0.0001 } else { expected[channel].abs().max(1.0) * 0.035 };
                            assert!((average[channel] - expected[channel]).abs() < tolerance,
                                "energy: lights={lights} samples={samples} scenario={scenario} rate={rate}: {average:?} != {expected:?}");
                        }
                        let actual = bytemuck::pod_read_unaligned::<[u32; 2]>(&bytes[output_bytes as usize..]);
                        let ray_bound = ((lights - static_lights) * samples).div_ceil(rate) * PIXELS;
                        assert!(actual[0] <= ray_bound, "ray budget: {actual:?} > {ray_bound}");
                        assert_eq!(actual[1], static_lights * PIXELS, "static lights retain raster shadows");
                        if scenario == 0 { assert_eq!(actual[0], ray_bound, "exact Full/Half/Quarter ray counts"); }
                    }
                }
            }
        }
    });
}
