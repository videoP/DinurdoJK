//! Hud team.
use crate::ui::{
    fixed_charset_text, proportional_text, proportional_text_width, rect, text,
    textured_rect_with_source, truncate_jka_text, visible_jka_chars, ProportionalFont,
    TeamOverlayUi, UiSnapshot, UiVertex, TEAM_ICON_CELL, TEAM_ICON_COLUMNS,
    TEAM_ICON_TEXTURE_SOURCE,
};

pub(in crate::ui) fn player_icon_atlas_cell(
    out: &mut Vec<UiVertex>,
    icon_count: usize,
    index: Option<u16>,
    x: f32,
    y: f32,
    width_px: f32,
    height_px: f32,
    w: u32,
    h: u32,
) {
    let Some(index) = index.map(usize::from).filter(|&i| i < icon_count) else {
        return;
    };
    let columns = TEAM_ICON_COLUMNS.max(1);
    let rows = ((icon_count as u32 + columns - 1) / columns).max(1);
    let cell = TEAM_ICON_CELL as f32;
    let atlas_w = columns as f32 * cell;
    let atlas_h = rows as f32 * cell;
    let col = index as u32 % columns;
    let row = index as u32 / columns;
    let uv0 = [
        (col as f32 * cell + 0.5) / atlas_w,
        (row as f32 * cell + 0.5) / atlas_h,
    ];
    let uv1 = [
        ((col + 1) as f32 * cell - 0.5) / atlas_w,
        ((row + 1) as f32 * cell - 0.5) / atlas_h,
    ];
    textured_rect_with_source(
        out,
        x,
        y,
        width_px,
        height_px,
        uv0,
        uv1,
        [1.0; 4],
        TEAM_ICON_TEXTURE_SOURCE,
        w,
        h,
    );
}

pub(in crate::ui) fn team_overlay_icon(
    out: &mut Vec<UiVertex>,
    overlay: &TeamOverlayUi,
    index: Option<u16>,
    x: f32,
    y: f32,
    width_px: f32,
    height_px: f32,
    w: u32,
    h: u32,
) {
    player_icon_atlas_cell(
        out,
        overlay.icon_paths.len(),
        index,
        x,
        y,
        width_px,
        height_px,
        w,
        h,
    );
}

pub(in crate::ui) fn team_overlay_health_color(health: i32, armor: i32) -> [f32; 4] {
    // OpenJK CG_GetColorForHealth. ARMOR_PROTECTION is 0.5 in JKA, so useful
    // armor is capped to health before selecting the classic red/yellow/white tint.
    if health <= 0 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let effective = health.saturating_add(armor.max(0).min(health));
    let blue = if effective >= 100 {
        1.0
    } else if effective < 66 {
        0.0
    } else {
        (effective - 66) as f32 / 33.0
    };
    let green = if effective > 60 {
        1.0
    } else if effective < 30 {
        0.0
    } else {
        (effective - 30) as f32 / 30.0
    };
    [1.0, green, blue, 1.0]
}

pub(in crate::ui) fn team_overlay_force_color(force: i32) -> [f32; 4] {
    // TaystJK/OpenJK CG_GetColorForForce.
    if force <= 0 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let red = if force >= 100 {
        1.0
    } else if force < 66 {
        0.0
    } else {
        (force - 66) as f32 / 33.0
    };
    let green = if force > 60 {
        1.0
    } else if force < 30 {
        0.0
    } else {
        (force - 30) as f32 / 30.0
    };
    [red, green, 1.0, 1.0]
}

pub(in crate::ui) fn build_team_overlay(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    small_font: Option<&ProportionalFont>,
    w: u32,
    h: u32,
) {
    let Some(overlay) = &ui.team_overlay else {
        return;
    };
    if overlay.settings.mode == 0 || overlay.entries.is_empty() {
        return;
    }
    let (sx, sy) = (w as f32 / 640.0, h as f32 / 480.0);
    // TaystJK cgs.widthRatioCoef keeps horizontal HUD dimensions at a 4:3
    // physical scale on widescreen displays. Keep it in virtual-space math,
    // then apply the renderer's ordinary 640x480 -> framebuffer transform.
    let ratio = crate::lagometer::width_ratio_coef(w, h);
    let draw_force = overlay.settings.force && overlay.entries.iter().any(|entry| entry.force >= 0);

    match overlay.settings.mode {
        1 | 2 => {
            // TaystJK CG_DrawTeamOverlay: fixed 8x8 text, max 12-char names and
            // 16-char locations, right anchored by cg_drawTeamOverlayX/Y.
            const CW: f32 = 8.0;
            const CH: f32 = 8.0;
            let pwidth = overlay
                .entries
                .iter()
                .map(|e| visible_jka_chars(&e.name))
                .max()
                .unwrap_or(0)
                .min(12);
            let lwidth = overlay.location_width.min(16);
            let mut width_v = (pwidth + lwidth + 11) as f32 * CW * ratio;
            if overlay.settings.weapons {
                width_v += CW * ratio;
            }
            // TaystJK's jaPRO force extension reserves a literal 32 virtual
            // units beyond the ratio-corrected stock overlay width.
            if draw_force {
                width_v += 32.0;
            }
            let x_v = overlay.settings.x as f32 - width_v;
            let y_v = if overlay.settings.y != 0 {
                overlay.settings.y as f32
            } else {
                0.0
            };
            let background = if overlay.team == 1 {
                [1.0, 0.0, 0.0, 0.33]
            } else {
                [0.0, 0.0, 1.0, 0.33]
            };
            rect(
                out,
                x_v * sx,
                y_v * sy,
                width_v * sx,
                overlay.entries.len() as f32 * CH * sy,
                background,
                w,
                h,
            );

            for (row, entry) in overlay.entries.iter().enumerate() {
                let y = (y_v + row as f32 * CH) * sy;
                let name = truncate_jka_text(&entry.name, 12);
                let location = truncate_jka_text(&entry.location, 16);
                let cw_x = CW * ratio;
                fixed_charset_text(
                    out,
                    &name,
                    (x_v + cw_x) * sx,
                    y,
                    cw_x * sx,
                    CH * sy,
                    cw_x * sx,
                    [1.0; 4],
                    false,
                    w,
                    h,
                );
                if lwidth > 0 {
                    fixed_charset_text(
                        out,
                        &location,
                        (x_v + cw_x * 2.0 + pwidth as f32 * cw_x) * sx,
                        y,
                        cw_x * sx,
                        CH * sy,
                        cw_x * sx,
                        [1.0; 4],
                        false,
                        w,
                        h,
                    );
                }
                let stats_x_v = x_v + cw_x * 3.0 + (pwidth + lwidth) as f32 * cw_x;
                let stats = format!("{:>3} {:>3}", entry.health.max(0), entry.armor.max(0));
                fixed_charset_text(
                    out,
                    &stats,
                    stats_x_v * sx,
                    y,
                    cw_x * sx,
                    CH * sy,
                    cw_x * sx,
                    team_overlay_health_color(entry.health, entry.armor),
                    false,
                    w,
                    h,
                );
                let mut icon_x_v = stats_x_v + cw_x * 7.0;
                if draw_force {
                    let fp = format!("{:>3}", entry.force.max(0));
                    fixed_charset_text(
                        out,
                        &fp,
                        (stats_x_v + 66.0 * ratio) * sx,
                        y,
                        cw_x * sx,
                        CH * sy,
                        cw_x * sx,
                        team_overlay_force_color(entry.force),
                        false,
                        w,
                        h,
                    );
                    icon_x_v = stats_x_v + 66.0 * ratio + cw_x * 4.0;
                }
                if overlay.settings.weapons {
                    team_overlay_icon(
                        out,
                        overlay,
                        entry.weapon_icon,
                        icon_x_v * sx,
                        y,
                        cw_x * sx,
                        CH * sy,
                        w,
                        h,
                    );
                }
                let mut pw_x_v = x_v;
                for &icon in &entry.powerup_icons {
                    team_overlay_icon(
                        out,
                        overlay,
                        Some(icon),
                        pw_x_v * sx,
                        y,
                        cw_x * sx,
                        CH * sy,
                        w,
                        h,
                    );
                    pw_x_v -= cw_x;
                }
            }
        }
        3 | 4 => {
            // TaystJK CG_DrawTeamOverlay2: four cards across the right half, then
            // a second row, with name/location, H/A(/F) and optional icons.
            let text_h_v = small_font.map_or(11.0, |font| font.height.max(1) as f32 * 0.55);
            let has_locations = overlay.has_locations;
            let card_h_v = if has_locations {
                text_h_v * 3.0 + 10.0
            } else {
                text_h_v * 2.0 + 7.5
            };
            let gap_v = 3.0 * ratio;
            // CG_DrawTeamOverlay2 sets overlayXPos to SCREEN_WIDTH/2 in the normal
            // upper-right HUD path and ratio-corrects both the half-screen width
            // and its four 3px gutters before /4.
            let card_w_v = (320.0 * ratio - 4.0 * gap_v) / 4.0;
            let origin_y_v = 0.0;
            let right_v = 640.0;
            for (i, entry) in overlay.entries.iter().enumerate() {
                let col = i % 4;
                let row = i / 4;
                let x_v = right_v - (col + 1) as f32 * card_w_v - col as f32 * gap_v;
                let y_v = origin_y_v + row as f32 * (card_h_v + 8.0);
                let bg = if entry.health < 1 {
                    [0.4, 0.4, 0.4, 0.4]
                } else if overlay.team == 1 {
                    [0.65, 0.01, 0.02, 0.70]
                } else {
                    [0.02, 0.40, 0.65, 0.70]
                };
                rect(
                    out,
                    x_v * sx,
                    y_v * sy,
                    card_w_v * sx,
                    card_h_v * sy,
                    bg,
                    w,
                    h,
                );
                let name = truncate_jka_text(&entry.name, 16);
                let loc = truncate_jka_text(&entry.location, 16);
                let name_y_v = if has_locations {
                    y_v + card_h_v / 4.0 - text_h_v / 2.0 - 2.5
                } else {
                    y_v + card_h_v / 3.0 - text_h_v / 2.0 - 2.5
                };
                if let Some(font) = small_font {
                    let name_w_v = proportional_text_width(&name, font, 0.55);
                    proportional_text(
                        out,
                        &name,
                        font,
                        (x_v + card_w_v / 2.0 - name_w_v / 2.0) * sx,
                        name_y_v * sy,
                        0.55,
                        [1.0; 4],
                        false,
                        w,
                        h,
                    );
                    if has_locations {
                        let loc_w_v = proportional_text_width(&loc, font, 0.55);
                        proportional_text(
                            out,
                            &loc,
                            font,
                            (x_v + card_w_v / 2.0 - loc_w_v / 2.0) * sx,
                            (y_v + card_h_v / 2.0 - text_h_v / 1.4) * sy,
                            0.55,
                            [1.0; 4],
                            false,
                            w,
                            h,
                        );
                    }
                } else {
                    let name_w_v = visible_jka_chars(&name) as f32 * 5.0;
                    fixed_charset_text(
                        out,
                        &name,
                        (x_v + card_w_v / 2.0 - name_w_v / 2.0) * sx,
                        name_y_v * sy,
                        5.0 * sx,
                        8.0 * sy,
                        5.0 * sx,
                        [1.0; 4],
                        false,
                        w,
                        h,
                    );
                    if has_locations {
                        let loc_w_v = visible_jka_chars(&loc) as f32 * 5.0;
                        fixed_charset_text(
                            out,
                            &loc,
                            (x_v + card_w_v / 2.0 - loc_w_v / 2.0) * sx,
                            (y_v + card_h_v / 2.0 - text_h_v / 1.4) * sy,
                            5.0 * sx,
                            8.0 * sy,
                            5.0 * sx,
                            [1.0; 4],
                            false,
                            w,
                            h,
                        );
                    }
                }
                let stats_y_v = if has_locations {
                    y_v + 3.0 * card_h_v / 4.0 - text_h_v / 2.0
                } else {
                    y_v + 2.0 * card_h_v / 3.0 - text_h_v / 2.0
                };
                let elements = if draw_force { 3.0 } else { 2.0 };
                let health_color = team_overlay_health_color(entry.health, entry.armor);
                let values = [
                    (format!("{:>3}", entry.health.max(0)), health_color),
                    (format!("{:>3}", entry.armor.max(0)), health_color),
                    (
                        format!("{:>3}", entry.force.max(0)),
                        team_overlay_force_color(entry.force),
                    ),
                ];
                for (slot, (value, color)) in values.into_iter().enumerate() {
                    if slot == 2 && !draw_force {
                        break;
                    }
                    let value_w_v = if let Some(font) = small_font {
                        proportional_text_width(&value, font, 0.55)
                    } else {
                        visible_jka_chars(&value) as f32 * 5.0
                    };
                    let tx_v =
                        x_v + (slot as f32 + 1.0) * card_w_v / (elements + 1.0) - value_w_v / 2.0;
                    if let Some(font) = small_font {
                        proportional_text(
                            out,
                            &value,
                            font,
                            tx_v * sx,
                            stats_y_v * sy,
                            0.55,
                            color,
                            false,
                            w,
                            h,
                        );
                    } else {
                        fixed_charset_text(
                            out,
                            &value,
                            tx_v * sx,
                            stats_y_v * sy,
                            5.0 * sx,
                            8.0 * sy,
                            5.0 * sx,
                            color,
                            false,
                            w,
                            h,
                        );
                    }
                }
                let mut ix_v = x_v;
                let iy_v = (y_v + card_h_v) * sy;
                if overlay.settings.weapons {
                    team_overlay_icon(
                        out,
                        overlay,
                        entry.weapon_icon,
                        ix_v * sx,
                        iy_v,
                        8.0 * ratio * sx,
                        8.0 * sy,
                        w,
                        h,
                    );
                    ix_v += 8.0 * ratio;
                }
                for &icon in &entry.powerup_icons {
                    team_overlay_icon(
                        out,
                        overlay,
                        Some(icon),
                        ix_v * sx,
                        iy_v,
                        8.0 * ratio * sx,
                        8.0 * sy,
                        w,
                        h,
                    );
                    ix_v += 8.0 * ratio;
                }
            }
        }
        5 | 6 => {
            // TaystJK CG_DrawTeamOverlay3. All geometry derives from scale and
            // CG_Text_Height; use the resident small font's metrics where available.
            let scale = overlay.settings.scale.clamp(0.5, 2.5);
            let text_scale = 0.8 * scale;
            let text_h_v =
                small_font.map_or(12.0 * scale, |font| font.height.max(1) as f32 * text_scale);
            let bar_h_v = text_h_v * 0.55;
            let pad_v = 3.0 * scale;
            let pad_x_v = pad_v * ratio;
            let has_locations = overlay.has_locations;
            let row_h_v = pad_v
                + text_h_v
                + pad_v
                + bar_h_v
                + pad_v
                + if has_locations { text_h_v + pad_v } else { 0.0 };
            let panel_w_v = 190.0 * scale * ratio;
            let panel_x_v = overlay.settings.x as f32 - panel_w_v;
            let panel_y_v = if overlay.settings.y != 0 {
                overlay.settings.y as f32
            } else {
                0.0
            };
            let icon_size_v = row_h_v - pad_v * 2.0;
            let icon_w_v = icon_size_v * ratio;
            let pw_icon_w_v = text_h_v * ratio;
            let icon_x_v = panel_x_v + pad_x_v;
            let text_x_v = icon_x_v + icon_w_v + pad_x_v;
            let mut bar_w_v = panel_x_v + panel_w_v - pad_x_v - text_x_v;
            let fp_bar_w_v = if draw_force { bar_w_v * 0.22 } else { 0.0 };
            if draw_force {
                bar_w_v -= fp_bar_w_v + pad_x_v;
            }
            let fp_bar_x_v = text_x_v + bar_w_v + pad_x_v;
            let max_hp = overlay.settings.max_hp.max(1.0);

            for (row, entry) in overlay.entries.iter().enumerate() {
                let y_v = panel_y_v + row as f32 * row_h_v;
                let bg = if entry.health < 1 {
                    [0.35, 0.35, 0.35, 0.50]
                } else if overlay.team == 1 {
                    [0.45, 0.05, 0.05, 0.35]
                } else {
                    [0.05, 0.15, 0.45, 0.35]
                };
                rect(
                    out,
                    panel_x_v * sx,
                    y_v * sy,
                    panel_w_v * sx,
                    row_h_v * sy,
                    bg,
                    w,
                    h,
                );
                team_overlay_icon(
                    out,
                    overlay,
                    entry.model_icon,
                    icon_x_v * sx,
                    (y_v + pad_v) * sy,
                    icon_w_v * sx,
                    icon_size_v * sy,
                    w,
                    h,
                );

                let baseline = (y_v + pad_v + text_h_v * 0.82) * sy;
                let total = entry.health.max(0).saturating_add(entry.armor.max(0));
                let total_text = total.to_string();
                let total_w_v = if let Some(font) = small_font {
                    proportional_text_width(&total_text, font, text_scale)
                } else {
                    visible_jka_chars(&total_text) as f32 * 5.0 * scale
                };
                let total_x_v = panel_x_v + panel_w_v - pad_x_v - total_w_v;
                let mut text_color = [1.0; 4];
                if entry.health < 1 {
                    text_color[3] = 0.4;
                }
                if let Some(font) = small_font {
                    proportional_text(
                        out,
                        &total_text,
                        font,
                        total_x_v * sx,
                        baseline,
                        text_scale,
                        text_color,
                        false,
                        w,
                        h,
                    );
                } else {
                    fixed_charset_text(
                        out,
                        &total_text,
                        total_x_v * sx,
                        (y_v + pad_v) * sy,
                        5.0 * scale * sx,
                        8.0 * scale * sy,
                        5.0 * scale * sx,
                        text_color,
                        false,
                        w,
                        h,
                    );
                }

                // Powerups and the optional weapon consume space from right to left,
                // immediately before the total, exactly like CG_DrawTeamOverlay3.
                let mut pw_x_v = total_x_v - pad_x_v;
                for &icon in &entry.powerup_icons {
                    if pw_x_v - pw_icon_w_v < text_x_v {
                        break;
                    }
                    pw_x_v -= pw_icon_w_v;
                    team_overlay_icon(
                        out,
                        overlay,
                        Some(icon),
                        pw_x_v * sx,
                        (y_v + pad_v) * sy,
                        pw_icon_w_v * sx,
                        text_h_v * sy,
                        w,
                        h,
                    );
                }
                if overlay.settings.weapons && pw_x_v - pw_icon_w_v >= text_x_v {
                    pw_x_v -= pw_icon_w_v;
                    team_overlay_icon(
                        out,
                        overlay,
                        entry.weapon_icon,
                        pw_x_v * sx,
                        (y_v + pad_v) * sy,
                        pw_icon_w_v * sx,
                        text_h_v * sy,
                        w,
                        h,
                    );
                }

                let name_w_v = (pw_x_v - text_x_v - pad_x_v).max(0.0);
                let mut name_len = visible_jka_chars(&entry.name).min(36);
                let mut name = truncate_jka_text(&entry.name, name_len);
                loop {
                    let width_v = if let Some(font) = small_font {
                        proportional_text_width(&name, font, text_scale)
                    } else {
                        visible_jka_chars(&name) as f32 * 5.0 * scale
                    };
                    if name_len <= 1 || width_v <= name_w_v {
                        break;
                    }
                    name_len -= 1;
                    name = truncate_jka_text(&entry.name, name_len);
                }
                if let Some(font) = small_font {
                    proportional_text(
                        out,
                        &name,
                        font,
                        text_x_v * sx,
                        baseline,
                        text_scale,
                        text_color,
                        true,
                        w,
                        h,
                    );
                } else {
                    fixed_charset_text(
                        out,
                        &name,
                        text_x_v * sx,
                        (y_v + pad_v) * sy,
                        5.0 * scale * sx,
                        8.0 * scale * sy,
                        5.0 * scale * sx,
                        text_color,
                        true,
                        w,
                        h,
                    );
                }

                let mut bars_y_v = y_v + pad_v + text_h_v + pad_v;
                if has_locations {
                    let loc = truncate_jka_text(&entry.location, 16);
                    if let Some(font) = small_font {
                        proportional_text(
                            out,
                            &loc,
                            font,
                            text_x_v * sx,
                            (bars_y_v + text_h_v * 0.82) * sy,
                            text_scale * 0.8,
                            text_color,
                            false,
                            w,
                            h,
                        );
                    } else {
                        fixed_charset_text(
                            out,
                            &loc,
                            text_x_v * sx,
                            bars_y_v * sy,
                            5.0 * scale * sx,
                            8.0 * scale * sy,
                            5.0 * scale * sx,
                            text_color,
                            false,
                            w,
                            h,
                        );
                    }
                    bars_y_v += text_h_v + pad_v;
                }
                rect(
                    out,
                    text_x_v * sx,
                    bars_y_v * sy,
                    bar_w_v * sx,
                    bar_h_v * sy,
                    [0.0, 0.0, 0.0, 0.55],
                    w,
                    h,
                );
                let health = entry.health.max(0) as f32;
                let armor = entry.armor.max(0) as f32;
                if health > 0.0 {
                    let health_w = (health / max_hp).clamp(0.0, 1.0) * bar_w_v;
                    let armor_w =
                        ((armor / max_hp).max(0.0) * bar_w_v).min((bar_w_v - health_w).max(0.0));
                    let mut health_color =
                        team_overlay_health_color((health + armor).min(100.0) as i32, 0);
                    health_color[3] = 0.90;
                    rect(
                        out,
                        text_x_v * sx,
                        bars_y_v * sy,
                        health_w * sx,
                        bar_h_v * sy,
                        health_color,
                        w,
                        h,
                    );
                    if armor > 0.0 && health_w < bar_w_v {
                        rect(
                            out,
                            (text_x_v + health_w) * sx,
                            bars_y_v * sy,
                            armor_w * sx,
                            bar_h_v * sy,
                            [0.20, 0.85, 0.30, 0.90],
                            w,
                            h,
                        );
                    }
                    if health + armor > max_hp {
                        let over = bar_w_v * 0.04;
                        rect(
                            out,
                            (text_x_v + bar_w_v - over) * sx,
                            bars_y_v * sy,
                            over * sx,
                            bar_h_v * sy,
                            [1.0, 1.0, 1.0, 0.90],
                            w,
                            h,
                        );
                    }
                }
                if draw_force {
                    rect(
                        out,
                        fp_bar_x_v * sx,
                        bars_y_v * sy,
                        fp_bar_w_v * sx,
                        bar_h_v * sy,
                        [0.0, 0.0, 0.0, 0.55],
                        w,
                        h,
                    );
                    if health > 0.0 && entry.force > 0 {
                        let fw = (entry.force.clamp(0, 100) as f32 / 100.0) * fp_bar_w_v;
                        let mut force_color = team_overlay_force_color(entry.force);
                        force_color[3] = 0.92;
                        rect(
                            out,
                            fp_bar_x_v * sx,
                            bars_y_v * sy,
                            fw * sx,
                            bar_h_v * sy,
                            force_color,
                            w,
                            h,
                        );
                    }
                }
            }
        }
        _ => {}
    }
}

pub(in crate::ui) fn build_prediction_debug(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    w: u32,
    h: u32,
) {
    let Some(debug) = &ui.prediction_debug else {
        return;
    };

    if debug.flash {
        // A border instead of a full-screen wash keeps the world readable while
        // still making a one/few-frame prediction discontinuity impossible to miss.
        let t = 8.0_f32.min((w.min(h) as f32 * 0.02).max(3.0));
        let c = [1.0, 0.05, 0.02, 0.72];
        rect(out, 0.0, 0.0, w as f32, t, c, w, h);
        rect(out, 0.0, h as f32 - t, w as f32, t, c, w, h);
        rect(out, 0.0, 0.0, t, h as f32, c, w, h);
        rect(out, w as f32 - t, 0.0, t, h as f32, c, w, h);
    }

    if !debug.show_panel {
        return;
    }

    let x = 18.0;
    // Keep clear of the simple/detailed FPS block in the upper-left.
    let y = if ui.video.draw_fps == 0 { 18.0 } else { 92.0 };
    let line_h = 10.0;
    let line_count = debug.lines.len().max(1) as f32;
    let panel_w = (w as f32 * 0.72).clamp(420.0, 980.0);
    let panel_h = 34.0 + line_count * line_h;
    rect(
        out,
        x - 8.0,
        y - 7.0,
        panel_w,
        panel_h,
        [0.0, 0.0, 0.0, 0.68],
        w,
        h,
    );
    rect(
        out,
        x - 8.0,
        y - 7.0,
        3.0,
        panel_h,
        [0.9, 0.18, 0.05, 0.95],
        w,
        h,
    );

    let heading = debug.last_miss.map_or_else(
        || {
            format!(
                "PREDICTION DIAGNOSTICS  threshold {:.1}u  no miss captured yet",
                debug.threshold
            )
        },
        |miss| {
            format!(
                "PREDICTION DIAGNOSTICS  last miss {:.2}u  threshold {:.1}u",
                miss, debug.threshold
            )
        },
    );
    text(out, &heading, x, y, 1.0, [1.0, 0.72, 0.45, 1.0], w, h);
    for (index, line) in debug.lines.iter().enumerate() {
        text(
            out,
            line,
            x,
            y + 13.0 + index as f32 * line_h,
            0.9,
            [0.92, 0.94, 0.98, 1.0],
            w,
            h,
        );
    }
}
