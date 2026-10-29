//! Taffy owns Render Studio's panel layout. egui only renders and handles
//! widgets inside these rectangles; the WGPU preview uses the exact same rect.
//!
//! Every pane is a Taffy column (header / body / footer), so captions, tool
//! strips and transport bars share one grid instead of hand-tuned offsets.
use super::render_studio_ui as sui;
use taffy::prelude::{
    auto, length, AvailableSpace, FlexDirection, NodeId, Size, Style, TaffyTree,
};

pub(super) type Bounds = [f32; 4];

#[derive(Clone, Copy, Default)]
pub(super) struct StudioPanels {
    pub library: Bounds,
    pub library_header: Bounds,
    pub library_body: Bounds,
    pub library_footer: Bounds,
    pub preview: Bounds,
    pub preview_header: Bounds,
    /// The 3D viewport proper: the renderer composites into exactly this.
    pub viewport: Bounds,
    pub transport: Bounds,
    pub inspector: Bounds,
    pub inspector_header: Bounds,
    pub inspector_body: Bounds,
    pub timeline: Bounds,
    pub timeline_header: Bounds,
    pub ruler: Bounds,
    pub tracks: Bounds,
    pub scrollbar: Bounds,
}

struct Tree {
    tree: TaffyTree<()>,
}

impl Tree {
    fn leaf(&mut self, width: Option<f32>, height: Option<f32>, grow: bool) -> Option<NodeId> {
        self.tree.new_leaf(Style {
            size: Size {
                width: width.map_or(auto(), length),
                height: height.map_or(auto(), length),
            },
            flex_grow: if grow { 1.0 } else { 0.0 },
            flex_shrink: if grow { 1.0 } else { 0.0 },
            min_size: Size { width: length(0.0), height: length(0.0) },
            ..Default::default()
        }).ok()
    }
    /// Fixed-size header/footer rows around one growing body.
    fn column(&mut self, width: Option<f32>, height: Option<f32>, grow: bool,
        children: &[NodeId]) -> Option<NodeId> {
        self.tree.new_with_children(Style {
            flex_direction: FlexDirection::Column,
            size: Size {
                width: width.map_or(auto(), length),
                height: height.map_or(auto(), length),
            },
            flex_grow: if grow { 1.0 } else { 0.0 },
            flex_shrink: if grow { 1.0 } else { 0.0 },
            min_size: Size { width: length(0.0), height: length(0.0) },
            ..Default::default()
        }, children).ok()
    }
    fn rect(&self, node: NodeId, offset: [f32; 2]) -> Option<Bounds> {
        let l = self.tree.layout(node).ok()?;
        let x = offset[0] + l.location.x;
        let y = offset[1] + l.location.y;
        Some([x, y, x + l.size.width, y + l.size.height])
    }
}

/// Taffy flex layout (library/viewer/inspector row over the timeline dock).
/// Coordinates are in egui logical points, not raw window pixels.
pub(super) fn compute(w: f32, h: f32, tracks: usize,
    preferences: &crate::app::render_studio::StudioPanelState) -> Option<StudioPanels> {
    let (w, h) = (w.max(400.0), h.max(300.0));
    let gap = sui::PANE_GAP;
    // Header, ruler and scrollbar take ~80 pt; each track row is LANE_H. Cap
    // the dock to preserve the viewport; larger projects scroll in the dock.
    let chrome = sui::TIMELINE_HEADER_H + sui::RULER_H + sui::SCROLLBAR_H + 6.0;
    let timeline_need = tracks as f32 * sui::LANE_H + chrome;
    let default_h = timeline_need.clamp(184.0, (h * 0.38).max(184.0));
    let timeline_h = preferences.timeline_height.unwrap_or(default_h)
        .clamp(130.0, (h - 170.0).max(130.0));
    let library_default = (w * 0.17).clamp(200.0, 340.0);
    let right_default = if w >= 960.0 { (w * 0.20).clamp(260.0, 400.0) } else { 0.0 };
    let left_w = if preferences.library_collapsed { sui::COLLAPSED_W } else {
        preferences.library_width.unwrap_or(library_default).clamp(120.0, (w * 0.55).max(120.0)) };
    let right_w = if preferences.inspector_collapsed { sui::COLLAPSED_W } else {
        preferences.inspector_width.unwrap_or(right_default).clamp(0.0, (w - left_w - 260.0).max(0.0)) };

    let mut t = Tree { tree: TaffyTree::new() };
    let (lh, lb, lf) = (t.leaf(None, Some(sui::PANE_HEADER_H), false)?, t.leaf(None, None, true)?,
        t.leaf(None, Some(sui::PANE_FOOTER_H), false)?);
    let library = t.column(Some(left_w), None, false, &[lh, lb, lf])?;
    let (ph, pv, pt) = (t.leaf(None, Some(sui::PANE_HEADER_H), false)?, t.leaf(None, None, true)?,
        t.leaf(None, Some(sui::TRANSPORT_H), false)?);
    let preview = t.column(None, None, true, &[ph, pv, pt])?;
    let inspector_nodes = if right_w > 0.0 {
        let (ih, ib) = (t.leaf(None, Some(sui::PANE_HEADER_H), false)?, t.leaf(None, None, true)?);
        Some((t.column(Some(right_w), None, false, &[ih, ib])?, ih, ib))
    } else { None };
    let (th, tr, tt, ts) = (t.leaf(None, Some(sui::TIMELINE_HEADER_H), false)?,
        t.leaf(None, Some(sui::RULER_H), false)?, t.leaf(None, None, true)?,
        t.leaf(None, Some(sui::SCROLLBAR_H), false)?);
    let timeline = t.column(Some(w), Some(timeline_h), false, &[th, tr, tt, ts])?;

    let mut row_children = vec![library, preview];
    if let Some((inspector, _, _)) = inspector_nodes { row_children.push(inspector); }
    let body = t.tree.new_with_children(Style {
        flex_direction: FlexDirection::Row,
        flex_grow: 1.0,
        size: Size { width: auto(), height: auto() },
        min_size: Size { width: length(0.0), height: length(0.0) },
        gap: Size { width: length(gap), height: length(0.0) },
        ..Default::default()
    }, &row_children).ok()?;
    let root = t.tree.new_with_children(Style {
        flex_direction: FlexDirection::Column,
        size: Size { width: length(w), height: length(h) },
        gap: Size { width: length(0.0), height: length(gap) },
        ..Default::default()
    }, &[body, timeline]).ok()?;
    t.tree.compute_layout(root, Size {
        width: AvailableSpace::Definite(w), height: AvailableSpace::Definite(h)
    }).ok()?;

    let body_rect = t.rect(body, [0.0, 0.0])?;
    let within = |t: &Tree, parent: Bounds, node: NodeId| t.rect(node, [parent[0], parent[1]]);
    let library_rect = t.rect(library, [body_rect[0], body_rect[1]])?;
    let preview_rect = t.rect(preview, [body_rect[0], body_rect[1]])?;
    let timeline_rect = t.rect(timeline, [0.0, 0.0])?;
    let mut panels = StudioPanels {
        library: library_rect,
        library_header: within(&t, library_rect, lh)?,
        library_body: within(&t, library_rect, lb)?,
        library_footer: within(&t, library_rect, lf)?,
        preview: preview_rect,
        preview_header: within(&t, preview_rect, ph)?,
        viewport: within(&t, preview_rect, pv)?,
        transport: within(&t, preview_rect, pt)?,
        timeline: timeline_rect,
        timeline_header: within(&t, timeline_rect, th)?,
        ruler: within(&t, timeline_rect, tr)?,
        tracks: within(&t, timeline_rect, tt)?,
        scrollbar: within(&t, timeline_rect, ts)?,
        ..Default::default()
    };
    if let Some((inspector, ih, ib)) = inspector_nodes {
        let rect = t.rect(inspector, [body_rect[0], body_rect[1]])?;
        panels.inspector = rect;
        panels.inspector_header = within(&t, rect, ih)?;
        panels.inspector_body = within(&t, rect, ib)?;
    }
    Some(panels)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn height(b: Bounds) -> f32 { b[3] - b[1] }
    fn width(b: Bounds) -> f32 { b[2] - b[0] }
    #[test]
    fn timeline_grows_for_more_layers_but_stays_bounded() {
        let a=compute(1920.,1080.,5,&Default::default()).unwrap();
        let b=compute(1920.,1080.,11,&Default::default()).unwrap();
        assert!(height(b.timeline)>height(a.timeline));
        assert!(height(b.timeline)<=1080.0*0.38+0.1);
    }
    #[test]
    fn user_resized_and_collapsed_panels_stay_inside_editor() {
        let p=crate::app::render_studio::StudioPanelState {library_width:Some(450.0),
            inspector_width:Some(400.0),timeline_height:Some(390.0),..Default::default()};
        let bounds=compute(1600.,900.,5,&p).unwrap();
        assert!((width(bounds.library)-450.0).abs()<0.1);
        assert!((height(bounds.timeline)-390.0).abs()<0.1);
        let collapsed=crate::app::render_studio::StudioPanelState {library_collapsed:true,
            inspector_collapsed:true,..Default::default()};
        let bounds=compute(1600.,900.,5,&collapsed).unwrap();
        assert!((width(bounds.library)-sui::COLLAPSED_W).abs()<0.1);
        assert!((width(bounds.inspector)-sui::COLLAPSED_W).abs()<0.1);
    }
    #[test]
    fn taffy_panes_never_overlap() {
        for (w, h) in [(1024.,700.),(1280.,720.),(1920.,1080.),(2560.,1440.),(750.,600.)] {
            let p = compute(w, h, 5, &Default::default()).unwrap();
            assert!(p.library[2] < p.preview[0]);
            assert!(p.preview[3] < p.timeline[1]);
            if p.inspector[2] > p.inspector[0] {
                assert!(p.preview[2] < p.inspector[0]);
            }
            assert!(p.timeline[3] <= h + 0.1);
        }
    }
    #[test]
    fn pane_parts_stack_inside_their_pane() {
        let p = compute(2000., 1125., 5, &Default::default()).unwrap();
        // Viewer: header, viewport and transport tile the pane exactly.
        assert!((p.preview_header[1] - p.preview[1]).abs() < 0.1);
        assert!((p.viewport[1] - p.preview_header[3]).abs() < 0.1);
        assert!((p.transport[1] - p.viewport[3]).abs() < 0.1);
        assert!((p.transport[3] - p.preview[3]).abs() < 0.1);
        assert!((height(p.transport) - sui::TRANSPORT_H).abs() < 0.1);
        // Timeline: header / ruler / tracks / scrollbar share the dock width.
        assert!((p.ruler[1] - p.timeline_header[3]).abs() < 0.1);
        assert!((p.tracks[1] - p.ruler[3]).abs() < 0.1);
        assert!((p.scrollbar[3] - p.timeline[3]).abs() < 0.1);
        assert!(height(p.tracks) > 60.0);
        assert!((p.library_footer[3] - p.library[3]).abs() < 0.1);
        assert!((p.inspector_body[3] - p.inspector[3]).abs() < 0.1);
    }
}
