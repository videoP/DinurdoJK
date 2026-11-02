//! Shared saber blade picker for Profile and Render Studio.
//! Presets select authored JKA glow/core shaders, never a canned RGB value.

/// The caller owns persistence: Profile writes `color1/2` (+ `cp_sbRGB` on
/// custom edits); Render Studio writes its encoded timeline saber mode.
pub(super) enum SaberColorEdit {
    Stock(i32),
    CustomRgb([u8; 3]),
}

pub(super) struct SaberPickerResult {
    /// True when the control or its popup was interacted with (camera focus).
    pub(super) opened: bool,
    pub(super) edit: Option<SaberColorEdit>,
    pub(super) middle_reset: bool,
}

const STOCK: [(i32, &str, [u8; 3]); 7] = [
    (0, "Red", [255, 51, 51]),
    (1, "Orange", [255, 128, 26]),
    (2, "Yellow", [255, 255, 51]),
    (3, "Green", [51, 255, 51]),
    (4, "Blue", [51, 102, 255]),
    (5, "Purple", [230, 51, 255]),
    (crate::cgame::SABER_BLACK, "Black", [0, 0, 0]),
];

fn ink_for(background: egui::Color32) -> egui::Color32 {
    // Perceptual luminance; yellow and green need a dark foreground.
    let luma = 0.2126 * f32::from(background.r())
        + 0.7152 * f32::from(background.g())
        + 0.0722 * f32::from(background.b());
    if luma >= 145.0 { egui::Color32::BLACK } else { egui::Color32::WHITE }
}

/// Render both pages with one trigger, one RGB editor and the same stock row.
/// `mode` is the raw shader selection, not the preview swatch's RGB value.
pub(super) fn saber_color_picker(
    ui: &mut egui::Ui, mode: i32, rgb: [u8; 3],
) -> SaberPickerResult {
    let swatch = egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
    let title = STOCK.iter().find(|(value, _, _)| *value == mode)
        .map_or_else(|| format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2]),
            |(_, name, _)| (*name).to_owned());
    let button = ui.add(
        egui::Button::new(egui::RichText::new(title).color(ink_for(swatch)))
            .fill(swatch)
            .min_size(egui::vec2(92.0, 23.0)),
    ).on_hover_text("Blade color · Choose custom RGB or a stock shader · Middle-click resets blue");

    let middle_reset = button.clicked_by(egui::PointerButton::Middle);
    let mut edit = if middle_reset { Some(SaberColorEdit::Stock(4)) } else { None };
    egui::Popup::menu(&button)
        .style(super::render_studio_ui::menu_style())
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(208.0);
            let mut chosen = swatch;
            if egui::color_picker::color_picker_color32(
                ui, &mut chosen, egui::color_picker::Alpha::Opaque,
            ) {
                edit = Some(SaberColorEdit::CustomRgb([chosen.r(), chosen.g(), chosen.b()]));
            }
            ui.separator();
            ui.label(egui::RichText::new("STOCK SABER SHADERS").small());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                for (value, name, tint) in STOCK {
                    let fill = egui::Color32::from_rgb(tint[0], tint[1], tint[2]);
                    let selected = mode == value;
                    let text = if value == crate::cgame::SABER_BLACK { "K".to_owned() }
                        else { (value + 1).to_string() };
                    let button = egui::Button::new(
                        egui::RichText::new(text).color(ink_for(fill)),
                    ).fill(fill)
                        .stroke(egui::Stroke::new(
                            if selected { 2.0 } else { 0.5 },
                            if selected { super::render_studio_ui::palette::ACCENT() }
                            else { super::render_studio_ui::palette::LINE() },
                        ))
                        .min_size(egui::vec2(24.0, 22.0));
                    if ui.add(button).on_hover_text(format!("{name} · original saber shader")).clicked() {
                        edit = Some(SaberColorEdit::Stock(value));
                        ui.close();
                    }
                }
            });
        });
    let opened = button.clicked() || middle_reset || edit.is_some();
    SaberPickerResult { opened, edit, middle_reset }
}
