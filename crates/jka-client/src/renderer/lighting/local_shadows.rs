//! Lighting local shadows.
use crate::renderer::{
    draw_world_batch, scene, BlendMode, Camera, DrawClass, DynamicLightsMode, DynamicShadowsMode,
    GpuPointLight, LocalShadowCacheEntry, LocalShadowResources, Mat4, Renderer,
    ShadowCasterUniform, Vec3, WorldGpu, DEPTH_FORMAT, LOCAL_SHADOW_CACHE_SLOTS,
    LOCAL_SHADOW_HYSTERESIS, LOCAL_SHADOW_MAP_SIZE, LOCAL_SHADOW_NEAR,
    LOCAL_SHADOW_RESELECT_DISTANCE, MAX_LOCAL_SHADOW_LIGHTS,
};
use bytemuck::Zeroable;
use wgpu::util::DeviceExt;

impl Renderer {
    pub(in crate::renderer) fn ray_traced_local_shadows_active(&self) -> bool {
        self.world.as_ref().is_some_and(|world| {
            world.active_pipeline_variant.ray_traced_shadows
                && world.active_pipeline_variant.local_shadows
        })
    }

    pub(in crate::renderer) fn hardware_rt_requested(&self) -> bool {
        self.cascaded_shadow_mode == DynamicShadowsMode::RayTraced
            || self.dynamic_lights_mode == DynamicLightsMode::RayTracedHardware
    }

    pub(in crate::renderer) fn hardware_rt_active(&self) -> bool {
        self.world
            .as_ref()
            .is_some_and(|world| world.active_pipeline_variant.ray_traced_shadows)
    }

    pub(in crate::renderer) fn update_local_shadow_selection(&mut self, camera: &Camera) {
        // RT queries cover all existing clustered lights, including transient
        // lights with no cubemap slot. Keep the raster cache intact for toggles.
        if self.ray_traced_local_shadows_active() {
            return;
        }
        let direct_local_lighting_enabled =
            self.static_point_lighting_enabled() || self.emissive_area_lights_enabled;
        if !direct_local_lighting_enabled || !self.local_light_shadows_enabled {
            return;
        }

        let should_reselect = self.world.as_ref().is_some_and(|world| {
            if world.light_count == 0 {
                return false;
            }
            let shadows = &world.local_shadows;
            let cluster_changed = shadows.selection_cluster != world.last_cluster;
            let moved = shadows.selection_position.is_none_or(|position| {
                position.distance_squared(camera.position)
                    >= LOCAL_SHADOW_RESELECT_DISTANCE * LOCAL_SHADOW_RESELECT_DISTANCE
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
                camera,
                self.static_point_lighting_enabled(),
                self.emissive_area_lights_enabled,
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
            shadows.selection_position = Some(camera.position);

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
        if self.ray_traced_local_shadows_active() {
            return;
        }
        let direct_local_lighting_enabled =
            self.static_point_lighting_enabled() || self.emissive_area_lights_enabled;
        if !direct_local_lighting_enabled || !self.local_light_shadows_enabled {
            return;
        }
        let pending_slots = self
            .world
            .as_ref()
            .map(|world| world.local_shadows.pending_slots.clone())
            .unwrap_or_default();
        if pending_slots.is_empty() {
            return;
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("JKA local-light shadow cache encoder"),
            });
        if let Some(world) = &self.world {
            for &slot in &pending_slots {
                let Some(light_index) = world.local_shadows.cache_entries[slot].light_index else {
                    continue;
                };
                let light = world.dynamic_lights[light_index];
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
                            DrawClass::Opaque if batch.source.alpha_cutoff <= 0.0 => {
                                if mask_pipeline_active {
                                    pass.set_pipeline(&self.shadow_resources.pipeline);
                                    mask_pipeline_active = false;
                                }
                                draw_world_batch(&mut pass, batch, 0..1, None);
                            }
                            DrawClass::Mask if batch.source.alpha_cutoff > 0.0 => {
                                if !mask_pipeline_active {
                                    pass.set_pipeline(&self.shadow_resources.mask_pipeline);
                                    mask_pipeline_active = true;
                                }
                                pass.set_bind_group(1, &batch.bind_group, &[]);
                                draw_world_batch(&mut pass, batch, 0..1, None);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
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

    let distance = Vec3::from_array(light.position)
        .distance(camera_position)
        .max(LOCAL_SHADOW_NEAR);
    let radius = light.radius.max(LOCAL_SHADOW_NEAR);
    let outside_distance = (distance - radius).max(0.0);
    let projected_influence = (radius / distance.max(radius * 0.25)).clamp(0.0, 4.0);
    let proximity = 1.0 / (1.0 + outside_distance / radius);
    let energy = light.intensity.max(0.0).sqrt();
    let emitter_normal = Vec3::from_array(light.emitter_normal);
    let emitter_relevance = if emitter_normal.length_squared() > 1e-6 {
        let to_camera = (camera_position - Vec3::from_array(light.position)).normalize_or_zero();
        let facing = emitter_normal.normalize().dot(to_camera);
        if light.emitter_two_sided {
            facing.abs()
        } else {
            facing.max(0.0)
        }
    } else {
        1.0
    };
    let hysteresis = if already_active {
        LOCAL_SHADOW_HYSTERESIS
    } else {
        1.0
    };
    energy * projected_influence * projected_influence * proximity * emitter_relevance * hysteresis
}

pub(in crate::renderer) fn select_relevant_shadow_lights(
    world: &WorldGpu,
    camera: &Camera,
    include_point_lights: bool,
    include_area_lights: bool,
) -> Vec<usize> {
    let mut candidates = world
        .dynamic_lights
        .iter()
        .copied()
        .enumerate()
        .filter_map(|(index, light)| {
            if !light.surface_lighting {
                return None;
            }
            let is_area = light.emitter_normal.iter().any(|value| value.abs() > 1e-6);
            if is_area && !include_area_lights {
                return None;
            }
            if !is_area && !include_point_lights {
                return None;
            }
            // A single cubemap sample cannot correctly shadow both hemispheres
            // of a two-sided area emitter. One-sided q3map emitters are offset
            // from their source plane and can use the normal local-shadow cache.
            if light.emitter_two_sided
                && light.emitter_normal.iter().any(|value| value.abs() > 1e-6)
            {
                return None;
            }
            if !local_light_is_pvs_relevant(world, light) {
                return None;
            }
            let score = local_shadow_importance(
                light,
                camera.position,
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

pub(in crate::renderer) fn local_shadow_face_matrices(position: Vec3, radius: f32) -> [Mat4; 6] {
    let far_plane = radius.max(LOCAL_SHADOW_NEAR + 1.0);
    let projection = Mat4::perspective_rh(
        std::f32::consts::FRAC_PI_2,
        1.0,
        LOCAL_SHADOW_NEAR,
        far_plane,
    );
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

pub(in crate::renderer) fn create_local_shadow_resources(
    device: &wgpu::Device,
    caster_layout: &wgpu::BindGroupLayout,
) -> LocalShadowResources {
    let layer_count = u32::try_from(LOCAL_SHADOW_CACHE_SLOTS * 6).unwrap_or(6);
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
    for layer in 0..LOCAL_SHADOW_CACHE_SLOTS * 6 {
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
