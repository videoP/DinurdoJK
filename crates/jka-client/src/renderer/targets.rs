//! Targets.
use crate::renderer::{
    create_bloom_pyramid, create_dof_target, create_temporal_ssao_history,
    create_temporal_ssr_history, sampler_entry, texture_entry, weather, Arc, AtomicU64,
    CloudRenderResolution, Duration, Instant, DEPTH_FORMAT,
};

pub(in crate::renderer) struct AuthoredGlowTarget {
    pub(in crate::renderer) _texture: wgpu::Texture,
    pub(in crate::renderer) view: wgpu::TextureView,
    pub(in crate::renderer) _msaa_texture: Option<wgpu::Texture>,
    pub(in crate::renderer) msaa_view: Option<wgpu::TextureView>,
}

pub(in crate::renderer) struct BloomPyramid {
    pub(in crate::renderer) _textures: [wgpu::Texture; 3],
    pub(in crate::renderer) views: [wgpu::TextureView; 3],
}

pub(in crate::renderer) struct DofTarget {
    pub(in crate::renderer) _texture: wgpu::Texture,
    pub(in crate::renderer) view: wgpu::TextureView,
}

pub(in crate::renderer) struct TemporalSsaoHistory {
    pub(in crate::renderer) _textures: [wgpu::Texture; 2],
    pub(in crate::renderer) views: [wgpu::TextureView; 2],
}

pub(in crate::renderer) struct TemporalSsrHistory {
    pub(in crate::renderer) _radiance_textures: [wgpu::Texture; 2],
    pub(in crate::renderer) radiance_views: [wgpu::TextureView; 2],
    pub(in crate::renderer) _depth_textures: [wgpu::Texture; 2],
    pub(in crate::renderer) depth_views: [wgpu::TextureView; 2],
}

pub(in crate::renderer) struct MenuBackdropResources {
    pub(in crate::renderer) width: u32,
    pub(in crate::renderer) height: u32,
    pub(in crate::renderer) _texture: wgpu::Texture,
    pub(in crate::renderer) view: wgpu::TextureView,
    pub(in crate::renderer) bind_group: wgpu::BindGroup,
    pub(in crate::renderer) pipeline: wgpu::RenderPipeline,
}

impl RenderTargets {
    pub(in crate::renderer) fn memory_categories(&self) -> [(&'static str, u64); 6] {
        use crate::renderer::diagnostics::memory::texture_bytes as bytes;
        let scene = bytes(&self._scene_texture)
            + bytes(&self._depth_texture)
            + self._msaa_texture.as_ref().map_or(0, bytes);
        let post = self.authored_glow.as_ref().map_or(0, |target| {
            bytes(&target._texture) + target._msaa_texture.as_ref().map_or(0, bytes)
        }) + self
            .bloom
            .as_ref()
            .map_or(0, |target| target._textures.iter().map(bytes).sum())
            + self
                .dof
                .as_ref()
                .map_or(0, |target| bytes(&target._texture));
        let temporal = self._history_textures.iter().map(bytes).sum();
        let clouds = self._cloud_transfer_textures.iter().map(bytes).sum::<u64>()
            + bytes(&self._cloud_march_texture);
        let depth = bytes(&self._ao_depth_texture)
            + bytes(&self._linear_depth_texture)
            + bytes(&self._motion_vector_texture)
            + bytes(&self._reflection_mask_texture)
            + bytes(&self._hiz_texture)
            + self._ssr_visibility_texture.as_ref().map_or(0, bytes);
        let effects = self
            .ssao_history
            .as_ref()
            .map_or(0, |target| target._textures.iter().map(bytes).sum::<u64>())
            + self.ssr_history.as_ref().map_or(0, |target| {
                target
                    ._radiance_textures
                    .iter()
                    .chain(target._depth_textures.iter())
                    .map(bytes)
                    .sum::<u64>()
            })
            + bytes(&self._rain_haze_mask_texture);
        [
            ("scene/depth/MSAA", scene),
            ("glow/bloom/DOF", post),
            ("temporal color", temporal),
            ("legacy clouds", clouds),
            ("depth/motion/reflection/Hi-Z", depth),
            ("AO/SSR/weather", effects),
        ]
    }

    pub(in crate::renderer) fn cloud_width(&self) -> u32 {
        self._cloud_march_texture.width()
    }

    pub(in crate::renderer) fn cloud_height(&self) -> u32 {
        self._cloud_march_texture.height()
    }
}

impl crate::renderer::Renderer {
    pub(in crate::renderer) fn needs_resolved_depth_target(&self) -> bool {
        // This is also read by legacy global fog and native eye adaptation.
        // Use settings rather than map/GPU readiness so later world loads do
        // not need to allocate a target in the middle of a frame.
        self.ssr_enabled
            || self.reflection_debug_enabled
            || self.weather.fog.mode == crate::ui::FogMode::LegacyDrawFog1
            || (self.sky_backend == 1 && self.blue_water_environment.filmic)
    }
}

pub(in crate::renderer) struct RenderTargets {
    pub(in crate::renderer) _depth_texture: wgpu::Texture,
    pub(in crate::renderer) depth_view: wgpu::TextureView,
    pub(in crate::renderer) _msaa_texture: Option<wgpu::Texture>,
    pub(in crate::renderer) msaa_view: Option<wgpu::TextureView>,
    pub(in crate::renderer) _scene_texture: wgpu::Texture,
    pub(in crate::renderer) scene_view: wgpu::TextureView,
    pub(in crate::renderer) authored_glow: Option<AuthoredGlowTarget>,
    pub(in crate::renderer) bloom: Option<BloomPyramid>,
    pub(in crate::renderer) dof: Option<DofTarget>,
    pub(in crate::renderer) ssao_history: Option<TemporalSsaoHistory>,
    pub(in crate::renderer) ssr_history: Option<TemporalSsrHistory>,
    pub(in crate::renderer) _history_textures: [wgpu::Texture; 2],
    pub(in crate::renderer) history_views: [wgpu::TextureView; 2],
    pub(in crate::renderer) _cloud_transfer_textures: [wgpu::Texture; 2],
    pub(in crate::renderer) cloud_transfer_views: [wgpu::TextureView; 2],
    /// Sparse output of the cloud march pass, consumed by the resolve pass in
    /// the same frame. Not ping-ponged: nothing reads it across frames.
    pub(in crate::renderer) _cloud_march_texture: wgpu::Texture,
    pub(in crate::renderer) cloud_march_view: wgpu::TextureView,
    pub(in crate::renderer) _ao_depth_texture: wgpu::Texture,
    pub(in crate::renderer) ao_depth_view: wgpu::TextureView,
    pub(in crate::renderer) _linear_depth_texture: wgpu::Texture,
    pub(in crate::renderer) linear_depth_view: wgpu::TextureView,
    pub(in crate::renderer) linear_depth_render_view: wgpu::TextureView,
    pub(in crate::renderer) _motion_vector_texture: wgpu::Texture,
    pub(in crate::renderer) motion_vector_view: wgpu::TextureView,
    pub(in crate::renderer) _reflection_mask_texture: wgpu::Texture,
    pub(in crate::renderer) reflection_mask_view: wgpu::TextureView,
    pub(in crate::renderer) _ssr_visibility_texture: Option<wgpu::Texture>,
    pub(in crate::renderer) ssr_visibility_view: Option<wgpu::TextureView>,
    /// Power-of-two Hi-Z pyramid of `ao_depth` (reversed-Z, min-reduced). Its
    /// base level is the previous power of two of the viewport so every level
    /// maps exactly onto UV.
    pub(in crate::renderer) _hiz_texture: wgpu::Texture,
    pub(in crate::renderer) hiz_view: wgpu::TextureView,
    pub(in crate::renderer) hiz_mip_views: Vec<wgpu::TextureView>,
    pub(in crate::renderer) hiz_mip_count: u32,
    pub(in crate::renderer) hiz_width: u32,
    pub(in crate::renderer) hiz_height: u32,
    pub(in crate::renderer) _rain_haze_mask_texture: wgpu::Texture,
    pub(in crate::renderer) rain_haze_mask_view: wgpu::TextureView,
    pub(in crate::renderer) rain_haze_mask_width: u32,
    pub(in crate::renderer) rain_haze_mask_height: u32,
}

pub(in crate::renderer) const COMPANION_SCENE_RING_SIZE: usize = 3;

pub(in crate::renderer) const COMPANION_SCENE_FRAME_INTERVAL: Duration = Duration::from_millis(16);

pub(in crate::renderer) struct CompanionSceneSlot {
    pub(in crate::renderer) _color: wgpu::Texture,
    pub(in crate::renderer) color_view: wgpu::TextureView,
    pub(in crate::renderer) sequence: u64,
}

pub(in crate::renderer) struct CompanionSceneTarget {
    pub(in crate::renderer) width: u32,
    pub(in crate::renderer) height: u32,
    pub(in crate::renderer) format: wgpu::TextureFormat,
    pub(in crate::renderer) samples: u32,
    pub(in crate::renderer) generation: u64,
    pub(in crate::renderer) camera_buffer: wgpu::Buffer,
    pub(in crate::renderer) camera_bind_group: wgpu::BindGroup,
    pub(in crate::renderer) fast_camera_bind_group: wgpu::BindGroup,
    pub(in crate::renderer) _msaa: Option<wgpu::Texture>,
    pub(in crate::renderer) msaa_view: Option<wgpu::TextureView>,
    pub(in crate::renderer) _depth: wgpu::Texture,
    pub(in crate::renderer) depth_view: wgpu::TextureView,
    pub(in crate::renderer) slots: Vec<CompanionSceneSlot>,
    pub(in crate::renderer) last_render_at: Instant,
    pub(in crate::renderer) consumed_sequence: Arc<AtomicU64>,
}

pub(in crate::renderer) fn create_menu_backdrop_resources(
    device: &wgpu::Device,
    sampler: &wgpu::Sampler,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> MenuBackdropResources {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("JKA lazy menu backdrop source"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("JKA lazy menu backdrop layout"),
        entries: &[texture_entry(0), sampler_entry(1)],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("JKA lazy menu backdrop pipeline layout"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("JKA lazy menu backdrop shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../menu_backdrop.wgsl").into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("JKA lazy menu backdrop pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: crate::pipeline_cache::get().as_ref(),
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA lazy menu backdrop bind group"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    });
    MenuBackdropResources {
        width,
        height,
        _texture: texture,
        view,
        bind_group,
        pipeline,
    }
}

pub(in crate::renderer) fn create_targets(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    color_format: wgpu::TextureFormat,
    samples: u32,
    enable_bloom: bool,
    enable_authored_glow: bool,
    enable_dof: bool,
    enable_ssao: bool,
    enable_ssr: bool,
    enable_resolved_depth: bool,
    enable_temporal: bool,
    enable_clouds: bool,
    cloud_render_resolution: CloudRenderResolution,
) -> RenderTargets {
    let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("JKA depth"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: samples,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let msaa_texture = (samples > 1).then(|| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("JKA MSAA color"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format: color_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    });
    let msaa_view = msaa_texture
        .as_ref()
        .map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()));
    let scene_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("JKA post-process scene color"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: color_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let scene_view = scene_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let authored_glow = enable_authored_glow.then(|| {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("JKA authored glow source"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: color_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let msaa_texture = (samples > 1).then(|| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("JKA authored glow source MSAA"),
                size: wgpu::Extent3d {
                    width: width.max(1),
                    height: height.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format: color_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
        });
        let msaa_view = msaa_texture
            .as_ref()
            .map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()));
        AuthoredGlowTarget {
            _texture: texture,
            view,
            _msaa_texture: msaa_texture,
            msaa_view,
        }
    });
    let bloom = enable_bloom.then(|| create_bloom_pyramid(device, width, height));
    let dof = enable_dof.then(|| create_dof_target(device, width, height));
    let ssao_history = enable_ssao.then(|| create_temporal_ssao_history(device, width, height));
    let ssr_history = enable_ssr.then(|| create_temporal_ssr_history(device, width, height));
    // Inactive effects still need valid sampled bindings, but no viewport-sized
    // storage. Rebuild at full size before enabling their render passes.
    let (history_width, history_height) = if enable_temporal {
        (width.max(1), height.max(1))
    } else {
        (1, 1)
    };
    let history_textures: [wgpu::Texture; 2] = std::array::from_fn(|_| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("JKA temporal history ping-pong"),
            size: wgpu::Extent3d {
                width: history_width,
                height: history_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: color_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    });
    let history_views: [wgpu::TextureView; 2] = std::array::from_fn(|index| {
        history_textures[index].create_view(&wgpu::TextureViewDescriptor::default())
    });
    let cloud_scale = cloud_render_resolution.scale().clamp(0.01, 1.0);
    let (cloud_width, cloud_height) = if enable_clouds {
        (
            ((width.max(1) as f32 * cloud_scale).round() as u32).max(1),
            ((height.max(1) as f32 * cloud_scale).round() as u32).max(1),
        )
    } else {
        (1, 1)
    };
    let cloud_transfer_textures: [wgpu::Texture; 2] = std::array::from_fn(|_| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("JKA volumetric cloud transfer ping-pong"),
            size: wgpu::Extent3d {
                width: cloud_width,
                height: cloud_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    });
    let cloud_transfer_views: [wgpu::TextureView; 2] = std::array::from_fn(|index| {
        cloud_transfer_textures[index].create_view(&wgpu::TextureViewDescriptor::default())
    });
    let cloud_march_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("JKA volumetric cloud sparse march"),
        size: wgpu::Extent3d {
            width: cloud_width,
            height: cloud_height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let cloud_march_view = cloud_march_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let ao_depth_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("JKA AO depth attachment"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let ao_depth_view = ao_depth_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let motion_vector_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("JKA Bevy TAA motion-vector prepass"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rg16Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let motion_vector_view =
        motion_vector_texture.create_view(&wgpu::TextureViewDescriptor::default());
    // Reflection policy mask written by the existing depth prepass:
    // R = SSR eligibility, G = active planar winner, B = roughness hint, A = probe.
    // Keeping this alongside depth/motion avoids another geometry classification pass.
    let reflection_mask_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("JKA reflection policy mask"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let reflection_mask_view =
        reflection_mask_texture.create_view(&wgpu::TextureViewDescriptor::default());
    // Resolved final scene depth for SSR, legacy global fog and native metering.
    // Unlike the linear-depth prepass, it also includes main-pass-only depth
    // writers (grass, promoted ocean). Other fog modes read linear depth.
    let ssr_visibility_texture = enable_resolved_depth.then(|| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("JKA resolved final scene depth"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    });
    let ssr_visibility_view = ssr_visibility_texture
        .as_ref()
        .map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()));
    let previous_power_of_two = |value: u32| 1_u32 << (31 - value.max(1).leading_zeros());
    let hiz_width = previous_power_of_two(width);
    let hiz_height = previous_power_of_two(height);
    let hiz_mip_count = 32 - hiz_width.max(hiz_height).leading_zeros();
    let hiz_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("JKA Hi-Z depth pyramid"),
        size: wgpu::Extent3d {
            width: hiz_width,
            height: hiz_height,
            depth_or_array_layers: 1,
        },
        mip_level_count: hiz_mip_count,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
        view_formats: &[],
    });
    let hiz_view = hiz_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let hiz_mip_views = (0..hiz_mip_count)
        .map(|mip| {
            hiz_texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("JKA Hi-Z mip view"),
                base_mip_level: mip,
                mip_level_count: Some(1),
                ..Default::default()
            })
        })
        .collect();
    let linear_depth_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("JKA linear depth"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let linear_depth_view =
        linear_depth_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let linear_depth_render_view = linear_depth_texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("JKA linear depth mip 0 render view"),
        base_mip_level: 0,
        mip_level_count: Some(1),
        usage: Some(wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING),
        ..Default::default()
    });
    let (rain_haze_mask_width, rain_haze_mask_height) =
        weather::rain_haze_mask_dimensions(width.max(1), height.max(1));
    let rain_haze_mask_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("JKA rain haze exposure mask"),
        size: wgpu::Extent3d {
            width: rain_haze_mask_width,
            height: rain_haze_mask_height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
        view_formats: &[],
    });
    let rain_haze_mask_view =
        rain_haze_mask_texture.create_view(&wgpu::TextureViewDescriptor::default());
    RenderTargets {
        _depth_texture: depth_texture,
        depth_view,
        _msaa_texture: msaa_texture,
        msaa_view,
        _scene_texture: scene_texture,
        scene_view,
        authored_glow,
        bloom,
        dof,
        ssao_history,
        ssr_history,
        _history_textures: history_textures,
        history_views,
        _cloud_transfer_textures: cloud_transfer_textures,
        cloud_transfer_views,
        _cloud_march_texture: cloud_march_texture,
        cloud_march_view,
        _ao_depth_texture: ao_depth_texture,
        ao_depth_view,
        _linear_depth_texture: linear_depth_texture,
        linear_depth_view,
        linear_depth_render_view,
        _motion_vector_texture: motion_vector_texture,
        motion_vector_view,
        _reflection_mask_texture: reflection_mask_texture,
        reflection_mask_view,
        _ssr_visibility_texture: ssr_visibility_texture,
        ssr_visibility_view,
        _hiz_texture: hiz_texture,
        hiz_view,
        hiz_mip_views,
        hiz_mip_count,
        hiz_width,
        hiz_height,
        _rain_haze_mask_texture: rain_haze_mask_texture,
        rain_haze_mask_view,
        rain_haze_mask_width,
        rain_haze_mask_height,
    }
}
