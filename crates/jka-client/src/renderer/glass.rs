use super::{
    camera_depth_compare, draw_world_batch, DrawIndexedIndirectArgs, DynamicModelAlphaMode,
    DynamicModelRenderer, DynamicModelVertex, Ghoul2GpuVertex, GpuVertex, Renderer, WorldBatch,
    WorldGpu, DEPTH_FORMAT, DYNAMIC_MODEL_VERTEX_ATTRIBUTES, GHOUL2_GPU_VERTEX_ATTRIBUTES,
    VERTEX_ATTRIBUTES,
};
use crate::materials::CullMode;
use std::collections::BTreeMap;

pub(super) struct WorldPipeline {
    key: (wgpu::TextureFormat, u32),
    pub pipelines: BTreeMap<CullMode, wgpu::RenderPipeline>,
    masks: BTreeMap<CullMode, wgpu::RenderPipeline>,
    md3_mask: wgpu::RenderPipeline,
    skin_mask: wgpu::RenderPipeline,
    pub settings: crate::glass::Settings,
}

impl Renderer {
    pub(in crate::renderer) fn prepare_glass(&mut self) {
        let mut settings = crate::glass::settings();
        let opaque_models = self
            .dynamic_model_renderer
            .draws
            .iter()
            .any(|draw| super::dynamic_model_writes_depth(draw.alpha_mode))
            || self
                .dynamic_model_renderer
                .skin_draws
                .iter()
                .any(|draw| super::dynamic_model_writes_depth(draw.alpha_mode));
        if !opaque_models || !settings.needs_background() {
            settings.background_recovery = 0.0;
        }
        self.ocean_optics.configure_background(
            &self.device,
            &self.ocean_optics_layout,
            settings.needs_background(),
        );
        let sky = self
            .world
            .as_ref()
            .map_or([0.1; 3], |world| world.sky_average);
        self.queue.write_buffer(
            &self.ocean_optics.glass_uniform,
            0,
            bytemuck::bytes_of(&settings.uniform(sky)),
        );
        let key = (self.scene_format(), self.msaa_samples);
        if self.glass_world.as_ref().is_some_and(|p| p.key == key) {
            self.glass_world.as_mut().unwrap().settings = settings;
            return;
        }
        println!(
            "Glass transmission: shared BSP/model optics; live styles bevy, frosted, liquid, ocean"
        );
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Shared glass transmission"),
                source: wgpu::ShaderSource::Wgsl(crate::glass::world_shader().into()),
            });
        let layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Glass transmission layout"),
                bind_group_layouts: &[
                    Some(&self.camera_layout),
                    Some(&self.surface_layout),
                    Some(&self.ocean_optics_layout),
                ],
                immediate_size: 0,
            });
        let pipelines = [CullMode::None, CullMode::Front, CullMode::Back]
            .into_iter()
            .map(|cull| {
                let pipeline =
                    self.device
                        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                            label: Some("Glass transmission (all styles)"),
                            layout: Some(&layout),
                            vertex: wgpu::VertexState {
                                module: &shader,
                                entry_point: Some("vs_main"),
                                compilation_options: Default::default(),
                                buffers: &[Some(wgpu::VertexBufferLayout {
                                    array_stride: std::mem::size_of::<GpuVertex>() as u64,
                                    step_mode: wgpu::VertexStepMode::Vertex,
                                    attributes: &VERTEX_ATTRIBUTES,
                                })],
                            },
                            primitive: wgpu::PrimitiveState {
                                cull_mode: match cull {
                                    CullMode::None => None,
                                    CullMode::Front => Some(wgpu::Face::Front),
                                    CullMode::Back => Some(wgpu::Face::Back),
                                },
                                ..Default::default()
                            },
                            depth_stencil: Some(wgpu::DepthStencilState {
                                format: DEPTH_FORMAT,
                                depth_write_enabled: Some(false),
                                depth_compare: Some(camera_depth_compare()),
                                stencil: Default::default(),
                                bias: Default::default(),
                            }),
                            multisample: wgpu::MultisampleState {
                                count: key.1,
                                ..Default::default()
                            },
                            fragment: Some(wgpu::FragmentState {
                                module: &shader,
                                entry_point: Some("fs_main"),
                                compilation_options: Default::default(),
                                targets: &[Some(wgpu::ColorTargetState {
                                    format: key.0,
                                    blend: None,
                                    write_mask: wgpu::ColorWrites::ALL,
                                })],
                            }),
                            multiview_mask: None,
                            cache: crate::pipeline_cache::get().as_ref(),
                        });
                (cull, pipeline)
            })
            .collect();
        let mask_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Glass geometry-only coverage layout"),
                bind_group_layouts: &[Some(&self.camera_layout)],
                immediate_size: 0,
            });
        let masks = [CullMode::None, CullMode::Front, CullMode::Back]
            .into_iter()
            .map(|cull| {
                (
                    cull,
                    mask_pipeline(
                        &self.device,
                        &mask_layout,
                        &shader,
                        std::mem::size_of::<GpuVertex>() as u64,
                        &VERTEX_ATTRIBUTES,
                        cull,
                        key.1,
                    ),
                )
            })
            .collect();
        let models = &self.dynamic_model_renderer;
        let md3_mask = mask_pipeline(
            &self.device,
            &models.pipeline_layout,
            &models.shader,
            std::mem::size_of::<DynamicModelVertex>() as u64,
            &DYNAMIC_MODEL_VERTEX_ATTRIBUTES,
            CullMode::None,
            key.1,
        );
        let skin_mask = mask_pipeline(
            &self.device,
            &models.skin_pipeline_layout,
            &models.skin_shader,
            std::mem::size_of::<Ghoul2GpuVertex>() as u64,
            &GHOUL2_GPU_VERTEX_ATTRIBUTES,
            CullMode::None,
            key.1,
        );
        self.glass_world = Some(WorldPipeline {
            key,
            pipelines,
            masks,
            md3_mask,
            skin_mask,
            settings,
        });
    }
}

fn mask_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    stride: u64,
    attributes: &[wgpu::VertexAttribute],
    cull: CullMode,
    samples: u32,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Glass geometry coverage"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: stride,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes,
            })],
        },
        primitive: wgpu::PrimitiveState {
            cull_mode: match cull {
                CullMode::None => None,
                CullMode::Front => Some(wgpu::Face::Front),
                CullMode::Back => Some(wgpu::Face::Back),
            },
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(camera_depth_compare()),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: samples,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_glass_mask"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba16Float,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: crate::pipeline_cache::get().as_ref(),
    })
}

impl WorldPipeline {
    pub(in crate::renderer) fn capture_geometry(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        optics: &crate::ocean::optics::Resources,
        camera: &wgpu::BindGroup,
        world: &WorldGpu,
        batches: &[WorldBatch],
        indices: &[usize],
        deferred: &[usize],
        models: &DynamicModelRenderer,
        gpu_driven: bool,
    ) {
        if !self.settings.needs_mask() {
            return;
        }
        let edges = &optics.edges;
        {
            let view = edges.mask_msaa.as_ref().unwrap_or(&edges.mask);
            let resolve = edges.mask_msaa.as_ref().map(|_| &edges.mask);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Glass geometry boundaries (no foreground actors)"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: resolve,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &edges.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(0, camera, &[]);
            pass.set_vertex_buffer(0, world.vertex_buffer.slice(..));
            pass.set_index_buffer(world.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            for &position in deferred {
                let batch = &batches[indices[position]];
                if !batch.source.glass_primary {
                    continue;
                }
                pass.set_pipeline(&self.masks[&batch.source.pipeline.cull]);
                if gpu_driven {
                    pass.draw_indexed_indirect(
                        &world.indirect_buffer,
                        u64::from(batch.cull_index)
                            * std::mem::size_of::<DrawIndexedIndirectArgs>() as u64,
                    );
                } else {
                    draw_world_batch(&mut pass, batch, 0..1, None);
                }
            }
            let (vertices, indices) = models.prepared_dynamic_buffers();
            pass.set_pipeline(&self.md3_mask);
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            for draw in models
                .draws
                .iter()
                .filter(|draw| draw.alpha_mode == DynamicModelAlphaMode::Glass)
            {
                let texture = draw
                    .texture_key
                    .as_ref()
                    .and_then(|key| models.textures.get(key))
                    .map_or(&models.fallback_bind_group, |texture| &texture.bind_group);
                pass.set_bind_group(1, texture, &[]);
                pass.draw_indexed(
                    draw.first_index..draw.first_index.saturating_add(draw.index_count),
                    draw.base_vertex,
                    0..1,
                );
            }
            pass.set_pipeline(&self.skin_mask);
            pass.set_bind_group(2, &models.skin_bind_group, &[]);
            for draw in models
                .skin_draws
                .iter()
                .filter(|draw| draw.alpha_mode == DynamicModelAlphaMode::Glass)
            {
                let Some(mesh) = models.ghoul2_meshes.get(&draw.mesh_key) else {
                    continue;
                };
                let texture = draw
                    .texture_key
                    .as_ref()
                    .and_then(|key| models.textures.get(key))
                    .map_or(&models.fallback_bind_group, |texture| &texture.bind_group);
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.set_bind_group(1, texture, &[]);
                pass.draw_indexed(
                    0..mesh.index_count,
                    0,
                    draw.first_instance..draw.first_instance.saturating_add(draw.instance_count),
                );
            }
        }
        // Bound both screen-space passes to what covered glass can read.
        let radius = self.settings.edge_radius(optics.key.0, optics.key.1);
        let blur_levels = self.settings.blur_levels(optics.blur_levels());
        let reach = crate::glass::edges::Reach {
            edge: crate::glass::edges::edge_pad(radius),
            blur: self.settings.blur_reach(blur_levels),
        };
        edges.locate(queue, encoder, reach, optics.key.2 == 1);
        if self.settings.needs_edges() {
            edges.resolve(encoder, radius);
        }
        if self.settings.needs_blur() {
            optics.blur(encoder, blur_levels);
        }
    }
}
