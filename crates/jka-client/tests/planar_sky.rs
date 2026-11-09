#[path = "../src/renderer/reflections/planar_target.rs"]
mod planar_target;

#[test]
fn lake_planar_sky_survives_sdr_and_hdr_targets() {
    // Exercise the real capture allocation and the lake's actual sampling
    // helper. An SDR scene must not clamp away the negative sky marker.
    futures_lite::future::block_on(async {
        let instance = wgpu::Instance::default();
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: adapter.limits(), ..Default::default()
        }).await.unwrap();
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        // Both full lake materials must retain compatible SDR/HDR pipelines;
        // the capture's floating-point format is independent of these targets.
        let _sdr_lake = blue_water_gpu::lakes::LakePipeline::new(&device, wgpu::TextureFormat::Bgra8UnormSrgb, 1);
        let _hdr_lake = blue_water_gpu::lakes::LakePipeline::new(&device, wgpu::TextureFormat::Rgba16Float, 1);
        let capture = planar_target::color_texture(&device, 4, 1, 4);
        let capture_view = capture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2), array_layer_count: Some(1), ..Default::default()
        });
        let capture_array_view = capture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default()
        });
        let capture_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let helper = include_str!("../../blue-water-gpu/src/lake_deformation.wgsl");
        let helper = &helper[helper.find("@group(1) @binding(12)").unwrap()
            ..helper.find("// Lake-local SSR").unwrap()];
        let source = format!(r#"
struct Lake {{ tint: vec4<f32>, reflection: vec4<f32> }}
const lake = Lake(vec4<f32>(1.0), vec4<f32>(0.0, 2.0, 0.0, 0.0));
struct Camera {{ view_projection: mat4x4<f32> }}
const camera = Camera(mat4x4<f32>(vec4(1.,0.,0.,0.), vec4(0.,1.,0.,0.),
    vec4(0.,0.,1.,0.), vec4(0.,0.,0.,1.)));
fn jka_lake_ssr(world: vec3<f32>, up: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {{ return fallback; }}
{helper}
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {{
    let p = array<vec2<f32>,3>(vec2(-1.,-1.), vec2(3.,-1.), vec2(-1.,3.));
    return vec4(p[i], 0., 1.);
}}
@fragment fn opaque() -> @location(0) vec4<f32> {{ return vec4(1.,0.,0.,0.); }}
@fragment fn additive() -> @location(0) vec4<f32> {{ return vec4(0.,0.25,0.,1.); }}
@fragment fn water(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {{
    let world = vec3<f32>(p.x * 0.5 - 1.0, 0.0, 0.0);
    return vec4(jka_lake_reflection(world, vec3<f32>(0.0), vec3<f32>(0.,0.,1.)), 1.0);
}}
"#, helper = helper.replace("@group(1)", "@group(0)"));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("lake planar sky regression"), source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = |entry: &str, format, blend| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(entry), layout: None,
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"),
                compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(entry),
                compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState {
                    format, blend, write_mask: wgpu::ColorWrites::ALL,
                })] }), primitive: Default::default(), depth_stencil: None,
            multisample: Default::default(), multiview_mask: None, cache: None,
        });
        let format = planar_target::COLOR_FORMAT;
        let opaque = pipeline("opaque", format, None);
        let additive = pipeline("additive", format, Some(wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
        }));
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &capture_view, depth_slice: None, resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0., g: 0., b: 0., a: -1.,
                    }), store: wgpu::StoreOp::Store },
                })], ..Default::default()
            });
            pass.set_pipeline(&additive);
            pass.set_scissor_rect(1, 0, 1, 1);
            pass.draw(0..3, 0..1);
            pass.set_pipeline(&opaque);
            pass.set_scissor_rect(2, 0, 2, 1);
            pass.draw(0..3, 0..1);
        }
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None, size: 512, mapped_at_creation: false,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        });
        // Resolve the same floating-point reflection into each main-view
        // format and inspect its stored sRGB8 or half-float pixels.
        for (i, format) in [wgpu::TextureFormat::Bgra8UnormSrgb, wgpu::TextureFormat::Rgba16Float].into_iter().enumerate() {
            let water = pipeline("water", format, None);
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None, layout: &water.get_bind_group_layout(0), entries: &[
                    wgpu::BindGroupEntry { binding: 12, resource: wgpu::BindingResource::TextureView(&capture_array_view) },
                    wgpu::BindGroupEntry { binding: 13, resource: wgpu::BindingResource::Sampler(&capture_sampler) },
                ],
            });
            let target = device.create_texture(&wgpu::TextureDescriptor {
                label: None, size: wgpu::Extent3d { width: 4, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
                format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = target.create_view(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view,
                        depth_slice: None, resolve_target: None, ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store,
                        } })], ..Default::default()
                });
                pass.set_pipeline(&water); pass.set_bind_group(0, &group, &[]); pass.draw(0..3, 0..1);
            }
            encoder.copy_texture_to_buffer(target.as_image_copy(), wgpu::TexelCopyBufferInfo {
                buffer: &readback, layout: wgpu::TexelCopyBufferLayout {
                    offset: i as u64 * 256, bytes_per_row: Some(256), rows_per_image: Some(1),
                },
            }, target.size());
        }
        queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
        device.poll(wgpu::PollType::Wait { submission_index: None,
            timeout: Some(std::time::Duration::from_secs(30)) }).unwrap();
        rx.recv().unwrap().unwrap();
        let bytes = slice.get_mapped_range().unwrap();
        assert_eq!(&bytes[0..4], &[255,0,0,255], "SDR sky must be the blue fallback");
        assert_eq!(bytes[4], 255, "additive light must retain the blue sky");
        assert!(bytes[5] > 100, "additive light must remain visible");
        assert_eq!(&bytes[8..12], &[0,0,255,255], "opaque red geometry must cover sky even with alpha=0");
        let hdr: &[u16] = bytemuck::cast_slice(&bytes[256..288]);
        assert_eq!(&hdr[0..4], &[0,0,0x3c00,0x3c00], "HDR sky must be the blue fallback");
        assert_eq!(&hdr[4..8], &[0,0x3400,0x3c00,0x3c00], "HDR additive light must retain sky");
        assert_eq!(&hdr[8..12], &[0x3c00,0,0,0x3c00], "HDR geometry must cover sky");
        drop(bytes); readback.unmap();
        assert!(scope.pop().await.is_none(), "planar capture and both destination formats must validate");
    });
}

#[test]
fn blue_water_and_hybrid_sky_reflection_shaders_validate() {
    use blue_water_gpu::shader_composer::*;
    for probe in [0, COST_PROBE_SSR, SURFACE_FAST_HYBRID, SURFACE_FAST_HYBRID | COST_PROBE_SSR] {
        let mut composer = composer_with_surface_probes(probe, 1).expect("water shader modules");
        let module = jka_surface_module_with_probe(&mut composer, 16, 0, probe)
            .unwrap_or_else(|error| panic!("water reflection {probe:#x}: {}", error.emit_to_string(&composer)));
        wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all()).validate(&module).expect("water reflection shader validates");
    }
}
