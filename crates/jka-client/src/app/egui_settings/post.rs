//! Post.
use crate::app::egui_settings::{
    index_of, percent, quality_table_row, segmented_row, theme, ui, App, CrtMask, CrtQuality,
    CrtReferenceResolution, CrtScope, CrtStyle, DofMode, DofQuality, GlowMode,
};

impl App {
    pub(in crate::app::egui_settings) fn egui_post_processing(&mut self, ui: &mut egui::Ui) {
        theme::section(
            ui,
            "CRT",
            "Native Timothy Lottes CRT filters. The scope controls which UI layers are sent through the CRT pass.",
        );

        const CRT_SCOPE: [(CrtScope, &str); 4] = [
            (CrtScope::Off, "Off"),
            (CrtScope::Scene, "Scene"),
            (CrtScope::SceneHud, "Scene + HUD"),
            (CrtScope::Everything, "Everything"),
        ];
        if let Some(scope) = segmented_row(
            ui,
            "CRT",
            "Off skips the pass entirely. Scene keeps HUD/UI crisp; Scene + HUD filters gameplay HUD too; Everything filters the final composited window.",
            theme::Reset::Video(ui::VIDEO_ROW_CRT_SCOPE),
            self.video.crt_scope,
            &CRT_SCOPE,
        ) {
            self.set_crt_scope(scope);
        }

        if self.video.crt_scope != CrtScope::Off {
            const CRT_STYLE: [(CrtStyle, &str); 2] = [
                (CrtStyle::LottesFast, "Lottes Fast"),
                (CrtStyle::Lottes, "Lottes"),
            ];
            if let Some(style) = segmented_row(
                ui,
                "CRT style",
                "Lottes Fast is the optimized 2/8-tap CRTS port. Lottes is Timothy Lottes' original full 42-fetch filter with its built-in bloom and upstream defaults.",
                theme::Reset::Video(ui::VIDEO_ROW_CRT_STYLE),
                self.video.crt_style,
                &CRT_STYLE,
            ) {
                self.set_crt_style(style);
            }

            const CRT_REFERENCE: [(CrtReferenceResolution, &str); 4] = [
                (CrtReferenceResolution::P480, "480p"),
                (CrtReferenceResolution::P600, "600p"),
                (CrtReferenceResolution::P720, "720p"),
                (CrtReferenceResolution::Native, "Native"),
            ];
            if let Some(target) = quality_table_row(
                ui,
                "Reference resolution",
                "Virtual source resolution used by the CRT reconstruction. It does not lower the renderer's actual resolution.",
                theme::Reset::Video(ui::VIDEO_ROW_CRT_REFERENCE),
                self.video.crt_reference,
                &CRT_REFERENCE,
                0,
            ) {
                self.set_crt_reference(CRT_REFERENCE[target].0);
            }

            if self.video.crt_style == CrtStyle::LottesFast {
            const CRT_QUALITY: [(CrtQuality, &str); 2] = [
                (CrtQuality::Fast, "Fast (2 tap)"),
                (CrtQuality::Quality, "Quality (8 tap)"),
            ];
            if let Some(target) = quality_table_row(
                ui,
                "Quality",
                "Uses crt-lottes-fast's real 2-tap path or its normal 8-fetch reconstruction path.",
                theme::Reset::Video(ui::VIDEO_ROW_CRT_QUALITY),
                self.video.crt_quality,
                &CRT_QUALITY,
                0,
            ) {
                self.set_crt_quality(CRT_QUALITY[target].0);
            }

            const CRT_MASK: [(CrtMask, &str); 4] = [
                (CrtMask::None, "Off"),
                (CrtMask::GrilleLite, "Grille lite"),
                (CrtMask::Grille, "Grille"),
                (CrtMask::Shadow, "Shadow"),
            ];
            if let Some(mask) = segmented_row(
                ui,
                "Phosphor mask",
                "Lottes mask layout. Grille Lite is the brighter default; Shadow uses a staggered RGB mask.",
                theme::Reset::Video(ui::VIDEO_ROW_CRT_MASK),
                self.video.crt_mask,
                &CRT_MASK,
            ) {
                self.set_crt_mask(mask);
            }

            theme::row(
                ui,
                "Scanline strength",
                "Controls scanline thinness/contrast in the original Lottes filter.",
                theme::Reset::Video(ui::VIDEO_ROW_CRT_SCANLINE),
                |ui| {
                    let mut value = self.video.crt_scanline;
                    let readout = percent(value);
                    if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                        self.set_crt_scanline(value);
                    }
                },
            );
            if self.video.crt_quality == CrtQuality::Quality {
                theme::row(
                    ui,
                    "Horizontal sharpness",
                    "Gaussian horizontal reconstruction sharpness for the 8-tap Quality path. Higher is tighter/sharper.",
                    theme::Reset::Video(ui::VIDEO_ROW_CRT_SHARPNESS),
                    |ui| {
                        let mut value = self.video.crt_sharpness;
                        let readout = format!("{value:.1}");
                        if theme::slider(ui, &mut value, 1.0..=6.0, &readout) {
                            self.set_crt_sharpness(value);
                        }
                    },
                );
            }
            theme::row(
                ui,
                "Mask strength",
                "Strength of the RGB phosphor mask. Zero makes the selected mask effectively neutral.",
                theme::Reset::Video(ui::VIDEO_ROW_CRT_MASK_INTENSITY),
                |ui| {
                    let mut value = self.video.crt_mask_intensity;
                    let readout = percent(value);
                    if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                        self.set_crt_mask_intensity(value);
                    }
                },
            );
            theme::row(
                ui,
                "Curvature",
                "Curves the virtual CRT glass. Zero keeps the image flat.",
                theme::Reset::Video(ui::VIDEO_ROW_CRT_CURVATURE),
                |ui| {
                    let mut value = self.video.crt_curvature;
                    let readout = format!("{value:.3}");
                    if theme::slider(ui, &mut value, 0.0..=0.25, &readout) {
                        self.set_crt_curvature(value);
                    }
                },
            );
            theme::row(
                ui,
                "Corner rounding",
                "Softens/rounds the visible CRT corners using Lottes' CORNER parameter.",
                theme::Reset::Video(ui::VIDEO_ROW_CRT_CORNER),
                |ui| {
                    let mut value = self.video.crt_corner;
                    let readout = format!("{value:.1}");
                    if theme::slider(ui, &mut value, 0.0..=11.0, &readout) {
                        self.set_crt_corner(value);
                    }
                },
            );
            }
        }

        theme::section(ui, "CAMERA EFFECTS", "Lens and camera-space finishing.");
        const GLOW: [(GlowMode, &str); 4] = [
            (GlowMode::Off, "Off"),
            (GlowMode::JkaAuthored, "JKA Authored"),
            (GlowMode::ModernBloom, "Modern Bloom"),
            (GlowMode::JkaAndBloom, "JKA + Bloom"),
        ];
        if let Some(target) = quality_table_row(
            ui,
            "Glow",
            "Controls what feeds the shared glow pyramid. JKA Authored uses only shader stages \
             explicitly marked glow; Modern Bloom uses HDR brightness; JKA + Bloom combines both.",
            theme::Reset::Video(ui::VIDEO_ROW_GLOW),
            self.video.glow,
            &GLOW,
            0,
        ) {
            let current = index_of(&GLOW, self.video.glow, 0);
            self.video_selected = ui::VIDEO_ROW_GLOW;
            self.change_video_setting(target as i32 - current as i32);
        }

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
        const DOF_MODES: [(DofMode, &str); 3] = [
            (DofMode::Off, "Off"),
            (DofMode::UnderwaterOnly, "Underwater only"),
            (DofMode::On, "On"),
        ];
        if let Some(mode) = segmented_row(
            ui,
            "Depth of field mode",
            "r_dofMode: Off disables blur; Underwater only focuses at a fixed 48 game units (no autofocus); On uses the normal focus behavior.",
            theme::Reset::Video(ui::VIDEO_ROW_DOF_MODE),
            self.video.dof_mode,
            &DOF_MODES,
        ) {
            self.set_dof_mode(mode);
        }

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

        if self.video.dof_mode == DofMode::On {
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
        }

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
        self.egui_post_color_looks(ui);
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
