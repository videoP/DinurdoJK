//! Post.
use crate::app::egui_settings::{index_of, percent, quality_table_row, theme, ui, App, DofQuality};

impl App {
    pub(in crate::app::egui_settings) fn egui_post_processing(&mut self, ui: &mut egui::Ui) {
        self.egui_toggle_row(
            ui,
            "Bloom",
            "Bleeds light from very bright pixels into their surroundings, the \
             way a real lens scatters light.",
            ui::VIDEO_ROW_BLOOM,
            self.video.bloom,
        );

        theme::row(
            ui,
            "Motion blur",
            "Smears the frame along per-pixel motion vectors. Small amounts \
             smooth fast turns; large amounts hurt target tracking.",
            theme::Reset::Video(ui::VIDEO_ROW_MOTION_BLUR),
            |ui| {
                let mut value = self.video.motion_blur_strength;
                let readout = percent(value);
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    self.set_motion_blur_strength(value);
                }
            },
        );
        theme::row(
            ui,
            "Depth of field",
            "Blurs what is not at the focus distance. Cinematic, but it softens \
             distant players.",
            theme::Reset::Video(ui::VIDEO_ROW_DEPTH_OF_FIELD),
            |ui| {
                let mut value = self.video.depth_of_field_strength;
                let readout = percent(value);
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    self.set_depth_of_field_strength(value);
                }
            },
        );

        theme::row(
            ui,
            "Autofocus",
            "r_dofAutoFocus. When depth of field is enabled, continuously focuses on the surface under the crosshair. Off holds the current focus distance.",
            theme::Reset::Video(ui::VIDEO_ROW_DOF_AUTOFOCUS),
            |ui| {
                if let Some(enabled) = theme::switch(ui, self.video.dof_autofocus) {
                    let _ = self.set_console_cvar("r_dofAutoFocus", if enabled { "1" } else { "0" });
                }
            },
        );

        const DOF: [(DofQuality, &str); 3] = [
            (DofQuality::Performance, "Performance"),
            (DofQuality::Adaptive, "Adaptive"),
            (DofQuality::High, "High"),
        ];
        if let Some(target) = quality_table_row(
            ui,
            "Depth of field quality",
            "Sample count of the blur kernel. Higher removes the ringing and \
             banding visible in strong blur at Performance.",
            theme::Reset::Video(ui::VIDEO_ROW_DOF_QUALITY),
            self.video.dof_quality,
            &DOF,
            1,
        ) {
            let current = index_of(&DOF, self.video.dof_quality, 1);
            self.video_selected = ui::VIDEO_ROW_DOF_QUALITY;
            self.change_video_setting(target as i32 - current as i32);
        }

        self.egui_film(ui);
    }

    pub(in crate::app::egui_settings) fn egui_film(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "FILM EMULATION", "Photochemical-inspired finishing.");
        self.egui_toggle_row(
            ui,
            "Film halation",
            "The warm glow film gets when bright light scatters back off the \
             backing layer. Softer and redder than bloom.",
            ui::VIDEO_ROW_HALATION,
            self.video.halation,
        );
        theme::row(
            ui,
            "Chromatic aberration",
            "Chromatic aberration: colour channels are offset slightly toward \
             the frame edges, as a real lens does.",
            theme::Reset::Video(ui::VIDEO_ROW_CHROMATIC_ABERRATION),
            |ui| {
                let mut value = self.video.chromatic_aberration;
                let readout = format!("{value:.0}%");
                if theme::slider(ui, &mut value, 0.0..=100.0, &readout) {
                    self.set_chromatic_aberration_strength(value);
                }
            },
        );
        self.egui_toggle_row(
            ui,
            "Vignette",
            "Darkens the corners of the frame the way a lens falls off toward \
             its edges.",
            ui::VIDEO_ROW_VIGNETTE,
            self.video.vignette,
        );
        theme::row(
            ui,
            "Film grain",
            "Animated noise over the frame. A little grain hides banding in dark \
             gradients.",
            theme::Reset::Video(ui::VIDEO_ROW_FILM_GRAIN),
            |ui| {
                let mut value = self.video.film_grain_strength;
                let readout = percent(value);
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    self.set_film_grain_strength(value);
                }
            },
        );
    }
}
