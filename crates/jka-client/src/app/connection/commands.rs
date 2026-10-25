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

    /// The source for local geometric deformation cvars is VideoSettings.
    /// They are presentation choices; do not change gameplay collision bounds.
    pub(in crate::app) fn current_profile_tuning(&self) -> crate::cgame::jiggle::JiggleRemeshTuning {
        crate::cgame::jiggle::JiggleRemeshTuning {
            body_shape: self.video.body_shape_params(),
            body_details: self.video.body_details,
            breast_size: self.video.jiggle_breast_size,
            glute_size: self.video.jiggle_glute_size,
            breast_bottom_z: self.video.jiggle_breast_remesh_bottom_z,
            breast_top_z: self.video.jiggle_breast_remesh_top_z,
            glute_bottom_z: self.video.jiggle_glute_remesh_bottom_z,
            glute_top_z: self.video.jiggle_glute_remesh_top_z,
        }
    }

    /// Update the real color1 cvar. It then reaches the server through the
    /// same NetworkSettings::userinfo() path as every other CVAR_USERINFO.
    pub(in crate::app) fn sync_packed_cosmetics_cvar(&mut self) -> Result<(), String> {
        let cosmetics = crate::profile_wire::PackedCosmetics {
            mask: self.network.cosmetics,
            hat: crate::profile_wire::cosmetic_id(&self.network.global_hat),
            cape: crate::profile_wire::cosmetic_id(&self.network.global_cape),
        };
        let encoded = if cosmetics == crate::profile_wire::PackedCosmetics::default() {
            crate::global_cosmetics::encode(self.network.color1, &self.network.global_hat)
        } else {
            crate::profile_wire::encode_cosmetics(self.network.color1, cosmetics)
        };
        if self.network.color1_value() != encoded {
            self.network.set_cvar("color1", &encoded)
                .ok_or_else(|| "color1 cvar is not registered".to_owned())??;
        }
        Ok(())
    }

    /// Body settings feed the actual color2 cvar. Never write outgoing
    /// info strings here: they simply serialize the existing cvar value.
    pub(in crate::app) fn sync_packed_body_cvar(&mut self) -> Result<(), String> {
        let body = crate::profile_wire::BodyWire::from_tuning(self.current_profile_tuning());
        let encoded = if body.is_neutral() {
            crate::global_cosmetics::encode(self.network.color2, &self.network.global_cape)
        } else if let Some(encoded) = body.encode(self.network.color2) {
            encoded
        } else {
            // Do not transmit stale body data when the updated profile no
            // longer fits. Fall back to saber color, never partial deformations.
            self.push_console_error("color2: body profile exceeds the 220-character wire limit; sending saber color only".to_owned());
            self.network.color2.to_string()
        };
        if self.network.color2_value() != encoded {
            self.network.set_cvar("color2", &encoded)
                .ok_or_else(|| "color2 cvar is not registered".to_owned())??;
        }
        Ok(())
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
        let info = self.network.userinfo_for_server(&self.solo_client_info.model_cvar(), &server_info);
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
            // `cmd` (CL_ForwardToServer_f) only forwards once the connection is
            // Active. Queuing forcechanged during the join window is the one
            // path that never reached the server, so wait for Active and keep
            // the touched bit set instead of clearing it.
            let active = self.net.as_ref().is_some_and(|net| {
                net.state() == jka_protocol::session::ConnectionState::Active
            });
            if force_touched && !active {
                self.push_console_line(
                    "^3Force loadout pending: still joining the server; press APPLY again once in game"
                        .to_owned(),
                );
                return;
            }
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
                let sequence = self.net.as_ref().map_or(-1, |net| net.session().reliable_sequence());
                self.profile_force_pending = false;
                self.push_console_line(format!(
                    "^5Force loadout applied: sent userinfo + {command} (reliable seq {sequence})"
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
    pub(in crate::app) fn execute_game_command(&mut self, line: &str) -> bool {
        use crate::app::GameCommand;
        let words: Vec<_> = line.split_whitespace().collect();
        let Some(&verb) = words.first() else { return false; };
        let local_active = self.game_session.as_ref().is_some_and(|session| session.live && session.local);
        if !self.live_connected() && !local_active { return false; }
        let action = crate::app::command_registry().find(verb).and_then(|def| def.game_command);
        match action {
            Some(GameCommand::AmRun) => {
                if self.active_server_mod() == crate::net::mod_support::ServerMod::Japro {
                    let bits = crate::japro_cg::toggle_run_animation(self.network.plugin_disable);
                    if let Err(error) = self.set_console_cvar("cp_pluginDisable", &bits.to_string()) {
                        self.push_console_line(format!("^1amrun:^7 {error}"));
                    }
                }
                return true;
            }
            Some(GameCommand::Flipkick) => { self.start_japro_flipkick(); return true; }
            Some(GameCommand::Generic(value)) => {
                let local_supported = local_active && self.local_server.as_ref()
                    .is_some_and(|server| server.supports_generic_command(value));
                if self.live_connected() || local_supported {
                    self.live_input.queue_generic_command(value);
                    if (27..=31).contains(&value) && crate::emotes::stop_world() {
                        self.live_input.queue_emote_stop();
                    }
                    return true;
                }
            }
            Some(GameCommand::InventoryFree) => {
                if let Some(ps) = self.live_player_state().cloned() {
                    if let Some(generic) = self.live_input.use_first_inventory(&ps) {
                        self.live_input.queue_generic_command(generic);
                    }
                }
                return true;
            }
            Some(action @ (GameCommand::InventoryNext | GameCommand::InventoryPrev)) => {
                if let Some(ps) = self.live_player_state().cloned() {
                    self.live_input.cycle_inventory(&ps, matches!(action, GameCommand::InventoryNext));
                }
                return true;
            }
            Some(action @ (GameCommand::WeaponNext | GameCommand::WeaponPrev |
                GameCommand::WeaponLast | GameCommand::WeaponClean | GameCommand::ToggleFireMode |
                GameCommand::ForceNext | GameCommand::ForcePrev | GameCommand::Weapon)) => {
                if let Some(ps) = self.live_player_state().cloned() {
                    match action {
                        GameCommand::WeaponNext => self.live_input.cycle_weapon(&ps, true),
                        GameCommand::WeaponPrev => self.live_input.cycle_weapon(&ps, false),
                        GameCommand::WeaponLast => self.live_input.select_last_weapon(&ps),
                        GameCommand::ToggleFireMode => {
                            self.live_input.toggle_single_fire(&ps);
                            self.redirect_held_singlefire();
                        },
                        GameCommand::WeaponClean => if let Some(arg) = words.get(1) {
                            if self.live_input.select_weapon_clean(&ps, arg) {
                                self.live_input.queue_generic_command(1); // sv_saberswitch
                            }
                        },
                        GameCommand::ForceNext | GameCommand::ForcePrev => {
                            let forward = matches!(action, GameCommand::ForceNext);
                            // TaystJK: force cycling becomes inventory cycling
                            // while USE is held or there are no usable powers.
                            if self.live_buttons.contains("+use")
                                || self.live_buttons.contains("+button5")
                                || !crate::net::LiveInput::has_selectable_force(&ps)
                            {
                                self.live_input.cycle_inventory(&ps, forward);
                            } else {
                                self.live_input.cycle_force(&ps, forward);
                                self.force_select_until = Some(Instant::now() + Duration::from_millis(1400));
                                self.weapon_select_until = None;
                                self.publish_transient_ui();
                            }
                        }
                        GameCommand::Weapon => if let Some(slot) = words.get(1) { self.live_input.select_weapon_slot(&ps, slot); },
                        _ => unreachable!(),
                    }
                    if matches!(action, GameCommand::WeaponNext | GameCommand::WeaponPrev
                        | GameCommand::WeaponLast | GameCommand::WeaponClean | GameCommand::Weapon)
                        && ps.field_i32("pm_flags").unwrap_or(0) & 4096 == 0
                        && ps.field_i32("pm_type").unwrap_or(0) != 4
                        && ps.field_i32("emplacedIndex").unwrap_or(0) == 0
                    {
                        self.weapon_select_until = Some(Instant::now() + Duration::from_millis(1400));
                        self.force_select_until = None;
                        self.publish_transient_ui();
                    }
                }
                return true;
            }
            None => {}
        }
        // Remote reliable commands retain priority over lightweight local
        // authority handlers, which have no netchan and must still fall through.
        if self.live_connected() && crate::console::is_server_command(verb) {
            self.forward_command_to_server(line); return true;
        }
        false
    }
}
