//! Persistent frontend chrome. One Taffy layout drives pointer targets and
//! the WGPU neon compositor; no egui panels, fonts or button widgets.
use super::{font, layout::TextLine};
use taffy::prelude::{length, AvailableSpace, FlexDirection, JustifyContent, AlignItems, NodeId, Size, Style, TaffyTree};

pub(crate) const HEIGHT: f32 = 74.0;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FooterLayout {
    pub top: f32,
    pub brand: TextLine,
    pub apply: Option<(TextLine, [f32; 4])>,
    pub back: Option<(TextLine, [f32; 4])>,
    pub quit: (TextLine, [f32; 4]),
}

pub(crate) fn compute(w: f32, h: f32, back: bool) -> Option<FooterLayout> {
    compute_with_action(w, h, back, None)
}

pub(crate) fn compute_with_action(w: f32, h: f32, back: bool, action: Option<&str>) -> Option<FooterLayout> {
    let (w, h) = (w.max(1.0), h.max(1.0));
    let scale = (h / 1080.0).min(w / 1280.0).clamp(0.65, 1.6);
    let height = HEIGHT.min(h * 0.19);
    let cap = (17.0 * scale).min(height * 0.32);
    // Match the main menu title cap size and tube geometry at the same resolution.
    let title_scale = (h / 1080.0).min(w / 1500.0).max(0.35);
    let brand_cap = (20.0 * title_scale).min(height * 0.32);
    let edge = (48.0 * scale).min(w * 0.055).max(16.0);
    let mut t = TaffyTree::<()>::new();
    let leaf = |tree: &mut TaffyTree<()>, width: f32| tree.new_leaf(Style {
        size: Size { width: length(width), height: length(height) },
        flex_shrink: 0.0, ..Default::default()
    }).ok();
    let brand = leaf(&mut t, font::measure(super::TITLE) * brand_cap + 7.0 * scale)?;
    let left = t.new_with_children(Style { flex_direction: FlexDirection::Row,
        align_items: AlignItems::CENTER, ..Default::default() }, &[brand]).ok()?;
    let action_node = if let Some(label) = action { Some(leaf(&mut t, font::measure(label) * cap + 48.0 * scale)?) } else { None };
    let back_node = if back { Some(leaf(&mut t, font::measure("BACK") * cap + 62.0 * scale)?) } else { None };
    let quit_node = leaf(&mut t, font::measure("QUIT") * cap + 62.0 * scale)?;
    let mut buttons = Vec::new();
    if let Some(node) = action_node { buttons.push(node); }
    if let Some(node) = back_node { buttons.push(node); }
    buttons.push(quit_node);
    let right = t.new_with_children(Style { flex_direction: FlexDirection::Row,
        align_items: AlignItems::CENTER, gap: taffy::prelude::Size { width: length(11.0 * scale), height: length(0.0) },
        ..Default::default() }, &buttons).ok()?;
    let root = t.new_with_children(Style {
        size: Size { width: length((w - edge * 2.0).max(1.0)), height: length(height) },
        flex_direction: FlexDirection::Row, justify_content: JustifyContent::SPACE_BETWEEN,
        align_items: AlignItems::CENTER, ..Default::default()
    }, &[left, right]).ok()?;
    t.compute_layout(root, Size { width: AvailableSpace::Definite((w - edge*2.0).max(1.0)),
        height: AvailableSpace::Definite(height) }).ok()?;
    let left_l = t.layout(left).ok()?;
    let right_l = t.layout(right).ok()?;
    let make_line = |node: NodeId, parent: [f32; 2], cap: f32, label: &str| -> Option<(TextLine, [f32; 4])> {
        let l = t.layout(node).ok()?;
        let x = edge + parent[0] + l.location.x;
        let y = h - height + parent[1] + l.location.y;
        let pad = (l.size.width - font::measure(label)*cap).max(0.0)*0.5;
        let line = TextLine { x: (x + pad) / w,
            baseline: (y + height*0.5 + cap*0.34) / h,
            cap: cap/h };
        Some((line, [x/w, y/h, (x+l.size.width)/w, (y+l.size.height)/h]))
    };
    let brand = make_line(brand, [left_l.location.x, left_l.location.y], brand_cap, super::TITLE)?.0;
    let apply = action_node.map(|node| make_line(node, [right_l.location.x, right_l.location.y], cap, action.unwrap_or(""))).transpose_option()?;
    let back = back_node.map(|node| make_line(node, [right_l.location.x, right_l.location.y], cap, "BACK")).transpose_option()?;
    let quit = make_line(quit_node, [right_l.location.x, right_l.location.y], cap, "QUIT")?;
    Some(FooterLayout { top: (h-height)/h, brand, apply, back, quit })
}

trait TransposeOption<T> { fn transpose_option(self) -> Option<Option<T>>; }
impl<T> TransposeOption<T> for Option<Option<T>> {
    fn transpose_option(self) -> Option<Option<T>> { match self { Some(Some(v))=>Some(Some(v)), Some(None)=>None, None=>Some(None) } }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn footer_layout_buttons_are_inside_and_do_not_overlap() {
        for (w,h) in [(1920.0,1080.0),(1280.0,720.0),(800.0,600.0),(3440.0,1440.0)] {
            let f = compute(w,h,true).unwrap();
            let b=f.back.unwrap().1; let q=f.quit.1;
            assert!(b[2]<q[0] && q[2]<=1.0 && b[1]>=f.top && q[3]<=1.0);
            let a=compute_with_action(w,h,true,Some("APPLY (5)")).unwrap();
            assert!(a.apply.unwrap().1[2] < a.back.unwrap().1[0]);
        }
    }
}
