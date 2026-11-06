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
    csm_layout: wgpu::BindGroupLayout,
    csm_fallback: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    replace: wgpu::RenderPipeline,
    over: wgpu::RenderPipeline,
    light: wgpu::RenderPipeline,
    merge_layout: wgpu::BindGroupLayout,
    merge_uniform: wgpu::Buffer,
    merge: wgpu::RenderPipeline,
    model_sky_layout: wgpu::BindGroupLayout,
    model_sky_uniform: wgpu::Buffer,
    model_sky_buffer: wgpu::Buffer,
    model_sky: wgpu::ComputePipeline,
}

/// The sky world light for models (md3/ghoul2 `sky_world_model_gain`): a six-axis
/// ambient cube of the same irradiance the post pass reads per pixel, plus the
/// direct key, in `DynamicEntityLightingUniform::sky_world` layout. The moon
/// override matches `light` so models keep the look the post pass gave them.
const MODEL_SKY_LIGHT: &str = "#import naval_sky::environment::{SkyEnvironment, env_irradiance}
struct Params { strength:vec4<f32>, moon:vec4<f32> }
@group(0) @binding(0) var<storage, read_write> model_sky:array<vec4<f32>,8>;
@group(0) @binding(1) var<uniform> params:Params;
@group(1) @binding(0) var env_map:texture_2d<f32>;
@group(1) @binding(1) var env_sampler:sampler;
@group(1) @binding(2) var<uniform> env:SkyEnvironment;
@compute @workgroup_size(6) fn model_sky_light(@builtin(local_invocation_index) i:u32) {
 var axes=array<vec3<f32>,6>(vec3(1.,0.,0.),vec3(-1.,0.,0.),vec3(0.,1.,0.),vec3(0.,-1.,0.),vec3(0.,0.,1.),vec3(0.,0.,-1.));
 model_sky[i]=vec4<f32>(env_irradiance(env_map,env_sampler,env,axes[i])/3.14159265*0.5,0.);
 if i==0u {
  let moon=params.strength.y>0.5;
  let color=select(env.light_color*env.light_intensity,vec3<f32>(0.70,0.82,1.)*params.moon.w,moon);
  model_sky[6]=vec4<f32>(color*0.2,params.strength.x);
  model_sky[7]=vec4<f32>(select(env.light_direction,params.moon.xyz,moon),0.);
 }
}
";

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
                tex(3, false),
                tex(4, false),
            ],
        });
        let csm_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native world lighting CSM"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        // Cloud-only and standalone atmosphere draws have no JKA shadow map.
        // Keep the uniform group valid without allocating any cascades.
        let fallback_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("native world lighting CSM fallback"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: crate::renderer::depth_format(),
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let fallback_view = fallback_texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let fallback_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            compare: Some(wgpu::CompareFunction::GreaterEqual),
            ..Default::default()
        });
        let fallback_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("native world lighting CSM empty uniform"),
            size: std::mem::size_of::<crate::renderer::ShadowReceiverUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: false,
        });
        let csm_fallback = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native world lighting CSM fallback group"),
            layout: &csm_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&fallback_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&fallback_sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: fallback_uniform.as_entire_binding() },
            ],
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("native scene light camera"),
            size: 144,
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
        // The cloud overlay over the scene: premultiplied colour, the scene's own alpha kept
        // (the overlay's alpha is the clouds' opacity, not a coverage the scene should take on).
        let cloud_over = wgpu::BlendState {
            color: wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING.color,
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let over = pipeline(&blit, "fragment", &[&image_layout], Some(cloud_over));
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
                &csm_layout,
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
                format: crate::renderer::depth_format(),
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
        let model_sky_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native model sky light"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let model_sky_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("native model sky light params"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let model_sky_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("native model sky light"),
            size: 128,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let model_sky_module = blue_water_clouds::shaders::compose(MODEL_SKY_LIGHT, "JKA model sky light", &[])
            .expect("model sky light shader");
        let model_sky_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native model sky light"),
            source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(model_sky_module)),
        });
        let model_sky_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native model sky light"),
            bind_group_layouts: &[Some(&model_sky_layout), Some(&sky.environment().bind_group_layout)],
            immediate_size: 0,
        });
        let model_sky = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("native model sky light"),
            layout: Some(&model_sky_pipeline_layout),
            module: &model_sky_module,
            entry_point: Some("model_sky_light"),
            compilation_options: Default::default(),
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
            csm_layout,
            csm_fallback,
            uniform,
            replace,
            over,
            light,
            merge_layout,
            merge_uniform,
            merge,
            model_sky_layout,
            model_sky_uniform,
            model_sky_buffer,
            model_sky,
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
        crate::renderer::tile_probe_scissor_surface(&mut pass);
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
        crate::renderer::tile_probe_scissor_surface(&mut pass);
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
        world: f32,
        shadows: bool,
        flash: f32,
        moon_light: Option<([f32; 3], f32)>,
        moon_csm: Option<(&wgpu::TextureView, &wgpu::Sampler, &wgpu::Buffer)>,
        // Depth-prepass policy mask, and whether lightmapped world pixels in it
        // already carry this sky light from the composed lightmap.
        world_mask: (&wgpu::TextureView, &wgpu::TextureView, bool),
        // Model pixels (policy R 0.125) lit by their own shader: skip them too.
        models_lit: bool,
    ) {
        let mut values = [0f32; 36];
        values[..16].copy_from_slice(&camera.inverse_view_projection);
        values[16..19].copy_from_slice(&camera.position);
        values[20..24].copy_from_slice(&camera.viewport);
        values[24] = world.clamp(0.0, 1.0);
        values[25] = u8::from(shadows) as f32;
        values[26] = flash;
        values[27] = u8::from(moon_csm.is_some()) as f32;
        if let Some((direction, intensity)) = moon_light {
            values[28..31].copy_from_slice(&direction);
            values[31] = intensity;
            values[32] = 1.0; // explicit moon-light mode; even intensity 0 hides the sun
        }
        values[33] = u8::from(world_mask.2) as f32;
        values[34] = crate::ocean::OCEAN_UNITS_PER_METER;
        values[35] = u8::from(models_lit) as f32;
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
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(world_mask.0),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(world_mask.1),
                },
            ],
        });
        let csm_group = moon_csm.map(|(texture, sampler, uniform)| device.create_bind_group(
            &wgpu::BindGroupDescriptor {
                label: Some("native world lighting moon CSM"),
                layout: &self.csm_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(texture) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
                    wgpu::BindGroupEntry { binding: 2, resource: uniform.as_entire_binding() },
                ],
            }
        ));
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
        crate::renderer::tile_probe_scissor_surface(&mut pass);
        pass.set_pipeline(&self.light);
        pass.set_bind_group(0, &group, &[]);
        pass.set_bind_group(1, &clouds.shadow().bind_group, &[]);
        pass.set_bind_group(2, &sky.environment().bind_group, &[]);
        pass.set_bind_group(3, csm_group.as_ref().unwrap_or(&self.csm_fallback), &[]);
        pass.draw(0..3, 0..1);
    }

    /// Refreshes the models' sky world light (`DynamicEntityLightingUniform::sky_world`
    /// at `target.1` in `target.0`). Models draw before this pass, so they read
    /// the previous frame's values; the sky changes far slower than that.
    pub fn model_sky_light(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        sky: &Sky,
        strength: f32,
        moon_light: Option<([f32; 3], f32)>,
        target: (&wgpu::Buffer, u64),
    ) {
        let mut params = [0f32; 8];
        params[0] = strength.clamp(0.0, 1.0);
        if let Some((direction, intensity)) = moon_light {
            params[1] = 1.0;
            params[4..7].copy_from_slice(&direction);
            params[7] = intensity;
        }
        queue.write_buffer(&self.model_sky_uniform, 0, bytemuck::cast_slice(&params));
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native model sky light"),
            layout: &self.model_sky_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.model_sky_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: self.model_sky_uniform.as_entire_binding() },
            ],
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("native model sky light"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.model_sky);
            pass.set_bind_group(0, &group, &[]);
            pass.set_bind_group(1, &sky.environment().bind_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&self.model_sky_buffer, 0, target.0, target.1, 128);
    }
}
