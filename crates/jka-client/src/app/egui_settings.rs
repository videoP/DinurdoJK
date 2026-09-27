//! Setup -> Game/View and Video settings.
//!
//! The rail in [`super::egui_menu`] picks which [`VideoSection`] is shown for Video;
//! this module also owns the compact Game/View page. Ordered video options (filtering,
//! shadow technique, AO samples, …) use the shared quality meter so their cost
//! ordering is visible, while unordered choices stay as plain segmented chips.
//! Every label carries a tooltip explaining what the setting actually does.
//!
//! Every mutation still funnels through the same `change_video_setting` /
//! `change_environment_setting` helpers the console and config loader use, so
//! the renderer sync, dirty-marking and UI publish behave identically.

use super::egui_menu::VideoSection;
use super::egui_theme as theme;
use super::quality::QualityPreset;
use super::*;

impl App {
    pub(super) fn egui_mod_settings(&mut self, ui: &mut egui::Ui) {
        use crate::net::mod_support::{server_info, ServerMod};

        theme::page_title(ui, "MOD", "Settings for the connected server's mod.");
        let active = self
            .net
            .as_ref()
            .map(|net| ServerMod::detect(server_info(&net.session().decoder().configstrings)));

        theme::section(ui, "SERVER MOD", "Detected from the current server's game state.");
        theme::row(
            ui,
            "Active mod",
            "The server mod detected from its advertised/configstring state.",
            theme::Reset::None,
            |ui| {
                theme::glow_label(
                    ui,
                    active.map_or("Not connected", ServerMod::label),
                    12.5,
                    theme::TEXT_DIM,
                );
            },
        );

        match active {
            Some(ServerMod::Japro) => {}
            Some(_) => {
                theme::section(
                    ui,
                    "SETTINGS",
                    "No client-side settings are available for this server mod yet.",
                );
                return;
            }
            None => {
                theme::section(
                    ui,
                    "SETTINGS",
                    "Connect to a server to view settings for its active mod.",
                );
                return;
            }
        }

        theme::section(
            ui,
            "JAPRO MOVEMENT PREFERENCES",
            "Saved locally and sent when you join a JAPRO server. Server rules still apply.",
        );
        let mut bits = self.network.plugin_disable;
        for (bit, label) in [
            (15, "Disable katas"),
            (16, "Disable butterflies"),
            (17, "Disable backstabs and roll stabs"),
            (18, "Disable DFA attacks"),
            (19, "Only bunny hop"),
            (20, "Disable rolls"),
            (21, "Disable cartwheels"),
            (22, "Use Jawa run animation"),
        ] {
            let enabled = bits & (1 << bit) != 0;
            theme::row(
                ui,
                label,
                "JAPRO cp_pluginDisable preference. This is a client preference; server rules still take precedence.",
                theme::Reset::None,
                |ui| {
                    if let Some(enabled) = theme::switch(ui, enabled) {
                        if enabled {
                            bits |= 1 << bit;
                        } else {
                            bits &= !(1 << bit);
                        }
                    }
                },
            );
        }
        if bits != self.network.plugin_disable {
            if let Err(error) = self.set_console_cvar("cp_pluginDisable", &bits.to_string()) {
                self.push_console_line(error);
            }
        }
    }

    pub(super) fn egui_game_settings(&mut self, ui: &mut egui::Ui) {
        theme::page_title(
            ui,
            "GAME / VIEW",
            "OpenJK local-player model and third-person camera controls.",
        );

        egui::ScrollArea::vertical()
            .id_salt("jka_game_view_settings")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                theme::section(ui, "PLAYER", "Local Solo Game presentation.");
                theme::row(
                    ui,
                    "Player model",
                    "OpenJK model cvar in model[/skin] form. Solo Game defaults to kyle; omitted skin means default.",
                    theme::Reset::None,
                    |ui| {
                        if !self.player_model_editing {
                            self.player_model_input = self.solo_client_info.model_cvar();
                        }
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut self.player_model_input)
                                .desired_width(theme::track_width(ui))
                                .hint_text("kyle"),
                        );
                        if response.gained_focus() {
                            self.player_model_editing = true;
                        }
                        let enter_pressed =
                            response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                        if response.lost_focus() || enter_pressed {
                            let model = self.player_model_input.clone();
                            let _ = self.set_console_cvar("model", &model);
                            self.player_model_editing = false;
                            self.player_model_input = self.solo_client_info.model_cvar();
                            if enter_pressed {
                                ui.memory_mut(|memory| memory.surrender_focus(response.id));
                            }
                        }
                    },
                );

                theme::section(
                    ui,
                    "THIRD PERSON",
                    "Stock OpenJK MP cg_thirdPerson camera cvars.",
                );
                theme::row(
                    ui,
                    "Third person",
                    "Toggles cg_thirdPerson. With First-person saber / melee enabled, turning this off also keeps saber/melee in first person; special forced-camera states remain separate.",
                    theme::Reset::None,
                    |ui| {
                        if let Some(value) = theme::switch(ui, self.third_person.enabled) {
                            let _ = self.set_console_cvar(
                                "cg_thirdPerson",
                                if value { "1" } else { "0" },
                            );
                        }
                    },
                );
                theme::row(
                    ui,
                    "First-person saber / melee",
                    "cg_fpls. Allow cg_thirdPerson 0 to remain first person with saber/melee. DinurdoJK defaults this on and archives the choice.",
                    theme::Reset::None,
                    |ui| {
                        if let Some(value) = theme::switch(ui, self.first_person_lightsaber) {
                            let _ = self.set_console_cvar(
                                "cg_fpls",
                                if value { "1" } else { "0" },
                            );
                        }
                    },
                );

                theme::section(
                    ui,
                    "SMOOTH PRESENTATION (A/B)",
                    "Render-only decoupling from fixed pmove_msec. These switches never change movement simulation or network command timing.",
                );
                theme::row(
                    ui,
                    "Smooth player position",
                    "cg_smoothPlayerOrigin. Interpolate the local player model root between the previous and current fixed pmove samples.",
                    theme::Reset::None,
                    |ui| {
                        if let Some(value) = theme::switch(ui, self.presentation_smoothing.smooth_player_origin) {
                            let _ = self.set_console_cvar("cg_smoothPlayerOrigin", if value { "1" } else { "0" });
                        }
                    },
                );
                theme::row(
                    ui,
                    "Smooth third-person target",
                    "cg_smoothThirdPersonOrigin. Feed the third-person camera the same interpolated movement timeline instead of the latest stepped pmove origin.",
                    theme::Reset::None,
                    |ui| {
                        if let Some(value) = theme::switch(ui, self.presentation_smoothing.smooth_third_person_origin) {
                            let _ = self.set_console_cvar("cg_smoothThirdPersonOrigin", if value { "1" } else { "0" });
                        }
                    },
                );
                theme::row(
                    ui,
                    "Smooth player animation time",
                    "cg_smoothPlayerAnimation. Advance Ghoul2 animation and BG_G2PlayerAngles on continuous presentation time instead of stepped playerState commandTime.",
                    theme::Reset::None,
                    |ui| {
                        if let Some(value) = theme::switch(ui, self.presentation_smoothing.smooth_player_animation) {
                            let _ = self.set_console_cvar("cg_smoothPlayerAnimation", if value { "1" } else { "0" });
                        }
                    },
                );
                theme::row(
                    ui,
                    "Subframe local-player pose",
                    "cg_subframePlayerAngles. Render-only: when cl_input_subframe is enabled, use its newest pitch/yaw for the visible local player/Ghoul2 pose instead of waiting for the next pmove tick. Does not change mouse aim, usercmd angles, movement, weapon/saber gameplay traces, or networking.",
                    theme::Reset::None,
                    |ui| {
                        if let Some(value) = theme::switch(ui, self.presentation_smoothing.subframe_player_angles) {
                            let _ = self.set_console_cvar("cg_subframePlayerAngles", if value { "1" } else { "0" });
                        }
                    },
                );
                theme::row(
                    ui,
                    "Smooth third-person camera time",
                    "cg_smoothThirdPersonTime. Run camera damping from the continuous presentation clock instead of stepped commandTime.",
                    theme::Reset::None,
                    |ui| {
                        if let Some(value) = theme::switch(ui, self.presentation_smoothing.smooth_third_person_time) {
                            let _ = self.set_console_cvar("cg_smoothThirdPersonTime", if value { "1" } else { "0" });
                        }
                    },
                );

                macro_rules! third_person_number {
                    ($label:literal, $tip:literal, $field:ident, $cvar:literal, $speed:expr) => {
                        theme::row(ui, $label, $tip, theme::Reset::None, |ui| {
                            let mut value = self.third_person.$field;
                            if ui
                                .add(
                                    egui::DragValue::new(&mut value)
                                        .speed($speed)
                                        .max_decimals(3)
                                        .update_while_editing(false),
                                )
                                .changed()
                            {
                                let _ = self.set_console_cvar($cvar, &value.to_string());
                            }
                        });
                    };
                }

                third_person_number!(
                    "Range",
                    "cg_thirdPersonRange: distance behind the player.",
                    range,
                    "cg_thirdPersonRange",
                    1.0
                );
                third_person_number!(
                    "Orbit angle",
                    "cg_thirdPersonAngle: yaw offset around the player in degrees.",
                    angle,
                    "cg_thirdPersonAngle",
                    1.0
                );
                third_person_number!(
                    "Vertical offset",
                    "cg_thirdPersonVertOffset: target height above the player origin.",
                    vert_offset,
                    "cg_thirdPersonVertOffset",
                    0.5
                );
                third_person_number!(
                    "Horizontal offset",
                    "cg_thirdPersonHorzOffset: left/right camera offset.",
                    horz_offset,
                    "cg_thirdPersonHorzOffset",
                    0.5
                );
                third_person_number!(
                    "Pitch offset",
                    "cg_thirdPersonPitchOffset: camera pitch offset in degrees.",
                    pitch_offset,
                    "cg_thirdPersonPitchOffset",
                    0.5
                );
                third_person_number!(
                    "Camera damping",
                    "cg_thirdPersonCameraDamp: OpenJK camera-position smoothing factor.",
                    camera_damp,
                    "cg_thirdPersonCameraDamp",
                    0.01
                );
                third_person_number!(
                    "Target damping",
                    "cg_thirdPersonTargetDamp: OpenJK camera-target smoothing factor.",
                    target_damp,
                    "cg_thirdPersonTargetDamp",
                    0.01
                );
                third_person_number!(
                    "Player alpha",
                    "cg_thirdPersonAlpha stock cvar. Preserved for the OpenJK player-rendering path.",
                    alpha,
                    "cg_thirdPersonAlpha",
                    0.01
                );
                theme::row(
                    ui,
                    "Special camera",
                    "cg_thirdPersonSpecialCam. In TaystJK this switches to CG_ThirdPersonActionCam during saber special moves and falls back to the normal third-person camera if that action cam cannot be used. The Rust action-camera branch is not ported yet, so this switch is currently inert.",
                    theme::Reset::None,
                    |ui| {
                        if let Some(value) = theme::switch(ui, self.third_person.special_cam) {
                            let _ = self.set_console_cvar(
                                "cg_thirdPersonSpecialCam",
                                if value { "1" } else { "0" },
                            );
                        }
                    },
                );
            });
    }

    pub(super) fn egui_interface_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(
            ui,
            "INTERFACE / HUD",
            "HUD presentation. This is the home for crosshair, movement helpers and the future drag/snap HUD editor.",
        );

        egui::ScrollArea::vertical()
            .id_salt("jka_interface_settings")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                theme::section(ui, "HUD", "Heads-up display controls.");
                theme::row(
                    ui,
                    "HUD layout",
                    "Open HUD Edit Mode. Drag Health, Shield, Ammo and Force directly over the game; positions/scales are cached as typed runtime data and archived through cg_hud* cvars.",
                    theme::Reset::None,
                    |ui| {
                        let response = ui.add_enabled(!self.front_end, egui::Button::new("EDIT HUD"));
                        if response.clicked() {
                            self.hud_edit_selected = None;
                            self.hud_edit_drag_origin = None;
                            self.hud_edit_drag_delta = [0.0, 0.0];
                            self.set_overlay(OverlayMode::HudEdit);
                        }
                        if self.front_end {
                            response.on_hover_text("Enter a game before editing the HUD.");
                        } else {
                            response.on_hover_text("Also available as /hudedit.");
                        }
                    },
                );
                theme::section(
                    ui,
                    "CROSSHAIR",
                    "JKA/TaystJK-compatible local crosshair cvars with a native picker.",
                );

                const SHAPES: [(u8, &str); 7] = [
                    (0, "Off"),
                    (1, "Classic"),
                    (2, "Dot"),
                    (3, "Plus"),
                    (4, "+ Dot"),
                    (5, "Brackets"),
                    (6, "Box"),
                ];
                if let Some(style) = segmented_row(
                    ui,
                    "Shape",
                    "cg_drawCrosshair. Zero disables it; the other values select the local crosshair geometry.",
                    theme::Reset::None,
                    self.crosshair.style,
                    &SHAPES,
                ) {
                    let _ = self.set_console_cvar("cg_drawCrosshair", &style.to_string());
                }

                theme::row(
                    ui,
                    "Size",
                    "cg_crosshairSize. Uses JKA's stock default of 24; procedural geometry compensates for the transparent padding present in the original crosshair artwork.",
                    theme::Reset::None,
                    |ui| {
                        let mut value = self.crosshair.size;
                        let readout = format!("{value:.0}");
                        if theme::slider(ui, &mut value, 4.0..=96.0, &readout) {
                            let _ = self.set_console_cvar("cg_crosshairSize", &value.to_string());
                        }
                    },
                );

                theme::row(
                    ui,
                    "Color",
                    "cg_crosshairColor. RGB picker; the archived cvar keeps TaystJK's R G B A 0..255 format.",
                    theme::Reset::None,
                    |ui| {
                        let mut rgb = [
                            f32::from(self.crosshair.color[0]) / 255.0,
                            f32::from(self.crosshair.color[1]) / 255.0,
                            f32::from(self.crosshair.color[2]) / 255.0,
                        ];
                        if ui.color_edit_button_rgb(&mut rgb).changed() {
                            let to_u8 = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
                            let [_, _, _, alpha] = self.crosshair.color;
                            let color = [to_u8(rgb[0]), to_u8(rgb[1]), to_u8(rgb[2]), alpha];
                            let value = format!(
                                "{} {} {} {}",
                                color[0], color[1], color[2], color[3]
                            );
                            let _ = self.set_console_cvar("cg_crosshairColor", &value);
                        }
                        ui.add_space(10.0);
                        theme::glow_label(
                            ui,
                            &format!(
                                "{}  {}  {}",
                                self.crosshair.color[0],
                                self.crosshair.color[1],
                                self.crosshair.color[2]
                            ),
                            12.5,
                            theme::TEXT_FAINT,
                        );
                    },
                );
                theme::section(
                    ui,
                    "PREVIEW",
                    "Live center-screen sample of the current crosshair settings.",
                );
                draw_crosshair_preview(ui, self.crosshair);
                theme::glow_label(
                    ui,
                    "Preview scales to fit the menu box; in-game placement remains dead-center.",
                    11.5,
                    theme::TEXT_FAINT,
                );


                theme::section(
                    ui,
                    "MOVEMENT KEYS",
                    "TaystJK-compatible cg_movementKeys overlay and layout controls.",
                );
                const MOVEMENT_KEY_MODES: [(u8, &str); 5] = [
                    (0, "Off"), (1, "Original"), (2, "+ Attack"), (3, "Compact"), (4, "Movable"),
                ];
                if let Some(mode) = segmented_row(
                    ui, "Mode", "cg_movementKeys. Uses TaystJK's 0..4 display modes.",
                    theme::Reset::None, self.movement_keys_hud.mode, &MOVEMENT_KEY_MODES,
                ) {
                    let _ = self.set_console_cvar("cg_movementKeys", &mode.to_string());
                }
                theme::row(ui, "Horizontal", "cg_movementKeysX.", theme::Reset::None, |ui| {
                    let mut value = self.movement_keys_hud.x;
                    let readout = format!("{value:.0}");
                    if theme::slider(ui, &mut value, -320.0..=320.0, &readout) {
                        let _ = self.set_console_cvar("cg_movementKeysX", &value.to_string());
                    }
                });
                theme::row(ui, "Vertical", "cg_movementKeysY.", theme::Reset::None, |ui| {
                    let mut value = self.movement_keys_hud.y;
                    let readout = format!("{value:.0}");
                    if theme::slider(ui, &mut value, -240.0..=240.0, &readout) {
                        let _ = self.set_console_cvar("cg_movementKeysY", &value.to_string());
                    }
                });
                theme::row(ui, "Scale", "cg_movementKeysSize.", theme::Reset::None, |ui| {
                    let mut value = self.movement_keys_hud.size;
                    let readout = format!("{value:.2}x");
                    if theme::slider(ui, &mut value, 0.25..=4.0, &readout) {
                        let _ = self.set_console_cvar("cg_movementKeysSize", &value.to_string());
                    }
                });
                theme::row(ui, "Walk key", "cg_movementKeysWalk. Include the walk/run state in the overlay.", theme::Reset::None, |ui| {
                    if let Some(value) = theme::switch(ui, self.movement_keys_hud.walk) {
                        let _ = self.set_console_cvar("cg_movementKeysWalk", if value { "1" } else { "0" });
                    }
                });

                theme::section(
                    ui,
                    "STRAFEHELPER",
                    "TaystJK CGAZ/Strafehelper controls. Direction bits remain available through cg_strafeHelper.",
                );
                const STRAFE_STYLES: [(u8, &str); 4] = [
                    (0, "Off"), (1, "Original"), (2, "Updated"), (3, "CGAZ"),
                ];
                let style = if self.strafe_helper.flags & crate::ui::SHELPER_CGAZ != 0 { 3 }
                    else if self.strafe_helper.flags & crate::ui::SHELPER_UPDATED != 0 { 2 }
                    else if self.strafe_helper.flags & crate::ui::SHELPER_ORIGINAL != 0 { 1 }
                    else { 0 };
                if let Some(selected) = segmented_row(
                    ui, "Style", "Visual style bits inside cg_strafeHelper.",
                    theme::Reset::None, style, &STRAFE_STYLES,
                ) {
                    let style_bit = match selected {
                        1 => crate::ui::SHELPER_ORIGINAL,
                        2 => crate::ui::SHELPER_UPDATED,
                        3 => crate::ui::SHELPER_CGAZ,
                        _ => 0,
                    };
                    let flags = (self.strafe_helper.flags & !crate::ui::SHELPER_STYLE_MASK) | style_bit;
                    let _ = self.set_console_cvar("cg_strafeHelper", &flags.to_string());
                }
                theme::row(ui, "Directions", "Direction bits inside cg_strafeHelper. Defaults match TaystJK: WA, WD, A, D and Center.", theme::Reset::None, |ui| {
                    let mut flags = self.strafe_helper.flags;
                    for (bit, label) in [
                        (crate::ui::SHELPER_W, "W"),
                        (crate::ui::SHELPER_WA, "WA"),
                        (crate::ui::SHELPER_WD, "WD"),
                        (crate::ui::SHELPER_A, "A"),
                        (crate::ui::SHELPER_D, "D"),
                        (crate::ui::SHELPER_CENTER, "Center"),
                    ] {
                        let enabled = flags & bit != 0;
                        if theme::chip(ui, label, enabled).clicked() { flags ^= bit; }
                        ui.add_space(3.0);
                    }
                    if flags != self.strafe_helper.flags {
                        let _ = self.set_console_cvar("cg_strafeHelper", &flags.to_string());
                    }
                });
                theme::row(ui, "Offset", "cg_strafeHelperOffset. TaystJK stores hundredths of a degree; default 75 = 0.75 degrees.", theme::Reset::None, |ui| {
                    let mut value = self.strafe_helper.offset;
                    let readout = format!("{:.2}°", value * 0.01);
                    if theme::slider(ui, &mut value, -500.0..=500.0, &readout) {
                        let _ = self.set_console_cvar("cg_strafeHelperOffset", &value.to_string());
                    }
                });
                theme::row(ui, "Line width", "cg_strafeHelperLineWidth.", theme::Reset::None, |ui| {
                    let mut value = self.strafe_helper.line_width;
                    let readout = format!("{value:.2}");
                    if theme::slider(ui, &mut value, 0.25..=5.0, &readout) {
                        let _ = self.set_console_cvar("cg_strafeHelperLineWidth", &value.to_string());
                    }
                });
                theme::row(ui, "Physics FPS", "cg_strafeHelper_FPS. Zero follows com_maxfps like TaystJK; uncapped falls back to 125.", theme::Reset::None, |ui| {
                    let mut value = self.strafe_helper.fps;
                    let readout = if value < 1.0 { "Auto (125)".to_owned() } else { format!("{value:.0}") };
                    if theme::slider(ui, &mut value, 0.0..=1000.0, &readout) {
                        let _ = self.set_console_cvar("cg_strafeHelper_FPS", &value.to_string());
                    }
                });
                theme::row(ui, "Cutoff", "cg_strafeHelperCutoff. Controls the visible line length.", theme::Reset::None, |ui| {
                    let mut value = self.strafe_helper.cutoff;
                    let readout = format!("{value:.0}");
                    if theme::slider(ui, &mut value, 0.0..=480.0, &readout) {
                        let _ = self.set_console_cvar("cg_strafeHelperCutoff", &value.to_string());
                    }
                });
                theme::row(ui, "Inactive alpha", "cg_strafeHelperInactiveAlpha.", theme::Reset::None, |ui| {
                    let mut value = self.strafe_helper.inactive_alpha as f32;
                    let readout = format!("{value:.0}");
                    if theme::slider(ui, &mut value, 0.0..=255.0, &readout) {
                        let _ = self.set_console_cvar("cg_strafeHelperInactiveAlpha", &(value.round() as u8).to_string());
                    }
                });
                theme::row(ui, "Active color", "cg_strafeHelperActiveColor.", theme::Reset::None, |ui| {
                    let mut rgb = [
                        f32::from(self.strafe_helper.active_color[0]) / 255.0,
                        f32::from(self.strafe_helper.active_color[1]) / 255.0,
                        f32::from(self.strafe_helper.active_color[2]) / 255.0,
                    ];
                    if ui.color_edit_button_rgb(&mut rgb).changed() {
                        let to_u8 = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
                        let a = self.strafe_helper.active_color[3];
                        let value = format!("{} {} {} {}", to_u8(rgb[0]), to_u8(rgb[1]), to_u8(rgb[2]), a);
                        let _ = self.set_console_cvar("cg_strafeHelperActiveColor", &value);
                    }
                });
            });
    }

    pub(super) fn egui_video_page(&mut self, ui: &mut egui::Ui) {
        let (title, detail) = match self.video_section {
            VideoSection::Display => ("DISPLAY & FRAME PACING", "Window mode, timing and output."),
            VideoSection::ImageQuality => (
                "IMAGE QUALITY",
                "Edge sampling, texture reconstruction and framebuffer precision.",
            ),
            VideoSection::Visibility => (
                "VISIBILITY & GEOMETRY",
                "World visibility and GPU culling paths.",
            ),
            VideoSection::Models => (
                "MODELS",
                "Ghoul2 skinning, authored GLM detail and model submission.",
            ),
            VideoSection::Lighting => ("LIGHTING", "Ambient, dynamic and indirect lighting."),
            VideoSection::Shadows => ("SHADOWS", "Dynamic and local shadowing."),
            VideoSection::Reflections => (
                "REFLECTIONS",
                "Environment probes, screen-space reflections and dynamically budgeted planar views.",
            ),
            VideoSection::PostProcessing => (
                "POST PROCESSING",
                "Effects applied to the finished frame.",
            ),
            VideoSection::Film => ("FILM EMULATION", "Photochemical-inspired finishing."),
            VideoSection::DebugTools => (
                "DEBUG & TOOLS",
                "Renderer diagnostics and instrumentation.",
            ),
            VideoSection::BakedAo => ("BAKED AO", "Quality of the cached ambient occlusion bake."),
            VideoSection::Physics => (
                "PHYSICS",
                "Client-side visual simulation for ragdolls, dynamic props and debris.",
            ),
            VideoSection::Sun => (
                "SUN",
                "Runtime q3 shader-sun direction, intensity and chromaticity.",
            ),
            VideoSection::Clouds => ("CLOUDS", "Volumetric cloud deck and shaping controls."),
            VideoSection::Weather => (
                "WEATHER",
                "Shared world wind, fog, precipitation and atmospheric effects.",
            ),
            VideoSection::Surface => (
                "SURFACE INTERACTION",
                "Footprints and procedural ground detail.",
            ),
            VideoSection::Water => ("WATER", "FFT ocean simulation and promoted water surfaces."),
        };
        theme::page_title(ui, title, detail);

        egui::ScrollArea::vertical()
            .id_salt(("jka_video_section", title))
            .auto_shrink([false, false])
            .show(ui, |ui| match self.video_section {
                VideoSection::Display => self.egui_display_settings(ui),
                VideoSection::ImageQuality => self.egui_image_quality(ui),
                VideoSection::Visibility => self.egui_visibility(ui),
                VideoSection::Models => self.egui_models(ui),
                VideoSection::Lighting => self.egui_lighting(ui),
                VideoSection::Shadows => self.egui_shadows(ui),
                VideoSection::Reflections => self.egui_reflections(ui),
                VideoSection::PostProcessing => self.egui_post_processing(ui),
                VideoSection::Film => self.egui_film(ui),
                VideoSection::DebugTools => self.egui_debug_tools(ui),
                VideoSection::BakedAo => self.egui_baked_ao(ui),
                VideoSection::Physics => self.egui_physics(ui),
                VideoSection::Sun => self.egui_sun(ui),
                VideoSection::Clouds => self.egui_clouds(ui),
                VideoSection::Weather => self.egui_weather(ui),
                VideoSection::Surface => self.egui_surface(ui),
                VideoSection::Water => self.egui_water(ui),
            });

        // Drained after the page is built: a label click posts its tag while
        // the rows are being laid out, and applying it there would mutate the
        // settings mid-frame.
        if let Some(reset) = theme::take_reset(ui.ctx()) {
            self.apply_setting_reset(reset);
        }
    }

    // ------------------------------------------------------------ rendering --

    fn egui_display_settings(&mut self, ui: &mut egui::Ui) {
        const PRESETS: [(QualityPreset, &str); 4] = [
            (QualityPreset::Minimal, "Minimal"),
            (QualityPreset::Low, "Low"),
            (QualityPreset::Medium, "Medium"),
            (QualityPreset::High, "High"),
        ];
        if let Some(index) = optional_quality_table_row(
            ui,
            "Quality preset",
            "Sets every rendering cost lever at once. Resolution, display mode, \
             render backend, vsync, frame queue, FPS cap and the debug toggles are left \
             alone. The bar becomes Custom once you change a preset-controlled setting.",
            theme::Reset::Video(ui::VIDEO_ROW_QUALITY_PRESET),
            self.active_quality_preset(),
            &PRESETS,
        ) {
            self.video_selected = ui::VIDEO_ROW_QUALITY_PRESET;
            self.apply_quality_preset(PRESETS[index].0);
        }

        const BACKENDS: [(RendererBackend, &str); 2] = [
            (RendererBackend::Vulkan, "Vulkan"),
            (RendererBackend::Dx12, "DX12"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "Render backend",
            "Graphics API the renderer runs on. Driver quality differs per \
             vendor, so try the other one if something looks wrong. DX12 caps \
             presentation at three frames per refresh; Vulkan does not. Needs \
             Apply.",
            theme::Reset::Video(ui::VIDEO_ROW_RENDER_BACKEND),
            self.video.renderer_backend,
            &BACKENDS,
        ) {
            let current = usize::from(self.video.renderer_backend == RendererBackend::Dx12);
            let next = usize::from(target == RendererBackend::Dx12);
            self.video_selected = ui::VIDEO_ROW_RENDER_BACKEND;
            self.change_video_setting(next as i32 - current as i32);
        }

        const DISPLAY_MODES: [(FullscreenMode, &str); 3] = [
            (FullscreenMode::Windowed, "Windowed"),
            (FullscreenMode::Borderless, "Borderless"),
            (FullscreenMode::Exclusive, "Exclusive"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "Display mode",
            "Windowed keeps the desktop usable. Borderless keeps the desktop video mode. \
             Vulkan Exclusive switches the monitor video mode. Direct3D 12 has no classic \
             fullscreen-exclusive mode; its Exclusive option uses the HWND flip-model/FSO \
             path that Windows can promote to DirectFlip / Independent Flip. Needs Apply.",
            theme::Reset::Video(ui::VIDEO_ROW_FULLSCREEN),
            self.video.fullscreen,
            &DISPLAY_MODES,
        ) {
            let current = index_of(&DISPLAY_MODES, self.video.fullscreen, 0);
            let next = index_of(&DISPLAY_MODES, target, current);
            self.video_selected = ui::VIDEO_ROW_FULLSCREEN;
            self.change_video_setting(next as i32 - current as i32);
        }

        let resolutions = Self::available_resolutions(self.window.as_deref(), self.video.resolution);
        theme::row(
            ui,
            "Resolution",
            "Backbuffer size the world is rendered at. Lower resolutions cost \
             less on the GPU at the price of sharpness. Needs Apply.",
            theme::Reset::Video(ui::VIDEO_ROW_RESOLUTION),
            |ui| {
                egui::ComboBox::from_id_salt("video_resolution")
                    .selected_text(format!(
                        "{} × {}",
                        self.video.resolution[0], self.video.resolution[1]
                    ))
                    .width(190.0)
                    .show_ui(ui, |ui| {
                        for resolution in resolutions {
                            if ui
                                .selectable_label(
                                    self.video.resolution == resolution,
                                    format!("{} × {}", resolution[0], resolution[1]),
                                )
                                .clicked()
                            {
                                self.video.resolution = resolution;
                                self.mark_config_dirty();
                                self.console_status = format!(
                                    "RESOLUTION: {}X{} (PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART)",
                                    resolution[0], resolution[1]
                                );
                            }
                        }
                    });
                if self.video.resolution != self.applied_resolution {
                    theme::hint(ui, "needs apply", theme::WARNING);
                }
            },
        );

        const VSYNC: [(VsyncMode, &str); 4] = [
            (VsyncMode::Off, "Off"),
            (VsyncMode::On, "On"),
            (VsyncMode::Fast, "Fast"),
            (VsyncMode::Adaptive, "Adaptive"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "VSync",
            "Off tears but has the lowest latency. On locks to the refresh rate. \
             Fast presents the newest frame without blocking. Adaptive drops \
             sync only when the frame rate falls below the refresh rate.",
            theme::Reset::Video(ui::VIDEO_ROW_VSYNC),
            self.video.vsync,
            &VSYNC,
        ) {
            let current = index_of(&VSYNC, self.video.vsync, 0);
            let next = index_of(&VSYNC, target, current);
            self.video_selected = ui::VIDEO_ROW_VSYNC;
            self.change_video_setting(next as i32 - current as i32);
        }

        const FRAME_LATENCY: [(u32, &str); 3] = [
            (1, "1 Lowest"),
            (2, "2 Balanced"),
            (3, "3 Throughput"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "Frame queue",
            "Maximum WGPU presentation frames in flight. 1 favors the lowest \
             presentation/input latency, 2 balances latency and throughput, and 3 \
             favors maximum throughput. Changes apply live through a surface-only \
             reconfigure: no shader or render-pipeline rebuild. Useful for A/B testing.",
            theme::Reset::Video(ui::VIDEO_ROW_MAX_FRAME_LATENCY),
            self.video.max_frame_latency,
            &FRAME_LATENCY,
        ) {
            let current = index_of(&FRAME_LATENCY, self.video.max_frame_latency, 2);
            let next = index_of(&FRAME_LATENCY, target, current);
            self.video_selected = ui::VIDEO_ROW_MAX_FRAME_LATENCY;
            self.change_video_setting(next as i32 - current as i32);
        }

        let fps_cap_tip = match self.present_fps_ceiling() {
            Some(ceiling) => format!(
                "Upper bound on rendered frames per second. DX12 with FAST vsync \
                 presents without the tearing flag, so frames retire at vblank and \
                 stop at {ceiling} fps here. Higher values snap back to it. Switch \
                 VSync to Off for an uncapped, tearing present path."
            ),
            None => "Upper bound on rendered frames per second. 0 is unlimited. A cap a \
                     little under the refresh rate keeps input latency steady."
                .to_owned(),
        };
        theme::row(
            ui,
            "Frame rate cap",
            &fps_cap_tip,
            theme::Reset::Video(ui::VIDEO_ROW_FPS_CAP),
            |ui| {
                let enforced = self.present_fps_ceiling();
                let mut cap = self.effective_fps_cap();
                let changed = ui
                    .add(
                        egui::DragValue::new(&mut cap)
                            .range(0..=config::FPS_CAP_MAX)
                            .speed(10.0)
                            .suffix(" fps")
                            // Typing "142" would otherwise apply 1, then 14, and
                            // a one-frame-per-second cap locks the window up long
                            // before the rest of the number arrives.
                            .update_while_editing(false),
                    )
                    .changed();
                ui.add_space(10.0);
                let state = match enforced {
                    Some(ceiling) if ceiling == cap => "Present limit",
                    _ if cap >= config::FPS_CAP_MAX => "Maximum",
                    _ => "Capped",
                };
                theme::glow_label(ui, state, 12.5, theme::TEXT_FAINT);
                if changed {
                    self.set_fps_cap(cap);
                }
            },
        );

        theme::row(
            ui,
            "Player movement rate",
            "Fixed OpenJK/JKA player movement tick. Stock Jedi Academy runs 125 Hz; \
             changing it changes movement feel, including strafe jump behaviour. \
             This is separate from client-side Rapier visual physics.",
            theme::Reset::Video(ui::VIDEO_ROW_PHYSICS_FPS),
            |ui| {
                let mut fps = config::physics_fps_from_msec(self.video.physics_msec);
                if ui
                    .add(
                        egui::DragValue::new(&mut fps)
                            .range(20..=1000)
                            .speed(1.0)
                            .suffix(" Hz")
                            .update_while_editing(false),
                    )
                    .changed()
                {
                    let msec =
                        config::physics_msec_from_fps(&fps.to_string(), self.video.physics_msec);
                    self.set_physics_msec(msec);
                }
            },
        );

        theme::row(
            ui,
            "Brightness / gamma",
            "Output gamma curve. Raise it if dark corners of a map are \
             unreadable; it does not affect how the scene is lit.",
            theme::Reset::Video(ui::VIDEO_ROW_BRIGHTNESS),
            |ui| {
                let mut value = self.video.gamma;
                let readout = format!("{value:.2}");
                if theme::slider(ui, &mut value, 0.5..=3.0, &readout) {
                    self.set_gamma(value);
                }
            },
        );
    }

    fn egui_image_quality(&mut self, ui: &mut egui::Ui) {
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

        if self.video.detail_texture_fade {
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
        self.egui_toggle_row(
            ui,
            "Float lightmaps",
            "Rend2-compatible HDR baked lighting. While HDR rendering is enabled, \
             load maps/<map>/lm_XXXX.hdr companions into FP16 when they exist; \
             ordinary JKA lightmaps are promoted to FP16 as the fallback. Requires Apply Video Settings / vid_restart while a map is loaded.",
            ui::VIDEO_ROW_FLOAT_LIGHTMAP,
            self.video.float_lightmap,
        );
    }

    fn egui_visibility(&mut self, ui: &mut egui::Ui) {
        const PVS: [(PvsMode, &str); 7] = [
            (PvsMode::Off, "Off"),
            (PvsMode::Minimal, "Minimal"),
            (PvsMode::Full, "Full"),
            (PvsMode::Auto, "Auto"),
            (PvsMode::Auto2, "Auto 2"),
            (PvsMode::Auto3, "Auto 3"),
            (PvsMode::Auto4, "Auto 4"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "PVS portal culling",
            "Uses the map's precomputed visibility set to skip rooms the camera \
             cannot see. Auto keeps the existing whole-cluster coarse/full choice; \
             Auto 2 chooses coarse or full independently per material surface group. Auto 3 uses\
             coarse only when it is exactly equivalent to the currently visible Full children. Auto 4\
             builds portal/cluster-owned base batches at map load, precomputes the exact visible batch\
             recipe for every camera cluster, merges compatible batches there, and reuses identical\
             merged batches and whole recipes across clusters.",
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
        self.egui_toggle_row(
            ui,
            "Hi-Z occlusion",
            "Tests batches against a hierarchical depth pyramid so geometry \
             hidden behind walls is never submitted. Needs GPU-driven draws.",
            ui::VIDEO_ROW_HIZ,
            self.video.hiz_occlusion,
        );
    }

    fn egui_lighting(&mut self, ui: &mut egui::Ui) {
        const AMBIENT_OPTIONS: [(EntityAmbientLightingMode, &str, &str); 3] = [
            (
                EntityAmbientLightingMode::Off,
                "Off",
                "Dynamic objects receive a uniform, constant ambient color with no environmental lighting.",
            ),
            (
                EntityAmbientLightingMode::BspLightgridClassic,
                "BSP lightgrid",
                "OpenJK-style entity lighting from the map's BSP lightgrid: trilinear ambient + directed light and direction, sampled at the entity origin.",
            ),
            (
                EntityAmbientLightingMode::BevyIrradianceVolume,
                "Irradiance volume",
                "Bevy irradiance-volume runtime over the BSP lightgrid: bounded LightProbe volume, Valve ambient cubes, hardware trilinear interpolation, N² weighting and diffuse-indirect PBR integration.",
            ),
        ];
        if let Some(target) = mode_row(
            ui,
            "Entity ambient lighting",
            "How players, NPCs and moveable objects pick up the room's ambient \
             light as they move through the map.",
            theme::Reset::Video(ui::VIDEO_ROW_ENTITY_AMBIENT_LIGHTING),
            self.video.entity_ambient_lighting,
            &AMBIENT_OPTIONS,
        ) {
            let current = mode_index(&AMBIENT_OPTIONS, self.video.entity_ambient_lighting);
            self.video_selected = ui::VIDEO_ROW_ENTITY_AMBIENT_LIGHTING;
            self.change_video_setting(target as i32 - current as i32);
        }

        self.egui_toggle_row(
            ui,
            "Physically based rendering (PBR)",
            "Master switch for the complete Rend2-style PBR material profile. \
             Disabling it turns off PBR companion-map shading, parallax occlusion, \
             PBR lightgrid/probe response, dynamic-light PBR BRDF shading and \
             emissive companion shading together.",
            ui::VIDEO_ROW_PBR,
            self.video.pbr,
        );
        self.egui_toggle_row(
            ui,
            "Allow asset overrides",
            "JKA/OpenJK VFS compatibility policy. When enabled, higher-priority addon PK3s \
             and loose files may replace assets from retail assets0.pk3 through assets3.pk3. \
             When disabled, ordinary lookups protect those retail qpaths. Active PBR .mtr \
             materials may still load their explicitly referenced base/normal/RMO/etc. \
             textures from the same package as the material. Applies next map load / vid_restart.",
            ui::VIDEO_ROW_ASSET_OVERRIDES,
            self.video.allow_asset_overrides,
        );
        self.egui_toggle_row(
            ui,
            "Generate normal maps",
            "Rend2 compatibility fallback. On the next renderer restart or map load, \
             synthesize a normal map from diffuse luminance only when no authored \
             normal map exists. Useful for old assets, but authored normals are better.",
            ui::VIDEO_ROW_GEN_NORMAL_MAPS,
            self.video.gen_normal_maps,
        );
        self.egui_toggle_row(
            ui,
            "Deluxe mapping",
            "Use q3map2 directional lightmaps for normal/specular response when \
             present. Maps without deluxemaps retain the existing BSP-lightgrid \
             directional fallback.",
            ui::VIDEO_ROW_DELUXE_MAPPING,
            self.video.deluxe_mapping,
        );
        theme::row(
            ui,
            "Deluxe specular",
            "Scales only the specular lobe produced by directional baked lighting. \
             0 disables baked-direction specular while keeping diffuse deluxe relighting.",
            theme::Reset::Video(ui::VIDEO_ROW_DELUXE_SPECULAR),
            |ui| {
                let mut value = self.video.deluxe_specular;
                let readout = format!("{value:.2}×");
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    self.video.deluxe_specular = value;
                    self.sync_pbr();
                    self.mark_config_dirty();
                }
            },
        );

        // Classic world lighting is one three-state control. `Off` maps to
        // r_fullbright 1; once enabled, the meter chooses between the cheap
        // r_vertexLight path and normal authored BSP lightmaps.
        let world_lighting = if !self.video.world_lighting {
            0
        } else if self.video.vertex_lighting {
            1
        } else {
            2
        };
        const WORLD_LIGHTING: [&str; 3] = ["Off", "Vertex light", "BSP lightmaps"];
        if let Some(target) = quality_row(
            ui,
            "World lighting",
            "Master for classic BSP world lighting. Off is vanilla r_fullbright 1. \
             When enabled, Vertex light uses BSP vertex colors (r_vertexLight 1); \
             BSP lightmaps uses the normal authored baked-lightmap path (r_vertexLight 0).",
            theme::Reset::Video(ui::VIDEO_ROW_WORLD_LIGHTING),
            world_lighting,
            &WORLD_LIGHTING,
        ) {
            self.video_selected = ui::VIDEO_ROW_WORLD_LIGHTING;
            self.set_world_lighting_quality(target);
        }

        const LIGHT_OPTIONS: [(DynamicLightsMode, &str, &str); 6] = [
            (
                DynamicLightsMode::Off,
                "Off",
                "Runtime-authored dynamic lights are disabled.",
            ),
            (
                DynamicLightsMode::Legacy,
                "Legacy",
                "Low-cost JKA-style authored dynamic lights with a smooth radial falloff and no shadow pass.",
            ),
            (
                DynamicLightsMode::Vertex,
                "Vertex",
                "Cheapest authored dynamic-light path: evaluate runtime lights at BSP vertices and interpolate them. This is separate from static r_vertexLight.",
            ),
            (
                DynamicLightsMode::ClusteredLite,
                "Clustered lite",
                "Transient authored lights only through GPU cluster lists, using the cheap Lambert model and no local-light shadows.",
            ),
            (
                DynamicLightsMode::PerPixelForwardPlus,
                "Forward+",
                "Full clustered per-pixel local lighting, including the modern map-light path.",
            ),
            (
                DynamicLightsMode::RayTracedHardware,
                "Ray traced",
                "Hardware ray-traced local lighting. WIP — currently behaves as Off.",
            ),
        ];
        if let Some(target) = mode_row(
            ui,
            "Dynamic lights",
            "Technique used for light emitted by moving things: blaster bolts, \
             sabers, explosions and flickering map lights.",
            theme::Reset::Video(ui::VIDEO_ROW_DYNAMIC_LIGHTS),
            self.video.dynamic_lights,
            &LIGHT_OPTIONS,
        ) {
            let current = mode_index(&LIGHT_OPTIONS, self.video.dynamic_lights);
            self.video_selected = ui::VIDEO_ROW_DYNAMIC_LIGHTS;
            self.change_video_setting(target as i32 - current as i32);
        }

        self.egui_toggle_row(
            ui,
            ".map light simulation",
            "Source .map preview only. Approximates a compiled light stage from authored light/lightJunior entities, including color, strength and q3map-style falloff. It uses the modern clustered renderer and leaves compiled BSP lighting unchanged.",
            ui::VIDEO_ROW_MAP_LIGHT_SIMULATION,
            self.video.map_light_simulation,
        );

        theme::row(
            ui,
            "FX FPS",
            "Fixed sampling rate for continuous projectile/trail EFX. This removes the stock JKA \
             render-FPS dependency without changing authored EFX count/life/delay values. Drag to \
             the far-right Legacy JKA endpoint to restore one PlayEffect call per presentation frame. \
             0 in cg_fxFPS also selects Legacy JKA.",
            theme::Reset::Video(ui::VIDEO_ROW_FX_FPS),
            |ui| {
                const LEGACY_SLIDER_VALUE: f32 = crate::fx::FX_FPS_MAX as f32 + 1.0;
                let mut slider_value = if self.video.fx_fps == crate::fx::FX_FPS_LEGACY_JKA {
                    LEGACY_SLIDER_VALUE
                } else {
                    self.video.fx_fps.clamp(crate::fx::FX_FPS_MIN, crate::fx::FX_FPS_MAX) as f32
                };
                let readout = if self.video.fx_fps == crate::fx::FX_FPS_LEGACY_JKA {
                    "Legacy JKA".to_owned()
                } else {
                    format!("{} Hz", self.video.fx_fps)
                };
                if theme::slider(
                    ui,
                    &mut slider_value,
                    crate::fx::FX_FPS_MIN as f32..=LEGACY_SLIDER_VALUE,
                    &readout,
                ) {
                    let value = if slider_value >= LEGACY_SLIDER_VALUE {
                        crate::fx::FX_FPS_LEGACY_JKA
                    } else {
                        (slider_value.round() as u32)
                            .clamp(crate::fx::FX_FPS_MIN, crate::fx::FX_FPS_MAX)
                    };
                    let _ = self.set_console_cvar("cg_fxFPS", &value.to_string());
                }
            },
        );

        self.egui_toggle_row(
            ui,
            "Modern saber rendering",
            "Uses a continuous view-facing glow ribbon with the stock saber shaders instead of \
             OpenJK's chain of glow sprites. The authored line/glow textures and exact additive \
             blend rules are preserved. Off is the OpenJK-compatible presentation.",
            ui::VIDEO_ROW_MODERN_SABERS,
            self.video.modern_sabers,
        );

        const SABER_MARK_OPTIONS: [(ui::SaberMarkMode, &str); 3] = [
            (ui::SaberMarkMode::Off, "Off"),
            (ui::SaberMarkMode::Legacy, "Legacy"),
            (ui::SaberMarkMode::Enhanced, "Enhanced"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "Saber marks",
            "Saber/world contact presentation. Legacy follows OpenJK: contact is evaluated once per \
             presentation frame, the stock burn/glow mark lasts 10 seconds, and saberhitwall audio \
             uses the original 100 ms debounce. Enhanced is a separate smooth molten-surface effect: \
             movement lays a spatially sampled curved melt path, while holding the blade in one place \
             accumulates heat, widens/raises the molten lips, makes the material sag under gravity, \
             and grows smooth sludge/drips before cooling to a dark scar. cg_fxFPS does not control \
             saber/world marks. This is presentation-only.",
            theme::Reset::Video(ui::VIDEO_ROW_SABER_MARKS),
            self.video.saber_marks,
            &SABER_MARK_OPTIONS,
        ) {
            let current = index_of(&SABER_MARK_OPTIONS, self.video.saber_marks, 1);
            let next = index_of(&SABER_MARK_OPTIONS, target, current);
            self.video_selected = ui::VIDEO_ROW_SABER_MARKS;
            self.change_video_setting(next as i32 - current as i32);
        }

        self.egui_toggle_row(
            ui,
            "Emissive / area lights",
            "Lets glowing surfaces act as light sources with a shape, instead of \
             only looking bright themselves.",
            ui::VIDEO_ROW_EMISSIVE_AREA_LIGHTS,
            self.video.emissive_area_lights,
        );

        let ao_mode = if self.video.ssao {
            1usize
        } else if self.video.static_bsp_ao {
            2
        } else {
            0
        };
        const AO_OPTIONS: [(usize, &str); 3] = [(0, "Off"), (1, "Realtime"), (2, "Baked")];
        if let Some(target) = segmented_row(
            ui,
            "Ambient occlusion",
            "Contact darkening where surfaces meet. Realtime computes it every \
             frame from depth; Baked precomputes it per map and costs nothing \
             at runtime but misses moving objects.",
            theme::Reset::Video(ui::VIDEO_ROW_AMBIENT_OCCLUSION),
            ao_mode,
            &AO_OPTIONS,
        ) {
            self.video_selected = ui::VIDEO_ROW_AMBIENT_OCCLUSION;
            self.change_video_setting(target as i32 - ao_mode as i32);
        }
        self.egui_toggle_row(
            ui,
            "Voxel / probe GI",
            "Bounced indirect light gathered into a voxel or probe volume, so \
             lit surfaces spill colour onto their surroundings. Enabling it on a \
             map prepared without GI requires Apply Video Settings / vid_restart.",
            ui::VIDEO_ROW_VOXEL_PROBE_GI,
            self.video.voxel_probe_gi,
        );
    }

    fn egui_shadows(&mut self, ui: &mut egui::Ui) {
        const SHADOW_OPTIONS: [(DynamicShadowsMode, &str, &str); 5] = [
            (
                DynamicShadowsMode::Off,
                "Off",
                "No dynamic shadows cast by players, NPCs or moveable objects.",
            ),
            (
                DynamicShadowsMode::BlobStencilLegacy,
                "Blob / stencil",
                "Dark circular decals or stencil shadow volumes beneath entities. WIP — currently behaves as Off.",
            ),
            (
                DynamicShadowsMode::CascadedShadowMaps,
                "Cascaded maps (CSM)",
                "Multi-resolution depth textures along view-frustum slices for crisp rasterized shadows.",
            ),
            (
                DynamicShadowsMode::CascadedShadowMapsBevy,
                "Cascaded maps (Bevy)",
                "Bevy 0.19.1 directional-light CSM: four exponential cascades, stable texel snapping, reverse-Z and Bevy shadow filtering.",
            ),
            (
                DynamicShadowsMode::RayTraced,
                "Ray traced",
                "Hardware ray-query sun visibility using the current wgpu BLAS/TLAS API. This first port traces opaque static BSP casters; alpha-tested/translucent surfaces and moving/skinned casters are deliberately excluded rather than approximated.",
            ),
        ];
        if let Some(target) = mode_row(
            ui,
            "Dynamic shadows",
            "Real-time shadow technique layered over the map's authored/baked lighting. Hardware ray tracing currently covers opaque static BSP sun occlusion.",
            theme::Reset::Video(ui::VIDEO_ROW_DYNAMIC_SHADOWS),
            self.video.dynamic_shadows,
            &SHADOW_OPTIONS,
        ) {
            let current = mode_index(&SHADOW_OPTIONS, self.video.dynamic_shadows);
            self.video_selected = ui::VIDEO_ROW_DYNAMIC_SHADOWS;
            self.change_video_setting(target as i32 - current as i32);
        }
        self.egui_toggle_row(
            ui,
            "Local light shadows",
            "Adds shadows to local lights. With RT Shadows, uses hard ray-traced \
             shadows for clustered lights, including moving FX lights. Other \
             shadow modes use cached shadow cubemaps. Requires local lighting.",
            ui::VIDEO_ROW_LOCAL_LIGHT_SHADOWS,
            self.video.local_light_shadows,
        );
        self.egui_toggle_row(
            ui,
            "Contact shadows",
            "A short screen-space depth trace that fills in the fine contact \
             darkening shadow maps are too coarse to resolve.",
            ui::VIDEO_ROW_CONTACT_SHADOWS,
            self.video.contact_shadows,
        );
    }

    fn egui_reflections(&mut self, ui: &mut egui::Ui) {
        const REFLECTION_QUALITY: [(ReflectionQuality, &str); 6] = [
            (ReflectionQuality::Off, "Off"),
            (ReflectionQuality::Legacy, "Legacy"),
            (ReflectionQuality::Low, "Low"),
            (ReflectionQuality::Medium, "Medium"),
            (ReflectionQuality::High, "High"),
            (ReflectionQuality::Ultra, "Ultra"),
        ];
        if let Some(target) = quality_table_row(
            ui,
            "Reflection quality",
            "Master reflection policy. Off removes authored tcGen environment stages entirely; \
             Legacy preserves those vanilla environment-mapped stages but enables no enhanced \
             reflection technique. Low uses reflection probes only; Medium adds temporal \
             screen-space reflections; High adds one dynamically selected planar reflector; \
             Ultra raises SSR quality and allows up to four planar reflectors. Reflection \
             quality is applied on vid_restart because Off specializes the prepared material \
             set and planar modes preserve reflection-plane BSP topology. Expensive techniques \
             fall back to cheaper ones automatically.",
            theme::Reset::Video(ui::VIDEO_ROW_SSR),
            self.video.reflection_quality,
            &REFLECTION_QUALITY,
            4,
        ) {
            let current = index_of(&REFLECTION_QUALITY, self.video.reflection_quality, 4);
            self.video_selected = ui::VIDEO_ROW_SSR;
            self.change_video_setting(target as i32 - current as i32);
        }

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

    fn egui_post_processing(&mut self, ui: &mut egui::Ui) {
        self.egui_toggle_row(
            ui,
            "Tone mapping",
            "Maps the HDR frame down to what the display can show, rolling off \
             highlights instead of clipping them. Needs HDR rendering to have \
             anything to roll off.",
            ui::VIDEO_ROW_TONE_MAPPING,
            self.video.tone_mapping,
        );
        self.egui_toggle_row(
            ui,
            "Auto exposure",
            "Meters HDR scene luminance and smoothly adapts exposure before tone \
             mapping, similar to eye/camera adaptation. Effective only with HDR \
             rendering and tone mapping enabled.",
            ui::VIDEO_ROW_AUTO_EXPOSURE,
            self.video.auto_exposure,
        );
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
    }

    fn egui_film(&mut self, ui: &mut egui::Ui) {
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
                        for preset in ColorLutPreset::ALL {
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

        theme::section(ui, "LENS & STOCK", "");
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
            "Purple fringing",
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

    fn egui_models(&mut self, ui: &mut egui::Ui) {
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
            "Model LOD bias",
            "JKA r_lodbias for authored model/Ghoul2 LODs. 0 uses projected screen size normally; higher values bias toward cheaper GLM LODs. OpenJK defaults to 0.",
            theme::Reset::Video(ui::VIDEO_ROW_GHOUL2_LOD_BIAS),
            |ui| {
                let mut bias = self.video.ghoul2_lod_bias;
                if ui
                    .add(
                        egui::DragValue::new(&mut bias)
                            .range(0..=8)
                            .speed(1.0)
                            .update_while_editing(false),
                    )
                    .changed()
                {
                    let _ = self.set_console_cvar("r_lodbias", &bias.to_string());
                }
            },
        );

        self.egui_toggle_row(
            ui,
            "Offscreen early cull",
            "Skips the final full Ghoul2 pose and render submission once a model is outside the view frustum, while retaining the lightweight animation/angle state needed for smooth re-entry. Disable for A/B diagnostics.",
            ui::VIDEO_ROW_GHOUL2_EARLY_CULL,
            self.video.ghoul2_early_cull,
        );
    }

    fn egui_debug_tools(&mut self, ui: &mut egui::Ui) {
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
            (ui::wireframe::MAP, "Map", "BSP/map triangles (excluding promoted ocean clipmaps)."),
            (ui::wireframe::PLAYERS, "Players", "Player Ghoul2/MD3 geometry, including GPU-skinned surfaces."),
            (ui::wireframe::ENTITIES, "Entities", "Items, vehicles, model entities and inline BSP brush movers."),
            (ui::wireframe::EFFECTS, "Effects", "Transient FX/event geometry plus map-authored surface sprites."),
            (ui::wireframe::GRASS, "Procedural grass", "The actual animated per-blade grass mesh after LOD/wind deformation."),
            (ui::wireframe::OCEAN, "Ocean", "Promoted ocean clipmap triangles after wave displacement."),
            (ui::wireframe::DEFORMATION, "Surface deformation", "3D Snowflow/footprint shell geometry after deformation."),
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
        self.egui_toggle_row(
            ui,
            "GPU timestamps",
            "Queries the GPU for real per-pass durations instead of estimating \
             them from CPU submission time.",
            ui::VIDEO_ROW_GPU_TIMINGS,
            self.video.gpu_timings,
        );

    }

    fn egui_baked_ao(&mut self, ui: &mut egui::Ui) {
        if !self.video.static_bsp_ao {
            theme::banner(
                ui,
                "Baked ambient occlusion is off. Enable it under Lighting to use these settings.",
                theme::TEXT_FAINT,
            );
            ui.add_space(6.0);
        }

        const SAMPLES: [(u32, &str); 5] =
            [(8, "8"), (16, "16"), (32, "32"), (64, "64"), (128, "128")];
        if let Some(target) = quality_table_row(
            ui,
            "Samples per point",
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
            "Bake resolution",
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
            "Strength",
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
            "Range",
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
            "Only bake the current cell",
            "Bakes just the PVS cell the camera stands in. Fast to iterate on \
             while tuning the settings above.",
            ui::VIDEO_ROW_BAKED_AO_CURRENT_CELL,
            self.video.static_bsp_ao_current_cell,
        );
    }

    // ---------------------------------------------------------- environment --

    fn egui_sun(&mut self, ui: &mut egui::Ui) {
        const SOURCES: [(bool, &str); 2] = [(false, "Map shader"), (true, "Custom")];
        if let Some(target) = segmented_row(
            ui,
            "Sun source",
            "Map shader uses the strongest sun/q3map_sun/q3map_sunExt referenced by the current sky. Custom replaces that runtime sun everywhere the renderer consumes it; baked lightmaps and the map-load voxel GI bake are unchanged.",
            theme::Reset::Environment(ui::ENV_ROW_SUN_SOURCE),
            self.video.sun_override,
            &SOURCES,
        ) {
            self.environment_selected = ui::ENV_ROW_SUN_SOURCE;
            self.set_sun_override(target, target);
        }

        let authored = self.map_sun_editor_values();
        const VISIBILITY: [(ui::SunVisibilityMode, &str); 3] = [
            (ui::SunVisibilityMode::Legacy, "Legacy"),
            (ui::SunVisibilityMode::SkyPortals, "Sky portals"),
            (ui::SunVisibilityMode::Filtered, "Filtered / High"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "Sun visibility",
            "Legacy treats q3 shader sun as an unconditional directional light. Sky portals requires an authored sky surface along the sun ray. Filtered / High also evaluates translucent world surfaces with stable alpha/opacity coverage; alpha-tested cutouts keep their exact texture mask.",
            theme::Reset::Environment(ui::ENV_ROW_SUN_VISIBILITY),
            self.video.sun_visibility,
            &VISIBILITY,
        ) {
            self.environment_selected = ui::ENV_ROW_SUN_VISIBILITY;
            self.video.sun_visibility = target;
            self.sync_post_effects();
            self.mark_config_dirty();
        }

        if !self.video.sun_override {
            if let Some((yaw, pitch, intensity, color)) = authored {
                theme::banner(
                    ui,
                    &format!(
                        "MAP SUN  yaw {:.1}°   pitch {:.1}°   intensity {:.0}   rgb {:.2} {:.2} {:.2}",
                        yaw, pitch, intensity, color[0], color[1], color[2]
                    ),
                    theme::TEXT_FAINT,
                );
            } else {
                theme::banner(
                    ui,
                    "No referenced q3 shader sun on this map; renderer fallback sun is active.",
                    theme::TEXT_FAINT,
                );
            }
        }

        ui.add_enabled_ui(self.video.sun_override, |ui| {
            theme::row(
                ui,
                "Yaw / azimuth",
                "q3map sun azimuth in map space. 0° is east and 90° is north.",
                theme::Reset::Environment(ui::ENV_ROW_SUN_YAW),
                |ui| {
                    let mut value = self.video.sun_yaw;
                    let readout = format!("{value:.1}°");
                    if theme::slider(ui, &mut value, 0.0..=360.0, &readout) {
                        self.environment_selected = ui::ENV_ROW_SUN_YAW;
                        self.set_sun_yaw(value);
                    }
                },
            );
            theme::row(
                ui,
                "Pitch / elevation",
                "q3map sun elevation above the horizon. 0° is on the horizon; 90° is directly overhead.",
                theme::Reset::Environment(ui::ENV_ROW_SUN_PITCH),
                |ui| {
                    let mut value = self.video.sun_pitch;
                    let readout = format!("{value:.1}°");
                    if theme::slider(ui, &mut value, -90.0..=90.0, &readout) {
                        self.environment_selected = ui::ENV_ROW_SUN_PITCH;
                        self.set_sun_pitch(value);
                    }
                },
            );
            theme::row(
                ui,
                "Intensity",
                "Runtime q3 shader-sun intensity. This drives sun-aware effects but does not relight baked lightmaps.",
                theme::Reset::Environment(ui::ENV_ROW_SUN_INTENSITY),
                |ui| {
                    let mut value = self.video.sun_intensity;
                    let readout = format!("{value:.0}");
                    if theme::slider(ui, &mut value, 0.0..=ui::SUN_INTENSITY_MAX, &readout) {
                        self.environment_selected = ui::ENV_ROW_SUN_INTENSITY;
                        self.set_sun_intensity(value);
                    }
                },
            );
            let mut color = self.video.sun_color;
            let mut changed = false;
            color_row(
                ui,
                "Color",
                "Sun chromaticity. RGB is normalized exactly like q3map_sun, so intensity remains the separate brightness control.",
                theme::Reset::Environment(ui::ENV_ROW_SUN_COLOR),
                &mut color,
                &mut changed,
            );
            if changed {
                self.environment_selected = ui::ENV_ROW_SUN_COLOR;
                self.set_sun_color(color);
            }
        });
    }

    fn egui_fog(&mut self, ui: &mut egui::Ui) {
        const FOG: [(FogMode, &str); 4] = [
            (FogMode::Off, "Off"),
            (FogMode::LegacyDrawFog1, "Legacy 1"),
            (FogMode::LegacyDrawFog2, "Legacy 2"),
            (FogMode::Volumetric, "Volumetric"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "Fog mode",
            "Legacy 1 mirrors OpenJK r_drawfog 1: redraw fog after all material stages. \
             Legacy 2 mirrors the JKA/OpenJK default r_drawfog 2: global fog is applied \
             during material stages while local brush fog uses a redraw. Volumetric \
             marches a froxel grid instead, so fog receives light and shows god rays.",
            theme::Reset::Environment(ui::ENV_ROW_FOG_MODE),
            self.video.fog_mode,
            &FOG,
        ) {
            self.environment_selected = ui::ENV_ROW_FOG_MODE;
            self.video.fog_mode = target;
            self.sync_post_effects();
            self.mark_config_dirty();
        }
        theme::row(
            ui,
            "Fog strength",
            "Multiplies the density the map authored. Leave at the map default \
             unless a map is unplayably thick.",
            theme::Reset::Environment(ui::ENV_ROW_FOG_STRENGTH),
            |ui| {
                let mut value = self.video.fog_strength;
                let readout = if self.video.fog_strength <= 0.001 {
                    "Map default".to_owned()
                } else {
                    format!("{:.2}×", self.video.fog_strength)
                };
                if theme::slider(ui, &mut value, 0.0..=ui::MAX_FOG_STRENGTH, &readout) {
                    self.set_fog_strength(value);
                }
            },
        );
    }

    fn egui_clouds(&mut self, ui: &mut egui::Ui) {
        self.egui_environment_toggle(
            ui,
            "Volumetric clouds",
            "Replaces the flat skybox clouds with a raymarched cloud deck that \
             has real depth, lighting and motion.",
            ui::ENV_ROW_CLOUDS,
            self.video.clouds,
        );

        const TYPES: [(CloudType, &str); 3] = [
            (CloudType::Cumulus, "Cumulus"),
            (CloudType::Stratus, "Stratus"),
            (CloudType::Storm, "Storm"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "Cloud type",
            "Shape preset for the deck: puffy cumulus, flat stratus sheet, or a \
             tall dark storm front.",
            theme::Reset::Environment(ui::ENV_ROW_CLOUD_TYPE),
            self.video.cloud_type,
            &TYPES,
        ) {
            let current = index_of(&TYPES, self.video.cloud_type, 0);
            let next = index_of(&TYPES, target, current);
            self.environment_selected = ui::ENV_ROW_CLOUD_TYPE;
            self.change_environment_setting(next as i32 - current as i32);
        }

        theme::row(
            ui,
            "Raymarch quality",
            "Steps taken through the cloud volume. The single biggest cost in \
             this section: lower it first if clouds are expensive.",
            theme::Reset::Environment(ui::ENV_ROW_CLOUD_QUALITY),
            |ui| {
                let mut value = self.video.cloud_quality;
                let readout = percent(value);
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    self.set_cloud_quality(value);
                }
            },
        );
        theme::row(
            ui,
            "Coverage",
            "Fraction of the sky the deck fills, from scattered wisps to solid \
             overcast.",
            theme::Reset::Environment(ui::ENV_ROW_CLOUD_COVERAGE),
            |ui| {
                let mut value = self.video.cloud_coverage;
                let readout = percent(value);
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    self.set_cloud_coverage(value);
                }
            },
        );
        theme::row(
            ui,
            "Base height",
            "World height of the bottom of the deck, in map units.",
            theme::Reset::Environment(ui::ENV_ROW_CLOUD_HEIGHT),
            |ui| {
                let mut value = self.video.cloud_height;
                let readout = format!("{value:.0} u");
                if theme::slider(
                    ui,
                    &mut value,
                    ui::CLOUD_HEIGHT_MIN..=ui::CLOUD_HEIGHT_MAX,
                    &readout,
                ) {
                    self.set_cloud_height(value);
                }
            },
        );
        theme::row(
            ui,
            "Thickness",
            "How tall the deck is above its base. Thicker decks are darker \
             underneath and cost more to march.",
            theme::Reset::Environment(ui::ENV_ROW_CLOUD_THICKNESS),
            |ui| {
                let mut value = self.video.cloud_thickness;
                let readout = format!("{value:.0} u");
                if theme::slider(
                    ui,
                    &mut value,
                    ui::CLOUD_THICKNESS_MIN..=ui::CLOUD_THICKNESS_MAX,
                    &readout,
                ) {
                    self.set_cloud_thickness(value);
                }
            },
        );

        theme::section(ui, "RENDER COST", "");
        self.egui_environment_toggle(
            ui,
            "Cloud shadows on map",
            "Projects the deck's shadow onto the world, so cloud cover sweeps \
             across the ground as it drifts.",
            ui::ENV_ROW_CLOUD_SHADOWS,
            self.video.cloud_shadows,
        );
        const CLOUD_RES: [(CloudRenderResolution, &str); 4] = [
            (CloudRenderResolution::Quarter, "25%"),
            (CloudRenderResolution::Half, "50%"),
            (CloudRenderResolution::ThreeQuarter, "75%"),
            (CloudRenderResolution::Full, "100%"),
        ];
        if let Some(target) = quality_table_row(
            ui,
            "Render resolution",
            "Fraction of the screen resolution the clouds are marched at before \
             being upscaled. 50% is roughly four times cheaper than full.",
            theme::Reset::Environment(ui::ENV_ROW_CLOUD_RENDER_RESOLUTION),
            self.video.cloud_render_resolution,
            &CLOUD_RES,
            1,
        ) {
            self.set_environment_quality_segment(ui::ENV_ROW_CLOUD_RENDER_RESOLUTION, target);
        }
        self.egui_environment_toggle(
            ui,
            "Temporal interleave",
            "Marches part of the cloud buffer each frame and reuses the rest. \
             Much cheaper, at the cost of some smearing when the camera whips \
             around.",
            ui::ENV_ROW_CLOUD_TEMPORAL,
            self.video.cloud_temporal,
        );

        theme::section(ui, "TUNING", "Fine control over the cloud model.");
        self.egui_cloud_tuning(ui);
    }

    fn egui_cloud_tuning(&mut self, ui: &mut egui::Ui) {
        theme::row(
            ui,
            "Temporal depth fix",
            "Rejects reprojected cloud samples whose depth no longer matches, \
             which removes ghosting behind moving geometry.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_TEMPORAL_DEPTH_FIX),
            |ui| {
                if theme::switch(ui, self.video.cloud_temporal_depth_fix).is_some() {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_TEMPORAL_DEPTH_FIX, true, None);
                }
            },
        );
        theme::row(
            ui,
            "Wind shear",
            "How much faster the top of the deck moves than the bottom, which \
             tilts and stretches the clouds.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_SHEAR),
            |ui| {
                let mut value = self.video.cloud_shear;
                let readout = percent(value);
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_SHEAR, false, Some(value));
                }
            },
        );
        theme::row(
            ui,
            "Base height variation",
            "Raises and lowers the base across the deck so its underside is not \
             one flat plane.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_BASE_VARIATION),
            |ui| {
                let mut value = self.video.cloud_base_variation;
                let readout = percent(value);
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_BASE_VARIATION, false, Some(value));
                }
            },
        );
        theme::row(
            ui,
            "Cloud size",
            "Scales cloud masses in all directions. Their width already \
             tracks the thickness slider so their shape stays constant; \
             this is the multiplier on top.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_SIZE),
            |ui| {
                let mut value = self.video.cloud_size;
                // Readout is the world multiplier the shader applies, not the
                // raw slider position, so the number means something.
                // Log mapping, matching cloud_size_scale() in the shader.
                let readout = format!("{:.2}x", 0.5 * 8.0f32.powf(value));
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_SIZE, false, Some(value));
                }
            },
        );
        theme::row(
            ui,
            "Thickness variation",
            "How much the deck varies in depth. At zero every column fills the \
             whole thickness; higher leaves some shallow and some towering.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_THICKNESS_VARIATION),
            |ui| {
                let mut value = self.video.cloud_thickness_variation;
                let readout = percent(value);
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    self.apply_cloud_tuning(
                        ui::CLOUD_ROW_THICKNESS_VARIATION,
                        false,
                        Some(value),
                    );
                }
            },
        );

        theme::section(
            ui,
            "DYNAMICS / A/B",
            "Independent experimental gates. All are off by default.",
        );
        theme::row(
            ui,
            "Shape evolution",
            "Lets the high-frequency erosion field drift through the macro weather \
             field so cloud edges and billows slowly reform.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_SHAPE_EVOLUTION),
            |ui| {
                if theme::switch(ui, self.video.cloud_shape_evolution).is_some() {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_SHAPE_EVOLUTION, true, None);
                }
            },
        );
        theme::row(
            ui,
            "Terrain interaction",
            "Uses the shared map weather heightfield to lift low cloud over terrain \
             and large solid geometry. Has no effect when the deck is well above \
             the map.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_TERRAIN_INTERACTION),
            |ui| {
                if theme::switch(ui, self.video.cloud_terrain_interaction).is_some() {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_TERRAIN_INTERACTION, true, None);
                }
            },
        );
        theme::row(
            ui,
            "Empty-space skip",
            "A/B optimization: takes larger primary-ray steps only when the macro \
             cloud field is safely below the coverage threshold, then falls back \
             to fine stepping near potential cloud.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_EMPTY_SKIP),
            |ui| {
                if theme::switch(ui, self.video.cloud_empty_skip).is_some() {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_EMPTY_SKIP, true, None);
                }
            },
        );
        theme::row(
            ui,
            "Aerial perspective",
            "Blends distant clouds toward the atmosphere colour so the deck \
             reads as receding into haze.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_AERIAL),
            |ui| {
                let mut value = self.video.cloud_aerial;
                let readout = percent(value);
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_AERIAL, false, Some(value));
                }
            },
        );
        theme::row(
            ui,
            "Sky-tinted ambient",
            "Lights the shadowed side of the clouds with the sky colour instead \
             of flat grey.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_SKY_AMBIENT),
            |ui| {
                if theme::switch(ui, self.video.cloud_sky_ambient).is_some() {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_SKY_AMBIENT, true, None);
                }
            },
        );
        theme::row(
            ui,
            "History blend",
            "How much of the previous frame each temporal sample keeps. Higher              is smoother and cheaper to converge, but holds onto mistakes for              more frames.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_HISTORY_BLEND),
            |ui| {
                let mut value = self.video.cloud_history_blend;
                let readout = percent(value);
                if theme::slider(ui, &mut value, 0.0..=0.98, &readout) {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_HISTORY_BLEND, false, Some(value));
                }
            },
        );
        theme::row(
            ui,
            "Motion reject",
            "How strongly camera movement throws history away. At 100% any real              movement falls back to a full march, which is what the reference              implementation does and cannot smear; lower keeps the interleave              running while you move, at the cost of trails.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_MOTION_REJECT),
            |ui| {
                let mut value = self.video.cloud_motion_reject;
                let readout = percent(value);
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_MOTION_REJECT, false, Some(value));
                }
            },
        );
        theme::row(
            ui,
            "History depth reject",
            "Discards history on pixels where solid geometry sits in front of              the cloud layer, which stops world geometry and the player model              dragging their silhouettes across the sky.",
            theme::Reset::CloudTuning(ui::CLOUD_ROW_HISTORY_DEPTH_REJECT),
            |ui| {
                if theme::switch(ui, self.video.cloud_history_depth_reject).is_some() {
                    self.apply_cloud_tuning(ui::CLOUD_ROW_HISTORY_DEPTH_REJECT, true, None);
                }
            },
        );
    }

    fn egui_weather(&mut self, ui: &mut egui::Ui) {
        theme::section(
            ui,
            "WIND",
            "One authoritative atmospheric wind shared by clouds, rain, grass, ocean chop, spray and foam streaks.",
        );
        let wind = self.video.weather_wind;
        theme::row(
            ui,
            "Wind speed",
            "Base atmospheric wind speed in JKA map units per second.",
            theme::Reset::Environment(ui::ENV_ROW_WEATHER_WIND_SPEED),
            |ui| {
                let mut value = wind.speed;
                let readout = format!("{value:.0} u/s");
                if theme::slider(ui, &mut value, 0.0..=8192.0, &readout) {
                    self.set_weather_wind_speed(value);
                }
            },
        );
        theme::row(
            ui,
            "Wind direction",
            "Compass heading the wind blows toward, in degrees.",
            theme::Reset::Environment(ui::ENV_ROW_WEATHER_WIND_DIRECTION),
            |ui| {
                let mut value = wind.direction.rem_euclid(360.0);
                let readout = format!("{value:.0}°");
                if theme::slider(ui, &mut value, 0.0..=360.0, &readout) {
                    self.set_weather_wind_direction(value);
                }
            },
        );
        theme::row(
            ui,
            "Gust strength",
            "Amount of continuous speed variation applied to the shared weather wind.",
            theme::Reset::Environment(ui::ENV_ROW_WEATHER_GUST_STRENGTH),
            |ui| {
                let mut value = wind.gust;
                let readout = percent(value);
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    self.set_weather_gust_strength(value);
                }
            },
        );
        theme::row(
            ui,
            "Direction variation",
            "Maximum continuous angular wandering of the shared weather wind.",
            theme::Reset::Environment(ui::ENV_ROW_WEATHER_DIRECTION_VARIATION),
            |ui| {
                let mut value = wind.shift;
                let readout = format!("{value:.0}°");
                if theme::slider(ui, &mut value, 0.0..=180.0, &readout) {
                    self.set_weather_direction_variation(value);
                }
            },
        );
        theme::row(
            ui,
            "Mapper export",
            "Copy the shared weather wind as map entity keys.",
            theme::Reset::None,
            |ui| {
                if theme::ghost_button(ui, "COPY WIND KEYS").clicked() {
                    let w = self.video.weather_wind;
                    ui.ctx().copy_text(format!(
                        "\"windSpeed\" \"{}\"\n\"windAngle\" \"{}\"\n\"windGust\" \"{}\"\n\"windShift\" \"{}\"\n",
                        w.speed, w.direction, w.gust, w.shift
                    ));
                }
            },
        );

        theme::section(ui, "FOG", "Map fog and volumetric atmospheric scattering.");
        self.egui_fog(ui);

        theme::section(ui, "PRECIPITATION", "Rainfall, wetness and precipitation haze.");
        self.egui_environment_toggle(
            ui,
            "Rain",
            "Simulated rainfall with splashes, wetness on surfaces and haze. \
             Follows the shared weather wind.",
            ui::ENV_ROW_RAIN,
            self.video.rain,
        );
        const RAIN: [(RainIntensity, &str); 3] = [
            (RainIntensity::Light, "Light"),
            (RainIntensity::Rain, "Rain"),
            (RainIntensity::Heavy, "Heavy"),
        ];
        if let Some(target) = segmented_row(
            ui,
            "Rain intensity",
            "Particle count, streak length and how much haze the rain adds. \
             Heavy is noticeably more expensive.",
            theme::Reset::Environment(ui::ENV_ROW_RAIN_INTENSITY),
            self.video.rain_intensity,
            &RAIN,
        ) {
            let current = index_of(&RAIN, self.video.rain_intensity, 1);
            let next = index_of(&RAIN, target, current);
            self.environment_selected = ui::ENV_ROW_RAIN_INTENSITY;
            self.change_environment_setting(next as i32 - current as i32);
        }
    }

    fn egui_surface(&mut self, ui: &mut egui::Ui) {
        const FOOTPRINTS: [(FootprintMode, &str); 3] = [
            (FootprintMode::Off, "Off"),
            (FootprintMode::TwoD, "2D stamp"),
            (FootprintMode::ThreeD, "3D + 2D stamp"),
        ];
        if let Some(target) = quality_table_row(
            ui,
            "Footprints",
            "Tracks left in snow, sand and mud. The 2D stamp is a decal; the 3D \
             mode also displaces the surface so prints have real depth.",
            theme::Reset::Environment(ui::ENV_ROW_FOOTPRINTS),
            self.video.footprints,
            &FOOTPRINTS,
            2,
        ) {
            self.set_environment_quality_segment(ui::ENV_ROW_FOOTPRINTS, target);
        }
        self.egui_environment_toggle(
            ui,
            "Procedural grass",
            "Scatters GPU-generated grass over surfaces the map marks as \
             ground. Reacts to wind and to players moving through it.",
            ui::ENV_ROW_GRASS,
            self.video.grass,
        );
        if self.video.grass && !self.applied_grass {
            theme::banner(
                ui,
                "Grass needs Apply: this map was prepared without grass resources.",
                theme::WARNING,
            );
        }
    }

    fn egui_water(&mut self, ui: &mut egui::Ui) {
        self.egui_environment_toggle(
            ui,
            "Godot ocean waves",
            "Replaces flat water surfaces with an FFT wave simulation ported \
             from GodotOceanWaves, including displacement and foam.",
            ui::ENV_ROW_OCEAN,
            self.video.ocean,
        );
        if self.video.ocean && !self.applied_ocean {
            theme::banner(
                ui,
                "Ocean needs Apply: this renderer started without ocean resources.",
                theme::WARNING,
            );
        }

        if !self.authored_oceans.is_empty() {
            let old = self.authored_ocean_selected;
            let mut selected = old;
            theme::row(
                ui,
                "Authored ocean",
                "Select which server/map-authored ocean volume is shown in the local tuning controls.",
                theme::Reset::None,
                |ui| {
                    egui::ComboBox::from_id_salt("authored_ocean_selector")
                        .selected_text(format!("Ocean {}", self.authored_oceans[selected].index))
                        .show_ui(ui, |ui| {
                            for (index, ocean) in self.authored_oceans.iter().enumerate() {
                                ui.selectable_value(&mut selected, index, format!("Ocean {} · weather {}", ocean.index, ocean.weather));
                            }
                        });
                },
            );
            self.authored_ocean_selected = selected;
            if old != self.authored_ocean_selected {
                self.authored_ocean_preview = false;
                self.publish_authored_oceans();
            }
            if !self.authored_ocean_preview {
                let ocean = &self.authored_oceans[self.authored_ocean_selected];
                self.video.ocean_settings.authored = ocean.waves;
            }

            let mut preview = self.authored_ocean_preview;
            let mut preview_changed = false;
            theme::row(
                ui,
                "Local preview override",
                "Temporarily edit map-authored swell locally without changing the server values.",
                theme::Reset::None,
                |ui| {
                    if let Some(value) = theme::switch(ui, preview) {
                        preview = value;
                        preview_changed = true;
                    }
                },
            );
            if preview_changed {
                self.authored_ocean_preview = preview;
                self.publish_authored_oceans();
            }
            theme::row(
                ui,
                "Server ocean values",
                "Discard the local preview and restore the selected server/map-authored values.",
                theme::Reset::None,
                |ui| {
                    if theme::ghost_button(ui, "RESTORE SERVER VALUES").clicked() {
                        self.authored_ocean_preview = false;
                        self.publish_authored_oceans();
                        let ocean = &self.authored_oceans[self.authored_ocean_selected];
                        self.video.ocean_settings.authored = ocean.waves;
                    }
                },
            );
        }
        let mut settings = self.video.ocean_settings;
        let mut changed = false;

        theme::section(
            ui,
            "SIMULATION",
            "Map-facing swell controls. Distances and speeds use JKA map units (40 units = 1 metre).",
        );
        ui.add_enabled_ui(self.authored_oceans.is_empty() || self.authored_ocean_preview, |ui| {
            let a = &mut settings.authored;
            ocean_slider(ui, "Swell amplitude", "Height scale of the authored long swell, in map units.", theme::Reset::None, &mut a.amplitude, 0.0..=5000.0, &mut changed);
            ocean_slider(ui, "Swell wavelength", "Distance between authored swell crests, in map units.", theme::Reset::None, &mut a.wavelength, 64.0..=32768.0, &mut changed);
            ocean_slider(ui, "Swell travel direction", "Heading of the authored swell in degrees.", theme::Reset::None, &mut a.direction, -180.0..=180.0, &mut changed);
            ocean_slider(ui, "Primary wave speed", "Travel speed of the authored swell in map units per second.", theme::Reset::None, &mut a.speed, -4096.0..=4096.0, &mut changed);
            ocean_slider(ui, "Base choppiness", "Horizontal steepness of the authored swell.", theme::Reset::None, &mut a.steepness, 0.0..=1.0, &mut changed);
            ocean_slider(ui, "Cross-sea / FFT blend", "How strongly the first two FFT cascades roughen and cross the authored swell.", theme::Reset::None, &mut a.slosh, 0.0..=1.0, &mut changed);
            ocean_slider(ui, "Wind chop", "Scales the weather-driven fine chop cascade.", theme::Reset::None, &mut a.wind_chop, 0.0..=4.0, &mut changed);
            ocean_slider(ui, "Foam amount", "Global multiplier for crest foam generation.", theme::Reset::None, &mut a.foam, 0.0..=4.0, &mut changed);
            ocean_slider(ui, "Foam lifetime", "How long accumulated crest foam persists before decaying.", theme::Reset::None, &mut a.foam_lifetime, 0.1..=30.0, &mut changed);
            ocean_slider(ui, "Spray amount", "Global multiplier for crest-triggered sea spray particles.", theme::Reset::None, &mut a.spray, 0.0..=4.0, &mut changed);
            theme::row(
                ui,
                "Wave seed",
                "Random seed used to generate the repeatable FFT spectrum phases.",
                theme::Reset::None,
                |ui| {
                    changed |= ui.add(egui::DragValue::new(&mut a.seed).speed(1.0)).changed();
                },
            );

        });

        theme::row(
            ui,
            "Mapper export",
            "Copy the current values as map entity keys.",
            theme::Reset::None,
            |ui| {
                if theme::ghost_button(ui, "COPY OCEAN KEYS").clicked() {
                    let a = settings.authored;
                    ui.ctx().copy_text(format!("\"amplitude\" \"{}\"\n\"wavelength\" \"{}\"\n\"speed\" \"{}\"\n\"waveAngle\" \"{}\"\n\"steepness\" \"{}\"\n\"slosh\" \"{}\"\n\"waveSeed\" \"{}\"\n\"waveModel\" \"1\"\n\"windChop\" \"{}\"\n\"foamAmount\" \"{}\"\n\"foamLifetime\" \"{}\"\n\"sprayAmount\" \"{}\"\n",a.amplitude,a.wavelength,a.speed,a.direction,a.steepness,a.slosh,a.seed,a.wind_chop,a.foam,a.foam_lifetime,a.spray));
                }
            },
        );
        const MAP_SIZES: [(u32, &str); 4] =
            [(128, "128"), (256, "256"), (512, "512"), (1024, "1024")];
        if let Some(target) = quality_table_row(
            ui,
            "FFT resolution",
            "Resolution of each cascade's displacement/normal/foam simulation \
             textures and FFT input. This is NOT the water mesh grid; Mesh \
             quality below controls geometry. Higher resolves finer ripples; \
             memory cost grows with N^2 and FFT work roughly with N^2 log N.",
            theme::Reset::Ocean(OCEAN_MAP_SIZE),
            settings.map_size,
            &MAP_SIZES,
            1,
        ) {
            settings.map_size = MAP_SIZES[target].0;
            changed = true;
        }
        const MESHES: [(u8, &str); 2] = [(0, "Low"), (1, "High")];
        if let Some(target) = quality_table_row(
            ui,
            "Mesh quality",
            "Tessellation density of the water surface the displacement is \
             applied to.",
            theme::Reset::Ocean(OCEAN_MESH_QUALITY),
            settings.mesh_quality,
            &MESHES,
            0,
        ) {
            settings.mesh_quality = MESHES[target].0;
            changed = true;
        }
        theme::row(
            ui,
            "Simulation updates",
            "How often the wave spectrum is stepped, independent of frame rate. \
             Lower rates save GPU time and make the motion choppier.",
            theme::Reset::Ocean(OCEAN_UPDATES),
            |ui| {
                let mut value = settings.updates_per_second;
                let readout = format!("{value:.0} Hz");
                if theme::slider(ui, &mut value, 0.0..=60.0, &readout) {
                    settings.updates_per_second = value;
                    changed = true;
                }
            },
        );

        theme::row(
            ui,
            "Sea spray",
            "GodotOceanWaves-style crest spray. Samples the same FFT foam/normal field and emits only from breaking whitecaps.",
            theme::Reset::Ocean(OCEAN_SEA_SPRAY),
            |ui| {
                if let Some(value) = theme::switch(ui, settings.sea_spray) {
                    settings.sea_spray = value;
                    changed = true;
                }
            },
        );

        theme::row(
            ui,
            "Wind foam streaks",
            "Severe-wind surface foam transport. Reuses real FFT whitecaps, advects them with weather wind, and concentrates them into long wind-aligned foam streaks for gale conditions.",
            theme::Reset::Ocean(OCEAN_WIND_FOAM),
            |ui| {
                if let Some(value) = theme::switch(ui, settings.wind_foam_streaks) {
                    settings.wind_foam_streaks = value;
                    changed = true;
                }
            },
        );

        theme::section(ui, "SURFACE", "");
        theme::row(
            ui,
            "Roughness",
            "Microfacet roughness of the water. Low is glassy and mirror-like, \
             high is a dull matte sheet.",
            theme::Reset::Ocean(OCEAN_ROUGHNESS),
            |ui| {
                let mut value = settings.roughness;
                let readout = percent(value);
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    settings.roughness = value;
                    changed = true;
                }
            },
        );
        theme::row(
            ui,
            "Normal strength",
            "How strongly the simulated ripple normals perturb the lighting.",
            theme::Reset::Ocean(OCEAN_NORMAL_STRENGTH),
            |ui| {
                let mut value = settings.normal_strength;
                let readout = percent(value);
                if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                    settings.normal_strength = value;
                    changed = true;
                }
            },
        );
        color_row(
            ui,
            "Water color",
            "Colour of the water body itself, before reflection and foam.",
            theme::Reset::Ocean(OCEAN_WATER_COLOR),
            &mut settings.water_color,
            &mut changed,
        );
        color_row(
            ui,
            "Foam color",
            "Colour of whitecaps and the foam trail behind breaking waves.",
            theme::Reset::Ocean(OCEAN_FOAM_COLOR),
            &mut settings.foam_color,
            &mut changed,
        );

        theme::section(ui, "WATER OPTICS", "");
        color_row(ui, "Fog color", "Scattered underwater light; converted to linear color.",
            theme::Reset::Ocean(9), &mut settings.optics.fog_color, &mut changed);
        ocean_slider(ui, "Fog distance", "Base absorption distance in game units.", theme::Reset::Ocean(10), &mut settings.optics.fog_distance, 1.0..=4000.0, &mut changed);
        ocean_slider(ui, "Transparency", "Multiplies the absorption distance above and below water.", theme::Reset::Ocean(11), &mut settings.optics.transparency, 0.1..=16.0, &mut changed);
        ocean_slider(ui, "Depth darkening", "Sunlight penetration multiplier; larger values stay bright deeper.", theme::Reset::Ocean(12), &mut settings.optics.depth_darkening, 0.01..=8.0, &mut changed);
        ocean_slider(ui, "Refraction", "Wave distortion of objects viewed through water. Zero disables distortion.", theme::Reset::Ocean(13), &mut settings.optics.refraction, 0.0..=0.15, &mut changed);
        ocean_slider(ui, "Caustics", "Experimental wave-curvature sunlight focusing. Not shadow-aware yet; zero disables.", theme::Reset::Ocean(14), &mut settings.optics.caustics, 0.0..=4.0, &mut changed);
        ocean_slider(ui, "Underwater cull", "Absorption-distance multiple for fully submerged distant world geometry. Zero disables.", theme::Reset::Ocean(15), &mut settings.optics.underwater_cull, 0.0..=8.0, &mut changed);

        theme::section(
            ui,
            "ADVANCED SPECTRUM",
            "Three layered FFT spectra reduce tiling. These controls use the native GodotOceanWaves units: metres, m/s and km.",
        );
        for cascade_index in 0..crate::ocean::OCEAN_CASCADES {
            let (role, detail) = match cascade_index {
                0 => ("BROAD SWELL", "Largest repeating wave field; carries most large-scale displacement and foam."),
                1 => ("MID-SCALE CROSS SEA", "Secondary wave field layered over the swell to break up repetition and add crossing chop."),
                _ => ("FINE WIND CHOP", "Small-scale weather-driven detail; mainly normals/foam with only a small displacement contribution."),
            };
            let title = format!("CASCADE {}  ·  {role}", cascade_index + 1);
            egui::CollapsingHeader::new(theme::plain(&title, 13.0, theme::TEXT))
                .id_salt(("ocean_cascade", cascade_index))
                .default_open(false)
                .show(ui, |ui| {
                    theme::banner(ui, detail, theme::TEXT_DIM);
                    let cascade = &mut settings.cascades[cascade_index];
                    ocean_slider(ui, "Tile length X", "Repeat size of this FFT cascade along X, in metres. Larger values carry broader waves and repeat less often.", theme::Reset::OceanCascade(cascade_index as u8, 0), &mut cascade.tile_length[0], 1.0..=2000.0, &mut changed);
                    ocean_slider(ui, "Tile length Y", "Repeat size of this FFT cascade along Y, in metres. Keeping X/Y different can make repetition less obvious.", theme::Reset::OceanCascade(cascade_index as u8, 1), &mut cascade.tile_length[1], 1.0..=2000.0, &mut changed);
                    ocean_slider(ui, "Displacement scale", "Multiplier for this cascade's geometric displacement. Reduce it on higher-frequency cascades to avoid over-busy silhouettes.", theme::Reset::OceanCascade(cascade_index as u8, 2), &mut cascade.displacement_scale, 0.0..=2.0, &mut changed);
                    ocean_slider(ui, "Normal scale", "Multiplier for this cascade's shading normals. It can add small-scale sparkle/chop without adding the same amount of geometry displacement.", theme::Reset::OceanCascade(cascade_index as u8, 3), &mut cascade.normal_scale, 0.0..=2.0, &mut changed);
                    if cascade_index < 2 {
                        ocean_slider(ui, "Spectrum wind speed", "Reference wind speed for this TMA/JONSWAP spectrum, in m/s. It changes spectrum energy and peak frequency; it is separate from live weather wind.", theme::Reset::OceanCascade(cascade_index as u8, 4), &mut cascade.wind_speed, 0.0001..=60.0, &mut changed);
                        ocean_slider(ui, "Spectrum angle offset", "Directional offset from the authored swell heading, in degrees. This lets the cascades cross rather than stack in exactly one direction.", theme::Reset::OceanCascade(cascade_index as u8, 5), &mut cascade.wind_direction, -360.0..=360.0, &mut changed);
                    } else {
                        theme::banner(
                            ui,
                            "This cascade follows the weather wind speed and direction.",
                            theme::TEXT_FAINT,
                        );
                    }
                    ocean_slider(ui, "Fetch length", "Distance from shoreline / wind fetch, in kilometres. Longer fetch shifts the TMA/JONSWAP sea state toward more developed waves.", theme::Reset::OceanCascade(cascade_index as u8, 6), &mut cascade.fetch_length, 0.1..=1000.0, &mut changed);
                    ocean_slider(ui, "Directional concentration", "GodotOceanWaves calls this swell. Higher values elongate/concentrate wave energy around the preferred heading; it is not your authored swell height.", theme::Reset::OceanCascade(cascade_index as u8, 7), &mut cascade.swell, 0.0..=2.0, &mut changed);
                    ocean_slider(ui, "Spread", "Mix between strongly directional and flatter/isotropic wave energy. Higher values allow more energy away from the preferred heading.", theme::Reset::OceanCascade(cascade_index as u8, 8), &mut cascade.spread, 0.0..=1.0, &mut changed);
                    ocean_slider(ui, "Detail", "Small-wave suppression control. Lower values attenuate high-frequency waves; 1 keeps the full high-frequency tail.", theme::Reset::OceanCascade(cascade_index as u8, 9), &mut cascade.detail, 0.0..=1.0, &mut changed);
                    ocean_slider(ui, "Whitecap threshold", "Controls how steep/compressed a crest must be before foam accumulates. Higher values make whitecaps trigger more readily in this implementation.", theme::Reset::OceanCascade(cascade_index as u8, 10), &mut cascade.whitecap, 0.0..=2.0, &mut changed);
                    ocean_slider(ui, "Foam amount", "Per-cascade foam growth multiplier. Lifetime/decay is controlled by the main Foam lifetime setting above.", theme::Reset::OceanCascade(cascade_index as u8, 11), &mut cascade.foam_amount, 0.0..=10.0, &mut changed);
                    if cascade.displacement_scale <= 0.001 {
                        theme::banner(
                            ui,
                            "Displacement is 0: this cascade can still affect normals/foam, but not the mesh silhouette.",
                            theme::TEXT_FAINT,
                        );
                    }
                    if cascade.foam_amount <= 0.001 {
                        theme::banner(
                            ui,
                            "Foam amount is 0: Whitecap changes in this cascade will not visibly add foam until Foam amount is raised.",
                            theme::TEXT_FAINT,
                        );
                    }
                });
        }

        ui.add_space(10.0);
        if theme::ghost_button(ui, "RESET OCEAN DEFAULTS").clicked() {
            settings = crate::ocean::OceanSettings::default();
            changed = true;
        }

        if changed {
            self.video.ocean_settings = settings;
            self.commit_ocean_settings();
        }
    }

    fn egui_physics(&mut self, ui: &mut egui::Ui) {
        theme::banner(
            ui,
            "RAPIER IS CLIENT-ONLY VISUAL PHYSICS — OPENJK/JKA MOVEMENT AND SERVER SNAPSHOTS STAY AUTHORITATIVE.",
            theme::TEXT_FAINT,
        );

        theme::section(ui, "SIMULATION", "Independent from authoritative player movement and server snapshots.");
        self.egui_physics_toggle(
            ui,
            "Client physics",
            "Master switch for Rapier-driven client-side visual physics. It must never replace \
             OpenJK/JKA movement prediction or server-authoritative entity state.",
            PHYS_CLIENT_ENABLED,
            self.video.client_physics,
        );

        const RATES: [(u32, &str); 4] = [(30, "30 Hz"), (60, "60 Hz"), (120, "120 Hz"), (240, "240 Hz")];
        if let Some(rate) = segmented_row(
            ui,
            "Simulation rate",
            "Fixed Rapier timestep for visual physics. 60 Hz is the baseline; higher rates improve \
             fast contacts and joints at additional CPU cost.",
            theme::Reset::Physics(PHYS_RATE),
            self.video.client_physics_hz,
            &RATES,
        ) {
            self.video.client_physics_hz = rate;
            self.physics_menu_changed();
        }

        const SUBSTEPS: [(u32, &str); 4] = [(1, "1"), (2, "2"), (4, "4"), (8, "8")];
        if let Some(steps) = quality_table_row(
            ui,
            "Catch-up steps",
            "Maximum fixed steps the visual simulation may execute after a slow frame. This caps \
             spiral-of-death behavior without changing the fixed timestep.",
            theme::Reset::Physics(PHYS_MAX_SUBSTEPS),
            self.video.client_physics_max_substeps,
            &SUBSTEPS,
            2,
        ) {
            self.video.client_physics_max_substeps = SUBSTEPS[steps].0;
            self.physics_menu_changed();
        }

        self.egui_physics_toggle(
            ui,
            "Continuous collision detection",
            "Enables Rapier CCD for fast-moving visual bodies that would otherwise tunnel through \
             thin map geometry. Individual bodies can still opt out later.",
            PHYS_CCD,
            self.video.client_physics_ccd,
        );
        self.egui_physics_toggle(
            ui,
            "Sleeping",
            "Lets inactive rigid bodies sleep so settled ragdolls, props and debris stop consuming \
             solver time until disturbed.",
            PHYS_SLEEPING,
            self.video.client_physics_sleeping,
        );

        theme::section(ui, "RAGDOLLS", "Death and knockdown presentation driven by a local articulated body.");
        self.egui_physics_toggle(
            ui,
            "Ragdolls",
            "Allow player/NPC corpses to transition from snapshot-driven animation to a client-only \
             Rapier ragdoll after the authoritative death event.",
            PHYS_RAGDOLLS,
            self.video.ragdolls,
        );
        const RAGDOLL_MAX: [(u32, &str); 5] = [(2, "2"), (4, "4"), (8, "8"), (16, "16"), (32, "32")];
        if let Some(index) = quality_table_row(
            ui,
            "Active ragdolls",
            "Maximum articulated ragdolls kept in the local physics world before the oldest are \
             retired to a cheaper static/animated corpse path.",
            theme::Reset::Physics(PHYS_RAGDOLL_MAX),
            self.video.ragdoll_max,
            &RAGDOLL_MAX,
            2,
        ) {
            self.video.ragdoll_max = RAGDOLL_MAX[index].0;
            self.physics_menu_changed();
        }
        const RAGDOLL_LIFE: [(u32, &str); 5] = [(5, "5 s"), (10, "10 s"), (20, "20 s"), (30, "30 s"), (60, "60 s")];
        let ragdoll_life = self.video.ragdoll_lifetime.round().clamp(1.0, 300.0) as u32;
        if let Some(index) = quality_table_row(
            ui,
            "Ragdoll lifetime",
            "How long a client ragdoll remains actively simulated before its bodies are put to \
             sleep. The solved pose remains visible; server entity lifetime is unchanged.",
            theme::Reset::Physics(PHYS_RAGDOLL_LIFETIME),
            ragdoll_life,
            &RAGDOLL_LIFE,
            2,
        ) {
            self.video.ragdoll_lifetime = RAGDOLL_LIFE[index].0 as f32;
            self.physics_menu_changed();
        }
        self.egui_physics_toggle(
            ui,
            "Ragdoll self-collision",
            "Allows limbs on the same ragdoll to collide. This can look more physical but increases \
             solver work and can make constrained skeletons less stable.",
            PHYS_RAGDOLL_SELF_COLLISION,
            self.video.ragdoll_self_collision,
        );

        theme::section(ui, "PROPS", "Client-only dynamic objects layered over the server world.");
        self.egui_physics_toggle(
            ui,
            "Dynamic props",
            "Enable Rapier simulation for explicitly promoted visual props. Server-owned movers and \
             gameplay entities remain snapshot-driven.",
            PHYS_PROPS,
            self.video.physics_props,
        );
        const PROP_MAX: [(u32, &str); 5] = [(32, "32"), (64, "64"), (96, "96"), (192, "192"), (384, "384")];
        if let Some(index) = quality_table_row(
            ui,
            "Active props",
            "Budget for simultaneously simulated loose visual props.",
            theme::Reset::Physics(PHYS_PROP_MAX),
            self.video.physics_prop_max,
            &PROP_MAX,
            2,
        ) {
            self.video.physics_prop_max = PROP_MAX[index].0;
            self.physics_menu_changed();
        }

        theme::section(ui, "DEBRIS", "Short-lived physical fragments spawned from presentation events.");
        self.egui_physics_toggle(
            ui,
            "Physical debris",
            "Use rigid bodies for client-spawned fragments instead of purely ballistic particles.",
            PHYS_DEBRIS,
            self.video.physics_debris,
        );
        const DEBRIS_MAX: [(u32, &str); 5] = [(64, "64"), (128, "128"), (192, "192"), (384, "384"), (768, "768")];
        if let Some(index) = quality_table_row(
            ui,
            "Debris pieces",
            "Maximum number of short-lived debris rigid bodies in the local simulation.",
            theme::Reset::Physics(PHYS_DEBRIS_MAX),
            self.video.physics_debris_max,
            &DEBRIS_MAX,
            2,
        ) {
            self.video.physics_debris_max = DEBRIS_MAX[index].0;
            self.physics_menu_changed();
        }
        const DEBRIS_LIFE: [(u32, &str); 5] = [(2, "2 s"), (5, "5 s"), (10, "10 s"), (20, "20 s"), (30, "30 s")];
        let debris_life = self.video.physics_debris_lifetime.round().clamp(1.0, 300.0) as u32;
        if let Some(index) = quality_table_row(
            ui,
            "Debris lifetime",
            "How long physical debris remains before it is culled from the local physics world.",
            theme::Reset::Physics(PHYS_DEBRIS_LIFETIME),
            debris_life,
            &DEBRIS_LIFE,
            2,
        ) {
            self.video.physics_debris_lifetime = DEBRIS_LIFE[index].0 as f32;
            self.physics_menu_changed();
        }

        theme::section(ui, "INTERACTION", "One-way impulses into visual physics; gameplay remains server authoritative.");
        self.egui_physics_toggle(
            ui,
            "Player pushes props",
            "Represent the local/server player capsule as kinematic input to Rapier so contact can \
             push visual props without letting those props alter JKA player movement.",
            PHYS_PLAYER_PUSH,
            self.video.physics_player_push,
        );
        self.egui_physics_toggle(
            ui,
            "Weapon impacts",
            "Apply client-visible saber, projectile and hitscan impact impulses to ragdolls, props \
             and debris when corresponding game events are observed.",
            PHYS_WEAPON_IMPULSES,
            self.video.physics_weapon_impulses,
        );
        self.egui_physics_toggle(
            ui,
            "Explosion impulses",
            "Apply radial impulses from observed explosion events to client physics bodies.",
            PHYS_EXPLOSION_IMPULSES,
            self.video.physics_explosion_impulses,
        );
        self.egui_physics_toggle(
            ui,
            "Force power impulses",
            "Feed observed Force push/pull style events into the visual physics layer where the \
             protocol exposes enough information to do so faithfully.",
            PHYS_FORCE_IMPULSES,
            self.video.physics_force_impulses,
        );

        theme::section(ui, "DEBUG", "Development-only diagnostics for the Rapier world.");
        self.egui_physics_toggle(
            ui,
            "Physics diagnostics",
            "Print Rapier initialization, ragdoll candidate, rejection and spawn diagnostics to the console.",
            PHYS_DEBUG_DRAW,
            self.video.physics_debug_draw,
        );
        self.egui_physics_toggle(
            ui,
            "Physics stats",
            "Print Rapier body, collider, joint and fixed-step counters to the console once per second.",
            PHYS_STATS,
            self.video.physics_stats,
        );
    }

    fn egui_physics_toggle(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        tip: &str,
        field: u8,
        current: bool,
    ) {
        theme::row(ui, label, tip, theme::Reset::Physics(field), |ui| {
            if let Some(value) = theme::switch(ui, current) {
                self.set_physics_menu_bool(field, value);
            }
        });
    }

    fn set_physics_menu_bool(&mut self, field: u8, value: bool) {
        match field {
            PHYS_CLIENT_ENABLED => self.video.client_physics = value,
            PHYS_CCD => self.video.client_physics_ccd = value,
            PHYS_SLEEPING => self.video.client_physics_sleeping = value,
            PHYS_RAGDOLLS => self.video.ragdolls = value,
            PHYS_RAGDOLL_SELF_COLLISION => self.video.ragdoll_self_collision = value,
            PHYS_PROPS => self.video.physics_props = value,
            PHYS_DEBRIS => self.video.physics_debris = value,
            PHYS_PLAYER_PUSH => self.video.physics_player_push = value,
            PHYS_WEAPON_IMPULSES => self.video.physics_weapon_impulses = value,
            PHYS_EXPLOSION_IMPULSES => self.video.physics_explosion_impulses = value,
            PHYS_FORCE_IMPULSES => self.video.physics_force_impulses = value,
            PHYS_DEBUG_DRAW => self.video.physics_debug_draw = value,
            PHYS_STATS => self.video.physics_stats = value,
            _ => return,
        }
        self.physics_menu_changed();
    }

    fn physics_menu_changed(&mut self) {
        self.mark_config_dirty();
        if self.map_prepare_restart_required() {
            self.console_status =
                "CLIENT PHYSICS: PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART TO BUILD MAP COLLISION".into();
        }
        self.egui_repaint_requested = true;
        self.publish_ui();
    }

    // --------------------------------------------------------------- rows --

    pub(super) fn set_world_lighting_quality(&mut self, target: usize) {
        match target.min(2) {
            0 => {
                // Preserve vertex_lighting while disabled so r_fullbright can
                // be used as a true A/B master without forgetting the chosen
                // enabled path.
                self.video.world_lighting = false;
            }
            1 => {
                self.video.world_lighting = true;
                self.video.vertex_lighting = true;
            }
            _ => {
                self.video.world_lighting = true;
                self.video.vertex_lighting = false;
            }
        }
        self.sync_classic_world_lighting();
        self.mark_config_dirty();
    }

    fn egui_toggle_row(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        tip: &str,
        row: usize,
        current: bool,
    ) {
        theme::row(ui, label, tip, theme::Reset::Video(row), |ui| {
            if theme::switch(ui, current).is_some() {
                self.video_selected = row;
                self.change_video_setting(1);
            }
        });
    }

    fn egui_environment_toggle(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        tip: &str,
        row: usize,
        current: bool,
    ) {
        theme::row(ui, label, tip, theme::Reset::Environment(row), |ui| {
            if theme::switch(ui, current).is_some() {
                self.environment_selected = row;
                self.change_environment_setting(1);
            }
        });
    }
}

// --------------------------------------------------------------- helpers --

/// Field ids carried by [`theme::Reset::Ocean`] and
/// [`theme::Reset::OceanCascade`]. Ocean settings live in a plain struct rather
/// than the indexed row tables the rest of the page uses, so they get their own
/// compact numbering.
const PHYS_CLIENT_ENABLED: u8 = 0;
const PHYS_RATE: u8 = 1;
const PHYS_MAX_SUBSTEPS: u8 = 2;
const PHYS_CCD: u8 = 3;
const PHYS_SLEEPING: u8 = 4;
const PHYS_RAGDOLLS: u8 = 5;
const PHYS_RAGDOLL_MAX: u8 = 6;
const PHYS_RAGDOLL_LIFETIME: u8 = 7;
const PHYS_RAGDOLL_SELF_COLLISION: u8 = 8;
const PHYS_PROPS: u8 = 9;
const PHYS_PROP_MAX: u8 = 10;
const PHYS_DEBRIS: u8 = 11;
const PHYS_DEBRIS_MAX: u8 = 12;
const PHYS_DEBRIS_LIFETIME: u8 = 13;
const PHYS_PLAYER_PUSH: u8 = 14;
const PHYS_WEAPON_IMPULSES: u8 = 15;
const PHYS_EXPLOSION_IMPULSES: u8 = 16;
const PHYS_FORCE_IMPULSES: u8 = 17;
const PHYS_DEBUG_DRAW: u8 = 18;
const PHYS_STATS: u8 = 19;

const OCEAN_MAP_SIZE: u8 = 0;
const OCEAN_MESH_QUALITY: u8 = 1;
const OCEAN_UPDATES: u8 = 2;
const OCEAN_ROUGHNESS: u8 = 3;
const OCEAN_NORMAL_STRENGTH: u8 = 4;
const OCEAN_WATER_COLOR: u8 = 5;
const OCEAN_FOAM_COLOR: u8 = 6;
const OCEAN_SEA_SPRAY: u8 = 7;
const OCEAN_WIND_FOAM: u8 = 8;


fn percent(value: f32) -> String {
    format!("{:.0}%", value * 100.0)
}

fn index_of<T: Copy + PartialEq>(options: &[(T, &str)], value: T, fallback: usize) -> usize {
    options
        .iter()
        .position(|(candidate, _)| *candidate == value)
        .unwrap_or(fallback)
}

fn mode_index<T: Copy + PartialEq>(options: &[(T, &str, &str)], value: T) -> usize {
    options
        .iter()
        .position(|(candidate, _, _)| *candidate == value)
        .unwrap_or(0)
}

/// Mutually exclusive options with no cost ordering: flat chips.
fn segmented_row<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    label: &str,
    tip: &str,
    reset: theme::Reset,
    current: T,
    options: &[(T, &str)],
) -> Option<T> {
    theme::row(ui, label, tip, reset, |ui| {
        let mut selected = None;
        for (value, text) in options.iter().copied() {
            if theme::chip(ui, text, value == current).clicked() && value != current {
                selected = Some(value);
            }
            ui.add_space(3.0);
        }
        selected
    })
}

/// Ordered options that may have no active value. This is used for quality
/// presets because changing any preset-owned cvar puts the row into a real
/// Custom state instead of pretending one of the four presets is still active.
fn optional_quality_table_row<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    label: &str,
    tip: &str,
    reset: theme::Reset,
    current: Option<T>,
    options: &[(T, &str)],
) -> Option<usize> {
    let index = current.and_then(|value| {
        options
            .iter()
            .position(|(candidate, _)| *candidate == value)
    });
    let labels: Vec<&str> = options.iter().map(|(_, text)| *text).collect();
    theme::row(ui, label, tip, reset, |ui| {
        let width = theme::track_width(ui);
        theme::quality_meter_optional(ui, index, &labels, width, "Custom")
    })
}

/// Ordered options: quality meter driven by the table's own order.
fn quality_table_row<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    label: &str,
    tip: &str,
    reset: theme::Reset,
    current: T,
    options: &[(T, &str)],
    fallback: usize,
) -> Option<usize> {
    let index = index_of(options, current, fallback);
    let labels: Vec<&str> = options.iter().map(|(_, text)| *text).collect();
    quality_row(ui, label, tip, reset, index, &labels)
}

fn quality_row(
    ui: &mut egui::Ui,
    label: &str,
    tip: &str,
    reset: theme::Reset,
    current: usize,
    labels: &[&str],
) -> Option<usize> {
    theme::row(ui, label, tip, reset, |ui| {
        let width = theme::track_width(ui);
        theme::quality_meter(ui, current, labels, width)
    })
}

/// Ordered options that each carry an explanation of what the renderer does.
fn mode_row<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    label: &str,
    tip: &str,
    reset: theme::Reset,
    current: T,
    options: &[(T, &str, &str)],
) -> Option<usize> {
    let index = mode_index(options, current);
    let labels: Vec<&str> = options.iter().map(|(_, text, _)| *text).collect();
    let note = options.get(index).map(|(_, _, note)| *note).unwrap_or("");
    theme::row_with_note(ui, label, tip, reset, note, |ui| {
        let width = theme::track_width(ui);
        theme::quality_meter(ui, index, &labels, width)
    })
}

fn color_row(
    ui: &mut egui::Ui,
    label: &str,
    tip: &str,
    reset: theme::Reset,
    values: &mut [f32; 3],
    changed: &mut bool,
) {
    theme::row(ui, label, tip, reset, |ui| {
        let mut rgb = *values;
        if ui.color_edit_button_rgb(&mut rgb).changed() {
            *values = rgb;
            *changed = true;
        }
        ui.add_space(10.0);
        theme::glow_label(
            ui,
            &format!("{:.2}  {:.2}  {:.2}", values[0], values[1], values[2]),
            12.5,
            theme::TEXT_FAINT,
        );
    });
}

fn ocean_slider(
    ui: &mut egui::Ui,
    label: &str,
    tip: &str,
    reset: theme::Reset,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    changed: &mut bool,
) {
    theme::row(ui, label, tip, reset, |ui| {
        let mut local = *value;
        let readout = format!("{local:.3}");
        if theme::slider(ui, &mut local, range, &readout) {
            *value = local;
            *changed = true;
        }
    });
}

// ---------------------------------------------------------------- resets --

impl App {
    /// Restore one setting to the value a fresh install would have.
    ///
    /// Each arm mirrors the corresponding arm of `change_video_setting` /
    /// `change_environment_setting`: same field, same renderer sync, same
    /// dirty-marking. Assigning the default directly rather than cycling the
    /// control to it avoids walking through every intermediate state, which for
    /// anti-aliasing or the backend would mean several pipeline rebuilds on one
    /// click.
    pub(super) fn apply_setting_reset(&mut self, reset: theme::Reset) {
        match reset {
            theme::Reset::None => {}
            theme::Reset::Video(row) => self.reset_video_row(row),
            theme::Reset::Environment(row) => self.reset_environment_row(row),
            theme::Reset::CloudTuning(row) => self.reset_cloud_tuning_row(row),
            theme::Reset::Physics(field) => self.reset_physics_field(field),
            theme::Reset::Ocean(field) => self.reset_ocean_global(field),
            theme::Reset::OceanCascade(cascade, field) => {
                self.reset_ocean_cascade(cascade as usize, field)
            }
        }
        self.egui_repaint_requested = true;
        self.publish_ui();
    }

    fn reset_video_row(&mut self, row: usize) {
        let defaults = VideoSettings::default();
        match row {
            ui::VIDEO_ROW_FULLSCREEN => self.set_fullscreen_mode(defaults.fullscreen),
            ui::VIDEO_ROW_RESOLUTION => {
                self.video.resolution = defaults.resolution;
                self.mark_config_dirty();
                self.console_status = format!(
                    "RESOLUTION: {}X{} (PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART)",
                    self.video.resolution[0], self.video.resolution[1]
                );
            }
            ui::VIDEO_ROW_VSYNC => {
                self.video.vsync = defaults.vsync;
                self.render_command(RenderCommand::SetVsync(self.video.vsync));
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_MAX_FRAME_LATENCY => {
                self.set_max_frame_latency(defaults.max_frame_latency);
            }
            ui::VIDEO_ROW_QUALITY_PRESET => self.apply_quality_preset(QualityPreset::Low),
            ui::VIDEO_ROW_FPS_CAP => self.set_fps_cap(defaults.fps_cap),
            ui::VIDEO_ROW_PHYSICS_FPS => self.set_physics_msec(defaults.physics_msec),
            ui::VIDEO_ROW_BRIGHTNESS => self.set_gamma(defaults.gamma),
            ui::VIDEO_ROW_ANTI_ALIASING => {
                self.video.msaa_samples = defaults.msaa_samples;
                self.video.fxaa = defaults.fxaa;
                self.video.smaa = defaults.smaa;
                self.video.taa = defaults.taa;
                self.render_command(RenderCommand::SetMsaa(self.video.msaa_samples));
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_TEXTURE_FILTER => {
                self.video.texture_filter = defaults.texture_filter;
                self.render_command(RenderCommand::SetTextureFilter(self.video.texture_filter));
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_DETAIL_TEXTURES => {
                self.video.detail_textures = defaults.detail_textures;
                self.video.detail_texture_fade = defaults.detail_texture_fade;
                self.video.detail_texture_fade_distance = defaults.detail_texture_fade_distance;
                self.render_command(RenderCommand::SetDetailTextures(self.video.detail_textures));
                self.render_command(RenderCommand::SetDetailTextureFade {
                    enabled: self.video.detail_texture_fade,
                    distance: self.video.detail_texture_fade_distance,
                });
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_PVS => {
                self.video.pvs_mode = defaults.pvs_mode;
                self.render_command(RenderCommand::SetPvsMode(self.video.pvs_mode));
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_DISTANCE_CULL => {
                self.set_distance_cull_scale(defaults.distance_cull_scale)
            }
            ui::VIDEO_ROW_GPU_DRIVEN => {
                self.video.gpu_driven = defaults.gpu_driven;
                self.sync_gpu_visibility();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_HIZ => {
                self.video.hiz_occlusion = defaults.hiz_occlusion;
                self.sync_gpu_visibility();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_ENTITY_AMBIENT_LIGHTING => {
                self.video.entity_ambient_lighting = defaults.entity_ambient_lighting;
                self.sync_entity_ambient_lighting();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_PBR => {
                self.video.pbr = defaults.pbr;
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_ASSET_OVERRIDES => {
                self.video.allow_asset_overrides = defaults.allow_asset_overrides;
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_GEN_NORMAL_MAPS => {
                self.video.gen_normal_maps = defaults.gen_normal_maps;
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_DELUXE_MAPPING => {
                self.video.deluxe_mapping = defaults.deluxe_mapping;
                self.sync_pbr();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_DELUXE_SPECULAR => {
                self.video.deluxe_specular = defaults.deluxe_specular;
                self.sync_pbr();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_WORLD_LIGHTING => {
                self.video.world_lighting = defaults.world_lighting;
                self.video.vertex_lighting = defaults.vertex_lighting;
                self.sync_classic_world_lighting();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_VERTEX_LIGHTING => {
                self.video.vertex_lighting = defaults.vertex_lighting;
                self.sync_classic_world_lighting();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_LIGHTMAP_ONLY => {
                self.video.lightmap_only = defaults.lightmap_only;
                self.sync_classic_world_lighting();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_DYNAMIC_LIGHTS => {
                self.video.dynamic_lights = defaults.dynamic_lights;
                self.sync_dynamic_lighting();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_MAP_LIGHT_SIMULATION => {
                self.video.map_light_simulation = defaults.map_light_simulation;
                self.sync_map_light_simulation();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_FX_FPS => {
                self.video.fx_fps = defaults.fx_fps;
                self.mark_config_dirty();
                self.console_status = format!("FX FPS: {} HZ", self.video.fx_fps);
            }
            ui::VIDEO_ROW_MODERN_SABERS => {
                self.video.modern_sabers = defaults.modern_sabers;
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_SABER_MARKS => {
                self.video.saber_marks = defaults.saber_marks;
                if let Some(session) = self.game_session.as_mut() {
                    session.weapon_fx.set_saber_marks(self.video.saber_marks);
                }
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_EMISSIVE_AREA_LIGHTS => {
                self.video.emissive_area_lights = defaults.emissive_area_lights;
                self.sync_emissive_area_lights();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_AMBIENT_OCCLUSION => {
                self.video.ssao = defaults.ssao;
                self.video.static_bsp_ao = defaults.static_bsp_ao;
                self.video.static_bsp_ao_lightmap = defaults.static_bsp_ao_lightmap;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_VOXEL_PROBE_GI => {
                self.video.voxel_probe_gi = defaults.voxel_probe_gi;
                self.sync_voxel_probe_gi();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_DYNAMIC_SHADOWS => {
                self.video.dynamic_shadows = defaults.dynamic_shadows;
                self.video.cascaded_shadows = matches!(
                    self.video.dynamic_shadows,
                    DynamicShadowsMode::CascadedShadowMaps | DynamicShadowsMode::CascadedShadowMapsBevy
                );
                self.sync_cascaded_shadows();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_LOCAL_LIGHT_SHADOWS => {
                self.video.local_light_shadows = defaults.local_light_shadows;
                self.sync_local_light_shadows();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_CONTACT_SHADOWS => {
                self.video.contact_shadows = defaults.contact_shadows;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_SSR => {
                self.video.reflection_quality = defaults.reflection_quality;
                self.mark_config_dirty();
                self.console_status = format!(
                    "REFLECTION QUALITY: {} - PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART",
                    self.video.reflection_quality.label()
                );
            }
            ui::VIDEO_ROW_PLANAR_REFLECTIONS => {
                self.video.reflection_debug = defaults.reflection_debug;
                self.sync_post_effects();
            }
            ui::VIDEO_ROW_PLANAR_REFLECTION_DEBUG => {
                self.video.planar_reflection_debug = defaults.planar_reflection_debug;
                self.sync_planar_reflection_debug();
            }
            ui::VIDEO_ROW_HDR => {
                self.video.hdr = defaults.hdr;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_FLOAT_LIGHTMAP => {
                self.video.float_lightmap = defaults.float_lightmap;
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_TONE_MAPPING => {
                self.video.tone_mapping = defaults.tone_mapping;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_AUTO_EXPOSURE => {
                self.video.auto_exposure = defaults.auto_exposure;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_BLOOM => {
                self.video.bloom = defaults.bloom;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_MOTION_BLUR => {
                self.set_motion_blur_strength(defaults.motion_blur_strength)
            }
            ui::VIDEO_ROW_DEPTH_OF_FIELD => {
                self.set_depth_of_field_strength(defaults.depth_of_field_strength)
            }
            ui::VIDEO_ROW_DOF_QUALITY => {
                self.video.dof_quality = defaults.dof_quality;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_HALATION => {
                self.video.halation = defaults.halation;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_CHROMATIC_ABERRATION => {
                self.set_chromatic_aberration_strength(defaults.chromatic_aberration)
            }
            ui::VIDEO_ROW_VIGNETTE => {
                self.video.vignette = defaults.vignette;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_FILM_GRAIN => self.set_film_grain_strength(defaults.film_grain_strength),
            ui::VIDEO_ROW_COLOR_LUT => self.set_color_lut(defaults.color_lut),
            ui::VIDEO_ROW_LUT_STRENGTH => self.set_color_lut_strength(defaults.color_lut_strength),
            ui::VIDEO_ROW_WIREFRAME => {
                self.video.wireframe_mask = defaults.wireframe_mask;
                self.render_command(RenderCommand::SetWireframeMask(self.video.wireframe_mask));
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_CULL_DEBUG => {
                self.video.cull_debug = defaults.cull_debug;
                self.sync_cull_debug();
            }
            ui::VIDEO_ROW_DRAW_FPS => {
                self.video.draw_fps = defaults.draw_fps;
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_DEVELOPER_TOOLS => {
                self.video.developer_tools = defaults.developer_tools;
                if !self.video.developer_tools {
                    self.surface_inspector = None;
                    self.render_command(RenderCommand::ClearSurfaceInspection);
                }
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_PERF_TRACE => {
                self.video.perf_trace = defaults.perf_trace;
                self.render_command(RenderCommand::SetPerfTrace(self.video.perf_trace));
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_GPU_TIMINGS => {
                self.video.gpu_timings = defaults.gpu_timings;
                self.render_command(RenderCommand::SetGpuTimings(self.video.gpu_timings));
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_GHOUL2_SKINNING => {
                self.video.ghoul2_skinning = defaults.ghoul2_skinning;
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_GHOUL2_LOD_BIAS => {
                self.video.ghoul2_lod_bias = defaults.ghoul2_lod_bias;
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_GHOUL2_BATCH_DRAWS => {
                self.video.ghoul2_batch_draws = defaults.ghoul2_batch_draws;
                self.render_command(RenderCommand::SetGhoul2BatchDraws(
                    self.video.ghoul2_batch_draws,
                ));
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_GHOUL2_EARLY_CULL => {
                self.video.ghoul2_early_cull = defaults.ghoul2_early_cull;
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_RENDER_BACKEND => {
                self.video.renderer_backend = defaults.renderer_backend;
                self.mark_config_dirty();
                self.console_status = format!(
                    "RENDER BACKEND: {} (PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART)",
                    self.video.renderer_backend.label()
                );
            }
            ui::VIDEO_ROW_BAKED_AO_SAMPLES => {
                self.video.static_bsp_ao_samples = defaults.static_bsp_ao_samples;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_BAKED_AO_RESOLUTION => {
                self.video.static_bsp_ao_resolution = defaults.static_bsp_ao_resolution;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_BAKED_AO_STRENGTH => {
                self.video.static_bsp_ao_strength = defaults.static_bsp_ao_strength;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_BAKED_AO_RANGE => {
                self.video.static_bsp_ao_range = defaults.static_bsp_ao_range;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_BAKED_AO_CURRENT_CELL => {
                self.video.static_bsp_ao_current_cell = defaults.static_bsp_ao_current_cell;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            _ => {}
        }
    }

    fn reset_physics_field(&mut self, field: u8) {
        let defaults = VideoSettings::default();
        match field {
            PHYS_CLIENT_ENABLED => self.video.client_physics = defaults.client_physics,
            PHYS_RATE => self.video.client_physics_hz = defaults.client_physics_hz,
            PHYS_MAX_SUBSTEPS => self.video.client_physics_max_substeps = defaults.client_physics_max_substeps,
            PHYS_CCD => self.video.client_physics_ccd = defaults.client_physics_ccd,
            PHYS_SLEEPING => self.video.client_physics_sleeping = defaults.client_physics_sleeping,
            PHYS_RAGDOLLS => self.video.ragdolls = defaults.ragdolls,
            PHYS_RAGDOLL_MAX => self.video.ragdoll_max = defaults.ragdoll_max,
            PHYS_RAGDOLL_LIFETIME => self.video.ragdoll_lifetime = defaults.ragdoll_lifetime,
            PHYS_RAGDOLL_SELF_COLLISION => self.video.ragdoll_self_collision = defaults.ragdoll_self_collision,
            PHYS_PROPS => self.video.physics_props = defaults.physics_props,
            PHYS_PROP_MAX => self.video.physics_prop_max = defaults.physics_prop_max,
            PHYS_DEBRIS => self.video.physics_debris = defaults.physics_debris,
            PHYS_DEBRIS_MAX => self.video.physics_debris_max = defaults.physics_debris_max,
            PHYS_DEBRIS_LIFETIME => self.video.physics_debris_lifetime = defaults.physics_debris_lifetime,
            PHYS_PLAYER_PUSH => self.video.physics_player_push = defaults.physics_player_push,
            PHYS_WEAPON_IMPULSES => self.video.physics_weapon_impulses = defaults.physics_weapon_impulses,
            PHYS_EXPLOSION_IMPULSES => self.video.physics_explosion_impulses = defaults.physics_explosion_impulses,
            PHYS_FORCE_IMPULSES => self.video.physics_force_impulses = defaults.physics_force_impulses,
            PHYS_DEBUG_DRAW => self.video.physics_debug_draw = defaults.physics_debug_draw,
            PHYS_STATS => self.video.physics_stats = defaults.physics_stats,
            _ => return,
        }
        self.mark_config_dirty();
    }

    fn reset_environment_row(&mut self, row: usize) {
        let defaults = VideoSettings::default();
        match row {
            ui::ENV_ROW_FOG_MODE => {
                self.video.fog_mode = defaults.fog_mode;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_FOG_STRENGTH => self.set_fog_strength(defaults.fog_strength),
            ui::ENV_ROW_SUN_SOURCE => self.set_sun_override(false, false),
            ui::ENV_ROW_SUN_VISIBILITY => {
                self.video.sun_visibility = defaults.sun_visibility;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_SUN_YAW => {
                let yaw = self.map_sun_editor_values().map(|v| v.0).unwrap_or(defaults.sun_yaw);
                self.set_sun_yaw(yaw);
            }
            ui::ENV_ROW_SUN_PITCH => {
                let pitch = self.map_sun_editor_values().map(|v| v.1).unwrap_or(defaults.sun_pitch);
                self.set_sun_pitch(pitch);
            }
            ui::ENV_ROW_SUN_INTENSITY => {
                let intensity = self.map_sun_editor_values().map(|v| v.2).unwrap_or(defaults.sun_intensity);
                self.set_sun_intensity(intensity);
            }
            ui::ENV_ROW_SUN_COLOR => {
                let color = self.map_sun_editor_values().map(|v| v.3).unwrap_or(defaults.sun_color);
                self.set_sun_color(color);
            }
            ui::ENV_ROW_CLOUDS => {
                self.video.clouds = defaults.clouds;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_CLOUD_TYPE => {
                self.video.cloud_type = defaults.cloud_type;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_CLOUD_QUALITY => self.set_cloud_quality(defaults.cloud_quality),
            ui::ENV_ROW_CLOUD_COVERAGE => self.set_cloud_coverage(defaults.cloud_coverage),
            ui::ENV_ROW_CLOUD_HEIGHT => self.set_cloud_height(defaults.cloud_height),
            ui::ENV_ROW_CLOUD_THICKNESS => self.set_cloud_thickness(defaults.cloud_thickness),
            ui::ENV_ROW_WEATHER_WIND_SPEED => self.set_weather_wind_speed(defaults.weather_wind.speed),
            ui::ENV_ROW_WEATHER_WIND_DIRECTION => {
                self.set_weather_wind_direction(defaults.weather_wind.direction)
            }
            ui::ENV_ROW_WEATHER_GUST_STRENGTH => {
                self.set_weather_gust_strength(defaults.weather_wind.gust)
            }
            ui::ENV_ROW_WEATHER_DIRECTION_VARIATION => {
                self.set_weather_direction_variation(defaults.weather_wind.shift)
            }
            ui::ENV_ROW_CLOUD_SHADOWS => {
                self.video.cloud_shadows = defaults.cloud_shadows;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_CLOUD_RENDER_RESOLUTION => {
                self.video.cloud_render_resolution = defaults.cloud_render_resolution;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_CLOUD_TEMPORAL => {
                self.video.cloud_temporal = defaults.cloud_temporal;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_RAIN => {
                self.video.rain = defaults.rain;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_RAIN_INTENSITY => {
                self.video.rain_intensity = defaults.rain_intensity;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_FOOTPRINTS => {
                self.video.footprints = defaults.footprints;
                self.render_command(RenderCommand::SetFootprintMode(self.video.footprints));
                self.mark_config_dirty();
            }
            ui::ENV_ROW_GRASS => {
                self.video.grass = defaults.grass;
                if !self.video.grass || self.applied_grass {
                    self.render_command(RenderCommand::SetGrassEnabled(self.video.grass));
                }
                self.mark_config_dirty();
            }
            ui::ENV_ROW_OCEAN => {
                self.video.ocean = defaults.ocean;
                if !self.video.ocean || self.applied_ocean {
                    self.render_command(RenderCommand::SetOceanEnabled(self.video.ocean));
                }
                self.mark_config_dirty();
            }
            _ => {}
        }
    }

    /// Cloud tuning rows go back through `apply_cloud_tuning`, which owns the
    /// clamping and renderer sync for this block.
    fn reset_cloud_tuning_row(&mut self, row: usize) {
        let defaults = VideoSettings::default();
        match row {
            ui::CLOUD_ROW_TEMPORAL_DEPTH_FIX => {
                if self.video.cloud_temporal_depth_fix != defaults.cloud_temporal_depth_fix {
                    self.apply_cloud_tuning(row, true, None);
                }
            }
            ui::CLOUD_ROW_SKY_AMBIENT => {
                if self.video.cloud_sky_ambient != defaults.cloud_sky_ambient {
                    self.apply_cloud_tuning(row, true, None);
                }
            }
            ui::CLOUD_ROW_SHAPE_EVOLUTION => {
                if self.video.cloud_shape_evolution != defaults.cloud_shape_evolution {
                    self.apply_cloud_tuning(row, true, None);
                }
            }
            ui::CLOUD_ROW_TERRAIN_INTERACTION => {
                if self.video.cloud_terrain_interaction != defaults.cloud_terrain_interaction {
                    self.apply_cloud_tuning(row, true, None);
                }
            }
            ui::CLOUD_ROW_EMPTY_SKIP => {
                if self.video.cloud_empty_skip != defaults.cloud_empty_skip {
                    self.apply_cloud_tuning(row, true, None);
                }
            }
            ui::CLOUD_ROW_SHEAR => self.apply_cloud_tuning(row, false, Some(defaults.cloud_shear)),
            ui::CLOUD_ROW_BASE_VARIATION => {
                self.apply_cloud_tuning(row, false, Some(defaults.cloud_base_variation))
            }
            ui::CLOUD_ROW_AERIAL => {
                self.apply_cloud_tuning(row, false, Some(defaults.cloud_aerial))
            }
            ui::CLOUD_ROW_HISTORY_BLEND => {
                self.apply_cloud_tuning(row, false, Some(defaults.cloud_history_blend))
            }
            ui::CLOUD_ROW_MOTION_REJECT => {
                self.apply_cloud_tuning(row, false, Some(defaults.cloud_motion_reject))
            }
            ui::CLOUD_ROW_THICKNESS_VARIATION => {
                self.apply_cloud_tuning(row, false, Some(defaults.cloud_thickness_variation))
            }
            ui::CLOUD_ROW_SIZE => {
                self.apply_cloud_tuning(row, false, Some(defaults.cloud_size))
            }
            ui::CLOUD_ROW_HISTORY_DEPTH_REJECT => {
                if self.video.cloud_history_depth_reject != defaults.cloud_history_depth_reject {
                    self.apply_cloud_tuning(row, true, None);
                }
            }
            _ => {}
        }
    }

    fn reset_ocean_global(&mut self, field: u8) {
        let defaults = crate::ocean::OceanSettings::default();
        let settings = &mut self.video.ocean_settings;
        match field {
            OCEAN_MAP_SIZE => settings.map_size = defaults.map_size,
            OCEAN_MESH_QUALITY => settings.mesh_quality = defaults.mesh_quality,
            OCEAN_UPDATES => settings.updates_per_second = defaults.updates_per_second,
            OCEAN_ROUGHNESS => settings.roughness = defaults.roughness,
            OCEAN_NORMAL_STRENGTH => settings.normal_strength = defaults.normal_strength,
            OCEAN_WATER_COLOR => settings.water_color = defaults.water_color,
            OCEAN_FOAM_COLOR => settings.foam_color = defaults.foam_color,
            OCEAN_SEA_SPRAY => settings.sea_spray = defaults.sea_spray,
            OCEAN_WIND_FOAM => settings.wind_foam_streaks = defaults.wind_foam_streaks,
            9 => settings.optics.fog_color = defaults.optics.fog_color,
            10 => settings.optics.fog_distance = defaults.optics.fog_distance,
            11 => settings.optics.transparency = defaults.optics.transparency,
            12 => settings.optics.depth_darkening = defaults.optics.depth_darkening,
            13 => settings.optics.refraction = defaults.optics.refraction,
            14 => settings.optics.caustics = defaults.optics.caustics,
            15 => settings.optics.underwater_cull = defaults.optics.underwater_cull,
            _ => return,
        }
        self.commit_ocean_settings();
    }

    fn reset_ocean_cascade(&mut self, cascade: usize, field: u8) {
        if cascade >= crate::ocean::OCEAN_CASCADES {
            return;
        }
        let defaults = crate::ocean::OceanSettings::default();
        let source = defaults.cascades[cascade];
        let target = &mut self.video.ocean_settings.cascades[cascade];
        match field {
            0 => target.tile_length[0] = source.tile_length[0],
            1 => target.tile_length[1] = source.tile_length[1],
            2 => target.displacement_scale = source.displacement_scale,
            3 => target.normal_scale = source.normal_scale,
            4 => target.wind_speed = source.wind_speed,
            5 => target.wind_direction = source.wind_direction,
            6 => target.fetch_length = source.fetch_length,
            7 => target.swell = source.swell,
            8 => target.spread = source.spread,
            9 => target.detail = source.detail,
            10 => target.whitecap = source.whitecap,
            11 => target.foam_amount = source.foam_amount,
            _ => return,
        }
        self.commit_ocean_settings();
    }
}

fn draw_crosshair_preview(ui: &mut egui::Ui, crosshair: crate::ui::CrosshairSettings) {
    let width = theme::track_width(ui).max(180.0) + 96.0;
    let desired = egui::vec2(width, 118.0);
    let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let panel = rect.shrink(1.0);
    painter.rect_filled(panel, egui::CornerRadius::ZERO, theme::INSET);
    painter.rect_stroke(
        panel,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(1.0_f32, theme::LINE),
        egui::StrokeKind::Inside,
    );

    let view = panel.shrink2(egui::vec2(14.0, 12.0));
    let center = view.center();
    let guide = theme::HAIRLINE;
    painter.line_segment(
        [egui::pos2(view.left(), center.y), egui::pos2(view.right(), center.y)],
        egui::Stroke::new(1.0_f32, guide),
    );
    painter.line_segment(
        [egui::pos2(center.x, view.top()), egui::pos2(center.x, view.bottom())],
        egui::Stroke::new(1.0_f32, guide),
    );

    let font = egui::FontId::proportional(11.5);
    theme::glow_text(
        &painter,
        egui::pos2(view.left(), view.top() - 2.0),
        egui::Align2::LEFT_TOP,
        "LIVE PREVIEW",
        font.clone(),
        theme::TEXT_FAINT,
    );
    let detail = format!(
        "shape {}   size {:.0}   rgb {} {} {}",
        crosshair.style,
        crosshair.size,
        crosshair.color[0],
        crosshair.color[1],
        crosshair.color[2]
    );
    theme::glow_text(
        &painter,
        egui::pos2(view.right(), view.bottom() + 2.0),
        egui::Align2::RIGHT_BOTTOM,
        &detail,
        font,
        theme::TEXT_FAINT,
    );

    draw_crosshair_preview_shape(&painter, center, view, crosshair);
}

fn draw_crosshair_preview_shape(
    painter: &egui::Painter,
    center: egui::Pos2,
    bounds: egui::Rect,
    crosshair: crate::ui::CrosshairSettings,
) {
    if crosshair.style == 0 {
        theme::glow_text(
            painter,
            center,
            egui::Align2::CENTER_CENTER,
            "CROSSHAIR OFF",
            egui::FontId::proportional(13.0),
            theme::TEXT_DISABLED,
        );
        return;
    }

    let color = egui::Color32::from_rgba_premultiplied(
        crosshair.color[0],
        crosshair.color[1],
        crosshair.color[2],
        crosshair.color[3],
    );
    let max_preview = bounds.width().min(bounds.height()) - 26.0;
    let size = (crosshair.size.clamp(4.0, 96.0) * (2.0 / 3.0)).min(max_preview.max(8.0));
    let half = size * 0.5;
    let thickness = (size / 8.0).clamp(1.0, 4.0);
    let gap_max = (half - thickness * 0.5).max(0.5);
    let gap = (size / 8.0).clamp(0.5, gap_max);
    let arm = (half - gap).max(1.0);

    let rect = |x: f32, y: f32, w: f32, h: f32| {
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h)),
            egui::CornerRadius::ZERO,
            color,
        );
    };
    let hbar = |x: f32, y: f32, width: f32| rect(x, y - thickness * 0.5, width, thickness);
    let vbar = |x: f32, y: f32, height: f32| rect(x - thickness * 0.5, y, thickness, height);

    match crosshair.style {
        1 => {
            hbar(center.x - half, center.y, arm);
            hbar(center.x + gap, center.y, arm);
            vbar(center.x, center.y - half, arm);
            vbar(center.x, center.y + gap, arm);
        }
        2 => {
            let dot = (thickness * 1.6).clamp(2.0, 6.0);
            painter.circle_filled(center, dot * 0.5, color);
        }
        3 => {
            hbar(center.x - half, center.y, size);
            vbar(center.x, center.y - half, size);
        }
        4 => {
            hbar(center.x - half, center.y, arm);
            hbar(center.x + gap, center.y, arm);
            vbar(center.x, center.y - half, arm);
            vbar(center.x, center.y + gap, arm);
            let dot = thickness.max(2.0);
            painter.circle_filled(center, dot * 0.5, color);
        }
        5 => {
            let bracket_h = size * 0.7;
            let cap = (size * 0.2).max(thickness);
            vbar(center.x - half, center.y - bracket_h * 0.5, bracket_h);
            vbar(center.x + half, center.y - bracket_h * 0.5, bracket_h);
            hbar(center.x - half, center.y - bracket_h * 0.5, cap);
            hbar(center.x - half, center.y + bracket_h * 0.5, cap);
            hbar(center.x + half - cap, center.y - bracket_h * 0.5, cap);
            hbar(center.x + half - cap, center.y + bracket_h * 0.5, cap);
        }
        _ => {
            painter.rect_stroke(
                egui::Rect::from_center_size(center, egui::vec2(size, size)),
                egui::CornerRadius::ZERO,
                egui::Stroke::new(thickness, color),
                egui::StrokeKind::Middle,
            );
        }
    }
}
