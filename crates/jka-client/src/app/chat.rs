//! Chat.
use crate::app::{App, ChatMode, ChatPlayerCompletionCycle, ChatRecord, Instant, OverlayMode};

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
fn taystjk_chat_command(mode: ChatMode, message: &str) -> String {
    let message = taystjk_chat_wire_text(message);
    match mode {
        ChatMode::Global => format!("say \"{message}\""),
        ChatMode::Team => format!("say_team \"{message}\""),
        ChatMode::Direct(client) => format!("tell {client} \"{message}\""),
    }
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

    pub(in crate::app) fn begin_chat(&mut self, mode: ChatMode) {
        self.chat_mode = mode;
        self.chat_input.clear();
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

    pub(in crate::app) fn push_chat_line(&mut self, mode: ChatMode, message: &str) {
        let rendered = match mode {
            // Match JKA/OpenJK chat coloring: the name/punctuation finish in
            // white, then the server inserts the chat-mode color before text.
            ChatMode::Global => format!("^7YOU:^2 {message}"),
            ChatMode::Team => format!("^5(TEAM) ^7YOU:^5 {message}"),
            ChatMode::Direct(client) => format!("^6(TO #{client}) ^7YOU:^6 {message}"),
        };
        // A solo `say` never round-trips through a server `chat`/`tchat`
        // command, so the CG_ServerCommand beep has to be played here.
        let kind = match mode {
            ChatMode::Global => crate::cgame::ChatKind::Say,
            ChatMode::Team => crate::cgame::ChatKind::Team,
            ChatMode::Direct(_) => crate::cgame::ChatKind::Say,
        };
        self.play_chat_sound(kind, &rendered);
        if matches!(mode, ChatMode::Team) {
            self.push_console_team_chat_line(rendered.clone());
        } else {
            self.push_console_line(rendered.clone());
        }
        if self.chat_lines.len() >= crate::app::chat_anim::KEEP_LINES {
            self.chat_lines.pop_front();
        }
        let now = Instant::now();
        self.chat_lines.push_back(ChatRecord {
            text: rendered,
            team: matches!(mode, ChatMode::Team),
            created: now,
            demo_elapsed_ms: self.current_demo_elapsed_ms(now),
        });
        self.last_chat_refresh = Instant::now();
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
        self.chat_player_completion = Some(ChatPlayerCompletionCycle {
            word_start,
            candidates,
            index: 0,
        });
        self.publish_ui();
    }

    pub(in crate::app) fn submit_chat(&mut self) {
        self.chat_player_completion = None;
        let message = std::mem::take(&mut self.chat_input);
        if !message.is_empty() {
            // Port TaystJK Message_Key rather than feeding chat through the
            // generic unquoted command path. Do not trim the field: the stock
            // edit field preserves leading/trailing spaces inside the quoted
            // say argument.
            if self.live_connected() {
                // As with TaystJK, only the server echoes the `tell` (or say).
                self.forward_command_to_server(&taystjk_chat_command(self.chat_mode, &message));
            } else {
                // The solo shim has no server command round-trip.
                self.push_chat_line(self.chat_mode, &taystjk_chat_wire_text(&message));
            }
        }
        // set_overlay publishes the retained UI once; the new chat line is
        // already included in that snapshot, so avoid a duplicate full publish.
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
    use super::{taystjk_chat_command, taystjk_chat_wire_text, taystjk_last_attacker_slot};
    use crate::ui::ChatMode;

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
