//! Taffy owns Render Studio's panel layout. egui only renders and handles
//! widgets inside these rectangles; the WGPU preview uses the exact same rect.
use taffy::prelude::{
    auto, length, AvailableSpace, FlexDirection, NodeId, Size, Style, TaffyTree,
};

#[derive(Clone, Copy)]
pub(super) struct StudioPanels {
    pub header: [f32; 4],
    pub library: [f32; 4],
    pub preview: [f32; 4],
    pub inspector: [f32; 4],
    pub timeline: [f32; 4],
}

/// Taffy flex layout (column shell, row of panes, fixed-height timeline).
/// Coordinates are in egui logical points, not raw window pixels.
pub(super) fn compute(w: f32, h: f32) -> Option<StudioPanels> {
    let (w, h) = (w.max(400.0), h.max(300.0));
    let gap = 8.0;
    let header_h = 45.0;
    let timeline_h = (h * 0.35).clamp(150.0, 320.0).min(h * 0.5);
    let left_w = (w * 0.23).clamp(190.0, 310.0).min(w * 0.39);
    let right_w = if w >= 1000.0 { (w * 0.23).clamp(210.0, 320.0) } else { 0.0 };
    let mut tree = TaffyTree::<()>::new();
    let fixed = |width: f32, height: f32| Style {
        size: Size { width: length(width), height: length(height) },
        flex_shrink: 0.0, ..Default::default()
    };
    let header = tree.new_leaf(fixed(w, header_h)).ok()?;
    let timeline = tree.new_leaf(fixed(w, timeline_h)).ok()?;
    let library = tree.new_leaf(Style {
        size: Size { width: length(left_w), height: auto() },
        flex_shrink: 0.0, ..Default::default()
    }).ok()?;
    let preview = tree.new_leaf(Style {
        size: Size { width: auto(), height: auto() },
        flex_grow: 1.0, flex_shrink: 1.0, ..Default::default()
    }).ok()?;
    let inspector = if right_w > 0.0 {
        Some(tree.new_leaf(Style {
            size: Size { width: length(right_w), height: auto() },
            flex_shrink: 0.0, ..Default::default()
        }).ok()?)
    } else { None };
    let mut row_children = vec![library, preview];
    if let Some(inspector) = inspector { row_children.push(inspector); }
    let body = tree.new_with_children(Style {
        flex_direction: FlexDirection::Row,
        flex_grow: 1.0,
        size: Size { width: auto(), height: auto() },
        gap: Size { width: length(gap), height: length(0.0) },
        ..Default::default()
    }, &row_children).ok()?;
    let root = tree.new_with_children(Style {
        flex_direction: FlexDirection::Column,
        size: Size { width: length(w), height: length(h) },
        gap: Size { width: length(0.0), height: length(gap) },
        ..Default::default()
    }, &[header, body, timeline]).ok()?;
    tree.compute_layout(root, Size {
        width: AvailableSpace::Definite(w), height: AvailableSpace::Definite(h)
    }).ok()?;
    let rect = |node: NodeId, offset: [f32; 2]| -> Option<[f32; 4]> {
        let l = tree.layout(node).ok()?;
        let x = offset[0] + l.location.x;
        let y = offset[1] + l.location.y;
        Some([x, y, x + l.size.width, y + l.size.height])
    };
    let body_rect = rect(body, [0.0, 0.0])?;
    let offset = [body_rect[0], body_rect[1]];
    Some(StudioPanels {
        header: rect(header, [0.0, 0.0])?,
        library: rect(library, offset)?,
        preview: rect(preview, offset)?,
        inspector: inspector.map_or(Some([0.0; 4]), |node| rect(node, offset))?,
        timeline: rect(timeline, [0.0, 0.0])?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn taffy_panes_never_overlap() {
        for (w, h) in [(1024.,700.),(1280.,720.),(1920.,1080.),(2560.,1440.),(750.,600.)] {
            let p = compute(w, h).unwrap();
            assert!(p.library[2] < p.preview[0]);
            assert!(p.preview[3] < p.timeline[1]);
            if p.inspector[2] > p.inspector[0] {
                assert!(p.preview[2] < p.inspector[0]);
            }
            assert!(p.timeline[3] <= h + 0.1);
        }
    }
}
