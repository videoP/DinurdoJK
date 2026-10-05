//! Connection commands.
use crate::app::{App, Duration, Instant};

impl App {
    /// CL_ForwardCommandToServer.
    pub(in crate::app) fn forward_command_to_server(&mut self, line: &str) {
        let line = line.trim();
        let verb = line.split_whitespace().next().unwrap_or("");
        if verb.starts_with('-') {
            return;
        }
        if verb.starts_with('+') || !self.live_connected() {
            self.push_console_line(format!("^1Unknown command \"{verb}^7\""));
            return;
        }
        let text = if line.split_whitespace().nth(1).is_some() {
            line
        } else {
            verb
        };
        if let Some(net) = self.net.as_mut() {
            let bytes = crate::cgame::text_to_jka_bytes(text);
            if let Err(error) = net.session_mut().add_reliable_command(&bytes, false) {
                self.push_console_line(format!("^1{error}"));
            }
        }
    }

    /// Queue CL_CheckUserinfo and report whether the reliable command really
    /// entered the stream. Callers that gate UI state on delivery must not
    /// silently clear their pending bit when the reliable ring is full.
    pub(in crate::app) fn queue_userinfo(&mut self) -> Result<(), String> {
        if !self.live_connected() {
            return Ok(());
        }
        let server_info = self
            .net
            .as_ref()
            .map(|net| {
                crate::net::mod_support::server_info(&net.session().decoder().configstrings)
                    .to_vec()
            })
            .unwrap_or_default();
        self.refresh_japro_userinfo_state();
        let info = self
            .network
            .userinfo_for_server(&self.solo_client_info.model_cvar(), &server_info);
        let command = [b"userinfo \"".as_slice(), &info, b"\""].concat();
        let Some(net) = self.net.as_mut() else {
            return Err("network session disappeared while queuing userinfo".to_owned());
        };
        net.session_mut()
            .add_reliable_command(&command, false)
            .map_err(|error| error.to_string())
    }

    /// CL_CheckUserinfo: resend userinfo after a CVAR_USERINFO change.
    pub(in crate::app) fn send_userinfo(&mut self) {
        if !self.live_connected() {
            return;
        }
        match self.queue_userinfo() {
            Ok(()) => self.profile_userinfo_pending = false,
            Err(error) => self.push_console_line(format!("^1{error}")),
        }
    }

    /// Queue one already-engine-parsed client command as a reliable server
    /// command. This is the `cmd ...` payload after the engine strips `cmd`.
    pub(in crate::app) fn queue_server_command(&mut self, line: &str) -> Result<(), String> {
        if !self.live_connected() {
            return Err("not connected to a live server".to_owned());
        }
        let bytes = crate::cgame::text_to_jka_bytes(line.trim());
        let Some(net) = self.net.as_mut() else {
            return Err("network session disappeared while queuing command".to_owned());
        };
        net.session_mut()
            .add_reliable_command(&bytes, false)
            .map_err(|error| error.to_string())
    }

    /// TaystJK `setForce same`: active players send their current team with
    /// `forcechanged`, while spectators send the bare command. Read the local
    /// client's CS_PLAYERS entry rather than the viewed snapshot so follow mode
    /// cannot accidentally use the followed player's team.
    pub(in crate::app) fn live_forcechanged_team_arg(&self) -> Option<&'static str> {
        let net = self.net.as_ref()?;
        let decoder = net.session().decoder();
        // TaystJK setForce same reads ui_myteam, whose numeric cvar default
        // is TEAM_FREE (0). Our closest authoritative copy is the local
        // player configstring. If it has not arrived yet, preserve that
        // same TEAM_FREE default instead of accidentally sending the bare
        // spectator form. Follow mode must never use the followed client.
        let team = usize::try_from(decoder.client_number)
            .ok()
            .and_then(|client_num| u16::try_from(client_num).ok())
            .and_then(|client_num| crate::cgame::CS_PLAYERS.checked_add(client_num))
            .and_then(|index| decoder.configstrings.get(&index))
            .and_then(|info| jka_protocol::commands::info_value(info, b"t"))
            .and_then(|value| std::str::from_utf8(value).ok())
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(0);
        match team {
            0 => Some("FREE"),
            1 => Some("RED"),
            2 => Some("BLUE"),
            // TEAM_SPECTATOR (and malformed/unknown values) use the bare
            // command, exactly like UI_UpdateClientForcePowers(NULL).
            _ => None,
        }
    }

    /// Profile APPLY: commit the staged name, send userinfo once, then the
    /// TaystJK/JKA `cmd forcechanged [current team]` if Force was touched.
    pub(in crate::app) fn apply_profile_changes(&mut self) {
        let staged_name = self.profile_name_input.trim().to_owned();
        if !staged_name.is_empty() && staged_name != self.network.name {
            let name = crate::cgame::unicode_input_to_jka_text(&staged_name);
            if let Err(error) = self.profile_set_cvar("name", &name) {
                self.console_status = error;
            } else {
                self.profile_name_input = name.clone();
            }
        }
        let force_touched = self.profile_force_pending;
        if self.live_connected() {
            // TaystJK UI_UpdateClientForcePowers writes forcepowers first, then
            // executes `cmd forcechanged [team]` only when force was touched.
            // Always put a fresh userinfo immediately before that notification:
            // the server reads forcepowers from userinfo while handling it.
            if self.profile_userinfo_pending || force_touched {
                if let Err(error) = self.queue_userinfo() {
                    self.push_console_line(format!("^1Could not apply profile userinfo: {error}"));
                    return;
                }
                self.profile_userinfo_pending = false;
            }
            if force_touched {
                let command = self.live_forcechanged_team_arg().map_or_else(
                    || "forcechanged".to_owned(),
                    |team| format!("forcechanged \"{team}\""),
                );
                if let Err(error) = self.queue_server_command(&command) {
                    // Keep the touched bit set so Apply remains available and
                    // retries the exact Tayst notification next time.
                    self.push_console_line(format!("^1Could not apply Force loadout: {error}"));
                    return;
                }
                self.profile_force_pending = false;
                self.push_console_line(format!(
                    "^5Force loadout applied: sent userinfo + {command}"
                ));
            }
        } else {
            // No server can consume forcechanged. The cvar is still retained
            // locally for the next connection/profile use.
            self.profile_userinfo_pending = false;
            if force_touched {
                self.profile_force_pending = false;
                self.push_console_line(
                    "^3Force loadout saved locally; forcechanged not sent (not on a live server)"
                        .to_owned(),
                );
            }
        }
    }

    /// True when the Profile page has edits that APPLY has not sent yet.
    pub(in crate::app) fn profile_has_pending_changes(&self) -> bool {
        let name = self.profile_name_input.trim();
        self.profile_userinfo_pending
            || self.profile_force_pending
            || (!name.is_empty() && name != self.network.name)
    }

    /// Write a cvar from a Profile control: applied locally at once, sent on APPLY.
    pub(in crate::app) fn profile_set_cvar(
        &mut self,
        name: &str,
        value: &str,
    ) -> Result<(), String> {
        self.profile_defer_send = true;
        let result = self.set_console_cvar(name, value);
        self.profile_defer_send = false;
        result
    }

    /// Engine and cgame commands that only exist for a live connection.
    /// Returns true when the line was consumed.
    pub(in crate::app) fn execute_network_command(&mut self, line: &str) -> bool {
        let words: Vec<&str> = line.split_whitespace().collect();
        let Some(&verb) = words.first() else {
            return false;
        };
        let verb_lower = verb.to_ascii_lowercase();
        match verb_lower.as_str() {
            "connect" => {
                match words.get(1) {
                    Some(target) if words.len() == 2 => self.connect_to_server(target),
                    _ => self.push_console_line("^3usage:^7 connect [server]"),
                }
                return true;
            }
            "reconnect" => {
                if let Some(target) = self.reconnect_target.clone() {
                    self.connect_to_server(&target);
                }
                return true;
            }
            "cmd" => {
                // CL_ForwardToServer_f: requires CA_ACTIVE; argv(0) is not sent.
                let active = self.net.as_ref().is_some_and(|net| {
                    net.state() == jka_protocol::session::ConnectionState::Active
                });
                if !active {
                    self.push_console_line("Not connected to a server.");
                } else if let Some(rest) = line
                    .split_once(char::is_whitespace)
                    .map(|(_, rest)| rest.trim())
                {
                    if !rest.is_empty() {
                        if let Some(net) = self.net.as_mut() {
                            let _ = net
                                .session_mut()
                                .add_reliable_command(rest.as_bytes(), false);
                        }
                    }
                }
                return true;
            }
            "userinfo" => {
                let info = self.network.userinfo(&self.solo_client_info.model_cvar());
                self.push_console_line(format!("^3userinfo:^7 {}", String::from_utf8_lossy(&info)));
                return true;
            }
            "serverdump" => {
                self.server_dump();
                return true;
            }
            "serverinfo" | "systeminfo" if self.live_connected() => {
                let index = if verb_lower == "serverinfo" { 0 } else { 1 };
                let text = self
                    .net
                    .as_ref()
                    .and_then(|net| net.session().decoder().configstrings.get(&index).cloned())
                    .unwrap_or_default();
                // Keep empty values (`\fs_game\\`), or every later pair shifts by a field.
                let fields: Vec<&[u8]> = text
                    .split(|&b| b == b'\\')
                    .skip(usize::from(text.first() == Some(&b'\\')))
                    .collect();
                for pair in fields.chunks(2).filter(|pair| !pair[0].is_empty()) {
                    let key = String::from_utf8_lossy(pair[0]);
                    let value = pair
                        .get(1)
                        .map(|v| String::from_utf8_lossy(v).into_owned())
                        .unwrap_or_default();
                    self.push_console_line(format!("{key:<24} {value}"));
                }
                return true;
            }
            _ => {}
        }
        // Commands such as saberAttackCycle/weapnext are CL_CreateCmd inputs,
        // not reliable server strings.  The local-server shim must accept the
        // same client-side commands even though there is deliberately no netchan.
        let local_active = self
            .game_session
            .as_ref()
            .is_some_and(|session| session.live && session.local);
        if !self.live_connected() && !local_active {
            return false;
        }
        // TaystJK CG_AmRun_f is a local CGame command: on jaPRO it XORs
        // JAPRO_PLUGIN_JAWARUN in cp_pluginDisable after masking to the
        // supported plugin-disable range. It is never forwarded to the server.
        if verb_lower == "amrun" {
            if self.active_server_mod() == crate::net::mod_support::ServerMod::Japro {
                let bits = crate::japro_cg::toggle_run_animation(self.network.plugin_disable);
                if let Err(error) = self.set_console_cvar("cp_pluginDisable", &bits.to_string()) {
                    self.push_console_line(format!("^1amrun:^7 {error}"));
                }
            }
            return true;
        }

        // TaystJK registers flipkick as a local CGame command. It arms the
        // CG_DoAsync jump-tap sequence and is never forwarded to the server.
        if verb_lower == "flipkick" {
            self.start_japro_flipkick();
            return true;
        }

        // cgame commands that only change the next usercmd. Remote servers own
        // the whole generic-command table. The lightweight local authority
        // advertises only the game-side cases it actually implements so older
        // local handlers (currently force_speed/force_rage) still fall through.
        if let Some(value) = crate::net::generic_command(&verb_lower) {
            let local_supported = local_active
                && self
                    .local_server
                    .as_ref()
                    .is_some_and(|server| server.supports_generic_command(value));
            if self.live_connected() || local_supported {
                self.live_input.queue_generic_command(value);
                return true;
            }
        }
        if matches!(
            verb_lower.as_str(),
            "weapnext" | "weapprev" | "weapon" | "forcenext" | "forceprev"
        ) {
            if let Some(ps) = self.live_player_state().cloned() {
                match verb_lower.as_str() {
                    "weapnext" => self.live_input.cycle_weapon(&ps, true),
                    "weapprev" => self.live_input.cycle_weapon(&ps, false),
                    "forcenext" => {
                        self.live_input.cycle_force(&ps, true);
                        self.force_select_until =
                            Some(Instant::now() + Duration::from_millis(1400));
                        self.publish_transient_ui();
                    }
                    "forceprev" => {
                        self.live_input.cycle_force(&ps, false);
                        self.force_select_until =
                            Some(Instant::now() + Duration::from_millis(1400));
                        self.publish_transient_ui();
                    }
                    _ => {
                        if let Some(slot) = words.get(1) {
                            self.live_input.select_weapon_slot(&ps, slot);
                        }
                    }
                }
            }
            return true;
        }
        // Registered game/server commands are reliable strings only for a real
        // remote connection.  A local-server session deliberately has no
        // netchan, so consuming them here would prevent the local command
        // dispatcher below from handling commands such as noclip/force_speed.
        //
        // Keep local CL_CreateCmd commands (generic_cmd/weapon selection) above,
        // but let all other local commands fall through to the lightweight
        // authority's explicit handlers instead of feeding CL_ForwardCommandToServer.
        if self.live_connected() && crate::console::is_server_command(verb) {
            self.forward_command_to_server(line);
            return true;
        }
        false
    }
}
