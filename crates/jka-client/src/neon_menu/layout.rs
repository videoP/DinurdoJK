//! Taffy layout of the main page. One function, used for both hit testing and
//! drawing, so the two can never disagree. Results are normalised to the view:
//! x by width, y and cap heights by height, so the renderer can map them onto
//! physical pixels without knowing the UI scale.
use super::{font, Group, ITEMS, ITEM_COUNT, TITLE};
use taffy::prelude::{
    auto, length, AvailableSpace, FlexDirection, NodeId, Position, Rect, Size, Style, TaffyTree,
};

/// A neon text line: left edge, baseline and cap height.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct TextLine {
    pub x: f32,
    pub baseline: f32,
    pub cap: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct MenuLayout {
    pub title: TextLine,
    pub items: [TextLine; ITEM_COUNT],
    /// Pointer targets, [x0, y0, x1, y1].
    pub hit: [[f32; 4]; ITEM_COUNT],
    /// Vertical lightbox sign; zero-area when the view is too narrow.
    pub lightbox: [f32; 4],
}

struct Metrics {
    cap: f32,
    row: f32,
}

fn metrics(group: Group, s: f32) -> Metrics {
    let (cap, row) = match group {
        Group::Hero => (80.0, 1.55),
        Group::Primary => (34.0, 1.95),
        Group::Utility | Group::Exit => (21.0, 2.15),
    };
    Metrics { cap: cap * s, row: cap * s * row }
}

fn leaf(tree: &mut TaffyTree<()>, w: f32, h: f32) -> Option<NodeId> {
    tree.new_leaf(Style {
        size: Size { width: length(w), height: length(h) },
        flex_shrink: 0.0,
        ..Default::default()
    })
    .ok()
}

fn absolute(tree: &mut TaffyTree<()>, x: f32, y: f32, w: f32, h: f32) -> Option<NodeId> {
    tree.new_leaf(Style {
        position: Position::Absolute,
        inset: Rect { left: length(x), top: length(y), right: auto(), bottom: auto() },
        size: Size { width: length(w), height: length(h) },
        ..Default::default()
    })
    .ok()
}

/// Lay out the page for a `w` x `h` view (any unit; egui points in practice).
pub(crate) fn compute(w: f32, h: f32) -> Option<MenuLayout> {
    let w = w.max(1.0);
    let h = h.max(1.0);
    // 1080p reference. Narrow windows shrink by width instead of overflowing.
    let s = (h / 1080.0).min(w / 1500.0).max(0.35);
    let title_cap = 20.0 * s;
    let title_row = title_cap + 64.0 * s;
    let gap = 30.0 * s;
    let pad = 0.6;

    let mut tree = TaffyTree::new();
    let mut children = Vec::new();
    let mut rows = [None; ITEM_COUNT];
    let title = leaf(&mut tree, font::measure(TITLE) * title_cap, title_row)?;
    children.push(title);
    let mut previous = None;
    for (index, info) in ITEMS.iter().enumerate() {
        if previous.is_some_and(|group| group != info.group) {
            children.push(leaf(&mut tree, 1.0, gap)?);
        }
        previous = Some(info.group);
        let m = metrics(info.group, s);
        let node = leaf(&mut tree, (font::measure(info.label) + pad * 2.0) * m.cap, m.row)?;
        rows[index] = Some(node);
        children.push(node);
    }
    let column = tree
        .new_with_children(Style { flex_direction: FlexDirection::Column, ..Default::default() }, &children)
        .ok()?;
    let mut nodes = vec![column];

    // The lightbox hangs just left of the scene's TV, only when the view is
    // wide enough to keep the scene's centre clear of it.
    let wide = w / h >= 1.3 && w >= 900.0;
    let lightbox = if wide {
        let lightbox_w = 0.052 * h;
        let tv_left = super::tv_corners(w / h).iter().map(|c| c[0]).fold(1.0, f32::min) * w;
        let lightbox = absolute(&mut tree, tv_left - 0.03 * w - lightbox_w, 0.11 * h, lightbox_w, 0.47 * h)?;
        nodes.push(lightbox);
        Some(lightbox)
    } else {
        None
    };

    let root = tree
        .new_with_children(
            Style { size: Size { width: length(w), height: length(h) }, ..Default::default() },
            &nodes,
        )
        .ok()?;
    tree.compute_layout(root, Size {
        width: AvailableSpace::Definite(w),
        height: AvailableSpace::Definite(h),
    })
    .ok()?;

    // Centre the column vertically, a little high, and inset it from the left.
    // The root stretches the column to full height; measure its content.
    let last = tree.layout(*children.last()?).ok()?;
    let content_height = last.location.y + last.size.height;
    let left = (0.075 * w).max(40.0 * s);
    let top = ((h - content_height) * 0.47).max(0.05 * h);
    let row_rect = |node: NodeId| -> Option<[f32; 4]> {
        let l = tree.layout(node).ok()?;
        Some([left + l.location.x, top + l.location.y, left + l.location.x + l.size.width, top + l.location.y + l.size.height])
    };
    let normalise = |r: [f32; 4]| [r[0] / w, r[1] / h, r[2] / w, r[3] / h];
    let line = |r: [f32; 4], cap: f32| TextLine {
        x: (r[0] + pad * cap) / w,
        baseline: ((r[1] + r[3]) * 0.5 + cap * 0.5) / h,
        cap: cap / h,
    };

    let title_rect = row_rect(title)?;
    let mut layout = MenuLayout {
        title: TextLine { x: (left + pad * title_cap) / w, baseline: (title_rect[1] + title_cap) / h, cap: title_cap / h },
        ..Default::default()
    };
    for (index, info) in ITEMS.iter().enumerate() {
        let r = row_rect(rows[index]?)?;
        let m = metrics(info.group, s);
        layout.items[index] = line(r, m.cap);
        layout.hit[index] = normalise(r);
    }
    let placed = |node: Option<NodeId>| -> Option<[f32; 4]> {
        let Some(node) = node else { return Some([0.0; 4]) };
        let l = tree.layout(node).ok()?;
        Some(normalise([l.location.x, l.location.y, l.location.x + l.size.width, l.location.y + l.size.height]))
    };
    layout.lightbox = placed(lightbox)?;
    Some(layout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_fit_and_never_overlap() {
        for (w, h) in [(1920.0, 1080.0), (2560.0, 1440.0), (1280.0, 720.0), (1024.0, 768.0), (800.0, 600.0), (3440.0, 1440.0)] {
            let layout = compute(w, h).expect("layout");
            for (i, hit) in layout.hit.iter().enumerate() {
                assert!(hit[0] >= 0.0 && hit[2] <= 1.0 && hit[1] >= 0.0 && hit[3] <= 1.0, "{w}x{h} item {i}: {hit:?}");
                if i > 0 {
                    assert!(layout.hit[i - 1][3] <= hit[1] + 1e-5, "{w}x{h} rows {i} overlap");
                }
            }
            let column_right = layout.hit.iter().map(|r| r[2]).fold(0.0, f32::max);
            if layout.lightbox[2] > layout.lightbox[0] {
                assert!(layout.lightbox[0] > column_right, "{w}x{h}: lightbox overlaps the menu");
                let tv_left = crate::neon_menu::tv_corners(w / h).iter().map(|c| c[0]).fold(1.0, f32::min);
                assert!(layout.lightbox[2] < tv_left, "{w}x{h}: lightbox overlaps the TV");
            }
        }
    }
}
