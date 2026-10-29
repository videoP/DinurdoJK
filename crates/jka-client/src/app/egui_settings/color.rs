//! HDR, color grading and output controls, separate from lens and spatial post effects.
use crate::app::egui_settings::{percent, theme, ui, App, ColorLutPreset};

impl App {
    pub(in crate::app::egui_settings) fn egui_color_settings(&mut self, ui: &mut egui::Ui) {
        theme::section(
            ui,
            "BRIGHTNESS",
            "Output brightness and per-lighting-class scaling.",
        );
        theme::row(
            ui,
            "Brightness method",
            "Shader corrects the finished game image. Baked textures avoids the brightness pass but changes material colors before lighting and uses extra RAM. Hardware changes the Windows SDR display ramp (including menus, HUD and desktop). Hardware (scene only) keeps the fast hardware ramp and counter-adjusts the UI; it adds an overlay composite pass only while an egui menu is visible. Both restore desktop brightness on focus loss and exit.",
            theme::Reset::Color("r_gammaMethod"),
            |ui| {
                egui::ComboBox::from_id_salt("gamma_method")
                    .selected_text(self.video.gamma_method.label())
                    .width(230.0)
                    .show_ui(ui, |ui| {
                        for method in crate::gamma::GammaMethod::ALL {
                            if ui.add_enabled(!self.native_hdr_blocks_hardware_gamma() || !method.is_hardware(), egui::Button::selectable(self.video.gamma_method == method, method.label())).clicked() {
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

        theme::section(
            ui,
            "HDR",
            "Scene precision, HDR baked lighting and highlight rolloff.",
        );
        // Match the Dynamic Lights row: one switch and its enabled quality
        // levels share a single row. The switch is strictly r_hdr 0/1;
        // selecting either meter segment sets r_hdr to 1 or 2.
        let changing_adapter = self.video.renderer_backend != self.applied_renderer_backend
            || self.video.gpu_adapter != self.applied_gpu_adapter;
        let known_for_adapter = self.hdr_modes_known && !changing_adapter;
        let available_modes = self.hdr_available_modes;
        let supports = |mode: u32| -> bool {
            (1..=3).contains(&mode)
                && known_for_adapter
                && (available_modes & (1 << mode)) != 0
        };
        let any_native_output = (1..=3).any(|mode| supports(mode));
        let hdr_mode = self.video.hdr_mode();
        let hdr_preview_label = theme::preset_preview_target_label(ui.ctx(), theme::Reset::Video(ui::VIDEO_ROW_HDR));
        let display_hdr_mode = match hdr_preview_label.as_deref() {
            Some("Off") => 0usize,
            Some("HDR Math Only") => 1usize,
            Some("Full HDR Output") => 2usize,
            _ => hdr_mode as usize,
        };
        let next_hdr_mode = theme::row(
            ui,
            "HDR",
            "Off disables HDR math. HDR Math Only keeps the normal SDR output path. \
             Full HDR Output requires a supported monitor, GPU and backend.",
            theme::Reset::Video(ui::VIDEO_ROW_HDR),
            |ui| {
                let width = theme::track_width(ui);
                theme::quality_meter_power_limited(
                    ui,
                    display_hdr_mode,
                    &["HDR Math Only", "Full HDR Output"],
                    width,
                    if any_native_output { 2 } else { 1 },
                )
            },
        );
        if let Some(mode) = next_hdr_mode {
            if mode == 2 && !supports(self.video.hdr_style + 1) {
                // If the preferred style is unavailable, use the first native
                // format accepted by this display and backend.
                if let Some(output) = (1..=3).find(|&output| supports(output)) {
                    let _ = self.cvar_set_hdr_style("r_hdrStyle", &(output - 1).to_string());
                }
            }
            let _ = self.cvar_set_r_hdr("r_hdr", &mode.to_string());
        }
        ui.add_enabled_ui(hdr_mode != 0, |ui| {
            theme::row(
                ui, "HDR output style",
                "Output encoding for Full HDR Output. Changing this preference in HDR Math Only does not change presentation.",
                theme::Reset::Color("r_hdrStyle"),
                |ui| {
                    let style = self.video.hdr_style.min(2);
                    ui.add_enabled_ui(any_native_output, |ui| {
                        egui::ComboBox::from_id_salt("hdr_style")
                            .selected_text(if any_native_output {
                                crate::renderer::hdr_output::OUTPUT_LABELS[(style + 1) as usize]
                            } else { "Unavailable" })
                            .width(230.0)
                            .show_ui(ui, |ui| {
                                for candidate in 0..=2_u32 {
                                    if ui.add_enabled(supports(candidate + 1), egui::Button::selectable(
                                        style == candidate,
                                        crate::renderer::hdr_output::OUTPUT_LABELS[(candidate + 1) as usize]
                                    )).clicked() {
                                        let _ = self.cvar_set_hdr_style("r_hdrStyle", &candidate.to_string());
                                    }
                                }
                            });
                    });
                },
            );
        });
        // Nits calibration is meaningful only with native HDR selected/active.
        // These controls do not affect HDR Math Only on an SDR display.
        let native_hdr = self.hdr_output_active
            || (self.video.hdr_mode() == 2 && supports(self.video.hdr_output));
        ui.add_enabled_ui(native_hdr, |ui| {
            theme::row(ui, "HDR paper white", "Brightness of ordinary whites and UI, in nits. Highlights have room above this level.", theme::Reset::Color("r_hdrPaperWhite"), |ui| {
                let mut white = self.video.hdr_paper_white;
                let readout = format!("{white:.0} nits");
                if theme::slider(ui, &mut white, 80.0..=500.0, &readout) {
                    let _ = self.cvar_set_hdr_paper_white("r_hdrPaperWhite", &white.to_string());
                }
            });
            theme::row(ui, "HDR peak brightness", "Display highlight ceiling, in nits. Zero uses reported display metadata, or 1000 nits when unavailable. Set your display's measured peak to calibrate manually.", theme::Reset::Color("r_hdrPeak"), |ui| {
                let mut peak = self.video.hdr_peak;
                let text = if peak == 0.0 { "Auto".to_owned() } else { format!("{peak:.0} nits") };
                if theme::slider(ui, &mut peak, 0.0..=10000.0, &text) {
                    let _ = self.cvar_set_hdr_peak("r_hdrPeak", &peak.to_string());
                }
            });
        });
        ui.add_enabled_ui(self.video.hdr_mode() != 0, |ui| {
            self.egui_toggle_row(
                ui,
                "Float lightmaps",
                "Rend2-compatible HDR baked lighting. While HDR rendering is enabled, \
                 load maps/<map>/lm_XXXX.hdr companions into FP16 when they exist; \
                 ordinary JKA lightmaps are promoted to FP16 as the fallback. While a map is loaded, Apply Video Settings reloads it with the new setting.",
                ui::VIDEO_ROW_FLOAT_LIGHTMAP,
                self.video.float_lightmap,
            );
            theme::row(
                ui,
                "Tone mapping",
                "Strength of the SDR ACES / native HDR highlight rolloff. 0% disables the effect; 100% keeps the original appearance.",
                theme::Reset::Video(ui::VIDEO_ROW_TONE_MAPPING),
                |ui| {
                    let mut value = self.video.tone_mapping;
                    let readout = percent(value);
                    if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                        self.set_tone_mapping_strength(value);
                    }
                },
            );
        });
        // Sky-referenced reads the Blue Water sky, so it only exists under that sky.
        // The filmic flag is kept while the JKA sky is shown (so switching back
        // restores it), which is why the mode is derived rather than stored.
        let sky_ok = self.video.sky_backend == 1;
        let exposure_mode: u8 = if sky_ok && self.video.blue_water_environment.filmic {
            2
        } else if self.video.auto_exposure > 0.0 {
            1
        } else {
            0
        };
        let mut picked: Option<u8> = None;
        theme::row(
            ui,
            "Auto exposure mode",
            "Off keeps exposure fixed. Balanced meters the whole image toward 18% grey (needs HDR and tone mapping) and lifts dark scenes toward daylight. Sky-referenced is the Blue Water eye: it meters against the sky's own brightness, closes on glare, stays dark at night, and brings AgX tone mapping and a night-readability lift with it. Sky-referenced needs the Blue Water sky.",
            theme::Reset::None,
            |ui| {
                for (mode, name) in [(0u8, "Off"), (1, "Balanced"), (2, "Sky-referenced")] {
                    let enabled = mode != 2 || sky_ok;
                    let clicked = ui
                        .add_enabled_ui(enabled, |ui| theme::chip(ui, name, exposure_mode == mode).clicked())
                        .inner;
                    if clicked && exposure_mode != mode {
                        picked = Some(mode);
                    }
                    ui.add_space(3.0);
                }
            },
        );
        if let Some(mode) = picked {
            self.set_auto_exposure_mode(mode);
        }
        ui.add_enabled_ui(
            (self.video.hdr_mode() != 0) && self.video.tone_mapping > 0.0 && exposure_mode != 2,
            |ui| {
                theme::row(
                    ui,
                    "Balanced strength",
                    "Amount of adaptive exposure correction. 0% disables metering; 100% keeps the original response speed and range.",
                    theme::Reset::Video(ui::VIDEO_ROW_AUTO_EXPOSURE),
                    |ui| {
                        let mut value = self.video.auto_exposure;
                        let readout = percent(value);
                        if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                            self.set_auto_exposure_strength(value);
                        }
                    },
                );
            },
        );

        theme::section(
            ui,
            "COLOR GRADING",
            "Primary image controls. Exposure is scene-referred; the other controls are baked into the existing 3-D LUT.",
        );
        let grade = self.video.color_grade;
        self.egui_color_grade_slider(
            ui,
            "Exposure",
            "Manual scene exposure in stops. Applied before tone mapping, so +1 EV doubles scene light and -1 EV halves it. Auto exposure is added on top when enabled.",
            "r_colorExposure",
            grade.exposure_ev,
            -4.0..=4.0,
            |v| format!("{v:+.2} EV"),
            false,
        );
        self.egui_color_grade_slider(
            ui,
            "Contrast",
            "Global contrast around middle grey. 100% is neutral.",
            "r_colorContrast",
            grade.contrast,
            0.0..=2.0,
            percent,
            true,
        );
        self.egui_color_grade_slider(
            ui,
            "Saturation",
            "Global color saturation. 100% is neutral; 0% is greyscale.",
            "r_colorSaturation",
            grade.saturation,
            0.0..=2.0,
            percent,
            true,
        );
        self.egui_color_grade_slider(
            ui,
            "Vibrance",
            "Selective saturation that favors muted colors and protects already-vivid colors such as sabers and neon.",
            "r_colorVibrance",
            grade.vibrance,
            -1.0..=1.0,
            |v| format!("{:+.0}%", v * 100.0),
            true,
        );
        self.egui_color_grade_slider(
            ui,
            "Temperature",
            "White-balance temperature. 6500 K is neutral; lower values warm the image and higher values cool it.",
            "r_colorTemperature",
            grade.temperature,
            2000.0..=11000.0,
            |v| format!("{v:.0} K"),
            true,
        );
        self.egui_color_grade_slider(
            ui,
            "Tint",
            "Green/magenta white-balance axis. Negative is greener; positive is more magenta.",
            "r_colorTint",
            grade.tint,
            -1.0..=1.0,
            |v| format!("{:+.0}%", v * 100.0),
            true,
        );
        self.egui_color_grade_slider(
            ui,
            "Shadows",
            "Lift or deepen dark tones on a monotonic tone curve. -100 to +100 is adjustment strength, not EV or literal percent brightness; 0 is neutral. Middle grey and the black/white endpoints stay fixed.",
            "r_colorShadows",
            grade.shadows,
            -100.0..=100.0,
            |v| format!("{v:+.0}"),
            true,
        );
        self.egui_color_grade_slider(
            ui,
            "Highlights",
            "Recover or boost bright tones on a monotonic tone curve. -100 to +100 is adjustment strength, not EV or literal percent brightness; 0 is neutral. Shadows cannot overtake highlights on a grey ramp.",
            "r_colorHighlights",
            grade.highlights,
            -100.0..=100.0,
            |v| format!("{v:+.0}"),
            true,
        );
    }

    pub(in crate::app::egui_settings) fn egui_post_color_looks(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "COLOR LOOK", "Choose a film look or an external color grade.");
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
    }

    fn egui_color_grade_slider(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        tip: &str,
        name: &'static str,
        committed: f32,
        range: std::ops::RangeInclusive<f32>,
        readout: fn(f32) -> String,
        deferred: bool,
    ) {
        theme::row(ui, label, tip, theme::Reset::Color(name), |ui| {
            let state_id = ui.id().with(name);
            let mut value = if deferred {
                ui.ctx()
                    .data(|data| data.get_temp::<f32>(state_id))
                    .unwrap_or(committed)
            } else {
                committed
            };
            let text = readout(value);
            let changed = theme::slider(ui, &mut value, range, &text);
            if !deferred {
                if changed {
                    let _ = self.set_console_cvar(name, &value.to_string());
                }
                return;
            }
            if changed {
                ui.ctx().data_mut(|data| data.insert_temp(state_id, value));
            }
            let (primary_down, primary_released) = ui.input(|input| {
                (
                    input.pointer.primary_down(),
                    input.pointer.primary_released(),
                )
            });
            let staged = ui.ctx().data(|data| data.get_temp::<f32>(state_id));
            let commit = if changed && !primary_down {
                Some(value)
            } else if primary_released {
                staged
            } else {
                None
            };
            if let Some(value) = commit {
                ui.ctx().data_mut(|data| {
                    data.remove::<f32>(state_id);
                });
                let _ = self.set_console_cvar(name, &value.to_string());
            }
        });
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
            let staged = ui.ctx().data(|data| data.get_temp::<[u8; 3]>(state_id));
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
                let (next_hue, next_saturation) = split_toning_hue_saturation(selected, hue);
                self.set_split_toning_color(hue_cvar, saturation_cvar, next_hue, next_saturation);
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
