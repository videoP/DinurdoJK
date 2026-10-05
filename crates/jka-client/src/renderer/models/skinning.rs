//! Models skinning.
use crate::renderer::{
    classic_vertex_illumination, ghoul2_specular_light, ghoul2_specular_viewer,
    sample_entity_classic_light, scene, storage_buffer_entry, Arc, DynamicModelAlphaMode,
    DynamicModelRenderer, DynamicModelSurface, DynamicModelVertex, DynamicPreparedDraw,
    EntityAmbientLightingMode, Entry, Ghoul2GpuDrawUniform, Ghoul2RtComputeBatch,
    Ghoul2RtComputeMesh, Ghoul2RtInputVertex, Ghoul2RtSkinningResources, HashMap, PipelineJobKey,
    PipelineJobManager, RayTracedSkinnedMeshKey, RayTracedSkinnedPrepared, Vec3,
};
use bytemuck::Zeroable;
use wgpu::util::DeviceExt;

impl DynamicModelRenderer {
    pub(in crate::renderer) fn ensure_rt_skinning_resources(
        &mut self,
        jobs: &mut PipelineJobManager,
        device: &wgpu::Device,
    ) -> bool {
        if self.rt_skinning.is_some() {
            return true;
        }
        let job_key = PipelineJobKey::new("ghoul2-rt-skinning", 0, 0);
        if let Some(resources) = jobs.take_ready::<Ghoul2RtSkinningResources>(job_key) {
            self.rt_skinning = Some(resources);
            rverbose!(1, "RT Shadows: Ghoul2 compute-skin path initialized (RT-only; normal GPU skinning unchanged when RT is off)");
            return true;
        }

        let device = device.clone();
        let skin_bone_buffer = self.skin_bone_buffer.clone();
        let skin_draw_buffer = self.skin_draw_buffer.clone();
        jobs.request(job_key, "Ghoul2 RT skinning compute pipeline", move || {
            let mesh_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("JKA Ghoul2 RT compute mesh layout"),
                entries: &[
                    storage_buffer_entry(0, true, wgpu::ShaderStages::COMPUTE),
                    storage_buffer_entry(1, true, wgpu::ShaderStages::COMPUTE),
                ],
            });
            let global_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("JKA Ghoul2 RT compute global layout"),
                entries: &[
                    storage_buffer_entry(0, true, wgpu::ShaderStages::COMPUTE),
                    storage_buffer_entry(1, true, wgpu::ShaderStages::COMPUTE),
                    storage_buffer_entry(2, false, wgpu::ShaderStages::COMPUTE),
                ],
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("JKA Ghoul2 RT compute skinning pipeline layout"),
                bind_group_layouts: &[Some(&mesh_layout), Some(&global_layout)],
                immediate_size: 0,
            });
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("JKA Ghoul2 RT compute skinning shader"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../../ghoul2_skin_compute.wgsl").into(),
                ),
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("JKA Ghoul2 RT compute skinning pipeline"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some("cs_main"),
                compilation_options: Default::default(),
                cache: None,
            });
            let vertex_capacity = 4096;
            let index_capacity = 4096;
            let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("JKA RT skinned dynamic vertex buffer"),
                size: vertex_capacity,
                usage: wgpu::BufferUsages::VERTEX
                    | wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::BLAS_INPUT,
                mapped_at_creation: false,
            });
            let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("JKA RT skinned dynamic index buffer"),
                size: index_capacity,
                usage: wgpu::BufferUsages::INDEX
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::BLAS_INPUT,
                mapped_at_creation: false,
            });
            let global_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("JKA Ghoul2 RT compute global bind group"),
                layout: &global_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: skin_bone_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: skin_draw_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: vertex_buffer.as_entire_binding(),
                    },
                ],
            });
            Ghoul2RtSkinningResources {
                mesh_layout,
                global_layout,
                pipeline,
                meshes: HashMap::new(),
                vertex_buffer,
                index_buffer,
                vertex_capacity,
                index_capacity,
                global_bind_group,
                batches: Vec::new(),
                casters: Vec::new(),
            }
        });
        false
    }

    pub(in crate::renderer) fn ensure_rt_skinning_capacity(
        &mut self,
        device: &wgpu::Device,
        vertex_required: u64,
        index_required: u64,
    ) {
        let Some(rt) = self.rt_skinning.as_mut() else {
            return;
        };
        let mut vertex_changed = false;
        if vertex_required > rt.vertex_capacity {
            rt.vertex_capacity = vertex_required.next_power_of_two().max(4096);
            rt.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("JKA RT skinned dynamic vertex buffer"),
                size: rt.vertex_capacity,
                usage: wgpu::BufferUsages::VERTEX
                    | wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::BLAS_INPUT,
                mapped_at_creation: false,
            });
            vertex_changed = true;
        }
        if index_required > rt.index_capacity {
            rt.index_capacity = index_required.next_power_of_two().max(4096);
            rt.index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("JKA RT skinned dynamic index buffer"),
                size: rt.index_capacity,
                usage: wgpu::BufferUsages::INDEX
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::BLAS_INPUT,
                mapped_at_creation: false,
            });
        }
        if vertex_changed {
            rt.global_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("JKA Ghoul2 RT compute global bind group"),
                layout: &rt.global_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.skin_bone_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.skin_draw_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: rt.vertex_buffer.as_entire_binding(),
                    },
                ],
            });
        }
    }

    pub(in crate::renderer) fn rebuild_rt_skinning_global_bind_group(
        &mut self,
        device: &wgpu::Device,
    ) {
        let Some(rt) = self.rt_skinning.as_mut() else {
            return;
        };
        rt.global_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("JKA Ghoul2 RT compute global bind group"),
            layout: &rt.global_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.skin_bone_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.skin_draw_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: rt.vertex_buffer.as_entire_binding(),
                },
            ],
        });
    }

    pub(in crate::renderer) fn encode_rt_skinning(&self, encoder: &mut wgpu::CommandEncoder) {
        if !self.rt_prepared {
            return;
        }
        let Some(rt) = self.rt_skinning.as_ref() else {
            return;
        };
        if rt.batches.is_empty() {
            return;
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("JKA Ghoul2 RT compute skinning"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&rt.pipeline);
        pass.set_bind_group(1, &rt.global_bind_group, &[]);
        for batch in &rt.batches {
            let Some(mesh) = rt.meshes.get(&batch.mesh_key) else {
                continue;
            };
            if mesh.vertex_count == 0 || batch.instance_count == 0 {
                continue;
            }
            pass.set_bind_group(0, &mesh.bind_group, &[]);
            pass.dispatch_workgroups(mesh.vertex_count.div_ceil(64), batch.instance_count, 1);
        }
    }

    pub(in crate::renderer) fn rt_skinned_casters(&self) -> &[RayTracedSkinnedPrepared] {
        self.rt_skinning
            .as_ref()
            .filter(|_| self.rt_prepared)
            .map_or(&[], |rt| rt.casters.as_slice())
    }

    pub(in crate::renderer) fn rt_skinning_buffers(
        &self,
    ) -> Option<(&wgpu::Buffer, &wgpu::Buffer)> {
        let rt = self.rt_skinning.as_ref().filter(|_| self.rt_prepared)?;
        Some((&rt.vertex_buffer, &rt.index_buffer))
    }

    pub(in crate::renderer) fn prepare_rt_skinned(
        &mut self,
        brightness: &mut crate::renderer::brightness::BakedBrightness,
        jobs: &mut PipelineJobManager,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surfaces: &[DynamicModelSurface],
        classic_grid: Option<&scene::ClassicEntityLightGrid>,
        rt_receivers: bool,
    ) -> bool {
        if !self.ensure_rt_skinning_resources(jobs, device) {
            return false;
        }
        let mut raw_colors = std::mem::take(&mut self.scratch_raw_colors);
        raw_colors.clear();
        self.draws.clear();
        self.fx_sprite_draws.clear();
        self.skin_draws.clear();
        self.frame_texture_keys.clear();
        self.ghoul2_input_instances = 0;
        self.ghoul2_encoded_draws = 0;

        let mut vertices = std::mem::take(&mut self.scratch_vertices);
        vertices.clear();
        let mut indices = std::mem::take(&mut self.scratch_indices);
        indices.clear();
        let mut bones = std::mem::take(&mut self.scratch_bones);
        bones.clear();
        let mut gpu_draw_data = std::mem::take(&mut self.scratch_draw_data);
        gpu_draw_data.clear();
        let mut bone_offsets = std::mem::take(&mut self.scratch_bone_offsets);
        bone_offsets.clear();
        let mut skin_pending = std::mem::take(&mut self.scratch_skin_pending);
        skin_pending.clear();

        let estimated_vertices = surfaces.iter().map(DynamicModelSurface::vertex_count).sum();
        let estimated_indices = surfaces.iter().map(DynamicModelSurface::index_count).sum();
        vertices.reserve(estimated_vertices);
        indices.reserve(estimated_indices);
        gpu_draw_data.reserve(surfaces.len());
        bone_offsets.reserve(surfaces.len());

        let mut mesh_draw_indices = HashMap::<Arc<str>, Vec<u32>>::new();
        let mut rt_casters = Vec::<RayTracedSkinnedPrepared>::new();
        let mut visible_gpu_draws = 0u32;

        for surface in surfaces {
            // Camera-culled RT-only surfaces should never leak into ordinary
            // entity rendering. Non-Ghoul2/non-RT transient surfaces have no
            // reason to survive the presentation cull.
            if !surface.raster_visible && surface.rt_skinned_key.is_none() {
                continue;
            }
            let texture_key = surface
                .raster_visible
                .then(|| self.ensure_texture(brightness, device, queue, surface.texture.as_ref()))
                .flatten();
            let classic_light = if surface.raster_visible
                && self.entity_ambient_lighting == EntityAmbientLightingMode::BspLightgridClassic
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
                let needs_mesh = self
                    .rt_skinning
                    .as_ref()
                    .is_none_or(|rt| !rt.meshes.contains_key(&mesh_key));
                if needs_mesh {
                    let packed = skin
                        .vertices
                        .iter()
                        .map(Ghoul2RtInputVertex::from)
                        .collect::<Vec<_>>();
                    let input_buffer =
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some(&format!("JKA Ghoul2 RT compute input {mesh_key}")),
                            contents: bytemuck::cast_slice(packed.as_slice()),
                            usage: wgpu::BufferUsages::STORAGE,
                        });
                    let draw_indices_capacity = 4u64;
                    let draw_indices_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some(&format!("JKA Ghoul2 RT draw indices {mesh_key}")),
                        size: draw_indices_capacity,
                        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    let rt = self.rt_skinning.as_mut().expect("created above");
                    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some(&format!("JKA Ghoul2 RT compute mesh bind group {mesh_key}")),
                        layout: &rt.mesh_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: input_buffer.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: draw_indices_buffer.as_entire_binding(),
                            },
                        ],
                    });
                    rt.meshes.insert(
                        Arc::clone(&mesh_key),
                        Ghoul2RtComputeMesh {
                            input_buffer,
                            draw_indices_buffer,
                            draw_indices_capacity,
                            bind_group,
                            vertex_count: u32::try_from(skin.vertices.len()).unwrap_or(u32::MAX),
                        },
                    );
                }

                let base_vertex = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
                let first_index = u32::try_from(indices.len()).unwrap_or(u32::MAX);
                vertices.resize(
                    vertices.len().saturating_add(skin.vertices.len()),
                    DynamicModelVertex::zeroed(),
                );
                if rt_receivers {
                    raw_colors.resize(vertices.len(), skin.color);
                }
                indices.extend_from_slice(skin.indices.as_slice());

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
                let draw_index = u32::try_from(gpu_draw_data.len()).unwrap_or(u32::MAX);
                gpu_draw_data.push(Ghoul2GpuDrawUniform {
                    axis0: [skin.axis[0][0], skin.axis[0][1], skin.axis[0][2], 0.0],
                    axis1: [skin.axis[1][0], skin.axis[1][1], skin.axis[1][2], 0.0],
                    axis2: [skin.axis[2][0], skin.axis[2][1], skin.axis[2][2], 0.0],
                    origin: [skin.origin[0], skin.origin[1], skin.origin[2], 0.0],
                    color: skin.color,
                    params: [
                        bone_base,
                        u32::from(classic_light.is_some()),
                        base_vertex,
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
                });
                mesh_draw_indices
                    .entry(Arc::clone(&mesh_key))
                    .or_default()
                    .push(draw_index);
                self.ghoul2_input_instances = self.ghoul2_input_instances.saturating_add(1);

                if surface.raster_visible {
                    self.draws.push(DynamicPreparedDraw {
                        entity_num: surface.entity_num,
                        first_index,
                        index_count: u32::try_from(skin.indices.len()).unwrap_or(u32::MAX),
                        base_vertex: i32::try_from(base_vertex).unwrap_or(i32::MAX),
                        texture_key,
                        alpha_mode: surface.alpha_mode,
                        wireframe_class: surface.wireframe_class,
                    });
                    visible_gpu_draws = visible_gpu_draws.saturating_add(1);
                }
                if matches!(
                    surface.alpha_mode,
                    DynamicModelAlphaMode::Opaque | DynamicModelAlphaMode::Mask
                ) {
                    if let Some(mesh_key) = surface.rt_skinned_key.as_ref() {
                        rt_casters.push(RayTracedSkinnedPrepared {
                            key: RayTracedSkinnedMeshKey {
                                entity_num: surface.entity_num,
                                mesh_key: Arc::clone(mesh_key),
                                non_opaque: surface.alpha_mode == DynamicModelAlphaMode::Mask,
                            },
                            first_vertex: base_vertex,
                            vertex_count: u32::try_from(skin.vertices.len()).unwrap_or(u32::MAX),
                            first_index,
                            index_count: u32::try_from(skin.indices.len()).unwrap_or(u32::MAX),
                            alpha_texture: surface.texture.clone(),
                        });
                    }
                }
                continue;
            }

            if surface.vertices.is_empty() || surface.indices.is_empty() {
                continue;
            }
            let base_vertex = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
            let first_index = u32::try_from(indices.len()).unwrap_or(u32::MAX);
            if rt_receivers {
                raw_colors.extend(surface.vertices.iter().map(|vertex| vertex.color));
            }
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
            // RT-on keeps each surface's original local index values and uses
            // base_vertex at raster time. The same ranges can therefore feed a
            // BLAS without repacking another index stream.
            indices.extend_from_slice(surface.indices.as_slice());
            if surface.raster_visible {
                self.draws.push(DynamicPreparedDraw {
                    entity_num: surface.entity_num,
                    first_index,
                    index_count: u32::try_from(surface.indices.len()).unwrap_or(u32::MAX),
                    base_vertex: i32::try_from(base_vertex).unwrap_or(i32::MAX),
                    texture_key,
                    alpha_mode: surface.alpha_mode,
                    wireframe_class: surface.wireframe_class,
                });
            }
            if matches!(
                surface.alpha_mode,
                DynamicModelAlphaMode::Opaque | DynamicModelAlphaMode::Mask
            ) {
                if let Some(mesh_key) = surface.rt_skinned_key.as_ref() {
                    rt_casters.push(RayTracedSkinnedPrepared {
                        key: RayTracedSkinnedMeshKey {
                            entity_num: surface.entity_num,
                            mesh_key: Arc::clone(mesh_key),
                            non_opaque: surface.alpha_mode == DynamicModelAlphaMode::Mask,
                        },
                        first_vertex: base_vertex,
                        vertex_count: u32::try_from(surface.vertices.len()).unwrap_or(u32::MAX),
                        first_index,
                        index_count: u32::try_from(surface.indices.len()).unwrap_or(u32::MAX),
                        alpha_texture: surface.texture.clone(),
                    });
                }
            }
        }

        // Material shell/custom clones can reference the same physical Ghoul2
        // geometry. Prefer an opaque occurrence over a masked shell because the
        // opaque surface already blocks the complete triangle in JKA's depth
        // semantics; otherwise retain one masked caster with its alpha material.
        let mut chosen = HashMap::<(u16, Arc<str>), usize>::new();
        let mut deduped = Vec::<RayTracedSkinnedPrepared>::with_capacity(rt_casters.len());
        for caster in rt_casters.drain(..) {
            let physical = (caster.key.entity_num, Arc::clone(&caster.key.mesh_key));
            if let Some(&index) = chosen.get(&physical) {
                if deduped[index].key.non_opaque && !caster.key.non_opaque {
                    deduped[index] = caster;
                }
                continue;
            }
            chosen.insert(physical, deduped.len());
            deduped.push(caster);
        }
        rt_casters = deduped;

        self.ghoul2_encoded_draws = visible_gpu_draws;
        if rt_receivers && !raw_colors.is_empty() {
            debug_assert_eq!(raw_colors.len(), vertices.len());
            let bytes = bytemuck::cast_slice(raw_colors.as_slice());
            let size = bytes.len() as u64;
            if self
                .rt_raw_color_buffer
                .as_ref()
                .is_none_or(|buffer| buffer.size() < size)
            {
                self.rt_raw_color_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("RT model original material colors"),
                    size: size.next_power_of_two().max(16),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            }
            queue.write_buffer(self.rt_raw_color_buffer.as_ref().unwrap(), 0, bytes);
        }
        self.scratch_raw_colors = raw_colors;
        let vertex_bytes = bytemuck::cast_slice(vertices.as_slice());
        let index_bytes = bytemuck::cast_slice(indices.as_slice());
        let bone_bytes = bytemuck::cast_slice(bones.as_slice());
        let draw_bytes = bytemuck::cast_slice(gpu_draw_data.as_slice());

        self.ensure_skin_capacity(device, bone_bytes.len() as u64, draw_bytes.len() as u64);
        self.ensure_rt_skinning_capacity(
            device,
            vertex_bytes.len() as u64,
            index_bytes.len() as u64,
        );
        let rt = self.rt_skinning.as_mut().expect("created above");
        if !vertex_bytes.is_empty() {
            queue.write_buffer(&rt.vertex_buffer, 0, vertex_bytes);
        }
        if !index_bytes.is_empty() {
            queue.write_buffer(&rt.index_buffer, 0, index_bytes);
        }
        if !bone_bytes.is_empty() {
            queue.write_buffer(&self.skin_bone_buffer, 0, bone_bytes);
        }
        if !draw_bytes.is_empty() {
            queue.write_buffer(&self.skin_draw_buffer, 0, draw_bytes);
        }

        rt.batches.clear();
        for (mesh_key, draw_indices) in mesh_draw_indices {
            if draw_indices.is_empty() {
                continue;
            }
            let mesh = rt
                .meshes
                .get_mut(&mesh_key)
                .expect("created while preparing surface");
            let bytes = bytemuck::cast_slice(draw_indices.as_slice());
            if bytes.len() as u64 > mesh.draw_indices_capacity {
                mesh.draw_indices_capacity = (bytes.len() as u64).next_power_of_two().max(4);
                mesh.draw_indices_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&format!("JKA Ghoul2 RT draw indices {mesh_key}")),
                    size: mesh.draw_indices_capacity,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                mesh.bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(&format!("JKA Ghoul2 RT compute mesh bind group {mesh_key}")),
                    layout: &rt.mesh_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: mesh.input_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: mesh.draw_indices_buffer.as_entire_binding(),
                        },
                    ],
                });
            }
            queue.write_buffer(&mesh.draw_indices_buffer, 0, bytes);
            rt.batches.push(Ghoul2RtComputeBatch {
                mesh_key,
                instance_count: u32::try_from(draw_indices.len()).unwrap_or(u32::MAX),
            });
        }
        rt.casters = rt_casters;
        self.rt_prepared = true;
        self.ensure_model_pipelines(jobs, device);

        self.scratch_vertices = vertices;
        self.scratch_indices = indices;
        self.scratch_bones = bones;
        self.scratch_draw_data = gpu_draw_data;
        self.scratch_bone_offsets = bone_offsets;
        self.scratch_skin_pending = skin_pending;
        true
    }
}
