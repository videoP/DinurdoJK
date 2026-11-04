//! Hud movement.
use crate::extensions::NativeHud;
use crate::ui::{
    color_code, draw_placed, fixed_charset_text, hud_line, proportional_text_uniform,
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

/// A fixed source anchor for Q3DF's alternate speed readout, which is
/// produced by the independent strafe-helper renderer at x=266, y=200.  Map
/// it to the editable speedometer origin without inspecting current glyph
/// bounds; 9, 99 and 9999 UPS must all begin at exactly the same pixel.
fn position_q3df_speedometer_batch(batch: &mut [UiVertex], ui: &UiSnapshot, w: u32, h: u32) {
    if batch.is_empty() || w == 0 || h == 0 { return; }
    let ctx = crate::ui::HudRectContext::from_snapshot(ui);
    let stock = crate::ui::hud_stock_rect(HudElementId::Speedometer, &ctx, w, h);
    let sy = h as f32 / 480.0;
    // draw_extended anchors Q3DF labels to the widescreen-safe 4:3 centre.
    let source_x = w as f32 * 0.5 + (crate::strafehelper::Q3DF_SPEED_LEFT_X - 320.0) * sy;
    let source_y = crate::strafehelper::Q3DF_SPEED_TOP_Y * sy;
    let offset_x = (stock.x - source_x) * 2.0 / w as f32;
    let offset_y = (stock.y - source_y) * -2.0 / h as f32;
    for v in batch {
        v.position[0] += offset_x;
        v.position[1] += offset_y;
    }
}

/// Numeric jaPRO movement readouts all use the same typography contract:
/// left inset inside their HUD Edit rectangle, vertically centred on the
/// *digit* line. This is independent of the values (and their color codes),
/// which otherwise cause speed, pre-speed, and yaw to drift when rearranged.
/// The jump history uses one such line per 20-unit row.
fn numeric_readout_position(
    id: HudElementId,
    item: &crate::speedometer::Text,
    settings: &crate::speedometer::Settings,
    stock: crate::ui::HudRect,
    font: Option<&ProportionalFont>,
    pixel_height: f32,
) -> (f32, f32) {
    const LEFT_INSET: f32 = 2.0; // in height-scaled 640x480 units
    const JUMP_ROW_HEIGHT: f32 = 20.0;
    let sy = pixel_height / 480.0;
    let source_x = if id == HudElementId::SpeedometerJumps {
        settings.jumps_x
    } else {
        crate::speedometer::component_origin(id, settings)[0]
    };
    let x = stock.x + LEFT_INSET * sy + (item.x - source_x) * sy;
    let row_offset = if id == HudElementId::SpeedometerJumps {
        (item.y - (settings.jumps_y + 110.0)) * sy
    } else {
        0.0
    };
    let center_y = stock.y + JUMP_ROW_HEIGHT * 0.5 * sy + row_offset;
    let y = if let Some(font) = font {
        // CG_Text_Paint takes a *baseline*, not the top of the glyph. Anchor
        // using an actual digit's metrics so 612, 704, and 999 share one line.
        let digit = font.glyphs[b'0' as usize];
        let scale = settings.size * 0.78;
        center_y + (digit.baseline as f32 - digit.height as f32 * 0.5) * scale * sy
    } else {
        // charsgrid_med's glyph_quad is top-anchored and 8 * scale high.
        center_y - 4.0 * settings.size * 1.4
    };
    (x, y)
}

fn is_numeric_readout(id: HudElementId) -> bool {
    matches!(id, HudElementId::Speedometer | HudElementId::GroundSpeed
        | HudElementId::JumpHeight | HudElementId::JumpDistance
        | HudElementId::VerticalSpeed | HudElementId::YawSpeed
        | HudElementId::SpeedometerJumps)
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
    // Preserve the panel's original configurable X coordinate, but scale the
    // spacing of its contents uniformly with the screen's height. Otherwise
    // jumps columns, acceleration bars and the speed graph spread out on 21:9.
    let panel_x = |x: f32, anchor: f32| anchor * sx + (x - anchor) * sy;
    let ctx = crate::ui::HudRectContext::from_snapshot(ui);
    let paint = |out: &mut Vec<UiVertex>, item: &crate::speedometer::Text, id: HudElementId, anchor: f32| {
        let (px, py) = if is_numeric_readout(id) {
            let stock = crate::ui::hud_stock_rect(id, &ctx, w, h);
            numeric_readout_position(id, item, &speedo.settings, stock, small_font, h as f32)
        } else {
            (panel_x(item.x, anchor), item.y * sy)
        };
        let color = speedometer_color(item.color);
        if let Some(font) = small_font {
            // The acceleration label uses the legacy font's 0xB5 glyph.
            proportional_text_uniform(
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
    let fill = |out: &mut Vec<UiVertex>, item: &crate::speedometer::Rect, anchor: f32, legacy: bool| {
        let color = speedometer_color(item.color);
        // The old lagometer-shaped graph already carries widthRatioCoef on
        // both its X increments and bar widths, so don't apply it twice.
        let (x, rw) = if legacy { (item.x * sx, item.w * sx) }
            else { (panel_x(item.x, anchor), item.w * sy) };
        let (y, rh) = (item.y * sy, item.h * sy);
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
        let anchor = crate::speedometer::component_origin(*id, &speedo.settings)[0];
        draw_placed(out, ui, *id, w, h, |out| {
            // Native jaPRO text retains the authored left anchor. No bounds-
            // dependent recentering or anisotropic glyph/rectangle scaling.
            for item in &speedo.rects[rects.clone()] { fill(out, item, anchor, false); }
            for item in &speedo.texts[texts.clone()] { paint(out, item, *id, anchor); }
        });
    }
    if crate::ui::movement_components::selected(HudElementId::SpeedometerJumps, ui.strafe_helper.flags, ui.speedometer_settings.flags) != 0 {
    draw_placed(out, ui, HudElementId::SpeedometerJumps, w, h, |out| {
        for item in &speedo.jump_texts {
            paint(out, item, HudElementId::SpeedometerJumps, speedo.settings.jumps_x);
        }
    });
    }
    if crate::ui::movement_components::selected(HudElementId::SpeedGraph, ui.strafe_helper.flags, ui.speedometer_settings.flags) != 0 {
    draw_placed(out, ui, HudElementId::SpeedGraph, w, h, |out| {
        // The graph is independent of the numeric speedometer.
        for pic in &speedo.graph_pics {
            lagometer_pic(out, pic, ui, w, h);
        }
        let legacy = ui.speedometer_settings.flags & crate::speedometer::flag::SPEEDGRAPH == 0;
        for item in &speedo.graph_rects {
            fill(out, item, 245.0, legacy);
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
pub(crate) const ICON_RAGE_RECOVERY: usize = ICON_COMBAT_BASE + COMBAT_ICON_NAMES.len();
pub const ICON_NAMES: [&str; ICON_RAGE_RECOVERY + 1] = {
    let mut names = [""; ICON_RAGE_RECOVERY + 1];
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
    names[ICON_RAGE_RECOVERY] = "gfx/mp/f_icon_ragerec";
    names
};

/// The asset an atlas cell loads for the installed game. On a JK2 filesystem
/// weapon and inventory cells use JK2's own bg_itemlist icons (presentation
/// IDs index them); cells for JKA-only weapons/holdables stay empty.
pub fn icon_name(index: usize) -> std::borrow::Cow<'static, str> {
    let name = std::borrow::Cow::Borrowed(ICON_NAMES.get(index).copied().unwrap_or(""));
    if jka_assets::pk3::active_jk2().is_none() {
        return name;
    }
    let cell = |base: usize, len: usize| index.checked_sub(base).filter(|&offset| offset < len);
    let tagged = if let Some(weapon) = cell(ICON_WEAPON_BASE, WEAPON_ICON_NAMES.len())
        .or_else(|| cell(ICON_WEAPON_NO_AMMO_BASE, WEAPON_ICON_NAMES.len()))
    {
        crate::items::tagged_icon(1, weapon as i32)
    } else if let Some(tag) = cell(ICON_INVENTORY_BASE, INVENTORY_ICON_NAMES.len()) {
        crate::items::tagged_icon(6, tag as i32)
    } else {
        return name;
    };
    tagged.map_or(std::borrow::Cow::Borrowed(""), std::borrow::Cow::Owned)
}

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
/// Per-icon source tag. Kept outside the native UI shader's source range.
/// No dynamic shader pipelines are created unless tagged quads are visible.
pub(in crate::ui) const ICON_SHADER_SOURCE_BASE: f32 = 1024.0;

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
        ui.icon_texture_source(index),
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
            item.x * sx - proportional_text_width(&item.text, font, SCALE) * sy
        } else {
            item.x * sx
        };
        proportional_text_uniform(
            out,
            &item.text,
            font,
            x,
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
            item.x * sx - visible_jka_chars(&item.text) as f32 * GLYPH_W * sy
        } else {
            item.x * sx
        };
        fixed_charset_text(
            out,
            &item.text,
            x,
            item.y * sy,
            GLYPH_W * sy,
            8.0 * sy,
            GLYPH_W * sy,
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
            proportional_text_uniform(
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
                    position_q3df_speedometer_batch(&mut out[start..], ui, w, h);
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

    fn bounds_x(vertices: &[UiVertex], w: u32) -> (f32, f32) {
        let scale = w as f32 * 0.5;
        (
            vertices.iter().map(|v| (v.position[0] + 1.0) * scale).fold(f32::INFINITY, f32::min),
            vertices.iter().map(|v| (v.position[0] + 1.0) * scale).fold(f32::NEG_INFINITY, f32::max),
        )
    }

    /// Every numeric panel's first row has the same left inset and the same
    /// vertically centred digit line, including after the editor scales/moves
    /// a panel. The chargrid fallback has uniform eight-pixel glyphs so we
    /// can compare the actual triangles to the editor rectangles exactly.
    #[test]
    fn numeric_speed_panels_share_left_alignment_and_vertical_center() {
        use crate::ui::{hud_element_rect, HudRectContext};
        for (w, h) in [(1280, 960), (1920, 1080), (3440, 1440)] {
            for scale in [1.0, 1.5] {
                for id in [
                    HudElementId::Speedometer, HudElementId::GroundSpeed,
                    HudElementId::SpeedometerJumps, HudElementId::JumpHeight,
                    HudElementId::JumpDistance, HudElementId::VerticalSpeed,
                    HudElementId::YawSpeed,
                ] {
                    let mut ui = UiSnapshot::default();
                    ui.mode = crate::ui::OverlayMode::HudEdit;
                    ui.speedometer_settings.flags = crate::ui::movement_components::native_bit(id);
                    ui.speedometer = Some(crate::speedometer::preview(&ui.speedometer_settings));
                    ui.hud_layout.element_mut(id).scale = scale;
                    ui.hud_layout.element_mut(id).offset = [27.0, -13.0];

                    let mut vertices = Vec::new();
                    build_speedometer(&mut vertices, &ui, None, w, h);
                    assert!(!vertices.is_empty(), "{id:?}: missing sample text");
                    let ctx = HudRectContext::from_snapshot(&ui);
                    let panel = hud_element_rect(id, ui.hud_layout.element(id), &ctx, w, h);
                    let left = vertices.iter()
                        .map(|v| (v.position[0] + 1.0) * w as f32 * 0.5)
                        .fold(f32::INFINITY, f32::min);
                    let top = vertices.iter()
                        .map(|v| (1.0 - v.position[1]) * h as f32 * 0.5)
                        .fold(f32::INFINITY, f32::min);
                    let bottom = vertices.iter()
                        .map(|v| (1.0 - v.position[1]) * h as f32 * 0.5)
                        .fold(f32::NEG_INFINITY, f32::max);
                    let expected_left = panel.x + 2.0 * (h as f32 / 480.0) * scale;
                    let expected_center = panel.y + 10.0 * (h as f32 / 480.0) * scale;
                    assert!((left - expected_left).abs() < 0.1,
                        "{id:?} {w}x{h} scale={scale}: left {left} != {expected_left}");
                    assert!(((top + bottom) * 0.5 - expected_center).abs() < 0.1,
                        "{id:?} {w}x{h} scale={scale}: center {} != {expected_center}",
                        (top + bottom) * 0.5);
                }
            }
        }
    }

    #[test]
    fn native_speedometer_left_anchor_and_glyph_width_are_widescreen_safe() {
        let mut ui = UiSnapshot::default();
        ui.speedometer_settings.flags = crate::speedometer::flag::ENABLE;
        let mut speed = crate::speedometer::preview(&ui.speedometer_settings);
        speed.texts.retain(|t| !t.text.contains("612"));
        // A single "1" versus "1000" at identical authored x/y.
        let base = crate::speedometer::Text {
            text: "1".into(), x: 132.0, y: 459.0, color: [1.0; 4],
        };
        speed.components = vec![(HudElementId::Speedometer, 0..1, 0..0)];
        speed.rects.clear();
        for (w, h) in [(1024, 768), (1920, 1080), (3440, 1440)] {
            let mut origins = Vec::new();
            let mut glyph_width = 0.0;
            for value in ["1", "1000"] {
                speed.texts = vec![crate::speedometer::Text { text: value.into(), ..base.clone() }];
                ui.speedometer = Some(speed.clone());
                let mut vertices = Vec::new();
                build_speedometer(&mut vertices, &ui, None, w, h);
                let (left, right) = bounds_x(&vertices, w);
                origins.push(left);
                if value == "1000" { glyph_width = right - left; }
            }
            assert!((origins[0] - origins[1]).abs() < 0.05, "{w}x{h}: {origins:?}");
            // The fixed fallback font has a 6px advance per 640x480 unit.
            // Four digits should occupy the same *height-based* width in 4:3
            // and ultrawide (not scale with physical framebuffer width).
            assert!(glyph_width.is_finite() && glyph_width > 0.0);
        }
    }

    #[test]
    fn new_speed_graph_keeps_its_width_at_4_by_3_and_ultrawide() {
        let mut ui = UiSnapshot::default();
        ui.speedometer_settings.flags = crate::speedometer::flag::SPEEDGRAPH;
        ui.speedometer = Some(crate::speedometer::Ui {
            graph_rects: vec![crate::speedometer::Rect {
                x: 245.0, y: 368.0, w: 150.0, h: 22.0,
                color: [1.0; 4], outline: None,
            }],
            ..Default::default()
        });
        for (w, h) in [(1440, 1080), (1920, 1080), (2560, 1080)] {
            let mut vertices = Vec::new();
            build_speedometer(&mut vertices, &ui, None, w, h);
            let (left, right) = bounds_x(&vertices, w);
            assert!((right - left - 150.0 * h as f32 / 480.0).abs() < 0.05, "{w}x{h}");
        }
    }

    #[test]
    fn q3df_speedometer_position_does_not_depend_on_batch_bounds() {
        let ui = UiSnapshot::default();
        for (w, h) in [(1280, 960), (1920, 1080), (3440, 1440)] {
            let sy = h as f32 / 480.0;
            let source_x = w as f32 * 0.5 + (crate::strafehelper::Q3DF_SPEED_LEFT_X - 320.0) * sy;
            let source_y = crate::strafehelper::Q3DF_SPEED_TOP_Y * sy;
            let make_vertex = |x: f32| UiVertex {
                position: [x / w as f32 * 2.0 - 1.0, 1.0 - source_y / h as f32 * 2.0],
                uv: [0.0; 2], color: [1.0; 4], textured: 1.0,
            };
            let mut short = vec![make_vertex(source_x), make_vertex(source_x + 12.0 * sy)];
            let mut long = vec![make_vertex(source_x), make_vertex(source_x + 95.0 * sy)];
            position_q3df_speedometer_batch(&mut short, &ui, w, h);
            position_q3df_speedometer_batch(&mut long, &ui, w, h);
            assert!((short[0].position[0] - long[0].position[0]).abs() < 1e-6);
            assert!((short[0].position[1] - long[0].position[1]).abs() < 1e-6);
            let ctx = crate::ui::HudRectContext::from_snapshot(&ui);
            let stock = crate::ui::hud_stock_rect(HudElementId::Speedometer, &ctx, w, h);
            assert!(((short[0].position[0] + 1.0) * 0.5 * w as f32 - stock.x).abs() < 0.05);
        }
    }

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

#[cfg(test)]
mod widescreen_lagometer_tests {
    use super::*;

    #[test]
    fn right_aligned_lagometer_numbers_keep_aspect_and_anchor() {
        let mut glyphs = [crate::ui::ProportionalGlyph::default(); 256];
        for glyph in &mut glyphs {
            glyph.width = 8; glyph.height = 12; glyph.horiz_advance = 8;
            glyph.baseline = 10; glyph.s2 = 1.0; glyph.t2 = 1.0;
        }
        let font = ProportionalFont { glyphs, point_size: 12, height: 12 };
        let item = crate::lagometer::Text {
            text: "100".into(), x: 620.0, y: 200.0, right: true,
        };
        for (w, h) in [(640, 480), (1920, 1080), (2560, 1080), (1920, 700)] {
            let mut out = Vec::new();
            lagometer_text(&mut out, &item, Some(&font), w, h);
            let x = |v: &UiVertex| (v.position[0] + 1.0) * w as f32 * 0.5;
            let left = out.iter().map(x).fold(f32::INFINITY, f32::min);
            let right = out.iter().map(x).fold(f32::NEG_INFINITY, f32::max);
            let sy = h as f32 / 480.0;
            assert!((right - 620.0 * w as f32 / 640.0 - 1.0).abs() < 0.003);
            assert!(((right - left) - 24.0 * 0.5 * sy - 1.0).abs() < 0.003);
        }
    }
}
