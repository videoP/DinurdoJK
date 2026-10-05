//! Models.
use crate::app::egui_settings::{
    quality_table_row, segmented_row, theme, ui, App, Ghoul2BatchMode, Ghoul2SkinningMode,
};

impl App {
    pub(in crate::app::egui_settings) fn egui_models(&mut self, ui: &mut egui::Ui) {
        self.egui_toggle_row(
            ui,
            "Map models",
            "Draws MD3 props the map's entities place in the world (misc_model_* and func_static models). Off removes them for a cleaner or faster view. Brush models such as doors and platforms, and any model geometry already compiled into the BSP by the map compiler, are always drawn.",
            ui::VIDEO_ROW_DRAW_MAP_MODELS,
            self.video.draw_map_models,
        );

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

        const GHOUL2_BATCHING: [(Ghoul2BatchMode, &str); 3] = [
            (Ghoul2BatchMode::Off, "Off"),
            (Ghoul2BatchMode::Adaptive, "Adaptive"),
            (Ghoul2BatchMode::Force, "Force"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "Ghoul2 draw batching",
            "GPU-skinning draw-call batching. Adaptive is the default: it skips batching below 32 GPU-skinned surfaces, samples at most 64 surfaces for a real 3-instance match, and only then groups matching opaque/masked mesh, material and LOD draws. Force always attempts grouping for profiling.",
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
            "Model LOD scale",
            "OpenJK r_lodscale for Ghoul2 model LODs (default 5). Larger keeps higher-detail GLM LODs at greater distances; smaller drops to cheaper LODs sooner. Model LOD bias is added afterward.",
            theme::Reset::None,
            |ui| {
                let mut scale = self.video.lod_scale;
                if ui
                    .add(egui::DragValue::new(&mut scale).range(0.5..=20.0).speed(0.1).update_while_editing(false))
                    .changed()
                {
                    let _ = self.set_console_cvar("r_lodScale", &scale.to_string());
                }
            },
        );

        // Cheapest first, like the other quality meters; the quality presets only
        // use 0..=3, and the console still takes up to 8 (shown as Low).
        const MODEL_DETAIL: [(i32, &str); 4] =
            [(3, "Low"), (2, "Medium"), (1, "High"), (0, "Highest")];
        if let Some(index) = quality_table_row(
            ui,
            "Model detail",
            "r_lodbias. How early Ghoul2 models drop to their cheaper authored LODs. Highest (0) uses projected screen size normally and is the OpenJK default; lower settings bias toward cheaper GLM LODs. Model LOD scale is applied first.",
            theme::Reset::Video(ui::VIDEO_ROW_GHOUL2_LOD_BIAS),
            self.video.ghoul2_lod_bias,
            &MODEL_DETAIL,
            0,
        ) {
            let _ = self.set_console_cvar("r_lodbias", &MODEL_DETAIL[index].0.to_string());
        }

        self.egui_toggle_row(
            ui,
            "Offscreen early cull",
            "Skips the final full Ghoul2 pose and render submission once a model is outside the view frustum, while retaining the lightweight animation/angle state needed for smooth re-entry. Disable for A/B diagnostics.",
            ui::VIDEO_ROW_GHOUL2_EARLY_CULL,
            self.video.ghoul2_early_cull,
        );
    }
}
