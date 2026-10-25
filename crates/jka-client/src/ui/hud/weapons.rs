//! TaystJK `CG_DrawWeaponSelect` (cg_weapons.c): weapon cycling strip.
//! This is deliberately separate from the status/ammo HUD; it uses the original
//! JKA weapon art already packed into the shared UI icon atlas.
use crate::ui::{
    fixed_charset_text, proportional_text, proportional_text_width,
    textured_rect_with_source, ProportionalFont, UiSnapshot, UiVertex, UiWeaponSelect,
    ICON_TEXTURE_SOURCE,
};
use crate::ui::hud::movement::{
    ICON_WEAPON_BASE, ICON_WEAPON_DUAL, ICON_WEAPON_NO_AMMO_BASE, ICON_WEAPON_STAFF,
};

// TaystJK swaps WP_CONCUSSION (15) in ahead of WP_ROCKET_LAUNCHER (11).
// LAST_USEABLE_WEAPON = WP_BRYAR_OLD (16); no WP_EMPLACED_GUN.
const WEAPON_ORDER: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 15, 11, 12, 13, 14, 16];
const WEAPON_NAMES: [&str; 17] = [
    "", "Stun Baton", "Melee", "Lightsaber", "Blaster Pistol",
    "E-11 Blaster Rifle", "Tenloss Disruptor Rifle", "Wookiee Bowcaster",
    "Imperial Heavy Repeater", "DEMP 2", "Golan Arms Flechette", "Merr-Sonn Missile System",
    "Thermal Detonator", "Trip Mine", "Det Pack", "Concussion Rifle", "Bryar Pistol",
];

/// Owned weapons can be shown with their `_na` artwork when depleted.
/// TaystJK specifically hides empty thermal detonators and trip mines from
/// neighbour slots, but retains an empty selected one until the next scroll.
fn visible_weapons(selector: UiWeaponSelect) -> Vec<u8> {
    let mut weapons = Vec::with_capacity(16);
    for weapon in WEAPON_ORDER {
        let owned = selector.owned_bits & (1 << weapon) != 0;
        let selectable = selector.selectable_bits & (1 << weapon) != 0;
        if (owned && (selectable || !matches!(weapon, 12 | 13)))
            || (weapon == selector.selected && matches!(weapon, 12 | 13))
        {
            weapons.push(weapon);
        }
    }
    weapons
}

fn weapon_icon_cell(selector: UiWeaponSelect, weapon: u8) -> usize {
    let id = usize::from(weapon);
    if selector.ammo_available_bits & (1 << weapon) == 0 {
        return ICON_WEAPON_NO_AMMO_BASE + id;
    }
    if weapon == 3 {
        match selector.saber_style {
            7 => return ICON_WEAPON_STAFF,
            6 => return ICON_WEAPON_DUAL,
            _ => {}
        }
    }
    ICON_WEAPON_BASE + id
}

pub(in crate::ui) fn build_weapon_select(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    // The App has already applied the 1400ms WEAPON_SELECT_TIME, dead-player,
    // spectator/follow, and emplaced-gun gates. The selection UI is independent
    // of cg_drawHud, just as the already-ported force selector is.
    let Some(selector) = ui.weapon_select else { return; };
    if !(1..=16).contains(&selector.selected) { return; }
    let weapons = visible_weapons(selector);
    let Some(center) = weapons.iter().position(|&weapon| weapon == selector.selected) else {
        return;
    };
    let count = weapons.len();
    if count == 0 { return; }

    // TaystJK's `cgs.widthRatioCoef` and sideMax thresholds. Icon geometry is
    // kept square on widescreen rather than stretching with the framebuffer.
    let ratio = (4.0 / 3.0) / (w.max(1) as f32 / h.max(1) as f32);
    let side_max = if ratio >= 0.8 { 3 } else if ratio >= 0.625 { 5 } else { 7 };
    let (left, right) = if count == 1 {
        (0, 0)
    } else if count > 2 * side_max {
        (side_max, side_max)
    } else {
        let left = (count - 1) / 2;
        (left, count - 1 - left)
    };

    const SMALL: f32 = 40.0;
    const BIG: f32 = 80.0;
    const PAD: f32 = 12.0;
    // JKA CG_DrawPic uses a virtual 640x480 screen and x = 320, y = 410.
    let s = h as f32 / 480.0;
    let cx = w as f32 * 0.5;
    let draw_icon = |out: &mut Vec<UiVertex>, weapon: u8, x: f32, y: f32, size: f32| {
        let (uv0, uv1) = ui.icon_uv(weapon_icon_cell(selector, weapon));
        textured_rect_with_source(
            out, x, y, size * s, size * s,
            uv0, uv1, [1.0; 4], ICON_TEXTURE_SOURCE, w, h,
        );
    };

    let mut x = cx - (BIG / 2.0 + PAD + SMALL) * s;
    for step in 1..=left {
        let weapon = weapons[(center + count - step) % count];
        draw_icon(out, weapon, x, 420.0 * s, SMALL);
        x -= (SMALL + PAD) * s;
    }

    draw_icon(out, selector.selected, cx - BIG * s / 2.0, 400.0 * s, BIG);

    x = cx + (BIG / 2.0 + PAD) * s;
    for step in 1..=right {
        let weapon = weapons[(center + step) % count];
        draw_icon(out, weapon, x, 420.0 * s, SMALL);
        x += (SMALL + PAD) * s;
    }

    // TaystJK: CG_DrawProportionalString(320, y+45, name, UI_SMALLFONT,
    // {0.875, 0.718, 0.121, 1}). Same amber and baseline, centered.
    let name = WEAPON_NAMES[usize::from(selector.selected)];
    let color = [0.875, 0.718, 0.121, 1.0];
    let text_y = 455.0 * s;
    if let Some(font) = small_font {
        let scale = 0.55;
        let width = proportional_text_width(name, font, scale) * w as f32 / 640.0;
        proportional_text(out, name, font, cx - width / 2.0, text_y, scale, color, true, w, h);
    } else {
        let glyph_width = 8.0 * s;
        let x = cx - name.len() as f32 * glyph_width * 0.5;
        fixed_charset_text(out, name, x, text_y, glyph_width, 12.0 * s,
            glyph_width, color, true, w, h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn selector(owned: &[u8], selected: u8) -> UiWeaponSelect {
        let bits = owned.iter().fold(0_u32, |bits, weapon| bits | (1_u32 << *weapon));
        UiWeaponSelect { selected, owned_bits: bits, selectable_bits: bits,
            ammo_available_bits: bits, saber_style: 1 }
    }
    #[test]
    fn concussion_is_between_flechette_and_rocket() {
        let visible = visible_weapons(selector(&[10, 11, 15], 15));
        assert_eq!(visible, [10, 15, 11]);
    }
    #[test]
    fn depleted_explosives_are_excluded_except_the_selected_one() {
        let mut value = selector(&[3, 12, 13, 14], 3);
        value.selectable_bits &= !((1 << 12) | (1 << 13));
        assert_eq!(visible_weapons(value), [3, 14]);
        value.selected = 12;
        assert_eq!(visible_weapons(value), [3, 12, 14]);
    }
    #[test]
    fn staff_and_dual_sabers_use_their_own_artwork() {
        let mut value = selector(&[3], 3);
        value.saber_style = 7;
        assert_eq!(weapon_icon_cell(value, 3), ICON_WEAPON_STAFF);
        value.saber_style = 6;
        assert_eq!(weapon_icon_cell(value, 3), ICON_WEAPON_DUAL);
        value.ammo_available_bits = 0;
        assert_eq!(weapon_icon_cell(value, 3), ICON_WEAPON_NO_AMMO_BASE + 3);
    }
    #[test]
    fn draws_icons_only_when_visible() {
        let mut ui = UiSnapshot::default();
        let mut vertices = Vec::new();
        build_weapon_select(&mut vertices, &ui, None, 1280, 720);
        assert!(vertices.is_empty());
        ui.weapon_select = Some(selector(&[3, 4, 5], 4));
        build_weapon_select(&mut vertices, &ui, None, 1280, 720);
        assert_eq!(vertices.iter().filter(|v| v.textured == ICON_TEXTURE_SOURCE).count(), 18);
        vertices.clear();
        ui.video.draw_hud = false; // cg_drawHud only gates the core status HUD.
        build_weapon_select(&mut vertices, &ui, None, 1280, 720);
        assert_eq!(vertices.iter().filter(|v| v.textured == ICON_TEXTURE_SOURCE).count(), 18);
    }
}
