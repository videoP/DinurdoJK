//! Models draw.
use crate::renderer::{
    classic_vertex_illumination, dynamic_model_writes_depth, ghoul2_specular_light,
    ghoul2_specular_viewer, rt_models, sample_entity_classic_light, scene, Arc,
    DynamicModelAlphaMode, DynamicModelRenderer, DynamicModelSurface, DynamicPreparedDraw,
    EntityAmbientLightingMode, Entry, FxGpuSpritePreparedDraw, Ghoul2BatchMode,
    Ghoul2GpuDrawUniform, Ghoul2GpuMesh, Ghoul2PendingDraw, Ghoul2PreparedDraw, GpuPass,
    GpuProfiler, PipelineJobManager, Vec3, DYNAMIC_MODEL_ALPHA_ORDER,
};
use wgpu::util::DeviceExt;

impl DynamicModelRenderer {
    pub(in crate::renderer) fn prepare(
        &mut self,
        brightness: &mut crate::renderer::brightness::BakedBrightness,
        jobs: &mut PipelineJobManager,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surfaces: &[DynamicModelSurface],
        classic_grid: Option<&scene::ClassicEntityLightGrid>,
        separate_wireframe_classes: bool,
        ray_traced_shadows: bool,
        rt_receivers: bool,
    ) {
        self.rt_prepared = false;
        if ray_traced_shadows
            && self.prepare_rt_skinned(
                brightness,
                jobs,
                device,
                queue,
                surfaces,
                classic_grid,
                rt_receivers,
            )
        {
            return;
        }
        self.draws.clear();
        self.fx_sprite_draws.clear();
        self.skin_draws.clear();
        self.frame_texture_keys.clear();
        self.ghoul2_input_instances = 0;
        self.ghoul2_encoded_draws = 0;
        if surfaces.is_empty() {
            return;
        }

        let total_vertices = surfaces
            .iter()
            .filter(|surface| surface.ghoul2_gpu.is_none() && surface.fx_gpu_sprites.is_none())
            .map(|surface| surface.vertices.len())
            .sum();
        let total_indices = surfaces
            .iter()
            .filter(|surface| surface.ghoul2_gpu.is_none() && surface.fx_gpu_sprites.is_none())
            .map(|surface| surface.indices.len())
            .sum();
        let mut vertices = std::mem::take(&mut self.scratch_vertices);
        vertices.clear();
        vertices.reserve(total_vertices);
        let mut indices = std::mem::take(&mut self.scratch_indices);
        indices.clear();
        indices.reserve(total_indices);
        let mut fx_sprite_instances = std::mem::take(&mut self.scratch_fx_sprite_instances);
        fx_sprite_instances.clear();
        let mut bones = std::mem::take(&mut self.scratch_bones);
        bones.clear();
        let mut gpu_draw_data = std::mem::take(&mut self.scratch_draw_data);
        gpu_draw_data.clear();
        gpu_draw_data.reserve(surfaces.len());
        let mut bone_offsets = std::mem::take(&mut self.scratch_bone_offsets);
        bone_offsets.clear();
        bone_offsets.reserve(surfaces.len());
        let mut skin_pending = std::mem::take(&mut self.scratch_skin_pending);
        skin_pending.clear();
        skin_pending.reserve(surfaces.len());

        for surface in surfaces {
            let texture_key =
                self.ensure_texture(brightness, device, queue, surface.texture.as_ref());
            if let Some(sprites) = surface.fx_gpu_sprites.as_ref() {
                if sprites.blob_shadow {
                    // Requests were clipped onto the rendered floor by the renderer
                    // and arrive as an ordinary mesh drawn through the CPU path.
                    if !self.blob_mesh_indices.is_empty() {
                        let base_vertex = vertices.len() as u32;
                        let first_index = indices.len() as u32;
                        vertices.extend_from_slice(self.blob_mesh_vertices.as_slice());
                        indices.extend(
                            self.blob_mesh_indices
                                .iter()
                                .map(|index| base_vertex.saturating_add(*index)),
                        );
                        self.draws.push(DynamicPreparedDraw {
                            entity_num: surface.entity_num,
                            first_index,
                            index_count: self.blob_mesh_indices.len() as u32,
                            base_vertex: 0,
                            texture_key,
                            alpha_mode: surface.alpha_mode,
                            wireframe_class: surface.wireframe_class,
                        });
                        self.blob_mesh_vertices.clear();
                        self.blob_mesh_indices.clear();
                    }
                    continue;
                }
                if sprites.instances.is_empty() {
                    continue;
                }
                let instances = sprites.instances.as_slice();
                let first_instance = u32::try_from(fx_sprite_instances.len()).unwrap_or(u32::MAX);
                fx_sprite_instances.extend_from_slice(instances);
                self.fx_sprite_draws.push(FxGpuSpritePreparedDraw {
                    entity_num: surface.entity_num,
                    first_instance,
                    instance_count: u32::try_from(instances.len()).unwrap_or(u32::MAX),
                    texture_key,
                    alpha_mode: surface.alpha_mode,
                    wireframe_class: surface.wireframe_class,
                });
                continue;
            }
            let classic_light = if self.entity_ambient_lighting
                == EntityAmbientLightingMode::BspLightgridClassic
                && !matches!(
                    surface.alpha_mode,
                    DynamicModelAlphaMode::AdditiveOne
                        | DynamicModelAlphaMode::Additive
                        | DynamicModelAlphaMode::BlendUnlit
                        | DynamicModelAlphaMode::Modulate
                        | DynamicModelAlphaMode::DstColorAdd
                        | DynamicModelAlphaMode::Modulate2x
                        | DynamicModelAlphaMode::Darken
                ) {
                match (surface.lighting_origin, classic_grid) {
                    (Some(origin), Some(grid)) => {
                        sample_entity_classic_light(grid, origin, self.entity_sun_relight)
                    }
                    _ => None,
                }
            } else {
                None
            };
            if let Some(skin) = surface.ghoul2_gpu.as_ref() {
                if skin.vertices.is_empty() || skin.indices.is_empty() || skin.bones.is_empty() {
                    continue;
                }
                let mesh_key = Arc::clone(&skin.mesh_key);
                if let Entry::Vacant(entry) = self.ghoul2_meshes.entry(Arc::clone(&mesh_key)) {
                    let vertex_buffer =
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some(&format!("JKA Ghoul2 bind-pose vertices {mesh_key}")),
                            contents: bytemuck::cast_slice(skin.vertices.as_slice()),
                            usage: wgpu::BufferUsages::VERTEX,
                        });
                    let index_buffer =
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some(&format!("JKA Ghoul2 bind-pose indices {mesh_key}")),
                            contents: bytemuck::cast_slice(skin.indices.as_slice()),
                            usage: wgpu::BufferUsages::INDEX,
                        });
                    entry.insert(Ghoul2GpuMesh {
                        vertex_buffer,
                        index_buffer,
                        index_count: u32::try_from(skin.indices.len()).unwrap_or(u32::MAX),
                    });
                }

                let pose_key = Arc::as_ptr(&skin.bones) as usize;
                let bone_base = match bone_offsets.entry(pose_key) {
                    Entry::Occupied(entry) => *entry.get(),
                    Entry::Vacant(entry) => {
                        let base = u32::try_from(bones.len()).unwrap_or(u32::MAX);
                        bones.extend_from_slice(skin.bones.as_slice());
                        entry.insert(base);
                        base
                    }
                };
                let uniform = Ghoul2GpuDrawUniform {
                    axis0: [skin.axis[0][0], skin.axis[0][1], skin.axis[0][2], 0.0],
                    axis1: [skin.axis[1][0], skin.axis[1][1], skin.axis[1][2], 0.0],
                    axis2: [skin.axis[2][0], skin.axis[2][1], skin.axis[2][2], 0.0],
                    origin: [skin.origin[0], skin.origin[1], skin.origin[2], 0.0],
                    color: skin.color,
                    params: [
                        bone_base,
                        u32::from(classic_light.is_some()),
                        0,
                        u32::from(skin.env_map),
                    ],
                    classic_ambient: classic_light
                        .map(|light| [light.ambient[0], light.ambient[1], light.ambient[2], 0.0])
                        .unwrap_or([0.0; 4]),
                    classic_directed: classic_light
                        .map(|light| [light.directed[0], light.directed[1], light.directed[2], 0.0])
                        .unwrap_or([0.0; 4]),
                    classic_direction: classic_light
                        .map(|light| {
                            [
                                light.direction[0],
                                light.direction[1],
                                light.direction[2],
                                0.0,
                            ]
                        })
                        .unwrap_or([0.0; 4]),
                    classic_sun_directed: classic_light
                        .map(|light| {
                            [
                                light.sun_directed[0],
                                light.sun_directed[1],
                                light.sun_directed[2],
                                0.0,
                            ]
                        })
                        .unwrap_or([0.0; 4]),
                    classic_sun_direction: classic_light
                        .map(|light| {
                            [
                                light.sun_direction[0],
                                light.sun_direction[1],
                                light.sun_direction[2],
                                0.0,
                            ]
                        })
                        .unwrap_or([0.0; 4]),
                    uv_xform: skin.uv_xform,
                    spec_light: ghoul2_specular_light(skin.specular),
                    spec_viewer: ghoul2_specular_viewer(skin.specular, skin.bulge_height),
                    jiggle0: skin.jiggle_offsets[0],
                    jiggle1: skin.jiggle_offsets[1],
                    jiggle2: skin.jiggle_offsets[2],
                    jiggle3: skin.jiggle_offsets[3],
                };
                skin_pending.push(Ghoul2PendingDraw {
                    entity_num: surface.entity_num,
                    mesh_key,
                    texture_key,
                    alpha_mode: surface.alpha_mode,
                    wireframe_class: surface.wireframe_class,
                    uniform,
                    sequence: u32::try_from(skin_pending.len()).unwrap_or(u32::MAX),
                    batch_hash: None,
                });
                continue;
            }

            if surface.vertices.is_empty() || surface.indices.is_empty() {
                continue;
            }
            let base_vertex = vertices.len() as u32;
            let first_index = indices.len() as u32;
            if let Some(light) = classic_light {
                for source in surface.vertices.iter() {
                    let normal = Vec3::from_array(source.normal).normalize_or_zero();
                    let illumination = classic_vertex_illumination(&light, normal);
                    let mut vertex = *source;
                    for channel in 0..3 {
                        vertex.color[channel] *= illumination[channel];
                    }
                    vertices.push(vertex);
                }
            } else {
                vertices.extend_from_slice(surface.vertices.as_slice());
            }
            indices.extend(
                surface
                    .indices
                    .iter()
                    .map(|index| base_vertex.saturating_add(*index)),
            );
            self.draws.push(DynamicPreparedDraw {
                entity_num: surface.entity_num,
                first_index,
                index_count: surface.indices.len() as u32,
                base_vertex: 0,
                texture_key,
                alpha_mode: surface.alpha_mode,
                wireframe_class: surface.wireframe_class,
            });
        }

        self.ghoul2_input_instances = u32::try_from(skin_pending.len()).unwrap_or(u32::MAX);
        let batching_active = match self.ghoul2_batch_draws {
            Ghoul2BatchMode::Off => false,
            Ghoul2BatchMode::Adaptive => Self::should_adaptively_batch(&mut skin_pending),
            Ghoul2BatchMode::Force => true,
        };
        if batching_active {
            // Continue the adaptive probe rather than restarting it: sampled
            // draws already carry their hash, and only the remaining draws are
            // hashed here.  Force mode gets the same cheaper sort key.
            for draw in &mut skin_pending {
                if matches!(
                    draw.alpha_mode,
                    DynamicModelAlphaMode::Opaque | DynamicModelAlphaMode::Mask
                ) && Some(draw.entity_num) != self.companion_excluded_entity
                    && draw.batch_hash.is_none()
                {
                    draw.batch_hash = Some(Self::ghoul2_batch_hash(draw));
                }
            }
            let companion_excluded_entity = self.companion_excluded_entity;
            skin_pending.sort_unstable_by(|a, b| {
                let batchable_a = matches!(
                    a.alpha_mode,
                    DynamicModelAlphaMode::Opaque | DynamicModelAlphaMode::Mask
                ) && Some(a.entity_num) != companion_excluded_entity;
                let batchable_b = matches!(
                    b.alpha_mode,
                    DynamicModelAlphaMode::Opaque | DynamicModelAlphaMode::Mask
                ) && Some(b.entity_num) != companion_excluded_entity;
                match (batchable_a, batchable_b) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    (false, false) => a.sequence.cmp(&b.sequence),
                    (true, true) => {
                        let alpha_a = u8::from(matches!(a.alpha_mode, DynamicModelAlphaMode::Mask));
                        let alpha_b = u8::from(matches!(b.alpha_mode, DynamicModelAlphaMode::Mask));
                        let order = alpha_a.cmp(&alpha_b);
                        let order = if separate_wireframe_classes {
                            order.then_with(|| {
                                a.wireframe_class
                                    .mask_bit()
                                    .cmp(&b.wireframe_class.mask_bit())
                            })
                        } else {
                            order
                        };
                        order
                            .then_with(|| a.batch_hash.cmp(&b.batch_hash))
                            .then_with(|| a.mesh_key.as_ref().cmp(b.mesh_key.as_ref()))
                            .then_with(|| a.texture_key.as_deref().cmp(&b.texture_key.as_deref()))
                    }
                }
            });
        }

        let mut pending_index = 0usize;
        while pending_index < skin_pending.len() {
            let pending = &skin_pending[pending_index];
            let candidate = batching_active
                && matches!(
                    pending.alpha_mode,
                    DynamicModelAlphaMode::Opaque | DynamicModelAlphaMode::Mask
                )
                && Some(pending.entity_num) != self.companion_excluded_entity;
            let mut group_end = pending_index + 1;
            if candidate {
                while group_end < skin_pending.len() {
                    let next = &skin_pending[group_end];
                    if Some(next.entity_num) == self.companion_excluded_entity
                        || next.alpha_mode != pending.alpha_mode
                        || (separate_wireframe_classes
                            && next.wireframe_class != pending.wireframe_class)
                        || next.mesh_key != pending.mesh_key
                        || next.texture_key != pending.texture_key
                    {
                        break;
                    }
                    group_end += 1;
                }
            }

            let group_len = group_end - pending_index;
            let instance_group = candidate
                && match self.ghoul2_batch_draws {
                    Ghoul2BatchMode::Off => false,
                    Ghoul2BatchMode::Adaptive => group_len >= 3,
                    Ghoul2BatchMode::Force => group_len >= 2,
                };

            if instance_group {
                let first_instance = u32::try_from(gpu_draw_data.len()).unwrap_or(u32::MAX);
                for item in &skin_pending[pending_index..group_end] {
                    gpu_draw_data.push(item.uniform);
                }
                self.skin_draws.push(Ghoul2PreparedDraw {
                    entity_num: pending.entity_num,
                    mesh_key: Arc::clone(&pending.mesh_key),
                    texture_key: pending.texture_key.as_ref().map(Arc::clone),
                    alpha_mode: pending.alpha_mode,
                    wireframe_class: pending.wireframe_class,
                    first_instance,
                    instance_count: u32::try_from(group_len).unwrap_or(u32::MAX),
                });
            } else {
                for item in &skin_pending[pending_index..group_end] {
                    let first_instance = u32::try_from(gpu_draw_data.len()).unwrap_or(u32::MAX);
                    gpu_draw_data.push(item.uniform);
                    self.skin_draws.push(Ghoul2PreparedDraw {
                        entity_num: item.entity_num,
                        mesh_key: Arc::clone(&item.mesh_key),
                        texture_key: item.texture_key.as_ref().map(Arc::clone),
                        alpha_mode: item.alpha_mode,
                        wireframe_class: item.wireframe_class,
                        first_instance,
                        instance_count: 1,
                    });
                }
            }
            pending_index = group_end;
        }
        self.ghoul2_encoded_draws = u32::try_from(self.skin_draws.len()).unwrap_or(u32::MAX);

        let vertex_bytes = bytemuck::cast_slice(vertices.as_slice());
        let index_bytes = bytemuck::cast_slice(indices.as_slice());
        self.ensure_vertex_capacity(device, vertex_bytes.len() as u64);
        self.ensure_index_capacity(device, index_bytes.len() as u64);
        if !vertex_bytes.is_empty() {
            queue.write_buffer(&self.vertex_buffer, 0, vertex_bytes);
        }
        if !index_bytes.is_empty() {
            queue.write_buffer(&self.index_buffer, 0, index_bytes);
        }

        self.ensure_model_pipelines(jobs, device);
        if !self.fx_sprite_draws.is_empty() {
            for alpha_mode in DYNAMIC_MODEL_ALPHA_ORDER {
                if self
                    .fx_sprite_draws
                    .iter()
                    .any(|draw| draw.alpha_mode == alpha_mode)
                {
                    self.ensure_fx_sprite_pipeline(jobs, device, alpha_mode);
                }
            }
        }
        let fx_sprite_bytes = bytemuck::cast_slice(fx_sprite_instances.as_slice());
        self.ensure_fx_sprite_instance_capacity(device, fx_sprite_bytes.len() as u64);
        if !fx_sprite_bytes.is_empty() {
            queue.write_buffer(&self.fx_sprite_instance_buffer, 0, fx_sprite_bytes);
        }

        let bone_bytes = bytemuck::cast_slice(bones.as_slice());
        let draw_bytes = bytemuck::cast_slice(gpu_draw_data.as_slice());
        self.ensure_skin_capacity(device, bone_bytes.len() as u64, draw_bytes.len() as u64);
        if !bone_bytes.is_empty() {
            queue.write_buffer(&self.skin_bone_buffer, 0, bone_bytes);
        }
        if !draw_bytes.is_empty() {
            queue.write_buffer(&self.skin_draw_buffer, 0, draw_bytes);
        }

        self.scratch_vertices = vertices;
        self.scratch_indices = indices;
        self.scratch_fx_sprite_instances = fx_sprite_instances;
        self.scratch_bones = bones;
        self.scratch_draw_data = gpu_draw_data;
        self.scratch_bone_offsets = bone_offsets;
        self.scratch_skin_pending = skin_pending;
    }

    pub(in crate::renderer) fn prepared_dynamic_buffers(&self) -> (&wgpu::Buffer, &wgpu::Buffer) {
        if self.rt_prepared {
            if let Some(rt) = self.rt_skinning.as_ref() {
                return (&rt.vertex_buffer, &rt.index_buffer);
            }
        }
        (&self.vertex_buffer, &self.index_buffer)
    }

    pub(in crate::renderer) fn draw_phase<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        camera: &'a wgpu::BindGroup,
        depth_writing_phase: bool,
        rt_receivers: Option<(&'a rt_models::RtModelReceivers, &'a wgpu::BindGroup)>,
        mut gpu_profiler: Option<&mut GpuProfiler>,
        excluded_entity: Option<u16>,
    ) {
        // Match the renderer-wide opaque-before-blended contract across both
        // dynamic geometry backends. The phase split is also used by water:
        // opaque/masked entities establish depth before the pre-water scene
        // capture, while saber glow, smoke and every other non-depth-writing FX
        // draw after the ocean surface/spray so water cannot paint over them.
        if !self.draws.is_empty() {
            let (vertex_buffer, index_buffer) = self.prepared_dynamic_buffers();
            pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.set_bind_group(0, camera, &[]);

            for alpha_mode in DYNAMIC_MODEL_ALPHA_ORDER
                .into_iter()
                .filter(|mode| dynamic_model_writes_depth(*mode) == depth_writing_phase)
                // Pipelines are lazy: only modes with draws were compiled.
                .filter(|mode| self.model_alpha_mask & (1 << mode.index()) != 0)
            {
                let Some(pipeline) = self.model_pipelines[alpha_mode.index()].as_ref() else {
                    continue;
                };
                let pipeline =
                    if let Some((models, scene)) = rt_receivers.filter(|_| self.rt_prepared) {
                        let receiver_pipeline = match alpha_mode {
                            DynamicModelAlphaMode::Opaque => Some(&models.opaque),
                            DynamicModelAlphaMode::Mask => Some(&models.mask),
                            DynamicModelAlphaMode::Blend => Some(&models.blend),
                            DynamicModelAlphaMode::MaskBlend => Some(&models.mask_blend),
                            _ => None,
                        };
                        if let Some(receiver_pipeline) = receiver_pipeline {
                            pass.set_vertex_buffer(
                                1,
                                self.rt_raw_color_buffer
                                    .as_ref()
                                    .expect("RT receiver colors prepared")
                                    .slice(..),
                            );
                            pass.set_bind_group(2, &models.lights, &[]);
                            pass.set_bind_group(3, scene, &[]);
                            receiver_pipeline
                        } else {
                            pipeline
                        }
                    } else {
                        pipeline
                    };
                pass.set_pipeline(pipeline);
                for draw in self.draws.iter().filter(|draw| {
                    draw.alpha_mode == alpha_mode && Some(draw.entity_num) != excluded_entity
                }) {
                    let bind_group = draw
                        .texture_key
                        .as_ref()
                        .and_then(|key| self.textures.get(key))
                        .map_or(&self.fallback_bind_group, |texture| &texture.bind_group);
                    pass.set_bind_group(1, bind_group, &[]);
                    pass.draw_indexed(
                        draw.first_index..draw.first_index.saturating_add(draw.index_count),
                        draw.base_vertex,
                        0..1,
                    );
                }
            }
        }

        let has_fx_sprites_in_phase = self.fx_sprite_draws.iter().any(|draw| {
            dynamic_model_writes_depth(draw.alpha_mode) == depth_writing_phase
                && Some(draw.entity_num) != excluded_entity
        });
        if has_fx_sprites_in_phase {
            let timed_pass = if depth_writing_phase {
                GpuPass::FxSpritesOpaque
            } else {
                GpuPass::FxSpritesTranslucent
            };
            if let Some(profiler) = gpu_profiler.as_deref_mut() {
                profiler.write_render_pass_timestamp(pass, timed_pass, false);
            }
            pass.set_vertex_buffer(0, self.fx_sprite_instance_buffer.slice(..));
            pass.set_bind_group(0, camera, &[]);
            for alpha_mode in DYNAMIC_MODEL_ALPHA_ORDER
                .into_iter()
                .filter(|mode| dynamic_model_writes_depth(*mode) == depth_writing_phase)
                // FX sprite pipelines are intentionally lazy.  Only visit alpha
                // modes that actually have sprite draws this frame; otherwise a
                // missing (correctly-unbuilt) pipeline would be mistaken for an
                // error when GPU FX is enabled.
                .filter(|mode| {
                    self.fx_sprite_draws.iter().any(|draw| {
                        draw.alpha_mode == *mode && Some(draw.entity_num) != excluded_entity
                    })
                })
            {
                let Some(pipeline) = self.fx_sprite_pipeline(alpha_mode) else {
                    // Runtime-lazy PSO still compiling; skip this FX variant for
                    // the handful of frames needed to finish rather than hitch.
                    continue;
                };
                pass.set_pipeline(pipeline);
                for draw in self.fx_sprite_draws.iter().filter(|draw| {
                    draw.alpha_mode == alpha_mode && Some(draw.entity_num) != excluded_entity
                }) {
                    let bind_group = draw
                        .texture_key
                        .as_ref()
                        .and_then(|key| self.textures.get(key))
                        .map_or(&self.fallback_bind_group, |texture| &texture.bind_group);
                    pass.set_bind_group(1, bind_group, &[]);
                    pass.draw(
                        0..6,
                        draw.first_instance
                            ..draw.first_instance.saturating_add(draw.instance_count),
                    );
                }
            }
            if let Some(profiler) = gpu_profiler.as_deref_mut() {
                profiler.write_render_pass_timestamp(pass, timed_pass, true);
            }
        }

        if !self.skin_draws.is_empty() {
            pass.set_bind_group(0, camera, &[]);
            for alpha_mode in DYNAMIC_MODEL_ALPHA_ORDER
                .into_iter()
                .filter(|mode| dynamic_model_writes_depth(*mode) == depth_writing_phase)
                .filter(|mode| self.skin_alpha_mask & (1 << mode.index()) != 0)
            {
                let Some(pipeline) = self.skin_pipelines[alpha_mode.index()].as_ref() else {
                    continue;
                };
                pass.set_pipeline(pipeline);
                pass.set_bind_group(2, &self.skin_bind_group, &[]);
                for draw in self.skin_draws.iter().filter(|draw| {
                    draw.alpha_mode == alpha_mode && Some(draw.entity_num) != excluded_entity
                }) {
                    let Some(mesh) = self.ghoul2_meshes.get(&draw.mesh_key) else {
                        continue;
                    };
                    let bind_group = draw
                        .texture_key
                        .as_ref()
                        .and_then(|key| self.textures.get(key))
                        .map_or(&self.fallback_bind_group, |texture| &texture.bind_group);
                    pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                    pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.set_bind_group(1, bind_group, &[]);
                    pass.draw_indexed(
                        0..mesh.index_count,
                        0,
                        draw.first_instance
                            ..draw.first_instance.saturating_add(draw.instance_count),
                    );
                }
            }
        }
    }

    pub(in crate::renderer) fn draw<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        camera: &'a wgpu::BindGroup,
    ) {
        for depth_writing_phase in [true, false] {
            self.draw_phase(pass, camera, depth_writing_phase, None, None, None);
        }
    }

    pub(in crate::renderer) fn draw_excluding_entity<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        camera: &'a wgpu::BindGroup,
        excluded_entity: u16,
    ) {
        for depth_writing_phase in [true, false] {
            self.draw_phase(
                pass,
                camera,
                depth_writing_phase,
                None,
                None,
                Some(excluded_entity),
            );
        }
    }

    pub(in crate::renderer) fn draw_wireframe<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        camera: &'a wgpu::BindGroup,
        mask: u32,
    ) {
        if let Some(pipeline) = &self.wireframe_pipeline {
            let mut bound = false;
            for draw in self
                .draws
                .iter()
                .filter(|draw| mask & draw.wireframe_class.mask_bit() != 0)
            {
                if !bound {
                    pass.set_pipeline(pipeline);
                    let (vertex_buffer, index_buffer) = self.prepared_dynamic_buffers();
                    pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                    pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.set_bind_group(0, camera, &[]);
                    // Keep slot 1 layout-compatible even though fs_wireframe does
                    // not sample it. Debug overlays share the normal model
                    // pipeline layout, so relying on whatever another overlay
                    // left bound here is invalid in WGPU.
                    pass.set_bind_group(1, &self.fallback_bind_group, &[]);
                    bound = true;
                }
                pass.draw_indexed(
                    draw.first_index..draw.first_index.saturating_add(draw.index_count),
                    draw.base_vertex,
                    0..1,
                );
            }
        }

        if let Some(pipeline) = &self.fx_sprite_wireframe_pipeline {
            let mut bound = false;
            for draw in self
                .fx_sprite_draws
                .iter()
                .filter(|draw| mask & draw.wireframe_class.mask_bit() != 0)
            {
                if !bound {
                    pass.set_pipeline(pipeline);
                    pass.set_vertex_buffer(0, self.fx_sprite_instance_buffer.slice(..));
                    pass.set_bind_group(0, camera, &[]);
                    pass.set_bind_group(1, &self.fallback_bind_group, &[]);
                    bound = true;
                }
                pass.draw(
                    0..6,
                    draw.first_instance..draw.first_instance.saturating_add(draw.instance_count),
                );
            }
        }

        if let Some(pipeline) = &self.skin_wireframe_pipeline {
            let mut bound = false;
            for draw in self
                .skin_draws
                .iter()
                .filter(|draw| mask & draw.wireframe_class.mask_bit() != 0)
            {
                let Some(mesh) = self.ghoul2_meshes.get(&draw.mesh_key) else {
                    continue;
                };
                if !bound {
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, camera, &[]);
                    pass.set_bind_group(1, &self.fallback_bind_group, &[]);
                    pass.set_bind_group(2, &self.skin_bind_group, &[]);
                    bound = true;
                }
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(
                    0..mesh.index_count,
                    0,
                    draw.first_instance..draw.first_instance.saturating_add(draw.instance_count),
                );
            }
        }
    }

    /// Adds depth-writing entities to the scene depth prepass, making linear
    /// depth the authoritative nearest-opaque depth. Fog and the other post
    /// consumers then read each entity's own depth instead of the BSP behind it.
    /// Requires `prepare` to have run for this frame.
    pub(in crate::renderer) fn draw_prepass<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        camera: &'a wgpu::BindGroup,
    ) {
        self.draw_depth_only(
            pass,
            camera,
            [
                (&self.prepass_pipeline, &self.skin_prepass_pipeline),
                (
                    &self.prepass_mask_pipeline,
                    &self.skin_prepass_mask_pipeline,
                ),
            ],
        );
    }

    /// Draws this frame's depth-writing entities into a shadow layer.
    /// `camera` is a camera-layout bind group holding the light's view-projection.
    /// Requires `prepare` and a matching `ensure_entity_shadow_pipelines`.
    pub(in crate::renderer) fn draw_entity_shadow<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        camera: &'a wgpu::BindGroup,
        reverse_z: bool,
    ) {
        let Some([md3, md3_mask, skin, skin_mask]) =
            self.entity_shadow_pipelines[usize::from(reverse_z)].as_ref()
        else {
            return;
        };
        self.draw_depth_only(pass, camera, [(md3, skin), (md3_mask, skin_mask)]);
    }

    /// Shared opaque + alpha-tested entity draw loop for every depth-only pass.
    /// `pipelines` is [(md3, ghoul2 skin); opaque, then alpha-tested].
    pub(in crate::renderer) fn draw_depth_only<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        camera: &'a wgpu::BindGroup,
        pipelines: [(&'a wgpu::RenderPipeline, &'a wgpu::RenderPipeline); 2],
    ) {
        let prepass_modes = [
            (
                DynamicModelAlphaMode::Opaque,
                pipelines[0].0,
                pipelines[0].1,
            ),
            (DynamicModelAlphaMode::Mask, pipelines[1].0, pipelines[1].1),
        ];
        if !self.draws.is_empty() {
            let (vertex_buffer, index_buffer) = self.prepared_dynamic_buffers();
            pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.set_bind_group(0, camera, &[]);
            for (alpha_mode, pipeline, _) in prepass_modes {
                let mut bound = false;
                for draw in self
                    .draws
                    .iter()
                    .filter(|draw| draw.alpha_mode == alpha_mode)
                {
                    if !bound {
                        pass.set_pipeline(pipeline);
                        // Opaque depth never samples the texture; one bind keeps
                        // the layout satisfied without per-draw state changes.
                        pass.set_bind_group(1, &self.fallback_bind_group, &[]);
                        bound = true;
                    }
                    if alpha_mode == DynamicModelAlphaMode::Mask {
                        let bind_group = draw
                            .texture_key
                            .as_ref()
                            .and_then(|key| self.textures.get(key))
                            .map_or(&self.fallback_bind_group, |texture| &texture.bind_group);
                        pass.set_bind_group(1, bind_group, &[]);
                    }
                    pass.draw_indexed(
                        draw.first_index..draw.first_index.saturating_add(draw.index_count),
                        draw.base_vertex,
                        0..1,
                    );
                }
            }
        }

        if !self.skin_draws.is_empty() {
            pass.set_bind_group(0, camera, &[]);
            for (alpha_mode, _, pipeline) in prepass_modes {
                let mut bound = false;
                for draw in self
                    .skin_draws
                    .iter()
                    .filter(|draw| draw.alpha_mode == alpha_mode)
                {
                    let Some(mesh) = self.ghoul2_meshes.get(&draw.mesh_key) else {
                        continue;
                    };
                    if !bound {
                        pass.set_pipeline(pipeline);
                        pass.set_bind_group(1, &self.fallback_bind_group, &[]);
                        pass.set_bind_group(2, &self.skin_bind_group, &[]);
                        bound = true;
                    }
                    if alpha_mode == DynamicModelAlphaMode::Mask {
                        let bind_group = draw
                            .texture_key
                            .as_ref()
                            .and_then(|key| self.textures.get(key))
                            .map_or(&self.fallback_bind_group, |texture| &texture.bind_group);
                        pass.set_bind_group(1, bind_group, &[]);
                    }
                    pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                    pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(
                        0..mesh.index_count,
                        0,
                        draw.first_instance
                            ..draw.first_instance.saturating_add(draw.instance_count),
                    );
                }
            }
        }
    }
}
