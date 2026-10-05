//! Lighting entity shadows.
use crate::renderer::{
    sample_entity_classic_light, scene, Camera, CameraUniform, DynamicShadowsMode,
    EntityShadowLight, Mat4, Renderer, ShadowCasterUniform, ShadowReceiverUniform, Vec3,
    BEVY_CSM_OVERLAP_PROPORTION, BEVY_CSM_SHADOW_DEPTH_BIAS, BEVY_CSM_SHADOW_NORMAL_BIAS,
    ENTITY_SHADOW_CAMERAS, ENTITY_SHADOW_LIGHT_RANGE, ENTITY_SHADOW_MAX_STRENGTH,
    ENTITY_SHADOW_MIN_ELEVATION, ENTITY_SHADOW_RADIUS, ENTITY_SHADOW_SMOOTHING_SECONDS,
    RT_SUN_ANGULAR_DIAMETER_RADIANS, SHADOW_CASCADES, SHADOW_MAP_SIZE,
};

/// Entity shadow state: per-cascade light-camera uniforms (the entity shaders take
/// the light's view-projection through the ordinary camera slot) plus the smoothed
/// light the Entity map is aimed with.
pub(in crate::renderer) struct EntityShadowState {
    pub(in crate::renderer) cameras: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    /// Smoothed unit direction the light travels, renderer coordinates.
    pub(in crate::renderer) direction: Vec3,
    /// Smoothed 0..1 fraction of `ENTITY_SHADOW_MAX_STRENGTH`.
    pub(in crate::renderer) strength: f32,
    pub(in crate::renderer) initialized: bool,
}

impl EntityShadowState {
    pub(in crate::renderer) fn new(
        device: &wgpu::Device,
        camera_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let cameras = (0..ENTITY_SHADOW_CAMERAS)
            .map(|_| {
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("JKA entity shadow light camera"),
                    size: std::mem::size_of::<CameraUniform>() as wgpu::BufferAddress,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("JKA entity shadow light camera bind group"),
                    layout: camera_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    }],
                });
                (buffer, bind_group)
            })
            .collect();
        Self {
            cameras,
            direction: Vec3::NEG_Y,
            strength: 0.0,
            initialized: false,
        }
    }

    /// Stores `view_proj` as light camera `index`; entity shaders read only that.
    pub(in crate::renderer) fn write_camera(
        &self,
        queue: &wgpu::Queue,
        index: usize,
        view_proj: Mat4,
    ) {
        let light_camera = CameraUniform {
            view_proj: view_proj.to_cols_array_2d(),
            camera_pos_time: [0.0; 4],
            clip_plane: [0.0; 4],
            render_flags: [0; 4],
            camera_forward: [0.0, 0.0, -1.0, 0.0],
            unjittered_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
            previous_unjittered_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
            jump_shade: [0.0; 4],
        };
        queue.write_buffer(&self.cameras[index].0, 0, bytemuck::bytes_of(&light_camera));
    }
}

/// What the Entity map needs for one frame, produced before command encoding.
pub(in crate::renderer) struct EntityShadowFrame {
    pub(in crate::renderer) view_proj: Mat4,
    /// False when the baked light here is too weak/ambient for a visible shadow:
    /// receivers are told shadows are off and the pass is skipped.
    pub(in crate::renderer) active: bool,
}

impl Renderer {
    /// Direction the Entity map's light travels and how visible its shadow
    /// should be, taken from the baked lightgrid at the local player (camera
    /// when there is none). The lightgrid stores one dominant light direction
    /// per probe, which is exactly what the original renderer aimed entity
    /// shadows with. With entity sun lighting on, the runtime sun competes with
    /// the remaining baked light and the brighter one wins.
    ///
    /// With `EntityShadowLight::Authored`, a non-sun dominant light is replaced by
    /// the direction to the best authored map light (see
    /// `authored_light_direction`), keeping the lightgrid's strength estimate.
    /// Maps with no lightgrid cast a sun shadow only if the map has a sun.
    pub(in crate::renderer) fn entity_shadow_target(
        &self,
        camera: &Camera,
        player_position: Option<Vec3>,
    ) -> (Vec3, f32) {
        const STANDING_VIEW_HEIGHT: f32 = 36.0;
        let focus = player_position
            .map(|origin| origin + Vec3::Y * STANDING_VIEW_HEIGHT)
            .unwrap_or(camera.position);
        let luma = |rgb: [f32; 3]| rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722;
        let smoothstep = |edge0: f32, edge1: f32, value: f32| {
            let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        let sampled = self
            .world
            .as_ref()
            .and_then(|world| world.entity_light_grid.as_ref())
            .and_then(|grid| {
                sample_entity_classic_light(
                    grid,
                    scene::jka_position(focus.to_array()),
                    self.entity_sun_relight(),
                )
            });
        let from_grid = sampled.and_then(|light| {
            let sun_dominant = luma(light.sun_directed) > luma(light.directed);
            let (toward, directed) = if sun_dominant {
                (light.sun_direction, light.sun_directed)
            } else {
                (light.direction, light.directed)
            };
            let mut toward = Vec3::from_array(toward).try_normalize()?;
            let directed_luma = luma(directed);
            let ambient = luma(light.ambient);
            // Light units are 0..255. Contrast is how much of the local light is
            // directional (a shadow needs something to be missing); presence fades
            // the shadow out in near-black probes where it would be invisible;
            // coherence fades it where neighbouring probes disagree on direction.
            let contrast = directed_luma / (directed_luma + ambient).max(1.0);
            let coherence = if sun_dominant {
                1.0
            } else {
                smoothstep(0.35, 0.8, light.direction_coherence)
            };
            let strength =
                smoothstep(0.15, 0.6, contrast) * smoothstep(2.0, 16.0, directed_luma) * coherence;
            // A baked sun share at least half of the directed light means the
            // direction is already the exact sun direction: nothing to refine.
            let sun_like =
                sun_dominant || luma(light.baked_sun) >= 0.5 * luma(light.directed).max(1.0e-3);
            if self.entity_shadow_light == EntityShadowLight::Authored && !sun_like {
                if let Some(authored) = self.authored_light_direction(focus, Some(toward)) {
                    toward = authored;
                }
            }
            Some((toward, strength))
        });
        let from_world = || {
            // No lightgrid sample: only a map sun gives a believable direction, and
            // with `Authored` the best authored light can still be used.
            let world = self.world.as_ref()?;
            if self.entity_shadow_light == EntityShadowLight::Authored {
                if let Some(authored) = self.authored_light_direction(focus, None) {
                    return Some((authored, 0.6));
                }
            }
            world.sun?;
            let toward = -Vec3::from_array(self.active_sun().direction);
            Some((toward.try_normalize().unwrap_or(Vec3::Y), 0.6))
        };
        let Some((mut toward_light, strength)) = from_grid.or_else(from_world) else {
            return (self.entity_shadow.direction, 0.0);
        };
        if toward_light.y < ENTITY_SHADOW_MIN_ELEVATION {
            toward_light.y = ENTITY_SHADOW_MIN_ELEVATION;
            toward_light = toward_light.normalize();
        }
        (-toward_light, strength)
    }

    /// Unit direction from `focus` toward the authored map light most likely to be
    /// lighting it. Score is irradiance at the focus (brightness over distance
    /// squared). The map stores no light visibility, so when a lightgrid direction
    /// is available it stands in for one: q3map2 traced occlusion when it baked
    /// that direction, so a lamp only qualifies if it lies within about 60 degrees
    /// of it, which rejects lamps in other rooms.
    pub(in crate::renderer) fn authored_light_direction(
        &self,
        focus: Vec3,
        grid_toward: Option<Vec3>,
    ) -> Option<Vec3> {
        let world = self.world.as_ref()?;
        best_authored_light_direction(&world.dynamic_lights, focus, grid_toward)
    }

    pub(in crate::renderer) fn set_entity_shadow_light(&mut self, source: EntityShadowLight) {
        self.entity_shadow_light = source;
        // Re-aim immediately instead of easing from the previous source's light.
        self.entity_shadow.initialized = false;
    }

    /// Smooths the light, builds the texel-snapped orthographic light matrix
    /// around the view, and reports whether a visible shadow is wanted at all.
    pub(in crate::renderer) fn prepare_entity_shadow(
        &mut self,
        camera: &Camera,
        player_position: Option<Vec3>,
        frame_delta: f32,
    ) -> EntityShadowFrame {
        let (target_direction, target_strength) =
            self.entity_shadow_target(camera, player_position);
        let state = &mut self.entity_shadow;
        if state.initialized {
            let alpha = 1.0 - (-frame_delta / ENTITY_SHADOW_SMOOTHING_SECONDS).exp();
            state.direction = state
                .direction
                .lerp(target_direction, alpha)
                .try_normalize()
                .unwrap_or(target_direction);
            state.strength += (target_strength - state.strength) * alpha;
        } else {
            state.direction = target_direction;
            state.strength = target_strength;
            state.initialized = true;
        }
        let direction = state.direction;
        let reference_up = if direction.dot(Vec3::Y).abs() > 0.95 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        let right = direction.cross(reference_up).normalize_or_zero();
        let light_up = right.cross(direction).normalize_or_zero();
        let radius = ENTITY_SHADOW_RADIUS;
        // Bias the window toward where the camera is looking; characters behind
        // the viewer cast onto nothing visible.
        let centre = camera.position + camera.forward() * (radius * 0.45);
        let texel_size = (2.0 * radius) / SHADOW_MAP_SIZE as f32;
        let centre_right = centre.dot(right);
        let centre_up = centre.dot(light_up);
        let snapped_centre = centre
            + right * ((centre_right / texel_size).round() * texel_size - centre_right)
            + light_up * ((centre_up / texel_size).round() * texel_size - centre_up);
        let eye = snapped_centre - direction * (radius * 2.0);
        let view = Mat4::look_at_rh(eye, snapped_centre, reference_up);
        let projection = Mat4::orthographic_rh(-radius, radius, -radius, radius, 0.1, radius * 4.0);
        EntityShadowFrame {
            view_proj: projection * view,
            active: state.strength > 0.02,
        }
    }

    /// Receiver uniform for the Entity map: one cascade (layer 0) that every
    /// receiver treats as the only slice, with sky admission off because the map
    /// holds no world geometry. `enabled` is false when there is nothing to cast,
    /// which turns every receiver's 9-tap lookup into a single uniform branch.
    pub(in crate::renderer) fn write_entity_shadow_uniforms(
        &self,
        frame: &EntityShadowFrame,
        camera: &Camera,
        enabled: bool,
    ) {
        let direction = self.entity_shadow.direction;
        let forward = camera.forward();
        let texel_size = (2.0 * ENTITY_SHADOW_RADIUS) / SHADOW_MAP_SIZE as f32;
        let receiver = ShadowReceiverUniform {
            view_proj: std::array::from_fn(|index| {
                if index == 0 {
                    frame.view_proj
                } else {
                    Mat4::IDENTITY
                }
                .to_cols_array_2d()
            }),
            // Past this eye depth the window cannot cover the fragment, so the
            // receivers return lit without a matrix multiply or any taps.
            split_depths: [ENTITY_SHADOW_RADIUS * 1.6; SHADOW_CASCADES],
            light_direction_enabled: [
                direction.x,
                direction.y,
                direction.z,
                if enabled { 1.0 } else { 0.0 },
            ],
            params: [
                SHADOW_MAP_SIZE as f32,
                0.00025,
                ENTITY_SHADOW_MAX_STRENGTH * self.entity_shadow.strength,
                0.0,
            ],
            camera_forward: [forward.x, forward.y, forward.z, 0.0],
            cascade_texel_sizes: [texel_size; SHADOW_CASCADES],
            bevy_params: [
                BEVY_CSM_SHADOW_DEPTH_BIAS,
                BEVY_CSM_SHADOW_NORMAL_BIAS,
                BEVY_CSM_OVERLAP_PROPORTION,
                1.0,
            ],
        };
        self.queue.write_buffer(
            &self.shadow_resources.receiver_buffer,
            0,
            bytemuck::bytes_of(&receiver),
        );
    }

    /// Renders this frame's entities into shadow layer 0. Needs the dynamic
    /// models already prepared. Frames with nothing to cast skip the pass (the
    /// receivers are disabled for them), so there is no clear either.
    pub(in crate::renderer) fn encode_entity_shadow_pass(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        frame: &EntityShadowFrame,
    ) {
        if !frame.active || !self.dynamic_model_renderer.has_depth_casters() {
            return;
        }
        let unclipped = self.entity_shadow_unclipped_depth();
        self.dynamic_model_renderer.ensure_entity_shadow_pipelines(
            &mut self.pipeline_jobs,
            &self.device,
            false,
            unclipped,
        );
        self.entity_shadow
            .write_camera(&self.queue, 0, frame.view_proj);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("JKA entity shadow map"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.shadow_resources.layer_views[0],
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
        self.dynamic_model_renderer.draw_entity_shadow(
            &mut pass,
            &self.entity_shadow.cameras[0].1,
            false,
        );
    }

    /// Same depth-clip behaviour as the BSP cascade casters.
    pub(in crate::renderer) fn entity_shadow_unclipped_depth(&self) -> bool {
        self.device
            .features()
            .contains(wgpu::Features::DEPTH_CLIP_CONTROL)
    }

    /// Entity casters for the CSM modes: entities are also drawn into cascades 0
    /// and 1 (near and mid range; farther characters are too small to matter and
    /// every extra cascade repeats the entity draws). Writes the light cameras and
    /// builds the pipelines; returns whether the cascade loop should draw them.
    pub(in crate::renderer) fn prepare_entity_cascade_casters(
        &mut self,
        matrices: &[Mat4; SHADOW_CASCADES],
    ) -> bool {
        if !self.dynamic_model_renderer.has_depth_casters() {
            return false;
        }
        let unclipped = self.entity_shadow_unclipped_depth();
        self.dynamic_model_renderer.ensure_entity_shadow_pipelines(
            &mut self.pipeline_jobs,
            &self.device,
            true,
            unclipped,
        );
        for index in 0..ENTITY_SHADOW_CAMERAS {
            self.entity_shadow
                .write_camera(&self.queue, index, matrices[index]);
        }
        true
    }

    pub(in crate::renderer) fn write_shadow_uniforms(
        &self,
        matrices: &[Mat4; SHADOW_CASCADES],
        splits: &[f32; SHADOW_CASCADES],
        texel_sizes: &[f32; SHADOW_CASCADES],
        enabled: bool,
        camera_forward: Vec3,
        camera_pos_time: [f32; 4],
        bevy_mode: bool,
    ) {
        let sun = self.active_sun();
        let light_direction = Vec3::from_array(sun.direction).normalize_or_zero();
        let intensity_scale = (sun.intensity / 250.0).max(0.0).sqrt().clamp(0.0, 1.5);
        let receiver = ShadowReceiverUniform {
            view_proj: std::array::from_fn(|index| matrices[index].to_cols_array_2d()),
            split_depths: *splits,
            light_direction_enabled: [
                light_direction.x,
                light_direction.y,
                light_direction.z,
                if enabled { 1.0 } else { 0.0 },
            ],
            params: [
                SHADOW_MAP_SIZE as f32,
                0.00035,
                0.30 * intensity_scale,
                if bevy_mode { 1.0 } else { 0.0 },
            ],
            camera_forward: [
                camera_forward.x,
                camera_forward.y,
                camera_forward.z,
                self.sun_visibility.shader_value(),
            ],
            cascade_texel_sizes: *texel_sizes,
            bevy_params: if self.cascaded_shadow_mode == DynamicShadowsMode::RayTraced {
                // RT interpretation: only x is read, as cos(max directional-light
                // cone half-angle), matching Bevy Solari. y/z/w stay zero; w in
                // particular is the CSM cascade count other receivers (grass)
                // read, and zero makes them skip the unrendered shadow maps.
                // The visibility ray count comes from LightingSettings.
                let half_angle_radians = 0.5 * RT_SUN_ANGULAR_DIAMETER_RADIANS;
                [half_angle_radians.cos(), 0.0, 0.0, 0.0]
            } else {
                [
                    BEVY_CSM_SHADOW_DEPTH_BIAS,
                    BEVY_CSM_SHADOW_NORMAL_BIAS,
                    BEVY_CSM_OVERLAP_PROPORTION,
                    SHADOW_CASCADES as f32,
                ]
            },
        };
        self.queue.write_buffer(
            &self.shadow_resources.receiver_buffer,
            0,
            bytemuck::bytes_of(&receiver),
        );
        for (index, matrix) in matrices.iter().enumerate() {
            let caster = ShadowCasterUniform {
                view_proj: matrix.to_cols_array_2d(),
                camera_pos_time,
            };
            self.queue.write_buffer(
                &self.shadow_resources.caster_buffers[index],
                0,
                bytemuck::bytes_of(&caster),
            );
        }
    }
}

/// Picks the authored light that best explains the lighting at `focus` and returns
/// the unit direction toward it; see `Renderer::authored_light_direction`.
pub(in crate::renderer) fn best_authored_light_direction(
    lights: &[scene::DynamicLight],
    focus: Vec3,
    grid_toward: Option<Vec3>,
) -> Option<Vec3> {
    let mut best: Option<(f32, Vec3)> = None;
    for light in lights {
        let to_light = Vec3::from_array(light.position) - focus;
        let distance = to_light.length();
        if !(16.0..=ENTITY_SHADOW_LIGHT_RANGE).contains(&distance) {
            continue;
        }
        let direction = to_light / distance;
        let alignment = grid_toward.map_or(1.0, |toward| direction.dot(toward).max(0.0));
        if alignment < 0.5 {
            continue;
        }
        let brightness =
            (light.color[0] * 0.2126 + light.color[1] * 0.7152 + light.color[2] * 0.0722)
                * light.intensity;
        let score = brightness / (distance * distance).max(64.0 * 64.0) * alignment * alignment;
        if best.map_or(true, |(top, _)| score > top) {
            best = Some((score, direction));
        }
    }
    best.map(|(_, direction)| direction)
}
