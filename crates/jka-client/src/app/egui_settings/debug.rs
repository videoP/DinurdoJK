//! Debug.
use crate::app::egui_settings::{
    index_of, quality_table_row, segmented_row, theme, ui, App, CullDebugMode,
    PlanarReflectionDebugMode,
};

impl App {
    pub(in crate::app::egui_settings) fn egui_debug_tools(&mut self, ui: &mut egui::Ui) {
        const DIAGNOSTIC_LEVELS: [(u8, &str); 4] =
            [(0, "Off"), (1, "Basic"), (2, "Verbose"), (3, "Trace")];

        if let Some(level) = segmented_row(
            ui,
            "Developer output",
            "developer. Global diagnostic gate. Off keeps routine engine/cgame diagnostics out of the console; Basic shows lifecycle/setup information; Verbose adds per-entity and worker/job diagnostics; Trace is reserved for very noisy tracing. Any non-zero level also enables developer-only inspector tools.",
            theme::Reset::Video(ui::VIDEO_ROW_DEVELOPER_TOOLS),
            self.video.developer_level,
            &DIAGNOSTIC_LEVELS,
        ) {
            let _ = self.set_console_cvar("developer", &level.to_string());
        }

        if let Some(level) = segmented_row(
            ui,
            "Renderer verbose output",
            "r_verbose. Renderer-only diagnostic gate, matching the classic JKA renderer cvar's purpose. Use this when you want renderer allocation/pipeline/material diagnostics without enabling cgame/client developer spew. developer at the same level also enables these lines.",
            theme::Reset::Video(ui::VIDEO_ROW_RENDERER_VERBOSE),
            self.video.renderer_verbose,
            &DIAGNOSTIC_LEVELS,
        ) {
            let _ = self.set_console_cvar("r_verbose", &level.to_string());
        }

        self.egui_toggle_row(
            ui,
            "Lightmap-only debug view",
            "Classic r_lightmap debug view: show the baked lightmap/vertex-light contribution without the diffuse texture where available.",
            ui::VIDEO_ROW_LIGHTMAP_ONLY,
            self.video.lightmap_only,
        );

        let supported = self.wireframe_supported;
        if !supported {
            theme::hint(
                ui,
                "Wireframe categories are unavailable on this adapter (non-solid polygon fill unsupported).",
                theme::TEXT_DISABLED,
            );
        }

        let categories = [
            (
                ui::wireframe::MAP,
                "Map",
                "BSP/map triangles (excluding promoted ocean clipmaps).",
            ),
            (
                ui::wireframe::PLAYERS,
                "Players",
                "Player Ghoul2/MD3 geometry, including GPU-skinned surfaces.",
            ),
            (
                ui::wireframe::ENTITIES,
                "Entities",
                "Items, vehicles, model entities and inline BSP brush movers.",
            ),
            (
                ui::wireframe::EFFECTS,
                "Effects",
                "Transient FX/event geometry plus map-authored surface sprites.",
            ),
            (
                ui::wireframe::GRASS,
                "Procedural grass",
                "The actual animated per-blade grass mesh after LOD/wind deformation.",
            ),
            (
                ui::wireframe::OCEAN,
                "Ocean",
                "Promoted ocean clipmap triangles after wave displacement.",
            ),
            (
                ui::wireframe::DEFORMATION,
                "Surface deformation",
                "3D Snowflow/footprint shell geometry after deformation.",
            ),
        ];
        for (bit, label, help) in categories {
            theme::row(ui, label, help, theme::Reset::None, |ui| {
                ui.add_enabled_ui(supported, |ui| {
                    let enabled = self.video.wireframe_mask & bit != 0;
                    if theme::switch(ui, enabled).is_some() {
                        self.video.wireframe_mask ^= bit;
                        self.render_command(crate::renderer::RenderCommand::SetWireframeMask(
                            self.video.wireframe_mask,
                        ));
                        self.mark_config_dirty();
                    }
                });
            });
        }

        self.egui_toggle_row(
            ui,
            "Draw triggers",
            "Draws every trigger_* volume as a translucent colored brush with an outline: push green, teleport purple, hurt red, multiple blue, once cyan, other yellow. Built once per map and drawn from static buffers in two draws, so it costs nothing while off and very little while on. Not saved between sessions.",
            ui::VIDEO_ROW_DRAW_TRIGGERS,
            self.video.draw_triggers,
        );
        self.egui_toggle_row(
            ui,
            "Draw clip brushes",
            "Draws clip-only brushes (invisible collision the compiler leaves out of the render mesh): player clip orange, shot clip magenta, monster/bot clip yellow. Static buffers, two draws, no cost while off. Not saved between sessions.",
            ui::VIDEO_ROW_DRAW_CLIP_BRUSHES,
            self.video.draw_clip_brushes,
        );
        self.egui_toggle_row(
            ui,
            "Draw entities",
            "NetRadiant-style overlay: a colored box and classname label above every map entity, colored by category, with lines to its target/targetname links. Rebuilt every tick while on (map entity counts are small), and keeps egui ticking every frame like any other overlay. Not saved between sessions.",
            ui::VIDEO_ROW_DRAW_ENTITIES,
            self.video.draw_entities,
        );

        const CULL: [(CullDebugMode, &str); 2] = [
            (CullDebugMode::Off, "Off"),
            (CullDebugMode::RejectionReasons, "Rejection reasons"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "Cull rejection debug",
            "Tints each batch by the reason it was culled, so over-aggressive \
             culling can be traced back to a specific test.",
            theme::Reset::Video(ui::VIDEO_ROW_CULL_DEBUG),
            self.video.cull_debug,
            &CULL,
        ) {
            let current = usize::from(self.video.cull_debug != CullDebugMode::Off);
            let next = usize::from(target != CullDebugMode::Off);
            self.video_selected = ui::VIDEO_ROW_CULL_DEBUG;
            self.change_video_setting(next as i32 - current as i32);
        }

        const FPS: [(u8, &str); 3] = [(0, "Off"), (1, "Simple"), (2, "Detailed")];
        let fps_current = self.video.draw_fps.min(2);
        if let Some(target) = segmented_row(
            ui,
            "Frame rate display",
            "Simple shows frames per second. Detailed adds CPU and GPU pass \
             timings and per-thread activity.",
            theme::Reset::Video(ui::VIDEO_ROW_DRAW_FPS),
            fps_current,
            &FPS,
        ) {
            self.video_selected = ui::VIDEO_ROW_DRAW_FPS;
            self.change_video_setting(target as i32 - fps_current as i32);
        }

        self.egui_toggle_row(
            ui,
            "Perf console trace",
            "Writes per-frame timing lines to the console and the log. Noisy, \
             and it costs frame time itself.",
            ui::VIDEO_ROW_PERF_TRACE,
            self.video.perf_trace,
        );

        theme::section(
            ui,
            "PREDICTION",
            "Client-prediction diagnostics. These switches do not change Pmove results.",
        );
        theme::row(
            ui,
            "Prediction diagnostics",
            "cg_predictionDebug. Shows the current predicted origin/ground state, view correction, the OpenJK 0.25-unit ground trace and 64-unit ground probe, plus the last prediction miss. Rich miss details are also printed to the console.",
            theme::Reset::None,
            |ui| {
                if let Some(value) = theme::switch(ui, self.network.prediction_debug) {
                    let _ = self.set_console_cvar("cg_predictionDebug", if value { "1" } else { "0" });
                    self.publish_transient_ui();
                }
            },
        );
        theme::row(
            ui,
            "Print prediction misses",
            "cg_showMiss. Keeps the compact OpenJK-style prediction-miss line in the console. Prediction diagnostics adds richer state/trace lines.",
            theme::Reset::None,
            |ui| {
                if let Some(value) = theme::switch(ui, self.network.show_miss) {
                    let _ = self.set_console_cvar("cg_showMiss", if value { "1" } else { "0" });
                }
            },
        );
        theme::row(
            ui,
            "Highlight prediction misses",
            "cg_predictionMissHighlight. Flashes a red border for 350 ms when a correction exceeds the threshold, making rare one/few-frame failures easy to spot while reproducing them.",
            theme::Reset::None,
            |ui| {
                if let Some(value) = theme::switch(ui, self.network.prediction_miss_highlight) {
                    let _ = self.set_console_cvar("cg_predictionMissHighlight", if value { "1" } else { "0" });
                    self.publish_transient_ui();
                }
            },
        );
        theme::row(
            ui,
            "Miss highlight threshold",
            "cg_predictionMissThreshold. Correction distance in JKA units. 8 filters tiny routine corrections while still catching visible positional failures.",
            theme::Reset::None,
            |ui| {
                let mut value = self.network.prediction_miss_threshold;
                if ui
                    .add(
                        egui::DragValue::new(&mut value)
                            .range(0.0..=4096.0)
                            .speed(1.0)
                            .suffix(" u")
                            .update_while_editing(false),
                    )
                    .changed()
                {
                    let _ = self.set_console_cvar("cg_predictionMissThreshold", &format!("{value:.2}"));
                    self.publish_transient_ui();
                }
            },
        );
        if self.network.no_predict {
            theme::hint(
                ui,
                "cg_noPredict is enabled, so the live predictor and its ground traces are inactive.",
                theme::TEXT_DISABLED,
            );
        }

        theme::row(
            ui,
            "Event worker queue",
            "A/B test for client event processing. Off performs the same semantic preparation on the CGame thread. On prepares multi-event receive batches on a dedicated worker pool, then applies all player/audio/FX/presentation side effects in original queue order. Single-event batches intentionally stay inline.",
            theme::Reset::None,
            |ui| {
                if let Some(value) = theme::switch(ui, self.cg_event_workers) {
                    let _ = self.set_console_cvar("cg_eventWorkers", if value { "1" } else { "0" });
                }
            },
        );
        self.egui_toggle_row(
            ui,
            "GPU timestamps",
            "Queries the GPU for real per-pass durations instead of estimating \
             them from CPU submission time.",
            ui::VIDEO_ROW_GPU_TIMINGS,
            self.video.gpu_timings,
        );

        theme::section(ui, "REFLECTIONS", "Diagnostics for the Reflections page.");
        self.egui_toggle_row(
            ui,
            "Reflection resolver debug",
            "Shows the reflection source selected for each visible opaque/masked world surface. \
             Magenta = planar, green = valid SSR hit, blue = cubemap/probe fallback, \
             gray = SSR-eligible surface with no valid hit, black = no enhanced reflection path.",
            ui::VIDEO_ROW_PLANAR_REFLECTIONS,
            self.video.reflection_debug,
        );
        theme::row(
            ui,
            "Planar reflection debug",
            "Specialised planar-pass diagnostics: candidates, selected plane, render target, applied sample or binding test.",
            theme::Reset::Video(ui::VIDEO_ROW_PLANAR_REFLECTION_DEBUG),
            |ui| {
                egui::ComboBox::from_id_salt("planar_debug")
                    .selected_text(self.video.planar_reflection_debug.label())
                    .width(240.0)
                    .show_ui(ui, |ui| {
                        for (index, mode) in
                            PlanarReflectionDebugMode::ALL.iter().copied().enumerate()
                        {
                            if ui
                                .selectable_label(
                                    mode == self.video.planar_reflection_debug,
                                    mode.label(),
                                )
                                .clicked()
                            {
                                let current = PlanarReflectionDebugMode::ALL
                                    .iter()
                                    .position(|value| *value == self.video.planar_reflection_debug)
                                    .unwrap_or(0);
                                self.video_selected = ui::VIDEO_ROW_PLANAR_REFLECTION_DEBUG;
                                self.change_video_setting(index as i32 - current as i32);
                            }
                        }
                    });
            },
        );
    }

    /// Bake settings, shown under the Ambient occlusion row while Baked is selected.
    pub(in crate::app::egui_settings) fn egui_baked_ao(&mut self, ui: &mut egui::Ui) {
        const SAMPLES: [(u32, &str); 5] =
            [(8, "8"), (16, "16"), (32, "32"), (64, "64"), (128, "128")];
        if let Some(target) = quality_table_row(
            ui,
            "Baked AO samples",
            "Rays cast per bake point. More samples remove the blotchiness in \
             the result and make the bake take proportionally longer.",
            theme::Reset::Video(ui::VIDEO_ROW_BAKED_AO_SAMPLES),
            self.video.static_bsp_ao_samples,
            &SAMPLES,
            2,
        ) {
            let current = index_of(&SAMPLES, self.video.static_bsp_ao_samples, 2);
            self.video_selected = ui::VIDEO_ROW_BAKED_AO_SAMPLES;
            self.change_video_setting(target as i32 - current as i32);
        }

        const RESOLUTIONS: [(u32, &str); 3] = [(1, "1×"), (3, "3×"), (5, "5×")];
        if let Some(target) = quality_table_row(
            ui,
            "Baked AO resolution",
            "Density of bake points across a surface. Higher resolves occlusion \
             around small detail geometry at the cost of memory.",
            theme::Reset::Video(ui::VIDEO_ROW_BAKED_AO_RESOLUTION),
            self.video.static_bsp_ao_resolution,
            &RESOLUTIONS,
            1,
        ) {
            let current = index_of(&RESOLUTIONS, self.video.static_bsp_ao_resolution, 1);
            self.video_selected = ui::VIDEO_ROW_BAKED_AO_RESOLUTION;
            self.change_video_setting(target as i32 - current as i32);
        }

        const STRENGTHS: [(u32, &str); 4] = [(25, "25%"), (50, "50%"), (75, "75%"), (100, "100%")];
        if let Some(target) = quality_table_row(
            ui,
            "Baked AO strength",
            "How dark the baked occlusion is allowed to get before it is applied \
             to the lightmap.",
            theme::Reset::Video(ui::VIDEO_ROW_BAKED_AO_STRENGTH),
            self.video.static_bsp_ao_strength,
            &STRENGTHS,
            2,
        ) {
            let current = index_of(&STRENGTHS, self.video.static_bsp_ao_strength, 2);
            self.video_selected = ui::VIDEO_ROW_BAKED_AO_STRENGTH;
            self.change_video_setting(target as i32 - current as i32);
        }

        const RANGES: [(u32, &str); 4] = [(50, "0.5×"), (100, "1×"), (150, "1.5×"), (200, "2×")];
        if let Some(target) = quality_table_row(
            ui,
            "Baked AO range",
            "World distance the occlusion rays reach. Longer range darkens whole \
             rooms; shorter keeps it to creases and corners.",
            theme::Reset::Video(ui::VIDEO_ROW_BAKED_AO_RANGE),
            self.video.static_bsp_ao_range,
            &RANGES,
            1,
        ) {
            let current = index_of(&RANGES, self.video.static_bsp_ao_range, 1);
            self.video_selected = ui::VIDEO_ROW_BAKED_AO_RANGE;
            self.change_video_setting(target as i32 - current as i32);
        }

        self.egui_toggle_row(
            ui,
            "Bake current cell only",
            "Bakes just the PVS cell the camera stands in. Fast to iterate on \
             while tuning the settings above.",
            ui::VIDEO_ROW_BAKED_AO_CURRENT_CELL,
            self.video.static_bsp_ao_current_cell,
        );
    }
}
