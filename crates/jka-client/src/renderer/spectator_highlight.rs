//! Spectator/demo through-wall player outlines.
//!
//! TaystJK's cg_specHud redraws occluded player refEntities with RF_NODEPTH and a
//! red/blue shield shader. DinurdoJK keeps the same eligibility semantics but uses
//! a modern silhouette mask: candidate players are drawn once without depth, then
//! visible pixels are erased against the final scene depth. The fullscreen pass
//! expands only the genuinely occluded remainder into a narrow outside edge.

use crate::renderer::{
    DynamicModelRenderer, DynamicModelVertex, DynamicWireframeClass, Ghoul2GpuVertex,
    SpectatorHighlight, SpectatorHighlightTeam, DYNAMIC_MODEL_VERTEX_ATTRIBUTES,
    GHOUL2_GPU_VERTEX_ATTRIBUTES, depth_format,
};

const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
// Allow shallow player/floor overlap to count as visible in world units.
const VISIBLE_ERASE_TOLERANCE_UNITS: f64 = 2.0;

pub(in crate::renderer) struct SpectatorHighlightResources {
    width: u32,
    height: u32,
    _mask_texture: Option<wgpu::Texture>,
    mask_view: Option<wgpu::TextureView>,
    _mask_msaa_texture: Option<wgpu::Texture>,
    mask_msaa_view: Option<wgpu::TextureView>,
    mask_samples: u32,
    mask_layout: wgpu::BindGroupLayout,
    mask_sampler: wgpu::Sampler,
    mask_bind_group: Option<wgpu::BindGroup>,
    surface_format: wgpu::TextureFormat,
    composite_pipeline: Option<wgpu::RenderPipeline>,
    model_mask_pipelines: Option<[wgpu::RenderPipeline; 3]>,
    skin_mask_pipelines: Option<[wgpu::RenderPipeline; 3]>,
    model_visible_clear_pipeline: Option<wgpu::RenderPipeline>,
    skin_visible_clear_pipeline: Option<wgpu::RenderPipeline>,
}

impl SpectatorHighlightResources {
    pub(in crate::renderer) fn new(
        device: &wgpu::Device,
        _models: &DynamicModelRenderer,
        surface_format: wgpu::TextureFormat,
        _width: u32,
        _height: u32,
    ) -> Self {
        // Keep the feature out of ordinary gameplay startup. The mask textures
        // and tiny PSOs are created on the first spectator/demo frame only.
        let mask_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA spectator outline mask texture layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let mask_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("JKA spectator outline mask sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        Self {
            width: 0,
            height: 0,
            _mask_texture: None,
            mask_view: None,
            _mask_msaa_texture: None,
            mask_msaa_view: None,
            mask_samples: 0,
            mask_layout,
            mask_sampler,
            mask_bind_group: None,
            surface_format,
            composite_pipeline: None,
            model_mask_pipelines: None,
            skin_mask_pipelines: None,
            model_visible_clear_pipeline: None,
            skin_visible_clear_pipeline: None,
        }
    }

    fn ensure_ready(
        &mut self,
        device: &wgpu::Device,
        models: &DynamicModelRenderer,
        width: u32,
        height: u32,
        samples: u32,
    ) {
        if self.composite_pipeline.is_none() {
            let composite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("JKA spectator outline composite layout"),
                bind_group_layouts: &[Some(&self.mask_layout)],
                immediate_size: 0,
            });
            let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("JKA spectator outline composite shader"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../spectator_highlight.wgsl").into(),
                ),
            });
            self.composite_pipeline = Some(device.create_render_pipeline(
                &wgpu::RenderPipelineDescriptor {
                    label: Some("JKA spectator outline composite pipeline"),
                    layout: Some(&composite_layout),
                    vertex: wgpu::VertexState {
                        module: &composite_shader,
                        entry_point: Some("vs_fullscreen"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &composite_shader,
                        entry_point: Some("fs_outline"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: self.surface_format,
                            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: crate::pipeline_cache::get().as_ref(),
                },
            ));
        }

        let samples = samples.max(1);
        if self.mask_samples != samples
            || self.model_mask_pipelines.is_none()
            || self.skin_mask_pipelines.is_none()
            || self.model_visible_clear_pipeline.is_none()
            || self.skin_visible_clear_pipeline.is_none()
        {
            self.model_mask_pipelines = Some(std::array::from_fn(|team| {
                create_mask_pipeline(
                    device,
                    &models.pipeline_layout,
                    &models.shader,
                    team_entry(team),
                    false,
                    samples,
                    None,
                )
            }));
            self.skin_mask_pipelines = Some(std::array::from_fn(|team| {
                create_mask_pipeline(
                    device,
                    &models.skin_pipeline_layout,
                    &models.skin_shader,
                    team_entry(team),
                    true,
                    samples,
                    None,
                )
            }));
            self.model_visible_clear_pipeline = Some(create_mask_pipeline(
                device,
                &models.pipeline_layout,
                &models.shader,
                "fs_spec_mask_clear",
                false,
                samples,
                Some(wgpu::CompareFunction::GreaterEqual),
            ));
            self.skin_visible_clear_pipeline = Some(create_mask_pipeline(
                device,
                &models.skin_pipeline_layout,
                &models.skin_shader,
                "fs_spec_mask_clear",
                true,
                samples,
                Some(wgpu::CompareFunction::GreaterEqual),
            ));
        }

        let width = width.max(1);
        let height = height.max(1);
        if self.width != width
            || self.height != height
            || self.mask_samples != samples
            || self.mask_view.is_none()
        {
            let (texture, view, msaa_texture, msaa_view, bind_group) = create_mask_target(
                device,
                &self.mask_layout,
                &self.mask_sampler,
                width,
                height,
                samples,
            );
            self.width = width;
            self.height = height;
            self.mask_samples = samples;
            self._mask_texture = Some(texture);
            self.mask_view = Some(view);
            self._mask_msaa_texture = msaa_texture;
            self.mask_msaa_view = msaa_view;
            self.mask_bind_group = Some(bind_group);
        }
    }

    pub(in crate::renderer) fn encode(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
        models: &DynamicModelRenderer,
        camera: &wgpu::BindGroup,
        scene_depth: &wgpu::TextureView,
        highlights: &[SpectatorHighlight],
        width: u32,
        height: u32,
        samples: u32,
    ) {
        if highlights.is_empty() {
            return;
        }
        self.ensure_ready(device, models, width, height, samples);
        let mask_view = self.mask_view.as_ref().expect("spectator mask created");
        let mask_render_view = self.mask_msaa_view.as_ref().unwrap_or(mask_view);
        let model_mask_pipelines = self
            .model_mask_pipelines
            .as_ref()
            .expect("spectator model mask pipelines created");
        let skin_mask_pipelines = self
            .skin_mask_pipelines
            .as_ref()
            .expect("spectator skin mask pipelines created");
        let model_visible_clear_pipeline = self
            .model_visible_clear_pipeline
            .as_ref()
            .expect("spectator model visible-clear pipeline created");
        let skin_visible_clear_pipeline = self
            .skin_visible_clear_pipeline
            .as_ref()
            .expect("spectator skin visible-clear pipeline created");
        let composite_pipeline = self
            .composite_pipeline
            .as_ref()
            .expect("spectator composite pipeline created");
        let mask_bind_group = self
            .mask_bind_group
            .as_ref()
            .expect("spectator mask bind group created");
        let mut teams = [None; 1024];
        for highlight in highlights {
            if let Some(slot) = teams.get_mut(usize::from(highlight.entity_num)) {
                *slot = Some(highlight.team);
            }
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("JKA spectator outline player mask"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: mask_render_view,
                    depth_slice: None,
                    // Preserve the multisampled candidate mask for the second
                    // pass; resolve only after visible pixels have been erased.
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            if !models.draws.is_empty() {
                let (vertices, indices) = models.prepared_dynamic_buffers();
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.set_bind_group(0, camera, &[]);
                pass.set_bind_group(1, &models.fallback_bind_group, &[]);
                for (team_index, team) in [
                    SpectatorHighlightTeam::Red,
                    SpectatorHighlightTeam::Blue,
                    SpectatorHighlightTeam::Yellow,
                ]
                .into_iter()
                .enumerate()
                {
                    pass.set_pipeline(&model_mask_pipelines[team_index]);
                    for draw in models.draws.iter().filter(|draw| {
                        draw.wireframe_class == DynamicWireframeClass::Player
                            && teams
                                .get(usize::from(draw.entity_num))
                                .copied()
                                .flatten()
                                == Some(team)
                    }) {
                        pass.draw_indexed(
                            draw.first_index..draw.first_index.saturating_add(draw.index_count),
                            draw.base_vertex,
                            0..1,
                        );
                    }
                }
            }

            if !models.skin_draws.is_empty() {
                pass.set_bind_group(0, camera, &[]);
                pass.set_bind_group(1, &models.fallback_bind_group, &[]);
                pass.set_bind_group(2, &models.skin_bind_group, &[]);
                for (team_index, team) in [
                    SpectatorHighlightTeam::Red,
                    SpectatorHighlightTeam::Blue,
                    SpectatorHighlightTeam::Yellow,
                ]
                .into_iter()
                .enumerate()
                {
                    pass.set_pipeline(&skin_mask_pipelines[team_index]);
                    for draw in models
                        .skin_draws
                        .iter()
                        .filter(|draw| draw.wireframe_class == DynamicWireframeClass::Player)
                    {
                        let Some(mesh) = models.ghoul2_meshes.get(&draw.mesh_key) else {
                            continue;
                        };
                        pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                        pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                        let first = usize::try_from(draw.first_instance).unwrap_or(usize::MAX);
                        let end = first.saturating_add(usize::try_from(draw.instance_count).unwrap_or(usize::MAX));
                        let end = end.min(models.skin_instance_entities.len());
                        let mut cursor = first.min(end);
                        while cursor < end {
                            while cursor < end
                                && teams
                                    .get(usize::from(models.skin_instance_entities[cursor]))
                                    .copied()
                                    .flatten()
                                    != Some(team)
                            {
                                cursor += 1;
                            }
                            let run_start = cursor;
                            while cursor < end
                                && teams
                                    .get(usize::from(models.skin_instance_entities[cursor]))
                                    .copied()
                                    .flatten()
                                    == Some(team)
                            {
                                cursor += 1;
                            }
                            if run_start < cursor {
                                pass.draw_indexed(
                                    0..mesh.index_count,
                                    0,
                                    u32::try_from(run_start).unwrap_or(u32::MAX)
                                        ..u32::try_from(cursor).unwrap_or(u32::MAX),
                                );
                            }
                        }
                    }
                }
            }
        }

        // Erase candidate pixels that are visible, or lie within 2 world units
        // behind scene depth. The vertex shader biases only the depth used by
        // this reverse-Z GreaterEqual test; the silhouette is unchanged.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("JKA spectator outline visible-player erase"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: mask_render_view,
                    depth_slice: None,
                    resolve_target: self.mask_msaa_view.as_ref().map(|_| mask_view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: scene_depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            if !models.draws.is_empty() {
                let (vertices, indices) = models.prepared_dynamic_buffers();
                pass.set_pipeline(model_visible_clear_pipeline);
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.set_bind_group(0, camera, &[]);
                pass.set_bind_group(1, &models.fallback_bind_group, &[]);
                for draw in models.draws.iter().filter(|draw| {
                    draw.wireframe_class == DynamicWireframeClass::Player
                        && teams
                            .get(usize::from(draw.entity_num))
                            .is_some_and(|team| team.is_some())
                }) {
                    pass.draw_indexed(
                        draw.first_index..draw.first_index.saturating_add(draw.index_count),
                        draw.base_vertex,
                        0..1,
                    );
                }
            }

            if !models.skin_draws.is_empty() {
                pass.set_pipeline(skin_visible_clear_pipeline);
                pass.set_bind_group(0, camera, &[]);
                pass.set_bind_group(1, &models.fallback_bind_group, &[]);
                pass.set_bind_group(2, &models.skin_bind_group, &[]);
                for draw in models
                    .skin_draws
                    .iter()
                    .filter(|draw| draw.wireframe_class == DynamicWireframeClass::Player)
                {
                    let Some(mesh) = models.ghoul2_meshes.get(&draw.mesh_key) else {
                        continue;
                    };
                    pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                    pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    let first = usize::try_from(draw.first_instance).unwrap_or(usize::MAX);
                    let end = first
                        .saturating_add(usize::try_from(draw.instance_count).unwrap_or(usize::MAX))
                        .min(models.skin_instance_entities.len());
                    let mut cursor = first.min(end);
                    while cursor < end {
                        while cursor < end
                            && !teams
                                .get(usize::from(models.skin_instance_entities[cursor]))
                                .is_some_and(|team| team.is_some())
                        {
                            cursor += 1;
                        }
                        let run_start = cursor;
                        while cursor < end
                            && teams
                                .get(usize::from(models.skin_instance_entities[cursor]))
                                .is_some_and(|team| team.is_some())
                        {
                            cursor += 1;
                        }
                        if run_start < cursor {
                            pass.draw_indexed(
                                0..mesh.index_count,
                                0,
                                u32::try_from(run_start).unwrap_or(u32::MAX)
                                    ..u32::try_from(cursor).unwrap_or(u32::MAX),
                            );
                        }
                    }
                }
            }
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("JKA spectator outline composite"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(composite_pipeline);
        pass.set_bind_group(0, mask_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn team_entry(index: usize) -> &'static str {
    match index {
        0 => "fs_spec_mask_red",
        1 => "fs_spec_mask_blue",
        _ => "fs_spec_mask_yellow",
    }
}

fn create_mask_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    fragment_entry: &'static str,
    skin: bool,
    samples: u32,
    depth_compare: Option<wgpu::CompareFunction>,
) -> wgpu::RenderPipeline {
    // Keep the selected layout in a named local so the slice borrowed by
    // VertexState lives for the entire create_render_pipeline call.  Building
    // `&[skin_layout]` inside the conditional creates a branch-local temporary
    // that is dropped too early (E0716).
    let vertex_layout = if skin {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Ghoul2GpuVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &GHOUL2_GPU_VERTEX_ATTRIBUTES,
        }
    } else {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<DynamicModelVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &DYNAMIC_MODEL_VERTEX_ATTRIBUTES,
        }
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(if skin {
            "JKA spectator outline Ghoul2 mask pipeline"
        } else {
            "JKA spectator outline model mask pipeline"
        }),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &[(
                    "SPEC_ERASE_DEPTH_TOLERANCE",
                    if depth_compare.is_some() {
                        VISIBLE_ERASE_TOLERANCE_UNITS
                    } else {
                        0.0
                    },
                )],
                ..Default::default()
            },
            buffers: &[Some(vertex_layout)],
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
        depth_stencil: depth_compare.map(|depth_compare| wgpu::DepthStencilState {
            format: depth_format(),
            depth_write_enabled: Some(false),
            depth_compare: Some(depth_compare),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: samples.max(1),
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment_entry),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: MASK_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: crate::pipeline_cache::get().as_ref(),
    })
}

fn create_mask_target(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    width: u32,
    height: u32,
    samples: u32,
) -> (
    wgpu::Texture,
    wgpu::TextureView,
    Option<wgpu::Texture>,
    Option<wgpu::TextureView>,
    wgpu::BindGroup,
) {
    // The hidden-player pass shares the main scene depth attachment, so its
    // color target must have the same dimensions/sample count.  Keep a
    // single-sample resolved texture for the fullscreen outline composite.
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("JKA spectator outline mask"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: MASK_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let samples = samples.max(1);
    let msaa_texture = (samples > 1).then(|| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("JKA spectator outline MSAA mask"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format: MASK_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    });
    let msaa_view = msaa_texture
        .as_ref()
        .map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()));
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA spectator outline mask bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    });
    (texture, view, msaa_texture, msaa_view, bind_group)
}
