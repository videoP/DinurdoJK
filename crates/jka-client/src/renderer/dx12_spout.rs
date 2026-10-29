//! DX12-only, GPU-native Spout2 transport for the independently masked stream target.
//!
//! All Spout/D3D11On12 calls run on the render thread, on WGPU's *same* D3D12
//! device and queue. We never copy a frame through system RAM. Spout itself may
//! copy between GPU textures to present the cross-process shared handle.
//!
//! Important: D3D11On12 releases its wrapped resource to COMMON/PRESENT (0).
//! WGPU still tracks it as COLOR_TARGET. After every send we therefore enqueue
//! COMMON -> RENDER_TARGET on that same queue. We rotate three command allocators
//! guarded by a fence so resetting one never races its GPU execution.
use core::ffi::c_void;
use windows::core::Interface;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Graphics::Direct3D12::{
    ID3D12CommandAllocator, ID3D12CommandList, ID3D12CommandQueue, ID3D12Fence,
    ID3D12GraphicsCommandList, ID3D12Resource, D3D12_COMMAND_LIST_TYPE_DIRECT,
    D3D12_FENCE_FLAG_NONE, D3D12_RESOURCE_BARRIER, D3D12_RESOURCE_BARRIER_0,
    D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES, D3D12_RESOURCE_BARRIER_FLAG_NONE,
    D3D12_RESOURCE_BARRIER_TYPE_TRANSITION, D3D12_RESOURCE_STATE_COMMON,
    D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_TRANSITION_BARRIER,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};

const RESTORE_SLOTS: usize = 3;

struct RestoreSlot {
    allocator: ID3D12CommandAllocator,
    command_list: ID3D12GraphicsCommandList,
    fence_value: u64,
}

/// Renderer values can move from a Studio initialization worker before output
/// starts. Once created, native Spout state must stay on its owning render thread.
pub(super) struct Dx12Sender {
    owner: std::thread::ThreadId,
    native: std::mem::ManuallyDrop<NativeSender>,
}

// SAFETY: every native access and destruction checks the creating thread.
// ManuallyDrop prevents unwinding on a wrong-thread drop from invoking native
// destructors there. No Sync implementation or unchecked access is exposed.
unsafe impl Send for Dx12Sender {}

impl Dx12Sender {
    pub(super) fn new(
        device: &wgpu::Device,
        texture: &wgpu::Texture,
        format: wgpu::TextureFormat,
        sender_name: &str,
    ) -> Result<Self, String> {
        Ok(Self {
            owner: std::thread::current().id(),
            native: std::mem::ManuallyDrop::new(NativeSender::new(
                device,
                texture,
                format,
                sender_name,
            )?),
        })
    }

    fn check_owner(&self) {
        assert_eq!(
            self.owner,
            std::thread::current().id(),
            "Spout sender left its owning render thread"
        );
    }

    pub(super) fn ready(&self) -> bool {
        self.check_owner();
        self.native.ready()
    }

    pub(super) fn publish(&mut self) -> Result<bool, String> {
        self.check_owner();
        self.native.publish()
    }
}

impl Drop for Dx12Sender {
    fn drop(&mut self) {
        self.check_owner();
        // SAFETY: owner checked above; this is the only native destructor call.
        unsafe {
            std::mem::ManuallyDrop::drop(&mut self.native);
        }
    }
}

struct NativeSender {
    // Drop in declaration order: wrapped D3D11 resource BEFORE Spout sender.
    wrapped: spout2::dx12::WrappedResource,
    sender: spout2::dx12::Sender,
    queue: ID3D12CommandQueue,
    fence: ID3D12Fence,
    resource: ID3D12Resource,
    slots: Vec<RestoreSlot>,
    next_fence: u64,
    // Spout's with_device expects ID3D12CommandQueue**. Keep the pointer cell
    // at a stable address until Spout sender is destroyed.
    _queue_ptr: Box<*mut c_void>,
}

impl NativeSender {
    pub(super) fn new(
        device: &wgpu::Device,
        texture: &wgpu::Texture,
        format: wgpu::TextureFormat,
        sender_name: &str,
    ) -> Result<Self, String> {
        // SAFETY: The HAL guards own live references to WGPU's native resources;
        // native COM objects are cloned before each guard is released. The DX12
        // backend check makes non-DX12 devices fail closed (no CPU fallback).
        let (raw_device, queue) = {
            let hal = unsafe { device.as_hal::<wgpu::hal::api::Dx12>() }
                .ok_or("Spout GPU sharing requires WGPU's DX12 backend")?;
            (hal.raw_device().clone(), hal.raw_queue().clone())
        };
        let resource = {
            let hal = unsafe { texture.as_hal::<wgpu::hal::api::Dx12>() }
                .ok_or("private stream texture is not DX12")?;
            unsafe { hal.raw_resource() }.clone()
        };
        let mut queue_ptr = Box::new(queue.as_raw());
        // SAFETY: spout2-sys retains its own COM reference to raw_device.
        // The queue and its pointer cell remain alive until Sender is dropped.
        let mut sender = unsafe {
            spout2::dx12::Sender::with_device(sender_name, raw_device.as_raw(), &mut *queue_ptr)
        }
        .map_err(|error| format!("Spout2 D3D11On12 initialization: {error}"))?;
        let dxgi_format = match format.remove_srgb_suffix() {
            wgpu::TextureFormat::Bgra8Unorm => spout2::dx12::format::B8G8R8A8_UNORM,
            wgpu::TextureFormat::Rgba8Unorm => spout2::dx12::format::R8G8B8A8_UNORM,
            _ => return Err("Spout2 DX12 needs a four-channel SDR target".into()),
        };
        sender.set_format(dxgi_format);
        // The encoder always ends with COLOR_TARGET state before publication.
        let wrapped = unsafe {
            sender.wrap_resource(
                resource.as_raw(),
                spout2::dx12::resource_state::RENDER_TARGET,
            )
        }
        .map_err(|error| format!("Spout2 D3D12 resource wrap: {error}"))?;
        let fence: ID3D12Fence = unsafe { raw_device.CreateFence(0, D3D12_FENCE_FLAG_NONE) }
            .map_err(|error| format!("Spout2 restore fence: {error}"))?;
        let mut slots = Vec::with_capacity(RESTORE_SLOTS);
        for _ in 0..RESTORE_SLOTS {
            let allocator: ID3D12CommandAllocator =
                unsafe { raw_device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT) }
                    .map_err(|error| format!("Spout2 restore allocator: {error}"))?;
            let command_list: ID3D12GraphicsCommandList = unsafe {
                raw_device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)
            }
            .map_err(|error| format!("Spout2 restore list: {error}"))?;
            unsafe { command_list.Close() }
                .map_err(|error| format!("Spout2 restore list close: {error}"))?;
            slots.push(RestoreSlot {
                allocator,
                command_list,
                fence_value: 0,
            });
        }
        Ok(Self {
            wrapped,
            sender,
            queue,
            fence,
            resource,
            slots,
            next_fence: 1,
            _queue_ptr: queue_ptr,
        })
    }

    /// A no-wait readiness test: if the GPU hasn't retired a restore command
    /// list, drop this OBS frame rather than block the game's render thread.
    pub(super) fn ready(&self) -> bool {
        let completed = unsafe { self.fence.GetCompletedValue() };
        self.slots.iter().any(|slot| slot.fence_value <= completed)
    }

    /// Called after WGPU's private composition has been submitted. We rely on
    /// GPU FIFO ordering on the same ID3D12CommandQueue and never map pixels.
    pub(super) fn publish(&mut self) -> Result<bool, String> {
        let completed = unsafe { self.fence.GetCompletedValue() };
        let Some(slot) = self
            .slots
            .iter_mut()
            .find(|slot| slot.fence_value <= completed)
        else {
            return Ok(false);
        };
        // SAFETY: WGPU has transitioned the private texture to RENDER_TARGET,
        // and its composition command list was queued earlier on *this* queue.
        // Spout acquires the wrapped D3D11On12 resource and releases to COMMON.
        unsafe { self.sender.send_wrapped_resource(&self.wrapped) }
            .map_err(|error| format!("Spout2 GPU send: {error}"))?;
        // SAFETY: after the D3D11On12 release, the image is COMMON (0); restore
        // WGPU's tracked RENDER_TARGET state on the same queue for next frame.
        // A fence protects the command allocator from early reuse.
        unsafe {
            slot.allocator
                .Reset()
                .map_err(|error| format!("Spout2 restore allocator reset: {error}"))?;
            slot.command_list
                .Reset(&slot.allocator, None)
                .map_err(|error| format!("Spout2 restore list reset: {error}"))?;
            // D3D12_RESOURCE_BARRIER is a tagged union with a ManuallyDrop COM
            // pointer. The borrowed pointer is safe while self.resource is alive.
            let transition = D3D12_RESOURCE_BARRIER {
                Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
                Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
                Anonymous: D3D12_RESOURCE_BARRIER_0 {
                    Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                        pResource: std::mem::transmute_copy(&self.resource),
                        Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                        StateBefore: D3D12_RESOURCE_STATE_COMMON,
                        StateAfter: D3D12_RESOURCE_STATE_RENDER_TARGET,
                    }),
                },
            };
            slot.command_list.ResourceBarrier(&[transition]);
            slot.command_list
                .Close()
                .map_err(|error| format!("Spout2 restore list close: {error}"))?;
            let base: ID3D12CommandList = slot
                .command_list
                .cast()
                .map_err(|error| format!("Spout2 restore command list cast: {error}"))?;
            self.queue.ExecuteCommandLists(&[Some(base)]);
            let value = self.next_fence;
            self.queue
                .Signal(&self.fence, value)
                .map_err(|error| format!("Spout2 restore fence signal: {error}"))?;
            slot.fence_value = value;
            self.next_fence = self.next_fence.wrapping_add(1).max(1);
        }
        Ok(true)
    }
}

// Only the stream toggle, source resize, or teardown waits for outstanding
// restore command allocators. No fence wait occurs on the steady-state render
// path; if the GPU is behind, an OBS frame is discarded instead.
impl Drop for NativeSender {
    fn drop(&mut self) {
        // Cover even a failed send, which may already have flushed D3D11On12
        // work without reaching our regular restoration fence signal.
        let final_value = self.next_fence;
        if unsafe { self.queue.Signal(&self.fence, final_value) }.is_err() {
            // A removed device cannot execute further work or signal a fence.
            return;
        }
        // We must not free command allocators or the wrapped D3D12 resource
        // while the queue is still executing restoration barriers.
        unsafe {
            if let Ok(event) = CreateEventW(None, false, false, None) {
                if self.fence.SetEventOnCompletion(final_value, event).is_ok() {
                    WaitForSingleObject(event, INFINITE);
                } else {
                    while self.fence.GetCompletedValue() < final_value {
                        std::thread::yield_now();
                    }
                }
                let _ = CloseHandle(event);
            } else {
                // Rare event creation failure: poll the D3D12 fence directly
                // rather than invalidating command allocators that are in use.
                while self.fence.GetCompletedValue() < final_value {
                    std::thread::yield_now();
                }
            }
        }
    }
}
