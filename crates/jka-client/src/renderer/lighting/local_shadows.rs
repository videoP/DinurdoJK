//! Lighting local shadows.
use crate::renderer::{
    hidden_inline_batch, scene, BlendMode, Camera, DrawClass, DynamicLightsMode,
    GpuPointLight, LocalShadowCacheEntry, LocalShadowResources, Mat4, Renderer,
    ShadowCasterUniform, Vec3, WorldGpu, GpuPass, DEPTH_FORMAT, LOCAL_SHADOW_CACHE_SLOTS,
    LOCAL_SHADOW_HYSTERESIS, LOCAL_SHADOW_MAP_SIZE, LOCAL_SHADOW_NEAR,
    LOCAL_SHADOW_RESELECT_DISTANCE, MAX_LOCAL_SHADOW_LIGHTS,
};
use bytemuck::Zeroable;
use crate::renderer::lighting::glass_transmission::{
    RasterGlassSlot, RASTER_GLASS_PER_LIGHT,
};
use wgpu::util::DeviceExt;

impl Renderer {
    pub(in crate::renderer) fn set_lamp_shadow_ab(&mut self, mode: u8) {
        let mode = mode.min(4);
        if self.lamp_shadow_ab == mode { return; }
        self.lamp_shadow_ab = mode;
        if let Some(world) = &mut self.world {
            // Explicitly reset the GPU octant payload when returning to control.
            world.local_shadows.moving_octant_masks.fill(0);
        }
        self.upload_light_buffer();
        // A/B=4 also toggles the conservative pixel receiver early-out
        // bit carried in LightingSettings, not just per-light octant masks.
        self.update_lighting_settings();
        rverbose!(1, "[LAMP SHADOW A/B] mode={} (0=control, 1=receiver octants, 2=caster prefilter, 3=combined, 4=conservative early-out)", mode);
    }

    pub(in crate::renderer) fn ray_traced_local_shadows_active(&self) -> bool {
        self.world.as_ref().is_some_and(|world| {
            world.active_pipeline_variant.ray_traced_shadows
                && world.active_pipeline_variant.local_shadows
        })
    }

    pub(in crate::renderer) fn hardware_rt_requested(&self) -> bool {
        self.dynamic_lights_mode == DynamicLightsMode::RayTracedHardware
    }

    pub(in crate::renderer) fn hardware_rt_active(&self) -> bool {
        self.world
            .as_ref()
            .is_some_and(|world| world.active_pipeline_variant.ray_traced_shadows)
    }

    pub(in crate::renderer) fn update_local_shadow_selection(&mut self, camera: &Camera, player_position: Option<Vec3>) {
        // Static map lights keep their raster cache even when transient lights
        // use RT; transient lights never consume cubemap slots.
        let direct_local_lighting_enabled =
            self.static_point_lighting_enabled() || self.any_area_lights_enabled() || self.baked_local_shadows;
        if !direct_local_lighting_enabled || !(self.local_light_shadows_enabled || self.baked_local_shadows) {
            return;
        }

        let focus=player_position.unwrap_or(camera.position);
        let reselect_distance=if self.baked_local_shadows {32.0} else {LOCAL_SHADOW_RESELECT_DISTANCE};
        let should_reselect = self.world.as_ref().is_some_and(|world| {
            if world.light_count == 0 {
                return false;
            }
            let shadows = &world.local_shadows;
            let cluster_changed = shadows.selection_cluster != world.last_cluster;
            let moved = shadows.selection_position.is_none_or(|position| {
                position.distance_squared(focus)
                    >= reselect_distance * reselect_distance
            });
            cluster_changed || moved
        });
        if !should_reselect {
            return;
        }

        let selected = {
            let Some(world) = self.world.as_ref() else {
                return;
            };
            select_relevant_shadow_lights(
                world,
                focus,
                self.static_point_lighting_enabled(),
                self.emissive_area_lights_enabled,
                self.shader_light_promotion_enabled,
                self.shader_light_promotion_mask,
                if self.baked_local_shadows { self.baked_local_sources } else { 0 },
            )
        };

        let mut changed = false;
        if let Some(world) = &mut self.world {
            let previous_active = world.local_shadows.active_light_indices.clone();
            let shadows = &mut world.local_shadows;
            shadows.selection_serial = shadows.selection_serial.wrapping_add(1).max(1);
            let serial = shadows.selection_serial;

            for &light_index in &selected {
                let slot = if let Some(slot) = shadows
                    .cache_entries
                    .iter()
                    .position(|entry| entry.light_index == Some(light_index))
                {
                    slot
                } else {
                    let slot = shadows
                        .cache_entries
                        .iter()
                        .position(|entry| entry.light_index.is_none())
                        .or_else(|| {
                            shadows
                                .cache_entries
                                .iter()
                                .enumerate()
                                .filter(|(_, entry)| {
                                    entry
                                        .light_index
                                        .is_none_or(|cached| !selected.contains(&cached))
                                })
                                .min_by_key(|(_, entry)| entry.last_used)
                                .map(|(slot, _)| slot)
                        })
                        .unwrap_or(0);

                    shadows.cache_entries[slot] = LocalShadowCacheEntry {
                        light_index: Some(light_index),
                        last_used: serial,
                    };
                    if !shadows.pending_slots.contains(&slot) {
                        shadows.pending_slots.push(slot);
                    }

                    let light = world.dynamic_lights[light_index];
                    let matrices =
                        local_shadow_face_matrices(Vec3::from_array(light.position), light.radius);
                    for (face, matrix) in matrices.into_iter().enumerate() {
                        let face_index = slot * 6 + face;
                        self.queue.write_buffer(
                            &shadows.caster_buffers[face_index],
                            0,
                            bytemuck::bytes_of(&ShadowCasterUniform {
                                view_proj: matrix.to_cols_array_2d(),
                                camera_pos_time: [
                                    camera.position.x,
                                    camera.position.y,
                                    camera.position.z,
                                    self.started.elapsed().as_secs_f32(),
                                ],
                            }),
                        );
                    }
                    slot
                };
                shadows.cache_entries[slot].last_used = serial;
            }

            shadows.active_light_indices = selected.clone();
            shadows.shadowed_light_count = u32::try_from(selected.len()).unwrap_or(0);
            shadows.selection_cluster = world.last_cluster;
            shadows.selection_position = Some(focus);

            // Upload after this mutable world borrow ends so the transient,
            // explicitly-unshadowed tail is preserved as well.
            changed = previous_active != selected;
        }

        self.upload_light_buffer();

        if changed {
            if let Some(world) = &self.world {
                rverbose!(
                    1,
                    "Local light shadows: {} active nearby/PVS-relevant light(s), {} cached cubemap slot(s)",
                    world.local_shadows.shadowed_light_count,
                    LOCAL_SHADOW_CACHE_SLOTS
                );
            }
        }
        self.update_lighting_settings();
    }

    pub(in crate::renderer) fn ensure_local_shadow_cache(&mut self) {
        let direct_local_lighting_enabled =
            self.static_point_lighting_enabled() || self.any_area_lights_enabled() || self.baked_local_shadows;
        if !direct_local_lighting_enabled || !(self.local_light_shadows_enabled || self.baked_local_shadows) {
            return;
        }
        let pending_slots = self
            .world
            .as_ref()
            .map(|world| world.local_shadows.pending_slots.clone())
            .unwrap_or_default();
        if let Some(world) = &mut self.world {
            world.local_shadows.ab_last_static_rebuilds = pending_slots.len() as u32;
        }
        if pending_slots.is_empty() {
            return;
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("JKA local-light shadow cache encoder"),
            });
        self.gpu_profiler.write_encoder_timestamp(&mut encoder, GpuPass::LocalShadowStatic, false);
        if let Some(world) = &self.world {
            for &slot in &pending_slots {
                let Some(light_index) = world.local_shadows.cache_entries[slot].light_index else {
                    continue;
                };
                let light = world.dynamic_lights[light_index];
                // Pack only triangles inside the light volume, once per invalidated
                // slot. Store count in the slot header, not in global light color.
                let glass = if self.tinted_glass_enabled {
                    let pack_started = std::time::Instant::now();
                    let (glass, total) = crate::renderer::lighting::glass_transmission::pack_raster_glass_slot(
                        light.position, light.radius,
                        &world.raster_glass_static, &world.raster_glass_dynamic);
                    if total > RASTER_GLASS_PER_LIGHT {
                        rverbose!(1, "Raster tinted glass: light {} has {} candidate triangles (cap {}), tint disabled for this light",
                            light_index, total, RASTER_GLASS_PER_LIGHT);
                    }
                    rverbose!(2, "Raster tinted glass: light {}, slot {}, candidates {}, packed {}, cpu {:.3}ms",
                        light_index, slot, total, glass.header[0],
                        pack_started.elapsed().as_secs_f64() * 1000.0);
                    glass
                } else { RasterGlassSlot::default() };
                self.queue.write_buffer(&world.local_shadows.glass_buffer,
                    (slot * std::mem::size_of::<RasterGlassSlot>()) as u64,
                    bytemuck::bytes_of(&glass));
                let light_sphere = [
                    light.position[0],
                    light.position[1],
                    light.position[2],
                    light.radius,
                ];
                let face_matrices =
                    local_shadow_face_matrices(Vec3::from_array(light.position), light.radius);
                for face in 0..6 {
                    let face_index = slot * 6 + face;
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("JKA cached static local-light cubemap shadow"),
                        color_attachments: &[],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &world.local_shadows.layer_views[face_index],
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(1.0),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_pipeline(&self.shadow_resources.pipeline);
                    pass.set_bind_group(
                        0,
                        &world.local_shadows.caster_bind_groups[face_index],
                        &[],
                    );
                    pass.set_vertex_buffer(0, world.vertex_buffer.slice(..));
                    pass.set_vertex_buffer(1, world.legacy_dlight_surface_id_buffer.slice(..));
                    pass.set_index_buffer(world.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    let mut mask_pipeline_active = false;
                    for batch in &world.coarse_batches {
                        if (self.baked_local_shadows && (batch.is_inline_entity || super::lightmap_sun::moving_static_material(&batch.source,crate::renderer::SunVisibilityMode::Filtered)))
                            || (batch.is_inline_entity && !self.draw_inline_models)
                            || hidden_inline_batch(batch, &world.inline_models) {
                            continue;
                        }
                        if self.tinted_glass_enabled
                            && crate::renderer::lighting::glass_transmission::classify_glass(
                                &batch.source,
                                batch.source.texture.and_then(|index| world.glass_texture_names.get(index)).map(String::as_str),
                                None,
                                true,
                            ).is_some()
                        { continue; }
                        if batch.source.pipeline.blend != BlendMode::Opaque
                            || !aabb_intersects_sphere(
                                batch.bounds_min,
                                batch.bounds_max,
                                [light_sphere[0], light_sphere[1], light_sphere[2]],
                                light_sphere[3],
                            )
                            || !aabb_intersects_clip_frustum(
                                batch.bounds_min,
                                batch.bounds_max,
                                face_matrices[face],
                            )
                        {
                            continue;
                        }
                        match batch.source.pipeline.class {
                            DrawClass::Opaque if batch.source.alpha_cutoff == 0.0 => {
                                if mask_pipeline_active {
                                    pass.set_pipeline(&self.shadow_resources.pipeline);
                                    mask_pipeline_active = false;
                                }
                                crate::renderer::draw_world_batch_secondary(&mut pass, batch, 0..1, None);
                            }
                            DrawClass::Mask if batch.source.alpha_cutoff != 0.0 => {
                                if !mask_pipeline_active {
                                    pass.set_pipeline(&self.shadow_resources.mask_pipeline);
                                    mask_pipeline_active = true;
                                }
                                pass.set_bind_group(1, &batch.bind_group, &[]);
                                crate::renderer::draw_world_batch_secondary(&mut pass, batch, 0..1, None);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        self.gpu_profiler.write_encoder_timestamp(&mut encoder, GpuPass::LocalShadowStatic, true);
        self.queue.submit(Some(encoder.finish()));
        if let Some(world) = &mut self.world {
            world.local_shadows.pending_slots.clear();
        }
    }
}

pub(in crate::renderer) fn gpu_point_lights(
    lights: &[scene::DynamicLight],
    shadow_slot_by_light: &[u32],
) -> Vec<GpuPointLight> {
    let mut gpu_lights = lights
        .iter()
        .enumerate()
        .map(|(index, light)| GpuPointLight {
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
                if light.emitter_normal.iter().any(|value| value.abs() > 1e-6) {
                    if light.emitter_two_sided {
                        2.0
                    } else {
                        1.0
                    }
                } else if light.falloff.shader_value() > 0.5 {
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
        })
        .collect::<Vec<_>>();
    if gpu_lights.is_empty() {
        gpu_lights.push(GpuPointLight::zeroed());
    }
    gpu_lights
}

pub(in crate::renderer) fn local_light_is_pvs_relevant(
    world: &WorldGpu,
    light: scene::DynamicLight,
) -> bool {
    let Some(cluster) = world.last_cluster.flatten() else {
        return true;
    };

    if let Some(indices) = world.full_visible_batches_by_cluster.get(cluster) {
        if !indices.is_empty() {
            return indices.iter().any(|&batch_index| {
                let batch = &world.full_batches[batch_index];
                aabb_intersects_sphere(
                    batch.bounds_min,
                    batch.bounds_max,
                    light.position,
                    light.radius,
                )
            });
        }
    }

    world
        .coarse_visible_batches_by_cluster
        .get(cluster)
        .is_none_or(|indices| {
            indices.iter().any(|&batch_index| {
                let batch = &world.coarse_batches[batch_index];
                aabb_intersects_sphere(
                    batch.bounds_min,
                    batch.bounds_max,
                    light.position,
                    light.radius,
                )
            })
        })
}

pub(in crate::renderer) fn local_shadow_importance(
    light: scene::DynamicLight,
    camera_position: Vec3,
    already_active: bool,
) -> f32 {
    if !light.intensity.is_finite()
        || !light.radius.is_finite()
        || light.intensity <= 0.0
        || light.radius <= LOCAL_SHADOW_NEAR * 2.0
    {
        return 0.0;
    }

    if light.shadow_power > 0.0 {
        let distance_sq=Vec3::from_array(light.position).distance_squared(camera_position);
        let normal=Vec3::from_array(light.emitter_normal);
        let toward_player=(camera_position-Vec3::from_array(light.position)).normalize_or_zero();
        let emission=if normal.length_squared()>1e-6 {
            let cosine=normal.normalize().dot(toward_player);
            if light.emitter_two_sided {cosine.abs()} else {cosine.max(0.0)}
        } else {1.0};
        let reach=light.radius+128.0;
        let window=(1.0-distance_sq/(reach*reach)).max(0.0);
        return emission*light.shadow_power/(distance_sq+4096.0)*window*window
            * if already_active {LOCAL_SHADOW_HYSTERESIS} else {1.0};
    }

    let distance = Vec3::from_array(light.position)
        .distance(camera_position)
        .max(LOCAL_SHADOW_NEAR);
    let radius = light.radius.max(LOCAL_SHADOW_NEAR);
    let outside_distance = (distance - radius).max(0.0);
    let projected_influence = (radius / distance.max(radius * 0.25)).clamp(0.0, 4.0);
    let proximity = 1.0 / (1.0 + outside_distance / radius);
    let energy = light.intensity.max(0.0).sqrt();
    let hysteresis = if already_active {
        LOCAL_SHADOW_HYSTERESIS
    } else {
        1.0
    };
    // Eye position is not a receiver: a third-person camera behind the lamp
    // plane must not remove the light casting a shadow from the player.
    energy * projected_influence * projected_influence * proximity * hysteresis
}

pub(in crate::renderer) fn inferred_lamp_in_player_pvs(
    visibility: Option<&jka_assets::bsp::Visibility>,
    light: scene::DynamicLight,
    focus: Vec3,
) -> bool {
    let Some(vis)=visibility else {return true;};
    let Some(lamp_cluster)=vis.cluster_at(scene::jka_position(light.position)) else {return true;};
    vis.visible(vis.cluster_at(scene::jka_position(focus.to_array())), &[lamp_cluster])
}

pub(in crate::renderer) fn select_relevant_shadow_lights(
    world: &WorldGpu,
    focus: Vec3,
    include_point_lights: bool,
    include_authored_area_lights: bool,
    shader_promotion_enabled: bool,
    shader_promotion_mask: u32,
    baked_sources: u32,
) -> Vec<usize> {
    let mut candidates = world
        .dynamic_lights
        .iter()
        .copied()
        .enumerate()
        .filter_map(|(index, light)| {
            let baked = world.static_bake_sources.iter().any(|(source,l)|*source==index && super::lightmap_local::source_enabled(l,baked_sources));
            if baked && !inferred_lamp_in_player_pvs(world.visibility.as_ref(),light,focus) {return None;}
            if index >= world.runtime_light_count && !baked { return None; }
            if !light.surface_lighting {
                return None;
            }
            let is_area = light.emitter_normal.iter().any(|value| value.abs() > 1e-6);
            if is_area {
                let enabled = if light.shader_promotion_mask != 0 {
                    shader_promotion_enabled
                        && (light.shader_promotion_mask & shader_promotion_mask) != 0
                        && !(include_authored_area_lights
                            && (light.shader_promotion_mask
                                & crate::materials::SHADER_LIGHT_AUTHORED_OVERLAP)
                                != 0)
                } else {
                    include_authored_area_lights
                };
                if !enabled && !baked {
                    return None;
                }
            }
            if !is_area && !include_point_lights && !baked {
                return None;
            }
            // Cubemaps cover both hemispheres, including two-sided glow.
            if !local_light_is_pvs_relevant(world, light) {
                return None;
            }
            let score = local_shadow_importance(
                light,
                focus,
                world.local_shadows.active_light_indices.contains(&index),
            );
            (score > 0.0).then_some((index, score))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|(a_index, a_score), (b_index, b_score)| {
        b_score
            .total_cmp(a_score)
            .then_with(|| a_index.cmp(b_index))
    });
    candidates.truncate(MAX_LOCAL_SHADOW_LIGHTS);
    candidates.into_iter().map(|(index, _)| index).collect()
}

/// Octants intersected by an AABB as seen from a fixed world-space lamp.
/// This is a conservative ray-direction test, not a depth or screen-space test.
/// A blocker in another octant cannot shadow this receiver. Widen around axis
/// planes to retain PCF samples that cross a sign boundary.
pub(in crate::renderer) fn moving_caster_octants(
    minimum: [f32; 3], maximum: [f32; 3], lamp: [f32; 3], radius: f32,
) -> u32 {
    let pad = (radius * 0.02).max(2.0);
    let mut result = 0u32;
    for octant in 0..8u32 {
        if (0..3).all(|axis| {
            if octant & (1u32 << axis) == 0 {
                minimum[axis] <= lamp[axis] + pad
            } else {
                maximum[axis] >= lamp[axis] - pad
            }
        }) { result |= 1u32 << octant; }
    }
    result
}

pub(in crate::renderer) fn local_shadow_face_matrices(position: Vec3, radius: f32) -> [Mat4; 6] {
    let far_plane = radius.max(LOCAL_SHADOW_NEAR + 1.0);
    let mut projection = Mat4::perspective_rh(
        std::f32::consts::FRAC_PI_2,
        1.0,
        LOCAL_SHADOW_NEAR,
        far_plane,
    );
    // Cube sampling defines its face V axis independently of render-target
    // coordinates. Wgpu raster targets have Y down, unlike the GL cubemap
    // camera convention below. Flip projection Y so captured casters and
    // textureSampleCompareLevel address the same direction off the face centre.
    projection.y_axis.y = -projection.y_axis.y;
    let faces = [
        (Vec3::X, -Vec3::Y),
        (-Vec3::X, -Vec3::Y),
        (Vec3::Y, Vec3::Z),
        (-Vec3::Y, -Vec3::Z),
        (Vec3::Z, -Vec3::Y),
        (-Vec3::Z, -Vec3::Y),
    ];
    faces.map(|(direction, up)| projection * Mat4::look_to_rh(position, direction, up))
}

#[cfg(test)]
#[path = "local_shadow_tests.rs"]
mod tests;

pub(in crate::renderer) fn create_local_shadow_resources(
    device: &wgpu::Device,
    caster_layout: &wgpu::BindGroupLayout,
) -> LocalShadowResources {
    let layer_count = u32::try_from(LOCAL_SHADOW_CACHE_SLOTS * 12).unwrap_or(6);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("JKA local-light shadow cubemap cache"),
        size: wgpu::Extent3d {
            width: LOCAL_SHADOW_MAP_SIZE,
            height: LOCAL_SHADOW_MAP_SIZE,
            depth_or_array_layers: layer_count,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let cube_view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("JKA local-light shadow cache cube-array view"),
        format: Some(DEPTH_FORMAT),
        dimension: Some(wgpu::TextureViewDimension::CubeArray),
        base_mip_level: 0,
        mip_level_count: Some(1),
        base_array_layer: 0,
        array_layer_count: Some(layer_count),
        aspect: wgpu::TextureAspect::DepthOnly,
        usage: Some(wgpu::TextureUsages::TEXTURE_BINDING),
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("JKA local-light shadow comparison sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        compare: Some(wgpu::CompareFunction::LessEqual),
        ..Default::default()
    });

    let identity = ShadowCasterUniform {
        view_proj: Mat4::IDENTITY.to_cols_array_2d(),
        camera_pos_time: [0.0; 4],
    };
    let mut layer_views = Vec::with_capacity(LOCAL_SHADOW_CACHE_SLOTS * 6);
    let mut caster_buffers = Vec::with_capacity(LOCAL_SHADOW_CACHE_SLOTS * 6);
    let mut caster_bind_groups = Vec::with_capacity(LOCAL_SHADOW_CACHE_SLOTS * 6);
    for layer in 0..LOCAL_SHADOW_CACHE_SLOTS * 12 {
        layer_views.push(texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("JKA local-light cached shadow face"),
            format: Some(DEPTH_FORMAT),
            dimension: Some(wgpu::TextureViewDimension::D2),
            base_mip_level: 0,
            mip_level_count: Some(1),
            base_array_layer: u32::try_from(layer).unwrap_or(0),
            array_layer_count: Some(1),
            aspect: wgpu::TextureAspect::DepthOnly,
            usage: Some(wgpu::TextureUsages::RENDER_ATTACHMENT),
        }));
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("JKA local-light cached shadow caster uniform"),
            contents: bytemuck::bytes_of(&identity),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        caster_bind_groups.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("JKA local-light cached shadow caster bind group"),
            layout: caster_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        }));
        caster_buffers.push(buffer);
    }

    LocalShadowResources {
        moving_face_masks: vec![0; LOCAL_SHADOW_CACHE_SLOTS],
        moving_octant_masks: vec![0; LOCAL_SHADOW_CACHE_SLOTS],
        ab_last_cpu_ms: 0.0,
        ab_last_dynamic_faces: 0,
        ab_last_entity_face_pairs: 0,
        ab_last_bsp_face_pairs: 0,
        ab_last_static_rebuilds: 0,
        moving_cameras: None,
        _texture: texture,
        cube_view,
        sampler,
        layer_views,
        caster_buffers,
        caster_bind_groups,
        cache_entries: vec![LocalShadowCacheEntry::default(); LOCAL_SHADOW_CACHE_SLOTS],
        active_light_indices: Vec::new(),
        pending_slots: Vec::new(),
        shadowed_light_count: 0,
        selection_cluster: None,
        selection_position: None,
        selection_serial: 0,
        glass_buffer: device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA raster RGB glass caster slots"),
            size: ((LOCAL_SHADOW_CACHE_SLOTS + crate::renderer::lighting::glass_transmission::RASTER_SUN_GLASS_SLOTS)
                * std::mem::size_of::<RasterGlassSlot>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }),
    }
}

pub(in crate::renderer) fn aabb_intersects_sphere(
    minimum: [f32; 3],
    maximum: [f32; 3],
    center: [f32; 3],
    radius: f32,
) -> bool {
    let mut squared_distance = 0.0_f32;
    for axis in 0..3 {
        let value = center[axis];
        let nearest = value.clamp(minimum[axis], maximum[axis]);
        let delta = value - nearest;
        squared_distance += delta * delta;
    }
    squared_distance <= radius * radius
}

/// True when every point of the box is more than one unit on the negative
/// side of `plane` (n·p + d < 0), i.e. fully discarded by a clip plane.
pub(in crate::renderer) fn aabb_behind_plane(
    minimum: [f32; 3],
    maximum: [f32; 3],
    plane: [f32; 4],
) -> bool {
    // The box corner furthest along the plane normal.
    let farthest: f32 = (0..3)
        .map(|axis| {
            plane[axis]
                * if plane[axis] >= 0.0 {
                    maximum[axis]
                } else {
                    minimum[axis]
                }
        })
        .sum();
    farthest + plane[3] < -1.0
}

pub(in crate::renderer) fn aabb_intersects_clip_frustum(
    minimum: [f32; 3],
    maximum: [f32; 3],
    view_proj: Mat4,
) -> bool {
    let corners = [
        Vec3::new(minimum[0], minimum[1], minimum[2]),
        Vec3::new(maximum[0], minimum[1], minimum[2]),
        Vec3::new(minimum[0], maximum[1], minimum[2]),
        Vec3::new(maximum[0], maximum[1], minimum[2]),
        Vec3::new(minimum[0], minimum[1], maximum[2]),
        Vec3::new(maximum[0], minimum[1], maximum[2]),
        Vec3::new(minimum[0], maximum[1], maximum[2]),
        Vec3::new(maximum[0], maximum[1], maximum[2]),
    ];
    let clip = corners.map(|corner| view_proj * corner.extend(1.0));

    let outside_left = clip.iter().all(|point| point.x < -point.w);
    let outside_right = clip.iter().all(|point| point.x > point.w);
    let outside_bottom = clip.iter().all(|point| point.y < -point.w);
    let outside_top = clip.iter().all(|point| point.y > point.w);
    // glam's *_rh projection helpers use wgpu/D3D depth convention: z in [0, w].
    let outside_near = clip.iter().all(|point| point.z < 0.0);
    let outside_far = clip.iter().all(|point| point.z > point.w);

    !(outside_left || outside_right || outside_bottom || outside_top || outside_near || outside_far)
}

/// Pixel-space bounding rectangle of a world AABB, as `(x, y, width, height)`.
///
/// `None` means "cannot be bounded, use the whole target": any corner at or
/// behind the near plane makes the perspective divide meaningless, and a box
/// straddling the eye projects to the entire screen anyway. Callers use this to
/// restrict full-screen work to the part of the framebuffer an object can
/// affect, so an unbounded answer has to be the conservative one.
pub(in crate::renderer) fn aabb_screen_rect(
    minimum: [f32; 3],
    maximum: [f32; 3],
    view_proj: Mat4,
    width: u32,
    height: u32,
) -> Option<(f32, f32, f32, f32)> {
    let corners = [
        Vec3::new(minimum[0], minimum[1], minimum[2]),
        Vec3::new(maximum[0], minimum[1], minimum[2]),
        Vec3::new(minimum[0], maximum[1], minimum[2]),
        Vec3::new(maximum[0], maximum[1], minimum[2]),
        Vec3::new(minimum[0], minimum[1], maximum[2]),
        Vec3::new(maximum[0], minimum[1], maximum[2]),
        Vec3::new(minimum[0], maximum[1], maximum[2]),
        Vec3::new(maximum[0], maximum[1], maximum[2]),
    ];
    let (width, height) = (width as f32, height as f32);
    let mut lo = [f32::INFINITY; 2];
    let mut hi = [f32::NEG_INFINITY; 2];
    for corner in corners {
        let clip = view_proj * corner.extend(1.0);
        if clip.w <= 1.0e-4 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        // wgpu NDC has +y up; the framebuffer has +y down.
        let x = (ndc.x * 0.5 + 0.5) * width;
        let y = (0.5 - ndc.y * 0.5) * height;
        lo[0] = lo[0].min(x);
        lo[1] = lo[1].min(y);
        hi[0] = hi[0].max(x);
        hi[1] = hi[1].max(y);
    }
    (lo[0].is_finite() && lo[1].is_finite()).then_some((lo[0], lo[1], hi[0], hi[1]))
}

/// Culls a caster against a directional cascade. Only the sides of the box are
/// tested against the sun-facing depth range: anything between the sun and the
/// cascade can still shadow it (the caster pipelines run with unclipped depth),
/// so rejecting on the sunward plane deleted roofs whenever the view frustum's
/// light-space extent happened to end below them. Only the plane beyond the
/// cascade, away from the sun, may cull. `reverse_z` selects which clip-z end
/// that is (Bevy cascades are reverse-Z, the legacy ones are not).
pub(in crate::renderer) fn aabb_intersects_shadow_frustum(
    minimum: [f32; 3],
    maximum: [f32; 3],
    view_proj: Mat4,
    reverse_z: bool,
) -> bool {
    let corners = [
        Vec3::new(minimum[0], minimum[1], minimum[2]),
        Vec3::new(maximum[0], minimum[1], minimum[2]),
        Vec3::new(minimum[0], maximum[1], minimum[2]),
        Vec3::new(maximum[0], maximum[1], minimum[2]),
        Vec3::new(minimum[0], minimum[1], maximum[2]),
        Vec3::new(maximum[0], minimum[1], maximum[2]),
        Vec3::new(minimum[0], maximum[1], maximum[2]),
        Vec3::new(maximum[0], maximum[1], maximum[2]),
    ];
    let clip = corners.map(|corner| view_proj * corner.extend(1.0));

    let outside_left = clip.iter().all(|point| point.x < -point.w);
    let outside_right = clip.iter().all(|point| point.x > point.w);
    let outside_bottom = clip.iter().all(|point| point.y < -point.w);
    let outside_top = clip.iter().all(|point| point.y > point.w);
    let beyond_cascade = if reverse_z {
        clip.iter().all(|point| point.z < 0.0)
    } else {
        clip.iter().all(|point| point.z > point.w)
    };

    !(outside_left || outside_right || outside_bottom || outside_top || beyond_cascade)
}

impl Renderer {
    pub(in crate::renderer) fn baked_local_moving_casters(&self) -> bool {
        self.baked_local_shadows &&
            (self.dynamic_model_renderer.has_depth_casters() || self.world.as_ref().is_some_and(|w|w.coarse_batches.iter().any(|b|((self.draw_inline_models && b.is_inline_entity) || super::lightmap_sun::moving_static_material(&b.source,crate::renderer::SunVisibilityMode::Filtered)) && !hidden_inline_batch(b,&w.inline_models))))
    }
    pub(in crate::renderer) fn encode_baked_local_moving_shadows(&mut self, encoder: &mut wgpu::CommandEncoder, models: &[crate::renderer::DynamicModelSurface]) {
        let moving=self.baked_local_moving_casters();
        if self.baked_local_had_casters != moving { self.baked_local_had_casters=moving;self.update_lighting_settings(); }
        // Original-lighting correction uses the moving cube independently of
        // the renderer's optional RT path for added direct illumination.
        if !moving {
            if let Some(world) = &mut self.world {
                world.local_shadows.ab_last_dynamic_faces = 0;
                world.local_shadows.ab_last_entity_face_pairs = 0;
                world.local_shadows.ab_last_bsp_face_pairs = 0;
                world.local_shadows.ab_last_cpu_ms = 0.0;
            }
            return;
        }
        let cpu_started = std::time::Instant::now();
        self.gpu_profiler.write_encoder_timestamp(encoder, GpuPass::LocalShadowMoving, false);
        let unclipped=self.entity_shadow_unclipped_depth();
        self.dynamic_model_renderer.ensure_entity_shadow_pipelines(&mut self.pipeline_jobs,&self.device,false,unclipped);
        let mut bounds = std::collections::HashMap::<u16,(Vec3,Vec3)>::new();
        for model in models.iter().filter(|m| m.raster_visible && matches!(m.alpha_mode,
            crate::renderer::DynamicModelAlphaMode::Opaque | crate::renderer::DynamicModelAlphaMode::Mask)) {
            let (min,max) = if let Some(skin) = model.ghoul2_gpu.as_ref() {
                self.dynamic_model_renderer.shadow_skin_sphere(skin).map_or(
                    (Vec3::splat(-1e20),Vec3::splat(1e20)),
                    |(p,r)| (p-Vec3::splat(r),p+Vec3::splat(r)))
            } else {
                model.vertices.iter().fold((Vec3::splat(f32::INFINITY),Vec3::splat(f32::NEG_INFINITY)),
                    |(min,max),v| { let p=Vec3::from_array(v.position);(min.min(p),max.max(p)) })
            };
            let entry = bounds.entry(model.entity_num).or_insert((min,max));
            entry.0 = entry.0.min(min); entry.1 = entry.1.max(max);
        }
        let Some(world)=self.world.as_mut() else {return;};
        let mut masks_changed=false;
        let mut rendered_faces=0u32;
        let mut entity_face_pairs=0u32;
        let mut bsp_face_pairs=0u32;
        let moving_batches: Vec<_> = world.coarse_batches.iter().filter(|batch|
            (batch.is_inline_entity && self.draw_inline_models || super::lightmap_sun::moving_static_material(&batch.source,crate::renderer::SunVisibilityMode::Filtered))
                && !hidden_inline_batch(batch,&world.inline_models)
                && (batch.source.pipeline.class==DrawClass::Opaque || batch.source.alpha_cutoff!=0.0)).collect();
        let cameras=world.local_shadows.moving_cameras.get_or_insert_with(||
            crate::renderer::lighting::entity_shadows::EntityShadowState::with_count(&self.device,&self.camera_layout,LOCAL_SHADOW_CACHE_SLOTS*6));
        for &light_index in &world.local_shadows.active_light_indices {
            let Some(slot)=world.local_shadows.cache_entries.iter().position(|e|e.light_index==Some(light_index)) else {continue;};
            let light=world.dynamic_lights[light_index];
            let matrices=local_shadow_face_matrices(Vec3::from_array(light.position),light.radius);
            let previous_mask=world.local_shadows.moving_face_masks[slot];
            let previous_octants=world.local_shadows.moving_octant_masks[slot];
            let caster_prefilter = (self.lamp_shadow_ab & 2) != 0;
            // C variant: evaluate each model / mover's lamp sphere intersection
            // once instead of six times. No temporal caching or reduced Hz.
            let near_entities = caster_prefilter.then(|| bounds.iter().filter(|(_, (min, max))|
                min.is_finite() && aabb_intersects_sphere(min.to_array(), max.to_array(), light.position, light.radius)
            ).map(|(&id, &(min,max))| (id,min,max)).collect::<Vec<_>>());
            let near_batches = caster_prefilter.then(|| moving_batches.iter().copied().filter(|batch|
                aabb_intersects_sphere(batch.bounds_min,batch.bounds_max,light.position,light.radius)
            ).collect::<Vec<_>>());
            let mut octants=0u32;
            if (self.lamp_shadow_ab & 1) != 0 || self.lamp_shadow_ab == 4 {
                if let Some(ref entities) = near_entities {
                    for &(_,min,max) in entities {octants |= moving_caster_octants(min.to_array(),max.to_array(),light.position,light.radius);}
                } else {
                    for (_, (min,max)) in &bounds {
                        if min.is_finite() && aabb_intersects_sphere(min.to_array(),max.to_array(),light.position,light.radius) {
                            octants |= moving_caster_octants(min.to_array(),max.to_array(),light.position,light.radius);
                        }
                    }
                }
                if let Some(ref batches) = near_batches {
                    for batch in batches {octants |= moving_caster_octants(batch.bounds_min,batch.bounds_max,light.position,light.radius);}
                } else {
                    for batch in &moving_batches {
                        if aabb_intersects_sphere(batch.bounds_min,batch.bounds_max,light.position,light.radius) {
                            octants |= moving_caster_octants(batch.bounds_min,batch.bounds_max,light.position,light.radius);
                        }
                    }
                }
            }
            // If every octant is occupied, the receiver mask cannot reject
            // anything: encode zero to retain the legacy fast sampling path.
            if octants == 0xff { octants = 0; }
            let mut face_mask=0;
            for (face,matrix) in matrices.into_iter().enumerate() {
                let entities: std::collections::HashSet<_> = if let Some(ref near) = near_entities {
                    near.iter().filter_map(|&(id,min,max)|
                        aabb_intersects_clip_frustum(min.to_array(),max.to_array(),matrix).then_some(id)).collect()
                } else {
                    bounds.iter().filter_map(|(&id,(min,max))|
                        (min.is_finite() && aabb_intersects_sphere(min.to_array(),max.to_array(),light.position,light.radius)
                            && aabb_intersects_clip_frustum(min.to_array(),max.to_array(),matrix)).then_some(id)).collect()
                };
                let batches: Vec<_> = if let Some(ref near) = near_batches {
                    near.iter().copied().filter(|batch|
                        aabb_intersects_clip_frustum(batch.bounds_min,batch.bounds_max,matrix)).collect()
                } else {
                    moving_batches.iter().copied().filter(|batch|
                        aabb_intersects_sphere(batch.bounds_min,batch.bounds_max,light.position,light.radius)
                            && aabb_intersects_clip_frustum(batch.bounds_min,batch.bounds_max,matrix)).collect()
                };
                let has_casters = !entities.is_empty() || !batches.is_empty();
                if has_casters { face_mask |= 1 << face; }
                // Empty faces need clearing only when a previous caster leaves.
                if !has_casters && previous_mask & (1 << face) == 0 {continue;}
                rendered_faces += 1;
                entity_face_pairs += entities.len() as u32;
                bsp_face_pairs += batches.len() as u32;
                let face_index=slot*6+face;
                cameras.write_camera(&self.queue,face_index,matrix);
                self.queue.write_buffer(&world.local_shadows.caster_buffers[face_index+LOCAL_SHADOW_CACHE_SLOTS*6],0,
                    bytemuck::bytes_of(&ShadowCasterUniform { view_proj:matrix.to_cols_array_2d(),camera_pos_time:[0.0;4] }));
                let mut pass=encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label:Some("baked local light moving casters"),color_attachments:&[],
                    depth_stencil_attachment:Some(wgpu::RenderPassDepthStencilAttachment {
                        view:&world.local_shadows.layer_views[face_index+LOCAL_SHADOW_CACHE_SLOTS*6],
                        depth_ops:Some(wgpu::Operations {load:wgpu::LoadOp::Clear(1.0),store:wgpu::StoreOp::Store}),stencil_ops:None,
                    }),timestamp_writes:None,occlusion_query_set:None,multiview_mask:None,
                });
                if !batches.is_empty() {
                    pass.set_pipeline(&self.shadow_resources.pipeline);
                    pass.set_bind_group(0,&world.local_shadows.caster_bind_groups[face_index+LOCAL_SHADOW_CACHE_SLOTS*6],&[]);
                    pass.set_vertex_buffer(0,world.vertex_buffer.slice(..));
                    pass.set_vertex_buffer(1,world.legacy_dlight_surface_id_buffer.slice(..));
                    pass.set_index_buffer(world.index_buffer.slice(..),wgpu::IndexFormat::Uint32);
                    for batch in batches {
                        if batch.source.pipeline.class==DrawClass::Opaque && batch.source.alpha_cutoff==0.0 {
                            pass.set_pipeline(&self.shadow_resources.pipeline);
                        } else if batch.source.alpha_cutoff!=0.0 {
                            pass.set_pipeline(&self.shadow_resources.mask_pipeline);pass.set_bind_group(1,&batch.bind_group,&[]);
                        } else {continue;}
                        crate::renderer::draw_world_batch_secondary(&mut pass,batch,0..1,None);
                    }
                }
                if !entities.is_empty() { self.dynamic_model_renderer.draw_local_entity_shadow(&mut pass,&cameras.cameras[face_index].1,&entities); }
            }
            masks_changed |= previous_mask != face_mask || previous_octants != octants;
            world.local_shadows.moving_face_masks[slot]=face_mask;
            world.local_shadows.moving_octant_masks[slot]=octants;
        }
        world.local_shadows.ab_last_dynamic_faces = rendered_faces;
        world.local_shadows.ab_last_entity_face_pairs = entity_face_pairs;
        world.local_shadows.ab_last_bsp_face_pairs = bsp_face_pairs;
        if masks_changed { self.upload_light_buffer(); }
        self.gpu_profiler.write_encoder_timestamp(encoder, GpuPass::LocalShadowMoving, true);
        // Times only the CPU encoding, not GPU execution. GPU timestamps are
        // exported separately as local_shadow_moving in perfcapture.
        let elapsed_ms = cpu_started.elapsed().as_secs_f64() * 1000.0;
        if let Some(world) = &mut self.world {
            world.local_shadows.ab_last_cpu_ms = elapsed_ms;
        }
    }
}
