//! Chat.
use crate::app::{App, ChatMode, ChatPlayerCompletionCycle, Instant, OverlayMode};

/// TaystJK `Message_Key` wire sanitization. JKA reliable commands are tokenized
/// text, so a literal double quote cannot safely be embedded inside the quoted
/// `say` argument. TaystJK replaces it with two apostrophes and replaces `%`
/// with the exact three legacy bytes it uses before queueing the command.
fn taystjk_chat_wire_text(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    for ch in message.chars() {
        match ch {
            '"' => out.push_str("''"),
            '%' => {
                // TaystJK writes raw byte 0xB0 followed by `/.`. DinurdoJK's
                // U+00xx strings are lossless byte containers, so U+00B0 maps
                // back to protocol byte 0xB0 in `text_to_jka_bytes`.
                out.push(char::from(0xB0));
                out.push('/');
                out.push('.');
            }
            _ => out.push(ch),
        }
    }
    out
}

/// TaystJK `Message_Key`: the intended recipient is a stable client slot, not
/// a player name, and `tell` is sent as a reliable client command.
#[cfg(test)]
fn taystjk_chat_command(mode: ChatMode, message: &str) -> String {
    let message = taystjk_chat_wire_text(message);
    match mode {
        ChatMode::Global => format!("say \"{message}\""),
        ChatMode::Team => format!("say_team \"{message}\""),
        ChatMode::Direct(client) => format!("tell {client} \"{message}\""),
    }
}

/// Chat is special: the protocol is a byte stream, not UTF-8 by definition.
/// Keep legacy <=255 as one wire byte, encode genuinely Unicode codepoints as
/// UTF-8, and truncate at a *character* boundary within Raven's 149-byte
/// message buffer. Ordinary commands/cvars continue using JKA byte encoding.
fn chat_command_bytes(mode: ChatMode, message: &str) -> Vec<u8> {
    let sanitized = taystjk_chat_wire_text(message);
    let mut bytes = match mode {
        ChatMode::Global => b"say \"".to_vec(),
        ChatMode::Team => b"say_team \"".to_vec(),
        ChatMode::Direct(client) => format!("tell {client} \"").into_bytes(),
    };
    append_chat_body(&mut bytes, &sanitized);
    bytes.push(b'"');
    bytes
}

fn append_chat_body(bytes: &mut Vec<u8>, message: &str) {
    const MAX_SAY_BYTES: usize = 149;
    let mut used = 0;
    let mut end = 0;
    for (index, ch) in message.char_indices() {
        let len = if ch as u32 <= 0xff { 1 } else { ch.len_utf8() };
        if used + len > MAX_SAY_BYTES { break; }
        used += len;
        end = index + ch.len_utf8();
    }
    bytes.extend(crate::cgame::unicode_chat_wire_bytes(&message[..end]));
}

/// Match the server's ConcatArgs interpretation, then cap the message before
/// encoding so server-side byte truncation cannot split a Unicode character.
/// Recipient names stay in the legacy encoding, like other name lookups.
pub(in crate::app) fn forwarded_chat_command_bytes(line: &str) -> Option<Vec<u8>> {
    let args = jka_protocol::commands::tokenize(line.as_bytes());
    let verb = std::str::from_utf8(args.first()?).ok()?;
    let first_body = match verb.to_ascii_lowercase().as_str() {
        "say" | "say_team" => 1,
        "tell" => 2,
        _ => return None,
    };
    if args.len() <= first_body { return None; }
    let mut bytes = verb.as_bytes().to_vec();
    if first_body == 2 {
        bytes.extend_from_slice(b" \"");
        bytes.extend(crate::cgame::text_to_jka_bytes(std::str::from_utf8(&args[1]).ok()?));
        bytes.push(b'"');
    }
    bytes.extend_from_slice(b" \"");
    let message = args[first_body..].iter()
        .map(|arg| std::str::from_utf8(arg)).collect::<Result<Vec<_>, _>>().ok()?.join(" ");
    append_chat_body(&mut bytes, &message);
    bytes.push(b'"');
    Some(bytes)
}

/// CG_LastAttacker/Con_MessageMode4_f. The attacker index comes from the
/// server, not an inferred hit direction or entity event.
fn taystjk_last_attacker_slot(had_damage_feedback: bool, attacker: i32) -> Option<u8> {
    if !had_damage_feedback {
        return None;
    }
    u8::try_from(attacker).ok().filter(|&slot| slot < 32)
}

impl App {
    pub(in crate::app) fn paste_into(
        target: &mut String,
        max_len: usize,
        reject_console_toggle: bool,
    ) -> Result<bool, String> {
        let value = crate::clipboard::get_text()?;
        let before = target.len();
        for ch in value.chars() {
            if ch.is_control() || (reject_console_toggle && matches!(ch, '`' | '~')) {
                continue;
            }
            if target.len() >= max_len {
                break;
            }
            target.push(ch);
        }
        Ok(target.len() != before)
    }

    /// Only a connected multiplayer server can receive chat. Demo playback,
    /// in-process solo games, and the menus must never manufacture local chat.
    pub(in crate::app) fn chat_unavailable(&self) -> bool {
        !self.live_connected()
    }

    pub(in crate::app) fn begin_chat(&mut self, mode: ChatMode) {
        if self.chat_unavailable() {
            return;
        }
        self.chat_mode = mode;
        self.chat_input.clear();
        self.chat_input_selected = false;
        self.chat_emoji_picker.open = false;
        self.chat_player_completion = None;
        self.set_overlay(OverlayMode::Chat);
    }

    /// TaystJK Con_MessageMode3_f / CG_CrosshairPlayer: capture the recipient
    /// when the key is pressed, not on Enter. Return None for an invalid slot.
    pub(in crate::app) fn message_mode_target(&mut self) -> Option<u8> {
        const PMF_FOLLOW: i32 = 4096;
        const MAX_CLIENTS: i32 = 32;
        if !self.live_connected() {
            return None;
        }
        let snapshot = self.game_session.as_ref()?.client_game.current_snapshot()?;
        let ps = &snapshot.player_state;
        if ps.field_i32("pm_flags").unwrap_or(0) & PMF_FOLLOW != 0 {
            return u8::try_from(ps.field_i32("clientNum")?)
                .ok()
                .filter(|&client| i32::from(client) < MAX_CLIENTS);
        }

        let now = Instant::now();
        self.update_crosshair_target_for_chat(now);
        let seen = self.crosshair_seen?;
        // CG_CrosshairPlayer accepts a last-seen client for one second.
        if now.saturating_duration_since(seen.at).as_millis() > 1000 {
            return None;
        }
        u8::try_from(seen.entity)
            .ok()
            .filter(|&client| i32::from(client) < MAX_CLIENTS)
    }

    /// TaystJK `CG_LastAttacker` reads the server-authored
    /// `playerState.persistant[PERS_ATTACKER]` (index 6). Like `cg.attackerTime`,
    /// the session flag must first be set by a real damage-event transition.
    /// This also avoids accidentally sending to slot zero before any damage.
    pub(in crate::app) fn message_mode_attacker(&self) -> Option<u8> {
        const PERS_ATTACKER: usize = 6;
        if !self.live_connected() {
            return None;
        }
        let session = self.game_session.as_ref()?;
        let snapshot = session.client_game.current_snapshot()?;
        taystjk_last_attacker_slot(
            session.attacker_damage_seen,
            snapshot.player_state.persistant[PERS_ATTACKER],
        )
    }

    /// TaystJK CG_TellTarget_f / CG_TellAttacker_f: route one console
    /// message to a resolved client slot, without opening the Chat overlay.
    pub(in crate::app) fn tell_selected_player(&mut self, command: &str, attacker: bool) {
        let client = if attacker { self.message_mode_attacker() } else { self.message_mode_target() };
        let Some(client) = client else { return; };
        let message = command.split_once(char::is_whitespace)
            .map(|(_, rest)| rest.trim()).unwrap_or("");
        // The CGame original passes Cmd_Args without quote delimiters.
        // Filter control characters before putting text on the reliable wire.
        let message = message.chars().filter(|ch| !ch.is_control()).collect::<String>();
        let safe = crate::cgame::unicode_input_to_jka_text(&message);
        if let Err(error) = self.queue_server_command(&format!("tell {client} {safe}")) {
            self.push_console_line(format!("^1tell:^7 {error}"));
        }
    }

    /// Names of every connected client except the local player, colour codes intact.
    pub(in crate::app) fn connected_player_names(&self) -> Vec<String> {
        const MAX_CLIENTS: usize = 32;
        let Some(session) = self.game_session.as_ref() else {
            return Vec::new();
        };
        // Online it is the connection's slot; in a local game the snapshot's
        // player state is the local player (nothing to follow).
        let own = self.net.as_ref().map_or_else(
            || {
                session
                    .client_game
                    .current_snapshot()
                    .and_then(|snapshot| snapshot.player_state.field_i32("clientNum"))
                    .unwrap_or(-1)
            },
            |net| net.session().client_num(),
        );
        (0..MAX_CLIENTS)
            .filter(|&client| i32::try_from(client).ok() != Some(own))
            .filter_map(|client| session.client_game.client_name(client))
            .filter(|name| !name.is_empty())
            .collect()
    }

    /// JAPP `CG_ChatboxTabComplete`, with deterministic cycling for ambiguous
    /// player names. The first Tab ranks candidates from the original word;
    /// repeated Tabs keep cycling that captured set instead of matching the
    /// already-completed replacement text.
    pub(in crate::app) fn complete_chat_player_name(&mut self) {
        use crate::console::PlayerNameCompletion;

        if let Some(cycle) = self.chat_player_completion.as_mut() {
            if !cycle.candidates.is_empty() {
                cycle.index = (cycle.index + 1) % cycle.candidates.len();
                let word_start = cycle.word_start;
                let name = cycle.candidates[cycle.index].clone();
                self.chat_input.truncate(word_start);
                self.chat_input.push_str(&name);
                self.chat_input.push(' ');
                self.chat_input_selected = false;
                self.publish_ui();
                return;
            }
        }

        let word_start = self
            .chat_input
            .rfind(char::is_whitespace)
            .map_or(0, |index| {
                index
                    + self.chat_input[index..]
                        .chars()
                        .next()
                        .map_or(1, char::len_utf8)
            });
        let query = self.chat_input[word_start..].to_owned();
        let mut candidates =
            match crate::console::complete_player_name(&query, &self.connected_player_names()) {
                PlayerNameCompletion::None => return,
                PlayerNameCompletion::One(name) => vec![name],
                PlayerNameCompletion::Many(names) => names,
            };

        // Chat input is capped at 160 bytes. Drop candidates that cannot replace
        // this word without overflowing rather than truncating a name/color code.
        candidates.retain(|name| word_start + name.len() + 1 <= 160);
        let Some(name) = candidates.first().cloned() else {
            return;
        };

        self.chat_input.truncate(word_start);
        self.chat_input.push_str(&name);
        self.chat_input.push(' ');
        self.chat_input_selected = false;
        self.chat_player_completion = Some(ChatPlayerCompletionCycle {
            word_start,
            candidates,
            index: 0,
        });
        self.publish_ui();
    }

    pub(in crate::app) fn submit_chat(&mut self) {
        // An input opened while connected cannot be submitted after a
        // disconnect or transition to a demo or in-process solo game.
        if self.chat_unavailable() {
            self.chat_input.clear();
            self.chat_input_selected = false;
            self.chat_player_completion = None;
            self.set_overlay(OverlayMode::None);
            return;
        }
        self.chat_player_completion = None;
        self.chat_input_selected = false;
        let message = std::mem::take(&mut self.chat_input);
        if !message.is_empty() {
            // Port TaystJK Message_Key rather than feeding chat through the
            // generic unquoted command path. Do not trim the field: the stock
            // edit field preserves leading/trailing spaces inside the quoted
            // say argument.
            // The server is the sole source of authoritative chat echoes.
            // Only chat takes this Unicode-capable wire path. No global change
            // to command, cvar, player-name or userinfo serialization.
            let command = chat_command_bytes(self.chat_mode, &message);
            if let Some(net) = self.net.as_mut() {
                if let Err(error) = net.session_mut().add_reliable_command(&command, false) {
                    self.push_console_line(format!("^1{error}"));
                }
            }
        }
        // Closing the chat overlay publishes the cleared input once.
        self.set_overlay(OverlayMode::None);
    }

    /// Whether the console is connected to a server; also tells the command
    /// registry whether jaPRO's server commands are available to complete.
    pub(in crate::app) fn console_scope(&self) -> bool {
        let connected = self.live_connected();
        crate::console::set_japro_server(
            connected && self.connected_server_mod() == crate::net::mod_support::ServerMod::Japro,
        );
        connected
    }
}

#[cfg(test)]
mod tests {
    use super::{taystjk_chat_command, chat_command_bytes, taystjk_chat_wire_text, taystjk_last_attacker_slot};
    use crate::ui::ChatMode;

    #[test]
    fn unicode_chat_forwarding_caps_console_and_explicit_commands() {
        use super::forwarded_chat_command_bytes;
        let body = format!("{}Ж", "a".repeat(148));
        assert_eq!(forwarded_chat_command_bytes(&format!("say \"{body}\"")),
            Some(format!("say \"{}\"", "a".repeat(148)).into_bytes()));
        assert_eq!(forwarded_chat_command_bytes("tell \"a b\" Привет"),
            Some("tell \"a b\" \"Привет\"".as_bytes().to_vec()));
        assert_eq!(forwarded_chat_command_bytes("say_team \"你好\""),
            Some("say_team \"你好\"".as_bytes().to_vec()));
        assert!(forwarded_chat_command_bytes("name Жук").is_none());
        assert!(forwarded_chat_command_bytes("tell 5").is_none());
    }

    #[test]
    fn unicode_chat_bytes_preserve_jka_and_utf8_without_splitting() {
        use crate::app::ChatMode;
        assert_eq!(chat_command_bytes(ChatMode::Global, "hello"), b"say \"hello\"");
        assert_eq!(chat_command_bytes(ChatMode::Global, "Привет"),
            [b"say \"".as_slice(), "Привет".as_bytes(), b"\""].concat());
        assert_eq!(chat_command_bytes(ChatMode::Global, "°"),
            [b"say \"".as_slice(), &[0xB0], b"\""].concat());
        let bytes = chat_command_bytes(ChatMode::Global, &"Ж".repeat(90));
        assert_eq!(bytes.len(), b"say \"".len() + 148 + 1);
        assert!(std::str::from_utf8(&bytes).is_ok());
    }

    #[test]
    fn legacy_alt_code_glyph_survives_chat_wire_path() {
        // This is the TaystJK result of U+2310 (Windows CP437 Alt+169):
        // a font byte, not UTF-8 U+2310 and not CP437 byte 0xA9.
        assert_eq!(chat_command_bytes(ChatMode::Global, &char::from(0x10).to_string()),
            b"say \"\x10\"");
        assert_eq!(chat_command_bytes(ChatMode::Global, &char::from(0xA9).to_string()),
            [b"say \"".as_slice(), &[0xA9], b"\""].concat());
    }

    #[test]
    fn unicode_chat_keeps_greek_and_middle_dot_distinct_from_cp437() {
        // The chat editor must retain semantic Unicode; CP437 conversion is
        // reserved for legacy name/byte paths, not pasted or typed chat text.
        let message = "Γεια σου · Привет 你好";
        let command = chat_command_bytes(ChatMode::Global, message);
        let body = &command[b"say \"".len()..command.len() - 1];
        let expected = crate::cgame::unicode_chat_wire_bytes(message);
        assert_eq!(body, expected.as_slice());
        // U+00B7 is a single Latin-1 protocol byte, not CP437's 0xFA ('ú').
        assert_eq!(body.iter().filter(|&&byte| byte == 0xB7).count(), 1);
        assert!(!body.contains(&0xFA));
        let received = crate::cgame::bytes_to_lossless_ascii(body);
        assert_eq!(crate::cgame::received_chat_for_display(&received, None), message);
    }

    #[test]
    fn messagemode3_sends_tell_to_captured_slot() {
        assert_eq!(taystjk_chat_command(ChatMode::Global, "hello"), "say \"hello\"");
        assert_eq!(taystjk_chat_command(ChatMode::Team, "hello"), "say_team \"hello\"");
        assert_eq!(taystjk_chat_command(ChatMode::Direct(5), "hello"), "tell 5 \"hello\"");
        assert_eq!(taystjk_chat_command(ChatMode::Direct(31), "hi \"X\""), "tell 31 \"hi ''X''\"");
    }

    #[test]
    fn last_attacker_is_unset_until_damage_and_validates_server_slot() {
        assert_eq!(taystjk_last_attacker_slot(false, 0), None);
        assert_eq!(taystjk_last_attacker_slot(true, 0), Some(0));
        assert_eq!(taystjk_last_attacker_slot(true, 31), Some(31));
        assert_eq!(taystjk_last_attacker_slot(true, -1), None);
        assert_eq!(taystjk_last_attacker_slot(true, 32), None);
    }

    #[test]
    fn taystjk_chat_wire_substitutions_match_message_key() {
        assert_eq!(taystjk_chat_wire_text("plain text"), "plain text");
        assert_eq!(taystjk_chat_wire_text("\""), "''");
        assert_eq!(
            taystjk_chat_wire_text("100%"),
            format!("100{}/.", char::from(0xB0))
        );
        assert_eq!(
            crate::cgame::text_to_jka_bytes(&taystjk_chat_wire_text("%\"")),
            vec![0xB0, b'/', b'.', b'\'', b'\'']
        );
    }
}
