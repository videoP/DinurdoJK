//! Compact native WGPU update prompt. Taffy owns both visual geometry and hitboxes.
//! Update signatures, hashes and staging are handled by the updater, not this UI.
use super::{flex::{self, Bounds, Container, Flex}, rect, rect_gradient, UiSnapshot, UiVertex, ProportionalFont};
use super::text::UiText;
use taffy::prelude::JustifyContent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action { Download, Apply, Dismiss }

#[derive(Debug, Clone)]
pub(crate) struct UpdateNoticeUi {
    pub version: String,
    pub downloading: bool,
    pub applying: bool,
    pub staged: bool,
    pub received: u64,
    pub total: u64,
    pub error: Option<String>,
    pub(crate) hovered: Option<Action>,
}

#[derive(Debug, Clone)]
pub(crate) struct NoticeLayout {
    pub panel: Bounds,
    pub heading: Bounds,
    pub version: Bounds,
    pub status: Option<Bounds>,
    pub rail: Option<Bounds>,
    pub primary: Bounds,
    pub secondary: Bounds,
    pub error: Option<Bounds>,
    pub scale: f32,
}

impl NoticeLayout {
    pub(crate) fn contains(&self, p: [f32; 2]) -> bool { flex::contains(self.panel, p) }

    pub(crate) fn hit_test(&self, p: [f32; 2], data: &UpdateNoticeUi) -> Option<Action> {
        if flex::contains(self.secondary, p) { return Some(Action::Dismiss); }
        if flex::contains(self.primary, p) && !data.downloading && !data.applying {
            return Some(if data.staged { Action::Apply } else { Action::Download });
        }
        None
    }
}

pub(crate) fn layout(data: &UpdateNoticeUi, width: u32, height: u32) -> NoticeLayout {
    let w = width.max(1) as f32;
    let h = height.max(1) as f32;
    // Use exactly the same Taffy rectangles for hit-testing and drawing.
    let scale = ((h / 900.0).min(w / 1560.0) * 1.14).clamp(0.72, 1.28);
    let margin = (25.0 * scale).min((w * 0.04).max(6.0));
    let panel_w = (430.0 * scale).min((w - 2.0 * margin).max(1.0));
    let pad = (22.0 * scale).min(panel_w * 0.06);
    let inner = (panel_w - 2.0 * pad).max(1.0);
    let gap = 10.0 * scale;
    let heading_h = 33.0 * scale;
    let version_h = 24.0 * scale;
    let status_h = 20.0 * scale;
    let rail_h = 7.0 * scale;
    let buttons_h = 40.0 * scale;
    let error_h = 23.0 * scale;
    // Neither the status nor the progress rail should occupy space while idle.
    let show_status = data.downloading || data.applying;
    let show_rail = data.downloading;
    let show_error = data.error.is_some();
    let children = 3 + usize::from(show_status) + usize::from(show_rail) + usize::from(show_error);
    let panel_h = 2.0 * pad + heading_h + version_h + buttons_h
        + (if show_status { status_h } else { 0.0 })
        + (if show_rail { rail_h } else { 0.0 })
        + (if show_error { error_h } else { 0.0 })
        + (children - 1) as f32 * gap;
    let x = (w - margin - panel_w).max(0.0);
    let y = (86.0 * scale).min((h - panel_h - (75.0 * scale)).max(margin));

    let mut f = Flex::new();
    let heading = f.leaf(inner, heading_h);
    let version = f.leaf(inner, version_h);
    let status = show_status.then(|| f.leaf(inner, status_h));
    let rail = show_rail.then(|| f.leaf(inner, rail_h));
    let primary = f.leaf((inner * 0.62).max(1.0), buttons_h);
    let secondary = f.leaf((inner * 0.33).max(1.0), buttons_h);
    let buttons = f.node(Container::row().width(inner).height(buttons_h)
        .justify(JustifyContent::SPACE_BETWEEN), &[primary, secondary]);
    let error = show_error.then(|| f.leaf(inner, error_h));
    let mut items = vec![heading, version];
    if let Some(node) = status { items.push(node); }
    if let Some(node) = rail { items.push(node); }
    items.push(buttons);
    if let Some(node) = error { items.push(node); }
    let panel = f.node(Container::column().width(panel_w).height(panel_h)
        .pad(pad, pad).gap(gap), &items);
    f.compute(panel, panel_w, panel_h);
    let shift = |mut rect: Bounds| { rect[0] += x; rect[1] += y; rect };
    NoticeLayout {
        panel: shift(f.bounds(panel)), heading: shift(f.bounds(heading)), version: shift(f.bounds(version)),
        status: status.map(|id| shift(f.bounds(id))), rail: rail.map(|id| shift(f.bounds(id))),
        primary: shift(f.bounds(primary)), secondary: shift(f.bounds(secondary)),
        error: error.map(|id| shift(f.bounds(id))), scale,
    }
}

fn fit(txt: &UiText<'_>, value: &str, px: f32, available: f32) -> String {
    if txt.width(value, px) <= available { return value.to_owned(); }
    let mut out = String::new();
    let limit = (available - txt.width("...", px)).max(0.0);
    for ch in value.chars() {
        let mut candidate = out.clone();
        candidate.push(ch);
        if txt.width(&candidate, px) > limit { break; }
        out.push(ch);
    }
    out.push_str("...");
    out
}

fn draw_line(txt: &UiText<'_>, out: &mut Vec<UiVertex>, value: &str, box_: Bounds, px: f32, tint: [f32; 4]) {
    let line = fit(txt, value, px, box_[2].max(1.0));
    txt.draw(out, &line, box_[0], txt.baseline_in(box_, px), px, tint);
}

pub(in crate::ui) fn build(out: &mut Vec<UiVertex>, ui: &UiSnapshot,
    data: &UpdateNoticeUi, small_font: Option<&ProportionalFont>, w: u32, h: u32)
{
    let l = layout(data, w, h);
    let s = l.scale;
    let b = l.panel;
    let cyan = [0.28, 0.91, 1.0, 1.0];
    let white = [0.89, 0.93, 0.98, 1.0];
    let muted = [0.62, 0.71, 0.81, 1.0];
    let ink = [0.055, 0.072, 0.105, 0.95];
    rect(out, b[0] - 2.0*s, b[1] - 2.0*s, b[2] + 4.0*s, b[3] + 4.0*s,
        [0.01, 0.02, 0.04, 0.36], w, h);
    rect_gradient(out, b[0], b[1], b[2], b[3], ink, ink,
        [0.025, 0.043, 0.067, 0.97], [0.025, 0.043, 0.067, 0.97], w, h);
    rect_gradient(out, b[0], b[1], b[2], 2.0*s,
        cyan, [0.68, 0.48, 1.0, 0.9], cyan, [0.68, 0.48, 1.0, 0.9], w, h);
    rect(out, b[0], b[1] + b[3] - 1.0*s, b[2], 1.0*s,
        [0.30, 0.47, 0.60, 0.38], w, h);

    let txt = UiText::new(ui, small_font, w, h);
    draw_line(&txt, out, "DinurdoJK Update", l.heading, 26.0*s, white);
    draw_line(&txt, out, &format!("Version {} available", data.version), l.version, 17.0*s, muted);

    if let Some(bounds) = l.status {
        let status = if data.applying {
            "Restarting...".to_owned()
        } else if data.total == 0 {
            "Preparing download...".to_owned()
        } else {
            let mb = |bytes: u64| bytes as f64 / 1_048_576.0;
            format!("Downloading  {:.1} / {:.1} MB", mb(data.received), mb(data.total))
        };
        draw_line(&txt, out, &status, bounds, 15.0*s, muted);
    }
    if let Some(rail) = l.rail {
        let fraction = if data.total == 0 { 0.0 }
            else { (data.received as f32 / data.total as f32).clamp(0.0, 1.0) };
        super::loading::progress_track(out, rail[0], rail[1], rail[2], rail[3],
            fraction, [0.13, 0.60, 0.77, 0.9], cyan, w, h);
    }

    let action = if data.staged { Action::Apply } else { Action::Download };
    for (bounds, button, label, enabled) in [
        (l.primary, action, if data.applying { "Restarting..." }
            else if data.staged { "Restart & Update" }
            else if data.downloading { "Downloading..." }
            else { "Download" }, !data.downloading && !data.applying),
        (l.secondary, Action::Dismiss, "Later", true),
    ] {
        let highlight = enabled && data.hovered == Some(button);
        let fill = if !enabled { [0.10, 0.13, 0.16, 0.85] }
            else if button == Action::Dismiss { if highlight { [0.22, 0.26, 0.32, 0.98] } else { [0.13, 0.17, 0.22, 0.96] } }
            else if highlight { [0.11, 0.39, 0.49, 0.99] } else { [0.08, 0.25, 0.33, 0.97] };
        rect(out, bounds[0], bounds[1], bounds[2], bounds[3], fill, w, h);
        rect(out, bounds[0], bounds[1], bounds[2], 1.3*s,
            if highlight { [0.67, 0.95, 1.0, 1.0] } else { [0.36, 0.62, 0.70, 0.72] }, w, h);
        let px = 15.5 * s;
        let label_width = txt.width(label, px);
        txt.draw(out, label, bounds[0] + (bounds[2] - label_width).max(0.0)*0.5,
            txt.baseline_in(bounds, px), px, if enabled { white } else { muted });
    }
    if let Some(bounds) = l.error {
        // Keep technical diagnostics in the console, not in this player-facing prompt.
        let message = if data.staged { "Couldn't restart. Try again." }
            else { "Download failed. Try again." };
        draw_line(&txt, out, message, bounds, 14.5*s, [1.0, 0.74, 0.69, 1.0]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn notice() -> UpdateNoticeUi {
        UpdateNoticeUi {version:"v0.1.76".into(),downloading:false,
            applying:false,staged:false,received:0,total:0,error:None,hovered:None}
    }
    #[test]
    fn button_hits_and_paint_use_identical_taffy_layout() {
        for (w, h) in [(1650, 900), (1280, 720), (800, 600)] {
            let ui=notice(); let layout=layout(&ui,w,h);
            assert!(layout.panel[0]>=0.0 && layout.panel[1]>=0.0);
            assert!(layout.panel[0]+layout.panel[2]<=w as f32+0.5);
            assert!(layout.panel[1]+layout.panel[3]<=h as f32+0.5);
            assert!(layout.status.is_none() && layout.rail.is_none());
            let p=[layout.primary[0]+layout.primary[2]*0.5,layout.primary[1]+layout.primary[3]*0.5];
            assert_eq!(layout.hit_test(p,&ui),Some(Action::Download));
            let s=[layout.secondary[0]+layout.secondary[2]*0.5,layout.secondary[1]+layout.secondary[3]*0.5];
            assert_eq!(layout.hit_test(s,&ui),Some(Action::Dismiss));
            assert!(!flex::contains(layout.primary,s));
        }
    }
    #[test]
    fn staged_notice_changes_primary_action_without_changing_geometry() {
        let mut ui=notice(); let before=layout(&ui,1600,900);
        ui.staged=true;let after=layout(&ui,1600,900);
        assert_eq!(before.primary,after.primary);
        let p=[after.primary[0]+4.0,after.primary[1]+4.0];
        assert_eq!(after.hit_test(p,&ui),Some(Action::Apply));
        ui.applying=true;
        assert_eq!(after.hit_test(p,&ui),None);
    }
    #[test]
    fn progress_and_error_only_take_space_when_needed() {
        let mut ui = notice();
        let idle = layout(&ui, 1600, 900);
        ui.downloading = true;
        let downloading = layout(&ui, 1600, 900);
        assert!(downloading.panel[3] > idle.panel[3]);
        assert!(downloading.status.is_some() && downloading.rail.is_some());
        let p = [downloading.primary[0] + 3.0, downloading.primary[1] + 3.0];
        assert_eq!(downloading.hit_test(p, &ui), None);
        ui.downloading = false;
        ui.error = Some("full technical error".into());
        let failed = layout(&ui, 1600, 900);
        assert!(failed.error.is_some());
        assert!(failed.rail.is_none());
        assert!(failed.panel[3] > idle.panel[3]);
    }
}
