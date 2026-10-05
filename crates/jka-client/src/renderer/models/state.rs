//! Models state.
use crate::renderer::{
    create_entity_prepass_pipeline, dynamic_model_writes_depth, sampler_entry,
    storage_buffer_entry, texture_entry, uniform_entry, upload_texture, Arc,
    DynamicEntityLightingUniform, DynamicGpuTexture, DynamicModelAlphaMode, DynamicModelRenderer,
    DynamicModelVertex, EntityAmbientLightingMode, EntityLegacyFog, Entry, Ghoul2BatchMode,
    Ghoul2GpuVertex, Ghoul2PendingDraw, GpuImage, HashMap, PipelineJobManager, TextureData,
    DYNAMIC_MODEL_VERTEX_ATTRIBUTES, GHOUL2_GPU_VERTEX_ATTRIBUTES,
};
use std::hash::Hash;
use std::hash::Hasher;
use wgpu::util::DeviceExt;

impl DynamicModelRenderer {
    pub(in crate::renderer) fn new(
        device: &wgpu::Device,
        camera_layout: &wgpu::BindGroupLayout,
        surface_format: wgpu::TextureFormat,
        samples: u32,
        white: &GpuImage,
    ) -> Self {
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA dynamic model texture layout"),
            entries: &[
                texture_entry(0),
                sampler_entry(1),
                uniform_entry(2, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let entity_lighting_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("JKA dynamic model entity lighting uniform"),
            contents: bytemuck::bytes_of(&DynamicEntityLightingUniform::new(
                EntityAmbientLightingMode::Off,
                EntityLegacyFog::default(),
                1.0,
                [0.0; 4],
            )),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("JKA dynamic model pipeline layout"),
            bind_group_layouts: &[Some(camera_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("JKA dynamic model shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../md3.wgsl").into()),
        });
        let skin_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA Ghoul2 GPU skinning layout"),
            entries: &[
                storage_buffer_entry(0, true, wgpu::ShaderStages::VERTEX),
                storage_buffer_entry(1, true, wgpu::ShaderStages::VERTEX),
            ],
        });
        let skin_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("JKA Ghoul2 GPU skinning pipeline layout"),
            bind_group_layouts: &[
                Some(camera_layout),
                Some(&texture_layout),
                Some(&skin_layout),
            ],
            immediate_size: 0,
        });
        let skin_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("JKA Ghoul2 GPU skinning shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../ghoul2_skin.wgsl").into()),
        });
        let repeat_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("JKA dynamic model repeat sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let clamp_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("JKA dynamic model clamp sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let fallback_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("JKA dynamic model white fallback"),
            layout: &texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&white.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&clamp_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: entity_lighting_buffer.as_entire_binding(),
                },
            ],
        });
        let vertex_capacity = 4096;
        let index_capacity = 4096;
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA dynamic model vertex buffer"),
            size: vertex_capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA dynamic model index buffer"),
            size: index_capacity,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let fx_sprite_instance_capacity = 4096;
        let fx_sprite_instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA GPU FX sprite instance buffer"),
            size: fx_sprite_instance_capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let skin_bone_capacity = 4096;
        let skin_draw_capacity = 4096;
        let skin_bone_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA Ghoul2 GPU bone buffer"),
            size: skin_bone_capacity,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let skin_draw_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA Ghoul2 GPU draw buffer"),
            size: skin_draw_capacity,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let skin_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("JKA Ghoul2 GPU skinning bind group"),
            layout: &skin_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: skin_bone_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: skin_draw_buffer.as_entire_binding(),
                },
            ],
        });

        let md3_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<DynamicModelVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &DYNAMIC_MODEL_VERTEX_ATTRIBUTES,
        };
        let skin_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Ghoul2GpuVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &GHOUL2_GPU_VERTEX_ATTRIBUTES,
        };
        let prepass_pipeline = create_entity_prepass_pipeline(
            device,
            &pipeline_layout,
            &shader,
            md3_vertex_layout.clone(),
            false,
            "JKA dynamic model depth prepass pipeline",
        );
        let prepass_mask_pipeline = create_entity_prepass_pipeline(
            device,
            &pipeline_layout,
            &shader,
            md3_vertex_layout,
            true,
            "JKA dynamic model alpha-test depth prepass pipeline",
        );
        let skin_prepass_pipeline = create_entity_prepass_pipeline(
            device,
            &skin_pipeline_layout,
            &skin_shader,
            skin_vertex_layout.clone(),
            false,
            "JKA Ghoul2 GPU skin depth prepass pipeline",
        );
        let skin_prepass_mask_pipeline = create_entity_prepass_pipeline(
            device,
            &skin_pipeline_layout,
            &skin_shader,
            skin_vertex_layout,
            true,
            "JKA Ghoul2 GPU skin alpha-test depth prepass pipeline",
        );

        Self {
            texture_layout,
            entity_lighting_buffer,
            entity_ambient_lighting: EntityAmbientLightingMode::Off,
            entity_sun_relight: None,
            model_brightness: 1.0,
            entity_legacy_fog: EntityLegacyFog::default(),
            cloud_shadow_sun: [0.0; 4],
            legacy_fog_compiled: false,
            pipeline_layout,
            shader,
            wireframe_pipeline: None,
            model_pipelines: Default::default(),
            model_alpha_mask: 0,
            fx_sprite_shader: None,
            fx_sprite_surface_format: surface_format,
            fx_sprite_samples: samples,
            fx_sprite_wireframe_pipeline: None,
            fx_sprite_opaque_pipeline: None,
            fx_sprite_mask_pipeline: None,
            fx_sprite_blend_pipeline: None,
            fx_sprite_mask_blend_pipeline: None,
            fx_sprite_additive_one_pipeline: None,
            fx_sprite_additive_pipeline: None,
            fx_sprite_blend_unlit_pipeline: None,
            fx_sprite_modulate_pipeline: None,
            fx_sprite_dst_color_add_pipeline: None,
            fx_sprite_modulate2x_pipeline: None,
            fx_sprite_darken_pipeline: None,
            fx_zero_alpha_discard: false,
            skin_layout,
            skin_pipeline_layout,
            skin_shader,
            skin_wireframe_pipeline: None,
            skin_pipelines: Default::default(),
            skin_alpha_mask: 0,
            prepass_pipeline,
            prepass_mask_pipeline,
            skin_prepass_pipeline,
            skin_prepass_mask_pipeline,
            entity_shadow_pipelines: [None, None],
            repeat_sampler,
            clamp_sampler,
            fallback_bind_group,
            textures: HashMap::new(),
            vertex_buffer,
            index_buffer,
            vertex_capacity,
            index_capacity,
            draws: Vec::new(),
            fx_sprite_instance_buffer,
            fx_sprite_instance_capacity,
            fx_sprite_draws: Vec::new(),
            scratch_fx_sprite_instances: Vec::new(),
            blob_mesh_vertices: Vec::new(),
            blob_mesh_indices: Vec::new(),
            ghoul2_meshes: HashMap::new(),
            skin_bone_buffer,
            skin_draw_buffer,
            skin_bone_capacity,
            skin_draw_capacity,
            skin_bind_group,
            skin_draws: Vec::new(),
            ghoul2_batch_draws: Ghoul2BatchMode::Adaptive,
            companion_excluded_entity: None,
            ghoul2_input_instances: 0,
            ghoul2_encoded_draws: 0,
            scratch_vertices: Vec::new(),
            scratch_raw_colors: Vec::new(),
            rt_raw_color_buffer: None,
            scratch_indices: Vec::new(),
            scratch_bones: Vec::new(),
            scratch_draw_data: Vec::new(),
            scratch_bone_offsets: HashMap::new(),
            scratch_skin_pending: Vec::new(),
            rt_skinning: None,
            rt_prepared: false,
            frame_texture_keys: HashMap::new(),
        }
    }

    pub(in crate::renderer) fn set_ghoul2_batch_draws(&mut self, mode: Ghoul2BatchMode) {
        self.ghoul2_batch_draws = mode;
    }

    pub(in crate::renderer) fn ghoul2_batch_hash(draw: &Ghoul2PendingDraw) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        draw.alpha_mode.hash(&mut hasher);
        draw.mesh_key.hash(&mut hasher);
        draw.texture_key.hash(&mut hasher);
        hasher.finish()
    }

    pub(in crate::renderer) fn should_adaptively_batch(pending: &mut [Ghoul2PendingDraw]) -> bool {
        const MIN_SURFACES: usize = 32;
        const SAMPLE_SURFACES: usize = 64;
        const MIN_MATCHING_INSTANCES: u8 = 3;

        if pending.len() < MIN_SURFACES {
            return false;
        }

        // Start the real batching analysis, but only over a bounded set of
        // entries spread across the full frame.  The previous implementation
        // inspected only the first 64 surfaces, which could completely miss a
        // large repeated crowd later in the submission list.
        let sample_count = pending.len().min(SAMPLE_SURFACES);
        let mut counts: HashMap<u64, u8> = HashMap::with_capacity(sample_count);
        for sample in 0..sample_count {
            let index = if sample_count <= 1 {
                0
            } else {
                sample * (pending.len() - 1) / (sample_count - 1)
            };
            let draw = &mut pending[index];
            if !matches!(
                draw.alpha_mode,
                DynamicModelAlphaMode::Opaque | DynamicModelAlphaMode::Mask
            ) {
                continue;
            }
            let hash = if let Some(hash) = draw.batch_hash {
                hash
            } else {
                let hash = Self::ghoul2_batch_hash(draw);
                draw.batch_hash = Some(hash);
                hash
            };
            let count = counts.entry(hash).or_insert(0);
            *count = count.saturating_add(1);
            if *count >= MIN_MATCHING_INSTANCES {
                return true;
            }
        }
        false
    }

    pub(in crate::renderer) fn ghoul2_draw_stats(&self) -> (u32, u32) {
        (self.ghoul2_input_instances, self.ghoul2_encoded_draws)
    }

    pub(in crate::renderer) fn set_companion_excluded_entity(&mut self, entity: Option<u16>) {
        self.companion_excluded_entity = entity;
    }

    pub(in crate::renderer) fn ensure_vertex_capacity(
        &mut self,
        device: &wgpu::Device,
        required: u64,
    ) {
        if required <= self.vertex_capacity {
            return;
        }
        self.vertex_capacity = required.next_power_of_two().max(4096);
        self.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA dynamic model vertex buffer"),
            size: self.vertex_capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
    }

    pub(in crate::renderer) fn ensure_index_capacity(
        &mut self,
        device: &wgpu::Device,
        required: u64,
    ) {
        if required <= self.index_capacity {
            return;
        }
        self.index_capacity = required.next_power_of_two().max(4096);
        self.index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA dynamic model index buffer"),
            size: self.index_capacity,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
    }

    pub(in crate::renderer) fn set_fx_zero_alpha_discard(&mut self, enabled: bool) {
        if self.fx_zero_alpha_discard == enabled {
            return;
        }
        self.fx_zero_alpha_discard = enabled;
        // Pipelines are lazy; invalidate only source-alpha variants whose
        // fragment entry point actually changes for this A/B. GL_ONE, mask and
        // modulation variants stay hot and are not needlessly recompiled.
        self.fx_sprite_blend_pipeline = None;
        self.fx_sprite_additive_pipeline = None;
        self.fx_sprite_blend_unlit_pipeline = None;
    }

    pub(in crate::renderer) fn ensure_fx_sprite_instance_capacity(
        &mut self,
        device: &wgpu::Device,
        required: u64,
    ) {
        if required <= self.fx_sprite_instance_capacity {
            return;
        }
        self.fx_sprite_instance_capacity = required.next_power_of_two().max(4096);
        self.fx_sprite_instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA GPU FX sprite instance buffer"),
            size: self.fx_sprite_instance_capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
    }

    pub(in crate::renderer) fn ensure_skin_capacity(
        &mut self,
        device: &wgpu::Device,
        bone_required: u64,
        draw_required: u64,
    ) {
        let mut changed = false;
        if bone_required > self.skin_bone_capacity {
            self.skin_bone_capacity = bone_required.next_power_of_two().max(4096);
            self.skin_bone_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("JKA Ghoul2 GPU bone buffer"),
                size: self.skin_bone_capacity,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            changed = true;
        }
        if draw_required > self.skin_draw_capacity {
            self.skin_draw_capacity = draw_required.next_power_of_two().max(4096);
            self.skin_draw_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("JKA Ghoul2 GPU draw buffer"),
                size: self.skin_draw_capacity,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            changed = true;
        }
        if changed {
            self.skin_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("JKA Ghoul2 GPU skinning bind group"),
                layout: &self.skin_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.skin_bone_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.skin_draw_buffer.as_entire_binding(),
                    },
                ],
            });
            self.rebuild_rt_skinning_global_bind_group(device);
        }
    }

    pub(in crate::renderer) fn ensure_texture(
        &mut self,
        brightness: &mut crate::renderer::brightness::BakedBrightness,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: Option<&Arc<TextureData>>,
    ) -> Option<Arc<str>> {
        let texture = texture?;
        let pointer_key = Arc::as_ptr(texture) as usize;
        if let Some(key) = self.frame_texture_keys.get(&pointer_key) {
            return Some(Arc::clone(key));
        }

        let key: Arc<str> = Arc::from(format!(
            "{}|{}x{}|m{}|c{}|s{}",
            texture.label,
            texture.width,
            texture.height,
            texture.mip_level_count,
            u8::from(texture.clamp),
            u8::from(texture.srgb),
        ));
        if let Entry::Vacant(entry) = self.textures.entry(Arc::clone(&key)) {
            let image = upload_texture(device, queue, texture);
            brightness.register(&image._texture);
            let sampler = if texture.clamp {
                &self.clamp_sampler
            } else {
                &self.repeat_sampler
            };
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("JKA dynamic model texture {}", texture.label)),
                layout: &self.texture_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&image.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.entity_lighting_buffer.as_entire_binding(),
                    },
                ],
            });
            entry.insert(DynamicGpuTexture {
                _image: image,
                bind_group,
            });
        }
        self.frame_texture_keys
            .insert(pointer_key, Arc::clone(&key));
        Some(key)
    }

    pub(in crate::renderer) fn set_entity_ambient_lighting(
        &mut self,
        queue: &wgpu::Queue,
        mode: EntityAmbientLightingMode,
    ) {
        self.entity_ambient_lighting = mode;
        self.write_entity_lighting(queue);
    }

    /// Applies FogSystem::legacy_self_fog. Called only when fog settings or the
    /// map change; pipelines are respecialized only when fog turns on or off.
    pub(in crate::renderer) fn set_legacy_fog(
        &mut self,
        jobs: &mut PipelineJobManager,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        samples: u32,
        fog: EntityLegacyFog,
    ) {
        self.entity_legacy_fog = fog;
        self.write_entity_lighting(queue);
        let enabled = fog.params[0] > 0.5;
        if enabled != self.legacy_fog_compiled {
            self.legacy_fog_compiled = enabled;
            self.rebuild_pipelines(device, surface_format, samples);
            self.prewarm_core_model_pipelines(jobs, device);
        }
    }

    pub(in crate::renderer) fn write_entity_lighting(&self, queue: &wgpu::Queue) {
        queue.write_buffer(
            &self.entity_lighting_buffer,
            0,
            bytemuck::bytes_of(&DynamicEntityLightingUniform::new(
                self.entity_ambient_lighting,
                self.entity_legacy_fog,
                self.model_brightness,
                self.cloud_shadow_sun,
            )),
        );
    }

    /// Entity prepass input for projected cloud shadows. Called every frame but
    /// only touches the GPU when the direction or the on/off state changes.
    pub(in crate::renderer) fn set_cloud_shadow_sun(&mut self, queue: &wgpu::Queue, sun: [f32; 4]) {
        if sun == self.cloud_shadow_sun {
            return;
        }
        self.cloud_shadow_sun = sun;
        self.write_entity_lighting(queue);
    }

    pub(in crate::renderer) fn set_model_brightness(
        &mut self,
        queue: &wgpu::Queue,
        brightness: f32,
    ) {
        let brightness = if brightness.is_finite() {
            brightness.max(0.0)
        } else {
            1.0
        };
        if (self.model_brightness - brightness).abs() < f32::EPSILON {
            return;
        }
        self.model_brightness = brightness;
        self.write_entity_lighting(queue);
    }

    /// True when this frame's prepared entities include any depth-writing
    /// geometry, i.e. something the Entity map shadow pass would draw.
    pub(in crate::renderer) fn has_depth_casters(&self) -> bool {
        self.draws
            .iter()
            .any(|draw| dynamic_model_writes_depth(draw.alpha_mode))
            || self
                .skin_draws
                .iter()
                .any(|draw| dynamic_model_writes_depth(draw.alpha_mode))
    }
}
