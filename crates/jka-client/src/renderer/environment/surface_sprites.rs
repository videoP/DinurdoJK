//! Environment surface sprites.
pub(in crate::renderer) use crate::scene::surface_sprite_bake::{
    surface_sprite_byte, surface_sprite_random, SurfaceSpriteBake, SurfaceSpriteChunk,
};
#[cfg(test)]
use crate::scene::surface_sprite_bake::{bucket_resident_instances, build_vegetation_instances, SurfaceSpriteInstance};
use crate::renderer::{
    aabb_intersects_clip_frustum, camera_depth_compare, gpu_blend_factor, pipeline_hash,
    sampler_entry, scene, storage_buffer_entry, texture_entry, uniform_entry, BTreeMap,
    BlendFactor, BlendFunc, Camera, Instant, Mat4, PipelineJobKey, PipelineJobManager, Pod, Vec3,
    Zeroable, DEPTH_FORMAT,
};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub(in crate::renderer) struct SurfaceSpriteEffectVertex {
    pub(in crate::renderer) position: [f32; 3],
    pub(in crate::renderer) uv: [f32; 2],
    pub(in crate::renderer) color: [f32; 4],
    pub(in crate::renderer) alpha_cutoff: f32,
}

pub(in crate::renderer) const SURFACE_SPRITE_EFFECT_ATTRIBUTES: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x4, 3 => Float32];

pub(in crate::renderer) struct SurfaceSpriteEffectGpu {
    pub(in crate::renderer) source: scene::SurfaceSpriteEffectEmitter,
    pub(in crate::renderer) bind_group: wgpu::BindGroup,
}

/// Per-frame cost of native surfaceSprites, reported by `r_perftrace`.
#[derive(Clone, Copy, Default, Debug)]
pub(in crate::renderer) struct SurfaceSpriteStats {
    pub(in crate::renderer) cpu_ms: f64,
    /// Sprites generated on the CPU this frame (effect kind, or every kind with
    /// `r_surfaceSpritesGpu 0`).
    pub(in crate::renderer) cpu_sprites: u32,
    /// Resident instances submitted after chunk culling (the vertex shader
    /// still fades out the ones beyond their authored distance).
    pub(in crate::renderer) resident_instances: u32,
    pub(in crate::renderer) draws: u32,
    pub(in crate::renderer) chunks_visible: u32,
    pub(in crate::renderer) chunks_total: u32,
    pub(in crate::renderer) upload_bytes: u64,
}

#[derive(Default)]
pub(in crate::renderer) struct PreparedSurfaceSpriteEffects {
    pub(in crate::renderer) draws: Vec<SurfaceSpriteEffectDraw>,
    pub(in crate::renderer) stats: SurfaceSpriteStats,
}

pub(in crate::renderer) struct SurfaceSpriteEffectDraw {
    pub(in crate::renderer) emitter: usize,
    /// CPU path: vertex range. Resident path: instance range.
    pub(in crate::renderer) vertices: std::ops::Range<u32>,
    pub(in crate::renderer) resident: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub(in crate::renderer) struct SurfaceSpriteResidentParams {
    /// fadedist^2, 1 / (cutdist^2 - fadedist^2), fade range, ssFadescale.
    pub(in crate::renderer) fade: [f32; 4],
    /// ssWind, ssWindIdle, alpha cutoff, unused.
    pub(in crate::renderer) wind: [f32; 4],
    /// hang_down, flattened, oriented, additive.
    pub(in crate::renderer) flags: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug, Default)]
pub(in crate::renderer) struct SurfaceSpriteResidentFrame {
    /// Camera position (JKA) and shader time in seconds.
    pub(in crate::renderer) camera_seconds: [f32; 4],
    pub(in crate::renderer) right: [f32; 4],
    pub(in crate::renderer) forward: [f32; 4],
    /// Normalized weather wind direction (xy) and strength (z).
    pub(in crate::renderer) wind: [f32; 4],
}

/// Map-lifetime GPU data for the resident path.
pub(in crate::renderer) struct ResidentSurfaceSprites {
    _instances: wgpu::Buffer,
    _params: wgpu::Buffer,
    pub(in crate::renderer) bind_group: wgpu::BindGroup,
    pub(in crate::renderer) chunks: Vec<SurfaceSpriteChunk>,
    /// Chunk range per emitter; empty for emitters left on the CPU path.
    pub(in crate::renderer) emitter_chunks: Vec<std::ops::Range<u32>>,
}

pub(in crate::renderer) struct SurfaceSpriteEffectRenderer {
    pub(in crate::renderer) texture_layout: wgpu::BindGroupLayout,
    pub(in crate::renderer) pipeline_layout: wgpu::PipelineLayout,
    pub(in crate::renderer) shader: wgpu::ShaderModule,
    pub(in crate::renderer) resident_layout: wgpu::BindGroupLayout,
    pub(in crate::renderer) resident_pipeline_layout: wgpu::PipelineLayout,
    pub(in crate::renderer) resident_shader: wgpu::ShaderModule,
    pub(in crate::renderer) resident_frame: wgpu::Buffer,
    /// r_surfaceSpritesGpu: 1 resident instances for vegetation kinds, 0 the
    /// original per-frame CPU generation for everything.
    pub(in crate::renderer) resident_enabled: bool,
    /// r_surfaceSprites 0 draws no native surfaceSprites (Godot grass is separate).
    pub(in crate::renderer) enabled: bool,
    /// Index 0 CPU path, 1 resident path.
    pub(in crate::renderer) wireframe_pipelines: [Option<wgpu::RenderPipeline>; 2],
    pub(in crate::renderer) pipelines: BTreeMap<(Option<BlendFunc>, bool, bool), wgpu::RenderPipeline>,
    pub(in crate::renderer) vertex_buffer: wgpu::Buffer,
    pub(in crate::renderer) vertex_capacity_bytes: u64,
    pub(in crate::renderer) surface_format: wgpu::TextureFormat,
    pub(in crate::renderer) samples: u32,
}

impl SurfaceSpriteEffectRenderer {
    pub(in crate::renderer) fn new(
        device: &wgpu::Device,
        camera_layout: &wgpu::BindGroupLayout,
        surface_format: wgpu::TextureFormat,
        samples: u32,
    ) -> Self {
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA surfaceSprites effect texture layout"),
            entries: &[texture_entry(0), sampler_entry(1)],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("JKA surfaceSprites effect pipeline layout"),
            bind_group_layouts: &[Some(camera_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("JKA surfaceSprites effect shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../surface_sprite_effect.wgsl").into(),
            ),
        });
        let resident_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA surfaceSprites resident layout"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::VERTEX),
                storage_buffer_entry(1, true, wgpu::ShaderStages::VERTEX),
                storage_buffer_entry(2, true, wgpu::ShaderStages::VERTEX),
            ],
        });
        let resident_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("JKA surfaceSprites resident pipeline layout"),
            bind_group_layouts: &[Some(camera_layout), Some(&texture_layout), Some(&resident_layout)],
            immediate_size: 0,
        });
        let resident_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("JKA surfaceSprites resident shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../surface_sprite_resident.wgsl").into(),
            ),
        });
        let resident_frame = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA surfaceSprites resident frame"),
            size: std::mem::size_of::<SurfaceSpriteResidentFrame>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let vertex_capacity_bytes = 64 * 1024;
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA surfaceSprites effect dynamic vertices"),
            size: vertex_capacity_bytes,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            texture_layout,
            pipeline_layout,
            shader,
            resident_layout,
            resident_pipeline_layout,
            resident_shader,
            resident_frame,
            resident_enabled: true,
            enabled: true,
            wireframe_pipelines: [None, None],
            pipelines: BTreeMap::new(),
            vertex_buffer,
            vertex_capacity_bytes,
            surface_format,
            samples,
        }
    }

    pub(in crate::renderer) fn rebuild_pipelines(
        &mut self,
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        samples: u32,
    ) {
        self.surface_format = surface_format;
        self.samples = samples;
        self.wireframe_pipelines = [None, None];
        self.pipelines.clear();
        // Pipelines are recreated lazily for only the blend functions the map uses.
        let _ = device;
    }

    pub(in crate::renderer) fn ensure_wireframe_pipeline(
        &mut self,
        jobs: &mut PipelineJobManager,
        device: &wgpu::Device,
    ) {
        for resident in [false, true] {
            let slot = usize::from(resident);
            if self.wireframe_pipelines[slot].is_some() {
                continue;
            }
            let key = PipelineJobKey::new(
                if resident { "surface-sprites-resident-wireframe" } else { "surface-sprites-wireframe" },
                0,
                pipeline_hash(&(self.surface_format, self.samples)),
            );
            if let Some(pipeline) = jobs.take_ready(key) {
                self.wireframe_pipelines[slot] = Some(pipeline);
                continue;
            }
            let recipe = self.pipeline_recipe(device, resident);
            jobs.request(key, "surfaceSprites wireframe", move || recipe.create(None, false, true));
        }
    }

    fn pipeline_recipe(&self, device: &wgpu::Device, resident: bool) -> SurfaceSpritePipelineRecipe {
        SurfaceSpritePipelineRecipe {
            device: device.clone(),
            layout: if resident { self.resident_pipeline_layout.clone() } else { self.pipeline_layout.clone() },
            shader: if resident { self.resident_shader.clone() } else { self.shader.clone() },
            resident,
            surface_format: self.surface_format,
            samples: self.samples,
        }
    }

    pub(in crate::renderer) fn create_texture_bind_group(
        &self,
        device: &wgpu::Device,
        view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("JKA surfaceSprites effect texture"),
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }

    pub(in crate::renderer) fn ensure_pipeline(
        &mut self,
        jobs: &mut PipelineJobManager,
        device: &wgpu::Device,
        blend: Option<BlendFunc>,
        depth_write: bool,
        resident: bool,
    ) {
        let variant = (blend, depth_write, resident);
        if self.pipelines.contains_key(&variant) {
            return;
        }
        let blend_id = pipeline_hash(&(blend, depth_write));
        let key = PipelineJobKey::new(
            if resident { "surface-sprites-resident" } else { "surface-sprites" },
            blend_id as u32,
            pipeline_hash(&(self.surface_format, self.samples, blend_id)),
        );
        if let Some(pipeline) = jobs.take_ready(key) {
            self.pipelines.insert(variant, pipeline);
            return;
        }
        let recipe = self.pipeline_recipe(device, resident);
        jobs.request(key, "surfaceSprites blend variant", move || {
            recipe.create(blend, depth_write, false)
        });
    }

    /// Uploads the map-prep bake (`scene::surface_sprite_bake`): instances,
    /// per-emitter parameters and the chunk index used for CPU culling.
    pub(in crate::renderer) fn build_resident(
        &self,
        device: &wgpu::Device,
        effects: &[SurfaceSpriteEffectGpu],
        bake: &SurfaceSpriteBake,
    ) -> Option<ResidentSurfaceSprites> {
        use wgpu::util::DeviceExt;
        if bake.instances.is_empty() || bake.emitter_chunks.len() != effects.len() {
            return None;
        }
        let params: Vec<_> = effects.iter().map(|effect| resident_params(&effect.source)).collect();
        let instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("JKA surfaceSprites resident instances"),
            contents: bytemuck::cast_slice(&bake.instances),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("JKA surfaceSprites resident emitters"),
            contents: bytemuck::cast_slice(&params),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("JKA surfaceSprites resident"),
            layout: &self.resident_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.resident_frame.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: instance_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: params_buffer.as_entire_binding() },
            ],
        });
        Some(ResidentSurfaceSprites {
            _instances: instance_buffer,
            _params: params_buffer,
            bind_group,
            chunks: bake.chunks.clone(),
            emitter_chunks: bake.emitter_chunks.clone(),
        })
    }

    pub(in crate::renderer) fn prepare_draw(
        &mut self,
        jobs: &mut PipelineJobManager,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        effects: &[SurfaceSpriteEffectGpu],
        resident: Option<&ResidentSurfaceSprites>,
        camera: &Camera,
        view_proj: Mat4,
        pvs_cluster: Option<usize>,
        time_ms: f32,
        godot_grass_active: bool,
        weather_wind: crate::ocean::AtmosphericWind,
    ) -> PreparedSurfaceSpriteEffects {
        if !self.enabled {
            return PreparedSurfaceSpriteEffects::default();
        }
        let started = Instant::now();
        let mut stats = SurfaceSpriteStats::default();
        let resident = resident.filter(|_| self.resident_enabled);
        let mut vertices = Vec::<SurfaceSpriteEffectVertex>::new();
        let mut draws = Vec::new();
        let camera_jka = Vec3::from_array(scene::jka_position(camera.position.to_array()));
        let forward = camera.forward();
        let screen_right = forward.cross(Vec3::Y).normalize_or_zero();
        // OpenJK viewParms.ori.axis[1] is -AngleVectors(right), i.e. the
        // engine's historical "left" basis even though surface-sprite code
        // names it ssViewRight. Preserve that sign so legacy splash textures
        // are not mirrored relative to retail JKA.
        let sprite_right_render = -screen_right;
        let up_render = screen_right.cross(forward).normalize_or_zero();
        let right_jka = Vec3::from_array(scene::jka_position(sprite_right_render.to_array()));
        let up_jka = Vec3::from_array(scene::jka_position(up_render.to_array()));
        let mut resident_used = false;

        for (emitter_index, effect) in effects.iter().enumerate() {
            if !surface_sprite_signature_visible(&effect.source.pvs_signature, pvs_cluster)
                || (godot_grass_active && effect.source.godot_candidate)
            {
                continue;
            }
            let resident_chunks = resident
                .and_then(|resident| resident.emitter_chunks.get(emitter_index).cloned())
                .filter(|chunks| !chunks.is_empty());
            if let (Some(resident), Some(chunk_range)) = (resident, resident_chunks) {
                // Raven's per-sprite fade reaches zero at cutdist; a chunk
                // whose nearest point is past it cannot draw anything.
                let sprite = effect.source.sprite;
                let cutdist = sprite.fade_max.max(sprite.fade_dist.max(0.0) + 0.001);
                let mut open: Option<std::ops::Range<u32>> = None;
                let flush = |range: std::ops::Range<u32>, draws: &mut Vec<SurfaceSpriteEffectDraw>, stats: &mut SurfaceSpriteStats| {
                    stats.resident_instances += range.end - range.start;
                    draws.push(SurfaceSpriteEffectDraw { emitter: emitter_index, vertices: range, resident: true });
                };
                for chunk in &resident.chunks[chunk_range.start as usize..chunk_range.end as usize] {
                    stats.chunks_total += 1;
                    let min = Vec3::from_array(chunk.min_jka);
                    let max = Vec3::from_array(chunk.max_jka);
                    let nearest = camera_jka.clamp(min, max);
                    let render_min = [min.x, min.z, -max.y];
                    let render_max = [max.x, max.z, -min.y];
                    if nearest.distance_squared(camera_jka) > cutdist * cutdist
                        || !aabb_intersects_clip_frustum(render_min, render_max, view_proj)
                    {
                        continue;
                    }
                    stats.chunks_visible += 1;
                    match open.as_mut() {
                        Some(range) if range.end == chunk.instances.start => range.end = chunk.instances.end,
                        _ => {
                            if let Some(range) = open.replace(chunk.instances.clone()) {
                                flush(range, &mut draws, &mut stats);
                            }
                        }
                    }
                }
                // Earlier ranges were flushed as gaps opened; `open` is the last.
                if let Some(range) = open {
                    self.ensure_pipeline(jobs, device, effect.source.blend, effect.source.depth_write, true);
                    flush(range, &mut draws, &mut stats);
                    resident_used = true;
                }
                continue;
            }
            let start = vertices.len() as u32;
            match effect.source.sprite.kind {
                jka_assets::shader::SurfaceSpriteType::Effect => append_jka_effect_surface_sprites(
                    &effect.source, camera_jka, right_jka, up_jka, time_ms, &mut vertices,
                ),
                jka_assets::shader::SurfaceSpriteType::Vertical
                | jka_assets::shader::SurfaceSpriteType::Flattened
                | jka_assets::shader::SurfaceSpriteType::Oriented => append_jka_vegetation_surface_sprites(
                    &effect.source, camera_jka, right_jka, time_ms, weather_wind, &mut vertices,
                ),
            }
            let end = vertices.len() as u32;
            if end > start {
                self.ensure_pipeline(jobs, device, effect.source.blend, effect.source.depth_write, false);
                draws.push(SurfaceSpriteEffectDraw {
                    emitter: emitter_index,
                    vertices: start..end,
                    resident: false,
                });
            }
        }

        if resident_used {
            let frame = resident_frame(camera_jka, right_jka, time_ms, weather_wind);
            queue.write_buffer(&self.resident_frame, 0, bytemuck::bytes_of(&frame));
            stats.upload_bytes += std::mem::size_of::<SurfaceSpriteResidentFrame>() as u64;
        }
        let required_bytes =
            (vertices.len() * std::mem::size_of::<SurfaceSpriteEffectVertex>()) as u64;
        if required_bytes > self.vertex_capacity_bytes {
            self.vertex_capacity_bytes = required_bytes.next_power_of_two().max(64 * 1024);
            self.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("JKA surfaceSprites effect dynamic vertices"),
                size: self.vertex_capacity_bytes,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if required_bytes != 0 {
            queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&vertices));
        }
        stats.upload_bytes += required_bytes;
        stats.cpu_sprites = (vertices.len() / 6) as u32;
        stats.draws = draws.len() as u32;
        stats.cpu_ms = started.elapsed().as_secs_f64() * 1000.0;
        PreparedSurfaceSpriteEffects { draws, stats }
    }

    pub(in crate::renderer) fn draw<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        camera_bind_group: &'a wgpu::BindGroup,
        effects: &'a [SurfaceSpriteEffectGpu],
        resident: Option<&'a ResidentSurfaceSprites>,
        prepared: &PreparedSurfaceSpriteEffects,
    ) {
        if prepared.draws.is_empty() {
            return;
        }
        pass.set_bind_group(0, camera_bind_group, &[]);
        let mut cpu_bound = false;
        let mut resident_bound = false;
        for draw in &prepared.draws {
            let effect = &effects[draw.emitter];
            let Some(pipeline) = self.pipelines.get(&(effect.source.blend, effect.source.depth_write, draw.resident)) else {
                continue;
            };
            if draw.resident {
                let Some(resident) = resident else { continue };
                if !resident_bound {
                    pass.set_bind_group(2, &resident.bind_group, &[]);
                    resident_bound = true;
                }
                pass.set_pipeline(pipeline);
                pass.set_bind_group(1, &effect.bind_group, &[]);
                pass.draw(0..6, draw.vertices.clone());
            } else {
                if !cpu_bound {
                    pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                    cpu_bound = true;
                }
                pass.set_pipeline(pipeline);
                pass.set_bind_group(1, &effect.bind_group, &[]);
                pass.draw(draw.vertices.clone(), 0..1);
            }
        }
    }

    pub(in crate::renderer) fn draw_wireframe<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        camera_bind_group: &'a wgpu::BindGroup,
        effects: &'a [SurfaceSpriteEffectGpu],
        resident: Option<&'a ResidentSurfaceSprites>,
        prepared: &PreparedSurfaceSpriteEffects,
    ) {
        if prepared.draws.is_empty() {
            return;
        }
        pass.set_bind_group(0, camera_bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        for draw in &prepared.draws {
            let Some(pipeline) = self.wireframe_pipelines[usize::from(draw.resident)].as_ref() else {
                continue;
            };
            // The wireframe shader does not sample the sprite texture, but this
            // pipeline intentionally shares the normal surfaceSprites pipeline
            // layout. WGPU therefore still requires bind-group slot 1 to be
            // layout-compatible at draw time. Explicitly bind the emitter's
            // surfaceSprites group so stale dynamic-model/world bindings can
            // never leak across debug overlay draws.
            let effect = &effects[draw.emitter];
            pass.set_pipeline(pipeline);
            pass.set_bind_group(1, &effect.bind_group, &[]);
            if draw.resident {
                let Some(resident) = resident else { continue };
                pass.set_bind_group(2, &resident.bind_group, &[]);
                pass.draw(0..6, draw.vertices.clone());
            } else {
                pass.draw(draw.vertices.clone(), 0..1);
            }
        }
    }
}

/// Everything a surfaceSprites pipeline job needs off the render thread.
struct SurfaceSpritePipelineRecipe {
    device: wgpu::Device,
    layout: wgpu::PipelineLayout,
    shader: wgpu::ShaderModule,
    resident: bool,
    surface_format: wgpu::TextureFormat,
    samples: u32,
}

impl SurfaceSpritePipelineRecipe {
    fn create(&self, blend: Option<BlendFunc>, depth_write: bool, wireframe: bool) -> wgpu::RenderPipeline {
        let gpu_blend = blend.map(|blend| {
            let component = wgpu::BlendComponent {
                src_factor: gpu_blend_factor(blend.src),
                dst_factor: gpu_blend_factor(blend.dst),
                operation: wgpu::BlendOperation::Add,
            };
            wgpu::BlendState {
                color: component,
                alpha: component,
            }
        });
        // The resident path pulls instances from storage; it has no vertex buffer.
        let cpu_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<SurfaceSpriteEffectVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &SURFACE_SPRITE_EFFECT_ATTRIBUTES,
        })];
        self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(match (self.resident, wireframe) {
                (false, false) => "JKA surfaceSprites effect pipeline",
                (false, true) => "JKA surfaceSprites wireframe pipeline",
                (true, false) => "JKA surfaceSprites resident pipeline",
                (true, true) => "JKA surfaceSprites resident wireframe pipeline",
            }),
            layout: Some(&self.layout),
            vertex: wgpu::VertexState {
                module: &self.shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: if self.resident { &[] } else { &cpu_buffers },
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: if wireframe { wgpu::PolygonMode::Line } else { wgpu::PolygonMode::Fill },
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(depth_write && !wireframe),
                depth_compare: Some(if wireframe { wgpu::CompareFunction::Always } else { camera_depth_compare() }),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: self.samples,
                ..Default::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: &self.shader,
                entry_point: Some(if wireframe { "fs_wireframe" } else { "fs_main" }),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: self.surface_format,
                    blend: if wireframe { None } else { gpu_blend },
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: crate::pipeline_cache::get().as_ref(),
        })
    }
}

fn resident_params(emitter: &scene::SurfaceSpriteEffectEmitter) -> SurfaceSpriteResidentParams {
    use jka_assets::shader::{SurfaceSpriteFacing, SurfaceSpriteType};
    const FADE_RANGE: f32 = 250.0;
    let sprite = emitter.sprite;
    let fadedist = sprite.fade_dist.max(0.0);
    let cutdist = sprite.fade_max.max(fadedist + 0.001);
    let fadedist2 = fadedist * fadedist;
    let additive = emitter.blend.is_some_and(|blend| {
        blend.src == BlendFactor::One && blend.dst == BlendFactor::One
    });
    SurfaceSpriteResidentParams {
        fade: [
            fadedist2,
            1.0 / (cutdist * cutdist - fadedist2),
            (FADE_RANGE / (cutdist - fadedist)).min(1.0),
            sprite.fade_scale,
        ],
        wind: [sprite.wind, sprite.wind_idle, emitter.alpha_cutoff, 0.0],
        flags: [
            u32::from(matches!(sprite.facing, SurfaceSpriteFacing::HangDown)),
            u32::from(matches!(sprite.kind, SurfaceSpriteType::Flattened)),
            u32::from(matches!(sprite.kind, SurfaceSpriteType::Oriented)),
            u32::from(additive),
        ],
    }
}

pub(in crate::renderer) fn resident_frame(
    camera_jka: Vec3,
    right_jka: Vec3,
    time_ms: f32,
    weather_wind: crate::ocean::AtmosphericWind,
) -> SurfaceSpriteResidentFrame {
    let seconds = time_ms * 0.001;
    let forward_jka = right_jka.cross(Vec3::Z).normalize_or_zero();
    let wind = Vec3::from_array({
        let v = weather_wind.at(seconds);
        [v[0], v[1], 0.0]
    });
    let strength = (wind.length() / 180.0).clamp(0.0, 1.0);
    let direction = wind.normalize_or_zero();
    SurfaceSpriteResidentFrame {
        camera_seconds: [camera_jka.x, camera_jka.y, camera_jka.z, seconds],
        right: [right_jka.x, right_jka.y, right_jka.z, 0.0],
        forward: [forward_jka.x, forward_jka.y, forward_jka.z, 0.0],
        wind: [direction.x, direction.y, strength, 0.0],
    }
}

pub(in crate::renderer) fn surface_sprite_signature_visible(
    signature: &[u64],
    cluster: Option<usize>,
) -> bool {
    let Some(cluster) = cluster else {
        return true;
    };
    if signature.is_empty() {
        return true;
    }
    let word = cluster / 64;
    let bit = cluster % 64;
    signature
        .get(word)
        .is_none_or(|value| *value & (1_u64 << bit) != 0)
}

pub(in crate::renderer) fn append_surface_sprite_quad(
    loc: Vec3,
    width: f32,
    height: f32,
    face_up: bool,
    view_right: Vec3,
    view_up: Vec3,
    light: f32,
    alpha: f32,
    alpha_cutoff: f32,
    out: &mut Vec<SurfaceSpriteEffectVertex>,
) {
    let points = if face_up {
        // RB_EffectSurfaceSprite intentionally makes face-up effects square:
        // it halves height too, but only the half-width is used for X/Y.
        let half = width * 0.5;
        [
            Vec3::new(loc.x + half, loc.y - half, loc.z + 1.0),
            Vec3::new(loc.x + half, loc.y + half, loc.z + 1.0),
            Vec3::new(loc.x - half, loc.y + half, loc.z + 1.0),
            Vec3::new(loc.x - half, loc.y - half, loc.z + 1.0),
        ]
    } else {
        let top = loc + view_up * height;
        let right = view_right * (width * 0.5);
        [loc + right, top + right, top - right, loc - right]
    };
    let uv = [[1.0, 1.0], [1.0, 0.0], [0.0, 0.0], [0.0, 1.0]];
    let color = [light, light, light, alpha];
    let converted = points.map(|point| scene::render_position(point.to_array()));
    for corner in [0usize, 1, 2, 0, 2, 3] {
        out.push(SurfaceSpriteEffectVertex {
            position: converted[corner],
            uv: uv[corner],
            color,
            alpha_cutoff,
        });
    }
}

pub(in crate::renderer) fn append_jka_effect_surface_sprites(
    emitter: &scene::SurfaceSpriteEffectEmitter,
    camera_jka: Vec3,
    view_right_jka: Vec3,
    view_up_jka: Vec3,
    time_ms: f32,
    out: &mut Vec<SurfaceSpriteEffectVertex>,
) {
    const FADE_RANGE: f32 = 250.0;
    let sprite = emitter.sprite;
    let cut_dist = sprite.fade_max.max(sprite.fade_dist + 0.001);
    let fade_dist = sprite.fade_dist;
    let cut_dist2 = cut_dist * cut_dist;
    let fade_dist2 = fade_dist * fade_dist;
    let inv_fade_diff = 1.0 / (cut_dist2 - fade_dist2);
    let fade_range = (FADE_RANGE / (cut_dist - fade_dist)).min(1.0);
    let face_up = !matches!(
        sprite.facing,
        jka_assets::shader::SurfaceSpriteFacing::Normal
    );
    let min_normal = if face_up { 0.99 } else { 0.5 };
    let fx_alpha = sprite.fx_alpha_end - sprite.fx_alpha_start;
    let fade_in_out = sprite.fx_alpha_end < 0.05 && sprite.height >= 0.1 && sprite.width >= 0.1;
    let additive = emitter
        .blend
        .is_some_and(|blend| blend.src == BlendFactor::One && blend.dst == BlendFactor::One);

    for triangle in &emitter.triangles {
        if triangle.normal_z.iter().any(|normal| *normal < min_normal) {
            continue;
        }
        let v = triangle.positions_jka.map(Vec3::from_array);
        let alpha_at = v.map(|point| {
            1.0 - ((camera_jka - point).length_squared() - fade_dist2) * inv_fade_diff
        });
        if alpha_at.iter().all(|alpha| *alpha <= 0.0) {
            continue;
        }
        let e12 = v[1] - v[0];
        let e13 = v[2] - v[0];
        let tri_area = (e13.x * e12.y - e13.y * e12.x).abs();
        if tri_area <= 1.0 || !tri_area.is_finite() {
            continue;
        }
        let step = sprite.density / tri_area.sqrt();
        if !step.is_finite() || step <= 0.0 {
            continue;
        }

        let mut random_index =
            surface_sprite_byte(v[0].x + v[0].y + v[1].x + v[1].y + v[2].x + v[2].y);
        let random_interval = surface_sprite_byte(v[0].x + v[1].y + v[2].z) | 0x03;
        let mut pos_i = 0.0_f32;
        while pos_i < 1.0 {
            let mut pos_j = 0.0_f32;
            while pos_j < 1.0 - pos_i {
                let effect_time =
                    (time_ms + 10000.0 * surface_sprite_random(random_index)) / sprite.fx_duration;
                let effect_cycle = effect_time.trunc() as i64;
                let effect_pos = effect_time - effect_time.trunc();
                let mut random_index2 =
                    random_index.wrapping_add(effect_cycle.rem_euclid(256) as u8);
                random_index = random_index.wrapping_add(random_interval);

                let fa = pos_i + surface_sprite_random(random_index2) * step;
                random_index2 = random_index2.wrapping_add(1);
                if fa <= 1.0 {
                    let fb = pos_j + surface_sprite_random(random_index2) * step;
                    random_index2 = random_index2.wrapping_add(1);
                    if fb <= 1.0 - fa {
                        let fc = 1.0 - fa - fb;
                        let alpha_pos = alpha_at[0] * fa + alpha_at[1] * fb + alpha_at[2] * fc;
                        let this_fade_start =
                            fade_range + (1.0 - fade_range) * surface_sprite_random(random_index2);
                        random_index2 = random_index2.wrapping_add(random_interval);
                        let _ = random_index2;
                        let mut alpha =
                            1.0 - ((this_fade_start - alpha_pos) / fade_range.max(1.0e-6));
                        if alpha > 0.0 {
                            alpha = alpha.min(1.0);
                            let point = v[0] * fa + v[1] * fb + v[2] * fc;
                            let mut light = f32::from(triangle.light[0]) * fa
                                + f32::from(triangle.light[1]) * fb
                                + f32::from(triangle.light[2]) * fc;

                            let mut size_index = random_index;
                            let variance = surface_sprite_random(size_index);
                            let mut width = sprite.width * (1.0 + sprite.variance[0] * variance);
                            let mut height = sprite.height * (1.0 + sprite.variance[1] * variance);
                            size_index = size_index.wrapping_add(1);
                            width += effect_pos * sprite.fx_grow[0] * width;
                            height += effect_pos * sprite.fx_grow[1] * height;

                            if fade_in_out {
                                if effect_pos > 0.5 {
                                    alpha *=
                                        sprite.fx_alpha_start + fx_alpha * (effect_pos - 0.5) * 2.0;
                                } else {
                                    alpha *=
                                        sprite.fx_alpha_start + fx_alpha * (0.5 - effect_pos) * 2.0;
                                }
                            } else {
                                alpha *= sprite.fx_alpha_start + fx_alpha * effect_pos;
                            }

                            if additive {
                                light = (128.0 + light * 0.5) * alpha;
                                alpha = 1.0;
                            }
                            if surface_sprite_random(size_index) > 0.5 {
                                width = -width;
                            }
                            if sprite.fade_scale != 0.0 && alpha_pos < 1.0 {
                                width *= 1.0 + sprite.fade_scale * (1.0 - alpha_pos);
                            }

                            append_surface_sprite_quad(
                                point,
                                width,
                                height,
                                face_up,
                                view_right_jka,
                                view_up_jka,
                                (light / 255.0).clamp(0.0, 1.0),
                                alpha.clamp(0.0, 1.0),
                                emitter.alpha_cutoff,
                                out,
                            );
                        }
                    }
                }
                pos_j += step;
            }
            pos_i += step;
        }
    }
}

/// TaystJK rd-vanilla/tr_surfacesprites.cpp: RB_DrawVerticalSurfaceSprites,
/// RB_VerticalSurfaceSprite and RB_DrawOrientedSurfaceSprites. Native sprites
/// are *not* a shader pass on the parent BSP polygon: every iteration emits
/// a camera-facing (or flattened/oriented) textured quad.
pub(in crate::renderer) fn append_jka_vegetation_surface_sprites(
    emitter: &scene::SurfaceSpriteEffectEmitter,
    camera_jka: Vec3,
    view_right_jka: Vec3,
    time_ms: f32,
    weather_wind: crate::ocean::AtmosphericWind,
    out: &mut Vec<SurfaceSpriteEffectVertex>,
) {
    use jka_assets::shader::{SurfaceSpriteFacing, SurfaceSpriteType};

    const FADE_RANGE: f32 = 250.0;
    // Protect the render thread from malformed shader densities, without
    // changing the authored spacing for normal JKA materials.
    const MAX_SPRITES_PER_TRIANGLE: usize = 16384;
    const MAX_CELLS_PER_TRIANGLE: usize = 65536;
    let sprite = emitter.sprite;
    if !sprite.width.is_finite() || !sprite.height.is_finite()
        || !sprite.density.is_finite() || sprite.density <= 0.0
        || sprite.width <= 0.0 || sprite.height <= 0.0
    {
        return;
    }
    let fadedist = sprite.fade_dist.max(0.0);
    let cutdist = sprite.fade_max.max(fadedist + 0.001);
    let fadedist2 = fadedist * fadedist;
    let inv_fade_diff = 1.0 / (cutdist * cutdist - fadedist2);
    let fade_range = (FADE_RANGE / (cutdist - fadedist)).min(1.0);
    let hang_down = matches!(sprite.facing, SurfaceSpriteFacing::HangDown);
    let any_angle = matches!(sprite.facing, SurfaceSpriteFacing::AnyAngle);
    let flattened = matches!(sprite.kind, SurfaceSpriteType::Flattened);
    let oriented = matches!(sprite.kind, SurfaceSpriteType::Oriented);
    let additive = emitter.blend.is_some_and(|blend| {
        blend.src == BlendFactor::One && blend.dst == BlendFactor::One
    });
    let seconds = time_ms * 0.001;
    let forward_jka = view_right_jka.cross(Vec3::Z).normalize_or_zero();
    let right_vectors = [
        view_right_jka * 0.985 + forward_jka * 0.174,
        view_right_jka * 0.866 - forward_jka * 0.5,
        view_right_jka * 0.866 + forward_jka * 0.5,
        view_right_jka * 0.985 - forward_jka * 0.174,
    ];
    // The game's modern atmospheric wind is shared with Godot grass. It uses
    // different units from Raven's r_windSpeed, so normalize before bending.
    let wind = Vec3::from_array({
        let v = weather_wind.at(seconds);
        [v[0], v[1], 0.0]
    });
    let wind_strength = (wind.length() / 180.0).clamp(0.0, 1.0);
    let wind_direction = wind.normalize_or_zero();

    for triangle in &emitter.triangles {
        let valid_slope = triangle.normal_z.iter().all(|&nz| {
            if any_angle { true }
            else if hang_down { nz <= -0.5 }
            else { nz >= 0.5 }
        });
        if !valid_slope {
            continue;
        }
        let v = triangle.positions_jka.map(Vec3::from_array);
        let a = v.map(|point| {
            1.0 - ((camera_jka - point).length_squared() - fadedist2) * inv_fade_diff
        });
        if a.iter().all(|&alpha| alpha <= 0.0) {
            continue;
        }
        let v12 = v[1] - v[0];
        let v13 = v[2] - v[0];
        // Raven uses twice the XY projected area, not the full 3D area.
        let triarea = (v13.x * v12.y - v13.y * v12.x).abs();
        if !triarea.is_finite() || triarea <= 1.0 {
            continue;
        }
        let step = sprite.density / triarea.sqrt();
        if !step.is_finite() || step <= 0.0 {
            continue;
        }
        let mut random_index = surface_sprite_byte(
            v[0].x + v[0].y + v[1].x + v[1].y + v[2].x + v[2].y
        );
        let random_interval = surface_sprite_byte(v[0].x + v[1].y + v[2].z) | 0x03;
        let mut right_index = 0usize;
        let mut sprites = 0usize;
        let mut cells = 0usize;
        let mut pos_i = 0.0_f32;
        while pos_i < 1.0 && sprites < MAX_SPRITES_PER_TRIANGLE && cells < MAX_CELLS_PER_TRIANGLE {
            let mut pos_j = 0.0_f32;
            while pos_j < 1.0 - pos_i && sprites < MAX_SPRITES_PER_TRIANGLE && cells < MAX_CELLS_PER_TRIANGLE {
                cells += 1;
                // Advance the RNG exactly as Raven's vertical sprite distribution.
                let fa = pos_i + surface_sprite_random(random_index) * step;
                random_index = random_index.wrapping_add(random_interval);
                let fb = pos_j + surface_sprite_random(random_index) * step;
                random_index = random_index.wrapping_add(random_interval);
                right_index = (right_index + 1) & 3;
                if fa <= 1.0 && fb <= 1.0 - fa {
                    let fc = 1.0 - fa - fb;
                    let alpha_pos = a[0] * fa + a[1] * fb + a[2] * fc;
                    let fade_start = fade_range
                        + (1.0 - fade_range) * surface_sprite_random(random_index);
                    random_index = random_index.wrapping_add(random_interval);
                    let mut alpha = (1.0 - (fade_start - alpha_pos) / fade_range.max(1.0e-6))
                        .clamp(0.0, 1.0);
                    if alpha > 0.0 {
                        let position = v[0] * fa + v[1] * fb + v[2] * fc;
                        let mut light = f32::from(triangle.light[0]) * fa
                            + f32::from(triangle.light[1]) * fb
                            + f32::from(triangle.light[2]) * fc;
                        if additive {
                            light = (128.0 + light * 0.5) * alpha;
                            alpha = 1.0;
                        }
                        let mut size_index = random_index;
                        let variance = surface_sprite_random(size_index);
                        let mut width = sprite.width * (1.0 + sprite.variance[0] * variance);
                        let height = sprite.height * (1.0 + sprite.variance[1] * variance);
                        size_index = size_index.wrapping_add(1);
                        if surface_sprite_random(size_index) > 0.5 {
                            width = -width;
                        }
                        size_index = size_index.wrapping_add(1);
                        if sprite.fade_scale != 0.0 && alpha_pos < 1.0 {
                            width *= 1.0 + sprite.fade_scale * (1.0 - alpha_pos);
                        }
                        let skew = if sprite.vert_skew != 0.0 {
                            let x = surface_sprite_random(size_index) * 2.0 - 1.0;
                            size_index = size_index.wrapping_add(1);
                            let y = surface_sprite_random(size_index) * 2.0 - 1.0;
                            Vec3::new(x, y, 0.0) * (height * sprite.vert_skew)
                        } else {
                            Vec3::ZERO
                        };
                        let angle = (position.x + position.y) * 0.02 + seconds * 1.5;
                        let idle = height * sprite.wind_idle * 0.075;
                        let mut tip = position + skew
                            + Vec3::new(angle.cos(), angle.sin(), 0.0) * idle;
                        tip.z += if hang_down { -height } else { height };
                        if sprite.wind > 0.0 && wind_strength > 0.001 {
                            tip += wind_direction * (height * sprite.wind * wind_strength * 0.3);
                            tip.z += (angle * 2.5).sin()
                                * height * sprite.wind * wind_strength * 0.03;
                        }
                        let right = if flattened {
                            // TaystJK's distinctive flattened sprite right-vector.
                            Vec3::new(position.x.to_radians().sin() * width,
                                position.x.to_radians().cos() * height, 0.0)
                        } else if oriented {
                            view_right_jka * (width * 0.5)
                        } else {
                            right_vectors[right_index] * (width * 0.5)
                        };
                        let top_left_skew = if oriented {
                            Vec3::ZERO
                        } else {
                            forward_jka * (width * 0.2)
                        };
                        let points = [position + right, tip + right,
                            tip - right + top_left_skew, position - right];
                        let uv = [[1.0, 1.0], [1.0, 0.0], [0.0, 0.0], [0.0, 1.0]];
                        let converted = points.map(|p| scene::render_position(p.to_array()));
                        let intensity = (light / 255.0).clamp(0.0, 1.0);
                        for corner in [0usize, 1, 2, 0, 2, 3] {
                            out.push(SurfaceSpriteEffectVertex {
                                position: converted[corner],
                                uv: uv[corner],
                                color: [intensity, intensity, intensity, alpha],
                                alpha_cutoff: emitter.alpha_cutoff,
                            });
                        }
                        sprites += 1;
                    }
                }
                pos_j += step;
            }
            pos_i += step;
        }
    }
}

pub(in crate::renderer) const DYNAMIC_MODEL_VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
    0 => Float32x3,
    1 => Float32x3,
    2 => Float32x2,
    3 => Float32x4,
    4 => Float32,
    5 => Float32,
    6 => Float32
];

/// The RT receiver variant of the dynamic-model pipeline binds a second vertex
/// buffer holding each vertex's pre-lighting material colour. It has to sit past
/// every DynamicModelVertex attribute: md3.wgsl declares those by literal
/// location, so a new vertex field silently shadows this one otherwise.
pub(in crate::renderer) const RT_RECEIVER_RAW_COLOR_LOCATION: u32 =
    DYNAMIC_MODEL_VERTEX_ATTRIBUTES.len() as u32;
pub(in crate::renderer) const RT_RECEIVER_RAW_COLOR_ATTRIBUTES: [wgpu::VertexAttribute; 1] =
    [wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x4,
        offset: 0,
        shader_location: RT_RECEIVER_RAW_COLOR_LOCATION,
    }];

pub(in crate::renderer) const FX_GPU_SPRITE_INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
    0 => Float32x4,
    1 => Float32x4,
    2 => Float32x4,
    3 => Float32x4
];

pub(in crate::renderer) const GHOUL2_GPU_VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 9] = wgpu::vertex_attr_array![
    0 => Float32x3,
    1 => Float32x3,
    2 => Float32x2,
    3 => Uint32x4,
    4 => Float32x4,
    5 => Uint32,
    6 => Uint32,
    7 => Float32,
    8 => Float32
];

pub(in crate::renderer) fn ghoul2_specular_light(
    specular: Option<([f32; 3], [f32; 3])>,
) -> [f32; 4] {
    specular.map_or([0.0; 4], |(light, _)| [light[0], light[1], light[2], 1.0])
}

/// `.w` doubles as the `deformVertexes bulge` static offset (see
/// `Ghoul2GpuSkinning::bulge_height`): shells that bulge never carry specular,
/// and specular surfaces never bulge, so the two never collide.
pub(in crate::renderer) fn ghoul2_specular_viewer(
    specular: Option<([f32; 3], [f32; 3])>,
    bulge_height: f32,
) -> [f32; 4] {
    let viewer = specular.map_or([0.0; 3], |(_, viewer)| viewer);
    [viewer[0], viewer[1], viewer[2], bulge_height]
}

#[cfg(test)]
mod native_surface_sprite_tests {
    use super::*;
    use jka_assets::shader::{SurfaceSprite, SurfaceSpriteFacing, SurfaceSpriteType};

    fn emitter(kind: SurfaceSpriteType) -> scene::SurfaceSpriteEffectEmitter {
        scene::SurfaceSpriteEffectEmitter {
            texture: 0,
            clamp: false,
            blend: Some(BlendFunc { src: BlendFactor::One, dst: BlendFactor::Zero }),
            sprite: SurfaceSprite {
                kind,
                width: 48.0,
                height: 24.0,
                density: 45.0,
                fade_dist: 400.0,
                fade_max: 3000.0,
                fade_scale: 2.5,
                variance: [1.0, 2.0],
                wind: 0.8,
                wind_idle: 0.0,
                vert_skew: 0.0,
                fx_duration: 1000.0,
                fx_grow: [0.0; 2],
                fx_alpha_start: 1.0,
                fx_alpha_end: 0.0,
                facing: SurfaceSpriteFacing::Normal,
            },
            alpha_cutoff: 0.75,
            depth_write: true,
            godot_candidate: true,
            triangles: vec![scene::SurfaceSpriteEffectTriangle {
                positions_jka: [[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [0.0, 100.0, 0.0]],
                normal_z: [1.0; 3],
                light: [240; 3],
            }],
            pvs_signature: Vec::new(),
        }
    }

    #[test]
    fn flattened_surface_sprite_emits_native_quads_and_authored_alpha_test() {
        let emitter = emitter(SurfaceSpriteType::Flattened);
        let mut verts = Vec::new();
        append_jka_vegetation_surface_sprites(
            &emitter, Vec3::new(30.0, 30.0, 64.0), Vec3::X, 0.0,
            crate::ocean::AtmosphericWind::default(), &mut verts,
        );
        assert!(!verts.is_empty());
        assert_eq!(verts.len() % 6, 0);
        assert!(verts.iter().all(|v| v.alpha_cutoff == 0.75));
        assert!(verts.iter().all(|v| v.position.iter().all(|p| p.is_finite())));
    }

    /// Rust mirror of `surface_sprite_resident.wgsl` vs_main (one quad).
    fn resident_quad(
        sprite: &SurfaceSpriteInstance, emitter: &SurfaceSpriteResidentParams,
        frame: &SurfaceSpriteResidentFrame,
    ) -> Option<[SurfaceSpriteEffectVertex; 6]> {
        let p = Vec3::from_slice(&sprite.position_spread[..3]);
        let to_camera = Vec3::from_slice(&frame.camera_seconds[..3]) - p;
        let alpha_pos = 1.0 - (to_camera.length_squared() + sprite.position_spread[3] - emitter.fade[0]) * emitter.fade[1];
        let mut alpha = (1.0 - (sprite.size_fade_light[2] - alpha_pos) / emitter.fade[2].max(1.0e-6)).clamp(0.0, 1.0);
        if alpha <= 0.0 { return None; }
        let mut light = sprite.size_fade_light[3];
        if emitter.flags[3] != 0 { light = (128.0 + light * 0.5) * alpha; alpha = 1.0; }
        let mut width = sprite.size_fade_light[0];
        let height = sprite.size_fade_light[1];
        if emitter.fade[3] != 0.0 && alpha_pos < 1.0 { width *= 1.0 + emitter.fade[3] * (1.0 - alpha_pos); }
        let seconds = frame.camera_seconds[3];
        let angle = (p.x + p.y) * 0.02 + seconds * 1.5;
        let idle = height * emitter.wind[1] * 0.075;
        let mut tip = p + Vec3::new(sprite.skew[0], sprite.skew[1], 0.0) + Vec3::new(angle.cos(), angle.sin(), 0.0) * idle;
        tip.z += if emitter.flags[0] != 0 { -height } else { height };
        let strength = frame.wind[2];
        if emitter.wind[0] > 0.0 && strength > 0.001 {
            tip += Vec3::new(frame.wind[0], frame.wind[1], 0.0) * (height * emitter.wind[0] * strength * 0.3);
            tip.z += (angle * 2.5).sin() * height * emitter.wind[0] * strength * 0.03;
        }
        let r = Vec3::from_slice(&frame.right[..3]);
        let f = Vec3::from_slice(&frame.forward[..3]);
        let right = if emitter.flags[1] != 0 {
            Vec3::new(p.x.to_radians().sin() * width, p.x.to_radians().cos() * height, 0.0)
        } else if emitter.flags[2] != 0 {
            r * (width * 0.5)
        } else {
            [r * 0.985 + f * 0.174, r * 0.866 - f * 0.5, r * 0.866 + f * 0.5, r * 0.985 - f * 0.174]
                [(sprite.right_index & 3) as usize] * (width * 0.5)
        };
        let tls = if emitter.flags[2] != 0 { Vec3::ZERO } else { f * (width * 0.2) };
        let points = [p + right, tip + right, tip - right + tls, p - right];
        let uv = [[1.0, 1.0], [1.0, 0.0], [0.0, 0.0], [0.0, 1.0]];
        let intensity = (light / 255.0).clamp(0.0, 1.0);
        Some([0usize, 1, 2, 0, 2, 3].map(|corner| SurfaceSpriteEffectVertex {
            position: scene::render_position(points[corner].to_array()),
            uv: uv[corner],
            color: [intensity, intensity, intensity, alpha],
            alpha_cutoff: emitter.wind[2],
        }))
    }

    #[test]
    fn resident_instances_reproduce_cpu_vegetation_sprites() {
        let wind = crate::ocean::AtmosphericWind { speed: 120.0, direction: 30.0, gust: 0.2, shift: 0.1 };
        let right = Vec3::new(0.6, -0.8, 0.0);
        for kind in [SurfaceSpriteType::Vertical, SurfaceSpriteType::Flattened, SurfaceSpriteType::Oriented] {
            let mut source = emitter(kind);
            source.sprite.vert_skew = 0.3;
            source.sprite.wind_idle = 0.5;
            source.triangles.push(scene::SurfaceSpriteEffectTriangle {
                positions_jka: [[900.0, 40.0, 8.0], [1300.0, 60.0, 30.0], [950.0, 500.0, 12.0]],
                normal_z: [0.9; 3],
                light: [10, 200, 90],
            });
            let mut instances = Vec::new();
            build_vegetation_instances(&source, 0, &mut instances);
            let params = resident_params(&source);
            for (camera, time_ms) in [(Vec3::new(30.0, 30.0, 64.0), 0.0), (Vec3::new(1100.0, 900.0, 80.0), 2500.0),
                (Vec3::new(-1500.0, -900.0, 60.0), 7300.0)] {
                let mut cpu = Vec::new();
                append_jka_vegetation_surface_sprites(&source, camera, right, time_ms, wind, &mut cpu);
                let frame = resident_frame(camera, right, time_ms, wind);
                let gpu: Vec<_> = instances.iter()
                    .filter_map(|instance| resident_quad(instance, &params, &frame)).flatten().collect();
                assert_eq!(cpu.len(), gpu.len(), "{kind:?} at {camera}");
                assert!(!cpu.is_empty());
                for (a, b) in cpu.iter().zip(&gpu) {
                    for axis in 0..3 {
                        assert!((a.position[axis] - b.position[axis]).abs() < 1.0e-2, "{kind:?} {a:?} {b:?}");
                    }
                    assert_eq!(a.uv, b.uv);
                    for channel in 0..4 {
                        assert!((a.color[channel] - b.color[channel]).abs() < 1.0e-3, "{kind:?} color");
                    }
                    assert_eq!(a.alpha_cutoff, b.alpha_cutoff);
                }
            }
        }
    }

    #[test]
    fn resident_chunks_partition_every_instance() {
        let source = emitter(SurfaceSpriteType::Vertical);
        let mut scratch = Vec::new();
        build_vegetation_instances(&source, 0, &mut scratch);
        let total = scratch.len();
        let (mut instances, mut chunks) = (Vec::new(), Vec::new());
        bucket_resident_instances(&source, &mut scratch, &mut instances, &mut chunks);
        assert_eq!(instances.len(), total);
        assert_eq!(chunks.first().unwrap().instances.start, 0);
        assert_eq!(chunks.last().unwrap().instances.end as usize, total);
        assert!(chunks.windows(2).all(|pair| pair[0].instances.end == pair[1].instances.start));
        for chunk in &chunks {
            for instance in &instances[chunk.instances.start as usize..chunk.instances.end as usize] {
                for axis in 0..3 {
                    assert!(instance.position_spread[axis] >= chunk.min_jka[axis]);
                    assert!(instance.position_spread[axis] <= chunk.max_jka[axis]);
                }
            }
        }
    }

    #[test]
    fn native_surface_sprites_are_distance_faded() {
        let emitter = emitter(SurfaceSpriteType::Vertical);
        let mut verts = Vec::new();
        append_jka_vegetation_surface_sprites(
            &emitter, Vec3::new(10000.0, 0.0, 0.0), Vec3::X, 0.0,
            crate::ocean::AtmosphericWind::default(), &mut verts,
        );
        assert!(verts.is_empty());
    }
}
