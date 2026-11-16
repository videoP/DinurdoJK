//! Text input.
use crate::app::{App, ElementState, KeyCode, KeyEvent, OverlayMode};
use std::time::{Duration, Instant};

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

    /// The prompt is a single line, so the selection's moving end is always
    /// `console_cursor`. Both positions are UTF-8 boundaries.
    pub(in crate::app) fn console_input_selection_bounds(&self) -> Option<(usize, usize)> {
        let anchor = self.console_input_selection_anchor?;
        let cursor = self.console_cursor;
        (anchor != cursor).then_some((anchor.min(cursor), anchor.max(cursor)))
    }

    pub(in crate::app) fn clear_console_input_selection(&mut self) {
        self.console_input_selection_anchor = None;
        self.console_input_selecting = false;
    }

    pub(in crate::app) fn focus_console_input(&mut self) {
        self.console_input_focused = true;
        self.console_selection_anchor = None;
        self.console_selection_focus = None;
        self.console_selecting = false;
    }

    pub(in crate::app) fn console_input_byte_at_char(&self, column: usize) -> usize {
        self.console_input.char_indices().nth(column)
            .map_or(self.console_input.len(), |(byte, _)| byte)
    }

    /// The classic font treats ^N (and NWH's ^x/^X/^y/^Y) color escapes as
    /// zero-width; mouse placement must agree with the rendered glyph columns.
    pub(in crate::app) fn console_input_byte_at_visible_column(&self, column: usize) -> usize {
        let input = self.console_input.as_str();
        let bytes = input.as_bytes();
        let nwh_colors = self.console_nwh_hex_colors();
        let mut offset = 0;
        let mut visible = 0;
        while offset < input.len() {
            if visible == column { return offset; }
            if bytes[offset] == b'^' {
                let hex_length = if nwh_colors {
                    match bytes.get(offset + 1) {
                        Some(&b'x') => 3, Some(&b'X') => 6,
                        Some(&b'y') => 4, Some(&b'Y') => 8, _ => 0,
                    }
                } else { 0 };
                let escape_length = if hex_length > 0
                    && bytes.len() >= offset + 2 + hex_length
                    && bytes[offset + 2..offset + 2 + hex_length]
                        .iter().all(u8::is_ascii_hexdigit)
                { 2 + hex_length } else if bytes.get(offset + 1).is_some_and(u8::is_ascii_digit) {
                    2
                } else { 0 };
                if escape_length > 0 {
                    offset += escape_length;
                    continue;
                }
            }
            let ch = input[offset..].chars().next().expect("valid UTF-8 boundary");
            offset += ch.len_utf8();
            visible += 1;
        }
        input.len()
    }

    pub(in crate::app) fn begin_console_input_selection(&mut self, cursor: usize) {
        // A pointer edit changes which token/argument is active. Stop using
        // the query captured for arrow-key completion previews.
        self.console_suggest_origin = None;
        self.console_suggest_picked = false;
        const MULTI_CLICK_WINDOW: Duration = Duration::from_millis(450);
        let cursor = cursor.min(self.console_input.len());
        let column = self.console_input[..cursor].chars().count();
        let now = Instant::now();
        let continues = self.console_input_last_click.is_some_and(|(last, old)| {
            now.saturating_duration_since(last) <= MULTI_CLICK_WINDOW && old.abs_diff(column) <= 1
        });
        self.console_input_click_count = super::console_state::console_next_click_count(
            continues, self.console_input_click_count,
        );
        self.console_input_last_click = Some((now, column));
        self.focus_console_input();
        self.console_last_click = None;
        self.console_click_count = 0;
        match self.console_input_click_count {
            2 => {
                if let Some((start, end)) = super::console_state::console_word_range(&self.console_input, column) {
                    self.console_input_selection_anchor = Some(self.console_input_byte_at_char(start));
                    self.console_cursor = self.console_input_byte_at_char(end);
                    self.console_input_selecting = false;
                } else {
                    self.console_input_selection_anchor = Some(cursor);
                    self.console_cursor = cursor;
                    self.console_input_selecting = true;
                }
            }
            3 => {
                self.console_input_selection_anchor = Some(0);
                self.console_cursor = self.console_input.len();
                self.console_input_selecting = false;
            }
            _ => {
                self.console_input_selection_anchor = Some(cursor);
                self.console_cursor = cursor;
                self.console_input_selecting = true;
            }
        }
    }

    pub(in crate::app) fn finish_console_input_selection(&mut self) {
        self.console_input_selecting = false;
        if self.console_input_selection_bounds().is_some() && self.console_input_click_count == 1 {
            // A drag is not the first click of a future double-click sequence.
            self.console_input_last_click = None;
            self.console_input_click_count = 0;
        }
        if self.console_input_selection_anchor == Some(self.console_cursor) {
            self.console_input_selection_anchor = None;
        }
    }

    fn delete_console_input_selection(&mut self) -> bool {
        let Some((start, end)) = self.console_input_selection_bounds() else { return false; };
        self.console_input.replace_range(start..end, "");
        self.console_cursor = start;
        self.clear_console_input_selection();
        true
    }

    fn move_console_input_cursor(&mut self, cursor: usize, extend_selection: bool) {
        if extend_selection {
            if self.console_input_selection_anchor.is_none() {
                self.console_input_selection_anchor = Some(self.console_cursor);
            }
        } else {
            self.clear_console_input_selection();
        }
        self.console_cursor = cursor;
        self.focus_console_input();
    }

    pub(in crate::app) fn insert_console_text(&mut self, value: &str) -> bool {
        self.console_cursor = self.console_cursor.min(self.console_input.len());
        let deleted = self.delete_console_input_selection();
        self.focus_console_input();
        let before = self.console_input.len();
        let mut used = crate::cgame::unicode_input_wire_len(&self.console_input);
        for ch in value.chars() {
            if ch.is_control() || matches!(ch, '`' | '~') {
                continue;
            }
            let len = if ch as u32 <= 0xff { 1 } else { ch.len_utf8() };
            if used + len > 160 { break; }
            self.console_input.insert(self.console_cursor, ch);
            self.console_cursor += ch.len_utf8();
            used += len;
        }
        deleted || self.console_input.len() != before
    }

    /// Paste text into the console. A normal one-line paste remains editable;
    /// once the clipboard contains a line ending, every pasted line is submitted
    /// exactly like pressing Enter after it.
    pub(in crate::app) fn paste_console_text(&mut self, value: &str) -> bool {
        if !value.contains('\r') && !value.contains('\n') {
            return self.insert_console_text(value);
        }

        let mut changed = false;
        let mut segment_start = 0;
        let mut chars = value.char_indices().peekable();

        while let Some((index, ch)) = chars.next() {
            if !matches!(ch, '\r' | '\n') {
                continue;
            }

            self.insert_console_text(&value[segment_start..index]);
            self.execute_console();
            changed = true;

            // Windows clipboard text normally uses CRLF. Treat that pair as one
            // Enter instead of submitting a second empty command for the LF.
            if ch == '\r'
                && chars.peek().is_some_and(|(_, next)| *next == '\n')
            {
                let (lf_index, _) = chars.next().expect("peeked CRLF continuation");
                segment_start = lf_index + '\n'.len_utf8();
            } else {
                segment_start = index + ch.len_utf8();
            }
        }

        // A copied block usually has no newline after its last command. Since a
        // multiline paste is command-oriented, submit that final line as well.
        if segment_start < value.len() {
            self.insert_console_text(&value[segment_start..]);
            self.execute_console();
            changed = true;
        }

        changed
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
                    if self.console_input_focused && !self.console_input.is_empty() {
                        self.console_input_selection_anchor = Some(0);
                        self.console_cursor = self.console_input.len();
                        self.console_input_selecting = false;
                    } else {
                        self.select_all_console();
                    }
                    self.publish_ui();
                    return;
                }
                KeyCode::KeyC => {
                    if let Some((start, end)) = self.console_input_selection_bounds() {
                        if let Err(error) = crate::clipboard::set_text(&self.console_input[start..end]) {
                            self.console_status = format!("CLIPBOARD ERROR: {error}");
                            self.push_console_line(format!("^1{}", self.console_status));
                        }
                    } else {
                        self.copy_console_selection();
                    }
                    self.publish_ui();
                    return;
                }
                KeyCode::KeyX => {
                    if let Some((start, end)) = self.console_input_selection_bounds() {
                        match crate::clipboard::set_text(&self.console_input[start..end]) {
                            Ok(()) => {
                                self.delete_console_input_selection();
                                self.console_history_index = None;
                                self.console_suggest_reset();
                            }
                            Err(error) => {
                                self.console_status = format!("CLIPBOARD ERROR: {error}");
                                self.push_console_line(format!("^1{}", self.console_status));
                            }
                        }
                    }
                    self.publish_ui();
                    return;
                }
                KeyCode::KeyV => {
                    match crate::clipboard::get_text() {
                        Ok(value) => {
                            if self.paste_console_text(&value) {
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
                // Esc cancels an arrow-key preview instead of leaving the
                // completed name in a hidden suggestion popup.
                if let Some((input, cursor)) = self.console_suggest_origin.take() {
                    self.console_input = input;
                    self.console_cursor = cursor;
                }
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
                let cursor = if !self.modifiers.shift_key() {
                    self.console_input_selection_bounds().map(|(start, _)| start)
                } else { None }.unwrap_or_else(|| if self.modifiers.control_key() {
                    Self::console_word_left(&self.console_input, self.console_cursor)
                } else {
                    Self::console_prev_boundary(&self.console_input, self.console_cursor)
                });
                self.move_console_input_cursor(cursor, self.modifiers.shift_key());
                self.publish_ui();
            }
            KeyCode::ArrowRight => {
                let cursor = if !self.modifiers.shift_key() {
                    self.console_input_selection_bounds().map(|(_, end)| end)
                } else { None }.unwrap_or_else(|| if self.modifiers.control_key() {
                    Self::console_word_right(&self.console_input, self.console_cursor)
                } else {
                    Self::console_next_boundary(&self.console_input, self.console_cursor)
                });
                self.move_console_input_cursor(cursor, self.modifiers.shift_key());
                self.publish_ui();
            }
            KeyCode::Home => {
                self.move_console_input_cursor(0, self.modifiers.shift_key());
                self.publish_ui();
            }
            KeyCode::End => {
                self.move_console_input_cursor(self.console_input.len(), self.modifiers.shift_key());
                self.publish_ui();
            }
            KeyCode::PageUp => {
                if self.console_hl2 {
                    self.console_hl2_page_delta += 8;
                } else {
                    self.scroll_console(8);
                }
                self.publish_ui();
            }
            KeyCode::PageDown => {
                if self.console_hl2 {
                    self.console_hl2_page_delta -= 8;
                } else {
                    self.console_scroll = self.console_scroll.saturating_sub(8);
                }
                self.publish_ui();
            }
            KeyCode::Backspace => {
                self.console_history_index = None;
                self.console_scroll = 0;
                self.console_suggest_reset();
                self.focus_console_input();
                if !self.delete_console_input_selection() && self.console_cursor > 0 {
                    let previous = if self.modifiers.control_key() {
                        Self::console_word_left(&self.console_input, self.console_cursor)
                    } else {
                        Self::console_prev_boundary(&self.console_input, self.console_cursor)
                    };
                    self.console_input.drain(previous..self.console_cursor);
                    self.console_cursor = previous;
                }
                self.clear_console_input_selection();
                self.publish_ui();
            }
            KeyCode::Delete => {
                self.console_history_index = None;
                self.console_scroll = 0;
                self.console_suggest_reset();
                self.focus_console_input();
                if !self.delete_console_input_selection() && self.console_cursor < self.console_input.len() {
                    let next = if self.modifiers.control_key() {
                        Self::console_word_right(&self.console_input, self.console_cursor)
                    } else {
                        Self::console_next_boundary(&self.console_input, self.console_cursor)
                    };
                    self.console_input.drain(self.console_cursor..next);
                }
                self.clear_console_input_selection();
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
        // The two chatbox styles share this editor; the shortcuts must not
        // depend on cg_chatStyle or the renderer used for the field.
        if self.modifiers.control_key() && !event.repeat {
            match code {
                KeyCode::KeyA => {
                    self.chat_input_selected = !self.chat_input.is_empty();
                    self.chat_player_completion = None;
                    self.publish_ui();
                    return;
                }
                KeyCode::KeyC => {
                    if self.chat_input_selected && !self.chat_input.is_empty() {
                        if let Err(error) = crate::clipboard::set_text(&self.chat_input) {
                            self.console_status = format!("CLIPBOARD ERROR: {error}");
                            self.push_console_line(format!("^1{}", self.console_status));
                        }
                    }
                    self.publish_ui();
                    return;
                }
                KeyCode::KeyX => {
                    if self.chat_input_selected && !self.chat_input.is_empty() {
                        match crate::clipboard::set_text(&self.chat_input) {
                            Ok(()) => {
                                self.chat_input.clear();
                                self.chat_input_selected = false;
                                self.chat_player_completion = None;
                            }
                            Err(error) => {
                                self.console_status = format!("CLIPBOARD ERROR: {error}");
                                self.push_console_line(format!("^1{}", self.console_status));
                            }
                        }
                    }
                    self.publish_ui();
                    return;
                }
                _ => {}
            }
        }
        if self.modifiers.control_key() && code == KeyCode::KeyV && !event.repeat {
            match crate::clipboard::get_text() {
                Ok(value) => {
                    if self.chat_input_selected && value.chars().any(|ch| !ch.is_control()) {
                        self.chat_input.clear();
                        self.chat_input_selected = false;
                    }
                    let before = self.chat_input.len();
                    let mut used = crate::cgame::unicode_input_wire_len(&self.chat_input);
                    for ch in value.chars() {
                        if ch.is_control() {
                            continue;
                        }
                        let len = if ch as u32 <= 0xff { 1 } else { ch.len_utf8() };
                        if used + len > 160 { break; }
                        self.chat_input.push(ch);
                        used += len;
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
                self.chat_input_selected = false;
                self.chat_player_completion = None;
                self.set_overlay(OverlayMode::None);
            }
            KeyCode::Enter | KeyCode::NumpadEnter if !event.repeat => self.submit_chat(),
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
                if self.chat_input_selected {
                    self.chat_input.clear();
                    self.chat_input_selected = false;
                } else if self.modifiers.control_key() {
                    // Identical UTF-8/whitespace behavior to console Ctrl+Backspace.
                    let start = Self::console_word_left(&self.chat_input, self.chat_input.len());
                    self.chat_input.truncate(start);
                } else {
                    self.chat_input.pop();
                }
                self.publish_ui();
            }
            _ => {
                if self.modifiers.control_key() {
                    return;
                }
                if let Some(value) = &event.text {
                    if self.chat_input_selected && value.chars().any(|ch| !ch.is_control()) {
                        self.chat_input.clear();
                        self.chat_input_selected = false;
                    }
                    let mut changed = false;
                    let mut used = crate::cgame::unicode_input_wire_len(&self.chat_input);
                    for ch in value.chars() {
                        let len = if ch as u32 <= 0xff { 1 } else { ch.len_utf8() };
                        if !ch.is_control() && used + len <= 160
                        {
                            self.chat_input.push(ch);
                            used += len;
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

#[cfg(test)]
mod console_edit_tests {
    use super::App;

    #[test]
    fn ctrl_backspace_respects_whitespace_and_utf8_boundaries() {
        let input = "echo Привет мир";
        let start = App::console_word_left(input, input.len());
        assert_eq!(&input[..start], "echo Привет ");
        let next = App::console_word_left(input, start);
        assert_eq!(&input[..next], "echo ");
        assert_eq!(App::console_prev_boundary("é", "é".len()), 0);
        assert_eq!(App::console_next_boundary("é", 0), "é".len());
        // The chat editor uses the same word boundary helper for Ctrl+Backspace.
        let chat = "hello 你好 мир";
        assert_eq!(&chat[..App::console_word_left(chat, chat.len())], "hello 你好 ");
    }
}
