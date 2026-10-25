//! Environment -> Glass. Menu changes use the same archived cvars as console edits.
use super::{color_row, ocean_slider, segmented_row, theme, App};
use crate::glass::{Settings, Style};

impl App {
    pub(in crate::app::egui_settings) fn egui_glass(&mut self, ui: &mut egui::Ui) {
        let promotion = self
            .latched_console_cvars
            .get("r_glasspromotion")
            .and_then(|value| Self::parse_cvar_bool("r_glassPromotion", value).ok())
            .unwrap_or_else(crate::materials::glass_promotion_enabled);
        theme::row(ui, "Glass promotion",
            "Enhance surfaces marked as glass by the map author. Off restores their original shaders. Apply Video Settings or reload the map after changing this.",
            theme::Reset::Glass("r_glassPromotion"), |ui| {
                if let Some(enabled) = theme::switch(ui, promotion) {
                    self.set_glass_menu_cvar("r_glassPromotion", if enabled { "1" } else { "0" });
                }
            });
        if self.latched_console_cvars.contains_key("r_glasspromotion") {
            theme::banner(
                ui,
                "Glass promotion needs Apply Video Settings or a map reload.",
                theme::WARNING,
            );
        }
        const STYLES: [(Style, &str); 4] = [
            (Style::Bevy, "Bevy"),
            (Style::Frosted, "Frosted"),
            (Style::Liquid, "Liquid"),
            (Style::Ocean, "Ocean"),
        ];
        if let Some(style) = segmented_row(ui, "Glass style",
            "Bevy: physical transmission. Frosted: soft blur. Liquid: refractive bevels and colored separation. Ocean: animated waves. Changing style resets all tuning below.",
            theme::Reset::Glass("r_glassStyle"), crate::glass::settings().style, &STYLES) {
            self.set_glass_menu_cvar("r_glassStyle", style.name());
        }
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Tuning updates live. Changing style restores its preset.")
                    .small()
                    .color(theme::TEXT_FAINT),
            );
            if theme::ghost_button(ui, "RESET STYLE TUNING").clicked() {
                self.set_glass_menu_cvar("r_glassStyle", crate::glass::settings().style.name());
            }
        });

        theme::section(
            ui,
            "APPEARANCE",
            "Clear transmission, frosting and colored haze.",
        );
        self.glass_slider(ui, "Effect strength", "r_glassEffectStrength", 0.0..=1.0,
            "Overall visibility of the promoted effect. Zero shows the unchanged scene; use promotion Off and Apply to restore the authored shader.");
        self.glass_slider(
            ui,
            "Blur radius",
            "r_glassBlur",
            0.0..=64.0,
            "Background blur in screen pixels. Larger values soften more detail.",
        );
        self.glass_slider(ui, "Frost strength", "r_glassFrost", 0.0..=1.0,
            "Mix sharp and blurred transmission. Zero keeps refraction sharp; one uses the full blur.");
        self.glass_slider(ui, "Haze opacity", "r_glassOpacity", 0.0..=1.0,
            "Fill the glass with its tint color. Independent of frosting: zero adds no fill and one covers the background. This control adds no render pass.");
        let mut tint = crate::glass::settings().tint;
        let mut changed = false;
        color_row(
            ui,
            "Glass tint",
            "Color of transmitted light and haze. White preserves neutral glass.",
            theme::Reset::Glass("r_glassTint"),
            &mut tint,
            &mut changed,
        );
        if changed {
            self.set_glass_menu_cvar(
                "r_glassTint",
                &format!("{} {} {}", tint[0], tint[1], tint[2]),
            );
        }
        self.glass_slider(
            ui,
            "Tint absorption",
            "r_glassTintStrength",
            0.0..=1.0,
            "How strongly the tint and the authored glass color filter transmitted light.",
        );
        self.glass_slider(
            ui,
            "Surface roughness",
            "r_glassRoughness",
            0.0..=1.0,
            "Additional transmission blur based on roughness and viewing distance.",
        );
        self.glass_slider(ui, "Reflection strength", "r_glassReflection", 0.0..=1.0,
            "Environment reflection, strongest at grazing angles. Lower it for a clearer view through the glass.");
        self.glass_slider(
            ui,
            "Edge frosting",
            "r_glassEdgeBlur",
            0.0..=64.0,
            "Extra blur near the pane boundaries. The bevel width sets how far inward it extends.",
        );
        self.glass_mode(
            ui,
            "Edge frosting direction",
            "r_glassEdgeBlurMode",
            "Apply edge frosting around the whole pane, or emphasize edges facing up and down.",
            &[(0.0, "All edges"), (1.0, "Top / bottom")],
        );

        theme::section(
            ui,
            "REFRACTION & EDGES",
            "Body distortion, chromatic separation and pane bevels.",
        );
        self.glass_slider(ui, "Index of refraction", "r_glassIOR", 1.0..=2.5,
            "How strongly light bends through the material. Ordinary glass is around 1.5; one removes physical refraction.");
        self.glass_slider(
            ui,
            "Glass thickness",
            "r_glassThickness",
            0.1..=128.0,
            "Approximate thickness in map units. Thicker glass produces more refraction.",
        );
        self.glass_slider(ui, "Body distortion", "r_glassDistortion", 0.0..=8.0,
            "Refraction across the body of the pane. Bevel refraction has its own strength control.");
        self.glass_slider(ui, "Chromatic separation", "r_glassDispersion", 0.0..=0.25,
            "Separate red and blue along the refraction direction. Zero removes the colored fringe.");
        self.glass_slider(
            ui,
            "Chromatic multiplier",
            "r_glassDispersionBase",
            0.0..=4.0,
            "Additional multiplier for chromatic separation. Zero disables it.",
        );
        self.glass_slider(ui, "Maximum displacement", "r_glassMaxDisplacement", 0.0..=128.0,
            "Cap refraction displacement in screen pixels. Smaller values limit extreme distortion.");
        self.glass_slider(ui, "Magnification", "r_glassMagnification", -1.0..=1.0,
            "Zoom the scene toward the visible pane center. Zero disables it; nonzero values add sampling work.");
        self.glass_slider(ui, "Bevel width", "r_glassEdgeWidth", 0.0..=128.0,
            "Width of the refractive edge, inward from the visible pane boundary, in screen pixels.");
        self.glass_slider(
            ui,
            "Bevel refraction",
            "r_glassEdgeStrength",
            0.0..=8.0,
            "Refraction strength along the beveled edge. Zero disables this distortion.",
        );
        const PROFILES: [(&str, &str); 4] = [
            ("convex", "Convex"),
            ("squircle", "Squircle"),
            ("concave", "Concave"),
            ("lip", "Lip"),
        ];
        if let Some(profile) = segmented_row(
            ui,
            "Bevel profile",
            "Shape of the refractive pane edge.",
            theme::Reset::Glass("r_glassEdgeProfile"),
            crate::glass::settings().edge_profile.name(),
            &PROFILES,
        ) {
            self.set_glass_menu_cvar("r_glassEdgeProfile", profile);
        }

        theme::section(ui, "RIM HIGHLIGHTS", "Light along the glass boundaries.");
        self.glass_mode(
            ui,
            "Rim style",
            "r_glassRimType",
            "Use white highlights, or enhance the background colors at the rim first.",
            &[(0.0, "Color enhanced"), (1.0, "White")],
        );
        self.glass_slider(
            ui,
            "Rim strength",
            "r_glassRimStrength",
            0.0..=2.0,
            "Brightness of the edge highlights. Zero disables them.",
        );
        self.glass_slider(
            ui,
            "Rim width",
            "r_glassRimWidth",
            0.0..=32.0,
            "Highlight width in screen pixels.",
        );
        self.glass_slider(
            ui,
            "Rim softness",
            "r_glassRimSoftness",
            0.1..=32.0,
            "Feather the highlight over this many screen pixels.",
        );
        self.glass_slider(
            ui,
            "Light direction",
            "r_glassRimAngle",
            0.0..=360.0,
            "Direction of the rim light in screen degrees.",
        );
        self.glass_slider(ui, "Rim color saturation", "r_glassRimSaturation", 0.0..=4.0,
            "Color saturation for Color enhanced rims. One preserves saturation; White rims ignore this setting.");

        theme::section(ui, "ANIMATION", "Optional procedural waves on the glass.");
        self.glass_slider(
            ui,
            "Waviness",
            "r_glassWaviness",
            0.0..=4.0,
            "Wave distortion. Zero keeps the surface flat.",
        );
        self.glass_slider(
            ui,
            "Animation speed",
            "r_glassAnimationSpeed",
            0.0..=4.0,
            "Wave animation rate. Zero freezes the waves.",
        );

        theme::section(
            ui,
            "FOREGROUND HANDLING",
            "Control artifacts around players and other foreground objects.",
        );
        self.glass_slider(ui, "Foreground guard", "r_glassForegroundGuard", 0.0..=1.0,
            "Fade refraction near foreground silhouettes when the hidden background cannot be recovered.");
        theme::row(ui, "Background recovery",
            "Save the world before opaque models draw to reduce contours around foreground players. Adds one fullscreen capture and an 8-byte-per-pixel texture when needed. Off by default; disabling it removes that capture and releases its full-size texture.",
            theme::Reset::Glass("r_glassBackgroundRecovery"), |ui| {
                if let Some(enabled) = theme::switch(ui, crate::glass::settings().background_recovery > 0.5) {
                    self.set_glass_menu_cvar("r_glassBackgroundRecovery", if enabled { "1" } else { "0" });
                }
            });
    }

    fn glass_slider(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        name: &'static str,
        range: std::ops::RangeInclusive<f32>,
        tip: &str,
    ) {
        let mut value = crate::glass::value(name);
        let mut changed = false;
        ocean_slider(
            ui,
            label,
            tip,
            theme::Reset::Glass(name),
            &mut value,
            range,
            &mut changed,
        );
        if changed {
            self.set_glass_menu_cvar(name, &value.to_string());
        }
    }

    fn glass_mode(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        name: &'static str,
        tip: &str,
        modes: &[(f32, &str)],
    ) {
        if let Some(value) = segmented_row(
            ui,
            label,
            tip,
            theme::Reset::Glass(name),
            crate::glass::value(name),
            modes,
        ) {
            self.set_glass_menu_cvar(name, &value.to_string());
        }
    }

    fn set_glass_menu_cvar(&mut self, name: &str, value: &str) {
        let result = if name == "r_glassPromotion" {
            self.set_console_cvar_from_console(name, value).map(|_| ())
        } else {
            self.set_console_cvar(name, value)
        };
        if let Err(error) = result {
            self.push_console_line(format!("^1{error}"));
        }
        self.egui_repaint_requested = true;
        self.publish_ui();
    }

    pub(in crate::app::egui_settings) fn reset_glass_control(&mut self, name: &'static str) {
        let defaults = Settings::preset(crate::glass::settings().style);
        let value = match name {
            "r_glassPromotion" => "0".to_owned(),
            "r_glassStyle" => "liquid".to_owned(),
            "r_glassTint" => format!(
                "{} {} {}",
                defaults.tint[0], defaults.tint[1], defaults.tint[2]
            ),
            "r_glassEdgeProfile" => defaults.edge_profile.name().to_owned(),
            _ => defaults.value(name).to_string(),
        };
        self.set_glass_menu_cvar(name, &value);
    }
}
