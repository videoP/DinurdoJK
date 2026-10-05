//! Hud draw.
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
        _ => ("--", HUD_LABEL, 0),
    }
}

pub(in crate::ui) fn build_hud(out: &mut Vec<UiVertex>, ui: &UiSnapshot, w: u32, h: u32) {
    let Some(hud) = ui.hud else {
        return;
    };

    let health_layout = ui.hud_layout.health;
    let ctx = HudRectContext::from_snapshot(ui);
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
        w,
        h,
    );

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
        w,
        h,
    );

    // The ammo slot doubles as the saber-style readout while the saber is out.
    let ammo_layout = ui.hud_layout.ammo;
    let ammo = hud_element_rect(HudElementId::Ammo, ammo_layout, &ctx, w, h);
    if hud.weapon == HUD_WP_SABER {
        hud_style_panel(out, ammo, ammo_layout.scale, hud.saber_style, w, h);
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
            w,
            h,
        );
    }

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
            w,
            h,
        );
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
    let s = scale.clamp(0.5, 2.0);
    rect_gradient(
        out,
        rect_.x,
        rect_.y,
        rect_.width,
        rect_.height,
        HUD_INK_TOP,
        HUD_INK_TOP,
        HUD_INK_BOTTOM,
        HUD_INK_BOTTOM,
        w,
        h,
    );
    rect(
        out,
        rect_.x,
        rect_.y,
        rect_.width,
        1.0,
        with_alpha(accent, 0.22),
        w,
        h,
    );
    rect(
        out,
        rect_.x,
        rect_.y,
        3.0 * s,
        rect_.height,
        with_alpha(accent, 0.95),
        w,
        h,
    );
}

/// Label top-left and value top-right, in the shared HUD type sizes.
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
        HUD_LABEL,
        w,
        h,
    );
    let value_scale = 1.6 * s;
    let value_width = value.len() as f32 * 6.0 * value_scale;
    text(
        out,
        value,
        rect_.x + rect_.width - value_width - 8.0 * s,
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
    w: u32,
    h: u32,
) {
    let s = scale.clamp(0.5, 2.0);
    hud_frame(out, rect_, scale, accent, w, h);
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
        HUD_VALUE
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
    w: u32,
    h: u32,
) {
    let s = scale.clamp(0.5, 2.0);
    hud_frame(out, rect_, scale, accent, w, h);
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
    hud_texts(out, rect_, scale, label, value, HUD_VALUE, w, h);
}

/// Saber form readout: style name plus three segments that light up with the
/// form's intensity (fast, medium, strong; the special forms fill all three).
pub(in crate::ui) fn hud_style_panel(
    out: &mut Vec<UiVertex>,
    rect_: HudRect,
    scale: f32,
    style: i32,
    w: u32,
    h: u32,
) {
    let s = scale.clamp(0.5, 2.0);
    let (name, accent, lit) = saber_style_display(style);
    hud_frame(out, rect_, scale, accent, w, h);
    let track_x = rect_.x + 10.0 * s;
    let track_w = rect_.width - 18.0 * s;
    let track_y = rect_.y + rect_.height - 6.0 * s;
    let track_h = 3.0 * s;
    let gap = 3.0 * s;
    let segment_w = (track_w - 2.0 * gap) / 3.0;
    for index in 0..3u32 {
        let segment_x = track_x + index as f32 * (segment_w + gap);
        if index < lit {
            let dim = with_alpha(accent, 0.6);
            rect_gradient(
                out, segment_x, track_y, segment_w, track_h, dim, accent, dim, accent, w, h,
            );
        } else {
            rect(
                out,
                segment_x,
                track_y,
                segment_w,
                track_h,
                with_alpha(accent, 0.16),
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
        lighten(accent, 0.25),
        w,
        h,
    );
}

pub(in crate::ui) const CHATBOX_Y: f32 = 425.0;

pub(in crate::ui) const CHATBOX_FONT_HEIGHT: f32 = 20.0;

pub(in crate::ui) const CHATBOX_FONT_SCALE: f32 = 0.65 * 0.5;

pub(in crate::ui) const CHATBOX_CUTOFF: f32 = 550.0;

// Night-Tokyo palette shared with the console: indigo ink, cyan/magenta neon,
// tungsten amber. HUD accents pick one role per panel.
pub(in crate::ui) const HUD_INK_TOP: [f32; 4] = [0.010, 0.014, 0.032, 0.84];

pub(in crate::ui) const HUD_INK_BOTTOM: [f32; 4] = [0.020, 0.026, 0.058, 0.80];

pub(in crate::ui) const HUD_LABEL: [f32; 4] = [0.56, 0.66, 0.79, 1.0];

pub(in crate::ui) const HUD_VALUE: [f32; 4] = [0.96, 0.98, 1.0, 1.0];

pub(in crate::ui) const HUD_HEALTH: [f32; 4] = [0.92, 0.18, 0.15, 1.0];

pub(in crate::ui) const HUD_HEALTH_LOW: [f32; 4] = [1.0, 0.07, 0.05, 1.0];

pub(in crate::ui) const HUD_SHIELD: [f32; 4] = [0.26, 0.86, 0.36, 1.0];

pub(in crate::ui) const HUD_FORCE: [f32; 4] = [0.22, 0.50, 1.0, 1.0];

pub(in crate::ui) const HUD_AMMO: [f32; 4] = [1.0, 0.72, 0.28, 1.0];
