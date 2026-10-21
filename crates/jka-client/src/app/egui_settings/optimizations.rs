//! Optimizations: speed-ups and A/B switches that should not change the picture.
//!
//! Everything here is meant to stay on unless it breaks something, and is a
//! candidate for being hard-coded once it has been tested enough. Quality presets
//! deliberately own none of these cvars (see the
//! `quality_presets_do_not_touch_optimization_controls` test), so a preset click
//! never changes which optimizations are active.
use crate::app::egui_settings::{
    index_of, segmented_row, theme, ui, App, FxGeometryMode, Ghoul2BatchMode, Ghoul2SkinningMode,
    PvsMode,
};
use crate::app::quality::cvar_values_match;

/// Every cvar this page edits: `(cvar, default, reference)`. `reference` is the
/// slow, exact path that "Disable all" selects, so a visual bug can be bisected
/// in one click. Rows whose only alternative is the default (plain A/B draw
/// switches) list the default for both.
pub(in crate::app) const OPTIMIZATION_CVARS: &[(&str, &str, &str)] = &[
    ("r_pvsMode", "auto", "full"),
    ("r_gpuDriven", "0", "0"),
    ("r_hizOcclusion", "0", "0"),
    ("r_ghoul2Skinning", "gpu", "cpu"),
    ("r_inlineTransform", "gpu", "cpu"),
    ("r_ghoul2BatchDraws", "1", "0"),
    ("r_staticMd3Instances", "0", "0"),
    ("r_ghoul2EarlyCull", "1", "0"),
    ("r_fxGeometry", "gpu", "cpu"),
    ("r_fxZeroAlphaDiscard", "0", "0"),
    ("cg_eventWorkers", "1", "0"),
    ("r_cloudEmptySkip", "0", "0"),
    ("r_fragmentShaderMode", "1", "0"),
    ("r_colorClearMode", "1", "0"),
    ("r_additiveStageFusion", "1", "0"),
    ("r_facePlaneCull", "0", "0"),
    ("r_worldSortMode", "0", "0"),
    ("r_worldCompactMinDraws", "1", "1"),
    ("r_encoderFinishThreads", "2", "0"),
    ("r_stageCollapseExtra", "0", "0"),
    ("cm_collisionMerge", "0", "0"),
    ("r_md3GeometryCache", "1", "0"),
    ("r_modelIndexUploadCache", "1", "0"),
    ("r_modelRenderBundles", "0", "0"),
    ("r_modelStateCache", "0", "0"),
    ("r_modelPrepareThreads", "0", "0"),
    ("r_dynamicBatchDraws", "1", "0"),
    ("r_ghoul2BatchKeyCache", "1", "0"),
    ("r_fxParticleSimulation", "1", "0"),
    ("r_surfaceSpritesGpu", "1", "0"),
    ("r_grassFrontToBack", "1", "0"),
    ("r_grassMidLod", "1", "0"),
    ("r_grassPrecompute", "1", "0"),
    ("r_rainFastPost", "0", "0"),
    ("r_rainFastVertex", "0", "0"),
    ("r_rainLeanDepth", "0", "0"),
    ("r_rainLeanModels", "0", "0"),
    ("r_rainLeanPost", "0", "0"),
    ("r_rainOpaqueDepth", "0", "0"),
    ("r_rainStaticDepth", "0", "0"),
    ("r_rainWaterField", "0", "0"),
    ("r_oceanOpticsCull", "1", "0"),
    ("r_oceanSurfacePixels", "1", "1"),
    ("r_oceanDynamicUpdates", "0", "0"),
    ("r_oceanWindFoamSync", "1", "0"),
    ("r_blueWaterMeshSegments", "64", "128"),
    ("r_blueWaterMeshLevels", "6", "9"),
    ("r_oceanScreenLodPixels", "0", "0"),
    ("r_oceanPixelDetail", "0", "0"),
    ("r_blueWaterSSRSteps", "16", "16"),
    ("r_blueWaterBoundaryMode", "3", "0"),
    ("r_blueWaterPrepareGodotMesh", "0", "1"),
    ("r_blueWaterAdapter", "1", "1"),
];

/// Bool spellings differ between cvar getters ("1", "true", "on"); fold them so
/// the comparison is about the setting, not its spelling.
fn normalized(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "on" | "yes" => "1".to_owned(),
        "false" | "off" | "no" => "0".to_owned(),
        other => other.to_owned(),
    }
}

fn same_setting(a: &str, b: &str) -> bool {
    cvar_values_match(&normalized(a), &normalized(b))
}

impl App {
    /// How many optimization cvars currently differ from their defaults.
    pub(in crate::app) fn optimizations_off_default(&self) -> usize {
        OPTIMIZATION_CVARS
            .iter()
            .filter(|(name, default, _)| {
                self.console_cvar_value(name)
                    .is_some_and(|current| !same_setting(&current, default))
            })
            .count()
    }

    fn apply_optimization_values(&mut self, title: &str, reference: bool) {
        self.begin_settings_batch(title);
        for (name, default, safe) in OPTIMIZATION_CVARS {
            let value = if reference { safe } else { default };
            // Some of these rebuild GPU state when written, so skip no-ops.
            if self
                .console_cvar_value(name)
                .is_some_and(|current| same_setting(&current, value))
            {
                continue;
            }
            self.latched_console_cvars.remove(&name.to_ascii_lowercase());
            if let Err(error) = self.set_console_cvar(name, value) {
                eprintln!("[OPTIMIZATIONS] {title}: {error}");
            }
        }
        self.end_settings_batch();
        self.mark_config_dirty();
        self.egui_repaint_requested = true;
        self.publish_ui();
    }

    pub(in crate::app::egui_settings) fn egui_optimizations(&mut self, ui: &mut egui::Ui) {
        let differing = self.optimizations_off_default();
        theme::row(
            ui,
            "All optimizations",
            "Restore defaults returns every row on this page to its shipped state. Disable all \
             switches every optimization to its slow reference path in one click, which is the \
             fastest way to find out whether a visual bug comes from an optimization. Neither \
             touches quality presets.",
            theme::Reset::None,
            |ui| {
                if theme::ghost_button(ui, "RESTORE DEFAULTS").clicked() {
                    self.apply_optimization_values("Restore optimization defaults", false);
                }
                ui.add_space(6.0);
                if theme::ghost_button(ui, "DISABLE ALL").clicked() {
                    self.apply_optimization_values("Disable all optimizations", true);
                }
                ui.add_space(10.0);
                let (text, color) = if differing == 0 {
                    ("All at defaults".to_owned(), theme::TEXT_FAINT)
                } else {
                    (format!("{differing} differ from default"), theme::WARNING)
                };
                theme::glow_label(ui, &text, 12.5, color);
            },
        );

        self.egui_world_optimizations(ui);
        self.egui_model_optimizations(ui);
        self.egui_effect_optimizations(ui);
        self.egui_cloud_optimizations(ui);
        self.egui_grass_rain_optimizations(ui);
        self.egui_ocean_optimizations(ui);
    }

    /// On/off row that reads and writes a cvar by name, for optimizations that
    /// have no dedicated settings row.
    fn cvar_switch_row(&mut self, ui: &mut egui::Ui, label: &str, tip: &str, cvar: &str) {
        let on = self
            .console_cvar_value(cvar)
            .and_then(|value| Self::parse_console_bool(&value))
            .unwrap_or(false);
        theme::row(ui, label, tip, theme::Reset::None, |ui| {
            if let Some(value) = theme::switch(ui, on) {
                let _ = self.set_console_cvar(cvar, if value { "1" } else { "0" });
            }
        });
    }

    /// Chip row over a handful of fixed cvar values.
    fn cvar_choice_row(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        tip: &str,
        cvar: &str,
        options: &[(&str, &str)],
    ) {
        let current = self.console_cvar_value(cvar).unwrap_or_default();
        theme::row(ui, label, tip, theme::Reset::None, |ui| {
            for (value, text) in options {
                if theme::chip(ui, text, same_setting(&current, value)).clicked() {
                    let _ = self.set_console_cvar(cvar, value);
                }
                ui.add_space(3.0);
            }
        });
    }

    fn egui_render_path_optimizations(&mut self, ui: &mut egui::Ui) {
        self.cvar_choice_row(
            ui,
            "Fragment shader mode",
            "World shaders: 0 general; 1 material specialization (default); 2 specialization plus minimal entries; 3 shared basic/enhanced families; 4 generated minimal vertex/fragment pairs per encountered stage recipe. Modes 3 and 4 target Minimal Unified, Low and Medium, with reported fallbacks for other features. Switches after background compilation; quality settings are preserved.",
            "r_fragmentShaderMode",
            &[("0", "General"), ("1", "Material"), ("2", "Material + minimal"), ("3", "Shared families"), ("4", "Generated pairs")],
        );
        self.cvar_switch_row(
            ui,
            "Skip colour clear",
            "r_colorClearMode: skips the per-frame world colour clear when every sky on the map draws a sky box or the map has no sky (vanilla/TaystJK behaviour; holes into the void then show the previous frame). Fastsky always clears.",
            "r_colorClearMode",
        );
        self.cvar_switch_row(
            ui,
            "Additive stage fusion",
            "r_additiveStageFusion: fuse equivalent consecutive additive BSP shader stages into one draw. Toggling reuploads the cached render world and may briefly stall; no map reparse.",
            "r_additiveStageFusion",
        );
        self.cvar_switch_row(
            ui,
            "Face-plane culling",
            "r_facePlaneCull: BSP plane culling. Packed-batch rejection plus CPU subrange rejection of planar faces (GPU-driven uses batch-only fallback).",
            "r_facePlaneCull",
        );
        self.cvar_switch_row(
            ui,
            "Sort by compiled pipeline (A/B)",
            "r_worldSortMode: additionally groups independent opaque/masked stages by their actual compiled pipeline inside the same authored shader sort bucket. Never reorders blended, sky, depth-equal, multi-stage, mover, water, mirror or ocean stages. Session only.",
            "r_worldSortMode",
        );
        self.cvar_choice_row(
            ui,
            "CPU world compaction",
            "r_worldCompactMinDraws: 1 baseline, 2 draws singleton visible groups directly. Keeps map-load packing and GPU-driven behaviour. Session only.",
            "r_worldCompactMinDraws",
            &[("1", "Baseline"), ("2", "Singletons direct")],
        );
        self.cvar_choice_row(
            ui,
            "Encoder finish threads",
            "r_encoderFinishThreads: 0 original encoder; 1 split world/model-post encoders finished serially; 2 finish world on a persistent worker concurrently with remaining recording (default). Same-frame ordered submit. Unified MSAA1 CPU raster only; not used with ocean/optics/RT/planar views. Session only.",
            "r_encoderFinishThreads",
            &[("0", "Original"), ("1", "Split"), ("2", "Concurrent")],
        );
        self.cvar_switch_row(
            ui,
            "Reverse lightmap fold (experimental)",
            "r_stageCollapseExtra: experimental OpenJK reverse lightmap modulation fold. Rebuilds GPU world batches from the cached source when toggled (no BSP/asset reparse); may briefly stall during upload.",
            "r_stageCollapseExtra",
        );
        self.cvar_switch_row(
            ui,
            "Collision merge (experimental)",
            "cm_collisionMerge: merges perfectly flush solid box brushes into one at map load. Live A/B switch. Affects local movement prediction and may differ from vanilla servers.",
            "cm_collisionMerge",
        );
    }

    fn egui_model_extra_optimizations(&mut self, ui: &mut egui::Ui) {
        self.cvar_switch_row(
            ui,
            "MD3 geometry cache",
            "r_md3GeometryCache: reuses unchanged transformed/lit MD3 geometry and resident GPU stream ranges. Animation, transforms and lights invalidate the exact cached inputs. Raster stream cache; RT keeps its original preparation. Session only.",
            "r_md3GeometryCache",
        );
        self.cvar_switch_row(
            ui,
            "Model index upload cache",
            "r_modelIndexUploadCache: skips unchanged index uploads and uploads only a changed span on the raster CPU-model path. Session only.",
            "r_modelIndexUploadCache",
        );
        self.cvar_switch_row(
            ui,
            "Model render bundles (prototype)",
            "r_modelRenderBundles: reusable opaque/masked indexed-model command recording with exact draw/resource checks and two-frame stability. RT, glow and excluded views keep direct submission. Session-only prototype.",
            "r_modelRenderBundles",
        );
        self.cvar_switch_row(
            ui,
            "Model state cache",
            "r_modelStateCache: suppresses repeated texture and Ghoul2 mesh bindings in model/FX draw phases. Session only.",
            "r_modelStateCache",
        );
        self.cvar_choice_row(
            ui,
            "Model prepare threads",
            "r_modelPrepareThreads: CPU model packing. 0 original serial path, 1 serial prepared jobs, 2 or 4 bounded worker threads for vertex lighting and index rebasing. Raster path only. Session only.",
            "r_modelPrepareThreads",
            &[("0", "Serial"), ("1", "Prepared"), ("2", "2 threads"), ("4", "4 threads")],
        );
        self.cvar_switch_row(
            ui,
            "Dynamic draw batching",
            "r_dynamicBatchDraws: merges adjacent compatible CPU model index ranges and GPU FX instance ranges without reordering. Session only.",
            "r_dynamicBatchDraws",
        );
        self.cvar_switch_row(
            ui,
            "Ghoul2 batch key cache",
            "r_ghoul2BatchKeyCache: cached group IDs plus linear visible bucketing, instead of the legacy full-frame hash/sort. Session only.",
            "r_ghoul2BatchKeyCache",
        );
    }

    fn egui_grass_rain_optimizations(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "GRASS & RAIN", "Surface sprites, grass and rain passes. Runtime A/B switches; most are not saved.");
        self.cvar_switch_row(
            ui,
            "Surface sprites on GPU",
            "r_surfaceSpritesGpu: bakes native vertical/flattened/oriented surfaceSprites into GPU-resident instances at map load and only culls 512-unit chunks per frame. Off regenerates and uploads every sprite on the CPU each frame (the original path). Effect-kind sprites always use the CPU path.",
            "r_surfaceSpritesGpu",
        );
        self.cvar_switch_row(
            ui,
            "Grass front-to-back",
            "r_grassFrontToBack: orders opaque grass front to back for early-Z.",
            "r_grassFrontToBack",
        );
        self.cvar_switch_row(
            ui,
            "Grass middle LOD",
            "r_grassMidLod: adds a 5-triangle middle blade LOD.",
            "r_grassMidLod",
        );
        self.cvar_switch_row(
            ui,
            "Grass precompute",
            "r_grassPrecompute: compute prepass for grass root wind and clump.",
            "r_grassPrecompute",
        );
        self.cvar_switch_row(
            ui,
            "Rain fast post",
            "r_rainFastPost: omits rain's linear-depth/full spatial post requirement and depth-aware haze.",
            "r_rainFastPost",
        );
        self.cvar_switch_row(
            ui,
            "Rain fast vertex",
            "r_rainFastVertex: squared compute bounds and early centre clip rejection before streak math.",
            "r_rainFastVertex",
        );
        self.cvar_switch_row(
            ui,
            "Rain lean depth",
            "r_rainLeanDepth: when rain is the only linear-depth consumer, writes only linear depth plus hardware depth instead of motion/reflection targets. Dynamic models stay omitted unless Rain lean models is on.",
            "r_rainLeanDepth",
        );
        self.cvar_switch_row(
            ui,
            "Rain lean models",
            "r_rainLeanModels: with Rain lean depth on, draws dynamic models into the lean R32 linear-depth target instead of omitting them.",
            "r_rainLeanModels",
        );
        self.cvar_switch_row(
            ui,
            "Rain lean post",
            "r_rainLeanPost: when rain is the only spatial post effect, uses the specialized rain-only fullscreen post shader.",
            "r_rainLeanPost",
        );
        self.cvar_switch_row(
            ui,
            "Rain opaque depth",
            "r_rainOpaqueDepth: skips alpha-tested BSP surfaces in a rain-only linear-depth prepass.",
            "r_rainOpaqueDepth",
        );
        self.cvar_switch_row(
            ui,
            "Rain static depth",
            "r_rainStaticDepth: skips players, vehicles and dynamic models in a rain-only linear-depth prepass.",
            "r_rainStaticDepth",
        );
        self.cvar_switch_row(
            ui,
            "Rain water field",
            "r_rainWaterField: one prebaked water-height texture load instead of per-drop promoted-water outline scans.",
            "r_rainWaterField",
        );
    }

    fn egui_world_optimizations(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "WORLD", "What the map renderer skips and how draws are built.");
        const PVS: [(PvsMode, &str); 4] = [
            (PvsMode::Off, "Off"),
            (PvsMode::Minimal, "Minimal"),
            (PvsMode::Full, "Full"),
            (PvsMode::Auto, "Auto"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "PVS portal culling",
            "Uses the map's precomputed visibility set to skip rooms the camera cannot see. Off draws everything. Minimal draws the fewest batches; Full keeps every triangle of each visible batch. Auto precomputes merged batches per camera cluster at map load: the minimal draw count with the exact visible triangles. If a map shows missing geometry, try Full.",
            theme::Reset::Video(ui::VIDEO_ROW_PVS),
            self.video.pvs_mode,
            &PVS,
        ) {
            let current = index_of(&PVS, self.video.pvs_mode, 3);
            let next = index_of(&PVS, target, current);
            self.video_selected = ui::VIDEO_ROW_PVS;
            self.change_video_setting(next as i32 - current as i32);
        }
        self.egui_toggle_row(
            ui,
            "GPU-driven draws",
            "Builds the draw list on the GPU with a compute pass instead of on \
             the CPU. Helps on maps with many batches.",
            ui::VIDEO_ROW_GPU_DRIVEN,
            self.video.gpu_driven,
        );
        ui.add_enabled_ui(self.video.gpu_driven, |ui| {
            self.egui_toggle_row(
                ui,
                "Hi-Z occlusion",
                "Tests batches against a hierarchical depth pyramid so geometry \
                 hidden behind walls is never submitted. Needs GPU-driven draws.",
                ui::VIDEO_ROW_HIZ,
                self.video.hiz_occlusion,
            );
        });
        self.egui_render_path_optimizations(ui);
    }

    fn egui_model_optimizations(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "MODELS", "Ghoul2 and MD3 submission paths.");
        const GHOUL2_SKINNING: [(Ghoul2SkinningMode, &str); 3] = [
            (Ghoul2SkinningMode::Cpu, "CPU"),
            (Ghoul2SkinningMode::CpuWorkers, "CPU workers"),
            (Ghoul2SkinningMode::Gpu, "GPU"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "Ghoul2 skinning",
            "GLM vertex deformation path. GPU is the default and keeps JKA pose/bolt evaluation on the CPU while skinning bind-pose vertices in the WGPU vertex shader. CPU remains the fidelity reference.",
            theme::Reset::Video(ui::VIDEO_ROW_GHOUL2_SKINNING),
            self.video.ghoul2_skinning,
            &GHOUL2_SKINNING,
        ) {
            let current = Ghoul2SkinningMode::ALL
                .iter()
                .position(|mode| *mode == self.video.ghoul2_skinning)
                .unwrap_or(0);
            let next = Ghoul2SkinningMode::ALL
                .iter()
                .position(|mode| *mode == target)
                .unwrap_or(current);
            self.video_selected = ui::VIDEO_ROW_GHOUL2_SKINNING;
            self.change_video_setting(next as i32 - current as i32);
        }

        const INLINE_TRANSFORMS: [(ui::InlineTransformMode, &str); 3] = [
            (ui::InlineTransformMode::Cpu, "CPU"),
            (ui::InlineTransformMode::CpuWorkers, "CPU workers"),
            (ui::InlineTransformMode::Gpu, "GPU"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "Inline mover transforms",
            "Moves BSP objects such as boats, doors and platforms. GPU, the default, transforms vertices with compute. CPU is the reference. CPU workers splits large meshes across worker threads before uploading them. Applies immediately; collision and gameplay use the same poses in every mode.",
            theme::Reset::None,
            self.video.inline_transform_mode,
            &INLINE_TRANSFORMS,
        ) {
            let _ = self.set_console_cvar("r_inlineTransform", target.config_value());
        }

        const GHOUL2_BATCHING: [(Ghoul2BatchMode, &str); 3] = [
            (Ghoul2BatchMode::Off, "Off"),
            (Ghoul2BatchMode::Adaptive, "Adaptive"),
            (Ghoul2BatchMode::Force, "Force"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "Ghoul2 draw batching",
            "GPU-skinning draw-call batching. Adaptive is the default: it skips batching below 32 GPU-skinned surfaces, samples at most 64 surfaces for a real 3-instance match, and only then groups matching opaque/masked mesh, material and LOD draws. Force always attempts compatible grouping.",
            theme::Reset::Video(ui::VIDEO_ROW_GHOUL2_BATCH_DRAWS),
            self.video.ghoul2_batch_draws,
            &GHOUL2_BATCHING,
        ) {
            let current = Ghoul2BatchMode::ALL
                .iter()
                .position(|mode| *mode == self.video.ghoul2_batch_draws)
                .unwrap_or(1);
            let next = Ghoul2BatchMode::ALL
                .iter()
                .position(|mode| *mode == target)
                .unwrap_or(current);
            self.video_selected = ui::VIDEO_ROW_GHOUL2_BATCH_DRAWS;
            self.change_video_setting(next as i32 - current as i32);
        }

        theme::row(
            ui,
            "Static MD3 GPU instances (A/B)",
            "Experimental resident mesh + instance transforms for repeated misc_model_static MD3s. Reuses the Ghoul2 GPU batching path for eligible opaque/masked surfaces. CPU frustum/PVS/distance/LOD is unchanged; RT casters and complex shader stages remain on the baseline. A/B with r_staticMd3Instances 0/1 (default Off).",
            theme::Reset::None,
            |ui| {
                if let Some(enabled) = theme::switch(ui, self.video.static_md3_instances) {
                    let _ = self.set_console_cvar("r_staticMd3Instances", if enabled { "1" } else { "0" });
                }
            },
        );

        self.egui_toggle_row(
            ui,
            "Offscreen early cull",
            "Skips the final full Ghoul2 pose and render submission once a model is outside the view frustum, while retaining the lightweight animation/angle state needed for smooth re-entry. Disable for A/B diagnostics.",
            ui::VIDEO_ROW_GHOUL2_EARLY_CULL,
            self.video.ghoul2_early_cull,
        );
        self.egui_model_extra_optimizations(ui);
    }

    fn egui_effect_optimizations(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "EFFECTS", "EFX geometry submission and client event processing.");
        const FX_GEOMETRY: [(FxGeometryMode, &str); 3] = [
            (FxGeometryMode::Cpu, "CPU"),
            (FxGeometryMode::CpuWorkers, "CPU workers"),
            (FxGeometryMode::Gpu, "GPU"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "FX geometry",
            "View-dependent EFX geometry. CPU is the exact current reference path. CPU workers uses the same authored particles/materials and exact same CPU geometry, but parallelizes tessellation over Rayon. GPU keeps FX simulation on the dedicated jka-fx CPU thread and packs Particle, OrientedParticle, Line, Tail and legacy SaberGlow geometry into compact WGPU instances (including under RT lighting/shadows). Animated tcMod stages and complex primitives remain on CPU. This is geometry acceleration, not GPU-resident particle simulation.",
            theme::Reset::Video(ui::VIDEO_ROW_FX_GEOMETRY),
            self.video.fx_geometry,
            &FX_GEOMETRY,
        ) {
            let current = FxGeometryMode::ALL
                .iter()
                .position(|mode| *mode == self.video.fx_geometry)
                .unwrap_or(0);
            let next = FxGeometryMode::ALL
                .iter()
                .position(|mode| *mode == target)
                .unwrap_or(current);
            self.video_selected = ui::VIDEO_ROW_FX_GEOMETRY;
            self.change_video_setting(next as i32 - current as i32);
        }

        ui.add_enabled_ui(self.video.fx_geometry == FxGeometryMode::Gpu, |ui| {
            self.egui_toggle_row(
                ui,
                "FX zero-alpha discard (A/B)",
                "A/B diagnostic for r_fxGeometry GPU. Discards only fragments whose final source alpha is exactly zero on source-alpha EFX sprite blend modes, before fog/blending. Non-zero-alpha edges are unchanged; GL_ONE/additive-one and modulation modes are intentionally untouched.",
                ui::VIDEO_ROW_FX_ZERO_ALPHA_DISCARD,
                self.video.fx_zero_alpha_discard,
            );
        });

        ui.add_enabled_ui(self.video.fx_geometry == FxGeometryMode::Gpu, |ui| {
            self.cvar_switch_row(
                ui,
                "GPU particle motion (experimental)",
                "r_fxParticleSimulation: GPU-resident motion for compatible non-colliding EFX sprite primitives. Needs FX geometry on GPU. Other primitives keep their CPU fallback. Experimental until FX visual parity and frame-time A/B testing are done.",
                "r_fxParticleSimulation",
            );
        });

        theme::row(
            ui,
            "Event worker queue",
            "Client event processing. Off performs the same semantic preparation on the CGame thread. On prepares multi-event receive batches on a dedicated worker pool, then applies all player/audio/FX/presentation side effects in original queue order. Single-event batches intentionally stay inline.",
            theme::Reset::None,
            |ui| {
                if let Some(value) = theme::switch(ui, self.cg_event_workers) {
                    let _ = self.set_console_cvar("cg_eventWorkers", if value { "1" } else { "0" });
                }
            },
        );
    }

    fn egui_cloud_optimizations(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "CLOUDS", "Volumetric cloud march.");
        theme::row(
            ui,
            "Empty-space skip (A/B)",
            "Takes larger primary-ray steps only when the macro cloud field is safely below the \
             coverage threshold, then falls back to fine stepping near potential cloud.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_EMPTY_SKIP),
            |ui| {
                if theme::switch(ui, self.video.cloud_empty_skip).is_some() {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_EMPTY_SKIP, true, None);
                }
            },
        );
    }

    fn egui_ocean_optimizations(&mut self, ui: &mut egui::Ui) {
        if !self.video.ocean {
            return;
        }
        theme::section(ui, "OCEAN", "Surface draw, mesh and shading cost. Each change applies live.");
        let backend_native = self.video.ocean_settings.backend != 0;
        let hybrid = self.video.ocean_settings.backend == 2;
        let mut ocean = self.video.ocean_settings;
        let d = crate::ocean::OceanSettings::default();
        let mut changed = false;

        theme::row(
            ui,
            "Render ocean surface pixels (A/B)",
            "r_oceanSurfacePixels: turn off ALL promoted ocean surface draws, including native Blue Water and infinite water. FFT, wind foam, water interaction and CPU mesh updates keep running; no ocean surface is visible or writes depth. Other ocean optics/caustics remain enabled. For benchmarking.",
            theme::Reset::None,
            |ui| {
                if let Some(value) = theme::switch(ui, ocean.surface_pixels) {
                    ocean.surface_pixels = value;
                    changed = true;
                }
            },
        );
        theme::row(
            ui,
            "Dynamic mesh LOD rates",
            "r_oceanDynamicUpdates: 0 original; 1 nearest two mesh rings use live FFT (48 Hz), third uses a 32 Hz displacement snapshot, outer rings a 24 Hz snapshot. Wind foam streak appearance follows these mesh rates. All FFT cascades remain synchronized; this reduces cached texture bandwidth, not FFT dispatch count.",
            theme::Reset::None,
            |ui| {
                if let Some(value) = theme::switch(ui, ocean.dynamic_update_rates) {
                    ocean.dynamic_update_rates = value;
                    changed = true;
                }
            },
        );
        theme::row(
            ui,
            "Sync wind foam to waves",
            "r_oceanWindFoamSync: wind-streak transport runs only when the fine FFT layer finishes updating, rather than every rendered frame. The mesh still renders each frame.",
            theme::Reset::None,
            |ui| {
                if let Some(value) = theme::switch(ui, ocean.wind_foam_sync) {
                    ocean.wind_foam_sync = value;
                    changed = true;
                }
            },
        );

        self.cvar_switch_row(
            ui,
            "Skip idle underwater optics",
            "r_oceanOpticsCull: skips the underwater optics capture entirely when the camera is dry and no ocean surface is on screen, since nothing can sample it then.",
            "r_oceanOpticsCull",
        );
        if backend_native {
            theme::row(ui, "Native mesh segments", "Blue Water only: 64, 128 (reference), 256. Rebuilds native mesh buffers. Godot mesh quality does not affect this mesh.", theme::Reset::None, |ui| {
                for segments in [64, 128, 256] {
                    if theme::chip(ui, &segments.to_string(), ocean.blue_water_mesh_segments == segments).clicked() {
                        ocean.blue_water_mesh_segments = segments;
                        changed = true;
                    }
                }
            });
            theme::row(ui, "Native clipmap levels", "Blue Water only: 6 to 9, reference 9. Fewer rings reduce horizon reach.", theme::Reset::None, |ui| {
                for levels in [6, 7, 8, 9] {
                    if theme::chip(ui, &levels.to_string(), ocean.blue_water_mesh_levels == levels).clicked() {
                        ocean.blue_water_mesh_levels = levels;
                        changed = true;
                    }
                }
            });
            theme::row(ui, "Adaptive ocean geometry", "0 original mesh; 1 conservative whole-ring curvature LOD; 2 experimental per-tile wave-curvature LOD with GPU vertex geomorph and stitched edges. All modes preserve screen-space LOD.", theme::Reset::None, |ui| {
                for (mode, title) in [(0, "Off"), (1, "Rings"), (2, "Local")] {
                    if theme::chip(ui, title, ocean.adaptive_mesh == mode).clicked() {
                        ocean.adaptive_mesh = mode; changed = true;
                    }
                }
            });
            theme::row(ui, "Local mesh tolerance", "Mode 2 only: scales the allowed projected and normal geometry error. Higher = fewer triangles, potentially more visible geometric error. 1x retains the original thresholds.", theme::Reset::None, |ui| {
                for (tolerance, title) in [(0.5, "0.5x"), (1.0, "1x"), (2.0, "2x"), (4.0, "4x"), (8.0, "8x"), (16.0, "16x")] {
                    if theme::chip(ui, title, ocean.adaptive_mesh_tolerance == tolerance).clicked() {
                        ocean.adaptive_mesh_tolerance = tolerance; changed = true;
                    }
                }
            });
            theme::row(ui, "Screen-space mesh LOD", "Native/Hybrid only. Decimates outer-ring interiors at shallow elevation angles; the nearest three rings are protected when curvature-adaptive mesh is Off. With adaptive mesh On, combines with curvature LOD instead of being overridden. Boundary vertices stay stitched. Default Off.", theme::Reset::None, |ui| {
                for (threshold, name) in [(0.0, "Off"), (1.0, "1px"), (2.0, "2px"), (4.0, "4px"), (8.0, "8px")] {
                    if theme::chip(ui, name, ocean.screen_lod_pixels == threshold).clicked() {
                        ocean.screen_lod_pixels = threshold;
                        changed = true;
                    }
                }
            });
            theme::row(ui, "Expensive pixel detail", "Native/Hybrid: render the original material on real 1/2 or 1/4 resolution targets, then depth-aware upscale against full-resolution wave geometry. Far modes blend to full-quality shading near the camera. Adaptive 2x2 shades full resolution only where the 2x2 image would visibly differ (edges, crests, glints, foam lace); this is not hardware VRS.", theme::Reset::None, |ui| {
                for (mode, name) in [(0, "Full"), (1, "2x2"), (2, "4x4"), (3, "Far 2x2"), (4, "Far 4x4"), (5, "Adaptive 2x2")] {
                    if theme::chip(ui, name, ocean.pixel_detail_mode == mode).clicked() {
                        ocean.pixel_detail_mode = mode;
                        changed = true;
                    }
                }
            });
            if matches!(ocean.pixel_detail_mode, 3 | 4) {
                theme::row(ui, "Detail falloff distance", "Full shading within this many metres of the camera; coarse shading outside.", theme::Reset::None, |ui| {
                    let mut distance = ocean.pixel_detail_distance;
                    let readout = format!("{distance:.0} m");
                    if theme::slider_default(ui, &mut distance, 1.0..=1000.0, &readout, d.pixel_detail_distance) {
                        ocean.pixel_detail_distance = distance;
                        changed = true;
                    }
                });
            }
            if ocean.pixel_detail_mode == 5 {
                theme::row(ui, "Adaptive contrast", "How much the coarse image may bend across a 2x2 block before that block gets full shading. Lower = more full-resolution blocks: closer to Full, slower.", theme::Reset::None, |ui| {
                    let mut contrast = ocean.pixel_detail_contrast;
                    let readout = format!("{:.1} %", contrast * 100.0);
                    if theme::slider_default(ui, &mut contrast, 0.001..=0.2, &readout, d.pixel_detail_contrast) {
                        ocean.pixel_detail_contrast = contrast;
                        changed = true;
                    }
                });
            }
            theme::row(ui, "SSR trace steps", "Compile-time ray budget. 16 is upstream; fewer steps can speed up water-heavy views but reduce hit quality. Rebuilds only the native surface shader.", theme::Reset::None, |ui| {
                for count in [4, 8, 12, 16, 24, 32] {
                    if theme::chip(ui, &count.to_string(), ocean.blue_water_ssr_steps == count).clicked() {
                        ocean.blue_water_ssr_steps = count;
                        changed = true;
                    }
                }
            });
            theme::row(ui, "JKA boundary tests", "Exact binned uses per-tile triangle candidates and keeps the precise footprint/PVS test.", theme::Reset::None, |ui| {
                for (mode, name) in [(0, "Exact"), (3, "Exact binned"), (1, "Box/PVS"), (2, "Off/test")] {
                    if theme::chip(ui, name, ocean.blue_water_boundary_mode == mode).clicked() {
                        ocean.blue_water_boundary_mode = mode;
                        changed = true;
                    }
                }
            });
            theme::row(ui, "Update Godot mesh (A/B)", "Legacy clipmap indices are not drawn by Blue Water. OFF avoids their CPU updates/uploads.", theme::Reset::None, |ui| {
                if let Some(value) = theme::switch(ui, ocean.blue_water_prepare_godot_mesh) {
                    ocean.blue_water_prepare_godot_mesh = value;
                    changed = true;
                }
            });
            theme::row(ui, "Legacy wave adapter (A/B)", "OFF leaves old JKA water textures stale; native surface continues using Blue Water waves.", theme::Reset::None, |ui| {
                if let Some(value) = theme::switch(ui, ocean.blue_water_adapter) {
                    ocean.blue_water_adapter = value;
                    changed = true;
                }
            });
            if hybrid && ocean.hybrid_caustics_mode == 3 {
                theme::row(ui, "Fast caustic receiver (A/B)", "Forward differences use 3 normal texture taps per caustic pixel instead of 4, with slightly less stable focusing. Keeps exact outline and scene-depth tests.", theme::Reset::None, |ui| {
                    if let Some(value) = theme::switch(ui, ocean.hybrid_caustics_fast) {
                        ocean.hybrid_caustics_fast = value;
                        changed = true;
                    }
                });
            }
            theme::hint(
                ui,
                "r_showtris 32 draws the native ocean mesh; render_perf_capture reports ocean GPU pass timings.",
                theme::TEXT_FAINT,
            );
        }

        if changed {
            self.video.ocean_settings = ocean;
            self.commit_ocean_settings();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bool_spellings_fold_to_the_same_setting() {
        assert!(same_setting("true", "1"));
        assert!(same_setting("ON", "1"));
        assert!(same_setting("0.000", "0"));
        assert!(same_setting("gpu", "GPU"));
        assert!(!same_setting("gpu", "cpu"));
    }

    #[test]
    fn optimization_table_has_no_duplicates() {
        let mut names = std::collections::BTreeSet::new();
        assert!(OPTIMIZATION_CVARS
            .iter()
            .all(|(name, _, _)| names.insert(name.to_ascii_lowercase())));
    }
}
