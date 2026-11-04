//! Taffy owns the layout and hitboxes for the server join / map loading UI.
//! The native HUD painter consumes these rectangles; no egui widgets or
//! independently computed mouse targets are used for this screen.
use super::{flex::{Bounds, Container, Flex}, MapLoadingUi};
use taffy::prelude::{AlignItems, JustifyContent};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConnectionAction {
    CancelJoin,
    Download,
    ConnectAnyway,
    Abort,
    PasswordField,
    PasswordConnect,
    PasswordCancel,
}

#[derive(Clone, Debug)]
pub(crate) enum ConnectionPromptUi {
    MissingMap { map_name: String, reason: String, can_download: bool },
    Password { target: String, message: Option<String>, masked_len: usize },
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DialogLayout {
    pub panel: Bounds,
    pub kicker: Bounds,
    pub title: Bounds,
    pub subject: Bounds,
    pub explanation: Bounds,
    pub help: Bounds,
    pub primary: Bounds,
    pub secondary: Bounds,
    pub tertiary: Bounds,
}

#[derive(Clone, Debug)]
pub(crate) struct LoadingLayout {
    pub panel: Bounds,
    pub kicker: Bounds,
    pub heading: Bounds,
    pub percentage: Bounds,
    pub overall_track: Bounds,
    /// [label, rail, readout] for each visible enabled stage.
    pub rows: Vec<[Bounds; 3]>,
    pub status: Bounds,
    pub cancel: Option<Bounds>,
    pub dialog: Option<DialogLayout>,
    /// All geometry and font sizes use physical pixels, not egui points.
    pub text_scale: f32,
}

impl LoadingLayout {
    pub(crate) fn hit_test(&self, p: [f32; 2], prompt: Option<&ConnectionPromptUi>) -> Option<ConnectionAction> {
        use super::flex::contains;
        if let (Some(dialog), Some(prompt)) = (&self.dialog, prompt) {
            if contains(dialog.primary, p) {
                return match prompt {
                    ConnectionPromptUi::MissingMap { can_download: true, .. } => Some(ConnectionAction::Download),
                    ConnectionPromptUi::MissingMap { .. } => None,
                    ConnectionPromptUi::Password { .. } => Some(ConnectionAction::PasswordConnect),
                };
            }
            if contains(dialog.secondary, p) {
                return Some(match prompt {
                    ConnectionPromptUi::MissingMap { .. } => ConnectionAction::ConnectAnyway,
                    ConnectionPromptUi::Password { .. } => ConnectionAction::PasswordCancel,
                });
            }
            if matches!(prompt, ConnectionPromptUi::MissingMap { .. }) && contains(dialog.tertiary, p) {
                return Some(ConnectionAction::Abort);
            }
            if matches!(prompt, ConnectionPromptUi::Password { .. }) && contains(dialog.subject, p) {
                return Some(ConnectionAction::PasswordField);
            }
            // A modal blocks all other loading-screen clicks, including cancel.
            return None;
        }
        self.cancel.filter(|&r| contains(r, p)).map(|_| ConnectionAction::CancelJoin)
    }
}

pub(crate) fn layout(loading: &MapLoadingUi, width: u32, height: u32) -> LoadingLayout {
    let (w, h) = (width.max(1) as f32, height.max(1) as f32);
    // Small but legible. Do not expand 6x8 bitmap glyphs with screen size:
    // loading.rs uses the clean UI atlas and true pixel-height text metrics.
    let s = (h / 900.0).min(w / 1250.0).clamp(0.78, 1.15);
    let pad = 22.0 * s;
    let margin = (26.0 * s).min(w * 0.05).max(8.0);
    // Main-menu footer is its own compositor surface, not part of Taffy.
    let bottom_safe = (70.0 * s).clamp(52.0, h * 0.16);
    let panel_w = (830.0 * s).min(w - margin * 2.0).max(1.0);
    let inner = (panel_w - pad * 2.0).max(1.0);
    let rows_count = loading.bars.iter().filter(|bar| !bar.skipped).count();
    let head_h = 76.0 * s;
    let foot_h = if loading.status_line.is_some() { 46.0 * s } else { 8.0 * s };
    let gap = 11.0 * s;
    let row_budget = (h - margin - bottom_safe - pad * 2.0 - head_h - foot_h - gap * 2.0).max(0.0);
    let two_columns = rows_count > 8 && w >= 680.0
        && row_budget / (rows_count as f32) < 26.0 * s;
    let rows_per_column = if two_columns { rows_count.div_ceil(2) } else { rows_count };
    let row_h = if rows_per_column == 0 { 0.0 } else {
        (29.0 * s).min(row_budget / rows_per_column as f32).max(0.0)
    };
    let rows_h = row_h * rows_per_column as f32;
    let panel_h = (pad * 2.0 + head_h + foot_h + rows_h + 2.0 * gap)
        .min((h - margin - bottom_safe).max(1.0));

    let mut f = Flex::new();
    // The top-right cancel is a Taffy child of the header, not a manually
    // positioned floating rectangle. Painting and hit testing share it.
    let kicker = f.leaf((inner * 0.54).max(1.0), 23.0 * s);
    let cancel_node = loading.joining_server.then(|| f.leaf(178.0 * s, 27.0 * s));
    let top = if let Some(cancel) = cancel_node {
        f.node(Container::row().width(inner).height(27.0 * s)
            .justify(JustifyContent::SPACE_BETWEEN), &[kicker, cancel])
    } else {
        f.node(Container::row().width(inner).height(27.0 * s), &[kicker])
    };
    let title = f.leaf((inner - 100.0 * s).max(1.0), 34.0 * s);
    let pct = f.leaf(92.0 * s, 34.0 * s);
    let title_row = f.node(Container::row().width(inner).height(34.0 * s)
        .justify(JustifyContent::SPACE_BETWEEN), &[title, pct]);
    let track = f.leaf(inner, 6.0 * s);
    let header = f.node(Container::column().width(inner).height(head_h).gap(4.0 * s),
        &[top, title_row, track]);

    let column_gap = if two_columns { 17.0 * s } else { 0.0 };
    let column_w = if two_columns { (inner - column_gap) * 0.5 } else { inner };
    let label_w = if two_columns { column_w * 0.34 } else { (inner * 0.31).min(230.0 * s) };
    let value_w = if two_columns { 57.0 * s } else { 76.0 * s };
    let rail_w = (column_w - label_w - value_w - 18.0 * s).max(1.0);
    let mut row_nodes = Vec::with_capacity(rows_count);
    let mut row_parts = Vec::with_capacity(rows_count);
    for _ in 0..rows_count {
        let label = f.leaf(label_w, row_h);
        let rail = f.leaf(rail_w, row_h);
        let value = f.leaf(value_w, row_h);
        let row = f.node(Container::row().width(column_w).height(row_h)
            .gap(9.0 * s).align(AlignItems::CENTER), &[label, rail, value]);
        row_nodes.push(row);
        row_parts.push([label, rail, value]);
    }
    let rows = if two_columns {
        let first = f.node(Container::column().width(column_w).height(rows_h), &row_nodes[..rows_per_column]);
        let second = f.node(Container::column().width(column_w).height(rows_h), &row_nodes[rows_per_column..]);
        f.node(Container::row().width(inner).height(rows_h).gap(column_gap), &[first, second])
    } else {
        f.node(Container::column().width(inner).height(rows_h), &row_nodes)
    };
    let status = f.leaf(inner, foot_h);
    let body = f.node(Container::column().width(inner).gap(gap), &[header, rows, status]);
    let panel = f.node(Container::column().width(panel_w).height(panel_h).pad(pad, pad), &[body]);
    let mut viewport = Container::column().width(w).height(h)
        .align(AlignItems::CENTER).justify(JustifyContent::FLEX_END);
    viewport.padding = [0.0, margin, 0.0, bottom_safe];
    let root = f.node(viewport, &[panel]);
    f.compute(root, w, h);
    let dialog = loading.connection_prompt.as_ref().map(|p| dialog_layout(p, w, h, s));
    LoadingLayout {
        panel: f.bounds(panel), kicker: f.bounds(kicker), heading: f.bounds(title),
        percentage: f.bounds(pct), overall_track: f.bounds(track),
        rows: row_parts.into_iter().map(|ids| [f.bounds(ids[0]), f.bounds(ids[1]), f.bounds(ids[2])]).collect(),
        status: f.bounds(status), cancel: cancel_node.map(|id| f.bounds(id)), dialog,
        text_scale: s,
    }
}

fn dialog_layout(prompt: &ConnectionPromptUi, w: f32, h: f32, s: f32) -> DialogLayout {
    let compact = h < 610.0;
    let dialog_w = (660.0 * s).min(w - 24.0).max(1.0);
    let pad = (22.0 * s).max(12.0);
    let inner = (dialog_w - 2.0 * pad).max(1.0);
    let field_h = if compact { 36.0 * s } else { 44.0 * s };
    let explanation_h = if compact { 40.0 * s } else { 45.0 * s };
    let help_h = if compact { 35.0 * s } else { 41.0 * s };
    let button_h = if compact { 38.0 * s } else { 43.0 * s };
    let secondary_h = 38.0 * s;
    let gap = if compact { 6.0 * s } else { 9.0 * s };
    let mut f = Flex::new();
    let kicker = f.leaf(inner, 20.0 * s);
    let title = f.leaf(inner, 35.0 * s);
    let subject = f.leaf(inner, field_h);
    let explanation = f.leaf(inner, explanation_h);
    let help = f.leaf(inner, help_h);
    let primary = f.leaf(inner, button_h);
    let secondary = f.leaf((inner - 10.0 * s) * 0.5, secondary_h);
    let tertiary = f.leaf((inner - 10.0 * s) * 0.5, secondary_h);
    let choice_row = match prompt {
        ConnectionPromptUi::MissingMap { .. } => f.node(Container::row().width(inner)
            .gap(10.0 * s), &[secondary, tertiary]),
        ConnectionPromptUi::Password { .. } => f.node(Container::row().width(inner)
            .gap(10.0 * s), &[secondary]),
    };
    let children = [kicker, title, subject, explanation, help, primary, choice_row];
    let content_h = 20.0*s + 35.0*s + field_h + explanation_h + help_h
        + button_h + secondary_h + 6.0*gap;
    let dialog_h = (content_h + 2.0*pad).min((h - 16.0).max(1.0));
    let stack = f.node(Container::column().width(inner).gap(gap), &children);
    let panel = f.node(Container::column().width(dialog_w).height(dialog_h)
        .pad(pad, pad), &[stack]);
    let root = f.node(Container::column().width(w).height(h)
        .align(AlignItems::CENTER).justify(JustifyContent::CENTER), &[panel]);
    f.compute(root, w, h);
    DialogLayout {
        panel: f.bounds(panel), kicker: f.bounds(kicker), title: f.bounds(title),
        subject: f.bounds(subject), explanation: f.bounds(explanation), help: f.bounds(help),
        primary: f.bounds(primary), secondary: f.bounds(secondary), tertiary: f.bounds(tertiary),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::MapLoadingBar;

    fn sample(rows: usize) -> MapLoadingUi {
        MapLoadingUi {
            map_name: "mp/ffa3".into(), active_game_dir: None, preparation_finished: false,
            bars: (0..rows).map(|_| MapLoadingBar { label: "TEST", completed: 0, total: 1,
                skipped: false, no_work: false }).collect(),
            joining_server: true, status_line: Some("Downloading a very long map name".into()),
            download_file_name: None, download_overall_fraction: None,
            connection_prompt: None, pointer: None,
        }
    }

    #[test]
    fn taffy_layout_reserves_footer_without_intersecting_rows() {
        let loading = sample(16);
        for (w,h) in [(1920,1080), (1280,720), (854,480)] {
            let l = layout(&loading, w,h);
            let last = l.rows.iter().map(|row| row[0])
                .max_by(|a,b| (a[1]+a[3]).total_cmp(&(b[1]+b[3]))).unwrap();
            assert!(last[1] + last[3] <= l.status[1] + 1.0, "{w}x{h}");
            if h == 480 { assert!(l.rows[8][0][0] > l.rows[0][0][0]); }
            assert!(l.status[1] + l.status[3] <= h as f32 - (70.0*l.text_scale).clamp(52.0,h as f32*0.16) + 1.0);
            assert!(l.panel[0] >= 0.0 && l.panel[1] >= 0.0);
            let cancel = l.cancel.unwrap();
            assert!(cancel[0] >= l.panel[0] && cancel[0] + cancel[2] <= l.panel[0] + l.panel[2]);
        }
    }

    #[test]
    fn modal_hitboxes_match_taffy_buttons_and_disabled_download() {
        let mut loading = sample(0);
        loading.connection_prompt = Some(ConnectionPromptUi::MissingMap {
            map_name: "mp/test".into(), reason: "not found".into(), can_download: true,
        });
        let center = |r: Bounds| [r[0] + r[2] * 0.5, r[1] + r[3] * 0.5];
        let g = layout(&loading, 1280, 720);
        let d = g.dialog.as_ref().unwrap();
        assert_eq!(g.hit_test(center(d.primary), loading.connection_prompt.as_ref()), Some(ConnectionAction::Download));
        assert_eq!(g.hit_test(center(d.secondary), loading.connection_prompt.as_ref()), Some(ConnectionAction::ConnectAnyway));
        assert_eq!(g.hit_test(center(d.tertiary), loading.connection_prompt.as_ref()), Some(ConnectionAction::Abort));
        loading.connection_prompt = Some(ConnectionPromptUi::MissingMap {
            map_name: "mp/test".into(), reason: "not found".into(), can_download: false,
        });
        let g = layout(&loading, 1280, 720);
        assert_eq!(g.hit_test(center(g.dialog.as_ref().unwrap().primary), loading.connection_prompt.as_ref()), None);
    }
}
