//! Declarative command handlers.
use crate::app::{split_console_script, App, ChatMode, ConsoleCvarSetResult};

use super::{CommandDef, CommandResult, CommandScope};
commands! {
    cmd_emote_gallery ["emote_gallery"] ("emote_gallery <first> [count] [samples] | ids <EID,...> [samples] | stop", "Render emotes offline on the Profile stage from four angles at scrubbed times; PNGs and report.json go to emote-gallery/.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        if words.get(1) == Some(&"stop") {
            self.emote_gallery = None;
            self.emote_gallery_scene = std::sync::Arc::new(Vec::new());
            self.console_status = "emote_gallery stopped".into();
        } else {
            // The catalogue loads in the background; start once it is ready.
            self.emote_gallery_request = Some(words[1..].iter().map(|w| w.to_string()).collect());
            self.console_status = "emote_gallery: waiting for the emote catalogue".into();
        }
        CommandResult::Handled
    }

    cmd_djk_emote ["djk_emote"] ("djk_emote [list|refresh|stop|networkID|EID]", "Play a cosmetic emote using a normal game taunt and permanent network ID.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_, "stop"] => { self.stop_game_emote(); self.console_status = "Emote stopped".into(); }
            [_, "refresh"] => { self.stop_game_emote(); crate::emotes::refresh_catalog(); self.console_status = "Emote catalog refreshed".into(); }
            [_] | [_, "list"] => {
                match crate::emotes::catalog(&self.base) {
                    Ok(emotes) => {
                        let mut rows = std::collections::BTreeMap::new();
                        for e in emotes.iter().flat_map(|e| std::iter::once(e).chain(&e.variants)) {
                            if let Some(id) = crate::emotes::network::wire_id(e) {
                                rows.entry(id).or_insert_with(|| format!("{id} {} / {}", crate::emotes::display_title(e), e.variant_name));
                            }
                        }
                        self.console_status = rows.into_values().collect::<Vec<_>>().join("\n");
                    },
                    Err(error) => self.console_status = error,
                }
            }
            [_, id] => {
                if self.game_session.is_none() { self.console_status = "Load a game first, or preview in Profile > Emotes".into(); }
                else {
                    match crate::emotes::catalog(&self.base) {
                        Ok(emotes) => match id.parse::<u16>().ok().and_then(|number| crate::emotes::network::resolve(&emotes, number)).or_else(|| emotes.iter().find_map(|e| {
                            if e.id.eq_ignore_ascii_case(id) || e.catalog_aliases.iter().any(|alias|alias.eq_ignore_ascii_case(id)) {Some(e.clone())}
                            else {e.variants.iter().find(|v|v.id.eq_ignore_ascii_case(id)).cloned()}
                        })) {
                            Some(emote) => { self.play_game_emote(emote); self.console_status = format!("Playing emote {id}"); }
                            None => self.console_status = format!("Unknown emote {id}; use djk_emote list"),
                        },
                        Err(error) => self.console_status = error,
                    }
                }
            }
            _ => self.console_status = "usage: djk_emote [list|refresh|stop|networkID|EID]".into(),
        }
        CommandResult::Status
    }

    cmd_snd_restart ["snd_restart", ] ("snd_restart", "Restart the sound subsystem and apply current audio settings.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.restart_sound_subsystem();
                CommandResult::Handled
            }
            [_verb, ..] => {
                self.push_console_line("^3usage:^7 snd_restart".to_owned());
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_clear ["clear", ] ("clear", "Clear the console output.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.clear_console_output();
                self.publish_ui();
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    // This changes local renderer state, not a server command. Register it in
    // the always-available console set (also used by completion in devmap).
    cmd_remap_shader ["remapShader", ] ("remapShader <old> <new>", "Locally remap one renderer shader to another, matching TaystJK R_RemapShader behavior.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb, old, new] => {
                self.local_remap_shader(old, new);
                CommandResult::Handled
            }
            [_verb, ..] => {
                self.push_console_line("^3usage:^7 remapShader <old> <new>".to_owned());
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_list_remaps ["listRemaps", ] ("listRemaps", "List active renderer shader remaps (local and server-provided).", CommandScope::Always) |self, command| {
        match command.split_whitespace().collect::<Vec<_>>().as_slice() {
            [_verb] => {
                self.local_list_shader_remaps();
                CommandResult::Handled
            }
            [_verb, ..] => {
                self.push_console_line("^3usage:^7 listRemaps".to_owned());
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_plugin ["plugin", "pluginDisable"] ("plugin [bit]", "List or toggle a jaPRO cp_pluginDisable preference by the bit number shown by /plugin.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb, rest @ ..] => {
                self.plugin_disable_command(rest);
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_echo ["echo", ] ("echo [message]", "Print message text to the console.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb, ..] => {
                let message = command
                    .split_once(char::is_whitespace)
                    .map(|(_, args)| args.trim())
                    .unwrap_or("");
                let message = message
                    .strip_prefix('"')
                    .and_then(|value| value.strip_suffix('"'))
                    .unwrap_or(message);
                self.push_console_line(message.to_owned());
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_vstr ["vstr", ] ("vstr <variable>", "Execute the current value of a cvar as command text.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb, name] => {
                let Some(value) = self.console_cvar_value(name) else {
                    self.push_console_error(format!("^1Cvar {name} does not exist."));
                    return CommandResult::Handled;
                };
                // Cbuf_InsertText: execute the variable contents before the
                // remainder of the current command buffer.
                self.insert_console_commands_front(split_console_script(&value));
                CommandResult::Handled
            }
            [_verb] => {
                self.push_console_line(
                    "^3vstr <variablename>^7 : execute a variable command".to_owned(),
                );
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_wait ["wait", ] ("wait [frames]", "Pause execution of the remaining command buffer for one or more cl_commandRate frames.", CommandScope::Always) |self, command| {
        // Keep OpenJK/TaystJK Cmd_TokenizeString + atoi argument semantics, but
        // use DinurdoJK's usercmd clock as the frame authority. Rendering is
        // deliberately decoupled from movement here, so com_maxfps must not
        // change the duration of gameplay-oriented wait scripts.
        let args = jka_protocol::commands::tokenize(command.as_bytes());
        let frames = match args.as_slice() {
            [_verb, argument] => {
                let parsed = jka_protocol::commands::atoi(argument);
                if parsed < 0 { 1 } else { parsed as u32 }
            }
            [_verb] | [_verb, _, ..] => 1,
            _ => return CommandResult::Unhandled,
        };
        self.console_wait_until = if frames == 0 {
            None
        } else {
            let rate = f64::from(self.network.command_rate.max(15));
            Some(crate::app::Instant::now()
                + crate::app::Duration::from_secs_f64(f64::from(frames) / rate))
        };
        CommandResult::Handled
    }

    cmd_cmdlist ["cmdlist", ] ("cmdlist [pattern]", "List registered console commands, optionally filtered by a wildcard pattern.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb, filter @ ..] => {
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
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_help ["help", ] ("help <command or cvar>", "Print help, current value, default and range for a registered command or cvar.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        if let [_, name] = words.as_slice() {
            if let Some(entry) = crate::console::find_command(name, self.console_scope()) {
                if entry.kind == crate::console::EntryKind::Cvar {
                    self.push_console_line(self.format_console_entry(entry));
                } else {
                    self.push_console_line(format!(
                        "^8Cmd ^7{}^2 - {}",
                        entry.name, entry.description
                    ));
                    if !entry.usage.is_empty() {
                        self.push_console_line(format!("^3usage:^7 {}", entry.usage));
                    }
                }
            } else {
                self.push_console_error(format!("^1Command {name} does not exist."));
            }
        } else {
            self.push_console_line("^3usage:^7 help <command or cvar>");
        }
        CommandResult::Handled
    }

    cmd_cvarlist ["cvarlist", ] ("cvarlist [pattern]", "List registered cvars, optionally filtered by a wildcard pattern.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        let mut entries: Vec<_> = crate::console::cvars().collect();
        entries.sort_by_key(|entry| entry.name.to_ascii_lowercase());
        let total = entries.len();
        let mut matched = 0;
        for entry in entries {
            if words
                .get(1)
                .is_none_or(|pattern| crate::console::filter_match(pattern, entry.name))
            {
                self.push_console_line(self.format_console_entry(entry));
                matched += 1;
            }
        }
        self.push_console_line(format!("^7{total} total cvars"));
        if matched != total {
            self.push_console_line(format!("^7{matched} matching cvars"));
        }
        CommandResult::Handled
    }

    cmd_fs_refresh ["fs_refresh", ] ("fs_refresh", "Re-scan the active base/fs_game VFS and retry assets that previously failed without restarting CGame.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.refresh_filesystem();
                CommandResult::Handled
            }
            [_verb, ..] => {
                self.push_console_line("^3usage:^7 fs_refresh".to_owned());
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_exec ["exec", "execq"] ("exec <filename>", "Execute a .cfg script through the active fs_game/base VFS.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [verb, filename] => {
                self.exec_cfg_file(filename, verb.eq_ignore_ascii_case("execq"));
                CommandResult::Handled
            }
            [verb] => {
                self.push_console_line(format!(
                    "^3{} <filename>^7 : execute a script file{}",
                    verb,
                    if verb.eq_ignore_ascii_case("execq") {
                        " without notification"
                    } else {
                        ""
                    }
                ));
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_write ["write", "writeconfig"] ("writeconfig <filename> [verbose]", "Write archived cvars and binds in sections; verbose adds concise value help.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb, filename] => {
                self.write_named_config(filename, false);
                CommandResult::Handled
            }
            [_verb, filename, verbose] if verbose.eq_ignore_ascii_case("verbose") => {
                self.write_named_config(filename, true);
                CommandResult::Handled
            }
            [_verb] => {
                self.push_console_line(
                    "^3usage:^7 writeconfig <filename> [verbose]  (write is an alias)".to_owned(),
                );
                CommandResult::Handled
            }
            [_verb, ..] => {
                self.push_console_line("^3usage:^7 writeconfig <filename> [verbose]");
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_vid_restart ["vid_restart", ] ("vid_restart", "Restart the WGPU renderer and reload GPU resources using current video settings.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.restart_renderer();
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_say ["say", "say_team"] ("say <message>", "Send a global chat message: say <message>.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        if self.execute_game_command(command) {
            return CommandResult::Handled;
        }
        if let Some((verb, message)) = command.split_once(char::is_whitespace) {
            let message = message.trim();
            if message.is_empty() {
                self.push_console_line(format!(
                    "^3{}^7 [command] - Message text is required.",
                    verb
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
            return CommandResult::Handled;
        }
        match words.as_slice() {
            [verb] => {
                self.console_status = format!("USAGE: {} <MESSAGE>", verb.to_ascii_uppercase());
                CommandResult::Status
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_rcon ["rcon", ] ("rcon <command>", "Run a command on a server remotely: rcon <command>. Needs rconPassword, and a connection or rconAddress.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        if let Some((_, message)) = command.split_once(char::is_whitespace) {
            self.send_rcon(message.trim_start());
            return CommandResult::Handled;
        }
        match words.as_slice() {
            [_verb] => {
                self.push_console_line("^3usage:^7 rcon <command>");
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_disconnect ["disconnect", ] ("disconnect", "Leave the current game and return to the main menu.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                if self.net.is_some() {
                    self.push_console_line("^3Disconnected from server.");
                }
                self.disconnect_to_main_menu();
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_quit ["quit", "exit"] ("quit", "Exit the client.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.request_quit();
                CommandResult::Status
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_toggle ["toggle", ] ("toggle <variable> [value1 value2 ...]", "Cycle a cvar through its declared discrete options; explicit value lists are also supported.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();

        if words.len() < 2 {
            self.push_console_line("^3usage:^7 toggle <variable> [value1 value2 ...]".to_owned());
            return CommandResult::Handled;
        }
        let name = words[1];
        let Some(entry) = crate::console::find(name) else {
            self.push_console_error(format!("^1Cvar {name} does not exist."));
            return CommandResult::Handled;
        };
        if entry.kind != crate::console::EntryKind::Cvar {
            self.push_console_error(format!("^1{name} is not a cvar."));
            return CommandResult::Handled;
        }
        let Some(current) = self.console_cvar_value(name) else {
            self.push_console_error(format!("^1Cvar {name} is not writable here."));
            return CommandResult::Handled;
        };
        let target = if words.len() == 2 {
            let Some(values) = crate::console::declared_integer_options(entry) else {
                self.push_console_line(format!(
                        "^3toggle:^7 {name} has no declared discrete options; use `{name} !` for a logical 0/1 flip or pass explicit values"
                    ));
                return CommandResult::Handled;
            };
            values
                .iter()
                .position(|candidate| candidate == &current)
                .map(|index| values[(index + 1) % values.len()].clone())
                .unwrap_or_else(|| values[0].clone())
        } else if words.len() == 3 {
            self.push_console_line("^3toggle:^7 nothing to toggle to".to_owned());
            return CommandResult::Handled;
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
            Ok(ConsoleCvarSetResult::Applied) => {}
            Ok(ConsoleCvarSetResult::Latched) => {
                self.push_console_line(format!("^7{} will be changed upon restarting.", entry.name))
            }
            Ok(ConsoleCvarSetResult::Unchanged) => {}
            Err(error) => self.push_console_error(format!("^1{error}")),
        }
        return CommandResult::Handled;
    }

    cmd_set ["set", "seta", "sets", "setu"] ("set <variable> <value>", "Set a client cvar; legacy flag forms are accepted.", CommandScope::Always) |self, command| {
        // Cvar_Set_f family: `set <name> <value ...>` is `<name> <value>`.
        let set_form = command
            .split_once(char::is_whitespace)
            .map(|(_, rest)| rest.trim().to_owned());
        if let Some(rest) = set_form {
            let Some((name, value)) = rest.split_once(char::is_whitespace) else {
                self.push_console_line("^3usage:^7 set <variable> <value>");
                return CommandResult::Handled;
            };
            let value = value.trim().trim_matches('"');
            match self.set_console_cvar(name, value) {
                Ok(()) => {}
                Err(error) => self.push_console_error(format!("^1{error}")),
            }
            return CommandResult::Handled;
        }

        self.push_console_line("^3usage:^7 set <variable> <value>");
        CommandResult::Handled
    }

}
