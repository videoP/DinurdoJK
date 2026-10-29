//! Native display output. World, AA and UI compose in linear FP16 before encoding.
use crate::renderer::Renderer;
use std::time::{Duration, Instant};
use wgpu::util::DeviceExt;

pub(crate) const OUTPUT_LABELS: [&str; 4] = ["SDR", "HDR Auto", "HDR10 (PQ)", "HDR scRGB"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Selection {
    pub format: wgpu::TextureFormat,
    pub color_space: wgpu::SurfaceColorSpace,
    pub encoding: u32,
}

pub(super) fn select(
    caps: &wgpu::SurfaceCapabilities,
    requested: u32,
    info: Option<&wgpu::DisplayHdrInfo>,
) -> Option<Selection> {
    let definitely_sdr = info
        .and_then(|i| i.coarse)
        .and_then(|c| c.high_dynamic_range)
        == Some(false);
    let supports = |format, space: wgpu::SurfaceColorSpace| {
        caps.color_spaces(format)
            .contains(space.to_color_spaces().unwrap())
    };
    let scrgb = Selection {
        format: wgpu::TextureFormat::Rgba16Float,
        color_space: wgpu::SurfaceColorSpace::ExtendedSrgbLinear,
        encoding: 1,
    };
    let pq = Selection {
        format: wgpu::TextureFormat::Rgb10a2Unorm,
        color_space: wgpu::SurfaceColorSpace::Bt2100Pq,
        encoding: 2,
    };
    if requested != 0 && !definitely_sdr {
        // Prefer FP16 scRGB for Windows precision and compositor integration.
        let choices: &[Selection] = match requested {
            1 => &[scrgb, pq],
            2 => &[pq],
            3 => &[scrgb],
            _ => &[],
        };
        for choice in choices {
            if supports(choice.format, choice.color_space) {
                return Some(*choice);
            }
        }
    }
    for format in [
        wgpu::TextureFormat::Bgra8UnormSrgb,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        wgpu::TextureFormat::Bgra8Unorm,
        wgpu::TextureFormat::Rgba8Unorm,
    ] {
        if supports(format, wgpu::SurfaceColorSpace::Srgb) {
            return Some(Selection {
                format,
                color_space: wgpu::SurfaceColorSpace::Srgb,
                encoding: if format.is_srgb() { 0 } else { 3 },
            });
        }
    }
    None
}

/// Bit 0 is SDR; bits 1..3 advertise currently usable HDR Auto/PQ/scRGB.
/// Use the same selection rule as the swapchain so the menu cannot claim a
/// mode is available when the renderer would immediately fall back to SDR.
/// This is display + adapter + backend specific, not a global GPU property.
pub(super) fn available_modes(
    caps: &wgpu::SurfaceCapabilities,
    info: Option<&wgpu::DisplayHdrInfo>,
) -> u8 {
    let mut modes = 1_u8; // SDR is always the user-facing recovery option.
    for requested in 1..=3 {
        if select(caps, requested, info)
            .is_some_and(|selection| matches!(selection.encoding, 1 | 2))
        {
            modes |= 1 << requested;
        }
    }
    modes
}

/// Native compositor only when the *selected* swapchain is HDR, never just
/// because the user requested HDR or the display is in SDR fallback.
pub(super) fn requires_compositor(selection: Selection) -> bool {
    matches!(selection.encoding, 1 | 2)
}

pub(super) struct HdrOutput {
    pub selection: Selection,
    pub requested: u32,
    pub paper_white: f32,
    pub peak: f32,
    pub detected_peak: Option<f32>,
    pub view: wgpu::TextureView,
    pub ui_view: wgpu::TextureView,
    pub capture: wgpu::Texture,
    capture_view: wgpu::TextureView,
    uniform: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    shader: wgpu::ShaderModule,
    output_pipeline: wgpu::RenderPipeline,
    ui_pipeline: wgpu::RenderPipeline,
    capture_pipeline: wgpu::RenderPipeline,
    capture_alpha_pipeline: wgpu::RenderPipeline,
    scene_group: wgpu::BindGroup,
    ui_group: wgpu::BindGroup,
    capture_group: wgpu::BindGroup,
    last_poll: Instant,
    last_status: String,
}

fn pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    entry: &str,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("HDR output layout"),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("HDR output pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: crate::pipeline_cache::get().as_ref(),
    })
}

fn target(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("HDR composition target"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

impl HdrOutput {
    pub fn new(
        device: &wgpu::Device,
        selection: Selection,
        requested: u32,
        width: u32,
        height: u32,
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("HDR output bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("HDR display encoding"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../hdr_output.wgsl").into()),
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("HDR output settings"),
            contents: bytemuck::cast_slice(&[selection.encoding as f32, 200.0f32, 1000.0, 1.0]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let capture_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("HDR SDR capture settings"),
            contents: bytemuck::cast_slice(&[0.0f32, 200.0, 1000.0, 1.0]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let view = target(device, width, height, wgpu::TextureFormat::Rgba16Float)
            .create_view(&Default::default());
        let ui_view = target(device, width, height, wgpu::TextureFormat::Rgba8Unorm)
            .create_view(&Default::default());
        let capture = target(device, width, height, wgpu::TextureFormat::Bgra8UnormSrgb);
        let capture_view = capture.create_view(&Default::default());
        let group = |view: &wgpu::TextureView, buffer: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("HDR output group"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: buffer.as_entire_binding(),
                    },
                ],
            })
        };
        let scene_group = group(&view, &uniform);
        let ui_group = group(&ui_view, &uniform);
        let capture_group = group(&view, &capture_uniform);
        let output_pipeline = pipeline(
            device,
            &layout,
            &shader,
            selection.format,
            "fs_output",
            None,
        );
        let ui_pipeline = pipeline(
            device,
            &layout,
            &shader,
            wgpu::TextureFormat::Rgba16Float,
            "fs_ui",
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        );
        let capture_pipeline = pipeline(
            device,
            &layout,
            &shader,
            wgpu::TextureFormat::Bgra8UnormSrgb,
            "fs_output",
            None,
        );
        let capture_alpha_pipeline = pipeline(
            device, &layout, &shader, wgpu::TextureFormat::Bgra8UnormSrgb,
            "fs_capture_alpha", None,
        );
        Self {
            selection,
            requested,
            paper_white: 200.0,
            peak: 1000.0,
            detected_peak: None,
            view,
            ui_view,
            capture,
            capture_view,
            uniform,
            layout,
            shader,
            output_pipeline,
            ui_pipeline,
            capture_pipeline,
            capture_alpha_pipeline,
            scene_group,
            ui_group,
            capture_group,
            last_poll: Instant::now(),
            last_status: String::new(),
        }
    }

    pub fn write_settings(&mut self, queue: &wgpu::Queue, white: f32, peak: f32, rolloff: f32) {
        self.peak = if peak > 0.0 && peak.is_finite() {
            peak.clamp(80.0, 10000.0)
        } else {
            self.detected_peak
                .filter(|v| v.is_finite() && *v >= 80.0)
                .unwrap_or(1000.0)
                .clamp(80.0, 10000.0)
        };
        self.paper_white = if white.is_finite() {
            white.clamp(80.0, 500.0)
        } else {
            200.0
        }
        .min(self.peak);
        queue.write_buffer(
            &self.uniform,
            0,
            bytemuck::cast_slice(&[
                self.selection.encoding as f32,
                self.paper_white,
                self.peak,
                rolloff.clamp(0.0, 1.0),
            ]),
        );
    }

    pub fn status(&self) -> String {
        let mode = match self.selection.encoding {
            1 => "HDR scRGB", 2 => "HDR10 / PQ",
            _ => return "SDR fallback: HDR is unavailable on this display/backend or disabled in Windows".to_owned(),
        };
        format!(
            "{mode} | paper white {:.0} nits | peak {:.0} nits",
            self.paper_white, self.peak
        )
    }

    fn pass(
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        pipeline: &wgpu::RenderPipeline,
        group: &wgpu::BindGroup,
        load: wgpu::LoadOp<wgpu::Color>,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("HDR composition / output"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.draw(0..3, 0..1);
    }
    pub fn composite_ui(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        destination: &wgpu::TextureView,
    ) {
        Self::pass(
            encoder,
            destination,
            &self.ui_pipeline,
            &self.ui_group,
            wgpu::LoadOp::Load,
        );
    }
    pub fn submit(
        &self, device: &wgpu::Device, queue: &wgpu::Queue,
        surface: &wgpu::TextureView, capture: bool,
    ) {
        self.submit_with_alpha(device, queue, surface, capture, false);
    }

    pub fn submit_with_alpha(
        &self, device: &wgpu::Device, queue: &wgpu::Queue,
        surface: &wgpu::TextureView, capture: bool, transparent: bool,
    ) {
        let mut encoder = device.create_command_encoder(&Default::default());
        Self::pass(
            &mut encoder,
            surface,
            &self.output_pipeline,
            &self.scene_group,
            wgpu::LoadOp::Clear(wgpu::Color::BLACK),
        );
        if capture {
            Self::pass(
                &mut encoder,
                &self.capture_view,
                if transparent { &self.capture_alpha_pipeline } else { &self.capture_pipeline },
                &self.capture_group,
                wgpu::LoadOp::Clear(wgpu::Color::BLACK),
            );
        }
        queue.submit([encoder.finish()]);
    }
}

impl Renderer {
    pub(super) fn configure_surface(&self) {
        if self.offscreen_output.is_some() { return; }
        let mut config =
            super::context::presentation_config(&self.config, self.surface_linear_present);
        if let Some(hdr) = &self.hdr_output {
            config.format = hdr.selection.format;
            config.color_space = hdr.selection.color_space;
            config.view_formats.clear();
        }
        self.surface.configure(&self.device, &config);
    }
    pub(super) fn resize_hdr_output(&mut self) {
        if let Some(old) = self.hdr_output.take() {
            let mut output = HdrOutput::new(
                &self.device,
                old.selection,
                old.requested,
                self.config.width,
                self.config.height,
            );
            output.detected_peak = old.detected_peak;
            output.write_settings(
                &self.queue,
                old.paper_white,
                old.peak,
                self.tone_mapping_strength,
            );
            self.hdr_output = Some(output);
        }
    }
    pub(super) fn poll_hdr_display(&mut self) -> bool {
        let Some(output) = self.hdr_output.as_mut() else {
            return false;
        };
        if output.last_poll.elapsed() < Duration::from_secs(2) {
            return false;
        }
        output.last_poll = Instant::now();
        let info = self.surface.display_hdr_info(&self.adapter);
        output.detected_peak = info.luminance.and_then(|l| l.max_nits);
        let Some(selection) = select(
            &self.surface.get_capabilities(&self.adapter),
            output.requested,
            Some(&info),
        ) else {
            return false;
        };
        let changed = output.selection != selection;
        if changed {
            // HDR disappeared (Windows HDR toggle / display move): stay on SDR
            // until the client rebuilds the renderer with HDR Math Only.
            // Do not keep the native output compositor alive on SDR fallback.
            if matches!(output.selection.encoding, 1 | 2)
                && !matches!(selection.encoding, 1 | 2)
            {
                output.requested = 0;
            }
            output.selection = selection;
            output.output_pipeline = pipeline(
                &self.device,
                &output.layout,
                &output.shader,
                selection.format,
                "fs_output",
                None,
            );
        }
        output.write_settings(
            &self.queue,
            self.ui_state.video.hdr_paper_white,
            self.ui_state.video.hdr_peak,
            self.tone_mapping_strength,
        );
        let status = output.status();
        let status_changed = output.last_status != status;
        output.last_status = status;
        if changed {
            self.configure_surface();
            self.note_surface_configured("hdr_display_change");
        }
        changed || status_changed
    }
}

#[cfg(test)]
#[path = "hdr_output/tests.rs"]
mod tests;
