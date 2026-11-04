//! Lighting sun.
use crate::renderer::{
    Camera, DirectionalSun, EntityAmbientLightingMode, EntitySunRelight, Renderer, Vec3,
    FALLBACK_SUN_COLOR, FALLBACK_SUN_DIRECTION, FALLBACK_SUN_INTENSITY,
};

impl Renderer {
    /// Feeds the sun-to-head beam shown while the sun angles are edited. The head
    /// is the local player's eye (actor origin + JKA standing view height); the
    /// camera stands in when there is no local player (demos, free camera).
    pub(in crate::renderer) fn update_sun_ray_preview(
        &mut self,
        camera: &Camera,
        player_position: Option<Vec3>,
    ) {
        if !self.debug_volumes.sun_ray_enabled {
            return;
        }
        const STANDING_VIEW_HEIGHT: f32 = 36.0;
        let head = player_position
            .map(|origin| origin + Vec3::Y * STANDING_VIEW_HEIGHT)
            .unwrap_or(camera.position);
        let sun = if self.debug_volumes.sun_ray_authored {
            self.world
                .as_ref()
                .and_then(|world| world.sun)
                .unwrap_or_else(|| self.active_sun())
        } else {
            self.active_sun()
        };
        let light_direction = Vec3::from_array(sun.direction);
        self.debug_volumes.update_sun_ray(
            &self.device,
            &self.queue,
            head,
            light_direction,
            camera.position,
            sun.color,
        );
    }

    pub(in crate::renderer) fn active_sun(&self) -> DirectionalSun {
        if self.sun_override {
            return DirectionalSun::from_q3_angles(
                self.sun_color,
                self.sun_intensity,
                self.sun_yaw,
                self.sun_pitch,
            );
        }
        self.world
            .as_ref()
            .and_then(|world| world.sun)
            .unwrap_or(DirectionalSun {
                direction: FALLBACK_SUN_DIRECTION,
                color: FALLBACK_SUN_COLOR,
                intensity: FALLBACK_SUN_INTENSITY,
            })
    }

    /// Keep the editor's authored sun for the atmosphere and GUI, but stop
    /// injecting real-time sunlight into the BSP after it sets. This does not
    /// alter the map's baked lightmap (which requires baked-sun removal).
    pub(in crate::renderer) fn relit_sun(&self) -> DirectionalSun {
        let mut sun = self.active_sun();
        if self.sky_backend == 1 {
            // `direction` is light travel, so positive Y is below the horizon.
            // Fade across the horizon instead of instantaneously popping.
            sun.intensity *= (-sun.direction[1] / 0.035).clamp(0.0, 1.0);
        }
        sun
    }

    pub(in crate::renderer) fn blue_water_night(&self) -> bool {
        self.sky_backend == 1 && self.active_sun().direction[1] >= 0.0
    }

    /// The direct light used by BSP Relighting, static-sun composition, and
    /// entity-sun replacement. One celestial key light at a time. Moon intensity
    /// uses the same 0.2 direct-light gain as Blue Water's world-lighting pass,
    /// converted to the renderer's /255 radiance convention.
    pub(in crate::renderer) fn relit_celestial(&self) -> DirectionalSun {
        let mut light = self.relit_sun();
        if self.sun_override && self.blue_water_night() {
            light.intensity = 0.0; // No lingering direct sunlight after sunset.
            if let Some(toward_moon) = self.blue_water_moon_toward() {
                light.direction = (-toward_moon).to_array();
                light.color = [0.70, 0.82, 1.0];
                light.intensity = self.blue_water_environment.effective_moon_intensity() * (255.0 * 0.2);
            }
        }
        light
    }

    /// Moon direction is derived from the SAME position model the Moon editor
    /// displays. `Sky::light()` is not an authoritative moon source: it can
    /// report the selected solar light even after sunset.
    pub(in crate::renderer) fn blue_water_moon_toward(&self) -> Option<Vec3> {
        if !self.blue_water_night() { return None; }
        let s = self.blue_water_environment.sanitize();
        let (elevation, azimuth) = if s.moon_custom {
            (f64::from(s.moon_elevation), f64::from(s.moon_azimuth))
        } else {
            let [yaw, pitch] = self.active_sun().q3_angles();
            let mut celestial = ::blue_water_sky::state::CelestialModel::default();
            celestial.set(f64::from(pitch), f64::from((yaw + 90.0).rem_euclid(360.0)), f64::from(s.moon_phase));
            ::blue_water_sky::state::angles_of(celestial.moon)
        };
        if elevation <= 0.0 { return None; }
        let (e, a) = ((elevation as f32).to_radians(), (azimuth as f32).to_radians());
        Some(Vec3::new(e.cos() * a.sin(), e.sin(), e.cos() * a.cos()))
    }

    /// One existing cascade set: at night it follows the moon if moonlight is
    /// enabled and the moon is up, otherwise it keeps the sun. No extra pass.
    pub(in crate::renderer) fn blue_water_moon_shadow_direction(&self) -> Option<Vec3> {
        if !(self.sun_override || self.blue_water_environment.world_lighting > 0.0)
            || !self.cascaded_shadows_enabled
            || self.blue_water_environment.effective_moon_intensity() <= 0.0
        { return None; }
        self.blue_water_moon_toward().map(|toward| -toward)
    }

    pub(in crate::renderer) fn shadow_celestial_direction(&self) -> Vec3 {
        self.blue_water_moon_shadow_direction().unwrap_or_else(|| {
            Vec3::from_array(self.active_sun().direction).normalize_or_zero()
        })
    }

    /// The sun the native river's Single Layer Water follows: Blue Water's
    /// celestial light while that sky is on (as the Blue Water sea receives
    /// it), otherwise the classic `active_sun()` the world is lit with.
    pub(in crate::renderer) fn river_scene_sun(&self) -> Option<crate::river_slw::SceneSun> {
        if self.sky_backend == 1 {
            if let Some(atmosphere) = &self.blue_water_sky {
                let light = atmosphere.sky.light();
                return crate::river_slw::SceneSun::blue_water(
                    light.direction.to_array(),
                    light.color.to_array(),
                    light.intensity,
                );
            }
        }
        let sun = self.active_sun();
        crate::river_slw::SceneSun::classic(sun.direction, sun.color, sun.intensity)
    }

    /// Sun swap for the entity lightgrid: strips the map's baked sun share out of
    /// each probe sample and lights entities with `active_sun()` instead. None
    /// when the setting is off, the entity lightgrid mode isn't in use, or the
    /// map has no authored sun to have been baked in.
    pub(in crate::renderer) fn entity_sun_relight(&self) -> Option<EntitySunRelight> {
        if !self.entity_sun_lighting
            || self.dynamic_model_renderer.entity_ambient_lighting
                != EntityAmbientLightingMode::BspLightgridClassic
        {
            return None;
        }
        let world = self.world.as_ref()?;
        let map_sun = world.sun?;
        let map_toward_sun = world.entity_light_grid.as_ref()?.map_toward_sun()?;
        let sun = self.relit_celestial();
        let toward_sun = Vec3::from_array(sun.direction).normalize_or_zero() * -1.0;
        if toward_sun == Vec3::ZERO {
            return None;
        }
        // Radiance ratio, per channel. The floor keeps a near-zero map channel
        // (a strongly tinted map sun) from exploding the ratio; the probe's
        // baked share is ~0 in that channel too, so the floor costs nothing.
        let radiance = |sun: &DirectionalSun| Vec3::from_array(sun.color) * sun.intensity;
        let baked = radiance(&map_sun);
        let floor = (baked.max_element() * 0.05).max(1e-4);
        let gain = (radiance(&sun) / baked.max(Vec3::splat(floor))).min(Vec3::splat(16.0));
        Some(EntitySunRelight {
            map_toward_sun,
            toward_sun: toward_sun.to_array(),
            gain: gain.to_array(),
        })
    }
}
