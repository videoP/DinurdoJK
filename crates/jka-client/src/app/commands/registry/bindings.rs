use super::{CommandDef, CommandResult, CommandScope};
use crate::app::{keybinds, App};
commands! {
    cmd_bind ["bind", "bindspec", "binddemo"] ("bind <key> [command]", "Bind a key to a command; bind <key> queries the current binding.", CommandScope::Always) |self, command| {
        let words = keybinds::split_command_words(command.trim_start_matches('/'));
        let Some(verb) = words.first() else {
            return CommandResult::Unhandled;
        };

        let spectator = verb.eq_ignore_ascii_case("bindspec");
        let demo = verb.eq_ignore_ascii_case("binddemo");
        let label = if demo { "binddemo" } else if spectator { "bindspec" } else { "bind" };
        match words.as_slice() {
            [_] => self.push_console_line(format!(
                "^3{label} <key> [command]^7 - attach a {}command to a key",
                if demo { "demo-only " } else if spectator { "spectator-only " } else { "" }
            )),
            [_, key_name] => match keybinds::parse_key(key_name) {
                Some(key) => {
                    let binding = if demo {
                        self.bindings.get_demo(key)
                    } else if spectator {
                        self.bindings.get_spectator(key)
                    } else {
                        self.bindings.get(key)
                    }
                    .map(str::to_owned);
                    match binding {
                        Some(binding) => self.push_console_line(format!(
                            "^7\"{}\" = \"{}\"{}",
                            keybinds::key_name(key),
                            binding,
                            if demo { " ^5[demo]" } else if spectator { " ^5[spectator]" } else { "" }
                        )),
                        None if demo => self.push_console_line(format!(
                            "^7\"{}\" has no demo override (inherits normal/spectator bind)",
                            keybinds::key_name(key)
                        )),
                        None if spectator => self.push_console_line(format!(
                            "^7\"{}\" has no spectator override (inherits normal bind)",
                            keybinds::key_name(key)
                        )),
                        None => self.push_console_line(format!(
                            "^7\"{}\" is not bound",
                            keybinds::key_name(key)
                        )),
                    }
                }
                None => self.push_console_line(format!("^1\"{key_name}\" isn't a valid key")),
            },
            [_, key_name, rest @ ..] => match keybinds::parse_key(key_name) {
                Some(key) => {
                    if demo {
                        let command = rest.join(" ");
                        if !keybinds::DEMO_CONTROL_ACTIONS.iter().any(|action| action.command == command) {
                            self.push_console_line(format!("^1Invalid demo action: {command}"));
                            return CommandResult::Handled;
                        }
                        self.bindings.set_demo(key, command);
                    } else if spectator {
                        self.bindings.set_spectator(key, rest.join(" "));
                    } else {
                        self.bindings.set(key, rest.join(" "));
                    }
                    self.refresh_bound_state();
                    self.mark_config_dirty();
                }
                None => self.push_console_line(format!("^1\"{key_name}\" isn't a valid key")),
            },
            [] => unreachable!(),
        }
        self.publish_ui();
        return CommandResult::Handled;
    }

    cmd_unbind ["unbind", "unbindspec", "unbinddemo"] ("unbind <key>", "Remove a key binding.", CommandScope::Always) |self, command| {
        let words = keybinds::split_command_words(command.trim_start_matches('/'));
        let Some(verb) = words.first() else {
            return CommandResult::Unhandled;
        };

        let spectator = verb.eq_ignore_ascii_case("unbindspec");
        let demo = verb.eq_ignore_ascii_case("unbinddemo");
        let label = if demo { "unbinddemo" } else if spectator { "unbindspec" } else { "unbind" };
        if words.len() != 2 {
            self.push_console_line(format!(
                "^3{label} <key>^7 - remove a {}key binding",
                if demo { "demo-only " } else if spectator { "spectator-only " } else { "" }
            ));
        } else if let Some(key) = keybinds::parse_key(&words[1]) {
            if demo {
                self.bindings.unbind_demo(key);
            } else if spectator {
                self.bindings.unbind_spectator(key);
            } else {
                self.bindings.unbind(key);
            }
            self.refresh_bound_state();
            self.mark_config_dirty();
        } else {
            self.push_console_line(format!("^1\"{}\" isn't a valid key", words[1]));
        }
        self.publish_ui();
        return CommandResult::Handled;
    }

    cmd_unbindall ["unbindall", "unbindspecall", "unbinddemoall"] ("unbindall", "Remove all key bindings.", CommandScope::Always) |self, command| {
        let words = keybinds::split_command_words(command.trim_start_matches('/'));
        let Some(verb) = words.first() else {
            return CommandResult::Unhandled;
        };

        if verb.eq_ignore_ascii_case("unbinddemoall") {
            self.bindings.clear_demo();
        } else if verb.eq_ignore_ascii_case("unbindspecall") {
            self.bindings.clear_spectator();
        } else {
            self.bindings.clear();
        }
        self.refresh_bound_state();
        self.mark_config_dirty();
        self.publish_ui();
        return CommandResult::Handled;
    }

    cmd_bindlist ["bindlist", "bindspeclist", "binddemolist"] ("bindlist", "List all current key bindings.", CommandScope::Always) |self, command| {
        let words = keybinds::split_command_words(command.trim_start_matches('/'));
        let Some(verb) = words.first() else {
            return CommandResult::Unhandled;
        };

        let spectator = verb.eq_ignore_ascii_case("bindspeclist");
        let demo = verb.eq_ignore_ascii_case("binddemolist");
        let entries = if demo {
            self.bindings.demo_sorted()
        } else if spectator {
            self.bindings.spectator_sorted()
        } else {
            self.bindings.sorted()
        };
        for (key, binding) in entries {
            self.push_console_line(format!(
                "^7{} \"{}\"{}",
                keybinds::key_name(key),
                binding,
                if demo { " ^5[demo]" } else if spectator { " ^5[spectator]" } else { "" }
            ));
        }
        return CommandResult::Handled;
    }

}
