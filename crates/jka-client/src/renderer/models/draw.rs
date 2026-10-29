//! Models draw.
use crate::renderer::{
    classic_vertex_illumination, dynamic_model_writes_depth, ghoul2_specular_light,
    ghoul2_specular_viewer, rt_models, scene, Arc, DynamicModelAlphaMode, DynamicModelRenderer,
    DynamicModelSurface, DynamicPreparedDraw, EntityAmbientLightingMode, EntityLightMemo, Entry, FxGpuSpritePreparedDraw, Ghoul2BatchMode,
    Ghoul2GpuDrawUniform, Ghoul2GpuMesh, Ghoul2PendingDraw, Ghoul2PreparedDraw, GpuPass,
    GpuProfiler, PipelineJobManager, Vec3, source_fresnel, source_phong,
    DYNAMIC_MODEL_ALPHA_ORDER,
};
use wgpu::util::DeviceExt;
use super::submission::{fx_runs, model_runs, model_phase_runs, compositing_requires_order, ModelPhaseCalls};
use super::prepare::{pack_jobs, CpuPackJob, PreparePerf};
use std::time::Instant;

impl DynamicModelRenderer {
    /// Preserve the same packed FX and clipped blob streams in raster and RT preparation.
    pub(in crate::renderer) fn prepare_fx_surface(
        &mut self,
        surface: &DynamicModelSurface,
        texture_key: &Option<Arc<str>>,
        vertices: &mut Vec<crate::renderer::DynamicModelVertex>,
        indices: &mut Vec<u32>,
        fx_sprite_instances: &mut Vec<crate::renderer::FxGpuSpriteInstance>,
    ) -> bool {
        let Some(sprites) = surface.fx_gpu_sprites.as_ref() else {
            return false;
        };

        if sprites.blob_shadow {
            // Requests were clipped onto the rendered floor by the renderer
            // and arrive as an ordinary mesh drawn through the CPU path.
            if !self.blob_mesh_indices.is_empty() {
                self.record_cpu_geometry(crate::renderer::ModelGeometrySource::BlobShadow,
                    self.blob_mesh_vertices.len(), self.blob_mesh_indices.len());
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
                    texture_key: texture_key.clone(),
                    alpha_mode: surface.alpha_mode,
                    wireframe_class: surface.wireframe_class,
                    authored_glow: surface.authored_glow,
                });
                self.blob_mesh_vertices.clear();
                self.blob_mesh_indices.clear();
            }
            return true;
        }
        if let Some(resident) = &sprites.resident {
            // Carry GPU birth commands even when all eligible effects are
            // temporarily hidden. No empty frame may lose a slot upload.
            self.resident_fx_frame = Some(Arc::clone(&resident.frame));
            if !resident.slots.is_empty() {
                self.resident_fx_requested_modes |= 1u16 << surface.alpha_mode.index();
                // Never drop FX while a lazy GPU shader/PSO is compiling.
                // The packed ordinary instances are the visual fallback.
                if self.resident_fx.as_ref().is_some_and(|backend| backend.ready(surface.alpha_mode)) {
                    let first_instance = self.resident_fx_slots.len() as u32;
                    self.resident_fx_slots.extend(resident.slots.iter().copied());
                    self.resident_fx_draws.push(super::resident_fx::PreparedResidentFxDraw {
                        entity_num: surface.entity_num,
                        first_instance,
                        instance_count: resident.slots.len() as u32,
                        texture_key: texture_key.clone(),
                        alpha_mode: surface.alpha_mode,
                        authored_glow: surface.authored_glow,
                    });
                    return true;
                }
            }
        }
        if sprites.instances.is_empty() {
            return true;
        }
        let instances = sprites.instances.as_slice();
        let first_instance = u32::try_from(fx_sprite_instances.len()).unwrap_or(u32::MAX);
        fx_sprite_instances.extend_from_slice(instances);
        self.fx_sprite_draws.push(FxGpuSpritePreparedDraw {
            entity_num: surface.entity_num,
            first_instance,
            instance_count: u32::try_from(instances.len()).unwrap_or(u32::MAX),
            texture_key: texture_key.clone(),
            alpha_mode: surface.alpha_mode,
            wireframe_class: surface.wireframe_class,
            authored_glow: surface.authored_glow,
        });
        return true;
    }

    pub(in crate::renderer) fn upload_prepared_fx_sprites(
        &mut self,
        jobs: &mut PipelineJobManager,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        fx_sprite_instances: &[crate::renderer::FxGpuSpriteInstance],
        prepare_authored_glow: bool,
    ) {
        // Request only modes actually referenced by this FX frame. Other
        // unrelated renderer shaders remain cold; no startup compile cost.
        if self.resident_fx_frame.is_some() {
            let camera_layout = self.camera_layout.clone();
            let texture_layout = self.texture_layout.clone();
            let resident = self.resident_fx.get_or_insert_with(|| {
                super::resident_fx::ResidentFxBackend::new(
                    device, &camera_layout, &texture_layout,
                )
            });
            for alpha_mode in DYNAMIC_MODEL_ALPHA_ORDER {
                if self.resident_fx_requested_modes & (1u16 << alpha_mode.index()) != 0 {
                    resident.ensure_pipeline(jobs, device, alpha_mode,
                        self.fx_sprite_surface_format, self.fx_sprite_samples,
                        self.legacy_fog_compiled, self.fx_zero_alpha_discard);
                }
            }
            if let Some(frame) = self.resident_fx_frame.as_ref() {
                resident.upload(device, queue, frame, &self.resident_fx_slots);
            }
        }
        if !self.fx_sprite_draws.is_empty() {
            for alpha_mode in DYNAMIC_MODEL_ALPHA_ORDER {
                if self
                    .fx_sprite_draws
                    .iter()
                    .any(|draw| draw.alpha_mode == alpha_mode)
                {
                    self.ensure_fx_sprite_pipeline(jobs, device, alpha_mode);
                    if prepare_authored_glow
                        && self
                            .fx_sprite_draws
                            .iter()
                            .any(|draw| draw.alpha_mode == alpha_mode && draw.authored_glow)
                    {
                        self.ensure_fx_sprite_glow_pipeline(jobs, device, alpha_mode);
                    }
                }
            }
        }
        let fx_sprite_bytes = bytemuck::cast_slice(fx_sprite_instances);
        self.ensure_fx_sprite_instance_capacity(device, fx_sprite_bytes.len() as u64);
        if !fx_sprite_bytes.is_empty() {
            queue.write_buffer(&self.fx_sprite_instance_buffer, 0, fx_sprite_bytes);
        }
    }
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
        prepare_authored_glow: bool,
    ) {
        self.rt_prepared = false;
        if let Some(fixtures) = &mut self.fixture_lighting { fixtures.raw_colors.clear(); }
        self.perf_phase_calls.set(ModelPhaseCalls::default());
        self.capture_prepare = crate::renderer::diagnostics::perf_capture::enabled();
        self.perf_prepare = PreparePerf::default();
        if ray_traced_shadows {
            // RT preparation uses different buffers and may repack the scratch
            // streams. Re-enter raster through a cold, explicitly valid cache.
            self.md3_stream_cache.clear();
        }
        if ray_traced_shadows
            && self.prepare_rt_skinned(
                brightness,
                jobs,
                device,
                queue,
                surfaces,
                classic_grid,
                rt_receivers,
                prepare_authored_glow,
            )
        {
            return;
        }
        self.draws.clear();
        self.fx_sprite_draws.clear();
        self.resident_fx_draws.clear();
        self.resident_fx_slots.clear();
        self.resident_fx_frame = None;
        self.resident_fx_requested_modes = 0;
        self.skin_draws.clear();
        self.skin_instance_entities.clear();
        self.frame_texture_keys.clear();
        self.ghoul2_input_instances = 0;
        self.ghoul2_encoded_draws = 0;
        if surfaces.is_empty() {
            self.md3_stream_cache.clear();
            return;
        }
        if self.md3_geometry_cache { self.md3_stream_cache.begin_frame(); }

        let gather_started = self.capture_prepare.then(Instant::now);
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
        let mut light_memo = EntityLightMemo::default();
        let mut cpu_jobs = std::mem::take(&mut self.scratch_cpu_pack_jobs);
        cpu_jobs.clear();
        if self.model_prepare_threads != 0 { cpu_jobs.reserve(surfaces.len()); }

        for surface in surfaces {
            let source_material = surface
                .ghoul2_gpu
                .as_ref()
                .and_then(|skin| skin.source_material.as_ref());
            let texture_key = self.ensure_texture(
                brightness,
                device,
                queue,
                surface.texture.as_ref(),
                source_material.and_then(|material| material.normal_map.as_ref()),
            );
            if self.prepare_fx_surface(surface, &texture_key, &mut vertices, &mut indices,
                &mut fx_sprite_instances) {
                continue;
            }
            let classic_light = if (self.entity_ambient_lighting
                == EntityAmbientLightingMode::BspLightgridClassic
                || (surface.authored_lighting && self.entity_ambient_lighting == EntityAmbientLightingMode::Off))
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
                    (Some(origin), Some(grid)) => light_memo.light(
                        grid,
                        origin,
                        self.entity_sun_relight,
                        &self.entity_dlights,
                    ),
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
                let mesh = self.ghoul2_meshes.entry(Arc::clone(&mesh_key)).or_insert_with(|| {
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
                    Ghoul2GpuMesh {
                        bind_radius: skin.vertices.iter().map(|v|Vec3::from_array(v.position).length()).fold(0.0,f32::max),
                        vertex_buffer,
                        index_buffer,
                        index_count: u32::try_from(skin.indices.len()).unwrap_or(u32::MAX),
                        batch_keys: Default::default(),
                    }
                });

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
                    axis0: [skin.axis[0][0], skin.axis[0][1], skin.axis[0][2], if skin.foliage_wind { crate::foliage::model_tag(skin.origin) } else { 0.0 }],
                    axis1: [skin.axis[1][0], skin.axis[1][1], skin.axis[1][2], 0.0],
                    axis2: [skin.axis[2][0], skin.axis[2][1], skin.axis[2][2], 0.0],
                    // w: thermal heat-source matrix index (ghoul2_skin.wgsl thermal_body_heat).
                    origin: [skin.origin[0], skin.origin[1], skin.origin[2], skin.heat_bone as f32],
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
                    uv_cross: skin.uv_cross,
                    spec_light: ghoul2_specular_light(skin.specular),
                    spec_viewer: ghoul2_specular_viewer(skin.specular, skin.bulge_height),
                    jiggle0: skin.jiggle_offsets[0],
                    jiggle1: skin.jiggle_offsets[1],
                    jiggle2: skin.jiggle_offsets[2],
                    jiggle3: skin.jiggle_offsets[3],
                    jiggle4: skin.jiggle_offsets[4],
                    source_phong: source_phong(source_material),
                    source_fresnel: source_fresnel(source_material),
                    water_bands0: self.entity_water.get(&surface.entity_num).map_or([0.0; 4], |p| [p.bands[0],p.bands[1],p.bands[2],p.bands[3]]),
                    water_bands1: self.entity_water.get(&surface.entity_num).map_or([0.0; 4], |p| [p.bands[4],p.bands[5],p.bands[6],p.bands[7]]),
                    water_base: self.entity_water.get(&surface.entity_num).map_or([0.0; 4], |p| [p.first_z, 0.0, 0.0, 1.0]),
                };
                let texture_binding_id = if self.ghoul2_batch_draws == Ghoul2BatchMode::Off {
                    0
                } else {
                    texture_key.as_ref()
                        .and_then(|key| self.textures.get(key))
                        .map_or(0, |texture| texture.binding_id)
                };
                let mut pending = Ghoul2PendingDraw {
                    entity_num: surface.entity_num,
                    mesh_key,
                    texture_key,
                    alpha_mode: surface.alpha_mode,
                    wireframe_class: surface.wireframe_class,
                    authored_glow: surface.authored_glow,
                    source_material: source_material.is_some(),
                    texture_binding_id,
                    uniform,
                    sequence: u32::try_from(skin_pending.len()).unwrap_or(u32::MAX),
                    batch_hash: None,
                    batch_group_id: None,
                };
                if self.ghoul2_batch_key_cache && self.ghoul2_batch_draws != Ghoul2BatchMode::Off
                    && matches!(pending.alpha_mode, DynamicModelAlphaMode::Opaque | DynamicModelAlphaMode::Mask)
                    && Some(pending.entity_num) != self.companion_excluded_entity
                {
                    let (id, hit) = mesh.batch_keys.resolve_group_id(
                        &pending, texture_binding_id, prepare_authored_glow,
                        &mut self.next_ghoul2_group_id,
                    );
                    pending.batch_group_id = Some(id);
                    if self.capture_prepare {
                        if hit { self.perf_prepare.batch_key_hits += 1; }
                        else { self.perf_prepare.batch_key_misses += 1; }
                    }
                }
                skin_pending.push(pending);
                continue;
            }

            if surface.vertices.is_empty() || surface.indices.is_empty() {
                continue;
            }
            let base_vertex = vertices.len() as u32;
            let first_index = indices.len() as u32;
            if let Some(fixtures) = &mut self.fixture_lighting {
                // Keep this stream aligned with CPU vertices, including gaps
                // occupied by sprites/FX. Never derive albedo from baked RGB.
                fixtures.raw_colors.resize(base_vertex as usize, [0.0;4]);
                let lit = surface.lighting_origin.is_some() || surface.authored_lighting;
                fixtures.raw_colors.extend(surface.vertices.iter().map(|v|
                    if lit { v.color } else { [0.0;4] }));
            }
            self.record_cpu_geometry(surface.geometry_source, surface.vertices.len(), surface.indices.len());
            let cached_md3 = self.md3_geometry_cache
                && surface.geometry_source == crate::renderer::ModelGeometrySource::Md3
                // A reused dry MD3 vertex stream must not mask live player wetness.
                && !(surface.wireframe_class == crate::renderer::DynamicWireframeClass::Player
                    && self.entity_water.contains_key(&surface.entity_num))
                && self.md3_stream_cache.append(surface, classic_light, &mut vertices, &mut indices);
            if cached_md3 {
                // Already lit and rebased with these exact source allocations,
                // sampled lights and stream offsets. Preserve the original draw.
            } else if self.model_prepare_threads != 0 {
                vertices.extend_from_slice(surface.vertices.as_slice());
                indices.extend_from_slice(surface.indices.as_slice());
                cpu_jobs.push(CpuPackJob {
                    vertices: base_vertex as usize..vertices.len(),
                    indices: first_index as usize..indices.len(),
                    light: classic_light,
                    water: self.entity_water.get(&surface.entity_num).copied()
                        .filter(|_| surface.wireframe_class == crate::renderer::DynamicWireframeClass::Player),
                });
            } else {
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
            }
            if self.model_prepare_threads == 0
                && surface.wireframe_class == crate::renderer::DynamicWireframeClass::Player
            {
                if let Some(water) = self.entity_water.get(&surface.entity_num) {
                    for vertex in &mut vertices[base_vertex as usize..] {
                        vertex.water_wetness = water.wetness_at_z(vertex.position[1]);
                    }
                }
            }
            self.draws.push(DynamicPreparedDraw {
                entity_num: surface.entity_num,
                first_index,
                index_count: surface.indices.len() as u32,
                base_vertex: 0,
                texture_key,
                alpha_mode: surface.alpha_mode,
                wireframe_class: surface.wireframe_class,
                authored_glow: surface.authored_glow,
            });
        }

        if let Some(started) = gather_started { self.perf_prepare.gather_ms = started.elapsed().as_secs_f64() * 1000.0; }
        let pack_started = self.capture_prepare.then(Instant::now);
        if !cpu_jobs.is_empty() {
            let parallel = self.cpu_pack_pool.is_some()
                && (vertices.len() >= 4096 || indices.len() >= 8192) && cpu_jobs.len() >= 2;
            if parallel {
                let splits = if self.model_prepare_threads == 4 { 3 } else { 2 };
                self.cpu_pack_pool.as_ref().unwrap().install(||
                    pack_jobs(&cpu_jobs, &mut vertices, &mut indices, 0, 0, splits));
            } else {
                pack_jobs(&cpu_jobs, &mut vertices, &mut indices, 0, 0, 0);
            }
            self.perf_prepare.parallel_used = parallel;
        }
        self.scratch_cpu_pack_jobs = cpu_jobs;
        if self.md3_geometry_cache {
            self.md3_stream_cache.finish_frame(&vertices, &indices);
            self.perf_prepare.md3_cache_hits = self.md3_stream_cache.hits;
            self.perf_prepare.md3_cache_misses = self.md3_stream_cache.misses;
            self.perf_prepare.md3_reused_vertices = self.md3_stream_cache.vertex_hits.iter()
                .map(|range| range.len() as u64).sum();
        }
        if let Some(started) = pack_started { self.perf_prepare.cpu_pack_ms = started.elapsed().as_secs_f64() * 1000.0; }
        let group_started = self.capture_prepare.then(Instant::now);
        self.ghoul2_input_instances = u32::try_from(skin_pending.len()).unwrap_or(u32::MAX);
        // Fast path: per-mesh cached group IDs, one visible-instance bucket
        // pass, and sorting only *unique* groups. All scratch allocations are
        // retained. The older full-frame hash+sort remains an A/B fallback.
        let mut group_lookup = std::mem::take(&mut self.scratch_group_lookup);
        let mut frame_groups = std::mem::take(&mut self.scratch_frame_groups);
        let mut group_sort = std::mem::take(&mut self.scratch_group_sort);
        let mut group_order = std::mem::take(&mut self.scratch_group_order);
        let cached_grouping = self.ghoul2_batch_key_cache
            && self.ghoul2_batch_draws != Ghoul2BatchMode::Off;
        let batching_active = if cached_grouping {
            Self::prepare_cached_ghoul2_order(
                &skin_pending, separate_wireframe_classes,
                self.companion_excluded_entity, self.ghoul2_batch_draws,
                &mut group_lookup, &mut frame_groups, &mut group_sort, &mut group_order,
            )
        } else {
            match self.ghoul2_batch_draws {
                Ghoul2BatchMode::Off => false,
                Ghoul2BatchMode::Adaptive => {
                    Self::should_adaptively_batch(
                        &mut skin_pending, prepare_authored_glow,
                        separate_wireframe_classes, self.companion_excluded_entity,
                        &mut self.scratch_batch_counts,
                    )
                }
                Ghoul2BatchMode::Force => true,
            }
        };
        if batching_active && !cached_grouping {
            for draw in &mut skin_pending {
                if matches!(draw.alpha_mode, DynamicModelAlphaMode::Opaque | DynamicModelAlphaMode::Mask)
                    && Some(draw.entity_num) != self.companion_excluded_entity
                    && draw.batch_hash.is_none()
                {
                    draw.batch_hash = Some(Self::ghoul2_batch_hash(draw, prepare_authored_glow));
                }
            }
            Self::sort_ghoul2_pending(&mut skin_pending, prepare_authored_glow,
                separate_wireframe_classes, self.companion_excluded_entity);
        }
        // For an inactive adaptive frame, keep exact original submission order.
        let ordered_index = |index: usize| -> usize {
            if batching_active && cached_grouping { group_order[index] } else { index }
        };

        let mut pending_index = 0usize;
        while pending_index < skin_pending.len() {
            let pending = &skin_pending[ordered_index(pending_index)];
            let candidate = batching_active
                && matches!(
                    pending.alpha_mode,
                    DynamicModelAlphaMode::Opaque | DynamicModelAlphaMode::Mask
                )
                && Some(pending.entity_num) != self.companion_excluded_entity;
            let mut group_end = pending_index + 1;
            if candidate {
                while group_end < skin_pending.len() {
                    let next = &skin_pending[ordered_index(group_end)];
                    if Some(next.entity_num) == self.companion_excluded_entity
                        || next.alpha_mode != pending.alpha_mode
                        // One instanced draw call is one pipeline, and a Source
                        // material takes a different one.
                        || next.source_material != pending.source_material
                        || (separate_wireframe_classes
                            && next.wireframe_class != pending.wireframe_class)
                        || next.mesh_key != pending.mesh_key
                        || next.texture_key != pending.texture_key
                        || (prepare_authored_glow
                            && next.authored_glow != pending.authored_glow)
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
                    Ghoul2BatchMode::Adaptive => group_len >= 2,
                    Ghoul2BatchMode::Force => group_len >= 2,
                };

            if self.capture_prepare {
                if instance_group {
                    self.perf_prepare.ghoul2_instanced_groups += 1;
                    self.perf_prepare.ghoul2_largest_group = self.perf_prepare.ghoul2_largest_group.max(group_len as u32);
                } else if Some(pending.entity_num) == self.companion_excluded_entity {
                    self.perf_prepare.ghoul2_excluded_instances += group_len as u32;
                } else if !matches!(pending.alpha_mode, DynamicModelAlphaMode::Opaque | DynamicModelAlphaMode::Mask) {
                    self.perf_prepare.ghoul2_ordered_instances += group_len as u32;
                } else {
                    self.perf_prepare.ghoul2_singleton_groups += group_len as u32;
                }
            }

            if instance_group {
                let first_instance = u32::try_from(gpu_draw_data.len()).unwrap_or(u32::MAX);
                for index in pending_index..group_end {
                    let item = &skin_pending[ordered_index(index)];
                    gpu_draw_data.push(item.uniform);
                    self.skin_instance_entities.push(item.entity_num);
                }
                self.skin_draws.push(Ghoul2PreparedDraw {
                    entity_num: pending.entity_num,
                    mesh_key: Arc::clone(&pending.mesh_key),
                    texture_key: pending.texture_key.as_ref().map(Arc::clone),
                    alpha_mode: pending.alpha_mode,
                    wireframe_class: pending.wireframe_class,
                    authored_glow: pending.authored_glow,
                    source_material: pending.source_material,
                    first_instance,
                    instance_count: u32::try_from(group_len).unwrap_or(u32::MAX),
                });
            } else {
                for index in pending_index..group_end {
                    let item = &skin_pending[ordered_index(index)];
                    let first_instance = u32::try_from(gpu_draw_data.len()).unwrap_or(u32::MAX);
                    gpu_draw_data.push(item.uniform);
                    self.skin_instance_entities.push(item.entity_num);
                    self.skin_draws.push(Ghoul2PreparedDraw {
                        entity_num: item.entity_num,
                        mesh_key: Arc::clone(&item.mesh_key),
                        texture_key: item.texture_key.as_ref().map(Arc::clone),
                        alpha_mode: item.alpha_mode,
                        wireframe_class: item.wireframe_class,
                        authored_glow: item.authored_glow,
                        source_material: item.source_material,
                        first_instance,
                        instance_count: 1,
                    });
                }
            }
            pending_index = group_end;
        }
        self.scratch_group_lookup = group_lookup;
        self.scratch_frame_groups = frame_groups;
        self.scratch_group_sort = group_sort;
        self.scratch_group_order = group_order;
        self.ghoul2_encoded_draws = u32::try_from(self.skin_draws.len()).unwrap_or(u32::MAX);
        if let Some(started) = group_started { self.perf_prepare.ghoul2_group_ms = started.elapsed().as_secs_f64() * 1000.0; }
        let upload_started = self.capture_prepare.then(Instant::now);

        let vertex_bytes = bytemuck::cast_slice(vertices.as_slice());
        let index_bytes = bytemuck::cast_slice(indices.as_slice());
        let vertices_grew = vertex_bytes.len() as u64 > self.vertex_capacity;
        let indices_grew = index_bytes.len() as u64 > self.index_capacity;
        self.ensure_vertex_capacity(device, vertex_bytes.len() as u64);
        self.ensure_index_capacity(device, index_bytes.len() as u64);
        if self.md3_geometry_cache {
            self.perf_prepare.vertex_upload_bytes = super::md3_cache::upload_stream(
                queue, &self.vertex_buffer, &vertices,
                &self.md3_stream_cache.vertex_hits, vertices_grew);
            self.perf_prepare.md3_vertex_skipped_bytes = vertex_bytes.len() as u64
                - self.perf_prepare.vertex_upload_bytes;
        } else if !vertex_bytes.is_empty() {
            queue.write_buffer(&self.vertex_buffer, 0, vertex_bytes);
            self.perf_prepare.vertex_upload_bytes = vertex_bytes.len() as u64;
        }
        if self.model_index_upload_cache {
            let changed = self.index_upload_cache.update(&indices);
            if let Some(span) = changed.as_ref() {
                let bytes = bytemuck::cast_slice(&indices[span.clone()]);
                queue.write_buffer(&self.index_buffer, span.start as u64 * 4, bytes);
                self.perf_prepare.index_upload_bytes = bytes.len() as u64;
            }
            self.perf_prepare.index_skipped_bytes = index_bytes.len() as u64 - self.perf_prepare.index_upload_bytes;
            if self.md3_geometry_cache {
                self.perf_prepare.md3_index_skipped_bytes = self.md3_stream_cache.index_hits.iter().map(|hit| {
                    let overlap = changed.as_ref().map_or(0, |span|
                        hit.end.min(span.end).saturating_sub(hit.start.max(span.start)));
                    (hit.len() - overlap) as u64 * 4
                }).sum();
            }
        } else if self.md3_geometry_cache {
            self.perf_prepare.index_upload_bytes = super::md3_cache::upload_stream(
                queue, &self.index_buffer, &indices,
                &self.md3_stream_cache.index_hits, indices_grew);
            self.perf_prepare.md3_index_skipped_bytes = index_bytes.len() as u64
                - self.perf_prepare.index_upload_bytes;
            self.perf_prepare.index_skipped_bytes = self.perf_prepare.md3_index_skipped_bytes;
        } else if !index_bytes.is_empty() {
            queue.write_buffer(&self.index_buffer, 0, index_bytes);
            self.perf_prepare.index_upload_bytes = index_bytes.len() as u64;
        }

        self.ensure_model_pipelines(jobs, device, prepare_authored_glow);
        if let Some(fixtures) = &mut self.fixture_lighting {
            fixtures.raw_colors.resize(vertices.len(), [0.0;4]);
            fixtures.upload_raw(device, queue);
            for draw in &self.draws { fixtures.ensure(device,jobs,draw.alpha_mode,false,false); }
            for draw in &self.skin_draws { fixtures.ensure(device,jobs,draw.alpha_mode,true,draw.source_material); }
        }
        self.upload_prepared_fx_sprites(jobs, device, queue, &fx_sprite_instances, prepare_authored_glow);

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
        if let Some(started) = upload_started { self.perf_prepare.upload_ms = started.elapsed().as_secs_f64() * 1000.0; }
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
        gpu_profiler: Option<&mut GpuProfiler>,
        excluded_entity: Option<u16>,
        authored_glow_only: bool,
    ) {
        self.draw_phase_inner(pass,camera,depth_writing_phase,rt_receivers,gpu_profiler,
            excluded_entity,authored_glow_only,false,None);
    }

    /// Primary scene draws these legacy shells in a separate encoded pass.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::renderer) fn draw_scene_phase<'a>(
        &'a self, pass:&mut wgpu::RenderPass<'a>, camera:&'a wgpu::BindGroup,
        depth_writing_phase:bool,
        rt_receivers:Option<(&'a rt_models::RtModelReceivers,&'a wgpu::BindGroup)>,
        profiler:Option<&mut GpuProfiler>, excluded:Option<u16>, glow:bool,
    ) {
        let split = !depth_writing_phase && !glow && self.legacy_blend_active();
        self.draw_phase_inner(pass,camera,depth_writing_phase,rt_receivers,profiler,excluded,glow,false,
            split.then_some(false));
    }

    pub(in crate::renderer) fn draw_encoded_legacy_models<'a>(&'a self,
        pass:&mut wgpu::RenderPass<'a>,camera:&'a wgpu::BindGroup) {
        self.draw_phase_inner(pass,camera,false,None,None,None,false,false,Some(true));
    }

    fn draw_phase_inner<'a>(
        &'a self, pass:&mut wgpu::RenderPass<'a>, camera:&'a wgpu::BindGroup,
        depth_writing_phase:bool,
        rt_receivers:Option<(&'a rt_models::RtModelReceivers,&'a wgpu::BindGroup)>,
        mut gpu_profiler:Option<&mut GpuProfiler>, excluded_entity:Option<u16>,
        authored_glow_only:bool,studio_capture:bool,legacy_effects:Option<bool>,
    ) {
        let capture_calls = crate::renderer::diagnostics::perf_capture::enabled();
        let mut calls = self.perf_phase_calls.get();
        // Match the renderer-wide opaque-before-blended contract across both
        // dynamic geometry backends. The phase split is also used by water:
        // opaque/masked entities establish depth before the pre-water scene
        // capture, while saber glow, smoke and every other non-depth-writing FX
        // draw after the ocean surface/spray so water cannot paint over them.
        if !self.draws.is_empty() {
            let (vertex_buffer, index_buffer) = self.prepared_dynamic_buffers();
            let mut cpu_buffers_bound = false;

            let preserve_order = !depth_writing_phase && self.draws.iter().any(|draw| compositing_requires_order(draw.alpha_mode));
            for (alpha_mode, draw_range) in model_phase_runs(
                self.draws.iter().map(|draw| draw.alpha_mode), preserve_order, depth_writing_phase,
            ).filter(|(mode, _)| self.model_alpha_mask & (1 << mode.index()) != 0
                && legacy_effects.is_none_or(|encoded| super::legacy_blend::eligible(*mode) == encoded))
            {
                let pipeline = if legacy_effects == Some(true) {
                    self.legacy_model_pipelines[alpha_mode.index()].as_ref()
                } else if studio_capture {
                    self.studio_capture_pipelines[alpha_mode.index()].as_ref()
                } else if authored_glow_only {
                    self.model_glow_pipelines[alpha_mode.index()].as_ref()
                } else {
                    self.model_pipelines[alpha_mode.index()].as_ref()
                };
                let Some(pipeline) = pipeline else {
                    continue;
                };
                let fixture = self.fixture_lighting.as_ref().filter(|f|
                    legacy_effects != Some(true) && !studio_capture && !authored_glow_only && !self.rt_prepared
                        && f.pipeline(alpha_mode,false,false).is_some());
                if legacy_effects != Some(true) && !studio_capture && fixture.is_none() && alpha_mode != DynamicModelAlphaMode::Glass && self.model_render_bundles && !preserve_order
                    && super::bundles::eligible_pass(self.rt_prepared, excluded_entity, authored_glow_only)
                {
                    if let Some((bundle, represented_draws, last_texture)) = self.model_bundles.executable(
                        alpha_mode, camera, pipeline, vertex_buffer, index_buffer,
                    ) {
                        super::bundles::execute_and_restore_model_state(
                            pass, bundle, camera, pipeline, vertex_buffer, index_buffer, last_texture,
                        );
                        if capture_calls {
                            calls.bundle_executes += 1;
                            calls.bundle_draw_indexed += represented_draws;
                        }
                        // Restore the state left by the equivalent direct draws.
                        // execute_bundles clears pipeline, bind groups and buffers;
                        // callers and subsequent modes retain the old contract.
                        if capture_calls {
                            calls.set_pipeline += 1;
                            calls.set_vertex_buffer += 1;
                            calls.set_index_buffer += 1;
                            calls.set_bind_group += 2;
                        }
                        cpu_buffers_bound = true;
                        continue;
                    }
                }
                if !cpu_buffers_bound {
                    if capture_calls { calls.set_vertex_buffer += 1; }
                    pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                    if capture_calls { calls.set_index_buffer += 1; }
                    pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    if capture_calls { calls.set_bind_group += 1; }
                    pass.set_bind_group(0, camera, &[]);
                    cpu_buffers_bound = true;
                }
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
                            if capture_calls { calls.set_vertex_buffer += 1; }
                            pass.set_vertex_buffer(
                                1,
                                self.rt_raw_color_buffer
                                    .as_ref()
                                    .expect("RT receiver colors prepared")
                                    .slice(..),
                            );
                            if capture_calls { calls.set_bind_group += 1; }
                            pass.set_bind_group(2, &models.lights, &[]);
                            if capture_calls { calls.set_bind_group += 1; }
                            pass.set_bind_group(3, scene, &[]);
                            receiver_pipeline
                        } else {
                            pipeline
                        }
                    } else {
                        pipeline
                    };
                let pipeline = if let Some(fixture) = fixture {
                    pass.set_bind_group(2,&fixture.bind_group,&[]);
                    pass.set_vertex_buffer(1,fixture.raw_buffer.as_ref().expect("fixture raw tint prepared").slice(..));
                    if capture_calls { calls.set_bind_group += 1; calls.set_vertex_buffer += 1; }
                    fixture.pipeline(alpha_mode,false,false).unwrap()
                } else { pipeline };
                if capture_calls { calls.set_pipeline += 1; }
                pass.set_pipeline(pipeline);
                if alpha_mode == DynamicModelAlphaMode::Glass {
                    let Some(glass) = self.glass_bind_group.as_ref() else { continue; };
                    pass.set_bind_group(2, glass, &[]);
                }
                // Track only this pipeline's bindings: subsequent pipelines,
                // draw_phase invocations and passes always start afresh.
                let mut current_texture = None;
                for (draw, indices) in model_runs(self.draws.iter().skip(draw_range.start).take(draw_range.end - draw_range.start).filter(|draw| {
                    draw.alpha_mode == alpha_mode
                        && Some(draw.entity_num) != excluded_entity
                        && (!authored_glow_only || draw.authored_glow)
                }), self.dynamic_batch_draws) {
                    let bind_group = draw
                        .texture_key
                        .as_ref()
                        .and_then(|key| self.textures.get(key))
                        .map_or(&self.fallback_bind_group, |texture| &texture.bind_group);
                    if !self.model_state_cache
                        || !current_texture.is_some_and(|current| std::ptr::eq(current, bind_group))
                    {
                        if capture_calls { calls.set_bind_group += 1; }
                        pass.set_bind_group(1, bind_group, &[]);
                        current_texture = Some(bind_group);
                    }
                    if capture_calls { calls.cpu_draw_indexed += 1; }
                    pass.draw_indexed(indices, draw.base_vertex, 0..1);
                }
            }
        }

        if legacy_effects != Some(true) {
            let has_fx_sprites_in_phase = self.fx_sprite_draws.iter().any(|draw| {
                dynamic_model_writes_depth(draw.alpha_mode) == depth_writing_phase
                    && Some(draw.entity_num) != excluded_entity
                    && (!authored_glow_only || draw.authored_glow)
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
                if capture_calls { calls.set_vertex_buffer += 1; }
                pass.set_vertex_buffer(0, self.fx_sprite_instance_buffer.slice(..));
                if capture_calls { calls.set_bind_group += 1; }
                pass.set_bind_group(0, camera, &[]);
                let preserve_order = !depth_writing_phase && self.fx_sprite_draws.iter().any(|draw| compositing_requires_order(draw.alpha_mode));
                for (alpha_mode, draw_range) in model_phase_runs(
                    self.fx_sprite_draws.iter().map(|draw| draw.alpha_mode), preserve_order, depth_writing_phase,
                )
                    // FX sprite pipelines are intentionally lazy.  Only visit alpha
                    // modes that actually have sprite draws this frame; otherwise a
                    // missing (correctly-unbuilt) pipeline would be mistaken for an
                    // error when GPU FX is enabled.
                    .filter(|(mode, _)| {
                        self.fx_sprite_draws.iter().any(|draw| {
                            draw.alpha_mode == *mode
                                && Some(draw.entity_num) != excluded_entity
                                && (!authored_glow_only || draw.authored_glow)
                        })
                    })
                {
                    let pipeline = if authored_glow_only {
                        self.fx_sprite_glow_pipelines[alpha_mode.index()].as_ref()
                    } else {
                        self.fx_sprite_pipeline(alpha_mode)
                    };
                    let Some(pipeline) = pipeline else {
                        // Runtime-lazy PSO still compiling; skip this FX variant for
                        // the handful of frames needed to finish rather than hitch.
                        continue;
                    };
                    if capture_calls { calls.set_pipeline += 1; }
                    pass.set_pipeline(pipeline);
                    let mut current_texture = None;
                    for (draw, instances) in fx_runs(self.fx_sprite_draws.iter().skip(draw_range.start).take(draw_range.end - draw_range.start).filter(|draw| {
                        draw.alpha_mode == alpha_mode
                            && Some(draw.entity_num) != excluded_entity
                            && (!authored_glow_only || draw.authored_glow)
                    }), self.dynamic_batch_draws) {
                        let bind_group = draw
                            .texture_key
                            .as_ref()
                            .and_then(|key| self.textures.get(key))
                            .map_or(&self.fallback_bind_group, |texture| &texture.bind_group);
                        if !self.model_state_cache
                            || !current_texture.is_some_and(|current| std::ptr::eq(current, bind_group))
                        {
                            if capture_calls { calls.set_bind_group += 1; }
                            pass.set_bind_group(1, bind_group, &[]);
                            current_texture = Some(bind_group);
                        }
                        if capture_calls { calls.fx_draw += 1; }
                        pass.draw(0..6, instances);
                    }
                }
                if let Some(profiler) = gpu_profiler.as_deref_mut() {
                    profiler.write_render_pass_timestamp(pass, timed_pass, true);
                }
            }

            // GPU-resident effects use the SAME material group as FX sprites but
            // their independent group 2 binds persistent spawn data, draw slots and
            // the demo/FX clock. They never borrow Ghoul2's group 2 or weather's.
            if let Some(resident) = self.resident_fx.as_ref() {
                let relevant = self.resident_fx_draws.iter().any(|draw| {
                    dynamic_model_writes_depth(draw.alpha_mode) == depth_writing_phase
                        && Some(draw.entity_num) != excluded_entity
                        && (!authored_glow_only || draw.authored_glow)
                });
                if relevant {
                    pass.set_bind_group(0, camera, &[]);
                    pass.set_bind_group(2, resident.bind_group(), &[]);
                    if capture_calls { calls.set_bind_group += 2; }
                    let mut last_pipeline = None;
                    let mut last_texture = None;
                    for draw in &self.resident_fx_draws {
                        if dynamic_model_writes_depth(draw.alpha_mode) != depth_writing_phase
                            || Some(draw.entity_num) == excluded_entity
                            || (authored_glow_only && !draw.authored_glow)
                        { continue; }
                        // Stage `glow` replay is never promoted until it has an
                        // independently-specialized FORCE_FOG_BLACK pipeline.
                        if authored_glow_only || draw.authored_glow { continue; }
                        let Some(pipeline) = resident.pipeline(draw.alpha_mode) else { continue; };
                        if !last_pipeline.is_some_and(|mode| mode == draw.alpha_mode) {
                            pass.set_pipeline(pipeline);
                            last_pipeline = Some(draw.alpha_mode);
                            last_texture = None;
                            if capture_calls { calls.set_pipeline += 1; }
                        }
                        let bind_group = draw.texture_key.as_ref()
                            .and_then(|key| self.textures.get(key))
                            .map_or(&self.fallback_bind_group, |texture| &texture.bind_group);
                        if !self.model_state_cache
                            || !last_texture.is_some_and(|prior| std::ptr::eq(prior, bind_group))
                        {
                            pass.set_bind_group(1, bind_group, &[]);
                            last_texture = Some(bind_group);
                            if capture_calls { calls.set_bind_group += 1; }
                        }
                        pass.draw(0..6, draw.first_instance..draw.first_instance.saturating_add(draw.instance_count));
                        if capture_calls { calls.fx_draw += 1; }
                    }
                }
            }

        }

        if !self.skin_draws.is_empty() {
            if capture_calls { calls.set_bind_group += 1; }
            pass.set_bind_group(0, camera, &[]);
            let preserve_order = !depth_writing_phase && self.skin_draws.iter().any(|draw| compositing_requires_order(draw.alpha_mode));
            for (alpha_mode, draw_range) in model_phase_runs(
                self.skin_draws.iter().map(|draw| draw.alpha_mode), preserve_order, depth_writing_phase,
            ).filter(|(mode, _)| self.skin_alpha_mask & (1 << mode.index()) != 0
                && legacy_effects.is_none_or(|encoded| super::legacy_blend::eligible(*mode) == encoded))
            {
                // Source 1 materials need their own pipeline, so this mode is
                // submitted twice when the frame has both kinds. A frame with no
                // Source character keeps a single pass over a single pipeline,
                // exactly as before.
                let source_variants: &[bool] = if authored_glow_only
                    || self.skin_source_mask & (1 << alpha_mode.index()) == 0
                {
                    &[false]
                } else {
                    &[false, true]
                };
                for &source_material in source_variants {
                    let pipeline = if legacy_effects == Some(true) {
                        self.legacy_skin_pipelines[alpha_mode.index()].as_ref()
                    } else if authored_glow_only {
                        self.skin_glow_pipelines[alpha_mode.index()].as_ref()
                    } else if source_material {
                        self.skin_source_pipelines[alpha_mode.index()].as_ref()
                    } else {
                        self.skin_pipelines[alpha_mode.index()].as_ref()
                    };
                    let Some(pipeline) = pipeline else {
                        continue;
                    };
                    let fixture = self.fixture_lighting.as_ref().filter(|f|
                        legacy_effects != Some(true) && !authored_glow_only && f.pipeline(alpha_mode,true,source_material).is_some());
                    let pipeline = if let Some(fixture) = fixture {
                        pass.set_bind_group(3,&fixture.bind_group,&[]);
                        if capture_calls { calls.set_bind_group += 1; }
                        fixture.pipeline(alpha_mode,true,source_material).unwrap()
                    } else { pipeline };
                    if capture_calls { calls.set_pipeline += 1; }
                    pass.set_pipeline(pipeline);
                    if capture_calls { calls.set_bind_group += 1; }
                    pass.set_bind_group(2, &self.skin_bind_group, &[]);
                    if alpha_mode == DynamicModelAlphaMode::Glass {
                        let Some(glass) = self.glass_bind_group.as_ref() else { continue; };
                        pass.set_bind_group(3, glass, &[]);
                    }
                    let mut current_texture = None;
                    let mut current_mesh = None;
                    for draw in self.skin_draws.iter().skip(draw_range.start).take(draw_range.end - draw_range.start).filter(|draw| {
                        draw.alpha_mode == alpha_mode
                            && draw.source_material == source_material
                            && Some(draw.entity_num) != excluded_entity
                            && (!authored_glow_only || draw.authored_glow)
                    }) {
                        let Some(mesh) = self.ghoul2_meshes.get(&draw.mesh_key) else {
                            continue;
                        };
                        let bind_group = draw
                            .texture_key
                            .as_ref()
                            .and_then(|key| self.textures.get(key))
                            .map_or(&self.fallback_bind_group, |texture| &texture.bind_group);
                        if !self.model_state_cache
                            || !current_mesh.is_some_and(|current| std::ptr::eq(current, mesh))
                        {
                            if capture_calls { calls.set_vertex_buffer += 1; }
                            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                            if capture_calls { calls.set_index_buffer += 1; }
                            pass.set_index_buffer(
                                mesh.index_buffer.slice(..),
                                wgpu::IndexFormat::Uint32,
                            );
                            current_mesh = Some(mesh);
                        }
                        if !self.model_state_cache
                            || !current_texture.is_some_and(|current| std::ptr::eq(current, bind_group))
                        {
                            if capture_calls { calls.set_bind_group += 1; }
                            pass.set_bind_group(1, bind_group, &[]);
                            current_texture = Some(bind_group);
                        }
                        if capture_calls { calls.skin_draw_indexed += 1; }
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
        if capture_calls { self.perf_phase_calls.set(calls); }
    }

    pub(in crate::renderer) fn draw<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        camera: &'a wgpu::BindGroup,
    ) {
        for depth_writing_phase in [true, false] {
            self.draw_phase(pass, camera, depth_writing_phase, None, None, None, false);
        }
    }

    pub(in crate::renderer) fn draw_studio_capture<'a>(&'a self,pass:&mut wgpu::RenderPass<'a>,camera:&'a wgpu::BindGroup) {
        for phase in [true,false] {
            self.draw_phase_inner(pass,camera,phase,None,None,None,false,true,None);
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
                false,
            );
        }
    }

    /// Replay only exact JKA shader stages marked `glow` into the authored
    /// glow source. Geometry/material pipelines remain identical to the visible
    /// scene; submission alone is filtered.
    pub(in crate::renderer) fn draw_authored_glow<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        camera: &'a wgpu::BindGroup,
    ) {
        for depth_writing_phase in [true, false] {
            self.draw_phase(
                pass,
                camera,
                depth_writing_phase,
                None,
                None,
                None,
                true,
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

    /// Adds dynamic entities to the one-target rain lean-depth prepass.
    pub(in crate::renderer) fn draw_rain_prepass<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        camera: &'a wgpu::BindGroup,
    ) {
        self.draw_depth_only(
            pass,
            camera,
            [
                (&self.rain_prepass_pipeline, &self.skin_rain_prepass_pipeline),
                (
                    &self.rain_prepass_mask_pipeline,
                    &self.skin_rain_prepass_mask_pipeline,
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
        self.draw_depth_only_filtered(pass,camera,pipelines,None);
    }

    pub(in crate::renderer) fn draw_local_entity_shadow<'a>(
        &'a self, pass: &mut wgpu::RenderPass<'a>, camera: &'a wgpu::BindGroup,
        entities: &std::collections::HashSet<u16>,
    ) {
        let Some([md3,md3_mask,skin,skin_mask]) = self.entity_shadow_pipelines[0].as_ref() else { return; };
        self.draw_depth_only_filtered(pass,camera,[(md3,skin),(md3_mask,skin_mask)],Some(entities));
    }

    fn draw_depth_only_filtered<'a>(
        &'a self, pass: &mut wgpu::RenderPass<'a>, camera: &'a wgpu::BindGroup,
        pipelines: [(&'a wgpu::RenderPipeline, &'a wgpu::RenderPipeline); 2],
        entities: Option<&std::collections::HashSet<u16>>,
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
                let masked = alpha_mode == DynamicModelAlphaMode::Mask;
                for (draw, indices) in super::submission::depth_runs(
                    self.draws.iter().filter(|draw| draw.alpha_mode == alpha_mode
                        && entities.is_none_or(|e| e.contains(&draw.entity_num))),
                    self.dynamic_batch_draws,
                    masked,
                ) {
                    if !bound {
                        pass.set_pipeline(pipeline);
                        // Opaque depth never samples the texture; one bind keeps
                        // the layout satisfied without per-draw state changes.
                        pass.set_bind_group(1, &self.fallback_bind_group, &[]);
                        bound = true;
                    }
                    if masked {
                        let bind_group = draw
                            .texture_key
                            .as_ref()
                            .and_then(|key| self.textures.get(key))
                            .map_or(&self.fallback_bind_group, |texture| &texture.bind_group);
                        pass.set_bind_group(1, bind_group, &[]);
                    }
                    pass.draw_indexed(indices, draw.base_vertex, 0..1);
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
                    let end = draw.first_instance.saturating_add(draw.instance_count);
                    if entities.is_none() {
                        pass.draw_indexed(0..mesh.index_count,0,draw.first_instance..end);
                        continue;
                    }
                    let mut start = draw.first_instance;
                    while start < end {
                        let eligible = |instance: u32| entities.is_none_or(|e|
                            self.skin_instance_entities.get(instance as usize).is_none_or(|id| e.contains(id)));
                        if !eligible(start) { start += 1; continue; }
                        let mut run_end = start + 1;
                        while run_end < end && eligible(run_end) { run_end += 1; }
                        pass.draw_indexed(0..mesh.index_count,0,start..run_end);
                        start = run_end;
                    }
                }
            }
        }
    }
}
