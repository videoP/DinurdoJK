//! Native D3D12 NGX Super Resolution bridge.
//!
//! NVIDIA records its work into *wgpu's* command list. No additional DX12
//! command queue, CPU readback, shared texture, or cross-API copy is involved.
//! The raw command list is exposed by a small wgpu-hal accessor documented in
//! DX12_INTEGRATION.md. This code is deliberately limited to DLSS SR/DLAA;
//! Ray Reconstruction remains Vulkan-only.

use crate::{nvsdk_ngx::*, super_resolution::{DlssSuperResolutionExposure, DlssSuperResolutionRenderParameters}};
use std::ptr;
use wgpu::{CommandEncoder, TextureView, hal::api::Dx12};
use windows::core::Interface;

/// Use only while `as_hal_mut` gives exclusive access to the current wgpu
/// command encoder. The pointer belongs to wgpu and must not be closed or kept.
unsafe fn with_ngx_list<R>(
    encoder: &mut CommandEncoder,
    callback: impl FnOnce(*mut ID3D12GraphicsCommandList) -> Result<R, DlssError>,
) -> Result<R, DlssError> {
    unsafe {
        encoder.as_hal_mut::<Dx12, _, _>(|hal| {
            let hal = hal.ok_or(DlssError::FeatureNotSupported)?;
            let command_list = hal.raw_command_list()
                .ok_or(DlssError::NotInitialized)?;
            callback(command_list.as_raw().cast())
        })
    }
}

/// NGX retains no reference to this pointer beyond recording its commands;
/// the wgpu TextureView remains owned by the renderer and pinned until submit.
unsafe fn resource(view: &TextureView) -> Result<*mut ID3D12Resource, DlssError> {
    let texture = view.texture();
    let hal = unsafe { texture.as_hal::<Dx12>() }
        .ok_or(DlssError::FeatureNotSupported)?;
    Ok(unsafe { hal.raw_resource() }.as_raw().cast())
}

pub(crate) unsafe fn create_feature(
    command_encoder: &mut CommandEncoder,
    parameters: *mut NVSDK_NGX_Parameter,
    create_params: &mut NVSDK_NGX_DLSS_Create_Params,
    feature: &mut *mut NVSDK_NGX_Handle,
) -> Result<(), DlssError> {
    unsafe {
        with_ngx_list(command_encoder, |list| {
            check_ngx_result(NGX_D3D12_CREATE_DLSS_EXT(
                list, 1, 1, feature, parameters, create_params,
            ))
        })
    }
}

pub(crate) unsafe fn evaluate_feature(
    command_encoder: &mut CommandEncoder,
    parameters: *mut NVSDK_NGX_Parameter,
    feature: *mut NVSDK_NGX_Handle,
    frame: &DlssSuperResolutionRenderParameters<'_>,
    render_size: [u32; 2],
) -> Result<(), DlssError> {
    // The native NGX D3D12 helper accepts ID3D12Resource*, unlike Vulkan's
    // NVSDK_NGX_Resource_VK image-view descriptors. All resources are owned
    // by WGPU; NGX never takes ownership of them.
    let (exposure, exposure_scale, pre_exposure) = match &frame.exposure {
        DlssSuperResolutionExposure::Manual { exposure, exposure_scale, pre_exposure } => (
            unsafe { resource(exposure)? },
            exposure_scale.unwrap_or(1.0),
            pre_exposure.unwrap_or(0.0),
        ),
        DlssSuperResolutionExposure::Automatic => (ptr::null_mut(), 0.0, 0.0),
    };

    // Bindgen produces a C-compatible, plain-old-data struct. Zero all
    // unused optional resources and coordinates just like the Vulkan path.
    let mut eval: NVSDK_NGX_D3D12_DLSS_Eval_Params = unsafe { std::mem::zeroed() };
    eval.Feature.pInColor = unsafe { resource(frame.color)? };
    eval.Feature.pInOutput = unsafe { resource(frame.dlss_output)? };
    eval.Feature.InSharpness = 0.0;
    eval.pInDepth = unsafe { resource(frame.depth)? };
    eval.pInMotionVectors = unsafe { resource(frame.motion_vectors)? };
    eval.InJitterOffsetX = frame.jitter_offset[0];
    eval.InJitterOffsetY = frame.jitter_offset[1];
    eval.InRenderSubrectDimensions = NVSDK_NGX_Dimensions {
        Width: render_size[0], Height: render_size[1],
    };
    eval.InReset = frame.reset as _;
    let scale = frame.motion_vector_scale.unwrap_or([1.0, 1.0]);
    eval.InMVScaleX = scale[0];
    eval.InMVScaleY = scale[1];
    eval.pInExposureTexture = exposure;
    eval.pInBiasCurrentColorMask = match frame.bias {
        Some(bias) => unsafe { resource(bias)? },
        None => ptr::null_mut(),
    };
    eval.InPreExposure = pre_exposure;
    eval.InExposureScale = exposure_scale;
    eval.InToneMapperType = NVSDK_NGX_ToneMapperType_NVSDK_NGX_TONEMAPPER_STRING;

    unsafe {
        with_ngx_list(command_encoder, |list| {
            check_ngx_result(NGX_D3D12_EVALUATE_DLSS_EXT(
                list, feature, parameters, &mut eval,
            ))
        })
    }
}
