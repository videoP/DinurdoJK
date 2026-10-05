//! Text.
use crate::ui::{
    glyph_quad, glyph_quad_sized, rect, textured_rect_with_source, Pod, UiWorldPlayerLabel,
    Zeroable, CONSOLE_CHAR_HEIGHT, CONSOLE_CHAR_WIDTH,
};

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct UiVertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
    /// 0.0 = solid color, 1.0 = charsgrid, 2.0 = proportional font, 3.0 = splash.
    pub textured: f32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ProportionalGlyph {
    pub width: i16,
    pub height: i16,
    pub horiz_advance: i16,
    pub horiz_offset: i16,
    pub baseline: i32,
    pub s: f32,
    pub t: f32,
    pub s2: f32,
    pub t2: f32,
}

#[derive(Debug, Clone)]
pub struct ProportionalFont {
    pub glyphs: [ProportionalGlyph; 256],
    pub point_size: i16,
    pub height: i16,
}

pub fn parse_fontdat(bytes: &[u8]) -> Result<ProportionalFont, String> {
    const GLYPH_COUNT: usize = 256;
    const GLYPH_BYTES: usize = 28;
    const HEADER_BYTES: usize = GLYPH_COUNT * GLYPH_BYTES + 10;
    if bytes.len() < HEADER_BYTES {
        return Err(format!(
            "fontdat is {} bytes; expected at least {HEADER_BYTES}",
            bytes.len()
        ));
    }

    fn read_i16(bytes: &[u8], offset: &mut usize) -> i16 {
        let value = i16::from_le_bytes([bytes[*offset], bytes[*offset + 1]]);
        *offset += 2;
        value
    }
    fn read_i32(bytes: &[u8], offset: &mut usize) -> i32 {
        let value = i32::from_le_bytes([
            bytes[*offset],
            bytes[*offset + 1],
            bytes[*offset + 2],
            bytes[*offset + 3],
        ]);
        *offset += 4;
        value
    }
    fn read_f32(bytes: &[u8], offset: &mut usize) -> f32 {
        let value = f32::from_le_bytes([
            bytes[*offset],
            bytes[*offset + 1],
            bytes[*offset + 2],
            bytes[*offset + 3],
        ]);
        *offset += 4;
        value
    }

    let mut offset = 0usize;
    let mut glyphs = [ProportionalGlyph::default(); GLYPH_COUNT];
    for glyph in &mut glyphs {
        *glyph = ProportionalGlyph {
            width: read_i16(bytes, &mut offset),
            height: read_i16(bytes, &mut offset),
            horiz_advance: read_i16(bytes, &mut offset),
            horiz_offset: read_i16(bytes, &mut offset),
            baseline: read_i32(bytes, &mut offset),
            s: read_f32(bytes, &mut offset),
            t: read_f32(bytes, &mut offset),
            s2: read_f32(bytes, &mut offset),
            t2: read_f32(bytes, &mut offset),
        };
        if glyph.width < 0
            || glyph.height < 0
            || glyph.horiz_advance < 0
            || ![glyph.s, glyph.t, glyph.s2, glyph.t2]
                .into_iter()
                .all(f32::is_finite)
        {
            return Err("fontdat contains invalid glyph metrics".into());
        }
    }
    let point_size = read_i16(bytes, &mut offset);
    let height = read_i16(bytes, &mut offset);
    let _ascender = read_i16(bytes, &mut offset);
    let _descender = read_i16(bytes, &mut offset);
    let _korean_hack = read_i16(bytes, &mut offset);
    if point_size <= 0 || height <= 0 {
        return Err(format!(
            "fontdat has invalid dimensions point_size={point_size} height={height}"
        ));
    }

    Ok(ProportionalFont {
        glyphs,
        point_size,
        height,
    })
}

pub(in crate::ui) fn truncate_jka_text(value: &str, max_visible: usize) -> String {
    let bytes = crate::cgame::text_to_jka_bytes(value);
    let mut out = Vec::with_capacity(bytes.len().min(max_visible + 8));
    let mut visible = 0usize;
    let mut index = 0usize;
    while index < bytes.len() && visible < max_visible {
        if bytes[index] == b'^'
            && index + 1 < bytes.len()
            && color_code(bytes[index + 1] as char, 1.0).is_some()
        {
            out.extend_from_slice(&bytes[index..index + 2]);
            index += 2;
            continue;
        }
        out.push(bytes[index]);
        visible += 1;
        index += 1;
    }
    out.into_iter().map(char::from).collect()
}

#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn text(
    out: &mut Vec<UiVertex>,
    value: &str,
    x: f32,
    y: f32,
    scale: f32,
    color: [f32; 4],
    width: u32,
    height: u32,
) {
    let mut cursor_x = x;
    let mut cursor_y = y;
    let mut active_color = color;
    let bytes = crate::cgame::text_to_jka_bytes(value);
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'\n' {
            cursor_x = x;
            cursor_y += 9.0 * scale;
            index += 1;
            continue;
        }
        if byte == b'^' && index + 1 < bytes.len() {
            if let Some(code) = color_code(bytes[index + 1] as char, color[3]) {
                active_color = code;
                index += 2;
                continue;
            }
        }
        if byte == b' ' {
            cursor_x += 6.0 * scale;
            index += 1;
            continue;
        }
        glyph_quad(
            out,
            byte,
            cursor_x,
            cursor_y,
            scale,
            active_color,
            width,
            height,
        );
        cursor_x += 6.0 * scale;
        index += 1;
    }
}

pub(in crate::ui) fn proportional_text_width(
    value: &str,
    font: &ProportionalFont,
    scale: f32,
) -> f32 {
    let bytes = crate::cgame::text_to_jka_bytes(value);
    let mut width = 0.0f32;
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'^'
            && index + 1 < bytes.len()
            && color_code(bytes[index + 1] as char, 1.0).is_some()
        {
            index += 2;
            continue;
        }
        if bytes[index] == b'\n' {
            break;
        }
        width += font.glyphs[bytes[index] as usize].horiz_advance.max(0) as f32 * scale;
        index += 1;
    }
    width
}

pub(in crate::ui) fn wrap_proportional_text(
    value: &str,
    font: &ProportionalFont,
    scale: f32,
    max_width: f32,
) -> String {
    if proportional_text_width(value, font, scale) <= max_width {
        return value.to_owned();
    }

    let bytes = crate::cgame::text_to_jka_bytes(value);
    let mut out = String::with_capacity(value.len() + value.len() / 32);
    let mut line_width = 0.0f32;
    let mut line_start = 0usize;
    let mut last_space_out = None::<usize>;
    let mut index = 0usize;

    while index < bytes.len() {
        if bytes[index] == b'^'
            && index + 1 < bytes.len()
            && color_code(bytes[index + 1] as char, 1.0).is_some()
        {
            out.push(bytes[index] as char);
            out.push(bytes[index + 1] as char);
            index += 2;
            continue;
        }
        let byte = bytes[index];
        if byte == b'\n' {
            out.push('\n');
            line_width = 0.0;
            line_start = out.len();
            last_space_out = None;
            index += 1;
            continue;
        }

        if byte == b' ' {
            last_space_out = Some(out.len());
        }
        out.push(byte as char);
        line_width += font.glyphs[byte as usize].horiz_advance.max(0) as f32 * scale;

        if line_width >= max_width {
            let break_at = last_space_out.filter(|space| *space >= line_start);
            if let Some(space) = break_at {
                out.replace_range(space..=space, "\n");
                let tail = out[space + 1..].to_owned();
                line_width = proportional_text_width(&tail, font, scale);
                line_start = space + 1;
            } else {
                out.push('\n');
                line_width = 0.0;
                line_start = out.len();
            }
            last_space_out = None;
        }
        index += 1;
    }
    out
}

#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn proportional_text(
    out: &mut Vec<UiVertex>,
    value: &str,
    font: &ProportionalFont,
    x: f32,
    baseline_y: f32,
    scale: f32,
    color: [f32; 4],
    shadow: bool,
    width: u32,
    height: u32,
) {
    let x_scale = width.max(1) as f32 / 640.0;
    let y_scale = height.max(1) as f32 / 480.0;
    let line_step = 20.0 * scale * y_scale;

    let draw_pass = |out: &mut Vec<UiVertex>, shadow_pass: bool| {
        let mut cursor_x = x;
        let mut cursor_y = baseline_y;
        let mut active_color = color;
        let bytes = crate::cgame::text_to_jka_bytes(value);
        let mut index = 0usize;
        while index < bytes.len() {
            let byte = bytes[index];
            if byte == b'\n' {
                cursor_x = x;
                cursor_y += line_step;
                index += 1;
                continue;
            }
            if byte == b'^' && index + 1 < bytes.len() {
                if let Some(code) = color_code(bytes[index + 1] as char, color[3]) {
                    active_color = code;
                    index += 2;
                    continue;
                }
            }

            let glyph = font.glyphs[byte as usize];
            if glyph.width > 0 && glyph.height > 0 && byte != b' ' {
                // OpenJK chat uses the font drop-shadow style. Keep this
                // screen-space tight instead of scaling the offset with the
                // framebuffer, which made the shadow too far away at 1080p+.
                let shadow_offset_x = if shadow_pass { 1.0 } else { 0.0 };
                let shadow_offset_y = if shadow_pass { 1.0 } else { 0.0 };
                let draw_color = if shadow_pass {
                    [0.0, 0.0, 0.0, color[3] * 0.72]
                } else {
                    active_color
                };
                let draw_x =
                    cursor_x + glyph.horiz_offset as f32 * scale * x_scale + shadow_offset_x;
                let draw_y = cursor_y - glyph.baseline as f32 * scale * y_scale + shadow_offset_y;
                textured_rect_with_source(
                    out,
                    draw_x,
                    draw_y,
                    glyph.width as f32 * scale * x_scale,
                    glyph.height as f32 * scale * y_scale,
                    [glyph.s, glyph.t],
                    [glyph.s2, glyph.t2],
                    draw_color,
                    2.0,
                    width,
                    height,
                );
            }
            cursor_x += glyph.horiz_advance.max(0) as f32 * scale * x_scale;
            index += 1;
        }
    };

    if shadow {
        draw_pass(out, true);
    }
    draw_pass(out, false);
}

/// Append one projected `cg_drawPlayerNames` label. `x`/`y` are physical
/// framebuffer coordinates produced from the final render camera.
pub fn build_world_player_label_vertices(
    out: &mut Vec<UiVertex>,
    label: &UiWorldPlayerLabel,
    x: f32,
    y: f32,
    scale: f32,
    font: Option<&ProportionalFont>,
    width: u32,
    height: u32,
) {
    let sx = width.max(1) as f32 / 640.0;
    let sy = height.max(1) as f32 / 480.0;
    let scale = scale.clamp(0.05, 4.0);

    if let Some(font) = font {
        // Most player labels are one line, but race-ghost comparison labels can
        // add a second stats line. Center each line independently instead of
        // letting the generic text drawer inherit the first line's x origin.
        let line_step = 20.0 * scale * sy;
        for (line, text) in label.text.split('\n').enumerate() {
            let text_width = proportional_text_width(text, font, scale) * sx;
            proportional_text(
                out,
                text,
                font,
                x - text_width * 0.5,
                y + line as f32 * line_step,
                scale,
                [1.0, 1.0, 1.0, 1.0],
                true,
                width,
                height,
            );
        }
    } else {
        // Startup/font-load fallback. The ordinary renderer normally has the
        // proportional JKA font by the time a game snapshot can exist.
        let fallback_scale = (1.25 * scale).max(0.25);
        for (line, value) in label.text.split('\n').enumerate() {
            let visible = crate::cgame::text_to_jka_bytes(value)
                .iter()
                .filter(|&&c| c != b'^')
                .count() as f32;
            text(
                out,
                value,
                x - visible * 3.0 * fallback_scale,
                y - 4.0 * fallback_scale + line as f32 * 12.0 * fallback_scale,
                fallback_scale,
                [1.0, 1.0, 1.0, 1.0],
                width,
                height,
            );
        }
    }

    let Some(fraction) = label.health_fraction else {
        return;
    };
    // TaystJK HEALTH_WIDTH/HEIGHT are 50x5 virtual pixels. Keep the same
    // authored size and alpha, but render it at native framebuffer resolution.
    let bar_w = 50.0 * sx;
    let bar_h = 5.0 * sy;
    let border = 1.0_f32.max(sx.min(sy));
    let bar_x = x - bar_w * 0.5;
    let bar_y = y - 7.0 * sy;
    rect(
        out,
        bar_x - border,
        bar_y - border,
        bar_w + border * 2.0,
        bar_h + border * 2.0,
        [0.0, 0.0, 0.0, 0.75],
        width,
        height,
    );
    rect(
        out,
        bar_x,
        bar_y,
        bar_w,
        bar_h,
        [0.5, 0.5, 0.5, 0.4],
        width,
        height,
    );
    rect(
        out,
        bar_x,
        bar_y,
        bar_w * fraction.clamp(0.0, 1.0),
        bar_h,
        label.health_color,
        width,
        height,
    );
}

#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn fixed_charset_text(
    out: &mut Vec<UiVertex>,
    value: &str,
    x: f32,
    y: f32,
    glyph_w: f32,
    glyph_h: f32,
    advance: f32,
    color: [f32; 4],
    shadow: bool,
    width: u32,
    height: u32,
) {
    if shadow {
        let mut cursor_x = x + 2.0;
        let mut cursor_y = y + 2.0;
        let bytes = crate::cgame::text_to_jka_bytes(value);
        let mut index = 0usize;
        while index < bytes.len() {
            let byte = bytes[index];
            if byte == b'\n' {
                cursor_x = x + 2.0;
                cursor_y += glyph_h;
                index += 1;
                continue;
            }
            if byte == b'^'
                && index + 1 < bytes.len()
                && color_code(bytes[index + 1] as char, color[3]).is_some()
            {
                index += 2;
                continue;
            }
            if byte != b' ' {
                glyph_quad_sized(
                    out,
                    byte,
                    cursor_x,
                    cursor_y,
                    glyph_w,
                    glyph_h,
                    [0.0, 0.0, 0.0, color[3]],
                    width,
                    height,
                );
            }
            cursor_x += advance;
            index += 1;
        }
    }

    let mut cursor_x = x;
    let mut cursor_y = y;
    let mut active_color = color;
    let bytes = crate::cgame::text_to_jka_bytes(value);
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'\n' {
            cursor_x = x;
            cursor_y += glyph_h;
            index += 1;
            continue;
        }
        if byte == b'^' && index + 1 < bytes.len() {
            if let Some(code) = color_code(bytes[index + 1] as char, color[3]) {
                active_color = code;
                index += 2;
                continue;
            }
        }
        if byte != b' ' {
            glyph_quad_sized(
                out,
                byte,
                cursor_x,
                cursor_y,
                glyph_w,
                glyph_h,
                active_color,
                width,
                height,
            );
        }
        cursor_x += advance;
        index += 1;
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn console_text(
    out: &mut Vec<UiVertex>,
    value: &str,
    x: f32,
    y: f32,
    color: [f32; 4],
    width: u32,
    height: u32,
) {
    let mut cursor_x = x;
    let mut cursor_y = y;
    let mut active_color = color;
    let bytes = crate::cgame::text_to_jka_bytes(value);
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'\n' {
            cursor_x = x;
            cursor_y += CONSOLE_CHAR_HEIGHT;
            index += 1;
            continue;
        }
        if byte == b'^' && index + 1 < bytes.len() {
            if let Some(code) = color_code(bytes[index + 1] as char, color[3]) {
                active_color = code;
                index += 2;
                continue;
            }
        }
        if byte != b' ' {
            glyph_quad_sized(
                out,
                byte,
                cursor_x,
                cursor_y,
                CONSOLE_CHAR_WIDTH,
                CONSOLE_CHAR_HEIGHT,
                active_color,
                width,
                height,
            );
        }
        cursor_x += CONSOLE_CHAR_WIDTH;
        index += 1;
    }
}

pub(in crate::ui) fn color_code(code: char, alpha: f32) -> Option<[f32; 4]> {
    let rgb = match code {
        // JKA/Quake color escapes are saturated primaries/secondaries.
        // Do not pastelize these: ^2 in particular is the stock chat green.
        '0' => [0.0, 0.0, 0.0],
        '1' => [1.0, 0.0, 0.0],
        '2' => [0.0, 1.0, 0.0],
        '3' => [1.0, 1.0, 0.0],
        '4' => [0.0, 0.0, 1.0],
        '5' => [0.0, 1.0, 1.0],
        '6' => [1.0, 0.0, 1.0],
        '7' => [1.0, 1.0, 1.0],
        '8' => [1.0, 0.62, 0.22],
        '9' => [0.62, 0.62, 0.62],
        _ => return None,
    };
    Some([rgb[0], rgb[1], rgb[2], alpha])
}

/// Generate a tiny recovery charset using the same atlas layout as JKA's
/// gfx/2d/charsgrid_med: 16x16 logical slots on a 256x256 texture, with each
/// glyph living in the left 8x16 pixels of its 16x16 slot. Keeping the fallback
/// in the same layout means the normal OpenJK-compatible UV path stays valid.
pub fn fallback_font_rgba() -> (u32, u32, Vec<u8>) {
    const SLOT: usize = 16;
    const GRID: usize = 16;
    let width = SLOT * GRID;
    let height = SLOT * GRID;
    let mut rgba = vec![0u8; width * height * 4];
    for code in 0u16..=255 {
        let rows = glyph(code as u8 as char);
        let cell_x = (code as usize & 15) * SLOT;
        let cell_y = (code as usize >> 4) * SLOT;
        for (gy, bits) in rows.into_iter().enumerate() {
            for gx in 0..5 {
                if bits & (1 << (4 - gx)) == 0 {
                    continue;
                }
                let px = cell_x + gx + 1;
                let py = cell_y + gy + 4;
                let offset = (py * width + px) * 4;
                rgba[offset..offset + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
    }
    (width as u32, height as u32, rgba)
}

pub(in crate::ui) fn glyph(ch: char) -> [u8; 7] {
    match ch.to_ascii_uppercase() {
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01111, 0b10000, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110,
        ],
        '6' => [
            0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110,
        ],
        '/' => [
            0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000,
        ],
        '\\' => [
            0b10000, 0b01000, 0b01000, 0b00100, 0b00010, 0b00010, 0b00001,
        ],
        '-' => [0, 0, 0, 0b11111, 0, 0, 0],
        '_' => [0, 0, 0, 0, 0, 0, 0b11111],
        '.' => [0, 0, 0, 0, 0, 0b00110, 0b00110],
        ':' => [0, 0b00110, 0b00110, 0, 0b00110, 0b00110, 0],
        '>' => [
            0b10000, 0b01000, 0b00100, 0b00010, 0b00100, 0b01000, 0b10000,
        ],
        '<' => [
            0b00001, 0b00010, 0b00100, 0b01000, 0b00100, 0b00010, 0b00001,
        ],
        '=' => [0, 0b11111, 0, 0b11111, 0, 0, 0],
        '+' => [0, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0],
        '%' => [0b11001, 0b11010, 0b00100, 0b01000, 0b10110, 0b00110, 0],
        '?' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0, 0b00100],
        '[' => [
            0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110,
        ],
        ']' => [
            0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110,
        ],
        '(' => [
            0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010,
        ],
        ')' => [
            0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000,
        ],
        ' ' => [0; 7],
        _ => [0b01110, 0b10001, 0b00010, 0b00100, 0b00100, 0, 0b00100],
    }
}
