use super::*;

// Execute the production receiver dispatch with counted raster/RT oracles.
// Distinct visibility values catch static lights accidentally reading RT history
// or losing their raster shadow when transient lighting enables hardware RT.
fn routing_source(glass: bool) -> String {
    let donor = ray_traced_world_shader_source_with_features(
        include_str!("../../bsp.wgsl"), ReceiverFeatures { glass },
    ).unwrap();
    let mut source = String::new();
    source.push_str(rt_models::declaration(&donor, "struct PointLight ").unwrap());
    source.push_str(r#";
struct VertexOut { clip_position: vec4<f32>, world_position: vec3<f32> };
struct Camera { render_flags: vec4<u32> };
struct Lighting { feature_flags: vec4<u32>, baked_sun_debug_mode: vec4<u32> };
struct Flags { flags: vec4<u32> };
struct Counts { rays: atomic<u32>, cache_reads: atomic<u32> };
var<private> camera: Camera;
var<private> lighting_settings: Lighting;
var<private> rt_shadow_flags: Flags;
const ENABLE_RAY_TRACED_SHADOWS: bool = true;
const ENABLE_LOCAL_SHADOWS: bool = true;
const ENABLE_MAP_LIGHT_SIMULATION: bool = false;
@group(0) @binding(0) var<storage, read_write> results: array<vec4<f32>>;
@group(0) @binding(1) var<storage, read_write> counts: Counts;
fn sample_local_shadow_visibility(light: PointLight, p: vec3<f32>, n: vec3<f32>) -> f32 { return 0.25; }
fn baked_local_moving_visibility(light: PointLight, p: vec3<f32>, n: vec3<f32>) -> f32 { return 0.5; }
fn raster_glass_throughput(light: PointLight, p: vec3<f32>) -> vec3<f32> { return vec3<f32>(0.5, 0.25, 1.0); }
fn rt_trace_local_visibility(light: PointLight, p: vec3<f32>, n: vec3<f32>) -> f32 {
    atomicAdd(&counts.rays, 1u); return 0.75;
}
fn rt_trace_shadow_query(p: vec3<f32>, d: vec3<f32>, t: f32, colored: bool) -> vec3<f32> {
    atomicAdd(&counts.rays, 1u); return vec3<f32>(0.75);
}
fn q3map_surface_angle(light: PointLight, cosine: f32) -> f32 { return cosine; }
fn q3map_light_scalar(light: PointLight, distance: f32, angle: f32) -> f32 { return 1.0; }
fn q3map_fast_contribution_visible(scalar: f32) -> bool { return true; }
fn emitter_visibility(light: PointLight, d: vec3<f32>) -> f32 { return 1.0; }
fn rt_rate_local_visibility(input: VertexOut, n: vec3<f32>, slot: u32, sample: u32) -> f32 {
    atomicAdd(&counts.cache_reads, 1u); return 0.125;
}
"#);
    for name in ["ray_traced_local_shadow_visibility", "rt_local_shadow_rgb",
        "local_light_shadow_visibility", "local_light_shadow_rgb", "rt_rate_local_shadow_rgb"] {
        source.push_str(rt_models::declaration(&donor, &format!("fn {name}(")).unwrap());
        source.push('\n');
    }
    source.push_str(r#"
@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    var light: PointLight;
    light.position_radius = vec4<f32>(0.0, 0.0, 10.0, 100.0);
    light.color_intensity = vec4<f32>(1.0);
    // Static point, authored area, promoted area, then transient FX.
    if (id.x == 1u || id.x == 2u) { light.emitter.w = 1.0; }
    if (id.x == 2u) { light.shape.z = 1.0; }
    if (id.x == 3u) { light.shadow.w = 1.0; }
    var input: VertexOut;
    let normal = vec3<f32>(0.0, 0.0, 1.0);
    let direct = local_light_shadow_rgb(light, input.world_position, normal);
    let cached = rt_rate_local_shadow_rgb(light, input, normal, id.x, 0u);
    results[id.x] = vec4<f32>(direct, cached.x);
}
"#);
    source
}

#[test]
fn transient_rt_dispatch_shader_validates() {
    for glass in [false, true] {
        let source = routing_source(glass);
        let module = wgpu::naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
        wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all()).validate(&module)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
    }
}

#[test]
#[ignore = "requires a Vulkan GPU"]
fn transient_rt_gpu_routes_static_lights_to_raster() {
    crate::renderer::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
        for glass in [false, true] {
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("transient RT routing"),
                source: wgpu::ShaderSource::Wgsl(routing_source(glass).into()),
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: None, layout: None, module: &shader, entry_point: Some("main"),
                compilation_options: Default::default(), cache: None,
            });
            let results = device.create_buffer(&wgpu::BufferDescriptor {
                label: None, size: 64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let counts = device.create_buffer(&wgpu::BufferDescriptor {
                label: None, size: 8,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: None, size: 72,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None, layout: &pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: results.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: counts.as_entire_binding() },
                ],
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.dispatch_workgroups(4, 1, 1);
            }
            encoder.copy_buffer_to_buffer(&results, 0, &readback, 0, 64);
            encoder.copy_buffer_to_buffer(&counts, 0, &readback, 64, 8);
            queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            readback.slice(..).map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
            device.poll(wgpu::PollType::Wait {
                submission_index: None, timeout: Some(std::time::Duration::from_secs(30)),
            }).unwrap();
            rx.recv().unwrap().unwrap();
            let bytes = readback.slice(..).get_mapped_range().unwrap();
            let expected_static = if glass { [0.125, 0.0625, 0.25, 0.125] }
                else { [0.25; 4] };
            for (index, record) in bytes[..64].chunks_exact(16).enumerate() {
                let actual = bytemuck::pod_read_unaligned::<[f32; 4]>(record);
                assert_eq!(actual, if index == 3 { [0.75; 4] }
                    else { expected_static }, "glass={glass} light={index}");
            }
            assert_eq!(bytemuck::pod_read_unaligned::<[u32; 2]>(&bytes[64..72]), [2, 0],
                "only transient light calls trace; visibility history is bypassed; glass={glass}");
        }
    });
}

#[test]
fn rt_dynamic_lights_preserve_raster_sun_receivers() {
    for base in [include_str!("../../bsp.wgsl"), include_str!("../../bsp_lean.wgsl")] {
        let base = base.replace("\r\n", "\n");
        let source = ray_traced_world_shader_source_with_features(&base, ReceiverFeatures { glass: true }).unwrap();
        for name in ["cascaded_shadow_visibility", "sun_shadow_transmission"] {
            let anchor = format!("fn {name}(");
            assert_eq!(rt_models::declaration(&source, &anchor).unwrap(),
                rt_models::declaration(&base, &anchor).unwrap());
        }
        assert!(!source.contains("ENABLE_RAY_TRACED_SUN"));
        assert!(!source.contains("rt_sun_shadow"));
        assert!(!source.contains("rt_cached_sun_visibility"));
    }
}
