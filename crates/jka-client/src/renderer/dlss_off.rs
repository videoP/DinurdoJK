//! `dlss.rs` API for builds without the `nvidia-dlss` feature: ordinary WGPU
//! instance/device creation and a DLAA state that never runs.

pub(super) fn detect_dx12_support(_adapter: &wgpu::Adapter, _device: &wgpu::Device) {}

pub(super) fn device_supported() -> bool {
    false
}

pub(super) fn create_instance(desc: wgpu::InstanceDescriptor) -> wgpu::Instance {
    wgpu::Instance::new(desc)
}

pub(super) fn try_request_device(
    _adapter: &wgpu::Adapter,
    _descriptor: &wgpu::DeviceDescriptor<'_>,
) -> Option<(wgpu::Device, wgpu::Queue)> {
    None
}

#[allow(dead_code)] // Read only by the nvidia-dlss implementation.
pub(super) struct DlssFrame<'a> {
    pub input: &'a wgpu::TextureView,
    pub output: &'a wgpu::TextureView,
    pub depth: &'a wgpu::TextureView,
    pub motion_vectors: &'a wgpu::TextureView,
    pub render_size: [u32; 2],
    pub output_size: [u32; 2],
    pub scene_format: wgpu::TextureFormat,
    pub mode: crate::ui::DlssMode,
    pub model: crate::ui::DlssModel,
    pub jitter: [f32; 2],
    pub reset: bool,
}

#[derive(Default)]
pub(super) struct DlssState;

impl DlssState {
    pub(super) fn release(&mut self) {}

    pub(super) fn evaluate(
        &mut self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _adapter: &wgpu::Adapter,
        _encoder: &mut wgpu::CommandEncoder,
        _frame: DlssFrame<'_>,
    ) -> Result<[wgpu::CommandBuffer; 2], String> {
        Err("this build lacks the nvidia-dlss feature".into())
    }
}
