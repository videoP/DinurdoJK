//! Hud draw.
use crate::extensions::NativeHud;
use crate::ui::{
    hud_element_rect, rect, rect_gradient, text, with_alpha, HudElementId, HudRect, HudRectContext,
    UiSnapshot, UiVertex,
};

pub(in crate::ui) fn lighten(color: [f32; 4], amount: f32) -> [f32; 4] {
    [
        color[0] + (1.0 - color[0]) * amount,
        color[1] + (1.0 - color[1]) * amount,
        color[2] + (1.0 - color[2]) * amount,
        color[3],
    ]
}

/// OpenJK `saber_styles_t` as shown on the HUD: name, accent, lit segments (of 3).
pub(in crate::ui) fn saber_style_display(style: i32) -> (&'static str, [f32; 4], u32) {
    match style {
        1 => ("FAST", [0.35, 0.72, 1.0, 1.0], 1),
        2 => ("MEDIUM", [1.0, 0.85, 0.30, 1.0], 2),
        3 => ("STRONG", [1.0, 0.28, 0.36, 1.0], 3),
        4 => ("DESANN", [1.0, 0.30, 0.36, 1.0], 3),
        5 => ("TAVION", [0.75, 0.45, 1.0, 1.0], 3),
        6 => ("DUAL", [0.40, 1.0, 0.65, 1.0], 3),
        7 => ("STAFF", [1.0, 0.58, 0.25, 1.0], 3),
        _ => ("--", HUD_LABEL(), 0),
    }
}

/// jaPRO/TaystJK cg_tintHud from CG_DrawHUD. This colors the native status
/// frames/tics, not health/armor numeric readouts, team overlays or selectors.
/// JK2 server gametypes are remapped to JKA numbering before reaching HudState.
pub(in crate::ui) fn native_hud_tint(hud: &crate::ui::HudState, enabled: bool) -> [f32; 4] {
    if !enabled || hud.spectating || hud.gametype < crate::cgame::GT_TEAM
        || hud.gametype == crate::cgame::GT_SIEGE
    {
        return [1.0; 4];
    }
    match hud.team {
        crate::cgame::TEAM_RED => [1.0, 0.5, 0.5, 1.0],
        crate::cgame::TEAM_BLUE => [0.5, 0.5, 1.0, 1.0],
        _ => [1.0; 4],
    }
}

pub(in crate::ui) fn build_hud(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let Some(hud) = ui.hud else {
        return;
    };

    if ui.video.hud_style == 1 {
        if ui.jk2_mode {
            super::jk2_retail::build(out, ui, w, h);
        } else {
            super::retail::build(out, ui, w, h);
        }
        return;
    }
    let ctx = HudRectContext::from_snapshot(ui);
    // Custom panels have stat-coloured meters; only their chrome takes the
    // team tint, preserving health, shield, ammo and force readability.
    let team_tint = native_hud_tint(&hud, ui.video.tint_hud);
    let frame_tint = (team_tint != [1.0; 4]).then_some(team_tint);
    if !ui.native_replaced(NativeHud::Health) {
        let health_layout = ui.hud_layout.health;
        let health = hud_element_rect(HudElementId::Health, health_layout, &ctx, w, h);
        let low_health = hud.health > 0 && hud.health * 4 <= hud.max_health.max(1);
        hud_meter(
            out,
            health,
            health_layout.scale,
            "HEALTH",
            hud.health,
            hud.max_health.max(1),
            if low_health {
                HUD_HEALTH_LOW
            } else {
                HUD_HEALTH
            },
            frame_tint,
            w,
            h,
        );
    }

    if !ui.native_replaced(NativeHud::Shield) {
        let shield_layout = ui.hud_layout.shield;
        let shield = hud_element_rect(HudElementId::Shield, shield_layout, &ctx, w, h);
        hud_meter(
            out,
            shield,
            shield_layout.scale,
            "SHIELD",
            hud.armor,
            hud.max_health.max(1),
            HUD_SHIELD,
            frame_tint,
            w,
            h,
        );
    }

    if !ui.native_replaced(NativeHud::Ammo) {
        // The ammo slot doubles as the saber-style readout while the saber is out.
        let ammo_layout = ui.hud_layout.ammo;
        let ammo = hud_element_rect(HudElementId::Ammo, ammo_layout, &ctx, w, h);
        // HudState.weapon is JK2-numbered in JK2 mode (WP_SABER is 2 there).
        if hud.weapon == if ui.jk2_mode { 2 } else { HUD_WP_SABER } {
            let style_pending = hud.saber_style != 0
                && hud.saber_active_style != 0
                && hud.saber_style != hud.saber_active_style;
            hud_style_panel(
                out,
                ammo,
                ammo_layout.scale,
                hud.saber_style,
                style_pending,
                frame_tint,
                w,
                h,
            );
        } else {
            let ammo_text = hud
                .ammo
                .map_or_else(|| "--".to_owned(), |value| value.max(0).to_string());
            hud_value_panel(
                out,
                ammo,
                ammo_layout.scale,
                "AMMO",
                &ammo_text,
                HUD_AMMO,
                frame_tint,
                w,
                h,
            );
        }
    }

    if !ui.native_replaced(NativeHud::Force) {
        let force_layout = ui.hud_layout.force;
        let force = hud_element_rect(HudElementId::Force, force_layout, &ctx, w, h);
        if let Some(force_power) = hud.force_power {
            hud_meter(
                out,
                force,
                force_layout.scale,
                "FORCE",
                force_power,
                hud.force_power_max.max(1),
                if hud.force_flash {
                    [1.0, 0.15, 0.15, 1.0]
                } else {
                    HUD_FORCE
                },
                frame_tint,
                w,
                h,
            );
        } else {
            hud_value_panel(
                out,
                force,
                force_layout.scale,
                "FORCE",
                "--",
                HUD_FORCE,
                frame_tint,
                w,
                h,
            );
        }
    }
}

#[cfg(test)]
mod tint_hud_tests {
    use super::*;
    use crate::ui::HudState;

    #[test]
    fn tint_hud_matches_taystjk_team_gates_in_both_native_modes() {
        let mut state = HudState { gametype: 6, team: 1, ..Default::default() };
        assert_eq!(native_hud_tint(&state, true), [1.0, 0.5, 0.5, 1.0]);
        state.team = 2;
        assert_eq!(native_hud_tint(&state, true), [0.5, 0.5, 1.0, 1.0]);
        assert_eq!(native_hud_tint(&state, false), [1.0; 4]);
        state.gametype = 3; // duel / FFA, even if stale team assignment survives
        assert_eq!(native_hud_tint(&state, true), [1.0; 4]);
        state.gametype = 7; // Siege explicitly excluded by TaystJK
        assert_eq!(native_hud_tint(&state, true), [1.0; 4]);
        state.gametype = 8; // CTF (JK2 7 -> JKA 8 in the presentation adapter)
        assert_eq!(native_hud_tint(&state, true), [0.5, 0.5, 1.0, 1.0]);
        state.spectating = true;
        assert_eq!(native_hud_tint(&state, true), [1.0; 4]);
        state.spectating = false;
        state.team = 3;
        assert_eq!(native_hud_tint(&state, true), [1.0; 4]);
    }

    #[test]
    fn custom_style_only_tints_chrome_and_works_for_both_protocols() {
        for jk2 in [false, true] {
            let mut ui = UiSnapshot::default();
            ui.jk2_mode = jk2;
            ui.hud = Some(HudState {
                gametype: 6, team: 1, health: 100, max_health: 100,
                armor: 50, weapon: if jk2 { 2 } else { 3 },
                force_power: Some(75), saber_style: 2, ..Default::default()
            });
            ui.video.hud_style = 0;
            ui.video.tint_hud = false;
            let mut untinted = Vec::new();
            build_hud(&mut untinted, &ui, 1280, 720);
            ui.video.tint_hud = true;
            let mut tinted = Vec::new();
            build_hud(&mut tinted, &ui, 1280, 720);
            assert_eq!(untinted.len(), tinted.len());
            assert_ne!(untinted[6].color, tinted[6].color,
                "{jk2}: status panel chrome should change with cg_tintHud");
            assert_eq!(untinted.last().unwrap().color, tinted.last().unwrap().color,
                "{jk2}: force numeric/stat colors must remain unchanged");
        }
    }
}

/// `WP_SABER` in OpenJK's `weapon_t`.
pub(in crate::ui) const HUD_WP_SABER: i32 = 3;

/// Panel backing shared by every HUD readout: ink gradient, an accent notch on
/// the left edge and a faint accent hairline along the top.
pub(in crate::ui) fn hud_frame(
    out: &mut Vec<UiVertex>,
    rect_: HudRect,
    scale: f32,
    accent: [f32; 4],
    w: u32,
    h: u32,
) {
    hud_frame_with_accent_alpha(out, rect_, scale, accent, 1.0, w, h);
}

/// Shared panel backing with an independently fadeable accent. The backing
/// stays stable while a queued saber style fades only its style-specific cues.
#[allow(clippy::too_many_arguments)]
fn hud_frame_with_accent_alpha(
    out: &mut Vec<UiVertex>,
    rect_: HudRect,
    scale: f32,
    accent: [f32; 4],
    accent_alpha: f32,
    w: u32,
    h: u32,
) {
    let s = scale.clamp(0.5, 2.0);
    let accent_alpha = accent_alpha.clamp(0.0, 1.0);
    rect_gradient(
        out,
        rect_.x,
        rect_.y,
        rect_.width,
        rect_.height,
        HUD_INK_TOP(),
        HUD_INK_TOP(),
        HUD_INK_BOTTOM(),
        HUD_INK_BOTTOM(),
        w,
        h,
    );
    rect(
        out,
        rect_.x,
        rect_.y,
        rect_.width,
        1.0,
        with_alpha(accent, 0.22 * accent_alpha),
        w,
        h,
    );
    rect(
        out,
        rect_.x,
        rect_.y,
        3.0 * s,
        rect_.height,
        with_alpha(accent, 0.95 * accent_alpha),
        w,
        h,
    );
}

/// Label top-left and value in a fixed left-aligned numeric column.  Do not
/// move HP/ammo/force text horizontally when the number of digits changes.
#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn hud_texts(
    out: &mut Vec<UiVertex>,
    rect_: HudRect,
    scale: f32,
    label: &str,
    value: &str,
    value_color: [f32; 4],
    w: u32,
    h: u32,
) {
    let s = scale.clamp(0.5, 2.0);
    text(
        out,
        label,
        rect_.x + 10.0 * s,
        rect_.y + 5.5 * s,
        1.15 * s,
        HUD_LABEL(),
        w,
        h,
    );
    let value_scale = 1.6 * s;
    // Reserve four normal-width glyph cells (up to 9999).  A varying label
    // must never change the screen x of the first digit; longer words (saber
    // style names) widen the column so they end inside the panel.
    const VALUE_COLUMN_CHARS: usize = 4;
    let value_chars = value.chars().count().max(VALUE_COLUMN_CHARS);
    let value_column_width = value_chars as f32 * 6.0 * value_scale;
    text(
        out,
        value,
        rect_.x + rect_.width - value_column_width - 8.0 * s,
        rect_.y + 4.0 * s,
        value_scale,
        value_color,
        w,
        h,
    );
}

#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn hud_meter(
    out: &mut Vec<UiVertex>,
    rect_: HudRect,
    scale: f32,
    label: &str,
    value: i32,
    maximum: i32,
    accent: [f32; 4],
    frame_tint: Option<[f32; 4]>,
    w: u32,
    h: u32,
) {
    let s = scale.clamp(0.5, 2.0);
    hud_frame(out, rect_, scale, frame_tint.unwrap_or(accent), w, h);
    let fraction = (value.max(0) as f32 / maximum.max(1) as f32).clamp(0.0, 1.0);
    let track_x = rect_.x + 10.0 * s;
    let track_w = rect_.width - 18.0 * s;
    let track_y = rect_.y + rect_.height - 6.0 * s;
    let track_h = 3.0 * s;
    rect(
        out,
        track_x,
        track_y,
        track_w,
        track_h,
        with_alpha(accent, 0.16),
        w,
        h,
    );
    let fill_w = track_w * fraction;
    if fill_w > 0.5 {
        let dim = with_alpha(accent, 0.55);
        rect_gradient(
            out, track_x, track_y, fill_w, track_h, dim, accent, dim, accent, w, h,
        );
        // Bright leading edge.
        let edge = (2.0 * s).min(fill_w);
        rect(
            out,
            track_x + fill_w - edge,
            track_y,
            edge,
            track_h,
            lighten(accent, 0.55),
            w,
            h,
        );
    }
    // Quarter ticks read as a scale without adding text.
    for quarter in 1..4 {
        rect(
            out,
            track_x + track_w * quarter as f32 / 4.0 - 0.5,
            track_y,
            1.0_f32.max(0.6 * s),
            track_h,
            [0.0, 0.0, 0.02, 0.6],
            w,
            h,
        );
    }
    let value_color = if accent == HUD_HEALTH_LOW {
        [1.0, 0.64, 0.60, 1.0]
    } else {
        HUD_VALUE()
    };
    hud_texts(
        out,
        rect_,
        scale,
        label,
        &value.max(0).to_string(),
        value_color,
        w,
        h,
    );
}

#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn hud_value_panel(
    out: &mut Vec<UiVertex>,
    rect_: HudRect,
    scale: f32,
    label: &str,
    value: &str,
    accent: [f32; 4],
    frame_tint: Option<[f32; 4]>,
    w: u32,
    h: u32,
) {
    let s = scale.clamp(0.5, 2.0);
    hud_frame(out, rect_, scale, frame_tint.unwrap_or(accent), w, h);
    rect(
        out,
        rect_.x + 10.0 * s,
        rect_.y + rect_.height - 6.0 * s,
        rect_.width - 18.0 * s,
        3.0 * s,
        with_alpha(accent, 0.34),
        w,
        h,
    );
    hud_texts(out, rect_, scale, label, value, HUD_VALUE(), w, h);
}

/// Saber form readout: style name plus three segments that light up with the
/// form's intensity (fast, medium, strong; the special forms fill all three).
pub(in crate::ui) fn hud_style_panel(
    out: &mut Vec<UiVertex>,
    rect_: HudRect,
    scale: f32,
    style: i32,
    pending: bool,
    frame_tint: Option<[f32; 4]>,
    w: u32,
    h: u32,
) {
    let s = scale.clamp(0.5, 2.0);
    let (name, accent, lit) = saber_style_display(style);
    // EternalJK exposes the queued style through saberDrawAnimLevel before
    // saberAnimLevel actually changes. Keep that immediate preview, but make
    // the distinction visible: queued = faded, active = fully lit.
    let pending_alpha = if pending { 0.48 } else { 1.0 };
    hud_frame_with_accent_alpha(out, rect_, scale, frame_tint.unwrap_or(accent), pending_alpha, w, h);
    let track_x = rect_.x + 10.0 * s;
    let track_w = rect_.width - 18.0 * s;
    let track_y = rect_.y + rect_.height - 6.0 * s;
    let track_h = 3.0 * s;
    let gap = 3.0 * s;
    let segment_w = (track_w - 2.0 * gap) / 3.0;
    for index in 0..3u32 {
        let segment_x = track_x + index as f32 * (segment_w + gap);
        if index < lit {
            let dim = with_alpha(accent, 0.6 * pending_alpha);
            let bright = with_alpha(accent, pending_alpha);
            rect_gradient(
                out,
                segment_x,
                track_y,
                segment_w,
                track_h,
                dim,
                bright,
                dim,
                bright,
                w,
                h,
            );
        } else {
            rect(
                out,
                segment_x,
                track_y,
                segment_w,
                track_h,
                with_alpha(accent, 0.16 * pending_alpha),
                w,
                h,
            );
        }
    }
    hud_texts(
        out,
        rect_,
        scale,
        "STYLE",
        name,
        with_alpha(lighten(accent, 0.25), pending_alpha),
        w,
        h,
    );
}

pub(in crate::ui) const CHATBOX_Y: f32 = 425.0;

pub(in crate::ui) const CHATBOX_FONT_HEIGHT: f32 = 20.0;

pub(in crate::ui) const CHATBOX_FONT_SCALE: f32 = 0.65 * 0.5;

pub(in crate::ui) const CHATBOX_CUTOFF: f32 = 550.0;

// Panel ink and captions come from `crate::palette` (warm Stone Brown ink,
// Silver labels, White Smoke values); the stat colours stay semantic.
#[allow(non_snake_case)]
pub(in crate::ui) fn HUD_INK_TOP() -> [f32; 4] { crate::palette::hud::HUD_INK_TOP() }

#[allow(non_snake_case)]
pub(in crate::ui) fn HUD_INK_BOTTOM() -> [f32; 4] { crate::palette::hud::HUD_INK_BOTTOM() }

#[allow(non_snake_case)]
pub(in crate::ui) fn HUD_LABEL() -> [f32; 4] { crate::palette::hud::HUD_LABEL() }

#[allow(non_snake_case)]
pub(in crate::ui) fn HUD_VALUE() -> [f32; 4] { crate::palette::hud::HUD_VALUE() }

pub(in crate::ui) const HUD_HEALTH: [f32; 4] = [0.92, 0.18, 0.15, 1.0];

pub(in crate::ui) const HUD_HEALTH_LOW: [f32; 4] = [1.0, 0.07, 0.05, 1.0];

pub(in crate::ui) const HUD_SHIELD: [f32; 4] = [0.26, 0.86, 0.36, 1.0];

pub(in crate::ui) const HUD_FORCE: [f32; 4] = [0.22, 0.50, 1.0, 1.0];

pub(in crate::ui) const HUD_AMMO: [f32; 4] = [1.0, 0.72, 0.28, 1.0];
