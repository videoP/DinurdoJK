//! The Map Mods dialog's regions are laid out by Taffy. egui paints and handles
//! input inside these rectangles, but never determines the pane geometry.
use crate::ui::flex::{Container, Flex};
use egui::{pos2, vec2, Rect};
use taffy::prelude::AlignItems;

#[derive(Clone, Copy)]
pub(super) struct Panels {
    pub header: Rect,
    pub files: Rect,
    pub editor: Rect,
    pub footer: Rect,
}

fn as_rect(parent: Rect, bounds: [f32; 4]) -> Rect {
    Rect::from_min_size(pos2(parent.left() + bounds[0], parent.top() + bounds[1]),
        vec2(bounds[2], bounds[3]))
}

pub(super) fn compute(rect: Rect) -> Panels {
    let w = rect.width().max(1.0);
    let h = rect.height().max(1.0);
    let mut flex = Flex::new();
    let header = flex.band(68.0);
    let files = flex.node(Container::column().width((w * 0.36).clamp(270.0, 375.0)), &[]);
    let editor = flex.spring();
    let body = flex.node(Container::row().gap(12.0).align(AlignItems::STRETCH), &[files, editor]);
    flex.set_grow(body, 1.0);
    let footer = flex.band(79.0);
    let root = flex.node(Container::column().width(w).height(h).gap(11.0), &[header, body, footer]);
    flex.compute(root, w, h);
    Panels {
        header: as_rect(rect, flex.bounds(header)),
        files: as_rect(rect, flex.bounds(files)),
        editor: as_rect(rect, flex.bounds(editor)),
        footer: as_rect(rect, flex.bounds(footer)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pane_rects_fit_without_overlap() {
        for (w, h) in [(760.0, 510.0), (950.0, 650.0), (1400.0, 900.0)] {
            let root = Rect::from_min_size(pos2(15.0, 22.0), vec2(w, h));
            let p = compute(root);
            for item in [p.header, p.files, p.editor, p.footer] {
                assert!(root.contains_rect(item));
                assert!(item.width() > 0.0 && item.height() > 0.0);
            }
            assert!(p.files.right() < p.editor.left());
            assert!(p.header.bottom() < p.files.top());
            assert!(p.editor.bottom() < p.footer.top());
            assert!(!p.files.intersects(p.editor));
        }
    }
}
