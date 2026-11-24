//! r_lakeUnderwaterMode 6: HL2's underwater view for Source lake water. The
//! surface seen from below is the `$bottommaterial` (source_water.wgsl); this
//! pass adds the water volume's range fog to everything drawn and, for the
//! coast material, its `$underwateroverlay` Refract warp. See the WGSL header.
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    inverse_view_projection: [[f32; 4]; 4],
    camera: [f32; 4],
    forward: [f32; 4],
    fog_color: [f32; 4],
    fog: [f32; 4],
    size: [f32; 4],
    caustics: [f32; 4],
}

/// The lake's Source fog, shared above and below the surface: color from
/// `deep_color` (linear), distance from `opaque_depth` (feet). The overlay is
/// `$underwateroverlay "effects/water_warp01"`, set only on water_coast01_beneath.
#[derive(Clone, Copy)]
pub struct Fog {
    /// Linear.
    pub color: [f32; 3],
    /// Render units.
    pub start: f32,
    pub end: f32,
    pub overlay: bool,
}

impl Fog {
    pub fn of(lake: &super::LakeSettings) -> Self {
        Self {
            color: lake.deep_color,
            start: 0.0,
            end: lake.opaque_depth * 0.3048 * super::OCEAN_UNITS_PER_METER,
            overlay: lake.source_style == 1,
        }
    }

    /// Radial distance (units) where 1 - exp2(-4.32 t) reaches 99.8%.
    pub fn cull_distance(self) -> f32 {
        const T: f32 = 8.965784 / 4.321928; // log2(1 / 0.002) / 4.32
        self.start + T * (self.end - self.start).max(1.0)
    }
}

/// `_rt_PowerOfTwoFB` is 1024x1024 with RT_SIZE_DEFAULT: kept as requested
/// unless larger than the framebuffer, then halved until it fits.
fn power_of_two_size(width: u32, height: u32) -> u32 {
    let mut size = 1024;
    while size > 1 && (size > width || size > height) {
        size /= 2;
    }
    size
}

pub struct View {
    pub view_projection: Mat4,
    pub eye: Vec3,
    pub forward: Vec3,
    pub seconds: f32,
    pub fog: Fog,
    /// The lake surface above the camera (render units).
    pub plane: f32,
}

pub struct SourceUnderwater {
    layout: wgpu::BindGroupLayout,
    fog: wgpu::RenderPipeline,
    pot: wgpu::RenderPipeline,
    overlay: wgpu::RenderPipeline,
    params: wgpu::Buffer,
    clamp_linear: wgpu::Sampler,
    repeat_linear: wgpu::Sampler,
    placeholder_2d: wgpu::TextureView,
    placeholder_array: wgpu::TextureView,
    placeholder_3d: wgpu::TextureView,
    target: Option<(u32, wgpu::TextureView)>,
}

impl SourceUnderwater {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, samples: u32) -> Self {
        let frag = wgpu::ShaderStages::FRAGMENT;
        let texture = |binding, sample_type, view_dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: frag,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension, multisampled: false },
            count: None,
        };
        let sampler = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: frag,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let float = wgpu::TextureSampleType::Float { filterable: true };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Source underwater"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: frag,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture(1, float, wgpu::TextureViewDimension::D2),
                texture(2, wgpu::TextureSampleType::Depth, wgpu::TextureViewDimension::D2),
                texture(3, float, wgpu::TextureViewDimension::D2),
                sampler(4),
                texture(5, float, wgpu::TextureViewDimension::D2Array),
                sampler(6),
                texture(7, float, wgpu::TextureViewDimension::D3),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Source underwater"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        // r_lakeUnderwaterCaustics calls the private Fluid Ninja ninja_lake_caustics
        // (jka-ninja lake_api.rs); public builds or a failing private shader use the
        // inert stand-in (caustics scale 1).
        let compose = |ninja: &str| format!("{ninja}\n{}", include_str!("source_underwater.wgsl"));
        let inert = || device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Source underwater"),
            source: wgpu::ShaderSource::Wgsl(compose(blue_water_gpu::lakes::NINJA_LAKE_WAKE_OFF).into()),
        });
        let shader = match jka_ninja::lake_wake_wgsl() {
            Some(ninja) => {
                let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
                let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("Source underwater (Fluid Ninja caustics)"),
                    source: wgpu::ShaderSource::Wgsl(compose(&ninja).into()),
                });
                match jka_river::river_ninja::checked_scope(scope) {
                    Ok(()) => module,
                    Err(e) => { eprintln!("[LAKE NINJA] underwater caustics shader failed, drawing without: {e}"); inert() }
                }
            }
            None => inert(),
        };
        let pipeline = |entry: &str, format: wgpu::TextureFormat, samples: u32| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Source underwater"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vertex"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(format.into())],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
                multiview_mask: None,
                cache: None,
            })
        };
        let placeholder = |label, layers, dimension| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: layers },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor { dimension: Some(dimension), ..Default::default() })
        };
        let linear = |mode| wgpu::SamplerDescriptor {
            address_mode_u: mode,
            address_mode_v: mode,
            // W too: the caustics volume scrolls along W with time.
            address_mode_w: mode,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        };
        Self {
            fog: pipeline("fog_fragment", format, samples),
            pot: pipeline("pot_fragment", wgpu::TextureFormat::Rgba16Float, 1),
            overlay: pipeline("overlay_fragment", format, samples),
            params: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Source underwater params"),
                size: std::mem::size_of::<Params>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            clamp_linear: device.create_sampler(&linear(wgpu::AddressMode::ClampToEdge)),
            repeat_linear: device.create_sampler(&linear(wgpu::AddressMode::Repeat)),
            placeholder_2d: placeholder("Source underwater placeholder", 1, wgpu::TextureViewDimension::D2),
            placeholder_array: placeholder("Source underwater placeholder array", 1, wgpu::TextureViewDimension::D2Array),
            placeholder_3d: device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("Source underwater no caustics"),
                    size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D3,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default()),
            target: None,
            layout,
        }
    }

    /// Fog (and, for the coast material with its normal map loaded, the
    /// overlay) over `color`, reading the post-water `scene` copy.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        resolve: Option<&wgpu::TextureView>,
        scene: &blue_water_gpu::scene_copy::SceneCopy,
        normals: Option<&wgpu::TextureView>,
        caustics: Option<&wgpu::TextureView>,
        view: View,
    ) {
        let material = view.fog;
        let (width, height) = (scene.key.0, scene.key.1);
        // The overlay samples the coast normal: the last array layer.
        let overlay = normals.filter(|_| material.overlay);
        static FIRST: std::sync::Once = std::sync::Once::new();
        FIRST.call_once(|| {
            eprintln!(
                "Source underwater: {} fog {}..{} units, overlay {}",
                if material.overlay { "coast" } else { "canal" },
                material.start,
                material.end,
                match (material.overlay, normals.is_some()) {
                    (false, _) => "none (canal beneath material has no $underwateroverlay)".to_owned(),
                    (true, true) => format!("water_warp01 via {0}x{0} _rt_PowerOfTwoFB", power_of_two_size(scene.key.0, scene.key.1)),
                    (true, false) => "skipped: Source water normals not loaded".to_owned(),
                }
            );
        });
        let pot_size = power_of_two_size(width, height);
        let params = Params {
            inverse_view_projection: view.view_projection.inverse().to_cols_array_2d(),
            camera: view.eye.extend(0.0).to_array(),
            forward: view.forward.extend(0.0).to_array(),
            fog_color: [material.color[0], material.color[1], material.color[2], 1.0],
            fog: [material.start, material.end, view.seconds, 0.0],
            size: [width as f32, height as f32, pot_size as f32, 0.0],
            caustics: [view.plane, f32::from(caustics.is_some()), 0.0, 0.0],
        };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        let group = |pot: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Source underwater"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.params.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&scene.color) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&scene.depth) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(pot) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.clamp_linear) },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(overlay.unwrap_or(&self.placeholder_array)),
                    },
                    wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::Sampler(&self.repeat_linear) },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(caustics.unwrap_or(&self.placeholder_3d)),
                    },
                ],
            })
        };
        let full_screen = |encoder: &mut wgpu::CommandEncoder, pipeline: &wgpu::RenderPipeline, group: &wgpu::BindGroup| {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Source underwater"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color,
                    depth_slice: None,
                    resolve_target: resolve,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        };
        if overlay.is_none() {
            full_screen(encoder, &self.fog, &group(&self.placeholder_2d));
            return;
        }
        if self.target.as_ref().is_none_or(|(size, _)| *size != pot_size) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Source _rt_PowerOfTwoFB"),
                size: wgpu::Extent3d { width: pot_size, height: pot_size, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            self.target = Some((pot_size, texture.create_view(&Default::default())));
        }
        let pot = &self.target.as_ref().expect("power-of-two target").1;
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Source _rt_PowerOfTwoFB copy"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: pot,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pot);
            pass.set_bind_group(0, &group(&self.placeholder_2d), &[]);
            pass.draw(0..3, 0..1);
        }
        full_screen(encoder, &self.overlay, &group(pot));
    }
}
