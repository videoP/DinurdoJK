//! GPU-native Vulkan -> D3D11 KMT -> Spout2 bridge.
//!
//! Ported from leadedge/SpoutVulkan's LinkVulkanImage concept. The existing
//! masked WGPU composition is copied on the Vulkan GPU into a KMT-imported
//! D3D11 texture. Spout then copies that *GPU texture* into its shared sender.
//! No CPU pixel mapping, vkDeviceWaitIdle or waits in the per-frame path.
//!
//! IMPORTANT: This is a Windows-only, backend-specific unsafe bridge. The
//! COM/Vulkan operations must be validated on an actual Windows Vulkan GPU.
use ash::vk;
use std::ffi::c_void;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use windows::core::Interface;
use windows::core::BOOL;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Query, ID3D11Texture2D,
    D3D11_ASYNC_GETDATA_DONOTFLUSH, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_QUERY_DESC, D3D11_QUERY_EVENT,
    D3D11_RESOURCE_MISC_SHARED, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGIResource};

const SLOTS: usize = 3;
const HANDLE_TYPE: vk::ExternalMemoryHandleTypeFlags =
    vk::ExternalMemoryHandleTypeFlags::D3D11_TEXTURE_KMT;

#[derive(Clone)]
enum Stage {
    Free,
    VulkanPending(Arc<AtomicBool>),
    D3d11Pending,
}

struct ImportedImage {
    raw: ash::Device,
    image: vk::Image,
    memory: vk::DeviceMemory,
}
impl Drop for ImportedImage {
    fn drop(&mut self) {
        unsafe {
            self.raw.destroy_image(self.image, None);
            if self.memory != vk::DeviceMemory::null() {
                self.raw.free_memory(self.memory, None);
            }
        }
    }
}

// Like the DX12 sender, a live native sender belongs to its render thread.
// Studio initialization may move an empty Renderer before any sender exists.
pub(super) struct VulkanSender {
    owner: std::thread::ThreadId,
    native: std::mem::ManuallyDrop<NativeSender>,
}
// SAFETY: every access/destruction checks owner; ManuallyDrop prevents native
// destructors from running on the wrong thread during panic unwinding. No Sync.
unsafe impl Send for VulkanSender {}
impl VulkanSender {
    pub(super) fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        name: &str,
    ) -> Result<Self, String> {
        Ok(Self {
            owner: std::thread::current().id(),
            native: std::mem::ManuallyDrop::new(NativeSender::new(
                device, width, height, format, name,
            )?),
        })
    }
    fn check_owner(&self) {
        assert_eq!(
            self.owner,
            std::thread::current().id(),
            "Vulkan Spout sender left its render thread"
        );
    }
    pub(super) fn poll_ready(&mut self) -> Result<(), String> {
        self.check_owner();
        self.native.poll_ready()
    }
    pub(super) fn free_slot(&self) -> Option<usize> {
        self.check_owner();
        self.native.free_slot()
    }
    pub(super) fn encode_copy(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::Texture,
        index: usize,
    ) -> Result<(), String> {
        self.check_owner();
        self.native.encode_copy(encoder, source, index)
    }
    pub(super) fn queued(&mut self, queue: &wgpu::Queue, index: usize) {
        self.check_owner();
        self.native.queued(queue, index);
    }
}
impl Drop for VulkanSender {
    fn drop(&mut self) {
        self.check_owner();
        unsafe {
            std::mem::ManuallyDrop::drop(&mut self.native);
        }
    }
}

struct Slot {
    // Native Vulkan image and imported memory drop BEFORE their D3D11 owner.
    imported: ImportedImage,
    dx_texture: ID3D11Texture2D,
    event: ID3D11Query,
    stage: Stage,
    serial: u64,
}

struct NativeSender {
    // Drop the ring before the sender and its D3D11 device.
    slots: Vec<Slot>,
    sender: spout2::dx::Sender,
    context: ID3D11DeviceContext,
    _device: ID3D11Device,
    gpu_device: wgpu::Device,
    raw: ash::Device,
    queue_family: u32,
    next_serial: u64,
}

impl NativeSender {
    pub(super) fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        name: &str,
    ) -> Result<Self, String> {
        if !device
            .features()
            .contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32)
        {
            return Err("Vulkan adapter did not enable VULKAN_EXTERNAL_MEMORY_WIN32".into());
        }
        let (raw, physical, instance, vendor_luid, queue_family) = {
            let hal = unsafe { device.as_hal::<wgpu::hal::api::Vulkan>() }
                .ok_or("Spout2 Vulkan sender requires the WGPU Vulkan backend")?;
            let instance = hal.shared_instance().raw_instance().clone();
            let raw = hal.raw_device().clone();
            let physical = hal.raw_physical_device();
            let mut ids = vk::PhysicalDeviceIDProperties::default();
            let mut props = vk::PhysicalDeviceProperties2::default().push_next(&mut ids);
            unsafe {
                instance.get_physical_device_properties2(physical, &mut props);
            }
            if ids.device_luid_valid == 0 {
                return Err("Vulkan GPU has no Windows LUID for D3D11 adapter validation".into());
            }
            (
                raw,
                physical,
                instance,
                ids.device_luid,
                hal.queue_family_index(),
            )
        };

        // The HWND/monitor's default D3D11 GPU may differ on hybrid systems.
        // Never import cross-adapter handles; fail closed if LUIDs differ.
        let mut dx_device = None;
        let mut dx_context = None;
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                windows::Win32::Foundation::HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut dx_device),
                None,
                Some(&mut dx_context),
            )
        }
        .map_err(|e| format!("D3D11 device creation: {e}"))?;
        let dx_device = dx_device.ok_or("D3D11CreateDevice returned no device")?;
        let context = dx_context.ok_or("D3D11CreateDevice returned no immediate context")?;
        let dxgi: IDXGIDevice = dx_device
            .cast()
            .map_err(|e| format!("D3D11 DXGI interface: {e}"))?;
        let adapter = unsafe { dxgi.GetAdapter() }.map_err(|e| format!("D3D11 adapter: {e}"))?;
        let desc =
            unsafe { adapter.GetDesc() }.map_err(|e| format!("DXGI adapter description: {e}"))?;
        let mut dx_luid = [0u8; 8];
        dx_luid[..4].copy_from_slice(&desc.AdapterLuid.LowPart.to_le_bytes());
        dx_luid[4..].copy_from_slice(&desc.AdapterLuid.HighPart.to_le_bytes());
        if vendor_luid != dx_luid {
            return Err("D3D11 GPU differs from the selected Vulkan GPU (LUID mismatch)".into());
        }

        let (dx_format, vk_format, dx_spout_format) = match format.remove_srgb_suffix() {
            wgpu::TextureFormat::Bgra8Unorm => (
                DXGI_FORMAT_B8G8R8A8_UNORM,
                vk::Format::B8G8R8A8_UNORM,
                spout2::dx::format::B8G8R8A8_UNORM,
            ),
            wgpu::TextureFormat::Rgba8Unorm => (
                DXGI_FORMAT_R8G8B8A8_UNORM,
                vk::Format::R8G8B8A8_UNORM,
                spout2::dx::format::R8G8B8A8_UNORM,
            ),
            _ => return Err("SpoutVulkan supports only BGRA8/RGBA8 SDR".into()),
        };
        // The stream composition is copied into this shared image on the GPU.
        let image_usage = vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::COLOR_ATTACHMENT;
        let mut ext_query =
            vk::PhysicalDeviceExternalImageFormatInfo::default().handle_type(HANDLE_TYPE);
        let image_query = vk::PhysicalDeviceImageFormatInfo2::default()
            .format(vk_format)
            .ty(vk::ImageType::TYPE_2D)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(image_usage)
            .push_next(&mut ext_query);
        let mut ext_props = vk::ExternalImageFormatProperties::default();
        let mut image_props = vk::ImageFormatProperties2::default().push_next(&mut ext_props);
        unsafe {
            instance.get_physical_device_image_format_properties2(
                physical,
                &image_query,
                &mut image_props,
            )
        }
        .map_err(|e| format!("Vulkan KMT image format support: {e:?}"))?;
        let ext_flags = ext_props
            .external_memory_properties
            .external_memory_features;
        if !ext_flags.contains(vk::ExternalMemoryFeatureFlags::IMPORTABLE) {
            return Err("Vulkan driver cannot import D3D11 KMT shared texture memory".into());
        }

        // `with_device` owns only a pointer/reference to the D3D11 device.
        // Store the COM object in this sender for at least as long as Spout.
        let mut sender = unsafe { spout2::dx::Sender::with_device(name, dx_device.as_raw()) }
            .map_err(|e| format!("Spout2 D3D11 sender init: {e}"))?;
        sender.set_format(dx_spout_format);
        let mut slots = Vec::with_capacity(SLOTS);
        for _ in 0..SLOTS {
            let dx_desc = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: dx_format,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE).0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: D3D11_RESOURCE_MISC_SHARED.0 as u32,
            };
            let mut dx_texture = None;
            unsafe { dx_device.CreateTexture2D(&dx_desc, None, Some(&mut dx_texture)) }
                .map_err(|e| format!("D3D11 KMT shared texture: {e}"))?;
            let dx_texture = dx_texture.ok_or("D3D11 shared texture missing")?;
            let dx_resource: IDXGIResource = dx_texture
                .cast()
                .map_err(|e| format!("D3D11 shared resource interface: {e}"))?;
            let kmt_handle = unsafe { dx_resource.GetSharedHandle() }
                .map_err(|e| format!("D3D11 KMT shared handle: {e}"))?;
            // KMT handles are NOT NT handles: never CloseHandle() them.
            let imported = Self::import_image(
                device,
                &instance,
                &raw,
                physical,
                width,
                height,
                format.remove_srgb_suffix(),
                vk_format,
                kmt_handle.0,
                image_usage,
                ext_flags,
            )?;
            let mut event = None;
            unsafe {
                dx_device.CreateQuery(
                    &D3D11_QUERY_DESC {
                        Query: D3D11_QUERY_EVENT,
                        MiscFlags: 0,
                    },
                    Some(&mut event),
                )
            }
            .map_err(|e| format!("D3D11 stream completion query: {e}"))?;
            slots.push(Slot {
                imported,
                dx_texture,
                event: event.ok_or("D3D11 completion query missing")?,
                stage: Stage::Free,
                serial: 0,
            });
        }
        Ok(Self {
            slots,
            sender,
            context,
            _device: dx_device,
            gpu_device: device.clone(),
            raw,
            queue_family,
            next_serial: 0,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn import_image(
        _device: &wgpu::Device,
        instance: &ash::Instance,
        raw: &ash::Device,
        physical: vk::PhysicalDevice,
        width: u32,
        height: u32,
        _format: wgpu::TextureFormat,
        vk_format: vk::Format,
        shared_handle: *mut c_void,
        image_usage: vk::ImageUsageFlags,
        _flags: vk::ExternalMemoryFeatureFlags,
    ) -> Result<ImportedImage, String> {
        let mut external = vk::ExternalMemoryImageCreateInfo::default().handle_types(HANDLE_TYPE);
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk_format)
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(image_usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut external);
        let image = unsafe { raw.create_image(&image_info, None) }
            .map_err(|e| format!("Vulkan KMT imported image create: {e:?}"))?;
        // Own the native allocation directly. WGPU deferred destruction must
        // not outlive the D3D11 texture: KMT imports do not retain its lifetime.
        let mut imported_image = ImportedImage {
            raw: raw.clone(),
            image,
            memory: vk::DeviceMemory::null(),
        };
        let requirements = unsafe { raw.get_image_memory_requirements(image) };
        let mut handle_props = vk::MemoryWin32HandlePropertiesKHR::default();
        let win32 = ash::khr::external_memory_win32::Device::new(instance, raw);
        unsafe {
            win32.get_memory_win32_handle_properties(
                HANDLE_TYPE,
                shared_handle as isize,
                &mut handle_props,
            )
        }
        .map_err(|e| format!("Vulkan KMT memory properties: {e:?}"))?;
        let mem_props = unsafe { instance.get_physical_device_memory_properties(physical) };
        let mem_index = (0..mem_props.memory_type_count)
            .find(|i| {
                requirements.memory_type_bits & handle_props.memory_type_bits & (1 << i) != 0
                    && mem_props.memory_types[*i as usize]
                        .property_flags
                        .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
            })
            .ok_or("No compatible Vulkan device-local memory type for KMT image")?;
        let mut imported = vk::ImportMemoryWin32HandleInfoKHR::default()
            .handle_type(HANDLE_TYPE)
            .handle(shared_handle as isize);
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
        let allocation = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(mem_index)
            .push_next(&mut dedicated)
            .push_next(&mut imported);
        imported_image.memory = unsafe { raw.allocate_memory(&allocation, None) }
            .map_err(|e| format!("Vulkan D3D11 memory import: {e:?}"))?;
        unsafe { raw.bind_image_memory(image, imported_image.memory, 0) }
            .map_err(|e| format!("Vulkan imported image bind: {e:?}"))?;
        Ok(imported_image)
    }

    fn encode_copy(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::Texture,
        index: usize,
    ) -> Result<(), String> {
        let slot = &self.slots[index];
        assert!(matches!(slot.stage, Stage::Free));
        let source_image = {
            let hal = unsafe { source.as_hal::<wgpu::hal::api::Vulkan>() }
                .ok_or("stream source is not Vulkan")?;
            unsafe { hal.raw_handle() }
        };
        // Caller has transitioned source with WGPU to COPY_SRC. Keep imported
        // targets outside WGPU's tracker so its submission-prefix barriers
        // cannot race external ownership acquisition or overwrite our layout.
        unsafe {
            encoder.as_hal_mut::<wgpu::hal::api::Vulkan, _, _>(|hal| {
                let hal = hal.ok_or("stream encoder is not Vulkan")?;
                let cmd = hal.raw_handle();
                let range = vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(0)
                    .layer_count(1);
                let acquire = vk::ImageMemoryBarrier::default()
                    .image(slot.imported.image)
                    .subresource_range(range)
                    // Discard the previous D3D11 content: every texel is overwritten.
                    .old_layout(vk::ImageLayout::UNDEFINED)
                    .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_EXTERNAL)
                    .dst_queue_family_index(self.queue_family)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
                self.raw.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[acquire],
                );
                let layers = vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .layer_count(1);
                let copy = vk::ImageCopy::default()
                    .src_subresource(layers)
                    .dst_subresource(layers)
                    .extent(vk::Extent3D {
                        width: source.width(),
                        height: source.height(),
                        depth: 1,
                    });
                self.raw.cmd_copy_image(
                    cmd,
                    source_image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    slot.imported.image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[copy],
                );
                let release = vk::ImageMemoryBarrier::default()
                    .image(slot.imported.image)
                    .subresource_range(range)
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::GENERAL)
                    .src_queue_family_index(self.queue_family)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_EXTERNAL)
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE);
                self.raw.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[release],
                );
                Ok::<(), String>(())
            })
        }
    }

    fn poll_ready(&mut self) -> Result<(), String> {
        self.gpu_device
            .poll(wgpu::PollType::Poll)
            .map_err(|e| format!("Vulkan stream device poll: {e}"))?;
        for slot in &mut self.slots {
            if matches!(slot.stage, Stage::D3d11Pending) {
                let mut finished = BOOL(0);
                unsafe {
                    self.context.GetData(
                        &slot.event,
                        Some(&mut finished as *mut BOOL as *mut c_void),
                        std::mem::size_of::<BOOL>() as u32,
                        D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32,
                    )
                }
                .map_err(|e| format!("D3D11 GPU copy query: {e}"))?;
                if finished.as_bool() {
                    slot.stage = Stage::Free;
                }
            }
        }
        // Ring indices are reused; index order is not publication order.
        let mut pending: Vec<_> = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                matches!(s.stage, Stage::VulkanPending(_)).then_some((s.serial, i))
            })
            .collect();
        pending.sort_unstable();
        for (_, i) in pending {
            let slot = &mut self.slots[i];
            let Stage::VulkanPending(flag) = &slot.stage else {
                unreachable!()
            };
            if !flag.load(Ordering::Acquire) {
                break;
            }
            let sent = unsafe { self.sender.send_texture(slot.dx_texture.as_raw()) };
            // A failed send can already have issued GPU work. Always fence it
            // before propagating an error and destroying shared allocations.
            unsafe {
                self.context.End(&slot.event);
                self.context.Flush();
            }
            slot.stage = Stage::D3d11Pending;
            sent.map_err(|e| format!("Spout Vulkan D3D11 GPU send: {e}"))?;
        }
        Ok(())
    }

    pub(super) fn free_slot(&self) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| matches!(s.stage, Stage::Free))
    }

    pub(super) fn queued(&mut self, queue: &wgpu::Queue, index: usize) {
        let complete = Arc::new(AtomicBool::new(false));
        self.slots[index].serial = self.next_serial;
        self.next_serial = self.next_serial.wrapping_add(1);
        self.slots[index].stage = Stage::VulkanPending(complete.clone());
        queue.on_submitted_work_done(move || {
            complete.store(true, Ordering::Release);
        });
    }

    pub(super) fn shutdown(&mut self, device: &wgpu::Device) {
        // Destruction/resizing is a cold path. Wait here, never per frame.
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        // Retire pending D3D11 sends before COM/Vulkan textures are freed.
        // Avoid `vkDeviceWaitIdle` in upstream's sender CHECK loop.
        for slot in &self.slots {
            if matches!(slot.stage, Stage::D3d11Pending) {
                let mut ready = BOOL(0);
                loop {
                    let result = unsafe {
                        self.context.GetData(
                            &slot.event,
                            Some(&mut ready as *mut BOOL as *mut c_void),
                            std::mem::size_of::<BOOL>() as u32,
                            0,
                        )
                    };
                    if result.is_err() || ready.as_bool() {
                        break;
                    }
                    std::thread::yield_now();
                }
            }
        }
    }
}

impl Drop for NativeSender {
    fn drop(&mut self) {
        // A stream toggle/resize can happen between submission and the GPU
        // completion callback. Do not destroy imported memory while in use.
        let device = self.gpu_device.clone();
        self.shutdown(&device);
        self.slots.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use windows::Win32::Graphics::Direct3D11::{
        D3D11_CPU_ACCESS_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_USAGE_STAGING,
    };

    #[test]
    #[ignore = "requires a Windows Vulkan GPU with D3D11 KMT import"]
    fn vulkan_kmt_pixels_ring_reuse_and_teardown() {
        crate::renderer::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::VULKAN,
                flags: wgpu::InstanceFlags::VALIDATION | wgpu::InstanceFlags::DEBUG,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter = instance.request_adapter(&Default::default()).await.unwrap();
            assert!(adapter
                .features()
                .contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32));
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    required_features: wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32,
                    ..Default::default()
                })
                .await
                .unwrap();
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            // Recreate at different sizes; repeatedly reuse the ring allocation.
            for width in [16, 32] {
                let mut sender = VulkanSender::new(
                    &device,
                    width,
                    16,
                    wgpu::TextureFormat::Bgra8Unorm,
                    "DinurdoJK Vulkan Pixel Test",
                )
                .unwrap();
                let source = device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width,
                        height: 16,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Bgra8Unorm,
                    usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                });
                let view = source.create_view(&Default::default());
                for frame in 0..8 {
                    let index = sender.free_slot().unwrap();
                    let mut encoder = device.create_command_encoder(&Default::default());
                    let red = frame % 2 == 0;
                    drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color {
                                    r: if red { 1.0 } else { 0.0 },
                                    g: 0.0,
                                    b: if red { 0.0 } else { 1.0 },
                                    a: 1.0,
                                }),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        ..Default::default()
                    }));
                    encoder.transition_resources(
                        std::iter::empty(),
                        std::iter::once(wgpu::TextureTransition {
                            texture: &source,
                            selector: None,
                            state: wgpu::TextureUses::COPY_SRC,
                        }),
                    );
                    let mut native = device.create_command_encoder(&Default::default());
                    sender.encode_copy(&mut native, &source, index).unwrap();
                    queue.submit([encoder.finish(), native.finish()]);
                    sender.queued(&queue, index);
                    let deadline = Instant::now() + Duration::from_secs(5);
                    loop {
                        sender.poll_ready().unwrap();
                        if matches!(sender.native.slots[index].stage, Stage::Free) {
                            break;
                        }
                        assert!(
                            Instant::now() < deadline,
                            "Vulkan/D3D11 ring failed to retire"
                        );
                        std::thread::yield_now();
                    }
                    assert!(sender.native.sender.is_initialized());
                    // TEST ONLY: D3D11 staging readback verifies actual pixels
                    // across APIs. Runtime transport has no CPU pixel path.
                    let mut desc = D3D11_TEXTURE2D_DESC::default();
                    unsafe {
                        sender.native.slots[index].dx_texture.GetDesc(&mut desc);
                    }
                    desc.Usage = D3D11_USAGE_STAGING;
                    desc.BindFlags = 0;
                    desc.MiscFlags = 0;
                    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                    let mut staging = None;
                    unsafe {
                        sender
                            .native
                            ._device
                            .CreateTexture2D(&desc, None, Some(&mut staging))
                            .unwrap();
                    }
                    let staging = staging.unwrap();
                    let context = &sender.native.context;
                    unsafe {
                        context.CopyResource(&staging, &sender.native.slots[index].dx_texture);
                        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                        context
                            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                            .unwrap();
                        let pixel =
                            std::slice::from_raw_parts(mapped.pData as *const u8, 4).to_vec();
                        context.Unmap(&staging, 0);
                        assert_eq!(
                            pixel,
                            if red {
                                vec![0, 0, 255, 255]
                            } else {
                                vec![255, 0, 0, 255]
                            }
                        );
                    }
                }
                // Dropping immediately after queued Vulkan work is also valid.
                let mut encoder = device.create_command_encoder(&Default::default());
                let index = sender.free_slot().unwrap();
                sender.encode_copy(&mut encoder, &source, index).unwrap();
                queue.submit([encoder.finish()]);
                sender.queued(&queue, index);
                drop(sender);
            }
            assert!(scope.pop().await.is_none());
        });
    }
}
