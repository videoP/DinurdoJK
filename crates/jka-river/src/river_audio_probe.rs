//! Asynchronous 1x1 renderer probe of the solved and vertex-displaced river.
//! Not a second solver or a CPU approximation of the source radius. One MAX-
//! blended pixel represents the union of all visible resident river windows.
//! No resources or extra GPU work in maps without rivers/audio interest.
use std::{sync::mpsc::{self, Receiver}, time::{Duration, Instant}};

use crate::{river_runtime, river_surface::RiverSurface};

const INTERVAL: Duration = Duration::from_millis(100);

enum Readback {
    Idle,
    Copied(river_runtime::AudioListenerProbe),
    Mapping(river_runtime::AudioListenerProbe, Receiver<Result<(), wgpu::BufferAsyncError>>),
}

pub(super) struct AudioProbe {
    pipeline: wgpu::RenderPipeline,
    eye: wgpu::Buffer,
    eye_group: wgpu::BindGroup,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    staging: wgpu::Buffer,
    state: Readback,
    last_sample: Instant,
    last_poll: Instant,
}

impl AudioProbe {
    pub(super) fn new(
        device: &wgpu::Device,
        surface: &RiverSurface,
        camera: &wgpu::BindGroupLayout,
        shadow: &wgpu::BindGroupLayout,
    ) -> Self {
        let eye_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("River audio sample listener"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let eye = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("River audio listener eye and foot"), size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let eye_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("River audio listener group"), layout: &eye_layout,
            entries: &[wgpu::BindGroupEntry {binding: 4, resource: eye.as_entire_binding()}],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("River audio probe layout"),
            bind_group_layouts: &[
                Some(camera), Some(&surface.body_layout), Some(shadow),
                Some(&surface.frame_layout), Some(&surface.env_layout),
                Some(&surface.sky_layout), Some(&eye_layout),
            ],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("River audio one-pixel GPU classification"), layout: Some(&layout),
            vertex: wgpu::VertexState { module: &surface.module, entry_point: Some("vs_river_audio"),
                compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState {
                module: &surface.module, entry_point: Some("fs_river_audio"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Max,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Max,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(), multiview_mask: None, cache: None,
        });
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("River audio one-pixel occupancy"),
            size: wgpu::Extent3d {width:1,height:1,depth_or_array_layers:1},
            mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,
            format:wgpu::TextureFormat::Rgba8Unorm,
            usage:wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats:&[],
        });
        let view = texture.create_view(&Default::default());
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("River audio async 256-byte staging"), size: 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self { pipeline, eye, eye_group, texture, view, staging,
            state: Readback::Idle, last_sample: Instant::now() - INTERVAL, last_poll: Instant::now() }
    }

    /// Must be called after the river solver has written this frame's fields.
    /// Only one 1-pixel pass/readback per sampling interval, regardless of river count.
    pub(super) fn encode(
        &mut self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder,
        surface: &RiverSurface, camera: &wgpu::BindGroup, shadow: &wgpu::BindGroup,
        environment: Option<&wgpu::BindGroup>, rivers: &[crate::river_runtime::LiveRiver],
        sample: river_runtime::AudioListenerProbe,
    ) {
        if !matches!(self.state, Readback::Idle) || self.last_sample.elapsed() < INTERVAL { return; }
        // Audio ear arrives in JKA map units; shader river_view/body are in
        // centimetres of the native Unreal hydraulic lattice.
        let cm = jka_water::river::CENTIMETRES_PER_MAP_UNIT;
        queue.write_buffer(&self.eye, 0, bytemuck::cast_slice(&[
            sample.eye[0]*cm, sample.eye[1]*cm, sample.eye[2]*cm, 0.0f32,
            sample.foot[0]*cm, sample.foot[1]*cm, sample.foot[2]*cm, 0.0f32,
        ]));
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("River audio one-pixel test"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.view, depth_slice: None, resolve_target: None,
                    ops: wgpu::Operations {load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store},
                })],
                depth_stencil_attachment: None, ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            for river in rivers {
                let Some(body) = river.body_group() else { continue; };
                pass.set_bind_group(0, camera, &[]);
                pass.set_bind_group(1, body, &[]);
                pass.set_bind_group(2, shadow, &[]);
                if let Some(frame) = &surface.frame {pass.set_bind_group(3, frame, &[]);} else {continue;}
                pass.set_bind_group(4, environment.unwrap_or(&surface.inert_env), &[]);
                pass.set_bind_group(5, &surface.sky_group, &[]);
                pass.set_bind_group(6, &self.eye_group, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo { buffer: &self.staging,
                layout: wgpu::TexelCopyBufferLayout {offset:0,bytes_per_row:Some(256),rows_per_image:Some(1)} },
            wgpu::Extent3d {width:1,height:1,depth_or_array_layers:1},
        );
        self.state = Readback::Copied(sample);
        self.last_sample = Instant::now();
    }

    pub(super) fn poll(&mut self, device: &wgpu::Device) {
        if let Readback::Copied(sample) = &self.state {
            let sample = *sample;
            let (tx, rx) = mpsc::channel();
            self.staging.slice(..).map_async(wgpu::MapMode::Read, move |result| { let _=tx.send(result); });
            self.state = Readback::Mapping(sample, rx);
        }
        let Readback::Mapping(sample, rx) = &self.state else { return; };
        // Poll, never wait. This covers renderers that do not otherwise pump
        // map_async callbacks when the client has no other readbacks pending.
        if self.last_poll.elapsed() >= Duration::from_millis(50) {
            let _ = device.poll(wgpu::PollType::Poll);
            self.last_poll = Instant::now();
        }
        let Ok(result) = rx.try_recv() else { return; };
        let sampled = *sample;
        let water = if result.is_ok() {
            let view = self.staging.slice(..).get_mapped_range();
            let contact = view.ok().map(|v| river_runtime::RiverAudioContact {
                eye: v[0] > 127, foot: v[1] > 127,
            });
            self.staging.unmap();
            contact
        } else { self.staging.unmap(); None };
        self.state = Readback::Idle;
        // Failed GPU maps are missing samples, not an artificial water exit.
        if let Some(water) = water {
            river_runtime::publish_audio_result(sampled, water);
        }
    }
}
