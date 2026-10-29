//! The original atmosphere on JKA's device; BSP sky polygons authorize display.
use crate::{camera::Camera, scene::GpuVertex};
use ::blue_water_sky::{Sky, SkyScene, SkyTargets, SkyView, math};

/// Only structural changes require a build. Sun, wind, coverage and weather
/// intensity remain ordinary per-frame uniforms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct AtmosphereConfig {
    target: (u32, u32, u32, wgpu::TextureFormat),
    quality: u8,
    cloud_quality: u8,
    clouds: bool,
    weather: bool,
    aerial: bool,
}

impl crate::renderer::Renderer {
    /// Poll once before recording a frame. Requested settings stay untouched;
    /// absence of a compatible result means the authored sky keeps drawing.
    pub(in crate::renderer) fn ensure_native_atmosphere(&mut self, camera: &Camera) {
        use crate::pipeline_jobs::{hash_config, PipelineJobKey};
        let requested = self.sky_backend == 1 || self.blue_water_environment.world_lighting > 0.0
            || self.blue_water_environment.weather_backend == 1
            || (self.cloud_backend == 1 && self.clouds_enabled);
        if !requested {
            self.pipeline_jobs.cancel_slot("native-atmosphere", 0);
            self.blue_water_sky = None;
            return;
        }
        let settings = self.blue_water_environment.sanitize();
        let config = AtmosphereConfig {
            target: (self.config.width, self.config.height, self.msaa_samples, self.scene_format()),
            quality: settings.quality,
            cloud_quality: settings.cloud_quality,
            clouds: self.cloud_backend == 1 && self.clouds_enabled,
            weather: settings.weather_backend == 1,
            aerial: self.sky_backend == 1 && self.fast_sky == 0,
        };
        if self.blue_water_sky.as_ref().is_some_and(|sky| sky.config == config) {
            return;
        }
        let was_active = self.blue_water_sky.take().is_some();
        let key = PipelineJobKey::new("native-atmosphere", 0, hash_config(&config));
        if let Some(atmosphere) = self.pipeline_jobs.take_ready::<AtmosphereRenderer>(key) {
            self.blue_water_sky = Some(atmosphere);
            self.history_valid = false;
            self.ssr_history_valid = false;
            self.ssao_history_valid = false;
            self.rebuild_frame_plan();
            rverbose!(1, "[ENVIRONMENT READY] atmosphere activated at frame boundary {config:?}");
            return;
        }
        if was_active {
            self.history_valid = false;
            self.rebuild_frame_plan();
        }
        if self.pipeline_jobs.is_compiling(key) {
            return;
        }
        let sun = self.active_sun();
        let sea_plane = self.world.as_ref().and_then(|world| world.ocean_surfaces.iter().max_by(|a, b| {
            let area = |s: &crate::ocean::OceanSurface|
                (s.maximum[0] - s.minimum[0]) * (s.maximum[1] - s.minimum[1]);
            area(a).total_cmp(&area(b))
        })).map_or(0.0, |surface| surface.plane_height);
        let render_scale = self.cloud_render_resolution.scale();
        let (device, queue, camera, coverage, wind) = (
            self.device.clone(), self.queue.clone(), *camera, self.cloud_coverage,
            [self.weather_wind.speed, self.weather_wind.direction],
        );
        self.pipeline_jobs.request(key, "native atmosphere", move || {
            let started = std::time::Instant::now();
            let mut atmosphere = AtmosphereRenderer::build(&device, &queue, config, settings);
            // Fill immutable noise and warm size-dependent targets/tables on
            // the worker too. Queue ordering makes these writes precede every
            // subsequent frame using the published resource; no render-thread
            // device wait or empty placeholder draw is necessary.
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("native atmosphere preparation"),
            });
            atmosphere.prepare(&device, &queue, &mut encoder, config.target, &camera, [0.0; 2],
                0.0, sun.direction.map(|value| -value), sea_plane, config.aerial, config.clouds, coverage,
                wind, settings, render_scale, sun.intensity, 0.0, false);
            atmosphere.render_image(&device, &mut encoder, None);
            queue.submit([encoder.finish()]);
            // The view may have moved while compilation ran. First use must
            // start fresh temporal histories using the current frame's camera.
            atmosphere.last_view = None;
            atmosphere.last_time = 0.0;
            rverbose!(1, "[ENVIRONMENT PREPARED] atmosphere worker {:.1} ms {config:?}",
                started.elapsed().as_secs_f64() * 1000.0);
            atmosphere
        });
    }
}

pub(in crate::renderer) struct AtmosphereRenderer {
    config: AtmosphereConfig,
    pub sky: Sky,
    pub clouds: ::blue_water_clouds::CloudLayer,
    weather: ::blue_water_clouds::weather::Weather,
    weather_reactive: Option<::blue_water_clouds::reactive::WeatherReactive>,
    weather_post: ::blue_water_clouds::post::PostProcess,
    exposure: super::blue_water_exposure::NativeExposure,
    scene: super::blue_water_scene::Scene,
    settings: crate::blue_water_environment::Settings,
    last_time: f32,
    frozen_time: Option<f32>,
    pub aerial: ::blue_water_render::AerialVolume,
    air_composite: super::blue_water_air::AirComposite,
    aerial_pass: ::blue_water_clouds::aerial::AerialPerspective,
    frame_camera: Option<::blue_water_render::CameraUniforms>,
    map_height: f32,
    last_view: Option<(math::Vec3, math::Vec3, f32)>,
    last_scene: Option<SkyScene>,
    probe: Option<wgpu::TextureView>,
    sea_heights: Option<wgpu::TextureView>,
    preview_strike: Option<math::Vec3>,
    show_atmosphere: bool,
    clouds_active: bool,
    key: (u32, u32, u32, wgpu::TextureFormat),
    image: wgpu::TextureView,
    camera: wgpu::Buffer,
    portal_camera: wgpu::Buffer,
    portal_group: wgpu::BindGroup,
    group: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
}
impl AtmosphereRenderer {
    fn build(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        config: AtmosphereConfig,
        settings: crate::blue_water_environment::Settings,
    ) -> Self {
        let key = config.target;
        let quality = settings.quality();
        let cache = crate::pipeline_cache::get();
        let placeholders = ::blue_water_render::PlaceholderPipelines::new(device, cache.as_ref());
        let mut sky = Sky::new_deferred(
            device,
            queue,
            quality,
            SkyTargets {
                color: wgpu::TextureFormat::Rgba16Float,
                depth: None,
            },
            &placeholders,
        );
        // The scene/aerial bindings share the cloud shadow interface even in a
        // clear sky. Deferred construction leaves its expensive pipelines and
        // noise fills unbuilt until clouds are requested.
        let mut clouds = ::blue_water_clouds::CloudLayer::new_deferred(
            device,
            queue,
            &sky,
            settings.cloud_quality(),
            ::blue_water_clouds::Targets {
                color: wgpu::TextureFormat::Rgba16Float,
                depth: None,
            },
            &placeholders,
        );
        clouds.set_enabled(config.clouds);
        if config.clouds {
            sky.attach_clouds(device, clouds.hooks());
        }
        let aerial = ::blue_water_render::AerialVolume::new(device);
        let air_composite = super::blue_water_air::AirComposite::new(device, key, &aerial);
        let mut aerial_pass = ::blue_water_clouds::aerial::AerialPerspective::new(
            device,
            &sky,
            clouds.shadow(),
            ::blue_water_render::MakePipelines::Deferred(&placeholders),
        );
        let mut weather = ::blue_water_clouds::weather::Weather::new_deferred(
            device,
            sky.bind_group_layout(),
            quality,
            ::blue_water_clouds::Targets {
                color: wgpu::TextureFormat::Rgba16Float,
                depth: Some(wgpu::TextureFormat::Depth32Float),
            },
            &placeholders,
        );
        let mut weather_post = ::blue_water_clouds::post::PostProcess::new_deferred(
            device,
            sky.bind_group_layout(),
            wgpu::TextureFormat::Rgba16Float,
            &placeholders,
        );
        let mut registry = ::blue_water_render::PipelineRegistry::default();
        let mut context = ::blue_water_render::RenderContext::new(device, &mut registry);
        context.cache = cache.as_ref();
        ::blue_water_sky::shaders::register(&mut context).expect("native sky shaders");
        ::blue_water_clouds::shaders::register(&mut context).expect("native cloud shaders");
        let sky_ids = sky.register_pipelines(&mut context).expect("native sky pipelines");
        let cloud_ids = config.clouds.then(|| clouds.register_pipelines(&mut context, &sky).expect("native cloud pipelines"));
        let weather_ids = config.weather.then(|| weather.register_pipelines(&mut context).expect("native weather pipelines"));
        let post_ids = config.weather.then(|| weather_post.register_pipelines(&mut context).expect("native weather post"));
        let aerial_ids = config.aerial.then(|| aerial_pass.register_pipelines(&mut context).expect("native aerial pipelines"));
        drop(context);
        sky.adopt_pipelines(&registry, &sky_ids);
        if let Some(ids) = cloud_ids { clouds.adopt_pipelines(&registry, &ids); }
        if let Some(ids) = weather_ids { weather.adopt_pipelines(&registry, &ids); }
        if let Some(ids) = post_ids { weather_post.adopt_pipelines(&registry, &ids); }
        if let Some(ids) = aerial_ids { aerial_pass.adopt_pipelines(&registry, &ids); }
        let weather_reactive = config.weather.then(||
            ::blue_water_clouds::reactive::WeatherReactive::new(device, weather.draw_layouts()));
        let scene = super::blue_water_scene::Scene::new(device, key, &sky, &clouds);
        let (image, camera, group, pipeline) = Self::targets(device, key);
        let (portal_camera, portal_group) = Self::portal_binding(device, &image, &pipeline);
        Self {
            config,
            sky,
            clouds,
            weather,
            weather_reactive,
            weather_post,
            exposure: super::blue_water_exposure::NativeExposure::new(device),
            scene,
            settings,
            last_time: 0.0,
            frozen_time: None,
            aerial,
            air_composite,
            aerial_pass,
            frame_camera: None,
            map_height: 0.0,
            last_view: None,
            last_scene: None,
            probe: None,
            sea_heights: None,
            preview_strike: None,
            show_atmosphere: true,
            clouds_active: false,
            key,
            image,
            camera,
            portal_camera,
            portal_group,
            group,
            pipeline,
        }
    }
    fn portal_binding(
        device: &wgpu::Device,
        image: &wgpu::TextureView,
        pipeline: &wgpu::RenderPipeline,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let camera = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Blue Water sky portal camera"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Blue Water sky portal display"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(image),
                },
            ],
        });
        (camera, group)
    }
    pub fn write_portal_camera(&self, queue: &wgpu::Queue, matrix: glam::Mat4) {
        queue.write_buffer(
            &self.portal_camera,
            0,
            bytemuck::bytes_of(&matrix.to_cols_array_2d()),
        );
    }
    pub fn bind_portal<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.portal_group, &[]);
    }
    fn targets(
        device: &wgpu::Device,
        key: (u32, u32, u32, wgpu::TextureFormat),
    ) -> (
        wgpu::TextureView,
        wgpu::Buffer,
        wgpu::BindGroup,
        wgpu::RenderPipeline,
    ) {
        let image = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("Blue Water atmosphere screen"),
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
        let camera = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Blue Water BSP sky camera"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Blue Water BSP sky display"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Blue Water BSP sky display"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&image),
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Blue Water BSP sky display"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Blue Water BSP sky display"),
            source: wgpu::ShaderSource::Wgsl(include_str!("blue_water_sky.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Blue Water BSP sky display"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("sky_mask"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GpuVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x3,
                        offset: 0,
                        shader_location: 0,
                    }],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("sky_color"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: key.3,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: key.2,
                ..Default::default()
            },
            multiview_mask: None,
            cache: crate::pipeline_cache::get().as_ref(),
        });
        (image, camera, group, pipeline)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        key: (u32, u32, u32, wgpu::TextureFormat),
        camera: &Camera,
        jitter: [f32; 2],
        time: f32,
        toward_sun: [f32; 3],
        sea_plane: f32,
        show_atmosphere: bool,
        clouds_enabled: bool,
        coverage: f32,
        wind: [f32; 2],
        settings: crate::blue_water_environment::Settings,
        cloud_render_scale: f32,
        sun_intensity: f32,
        frame_dt: f32,
        sheltered: bool,
    ) {
        // Resize/quality/features use the same worker path as first use. A
        // half-rebuilt atmosphere must never reach frame recording.
        debug_assert_eq!(self.key, key);
        let sun = math::Vec3::from_array(toward_sun).normalize_or_zero();
        let elevation = f64::from(sun.y.clamp(-1.0, 1.0).asin().to_degrees());
        let azimuth = f64::from(sun.x.atan2(sun.z).to_degrees());
        self.show_atmosphere = show_atmosphere;
        self.clouds_active = clouds_enabled;
        let settings = settings.sanitize();
        let rebase_changed = self.settings.cloud_altitude_rebase != settings.cloud_altitude_rebase;
        let (time, frame_dt) = if settings.cloud_freeze {
            (*self.frozen_time.get_or_insert(if self.last_time == 0.0 { time } else { self.last_time }), 0.0)
        } else {
            self.frozen_time = None;
            (time, frame_dt)
        };
        debug_assert_eq!(self.config.quality, settings.quality);
        debug_assert_eq!(self.config.cloud_quality, settings.cloud_quality);
        self.settings = settings;
        self.clouds.set_render_scale(cloud_render_scale);
        self.clouds.set_tuning(::blue_water_clouds::layer::CloudTuning {
            debug: settings.cloud_debug,
            shadow_mode: settings.cloud_shadow_mode,
            shadow_far: settings.cloud_shadow_far,
            far_step_scale: settings.cloud_far_step_scale,
            sun_steps: settings.cloud_sun_steps,
        });
        let scene = settings.scene(
            elevation,
            azimuth,
            sun_intensity,
            coverage,
            wind,
            clouds_enabled,
        );
        if self.last_scene != Some(scene) || rebase_changed {
            self.sky.apply(&scene);
            self.clouds.apply(queue, &mut self.sky, &scene);
            if settings.cloud_altitude_rebase {
                // A/B only: rebase *resolved* layers, including automatic high cloud, rain shafts and fog.
                // Keep sky haze / weather particles unchanged to isolate the cloud-placement hypothesis.
                self.clouds.set_layers(rebased_cloud_layers(&scene, self.clouds.tier().rain_shafts,
                    (-settings.altitude).max(0.0)));
            }
            self.weather.apply(&scene);
            self.last_scene = Some(scene);
        }
        if self.clouds.enabled() != clouds_enabled {
            self.clouds.set_enabled(clouds_enabled);
        }
        self.clouds.set_time_stands(frame_dt <= 0.0);
        // A negative cloud base lowers the whole sky reference (its ground plane
        // and the cloud deck with it) below the sea plane by that many metres,
        // so clouds can stand at the level of a map that lies under the sea plane.
        let reference_drop = (-settings.altitude).max(0.0);
        self.map_height = sea_plane - reference_drop * crate::ocean::OCEAN_UNITS_PER_METER;
        let mut settings = settings;
        settings.altitude = settings.altitude.max(0.0);
        self.clouds.set_still(frame_dt <= 0.0);
        if self.sky.set_weather_haze(::blue_water_sky::haze::Haze::new(
            settings.visibility as f64,
            settings.altitude as f64,
        )) {
            self.clouds.air_changed();
        }
        self.sky.set_night_sky_brightness(settings.night_sky);
        self.sky.seek(f64::from(time));
        let position = (camera.position - glam::Vec3::Y * sea_plane)
            / crate::ocean::OCEAN_UNITS_PER_METER
            + glam::Vec3::Y * reference_drop;
        let view = glam::camera::rh::view::look_to_mat4(position, camera.forward(), glam::Vec3::Y);
        let fov = camera.fov_y_for_viewport(key.0, key.1);
        let projection = glam::Mat4::from_translation(glam::Vec3::new(
            2.0 * jitter[0] / key.0.max(1) as f32,
            -2.0 * jitter[1] / key.1.max(1) as f32,
            0.0,
        )) * glam::camera::rh::proj::directx::perspective(
            fov,
            key.0 as f32 / key.1.max(1) as f32,
            camera.far_distance() / crate::ocean::OCEAN_UNITS_PER_METER,
            camera.near_distance() / crate::ocean::OCEAN_UNITS_PER_METER,
        );
        let sky_view = SkyView {
            position: math::Vec3::from_array(position.to_array()),
            camera_to_world: math::Mat4::from_cols_array_2d(&view.inverse().to_cols_array_2d()),
            projection: math::Mat4::from_cols_array_2d(&projection.to_cols_array_2d()),
            viewport: [key.0, key.1],
        };

        // Cloud temporal reconstruction owns its subpixel pattern, so its view is unjittered.
        let cloud_projection = math::Mat4::from_cols_array_2d(
            &glam::camera::rh::proj::directx::perspective(
                fov,
                key.0 as f32 / key.1.max(1) as f32,
                camera.far_distance() / crate::ocean::OCEAN_UNITS_PER_METER,
                camera.near_distance() / crate::ocean::OCEAN_UNITS_PER_METER,
            )
            .to_cols_array_2d(),
        );
        let cloud_view = ::blue_water_clouds::CloudView {
            position: sky_view.position,
            view: sky_view.camera_to_world.inverse(),
            projection: cloud_projection,
            near: camera.near_distance() / crate::ocean::OCEAN_UNITS_PER_METER,
            far: camera.far_distance() / crate::ocean::OCEAN_UNITS_PER_METER,
            viewport: sky_view.viewport,
        };
        let forward = math::Vec3::from_array(camera.forward().to_array());
        let cut = self.last_view.is_none_or(|(position, direction, focal)| {
            position.distance(cloud_view.position) > 30.0
                || direction.dot(forward) < 0.94
                || (fov / focal - 1.0).abs() > 0.2
        });
        self.last_view = Some((cloud_view.position, forward, fov));
        let dt = if self.last_time == 0.0 {
            0.0
        } else {
            frame_dt.max(0.0).min(0.25) as f64
        };
        self.last_time = time;
        self.weather.set_fall_speed(settings.rain_speed);
        if settings.strike_preview && settings.weather_backend == 1 {
            let target = *self.preview_strike.get_or_insert_with(|| {
                let forward = math::Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
                cloud_view.position + forward * 2600.0
            });
            self.weather.force_strike(queue, target, 0.006,
                ::blue_water_clouds::lightning::StrikeKind::Ground, 7);
        } else if self.preview_strike.take().is_some() {
            self.weather.clear_strike();
        }
        // Weather geometry uses the jittered host projection, matching scene depth.
        let weather_view = ::blue_water_clouds::CloudView {
            projection: sky_view.projection,
            ..cloud_view
        };
        self.weather.update(
            &mut ::blue_water_render::UploadSink::Queue(queue),
            &weather_view,
            dt,
            cut,
            sheltered,
        );
        let flash = self.weather.light();
        self.sky
            .set_lightning(flash.position, flash.intensity, flash.flash, flash.bolt);
        self.sky.update_tables(queue, encoder, &sky_view, 0.0);
        self.weather_post
            .update(&cloud_view, self.weather.haze(), flash);
        if clouds_enabled {
            self.clouds.encode_noise_fill(encoder);
            self.clouds.prepare(
                device,
                &mut ::blue_water_render::UploadSink::Queue(queue),
                &cloud_view,
                &self.sky,
                cut,
            );
            if let Some(group) = self.clouds.take_field_group() {
                self.sky.set_cloud_bind_group(group);
            }
        } else {
            self.clouds.write_shadow_interface(
                &mut ::blue_water_render::UploadSink::Queue(queue), &self.sky, &cloud_view);
        }
        self.sky.update_environment(queue, encoder);
        if clouds_enabled { self.clouds.encode_shadows(encoder); }
        let vp = sky_view.projection * cloud_view.view;
        let camera_uniforms = ::blue_water_render::CameraUniforms {
            view: cloud_view.view.to_cols_array(),
            projection: sky_view.projection.to_cols_array(),
            view_projection: vp.to_cols_array(),
            inverse_view: sky_view.camera_to_world.to_cols_array(),
            inverse_projection: sky_view.projection.inverse().to_cols_array(),
            inverse_view_projection: vp.inverse().to_cols_array(),
            position: sky_view.position.to_array(),
            time,
            viewport: [
                key.0 as f32,
                key.1 as f32,
                1.0 / key.0.max(1) as f32,
                1.0 / key.1.max(1) as f32,
            ],
            near: cloud_view.near,
            far: cloud_view.far,
            tan_half_fov_y: (fov * 0.5).tan(),
            aspect: key.0 as f32 / key.1.max(1) as f32,
            dt: frame_dt,
            frame: 0,
            curvature: 0.0,
            padding: 0,
        };
        self.aerial_pass.enabled = show_atmosphere;
        self.aerial_pass.encode(
            device,
            &mut ::blue_water_render::UploadSink::Queue(queue),
            encoder,
            &camera_uniforms,
            &self.aerial,
            &self.sky,
            self.clouds.shadow(),
            [
                self.clouds.uniforms().heights[0],
                self.clouds.uniforms().heights[1],
            ],
            None,
        );
        self.frame_camera = Some(camera_uniforms);
        queue.write_buffer(
            &self.camera,
            0,
            bytemuck::bytes_of(
                &camera
                    .view_projection_jittered(key.0, key.1, jitter)
                    .to_cols_array_2d(),
            ),
        );
    }
    /// The probe is encoded after the ocean's current camera/boundary writes and before this march.
    pub fn render_image(
        &mut self,
        _device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        probe: Option<&wgpu::TextureView>,
    ) {
        let next_probe = probe.cloned();
        if self.probe != next_probe {
            self.clouds.set_underwater_probe(next_probe.clone());
            self.probe = next_probe;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Blue Water original atmosphere dome"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.image,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        crate::renderer::tile_probe_scissor_surface(&mut pass);
        if self.show_atmosphere {
            self.sky.draw_dome(&mut pass);
        }
    }
    pub fn apply_scene(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        resolve: Option<&wgpu::TextureView>,
        depth: &wgpu::TextureView,
        shadows: bool,
        // Sky-portal depth and 1 / portal scale (r_cloudPortalDepth): merged into
        // the copy the cloud march reads, after lighting so portal terrain is not
        // lit as if it were world geometry.
        portal: Option<(&wgpu::TextureView, f32)>,
    ) {
        self.scene.copy.capture(device, encoder, color, depth);
        if let Some(camera) = &self.frame_camera {
            let flash = if self.settings.weather_backend == 1 {
                self.weather.light().flash
            } else {
                0.0
            };
            if self.settings.world_lighting > 0.0 || (self.clouds_active && shadows) || flash > 0.0 {
                self.scene.light(
                    device,
                    queue,
                    encoder,
                    camera,
                    &self.sky,
                    &self.clouds,
                    color,
                    resolve,
                    self.settings.world_lighting,
                    shadows && self.clouds_active,
                    flash,
                );
            }
        }
        if self.clouds_active {
            if let (Some(camera), Some((portal_depth, inverse_scale))) = (&self.frame_camera, portal) {
                self.scene
                    .merge_portal_depth(device, queue, encoder, camera, portal_depth, inverse_scale);
            }
            self.clouds.encode_march(
                device,
                encoder,
                &self.sky,
                Some((&self.scene.copy.depth, self.scene.depth_key)),
            );
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("native clouds against opaque depth"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.scene.overlay,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                crate::renderer::tile_probe_scissor_surface(&mut pass);
                self.clouds.draw_composite(&mut pass);
            }
            if self.settings.cloud_composite {
                self.scene
                    .blit(device, encoder, &self.scene.overlay, color, resolve, true);
            }
        }
    }
    /// Lightning's current flash (a multiple of the sky's light), 0 without native weather.
    pub fn flash(&self) -> f32 {
        if self.settings.weather_backend == 1 { self.weather.light().flash } else { 0.0 }
    }
    pub fn take_thunder(&mut self) -> Vec<::blue_water_clouds::lightning::Thunder> {
        std::mem::take(&mut self.weather.thunder)
    }
    pub fn adapt_exposure(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView, state: &wgpu::Buffer, settings: &wgpu::Buffer) {
        self.exposure.encode(device, encoder, self.sky.environment(), scene, state, settings);
    }
    pub fn draw_weather(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        resolve: Option<&wgpu::TextureView>,
        depth: &wgpu::TextureView,
        motion: Option<&wgpu::TextureView>,
    ) {
        if self.settings.weather_backend != 1 {
            return;
        }
        self.scene.copy.capture(device, encoder, color, depth);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("original Blue Water rain splashes and lightning"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.scene.copy.color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.scene.copy.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            crate::renderer::tile_probe_scissor_surface(&mut pass);
            self.weather.draw(&mut pass, self.sky.bind_group());
        }
        if let Some(motion) = motion {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("native weather temporal rejection"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {view:motion,depth_slice:None,resolve_target:None,
                    ops:wgpu::Operations {load:wgpu::LoadOp::Load,store:wgpu::StoreOp::Store}})],
                depth_stencil_attachment:Some(wgpu::RenderPassDepthStencilAttachment {view:&self.scene.copy.depth,
                    depth_ops:Some(wgpu::Operations {load:wgpu::LoadOp::Load,store:wgpu::StoreOp::Store}),stencil_ops:None}),
                ..Default::default()
            });
            crate::renderer::tile_probe_scissor_surface(&mut pass);
            self.weather.draw_reactive(&mut pass, self.sky.bind_group(),
                &self.weather_reactive.as_ref().expect("prepared native weather").0);
        }
        if self.weather_post.encode(
            device,
            queue,
            encoder,
            self.sky.bind_group(),
            &self.scene.copy.color,
            &self.scene.copy.depth,
            &self.scene.overlay,
        ) {
            self.scene
                .blit(device, encoder, &self.scene.overlay, color, resolve, false);
        } else {
            self.scene.blit(
                device,
                encoder,
                &self.scene.copy.color,
                color,
                resolve,
                false,
            );
        }
    }
    pub fn weather_eye(&self) -> Option<[f32; 3]> {
        self.frame_camera.as_ref().map(|camera| camera.position)
    }
    pub fn set_sea_heights(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, heights: Option<wgpu::TextureView>) {
        if self.sea_heights != heights {
            self.weather.set_sea_heights(device, heights.as_ref().map(|view|
                (view, blue_water_gpu::rain_heights::SPAN)));
            self.weather.upload_splash_surface(queue);
            self.sea_heights = heights;
        }
    }
    pub fn apply_opaque_air(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        resolve: Option<&wgpu::TextureView>,
        depth: &wgpu::TextureView,
        roof: &(wgpu::TextureView, [f32; 4]),
    ) {
        if self.show_atmosphere {
            if let Some(camera) = &self.frame_camera {
                self.air_composite
                    .draw(device, queue, encoder, camera, color, resolve, depth, &self.aerial, roof, self.map_height);
            }
        }
    }
    pub fn bind<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.group, &[]);
    }
}

/// Translate final vendor layers, rather than rebasing authored inputs and leaving implicit layers behind.
fn rebased_cloud_layers(scene: &SkyScene, rain_shafts: bool, reference_drop: f32) -> ::blue_water_clouds::layers::Layers {
    let mut layers = ::blue_water_clouds::weathers::layers(scene, rain_shafts);
    for layer in &mut layers.0 {
        layer.altitude += reference_drop;
    }
    layers
}

#[cfg(test)]
mod native_tests {
    use super::*;
    #[test]
    fn cloud_altitude_experiment_preserves_world_heights_for_all_generated_layers() {
        for altitude in [200.0f32, 0.0, -200.0, -1000.0] {
            for high_genus in [0, 1, 2, 3] {
                for fog in [0.0, 1.0] {
                    let settings = crate::blue_water_environment::Settings {
                        altitude, high_genus, weather_backend: 1, rain: 1.0, fog,
                        ..Default::default()
                    };
                    let scene = settings.scene(40.0, 0.0, 255.0, 0.5, [0.0; 2], true);
                    let drop = (-altitude).max(0.0);
                    let authored = ::blue_water_clouds::weathers::layers(&scene, true);
                    let native = rebased_cloud_layers(&scene, true, drop);
                    for (original, converted) in authored.0.iter().zip(native.0.iter()) {
                        assert!((converted.altitude - drop - original.altitude).abs() < 0.001);
                        assert_eq!(converted.height, original.height);
                        assert_eq!(converted.coverage, original.coverage);
                    }
                    if high_genus != 0 {
                        assert_eq!(native.0[2].altitude - drop, settings.high_altitude);
                    }
                    if fog == 0.0 {
                        assert!((native.0[0].altitude - drop - altitude).abs() < 0.001);
                        assert!((native.0[3].altitude + native.0[3].height - native.0[0].altitude).abs() < 0.001);
                    }
                }
            }
        }
    }

    #[test]
    fn native_atmosphere_depth_msaa_weather_and_history() {
        futures_lite::future::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance
                .request_adapter(&Default::default())
                .await
                .expect("GPU adapter");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    required_limits: adapter.limits(),
                    ..Default::default()
                })
                .await
                .expect("GPU device");
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            for samples in [1, 4] {
              for (clouds, weather, aerial) in [(false, false, true), (true, false, false), (true, true, true)] {
                // Odd dimensions catch aspect and rounded-up cloud/depth coordinates.
                let key = (65, 33, samples, wgpu::TextureFormat::Rgba16Float);
                let texture = |format, samples| {
                    device
                        .create_texture(&wgpu::TextureDescriptor {
                            label: Some("native scene fixture"),
                            size: wgpu::Extent3d {
                                width: key.0,
                                height: key.1,
                                depth_or_array_layers: 1,
                            },
                            mip_level_count: 1,
                            sample_count: samples,
                            dimension: wgpu::TextureDimension::D2,
                            format,
                            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                                | wgpu::TextureUsages::TEXTURE_BINDING,
                            view_formats: &[],
                        })
                        .create_view(&Default::default())
                };
                let color = texture(key.3, samples);
                let depth = texture(wgpu::TextureFormat::Depth32Float, samples);
                let motion = texture(wgpu::TextureFormat::Rg16Float, 1);
                let roof = (device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("native air no-roof fixture"),
                    size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                    mip_level_count: 1, sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba32Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
                }).create_view(&Default::default()), [0.0; 4]);
                let resolved = texture(key.3, 1);
                let resolve = (samples > 1).then_some(&resolved);
                let camera = Camera::new([0., 800., 0.], 0.);
                let settings = crate::blue_water_environment::Settings {
                    quality: 0,
                    cloud_quality: 0,
                    weather_backend: u8::from(weather),
                    rain: 1.,
                    rain_speed: 2.,
                    world_lighting: 1.0,
                    ..Default::default()
                };
                let config = AtmosphereConfig { target: key, quality: settings.quality, cloud_quality: settings.cloud_quality,
                    clouds, weather, aerial };
                let mut atmosphere = AtmosphereRenderer::build(&device, &queue, config, settings);
                assert_eq!(atmosphere.weather_reactive.is_some(), weather);
                for frame in 0..8 {
                    let scale = [1.0, 0.75, 0.5, 0.25][frame / 2];
                    let mut encoder = device.create_command_encoder(&Default::default());
                    if frame == 1 && weather {
                        // Exercise the shaped bolt draw beyond JKA's map far
                        // plane, in addition to the ordinary cloud flash path.
                        let target = camera.position / crate::ocean::OCEAN_UNITS_PER_METER
                            + camera.forward() * 2600.0;
                        atmosphere.weather.force_strike(
                            &queue,
                            math::Vec3::from_array(target.to_array()),
                            0.006,
                            ::blue_water_clouds::lightning::StrikeKind::Ground,
                            7,
                        );
                    }
                    atmosphere.prepare(
                        &device,
                        &queue,
                        &mut encoder,
                        key,
                        &camera,
                        [0.; 2],
                        1. + frame as f32 / 60.,
                        [0.3, 0.8, 0.2],
                        0.,
                        aerial,
                        clouds,
                        0.5,
                        [100., 45.],
                        settings,
                        scale,
                        255.,
                        1. / 60.,
                        false,
                    );
                    if clouds {
                        let expected = [key.0, key.1].map(|v| (v as f32 * scale).ceil());
                        assert_eq!(&atmosphere.clouds.uniforms().screen[..2], &expected);
                        assert_eq!(&atmosphere.clouds.uniforms().viewport[..2], &[key.0 as f32, key.1 as f32]);
                        assert_eq!(atmosphere.clouds.uniforms().frame[3], (frame % 2) as f32,
                            "resizing drops cloud history once; unchanged sizes keep it");
                    }
                    if frame % 2 == 1 && clouds {
                        assert_eq!(
                            atmosphere.clouds.uniforms().frame[3],
                            1.,
                            "cloud history survives unchanged enable state"
                        );
                        assert!(
                            !atmosphere.clouds.marched_whole(),
                            "live frames use the quarter-resolution reconstruction"
                        );
                    }
                    atmosphere.render_image(&device, &mut encoder, None);
                    {
                        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &color,
                                depth_slice: None,
                                resolve_target: resolve,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color {
                                        r: 0.3,
                                        g: 0.4,
                                        b: 0.5,
                                        a: 1.,
                                    }),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: Some(
                                wgpu::RenderPassDepthStencilAttachment {
                                    view: &depth,
                                    depth_ops: Some(wgpu::Operations {
                                        load: wgpu::LoadOp::Clear(0.25),
                                        store: wgpu::StoreOp::Store,
                                    }),
                                    stencil_ops: None,
                                },
                            ),
                            ..Default::default()
                        });
                    }
                    atmosphere.apply_scene(
                        &device,
                        &queue,
                        &mut encoder,
                        &color,
                        resolve,
                        &depth,
                        true,
                        None,
                    );
                    atmosphere.apply_opaque_air(
                        &device,
                        &queue,
                        &mut encoder,
                        &color,
                        resolve,
                        &depth,
                        &roof,
                    );
                    atmosphere.draw_weather(&device, &queue, &mut encoder, &color, resolve, &depth, Some(&motion));
                    queue.submit(Some(encoder.finish()));
                    device
                        .poll(wgpu::PollType::Wait {
                            submission_index: None,
                            timeout: None,
                        })
                        .unwrap();
                }
                if clouds {
                    for debug in 0..=5 {
                        let settings = crate::blue_water_environment::Settings {
                            cloud_debug: debug, cloud_freeze: true, cloud_shadow_mode: debug % 2,
                            cloud_shadow_far: 60_000.0, cloud_far_step_scale: 2.0, cloud_sun_steps: 0,
                            cloud_composite: debug != 1, cloud_altitude_rebase: true, altitude: -200.0,
                            ..settings
                        };
                        let mut encoder = device.create_command_encoder(&Default::default());
                        atmosphere.prepare(&device, &queue, &mut encoder, key, &camera, [0.0; 2],
                            100.0 + f32::from(debug), [0.3, 0.8, 0.2], 0.0, aerial, true, 0.5,
                            [100.0, 45.0], settings, 0.5, 255.0, 1.0 / 60.0, false);
                        let uniforms = atmosphere.clouds.uniforms();
                        assert_eq!(uniforms.experiments, [f32::from(debug), f32::from(debug % 2), 2.0, 1.0]);
                        assert_eq!(uniforms.planes[1], camera.far_distance() / crate::ocean::OCEAN_UNITS_PER_METER);
                        assert_eq!(uniforms.planes[2], 60_000.0);
                        assert_eq!(uniforms.frame[3], 0.0, "each new diagnostic drops its history");
                        assert_eq!(atmosphere.clouds.layers().0[0].altitude, 0.0);
                        assert_eq!(Some(atmosphere.last_time), atmosphere.frozen_time);
                        atmosphere.render_image(&device, &mut encoder, None);
                        atmosphere.apply_scene(&device, &queue, &mut encoder, &color, resolve, &depth, true, None);
                        queue.submit(Some(encoder.finish()));
                        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
                    }
                }
              }
            }
            let error = scope.pop().await;
            assert!(error.is_none(), "native environment GPU validation: {error:?}");
        });
    }
}
