//! Controls.
use crate::app::egui_menu::{binding_slot, keybinds, theme, title_case, App};

impl App {
    /// Binding capture status is an overlay, not layout content. A normal
    /// banner here moved every binding row as soon as it was clicked, making
    /// the control the player was interacting with jump under the pointer.
    pub(in crate::app) fn egui_binding_capture_overlay(&self, ctx: &egui::Context) {
        egui::Area::new(egui::Id::new("jka_binding_capture_status"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -18.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_premultiplied(12, 17, 24, 242))
                    .stroke(egui::Stroke::new(1.0_f32, theme::WARNING))
                    .inner_margin(egui::Margin::symmetric(12, 7))
                    .show(ui, |ui| {
                        theme::label(
                            ui,
                            theme::plain(
                                "Press any key, mouse button or wheel direction…  ESC cancels.",
                                12.5,
                                theme::WARNING,
                            ),
                        );
                    });
            });
    }

    pub(in crate::app::egui_menu) fn egui_controls_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(
            ui,
            "CONTROLS",
            "Base JKA, spectator and mouse bindings. Spectator binds override normal keys only while spectating.",
        );

        if self.controls_waiting_for_key {
            self.egui_binding_capture_overlay(ui.ctx());
        }

        // One tab per action group, then Mouse, styled like the Setup tab strip.
        let mut groups: Vec<&str> = Vec::new();
        for action in keybinds::CONTROL_ACTIONS {
            if !groups.contains(&action.group) {
                groups.push(action.group);
            }
        }
        if !keybinds::SPECTATOR_CONTROL_ACTIONS.is_empty() {
            groups.push("Spectate");
        }
        let mouse_tab = groups.len();
        self.controls_section = self.controls_section.min(mouse_tab);
        let mut picked_tab = None;
        egui::Frame::new().fill(theme::SURFACE_ALT).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (index, label) in groups.iter().copied().chain(["Mouse"]).enumerate() {
                    let label = label.to_uppercase();
                    let width = 24.0 + label.len() as f32 * 9.0;
                    if theme::nav_item(
                        ui,
                        &label,
                        12.5,
                        width,
                        theme::TAB_BAR_H,
                        index == self.controls_section,
                    )
                    .clicked()
                    {
                        picked_tab = Some(index);
                    }
                }
            });
        });
        if let Some(index) = picked_tab {
            if index != self.controls_section {
                self.controls_section = index;
                self.controls_waiting_for_key = false;
                self.publish_ui();
            }
        }
        ui.add_space(4.0);

        let mut rebind = None;
        let mut clear = None;
        let mut restore_defaults = false;
        let section = self.controls_section;
        egui::ScrollArea::vertical()
            .id_salt(("jka_controls_scroll", section))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if section == mouse_tab {
                    self.egui_mouse_controls(ui);
                    return;
                }
                let group = groups[section];
                let spectator_group = group == "Spectate";
                theme::section(
                    ui,
                    &group.to_uppercase(),
                    if spectator_group {
                        "Overrides used only while spectating. Unassigned keys inherit the normal binding."
                    } else {
                        ""
                    },
                );

                if spectator_group {
                    for (index, action) in keybinds::SPECTATOR_CONTROL_ACTIONS.iter().enumerate() {
                        let selection = keybinds::spectator_selection(index);
                        let waiting = self.controls_waiting_for_key && self.controls_selected == selection;
                        let binding = self.bindings.display_for_spectator_command(action.command);
                        let tip = format!(
                            "Spectator override for the \"{}\" command. Right-click to inherit the normal bind again.",
                            action.command
                        );
                        theme::row(ui, &title_case(action.label), &tip, theme::Reset::None, |ui| {
                            let (text, color) = if waiting {
                                ("PRESS A KEY…".to_owned(), theme::WARNING)
                            } else if binding == "INHERIT NORMAL" {
                                ("Inherit normal".to_owned(), theme::TEXT_DISABLED)
                            } else {
                                (binding.clone(), theme::ACCENT)
                            };
                            let response = binding_slot(ui, &text, color, waiting);
                            if response.clicked() {
                                rebind = Some(selection);
                            }
                            if response.secondary_clicked() {
                                clear = Some(selection);
                            }
                        });
                    }
                } else {
                    for (index, action) in keybinds::CONTROL_ACTIONS.iter().enumerate() {
                        if action.group != group {
                            continue;
                        }
                        let waiting = self.controls_waiting_for_key && self.controls_selected == index;
                        let binding = self.bindings.display_for_command(action.command);
                        let tip = format!("Bound to the \"{}\" command.", action.command);
                        theme::row(ui, &title_case(action.label), &tip, theme::Reset::None, |ui| {
                            let (text, color) = if waiting {
                                ("PRESS A KEY…".to_owned(), theme::WARNING)
                            } else if binding == "UNBOUND" {
                                ("Unbound".to_owned(), theme::TEXT_DISABLED)
                            } else {
                                (binding.clone(), theme::ACCENT)
                            };
                            let response = binding_slot(ui, &text, color, waiting);
                            if response.clicked() {
                                rebind = Some(index);
                            }
                            if response.secondary_clicked() {
                                clear = Some(index);
                            }
                        });
                    }
                }

                theme::section(ui, "DEFAULTS", "");
                theme::row(
                    ui,
                    "Default bindings",
                    "Replaces normal bindings with the stock jaPRO/JKA multiplayer layout and restores DinurdoJK spectator defaults.",
                    theme::Reset::None,
                    |ui| {
                        let armed_id = ui.id().with("restore_default_binds");
                        let armed = ui.ctx().data(|data| data.get_temp::<bool>(armed_id)).unwrap_or(false);
                        let label = if armed { "CLICK AGAIN TO CONFIRM" } else { "RESTORE DEFAULTS" };
                        let response = theme::ghost_button(ui, label);
                        if response.clicked() {
                            ui.ctx().data_mut(|data| data.insert_temp(armed_id, !armed));
                            restore_defaults = armed;
                        } else if armed && !response.hovered() {
                            // Moving away cancels, so a later single click can't confirm.
                            ui.ctx().data_mut(|data| data.insert_temp(armed_id, false));
                        }
                    },
                );
            });

        if restore_defaults {
            self.bindings = keybinds::Bindings::default();
            self.controls_waiting_for_key = false;
            self.refresh_bound_state();
            self.mark_config_dirty();
            self.publish_ui();
        } else if let Some(index) = rebind {
            self.controls_selected = index;
            self.controls_waiting_for_key = true;
            self.publish_ui();
        } else if let Some(index) = clear {
            self.controls_selected = index;
            self.unbind_selected_control();
        }
    }

    pub(in crate::app::egui_menu) fn egui_mouse_controls(&mut self, ui: &mut egui::Ui) {
        theme::section(
            ui,
            "MOUSE",
            "Mouse look, raw-input scaling and client-side input latency controls.",
        );
        macro_rules! mouse_number {
            ($label:literal, $tip:literal, $field:ident, $cvar:literal, $speed:expr, $decimals:expr) => {
                theme::row(ui, $label, $tip, theme::Reset::None, |ui| {
                    let mut value = self.mouse_input.$field;
                    if ui
                        .add(
                            egui::DragValue::new(&mut value)
                                .speed($speed)
                                .max_decimals($decimals)
                                .update_while_editing(false),
                        )
                        .changed()
                    {
                        let _ = self.set_console_cvar($cvar, &value.to_string());
                    }
                });
            };
        }
        mouse_number!(
            "Sensitivity",
            "sensitivity. TaystJK mouse sensitivity multiplier; stock default is 5. Raw input does not bypass this scaling.",
            sensitivity,
            "sensitivity",
            0.1,
            4
        );
        mouse_number!(
            "Yaw scale",
            "m_yaw. Horizontal mouse scale applied after sensitivity/acceleration; TaystJK default is 0.022.",
            yaw,
            "m_yaw",
            0.001,
            6
        );
        mouse_number!(
            "Pitch scale",
            "m_pitch. Vertical mouse scale applied after sensitivity/acceleration; TaystJK default is 0.022. A negative value inverts vertical look.",
            pitch,
            "m_pitch",
            0.001,
            6
        );
        mouse_number!(
            "Mouse acceleration",
            "cl_mouseAccel. TaystJK legacy style-0 acceleration; 0 disables it. With subframe input, DinurdoJK uses the raw-event interval as the acceleration timebase rather than waiting for a client frame.",
            accel,
            "cl_mouseAccel",
            0.01,
            4
        );
        theme::row(
            ui,
            "Late-latched view",
            "cl_input_latelatch. Experimental A/B switch. Subframe input is always enabled; the render thread resamples the newest real view orientation at the latest point that is still coherent with camera-dependent work. No mouse prediction or extra physics ticks.",
            theme::Reset::None,
            |ui| {
                if let Some(enabled) = theme::switch(ui, self.video.input_latelatch) {
                    self.set_input_latelatch(enabled);
                    self.egui_repaint_requested = true;
                }
            },
        );

        theme::section(
            ui,
            "MOUSE - ADVANCED LATENCY (A/B)",
            "Windows scheduling experiments. These do not alter JKA movement or networking.",
        );
        theme::row(
            ui,
            "1 ms Windows timer",
            "cl_timerResolution1ms. Windows only: requests timeBeginPeriod(1) while enabled, then pairs it with timeEndPeriod(1) when disabled/shutting down. This can improve timeout/sleep wake precision (for example capped frame pacing), but raw mouse input already wakes the event loop immediately, so it is not expected to reduce raw mouse-event wake latency. May increase power use.",
            theme::Reset::None,
            |ui| {
                if !cfg!(windows) {
                    ui.add_enabled(false, egui::Label::new("Windows only"));
                    return;
                }
                if let Some(enabled) = theme::switch(ui, self.video.timer_resolution_1ms) {
                    if let Err(error) = self.set_timer_resolution_1ms(enabled) {
                        self.console_status = format!("1 MS WINDOWS TIMER FAILED: {error}");
                        self.push_console_line(format!("^1{}", self.console_status));
                        self.publish_ui();
                    }
                    self.egui_repaint_requested = true;
                }
            },
        );
    }
}
