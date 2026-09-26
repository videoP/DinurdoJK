//! Screen-space water optics. Capture precedes ocean and debug draws; never
//! sample the live color/depth attachments. See Development Docs/Ocean optics audit.md.
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

pub fn linear_color(rgb: [f32; 3]) -> [f32; 3] {
    rgb.map(|c| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) })
}

/// Cull only rays guaranteed to stay inside one convex water volume. A blanket
/// underwater far plane would incorrectly remove the sky/shore through the surface.
pub fn cull_submerged(eye: [f32; 3], min: [f32; 3], max: [f32; 3], volumes: &Uniform, cutoff: f32, wave_margin: f32) -> bool {
    if cutoff <= 0.0 { return false; }
    let distance_squared: f32 = (0..3).map(|i| (eye[i] - eye[i].clamp(min[i], max[i])).powi(2)).sum();
    if distance_squared < cutoff * cutoff { return false; }
    (0..volumes.params[3] as usize).any(|v| {
        (0..3).all(|i| eye[i] >= volumes.minimum[v][i] && eye[i] <= volumes.maximum[v][i]
            && min[i] >= volumes.minimum[v][i] && max[i] <= volumes.maximum[v][i])
            && eye[1].max(max[1]) < volumes.maximum[v][1] - wave_margin
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub fog_color: [f32; 3],
    pub fog_distance: f32,
    pub transparency: f32,
    pub depth_darkening: f32,
    pub refraction: f32,
    pub caustics: f32,
    pub underwater_cull: f32,
}
impl Default for Settings {
    fn default() -> Self {
        Self { fog_color: [20.0 / 255.0, 40.0 / 255.0, 50.0 / 255.0], fog_distance: 300.0, transparency: 4.0,
            depth_darkening: 1.0, refraction: 0.025, caustics: 0.0, underwater_cull: 3.0 }
    }
}
impl Settings {
    pub fn sanitize(mut self) -> Self {
        fn valid(x: f32, fallback: f32, lo: f32, hi: f32) -> f32 {
            if x.is_finite() { x.clamp(lo, hi) } else { fallback }
        }
        let d = Self::default();
        for i in 0..3 { self.fog_color[i] = valid(self.fog_color[i], d.fog_color[i], 0.0, 1.0); }
        self.fog_distance = valid(self.fog_distance, d.fog_distance, 1.0, 65536.0);
        self.transparency = valid(self.transparency, d.transparency, 0.1, 16.0);
        self.depth_darkening = valid(self.depth_darkening, d.depth_darkening, 0.01, 32.0);
        self.refraction = valid(self.refraction, d.refraction, 0.0, 0.15);
        self.caustics = valid(self.caustics, d.caustics, 0.0, 8.0);
        self.underwater_cull = valid(self.underwater_cull, d.underwater_cull, 0.0, 32.0);
        self
    }
    pub fn absorption_distance(self) -> f32 { self.fog_distance * self.transparency }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Uniform {
    pub inverse_view_projection: [[f32; 4]; 4],
    pub eye: [f32; 4],
    pub forward: [f32; 4],
    pub fog: [f32; 4], // linear color, absorption distance
    pub params: [f32; 4], // darkening distance, refraction, caustics, volume count
    pub minimum: [[f32; 4]; 8],
    pub maximum: [[f32; 4]; 8],
}

pub fn layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    make_layout(device, false, false)
}
fn make_layout(device: &wgpu::Device, capture: bool, msaa: bool) -> wgpu::BindGroupLayout {
    let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
        binding, visibility: wgpu::ShaderStages::FRAGMENT, ty, count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Ocean optics capture/sampling layout"),
        entries: &[
            entry(0, wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: !(capture && msaa) },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: capture && msaa,
            }),
            entry(1, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            entry(2, wgpu::BindingType::Texture { sample_type: if capture { wgpu::TextureSampleType::Depth } else { wgpu::TextureSampleType::Float { filterable: false } }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: capture && msaa }),
            entry(3, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }),
        ],
    })
}

pub struct Resources {
    pub key: (u32, u32, u32, wgpu::TextureFormat),
    pub bind_group: wgpu::BindGroup,
    pub uniform: wgpu::Buffer,
    capture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    capture: wgpu::RenderPipeline,
    blit: wgpu::RenderPipeline,
}
impl Resources {
    pub fn new(device: &wgpu::Device, output_layout: &wgpu::BindGroupLayout,
        ocean_layout: &wgpu::BindGroupLayout, key: (u32, u32, u32, wgpu::TextureFormat)) -> Self {
        let (width, height, samples, format) = key;
        let target = |format, label| device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label), size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
        }).create_view(&Default::default());
        let color = target(format, "Ocean pre-surface color");
        let depth = target(wgpu::TextureFormat::R32Float, "Ocean pre-surface linear depth");
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Ocean clamp sampler"), mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear, ..Default::default()
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Ocean optical volume uniform"), contents: bytemuck::bytes_of(&Uniform::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = bind(device, output_layout, &color, &depth, &sampler, &uniform);
        let capture_layout = make_layout(device, true, samples > 1);
        let capture_source = shader_source(samples > 1);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Ocean optical capture"), source: wgpu::ShaderSource::Wgsl(capture_source.into()),
        });
        let capture = pipeline(device, &shader, &[&capture_layout, ocean_layout], "fs_capture", &[format, wgpu::TextureFormat::R32Float], 1);
        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Ocean optical composite"), source: wgpu::ShaderSource::Wgsl(include_str!("../ocean_composite.wgsl").into()),
        });
        let blit = pipeline(device, &blit_shader, &[output_layout], "fs_main", &[format], samples);
        Self { key, bind_group, uniform, capture_layout, sampler, color, depth, capture, blit }
    }

    pub fn capture(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView, depth: &wgpu::TextureView, ocean: &wgpu::BindGroup) {
        let input = bind(device, &self.capture_layout, scene, depth, &self.sampler, &self.uniform);
        let attachments = [&self.color, &self.depth].map(|view| Some(wgpu::RenderPassColorAttachment {
            view, depth_slice: None, resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
        }));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Ocean optical capture"), color_attachments: &attachments, ..Default::default()
        });
        pass.set_pipeline(&self.capture);
        pass.set_bind_group(0, &input, &[]);
        pass.set_bind_group(1, ocean, &[]);
        pass.draw(0..3, 0..1);
    }
    pub fn composite(&self, encoder: &mut wgpu::CommandEncoder, color: &wgpu::TextureView, resolve: Option<&wgpu::TextureView>) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Ocean optical composite"), color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color, depth_slice: None, resolve_target: resolve,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })], ..Default::default()
        });
        pass.set_pipeline(&self.blit);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
fn bind(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, color: &wgpu::TextureView,
    depth: &wgpu::TextureView, sampler: &wgpu::Sampler, uniform: &wgpu::Buffer) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("Ocean optics bindings"), layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(color) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(depth) },
            wgpu::BindGroupEntry { binding: 3, resource: uniform.as_entire_binding() },
        ],
    })
}
fn pipeline(device: &wgpu::Device, shader: &wgpu::ShaderModule, layouts: &[&wgpu::BindGroupLayout], entry: &str,
    formats: &[wgpu::TextureFormat], samples: u32) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Ocean optics pipeline layout"), bind_group_layouts: &layouts.iter().map(|l| Some(*l)).collect::<Vec<_>>(), immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(entry), layout: Some(&layout),
        vertex: wgpu::VertexState { module: shader, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[] },
        primitive: Default::default(), depth_stencil: None,
        multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
        fragment: Some(wgpu::FragmentState { module: shader, entry_point: Some(entry), compilation_options: Default::default(),
            targets: &formats.iter().map(|&format| Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })).collect::<Vec<_>>() }),
        multiview_mask: None, cache: None,
    })
}

pub fn shader_source(msaa: bool) -> String {
        let bsp = include_str!("../bsp.wgsl");
        let ocean_struct = &bsp[bsp.find("struct OceanRenderSettings {").unwrap()..];
        let ocean_struct = &ocean_struct[..ocean_struct.find("};").unwrap() + 2];
        let source = include_str!("../ocean_optics.wgsl").replace("// OCEAN_STRUCT", ocean_struct);
        let capture_source = source
            .replace("COLOR_DECLARATION", if msaa {
                "@group(0) @binding(0) var scene_color: texture_multisampled_2d<f32>;"
            } else {
                "@group(0) @binding(0) var scene_color: texture_2d<f32>;"
            })
            .replace("DEPTH_DECLARATION", if msaa {
                "@group(0) @binding(2) var scene_depth: texture_depth_multisampled_2d;"
            } else {
                "@group(0) @binding(2) var scene_depth: texture_depth_2d;"
            })
            .replace("CAPTURE_BODY", if msaa {
                r#"let sample_count = textureNumSamples(scene_depth);
    let first_depth = textureLoad(scene_depth, pixel, 0);
    var min_depth = first_depth;
    var max_depth = first_depth;
    for (var i = 1u; i < sample_count; i += 1u) {
        let d = textureLoad(scene_depth, pixel, i32(i));
        min_depth = min(min_depth, d);
        max_depth = max(max_depth, d);
    }
    // Interior pixels take the old one-shade fast path. Only MSAA depth
    // discontinuities need sample-matched optics; this keeps the full-screen
    // water pass close to its previous cost while fixing silhouette halos.
    if (max_depth - min_depth <= 0.00001) {
        return capture_sample(uv, first_depth, textureLoad(scene_color, pixel, 0).rgb);
    }
    var out: Capture;
    out.color = vec4<f32>(0.0);
    out.depth = 1000000.0;
    for (var i = 0u; i < sample_count; i += 1u) {
        let sample = capture_sample(
            uv,
            textureLoad(scene_depth, pixel, i32(i)),
            textureLoad(scene_color, pixel, i32(i)).rgb,
        );
        out.color += sample.color;
        out.depth = min(out.depth, sample.depth);
    }
    out.color = vec4<f32>(out.color.rgb / f32(sample_count), 1.0);
    return out;"#
            } else {
                r#"return capture_sample(
        uv,
        textureLoad(scene_depth, pixel, 0),
        textureLoad(scene_color, pixel, 0).rgb,
    );"#
            });
        capture_source
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optics_shaders_validate() {
        for source in [shader_source(false), shader_source(true), include_str!("../ocean_composite.wgsl").to_owned()] {
            let module = naga::front::wgsl::parse_str(&source).unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
            naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
                .validate(&module).unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
        }
        assert_eq!(std::mem::size_of::<Uniform>(), 384);
    }

    #[test]
    fn culling_preserves_surface_and_dry_exit_rays() {
        let mut volume = Uniform::zeroed();
        volume.params[3] = 1.0;
        volume.minimum[0] = [-10000.0, -10000.0, -10000.0, 0.0];
        volume.maximum[0] = [10000.0, 0.0, 10000.0, 0.0];
        let eye = [0.0, -1000.0, 0.0];
        assert!(cull_submerged(eye, [4000.0,-1200.0,0.0], [4100.0,-800.0,100.0], &volume, 3600.0, 512.0));
        assert!(!cull_submerged(eye, [4000.0,-1200.0,0.0], [4100.0,10.0,100.0], &volume, 3600.0, 512.0));
        assert!(!cull_submerged(eye, [4000.0,-1200.0,0.0], [11000.0,-800.0,100.0], &volume, 3600.0, 512.0));
        assert!(!cull_submerged(eye, [40.0,-1200.0,0.0], [100.0,-800.0,100.0], &volume, 3600.0, 512.0));
        assert!(!cull_submerged(eye, [4000.0,-1200.0,0.0], [4100.0,-800.0,100.0], &volume, 0.0, 512.0));
    }

    #[test]
    fn invalid_settings_are_finite_and_bounded() {
        let settings = Settings { transparency: f32::NAN, fog_distance: -1.0, refraction: f32::INFINITY, ..Default::default() }.sanitize();
        assert_eq!(settings.fog_distance, 1.0);
        assert_eq!(settings.transparency, 4.0);
        assert_eq!(settings.refraction, 0.025);
    }

    #[test]
    #[ignore = "requires a GPU adapter; run explicitly"]
    fn optics_gpu_capture_matches_beer_lambert() {
        futures_lite::future::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance.request_adapter(&Default::default()).await.expect("GPU adapter");
            let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(), ..Default::default()
            }).await.expect("GPU device");
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let ocean_layout = crate::ocean::OceanGpu::create_render_layout(&device);
            let ocean = crate::ocean::OceanGpu::create_inert_render_bind_group(&device, &ocean_layout);
            let layout = layout(&device);
            let extent = wgpu::Extent3d { width: 8, height: 8, depth_or_array_layers: 1 };
            for samples in [1, 4] {
                let resources = Resources::new(&device, &layout, &ocean_layout, (8, 8, samples, wgpu::TextureFormat::Rgba8Unorm));
                let texture = |format, samples, usage| device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("Optics GPU test target"), size: extent, mip_level_count: 1, sample_count: samples,
                    dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[],
                });
                let color = texture(wgpu::TextureFormat::Rgba8Unorm, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC);
                let color_view = color.create_view(&Default::default());
                let msaa = texture(
                    wgpu::TextureFormat::Rgba8Unorm,
                    samples,
                    wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                );
                let msaa_view = msaa.create_view(&Default::default());
                let depth = texture(wgpu::TextureFormat::Depth32Float, samples, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);
                let depth_view = depth.create_view(&Default::default());
                let output = if samples == 1 { &color_view } else { &msaa_view };
                let resolve = (samples > 1).then_some(&color_view);
                for (volumes, surface_z, fraction) in [(0, 10.0, 0.0), (1, 10.0, 1.0), (2, 10.0, 1.0), (1, 0.25, 0.5)] {
                    let mut uniform = Uniform::zeroed();
                    uniform.inverse_view_projection = glam::Mat4::IDENTITY.to_cols_array_2d();
                    uniform.forward = [0.0,0.0,1.0,0.0];
                    uniform.fog[3] = 20.0;
                    uniform.params = [20.0,0.0,0.0,volumes as f32];
                    for i in 0..volumes {
                        uniform.minimum[i] = [-10.0,-10.0,-10.0,0.0];
                        uniform.maximum[i] = [10.0,10.0,surface_z,0.0];
                    }
                    queue.write_buffer(&resources.uniform, 0, bytemuck::bytes_of(&uniform));
                    let mut encoder = device.create_command_encoder(&Default::default());
                    {
                        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: output, depth_slice: None, resolve_target: resolve,
                                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::WHITE), store: wgpu::StoreOp::Store } })],
                            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &depth_view,
                                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.5), store: wgpu::StoreOp::Store }), stencil_ops: None }), ..Default::default()
                        });
                    }
                    let capture_color = if samples == 1 { &color_view } else { &msaa_view };
                    resources.capture(&device, &mut encoder, capture_color, &depth_view, &ocean);
                    resources.composite(&mut encoder, output, resolve);
                    let readback = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 256*8,
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
                    encoder.copy_texture_to_buffer(color.as_image_copy(), wgpu::TexelCopyBufferInfo { buffer: &readback,
                        layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(256), rows_per_image: Some(8) } }, extent);
                    queue.submit([encoder.finish()]);
                    readback.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
                    device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
                    let bytes = readback.slice(..).get_mapped_range();
                    let distance = (0.125_f32.powi(2)*2.0 + 0.5_f32.powi(2)).sqrt() * fraction;
                    for (channel, ratio) in [38.0/7.5, 38.0/22.0, 1.0].iter().enumerate() {
                        let expected = (-4.321928 * ratio * distance / 20.0).exp2();
                        let actual = bytes[4*256+4*4+channel] as f32 / 255.0;
                        assert!((expected-actual).abs() < 0.008, "samples={samples} volumes={volumes} channel={channel}: {actual} != {expected}");
                    }
                }
            }
            assert!(scope.pop().await.is_none(), "GPU validation error");
        });
    }
}
