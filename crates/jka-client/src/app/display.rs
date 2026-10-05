//! Display.
use crate::app::{
    scene, ui, ActiveEventLoop, App, Arc, BTreeMap, BTreeSet, Duration, Fullscreen, FullscreenMode,
    Instant, OverlayMode, PerfStats, PhysicalPosition, PhysicalSize, ReflectionQuality,
    RenderCommand, RenderThread, RendererBackend, TextureFilter, VideoSettings, Window,
    FALLBACK_RESOLUTIONS, VIDEO_CONFIRM_TIMEOUT_SECS,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) struct AppliedVideoMode {
    pub(in crate::app) fullscreen: FullscreenMode,
    pub(in crate::app) renderer_backend: RendererBackend,
    pub(in crate::app) resolution: [u32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum ApplyLatchedScope {
    MapLoad,
    VidRestart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum ConsoleCvarSetResult {
    Applied,
    Latched,
    Unchanged,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::app) struct VideoConfirmation {
    pub(in crate::app) previous: AppliedVideoMode,
    pub(in crate::app) deadline: Instant,
    pub(in crate::app) shown_seconds: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum PendingRendererRestartStage {
    /// The old render thread was asked to exit; poll it from the event loop so
    /// the UI thread keeps pumping window messages while it tears down.
    WaitRenderExit,
    RecreateWindow,
    ApplyTargetDisplayMode,
    SpawnRenderer,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::app) struct PendingRendererRestart {
    pub(in crate::app) previous: AppliedVideoMode,
    pub(in crate::app) target: AppliedVideoMode,
    pub(in crate::app) confirm_on_change: bool,
    pub(in crate::app) stage: PendingRendererRestartStage,
    pub(in crate::app) ready_at: Instant,
}

/// How much of the renderer an Apply Video Settings request has to rebuild.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum ApplyVideoPath {
    /// Backend switch or a real exclusive display-mode change.
    RestartRenderer,
    /// Windowed/borderless and resolution changes on the running renderer.
    LiveDisplay,
    /// Only prepared map data changed.
    ReprepareMap,
}

#[derive(Debug, Clone)]
pub(in crate::app) struct PendingVideoChange {
    /// Stable UI-facing key used to associate this semantic change with its row.
    pub(in crate::app) key: String,
    pub(in crate::app) label: String,
    pub(in crate::app) detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum MapLoadPurpose {
    Gameplay,
    LivePrefetch,
    FrontendBackground,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::app) struct FrontendCinematic {
    pub(in crate::app) position: [f32; 3],
    pub(in crate::app) yaw: f32,
    /// Camera animation may start as soon as CPU preparation completes.
    pub(in crate::app) started: Instant,
    /// Start the reveal only after the renderer presents the uploaded world.
    /// Otherwise GPU upload time consumes part of the fade before it is visible.
    pub(in crate::app) fade_started: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AntiAliasingChoice {
    Off,
    Fxaa,
    Smaa,
    Taa,
    Msaa(u32),
}

pub(in crate::app) fn normalize_anti_aliasing(video: &mut VideoSettings) -> bool {
    let before = (video.msaa_samples, video.fxaa, video.smaa, video.taa);
    if video.taa {
        video.fxaa = false;
        video.smaa = false;
        video.msaa_samples = 1;
    } else if video.smaa {
        video.fxaa = false;
        video.msaa_samples = 1;
    } else if video.fxaa {
        video.msaa_samples = 1;
    }
    before != (video.msaa_samples, video.fxaa, video.smaa, video.taa)
}

impl App {
    pub(in crate::app) fn exclusive_video_mode(
        window: &Window,
        resolution: [u32; 2],
    ) -> Option<winit::monitor::VideoModeHandle> {
        let monitor = window
            .current_monitor()
            .or_else(|| window.primary_monitor())?;
        let target_size = PhysicalSize::new(resolution[0], resolution[1]);
        let target_refresh = monitor.refresh_rate_millihertz().unwrap_or(0);
        monitor
            .video_modes()
            .filter(|mode| mode.size() == target_size)
            .min_by_key(|mode| {
                (
                    target_refresh.abs_diff(mode.refresh_rate_millihertz()),
                    std::cmp::Reverse(mode.bit_depth()),
                )
            })
    }

    pub(in crate::app) fn native_fullscreen_mode(
        mode: FullscreenMode,
        backend: RendererBackend,
    ) -> FullscreenMode {
        if mode == FullscreenMode::Exclusive && backend == RendererBackend::Dx12 {
            FullscreenMode::Borderless
        } else {
            mode
        }
    }

    pub(in crate::app) fn apply_fullscreen_mode(
        window: &Window,
        mode: FullscreenMode,
        resolution: [u32; 2],
        backend: RendererBackend,
    ) -> Result<(), String> {
        match mode {
            FullscreenMode::Windowed => {
                window.set_fullscreen(None);
                Ok(())
            }
            FullscreenMode::Borderless => {
                window.set_fullscreen(Some(Fullscreen::Borderless(None)));
                Ok(())
            }
            FullscreenMode::Exclusive if backend == RendererBackend::Dx12 => {
                // Direct3D 12 does not expose classic fullscreen-exclusive mode.
                // wgpu's DX12 surface is an HWND flip-model swapchain, and putting
                // that HWND into Winit's legacy Exclusive state makes
                // CreateSwapChainForHwnd/Surface::configure invalid on Windows.
                // Use the D3D12-native fullscreen path instead: a monitor-sized
                // borderless HWND that Windows may promote through FSO/DirectFlip/
                // Independent Flip. Keep the logical "Exclusive" selection so
                // Vulkan can still use a real monitor-mode switch when selected.
                window.set_fullscreen(Some(Fullscreen::Borderless(None)));
                Ok(())
            }
            FullscreenMode::Exclusive => {
                let video_mode =
                    Self::exclusive_video_mode(window, resolution).ok_or_else(|| {
                        format!(
                            "No exclusive fullscreen video mode matches {}x{}",
                            resolution[0], resolution[1]
                        )
                    })?;
                window.set_fullscreen(Some(Fullscreen::Exclusive(video_mode)));
                Ok(())
            }
        }
    }

    pub(in crate::app) fn set_fullscreen_mode(&mut self, mode: FullscreenMode) {
        if mode == self.video.fullscreen {
            return;
        }

        self.video.fullscreen = mode;
        self.mark_config_dirty();
        self.console_status = format!(
            "FULLSCREEN: {} (PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART)",
            mode.label()
        );
        self.publish_ui();
    }

    pub(in crate::app) fn cycle_fullscreen(&mut self, direction: i32) {
        let modes: &[FullscreenMode] = &[
            FullscreenMode::Windowed,
            FullscreenMode::Borderless,
            FullscreenMode::Exclusive,
        ];
        let current = modes
            .iter()
            .position(|mode| *mode == self.video.fullscreen)
            .unwrap_or(0) as i32;
        let next = (current + direction).rem_euclid(modes.len() as i32) as usize;
        self.set_fullscreen_mode(modes[next]);
    }

    /// Alt+Enter is a runtime display-mode shortcut, not a staged Video-menu edit.
    /// Toggle the currently applied fullscreen presentation between borderless and
    /// exclusive and use the same safe transition path as Apply Video Settings.
    pub(in crate::app) fn toggle_borderless_exclusive(&mut self) {
        if self.video_restart_in_flight() {
            self.console_status =
                "ALT+ENTER: VIDEO SETTINGS ARE STILL APPLYING OR AWAITING CONFIRMATION".into();
            self.publish_ui();
            return;
        }

        let target = match self.applied_fullscreen {
            FullscreenMode::Exclusive => FullscreenMode::Borderless,
            FullscreenMode::Borderless => FullscreenMode::Exclusive,
            // If a config or command left us windowed, Alt+Enter enters the
            // non-mode-switching fullscreen path first. Subsequent presses then
            // alternate strictly between Borderless and Exclusive.
            FullscreenMode::Windowed => FullscreenMode::Borderless,
        };

        // The shortcut changes only fullscreen presentation. Do not accidentally
        // apply an uncommitted resolution/backend selection from the Video menu.
        self.video.renderer_backend = self.applied_renderer_backend;
        self.video.resolution = self.applied_resolution;

        if target == FullscreenMode::Exclusive
            && self.applied_renderer_backend != RendererBackend::Dx12
        {
            let Some(window) = self.window.as_ref() else {
                self.console_status = "ALT+ENTER: WINDOW IS NOT READY".into();
                self.publish_ui();
                return;
            };
            if Self::exclusive_video_mode(window, self.applied_resolution).is_none() {
                self.console_status = format!(
                    "ALT+ENTER: NO EXCLUSIVE VIDEO MODE MATCHES {}X{}",
                    self.applied_resolution[0], self.applied_resolution[1]
                );
                self.push_console_line(format!("^1{}", self.console_status));
                self.publish_ui();
                return;
            }
        }

        self.video.fullscreen = target;
        self.mark_config_dirty();

        // DX12's logical Exclusive mode is borderless natively, so this remains a
        // live HWND transition. Vulkan exclusive owns a monitor mode and therefore
        // goes through the existing renderer/surface restart state machine.
        if self.live_display_change_possible() {
            self.apply_display_live(false);
        } else {
            self.restart_renderer_internal(false);
        }
    }

    pub(in crate::app) fn available_resolutions(
        window: Option<&Window>,
        current: [u32; 2],
    ) -> Vec<[u32; 2]> {
        let mut unique = BTreeSet::new();

        if let Some(window) = window {
            if let Some(monitor) = window
                .current_monitor()
                .or_else(|| window.primary_monitor())
            {
                // A monitor usually reports the same pixel dimensions once per
                // refresh-rate / bit-depth combination. Collapse those down to
                // one menu entry per actual resolution. Keep tiny legacy modes
                // out of the normal list, but always retain the monitor's native
                // size below.
                for mode in monitor.video_modes() {
                    let size = mode.size();
                    if size.width >= 1024 && size.height >= 720 {
                        unique.insert((size.width, size.height));
                    }
                }

                let native = monitor.size();
                if native.width > 0 && native.height > 0 {
                    unique.insert((native.width, native.height));
                }
            }
        }

        if unique.is_empty() {
            unique.extend(
                FALLBACK_RESOLUTIONS
                    .iter()
                    .map(|resolution| (resolution[0], resolution[1])),
            );
        }

        // Preserve a custom/configured window size even when it is not an
        // advertised exclusive display mode, so the first arrow press advances
        // naturally from what the user currently has selected.
        unique.insert((current[0], current[1]));

        let mut resolutions: Vec<_> = unique
            .into_iter()
            .map(|(width, height)| [width, height])
            .collect();
        resolutions.sort_unstable_by_key(|resolution| {
            (
                u64::from(resolution[0]) * u64::from(resolution[1]),
                resolution[0],
                resolution[1],
            )
        });
        resolutions
    }

    pub(in crate::app) fn cycle_resolution(&mut self, direction: i32) {
        let resolutions =
            Self::available_resolutions(self.window.as_deref(), self.video.resolution);
        let current = resolutions
            .iter()
            .position(|value| *value == self.video.resolution)
            .unwrap_or(0) as i32;
        let next = (current + direction).rem_euclid(resolutions.len() as i32) as usize;
        self.video.resolution = resolutions[next];
        self.mark_config_dirty();
        self.console_status = format!(
            "RESOLUTION: {}X{} (PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART)",
            self.video.resolution[0], self.video.resolution[1]
        );
    }

    /// The anti-aliasing ladder, ordered by the menu's intended quality scale:
    /// Off/FXAA first, then the adapter-supported MSAA levels, then SMAA and TAA.
    /// TAA remains the final option because it also resolves shader/specular
    /// aliasing across frames. MSAA levels are probed per adapter, so that part
    /// of the ladder is built fresh rather than kept in a const.
    pub(crate) fn anti_aliasing_ladder(&self) -> Vec<(AntiAliasingChoice, String)> {
        let mut choices = vec![
            (AntiAliasingChoice::Off, "Off".to_owned()),
            (AntiAliasingChoice::Fxaa, "FXAA".to_owned()),
        ];
        choices.extend(
            self.supported_msaa
                .iter()
                .copied()
                .filter(|samples| *samples > 1)
                .map(|samples| {
                    (
                        AntiAliasingChoice::Msaa(samples),
                        format!("{samples}× MSAA"),
                    )
                }),
        );
        choices.push((AntiAliasingChoice::Smaa, "SMAA".to_owned()));
        choices.push((AntiAliasingChoice::Taa, "TAA".to_owned()));
        choices
    }

    pub(crate) fn current_anti_aliasing(&self) -> AntiAliasingChoice {
        if self.video.taa {
            AntiAliasingChoice::Taa
        } else if self.video.smaa {
            AntiAliasingChoice::Smaa
        } else if self.video.fxaa {
            AntiAliasingChoice::Fxaa
        } else if self.video.msaa_samples > 1 {
            AntiAliasingChoice::Msaa(self.video.msaa_samples)
        } else {
            AntiAliasingChoice::Off
        }
    }

    pub(in crate::app) fn cycle_anti_aliasing(&mut self, direction: i32) {
        let choices: Vec<AntiAliasingChoice> = self
            .anti_aliasing_ladder()
            .into_iter()
            .map(|(choice, _)| choice)
            .collect();
        let current = self.current_anti_aliasing();
        let current_index = choices
            .iter()
            .position(|choice| *choice == current)
            .unwrap_or(0) as i32;
        let next_index = (current_index + direction).rem_euclid(choices.len() as i32) as usize;
        let next = choices[next_index];

        let previous_msaa = self.video.msaa_samples;
        self.video.msaa_samples = 1;
        self.video.fxaa = false;
        self.video.smaa = false;
        self.video.taa = false;
        match next {
            AntiAliasingChoice::Off => {}
            AntiAliasingChoice::Fxaa => self.video.fxaa = true,
            AntiAliasingChoice::Smaa => self.video.smaa = true,
            AntiAliasingChoice::Taa => self.video.taa = true,
            AntiAliasingChoice::Msaa(samples) => self.video.msaa_samples = samples,
        }

        if self.video.msaa_samples != previous_msaa {
            self.render_command(RenderCommand::SetMsaa(self.video.msaa_samples));
        }
        self.sync_post_effects();
        self.mark_config_dirty();
    }

    pub(in crate::app) fn cycle_texture_filter(&mut self, direction: i32) {
        const MODES: [TextureFilter; 7] = [
            TextureFilter::Nearest,
            TextureFilter::Bilinear,
            TextureFilter::Trilinear,
            TextureFilter::Anisotropic2x,
            TextureFilter::Anisotropic4x,
            TextureFilter::Anisotropic8x,
            TextureFilter::Anisotropic16x,
        ];
        let current = MODES
            .iter()
            .position(|mode| *mode == self.video.texture_filter)
            .unwrap_or(1) as i32;
        let next = (current + direction).rem_euclid(MODES.len() as i32) as usize;
        self.video.texture_filter = MODES[next];
        self.render_command(RenderCommand::SetTextureFilter(self.video.texture_filter));
        self.mark_config_dirty();
    }

    pub(in crate::app) fn cycle_renderer_backend(&mut self, direction: i32) {
        const BACKENDS: [RendererBackend; 2] = [RendererBackend::Vulkan, RendererBackend::Dx12];
        let current = BACKENDS
            .iter()
            .position(|backend| *backend == self.video.renderer_backend)
            .unwrap_or(0) as i32;
        let next = (current + direction).rem_euclid(BACKENDS.len() as i32) as usize;
        self.video.renderer_backend = BACKENDS[next];
        self.mark_config_dirty();
        self.console_status = format!(
            "RENDER BACKEND: {} (PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART)",
            self.video.renderer_backend.label()
        );
    }

    pub(in crate::app) fn applied_video_mode(&self) -> AppliedVideoMode {
        AppliedVideoMode {
            fullscreen: self.applied_fullscreen,
            renderer_backend: self.applied_renderer_backend,
            resolution: self.applied_resolution,
        }
    }

    /// Prepare options of the map currently loaded (or being shown behind the
    /// front end), or `None` when there is no prepared world to compare against.
    pub(in crate::app) fn loaded_map_prepare_options(&self) -> Option<scene::MapPrepareOptions> {
        if self.front_end && self.frontend_cinematic.is_none() {
            return None;
        }
        let label = self.initial_source.label();
        self.prepared_map_cache
            .as_ref()
            .filter(|cache| cache.label == label)
            .map(|cache| cache.prepare_options)
    }

    /// (planar_reflections, planar_environment, omit_environment_stages)
    pub(in crate::app) fn reflection_prep_flags(quality: ReflectionQuality) -> (bool, bool, bool) {
        (
            quality.planar_slot_budget() > 0,
            quality.promotes_environment_planars(),
            quality.omits_environment_stages(),
        )
    }

    /// A map prepared with reflection-plane topology keeps a superset of what
    /// any lower planar tier needs (extra plane splits only cost draw calls), so
    /// quality can drop live. Going up, or across the Off boundary that
    /// specializes the material set, needs the map prepared again.
    pub(in crate::app) fn reflection_prep_serves(
        prepared: scene::MapPrepareOptions,
        requested: (bool, bool, bool),
    ) -> bool {
        prepared.omit_environment_stages == requested.2
            && (prepared.planar_reflections || !requested.0)
            && (prepared.planar_environment || !requested.1)
    }

    /// Reflection quality the renderer should actually run. This is the
    /// requested quality when the loaded map can serve it; otherwise the nearest
    /// quality it can, so a change is never held back at the old value while the
    /// map is being prepared again.
    pub(in crate::app) fn effective_reflection_quality(&self) -> ReflectionQuality {
        let requested = self.video.reflection_quality;
        let Some(prepared) = self.loaded_map_prepare_options() else {
            return requested;
        };
        let serves = |quality: &ReflectionQuality| {
            Self::reflection_prep_serves(prepared, Self::reflection_prep_flags(*quality))
        };
        if serves(&requested) {
            return requested;
        }
        ReflectionQuality::ALL
            .iter()
            .rev()
            .copied()
            .filter(|quality| *quality < requested)
            .find(serves)
            .or_else(|| {
                ReflectionQuality::ALL
                    .iter()
                    .copied()
                    .filter(|quality| *quality > requested)
                    .find(serves)
            })
            .unwrap_or(requested)
    }

    /// Push a changed reflection quality to the renderer and say whether it took
    /// effect fully or is waiting for the map to be prepared again.
    pub(in crate::app) fn reflection_quality_changed(&mut self) {
        self.sync_post_effects();
        self.mark_config_dirty();
        let effective = self.effective_reflection_quality();
        self.console_status = if effective == self.video.reflection_quality {
            format!(
                "REFLECTION QUALITY: {} (APPLIED LIVE)",
                self.video.reflection_quality.label()
            )
        } else {
            format!(
                "REFLECTION QUALITY: {} - RUNNING {} UNTIL APPLY VIDEO SETTINGS OR VID_RESTART",
                self.video.reflection_quality.label(),
                effective.label()
            )
        };
    }

    pub(in crate::app) fn map_prepare_restart_required(&self) -> bool {
        let Some(prepared) = self.loaded_map_prepare_options() else {
            return false;
        };
        let requested = self.active_map_prepare_options();

        // GI can be disabled live because its prepared volume remains resident;
        // only enabling it on a map that was prepared without GI needs a rebuild.
        // Reflection-plane topology likewise only matters when it must grow.
        // Generated normals, FP16 lightmaps, PBR material-library selection, and
        // asset-override policy alter prepared material/texture data in both
        // directions, so any mismatch is restart-sensitive.
        (requested.voxel_probe_gi && !prepared.voxel_probe_gi)
            || requested.picmip != prepared.picmip
            || requested.gen_normal_maps != prepared.gen_normal_maps
            || requested.float_lightmap != prepared.float_lightmap
            || !Self::reflection_prep_serves(
                prepared,
                (
                    requested.planar_reflections,
                    requested.planar_environment,
                    requested.omit_environment_stages,
                ),
            )
            || requested.pbr_materials != prepared.pbr_materials
            || requested.allow_asset_overrides != prepared.allow_asset_overrides
    }

    /// The only staged changes that genuinely need the renderer torn down and
    /// rebuilt: the graphics backend and the display mode/resolution.
    pub(in crate::app) fn display_restart_required(&self) -> bool {
        self.video.fullscreen != self.applied_fullscreen
            || self.video.renderer_backend != self.applied_renderer_backend
            || self.video.resolution != self.applied_resolution
    }

    /// Staged options whose only effect is on CPU-prepared map data.
    pub(in crate::app) fn map_reprep_wanted(&self) -> bool {
        self.loaded_map_prepare_options().is_some()
            && (self.map_prepare_restart_required()
                || (self.video.grass && !self.applied_grass)
                || (self.video.ocean && !self.applied_ocean))
    }

    pub(in crate::app) fn video_restart_required(&self) -> bool {
        self.display_restart_required()
            // Disabling these effects is a live operation. Enabling still needs
            // a restart only when this renderer/map was started without the
            // resources needed to draw them.
            || (self.video.grass && !self.applied_grass)
            || (self.video.ocean && !self.applied_ocean)
            || self.map_prepare_restart_required()
            || !self.latched_console_cvars.is_empty()
    }

    /// Semantic settings that the next Apply Video Settings / vid_restart will
    /// actually consume. Keep this derived from the same applied/prepared state
    /// as `video_restart_required()` so the footer count and tooltip cannot drift
    /// into being a second, UI-only dirty flag.
    pub(in crate::app) fn pending_video_changes(&self) -> Vec<PendingVideoChange> {
        let mut changes = BTreeMap::<String, PendingVideoChange>::new();
        let mut add = |key: &str, label: &str, detail: String| {
            changes.insert(
                key.to_owned(),
                PendingVideoChange {
                    key: key.to_owned(),
                    label: label.to_owned(),
                    detail,
                },
            );
        };
        let on_off = |enabled: bool| if enabled { "On" } else { "Off" };

        if self.video.renderer_backend != self.applied_renderer_backend {
            add(
                "render_backend",
                "Render backend",
                format!(
                    "{} -> {}",
                    self.applied_renderer_backend.label(),
                    self.video.renderer_backend.label()
                ),
            );
        }
        if self.video.fullscreen != self.applied_fullscreen {
            add(
                "display_mode",
                "Display mode",
                format!(
                    "{} -> {}",
                    self.applied_fullscreen.label(),
                    self.video.fullscreen.label()
                ),
            );
        }
        if self.video.resolution != self.applied_resolution {
            add(
                "resolution",
                "Resolution",
                format!(
                    "{} x {} -> {} x {}",
                    self.applied_resolution[0],
                    self.applied_resolution[1],
                    self.video.resolution[0],
                    self.video.resolution[1]
                ),
            );
        }

        // Grass/ocean retain their resources while disabled. Only a requested
        // enable that the active renderer/map was not prepared for is staged.
        if self.video.grass && !self.applied_grass {
            add("grass", "Procedural grass", "Off -> On".to_owned());
        }
        if self.video.ocean && !self.applied_ocean {
            add("ocean", "Ocean", "Off -> On".to_owned());
        }

        if let Some(prepared) = self.loaded_map_prepare_options() {
            let requested = self.active_map_prepare_options();
            if requested.picmip != prepared.picmip {
                add(
                    "picmip",
                    "Texture quality",
                    format!("r_picmip {} -> {}", prepared.picmip, requested.picmip),
                );
            }
            if requested.voxel_probe_gi && !prepared.voxel_probe_gi {
                add("voxel_probe_gi", "Voxel / probe GI", "Off -> On".to_owned());
            }
            if requested.gen_normal_maps != prepared.gen_normal_maps {
                add(
                    "gen_normal_maps",
                    "Generated normal maps",
                    format!(
                        "{} -> {}",
                        on_off(prepared.gen_normal_maps),
                        on_off(requested.gen_normal_maps)
                    ),
                );
            }
            if requested.float_lightmap != prepared.float_lightmap {
                add(
                    "float_lightmap",
                    "Float lightmaps",
                    format!(
                        "{} -> {}",
                        on_off(prepared.float_lightmap),
                        on_off(requested.float_lightmap)
                    ),
                );
            }
            let requested_reflections = (
                requested.planar_reflections,
                requested.planar_environment,
                requested.omit_environment_stages,
            );
            if !Self::reflection_prep_serves(prepared, requested_reflections) {
                add(
                    "reflections",
                    "Reflection quality",
                    format!(
                        "Running {} -> requested {}",
                        self.effective_reflection_quality().label(),
                        self.video.reflection_quality.label()
                    ),
                );
            }
            if requested.pbr_materials != prepared.pbr_materials {
                add(
                    "pbr",
                    "Physically based rendering (PBR)",
                    format!(
                        "{} -> {}",
                        on_off(prepared.pbr_materials),
                        on_off(requested.pbr_materials)
                    ),
                );
            }
            if requested.allow_asset_overrides != prepared.allow_asset_overrides {
                add(
                    "asset_overrides",
                    "Asset overrides",
                    format!(
                        "{} -> {}",
                        on_off(prepared.allow_asset_overrides),
                        on_off(requested.allow_asset_overrides)
                    ),
                );
            }
        }

        // Console latches are another way to stage the same settings. Resolve
        // their target exactly as Apply does, then overwrite an existing semantic
        // entry instead of double-counting e.g. width + height as two changes.
        if !self.latched_console_cvars.is_empty() {
            let mut target = self.video;
            self.apply_latched_values_to_settings(&mut target);
            for (name, value) in &self.latched_console_cvars {
                match name.as_str() {
                    "r_backend" => add(
                        "render_backend",
                        "Render backend",
                        format!(
                            "{} -> {}",
                            self.applied_renderer_backend.label(),
                            target.renderer_backend.label()
                        ),
                    ),
                    "r_fullscreen" => add(
                        "display_mode",
                        "Display mode",
                        format!(
                            "{} -> {}",
                            self.applied_fullscreen.label(),
                            target.fullscreen.label()
                        ),
                    ),
                    "r_customwidth" | "r_customheight" => add(
                        "resolution",
                        "Resolution",
                        format!(
                            "{} x {} -> {} x {}",
                            self.applied_resolution[0],
                            self.applied_resolution[1],
                            target.resolution[0],
                            target.resolution[1]
                        ),
                    ),
                    "r_pbr" => add(
                        "pbr",
                        "Physically based rendering (PBR)",
                        format!("pending console value: {}", on_off(target.pbr)),
                    ),
                    "r_gennormalmaps" => add(
                        "gen_normal_maps",
                        "Generated normal maps",
                        format!("pending console value: {}", on_off(target.gen_normal_maps)),
                    ),
                    "r_floatlightmap" => add(
                        "float_lightmap",
                        "Float lightmaps",
                        format!("pending console value: {}", on_off(target.float_lightmap)),
                    ),
                    "fs_allowassetoverrides" => add(
                        "asset_overrides",
                        "Asset overrides",
                        format!(
                            "pending console value: {}",
                            on_off(target.allow_asset_overrides)
                        ),
                    ),
                    _ => {
                        let label = crate::console::find(name)
                            .map(|entry| entry.name)
                            .unwrap_or(name.as_str());
                        add(
                            &format!("cvar:{name}"),
                            label,
                            format!("pending value: {value}"),
                        );
                    }
                }
            }
        }

        changes.into_values().collect()
    }

    pub(in crate::app) fn video_confirmation_seconds(&self) -> Option<u32> {
        let confirmation = self.video_confirmation?;
        let remaining = confirmation
            .deadline
            .saturating_duration_since(Instant::now());
        let seconds = remaining.as_secs() + if remaining.subsec_nanos() != 0 { 1 } else { 0 };
        Some(seconds.min(u32::MAX as u64) as u32)
    }

    pub(in crate::app) fn begin_pending_video_confirmation(&mut self) {
        let Some(previous) = self.pending_video_confirmation.take() else {
            return;
        };
        let seconds = VIDEO_CONFIRM_TIMEOUT_SECS as u32;
        self.video_confirmation = Some(VideoConfirmation {
            previous,
            deadline: Instant::now() + Duration::from_secs(VIDEO_CONFIRM_TIMEOUT_SECS),
            shown_seconds: seconds,
        });
        let applied = self.applied_video_mode();
        self.console_status = format!(
            "VIDEO CHANGED TO {}X{} {} / {} - CONFIRM WITHIN {seconds} SECONDS",
            applied.resolution[0],
            applied.resolution[1],
            applied.fullscreen.label(),
            applied.renderer_backend.label()
        );
        self.push_console_line(format!("^3{}", self.console_status));
        self.egui_repaint_requested = true;
    }

    pub(in crate::app) fn normalize_video_selection(&mut self) {
        if !self.video_restart_required() && self.video_selected == ui::VIDEO_ROW_VID_RESTART {
            self.video_selected = ui::VIDEO_ROW_RENDER_BACKEND;
        }
        if !self.video_restart_required() && self.environment_selected == ui::ENV_ROW_VID_RESTART {
            self.environment_selected = ui::ENV_ROW_OCEAN_SETTINGS;
        }
    }

    pub(in crate::app) fn confirm_video_settings(&mut self) {
        if self.video_confirmation.take().is_none() {
            return;
        }
        self.normalize_video_selection();
        self.console_status = format!(
            "VIDEO SETTINGS KEPT: {}X{} {} / {}",
            self.applied_resolution[0],
            self.applied_resolution[1],
            self.applied_fullscreen.label(),
            self.applied_renderer_backend.label()
        );
        self.push_console_line(format!("^2{}", self.console_status));
        self.mark_config_dirty();
        self.flush_config();
        if self.overlay == OverlayMode::None {
            self.set_capture(true);
        }
        self.publish_ui();
    }

    pub(in crate::app) fn revert_video_settings(&mut self, reason: &str) {
        // Persist any unrelated dirty settings while the confirmation object still
        // identifies the last known-good video mode. Do not lie to the runtime by
        // overwriting applied_* before the HWND/display mode has actually rolled back.
        self.flush_config();
        let previous = if let Some(confirmation) = self.video_confirmation.take() {
            confirmation.previous
        } else if let Some(previous) = self.pending_video_confirmation.take() {
            previous
        } else {
            return;
        };
        self.video.fullscreen = previous.fullscreen;
        self.video.renderer_backend = previous.renderer_backend;
        self.video.resolution = previous.resolution;
        self.normalize_video_selection();
        self.console_status = format!("VIDEO SETTINGS {reason}; REVERTING...");
        self.push_console_line(format!("^3{}", self.console_status));
        if self.live_display_change_possible() {
            self.apply_display_live(false);
        } else {
            self.restart_renderer_internal(false);
        }
    }

    pub(in crate::app) fn apply_pending_display_settings(
        &mut self,
        window: &Window,
        mode: FullscreenMode,
    ) -> Result<(), String> {
        match mode {
            FullscreenMode::Windowed => {
                window.set_fullscreen(None);
                if let Some([x, y]) = self.video.window_position {
                    window.set_outer_position(PhysicalPosition::new(x, y));
                }
                if self.video.window_maximized {
                    window.set_maximized(true);
                } else {
                    window.set_maximized(false);
                    let _ = window.request_inner_size(PhysicalSize::new(
                        self.video.resolution[0],
                        self.video.resolution[1],
                    ));
                }
            }
            _ => Self::apply_fullscreen_mode(
                window,
                mode,
                self.video.resolution,
                self.video.renderer_backend,
            )?,
        }

        self.applied_fullscreen = mode;
        self.applied_resolution = self.video.resolution;
        Ok(())
    }

    pub(in crate::app) fn restart_renderer(&mut self) {
        if self.video_restart_in_flight() {
            self.console_status =
                "VIDEO SETTINGS ARE STILL APPLYING OR AWAITING CONFIRMATION".into();
            self.publish_ui();
            return;
        }
        self.apply_latched_console_cvars(ApplyLatchedScope::VidRestart);
        self.restart_renderer_internal(true);
    }

    /// First half of Apply Video Settings: settle any console latches, then say
    /// how much has to be rebuilt. `None` means an earlier apply is still running.
    pub(in crate::app) fn begin_apply_video_settings(&mut self) -> Option<ApplyVideoPath> {
        if self.video_restart_in_flight() {
            self.console_status =
                "VIDEO SETTINGS ARE STILL APPLYING OR AWAITING CONFIRMATION".into();
            self.publish_ui();
            return None;
        }
        self.apply_latched_console_cvars(ApplyLatchedScope::VidRestart);
        Some(if !self.display_restart_required() {
            ApplyVideoPath::ReprepareMap
        } else if self.live_display_change_possible() {
            ApplyVideoPath::LiveDisplay
        } else {
            ApplyVideoPath::RestartRenderer
        })
    }

    /// Apply Video Settings. Unlike `vid_restart`, this only tears the renderer
    /// down for a backend change or a real exclusive-mode switch. Windowed and
    /// borderless changes resize the running renderer; everything else staged is
    /// prepared map data rebuilt on the map worker.
    pub(in crate::app) fn apply_video_settings(&mut self) {
        match self.begin_apply_video_settings() {
            Some(ApplyVideoPath::RestartRenderer) => self.restart_renderer_internal(true),
            Some(ApplyVideoPath::LiveDisplay) => self.apply_display_live(true),
            Some(ApplyVideoPath::ReprepareMap) => self.reprepare_map_in_place(),
            None => {}
        }
    }

    /// True when the requested display change can be made on the running
    /// renderer: same backend, and neither side is a real monitor-mode switch
    /// (Vulkan exclusive), which invalidates the live surface. DX12 "Exclusive"
    /// is already a borderless window natively, so it qualifies.
    pub(in crate::app) fn live_display_change_possible(&self) -> bool {
        let backend = self.applied_renderer_backend;
        self.render.is_some()
            && self.window.is_some()
            && self.video.renderer_backend == backend
            && [self.applied_fullscreen, self.video.fullscreen]
                .into_iter()
                .all(|mode| {
                    Self::native_fullscreen_mode(mode, backend) != FullscreenMode::Exclusive
                })
    }

    pub(in crate::app) fn display_transition_active(&self) -> bool {
        self.display_transition_until
            .is_some_and(|until| Instant::now() < until)
    }

    /// Change window mode/size without touching the device, surface or map. The
    /// resulting Resized event drives the renderer's normal live resize.
    pub(in crate::app) fn apply_display_live(&mut self, confirm_on_change: bool) {
        self.restore_gamma_before_restart();
        let Some(window) = self.window.clone() else {
            return;
        };
        let previous = self.applied_video_mode();
        let mode = self.video.fullscreen;

        // Remember windowed placement before it is left, as a restart would.
        if self.applied_fullscreen.is_windowed() && !mode.is_windowed() {
            self.video.window_maximized = window.is_maximized();
            if !self.video.window_maximized {
                if let Ok(position) = window.outer_position() {
                    self.video.window_position = Some([position.x, position.y]);
                }
            }
            self.mark_config_dirty();
        }

        self.display_transition_until = Some(Instant::now() + Duration::from_millis(750));
        if let Err(error) = self.apply_pending_display_settings(&window, mode) {
            self.display_transition_until = None;
            self.push_console_line(format!(
                "^3Live display change failed ({error}); restarting the renderer instead"
            ));
            self.restart_renderer_internal(confirm_on_change);
            return;
        }

        let applied = self.applied_video_mode();
        if confirm_on_change && applied != previous {
            self.pending_video_confirmation = Some(previous);
            self.set_capture(false);
            self.begin_pending_video_confirmation();
        } else {
            self.console_status = format!(
                "VIDEO APPLIED: {}X{} {} / {}",
                applied.resolution[0],
                applied.resolution[1],
                applied.fullscreen.label(),
                applied.renderer_backend.label()
            );
            self.push_console_line(format!("^2{}", self.console_status));
            self.mark_config_dirty();
            self.flush_config();
        }
        self.normalize_video_selection();
        self.egui_repaint_requested = true;
        self.publish_ui();
    }

    /// Once a live display transition has settled, adopt the size Windows
    /// actually gave a windowed window (it may clamp the request).
    pub(in crate::app) fn tick_display_transition(&mut self, now: Instant) {
        let Some(until) = self.display_transition_until else {
            return;
        };
        if now < until {
            return;
        }
        self.display_transition_until = None;
        let Some(window) = self.window.as_ref() else {
            return;
        };
        if self.applied_fullscreen.is_windowed() && !window.is_maximized() {
            let size = window.inner_size();
            if size.width > 0 && size.height > 0 {
                let actual = [size.width, size.height];
                if actual != self.applied_resolution && self.video_confirmation.is_none() {
                    self.video.resolution = actual;
                    self.applied_resolution = actual;
                    self.mark_config_dirty();
                }
            }
        }
        self.sync_gamma_focus();
    }

    /// Rebuild the loaded map with the current prepare options on the map worker
    /// and swap it in, without recreating the device, surface, window or game
    /// state. The same in-place swap the map editor preview and the vid_restart
    /// cache-miss fallback use.
    pub(in crate::app) fn reprepare_map_in_place(&mut self) {
        if self.loaded_map_prepare_options().is_none() {
            // No prepared world holds anything the staged options could
            // invalidate; the next map load prepares with them.
            self.applied_grass = self.video.grass;
            self.applied_ocean = self.video.ocean;
            self.publish_ui();
            return;
        }
        if !self.map_reprep_wanted() {
            self.publish_ui();
            return;
        }
        let source = self.initial_source.clone();
        self.console_status =
            "APPLYING MAP SETTINGS: REBUILDING THE WORLD ON THE MAP WORKER...".into();
        self.push_console_line(format!("^5{}", self.console_status));
        if self.front_end {
            self.request_frontend_background();
        } else {
            self.preserve_game_state_on_next_map_upload = true;
            self.request_map(source);
        }
    }

    /// Reveal the window that a restart hid once the replacement renderer has
    /// finished device/surface setup. Safe to call at any time; it only acts when
    /// a restart is actually holding the window hidden.
    pub(in crate::app) fn show_window_after_restart(&mut self) {
        if !self.window_hidden_for_restart {
            return;
        }
        self.window_hidden_for_restart = false;
        if let Some(window) = &self.window {
            window.set_visible(true);
            window.focus_window();
            window.request_redraw();
        }
    }

    /// True between the start of a renderer/display restart and the moment the
    /// user has kept or reverted the resulting mode.
    pub(in crate::app) fn video_restart_in_flight(&self) -> bool {
        self.pending_renderer_restart.is_some()
            || self.pending_video_confirmation.is_some()
            || self.video_confirmation.is_some()
    }

    pub(in crate::app) fn set_requested_video_mode(&mut self, target: AppliedVideoMode) {
        self.video.fullscreen = target.fullscreen;
        self.video.renderer_backend = target.renderer_backend;
        self.video.resolution = target.resolution;
    }

    pub(in crate::app) fn restart_display_apply_failed(
        &mut self,
        previous: AppliedVideoMode,
        confirm_on_change: bool,
        error: String,
    ) {
        self.renderer_restart_started = None;
        self.show_window_after_restart();
        self.console_status = format!("VID_RESTART DISPLAY APPLY FAILED: {error}");
        self.push_console_line(format!("^1{}", self.console_status));
        if confirm_on_change {
            self.set_requested_video_mode(previous);
            self.push_console_line("^3RESTORING PREVIOUS VIDEO SETTINGS...".to_owned());
            self.restart_renderer_internal(false);
        } else {
            self.publish_ui();
        }
    }

    pub(in crate::app) fn spawn_restarted_renderer(
        &mut self,
        window: Arc<Window>,
        previous: AppliedVideoMode,
        confirm_on_change: bool,
    ) {
        self.perf = PerfStats::default();
        self.supported_msaa = vec![1];
        self.wireframe_supported = false;
        self.reset_egui_for_window(&window);
        let initial = self.render_snapshot();
        let initial_ui = self.ui_snapshot();
        match RenderThread::spawn(
            window,
            self.proxy.clone(),
            self.video.renderer_backend,
            initial,
            initial_ui,
            self.base.clone(),
            self.game.clone(),
            "auto".to_owned(),
        ) {
            Ok(render) => {
                self.attach_companion_to_renderer(&render);
                self.render = Some(render);
                // Renderer restarts recreate these lightweight runtime flags;
                // immediately restore the classic world-lighting controls from
                // the retained VideoSettings state.
                self.sync_classic_world_lighting();
                self.applied_renderer_backend = self.video.renderer_backend;
                self.applied_grass = self.video.grass;
                self.applied_ocean = self.video.ocean;
                // Only DX12 has a present ceiling, so switching backends changes
                // what the cap resolves to.
                self.set_fps_cap(self.video.fps_cap);
                let applied = self.applied_video_mode();
                if confirm_on_change && applied != previous {
                    // Do not start the safety countdown merely because the render
                    // thread was spawned. Device/surface creation and world upload
                    // are asynchronous; WorldUploaded starts the timer after the
                    // replacement renderer has actually drawn the map.
                    self.pending_video_confirmation = Some(previous);
                    self.set_capture(false);
                    self.console_status = format!(
                        "VIDEO INITIALIZING {}X{} {} / {}...",
                        applied.resolution[0],
                        applied.resolution[1],
                        applied.fullscreen.label(),
                        applied.renderer_backend.label()
                    );
                    self.push_console_line(format!("^5{}", self.console_status));
                } else {
                    self.console_status = format!(
                        "VIDEO APPLIED: {}X{} {} / {}",
                        applied.resolution[0],
                        applied.resolution[1],
                        applied.fullscreen.label(),
                        applied.renderer_backend.label()
                    );
                    self.push_console_line(format!("^2{}", self.console_status));
                    self.mark_config_dirty();
                    self.flush_config();
                }
                self.normalize_video_selection();
                self.publish_ui();
                // GPU resources live entirely on the renderer thread, but the CPU
                // map preparation is backend-independent. Reuse the retained
                // prepared map whenever its feature-preparation options still match.
                // The frontend now owns a passive duel3 world too, so restore it
                // exactly like a gameplay world but never create a local player.
                if self.live_without_world || self.demo_without_world {
                    rverbose!(
                        1,
                        "[VID_RESTART] active session has no local BSP; keeping renderer worldless"
                    );
                } else if self.front_end && self.frontend_cinematic.is_some() {
                    if !self.reupload_cached_map() {
                        rverbose!(
                            1,
                            "[VID_RESTART] frontend map cache unavailable/incompatible; rebuilding cinematic background"
                        );
                        self.request_frontend_background();
                    }
                } else if !self.front_end && !self.reupload_cached_map() {
                    self.preserve_game_state_on_next_map_upload = true;
                    let source = self.initial_source.clone();
                    rverbose!(
                        1,
                        "[VID_RESTART] prepared map cache unavailable/incompatible; rebuilding CPU map data"
                    );
                    self.request_map(source);
                }
            }
            Err(error) => {
                self.renderer_restart_started = None;
                self.preserve_game_state_on_next_map_upload = false;
                self.show_window_after_restart();
                self.console_status = format!("VID_RESTART FAILED: {error}");
                self.push_console_line(format!("^1{}", self.console_status));
                if confirm_on_change {
                    // A backend or surface restart can fail before the confirmation
                    // dialog can be shown. Restore the last renderer/mode that was
                    // known to work instead of leaving the user on a dead screen.
                    self.set_requested_video_mode(previous);
                    self.push_console_line("^3RESTORING PREVIOUS VIDEO SETTINGS...".to_owned());
                    self.restart_renderer_internal(false);
                } else {
                    self.publish_ui();
                }
            }
        }
    }

    /// Build a fresh OS window for a backend switch, mirroring startup exactly:
    /// hidden, sized to the target mode, fullscreen applied before any surface is
    /// created. The old window is dropped here, which destroys the HWND still
    /// associated with the outgoing backend's presentation path.
    pub(in crate::app) fn recreate_window_for_backend(
        &mut self,
        event_loop: &ActiveEventLoop,
        target: AppliedVideoMode,
    ) -> Option<Arc<Window>> {
        let mut attributes = Window::default_attributes()
            .with_title("DinurdoJK")
            .with_visible(false)
            .with_inner_size(PhysicalSize::new(
                target.resolution[0],
                target.resolution[1],
            ));
        if target.fullscreen.is_windowed() {
            if let Some([x, y]) = self.video.window_position {
                attributes = attributes.with_position(PhysicalPosition::new(x, y));
            }
        }
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("[VID_RESTART] window recreation failed: {error}");
                return None;
            }
        };
        if !target.fullscreen.is_windowed() {
            if let Err(error) = Self::apply_fullscreen_mode(
                &window,
                target.fullscreen,
                target.resolution,
                target.renderer_backend,
            ) {
                eprintln!("[VID_RESTART] fullscreen setup failed on new window: {error}");
            }
        } else if self.video.window_maximized {
            window.set_maximized(true);
        }
        self.applied_fullscreen = target.fullscreen;
        self.applied_resolution = target.resolution;
        self.window_hidden_for_restart = true;
        self.window = Some(Arc::clone(&window));
        self.reset_egui_for_window(&window);
        Some(window)
    }

    pub(in crate::app) fn continue_pending_renderer_restart(
        &mut self,
        event_loop: &ActiveEventLoop,
    ) {
        let Some(mut pending) = self.pending_renderer_restart.take() else {
            return;
        };
        let now = Instant::now();
        if now < pending.ready_at {
            self.pending_renderer_restart = Some(pending);
            return;
        }
        let Some(window) = self.window.clone() else {
            self.renderer_restart_started = None;
            self.console_status = "VID_RESTART: WINDOW DISAPPEARED DURING TRANSITION".into();
            self.push_console_line(format!("^1{}", self.console_status));
            self.publish_ui();
            return;
        };

        // Transient Win32 resize/move messages while leaving fullscreen must not
        // overwrite the requested resolution/backend before we finish the staged
        // transition. Reassert the latched target at each stage.
        self.set_requested_video_mode(pending.target);

        match pending.stage {
            PendingRendererRestartStage::WaitRenderExit => {
                // Generous: a healthy teardown is a few hundred ms. Past this the
                // thread is wedged in a driver call; keep the UI alive and move on.
                const RENDER_EXIT_TIMEOUT: Duration = Duration::from_secs(8);
                let mut abandoned = false;
                if let Some((render, started)) = self.retiring_render.as_ref() {
                    if !render.is_finished() {
                        if started.elapsed() < RENDER_EXIT_TIMEOUT {
                            self.pending_renderer_restart = Some(pending);
                            return;
                        }
                        abandoned = true;
                    }
                }
                if let Some((mut render, started)) = self.retiring_render.take() {
                    if abandoned {
                        eprintln!(
                            "[VID_RESTART] old render thread still running after {:.1} s; abandoning it and creating a new window",
                            started.elapsed().as_secs_f64()
                        );
                        render.abandon();
                        self.console_status = "^1VID_RESTART: OLD RENDERER DID NOT EXIT (GPU DRIVER HANG?) - ABANDONED, CONTINUING ON A NEW WINDOW".into();
                        self.publish_ui();
                    } else {
                        render.shutdown();
                        rverbose!(
                            1,
                            "[VID_RESTART] old renderer shutdown {:.1} ms",
                            started.elapsed().as_secs_f64() * 1000.0
                        );
                    }
                }
                self.begin_restart_after_render_exit(
                    window,
                    pending.previous,
                    pending.target,
                    pending.confirm_on_change,
                    abandoned,
                );
            }
            PendingRendererRestartStage::RecreateWindow => {
                drop(window);
                let Some(fresh) = self.recreate_window_for_backend(event_loop, pending.target)
                else {
                    self.restart_display_apply_failed(
                        pending.previous,
                        pending.confirm_on_change,
                        "could not create a window for the new backend".into(),
                    );
                    return;
                };
                self.spawn_restarted_renderer(fresh, pending.previous, pending.confirm_on_change);
            }
            PendingRendererRestartStage::ApplyTargetDisplayMode => {
                if let Err(error) =
                    self.apply_pending_display_settings(&window, pending.target.fullscreen)
                {
                    self.restart_display_apply_failed(
                        pending.previous,
                        pending.confirm_on_change,
                        error,
                    );
                    return;
                }
                pending.stage = PendingRendererRestartStage::SpawnRenderer;
                pending.ready_at = now + Duration::from_millis(16);
                self.pending_renderer_restart = Some(pending);
                rverbose!(
                    1,
                    "[VID_RESTART] target Win32 display mode applied; waiting one frame before WGPU surface creation"
                );
            }
            PendingRendererRestartStage::SpawnRenderer => {
                rverbose!(
                    1,
                    "[VID_RESTART] Win32 display transition settled; creating {} renderer",
                    pending.target.renderer_backend.label()
                );
                self.spawn_restarted_renderer(window, pending.previous, pending.confirm_on_change);
            }
        }
    }

    pub(in crate::app) fn restart_renderer_internal(&mut self, confirm_on_change: bool) {
        self.restore_gamma_before_restart();
        let previous = self.applied_video_mode();
        let Some(window) = self.window.clone() else {
            self.console_status = "VID_RESTART: WINDOW IS NOT READY".into();
            self.publish_ui();
            return;
        };

        let mut mode = self.video.fullscreen;
        if mode == FullscreenMode::Exclusive
            && self.video.renderer_backend != RendererBackend::Dx12
            && Self::exclusive_video_mode(&window, self.video.resolution).is_none()
        {
            if confirm_on_change {
                self.console_status = format!(
                    "VID_RESTART: NO EXCLUSIVE VIDEO MODE MATCHES {}X{}",
                    self.video.resolution[0], self.video.resolution[1]
                );
                self.push_console_line(format!("^1{}", self.console_status));
                self.publish_ui();
                return;
            }

            // The monitor may have changed while the confirmation timer was
            // active. If the exact old exclusive mode no longer exists, prefer a
            // safe windowed rollback instead of leaving the user stuck.
            self.video.fullscreen = FullscreenMode::Windowed;
            mode = FullscreenMode::Windowed;
            self.push_console_line(
                "^3PREVIOUS EXCLUSIVE MODE IS UNAVAILABLE; REVERTING TO WINDOWED".to_owned(),
            );
        }

        let target = AppliedVideoMode {
            fullscreen: mode,
            renderer_backend: self.video.renderer_backend,
            resolution: self.video.resolution,
        };

        // Preserve the current window placement before leaving windowed mode,
        // but do not overwrite the newly selected pending resolution.
        if self.applied_fullscreen.is_windowed() && !mode.is_windowed() {
            self.video.window_maximized = window.is_maximized();
            if !self.video.window_maximized {
                if let Ok(position) = window.outer_position() {
                    self.video.window_position = Some([position.x, position.y]);
                }
            }
            self.mark_config_dirty();
        }

        let restart_started = Instant::now();
        self.renderer_restart_started = Some(restart_started);

        // flush_config deliberately writes the last known-good restart-sensitive
        // values. The newly requested mode is only persisted after confirmation.
        self.flush_config();
        self.forget_trace();
        self.console_status = format!(
            "VID_RESTART: APPLYING {}X{} {} / {}...",
            target.resolution[0],
            target.resolution[1],
            target.fullscreen.label(),
            target.renderer_backend.label()
        );
        self.push_console_line(format!("^5{}", self.console_status));
        self.publish_ui();

        // Display mode and renderer backend are a single latched transaction.
        // Stop presenting first so a fullscreen-mode switch cannot invalidate a
        // live WGPU surface underneath the render thread.
        //
        // Do not join here: the renderer's swapchain/surface teardown can
        // synchronously message this thread's window, so blocking the event
        // loop deadlocks on some drivers. Poll for the exit from the event loop.
        if let Some(render) = self.render.take() {
            render.request_shutdown();
            self.retiring_render = Some((render, Instant::now()));
            rverbose!(1, "[VID_RESTART] old renderer shutdown requested");
        }
        self.pending_renderer_restart = Some(PendingRendererRestart {
            previous,
            target,
            confirm_on_change,
            stage: PendingRendererRestartStage::WaitRenderExit,
            ready_at: Instant::now(),
        });
    }

    /// Second half of a renderer restart, run once the old render thread is
    /// gone. `force_new_window` is set when that thread had to be abandoned,
    /// because its surface may still own the current HWND.
    pub(in crate::app) fn begin_restart_after_render_exit(
        &mut self,
        window: Arc<Window>,
        previous: AppliedVideoMode,
        target: AppliedVideoMode,
        confirm_on_change: bool,
        force_new_window: bool,
    ) {
        if force_new_window || previous.renderer_backend != target.renderer_backend {
            // An HWND keeps the presentation association of the backend that first
            // drew to it, so a Vulkan swapchain created on a window that already
            // hosted a DX12 one renders at full speed into something Windows never
            // shows. Every same-backend transition works, including real exclusive
            // mode switches, and launching straight into either backend works
            // because the window is new. So give the new backend a new window.
            self.pending_renderer_restart = Some(PendingRendererRestart {
                previous,
                target,
                confirm_on_change,
                stage: PendingRendererRestartStage::RecreateWindow,
                ready_at: Instant::now(),
            });
            rverbose!(
                1,
                "[VID_RESTART] backend {} -> {}{}; recreating the OS window",
                previous.renderer_backend.label(),
                target.renderer_backend.label(),
                if force_new_window {
                    " (old render thread abandoned)"
                } else {
                    ""
                }
            );
            return;
        }

        let previous_native =
            Self::native_fullscreen_mode(previous.fullscreen, previous.renderer_backend);
        let requested_native =
            Self::native_fullscreen_mode(target.fullscreen, target.renderer_backend);
        let display_state_changed = previous_native != requested_native
            || (requested_native != FullscreenMode::Borderless
                && previous.resolution != target.resolution);

        if display_state_changed {
            // Winit's Windows fullscreen transition and WGPU/DXGI/Vulkan surface
            // creation must not happen in one event-loop callback. This was the
            // source of the DX12 -> Vulkan frozen presentation and the invalid
            // DX12 surface on rollback: the new surface could be created while
            // the HWND was still changing native fullscreen state.
            //
            // First release any native fullscreen/borderless state. Then let the
            // Win32 message queue run, apply the requested target mode, let it run
            // once more, and only then create the replacement WGPU surface.
            window.set_fullscreen(None);
            self.applied_fullscreen = FullscreenMode::Windowed;
            let size = window.inner_size();
            if size.width > 0 && size.height > 0 {
                self.applied_resolution = [size.width, size.height];
            }
            self.pending_renderer_restart = Some(PendingRendererRestart {
                previous,
                target,
                confirm_on_change,
                stage: PendingRendererRestartStage::ApplyTargetDisplayMode,
                ready_at: Instant::now() + Duration::from_millis(16),
            });
            rverbose!(
                1,
                "[VID_RESTART] released old Win32 fullscreen state; deferring target mode for one frame"
            );
            return;
        }

        // Backend-only switches between identical native window modes can reuse
        // the settled HWND immediately. The old render thread has already joined,
        // so its WGPU surface/device are gone before the replacement is created.
        self.applied_fullscreen = target.fullscreen;
        self.applied_resolution = target.resolution;
        rverbose!(
            1,
            "[VID_RESTART] display mode unchanged; skipped Win32 mode transition"
        );
        self.spawn_restarted_renderer(window, previous, confirm_on_change);
    }
}
