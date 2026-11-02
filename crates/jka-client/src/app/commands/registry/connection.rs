use super::{CommandDef, CommandResult, CommandScope};
use crate::app::App;
commands! {
    cmd_net_jk2_connect ["jk2_connect"] ("jk2_connect <1.02|1.04> <host[:port]>", "Connect to a JK2 server for spectating (default port 28070).", CommandScope::Always) |self, line| {
        let words: Vec<_> = line.split_whitespace().collect();
        match words.as_slice() {
            [_, version, target] => self.connect_to_jk2_server(version, target),
            _ => self.push_console_line("^3usage:^7 jk2_connect <1.02|1.04> <host[:port]>"),
        }
        CommandResult::Handled
    }

    cmd_net_connect ["connect"] ("connect <host[:port]>", "Connect to a server: connect <host[:port]> (default port 29070).", CommandScope::Always) |self, line| {
        let words: Vec<_> = line.split_whitespace().collect();
        match words.get(1) {
            Some(target) if words.len() == 2 => self.connect_to_server(target),
            _ => self.push_console_line("^3usage:^7 connect [server]"),
        }
        return CommandResult::Handled;
    }

    cmd_net_reconnect ["reconnect"] ("reconnect", "Reconnect to the last server.", CommandScope::Always) |self, _line| {
        if let Some(target) = self.reconnect_target.clone() {
            self.connect_to_server(&target);
        }
        return CommandResult::Handled;
    }

    cmd_net_cmd ["cmd"] ("cmd <command>", "Send the rest of the line to the server as a client command.", CommandScope::Always) |self, line| {
        // CL_ForwardToServer_f: requires CA_ACTIVE; argv(0) is not sent.
        let active = self
            .net
            .as_ref()
            .is_some_and(|net| net.state() == jka_protocol::session::ConnectionState::Active);
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
        return CommandResult::Handled;
    }

    cmd_net_userinfo ["userinfo"] ("userinfo", "Print the userinfo sent to the server.", CommandScope::Always) |self, _line| {
        let info = self.network.userinfo(&self.solo_client_info.model_cvar());
        self.push_console_line(format!("^3userinfo:^7 {}", String::from_utf8_lossy(&info)));
        return CommandResult::Handled;
    }

    cmd_net_serverdump ["serverdump"] ("serverdump", "Dump the server's serverinfo + systeminfo with derived numbers (estimated sv_fps from snapshot gaps, ping, command lag) to the console and <game>/serverinfo/<addr>_<time>.txt, and append a one-line summary to <game>/serverinfo/servers.log for comparing servers.", CommandScope::Always) |self, _line| {
        self.server_dump();
        return CommandResult::Handled;
    }

    cmd_net_serverinfo ["serverinfo", "systeminfo"] ("serverinfo", "Print the connected server's serverinfo configstring.", CommandScope::Always) |self, line| {
        let words: Vec<_> = line.split_whitespace().collect();
        let verb_lower = words.first().unwrap_or(&"").to_ascii_lowercase();
        if !self.live_connected() {
            return CommandResult::Unhandled;
        }
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
        return CommandResult::Handled;
    }

}
