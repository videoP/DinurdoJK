//! Stream-only 2D redaction. Original UI snapshots, console history, logs and
//! protocol messages are never modified. Apply to a *separate* stream snapshot.
use crate::ui::UiSnapshot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamerSettings {
    pub enabled: bool,
    pub hide_private_messages: bool,
    pub hide_console: bool,
}
impl Default for StreamerSettings {
    fn default() -> Self {
        Self { enabled: false, hide_private_messages: true, hide_console: true }
    }
}

/// JKA chat has no protocol-level private-message flag. jaPRO uses the
/// `^7]: ^6` separator; recognize only explicit alternative PM tags instead
/// of broad substring guesses that would mask normal public chat.
pub fn is_private_message(text: &str) -> bool {
    if text.contains("^7]: ^6") { return true; }
    let plain = crate::logging::strip_jka_colors(text);
    let plain = plain.trim_start_matches(|c: char| c == '\x19' || c == ' ' || c == '\t');
    let lower = plain.to_ascii_lowercase();
    lower.starts_with("(to #")
        || lower.starts_with("/tell ")
        || lower.starts_with("tell ")
        || lower.starts_with("/etell ")
        || lower.starts_with("etell ")
        || lower.starts_with("[pm]")
        || lower.starts_with("[pm ")
        || lower.starts_with("[private]")
        || lower.starts_with("[private message]")
        || lower.starts_with("private message from ")
        || lower.starts_with("private message to ")
        || lower.starts_with("tell from ")
        || lower.starts_with("tell to ")
}

const PRIVATE_MASK: &str = "^7[PRIVATE MESSAGE] ********";
const CONSOLE_MASK: &str = "********";

/// Operates on a snapshot exclusively owned by the OBS presenter. Keep local
/// snapshots untouched, including the local console's command input/cursor.
pub fn redact_stream_snapshot(snapshot: &mut UiSnapshot, options: StreamerSettings) {
    if !options.enabled { return; }
    redact_stream_transients(&mut snapshot.chat_lines, &mut snapshot.console_notify_lines, options);
    if options.hide_private_messages {
        if matches!(snapshot.chat_mode, crate::ui::ChatMode::Direct(_)) && !snapshot.chat_input.is_empty() {
            snapshot.chat_input = CONSOLE_MASK.to_owned();
        }
        if is_private_message(&snapshot.console_input) {
            snapshot.console_input = CONSOLE_MASK.to_owned();
            snapshot.console_cursor = snapshot.console_input.len();
        }
        for line in &mut snapshot.console_lines {
            if is_private_message(line) { *line = PRIVATE_MASK.to_owned(); }
        }
        if is_private_message(&snapshot.console_status) {
            snapshot.console_status = PRIVATE_MASK.to_owned();
        }
    }
    if options.hide_console {
        // Mask rather than omit all scrollback to make it visibly clear that
        // the console is open without exposing server addresses or commands.
        for line in &mut snapshot.console_lines {
            *line = CONSOLE_MASK.to_owned();
        }
        for line in &mut snapshot.console_notify_lines {
            line.text = CONSOLE_MASK.to_owned();
        }
        if !snapshot.console_input.is_empty() {
            snapshot.console_input = CONSOLE_MASK.to_owned();
        }
        snapshot.console_cursor = snapshot.console_input.len();
        if !snapshot.console_status.is_empty() {
            snapshot.console_status = CONSOLE_MASK.to_owned();
        }
        snapshot.console_suggestions.clear();
        snapshot.console_suggest_total = 0;
        snapshot.console_suggest_selected = 0;
        snapshot.console_suggest_hint = false;
        snapshot.console_help_hover = false;
        snapshot.console_search_query.clear();
        snapshot.console_search_open = false;
        snapshot.console_search_matches.clear();
        snapshot.console_search_active = None;
        snapshot.console_selection = None;
        snapshot.console_path_links.clear();
    }
}

/// Same policy for the high-frequency `SetTransientUi` update path; a snapshot
/// is not regenerated for every incoming chat line in this client.
pub fn redact_stream_transients(
    chat: &mut [crate::ui::UiChatLine],
    notify: &mut [crate::ui::UiConsoleNotifyLine],
    options: StreamerSettings,
) {
    if !options.enabled { return; }
    if options.hide_private_messages {
        for line in chat {
            if is_private_message(&line.text) { line.text = PRIVATE_MASK.to_owned(); }
        }
    }
    for line in notify {
        if options.hide_console { line.text = CONSOLE_MASK.to_owned(); }
        else if options.hide_private_messages && is_private_message(&line.text) {
            line.text = PRIVATE_MASK.to_owned();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn japro_private_and_nonprivate() {
        assert!(is_private_message("^5Luke^7]: ^6secret"));
        assert!(is_private_message("[PM] Luke: secret"));
        assert!(is_private_message("^6(TO #2) ^7YOU:^6 secret"));
        assert!(is_private_message("^7Private message from Luke: secret"));
        assert!(!is_private_message("Luke: let's meet on duel3"));
        assert!(!is_private_message("Someone: this is a pm test"));
    }
    #[test]
    fn stream_only_snapshot_does_not_modify_source() {
        let original = crate::ui::UiSnapshot {
            chat_lines: vec![crate::ui::UiChatLine { text: "^5Luke^7]: ^6secret".into(), alpha: 1.0 }],
            console_lines: vec!["/connect secret.server".into(), "^5Luke^7]: ^6secret".into()],
            console_input: "rcon password".into(),
            console_status: "secret.server".into(),
            ..Default::default()
        };
        let mut stream = crate::ui::UiSnapshot {
            chat_lines: original.chat_lines.clone(),
            console_lines: original.console_lines.clone(),
            console_input: original.console_input.clone(),
            console_status: original.console_status.clone(),
            ..Default::default()
        };
        redact_stream_snapshot(&mut stream, StreamerSettings { enabled: true, hide_private_messages: true, hide_console: true });
        assert_eq!(stream.chat_lines[0].text, PRIVATE_MASK);
        assert_eq!(stream.console_lines[0], CONSOLE_MASK);
        assert_eq!(stream.console_input, CONSOLE_MASK);
        assert_eq!(original.console_input, "rcon password");
        assert_eq!(original.chat_lines[0].text, "^5Luke^7]: ^6secret");
    }
    #[test]
    fn separate_toggles() {
        let mut snapshot = crate::ui::UiSnapshot {
            console_lines: vec!["^5Luke^7]: ^6secret".into(), "normal".into()],
            ..Default::default()
        };
        redact_stream_snapshot(&mut snapshot, StreamerSettings { enabled: true, hide_private_messages: true, hide_console: false });
        assert_eq!(snapshot.console_lines[0], PRIVATE_MASK);
        assert_eq!(snapshot.console_lines[1], "normal");
    }
}
