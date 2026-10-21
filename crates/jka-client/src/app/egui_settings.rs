//! app::egui_settings module facade. Implementations are grouped by responsibility.
mod account;
mod blue_water;
mod clouds;
mod native_environment;
mod color;
mod debug;
mod display;
mod effects;
mod extensions;
mod streamer;
mod fog;
mod game;
mod geometry;
mod glass;
mod image;
mod interface;
mod lighting;
mod mod_options;
mod models;
mod optimizations;
mod physics;
mod preset_preview;
// Shared with quality::apply_quality_preset_to_page; keeps the cvar-to-page
// ownership mapping in one place.
pub(in crate::app) use preset_preview::preset_cvar_section;
pub(in crate::app) use optimizations::OPTIMIZATION_CVARS;
mod post;
mod reflections;
mod reset;
mod shadows;
mod sun;
mod water;
mod weather;
mod widgets;

// Setup -> Game/View and Video settings.
//
// The rail in [`super::egui_menu`] picks which [`VideoSection`] is shown for Video;
// this module also owns the compact Game/View page. Ordered video options (filtering,
// shadow technique, AO samples, …) use the shared quality meter so their cost
// ordering is visible, while unordered choices stay as plain segmented chips.
// Every label carries a tooltip explaining what the setting actually does.
//
// Every mutation still funnels through the same `change_video_setting` /
// `change_environment_setting` helpers the console and config loader use, so
// the renderer sync, dirty-marking and UI publish behave identically.

use super::egui_menu::VideoSection;
use super::egui_theme as theme;
use super::quality::QualityPreset;
use super::*;

// --------------------------------------------------------------- helpers --

/// Field ids carried by [`theme::Reset::Ocean`] and
/// [`theme::Reset::OceanCascade`]. Ocean settings live in a plain struct rather
/// than the indexed row tables the rest of the page uses, so they get their own
/// compact numbering.
const PHYS_CLIENT_ENABLED: u8 = 0;
const PHYS_RATE: u8 = 1;
const PHYS_MAX_SUBSTEPS: u8 = 2;
const PHYS_CCD: u8 = 3;
const PHYS_SLEEPING: u8 = 4;
const PHYS_RAGDOLLS: u8 = 5;
const PHYS_RAGDOLL_MAX: u8 = 6;
const PHYS_RAGDOLL_LIFETIME: u8 = 7;
const PHYS_RAGDOLL_SELF_COLLISION: u8 = 8;
const PHYS_PROPS: u8 = 9;
const PHYS_PROP_MAX: u8 = 10;
const PHYS_DEBRIS: u8 = 11;
const PHYS_DEBRIS_MAX: u8 = 12;
const PHYS_DEBRIS_LIFETIME: u8 = 13;
const PHYS_PLAYER_PUSH: u8 = 14;
const PHYS_WEAPON_IMPULSES: u8 = 15;
const PHYS_EXPLOSION_IMPULSES: u8 = 16;
const PHYS_FORCE_IMPULSES: u8 = 17;
const PHYS_DEBUG_DRAW: u8 = 18;
const PHYS_STATS: u8 = 19;
const PHYS_CLOTH: u8 = 20;
const PHYS_CLOTH_BODY_COLLISION: u8 = 21;
const PHYS_CLOTH_WIND: u8 = 22;
const PHYS_CLOTH_AIR: u8 = 23;
const PHYS_CLOTH_TURN: u8 = 24;
const PHYS_CLOTH_ANIMATION: u8 = 25;
const PHYS_CLOTH_CLEARANCE: u8 = 26;
const PHYS_DISMEMBERMENT: u8 = 27;
const PHYS_DISMEMBER_MAX: u8 = 28;
const PHYS_DISMEMBER_LIFETIME: u8 = 29;
const PHYS_JIGGLE: u8 = 30;
const PHYS_JIGGLE_STRENGTH: u8 = 31;
const PHYS_JIGGLE_BREAST: u8 = 32;
const PHYS_JIGGLE_GLUTE: u8 = 33;
const PHYS_JIGGLE_STIFFNESS: u8 = 34;
const PHYS_JIGGLE_DAMPING: u8 = 35;
const PHYS_JIGGLE_GLUTE_LIFT: u8 = 36;
const PHYS_JIGGLE_SOLVER: u8 = 37;
const PHYS_JIGGLE_JP_STIFFNESS: u8 = 38;
const PHYS_JIGGLE_JP_DRAG: u8 = 39;
const PHYS_JIGGLE_JP_AIR_DRAG: u8 = 40;
const PHYS_JIGGLE_JP_STRETCH: u8 = 41;
const PHYS_JIGGLE_JP_SOFTEN: u8 = 42;
const PHYS_JIGGLE_JP_GRAVITY: u8 = 43;
const PHYS_JIGGLE_BREAST_REMESH_BOTTOM_Z: u8 = 44;
const PHYS_JIGGLE_BREAST_REMESH_TOP_Z: u8 = 45;
const PHYS_JIGGLE_GLUTE_REMESH_BOTTOM_Z: u8 = 46;
const PHYS_JIGGLE_GLUTE_REMESH_TOP_Z: u8 = 47;
const PHYS_JIGGLE_GLUTE_JIGGLE_BOTTOM_Z: u8 = 48;
const PHYS_JIGGLE_GLUTE_JIGGLE_TOP_Z: u8 = 49;
const PHYS_JIGGLE_BREAST_SIZE: u8 = 50;
const PHYS_JIGGLE_GLUTE_SIZE: u8 = 51;
const PHYS_JIGGLE_LOCOMOTION: u8 = 52;
const PHYS_JIGGLE_SWIRL: u8 = 53;
const PHYS_JIGGLE_BREAST_STIFFNESS: u8 = 54;
const PHYS_JIGGLE_BREAST_DAMPING: u8 = 55;
const PHYS_JIGGLE_GLUTE_STIFFNESS: u8 = 56;
const PHYS_JIGGLE_GLUTE_DAMPING: u8 = 57;
const PHYS_JIGGLE_STOMACH: u8 = 58;
const PHYS_JIGGLE_STOMACH_STIFFNESS: u8 = 59;
const PHYS_JIGGLE_STOMACH_DAMPING: u8 = 60;

const OCEAN_MAP_SIZE: u8 = 0;
const OCEAN_MESH_QUALITY: u8 = 1;
const OCEAN_UPDATES: u8 = 2;
const OCEAN_ROUGHNESS: u8 = 3;
const OCEAN_NORMAL_STRENGTH: u8 = 4;
const OCEAN_WATER_COLOR: u8 = 5;
const OCEAN_FOAM_COLOR: u8 = 6;
const OCEAN_SEA_SPRAY: u8 = 7;
const OCEAN_WIND_FOAM: u8 = 8;

fn pending_video_reset(key: &str) -> Option<theme::Reset> {
    Some(match key {
        "hdr_output" | "cvar:r_hdr" => theme::Reset::Video(ui::VIDEO_ROW_HDR),
        "render_backend" => theme::Reset::Video(ui::VIDEO_ROW_RENDER_BACKEND),
        "gpu_adapter" => theme::Reset::Video(ui::VIDEO_ROW_GPU_ADAPTER),
        "display_mode" => theme::Reset::Video(ui::VIDEO_ROW_FULLSCREEN),
        "resolution" => theme::Reset::Video(ui::VIDEO_ROW_RESOLUTION),
        "voxel_probe_gi" => theme::Reset::Video(ui::VIDEO_ROW_VOXEL_PROBE_GI),
        "gen_normal_maps" => theme::Reset::Video(ui::VIDEO_ROW_GEN_NORMAL_MAPS),
        "float_lightmap" => theme::Reset::Video(ui::VIDEO_ROW_FLOAT_LIGHTMAP),
        "picmip" => theme::Reset::Video(ui::VIDEO_ROW_PICMIP),
        "patch_subdivisions" | "cvar:r_subdivisions" => theme::Reset::Video(ui::VIDEO_ROW_PATCH_SUBDIVISIONS),
        "patch_stitching" => theme::Reset::Video(ui::VIDEO_ROW_PATCH_STITCHING),
        "reflections" => theme::Reset::Video(ui::VIDEO_ROW_SSR),
        "pbr" => theme::Reset::Video(ui::VIDEO_ROW_PBR),
        "asset_overrides" => theme::Reset::Video(ui::VIDEO_ROW_ASSET_OVERRIDES),
        "grass" => theme::Reset::Environment(ui::ENV_ROW_GRASS),
        "ocean" => theme::Reset::Environment(ui::ENV_ROW_OCEAN),
        "glass_promotion" | "cvar:r_glasspromotion" | "cvar:r_glassPromotion" => theme::Reset::Glass("r_glassPromotion"),
        _ => return None,
    })
}

fn percent(value: f32) -> String {
    format!("{:.0}%", value * 100.0)
}

fn index_of<T: Copy + PartialEq>(options: &[(T, &str)], value: T, fallback: usize) -> usize {
    options
        .iter()
        .position(|(candidate, _)| *candidate == value)
        .unwrap_or(fallback)
}

fn mode_index<T: Copy + PartialEq>(options: &[(T, &str, &str)], value: T) -> usize {
    options
        .iter()
        .position(|(candidate, _, _)| *candidate == value)
        .unwrap_or(0)
}

/// Mutually exclusive options with no cost ordering: flat chips.
pub(super) fn segmented_row<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    label: &str,
    tip: &str,
    reset: theme::Reset,
    current: T,
    options: &[(T, &str)],
) -> Option<T> {
    theme::row(ui, label, tip, reset, |ui| {
        let preview_label = theme::preset_preview_target_label(ui.ctx(), reset);
        let mut selected = None;
        for (value, text) in options.iter().copied() {
            let show_selected = preview_label.as_deref().map_or(value == current, |label| label == text);
            if theme::chip(ui, text, show_selected).clicked() && value != current {
                selected = Some(value);
            }
            ui.add_space(3.0);
        }
        selected
    })
}

/// Ordered options that may have no active value. This is used for quality
/// presets because changing any preset-owned cvar puts the row into a real
/// Custom state instead of pretending one of the four presets is still active.
fn optional_quality_table_row<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    label: &str,
    tip: &str,
    reset: theme::Reset,
    current: Option<T>,
    options: &[(T, &str)],
) -> Option<usize> {
    let index = current.and_then(|value| {
        options
            .iter()
            .position(|(candidate, _)| *candidate == value)
    });
    let labels: Vec<&str> = options.iter().map(|(_, text)| *text).collect();
    let display_index = theme::preview_index_for_labels(ui.ctx(), reset, &labels, index);
    theme::row(ui, label, tip, reset, |ui| {
        let width = theme::track_width(ui);
        theme::quality_meter_optional(ui, display_index, &labels, width, "Custom")
    })
}

/// Returns the clicked and hovered quality preset separately. Hover alone must
/// never call any cvar setter or update the active preset.
fn preview_quality_preset_row(
    ui: &mut egui::Ui,
    current: Option<QualityPreset>,
    options: &[(QualityPreset, &str)],
) -> (Option<usize>, Option<usize>) {
    let selected = current.and_then(|value| options.iter().position(|(p, _)| *p == value));
    let labels: Vec<&str> = options.iter().map(|(_, name)| *name).collect();
    theme::row(ui, "Quality preset",
        "Hover over a preset level to preview changes in the rows below. Click to apply. \
         Hover never changes live settings or compiles shaders. The preset leaves \
         monitor, backend, vsync, FPS cap and debug preferences alone.",
        theme::Reset::Video(ui::VIDEO_ROW_QUALITY_PRESET), |ui| {
            let mut hovered = None;
            let width = theme::track_width(ui);
            let clicked = theme::quality_meter_optional_preview(ui, selected, &labels, width, &mut hovered);
            (clicked, hovered)
        })
}

/// Ordered options: quality meter driven by the table's own order.
fn quality_table_row<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    label: &str,
    tip: &str,
    reset: theme::Reset,
    current: T,
    options: &[(T, &str)],
    fallback: usize,
) -> Option<usize> {
    let index = index_of(options, current, fallback);
    let labels: Vec<&str> = options.iter().map(|(_, text)| *text).collect();
    let display_index = theme::preview_index_for_labels(ui.ctx(), reset, &labels, Some(index)).unwrap_or(index);
    quality_row(ui, label, tip, reset, display_index, &labels)
}

fn quality_row(
    ui: &mut egui::Ui,
    label: &str,
    tip: &str,
    reset: theme::Reset,
    current: usize,
    labels: &[&str],
) -> Option<usize> {
    let display_current = theme::preview_index_for_labels(ui.ctx(), reset, labels, Some(current)).unwrap_or(current);
    theme::row(ui, label, tip, reset, |ui| {
        let width = theme::track_width(ui);
        theme::quality_meter(ui, display_current, labels, width)
    })
}

/// Ordered options that each carry an explanation of what the renderer does.
fn mode_row<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    label: &str,
    tip: &str,
    reset: theme::Reset,
    current: T,
    options: &[(T, &str, &str)],
) -> Option<usize> {
    let index = mode_index(options, current);
    let labels: Vec<&str> = options.iter().map(|(_, text, _)| *text).collect();
    let display_index = theme::preview_index_for_labels(ui.ctx(), reset, &labels, Some(index)).unwrap_or(index);
    let note = options.get(display_index).map(|(_, _, note)| *note).unwrap_or("");
    theme::row_with_note(ui, label, tip, reset, note, |ui| {
        let width = theme::track_width(ui);
        theme::quality_meter(ui, display_index, &labels, width)
    })
}

fn color_row(
    ui: &mut egui::Ui,
    label: &str,
    tip: &str,
    reset: theme::Reset,
    values: &mut [f32; 3],
    changed: &mut bool,
) {
    theme::row(ui, label, tip, reset, |ui| {
        let mut rgb = *values;
        if ui.color_edit_button_rgb(&mut rgb).changed() {
            *values = rgb;
            *changed = true;
        }
        ui.add_space(10.0);
        theme::glow_label(
            ui,
            &format!("{:.2}  {:.2}  {:.2}", values[0], values[1], values[2]),
            12.5,
            theme::TEXT_FAINT,
        );
    });
}

/// A slider that follows the swell amplitude while its padlock is closed.
/// Unlocking leaves the current value in place, so it never changes the picture.
#[allow(clippy::too_many_arguments)]
fn linked_ocean_slider(
    ui: &mut egui::Ui,
    label: &str,
    tip: &str,
    value: &mut f32,
    default: f32,
    range: std::ops::RangeInclusive<f32>,
    bit: u8,
    links: &mut u8,
    changed: &mut bool,
) {
    theme::row(ui, label, tip, theme::Reset::None, |ui| {
        let locked = *links & bit != 0;
        let mut local = *value;
        let readout = format!("{local:.3}");
        let moved = ui
            .add_enabled_ui(!locked, |ui| theme::slider_default(ui, &mut local, range, &readout, default))
            .inner;
        ui.add_space(8.0);
        if let Some(now_locked) = theme::lock_toggle(ui, locked) {
            if now_locked { *links |= bit } else { *links &= !bit }
            *changed = true;
        } else if moved {
            *value = local;
            *changed = true;
        }
    });
}

/// [`ocean_slider`] whose slider (and label) also restore `default` on double-click.
#[allow(clippy::too_many_arguments)]
fn ocean_slider_dd(
    ui: &mut egui::Ui,
    label: &str,
    tip: &str,
    reset: theme::Reset,
    value: &mut f32,
    default: f32,
    range: std::ops::RangeInclusive<f32>,
    changed: &mut bool,
) {
    theme::row(ui, label, tip, reset, |ui| {
        let mut local = *value;
        let readout = format!("{local:.3}");
        if theme::slider_default(ui, &mut local, range, &readout, default) {
            *value = local;
            *changed = true;
        }
    });
}

fn ocean_slider(
    ui: &mut egui::Ui,
    label: &str,
    tip: &str,
    reset: theme::Reset,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    changed: &mut bool,
) {
    theme::row(ui, label, tip, reset, |ui| {
        let mut local = *value;
        let readout = format!("{local:.3}");
        if theme::slider(ui, &mut local, range, &readout) {
            *value = local;
            *changed = true;
        }
    });
}

/// [`ocean_slider`] for rows with no archived reset tag: double-clicking the
/// slider restores `default`.
fn ocean_slider_d(
    ui: &mut egui::Ui,
    label: &str,
    tip: &str,
    value: &mut f32,
    default: f32,
    range: std::ops::RangeInclusive<f32>,
    changed: &mut bool,
) {
    theme::row(ui, label, tip, theme::Reset::None, |ui| {
        let mut local = *value;
        let readout = format!("{local:.3}");
        if theme::slider_default(ui, &mut local, range, &readout, default) {
            *value = local;
            *changed = true;
        }
    });
}

/// Like [`ocean_slider_d`], but `value` is stored in JKA units while the slider
/// shows and edits metres (`units_per_metre` units each).
fn ocean_slider_metres_d(
    ui: &mut egui::Ui,
    label: &str,
    tip: &str,
    value: &mut f32,
    default: f32,
    max_metres: f32,
    min_metres: f32,
    changed: &mut bool,
) {
    let scale = crate::ocean::OCEAN_UNITS_PER_METER;
    theme::row(ui, label, tip, theme::Reset::None, |ui| {
        let mut metres = (*value / scale).clamp(min_metres, max_metres);
        let readout = format!("{metres:.1} m");
        if theme::slider_default(ui, &mut metres, min_metres..=max_metres, &readout, default / scale) {
            *value = metres * scale;
            *changed = true;
        }
    });
}

/// Draws one crosshair (shape or stock image) centered in a square of at most
/// `extent` px. `crosshair.style` must be non-zero.
fn draw_crosshair_glyph(
    painter: &egui::Painter,
    center: egui::Pos2,
    extent: f32,
    crosshair: crate::ui::CrosshairSettings,
    image: Option<&egui::TextureHandle>,
    line_width: f32,
) {
    // Mirrors build_crosshair: strength fades below 100%, and above it layers the
    // faint stock images.
    let strength = crosshair
        .strength
        .clamp(0.0, crate::ui::CROSSHAIR_STRENGTH_MAX);
    let alpha = (f32::from(crosshair.color[3]) * strength.min(1.0)).round() as u8;
    let color = egui::Color32::from_rgba_unmultiplied(
        crosshair.color[0],
        crosshair.color[1],
        crosshair.color[2],
        alpha,
    );
    let max_preview = extent;
    if crosshair.image != 0 {
        // Same pixel size the game uses; the stock artwork is padded, so it reads
        // like the shapes at the same size.
        let side = crosshair.size.clamp(4.0, 96.0).min(max_preview.max(8.0));
        if let Some(texture) = image {
            let extra = (strength - 1.0).max(0.0) * 3.0;
            for pass in 0..1 + extra.ceil() as usize {
                let weight = if pass == 0 {
                    1.0
                } else {
                    (extra - (pass - 1) as f32).min(1.0)
                };
                painter.image(
                    texture.id(),
                    egui::Rect::from_center_size(center, egui::vec2(side, side)),
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::from_rgba_unmultiplied(
                        crosshair.color[0],
                        crosshair.color[1],
                        crosshair.color[2],
                        (f32::from(alpha) * weight).round() as u8,
                    ),
                );
            }
        }
        return;
    }
    let size = (crosshair.size.clamp(4.0, 96.0) * (2.0 / 3.0)).min(max_preview.max(8.0));
    if crosshair.style == crate::ui::CROSSHAIR_STYLE_LINE {
        // Roughly a 2 px per 640x480-unit view, like the game at 960x720.
        let line_w = line_width.clamp(0.25, 5.0) * 2.0;
        let line_h = (size * 1.25).min(max_preview.max(8.0));
        painter.rect_filled(
            egui::Rect::from_center_size(center, egui::vec2(line_w, line_h)),
            egui::CornerRadius::ZERO,
            color,
        );
        return;
    }
    let half = size * 0.5;
    let thickness = (size / 8.0).clamp(1.0, 4.0);
    let gap_max = (half - thickness * 0.5).max(0.5);
    let gap = (size / 8.0).clamp(0.5, gap_max);
    let arm = (half - gap).max(1.0);

    let rect = |x: f32, y: f32, w: f32, h: f32| {
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h)),
            egui::CornerRadius::ZERO,
            color,
        );
    };
    let hbar = |x: f32, y: f32, width: f32| rect(x, y - thickness * 0.5, width, thickness);
    let vbar = |x: f32, y: f32, height: f32| rect(x - thickness * 0.5, y, thickness, height);

    match crosshair.style {
        1 => {
            hbar(center.x - half, center.y, arm);
            hbar(center.x + gap, center.y, arm);
            vbar(center.x, center.y - half, arm);
            vbar(center.x, center.y + gap, arm);
        }
        2 => {
            let dot = (thickness * 1.6).clamp(2.0, 6.0);
            painter.circle_filled(center, dot * 0.5, color);
        }
        3 => {
            hbar(center.x - half, center.y, size);
            vbar(center.x, center.y - half, size);
        }
        4 => {
            hbar(center.x - half, center.y, arm);
            hbar(center.x + gap, center.y, arm);
            vbar(center.x, center.y - half, arm);
            vbar(center.x, center.y + gap, arm);
            let dot = thickness.max(2.0);
            painter.circle_filled(center, dot * 0.5, color);
        }
        5 => {
            let bracket_h = size * 0.7;
            let cap = (size * 0.2).max(thickness);
            vbar(center.x - half, center.y - bracket_h * 0.5, bracket_h);
            vbar(center.x + half, center.y - bracket_h * 0.5, bracket_h);
            hbar(center.x - half, center.y - bracket_h * 0.5, cap);
            hbar(center.x - half, center.y + bracket_h * 0.5, cap);
            hbar(center.x + half - cap, center.y - bracket_h * 0.5, cap);
            hbar(center.x + half - cap, center.y + bracket_h * 0.5, cap);
        }
        _ => {
            painter.rect_stroke(
                egui::Rect::from_center_size(center, egui::vec2(size, size)),
                egui::CornerRadius::ZERO,
                egui::Stroke::new(thickness, color),
                egui::StrokeKind::Middle,
            );
        }
    }
}
