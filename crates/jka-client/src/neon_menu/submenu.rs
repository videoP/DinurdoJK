//! Taffy layout for clean, understated frontend submenus. The same normalized
//! rectangles feed egui hit testing and neon tube drawing. No hard-coded
//! fullscreen pixel positions and no stock egui widgets in the menu body.
use super::{font, layout::TextLine};
use taffy::prelude::{length, AvailableSpace, FlexDirection, Size, Style, TaffyTree};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SubmenuLayout<const N: usize> {
    pub lines: [TextLine; N],
    pub hit: [[f32; 4]; N],
}

/// Layout for main-menu-style signs, slightly inset from the main
/// column. Uses the *same* font cap sizes/group spacing as the main stage,
/// while retaining the full-viewport Taffy hitbox/paint coordinate contract.
pub(crate) fn compute<const N: usize>(w: f32, h: f32, labels: &[&str; N]) -> Option<SubmenuLayout<N>> {
    let (w, h) = (w.max(1.0), h.max(1.0));
    let scale = (h / 1080.0).min(w / 1500.0).max(0.35);
    let caps = [80.0 * scale, 34.0 * scale, 21.0 * scale, 21.0 * scale];
    let heights = [124.0 * scale, 66.0 * scale, 46.0 * scale, 46.0 * scale];
    let mut tree = TaffyTree::<()>::new();
    let mut children = Vec::new();
    for (i, label) in labels.iter().enumerate() {
        let node = tree.new_leaf(Style {
            size: Size { width: length((font::measure(label) + 1.2) * caps[i]), height: length(heights[i]) },
            flex_shrink: 0.0,
            ..Default::default()
        }).ok()?;
        children.push(node);
        if i + 1 < N {
            children.push(tree.new_leaf(Style {
                size: Size { width: length(1.0), height: length(26.0 * scale) },
                ..Default::default()
            }).ok()?);
        }
    }
    let col = tree.new_with_children(Style { flex_direction: FlexDirection::Column, ..Default::default() }, &children).ok()?;
    let root = tree.new_with_children(Style {
        size: Size { width: length(w), height: length(h) }, ..Default::default()
    }, &[col]).ok()?;
    tree.compute_layout(root, Size {
        width: AvailableSpace::Definite(w), height: AvailableSpace::Definite(h),
    }).ok()?;
    // The root's default cross-axis stretch can make `col` as tall as the
    // viewport. Measure the last row instead to center the actual signs.
    let last = tree.layout(*children.last()?).ok()?;
    let content_h = last.location.y + last.size.height;
    let left = (0.105 * w).max(42.0 * scale);
    let top = ((h - super::footer::HEIGHT - content_h) * 0.46).max(0.04 * h);
    let mut lines = [TextLine::default(); N];
    let mut hit = [[0.0; 4]; N];
    for i in 0..N {
        let node = children[i * 2];
        let r = tree.layout(node).ok()?;
        let x = left + r.location.x + if i == 0 { 0.0 } else { 12.0 * scale };
        let y = top + r.location.y;
        lines[i] = TextLine {
            x: (x + 0.6 * caps[i]) / w,
            baseline: (y + r.size.height * 0.5 + 0.5 * caps[i]) / h,
            cap: caps[i] / h,
        };
        hit[i] = [x / w, y / h, (x + r.size.width).min(w) / w,
            (y + r.size.height).min(h - super::footer::HEIGHT) / h];
    }
    Some(SubmenuLayout { lines, hit })
}

/// The renderer's fixed-size GPU buffers hold up to the main page's seven
/// signs. Unused slots on a submenu stay empty.
pub(crate) fn neon_layout<const N: usize>(page: &SubmenuLayout<N>) -> super::layout::MenuLayout {
    let mut result = super::layout::MenuLayout::default();
    result.items[..N].copy_from_slice(&page.lines);
    result.hit[..N].copy_from_slice(&page.hit);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn developer_tools_rows_do_not_overlap_footer_or_each_other() {
        for (w,h) in [(1920.,1080.),(1280.,720.),(1024.,768.),(800.,600.),(3440.,1440.)] {
            let p=compute(w,h, &["ASSET VIEWER", "RENDER STUDIO", "MAP VIEWER", "PK3 MANAGER"]).unwrap();
            for (i,r) in p.hit.iter().enumerate() {
                assert!(r[0]>=0.0 && r[2]<=1.0 && r[1]>=0.0 && r[3]<=1.0-crate::neon_menu::footer::HEIGHT/h);
                if i>0 { assert!(p.hit[i-1][3]<=r[1]); }
            }
        }
    }
}
