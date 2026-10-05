//! Environment weather.
use crate::renderer::{
    create_rt_receiver_bind_group, create_shadow_receiver_bind_group,
    create_world_froxel_bind_group, scene, weather, Camera, DynamicModelSurface,
    DynamicShadowsMode, DynamicWireframeClass, Renderer, Vec3, NO_WATER_SURFACE,
};

impl Renderer {
    pub(in crate::renderer) fn rebuild_weather_surface_receiver_bind_group(&mut self) {
        self.shadow_resources.receiver_bind_group = create_shadow_receiver_bind_group(
            &self.device,
            &self.shadow_receiver_layout,
            &self.shadow_resources._array_view,
            if self.cascaded_shadow_mode == DynamicShadowsMode::CascadedShadowMaps {
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
                &rt.sun_shadow_history,
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
            boxes.push((
                [a.mins[0], a.mins[2], -a.maxs[1]],
                [a.maxs[0], a.height, -a.mins[1]],
            ));
        }
        if let Some(world) = &self.world {
            for s in &world.ocean_surfaces {
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
                boxes.push((
                    [s.minimum[0], -65536.0, s.minimum[1]],
                    [s.maximum[0], s.plane_height, s.maximum[1]],
                ));
            }
        }
        self.camera_water_surface = boxes
            .iter()
            .filter(|(lo, hi)| (0..3).all(|axis| eye[axis] >= lo[axis] && eye[axis] <= hi[axis]))
            .map(|(_, hi)| hi[1])
            .fold(NO_WATER_SURFACE, f32::max);
        self.water_boxes = boxes;
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
            self.ocean_enabled && self.camera_water_surface > NO_WATER_SURFACE,
        );
    }
}
