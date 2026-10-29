//! Declarative command handlers.
use crate::app::{session_tools, App, ChatMode, OverlayMode};

use super::{CommandDef, CommandResult, CommandScope};
commands! {
    cmd_voicechat ["voicechat", ] ("voicechat", "Open TaystJK VGS when connected to a jaPRO server and ui_vgs is enabled.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.open_vgs_voicechat();
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_ui_opensiegemenu ["ui_opensiegemenu", ] ("ui_opensiegemenu <menu>", "Open a stock in-game Siege UI surface (currently ingame_objectives).", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb, menu] if menu.eq_ignore_ascii_case("ingame_objectives") => {
                self.open_siege_objectives_menu();
                CommandResult::Handled
            }
            [_verb, ..] => {
                // TaystJK's UI_OpenSiegeMenu_f simply fails to activate an unknown
                // menu name. Keep unsupported legacy UI names as a handled no-op
                // instead of forwarding a client UI command to the server.
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_followfastest ["followFastest", ] ("followFastest", "Spectate whoever is moving fastest right now.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.follow_fastest_now();
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_followredflag ["followRedFlag", ] ("followRedFlag", "Spectate the red flag carrier.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.follow_flag_carrier(session_tools::FlagCarrier::Red);
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_followblueflag ["followBlueFlag", ] ("followBlueFlag", "Spectate the blue flag carrier.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.follow_flag_carrier(session_tools::FlagCarrier::Blue);
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_speedometer ["speedometer", ] ("speedometer [option]", "List the cg_speedometer options, or toggle option <num> (see /speedometer).", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb, rest @ ..] => {
                self.speedometer_command(rest);
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_amtelegui ["amtelegui", ] ("amtelegui", "Open persistent map-scoped teleport bookmarks.", CommandScope::Always) |self, command| {
        match command.split_whitespace().collect::<Vec<_>>().as_slice() {
            [_verb] => { self.open_telemarks(); CommandResult::Handled }
            _ => CommandResult::Unhandled,
        }
    }

    cmd_trailmenu ["trailMenu", ] ("trailMenu", "Open the Strafe Trails GUI.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.open_strafe_trails();
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_loadtrail ["loadTrail", ] ("loadTrail <name> [slot 0..31]", "Load a jaPRO strafetrails/<name>.cfg asynchronously: /loadTrail <name> [slot].", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb, trail] => {
                match self.strafe_trails.request_load(
                    &self.base,
                    self.game.as_deref(),
                    trail,
                    crate::strafe_trail::DEFAULT_SLOT,
                ) {
                    Ok(()) => self.push_console_line(format!(
                        "^3loadTrail:^7 queued {trail} (slot {})",
                        crate::strafe_trail::DEFAULT_SLOT
                    )),
                    Err(error) => self.push_console_line(format!("^1loadTrail:^7 {error}")),
                }
                CommandResult::Handled
            }
            [_verb, trail, slot] => {
                let slot = slot
                    .parse::<u8>()
                    .ok()
                    .filter(|slot| usize::from(*slot) < crate::strafe_trail::MAX_CLIENTS);
                match slot {
                    Some(slot) => match self.strafe_trails.request_load(
                        &self.base,
                        self.game.as_deref(),
                        trail,
                        slot,
                    ) {
                        Ok(()) => self.push_console_line(format!(
                            "^3loadTrail:^7 queued {trail} (slot {slot})"
                        )),
                        Err(error) => self.push_console_line(format!("^1loadTrail:^7 {error}")),
                    },
                    None => self
                        .push_console_line("^3usage:^7 loadTrail <name> [slot 0..31]".to_owned()),
                }
                CommandResult::Handled
            }
            [_verb, ..] => {
                self.push_console_line("^3usage:^7 loadTrail <name> [slot 0..31]".to_owned());
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_cleartrail ["clearTrail", ] ("clearTrail <slot 0..31|-1>", "Clear existing trail geometry by client/color slot; -1 clears all.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb, slot] => {
                match slot.parse::<i32>() {
                    Ok(slot)
                        if slot == -1
                            || (0..crate::strafe_trail::MAX_CLIENTS as i32).contains(&slot) =>
                    {
                        let removed = self.strafe_trails.clear(slot);
                        self.push_console_line(format!("^3clearTrail:^7 cleared matching trail geometry ({removed} loaded file(s))."));
                    }
                    _ => self.push_console_line("^3usage:^7 clearTrail <slot 0..31|-1>".to_owned()),
                }
                CommandResult::Handled
            }
            [_verb, ..] => {
                self.push_console_line("^3usage:^7 clearTrail <slot 0..31|-1>".to_owned());
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_strafetrail ["strafeTrail", ] ("strafeTrail [client|-1]", "List/toggle live player trail tracing: /strafeTrail [client|-1].", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                let players = self
                    .game_session
                    .as_ref()
                    .map(|session| {
                        (0..crate::strafe_trail::MAX_CLIENTS)
                            .filter_map(|client| {
                                session
                                    .client_game
                                    .client_name(client)
                                    .map(|name| (client, name))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if players.is_empty() {
                    self.push_console_line(
                        "^3strafeTrail:^7 no player list is available.".to_owned(),
                    );
                } else {
                    self.push_console_line("^3num  Trace  Player".to_owned());
                    for (client, name) in players {
                        self.push_console_line(format!(
                            "^7{client:>3}   {}    ^7{name}",
                            if self.strafe_trails.is_tracing(client) {
                                "^2X"
                            } else {
                                "^8-"
                            }
                        ));
                    }
                }
                CommandResult::Handled
            }
            [_verb, client] => {
                match client.parse::<i32>().map_err(|_| ()).and_then(|client| {
                    self.strafe_trails
                        .toggle_player(client)
                        .map(|mask| (client, mask))
                        .map_err(|_| ())
                }) {
                    Ok((client, mask)) => {
                        self.mark_config_dirty();
                        if client == -1 {
                            self.push_console_line(format!(
                                "^3strafeTrail:^7 all-player mask is now {mask:#010x}."
                            ));
                        } else {
                            let enabled = self.strafe_trails.is_tracing(client as usize);
                            self.push_console_line(format!(
                                "^3strafeTrail {client}:^7 {}",
                                if enabled { "^2ON" } else { "^1OFF" }
                            ));
                        }
                    }
                    Err(()) => self
                        .push_console_line("^3usage:^7 strafeTrail [client 0..31|-1]".to_owned()),
                }
                CommandResult::Handled
            }
            [_verb, ..] => {
                self.push_console_line("^3usage:^7 strafeTrail [client 0..31|-1]".to_owned());
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_hudedit ["hudedit", ] ("hudedit", "Open the in-game drag/snap HUD editor.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                if self.front_end {
                    self.push_console_line(
                        "^3hudedit:^7 enter a game before editing the HUD".to_owned(),
                    );
                } else {
                    self.hud_edit_selected = None;
                    self.hud_edit_drag_origin = None;
                    self.hud_edit_drag_delta = [0.0, 0.0];
                    self.hud_edit_chat_resize_origin = None;
                    self.hud_edit_chat_resize_corner = None;
                    self.align_hud_defaults_for_editor();
                    self.set_overlay(OverlayMode::HudEdit);
                }
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_scores_show ["+scores", ] ("+scores", "Show the live scoreboard while held and request fresh scores.", CommandScope::Always) |self, command| {
        if self.execute_input_button_command(command) { CommandResult::Handled } else { CommandResult::Unhandled }
    }

    cmd_scores_hide ["-scores", ] ("-scores", "Hide the live scoreboard.", CommandScope::Always) |self, command| {
        if self.execute_input_button_command(command) { CommandResult::Handled } else { CommandResult::Unhandled }
    }

    cmd_messagemode ["messagemode", ] ("messagemode", "Open global chat input.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.begin_chat(ChatMode::Global);
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_messagemode2 ["messagemode2", ] ("messagemode2", "Open team chat input.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.begin_chat(ChatMode::Team);
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    // TaystJK Con_MessageMode3_f: PMF_FOLLOW selects the followed player;
    // otherwise CGVM_CrosshairPlayer supplies a recent target. No valid target
    // means the command does nothing, rather than opening global chat.
    cmd_messagemode3 ["messagemode3", ] ("messagemode3", "Open a private message to the targeted or spectated player.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                if let Some(slot) = self.message_mode_target() {
                    self.begin_chat(ChatMode::Direct(slot));
                }
                CommandResult::Handled
            }
            _ => CommandResult::Unhandled,
        }
    }

    // TaystJK Con_MessageMode4_f calls CG_LastAttacker and does not open
    // a chat box until CG_DamageFeedback has registered actual damage.
    cmd_messagemode4 ["messagemode4", ] ("messagemode4", "Open a private message to your last attacker.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                if let Some(slot) = self.message_mode_attacker() {
                    self.begin_chat(ChatMode::Direct(slot));
                }
                CommandResult::Handled
            }
            _ => CommandResult::Unhandled,
        }
    }

}
