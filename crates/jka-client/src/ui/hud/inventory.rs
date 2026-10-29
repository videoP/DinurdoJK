//! TaystJK `CG_DrawInvenSelect` (cg_draw.c): scrollable holdable-item strip.
//! Reuses the shared, native-resolution UI icon atlas (no per-frame texture IO).
use crate::ui::{
    fixed_charset_text, proportional_text, proportional_text_width,
    textured_rect_with_source, ProportionalFont, UiInventorySelect, UiSnapshot,
    UiVertex, ICON_TEXTURE_SOURCE,
};
use super::movement::ICON_INVENTORY_BASE;

const HI_NUM_HOLDABLE: usize = 12;
// Same tag -> name mapping as bg_itemlist IT_HOLDABLE. The stock selector
// localizes the classname via SP_INGAME_* (if available), with classname as
// fallback. These readable English captions follow the existing weapon/Force
// selector approach until the shared string table is exposed to this layer.
const ITEM_NAMES: [&str; HI_NUM_HOLDABLE] = [
    "", "Seeker Drone", "Portable Shield", "Bacta Canister",
    "Large Bacta Canister", "Binoculars", "Sentry Gun", "Jetpack",
    "Health Dispenser", "Ammo Dispenser", "E-Web", "Cloaking Device",
];

/// BG_IsItemSelectable excludes the jetpack and Siege dispensers from scrolling.
fn selectable(tag: usize) -> bool {
    (1..HI_NUM_HOLDABLE).contains(&tag) && !matches!(tag, 7 | 8 | 9)
}

fn visible_items(selector: UiInventorySelect) -> Vec<usize> {
    (1..HI_NUM_HOLDABLE)
        .filter(|&tag| selectable(tag) && selector.owned_bits & (1 << tag) != 0)
        .collect()
}

pub(in crate::ui) fn build_inventory_select(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    let Some(selector) = ui.inventory_select else { return; };
    let selected = usize::from(selector.selected);
    if !selectable(selected) { return; }
    let items = visible_items(selector);
    let Some(center) = items.iter().position(|&tag| tag == selected) else { return; };
    let count = items.len();

    // TaystJK: three icons on each side maximum; with fewer items divide
    // the remaining icons between left and right (odd goes to the right).
    let (left, right) = if count == 1 { (0, 0) }
        else if count > 6 { (3, 3) }
        else { let left = (count - 1) / 2; (left, count - 1 - left) };
    const SMALL: f32 = 40.0;
    const BIG: f32 = 80.0;
    const PAD: f32 = 16.0;
    // Stock CG_DrawInvenSelect centers x=320/y=410 in virtual 640x480.
    // Like the other selectors, keep icons square on widescreen displays.
    let scale = h as f32 / 480.0;
    let cx = w as f32 * 0.5;
    let draw_icon = |out: &mut Vec<UiVertex>, tag: usize, x: f32, y: f32, size: f32| {
        let (uv0, uv1) = ui.icon_uv(ICON_INVENTORY_BASE + tag);
        textured_rect_with_source(out, x, y, size * scale, size * scale,
            uv0, uv1, [1.0; 4], ICON_TEXTURE_SOURCE, w, h);
    };

    let mut x = cx - (BIG / 2.0 + PAD + SMALL) * scale;
    for step in 1..=left {
        draw_icon(out, items[(center + count - step) % count], x, 420.0 * scale, SMALL);
        x -= (SMALL + PAD) * scale;
    }
    draw_icon(out, selected, cx - BIG * scale / 2.0, 400.0 * scale, BIG);
    x = cx + (BIG / 2.0 + PAD) * scale;
    for step in 1..=right {
        draw_icon(out, items[(center + step) % count], x, 420.0 * scale, SMALL);
        x += (SMALL + PAD) * scale;
    }

    // Stock inventory captions use a green tint instead of the weapon amber.
    let label = ITEM_NAMES[selected];
    let tint = [0.312, 0.75, 0.621, 1.0];
    let y = 455.0 * scale;
    if let Some(font) = small_font {
        let font_scale = 0.55;
        let width = proportional_text_width(label, font, font_scale) * w as f32 / 640.0;
        proportional_text(out, label, font, cx - width / 2.0, y,
            font_scale, tint, true, w, h);
    } else {
        let glyph_width = 8.0 * scale;
        let x = cx - label.len() as f32 * glyph_width / 2.0;
        fixed_charset_text(out, label, x, y, glyph_width, 12.0 * scale,
            glyph_width, tint, true, w, h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inventory_neighbours_skip_unselectable_items() {
        let items = visible_items(UiInventorySelect {
            selected: 4,
            owned_bits: (1 << 1) | (1 << 4) | (1 << 7) | (1 << 8) | (1 << 11),
        });
        assert_eq!(items, [1, 4, 11]);
    }
    #[test]
    fn holdable_selector_uses_shared_icon_atlas() {
        let mut ui = UiSnapshot::default();
        let mut verts = Vec::new();
        build_inventory_select(&mut verts, &ui, None, 1280, 720);
        assert!(verts.is_empty());
        ui.inventory_select = Some(UiInventorySelect {
            selected: 4,
            owned_bits: (1 << 1) | (1 << 4) | (1 << 11),
        });
        build_inventory_select(&mut verts, &ui, None, 1280, 720);
        assert_eq!(verts.iter().filter(|v| v.textured == ICON_TEXTURE_SOURCE).count(), 18);
    }
    #[test]
    fn unowned_selection_does_not_draw_anything() {
        let mut ui = UiSnapshot::default();
        ui.inventory_select = Some(UiInventorySelect { selected: 4, owned_bits: 1 << 1 });
        let mut verts = Vec::new();
        build_inventory_select(&mut verts, &ui, None, 1280, 720);
        assert!(verts.is_empty());
    }
}
