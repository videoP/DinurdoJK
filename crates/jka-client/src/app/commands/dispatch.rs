//! Commands dispatch.
use crate::app::{
    scene, session_tools, split_console_script, App, Arc, ChatMode, ConsoleCvarSetResult,
    DemoViewMode, GameSession, Instant, JoinMode, OverlayMode, PlanarReflectionDebugMode,
    RenderCommand, SessionPhase,
};

impl App {
    pub(in crate::app) fn execute_command_line(&mut self, command: &str) {
        self.console_status.clear();
        // Cbuf/Cmd: a leading slash or backslash is ignored.
        let command = command.trim().trim_start_matches(['/', '\\']).trim_start();
        if command.is_empty() {
            return;
        }

        if self.handle_bind_console_command(command) {
            return;
        }
        // Cvar_Set_f family: `set <name> <value ...>` is `<name> <value>`.
        let set_form = command
            .split_once(char::is_whitespace)
            .filter(|(verb, _)| {
                ["set", "seta", "sets", "setu"]
                    .iter()
                    .any(|v| verb.eq_ignore_ascii_case(v))
            })
            .map(|(_, rest)| rest.trim().to_owned());
        if let Some(rest) = set_form {
            let Some((name, value)) = rest.split_once(char::is_whitespace) else {
                self.push_console_line("^3usage:^7 set <variable> <value>");
                return;
            };
            let value = value.trim().trim_matches('"');
            match self.set_console_cvar(name, value) {
                Ok(()) => {}
                Err(error) => self.push_console_line(format!("^1{error}")),
            }
            return;
        }
        if self.execute_network_command(command) {
            return;
        }

        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [verb] if verb.eq_ignore_ascii_case("clear") => {
                self.clear_console_output();
                self.publish_ui();
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("configstrings") => {
                // TaystJK CL_Configstrings_f: only an active CGame owns the
                // printable gameState table, and empty configstrings are skipped.
                let active = self
                    .game_session
                    .as_ref()
                    .filter(|session| session.phase == SessionPhase::Playing)
                    .map(|session| {
                        session
                            .client_game
                            .configstrings()
                            .iter()
                            .filter(|(_, value)| !value.is_empty())
                            .map(|(&index, value)| {
                                (index, String::from_utf8_lossy(value).into_owned())
                            })
                            .collect::<Vec<_>>()
                    });
                let Some(strings) = active else {
                    self.push_console_line("Not connected to a server.".to_owned());
                    return;
                };
                for (index, value) in strings {
                    self.push_console_line(format!("{index:4}: {value}"));
                }
                return;
            }
            [verb, ..] if verb.eq_ignore_ascii_case("configstrings") => {
                self.push_console_line("^3usage:^7 configstrings".to_owned());
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("version") => {
                self.push_console_line(format!(
                    "^2DinurdoJK^7 {}  ^8compiled^7 {}",
                    env!("CARGO_PKG_VERSION"),
                    env!("DINURDOJK_BUILD_UTC")
                ));
                return;
            }
            [verb, ..] if verb.eq_ignore_ascii_case("version") => {
                self.push_console_line("^3usage:^7 version".to_owned());
                return;
            }
            [verb, rest @ ..]
                if verb.eq_ignore_ascii_case("plugin")
                    || verb.eq_ignore_ascii_case("pluginDisable") =>
            {
                self.plugin_disable_command(rest);
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("voicechat") => {
                self.open_vgs_voicechat();
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("cameraedit") => {
                self.start_camera_edit();
                return;
            }
            [verb]
                if verb.eq_ignore_ascii_case("weapnext")
                    && self.demo_playback_active()
                    && self.demo_view_mode == DemoViewMode::Free =>
            {
                // Weapon-next / wheel-down slows free movement.
                self.adjust_demo_free_speed(-1);
                return;
            }
            [verb]
                if verb.eq_ignore_ascii_case("weapprev")
                    && self.demo_playback_active()
                    && self.demo_view_mode == DemoViewMode::Free =>
            {
                // Weapon-prev / wheel-up speeds free movement up.
                self.adjust_demo_free_speed(1);
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("follownext") && self.demo_playback_active() => {
                self.cycle_demo_follow(1);
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("followprev") && self.demo_playback_active() => {
                self.cycle_demo_follow(-1);
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("followfastest") => {
                self.follow_fastest_now();
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("followredflag") => {
                self.follow_flag_carrier(session_tools::FlagCarrier::Red);
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("followblueflag") => {
                self.follow_flag_carrier(session_tools::FlagCarrier::Blue);
                return;
            }
            [verb, rest @ ..] if verb.eq_ignore_ascii_case("speedometer") => {
                self.speedometer_command(rest);
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("trailmenu") => {
                self.open_strafe_trails();
                return;
            }
            [verb, trail] if verb.eq_ignore_ascii_case("loadtrail") => {
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
                return;
            }
            [verb, trail, slot] if verb.eq_ignore_ascii_case("loadtrail") => {
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
                return;
            }
            [verb, ..] if verb.eq_ignore_ascii_case("loadtrail") => {
                self.push_console_line("^3usage:^7 loadTrail <name> [slot 0..31]".to_owned());
                return;
            }
            [verb, slot] if verb.eq_ignore_ascii_case("cleartrail") => {
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
                return;
            }
            [verb, ..] if verb.eq_ignore_ascii_case("cleartrail") => {
                self.push_console_line("^3usage:^7 clearTrail <slot 0..31|-1>".to_owned());
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("strafetrail") => {
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
                return;
            }
            [verb, client] if verb.eq_ignore_ascii_case("strafetrail") => {
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
                return;
            }
            [verb, ..] if verb.eq_ignore_ascii_case("strafetrail") => {
                self.push_console_line("^3usage:^7 strafeTrail [client 0..31|-1]".to_owned());
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("hudedit") => {
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
                    self.set_overlay(OverlayMode::HudEdit);
                }
                return;
            }
            [verb, ..] if verb.eq_ignore_ascii_case("echo") => {
                let message = command
                    .split_once(char::is_whitespace)
                    .map(|(_, args)| args.trim())
                    .unwrap_or("");
                let message = message
                    .strip_prefix('"')
                    .and_then(|value| value.strip_suffix('"'))
                    .unwrap_or(message);
                self.push_console_line(message.to_owned());
                return;
            }
            [verb, name] if verb.eq_ignore_ascii_case("vstr") => {
                let Some(value) = self.console_cvar_value(name) else {
                    self.push_console_line(format!("^1Cvar {name} does not exist."));
                    return;
                };
                // Cbuf_InsertText: execute the variable contents before the
                // remainder of the current command buffer.
                self.insert_console_commands_front(split_console_script(&value));
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("vstr") => {
                self.push_console_line(
                    "^3vstr <variablename>^7 : execute a variable command".to_owned(),
                );
                return;
            }
            [verb, argument] if verb.eq_ignore_ascii_case("wait") => {
                let parsed = argument.parse::<i32>().unwrap_or(0);
                self.console_wait_frames = if parsed < 0 { 1 } else { parsed as u32 };
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("wait") => {
                self.console_wait_frames = 1;
                return;
            }
            [verb, ..] if verb.eq_ignore_ascii_case("wait") => {
                // OpenJK treats argc != 2 exactly like bare `wait`.
                self.console_wait_frames = 1;
                return;
            }
            [verb, filter @ ..] if verb.eq_ignore_ascii_case("cmdlist") => {
                let pattern = filter.first().copied();
                let mut all: Vec<_> = crate::console::commands(self.console_scope()).collect();
                all.sort_by_key(|entry| entry.name.to_ascii_lowercase());
                let total = all.len();
                let matches: Vec<_> = all
                    .into_iter()
                    .filter(|entry| {
                        pattern
                            .is_none_or(|pattern| crate::console::filter_match(pattern, entry.name))
                    })
                    .collect();
                for entry in &matches {
                    if entry.description.is_empty() {
                        self.push_console_line(format!(" ^7{}", entry.name));
                    } else {
                        self.push_console_line(format!(
                            " ^7{}^2 - {}",
                            entry.name, entry.description
                        ));
                    }
                }
                self.push_console_line(format!("^7{total} total commands"));
                if matches.len() != total {
                    self.push_console_line(format!("^7{} matching commands", matches.len()));
                }
                return;
            }
            [verb, name] if verb.eq_ignore_ascii_case("help") => {
                let entry = crate::console::commands(self.console_scope())
                    .find(|entry| entry.name.eq_ignore_ascii_case(name));
                if let Some(entry) = entry {
                    if entry.description.is_empty() {
                        self.push_console_line(format!("^8Cmd ^7{}", entry.name));
                    } else {
                        self.push_console_line(format!(
                            "^8Cmd ^7{}^2 - {}",
                            entry.name, entry.description
                        ));
                    }
                } else {
                    self.push_console_line(format!("^1Command {name} does not exist."));
                }
                return;
            }
            [verb, ..] if verb.eq_ignore_ascii_case("help") => {
                self.push_console_line("^3usage:^7 help <command or alias>".to_owned());
                return;
            }
            [verb, demo_name] if verb.eq_ignore_ascii_case("record") => {
                self.start_demo_recording(Some(demo_name));
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("record") => {
                self.start_demo_recording(None);
                return;
            }
            [verb, ..] if verb.eq_ignore_ascii_case("record") => {
                self.push_console_line("^3record [demoname]".to_owned());
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("predsettings") => {
                let command_rate = format!(
                    "{} ({:.2} ms usercmd gaps)",
                    self.network.command_rate,
                    1000.0 / f64::from(self.network.command_rate.max(15))
                );
                for line in self.predictor.regime_report(&command_rate) {
                    self.push_console_line(line);
                }
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("hitchmark") => {
                self.hitch_mark(None);
                return;
            }
            [verb, label] if verb.eq_ignore_ascii_case("hitchmark") => {
                self.hitch_mark(Some(label));
                return;
            }
            [verb, ..] if verb.eq_ignore_ascii_case("hitchmark") => {
                self.push_console_line("^3usage:^7 hitchmark [label]".to_owned());
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("stoprecord") => {
                self.stop_demo_recording();
                return;
            }
            [verb, ..] if verb.eq_ignore_ascii_case("stoprecord") => {
                self.push_console_line("^3stoprecord".to_owned());
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("fs_refresh") => {
                self.refresh_filesystem();
                return;
            }
            [verb, ..] if verb.eq_ignore_ascii_case("fs_refresh") => {
                self.push_console_line("^3usage:^7 fs_refresh".to_owned());
                return;
            }
            [verb, demo_name] if verb.eq_ignore_ascii_case("demo") => {
                self.play_demo_named(demo_name);
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("demo") => {
                self.push_console_line("^3demo <demoname>".to_owned());
                return;
            }
            [verb, action]
                if verb.eq_ignore_ascii_case("rGhost") && action.eq_ignore_ascii_case("clear") =>
            {
                self.clear_race_ghosts();
                return;
            }
            [verb, demo_name] if verb.eq_ignore_ascii_case("rGhost") => {
                self.load_race_ghost_named(demo_name);
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("rGhost") => {
                self.push_console_line("^3rGhost <demoname>^7 | ^3rGhost clear".to_owned());
                return;
            }
            [verb, ..] if verb.eq_ignore_ascii_case("rGhost") => {
                self.push_console_line("^3rGhost <demoname>^7 | ^3rGhost clear".to_owned());
                return;
            }
            [verb, filename]
                if verb.eq_ignore_ascii_case("exec") || verb.eq_ignore_ascii_case("execq") =>
            {
                self.exec_cfg_file(filename, verb.eq_ignore_ascii_case("execq"));
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("exec") || verb.eq_ignore_ascii_case("execq") => {
                self.push_console_line(format!(
                    "^3{} <filename>^7 : execute a script file{}",
                    verb,
                    if verb.eq_ignore_ascii_case("execq") {
                        " without notification"
                    } else {
                        ""
                    }
                ));
                return;
            }
            [verb, filename]
                if verb.eq_ignore_ascii_case("write")
                    || verb.eq_ignore_ascii_case("writeconfig") =>
            {
                self.write_named_config(filename);
                return;
            }
            [verb]
                if verb.eq_ignore_ascii_case("write")
                    || verb.eq_ignore_ascii_case("writeconfig") =>
            {
                self.push_console_line(
                    "^3usage:^7 writeconfig <filename>  (write is an alias)".to_owned(),
                );
                return;
            }
            _ => {}
        }
        if let [name] = words.as_slice() {
            if let Some(entry) = crate::console::find(name) {
                if entry.kind == crate::console::EntryKind::Cvar {
                    let line = self.format_console_entry(entry);
                    self.push_console_line(line);
                    return;
                }
            }
        }
        if words.len() >= 2 && words[1] == "!" {
            let name = words[0];
            if let Some(entry) = crate::console::find(name) {
                if entry.kind == crate::console::EntryKind::Cvar {
                    let Some(current) = self.console_cvar_value(name) else {
                        self.push_console_line(format!("^1Cvar {name} is not writable here."));
                        return;
                    };
                    // OpenJK Cvar_Command semantics: a first argument of `!`
                    // sets the numeric cvar to logical-not(current value). This
                    // is what makes binds such as `cg_thirdPerson !` toggles.
                    let numeric = current.parse::<f32>().unwrap_or(0.0);
                    let target = if numeric == 0.0 { "1" } else { "0" };
                    match self.set_console_cvar(name, target) {
                        Ok(()) => self.push_console_line(self.format_console_entry(entry)),
                        Err(error) => self.push_console_line(format!("^1{error}")),
                    }
                    return;
                }
            }
        }
        // Cvar_Command semantics: if the first token is a cvar, the entire
        // remainder is its value. This is important for string cvars such as
        // packed HUD layouts and also matches normal JKA console behavior.
        if let Some((name, raw_value)) = command.split_once(char::is_whitespace) {
            if let Some(entry) = crate::console::find(name) {
                if entry.kind == crate::console::EntryKind::Cvar {
                    let value = raw_value.trim().trim_matches('"');
                    match self.set_console_cvar_from_console(name, value) {
                        Ok(ConsoleCvarSetResult::Applied) => {
                            let line = self.format_console_entry(entry);
                            self.push_console_line(line);
                        }
                        Ok(ConsoleCvarSetResult::Latched) => self.push_console_line(format!(
                            "^7{} will be changed upon restarting.",
                            entry.name
                        )),
                        Ok(ConsoleCvarSetResult::Unchanged) => {}
                        Err(error) => self.push_console_line(format!("^1{error}")),
                    }
                    return;
                }
            }
        }

        if words
            .first()
            .is_some_and(|verb| verb.eq_ignore_ascii_case("toggle"))
        {
            if words.len() < 2 {
                self.push_console_line(
                    "^3usage:^7 toggle <variable> [value1 value2 ...]".to_owned(),
                );
                return;
            }
            let name = words[1];
            let Some(entry) = crate::console::find(name) else {
                self.push_console_line(format!("^1Cvar {name} does not exist."));
                return;
            };
            if entry.kind != crate::console::EntryKind::Cvar {
                self.push_console_line(format!("^1{name} is not a cvar."));
                return;
            }
            let Some(current) = self.console_cvar_value(name) else {
                self.push_console_line(format!("^1Cvar {name} is not writable here."));
                return;
            };
            let target = if words.len() == 2 {
                let Some(values) = crate::console::declared_integer_options(entry) else {
                    self.push_console_line(format!(
                        "^3toggle:^7 {name} has no declared discrete options; use `{name} !` for a logical 0/1 flip or pass explicit values"
                    ));
                    return;
                };
                values
                    .iter()
                    .position(|candidate| candidate == &current)
                    .map(|index| values[(index + 1) % values.len()].clone())
                    .unwrap_or_else(|| values[0].clone())
            } else if words.len() == 3 {
                self.push_console_line("^3toggle:^7 nothing to toggle to".to_owned());
                return;
            } else {
                let values = &words[2..];
                let next = values
                    .iter()
                    .take(values.len().saturating_sub(1))
                    .position(|candidate| *candidate == current.as_str())
                    .map(|index| values[index + 1])
                    .unwrap_or(values[0]);
                next.to_owned()
            };
            match self.set_console_cvar_from_console(name, &target) {
                Ok(ConsoleCvarSetResult::Applied) => {
                    self.push_console_line(self.format_console_entry(entry))
                }
                Ok(ConsoleCvarSetResult::Latched) => self.push_console_line(format!(
                    "^7{} will be changed upon restarting.",
                    entry.name
                )),
                Ok(ConsoleCvarSetResult::Unchanged) => {}
                Err(error) => self.push_console_line(format!("^1{error}")),
            }
            return;
        }

        if let Some((verb, message)) = command.split_once(char::is_whitespace) {
            if verb.eq_ignore_ascii_case("rcon") {
                // Cmd_Cmd()+5: the rest of the line verbatim.
                self.send_rcon(message.trim_start());
                return;
            }
            let message = message.trim();
            if verb.eq_ignore_ascii_case("say") || verb.eq_ignore_ascii_case("say_team") {
                if message.is_empty() {
                    self.push_console_line(format!(
                        "^3{}^7 [command] - {}",
                        verb, "Message text is required."
                    ));
                } else {
                    self.push_chat_line(
                        if verb.eq_ignore_ascii_case("say_team") {
                            ChatMode::Team
                        } else {
                            ChatMode::Global
                        },
                        message,
                    );
                }
                self.publish_ui();
                return;
            }
        }

        match words.as_slice() {
            [verb, name]
                if verb.eq_ignore_ascii_case("devmap") || verb.eq_ignore_ascii_case("map") =>
            {
                match scene::MapSource::from_map_argument(name) {
                    Ok(source) => match scene::verify_map_source_exists(
                        &self.base,
                        self.game.as_deref(),
                        &source,
                    ) {
                        Ok(()) => {
                            // A local map/devmap launch owns the client state. Tear down any
                            // live netchan first so server snapshots/commands cannot continue
                            // arriving while the local map is being prepared.
                            if self.net.is_some() {
                                self.disconnect_to_main_menu();
                            } else {
                                // `map`/`devmap` starts a new local server lifetime.
                                // Do not let the previous local/demo CGame make the
                                // MapPrepared handler think an external authority
                                // already owns the new world.
                                self.game_session = None;
                                self.local_server = None;
                                self.predictor.reset();
                                self.solo_dynamic_models = Arc::new(Vec::new());
                            }
                            self.enter_local_game_dir();
                            self.request_map(source);
                            self.console_map_barrier = !self.console_command_buffer.is_empty();
                        }
                        Err(error) => {
                            self.console_status = error.to_ascii_uppercase();
                        }
                    },
                    Err(error) => {
                        self.console_status = error.to_ascii_uppercase();
                    }
                }
            }
            [verb] if verb.eq_ignore_ascii_case("devmap") || verb.eq_ignore_ascii_case("map") => {
                self.console_status = "USAGE: DEVMAP MP/FFA3[.BSP|.MAP]".into();
            }
            [verb] if verb.eq_ignore_ascii_case("cg_eventstats") => {
                let lines = self
                    .game_session
                    .as_ref()
                    .map(GameSession::event_stats_lines)
                    .unwrap_or_else(|| vec!["^3CG EVENT STATS:^7 no demo is playing".into()]);
                for line in lines {
                    self.push_console_line(line);
                }
                return;
            }
            [verb, argument]
                if verb.eq_ignore_ascii_case("cg_eventstats")
                    && argument.eq_ignore_ascii_case("clear") =>
            {
                if let Some(playback) = self.game_session.as_mut() {
                    playback.clear_event_stats();
                    self.console_status = "CG EVENT STATS CLEARED".into();
                } else {
                    self.console_status = "NO DEMO IS PLAYING".into();
                }
            }
            [verb]
                if verb.eq_ignore_ascii_case("r_dumpmaterials")
                    || verb.eq_ignore_ascii_case("dumpmaterials") =>
            {
                self.render_command(RenderCommand::DumpMaterials {
                    filter: None,
                    all: false,
                });
                self.console_status =
                    "MATERIAL DEBUG DUMPED TO TERMINAL (ENHANCED MATERIALS ONLY)".into();
            }
            [verb, argument]
                if verb.eq_ignore_ascii_case("r_dumpmaterials")
                    || verb.eq_ignore_ascii_case("dumpmaterials") =>
            {
                self.render_command(RenderCommand::DumpMaterials {
                    filter: (!argument.eq_ignore_ascii_case("all")).then(|| (*argument).to_owned()),
                    all: argument.eq_ignore_ascii_case("all"),
                });
                self.console_status = if argument.eq_ignore_ascii_case("all") {
                    "ALL MATERIAL DEBUG DUMPED TO TERMINAL".into()
                } else {
                    format!("MATERIAL DEBUG FILTER '{argument}' DUMPED TO TERMINAL")
                };
            }
            [verb, argument] if verb.eq_ignore_ascii_case("r_planardebug") => {
                if let Some(mode) = PlanarReflectionDebugMode::from_config(argument) {
                    self.video.planar_reflection_debug = mode;
                    self.sync_planar_reflection_debug();
                    self.console_status = format!("PLANAR REFLECTION DEBUG: {}", mode.label());
                    self.publish_ui();
                } else {
                    self.console_status =
                        "R_PLANARDEBUG: OFF | CANDIDATES | SELECTED | TEXTURE | APPLIED".into();
                }
            }
            [verb] if verb.eq_ignore_ascii_case("+scores") => {
                let was_showing = self.scores_showing;
                self.scores_showing = true;
                if !was_showing {
                    if !self.refresh_demo_scoreboard(Instant::now()) {
                        self.request_scores_if_due(true);
                    }
                }
                self.publish_ui();
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("-scores") => {
                self.scores_showing = false;
                self.publish_ui();
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("messagemode") => {
                self.begin_chat(ChatMode::Global);
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("messagemode2") => {
                self.begin_chat(ChatMode::Team);
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("force_speed") => {
                if let Some(player) = &mut self.local_server {
                    player.request_power(jka_movement::MovementPower::Speed);
                }
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("force_rage") => {
                if let Some(player) = &mut self.local_server {
                    player.request_power(jka_movement::MovementPower::Rage);
                }
                return;
            }
            [verb, requested_team]
                if verb.eq_ignore_ascii_case("team") && self.local_server.is_some() =>
            {
                // OpenJK SetTeam in GT_FFA: scoreboard/follow/spectator forms become
                // TEAM_SPECTATOR; every other token becomes TEAM_FREE. That is why
                // stock `team f` joins the game even though `f` has no explicit case.
                let mode = if matches!(
                    requested_team.to_ascii_lowercase().as_str(),
                    "scoreboard" | "score" | "follow1" | "follow2" | "spectator" | "s"
                ) {
                    JoinMode::Spectator
                } else {
                    JoinMode::Player
                };
                self.join_as(mode);
                return;
            }
            [verb, what]
                if verb.eq_ignore_ascii_case("give")
                    && what.eq_ignore_ascii_case("all")
                    && self.local_server.is_some() =>
            {
                let result = self
                    .local_server
                    .as_mut()
                    .expect("checked above")
                    .give_all();
                match result {
                    Ok(()) => {
                        self.push_local_snapshot(true);
                        self.update_solo_player_view_and_presentation();
                        self.publish_ui();
                    }
                    Err(error) => self.push_console_line(format!("^1{error}")),
                }
                return;
            }
            [verb, arguments @ ..]
                if verb.eq_ignore_ascii_case("setviewpos") && self.local_server.is_some() =>
            {
                self.local_setviewpos(arguments);
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("viewpos") => {
                self.print_viewpos();
                return;
            }
            [verb, arguments @ ..] if verb.eq_ignore_ascii_case("perfsample") => {
                self.begin_perf_sample(arguments);
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("puddle_debug") => {
                self.puddle_debug_visualization = !self.puddle_debug_visualization;
                self.render_command(RenderCommand::SetPuddleDebug(
                    self.puddle_debug_visualization,
                ));
                self.console_status = if self.puddle_debug_visualization {
                    "PUDDLE DEBUG ON (blue=puddle, green=wet film)".into()
                } else {
                    "PUDDLE DEBUG OFF".into()
                };
            }
            [verb] if verb.eq_ignore_ascii_case("trace") => {
                self.trace_surface_center();
            }
            [verb] if verb.eq_ignore_ascii_case("trace_clear") => {
                self.clear_surface_inspection();
            }
            [verb, query @ ..]
                if verb.eq_ignore_ascii_case("entities")
                    || verb.eq_ignore_ascii_case("entgraph") =>
            {
                let query = query.join(" ");
                self.open_entity_graph((!query.is_empty()).then_some(query.as_str()));
                return;
            }
            [verb, page @ ..] if verb.eq_ignore_ascii_case("uipage") => {
                self.open_ui_page(page);
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("cycle_spawn") => {
                self.cycle_spawn();
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("noclip") => {
                self.toggle_noclip();
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("vid_restart") => {
                self.restart_renderer();
                return;
            }
            [verb]
                if verb.eq_ignore_ascii_case("screenshot")
                    || verb.eq_ignore_ascii_case("/screenshot") =>
            {
                self.request_screenshot();
                self.console_status = "SCREENSHOT QUEUED".into();
            }
            [verb] if verb.eq_ignore_ascii_case("say") || verb.eq_ignore_ascii_case("say_team") => {
                self.console_status = format!("USAGE: {} <MESSAGE>", verb.to_ascii_uppercase());
            }
            [verb] if verb.eq_ignore_ascii_case("rcon") => {
                self.push_console_line("^3usage:^7 rcon <command>");
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("disconnect") => {
                if self.net.is_some() {
                    self.push_console_line("^3Disconnected from server.");
                }
                self.disconnect_to_main_menu();
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("demo_pause") => {
                self.console_status = match self.game_session.as_mut() {
                    Some(playback) => match playback.toggle_pause(Instant::now()) {
                        Ok(0.0) => "DEMO PAUSED".into(),
                        Ok(rate) => format!("DEMO PLAYING AT {rate}X"),
                        Err(error) => error,
                    },
                    None => "NO DEMO IS PLAYING".into(),
                };
            }
            [verb] if verb.eq_ignore_ascii_case("fxinfo") => {
                match self.game_session.as_ref() {
                    Some(playback) => {
                        let stats = playback.weapon_fx.stats();
                        self.push_console_line(format!(
                            "^3FX:^7 effects {} | live primitives {} | scheduled {} | dropped {} | unsupported spawns {} | lod culled {} | lod saved {} | surfaces {}",
                            stats.registered,
                            stats.active,
                            stats.scheduled,
                            stats.dropped,
                            stats.unsupported_spawns,
                            stats.lod_culled,
                            stats.lod_saved,
                            playback.fx_surfaces.len(),
                        ));
                    }
                    None => self.push_console_line("^3FX:^7 no active CGame (start a demo first)."),
                }
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("soundinfo") => {
                let sound_state = self
                    .game_session
                    .as_ref()
                    .and_then(|playback| playback.sound_presenter.as_ref())
                    .and_then(|sound| sound.info().map(|info| (info, sound.source_rate_summary())));
                if let Some((info, source_rates)) = sound_state {
                    self.push_console_line(format!(
                        "^3AUDIO OUTPUT:^7 {} ch | {} Hz | {} | buffer {}",
                        info.channels, info.sample_rate, info.sample_format, info.buffer_size
                    ));
                    self.push_console_line(format!(
                        "^3AUDIO MIX:^7 voices {} / 32 | loops {} ({} requests) | pre-limit peak {:.3} | overload samples {}",
                        info.active_voices, info.active_loops, info.loop_requests, info.peak_before_limiter, info.overload_samples
                    ));
                    self.push_console_line(format!(
                        "^3AUDIO SOURCES:^7 {} (each retains its decoded rate; mixer converts to {} Hz output)",
                        source_rates, info.sample_rate
                    ));
                    self.push_console_line(format!(
                        "^3AUDIO LEVELS:^7 s_volume {:.3} | s_volumeVoice {:.3} | s_musicvolume {:.3} | s_separation {:.3}",
                        self.audio.effects_volume,
                        self.audio.voice_volume,
                        self.audio.music_volume,
                        self.audio.separation
                    ));
                    self.push_console_line(format!(
                        "^3AUDIO PAN:^7 largest per-frame gain step {:.3} (glided per sample, not stepped)",
                        info.largest_gain_step
                    ));
                    if info.overload_samples > 0 {
                        self.push_console_line(
                            "^3AUDIO NOTE:^7 the game mix exceeded full scale before the output limiter; this is a likely crackle/clipping source.",
                        );
                    }
                } else {
                    self.push_console_line(
                        "^3AUDIO:^7 no active CGame audio backend (start a demo/map presentation first).",
                    );
                }
                return;
            }
            [verb, rate] if verb.eq_ignore_ascii_case("demo_speed") => {
                self.console_status = match rate.parse::<f64>() {
                    Ok(rate) => match self.game_session.as_mut() {
                        Some(playback) => match playback.set_playback_rate(Instant::now(), rate) {
                            Ok(()) if rate == 0.0 => "DEMO PAUSED".into(),
                            Ok(()) => format!("DEMO SPEED: {rate}X"),
                            Err(error) => error,
                        },
                        None => "NO DEMO IS PLAYING".into(),
                    },
                    Err(_) => "USAGE: demo_speed <0..100>".into(),
                };
            }
            [verb] if verb.eq_ignore_ascii_case("quit") || verb.eq_ignore_ascii_case("exit") => {
                self.request_quit();
            }
            _ => {
                if self.live_connected() {
                    // CL_ForwardCommandToServer: anything unrecognised locally.
                    self.forward_command_to_server(command);
                    return;
                }
                self.console_status = format!("^1UNKNOWN COMMAND:^7 {command}");
            }
        }
        if !self.console_status.is_empty() {
            self.push_console_line(self.console_status.clone());
        }
    }
}
