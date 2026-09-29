//! Shared visual language for every egui menu surface.
//!
//! The top bar, Setup tab strip, settings pages and modal dialogs all pull
//! their colors, metrics and widgets from here so the menu reads as one
//! system instead of a pile of one-off panels. The language is deliberately
//! flat: square corners everywhere, hairline rules for structure, and filled
//! states that stay dark enough for light text to keep a comfortable contrast
//! ratio over the live scene behind the panel.

// ---------------------------------------------------------------- palette --

/// Window chrome behind the top bar and tab strip.
pub(super) const CHROME: egui::Color32 = egui::Color32::from_rgb(0x0B, 0x0F, 0x15);
/// Body surface for panels and pages.
pub(super) const SURFACE: egui::Color32 = egui::Color32::from_rgb(0x13, 0x18, 0x20);
/// Slightly lifted surface used for sidebars and grouped blocks.
pub(super) const SURFACE_ALT: egui::Color32 = egui::Color32::from_rgb(0x17, 0x1D, 0x26);
/// Sunken surface for text fields and slider rails.
pub(super) const INSET: egui::Color32 = egui::Color32::from_rgb(0x0D, 0x11, 0x17);
/// Resting fill for interactive controls.
pub(super) const CONTROL: egui::Color32 = egui::Color32::from_rgb(0x20, 0x28, 0x33);
pub(super) const CONTROL_HOVER: egui::Color32 = egui::Color32::from_rgb(0x2B, 0x35, 0x43);
/// Selected fill. Kept dark on purpose so the label above it stays light.
pub(super) const CONTROL_SELECTED: egui::Color32 = egui::Color32::from_rgb(0x1C, 0x3E, 0x63);
pub(super) const CONTROL_SELECTED_HOVER: egui::Color32 = egui::Color32::from_rgb(0x24, 0x4D, 0x79);

pub(super) const HAIRLINE: egui::Color32 = egui::Color32::from_rgb(0x1E, 0x25, 0x2F);
pub(super) const LINE: egui::Color32 = egui::Color32::from_rgb(0x2A, 0x33, 0x40);
pub(super) const LINE_STRONG: egui::Color32 = egui::Color32::from_rgb(0x3C, 0x49, 0x5A);

pub(super) const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x59, 0xA9, 0xFF);
pub(super) const ACCENT_DEEP: egui::Color32 = egui::Color32::from_rgb(0x2C, 0x63, 0xA8);
pub(super) const ACCENT_HOVER: egui::Color32 = egui::Color32::from_rgb(0x3A, 0x7C, 0xCB);

pub(super) const TEXT: egui::Color32 = egui::Color32::from_rgb(0xEB, 0xF0, 0xF6);
pub(super) const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(0xAD, 0xBA, 0xC9);
pub(super) const TEXT_FAINT: egui::Color32 = egui::Color32::from_rgb(0x8B, 0x99, 0xAA);
pub(super) const TEXT_DISABLED: egui::Color32 = egui::Color32::from_rgb(0x55, 0x5F, 0x6C);

pub(super) const WARNING: egui::Color32 = egui::Color32::from_rgb(0xFF, 0xB8, 0x3C);
pub(super) const DANGER: egui::Color32 = egui::Color32::from_rgb(0xFF, 0x74, 0x66);

/// Drop shadow painted behind menu captions so they stay readable over the
/// live, un-blurred world on the Video page.
pub(super) const SHADOW: egui::Color32 =
    egui::Color32::from_rgba_premultiplied(0x01, 0x03, 0x08, 0xE6);

/// Scrim behind the Video page column. Deliberately thin: the whole point of
/// leaving that page un-blurred is to watch a setting take effect, so the
/// scrim only has to lift text off the world, not hide it.
pub(super) const VIDEO_SCRIM: egui::Color32 =
    egui::Color32::from_rgba_premultiplied(0x06, 0x09, 0x0E, 0x99);

/// Video-page rail. Translucent so the world stays partly visible beside the
/// settings it is controlling.
pub(super) const RAIL_FILL: egui::Color32 =
    egui::Color32::from_rgba_premultiplied(0x0B, 0x0F, 0x15, 0xE0);

/// Panel body for the pages that sit over the renderer's blurred menu
/// backdrop. The Video page does not use this: it stays transparent so the
/// world behind it shows the effect of each setting as it changes.
pub(super) const PANEL_FILL: egui::Color32 =
    egui::Color32::from_rgba_premultiplied(0x11, 0x16, 0x1D, 0xF7);

// ---------------------------------------------------------------- metrics --

pub(super) const TOP_BAR_H: f32 = 44.0;
pub(super) const TAB_BAR_H: f32 = 34.0;
pub(super) const FOOTER_H: f32 = 40.0;
pub(super) const ROW_H: f32 = 30.0;
pub(super) const LABEL_W: f32 = 210.0;
pub(super) const VALUE_W: f32 = 132.0;
pub(super) const NAV_W: f32 = 170.0;

/// egui zoom derived from the drawable height so the menu keeps a usable
/// physical size from 900p up to 4K without a manual UI-scale setting.
pub(super) fn zoom_for_height(height: f32) -> f32 {
    (height / 1080.0).clamp(0.85, 2.0)
}

// ------------------------------------------------------------------ style --

fn widget(
    bg_fill: egui::Color32,
    weak_bg_fill: egui::Color32,
    stroke: egui::Color32,
    text: egui::Color32,
) -> egui::style::WidgetVisuals {
    egui::style::WidgetVisuals {
        bg_fill,
        weak_bg_fill,
        bg_stroke: egui::Stroke::new(1.0_f32, stroke),
        corner_radius: egui::CornerRadius::ZERO,
        fg_stroke: egui::Stroke::new(1.0_f32, text),
        expansion: 0.0,
    }
}

pub(super) fn apply(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = SURFACE;
    visuals.window_fill = SURFACE;
    visuals.window_stroke = egui::Stroke::new(1.0_f32, LINE_STRONG);
    visuals.window_corner_radius = egui::CornerRadius::ZERO;
    visuals.menu_corner_radius = egui::CornerRadius::ZERO;
    visuals.window_shadow = egui::epaint::Shadow::NONE;
    visuals.popup_shadow = egui::epaint::Shadow::NONE;
    visuals.extreme_bg_color = INSET;
    visuals.faint_bg_color = SURFACE_ALT;
    visuals.code_bg_color = INSET;
    visuals.warn_fg_color = WARNING;
    visuals.error_fg_color = DANGER;
    visuals.hyperlink_color = ACCENT;
    visuals.override_text_color = None;
    visuals.weak_text_color = Some(TEXT_FAINT);
    visuals.selection.bg_fill = CONTROL_SELECTED;
    visuals.selection.stroke = egui::Stroke::new(1.0_f32, TEXT);
    visuals.slider_trailing_fill = true;
    visuals.handle_shape = egui::style::HandleShape::Rect { aspect_ratio: 0.35 };
    visuals.interact_cursor = Some(egui::CursorIcon::PointingHand);
    visuals.indent_has_left_vline = false;
    visuals.striped = false;
    visuals.disabled_alpha = 0.45;

    visuals.widgets.noninteractive = widget(SURFACE, SURFACE, HAIRLINE, TEXT_DIM);
    visuals.widgets.inactive = widget(CONTROL, CONTROL, LINE, TEXT_DIM);
    visuals.widgets.hovered = widget(CONTROL_HOVER, CONTROL_HOVER, LINE_STRONG, TEXT);
    visuals.widgets.active = widget(CONTROL_SELECTED, CONTROL_SELECTED, ACCENT, TEXT);
    visuals.widgets.open = widget(CONTROL_HOVER, CONTROL_HOVER, LINE_STRONG, TEXT);
    // Slider rails and check boxes read their fill from `inactive.bg_fill`;
    // the sunken tone separates them from the flat button chrome.
    visuals.widgets.inactive.bg_fill = INSET;
    visuals.widgets.hovered.bg_fill = INSET;

    ctx.set_visuals(visuals);

    ctx.global_style_mut(|style| {
        use egui::{FontFamily::Proportional, FontId, TextStyle};
        style.text_styles = [
            (TextStyle::Heading, FontId::new(21.0, Proportional)),
            (TextStyle::Body, FontId::new(13.5, Proportional)),
            (TextStyle::Button, FontId::new(13.0, Proportional)),
            (TextStyle::Small, FontId::new(11.5, Proportional)),
            (
                TextStyle::Monospace,
                FontId::new(12.5, egui::FontFamily::Monospace),
            ),
        ]
        .into();

        // Selecting label text with the mouse looked like a bug on a game
        // menu, and every label here is a caption rather than content.
        style.interaction.selectable_labels = false;
        style.interaction.multi_widget_text_select = false;
        style.wrap_mode = Some(egui::TextWrapMode::Extend);

        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(11.0, 5.0);
        style.spacing.interact_size = egui::vec2(24.0, 22.0);
        style.spacing.slider_rail_height = 5.0;
        style.spacing.icon_width = 15.0;
        style.spacing.icon_width_inner = 9.0;
        style.spacing.menu_margin = egui::Margin::same(4);
        style.spacing.combo_width = 60.0;
        style.spacing.scroll.bar_width = 9.0;
        style.spacing.scroll.floating = false;
        style.spacing.scroll.bar_inner_margin = 2.0;
        style.spacing.scroll.bar_outer_margin = 0.0;
    });
}

// ------------------------------------------------------------ primitives --

pub(super) fn plain(text: &str, size: f32, color: egui::Color32) -> egui::RichText {
    egui::RichText::new(text).size(size).color(color)
}

pub(super) fn label(ui: &mut egui::Ui, text: egui::RichText) -> egui::Response {
    ui.add(egui::Label::new(text).selectable(false))
}

/// Draws `text` with a tight dark-blue halo.
///
/// The Video page deliberately leaves the world un-blurred behind it so the
/// effect of a setting is visible while it is being changed. That means every
/// caption has to stay legible over arbitrary geometry, which the halo buys
/// without painting a slab of panel behind the text.
pub(super) fn glow_text(
    painter: &egui::Painter,
    pos: egui::Pos2,
    anchor: egui::Align2,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
) {
    // A drop shadow, not an outline: ringing the glyphs on all sides made the
    // text look embossed against bright geometry.
    for offset in [egui::vec2(1.0, 1.0), egui::vec2(1.0, 0.0)] {
        painter.text(pos + offset, anchor, text, font.clone(), SHADOW);
    }
    painter.text(pos, anchor, text, font, color);
}

/// A haloed text run laid out as a widget, for use inside a control column.
pub(super) fn glow_label(ui: &mut egui::Ui, text: &str, size: f32, color: egui::Color32) {
    let font = egui::FontId::proportional(size);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font.clone(), color);
    let (rect, _) = ui.allocate_exact_size(galley.size(), egui::Sense::hover());
    let painter = ui.painter().clone();
    glow_text(
        &painter,
        rect.left_center(),
        egui::Align2::LEFT_CENTER,
        text,
        font,
        color,
    );
}

/// Fills `rect` and draws an optional 1px underline in `accent`.
fn fill_with_underline(
    painter: &egui::Painter,
    rect: egui::Rect,
    fill: egui::Color32,
    underline: Option<egui::Color32>,
) {
    painter.rect_filled(rect, egui::CornerRadius::ZERO, fill);
    if let Some(color) = underline {
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.bottom() - 2.0),
                rect.right_bottom(),
            ),
            egui::CornerRadius::ZERO,
            color,
        );
    }
}

/// A flat, square nav item: used for both the top bar and the Setup tabs.
pub(super) fn nav_item(
    ui: &mut egui::Ui,
    text: &str,
    size: f32,
    width: f32,
    height: f32,
    selected: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let painter = ui.painter().clone();
    let hovered = response.hovered();
    let fill = match (selected, hovered) {
        (true, _) => CONTROL_SELECTED,
        (false, true) => CONTROL_HOVER,
        (false, false) => egui::Color32::TRANSPARENT,
    };
    if fill != egui::Color32::TRANSPARENT {
        fill_with_underline(&painter, rect, fill, selected.then_some(ACCENT));
    }
    let color = if selected || hovered { TEXT } else { TEXT_DIM };
    glow_text(
        &painter,
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(size),
        color,
    );
    response
}

/// One option inside a segmented control.
pub(super) fn chip(ui: &mut egui::Ui, text: &str, selected: bool) -> egui::Response {
    let font = egui::FontId::proportional(12.5);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font.clone(), TEXT);
    let size = egui::vec2(galley.size().x + 20.0, 23.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let painter = ui.painter().clone();
    let fill = match (selected, response.hovered()) {
        (true, false) => CONTROL_SELECTED,
        (true, true) => CONTROL_SELECTED_HOVER,
        (false, false) => CONTROL,
        (false, true) => CONTROL_HOVER,
    };
    fill_with_underline(&painter, rect, fill, selected.then_some(ACCENT));
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        font,
        if selected { TEXT } else { TEXT_DIM },
    );
    response
}

/// Rectangular on/off switch. Returns the new value when the user flips it.
pub(super) fn switch(ui: &mut egui::Ui, on: bool) -> Option<bool> {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(40.0, 20.0), egui::Sense::click());
    let painter = ui.painter().clone();
    let track = match (on, response.hovered()) {
        (true, false) => ACCENT_DEEP,
        (true, true) => ACCENT,
        (false, false) => CONTROL,
        (false, true) => CONTROL_HOVER,
    };
    painter.rect_filled(rect, egui::CornerRadius::ZERO, track);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(1.0_f32, if on { ACCENT } else { LINE }),
        egui::StrokeKind::Inside,
    );
    let knob_w = 16.0;
    let knob_x = if on {
        rect.right() - knob_w - 2.0
    } else {
        rect.left() + 2.0
    };
    painter.rect_filled(
        egui::Rect::from_min_size(
            egui::pos2(knob_x, rect.top() + 2.0),
            egui::vec2(knob_w, rect.height() - 4.0),
        ),
        egui::CornerRadius::ZERO,
        if on { TEXT } else { TEXT_FAINT },
    );
    ui.add_space(8.0);
    glow_label(
        ui,
        if on { "On" } else { "Off" },
        12.5,
        if on { TEXT } else { TEXT_FAINT },
    );
    response.clicked().then_some(!on)
}

/// Small padlock toggle drawn with the painter (no font dependency). Closed
/// means the value is linked to another control; open means it is independent.
/// Returns the new locked state when clicked.
pub(super) fn lock_toggle(ui: &mut egui::Ui, locked: bool) -> Option<bool> {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
    let painter = ui.painter().clone();
    let fill = match (locked, response.hovered()) {
        (true, false) => CONTROL,
        (true, true) => CONTROL_HOVER,
        (false, false) => CONTROL_SELECTED,
        (false, true) => CONTROL_SELECTED_HOVER,
    };
    painter.rect_filled(rect, egui::CornerRadius::ZERO, fill);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(1.0_f32, if locked { LINE } else { ACCENT }),
        egui::StrokeKind::Inside,
    );
    let color = if locked { TEXT_DIM } else { ACCENT };
    let stroke = egui::Stroke::new(1.6_f32, color);
    let center_x = rect.center().x;
    let body = egui::Rect::from_min_size(
        egui::pos2(center_x - 5.0, rect.center().y - 1.0),
        egui::vec2(10.0, 7.0),
    );
    painter.rect_filled(body, egui::CornerRadius::ZERO, color);
    // Shackle: two posts and a top bar. The open padlock lifts and shifts the
    // right post so the shackle visibly detaches from the body.
    let top = body.top() - 5.0;
    let left_x = center_x - 3.0;
    let right_x = center_x + 3.0;
    painter.line_segment([egui::pos2(left_x, body.top()), egui::pos2(left_x, top)], stroke);
    painter.line_segment([egui::pos2(left_x, top), egui::pos2(right_x, top)], stroke);
    if locked {
        painter.line_segment([egui::pos2(right_x, top), egui::pos2(right_x, body.top())], stroke);
    } else {
        painter.line_segment([egui::pos2(right_x, top), egui::pos2(right_x, top + 2.5)], stroke);
    }
    let response = response.on_hover_text(if locked {
        "Locked: follows the master brightness slider. Click to adjust independently."
    } else {
        "Unlocked: adjusted independently. Click to link to the master slider again."
    });
    response.clicked().then_some(!locked)
}

/// Segmented quality meter: the bar fills up to the chosen level so the cost
/// ordering of the options is visible at a glance. Clicking or dragging a
/// segment selects it.
///
/// A ladder whose first value is literally `Off` is presented as a master
/// switch followed by a meter containing only the enabled quality levels. Off
/// therefore has no segment of its own: switch Off means zero filled segments;
/// switch On always means at least the first quality segment is filled. The
/// widget remembers the most recent enabled level so an Off -> On A/B toggle
/// restores it instead of silently dropping back to the cheapest mode.
pub(super) fn quality_meter(
    ui: &mut egui::Ui,
    current: usize,
    labels: &[&str],
    track_width: f32,
) -> Option<usize> {
    quality_meter_impl(ui, Some(current), labels, track_width, None)
}

/// Same ordered segmented meter as [`quality_meter`], but permits a genuine
/// unselected state. In that state no segment is filled or ticked and
/// `empty_label` is shown to the right. This is useful for quality presets:
/// once any preset-owned cvar is edited, the settings are Custom rather than a
/// different preset.
pub(super) fn quality_meter_optional(
    ui: &mut egui::Ui,
    current: Option<usize>,
    labels: &[&str],
    track_width: f32,
    empty_label: &str,
) -> Option<usize> {
    quality_meter_impl(ui, current, labels, track_width, Some(empty_label))
}

fn quality_meter_impl(
    ui: &mut egui::Ui,
    current: Option<usize>,
    labels: &[&str],
    track_width: f32,
    empty_label: Option<&str>,
) -> Option<usize> {
    let segments = labels.len();
    if segments == 0 {
        return None;
    }
    let current = current.map(|value| value.min(segments - 1));

    // `Off` is an enable state, not a quality level. Keep it out of the meter
    // so segment 0 always means the first *enabled* quality choice.
    if labels[0].eq_ignore_ascii_case("off") && segments > 1 {
        let on = current.is_some_and(|selected| selected > 0);
        let last_on_id = ui.id().with("quality_meter_last_on");

        // Keep the restore target warm whenever the setting is enabled. Temp
        // UI memory is sufficient: this is presentation state, not a cvar.
        if let Some(selected) = current.filter(|selected| *selected > 0) {
            ui.ctx()
                .data_mut(|data| data.insert_temp(last_on_id, selected));
        }

        if let Some(next_on) = switch(ui, on) {
            if next_on {
                let restore = ui
                    .ctx()
                    .data(|data| data.get_temp::<usize>(last_on_id))
                    .unwrap_or(1)
                    .clamp(1, segments - 1);
                return Some(restore);
            }
            if let Some(selected) = current.filter(|selected| *selected > 0) {
                ui.ctx()
                    .data_mut(|data| data.insert_temp(last_on_id, selected));
            }
            return Some(0);
        }

        ui.add_space(10.0);
        let enabled_labels = &labels[1..];
        let selected_enabled = current
            .filter(|selected| *selected > 0)
            .map(|selected| selected - 1);

        // Recompute after the switch/readout consumed horizontal space. This
        // keeps the enabled-quality meter aligned without reserving a phantom
        // segment for Off.
        let enabled_track_width = self::track_width(ui).min(track_width);
        if let Some(target) = quality_meter_track(
            ui,
            selected_enabled,
            enabled_labels,
            enabled_track_width,
            if on { None } else { Some("") },
        ) {
            // Clicking a quality segment while disabled is an explicit request
            // to enable that quality, so the returned enum index is +1.
            return Some(target + 1);
        }
        return None;
    }

    quality_meter_track(ui, current, labels, track_width, empty_label)
}

fn quality_meter_track(
    ui: &mut egui::Ui,
    current: Option<usize>,
    labels: &[&str],
    track_width: f32,
    empty_label: Option<&str>,
) -> Option<usize> {
    let segments = labels.len();
    if segments == 0 {
        return None;
    }
    let current = current.map(|value| value.min(segments - 1));
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(track_width, 18.0), egui::Sense::click_and_drag());
    let gap = 2.0;
    let seg_w = ((rect.width() - gap * (segments - 1) as f32) / segments as f32).max(2.0);
    let index_at = |x: f32| {
        (((x - rect.left()) / (seg_w + gap)).floor() as i32).clamp(0, segments as i32 - 1) as usize
    };
    let hovered = response
        .hover_pos()
        .filter(|pos| rect.contains(*pos))
        .map(|pos| index_at(pos.x));

    let painter = ui.painter().clone();
    for segment in 0..segments {
        let x = rect.left() + segment as f32 * (seg_w + gap);
        let seg_rect =
            egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(seg_w, rect.height()));
        let lit = current.is_some_and(|selected| segment <= selected);
        let fill = if lit {
            // Ramp the filled run from deep to bright so "more" reads as
            // "heavier" rather than as a flat block of one color.
            let selected = current.expect("lit segments require an active selection");
            let t = if selected == 0 {
                1.0
            } else {
                segment as f32 / selected as f32
            };
            ACCENT_DEEP.lerp_to_gamma(ACCENT, t)
        } else if hovered == Some(segment) {
            CONTROL_HOVER
        } else {
            CONTROL
        };
        painter.rect_filled(seg_rect, egui::CornerRadius::ZERO, fill);
    }

    // Mark the exact selected level. An empty/disabled meter has no marker.
    if let Some(selected) = current {
        let tick_x = rect.left() + selected as f32 * (seg_w + gap) + seg_w - 2.0;
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(tick_x, rect.top() - 2.0),
                egui::vec2(2.0, rect.height() + 4.0),
            ),
            egui::CornerRadius::ZERO,
            TEXT,
        );
    }

    ui.add_space(10.0);
    match current {
        Some(selected) => glow_label(ui, labels[selected], 12.5, TEXT),
        None => glow_label(ui, empty_label.unwrap_or("Custom"), 12.5, TEXT_FAINT),
    };

    let interacted = response.clicked() || response.dragged();
    let target = interacted
        .then(|| response.interact_pointer_pos().map(|pos| index_at(pos.x)))
        .flatten();
    target.filter(|value| current != Some(*value))
}

/// Which stored setting a row's label restores when it is clicked.
///
/// Rows carry a tag rather than a reset closure because the closure would have
/// to borrow `App` mutably at the same time as the control closure does. The
/// tag is posted to egui's frame storage and drained once, after the page has
/// been built, by `App::apply_pending_reset`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Reset {
    /// Row has no stored default worth restoring.
    None,
    /// A `ui::VIDEO_ROW_*` row.
    Video(usize),
    /// A `ui::ENV_ROW_*` row.
    Environment(usize),
    /// A `ui::CLOUD_ROW_*` row.
    CloudTuning(usize),
    /// A client-side visual-physics field on Video -> Physics.
    Physics(u8),
    /// A global ocean setting, indexed by `OceanField`.
    Ocean(u8),
    /// A per-cascade ocean setting: cascade index, then `OceanField`.
    OceanCascade(u8, u8),
}

fn reset_channel() -> egui::Id {
    egui::Id::new("jka_pending_setting_reset")
}

/// Drains the reset posted by a label click this frame, if any.
pub(super) fn take_reset(ctx: &egui::Context) -> Option<Reset> {
    ctx.data_mut(|data| {
        let pending = data.get_temp::<Reset>(reset_channel());
        if pending.is_some() {
            data.remove::<Reset>(reset_channel());
        }
        pending
    })
}

// ----------------------------------------------------------------- layout --

/// Section heading with a rule that runs to the right edge.
pub(super) fn section(ui: &mut egui::Ui, title: &str, detail: &str) {
    ui.add_space(16.0);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 22.0), egui::Sense::hover());
    let painter = ui.painter().clone();
    let font = egui::FontId::proportional(12.5);
    let galley = painter.layout_no_wrap(title.to_owned(), font.clone(), ACCENT);
    glow_text(
        &painter,
        rect.left_center(),
        egui::Align2::LEFT_CENTER,
        title,
        font,
        ACCENT,
    );
    let rule_x = rect.left() + galley.size().x + 12.0;
    if rule_x < rect.right() {
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(rule_x, rect.center().y),
                egui::vec2(rect.right() - rule_x, 1.0),
            ),
            egui::CornerRadius::ZERO,
            LINE,
        );
    }
    if !detail.is_empty() {
        let (detail_rect, _) =
            ui.allocate_exact_size(egui::vec2(width, 16.0), egui::Sense::hover());
        let painter = ui.painter().clone();
        glow_text(
            &painter,
            detail_rect.left_center(),
            egui::Align2::LEFT_CENTER,
            detail,
            egui::FontId::proportional(11.5),
            TEXT_FAINT,
        );
    }
    ui.add_space(4.0);
}

/// A label/control row with a hairline baseline. `tip` is shown when the
/// pointer rests on the label; pass `""` for a row that needs no explanation.
/// The control closure runs in a left-to-right `Ui` that starts at the control
/// column.
pub(super) fn row<R>(
    ui: &mut egui::Ui,
    text: &str,
    tip: &str,
    reset: Reset,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    row_inner(ui, text, tip, reset, false, None, add)
}

/// Same as [`row`], but the label is emphasised and an explanation of the
/// selected option is printed under the control. Used for the multi-mode
/// renderer settings.
pub(super) fn row_with_note<R>(
    ui: &mut egui::Ui,
    text: &str,
    tip: &str,
    reset: Reset,
    note: &str,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    row_inner(ui, text, tip, reset, true, Some(note), add)
}

fn row_inner<R>(
    ui: &mut egui::Ui,
    text: &str,
    tip: &str,
    reset: Reset,
    emphasise: bool,
    note: Option<&str>,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let width = ui.available_width();
    let note_h = if note.is_some() { 17.0 } else { 0.0 };
    let height = ROW_H + note_h;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter().clone();

    // Only the label column is interactive, so resting on (or clicking) a
    // slider or meter is never intercepted by the label behind it.
    let resettable = reset != Reset::None;
    let label_font = egui::FontId::proportional(13.5);
    let label_width = painter
        .layout_no_wrap(text.to_owned(), label_font.clone(), TEXT)
        .size()
        .x;
    let label_rect = egui::Rect::from_min_max(
        rect.left_top(),
        egui::pos2(
            (rect.left() + label_width + 4.0).min(rect.left() + LABEL_W - 8.0),
            rect.top() + ROW_H,
        ),
    );
    let sense = if resettable {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let mut response = ui.interact(label_rect, ui.id().with(("row_label", text)), sense);
    if !tip.is_empty() {
        let hint = if resettable {
            format!("{tip}

Click the label to restore the default.")
        } else {
            tip.to_owned()
        };
        response = response.on_hover_text(hint);
    }
    let hovered = response.hovered();
    if response.clicked() {
        ui.ctx()
            .data_mut(|data| data.insert_temp(reset_channel(), reset));
    }

    let label_color = if emphasise || hovered { TEXT } else { TEXT_DIM };
    let label_pos = egui::pos2(rect.left(), rect.top() + ROW_H * 0.5);
    glow_text(
        &painter,
        label_pos,
        egui::Align2::LEFT_CENTER,
        text,
        label_font,
        label_color,
    );
    // Underline on hover, so it is discoverable that the label itself is the
    // control that restores the default.
    if resettable && hovered {
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(rect.left(), label_pos.y + 9.0),
                egui::vec2(label_width, 1.0),
            ),
            egui::CornerRadius::ZERO,
            ACCENT,
        );
    }
    if let Some(note) = note {
        glow_text(
            &painter,
            egui::pos2(rect.left() + LABEL_W, rect.top() + ROW_H + 1.0),
            egui::Align2::LEFT_TOP,
            note,
            egui::FontId::proportional(11.5),
            TEXT_FAINT,
        );
    }
    painter.rect_filled(
        egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.bottom() - 1.0),
            egui::vec2(rect.width(), 1.0),
        ),
        egui::CornerRadius::ZERO,
        HAIRLINE,
    );

    // The row's height is already reserved above, so the control lives in a
    // child that does NOT advance the parent cursor: `scope_builder` would pull
    // it back to the control's own height and let the next row overlap.
    let control_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + LABEL_W, rect.top()),
        egui::pos2(rect.right(), rect.top() + ROW_H),
    );
    let mut control = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(text)
            .max_rect(control_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    add(&mut control)
}

/// Width available to a meter or slider track inside a control column.
pub(super) fn track_width(ui: &egui::Ui) -> f32 {
    (ui.available_width() - VALUE_W).clamp(120.0, 320.0)
}

/// Slider with the numeric readout printed beside it instead of inside a box.
pub(super) fn slider(
    ui: &mut egui::Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    readout: &str,
) -> bool {
    // Slider ignores `add_sized` for its rail, so drive it through the spacing
    // width instead. Matching `track_width` keeps every slider readout on the
    // same column as the quality meters' value labels.
    let width = track_width(ui);
    ui.spacing_mut().slider_width = width;
    let changed = ui
        .add(
            egui::Slider::new(value, range)
                .show_value(false)
                .trailing_fill(true),
        )
        .changed();
    ui.add_space(10.0);
    glow_label(ui, readout, 12.5, TEXT);
    changed
}

/// Right-aligned hint printed at the end of a control row.
pub(super) fn hint(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        glow_label(ui, text, 11.5, color);
    });
}

/// A full-width banner used for "needs apply" and similar page-level notices.
pub(super) fn banner(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 26.0), egui::Sense::hover());
    let painter = ui.painter().clone();
    painter.rect_filled(rect, egui::CornerRadius::ZERO, SURFACE_ALT);
    painter.rect_filled(
        egui::Rect::from_min_size(rect.left_top(), egui::vec2(2.0, rect.height())),
        egui::CornerRadius::ZERO,
        color,
    );
    painter.text(
        egui::pos2(rect.left() + 12.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(11.5),
        color,
    );
}

/// Primary call-to-action button. Square, filled, light text.
pub(super) fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let font = egui::FontId::proportional(13.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font.clone(), TEXT);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(galley.size().x + 32.0, 28.0),
        egui::Sense::click(),
    );
    let painter = ui.painter().clone();
    // Light text on a mid-blue fill: the reverse (near-black on bright blue)
    // only reaches ~3:1 and was hard to read on this panel.
    let fill = if response.hovered() {
        ACCENT_HOVER
    } else {
        ACCENT_DEEP
    };
    painter.rect_filled(rect, egui::CornerRadius::ZERO, fill);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(1.0_f32, ACCENT),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        font,
        egui::Color32::from_rgb(0xF4, 0xF9, 0xFF),
    );
    response
}

/// Secondary button: outline only.
pub(super) fn ghost_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let font = egui::FontId::proportional(13.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font.clone(), TEXT);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(galley.size().x + 32.0, 28.0),
        egui::Sense::click(),
    );
    let painter = ui.painter().clone();
    painter.rect_filled(
        rect,
        egui::CornerRadius::ZERO,
        if response.hovered() {
            CONTROL_HOVER
        } else {
            CONTROL
        },
    );
    painter.rect_stroke(
        rect,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(
            1.0_f32,
            if response.hovered() { ACCENT } else { LINE_STRONG },
        ),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        font,
        if response.hovered() { TEXT } else { TEXT_DIM },
    );
    response
}

/// Page title block shown at the top of every Setup page.
pub(super) fn page_title(ui: &mut egui::Ui, title: &str, detail: &str) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), egui::Sense::hover());
    let painter = ui.painter().clone();
    glow_text(
        &painter,
        rect.left_center(),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(19.0),
        TEXT,
    );
    if !detail.is_empty() {
        let (detail_rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 18.0), egui::Sense::hover());
        let painter = ui.painter().clone();
        glow_text(
            &painter,
            detail_rect.left_center(),
            egui::Align2::LEFT_CENTER,
            detail,
            egui::FontId::proportional(12.0),
            TEXT_FAINT,
        );
    }
    ui.add_space(8.0);
}
