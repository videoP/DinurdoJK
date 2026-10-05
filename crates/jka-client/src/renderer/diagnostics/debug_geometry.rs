//! Diagnostics debug geometry.
use crate::renderer::{
    camera_depth_compare, pipeline_hash, scene, Arc, FxBlend, GpuVertex, PipelineJobKey,
    PipelineJobManager, Pod, UiVertex, Vec3, Zeroable, DEPTH_FORMAT, UI_VERTEX_ATTRIBUTES,
    VERTEX_ATTRIBUTES,
};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(in crate::renderer) struct DebugVolumeVertex {
    pub(in crate::renderer) position: [f32; 3],
    pub(in crate::renderer) color: [u8; 4],
}

pub(in crate::renderer) const DEBUG_VOLUME_ATTRIBUTES: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Unorm8x4];

pub(in crate::renderer) struct DebugVolumeMeshGpu {
    pub(in crate::renderer) vertices: wgpu::Buffer,
    pub(in crate::renderer) triangles: wgpu::Buffer,
    pub(in crate::renderer) triangle_count: u32,
    pub(in crate::renderer) lines: wgpu::Buffer,
    pub(in crate::renderer) line_count: u32,
}

/// Setup > Video > Debug & tools "Draw triggers" / "Draw clip brushes".
///
/// The brush volumes are triangulated once at map load (jka_assets
/// `Bsp::debug_volumes`) and uploaded only the first time a category is
/// enabled. Drawing is then at most one fill and one outline draw per enabled
/// category from static buffers: no per-frame CPU work and no per-entity
/// refEntity submission. Everything is skipped while both toggles are off.
#[derive(Default)]
pub(in crate::renderer) struct DebugVolumeRenderer {
    pub(in crate::renderer) cpu: Arc<jka_assets::bsp::DebugVolumes>,
    pub(in crate::renderer) triggers_enabled: bool,
    pub(in crate::renderer) clips_enabled: bool,
    pub(in crate::renderer) triggers: Option<DebugVolumeMeshGpu>,
    pub(in crate::renderer) clips: Option<DebugVolumeMeshGpu>,
    /// `r_drawEntities` boxes + link lines. Unlike `triggers`/`clips` this is
    /// replaced wholesale every time a new mesh arrives (built fresh on the
    /// app thread each tick) rather than uploaded once and left alone.
    pub(in crate::renderer) entities_enabled: bool,
    pub(in crate::renderer) entities: Option<DebugVolumeMeshGpu>,
    pub(in crate::renderer) fill_pipeline: Option<wgpu::RenderPipeline>,
    pub(in crate::renderer) line_pipeline: Option<wgpu::RenderPipeline>,
    /// Sun-to-head beam shown while the sun angles are being edited. Unlike the
    /// brush volumes this is rebuilt every frame (it follows the camera), but it
    /// is a dozen vertices and only exists while `sun_ray_enabled`.
    pub(in crate::renderer) sun_ray_enabled: bool,
    pub(in crate::renderer) sun_ray_buffer: Option<wgpu::Buffer>,
    pub(in crate::renderer) sun_ray_pipeline: Option<wgpu::RenderPipeline>,
    pub(in crate::renderer) sun_ray_ghost_pipeline: Option<wgpu::RenderPipeline>,
}

/// World units from the head to the drawn sun marker.
pub(in crate::renderer) const SUN_RAY_LENGTH: f32 = 4096.0;

pub(in crate::renderer) const SUN_RAY_MARKER_RADIUS: f32 = 112.0;

pub(in crate::renderer) const SUN_RAY_VERTEX_COUNT: usize = 12;

/// Two camera-facing quads: the beam from the sun down to the head, and a
/// diamond marking where the sun is. `toward_sun` is the (renderer-space) unit
/// vector from the head to the sun, i.e. the negated light travel direction.
pub(in crate::renderer) fn sun_ray_vertices(
    head: Vec3,
    toward_sun: Vec3,
    eye: Vec3,
) -> [DebugVolumeVertex; SUN_RAY_VERTEX_COUNT] {
    const COLOR: [u8; 4] = [255, 214, 72, 255];
    let toward_sun = toward_sun.normalize_or_zero();
    let sun = head + toward_sun * SUN_RAY_LENGTH;
    let vertex = |position: Vec3| DebugVolumeVertex {
        position: position.to_array(),
        color: COLOR,
    };

    // Keep the beam a roughly constant few pixels wide at any distance.
    let half_width = |point: Vec3| (eye.distance(point) * 0.0035).max(0.6);
    let mid = (head + sun) * 0.5;
    let axis = (head - sun).normalize_or_zero();
    let mut side = axis.cross(eye - mid).normalize_or_zero();
    if side == Vec3::ZERO {
        side = axis.any_orthonormal_vector();
    }
    let (sun_a, sun_b) = (sun - side * half_width(sun), sun + side * half_width(sun));
    let (head_a, head_b) = (
        head - side * half_width(head),
        head + side * half_width(head),
    );

    let view = (sun - eye).normalize_or_zero();
    let mut right = view.cross(Vec3::Y).normalize_or_zero();
    if right == Vec3::ZERO {
        right = Vec3::X;
    }
    let up = right.cross(view).normalize_or_zero();
    let (l, r) = (
        sun - right * SUN_RAY_MARKER_RADIUS,
        sun + right * SUN_RAY_MARKER_RADIUS,
    );
    let (d, u) = (
        sun - up * SUN_RAY_MARKER_RADIUS,
        sun + up * SUN_RAY_MARKER_RADIUS,
    );

    [
        vertex(sun_a),
        vertex(sun_b),
        vertex(head_b),
        vertex(sun_a),
        vertex(head_b),
        vertex(head_a),
        vertex(l),
        vertex(d),
        vertex(r),
        vertex(l),
        vertex(r),
        vertex(u),
    ]
}

impl DebugVolumeRenderer {
    pub(in crate::renderer) fn any_enabled(&self) -> bool {
        self.triggers_enabled || self.clips_enabled || self.entities_enabled || self.sun_ray_enabled
    }

    pub(in crate::renderer) fn update_sun_ray(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        head: Vec3,
        light_direction: Vec3,
        eye: Vec3,
    ) {
        if !self.sun_ray_enabled {
            return;
        }
        let vertices = sun_ray_vertices(head, -light_direction, eye);
        let buffer = self.sun_ray_buffer.get_or_insert_with(|| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("JKA sun ray preview"),
                size: std::mem::size_of_val(&vertices) as wgpu::BufferAddress,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        queue.write_buffer(buffer, 0, bytemuck::cast_slice(&vertices));
    }

    pub(in crate::renderer) fn set_map(&mut self, volumes: Arc<jka_assets::bsp::DebugVolumes>) {
        self.cpu = volumes;
        self.triggers = None;
        self.clips = None;
    }

    /// Replaces the entity-marker mesh immediately (not lazily like
    /// `triggers`/`clips`, since this is rebuilt fresh every tick while enabled).
    pub(in crate::renderer) fn set_entity_markers(
        &mut self,
        device: &wgpu::Device,
        mesh: Option<&jka_assets::bsp::DebugVolumeMesh>,
    ) {
        self.entities_enabled = mesh.is_some();
        self.entities = mesh.and_then(|mesh| Self::upload(device, mesh, "entities"));
    }

    pub(in crate::renderer) fn clear_pipelines(&mut self) {
        self.fill_pipeline = None;
        self.line_pipeline = None;
        self.sun_ray_pipeline = None;
        self.sun_ray_ghost_pipeline = None;
    }

    pub(in crate::renderer) fn upload(
        device: &wgpu::Device,
        mesh: &jka_assets::bsp::DebugVolumeMesh,
        label: &str,
    ) -> Option<DebugVolumeMeshGpu> {
        if mesh.is_empty() {
            return None;
        }
        let vertices = mesh
            .positions
            .iter()
            .zip(&mesh.colors)
            .map(|(&position, &color)| DebugVolumeVertex {
                position: scene::render_position(position),
                color,
            })
            .collect::<Vec<_>>();
        let make = |name: &str, contents: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("JKA debug volume {label} {name}")),
                contents,
                usage,
            })
        };
        Some(DebugVolumeMeshGpu {
            vertices: make(
                "vertices",
                bytemuck::cast_slice(&vertices),
                wgpu::BufferUsages::VERTEX,
            ),
            triangles: make(
                "triangles",
                bytemuck::cast_slice(&mesh.triangle_indices),
                wgpu::BufferUsages::INDEX,
            ),
            triangle_count: mesh.triangle_indices.len() as u32,
            lines: make(
                "lines",
                bytemuck::cast_slice(&mesh.line_indices),
                wgpu::BufferUsages::INDEX,
            ),
            line_count: mesh.line_indices.len() as u32,
        })
    }

    pub(in crate::renderer) fn ensure(
        &mut self,
        jobs: &mut PipelineJobManager,
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        shader: &wgpu::ShaderModule,
        surface_format: wgpu::TextureFormat,
        msaa_samples: u32,
    ) {
        if !self.any_enabled() {
            return;
        }
        let config = pipeline_hash(&(surface_format, msaa_samples));
        if self.sun_ray_enabled {
            for (slot, ghost) in [(0_u32, false), (1, true)] {
                let target = if ghost {
                    &mut self.sun_ray_ghost_pipeline
                } else {
                    &mut self.sun_ray_pipeline
                };
                if target.is_none() {
                    let key = PipelineJobKey::new("debug-sun-ray", slot, config);
                    if let Some(pipeline) = jobs.take_ready(key) {
                        *target = Some(pipeline);
                    } else {
                        let device = device.clone();
                        let layout = layout.clone();
                        let shader = shader.clone();
                        jobs.request(
                            key,
                            if ghost { "sun ray ghost" } else { "sun ray" },
                            move || {
                                create_sun_ray_pipeline(
                                    &device,
                                    &layout,
                                    &shader,
                                    surface_format,
                                    msaa_samples,
                                    ghost,
                                )
                            },
                        );
                    }
                }
            }
        }
        if !(self.triggers_enabled || self.clips_enabled || self.entities_enabled) {
            return;
        }
        if self.triggers_enabled && self.triggers.is_none() {
            self.triggers = Self::upload(device, &self.cpu.triggers, "triggers");
        }
        if self.clips_enabled && self.clips.is_none() {
            self.clips = Self::upload(device, &self.cpu.clips, "clips");
        }
        for (slot, lines) in [(0_u32, false), (1, true)] {
            let target = if lines {
                &mut self.line_pipeline
            } else {
                &mut self.fill_pipeline
            };
            if target.is_none() {
                let key = PipelineJobKey::new("debug-volumes", slot, config);
                if let Some(pipeline) = jobs.take_ready(key) {
                    *target = Some(pipeline);
                } else {
                    let device = device.clone();
                    let layout = layout.clone();
                    let shader = shader.clone();
                    jobs.request(
                        key,
                        if lines {
                            "debug volume lines"
                        } else {
                            "debug volume fill"
                        },
                        move || {
                            create_debug_volume_pipeline(
                                &device,
                                &layout,
                                &shader,
                                surface_format,
                                msaa_samples,
                                lines,
                            )
                        },
                    );
                }
            }
        }
    }

    /// Returns true when it bound its own vertex/index buffers, so the caller
    /// must restore the world geometry bindings before addressing BSP ranges.
    pub(in crate::renderer) fn draw<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        camera_bind_group: &'a wgpu::BindGroup,
    ) -> bool {
        let mut drew = false;
        if let (Some(fill), Some(line)) = (&self.fill_pipeline, &self.line_pipeline) {
            let meshes = [
                self.triggers.as_ref().filter(|_| self.triggers_enabled),
                self.clips.as_ref().filter(|_| self.clips_enabled),
                self.entities.as_ref().filter(|_| self.entities_enabled),
            ];
            for mesh in meshes.into_iter().flatten() {
                pass.set_bind_group(0, camera_bind_group, &[]);
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_pipeline(fill);
                pass.set_index_buffer(mesh.triangles.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.triangle_count, 0, 0..1);
                pass.set_pipeline(line);
                pass.set_index_buffer(mesh.lines.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.line_count, 0, 0..1);
                drew = true;
            }
        }
        if let (true, Some(buffer), Some(solid), Some(ghost)) = (
            self.sun_ray_enabled,
            &self.sun_ray_buffer,
            &self.sun_ray_pipeline,
            &self.sun_ray_ghost_pipeline,
        ) {
            pass.set_bind_group(0, camera_bind_group, &[]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            // The faint always-on-top copy shows where the beam goes behind
            // geometry; the solid copy disappears wherever the world blocks it,
            // which is exactly the occlusion the sun shadow should reproduce.
            pass.set_pipeline(ghost);
            pass.draw(0..SUN_RAY_VERTEX_COUNT as u32, 0..1);
            pass.set_pipeline(solid);
            pass.draw(0..SUN_RAY_VERTEX_COUNT as u32, 0..1);
            drew = true;
        }
        drew
    }
}

pub(in crate::renderer) fn create_debug_volume_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    msaa_samples: u32,
    outline: bool,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(if outline {
            "JKA debug volume outline pipeline"
        } else {
            "JKA debug volume fill pipeline"
        }),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<DebugVolumeVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &DEBUG_VOLUME_ATTRIBUTES,
            }],
        },
        primitive: wgpu::PrimitiveState {
            topology: if outline {
                wgpu::PrimitiveTopology::LineList
            } else {
                wgpu::PrimitiveTopology::TriangleList
            },
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        // The shader pulls clip z toward the camera; wgpu forbids depth bias on lines.
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(camera_depth_compare()),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: msaa_samples,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(if outline { "fs_line" } else { "fs_fill" }),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// Sun-ray preview pipeline: same vertex layout and shader as the debug
/// volumes. `ghost` ignores scene depth so the beam stays visible through walls.
pub(in crate::renderer) fn create_sun_ray_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    msaa_samples: u32,
    ghost: bool,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(if ghost {
            "JKA sun ray ghost pipeline"
        } else {
            "JKA sun ray pipeline"
        }),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<DebugVolumeVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &DEBUG_VOLUME_ATTRIBUTES,
            }],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(if ghost {
                wgpu::CompareFunction::Always
            } else {
                camera_depth_compare()
            }),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: msaa_samples,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(if ghost { "fs_ray_ghost" } else { "fs_ray" }),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

pub(in crate::renderer) fn create_surface_inspector_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    msaa_samples: u32,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("JKA surface inspector highlight pipeline"),
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
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(camera_depth_compare()),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: msaa_samples,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

pub(in crate::renderer) fn create_screen_fx_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    blend: FxBlend,
) -> wgpu::RenderPipeline {
    let blend_state = match blend {
        FxBlend::Add => Some(wgpu::BlendState {
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
        FxBlend::AddAlpha => Some(wgpu::BlendState {
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
        FxBlend::Alpha => Some(wgpu::BlendState::ALPHA_BLENDING),
        FxBlend::Modulate => Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Dst,
                dst_factor: wgpu::BlendFactor::Zero,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        }),
        FxBlend::DstColorAdd => Some(wgpu::BlendState {
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
        FxBlend::Modulate2x => Some(wgpu::BlendState {
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
        FxBlend::Darken => Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::OneMinusSrc,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        }),
        FxBlend::Opaque => None,
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("JKA CGame screen FX pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<UiVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &UI_VERTEX_ATTRIBUTES,
            }],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: blend_state,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

pub(in crate::renderer) fn create_ui_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("JKA UI pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<UiVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &UI_VERTEX_ATTRIBUTES,
            }],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}
