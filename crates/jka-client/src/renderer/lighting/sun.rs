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
        let light_direction = Vec3::from_array(self.active_sun().direction);
        self.debug_volumes.update_sun_ray(
            &self.device,
            &self.queue,
            head,
            light_direction,
            camera.position,
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
        let sun = self.active_sun();
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
