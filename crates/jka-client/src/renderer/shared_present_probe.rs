//! One-shot read-only Vulkan shared presentable image capability query.
//!
//! Uses the *selected* WGPU Vulkan physical device and the *actual* game
//! window surface. No extra instance, no replacement swapchain, no GPU work.
//! Do not run these Vulkan API queries each frame: only the console cvar can
//! enqueue this command on the owning render thread.

#[cfg(target_os = "windows")]
mod windows_probe {
    use ash::{khr, vk};

    fn bool_word(value: bool) -> &'static str {
        if value { "yes" } else { "no" }
    }

    pub(super) fn run(device: &wgpu::Device, surface: &wgpu::Surface<'_>) -> Result<(), String> {
        // SAFETY: read-only query of WGPU HAL raw handles, on the game's render
        // thread while the device + actual window surface remain alive. We do
        // not submit, mutate, destroy, or retain the borrowed Vulkan handles.
        let hal = unsafe { device.as_hal::<wgpu::hal::api::Vulkan>() }
            .ok_or("Current WGPU backend is not Vulkan (use the Vulkan renderer)")?;
        let surface_hal = unsafe { surface.as_hal::<wgpu::hal::api::Vulkan>() }
            .ok_or("Current surface is not a WGPU Vulkan surface")?;
        let vk_surface = unsafe { surface_hal.raw_native_handle() }
            .ok_or("This WGPU surface is not a native Vulkan window surface")?;
        let physical = hal.raw_physical_device();
        let shared = hal.shared_instance();
        let instance = shared.raw_instance();
        let props = unsafe { instance.get_physical_device_properties(physical) };
        let gpu_name = unsafe { std::ffi::CStr::from_ptr(props.device_name.as_ptr()) }
            .to_string_lossy();
        println!("[SHARED PRESENT] Selected Vulkan GPU: {gpu_name}");

        let extensions = unsafe { instance.enumerate_device_extension_properties(physical) }
            .map_err(|err| format!("device extensions: {err:?}"))?;
        let advertised = extensions.iter().any(|ext| {
            ext.extension_name_as_c_str() == Ok(khr::shared_presentable_image::NAME)
        });
        let device_enabled = hal.enabled_device_extensions()
            .contains(&khr::shared_presentable_image::NAME);
        println!("[SHARED PRESENT] VK_KHR_shared_presentable_image: advertised={} enabled_on_WGPU_device={}",
            bool_word(advertised), bool_word(device_enabled));

        let surface_loader = khr::surface::Instance::new(shared.entry(), instance);
        let modes = unsafe {
            surface_loader.get_physical_device_surface_present_modes(physical, vk_surface)
        }.map_err(|err| format!("surface present modes: {err:?}"))?;
        let continuous = modes.contains(&vk::PresentModeKHR::SHARED_CONTINUOUS_REFRESH);
        let demand = modes.contains(&vk::PresentModeKHR::SHARED_DEMAND_REFRESH);
        println!("[SHARED PRESENT] Actual window surface: shared_continuous={} shared_demand={} modes={modes:?}",
            bool_word(continuous), bool_word(demand));

        // VK_KHR_get_surface_capabilities2 is *not* part of standard WGPU 30
        // instance creation. Invoking its entry point without enabling the
        // extension would be invalid, even when the driver supports it.
        let capabilities2_enabled = shared.extensions()
            .contains(&khr::get_surface_capabilities2::NAME);
        if advertised && (continuous || demand) && capabilities2_enabled {
            let loader = khr::get_surface_capabilities2::Instance::new(shared.entry(), instance);
            let surface_info = vk::PhysicalDeviceSurfaceInfo2KHR::default().surface(vk_surface);
            let mut shared_caps = vk::SharedPresentSurfaceCapabilitiesKHR::default();
            let mut caps2 = vk::SurfaceCapabilities2KHR::default().push_next(&mut shared_caps);
            unsafe {
                loader.get_physical_device_surface_capabilities2(physical, &surface_info, &mut caps2)
            }.map_err(|err| format!("shared surface capabilities: {err:?}"))?;
            let color = shared_caps.shared_present_supported_usage_flags
                .contains(vk::ImageUsageFlags::COLOR_ATTACHMENT);
            println!("[SHARED PRESENT] Shared image: COLOR_ATTACHMENT={} usage={:?}",
                bool_word(color), shared_caps.shared_present_supported_usage_flags);
            if !color {
                println!("[SHARED PRESENT] NO-GO: native shared image does not advertise color attachment usage");
                return Ok(());
            }
        } else if advertised && (continuous || demand) {
            println!("[SHARED PRESENT] Shared image usage: not queried (VK_KHR_get_surface_capabilities2 not enabled on WGPU instance; color attachment is required by Vulkan spec)");
        } else {
            println!("[SHARED PRESENT] Shared image usage: not applicable without a shared surface present mode");
        }

        if advertised && (continuous || demand) {
            if device_enabled {
                println!("[SHARED PRESENT] CANDIDATE: driver + surface support shared present; a separate backend implementation/swapchain is still required");
            } else {
                println!("[SHARED PRESENT] CANDIDATE, NOT CURRENTLY ENABLED: driver + surface support shared present, but WGPU device did not enable extension; alternate device/presentation setup required");
            }
        } else {
            println!("[SHARED PRESENT] NO-GO for VK_KHR_shared_presentable_image on this GPU + window surface; ordinary WGPU presentation unchanged");
        }
        Ok(())
    }
}

pub(super) fn run(device: &wgpu::Device, surface: &wgpu::Surface<'_>) {
    #[cfg(target_os = "windows")]
    match windows_probe::run(device, surface) {
        Ok(()) => (),
        Err(err) => println!("[SHARED PRESENT] Probe unavailable: {err}"),
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (device, surface);
        println!("[SHARED PRESENT] Probe is currently implemented for Windows Vulkan only");
    }
}
