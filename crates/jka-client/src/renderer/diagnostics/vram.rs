//! Process dedicated-VRAM usage against the OS budget, read from DXGI so it
//! covers Vulkan and DX12 alike (WDDM accounts both). Unlike the allocator
//! report it includes driver and swapchain memory, which is what runs out.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct VramBudget {
    pub usage: u64,
    pub budget: u64,
    /// System memory this process has made GPU-visible (DXGI non-local group).
    pub shared_usage: u64,
}

#[cfg(windows)]
pub(in crate::renderer) fn query(info: &wgpu::AdapterInfo) -> Option<VramBudget> {
    use windows::core::Interface;
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, IDXGIAdapter3, IDXGIFactory1, DXGI_MEMORY_SEGMENT_GROUP_LOCAL,
        DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO,
    };
    // SAFETY: plain DXGI queries on locally owned COM interfaces.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        for index in 0.. {
            let adapter = factory.EnumAdapters1(index).ok()?;
            let desc = adapter.GetDesc1().ok()?;
            if desc.VendorId != info.vendor || desc.DeviceId != info.device {
                continue;
            }
            let adapter: IDXGIAdapter3 = adapter.cast().ok()?;
            let mut memory = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
            adapter
                .QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut memory)
                .ok()?;
            let mut shared = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
            let _ = adapter.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL, &mut shared);
            return (memory.Budget > 0).then_some(VramBudget {
                usage: memory.CurrentUsage,
                budget: memory.Budget,
                shared_usage: shared.CurrentUsage,
            });
        }
        None
    }
}

#[cfg(not(windows))]
pub(in crate::renderer) fn query(_info: &wgpu::AdapterInfo) -> Option<VramBudget> {
    None
}
