//! Models.
use crate::app::egui_settings::{
    quality_table_row, theme, ui, App,
};

impl App {
    pub(in crate::app::egui_settings) fn egui_models(&mut self, ui: &mut egui::Ui) {
        theme::section(
            ui,
            "MODELS",
            "Map props and model detail.",
        );
        theme::row(ui,"Smooth emotes","Four-frame cubic interpolation for emote body, props and grounding at render rate (including 60 FPS and above). Keeps authored timing and audio; disable to use linear interpolation.",theme::Reset::None,|ui|{
            let mut enabled=self.video.emote_cubic;
            if ui.checkbox(&mut enabled,"Enabled").changed(){let _=self.set_console_cvar("cg_emoteCubic",if enabled{"1"}else{"0"});}
        });
        theme::row(ui,"Emote effects","Particles, smoke, sparkles, mesh debris and lights compiled offline from each emote's Niagara systems and played back with the animation clock.",theme::Reset::None,|ui|{
            let mut enabled=self.video.emote_effects;
            if ui.checkbox(&mut enabled,"Enabled").changed(){let _=self.set_console_cvar("cg_emoteEffects",if enabled{"1"}else{"0"});}
        });
        self.egui_toggle_row(
            ui,
            "Map models",
            "Draws MD3 props the map's entities place in the world (misc_model_* and func_static models). Off removes them for a cleaner or faster view. Brush models such as doors and platforms are controlled by the separate Inline BSP models switch below; static BSP geometry is unaffected.",
            ui::VIDEO_ROW_DRAW_MAP_MODELS,
            self.video.draw_map_models,
        );

        theme::row(ui, "Inline BSP models",
            "Render-only A/B switch for server-controlled BSP brush movers (fish, doors, platforms, rotating objects). Does not change collision, server gameplay or BSP static world geometry.",
            theme::Reset::None,
            |ui| {
                if let Some(enabled) = theme::switch(ui, self.video.draw_inline_models) {
                    let _ = self.set_console_cvar("r_drawInlineModels", if enabled { "1" } else { "0" });
                }
            },
        );
        theme::row(ui, "Built-in model surfaces",
            "Render-only shader-path filter for static BSP surfaces authored with models/* materials (e.g. compiled Costa del Sol trees). This is not true model provenance: map geometry sharing a models/* shader will also disappear. Does not affect collision or runtime MD3s.",
            theme::Reset::None,
            |ui| {
                if let Some(enabled) = theme::switch(ui, self.video.draw_bsp_model_surfaces) {
                    let _ = self.set_console_cvar("r_drawBspModelSurfaces", if enabled { "1" } else { "0" });
                }
            },
        );

        theme::row(
            ui,
            "Model LOD scale",
            "OpenJK/TaystJK r_lodscale for Ghoul2, MD3 and compiled BSP-model LODs (default 5). Larger keeps higher-detail meshes at greater distances; r_lodbias is added afterward. Medium, High and Ultra presets use the supported maximum of 100.",
            theme::Reset::Video(ui::VIDEO_ROW_MODEL_LOD_SCALE),
            |ui| {
                let mut scale = self.video.lod_scale;
                if ui
                    .add(egui::DragValue::new(&mut scale).range(crate::fx::LOD_SCALE_MIN..=crate::fx::LOD_SCALE_MAX).speed(0.1).update_while_editing(false))
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
            "r_lodbias. Higher values bias Ghoul2, MD3 and baked BSP models toward cheaper LODs after projected-size selection, exactly like TaystJK. Highest (0) is the default.",
            theme::Reset::Video(ui::VIDEO_ROW_GHOUL2_LOD_BIAS),
            self.video.ghoul2_lod_bias,
            &MODEL_DETAIL,
            0,
        ) {
            let _ = self.set_console_cvar("r_lodbias", &MODEL_DETAIL[index].0.to_string());
        }

        theme::row(
            ui,
            "Generate missing model LODs",
            "Automatic LOD for both entity MD3 models and eligible map-compiled BSP model surfaces (default On). MD3s generate missing levels on a cached worker; authored _1/_2.md3 variants always take priority. BSP simplification uses separate index ranges prepared at map load. Off draws full-detail baked models while retaining authored MD3 LODs. r_bspModelLod is a separate developer A/B override.",
            theme::Reset::None,
            |ui| {
                let mut enabled = self.video.md3_auto_lod;
                if ui.checkbox(&mut enabled, "On").changed() {
                    let _ = self.set_console_cvar("r_md3AutoLod", if enabled { "1" } else { "0" });
                }
            },
        );

    }
}
