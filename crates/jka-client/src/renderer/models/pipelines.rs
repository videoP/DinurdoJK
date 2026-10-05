//! Models pipelines.
use crate::renderer::{
    camera_depth_compare, create_entity_shadow_pipeline, dynamic_model_writes_depth,
    gpu_blend_factor, pipeline_hash, polygon_offset_depth_bias, scene, BlendMode, CullMode,
    DrawClass, DynamicModelAlphaMode, DynamicModelRenderer, DynamicModelVertex,
    FxGpuSpriteInstance, Ghoul2GpuVertex, GpuVertex, PipelineJobKey, PipelineJobManager,
    PipelineKey, Vec3, WorldShaderVariantKey, BLOB_MARK_PROJECTION, BLOB_MARK_SURFACE_LIFT,
    DEPTH_FORMAT, DYNAMIC_MODEL_ALPHA_ORDER, DYNAMIC_MODEL_VERTEX_ATTRIBUTES,
    FX_GPU_SPRITE_INSTANCE_ATTRIBUTES, GHOUL2_GPU_VERTEX_ATTRIBUTES,
    LEGACY_DLIGHT_SURFACE_ID_ATTRIBUTES, VERTEX_ATTRIBUTES,
};

impl DynamicModelRenderer {
    pub(in crate::renderer) fn rebuild_pipelines(
        &mut self,
        _device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        samples: u32,
    ) {
        self.wireframe_pipeline = None;
        self.fx_sprite_wireframe_pipeline = None;
        self.fx_sprite_surface_format = surface_format;
        self.fx_sprite_samples = samples;
        self.skin_wireframe_pipeline = None;
        // MD3 / Ghoul2 alpha-mode pipelines are lazy: recompiled by
        // ensure_model_pipelines for only the modes that have draws.
        self.model_pipelines = Default::default();
        self.model_alpha_mask = 0;
        self.fx_sprite_opaque_pipeline = None;
        self.fx_sprite_mask_pipeline = None;
        self.fx_sprite_blend_pipeline = None;
        self.fx_sprite_mask_blend_pipeline = None;
        self.fx_sprite_additive_one_pipeline = None;
        self.fx_sprite_additive_pipeline = None;
        self.fx_sprite_blend_unlit_pipeline = None;
        self.fx_sprite_modulate_pipeline = None;
        self.fx_sprite_dst_color_add_pipeline = None;
        self.fx_sprite_modulate2x_pipeline = None;
        self.fx_sprite_darken_pipeline = None;
        self.skin_pipelines = Default::default();
        self.skin_alpha_mask = 0;
    }

    pub(in crate::renderer) fn request_model_pipeline_variant(
        &self,
        jobs: &mut PipelineJobManager,
        device: &wgpu::Device,
        alpha_mode: DynamicModelAlphaMode,
        skin: bool,
    ) {
        let surface_format = self.fx_sprite_surface_format;
        let samples = self.fx_sprite_samples;
        let config = pipeline_hash(&(surface_format, samples, self.legacy_fog_compiled));
        let index = alpha_mode.index();
        if skin {
            let key = PipelineJobKey::new("ghoul2-model", index as u32, config);
            let device = device.clone();
            let layout = self.skin_pipeline_layout.clone();
            let shader = self.skin_shader.clone();
            let legacy_fog = self.legacy_fog_compiled;
            jobs.request(key, "Ghoul2 alpha variant", move || {
                create_ghoul2_skin_pipeline(
                    &device,
                    &layout,
                    &shader,
                    surface_format,
                    samples,
                    alpha_mode,
                    legacy_fog,
                )
            });
        } else {
            let key = PipelineJobKey::new("dynamic-model", index as u32, config);
            let device = device.clone();
            let layout = self.pipeline_layout.clone();
            let shader = self.shader.clone();
            let legacy_fog = self.legacy_fog_compiled;
            jobs.request(key, "dynamic model alpha variant", move || {
                create_dynamic_model_pipeline(
                    &device,
                    &layout,
                    &shader,
                    surface_format,
                    samples,
                    alpha_mode,
                    legacy_fog,
                )
            });
        }
    }

    /// Opaque and alpha-tested models are ordinary gameplay content, not optional
    /// eye candy. Start their lazy jobs early so the first player/entity does not
    /// disappear for a frame merely because its PSO is still compiling. The rare
    /// blend/additive variants remain true first-use lazy jobs.
    pub(in crate::renderer) fn prewarm_core_model_pipelines(
        &self,
        jobs: &mut PipelineJobManager,
        device: &wgpu::Device,
    ) {
        for alpha_mode in [DynamicModelAlphaMode::Opaque, DynamicModelAlphaMode::Mask] {
            self.request_model_pipeline_variant(jobs, device, alpha_mode, false);
            self.request_model_pipeline_variant(jobs, device, alpha_mode, true);
        }
    }

    /// Requests the MD3 / Ghoul2 pipelines for exactly the alpha modes that
    /// have draws this frame. Finished worker jobs are installed immediately;
    /// still-compiling variants are simply skipped by draw_phase for this frame.
    pub(in crate::renderer) fn ensure_model_pipelines(
        &mut self,
        jobs: &mut PipelineJobManager,
        device: &wgpu::Device,
    ) {
        let mut model_mask = 0u16;
        for draw in &self.draws {
            model_mask |= 1 << draw.alpha_mode.index();
        }
        let mut skin_mask = 0u16;
        for draw in &self.skin_draws {
            skin_mask |= 1 << draw.alpha_mode.index();
        }
        self.model_alpha_mask = model_mask;
        self.skin_alpha_mask = skin_mask;
        if model_mask | skin_mask == 0 {
            return;
        }
        let surface_format = self.fx_sprite_surface_format;
        let samples = self.fx_sprite_samples;
        let config = pipeline_hash(&(surface_format, samples, self.legacy_fog_compiled));
        for alpha_mode in DYNAMIC_MODEL_ALPHA_ORDER {
            let index = alpha_mode.index();
            if model_mask & (1 << index) != 0 && self.model_pipelines[index].is_none() {
                let key = PipelineJobKey::new("dynamic-model", index as u32, config);
                if let Some(pipeline) = jobs.take_ready(key) {
                    self.model_pipelines[index] = Some(pipeline);
                } else {
                    self.request_model_pipeline_variant(jobs, device, alpha_mode, false);
                }
            }
            if skin_mask & (1 << index) != 0 && self.skin_pipelines[index].is_none() {
                let key = PipelineJobKey::new("ghoul2-model", index as u32, config);
                if let Some(pipeline) = jobs.take_ready(key) {
                    self.skin_pipelines[index] = Some(pipeline);
                } else {
                    self.request_model_pipeline_variant(jobs, device, alpha_mode, true);
                }
            }
        }
    }

    pub(in crate::renderer) fn fx_sprite_pipeline_ready(
        &self,
        alpha_mode: DynamicModelAlphaMode,
    ) -> bool {
        match alpha_mode {
            DynamicModelAlphaMode::Opaque => self.fx_sprite_opaque_pipeline.is_some(),
            DynamicModelAlphaMode::Mask => self.fx_sprite_mask_pipeline.is_some(),
            DynamicModelAlphaMode::Blend => self.fx_sprite_blend_pipeline.is_some(),
            DynamicModelAlphaMode::MaskBlend => self.fx_sprite_mask_blend_pipeline.is_some(),
            DynamicModelAlphaMode::AdditiveOne => self.fx_sprite_additive_one_pipeline.is_some(),
            DynamicModelAlphaMode::Additive => self.fx_sprite_additive_pipeline.is_some(),
            DynamicModelAlphaMode::BlendUnlit => self.fx_sprite_blend_unlit_pipeline.is_some(),
            DynamicModelAlphaMode::Modulate => self.fx_sprite_modulate_pipeline.is_some(),
            DynamicModelAlphaMode::DstColorAdd => self.fx_sprite_dst_color_add_pipeline.is_some(),
            DynamicModelAlphaMode::Modulate2x => self.fx_sprite_modulate2x_pipeline.is_some(),
            DynamicModelAlphaMode::Darken => self.fx_sprite_darken_pipeline.is_some(),
        }
    }

    pub(in crate::renderer) fn fx_sprite_pipeline(
        &self,
        alpha_mode: DynamicModelAlphaMode,
    ) -> Option<&wgpu::RenderPipeline> {
        match alpha_mode {
            DynamicModelAlphaMode::Opaque => self.fx_sprite_opaque_pipeline.as_ref(),
            DynamicModelAlphaMode::Mask => self.fx_sprite_mask_pipeline.as_ref(),
            DynamicModelAlphaMode::Blend => self.fx_sprite_blend_pipeline.as_ref(),
            DynamicModelAlphaMode::MaskBlend => self.fx_sprite_mask_blend_pipeline.as_ref(),
            DynamicModelAlphaMode::AdditiveOne => self.fx_sprite_additive_one_pipeline.as_ref(),
            DynamicModelAlphaMode::Additive => self.fx_sprite_additive_pipeline.as_ref(),
            DynamicModelAlphaMode::BlendUnlit => self.fx_sprite_blend_unlit_pipeline.as_ref(),
            DynamicModelAlphaMode::Modulate => self.fx_sprite_modulate_pipeline.as_ref(),
            DynamicModelAlphaMode::DstColorAdd => self.fx_sprite_dst_color_add_pipeline.as_ref(),
            DynamicModelAlphaMode::Modulate2x => self.fx_sprite_modulate2x_pipeline.as_ref(),
            DynamicModelAlphaMode::Darken => self.fx_sprite_darken_pipeline.as_ref(),
        }
    }

    pub(in crate::renderer) fn set_fx_sprite_pipeline(
        &mut self,
        alpha_mode: DynamicModelAlphaMode,
        pipeline: wgpu::RenderPipeline,
    ) {
        match alpha_mode {
            DynamicModelAlphaMode::Opaque => self.fx_sprite_opaque_pipeline = Some(pipeline),
            DynamicModelAlphaMode::Mask => self.fx_sprite_mask_pipeline = Some(pipeline),
            DynamicModelAlphaMode::Blend => self.fx_sprite_blend_pipeline = Some(pipeline),
            DynamicModelAlphaMode::MaskBlend => self.fx_sprite_mask_blend_pipeline = Some(pipeline),
            DynamicModelAlphaMode::AdditiveOne => {
                self.fx_sprite_additive_one_pipeline = Some(pipeline)
            }
            DynamicModelAlphaMode::Additive => self.fx_sprite_additive_pipeline = Some(pipeline),
            DynamicModelAlphaMode::BlendUnlit => {
                self.fx_sprite_blend_unlit_pipeline = Some(pipeline)
            }
            DynamicModelAlphaMode::Modulate => self.fx_sprite_modulate_pipeline = Some(pipeline),
            DynamicModelAlphaMode::DstColorAdd => {
                self.fx_sprite_dst_color_add_pipeline = Some(pipeline)
            }
            DynamicModelAlphaMode::Modulate2x => {
                self.fx_sprite_modulate2x_pipeline = Some(pipeline)
            }
            DynamicModelAlphaMode::Darken => self.fx_sprite_darken_pipeline = Some(pipeline),
        }
    }

    pub(in crate::renderer) fn ensure_fx_sprite_pipeline(
        &mut self,
        jobs: &mut PipelineJobManager,
        device: &wgpu::Device,
        alpha_mode: DynamicModelAlphaMode,
    ) {
        if self.fx_sprite_pipeline_ready(alpha_mode) {
            return;
        }

        // This shader used to be parsed/validated synchronously at the first FX
        // sprite sighting. Keep that laziness, but move even module creation off
        // the render thread before requesting any alpha-mode PSO.
        if self.fx_sprite_shader.is_none() {
            let shader_key = PipelineJobKey::new("fx-sprite-shader", 0, 0);
            if let Some(shader) = jobs.take_ready::<wgpu::ShaderModule>(shader_key) {
                self.fx_sprite_shader = Some(shader);
            } else {
                let device = device.clone();
                jobs.request(shader_key, "FX sprite shader module", move || {
                    device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("JKA GPU FX sprite shader"),
                        source: wgpu::ShaderSource::Wgsl(
                            include_str!("../../fx_sprite.wgsl").into(),
                        ),
                    })
                });
                return;
            }
        }

        let config = pipeline_hash(&(
            self.fx_sprite_surface_format,
            self.fx_sprite_samples,
            self.legacy_fog_compiled,
            self.fx_zero_alpha_discard,
        ));
        let key = PipelineJobKey::new("fx-sprite", alpha_mode.index() as u32, config);
        if let Some(pipeline) = jobs.take_ready(key) {
            self.set_fx_sprite_pipeline(alpha_mode, pipeline);
            return;
        }
        let device = device.clone();
        let layout = self.pipeline_layout.clone();
        let shader = self
            .fx_sprite_shader
            .as_ref()
            .expect("shader installed above")
            .clone();
        let surface_format = self.fx_sprite_surface_format;
        let samples = self.fx_sprite_samples;
        let legacy_fog = self.legacy_fog_compiled;
        let zero_alpha_discard = self.fx_zero_alpha_discard;
        jobs.request(key, "FX sprite alpha variant", move || {
            create_fx_sprite_pipeline(
                &device,
                &layout,
                &shader,
                surface_format,
                samples,
                alpha_mode,
                legacy_fog,
                zero_alpha_discard,
            )
        });
    }

    pub(in crate::renderer) fn ensure_wireframe_pipelines(
        &mut self,
        jobs: &mut PipelineJobManager,
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        samples: u32,
    ) {
        let config = pipeline_hash(&(surface_format, samples));
        if !self.draws.is_empty() && self.wireframe_pipeline.is_none() {
            let key = PipelineJobKey::new("dynamic-wireframe", 0, config);
            if let Some(pipeline) = jobs.take_ready(key) {
                self.wireframe_pipeline = Some(pipeline);
            } else {
                let device = device.clone();
                let layout = self.pipeline_layout.clone();
                let shader = self.shader.clone();
                jobs.request(key, "dynamic model wireframe", move || {
                    create_dynamic_wireframe_pipeline(
                        &device,
                        &layout,
                        &shader,
                        surface_format,
                        samples,
                    )
                });
            }
        }
        if !self.fx_sprite_draws.is_empty() && self.fx_sprite_wireframe_pipeline.is_none() {
            if let Some(shader) = self.fx_sprite_shader.as_ref() {
                let key = PipelineJobKey::new("dynamic-wireframe", 1, config);
                if let Some(pipeline) = jobs.take_ready(key) {
                    self.fx_sprite_wireframe_pipeline = Some(pipeline);
                } else {
                    let device = device.clone();
                    let layout = self.pipeline_layout.clone();
                    let shader = shader.clone();
                    jobs.request(key, "FX sprite wireframe", move || {
                        create_fx_sprite_wireframe_pipeline(
                            &device,
                            &layout,
                            &shader,
                            surface_format,
                            samples,
                        )
                    });
                }
            }
        }
        if !self.skin_draws.is_empty() && self.skin_wireframe_pipeline.is_none() {
            let key = PipelineJobKey::new("dynamic-wireframe", 2, config);
            if let Some(pipeline) = jobs.take_ready(key) {
                self.skin_wireframe_pipeline = Some(pipeline);
            } else {
                let device = device.clone();
                let layout = self.skin_pipeline_layout.clone();
                let shader = self.skin_shader.clone();
                jobs.request(key, "Ghoul2 wireframe", move || {
                    create_ghoul2_wireframe_pipeline(
                        &device,
                        &layout,
                        &shader,
                        surface_format,
                        samples,
                    )
                });
            }
        }
    }

    pub(in crate::renderer) fn ensure_entity_shadow_pipelines(
        &mut self,
        jobs: &mut PipelineJobManager,
        device: &wgpu::Device,
        reverse_z: bool,
        unclipped_depth: bool,
    ) {
        let slot = usize::from(reverse_z);
        if self.entity_shadow_pipelines[slot].is_some() {
            return;
        }
        let key = PipelineJobKey::new(
            "entity-shadow",
            slot as u32,
            pipeline_hash(&(reverse_z, unclipped_depth)),
        );
        if let Some(pipelines) = jobs.take_ready::<[wgpu::RenderPipeline; 4]>(key) {
            self.entity_shadow_pipelines[slot] = Some(pipelines);
            return;
        }
        let device = device.clone();
        let pipeline_layout = self.pipeline_layout.clone();
        let shader = self.shader.clone();
        let skin_pipeline_layout = self.skin_pipeline_layout.clone();
        let skin_shader = self.skin_shader.clone();
        jobs.request(key, "entity shadow variants", move || {
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
            [
                create_entity_shadow_pipeline(
                    &device,
                    &pipeline_layout,
                    &shader,
                    md3_vertex_layout.clone(),
                    false,
                    reverse_z,
                    unclipped_depth,
                    "JKA entity shadow pipeline",
                ),
                create_entity_shadow_pipeline(
                    &device,
                    &pipeline_layout,
                    &shader,
                    md3_vertex_layout,
                    true,
                    reverse_z,
                    unclipped_depth,
                    "JKA entity shadow alpha-test pipeline",
                ),
                create_entity_shadow_pipeline(
                    &device,
                    &skin_pipeline_layout,
                    &skin_shader,
                    skin_vertex_layout.clone(),
                    false,
                    reverse_z,
                    unclipped_depth,
                    "JKA Ghoul2 entity shadow pipeline",
                ),
                create_entity_shadow_pipeline(
                    &device,
                    &skin_pipeline_layout,
                    &skin_shader,
                    skin_vertex_layout,
                    true,
                    reverse_z,
                    unclipped_depth,
                    "JKA Ghoul2 entity shadow alpha-test pipeline",
                ),
            ]
        });
    }
}

pub(in crate::renderer) fn create_dynamic_wireframe_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    samples: u32,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("JKA dynamic model wireframe pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<DynamicModelVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &DYNAMIC_MODEL_VERTEX_ATTRIBUTES,
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
            module: shader,
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
}

pub(in crate::renderer) fn create_ghoul2_wireframe_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    samples: u32,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("JKA Ghoul2 wireframe pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Ghoul2GpuVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &GHOUL2_GPU_VERTEX_ATTRIBUTES,
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
            module: shader,
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
}

pub(in crate::renderer) fn fx_sprite_blend(
    alpha_mode: DynamicModelAlphaMode,
) -> Option<wgpu::BlendState> {
    match alpha_mode {
        DynamicModelAlphaMode::AdditiveOne => Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        }),
        DynamicModelAlphaMode::Additive => Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        }),
        DynamicModelAlphaMode::Blend
        | DynamicModelAlphaMode::MaskBlend
        | DynamicModelAlphaMode::BlendUnlit => Some(wgpu::BlendState::ALPHA_BLENDING),
        DynamicModelAlphaMode::Modulate => Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Dst,
                dst_factor: wgpu::BlendFactor::Zero,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        }),
        DynamicModelAlphaMode::DstColorAdd => Some(wgpu::BlendState {
            // Exact GL_DST_COLOR GL_ONE fixed-function equation:
            // src * dst + dst.
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Dst,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::DstAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        }),
        DynamicModelAlphaMode::Modulate2x => Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Dst,
                dst_factor: wgpu::BlendFactor::Src,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::DstAlpha,
                dst_factor: wgpu::BlendFactor::SrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        }),
        DynamicModelAlphaMode::Darken => Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::OneMinusSrc,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        }),
        _ => None,
    }
}

/// CG_ImpactMark for one blob shadow. The request is in render space: `center`
/// on the collision plane, `left`/`up` the in-plane half axes (length = radius)
/// with `left x up` the plane normal. The shared projector works in JKA space,
/// which is a pure rotation away, so handedness carries over. UVs follow
/// CG_ImpactMark: 0.5 + delta . axis * 0.5 / radius.
#[allow(clippy::too_many_arguments)]
pub(in crate::renderer) fn project_blob_shadow_mark(
    surfaces: &jka_assets::bsp::MarkSurfaces,
    buffer: &mut jka_assets::bsp::MarkBuffer,
    center: Vec3,
    left: Vec3,
    up: Vec3,
    shade: f32,
    vertices: &mut Vec<DynamicModelVertex>,
    indices: &mut Vec<u32>,
) {
    let radius = left.length();
    let normal = left.cross(up).normalize_or_zero();
    if radius <= 0.0 || normal == Vec3::ZERO {
        return;
    }
    let to_jka = |v: Vec3| Vec3::from_array(scene::jka_position(v.to_array()));
    let to_render = |v: Vec3| Vec3::from_array(scene::render_position(v.to_array()));
    let (center_jka, u_axis, v_axis, normal_jka) = (
        to_jka(center),
        to_jka(left / radius),
        to_jka(up / radius),
        to_jka(normal),
    );
    // CG_ImpactMark winds its quad with axis1 x axis2 = -normal.
    let (a1, a2) = (u_axis * radius, -v_axis * radius);
    let quad = [
        center_jka - a1 - a2,
        center_jka + a1 - a2,
        center_jka + a1 + a2,
        center_jka - a1 + a2,
    ]
    .map(|point| point.to_array());
    surfaces.project(
        &quad,
        (normal_jka * -BLOB_MARK_PROJECTION).to_array(),
        buffer,
    );
    let inverse_diameter = 0.5 / radius;
    for (polygon, surface_normal) in buffer.iter() {
        let base = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
        let render_normal = to_render(Vec3::from_array(surface_normal));
        for point in polygon {
            let world = Vec3::from_array(*point);
            let delta = world - center_jka;
            vertices.push(DynamicModelVertex {
                position: (to_render(world) + render_normal * BLOB_MARK_SURFACE_LIFT).to_array(),
                normal: render_normal.to_array(),
                uv: [
                    0.5 + delta.dot(u_axis) * inverse_diameter,
                    0.5 + delta.dot(v_axis) * inverse_diameter,
                ],
                color: [shade, shade, shade, 1.0],
                depth_hack: 0.0,
            });
        }
        // The mark index keeps the world mesh's OpenJK (clockwise) winding for BSP
        // maps but already-reversed render winding for source-map worlds, and the
        // dynamic-model pipeline culls back faces. Wind each triangle to face along
        // the surface normal so both come out visible.
        for fan in 1..polygon.len() as u32 - 1 {
            let corner =
                |offset: u32| Vec3::from_array(vertices[(base + offset) as usize].position);
            let facing = (corner(fan) - corner(0))
                .cross(corner(fan + 1) - corner(0))
                .dot(render_normal);
            if facing < 0.0 {
                indices.extend_from_slice(&[base, base + fan + 1, base + fan]);
            } else {
                indices.extend_from_slice(&[base, base + fan, base + fan + 1]);
            }
        }
    }
}

pub(in crate::renderer) fn create_fx_sprite_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    samples: u32,
    alpha_mode: DynamicModelAlphaMode,
    legacy_fog: bool,
    zero_alpha_discard: bool,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(match alpha_mode {
            DynamicModelAlphaMode::Opaque => "JKA GPU FX sprite opaque pipeline",
            DynamicModelAlphaMode::Mask => "JKA GPU FX sprite alpha-test pipeline",
            DynamicModelAlphaMode::Blend => "JKA GPU FX sprite blend pipeline",
            DynamicModelAlphaMode::MaskBlend => {
                "JKA GPU FX sprite alpha-test forced-alpha pipeline"
            }
            DynamicModelAlphaMode::AdditiveOne => "JKA GPU FX sprite GL_ONE GL_ONE pipeline",
            DynamicModelAlphaMode::Additive => "JKA GPU FX sprite src-alpha additive pipeline",
            DynamicModelAlphaMode::BlendUnlit => "JKA GPU FX sprite alpha-blend pipeline",
            DynamicModelAlphaMode::Modulate => "JKA GPU FX sprite modulate pipeline",
            DynamicModelAlphaMode::DstColorAdd => "JKA GPU FX sprite GL_DST_COLOR GL_ONE pipeline",
            DynamicModelAlphaMode::Modulate2x => "JKA GPU FX sprite modulate2x pipeline",
            DynamicModelAlphaMode::Darken => "JKA GPU FX sprite darken pipeline",
        }),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<FxGpuSpriteInstance>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &FX_GPU_SPRITE_INSTANCE_ATTRIBUTES,
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
            depth_write_enabled: Some(dynamic_model_writes_depth(alpha_mode)),
            depth_compare: Some(camera_depth_compare()),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: samples,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(match alpha_mode {
                DynamicModelAlphaMode::Mask | DynamicModelAlphaMode::MaskBlend => "fs_mask",
                DynamicModelAlphaMode::AdditiveOne | DynamicModelAlphaMode::Modulate => {
                    "fs_unlit_fog_black"
                }
                DynamicModelAlphaMode::Additive
                | DynamicModelAlphaMode::Blend
                | DynamicModelAlphaMode::BlendUnlit
                    if zero_alpha_discard =>
                {
                    "fs_unlit_zero_alpha"
                }
                DynamicModelAlphaMode::Additive
                | DynamicModelAlphaMode::BlendUnlit
                | DynamicModelAlphaMode::DstColorAdd
                | DynamicModelAlphaMode::Modulate2x
                | DynamicModelAlphaMode::Darken => "fs_unlit",
                _ => "fs_main",
            }),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &[("ENABLE_LEGACY_FOG", f64::from(u8::from(legacy_fog)))],
                ..Default::default()
            },
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: fx_sprite_blend(alpha_mode),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

pub(in crate::renderer) fn create_fx_sprite_wireframe_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    samples: u32,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("JKA GPU FX sprite wireframe pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<FxGpuSpriteInstance>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &FX_GPU_SPRITE_INSTANCE_ATTRIBUTES,
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
            module: shader,
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
}

pub(in crate::renderer) fn create_dynamic_model_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    samples: u32,
    alpha_mode: DynamicModelAlphaMode,
    legacy_fog: bool,
) -> wgpu::RenderPipeline {
    create_dynamic_model_pipeline_inner(
        device,
        layout,
        shader,
        surface_format,
        samples,
        alpha_mode,
        legacy_fog,
        false,
    )
}

pub(in crate::renderer) fn create_dynamic_model_pipeline_inner(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    samples: u32,
    alpha_mode: DynamicModelAlphaMode,
    legacy_fog: bool,
    rt_receiver: bool,
) -> wgpu::RenderPipeline {
    let vertex_layouts = [
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<DynamicModelVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &DYNAMIC_MODEL_VERTEX_ATTRIBUTES,
        },
        wgpu::VertexBufferLayout {
            array_stride: 16,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![4 => Float32x4],
        },
    ];
    let blend = match alpha_mode {
        DynamicModelAlphaMode::AdditiveOne => Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        }),
        DynamicModelAlphaMode::Additive => Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        }),
        DynamicModelAlphaMode::Blend
        | DynamicModelAlphaMode::MaskBlend
        | DynamicModelAlphaMode::BlendUnlit => Some(wgpu::BlendState::ALPHA_BLENDING),
        DynamicModelAlphaMode::Modulate => Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Dst,
                dst_factor: wgpu::BlendFactor::Zero,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        }),
        DynamicModelAlphaMode::DstColorAdd => Some(wgpu::BlendState {
            // Exact GL_DST_COLOR GL_ONE fixed-function equation:
            // src * dst + dst.
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Dst,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::DstAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        }),
        DynamicModelAlphaMode::Modulate2x => Some(wgpu::BlendState {
            // Exact GL_DST_COLOR GL_SRC_COLOR fixed-function equation:
            // src * dst + dst * src = 2 * src * dst.
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Dst,
                dst_factor: wgpu::BlendFactor::Src,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::DstAlpha,
                dst_factor: wgpu::BlendFactor::SrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        }),
        DynamicModelAlphaMode::Darken => Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::OneMinusSrc,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        }),
        _ => None,
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(match alpha_mode {
            DynamicModelAlphaMode::Opaque => "JKA dynamic model opaque pipeline",
            DynamicModelAlphaMode::Mask => "JKA dynamic model alpha-test pipeline",
            DynamicModelAlphaMode::Blend => "JKA dynamic model blend pipeline",
            DynamicModelAlphaMode::MaskBlend => {
                "JKA dynamic model alpha-test forced-alpha pipeline"
            }
            DynamicModelAlphaMode::AdditiveOne => {
                "JKA dynamic model GL_ONE GL_ONE additive unlit pipeline"
            }
            DynamicModelAlphaMode::Additive => {
                "JKA dynamic model src-alpha additive unlit pipeline"
            }
            DynamicModelAlphaMode::BlendUnlit => "JKA dynamic model alpha-blend unlit pipeline",
            DynamicModelAlphaMode::Modulate => "JKA dynamic model modulate unlit pipeline",
            DynamicModelAlphaMode::DstColorAdd => "JKA dynamic model GL_DST_COLOR GL_ONE pipeline",
            DynamicModelAlphaMode::Modulate2x => {
                "JKA dynamic model GL_DST_COLOR GL_SRC_COLOR pipeline"
            }
            DynamicModelAlphaMode::Darken => "JKA dynamic model darken unlit pipeline",
        }),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &vertex_layouts[..if rt_receiver { 2 } else { 1 }],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: Some(wgpu::Face::Back),
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(dynamic_model_writes_depth(alpha_mode)),
            depth_compare: Some(camera_depth_compare()),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: samples,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(match alpha_mode {
                DynamicModelAlphaMode::Mask | DynamicModelAlphaMode::MaskBlend => "fs_mask",
                // OpenJK GLFOGOVERRIDE_BLACK for GL_ONE GL_ONE and
                // GL_DST_COLOR GL_ZERO stages under r_drawfog 2.
                DynamicModelAlphaMode::AdditiveOne | DynamicModelAlphaMode::Modulate => {
                    "fs_unlit_fog_black"
                }
                DynamicModelAlphaMode::Additive
                | DynamicModelAlphaMode::BlendUnlit
                | DynamicModelAlphaMode::DstColorAdd
                | DynamicModelAlphaMode::Modulate2x
                | DynamicModelAlphaMode::Darken => "fs_unlit",
                _ => "fs_main",
            }),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &[("ENABLE_LEGACY_FOG", f64::from(u8::from(legacy_fog)))],
                ..Default::default()
            },
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

pub(in crate::renderer) fn create_ghoul2_skin_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    samples: u32,
    alpha_mode: DynamicModelAlphaMode,
    legacy_fog: bool,
) -> wgpu::RenderPipeline {
    let blend = match alpha_mode {
        DynamicModelAlphaMode::AdditiveOne => Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        }),
        DynamicModelAlphaMode::Additive => Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        }),
        DynamicModelAlphaMode::Blend
        | DynamicModelAlphaMode::MaskBlend
        | DynamicModelAlphaMode::BlendUnlit => Some(wgpu::BlendState::ALPHA_BLENDING),
        DynamicModelAlphaMode::Modulate => Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Dst,
                dst_factor: wgpu::BlendFactor::Zero,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        }),
        DynamicModelAlphaMode::DstColorAdd => Some(wgpu::BlendState {
            // Exact GL_DST_COLOR GL_ONE fixed-function equation:
            // src * dst + dst.
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Dst,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::DstAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        }),
        DynamicModelAlphaMode::Modulate2x => Some(wgpu::BlendState {
            // Exact GL_DST_COLOR GL_SRC_COLOR fixed-function equation:
            // src * dst + dst * src = 2 * src * dst.
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Dst,
                dst_factor: wgpu::BlendFactor::Src,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::DstAlpha,
                dst_factor: wgpu::BlendFactor::SrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        }),
        DynamicModelAlphaMode::Darken => Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::OneMinusSrc,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        }),
        _ => None,
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(match alpha_mode {
            DynamicModelAlphaMode::Opaque => "JKA Ghoul2 GPU skin opaque pipeline",
            DynamicModelAlphaMode::Mask => "JKA Ghoul2 GPU skin alpha-test pipeline",
            DynamicModelAlphaMode::Blend => "JKA Ghoul2 GPU skin blend pipeline",
            DynamicModelAlphaMode::MaskBlend => {
                "JKA Ghoul2 GPU skin alpha-test forced-alpha pipeline"
            }
            DynamicModelAlphaMode::AdditiveOne => {
                "JKA Ghoul2 GPU skin GL_ONE GL_ONE additive unlit pipeline"
            }
            DynamicModelAlphaMode::Additive => {
                "JKA Ghoul2 GPU skin src-alpha additive unlit pipeline"
            }
            DynamicModelAlphaMode::BlendUnlit => "JKA Ghoul2 GPU skin alpha-blend unlit pipeline",
            DynamicModelAlphaMode::Modulate => "JKA Ghoul2 GPU skin modulate unlit pipeline",
            DynamicModelAlphaMode::DstColorAdd => {
                "JKA Ghoul2 GPU skin GL_DST_COLOR GL_ONE pipeline"
            }
            DynamicModelAlphaMode::Modulate2x => {
                "JKA Ghoul2 GPU skin GL_DST_COLOR GL_SRC_COLOR pipeline"
            }
            DynamicModelAlphaMode::Darken => "JKA Ghoul2 GPU skin darken unlit pipeline",
        }),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Ghoul2GpuVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &GHOUL2_GPU_VERTEX_ATTRIBUTES,
            }],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: Some(wgpu::Face::Back),
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(dynamic_model_writes_depth(alpha_mode)),
            depth_compare: Some(camera_depth_compare()),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: samples,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(match alpha_mode {
                DynamicModelAlphaMode::Mask | DynamicModelAlphaMode::MaskBlend => "fs_mask",
                // OpenJK GLFOGOVERRIDE_BLACK for GL_ONE GL_ONE and
                // GL_DST_COLOR GL_ZERO stages under r_drawfog 2.
                DynamicModelAlphaMode::AdditiveOne | DynamicModelAlphaMode::Modulate => {
                    "fs_unlit_fog_black"
                }
                DynamicModelAlphaMode::Additive
                | DynamicModelAlphaMode::BlendUnlit
                | DynamicModelAlphaMode::DstColorAdd
                | DynamicModelAlphaMode::Modulate2x
                | DynamicModelAlphaMode::Darken => "fs_unlit",
                _ => "fs_main",
            }),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &[("ENABLE_LEGACY_FOG", f64::from(u8::from(legacy_fog)))],
                ..Default::default()
            },
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

pub(in crate::renderer) fn create_world_wireframe_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    msaa_samples: u32,
    shader_variant: WorldShaderVariantKey,
) -> wgpu::RenderPipeline {
    let shader_constants = shader_variant.compilation_constants();
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("JKA deformed-world wireframe pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &shader_constants,
                ..Default::default()
            },
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<GpuVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &VERTEX_ATTRIBUTES,
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
            count: msaa_samples,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_wireframe"),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &shader_constants,
                ..Default::default()
            },
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

pub(in crate::renderer) fn create_fast_world_wireframe_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    msaa_samples: u32,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("JKA fast deformed-world wireframe pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<GpuVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &VERTEX_ATTRIBUTES,
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
            count: msaa_samples,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
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
}

pub(in crate::renderer) fn create_world_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    key: PipelineKey,
    msaa_samples: u32,
    legacy_fog: bool,
    fog_pass: bool,
    shader_variant: Option<WorldShaderVariantKey>,
    legacy_dlight_pass: bool,
) -> wgpu::RenderPipeline {
    let blend = if legacy_dlight_pass {
        Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            // The projected-light redraw contributes RGB only. Preserve the
            // alpha produced by the authored material/fog passes.
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        })
    } else {
        match key.blend {
            BlendMode::Opaque => None,
            BlendMode::Custom(src, dst) => {
                let component = wgpu::BlendComponent {
                    src_factor: gpu_blend_factor(src),
                    dst_factor: gpu_blend_factor(dst),
                    operation: wgpu::BlendOperation::Add,
                };
                Some(wgpu::BlendState {
                    color: component,
                    alpha: component,
                })
            }
        }
    };
    let color_write = wgpu::ColorWrites::ALL;
    let shader_constants = shader_variant.map(WorldShaderVariantKey::compilation_constants);
    // wgpu validates specialization constants per entry point, not merely per
    // WGSL module. ENABLE_RAY_TRACED_SHADOWS is intentionally fragment-only, so
    // do not submit it while specializing vs_main. The remaining world overrides
    // are still fed to both stages because several existing variants consume them
    // in vertex work.
    let vertex_shader_constants = shader_constants.as_ref().map(|constants| {
        constants
            .iter()
            .copied()
            .filter(|(name, _)| {
                !matches!(*name, "ENABLE_RAY_TRACED_SHADOWS" | "ENABLE_RAY_TRACED_SUN")
            })
            .collect::<Vec<_>>()
    });
    let vertex_compilation_options = match vertex_shader_constants.as_ref() {
        Some(constants) => wgpu::PipelineCompilationOptions {
            constants,
            ..Default::default()
        },
        None => Default::default(),
    };
    let fragment_compilation_options = match shader_constants.as_ref() {
        Some(constants) => wgpu::PipelineCompilationOptions {
            constants,
            ..Default::default()
        },
        None => Default::default(),
    };
    let legacy_dlight_vertex_stream = shader_variant.is_some_and(|variant| variant.legacy_dlights);
    let vertex_entry_point = match (key.class, legacy_dlight_vertex_stream) {
        (DrawClass::Sky, true) => "vs_sky_legacy",
        (DrawClass::Sky, false) => "vs_sky",
        (_, true) => "vs_main_legacy",
        (_, false) => "vs_main",
    };
    let base_vertex_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<GpuVertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &VERTEX_ATTRIBUTES,
    };
    let legacy_surface_id_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<u32>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &LEGACY_DLIGHT_SURFACE_ID_ATTRIBUTES,
    };
    let vertex_buffers = if legacy_dlight_vertex_stream {
        vec![base_vertex_layout, legacy_surface_id_layout]
    } else {
        vec![base_vertex_layout]
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(if legacy_dlight_pass {
            "JKA BSP Legacy projected-dlight pass pipeline"
        } else if fog_pass {
            "JKA BSP legacy fog-pass pipeline"
        } else {
            "JKA BSP pipeline"
        }),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(vertex_entry_point),
            compilation_options: vertex_compilation_options,
            buffers: &vertex_buffers,
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: match key.cull {
                CullMode::None => None,
                CullMode::Front => Some(wgpu::Face::Front),
                CullMode::Back => Some(wgpu::Face::Back),
            },
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(if legacy_dlight_pass {
                false
            } else {
                key.depth_write
            }),
            depth_compare: Some(if legacy_dlight_pass || key.depth_equal {
                wgpu::CompareFunction::Equal
            } else {
                camera_depth_compare()
            }),
            stencil: wgpu::StencilState::default(),
            bias: if key.offset {
                polygon_offset_depth_bias()
            } else {
                wgpu::DepthBiasState::default()
            },
        }),
        multisample: wgpu::MultisampleState {
            count: msaa_samples,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(if legacy_dlight_pass {
                "fs_legacy_dlight_pass"
            } else if fog_pass {
                "fs_legacy_fog_pass"
            } else if key.class == DrawClass::Sky {
                "fs_sky"
            } else if legacy_fog {
                "fs_main_legacy_fog"
            } else {
                "fs_main"
            }),
            compilation_options: fragment_compilation_options,
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend,
                write_mask: color_write,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}
