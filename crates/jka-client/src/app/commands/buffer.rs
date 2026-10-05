//! Commands buffer.
use crate::app::{App, PathBuf};

/// A value in `[0, 1)` for gameplay choices that stock JKA makes with `rand()`.
pub(in crate::app) fn random_unit() -> f32 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let mut x = (nanos as u64) ^ ((nanos >> 64) as u64) ^ 0x9E37_79B9_7F4A_7C15;
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    (x >> 40) as f32 / (1u64 << 24) as f32
}

pub(in crate::app) fn normalize_cfg_qpath(name: &str) -> Result<PathBuf, String> {
    let name = name.trim().trim_matches('"');
    if name.is_empty() {
        return Err("config filename is empty".to_owned());
    }
    let mut path = PathBuf::from(name.replace('\\', "/"));
    if path.extension().is_none() {
        path.set_extension("cfg");
    }
    if !path
        .extension()
        .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case("cfg"))
    {
        return Err("Only the .cfg extension is supported".to_owned());
    }
    if path.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        )
    }) {
        return Err("config path must stay inside the active game directory".to_owned());
    }
    Ok(path)
}

pub(in crate::app) const MAX_CONSOLE_COMMAND_BUFFER_BYTES: usize = 128 * 1024;

pub(in crate::app) const MAX_CONSOLE_COMMANDS_PER_FRAME: usize = 4096;

pub(in crate::app) fn split_console_script(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut line_comment = false;
    let mut block_comment = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if line_comment {
            if c == '\n' || c == '\r' {
                line_comment = false;
                let command = current.trim();
                if !command.is_empty() {
                    out.push(command.to_owned());
                }
                current.clear();
            }
            i += 1;
            continue;
        }
        if block_comment {
            if c == '*' && next == Some('/') {
                block_comment = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if !quoted && c == '/' && next == Some('/') {
            line_comment = true;
            i += 2;
            continue;
        }
        if !quoted && c == '/' && next == Some('*') {
            block_comment = true;
            i += 2;
            continue;
        }
        if c == '"' {
            quoted = !quoted;
            current.push(c);
            i += 1;
            continue;
        }
        if !quoted && (c == ';' || c == '\n' || c == '\r') {
            let command = current.trim();
            if !command.is_empty() {
                out.push(command.to_owned());
            }
            current.clear();
            i += 1;
            continue;
        }
        current.push(c);
        i += 1;
    }
    let command = current.trim();
    if !command.is_empty() {
        out.push(command.to_owned());
    }
    out
}

impl App {
    pub(in crate::app) fn write_named_config(&mut self, name: &str) {
        let qpath = match normalize_cfg_qpath(name) {
            Ok(path) => path,
            Err(error) => {
                self.push_console_line(format!("^1{error}"));
                return;
            }
        };

        let file = qpath.file_name().map(|name| name.to_string_lossy());
        if file.as_deref().is_some_and(|name| {
            name.eq_ignore_ascii_case("default.cfg") || name.eq_ignore_ascii_case("mpdefault.cfg")
        }) {
            self.push_console_line(format!("^3The filename {} is reserved", qpath.display()));
            return;
        }

        // Mirror OpenJK writeconfig: bindings + archived cvars. The native
        // DinurdoJK.cfg already contains exactly that state, so flush it first
        // and then copy the generated snapshot into the active game directory.
        self.config_dirty = true;
        self.flush_config();
        let destination = self.game_write_path(&qpath);
        if destination == self.config_path {
            self.push_console_path_line(
                format!("^2Writing {}", destination.display()),
                destination.clone(),
            );
            return;
        }
        if let Some(parent) = destination.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                self.push_console_path_line(
                    format!("^1Couldn't write {}: {error}", destination.display()),
                    destination.clone(),
                );
                return;
            }
        }
        match std::fs::copy(&self.config_path, &destination) {
            Ok(_) => self.push_console_path_line(
                format!("^2Writing {}", destination.display()),
                destination.clone(),
            ),
            Err(error) => self.push_console_path_line(
                format!("^1Couldn't write {}: {error}", destination.display()),
                destination.clone(),
            ),
        }
    }

    pub(in crate::app) fn console_buffer_bytes(&self) -> usize {
        self.console_command_buffer
            .iter()
            .map(|command| command.len().saturating_add(1))
            .sum()
    }

    pub(in crate::app) fn insert_console_commands_front(&mut self, commands: Vec<String>) -> bool {
        let added = commands
            .iter()
            .map(|command| command.len().saturating_add(1))
            .sum::<usize>();
        if self.console_buffer_bytes().saturating_add(added) >= MAX_CONSOLE_COMMAND_BUFFER_BYTES {
            self.push_console_line("^1Cbuf_InsertText overflowed".to_owned());
            return false;
        }
        for command in commands.into_iter().rev() {
            self.console_command_buffer.push_front(command);
        }
        true
    }

    pub(in crate::app) fn append_console_commands(&mut self, commands: Vec<String>) -> bool {
        let added = commands
            .iter()
            .map(|command| command.len().saturating_add(1))
            .sum::<usize>();
        if self.console_buffer_bytes().saturating_add(added) >= MAX_CONSOLE_COMMAND_BUFFER_BYTES {
            self.push_console_line("^1Cbuf_AddText: overflow".to_owned());
            return false;
        }
        self.console_command_buffer.extend(commands);
        true
    }

    /// OpenJK Cbuf_Execute semantics: commands execute in order until the
    /// buffer empties or `wait` asks us to leave the remainder for later.
    pub(in crate::app) fn process_console_command_buffer(&mut self) -> bool {
        let mut executed = 0usize;
        self.console_buffer_running = true;
        loop {
            if self.console_map_barrier {
                if self.loading.is_some() {
                    break;
                }
                self.console_map_barrier = false;
            }
            if self.perf_sample.is_some() || self.quit_requested {
                break;
            }
            if self.console_wait_frames > 0 {
                self.console_wait_frames -= 1;
                break;
            }
            let Some(command) = self.console_command_buffer.pop_front() else {
                break;
            };
            self.execute_command_line(&command);
            executed += 1;
            // OpenJK has a fixed 128 KiB byte buffer but a self-reinserting
            // vstr/exec can otherwise spin forever in one host frame. Keep the
            // same ordering while yielding pathological scripts safely.
            if executed >= MAX_CONSOLE_COMMANDS_PER_FRAME {
                self.push_console_line(
                    "^3Cbuf_Execute:^7 command budget exhausted; remaining commands deferred"
                        .to_owned(),
                );
                break;
            }
        }
        self.console_buffer_running = false;
        if std::mem::take(&mut self.console_userinfo_dirty) {
            self.send_userinfo();
        }
        executed != 0
    }

    pub(in crate::app) fn exec_cfg_file(&mut self, name: &str, quiet: bool) {
        let qpath = match normalize_cfg_qpath(name) {
            Ok(path) => path,
            Err(error) => {
                self.push_console_line(format!("^1{error}"));
                return;
            }
        };
        let qpath_string = qpath.to_string_lossy().replace('\\', "/");
        const MAX_CFG_BYTES: usize = 4 * 1024 * 1024;
        let mut assets =
            match jka_assets::pk3::AssetSearchPath::open_game(&self.base, self.game.as_deref()) {
                Ok(assets) => assets,
                Err(error) => {
                    self.push_console_line(format!("^1couldn't exec {qpath_string}: {error}"));
                    return;
                }
            };
        let asset = match assets.read(&qpath_string, MAX_CFG_BYTES) {
            Ok(Some(asset)) => asset,
            Ok(None) => {
                self.push_console_line(format!("^1couldn't exec {qpath_string}"));
                return;
            }
            Err(error) => {
                self.push_console_line(format!("^1couldn't exec {qpath_string}: {error}"));
                return;
            }
        };
        let text = String::from_utf8_lossy(&asset.bytes);
        if !quiet {
            self.push_console_line(format!("^2execing {qpath_string}"));
        }
        // Cbuf_InsertText: the script runs before commands that were already
        // waiting behind the `exec` invocation.
        self.insert_console_commands_front(split_console_script(&text));
    }

    pub(in crate::app) fn execute_console(&mut self) {
        // OpenJK-style convenience: Enter accepts a uniquely abbreviated first
        // command/cvar token, while ambiguous abbreviations remain untouched.
        self.complete_unique_console_command();
        // Submitting returns to the live bottom so the echo and any reply
        // (local or from the server) are visible without manual scrolling.
        self.console_scroll = 0;
        let command = std::mem::take(&mut self.console_input);
        self.console_cursor = 0;
        self.console_suggest_reset();
        let command = command.trim().to_owned();
        if command.is_empty() {
            return;
        }

        self.push_console_line(format!("^7] {command}"));
        if self
            .console_history
            .last()
            .map_or(true, |previous| previous != &command)
        {
            self.console_history.push(command.clone());
            if self.console_history.len() > 512 {
                self.console_history.remove(0);
            }
        }
        self.console_history_index = None;
        self.append_console_commands(split_console_script(&command));
    }

    pub(in crate::app) fn plugin_disable_command(&mut self, args: &[&str]) {
        match args {
            [] => {
                self.push_console_line("^3num ^3Enabled ^3Name".to_owned());
                for option in crate::japro_cg::JAPRO_PLUGIN_DISABLE_OPTIONS {
                    let enabled = option.enabled(self.network.plugin_disable);
                    self.push_console_line(format!(
                        "^7{:>3}   {}       ^7{}",
                        option.bit,
                        if enabled { "^2X" } else { "^8-" },
                        option.label
                    ));
                }
                self.push_console_line(format!(
                    "^8cp_pluginDisable = {}^7  (/plugin <num> toggles one option)",
                    self.network.plugin_disable
                ));
            }
            [number] => {
                let Ok(bit) = number.parse::<u8>() else {
                    self.push_console_line("^3usage:^7 plugin [bit number]".to_owned());
                    return;
                };
                let Some(option) = crate::japro_cg::plugin_disable_option(bit) else {
                    self.push_console_line(format!(
                        "^1plugin:^7 bit {bit} is not a TaystJK jaPRO plugin-disable option; use /plugin to list them"
                    ));
                    return;
                };
                let bits = self.network.plugin_disable ^ option.mask();
                match self.set_console_cvar("cp_pluginDisable", &bits.to_string()) {
                    Ok(()) => self.push_console_line(format!(
                        "^3plugin {}:^7 {} {}",
                        option.bit,
                        option.label,
                        if option.enabled(bits) {
                            "^2ON"
                        } else {
                            "^1OFF"
                        }
                    )),
                    Err(error) => self.push_console_line(format!("^1{error}")),
                }
            }
            _ => self.push_console_line("^3usage:^7 plugin [bit number]".to_owned()),
        }
    }
}
