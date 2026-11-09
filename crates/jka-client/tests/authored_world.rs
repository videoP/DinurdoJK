/// Exercise the production BSP source modulation and fused weighting helpers
/// against the fixed-function display-space equation, through real SDR aliases.
#[test]
#[ignore = "requires a Vulkan GPU"]
fn authored_world_additive_alias_matches_tayst_equation() {
    futures_lite::future::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
        for bsp in [
            include_str!("../src/bsp.wgsl"),
            include_str!("../src/bsp_lean.wgsl"),
            include_str!("../src/bsp_fast.wgsl"),
        ] {
            let helpers = [
                "legacy_rgb_encode",
                "legacy_rgb_decode",
                "legacy_additive_source",
                "encoded_world_output",
                "legacy_rgb_output",
                "additive_stage_tint",
                "additive_weighted_sample",
            ]
            .map(|name| declaration(bsp, &format!("fn {name}(")).unwrap())
            .join("\n");
            let source = format!(
                r#"
struct Material {{
    header:vec4<u32>, color:vec4<f32>, wave_funcs:vec4<u32>,
    wave_rgb:vec4<f32>, wave_alpha:vec4<f32>,
}}
var<private> material:Material;
const MATERIAL_LEGACY_ADDITIVE_SRGB:u32 = 536870912u;
const LEGACY_WORLD_ENCODED_BLEND:bool = true;
fn material_flags()->u32 {{return material.header.z;}}
fn shader_stage_time()->f32 {{return 0.0;}}
fn additive_time(m:Material)->f32 {{return 0.0;}}
fn wave_value(f:u32,w:vec4<f32>,t:f32)->f32 {{return w.x;}}
{helpers}
fn setup(i:u32)->Material {{
    var m:Material;
    m.color=vec4<f32>(1.0);
    m.header.z=MATERIAL_LEGACY_ADDITIVE_SRGB | 1048576u;
    if(i==1u) {{m.header.z |= 134217728u; m.wave_rgb.x=0.6;}}
    if(i==2u) {{m.header.z=MATERIAL_LEGACY_ADDITIVE_SRGB | 2097152u;}}
    return m;
}}
fn texel(i:u32)->vec4<f32> {{
    return vec4<f32>(legacy_rgb_decode(vec3<f32>(0.07,0.11,0.17)*f32(i+1u)/3.0),0.8);
}}
fn ordinary(i:u32)->vec4<f32> {{
    material=setup(i);
    return legacy_rgb_output(texel(i)*additive_stage_tint(material,vec4<f32>(1.0,1.0,1.0,0.65)));
}}
fn weighted(i:u32)->vec4<f32> {{
    return additive_weighted_sample(texel(i),setup(i),vec4<f32>(1.0,1.0,1.0,0.65));
}}
@vertex fn vs(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32> {{
    let p=array<vec2<f32>,3>(vec2<f32>(-1.0,-1.0),vec2<f32>(3.0,-1.0),vec2<f32>(-1.0,3.0));
    return vec4<f32>(p[i],0.0,1.0);
}}
@fragment fn fs0()->@location(0) vec4<f32> {{return ordinary(0u);}}
@fragment fn fs1()->@location(0) vec4<f32> {{return ordinary(1u);}}
@fragment fn fs2()->@location(0) vec4<f32> {{return ordinary(2u);}}
@fragment fn fused()->@location(0) vec4<f32> {{return weighted(0u)+weighted(1u)+weighted(2u);}}
@fragment fn fog()->@location(0) vec4<f32> {{
    // Fog has no additive material flag or rgbGen wave. It must blend in the
    // same attachment without inheriting either source modulation.
    material.header.z=0u;
    return legacy_rgb_output(vec4<f32>(legacy_rgb_decode(vec3<f32>(0.2,0.3,0.4)),0.25));
}}
@fragment fn background(@builtin(sample_index) i:u32)->@location(0) vec4<f32> {{
    return vec4<f32>(legacy_rgb_decode(vec3<f32>(0.35,0.45,0.55)+0.14*(f32(i)-1.5)),1.0);
}}
@fragment fn marker(@builtin(sample_index) i:u32)->@location(0) vec4<f32> {{
    if(i!=0u) {{discard;}} return vec4<f32>(0.0,0.0,0.0,1.0);
}}
"#
            );
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("authored BSP blend GPU regression"),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            for samples in [1, 4] {
                for format in [
                    wgpu::TextureFormat::Rgba8UnormSrgb,
                    wgpu::TextureFormat::Rgba16Float,
                ] {
                    let encoded_format = if format.is_srgb() {
                        format.remove_srgb_suffix()
                    } else {
                        format
                    };
                    let pipeline = |entry, fused| {
                        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                            label: Some(entry),
                            layout: None,
                            vertex: wgpu::VertexState {
                                module: &shader,
                                entry_point: Some("vs"),
                                compilation_options: Default::default(),
                                buffers: &[],
                            },
                            primitive: Default::default(),
                            depth_stencil: None,
                            multisample: wgpu::MultisampleState {
                                count: samples,
                                ..Default::default()
                            },
                            fragment: Some(wgpu::FragmentState {
                                module: &shader,
                                entry_point: Some(entry),
                                compilation_options: Default::default(),
                                targets: &[Some(wgpu::ColorTargetState {
                                    format: encoded_format,
                                    blend: Some(wgpu::BlendState {
                                        color: wgpu::BlendComponent {
                                            src_factor: if fused {
                                                wgpu::BlendFactor::One
                                            } else {
                                                wgpu::BlendFactor::SrcAlpha
                                            },
                                            dst_factor: if entry == "fog" {
                                                wgpu::BlendFactor::OneMinusSrcAlpha
                                            } else { wgpu::BlendFactor::One },
                                            operation: wgpu::BlendOperation::Add,
                                        },
                                        alpha: wgpu::BlendComponent::OVER,
                                    }),
                                    write_mask: wgpu::ColorWrites::ALL,
                                })],
                            }),
                            multiview_mask: None,
                            cache: None,
                        })
                    };
                    let scene_pipeline = |entry| {
                        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                            label: None,
                            layout: None,
                            vertex: wgpu::VertexState {
                                module: &shader,
                                entry_point: Some("vs"),
                                buffers: &[],
                                compilation_options: Default::default(),
                            },
                            primitive: Default::default(),
                            depth_stencil: None,
                            multisample: wgpu::MultisampleState {
                                count: samples,
                                ..Default::default()
                            },
                            fragment: Some(wgpu::FragmentState {
                                module: &shader,
                                entry_point: Some(entry),
                                compilation_options: Default::default(),
                                targets: &[Some(wgpu::ColorTargetState {
                                    format,
                                    blend: None,
                                    write_mask: wgpu::ColorWrites::ALL,
                                })],
                            }),
                            multiview_mask: None,
                            cache: None,
                        })
                    };
                    let background_pipeline = scene_pipeline("background");
                    let marker_pipeline = scene_pipeline("marker");
                    let separate = [
                        pipeline("fs0", false),
                        pipeline("fs1", false),
                        pipeline("fs2", false),
                    ];
                    let fused = pipeline("fused", true);
                    let fog = pipeline("fog", false);
                    let texture = |count| {
                        device.create_texture(&wgpu::TextureDescriptor {
                            label: Some("BSP blend regression same-storage SDR"),
                            size: wgpu::Extent3d {
                                width: 1,
                                height: 1,
                                depth_or_array_layers: 1,
                            },
                            mip_level_count: 1,
                            sample_count: count,
                            dimension: wgpu::TextureDimension::D2,
                            format,
                            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                                | wgpu::TextureUsages::TEXTURE_BINDING
                                | if count == 1 {
                                    wgpu::TextureUsages::COPY_SRC
                                } else {
                                    wgpu::TextureUsages::empty()
                                },
                            view_formats: if format.is_srgb() {
                                &[wgpu::TextureFormat::Rgba8Unorm]
                            } else {
                                &[]
                            },
                        })
                    };
                    let scene = texture(1);
                    let scene_view = scene.create_view(&Default::default());
                    let encoded_scene = scene.create_view(&wgpu::TextureViewDescriptor {
                        format: Some(encoded_format),
                        ..Default::default()
                    });
                    let msaa = (samples > 1).then(|| texture(samples));
                    let msaa_view = msaa.as_ref().map(|t| t.create_view(&Default::default()));
                    let encoded_msaa = msaa.as_ref().map(|t| {
                        t.create_view(&wgpu::TextureViewDescriptor {
                            format: Some(encoded_format),
                            ..Default::default()
                        })
                    });
                    let mut transfer = None;
                    TestTransfer::resize_world(
                        &mut transfer,
                        &device,
                        1,
                        1,
                        samples,
                        format,
                        !format.is_srgb(),
                    );
                    let depth = device
                        .create_texture(&wgpu::TextureDescriptor {
                            label: None,
                            size: wgpu::Extent3d {
                                width: 1,
                                height: 1,
                                depth_or_array_layers: 1,
                            },
                            mip_level_count: 1,
                            sample_count: samples,
                            dimension: wgpu::TextureDimension::D2,
                            format: wgpu::TextureFormat::Depth32Float,
                            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                            view_formats: &[],
                        })
                        .create_view(&Default::default());
                    let decode = |v: f64| {
                        if v <= 0.04045 {
                            v / 12.92
                        } else {
                            ((v + 0.055) / 1.055).powf(2.4)
                        }
                    };
                    for combine in [false, true] {
                        let mut encoder = device.create_command_encoder(&Default::default());
                        {
                            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                    view: msaa_view.as_ref().unwrap_or(&scene_view),
                                    depth_slice: None,
                                    resolve_target: msaa_view.as_ref().map(|_| &scene_view),
                                    ops: wgpu::Operations {
                                        load: wgpu::LoadOp::Clear(wgpu::Color {
                                            r: decode(0.5),
                                            g: decode(0.6),
                                            b: decode(0.7),
                                            a: 1.0,
                                        }),
                                        store: wgpu::StoreOp::Store,
                                    },
                                })],
                                ..Default::default()
                            });
                            pass.set_pipeline(&background_pipeline);
                            pass.draw(0..3, 0..1);
                        }
                        if let Some(transfer) = &transfer {
                            transfer.encode_world(
                                &device,
                                &mut encoder,
                                msaa_view.as_ref().unwrap_or(&scene_view),
                                &depth,
                            );
                        }
                        {
                            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                    view: if let Some(transfer) = &transfer {
                                        transfer.color_view()
                                    } else {
                                        encoded_msaa.as_ref().unwrap_or(&encoded_scene)
                                    },
                                    depth_slice: None,
                                    resolve_target: if let Some(transfer) = &transfer {
                                        transfer.resolve_view()
                                    } else {
                                        encoded_msaa.as_ref().map(|_| &encoded_scene)
                                    },
                                    ops: wgpu::Operations {
                                        load: wgpu::LoadOp::Load,
                                        store: wgpu::StoreOp::Store,
                                    },
                                })],
                                ..Default::default()
                            });
                            if combine {
                                pass.set_pipeline(&fused);
                                pass.draw(0..3, 0..1);
                            } else {
                                for p in &separate {
                                    pass.set_pipeline(p);
                                    pass.draw(0..3, 0..1);
                                }
                            }
                            pass.set_pipeline(&fog);
                            pass.draw(0..3, 0..1);
                        }
                        if let Some(transfer) = &transfer {
                            transfer.decode_world(
                                &device,
                                &mut encoder,
                                msaa_view.as_ref().unwrap_or(&scene_view),
                                msaa_view.as_ref().map(|_| &scene_view),
                            );
                        }
                        if samples > 1 {
                            // A later covered sample must still see its own color,
                            // as depth-equal BSP edges do after the transfer.
                            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                    view: msaa_view.as_ref().unwrap(),
                                    depth_slice: None,
                                    resolve_target: Some(&scene_view),
                                    ops: wgpu::Operations {
                                        load: wgpu::LoadOp::Load,
                                        store: wgpu::StoreOp::Store,
                                    },
                                })],
                                ..Default::default()
                            });
                            pass.set_pipeline(&marker_pipeline);
                            pass.draw(0..3, 0..1);
                        }
                        let read = device.create_buffer(&wgpu::BufferDescriptor {
                            label: None,
                            size: 256,
                            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                            mapped_at_creation: false,
                        });
                        encoder.copy_texture_to_buffer(
                            scene.as_image_copy(),
                            wgpu::TexelCopyBufferInfo {
                                buffer: &read,
                                layout: wgpu::TexelCopyBufferLayout {
                                    offset: 0,
                                    bytes_per_row: Some(256),
                                    rows_per_image: Some(1),
                                },
                            },
                            wgpu::Extent3d {
                                width: 1,
                                height: 1,
                                depth_or_array_layers: 1,
                            },
                        );
                        queue.submit([encoder.finish()]);
                        read.slice(..)
                            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
                        device
                            .poll(wgpu::PollType::Wait {
                                submission_index: None,
                                timeout: None,
                            })
                            .unwrap();
                        let data = read.slice(..).get_mapped_range().unwrap();
                        for c in 0..3 {
                            let amount = 0.8 * 0.65 / 3.0
                                + 0.8 * 0.65 * 0.6 * 2.0 / 3.0
                                + 0.8 * (1.0 - 0.65);
                            let expected_linear = (0..samples)
                                .map(|sample| {
                                    if samples > 1 && sample == 0 {
                                        0.0
                                    } else {
                                        decode(
                                            ([0.35, 0.45, 0.55][c]
                                                + 0.14 * (f64::from(sample) - 1.5)
                                                + [0.07, 0.11, 0.17][c] * amount) * 0.75
                                                + [0.2, 0.3, 0.4][c] * 0.25,
                                        )
                                    }
                                })
                                .sum::<f64>()
                                / f64::from(samples);
                            let expected = 255.0
                                * if expected_linear <= 0.0031308 {
                                    expected_linear * 12.92
                                } else {
                                    1.055 * expected_linear.powf(1.0 / 2.4) - 0.055
                                };
                            let shown = if format.is_srgb() {
                                f64::from(data[c])
                            } else {
                                let bits = u16::from_le_bytes([data[c * 2], data[c * 2 + 1]]);
                                let e = ((bits >> 10) & 31) as i32;
                                let linear = if e == 0 {
                                    f64::from(bits & 1023) * 2.0f64.powi(-24)
                                } else {
                                    (1.0 + f64::from(bits & 1023) / 1024.0) * 2.0f64.powi(e - 15)
                                };
                                255.0
                                    * if linear <= 0.0031308 {
                                        linear * 12.92
                                    } else {
                                        1.055 * linear.powf(1.0 / 2.4) - 0.055
                                    }
                            };
                            assert!((shown-expected).abs()<=2.0,
                            "{format:?} samples={samples} fused={combine} channel={c}: {shown} expected {expected}");
                        }
                    }
                }
            }
        }
    });
}
fn declaration<'a>(source: &'a str, anchor: &str) -> Result<&'a str, &'static str> {
    let start = source.find(anchor).ok_or("missing declaration")?;
    let open = start + source[start..].find('{').ok_or("missing body")?;
    let mut depth = 1;
    for (offset, byte) in source.as_bytes()[open + 1..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            return Ok(&source[start..open + offset + 2]);
        }
    }
    Err("unclosed body")
}

// The runtime transfer shader, with only resource ownership replaced by a tiny
// fixture. Integration tests avoid unrelated unfinished game unit-test targets.
struct TestTransfer {
    view: wgpu::TextureView,
    msaa: Option<wgpu::TextureView>,
    encode: wgpu::RenderPipeline,
    decode: wgpu::RenderPipeline,
}
impl TestTransfer {
    fn resize_world(
        slot: &mut Option<Self>,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        samples: u32,
        format: wgpu::TextureFormat,
        needed: bool,
    ) {
        if !needed {
            return;
        }
        let source = include_str!("../src/renderer/models/legacy_blend.rs")
            .split(if samples > 1 {
                "const TRANSFER_MSAA: &str = r#\""
            } else {
                "const TRANSFER: &str = r#\""
            })
            .nth(1)
            .unwrap()
            .split("\"#;")
            .next()
            .unwrap();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    "{}\n{source}",
                    include_str!("../src/legacy_model_blend.wgsl")
                )
                .into(),
            ),
        });
        let pipeline = |entry, target| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: None,
                layout: None,
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vertex"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let texture = |count| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: count,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba16Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        *slot = Some(Self {
            view: texture(1),
            msaa: (samples > 1).then(|| texture(samples)),
            encode: pipeline("encode", wgpu::TextureFormat::Rgba16Float),
            decode: pipeline("decode", format),
        });
    }
    fn color_view(&self) -> &wgpu::TextureView {
        self.msaa.as_ref().unwrap_or(&self.view)
    }
    fn resolve_view(&self) -> Option<&wgpu::TextureView> {
        self.msaa.as_ref().map(|_| &self.view)
    }
    fn transfer(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView,
        color: &wgpu::TextureView,
        resolve: Option<&wgpu::TextureView>,
        pipeline: &wgpu::RenderPipeline,
    ) {
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(source),
            }],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: resolve,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
    }
    fn encode_world(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView,
        _depth: &wgpu::TextureView,
    ) {
        self.transfer(
            device,
            encoder,
            scene,
            self.color_view(),
            self.resolve_view(),
            &self.encode,
        );
    }
    fn decode_world(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        resolve: Option<&wgpu::TextureView>,
    ) {
        self.transfer(
            device,
            encoder,
            self.color_view(),
            color,
            resolve,
            &self.decode,
        );
    }
}

#[test]
#[ignore = "requires a Vulkan GPU"]
fn authored_world_inline_compute_preserves_billboard_payload() {
    use wgpu::util::DeviceExt;
    futures_lite::future::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance.request_adapter(&Default::default()).await.unwrap();
        let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
        let source: [[f32; 6]; 3] = [
            [10.0, 20.0, 30.0, -2403.0, 2403.0, -1337.0],
            [1.0, 2.0, 3.0, 0.25, -0.3, 0.5],
            [4.0, 5.0, 6.0, 0.0, 0.0, 8193.5],
        ];
        let base = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&source),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let destination = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&[0.625_f32; 45]),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        });
        let axes: [[f32; 4]; 4] = [
            [0.0, 1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [7.0, 11.0, 13.0, 0.0],
        ];
        let mut pose = bytemuck::cast_slice(&axes).to_vec();
        pose.extend_from_slice(bytemuck::cast_slice(&[3_u32, 0, 15, 7]));
        let pose = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: &pose,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../src/renderer/world/inline_transform.wgsl").into(),
            ),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &shader,
            entry_point: Some("transform"),
            compilation_options: Default::default(),
            cache: None,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: base.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: destination.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: pose.as_entire_binding(),
                },
            ],
        });
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 180,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&destination, 0, &read, 0, 180);
        queue.submit([encoder.finish()]);
        read.slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();
        let data = read.slice(..).get_mapped_range().unwrap();
        let data: &[f32] = bytemuck::cast_slice(&data);
        for (i, v) in source.iter().enumerate() {
            assert_eq!(
                &data[i * 15..i * 15 + 3],
                &[v[2] + 7.0, v[1] + 13.0, -v[0] - 11.0]
            );
            assert_eq!(
                &data[i * 15 + 3..i * 15 + 7],
                &[0.625; 4],
                "UVs stay authored"
            );
            assert_eq!(
                &data[i * 15 + 10..i * 15 + 15],
                &[0.625; 5],
                "color and AO stay authored"
            );
        }
        assert_eq!(
            &data[7..10],
            &source[0][3..6],
            "full autoSprite offsets and tag survive entity rotation"
        );
        assert_eq!(
            &data[37..40],
            &[1.0, 0.0, 8193.5],
            "autoSprite2 rotates only its axis, preserving width/tag"
        );
        let len = (0.25_f32.powi(2) + 0.3_f32.powi(2) + 0.5_f32.powi(2)).sqrt();
        for (actual, expected) in data[22..25]
            .iter()
            .zip([0.5 / len, -0.3 / len, -0.25 / len])
        {
            assert!(
                (actual - expected).abs() < 1e-5,
                "ordinary normals still rotate and normalize"
            );
        }
    });
}
