//! Text input.
use crate::app::{App, ElementState, KeyCode, KeyEvent, OverlayMode};

impl App {
    pub(in crate::app) fn console_prev_boundary(input: &str, cursor: usize) -> usize {
        let cursor = cursor.min(input.len());
        input[..cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    pub(in crate::app) fn console_next_boundary(input: &str, cursor: usize) -> usize {
        let cursor = cursor.min(input.len());
        input[cursor..]
            .chars()
            .next()
            .map_or(cursor, |ch| cursor + ch.len_utf8())
    }

    pub(in crate::app) fn console_word_left(input: &str, cursor: usize) -> usize {
        let mut cursor = cursor.min(input.len());
        while cursor > 0 {
            let previous = Self::console_prev_boundary(input, cursor);
            let ch = input[previous..cursor].chars().next().unwrap();
            if !ch.is_whitespace() {
                break;
            }
            cursor = previous;
        }
        while cursor > 0 {
            let previous = Self::console_prev_boundary(input, cursor);
            let ch = input[previous..cursor].chars().next().unwrap();
            if ch.is_whitespace() {
                break;
            }
            cursor = previous;
        }
        cursor
    }

    pub(in crate::app) fn console_word_right(input: &str, cursor: usize) -> usize {
        let mut cursor = cursor.min(input.len());
        while cursor < input.len() {
            let next = Self::console_next_boundary(input, cursor);
            let ch = input[cursor..next].chars().next().unwrap();
            if ch.is_whitespace() {
                break;
            }
            cursor = next;
        }
        while cursor < input.len() {
            let next = Self::console_next_boundary(input, cursor);
            let ch = input[cursor..next].chars().next().unwrap();
            if !ch.is_whitespace() {
                break;
            }
            cursor = next;
        }
        cursor
    }

    pub(in crate::app) fn insert_console_text(&mut self, value: &str) -> bool {
        self.console_cursor = self.console_cursor.min(self.console_input.len());
        let before = self.console_input.len();
        for ch in value
            .chars()
            .map(crate::cgame::unicode_input_char_to_jka_char)
        {
            if ch.is_control() || matches!(ch, '`' | '~') {
                continue;
            }
            if crate::cgame::text_to_jka_bytes(&self.console_input).len() >= 160 {
                break;
            }
            self.console_input.insert(self.console_cursor, ch);
            self.console_cursor += ch.len_utf8();
        }
        self.console_input.len() != before
    }

    pub(in crate::app) fn handle_console_key(&mut self, event: &KeyEvent, code: KeyCode) {
        if event.state != ElementState::Pressed {
            return;
        }

        if self.modifiers.control_key() && code == KeyCode::KeyF && !event.repeat {
            self.open_console_search();
            return;
        }

        if self.console_search_open {
            if self.modifiers.control_key() && code == KeyCode::KeyV && !event.repeat {
                match Self::paste_into(&mut self.console_search_input, 128, false) {
                    Ok(true) => self.rebuild_console_search(true),
                    Ok(false) => {}
                    Err(error) => {
                        self.console_status = format!("CLIPBOARD ERROR: {error}");
                        self.push_console_line(format!("^1{}", self.console_status));
                    }
                }
                self.publish_ui();
                return;
            }

            match code {
                KeyCode::Escape => {
                    self.console_search_open = false;
                    self.publish_ui();
                }
                KeyCode::Enter | KeyCode::NumpadEnter if !event.repeat => {
                    self.step_console_search(self.modifiers.shift_key());
                    self.publish_ui();
                }
                KeyCode::F3 if !event.repeat => {
                    self.step_console_search(self.modifiers.shift_key());
                    self.publish_ui();
                }
                KeyCode::Backspace => {
                    self.console_search_input.pop();
                    self.rebuild_console_search(true);
                    self.publish_ui();
                }
                _ => {
                    if !self.modifiers.control_key() {
                        if let Some(value) = &event.text {
                            let mut changed = false;
                            for ch in value.chars() {
                                if !ch.is_control() && self.console_search_input.len() < 128 {
                                    self.console_search_input.push(ch);
                                    changed = true;
                                }
                            }
                            if changed {
                                self.rebuild_console_search(true);
                                self.publish_ui();
                            }
                        }
                    }
                }
            }
            return;
        }

        if self.modifiers.control_key() && !event.repeat {
            match code {
                KeyCode::KeyA => {
                    self.select_all_console();
                    self.publish_ui();
                    return;
                }
                KeyCode::KeyC => {
                    if let Some(text) = self.selected_console_text() {
                        if let Err(error) = crate::clipboard::set_text(&text) {
                            self.console_status = format!("CLIPBOARD ERROR: {error}");
                            self.push_console_line(format!("^1{}", self.console_status));
                        }
                    }
                    self.publish_ui();
                    return;
                }
                KeyCode::KeyV => {
                    match crate::clipboard::get_text() {
                        Ok(value) => {
                            if self.insert_console_text(&value) {
                                self.console_history_index = None;
                                self.console_scroll = 0;
                                self.console_suggest_reset();
                            }
                        }
                        Err(error) => {
                            self.console_status = format!("CLIPBOARD ERROR: {error}");
                            self.push_console_line(format!("^1{}", self.console_status));
                        }
                    }
                    self.publish_ui();
                    return;
                }
                _ => {}
            }
        }
        // While the live filter popup is up it owns Esc/Enter/Tab/Up/Down.
        let suggest_open = self.console_suggest_list_open();
        match code {
            KeyCode::Escape if suggest_open => {
                self.console_suggest_hidden = true;
                self.console_suggest_picked = false;
                self.publish_ui();
            }
            KeyCode::Escape => self.set_overlay(self.overlay_after_console()),
            KeyCode::Enter | KeyCode::NumpadEnter if !event.repeat => {
                if !self.accept_picked_console_suggestion() {
                    self.execute_console();
                }
                self.publish_ui();
            }
            KeyCode::Tab if !event.repeat && suggest_open => {
                self.tab_console_suggestion();
                self.publish_ui();
            }
            KeyCode::Tab if !event.repeat => self.complete_console_input(),
            KeyCode::ArrowUp if suggest_open => {
                self.step_console_suggestion(-1);
                self.publish_ui();
            }
            KeyCode::ArrowDown if suggest_open => {
                self.step_console_suggestion(1);
                self.publish_ui();
            }
            KeyCode::ArrowUp => self.browse_console_history(-1),
            KeyCode::ArrowDown => self.browse_console_history(1),
            KeyCode::ArrowLeft => {
                self.console_cursor = if self.modifiers.control_key() {
                    Self::console_word_left(&self.console_input, self.console_cursor)
                } else {
                    Self::console_prev_boundary(&self.console_input, self.console_cursor)
                };
                self.publish_ui();
            }
            KeyCode::ArrowRight => {
                self.console_cursor = if self.modifiers.control_key() {
                    Self::console_word_right(&self.console_input, self.console_cursor)
                } else {
                    Self::console_next_boundary(&self.console_input, self.console_cursor)
                };
                self.publish_ui();
            }
            KeyCode::Home => {
                self.console_cursor = 0;
                self.publish_ui();
            }
            KeyCode::End => {
                self.console_cursor = self.console_input.len();
                self.publish_ui();
            }
            KeyCode::PageUp => {
                self.scroll_console(8);
                self.publish_ui();
            }
            KeyCode::PageDown => {
                self.console_scroll = self.console_scroll.saturating_sub(8);
                self.publish_ui();
            }
            KeyCode::Backspace => {
                self.console_history_index = None;
                self.console_scroll = 0;
                self.console_suggest_reset();
                if self.console_cursor > 0 {
                    let previous =
                        Self::console_prev_boundary(&self.console_input, self.console_cursor);
                    self.console_input.drain(previous..self.console_cursor);
                    self.console_cursor = previous;
                }
                self.publish_ui();
            }
            KeyCode::Delete => {
                self.console_history_index = None;
                self.console_scroll = 0;
                self.console_suggest_reset();
                if self.console_cursor < self.console_input.len() {
                    let next =
                        Self::console_next_boundary(&self.console_input, self.console_cursor);
                    self.console_input.drain(self.console_cursor..next);
                }
                self.publish_ui();
            }
            _ => {
                if let Some(value) = &event.text {
                    if self.insert_console_text(value) {
                        self.console_history_index = None;
                        self.console_scroll = 0;
                        self.console_suggest_reset();
                        self.publish_ui();
                    }
                }
            }
        }
    }

    pub(in crate::app) fn handle_chat_key(&mut self, event: &KeyEvent, code: KeyCode) {
        if event.state != ElementState::Pressed {
            return;
        }
        if self.modifiers.control_key() && code == KeyCode::KeyV && !event.repeat {
            match crate::clipboard::get_text() {
                Ok(value) => {
                    let before = self.chat_input.len();
                    for ch in value
                        .chars()
                        .map(crate::cgame::unicode_input_char_to_jka_char)
                    {
                        if ch.is_control() {
                            continue;
                        }
                        if crate::cgame::text_to_jka_bytes(&self.chat_input).len() >= 160 {
                            break;
                        }
                        self.chat_input.push(ch);
                    }
                    if self.chat_input.len() != before {
                        self.chat_player_completion = None;
                    }
                }
                Err(error) => {
                    self.console_status = format!("CLIPBOARD ERROR: {error}");
                    self.push_console_line(format!("^1{}", self.console_status));
                }
            }
            self.publish_ui();
            return;
        }
        match code {
            KeyCode::Escape => {
                self.chat_input.clear();
                self.chat_player_completion = None;
                self.set_overlay(OverlayMode::None);
            }
            KeyCode::Enter if !event.repeat => self.submit_chat(),
            KeyCode::Tab => {
                if !event.repeat {
                    if self.chatbox_completion {
                        self.complete_chat_player_name();
                    } else {
                        self.chat_player_completion = None;
                    }
                }
            }
            KeyCode::Backspace => {
                self.chat_player_completion = None;
                self.chat_input.pop();
                self.publish_ui();
            }
            _ => {
                if self.modifiers.control_key() {
                    return;
                }
                if let Some(value) = &event.text {
                    let mut changed = false;
                    for ch in value
                        .chars()
                        .map(crate::cgame::unicode_input_char_to_jka_char)
                    {
                        if !ch.is_control()
                            && crate::cgame::text_to_jka_bytes(&self.chat_input).len() < 160
                        {
                            self.chat_input.push(ch);
                            changed = true;
                        }
                    }
                    if changed {
                        self.chat_player_completion = None;
                        self.publish_ui();
                    }
                }
            }
        }
    }

    /// Keyboard for Setup. egui owns every control on the page, so the only
    /// key left here is the one that backs out to the Game menu.
    pub(in crate::app) fn handle_video_key(&mut self, event: &KeyEvent, code: KeyCode) {
        if event.state != ElementState::Pressed || code != KeyCode::Escape {
            return;
        }
        if self.front_end {
            self.frontend_back();
        } else {
            // Setup is an overlay on the live game, just like the other in-game
            // menu pages. ESC should leave the menu completely. Switching back
            // to OverlayMode::Game while menu_selected is TOP_SETUP leaves the
            // normal menu pointing at a page it deliberately does not render,
            // producing a blank body that persists the next time ESC is pressed.
            self.set_overlay(OverlayMode::None);
        }
    }
}
