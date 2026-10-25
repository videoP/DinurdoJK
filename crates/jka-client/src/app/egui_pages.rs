//! Menu pages added for the jaPRO cgame options: the Setup -> Camera page and
//! its in-game editor, Network and Vote pages, plus visibility and race HUD controls.

use super::egui_menu::jka_colored_text;
use super::egui_theme as theme;
use super::session_tools::FlagCarrier;
use super::*;
use crate::japro_cg;

/// Third-person settings as they were when the editor opened, so CANCEL (or
/// ESC) can put every value back.
pub(super) struct CameraEditState {
    pub(super) original: ThirdPersonSettings,
}

/// Text boxes and numbers of the Vote page's "call a vote" forms.
pub(super) struct VoteMenuState {
    map: String,
    poll: String,
    time_limit: u32,
    frag_limit: u32,
    capture_limit: u32,
}

impl Default for VoteMenuState {
    fn default() -> Self {
        Self { map: String::new(), poll: String::new(), time_limit: 20, frag_limit: 20, capture_limit: 8 }
    }
}

const CAMERA_RANGE: std::ops::RangeInclusive<f32> = 20.0..=500.0;

impl App {
    // ------------------------------------------------------------- camera --

    pub(super) fn egui_camera_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(
            ui,
            "CAMERA",
            "Third-person camera, field of view and how the view is smoothed.",
        );
        egui::ScrollArea::vertical()
            .id_salt("jka_camera_settings")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                theme::section(
                    ui,
                    "ADJUST IN GAME",
                    "Frame the shot with the mouse against the live game, then save it.",
                );
                let in_game = !self.front_end && self.game_session.is_some();
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(in_game, |ui| {
                        if theme::primary_button(ui, "ADJUST CAMERA").clicked() {
                            self.start_camera_edit();
                        }
                    });
                    ui.add_space(8.0);
                    if theme::ghost_button(ui, "RESET CAMERA").clicked() {
                        self.reset_camera_view();
                    }
                });
                theme::label(
                    ui,
                    theme::plain(
                        if in_game {
                            "Scroll to zoom, drag to move the camera, hold the right mouse button to aim it."
                        } else {
                            "Available once you are in a game or Solo Game."
                        },
                        11.5,
                        theme::TEXT_FAINT,
                    ),
                );

                theme::section(ui, "THIRD PERSON", "Stock OpenJK MP cg_thirdPerson camera cvars.");
                theme::row(
                    ui,
                    "Third person",
                    "Toggles cg_thirdPerson. With First-person saber / melee enabled, turning this off also keeps saber/melee in first person; special forced-camera states remain separate.",
                    theme::Reset::None,
                    |ui| {
                        if let Some(value) = theme::switch(ui, self.third_person.enabled) {
                            let _ = self.set_console_cvar("cg_thirdPerson", if value { "1" } else { "0" });
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
                            let _ = self.set_console_cvar("cg_fpls", if value { "1" } else { "0" });
                        }
                    },
                );
                macro_rules! camera_number {
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
                camera_number!("Range", "cg_thirdPersonRange: distance behind the player.", range, "cg_thirdPersonRange", 1.0);
                camera_number!("Orbit angle", "cg_thirdPersonAngle: yaw offset around the player in degrees.", angle, "cg_thirdPersonAngle", 1.0);
                camera_number!("Vertical offset", "cg_thirdPersonVertOffset: target height above the player origin.", vert_offset, "cg_thirdPersonVertOffset", 0.5);
                camera_number!("Horizontal offset", "cg_thirdPersonHorzOffset: left/right camera offset.", horz_offset, "cg_thirdPersonHorzOffset", 0.5);
                camera_number!("Pitch offset", "cg_thirdPersonPitchOffset: camera pitch offset in degrees.", pitch_offset, "cg_thirdPersonPitchOffset", 0.5);
                camera_number!("Camera damping", "cg_thirdPersonCameraDamp: OpenJK camera-position smoothing factor.", camera_damp, "cg_thirdPersonCameraDamp", 0.01);
                camera_number!("Target damping", "cg_thirdPersonTargetDamp: OpenJK camera-target smoothing factor.", target_damp, "cg_thirdPersonTargetDamp", 0.01);
                camera_number!("Player alpha", "cg_thirdPersonAlpha stock cvar. Preserved for the OpenJK player-rendering path.", alpha, "cg_thirdPersonAlpha", 0.01);
                theme::row(
                    ui,
                    "Special camera",
                    "cg_thirdPersonSpecialCam. In TaystJK this switches to CG_ThirdPersonActionCam during saber special moves and falls back to the normal third-person camera if that action cam cannot be used. The Rust action-camera branch is not ported yet, so this switch is currently inert.",
                    theme::Reset::None,
                    |ui| {
                        if let Some(value) = theme::switch(ui, self.third_person.special_cam) {
                            let _ = self.set_console_cvar("cg_thirdPersonSpecialCam", if value { "1" } else { "0" });
                        }
                    },
                );

                theme::section(ui, "FIELD OF VIEW", "Horizontal field of view on the 4:3 baseline.");
                theme::row(ui, "FOV", "cg_fov. Field of view while walking.", theme::Reset::None, |ui| {
                    let mut value = self.camera.cg_fov();
                    if ui
                        .add(
                            egui::DragValue::new(&mut value)
                                .range(crate::camera::MIN_CG_FOV..=crate::camera::MAX_CG_FOV)
                                .speed(0.5)
                                .max_decimals(1)
                                .update_while_editing(false),
                        )
                        .changed()
                    {
                        let _ = self.set_console_cvar("cg_fov", &value.to_string());
                    }
                });
                theme::row(ui, "Zoom FOV", "cg_zoomFov. Field of view while the zoom bind is held.", theme::Reset::None, |ui| {
                    let mut value = self.japro_zoom_fov;
                    if ui
                        .add(
                            egui::DragValue::new(&mut value)
                                .range(10.0..=90.0)
                                .speed(0.5)
                                .max_decimals(1)
                                .update_while_editing(false),
                        )
                        .changed()
                    {
                        let _ = self.set_console_cvar("cg_zoomFov", &value.to_string());
                    }
                });

            });
    }

    /// The full-screen camera editor: the live game with a toolbar over it.
    pub(super) fn egui_camera_editor(&mut self, root: &mut egui::Ui) {
        let surface = root.interact(
            root.max_rect(),
            egui::Id::new("jka_camera_edit_surface"),
            egui::Sense::click_and_drag(),
        );
        let mut changed = false;
        let panning = surface.dragged_by(egui::PointerButton::Primary);
        let aiming = surface.dragged_by(egui::PointerButton::Secondary);
        if panning || aiming {
            root.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if surface.hovered() {
            root.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }

        // Wheel: zoom. Multiplicative so a notch feels the same close up and far away.
        if surface.hovered() {
            let scroll = root.input(|input| input.smooth_scroll_delta.y);
            if scroll.abs() > f32::EPSILON {
                let range = self.third_person.range * (-scroll * 0.002).exp();
                self.third_person.range = range.clamp(*CAMERA_RANGE.start(), *CAMERA_RANGE.end());
                changed = true;
            }
        }
        let drag = surface.drag_delta();
        if panning && drag != egui::Vec2::ZERO {
            // The player follows the cursor: dragging right slides the view
            // right (the camera moves left), dragging down lowers the target.
            self.third_person.horz_offset = (self.third_person.horz_offset + drag.x * 0.25).clamp(-200.0, 200.0);
            self.third_person.vert_offset = (self.third_person.vert_offset + drag.y * 0.25).clamp(-80.0, 200.0);
            changed = true;
        }
        if aiming && drag != egui::Vec2::ZERO {
            // Dragging right swings the camera round to the right of the
            // player; dragging down lifts it to look down at them.
            let angle = self.third_person.angle + drag.x * 0.3;
            self.third_person.angle = (angle + 180.0).rem_euclid(360.0) - 180.0;
            self.third_person.pitch_offset = (self.third_person.pitch_offset + drag.y * 0.2).clamp(-80.0, 60.0);
            changed = true;
        }

        let (mut save, mut cancel, mut reset) = (false, false, false);
        if root.input(|input| input.key_pressed(egui::Key::Enter)) {
            save = true;
        }
        let third = self.third_person;
        egui::Area::new(egui::Id::new("jka_camera_edit_toolbar"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 14.0))
            .order(egui::Order::Foreground)
            .show(root.ctx(), |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_premultiplied(10, 15, 22, 236))
                    .stroke(egui::Stroke::new(1.0_f32, theme::LINE_STRONG))
                    .inner_margin(egui::Margin::symmetric(14, 9))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            theme::glow_label(ui, "CAMERA", 13.0, theme::TEXT);
                            ui.separator();
                            for (label, value) in [
                                ("Range", format!("{:.0}", third.range)),
                                ("Horizontal", format!("{:.0}", third.horz_offset)),
                                ("Vertical", format!("{:.0}", third.vert_offset)),
                                ("Angle", format!("{:.0}°", third.angle)),
                                ("Pitch", format!("{:.0}°", third.pitch_offset)),
                            ] {
                                theme::label(ui, theme::plain(label, 11.0, theme::TEXT_FAINT));
                                theme::label(ui, theme::plain(&value, 12.5, theme::TEXT));
                                ui.add_space(6.0);
                            }
                            ui.separator();
                            if theme::primary_button(ui, "SAVE").clicked() {
                                save = true;
                            }
                            if theme::ghost_button(ui, "RESET").clicked() {
                                reset = true;
                            }
                            if theme::ghost_button(ui, "CANCEL").clicked() {
                                cancel = true;
                            }
                        });
                        theme::label(
                            ui,
                            theme::plain(
                                "Scroll: zoom  ·  Drag: move the camera  ·  Hold right mouse: aim  ·  Enter: save  ·  Esc: cancel",
                                10.5,
                                theme::TEXT_FAINT,
                            ),
                        );
                    });
            });

        if reset {
            self.reset_camera_view();
            changed = true;
        }
        if save {
            self.finish_camera_edit(true);
        } else if cancel {
            self.finish_camera_edit(false);
        } else if changed {
            self.egui_repaint_requested = true;
        }
    }

    // ------------------------------------------------------------ network --

    pub(super) fn egui_network_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(ui, "NETWORK", "Connection, packet pacing and map/package autodownload policy.");
        egui::ScrollArea::vertical()
            .id_salt("jka_network_settings")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                theme::section(
                    ui,
                    "AUTOMATIC DOWNLOADS",
                    "When a server's map is missing, DinurdoJK asks before downloading. HTTP takes priority when the server advertises it; legacy JKA download is the fallback.",
                );
                theme::row(
                    ui,
                    "HTTP downloads",
                    "cl_allowHttpDownload. Allow PK3 downloads from TaystJK-compatible mvhttp/mvhttpurl endpoints. Preferred over legacy UDP when available.",
                    theme::Reset::None,
                    |ui| {
                        if let Some(enabled) = theme::switch(ui, self.network.allow_http_downloads) {
                            let _ = self.set_console_cvar("cl_allowHttpDownload", if enabled { "1" } else { "0" });
                        }
                    },
                );
                theme::row(
                    ui,
                    "Legacy server downloads",
                    "cl_allowDownload. Allow the stock Jedi Academy svc_download / nextdl transport. Used when HTTP is unavailable or an HTTP transfer fails.",
                    theme::Reset::None,
                    |ui| {
                        if let Some(enabled) = theme::switch(ui, self.network.allow_legacy_downloads) {
                            let _ = self.set_console_cvar("cl_allowDownload", if enabled { "1" } else { "0" });
                        }
                    },
                );

                theme::section(
                    ui,
                    "PACKET SETTINGS",
                    "Applied immediately; rate and snaps are also sent to the server you are on.",
                );
                theme::row(
                    ui,
                    "Rate",
                    "rate. Maximum bytes per second you ask the server to send you. The server may cap it (sv_maxRate).",
                    theme::Reset::None,
                    |ui| {
                        let mut value = self.network.rate;
                        if ui
                            .add(
                                egui::DragValue::new(&mut value)
                                    .range(1000..=90_000)
                                    .speed(250.0)
                                    .suffix(" B/s")
                                    .update_while_editing(false),
                            )
                            .changed()
                        {
                            let _ = self.set_console_cvar("rate", &value.to_string());
                        }
                    },
                );
                theme::row(
                    ui,
                    "Snapshots",
                    "snaps. Snapshots per second you ask the server for. Higher is smoother and needs more rate.",
                    theme::Reset::None,
                    |ui| {
                        let mut value = self.network.snaps;
                        if ui
                            .add(
                                egui::DragValue::new(&mut value)
                                    .range(1..=125)
                                    .speed(0.5)
                                    .suffix(" Hz")
                                    .update_while_editing(false),
                            )
                            .changed()
                        {
                            let _ = self.set_console_cvar("snaps", &value.to_string());
                        }
                    },
                );
                theme::row(
                    ui,
                    "Max packets",
                    "cl_maxpackets. Most packets per second the client sends. It cannot exceed your frame rate.",
                    theme::Reset::None,
                    |ui| {
                        let mut value = self.network.max_packets;
                        if ui
                            .add(
                                egui::DragValue::new(&mut value)
                                    .range(15..=1000)
                                    .speed(1.0)
                                    .suffix(" Hz")
                                    .update_while_editing(false),
                            )
                            .changed()
                        {
                            let _ = self.set_console_cvar("cl_maxpackets", &value.to_string());
                        }
                    },
                );
                theme::row(
                    ui,
                    "Packet duplication",
                    "cl_packetdup. Repeat your last N packets' movement commands in every packet so a lost packet does not cost you input. 0 sends each command once.",
                    theme::Reset::None,
                    |ui| {
                        let mut value = self.network.packet_dup;
                        if ui
                            .add(egui::DragValue::new(&mut value).range(0..=5).speed(0.1).update_while_editing(false))
                            .changed()
                        {
                            let _ = self.set_console_cvar("cl_packetdup", &value.to_string());
                        }
                    },
                );
                theme::row(
                    ui,
                    "Time nudge",
                    "cl_timeNudge. Shift the client's view of server time in milliseconds. Negative values show the world slightly ahead (less latency, more visible correction), positive values smooth out a jittery connection.",
                    theme::Reset::None,
                    |ui| {
                        let mut value = self.network.time_nudge;
                        if ui
                            .add(
                                egui::DragValue::new(&mut value)
                                    .range(-900..=900)
                                    .speed(1.0)
                                    .suffix(" ms")
                                    .update_while_editing(false),
                            )
                            .changed()
                        {
                            let _ = self.set_console_cvar("cl_timeNudge", &value.to_string());
                        }
                    },
                );
                ui.add_space(8.0);
                if theme::ghost_button(ui, "RESTORE DEFAULTS").clicked() {
                    let defaults = crate::net::NetworkSettings::default();
                    for (name, value) in [
                        ("rate", defaults.rate.to_string()),
                        ("snaps", defaults.snaps.to_string()),
                        ("cl_maxpackets", defaults.max_packets.to_string()),
                        ("cl_packetdup", defaults.packet_dup.to_string()),
                        ("cl_timeNudge", defaults.time_nudge.to_string()),
                    ] {
                        let _ = self.set_console_cvar(name, &value);
                    }
                }
            });
    }

    // --------------------------------------------------------------- vote --

    pub(super) fn egui_vote_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(ui, "VOTE", "Vote on the running vote or call a new one.");
        if !self.live_connected() {
            theme::banner(ui, "Voting is only available while connected to a server.", theme::WARNING);
            return;
        }
        let vote = self.game_session.as_ref().and_then(|session| session.vote_ui.clone());
        let japro = self.connected_server_mod() == crate::net::mod_support::ServerMod::Japro;
        let gametype = self.game_session.as_ref().map_or(0, |session| session.client_game.gametype());
        let players: Vec<(usize, String)> = self
            .game_session
            .as_ref()
            .map(|session| {
                (0..64)
                    .filter_map(|client| {
                        session
                            .client_game
                            .client_name(client)
                            .filter(|name| !name.is_empty())
                            .map(|name| (client, name))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut command: Option<String> = None;
        egui::ScrollArea::vertical()
            .id_salt("jka_vote_page")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                theme::section(ui, "CURRENT VOTE", "");
                match &vote {
                    Some(vote) => {
                        let title = match vote.param.as_deref().filter(|param| !param.is_empty()) {
                            Some(param) => format!("{}: {param}", vote.label),
                            None => vote.label.clone(),
                        };
                        ui.label(jka_colored_text(&title, 15.0, theme::TEXT));
                        ui.add_space(2.0);
                        theme::label(
                            ui,
                            theme::plain(
                                &format!(
                                    "{} s left  ·  Yes {}  ·  No {}",
                                    vote.seconds_left, vote.yes, vote.no
                                ),
                                12.0,
                                theme::TEXT_DIM,
                            ),
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if theme::primary_button(ui, "VOTE YES").clicked() {
                                command = Some("vote yes".into());
                            }
                            ui.add_space(8.0);
                            if theme::ghost_button(ui, "VOTE NO").clicked() {
                                command = Some("vote no".into());
                            }
                        });
                    }
                    None => {
                        theme::label(ui, theme::plain("No vote is running.", 12.5, theme::TEXT_FAINT));
                    }
                }

                theme::section(ui, "CALL A VOTE", "Servers decide which votes they allow; a refused vote is explained in the console.");
                if menu_call(ui, "Restart map", "Vote to restart the current map.") {
                    command = Some("callvote map_restart".into());
                }
                if menu_call(ui, "Next map", "Vote to move on to the next map in the rotation.") {
                    command = Some("callvote nextmap".into());
                }
                if menu_call(ui, "Warmup", "Vote to hold a warmup period before the next round.") {
                    command = Some("callvote g_doWarmup 1".into());
                }
                if japro {
                    if menu_call(ui, "Pause", "jaPRO: vote to pause the game.") {
                        command = Some("callvote pause".into());
                    }
                    if menu_call(ui, "Restart scores", "jaPRO: vote to reset every player's score.") {
                        command = Some("callvote score_restart".into());
                    }
                }

                theme::section(ui, "MAP", "Type a map name, or pick one from the maps this client knows.");
                ui.horizontal(|ui| {
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.vote_menu.map)
                            .desired_width(240.0)
                            .hint_text("mp/ffa3"),
                    );
                    let enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    let ready = crate::vote::map_vote(&self.vote_menu.map);
                    ui.add_enabled_ui(ready.is_some(), |ui| {
                        if theme::primary_button(ui, "CALL MAP VOTE").clicked() || (enter && ready.is_some()) {
                            command = ready.clone();
                        }
                    });
                });
                let needle = self.vote_menu.map.trim().to_ascii_lowercase();
                if !needle.is_empty() {
                    let matches: Vec<String> = self
                        .solo_maps
                        .iter()
                        .filter(|map| map.map_name.to_ascii_lowercase().contains(&needle) && !map.map_name.eq_ignore_ascii_case(&needle))
                        .take(10)
                        .map(|map| map.map_name.clone())
                        .collect();
                    ui.horizontal_wrapped(|ui| {
                        for name in matches {
                            if theme::chip(ui, &name, false).clicked() {
                                self.vote_menu.map = name;
                            }
                        }
                    });
                } else if self.solo_maps.is_empty() {
                    self.ensure_solo_map_catalog();
                }

                theme::section(ui, "GAME TYPE", "Vote to switch the server to another game type (the map restarts).");
                ui.horizontal_wrapped(|ui| {
                    for &(number, label) in crate::vote::GAME_TYPES {
                        if theme::chip(ui, label, i32::from(number) == gametype).clicked() {
                            command = Some(crate::vote::gametype_vote(number));
                        }
                    }
                });

                theme::section(ui, "LIMITS", "");
                let capture = matches!(gametype, 8 | 9);
                let limits: [(&str, &str, &str, u32); 3] = [
                    ("Time limit", "timelimit", "minutes", 0),
                    ("Frag limit", "fraglimit", "frags", 1),
                    ("Capture limit", "capturelimit", "captures", 2),
                ];
                for (label, kind, unit, slot) in limits {
                    if (slot == 2) != capture && slot != 0 {
                        // Frag limits apply outside capture-the-flag, capture limits inside it.
                        continue;
                    }
                    theme::row(ui, label, &format!("Vote to change the {unit} limit. 0 means no limit."), theme::Reset::None, |ui| {
                        let value = match slot {
                            0 => &mut self.vote_menu.time_limit,
                            1 => &mut self.vote_menu.frag_limit,
                            _ => &mut self.vote_menu.capture_limit,
                        };
                        ui.add(egui::DragValue::new(value).range(0..=999).speed(0.2).suffix(format!(" {unit}")));
                        ui.add_space(8.0);
                        if theme::ghost_button(ui, "CALL").clicked() {
                            command = Some(crate::vote::limit_vote(kind, *value));
                        }
                    });
                }

                theme::section(ui, "PLAYERS", "Vote to remove a player from the game or the server.");
                if players.is_empty() {
                    theme::label(ui, theme::plain("No players are listed yet.", 12.0, theme::TEXT_FAINT));
                }
                let own = self
                    .game_session
                    .as_ref()
                    .and_then(|session| session.current_snapshot.as_ref())
                    .and_then(|snapshot| snapshot.player_state.field_i32("clientNum"))
                    .and_then(|client| usize::try_from(client).ok());
                for (client, name) in &players {
                    ui.horizontal(|ui| {
                        ui.set_min_height(26.0);
                        theme::label(ui, theme::plain(&format!("{client:>2}"), 11.5, theme::TEXT_FAINT));
                        ui.add_space(6.0);
                        ui.label(jka_colored_text(name, 13.0, theme::TEXT));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let is_self = Some(*client) == own;
                            ui.add_enabled_ui(!is_self, |ui| {
                                if theme::ghost_button(ui, "KICK").clicked() {
                                    command = Some(crate::vote::clientkick_vote(*client));
                                }
                                if japro && theme::ghost_button(ui, "FORCE SPECTATE").clicked() {
                                    command = Some(crate::vote::forcespec_vote(*client));
                                }
                            });
                        });
                    });
                }

                if japro {
                    theme::section(ui, "POLL", "jaPRO: ask everyone a yes/no question.");
                    ui.horizontal(|ui| {
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut self.vote_menu.poll)
                                .desired_width(320.0)
                                .hint_text("Question"),
                        );
                        let enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                        let ready = crate::vote::poll_vote(&self.vote_menu.poll);
                        ui.add_enabled_ui(ready.is_some(), |ui| {
                            if theme::primary_button(ui, "CALL POLL").clicked() || (enter && ready.is_some()) {
                                command = ready.clone();
                            }
                        });
                    });
                }
            });

        if let Some(command) = command {
            self.forward_command_to_server(&command);
        }
    }

    // --------------------------------------------------------- cosmetics --

    fn set_style_bit(&mut self, bit: u32, on: bool) {
        let mut style = self.japro_cg.style_player;
        if on {
            style |= bit;
        } else {
            style &= !bit;
        }
        let _ = self.set_console_cvar("cg_stylePlayer", &style.to_string());
    }

    // --------------------------------------------------------- HUD options --

    /// jaPRO HUD controls live with the rest of Setup -> Interface / HUD.
    pub(super) fn egui_japro_hud_settings(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "RACE FORMATTING", "Component selection is above. These options change the selected readout's format.");
        if self.japro_cg.race_timer != 0 {
            theme::row(ui, "Timer format", "cg_raceTimer format for the enabled timer.", theme::Reset::None, |ui| {
                let mut value = self.japro_cg.race_timer;
                let choices = [(1,"Time"),(2,"Time + speeds"),(3,"Milliseconds")];
                egui::ComboBox::from_id_salt("race_timer_format").selected_text(choices.iter().find(|(id,_)| *id == value).map(|(_,label)| *label).unwrap_or("Time"))
                    .show_ui(ui, |ui| { for (id,label) in choices { ui.selectable_value(&mut value,id,label); } });
                if value != self.japro_cg.race_timer { let _ = self.set_console_cvar("cg_raceTimer", &value.to_string()); }
            });
        }
        macro_rules! race_number {
            ($label:literal, $tip:literal, $field:ident, $cvar:literal, $range:expr, $speed:expr) => {
                theme::row(ui, $label, $tip, theme::Reset::None, |ui| {
                    let mut value = self.japro_cg.$field;
                    if ui
                        .add(egui::DragValue::new(&mut value).range($range).speed($speed).max_decimals(2).update_while_editing(false))
                        .changed()
                    {
                        let _ = self.set_console_cvar($cvar, &value.to_string());
                    }
                });
            };
        }
        race_number!("Start speed goal", "cg_startGoal. The start speed readout turns green at or above this speed. 0 disables the colour.", start_goal, "cg_startGoal", 0.0..=5000.0, 5.0);

        theme::section(
            ui,
            "NATIVE READOUT FORMATTING",
            "Formatting for jaPRO readouts. Enable each component with its dropdown above; position and scale it with HUD Edit.",
        );
        for (index, label) in crate::speedometer::TOGGLE_LABELS.iter().enumerate() {
            if ![8,9,11,12,13,15].contains(&index) { continue; }
            theme::row(
                ui,
                label,
                &format!("cg_speedometer bit {index}."),
                theme::Reset::None,
                |ui| {
                    let flags = self.japro_cg.speedometer.flags;
                    if theme::switch(ui, flags & (1 << index) != 0).is_some() {
                        // The 256/512 and 4096/8192 pairs are radio groups: turning one on clears its partner.
                        let value = crate::speedometer::toggle(flags, index);
                        let _ = self.set_console_cvar("cg_speedometer", &value.to_string());
                    }
                },
            );
        }
        macro_rules! speedometer_number {
            ($label:literal, $tip:literal, $field:ident, $cvar:literal, $range:expr, $speed:expr) => {
                theme::row(ui, $label, $tip, theme::Reset::None, |ui| {
                    let mut value = self.japro_cg.speedometer.$field;
                    if ui
                        .add(egui::DragValue::new(&mut value).range($range).speed($speed).max_decimals(2).update_while_editing(false))
                        .changed()
                    {
                        let _ = self.set_console_cvar($cvar, &value.to_string());
                    }
                });
            };
        }
        speedometer_number!("Jumps stored", "cg_speedometerJumps. How many pre-speed jumps the jumps array keeps.", jumps, "cg_speedometerJumps", 0..=511, 1.0);
        speedometer_number!("Jump speed goal", "cg_jumpGoal. The first jump's pre-speed turns green at or above this speed. 0 disables the colour.", jump_goal, "cg_jumpGoal", 0.0..=5000.0, 5.0);

        theme::section(
            ui,
            "LAGOMETER",
            "The jaPRO network graph (cg_lagometer): snapshot ping and drops below, interpolation above. Not shown against the local server.",
        );
        theme::row(ui, "Lagometer", "Select one network graph style, or Off. An enabled graph is independently editable in HUD Edit Mode.", theme::Reset::None, |ui| {
            let mut value = self.japro_cg.lagometer.mode;
            let choices = [(0,"Off"),(1,"Graph"),(2,"Graph + numbers"),(3,"Numbers"),(4,"Numbers + loss")];
            egui::ComboBox::from_id_salt("lagometer_component").selected_text(choices.iter().find(|(id,_)| *id == value).map(|(_,label)| *label).unwrap_or("Off"))
                .show_ui(ui, |ui| { for (id,label) in choices { ui.selectable_value(&mut value,id,label); } });
            if value != self.japro_cg.lagometer.mode { let _ = self.set_console_cvar("cg_lagometer", &value.to_string()); }
        });
        theme::row(
            ui,
            "Warning delay (commands)",
            "cl_commandsize. The connection-interrupted warning looks this many usercmds back: smaller warns sooner. 64 is jaPRO's default.",
            theme::Reset::None,
            |ui| {
                let mut value = self.japro_cg.lagometer.command_size;
                if ui
                    .add(egui::DragValue::new(&mut value).range(4..=512).speed(1.0).update_while_editing(false))
                    .changed()
                {
                    let _ = self.set_console_cvar("cl_commandsize", &value.to_string());
                }
            },
        );

    }

    // ------------------------------------------------------- game options --

    /// jaPRO player visibility and spectating rows on Setup -> JAPRO.
    pub(super) fn egui_japro_game_settings(&mut self, ui: &mut egui::Ui) {
        theme::section(
            ui,
            "PLAYER VISIBILITY",
            "How other players in duels and race mode are drawn (jaPRO cg_stylePlayer).",
        );
        for &(bit, label, tip) in japro_cg::STYLE_TOGGLES {
            theme::row(ui, label, tip, theme::Reset::None, |ui| {
                if let Some(on) = theme::switch(ui, self.japro_cg.style_bit(bit)) {
                    self.set_style_bit(bit, on);
                }
            });
        }
        for &(bit, inverted, label, tip) in japro_cg::GHOST_TOGGLES {
            theme::row(ui, label, tip, theme::Reset::None, |ui| {
                let set = self.japro_cg.style_bit(bit);
                if let Some(on) = theme::switch(ui, set != inverted) {
                    self.set_style_bit(bit, on != inverted);
                }
            });
        }

        theme::section(ui, "SPECTATING", "Options while you watch other players.");
        theme::row(
            ui,
            "Follow the fastest player",
            "cg_specFollowFastest. While spectating on a server, keep following whoever is moving fastest. It only cuts to someone clearly faster who stays ahead for a moment, and never more than once every couple of seconds.",
            theme::Reset::None,
            |ui| {
                if let Some(on) = theme::switch(ui, self.japro_cg.follow_fastest) {
                    let _ = self.set_console_cvar("cg_specFollowFastest", if on { "1" } else { "0" });
                }
            },
        );
    }

    /// Companion window selector on the Join page. This is the single control
    /// for opening/closing the companion and choosing its optional second POV.
    pub(super) fn egui_companion_window_selector(&mut self, ui: &mut egui::Ui) {
        let enabled = self.companion_enabled();
        let current_target = self.companion_scene_target;
        let targets = self.companion_spectator_targets();
        let selected_text = if !enabled {
            "Off".to_owned()
        } else {
            current_target
                .map(|client| {
                    crate::logging::strip_jka_colors(&self.companion_scene_target_name(client))
                })
                .unwrap_or_else(|| "Dashboard".to_owned())
        };

        #[derive(Clone, Copy, PartialEq, Eq)]
        enum CompanionChoice {
            Off,
            Dashboard,
            Player(i32),
        }

        let current_choice = if !enabled {
            CompanionChoice::Off
        } else {
            current_target.map_or(CompanionChoice::Dashboard, CompanionChoice::Player)
        };
        let mut next_choice = current_choice;
        theme::row(
            ui,
            "Companion window",
            "Open the second-monitor companion as a passive dashboard, close it, or while spectating use a player as its second POV.",
            theme::Reset::None,
            |ui| {
                egui::ComboBox::from_id_salt("companion_window_mode")
                    .selected_text(selected_text)
                    .width(190.0)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(next_choice == CompanionChoice::Off, "Off")
                            .clicked()
                        {
                            next_choice = CompanionChoice::Off;
                        }
                        if ui
                            .selectable_label(next_choice == CompanionChoice::Dashboard, "Dashboard")
                            .clicked()
                        {
                            next_choice = CompanionChoice::Dashboard;
                        }
                        for (client, name) in &targets {
                            let label = crate::logging::strip_jka_colors(name);
                            if ui
                                .selectable_label(next_choice == CompanionChoice::Player(*client), label)
                                .clicked()
                            {
                                next_choice = CompanionChoice::Player(*client);
                            }
                        }
                    });
            },
        );

        if next_choice != current_choice {
            match next_choice {
                CompanionChoice::Off => self.set_companion_enabled(false),
                CompanionChoice::Dashboard => {
                    self.set_companion_enabled(true);
                    self.set_companion_scene_target(None);
                }
                CompanionChoice::Player(client) => {
                    self.set_companion_scene_target(Some(client));
                }
            }
        }
    }

    /// Quick spectator/demo actions on the Join page while watching players.
    pub(super) fn egui_spectator_actions(&mut self, ui: &mut egui::Ui) {
        let live_spectator = self.is_spectating_live();
        let demo = self.demo_playback_active();
        if !live_spectator && !demo {
            return;
        }
        theme::section(
            ui,
            if demo { "DEMO VIEW" } else { "SPECTATING" },
            if demo {
                "Options while watching this demo."
            } else {
                "Spectator presentation and automatic follow options."
            },
        );
        theme::row(
            ui,
            "Player outlines",
            "cg_specHud. Outline other living players through walls. Team players are red/blue; neutral/FFA players are yellow.",
            theme::Reset::None,
            |ui| {
                if let Some(on) = theme::switch(ui, self.japro_cg.spec_hud) {
                    let _ = self.set_console_cvar("cg_specHud", if on { "1" } else { "0" });
                }
            },
        );
        if !live_spectator {
            return;
        }
        let capture = self
            .game_session
            .as_ref()
            .is_some_and(|session| matches!(session.client_game.gametype(), 8 | 9));
        if capture {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if theme::ghost_button(ui, "FOLLOW RED FLAG").clicked() {
                    self.follow_flag_carrier(FlagCarrier::Red);
                }
                if theme::ghost_button(ui, "FOLLOW BLUE FLAG").clicked() {
                    self.follow_flag_carrier(FlagCarrier::Blue);
                }
            });
        }
        theme::row(
            ui,
            "Face direction of motion",
            "cg_specCameraMotion. In spectator Third person, use the followed player's current presented horizontal velocity as camera yaw, matching TaystJK cg_thirdPersonAngle -1 behavior.",
            theme::Reset::None,
            |ui| {
                ui.add_enabled_ui(self.spectator_camera.mode == crate::camera::SpectatorCameraMode::ThirdPerson, |ui| {
                    if let Some(on) = theme::switch(ui, self.spectator_camera.motion_direction) {
                        let _ = self.set_console_cvar("cg_specCameraMotion", if on { "1" } else { "0" });
                    }
                });
            },
        );
        theme::row(
            ui,
            "Keep following the fastest",
            "cg_specFollowFastest. Automatically follow whoever is moving fastest, without switching back and forth.",
            theme::Reset::None,
            |ui| {
                if let Some(on) = theme::switch(ui, self.japro_cg.follow_fastest) {
                    let _ = self.set_console_cvar("cg_specFollowFastest", if on { "1" } else { "0" });
                }
            },
        );
    }
}

/// A left-aligned action row: title and explanation with a CALL button.
fn menu_call(ui: &mut egui::Ui, title: &str, detail: &str) -> bool {
    let mut clicked = false;
    theme::row(ui, title, detail, theme::Reset::None, |ui| {
        clicked = theme::ghost_button(ui, "CALL VOTE").clicked();
    });
    clicked
}
