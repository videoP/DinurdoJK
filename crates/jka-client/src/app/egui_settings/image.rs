//! Image.
use crate::app::egui_settings::{
    index_of, quality_row, quality_table_row, segmented_row, theme, ui, App, DetailTextureMode,
    PvsMode, RenderCommand, TextureFilter,
};

impl App {
    pub(in crate::app::egui_settings) fn egui_image_quality(&mut self, ui: &mut egui::Ui) {
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

        const DETAIL_TEXTURES: [(DetailTextureMode, &str); 5] = [
            (DetailTextureMode::Off, "Off"),
            (DetailTextureMode::Neutral2x, "Neutral 2×"),
            (DetailTextureMode::Linear2x, "Linear 2×"),
            (DetailTextureMode::DstColorOne, "DstColor + One"),
            (DetailTextureMode::Multiply, "Multiply"),
        ];
        if let Some(target) = quality_table_row(
            ui,
            "Detail textures",
            "Adds the selected textures/japro/detail/* image as a synthetic high-frequency layer on \
             eligible opaque BSP materials. Modes expose several blend equations for A/B testing; \
             authored detail stages are left untouched. Off keeps the stripped fast BSP path.",
            theme::Reset::Video(ui::VIDEO_ROW_DETAIL_TEXTURES),
            self.video.detail_textures,
            &DETAIL_TEXTURES,
            0,
        ) {
            let current = index_of(&DETAIL_TEXTURES, self.video.detail_textures, 0);
            self.video_selected = ui::VIDEO_ROW_DETAIL_TEXTURES;
            self.change_video_setting(target as i32 - current as i32);
        }

        ui.add_enabled_ui(self.video.detail_textures != DetailTextureMode::Off, |ui| {
        theme::row(
            ui,
            "Detail distance fade",
            "Ports the standard distance-based detail blend: camera/world distance is divided by the fade distance, raised to the fourth power and clamped, then the detail contribution is lerped back to its neutral identity. This avoids alpha/transparency rendering and keeps the normal opaque BSP path.",
            theme::Reset::None,
            |ui| {
                let mut enabled = self.video.detail_texture_fade;
                if ui.checkbox(&mut enabled, "Enabled").changed() {
                    self.video.detail_texture_fade = enabled;
                    self.render_command(RenderCommand::SetDetailTextureFade {
                        enabled,
                        distance: self.video.detail_texture_fade_distance,
                    });
                    self.mark_config_dirty();
                }
            },
        );
        });

        if self.video.detail_texture_fade && self.video.detail_textures != DetailTextureMode::Off {
            theme::row(
                ui,
                "Detail fade distance",
                "Distance in JKA map units where the ported distance fade reaches the neutral/no-detail result. The reference technique uses 512 units.",
                theme::Reset::None,
                |ui| {
                    let mut value = self.video.detail_texture_fade_distance;
                    let readout = format!("{value:.0}");
                    if theme::slider(ui, &mut value, 64.0..=8192.0, &readout) {
                        self.video.detail_texture_fade_distance = value;
                        self.render_command(RenderCommand::SetDetailTextureFade {
                            enabled: self.video.detail_texture_fade,
                            distance: value,
                        });
                        self.mark_config_dirty();
                    }
                },
            );
        }

        // HDR is a framebuffer format decision, not a post effect: it decides
        // the precision everything downstream (bloom, tone mapping) works in.
        self.egui_toggle_row(
            ui,
            "HDR rendering",
            "Renders into a floating-point buffer so highlights can exceed white \
             instead of clipping. Costs bandwidth, and is what gives bloom and \
             tone mapping something to work with.",
            ui::VIDEO_ROW_HDR,
            self.video.hdr,
        );
        ui.add_enabled_ui(self.video.hdr, |ui| {
        self.egui_toggle_row(
            ui,
            "Float lightmaps",
            "Rend2-compatible HDR baked lighting. While HDR rendering is enabled, \
             load maps/<map>/lm_XXXX.hdr companions into FP16 when they exist; \
             ordinary JKA lightmaps are promoted to FP16 as the fallback. While a map is loaded, Apply Video Settings reloads it with the new setting.",
            ui::VIDEO_ROW_FLOAT_LIGHTMAP,
            self.video.float_lightmap,
        );
        });
    }

    pub(in crate::app::egui_settings) fn egui_visibility(&mut self, ui: &mut egui::Ui) {
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
    }
}
