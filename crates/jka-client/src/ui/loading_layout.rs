//! Taffy owns the layout and hitboxes for the server join / map loading UI.
//! The native HUD painter consumes these rectangles; no egui widgets or
//! independently computed mouse targets are used for this screen.
use super::{flex::{Bounds, Container, Flex}, MapLoadingPhase, MapLoadingUi};
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
pub(crate) struct PhaseLayout {
    pub heading: Bounds,
    pub status: Bounds,
    /// One real task rail per visible task in this phase, in original order.
    pub bars: Vec<Bounds>,
}

#[derive(Clone, Debug)]
pub(crate) struct LoadingLayout {
    pub panel: Bounds,
    pub kicker: Bounds,
    pub heading: Bounds,
    pub percentage: Bounds,
    pub overall_track: Bounds,
    /// Four fixed dependency columns. Individual tasks remain parallel.
    pub phases: Vec<PhaseLayout>,
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
    let s = (h / 900.0).min(w / 1250.0).clamp(0.78, 1.15);
    let pad = 22.0 * s;
    let margin = (26.0 * s).min(w * 0.05).max(8.0);
    let bottom_safe = (70.0 * s).max(52.0).min(h * 0.16);
    let panel_w = (830.0 * s).min(w - margin * 2.0).max(1.0);
    let inner = (panel_w - pad * 2.0).max(1.0);
    let head_h = 76.0 * s;
    let gap = 11.0 * s;
    let phase_groups = MapLoadingPhase::ALL.map(|phase| loading.bars.iter()
        .filter(|b| !b.skipped && b.phase == phase).collect::<Vec<_>>());
    let max_bars = phase_groups.iter().map(Vec::len).max().unwrap_or(0).max(1);
    // Size rows for readable labels, not just thin progress ticks. All four
    // columns share this height, so their unequal stacks stay centred.
    let target_bar_h = 21.0 * s;
    let target_bar_gap = 5.0 * s;
    let label_h = 20.0 * s;
    let state_h = 18.0 * s;
    let available_stack_h = (h - margin - bottom_safe - pad * 2.0 - head_h
        - gap - label_h - state_h).max(0.0);
    let natural_stack_h = (145.0 * s).max(target_bar_h * max_bars as f32
        + target_bar_gap * max_bars.saturating_sub(1) as f32);
    let stack_h = natural_stack_h.min(available_stack_h);
    let bar_gap = target_bar_gap.min(stack_h /
        (max_bars as f32 * 3.0).max(1.0));
    let bar_h = target_bar_h.min(((stack_h - bar_gap * (max_bars - 1) as f32)
        / max_bars as f32).max(0.0));
    let phases_h = stack_h + label_h + state_h;
    // The active task is now named inside its own bar; no redundant footer.
    let panel_h = (pad * 2.0 + head_h + phases_h + gap)
        .min((h - margin - bottom_safe).max(1.0));

    let mut f = Flex::new();
    let kicker = f.leaf((inner * 0.54).max(1.0), 23.0 * s);
    let cancel_node = loading.joining_server.then(|| f.leaf(178.0 * s, 27.0 * s));
    let top = if let Some(cancel) = cancel_node {
        f.node(Container::row().width(inner).height(27.0 * s)
            .justify(JustifyContent::SPACE_BETWEEN), &[kicker, cancel])
    } else {
        f.node(Container::row().width(inner).height(27.0 * s), &[kicker])
    };
    let title = f.leaf((inner - 128.0 * s).max(1.0), 34.0 * s);
    let pct = f.leaf(120.0 * s, 34.0 * s);
    let title_row = f.node(Container::row().width(inner).height(34.0 * s)
        .justify(JustifyContent::SPACE_BETWEEN), &[title, pct]);
    let track = f.leaf(inner, 6.0 * s);
    let header = f.node(Container::column().width(inner).height(head_h).gap(4.0 * s),
        &[top, title_row, track]);

    let col_gap = 12.0 * s;
    let col_w = ((inner - 3.0 * col_gap) / 4.0).max(1.0);
    let mut column_nodes = Vec::with_capacity(4);
    let mut part_nodes = Vec::with_capacity(4);
    for group in &phase_groups {
        let bar_nodes: Vec<_> = group.iter().map(|_| f.leaf((col_w - 4.0 * s).max(1.0), bar_h))
            .collect();
        let stack = f.node(Container::column().width(col_w).height(stack_h).gap(bar_gap)
            .align(AlignItems::CENTER).justify(JustifyContent::CENTER), &bar_nodes);
        let caption = f.leaf(col_w, label_h);
        let state = f.leaf(col_w, state_h);
        column_nodes.push(f.node(Container::column().width(col_w).height(phases_h),
            &[stack, caption, state]));
        part_nodes.push((bar_nodes, caption, state));
    }
    let phase_row = f.node(Container::row().width(inner).height(phases_h).gap(col_gap),
        &column_nodes);
    let body = f.node(Container::column().width(inner).gap(gap), &[header, phase_row]);
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
        phases: part_nodes.into_iter().map(|(bars, heading, state)| PhaseLayout {
            heading: f.bounds(heading), status: f.bounds(state),
            bars: bars.into_iter().map(|id| f.bounds(id)).collect(),
        }).collect(),
        cancel: cancel_node.map(|id| f.bounds(id)), dialog,
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
            bars: (0..rows).map(|i| MapLoadingBar {
                phase: MapLoadingPhase::ALL[i % 4], label: "TEST", completed: 0, total: 1,
                skipped: false, no_work: false }).collect(),
            joining_server: true, status_line: Some("Downloading a very long map name".into()),
            download_file_name: None, download_overall_fraction: None,
            connection_prompt: None, pointer: None,
        }
    }

    #[test]
    fn taffy_layout_centers_variable_parallel_bars_in_four_stage_columns() {
        let loading = sample(16);
        for (w,h) in [(1920,1080), (1280,720), (854,480)] {
            let l = layout(&loading,w,h);
            assert_eq!(l.phases.len(), 4);
            for stage in &l.phases {
                assert_eq!(stage.bars.len(), 4);
                assert!(stage.bars.iter().all(|bar|
                    bar[1] >= l.panel[1] && bar[1] + bar[3] <= stage.heading[1] + 1.0));
                assert!(stage.status[1] + stage.status[3] <= l.panel[1] + l.panel[3] + 1.0);
                assert!(stage.bars.iter().all(|bar| bar[3] >= 16.0 * l.text_scale));
            }
            assert!(l.phases.windows(2).all(|adjacent|
                adjacent[0].heading[0] < adjacent[1].heading[0]));
            assert!(l.panel[1] + l.panel[3] <= h as f32 -
                (70.0*l.text_scale).max(52.0).min(h as f32*0.16) + 1.0);
            let cancel = l.cancel.unwrap();
            assert!(cancel[0] >= l.panel[0]
                && cancel[0] + cancel[2] <= l.panel[0] + l.panel[2]);
        }
    }

    #[test]
    fn skipped_tasks_remove_only_their_subbar_not_the_stage() {
        let mut loading = sample(8);
        loading.bars[0].skipped = true;
        let l = layout(&loading, 1280, 720);
        assert_eq!(l.phases.len(), 4);
        assert_eq!(l.phases[0].bars.len(), 1);
        assert!(l.phases.iter().skip(1).all(|p| p.bars.len()==2));
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
