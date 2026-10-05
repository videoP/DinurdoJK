//! Chat.
use crate::app::{App, ChatMode, ChatPlayerCompletionCycle, ChatRecord, Instant, OverlayMode};

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

    pub(in crate::app) fn push_chat_line(&mut self, mode: ChatMode, message: &str) {
        let rendered = match mode {
            // Match JKA/OpenJK chat coloring: the name/punctuation finish in
            // white, then the server inserts the chat-mode color before text.
            ChatMode::Global => format!("^7YOU:^2 {message}"),
            ChatMode::Team => format!("^5(TEAM) ^7YOU:^5 {message}"),
        };
        // A solo `say` never round-trips through a server `chat`/`tchat`
        // command, so the CG_ServerCommand beep has to be played here.
        let kind = match mode {
            ChatMode::Global => crate::cgame::ChatKind::Say,
            ChatMode::Team => crate::cgame::ChatKind::Team,
        };
        self.play_chat_sound(kind, &rendered);
        self.push_console_line(rendered.clone());
        if self.chat_lines.len() >= 16 {
            self.chat_lines.pop_front();
        }
        let now = Instant::now();
        self.chat_lines.push_back(ChatRecord {
            text: rendered,
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
        let message = message.trim();
        if !message.is_empty() {
            if self.live_connected() {
                // The server echoes chat back as a `chat`/`tchat` command.
                let verb = if self.chat_mode == ChatMode::Team {
                    "say_team"
                } else {
                    "say"
                };
                self.forward_command_to_server(&format!("{verb} {message}"));
            } else {
                self.push_chat_line(self.chat_mode, message);
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
