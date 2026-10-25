use super::*;

fn capabilities(hdr: bool) -> wgpu::SurfaceCapabilities {
    let mut formats = vec![wgpu::SurfaceFormatCapabilities {
        format: wgpu::TextureFormat::Bgra8UnormSrgb,
        color_spaces: wgpu::SurfaceColorSpace::Srgb.to_color_spaces().unwrap(),
    }];
    if hdr {
        for (format, space) in [
            (
                wgpu::TextureFormat::Rgba16Float,
                wgpu::SurfaceColorSpace::ExtendedSrgbLinear,
            ),
            (
                wgpu::TextureFormat::Rgb10a2Unorm,
                wgpu::SurfaceColorSpace::Bt2100Pq,
            ),
        ] {
            formats.push(wgpu::SurfaceFormatCapabilities {
                format,
                color_spaces: space.to_color_spaces().unwrap(),
            });
        }
    }
    wgpu::SurfaceCapabilities {
        formats: vec![wgpu::TextureFormat::Bgra8UnormSrgb],
        format_capabilities: formats,
        present_modes: vec![wgpu::PresentMode::Fifo],
        alpha_modes: vec![wgpu::CompositeAlphaMode::Opaque],
        usages: wgpu::TextureUsages::RENDER_ATTACHMENT,
    }
}

#[test]
fn hdr_selection_uses_explicit_color_spaces_and_keeps_sdr_fallback() {
    let caps = capabilities(true);
    assert_eq!(select(&caps, 0, None).unwrap().encoding, 0);
    assert_eq!(select(&caps, 1, None).unwrap().encoding, 1);
    // HDR10 is absent from the legacy formats list, but present in explicit capabilities.
    assert_eq!(select(&caps, 2, None).unwrap().encoding, 2);
    assert_eq!(select(&caps, 3, None).unwrap().encoding, 1);
    for requested in 1..=3 {
        assert_eq!(
            select(&capabilities(false), requested, None)
                .unwrap()
                .encoding,
            0
        );
    }
    let info = wgpu::DisplayHdrInfo {
        coarse: Some(wgpu::DisplayCoarseRange {
            high_dynamic_range: Some(false),
            gamut: None,
        }),
        ..Default::default()
    };
    assert_eq!(select(&caps, 1, Some(&info)).unwrap().encoding, 0);
    assert_eq!(select(&caps, 2, Some(&info)).unwrap().encoding, 0);
    let mut hdr_only = capabilities(true);
    hdr_only.formats.clear();
    hdr_only.format_capabilities.remove(0);
    assert_eq!(select(&hdr_only, 2, None).unwrap().encoding, 2);
    assert!(select(&hdr_only, 0, None).is_none());
}

#[test]
fn hdr_mode_availability_matches_runtime_selection_and_windows_hdr_state() {
    assert_eq!(available_modes(&capabilities(true), None), 0b1111);
    assert_eq!(available_modes(&capabilities(false), None), 0b0001);
    let sdr_display = wgpu::DisplayHdrInfo {
        coarse: Some(wgpu::DisplayCoarseRange {
            high_dynamic_range: Some(false),
            gamut: None,
        }),
        ..Default::default()
    };
    assert_eq!(available_modes(&capabilities(true), Some(&sdr_display)), 0b0001);
    let mut pq_only = capabilities(true);
    pq_only.format_capabilities.retain(|c| c.format != wgpu::TextureFormat::Rgba16Float);
    assert_eq!(available_modes(&pq_only, None), 0b0111);
    let mut scrgb_only = capabilities(true);
    scrgb_only.format_capabilities.retain(|c| c.format != wgpu::TextureFormat::Rgb10a2Unorm);
    assert_eq!(available_modes(&scrgb_only, None), 0b1011);
}

#[test]
fn hdr_display_shader_validates() {
    let module = naga::front::wgsl::parse_str(include_str!("../../hdr_output.wgsl")).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap();
}

#[test]
#[ignore = "requires a Vulkan GPU"]
fn studio_alpha_hdr_capture_preserves_emission_and_translucent_edges() {
    fn decode(c:u8) -> f32 {
        let c=c as f32/255.0;
        if c<=0.04045 {c/12.92}else{((c+0.055)/1.055).powf(2.4)}
    }
    crate::renderer::block_on(async {
        let instance=wgpu::Instance::new(wgpu::InstanceDescriptor {backends:wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()});
        let adapter=instance.request_adapter(&Default::default()).await.unwrap();
        let (device,queue)=adapter.request_device(&Default::default()).await.unwrap();
        let selection=select(&capabilities(true),1,None).unwrap();
        let mut output=HdrOutput::new(&device,selection,1,1,1);
        let surface=target(&device,1,1,selection.format);
        let surface_view=surface.create_view(&Default::default());
        for display_rolloff in [0.0f32,1.0] {
            output.write_settings(&queue,200.0,1000.0,display_rolloff);
            // SDR file capture has its own fixed tone-map settings, independent
            // of the monitor's HDR rolloff preference.
            let strength=1.0f32;
            for input in [[0.0,0.0,0.0,0.0],[0.05,0.0,0.0,0.0],[0.1,0.0,0.2,0.0],
                [0.05,0.125,0.225,0.25],[0.02,0.02,0.02,1.0],[4.0,4.0,4.0,0.0]] {
                let mut encoder=device.create_command_encoder(&Default::default());
                {
                    let _pass=encoder.begin_render_pass(&wgpu::RenderPassDescriptor {label:None,
                        color_attachments:&[Some(wgpu::RenderPassColorAttachment {view:&output.view,depth_slice:None,resolve_target:None,
                            ops:wgpu::Operations {load:wgpu::LoadOp::Clear(wgpu::Color {r:input[0] as f64,g:input[1] as f64,b:input[2] as f64,a:input[3] as f64}),store:wgpu::StoreOp::Store}})],
                        depth_stencil_attachment:None,timestamp_writes:None,occlusion_query_set:None,multiview_mask:None});
                }
                queue.submit([encoder.finish()]);
                output.submit_with_alpha(&device,&queue,&surface_view,true,true);
                let pixel=read(&device,&queue,&output.capture,4); // BGRA, straight sRGB.
                let alpha=pixel[3] as f32/255.0;
                assert!(alpha+0.001>=input[3]);
                let reference=if input[3]>0.00001 {input[3]}else{1.0};
                for channel in 0..3 {
                    let linear=input[channel]/reference;
                    let mapped=(linear*(2.51*linear+0.03)/(linear*(2.43*linear+0.59)+0.14)).clamp(0.0,1.0);
                    let expected=((linear*(1.0-strength)+mapped*strength)*reference).clamp(0.0,1.0);
                    let actual=decode(pixel[2-channel])*alpha;
                    assert!((actual-expected).abs()<0.008,"input={input:?}, output={pixel:?}, strength={strength}");
                }
                if input==[0.0;4] {assert_eq!(pixel,vec![0,0,0,0]);}
                if input[0]==0.05 && input[3]==0.0 {assert!(pixel[3]>0 && pixel[3]<20);}
                if input[3]==1.0 {assert_eq!(pixel[3],255);}
            }
        }
    });
}

fn read(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    bytes_per_pixel: u32,
) -> Vec<u8> {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(1),
            },
        },
        texture.size(),
    );
    let submission = queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(Duration::from_secs(30)),
        })
        .unwrap();
    rx.recv_timeout(Duration::from_secs(30)).unwrap().unwrap();
    let result = buffer.slice(..).get_mapped_range().unwrap()[..bytes_per_pixel as usize].to_vec();
    buffer.unmap();
    result
}

fn half(bits: u16) -> f32 {
    let sign = if bits & 0x8000 == 0 { 1.0 } else { -1.0 };
    let exp = ((bits >> 10) & 31) as i32;
    let mantissa = (bits & 1023) as f32;
    if exp == 0 {
        sign * mantissa * 2.0f32.powi(-24)
    } else {
        sign * (1.0 + mantissa / 1024.0) * 2.0f32.powi(exp - 15)
    }
}

fn reference_pq(nits: f64) -> f64 {
    let p = (nits / 10000.0).powf(2610.0 / 16384.0);
    ((3424.0 / 4096.0 + 2413.0 / 128.0 * p) / (1.0 + 2392.0 / 128.0 * p)).powf(2523.0 / 32.0)
}

fn exercise(backend: wgpu::Backends) {
    crate::renderer::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: backend,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
        println!("Native HDR transfer test: {:?}", adapter.get_info());
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        for selection in [
            select(&capabilities(true), 3, None).unwrap(),
            select(&capabilities(true), 2, None).unwrap(),
        ] {
            let mut output = HdrOutput::new(&device, selection, 1, 1, 1);
            output.detected_peak = Some(600.0);
            output.write_settings(&queue, 200.0, 0.0, 0.0);
            assert_eq!(output.peak, 600.0);
            output.write_settings(&queue, 500.0, 100.0, 0.0);
            assert_eq!(
                output.paper_white, 100.0,
                "reference white cannot exceed display peak"
            );
            output.write_settings(&queue, 200.0, 1000.0, 0.0);
            let texture = target(&device, 1, 1, selection.format);
            let view = texture.create_view(&Default::default());
            // Values above SDR white must survive scene rendering and upstream SMAA.
            for level in [0.0, 0.5, 1.0, 4.0, 40.0] {
                let mut aa = smaa::SmaaTarget::new(
                    &device,
                    &queue,
                    1,
                    1,
                    wgpu::TextureFormat::Rgba16Float,
                    smaa::SmaaMode::Smaa1X,
                );
                let frame = aa.start_frame(&device, &queue, &output.view);
                let mut encoder = device.create_command_encoder(&Default::default());
                {
                    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &frame,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color {
                                    r: level,
                                    g: level,
                                    b: level,
                                    a: 1.0,
                                }),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        ..Default::default()
                    });
                }
                queue.submit([encoder.finish()]);
                frame.resolve();
                output.submit(&device, &queue, &view, true);
                let nits = (level * 200.0).min(1000.0);
                if selection.encoding == 1 {
                    let bytes = read(&device, &queue, &texture, 8);
                    for channel in 0..3 {
                        let actual = half(u16::from_le_bytes([
                            bytes[channel * 2],
                            bytes[channel * 2 + 1],
                        ]));
                        assert!(
                            (actual as f64 - nits / 80.0).abs() < 0.01,
                            "scRGB {level}: {actual}"
                        );
                    }
                } else {
                    let bytes = read(&device, &queue, &texture, 4);
                    let packed = u32::from_le_bytes(bytes.try_into().unwrap());
                    for channel in 0..3 {
                        let actual = ((packed >> (channel * 10)) & 1023) as f64 / 1023.0;
                        assert!(
                            (actual - reference_pq(nits)).abs() < 0.002,
                            "PQ {level}: {actual}"
                        );
                    }
                }
                let capture = read(&device, &queue, &output.capture, 4);
                assert_eq!(capture[3], 255, "SDR screenshot alpha");
                if level == 0.0 {
                    assert_eq!(&capture[..3], &[0, 0, 0]);
                }
            }
            // UI diffuse white must stay at paper white, independent of HDR headroom.
            let mut encoder = device.create_command_encoder(&Default::default());
            for (view, color) in [
                (&output.view, wgpu::Color::BLACK),
                (&output.ui_view, wgpu::Color::WHITE),
            ] {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(color),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
            }
            output.composite_ui(&mut encoder, &output.view);
            queue.submit([encoder.finish()]);
            output.submit(&device, &queue, &view, false);
            let bytes = read(
                &device,
                &queue,
                &texture,
                if selection.encoding == 1 { 8 } else { 4 },
            );
            if selection.encoding == 1 {
                assert_eq!(half(u16::from_le_bytes([bytes[0], bytes[1]])), 2.5);
            } else {
                assert!(
                    ((u32::from_le_bytes(bytes.try_into().unwrap()) & 1023) as f64 / 1023.0
                        - reference_pq(200.0))
                    .abs()
                        < 0.002
                );
            }
            // Premultiplied gamma-space translucent UI must decode after
            // unpremultiplication, otherwise its white becomes too dark.
            let mut encoder = device.create_command_encoder(&Default::default());
            for (destination, color) in [
                (
                    &output.view,
                    wgpu::Color {
                        r: 4.0,
                        g: 4.0,
                        b: 4.0,
                        a: 1.0,
                    },
                ),
                (
                    &output.ui_view,
                    wgpu::Color {
                        r: 128.0 / 255.0,
                        g: 128.0 / 255.0,
                        b: 128.0 / 255.0,
                        a: 128.0 / 255.0,
                    },
                ),
            ] {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: destination,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(color),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
            }
            output.composite_ui(&mut encoder, &output.view);
            queue.submit([encoder.finish()]);
            output.submit(&device, &queue, &view, false);
            let bytes = read(
                &device,
                &queue,
                &texture,
                if selection.encoding == 1 { 8 } else { 4 },
            );
            let alpha = 128.0 / 255.0;
            let nits = (alpha + 4.0 * (1.0 - alpha)) * 200.0;
            if selection.encoding == 1 {
                let actual = half(u16::from_le_bytes([bytes[0], bytes[1]])) as f64;
                assert!(
                    (actual - nits / 80.0).abs() < 0.01,
                    "translucent UI scRGB actual={actual}, expected={}",
                    nits / 80.0
                );
            } else {
                assert!(
                    ((u32::from_le_bytes(bytes.try_into().unwrap()) & 1023) as f64 / 1023.0
                        - reference_pq(nits))
                    .abs()
                        < 0.002
                );
            }
        }
        assert!(
            scope.pop().await.is_none(),
            "HDR output GPU validation error"
        );
    });
}

#[test]
#[ignore = "requires a Vulkan GPU"]
fn native_hdr_vulkan_transfer_ui_smaa_and_capture() {
    exercise(wgpu::Backends::VULKAN);
}

#[test]
#[ignore = "requires a DX12 GPU"]
fn native_hdr_dx12_transfer_ui_smaa_and_capture() {
    exercise(wgpu::Backends::DX12);
}

#[cfg(windows)]
#[test]
#[ignore = "requires a Windows desktop and native GPU surfaces"]
fn native_hdr_surface_capabilities_and_configuration() {
    use winit::platform::windows::EventLoopBuilderExtWindows;
    let event_loop = winit::event_loop::EventLoop::builder()
        .with_any_thread(true)
        .build()
        .unwrap();
    #[allow(deprecated)]
    let window = std::sync::Arc::new(
        event_loop
            .create_window(
                winit::window::Window::default_attributes()
                    .with_visible(false)
                    .with_inner_size(winit::dpi::PhysicalSize::new(64, 64)),
            )
            .unwrap(),
    );
    for backend in [wgpu::Backends::VULKAN, wgpu::Backends::DX12] {
        crate::renderer::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: backend,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let surface = instance.create_surface(window.clone()).unwrap();
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    compatible_surface: Some(&surface),
                    ..Default::default()
                })
                .await
                .unwrap();
            let (device, _queue) = adapter.request_device(&Default::default()).await.unwrap();
            let caps = surface.get_capabilities(&adapter);
            let info = surface.display_hdr_info(&adapter);
            println!(
                "{backend:?} live display: {info:?}; formats: {:?}",
                caps.format_capabilities
            );
            for requested in 0..=3 {
                let selection = select(&caps, requested, Some(&info)).unwrap();
                let mut config = surface.get_default_config(&adapter, 64, 64).unwrap();
                config.format = selection.format;
                config.color_space = selection.color_space;
                let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
                surface.configure(&device, &config);
                let error = scope.pop().await;
                assert!(
                    error.is_none(),
                    "native output configure {requested}: {selection:?}: {error:?}"
                );
                println!("{backend:?} requested {requested}: {selection:?}");
            }
            // Validate the actual renderer's FP16 initialization and egui path,
            // not only the standalone output stage. No user assets/config are written.
            let base =
                std::env::temp_dir().join(format!("DinurdoJK-hdr-init-{}", std::process::id()));
            std::fs::create_dir_all(&base).unwrap();
            let mut renderer = crate::renderer::Renderer::new(
                window.clone(),
                instance,
                surface,
                &base,
                None,
                1,
                1,
                "auto",
                0,
            )
            .await
            .unwrap();
            renderer.set_post_effects(crate::renderer::PostEffects {
                hdr_paper_white: 200.0,
                hdr_peak: 0.0,
                ..Default::default()
            });
            // SDR fallback must not create an FP16 compositor or output pass.
            if renderer.hdr_output.is_none() {
                assert_ne!(renderer.config.format, wgpu::TextureFormat::Rgba16Float);
                println!("{backend:?} SDR fallback: no HDR compositor allocated");
                return;
            }
            assert_eq!(renderer.config.format, wgpu::TextureFormat::Rgba16Float);
            assert!(renderer.hdr_enabled);
            let scope = renderer
                .device
                .push_error_scope(wgpu::ErrorFilter::Validation);
            let scene_view = renderer.hdr_output.as_ref().unwrap().view.clone();
            let mut encoder = renderer.device.create_command_encoder(&Default::default());
            {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &scene_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 4.0,
                                g: 4.0,
                                b: 4.0,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
            }
            renderer.queue.submit([encoder.finish()]);
            let ctx = egui::Context::default();
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(64.0, 64.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    let painter = ui.ctx().layer_painter(egui::LayerId::background());
                    painter.rect_filled(
                        egui::Rect::from_min_max(egui::pos2(-10.0, -10.0), egui::pos2(80.0, 80.0)),
                        0.0,
                        egui::Color32::WHITE,
                    );
                    painter.text(
                        egui::pos2(30.0, 30.0),
                        egui::Align2::CENTER_CENTER,
                        "HDR",
                        egui::FontId::default(),
                        egui::Color32::WHITE,
                    );
                },
            );
            renderer.set_egui(Some(crate::renderer::EguiRenderData {
                paint_jobs: ctx.tessellate(output.shapes, output.pixels_per_point),
                textures_delta: output.textures_delta,
                pixels_per_point: output.pixels_per_point,
                present_token: None,
            }));
            renderer.submit_egui(&scene_view);
            let display = renderer.hdr_output.as_ref().unwrap();
            let pixel = target(&renderer.device, 1, 1, display.selection.format);
            display.submit(
                &renderer.device,
                &renderer.queue,
                &pixel.create_view(&Default::default()),
                false,
            );
            let bytes = read(
                &renderer.device,
                &renderer.queue,
                &pixel,
                if display.selection.encoding == 1 {
                    8
                } else {
                    4
                },
            );
            if display.selection.encoding == 1 {
                let actual = half(u16::from_le_bytes([bytes[0], bytes[1]]));
                assert!((actual - display.paper_white / 80.0).abs() < 0.01);
            } else if display.selection.encoding == 2 {
                let actual = (u32::from_le_bytes(bytes.try_into().unwrap()) & 1023) as f64 / 1023.0;
                assert!((actual - reference_pq(display.paper_white as f64)).abs() < 0.002);
            } else {
                assert!(
                    bytes[0] > 225 && bytes[0] < 236,
                    "UI white must replace the HDR scene before SDR fallback: {bytes:?}"
                );
            }
            let error = scope.pop().await;
            assert!(error.is_none(), "renderer HDR/egui validation: {error:?}");
            renderer.resize(winit::dpi::PhysicalSize::new(64, 64));
            println!("{backend:?} actual FP16 renderer initialization and egui composition passed");
        });
    }
}

#[test]
fn native_output_compositor_only_for_native_encodings() {
    for encoding in 0..=3 {
        let selection = super::Selection {
            format: if encoding == 1 { wgpu::TextureFormat::Rgba16Float } else { wgpu::TextureFormat::Bgra8Unorm },
            color_space: wgpu::SurfaceColorSpace::Srgb,
            encoding,
        };
        assert_eq!(super::requires_compositor(selection), matches!(encoding, 1 | 2));
    }
}
