//! Environment surface sprites.
use crate::renderer::{
    camera_depth_compare, gpu_blend_factor, pipeline_hash, sampler_entry, scene, texture_entry,
    BTreeMap, BlendFactor, BlendFunc, Camera, PipelineJobKey, PipelineJobManager, Pod, Vec3,
    Zeroable, DEPTH_FORMAT,
};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct SurfaceSpriteEffectVertex {
    pub(in crate::renderer) position: [f32; 3],
    pub(in crate::renderer) uv: [f32; 2],
    pub(in crate::renderer) color: [f32; 4],
}

pub(in crate::renderer) const SURFACE_SPRITE_EFFECT_ATTRIBUTES: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x4];

/// Raven's fixed deterministic table from tr_surfacesprites.cpp. Keeping this
/// verbatim is important: surfaceSprites positions are seeded from triangle
/// coordinates and shader time, so replacing it with a modern RNG visibly moves
/// every splash/particle on legacy maps.
pub(in crate::renderer) const SURFACE_SPRITE_RANDOM: [f32; 256] = [
    0.6554, 0.6909, 0.4806, 0.6218, 0.5717, 0.3896, 0.0677, 0.7356, 0.8333, 0.1105, 0.4445, 0.8161,
    0.4689, 0.0433, 0.7152, 0.0336, 0.0186, 0.9140, 0.1626, 0.6553, 0.8340, 0.7094, 0.2020, 0.8087,
    0.9119, 0.8009, 0.1339, 0.8492, 0.9173, 0.5003, 0.6012, 0.6117, 0.5525, 0.5787, 0.1586, 0.3293,
    0.9273, 0.7791, 0.8589, 0.4985, 0.0883, 0.8545, 0.2634, 0.4727, 0.3624, 0.1631, 0.7825, 0.0662,
    0.6704, 0.3510, 0.7525, 0.9486, 0.4685, 0.1535, 0.1545, 0.1121, 0.4724, 0.8483, 0.3833, 0.1917,
    0.8207, 0.3885, 0.9702, 0.9200, 0.8348, 0.7501, 0.6675, 0.4994, 0.0301, 0.5225, 0.8011, 0.1696,
    0.5351, 0.2752, 0.2962, 0.7550, 0.5762, 0.7303, 0.2835, 0.4717, 0.1818, 0.2739, 0.6914, 0.7748,
    0.7640, 0.8355, 0.7314, 0.5288, 0.7340, 0.6692, 0.6813, 0.2810, 0.8057, 0.0648, 0.8749, 0.9199,
    0.1462, 0.5237, 0.3014, 0.4994, 0.0278, 0.4268, 0.7238, 0.5107, 0.1378, 0.7303, 0.7200, 0.3819,
    0.2034, 0.7157, 0.5552, 0.4887, 0.0871, 0.3293, 0.2892, 0.4545, 0.0088, 0.1404, 0.0275, 0.0238,
    0.0515, 0.4494, 0.7206, 0.2893, 0.6060, 0.5785, 0.4182, 0.5528, 0.9118, 0.8742, 0.3859, 0.6030,
    0.3495, 0.4550, 0.9875, 0.6900, 0.6416, 0.2337, 0.7431, 0.9788, 0.6181, 0.2464, 0.4661, 0.7621,
    0.7020, 0.8203, 0.8869, 0.2145, 0.7724, 0.6093, 0.6692, 0.9686, 0.5609, 0.0310, 0.2248, 0.2950,
    0.2365, 0.1347, 0.2342, 0.1668, 0.3378, 0.4330, 0.2775, 0.9901, 0.7053, 0.7266, 0.4840, 0.2820,
    0.5733, 0.4555, 0.6049, 0.0770, 0.4760, 0.6060, 0.4159, 0.3427, 0.1234, 0.7062, 0.8569, 0.1878,
    0.9057, 0.9399, 0.8139, 0.1407, 0.1794, 0.9123, 0.9493, 0.2827, 0.9934, 0.0952, 0.4879, 0.5160,
    0.4118, 0.4873, 0.3642, 0.7470, 0.0866, 0.5172, 0.6365, 0.2676, 0.2407, 0.7223, 0.5761, 0.1143,
    0.7137, 0.2342, 0.3353, 0.6880, 0.2296, 0.6023, 0.6027, 0.4138, 0.5408, 0.9859, 0.1503, 0.7238,
    0.6054, 0.2477, 0.6804, 0.1432, 0.4540, 0.9776, 0.8762, 0.7607, 0.9025, 0.9807, 0.0652, 0.8661,
    0.7663, 0.2586, 0.3994, 0.0335, 0.7328, 0.0166, 0.9589, 0.4348, 0.5493, 0.7269, 0.6867, 0.6614,
    0.6800, 0.7804, 0.5591, 0.8381, 0.0910, 0.7573, 0.8985, 0.3083, 0.3188, 0.8481, 0.2356, 0.6736,
    0.4770, 0.4560, 0.6266, 0.4677,
];

pub(in crate::renderer) struct SurfaceSpriteEffectGpu {
    pub(in crate::renderer) source: scene::SurfaceSpriteEffectEmitter,
    pub(in crate::renderer) bind_group: wgpu::BindGroup,
}

#[derive(Default)]
pub(in crate::renderer) struct PreparedSurfaceSpriteEffects {
    pub(in crate::renderer) draws: Vec<SurfaceSpriteEffectDraw>,
}

pub(in crate::renderer) struct SurfaceSpriteEffectDraw {
    pub(in crate::renderer) emitter: usize,
    pub(in crate::renderer) vertices: std::ops::Range<u32>,
}

pub(in crate::renderer) struct SurfaceSpriteEffectRenderer {
    pub(in crate::renderer) texture_layout: wgpu::BindGroupLayout,
    pub(in crate::renderer) pipeline_layout: wgpu::PipelineLayout,
    pub(in crate::renderer) shader: wgpu::ShaderModule,
    pub(in crate::renderer) wireframe_pipeline: Option<wgpu::RenderPipeline>,
    pub(in crate::renderer) pipelines: BTreeMap<Option<BlendFunc>, wgpu::RenderPipeline>,
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
            wireframe_pipeline: None,
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
        self.wireframe_pipeline = None;
        self.pipelines.clear();
        // Pipelines are recreated lazily for only the blend functions the map uses.
        let _ = device;
    }

    pub(in crate::renderer) fn ensure_wireframe_pipeline(
        &mut self,
        jobs: &mut PipelineJobManager,
        device: &wgpu::Device,
    ) {
        if self.wireframe_pipeline.is_some() {
            return;
        }
        let key = PipelineJobKey::new(
            "surface-sprites-wireframe",
            0,
            pipeline_hash(&(self.surface_format, self.samples)),
        );
        if let Some(pipeline) = jobs.take_ready(key) {
            self.wireframe_pipeline = Some(pipeline);
            return;
        }
        let device = device.clone();
        let layout = self.pipeline_layout.clone();
        let shader = self.shader.clone();
        let surface_format = self.surface_format;
        let samples = self.samples;
        jobs.request(key, "surfaceSprites wireframe", move || {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("JKA surfaceSprites wireframe pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<SurfaceSpriteEffectVertex>()
                            as wgpu::BufferAddress,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &SURFACE_SPRITE_EFFECT_ATTRIBUTES,
                    }],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    polygon_mode: wgpu::PolygonMode::Line,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_wireframe"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: surface_format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        });
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
    ) {
        if self.pipelines.contains_key(&blend) {
            return;
        }
        let blend_id = pipeline_hash(&blend);
        let key = PipelineJobKey::new(
            "surface-sprites",
            blend_id as u32,
            pipeline_hash(&(self.surface_format, self.samples, blend_id)),
        );
        if let Some(pipeline) = jobs.take_ready(key) {
            self.pipelines.insert(blend, pipeline);
            return;
        }
        let device = device.clone();
        let layout = self.pipeline_layout.clone();
        let shader = self.shader.clone();
        let surface_format = self.surface_format;
        let samples = self.samples;
        jobs.request(key, "surfaceSprites blend variant", move || {
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
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("JKA surfaceSprites effect pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<SurfaceSpriteEffectVertex>()
                            as wgpu::BufferAddress,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &SURFACE_SPRITE_EFFECT_ATTRIBUTES,
                    }],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(blend.is_none()),
                    depth_compare: Some(camera_depth_compare()),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: surface_format,
                        blend: gpu_blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        });
    }

    pub(in crate::renderer) fn prepare_draw(
        &mut self,
        jobs: &mut PipelineJobManager,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        effects: &[SurfaceSpriteEffectGpu],
        camera: &Camera,
        pvs_cluster: Option<usize>,
        time_ms: f32,
    ) -> PreparedSurfaceSpriteEffects {
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

        for (emitter_index, effect) in effects.iter().enumerate() {
            if !surface_sprite_signature_visible(&effect.source.pvs_signature, pvs_cluster) {
                continue;
            }
            self.ensure_pipeline(jobs, device, effect.source.blend);
            let start = vertices.len() as u32;
            append_jka_effect_surface_sprites(
                &effect.source,
                camera_jka,
                right_jka,
                up_jka,
                time_ms,
                &mut vertices,
            );
            let end = vertices.len() as u32;
            if end > start {
                draws.push(SurfaceSpriteEffectDraw {
                    emitter: emitter_index,
                    vertices: start..end,
                });
            }
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
        PreparedSurfaceSpriteEffects { draws }
    }

    pub(in crate::renderer) fn draw<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        camera_bind_group: &'a wgpu::BindGroup,
        effects: &'a [SurfaceSpriteEffectGpu],
        prepared: &PreparedSurfaceSpriteEffects,
    ) {
        if prepared.draws.is_empty() {
            return;
        }
        pass.set_bind_group(0, camera_bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        for draw in &prepared.draws {
            let effect = &effects[draw.emitter];
            let Some(pipeline) = self.pipelines.get(&effect.source.blend) else {
                continue;
            };
            pass.set_pipeline(pipeline);
            pass.set_bind_group(1, &effect.bind_group, &[]);
            pass.draw(draw.vertices.clone(), 0..1);
        }
    }

    pub(in crate::renderer) fn draw_wireframe<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        camera_bind_group: &'a wgpu::BindGroup,
        effects: &'a [SurfaceSpriteEffectGpu],
        prepared: &PreparedSurfaceSpriteEffects,
    ) {
        let Some(pipeline) = self.wireframe_pipeline.as_ref() else {
            return;
        };
        if prepared.draws.is_empty() {
            return;
        }
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, camera_bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        for draw in &prepared.draws {
            // The wireframe shader does not sample the sprite texture, but this
            // pipeline intentionally shares the normal surfaceSprites pipeline
            // layout. WGPU therefore still requires bind-group slot 1 to be
            // layout-compatible at draw time. Explicitly bind the emitter's
            // surfaceSprites group so stale dynamic-model/world bindings can
            // never leak across debug overlay draws.
            let effect = &effects[draw.emitter];
            pass.set_bind_group(1, &effect.bind_group, &[]);
            pass.draw(draw.vertices.clone(), 0..1);
        }
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

pub(in crate::renderer) fn surface_sprite_random(index: u8) -> f32 {
    SURFACE_SPRITE_RANDOM[index as usize]
}

pub(in crate::renderer) fn surface_sprite_byte(value: f32) -> u8 {
    ((value.trunc() as i64).rem_euclid(256)) as u8
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

pub(in crate::renderer) const DYNAMIC_MODEL_VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    0 => Float32x3,
    1 => Float32x3,
    2 => Float32x2,
    3 => Float32x4,
    4 => Float32
];

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
