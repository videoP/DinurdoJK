//! Rend2 local environment captures. See PROVENANCE.md in this directory.
use crate::renderer::{
    aabb_intersects_clip_frustum, camera_depth_clear, depth_format, CameraUniform, DrawClass,
    Renderer, Vec3, WorldShaderVariantKey,
};
use wgpu::util::DeviceExt;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const SOURCE: &str = include_str!("cubemap.wgsl");
#[path = "cubemap_view.rs"]
mod view;

fn texture(device: &wgpu::Device, label: &str, size: u32, layers: u32, mips: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: layers,
        },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}

fn face_view(texture: &wgpu::Texture, face: u32, mip: u32) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2),
        base_mip_level: mip,
        mip_level_count: Some(1),
        base_array_layer: face,
        array_layer_count: Some(1),
        ..Default::default()
    })
}

fn cube_view(texture: &wgpu::Texture, mip: Option<u32>) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::Cube),
        base_mip_level: mip.unwrap_or(0),
        mip_level_count: mip.map(|_| 1),
        array_layer_count: Some(6),
        ..Default::default()
    })
}

fn filter_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    entry: &str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(entry),
        layout: None,
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_fullscreen"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: crate::pipeline_cache::get().as_ref(),
    })
}

fn filter_pass(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::RenderPipeline,
    view: &wgpu::TextureView,
    group: Option<&wgpu::BindGroup>,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("Rend2 cubemap filter"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(pipeline);
    if let Some(group) = group {
        pass.set_bind_group(0, group, &[]);
    }
    pass.draw(0..3, 0..1);
}

pub(in crate::renderer) fn create_brdf_lut(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> wgpu::TextureView {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Rend2 environment integration"),
        source: wgpu::ShaderSource::Wgsl(SOURCE.into()),
    });
    let pipeline = filter_pipeline(device, &shader, "fs_brdf");
    let lut = texture(device, "Rend2 environment BRDF LUT", 128, 1, 1);
    let view = lut.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    filter_pass(&mut encoder, &pipeline, &view, None);
    queue.submit([encoder.finish()]);
    view
}

fn filter_group(
    device: &wgpu::Device,
    pipeline: &wgpu::RenderPipeline,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    values: [f32; 4],
) -> wgpu::BindGroup {
    let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Rend2 cubemap face/mip"),
        contents: bytemuck::cast_slice(&values),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Rend2 cubemap filter inputs"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: params.as_entire_binding(),
            },
        ],
    })
}

fn submit_capture_work(device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder) {
    // Rend2 issues pending commands per probe. Keep a map with many captures
    // from becoming one enormous GPU command buffer, while preserving queue
    // order with the lightmap work already recorded for this frame.
    let next = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("JKA frame after cubemap work") });
    let completed = std::mem::replace(encoder, next);
    queue.submit([completed.finish()]);
}

impl Renderer {
    pub(in crate::renderer) fn encode_generated_cubemaps(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        time: f32,
    ) {
        if !self.pbr_enabled || !self.pbr_cubemaps || !self.reflection_quality.pbr_probes_enabled()
        {
            return;
        }
        let Some(world) = self
            .world
            .as_ref()
            .filter(|w| w.reflection_probes.capture_pending)
        else {
            return;
        };
        let started = std::time::Instant::now();
        let captures: Vec<_> = world
            .reflection_probes
            .probes
            .iter()
            .enumerate()
            .filter_map(|(index, probe)| {
                probe.capture_position.map(|position| {
                    (
                        index,
                        position,
                        texture(
                            &self.device,
                            "Rend2 raw capture",
                            probe._texture.width(),
                            6,
                            probe._texture.width().ilog2() + 1,
                        ),
                    )
                })
            })
            .collect();
        if captures.is_empty() {
            return;
        }
        // The existing world material shader performs the capture, with the
        // main-camera-only effects disabled just as in a reflected view.
        let variant = WorldShaderVariantKey {
            pbr: true,
            pom: self.pom_enabled,
            pom_mip_aware: true,
            deluxe: self.deluxe_mapping_enabled,
            deluxe_specular: self.deluxe_specular > 0.0,
            pbr_vertex_lightgrid: world.active_pipeline_variant.pbr_vertex_lightgrid,
            baked_sun_relight: world.active_pipeline_variant.baked_sun_relight,
            ..Default::default()
        };
        let pipelines = crate::renderer::world::pipelines::create_reflection_pipelines(
            &self.device,
            &self.world_pipeline_layout_lean,
            &self.world_shader_lean,
            FORMAT,
            &world.coarse_batches,
            true,
            variant,
        );
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Rend2 cubemap convolution"),
                source: wgpu::ShaderSource::Wgsl(SOURCE.into()),
            });
        let downsample = filter_pipeline(&self.device, &shader, "fs_downsample");
        let prefilter = filter_pipeline(&self.device, &shader, "fs_prefilter");
        let size = captures[0].2.width();
        let depth = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Rend2 capture depth"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: depth_format(),
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth.create_view(&Default::default());
        let mut cameras = Vec::new();
        for &(_, position, _) in &captures {
            let position = Vec3::from_array(position);
            let mut faces = Vec::new();
            for face in 0..6 {
                let (matrix, direction) = view::view_projection(position, face);
                let uniform = CameraUniform {
                    view_proj: matrix.to_cols_array_2d(),
                    camera_pos_time: position.extend(time).to_array(),
                    clip_plane: [0.0; 4],
                    render_flags: [1, 0, 0, 0],
                    camera_forward: direction.extend(0.0).to_array(),
                    unjittered_view_proj: matrix.to_cols_array_2d(),
                    previous_unjittered_view_proj: matrix.to_cols_array_2d(),
                    jump_shade: [0.0; 4],
                    foliage_wind: [0.0; 4],
                };
                let buffer = self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("Rend2 capture camera"),
                        contents: bytemuck::bytes_of(&uniform),
                        usage: wgpu::BufferUsages::UNIFORM,
                    });
                let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Rend2 capture camera"),
                    layout: &self.camera_layout,
                    entries: &crate::foliage::camera_entries(
                        &buffer,
                        &self.weather.rain.gpu.wind_noise_view,
                        &self.weather.rain.gpu.wind_noise_sampler,
                    ),
                });
                faces.push((matrix, group));
            }
            cameras.push(faces);
        }
        // One direct capture plus two reflection bounces, matching the user's
        // TaystJK r_cubeMappingBounces=2. All faces of a bounce finish before
        // any destination probe is updated: never read and render one texture.
        for _bounce in 0..=2 {
            for (capture, faces) in captures.iter().zip(&cameras) {
                let (_, position, raw) = capture;
                let cluster = world
                    .visibility
                    .as_ref()
                    .and_then(|v| v.cluster_at(crate::scene::jka_position(*position)));
                let indices = cluster
                    .and_then(|c| world.coarse_visible_batches_by_cluster.get(c))
                    .unwrap_or(&world.coarse_all_batches);
                for (face, (matrix, camera)) in faces.iter().enumerate() {
                    let view = face_view(raw, face as u32, 0);
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("Rend2 cubemap capture"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &depth_view,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(camera_depth_clear()),
                                store: wgpu::StoreOp::Discard,
                            }),
                            stencil_ops: None,
                        }),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_bind_group(0, camera, &[]);
                    pass.set_bind_group(2, &world.lighting_bind_group_lean, &[]);
                    pass.set_bind_group(3, &self.shadow_resources.receiver_bind_group, &[]);
                    pass.set_bind_group(4, &self.planar_reflection.reflection_pass_bind_group, &[]);
                    pass.set_vertex_buffer(0, world.vertex_buffer.slice(..));
                    pass.set_vertex_buffer(1, world.legacy_dlight_surface_id_buffer.slice(..));
                    pass.set_index_buffer(world.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    for &index in indices {
                        let batch = &world.coarse_batches[index];
                        // Captures contain static BSP scenery, not players or
                        // moving entities, promoted FFT water or Force-only art.
                        if batch.is_inline_entity || batch.source.force_sight_only {
                            continue;
                        }
                        if batch.source.pipeline.class != DrawClass::Sky
                            && !aabb_intersects_clip_frustum(
                                batch.bounds_min,
                                batch.bounds_max,
                                *matrix,
                            )
                        {
                            continue;
                        }
                        let Some(pipeline) = pipelines.get(&batch.source.pipeline) else {
                            continue;
                        };
                        pass.set_pipeline(pipeline);
                        pass.set_bind_group(1, &batch.bind_group, &[]);
                        pass.draw_indexed(batch.full_indexed_range.clone(), 0, 0..1);
                    }
                }
                submit_capture_work(&self.device, &self.queue, encoder);
            }
            for (index, position, raw) in &captures {
                for mip in 1..raw.mip_level_count() {
                    let previous = cube_view(raw, Some(mip - 1));
                    let width = (raw.width() >> mip).max(1);
                    for face in 0..6 {
                        let group = filter_group(
                            &self.device,
                            &downsample,
                            &previous,
                            &world.reflection_probes.sampler,
                            [width as f32, face as f32, 0.0, 1.0],
                        );
                        filter_pass(
                            encoder,
                            &downsample,
                            &face_view(raw, face, mip),
                            Some(&group),
                        );
                    }
                }
                let destination = &world.reflection_probes.probes[*index]._texture;
                let luma = world
                    .entity_light_grid
                    .as_ref()
                    .and_then(|g| {
                        g.sample_classic_entity_light(crate::scene::jka_position(*position))
                    })
                    .map(|light| {
                        let luminance = |rgb: [f32; 3]| {
                            (rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722) / 255.0
                        };
                        luminance(light.directed)
                            .max(luminance(light.ambient))
                            .max(0.0001)
                    })
                    .unwrap_or(1.0);
                let source = cube_view(raw, None);
                for mip in 0..destination.mip_level_count() {
                    let roughness = mip as f32 / (destination.mip_level_count() - 1).max(1) as f32;
                    let width = (destination.width() >> mip).max(1);
                    for face in 0..6 {
                        let group = filter_group(
                            &self.device,
                            &prefilter,
                            &source,
                            &world.reflection_probes.sampler,
                            [width as f32, face as f32, roughness, luma],
                        );
                        filter_pass(
                            encoder,
                            &prefilter,
                            &face_view(destination, face, mip),
                            Some(&group),
                        );
                    }
                }
                submit_capture_work(&self.device, &self.queue, encoder);
            }
        }
        println!("[PBR CUBEMAPS] generated {} local probes, six {}x{} faces, GGX mips, 2 bounces; CPU encode {:.1} ms",
            captures.len(), size, size, started.elapsed().as_secs_f64()*1000.0);
        self.world
            .as_mut()
            .unwrap()
            .reflection_probes
            .capture_pending = false;
    }
}
