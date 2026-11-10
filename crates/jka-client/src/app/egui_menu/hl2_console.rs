//! Source/HL2-inspired floating developer console.
//!
//! The underlying command editor/history, execution, log records and search
//! stay in App. This is exclusively an opt-in presentation; the original
//! WGPU console and its mouse hit-testing are bypassed only while con_hl2=1.
//! The HL2 renderer must NOT create an independent egui text selection; it
//! uses the same ConsolePoint and clipboard handler as the classic console.
use crate::app::console_state::ConsolePoint;
use crate::app::egui_menu::App;
use crate::logging::ConsoleLineKind;

const PANEL: egui::Color32 = egui::Color32::from_rgba_premultiplied(39, 43, 45, 246);
const LOG_BG: egui::Color32 = egui::Color32::from_rgb(29, 32, 33);
const EDGE: egui::Color32 = egui::Color32::from_rgb(105, 110, 111);
const LABEL: egui::Color32 = egui::Color32::from_rgb(222, 224, 221);
const DIM: egui::Color32 = egui::Color32::from_rgb(160, 168, 167);
const INPUT: egui::Color32 = egui::Color32::from_rgb(204, 209, 206);

// Autocomplete is a dropdown BELOW the prompt/window. Reserve its full height
// *even when no suggestions are visible*, so Tab/typing cannot move the console.
const COMPLETION_ROW_HEIGHT: f32 = 20.0;
const COMPLETION_ROW_SPACING: f32 = 3.0;
const COMPLETION_MAX_ROWS: usize = 5;
const COMPLETION_HINT_HEIGHT: f32 = 20.0;
const COMPLETION_FRAME_PADDING: f32 = 12.0;
const COMPLETION_DROPDOWN_GAP: f32 = 7.0;
const COMPLETION_VIEWPORT_SLACK: f32 = 6.0;
const COMPLETION_MAX_RESERVE: f32 = COMPLETION_DROPDOWN_GAP
    + COMPLETION_FRAME_PADDING
    + COMPLETION_MAX_ROWS as f32 * (COMPLETION_ROW_HEIGHT + COMPLETION_ROW_SPACING)
    + COMPLETION_HINT_HEIGHT + COMPLETION_VIEWPORT_SLACK;

fn completion_height(rows: usize, show_hint: bool) -> f32 {
    COMPLETION_FRAME_PADDING + rows as f32 * (COMPLETION_ROW_HEIGHT + COMPLETION_ROW_SPACING)
        + if show_hint { COMPLETION_HINT_HEIGHT } else { 0.0 }
}

fn completion_rows_that_fit(free_height: f32, show_hint: bool) -> usize {
    let overhead = completion_height(0, show_hint);
    ((free_height - overhead).max(0.0) / (COMPLETION_ROW_HEIGHT + COMPLETION_ROW_SPACING))
        .floor().min(COMPLETION_MAX_ROWS as f32) as usize
}

/// Reserve a persistent strip BELOW the floating window. This is used for
/// both header dragging and all edge/corner resize limits, regardless of whether
/// autocomplete is currently shown. On tiny viewports use whatever can remain
/// after the minimum console height; suggestions then show fewer rows or hide.
fn console_work_area(viewport: egui::Rect) -> egui::Rect {
    let min_console_height = 220.0_f32.min(viewport.height());
    let reserve = COMPLETION_MAX_RESERVE.min((viewport.height() - min_console_height).max(0.0));
    egui::Rect::from_min_max(
        viewport.min, egui::pos2(viewport.right(), viewport.bottom() - reserve),
    )
}

/// One-way dropdown placement: always below the actual window bottom, NEVER
/// flipped above the prompt, and never outside the current viewport. The
/// reserved work-area margin normally fits all five rows (with ARGUMENT hint).
fn completion_popup_geometry(
    window: egui::Rect, prompt: egui::Rect, viewport: egui::Rect,
    count: usize, hint: bool,
) -> Option<(egui::Rect, usize)> {
    if count == 0 || prompt.width() <= 0.0 || viewport.width() <= 0.0 { return None; }
    let top = window.bottom() + COMPLETION_DROPDOWN_GAP;
    let available_height = (viewport.bottom() - COMPLETION_VIEWPORT_SLACK - top).max(0.0);
    let rows = count.min(completion_rows_that_fit(available_height, hint));
    if rows == 0 { return None; }
    let width = prompt.width().min(460.0).min(viewport.width());
    let x = prompt.left().clamp(viewport.left(), viewport.right() - width);
    let popup = egui::Rect::from_min_size(
        egui::pos2(x, top), egui::vec2(width, completion_height(rows, hint)),
    );
    debug_assert!(popup.top() > window.bottom() && popup.bottom() <= viewport.bottom());
    Some((popup, rows))
}


fn kind_color(kind: ConsoleLineKind) -> egui::Color32 {
    match kind {
        ConsoleLineKind::Error => egui::Color32::from_rgb(248, 105, 95),
        ConsoleLineKind::CommandEcho | ConsoleLineKind::TeamChatCommandEcho =>
            egui::Color32::from_rgb(219, 189, 139),
        ConsoleLineKind::NotifyMention | ConsoleLineKind::TeamChatMention =>
            egui::Color32::from_rgb(135, 211, 224),
        ConsoleLineKind::NotifyVote => egui::Color32::from_rgb(243, 186, 112),
        ConsoleLineKind::NotifyConnect => egui::Color32::from_rgb(171, 205, 139),
        ConsoleLineKind::Normal | ConsoleLineKind::TeamChat => LABEL,
    }
}

fn jka_color(code: u8) -> egui::Color32 {
    match code {
        b'0' => egui::Color32::from_rgb(23, 25, 27),
        b'1' => egui::Color32::from_rgb(255, 95, 95),
        b'2' => egui::Color32::from_rgb(105, 225, 105),
        b'3' => egui::Color32::from_rgb(249, 227, 105),
        b'4' => egui::Color32::from_rgb(126, 149, 255),
        b'5' => egui::Color32::from_rgb(109, 220, 230),
        b'6' => egui::Color32::from_rgb(241, 134, 237),
        _ => LABEL,
    }
}

/// Build a monospace line without showing Quake-style ^N markup. A timestamp
/// prefix is gray regardless of the record's severity, matching Source.
fn console_line_job(line: &str, kind: ConsoleLineKind) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let default_color = kind_color(kind);
    let mut active = default_color;
    let font = egui::FontId::monospace(12.0);
    let mut remaining = line;
    if remaining.len() >= 10 && remaining.starts_with('[')
        && remaining.as_bytes().get(9) == Some(&b']')
        && remaining.as_bytes()[1..9].iter().all(|c| c.is_ascii_digit() || *c == b':')
    {
        job.append(&remaining[..10], 0.0, egui::TextFormat {
            font_id: font.clone(), color: DIM, ..Default::default()
        });
        remaining = &remaining[10..];
    }
    while !remaining.is_empty() {
        let next = remaining.find('^');
        let n = next.unwrap_or(remaining.len());
        if n > 0 {
            job.append(&remaining[..n], 0.0, egui::TextFormat {
                font_id: font.clone(), color: active, ..Default::default()
            });
            remaining = &remaining[n..];
        }
        if remaining.starts_with('^') {
            let bytes = remaining.as_bytes();
            if bytes.len() > 1 && bytes[1].is_ascii_digit() {
                active = jka_color(bytes[1]);
                remaining = &remaining[2..];
                continue;
            }
            job.append("^", 0.0, egui::TextFormat {
                font_id: font.clone(), color: active, ..Default::default()
            });
            remaining = &remaining[1..];
        }
    }
    // Log lines are pre-wrapped by the text engine for the chosen width.
    // Keep one virtualized row per logical record, with horizontally clipped
    // overflow like VGUI's log list to avoid allocating thousands of widgets.
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job
}

/// Selection coordinates are character offsets into logical records, not
/// virtualized widget positions. One normalized range works for both views.
fn selected_columns(
    bounds: Option<(ConsolePoint, ConsolePoint)>, row: usize, length: usize,
) -> Option<(usize, usize)> {
    let (start, end) = bounds?;
    if row < start.line || row > end.line { return None; }
    let left = if row == start.line { start.col.min(length) } else { 0 };
    let right = if row == end.line { end.col.min(length) } else { length };
    (left < right).then_some((left, right))
}

/// Map a displayed galley hit to shared (line, character) coordinates. Using
/// the actual laid-out glyph positions avoids inaccurate x / fixed-width
/// guesses on DPI scaling and Unicode text.
fn galley_point(
    line: usize, galley: &egui::Galley, origin: egui::Pos2, pointer: egui::Pos2,
) -> ConsolePoint {
    ConsolePoint {
        line,
        col: galley.cursor_from_pos(pointer - origin).index.0,
    }
}

/// Row data is kept only for visible (virtualized) rows, never the whole log.
type VisibleLogRow = (usize, egui::Rect, std::sync::Arc<egui::Galley>, egui::Pos2);

/// Keep the complete floating console on the current egui viewport, including
/// when a stored rectangle is restored after changing resolution or DPI scale.
fn clamp_console_rect(saved: [f32; 4], viewport: egui::Rect) -> [f32; 4] {
    let available_w = viewport.width().max(1.0);
    let available_h = viewport.height().max(1.0);
    let width = saved[2].clamp(360.0_f32.min(available_w), available_w);
    let height = saved[3].clamp(220.0_f32.min(available_h), available_h);
    let x = saved[0].clamp(viewport.left(), viewport.right() - width);
    let y = saved[1].clamp(viewport.top(), viewport.bottom() - height);
    [x, y, width, height]
}

/// Resize only from an explicit pointer drag on a console border. In particular,
/// text width, log content, autocompletion, and repaint frequency must NEVER
/// affect the persisted rectangle.
fn resize_console_rect(
    saved: [f32; 4], delta: egui::Vec2, horizontal: i8, vertical: i8,
    viewport: egui::Rect,
) -> [f32; 4] {
    let mut rect = egui::Rect::from_min_size(
        egui::pos2(saved[0], saved[1]), egui::vec2(saved[2], saved[3]),
    );
    let min_w = 360.0_f32.min(viewport.width());
    let min_h = 220.0_f32.min(viewport.height());
    if horizontal < 0 {
        rect.min.x = (rect.min.x + delta.x).clamp(viewport.left(), rect.max.x - min_w);
    } else if horizontal > 0 {
        rect.max.x = (rect.max.x + delta.x).clamp(rect.min.x + min_w, viewport.right());
    }
    if vertical < 0 {
        rect.min.y = (rect.min.y + delta.y).clamp(viewport.top(), rect.max.y - min_h);
    } else if vertical > 0 {
        rect.max.y = (rect.max.y + delta.y).clamp(rect.min.y + min_h, viewport.bottom());
    }
    [rect.left(), rect.top(), rect.width(), rect.height()]
}

// The visible border is OUTSIDE the window's inner-margin content Ui.
// Keep hit targets centered on that border, not on ui.max_rect(): the old
// version forced the mouse about 8 points inside before resizing worked.
const RESIZE_BORDER_OUTSIDE: f32 = 5.0;
const RESIZE_BORDER_INSIDE: f32 = 5.0;
const RESIZE_CORNER: f32 = 18.0;

type ResizeGrip = (&'static str, i8, i8, egui::Rect);

fn console_resize_hit_rects(rect: egui::Rect) -> [ResizeGrip; 8] {
    let outside = RESIZE_BORDER_OUTSIDE;
    let inside = RESIZE_BORDER_INSIDE;
    let corner = RESIZE_CORNER;
    let left = rect.left();
    let right = rect.right();
    let top = rect.top();
    let bottom = rect.bottom();
    [
        // Corners have their own larger targets, including the actual corners.
        ("nw", -1, -1, egui::Rect::from_min_max(
            egui::pos2(left - outside, top - outside),
            egui::pos2(left + corner, top + corner),
        )),
        ("ne", 1, -1, egui::Rect::from_min_max(
            egui::pos2(right - corner, top - outside),
            egui::pos2(right + outside, top + corner),
        )),
        ("sw", -1, 1, egui::Rect::from_min_max(
            egui::pos2(left - outside, bottom - corner),
            egui::pos2(left + corner, bottom + outside),
        )),
        ("se", 1, 1, egui::Rect::from_min_max(
            egui::pos2(right - corner, bottom - corner),
            egui::pos2(right + outside, bottom + outside),
        )),
        // Edge strips straddle the FRAME stroke, not its inset content area.
        // Exclude corner zones so diagonal grabs always win in corners.
        ("w", -1, 0, egui::Rect::from_min_max(
            egui::pos2(left - outside, top + corner),
            egui::pos2(left + inside, bottom - corner),
        )),
        ("e", 1, 0, egui::Rect::from_min_max(
            egui::pos2(right - inside, top + corner),
            egui::pos2(right + outside, bottom - corner),
        )),
        ("n", 0, -1, egui::Rect::from_min_max(
            egui::pos2(left + corner, top - outside),
            egui::pos2(right - corner, top + inside),
        )),
        ("s", 0, 1, egui::Rect::from_min_max(
            egui::pos2(left + corner, bottom - inside),
            egui::pos2(right - corner, bottom + outside),
        )),
    ]
}

/// Each border section has a transparent foreground Area the exact size of its
/// hitbox. Using the window's inner `Ui` clipped these targets to the padding;
/// using ONE large foreground Area would instead steal input from the log and
/// command prompt. Separate thin Areas avoid both problems.
fn console_resize_grip(ctx: &egui::Context, window_rect: egui::Rect) -> Option<(i8, i8, egui::Vec2)> {
    let mut active = None;
    for (name, horizontal, vertical, hit) in console_resize_hit_rects(window_rect) {
        let cursor = match (horizontal, vertical) {
            (-1, -1) | (1, 1) => egui::CursorIcon::ResizeNwSe,
            (1, -1) | (-1, 1) => egui::CursorIcon::ResizeNeSw,
            (0, _) => egui::CursorIcon::ResizeVertical,
            _ => egui::CursorIcon::ResizeHorizontal,
        };
        let response = egui::Area::new(egui::Id::new(("hl2_console_resize", name)))
            .order(egui::Order::Foreground)
            .fixed_pos(hit.min)
            .constrain(false)
            .sense(egui::Sense::hover())
            .show(ctx, |ui| {
                ui.allocate_exact_size(hit.size(), egui::Sense::drag()).1
            })
            .inner;
        if response.hovered() || response.dragged() {
            ctx.set_cursor_icon(cursor);
        }
        if response.dragged() {
            active = Some((horizontal, vertical, ctx.input(|i| i.pointer.delta())));
        }
    }
    active
}

impl App {
    pub(in crate::app) fn egui_hl2_console(&mut self, ctx: &egui::Context) {
        self.ensure_chat_translation_fonts(ctx);
        // Keep a small inset so even the resize edges remain reachable.
        let viewport = ctx.content_rect().shrink(5.0);
        let work_area = console_work_area(viewport);
        let [x, y, width, height] = clamp_console_rect(self.console_hl2_rect, work_area);
        let mut close_requested = false;
        let mut header_drag = egui::Vec2::ZERO;
        // Completion is painted over the log INSIDE the window; it never gets
        // its own top-level Area and cannot extend beyond the log/frame.
        // Autocomplete must never affect the window's content height or width.
        let (suggestions, _, hint) = self.console_suggest_snapshot();
        // Use a truly fixed rectangle. `default_rect` is only an initial size:
        // egui will otherwise enlarge it to fit greedy child layouts, and the
        // old feedback loop saved that incidental growth on every repaint.
        let window_rect = egui::Rect::from_min_size(
            egui::pos2(x, y), egui::vec2(width, height),
        );
        let mut prompt_rect = None;
        let scroll_to = self.console_hl2_scroll_to.take();
        let page_delta = std::mem::take(&mut self.console_hl2_page_delta);
        egui::Window::new("Console")
            .id(egui::Id::new("dinurdo_hl2_console"))
            .title_bar(false)
            .collapsible(false)
            .fixed_rect(window_rect)
            .constrain_to(work_area)
            .frame(egui::Frame::new().fill(PANEL)
                .stroke(egui::Stroke::new(1.0, EDGE))
                .inner_margin(egui::Margin::same(8)))
            .show(ctx, |ui| {
                ui.style_mut().spacing.item_spacing = egui::vec2(4.0, 3.0);
                // A single drag target covers the *entire unused header*, not
                // just the title text. Place the two real buttons first in an
                // RTL row, then allocate ALL remaining width as the drag area.
                // This keeps the header clickable without stealing Clear/X.
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("×").on_hover_text("Close console").clicked() {
                            close_requested = true;
                        }
                        if ui.small_button("Clear").clicked() {
                            self.clear_console_output();
                        }
                        let (header_rect, drag) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width().max(1.0), 20.0),
                            egui::Sense::drag(),
                        );
                        let drag = drag.on_hover_cursor(egui::CursorIcon::Grab);
                        if drag.dragged() {
                            header_drag += ui.input(|i| i.pointer.delta());
                        }
                        // Paint within the fixed drag target; neither a long
                        // status nor the label can alter window geometry.
                        let painter = ui.painter().with_clip_rect(header_rect);
                        let title = painter.layout_no_wrap(
                            "Developer Console".to_owned(),
                            egui::FontId::proportional(14.0), LABEL,
                        );
                        let title_pos = egui::pos2(
                            header_rect.left(),
                            header_rect.center().y - title.size().y * 0.5,
                        );
                        let status_x = title_pos.x + title.size().x + 10.0;
                        painter.galley(title_pos, title, LABEL);
                        if !self.console_status.is_empty() {
                            painter.text(
                                egui::pos2(status_x, header_rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                &self.console_status,
                                egui::FontId::proportional(12.0), DIM,
                            );
                        }
                    });
                });
                ui.separator();
                if self.console_search_open {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("FIND:").monospace().color(DIM));
                        // Keyboard focus intentionally stays with the shared
                        // console search editor, not a second egui TextEdit.
                        ui.label(egui::RichText::new(format!("{}|", self.console_search_input))
                            .monospace().color(LABEL));
                        ui.label(egui::RichText::new(format!("{} matches", self.console_search_matches.len()))
                            .color(DIM).small());
                        if ui.small_button("Prev").clicked() {
                            self.step_console_search(true);
                        }
                        if ui.small_button("Next").clicked() {
                            self.step_console_search(false);
                        }
                        if ui.small_button("×").on_hover_text("Close search").clicked() {
                            self.console_search_open = false;
                        }
                    });
                }
                // The log always fills the same space regardless of whether
                // autocomplete is visible, preventing Tab from resizing it.
                let log_height = (ui.available_height() - 45.0).max(65.0);
                let mut reveal_path = None;
                let mut chat_action = None;
                // Egui is only the painter/window layout here. The classic
                // ConsolePoint selection and Ctrl+C path own all text state.
                // Individual selectable labels cannot select across virtual
                // rows and steal Ctrl+C, so draw rows and hit-test ourselves.
                let mut visible_rows: Vec<VisibleLogRow> = Vec::new();
                let selection = self.console_selection_bounds();
                let log_result = egui::Frame::new()
                    .fill(LOG_BG)
                    .stroke(egui::Stroke::new(1.0, EDGE))
                    .inner_margin(egui::Margin::symmetric(6, 5))
                    .show(ui, |ui| {
                        // show_rows adds the *parent* item spacing to its
                        // virtual row pitch. Set it before constructing the
                        // scroll area so 16px is truly 16px everywhere.
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let mut scroll = egui::ScrollArea::vertical()
                            .id_salt("hl2_console_log")
                            .max_height(log_height)
                            .min_scrolled_height(log_height)
                            .auto_shrink([false, false])
                            .stick_to_bottom(true);
                        if let Some(line) = scroll_to {
                            // This offset is only set on a search navigation
                            // request; otherwise preserve egui's user scroll.
                            let visible = (log_height / 16.0) as usize;
                            scroll = scroll.vertical_scroll_offset(
                                line.saturating_sub(visible / 2) as f32 * 16.0,
                            );
                        }
                        scroll.show_rows(ui, 16.0, self.console_lines.len(), |ui, range| {
                            if page_delta != 0 {
                                ui.scroll_with_delta(egui::vec2(0.0, page_delta as f32 * 16.0));
                            }
                            let primary_down = ui.input(|i| i.pointer.primary_down());
                            let pointer = ui.input(|i| i.pointer.interact_pos());
                            if self.console_selecting && primary_down {
                                // Continue dragging across the top/bottom of
                                // the virtual list, including offscreen rows.
                                if let Some(pointer) = pointer {
                                    let clip = ui.clip_rect();
                                    if pointer.y <= clip.top() + 8.0 {
                                        ui.scroll_with_delta(egui::vec2(0.0, 16.0));
                                        ui.ctx().request_repaint();
                                    } else if pointer.y >= clip.bottom() - 8.0 {
                                        ui.scroll_with_delta(egui::vec2(0.0, -16.0));
                                        ui.ctx().request_repaint();
                                    }
                                }
                            }
                            for index in range {
                                let Some(line) = self.console_lines.get(index) else { continue; };
                                let kind = self.console_line_kinds.get(index).copied().unwrap_or_default();
                                let chat_id = self.console_chat_ids.get(index).copied().flatten()
                                    .filter(|&id| self.chat_translation.has_message(id));
                                let translated = chat_id.and_then(|id| self.console_chat_translation_line(line, id));
                                let display = translated.as_deref().unwrap_or(line);
                                let job = console_line_job(display, kind);
                                let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width().max(1.0), 16.0),
                                    egui::Sense::hover(),
                                );
                                // Explicit per-record id: otherwise the widget
                                // identity changes as the virtualized range moves.
                                let response = ui.interact(
                                    rect, ui.id().with(("hl2_log_row", index)),
                                    egui::Sense::click_and_drag(),
                                ).on_hover_cursor(egui::CursorIcon::Text);
                                let origin = egui::pos2(rect.left(), rect.center().y - galley.size().y * 0.5);
                                let clip = ui.clip_rect().intersect(rect);
                                let painter = ui.painter().with_clip_rect(clip);
                                // Show current search hit(s) underneath the
                                // shared selection; navigation moves this list.
                                let hits = &self.console_search_matches;
                                let first = hits.partition_point(|hit| hit.line < index);
                                for hit in hits[first..].iter().take_while(|hit| hit.line == index) {
                                    let max_col = galley.text().chars().count();
                                    let left = galley.pos_from_cursor(egui::text::CCursor::new(hit.start_col.min(max_col))).left();
                                    let right = galley.pos_from_cursor(egui::text::CCursor::new(hit.end_col.min(max_col))).left();
                                    if right > left {
                                        painter.rect_filled(egui::Rect::from_min_max(
                                            egui::pos2(origin.x + left, rect.top()),
                                            egui::pos2(origin.x + right, rect.bottom()),
                                        ), 0.0, egui::Color32::from_rgb(99, 84, 46));
                                    }
                                }
                                if let Some((left, right)) = selected_columns(
                                    selection, index, galley.text().chars().count(),
                                ) {
                                    let start = galley.pos_from_cursor(egui::text::CCursor::new(left)).left();
                                    let end = galley.pos_from_cursor(egui::text::CCursor::new(right)).left();
                                    painter.rect_filled(egui::Rect::from_min_max(
                                        egui::pos2(origin.x + start, rect.top()),
                                        egui::pos2(origin.x + end, rect.bottom()),
                                    ), 0.0, egui::Color32::from_rgb(68, 107, 146));
                                }
                                painter.galley(origin, galley.clone(), LABEL);
                                if response.clicked() {
                                    if let Some(pointer) = response.interact_pointer_pos() {
                                        let point = galley_point(index, &galley, origin, pointer);
                                        self.begin_console_selection(point);
                                        self.finish_console_selection();
                                    }
                                } else if response.drag_started() {
                                    // Anchor at mouse-down, not at the delayed
                                    // drag threshold position.
                                    if let Some(pointer) = ui.input(|i| i.pointer.press_origin()) {
                                        self.begin_console_selection(galley_point(index, &galley, origin, pointer));
                                    }
                                }
                                let path_target = self.console_path_links.get(index)
                                    .and_then(|links| links.first()).map(|link| link.target.clone());
                                response.context_menu(|ui| {
                                    if ui.add_enabled(self.selected_console_text().is_some(),
                                        egui::Button::new("Copy selected text"))
                                        .clicked()
                                    {
                                        self.copy_console_selection();
                                        ui.close();
                                    }
                                    if ui.button("Select all").clicked() {
                                        self.select_all_console();
                                        ui.close();
                                    }
                                    if chat_id.is_some() || path_target.is_some() {
                                        ui.separator();
                                    }
                                    if let Some(id) = chat_id {
                                        if let Some(action) = self.chat_translation_menu(ui, id) {
                                            chat_action = Some(action);
                                            ui.close();
                                        }
                                    }
                                    if let Some(target) = &path_target {
                                        if chat_id.is_some() { ui.separator(); }
                                        if ui.button("Reveal linked file").clicked() {
                                            reveal_path = Some(target.clone());
                                            ui.close();
                                        }
                                    }
                                });
                                visible_rows.push((index, rect, galley, origin));
                            }
                        });
                    });
                // The active drag can travel across rows that are different
                // widgets or past the clipped viewport. Choose the nearest
                // visible record, then let the next scroll frame extend it.
                if self.console_selecting {
                    if ctx.input(|i| i.pointer.primary_down()) {
                        if let Some(pointer) = ctx.input(|i| i.pointer.interact_pos()) {
                            if !visible_rows.is_empty() {
                                let candidate = visible_rows.iter().min_by(|a, b| {
                                    let distance = |row: &VisibleLogRow| {
                                        if pointer.y < row.1.top() { row.1.top() - pointer.y }
                                        else { (pointer.y - row.1.bottom()).max(0.0) }
                                    };
                                    distance(a).total_cmp(&distance(b))
                                });
                                if let Some((line, _, galley, origin)) = candidate {
                                    self.console_selection_focus = Some(galley_point(*line, galley, *origin, pointer));
                                }
                            }
                        }
                    } else {
                        self.finish_console_selection();
                    }
                }
                // Keep the scrollframe a fixed-height element, even when rows
                // are empty, so its content never resizes the floating panel.
                let _ = log_result;
                if let Some(target) = reveal_path {
                    let _ = self.reveal_console_path(&target);
                }
                if let Some(action) = chat_action {
                    self.apply_chat_menu_action(action);
                }
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(">").monospace().color(DIM));
                    // Raw winit console input remains the only text editor.
                    // Render into a fixed-width clipped field, starting at its
                    // LEFT edge. `add_sized(Label)` centered this text before.
                    let field_width = (width - 120.0).max(40.0);
                    let field = egui::Frame::new().fill(INPUT)
                        .stroke(egui::Stroke::new(1.0, EDGE))
                        .inner_margin(egui::Margin::symmetric(5, 3))
                        .show(ui, |ui| {
                            let (text_rect, response) = ui.allocate_exact_size(
                                egui::vec2((field_width - 10.0).max(1.0), 17.0),
                                egui::Sense::click(),
                            );
                            let font = egui::FontId::monospace(12.5);
                            let color = egui::Color32::BLACK;
                            let cursor = self.console_cursor.min(self.console_input.len());
                            let cursor = if self.console_input.is_char_boundary(cursor) {
                                cursor
                            } else {
                                self.console_input.len()
                            };
                            let text_galley = ui.painter().layout_no_wrap(
                                self.console_input.clone(), font.clone(), color,
                            );
                            let prefix_galley = ui.painter().layout_no_wrap(
                                self.console_input[..cursor].to_owned(), font, color,
                            );
                            let caret_offset = prefix_galley.size().x;
                            let visible_width = text_rect.width().max(1.0);
                            let scroll = (caret_offset - visible_width + 3.0).max(0.0);
                            let painter = ui.painter().with_clip_rect(text_rect);
                            let text_y = text_rect.center().y - text_galley.size().y * 0.5;
                            let input_origin = egui::pos2(text_rect.left() - scroll, text_y);
                            if response.clicked() {
                                if let Some(pointer) = response.interact_pointer_pos() {
                                    let index = text_galley.cursor_from_pos(pointer - input_origin).index.0;
                                    self.console_cursor = self.console_input.char_indices()
                                        .nth(index).map_or(self.console_input.len(), |(byte, _)| byte);
                                }
                            }
                            painter.galley(input_origin, text_galley, color);
                            let caret_x = (text_rect.left() + caret_offset - scroll)
                                .clamp(text_rect.left(), text_rect.right());
                            painter.line_segment(
                                [egui::pos2(caret_x, text_rect.top() + 2.0),
                                 egui::pos2(caret_x, text_rect.bottom() - 2.0)],
                                egui::Stroke::new(1.0, color),
                            );
                        });
                    prompt_rect = Some(field.response.rect);
                    if ui.button("Submit").clicked() {
                        self.execute_console();
                    }
                });
                // Keep the little Source-style corner mark inside the frame.
                // The actual resize hitbox is anchored to window_rect outside
                // this inner content Ui, including the visible outer stroke.
                let corner = ui.max_rect().right_bottom();
                for inset in [5.0, 9.0, 13.0] {
                    ui.painter().line_segment(
                        [egui::pos2(corner.x - inset, corner.y - 2.0),
                         egui::pos2(corner.x - 2.0, corner.y - inset)],
                        egui::Stroke::new(1.0, DIM),
                    );
                }
            });
        let resized_by_pointer = console_resize_grip(ctx, window_rect);
        // Do NOT save result.response.rect. Its measured content size can
        // change when a long log arrives or even just moving the mouse repaints
        // egui. Only intentional drag gestures may modify the saved geometry.
        let next = if let Some((horizontal, vertical, delta)) = resized_by_pointer {
            resize_console_rect([x, y, width, height], delta, horizontal, vertical, work_area)
        } else {
            clamp_console_rect([x + header_drag.x, y + header_drag.y, width, height], work_area)
        };
        if next.iter().zip(self.console_hl2_rect).any(|(a, b)| (a - b).abs() > 0.5) {
            self.console_hl2_rect = next;
            self.mark_config_dirty();
        }
        if close_requested {
            self.set_overlay(self.overlay_after_console());
            return;
        }
        // An independent foreground layer is necessary because the popup
        // extends beneath the window, not into its log. Its *position* is
        // derived solely from window/prompt geometry; it never feeds back into
        // fixed_rect or the saved position/size.
        if let Some((popup, rows)) = prompt_rect.and_then(|prompt| {
            completion_popup_geometry(window_rect, prompt, viewport, suggestions.len(), hint)
        }) {
            egui::Area::new(egui::Id::new("dinurdo_hl2_console_suggestions"))
                .order(egui::Order::Foreground)
                .fixed_pos(popup.min)
                .constrain(false)
                .show(ctx, |ui| {
                    ui.allocate_exact_size(popup.size(), egui::Sense::hover());
                    let painter = ui.painter().with_clip_rect(popup);
                    painter.rect_filled(popup, egui::CornerRadius::ZERO,
                        egui::Color32::from_rgb(53, 57, 59));
                    painter.rect_stroke(popup, egui::CornerRadius::ZERO,
                        egui::Stroke::new(1.0, EDGE), egui::StrokeKind::Inside);
                    let row_width = (popup.width() - 12.0).max(1.0);
                    let mut row_top = popup.top() + 6.0;
                    if hint {
                        painter.text(
                            egui::pos2(popup.left() + 6.0, row_top + 8.0),
                            egui::Align2::LEFT_CENTER, "ARGUMENT",
                            egui::FontId::proportional(11.0), DIM,
                        );
                        row_top += COMPLETION_HINT_HEIGHT;
                    }
                    for (idx, suggestion) in suggestions.iter().take(rows).enumerate() {
                        let row = egui::Rect::from_min_size(
                            egui::pos2(popup.left() + 6.0, row_top),
                            egui::vec2(row_width, COMPLETION_ROW_HEIGHT),
                        );
                        let response = ui.interact(
                            row, ui.id().with(("hl2_completion", idx)), egui::Sense::click(),
                        );
                        let selected = self.console_suggest_picked && self.console_suggest_index == idx;
                        if selected || response.hovered() {
                            painter.rect_filled(row, egui::CornerRadius::ZERO,
                                if selected { egui::Color32::from_rgb(77, 101, 120) }
                                else { egui::Color32::from_rgb(70, 75, 77) });
                        }
                        let font = egui::FontId::monospace(12.0);
                        let value_width = if suggestion.value.is_empty() { 0.0 }
                            else { (row_width * 0.34).min(row_width * 0.5) };
                        let text_pos = egui::pos2(row.left() + 5.0, row.center().y);
                        let name_clip = egui::Rect::from_min_max(
                            row.min,
                            egui::pos2((row.right() - value_width - 5.0).max(row.left()), row.bottom()),
                        );
                        painter.with_clip_rect(name_clip).text(
                            text_pos, egui::Align2::LEFT_CENTER,
                            &suggestion.name, font.clone(), LABEL,
                        );
                        if !suggestion.value.is_empty() {
                            let value_clip = egui::Rect::from_min_max(
                                egui::pos2(row.right() - value_width, row.top()), row.max,
                            );
                            painter.with_clip_rect(value_clip).text(
                                egui::pos2(row.right() - 5.0, row.center().y),
                                egui::Align2::RIGHT_CENTER,
                                &suggestion.value, font, DIM,
                            );
                        }
                        if response.clicked() {
                            self.apply_console_suggestion(&suggestion.name);
                        }
                        response.on_hover_text(suggestion.description);
                        row_top += COMPLETION_ROW_HEIGHT + COMPLETION_ROW_SPACING;
                    }
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hl2_selection_uses_shared_character_positions_across_virtual_rows() {
        let bounds = Some((ConsolePoint { line: 2, col: 3 },
                           ConsolePoint { line: 4, col: 2 }));
        assert_eq!(selected_columns(bounds, 1, 10), None);
        assert_eq!(selected_columns(bounds, 2, 9), Some((3, 9)));
        assert_eq!(selected_columns(bounds, 3, 9), Some((0, 9)));
        assert_eq!(selected_columns(bounds, 4, 9), Some((0, 2)));
        assert_eq!(selected_columns(bounds, 5, 9), None);
        assert_eq!(selected_columns(bounds, 2, 2), None);
        assert_eq!(selected_columns(None, 3, 9), None);
    }

    #[test]
    fn console_geometry_changes_only_on_explicit_resize() {
        let viewport = egui::Rect::from_min_size(egui::pos2(5.0, 5.0), egui::vec2(980.0, 560.0));
        let saved = [60.0, 40.0, 650.0, 350.0];
        // No artificial content feedback: the fixed rectangle remains exactly saved.
        assert_eq!(clamp_console_rect(saved, viewport), saved);
        assert_eq!(resize_console_rect(saved, egui::Vec2::ZERO, 0, 0, viewport), saved);
        assert_eq!(resize_console_rect(saved, egui::vec2(30.0, 0.0), 1, 0, viewport),
            [60.0, 40.0, 680.0, 350.0]);
        assert_eq!(resize_console_rect(saved, egui::vec2(30.0, 0.0), -1, 0, viewport),
            [90.0, 40.0, 620.0, 350.0]);
        assert_eq!(resize_console_rect(saved, egui::vec2(0.0, 30.0), 0, 1, viewport),
            [60.0, 40.0, 650.0, 380.0]);
        let edge = resize_console_rect([300.0, 200.0, 600.0, 350.0],
            egui::vec2(5000.0, 5000.0), 1, 1, viewport);
        assert_eq!(edge[0] + edge[2], viewport.right());
        assert_eq!(edge[1] + edge[3], viewport.bottom());
        assert_eq!(completion_rows_that_fit(150.0, true), 5);
    }


    #[test]
    fn autocomplete_opens_down_and_console_cannot_drag_or_resize_under_it() {
        let viewport = egui::Rect::from_min_size(
            egui::pos2(5.0, 5.0), egui::vec2(1600.0, 900.0),
        );
        let work = console_work_area(viewport);
        assert_eq!(viewport.bottom() - work.bottom(), COMPLETION_MAX_RESERVE);

        for saved in [
            [60.0, 80.0, 700.0, 490.0],
            [730.0, 680.0, 800.0, 595.0], // previously below the screen
            [450.0, 800.0, 420.0, 220.0], // a short window dragged low
        ] {
            let [x, y, w, h] = clamp_console_rect(saved, work);
            let window = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h));
            assert!(window.bottom() <= work.bottom());
            let prompt = egui::Rect::from_min_size(
                egui::pos2(x + 22.0, window.bottom() - 32.0),
                egui::vec2(w - 110.0, 22.0),
            );
            for hint in [false, true] {
                for count in [1, 5, 50] {
                    let (popup, rows) = completion_popup_geometry(
                        window, prompt, viewport, count, hint,
                    ).expect("the reserved strip must always fit completion rows");
                    assert_eq!(rows, count.min(COMPLETION_MAX_ROWS));
                    assert!(popup.top() > window.bottom(), "completion must open DOWN");
                    assert!(popup.top() > prompt.bottom());
                    assert!(popup.bottom() <= viewport.bottom());
                    assert!(popup.left() >= viewport.left() && popup.right() <= viewport.right());
                }
            }
            let resized = resize_console_rect([x, y, w, h],
                egui::vec2(0.0, 5000.0), 0, 1, work);
            assert!(resized[1] + resized[3] <= work.bottom());
            let dragged = clamp_console_rect([x, y + 5000.0, w, h], work);
            assert!(dragged[1] + dragged[3] <= work.bottom());
        }
    }

    #[test]
    fn tiny_viewports_never_flip_autocomplete_upwards() {
        let viewport = egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0), egui::vec2(300.0, 250.0),
        );
        let [x, y, w, h] = clamp_console_rect([10.0, 400.0, 1000.0, 1000.0],
            console_work_area(viewport));
        let window = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h));
        let prompt = egui::Rect::from_min_size(
            egui::pos2(x + 15.0, window.bottom() - 30.0), egui::vec2(w - 40.0, 20.0),
        );
        // On a very short viewport, no complete row fits. Do not relocate the
        // popup to the top of the console or outside the display.
        assert!(completion_popup_geometry(window, prompt, viewport, 5, true).is_none());
        assert!(window.bottom() <= viewport.bottom());
    }

    #[test]
    fn resize_hitboxes_cover_the_visible_border_not_the_inset_content() {
        let frame = egui::Rect::from_min_size(egui::pos2(100.0, 80.0), egui::vec2(700.0, 420.0));
        let grips = console_resize_hit_rects(frame);
        let cases = [
            ("nw", egui::pos2(100.0, 80.0)),
            ("ne", egui::pos2(800.0, 80.0)),
            ("sw", egui::pos2(100.0, 500.0)),
            ("se", egui::pos2(800.0, 500.0)),
            ("w", egui::pos2(100.0, 300.0)),
            ("e", egui::pos2(800.0, 300.0)),
            ("n", egui::pos2(450.0, 80.0)),
            ("s", egui::pos2(450.0, 500.0)),
        ];
        for (name, point) in cases {
            let hits: Vec<_> = grips.iter().filter(|(_, _, _, hit)| hit.contains(point)).collect();
            assert_eq!(hits.len(), 1, "visible border {name} must have exactly one grip");
            assert_eq!(hits[0].0, name);
        }
        // Explicitly cover 4 points OUTSIDE the border, while leaving log,
        // header body, and command prompt usable a little farther inside.
        for point in [
            egui::pos2(97.0, 300.0), egui::pos2(803.0, 300.0),
            egui::pos2(450.0, 77.0), egui::pos2(450.0, 503.0),
        ] {
            assert_eq!(grips.iter().filter(|(_, _, _, hit)| hit.contains(point)).count(), 1);
        }
        assert!(grips.iter().all(|(_, _, _, hit)| !hit.contains(egui::pos2(450.0, 300.0))));
    }

    #[test]
    fn tiny_viewports_clamp_without_inverting() {
        let viewport = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(300.0, 250.0));
        let [x, y, w, h] = clamp_console_rect([0.0, 0.0, 1000.0, 1000.0], viewport);
        assert!(x + w <= viewport.right());
        assert!(y + h <= viewport.bottom());
    }
}
