//! Image.
use crate::app::egui_settings::{
    index_of, quality_row, quality_table_row, segmented_row, theme, ui, App,
    RenderCommand, TextureFilter,
};

impl App {
    pub(in crate::app::egui_settings) fn egui_image_quality(&mut self, ui: &mut egui::Ui) {
        theme::section(
            ui,
            "IMAGE QUALITY",
            "Anti-aliasing, texture filtering and texture detail.",
        );
        let choices = self.anti_aliasing_ladder();
        let current_aa = self.current_anti_aliasing();
        let current = choices
            .iter()
            .position(|(value, _)| *value == current_aa)
            .unwrap_or(0);
        let labels: Vec<&str> = choices.iter().map(|(_, text)| text.as_str()).collect();
        if let Some(target) = quality_row(
            ui,
            "Anti-aliasing",
            "Edge smoothing, cheapest first. FXAA and SMAA are screen-space \
             filters, MSAA supersamples geometry edges only, and TAA \
             accumulates jittered frames so it also resolves shader and \
             specular aliasing the others leave behind.",
            theme::Reset::Video(ui::VIDEO_ROW_ANTI_ALIASING),
            current,
            &labels,
        ) {
            self.video_selected = ui::VIDEO_ROW_ANTI_ALIASING;
            self.change_video_setting(target as i32 - current as i32);
        }

        const FILTERS: [(TextureFilter, &str); 7] = [
            (TextureFilter::Nearest, "Nearest"),
            (TextureFilter::Bilinear, "Bilinear"),
            (TextureFilter::Trilinear, "Trilinear"),
            (TextureFilter::Anisotropic2x, "2× AF"),
            (TextureFilter::Anisotropic4x, "4× AF"),
            (TextureFilter::Anisotropic8x, "8× AF"),
            (TextureFilter::Anisotropic16x, "16× AF"),
        ];
        if let Some(target) = quality_table_row(
            ui,
            "Texture filtering",
            "How textures are sampled at a distance and at grazing angles. \
             Anisotropic filtering keeps floors and walls sharp far away and is \
             nearly free on modern hardware.",
            theme::Reset::Video(ui::VIDEO_ROW_TEXTURE_FILTER),
            self.video.texture_filter,
            &FILTERS,
            2,
        ) {
            let current = index_of(&FILTERS, self.video.texture_filter, 2);
            self.video_selected = ui::VIDEO_ROW_TEXTURE_FILTER;
            self.change_video_setting(target as i32 - current as i32);
        }

        // quality_table_row is ordered low -> high quality. r_picmip runs in
        // the opposite numeric direction, so present the values reversed.
        const PICMIP: [(u32, &str); 5] =
            [(4, "1/16"), (3, "1/8"), (2, "1/4"), (1, "1/2"), (0, "Full")];
        if let Some(target) = quality_table_row(
            ui,
            "Texture quality",
            "Classic r_picmip. Omits the highest map-texture mip levels at upload: 0 keeps full resolution, 1 halves each dimension, 2 quarters it, and so on. Matches TaystJK's latched texture-quality behavior; Apply Video Settings reloads the current map.",
            theme::Reset::Video(ui::VIDEO_ROW_PICMIP),
            self.video.picmip.min(4),
            &PICMIP,
            0,
        ) {
            let current = PICMIP
                .iter()
                .position(|(value, _)| *value == self.video.picmip.min(4))
                .unwrap_or(PICMIP.len() - 1);
            self.video_selected = ui::VIDEO_ROW_PICMIP;
            self.change_video_setting(target as i32 - current as i32);
        }

        // Existing r_ext_compress_textures setting; no second setting/state.
        // Texture uploads are compressed on map load, not mid-frame.
        const TEXTURE_COMPRESSION: [(u8, &str); 3] = [
            (0, "Off"),
            (1, "BC1/BC3"),
            (2, "BC + lightmaps"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "Texture compression",
            "0 keeps original 8-bit textures; 1 uses BC1/BC3 for eligible color textures; 2 also compresses 8-bit lightmaps. Saves VRAM, but BC formats are lossy. Applies when loading the next map or after vid_restart.",
            theme::Reset::Video(ui::VIDEO_ROW_TEXTURE_COMPRESSION),
            self.video.texture_compression.min(2),
            &TEXTURE_COMPRESSION,
        ) {
            self.video_selected = ui::VIDEO_ROW_TEXTURE_COMPRESSION;
            let _ = self.set_console_cvar("r_ext_compress_textures", &target.to_string());
        }

        theme::row(
            ui,
            "Detail textures",
            "Adds textures/japro/detail/* to eligible opaque BSP materials using neutral 2× modulation. Authored detail stages are unchanged.",
            theme::Reset::Video(ui::VIDEO_ROW_DETAIL_TEXTURES),
            |ui| {
                let mut enabled = theme::preview_toggle_value(
                    ui.ctx(),
                    theme::Reset::Video(ui::VIDEO_ROW_DETAIL_TEXTURES),
                    self.video.detail_textures,
                );
                if ui.checkbox(&mut enabled, "Enabled").changed() {
                    self.video_selected = ui::VIDEO_ROW_DETAIL_TEXTURES;
                    self.video.detail_textures = enabled;
                    self.render_command(RenderCommand::SetDetailTextures(enabled));
                    self.mark_config_dirty();
                }
            },
        );
    }

    pub(in crate::app::egui_settings) fn egui_visibility(&mut self, ui: &mut egui::Ui) {
        theme::section(
            ui,
            "WORLD VISIBILITY",
            "How far away world geometry is still drawn.",
        );
        theme::row(
            ui,
            "Distance cull",
            "Scales how far away geometry is still drawn. Leave at the map \
             default unless a map pops in too aggressively.",
            theme::Reset::Video(ui::VIDEO_ROW_DISTANCE_CULL),
            |ui| {
                let mut value = self.video.distance_cull_scale;
                let readout = if self.video.distance_cull_scale <= 0.001 {
                    "Map default".to_owned()
                } else {
                    format!("{:.2}×", self.video.distance_cull_scale)
                };
                if theme::slider(ui, &mut value, 0.0..=ui::MAX_DISTANCE_CULL_SCALE, &readout) {
                    self.set_distance_cull_scale(value);
                }
            },
        );
    }
}
