//! Console dispatch through authoritative command and cvar definitions.
use crate::app::{App, ConsoleCvarSetResult};
impl App {
    pub(in crate::app) fn execute_command_line(&mut self, command: &str) {
        self.console_status.clear();
        let command = command.trim().trim_start_matches(['/', '\\']).trim_start();
        if command.is_empty() {
            return;
        }
        let words: Vec<_> = command.split_whitespace().collect();
        if words.first().is_some_and(|word| word.eq_ignore_ascii_case("cosmetics")) {
            self.command_cosmetics(&words);
            return;
        }

        if let Some(def) = super::registry::command_registry().find(words[0]) {
            match (def.handler)(self, command) {
                super::registry::CommandResult::Handled => return,
                super::registry::CommandResult::Status => {
                    if !self.console_status.is_empty() {
                        self.push_console_line(self.console_status.clone());
                    }
                    return;
                }
                super::registry::CommandResult::Unhandled => {}
            }
        }

        if let [name] = words.as_slice() {
            if let Some(entry) = crate::console::find(name) {
                if entry.kind == crate::console::EntryKind::Cvar {
                    let line = self.format_console_entry(entry);
                    self.push_console_line(line);
                    return;
                }
            } else if let Some(cvar) = self.network.user_cvar(name) {
                self.push_console_line(format!("^3{}^7 = \"{}\" (user-created)", cvar.name, cvar.value));
                return;
            }
        }
        if words.len() >= 2 && words[1] == "!" {
            let name = words[0];
            if let Some(entry) = crate::console::find(name) {
                if entry.kind == crate::console::EntryKind::Cvar {
                    let Some(current) = self.console_cvar_value(name) else {
                        self.push_console_error(format!("^1Cvar {name} is not writable here."));
                        return;
                    };
                    // OpenJK Cvar_Command semantics: a first argument of `!`
                    // sets the numeric cvar to logical-not(current value). This
                    // is what makes binds such as `cg_thirdPerson !` toggles.
                    let numeric = current.parse::<f32>().unwrap_or(0.0);
                    let target = if numeric == 0.0 { "1" } else { "0" };
                    match self.set_console_cvar(name, target) {
                        Ok(()) => {}
                        Err(error) => self.push_console_error(format!("^1{error}")),
                    }
                    return;
                }
            } else if let Some(current) = self.network.user_cvar(name).map(|cv| cv.value.clone()) {
                let numeric = current.parse::<f32>().unwrap_or(0.0);
                let target = if numeric == 0.0 { "1" } else { "0" };
                if let Err(error) = self.set_console_cvar(name, target) {
                    self.push_console_error(format!("^1{error}"));
                }
                return;
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
                        Ok(ConsoleCvarSetResult::Applied) => {}
                        Ok(ConsoleCvarSetResult::Latched) => self.push_console_line(format!(
                            "^7{} will be changed upon restarting.",
                            entry.name
                        )),
                        Ok(ConsoleCvarSetResult::Unchanged) => {}
                        Err(error) => self.push_console_error(format!("^1{error}")),
                    }
                    return;
                }
            } else if self.network.user_cvar(name).is_some() {
                let raw_value = raw_value.trim();
                let value = raw_value.strip_prefix('"').and_then(|v| v.strip_suffix('"'))
                    .unwrap_or(raw_value);
                if let Err(error) = self.set_console_cvar(name, value) {
                    self.push_console_error(format!("^1{error}"));
                }
                return;
            }
        }

        if self.live_connected() {
            self.forward_command_to_server(command);
            return;
        }
        self.console_status = format!("^1UNKNOWN COMMAND:^7 {command}");
        self.push_console_error(self.console_status.clone());
    }
}
