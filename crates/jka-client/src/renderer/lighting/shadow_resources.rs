//! Lighting shadow resources.
use crate::renderer::{
    bevy_cascade_bounds, create_shadow_receiver_bind_group, weather, GpuVertex, Mat4,
    ShadowCasterUniform, ShadowReceiverUniform, Vec3, WeatherSurfaceUniform,
    BEVY_CSM_OVERLAP_PROPORTION, BEVY_CSM_SHADOW_DEPTH_BIAS, BEVY_CSM_SHADOW_NORMAL_BIAS,
    DEPTH_FORMAT, FALLBACK_SUN_DIRECTION, SHADOW_CASCADES, SHADOW_MAP_SIZE, VERTEX_ATTRIBUTES,
};
use wgpu::util::DeviceExt;

pub(in crate::renderer) struct ShadowResources {
    pub(in crate::renderer) _texture: wgpu::Texture,
    pub(in crate::renderer) _array_view: wgpu::TextureView,
    pub(in crate::renderer) layer_views: Vec<wgpu::TextureView>,
    pub(in crate::renderer) _sky_texture: wgpu::Texture,
    pub(in crate::renderer) sky_array_view: wgpu::TextureView,
    pub(in crate::renderer) sky_layer_views: Vec<wgpu::TextureView>,
    pub(in crate::renderer) legacy_sampler: wgpu::Sampler,
    pub(in crate::renderer) bevy_sampler: wgpu::Sampler,
    // Static weather exposure uses the same R32F roof-height field as rain
    // collision. A dedicated non-filtering sampler lets shaders gather the four
    // neighboring heights in one instruction for a smooth shelter boundary.
    pub(in crate::renderer) _weather_fallback_texture: wgpu::Texture,
    pub(in crate::renderer) weather_height_view: wgpu::TextureView,
    pub(in crate::renderer) weather_sampler: wgpu::Sampler,
    pub(in crate::renderer) receiver_buffer: wgpu::Buffer,
    pub(in crate::renderer) weather_surface_buffer: wgpu::Buffer,
    pub(in crate::renderer) receiver_bind_group: wgpu::BindGroup,
    pub(in crate::renderer) caster_buffers: Vec<wgpu::Buffer>,
    pub(in crate::renderer) caster_bind_groups: Vec<wgpu::BindGroup>,
    pub(in crate::renderer) pipeline: wgpu::RenderPipeline,
    pub(in crate::renderer) mask_pipeline: wgpu::RenderPipeline,
    pub(in crate::renderer) bevy_pipeline: wgpu::RenderPipeline,
    pub(in crate::renderer) bevy_mask_pipeline: wgpu::RenderPipeline,
    pub(in crate::renderer) bevy_translucent_pipeline: wgpu::RenderPipeline,
    pub(in crate::renderer) bevy_sky_pipeline: wgpu::RenderPipeline,
}

#[derive(Clone, Copy, Default)]
pub(in crate::renderer) struct LocalShadowCacheEntry {
    pub(in crate::renderer) light_index: Option<usize>,
    pub(in crate::renderer) last_used: u64,
}

pub(in crate::renderer) struct LocalShadowResources {
    pub(in crate::renderer) _texture: wgpu::Texture,
    pub(in crate::renderer) cube_view: wgpu::TextureView,
    pub(in crate::renderer) sampler: wgpu::Sampler,
    pub(in crate::renderer) layer_views: Vec<wgpu::TextureView>,
    pub(in crate::renderer) caster_buffers: Vec<wgpu::Buffer>,
    pub(in crate::renderer) caster_bind_groups: Vec<wgpu::BindGroup>,
    pub(in crate::renderer) cache_entries: Vec<LocalShadowCacheEntry>,
    pub(in crate::renderer) active_light_indices: Vec<usize>,
    pub(in crate::renderer) pending_slots: Vec<usize>,
    pub(in crate::renderer) shadowed_light_count: u32,
    pub(in crate::renderer) selection_cluster: Option<Option<usize>>,
    pub(in crate::renderer) selection_position: Option<Vec3>,
    pub(in crate::renderer) selection_serial: u64,
}

pub(in crate::renderer) fn create_shadow_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    bevy_reverse_z: bool,
    cull_mode: Option<wgpu::Face>,
    unclipped_depth: bool,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("JKA cascaded-shadow caster pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<GpuVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &VERTEX_ATTRIBUTES,
            }],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode,
            unclipped_depth,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(if bevy_reverse_z {
                wgpu::CompareFunction::GreaterEqual
            } else {
                wgpu::CompareFunction::LessEqual
            }),
            stencil: wgpu::StencilState::default(),
            bias: if bevy_reverse_z {
                wgpu::DepthBiasState::default()
            } else {
                wgpu::DepthBiasState {
                    constant: 2,
                    slope_scale: 2.0,
                    clamp: 0.0,
                }
            },
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: None,
        multiview_mask: None,
        cache: None,
    })
}

pub(in crate::renderer) fn create_shadow_translucent_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    bevy_reverse_z: bool,
    unclipped_depth: bool,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("JKA filtered translucent sun-shadow caster pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_mask"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<GpuVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &VERTEX_ATTRIBUTES,
            }],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            unclipped_depth,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(if bevy_reverse_z {
                wgpu::CompareFunction::GreaterEqual
            } else {
                wgpu::CompareFunction::LessEqual
            }),
            stencil: wgpu::StencilState::default(),
            bias: if bevy_reverse_z {
                wgpu::DepthBiasState::default()
            } else {
                wgpu::DepthBiasState {
                    constant: 2,
                    slope_scale: 2.0,
                    clamp: 0.0,
                }
            },
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_translucent_shadow"),
            compilation_options: Default::default(),
            targets: &[],
        }),
        multiview_mask: None,
        cache: None,
    })
}

pub(in crate::renderer) fn create_sky_admission_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    bevy_reverse_z: bool,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("JKA shader-sun sky admission pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_sky"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<GpuVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &VERTEX_ATTRIBUTES,
            }],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(if bevy_reverse_z {
                wgpu::CompareFunction::GreaterEqual
            } else {
                wgpu::CompareFunction::LessEqual
            }),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: None,
        multiview_mask: None,
        cache: None,
    })
}

pub(in crate::renderer) fn create_shadow_mask_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    bevy_reverse_z: bool,
    unclipped_depth: bool,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("JKA alpha-tested shadow caster pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_mask"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<GpuVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &VERTEX_ATTRIBUTES,
            }],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            // Alpha-tested foliage/fences are commonly authored two-sided.
            // Matching their visible silhouette is more important than saving a
            // small amount of shadow-map raster work here.
            cull_mode: None,
            unclipped_depth,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(if bevy_reverse_z {
                wgpu::CompareFunction::GreaterEqual
            } else {
                wgpu::CompareFunction::LessEqual
            }),
            stencil: wgpu::StencilState::default(),
            bias: if bevy_reverse_z {
                wgpu::DepthBiasState::default()
            } else {
                wgpu::DepthBiasState {
                    constant: 2,
                    slope_scale: 2.0,
                    clamp: 0.0,
                }
            },
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_mask"),
            compilation_options: Default::default(),
            targets: &[],
        }),
        multiview_mask: None,
        cache: None,
    })
}

pub(in crate::renderer) fn create_shadow_resources(
    device: &wgpu::Device,
    receiver_layout: &wgpu::BindGroupLayout,
    caster_layout: &wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    mask_pipeline: wgpu::RenderPipeline,
    bevy_pipeline: wgpu::RenderPipeline,
    bevy_mask_pipeline: wgpu::RenderPipeline,
    bevy_translucent_pipeline: wgpu::RenderPipeline,
    bevy_sky_pipeline: wgpu::RenderPipeline,
    fog_control_buffer: &wgpu::Buffer,
) -> ShadowResources {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("JKA cascaded sun shadow array"),
        size: wgpu::Extent3d {
            width: SHADOW_MAP_SIZE,
            height: SHADOW_MAP_SIZE,
            depth_or_array_layers: SHADOW_CASCADES as u32,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let array_view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("JKA cascaded sun shadow array view"),
        format: Some(DEPTH_FORMAT),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        base_mip_level: 0,
        mip_level_count: Some(1),
        base_array_layer: 0,
        array_layer_count: Some(SHADOW_CASCADES as u32),
        aspect: wgpu::TextureAspect::DepthOnly,
        usage: Some(wgpu::TextureUsages::TEXTURE_BINDING),
    });
    let layer_views = (0..SHADOW_CASCADES)
        .map(|cascade| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("JKA cascaded sun shadow layer"),
                format: Some(DEPTH_FORMAT),
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_mip_level: 0,
                mip_level_count: Some(1),
                base_array_layer: cascade as u32,
                array_layer_count: Some(1),
                aspect: wgpu::TextureAspect::DepthOnly,
                usage: Some(wgpu::TextureUsages::RENDER_ATTACHMENT),
            })
        })
        .collect::<Vec<_>>();
    let sky_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("JKA cascaded shader-sun sky admission array"),
        size: wgpu::Extent3d {
            width: SHADOW_MAP_SIZE,
            height: SHADOW_MAP_SIZE,
            depth_or_array_layers: SHADOW_CASCADES as u32,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let sky_array_view = sky_texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("JKA cascaded shader-sun sky admission array view"),
        format: Some(DEPTH_FORMAT),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        base_mip_level: 0,
        mip_level_count: Some(1),
        base_array_layer: 0,
        array_layer_count: Some(SHADOW_CASCADES as u32),
        aspect: wgpu::TextureAspect::DepthOnly,
        usage: Some(wgpu::TextureUsages::TEXTURE_BINDING),
    });
    let sky_layer_views = (0..SHADOW_CASCADES)
        .map(|cascade| {
            sky_texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("JKA cascaded shader-sun sky admission layer"),
                format: Some(DEPTH_FORMAT),
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_mip_level: 0,
                mip_level_count: Some(1),
                base_array_layer: cascade as u32,
                array_layer_count: Some(1),
                aspect: wgpu::TextureAspect::DepthOnly,
                usage: Some(wgpu::TextureUsages::RENDER_ATTACHMENT),
            })
        })
        .collect::<Vec<_>>();

    let legacy_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("JKA cascaded sun shadow legacy comparison sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        compare: Some(wgpu::CompareFunction::LessEqual),
        ..Default::default()
    });
    let bevy_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("JKA Bevy CSM comparison sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        compare: Some(wgpu::CompareFunction::GreaterEqual),
        ..Default::default()
    });
    let weather_fallback_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("JKA weather-surface fallback heightfield"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    // Placeholder keeps the binding valid before the lazily built map field.
    // Once rain is first enabled this view is replaced by a second view of the
    // exact RGBA32F weather field used by GPU rain/materials (R=rain cover,
    // G=rendered topography height, B=physical upwardness, A=local basin score).
    let weather_height_view =
        weather_fallback_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let weather_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("JKA weather-surface gather sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    let receiver = ShadowReceiverUniform {
        view_proj: [Mat4::IDENTITY.to_cols_array_2d(); SHADOW_CASCADES],
        split_depths: bevy_cascade_bounds(),
        light_direction_enabled: [
            FALLBACK_SUN_DIRECTION[0],
            FALLBACK_SUN_DIRECTION[1],
            FALLBACK_SUN_DIRECTION[2],
            0.0,
        ],
        params: [SHADOW_MAP_SIZE as f32, 0.00035, 0.30, 0.0],
        camera_forward: [0.0, 0.0, -1.0, 0.0],
        cascade_texel_sizes: [0.0; 4],
        bevy_params: [
            BEVY_CSM_SHADOW_DEPTH_BIAS,
            BEVY_CSM_SHADOW_NORMAL_BIAS,
            BEVY_CSM_OVERLAP_PROPORTION,
            SHADOW_CASCADES as f32,
        ],
    };
    let receiver_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("JKA cascaded sun shadow receiver uniform"),
        contents: bytemuck::bytes_of(&receiver),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let weather_surface_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("JKA weather surface uniform"),
        contents: bytemuck::bytes_of(&WeatherSurfaceUniform {
            amount_distance: [
                0.0,
                weather::RAIN_WETNESS_FADE_START,
                weather::RAIN_WETNESS_FADE_END,
                1.0,
            ],
            // The film fade is rewritten with the rest of the look on first use;
            // a non-zero range keeps the shader's smoothstep well defined until then.
            film_fade: [1.0, 2.0, 0.0, 0.0],
            ..bytemuck::Zeroable::zeroed()
        }),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let receiver_bind_group = create_shadow_receiver_bind_group(
        device,
        receiver_layout,
        &array_view,
        &legacy_sampler,
        &sky_array_view,
        &receiver_buffer,
        fog_control_buffer,
        &weather_height_view,
        &weather_sampler,
        &weather_surface_buffer,
    );
    /*let receiver_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA cascaded sun shadow receiver bind group"),
        layout: receiver_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&array_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: receiver_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: fog_control_buffer.as_entire_binding(),
            },
        ],
    });*/
    let mut caster_buffers = Vec::with_capacity(SHADOW_CASCADES);
    let mut caster_bind_groups = Vec::with_capacity(SHADOW_CASCADES);
    for _ in 0..SHADOW_CASCADES {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("JKA cascaded sun shadow caster uniform"),
            contents: bytemuck::bytes_of(&ShadowCasterUniform {
                view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                camera_pos_time: [0.0; 4],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("JKA cascaded sun shadow caster bind group"),
            layout: caster_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        caster_buffers.push(buffer);
        caster_bind_groups.push(bind_group);
    }
    ShadowResources {
        _texture: texture,
        _array_view: array_view,
        layer_views,
        _sky_texture: sky_texture,
        sky_array_view,
        sky_layer_views,
        legacy_sampler,
        bevy_sampler,
        _weather_fallback_texture: weather_fallback_texture,
        weather_height_view,
        weather_sampler,
        receiver_buffer,
        weather_surface_buffer,
        receiver_bind_group,
        caster_buffers,
        caster_bind_groups,
        pipeline,
        mask_pipeline,
        bevy_pipeline,
        bevy_mask_pipeline,
        bevy_translucent_pipeline,
        bevy_sky_pipeline,
    }
}
