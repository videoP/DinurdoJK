//! Chat.
use crate::ui::{
    color_code, fixed_charset_text, proportional_text, rect, visible_jka_chars,
    wrap_proportional_text, ChatMode, ProportionalFont, UiSnapshot, UiVertex, CHATBOX_CUTOFF,
    CHATBOX_FONT_HEIGHT, CHATBOX_FONT_SCALE, CHATBOX_Y,
};

pub(in crate::ui) fn wrap_fixed_chat_text(value: &str, max_columns: usize) -> String {
    let max_columns = max_columns.max(1);
    let bytes = crate::cgame::text_to_jka_bytes(value);
    let mut out = String::with_capacity(value.len() + value.len() / max_columns.max(8));
    let mut line_start = 0usize;
    let mut line_columns = 0usize;
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
            line_start = out.len();
            line_columns = 0;
            last_space_out = None;
            index += 1;
            continue;
        }
        if byte == b' ' {
            last_space_out = Some(out.len());
        }
        out.push(byte as char);
        line_columns += 1;

        if line_columns >= max_columns {
            if let Some(space) = last_space_out.filter(|space| *space >= line_start) {
                out.replace_range(space..=space, "\n");
                line_start = space + 1;
                line_columns = visible_jka_chars(&out[line_start..]);
            } else {
                out.push('\n');
                line_start = out.len();
                line_columns = 0;
            }
            last_space_out = None;
        }
        index += 1;
    }
    out
}

pub(in crate::ui) fn clamp_wrapped_chat_lines(value: &str, max_lines: usize) -> String {
    let max_lines = max_lines.max(1);
    let mut kept = value.split('\n').take(max_lines);
    let Some(first) = kept.next() else {
        return String::new();
    };
    let mut out = first.to_owned();
    for line in kept {
        out.push('\n');
        out.push_str(line);
    }
    out
}

pub(in crate::ui) fn build_chat_history(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    if ui.chat_lines.is_empty() {
        return;
    }

    let Some(font) = small_font else {
        // Asset fallback only. Stock JKA uses proportional FONT_SMALL / ocr_a
        // here; charsgrid is retained solely so chat remains visible if a mod
        // removes the retail font files.
        // Uncolored chat starts with the speaker name. Keep that white;
        // inline ^2/^5/etc codes from JKA/TaystJK then color the message body.
        const CHAT_BASE_WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
        let bottom = h as f32 * (CHATBOX_Y / 480.0);
        let line_step = h as f32 * (CHATBOX_FONT_HEIGHT * CHATBOX_FONT_SCALE / 480.0);
        let chat_width = (CHATBOX_CUTOFF * ui.hud_layout.chat.extent[0]).clamp(120.0, 2200.0);
        let max_columns = (chat_width / 4.5).floor().max(1.0) as usize;
        let max_lines = ((52.0 * ui.hud_layout.chat.extent[1])
            / (CHATBOX_FONT_HEIGHT * CHATBOX_FONT_SCALE))
            .floor()
            .max(1.0) as usize;
        let mut wrapped = Vec::new();
        for line in &ui.chat_lines {
            let alpha = line.alpha.clamp(0.0, 1.0);
            if alpha <= 0.01 {
                continue;
            }
            let text = wrap_fixed_chat_text(&line.text, max_columns);
            let lines = text.bytes().filter(|&byte| byte == b'\n').count() + 1;
            wrapped.push((text, lines, alpha));
        }
        while wrapped.len() > 1
            && wrapped.iter().map(|(_, lines, _)| *lines).sum::<usize>() > max_lines
        {
            wrapped.remove(0);
        }
        if let Some((text, lines, _)) = wrapped.first_mut() {
            if *lines > max_lines {
                *text = clamp_wrapped_chat_lines(text, max_lines);
                *lines = max_lines;
            }
        }
        let total_lines = wrapped
            .iter()
            .map(|(_, lines, _)| *lines)
            .sum::<usize>()
            .min(max_lines);
        let first_y = bottom - total_lines.saturating_sub(1) as f32 * line_step;
        let mut y = first_y;
        for (text, lines, alpha) in wrapped {
            let mut color = CHAT_BASE_WHITE;
            color[3] = alpha;
            fixed_charset_text(
                out,
                &text,
                w as f32 * (30.0 / 640.0),
                y,
                w as f32 * (4.0 / 640.0),
                h as f32 * (8.0 / 480.0),
                w as f32 * (4.5 / 640.0),
                color,
                true,
                w,
                h,
            );
            y += lines as f32 * line_step;
        }
        return;
    };

    // OpenJK CG_ChatBox_DrawStrings uses FONT_SMALL at 0.65 scale. Match a
    // TaystJK-style cg_chatBoxFontSize 0.5 and cg_chatBoxHeight 425 here.
    // JKA's FONT_SMALL default is fonts/ocr_a.{fontdat,tga}.
    const CHATBOX_X: f32 = 30.0;
    // The server chat string carries message colors inline. The initial
    // uncolored run is the player name, so its base color must be white.
    const CHAT_BASE_WHITE: [f32; 3] = [1.0, 1.0, 1.0];

    let chat_width = (CHATBOX_CUTOFF * ui.hud_layout.chat.extent[0]).clamp(120.0, 2200.0);
    let max_lines = ((52.0 * ui.hud_layout.chat.extent[1])
        / (CHATBOX_FONT_HEIGHT * CHATBOX_FONT_SCALE))
        .floor()
        .max(1.0) as usize;
    let mut wrapped = Vec::new();
    for line in &ui.chat_lines {
        let alpha = line.alpha.clamp(0.0, 1.0);
        if alpha <= 0.01 {
            continue;
        }
        let text = wrap_proportional_text(&line.text, font, CHATBOX_FONT_SCALE, chat_width);
        let lines = text.bytes().filter(|&byte| byte == b'\n').count() + 1;
        wrapped.push((text, lines, alpha));
    }
    if wrapped.is_empty() {
        return;
    }

    while wrapped.len() > 1 && wrapped.iter().map(|(_, lines, _)| *lines).sum::<usize>() > max_lines
    {
        wrapped.remove(0);
    }
    if let Some((text, lines, _)) = wrapped.first_mut() {
        if *lines > max_lines {
            *text = clamp_wrapped_chat_lines(text, max_lines);
            *lines = max_lines;
        }
    }
    let total_lines: usize = wrapped.iter().map(|(_, lines, _)| *lines).sum();
    let line_step_virtual = CHATBOX_FONT_HEIGHT * CHATBOX_FONT_SCALE;
    let x = CHATBOX_X * w as f32 / 640.0;
    let mut baseline_y = (CHATBOX_Y - line_step_virtual * total_lines as f32) * h as f32 / 480.0;

    for (value, lines, alpha) in wrapped {
        proportional_text(
            out,
            &value,
            font,
            x,
            baseline_y,
            CHATBOX_FONT_SCALE,
            [
                CHAT_BASE_WHITE[0],
                CHAT_BASE_WHITE[1],
                CHAT_BASE_WHITE[2],
                alpha,
            ],
            true,
            w,
            h,
        );
        baseline_y += line_step_virtual * lines as f32 * h as f32 / 480.0;
    }
}

pub(in crate::ui) fn build_chat_input(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    // OpenJK draws the message-entry field with SCR_DrawBigString /
    // Field_BigDraw: 16x16 geometry sampling the same charsgrid_med atlas.
    const BIG_CHAR: f32 = 16.0;
    const CHAT_WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
    const INPUT_HEIGHT: f32 = 40.0;
    const INPUT_GAP: f32 = 6.0;
    let chat_width_virtual = (CHATBOX_CUTOFF * ui.hud_layout.chat.extent[0]).clamp(120.0, 2200.0);
    let max_history_lines = ((52.0 * ui.hud_layout.chat.extent[1])
        / (CHATBOX_FONT_HEIGHT * CHATBOX_FONT_SCALE))
        .floor()
        .max(1.0) as usize;
    let width = (chat_width_virtual * w as f32 / 640.0).clamp(220.0, (w as f32 - 36.0).max(220.0));
    let x = 18.0;

    // Anchor the entry box above the oldest visible chat line instead of to a
    // fixed number of pixels from the bottom.  cg_chatBoxHeight is in 640x480
    // virtual coordinates, while this field is drawn in framebuffer pixels.
    let y_scale = h as f32 / 480.0;
    let history_top = if let Some(font) = small_font {
        let total_lines: usize = ui
            .chat_lines
            .iter()
            .filter(|line| line.alpha > 0.01)
            .map(|line| {
                let wrapped = wrap_proportional_text(
                    &line.text,
                    font,
                    CHATBOX_FONT_SCALE,
                    chat_width_virtual,
                );
                wrapped.bytes().filter(|&byte| byte == b'\n').count() + 1
            })
            .sum::<usize>()
            .min(max_history_lines);
        let line_step = CHATBOX_FONT_HEIGHT * CHATBOX_FONT_SCALE;
        (CHATBOX_Y - line_step * (total_lines.max(1) as f32 + 1.0)) * y_scale
    } else {
        let count = ui.chat_lines.len().min(max_history_lines).max(1);
        let line_step = 6.5 * y_scale;
        CHATBOX_Y * y_scale - count as f32 * line_step
    };
    let y = (history_top - INPUT_GAP - INPUT_HEIGHT).max(0.0);
    rect(out, x, y, width, 40.0, [0.01, 0.018, 0.028, 0.92], w, h);
    rect(out, x, y + 38.0, width, 2.0, [0.25, 0.78, 0.30, 0.95], w, h);
    let prompt = match ui.chat_mode {
        ChatMode::Global => "SAY:",
        ChatMode::Team => "TEAM:",
    };
    fixed_charset_text(
        out,
        prompt,
        x + 8.0,
        y + 11.0,
        BIG_CHAR,
        BIG_CHAR,
        BIG_CHAR,
        CHAT_WHITE,
        true,
        w,
        h,
    );

    let input_x = x + 8.0 + (prompt.chars().count() as f32 + 1.0) * BIG_CHAR;
    fixed_charset_text(
        out,
        &format!("{}_", ui.chat_input),
        input_x,
        y + 11.0,
        BIG_CHAR,
        BIG_CHAR,
        BIG_CHAR,
        CHAT_WHITE,
        true,
        w,
        h,
    );
}

/// `gfx/hud/keys/*` images `DF_DrawMovementKeys` draws, in atlas order (see
/// `renderer::load_ui_key_atlas`). `KeyArt` indexes this table.
pub const MOVEMENT_KEY_ART: [&str; 27] = [
    "crouch_off",
    "crouch_on",
    "jump_off",
    "jump_on",
    "back_off",
    "back_on",
    "forward_off",
    "forward_on",
    "left_off",
    "left_on",
    "right_off",
    "right_on",
    "attack_off",
    "attack_on",
    "alt_off",
    "alt_on",
    "walk_off",
    "walk_on",
    "crouch_on2",
    "jump_on2",
    "back_on2",
    "forward_on2",
    "left_on2",
    "right_on2",
    "attack_on2",
    "alt_on2",
    "walk_on2",
];

pub const KEY_ART_SIZE: u32 = 128;

pub const KEY_ATLAS_COLUMNS: u32 = 6;

pub(in crate::ui) const KEY_ATLAS_ROWS: u32 =
    (MOVEMENT_KEY_ART.len() as u32).div_ceil(KEY_ATLAS_COLUMNS);

/// Texture source id of the key atlas in `ui.wgsl`.
pub(in crate::ui) const KEY_TEXTURE_SOURCE: f32 = 4.0;
