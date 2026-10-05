//! Console completion.
use crate::app::{ui, App, OverlayMode};

impl App {
    pub(in crate::app) fn format_console_entry(&self, entry: &crate::console::Entry) -> String {
        match entry.kind {
            crate::console::EntryKind::Command => {
                format!("^3{}^7 [command] - {}", entry.name, entry.description)
            }
            crate::console::EntryKind::ServerCommand => {
                format!("^3{}^7 [server] - {}", entry.name, entry.description)
            }
            crate::console::EntryKind::Cvar => {
                let value = self
                    .console_cvar_value(entry.name)
                    .unwrap_or_else(|| "?".into());
                let latched = self
                    .latched_console_cvars
                    .get(&entry.name.to_ascii_lowercase())
                    .map(|value| format!("  latched: \"{}\"", value))
                    .unwrap_or_default();
                if entry.range.is_empty() {
                    format!(
                        "^3{}^7 = \"{}\"{}  default: \"{}\"  - {}",
                        entry.name, value, latched, entry.default, entry.description
                    )
                } else {
                    format!(
                        "^3{}^7 = \"{}\"{}  default: \"{}\"  range: {}  - {}",
                        entry.name, value, latched, entry.default, entry.range, entry.description
                    )
                }
            }
        }
    }

    pub(in crate::app) fn console_first_token_span(input: &str) -> Option<(usize, usize)> {
        let start = input.find(|ch: char| !ch.is_whitespace())?;
        let end = input[start..]
            .find(char::is_whitespace)
            .map_or(input.len(), |offset| start + offset);
        Some((start, end))
    }

    pub(in crate::app) fn replace_console_first_token(
        &mut self,
        replacement: &str,
        append_space: bool,
    ) {
        let Some((start, end)) = Self::console_first_token_span(&self.console_input) else {
            return;
        };
        self.console_input.replace_range(start..end, replacement);
        if append_space && self.console_input.len() == start + replacement.len() {
            self.console_input.push(' ');
        }
        self.console_cursor = self.console_input.len();
        self.console_history_index = None;
        self.console_scroll = 0;
    }

    pub(in crate::app) fn complete_console_input(&mut self) {
        let Some((start, end)) = Self::console_first_token_span(&self.console_input) else {
            self.push_console_line("^7Type a command/cvar prefix, then press Tab to complete.");
            self.publish_ui();
            return;
        };
        let prefix = self.console_input[start..end].to_owned();
        if prefix.is_empty() {
            return;
        }

        // An exact registered name always wins, even if it is itself a prefix of
        // another command. Otherwise a single prefix match is unambiguous.
        let connected = self.console_scope();
        if let Some(entry) = crate::console::find_command(&prefix, connected)
            .or_else(|| crate::console::unique_prefix(&prefix, connected))
        {
            self.replace_console_first_token(entry.name, true);
            self.publish_ui();
            return;
        }

        let mut matches: Vec<_> = crate::console::prefix(&prefix, connected)
            .copied()
            .collect();
        if matches.is_empty() {
            self.push_console_line(format!("^1No commands or cvars start with '{prefix}'."));
            self.publish_ui();
            return;
        }

        if let Some(common) = crate::console::common_prefix(&prefix, connected) {
            if common.len() > prefix.len() || common != prefix {
                self.replace_console_first_token(&common, false);
            }
        }

        matches.sort_by_key(|entry| entry.name.to_ascii_lowercase());
        self.push_console_line(format!("^5{} match(es) for '{}':^7", matches.len(), prefix));
        for entry in matches {
            let line = self.format_console_entry(&entry);
            self.push_console_line(line);
        }
        self.publish_ui();
    }

    /// Rows for the live filter popup. The list is the first token while the
    /// caret is inside it; once the caret moves into the arguments of a
    /// complete name it collapses to a single non-interactive hint row.
    /// Returns `(rows, total_matches, is_hint)`.
    pub(in crate::app) fn console_suggest_state(
        &self,
    ) -> Option<(Vec<crate::console::Suggestion>, usize, bool)> {
        if !self.console_suggest || self.overlay != OverlayMode::Console || self.console_search_open
        {
            return None;
        }
        let (start, end) = Self::console_first_token_span(&self.console_input)?;
        let token = &self.console_input[start..end];
        let connected = self.console_scope();
        if self.console_cursor > end {
            let entry = crate::console::find_command(token, connected)?;
            let hit = crate::console::Suggestion {
                entry,
                tier: crate::console::MatchTier::Exact,
                mask: 0,
            };
            return Some((vec![hit], 1, true));
        }
        if self.console_suggest_hidden {
            return None;
        }
        let (hits, total) = crate::console::suggest(token, connected, 24);
        (!hits.is_empty()).then_some((hits, total, false))
    }

    /// Whether the interactive (non-hint) popup is up, which is what claims
    /// Up/Down/Tab/Esc/Enter from history and completion.
    pub(in crate::app) fn console_suggest_list_open(&self) -> bool {
        matches!(self.console_suggest_state(), Some((_, _, false)))
    }

    pub(in crate::app) fn console_suggest_snapshot(
        &self,
    ) -> (Vec<ui::ConsoleSuggestion>, usize, bool) {
        let Some((hits, total, hint)) = self.console_suggest_state() else {
            return (Vec::new(), 0, false);
        };
        let rows = hits
            .iter()
            .map(|hit| {
                let entry = hit.entry;
                let is_cvar = entry.kind == crate::console::EntryKind::Cvar;
                let value = if is_cvar {
                    self.console_cvar_value(entry.name).unwrap_or_default()
                } else {
                    String::new()
                };
                let same = |a: &str, b: &str| {
                    a.trim() == b.trim()
                        || matches!((a.trim().parse::<f64>(), b.trim().parse::<f64>()), (Ok(a), Ok(b)) if a == b)
                };
                ui::ConsoleSuggestion {
                    name: entry.name,
                    kind: match entry.kind {
                        crate::console::EntryKind::Cvar => ui::ConsoleSuggestKind::Cvar,
                        crate::console::EntryKind::Command => ui::ConsoleSuggestKind::Command,
                        crate::console::EntryKind::ServerCommand => ui::ConsoleSuggestKind::Server,
                    },
                    mask: hit.mask,
                    modified: is_cvar && !value.is_empty() && !same(&value, entry.default),
                    value,
                    default_value: entry.default,
                    range: entry.range,
                    description: entry.description,
                }
            })
            .collect();
        (rows, total, hint)
    }

    /// The typed text changed: the popup starts over at the best match.
    pub(in crate::app) fn console_suggest_reset(&mut self) {
        self.console_suggest_index = 0;
        self.console_suggest_picked = false;
        self.console_suggest_hidden = false;
    }

    /// Put `name` in place of the first token and leave the caret one space
    /// past it, ready for arguments.
    pub(in crate::app) fn apply_console_suggestion(&mut self, name: &str) {
        let Some((start, end)) = Self::console_first_token_span(&self.console_input) else {
            return;
        };
        let had_space = self.console_input[end..].starts_with(char::is_whitespace);
        self.console_input.replace_range(start..end, name);
        let mut cursor = start + name.len();
        if !had_space {
            self.console_input.insert(cursor, ' ');
        }
        cursor += 1;
        self.console_cursor = cursor.min(self.console_input.len());
        self.console_history_index = None;
        self.console_scroll = 0;
        self.console_suggest_reset();
    }

    /// Tab with the popup open: extend to the shared prefix when the best
    /// matches all start with what was typed, otherwise take the highlighted row.
    pub(in crate::app) fn tab_console_suggestion(&mut self) {
        let Some((hits, _, false)) = self.console_suggest_state() else {
            return;
        };
        if !self.console_suggest_picked && hits[0].tier == crate::console::MatchTier::Prefix {
            if let Some((start, end)) = Self::console_first_token_span(&self.console_input) {
                let token = self.console_input[start..end].to_owned();
                // Match TaystJK/legacy console ergonomics: if this prefix can
                // only resolve to one registered command/cvar, Tab completes
                // the whole token *and* leaves a trailing space ready for its
                // first argument. Previously the live-suggestion path stopped
                // after extending the common prefix ("conn" -> "connect") and
                // required a second keypress before arguments could be typed.
                if let Some(entry) = crate::console::unique_prefix(&token, self.console_scope()) {
                    self.apply_console_suggestion(entry.name);
                    return;
                }
                if let Some(common) = crate::console::common_prefix(&token, self.console_scope()) {
                    if common.len() > token.len() {
                        self.console_input.replace_range(start..end, &common);
                        self.console_cursor = start + common.len();
                        self.console_suggest_reset();
                        return;
                    }
                }
            }
        }
        let index = if self.console_suggest_picked {
            self.console_suggest_index.min(hits.len() - 1)
        } else {
            0
        };
        self.apply_console_suggestion(hits[index].entry.name);
    }

    /// Enter on an arrowed-to row that is not what was typed fills it in
    /// instead of running the half-typed text.
    pub(in crate::app) fn accept_picked_console_suggestion(&mut self) -> bool {
        if !self.console_suggest_picked {
            return false;
        }
        let Some((hits, _, false)) = self.console_suggest_state() else {
            return false;
        };
        let Some((start, end)) = Self::console_first_token_span(&self.console_input) else {
            return false;
        };
        let name = hits[self.console_suggest_index.min(hits.len() - 1)]
            .entry
            .name;
        if self.console_input[start..end].eq_ignore_ascii_case(name) {
            return false;
        }
        self.apply_console_suggestion(name);
        true
    }

    pub(in crate::app) fn step_console_suggestion(&mut self, delta: i32) {
        let Some((hits, _, false)) = self.console_suggest_state() else {
            return;
        };
        let current = if self.console_suggest_picked {
            self.console_suggest_index
        } else {
            0
        };
        self.console_suggest_index =
            (current as i32 + delta).rem_euclid(hits.len() as i32) as usize;
        self.console_suggest_picked = true;
    }

    /// Left click on the popup or the header switch. Returns whether it was consumed.
    pub(in crate::app) fn click_console_suggest(
        &mut self,
        width: u32,
        height: u32,
        x: f64,
        y: f64,
    ) -> bool {
        if ui::console_suggest_chip_hit(width, x, y) {
            let next = if self.console_suggest { "0" } else { "1" };
            let _ = self.set_console_cvar("con_suggest", next);
            return true;
        }
        let Some((hits, _, hint)) = self.console_suggest_state() else {
            return false;
        };
        if hint {
            return false;
        }
        let selected = if self.console_suggest_picked {
            self.console_suggest_index.min(hits.len() - 1)
        } else {
            0
        };
        let detail_lines =
            ui::console_suggest_detail_line_count(width, height, hits[selected].entry.description);
        let Some(index) = ui::console_suggest_hit(
            width,
            height,
            self.console_size,
            hits.len(),
            selected,
            detail_lines,
            x,
            y,
        ) else {
            return false;
        };
        self.apply_console_suggestion(hits[index].entry.name);
        true
    }

    pub(in crate::app) fn complete_unique_console_command(&mut self) {
        let Some((start, end)) = Self::console_first_token_span(&self.console_input) else {
            return;
        };
        let prefix = self.console_input[start..end].to_owned();
        let connected = self.console_scope();
        if crate::console::find_command(&prefix, connected).is_some() {
            return;
        }
        if let Some(entry) = crate::console::unique_prefix(&prefix, connected) {
            self.replace_console_first_token(entry.name, false);
        }
    }

    pub(in crate::app) fn browse_console_history(&mut self, direction: i32) {
        if self.console_history.is_empty() {
            return;
        }
        let len = self.console_history.len();
        let next = match (self.console_history_index, direction.signum()) {
            (None, -1) => Some(len - 1),
            (Some(index), -1) => Some(index.saturating_sub(1)),
            (Some(index), 1) if index + 1 < len => Some(index + 1),
            (Some(_), 1) => None,
            (index, _) => index,
        };
        self.console_history_index = next;
        self.console_input = next
            .map(|index| self.console_history[index].clone())
            .unwrap_or_default();
        self.console_cursor = self.console_input.len();
        self.console_scroll = 0;
        // A recalled command is not a search: keep Up/Down on history.
        self.console_suggest_reset();
        self.console_suggest_hidden = true;
        self.publish_ui();
    }
}
