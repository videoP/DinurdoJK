//! Color grading controls, separate from lens and spatial post effects.
use crate::app::egui_settings::{percent, theme, ui, App, ColorLutPreset};

impl App {
    pub(in crate::app::egui_settings) fn egui_color_settings(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "LOOK", "Choose a film look or an external color grade.");
        theme::row(
            ui,
            "Color LUT",
            "Colour grade applied as a 3D lookup table, the same way a film \
             print is graded. This sets the overall mood of the image.",
            theme::Reset::Video(ui::VIDEO_ROW_COLOR_LUT),
            |ui| {
                egui::ComboBox::from_id_salt("color_lut")
                    .selected_text(self.video.color_lut.label())
                    .width(230.0)
                    .show_ui(ui, |ui| {
                        for preset in ColorLutPreset::all() {
                            if ui
                                .selectable_label(preset == self.video.color_lut, preset.label())
                                .clicked()
                            {
                                self.set_color_lut(preset);
                            }
                        }
                    });
            },
        );
        theme::row(
            ui,
            "LUT strength",
            "How far the image is pushed toward the selected grade. 0% is the \
             ungraded frame.",
            theme::Reset::Video(ui::VIDEO_ROW_LUT_STRENGTH),
            |ui| {
                let mut value = self.video.color_lut_strength;
                let readout = percent(value);
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    self.set_color_lut_strength(value);
                }
            },
        );

        self.egui_split_toning(ui);
        theme::section(ui, "BRIGHTNESS", "Scene adaptation and output brightness.");
        ui.add_enabled_ui(self.video.hdr, |ui| {
            self.egui_toggle_row(
                ui,
                "Tone mapping",
                "Maps the HDR frame down to what the display can show, rolling off \
             highlights instead of clipping them. Needs HDR rendering to have \
             anything to roll off.",
                ui::VIDEO_ROW_TONE_MAPPING,
                self.video.tone_mapping,
            );
        });
        ui.add_enabled_ui(self.video.hdr && self.video.tone_mapping, |ui| {
            self.egui_toggle_row(
                ui,
                "Auto exposure",
                "Meters HDR scene luminance and smoothly adapts exposure before tone \
             mapping, similar to eye/camera adaptation. Effective only with HDR \
             rendering and tone mapping enabled.",
                ui::VIDEO_ROW_AUTO_EXPOSURE,
                self.video.auto_exposure,
            );
        });
        theme::row(
            ui,
            "Brightness method",
            "Shader corrects the finished game image. Baked textures avoids the brightness pass but changes material colors before lighting and uses extra RAM. Hardware corrects the monitor output without a game brightness pass; supports Windows SDR displays and restores desktop brightness on focus loss, exit and crashes.",
            theme::Reset::Color("r_gammaMethod"),
            |ui| {
                egui::ComboBox::from_id_salt("gamma_method")
                    .selected_text(self.video.gamma_method.label())
                    .width(230.0)
                    .show_ui(ui, |ui| {
                        for method in crate::gamma::GammaMethod::ALL {
                            if ui.selectable_label(self.video.gamma_method == method, method.label()).clicked() {
                                self.set_gamma_method(method);
                            }
                        }
                    });
            },
        );
        if let Some(message) = &self.gamma_runtime.message {
            ui.label(
                egui::RichText::new(message)
                    .small()
                    .color(ui.visuals().weak_text_color()),
            );
        }
        theme::row(
            ui,
            "Brightness / gamma",
            "Adjust brightness using the selected method. Texture baking and hardware updates finish in the background.",
            theme::Reset::Video(ui::VIDEO_ROW_BRIGHTNESS),
            |ui| {
                let mut value = self.video.gamma;
                let readout = format!("{value:.2}");
                if theme::slider(ui, &mut value, 0.5..=3.0, &readout) {
                    self.set_gamma(value);
                }
            },
        );

        // Both follow the master slider until unlocked; unlocked values sit on
        // the master's scale, so unlocking never changes the picture.
        theme::row(
            ui,
            "Model brightness",
            "Brightness of players, NPCs and props, like r_ambientScale. Locked, it follows the master slider and adds nothing of its own. Unlock it to set models brighter or darker than the rest of the scene.",
            theme::Reset::Video(ui::VIDEO_ROW_MODEL_BRIGHTNESS),
            |ui| {
                let locked = self.video.model_brightness_locked;
                let mut value = self.video.model_brightness;
                let readout = format!("{value:.2}");
                let changed = ui
                    .add_enabled_ui(!locked, |ui| theme::slider(ui, &mut value, 0.5..=3.0, &readout))
                    .inner;
                ui.add_space(8.0);
                if let Some(now_locked) = theme::lock_toggle(ui, locked) {
                    self.set_model_brightness_locked(now_locked);
                } else if changed {
                    self.set_model_brightness(value);
                }
            },
        );
        theme::row(
            ui,
            "Dynamic light brightness",
            "Brightness of runtime lights such as blaster bolts, sabers and explosions. Locked, it follows the master slider and adds nothing of its own. Unlock it to make dynamic lights stronger or weaker than the rest of the scene.",
            theme::Reset::Video(ui::VIDEO_ROW_DLIGHT_BRIGHTNESS),
            |ui| {
                let locked = self.video.dynamic_light_brightness_locked;
                let mut value = self.video.dynamic_light_brightness;
                let readout = format!("{value:.2}");
                let changed = ui
                    .add_enabled_ui(!locked, |ui| theme::slider(ui, &mut value, 0.5..=3.0, &readout))
                    .inner;
                ui.add_space(8.0);
                if let Some(now_locked) = theme::lock_toggle(ui, locked) {
                    self.set_dynamic_light_brightness_locked(now_locked);
                } else if changed {
                    self.set_dynamic_light_brightness(value);
                }
            },
        );
    }

    fn egui_split_toning(&mut self, ui: &mut egui::Ui) {
        theme::section(
            ui,
            "SPLIT TONING",
            "Give shadows and highlights different color tints.",
        );
        theme::row(
            ui,
            "Split toning",
            "Tint shadows and highlights by perceived scene brightness before output gamma. Works on its own or together with a film look.",
            theme::Reset::Color("r_splitToning"),
            |ui| {
                if let Some(enabled) = theme::switch(ui, self.video.split_toning.enabled) {
                    let _ = self.set_console_cvar("r_splitToning", if enabled { "1" } else { "0" });
                }
            },
        );
        ui.add_enabled_ui(self.video.split_toning.enabled, |ui| {
            let settings = self.video.split_toning;
            self.egui_toning_slider(
                ui,
                "Strength",
                "How strongly both tints affect the image.",
                "r_splitToningStrength",
                settings.strength,
                0.0..=1.0,
            );
            self.egui_toning_color(
                ui,
                "Shadow color",
                "Tint for dark areas. Blue and cyan make cool shadows.",
                "r_splitToningShadowHue",
                "r_splitToningShadowSaturation",
                settings.shadow_hue,
                settings.shadow_saturation,
                "split_toning_shadow_color",
            );
            self.egui_toning_color(
                ui,
                "Highlight color",
                "Tint for bright areas. Yellow and orange make warm highlights.",
                "r_splitToningHighlightHue",
                "r_splitToningHighlightSaturation",
                settings.highlight_hue,
                settings.highlight_saturation,
                "split_toning_highlight_color",
            );
            self.egui_toning_slider(
                ui,
                "Balance",
                "Negative favors highlights; positive favors shadows. Zero balances both.",
                "r_splitToningBalance",
                settings.balance,
                -1.0..=1.0,
            );
        });
    }

    fn egui_toning_slider(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        tip: &str,
        name: &'static str,
        mut value: f32,
        range: std::ops::RangeInclusive<f32>,
    ) {
        theme::row(ui, label, tip, theme::Reset::Color(name), |ui| {
            let readout = percent(value);
            if theme::slider(ui, &mut value, range, &readout) {
                let _ = self.set_console_cvar(name, &value.to_string());
            }
        });
    }

    fn egui_toning_color(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        tip: &str,
        hue_cvar: &'static str,
        saturation_cvar: &'static str,
        hue: f32,
        saturation: f32,
        picker_id: &'static str,
    ) {
        let reset = theme::Reset::ColorPair(hue_cvar, saturation_cvar);
        theme::row(ui, label, tip, reset, |ui| {
            // Keep picker interaction state in egui rather than applying the
            // expensive color-grading LUT rebuild for every drag sample.
            let state_id = ui.id().with(picker_id);
            let committed = split_toning_srgb(hue, saturation);
            let mut selected = ui
                .ctx()
                .data(|data| data.get_temp::<[u8; 3]>(state_id))
                .unwrap_or(committed);

            let response = ui.color_edit_button_srgb(&mut selected);
            if response.changed() {
                ui.ctx()
                    .data_mut(|data| data.insert_temp(state_id, selected));
            }

            // A click is a complete selection on release. A dragged picker is
            // staged while the pointer is down and committed once when the drag
            // ends. Keyboard edits have no held primary button, so commit them
            // immediately as a discrete selection.
            let (primary_down, primary_released) = ui.input(|input| {
                (
                    input.pointer.primary_down(),
                    input.pointer.primary_released(),
                )
            });
            let staged = ui
                .ctx()
                .data(|data| data.get_temp::<[u8; 3]>(state_id));
            let commit = if response.changed() && !primary_down {
                Some(selected)
            } else if primary_released {
                staged
            } else {
                None
            };

            if let Some(selected) = commit {
                ui.ctx().data_mut(|data| {
                    data.remove::<[u8; 3]>(state_id);
                });
                let (next_hue, next_saturation) =
                    split_toning_hue_saturation(selected, hue);
                self.set_split_toning_color(
                    hue_cvar,
                    saturation_cvar,
                    next_hue,
                    next_saturation,
                );
            }

            ui.add_space(10.0);
            let shown = staged.unwrap_or(committed);
            theme::glow_label(
                ui,
                &format!("{}  {}  {}", shown[0], shown[1], shown[2]),
                12.5,
                theme::TEXT_FAINT,
            );
        });
    }

    fn set_split_toning_color(
        &mut self,
        hue_cvar: &'static str,
        saturation_cvar: &'static str,
        hue: f32,
        saturation: f32,
    ) {
        match hue_cvar {
            "r_splitToningShadowHue" => {
                self.video.split_toning.shadow_hue = hue;
                self.video.split_toning.shadow_saturation = saturation;
            }
            "r_splitToningHighlightHue" => {
                self.video.split_toning.highlight_hue = hue;
                self.video.split_toning.highlight_saturation = saturation;
            }
            _ => return,
        }
        debug_assert!(matches!(
            saturation_cvar,
            "r_splitToningShadowSaturation" | "r_splitToningHighlightSaturation"
        ));
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }
}

fn split_toning_srgb(hue: f32, saturation: f32) -> [u8; 3] {
    let color = egui::Color32::from(egui::ecolor::Hsva::new(
        hue.rem_euclid(360.0) / 360.0,
        saturation.clamp(0.0, 1.0),
        1.0,
        1.0,
    ));
    [color.r(), color.g(), color.b()]
}

fn split_toning_hue_saturation(rgb: [u8; 3], fallback_hue: f32) -> (f32, f32) {
    let r = f32::from(rgb[0]) / 255.0;
    let g = f32::from(rgb[1]) / 255.0;
    let b = f32::from(rgb[2]) / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let saturation = if max > f32::EPSILON { delta / max } else { 0.0 };
    if delta <= 1.0 / 255.0 {
        return (fallback_hue.rem_euclid(360.0), saturation);
    }

    let sector = if max == r {
        ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    ((sector * 60.0).rem_euclid(360.0), saturation)
}
