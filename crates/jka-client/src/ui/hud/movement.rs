//! Hud movement.
use crate::ui::{
    color_code, draw_placed, fixed_charset_text, hud_line, proportional_text,
    proportional_text_width, rect, rect_outline, text, textured_rect_with_source, HudElementId,
    MovementKeysSettings, ProportionalFont, UiSnapshot, UiVertex, CROSSHAIR_IMAGE_COUNT,
    CROSSHAIR_IMAGE_NAMES, KEY_ART_SIZE, KEY_ATLAS_COLUMNS, KEY_ATLAS_ROWS, KEY_TEXTURE_SOURCE,
    SHELPER_CGAZ, SHELPER_ORIGINAL, SHELPER_UPDATED,
};

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
    draw_placed(out, ui, HudElementId::Speedometer, w, h, |out| {
        for item in &speedo.rects {
            fill(out, item);
        }
        for item in &speedo.texts {
            paint(out, item);
        }
    });
    draw_placed(out, ui, HudElementId::SpeedometerJumps, w, h, |out| {
        for item in &speedo.jump_texts {
            paint(out, item);
        }
    });
    draw_placed(out, ui, HudElementId::SpeedGraph, w, h, |out| {
        // The old speed graph's lag frame goes under its bars, its readout over them.
        for pic in &speedo.graph_pics {
            lagometer_pic(out, pic, w, h);
        }
        for item in &speedo.graph_rects {
            fill(out, item);
        }
        for item in &speedo.graph_texts {
            lagometer_text(out, item, small_font, w, h);
        }
    });
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

pub const ICON_NAMES: [&str; 2 + CROSSHAIR_IMAGE_COUNT as usize + FORCE_ICON_NAMES.len()] = [
    "gfx/2d/lag",
    "gfx/2d/net",
    CROSSHAIR_IMAGE_NAMES[0],
    CROSSHAIR_IMAGE_NAMES[1],
    CROSSHAIR_IMAGE_NAMES[2],
    CROSSHAIR_IMAGE_NAMES[3],
    CROSSHAIR_IMAGE_NAMES[4],
    CROSSHAIR_IMAGE_NAMES[5],
    CROSSHAIR_IMAGE_NAMES[6],
    CROSSHAIR_IMAGE_NAMES[7],
    CROSSHAIR_IMAGE_NAMES[8],
    CROSSHAIR_IMAGE_NAMES[9],
    FORCE_ICON_NAMES[0],
    FORCE_ICON_NAMES[1],
    FORCE_ICON_NAMES[2],
    FORCE_ICON_NAMES[3],
    FORCE_ICON_NAMES[4],
    FORCE_ICON_NAMES[5],
    FORCE_ICON_NAMES[6],
    FORCE_ICON_NAMES[7],
    FORCE_ICON_NAMES[8],
    FORCE_ICON_NAMES[9],
    FORCE_ICON_NAMES[10],
    FORCE_ICON_NAMES[11],
    FORCE_ICON_NAMES[12],
    FORCE_ICON_NAMES[13],
    FORCE_ICON_NAMES[14],
    FORCE_ICON_NAMES[15],
    FORCE_ICON_NAMES[16],
    FORCE_ICON_NAMES[17],
];

/// Atlas cell of the first image crosshair.
pub(in crate::ui) const ICON_CROSSHAIR_BASE: usize = 2;

pub(in crate::ui) const ICON_FORCE_BASE: usize = 2 + CROSSHAIR_IMAGE_COUNT as usize;

/// Edge of one atlas cell in texels; the retail 32x32 images are resampled to it.
pub const ICON_CELL: u32 = 64;

/// Texture source id of the icon atlas in `ui.wgsl`.
pub(in crate::ui) const ICON_TEXTURE_SOURCE: f32 = 5.0;

/// Lazily rebuilt team-overlay icon atlas (`ui.wgsl` texture source 6).
pub const TEAM_ICON_CELL: u32 = 64;

pub const TEAM_ICON_COLUMNS: u32 = 8;

pub(in crate::ui) const TEAM_ICON_TEXTURE_SOURCE: f32 = 6.0;

/// The whole of atlas cell `index` as (uv0, uv1).
pub(in crate::ui) fn icon_cell_uv(index: usize) -> ([f32; 2], [f32; 2]) {
    let cell = ICON_CELL as f32;
    let atlas_w = ICON_NAMES.len() as f32 * cell;
    let index = index as f32;
    // Half-texel inset keeps linear filtering inside this image's cell.
    let u0 = (index * cell + 0.5) / atlas_w;
    let u1 = ((index + 1.0) * cell - 0.5) / atlas_w;
    ([u0, 0.5 / cell], [u1, (cell - 0.5) / cell])
}

/// `CG_DrawPic` of a lagometer image: the whole image, white, alpha blended.
pub(in crate::ui) fn lagometer_pic(
    out: &mut Vec<UiVertex>,
    pic: &crate::lagometer::Pic,
    w: u32,
    h: u32,
) {
    let index = match pic.icon {
        crate::lagometer::Icon::Lag => 0,
        crate::lagometer::Icon::Net => 1,
    };
    let (uv0, uv1) = icon_cell_uv(index);
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
            lagometer_pic(out, pic, w, h);
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
        lagometer_pic(out, pic, w, h);
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
    draw_placed(out, ui, HudElementId::RaceTimer, w, h, |out| {
        paint(
            out,
            &race.timer_text,
            race.timer_x,
            race.timer_y,
            [1.0, 1.0, 1.0, 1.0],
        );
    });
    let [r, g, b] = race.start_color;
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
    let sh = ui.strafe_helper;
    // Cinematic is submitted through the 3D FX path in app.rs; do not also
    // spend time projecting/building an empty 2D strafehelper batch.
    if sh.flags & (SHELPER_ORIGINAL | SHELPER_UPDATED | SHELPER_CGAZ) == 0 {
        return;
    }
    let aspect = w.max(1) as f32 / h.max(1) as f32;
    let segments =
        crate::strafehelper::strafe_lines(&sh, &ui.movement_hud, ui.video.fps_cap, aspect);
    // cg_draw's 640x480 space maps straight onto the window on both axes.
    let (sx, sy) = (w as f32 / 640.0, h as f32 / 480.0);
    for segment in segments {
        let [x0, y0] = [segment.from[0] * sx, segment.from[1] * sy];
        let [x1, y1] = [segment.to[0] * sx, segment.to[1] * sy];
        // DF_DrawLine stamps size x size squares (scaled per axis) along the
        // segment; the swept shape is as thick as the square's extent across it.
        let length = (x1 - x0).hypot(y1 - y0);
        if length <= f32::EPSILON {
            continue;
        }
        let (nx, ny) = (-(y1 - y0) / length, (x1 - x0) / length);
        let thickness = segment.size * (sx * nx.abs() + sy * ny.abs());
        hud_line(out, x0, y0, x1, y1, thickness, segment.color, w, h);
    }
}
