//! Opt-in NVIDIA NGX DLSS (DLAA and upscaling) for WGPU 30 Vulkan/DX12
//! (cargo feature `nvidia-dlss`). Ordinary builds compile `dlss_off.rs`,
//! which exposes the same API as no-ops. Keep NGX calls, Vulkan extension
//! registration and the DLSS-only targets isolated here.
use dlss_wgpu::super_resolution::{
    DlssSuperResolution, DlssSuperResolutionExposure, DlssSuperResolutionRenderParameters,
};
use dlss_wgpu::{DlssFeatureFlags, DlssPerfQualityMode, DlssRenderPreset, DlssSdk, FeatureSupport};
use crate::ui::{DlssMode, DlssModel};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

// Stable identifier for this client, not an NVIDIA-issued production ID.
// Register a real NGX project ID before distributing builds with NGX enabled.
fn project_id() -> uuid::Uuid {
    uuid::Uuid::from_u128(0xd1a0_3d5a_9d7a_40d8_ba33_496e_d10a_2600)
}

static INSTANCE_SUPPORTS_NGX: AtomicBool = AtomicBool::new(false);
static DEVICE_SUPPORTS_NGX: AtomicBool = AtomicBool::new(false);

/// True when the active device can attempt NGX Super Resolution.
/// Vulkan needs extensions at device creation; DX12 checks NGX at SDK init.
pub(super) fn device_supported() -> bool {
    DEVICE_SUPPORTS_NGX.load(Ordering::Relaxed)
}

/// DX12 has no NGX device/instance extensions to register. Only mark a
/// vendor-matching DX12 device as eligible; `DlssSdk::new` performs the real
/// driver capability test, and fails normally on unsupported hardware.
pub(super) fn detect_dx12_support(adapter: &wgpu::Adapter, _device: &wgpu::Device) {
    if adapter.get_info().backend == wgpu::Backend::Dx12 {
        DEVICE_SUPPORTS_NGX.store(adapter.get_info().vendor == 0x10de, Ordering::Relaxed);
    }
}

/// `dlss_wgpu` registers Vulkan instance extensions *before* WGPU creates Vulkan.
pub(super) fn create_instance(desc: wgpu::InstanceDescriptor) -> wgpu::Instance {
    INSTANCE_SUPPORTS_NGX.store(false, Ordering::Relaxed);
    DEVICE_SUPPORTS_NGX.store(false, Ordering::Relaxed);
    let mut support = FeatureSupport::default();
    match dlss_wgpu::create_instance(project_id(), &desc, &mut support) {
        Ok(instance) => {
            INSTANCE_SUPPORTS_NGX.store(support.super_resolution_supported, Ordering::Relaxed);
            println!(
                "[DLSS] NGX Vulkan instance extensions available: {}",
                support.super_resolution_supported
            );
            instance
        }
        Err(error) => {
            eprintln!("[DLSS] NGX Vulkan instance setup unavailable: {error}; normal Vulkan instance");
            wgpu::Instance::new(desc)
        }
    }
}

/// NGX extension-aware device creation. `None` means the caller should use the
/// ordinary `request_device` (non-NVIDIA, non-Vulkan, or registration failed).
pub(super) fn try_request_device(
    adapter: &wgpu::Adapter,
    descriptor: &wgpu::DeviceDescriptor<'_>,
) -> Option<(wgpu::Device, wgpu::Queue)> {
    DEVICE_SUPPORTS_NGX.store(false, Ordering::Relaxed);
    let info = adapter.get_info();
    if info.backend != wgpu::Backend::Vulkan
        || info.vendor != 0x10de
        || !INSTANCE_SUPPORTS_NGX.load(Ordering::Relaxed)
    {
        return None;
    }
    let mut support = FeatureSupport::default();
    match dlss_wgpu::request_device(
        project_id(),
        adapter,
        descriptor,
        &mut support,
        Some(descriptor.required_limits.clone()),
    ) {
        Ok(pair) => {
            DEVICE_SUPPORTS_NGX.store(support.super_resolution_supported, Ordering::Relaxed);
            println!(
                "[DLSS] NGX Super Resolution device support: {}",
                support.super_resolution_supported
            );
            Some(pair)
        }
        Err(error) => {
            eprintln!("[DLSS] NGX Vulkan device extensions unavailable: {error}; normal device");
            None
        }
    }
}

/// Per-frame DLSS inputs. All views are single-sample.
pub(super) struct DlssFrame<'a> {
    /// Render-size world color (the window-size scene itself for DLAA).
    pub input: &'a wgpu::TextureView,
    /// Window-size scene color that post reads; receives the DLSS result.
    pub output: &'a wgpu::TextureView,
    /// Render-size reversed-Z depth and motion vectors.
    pub depth: &'a wgpu::TextureView,
    pub motion_vectors: &'a wgpu::TextureView,
    pub render_size: [u32; 2],
    pub output_size: [u32; 2],
    pub scene_format: wgpu::TextureFormat,
    pub mode: DlssMode,
    pub model: DlssModel,
    /// Render-pixel offset the jittered projection applied (+x right, +y down).
    pub jitter: [f32; 2],
    pub reset: bool,
}

/// The NGX SDK lives for the renderer; the feature context and its targets
/// only while DLSS runs at the current sizes, format, mode and model.
#[derive(Default)]
pub(super) struct DlssState {
    sdk: Option<Arc<Mutex<DlssSdk>>>,
    resources: Option<DlssResources>,
}

impl DlssState {
    /// Drop the NGX history and DLSS targets. Called when DLSS turns off and
    /// whenever the scene targets are recreated (the copy bind groups hold
    /// the old scene views).
    pub(super) fn release(&mut self) {
        self.resources = None;
    }

    /// Encode input -> NGX -> output. On success `encoder` has been replaced
    /// by a fresh encoder holding the restore pass, and the returned buffers
    /// (pre-DLSS work, then NGX) must be submitted, in order, immediately
    /// before it in the same `queue.submit`.
    pub(super) fn evaluate(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        adapter: &wgpu::Adapter,
        encoder: &mut wgpu::CommandEncoder,
        frame: DlssFrame<'_>,
    ) -> Result<[wgpu::CommandBuffer; 2], String> {
        let key = DlssKey {
            render_size: frame.render_size,
            output_size: frame.output_size,
            format: frame.scene_format,
            mode: frame.mode,
            model: frame.model,
        };
        if self.resources.as_ref().is_some_and(|r| r.key != key) {
            self.resources = None;
        }
        if self.resources.is_none() {
            let sdk = match &self.sdk {
                Some(sdk) => Arc::clone(sdk),
                None => {
                    if !device_supported() {
                        return Err("NGX Super Resolution is unavailable on this device".into());
                    }
                    let sdk = DlssSdk::new(project_id(), device.clone())
                        .map_err(|e| format!("NGX init failed: {e}"))?;
                    self.sdk = Some(Arc::clone(&sdk));
                    sdk
                }
            };
            self.resources = Some(DlssResources::new(device, queue, sdk, frame.input, key)?);
        }
        let resources = self.resources.as_mut().expect("DLSS resources created above");
        resources.copy(encoder, CopyPass::InputToNgx);
        let [render_width, render_height] = resources.key.render_size;
        let ngx = resources
            .context
            .render(
                DlssSuperResolutionRenderParameters {
                    color: &resources.input_view,
                    depth: frame.depth,
                    motion_vectors: frame.motion_vectors,
                    exposure: DlssSuperResolutionExposure::Automatic,
                    bias: None,
                    dlss_output: &resources.output_view,
                    reset: frame.reset,
                    jitter_offset: frame.jitter,
                    partial_texture_size: Some(resources.key.render_size),
                    // The prepass stores Bevy-convention motion: current minus
                    // previous, in UV units. NGX wants render pixels pointing
                    // back to the previous frame, hence the negated size (as
                    // Bevy does).
                    motion_vector_scale: Some([-(render_width as f32), -(render_height as f32)]),
                },
                encoder,
                adapter,
            )
            .map_err(|e| format!("NGX evaluation failed: {e}"))?;
        let pre = std::mem::replace(
            encoder,
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("JKA after NVIDIA DLSS"),
            }),
        )
        .finish();
        resources.copy(encoder, CopyPass::NgxToOutput(frame.output));
        Ok([pre, ngx])
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct DlssKey {
    render_size: [u32; 2],
    output_size: [u32; 2],
    format: wgpu::TextureFormat,
    mode: DlssMode,
    model: DlssModel,
}

fn ngx_mode(mode: DlssMode) -> DlssPerfQualityMode {
    match mode {
        DlssMode::UltraPerformance => DlssPerfQualityMode::UltraPerformance,
        DlssMode::Performance => DlssPerfQualityMode::Performance,
        DlssMode::Balanced => DlssPerfQualityMode::Balanced,
        DlssMode::Quality => DlssPerfQualityMode::Quality,
        DlssMode::Dlaa => DlssPerfQualityMode::Dlaa,
    }
}

fn ngx_preset(model: DlssModel, mode: DlssMode) -> DlssRenderPreset {
    match model.preset(mode) {
        'E' => DlssRenderPreset::E,
        'F' => DlssRenderPreset::F,
        'J' => DlssRenderPreset::J,
        'K' => DlssRenderPreset::K,
        'L' => DlssRenderPreset::L,
        'M' => DlssRenderPreset::M,
        _ => DlssRenderPreset::Default,
    }
}

enum CopyPass<'a> {
    InputToNgx,
    NgxToOutput(&'a wgpu::TextureView),
}

/// NGX reads a stable RGBA16F copy of the render-size input and writes a
/// window-size RGBA16F storage texture; the restore pass copies that into the
/// scene view so the existing post bind groups keep sampling it.
struct DlssResources {
    key: DlssKey,
    // Declared before `sdk`: the feature must be released before the SDK.
    context: DlssSuperResolution,
    _sdk: Arc<Mutex<DlssSdk>>,
    input_view: wgpu::TextureView,
    output_view: wgpu::TextureView,
    to_input_pipeline: wgpu::RenderPipeline,
    to_input_bind_group: wgpu::BindGroup,
    to_output_pipeline: wgpu::RenderPipeline,
    to_output_bind_group: wgpu::BindGroup,
}

impl DlssResources {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sdk: Arc<Mutex<DlssSdk>>,
        input: &wgpu::TextureView,
        key: DlssKey,
    ) -> Result<Self, String> {
        let preset = ngx_preset(key.model, key.mode);
        sdk.lock()
            .map_err(|_| "NGX SDK lock poisoned".to_owned())?
            .set_render_preset(ngx_mode(key.mode), preset);
        let hdr_flag = if key.format == wgpu::TextureFormat::Rgba16Float {
            DlssFeatureFlags::HighDynamicRange
        } else {
            DlssFeatureFlags::empty()
        };
        let context = DlssSuperResolution::new(
            key.output_size,
            ngx_mode(key.mode),
            hdr_flag
                | DlssFeatureFlags::LowResolutionMotionVectors
                | DlssFeatureFlags::InvertedDepth
                | DlssFeatureFlags::AutoExposure,
            Arc::clone(&sdk),
            device,
            queue,
        )
        .map_err(|e| format!("NGX feature creation failed: {e}"))?;
        // The renderer picks the render size from NVIDIA's standard ratios;
        // NGX must accept it for this mode.
        let range = context.render_resolution_range();
        let (min, max) = (*range.start(), *range.end());
        let [render_width, render_height] = key.render_size;
        if render_width < min[0] || render_height < min[1] || render_width > max[0] || render_height > max[1] {
            return Err(format!(
                "NGX rejects render size {render_width}x{render_height} for {:?} (allowed {}x{} to {}x{})",
                key.mode, min[0], min[1], max[0], max[1]
            ));
        }
        let texture = |label, [width, height]: [u32; 2], usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage,
                view_formats: &[],
            })
        };
        let input_view = texture(
            "JKA DLSS input",
            key.render_size,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        )
        .create_view(&wgpu::TextureViewDescriptor::default());
        let output_view = texture(
            "JKA DLSS output",
            key.output_size,
            wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        )
        .create_view(&wgpu::TextureViewDescriptor::default());
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA DLSS copy source layout"),
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
        let bind_group = |label, view| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                }],
            })
        };
        let to_input_bind_group = bind_group("JKA DLSS input source", input);
        let to_output_bind_group = bind_group("JKA DLSS result source", &output_view);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("JKA DLSS copy layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("JKA DLSS copy shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("dlss_copy.wgsl").into()),
        });
        let pipeline = |label, target| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: crate::pipeline_cache::get().as_ref(),
            })
        };
        let to_input_pipeline = pipeline("JKA DLSS input copy", wgpu::TextureFormat::Rgba16Float);
        let to_output_pipeline = pipeline("JKA DLSS result copy", key.format);
        println!(
            "[DLSS] NGX initialized: {:?}, {render_width}x{render_height} -> {}x{} ({:?}), {:?} preset {preset:?}",
            key.mode, key.output_size[0], key.output_size[1], key.format, key.model
        );
        Ok(Self {
            key,
            context,
            _sdk: sdk,
            input_view,
            output_view,
            to_input_pipeline,
            to_input_bind_group,
            to_output_pipeline,
            to_output_bind_group,
        })
    }

    fn copy(&self, encoder: &mut wgpu::CommandEncoder, copy: CopyPass<'_>) {
        let (label, destination, pipeline, group) = match copy {
            CopyPass::InputToNgx => (
                "JKA DLSS input copy",
                &self.input_view,
                &self.to_input_pipeline,
                &self.to_input_bind_group,
            ),
            CopyPass::NgxToOutput(output) => (
                "JKA DLSS result copy",
                output,
                &self.to_output_pipeline,
                &self.to_output_bind_group,
            ),
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: destination,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.draw(0..3, 0..1);
    }
}
