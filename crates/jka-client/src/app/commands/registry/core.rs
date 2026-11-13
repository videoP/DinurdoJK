//! Declarative command handlers.
use crate::app::{split_console_script, App, ConsoleCvarSetResult};

use super::{CommandDef, CommandResult, CommandScope};

// Many float-backed cvars display with fixed decimal places (e.g. 16.000 or
// 0.300), while binds normally use short forms (16, 0.3). Preserve exact text
// comparisons for string cvars, but treat equivalent finite numbers as equal.
fn toggle_value_matches(current: &str, candidate: &str) -> bool {
    current == candidate
        || matches!(
            (current.parse::<f64>(), candidate.parse::<f64>()),
            (Ok(lhs), Ok(rhs)) if lhs.is_finite() && rhs.is_finite() && lhs == rhs
        )
}

fn next_explicit_toggle_value<'a>(current: &str, values: &'a [&str]) -> &'a str {
    debug_assert!(!values.is_empty());
    values
        .iter()
        .take(values.len().saturating_sub(1))
        .position(|candidate| toggle_value_matches(current, candidate))
        .map(|index| values[index + 1])
        .unwrap_or(values[0])
}

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

    // TaystJK's Sys_Init registers this as a local, no-argument input reset.
    cmd_in_restart ["in_restart", ] ("in_restart", "Restart input state and reacquire native mouse capture without restarting the renderer or connection.", CommandScope::Always) |self, command| {
        if command.split_whitespace().count() != 1 {
            self.push_console_line("^3usage:^7 in_restart".to_owned());
        } else {
            self.restart_input_subsystem();
        }
        CommandResult::Handled
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

    // TaystJK codemp/client/snd_dma.cpp S_Play_f: accept multiple sound
    // names, append .wav when the name has no extension, register each and
    // start it on the local sound channel (without a volume argument).
    cmd_play ["play", ] ("play <sound> [sound ...]", "Play one or more local sound effects by asset path (TaystJK S_Play_f).", CommandScope::Always) |self, command| {
        let mut names = command.split_whitespace().skip(1).peekable();
        if names.peek().is_none() {
            self.push_console_line("^3usage:^7 play <sound> [sound ...]".to_owned());
            return CommandResult::Handled;
        }
        let Some(sound) = self.game_session.as_mut().and_then(|session| session.sound_presenter.as_mut()) else {
            self.push_console_line("^3play:^7 no active game sound system".to_owned());
            return CommandResult::Handled;
        };
        let mut failed = Vec::new();
        for name in names {
            let path = if name.contains('.') { name.to_owned() } else { format!("{name}.wav") };
            if !sound.play_local_sound(&[path.as_str()]) {
                failed.push(path);
            }
        }
        for path in failed {
            self.push_console_line(format!("^3play:^7 could not play {path}"));
        }
        CommandResult::Handled
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
        // OpenJK/TaystJK `wait N` advances by engine/client frames. In our
        // decoupled client the equivalent gameplay frame is an *authored*
        // usercmd, not a render frame or an approximate wall-clock interval.
        let args = jka_protocol::commands::tokenize(command.as_bytes());
        let frames = match args.as_slice() {
            [_verb, argument] => {
                let parsed = jka_protocol::commands::atoi(argument);
                if parsed < 0 { 1 } else { parsed as u32 }
            }
            [_verb] | [_verb, _, ..] => 1,
            _ => return CommandResult::Unhandled,
        };
        self.console_wait = if frames == 0 {
            None
        } else if self.net.as_ref().is_some_and(|net|
            net.state() >= jka_protocol::session::ConnectionState::Primed)
        {
            Some(crate::app::commands::buffer::ConsoleWait::UserCmds(frames))
        } else {
            // No live usercmd producer (menu/demo/loading): don't strand
            // config scripts on a movement clock that cannot advance.
            let rate = f64::from(self.network.command_rate.max(15));
            Some(crate::app::commands::buffer::ConsoleWait::Realtime(
                crate::app::Instant::now()
                    + crate::app::Duration::from_secs_f64(f64::from(frames) / rate),
            ))
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

    cmd_path ["path", "fs_path"] ("path", "Show the active VFS mount priority, writable location, and configured filesystem roots.", CommandScope::Always) |self, command| {
        if command.split_whitespace().count() != 1 {
            self.push_console_line("^3usage:^7 path");
            return CommandResult::Handled;
        }
        let effective = jka_assets::pk3::configured_filesystem_roots(&self.base);
        self.push_console_line(format!("^2FS:^7 basepath = {}", self.base.parent().unwrap_or(&self.base).display()));
        self.push_console_line(format!("^2FS:^7 homepath = {}", effective.homepath.as_ref().map_or("(JKA root)".into(), |p| p.display().to_string())));
        self.push_console_line(format!("^2FS:^7 cdpath = {}", effective.cdpath.as_ref().map_or("(none)".into(), |p| p.display().to_string())));
        self.push_console_line(format!("^2FS:^7 jk2basepath = {} (JK2 root containing base/)", jka_assets::pk3::jk2_root_directory(&self.base, &effective).map_or("(not set: JK2 disabled)".into(), |p| p.display().to_string())));
        self.push_console_line(format!("^2FS:^7 jk2homepath = {}", effective.jk2homepath.as_ref().map_or("(JK2 root)".into(), |p| p.display().to_string())));
        self.push_console_line(format!("^2FS:^7 jk2cdpath = {} (JK2 retail fallback; not mounted in JKA)", effective.jk2cdpath.as_ref().map_or("(none)".into(), |p| p.display().to_string())));
        self.push_console_line(format!("^2FS:^7 forcegame (JKA) = {}", effective.forcegame.as_deref().unwrap_or("(none)")));
        self.push_console_line(format!("^2FS:^7 jk2forcegame (JK2) = {}", effective.jk2forcegame.as_deref().unwrap_or("(none)")));
        self.push_console_line(format!("^2FS:^7 writes = {}", jka_assets::pk3::game_write_directory(&self.base, self.game.as_deref()).display()));
        for (i, dir) in jka_assets::pk3::filesystem_search_dirs(&self.base, self.game.as_deref(), &effective).iter().enumerate() {
            self.push_console_line(format!("^5FS:^7 {:>2}. {}", i + 1, dir.display()));
        }
        if self.fs_roots != effective {
            self.push_console_line("^3FS:^7 pending path changes; restart to apply");
        }
        CommandResult::Handled
    }

    cmd_which ["which", ] ("which <qpath> [all]", "Locate the winning file in an active PK3 or loose directory (TaystJK /which). Optional all lists shadowed files too.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        let args = match words.as_slice() {
            [_, path] => Some((*path, false)),
            [_, path, all] if all.eq_ignore_ascii_case("all") => Some((*path, true)),
            _ => None,
        };
        let Some((path, show_all)) = args else {
            self.push_console_line("^3usage:^7 which <qpath> [all]");
            return CommandResult::Handled;
        };
        let path = path.trim_start_matches(|c| c == '/' || c == '\\').replace('\\', "/");
        match jka_assets::pk3::AssetSearchPath::open_game(&self.base, self.game.as_deref()) {
            Ok(mut assets) => {
                // Match the actual game's asset-override setting, rather
                // than always claiming the highest-priority PK3 won.
                assets.set_allow_asset_overrides(self.video.allow_asset_overrides);
                let providers = assets.indexed_providers(&path);
                if providers.is_empty() {
                    self.push_console_line(format!("^3File not found:^7 \"{path}\""));
                } else {
                    for provider in providers.iter().filter(|info| show_all || info.selected) {
                        let label = if provider.selected { "^2active" } else { "^3shadowed" };
                        if provider.archive {
                            self.push_console_line(format!("^7File \"{path}\" found in \"{}\" ({label}^7)", provider.source.display()));
                        } else {
                            self.push_console_line(format!("^7File \"{path}\" found at \"{}\" ({label}^7, loose)", provider.source.display()));
                        }
                    }
                    if show_all { self.push_console_line(format!("^7{} provider(s) found", providers.len())); }
                }
            }
            Err(error) => self.push_console_error(format!("^1which:^7 {error}")),
        }
        CommandResult::Handled
    }

    cmd_fs_restart ["fs_restart", ] ("fs_restart", "Re-scan the active base/fs_game VFS and retry previously failed assets without clearing successful registrations.", CommandScope::Always) |self, command| {
        match command.split_whitespace().collect::<Vec<_>>().as_slice() {
            [_verb] => self.restart_filesystem(),
            _ => self.push_console_line("^3usage:^7 fs_restart".to_owned()),
        }
        CommandResult::Handled
    }

    cmd_fs_rebuild ["fs_rebuild", ] ("fs_rebuild", "Full PK3/filesystem rebuild: discard CGame asset caches and restart the renderer/world without disconnecting.", CommandScope::Always) |self, command| {
        match command.split_whitespace().collect::<Vec<_>>().as_slice() {
            [_verb] => {
                if let Err(error) = self.rebuild_filesystem() {
                    self.push_console_line(format!("^1FS_REBUILD:^7 {error}"));
                }
            }
            _ => self.push_console_line("^3usage:^7 fs_rebuild".to_owned()),
        }
        CommandResult::Handled
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
        if self.chat_unavailable() {
            self.console_status = "^3Chat is only available on multiplayer servers".to_owned();
            return CommandResult::Status;
        }
        if let Some((verb, message)) = command.split_once(char::is_whitespace) {
            if message.trim().is_empty() {
                self.push_console_line(format!(
                    "^3{}^7 [command] - Message text is required.",
                    verb
                ));
                return CommandResult::Handled;
            }
            self.forward_command_to_server(command);
            return CommandResult::Handled;
        }
        let verb = command.trim();
        self.console_status = format!("USAGE: {} <MESSAGE>", verb.to_ascii_uppercase());
        CommandResult::Status
    }

    // OpenJK CL_ServerStatus_f: an engine-side OOB query, not a command sent
    // through the connected game's reliable command stream.
    cmd_serverstatus ["serverstatus", ] ("serverstatus [server]", "Query server settings and players (current server or host[:port]), as in OpenJK.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => self.query_console_server_status(None),
            [_verb, address] => self.query_console_server_status(Some(address)),
            _ => self.push_console_line("^3usage:^7 serverstatus [server]"),
        }
        CommandResult::Handled
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
        let entry = crate::console::find(name);
        if entry.is_none() && self.network.user_cvar(name).is_none() {
            self.push_console_error(format!("^1Cvar {name} does not exist."));
            return CommandResult::Handled;
        }
        if entry.is_some_and(|entry| entry.kind != crate::console::EntryKind::Cvar) {
            self.push_console_error(format!("^1{name} is not a cvar."));
            return CommandResult::Handled;
        }
        let Some(current) = self.console_cvar_value(name) else {
            self.push_console_error(format!("^1Cvar {name} is not writable here."));
            return CommandResult::Handled;
        };
        let target = if words.len() == 2 {
            if let Some(entry) = entry {
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
            } else {
                // TaystJK toggles unknown-but-created cvars numerically.
                (if current.parse::<f64>().unwrap_or(0.0) == 0.0 { "1" } else { "0" }).to_owned()
            }
        } else if words.len() == 3 {
            self.push_console_line("^3toggle:^7 nothing to toggle to".to_owned());
            return CommandResult::Handled;
        } else {
            next_explicit_toggle_value(&current, &words[2..]).to_owned()
        };
        match self.set_console_cvar_from_console(name, &target) {
            Ok(ConsoleCvarSetResult::Applied) => {}
            Ok(ConsoleCvarSetResult::Latched) => {
                self.push_console_line(format!("^7{name} will be changed upon restarting."))
            }
            Ok(ConsoleCvarSetResult::Unchanged) => {}
            Err(error) => self.push_console_error(format!("^1{error}")),
        }
        return CommandResult::Handled;
    }

    cmd_set ["set", "seta", "sets", "setu"] ("set[u|a|s] <variable> <value>", "Create or set a cvar; setu marks userinfo, seta archives, sets marks serverinfo (TaystJK behavior).", CommandScope::Always) |self, command| {
        let (verb, rest) = command.split_once(char::is_whitespace)
            .map(|(verb, rest)| (verb, rest.trim())).unwrap_or((command, ""));
        let Some((name, value)) = rest.split_once(char::is_whitespace) else {
            if !rest.is_empty() {
                if let Some(value) = self.console_cvar_value(rest) {
                    self.push_console_line(format!("^3{rest}^7 = \"{value}\""));
                } else {
                    self.push_console_error(format!("^1Cvar {rest} does not exist."));
                }
            } else {
                self.push_console_line(format!("^3usage:^7 {verb} <variable> <value>"));
            }
            return CommandResult::Handled;
        };
        // `Cmd_ArgsFrom(2)` preserves a multi-word value. A surrounding
        // quoted pair is a single value just as in TaystJK.
        let value = value.trim().strip_prefix('"').and_then(|v| v.strip_suffix('"'))
            .unwrap_or(value.trim());
        if crate::console::find(name).is_some() {
            match self.set_console_cvar_from_console(name, value) {
                Ok(ConsoleCvarSetResult::Latched) => self.push_console_line(format!(
                    "^7{name} will be changed upon restarting.")),
                Ok(_) => {},
                Err(error) => self.push_console_error(format!("^1{error}")),
            }
        } else {
            let flags = if verb.eq_ignore_ascii_case("seta") { crate::net::UserCvarFlags::ARCHIVE }
                else if verb.eq_ignore_ascii_case("setu") { crate::net::UserCvarFlags::USERINFO }
                else if verb.eq_ignore_ascii_case("sets") { crate::net::UserCvarFlags::SERVERINFO }
                else { crate::net::UserCvarFlags::NONE };
            if let Err(error) = self.set_user_created_cvar(name, value, flags) {
                self.push_console_error(format!("^1{error}"));
            }
        }
        CommandResult::Handled
    }

}

#[cfg(test)]
mod explicit_toggle_tests {
    use super::{next_explicit_toggle_value, toggle_value_matches};

    #[test]
    fn cycles_float_cvars_even_when_getter_formats_three_decimal_places() {
        assert_eq!(next_explicit_toggle_value("16.000", &["16", "-16"]), "-16");
        assert_eq!(next_explicit_toggle_value("-16.000", &["16", "-16"]), "16");
        assert_eq!(next_explicit_toggle_value("0.300", &["0.3", "0"]), "0");
        assert_eq!(next_explicit_toggle_value("0.000", &["0.3", "0"]), "0.3");
    }

    #[test]
    fn cycles_multiple_values_and_defaults_to_first_when_unmatched() {
        assert_eq!(next_explicit_toggle_value("2.000", &["1", "2", "3"]), "3");
        assert_eq!(next_explicit_toggle_value("3.000", &["1", "2", "3"]), "1");
        assert_eq!(next_explicit_toggle_value("unknown", &["1", "2", "3"]), "1");
        assert_eq!(next_explicit_toggle_value("bar", &["foo", "bar"]), "foo");
    }

    #[test]
    fn preserves_string_matching_without_conflating_nonfinite_numbers() {
        assert!(toggle_value_matches("foo", "foo"));
        assert!(!toggle_value_matches("foo", "FOO"));
        assert!(!toggle_value_matches("NaN", "nan"));
        assert!(!toggle_value_matches("inf", "Infinity"));
    }
}
