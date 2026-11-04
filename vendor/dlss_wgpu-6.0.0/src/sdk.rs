use crate::{feature_info::with_feature_info, nvsdk_ngx::*};
use std::{
    ptr,
    sync::{Arc, Mutex},
    thread,
};
use uuid::Uuid;
use wgpu::{Device, hal::api::Vulkan};
#[cfg(windows)]
use wgpu::hal::api::Dx12;
#[cfg(windows)]
use windows::core::Interface;

/// Backend selected at runtime. NGX parameters and features cannot cross backends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NgxBackend {
    Vulkan,
    #[cfg(windows)]
    Dx12,
}

/// Application-wide DLSS object.
pub struct DlssSdk {
    pub(crate) parameters: *mut NVSDK_NGX_Parameter,
    pub(crate) device: Device,
    pub(crate) backend: NgxBackend,
}

impl DlssSdk {
    /// Creates the DLSS SDK.
    ///
    /// This should be done once per application.
    pub fn new(project_id: Uuid, device: Device) -> Result<Arc<Mutex<Self>>, DlssError> {
        check_for_updates(project_id);

        let mut parameters = ptr::null_mut();
        let backend = if let Some(hal_device) = unsafe { device.as_hal::<Vulkan>() } {
            // Vulkan retains its custom instance/device extension initialization.
            let shared_instance = hal_device.shared_instance();
            let raw_instance = shared_instance.raw_instance();
            with_feature_info(project_id, Default::default(), |feature_info| unsafe {
                check_ngx_result(NVSDK_NGX_VULKAN_Init_with_ProjectID(
                    feature_info.Identifier.v.ProjectDesc.ProjectId,
                    NVSDK_NGX_EngineType_NVSDK_NGX_ENGINE_TYPE_CUSTOM,
                    feature_info.Identifier.v.ProjectDesc.EngineVersion,
                    feature_info.ApplicationDataPath,
                    raw_instance.handle(),
                    hal_device.raw_physical_device(),
                    hal_device.raw_device().handle(),
                    shared_instance.entry().static_fn().get_instance_proc_addr,
                    raw_instance.fp_v1_0().get_device_proc_addr,
                    feature_info.FeatureInfo,
                    NVSDK_NGX_Version_NVSDK_NGX_Version_API,
                ))
            })?;
            unsafe { check_ngx_result(NVSDK_NGX_VULKAN_GetCapabilityParameters(&mut parameters))?; }
            NgxBackend::Vulkan
        } else {
            #[cfg(windows)]
            {
                let hal_device = unsafe { device.as_hal::<Dx12>() }
                    .ok_or(DlssError::FeatureNotSupported)?;
                with_feature_info(project_id, Default::default(), |feature_info| unsafe {
                    check_ngx_result(NVSDK_NGX_D3D12_Init_with_ProjectID(
                        feature_info.Identifier.v.ProjectDesc.ProjectId,
                        NVSDK_NGX_EngineType_NVSDK_NGX_ENGINE_TYPE_CUSTOM,
                        feature_info.Identifier.v.ProjectDesc.EngineVersion,
                        feature_info.ApplicationDataPath,
                        hal_device.raw_device().as_raw().cast(),
                        feature_info.FeatureInfo,
                        NVSDK_NGX_Version_NVSDK_NGX_Version_API,
                    ))
                })?;
                unsafe { check_ngx_result(NVSDK_NGX_D3D12_GetCapabilityParameters(&mut parameters))?; }
                NgxBackend::Dx12
            }
            #[cfg(not(windows))]
            { return Err(DlssError::FeatureNotSupported); }
        };
        let mut supported = 0;
        let result = unsafe {
            check_ngx_result(NVSDK_NGX_Parameter_GetI(
                parameters,
                NVSDK_NGX_Parameter_SuperSampling_Available.as_ptr().cast(),
                &mut supported,
            ))
        };
        if result.is_err() || supported == 0 {
            unsafe {
                match backend {
                    NgxBackend::Vulkan => {
                        let _ = NVSDK_NGX_VULKAN_DestroyParameters(parameters);
                        let hal = device.as_hal::<Vulkan>().unwrap();
                        let _ = NVSDK_NGX_VULKAN_Shutdown1(hal.raw_device().handle());
                    }
                    #[cfg(windows)]
                    NgxBackend::Dx12 => {
                        let _ = NVSDK_NGX_D3D12_DestroyParameters(parameters);
                        let hal = device.as_hal::<Dx12>().unwrap();
                        let _ = NVSDK_NGX_D3D12_Shutdown1(hal.raw_device().as_raw().cast());
                    }
                }
            }
            result?;
            return Err(DlssError::FeatureNotSupported);
        }

        Ok(Arc::new(Mutex::new(Self { parameters, device, backend })))
    }

    /// Returns the number of bytes of VRAM allocated by DLSS.
    /// DinurdoJK patch: select the Super Resolution model (NGX render preset)
    /// for one quality mode. NGX reads the hint when a feature is created, so
    /// recreate any existing `DlssSuperResolution` afterwards.
    pub fn set_render_preset(&mut self, mode: DlssPerfQualityMode, preset: DlssRenderPreset) {
        let name: &[u8] = match mode {
            DlssPerfQualityMode::Auto | DlssPerfQualityMode::Quality => {
                NVSDK_NGX_Parameter_DLSS_Hint_Render_Preset_Quality
            }
            DlssPerfQualityMode::Dlaa => NVSDK_NGX_Parameter_DLSS_Hint_Render_Preset_DLAA,
            DlssPerfQualityMode::Balanced => NVSDK_NGX_Parameter_DLSS_Hint_Render_Preset_Balanced,
            DlssPerfQualityMode::Performance => {
                NVSDK_NGX_Parameter_DLSS_Hint_Render_Preset_Performance
            }
            DlssPerfQualityMode::UltraPerformance => {
                NVSDK_NGX_Parameter_DLSS_Hint_Render_Preset_UltraPerformance
            }
        };
        unsafe {
            NVSDK_NGX_Parameter_SetUI(self.parameters, name.as_ptr().cast(), preset as u32);
        }
    }

    pub fn get_vram_allocated_bytes(&mut self) -> Result<u64, DlssError> {
        let mut vram_allocated_bytes = 0;
        check_ngx_result(unsafe {
            NGX_DLSS_GET_STATS(self.parameters, &mut vram_allocated_bytes)
        })?;
        Ok(vram_allocated_bytes)
    }
}

fn check_for_updates(project_id: Uuid) {
    thread::spawn(move || {
        with_feature_info(
            project_id,
            NVSDK_NGX_Feature_NVSDK_NGX_Feature_SuperSampling,
            |feature_info| unsafe {
                NVSDK_NGX_UpdateFeature(&feature_info.Identifier, feature_info.FeatureID);
            },
        );
        with_feature_info(
            project_id,
            NVSDK_NGX_Feature_NVSDK_NGX_Feature_RayReconstruction,
            |feature_info| unsafe {
                NVSDK_NGX_UpdateFeature(&feature_info.Identifier, feature_info.FeatureID);
            },
        );
    });
}

impl Drop for DlssSdk {
    fn drop(&mut self) {
        // NGX resources must not be released while a GPU submission uses them.
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        unsafe {
            match self.backend {
                NgxBackend::Vulkan => {
                    let hal = self.device.as_hal::<Vulkan>().unwrap();
                    check_ngx_result(NVSDK_NGX_VULKAN_DestroyParameters(self.parameters))
                        .expect("Failed to destroy Vulkan DLSS parameters");
                    check_ngx_result(NVSDK_NGX_VULKAN_Shutdown1(hal.raw_device().handle()))
                        .expect("Failed to shutdown Vulkan DLSS SDK");
                }
                #[cfg(windows)]
                NgxBackend::Dx12 => {
                    let hal = self.device.as_hal::<Dx12>().unwrap();
                    check_ngx_result(NVSDK_NGX_D3D12_DestroyParameters(self.parameters))
                        .expect("Failed to destroy D3D12 DLSS parameters");
                    check_ngx_result(NVSDK_NGX_D3D12_Shutdown1(hal.raw_device().as_raw().cast()))
                        .expect("Failed to shutdown D3D12 DLSS SDK");
                }
            }
        }
    }
}

unsafe impl Send for DlssSdk {}
unsafe impl Sync for DlssSdk {}
