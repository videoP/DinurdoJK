//! Console state.
use crate::app::{ui, App, ConsoleSearchMatch, Duration, Instant, OverlayMode, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) struct ConsolePoint {
    pub(in crate::app) line: usize,
    pub(in crate::app) col: usize,
}

#[derive(Debug, Clone)]
pub(in crate::app) struct ConsolePathLink {
    pub(in crate::app) start_col: usize,
    pub(in crate::app) end_col: usize,
    pub(in crate::app) target: PathBuf,
}

impl App {
    pub(in crate::app) fn clear_console_output(&mut self) {
        self.console_lines.clear();
        self.console_path_links.clear();
        self.console_scroll = 0;
        self.console_selection_anchor = None;
        self.console_selection_focus = None;
        self.console_selecting = false;
        self.console_last_click = None;
        self.console_click_count = 0;
        self.console_search_matches.clear();
        self.console_search_index = None;
        self.console_status.clear();
    }

    pub(in crate::app) fn append_console_line(
        &mut self,
        line: String,
        path_links: Vec<ConsolePathLink>,
    ) {
        const MAX_CONSOLE_LINES: usize = 10_000;
        let preserve_scroll = self.console_scroll > 0;
        if self.console_lines.len() >= MAX_CONSOLE_LINES {
            self.console_lines.pop_front();
            self.console_path_links.pop_front();

            let shift_point = |point: Option<ConsolePoint>| {
                point.map(|point| ConsolePoint {
                    line: point.line.saturating_sub(1),
                    col: if point.line == 0 { 0 } else { point.col },
                })
            };
            self.console_selection_anchor = shift_point(self.console_selection_anchor);
            self.console_selection_focus = shift_point(self.console_selection_focus);
            self.console_last_click = self.console_last_click.and_then(|(when, point)| {
                (point.line > 0).then_some((
                    when,
                    ConsolePoint {
                        line: point.line - 1,
                        col: point.col,
                    },
                ))
            });
        }
        self.console_lines.push_back(line);
        self.console_path_links.push_back(path_links);
        self.mark_companion_console_dirty();

        // If the user has scrolled up to inspect/select older output, keep the
        // viewport anchored on that same content as new lines arrive. At the
        // live bottom, remain pinned to the newest line as before.
        if preserve_scroll {
            self.console_scroll = (self.console_scroll + 1).min(self.console_max_scroll());
        } else {
            self.console_scroll = 0;
        }
    }

    pub(in crate::app) fn sync_console_log(&mut self) -> bool {
        let mut changed = false;
        while let Ok(record) = self.log_rx.try_recv() {
            // Server/engine-authored text (e.g. a multi-line disconnect reason)
            // can carry embedded '\n's. The console draws one row per entry in
            // `console_lines`, so a single entry spanning several visual rows
            // would overlap whatever gets appended after it. Split here so
            // every physical row is its own entry.
            let prefix = if self.console_timestamps {
                format!(
                    "^8[{:02}:{:02}:{:02}]^7 ",
                    record.local_time[0], record.local_time[1], record.local_time[2]
                )
            } else {
                String::new()
            };
            for (index, segment) in record.text.split('\n').enumerate() {
                let segment = segment.strip_suffix('\r').unwrap_or(segment);
                let line = if index == 0 {
                    format!("{prefix}{segment}")
                } else {
                    segment.to_owned()
                };
                let plain = crate::logging::strip_jka_colors(&line);
                let path_links = record
                    .path_links
                    .iter()
                    .filter(|link| {
                        link.target.is_absolute()
                            && !link.label.is_empty()
                            && !link
                                .label
                                .chars()
                                .any(|ch| matches!(ch, '\0' | '\r' | '\n'))
                    })
                    .filter_map(|link| {
                        let byte_start = plain.find(&link.label)?;
                        let start_col = plain[..byte_start].chars().count();
                        let end_col = start_col + link.label.chars().count();
                        Some(ConsolePathLink {
                            start_col,
                            end_col,
                            target: link.target.clone(),
                        })
                    })
                    .collect();
                self.append_console_line(line, path_links);
            }
            changed = true;
        }
        if changed && self.console_search_open {
            self.rebuild_console_search(false);
        }
        changed
    }

    pub(in crate::app) fn push_console_line(&mut self, line: impl Into<String>) {
        crate::logging::write_line(crate::logging::Level::Info, format_args!("{}", line.into()));
        // `push_console_line` drains the log channel immediately. That means the
        // next tick's `sync_console_log()` cannot notice this line and publish a
        // fresh retained console snapshot for us. While the console is open,
        // publish here so server prints/centerprints (jaPRO checkpoints included)
        // become visible immediately. `append_console_line` still owns scroll
        // anchoring: live-bottom stays pinned, while an intentional PageUp/scroll
        // remains anchored on the older text.
        if self.sync_console_log() && self.overlay == OverlayMode::Console {
            self.publish_ui();
        }
    }

    /// Emit an engine-authored console line with one explicitly trusted local
    /// path target. No path-looking text is inferred or auto-linked.
    pub(in crate::app) fn push_console_path_line(
        &mut self,
        line: impl Into<String>,
        path: impl Into<PathBuf>,
    ) {
        let line = line.into();
        crate::logging::write_line_with_path(
            crate::logging::Level::Info,
            format_args!("{line}"),
            path.into(),
        );
        if self.sync_console_log() && self.overlay == OverlayMode::Console {
            self.publish_ui();
        }
    }

    pub(in crate::app) fn console_path_at(&self, point: ConsolePoint) -> Option<PathBuf> {
        self.console_path_links
            .get(point.line)?
            .iter()
            .find(|link| point.col >= link.start_col && point.col < link.end_col)
            .map(|link| link.target.clone())
    }

    pub(in crate::app) fn validated_console_reveal_target(
        &self,
        target: &Path,
    ) -> Result<(PathBuf, bool), String> {
        if !target.is_absolute() {
            return Err("console path link is not absolute".to_owned());
        }
        let raw = target.as_os_str().to_string_lossy();
        if raw.is_empty() || raw.chars().any(|ch| matches!(ch, '\0' | '\r' | '\n')) {
            return Err("console path link contains invalid characters".to_owned());
        }

        // Resolve the nearest existing ancestor. Failed writes can legitimately
        // mention a file that does not exist yet; revealing its existing parent
        // is still useful, but arbitrary non-local destinations are rejected.
        let mut existing = target;
        while !existing.exists() {
            existing = existing
                .parent()
                .ok_or_else(|| "console path link has no existing parent".to_owned())?;
        }
        let canonical_existing = std::fs::canonicalize(existing)
            .map_err(|error| format!("could not validate console path: {error}"))?;

        let mut allowed_roots = Vec::with_capacity(4);
        if let Some(root) = self.base.parent().or(Some(self.base.as_path())) {
            if let Ok(root) = std::fs::canonicalize(root) {
                allowed_roots.push(root);
            }
        }
        let active_game = jka_assets::pk3::active_game_directory(&self.base, self.game.as_deref());
        if let Ok(root) = std::fs::canonicalize(active_game) {
            allowed_roots.push(root);
        }
        if let Some(parent) = self.config_path.parent() {
            if let Ok(root) = std::fs::canonicalize(parent) {
                allowed_roots.push(root);
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                if let Ok(root) = std::fs::canonicalize(parent) {
                    allowed_roots.push(root);
                }
            }
        }
        if !allowed_roots
            .iter()
            .any(|root| canonical_existing.starts_with(root))
        {
            return Err("console path link is outside DinurdoJK/JKA local roots".to_owned());
        }

        let target_exists = target.exists();
        let canonical_target = if target_exists {
            std::fs::canonicalize(target)
                .map_err(|error| format!("could not resolve console path: {error}"))?
        } else {
            canonical_existing
        };
        let is_file = target_exists && canonical_target.is_file();
        Ok((canonical_target, is_file))
    }

    pub(in crate::app) fn reveal_console_path(&mut self, target: &Path) -> bool {
        let (validated, is_file) = match self.validated_console_reveal_target(target) {
            Ok(validated) => validated,
            Err(error) => {
                self.console_status = format!("PATH LINK BLOCKED: {error}");
                return false;
            }
        };

        #[cfg(windows)]
        let result = {
            let mut command = std::process::Command::new("explorer.exe");
            if is_file {
                command.arg("/select,").arg(&validated);
            } else {
                command.arg(&validated);
            }
            command.spawn().map(|_| ())
        };

        #[cfg(target_os = "macos")]
        let result = {
            let mut command = std::process::Command::new("open");
            if is_file {
                command.arg("-R");
            }
            command.arg(&validated).spawn().map(|_| ())
        };

        #[cfg(all(not(windows), not(target_os = "macos")))]
        let result = {
            let directory = if is_file {
                validated.parent().unwrap_or(validated.as_path())
            } else {
                validated.as_path()
            };
            std::process::Command::new("xdg-open")
                .arg(directory)
                .spawn()
                .map(|_| ())
        };

        match result {
            Ok(()) => {
                self.console_status = format!("OPENED FOLDER: {}", validated.display());
                true
            }
            Err(error) => {
                self.console_status = format!("COULDN'T OPEN FOLDER: {error}");
                false
            }
        }
    }

    pub(in crate::app) fn console_point_at(
        &self,
        width: u32,
        height: u32,
        x: f64,
        y: f64,
    ) -> Option<ConsolePoint> {
        let (row, col) = ui::console_text_hit(width, height, self.console_size, x, y)?;
        let capacity = ui::console_visible_line_capacity(height, self.console_size);
        let scroll = self.console_scroll.min(self.console_max_scroll());
        let end = self.console_lines.len().saturating_sub(scroll);
        let start = end.saturating_sub(capacity);
        let line = start + row;
        if line >= end {
            return None;
        }
        let visible_len = crate::logging::strip_jka_colors(&self.console_lines[line])
            .chars()
            .count();
        Some(ConsolePoint {
            line,
            col: col.min(visible_len),
        })
    }

    pub(in crate::app) fn console_max_scroll(&self) -> usize {
        let visible_lines = self
            .window
            .as_ref()
            .map(|window| {
                ui::console_visible_line_capacity(window.inner_size().height, self.console_size)
            })
            .unwrap_or(1)
            .max(1);
        self.console_lines.len().saturating_sub(visible_lines)
    }

    pub(in crate::app) fn scroll_console(&mut self, rows: i32) {
        if rows > 0 {
            self.console_scroll =
                (self.console_scroll + rows as usize).min(self.console_max_scroll());
        } else if rows < 0 {
            self.console_scroll = self.console_scroll.saturating_sub((-rows) as usize);
        }
    }

    pub(in crate::app) fn console_word_selection(
        &self,
        point: ConsolePoint,
    ) -> Option<(ConsolePoint, ConsolePoint)> {
        let raw = self.console_lines.get(point.line)?;
        let plain = crate::logging::strip_jka_colors(raw);
        let chars: Vec<char> = plain.chars().collect();
        if chars.is_empty() {
            return None;
        }

        let index = point.col.min(chars.len().saturating_sub(1));
        if chars[index].is_whitespace() {
            return None;
        }

        fn word_char(ch: char) -> bool {
            ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | '\\' | ':' | '@')
        }

        let class_is_word = word_char(chars[index]);
        let same_class = |ch: char| !ch.is_whitespace() && word_char(ch) == class_is_word;
        let mut start = index;
        while start > 0 && same_class(chars[start - 1]) {
            start -= 1;
        }
        let mut end = index + 1;
        while end < chars.len() && same_class(chars[end]) {
            end += 1;
        }
        Some((
            ConsolePoint {
                line: point.line,
                col: start,
            },
            ConsolePoint {
                line: point.line,
                col: end,
            },
        ))
    }

    pub(in crate::app) fn console_line_selection(
        &self,
        point: ConsolePoint,
    ) -> Option<(ConsolePoint, ConsolePoint)> {
        let raw = self.console_lines.get(point.line)?;
        let len = crate::logging::strip_jka_colors(raw).chars().count();
        (len > 0).then_some((
            ConsolePoint {
                line: point.line,
                col: 0,
            },
            ConsolePoint {
                line: point.line,
                col: len,
            },
        ))
    }

    pub(in crate::app) fn begin_console_selection(&mut self, point: ConsolePoint) {
        const MULTI_CLICK_WINDOW: Duration = Duration::from_millis(450);
        let now = Instant::now();
        let continues = self
            .console_last_click
            .is_some_and(|(last_time, last_point)| {
                now.saturating_duration_since(last_time) <= MULTI_CLICK_WINDOW
                    && last_point.line == point.line
                    && last_point.col.abs_diff(point.col) <= 1
            });
        self.console_click_count = if continues && self.console_click_count < 3 {
            self.console_click_count + 1
        } else {
            1
        };
        self.console_last_click = Some((now, point));

        match self.console_click_count {
            2 => {
                if let Some((start, end)) = self.console_word_selection(point) {
                    self.console_selection_anchor = Some(start);
                    self.console_selection_focus = Some(end);
                    self.console_selecting = false;
                } else {
                    self.console_selection_anchor = Some(point);
                    self.console_selection_focus = Some(point);
                    self.console_selecting = true;
                }
            }
            3 => {
                if let Some((start, end)) = self.console_line_selection(point) {
                    self.console_selection_anchor = Some(start);
                    self.console_selection_focus = Some(end);
                } else {
                    self.console_selection_anchor = Some(point);
                    self.console_selection_focus = Some(point);
                }
                self.console_selecting = false;
            }
            _ => {
                self.console_selection_anchor = Some(point);
                self.console_selection_focus = Some(point);
                self.console_selecting = true;
            }
        }
    }

    pub(in crate::app) fn select_all_console(&mut self) {
        let Some(last_line) = self.console_lines.len().checked_sub(1) else {
            self.console_selection_anchor = None;
            self.console_selection_focus = None;
            self.console_selecting = false;
            return;
        };
        let last_col = self
            .console_lines
            .get(last_line)
            .map(|raw| crate::logging::strip_jka_colors(raw).chars().count())
            .unwrap_or(0);
        self.console_selection_anchor = Some(ConsolePoint { line: 0, col: 0 });
        self.console_selection_focus = Some(ConsolePoint {
            line: last_line,
            col: last_col,
        });
        self.console_selecting = false;
    }

    pub(in crate::app) fn selected_console_text(&self) -> Option<String> {
        let (mut start, mut end) = (
            self.console_selection_anchor?,
            self.console_selection_focus?,
        );
        if (start.line, start.col) > (end.line, end.col) {
            std::mem::swap(&mut start, &mut end);
        }
        if start == end {
            return None;
        }
        let mut out = String::new();
        for line_index in start.line..=end.line {
            let Some(raw) = self.console_lines.get(line_index) else {
                break;
            };
            let plain = crate::logging::strip_jka_colors(raw);
            let chars: Vec<char> = plain.chars().collect();
            let left = if line_index == start.line {
                start.col.min(chars.len())
            } else {
                0
            };
            let right = if line_index == end.line {
                end.col.min(chars.len())
            } else {
                chars.len()
            };
            if right > left {
                out.extend(chars[left..right].iter());
            }
            if line_index != end.line {
                out.push('\n');
            }
        }
        (!out.is_empty()).then_some(out)
    }

    pub(in crate::app) fn console_visible_range(&self) -> (usize, usize) {
        let capacity = self
            .window
            .as_ref()
            .map(|window| {
                ui::console_visible_line_capacity(window.inner_size().height, self.console_size)
            })
            .unwrap_or(1)
            .max(1);
        let scroll = self.console_scroll.min(self.console_max_scroll());
        let end = self.console_lines.len().saturating_sub(scroll);
        (end.saturating_sub(capacity), end)
    }

    pub(in crate::app) fn reveal_console_search_match(&mut self) {
        let Some(hit) = self
            .console_search_index
            .and_then(|index| self.console_search_matches.get(index).copied())
        else {
            return;
        };
        let (visible_start, visible_end) = self.console_visible_range();
        if hit.line >= visible_start && hit.line < visible_end {
            return;
        }

        let capacity = self
            .window
            .as_ref()
            .map(|window| {
                ui::console_visible_line_capacity(window.inner_size().height, self.console_size)
            })
            .unwrap_or(1)
            .max(1);
        let centered_start = hit.line.saturating_sub(capacity / 2);
        let centered_end = (centered_start + capacity).min(self.console_lines.len());
        self.console_scroll = self
            .console_lines
            .len()
            .saturating_sub(centered_end)
            .min(self.console_max_scroll());
    }

    pub(in crate::app) fn rebuild_console_search(&mut self, reveal: bool) {
        let previous = self
            .console_search_index
            .and_then(|index| self.console_search_matches.get(index).copied());
        let visible_start = self.console_visible_range().0;
        let needle = self.console_search_input.to_ascii_lowercase();
        let mut matches = Vec::new();

        if !needle.is_empty() {
            for (line, raw) in self.console_lines.iter().enumerate() {
                let plain = crate::logging::strip_jka_colors(raw);
                let haystack = plain.to_ascii_lowercase();
                for (byte_start, _) in haystack.match_indices(&needle) {
                    let byte_end = byte_start + needle.len();
                    let start_col = plain[..byte_start].chars().count();
                    let end_col = start_col + plain[byte_start..byte_end].chars().count();
                    matches.push(ConsoleSearchMatch {
                        line,
                        start_col,
                        end_col,
                    });
                }
            }
        }

        self.console_search_matches = matches;
        self.console_search_index = if self.console_search_matches.is_empty() {
            None
        } else if let Some(previous) = previous {
            self.console_search_matches
                .iter()
                .position(|hit| hit.line == previous.line && hit.start_col == previous.start_col)
                .or_else(|| {
                    self.console_search_matches
                        .iter()
                        .position(|hit| hit.line >= visible_start)
                })
                .or(Some(0))
        } else {
            self.console_search_matches
                .iter()
                .position(|hit| hit.line >= visible_start)
                .or(Some(0))
        };

        if reveal {
            self.reveal_console_search_match();
        }
    }

    pub(in crate::app) fn open_console_search(&mut self) {
        if !self.console_search_open {
            if let Some(selected) = self.selected_console_text() {
                if !selected.contains('\n') && selected.chars().count() <= 128 {
                    self.console_search_input = selected;
                    self.console_search_matches.clear();
                    self.console_search_index = None;
                }
            }
            self.console_search_open = true;
            self.console_selection_anchor = None;
            self.console_selection_focus = None;
            self.console_selecting = false;
            self.rebuild_console_search(true);
        }
        self.publish_ui();
    }

    pub(in crate::app) fn step_console_search(&mut self, backwards: bool) {
        if self.console_search_matches.is_empty() {
            return;
        }
        let len = self.console_search_matches.len();
        self.console_search_index = Some(match self.console_search_index {
            Some(index) if backwards => (index + len - 1) % len,
            Some(index) => (index + 1) % len,
            None if backwards => len - 1,
            None => 0,
        });
        self.reveal_console_search_match();
    }
}
