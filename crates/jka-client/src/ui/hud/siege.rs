//! Siege-specific HUD overlays ported from TaystJK/OpenJK semantics.

use crate::ui::{fixed_charset_text, proportional_text, rect, rect_outline, textured_rect_with_source, ProportionalFont, UiSiegeTimerMenu, UiSnapshot, UiVertex};
use super::extensions::{image_cell_uv_for_count, EXT_IMAGE_TEXTURE_SOURCE};


/// Parse the same menu and item names consumed by TaystJK's
/// CG_DrawSiegeDeathTimer. The path is the active game's `ui/hud.menu`, so
/// custom HUD replacements retain their authored colors, positions and art.
pub(crate) fn load_mp_timer(base: &std::path::Path, game: Option<&std::path::Path>) -> Option<UiSiegeTimerMenu> {
    let mut assets = jka_assets::pk3::AssetSearchPath::open_game(base, game).ok()?;
    let bytes = assets.read("ui/hud.menu", 2 * 1024 * 1024).ok()??.bytes;
    let contents = String::from_utf8_lossy(&bytes);
    parse_mp_timer(&contents)
}

// UI .menu files use Quake tokens (quoted strings, nested braces, // and /* */
// comments). Parse block depth so a different item's "frame" cannot match.
fn menu_tokens(text: &str) -> Vec<String> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_whitespace() { i += 1; continue; }
        if b[i] == b'/' && b.get(i+1) == Some(&b'/') {
            i += 2;
            while i < b.len() && b[i] != b'\n' { i += 1; }
            continue;
        }
        if b[i] == b'/' && b.get(i+1) == Some(&b'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i+1] == b'/') { i += 1; }
            i = (i+2).min(b.len());
            continue;
        }
        if b[i] == b'"' {
            let start = i+1;
            i += 1;
            while i < b.len() && b[i] != b'"' { if b[i] == b'\\' && i+1 < b.len() { i += 1; } i += 1; }
            out.push(text.get(start..i).unwrap_or("").to_owned());
            i = (i+1).min(b.len());
            continue;
        }
        if b[i] == b'{' || b[i] == b'}' { out.push((b[i] as char).to_string()); i += 1; continue; }
        let start = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'{' && b[i] != b'}' { i += 1; }
        out.push(text.get(start..i).unwrap_or("").to_owned());
    }
    out
}

fn token_block_end(t: &[String], start: usize) -> Option<usize> {
    if t.get(start).map(String::as_str) != Some("{") { return None; }
    let mut depth = 0;
    for (i, token) in t.iter().enumerate().skip(start) {
        if token == "{" { depth += 1; }
        if token == "}" { depth -= 1; if depth == 0 { return Some(i); } }
    }
    None
}

fn item_value<'a>(t: &'a [String], start: usize, end: usize, name: &str) -> Option<&'a str> {
    let mut depth = 0;
    for i in start+1..end {
        match t[i].as_str() {
            "{" => depth += 1,
            "}" => depth -= 1,
            key if depth == 0 && key.eq_ignore_ascii_case(name) => return t.get(i+1).map(String::as_str),
            _ => {}
        }
    }
    None
}
fn item_floats<const N: usize>(t: &[String], start: usize, end: usize, key: &str) -> Option<[f32; N]> {
    let mut depth = 0;
    for i in start+1..end {
        match t[i].as_str() {
            "{" => depth += 1,
            "}" => depth -= 1,
            k if depth == 0 && k.eq_ignore_ascii_case(key) => {
                let mut result = [0f32; N];
                for (j, slot) in result.iter_mut().enumerate() {
                    *slot = t.get(i+1+j)?.parse::<f32>().ok()?;
                    if !slot.is_finite() { return None; }
                }
                return Some(result);
            }
            _ => {}
        }
    }
    None
}
fn parse_mp_timer(text: &str) -> Option<UiSiegeTimerMenu> {
    let tokens = menu_tokens(text);
    let mut pos = 0;
    while pos + 1 < tokens.len() {
        if !tokens[pos].eq_ignore_ascii_case("menuDef") || tokens[pos+1] != "{" { pos += 1; continue; }
        let begin = pos+1;
        let end = token_block_end(&tokens, begin)?;
        if item_value(&tokens, begin, end, "name").is_some_and(|name| name.eq_ignore_ascii_case("mp_timer")) {
            let mut frame = None;
            let mut death = None;
            let mut j = begin+1;
            while j+1 < end {
                if tokens[j].eq_ignore_ascii_case("itemDef") && tokens[j+1] == "{" {
                    let sub_begin = j+1;
                    let sub_end = token_block_end(&tokens, sub_begin)?;
                    match item_value(&tokens, sub_begin, sub_end, "name") {
                        Some("frame") => frame = Some((
                            item_floats::<4>(&tokens, sub_begin, sub_end, "rect")?,
                            item_value(&tokens, sub_begin, sub_end, "background")?.to_owned(),
                            item_floats::<4>(&tokens, sub_begin, sub_end, "forecolor")?.map(|f| f.clamp(0.0, 1.0)),
                        )),
                        Some("deathtimer") => death = Some((
                            item_floats::<4>(&tokens, sub_begin, sub_end, "rect")?,
                            item_floats::<4>(&tokens, sub_begin, sub_end, "forecolor")?.map(|f| f.clamp(0.0, 1.0)),
                        )),
                        _ => {}
                    }
                    j = sub_end+1;
                } else { j += 1; }
            }
            let (mut frame_rect, frame_shader, frame_color) = frame?;
            let (mut text_rect, text_color) = death?;
            // TaystJK ui_shared.c Menu_PostParse aligns each item window rect
            // relative to its containing menuDef before it is drawn.
            // Item-only coordinates place the timer at the wrong origin.
            let menu_rect = item_floats::<4>(&tokens, begin, end, "rect")?;
            for rect in [&mut frame_rect, &mut text_rect] {
                rect[0] += menu_rect[0];
                rect[1] += menu_rect[1];
            }
            return Some(UiSiegeTimerMenu { frame_rect, frame_shader, frame_color, text_rect, text_color });
        }
        pos = end+1;
    }
    None
}

/// The atlas has one reserved *extra* cell beyond the 32 extension slots.
/// This must be held alongside existing extension images in every UI update.
pub(crate) fn append_mp_timer_image(images: &mut Vec<String>, menu: Option<&UiSiegeTimerMenu>) {
    if let Some(menu) = menu {
        if !images.iter().any(|p| p == &menu.frame_shader) { images.push(menu.frame_shader.clone()); }
    }
}

fn draw_mp_timer(out: &mut Vec<UiVertex>, text: &str, menu: &UiSiegeTimerMenu, images: &[String], font: Option<&ProportionalFont>, w: u32, h: u32) {
    // CG_DrawSiegeDeathTimer anchors to the right using cgs.widthRatioCoef.
    // The framebuffer's Y scale gives the 4:3 virtual coordinate unit.
    let unit = h as f32 / 480.0;
    let x = |virtual_x: f32| w as f32 - (640.0 - virtual_x) * unit;
    if let Some(index) = images.iter().position(|p| p == &menu.frame_shader) {
        let [fx, fy, fw, fh] = menu.frame_rect;
        let (uv0, uv1) = image_cell_uv_for_count(index, images.len());
        textured_rect_with_source(out, x(fx), fy * unit, fw * unit, fh * unit, uv0, uv1, menu.frame_color, EXT_IMAGE_TEXTURE_SOURCE, w, h);
    }
    if let Some(font) = font {
        // TaystJK cg_drawtools.c CG_DrawProportionalString selects FONT_SMALL
        // and calls CG_Text_Paint with scale 1.0 (NOT 0.75). Preserve the
        // authored item coordinates and use a 2-virtual-unit drop shadow.
        // Compose in 640x480 virtual coordinates, then apply the exact same
        // right-edge scale as CG_DrawSiegeDeathTimer's widthRatioCoef path.
        let virtual_x = menu.text_rect[0];
        // CG_DrawProportionalString accepts a top-left Y. DinurdoJK's
        // proportional_text accepts a baseline and subtracts the glyph's
        // baseline internally. Convert once using the actual font metrics,
        // rather than arbitrarily shifting the authored mp_timer item.
        let virtual_y = menu.text_rect[1]
            + font.glyphs[b'0' as usize].baseline as f32;
        let start = out.len();
        proportional_text(out, text, font, virtual_x + 2.0, virtual_y + 2.0,
            1.0, [0.0, 0.0, 0.0, menu.text_color[3]], false, 640, 480);
        proportional_text(out, text, font, virtual_x, virtual_y,
            1.0, menu.text_color, false, 640, 480);
        for vertex in &mut out[start..] {
            let virtual_x = (vertex.position[0] + 1.0) * 320.0;
            let virtual_y = (1.0 - vertex.position[1]) * 240.0;
            let screen_x = x(virtual_x);
            let screen_y = virtual_y * unit;
            vertex.position = [screen_x / w as f32 * 2.0 - 1.0, 1.0 - screen_y / h as f32 * 2.0];
        }
    }
}

fn centered_text(
    out: &mut Vec<UiVertex>,
    value: &str,
    y: f32,
    scale: f32,
    color: [f32; 4],
    w: u32,
    h: u32,
) {
    let glyph_w = 8.0 * scale;
    let glyph_h = 12.0 * scale;
    let advance = 8.0 * scale;
    let x = ((w as f32 - value.chars().count() as f32 * advance) * 0.5).max(4.0);
    fixed_charset_text(out, value, x + 1.0, y + 1.0, glyph_w, glyph_h, advance, [0.0, 0.0, 0.0, 0.75], true, w, h);
    fixed_charset_text(out, value, x, y, glyph_w, glyph_h, advance, color, true, w, h);
}

pub(in crate::ui) fn build_siege_hud(out: &mut Vec<UiVertex>, ui: &UiSnapshot, font: Option<&ProportionalFont>, w: u32, h: u32) {
    let Some(siege) = ui.siege_hud.as_ref() else { return; };

    if let Some(banner) = siege.banner.as_deref() {
        centered_text(out, banner, h as f32 * 0.19, 1.55, [1.0, 1.0, 1.0, 1.0], w, h);
    }

    if let Some(timer) = siege.timer.as_deref() {
        centered_text(out, timer, h as f32 * 0.075, 1.75, siege.timer_color, w, h);
    }

    if let Some(respawn) = siege.respawn.as_deref() {
        if let Some(menu) = siege.mp_timer.as_ref() {
            draw_mp_timer(out, respawn, menu, &ui.extension_images, font, w, h);
        } else if !siege.vote_delay {
            // Other Siege status still has its own separate text presentation.
            centered_text(out, respawn, h as f32 * 0.66, 1.35, [1.0, 0.78, 0.28, 1.0], w, h);
        }
    }

    let Some(stat) = siege.stat_viewer.as_ref() else { return; };
    let scale = (h as f32 / 1080.0).clamp(0.72, 2.0);
    let panel_w = 250.0 * scale;
    let panel_h = 74.0 * scale;
    let x = (w as f32 - panel_w) * 0.5;
    let y = h as f32 * 0.56;
    rect(out, x, y, panel_w, panel_h, [0.015, 0.02, 0.03, 0.80], w, h);
    rect_outline(out, x, y, panel_w, panel_h, (1.0 * scale).max(1.0), [0.45, 0.65, 0.82, 0.70], w, h);

    let name = stat.name.chars().take(24).collect::<String>();
    fixed_charset_text(out, &name, x + 10.0 * scale, y + 8.0 * scale, 7.0 * scale, 10.0 * scale, 7.0 * scale, [1.0; 4], true, w, h);

    let health_max = stat.max_health.max(1);
    let health_frac = (stat.health.max(0) as f32 / health_max as f32).clamp(0.0, 1.0);
    let bar_x = x + 10.0 * scale;
    let bar_w = panel_w - 20.0 * scale;
    let bar_h = 10.0 * scale;
    let health_y = y + 31.0 * scale;
    rect(out, bar_x, health_y, bar_w, bar_h, [0.18, 0.18, 0.18, 0.90], w, h);
    rect(out, bar_x, health_y, bar_w * health_frac, bar_h, [0.28, 0.88, 0.36, 0.95], w, h);
    let health = format!("HP {}/{}", stat.health.max(0), health_max);
    fixed_charset_text(out, &health, bar_x, health_y + 12.0 * scale, 5.5 * scale, 8.0 * scale, 5.5 * scale, [0.90, 0.94, 0.98, 1.0], true, w, h);

    if stat.ammo >= 0 && stat.ammo_max > 0 {
        let ammo_frac = (stat.ammo as f32 / stat.ammo_max as f32).clamp(0.0, 1.0);
        let ammo_y = y + 55.0 * scale;
        rect(out, bar_x, ammo_y, bar_w, bar_h, [0.18, 0.18, 0.18, 0.90], w, h);
        rect(out, bar_x, ammo_y, bar_w * ammo_frac, bar_h, [0.30, 0.62, 1.0, 0.95], w, h);
    }
}

#[cfg(test)]
mod mp_timer_tests {
    use super::*;

    #[test]
    fn menu_definition_is_authoritative_not_a_hardcoded_timer() {
        let menu = parse_mp_timer(r#"
            menuDef { name "unrelated" itemDef { name "frame" rect 0 0 10 10 } }
            menuDef { name "mp_timer" rect 0 0 640 480
                itemDef { name "frame" rect 536 42 80 30
                    background "gfx/hud/timer_frame" forecolor 1 0.5 0.4 0.7 }
                itemDef { name "deathtimer" rect 557 47 0 0
                    forecolor 0.2 0.8 1 1 }
            }
        "#).expect("parse authored HUD");
        assert_eq!(menu.frame_rect, [536.0, 42.0, 80.0, 30.0]);
        assert_eq!(menu.frame_shader, "gfx/hud/timer_frame");
        assert_eq!(menu.text_rect[0], 557.0);
        assert_eq!(menu.text_color, [0.2, 0.8, 1.0, 1.0]);
    }
    #[test]
    fn mp_timer_item_rectangles_include_parent_menu_origin() {
        // Child itemDef coordinates in JKA are relative to menuDef.
        let menu = parse_mp_timer(r#"
            menuDef { name "mp_timer" rect 490 280 130 55
                itemDef { name "frame" rect 0 0 80 30
                    background "gfx/hud/timer_frame" forecolor 1 1 1 1 }
                itemDef { name "deathtimer" rect 15 8 0 0
                    forecolor 0 1 0 1 }
            }
        "#).unwrap();
        assert_eq!(menu.frame_rect, [490.0, 280.0, 80.0, 30.0]);
        assert_eq!(menu.text_rect, [505.0, 288.0, 0.0, 0.0]);
    }
    #[test]
    fn missing_stock_menu_is_not_silently_replaced_with_invented_layout() {
        assert!(parse_mp_timer("menuDef { name unrelated }").is_none());
    }
}
