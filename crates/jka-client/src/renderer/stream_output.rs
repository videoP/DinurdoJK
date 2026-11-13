//! Independent, privacy-filtered OBS video output.
//!
//! The main 3D renderer remains single-pass. After world/post and before ANY
//! local egui/console/HUD paint, this copies the existing SDR scene and paints
//! a freshly redacted native 2D snapshot onto its own target. DX12 exposes that
//! private target to Spout2 through DX12/D3D11On12 or Vulkan/D3D11 KMT.
//! Neither transport reads pixels through system memory.
//!
//! Only frames due for OBS (`capture_due`, at most `STREAM_FPS`) do any of
//! this. Ordinarily the copy is encoded inside the main frame and the stream's
//! command buffers ride its submission; `submit_scene` is the deferred form
//! for layouts whose scene finishes after the main submit.
use crate::renderer::{
    ui, Renderer, UiVertex, UI_BUFFER_BYTES, UI_DYNAMIC_BUFFER_BYTES, UI_TRANSIENT_BUFFER_BYTES,
};
use crate::streamer::{redact_stream_snapshot, StreamerSettings};
#[cfg(windows)]
#[path = "dx12_spout.rs"]
mod dx12_spout;
#[cfg(windows)]
#[path = "vulkan_spout.rs"]
mod vulkan_spout;
use std::time::{Duration, Instant};

const STREAM_FPS: u32 = 60;
const STREAM_NAME: &str = "DinurdoJK Stream";
const STREAM_SHADER_MAX_VERTICES: u64 = 16_384;

struct FrameResources {
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    view_format: wgpu::TextureFormat,
    color: wgpu::Texture,
    view: wgpu::TextureView,
    encoded_view: wgpu::TextureView,
    ui_buffer: wgpu::Buffer,
    shader_buffer: wgpu::Buffer,
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a DX12 GPU and Spout shared-memory access"]
    fn dx12_stream_toggle_resize_and_privacy_lifecycle() {
        crate::renderer::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::DX12,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter = instance.request_adapter(&Default::default()).await.unwrap();
            let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let mut output = StreamOutput::default();
            assert!(!output.enabled());
            assert!(output.resources.is_none() && output.dx12_sender.is_none());
            for width in [16, 32, 16] {
                output.set_settings(StreamerSettings {
                    enabled: true,
                    ..Default::default()
                });
                let format = wgpu::TextureFormat::Bgra8Unorm;
                output.ensure_resources(&device, width, 16, format, format);
                let resources = output.resources.as_ref().unwrap();
                output.dx12_sender = Some(
                    dx12_spout::Dx12Sender::new(
                        &device,
                        &resources.color,
                        format,
                        "DinurdoJK Stream Lifecycle Test",
                    )
                    .unwrap(),
                );
                for _ in 0..4 {
                    // Repeated WGPU -> Spout -> WGPU ownership transfers exercise
                    // the native restoration barriers and allocator reuse.
                    let mut encoder = device.create_command_encoder(&Default::default());
                    drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &resources.view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        ..Default::default()
                    }));
                    queue.submit([encoder.finish()]);
                    assert!(output.dx12_sender.as_mut().unwrap().publish().unwrap());
                    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
                }
                output.set_settings(StreamerSettings {
                    enabled: true,
                    hide_team_chat: false,
                    ..Default::default()
                });
                assert!(output.dx12_sender.is_none());
                output.set_settings(StreamerSettings::default());
                assert!(!output.enabled());
                assert!(output.resources.is_none() && output.dx12_sender.is_none());
            }
            assert!(scope.pop().await.is_none());
        });
    }
}

pub(in crate::renderer) struct StreamOutput {
    settings: StreamerSettings,
    // Native sender must be destroyed before the WGPU texture it wraps.
    #[cfg(windows)]
    dx12_sender: Option<dx12_spout::Dx12Sender>,
    #[cfg(windows)]
    vulkan_sender: Option<vulkan_spout::VulkanSender>,
    resources: Option<FrameResources>,
    last_frame: Option<Instant>,
    warned_unsupported: bool,
}

impl Default for StreamOutput {
    fn default() -> Self {
        Self {
            settings: StreamerSettings::default(),
            #[cfg(windows)]
            dx12_sender: None,
            #[cfg(windows)]
            vulkan_sender: None,
            resources: None,
            last_frame: None,
            warned_unsupported: false,
        }
    }
}

impl StreamOutput {
    pub fn enabled(&self) -> bool {
        self.settings.enabled
    }

    pub fn pause_unsupported(&mut self, reason: &str) {
        // Fail closed. Never fall back to game-window capture or CPU readback.
        #[cfg(windows)]
        {
            self.dx12_sender = None;
            self.vulkan_sender = None;
        }
        // On any D3D11On12 error, discard the native texture: its actual D3D12
        // state can differ from WGPU's tracker if release failed mid-send.
        self.resources = None;
        if !self.warned_unsupported {
            eprintln!("[STREAMER] Spout2 GPU stream unavailable: {reason}");
            self.warned_unsupported = true;
        }
    }

    pub fn set_settings(&mut self, settings: StreamerSettings) {
        if self.settings == settings {
            return;
        }
        self.settings = settings;
        // Unregister the old sender on privacy changes. Previously published
        // frames already consumed by OBS cannot be retroactively withdrawn.
        #[cfg(windows)]
        {
            self.dx12_sender = None;
            self.vulkan_sender = None;
        }
        if !settings.enabled {
            self.resources = None;
            self.last_frame = None;
        }
        self.warned_unsupported = false;
    }

    fn ensure_resources(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        view_format: wgpu::TextureFormat,
    ) {
        if self.resources.as_ref().is_some_and(|r| {
            r.width == width
                && r.height == height
                && r.format == format
                && r.view_format == view_format
        }) {
            return;
        }
        #[cfg(windows)]
        {
            self.dx12_sender = None;
            self.vulkan_sender = None;
        } // wrapper outlives neither resource nor dimensions
        self.resources = None;
        // Offer both the regular sampled view and its encoded-color alias.
        // This is metadata on ONE stream texture, not a new composition image.
        let encoded_format = view_format.remove_srgb_suffix();
        let mut view_formats = Vec::with_capacity(2);
        for variant in [view_format, encoded_format] {
            if variant != format && !view_formats.contains(&variant) {
                view_formats.push(variant);
            }
        }
        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("JKA streamer private composition"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &view_formats,
        });
        let view = color.create_view(&wgpu::TextureViewDescriptor {
            format: Some(view_format),
            ..Default::default()
        });
        let encoded_view = color.create_view(&wgpu::TextureViewDescriptor {
            format: Some(encoded_format),
            ..Default::default()
        });
        let ui_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA streamer-only 2D vertices"),
            size: UI_BUFFER_BYTES + UI_DYNAMIC_BUFFER_BYTES + UI_TRANSIENT_BUFFER_BYTES,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });
        let shader_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA streamer authored HUD stages"),
            size: STREAM_SHADER_MAX_VERTICES * std::mem::size_of::<UiVertex>() as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });
        self.resources = Some(FrameResources {
            width,
            height,
            format,
            view_format,
            color,
            view,
            encoded_view,
            ui_buffer,
            shader_buffer,
        });
    }

    /// Per-frame gate, evaluated before the frame layout is chosen. Only a
    /// frame that will publish to OBS (at most `STREAM_FPS` per second) may
    /// change the layout; every other frame renders exactly as streaming off.
    pub(in crate::renderer) fn capture_due(&mut self, now: Instant) -> bool {
        if !self.settings.enabled || self.warned_unsupported {
            return false;
        }
        if self.last_frame.is_some_and(|t| {
            now.duration_since(t) < Duration::from_secs_f64(1.0 / f64::from(STREAM_FPS))
        }) {
            return false;
        }
        // An existing sender must accept a frame now, else this OBS frame is
        // dropped instead of paid for. `prepare` creates a missing sender.
        #[cfg(windows)]
        {
            if self.dx12_sender.as_ref().is_some_and(|sender| !sender.ready()) {
                return false;
            }
            if let Some(sender) = self.vulkan_sender.as_mut() {
                if let Err(error) = sender.poll_ready() {
                    self.pause_unsupported(&error);
                    return false;
                }
                if sender.free_slot().is_none() {
                    return false;
                }
            }
        }
        true
    }

    /// Deferred capture, for layouts whose scene is only final after the main
    /// submit (SMAA, CRT, menu backdrop). Called BEFORE egui/native local
    /// overlay. Source is the SDR backbuffer or early-present target.
    pub(in crate::renderer) fn submit_scene(
        &mut self,
        renderer: &mut Renderer,
        source: &wgpu::Texture,
        now: Instant,
    ) {
        let Some(mut capture) = self.prepare(renderer, source, now) else {
            return;
        };
        let mut encoder = renderer
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("JKA streamer deferred scene copy"),
            });
        capture.encode_scene_copy(&mut encoder);
        renderer.queue.submit(
            std::iter::once(encoder.finish()).chain(capture.take_command_buffers()),
        );
        self.finish(&renderer.queue, capture);
    }

    /// Record everything for one OBS frame except the scene copy: the
    /// privacy-filtered HUD over the stream target and the native transport.
    /// The caller encodes `StreamCapture::encode_scene_copy` where the scene
    /// is final and no local UI has been drawn, submits the capture's command
    /// buffers right after that encoder, then calls `finish`.
    pub(in crate::renderer) fn prepare(
        &mut self,
        renderer: &mut Renderer,
        source: &wgpu::Texture,
        now: Instant,
    ) -> Option<StreamCapture> {
        if !self.settings.enabled || self.warned_unsupported {
            return None;
        }
        let backend = renderer.device.adapter_info().backend;
        if !matches!(backend, wgpu::Backend::Dx12 | wgpu::Backend::Vulkan) {
            self.pause_unsupported("GPU-native Spout2 requires DX12 or Vulkan on Windows");
            return None;
        }
        let format = source.format();
        let view_format = renderer.config.format;
        if !matches!(
            format,
            wgpu::TextureFormat::Bgra8Unorm
                | wgpu::TextureFormat::Bgra8UnormSrgb
                | wgpu::TextureFormat::Rgba8Unorm
                | wgpu::TextureFormat::Rgba8UnormSrgb
        ) || !source.usage().contains(wgpu::TextureUsages::COPY_SRC)
            || format.remove_srgb_suffix() != view_format.remove_srgb_suffix()
            || source.sample_count() != 1
        {
            self.pause_unsupported(&format!(
                "unsupported source {:?}/{:?}; no unmasked fallback",
                source.format(),
                source.usage()
            ));
            return None;
        }
        let width = renderer.config.width;
        let height = renderer.config.height;
        if width == 0 || height == 0 || source.width() != width || source.height() != height {
            self.pause_unsupported("scene size differs from stream output size");
            return None;
        }
        self.ensure_resources(&renderer.device, width, height, format, view_format);
        #[cfg(windows)]
        if backend == wgpu::Backend::Dx12 && self.dx12_sender.is_none() {
            let result = {
                let resources = self.resources.as_ref()?;
                dx12_spout::Dx12Sender::new(&renderer.device, &resources.color, format, STREAM_NAME)
            };
            match result {
                Ok(sender) => {
                    self.dx12_sender = Some(sender);
                    eprintln!(
                        "[STREAMER] Spout2 GPU-native DX12 source '{}' online",
                        STREAM_NAME
                    );
                }
                Err(error) => {
                    self.pause_unsupported(&error);
                    return None;
                }
            }
        }
        #[cfg(windows)]
        if backend == wgpu::Backend::Dx12
            && !self
                .dx12_sender
                .as_ref()
                .is_some_and(dx12_spout::Dx12Sender::ready)
        {
            return None; // no blocking wait for a reusable DX12 command allocator
        }
        #[cfg(windows)]
        let vulkan_slot = if backend == wgpu::Backend::Vulkan {
            if self.vulkan_sender.is_none() {
                match vulkan_spout::VulkanSender::new(
                    &renderer.device,
                    width,
                    height,
                    format,
                    STREAM_NAME,
                ) {
                    Ok(sender) => {
                        self.vulkan_sender = Some(sender);
                        eprintln!(
                            "[STREAMER] Spout2 GPU-native Vulkan source '{}' online",
                            STREAM_NAME
                        );
                    }
                    Err(error) => {
                        self.pause_unsupported(&error);
                        return None;
                    }
                }
            }
            // `capture_due` already polled this frame; a new sender is empty.
            Some(self.vulkan_sender.as_ref()?.free_slot()?)
        } else {
            None
        };
        #[cfg(not(windows))]
        {
            self.pause_unsupported("Spout2 GPU transport is Windows only");
            return None;
        }
        let resources = self.resources.as_ref()?;

        let mut snapshot = renderer.ui_state.clone();
        crate::streamer::strip_local_only(&mut snapshot);
        redact_stream_snapshot(&mut snapshot, self.settings);
        let mut vertices = Vec::<UiVertex>::new();
        if !snapshot.video.skip_ui {
            // Same native HUD composition logic as local presentation, but all
            // text has been sanitized *before* vertex generation. Never reuse
            // the local unfiltered chat/console vertices.
            if renderer.vignette_enabled && !renderer.frame_plan.use_post {
                crate::renderer::append_vignette_vertices(&mut vertices);
            }
            ui::build_dynamic_static_vertices(&mut vertices, &snapshot, width, height);
            ui::build_movement_vertices(&mut vertices, &snapshot, width, height);
            ui::build_transient_vertices(
                &mut vertices,
                &snapshot,
                renderer.ui_small_font_metrics.as_ref(),
                width,
                height,
            );
            // This tail contains only projected player labels, the dynamic
            // crosshair and combat indicators. Chat/console vertices belong
            // to the stable prefix, rebuilt from the redacted snapshot above.
            vertices.extend_from_slice(
                &renderer.ui_transient_vertices[renderer.ui_transient_stable_vertex_count..],
            );
            vertices.extend(ui::build_vertices(
                &snapshot,
                width,
                height,
                renderer.ui_small_font_metrics.as_ref(),
                renderer.ui_splash_size,
            ));
        }
        let max = (UI_BUFFER_BYTES + UI_DYNAMIC_BUFFER_BYTES + UI_TRANSIENT_BUFFER_BYTES) as usize
            / std::mem::size_of::<UiVertex>();
        if vertices.len() > max {
            vertices.truncate(max - max % 3);
        }
        // Authored HUD shader quads are deliberately absent from the normal UI
        // atlas. Resolve the *filtered* stream's shader spans independently;
        // never copy unfiltered local shader batches or their vertex offsets.
        let (shader_vertices, shader_batches) =
            renderer.prepare_stream_ui_icon_shaders(&snapshot, &vertices);
        if !vertices.is_empty() {
            renderer.queue.write_buffer(
                &resources.ui_buffer, 0, bytemuck::cast_slice(&vertices));
        }
        if !shader_vertices.is_empty() {
            renderer.queue.write_buffer(
                &resources.shader_buffer, 0, bytemuck::cast_slice(&shader_vertices));
        }
        // The stream can have a different set of authored stages than the
        // local HUD. Include its blends in lazy encoded-pipeline readiness.
        let encoded = renderer.hdr_output.is_none() && renderer.config.format.is_srgb()
            && renderer.ensure_encoded_2d_pipelines_with_extra(&shader_batches);
        // Recorded now, submitted after the encoder holding the scene copy:
        // queue order, not recording order, sequences the two.
        let mut encoder = renderer
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("JKA streamer filtered 2D + transport state"),
            });
        if!vertices.is_empty() || renderer.screen_fx_vertex_count != 0 || renderer.enemy_icon_vertex_count != 0 {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("JKA streamer filtered 2D only"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: if encoded { &resources.encoded_view } else { &resources.view },
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
            let ui_gamma = if renderer.ui_gamma_active() {
                renderer.ui_gamma_native.as_ref()
            } else { None };
            let encoded_gamma = if encoded { ui_gamma } else { None };
            let encoded_gamma_group = encoded_gamma.map(|gamma| &gamma.encoded_group);
            let screen_fx_pipelines = if let Some(gamma) = encoded_gamma {
                &gamma.encoded_fx_pipelines
            } else if encoded {
                &renderer.screen_fx_encoded_pipelines
            } else {
                &renderer.screen_fx_pipelines
            };
            Renderer::draw_screen_fx_batches(
                &mut pass,
                &renderer.screen_fx_vertex_buffer,
                renderer.screen_fx_vertex_count,
                &renderer.screen_fx_batches,
                screen_fx_pipelines,
                &renderer.screen_fx_textures,
                &renderer.screen_fx_white_bind_group, encoded, encoded_gamma_group,
            );
            Renderer::draw_screen_fx_batches(
                &mut pass, &renderer.enemy_icon_vertex_buffer,
                renderer.enemy_icon_vertex_count, &renderer.enemy_icon_batches,
                screen_fx_pipelines, &renderer.screen_fx_textures,
                &renderer.screen_fx_white_bind_group, encoded, encoded_gamma_group,
            );
            if !vertices.is_empty() {
                // Match local HUD encoded blending and hardware-scene gamma.
                let pipeline = if let Some(gamma) = encoded_gamma {
                    gamma.encoded_pipeline.as_ref().expect("ready streamer gamma 2D pipeline")
                } else if encoded {
                    renderer.ui_encoded_pipeline.as_ref().expect("ready streamer legacy 2D pipeline")
                } else {
                    ui_gamma.map_or(&renderer.ui_pipeline, |gamma| &gamma.pipeline)
                };
                let ui_group = if encoded { &renderer.ui_encoded_bind_group }
                    else { &renderer.ui_bind_group };
                let gamma_group = encoded_gamma_group.or_else(|| ui_gamma.map(|gamma| &gamma.group));
                if let Some(group) = gamma_group {
                    pass.set_bind_group(1, group, &[]);
                }
                // All filtered UI was flattened into layer 0; the other
                // layers are empty. The shared draw routine preserves shader
                // stage/painter order and still uses its fast path otherwise.
                Renderer::draw_ui_batches_shadered(
                    &mut pass, pipeline, ui_group, gamma_group,
                    &resources.ui_buffer, vertices.len() as u32,
                    &resources.ui_buffer, 0,
                    &resources.ui_buffer, 0,
                    &resources.shader_buffer, &shader_batches,
                    screen_fx_pipelines, &renderer.screen_fx_textures,
                    &renderer.screen_fx_white_bind_group, encoded,
                    if encoded {
                        renderer.ui_encoded_format.expect("ready streamer encoded UI pipeline")
                    } else {
                        renderer.config.format
                    },
                    renderer.hud_shader_gpu.as_ref(),
                );
            }
        }
        // Vulkan copies the finished composition into its external-memory
        // ring. DX12 keeps the existing D3D11On12 RENDER_TARGET state contract.
        let mut transport_encoder: Option<wgpu::CommandEncoder> = None;
        #[cfg(windows)]
        if let Some(slot) = vulkan_slot {
            encoder.transition_resources(
                std::iter::empty(),
                std::iter::once(wgpu::TextureTransition {
                    texture: &resources.color,
                    selector: None,
                    state: wgpu::TextureUses::COPY_SRC,
                }),
            );
            // WGPU 30 separates its encoder API from native command recording.
            // Submit the native copy immediately after the composition buffer.
            let mut native =
                renderer
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("JKA Vulkan Spout external ownership and GPU copy"),
                    });
            if let Err(error) = self.vulkan_sender.as_ref().unwrap().encode_copy(
                &mut native,
                &resources.color,
                slot,
            ) {
                self.pause_unsupported(&error);
                return None;
            }
            transport_encoder = Some(native);
        }
        if backend == wgpu::Backend::Dx12 {
            encoder.transition_resources(
                std::iter::empty(),
                std::iter::once(wgpu::TextureTransition {
                    texture: &resources.color,
                    selector: None,
                    state: wgpu::TextureUses::COLOR_TARGET,
                }),
            );
        }
        let mut command_buffers = Vec::with_capacity(2);
        command_buffers.push(encoder.finish());
        command_buffers.extend(transport_encoder.map(wgpu::CommandEncoder::finish));
        Some(StreamCapture {
            source: source.clone(),
            color: resources.color.clone(),
            command_buffers,
            #[cfg(windows)]
            vulkan_slot,
            started: now,
        })
    }

    /// Publish after the queue submission that carried `capture`'s buffers.
    /// Dropping an unsubmitted capture instead is safe: no slot was claimed.
    pub(in crate::renderer) fn finish(&mut self, queue: &wgpu::Queue, capture: StreamCapture) {
        #[cfg(windows)]
        if let Some(slot) = capture.vulkan_slot {
            if let Some(sender) = self.vulkan_sender.as_mut() {
                sender.queued(queue, slot);
                self.last_frame = Some(capture.started);
            }
        }
        #[cfg(not(windows))]
        let _ = queue;
        #[cfg(windows)]
        if let Some(sender) = self.dx12_sender.as_mut() {
            match sender.publish() {
                Ok(true) => self.last_frame = Some(capture.started),
                Ok(false) => {} // send frame dropped; keep game responsive
                Err(error) => self.pause_unsupported(&error),
            }
        }
    }
}

/// One OBS frame. Only the scene copy has to sit between post and the local
/// HUD; the filtered HUD and native transport are separate command buffers
/// that must follow the copy's encoder in the same (or a later) submission.
pub(in crate::renderer) struct StreamCapture {
    source: wgpu::Texture,
    color: wgpu::Texture,
    command_buffers: Vec<wgpu::CommandBuffer>,
    #[cfg(windows)]
    vulkan_slot: Option<usize>,
    started: Instant,
}

impl StreamCapture {
    /// Encode where the scene is final and no local UI/console/egui is drawn.
    pub(in crate::renderer) fn encode_scene_copy(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.copy_texture_to_texture(
            self.source.as_image_copy(),
            self.color.as_image_copy(),
            self.color.size(),
        );
    }

    pub(in crate::renderer) fn take_command_buffers(&mut self) -> Vec<wgpu::CommandBuffer> {
        std::mem::take(&mut self.command_buffers)
    }
}
