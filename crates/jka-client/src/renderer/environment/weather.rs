//! Environment weather.
use crate::renderer::{
    create_rt_receiver_bind_group, create_shadow_receiver_bind_group,
    create_world_froxel_bind_group, scene, weather, Camera, DynamicModelSurface,
    DynamicShadowsMode, DynamicWireframeClass, EntityWeather, Renderer, Vec3, NO_WATER_SURFACE,
};

impl Renderer {
    pub(in crate::renderer) fn rebuild_weather_surface_receiver_bind_group(&mut self) {
        self.shadow_resources.receiver_bind_group = create_shadow_receiver_bind_group(
            &self.device,
            &self.shadow_receiver_layout,
            &self.shadow_resources._array_view,
            if matches!(self.cascaded_shadow_mode, DynamicShadowsMode::CascadedShadowMaps) {
                &self.shadow_resources.bevy_sampler
            } else {
                &self.shadow_resources.legacy_sampler
            },
            &self.shadow_resources.sky_array_view,
            &self.shadow_resources.receiver_buffer,
            self.weather.fog.legacy_control_buffer(),
            &self.shadow_resources.weather_height_view,
            &self.shadow_resources.weather_sampler,
            &self.shadow_resources.weather_surface_buffer,
        );
        if let Some(rt) = self.ray_traced_shadows.as_mut() {
            rt.receiver_bind_group = create_rt_receiver_bind_group(
                &self.device,
                &self.shadow_resources,
                self.weather.fog.legacy_control_buffer(),
                self.cascaded_shadow_mode,
                &rt.receiver_layout,
                rt.scene(),
                &rt.receiver,
            );
        }
    }

    pub(in crate::renderer) fn advance_surface_wetness(
        &mut self,
        frame_time: f32,
        camera_position: Vec3,
    ) -> bool {
        self.weather
            .rain
            .advance_surface_wetness(frame_time, camera_position.to_array())
    }

    /// Wet weather on players, props and the view model (model_weather.wgsl).
    /// Unlike the world surfaces this is pushed every frame rather than only
    /// while the weather uniform is being advanced: the setter compares before it
    /// uploads, so a dry map pays one struct comparison, and a frame where the
    /// rain has just stopped still gets its last update instead of leaving models
    /// permanently wet.
    pub(in crate::renderer) fn update_entity_weather(&mut self) {
        let wetness = self.weather.rain.entity_wetness();
        let weather = if wetness <= weather::RAIN_WETNESS_EPSILON
            && self.dynamic_model_renderer.entity_water.is_empty() {
            EntityWeather::default()
        } else {
            let sun = self.active_sun();
            EntityWeather {
                sun_direction: [
                    sun.direction[0],
                    sun.direction[1],
                    sun.direction[2],
                    (sun.intensity / 250.0).clamp(0.0, 4.0),
                ],
                sun_radiance: [
                    sun.color[0].max(0.0),
                    sun.color[1].max(0.0),
                    sun.color[2].max(0.0),
                    wetness,
                ],
            }
        };
        self.dynamic_model_renderer
            .set_entity_weather(&self.queue, weather);
    }

    pub(in crate::renderer) fn update_weather_surface_uniform(&self) {
        let sun = self.active_sun();
        let wind = self
            .weather_wind
            .at(self.weather.rain.surface_wetness_last_time);
        self.weather.rain.write_surface_uniform(
            &self.queue,
            &self.shadow_resources.weather_surface_buffer,
            &weather::WeatherSurfaceEnvironment {
                sun_direction: sun.direction,
                sun_color: sun.color,
                sun_intensity: sun.intensity,
                wind,
                has_sky: self
                    .world
                    .as_ref()
                    .is_some_and(|world| world.primary_skybox.is_some()),
                wake: self.weather.rain.wake.events(),
            },
        );
    }

    /// Feeds everyone who may wade (the local player and any player models in the
    /// scene) to the puddle wake tracker. Runs only while puddles exist.
    pub(in crate::renderer) fn update_wake(
        &mut self,
        camera: &Camera,
        player_position: Option<Vec3>,
        dynamic_models: &[DynamicModelSurface],
    ) {
        if self.weather.rain.puddle_amount <= 10.0 * weather::RAIN_PUDDLE_EPSILON
            && self.weather.rain.wake.events().is_empty()
        {
            return;
        }
        let sole = |origin: Vec3| origin - Vec3::Y * weather::wake::FEET_BELOW_ORIGIN;
        let local = player_position.map(sole);
        let mut people: Vec<(u32, Vec3)> = Vec::new();
        if let Some(feet) = local {
            people.push((u32::MAX, feet));
        }
        for surface in dynamic_models {
            if surface.wireframe_class != DynamicWireframeClass::Player {
                continue;
            }
            let Some(origin) = surface.lighting_origin else {
                continue;
            };
            let id = u32::from(surface.entity_num);
            if people.iter().any(|(seen, _)| *seen == id) {
                continue;
            }
            let feet = sole(Vec3::from_array(scene::render_position(origin)));
            // The local player's own model is submitted in third person.
            if local.is_some_and(|local| local.distance_squared(feet) < 30.0 * 30.0) {
                continue;
            }
            people.push((id, feet));
        }
        self.weather.rain.update_wake(camera.position, people);
    }

    pub(in crate::renderer) fn ensure_weather_occlusion(&mut self) {
        let Some(weather_height_view) = self
            .weather
            .rain
            .ensure_occlusion(&self.device, &self.queue)
        else {
            return;
        };
        self.shadow_resources.weather_height_view = weather_height_view;
        {
            let weather = &mut self.weather;
            weather.snow.rebuild_collision_bind_group(
                &self.device,
                weather.rain.collision_view(),
                &weather.rain.gpu.wind_noise_view,
                &weather.rain.gpu.wind_noise_sampler,
            );
            weather.wind_visuals.rebuild_field_bind_group(
                &self.device,
                weather.rain.collision_view(),
                &weather.rain.gpu.wind_noise_view,
                &weather.rain.gpu.wind_noise_sampler,
            );
        }
        self.rebuild_weather_surface_receiver_bind_group();
        self.rebuild_post_bind_group();
        self.rebuild_rain_haze_mask_bind_group();
        if let Some(world) = self.world.as_mut() {
            world.froxel_bind_group = Some(create_world_froxel_bind_group(
                &self.device,
                &self.weather.fog.resources.layout,
                &self.weather.fog.resources.uniform_buffer,
                &self.weather.fog.resources.buffer,
                &self.shadow_resources._array_view,
                &self.shadow_resources.sky_array_view,
                &world.light_buffer,
                &world._cluster_buffer,
                &self.lighting_settings_buffer,
                &world.local_shadows.cube_view,
                &world.local_shadows.sampler,
                self.weather.rain.collision_view(),
                &self.weather.rain.gpu.wind_noise_view,
                &self.weather.rain.gpu.wind_noise_sampler,
            ));
        }
    }

    /// Hybrid decides "under water", the underwater cull and the Godot/Blue Water
    /// mesh from the rasterized wave over the eye, not the flat BSP plane.
    pub(in crate::renderer) fn hybrid_wave_surface_active(&self) -> bool {
        self.ocean_enabled
            && self.ocean_settings.backend == 2
            && self.ocean_settings.blue_water_underwater_mode == 4
            && self.ocean_settings.blue_water_adapter
            && self.ocean.as_ref().is_some_and(|gpu| gpu.backend() != 0)
    }

    /// Promoted pool whose footprint (and outline) lies over `eye`; where several
    /// stack, the one whose plane is closest to the eye.
    pub(in crate::renderer) fn promoted_pool_over(&self, eye: Vec3) -> Option<usize> {
        let world = self.world.as_ref()?;
        world
            .ocean_surfaces
            .iter()
            .enumerate()
            .filter(|(slot, s)| {
                eye.x >= s.minimum[0] && eye.x <= s.maximum[0]
                    && eye.z >= s.minimum[1] && eye.z <= s.maximum[1]
                    && (!world.ocean_masks.is_masked(*slot) || world.ocean_masks.contains(*slot, [eye.x, eye.z]))
            })
            .min_by(|a, b| (a.1.plane_height - eye.y).abs().total_cmp(&(b.1.plane_height - eye.y).abs()))
            .map(|(slot, _)| slot)
    }

    /// `hybridwave` command: settings summary plus the last ~600 frames of
    /// classification, written next to the screenshot the command also queues.
    pub(in crate::renderer) fn write_hybrid_wave_dump(&self, path: &std::path::Path) {
        use std::fmt::Write as _;
        let s = &self.ocean_settings;
        let mut header = String::new();
        let _ = writeln!(header, "DinurdoJK hybridwave dump");
        let _ = writeln!(header, "exe: {:?}", std::env::current_exe().ok());
        let _ = writeln!(header, "backend={} underwater_mode={} adapter={} waterline_debug={} hybrid_active={}",
            s.backend, s.blue_water_underwater_mode, s.blue_water_adapter, s.hybrid_waterline_debug,
            self.hybrid_wave_surface_active());
        let _ = writeln!(header, "waves: {:?}", s.blue_water_waves);
        let _ = writeln!(header, "underwater_cull_distance={} absorption_distance={} mesh_quality={}",
            s.underwater_cull_distance(), s.optics.absorption_distance(), s.mesh_quality);
        let _ = writeln!(header, "sky_extend={} sky_infinite={} extension_mask={:#b}",
            self.ocean_sky_extend, self.ocean_sky_infinite, self.ocean_extension_mask);
        let _ = writeln!(header, "now: eye_wave_slot={:?} offset={} correction={:?} camera_water_surface={}",
            self.eye_wave_slot, self.eye_wave_offset, self.eye_wave_correction, self.camera_water_surface);
        let _ = writeln!(header, "water_boxes ({}):", self.water_boxes.len());
        for (i, volume) in self.water_boxes.iter().enumerate() {
            let _ = writeln!(header, "  [{i}] {volume:?}");
        }
        if let Some(world) = &self.world {
            let _ = writeln!(header, "promoted surfaces ({}):", world.ocean_surfaces.len());
            for (i, surface) in world.ocean_surfaces.iter().enumerate() {
                let _ = writeln!(header, "  [{i}] {surface:?} masked={}", world.ocean_masks.is_masked(i));
            }
        }
        match self.eye_wave_log.write(path, &header) {
            Ok(()) => println!("[HYBRIDWAVE] wrote {}", path.display()),
            Err(error) => eprintln!("[HYBRIDWAVE] could not write {}: {error}", path.display()),
        }
    }

    /// Refreshes `eye_wave_slot` / `eye_wave_offset` from the latest probe sample.
    /// Zero (the flat plane) whenever Hybrid is not classifying by waves.
    pub(in crate::renderer) fn refresh_eye_wave_offset(&mut self, eye: Vec3) {
        let slot = self.hybrid_wave_surface_active().then(|| self.promoted_pool_over(eye)).flatten();
        self.eye_wave_slot = slot;
        let sample = slot.and_then(|slot| self.ocean.as_ref()?.eye_wave_sample(slot));
        self.eye_wave_offset = sample.map_or(0.0, |s| s.offset());
        self.eye_wave_correction = sample.map(|s| s.correction());
    }

    /// Collects this frame's water volumes (authored oceans first, then promoted
    /// BSP water not already covered by one) and finds the surface above the camera.
    /// Shared by the ocean optics, the rain (which must stop at the surface) and the
    /// cloud composite (which must be seen through it).
    pub(in crate::renderer) fn refresh_water_boxes(&mut self, eye: Vec3) {
        let mut boxes = std::mem::take(&mut self.water_boxes);
        boxes.clear();
        for a in &self.authored_ocean_definitions {
            if boxes.len() == 8 {
                break;
            }
            boxes.push(crate::ocean::WaterVolume {
                minimum: [a.mins[0], a.mins[2], -a.maxs[1]],
                maximum: [a.maxs[0], a.height, -a.mins[1]],
                mask_slot: None,
            });
        }
        if let Some(world) = &self.world {
            // `slot` is the index baked into the clipmap, which is what the
            // outline masks are keyed by (ocean_surfaces is pushed in slot order).
            for (slot, s) in world.ocean_surfaces.iter().enumerate() {
                if boxes.len() == 8 {
                    break;
                }
                let centre = [
                    (s.minimum[0] + s.maximum[0]) * 0.5,
                    s.plane_height,
                    (s.minimum[1] + s.maximum[1]) * 0.5,
                ];
                if self
                    .authored_ocean_definitions
                    .iter()
                    .any(|a| a.contains_render_point(centre))
                {
                    continue;
                }
                boxes.push(crate::ocean::WaterVolume {
                    minimum: [s.minimum[0], world.ocean_bottoms[slot], s.minimum[1]],
                    maximum: [s.maximum[0], s.plane_height, s.maximum[1]],
                    mask_slot: world.ocean_masks.is_masked(slot).then_some(slot as u8),
                });
            }
        }
        // The camera is submerged only inside the real outline: standing in the
        // hollow of an island ring is inside the box but not in the water.
        let no_masks = crate::ocean::OceanMasks::default();
        let masks = self.world.as_ref().map_or(&no_masks, |world| &*world.ocean_masks);
        // Waves move the top to `plane + offset`; the brush bottom stays fixed.
        let wave_offset = self.eye_wave_offset;
        let eye_point = eye.to_array();
        let mut surface = boxes
            .iter()
            .filter(|volume| volume.contains_displaced(eye_point, masks, wave_offset))
            .map(|volume| volume.maximum[1])
            .fold(NO_WATER_SURFACE, f32::max);
        // A camera beyond a sky-connected water edge is still inside the
        // *rendered* ocean's underwater medium. Keep the original volume boxes
        // untouched; this is an independent rendering/optics classification.
        if self.ocean_sky_extend {
            if let Some(world) = self.world.as_ref() {
                for (slot, region) in world.ocean_sky_regions.iter().enumerate() {
                    let (Some(region), Some(ocean)) = (region.as_ref(), world.ocean_surfaces.get(slot)) else {
                        continue;
                    };
                    if self.ocean_extension_active(slot)
                        && eye.y >= world.ocean_bottoms[slot]
                        && eye.y - wave_offset <= ocean.plane_height && region.contains([eye.x, eye.z]) {
                        surface = surface.max(ocean.plane_height);
                    }
                }
            }
        }
        self.camera_water_surface = surface;
        self.water_boxes = boxes;
    }

    pub(in crate::renderer) fn prepare_wind_visuals_frame(&mut self, frame_time: f32) {
        if !self.weather.wind_visuals.active() {
            return;
        }
        self.weather
            .wind_visuals
            .ensure_render_pipelines(&mut self.pipeline_jobs, &self.device);
        // TaystJK/Raven weather particles are explicitly outside-only. Reuse the
        // existing heightfield rather than introducing another indoor/outdoor cache.
        self.ensure_weather_occlusion();
        let occlusion = match &self.weather.rain.occlusion {
            weather::WeatherOcclusionCache::Ready(info) => Some(*info),
            weather::WeatherOcclusionCache::Pending(_)
            | weather::WeatherOcclusionCache::Building(_)
            | weather::WeatherOcclusionCache::Unavailable => None,
        };
        self.weather.wind_visuals.prepare(
            &self.queue,
            frame_time,
            self.weather_wind,
            occlusion,
            self.ocean_enabled && self.camera_water_surface > NO_WATER_SURFACE,
        );
    }

    pub(in crate::renderer) fn prepare_rain_frame(&mut self, camera: &Camera, frame_time: f32) {
        self.weather
            .rain
            .ensure_render_pipelines(&mut self.pipeline_jobs, &self.device);
        self.ensure_weather_occlusion();
        let sun = self.active_sun();
        self.weather.rain.prepare_frame(
            &self.queue,
            camera,
            frame_time,
            self.previous_frame_time,
            self.config.width,
            self.config.height,
            self.weather_wind,
            sun.color,
            sun.intensity,
            // Rain only treats water as a surface when the GPU ocean is drawing it.
            if self.ocean_enabled {
                &self.water_boxes
            } else {
                &[]
            },
            &self
                .world
                .as_ref()
                .map_or_else(Default::default, |world| std::sync::Arc::clone(&world.ocean_masks)),
            self.ocean_enabled && self.camera_water_surface > NO_WATER_SURFACE,
        );
        // RainyGlass receives the same instantaneous weather gust/veer as the
        // rain particles. It transforms that wind into lens space and applies it
        // only to local droplet/rivulet flow; the authored mask itself stays put.
        self.weather.rain.prepare_lens_glass(
            &mut self.pipeline_jobs,
            &self.device,
            &self.queue,
            self.config.width,
            self.config.height,
            self.config.format,
            frame_time,
            camera,
            self.weather_wind,
            self.weather_assets.as_mut(),
        );
    }

    pub(in crate::renderer) fn prepare_snow_frame(&mut self, camera: &Camera, frame_time: f32) {
        self.weather
            .snow
            .ensure_render_pipeline(&mut self.pipeline_jobs, &self.device);
        self.ensure_weather_occlusion();
        let occlusion = match &self.weather.rain.occlusion {
            weather::WeatherOcclusionCache::Ready(info) => Some(*info),
            weather::WeatherOcclusionCache::Pending(_)
            | weather::WeatherOcclusionCache::Building(_)
            | weather::WeatherOcclusionCache::Unavailable => None,
        };
        let sun = self.active_sun();
        self.weather.snow.prepare_frame(
            &self.queue,
            camera,
            frame_time,
            self.previous_frame_time,
            self.config.height,
            self.weather_wind,
            occlusion,
            sun.color,
            sun.intensity,
        );
    }
}
