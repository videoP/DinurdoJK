//! Hud movement.
use crate::extensions::NativeHud;
use crate::ui::{
    color_code, draw_placed, fixed_charset_text, hud_line, proportional_text,
    proportional_text_width, rect, rect_outline, text, textured_rect_with_source, HudElementId,
    MovementKeysSettings, ProportionalFont, UiSnapshot, UiVertex, CROSSHAIR_IMAGE_COUNT,
    CROSSHAIR_IMAGE_NAMES, KEY_ART_SIZE, KEY_ATLAS_COLUMNS, KEY_ATLAS_ROWS, KEY_TEXTURE_SOURCE,
    SHELPER_XONOTIC, SHELPER_Q3DF, SHELPER_WARSOW_HUD, SHELPER_COMPONENT_MASK,
};
#[cfg(test)]
use crate::ui::SHELPER_UPDATED;

/// jaPRO `DF_RaceTimer`: `CG_Text_Paint(x, y, size, ..)` at the configured
/// 640x480 position, shadowed, for the timer block and the start-speed line.
/// Clamp a colour the speedometer derived from `1 / ratio^2` (infinite when the
/// reference speed is zero) into a drawable range.
pub(in crate::ui) fn speedometer_color(color: [f32; 4]) -> [f32; 4] {
    color.map(|channel| {
        if channel.is_nan() {
            1.0
        } else {
            channel.clamp(0.0, 1.0)
        }
    })
}

/// jaPRO `cg_speedometer`: text at the 640x480 positions `DF_DrawSpeedometer`
/// and friends pick, in the same shadowed font as the race timer.
/// Both native jaPRO text and the Q3DF provider use one saved HUD layout.
/// Map their actual glyph bounds to the same stock rectangle; this also keeps
/// switching between providers from jumping to the center of the viewport.
fn position_speedometer_batch(
    batch: &mut [UiVertex], ui: &UiSnapshot, w: u32, h: u32,
    native_font_aspect: bool,
) {
    if batch.is_empty() || w == 0 || h == 0 { return; }
    let ctx = crate::ui::HudRectContext::from_snapshot(ui);
    let stock = crate::ui::hud_stock_rect(HudElementId::Speedometer, &ctx, w, h);
    let target_x = (stock.x + stock.width * 0.5) / w as f32 * 2.0 - 1.0;
    let target_y = 1.0 - (stock.y + stock.height * 0.5) / h as f32 * 2.0;
    let min_x = batch.iter().map(|v| v.position[0]).fold(f32::INFINITY, f32::min);
    let max_x = batch.iter().map(|v| v.position[0]).fold(f32::NEG_INFINITY, f32::max);
    let min_y = batch.iter().map(|v| v.position[1]).fold(f32::INFINITY, f32::min);
    let max_y = batch.iter().map(|v| v.position[1]).fold(f32::NEG_INFINITY, f32::max);
    let center_x = (min_x + max_x) * 0.5;
    let center_y = (min_y + max_y) * 0.5;
    let x_aspect = if native_font_aspect { 2.0 * (h as f32 / 480.0) / (w as f32 / 640.0) } else { 1.0 };
    for v in batch {
        v.position[0] = target_x + (v.position[0] - center_x) * x_aspect;
        v.position[1] = target_y + (v.position[1] - center_y);
    }
}

pub(in crate::ui) fn build_speedometer(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    let Some(speedo) = ui.speedometer.as_ref() else {
        return;
    };
    if ui.native_replaced(NativeHud::Speedometer) {
        return;
    }
    let (sx, sy) = (w as f32 / 640.0, h as f32 / 480.0);
    let paint = |out: &mut Vec<UiVertex>, item: &crate::speedometer::Text| {
        let (px, py) = (item.x * sx, item.y * sy);
        let color = speedometer_color(item.color);
        if let Some(font) = small_font {
            // The acceleration label uses the legacy font's 0xB5 glyph.
            proportional_text(
                out,
                &item.text,
                font,
                px,
                py,
                speedo.size * 0.78,
                color,
                true,
                w,
                h,
            );
        } else {
            let ascii: String = item
                .text
                .chars()
                .map(|c| if c == '\u{b5}' { 'u' } else { c })
                .collect();
            text(out, &ascii, px, py, speedo.size * 1.4, color, w, h);
        }
    };
    let fill = |out: &mut Vec<UiVertex>, item: &crate::speedometer::Rect| {
        let color = speedometer_color(item.color);
        let (x, y, rw, rh) = (item.x * sx, item.y * sy, item.w * sx, item.h * sy);
        match item.outline {
            Some(thickness) => {
                rect_outline(out, x, y, rw, rh, (thickness * sy).max(1.0), color, w, h)
            }
            None => rect(out, x, y, rw, rh, color, w, h),
        }
    };
    for (id, texts, rects) in &speedo.components {
        // Gate cached data using the current selection, never the flags that
        // happened to build the previous frame's speedometer batch.
        if crate::ui::movement_components::selected(*id, ui.strafe_helper.flags, ui.speedometer_settings.flags) != 1 { continue; }
        draw_placed(out, ui, *id, w, h, |out| {
            let start = out.len();
            for item in &speedo.rects[rects.clone()] { fill(out, item); }
            for item in &speedo.texts[texts.clone()] { paint(out, item); }
            if *id == HudElementId::Speedometer && out.len() > start {
                // Keep the fonts square in widescreen and center the drawn
                // speedometer inside the editor's *configured* 640x480 stock
                // rectangle rather than forcibly placing it mid-screen.
                position_speedometer_batch(&mut out[start..], ui, w, h, true);
            }
        });
    }
    if crate::ui::movement_components::selected(HudElementId::SpeedometerJumps, ui.strafe_helper.flags, ui.speedometer_settings.flags) != 0 {
    draw_placed(out, ui, HudElementId::SpeedometerJumps, w, h, |out| {
        for item in &speedo.jump_texts {
            paint(out, item);
        }
    });
    }
    if crate::ui::movement_components::selected(HudElementId::SpeedGraph, ui.strafe_helper.flags, ui.speedometer_settings.flags) != 0 {
    draw_placed(out, ui, HudElementId::SpeedGraph, w, h, |out| {
        // The graph is independent of the numeric speedometer.
        for pic in &speedo.graph_pics {
            lagometer_pic(out, pic, ui, w, h);
        }
        for item in &speedo.graph_rects {
            fill(out, item);
        }
    });
    }
}

/// Image names of the icon atlas (see `renderer::load_ui_icon_atlas`): the two
/// lagometer images in `crate::lagometer::Icon` order, then the image crosshairs
/// from `ICON_CROSSHAIR_BASE` in `CROSSHAIR_IMAGE_NAMES` order.
pub const FORCE_ICON_NAMES: [&str; 18] = [
    "gfx/mp/f_icon_lt_heal",
    "gfx/mp/f_icon_levitation",
    "gfx/mp/f_icon_speed",
    "gfx/mp/f_icon_push",
    "gfx/mp/f_icon_pull",
    "gfx/mp/f_icon_lt_telepathy",
    "gfx/mp/f_icon_dk_grip",
    "gfx/mp/f_icon_dk_l1",
    "gfx/mp/f_icon_dk_rage",
    "gfx/mp/f_icon_lt_protect",
    "gfx/mp/f_icon_lt_absorb",
    "gfx/mp/f_icon_lt_healother",
    "gfx/mp/f_icon_dk_forceother",
    "gfx/mp/f_icon_dk_drain",
    "gfx/mp/f_icon_sight",
    "gfx/mp/f_icon_saber_attack",
    "gfx/mp/f_icon_saber_defend",
    "gfx/mp/f_icon_saber_throw",
];

// TaystJK CG_Init (cg_main.c): BG_FindItemForWeapon(i)->icon in WP_* order.
// The paths match the pinned OpenJK bg_itemlist and retain their original artwork.
pub const WEAPON_ICON_NAMES: [&str; 17] = [
    "", // WP_NONE (not a selectable weapon)
    "gfx/hud/w_icon_stunbaton",
    "gfx/hud/w_icon_melee",
    "gfx/hud/w_icon_lightsaber",
    "gfx/hud/w_icon_blaster_pistol",
    "gfx/hud/w_icon_blaster",
    "gfx/hud/w_icon_disruptor",
    "gfx/hud/w_icon_bowcaster",
    "gfx/hud/w_icon_repeater",
    "gfx/hud/w_icon_demp2",
    "gfx/hud/w_icon_flechette",
    "gfx/hud/w_icon_merrsonn",
    "gfx/hud/w_icon_thermal",
    "gfx/hud/w_icon_tripmine",
    "gfx/hud/w_icon_detpack",
    "gfx/hud/w_icon_c_rifle",
    "gfx/hud/w_icon_briar",
];

pub(in crate::ui) const ICON_WEAPON_BASE: usize = ICON_FORCE_BASE + FORCE_ICON_NAMES.len();
pub(crate) const ICON_WEAPON_NO_AMMO_BASE: usize = ICON_WEAPON_BASE + WEAPON_ICON_NAMES.len();
pub(crate) const ICON_WEAPON_STAFF: usize = ICON_WEAPON_NO_AMMO_BASE + WEAPON_ICON_NAMES.len();
pub(in crate::ui) const ICON_WEAPON_DUAL: usize = ICON_WEAPON_STAFF + 1;

// TaystJK CG_Init cgs.media.invenIcons[tag] uses the IT_HOLDABLE
// bg_itemlist icon for each HI_* tag. Pack once with the other native art.
// Index zero (HI_NONE) is intentionally empty.
pub const INVENTORY_ICON_NAMES: [&str; 12] = [
    "",
    "gfx/hud/i_icon_seeker",
    "gfx/hud/i_icon_shieldwall",
    "gfx/hud/i_icon_bacta",
    "gfx/hud/i_icon_big_bacta",
    "gfx/hud/i_icon_zoom",
    "gfx/hud/i_icon_sentrygun",
    "gfx/hud/i_icon_jetpack",
    "gfx/hud/i_icon_healthdisp",
    "gfx/hud/i_icon_ammodisp",
    "gfx/hud/i_icon_eweb",
    "gfx/hud/i_icon_cloak",
];
pub(crate) const ICON_INVENTORY_BASE: usize = ICON_WEAPON_DUAL + 1;

// TaystJK CG_DrawZoomMask and CG_DrawRocketLocking artwork. Appending to
// the native-resolution atlas preserves every existing icon index.
pub(crate) const ICON_COMBAT_BASE: usize = ICON_INVENTORY_BASE + INVENTORY_ICON_NAMES.len();
pub const COMBAT_ICON_NAMES: [&str; 12] = [
    "gfx/2d/cropCircle2", "gfx/2d/cropCircle", "gfx/2d/insertTick",
    "gfx/2d/crop_charge", "gfx/2d/binMask", "gfx/2d/binocularWindow",
    "gfx/2d/binSideArrow", "gfx/2d/wedge", "gfx/2d/lock",
    "gfx/2d/binCircle", "gfx/2d/binTopTri", "gfx/2d/binocularNumOverlay",
];
// Maintain the old atlas indices for crosshair/force icons, so existing HUD
// paths don't change; NA cells use the corresponding qpath + "_na" at load time.
pub const ICON_NAMES: [&str; ICON_COMBAT_BASE + COMBAT_ICON_NAMES.len()] = {
    let mut names = [""; ICON_COMBAT_BASE + COMBAT_ICON_NAMES.len()];
    names[0] = "gfx/2d/lag";
    names[1] = "gfx/2d/net";
    let mut i = 0;
    while i < CROSSHAIR_IMAGE_NAMES.len() {
        names[ICON_CROSSHAIR_BASE + i] = CROSSHAIR_IMAGE_NAMES[i];
        i += 1;
    }
    i = 0;
    while i < FORCE_ICON_NAMES.len() {
        names[ICON_FORCE_BASE + i] = FORCE_ICON_NAMES[i];
        i += 1;
    }
    i = 0;
    while i < WEAPON_ICON_NAMES.len() {
        names[ICON_WEAPON_BASE + i] = WEAPON_ICON_NAMES[i];
        names[ICON_WEAPON_NO_AMMO_BASE + i] = WEAPON_ICON_NAMES[i];
        i += 1;
    }
    names[ICON_WEAPON_STAFF] = "gfx/hud/w_icon_saberstaff";
    names[ICON_WEAPON_DUAL] = "gfx/hud/w_icon_duallightsaber";
    i = 0;
    while i < INVENTORY_ICON_NAMES.len() {
        names[ICON_INVENTORY_BASE + i] = INVENTORY_ICON_NAMES[i];
        i += 1;
    }
    i = 0;
    while i < COMBAT_ICON_NAMES.len() {
        names[ICON_COMBAT_BASE + i] = COMBAT_ICON_NAMES[i];
        i += 1;
    }
    names
};

/// Atlas cell of the first image crosshair.
pub(in crate::ui) const ICON_CROSSHAIR_BASE: usize = 2;

pub(in crate::ui) const ICON_FORCE_BASE: usize = 2 + CROSSHAIR_IMAGE_COUNT as usize;

/// Legacy fixed-grid UV fallback used only before the renderer has lazily
/// loaded the native-resolution atlas (and by standalone HUD unit tests).
/// Runtime icons use per-image UVs and retain their original pixel sizes.
pub const ICON_CELL: u32 = 256;

/// Pack icon cells in rows: a single row at 256px per icon would exceed the
/// max 2D texture dimension on many GPUs. Both the CPU atlas writer and the
/// UV calculation below must use the same column count.
pub const ICON_COLUMNS: u32 = 8;
pub const ICON_ROWS: u32 = (ICON_NAMES.len() as u32 + ICON_COLUMNS - 1) / ICON_COLUMNS;

/// Texture source id of the icon atlas in `ui.wgsl`.
pub(in crate::ui) const ICON_TEXTURE_SOURCE: f32 = 5.0;

/// Lazily rebuilt team-overlay icon atlas (`ui.wgsl` texture source 6).
pub const TEAM_ICON_CELL: u32 = 64;

pub const TEAM_ICON_COLUMNS: u32 = 8;

pub(in crate::ui) const TEAM_ICON_TEXTURE_SOURCE: f32 = 6.0;

/// The whole of atlas cell `index` as (uv0, uv1).
pub(in crate::ui) fn icon_cell_uv(index: usize) -> ([f32; 2], [f32; 2]) {
    let cell = ICON_CELL as f32;
    let col = (index as u32 % ICON_COLUMNS) as f32;
    let row = (index as u32 / ICON_COLUMNS) as f32;
    let atlas_w = ICON_COLUMNS as f32 * cell;
    let atlas_h = ICON_ROWS as f32 * cell;
    // Half-texel inset prevents linear filtering across adjacent atlas cells.
    let u0 = (col * cell + 0.5) / atlas_w;
    let u1 = ((col + 1.0) * cell - 0.5) / atlas_w;
    let v0 = (row * cell + 0.5) / atlas_h;
    let v1 = ((row + 1.0) * cell - 0.5) / atlas_h;
    ([u0, v0], [u1, v1])
}

/// `CG_DrawPic` of a lagometer image: the whole image, white, alpha blended.
pub(in crate::ui) fn lagometer_pic(
    out: &mut Vec<UiVertex>,
    pic: &crate::lagometer::Pic,
    ui: &UiSnapshot,
    w: u32,
    h: u32,
) {
    let index = match pic.icon {
        crate::lagometer::Icon::Lag => 0,
        crate::lagometer::Icon::Net => 1,
    };
    let (uv0, uv1) = ui.icon_uv(index);
    let (sx, sy) = (w as f32 / 640.0, h as f32 / 480.0);
    textured_rect_with_source(
        out,
        pic.x * sx,
        pic.y * sy,
        pic.w * sx,
        pic.h * sy,
        uv0,
        uv1,
        [1.0; 4],
        ICON_TEXTURE_SOURCE,
        w,
        h,
    );
}

/// `CG_Text_Paint(.., 0.5, colorWhite, .., ITEM_TEXTSTYLE_SHADOWEDMORE, FONT_SMALL)`; a
/// right-aligned item subtracts `CG_Text_Width` from its x first.
pub(in crate::ui) fn lagometer_text(
    out: &mut Vec<UiVertex>,
    item: &crate::lagometer::Text,
    small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    const SCALE: f32 = 0.5;
    let (sx, sy) = (w as f32 / 640.0, h as f32 / 480.0);
    if let Some(font) = small_font {
        let x = if item.right {
            item.x - proportional_text_width(&item.text, font, SCALE)
        } else {
            item.x
        };
        proportional_text(
            out,
            &item.text,
            font,
            x * sx,
            item.y * sy,
            SCALE,
            [1.0; 4],
            true,
            w,
            h,
        );
    } else {
        // Asset fallback only (see build_chat_history): the fixed charset at a small size.
        const GLYPH_W: f32 = 5.0;
        let x = if item.right {
            item.x - visible_jka_chars(&item.text) as f32 * GLYPH_W
        } else {
            item.x
        };
        fixed_charset_text(
            out,
            &item.text,
            x * sx,
            item.y * sy,
            GLYPH_W * sx,
            8.0 * sy,
            GLYPH_W * sx,
            [1.0; 4],
            true,
            w,
            h,
        );
    }
}

/// jaPRO `CG_DrawLagometer` / `CG_DrawDisconnect`: the graph in the lag frame, its
/// numbers, and the "Connection Interrupted" warning, at the 640x480 positions the
/// draw list carries (no drop of `widthRatioCoef`, as for the speedometer).
pub(in crate::ui) fn build_lagometer(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    let Some(lag) = ui.lagometer.as_ref() else {
        return;
    };
    let (sx, sy) = (w as f32 / 640.0, h as f32 / 480.0);

    // The graph/numbers are a normal editable HUD item. Connection-interrupted
    // and map-change warnings stay screen-centered exactly like JKA.
    draw_placed(out, ui, HudElementId::Lagometer, w, h, |out| {
        for pic in &lag.pics[..lag.graph_pic_count.min(lag.pics.len())] {
            lagometer_pic(out, pic, ui, w, h);
        }
        for bar in &lag.rects {
            rect(
                out,
                bar.x * sx,
                bar.y * sy,
                bar.w * sx,
                bar.h * sy,
                bar.color,
                w,
                h,
            );
        }
        for item in &lag.texts {
            lagometer_text(out, item, small_font, w, h);
        }
        for item in &lag.big_texts[..lag.graph_big_text_count.min(lag.big_texts.len())] {
            fixed_charset_text(
                out,
                &item.text,
                item.x * sx,
                item.y * sy,
                item.char_w * sx,
                crate::lagometer::BIGCHAR * sy,
                item.char_w * sx,
                [1.0; 4],
                true,
                w,
                h,
            );
        }
    });

    for pic in &lag.pics[lag.graph_pic_count.min(lag.pics.len())..] {
        lagometer_pic(out, pic, ui, w, h);
    }
    for item in &lag.big_texts[lag.graph_big_text_count.min(lag.big_texts.len())..] {
        fixed_charset_text(
            out,
            &item.text,
            item.x * sx,
            item.y * sy,
            item.char_w * sx,
            crate::lagometer::BIGCHAR * sy,
            item.char_w * sx,
            [1.0; 4],
            true,
            w,
            h,
        );
    }
}

pub(in crate::ui) fn build_race_timer(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    let Some(race) = ui.race_timer.as_ref() else {
        return;
    };
    let paint = |out: &mut Vec<UiVertex>, value: &str, x: f32, y: f32, color: [f32; 4]| {
        if value.is_empty() {
            return;
        }
        let px = x * w as f32 / 640.0;
        let py = y * h as f32 / 480.0;
        if let Some(font) = small_font {
            // 0.78 maps the medium-font role onto the shared OCR font (see build_center_print).
            proportional_text(
                out,
                value,
                font,
                px,
                py,
                race.size * 0.78,
                color,
                true,
                w,
                h,
            );
        } else {
            text(out, value, px, py, race.size * 1.4, color, w, h);
        }
    };
    if ui.race_components[0] {
    draw_placed(out, ui, HudElementId::RaceTimer, w, h, |out| {
        paint(
            out,
            &race.timer_text,
            race.timer_x,
            race.timer_y,
            [1.0, 1.0, 1.0, 1.0],
        );
    });
    }
    let [r, g, b] = race.start_color;
    if ui.race_components[1] {
    draw_placed(out, ui, HudElementId::RaceStart, w, h, |out| {
        paint(
            out,
            &race.start_text,
            race.start_x,
            race.start_y,
            [r, g, b, 1.0],
        );
    });
    }
}

pub(crate) fn visible_jka_chars(value: &str) -> usize {
    let mut chars = value.chars().peekable();
    let mut visible = 0usize;
    while let Some(ch) = chars.next() {
        if ch == '^' {
            if let Some(&code) = chars.peek() {
                if color_code(code, 1.0).is_some() {
                    let _ = chars.next();
                    continue;
                }
            }
        }
        visible += 1;
    }
    visible
}

pub(in crate::ui) fn center_print_lines(value: &str) -> Vec<String> {
    // OpenJK's CG_DrawCenterString wraps each source line at 50 characters and
    // prefers a whitespace break when possible. Keep color escapes zero-width.
    let mut lines = Vec::new();
    for source in value.split('\n') {
        let source = source.trim_end_matches('\r');
        if source.is_empty() {
            lines.push(String::new());
            continue;
        }
        let chars: Vec<char> = source.chars().collect();
        let mut start = 0usize;
        while start < chars.len() {
            let mut index = start;
            let mut visible = 0usize;
            let mut last_space = None;
            while index < chars.len() && visible < 50 {
                if chars[index] == '^'
                    && index + 1 < chars.len()
                    && color_code(chars[index + 1], 1.0).is_some()
                {
                    index += 2;
                    continue;
                }
                if chars[index].is_whitespace() {
                    last_space = Some(index);
                }
                index += 1;
                visible += 1;
            }
            if index >= chars.len() {
                let tail: String = chars[start..].iter().collect();
                lines.push(tail.trim().to_owned());
                break;
            }
            let cut = last_space.filter(|space| *space > start).unwrap_or(index);
            let head: String = chars[start..cut].iter().collect();
            lines.push(head.trim_end().to_owned());
            start = cut;
            while start < chars.len() && chars[start].is_whitespace() {
                start += 1;
            }
        }
    }
    lines
}

#[derive(Clone, Copy)]
#[repr(usize)]
pub(in crate::ui) enum KeyArt {
    CrouchOff,
    CrouchOn,
    JumpOff,
    JumpOn,
    BackOff,
    BackOn,
    ForwardOff,
    ForwardOn,
    LeftOff,
    LeftOn,
    RightOff,
    RightOn,
    AttackOff,
    AttackOn,
    AltOff,
    AltOn,
    WalkOff,
    WalkOn,
    CrouchOn2,
    JumpOn2,
    BackOn2,
    ForwardOn2,
    LeftOn2,
    RightOn2,
    AttackOn2,
    AltOn2,
    WalkOn2,
}

/// `CG_DrawPic` for one key image: the whole 128x128 image, white, alpha blended.
pub(in crate::ui) fn movement_key_pic(
    out: &mut Vec<UiVertex>,
    art: KeyArt,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    width: u32,
    height: u32,
) {
    let index = art as u32;
    let (column, row) = (index % KEY_ATLAS_COLUMNS, index / KEY_ATLAS_COLUMNS);
    let (atlas_w, atlas_h) = (
        (KEY_ATLAS_COLUMNS * KEY_ART_SIZE) as f32,
        (KEY_ATLAS_ROWS * KEY_ART_SIZE) as f32,
    );
    // Half-texel inset keeps linear filtering from pulling in the neighbouring
    // atlas cell; it stands in for the clamp-to-edge of a standalone image.
    let u0 = ((column * KEY_ART_SIZE) as f32 + 0.5) / atlas_w;
    let u1 = (((column + 1) * KEY_ART_SIZE) as f32 - 0.5) / atlas_w;
    let v0 = ((row * KEY_ART_SIZE) as f32 + 0.5) / atlas_h;
    let v1 = (((row + 1) * KEY_ART_SIZE) as f32 - 0.5) / atlas_h;
    textured_rect_with_source(
        out,
        x,
        y,
        w,
        h,
        [u0, v0],
        [u1, v1],
        [1.0; 4],
        KEY_TEXTURE_SOURCE,
        width,
        height,
    );
}

/// Port of TaystJK `DF_DrawMovementKeys`. Layout is in cgame's 640x480 space with
/// `cl_ratioFix` on (the default): image sizes come out square, x offsets scale
/// with the window width and y offsets with its height.
/// Tile edge in pixels (`w * widthRatioCoef` and `h`, scaled to the window) and
/// the top-left corner of the 3-wide key block; `None` while the keys are off.
pub(in crate::ui) fn movement_keys_origin(
    settings: &MovementKeysSettings,
    w: u32,
    h: u32,
) -> Option<(f32, f32, f32)> {
    if settings.mode == 0 || w == 0 || h == 0 {
        return None;
    }
    let (sx, sy) = (w as f32 / 640.0, h as f32 / 480.0);
    let size = settings.size;
    let walk = settings.walk;
    Some(match settings.mode {
        1 => {
            let tile = 16.0 * size * sy;
            (
                tile,
                320.0 * sx + settings.x * sx - tile * if walk { 1.0 } else { 1.5 },
                480.0 * 0.9 * sy + settings.y * sy - tile,
            )
        }
        2 => {
            let tile = 16.0 * size * sy;
            (
                tile,
                320.0 * sx + settings.x * sx - tile * if walk { 1.5 } else { 2.0 },
                480.0 * 0.9 * sy + settings.y * sy - tile,
            )
        }
        3 => {
            // TaystJK ignores cg_movementKeysX/Y in this mode.
            let tile = 6.0 * size * sy;
            (tile, 320.0 * sx - tile * 1.5, 240.0 * sy - tile * 1.5)
        }
        4 => {
            let tile = 12.0 * size * sy;
            (
                tile,
                320.0 * sx + settings.x * sx - tile * 1.5,
                480.0 * 0.9 * sy + settings.y * sy - tile * 1.5,
            )
        }
        _ => return None,
    })
}

pub(in crate::ui) fn build_movement_keys(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let settings = ui.movement_keys;
    let Some((tile, x, y)) = movement_keys_origin(&settings, w, h) else {
        return;
    };
    let state = ui.movement_hud;
    let walk = settings.walk;
    let (tw, th) = (tile, tile);

    let forward = state.forward_move;
    let right = state.right_move;
    let up = state.up_move;
    let attack = state.buttons & jka_movement::BUTTON_ATTACK != 0;
    let alt = state.buttons & jka_movement::BUTTON_ALT_ATTACK != 0;
    let walking = state.buttons & jka_movement::BUTTON_WALKING != 0;
    let mut pic = |art: KeyArt, col: f32, row: f32| {
        movement_key_pic(out, art, x + col * tw, y + row * th, tw, th, w, h);
    };

    if settings.mode >= 3 {
        // Compact style: only pressed keys are drawn, each with its "2" art.
        if up < 0 {
            pic(KeyArt::CrouchOn2, 2.0, 0.0);
        }
        if up > 0 {
            pic(KeyArt::JumpOn2, 0.0, 0.0);
        }
        if forward < 0 {
            pic(KeyArt::BackOn2, 1.0, 2.0);
        }
        if forward > 0 {
            pic(KeyArt::ForwardOn2, 1.0, 0.0);
        }
        if right < 0 {
            pic(KeyArt::LeftOn2, 0.0, 1.0);
        }
        if right > 0 {
            pic(KeyArt::RightOn2, 2.0, 1.0);
        }
        if attack {
            pic(KeyArt::AttackOn2, 0.0, 2.0);
        }
        if alt {
            pic(KeyArt::AltOn2, 2.0, 2.0);
        }
        if walk && walking {
            pic(KeyArt::WalkOn2, -1.0, 2.0);
        }
    } else {
        // Original style: every key is drawn, in its on or off art.
        pic(
            if up < 0 {
                KeyArt::CrouchOn
            } else {
                KeyArt::CrouchOff
            },
            2.0,
            0.0,
        );
        pic(
            if up > 0 {
                KeyArt::JumpOn
            } else {
                KeyArt::JumpOff
            },
            0.0,
            0.0,
        );
        pic(
            if forward < 0 {
                KeyArt::BackOn
            } else {
                KeyArt::BackOff
            },
            1.0,
            1.0,
        );
        pic(
            if forward > 0 {
                KeyArt::ForwardOn
            } else {
                KeyArt::ForwardOff
            },
            1.0,
            0.0,
        );
        pic(
            if right < 0 {
                KeyArt::LeftOn
            } else {
                KeyArt::LeftOff
            },
            0.0,
            1.0,
        );
        pic(
            if right > 0 {
                KeyArt::RightOn
            } else {
                KeyArt::RightOff
            },
            2.0,
            1.0,
        );
        if settings.mode == 2 {
            pic(
                if attack {
                    KeyArt::AttackOn
                } else {
                    KeyArt::AttackOff
                },
                3.0,
                0.0,
            );
            pic(if alt { KeyArt::AltOn } else { KeyArt::AltOff }, 3.0, 1.0);
        }
        if walk {
            pic(
                if walking {
                    KeyArt::WalkOn
                } else {
                    KeyArt::WalkOff
                },
                -1.0,
                1.0,
            );
        }
    }
}

pub(in crate::ui) fn build_strafe_helper(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    use crate::ui::movement_components as components;
    let sh = ui.strafe_helper;
    let native = ui.speedometer_settings.flags;
    let guide = components::selected(HudElementId::StrafeGuide, sh.flags, native);
    let aspect = w.max(1) as f32 / h.max(1) as f32;
    let mut state = ui.movement_hud;
    if ui.mode == crate::ui::OverlayMode::HudEdit {
        // Preview the selected style even while standing still or spectating.
        state = crate::ui::MovementHudState { velocity:[755.0,0.0,0.0],
            forward_move:127, right_move:-127, q3df_accel_bar_width:24.0,
            fov_x:90.0, ..crate::ui::MovementHudState::default() };
    }
    for (id, flags) in [
        (HudElementId::StrafeGuide, guide),
        (HudElementId::Speedometer, if components::selected(HudElementId::Speedometer, sh.flags, native) == 2 {crate::ui::SHELPER_Q3DF_SPEED} else {0}),
        (HudElementId::AccelMeter, if components::selected(HudElementId::AccelMeter, sh.flags, native) == 2 {crate::ui::SHELPER_Q3DF_ACCEL} else {0}),
        (HudElementId::VelocityCompass, sh.flags & crate::ui::SHELPER_Q3DF_COMPASS),
    ] {
        if flags == 0 { continue; }
        if id.native().is_some_and(|native| ui.native_replaced(native)) { continue; }
        let part = crate::ui::StrafeHelperSettings { flags: flags | (sh.flags & (crate::ui::SHELPER_XONOTIC_VELOCITY | crate::ui::SHELPER_XONOTIC_PERSPECTIVE | crate::ui::SHELPER_XONOTIC_PANORAMIC)), ..sh };
        if id == HudElementId::StrafeGuide && flags & (SHELPER_XONOTIC | SHELPER_Q3DF | SHELPER_WARSOW_HUD) == 0 {
            let part = crate::ui::StrafeHelperSettings {flags: (sh.flags & !crate::ui::SHELPER_STYLE_MASK & !SHELPER_COMPONENT_MASK) | guide, ..sh};
            let segments = crate::strafehelper::strafe_lines(&part, &state, ui.video.fps_cap, aspect);
            draw_placed(out, ui, id, w, h, |out| {
                let (sx,sy) = (w as f32/640.0,h as f32/480.0);
                for segment in segments {
                    let (x0,y0,x1,y1) = (segment.from[0]*sx,segment.from[1]*sy,segment.to[0]*sx,segment.to[1]*sy);
                    let length = (x1-x0).hypot(y1-y0);
                    if length <= f32::EPSILON {continue;}
                    let (nx,ny) = (-(y1-y0)/length,(x1-x0)/length);
                    hud_line(out,x0,y0,x1,y1,segment.size*(sx*nx.abs()+sy*ny.abs()),segment.color,w,h);
                }
            });
        } else {
            let hud = crate::strafehelper::extended_hud(&part, &state, aspect);
            draw_placed(out, ui, id, w, h, |out| {
                let start = out.len();
                draw_extended(out, hud, w, h);
                if id == HudElementId::Speedometer && out.len() > start {
                    position_speedometer_batch(&mut out[start..], ui, w, h, false);
                }
            });
        }
    }
}

fn draw_extended(out: &mut Vec<UiVertex>, hud: crate::strafehelper::ExtendedHud, w:u32, h:u32) {
    let (sx,sy) = (w as f32/640.0,h as f32/480.0);
    for triangle in hud.triangles {
        for [x,y] in triangle.points { out.push(UiVertex { position:[2.0*x/640.0-1.0,1.0-2.0*y/480.0], uv:[0.0,0.0], color:triangle.color,textured:0.0 }); }
    }
    for fill in hud.fills { rect(out,fill.x*sx,fill.y*sy,fill.w*sx,fill.h*sy,fill.color,w,h); }
    for label in hud.labels {
        // Keep glyph aspect consistent at widescreen resolutions. Positions
        // remain centered in the authored HUD space; text scales uniformly.
        let x = w as f32*0.5 + (label.x-320.0)*sy;
        fixed_charset_text(out,&label.text,x,label.y*sy,label.glyph_w*sy,label.glyph_h*sy,label.advance*sy,label.color,true,w,h);
    }
}

#[cfg(test)]
mod strafehelper_review {
    use super::*;

    #[test]
    fn movement_components_off_suppresses_stale_cached_readouts() {
        let mut ui = UiSnapshot::default();
        let settings = crate::speedometer::Settings {flags:crate::speedometer::flag::ENABLE | crate::speedometer::flag::ACCELMETER, ..Default::default()};
        ui.speedometer = Some(crate::speedometer::preview(&settings));
        let mut vertices = Vec::new();
        build_speedometer(&mut vertices,&ui,None,1280,800);
        assert!(vertices.is_empty());
        ui.strafe_helper.flags = crate::ui::SHELPER_Q3DF_SPEED;
        ui.speedometer_settings = settings;
        build_speedometer(&mut vertices,&ui,None,1280,800);
        // The stale native speed number is suppressed; only native accel remains.
        assert!(vertices.iter().all(|v| v.textured == 0.0));
    }

    #[test]
    fn movement_components_each_extended_component_moves_independently() {
        let mut ui = UiSnapshot::default();
        ui.mode = crate::ui::OverlayMode::HudEdit;
        ui.strafe_helper.flags = SHELPER_Q3DF | crate::ui::SHELPER_COMPONENT_MASK;
        let mut original = Vec::new();
        build_strafe_helper(&mut original,&ui,1280,800);
        ui.hud_layout.speedometer.offset = [73.0,-41.0];
        let mut moved = Vec::new();
        build_strafe_helper(&mut moved,&ui,1280,800);
        assert_eq!(original.len(),moved.len());
        let mut changed = 0;
        for (a,b) in original.iter().zip(&moved) {
            if a.textured == 1.0 {
                assert!((b.position[0]-a.position[0]-73.0*2.0/1280.0).abs()<1e-5);
                assert!((b.position[1]-a.position[1]-41.0*2.0/800.0).abs()<1e-5);
                changed += 1;
            } else { assert_eq!(a.position,b.position); }
        }
        assert!(changed>0);
    }

    #[test]
    fn strafehelper_readouts_do_not_suppress_legacy_lines() {
        let mut ui = UiSnapshot::default();
        ui.movement_hud.velocity = [700.0, 0.0, 0.0];
        ui.movement_hud.forward_move = 127;
        ui.movement_hud.right_move = -127;
        ui.strafe_helper.flags |= SHELPER_UPDATED;
        let mut lines = Vec::new();
        build_strafe_helper(&mut lines, &ui, 1280, 720);
        assert!(!lines.is_empty());
        ui.strafe_helper.flags |= crate::ui::SHELPER_Q3DF_SPEED;
        let mut combined = Vec::new();
        build_strafe_helper(&mut combined, &ui, 1280, 720);
        assert!(combined.len() > lines.len());
        let guide = &combined[..lines.len()];
        for (a, b) in guide.iter().zip(&lines) {
            assert_eq!(a.position, b.position);
            assert_eq!(a.color, b.color);
        }
        ui.strafe_helper.flags = crate::ui::SHELPER_Q3DF_SPEED;
        let mut readout = Vec::new();
        build_strafe_helper(&mut readout, &ui, 1280, 720);
        assert!(!readout.is_empty());
        assert_eq!(readout.len()+lines.len(), combined.len());
    }

    /// Export the actual UI triangle batch, including charset UVs, for a
    /// repeatable visual review without starting or modifying a game session.
    #[test]
    #[ignore = "writes visual-review artifacts; run explicitly"]
    fn strafehelper_export_visual_review() {
        let mut panels = Vec::new();
        let extras = crate::ui::SHELPER_COMPONENT_MASK;
        for (name, flags) in [
            ("Q3DF", SHELPER_Q3DF | extras),
            ("Xonotic", SHELPER_XONOTIC),
            ("Warsow", SHELPER_WARSOW_HUD),
            ("Warsow + independent Q3DF readouts", SHELPER_WARSOW_HUD | extras),
        ] {
            for yaw in [-30.0_f32, 0.0, 30.0] {
                let mut ui = UiSnapshot::default();
                ui.strafe_helper.flags = flags;
                ui.movement_hud.velocity = [755.0, 0.0, 0.0];
                ui.movement_hud.forward_move = 127;
                ui.movement_hud.right_move = if yaw < 0.0 { 127 } else { -127 };
                ui.movement_hud.view_yaw = yaw;
                ui.movement_hud.q3df_accel_bar_width = 32.0;
                let mut batch = Vec::new();
                build_strafe_helper(&mut batch, &ui, 1280, 720);
                let vertices: Vec<_> = batch.iter().map(|v| serde_json::json!({
                    "position": v.position, "uv": v.uv, "color": v.color, "textured": v.textured,
                })).collect();
                panels.push(serde_json::json!({ "style": name, "yaw": yaw,
                    "right": ui.movement_hud.right_move, "width": 1280, "height": 720,
                    "vertices": vertices }));
            }
        }
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../artifacts/strafehelper-review");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("geometry.json"), serde_json::to_vec(&panels).unwrap()).unwrap();
    }
}

#[cfg(test)]
mod icon_atlas_quality_tests {
    use super::*;

    #[test]
    fn every_icon_has_unique_uv_cell_in_two_dimensional_atlas() {
        let mut previous = std::collections::HashSet::new();
        assert!(ICON_NAMES.len() as u32 > ICON_COLUMNS);
        for idx in 0..ICON_NAMES.len() {
            let (min, max) = icon_cell_uv(idx);
            assert!(min[0] >= 0.0 && min[1] >= 0.0);
            assert!(max[0] <= 1.0 && max[1] <= 1.0);
            assert!(min[0] < max[0] && min[1] < max[1]);
            assert!(previous.insert((min[0].to_bits(), min[1].to_bits())));
        }
        assert!(ICON_COLUMNS * ICON_CELL <= 8192);
        assert!(ICON_ROWS * ICON_CELL <= 8192);
    }
}

#[cfg(test)]
mod native_lagometer_icon_uv_tests {
    use super::*;

    #[test]
    fn lagometer_uses_native_atlas_uvs() {
        let mut ui = UiSnapshot::default();
        ui.icon_atlas_uvs = vec![
            ([0.125, 0.25], [0.375, 0.5]),
            ([0.625, 0.125], [0.875, 0.375]),
        ];
        for (icon, index) in [
            (crate::lagometer::Icon::Lag, 0usize),
            (crate::lagometer::Icon::Net, 1),
        ] {
            let mut vertices = Vec::new();
            let pic = crate::lagometer::Pic {
                icon, x: 8.0, y: 16.0, w: 48.0, h: 48.0,
            };
            lagometer_pic(&mut vertices, &pic, &ui, 640, 480);
            assert_eq!(vertices.len(), 6);
            let (min, max) = ui.icon_atlas_uvs[index];
            for vertex in &vertices {
                assert!(vertex.uv[0] == min[0] || vertex.uv[0] == max[0]);
                assert!(vertex.uv[1] == min[1] || vertex.uv[1] == max[1]);
                assert_eq!(vertex.textured, ICON_TEXTURE_SOURCE);
            }
        }
    }
}
