//! Companion.
use crate::renderer::{
    aabb_intersects_clip_frustum, active_batch_selection, batch_area_visible, camera_depth_clear,
    draw_world_batch, effective_area_mask, mpsc, thread, Arc, AtomicU64, CameraUniform,
    CompanionId, CompanionSceneSlot, CompanionSceneTarget, CompanionSceneView, DrawClass, Duration,
    EguiRenderData, EventLoopProxy, Instant, JoinHandle, Mutex, Ordering, PhysicalSize, Receiver,
    Renderer, UserEvent, VecDeque, Window, COMPANION_SCENE_FRAME_INTERVAL,
    COMPANION_SCENE_RING_SIZE, DEPTH_FORMAT,
};
use bytemuck::Zeroable;
use wgpu::util::DeviceExt;

#[derive(Clone, Copy, Debug)]
pub(in crate::renderer) struct CompanionSceneFrame {
    pub(in crate::renderer) id: CompanionId,
    pub(in crate::renderer) generation: u64,
    pub(in crate::renderer) slot: usize,
    pub(in crate::renderer) sequence: u64,
}

pub(in crate::renderer) struct CompanionSceneSourceSet {
    pub(in crate::renderer) generation: u64,
    pub(in crate::renderer) hdr: bool,
    pub(in crate::renderer) views: Vec<wgpu::TextureView>,
}

#[derive(Default)]
pub(in crate::renderer) struct CompanionPending {
    pub(in crate::renderer) resize: Option<PhysicalSize<u32>>,
    /// Paint meshes are latest-wins, but egui texture deltas are a stateful
    /// stream. If an unpublished frame is superseded we must still replay its
    /// texture allocations/partial updates/frees before drawing the newest
    /// meshes, otherwise a later partial update can target a texture the GPU
    /// renderer never saw allocated.
    pub(in crate::renderer) skipped_texture_deltas: VecDeque<egui::TexturesDelta>,
    pub(in crate::renderer) frame: Option<EguiRenderData>,
    /// New offscreen ring after resize/format rebuild. The worker owns only
    /// sampling bind groups; all 3D rendering stays on the primary render thread.
    pub(in crate::renderer) scene_sources: Option<CompanionSceneSourceSet>,
    /// Latest completed secondary scene. Just like the UI meshes this is a
    /// latest-wins mailbox; superseded sequence numbers are acknowledged so the
    /// primary renderer can immediately reuse their ring slots.
    pub(in crate::renderer) scene_frame: Option<CompanionSceneFrame>,
    pub(in crate::renderer) shutdown: bool,
}

/// Auxiliary presenter. It shares the primary WGPU device/queue but owns its
/// Surface and egui renderer on a separate sleeping OS thread. 3D companion
/// views are rendered by the primary renderer into a non-blocking texture ring;
/// this worker only blits the newest completed texture and presents it, so a
/// slow compositor can never insert a surface-acquire wait in the 1000+ FPS path.
pub(in crate::renderer) struct CompanionRenderThread {
    pub(in crate::renderer) pending: Arc<Mutex<CompanionPending>>,
    pub(in crate::renderer) wake_tx: mpsc::SyncSender<()>,
    pub(in crate::renderer) join: Option<JoinHandle<()>>,
    pub(in crate::renderer) size: Arc<Mutex<PhysicalSize<u32>>>,
    pub(in crate::renderer) scene_consumed: Arc<AtomicU64>,
}

impl CompanionRenderThread {
    pub(in crate::renderer) fn spawn(
        id: CompanionId,
        window: Arc<Window>,
        initial_size: PhysicalSize<u32>,
        surface: wgpu::Surface<'static>,
        adapter: wgpu::Adapter,
        device: wgpu::Device,
        queue: wgpu::Queue,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Result<Self, String> {
        let pending = Arc::new(Mutex::new(CompanionPending::default()));
        let worker_pending = Arc::clone(&pending);
        let size = Arc::new(Mutex::new(initial_size));
        let worker_size = Arc::clone(&size);
        let scene_consumed = Arc::new(AtomicU64::new(0));
        let worker_scene_consumed = Arc::clone(&scene_consumed);
        let (wake_tx, wake_rx) = mpsc::sync_channel(1);
        let join = thread::Builder::new()
            .name(format!("jka-companion-{}", id.0))
            .spawn(move || {
                companion_render_thread_main(
                    id,
                    window,
                    initial_size,
                    surface,
                    adapter,
                    device,
                    queue,
                    proxy,
                    worker_pending,
                    worker_size,
                    worker_scene_consumed,
                    wake_rx,
                );
            })
            .map_err(|error| format!("could not start companion render worker: {error}"))?;
        Ok(Self {
            pending,
            wake_tx,
            join: Some(join),
            size,
            scene_consumed,
        })
    }

    pub(in crate::renderer) fn wake(&self) {
        let _ = self.wake_tx.try_send(());
    }

    pub(in crate::renderer) fn size(&self) -> PhysicalSize<u32> {
        match self.size.lock() {
            Ok(size) => *size,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    pub(in crate::renderer) fn scene_consumed(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.scene_consumed)
    }

    pub(in crate::renderer) fn resize(&self, size: PhysicalSize<u32>) {
        match self.size.lock() {
            Ok(mut current) => *current = size,
            Err(poisoned) => *poisoned.into_inner() = size,
        }
        match self.pending.lock() {
            Ok(mut pending) => pending.resize = Some(size),
            Err(poisoned) => poisoned.into_inner().resize = Some(size),
        }
        self.wake();
    }

    pub(in crate::renderer) fn publish(&self, frame: EguiRenderData) {
        let publish = |pending: &mut CompanionPending, frame: EguiRenderData| {
            if let Some(previous) = pending.frame.replace(frame) {
                // Dropping the old paint jobs is safe; dropping its texture
                // delta is not. egui may emit a partial ImageDelta in the new
                // frame that depends on an allocation from this old frame.
                if !previous.textures_delta.set.is_empty()
                    || !previous.textures_delta.free.is_empty()
                {
                    pending
                        .skipped_texture_deltas
                        .push_back(previous.textures_delta);
                }
            }
        };
        match self.pending.lock() {
            Ok(mut pending) => publish(&mut pending, frame),
            Err(poisoned) => publish(&mut poisoned.into_inner(), frame),
        }
        self.wake();
    }

    pub(in crate::renderer) fn set_scene_sources(&self, sources: CompanionSceneSourceSet) {
        let replace = |pending: &mut CompanionPending| {
            // A frame referring to the old ring can never be presented after
            // the bind groups change. Acknowledge it now instead of pinning a
            // retired slot until some later scene arrives.
            if let Some(old) = pending.scene_frame.take() {
                self.scene_consumed
                    .fetch_max(old.sequence, Ordering::Release);
            }
            pending.scene_sources = Some(sources);
        };
        match self.pending.lock() {
            Ok(mut pending) => replace(&mut pending),
            Err(poisoned) => replace(&mut poisoned.into_inner()),
        }
        self.wake();
    }

    pub(in crate::renderer) fn publish_scene(&self, frame: CompanionSceneFrame) {
        let publish = |pending: &mut CompanionPending| {
            if let Some(old) = pending.scene_frame.replace(frame) {
                self.scene_consumed
                    .fetch_max(old.sequence, Ordering::Release);
            }
        };
        match self.pending.lock() {
            Ok(mut pending) => publish(&mut pending),
            Err(poisoned) => publish(&mut poisoned.into_inner()),
        }
        self.wake();
    }

    pub(in crate::renderer) fn request_shutdown(&self) {
        match self.pending.lock() {
            Ok(mut pending) => pending.shutdown = true,
            Err(poisoned) => poisoned.into_inner().shutdown = true,
        }
        self.wake();
    }

    pub(in crate::renderer) fn shutdown(&mut self) {
        self.request_shutdown();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for CompanionRenderThread {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub(in crate::renderer) const COMPANION_BLIT_WGSL: &str = r#"
@group(0) @binding(0) var scene_texture: texture_2d<f32>;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    var out: VertexOut;
    out.position = vec4<f32>(positions[vertex_index], 0.0, 1.0);
    return out;
}

fn scene_load(position: vec4<f32>) -> vec3<f32> {
    let dimensions = vec2<i32>(textureDimensions(scene_texture));
    let pixel = clamp(vec2<i32>(position.xy), vec2<i32>(0), dimensions - vec2<i32>(1));
    return textureLoad(scene_texture, pixel, 0).rgb;
}

// Same fitted ACES curve used by post.wgsl. The companion keeps its own tiny
// presentation shader so HDR scene targets do not need to run the full main
// post stack a second time.
fn aces_fitted(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((x * (a * x + vec3<f32>(b))) / (x * (c * x + vec3<f32>(d)) + vec3<f32>(e)), vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fs_ldr(in: VertexOut) -> @location(0) vec4<f32> {
    return vec4<f32>(scene_load(in.position), 1.0);
}

@fragment
fn fs_hdr(in: VertexOut) -> @location(0) vec4<f32> {
    return vec4<f32>(aces_fitted(scene_load(in.position)), 1.0);
}
"#;

pub(in crate::renderer) fn create_companion_blit_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    fragment_entry: &'static str,
    label: &'static str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment_entry),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

#[allow(clippy::too_many_arguments)]
pub(in crate::renderer) fn companion_render_thread_main(
    id: CompanionId,
    _window: Arc<Window>,
    initial_size: PhysicalSize<u32>,
    surface: wgpu::Surface<'static>,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    proxy: EventLoopProxy<UserEvent>,
    pending: Arc<Mutex<CompanionPending>>,
    shared_size: Arc<Mutex<PhysicalSize<u32>>>,
    scene_consumed: Arc<AtomicU64>,
    wake_rx: Receiver<()>,
) {
    let caps = surface.get_capabilities(&adapter);
    let size = initial_size;
    let Some(mut config) =
        surface.get_default_config(&adapter, size.width.max(1), size.height.max(1))
    else {
        let _ = proxy.send_event(UserEvent::CompanionError {
            id: id.0,
            error: "Companion surface has no compatible configuration".into(),
        });
        return;
    };
    config.format = caps
        .formats
        .iter()
        .copied()
        .find(wgpu::TextureFormat::is_srgb)
        .unwrap_or(config.format);
    if caps.present_modes.contains(&wgpu::PresentMode::Fifo) {
        config.present_mode = wgpu::PresentMode::Fifo;
    }
    config.desired_maximum_frame_latency = 2;
    surface.configure(&device, &config);
    let mut egui_renderer = egui_wgpu::Renderer::new(
        &device,
        config.format,
        egui_wgpu::RendererOptions::default(),
    );

    let scene_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("DinurdoJK companion scene blit layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    });
    let scene_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("DinurdoJK companion scene blit pipeline layout"),
        bind_group_layouts: &[Some(&scene_layout)],
        immediate_size: 0,
    });
    let scene_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("DinurdoJK companion scene blit shader"),
        source: wgpu::ShaderSource::Wgsl(COMPANION_BLIT_WGSL.into()),
    });
    let scene_pipeline_ldr = create_companion_blit_pipeline(
        &device,
        &scene_pipeline_layout,
        &scene_shader,
        config.format,
        "fs_ldr",
        "DinurdoJK companion scene blit LDR",
    );
    let scene_pipeline_hdr = create_companion_blit_pipeline(
        &device,
        &scene_pipeline_layout,
        &scene_shader,
        config.format,
        "fs_hdr",
        "DinurdoJK companion scene blit HDR",
    );
    let mut scene_generation = 0u64;
    let mut scene_hdr = false;
    let mut scene_bind_groups = Vec::<wgpu::BindGroup>::new();
    let mut last_scene_slot = None::<usize>;

    let _ = proxy.send_event(UserEvent::CompanionReady { id: id.0 });

    while wake_rx.recv().is_ok() {
        let (resize, scene_sources, scene_frame, skipped_texture_deltas, frame, shutdown) = {
            let mut state = match pending.lock() {
                Ok(state) => state,
                Err(poisoned) => poisoned.into_inner(),
            };
            (
                state.resize.take(),
                state.scene_sources.take(),
                state.scene_frame.take(),
                std::mem::take(&mut state.skipped_texture_deltas),
                state.frame.take(),
                state.shutdown,
            )
        };
        if shutdown {
            return;
        }
        if let Some(size) = resize {
            if size.width > 0 && size.height > 0 {
                config.width = size.width;
                config.height = size.height;
                surface.configure(&device, &config);
            }
            match shared_size.lock() {
                Ok(mut current) => *current = size,
                Err(poisoned) => *poisoned.into_inner() = size,
            }
        }
        if let Some(sources) = scene_sources {
            scene_generation = sources.generation;
            scene_hdr = sources.hdr;
            scene_bind_groups = sources
                .views
                .iter()
                .enumerate()
                .map(|(slot, view)| {
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some(&format!("DinurdoJK companion scene slot {slot}")),
                        layout: &scene_layout,
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(view),
                        }],
                    })
                })
                .collect();
            last_scene_slot = None;
        }

        let mut scene_to_ack = None::<u64>;
        if let Some(scene_frame) = scene_frame {
            scene_to_ack = Some(scene_frame.sequence);
            if scene_frame.generation == scene_generation
                && scene_frame.slot < scene_bind_groups.len()
            {
                last_scene_slot = Some(scene_frame.slot);
            }
        }

        // Every superseded UI frame still contributes texture state. Replay
        // those deltas in frame order while intentionally skipping stale paint.
        for textures_delta in skipped_texture_deltas {
            for (texture_id, image_delta) in &textures_delta.set {
                egui_renderer.update_texture(&device, &queue, *texture_id, image_delta);
            }
            for texture_id in textures_delta.free {
                egui_renderer.free_texture(&texture_id);
            }
        }
        if let Some(frame) = frame.as_ref() {
            for (texture_id, image_delta) in &frame.textures_delta.set {
                egui_renderer.update_texture(&device, &queue, *texture_id, image_delta);
            }
        }

        if config.width == 0 || config.height == 0 {
            // Texture deltas above may have queued native staging uploads even
            // though a zero-sized surface cannot present a frame.
            queue.submit([]);
            if let Some(sequence) = scene_to_ack {
                scene_consumed.fetch_max(sequence, Ordering::Release);
            }
            if let Some(frame) = frame {
                for texture_id in frame.textures_delta.free {
                    egui_renderer.free_texture(&texture_id);
                }
            }
            continue;
        }

        let has_scene = last_scene_slot.is_some();
        if frame.is_none() && !has_scene {
            if let Some(sequence) = scene_to_ack {
                scene_consumed.fetch_max(sequence, Ordering::Release);
            }
            continue;
        }

        let output = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(output)
            | wgpu::CurrentSurfaceTexture::Suboptimal(output) => output,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                // Queue::write_buffer/write_texture use native staging allocations
                // that are released only after a queue submission completes. We may
                // already have applied egui texture deltas above, so make forward
                // progress even though this surface frame cannot be presented.
                queue.submit([]);
                if let Some(frame) = frame {
                    for texture_id in frame.textures_delta.free {
                        egui_renderer.free_texture(&texture_id);
                    }
                }
                if let Some(sequence) = scene_to_ack {
                    scene_consumed.fetch_max(sequence, Ordering::Release);
                }
                surface.configure(&device, &config);
                continue;
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                // Do not let staging allocations accumulate while the companion
                // window is minimized/occluded and no normal frame is submitted.
                queue.submit([]);
                if let Some(frame) = frame {
                    for texture_id in frame.textures_delta.free {
                        egui_renderer.free_texture(&texture_id);
                    }
                }
                if let Some(sequence) = scene_to_ack {
                    scene_consumed.fetch_max(sequence, Ordering::Release);
                }
                continue;
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                if let Some(frame) = frame {
                    for texture_id in frame.textures_delta.free {
                        egui_renderer.free_texture(&texture_id);
                    }
                }
                if let Some(sequence) = scene_to_ack {
                    scene_consumed.fetch_max(sequence, Ordering::Release);
                }
                let _ = proxy.send_event(UserEvent::CompanionError {
                    id: id.0,
                    error: "Companion surface validation failed".into(),
                });
                continue;
            }
        };
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("DinurdoJK companion presentation encoder"),
        });

        if let Some(slot) = last_scene_slot.filter(|slot| *slot < scene_bind_groups.len()) {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("DinurdoJK companion scene blit pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
            pass.set_pipeline(if scene_hdr {
                &scene_pipeline_hdr
            } else {
                &scene_pipeline_ldr
            });
            pass.set_bind_group(0, &scene_bind_groups[slot], &[]);
            pass.draw(0..3, 0..1);
        }

        let mut callback_buffers = Vec::new();
        if let Some(frame) = frame.as_ref() {
            let screen = egui_wgpu::ScreenDescriptor {
                size_in_pixels: [config.width.max(1), config.height.max(1)],
                pixels_per_point: frame.pixels_per_point.max(0.25),
            };
            callback_buffers = egui_renderer.update_buffers(
                &device,
                &queue,
                &mut encoder,
                &frame.paint_jobs,
                &screen,
            );
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("DinurdoJK companion egui pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if has_scene {
                            wgpu::LoadOp::Load
                        } else {
                            wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.025,
                                g: 0.028,
                                b: 0.034,
                                a: 1.0,
                            })
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            egui_renderer.render(&mut pass, &frame.paint_jobs, &screen);
        }

        queue.submit(
            callback_buffers
                .into_iter()
                .chain(std::iter::once(encoder.finish())),
        );
        if let Some(sequence) = scene_to_ack {
            // The read is now ordered on the shared queue. A later primary
            // submission may safely recycle this (and every older) slot.
            scene_consumed.fetch_max(sequence, Ordering::Release);
        }
        output.present();
        if let Some(frame) = frame {
            for texture_id in frame.textures_delta.free {
                egui_renderer.free_texture(&texture_id);
            }
        }
    }
}

impl Renderer {
    pub(in crate::renderer) fn remove_companion_scene_target(&mut self, id: CompanionId) {
        self.companion_scene_targets.remove(&id);
    }

    /// Lazily allocate a 3-slot scene ring matching the companion's physical
    /// size. The target is never a swapchain image, so encoding this view cannot
    /// block on the desktop compositor. The worker receives only sampleable views.
    pub(in crate::renderer) fn ensure_companion_scene_target(
        &mut self,
        id: CompanionId,
        size: PhysicalSize<u32>,
        consumed_sequence: Arc<AtomicU64>,
    ) -> Option<CompanionSceneSourceSet> {
        let width = size.width;
        let height = size.height;
        if width == 0 || height == 0 {
            // A minimized companion must not keep spending 3D work at its old
            // dimensions. Recreate lazily when Winit reports a drawable size.
            self.companion_scene_targets.remove(&id);
            return None;
        }
        let format = self.gameplay_scene_format();
        let samples = self.msaa_samples;
        if self.companion_scene_targets.get(&id).is_some_and(|target| {
            target.width == width
                && target.height == height
                && target.format == format
                && target.samples == samples
        }) {
            return None;
        }

        self.companion_scene_generation = self.companion_scene_generation.wrapping_add(1).max(1);
        let generation = self.companion_scene_generation;
        let camera_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("JKA companion camera uniform"),
                contents: bytemuck::bytes_of(&CameraUniform::zeroed()),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let camera_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("JKA companion camera bind group"),
            layout: &self.camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let fast_camera_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("JKA companion fast camera bind group"),
            layout: &self.fast_camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        let mut slots = Vec::with_capacity(COMPANION_SCENE_RING_SIZE);
        let mut source_views = Vec::with_capacity(COMPANION_SCENE_RING_SIZE);
        for slot in 0..COMPANION_SCENE_RING_SIZE {
            let color = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(&format!("JKA companion scene color {slot}")),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
            // Give the presentation worker its own view handle. This avoids
            // relying on TextureView cloning while both views still reference
            // the same immutable texture allocation.
            let source_view = color.create_view(&wgpu::TextureViewDescriptor::default());
            source_views.push(source_view);
            slots.push(CompanionSceneSlot {
                _color: color,
                color_view,
                sequence: 0,
            });
        }

        // Only the resolved color must survive until the presentation worker
        // samples it. MSAA/depth are primary-thread scratch attachments, so one
        // shared pair is sufficient for every ring slot and avoids multiplying
        // their VRAM cost by COMPANION_SCENE_RING_SIZE.
        let (msaa, msaa_view) = if samples > 1 {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("JKA companion scene shared MSAA"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            (Some(texture), Some(view))
        } else {
            (None, None)
        };
        let depth = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("JKA companion scene shared depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());

        let sources = CompanionSceneSourceSet {
            generation,
            hdr: format == wgpu::TextureFormat::Rgba16Float,
            views: source_views,
        };
        self.companion_scene_targets.insert(
            id,
            CompanionSceneTarget {
                width,
                height,
                format,
                samples,
                generation,
                camera_buffer,
                camera_bind_group,
                fast_camera_bind_group,
                _msaa: msaa,
                msaa_view,
                _depth: depth,
                depth_view,
                slots,
                last_render_at: Instant::now() - Duration::from_secs(1),
                consumed_sequence,
            },
        );
        Some(sources)
    }

    /// Encode a conservative secondary spectator view into a free ring slot.
    /// Visibility discovery is intentionally *not* run from this camera: the
    /// active batch list is the primary spectator's already-authorized PVS/area
    /// selection. The second view only adds its own frustum rejection inside it.
    pub(in crate::renderer) fn encode_companion_scene(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        request: &CompanionSceneView,
        area_mask: Option<&[u8; 32]>,
    ) -> Option<CompanionSceneFrame> {
        let pvs_mode = self.pvs_mode;
        let clear_color = self.weather.fog.clear_color();
        let static_bsp_ao = self.static_bsp_ao_enabled;
        let classic_flags = self.classic_world_render_flags();
        let time = self.started.elapsed().as_secs_f32();

        // A mapped world requires its known-fast PSO set. If it is still being
        // compiled in the background, skip this secondary frame rather than
        // stalling or drawing a misleading worldless POV.
        if self
            .world
            .as_ref()
            .is_some_and(|world| world.fast_pipelines.is_empty())
        {
            return None;
        }

        let target = self.companion_scene_targets.get_mut(&request.id)?;
        if target.last_render_at.elapsed() < COMPANION_SCENE_FRAME_INTERVAL {
            return None;
        }
        let consumed = target.consumed_sequence.load(Ordering::Acquire);
        let slot_index = target
            .slots
            .iter()
            .position(|slot| slot.sequence == 0 || slot.sequence <= consumed)?;
        let view_proj = request.camera.view_projection(target.width, target.height);
        let uniform = CameraUniform {
            view_proj: view_proj.to_cols_array_2d(),
            camera_pos_time: [
                request.camera.position.x,
                request.camera.position.y,
                request.camera.position.z,
                time,
            ],
            clip_plane: [0.0; 4],
            render_flags: [0, u32::from(static_bsp_ao), classic_flags, 0],
            camera_forward: request.camera.forward().extend(0.0).to_array(),
            unjittered_view_proj: view_proj.to_cols_array_2d(),
            previous_unjittered_view_proj: view_proj.to_cols_array_2d(),
            jump_shade: [0.0; 4],
        };
        self.queue
            .write_buffer(&target.camera_buffer, 0, bytemuck::bytes_of(&uniform));

        let slot = &target.slots[slot_index];
        let (color_view, resolve_target) = if let Some(msaa_view) = target.msaa_view.as_ref() {
            (msaa_view, Some(&slot.color_view))
        } else {
            (&slot.color_view, None)
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("JKA companion secondary spectator scene"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color_view,
                depth_slice: None,
                resolve_target,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear_color),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &target.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(camera_depth_clear()),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        if let Some(world) = self.world.as_ref() {
            let active = active_batch_selection(world, pvs_mode);
            let effective_area_mask = effective_area_mask(world, pvs_mode, area_mask);
            pass.set_bind_group(0, &target.fast_camera_bind_group, &[]);
            pass.set_vertex_buffer(0, world.vertex_buffer.slice(..));
            pass.set_vertex_buffer(1, world.legacy_dlight_surface_id_buffer.slice(..));
            pass.set_index_buffer(world.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            let mut current_pipeline = None;
            for &batch_index in active.indices {
                let batch = &active.batches[batch_index];
                if !batch_area_visible(batch, effective_area_mask) {
                    continue;
                }
                if batch.source.pipeline.class != DrawClass::Sky
                    && !aabb_intersects_clip_frustum(batch.bounds_min, batch.bounds_max, view_proj)
                {
                    continue;
                }
                if current_pipeline != Some(batch.source.pipeline) {
                    let Some(pipeline) = world.fast_pipelines.get(&batch.source.pipeline) else {
                        continue;
                    };
                    pass.set_pipeline(pipeline);
                    current_pipeline = Some(batch.source.pipeline);
                }
                pass.set_bind_group(1, &batch.fast_bind_group, &[]);
                // Draw authored water geometry here even if the main view promotes
                // it to the FFT ocean. The secondary view deliberately stays on
                // the cheap shared fast-world path and must not leave water holes.
                draw_world_batch(&mut pass, batch, 0..1, None);
            }
        }

        self.dynamic_model_renderer.draw_excluding_entity(
            &mut pass,
            &target.camera_bind_group,
            request.target_entity,
        );
        drop(pass);

        let sequence = self.companion_scene_next_sequence;
        self.companion_scene_next_sequence =
            self.companion_scene_next_sequence.wrapping_add(1).max(1);
        let target = self.companion_scene_targets.get_mut(&request.id)?;
        target.slots[slot_index].sequence = sequence;
        target.last_render_at = Instant::now();
        Some(CompanionSceneFrame {
            id: request.id,
            generation: target.generation,
            slot: slot_index,
            sequence,
        })
    }

    pub(in crate::renderer) fn take_companion_scene_ready(&mut self) -> Vec<CompanionSceneFrame> {
        std::mem::take(&mut self.companion_scene_ready)
    }
}
