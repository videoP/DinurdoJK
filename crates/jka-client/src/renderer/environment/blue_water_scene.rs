//! Single-sample scene inputs for native clouds/weather, independent of host MSAA.
use blue_water_clouds::CloudLayer;
use blue_water_gpu::scene_copy::SceneCopy;
use blue_water_render::CameraUniforms;
use blue_water_sky::Sky;
pub(super) struct Scene {
    pub copy: SceneCopy,
    pub overlay: wgpu::TextureView,
    pub depth_key: u64,
    image_layout: wgpu::BindGroupLayout,
    light_layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    replace: wgpu::RenderPipeline,
    over: wgpu::RenderPipeline,
    light: wgpu::RenderPipeline,
    merge_layout: wgpu::BindGroupLayout,
    merge_uniform: wgpu::Buffer,
    merge: wgpu::RenderPipeline,
}

/// Writes sky-portal geometry into the reversed-Z depth copy the cloud march
/// reads. The portal view is the host view shifted by a translation, so a portal
/// depth sample unprojects through the host inverse view-projection to a point
/// whose offset from the camera is the offset from the portal camera. Scaling
/// that offset by 1 / portal scale and projecting it back gives the depth the
/// same surface would have in the host view. Only nearer depth is kept.
const PORTAL_DEPTH_MERGE: &str = "
struct Merge {
    inverse_view_projection: mat4x4<f32>,
    view_projection: mat4x4<f32>,
    position_scale: vec4<f32>,
};
@group(0) @binding(0) var<uniform> merge: Merge;
@group(0) @binding(1) var portal_depth: texture_depth_2d;
struct Output { @builtin(frag_depth) depth: f32 }
@vertex fn vertex(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(p[i], 0.0, 1.0);
}
@fragment fn fragment(@builtin(position) position: vec4<f32>) -> Output {
    let size = vec2<f32>(textureDimensions(portal_depth));
    let source = textureLoad(portal_depth, vec2<i32>(position.xy), 0);
    if (source <= 0.0) {
        discard;
    }
    if (merge.position_scale.w < 0.0) {
        // Unscaled portal: infinitely far, so it hides the clouds outright.
        return Output(1.0);
    }
    let ndc = vec2<f32>(position.x / size.x * 2.0 - 1.0, 1.0 - position.y / size.y * 2.0);
    var world = merge.inverse_view_projection * vec4<f32>(ndc, source, 1.0);
    world = world / world.w;
    let eye = merge.position_scale.xyz;
    let moved = eye + (world.xyz - eye) * merge.position_scale.w;
    let clip = merge.view_projection * vec4<f32>(moved, 1.0);
    if (clip.w <= 0.0) {
        discard;
    }
    let depth = clip.z / clip.w;
    if (depth <= 0.0) {
        discard;
    }
    return Output(min(depth, 1.0));
}
";
impl Scene {
    pub fn new(
        device: &wgpu::Device,
        key: (u32, u32, u32, wgpu::TextureFormat),
        sky: &Sky,
        clouds: &CloudLayer,
    ) -> Self {
        let copy = SceneCopy::new(device, key.0, key.1, key.2);
        let overlay = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("native atmosphere overlay"),
                size: wgpu::Extent3d {
                    width: key.0.max(1),
                    height: key.1.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let tex = |binding, depth| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: if depth {
                    wgpu::TextureSampleType::Depth
                } else {
                    wgpu::TextureSampleType::Float { filterable: false }
                },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let image_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native scene blit"),
            entries: &[tex(0, false)],
        });
        let light_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native scene light"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                tex(1, true),
                tex(2, false),
            ],
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("native scene light camera"),
            size: 112,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let blit=device.create_shader_module(wgpu::ShaderModuleDescriptor {label:Some("native scene copy"),source:wgpu::ShaderSource::Wgsl("@group(0) @binding(0) var image:texture_2d<f32>; @vertex fn vertex(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32> {let p=array<vec2<f32>,3>(vec2<f32>(-1.,-1.),vec2<f32>(3.,-1.),vec2<f32>(-1.,3.));return vec4<f32>(p[i],0.,1.);} @fragment fn fragment(@builtin(position) p:vec4<f32>)->@location(0) vec4<f32>{return textureLoad(image,vec2<i32>(p.xy),0);}".into())});
        let pipeline = |shader: &wgpu::ShaderModule,
                        entry: &str,
                        layouts: &[&wgpu::BindGroupLayout],
                        blend| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("native scene"),
                bind_group_layouts: &layouts.iter().map(|v| Some(*v)).collect::<Vec<_>>(),
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vertex"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: key.3,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState {
                    count: key.2,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            })
        };
        let replace = pipeline(&blit, "fragment", &[&image_layout], None);
        let over = pipeline(
            &blit,
            "fragment",
            &[&image_layout],
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        );
        let module = blue_water_clouds::shaders::compose(
            include_str!("blue_water_scene.wgsl"),
            "JKA native world light",
            &[],
        )
        .expect("native world-light shader");
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native world light"),
            source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(module)),
        });
        let light = pipeline(
            &module,
            "light",
            &[
                &light_layout,
                &clouds.shadow().bind_group_layout,
                &sky.environment().bind_group_layout,
            ],
            None,
        );
        let merge_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native portal depth merge"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                tex(1, true),
            ],
        });
        let merge_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("native portal depth merge camera"),
            size: 144,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let merge_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native portal depth merge"),
            source: wgpu::ShaderSource::Wgsl(PORTAL_DEPTH_MERGE.into()),
        });
        let merge_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native portal depth merge"),
            bind_group_layouts: &[Some(&merge_layout)],
            immediate_size: 0,
        });
        let merge = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("native portal depth merge"),
            layout: Some(&merge_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &merge_module,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &merge_module,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[],
            }),
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                // Reversed-Z: larger is nearer, so Greater keeps the nearer surface.
                depth_compare: Some(wgpu::CompareFunction::Greater),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        static DEPTH_KEYS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let depth_key = DEPTH_KEYS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self {
            copy,
            overlay,
            depth_key,
            image_layout,
            light_layout,
            uniform,
            replace,
            over,
            light,
            merge_layout,
            merge_uniform,
            merge,
        }
    }
    /// Merges the sky-portal view's depth (`portal`, host-resolution reversed-Z)
    /// into the depth copy the cloud march and composite read, so portal terrain
    /// occludes and clips the clouds. `inverse_scale` is 1 / the portal scale.
    pub fn merge_portal_depth(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        camera: &CameraUniforms,
        portal: &wgpu::TextureView,
        inverse_scale: f32,
    ) {
        let mut values = [0f32; 36];
        values[..16].copy_from_slice(&camera.inverse_view_projection);
        values[16..32].copy_from_slice(&camera.view_projection);
        values[32..35].copy_from_slice(&camera.position);
        values[35] = inverse_scale;
        queue.write_buffer(&self.merge_uniform, 0, bytemuck::cast_slice(&values));
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native portal depth merge inputs"),
            layout: &self.merge_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.merge_uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(portal),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native portal depth merge"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.copy.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pass.set_pipeline(&self.merge);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }
    pub fn blit(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        image: &wgpu::TextureView,
        color: &wgpu::TextureView,
        resolve: Option<&wgpu::TextureView>,
        over: bool,
    ) {
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native scene image"),
            layout: &self.image_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(image),
            }],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native scene composite"),
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
        pass.set_pipeline(if over { &self.over } else { &self.replace });
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }
    pub fn light(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        camera: &CameraUniforms,
        sky: &Sky,
        clouds: &CloudLayer,
        color: &wgpu::TextureView,
        resolve: Option<&wgpu::TextureView>,
        world: bool,
        shadows: bool,
        flash: f32,
    ) {
        let mut values = [0f32; 28];
        values[..16].copy_from_slice(&camera.inverse_view_projection);
        values[16..19].copy_from_slice(&camera.position);
        values[20..24].copy_from_slice(&camera.viewport);
        values[24] = u8::from(world) as f32;
        values[25] = u8::from(shadows) as f32;
        values[26] = flash;
        queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&values));
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native lighting inputs"),
            layout: &self.light_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.copy.depth),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&self.copy.color),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native world lighting and cloud shadows"),
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
        pass.set_pipeline(&self.light);
        pass.set_bind_group(0, &group, &[]);
        pass.set_bind_group(1, &clouds.shadow().bind_group, &[]);
        pass.set_bind_group(2, &sky.environment().bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
