//! Lighting state.
use crate::renderer::{
    legacy_dlight_surface_mask, scene, transient_light_gpu, transient_light_samples,
    DirectionalSun, DynamicLightsMode, EntityDlight, GpuPointLight, LegacyDlightPerfStats,
    LightingSettings,
    Mat4, Renderer, TransientLight, Vec3, LOCAL_SHADOW_MAP_SIZE, MAX_DYNAMIC_LIGHTS,
    MAX_ENTITY_DLIGHTS,
};
use bytemuck::Zeroable;

impl Renderer {
    pub(in crate::renderer) fn shader_promoted_area_light_enabled(
        &self,
        light: &scene::DynamicLight,
    ) -> bool {
        light.shader_promotion_mask != 0
            && self.shader_light_promotion_enabled
            && (light.shader_promotion_mask & self.shader_light_promotion_mask) != 0
            // If this promoted emitter comes from a material that also has
            // q3map_surfacelight, authored lighting wins while that path is on.
            // Turning authored area lights off exposes the promotion candidate
            // immediately for A/B testing without rebuilding the map.
            && !(self.emissive_area_lights_enabled
                && (light.shader_promotion_mask
                    & crate::materials::SHADER_LIGHT_AUTHORED_OVERLAP)
                    != 0)
    }

    pub(in crate::renderer) fn static_area_light_enabled(
        &self,
        light: &scene::DynamicLight,
    ) -> bool {
        if light.shader_promotion_mask != 0 {
            self.shader_promoted_area_light_enabled(light)
        } else {
            self.emissive_area_lights_enabled
        }
    }

    pub(in crate::renderer) fn any_area_lights_enabled(&self) -> bool {
        self.emissive_area_lights_enabled
            || (self.shader_light_promotion_enabled && self.shader_light_promotion_mask != 0)
    }

    pub(in crate::renderer) fn transient_dlight_clusters_enabled(&self) -> bool {
        self.dynamic_lights_mode == DynamicLightsMode::ClusteredLite
    }

    pub(in crate::renderer) fn runtime_transient_lights_enabled(&self) -> bool {
        !self.classic_vertex_light
            && matches!(
                self.dynamic_lights_mode,
                DynamicLightsMode::Legacy
                    | DynamicLightsMode::Vertex
                    | DynamicLightsMode::ClusteredLite
                    | DynamicLightsMode::PerPixelForwardPlus
                    | DynamicLightsMode::RayTracedHardware
            )
    }

    pub(in crate::renderer) fn active_static_light_count(&self) -> usize {
        let Some(world) = self.world.as_ref() else {
            return 0;
        };
        world
            .dynamic_lights
            .iter()
            .filter(|light| {
                let is_area = light
                    .emitter_normal
                    .iter()
                    .any(|component| component.abs() > 1.0e-6);
                if is_area {
                    self.static_area_light_enabled(light) && light.surface_lighting
                } else {
                    self.static_point_lighting_enabled() && light.surface_lighting
                }
            })
            .take(MAX_DYNAMIC_LIGHTS)
            .count()
    }

    pub(in crate::renderer) fn active_transient_light_count(&self) -> usize {
        if !self.runtime_transient_lights_enabled() {
            return 0;
        }
        self.transient_lights
            .iter()
            .map(|light| {
                if self.dynamic_lights_mode == DynamicLightsMode::RayTracedHardware {
                    light
                        .blade_segments
                        .as_ref()
                        .map_or(1, |blades| blades.len().max(1))
                } else {
                    1
                }
            })
            .sum::<usize>()
            .min(MAX_DYNAMIC_LIGHTS.saturating_sub(self.active_static_light_count()))
    }

    pub(in crate::renderer) fn active_light_count(&self) -> u32 {
        u32::try_from(self.active_static_light_count() + self.active_transient_light_count())
            .unwrap_or(0)
    }

    /// Perf-trace mirror of the exact Legacy BSP surface masks uploaded to the GPU.
    /// This intentionally does no new culling and changes no rendering behavior; it
    /// only tells us how broad the current OpenJK-style coarse masks actually are.
    pub(in crate::renderer) fn legacy_dlight_perf_stats(&self) -> LegacyDlightPerfStats {
        let mut stats = LegacyDlightPerfStats::default();
        if self.dynamic_lights_mode != DynamicLightsMode::Legacy {
            return stats;
        }
        let Some(world) = self.world.as_ref() else {
            return stats;
        };
        let transient_count = self
            .active_transient_light_count()
            .min(32)
            .min(self.transient_lights.len());
        stats.transient_count = transient_count;
        stats.surfaces_total = world.legacy_dlight_surfaces.len();
        if transient_count == 0 {
            return stats;
        }

        let lights = &self.transient_lights[..transient_count];
        let mut surfaces_per_light = vec![0u32; transient_count];
        for light in lights {
            stats.radius_sum += f64::from(light.radius.max(0.0));
            stats.radius_max = stats.radius_max.max(light.radius.max(0.0));
            match light.kind {
                crate::fx::system::FxLightKind::Saber => stats.saber_sources += 1,
                crate::fx::system::FxLightKind::SaberMark => stats.saber_mark_sources += 1,
                crate::fx::system::FxLightKind::AuthoredEffect
                | crate::fx::system::FxLightKind::PlayerPowerup
                | crate::fx::system::FxLightKind::Missile
                | crate::fx::system::FxLightKind::EntityConstant => stats.authored_fx_sources += 1,
                crate::fx::system::FxLightKind::PromotedEffect => stats.promoted_fx_sources += 1,
            }
        }

        for surface in &world.legacy_dlight_surfaces {
            if surface.cull_kind == 0 {
                stats.surface_buckets[0] += 1;
                continue;
            }
            stats.surfaces_eligible += 1;
            let mask = legacy_dlight_surface_mask(surface, lights, transient_count);
            let count = mask.count_ones();
            stats.candidate_pairs += u64::from(count);
            stats.max_candidates_per_surface = stats.max_candidates_per_surface.max(count);
            if count == 0 {
                stats.surface_buckets[0] += 1;
            } else {
                stats.surfaces_touched += 1;
                let bucket = match count {
                    1 => 1,
                    2..=4 => 2,
                    5..=8 => 3,
                    9..=16 => 4,
                    _ => 5,
                };
                stats.surface_buckets[bucket] += 1;
            }

            let mut bits = mask;
            while bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                if let Some(per_light) = surfaces_per_light.get_mut(bit) {
                    *per_light += 1;
                }
                match lights[bit].kind {
                    crate::fx::system::FxLightKind::Saber => stats.saber_pairs += 1,
                    crate::fx::system::FxLightKind::SaberMark => stats.saber_mark_pairs += 1,
                    crate::fx::system::FxLightKind::AuthoredEffect
                    | crate::fx::system::FxLightKind::PlayerPowerup
                    | crate::fx::system::FxLightKind::Missile
                    | crate::fx::system::FxLightKind::EntityConstant => stats.authored_fx_pairs += 1,
                    crate::fx::system::FxLightKind::PromotedEffect => stats.promoted_fx_pairs += 1,
                }
            }
        }

        stats.max_surfaces_per_light = surfaces_per_light.iter().copied().max().unwrap_or(0);
        stats.total_surfaces_per_light = surfaces_per_light.iter().map(|&v| u64::from(v)).sum();
        stats
    }

    pub(in crate::renderer) fn set_transient_lights(&mut self, lights: &[TransientLight]) {
        if self.transient_lights == lights {
            return;
        }
        self.transient_lights.clear();
        self.transient_lights.extend_from_slice(lights);
        self.upload_light_buffer();
        self.update_lighting_settings();
    }

    /// Hands this frame's runtime lights (the same list the BSP light buffer is
    /// built from) to classic entity lighting, as R_SetupEntityLighting reads
    /// refdef->dlights. Call after `set_transient_lights` and before the dynamic
    /// model prepare. `rt_per_pixel` is true when the RT model receivers will
    /// light this frame's models per pixel, which already includes these lights.
    pub(in crate::renderer) fn sync_entity_dlights(&mut self, rt_per_pixel: bool) {
        let enabled = self.runtime_transient_lights_enabled();
        let brightness = self.dynamic_light_brightness;
        let models = &mut self.dynamic_model_renderer;
        models.entity_dlights_rt_per_pixel = rt_per_pixel;
        models.entity_dlights.clear();
        if !enabled {
            return;
        }
        models.entity_dlights.extend(
            self.transient_lights
                .iter()
                .take(MAX_ENTITY_DLIGHTS)
                .map(|light| EntityDlight {
                    position: Vec3::from_array(light.position),
                    color: Vec3::from_array(light.color) * (light.intensity.max(0.0) * brightness),
                    radius: light.radius.max(0.0),
                }),
        );
    }

    pub(in crate::renderer) fn update_legacy_dlight_surface_masks(&self) {
        if self.dynamic_lights_mode != DynamicLightsMode::Legacy {
            return;
        }
        let Some(world) = self.world.as_ref() else {
            return;
        };
        if world.legacy_dlight_surfaces.is_empty() {
            return;
        }

        let transient_count = self
            .active_transient_light_count()
            .min(32)
            .min(self.transient_lights.len());
        super::legacy_dlight_lists::update_legacy_dlight_masks(
            &self.queue,
            world,
            &self.transient_lights[..transient_count],
        );
    }

    pub(in crate::renderer) fn upload_light_buffer(&self) {
        let Some(world) = self.world.as_ref() else {
            return;
        };

        let mut shadow_slot_by_light = vec![0_u32; world.dynamic_lights.len()];
        if self.local_light_shadows_enabled {
            for (slot, entry) in world.local_shadows.cache_entries.iter().enumerate() {
                let Some(light_index) = entry.light_index else {
                    continue;
                };
                if world
                    .local_shadows
                    .active_light_indices
                    .contains(&light_index)
                    && light_index < shadow_slot_by_light.len()
                {
                    shadow_slot_by_light[light_index] = u32::try_from(slot + 1).unwrap_or(0);
                }
            }
        }

        // Keep only the static source classes required by the selected mode.
        // Legacy/Vertex/Clustered Lite intentionally exclude map point lights; they are
        // for runtime-authored RE_AddLightToScene/FX sources. Area lights remain independently gated.
        let mut gpu_lights = Vec::with_capacity(MAX_DYNAMIC_LIGHTS);
        for (index, light) in world.dynamic_lights.iter().enumerate() {
            let is_area = light
                .emitter_normal
                .iter()
                .any(|component| component.abs() > 1.0e-6);
            if !light.surface_lighting
                || (is_area && !self.static_area_light_enabled(light))
                || (!is_area && !self.static_point_lighting_enabled())
            {
                continue;
            }
            gpu_lights.push(GpuPointLight {
                position_radius: [
                    light.position[0],
                    light.position[1],
                    light.position[2],
                    light.radius,
                ],
                color_intensity: [
                    light.color[0],
                    light.color[1],
                    light.color[2],
                    light.intensity,
                ],
                emitter: [
                    light.emitter_normal[0],
                    light.emitter_normal[1],
                    light.emitter_normal[2],
                    if is_area {
                        if light.emitter_two_sided {
                            2.0
                        } else {
                            1.0
                        }
                    } else if light.falloff.shader_value() > 0.5 {
                        // Negative emitter.w is reserved for source-map q3map point
                        // metadata and remains a non-area light to all existing paths:
                        // -1 no angle attenuation, -2 normal Lambert, <-2 _anglescale.
                        if !light.angle_attenuation {
                            -1.0
                        } else if light.angle_scale != 0.0 {
                            -(2.0 + light.angle_scale.abs())
                        } else {
                            -2.0
                        }
                    } else {
                        0.0
                    },
                ],
                shape: [
                    0.0,
                    0.0,
                    (light.shader_promotion_mask & crate::materials::SHADER_LIGHT_ALL) as f32,
                    0.0,
                ],
                shadow: [
                    shadow_slot_by_light.get(index).copied().unwrap_or(0) as f32,
                    light.falloff.shader_value(),
                    light.extra_distance,
                    0.0,
                ],
            });
            if gpu_lights.len() == MAX_DYNAMIC_LIGHTS {
                break;
            }
        }

        let remaining = MAX_DYNAMIC_LIGHTS.saturating_sub(gpu_lights.len());
        if self.runtime_transient_lights_enabled() {
            // RenderSnapshot::transient_lights contains authored FX lights,
            // optional promoted emissive FX, sabers, and saber-mark heat lights.
            // Full Forward+/RT give runtime FX lights a mode gain so the saber lights a wall as strongly
            // as Legacy does (calibrated by screenshot at brightness 1.0, using
            // Legacy as the reference); static map and emissive-area lights above
            // keep their authored intensity. Brightness scales intensity only, in
            // every mode, so total added light stays proportional to it and the
            // modes stay equal at any brightness.
            let transient_intensity_scale = self.dynamic_light_brightness
                * if matches!(
                    self.dynamic_lights_mode,
                    DynamicLightsMode::PerPixelForwardPlus | DynamicLightsMode::RayTracedHardware
                ) {
                    2.43
                } else {
                    1.0
                };
            gpu_lights.extend(
                self.transient_lights
                    .iter()
                    .flat_map(|light| {
                        transient_light_samples(
                            light,
                            self.dynamic_lights_mode == DynamicLightsMode::RayTracedHardware,
                        )
                    })
                    .take(remaining)
                    .map(|light| {
                        transient_light_gpu(
                            &light,
                            transient_intensity_scale,
                            self.dynamic_lights_mode == DynamicLightsMode::RayTracedHardware,
                        )
                    }),
            );
        }
        if gpu_lights.is_empty() {
            gpu_lights.push(GpuPointLight::zeroed());
        }
        self.queue
            .write_buffer(&world.light_buffer, 0, bytemuck::cast_slice(&gpu_lights));
    }

    pub(in crate::renderer) fn update_lighting_settings(&self) {
        let light_count = self.active_light_count();
        let transient_start = u32::try_from(self.active_static_light_count()).unwrap_or(0);
        let shadowed_count = self
            .world
            .as_ref()
            .map_or(0, |world| world.local_shadows.shadowed_light_count);
        let direct_local_lighting_enabled = self.point_lighting_enabled()
            || matches!(
                self.dynamic_lights_mode,
                DynamicLightsMode::Legacy
                    | DynamicLightsMode::Vertex
                    | DynamicLightsMode::ClusteredLite
            )
            || self.any_area_lights_enabled();
        let source_map_lighting = self
            .world
            .as_ref()
            .filter(|world| world.source_map)
            .map_or(scene::SourceMapLighting::default(), |world| {
                world.source_map_lighting
            });
        // A BSP lightmap is one combined image, not a per-light decomposition.
        // Use a conservative estimate only for maps whose authored sun is known;
        // this path only subtracts its estimated direct contribution.
        let authored_sun = self.world.as_ref()
            .filter(|world| !world.source_map)
            .and_then(|world| world.sun);
        let has_authored_sun = authored_sun.is_some();
        let old = authored_sun.unwrap_or_else(|| self.active_sun());
        // Pending/unsupported reconstruction must preserve the authored image.
        let sun_bake_ready = self.world.as_ref().is_some_and(|world|
            world.sun_bake.ready && world.baked_sun_radiance.is_some());
        let strength = if has_authored_sun && sun_bake_ready { self.baked_sun_removal } else { 0.0 };
        let old_toward = -Vec3::from_array(old.direction).normalize_or_zero();
        let radiance = |sun: DirectionalSun| {
            let rgb = Vec3::from_array(sun.color) * (sun.intensity.max(0.0) / 255.0);
            [rgb.x, rgb.y, rgb.z, 0.0]
        };
        let old_map = self.shadow_resources.original_sun_cache.filter(|cache| {
            self.baked_sun_shadow_map && self.world.as_ref().is_some_and(|world| {
                !world.source_map && world.remap_asset_hash == cache.map_identity
                    && world.sun.is_some_and(|sun| sun.direction == cache.direction)
            })
        });
        let settings = LightingSettings {
            values: [
                u32::from(direct_local_lighting_enabled && light_count != 0),
                light_count,
                self.config.width.max(1),
                self.config.height.max(1),
            ],
            local_shadows: [
                u32::from(
                    direct_local_lighting_enabled
                        && self.local_light_shadows_enabled
                        && shadowed_count != 0,
                ),
                shadowed_count,
                LOCAL_SHADOW_MAP_SIZE,
                transient_start,
            ],
            feature_flags: [
                u32::from(self.any_area_lights_enabled()),
                u32::from(
                    self.voxel_probe_gi_enabled
                        && self
                            .world
                            .as_ref()
                            .is_some_and(|world| world.voxel_probe_gi.enabled),
                ),
                // Used by cluster/froxel consumers. Clustered Lite builds transient-only
                // lists. Legacy uses OpenJK-style BSP surface dlight masks instead.
                (if self.point_lighting_enabled() {
                    1
                } else if self.transient_dlight_clusters_enabled() {
                    2
                } else {
                    0
                }) | (if self.dynamic_light_debug { 0x100 } else { 0 })
                    | (if self.tinted_glass_enabled { 0x200 } else { 0 })
                    | (if self.tinted_glass_debug == 1 { 0x400 } else { 0 })
                    | (if self.tinted_glass_debug == 2 { 0x800 } else { 0 }),
                self.reflection_quality.shader_value(),
            ],
            map_ambient: [
                source_map_lighting.ambient[0],
                source_map_lighting.ambient[1],
                source_map_lighting.ambient[2],
                self.rt_samples as f32,
            ],
            baked_sun_direction_strength: [old_toward.x, old_toward.y, old_toward.z, strength],
            baked_sun_radiance: self.world.as_ref()
                .and_then(|world| world.baked_sun_radiance)
                .map_or_else(|| radiance(old), |rgb| [rgb[0], rgb[1], rgb[2], 1.0]),
            original_sun_shadow_view_proj: old_map.map_or(
                Mat4::IDENTITY.to_cols_array_2d(),
                |cache| cache.view_proj.to_cols_array_2d(),
            ),
            original_sun_shadow_params: [
                if self.baked_sun_shadow_map { 1.0 } else { 0.0 },
                old_map.map_or(0.0, |cache| cache.texel_size),
                if old_map.is_some() { 1.0 } else { 0.0 },
                0.0,
            ],
            baked_indirect_adjust: [
                self.baked_indirect_scale,
                self.baked_indirect_saturation,
                self.baked_sun_edge_softness,
                self.baked_sun_direct_limit,
            ],
            baked_indirect_curve: [
                self.baked_indirect_power,
                self.baked_indirect_highlight_preserve,
                0.0, 0.0,
            ],
            baked_sun_debug_mode: [if sun_bake_ready { self.baked_sun_debug_mode } else { 0 }, u32::from(self.baked_sun_cache_active()), 0, 0],
            runtime_sun_direction: {
                let toward = -Vec3::from_array(self.active_sun().direction);
                [toward.x, toward.y, toward.z, 0.0]
            },
            runtime_sun_radiance: {
                let rgb = radiance(self.active_sun());
                [rgb[0], rgb[1], rgb[2], if self.sun_lighting && self.world.as_ref().is_some_and(|w| !w.source_map) { 1.0 } else { 0.0 }]
            },
            map_minlight: [
                source_map_lighting.minlight[0],
                source_map_lighting.minlight[1],
                source_map_lighting.minlight[2],
                self.dynamic_light_falloff as f32,
            ],
        };
        let settings_bytes = bytemuck::bytes_of(&settings);
        // RT reads its sample count and every consumer the dlight falloff mode from
        // existing padding. The appended live-sun fields share this upload. Always
        // upload the whole struct so changes back to 0 cannot leave stale values.
        let upload_bytes = settings_bytes;
        self.queue
            .write_buffer(&self.lighting_settings_buffer, 0, upload_bytes);
        self.update_legacy_dlight_surface_masks();
    }
}
