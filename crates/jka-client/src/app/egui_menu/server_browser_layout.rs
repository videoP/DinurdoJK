//! Taffy geometry for the server browser. egui owns widgets, text input,
//! scrolling and popups, while Taffy owns their actual column/pane rectangles.
//! One table layout is shared by the header and EVERY row, preventing drift.
use crate::ui::flex::{Container, Flex};
use egui::{pos2, vec2, Pos2, Rect};
use taffy::prelude::AlignItems;

pub(super) const COLUMNS: usize = 7;
pub(super) const HEADER_H: f32 = 34.0;
pub(super) const ROW_H: f32 = 29.0;

#[derive(Clone, Copy, Debug)]
pub(super) struct Workspace {
    pub list: Rect,
    pub detail: Rect,
    pub stacked: bool,
}

fn to_rect(origin: Pos2, b: [f32; 4]) -> Rect {
    Rect::from_min_size(pos2(origin.x + b[0], origin.y + b[1]), vec2(b[2], b[3]))
}

/// Two-pane layout, with a responsive fallback on smaller screens. The list
/// takes all residual space instead of being constrained by a hardcoded width.
pub(super) fn workspace(rect: Rect) -> Workspace {
    let width = rect.width().max(1.0);
    let height = rect.height().max(1.0);
    let stacked = width < 950.0;
    let mut flex = Flex::new();
    let list = flex.spring();
    let detail = if stacked {
        flex.leaf(width, (height * 0.31).clamp(190.0, 290.0))
    } else {
        flex.leaf((width * 0.255).clamp(290.0, 355.0), height)
    };
    let root = if stacked {
        flex.node(Container::column().width(width).height(height).gap(12.0).align(AlignItems::STRETCH), &[list, detail])
    } else {
        flex.node(Container::row().width(width).height(height).gap(14.0).align(AlignItems::STRETCH), &[list, detail])
    };
    flex.compute(root, width, height);
    Workspace { list: to_rect(rect.min, flex.bounds(list)), detail: to_rect(rect.min, flex.bounds(detail)), stacked }
}

/// Taffy calculates the x-coordinates and widths once per browser frame.
/// Name grows/shrinks; data columns use compact responsive widths. The same
/// `Table` is used for both header buttons and server rows.
#[derive(Clone, Copy, Debug)]
pub(super) struct Table {
    x: [f32; COLUMNS],
    widths: [f32; COLUMNS],
    pub width: f32,
}

impl Table {
    pub fn compute(available_width: f32) -> Self {
        // A vertical scrollbar occupies some of the viewport. Leave its
        // gutter clear instead of having rightmost cells disappear behind it.
        let width = (available_width - 14.0).max(1.0);
        let compact = width < 1000.0;
        let nominal: [f32; 6] = if compact {
            [102.0, 90.0, 77.0, 84.0, 64.0, 113.0]
        } else {
            [130.0, 121.0, 96.0, 91.0, 72.0, 129.0]
        };
        let total: f32 = nominal.iter().sum();
        // Protect some space for NAME on narrow screens. Smaller fields
        // gracefully contract rather than overflow the right edge.
        let scale = ((width - 136.0).max(1.0) / total).min(1.0);
        let mut flex = Flex::new();
        let name = flex.spring();
        let cells = [
            name,
            flex.leaf(nominal[0] * scale, HEADER_H),
            flex.leaf(nominal[1] * scale, HEADER_H),
            flex.leaf(nominal[2] * scale, HEADER_H),
            flex.leaf(nominal[3] * scale, HEADER_H),
            flex.leaf(nominal[4] * scale, HEADER_H),
            flex.leaf(nominal[5] * scale, HEADER_H),
        ];
        let row = flex.node(Container::row().width(width).height(HEADER_H), &cells);
        flex.compute(row, width, HEADER_H);
        let mut x = [0.0; COLUMNS];
        let mut widths = [0.0; COLUMNS];
        for (i, node) in cells.iter().enumerate() {
            let b = flex.bounds(*node);
            x[i] = b[0];
            widths[i] = b[2];
        }
        Self { x, widths, width }
    }

    pub fn cell(self, row_rect: Rect, column: usize) -> Rect {
        Rect::from_min_size(
            pos2(row_rect.left() + self.x[column], row_rect.top()),
            vec2(self.widths[column], row_rect.height()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_columns_are_contiguous_and_fit() {
        for width in [660.0, 800.0, 1200.0, 1780.0] {
            let t = Table::compute(width);
            let row = Rect::from_min_size(pos2(13.0, 31.0), vec2(t.width, ROW_H));
            let first = t.cell(row, 0);
            assert!((first.left() - row.left()).abs() < 0.1);
            for i in 1..COLUMNS {
                let prev = t.cell(row, i - 1);
                let next = t.cell(row, i);
                assert!((prev.right() - next.left()).abs() < 0.2, "{width}: column {i} drifts");
                assert!(next.width() > 0.0);
            }
            assert!((t.cell(row, COLUMNS - 1).right() - row.left() - t.width).abs() < 0.2);
        }
    }

    #[test]
    fn workspace_regions_do_not_overlap() {
        for (width, height) in [(820.0, 720.0), (1550.0, 790.0)] {
            let area = Rect::from_min_size(pos2(24.0, 90.0), vec2(width, height));
            let layout = workspace(area);
            assert!(area.contains_rect(layout.list) && area.contains_rect(layout.detail));
            assert!(!layout.list.intersects(layout.detail));
            assert!(layout.list.height() > 180.0 && layout.list.width() > 400.0);
            if layout.stacked {
                assert!(layout.list.bottom() < layout.detail.top());
            } else {
                assert!(layout.list.right() < layout.detail.left());
            }
        }
    }
}
