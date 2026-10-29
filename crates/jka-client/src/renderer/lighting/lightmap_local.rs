//! Static local-light inputs and bounded lightmap-space irradiance.
//! The sunlight worker owns geometry, cancellation and the shared compositor.
use crate::renderer::{scene, Vec3};

pub(crate) const AUTHORED: u32 = crate::materials::ADDED_MAP_LIGHT_AUTHORED;
pub(crate) const FX: u32 = 1 << 4;
pub(crate) const ALL: u32 = crate::materials::SHADER_LIGHT_ALL | AUTHORED | FX;
pub(crate) const DEFAULT_SOURCES: u32 =
    crate::materials::SHADER_LIGHT_GLOW | crate::materials::SHADER_LIGHT_EMISSIVE_COMPANION;
// Reserve three array layers for the RGB sum used by moving sun shadows.
pub(super) const MAX_SOURCES: usize = 253;

pub(in crate::renderer) fn source_enabled(light: &scene::DynamicLight, mask: u32) -> bool {
    let reason = if light.shader_promotion_mask == 0 {
        if light.emitter_normal.iter().any(|v| v.abs() > 1e-6) {
            AUTHORED
        } else {
            0
        }
    } else {
        light.shader_promotion_mask
    };
    reason & mask & ALL != 0
        && !(mask & AUTHORED != 0
            && light.shader_promotion_mask & crate::materials::SHADER_LIGHT_AUTHORED_OVERLAP != 0)
        && light.surface_lighting
}

/// Per-light R32F irradiance stays within 64 MiB, independently of the sun.
/// Lower-resolution local pages preserve the original UV charts and page count.
pub(super) fn output_sizes(base: &[[u32; 2]], lights: usize, limit: u32) -> Vec<[u32; 2]> {
    if base.is_empty() || lights == 0 {
        return Vec::new();
    }
    let mut divisor = 1u32;
    loop {
        let sizes: Vec<_> = base
            .iter()
            .map(|s| [s[0].div_ceil(divisor).max(1), s[1].div_ceil(divisor).max(1)])
            .collect();
        let bytes: u64 = sizes
            .iter()
            .map(|s| u64::from(s[0]) * u64::from(s[1]) * (lights + 3) as u64 * 4)
            .sum();
        if bytes <= 64 * 1024 * 1024 && sizes.iter().all(|s| s[0] <= limit && s[1] <= limit) {
            return sizes;
        }
        if divisor >= 16384 {
            return Vec::new();
        }
        divisor *= 2;
    }
}

/// Matches the smooth local-light diffuse response used by the world shader.
/// Visibility is applied separately by the shared static-geometry tracer.
pub(super) fn irradiance(light: &scene::DynamicLight, position: Vec3, normal: Vec3) -> f32 {
    let delta = Vec3::from_array(light.position) - position;
    let distance = delta.length();
    if distance <= 0.0001 || distance >= light.radius {
        return 0.0;
    }
    let toward = delta / distance;
    let facing = normal.dot(toward).max(0.0);
    let emitter = Vec3::from_array(light.emitter_normal);
    let emission = if emitter.length_squared() > 1e-6 {
        let value = emitter.normalize().dot(-toward);
        if light.emitter_two_sided {
            value.abs()
        } else {
            value.max(0.0)
        }
    } else {
        1.0
    };
    let falloff = (1.0 - distance / light.radius).max(0.0);
    light.intensity * facing * emission * falloff * falloff * 0.35
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_light_budget_scales_all_pages_together() {
        let sizes = output_sizes(&[[2048, 2048], [1024, 1024]], 64, 8192);
        assert_eq!(sizes, [[256, 256], [128, 128]]);
        assert!(output_sizes(&[[32, 32]], 0, 8192).is_empty());
    }
}

/// An inferred static lamp is never an independent shadow mode. The source
/// toggles are the only user-facing on/off controls, while CSM is the
/// renderer permission gate (including during live mode switches).
pub(super) fn inferred_lamp_shadows_allowed(
    sources: u32,
    mode: crate::renderer::DynamicShadowsMode,
) -> bool {
    sources & ALL != 0
        && mode.is_csm()
}

#[cfg(test)]
mod lamp_shadow_gate_tests {
    use super::*;
    use crate::renderer::DynamicShadowsMode;
    #[test]
    fn lamp_shadows_require_a_source_and_csm() {
        for mode in DynamicShadowsMode::ALL {
            assert!(!inferred_lamp_shadows_allowed(0, mode));
            assert_eq!(inferred_lamp_shadows_allowed(AUTHORED, mode), mode.is_csm());
        }
        assert!(inferred_lamp_shadows_allowed(FX | AUTHORED, DynamicShadowsMode::CascadedShadowMaps));
    }
}

impl crate::renderer::Renderer {
    /// The same source selection drives runtime map illumination and optional
    /// lightmap-space reconstruction. Lamp-shadow permissions are independent.
    pub(in crate::renderer) fn added_map_light_sources(&self) -> u32 {
        self.added_map_light_sources_mask
    }

    pub(in crate::renderer) fn set_lamp_shadow_sources(&mut self, sources: u32) {
        let sources = sources & ALL;
        let moving_shadows = inferred_lamp_shadows_allowed(sources, self.cascaded_shadow_mode);
        let caster_policy_changed = self.baked_local_shadows != moving_shadows;
        if self.baked_local_sources == sources && !caster_policy_changed {
            return;
        }
        self.baked_local_sources = sources;
        self.baked_local_shadows = moving_shadows;
        if let Some(world) = self.world.as_mut() {
            world.local_shadows.selection_position = None;
            if caster_policy_changed {
                for (slot, entry) in world.local_shadows.cache_entries.iter().enumerate() {
                    if entry.light_index.is_some()
                        && !world.local_shadows.pending_slots.contains(&slot)
                    {
                        world.local_shadows.pending_slots.push(slot);
                    }
                }
            }
        }
        self.upload_light_buffer();
        self.update_lighting_settings();
        self.rebuild_frame_plan();
        self.activate_world_pipeline_variant();
        self.history_valid = false;
    }
    pub(in crate::renderer) fn baked_local_lightmap_active(&self) -> bool {
        self.added_map_light_sources() != 0
            && self.world.as_ref().is_some_and(|w| {
                w.static_sun.local_ready() && w.baked_sun_cache.as_ref().is_some_and(|c| c.active)
            })
    }
}

impl crate::renderer::Renderer {
    pub(in crate::renderer) fn inferred_shadow_source_enabled(
        &self,
        world: &crate::renderer::WorldGpu,
        index: usize,
    ) -> bool {
        self.baked_local_shadows
            && world
                .static_bake_sources
                .iter()
                .any(|(i, light)| *i == index && source_enabled(light, self.baked_local_sources))
    }

    pub(in crate::renderer) fn static_runtime_light_enabled(
        &self,
        world: &crate::renderer::WorldGpu,
        index: usize,
        light: &scene::DynamicLight,
    ) -> bool {
        index < world.runtime_light_count
            && light.surface_lighting
            && if light.emitter_normal.iter().any(|v| v.abs() > 1e-6) {
                self.static_area_light_enabled(light)
            } else {
                self.static_point_lighting_enabled()
            }
    }

    pub(in crate::renderer) fn static_gpu_light_enabled(
        &self,
        world: &crate::renderer::WorldGpu,
        index: usize,
        light: &scene::DynamicLight,
    ) -> bool {
        // Baking is never permission to turn a source into a runtime light.
        // Only the bounded, selected moving-shadow sources need GPU metadata.
        self.static_runtime_light_enabled(world, index, light)
            || (world.local_shadows.active_light_indices.contains(&index)
                && self.inferred_shadow_source_enabled(world, index))
    }
    /// Four selected moving-shadow sources are addressed directly, avoiding a
    /// cluster walk or per-light texture read on every static receiver.
    pub(in crate::renderer) fn baked_local_shader_inputs(&self) -> ([f32; 2], bool) {
        if !self.baked_local_lightmap_active() && !self.baked_local_shadows {
            return ([0.0; 2], false);
        }
        let Some(world) = self.world.as_ref() else {
            return ([0.0; 2], false);
        };
        let mut ids = [0u32; 4];
        let mut count = 0;
        let mut gpu_index = 0;
        let mut all_baked = self.active_transient_light_count() == 0;
        for (index, light) in world.dynamic_lights.iter().enumerate() {
            if !self.static_gpu_light_enabled(world, index, light) {
                continue;
            }
            let baked = world
                .static_sun
                .local_lights
                .iter()
                .any(|(i, _)| *i == index);
            all_baked &= baked;
            if self.inferred_shadow_source_enabled(world, index)
                && world.local_shadows.active_light_indices.contains(&index)
                && count < 4
                && gpu_index < 255
            {
                ids[count] = gpu_index + 1;
                count += 1;
            }
            gpu_index += 1;
        }
        (
            [(ids[0] | ids[1] << 8) as f32, (ids[2] | ids[3] << 8) as f32],
            all_baked,
        )
    }
}
