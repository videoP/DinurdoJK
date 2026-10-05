//! Initialize.
use crate::renderer::{
    comparison_sampler_entry, compose_world_shader, create_auto_exposure_bind_group,
    create_bloom_bind_groups, create_bloom_pipeline, create_cloud_fallback_resources,
    create_cloud_pass_pipeline, create_cloud_resolve_bind_group, create_color_lut_texture,
    create_depth_prepass_mask_pipeline, create_depth_prepass_pipeline, create_dof_bind_group,
    create_dof_pipeline, create_fast_post_bind_group, create_gamma_post_bind_group,
    create_gamma_post_pipeline, create_hiz_build_bind_group, create_hiz_prepass_pipeline,
    create_hiz_reduce_bind_groups, create_planar_debug_pipeline,
    create_planar_reflection_resources, create_post_bind_group, create_post_pipeline,
    create_shader_module_timed, create_shadow_mask_pipeline, create_shadow_pipeline,
    create_shadow_resources, create_shadow_translucent_pipeline, create_sky_admission_pipeline,
    create_ssao_temporal_bind_groups, create_ssao_temporal_pipeline,
    create_ssr_temporal_bind_groups, create_ssr_temporal_pipeline, create_ssr_visibility_resources,
    create_targets, create_texture_sampler, create_ui_pipeline, cube_texture_entry,
    depth_array_texture_entry, depth_cube_array_texture_entry, depth_texture_entry,
    empty_ui_font_texture, encode_screenshot_job, env_flag, load_texture_asset_or_missing,
    load_ui_font_texture, load_ui_loading_texture, load_ui_proportional_font,
    load_ui_unknown_map_texture, mpsc, no_vsync_mode, nonfiltering_sampler_entry, pipeline_hash,
    sampler_entry, sampler_entry_stages, storage_buffer_entry, supported_msaa,
    texture_2d_array_entry, texture_3d_entry, texture_3d_entry_stages, texture_entry,
    texture_entry_stages, thread, timed_init_step, unfilterable_texture_entry,
    unfilterable_texture_entry_stages, uniform_entry, upload_texture, weather, Arc,
    AutoExposureState, AutoExposureUniform, BTreeMap, CameraUniform, CloudLayerSettings,
    CloudRenderResolution, CloudRenderSettings, CloudSunSettings, CloudType, ColorLutPreset,
    CullDebugMode, CullDiagnosticsReadback, DebugVolumeRenderer, DetailTextureMode, DofQuality,
    DofUniform, DynamicLightsMode, DynamicModelRenderer, DynamicShadowsMode, EntityShadowLight,
    EntityShadowState, FramePlan, GammaPostUniform, GpuCullSettings, GpuProfiler, GrassRenderer,
    HashMap, InlineDrawScratch, Instant, JumpShadeState, LightingSettings, Mat4, Path, PbrSettings,
    PipelineJobKey, PipelineJobManager, PlanarReflectionDebugMode, PlanarReflectionMode,
    PostAaSettings, PostCameraFxSettings, PostCloudShadowSettings, PostCloudShapingSettings,
    PostCloudSkyAmbientSettings, PostCloudTemporalSettings, PostCloudTemporalTuningSettings,
    PostCloudVariationSettings, PostCloudWindSettings, PostColorSettings, PostFilmSettings,
    PostGrainSettings, PostSceneSettings, PostUniform, PvsMode, RainIntensity, ReflectionQuality,
    Renderer, ScreenshotEncodeJob, ScreenshotOutput, SsaoTemporalUniform, SsrTemporalUniform,
    SunVisibilityMode, SurfaceDeformationGpu, SurfaceSpriteEffectRenderer, TextureData,
    TextureFilter, UiSnapshot, Vec3, VsyncMode, WeatherSystem, Window, FALLBACK_SUN_COLOR,
    FALLBACK_SUN_DIRECTION, FALLBACK_SUN_INTENSITY, LOCAL_SHADOW_MAP_SIZE,
    MAX_CLOUD_FOREGROUND_BLADES, NO_WATER_SURFACE, PBR_PROFILE_MATERIALS,
    PBR_PROFILE_PARALLAX_OCCLUSION, PLANAR_REFLECTION_SLOTS, SCREEN_FX_BUFFER_BYTES,
    SHADOW_CASTER_CULL, UI_BUFFER_BYTES, UI_DYNAMIC_BUFFER_BYTES, UI_TRANSIENT_BUFFER_BYTES,
};
use bytemuck::Zeroable;
use wgpu::util::DeviceExt;

impl Renderer {
    pub(in crate::renderer) async fn new(
        window: Arc<Window>,
        instance: wgpu::Instance,
        surface: wgpu::Surface<'static>,
        base: &Path,
        game: Option<&Path>,
        desired_maximum_frame_latency: u32,
        _detail_texture_path: &str,
    ) -> Result<Self, String> {
        let size = window.inner_size();
        // Per-phase wall times for the "[RENDER INIT] phases" line, so startup
        // regressions show which section grew instead of one opaque total.
        let mut init_phase = Instant::now();
        let mut init_marks: Vec<(&'static str, f64)> = Vec::new();
        macro_rules! init_mark {
            ($name:expr) => {{
                init_marks.push(($name, init_phase.elapsed().as_secs_f64() * 1000.0));
                init_phase = Instant::now();
            }};
        }
        let adapter_started = Instant::now();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .map_err(|e| format!("No compatible GPU adapter: {e}"))?;
        rverbose!(
            1,
            "[RENDER INIT] adapter request {:.1} ms",
            adapter_started.elapsed().as_secs_f64() * 1000.0
        );
        let info = adapter.get_info();
        rverbose!(
            1,
            "GPU: {} ({:?}, {:?})",
            info.name,
            info.backend,
            info.device_type
        );
        rverbose!(
            1,
            "GPU driver: {} {} (vendor 0x{:04x}, device 0x{:04x})",
            info.driver,
            info.driver_info,
            info.vendor,
            info.device
        );
        let adapter_features = adapter.features();
        // Request timestamp-query capability up front when the adapter supports it so
        // the profiler can be switched on from the menu without recreating the wgpu
        // device. Query writes, resolves and readbacks stay disabled on the normal
        // maximum-FPS path until GPU TIMESTAMPS is explicitly enabled.
        let indirect_supported = adapter
            .get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::INDIRECT_EXECUTION);
        rverbose!(
            1,
            "Indirect execution: {}",
            if indirect_supported {
                "supported"
            } else {
                "unavailable"
            }
        );
        let adapter_specific_formats = wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES;
        let wireframe_feature = wgpu::Features::POLYGON_MODE_LINE;
        let multi_draw_count_feature = wgpu::Features::MULTI_DRAW_INDIRECT_COUNT;
        let timestamp_feature = wgpu::Features::TIMESTAMP_QUERY;
        let timestamp_encoder_feature = wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
        let timestamp_pass_feature = wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES;
        let bc_compression_feature = wgpu::Features::TEXTURE_COMPRESSION_BC;
        let ray_query_feature = wgpu::Features::EXPERIMENTAL_RAY_QUERY;
        let depth_clip_control_feature = wgpu::Features::DEPTH_CLIP_CONTROL;
        let ray_tracing_supported = adapter_features.contains(ray_query_feature);
        let bc_compression_supported = adapter_features.contains(bc_compression_feature);
        let wireframe_supported = adapter_features.contains(wireframe_feature);
        let gpu_compaction_supported =
            indirect_supported && adapter_features.contains(multi_draw_count_feature);
        // Normal gameplay still pays no per-frame timestamp-query cost. When the
        // runtime profiler is enabled, use in-pass timestamps when supported so
        // grass can be measured independently inside the World interval.
        let timestamp_supported = adapter_features.contains(timestamp_feature)
            && adapter_features.contains(timestamp_encoder_feature);
        let timestamp_inside_encoders = timestamp_supported;
        let timestamp_inside_passes =
            timestamp_supported && adapter_features.contains(timestamp_pass_feature);
        let mut required_features = wgpu::Features::empty();
        // Directional shadow casters must not be clipped by the cascade's depth
        // range (Bevy's UNCLIPPED_DEPTH_ORTHO): occluders between the sun and the
        // cascade would otherwise vanish from the map.
        let depth_clip_control_supported = adapter_features.contains(depth_clip_control_feature);
        if depth_clip_control_supported {
            required_features |= depth_clip_control_feature;
        }
        if adapter_features.contains(adapter_specific_formats) {
            required_features |= adapter_specific_formats;
        }
        if wireframe_supported {
            required_features |= wireframe_feature;
        }
        if gpu_compaction_supported {
            required_features |= multi_draw_count_feature;
        }
        if bc_compression_supported {
            required_features |= bc_compression_feature;
        }
        if timestamp_supported {
            required_features |= timestamp_feature | timestamp_encoder_feature;
            if timestamp_inside_passes {
                required_features |= timestamp_pass_feature;
            }
        }
        if ray_tracing_supported {
            required_features |= ray_query_feature;
        }
        rverbose!(
            1,
            "Wireframe polygon mode: {}",
            if wireframe_supported {
                "supported"
            } else {
                "unavailable"
            }
        );
        rverbose!(
            1,
            "GPU indirect compaction: {}",
            if gpu_compaction_supported {
                "supported"
            } else {
                "unavailable (falling back to per-batch indirect draws)"
            }
        );
        rverbose!(
            1,
            "PBR companion BC compression: {}",
            if bc_compression_supported {
                "supported"
            } else {
                "unavailable"
            }
        );
        rverbose!(
            1,
            "Hardware ray queries: {}",
            if ray_tracing_supported {
                "supported (RT Shadows available)"
            } else {
                "unavailable on this adapter/backend"
            }
        );
        rverbose!(
            1,
            "GPU timestamps: {}{}",
            if timestamp_supported {
                "supported"
            } else {
                "unavailable (encoder timestamps required)"
            },
            if timestamp_inside_passes {
                " (encoder + in-pass grass timing)"
            } else if timestamp_supported {
                " (encoder-only; grass GPU sub-timing unavailable)"
            } else {
                ""
            }
        );
        let device_started = Instant::now();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("DinurdoJK renderer"),
                required_features,
                required_limits: adapter.limits(),
                experimental_features: if ray_tracing_supported {
                    // wgpu 29 gates the native ray-query API behind its explicit
                    // experimental-feature token as well as EXPERIMENTAL_RAY_QUERY.
                    unsafe { wgpu::ExperimentalFeatures::enabled() }
                } else {
                    wgpu::ExperimentalFeatures::disabled()
                },
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|e| format!("GPU device creation failed: {e}"))?;
        rverbose!(
            1,
            "[RENDER INIT] device request {:.1} ms",
            device_started.elapsed().as_secs_f64() * 1000.0
        );
        init_mark!("adapter+device");

        // A lost device (TDR, driver reset, removed adapter) otherwise shows up
        // only as downstream validation noise or a dead screen. `Destroyed` is
        // the normal teardown notification and is logged as such.
        device.set_device_lost_callback(|reason, message| match reason {
            wgpu::DeviceLostReason::Destroyed => {
                rverbose!(1, "[GPU] device destroyed: {message}");
            }
            reason => eprintln!("[GPU DEVICE LOST] reason={reason:?}: {message}"),
        });

        // Diagnostics should print the actual wgpu validation reason instead of
        // collapsing it into the generic CurrentSurfaceTexture::Validation path.
        // The normal maximum-FPS path installs no callback.
        if env_flag("JKA_SURFACE_DIAG") {
            device.on_uncaptured_error(Arc::new(|error| {
                eprintln!("[JKA WGPU ERROR] {error:?}");
            }));
        }

        let caps = surface.get_capabilities(&adapter);
        let surface_format = caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(caps.formats[0]);
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("Surface has no compatible default configuration")?;
        config.format = surface_format;
        config.present_mode = no_vsync_mode(&caps.present_modes);
        config.desired_maximum_frame_latency = desired_maximum_frame_latency.clamp(1, 3);
        let screenshot_supported = caps.usages.contains(wgpu::TextureUsages::COPY_SRC);
        if screenshot_supported {
            config.usage |= wgpu::TextureUsages::COPY_SRC;
        }
        // Surface::configure reports validation through wgpu's device error
        // machinery. Capture the initial configure error instead of letting the
        // default uncaptured-error handler panic the render thread. That is
        // especially important during backend restarts: the app can immediately
        // roll back to the last known-good renderer instead of waiting on the
        // video confirmation timeout with a dead screen.
        let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        surface.configure(&device, &config);
        if let Some(error) = error_scope.pop().await {
            return Err(format!("Surface configuration failed: {error}"));
        }
        rverbose!(
            1,
            "Present mode: {:?}; surface: {:?}",
            config.present_mode,
            config.format
        );
        rverbose!(
            1,
            "Screenshot readback: {}",
            if screenshot_supported {
                "supported"
            } else {
                "unavailable for this surface"
            }
        );

        let surface_supported_msaa = supported_msaa(&adapter, surface_format);
        let hdr_supported_msaa = supported_msaa(&adapter, wgpu::TextureFormat::Rgba16Float);
        rverbose!(1, "MSAA supported: {surface_supported_msaa:?}");
        rverbose!(1, "HDR Rgba16Float MSAA supported: {hdr_supported_msaa:?}");
        let post_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA post-process layout"),
            entries: &[
                texture_entry(0),
                sampler_entry(1),
                uniform_entry(2, wgpu::ShaderStages::FRAGMENT),
                unfilterable_texture_entry(3),
                texture_entry(4),
                storage_buffer_entry(5, true, wgpu::ShaderStages::FRAGMENT),
                texture_entry(6),
                texture_entry(7),
                texture_entry(8),
                texture_3d_entry(9),
                sampler_entry(10),
                texture_entry(11),
                texture_entry(12),
                unfilterable_texture_entry(13),
                texture_entry(14),
                texture_3d_entry(15),
                sampler_entry(16),
                texture_entry(17),
                unfilterable_texture_entry(18),
                texture_entry(19),
                texture_entry(20),
                texture_entry(21),
                depth_texture_entry(22),
                texture_entry(23),
                storage_buffer_entry(24, true, wgpu::ShaderStages::FRAGMENT),
                unfilterable_texture_entry(25),
            ],
        });
        let post_shader = create_shader_module_timed(
            &device,
            wgpu::ShaderModuleDescriptor {
                label: Some("JKA post-process shader"),
                source: wgpu::ShaderSource::Wgsl(
                    weather::with_weather_surface(&weather::with_post_settings(include_str!(
                        "../post.wgsl"
                    )))
                    .into(),
                ),
            },
        );
        let post_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("JKA post-process pipeline layout"),
            bind_group_layouts: &[Some(&post_layout)],
            immediate_size: 0,
        });
        let mut pipeline_jobs = PipelineJobManager::new();
        // The full post pipeline is the slowest optional startup compile. Queue it
        // through the shared non-blocking pipeline worker system immediately; a
        // frame that needs it before completion uses the cheap gamma path rather
        // than ever waiting on the render thread.
        {
            let key = PipelineJobKey::new("post", 0, pipeline_hash(&(config.format,)));
            let (device, layout, shader, format) = (
                device.clone(),
                post_pipeline_layout.clone(),
                post_shader.clone(),
                config.format,
            );
            pipeline_jobs.request(key, "full post", move || {
                timed_init_step("post pipeline (background)", || {
                    create_post_pipeline(&device, &layout, &shader, format)
                })
            });
        }
        let msaa_samples = 1;
        let targets = create_targets(
            &device,
            config.width,
            config.height,
            config.format,
            msaa_samples,
            false,
            false,
            false,
            false,
            CloudRenderResolution::Half,
        );

        init_mark!("surface config+caps");
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("JKA camera uniform"),
            contents: bytemuck::bytes_of(&CameraUniform {
                view_proj: [[0.0; 4]; 4],
                camera_pos_time: [0.0; 4],
                clip_plane: [0.0; 4],
                render_flags: [0; 4],
                camera_forward: [0.0, 0.0, -1.0, 0.0],
                unjittered_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                previous_unjittered_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                jump_shade: [0.0; 4],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let planar_camera_buffers: Vec<wgpu::Buffer> = (0..PLANAR_REFLECTION_SLOTS)
            .map(|slot| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(&format!("JKA planar reflection camera uniform {slot}")),
                    contents: bytemuck::bytes_of(&CameraUniform {
                        view_proj: [[0.0; 4]; 4],
                        camera_pos_time: [0.0; 4],
                        clip_plane: [0.0; 4],
                        render_flags: [1, 0, 0, 0],
                        camera_forward: [0.0, 0.0, -1.0, 0.0],
                        unjittered_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                        previous_unjittered_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                        jump_shade: [0.0; 4],
                    }),
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                })
            })
            .collect();
        let surface_deformation = SurfaceDeformationGpu::new(&device);
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                // The BSP fragment shader uses camera position for clustered
                // lighting and sky direction, so this binding must be visible
                // to all stages that share the camera bind group.
                visibility: wgpu::ShaderStages::VERTEX
                    | wgpu::ShaderStages::FRAGMENT
                    | wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let surface_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("BSP surface layout"),
            entries: &[
                texture_entry(0),
                sampler_entry(1),
                texture_entry(2),
                sampler_entry(3),
                uniform_entry(4, wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT),
                texture_entry(5),
                texture_entry(6),
                texture_entry(7),
                texture_entry(8),
                texture_entry(9),
                texture_entry(10),
                sampler_entry(11),
                texture_entry(12),
                texture_entry(13),
                texture_entry(14),
                uniform_entry(15, wgpu::ShaderStages::FRAGMENT),
                texture_entry(16),
                texture_entry(17),
                texture_entry(18),
                cube_texture_entry(19),
                sampler_entry(20),
                uniform_entry(
                    21,
                    wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ),
                texture_entry(22),
                texture_entry(23),
                sampler_entry(24),
                texture_entry_stages(
                    25,
                    wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ),
                texture_entry_stages(
                    26,
                    wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ),
                sampler_entry_stages(
                    27,
                    wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ),
                sampler_entry(28),
                texture_entry(29),
                texture_entry(30),
                sampler_entry(31),
            ],
        });
        // Exact descriptor layouts used by the known-fast renderer. Advanced
        // bindings must not leak into the maximum-FPS path merely because they
        // exist elsewhere in the renderer. These are selected once when render
        // settings change through FramePlan::world_path.
        let fast_camera_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("JKA fast camera layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    // The fast fragment stage reads camera time for wave rgbGen/
                    // alphaGen and sky-cloud tcMods, so FRAGMENT must be visible.
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        let fast_surface_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("JKA fast BSP surface layout"),
                entries: &[
                    texture_entry(0),
                    sampler_entry(1),
                    texture_entry(2),
                    sampler_entry(3),
                    uniform_entry(4, wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT),
                    texture_entry(5),
                    texture_entry(6),
                    texture_entry(7),
                    texture_entry(8),
                    texture_entry(9),
                    texture_entry(10),
                    sampler_entry(11),
                    uniform_entry(
                        21,
                        wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ),
                    texture_entry(22),
                    texture_entry(23),
                    sampler_entry(24),
                    texture_entry_stages(
                        25,
                        wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ),
                    texture_entry_stages(
                        26,
                        wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ),
                    sampler_entry_stages(
                        27,
                        wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ),
                ],
            });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera bind group"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let entity_shadow = EntityShadowState::new(&device, &camera_layout);
        let fast_camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("JKA fast camera bind group"),
            layout: &fast_camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let planar_camera_bind_groups: Vec<wgpu::BindGroup> = planar_camera_buffers
            .iter()
            .enumerate()
            .map(|(slot, buffer)| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(&format!("JKA planar reflection camera bind group {slot}")),
                    layout: &camera_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    }],
                })
            })
            .collect();
        let sky_portal_camera_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("JKA sky portal camera uniform"),
                contents: bytemuck::bytes_of(&CameraUniform {
                    view_proj: [[0.0; 4]; 4],
                    camera_pos_time: [0.0; 4],
                    clip_plane: [0.0; 4],
                    render_flags: [0; 4],
                    camera_forward: [0.0, 0.0, -1.0, 0.0],
                    unjittered_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                    previous_unjittered_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                    jump_shade: [0.0; 4],
                }),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let sky_portal_camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("JKA sky portal camera bind group"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: sky_portal_camera_buffer.as_entire_binding(),
            }],
        });
        let planar_reflection_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("JKA planar reflection layout"),
                entries: &[
                    texture_2d_array_entry(0),
                    sampler_entry(1),
                    uniform_entry(2, wgpu::ShaderStages::FRAGMENT),
                ],
            });
        let planar_reflection = create_planar_reflection_resources(
            &device,
            &planar_reflection_layout,
            config.format,
            1,
            1,
            false,
        );
        init_mark!("camera/bind groups/planar reflection");
        let planar_debug_shader = create_shader_module_timed(
            &device,
            wgpu::ShaderModuleDescriptor {
                label: Some("JKA planar reflection debug preview shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../planar_debug.wgsl").into()),
            },
        );
        let planar_debug_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("JKA planar reflection debug preview pipeline layout"),
                bind_group_layouts: &[Some(&planar_reflection_layout)],
                immediate_size: 0,
            });
        let planar_debug_pipeline = create_planar_debug_pipeline(
            &device,
            &planar_debug_pipeline_layout,
            &planar_debug_shader,
            config.format,
        );
        let cluster_compute_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("JKA clustered-light compute layout"),
                entries: &[
                    storage_buffer_entry(0, true, wgpu::ShaderStages::COMPUTE),
                    storage_buffer_entry(1, false, wgpu::ShaderStages::COMPUTE),
                    uniform_entry(2, wgpu::ShaderStages::COMPUTE),
                ],
            });
        // Preserve the pre-area/GI descriptor interface as well as its shader.
        // This makes the all-OFF path use the same group-2 bindings as the old
        // renderer rather than merely branching out of the new resources.
        let lighting_layout_lean =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("JKA clustered-light render layout (lean baseline)"),
                entries: &[
                    storage_buffer_entry(
                        0,
                        true,
                        wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ),
                    storage_buffer_entry(1, true, wgpu::ShaderStages::FRAGMENT),
                    uniform_entry(2, wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT),
                    uniform_entry(3, wgpu::ShaderStages::FRAGMENT),
                    depth_cube_array_texture_entry(4),
                    comparison_sampler_entry(5),
                    texture_3d_entry_stages(
                        6,
                        wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ),
                    texture_3d_entry_stages(
                        7,
                        wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ),
                    sampler_entry_stages(
                        8,
                        wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ),
                    uniform_entry(9, wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT),
                    storage_buffer_entry(
                        16,
                        true,
                        wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ),
                ],
            });
        let lighting_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA clustered-light render layout"),
            entries: &[
                storage_buffer_entry(
                    0,
                    true,
                    wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ),
                storage_buffer_entry(1, true, wgpu::ShaderStages::FRAGMENT),
                uniform_entry(2, wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT),
                uniform_entry(3, wgpu::ShaderStages::FRAGMENT),
                depth_cube_array_texture_entry(4),
                comparison_sampler_entry(5),
                texture_3d_entry_stages(
                    6,
                    wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ),
                texture_3d_entry_stages(
                    7,
                    wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ),
                sampler_entry_stages(8, wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT),
                uniform_entry(9, wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT),
                texture_3d_entry(10),
                texture_3d_entry(11),
                uniform_entry(12, wgpu::ShaderStages::FRAGMENT),
                texture_3d_entry(13),
                uniform_entry(14, wgpu::ShaderStages::FRAGMENT),
                storage_buffer_entry(
                    16,
                    true,
                    wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ),
            ],
        });
        let shadow_receiver_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("JKA cascaded-shadow receiver layout"),
                entries: &[
                    depth_array_texture_entry(0),
                    comparison_sampler_entry(1),
                    uniform_entry(2, wgpu::ShaderStages::FRAGMENT),
                    uniform_entry(3, wgpu::ShaderStages::FRAGMENT),
                    unfilterable_texture_entry_stages(
                        4,
                        wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
                    ),
                    uniform_entry(
                        5,
                        wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
                    ),
                    nonfiltering_sampler_entry(6, wgpu::ShaderStages::FRAGMENT),
                    depth_array_texture_entry(7),
                ],
            });
        // Group 5 holds an immutable pre-water scene snapshot.
        let cloud_shadow_layout = crate::ocean::optics::layout(&device);
        let shadow_caster_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("JKA cascaded-shadow caster layout"),
                entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX)],
            });
        let shadow_shader = create_shader_module_timed(
            &device,
            wgpu::ShaderModuleDescriptor {
                label: Some("JKA cascaded-shadow caster shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../shadow.wgsl").into()),
            },
        );
        let shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("JKA cascaded-shadow caster pipeline layout"),
                bind_group_layouts: &[Some(&shadow_caster_layout)],
                immediate_size: 0,
            });
        let shadow_mask_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("JKA alpha-tested shadow caster pipeline layout"),
                bind_group_layouts: &[Some(&shadow_caster_layout), Some(&surface_layout)],
                immediate_size: 0,
            });
        let shadow_pipeline = create_shadow_pipeline(
            &device,
            &shadow_pipeline_layout,
            &shadow_shader,
            false,
            SHADOW_CASTER_CULL,
            depth_clip_control_supported,
        );
        let shadow_mask_pipeline = create_shadow_mask_pipeline(
            &device,
            &shadow_mask_pipeline_layout,
            &shadow_shader,
            false,
            depth_clip_control_supported,
        );
        let bevy_shadow_pipeline = create_shadow_pipeline(
            &device,
            &shadow_pipeline_layout,
            &shadow_shader,
            true,
            SHADOW_CASTER_CULL,
            depth_clip_control_supported,
        );
        let bevy_shadow_sky_pipeline =
            create_sky_admission_pipeline(&device, &shadow_pipeline_layout, &shadow_shader, true);
        let bevy_shadow_mask_pipeline = create_shadow_mask_pipeline(
            &device,
            &shadow_mask_pipeline_layout,
            &shadow_shader,
            true,
            depth_clip_control_supported,
        );
        let bevy_shadow_translucent_pipeline = create_shadow_translucent_pipeline(
            &device,
            &shadow_mask_pipeline_layout,
            &shadow_shader,
            true,
            depth_clip_control_supported,
        );
        // Grass owns the canonical GodotGrass Perlin/domain-warp wind texture.
        // Create it before weather so rain can reuse that exact GPU resource rather
        // than generating or uploading a second copy of the same noise.
        init_mark!("shadow layouts+pipelines");
        let grass_renderer = GrassRenderer::new(
            &device,
            &queue,
            &camera_layout,
            &shadow_receiver_layout,
            &shadow_caster_layout,
            gpu_compaction_supported,
            config.format,
            msaa_samples,
        );
        init_mark!("grass renderer");
        let surface_sprite_effect_renderer =
            SurfaceSpriteEffectRenderer::new(&device, &camera_layout, config.format, msaa_samples);
        let mut weather = WeatherSystem::new(
            &device,
            &queue,
            &camera_layout,
            config.format,
            msaa_samples,
            grass_renderer.wind_noise_texture(),
        );
        init_mark!("sprites+weather");
        let shadow_resources = create_shadow_resources(
            &device,
            &shadow_receiver_layout,
            &shadow_caster_layout,
            shadow_pipeline,
            shadow_mask_pipeline,
            bevy_shadow_pipeline,
            bevy_shadow_mask_pipeline,
            bevy_shadow_translucent_pipeline,
            bevy_shadow_sky_pipeline,
            weather.fog.legacy_control_buffer(),
        );
        init_mark!("shadow resources");
        let cluster_shader = create_shader_module_timed(
            &device,
            wgpu::ShaderModuleDescriptor {
                label: Some("JKA clustered-light builder"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../cluster_lights.wgsl").into()),
            },
        );
        let cluster_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("JKA clustered-light compute pipeline layout"),
                bind_group_layouts: &[Some(&camera_layout), Some(&cluster_compute_layout)],
                immediate_size: 0,
            });
        let cluster_compute_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("JKA clustered-light compute pipeline"),
                layout: Some(&cluster_pipeline_layout),
                module: &cluster_shader,
                entry_point: Some("cs_main"),
                compilation_options: Default::default(),
                cache: None,
            });
        let lighting_settings_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("JKA clustered-light settings"),
                contents: bytemuck::bytes_of(&LightingSettings {
                    values: [0, 0, config.width.max(1), config.height.max(1)],
                    local_shadows: [0, 0, LOCAL_SHADOW_MAP_SIZE, 0],
                    feature_flags: [0; 4],
                    map_ambient: [0.0; 4],
                    map_minlight: [0.0; 4],
                }),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let pbr_settings_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("JKA PBR enhancement settings"),
            contents: bytemuck::bytes_of(&PbrSettings {
                values: [
                    u32::from(PBR_PROFILE_MATERIALS),
                    u32::from(PBR_PROFILE_PARALLAX_OCCLUSION),
                    40,
                    0,
                ],
                deluxe: [1.0, 1.0, 0.0, 0.0],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let world_shader = create_shader_module_timed(
            &device,
            wgpu::ShaderModuleDescriptor {
                label: Some("JKA BSP shader"),
                source: wgpu::ShaderSource::Wgsl(
                    compose_world_shader(include_str!("../bsp.wgsl")).into(),
                ),
            },
        );
        let world_shader_lean = create_shader_module_timed(
            &device,
            wgpu::ShaderModuleDescriptor {
                label: Some("JKA BSP shader (lean baseline)"),
                source: wgpu::ShaderSource::Wgsl(
                    compose_world_shader(include_str!("../bsp_lean.wgsl")).into(),
                ),
            },
        );
        let fast_world_shader = create_shader_module_timed(
            &device,
            wgpu::ShaderModuleDescriptor {
                label: Some("JKA BSP shader (known-fast baseline)"),
                source: wgpu::ShaderSource::Wgsl(
                    format!(
                        "{}\n{}",
                        include_str!("../bsp_fast.wgsl"),
                        include_str!("../surface_deformation.wgsl")
                    )
                    .into(),
                ),
            },
        );
        init_mark!("buffers+shader modules");
        let ocean_layout = crate::ocean::OceanGpu::create_render_layout(&device);
        let ocean_optics = crate::ocean::optics::Resources::new(
            &device,
            &cloud_shadow_layout,
            &ocean_layout,
            (1, 1, msaa_samples, config.format),
        );
        let ocean_inert_bind_group =
            crate::ocean::OceanGpu::create_inert_render_bind_group(&device, &ocean_layout);
        let missing_texture_data = crate::materials::missing_texture_data();
        let ocean_spray_albedo_data = load_texture_asset_or_missing(
            base,
            game,
            "textures/japro/sea_spray",
            false,
            true,
            true,
            "Ocean sea spray",
            &missing_texture_data,
        );
        let ocean_spray_albedo = upload_texture(&device, &queue, &ocean_spray_albedo_data);
        init_mark!("ocean optics+spray");
        // Detail textures are permanently AUTO and intentionally lazy. Renderer
        // startup must not scan/decode the DT_* library before a BSP even exists.
        // This neutral 1x1 is only a safe binding fallback until the current map's
        // MATERIAL_* plan requests real detail images.
        let detail_texture_auto = true;
        let detail_texture_data = TextureData {
            label: "neutral AUTO BSP detail fallback".to_owned(),
            source: None,
            width: 1,
            height: 1,
            rgba: vec![128, 128, 128, 255],
            rgba16f: None,
            mip_level_count: 1,
            clamp: false,
            srgb: true,
        };
        let detail_texture = upload_texture(&device, &queue, &detail_texture_data);
        let detail_auto_textures = HashMap::new();
        let world_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("JKA BSP pipeline layout"),
                bind_group_layouts: &[
                    Some(&camera_layout),
                    Some(&surface_layout),
                    Some(&lighting_layout),
                    Some(&shadow_receiver_layout),
                    Some(&planar_reflection_layout),
                    Some(&cloud_shadow_layout),
                    Some(&ocean_layout),
                ],
                immediate_size: 0,
            });
        let world_pipeline_layout_lean =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("JKA BSP pipeline layout (lean baseline)"),
                bind_group_layouts: &[
                    Some(&camera_layout),
                    Some(&surface_layout),
                    Some(&lighting_layout_lean),
                    Some(&shadow_receiver_layout),
                    Some(&planar_reflection_layout),
                ],
                immediate_size: 0,
            });
        let fast_world_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("JKA BSP pipeline layout (known-fast baseline)"),
                bind_group_layouts: &[Some(&fast_camera_layout), Some(&fast_surface_layout)],
                immediate_size: 0,
            });
        let wireframe_shader = create_shader_module_timed(
            &device,
            wgpu::ShaderModuleDescriptor {
                label: Some("JKA wireframe overlay shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../wireframe.wgsl").into()),
            },
        );
        let wireframe_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("JKA wireframe overlay pipeline layout"),
                bind_group_layouts: &[Some(&camera_layout)],
                immediate_size: 0,
            });
        // Debug line pipelines are intentionally cold/lazy. Normal gameplay never
        // creates them, and changing the category mask only compiles a family the
        // first frame that family is actually requested.
        let wireframe_pipeline = None;
        let debug_volume_shader = create_shader_module_timed(
            &device,
            wgpu::ShaderModuleDescriptor {
                label: Some("JKA trigger/clip debug volume shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../volume_debug.wgsl").into()),
            },
        );
        let surface_inspector_shader = create_shader_module_timed(
            &device,
            wgpu::ShaderModuleDescriptor {
                label: Some("JKA surface inspector shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../surface_inspector.wgsl").into()),
            },
        );
        // Diagnostic-only: compiled by ensure_lazy_scene_pipelines on first use.
        let surface_inspector_pipeline = None;

        let ui_shader = create_shader_module_timed(
            &device,
            wgpu::ShaderModuleDescriptor {
                label: Some("JKA UI shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../ui.wgsl").into()),
            },
        );
        let ui_texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA UI font layout"),
            entries: &[
                texture_entry(0),
                sampler_entry(1),
                texture_entry(2),
                texture_entry(3),
                texture_entry(4),
                texture_entry(5),
                texture_entry(6),
            ],
        });
        let ui_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("JKA UI pipeline layout"),
            bind_group_layouts: &[Some(&ui_texture_layout)],
            immediate_size: 0,
        });
        let ui_pipeline =
            create_ui_pipeline(&device, &ui_pipeline_layout, &ui_shader, config.format);
        // CGame screen-space effects (for example CG_SaberClashFlare) must use
        // their authored JKA shader stage/blend, not the normal alpha-only HUD
        // pipeline. Pipelines are compiled lazily on the first requested blend.
        let screen_fx_shader = create_shader_module_timed(
            &device,
            wgpu::ShaderModuleDescriptor {
                label: Some("JKA CGame screen FX shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../screen_fx.wgsl").into()),
            },
        );
        let screen_fx_texture_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("JKA CGame screen FX texture layout"),
                entries: &[texture_entry(0), sampler_entry(1)],
            });
        let screen_fx_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("JKA CGame screen FX pipeline layout"),
                bind_group_layouts: &[Some(&screen_fx_texture_layout)],
                immediate_size: 0,
            });
        let screen_fx_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("JKA CGame screen FX sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let screen_fx_vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA CGame screen FX vertices"),
            size: SCREEN_FX_BUFFER_BYTES,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        init_mark!("detail/wireframe/UI pipelines");
        let ui_font_data = load_ui_font_texture(base, game);
        let ui_font = upload_texture(&device, &queue, &ui_font_data);
        let (ui_small_font_data, ui_small_font_metrics) =
            load_ui_proportional_font(base, game, "ocr_a");
        let ui_small_font = upload_texture(&device, &queue, &ui_small_font_data);
        let (ui_splash_data, ui_splash_size) = load_ui_loading_texture(base, game, None);
        let ui_splash = upload_texture(&device, &queue, &ui_splash_data);
        // The movement key atlas (27 TGAs) is only drawn while cg_movementKeys is
        // on; set_ui loads it the first time that happens.
        let ui_keys = upload_texture(
            &device,
            &queue,
            &empty_ui_font_texture("JKA UI movement key atlas (not loaded)"),
        );
        // Lagometer frame / phone jack: read on first use, like the key atlas.
        let ui_icons = upload_texture(
            &device,
            &queue,
            &empty_ui_font_texture("JKA UI icon atlas (not loaded)"),
        );
        let ui_team_icons = upload_texture(
            &device,
            &queue,
            &empty_ui_font_texture("JKA UI team overlay atlas (not loaded)"),
        );
        // Keep the MP unknown-map art resident from renderer startup. Map-load UI
        // can bind this immediately, present it once, and only then resolve the
        // real levelshot. Slow PK3/image I/O can no longer expose the clear color.
        let (ui_unknown_map_data, ui_unknown_map_size) = load_ui_unknown_map_texture(base, game);
        let ui_unknown_map = upload_texture(&device, &queue, &ui_unknown_map_data);
        init_mark!("UI textures");
        let ui_font_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("JKA UI font sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let ui_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("JKA UI font bind group"),
            layout: &ui_texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&ui_font.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&ui_font_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&ui_small_font.view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&ui_splash.view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&ui_keys.view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&ui_icons.view),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&ui_team_icons.view),
                },
            ],
        });
        let ui_vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA retained UI vertices"),
            size: UI_BUFFER_BYTES,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let ui_dynamic_vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA hot HUD vertices"),
            size: UI_DYNAMIC_BUFFER_BYTES,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let ui_transient_vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA transient UI vertices"),
            size: UI_TRANSIENT_BUFFER_BYTES,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // egui is used for the modern Setup -> Video panel. It is kept as a
        // separate late overlay so the game/world renderer and legacy HUD/menu
        // remain untouched, and it costs nothing when the Video panel is closed.
        init_mark!("UI buffers");
        let egui_renderer = egui_wgpu::Renderer::new(
            &device,
            config.format,
            egui_wgpu::RendererOptions::default(),
        );

        init_mark!("egui renderer");
        let texture_filter = TextureFilter::Trilinear;
        let repeat_sampler = create_texture_sampler(&device, false, texture_filter);
        let clamp_sampler = create_texture_sampler(&device, true, texture_filter);
        // Companion maps are low-frequency material data. Keeping this pair at
        // fixed trilinear filtering avoids inheriting expensive 8x/16x AF from
        // the base-color sampler while preserving mipmapped linear filtering.
        let pbr_repeat_sampler = create_texture_sampler(&device, false, TextureFilter::Trilinear);
        let pbr_clamp_sampler = create_texture_sampler(&device, true, TextureFilter::Trilinear);
        let lightmap_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("lightmap sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        // Quake 3 / JKA outer skybox faces are sampled linearly without using
        // the normal world-texture mip selection. Keep this sampler independent
        // from r_textureMode so changing wall filtering does not blur the sky.
        let sky_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("JKA skybox sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            lod_min_clamp: 0.0,
            lod_max_clamp: 0.0,
            ..Default::default()
        });
        let white = upload_texture(
            &device,
            &queue,
            &TextureData {
                label: "white fallback".into(),
                source: None,
                width: 1,
                height: 1,
                rgba: vec![255, 255, 255, 255],
                rgba16f: None,
                mip_level_count: 1,
                clamp: true,
                srgb: true,
            },
        );
        let screen_fx_white_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("JKA CGame screen FX white bind group"),
            layout: &screen_fx_texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&white.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&screen_fx_sampler),
                },
            ],
        });
        let missing = upload_texture(&device, &queue, &missing_texture_data);
        init_mark!("samplers+white/missing textures");
        let dynamic_model_renderer =
            DynamicModelRenderer::new(&device, &camera_layout, config.format, msaa_samples, &white);
        dynamic_model_renderer.prewarm_core_model_pipelines(&mut pipeline_jobs, &device);
        init_mark!("dynamic model renderer");
        let flat_normal = upload_texture(
            &device,
            &queue,
            &TextureData {
                label: "flat normal fallback".into(),
                source: None,
                width: 1,
                height: 1,
                rgba: vec![128, 128, 255, 255],
                rgba16f: None,
                mip_level_count: 1,
                clamp: true,
                srgb: false,
            },
        );

        init_mark!("cluster+lighting buffers (pre-post)");
        // Only used while TAA is enabled; compiled by ensure_lazy_scene_pipelines.
        let taa_post_pipeline = None;
        // The resolve pass reads the march output. It gets its own bind group
        // so the march pipeline layout never mentions the texture it renders
        // into, which wgpu rejects outright.
        let cloud_resolve_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("JKA cloud resolve layout"),
                entries: &[texture_entry(0)],
            });
        let cloud_resolve_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("JKA cloud resolve pipeline layout"),
                bind_group_layouts: &[Some(&post_layout), Some(&cloud_resolve_layout)],
                immediate_size: 0,
            });
        let cloud_resolve_bind_group = create_cloud_resolve_bind_group(
            &device,
            &cloud_resolve_layout,
            &targets.cloud_march_view,
        );

        // Port the structure Bevy tracks for its SSAO follow-up work: evaluate AO
        // at half resolution, temporally accumulate it, then bilateral-upsample
        // against full-resolution linear depth in the main post pass.
        let ssao_temporal_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("JKA temporal SSAO layout"),
                entries: &[
                    unfilterable_texture_entry(0),
                    texture_entry(1),
                    sampler_entry(2),
                    uniform_entry(3, wgpu::ShaderStages::FRAGMENT),
                ],
            });
        let ssao_temporal_shader = create_shader_module_timed(
            &device,
            wgpu::ShaderModuleDescriptor {
                label: Some("JKA half-resolution temporal SSAO shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../ssao_temporal.wgsl").into()),
            },
        );
        let ssao_temporal_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("JKA temporal SSAO pipeline layout"),
                bind_group_layouts: &[Some(&ssao_temporal_layout)],
                immediate_size: 0,
            });

        // Port Bevy's hybrid SSR ray refinement and FidelityFX's temporal
        // reflection history as a separate half-resolution pass. The expensive
        // ray march runs at one quarter of the output pixels; the main post pass
        // only performs a four-tap depth-aware upsample and composite.
        let ssr_temporal_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("JKA temporal SSR layout"),
                entries: &[
                    texture_entry(0),
                    sampler_entry(1),
                    unfilterable_texture_entry(2),
                    texture_entry(3),
                    unfilterable_texture_entry(4),
                    uniform_entry(5, wgpu::ShaderStages::FRAGMENT),
                    // Share the same static weather-surface field/uniform used by
                    // BSP shading so SSR sees the wet-adjusted surface response.
                    unfilterable_texture_entry(6),
                    nonfiltering_sampler_entry(7, wgpu::ShaderStages::FRAGMENT),
                    uniform_entry(8, wgpu::ShaderStages::FRAGMENT),
                    // Cached per-surface reflection eligibility from the depth prepass.
                    texture_entry(9),
                    // Resolved final scene depth used to reject a BSP reflection
                    // when a dynamic entity/ocean surface owns the pixel instead.
                    unfilterable_texture_entry(10),
                ],
            });
        let ssr_temporal_shader = create_shader_module_timed(
            &device,
            wgpu::ShaderModuleDescriptor {
                label: Some("JKA half-resolution temporal SSR shader"),
                source: wgpu::ShaderSource::Wgsl(
                    weather::with_bound_weather_surface(include_str!("../ssr_temporal.wgsl"))
                        .into(),
                ),
            },
        );
        let ssr_temporal_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("JKA temporal SSR pipeline layout"),
                bind_group_layouts: &[Some(&ssr_temporal_layout)],
                immediate_size: 0,
            });

        // Gamma is common even in otherwise-minimal configurations. Keep a tiny
        // specialized pipeline for the gamma-only case so advanced post features
        // add zero shader/resource work when disabled. The full fused post path
        // remains unchanged whenever any other post effect is active.
        let gamma_post_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA gamma-only post layout"),
            entries: &[
                texture_entry(0),
                sampler_entry(1),
                uniform_entry(2, wgpu::ShaderStages::FRAGMENT),
                texture_3d_entry(3),
                sampler_entry(4),
            ],
        });
        let gamma_post_shader = create_shader_module_timed(
            &device,
            wgpu::ShaderModuleDescriptor {
                label: Some("JKA gamma-only post shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../post_gamma.wgsl").into()),
            },
        );
        let gamma_post_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("JKA gamma-only post pipeline layout"),
                bind_group_layouts: &[Some(&gamma_post_layout)],
                immediate_size: 0,
            });
        let fast_post_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA known-fast gamma post layout"),
            entries: &[
                texture_entry(0),
                sampler_entry(1),
                uniform_entry(2, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let fast_post_shader = create_shader_module_timed(
            &device,
            wgpu::ShaderModuleDescriptor {
                label: Some("JKA known-fast gamma post shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../post_fast.wgsl").into()),
            },
        );
        let fast_post_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("JKA known-fast gamma post pipeline layout"),
                bind_group_layouts: &[Some(&fast_post_layout)],
                immediate_size: 0,
            });
        // Every always-on post/screen-space pipeline is independent once its
        // shader and layout exist. Their driver compiles dominate startup, so
        // overlap them instead of stalling this thread on one after another
        // (wgpu pipeline creation is thread-safe; see compile_pipeline_jobs).
        let post_compile_started = Instant::now();
        let (
            cloud_march_pipeline,
            cloud_resolve_pipeline,
            ssao_temporal_pipeline,
            ssr_temporal_pipeline,
            ssr_visibility_resources,
            gamma_post_pipeline,
            fast_post_pipeline,
        ) = thread::scope(|scope| {
            let cloud_march = scope.spawn(|| {
                timed_init_step("cloud march pipeline", || {
                    create_cloud_pass_pipeline(
                        &device,
                        &post_pipeline_layout,
                        &post_shader,
                        "fs_cloud_march",
                        "JKA volumetric cloud march pipeline",
                    )
                })
            });
            let cloud_resolve = scope.spawn(|| {
                timed_init_step("cloud resolve pipeline", || {
                    create_cloud_pass_pipeline(
                        &device,
                        &cloud_resolve_pipeline_layout,
                        &post_shader,
                        "fs_cloud_resolve",
                        "JKA volumetric cloud resolve pipeline",
                    )
                })
            });
            let ssao_temporal = scope.spawn(|| {
                timed_init_step("SSAO temporal pipeline", || {
                    create_ssao_temporal_pipeline(
                        &device,
                        &ssao_temporal_pipeline_layout,
                        &ssao_temporal_shader,
                    )
                })
            });
            let ssr_temporal = scope.spawn(|| {
                timed_init_step("SSR temporal pipeline", || {
                    create_ssr_temporal_pipeline(
                        &device,
                        &ssr_temporal_pipeline_layout,
                        &ssr_temporal_shader,
                    )
                })
            });
            let ssr_visibility = scope.spawn(|| {
                timed_init_step("SSR visibility resources", || {
                    create_ssr_visibility_resources(&device, &targets, msaa_samples)
                })
            });
            let gamma_post = scope.spawn(|| {
                timed_init_step("gamma post pipeline", || {
                    create_gamma_post_pipeline(
                        &device,
                        &gamma_post_pipeline_layout,
                        &gamma_post_shader,
                        config.format,
                    )
                })
            });
            // The last one runs on this thread rather than idling at the join.
            let fast_post = timed_init_step("fast post pipeline", || {
                create_gamma_post_pipeline(
                    &device,
                    &fast_post_pipeline_layout,
                    &fast_post_shader,
                    config.format,
                )
            });
            let joined = |name: &str| format!("{name} pipeline compile thread panicked");
            (
                cloud_march.join().expect(&joined("cloud march")),
                cloud_resolve.join().expect(&joined("cloud resolve")),
                ssao_temporal.join().expect(&joined("SSAO")),
                ssr_temporal.join().expect(&joined("SSR")),
                ssr_visibility.join().expect(&joined("SSR visibility")),
                gamma_post.join().expect(&joined("gamma post")),
                fast_post,
            )
        });
        rverbose!(
            1,
            "[RENDER INIT] post/screen-space pipelines (7) compiled in parallel: {:.1} ms wall",
            post_compile_started.elapsed().as_secs_f64() * 1000.0
        );
        let (ssr_visibility_layout, ssr_visibility_pipeline, ssr_visibility_bind_group) =
            ssr_visibility_resources
                .map_or((None, None, None), |(layout, pipeline, bind_group)| {
                    (Some(layout), Some(pipeline), Some(bind_group))
                });
        let post_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("JKA post-process sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let (color_lut_texture, color_lut_view) = create_color_lut_texture(
            &device,
            &queue,
            ColorLutPreset::Off,
            1.0,
            Default::default(),
            1.0,
            base,
            game,
        );
        let color_lut_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("JKA color LUT sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        // Finalized cloud path is lazy: the shared post pipeline is compiled once,
        // while the heavy 96^3 noise/cached-FBM texture is only generated and
        // uploaded if volumetric clouds are actually enabled.
        let cloud_noise_resources = None;
        let cloud_noise_fallback = create_cloud_fallback_resources(&device, &queue);
        // Bloom/halation share a low-resolution highlight pyramid. The pyramid is
        // generated only while either effect is enabled; the maximum-FPS path
        // does not allocate the targets and records no bloom passes.
        let bloom_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA bloom downsample layout"),
            entries: &[texture_entry(0), sampler_entry(1)],
        });
        init_mark!("post shaders/pipelines");
        let bloom_shader = create_shader_module_timed(
            &device,
            wgpu::ShaderModuleDescriptor {
                label: Some("JKA low-resolution bloom shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../bloom.wgsl").into()),
            },
        );
        let bloom_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("JKA bloom downsample pipeline layout"),
                bind_group_layouts: &[Some(&bloom_layout)],
                immediate_size: 0,
            });
        let bloom_extract_pipeline =
            create_bloom_pipeline(&device, &bloom_pipeline_layout, &bloom_shader, "fs_extract");
        let bloom_downsample_pipeline = create_bloom_pipeline(
            &device,
            &bloom_pipeline_layout,
            &bloom_shader,
            "fs_downsample",
        );

        // Fast DOF follows Bevy's Gaussian mode structurally: one horizontal
        // separable pass here, with the vertical pass fused into post.wgsl.
        // Resources only exist while DOF is enabled.
        let dof_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA Gaussian DOF layout"),
            entries: &[
                texture_entry(0),
                sampler_entry(1),
                unfilterable_texture_entry(2),
                uniform_entry(3, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let dof_shader = create_shader_module_timed(
            &device,
            wgpu::ShaderModuleDescriptor {
                label: Some("JKA Gaussian DOF shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../dof.wgsl").into()),
            },
        );
        let dof_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("JKA Gaussian DOF pipeline layout"),
            bind_group_layouts: &[Some(&dof_layout)],
            immediate_size: 0,
        });
        let dof_pipeline = create_dof_pipeline(&device, &dof_pipeline_layout, &dof_shader);
        let dof_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("JKA Gaussian DOF uniform"),
            contents: bytemuck::bytes_of(&DofUniform {
                focus_strength: [4096.0, 0.0, config.width as f32, config.height as f32],
                quality: [DofQuality::Adaptive.shader_value(), 0.0, 0.0, 0.0],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let gamma = 1.0;
        let post_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("JKA post-process uniform"),
            contents: bytemuck::bytes_of(&PostUniform {
                color: PostColorSettings {
                    gamma,
                    tone_mapping: 0.0,
                    bloom: 0.0,
                    ssao: 0.0,
                },
                aa: PostAaSettings {
                    fxaa: 0.0,
                    viewport_width: config.width as f32,
                    viewport_height: config.height as f32,
                    taa: 0.0,
                },
                taa_params: [0.0; 4],
                scene: PostSceneSettings {
                    contact_shadows: 0.0,
                    volumetric_fog: 0.0,
                    ssr: 0.0,
                    history_valid: 0.0,
                },
                film: PostFilmSettings {
                    halation: 0.0,
                    chromatic_aberration: 0.0,
                    vignette: 0.0,
                    lut_strength: 0.0,
                },
                grain: PostGrainSettings {
                    strength: 0.0,
                    grain_size: 1.0,
                    time: 0.0,
                    exposure: 0.0,
                },
                camera_fx: PostCameraFxSettings {
                    motion_blur_scale: 0.0,
                    depth_of_field: 0.0,
                    dof_focus_distance: 4096.0,
                    dof_quality: DofQuality::Adaptive.shader_value(),
                },
                legacy_fog: [0.0; 4],
                clouds: CloudRenderSettings {
                    values: [0.0, 0.0, 0.55, 0.55],
                },
                cloud_layer: CloudLayerSettings {
                    values: [2400.0, 1200.0, 120.0, 20.0_f32.to_radians()],
                },
                cloud_sun: CloudSunSettings {
                    direction_intensity: [
                        FALLBACK_SUN_DIRECTION[0],
                        FALLBACK_SUN_DIRECTION[1],
                        FALLBACK_SUN_DIRECTION[2],
                        FALLBACK_SUN_INTENSITY,
                    ],
                    color_density: [
                        FALLBACK_SUN_COLOR[0],
                        FALLBACK_SUN_COLOR[1],
                        FALLBACK_SUN_COLOR[2],
                        1.0,
                    ],
                },
                cloud_shadow: PostCloudShadowSettings {
                    values: [0.0, 0.75, 0.0, 0.0],
                },
                cloud_shaping: PostCloudShapingSettings {
                    values: [0.35, 0.25, 0.0, 0.5],
                },
                cloud_sky_ambient: PostCloudSkyAmbientSettings {
                    values: [0.45, 0.55, 0.75, 1.0],
                },
                cloud_temporal_tuning: PostCloudTemporalTuningSettings {
                    values: [0.85, 1.0, 1.0, 0.0],
                },
                cloud_variation: PostCloudVariationSettings {
                    values: [0.4, 0.333, 0.0, 0.0],
                },
                cloud_temporal: PostCloudTemporalSettings {
                    values: [0.0, 0.0, 0.88, 0.0],
                },
                cloud_wind: PostCloudWindSettings::zeroed(),
                rain: [
                    0.0,
                    RainIntensity::Rain.shader_value(),
                    RainIntensity::Rain.haze_strength(),
                    0.0,
                ],
                weather_occlusion: [0.0; 4],
                weather_look: [0.0; 4],
                underwater: [NO_WATER_SURFACE, 1.0, 0.0, 0.0],
                cloud_foreground: [0.0; 4],
                cloud_blades: [[0.0; 4]; MAX_CLOUD_FOREGROUND_BLADES * 2],
                camera_pos_time: [0.0; 4],
                prev_camera_pos_time: [0.0; 4],
                view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                inv_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                cloud_inv_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                cloud_prev_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                prev_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                motion_prev_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let auto_exposure_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("JKA auto-exposure layout"),
                entries: &[
                    unfilterable_texture_entry_stages(0, wgpu::ShaderStages::COMPUTE),
                    storage_buffer_entry(1, false, wgpu::ShaderStages::COMPUTE),
                    uniform_entry(2, wgpu::ShaderStages::COMPUTE),
                ],
            });
        init_mark!("bloom/dof");
        let auto_exposure_shader = create_shader_module_timed(
            &device,
            wgpu::ShaderModuleDescriptor {
                label: Some("JKA auto-exposure shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../auto_exposure.wgsl").into()),
            },
        );
        let auto_exposure_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("JKA auto-exposure pipeline layout"),
                bind_group_layouts: &[Some(&auto_exposure_layout)],
                immediate_size: 0,
            });
        let auto_exposure_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("JKA auto-exposure compute pipeline"),
                layout: Some(&auto_exposure_pipeline_layout),
                module: &auto_exposure_shader,
                entry_point: Some("cs_main"),
                compilation_options: Default::default(),
                cache: None,
            });
        let auto_exposure_state_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("JKA auto-exposure state"),
                contents: bytemuck::bytes_of(&AutoExposureState {
                    exposure_ev: 0.0,
                    average_luminance: 1.0,
                    target_ev: 0.0,
                    reserved: 0.0,
                }),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });
        let auto_exposure_settings_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("JKA auto-exposure settings"),
                contents: bytemuck::bytes_of(&AutoExposureUniform {
                    params: [0.0, 0.0, -2.0, 2.0],
                    adaptation: [0.18, 1.5, 3.0, 0.0],
                }),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let auto_exposure_bind_group = create_auto_exposure_bind_group(
            &device,
            &auto_exposure_layout,
            &targets.scene_view,
            &auto_exposure_state_buffer,
            &auto_exposure_settings_buffer,
        );

        let ssao_temporal_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("JKA temporal SSAO uniform"),
            contents: bytemuck::bytes_of(&SsaoTemporalUniform {
                viewport_history: [config.width as f32, config.height as f32, 0.0, 0.0],
                camera_pos_time: [0.0; 4],
                previous_camera_pos: [0.0; 4],
                view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                inv_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                prev_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let ssao_temporal_bind_groups = create_ssao_temporal_bind_groups(
            &device,
            &ssao_temporal_layout,
            &targets.linear_depth_view,
            targets.ssao_history.as_ref(),
            &post_sampler,
            &ssao_temporal_buffer,
        );
        let ssr_temporal_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("JKA temporal SSR uniform"),
            contents: bytemuck::bytes_of(&SsrTemporalUniform {
                viewport_history: [config.width as f32, config.height as f32, 0.0, 0.0],
                camera_pos_time: [0.0; 4],
                previous_camera_pos: [0.0; 4],
                view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                inv_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                prev_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let ssr_temporal_bind_groups = create_ssr_temporal_bind_groups(
            &device,
            &ssr_temporal_layout,
            &targets.scene_view,
            &targets.linear_depth_view,
            targets.ssr_history.as_ref(),
            &post_sampler,
            &ssr_temporal_buffer,
            &shadow_resources.weather_height_view,
            &shadow_resources.weather_sampler,
            &shadow_resources.weather_surface_buffer,
            &targets.reflection_mask_view,
            targets.ssr_visibility_view.as_ref(),
        );

        let post_bind_groups = std::array::from_fn(|history_index| {
            std::array::from_fn(|ssao_index| {
                std::array::from_fn(|ssr_index| {
                    std::array::from_fn(|cloud_index| {
                        let ssao_view = targets
                            .ssao_history
                            .as_ref()
                            .map_or(&targets.scene_view, |history| &history.views[ssao_index]);
                        let (ssr_radiance_view, ssr_depth_view) =
                            targets.ssr_history.as_ref().map_or(
                                (&targets.scene_view, &targets.linear_depth_view),
                                |history| {
                                    (
                                        &history.radiance_views[ssr_index],
                                        &history.depth_views[ssr_index],
                                    )
                                },
                            );
                        create_post_bind_group(
                            &device,
                            &post_layout,
                            &targets.scene_view,
                            &targets.linear_depth_view,
                            &targets.history_views[history_index],
                            ssao_view,
                            ssr_radiance_view,
                            ssr_depth_view,
                            &post_sampler,
                            &post_buffer,
                            &weather.fog.resources.buffer,
                            targets.bloom.as_ref(),
                            targets.dof.as_ref(),
                            &color_lut_view,
                            &color_lut_sampler,
                            &cloud_noise_fallback.detail_view,
                            &cloud_noise_fallback.sampler,
                            &targets.cloud_transfer_views[cloud_index],
                            &targets.scene_view,
                            &targets.rain_haze_mask_view,
                            &cloud_noise_fallback.weather_view,
                            &targets.motion_vector_view,
                            &targets.ao_depth_view,
                            &targets.reflection_mask_view,
                            &auto_exposure_state_buffer,
                            targets.ssr_visibility_view.as_ref().unwrap_or(&white.view),
                        )
                    })
                })
            })
        });
        let gamma_post_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("JKA gamma-only post uniform"),
            contents: bytemuck::bytes_of(&GammaPostUniform {
                values: [gamma, 0.0, 0.0, 0.0],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bloom_bind_groups = create_bloom_bind_groups(
            &device,
            &bloom_layout,
            &post_sampler,
            &targets.scene_view,
            targets.bloom.as_ref(),
        );
        let dof_bind_group = create_dof_bind_group(
            &device,
            &dof_layout,
            &targets.scene_view,
            &targets.linear_depth_view,
            &post_sampler,
            &dof_buffer,
            targets.dof.as_ref(),
        );

        let gamma_post_bind_group = create_gamma_post_bind_group(
            &device,
            &gamma_post_layout,
            &targets.scene_view,
            &post_sampler,
            &gamma_post_buffer,
            &color_lut_view,
            &color_lut_sampler,
        );
        let fast_post_bind_group = create_fast_post_bind_group(
            &device,
            &fast_post_layout,
            &targets.scene_view,
            &post_sampler,
            &gamma_post_buffer,
        );

        let gpu_profiler = GpuProfiler::new(
            &device,
            &queue,
            timestamp_supported,
            timestamp_inside_encoders,
            timestamp_inside_passes,
        );

        init_mark!("auto-exposure..pre-depth");
        let depth_prepass_shader = create_shader_module_timed(
            &device,
            wgpu::ShaderModuleDescriptor {
                label: Some("JKA AO depth prepass shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../depth_prepass.wgsl").into()),
            },
        );
        let depth_prepass_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("JKA AO depth prepass pipeline layout"),
            bind_group_layouts: &[
                Some(&camera_layout),
                Some(&surface_layout),
                Some(&planar_reflection_layout),
            ],
            immediate_size: 0,
        });
        let depth_prepass_mask_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("JKA alpha-tested depth prepass pipeline layout"),
                bind_group_layouts: &[
                    Some(&camera_layout),
                    Some(&surface_layout),
                    Some(&planar_reflection_layout),
                ],
                immediate_size: 0,
            });
        let depth_prepass_pipeline =
            create_depth_prepass_pipeline(&device, &depth_prepass_layout, &depth_prepass_shader);
        let depth_prepass_mask_pipeline = create_depth_prepass_mask_pipeline(
            &device,
            &depth_prepass_mask_layout,
            &depth_prepass_shader,
        );

        let hiz_prepass_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("JKA Hi-Z early depth pipeline layout"),
            bind_group_layouts: &[Some(&camera_layout), Some(&surface_layout)],
            immediate_size: 0,
        });
        let hiz_prepass_pipeline =
            create_hiz_prepass_pipeline(&device, &hiz_prepass_layout, &depth_prepass_shader, false);
        let hiz_prepass_mask_pipeline =
            create_hiz_prepass_pipeline(&device, &hiz_prepass_layout, &depth_prepass_shader, true);

        let hiz_build_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA Hi-Z build layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::R32Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let hiz_build_shader = create_shader_module_timed(
            &device,
            wgpu::ShaderModuleDescriptor {
                label: Some("JKA Hi-Z build shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../hiz_build.wgsl").into()),
            },
        );
        let hiz_build_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("JKA Hi-Z build pipeline layout"),
                bind_group_layouts: &[Some(&hiz_build_layout)],
                immediate_size: 0,
            });
        let hiz_build_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("JKA Hi-Z build pipeline"),
            layout: Some(&hiz_build_pipeline_layout),
            module: &hiz_build_shader,
            entry_point: Some("cs_main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let hiz_build_bind_group =
            create_hiz_build_bind_group(&device, &hiz_build_layout, &targets);

        let hiz_reduce_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA Hi-Z reduce layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::R32Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let hiz_reduce_shader = create_shader_module_timed(
            &device,
            wgpu::ShaderModuleDescriptor {
                label: Some("JKA Hi-Z reduce shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../hiz_reduce.wgsl").into()),
            },
        );
        let hiz_reduce_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("JKA Hi-Z reduce pipeline layout"),
                bind_group_layouts: &[Some(&hiz_reduce_layout)],
                immediate_size: 0,
            });
        let hiz_reduce_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("JKA Hi-Z reduce pipeline"),
                layout: Some(&hiz_reduce_pipeline_layout),
                module: &hiz_reduce_shader,
                entry_point: Some("cs_main"),
                compilation_options: Default::default(),
                cache: None,
            });
        let hiz_reduce_bind_groups =
            create_hiz_reduce_bind_groups(&device, &hiz_reduce_layout, &targets);

        let gpu_cull_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA GPU cull layout"),
            entries: &[
                storage_buffer_entry(0, true, wgpu::ShaderStages::COMPUTE),
                storage_buffer_entry(1, false, wgpu::ShaderStages::COMPUTE),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                uniform_entry(3, wgpu::ShaderStages::COMPUTE),
                storage_buffer_entry(4, true, wgpu::ShaderStages::COMPUTE),
                storage_buffer_entry(5, false, wgpu::ShaderStages::COMPUTE),
                storage_buffer_entry(6, false, wgpu::ShaderStages::COMPUTE),
                storage_buffer_entry(7, false, wgpu::ShaderStages::COMPUTE),
                storage_buffer_entry(8, false, wgpu::ShaderStages::COMPUTE),
                storage_buffer_entry(9, false, wgpu::ShaderStages::COMPUTE),
            ],
        });
        init_mark!("depth prepass/hiz");
        let gpu_cull_shader = create_shader_module_timed(
            &device,
            wgpu::ShaderModuleDescriptor {
                label: Some("JKA GPU cull shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../gpu_cull.wgsl").into()),
            },
        );
        let gpu_cull_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("JKA GPU cull pipeline layout"),
                bind_group_layouts: &[Some(&camera_layout), Some(&gpu_cull_layout)],
                immediate_size: 0,
            });
        let gpu_cull_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("JKA GPU cull pipeline"),
            layout: Some(&gpu_cull_pipeline_layout),
            module: &gpu_cull_shader,
            entry_point: Some("cs_main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let gpu_cull_early_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("JKA Hi-Z early draw-list pipeline"),
                layout: Some(&gpu_cull_pipeline_layout),
                module: &gpu_cull_shader,
                entry_point: Some("cs_early"),
                compilation_options: Default::default(),
                cache: None,
            });
        let gpu_cull_settings_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("JKA GPU cull settings"),
                contents: bytemuck::bytes_of(&GpuCullSettings {
                    viewport_mips_flags: [
                        targets.hiz_width,
                        targets.hiz_height,
                        targets.hiz_mip_count,
                        0,
                    ],
                    active_compaction: [0, 0, u32::from(gpu_compaction_supported), 0],
                }),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });

        let cull_debug_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA cull debug layout"),
            entries: &[storage_buffer_entry(0, true, wgpu::ShaderStages::VERTEX)],
        });
        init_mark!("gpu cull");
        let cull_debug_shader = create_shader_module_timed(
            &device,
            wgpu::ShaderModuleDescriptor {
                label: Some("JKA cull debug shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../cull_debug.wgsl").into()),
            },
        );
        let cull_debug_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("JKA cull debug pipeline layout"),
                bind_group_layouts: &[Some(&camera_layout), Some(&cull_debug_layout)],
                immediate_size: 0,
            });
        // Diagnostic-only: compiled by ensure_lazy_scene_pipelines on first use.
        let cull_debug_pipeline = None;
        let cull_diagnostics_readback = CullDiagnosticsReadback::new(&device);

        // Weather owns persistent precipitation GPU state. The haze binding is
        // attached here because it depends on post-process targets created later
        // than the core weather pipelines.
        weather.rain.rebuild_haze_bind_group(
            &device,
            &post_buffer,
            &targets.linear_depth_view,
            &targets.rain_haze_mask_view,
        );
        let (screenshot_worker_tx, screenshot_job_rx) = mpsc::channel::<ScreenshotEncodeJob>();
        let (screenshot_result_tx, screenshot_worker_rx) =
            mpsc::channel::<Result<ScreenshotOutput, String>>();
        thread::Builder::new()
            .name("screenshot-encode".into())
            .spawn(move || {
                while let Ok(job) = screenshot_job_rx.recv() {
                    let result = encode_screenshot_job(job);
                    if screenshot_result_tx.send(result).is_err() {
                        break;
                    }
                }
            })
            .map_err(|error| format!("could not start screenshot worker: {error}"))?;

        init_marks.push((
            "remaining pipelines/targets",
            init_phase.elapsed().as_secs_f64() * 1000.0,
        ));
        rverbose!(
            1,
            "[RENDER INIT] phases: {}",
            init_marks
                .iter()
                .map(|(name, ms)| format!("{name} {ms:.1}"))
                .collect::<Vec<_>>()
                .join(" | ")
        );
        Ok(Self {
            model_frame_log: crate::model_frame_log::ModelFrameLog::new(
                jka_assets::pk3::active_game_directory(base, game).join("hitch"),
            ),
            base: base.to_path_buf(),
            adapter,
            window,
            surface,
            device,
            queue,
            pipeline_jobs,
            config,
            ray_tracing_supported,
            ray_traced_shadows: None,
            size,
            present_modes: caps.present_modes,
            scene_rebuild_pending: false,
            supported_msaa: surface_supported_msaa,
            hdr_supported_msaa,
            vsync: VsyncMode::Off,
            msaa_samples,
            targets,
            camera_buffer,
            camera_layout,
            camera_bind_group,
            fast_camera_layout,
            fast_camera_bind_group,
            companion_scene_targets: HashMap::new(),
            companion_scene_generation: 0,
            companion_scene_next_sequence: 1,
            companion_scene_ready: Vec::new(),
            planar_camera_buffers,
            planar_camera_bind_groups,
            sky_portal_camera_buffer,
            sky_portal_camera_bind_group,
            surface_layout,
            fast_surface_layout,
            surface_deformation,
            planar_reflection_layout,
            planar_reflection,
            weather,
            grass_renderer,
            surface_sprite_effect_renderer,
            ocean_layout,
            ocean_optics_layout: cloud_shadow_layout,
            ocean_optics,
            ocean_inert_bind_group,
            ocean_spray_albedo,
            detail_texture,
            detail_texture_auto,
            detail_texture_game: game.map(Path::to_path_buf),
            detail_auto_textures,
            ocean: None,
            authored_oceans: Vec::new(),
            ocean_spray_pipeline: None,
            authored_ocean_definitions: Vec::new(),
            water_boxes: Vec::new(),
            camera_water_surface: NO_WATER_SURFACE,
            ocean_enabled: false,
            ocean_settings: crate::ocean::OceanSettings::default(),
            world_pipeline_layout,
            world_pipeline_layout_lean,
            fast_world_pipeline_layout,
            world_shader,
            world_shader_lean,
            fast_world_shader,
            wireframe_supported,
            wireframe_mask: 0,
            pvs_mode: PvsMode::Auto,
            wireframe_pipeline_layout,
            wireframe_shader,
            wireframe_pipeline,
            world_wireframe_pipelines: BTreeMap::new(),
            fast_world_wireframe_pipeline: None,
            surface_inspector_shader,
            surface_inspector_pipeline,
            debug_volumes: DebugVolumeRenderer::default(),
            debug_volume_shader,
            inspector_vertex_range: None,
            inspector_entity_num: None,
            blob_mark_buffer: jka_assets::bsp::MarkBuffer::default(),
            ui_pipeline,
            screen_fx_shader,
            screen_fx_texture_layout,
            screen_fx_pipeline_layout,
            screen_fx_sampler,
            screen_fx_white_bind_group,
            screen_fx_pipelines: HashMap::new(),
            screen_fx_textures: HashMap::new(),
            screen_fx_vertex_buffer,
            screen_fx_vertex_count: 0,
            screen_fx_batches: Vec::new(),
            ui_texture_layout,
            ui_bind_group,
            _ui_font: ui_font,
            _ui_small_font: ui_small_font,
            _ui_splash: ui_splash,
            _ui_keys: ui_keys,
            ui_keys_loaded: false,
            _ui_icons: ui_icons,
            ui_icons_loaded: false,
            _ui_team_icons: ui_team_icons,
            ui_team_icon_key: Vec::new(),
            ui_binding_is_unknown_map: false,
            _ui_unknown_map: ui_unknown_map,
            ui_unknown_map_size,
            ui_splash_size,
            ui_loading_image_key: None,
            ui_pending_levelshot: None,
            ui_small_font_metrics,
            _ui_font_sampler: ui_font_sampler,
            ui_vertex_buffer,
            ui_vertex_count: 0,
            ui_dynamic_vertex_buffer,
            ui_dynamic_vertex_count: 0,
            ui_dynamic_stable_vertex_count: 0,
            ui_dynamic_vertices: Vec::with_capacity(2048),
            ui_transient_vertex_buffer,
            ui_transient_vertex_count: 0,
            ui_transient_stable_vertex_count: 0,
            ui_transient_vertices: Vec::with_capacity(4096),
            player_names: None,
            ui_trace: env_flag("JKA_UI_TRACE"),
            ui_state: UiSnapshot::default(),
            egui_renderer,
            egui_paint_jobs: Vec::new(),
            egui_pixels_per_point: 1.0,
            egui_active: false,
            egui_pending_free: Vec::new(),
            asset_preview_mode: false,
            asset_preview_viewport: None,
            menu_backdrop: None,
            world: None,
            world_videos: Vec::new(),
            dynamic_model_renderer,
            white,
            missing,
            flat_normal,
            repeat_sampler,
            clamp_sampler,
            pbr_repeat_sampler,
            pbr_clamp_sampler,
            lightmap_sampler,
            sky_sampler,
            texture_filter,
            picmip: 0,
            detail_textures_mode: DetailTextureMode::Off,
            jump_shade: JumpShadeState::Off,
            detail_texture_fade: false,
            detail_texture_fade_distance: 512.0,
            gamma,
            baked_brightness: crate::renderer::brightness::BakedBrightness::default(),
            gamma_method: crate::gamma::GammaMethod::Shader,
            hdr_enabled: false,
            tone_mapping_enabled: false,
            auto_exposure_enabled: false,
            bloom_enabled: false,
            halation_enabled: false,
            ssao_enabled: false,
            static_bsp_ao_enabled: false,
            static_bsp_ao_lightmap: true,
            static_bsp_ao_samples: 32,
            static_bsp_ao_resolution: 3,
            static_bsp_ao_strength: 75,
            static_bsp_ao_range: 100,
            static_bsp_ao_current_cell: false,
            static_ao_pending: None,
            static_ao_deferred_force_rebuild: false,
            fxaa_enabled: false,
            smaa_enabled: false,
            taa_enabled: false,
            contact_shadows_enabled: false,
            sun_override: false,
            sun_yaw: 314.25595,
            sun_pitch: 57.04725,
            sun_intensity: 250.0,
            sun_color: [1.0, 1.0, 1.0],
            sun_visibility: SunVisibilityMode::SkyPortals,
            entity_sun_lighting: false,
            clouds_enabled: true,
            cloud_type: CloudType::Storm,
            cloud_quality: 1.0,
            cloud_coverage: 0.6,
            cloud_height: 4600.0,
            cloud_thickness: 1200.0,
            weather_wind: crate::ocean::OceanWind {
                speed: 167.0,
                direction: 220.0,
                gust: 0.2,
                shift: 0.0,
            },
            cloud_shadows_enabled: true,
            cloud_render_resolution: CloudRenderResolution::Full,
            cloud_temporal_enabled: true,
            cloud_temporal_depth_fix: true,
            cloud_shear: 0.2,
            cloud_base_variation: 1.0,
            cloud_shape_evolution: false,
            cloud_terrain_interaction: false,
            cloud_empty_skip: false,
            cloud_aerial: 0.0,
            cloud_sky_ambient_enabled: false,
            cloud_history_blend: 0.85,
            cloud_motion_reject: 1.0,
            cloud_history_depth_reject: false,
            cloud_thickness_variation: 0.75,
            cloud_size: 0.333,
            cloud_sky_average: [0.45, 0.55, 0.75],
            grass_enabled: true,
            grass_precompute_enabled: true,
            grass_mid_lod_enabled: true,
            grass_front_to_back_enabled: true,
            contact_shadow_debug: 0,
            cloud_history_valid: false,
            cloud_history_key: None,
            cloud_history_read_index: 0,
            cloud_temporal_frame_index: 0,
            reflection_quality: ReflectionQuality::Off,
            reflection_debug_enabled: false,
            ssr_enabled: false,
            chromatic_aberration_strength: 0.0,
            vignette_enabled: false,
            film_grain_strength: 0.0,
            motion_blur_strength: 0.0,
            depth_of_field_strength: 0.0,
            dof_quality: DofQuality::Adaptive,
            dof_focus_distance: 4096.0,
            dof_focus_valid: false,
            motion_blur_runtime_scale: 0.0,
            previous_frame_time: 0.0,
            color_lut_preset: ColorLutPreset::Off,
            color_lut_strength: 1.0,
            split_toning: Default::default(),
            color_lut_effective_strength: 0.0,
            _color_lut_texture: color_lut_texture,
            color_lut_view,
            color_lut_sampler,
            cloud_noise_resources,
            cloud_noise_fallback,
            frame_plan: FramePlan::default(),
            force_unified_world: false,
            indirect_supported,
            gpu_compaction_supported,
            compact_group_scratch: Vec::new(),
            cpu_compact_indirect_scratch: Vec::new(),
            cpu_compact_count_scratch: Vec::new(),
            reflection_compact_scratch: Vec::new(),
            reflection_compact_groups: Vec::new(),
            inline_visible_scratch: Vec::new(),
            post_ocean_transparent_scratch: Vec::new(),
            inline_pose_scratch: Vec::new(),
            inline_draw_scratch: InlineDrawScratch::default(),
            gpu_driven_enabled: false,
            hiz_occlusion_enabled: false,
            dynamic_lights_mode: DynamicLightsMode::Off,
            rt_samples: 1,
            dynamic_light_falloff: 0,
            rt_reduced_shadows: false,
            clustered_lighting_enabled: false,
            map_light_simulation_enabled: false,
            classic_fullbright: false,
            classic_vertex_light: false,
            classic_lightmap_only: false,
            transient_lights: Vec::new(),
            cloud_foreground: Vec::new(),
            dynamic_light_brightness: 1.0,
            emissive_area_lights_enabled: false,
            irradiance_volume_enabled: false,
            voxel_probe_gi_enabled: false,
            local_light_shadows_enabled: false,
            settings_batch_open: false,
            variant_activation_pending: false,
            world_variant_pending: None,
            pbr_enabled: true,
            pom_enabled: true,
            deluxe_mapping_enabled: true,
            deluxe_specular: 1.0,
            bc_compression_supported,
            cascaded_shadows_enabled: false,
            cascaded_shadow_mode: DynamicShadowsMode::Off,
            entity_shadow,
            entity_shadow_light: EntityShadowLight::Lightgrid,
            cull_debug_mode: CullDebugMode::Off,
            planar_reflection_mode: PlanarReflectionMode::Off,
            planar_reflection_debug_mode: PlanarReflectionDebugMode::Off,
            last_planar_debug_selection: None,
            planar_slot_history: [None; PLANAR_REFLECTION_SLOTS],
            planar_debug_pipeline,
            history_valid: false,
            camera_history_valid: false,
            previous_view_proj: Mat4::IDENTITY,
            previous_unjittered_view_proj: Mat4::IDENTITY,
            previous_cloud_view_proj: Mat4::IDENTITY,
            previous_camera_position: Vec3::ZERO,
            taa_frame_index: 0,
            ssao_history_valid: false,
            ssao_history_read_index: 0,
            ssr_history_valid: false,
            ssr_history_read_index: 0,
            ssr_frame_index: 0,
            depth_prepass_pipeline,
            depth_prepass_mask_pipeline,
            hiz_prepass_pipeline,
            hiz_prepass_mask_pipeline,
            hiz_build_layout,
            hiz_build_pipeline,
            hiz_build_bind_group,
            hiz_reduce_layout,
            hiz_reduce_pipeline,
            hiz_reduce_bind_groups,
            gpu_cull_layout,
            gpu_cull_pipeline,
            gpu_cull_early_pipeline,
            gpu_cull_settings_buffer,
            cull_debug_layout,
            cull_debug_pipeline_layout,
            cull_debug_shader,
            cull_debug_pipeline,
            cluster_compute_layout,
            cluster_compute_pipeline,
            lighting_layout,
            lighting_layout_lean,
            shadow_receiver_layout,
            shadow_caster_layout,
            lighting_settings_buffer,
            pbr_settings_buffer,
            shadow_resources,
            auto_exposure_layout,
            auto_exposure_pipeline,
            auto_exposure_state_buffer,
            auto_exposure_settings_buffer,
            auto_exposure_bind_group,
            post_layout,
            post_shader,
            post_pipeline_layout,
            post_pipeline: None,
            taa_post_pipeline,
            cloud_march_pipeline,
            cloud_resolve_pipeline,
            cloud_resolve_layout,
            cloud_resolve_bind_group,
            post_sampler,
            post_buffer,
            post_bind_groups,
            history_read_index: 0,
            ssao_temporal_layout,
            ssao_temporal_pipeline,
            ssao_temporal_buffer,
            ssao_temporal_bind_groups,
            ssr_temporal_layout,
            ssr_temporal_pipeline,
            ssr_temporal_buffer,
            ssr_temporal_bind_groups,
            _ssr_visibility_layout: ssr_visibility_layout,
            ssr_visibility_pipeline,
            ssr_visibility_bind_group,
            bloom_layout,
            bloom_extract_pipeline,
            bloom_downsample_pipeline,
            bloom_bind_groups,
            dof_layout,
            dof_pipeline,
            dof_buffer,
            dof_bind_group,
            gamma_post_layout,
            gamma_post_pipeline,
            gamma_post_buffer,
            gamma_post_bind_group,
            fast_post_layout,
            fast_post_pipeline,
            fast_post_bind_group,
            smaa_target: None,
            gpu_profiler,
            cull_diagnostics_readback,
            surface_diag_pending: env_flag("JKA_SURFACE_DIAG"),
            frame_diag_pending: env_flag("JKA_SURFACE_DIAG"),
            screenshot_supported,
            screenshot_requested: None,
            screenshot_result: None,
            screenshot_readback_buffer: None,
            screenshot_worker_tx,
            screenshot_worker_rx,
            started: Instant::now(),
        })
    }
}
