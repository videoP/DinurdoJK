//! Thread.
use crate::renderer::diagnostics::perf_capture::{LoopSections, PerfCapture};
use crate::renderer::diagnostics::sky_sweep::SkyPerfSweep;
use crate::renderer::diagnostics::tile_cost::{parse_pass, TileCostOverlay, TileCostProbe};
use crate::renderer::TileCostCommand;
use crate::renderer::{
    block_on, mpsc, sample_view_rotation, scene, thread, Arc, CompanionId, CompanionRenderThread,
    Duration, DynamicLightsMode, EventLoopProxy, FrameInfo, GpuPass, HashMap, InputLatencySample,
    InputDiagFrameSample, InputDiagLatchPhase, InputDiagRendererMetadata, InputDiagnostic, Instant,
    JoinHandle, LatestViewState, Mutex, PathBuf, PostEffects, PreparedMap, Receiver,
    RenderBatchTiming, RenderCommand, RenderError, RenderSnapshot, RenderStats, Renderer,
    RendererBackend, Sender, TryRecvError, UiSnapshot, UserEvent, Window, WorldRenderPath,
    WorldUploadTimings, PBR_PROFILE_MATERIALS, PBR_PROFILE_PARALLAX_OCCLUSION,
};

/// Newest app snapshot plus a publish counter, so the render loop can skip the
/// clone and every mailbox setter when no new snapshot arrived since the last
/// frame (the renderer often runs faster than the app publishes).
pub(in crate::renderer) struct SnapshotMailbox {
    pub(in crate::renderer) generation: u64,
    pub(in crate::renderer) snapshot: RenderSnapshot,
}

struct StudioSceneLoad {
    id: u64,
    cancelled: bool,
    game: Option<PathBuf>,
    map: Option<Box<PreparedMap>>,
    video: crate::ui::VideoSettings,
    progress: Arc<crate::runtime::LoadProgress>,
    worker: Option<JoinHandle<Result<Renderer, String>>>,
}

/// Panic payloads are `&str` or `String` in practice; anything else is reported
/// as an unknown panic rather than being dropped silently.
fn panic_detail(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("unknown panic")
}

/// Leave the renderer and its companion workers for the process to reclaim.
///
/// Used on the two paths where the GPU state can no longer be trusted: a device
/// loss, and a panic raised mid-frame. Running the normal teardown there means
/// destroying a surface whose last acquired image is still referenced by
/// submissions that a dead device will never retire, which makes wgpu-hal
/// assert. The device is gone either way, so its objects die with the process -
/// or, after a backend restart, with the driver-side device that already failed.
fn abandon_renderer(renderer: Renderer, companions: HashMap<CompanionId, CompanionRenderThread>) {
    std::mem::forget(companions);
    std::mem::forget(renderer);
}

/// Report the loss with the reason the device-lost callback recorded, then stop.
/// Rendering cannot resume on this device; recovery would mean building a new
/// one, which is a renderer restart.
fn stop_for_device_loss(
    proxy: &EventLoopProxy<UserEvent>,
    renderer: Renderer,
    companions: HashMap<CompanionId, CompanionRenderThread>,
) {
    let _ = proxy.send_event(UserEvent::RendererError(format!(
        "GPU device lost ({}) - renderer stopped",
        crate::renderer::device_lost_reason()
    )));
    abandon_renderer(renderer, companions);
}

/// Quake/TaystJK r_speeds modes, adapted to what Unified can actually
/// observe. WGPU multi-draw, GPU visibility and texture allocations do not
/// expose the old OpenGL counters: label estimates rather than inventing them.
fn print_r_speeds(
    renderer: &Renderer,
    camera: &crate::camera::Camera,
    snapshot: &RenderSnapshot,
    info: &FrameInfo,
) {
    let Some(world) = renderer.world.as_ref() else { return; };
    match renderer.patch_speeds {
        1 => {
            let active = crate::renderer::active_batch_selection(world, renderer.pvs_mode);
            let mask = crate::renderer::effective_area_mask(world, renderer.pvs_mode, snapshot.area_mask.as_ref());
            let proj = camera.view_projection(renderer.config.width, renderer.config.height);
            let mut candidates = 0u64;
            let mut indices = 0u64;
            let mut materials = std::collections::HashSet::new();
            for &i in active.indices {
                let batch = &active.batches[i];
                if !crate::renderer::batch_area_visible(batch, mask)
                    || crate::renderer::hidden_inline_batch(batch, &world.inline_models)
                    || crate::renderer::hidden_bsp_model_surface(renderer.draw_bsp_model_surfaces, batch, &world.material_debug)
                    || !crate::renderer::aabb_intersects_clip_frustum(batch.bounds_min, batch.bounds_max, proj)
                { continue; }
                let n = batch.indexed_range.end.saturating_sub(batch.indexed_range.start) as u64;
                if n == 0 { continue; }
                candidates += 1;
                indices += n;
                materials.insert(batch.source.pipeline);
            }
            println!("[r_speeds 1] {} material pipelines / {} visible world draw candidates, {} PVS/frustum tris ({} indices), {} world encoded batches, {} visible patches; MD3/GL overdraw/texture MB not included",
                materials.len(), candidates, indices / 3, indices,
                info.world_encoded_batches, info.patch_lod.visible_patches);
        }
        2 => {
            println!("[r_speeds 2] patch visible={} tris={}/{} (selected/full), updates={}; sphere/box cull patch/MD3 counters not tracked by Unified",
                info.patch_lod.visible_patches, info.patch_lod.selected_triangles,
                info.patch_lod.full_triangles, info.patch_lod.lod_updates);
        }
        3 => {
            let cluster = world.last_cluster.flatten().map_or(-1, |v| v as i64);
            println!("viewcluster: {cluster}");
        }
        4 => {
            if world.active_pipeline_variant.legacy_dlights {
                let draws = &world.legacy_dlight.draws;
                let triangles: u64 = draws.iter().map(|d| (d.indices.end - d.indices.start) as u64 / 3).sum();
                println!("[r_speeds 4] legacy dlight draw groups={} static tris={} lit movers={} active light sources={}; culled-surface/vertex counters unavailable",
                    draws.len(), triangles, world.legacy_dlight.inline_batches.len(), world.dynamic_lights.len());
            } else {
                println!("[r_speeds 4] legacy dlight pass not active (current mode {:?}); no compatible additive-pass counters", renderer.dynamic_lights_mode);
            }
        }
        5 => println!("zFar: {:.0}", camera.far_distance()),
        6 => println!("[r_speeds 6] world flares enabled={}; add/test/render counters not yet instrumented", renderer.world_flares_enabled),
        7 => {
            let vtx = world.vertex_buffer.size() as f64 / (1024.0 * 1024.0);
            let idx = world.index_buffer.size() as f64 / (1024.0 * 1024.0);
            println!("[r_speeds 7] BSP vertex/index GPU buffers {:.2}/{:.2} MiB; textures={} lightmaps={} (GPU texture allocations and total VRAM not tracked)",
                vtx, idx, world.textures.len(), world.lightmaps.len());
        }
        _ => {},
    }
}

// Diagnostic-only command names. Do not derive Debug on RenderCommand: many
// command payloads own WGPU resources that intentionally are not Debug.
#[allow(unreachable_patterns)]
fn menu_trace_command_name(command: &RenderCommand) -> &'static str {
    match command {
        RenderCommand::MemoryInfo => "MemoryInfo",
        RenderCommand::StartPerfCapture { .. } => "StartPerfCapture",
        RenderCommand::StartSkyPerfSweep { .. } => "StartSkyPerfSweep",
        RenderCommand::TileCost(..) => "TileCost",
        RenderCommand::BatchBegin(..) => "BatchBegin",
        RenderCommand::BatchLabel(..) => "BatchLabel",
        RenderCommand::BatchEnd => "BatchEnd",
        RenderCommand::Resize(..) => "Resize",
        RenderCommand::SetVsync(..) => "SetVsync",
        RenderCommand::SetMaxFrameLatency(..) => "SetMaxFrameLatency",
        RenderCommand::SetEarlySubmit(..) => "SetEarlySubmit",
        RenderCommand::SetPresentCopy(..) => "SetPresentCopy",
        RenderCommand::SetInputLateLatch(..) => "SetInputLateLatch",
        RenderCommand::SetMsaa(..) => "SetMsaa",
        RenderCommand::SetTextureFilter(..) => "SetTextureFilter",
        RenderCommand::SetPicmip(..) => "SetPicmip",
        RenderCommand::SetTextureCompression(..) => "SetTextureCompression",
        RenderCommand::SetDetailTextures(..) => "SetDetailTextures",
        RenderCommand::SetDetailTexture { .. } => "SetDetailTexture",
        RenderCommand::SetDetailTextureFade { .. } => "SetDetailTextureFade",
        RenderCommand::SetWireframeMask(..) => "SetWireframeMask",
        RenderCommand::SetShaderComplexity(..) => "SetShaderComplexity",
        RenderCommand::SetDebugVolumes { .. } => "SetDebugVolumes",
        RenderCommand::SetEntityMarkers(..) => "SetEntityMarkers",
        RenderCommand::SetCoastalDebugMesh(..) => "SetCoastalDebugMesh",
        RenderCommand::SetCoastalWater { .. } => "SetCoastalWater",
        RenderCommand::SetSunRayPreview { .. } => "SetSunRayPreview",
        RenderCommand::SetPvsMode(..) => "SetPvsMode",
        RenderCommand::SetFacePlaneCull(..) => "SetFacePlaneCull",
        RenderCommand::SetDrawInlineModels(..) => "SetDrawInlineModels",
        RenderCommand::SetInlineTransformMode(..) => "SetInlineTransformMode",
        RenderCommand::SetFragmentShaderMode(..) => "SetFragmentShaderMode",
        RenderCommand::SetWorldSortMode(..) => "SetWorldSortMode",
        RenderCommand::SetPostSpecialize(..) => "SetPostSpecialize",
        RenderCommand::SetColorClearMode(..) => "SetColorClearMode",
        RenderCommand::SetWorldStageDebug { .. } => "SetWorldStageDebug",
        RenderCommand::SetFastSky(..) => "SetFastSky",
        RenderCommand::SetWorldFlares(..) => "SetWorldFlares",
        RenderCommand::SetFpsCap(..) => "SetFpsCap",
        RenderCommand::SetGamma(..) => "SetGamma",
        RenderCommand::SetGammaMethod(..) => "SetGammaMethod",
        RenderCommand::SetHardwareGammaActive(..) => "SetHardwareGammaActive",
        RenderCommand::PrepareGammaMethod(..) => "PrepareGammaMethod",
        RenderCommand::SetModelBrightness(..) => "SetModelBrightness",
        RenderCommand::SetDynamicLightBrightness(..) => "SetDynamicLightBrightness",
        RenderCommand::SetPostEffects(..) => "SetPostEffects",
        RenderCommand::SetFootprintMode(..) => "SetFootprintMode",
        RenderCommand::SetGrassEnabled(..) => "SetGrassEnabled",
        RenderCommand::SetGrassPrecompute(..) => "SetGrassPrecompute",
        RenderCommand::SetGrassMidLod(..) => "SetGrassMidLod",
        RenderCommand::SetGrassFrontToBack(..) => "SetGrassFrontToBack",
        RenderCommand::SetRainOptimizations { .. } => "SetRainOptimizations",
        RenderCommand::SetContactShadowDebug(..) => "SetContactShadowDebug",
        RenderCommand::SetSsrDebug(..) => "SetSsrDebug",
        RenderCommand::SetOceanEnabled(..) => "SetOceanEnabled",
        RenderCommand::SetOceanSkyExtend(..) => "SetOceanSkyExtend",
        RenderCommand::SetOceanSettings(..) => "SetOceanSettings",
        RenderCommand::OceanWake(..) => "OceanWake",
        RenderCommand::SetPromotedOceanSettings(..) => "SetPromotedOceanSettings",
        RenderCommand::SetAuthoredOceans(..) => "SetAuthoredOceans",
        RenderCommand::SetOceanTime(..) => "SetOceanTime",
        RenderCommand::SetPerfTrace(..) => "SetPerfTrace",
        RenderCommand::SetGpuTimings(..) => "SetGpuTimings",
        RenderCommand::SetFrameTimeGraph(..) => "SetFrameTimeGraph",
        RenderCommand::SetFxZeroAlphaDiscard(..) => "SetFxZeroAlphaDiscard",
        RenderCommand::SetDebugWorldVertexAlphaOne(..) => "SetDebugWorldVertexAlphaOne",
        RenderCommand::SetGhoul2BatchDraws(..) => "SetGhoul2BatchDraws",
        RenderCommand::SetModelStateCache(..) => "SetModelStateCache",
        RenderCommand::SetDynamicBatchDraws(..) => "SetDynamicBatchDraws",
        RenderCommand::SetBspModelLod { .. } => "SetBspModelLod",
        RenderCommand::SetPatchLod { .. } => "SetPatchLod",
        RenderCommand::SetWorldCompactMinDraws(..) => "SetWorldCompactMinDraws",
        RenderCommand::SetModelPrepareThreads(..) => "SetModelPrepareThreads",
        RenderCommand::SetEncoderFinishThreads(..) => "SetEncoderFinishThreads",
        RenderCommand::SetModelRenderBundles(..) => "SetModelRenderBundles",
        RenderCommand::SetSurfaceSpritesGpu(..) => "SetSurfaceSpritesGpu",
        RenderCommand::SetSurfaceSprites(..) => "SetSurfaceSprites",
        RenderCommand::SetMd3GeometryCache(..) => "SetMd3GeometryCache",
        RenderCommand::SetModelIndexUploadCache(..) => "SetModelIndexUploadCache",
        RenderCommand::SetGhoul2BatchKeyCache(..) => "SetGhoul2BatchKeyCache",
        RenderCommand::SetGpuVisibility { .. } => "SetGpuVisibility",
        RenderCommand::SetDynamicLighting(..) => "SetDynamicLighting",
        RenderCommand::SetDynamicLightDebug(..) => "SetDynamicLightDebug",
        RenderCommand::SetRtSamples(..) => "SetRtSamples",
        RenderCommand::SetDynamicLightFalloff(..) => "SetDynamicLightFalloff",
        RenderCommand::SetRtTraceRate(..) => "SetRtTraceRate",
        RenderCommand::SetMapLightSimulation(..) => "SetMapLightSimulation",
        RenderCommand::SetClassicWorldLighting { .. } => "SetClassicWorldLighting",
        RenderCommand::SetAddedMapLightSources(..) => "SetAddedMapLightSources",
        RenderCommand::SetLampShadowSources(..) => "SetLampShadowSources",
        RenderCommand::SetEntityFixtureLightingAB(..) => "SetEntityFixtureLightingAB",
        RenderCommand::SetEntityAmbientLighting(..) => "SetEntityAmbientLighting",
        RenderCommand::SetVoxelProbeGi(..) => "SetVoxelProbeGi",
        RenderCommand::SetLocalLightShadows(..) => "SetLocalLightShadows",
        RenderCommand::SetTintedGlass(..) => "SetTintedGlass",
        RenderCommand::SetTintedGlassDebug(..) => "SetTintedGlassDebug",
        RenderCommand::SetEntityShadowLight(..) => "SetEntityShadowLight",
        RenderCommand::SetPbrSettings { .. } => "SetPbrSettings",
        RenderCommand::SetCascadedShadows(..) => "SetCascadedShadows",
        RenderCommand::SetCsmCoverageAB(..) => "SetCsmCoverageAB",
        RenderCommand::SetCullDebug(..) => "SetCullDebug",
        RenderCommand::SetForceUnifiedWorld(..) => "SetForceUnifiedWorld",
        RenderCommand::SetFoliageWind(..) => "SetFoliageWind",
        RenderCommand::SetPom(..) => "SetPom",
        RenderCommand::SetPlanarReflectionDebug(..) => "SetPlanarReflectionDebug",
        RenderCommand::SetPuddleDebug(..) => "SetPuddleDebug",
        RenderCommand::SetPuddleSkyBlend { .. } => "SetPuddleSkyBlend",
        RenderCommand::DumpMaterials { .. } => "DumpMaterials",
        RenderCommand::InspectSurface { .. } => "InspectSurface",
        RenderCommand::HybridWaveDump { .. } => "HybridWaveDump",
        RenderCommand::ClearSurfaceInspection => "ClearSurfaceInspection",
        RenderCommand::AddSurfaceDeformation(..) => "AddSurfaceDeformation",
        RenderCommand::Screenshot { .. } => "Screenshot",
        RenderCommand::CopyFrameToClipboard => "CopyFrameToClipboard",
        RenderCommand::CaptureFrontendScene { .. } => "CaptureFrontendScene",
        RenderCommand::CaptureVideoFrame { .. } => "CaptureVideoFrame",
        RenderCommand::SetUi(..) => "SetUi",
        RenderCommand::SetStreamerSettings(..) => "SetStreamerSettings",
        RenderCommand::SetCombatUi { .. } => "SetCombatUi",
        RenderCommand::SetNeonMenu(..) => "SetNeonMenu",
        RenderCommand::SetNeonChrome(..) => "SetNeonChrome",
        RenderCommand::SetNeonMenuCache(..) => "SetNeonMenuCache",
        RenderCommand::SetExtensionHud(..) => "SetExtensionHud",
        RenderCommand::SetTransientUi { .. } => "SetTransientUi",
        RenderCommand::SetUiTelemetry { .. } => "SetUiTelemetry",
        RenderCommand::SetEgui(..) => "SetEgui",
        RenderCommand::CreateCompanion { .. } => "CreateCompanion",
        RenderCommand::DestroyCompanion { .. } => "DestroyCompanion",
        RenderCommand::ResizeCompanion { .. } => "ResizeCompanion",
        RenderCommand::SetCompanionEgui { .. } => "SetCompanionEgui",
        RenderCommand::SetAssetPreviewMode(..) => "SetAssetPreviewMode",
        RenderCommand::SetAssetPreviewViewport(..) => "SetAssetPreviewViewport",
        RenderCommand::SetStudioWorldViewport(..) => "SetStudioWorldViewport",
        RenderCommand::SetStudioExportSize(..) => "SetStudioExportSize",
        RenderCommand::CaptureStudioFrame { .. } => "CaptureStudioFrame",
        RenderCommand::LoadStudioSource { .. } => "LoadStudioSource",
        RenderCommand::ReleaseStudioSources => "ReleaseStudioSources",
        RenderCommand::RemoveStudioSource(..) => "RemoveStudioSource",
        RenderCommand::UnloadMap => "UnloadMap",
        RenderCommand::GrowInlineInstances { .. } => "GrowInlineInstances",
        RenderCommand::LoadMap { .. } => "LoadMap",
        RenderCommand::RemapMap { .. } => "RemapMap",
        RenderCommand::Shutdown => "Shutdown",
        _ => "unknown command",
    }
}

/// Prefix of a Studio scene error caused by GPU memory exhaustion. The app
/// may evict retained demo scenes and retry instead of failing the clip.
pub const STUDIO_SCENE_OUT_OF_GPU_MEMORY: &str = "Not enough GPU memory for this demo scene";

pub struct RenderThread {
    pub(in crate::renderer) command_tx: Sender<RenderCommand>,
    /// Kept on the Winit/app side so auxiliary surfaces are created on the
    /// native window's origin thread before being handed to render workers.
    pub(in crate::renderer) instance: wgpu::Instance,
    pub(in crate::renderer) latest_snapshot: Arc<Mutex<SnapshotMailbox>>,
    pub(in crate::renderer) latest_view: Arc<LatestViewState>,
    pub(in crate::renderer) input_diagnostic: Arc<InputDiagnostic>,
    pub(in crate::renderer) join: Option<JoinHandle<()>>,
}

impl RenderThread {
    pub fn spawn(
        window: Arc<Window>,
        proxy: EventLoopProxy<UserEvent>,
        backend: RendererBackend,
        initial: RenderSnapshot,
        initial_ui: UiSnapshot,
        base: PathBuf,
        game: Option<PathBuf>,
        detail_texture_path: String,
    ) -> Result<Self, String> {
        let (command_tx, command_rx) = mpsc::channel();
        // Render snapshots are mailbox state, not a work queue: the renderer only
        // needs the newest camera/input state available when a frame begins.
        // A shared latest snapshot avoids retaining stale mouse samples when input
        // arrives faster than rendering (for example 1000 Hz input at 240-500 FPS).
        let latest_snapshot = Arc::new(Mutex::new(SnapshotMailbox { generation: 0, snapshot: initial.clone() }));
        let mut initial_view_camera = initial.camera;
        if let Some([yaw, pitch]) = initial.input_view_rotation {
            initial_view_camera.yaw = yaw;
            initial_view_camera.pitch = pitch;
        }
        let latest_view = Arc::new(LatestViewState::new(initial_view_camera));
        let input_diagnostic = Arc::new(InputDiagnostic::new());
        let render_snapshot = Arc::clone(&latest_snapshot);
        let render_latest_view = Arc::clone(&latest_view);
        let render_input_diagnostic = Arc::clone(&input_diagnostic);
        let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
        // Release builds run with the backend validation layers off. A GPU fault
        // ("[GPU DEVICE LOST] reason=Unknown: Device is lost") cannot be traced to
        // the command that caused it without them, so honour wgpu's own env vars
        // for a single diagnostic run: WGPU_VALIDATION=1, plus
        // WGPU_GPU_BASED_VALIDATION=1 (shader-side bounds checks) and
        // WGPU_VALIDATION_INDIRECT_CALL=1 (indirect draw/dispatch arguments).
        // wgpu's release default keeps VALIDATION_INDIRECT_CALL, which adds a
        // compute pass, buffer copies and barriers to every frame with an indirect
        // draw (measured on an Intel UHD 630). Our indirect arguments are built by
        // the renderer itself; WGPU_VALIDATION_INDIRECT_CALL=1 turns it back on.
        instance_desc.flags.remove(wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL);
        instance_desc.flags = instance_desc.flags.with_env();
        if instance_desc.flags.contains(wgpu::InstanceFlags::VALIDATION) {
            eprintln!("[GPU] backend validation layers enabled: {:?}", instance_desc.flags);
        }
        instance_desc.backends = match backend {
            RendererBackend::Vulkan => wgpu::Backends::VULKAN,
            RendererBackend::Dx12 => wgpu::Backends::DX12,
        };
        // DX12 prefers the DXC runtime shipped beside DinurdoJK.exe (loaded by
        // absolute path so no other dxcompiler.dll on the system can shadow it).
        // If it is missing or fails to initialise (seen: LoadLibrary error 1114
        // on an Intel iGPU PC), wgpu drops the whole DX12 backend and surface
        // creation fails with "any enabled backend: {}". Fall back to the FXC
        // compiler built into Windows instead of losing DX12.
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf));
        let dxc_path = exe_dir
            .as_ref()
            .map(|dir| dir.join("dxcompiler.dll"))
            .filter(|path| path.is_file() && path.with_file_name("dxil.dll").is_file());
        if backend == RendererBackend::Dx12 {
            // wgpu has no DX12 pipeline cache and DXC takes ~120 ms per large
            // shader, so the patched wgpu-hal keeps compiled bytecode here.
            std::env::set_var("DINURDOJK_DX12_SHADER_CACHE", base.join("cache").join("dx12_shaders"));
            instance_desc.backend_options.dx12.shader_compiler = match &dxc_path {
                Some(path) => wgpu::Dx12Compiler::DynamicDxc {
                    dxc_path: path.to_string_lossy().into_owned(),
                },
                None => {
                    eprintln!(
                        "[GPU] DX12: dxcompiler.dll/dxil.dll not beside DinurdoJK.exe; using the Windows FXC shader compiler"
                    );
                    wgpu::Dx12Compiler::Fxc
                }
            };
        }
        let instance = wgpu::Instance::new(instance_desc);
        let (instance, surface) = match instance.create_surface(window.clone()) {
            Ok(surface) => (instance, surface),
            Err(error) if backend == RendererBackend::Dx12 && dxc_path.is_some() => {
                eprintln!(
                    "[GPU] DX12 with the DXC runtime failed ({error}); retrying with the Windows FXC shader compiler"
                );
                let mut fxc_desc = wgpu::InstanceDescriptor::new_without_display_handle();
                fxc_desc.flags = fxc_desc.flags.with_env();
                fxc_desc.backends = wgpu::Backends::DX12;
                fxc_desc.backend_options.dx12.shader_compiler = wgpu::Dx12Compiler::Fxc;
                let instance = wgpu::Instance::new(fxc_desc);
                let surface = instance
                    .create_surface(window.clone())
                    .map_err(|error| format!("Could not create WGPU surface: {error}"))?;
                (instance, surface)
            }
            Err(error) => return Err(format!("Could not create WGPU surface: {error}")),
        };
        // Winit's Win32 window handles are thread-affine. Keep one cheap WGPU
        // Instance handle here so future native surfaces can be created by App
        // on the Winit thread, then moved to their dedicated render workers.
        let surface_instance = instance.clone();
        let panic_proxy = proxy.clone();
        let join = thread::Builder::new()
            .name("jka-render".into())
            // Full scene initialization and rendering have large descriptor
            // frames. Studio can enter an independent scene from this worker.
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    render_thread_main(
                        window,
                        instance,
                        surface,
                        proxy,
                        command_rx,
                        render_snapshot,
                        render_latest_view,
                        render_input_diagnostic,
                        initial,
                        initial_ui,
                        base,
                        game,
                        detail_texture_path,
                    )
                }));
                if let Err(payload) = result {
                    let _ = panic_proxy.send_event(UserEvent::RendererError(format!(
                        "render thread panicked: {}",
                        panic_detail(&*payload)
                    )));
                }
            })
            .map_err(|e| format!("Could not start render thread: {e}"))?;
        Ok(Self {
            command_tx,
            instance: surface_instance,
            latest_snapshot,
            latest_view,
            input_diagnostic,
            join: Some(join),
        })
    }

    /// Create a companion surface on the Winit/window thread, then transfer
    /// the already-created WGPU surface to the renderer. This mirrors the
    /// primary window path and avoids Win32 raw-handle thread affinity.
    pub fn create_companion(&self, id: CompanionId, window: Arc<Window>) -> Result<(), String> {
        let size = window.inner_size();
        let surface = self
            .instance
            .create_surface(window.clone())
            .map_err(|error| format!("Could not create companion WGPU surface: {error}"))?;
        self.command_tx
            .send(RenderCommand::CreateCompanion {
                id,
                window,
                size,
                surface,
            })
            .map_err(|_| "Companion renderer is unavailable".to_owned())?;
        Ok(())
    }

    pub fn command(&self, command: RenderCommand) {
        let _ = self.command_tx.send(command);
    }

    pub fn publish(&self, snapshot: RenderSnapshot) {
        // Subframe input is permanent. Publish the tiny *input-view* tuple first.
        // In third person the snapshot camera is derived output and must never
        // overwrite newer raw player viewangles in the lock-free mailbox.
        if let Some([yaw, pitch]) = snapshot.input_view_rotation {
            self.latest_view.publish_rotation(
                yaw,
                pitch,
                snapshot.dynamic_crosshair_world,
                snapshot.input_latency,
            );
        }
        let mut latest = match self.latest_snapshot.lock() {
            Ok(latest) => latest,
            Err(poisoned) => poisoned.into_inner(),
        };
        latest.snapshot = snapshot;
        latest.generation = latest.generation.wrapping_add(1);
    }

    pub fn publish_subframe_view_rotation(
        &self,
        yaw: f32,
        pitch: f32,
        crosshair_world: Option<[f32; 3]>,
        input: InputLatencySample,
    ) {
        self.latest_view
            .publish_rotation(yaw, pitch, crosshair_world, Some(input));
    }

    /// Request a bounded renderer CPU recording after map/shader warmup.
    pub fn start_render_perf_capture(&self, stem: PathBuf, frames: usize, seconds: Option<f64>) -> Result<(), &'static str> {
        self.command_tx.send(RenderCommand::StartPerfCapture { stem, frames, seconds })
            .map_err(|_| "renderer command channel closed")
    }

    pub fn tile_cost(&self, command: TileCostCommand) -> Result<(), &'static str> {
        self.command_tx.send(RenderCommand::TileCost(command))
            .map_err(|_| "renderer command channel closed")
    }

    pub fn start_sky_perf_sweep(&self, stem: PathBuf, frames: usize) -> Result<(), &'static str> {
        self.command_tx.send(RenderCommand::StartSkyPerfSweep { stem, frames })
            .map_err(|_| "renderer command channel closed")
    }

    /// Arm the short input/presentation capture. Recording begins on the first
    /// captured mouse event so opening/closing the console is excluded.
    pub fn arm_input_diagnostic(&self, stem: PathBuf) -> Result<(), &'static str> {
        self.input_diagnostic.arm(stem)
    }

    #[inline]
    pub fn begin_input_diagnostic_mouse(&self, event_at: Instant) {
        self.input_diagnostic.begin_mouse_event(event_at);
    }

    #[inline]
    pub fn finish_input_diagnostic_mouse(
        &self,
        event_at: Instant,
        simulation_at: Option<Instant>,
        completed_at: Instant,
        dx: f64,
        dy: f64,
        input_sequence: Option<u64>,
        device_id_hash: u64,
        window_focused: bool,
        captured: bool,
        grab_mode: &'static str,
    ) {
        self.input_diagnostic.finish_mouse_event(
            event_at,
            simulation_at,
            completed_at,
            dx,
            dy,
            input_sequence,
            device_id_hash,
            window_focused,
            captured,
            grab_mode,
        );
    }

    pub fn record_input_diagnostic_event_loop_turn(
        &self,
        at: Instant,
        cause: &'static str,
        window_focused: bool,
        captured: bool,
        grab_mode: &'static str,
    ) {
        self.input_diagnostic.record_event_loop_turn(
            at,
            cause,
            window_focused,
            captured,
            grab_mode,
        );
    }

    pub fn take_input_diagnostic_notices(&self) -> Vec<String> {
        self.input_diagnostic.take_notices()
    }

    pub fn shutdown(&mut self) {
        let _ = self.command_tx.send(RenderCommand::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }

    /// Ask the render thread to exit without waiting for it. Swapchain and
    /// surface teardown can synchronously message the window's owning thread
    /// (DXGI and some Vulkan drivers do), so the UI thread must keep pumping
    /// its event loop while the renderer drops instead of blocking in `join`.
    pub fn request_shutdown(&self) {
        let _ = self.command_tx.send(RenderCommand::Shutdown);
    }

    /// True once the render thread has returned (or was already detached).
    pub fn is_finished(&self) -> bool {
        self.join.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Detach a render thread that never exited so dropping this handle cannot
    /// block on it. The thread keeps its window/device alive until it dies.
    pub fn abandon(&mut self) {
        self.join.take();
    }
}

impl Drop for RenderThread {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub(in crate::renderer) fn apply_scene_video_settings(renderer: &mut Renderer, video: &crate::ui::VideoSettings) {
    renderer
        .dynamic_model_renderer
        .set_fx_zero_alpha_discard(video.fx_zero_alpha_discard);
    renderer.set_msaa(video.msaa_samples);
    renderer.set_texture_filter(video.texture_filter);
    renderer.set_picmip(video.picmip);
    renderer.set_detail_textures(video.detail_textures);
    renderer.set_detail_texture_fade(
        video.detail_texture_fade,
        video.detail_texture_fade_distance,
    );
    renderer.set_wireframe_mask(video.wireframe_mask);
    renderer.shader_complexity_enabled = video.shader_complexity;
    renderer.debug_volumes.triggers_enabled = video.draw_triggers;
    renderer.debug_volumes.clips_enabled = video.draw_clip_brushes;
    renderer.set_pvs_mode(video.pvs_mode);
    renderer.set_face_plane_cull(video.face_plane_cull);
    renderer.set_gamma_method(
        if video.gamma_method.is_hardware() {
            crate::gamma::GammaMethod::Shader
        } else {
            video.gamma_method
        },
    );
    renderer.set_gamma(video.gamma);
    renderer.set_model_brightness(video.effective_model_brightness());
    renderer.set_dynamic_light_brightness(video.effective_dynamic_light_brightness());
    renderer
        .surface_deformation
        .set_mode(&renderer.queue, video.footprints);
    let effective_lighting = video.effective_relighting();
    renderer.set_post_effects(PostEffects {
        thermal_vision: video.thermal_vision,
        hdr: video.hdr,
            hdr_paper_white: video.hdr_paper_white,
            hdr_peak: video.hdr_peak,
        auto_exposure: video.auto_exposure,
        tone_mapping: video.tone_mapping,
        glow: video.glow,
        halation: video.halation,
        ssao: video.ssao,
        static_bsp_ao: video.static_bsp_ao,
        static_bsp_ao_lightmap: video.static_bsp_ao_lightmap,
        static_bsp_ao_samples: video.static_bsp_ao_samples,
        static_bsp_ao_resolution: video.static_bsp_ao_resolution,
        static_bsp_ao_strength: video.static_bsp_ao_strength,
        static_bsp_ao_range: video.static_bsp_ao_range,
        static_bsp_ao_current_cell: video.static_bsp_ao_current_cell,
        fxaa: video.fxaa,
        smaa: video.smaa,
        taa: video.taa,
        rt_taa: video.rt_taa,
        contact_shadows: video.contact_shadows,
        fog_mode: video.fog_mode,
        fog_strength: video.fog_strength,
        sun_override: video.sun_override,
        sun_yaw: video.sun_yaw,
        sun_pitch: video.sun_pitch,
        sun_intensity: video.sun_intensity,
        sun_shadow_strength: video.sun_shadow_strength,
        sun_lighting: effective_lighting.sun_lighting,
        sun_color: video.sun_color,
        sun_visibility: video.sun_visibility,
        entity_sun_lighting: effective_lighting.entity_sun_lighting,
        baked_sun_removal: effective_lighting.baked_sun_removal,
        baked_sun_cache: video.baked_sun_cache,
        baked_sun_shadow_map: video.baked_sun_shadow_map,
        baked_sun_edge_softness: video.baked_sun_edge_softness,
        baked_sun_direct_limit: video.baked_sun_direct_limit,
        baked_indirect_scale: effective_lighting.baked_indirect_scale,
        baked_indirect_saturation: effective_lighting.baked_indirect_saturation,
        baked_indirect_power: effective_lighting.baked_indirect_power,
        baked_indirect_highlight_preserve: effective_lighting.baked_indirect_highlight_preserve,
        sky_fractal_override: video.sky_fractal_override,
        sky_backend: video.sky_backend,
        blue_water_environment: effective_lighting.blue_water_environment,
        cloud_backend: video.cloud_backend,
        cloud_portal_depth: video.cloud_portal_depth,
        clouds: video.clouds,
        cloud_type: video.cloud_type,
        cloud_quality: video.cloud_quality,
        cloud_coverage: video.cloud_coverage,
        cloud_height: video.cloud_height,
        cloud_thickness: video.cloud_thickness,
        weather_wind: video.weather_wind,
        wind_visuals: video.wind_visuals,
        cloud_shadows: video.cloud_shadows,
        cloud_render_resolution: video.cloud_render_resolution,
        cloud_temporal: video.cloud_temporal,
        cloud_temporal_depth_fix: video.cloud_temporal_depth_fix,
        cloud_shear: video.cloud_shear,
        cloud_base_variation: video.cloud_base_variation,
        cloud_shape_evolution: video.cloud_shape_evolution,
        cloud_terrain_interaction: video.cloud_terrain_interaction,
        cloud_empty_skip: video.cloud_empty_skip,
        cloud_aerial: video.cloud_aerial,
        cloud_sky_ambient: video.cloud_sky_ambient,
        cloud_history_blend: video.cloud_history_blend,
        cloud_motion_reject: video.cloud_motion_reject,
        cloud_history_depth_reject: video.cloud_history_depth_reject,
        cloud_thickness_variation: video.cloud_thickness_variation,
        cloud_size: video.cloud_size,
        rain: video.rain,
        rain_intensity: video.rain_intensity,
        rain_density_scale: 1.0,
        rain_tint: [1.0; 3],
        rain_frozen: false,
        snow: false,
        snow_density_scale: 1.0,
        snow_frozen: false,
        legacy_sand_layers: 0,
        legacy_space_dust_count: 0,
        legacy_weather_frozen: false,
        legacy_acid_rain: 0.0,
        rain_glass: video.rain_glass,
        puddle_quality: video.puddle_quality,
        puddle_scatter: video.puddle_scatter,
        rain_grade: video.rain_grade,
        reflection_quality: video.reflection_quality,
        reflection_debug: video.reflection_debug,
        chromatic_aberration: video.chromatic_aberration,
        vignette: video.vignette,
        film_grain_strength: video.film_grain_strength,
        motion_blur_strength: video.motion_blur_strength,
        depth_of_field_strength: video.depth_of_field_strength,
        dof_mode: video.dof_mode,
        crt_scope: video.crt_scope,
        crt_style: video.crt_style,
        crt_quality: video.crt_quality,
        crt_reference: video.crt_reference,
        crt_mask: video.crt_mask,
        crt_scanline: video.crt_scanline,
        crt_sharpness: video.crt_sharpness,
        crt_mask_intensity: video.crt_mask_intensity,
        crt_curvature: video.crt_curvature,
        crt_corner: video.crt_corner,
        dof_quality: video.dof_quality,
        color_lut: video.color_lut,
        color_lut_strength: video.color_lut_strength,
        color_grade: video.color_grade,
        split_toning: video.split_toning,
    });
    renderer.set_grass_enabled(video.grass);
    renderer.set_grass_precompute(video.grass_precompute);
    renderer.set_grass_mid_lod(video.grass_mid_lod);
    renderer.set_grass_front_to_back(video.grass_front_to_back);
    renderer.set_rain_optimizations(
        video.rain_fast_post,
        video.rain_water_field,
        video.rain_fast_vertex,
        video.rain_lean_depth,
        video.rain_static_depth,
        video.rain_opaque_depth,
        video.rain_lean_models,
        video.rain_lean_post,
    );
    let mut initial_ocean_settings = video.ocean_settings;
    initial_ocean_settings.wind = video.weather_wind;
    renderer.set_ocean_settings(initial_ocean_settings);
    renderer.set_ocean_enabled(video.ocean);
    renderer.set_gpu_timings(video.gpu_timings);
    renderer.set_ghoul2_batch_draws(video.ghoul2_batch_draws);
    renderer.dynamic_model_renderer.model_state_cache = video.model_state_cache;
    renderer.dynamic_model_renderer.dynamic_batch_draws = video.dynamic_batch_draws;
    renderer.world_compact_min_draws = video.world_compact_min_draws;
    renderer.patch_curve_error = video.lod_curve_error;
    renderer.bsp_model_lod_scale = video.lod_scale;
    renderer.bsp_model_lod_bias = video.ghoul2_lod_bias;
    renderer.bsp_model_lod_debug = video.md3_lod_debug;
    renderer.bsp_model_lod_enabled = video.md3_auto_lod && video.bsp_model_lod;
    renderer.patch_no_curves = video.no_curves;
    renderer.patch_speeds = video.speeds;
    if let Err(error) = renderer.dynamic_model_renderer.set_model_prepare_threads(video.model_prepare_threads) {
        eprintln!("Model packing workers: {error}");
    }
    renderer.dynamic_model_renderer.set_md3_geometry_cache(video.md3_geometry_cache);
    renderer.dynamic_model_renderer.set_model_render_bundles(video.model_render_bundles);
    renderer.surface_sprite_effect_renderer.resident_enabled = video.surface_sprites_gpu;
    renderer.surface_sprite_effect_renderer.enabled = video.surface_sprites;
    if let Err(error) = renderer.set_encoder_finish_threads(video.encoder_finish_threads) {
        eprintln!("Encoder finish worker: {error}");
    }
    renderer.dynamic_model_renderer.set_model_index_upload_cache(video.model_index_upload_cache);
    renderer.dynamic_model_renderer.ghoul2_batch_key_cache = video.ghoul2_batch_key_cache;
    renderer.set_gpu_visibility(video.gpu_driven, video.hiz_occlusion);
    renderer.early_submit_enabled = video.early_submit;
    renderer.present_copy_enabled = video.present_copy;
    renderer.texture_compression = video.texture_compression.min(2);
    renderer.set_draw_inline_models(video.draw_inline_models);
    renderer.set_inline_transform_mode(video.inline_transform_mode);
    renderer.set_fragment_shader_mode(video.fragment_shader_mode);
    renderer.world_sort_mode = video.world_sort_mode.min(1);
    renderer.post_specialize = video.post_specialize;
    renderer.color_clear_mode = video.color_clear_mode.min(1);
    renderer.set_world_stage_debug(video.draw_bsp_model_surfaces, video.world_stage_probe);
    renderer.set_fast_sky(video.fast_sky);
    renderer.set_dynamic_lighting(video.dynamic_lights);
    renderer.set_rt_samples(video.rt_samples);
    renderer.set_dynamic_light_falloff(video.dynamic_light_falloff);
    renderer.set_rt_trace_rate(video.rt_trace_rate);
    renderer.set_map_light_simulation(effective_lighting.map_light_simulation);
    renderer.set_added_map_light_sources(video.added_map_light_sources());
    renderer.set_lamp_shadow_sources(video.baked_local_sources);
    renderer.set_lamp_shadow_ab(video.lamp_shadow_ab);
    renderer.set_entity_fixture_lighting_ab(video.entity_fixture_lighting_ab);
    renderer.set_csm_coverage_ab(video.csm_coverage_ab);
    renderer.set_entity_ambient_lighting(video.entity_ambient_lighting);
    renderer.set_voxel_probe_gi(video.voxel_probe_gi);
    renderer.set_local_light_shadows(video.local_light_shadows);
    renderer.set_tinted_glass(video.tinted_glass);
    renderer.set_tinted_glass_debug(video.tinted_glass_debug);
    renderer.set_entity_shadow_light(video.entity_shadow_light);
    renderer.set_pbr_settings(
        video.pbr,
        video.deluxe_mapping,
        video.deluxe_specular,
    );
    renderer.set_cascaded_shadows(video.effective_shadow_mode());
    renderer.set_cull_debug(video.cull_debug);
    renderer.set_force_unified_world(video.force_unified_world);
    renderer.foliage_wind_enabled = video.foliage_wind;
    renderer.set_pom_enabled(video.pom);
    renderer.set_planar_reflection_debug(video.planar_reflection_debug);
}

#[allow(clippy::too_many_arguments)]
pub(in crate::renderer) fn render_thread_main(
    window: Arc<Window>,
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    proxy: EventLoopProxy<UserEvent>,
    command_rx: Receiver<RenderCommand>,
    latest_snapshot: Arc<Mutex<SnapshotMailbox>>,
    latest_view: Arc<LatestViewState>,
    input_diagnostic: Arc<InputDiagnostic>,
    mut snapshot: RenderSnapshot,
    initial_ui: UiSnapshot,
    base: PathBuf,
    game: Option<PathBuf>,
    detail_texture_path: String,
) {
    let renderer_init_started = Instant::now();
    let mut renderer = match block_on(Renderer::new(
        window,
        instance,
        surface,
        &base,
        game.as_deref(),
        initial_ui.video.max_frame_latency,
        initial_ui.video.hdr_output,
        &detail_texture_path,
        initial_ui.video.gpu_adapter,
    )) {
        Ok(renderer) => renderer,
        Err(error) => {
            let _ = proxy.send_event(UserEvent::RendererError(error));
            return;
        }
    };
    rverbose!(
        1,
        "[RENDER INIT] base device/resources/pipelines {:.1} ms",
        renderer_init_started.elapsed().as_secs_f64() * 1000.0
    );
    renderer.set_vsync(initial_ui.video.vsync);
    {
        let info = renderer.adapter.get_info();
        input_diagnostic.set_renderer_metadata(InputDiagRendererMetadata {
            adapter_name: info.name,
            backend: format!("{:?}", info.backend),
            device_type: format!("{:?}", info.device_type),
            driver: info.driver,
            driver_info: info.driver_info,
            vendor: info.vendor,
            device: info.device,
            surface_format: format!("{:?}", renderer.config.format),
            alpha_mode: format!("{:?}", renderer.config.alpha_mode),
            surface_usage: format!("{:?}", renderer.config.usage),
            supported_present_modes: format!("{:?}", renderer.present_modes),
        });
    }
    apply_scene_video_settings(&mut renderer, &initial_ui.video);
    let mut fps_cap = initial_ui.video.fps_cap;
    let mut input_latelatch = initial_ui.video.input_latelatch;
    // `inputdiag` force-enables GPU timestamps for the capture, because the CPU
    // side of this loop never waits on the GPU and therefore cannot show a GPU
    // that is missing the cap. These two track what to put back afterwards.
    let mut user_gpu_timings = initial_ui.video.gpu_timings;
    let mut diag_forced_gpu_timings = false;
    let mut perf_trace = initial_ui.video.perf_trace;
    renderer.set_ui(initial_ui);
    renderer.set_dynamic_hud(snapshot.hud, snapshot.movement_hud);
    renderer.set_player_names(snapshot.player_names.clone());
    renderer.set_jump_shade(snapshot.jump_shade);
    renderer.set_screen_fx(&snapshot.screen_fx);
    renderer.set_cloud_foreground(&snapshot.cloud_foreground);
    rverbose!(
        1,
        "[RENDER INIT] ready after applying video state {:.1} ms total",
        renderer_init_started.elapsed().as_secs_f64() * 1000.0
    );
    // The UI must see the same HDR capability verdict used to select the
    // actual presentation format. Query even when SDR was requested.
    let hdr_capabilities = renderer.surface.get_capabilities(&renderer.adapter);
    let hdr_display_info = renderer.surface.display_hdr_info(&renderer.adapter);
    let mut hdr_available_modes = super::hdr_output::available_modes(
        &hdr_capabilities,
        Some(&hdr_display_info),
    );
    let _ = proxy.send_event(UserEvent::RendererReady {
        supported_msaa: renderer.supported_msaa.clone(),
        wireframe_supported: renderer.wireframe_supported,
        hdr_output_status: renderer.hdr_output.as_ref().map_or_else(|| "SDR".to_owned(), |o| o.status()),
        hdr_output_active: renderer.hdr_output.as_ref().is_some_and(|o| matches!(o.selection.encoding, 1 | 2)),
        hdr_available_modes,
        adapter_name: renderer.adapter.get_info().name,
        adapter_type: format!("{:?}", renderer.adapter.get_info().device_type),
    });
    let mut companion_renderers: HashMap<CompanionId, CompanionRenderThread> = HashMap::new();

    let mut stats_started = Instant::now();
    // Start of the current cap interval. Pacing from a fixed cadence rather than
    // from the top of each frame keeps the loop's own overhead (command drain,
    // snapshot swap, scene rebuild) out of the interval, so a cap of N yields N.
    let mut frame_pace_anchor = Instant::now();
    let mut stats_frames = 0_u64;
    let mut last_info = FrameInfo::default();
    // Graph stage attribution is only taken from a completed render frame.
    let mut previous_graph_frame_complete = false;
    let mut last_r_speeds = Instant::now();
    let mut last_fx_diag_gpu = Instant::now();
    let mut last_ocean_mesh_diag = Instant::now();
    let mut last_measured_input_sequence = 0_u64;
    let mut input_latency_samples = 0_u64;
    let mut input_latch_samples = 0_u64;
    let mut input_event_to_sim_sum_ms = 0.0_f64;
    let mut input_sim_to_render_sum_ms = 0.0_f64;
    let mut input_event_to_latch_sum_ms = 0.0_f64;
    let mut input_latch_to_submit_sum_ms = 0.0_f64;
    let mut input_latch_to_present_call_sum_ms = 0.0_f64;
    let mut input_event_to_present_call_sum_ms = 0.0_f64;
    let mut input_event_to_present_call_max_ms = 0.0_f64;
    let mut pending_world_uploaded: Option<(
        u64,
        Instant,
        String,
        usize,
        usize,
        f64,
        WorldUploadTimings,
        scene::MapLoadTimings,
        Option<scene::BspMapStats>,
    )> = None;

    // The render thread can run several times faster than camera/input updates.
    // Track the previous *camera snapshot* rather than the previous rendered frame
    // so camera motion remains visible instead of collapsing to zero at 2000+ FPS.
    let mut motion_reference_camera = snapshot.camera;
    let mut shake_rng = crate::camera::ShakeRng::default();
    let mut motion_camera_sample_dt = 1.0 / 120.0;
    let mut motion_camera_changed_at = Instant::now();

    // Owned exclusively by this thread. There is no shared collection state,
    // and each successful frame adds one small, allocation-free PerfRow.
    let mut perf_capture: Option<PerfCapture> = None;
    let mut sky_sweep: Option<SkyPerfSweep> = None;
    let mut tile_probe: Option<TileCostProbe> = None;
    // Loop-section timing for render_perf_capture only (no clock reads otherwise).
    let mut last_render_returned: Option<Instant> = None;
    // Mailbox generation already applied; MAX forces the first frame to apply.
    let mut applied_snapshot_generation = u64::MAX;
    let mut tile_probe_reported_at = Instant::now();

    // Opt-in console diagnostics. Keep the normal maximum-FPS path free of
    // per-frame formatting/printing, and only aggregate when explicitly asked.
    let mut trace_started = Instant::now();
    let mut trace_frames = 0_u64;
    let mut trace_frame_ms = 0.0_f64;
    let mut trace_prepare_ms = 0.0_f64;
    let mut trace_acquire_ms = 0.0_f64;
    let mut trace_encode_ms = 0.0_f64;
    let mut trace_submit_ms = 0.0_f64;
    let mut trace_present_ms = 0.0_f64;
    let mut trace_dynamic_model_ms = 0.0_f64;
    let mut trace_grass_cpu_ms = 0.0_f64;
    let mut trace_surface_sprite_cpu_ms = 0.0_f64;
    let mut trace_ocean_topology_ms=0.0_f64;
    let mut trace_ocean_topology_max_ms=0.0_f64;
    // The per-pass GPU numbers printed below come from `gpu_profiler.latest`,
    // which is ONE frame. That is misleading for deliberately amortized work:
    // the ocean FFT encodes one cascade on roughly one frame in N, so a single
    // sample cannot tell a ~3ms cascade frame from a ~0.04ms idle frame, and
    // either one read alone gives the wrong answer about what the ocean costs.
    // Accumulate min/avg/max across the trace window for those passes instead.
    const TRACE_GPU_PASSES: [GpuPass; 5] = [
        GpuPass::Frame,
        GpuPass::OceanSim,
        GpuPass::CoastalSim,
        GpuPass::OceanSurface,
        GpuPass::OceanOptics,
    ];
    let mut trace_gpu_sum = [0.0_f64; TRACE_GPU_PASSES.len()];
    let mut trace_gpu_min = [f64::INFINITY; TRACE_GPU_PASSES.len()];
    let mut trace_gpu_max = [0.0_f64; TRACE_GPU_PASSES.len()];
    let mut trace_gpu_n = [0_u64; TRACE_GPU_PASSES.len()];
    // Advanced-path prepare breakdown: camera, shadow, world, reflection.
    let mut trace_prep_stage_ms = [0.0_f64; 4];
    if perf_trace {
        println!("JKA perf trace enabled (enable GPU TIMESTAMPS too for per-pass GPU timings)");
    }

    let mut shader_progress_polled_at = Instant::now() - Duration::from_millis(100);
    let mut gamma_preparation = None;
    let mut first_frame_notified = false;
    let mut first_menu_frame_notified = false;
    let mut neon_compositor_ready_notified = false;
    let mut last_stage_token: Option<u64> = None;
    let mut batch_timing: Option<RenderBatchTiming> = None;

    let mut deferred_load_map = None;
    // Separate queue/deferred-upload/first-render intervals. The old
    // first-frame residual lumped all of these together as "unaccounted".
    let mut load_map_received_at: Option<Instant> = None;
    let mut pending_first_world_phases: Option<(Instant, Instant, Instant, Instant)> = None;
    // Explicit type is needed: the first read (`take`) precedes the branch
    // that assigns a `RemapMap`, so inference cannot discover the tuple's
    // `PreparedRemapPlan` type from its field accesses alone.
    let mut deferred_remap_map: Option<(
        u64,
        Instant,
        String,
        Box<crate::scene::PreparedRemapPlan>,
        usize,
        Option<(u64, crate::scene::ShaderRemapState)>,
    )> = None;
    let mut hdr_capabilities_last_checked = Instant::now();
    let mut studio_scene_loads: Vec<StudioSceneLoad> = Vec::new();

    loop {
        // Shader/pipeline initialization must not stop presentation of an
        // already-playing source. Join only finished jobs on the render owner.
        let mut i = 0;
        while i < studio_scene_loads.len() {
            if studio_scene_loads[i].cancelled && studio_scene_loads[i].worker.is_none() {
                studio_scene_loads.remove(i); continue;
            }
            if studio_scene_loads[i].worker.is_none() {
                // Count cancelled but still-running initializers too: edits
                // cannot grow another set of GPU resources behind the cache.
                let occupied = studio_scene_loads.iter().filter(|l| l.worker.is_some()).count();
                if occupied < 2 {
                    let load = &mut studio_scene_loads[i];
                    // Retain prepared scenes, but allocate full-size targets
                    // only when a scene is actually drawn.
                    let size = winit::dpi::PhysicalSize::new(1, 1);
                    load.progress.set("Preparing preview", 0, 0);
                    let init = Renderer::new_scene(&renderer, load.game.as_deref(), size);
                    let video = load.video;
                    let map = load.map.take().unwrap();
                    let progress = Arc::clone(&load.progress);
                    let device = renderer.device.clone();
                    // Renderer initialization needs the same stack budget as
                    // the primary render thread (large pipeline/future state).
                    match thread::Builder::new().name("studio-scene-init".into())
                        .stack_size(8 * 1024 * 1024).spawn(move || {
                        // Error scopes are per thread, and every allocation of
                        // this scene happens here: running out of VRAM fails
                        // this scene instead of hitting wgpu's fatal handler.
                        let oom = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
                        let result = block_on(init).map(|mut source| {
                            apply_scene_video_settings(&mut source, &video);
                            source.studio_settings_signature = format!("{video:?}");
                            source.apply_pending_scene_rebuild();
                            let mut ui = UiSnapshot::default();
                            ui.video = video;
                            ui.video.skip_ui = true;
                            source.set_ui(ui);
                            progress.set("Uploading map images", 0, 0);
                            source.load_shared_studio_map(*map, None);
                            // Not drawn yet: hold 1x1 shadow arrays until it is.
                            source.set_shadow_arrays_parked(true);
                            source.set_local_shadow_cache_parked(true);
                            progress.set("Ready", 1, 1);
                            source
                        });
                        match block_on(oom.pop()) {
                            Some(error) => Err(format!("{STUDIO_SCENE_OUT_OF_GPU_MEMORY} ({error})")),
                            None => result,
                        }
                    }) {
                        Ok(worker) => load.worker = Some(worker),
                        Err(error) => {
                            let _ = proxy.send_event(UserEvent::StudioSourceReady {
                                id: load.id, error: Some(error.to_string()), vram: None, scenes: renderer.studio_scenes.len(),
                            });
                            studio_scene_loads.remove(i); continue;
                        }
                    }
                }
            }
            if !studio_scene_loads[i].worker.as_ref().is_some_and(|w| w.is_finished()) { i += 1; continue; }
            let load = studio_scene_loads.remove(i);
            let result = load.worker.unwrap().join().unwrap_or_else(|_| Err("Studio scene initialization panicked".into()));
            if load.cancelled { continue; }
            let result = result.map(|source| {
                #[cfg(test)]
                if std::env::var_os("DJK_STUDIO_TEST_FOUR_DEMOS").is_some() {
                    let shared = source.world.as_ref().and_then(|world| renderer.studio_scenes.values()
                        .filter_map(|s| s.world.as_ref()).find(|previous|
                            previous.remap_asset_source == world.remap_asset_source && previous.remap_asset_hash == world.remap_asset_hash));
                    if let (Some(previous), Some(world)) = (shared, source.world.as_ref()) {
                        let aliases = world.textures.iter().zip(&previous.textures)
                            .filter(|(a, b)| a._texture == b._texture).count();
                        assert!(aliases > 0, "same-map Studio views must share GPU texture handles");
                        println!("[STUDIO TEST] {aliases} world textures alias the other demo view");
                    }
                }
                renderer.studio_last_scene = None;
                renderer.studio_scenes.insert(load.id, Box::new(source));
                load.progress.set("Ready", 1, 1);
            });
            let vram = super::diagnostics::vram::query(&renderer.adapter.get_info())
                .map(|v| (v.usage, v.budget));
            let _ = proxy.send_event(UserEvent::StudioSourceReady {
                id: load.id, error: result.err(), vram, scenes: renderer.studio_scenes.len(),
            });
        }
        let loop_clock = perf_capture.is_some().then(Instant::now);
        let mut loop_sections = LoopSections::default();
        if let (Some(now), Some(returned)) = (loop_clock, last_render_returned.take()) {
            loop_sections.post_render_prev_ms = now.saturating_duration_since(returned).as_secs_f64() * 1000.0;
        }
        let lap = |mark: &mut Option<Instant>| -> f64 {
            match mark {
                Some(previous) => {
                    let now = Instant::now();
                    let ms = now.saturating_duration_since(*previous).as_secs_f64() * 1000.0;
                    *previous = now;
                    ms
                }
                None => f64::NAN,
            }
        };
        let mut loop_mark = loop_clock;
        // HDR availability can change with Windows HDR or a display move,
        // including while running in SDR. Never query per frame.
        let hdr_output_changed = renderer.poll_hdr_display();
        let mut hdr_capabilities_changed = false;
        if hdr_capabilities_last_checked.elapsed() >= Duration::from_secs(2) {
            hdr_capabilities_last_checked = Instant::now();
            let caps = renderer.surface.get_capabilities(&renderer.adapter);
            let info = renderer.surface.display_hdr_info(&renderer.adapter);
            let available = super::hdr_output::available_modes(&caps, Some(&info));
            if available != hdr_available_modes {
                hdr_available_modes = available;
                hdr_capabilities_changed = true;
            }
        }
        if hdr_output_changed || hdr_capabilities_changed {
            let output = renderer.hdr_output.as_ref();
            let _ = proxy.send_event(UserEvent::HdrOutputChanged {
                status: output.map_or_else(|| "SDR".to_owned(), |o| o.status()),
                active: output.is_some_and(|o| matches!(o.selection.encoding, 1 | 2)),
                available_modes: hdr_available_modes,
            });
        }
        // Before any encoding, so the capture's first frames carry timestamps.
        let diag_recording = input_diagnostic.is_recording();
        if diag_recording != diag_forced_gpu_timings {
            diag_forced_gpu_timings = diag_recording;
            renderer.set_gpu_timings(diag_recording || user_gpu_timings);
        }
        let mut diag_command_drain_count = 0_u32;
        let diag_command_drain_ms: f64;
        // Only measured while presenting the first frame of a just-uploaded map.
        let first_world_command_drain_ms: f64;
        let mut first_world_command_count = 0_u32;
        let mut first_world_slowest_command_ms = 0.0_f64;
        let mut first_world_slowest_command_name = "none";
        // Checked before the command batch as well as before the frame: map and
        // texture uploads run from here, and on a dead device those fail the same
        // silent way, panicking somewhere inside wgpu instead of reporting.
        if crate::renderer::device_lost() {
            stop_for_device_loss(&proxy, renderer, companion_renderers);
            return;
        }
        loop_sections.top_ms = lap(&mut loop_mark);
        {
            let _activity = crate::thread_activity::activity(
                crate::thread_activity::ThreadSlot::Render,
                crate::thread_activity::Task::RenderCommands,
            );
            if let Some((request_id, started, name, map, studio_world)) = deferred_load_map.take() {
                let map: Box<PreparedMap> = map;
                let triangles = map.triangles;
                let batches = map.batches.len();
                let timings = map.load_timings;
                let bsp_stats = map.bsp_stats.clone();
                let upload_started = Instant::now();
                let received_at = load_map_received_at.take().unwrap_or(upload_started);
                let upload_timings = renderer.load_studio_map(*map, studio_world);
                let upload_ended_at = Instant::now();
                let upload_ms = upload_ended_at.duration_since(upload_started).as_secs_f64() * 1000.0;
                pending_first_world_phases = Some((started, received_at, upload_started, upload_ended_at));
                pending_world_uploaded = Some((
                    request_id, started, name, triangles, batches, upload_ms,
                    upload_timings, timings, bsp_stats,
                ));
            }
            if let Some((request_id, started, name, plan, base_texture_count, studio_world)) = deferred_remap_map.take() {
                let triangles = plan.base.triangles;
                let batches = plan.batches.len();
                let timings = plan.base.load_timings;
                let bsp_stats = plan.base.bsp_stats.clone();
                let upload_started = Instant::now();
                let upload_timings = renderer.load_remapped_map(*plan, base_texture_count, studio_world);
                let upload_ms = upload_started.elapsed().as_secs_f64() * 1000.0;
                pending_world_uploaded = Some((
                    request_id, started, name, triangles, batches, upload_ms,
                    upload_timings, timings, bsp_stats,
                ));
            }
            let diag_command_drain_started = input_diagnostic.is_recording().then(Instant::now);
            let first_world_command_drain_started =
                pending_world_uploaded.is_some().then(Instant::now);
            loop {
                let received = command_rx.try_recv();
                if received.is_ok() && !matches!(received.as_ref(), Ok(RenderCommand::SetCombatUi { .. })) {
                    // Some commands mutate state owned by the render-snapshot
                    // mailbox. A combat HUD update only touches UI fields and
                    // must not force cloning/reapplying the world snapshot.
                    applied_snapshot_generation = u64::MAX;
                }
                if diag_command_drain_started.is_some() && received.is_ok() {
                    diag_command_drain_count = diag_command_drain_count.saturating_add(1);
                }
                if first_world_command_drain_started.is_some() && received.is_ok() {
                    first_world_command_count = first_world_command_count.saturating_add(1);
                }
                let command_name = if first_world_command_drain_started.is_some() {
                    received.as_ref().map(menu_trace_command_name).unwrap_or("queue empty")
                } else {
                    ""
                };
                let command_started = Instant::now();
                match received {
                    Ok(RenderCommand::MemoryInfo) => renderer.print_memory_report(),
                    Ok(RenderCommand::StartPerfCapture { stem, frames, seconds }) => {
                        if let Some(mut sweep) = sky_sweep.take() {
                            sweep.restore(&mut renderer);
                            renderer.set_gpu_timings(user_gpu_timings || diag_forced_gpu_timings);
                            eprintln!("[SKY SWEEP] cancelled by render_perf_capture");
                        }
                        // Re-arming discards an incomplete run. Drop clears the
                        // global hook flag before the new capture enables it.
                        drop(perf_capture.take());
                        match seconds {
                            Some(seconds) => eprintln!("[RENDER PERF] armed {seconds:.1}s -> {} (2s delay + 120 warmup frames)",
                                stem.display()),
                            None => eprintln!("[RENDER PERF] armed {} samples -> {} (2s delay + 120 warmup frames)",
                                frames.clamp(30, 6_000), stem.display()),
                        }
                        perf_capture = Some(PerfCapture::arm(stem, frames, seconds, &renderer));
                    }
                    Ok(RenderCommand::TileCost(command)) => match command {
                        TileCostCommand::Start { stem, cols, rows, samples } => {
                            if let Some(mut sweep) = sky_sweep.take() { sweep.restore(&mut renderer); }
                            drop(perf_capture.take());
                            if let Some(probe) = tile_probe.take() { probe.restore(&mut renderer); }
                            renderer.tile_cost_overlay = None;
                            renderer.set_gpu_timings(true);
                            let probe = TileCostProbe::new(stem.clone(), cols, rows, samples, &mut renderer);
                            let (_, total) = probe.progress();
                            eprintln!("[TILE COST] armed {cols}x{rows} grid, {total} timings -> {} (2s delay; hold still)",
                                stem.display());
                            tile_probe = Some(probe);
                            tile_probe_reported_at = Instant::now();
                        }
                        TileCostCommand::Cancel => {
                            if let Some(probe) = tile_probe.take() {
                                probe.restore(&mut renderer);
                                renderer.set_gpu_timings(user_gpu_timings || diag_forced_gpu_timings);
                                eprintln!("[TILE COST] cancelled");
                            }
                        }
                        TileCostCommand::Hide => {
                            if let Some(overlay) = renderer.tile_cost_overlay.as_mut() { overlay.visible = false; }
                        }
                        TileCostCommand::Show(name) => match renderer.tile_cost_overlay.as_mut() {
                            None => eprintln!("[TILE COST] no finished report; run gputilecost first"),
                            Some(overlay) => {
                                if !parse_pass(&name).is_some_and(|pass| overlay.show(pass)) {
                                    eprintln!("[TILE COST] no data for '{name}'. Passes: {}", overlay.available_passes());
                                }
                            }
                        },
                        TileCostCommand::ListPasses => match renderer.tile_cost_overlay.as_ref() {
                            Some(overlay) => eprintln!("[TILE COST] passes: {}", overlay.available_passes()),
                            None => eprintln!("[TILE COST] no finished report; run gputilecost first"),
                        },
                    },
                    Ok(RenderCommand::StartSkyPerfSweep { stem, frames }) => {
                        if let Some(mut sweep) = sky_sweep.take() { sweep.restore(&mut renderer); }
                        drop(perf_capture.take());
                        renderer.set_gpu_timings(true);
                        eprintln!("[SKY SWEEP] starting six-phase Unified sky experiment -> {}", stem.display());
                        sky_sweep = Some(SkyPerfSweep::new(stem, frames, &mut renderer));
                    }
                    Ok(RenderCommand::BatchBegin(title)) => {
                        renderer.begin_settings_batch();
                        batch_timing = Some(RenderBatchTiming::new(title));
                    }
                    Ok(RenderCommand::BatchLabel(label)) => {
                        if let Some(batch) = batch_timing.as_mut() {
                            batch.current = label;
                        }
                    }
                    Ok(RenderCommand::BatchEnd) => {
                        let finish_started = Instant::now();
                        renderer.end_settings_batch();
                        if let Some(mut batch) = batch_timing.take() {
                            batch.current = "(deferred world pipeline variant)".to_owned();
                            batch.record(finish_started.elapsed());
                            batch.print();
                        }
                    }
                    Ok(RenderCommand::Resize(size)) => renderer.resize(size),
                    Ok(RenderCommand::SetVsync(vsync)) => {
                        if renderer.vsync != vsync {
                            renderer.set_vsync(vsync);
                            // An ON/FIFO frame can block for a refresh interval.
                            // Do not carry that old pacing deadline into OFF.
                            frame_pace_anchor = Instant::now();
                        }
                    }
                    Ok(RenderCommand::SetMaxFrameLatency(latency)) => {
                        renderer.set_max_frame_latency(latency)
                    }
                    Ok(RenderCommand::SetEarlySubmit(enabled)) => {
                        renderer.early_submit_enabled = enabled;
                    }
                    Ok(RenderCommand::SetPresentCopy(enabled)) => {
                        renderer.present_copy_enabled = enabled;
                    }
                    Ok(RenderCommand::SetInputLateLatch(enabled)) => {
                        input_latelatch = enabled;
                    }
                    Ok(RenderCommand::SetMsaa(samples)) => renderer.set_msaa(samples),
                    Ok(RenderCommand::SetTextureFilter(filter)) => {
                        renderer.set_texture_filter(filter)
                    }
                    Ok(RenderCommand::SetPicmip(picmip)) => renderer.set_picmip(picmip),
                    Ok(RenderCommand::SetTextureCompression(level)) => {
                        renderer.texture_compression = level.min(2);
                    }
                    Ok(RenderCommand::SetDetailTextures(mode)) => {
                        renderer.set_detail_textures(mode)
                    }
                    Ok(RenderCommand::SetDetailTexture { path, game }) => {
                        renderer.set_detail_texture(&path, game.as_deref())
                    }
                    Ok(RenderCommand::SetDetailTextureFade { enabled, distance }) => {
                        renderer.set_detail_texture_fade(enabled, distance)
                    }
                    Ok(RenderCommand::SetWireframeMask(mask)) => renderer.set_wireframe_mask(mask),
                    Ok(RenderCommand::SetShaderComplexity(enabled)) => renderer.shader_complexity_enabled = enabled,
                    Ok(RenderCommand::SetDebugVolumes { triggers, clips }) => {
                        renderer.debug_volumes.triggers_enabled = triggers;
                        renderer.debug_volumes.clips_enabled = clips;
                    }
                    Ok(RenderCommand::SetEntityMarkers(mesh)) => {
                        renderer.set_entity_markers(mesh);
                    }
                    Ok(RenderCommand::SetCoastalDebugMesh(mesh)) => {
                        renderer.set_coastal_debug_mesh(mesh);
                    }
                    Ok(RenderCommand::SetCoastalWater { enabled, pool, atlas, upstream_source }) => {
                        renderer.coastal_enabled = enabled;
                        renderer.coastal_pool = pool;
                        renderer.coastal_atlas = atlas;
                        renderer.upstream_shore_source = upstream_source;
                    }
                    Ok(RenderCommand::SetSunRayPreview { enabled, debug_mode }) => {
                        renderer.debug_volumes.sun_ray_enabled = enabled;
                        renderer.debug_volumes.sun_ray_authored = debug_mode != 0;
                        renderer.baked_sun_debug_mode = if enabled && matches!(debug_mode, 1 | 2) {
                            u32::from(debug_mode)
                        } else {
                            0
                        };
                        renderer.update_lighting_settings();
                        renderer.rebuild_frame_plan();
                        renderer.sync_runtime_variant_flags();
                    }
                    Ok(RenderCommand::SetPvsMode(mode)) => renderer.set_pvs_mode(mode),
                    Ok(RenderCommand::SetFacePlaneCull(enabled)) => renderer.set_face_plane_cull(enabled),
                    Ok(RenderCommand::SetDrawInlineModels(enabled)) => renderer.set_draw_inline_models(enabled),
                    Ok(RenderCommand::SetInlineTransformMode(mode)) => renderer.set_inline_transform_mode(mode),
                    Ok(RenderCommand::SetFragmentShaderMode(mode)) => renderer.set_fragment_shader_mode(mode),
                    Ok(RenderCommand::SetWorldSortMode(mode)) => renderer.world_sort_mode = mode.min(1),
                    Ok(RenderCommand::SetPostSpecialize(enabled)) => renderer.post_specialize = enabled,
                    Ok(RenderCommand::SetColorClearMode(mode)) => renderer.color_clear_mode = mode.min(1),
                    Ok(RenderCommand::SetWorldStageDebug { draw_bsp_model_surfaces, probe }) => {
                        renderer.set_world_stage_debug(draw_bsp_model_surfaces, probe);
                    },
                    Ok(RenderCommand::SetFastSky(mode)) => renderer.set_fast_sky(mode),
                    Ok(RenderCommand::SetWorldFlares(enabled)) => {
                        if renderer.world_flares_enabled != enabled {
                            renderer.world_flares.invalidate_history();
                            renderer.world_flares_sky.invalidate_history();
                            for flare_view in &mut renderer.world_flares_planar {
                                flare_view.invalidate_history();
                            }
                        }
                        renderer.world_flares_enabled = enabled;
                    }
                    Ok(RenderCommand::SetFpsCap(cap)) => {
                        if fps_cap != cap {
                            fps_cap = cap;
                            frame_pace_anchor = Instant::now();
                        }
                    },
                    Ok(RenderCommand::SetGamma(gamma)) => renderer.set_gamma(gamma),
                    Ok(RenderCommand::SetGammaMethod(method)) => {
                        gamma_preparation = None;
                        renderer.set_gamma_method(method);
                    }
                    Ok(RenderCommand::SetHardwareGammaActive(active)) => {
                        renderer.hardware_gamma_active = active;
                    }
                    Ok(RenderCommand::PrepareGammaMethod(generation)) => {
                        renderer.set_gamma_method(crate::gamma::GammaMethod::Hardware);
                        gamma_preparation = Some(generation);
                    }
                    Ok(RenderCommand::SetModelBrightness(scale)) => {
                        renderer.set_model_brightness(scale)
                    }
                    Ok(RenderCommand::SetDynamicLightBrightness(scale)) => {
                        renderer.set_dynamic_light_brightness(scale)
                    }
                    Ok(RenderCommand::SetPostEffects(settings)) => {
                        renderer.set_post_effects(settings)
                    }
                    Ok(RenderCommand::SetFootprintMode(mode)) => {
                        renderer.surface_deformation.set_mode(&renderer.queue, mode)
                    }
                    Ok(RenderCommand::SetGrassEnabled(enabled)) => {
                        renderer.set_grass_enabled(enabled)
                    }
                    Ok(RenderCommand::SetGrassPrecompute(enabled)) => {
                        renderer.set_grass_precompute(enabled)
                    }
                    Ok(RenderCommand::SetGrassMidLod(enabled)) => {
                        renderer.set_grass_mid_lod(enabled)
                    }
                    Ok(RenderCommand::SetGrassFrontToBack(enabled)) => {
                        renderer.set_grass_front_to_back(enabled)
                    }
                    Ok(RenderCommand::SetRainOptimizations {
                        fast_post,
                        water_field,
                        fast_vertex,
                        lean_depth,
                        static_depth,
                        opaque_depth,
                        lean_models,
                        lean_post,
                    }) => renderer.set_rain_optimizations(
                        fast_post,
                        water_field,
                        fast_vertex,
                        lean_depth,
                        static_depth,
                        opaque_depth,
                        lean_models,
                        lean_post,
                    ),
                    Ok(RenderCommand::SetContactShadowDebug(enabled)) => {
                        renderer.contact_shadow_debug = enabled
                    }
                    Ok(RenderCommand::SetSsrDebug(mode)) => renderer.ssr_debug = mode,
                    Ok(RenderCommand::SetOceanEnabled(enabled)) => {
                        renderer.set_ocean_enabled(enabled)
                    }
                    Ok(RenderCommand::SetOceanSkyExtend(enabled)) => {
                        renderer.set_ocean_sky_extend(enabled)
                    }
                    Ok(RenderCommand::SetOceanSettings(settings)) => {
                        renderer.set_ocean_settings(settings)
                    }
                    Ok(RenderCommand::OceanWake(command)) => {
                        if let Some(ocean) = &mut renderer.ocean {
                            ocean.blue_wake_command(command);
                        }
                    }
                    Ok(RenderCommand::SetPromotedOceanSettings(settings)) => {
                        renderer.promoted_ocean_overrides = settings;
                    }
                    Ok(RenderCommand::SetAuthoredOceans(oceans)) => {
                        renderer.set_authored_oceans(oceans)
                    }
                    Ok(RenderCommand::SetOceanTime(time)) => {
                        if let Some(ocean) = &mut renderer.ocean {
                            ocean.set_time(time);
                        }
                        for (_, ocean) in &mut renderer.authored_oceans {
                            ocean.set_time(time);
                        }
                    }
                    Ok(RenderCommand::SetPerfTrace(enabled)) => {
                        perf_trace = enabled;
                        trace_started = Instant::now();
                        trace_frames = 0;
                        trace_frame_ms = 0.0;
                        trace_prepare_ms = 0.0;
                        trace_acquire_ms = 0.0;
                        trace_encode_ms = 0.0;
                        trace_submit_ms = 0.0;
                        trace_present_ms = 0.0;
                        trace_grass_cpu_ms = 0.0;
                        trace_surface_sprite_cpu_ms = 0.0;
                        trace_ocean_topology_ms=0.0;
                        trace_ocean_topology_max_ms=0.0;
                        trace_prep_stage_ms = [0.0; 4];
                        println!(
                            "JKA perf trace {}",
                            if enabled { "enabled" } else { "disabled" }
                        );
                    }
                    Ok(RenderCommand::SetFrameTimeGraph(enabled)) => {
                        renderer.frame_time_graph.set_enabled(enabled);
                    }
                    Ok(RenderCommand::SetGpuTimings(enabled)) => {
                        user_gpu_timings = enabled;
                        // A capture owns the profiler until it finishes; the new
                        // preference is applied when it restores.
                        if !diag_forced_gpu_timings && sky_sweep.is_none() && tile_probe.is_none() {
                            renderer.set_gpu_timings(enabled);
                        }
                    }
                    Ok(RenderCommand::SetFxZeroAlphaDiscard(enabled)) => {
                        renderer
                            .dynamic_model_renderer
                            .set_fx_zero_alpha_discard(enabled);
                    }
                    Ok(RenderCommand::SetDebugWorldVertexAlphaOne(enabled)) => {
                        renderer.set_debug_world_vertex_alpha_one(enabled);
                    }
                    Ok(RenderCommand::SetGhoul2BatchDraws(mode)) => {
                        renderer.set_ghoul2_batch_draws(mode)
                    }
                    Ok(RenderCommand::SetModelStateCache(enabled)) => {
                        renderer.dynamic_model_renderer.model_state_cache = enabled;
                    }
                    Ok(RenderCommand::SetDynamicBatchDraws(enabled)) => {
                        renderer.dynamic_model_renderer.dynamic_batch_draws = enabled;
                    }
                    Ok(RenderCommand::SetBspModelLod { scale, bias, debug, enabled }) => {
                        renderer.bsp_model_lod_scale = scale;
                        renderer.bsp_model_lod_bias = bias;
                        renderer.bsp_model_lod_debug = debug;
                        renderer.bsp_model_lod_enabled = enabled;
                    }
                    Ok(RenderCommand::SetPatchLod { curve_error, no_curves, speeds }) => {
                        renderer.patch_curve_error = curve_error;
                        renderer.patch_no_curves = no_curves;
                        renderer.patch_speeds = speeds;
                    }
                    Ok(RenderCommand::SetWorldCompactMinDraws(minimum)) => {
                        renderer.world_compact_min_draws = minimum.clamp(1, 2);
                    }
                    Ok(RenderCommand::SetModelPrepareThreads(threads)) => {
                        if let Err(error) = renderer.dynamic_model_renderer.set_model_prepare_threads(threads) {
                            eprintln!("Model packing workers: {error}");
                        }
                    }
                    Ok(RenderCommand::SetEncoderFinishThreads(mode)) => {
                        if let Err(error) = renderer.set_encoder_finish_threads(mode) {
                            eprintln!("Encoder finish worker: {error}");
                        }
                    }
                    Ok(RenderCommand::SetModelRenderBundles(enabled)) => {
                        renderer.dynamic_model_renderer.set_model_render_bundles(enabled);
                    }
                    Ok(RenderCommand::SetSurfaceSpritesGpu(enabled)) => {
                        renderer.surface_sprite_effect_renderer.resident_enabled = enabled;
                    }
                    Ok(RenderCommand::SetSurfaceSprites(enabled)) => {
                        renderer.surface_sprite_effect_renderer.enabled = enabled;
                    }
                    Ok(RenderCommand::SetMd3GeometryCache(enabled)) => {
                        renderer.dynamic_model_renderer.set_md3_geometry_cache(enabled);
                    }
                    Ok(RenderCommand::SetModelIndexUploadCache(enabled)) => {
                        renderer.dynamic_model_renderer.set_model_index_upload_cache(enabled);
                    }
                    Ok(RenderCommand::SetGhoul2BatchKeyCache(enabled)) => {
                        renderer.dynamic_model_renderer.ghoul2_batch_key_cache = enabled;
                    }
                    Ok(RenderCommand::SetGpuVisibility {
                        gpu_driven,
                        hiz_occlusion,
                    }) => {
                        renderer.set_gpu_visibility(gpu_driven, hiz_occlusion);
                    }
                    Ok(RenderCommand::SetDynamicLighting(mode)) => {
                        renderer.set_dynamic_lighting(mode);
                    }
                    Ok(RenderCommand::SetDynamicLightDebug(enabled)) => {
                        renderer.set_dynamic_light_debug(enabled);
                    }
                    Ok(RenderCommand::SetRtSamples(samples)) => renderer.set_rt_samples(samples),
                    Ok(RenderCommand::SetDynamicLightFalloff(mode)) => {
                        renderer.set_dynamic_light_falloff(mode)
                    }
                    Ok(RenderCommand::SetRtTraceRate(rate)) => {
                        renderer.set_rt_trace_rate(rate)
                    }
                    Ok(RenderCommand::SetMapLightSimulation(enabled)) => {
                        renderer.set_map_light_simulation(enabled);
                    }
                    Ok(RenderCommand::SetClassicWorldLighting {
                        fullbright,
                        vertex_light,
                        lightmap_only,
                    }) => {
                        renderer.set_classic_world_lighting(
                            fullbright,
                            vertex_light,
                            lightmap_only,
                        );
                    }
                    Ok(RenderCommand::SetAddedMapLightSources(mask)) => {
                        renderer.set_added_map_light_sources(mask);
                    }
                    Ok(RenderCommand::SetLampShadowSources(sources)) => {
                        renderer.set_lamp_shadow_sources(sources);
                    }
                    Ok(RenderCommand::SetCsmCoverageAB(enabled)) => {
                        renderer.set_csm_coverage_ab(enabled);
                    }
                    Ok(RenderCommand::SetLampShadowAB(mode)) => {
                        renderer.set_lamp_shadow_ab(mode);
                    }
                    Ok(RenderCommand::SetEntityFixtureLightingAB(enabled)) => {
                        renderer.set_entity_fixture_lighting_ab(enabled);
                    }
                    Ok(RenderCommand::SetEntityAmbientLighting(mode)) => {
                        renderer.set_entity_ambient_lighting(mode);
                    }
                    Ok(RenderCommand::SetVoxelProbeGi(enabled)) => {
                        renderer.set_voxel_probe_gi(enabled);
                    }
                    Ok(RenderCommand::SetLocalLightShadows(enabled)) => {
                        renderer.set_local_light_shadows(enabled);
                    }
                    Ok(RenderCommand::SetTintedGlass(enabled)) => {
                        renderer.set_tinted_glass(enabled);
                    }
                    Ok(RenderCommand::SetTintedGlassDebug(mode)) => {
                        renderer.set_tinted_glass_debug(mode);
                    }
                    Ok(RenderCommand::SetEntityShadowLight(source)) => {
                        renderer.set_entity_shadow_light(source);
                    }
                    Ok(RenderCommand::SetPbrSettings {
                        enabled,
                        deluxe_mapping,
                        deluxe_specular,
                    }) => {
                        renderer.set_pbr_settings(enabled, deluxe_mapping, deluxe_specular);
                    }
                    Ok(RenderCommand::SetCascadedShadows(mode)) => {
                        renderer.set_cascaded_shadows(mode);
                    }
                    Ok(RenderCommand::SetCullDebug(mode)) => {
                        renderer.set_cull_debug(mode);
                    }
                    Ok(RenderCommand::SetFoliageWind(enabled)) => {
                        renderer.foliage_wind_enabled = enabled;
                    }
                    Ok(RenderCommand::SetForceUnifiedWorld(force)) => {
                        renderer.set_force_unified_world(force);
                    }
                    Ok(RenderCommand::SetPom(enabled)) => {
                        renderer.set_pom_enabled(enabled);
                    }
                    Ok(RenderCommand::SetPlanarReflectionDebug(mode)) => {
                        renderer.set_planar_reflection_debug(mode);
                    }
                    Ok(RenderCommand::SetPuddleDebug(mode)) => {
                        renderer.set_puddle_debug(mode);
                    }
                    Ok(RenderCommand::SetPuddleSkyBlend { window, ceiling }) => {
                        renderer.set_puddle_sky_blend(window, ceiling);
                    }
                    Ok(RenderCommand::DumpMaterials { filter, all }) => {
                        renderer.dump_materials(filter.as_deref(), all);
                    }
                    Ok(RenderCommand::InspectSurface {
                        x,
                        y,
                        width,
                        height,
                        entity_hint,
                    }) => {
                        let info = renderer.inspect_surface(
                            &snapshot.camera,
                            snapshot.dynamic_models.as_slice(),
                            x,
                            y,
                            width,
                            height,
                            entity_hint,
                        );
                        let _ = proxy.send_event(UserEvent::SurfaceInspected(info));
                    }
                    Ok(RenderCommand::HybridWaveDump { path }) => {
                        renderer.write_hybrid_wave_dump(&path);
                    }
                    Ok(RenderCommand::ClearSurfaceInspection) => {
                        renderer.inspector_vertex_range = None;
                        renderer.inspector_entity_num = None;
                    }
                    Ok(RenderCommand::AddSurfaceDeformation(stamp)) => {
                        renderer.surface_deformation.push(&renderer.queue, stamp);
                    }
                    Ok(RenderCommand::Screenshot {
                        directory,
                        mut metadata,
                        format,
                    }) => {
                        // Use the renderer-owned snapshot as authority for the camera fields
                        // instead of the older main-thread copy bundled with the command.
                        metadata.camera_origin = Some(crate::scene::jka_position(
                            snapshot.camera.position.to_array(),
                        ));
                        metadata.view_angles = Some([
                            -snapshot.camera.pitch.to_degrees(),
                            snapshot.camera.yaw.to_degrees().rem_euclid(360.0),
                            0.0,
                        ]);
                        metadata.fov = snapshot.camera.cg_fov();
                        // Capture what the center ray is actually rendering on the same
                        // renderer snapshot as the screenshot. Preserve `/trace`'s
                        // selection/highlight state: screenshot metadata is observational.
                        let old_range = renderer.inspector_vertex_range.clone();
                        let old_entity = renderer.inspector_entity_num;
                        renderer.inspector_vertex_range = None;
                        renderer.inspector_entity_num = None;
                        let width = renderer.config.width.max(1);
                        let height = renderer.config.height.max(1);
                        let hit = renderer.inspect_surface(
                            &snapshot.camera,
                            snapshot.dynamic_models.as_slice(),
                            width as f32 * 0.5,
                            height as f32 * 0.5,
                            width,
                            height,
                            None,
                        );
                        renderer.inspector_vertex_range = old_range;
                        renderer.inspector_entity_num = old_entity;
                        metadata.crosshair = hit.map(|info| {
                            let material = info
                                .summary
                                .iter()
                                .find(|(label, _)| label.eq_ignore_ascii_case("MATERIAL"))
                                .map(|(_, value)| value.clone());
                            let distance = info
                                .summary
                                .iter()
                                .find(|(label, _)| label.eq_ignore_ascii_case("DISTANCE"))
                                .map(|(_, value)| value.clone());
                            crate::screenshot::ScreenshotCrosshair {
                                kind: info.kind,
                                title: info.title,
                                material,
                                distance,
                                entity_num: info.hit_entity_num,
                            }
                        });
                        renderer.request_screenshot(directory, metadata, format)
                    }
                    Ok(RenderCommand::CopyFrameToClipboard) => renderer.request_clipboard_capture(),
                    Ok(RenderCommand::CaptureFrontendScene { path, neutral_gamma }) => renderer.request_frontend_scene_capture(path, neutral_gamma),
                    Ok(RenderCommand::CaptureVideoFrame { frame_index, depth, alpha }) => renderer.request_video_frame_capture(frame_index, depth, alpha),
                    Ok(RenderCommand::SetUi(ui)) => renderer.set_ui(ui),
                    Ok(RenderCommand::SetStreamerSettings(settings)) => {
                        renderer.stream_output.set_settings(settings);
                        if settings.enabled && (renderer.studio_world_viewport.is_some() || renderer.studio_export_size.is_some()) {
                            renderer.stream_output.pause_unsupported("Spout is disabled in Render Studio");
                        }
                    }
                    Ok(RenderCommand::SetCombatUi { zoom_scope, rocket_lock }) =>
                        renderer.set_combat_ui(zoom_scope, rocket_lock),
                    Ok(RenderCommand::SetNeonMenu(frame)) => renderer.set_neon_menu(frame),
                    Ok(RenderCommand::SetNeonChrome(frame)) => renderer.set_neon_chrome(frame),
                    Ok(RenderCommand::SetNeonMenuCache(image)) => renderer.set_neon_menu_cache(image),
                    Ok(RenderCommand::SetExtensionHud(widgets, hidden, images)) => renderer.set_extension_hud(widgets, hidden, images),
                    Ok(RenderCommand::SetTransientUi {
                        chat_lines,
                        console_notify_lines,
                        center_print,
                        demo_timeline,
                        prediction_debug,
                        crosshair_target,
                        force_select,
                        weapon_select,
                        inventory_select,
                        follow_name,
                        game_timer,
                        mini_scores,
                        hud_score,
                        enemy_info,
                        race_timer,
                        vote_line,
                        scoreboard,
                        scoreboard_focus_client,
                        killfeed,
                        chat_anim,
                        speedometer,
                        lagometer,
                        team_overlay,
                    }) => {
                        renderer.set_transient_ui(
                            chat_lines,
                            console_notify_lines,
                            center_print,
                            demo_timeline,
                            prediction_debug,
                            crosshair_target,
                            force_select,
                            weapon_select,
                            inventory_select,
                            follow_name,
                            game_timer,
                            mini_scores,
                        hud_score,
                        enemy_info,
                            race_timer,
                            vote_line,
                            scoreboard,
                            scoreboard_focus_client,
                            killfeed,
                            chat_anim,
                            speedometer,
                            lagometer,
                            team_overlay,
                        );
                    }
                    Ok(RenderCommand::SetUiTelemetry { perf, threads }) => {
                        renderer.set_ui_telemetry(perf, threads);
                    }
                    Ok(RenderCommand::SetEgui(frame)) => renderer.set_egui(frame),
                    Ok(RenderCommand::CreateCompanion {
                        id,
                        window,
                        size,
                        surface,
                    }) => {
                        if let Some(mut old) = companion_renderers.remove(&id) {
                            old.shutdown();
                        }
                        renderer.remove_companion_scene_target(id);
                        match CompanionRenderThread::spawn(
                            id,
                            window,
                            size,
                            surface,
                            renderer.adapter.clone(),
                            renderer.device.clone(),
                            renderer.queue.clone(),
                            proxy.clone(),
                        ) {
                            Ok(worker) => {
                                companion_renderers.insert(id, worker);
                            }
                            Err(error) => {
                                let _ =
                                    proxy.send_event(UserEvent::CompanionError { id: id.0, error });
                            }
                        }
                    }
                    Ok(RenderCommand::DestroyCompanion { id }) => {
                        renderer.remove_companion_scene_target(id);
                        if let Some(mut worker) = companion_renderers.remove(&id) {
                            worker.shutdown();
                        }
                    }
                    Ok(RenderCommand::ResizeCompanion { id, size }) => {
                        if let Some(worker) = companion_renderers.get(&id) {
                            worker.resize(size);
                        }
                    }
                    Ok(RenderCommand::SetCompanionEgui { id, frame }) => {
                        if let Some(worker) = companion_renderers.get(&id) {
                            worker.publish(frame);
                        }
                    }
                    Ok(RenderCommand::SetAssetPreviewMode(enabled)) => {
                        if enabled { renderer.park_inactive_studio_views(&[]); }
                        renderer.set_asset_preview_mode(enabled);
                    }
                    Ok(RenderCommand::SetAssetPreviewViewport(viewport)) => {
                        renderer.asset_preview_viewport = viewport;
                    }
                    Ok(RenderCommand::SetStudioWorldViewport(viewport)) => {
                        renderer.studio_world_viewport = viewport;
                        if viewport.is_some() && renderer.stream_output.enabled() {
                            // Studio bypasses the gameplay draw path. Stop an
                            // existing game sender here, before its next frame.
                            renderer.stream_output.pause_unsupported("Spout is disabled in Render Studio");
                        }
                        if viewport.is_none() {
                            renderer.studio_transition_gpu = None;
                        }
                    }
                    Ok(RenderCommand::SetStudioExportSize(size)) => {
                        renderer.studio_export_size = size;
                        if size.is_some() && renderer.stream_output.enabled() {
                            renderer.stream_output.pause_unsupported("Spout is disabled in Render Studio");
                        }
                        if size.is_none() {
                            renderer.studio_export_target = None;
                            renderer.studio_export_preview_at = None;
                            renderer.studio_export_shown = None;
                            // A cancelled export must not deliver stale frames.
                            // Dropping the slots also drops any buffer still mapping.
                            renderer.studio_capture_queue.clear();
                            renderer.studio_readbacks.clear();
                            renderer.studio_readback_slots.clear();
                            renderer.studio_capture_results.clear();
                            renderer.studio_unpacking.clear();
                            renderer.studio_readback_generation += 1;
                        }
                    }
                    Ok(RenderCommand::CaptureStudioFrame { frame_index, depth, scene }) => {
                        renderer.queue_studio_capture(frame_index, depth, scene);
                    }
                    Ok(RenderCommand::LoadStudioSource {id,game,map,video,progress}) => {
                        // Release an old instance before allocating its replacement.
                        renderer.studio_scenes.remove(&id);
                        renderer.studio_last_scene = None;
                        for load in &mut studio_scene_loads {
                            if load.id == id { load.cancelled = true; }
                        }
                        studio_scene_loads.push(StudioSceneLoad { id, game, map: Some(map), video, progress, worker: None, cancelled: false });
                    }
                    Ok(RenderCommand::ReleaseStudioSources) => {
                        renderer.studio_scenes.clear();
                        renderer.studio_last_scene = None;
                        for load in &mut studio_scene_loads { load.cancelled = true; }
                    }
                    Ok(RenderCommand::RemoveStudioSource(id)) => {
                        renderer.studio_scenes.remove(&id);
                        renderer.studio_last_scene = None;
                        for load in &mut studio_scene_loads { if load.id == id { load.cancelled = true; } }
                    }
                    Ok(RenderCommand::SetAssetPreviewCheckerboard)=>{
                        renderer.asset_preview_background=super::frame::preview_background::PreviewBackground::checkerboard(
                            &renderer.device,&renderer.queue,renderer.config.format,renderer.msaa_samples).ok();
                    }
                    Ok(RenderCommand::SetAssetPreviewBackground(image))=>{
                        renderer.asset_preview_background=image.and_then(|(size,rgba)|
                            match super::frame::preview_background::PreviewBackground::new(&renderer.device,&renderer.queue,renderer.config.format,renderer.msaa_samples,size,&rgba){
                                Ok(background)=>Some(background),Err(error)=>{eprintln!("Profile background: {error}");None}
                            });
                    }
                    Ok(RenderCommand::UnloadMap) => renderer.unload_map(),
                    Ok(RenderCommand::GrowInlineInstances { map }) => {
                        renderer.load_map(*map);
                    }
                    Ok(RenderCommand::LoadMap {
                        request_id,
                        started,
                        name,
                        map,
                        studio_world,
                    }) => {
                        // The upload blocks this thread for seconds, and the loading
                        // screen is drawn by this same thread. Hold the map back,
                        // stop draining so command order is kept, and let one frame
                        // present the latest loading state ("uploading") first; the
                        // upload itself runs at the top of the next iteration.
                        load_map_received_at = Some(Instant::now());
                        deferred_load_map = Some((request_id, started, name, map, studio_world));
                        break;
                    }
                    Ok(RenderCommand::RemapMap { request_id, started, name, plan, base_texture_count, studio_world }) => {
                        deferred_remap_map = Some((request_id, started, name, plan, base_texture_count, studio_world));
                        break;
                    }
                    Ok(RenderCommand::Shutdown) => {
                        let teardown_started = Instant::now();
                        rverbose!(1, "[RENDER SHUTDOWN] command received; dropping renderer");
                        // Swapchain/device teardown can wedge or fault inside the
                        // GPU driver. If it is still running after a few seconds,
                        // log it and dump every thread's stack while it is stuck.
                        let (teardown_done, teardown_wait) = mpsc::channel::<()>();
                        let _ = thread::Builder::new()
                            .name("render-teardown-watchdog".into())
                            .spawn(move || {
                                if teardown_wait.recv_timeout(Duration::from_secs(4))
                                    == Err(mpsc::RecvTimeoutError::Timeout)
                                {
                                    eprintln!(
                                        "[RENDER SHUTDOWN] renderer drop still running after 4 s (GPU driver hang?)"
                                    );
                                    crate::crash::write_hang_dump("hang-render-shutdown");
                                }
                            });
                        for (_, mut companion) in companion_renderers.drain() {
                            companion.shutdown();
                        }
                        crate::pipeline_cache::shutdown();
                        drop(renderer);
                        let _ = teardown_done.send(());
                        rverbose!(
                            1,
                            "[RENDER SHUTDOWN] renderer dropped {:.1} ms",
                            teardown_started.elapsed().as_secs_f64() * 1000.0
                        );
                        return;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        rverbose!(
                            1,
                            "[RENDER SHUTDOWN] command channel closed; render thread exiting"
                        );
                        for (_, mut companion) in companion_renderers.drain() {
                            companion.shutdown();
                        }
                        return;
                    }
                }
                if first_world_command_drain_started.is_some() {
                    let duration_ms = command_started.elapsed().as_secs_f64() * 1000.0;
                    if duration_ms > first_world_slowest_command_ms {
                        first_world_slowest_command_ms = duration_ms;
                        first_world_slowest_command_name = command_name;
                    }
                }
                if let Some(batch) = batch_timing.as_mut() {
                    batch.record(command_started.elapsed());
                }
            }
            first_world_command_drain_ms = first_world_command_drain_started
                .map(|started| started.elapsed().as_secs_f64() * 1000.0)
                .unwrap_or(0.0);
            diag_command_drain_ms = diag_command_drain_started
                .map(|started| started.elapsed().as_secs_f64() * 1000.0)
                .unwrap_or(0.0);
            loop_sections.drain_ms = lap(&mut loop_mark);
            if let Some((completed, total)) = renderer.poll_static_ao_progress() {
                let _ = proxy.send_event(UserEvent::StaticAoProgress { completed, total });
            }
            renderer.poll_static_ao_updates();
            renderer.poll_static_ao_worker();
            let next = {
                let latest = match latest_snapshot.lock() {
                    Ok(latest) => latest,
                    Err(poisoned) => poisoned.into_inner(),
                };
                (latest.generation != applied_snapshot_generation).then(|| {
                    applied_snapshot_generation = latest.generation;
                    latest.snapshot.clone()
                })
            };
            // No new snapshot and no command since the last frame: everything
            // below would recompute the same state (and set_screen_fx would
            // re-upload identical vertices).
            if let Some(next) = next {
                let position_delta = next.camera.position - snapshot.camera.position;
                let yaw_delta = (next.camera.yaw - snapshot.camera.yaw).abs();
                let pitch_delta = (next.camera.pitch - snapshot.camera.pitch).abs();
                let camera_changed = position_delta.length_squared() > 1.0e-10
                    || yaw_delta > 1.0e-7
                    || pitch_delta > 1.0e-7;
                if camera_changed {
                    let now = Instant::now();
                    let elapsed = now.duration_since(motion_camera_changed_at).as_secs_f32();
                    // Large position discontinuities are camera cuts/teleports, not shutter
                    // motion. Reset the reference instead of smearing the whole image.
                    let camera_cut = position_delta.length_squared() > 256.0 * 256.0;
                    if camera_cut {
                        motion_reference_camera = next.camera;
                        motion_camera_sample_dt = 1.0 / 120.0;
                    } else {
                        // After an idle period, don't interpret the idle time as the camera's
                        // sample interval. The first new input sample should still blur.
                        motion_camera_sample_dt = if elapsed > 0.0 && elapsed <= 0.05 {
                            elapsed.max(1.0 / 10_000.0)
                        } else {
                            1.0 / 120.0
                        };
                        motion_reference_camera = snapshot.camera;
                    }
                    motion_camera_changed_at = now;
                }
                // HUD input/velocity/health are mailbox state just like the camera:
                // update only the tiny hot vertex buffer when they actually change.
                renderer.set_dynamic_hud(next.hud, next.movement_hud);
                renderer.set_player_names(next.player_names.clone());
                renderer.set_jump_shade(next.jump_shade);
                renderer.set_screen_fx(&next.screen_fx);
                renderer.set_cloud_foreground(&next.cloud_foreground);
                snapshot = next;
            }
            loop_sections.mailbox_ms = lap(&mut loop_mark);
        }
        if renderer.size.width == 0 || renderer.size.height == 0 {
            thread::sleep(Duration::from_millis(8));
            continue;
        }

        // Every settings command for this frame has been drained, so a burst that
        // asked for the scene rebuild several times over pays for it once.
        let pre_started = loop_mark;
        let first_world_rebuild_started = pending_world_uploaded.is_some().then(Instant::now);
        renderer.apply_pending_scene_rebuild();
        let mut pre_mark = pre_started;
        loop_sections.pre_scene_rebuild_ms = lap(&mut pre_mark);
        let first_world_rebuild_ms = first_world_rebuild_started
            .map(|started| started.elapsed().as_secs_f64() * 1000.0)
            .unwrap_or(0.0);

        let first_world_inline_started = pending_world_uploaded.is_some().then(Instant::now);
        renderer.update_inline_models(snapshot.inline_models.as_deref().map(Vec::as_slice));
        loop_sections.pre_inline_models_ms = lap(&mut pre_mark);
        let first_world_inline_ms = first_world_inline_started
            .map(|started| started.elapsed().as_secs_f64() * 1000.0)
            .unwrap_or(0.0);

        if let Some(scene) = snapshot.companion_scene.as_ref() {
            if let Some(worker) = companion_renderers.get(&scene.id) {
                if let Some(sources) = renderer.ensure_companion_scene_target(
                    scene.id,
                    worker.size(),
                    worker.scene_consumed(),
                ) {
                    worker.set_scene_sources(sources);
                }
            }
        }

        let frame_loop_started = Instant::now();
        renderer.frame_time_graph.sample(
            frame_loop_started,
            previous_graph_frame_complete.then_some(&last_info),
            renderer.gpu_profiler.latest,
            &renderer.device, &renderer.queue,
            renderer.size.width, renderer.size.height,
            renderer.world.as_ref(),
            renderer.ocean_visibility.latest_samples,
        );
        previous_graph_frame_complete = false;
        if let Some((_, _, _, uploaded_at)) = pending_first_world_phases.as_ref() {
            let post_upload_ms = frame_loop_started
                .saturating_duration_since(*uploaded_at).as_secs_f64() * 1000.0;
            let measured_ms = first_world_command_drain_ms
                + first_world_rebuild_ms + first_world_inline_ms;
            rverbose!(2, "[MAP RENDER SETTINGS] sky backend={} | clouds enabled={} backend={} | sun override={} | ocean enabled={} backend={}",
                renderer.sky_backend, renderer.clouds_enabled, renderer.cloud_backend,
                renderer.sun_override, renderer.ocean_enabled, renderer.ocean_settings.backend);
            rverbose!(2, "[MAP PRE-RENDER] post-upload {:.1} ms | command drain {:.1} ms ({} commands, slowest {:.1}: {}) | scene rebuild {:.1} | inline models {:.1} | remaining {:.1} ms (other loop work)",
                post_upload_ms, first_world_command_drain_ms, first_world_command_count,
                first_world_slowest_command_ms, first_world_slowest_command_name, first_world_rebuild_ms, first_world_inline_ms,
                (post_upload_ms - measured_ms).max(0.0));
        }
        let diag_frame_start_input = input_diagnostic
            .is_recording()
            .then(|| latest_view.load().input)
            .flatten();
        let mut diag_present_latest_input = None;
        let mut diag_rendered = false;
        // Permanent subframe input samples the same lock-free latest-view mailbox
        // at frame start. `cl_input_latelatch 1` defers that exact sample until
        // the renderer's last coherent camera point, making the A/B comparison
        // about latch timing rather than about two different input pipelines.
        let mut frame_camera = snapshot.camera;
        if let Some(shake) = snapshot.camera_shake {
            let now = Instant::now();
            if let Some(offset) = shake.sample(now, &mut shake_rng) {
                frame_camera.shake = offset;
                frame_camera.apply_shake();
            }
        }
        let frame_start_view_sample = (!input_latelatch)
            .then(|| {
                sample_view_rotation(&mut frame_camera, latest_view.as_ref(), snapshot.view_latch)
            })
            .flatten();
        let frame_crosshair_world = frame_start_view_sample
            .map(|measurement| measurement.crosshair_world)
            .unwrap_or(snapshot.dynamic_crosshair_world);
        let input_latency_sample = frame_start_view_sample
            .map(|measurement| measurement.input)
            .or(snapshot.input_latency)
            .filter(|sample| sample.sequence != last_measured_input_sequence);
        // Existing per-pass CPU counters are opt-in. Enable just for this one
        // first-world frame, so first-use pipeline and ocean costs can be located
        // without a 2s + 120-frame warmed-up performance capture.
        let first_world_profile = pending_first_world_phases.is_some();
        if first_world_profile {
            crate::renderer::diagnostics::perf_capture::set_first_world_frame_trace(true);
        }
        let render_result = {
            let mut model_source = snapshot.model_frame_source;
            if let Some(source) = model_source.as_mut() {
                source.raster_surfaces = snapshot
                    .dynamic_models
                    .iter()
                    .filter(|surface| surface.entity_num == source.entity && surface.raster_visible)
                    .count();
            }
            renderer.model_frame_log.set_source(model_source);
            renderer.studio_lens=snapshot.studio_lens;
            renderer.ocean_interaction.set_sources(&snapshot.ocean_actors);
            if let Some(ocean) = renderer.ocean.as_mut() {
                ocean.set_lake_actors(&snapshot.ocean_actors);
            }
            renderer.dynamic_model_renderer.entity_water = Arc::clone(&snapshot.player_water);
            if let Some(probe) = tile_probe.as_mut() {
                probe.begin_frame(&mut renderer);
            }
            let _activity = crate::thread_activity::activity(
                crate::thread_activity::ThreadSlot::Render,
                crate::thread_activity::Task::RenderFrame,
            );
            // Caught here rather than at the thread entry point so the renderer
            // is never dropped while this panic unwinds. The frame holds an
            // acquired swapchain image whose submissions are not retired yet;
            // tearing the surface down underneath it makes wgpu-hal assert
            // ("Trying to destroy a SwapchainAcquireSemaphore that is still in
            // use by a SurfaceTexture"), and a panic during a panic aborts the
            // process through __fastfail - no minidump, no log tail, and the
            // original message buried under the fallout.
            loop_sections.pre_render_ms = lap(&mut loop_mark);
            let frame_outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if shader_progress_polled_at.elapsed() >= Duration::from_millis(90) {
                    shader_progress_polled_at = Instant::now();
                    renderer.refresh_shader_compile_progress();
                }
                // One queued export capture per loop: it delivers this iteration.
                if let Some(capture) = renderer.studio_capture_queue.pop_front() {
                    return renderer.render_studio_capture(capture);
                }
                if let Some(scene) = snapshot.studio_scene.as_ref().filter(|_|
                    renderer.studio_world_viewport.is_some() || renderer.studio_export_size.is_some()) {
                    return renderer.render_studio_scene(scene);
                }
                if renderer.studio_world_viewport.is_some() && !renderer.asset_preview_mode {
                    renderer.dynamic_model_renderer.set_studio_environment(None,&renderer.queue);
                    renderer.dynamic_model_renderer.set_studio_lights(&[],&renderer.queue);
                    return renderer.render_asset_preview(&frame_camera,&[]);
                }
                renderer.render(
                    &frame_camera,
                    snapshot.player_position,
                    snapshot.area_mask.as_ref(),
                    snapshot.dynamic_models.as_slice(),
                    snapshot.spectator_highlights.as_slice(),
                    snapshot.transient_lights.as_slice(),
                    snapshot.studio_environment,
                    snapshot.companion_scene.as_ref(),
                    snapshot.dof_focus_target,
                    frame_crosshair_world,
                    &motion_reference_camera,
                    motion_camera_sample_dt,
                    motion_camera_changed_at.elapsed().as_secs_f32(),
                    snapshot.view_latch,
                    input_latelatch.then_some(latest_view.as_ref()),
                )
            }));
            loop_sections.render_call_ms = lap(&mut loop_mark);
            last_render_returned = loop_mark;
            if first_world_profile {
                crate::renderer::diagnostics::perf_capture::set_first_world_frame_trace(false);
            }
            match frame_outcome {
                Ok(result) => result,
                Err(payload) => {
                    let detail = panic_detail(&*payload).to_owned();
                    let _ = proxy.send_event(UserEvent::RendererError(format!(
                        "render thread panicked: {detail}"
                    )));
                    abandon_renderer(renderer, companion_renderers);
                    return;
                }
            }
        };
        if gamma_preparation.is_some() && render_result.is_ok() && !renderer.baked_brightness.busy {
            let generation = gamma_preparation.take().unwrap();
            let _ = proxy.send_event(UserEvent::GammaPrepared(generation));
        }
        if render_result.is_err() {
            renderer.retry_inline_transforms();
        }
        match render_result {
            Ok(info) => {
                #[cfg(test)]
                if let Some(scene)=snapshot.studio_scene.as_ref().filter(|_|renderer.studio_world_viewport.is_some()) {
                    let _=proxy.send_event(UserEvent::StudioTestScenePresented {overlap:scene.incoming.is_some()});
                }
                for cue in &info.native_thunder {
                    if cue.loudness<=0.0 {continue;}
                    let proxy=proxy.clone();let cue=*cue;
                    let _=std::thread::Builder::new().name("Blue Water thunder".into()).spawn(move || {
                        std::thread::sleep(Duration::from_secs_f64(cue.delay.clamp(0.0,180.0)));
                        let _=proxy.send_event(UserEvent::BlueWaterThunder(cue.loudness as f32));
                    });
                }

                if crate::fx::diagnostics::enabled()
                    && last_fx_diag_gpu.elapsed() >= Duration::from_secs(1)
                {
                    // Timestamp readback is already asynchronous. Missing
                    // samples are marked N/A; never trigger a GPU stall here.
                    let latest = renderer.gpu_profiler.latest;
                    let gpu_pass = |pass: GpuPass| latest
                        .and_then(|sample| sample.milliseconds[pass.index()]);
                    println!("[FX DIAG GPU] mode={:?} active_lights={} cpu_frame_ms={:.3} gpu_frame_ms={:?} gpu_legacy_dlights_ms={:?} gpu_rt_build_ms={:?} gpu_cluster_ms={:?}",
                        renderer.dynamic_lights_mode, renderer.active_light_count(), info.frame_ms,
                        gpu_pass(GpuPass::Frame), gpu_pass(GpuPass::LegacyDlights),
                        gpu_pass(GpuPass::RtBuild), gpu_pass(GpuPass::Cluster));
                    last_fx_diag_gpu = Instant::now();
                }
                if renderer.patch_speeds > 0 && renderer.patch_speeds != 8
                    && last_r_speeds.elapsed() >= Duration::from_secs(1)
                {
                    print_r_speeds(&renderer, &frame_camera, &snapshot, &info);
                    last_r_speeds = Instant::now();
                }
                if renderer.ocean_settings.adaptive_mesh_debug
                    && last_ocean_mesh_diag.elapsed() >= Duration::from_secs(1)
                {
                    last_ocean_mesh_diag = Instant::now();
                    let stats = &info.ocean;
                    let baseline = stats.adaptive_mesh_baseline_triangles;
                    let selected = stats.adaptive_mesh_selected_triangles;
                    let removed = baseline as i64 - selected as i64;
                    let percentage = if baseline != 0 {
                        removed as f64 * 100.0 / baseline as f64
                    } else { 0.0 };
                    let details = renderer.ocean.as_ref().map_or_else(
                        || "no OceanGpu resources".to_owned(),
                        |gpu| gpu.original_adaptive_mesh_steps(),
                    );
                    let reason = if !renderer.ocean_enabled { "ocean disabled" }
                        else if renderer.ocean_settings.backend == 0 { "Godot backend: native LOD not used" }
                        else if !renderer.ocean_settings.surface_pixels { "ocean pixel rendering disabled" }
                        else if stats.original_surfaces == 0 { "no native surface draw in this frame (PVS / Hybrid fallback / sky)" }
                        else if stats.adaptive_mesh_uncounted_slots != 0 { "baseline pending" }
                        else if baseline == 0 { "no visible native indices in admitted pools" }
                        else if baseline == selected { "NO REDUCTION (identical selected index count)" }
                        else if selected > baseline { "MESH INCREASED (more triangles than reference)" }
                        else { "native indexed mesh reduction confirmed" };
                    println!("[OCEAN ADAPTIVE DEBUG] A/B={} backend={} native_draws={} measured_pools={} pending={} full={} submitted={} saved={:+} ({:+.2}%) waves[height={:.3}m wavelength={:.1}m chop={:.2} wind={:.2}m/s blend={:.2}] screenLOD={:.1} localTol={:.2} status={} {}",
                        renderer.ocean_settings.adaptive_mesh, renderer.ocean_settings.backend,
                        stats.original_surfaces, stats.adaptive_mesh_counted_slots, stats.adaptive_mesh_uncounted_slots,
                        baseline, selected, removed, percentage,
                        renderer.ocean_settings.blue_water_waves.height,
                        renderer.ocean_settings.blue_water_waves.wavelength,
                        renderer.ocean_settings.blue_water_waves.choppiness,
                        renderer.ocean_settings.blue_water_waves.wind_speed,
                        renderer.ocean_settings.hybrid_fft_blend,
                        renderer.ocean_settings.screen_lod_pixels,
                        renderer.ocean_settings.adaptive_mesh_tolerance,
                        reason, details);
                }
                if perf_capture.as_mut().is_some_and(|capture| capture.record(&info, &renderer, &snapshot.client_perf, loop_sections)) {
                    if let Some(completed) = perf_capture.take() {
                        completed.finish();
                    }
                }
                if let Some(probe) = tile_probe.as_mut() {
                    let outcome = probe.record(&info, &mut renderer);
                    if matches!(outcome, Ok(false)) && tile_probe_reported_at.elapsed() >= Duration::from_secs(2) {
                        tile_probe_reported_at = Instant::now();
                        let (done, total) = probe.progress();
                        eprintln!("[TILE COST] {done}/{total} timings");
                    }
                    if !matches!(outcome, Ok(false)) {
                        if let Some(probe) = tile_probe.take() {
                            probe.restore(&mut renderer);
                            match outcome {
                                Ok(_) => {
                                    renderer.tile_cost_overlay = TileCostOverlay::new(probe.finish());
                                    if renderer.tile_cost_overlay.is_none() {
                                        eprintln!("[TILE COST] finished without frame or world GPU timings");
                                    }
                                }
                                Err(error) => eprintln!("[TILE COST] stopped: {error}"),
                            }
                        }
                        renderer.set_gpu_timings(user_gpu_timings || diag_forced_gpu_timings);
                    }
                }
                if let Some(sweep) = sky_sweep.as_mut() {
                    if sweep.record(&info, &mut renderer) {
                        if let Some(completed) = sky_sweep.take() {
                            completed.finish();
                        }
                        renderer.set_gpu_timings(user_gpu_timings || diag_forced_gpu_timings);
                    }
                }
                for scene_frame in renderer.take_companion_scene_ready() {
                    if let Some(worker) = companion_renderers.get(&scene_frame.id) {
                        worker.publish_scene(scene_frame);
                    }
                }
                if !first_frame_notified {
                    first_frame_notified = true;
                    let _ = proxy.send_event(UserEvent::RendererFirstFrame);
                }
                if !first_menu_frame_notified && renderer.egui_active {
                    first_menu_frame_notified = true;
                    let _ = proxy.send_event(UserEvent::RendererFirstMenuFrame);
                }
                if !neon_compositor_ready_notified && renderer.neon_menu_wanted() && renderer.neon_menu_ready() {
                    neon_compositor_ready_notified = true;
                    let _ = proxy.send_event(UserEvent::NeonMenuCompositorReady);
                }
                if let Some(token) = renderer.egui_present_token.filter(|_| renderer.egui_active) {
                    if last_stage_token != Some(token) {
                        last_stage_token = Some(token);
                        let _ = proxy.send_event(UserEvent::FrontendStagePresented(token));
                    }
                }
                let splashes = renderer.weather.rain.take_water_splashes();
                if !splashes.is_empty() {
                    let _ = proxy.send_event(UserEvent::WaterSplashes(splashes));
                }
                if perf_trace {
                    trace_frames += 1;
                    trace_frame_ms += info.frame_ms;
                    trace_prepare_ms += info.cpu_prepare_ms;
                    trace_acquire_ms += info.cpu_acquire_ms;
                    trace_encode_ms += info.cpu_encode_ms;
                    trace_submit_ms += info.cpu_submit_ms;
                    trace_present_ms += info.cpu_present_ms;
                    trace_dynamic_model_ms += info.dynamic_model_prepare_ms;
                    trace_grass_cpu_ms += info.grass.cpu_ms;
                    trace_surface_sprite_cpu_ms += info.surface_sprites.cpu_ms;
                    trace_ocean_topology_ms+=info.ocean.topology_ms;
                    trace_ocean_topology_max_ms=trace_ocean_topology_max_ms.max(info.ocean.topology_ms);
                    if let Some(timing) = renderer.gpu_profiler.latest {
                        for (slot, pass) in TRACE_GPU_PASSES.iter().enumerate() {
                            if let Some(ms) = timing.milliseconds[pass.index()] {
                                trace_gpu_sum[slot] += ms;
                                trace_gpu_min[slot] = trace_gpu_min[slot].min(ms);
                                trace_gpu_max[slot] = trace_gpu_max[slot].max(ms);
                                trace_gpu_n[slot] += 1;
                            }
                        }
                    }
                    trace_prep_stage_ms[0] += info.prep_camera_ms;
                    trace_prep_stage_ms[1] += info.prep_shadow_ms;
                    trace_prep_stage_ms[2] += info.prep_world_ms;
                    trace_prep_stage_ms[3] += info.prep_reflection_ms;
                }
                last_info = info;
                previous_graph_frame_complete = true;
                if input_diagnostic.is_recording() {
                    diag_rendered = true;
                    diag_present_latest_input = latest_view.load().input;
                }
                let late_measurement = input_latelatch
                    .then_some(last_info.late_latch)
                    .flatten()
                    .filter(|measurement| {
                        measurement.input.sequence != last_measured_input_sequence
                    });
                let measured_sample = late_measurement
                    .map(|measurement| measurement.input)
                    .or(input_latency_sample);
                if let Some(sample) = measured_sample {
                    let render_sample_at = late_measurement
                        .map(|measurement| measurement.sampled_at)
                        .or_else(|| {
                            frame_start_view_sample
                                .filter(|measurement| measurement.input.sequence == sample.sequence)
                                .map(|measurement| measurement.sampled_at)
                        })
                        .unwrap_or(frame_loop_started);
                    let input_event_to_sim_ms = sample
                        .simulation_at
                        .saturating_duration_since(sample.event_at)
                        .as_secs_f64()
                        * 1000.0;
                    let input_sim_to_render_ms = render_sample_at
                        .saturating_duration_since(sample.simulation_at)
                        .as_secs_f64()
                        * 1000.0;
                    let present_completed_at = last_info
                        .present_call_completed_at
                        .unwrap_or_else(Instant::now);
                    let input_event_to_present_call_ms = present_completed_at
                        .saturating_duration_since(sample.event_at)
                        .as_secs_f64()
                        * 1000.0;
                    input_latency_samples += 1;
                    input_event_to_sim_sum_ms += input_event_to_sim_ms;
                    input_sim_to_render_sum_ms += input_sim_to_render_ms;
                    if let Some(measurement) = late_measurement {
                        input_latch_samples += 1;
                        input_event_to_latch_sum_ms += measurement
                            .sampled_at
                            .saturating_duration_since(sample.event_at)
                            .as_secs_f64()
                            * 1000.0;
                        if let Some(submit_at) = last_info.submit_call_at {
                            input_latch_to_submit_sum_ms += submit_at
                                .saturating_duration_since(measurement.sampled_at)
                                .as_secs_f64()
                                * 1000.0;
                        }
                        input_latch_to_present_call_sum_ms += present_completed_at
                            .saturating_duration_since(measurement.sampled_at)
                            .as_secs_f64()
                            * 1000.0;
                    }
                    input_event_to_present_call_sum_ms += input_event_to_present_call_ms;
                    input_event_to_present_call_max_ms =
                        input_event_to_present_call_max_ms.max(input_event_to_present_call_ms);
                    last_measured_input_sequence = sample.sequence;
                }
                if let Some((
                    request_id,
                    started,
                    name,
                    triangles,
                    batches,
                    upload_ms,
                    upload_timings,
                    timings,
                    bsp_stats,
                )) = pending_world_uploaded.take()
                {
                    if let Some((requested_at, received_at, upload_started_at, upload_ended_at)) = pending_first_world_phases.take() {
                        let ms = |later: Instant, earlier: Instant| later.saturating_duration_since(earlier).as_secs_f64() * 1000.0;
                        rverbose!(2, "[MAP FIRST FRAME PHASES] request->render-queue {:.1} ms (includes preparation) | deferred loading-screen frame {:.1} | world upload {:.1} | post-upload->render {:.1} | first render/present {:.1} | total {:.1} ms",
                            ms(received_at, requested_at), ms(upload_started_at, received_at),
                            ms(upload_ended_at, upload_started_at), ms(frame_loop_started, upload_ended_at),
                            frame_loop_started.elapsed().as_secs_f64() * 1000.0, started.elapsed().as_secs_f64() * 1000.0);
                        rverbose!(2, "[MAP FIRST RENDER CPU] frame {:.1} | prepare {:.1} | acquire {:.1} | encode {:.1} | submit {:.1} | present {:.1} ms (CPU timings; no GPU wait inference)",
                            last_info.frame_ms, last_info.cpu_prepare_ms, last_info.cpu_acquire_ms,
                            last_info.cpu_encode_ms, last_info.cpu_submit_ms, last_info.cpu_present_ms);
                        let known_encode_ms = last_info.perf_encode_setup_ms
                            + last_info.perf_encode_pre_world_ms
                            + last_info.perf_encode_scene_ms
                            + last_info.perf_encode_post_ui_ms
                            + last_info.perf_encoder_finish_ms
                            + last_info.perf_profiler_readback_encode_ms;
                        rverbose!(2, "[MAP FIRST ENCODE STAGES] encoder setup {:.1} | pre-world (ocean/shadow/reflection/etc) {:.1} | scene {:.1} | post/UI {:.1} | encoder finish {:.1} | profiler copy {:.1} | other {:.1} ms",
                            last_info.perf_encode_setup_ms, last_info.perf_encode_pre_world_ms,
                            last_info.perf_encode_scene_ms, last_info.perf_encode_post_ui_ms,
                            last_info.perf_encoder_finish_ms, last_info.perf_profiler_readback_encode_ms,
                            (last_info.cpu_encode_ms - known_encode_ms).max(0.0));
                        rverbose!(2, "[MAP FIRST PRE-WORLD] ocean setup/FFT/coast {:.1} | grass+shadows {:.1} | remaining (depth/cull/reflection/etc) {:.1} ms",
                            last_info.perf_pre_world_ocean_ms,
                            last_info.perf_pre_world_shadows_ms,
                            last_info.perf_pre_world_remaining_ms);
                        rverbose!(2, "[MAP FIRST ENCODE COUNTS] {} render passes | {} compute passes | {} indexed draws | {} deferred bundle builds | {} deferred bundle hits",
                            last_info.perf_encode_calls.render_passes,
                            last_info.perf_encode_calls.compute_passes,
                            last_info.perf_encode_calls.draw_indexed,
                            last_info.perf_deferred_bundle_build,
                            last_info.perf_deferred_bundle_hit);
                    }
                    let _ = proxy.send_event(UserEvent::WorldUploaded {
                        request_id,
                        name,
                        triangles,
                        batches,
                        upload_ms,
                        upload_timings,
                        first_frame_ms: started.elapsed().as_secs_f64() * 1000.0,
                        timings,
                        bsp_stats,
                    });
                }

                // Slow levelshot VFS/decode work happens only after a visible
                // unknown-map frame has completed present().
                renderer.resolve_pending_ui_levelshot();
            }
            Err(RenderError::DeviceLost) => {
                stop_for_device_loss(&proxy, renderer, companion_renderers);
                return;
            }
            Err(RenderError::Lost | RenderError::Outdated) => {
                renderer.reconfigure("surface_lost_or_outdated")
            }
            Err(RenderError::Timeout) => {}
            Err(RenderError::Occluded) => {
                // An occluded/minimized/hidden surface cannot make forward progress
                // by spinning at uncapped render speed. In particular, startup used
                // to hold the HWND hidden until the first successful present, making
                // this path a self-sustaining busy loop. RendererReady now reveals
                // deliberately hidden startup/restart windows; this small backoff
                // also keeps genuinely occluded windows from burning a CPU core.
                thread::sleep(Duration::from_millis(8));
            }
            Err(RenderError::Validation) => {
                let _ = proxy.send_event(UserEvent::RendererError(
                    "GPU surface validation error".into(),
                ));
            }
        }

        // Export readbacks map asynchronously; collect whatever finished.
        renderer.finish_studio_readbacks(false);
        while let Some(result) = renderer.take_screenshot_result() {
            let _ = proxy.send_event(UserEvent::ScreenshotFinished(result));
        }

        stats_frames += 1;
        let elapsed = stats_started.elapsed();
        if elapsed >= Duration::from_millis(500) {
            let fps = stats_frames as f64 / elapsed.as_secs_f64();
            let threads = {
                // Sample outside the main-thread event callback so observing the
                // overlay does not make MAIN appear busy by definition. Mark this
                // tiny stats section as render work so RENDER's instantaneous state
                // remains meaningful at the sample point.
                let _activity = crate::thread_activity::activity(
                    crate::thread_activity::ThreadSlot::Render,
                    crate::thread_activity::Task::RenderFrame,
                );
                crate::thread_activity::sample(renderer.ui_state.video.draw_fps >= 2)
            };
            let gpu_timing = renderer.gpu_profiler.latest;
            let gpu_pass =
                |pass: GpuPass| gpu_timing.and_then(|timing| timing.milliseconds[pass.index()]);
            if proxy
                .send_event(UserEvent::RenderStats(RenderStats {
                    fps,
                    frame_ms: last_info.frame_ms,
                    cpu_prepare_ms: last_info.cpu_prepare_ms,
                    cpu_acquire_ms: last_info.cpu_acquire_ms,
                    cpu_encode_ms: last_info.cpu_encode_ms,
                    cpu_submit_ms: last_info.cpu_submit_ms,
                    cpu_present_ms: last_info.cpu_present_ms,
                    dynamic_model_prepare_ms: last_info.dynamic_model_prepare_ms,
                    dynamic_model_surfaces: last_info.dynamic_model_surfaces,
                    dynamic_model_vertices: last_info.dynamic_model_vertices,
                    dynamic_model_indices: last_info.dynamic_model_indices,
                    gpu_ms: last_info.gpu_ms,
                    gpu_depth_ms: gpu_pass(GpuPass::Depth),
                    gpu_hiz_ms: gpu_pass(GpuPass::HiZ),
                    gpu_cull_ms: gpu_pass(GpuPass::Cull),
                    gpu_cluster_ms: gpu_pass(GpuPass::Cluster),
                    gpu_world_ms: gpu_pass(GpuPass::World),
                    gpu_fx_sprites_ms: match (
                        gpu_pass(GpuPass::FxSpritesOpaque),
                        gpu_pass(GpuPass::FxSpritesTranslucent),
                    ) {
                        (Some(opaque), Some(translucent)) => Some(opaque + translucent),
                        (Some(opaque), None) => Some(opaque),
                        (None, Some(translucent)) => Some(translucent),
                        (None, None) => None,
                    },
                    gpu_post_ms: gpu_pass(GpuPass::Post),
                    gpu_ui_ms: gpu_pass(GpuPass::Ui),
                    cull_visible: last_info.cull_visible,
                    cull_frustum_rejected: last_info.cull_frustum_rejected,
                    cull_hiz_rejected: last_info.cull_hiz_rejected,
                    cull_pvs_rejected: last_info.cull_pvs_rejected,
                    cull_area_rejected: last_info.cull_area_rejected,
                    world_program_binds: (
                        last_info.world_specialized_binds,
                        last_info.world_general_binds,
                    ),
                    world_shader_programs: last_info.world_shader_programs,
                    input_event_to_sim_ms: (input_latency_samples != 0)
                        .then(|| input_event_to_sim_sum_ms / input_latency_samples as f64),
                    input_sim_to_render_ms: (input_latency_samples != 0)
                        .then(|| input_sim_to_render_sum_ms / input_latency_samples as f64),
                    input_event_to_latch_ms: (input_latch_samples != 0)
                        .then(|| input_event_to_latch_sum_ms / input_latch_samples as f64),
                    input_latch_to_submit_ms: (input_latch_samples != 0)
                        .then(|| input_latch_to_submit_sum_ms / input_latch_samples as f64),
                    input_latch_to_present_call_ms: (input_latch_samples != 0)
                        .then(|| input_latch_to_present_call_sum_ms / input_latch_samples as f64),
                    input_event_to_present_call_ms: (input_latency_samples != 0)
                        .then(|| input_event_to_present_call_sum_ms / input_latency_samples as f64),
                    input_event_to_present_call_max_ms: (input_latency_samples != 0)
                        .then_some(input_event_to_present_call_max_ms),
                    input_latency_samples: u32::try_from(input_latency_samples).unwrap_or(u32::MAX),
                    msaa_samples: renderer.msaa_samples,
                    vsync: renderer.vsync,
                    threads,
                }))
                .is_err()
            {
                return;
            }
            stats_started = Instant::now();
            stats_frames = 0;
            input_latency_samples = 0;
            input_latch_samples = 0;
            input_event_to_sim_sum_ms = 0.0;
            input_sim_to_render_sum_ms = 0.0;
            input_event_to_latch_sum_ms = 0.0;
            input_latch_to_submit_sum_ms = 0.0;
            input_latch_to_present_call_sum_ms = 0.0;
            input_event_to_present_call_sum_ms = 0.0;
            input_event_to_present_call_max_ms = 0.0;
        }

        if perf_trace && trace_started.elapsed() >= Duration::from_secs(1) {
            let elapsed = trace_started.elapsed().as_secs_f64();
            let frames = trace_frames.max(1) as f64;
            let gpu = renderer.gpu_profiler.latest;
            let gpu_value =
                |pass: GpuPass| gpu.and_then(|timing| timing.milliseconds[pass.index()]);
            let gpu_ms = |pass: GpuPass| {
                gpu_value(pass).map_or_else(|| "--".to_string(), |ms| format!("{ms:.4}"))
            };
            let dynamic_model_gpu = match (
                gpu_value(GpuPass::DynamicModels),
                gpu_value(GpuPass::DynamicModelsTranslucent),
            ) {
                (Some(opaque), Some(translucent)) => Some(opaque + translucent),
                (Some(opaque), None) => Some(opaque),
                (None, Some(translucent)) => Some(translucent),
                (None, None) => None,
            };
            let dynamic_model_ms =
                dynamic_model_gpu.map_or_else(|| "--".to_string(), |ms| format!("{ms:.4}"));
            let fx_sprite_gpu = match (
                gpu_value(GpuPass::FxSpritesOpaque),
                gpu_value(GpuPass::FxSpritesTranslucent),
            ) {
                (Some(opaque), Some(translucent)) => Some(opaque + translucent),
                (Some(opaque), None) => Some(opaque),
                (None, Some(translucent)) => Some(translucent),
                (None, None) => None,
            };
            let fx_sprite_ms =
                fx_sprite_gpu.map_or_else(|| "--".to_string(), |ms| format!("{ms:.4}"));
            let grass_prepare_gpu = gpu_value(GpuPass::GrassPrepare);
            let grass_draw_gpu = gpu_value(GpuPass::Grass);
            let grass_total_gpu = match (grass_prepare_gpu, grass_draw_gpu) {
                (Some(prepare), Some(draw)) => Some(prepare + draw),
                (None, Some(draw)) => Some(draw),
                _ => None,
            };
            let grass_total_ms =
                grass_total_gpu.map_or_else(|| "--".to_string(), |ms| format!("{ms:.4}"));
            println!(
                "[JKA PERF] fps={:.1} cpu_frame={:.4}ms prep={:.4}ms acquire={:.4}ms encode={:.4}ms submit={:.4}ms present={:.4}ms dyn_prepare={:.4}ms g2_draws={}/{} batch={} world_batches[pvs={} frustum_reject={} ocean_cull={}b/{}t encoded={} multidraw_groups={} multidraw_batches={}] vsync={} mode={:?} queue={} fps_cap={} surface_gen={} hotpath=1",
                trace_frames as f64 / elapsed,
                trace_frame_ms / frames,
                trace_prepare_ms / frames,
                trace_acquire_ms / frames,
                trace_encode_ms / frames,
                trace_submit_ms / frames,
                trace_present_ms / frames,
                trace_dynamic_model_ms / frames,
                last_info.ghoul2_gpu_draw_calls,
                last_info.ghoul2_gpu_instances,
                renderer.dynamic_model_renderer.ghoul2_batch_draws.config_value(),
                last_info.world_pvs_batches,
                last_info.world_frustum_rejected,
                last_info.world_ocean_culled,
                last_info.world_ocean_culled_triangles,
                last_info.world_encoded_batches,
                last_info.world_multidraw_groups,
                last_info.world_multidraw_batches,
                renderer.vsync.label(),
                renderer.config.present_mode,
                renderer.config.desired_maximum_frame_latency,
                fps_cap,
                renderer.surface_config_generation,
            );
            if renderer.world_sort_mode != 0 {
                println!("[JKA WORLD SORT] mode={} eligible={} runs={} moved={} theoretical_binds={}->{} actual_base_binds={}spec/{}general",
                    renderer.world_sort_mode,
                    last_info.world_sort_candidates, last_info.world_sort_runs, last_info.world_sort_moved,
                    last_info.world_sort_binds_before, last_info.world_sort_binds_after,
                    last_info.world_specialized_binds, last_info.world_general_binds);
            }
            println!("[JKA WORLD PROGRAMS] requested={} active={} path={} binds[general={} specialized={} minimal={} shared={} generated={}]",
                last_info.world_shader_programs.requested, last_info.world_shader_programs.active, last_info.world_path,
                last_info.world_shader_programs.binds[0], last_info.world_shader_programs.binds[1],
                last_info.world_shader_programs.binds[2], last_info.world_shader_programs.binds[3], last_info.world_shader_programs.binds[4]);
            // A/B line: which world renderer ran, plus the unified prepare split.
            // FastBaseline does not split its prepare stage, so those read 0.
            println!(
                "[JKA PERF PATH] path={} gpu_timings={} cpu_frame={:.4}ms prep={:.4}ms[camera={:.4} shadow={:.4} world={:.4} reflection={:.4} dynamic={:.4}] encode={:.4}ms submit={:.4}ms present={:.4}ms",
                last_info.world_path,
                renderer.gpu_profiler.enabled(),
                trace_frame_ms / frames,
                trace_prepare_ms / frames,
                trace_prep_stage_ms[0] / frames,
                trace_prep_stage_ms[1] / frames,
                trace_prep_stage_ms[2] / frames,
                trace_prep_stage_ms[3] / frames,
                trace_dynamic_model_ms / frames,
                trace_encode_ms / frames,
                trace_submit_ms / frames,
                trace_present_ms / frames,
            );
            let client = snapshot.client_perf;
            println!(
                "[JKA CLIENT PERF] total={:.4}ms snapshot={:.4}ms audio={:.4}ms events={:.4}ms event_prep={:.4}ms jobs={} pool_threads={} parallel={} event_decode={:.4}ms decode_jobs={} decode_parallel={} entities={:.4}ms players={:.4}ms followed={:.4}ms fx={:.4}ms fx_tess={:.4}ms g2_mode={} g2_pose={:.4}ms/{} motion={:.4}ms/{} g2_skin={:.4}ms g2_bolt={:.4}ms/{} surfaces={} verts={} cull={}/{} lod=[{},{},{},{}] dyn[surfaces={} verts={} indices={}]",
                client.total_ms,
                client.snapshot_ms,
                client.audio_ms,
                client.events_ms,
                client.event_prepare_ms,
                client.event_worker_jobs,
                client.event_worker_threads,
                client.event_worker_parallel,
                client.event_sound_decode_ms,
                client.event_sound_decode_jobs,
                client.event_sound_decode_parallel,
                client.entity_present_ms,
                client.player_present_ms,
                client.followed_player_ms,
                client.fx_ms,
                client.fx_tessellate_ms,
                client.ghoul2_skinning_mode.config_value(),
                client.ghoul2_pose_ms,
                client.ghoul2_pose_evals,
                client.ghoul2_motion_pose_ms,
                client.ghoul2_motion_pose_evals,
                client.ghoul2_skin_ms,
                client.ghoul2_bolt_ms,
                client.ghoul2_bolt_queries,
                client.ghoul2_surfaces,
                client.ghoul2_vertices,
                client.ghoul2_frustum_culled,
                client.ghoul2_frustum_tests,
                client.ghoul2_lod_counts[0],
                client.ghoul2_lod_counts[1],
                client.ghoul2_lod_counts[2],
                client.ghoul2_lod_counts[3],
                client.dynamic_surfaces,
                client.dynamic_vertices,
                client.dynamic_indices,
            );
            println!(
                "[JKA FX PERF] draws={} sprite={} oriented={} line={} quad={} mesh={} cylinder={} output[surfaces={} cpu_surfaces={} cpu_verts={} cpu_indices={} gpu_sprite_batches={} gpu_sprite_instances={}] zero_alpha_discard={} gpu_sprites={}ms",
                client.fx_draws,
                client.fx_sprites,
                client.fx_oriented_quads,
                client.fx_lines,
                client.fx_quads,
                client.fx_meshes,
                client.fx_cylinders,
                client.fx_render_surfaces,
                client.fx_cpu_geom_surfaces,
                client.fx_cpu_vertices,
                client.fx_cpu_indices,
                client.fx_gpu_sprite_batches,
                client.fx_gpu_sprite_instances,
                u8::from(renderer.dynamic_model_renderer.fx_zero_alpha_discard),
                fx_sprite_ms,
            );
            let dlight = renderer.legacy_dlight_perf_stats();
            if renderer.dynamic_lights_mode == DynamicLightsMode::Legacy {
                let avg_radius = if dlight.transient_count == 0 {
                    0.0
                } else {
                    dlight.radius_sum / dlight.transient_count as f64
                };
                let avg_candidates = if dlight.surfaces_touched == 0 {
                    0.0
                } else {
                    dlight.candidate_pairs as f64 / dlight.surfaces_touched as f64
                };
                let mask_density = if dlight.surfaces_eligible == 0 || dlight.transient_count == 0 {
                    0.0
                } else {
                    100.0 * dlight.candidate_pairs as f64
                        / (dlight.surfaces_eligible * dlight.transient_count) as f64
                };
                let avg_surfaces_per_light = if dlight.transient_count == 0 {
                    0.0
                } else {
                    dlight.total_surfaces_per_light as f64 / dlight.transient_count as f64
                };
                println!(
                    "[JKA DLIGHT PERF] transient={} sources[saber={} fx={} promoted_fx={} saber_mark={}] radius[avg={:.2} max={:.2}] surfaces[total={} eligible={} touched={}] pairs={} mask_density={:.2}% candidates_per_touched[avg={:.2} max={}] buckets[0={} 1={} 2-4={} 5-8={} 9-16={} 17+={}] pairs_by_source[saber={} fx={} promoted_fx={} saber_mark={}] surfaces_per_light[avg={:.2} max={}]",
                    dlight.transient_count,
                    dlight.saber_sources,
                    dlight.authored_fx_sources,
                    dlight.promoted_fx_sources,
                    dlight.saber_mark_sources,
                    avg_radius,
                    dlight.radius_max,
                    dlight.surfaces_total,
                    dlight.surfaces_eligible,
                    dlight.surfaces_touched,
                    dlight.candidate_pairs,
                    mask_density,
                    avg_candidates,
                    dlight.max_candidates_per_surface,
                    dlight.surface_buckets[0],
                    dlight.surface_buckets[1],
                    dlight.surface_buckets[2],
                    dlight.surface_buckets[3],
                    dlight.surface_buckets[4],
                    dlight.surface_buckets[5],
                    dlight.saber_pairs,
                    dlight.authored_fx_pairs,
                    dlight.promoted_fx_pairs,
                    dlight.saber_mark_pairs,
                    avg_surfaces_per_light,
                    dlight.max_surfaces_per_light,
                );
            }
            println!(
"[JKA PERF GPU] frame={}ms rt_build={}ms world={}ms dlights={}ms dyn_models={}ms fx_sprites={}ms ocean_sim={}ms coastal_sim={}ms ocean_surface={}ms ocean_optics={}ms grass={}ms grass_prepare={}ms grass_draw={}ms rain_sim={}ms rain_draw={}ms rain_haze={}ms rain_froxel={}ms rain_lens={}ms post={}ms depth={}ms hiz={}ms cull={}ms cluster={}ms ui={}ms clouds={}ms ssr_visibility={}ms ssr_trace={}ms",
                gpu_ms(GpuPass::Frame),
                gpu_ms(GpuPass::RtBuild),
                gpu_ms(GpuPass::World),
                gpu_ms(GpuPass::LegacyDlights),
                dynamic_model_ms,
                fx_sprite_ms,
                gpu_ms(GpuPass::OceanSim),
                gpu_ms(GpuPass::CoastalSim),
                gpu_ms(GpuPass::OceanSurface),
                gpu_ms(GpuPass::OceanOptics),
                grass_total_ms,
                gpu_ms(GpuPass::GrassPrepare),
                gpu_ms(GpuPass::Grass),
                gpu_ms(GpuPass::RainSim),
                gpu_ms(GpuPass::RainDraw),
                gpu_ms(GpuPass::RainHaze),
                gpu_ms(GpuPass::WeatherFog),
                gpu_ms(GpuPass::RainLens),
                gpu_ms(GpuPass::Post),
                gpu_ms(GpuPass::Depth),
                gpu_ms(GpuPass::HiZ),
                gpu_ms(GpuPass::Cull),
                gpu_ms(GpuPass::Cluster),
                gpu_ms(GpuPass::Ui),
                gpu_ms(GpuPass::Clouds),
                gpu_ms(GpuPass::SsrVisibility),
                gpu_ms(GpuPass::SsrTrace),
            );
            println!(
                "[JKA WEATHER PERF] ab[fast_post={} water_field={} fast_vertex={} lean_depth={} static_depth={} opaque_depth={} lean_models={} lean_post={} haze_2d={}] rain_only_depth={} rain_only_post={} rain[sim={}ms draw={}ms haze={}ms froxel={}ms lens={}ms] frame={}ms depth={}ms post={}ms",
                u8::from(renderer.weather.rain.fast_post),
                u8::from(renderer.weather.rain.water_field_enabled),
                u8::from(renderer.weather.rain.fast_vertex),
                u8::from(renderer.weather.rain.lean_depth),
                u8::from(renderer.weather.rain.static_depth),
                u8::from(renderer.weather.rain.opaque_depth),
                u8::from(renderer.weather.rain.lean_models),
                u8::from(renderer.weather.rain.lean_post),
                u8::from(renderer.weather_uses_2d_haze()),
                u8::from(renderer.frame_plan.rain_only_linear_depth),
                u8::from(renderer.frame_plan.rain_only_post),
                gpu_ms(GpuPass::RainSim),
                gpu_ms(GpuPass::RainDraw),
                gpu_ms(GpuPass::RainHaze),
                gpu_ms(GpuPass::WeatherFog),
                gpu_ms(GpuPass::RainLens),
                gpu_ms(GpuPass::Frame),
                gpu_ms(GpuPass::Depth),
                gpu_ms(GpuPass::Post),
            );
            if renderer.hardware_rt_active() {
                if let Some(rt) = renderer.ray_traced_shadows.as_ref() {
                    println!(
                        "[JKA RT PERF] build={}ms skin={}ms accel={}ms visibility={}ms rt_taa={} rt_trace_rate={} static_opaque[geoms={} tris={}] masked[geoms={} tris={}] dynamic[instances={} rigid_tris={} skinned_tris={} masked_rigid={} masked_skinned={}] caches[rigid={} skinned={}] alpha[textures={} packed_words={} generation={}] local_rays={} (budget_per_light_evaluation; not_measured)",
                    gpu_ms(GpuPass::RtBuild),
                    gpu_ms(GpuPass::RtSkin),
                    gpu_ms(GpuPass::RtAcceleration),
                    gpu_ms(GpuPass::RtVisibility),
                    renderer.rt_taa_mode,
                    renderer.rt_trace_rate,
                    rt.geometry_count,
                    rt.triangle_count,
                    rt.masked_geometry_count,
                    rt.masked_triangle_count,
                    rt.last_dynamic_instance_count,
                    rt.active_rigid_triangles,
                    rt.active_skinned_triangles,
                    rt.active_masked_rigid,
                    rt.active_masked_skinned,
                    rt.rigid_blas_cache.len(),
                    rt.skinned_blas_cache.len(),
                    rt.alpha_textures.len(),
                    rt.alpha_texels.len(),
                        rt.alpha_buffers_generation,
                        if renderer.ray_traced_local_shadows_active() { format!("up_to_{}_per_finite_emitter_1_per_hard_point", renderer.rt_samples) } else { "0".into() },
                    );
                }
            }
            let ocean = last_info.ocean;
            // `promoted` counts what the loaded map CARRIES, which says nothing
            // about whether the ocean is rendering: water is clustered and
            // promoted at upload regardless. The `gates` group is the authority.
            // `resources` is the renderer's OceanGpu (created by
            // SetOceanEnabled), `variant` is the active world PSO's ocean
            // support, and `active` is the `render_ocean` both must agree on
            // (draw.rs). cvar=1 with resources=0 means the setting was changed
            // on a world prepared without ocean: apply video settings or
            // vid_restart.
            let ocean_variant = renderer
                .world
                .as_ref()
                .is_some_and(|world| world.active_pipeline_variant.ocean);
            let ocean_resources = renderer.ocean.is_some();
            // avg/min/max over the trace window, not one frame: see TRACE_GPU_PASSES.
            // `avg` is what the ocean actually costs per frame; a large max/avg
            // ratio means the work is spiky rather than expensive.
            let gpu_window: [String; TRACE_GPU_PASSES.len()] =
                std::array::from_fn(|slot| {
                    if trace_gpu_n[slot] == 0 {
                        return "--/--/--".to_string();
                    }
                    format!(
                        "{:.4}/{:.4}/{:.4}",
                        trace_gpu_sum[slot] / trace_gpu_n[slot] as f64,
                        trace_gpu_min[slot],
                        trace_gpu_max[slot],
                    )
                });
            if ocean.promoted_surfaces > 0 || ocean.authored_oceans > 0 || renderer.ocean_enabled {
                println!(
                    "[JKA OCEAN PERF] gates[cvar={} resources={} variant={} active={}] topology_cpu[last/avg/max]={:.4}/{:.4}/{:.4}ms samples={} avg/min/max_ms sim={} coastal={} surface={} optics={} frame={} surfaces[promoted={} authored={}] clipmaps[visible={} drawn={} analytic={} duplicate={} reject_area={} reject_frustum={}] samples_passed={} primed={} triangles={} lod[min_level={} half_cells={}] optics[active={} submerged={} pixels={} viewport={} rect={}x{}+{}+{} unbounded={} sim_hidden={}] settings[mesh_quality={} map_size={} updates={:.1} wind_foam={} sea_spray={} ab[optics_cull={} optics_scissor={} sim_when_hidden={}]]",
                    u8::from(renderer.ocean_enabled),
                    u8::from(ocean_resources),
                    u8::from(ocean_variant),
                    u8::from(renderer.ocean_enabled && ocean_variant),
                    ocean.topology_ms,
                    trace_ocean_topology_ms/trace_frames.max(1) as f64,
                    trace_ocean_topology_max_ms,
                    trace_gpu_n[1],
                    gpu_window[1].as_str(),
                    gpu_window[2].as_str(),
                    gpu_window[3].as_str(),
                    gpu_window[4].as_str(),
                    gpu_window[0].as_str(),
                    ocean.promoted_surfaces,
                    ocean.authored_oceans,
                    ocean.clipmap_batches,
                    ocean.clipmaps_drawn,
                    ocean.analytic_draws,
                    ocean.rejected_duplicate,
                    ocean.rejected_area,
                    ocean.rejected_frustum,
                    ocean.visible_samples,
                    u8::from(ocean.visibility_primed),
                    ocean.triangles,
                    ocean.minimum_level,
                    ocean.half_cells,
                    u8::from(ocean.optics_active),
                    u8::from(ocean.submerged),
                    ocean.optics_pixels,
                    u64::from(renderer.config.width) * u64::from(renderer.config.height),
                    ocean.optics_rect[2],
                    ocean.optics_rect[3],
                    ocean.optics_rect[0],
                    ocean.optics_rect[1],
                    u8::from(ocean.optics_unbounded),
                    u8::from(ocean.sim_hidden),
                    renderer.ocean_settings.mesh_quality,
                    renderer.ocean_settings.map_size,
                    renderer.ocean_settings.updates_per_second,
                    u8::from(renderer.ocean_settings.wind_foam_streaks),
                    u8::from(renderer.ocean_settings.sea_spray),
                    u8::from(renderer.ocean_settings.optics.cull),
                    u8::from(renderer.ocean_settings.optics.scissor),
                    u8::from(renderer.ocean_settings.simulate_when_hidden),
                );
            }
            let grass = last_info.grass;
            println!(
                "[JKA GRASS PERF] cpu_select={:.4}ms gpu={}ms gpu_prepare={}ms gpu_draw={}ms patches={}/{} reject[pvs={} frustum={} distance={} lod_empty={}] draw_records={} submit_calls={} blades[high={} mid={} low={}] triangles={} ab[precompute={} mid_lod={} front_to_back={}]",
                trace_grass_cpu_ms / frames,
                grass_total_gpu.map_or_else(|| "--".to_string(), |ms| format!("{ms:.4}")),
                gpu_ms(GpuPass::GrassPrepare),
                gpu_ms(GpuPass::Grass),
                grass.patches_visible,
                grass.patches_total,
                grass.pvs_rejected,
                grass.frustum_rejected,
                grass.distance_rejected,
                grass.lod_empty,
                grass.draw_calls,
                grass.submit_calls,
                grass.high_blades,
                grass.mid_blades,
                grass.low_blades,
                grass.triangles,
                u8::from(renderer.grass_precompute_enabled),
                u8::from(renderer.grass_mid_lod_enabled),
                u8::from(renderer.grass_front_to_back_enabled),
            );
            let sprites = last_info.surface_sprites;
            println!(
                "[JKA SURFACE SPRITES PERF] gpu_path={} cpu={:.4}ms gpu={}ms cpu_sprites={} resident_instances={} draws={} chunks={}/{} upload_bytes={}",
                u8::from(renderer.surface_sprite_effect_renderer.resident_enabled),
                trace_surface_sprite_cpu_ms / frames,
                gpu_ms(GpuPass::SurfaceSprites),
                sprites.cpu_sprites,
                sprites.resident_instances,
                sprites.draws,
                sprites.chunks_visible,
                sprites.chunks_total,
                sprites.upload_bytes,
            );
            println!(
                "[JKA PERF PLAN] post={} post_compact={} linear_depth={} hiz={} gpu_cull={} compact={} clustered={} gamma={:.3} grain={:.3} lut={} lut_strength={:.2} fog={:?} hdr={} tonemap={} bloom={} ssao={} static_ao={} fxaa={} smaa={} taa={} contact={} ssr={} motion={:.2} motion_scale={:.2} dof={:.2} dof_quality={} dof_focus={:.0} ca={:.2} vignette={} local_shadows={} pbr={} pom={} cascaded={}",
                u8::from(renderer.frame_plan.use_post),
                u8::from(renderer.frame_plan.gamma_only_post),
                u8::from(renderer.frame_plan.needs_linear_depth),
                u8::from(renderer.frame_plan.use_hiz),
                u8::from(renderer.frame_plan.use_gpu_culling),
                u8::from(
                    renderer.frame_plan.use_gpu_culling
                        && renderer.gpu_compaction_supported
                        && renderer
                            .world
                            .as_ref()
                            .is_some_and(|world| !world.compact_groups.is_empty()),
                ),
                u8::from(renderer.frame_plan.use_clustered_lighting),
                renderer.gamma,
                renderer.film_grain_strength,
                renderer.color_lut_preset.config_value(),
                renderer.color_lut_strength,
                renderer.weather.fog.mode,
                u8::from(renderer.hdr_enabled),
                u8::from(renderer.tone_mapping_enabled),
                u8::from(renderer.glow_mode.enabled()),
                u8::from(renderer.ssao_enabled),
                u8::from(renderer.static_bsp_ao_enabled),
                u8::from(renderer.fxaa_enabled),
                u8::from(renderer.smaa_enabled),
                u8::from(renderer.taa_enabled),
                u8::from(renderer.contact_shadows_enabled),
                u8::from(renderer.ssr_enabled),
                renderer.motion_blur_strength,
                renderer.motion_blur_runtime_scale,
                renderer.depth_of_field_strength,
                renderer.dof_quality.config_value(),
                renderer.dof_focus_distance,
                renderer.chromatic_aberration_strength,
                u8::from(renderer.vignette_enabled),
                u8::from(renderer.local_light_shadows_enabled),
                u8::from(renderer.pbr_enabled && PBR_PROFILE_MATERIALS),
                u8::from(renderer.pbr_enabled && PBR_PROFILE_PARALLAX_OCCLUSION),
                u8::from(renderer.cascaded_shadows_enabled),
            );
            let (point_sources, area_sources, total_sources) =
                renderer
                    .world
                    .as_ref()
                    .map_or((0_usize, 0_usize, 0_usize), |world| {
                        let area_sources = world
                            .dynamic_lights
                            .iter()
                            .filter(|light| {
                                light
                                    .emitter_normal
                                    .iter()
                                    .any(|component| component.abs() > 1.0e-6)
                            })
                            .count();
                        (
                            world.dynamic_lights.len().saturating_sub(area_sources),
                            area_sources,
                            world.dynamic_lights.len(),
                        )
                    });
            let (pipeline_variant_label, cached_pipeline_variants) =
                renderer.world.as_ref().map_or_else(
                    || (renderer.world_shader_variant_key().short_label(), 0_usize),
                    |world| {
                        (
                            world.active_pipeline_variant.short_label(),
                            world.pipeline_variants.len(),
                        )
                    },
                );
            println!(
                "[JKA PERF STATE] camera=[{:.2},{:.2},{:.2}] yaw={:.5} pitch={:.5} dlights={} point={} area={} gi={} world_path={} shader={} pipeline_variant={} cached_variants={} planar={} planar_active={} lights_total={} point_sources={} area_sources={} transient_sources={} active_lights={}",
                snapshot.camera.position.x,
                snapshot.camera.position.y,
                snapshot.camera.position.z,
                snapshot.camera.yaw,
                snapshot.camera.pitch,
                renderer.dynamic_lights_mode.config_value(),
                u8::from(renderer.point_lighting_enabled()),
                u8::from(renderer.any_area_lights_enabled()),
                u8::from(renderer.voxel_probe_gi_enabled),
                match renderer.frame_plan.world_path {
                    WorldRenderPath::FastBaseline => "fast-baseline",
                    WorldRenderPath::Advanced => "advanced",
                },
                if renderer.enhanced_world_shader_needed() {
                    "enhanced"
                } else {
                    "lean"
                },
                pipeline_variant_label,
                cached_pipeline_variants,
                renderer.planar_reflection_mode.label(),
                u8::from(renderer.planar_reflection.active),
                total_sources,
                point_sources,
                area_sources,
                renderer.transient_lights.len(),
                renderer.active_light_count(),
            );
            trace_started = Instant::now();
            trace_gpu_sum = [0.0; TRACE_GPU_PASSES.len()];
            trace_gpu_min = [f64::INFINITY; TRACE_GPU_PASSES.len()];
            trace_gpu_max = [0.0; TRACE_GPU_PASSES.len()];
            trace_gpu_n = [0; TRACE_GPU_PASSES.len()];
            trace_frames = 0;
            trace_frame_ms = 0.0;
            trace_prepare_ms = 0.0;
            trace_acquire_ms = 0.0;
            trace_encode_ms = 0.0;
            trace_submit_ms = 0.0;
            trace_present_ms = 0.0;
            trace_dynamic_model_ms = 0.0;
            trace_grass_cpu_ms = 0.0;
            trace_surface_sprite_cpu_ms = 0.0;
            trace_ocean_topology_ms=0.0;
            trace_ocean_topology_max_ms=0.0;
            trace_prep_stage_ms = [0.0; 4];
        }

        let diag_pacer_active = input_diagnostic.is_recording();
        let mut diag_pace_target_ms = 0.0_f64;
        let mut diag_pace_wait_ms = 0.0_f64;
        let mut diag_pace_wake_error_ms = 0.0_f64;
        let mut diag_pace_late_before_wait_ms = 0.0_f64;
        let mut diag_pace_anchor_reset = false;
        // A Studio export is a serial request/readback loop: pacing would add
        // up to a whole frame to each exported frame. Its preview is throttled.
        if fps_cap > 0 && renderer.studio_export_size.is_none() {
            let target = Duration::from_secs_f64(1.0 / fps_cap as f64);
            let deadline = frame_pace_anchor + target;
            let before_wait = Instant::now();
            let missed_deadline = before_wait > deadline;
            if diag_pacer_active {
                diag_pace_target_ms = target.as_secs_f64() * 1000.0;
                diag_pace_late_before_wait_ms = signed_instant_delta_ms(before_wait, deadline);
            }
            precise_frame_wait(deadline);
            // On time: keep the exact cadence. Late (frame slower than the cap,
            // cap changed, menu/pause transition): restart it rather than
            // shortening the next frame to catch up. The old code only reset
            // after being a *full additional frame* late, which created
            // long/short motion cadence after smaller misses.
            let now = Instant::now();
            if diag_pacer_active {
                diag_pace_wait_ms = now.saturating_duration_since(before_wait).as_secs_f64() * 1000.0;
                diag_pace_wake_error_ms = signed_instant_delta_ms(now, deadline);
            }
            diag_pace_anchor_reset = missed_deadline;
            frame_pace_anchor = if diag_pace_anchor_reset { now } else { deadline };
        } else {
            frame_pace_anchor = Instant::now();
        }

        if input_diagnostic.is_recording() {
            let diag_frame_completed = Instant::now();
            if diag_rendered {
                let latch = if input_latelatch {
                    last_info.late_latch
                } else {
                    frame_start_view_sample
                };
                let latch_phase = if latch.is_none() {
                    InputDiagLatchPhase::None
                } else if !input_latelatch {
                    InputDiagLatchPhase::FrameStart
                } else if last_info.late_latch_after_acquire {
                    InputDiagLatchPhase::PostAcquire
                } else {
                    InputDiagLatchPhase::PreAcquire
                };
                input_diagnostic.record_frame(InputDiagFrameSample {
                    frame_started: frame_loop_started,
                    frame_completed: diag_frame_completed,
                    frame_start_input_sequence: diag_frame_start_input.map(|sample| sample.sequence),
                    present_latest_input_sequence: diag_present_latest_input.map(|sample| sample.sequence),
                    latch,
                    latch_phase,
                    frame_ms: last_info.frame_ms,
                    cpu_prepare_ms: last_info.cpu_prepare_ms,
                    cpu_acquire_ms: last_info.cpu_acquire_ms,
                    cpu_encode_ms: last_info.cpu_encode_ms,
                    cpu_submit_ms: last_info.cpu_submit_ms,
                    cpu_present_ms: last_info.cpu_present_ms,
                    submit_call_at: last_info.submit_call_at,
                    present_call_completed_at: last_info.present_call_completed_at,
                    command_drain_count: diag_command_drain_count,
                    command_drain_ms: diag_command_drain_ms,
                    world_path: last_info.world_path,
                    fps_cap,
                    vsync: renderer.vsync.label(),
                    present_mode: present_mode_label(renderer.config.present_mode),
                    max_frame_latency: renderer.config.desired_maximum_frame_latency,
                    surface_width: renderer.config.width,
                    surface_height: renderer.config.height,
                    surface_config_generation: renderer.surface_config_generation,
                    surface_config_reason: renderer.surface_config_reason,
                    surface_config_age_ms: diag_frame_completed
                        .saturating_duration_since(renderer.surface_configured_at)
                        .as_secs_f64()
                        * 1000.0,
                    input_latelatch,
                    pace_target_ms: diag_pace_target_ms,
                    pace_wait_ms: diag_pace_wait_ms,
                    pace_wake_error_ms: diag_pace_wake_error_ms,
                    pace_late_before_wait_ms: diag_pace_late_before_wait_ms,
                    pace_anchor_reset: diag_pace_anchor_reset,
                    gpu_frame_ms: renderer.gpu_profiler.latest_frame_ms(),
                    gpu_sample_generation: renderer.gpu_profiler.latest_generation(),
                    gpu_timing: renderer.gpu_profiler.latest_timing(),
                    client_perf: snapshot.client_perf,
                });
            }
            input_diagnostic.maybe_finish(diag_frame_completed);
        }
    }
}

fn signed_instant_delta_ms(actual: Instant, target: Instant) -> f64 {
    if actual >= target {
        actual.duration_since(target).as_secs_f64() * 1000.0
    } else {
        -target.duration_since(actual).as_secs_f64() * 1000.0
    }
}

fn present_mode_label(mode: wgpu::PresentMode) -> &'static str {
    match mode {
        wgpu::PresentMode::Fifo => "FIFO",
        wgpu::PresentMode::FifoRelaxed => "FIFO_RELAXED",
        wgpu::PresentMode::Immediate => "IMMEDIATE",
        wgpu::PresentMode::Mailbox => "MAILBOX",
        wgpu::PresentMode::AutoNoVsync => "AUTO_NO_VSYNC",
        _ => "OTHER",
    }
}

pub(in crate::renderer) fn precise_frame_wait(deadline: Instant) {
    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let remaining = deadline - now;
        if remaining > Duration::from_millis(2) {
            thread::sleep(remaining - Duration::from_millis(1));
        } else if remaining > Duration::from_micros(150) {
            thread::yield_now();
        } else {
            std::hint::spin_loop();
        }
    }
}
