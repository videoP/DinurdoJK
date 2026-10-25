//! Models state.
use crate::renderer::{
    create_entity_prepass_pipeline, create_entity_rain_prepass_pipeline,
    dynamic_model_writes_depth, sampler_entry, scene, Vec3,
    storage_buffer_entry, texture_entry, uniform_entry, upload_texture, Arc,
    DynamicEntityLightingUniform, DynamicGpuTexture, DynamicModelAlphaMode, DynamicModelRenderer,
    DynamicModelVertex, EntityAmbientLightingMode, EntityLegacyFog, EntityWeather, Entry,
    Ghoul2BatchMode,
    Ghoul2GpuVertex, Ghoul2PendingDraw, GpuImage, HashMap, PipelineJobManager, TextureData,
    DYNAMIC_MODEL_VERTEX_ATTRIBUTES, GHOUL2_GPU_VERTEX_ATTRIBUTES,
};
use std::hash::Hash;
use std::hash::Hasher;
use wgpu::util::DeviceExt;

impl DynamicModelRenderer {
    pub(in crate::renderer) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera_layout: &wgpu::BindGroupLayout,
        glass_layout: &wgpu::BindGroupLayout,
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
                // Source 1 `$bumpmap`. Shared with md3.wgsl and the RT receiver
                // shaders, which simply do not declare it: wgpu only requires a
                // shader's bindings to be a subset of its layout.
                texture_entry(3),
            ],
        });
        let entity_lighting_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("JKA dynamic model entity lighting uniform"),
            contents: bytemuck::bytes_of(&DynamicEntityLightingUniform::new(
                EntityAmbientLightingMode::Off,
                EntityLegacyFog::default(),
                1.0,
                [0.0; 4],
                EntityWeather::default(),
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
            source: wgpu::ShaderSource::Wgsl(
                crate::glass::model_shader(include_str!("../../md3.wgsl"), 2).into(),
            ),
        });
        let skin_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA Ghoul2 GPU skinning layout"),
            entries: &[
                storage_buffer_entry(0, true, wgpu::ShaderStages::VERTEX),
                // The per-draw table is also read by the fragment stage, which
                // is where a Source 1 material's phong and the light it reacts
                // to are evaluated per pixel.
                storage_buffer_entry(1, true, wgpu::ShaderStages::VERTEX_FRAGMENT),
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
        // Glass variants also sample the shared opaque-scene snapshot. Every
        // other pipeline keeps the original layouts so no draw path has to
        // bind a group it does not read.
        let glass_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("JKA dynamic model glass pipeline layout"),
            bind_group_layouts: &[Some(camera_layout), Some(&texture_layout), Some(glass_layout)],
            immediate_size: 0,
        });
        let glass_skin_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("JKA Ghoul2 GPU skinning glass pipeline layout"),
            bind_group_layouts: &[
                Some(camera_layout),
                Some(&texture_layout),
                Some(&skin_layout),
                Some(glass_layout),
            ],
            immediate_size: 0,
        });
        let skin_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("JKA Ghoul2 GPU skinning shader"),
            source: wgpu::ShaderSource::Wgsl(
                crate::glass::model_shader(include_str!("../../ghoul2_skin.wgsl"), 3)
                    .into(),
            ),
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
        let flat_normal = upload_texture(
            device,
            queue,
            &TextureData {
                label: "JKA flat tangent-space normal".to_owned(),
                source: None,
                width: 1,
                height: 1,
                // Tangent space's own +Z once decoded by `tex * 2 - 1`.
                rgba: vec![128, 128, 255, 255],
                rgba16f: None,
                mip_level_count: 1,
                clamp: true,
                // A normal map is data, never colour.
                srgb: false,
            },
        );
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
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&flat_normal.view),
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
            md3_vertex_layout.clone(),
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
            skin_vertex_layout.clone(),
            true,
            "JKA Ghoul2 GPU skin alpha-test depth prepass pipeline",
        );
        let rain_prepass_pipeline = create_entity_rain_prepass_pipeline(
            device,
            &pipeline_layout,
            &shader,
            md3_vertex_layout.clone(),
            false,
            "JKA dynamic model rain lean depth prepass pipeline",
        );
        let rain_prepass_mask_pipeline = create_entity_rain_prepass_pipeline(
            device,
            &pipeline_layout,
            &shader,
            md3_vertex_layout,
            true,
            "JKA dynamic model alpha-test rain lean depth prepass pipeline",
        );
        let skin_rain_prepass_pipeline = create_entity_rain_prepass_pipeline(
            device,
            &skin_pipeline_layout,
            &skin_shader,
            skin_vertex_layout.clone(),
            false,
            "JKA Ghoul2 GPU skin rain lean depth prepass pipeline",
        );
        let skin_rain_prepass_mask_pipeline = create_entity_rain_prepass_pipeline(
            device,
            &skin_pipeline_layout,
            &skin_shader,
            skin_vertex_layout,
            true,
            "JKA Ghoul2 GPU skin alpha-test rain lean depth prepass pipeline",
        );

        Self {
            camera_layout: camera_layout.clone(),
            texture_layout,
            entity_lighting_buffer,
            entity_ambient_lighting: EntityAmbientLightingMode::Off,
            entity_sun_relight: None,
            entity_dlights: Vec::new(),
            entity_dlights_rt_per_pixel: false,
            model_brightness: 1.0,
            entity_legacy_fog: EntityLegacyFog::default(),
            entity_weather: EntityWeather::default(),
            entity_water: Arc::new(std::collections::HashMap::new()),
            cloud_shadow_sun: [0.0; 4],
            legacy_fog_compiled: false,
            model_wet_compiled: true,
            pipeline_layout,
            glass_pipeline_layout,
            glass_skin_pipeline_layout,
            shader,
            wireframe_pipeline: None,
            model_pipelines: Default::default(),
            studio_capture_pipelines: Default::default(),
            model_glow_pipelines: Default::default(),
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
            fx_sprite_glow_pipelines: Default::default(),
            fx_zero_alpha_discard: false,
            skin_layout,
            skin_pipeline_layout,
            skin_shader,
            skin_wireframe_pipeline: None,
            skin_pipelines: Default::default(),
            skin_glow_pipelines: Default::default(),
            skin_source_pipelines: Default::default(),
            skin_alpha_mask: 0,
            skin_source_mask: 0,
            prepass_pipeline,
            prepass_mask_pipeline,
            skin_prepass_pipeline,
            skin_prepass_mask_pipeline,
            rain_prepass_pipeline,
            rain_prepass_mask_pipeline,
            skin_rain_prepass_pipeline,
            skin_rain_prepass_mask_pipeline,
            entity_shadow_pipelines: [None, None],
            repeat_sampler,
            clamp_sampler,
            flat_normal,
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
            resident_fx: None,
            resident_fx_frame: None,
            resident_fx_requested_modes: 0,
            resident_fx_draws: Vec::new(),
            resident_fx_slots: Vec::new(),
            scratch_fx_sprite_instances: Vec::new(),
            blob_mesh_vertices: Vec::new(),
            blob_mesh_indices: Vec::new(),
            ghoul2_meshes: HashMap::new(),
            skin_bone_buffer,
            skin_draw_buffer,
            skin_bone_capacity,
            skin_draw_capacity,
            skin_bind_group,
            glass_bind_group: None,
            skin_draws: Vec::new(),
            skin_instance_entities: Vec::new(),
            ghoul2_batch_draws: Ghoul2BatchMode::Adaptive,
            model_state_cache: false,
            dynamic_batch_draws: true,
            // Startup applies VideoSettings' serial default through the
            // fallible pool setter.
            model_prepare_threads: 0,
            cpu_pack_pool: None,
            scratch_cpu_pack_jobs: Vec::new(),
            model_render_bundles: false,
            model_bundles: Default::default(),
            perf_bundles: Default::default(),
            md3_geometry_cache: true,
            md3_stream_cache: Default::default(),
            model_index_upload_cache: true,
            index_upload_cache: super::prepare::IndexUploadCache::default(),
            ghoul2_batch_key_cache: true,
            next_texture_binding_id: 1,
            next_ghoul2_group_id: 1,
            capture_prepare: false,
            perf_prepare: super::prepare::PreparePerf::default(),
            perf_phase_calls: std::cell::Cell::new(super::submission::ModelPhaseCalls::default()),
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
            scratch_batch_counts: HashMap::new(),
            scratch_group_lookup: HashMap::new(),
            scratch_frame_groups: Vec::new(),
            scratch_group_sort: Vec::new(),
            scratch_group_order: Vec::new(),
            rt_skinning: None,
            rt_prepared: false,
            frame_texture_keys: HashMap::new(),
        }
    }

    pub(in crate::renderer) fn set_ghoul2_batch_draws(&mut self, mode: Ghoul2BatchMode) {
        self.ghoul2_batch_draws = mode;
    }

    pub(in crate::renderer) fn set_model_prepare_threads(&mut self, threads: u32) -> Result<(), String> {
        if !matches!(threads, 0 | 1 | 2 | 4) { return Err("expected 0|1|2|4".into()); }
        if threads == self.model_prepare_threads { return Ok(()); }
        let pool = if threads > 1 {
            Some(rayon::ThreadPoolBuilder::new().num_threads(threads as usize)
                .thread_name(|i| format!("jka-model-pack-{i}"))
                .build().map_err(|error| error.to_string())?)
        } else { None };
        self.cpu_pack_pool = pool;
        self.model_prepare_threads = threads;
        Ok(())
    }

    pub(in crate::renderer) fn set_md3_geometry_cache(&mut self, enabled: bool) {
        if self.md3_geometry_cache != enabled {
            self.md3_stream_cache.clear();
            self.index_upload_cache.clear();
            self.md3_geometry_cache = enabled;
        }
    }

    pub(in crate::renderer) fn set_model_index_upload_cache(&mut self, enabled: bool) {
        if self.model_index_upload_cache != enabled {
            self.index_upload_cache.clear();
            self.model_index_upload_cache = enabled;
        }
    }

    pub(in crate::renderer) fn record_cpu_geometry(&mut self,
        source: crate::renderer::ModelGeometrySource, vertices: usize, indices: usize) {
        if self.capture_prepare {
            let count = &mut self.perf_prepare.cpu_sources[source as usize];
            count.surfaces += 1;
            count.vertices += vertices as u64;
            count.indices += indices as u64;
        }
    }

    pub(in crate::renderer) fn ghoul2_batch_hash(
        draw: &Ghoul2PendingDraw,
        prepare_authored_glow: bool,
    ) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        draw.alpha_mode.hash(&mut hasher);
        draw.source_material.hash(&mut hasher);
        draw.mesh_key.hash(&mut hasher);
        draw.texture_key.hash(&mut hasher);
        // Glow Off keeps the original batching key and grouping behavior.
        if prepare_authored_glow {
            draw.authored_glow.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Probe the complete frame in O(n), instead of sampling 64 entries and
    /// missing repeated skins at the tail of a crowd. Only enable the sort
    /// when at least two indexed draws could be eliminated. An individual
    /// two-instance surface is legal to batch, but rarely repays a full sort.
    /// A hash collision can only cause an unnecessary sort: exact keys are
    /// checked again by the grouping pass, and never merged by hash alone.
    pub(in crate::renderer) fn should_adaptively_batch(
        pending: &mut [Ghoul2PendingDraw],
        prepare_authored_glow: bool,
        separate_wireframe_classes: bool,
        excluded_entity: Option<u16>,
        counts: &mut HashMap<(u64, u32), u32>,
    ) -> bool {
        const MIN_SAVED_DRAWS: usize = 2;
        counts.clear();
        let mut saved_draws = 0usize;
        for draw in pending {
            if !matches!(draw.alpha_mode, DynamicModelAlphaMode::Opaque | DynamicModelAlphaMode::Mask)
                || Some(draw.entity_num) == excluded_entity
            {
                continue;
            }
            let hash = match draw.batch_hash {
                Some(hash) => hash,
                None => {
                    let hash = Self::ghoul2_batch_hash(draw, prepare_authored_glow);
                    draw.batch_hash = Some(hash);
                    hash
                }
            };
            let wireframe = if separate_wireframe_classes {
                draw.wireframe_class.mask_bit()
            } else { 0 };
            let count = counts.entry((hash, wireframe)).or_insert(0);
            if *count != 0 {
                saved_draws += 1;
                if saved_draws >= MIN_SAVED_DRAWS { return true; }
            }
            *count += 1;
        }
        false
    }

    /// Cached, linear-time gather of visible GLM surfaces. Precomputed unique
    /// group IDs encode invariant mesh/material compatibility; the only work
    /// left on the render thread is bucketing *visible* instances. Sort the
    /// number of distinct groups instead of sorting all surface submissions.
    /// Unlike snapshot-cached visibility this also handles interpolation,
    /// local prediction, LOD, material and companion changes between snaps.
    pub(in crate::renderer) fn prepare_cached_ghoul2_order(
        pending: &[Ghoul2PendingDraw],
        separate_wireframe_classes: bool,
        excluded_entity: Option<u16>,
        mode: Ghoul2BatchMode,
        lookup: &mut HashMap<(u64, u32), usize>,
        groups: &mut Vec<super::prepare::Ghoul2FrameGroup>,
        group_sort: &mut Vec<usize>,
        order: &mut Vec<usize>,
    ) -> bool {
        lookup.clear();
        order.clear();
        group_sort.clear();
        // Reuse the high-water bucket allocation. Clear only buckets actually
        // visited in this frame; old, unused buckets may remain untouched.
        if mode == Ghoul2BatchMode::Off || pending.len() < 2 {
            return false;
        }
        let mut used = 0usize;
        let mut saved = 0usize;
        for (index, draw) in pending.iter().enumerate() {
            if !matches!(draw.alpha_mode, DynamicModelAlphaMode::Opaque | DynamicModelAlphaMode::Mask)
                || Some(draw.entity_num) == excluded_entity
            {
                continue;
            }
            let Some(id) = draw.batch_group_id else { continue };
            let wire = if separate_wireframe_classes { draw.wireframe_class.mask_bit() } else { 0 };
            let key = (id, wire);
            let group_index = match lookup.entry(key) {
                Entry::Occupied(entry) => {
                    saved += 1;
                    *entry.get()
                }
                Entry::Vacant(entry) => {
                    let group_index = used;
                    used += 1;
                    if group_index == groups.len() {
                        groups.push(super::prepare::Ghoul2FrameGroup::default());
                    }
                    let group = &mut groups[group_index];
                    group.indices.clear();
                    group.sort_key = (
                        u8::from(matches!(draw.alpha_mode, DynamicModelAlphaMode::Mask)),
                        wire,
                        draw.source_material,
                        draw.texture_binding_id,
                        id,
                    );
                    entry.insert(group_index);
                    group_index
                }
            };
            groups[group_index].indices.push(index);
        }
        if mode == Ghoul2BatchMode::Adaptive && saved < 2 {
            return false;
        }
        group_sort.extend(0..used);
        group_sort.sort_unstable_by_key(|&i| groups[i].sort_key);
        for &i in group_sort.iter() {
            order.extend_from_slice(&groups[i].indices);
        }
        // Preserve order for translucent and companion-excluded surfaces.
        for (index, draw) in pending.iter().enumerate() {
            if !matches!(draw.alpha_mode, DynamicModelAlphaMode::Opaque | DynamicModelAlphaMode::Mask)
                || Some(draw.entity_num) == excluded_entity
                || draw.batch_group_id.is_none()
            {
                order.push(index);
            }
        }
        debug_assert_eq!(order.len(), pending.len());
        true
    }

    /// Sort only commutative opaque/masked GLM draws. Transparent and
    /// companion-excluded surfaces retain their relative submission order.
    pub(in crate::renderer) fn sort_ghoul2_pending(
        pending: &mut [Ghoul2PendingDraw],
        prepare_authored_glow: bool,
        separate_wireframe_classes: bool,
        companion_excluded_entity: Option<u16>,
    ) {
        pending.sort_unstable_by(|a, b| {
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
                        // A Source VMT changes the pipeline. Keep it as a
                        // primary sort dimension rather than interleaving
                        // incompatible Source/non-Source draws of a mesh.
                        .then_with(|| a.source_material.cmp(&b.source_material))
                        // Multiple GLM meshes commonly share a skin. Sort
                        // by the existing GPU texture binding instead of
                        // randomizing material locality with a mesh hash.
                        .then_with(|| a.texture_binding_id.cmp(&b.texture_binding_id))
                        .then_with(|| a.texture_key.as_deref().cmp(&b.texture_key.as_deref()))
                        .then_with(|| a.batch_hash.cmp(&b.batch_hash))
                        .then_with(|| a.mesh_key.as_ref().cmp(b.mesh_key.as_ref()))
                        .then_with(|| if prepare_authored_glow {
                            a.authored_glow.cmp(&b.authored_glow)
                        } else { std::cmp::Ordering::Equal })
                }
            }
        });
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
        self.index_upload_cache.clear();
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

    /// Cache key for one `TextureData`. Identity is the label plus everything
    /// that changes the GPU resource, because two decodes of the same file must
    /// share a bind group while a repack must not reuse a stale one.
    fn texture_identity(texture: &TextureData) -> String {
        format!(
            "{}|{}x{}|m{}|c{}|s{}",
            texture.label,
            texture.width,
            texture.height,
            texture.mip_level_count,
            u8::from(texture.clamp),
            u8::from(texture.srgb),
        )
    }

    /// Bind group for one surface's material: its colour map and, for Source 1
    /// materials, its `$bumpmap`.
    ///
    /// Both halves are part of the key. A bind group holds *two* texture views,
    /// so keying on the colour map alone would let two surfaces that share a
    /// base texture but differ in normal map collide and silently take each
    /// other's bind group.
    pub(in crate::renderer) fn ensure_texture(
        &mut self,
        brightness: &mut crate::renderer::brightness::BakedBrightness,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: Option<&Arc<TextureData>>,
        normal_map: Option<&Arc<TextureData>>,
    ) -> Option<Arc<str>> {
        let texture = texture?;
        let pointer_key = (
            Arc::as_ptr(texture) as usize,
            normal_map.map_or(0, |normal| Arc::as_ptr(normal) as usize),
        );
        if let Some(key) = self.frame_texture_keys.get(&pointer_key) {
            return Some(Arc::clone(key));
        }

        let key: Arc<str> = Arc::from(match normal_map {
            None => Self::texture_identity(texture),
            Some(normal) => format!(
                "{}|n:{}",
                Self::texture_identity(texture),
                Self::texture_identity(normal)
            ),
        });
        if let Entry::Vacant(entry) = self.textures.entry(Arc::clone(&key)) {
            let binding_id = self.next_texture_binding_id;
            self.next_texture_binding_id += 1;
            let image = upload_texture(device, queue, texture);
            brightness.register(&image._texture);
            // Deliberately not registered with baked brightness: a normal map is
            // data, not colour, and `register` already skips non-sRGB textures.
            let normal_image = normal_map.map(|normal| upload_texture(device, queue, normal));
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
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(
                            normal_image
                                .as_ref()
                                .map_or(&self.flat_normal.view, |normal| &normal.view),
                        ),
                    },
                ],
            });
            entry.insert(DynamicGpuTexture {
                binding_id,
                _image: image,
                _normal_image: normal_image,
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

    /// Compiles the rain/water wetness path in or out of the model pipelines.
    /// Decided per map (rain, wetness, water surfaces, any entity in water), so
    /// pipelines are respecialized at map load, not while playing.
    pub(in crate::renderer) fn set_model_wet(
        &mut self,
        jobs: &mut PipelineJobManager,
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        samples: u32,
        wet: bool,
    ) {
        if wet != self.model_wet_compiled {
            self.model_wet_compiled = wet;
            self.rebuild_pipelines(device, surface_format, samples);
            self.prewarm_core_model_pipelines(jobs, device);
        }
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
                self.entity_weather,
            )),
        );
    }

    /// Wet weather on entities (model_weather.wgsl). Called every frame; only
    /// touches the GPU when the sun or the wetness actually changes, so a dry map
    /// costs one struct comparison a frame and no upload.
    pub(in crate::renderer) fn set_entity_weather(
        &mut self,
        queue: &wgpu::Queue,
        weather: EntityWeather,
    ) {
        if weather == self.entity_weather {
            return;
        }
        self.entity_weather = weather;
        self.write_entity_lighting(queue);
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
    pub(in crate::renderer) fn shadow_skin_sphere(&self, skin:&crate::renderer::Ghoul2GpuSkinning)->Option<(Vec3,f32)> {
        let mesh=self.ghoul2_meshes.get(&skin.mesh_key)?;
        // Frobenius norms bound arbitrary rotations/scales. Convex blend weights
        // cannot exceed the largest individual bone bound.
        let norm=|a:[f32;3],b:[f32;3],c:[f32;3]|(Vec3::from_array(a).length_squared()+Vec3::from_array(b).length_squared()+Vec3::from_array(c).length_squared()).sqrt();
        let mut radius=0.0f32;
        for bone in skin.bones.iter() {
            let linear=norm([bone.row0[0],bone.row0[1],bone.row0[2]],[bone.row1[0],bone.row1[1],bone.row1[2]],[bone.row2[0],bone.row2[1],bone.row2[2]]);
            radius=radius.max(linear*mesh.bind_radius+Vec3::new(bone.row0[3],bone.row1[3],bone.row2[3]).length());
        }
        let jiggle=skin.jiggle_offsets.iter().map(|v|Vec3::new(v[0],v[1],v[2]).length()+v[3].abs()).fold(0.0,f32::max);
        radius=(radius+jiggle+skin.bulge_height.abs())*norm(skin.axis[0],skin.axis[1],skin.axis[2]);
        let origin=Vec3::from_array(scene::render_position(skin.origin));
        (radius.is_finite() && origin.is_finite()).then_some((origin,radius+4.0))
    }

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
