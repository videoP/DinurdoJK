//! The single Profile page. Uses the real player, saber, Force and PK3 systems.
//! No parallel model loader, invented skeleton or duplicate userinfo cvars.
use crate::app::egui_menu::{
    jka_colored_text, theme, App, BTreeMap, ProfileModelEntry,
    PROFILE_EMOTES, PROFILE_FORCE, PROFILE_MODEL,
};

const INK: egui::Color32 = egui::Color32::from_rgb(10, 15, 23);
const PLATE: egui::Color32 = egui::Color32::from_rgb(17, 24, 35);
const PLATE_LIFT: egui::Color32 = egui::Color32::from_rgb(25, 34, 48);
const EDGE: egui::Color32 = egui::Color32::from_rgb(48, 66, 86);
const SKY: egui::Color32 = theme::ACCENT;
const SKY_SOFT: egui::Color32 = egui::Color32::from_rgb(125, 199, 255);
const MUTED: egui::Color32 = egui::Color32::from_rgb(142, 162, 183);
const STUDIO_NAV: [(usize, &str); 3] = [
    (PROFILE_MODEL, "LOOK"),
    (PROFILE_FORCE, "FORCE"),
    (PROFILE_EMOTES, "EMOTES"),
];

#[derive(Clone, Default)]
struct BodyEditor { group: String, search: String }
fn body_editor_id() -> egui::Id { egui::Id::new("studio_anny_editor") }


/// Inspect the actual virtual filesystem providers, not a guessed PK3 name.
/// The portrait is only one resource: a model may load its skeleton, meshes,
/// skins, and animation configuration from different archives.
fn studio_model_sources(base: &std::path::Path, game: Option<&std::path::Path>,
    allow_overrides: bool, entry: &ProfileModelEntry) -> String {
    let Ok(mut assets) = jka_assets::pk3::AssetSearchPath::open_game(base, game) else {
        return "Unable to inspect the current asset search path".into();
    };
    assets.set_allow_asset_overrides(allow_overrides);
    let folder = format!("models/players/{}/", entry.model_name);
    let mut lines = vec![format!("Model: {}    Skin: {}", entry.model_name, entry.skin_name),
        format!("Userinfo model: {}", entry.value), String::new()];
    let mut chosen_icon = None;
    let fallback = if crate::app::frontend::profile_is_rgb_skin(&entry.model_name, &entry.skin_name)
        || entry.skin_name.eq_ignore_ascii_case("sp") { "icons/icon_rgb_unknown" }
        else if entry.skin_name.eq_ignore_ascii_case("red") { "icons/icon_red_unknown" }
        else if entry.skin_name.eq_ignore_ascii_case("blue") { "icons/icon_blue_unknown" }
        else { "icons/icon_default_unknown" };
    // Same jpg/jpeg/png/tga priority as frontend::read_profile_icon.
    for stem in [format!("{folder}icon_{}", entry.skin_name), fallback.to_owned()] {
        for ext in ["jpg", "jpeg", "png", "tga"] {
            let path = format!("{stem}.{ext}");
            if assets.contains_qpath(&path) {
                chosen_icon = Some(path); break;
            }
        }
        if chosen_icon.is_some() { break; }
    }
    if let Some(icon) = &chosen_icon {
        lines.push(format!("PORTRAIT: {icon}"));
        for (i, info) in assets.indexed_providers(icon).into_iter().enumerate() {
            lines.push(format!("  {} {}{}", if info.selected { "ACTIVE" } else { "SHADOWED" },
                info.source.display(), if i == 0 { "" } else { " (override candidate)" }));
        }
    } else {
        lines.push("PORTRAIT: no image; UI draws a placeholder".into());
    }
    lines.push(String::new());
    lines.push("MODEL FILES / PROVIDERS (VFS priority):".into());
    let mut count = 0_usize;
    let mut total = 0_usize;
    for qpath in assets.names().filter(|name| name.starts_with(&folder)) {
        let leaf = qpath.rsplit('/').next().unwrap_or("");
        let skin = entry.skin_name.to_ascii_lowercase();
        let relevant_skin = leaf == format!("model_{skin}.skin")
            || leaf == format!("{skin}.skin")
            || leaf == "model_default.skin" && entry.source_model
            || leaf.starts_with("head_") && skin.starts_with("head_") && leaf == format!("{skin}.skin")
            || leaf.starts_with("torso_") && skin.starts_with("torso_") && leaf == format!("{skin}.skin")
            || leaf.starts_with("lower_") && skin.starts_with("lower_") && leaf == format!("{skin}.skin");
        let relevant = relevant_skin || leaf == "source_model.json" || leaf == "animation.cfg"
            || leaf.ends_with(".glm") || leaf.ends_with(".gla") || leaf.ends_with(".md3")
            || leaf.ends_with(".mdr") || leaf.ends_with(".iqm");
        if !relevant { continue; }
        total += 1;
        if count >= 70 { continue; }
        count += 1;
        let providers = assets.indexed_providers(qpath);
        lines.push(format!("{qpath}:"));
        for info in providers {
            lines.push(format!("  {} {}{}", if info.selected { "ACTIVE" } else { "SHADOWED" },
                info.source.display(), if info.archive { " [PK3/ZIP]" } else { " [loose]" }));
        }
    }
    if total > count { lines.push(format!("... {} more model files", total - count)); }
    if total == 0 { lines.push("No matching mesh/skin/animation files indexed".into()); }
    lines.push(String::new());
    lines.push("Textures and shader scripts referenced by .skin files may be in other PK3s.".into());
    lines.push("Use /which <qpath> all to check any dependency and its overrides.".into());
    lines.join("\n")
}

fn studio_model_source_popup(ui: &mut egui::Ui, base: &std::path::Path,
    game: Option<&std::path::Path>, allow_overrides: bool, entry: &ProfileModelEntry) {
    ui.set_min_width(360.0);
    ui.heading("MODEL ASSET SOURCES");
    ui.small("Selected model, skin, portrait and their physical PK3 providers");
    let text = studio_model_sources(base, game, allow_overrides, entry);
    if ui.button("Copy full report").clicked() { ui.ctx().copy_text(text.clone()); }
    egui::ScrollArea::vertical().max_height(340.0).show(ui, |ui| {
        ui.add(egui::Label::new(egui::RichText::new(text).monospace().size(10.0)).selectable(true));
    });
}

/// Navigate a focused Profile selector without changing its stored cvar format.
/// The selected value is always taken from the same filtered choices shown in
/// its popup (e.g. staff vs single hilts), not from a parallel catalog.
fn studio_selector_arrow(
    ui: &egui::Ui,
    response: &egui::Response,
    selected: &str,
    choices: &[&str],
) -> Option<String> {
    if response.clicked() { response.request_focus(); }
    if !(response.has_focus() || egui::ComboBox::is_open(ui.ctx(), response.id))
        || choices.is_empty() { return None; }
    let movement = ui.input(|input| {
        if input.key_pressed(egui::Key::ArrowDown) { 1isize }
        else if input.key_pressed(egui::Key::ArrowUp) { -1isize }
        else { 0 }
    });
    if movement == 0 { return None; }
    let old = choices.iter().position(|name| name.eq_ignore_ascii_case(selected));
    let next = match (old, movement) {
        (Some(index), step) => (index as isize + step).rem_euclid(choices.len() as isize) as usize,
        (None, 1) => 0,
        (None, _) => choices.len() - 1,
    };
    let next = choices[next];
    (!next.eq_ignore_ascii_case(selected)).then(|| next.to_owned())
}

/// Cosmetic buttons use a popup rather than an egui ComboBox. Keep its keyboard
/// sequence identical to the popup order, with 'None' as the first choice.
fn studio_cosmetic_arrow(group: u8, mask: u32, movement: isize) -> Option<Option<u32>> {
    if movement == 0 { return None; }
    let choices: Vec<Option<u32>> = std::iter::once(None)
        .chain(crate::japro_cg::cosmetics_in_group(group).map(|item| Some(item.bit)))
        .collect();
    let current = choices.iter().position(|bit| bit.is_some_and(|bit| mask & bit != 0)).unwrap_or(0);
    let next = (current as isize + movement).rem_euclid(choices.len() as isize) as usize;
    (next != current).then_some(choices[next])
}

fn studio_deformation_slider_reset(response: &egui::Response) -> bool {
    // Match egui_theme::slider_with: Slider rails sense drag only, so read
    // pointer double-clicks over the actual widget instead of clicked flags.
    (response.contains_pointer() && response.ctx.input(|input|
        input.pointer.button_double_clicked(egui::PointerButton::Primary)))
        || response.clicked_by(egui::PointerButton::Middle)
}

fn studio_deformation_reset_label(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>) -> egui::Response {
    ui.add(egui::Label::new(text).sense(egui::Sense::click()))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Click to restore neutral / default")
}

fn studio_anny_detail_title(name: &str) -> String {
    match name {
        "gender" => "Gender (Anny)".to_owned(),
        "cupsize" => "Breast size (Anny cup size)".to_owned(),
        "firmness" => "Breast firmness (Anny)".to_owned(),
        _ => name.trim_end_matches("-incr").replace('-', " "),
    }
}

fn label(ui: &mut egui::Ui, content: impl Into<String>, size: f32, color: egui::Color32) {
    ui.label(egui::RichText::new(content.into()).size(size).color(color));
}

fn cap(ui: &mut egui::Ui, title: &str, note: &str) {
    ui.horizontal(|ui| {
        label(ui, title, 11.0, SKY_SOFT);
        if !note.is_empty() {
            ui.add_space(5.0);
            label(ui, note, 10.0, MUTED);
        }
    });
    ui.add_space(6.0);
}

/// Custom painted, animated button. It still uses an egui response for focus,
/// hit-testing, accessibility and pointer capture. No invisible image hotspots.
fn studio_button(ui: &mut egui::Ui, text: &str, selected: bool, primary: bool, width: f32) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 35.0), egui::Sense::click());
    let hover = ui.ctx().animate_bool_with_time(response.id, response.hovered(), 0.16);
    let fill = if primary {
        SKY
    } else if selected {
        PLATE_LIFT
    } else {
        INK
    };
    let fill = fill.lerp_to_gamma(PLATE_LIFT, if primary { hover * 0.17 } else { hover * 0.55 });
    let outline = if selected || primary { SKY } else { EDGE };
    let painter = ui.painter();
    painter.rect_filled(rect, egui::CornerRadius::same(5), fill);
    painter.rect_stroke(rect, egui::CornerRadius::same(5), egui::Stroke::new(1.0, outline), egui::StrokeKind::Inside);
    if selected && !primary {
        painter.rect_filled(egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())), 1.0, SKY);
    }
    painter.text(rect.center(), egui::Align2::CENTER_CENTER, text,
        egui::FontId::proportional(11.0), if primary { INK } else { theme::TEXT });
    response.on_hover_text(text).clicked()
}

impl App {
    pub(in crate::app::egui_menu) fn egui_profile_studio(&mut self, ui: &mut egui::Ui) {
        self.ensure_profile_catalog();
        if !matches!(self.profile_selected_section, PROFILE_MODEL | PROFILE_FORCE | PROFILE_EMOTES) {
            self.profile_selected_section = PROFILE_MODEL;
        }
        let available_w = ui.available_width();
        // The frontend already owns a top bar. An additional large title was
        // clipping the stage and doubling the amount of chrome at 1080p.
        ui.add_space(3.0);
        let opened = self.profile_studio_started.elapsed().as_secs_f32();
        let body_h = ui.available_height().max(320.0);
        let controls_w = if self.profile_selected_section==PROFILE_EMOTES {((available_w-14.0)*0.55).clamp(300.0,880.0)}else{((available_w - 14.0) * 0.335).clamp(300.0, 450.0)}
            .min((available_w - 190.0).max(220.0));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            ui.allocate_ui_with_layout(
                egui::vec2(controls_w, body_h), egui::Layout::top_down(egui::Align::Min),
                |ui| self.studio_control_panel(ui, controls_w, body_h),
            );
            let stage_w = ui.available_width();
            ui.allocate_ui_with_layout(
                egui::vec2(stage_w, body_h), egui::Layout::top_down(egui::Align::Min),
                |ui| self.studio_character_stage(ui, stage_w, body_h, opened),
            );
        });
        self.update_profile_preview();
        // No Profile-specific animation/repaint timer: tick_egui_menu already
        // rebuilds the frontend at its active render cap (display refresh or
        // lower com_maxfps). The Ghoul2 preview clock samples continuous time.
    }

    fn studio_control_panel(&mut self, ui: &mut egui::Ui, width: f32, height: f32) {
        egui::Frame::new().fill(PLATE).corner_radius(egui::CornerRadius::same(6))
            .stroke(egui::Stroke::new(1.0, EDGE)).inner_margin(egui::Margin::same(14)).show(ui, |ui| {
                ui.set_width((width - 28.0).max(1.0));
                ui.set_min_height((height - 28.0).max(1.0));
                // The tabs and search remain fixed; catalog and the optional
                // other-player appearance group own bounded scroll regions.
                let nav_w = (ui.available_width() - 12.0) / STUDIO_NAV.len() as f32;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    for (section, title) in STUDIO_NAV {
                        if studio_button(ui, title, self.profile_selected_section == section, false, nav_w) {
                            self.profile_selected_section = section;
                            if section != PROFILE_MODEL { self.profile_studio_cosmetic_group = 0; }
                            let group = if section == PROFILE_MODEL {
                                ui.ctx().data_mut(|data| data.get_temp::<BodyEditor>(body_editor_id()))
                                    .map_or(String::new(), |editor| editor.group)
                            } else { String::new() };
                            self.focus_profile_studio_camera(&group);
                            self.profile_preview_key = None;
                            if section != PROFILE_EMOTES {
                                crate::emotes::stop();
                                self.studio_emote_audio(None);
                            }
                        }
                    }
                });
                ui.add_space(10.0);
                match self.profile_selected_section {
                    PROFILE_MODEL => self.studio_model_workshop(ui, (height - 151.0).max(140.0)),
                    PROFILE_FORCE => {
                        egui::ScrollArea::vertical().id_salt("studio_force_scroll")
                            .max_height((height - 125.0).max(150.0)).show(ui, |ui| self.egui_profile_force(ui));
                    }
                    PROFILE_EMOTES => self.studio_emotes(ui, (height - 151.0).max(140.0)),
                    _ => {}
                }
                if self.profile_selected_section!=PROFILE_EMOTES {
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);
                let pending = self.profile_has_pending_changes();
                if studio_button(ui, if pending { "APPLY CHANGES" } else { "APPLIED ✓" },
                    pending, pending, (ui.available_width() * 0.6).max(125.0)) && pending {
                    self.apply_profile_changes();
                }
                }
            });
    }

    /// Model variants are both visible for the active model and selectable via
    /// hover for other models. Multipart Jedi skins share the original cvar.
    fn studio_model_workshop(&mut self, ui: &mut egui::Ui, available_h: f32) {
        cap(ui, "CHARACTERS", "");
        if let Some(error) = self.profile_catalog_error.clone() {
            theme::banner(ui, &error, theme::WARNING);
        }
        let current = self.profile_model_staged.clone()
            .unwrap_or_else(|| self.solo_client_info.model_cvar());
        let current_model = current.split('/').next().unwrap_or_default().to_owned();


        // Skin variants remain in the model icon hover chooser. No duplicated
        // active-variant grid above the model catalog.
        self.studio_multipart_skin(ui, &current_model, &current);
        ui.add_space(8.0);
        cap(ui, "SHOW SKINS", "");
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            for (index, title) in ["Original", "Red", "Blue", "RGB"].into_iter().enumerate() {
                ui.checkbox(&mut self.profile_studio_skin_visible[index], title);
            }
        });
        ui.add_space(9.0);
        egui::CollapsingHeader::new("OTHER PLAYER APPEARANCE")
            .id_salt("studio_other_players")
            .default_open(false)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().id_salt("studio_other_player_options")
                    .max_height(220.0).show(ui, |ui| self.studio_visibility(ui));
            });
        ui.add_space(7.0);
        cap(ui, "MODEL LIBRARY", "");
        ui.add_sized([ui.available_width(), 30.0],
            egui::TextEdit::singleline(&mut self.profile_model_search).hint_text("Search models or skins…"));
        ui.add_space(8.0);
        let needle = self.profile_model_search.trim().to_ascii_lowercase();
        let mut models = BTreeMap::<String, Vec<usize>>::new();
        // Filter *skins* before grouping models. A character with Original,
        // Red and RGB skins remains in the library when one category is hidden,
        // while its hover chooser shows only visible variants. TaystJK does
        // RGB discovery by skin filename, so there is no runtime shader scan.
        // Only store catalog indices here. The library can contain hundreds
        // of skins; cloning every entry on every egui frame defeats the row
        // virtualization below. Clone only entries for rows actually visible.
        for (index, entry) in self.profile_models.iter().enumerate() {
            if !self.profile_studio_skin_visible[Self::studio_skin_class(entry)] {
                continue;
            }
            models.entry(entry.model_name.clone()).or_default().push(index);
        }
        // Searching for a model leaves its complete set of *visible* variants
        // available in the hover chooser instead of silently trimming them.
        models.retain(|name, variants| {
            needle.is_empty() || name.to_ascii_lowercase().contains(&needle)
                || variants.iter().any(|&index| {
                    let entry = &self.profile_models[index];
                    entry.skin_name.to_ascii_lowercase().contains(&needle)
                        || entry.value.to_ascii_lowercase().contains(&needle)
                })
        });
        if self.ui_catalog.pending.profile {
            label(ui, "Scanning game and mod asset archives…", 11.0, MUTED);
        } else if models.is_empty() {
            label(ui, "No characters match this search.", 11.0, MUTED);
        }
        // A single continuously scrollable library (no pagination). egui
        // virtualizes rows so thumbnails below the viewport are not requested.
        let list_height = (ui.available_height() - 64.0).min(available_h).max(100.0);
        let width = ui.available_width();
        let columns = ((width + 8.0) / 101.0).floor().max(1.0) as usize;
        let tile_w = ((width + 8.0) / columns as f32 - 8.0).clamp(80.0, 116.0);
        let mut selected = None;
        let model_rows: Vec<_> = models.into_values().collect();
        let row_height = tile_w * 1.22 + 10.0;
        egui::ScrollArea::vertical().id_salt("studio_model_library_scroll")
            .auto_shrink([false, false]).max_height(list_height).show_rows(ui, row_height,
                model_rows.len().div_ceil(columns), |ui, rows| {
                for row in rows {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        for variant_ids in model_rows.iter().skip(row * columns).take(columns) {
                            // The tile renderer may populate thumbnail caches,
                            // so own only this visible cell's catalog entries.
                            let variants: Vec<ProfileModelEntry> = variant_ids.iter()
                                .map(|&index| self.profile_models[index].clone()).collect();
                            let parts = ["head_", "torso_", "lower_"].map(|prefix|
                                variants.iter().find(|entry| entry.skin_name.starts_with(prefix)));
                            let multipart = parts.iter().all(Option::is_some);
                            let entry = if multipart { parts[0].unwrap() } else {
                                variants.iter().find(|entry| entry.value.eq_ignore_ascii_case(&current))
                                    .or_else(|| variants.iter().find(|entry| entry.skin_name.eq_ignore_ascii_case("default")))
                                    .unwrap_or(&variants[0])
                            };
                            if self.studio_model_tile(ui, entry, tile_w,
                                entry.model_name.eq_ignore_ascii_case(&current_model),
                                if multipart { None } else { Some(variants.as_slice()) }) {
                                selected = Some(if multipart {
                                    format!("{}/{}|{}|{}",entry.model_name,
                                        parts[0].unwrap().skin_name,
                                        parts[1].unwrap().skin_name,
                                        parts[2].unwrap().skin_name)
                                } else { entry.value.clone() });
                            }
                        }
                    });
                }
            });
        if let Some(value) = selected {
            self.studio_stage_model(&value, ui.ctx());
        }
    }

    /// TaystJK UI_SelectedTeamHead (codemp/ui/ui_main.c): the RGB browser
    /// uses /rgb* skin names and, exceptionally, the /sp skins of Trandoshan
    /// and Weequay. This is a *variant* classification, not an inspection of
    /// rgbGen stages; the custom char_color picker is separate.
    ///
    /// Return indices correspond to [Original, Red, Blue, RGB]. The current
    /// Profile catalog already caches all model/skin names from installed PK3s.
    fn studio_skin_class(entry: &ProfileModelEntry) -> usize {
        crate::app::frontend::profile_skin_category(&entry.model_name, &entry.skin_name)
    }

    // TaystJK supplies icon_default/red/blue/rgb_unknown when a model skin has
    // no portrait. When those assets are absent from the user's PK3 set, use
    // a color-coded in-painter stand-in rather than dropping the skin.
    fn studio_skin_icon_fallback(entry: &ProfileModelEntry) -> (&'static str, egui::Color32) {
        if entry.skin_name.eq_ignore_ascii_case("red") {
            ("RED", egui::Color32::from_rgb(224, 104, 109))
        } else if entry.skin_name.eq_ignore_ascii_case("blue") {
            ("BLUE", egui::Color32::from_rgb(103, 163, 239))
        } else if crate::app::frontend::profile_is_rgb_skin(&entry.model_name, &entry.skin_name)
            || entry.skin_name.eq_ignore_ascii_case("sp") {
            ("RGB", SKY_SOFT)
        } else {
            ("◈", MUTED)
        }
    }

    fn studio_model_tile(&mut self, ui: &mut egui::Ui, entry: &ProfileModelEntry, w: f32,
        selected: bool, variants: Option<&[ProfileModelEntry]>) -> bool {
        let variant_count = variants.map_or(0, |list| list.len());
        let h = w * 1.22;
        let (rect, response) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::click());
        let hovered = ui.ctx().animate_bool_with_time(response.id, response.hovered(), 0.12);
        let color = if selected { SKY } else { EDGE.lerp_to_gamma(SKY, hovered * 0.6) };
        let p = ui.painter();
        p.rect_filled(rect, 4.0, if selected { PLATE_LIFT } else { INK });
        p.rect_stroke(rect, 4.0, egui::Stroke::new(if selected { 2.0 } else { 1.0 }, color), egui::StrokeKind::Inside);
        let image = egui::Rect::from_min_max(rect.min + egui::vec2(4.0, 4.0),
            egui::pos2(rect.max.x - 4.0, rect.max.y - 24.0));
        if let Some(texture) = self.profile_model_icon_texture(entry) {
            ui.painter().image(texture.id(), image,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        } else {
            // TaystJK accepts RGB .skin files without matching icon_rgb*
            // images. Make these discoverable without an empty portrait.
            let (placeholder, color) = Self::studio_skin_icon_fallback(entry);
            ui.painter().text(image.center(), egui::Align2::CENTER_CENTER, placeholder,
                egui::FontId::proportional(17.0), color);
        }
        ui.painter().text(egui::pos2(rect.center().x, rect.bottom() - 12.0), egui::Align2::CENTER_CENTER,
            &entry.model_name, egui::FontId::proportional(10.5), if selected { theme::TEXT } else { MUTED });
        if variant_count > 1 {
            let badge = egui::Rect::from_min_size(rect.right_top() + egui::vec2(-24.0, 3.0), egui::vec2(20.0, 17.0));
            ui.painter().rect_filled(badge, 3.0, PLATE_LIFT);
            ui.painter().rect_stroke(badge, 3.0, egui::Stroke::new(1.0, SKY), egui::StrokeKind::Inside);
            ui.painter().text(badge.center(), egui::Align2::CENTER_CENTER, variant_count.to_string(),
                egui::FontId::proportional(10.0), theme::TEXT);
        }
        response.context_menu(|ui| {
            studio_model_source_popup(ui, &self.base, self.game.as_deref(), self.video.allow_asset_overrides, entry);
        });
        if let Some(variants) = variants.filter(|list| list.len() > 1) {
            let mut pick = None;
            let response = response.on_hover_ui(|ui| {
                // Show every variant at once: size the thumbnail grid to the
                // viewport instead of nesting a tiny vertical scroll area in
                // the tooltip. Wider popups handle models with many skins.
                let screen = ui.ctx().content_rect().size();
                // Fit the *entire* variant collection in one tooltip. Instead
                // of a nested scroll, select the columns/rows that maximize
                // icon size within the available viewport. This also avoids
                // overflow on skins with dozens of variants.
                let available_h = (screen.y * 0.75 - 44.0).max(160.0);
                let available_w = (screen.x * 0.80).max(215.0);
                let max_cols = ((available_w - 12.0) / 24.0).floor().max(1.0) as usize;
                let mut best = (1usize, 1.0_f32);
                for candidate in 1..=variants.len().min(max_cols) {
                    let rows = variants.len().div_ceil(candidate);
                    let width_per = (available_w - 12.0 - (candidate - 1) as f32 * 5.0)
                        / candidate as f32;
                    let height_per = (available_h - (rows - 1) as f32 * 7.0)
                        / rows as f32;
                    let tile_h = height_per.min(width_per / 0.765).min(85.0);
                    if tile_h > best.1 { best = (candidate, tile_h); }
                }
                let cols = best.0;
                let cell_height = best.1.max(6.0);
                let cell_width = cell_height * 0.765;
                let popup_w = cols as f32 * (cell_width + 5.0) + 12.0;
                ui.set_min_width(popup_w.max(215.0));
                label(ui, entry.model_name.as_str(), 13.0, theme::TEXT);
                ui.add_space(5.0);
                egui::Grid::new(("studio_skin_variants", &entry.model_name))
                    .num_columns(cols).spacing(egui::vec2(5.0, 7.0)).show(ui, |ui| {
                        for (i, skin) in variants.iter().enumerate() {
                            let (tile, click) = ui.allocate_exact_size(
                                egui::vec2(cell_width, cell_height), egui::Sense::click());
                            let chosen = skin.value.eq_ignore_ascii_case(
                                self.profile_model_staged.as_deref()
                                    .unwrap_or(&self.solo_client_info.model_cvar()));
                            ui.painter().rect_stroke(tile, 3.0, egui::Stroke::new(1.0,
                                if chosen { SKY } else { EDGE }), egui::StrokeKind::Inside);
                            if let Some(tex) = self.profile_model_icon_texture(skin) {
                                let image = egui::Rect::from_min_max(tile.min + egui::vec2(2.0,2.0),
                                    egui::pos2(tile.right()-2.0, (tile.bottom()-20.0).max(tile.top()+3.0)));
                                ui.painter().image(tex.id(), image, egui::Rect::from_min_max(
                                    egui::Pos2::ZERO, egui::pos2(1.0,1.0)), egui::Color32::WHITE);
                            } else {
                                let (placeholder, color) = Self::studio_skin_icon_fallback(skin);
                                ui.painter().text(tile.center() - egui::vec2(0.0, 11.0),
                                    egui::Align2::CENTER_CENTER, placeholder,
                                    egui::FontId::proportional(12.0), color);
                            }
                            ui.painter().text(egui::pos2(tile.center().x, tile.bottom()-10.0),
                                egui::Align2::CENTER_CENTER, &skin.skin_name,
                                egui::FontId::proportional(if cell_width < 40.0 { 7.0 } else { 9.0 }), theme::TEXT);
                            if click.clicked() { pick = Some(skin.value.clone()); }
                            click.context_menu(|ui| {
                                studio_model_source_popup(ui, &self.base, self.game.as_deref(), self.video.allow_asset_overrides, skin);
                            });
                            if (i + 1) % cols == 0 { ui.end_row(); }
                        }
                    });
            });
            if let Some(value) = pick { self.studio_stage_model(&value, ui.ctx()); }
            response.clicked()
        } else {
            response.on_hover_text(format!("{} / {}", entry.model_name, entry.skin_name)).clicked()
        }
    }

    /// JKA model userinfo: `jedi_hm/head_a1|torso_d1|lower_a1`.
    /// The skin loader inserts the leading pipe for `RE_SplitSkins`.
    /// Only advertise combinations represented by genuine installed skin icons.
    fn studio_multipart_skin(&mut self, ui: &mut egui::Ui, model: &str, active: &str) {
        // Use references rather than copying every multipart skin on every UI tick.
        let mut parts: [Vec<&ProfileModelEntry>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for entry in self.profile_models.iter().filter(|e| e.model_name.eq_ignore_ascii_case(model)) {
            for (i, prefix) in ["head_", "torso_", "lower_"].into_iter().enumerate() {
                if entry.skin_name.starts_with(prefix) { parts[i].push(entry); }
            }
        }
        if parts.iter().any(Vec::is_empty) { return; }
        let skin = active.split_once('/').map_or("", |(_, skin)| skin);
        let current = if skin.contains('|') { Some(skin.trim_start_matches('|').split('|').collect::<Vec<_>>()) } else { None };
        let mut selected: [String; 3] = std::array::from_fn(|i| {
            current.as_ref().and_then(|fields| fields.get(i))
                .filter(|field| parts[i].iter().any(|e| e.skin_name.as_str() == **field))
                .map(|field| (*field).to_owned())
                .unwrap_or_else(|| parts[i][0].skin_name.clone())
        });
        ui.add_space(10.0);
        cap(ui, "JEDI CUSTOMIZATION", "Head · Torso · Legs");
        let mut changed = false;
        for (i, title) in ["HEAD", "TORSO", "LEGS"].into_iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(title).size(10.0).color(MUTED));
                let dropdown = egui::ComboBox::from_id_salt(("studio_multipart", i))
                    .selected_text(&selected[i])
                    .width((ui.available_width()-75.0).max(110.0))
                    .show_ui(ui, |ui| {
                        for item in &parts[i] {
                            if ui.selectable_value(&mut selected[i], item.skin_name.clone(),
                                &item.skin_name).changed() { changed = true; }
                        }
                    }).response;
                if let Some(next) = studio_selector_arrow(ui, &dropdown, &selected[i],
                    &parts[i].iter().map(|item| item.skin_name.as_str()).collect::<Vec<_>>()) {
                    selected[i] = next;
                    changed = true;
                }
                if dropdown.clicked_by(egui::PointerButton::Middle) {
                    selected[i] = parts[i][0].skin_name.clone();
                    changed = true;
                }
            });
        }
        if changed {
            self.studio_stage_model(&format!("{model}/{}|{}|{}", selected[0],selected[1],selected[2]), ui.ctx());
        }
    }

    /// Display settings belong together, away from character appearance.
    /// `cg_forceModel` is TaystJK's authoritative off / all / ally,enemy
    /// control; `cg_stylePlayer` stores cosmetic presentation bits.
    fn studio_visibility(&mut self, ui: &mut egui::Ui) {
        cap(ui, "OTHER PLAYERS", "");
        let parsed = crate::cgame::ForcedPlayerModels::parse(&self.force_model).ok().flatten();
        let mut mode = match &parsed {
            None => 0u8,
            Some(value) if value.split => 2,
            Some(_) => 1,
        };
        let own = self.solo_client_info.model_cvar();
        let mut ally = parsed.as_ref().map(|v| v.ally.clone()).unwrap_or_else(|| own.clone());
        let mut enemy = parsed.as_ref().map(|v| v.enemy.clone()).unwrap_or_else(|| own.clone());
        ui.horizontal_wrapped(|ui| {
            if ui.selectable_label(mode == 0, "Original").clicked() { mode = 0; }
            if ui.selectable_label(mode == 1, "Force all").clicked() {
                if mode == 0 { ally = own.clone(); } mode = 1;
            }
            if ui.selectable_label(mode == 2, "Ally / enemy").clicked() {
                if mode == 0 { ally = own.clone(); enemy = own.clone(); }
                mode = 2;
            }
        });
        let mut choices: Vec<String> = self.profile_models.iter().map(|m| m.value.clone()).collect();
        choices.sort_by_key(|v| v.to_ascii_lowercase());
        choices.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        if mode != 0 {
            let selector = |ui: &mut egui::Ui, id: &str, title: &str, picked: &mut String| {
                ui.horizontal(|ui| {
                    label(ui, title, 11.0, MUTED);
                    let response = egui::ComboBox::from_id_salt(id)
                        .selected_text(picked.as_str()).width(190.0).show_ui(ui, |ui| {
                            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                                for model in &choices { ui.selectable_value(picked, model.clone(), model); }
                            });
                        }).response;
                    if let Some(next) = studio_selector_arrow(ui, &response, picked,
                        &choices.iter().map(String::as_str).collect::<Vec<_>>()) {
                        *picked = next;
                    }
                    if response.clicked_by(egui::PointerButton::Middle) { *picked = own.clone(); }
                });
            };
            selector(ui, "studio_force_model_ally", if mode == 1 { "All" } else { "Ally" }, &mut ally);
            if mode == 2 { selector(ui, "studio_force_model_enemy", "Enemy", &mut enemy); }
        }
        let requested = match mode {
            0 => "0".to_owned(),
            1 => ally,
            _ => format!("{ally},{enemy}"),
        };
        if !requested.eq_ignore_ascii_case(&self.force_model) {
            if let Err(error) = self.set_console_cvar("cg_forceModel", &requested) {
                self.console_status = error;
            }
        }
        ui.add_space(20.0);
        cap(ui, "COSMETIC VISIBILITY", "");
        const HIDE: u32 = 65536;
        const SEASONAL: u32 = 1048576;
        let flags = self.japro_cg.style_player;
        let mut hide = flags & HIDE != 0;
        let mut seasonal = flags & SEASONAL != 0;
        let mut next = flags;
        if ui.checkbox(&mut hide, "Hide other players’ cosmetics")
            .on_hover_text("Middle-click to restore visible cosmetics")
            .clicked_by(egui::PointerButton::Middle) { hide = false; }
        if ui.checkbox(&mut seasonal, "Seasonal cosmetics")
            .on_hover_text("Middle-click to turn off seasonal cosmetics")
            .clicked_by(egui::PointerButton::Middle) { seasonal = false; }
        if hide { next |= HIDE; } else { next &= !HIDE; }
        if seasonal { next |= SEASONAL; } else { next &= !SEASONAL; }
        if next != flags {
            if let Err(error) = self.set_console_cvar("cg_stylePlayer", &next.to_string()) {
                self.console_status = error;
            }
        }
    }

    fn studio_stage_model(&mut self, value: &str, ctx: &egui::Context) {
        // Selection refers to the old model's anatomy: start the new model
        // in Whole body so a previous named region cannot leak across rigs.
        ctx.data_mut(|data| data.remove::<BodyEditor>(body_editor_id()));
        self.profile_body_highlight = None;
        self.focus_profile_studio_camera("");
        self.profile_model_staged = if value.eq_ignore_ascii_case(&self.solo_client_info.model_cvar()) {
            None
        } else { Some(value.to_owned()) };
        self.profile_preview_key = None;
        // A new model may have no fitted anatomy. Do not show the old model's
        // sliders while its async Ghoul2 profile is loading.
        self.profile_studio_world_anchors[5] = None;
        self.profile_studio_world_anchors[6] = None;
    }

    /// Tint is a CHARACTER setting, not a model-library filter. These are the
    /// real userinfo char_color_* cvars.
    fn studio_character_tint(&mut self, ui: &mut egui::Ui) {
        let active = self.network.char_color;
        let mut rgb = active.map(|v| v as f32 / 255.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 7.0;
            label(ui, "MODEL COLOR", 10.0, MUTED);
            let picker = ui.color_edit_button_rgb(&mut rgb)
                .on_hover_text("Model RGB tint · middle-click for original white");
            let chosen = if picker.clicked_by(egui::PointerButton::Middle) {
                Some([255u8; 3])
            } else if picker.changed() {
                Some(rgb.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8))
            } else { None };
            if let Some(chosen) = chosen.filter(|rgb| *rgb != active) {
                for (index, cvar) in ["char_color_red", "char_color_green", "char_color_blue"]
                    .into_iter().enumerate() {
                    if let Err(error) = self.profile_set_cvar(cvar, &chosen[index].to_string()) {
                        self.console_status = error;
                    }
                }
                self.profile_preview_key = None;
            }
        });
    }

    /// Body-shape controls belong to the character they change. The cvars are
    /// shared with gameplay and config persistence; there are no Profile-only
    /// copies or additional background remesh loops.
    fn studio_body_size_slider(&mut self, ui: &mut egui::Ui, glute: bool) {
        let (title, cvar, max, current, help) = if glute {
            ("GLUTE SIZE", "r_jiggleGluteSize", 4.0,
             self.video.jiggle_glute_size,
             "Peak glute mass in JKA units; negative reduces. Double-click or middle-click to reset.")
        } else {
            ("BREAST SIZE", "r_jiggleBreastSize", 3.0,
             self.video.jiggle_breast_size,
             "Peak chest mass in JKA units; negative reduces. Double-click or middle-click to reset.")
        };
        let reset_label = studio_deformation_reset_label(ui, egui::RichText::new(title).size(10.0).color(SKY_SOFT));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            let mut size = current;
            let response = ui.add(egui::Slider::new(&mut size, -2.0..=max)
                .step_by(0.10).show_value(false))
                .on_hover_text(help);
            let response = response.labelled_by(reset_label.id);
            if reset_label.clicked() || studio_deformation_slider_reset(&response) {
                size = 0.0;
            }
            label(ui, format!("{size:+.1} u"), 10.0, theme::TEXT);
            if size != current {
                if let Err(error) = self.set_console_cvar(cvar, &size.to_string()) {
                    self.console_status = error;
                } else {
                    self.profile_preview_key = None;
                }
            }
        });
    }

    fn studio_body_shape_controls(&mut self, ui: &mut egui::Ui, current: &str, available_h: f32, stage: egui::Rect) {
        use jka_assets::body_shape::anny::{Details, EXTRA_CONTROLS, LOCAL_CONTROLS, LOCAL_COUNT, FACIAL_CONTROLS};
        let info = crate::cgame::ClientInfo::solo_model(current);
        let available = self.asset_preview_player_presenter.as_ref()
            .is_some_and(|presenter| presenter.profile_body_shape_available(&info));
        let mut editor = ui.ctx().data_mut(|d| d.get_temp::<BodyEditor>(body_editor_id())).unwrap_or_default();
        let previous_group = editor.group.clone();
        cap(ui, "BODY SHAPE", if editor.group.is_empty() { "Whole body" } else { &editor.group });
        // The two pre-existing jiggle offsets stay on the stage beside Anny,
        // not in the model browser. Preserve per-model anchor availability.
        let breast = self.studio_project_anchor(stage, 5).is_some();
        let glute = self.studio_project_anchor(stage, 6).is_some();
        if editor.group.is_empty() || editor.group == "all" || editor.group == "breast" || editor.group == "buttocks" {
            if breast && (editor.group.is_empty() || editor.group == "all" || editor.group == "breast") {
                self.studio_body_size_slider(ui, false);
            }
            if glute && (editor.group.is_empty() || editor.group == "all" || editor.group == "buttocks") {
                self.studio_body_size_slider(ui, true);
            }
        }
        if !available {
            label(ui, "Anny shaping needs a mapped humanoid skeleton.", 10.0, MUTED);
            ui.ctx().data_mut(|d| d.insert_temp(body_editor_id(), editor));
            return;
        }
        let mut groups: Vec<_> = LOCAL_CONTROLS.iter().map(|c|c.1).collect();
        groups.sort_unstable(); groups.dedup();
        let body_choices: Vec<_> = std::iter::once("")
            .chain(groups.iter().copied())
            .chain(["facial actions", "all"]).collect();
        let region = egui::ComboBox::from_id_salt("anny_body_region")
            .selected_text(if editor.group.is_empty() { "Whole body" } else { &editor.group })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut editor.group, String::new(), "Whole body");
                for group in &groups { ui.selectable_value(&mut editor.group, (*group).to_owned(), *group); }
                ui.selectable_value(&mut editor.group, "facial actions".to_owned(), "Facial actions");
                ui.selectable_value(&mut editor.group, "all".to_owned(), "All controls");
            }).response;
        if let Some(next) = studio_selector_arrow(ui, &region, &editor.group, &body_choices) {
            editor.group = next;
        }
        ui.add(egui::TextEdit::singleline(&mut editor.search).hint_text("Find a slider"));
        let needle = editor.search.to_ascii_lowercase();
        let searching = !needle.trim().is_empty();
        let show_global = editor.group.is_empty() || editor.group == "all";
        let mut details = self.video.body_details;
        let original = details;
        let defaults = Details::default();
        let show_gender = show_global
            && (!searching || studio_anny_detail_title("gender").to_ascii_lowercase().contains(&needle));
        ui.add_enabled_ui(available, |ui| {
        // Keep gender visible while the selected part's controls scroll.
        if show_gender {
            Self::studio_anny_detail_slider(ui, &mut details, 0, "gender", defaults.0[0]);
        }
        egui::ScrollArea::vertical().id_salt("anny_body_sliders")
            .max_height((available_h - 180.0 - if show_gender { 38.0 } else { 0.0 }).max(80.0))
            .auto_shrink([false, false]).show(ui, |ui| {
            if show_global {
            cap(ui, "PHENOTYPES", "");
            for (title, cvar, current, default) in [
                ("Muscle", "r_bodyMuscle", self.video.body_muscle, 0.5),
                ("Weight", "r_bodyWeight", self.video.body_weight, 0.5),
                ("Proportions", "r_bodyProportions", self.video.body_proportions, 0.5),
                ("Height", "r_bodyHeight", self.video.body_height, 0.5),
                ("Age", "r_bodyAge", self.video.body_age, 0.5),
            ] {
                if searching && !title.to_ascii_lowercase().contains(&needle) { continue; }
                let id = egui::Id::new(("anny_macro", cvar));
                let mut value = ui.ctx().data_mut(|data| data.get_temp::<f32>(id)).unwrap_or(current);
                let (response, reset_label) = ui.horizontal(|ui| {
                    let response = ui.add(egui::Slider::new(&mut value, 0.0..=1.0).step_by(0.05))
                        .on_hover_text("Release to update the character. Double-click or middle-click to reset.");
                    let reset_label = studio_deformation_reset_label(ui, title);
                    let response = response.labelled_by(reset_label.id);
                    (response, reset_label)
                }).inner;
                let reset = reset_label.clicked() || studio_deformation_slider_reset(&response);
                if reset { value = default; }
                if response.dragged() && !reset {
                    ui.ctx().data_mut(|data| data.insert_temp(id, value));
                } else {
                    ui.ctx().data_mut(|data| data.remove::<f32>(id));
                    if available && value != current {
                        if let Err(error) = self.set_console_cvar(cvar, &value.to_string()) {
                            self.console_status = error;
                        } else { self.profile_preview_key = None; }
                    }
                }
            }
            }
            // Anny normalizes the three artist-authored ancestry blends.
            // Display only the two independent degrees of freedom; the third
            // ratio is the remainder. Keep named cvar serialization backward
            // compatible for archived profiles.
            if show_global && (!searching || ["african", "asian", "caucasian", "blend", "ancestry"]
                .iter().any(|name| name.contains(needle.as_str()))) {
                cap(ui, "ANNY APPEARANCE BLEND", "2 controls");
                label(ui, "Artistic morph presets; not a measure of ancestry or identity.", 10.0, MUTED);
                Self::studio_anny_ancestry_slider(ui, &mut details, 0);
                Self::studio_anny_ancestry_slider(ui, &mut details, 1);
            }
            if editor.group == "breast" || editor.group == "all" {
            if !show_global { cap(ui, "BREAST PHENOTYPES", "Anny"); }
            for (i, name) in EXTRA_CONTROLS.iter().enumerate() {
                if i == 0 || (3..=5).contains(&i) {continue;} // Gender pinned, ancestry combined above.
                if editor.group != "all" && !matches!(i, 1 | 2) {continue;}
                if searching && !format!("{name} {}", studio_anny_detail_title(name)).to_ascii_lowercase().contains(&needle) {continue;}
                Self::studio_anny_detail_slider(ui, &mut details, i, name, defaults.0[i]);
            }
            }
            if !editor.group.is_empty() {
                let mut previous_group = "";
                for (i, &(name, group)) in LOCAL_CONTROLS.iter().enumerate() {
                    if editor.group != "all" && group != editor.group {continue;}
                    if searching && !format!("{group} {name}").contains(&needle) {continue;}
                    if previous_group != group {ui.add_space(8.0);cap(ui, &group.to_ascii_uppercase(), "");previous_group=group;}
                    Self::studio_anny_detail_slider(ui, &mut details, 6+i, name, 0.0);
                }
                if editor.group == "facial actions" || editor.group == "all" {
                    cap(ui, "FACIAL ACTIONS", "");
                    for (i, &name) in FACIAL_CONTROLS.iter().enumerate() {
                        if searching && !name.to_ascii_lowercase().contains(&needle) {continue;}
                        Self::studio_anny_detail_slider(ui, &mut details, 6+LOCAL_COUNT+i, name, 0.0);
                    }
                }
            }
        });
        });
        if available && details != original {
            if let Err(error) = self.set_console_cvar("r_bodyAnny", &details.encode()) {
                self.console_status = error;
            } else { self.profile_preview_key = None; }
        }
        let mut reset_part = false;
        let mut reset_body = false;
        ui.horizontal_wrapped(|ui| {
            reset_part = ui.add_enabled(available, egui::Button::new("Reset selected part")).clicked();
            reset_body = ui.add_enabled(available, egui::Button::new("Reset whole body"))
                .on_hover_text("Restore all Anny controls and breast/glute size offsets to neutral.")
                .clicked();
        });
        if reset_body {
            self.studio_reset_whole_body_shape(ui.ctx());
        } else if reset_part {
            if editor.group.is_empty() || editor.group=="all" {
                for cvar in ["r_bodyMuscle","r_bodyWeight","r_bodyProportions","r_bodyHeight","r_bodyAge"] {
                    if let Err(error)=self.set_console_cvar(cvar,"0.5") {self.console_status=error;}
                    ui.ctx().data_mut(|d|d.remove::<f32>(egui::Id::new(("anny_macro",cvar))));
                }
                details.0[..6].copy_from_slice(&defaults.0[..6]);
            }
            if editor.group == "breast" {
                details.0[1..=2].copy_from_slice(&defaults.0[1..=2]);
                if let Err(error) = self.set_console_cvar("r_jiggleBreastSize", "0") { self.console_status = error; }
            }
            if editor.group == "buttocks" {
                if let Err(error) = self.set_console_cvar("r_jiggleGluteSize", "0") { self.console_status = error; }
            }
            for (i,&(_,group)) in LOCAL_CONTROLS.iter().enumerate() {
                if editor.group=="all" || editor.group==group {details.0[6+i]=0.;}
            }
            if editor.group=="all" || editor.group=="facial actions" {details.0[6+LOCAL_COUNT..].fill(0.);}
            if let Err(error)=self.set_console_cvar("r_bodyAnny",&details.encode()) {self.console_status=error;}
            self.profile_preview_key=None;
            // Reset must also clear a slider's uncommitted drag value.
            for i in 0..details.0.len() {ui.ctx().data_mut(|d|d.remove::<f32>(egui::Id::new(("anny_detail",i))));}
        }
        if editor.group != previous_group {
            self.focus_profile_studio_camera(&editor.group);
        }
        ui.ctx().data_mut(|d|d.insert_temp(body_editor_id(),editor));
    }

    fn studio_reset_whole_body_shape(&mut self, ctx: &egui::Context) {
        for cvar in ["r_bodyMuscle", "r_bodyWeight", "r_bodyProportions", "r_bodyHeight", "r_bodyAge"] {
            if let Err(error) = self.set_console_cvar(cvar, "0.5") { self.console_status = error; }
            ctx.data_mut(|d|d.remove::<f32>(egui::Id::new(("anny_macro", cvar))));
        }
        for (cvar, value) in [("r_bodyAnny", ""), ("r_jiggleBreastSize", "0"), ("r_jiggleGluteSize", "0")] {
            if let Err(error) = self.set_console_cvar(cvar, value) { self.console_status = error; }
        }
        for i in 0..jka_assets::body_shape::anny::DETAIL_COUNT {
            ctx.data_mut(|d|d.remove::<f32>(egui::Id::new(("anny_detail", i))));
        }
        self.profile_preview_key = None;
    }

    /// A simplex blend: changing one axis rescales the remaining two while
    /// preserving their ratio, so neither control can push the sum over 100%.
    fn studio_anny_ancestry_slider(ui: &mut egui::Ui,
        details: &mut jka_assets::body_shape::anny::Details, which: usize) {
        let sum: f32 = details.0[3..6].iter().sum();
        let ratios = if sum > 0.0 {
            [details.0[3] / sum, details.0[4] / sum, details.0[5] / sum]
        } else { [1.0 / 3.0; 3] };
        let current = ratios[which];
        let id = egui::Id::new(("anny_ancestry_mix", which));
        let mut value = ui.ctx().data_mut(|d|d.get_temp::<f32>(id)).unwrap_or(current);
        let title = if which == 0 { "African blend" } else { "Asian blend" };
        let reset_label = studio_deformation_reset_label(ui, title)
            .on_hover_text("Reset all three Anny ancestry blend weights to equal values");
        let response = ui.add(egui::Slider::new(&mut value, 0.0..=1.0).step_by(1.0 / 63.0)
            .custom_formatter(|x, _| format!("{:.0}%", x * 100.0)))
            .on_hover_text("The third blend weight is inferred automatically; release to apply.");
        let response = response.labelled_by(reset_label.id);
        let reset = reset_label.clicked() || studio_deformation_slider_reset(&response);
        if reset {
            details.0[3..6].fill(0.5);
            ui.ctx().data_mut(|d| {
                d.remove::<f32>(egui::Id::new(("anny_ancestry_mix", 0)));
                d.remove::<f32>(egui::Id::new(("anny_ancestry_mix", 1)));
            });
        } else if response.dragged() {
            ui.ctx().data_mut(|d|d.insert_temp(id, value));
        } else {
            ui.ctx().data_mut(|d|d.remove::<f32>(id));
            if (current-value).abs() > 0.00001 {
                let remaining = 1.0 - value;
                let other_total = 1.0 - current;
                let mut result = ratios;
                result[which] = value;
                for i in 0..3 {
                    if i != which {
                        result[i] = remaining * if other_total > 0.00001 {
                            ratios[i] / other_total
                        } else { 0.5 };
                    }
                }
                details.0[3..6].copy_from_slice(&result);
            }
        }
    }

    fn studio_anny_detail_slider(ui: &mut egui::Ui, details: &mut jka_assets::body_shape::anny::Details,
        index: usize, name: &str, default: f32) {
        use jka_assets::body_shape::anny::Details;
        let id = egui::Id::new(("anny_detail", index));
        let current = details.0[index];
        let mut value = ui.ctx().data_mut(|d|d.get_temp::<f32>(id)).unwrap_or(current);
        // Keep Anny's exact label in the tooltip; trim only the positive target
        // suffix in the visible label, since each slider owns both opposites.
        let title = studio_anny_detail_title(name);
        let reset_label = studio_deformation_reset_label(ui, title).on_hover_text(name);
        let response = ui.add(egui::Slider::new(&mut value, Details::range(index))
            .step_by(0.05))
            .on_hover_text(format!("{name}\nRelease to update. Double-click or middle-click to reset."));
        let response = response.labelled_by(reset_label.id);
        let reset = reset_label.clicked() || studio_deformation_slider_reset(&response);
        if reset {value=default;}
        if response.dragged() && !reset {ui.ctx().data_mut(|d|d.insert_temp(id,value));}
        else {ui.ctx().data_mut(|d|d.remove::<f32>(id));details.0[index]=value;}
    }

    /// All saber selection takes place on the character stage. Source of truth:
    /// the existing ext_data/sabers catalog and userinfo cvars.
    fn studio_saber_mode(&mut self, ui: &mut egui::Ui) {
        let secondary = !self.network.saber2.is_empty()
            && !self.network.saber2.eq_ignore_ascii_case("none")
            && !self.network.saber2.eq_ignore_ascii_case("remove");
        let staff = self.profile_sabers.iter().any(|entry|
            entry.name.eq_ignore_ascii_case(&self.network.saber1)
                && Self::profile_saber_is_staff(entry));
        let active = if secondary { 1 } else if staff { 2 } else { 0 };
        let mut picked = None;
        for (mode, title) in [(0, "Single"), (1, "Dual"), (2, "Staff")] {
            if ui.selectable_label(active == mode, title).clicked() {
                picked = Some(mode);
            }
        }
        if picked.is_some() {
            self.focus_profile_studio_camera("saber:both");
        }
        if let Some(mode) = picked.filter(|mode| *mode != active) {
            let singles: Vec<_> = self.profile_sabers.iter()
                .filter(|entry| Self::profile_saber_is_single(entry)).collect();
            let staffs: Vec<_> = self.profile_sabers.iter()
                .filter(|entry| Self::profile_saber_is_staff(entry)).collect();
            let first = singles.iter().find(|entry| entry.name.eq_ignore_ascii_case("single_1"))
                .or_else(|| singles.first()).map(|entry| entry.name.clone());
            let first_staff = staffs.iter().find(|entry| entry.name.eq_ignore_ascii_case("dual_1"))
                .or_else(|| staffs.first()).map(|entry| entry.name.clone());
            let primary = if mode == 2 { first_staff } else if staff { first } else { None };
            if let Some(primary) = primary {
                if let Err(error) = self.profile_set_cvar("saber1", &primary) { self.console_status = error; }
            }
            let secondary = if mode == 1 {
                if secondary { self.network.saber2.clone() }
                else { self.network.saber1.clone() }
            } else { "none".to_owned() };
            if let Err(error) = self.profile_set_cvar("saber2", &secondary) { self.console_status = error; }
            self.profile_preview_key = None;
        }
    }

    fn studio_saber_editor(&mut self, ui: &mut egui::Ui, hand: usize) {
        use crate::net::{base_saber_rgb_packed, SABER_RGB};
        let dual = !self.network.saber2.is_empty()
            && !self.network.saber2.eq_ignore_ascii_case("none")
            && !self.network.saber2.eq_ignore_ascii_case("remove");
        let staff = self.profile_sabers.iter().any(|entry|
            entry.name.eq_ignore_ascii_case(&self.network.saber1)
                && Self::profile_saber_is_staff(entry));
        ui.set_min_width(216.0);
        label(ui, if hand == 0 { "RIGHT HAND" } else { "LEFT HAND" }, 10.0, SKY_SOFT);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let cvar = if hand == 0 { "saber1" } else { "saber2" };
            let current = if hand == 0 { self.network.saber1.clone() } else { self.network.saber2.clone() };
            let mut selected = current.clone();
            let entry_label = self.profile_sabers.iter()
                .find(|entry| entry.name.eq_ignore_ascii_case(&current))
                .map(Self::profile_saber_label).unwrap_or(current.as_str()).to_owned();
            // One filtered source for both mouse selection and arrow navigation.
            // Keep the real internal saber name in userinfo; colors affect labels only.
            let available: Vec<_> = self.profile_sabers.iter().filter(|saber| {
                if hand == 1 { Self::profile_saber_is_single(saber) }
                else if staff { Self::profile_saber_is_staff(saber) }
                else { Self::profile_saber_is_single(saber) }
            }).collect();
            let combo = egui::ComboBox::from_id_salt(("studio_saber_hilt", hand))
                .selected_text(jka_colored_text(
                    if hand == 1 && !dual { "None" } else { &entry_label },
                    12.0, theme::TEXT))
                .width(141.0).show_ui(ui, |ui| {
                    if hand == 1 {
                        ui.selectable_value(&mut selected, "none".to_owned(), "None");
                    }
                    egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                        for saber in &available {
                            ui.selectable_value(&mut selected, saber.name.clone(),
                                jka_colored_text(Self::profile_saber_label(saber), 12.0, theme::TEXT));
                        }
                    });
                }).response;
            let choices: Vec<_> = std::iter::once("none").filter(|_| hand == 1)
                .chain(available.iter().map(|saber| saber.name.as_str())).collect();
            if let Some(next) = studio_selector_arrow(ui, &combo, &selected, &choices) {
                selected = next;
            }
            if combo.clicked() { self.profile_studio_cosmetic_group = 0; }
            if combo.clicked() || combo.clicked_by(egui::PointerButton::Middle) || selected != current {
                self.focus_profile_studio_camera(if hand == 0 { "saber:right" } else { "saber:left" });
            }
            if combo.clicked_by(egui::PointerButton::Middle) {
                selected = if hand == 0 { "single_1".to_owned() } else { "none".to_owned() };
            }
            if selected != current {
                if hand == 1 && staff && selected != "none" {
                    let single = self.profile_sabers.iter()
                        .filter(|entry| Self::profile_saber_is_single(entry))
                        .find(|entry| entry.name.eq_ignore_ascii_case("single_1"))
                        .or_else(|| self.profile_sabers.iter().find(|entry| Self::profile_saber_is_single(entry)))
                        .map(|entry| entry.name.clone());
                    if let Some(single) = single {
                        if let Err(error) = self.profile_set_cvar("saber1", &single) {
                            self.console_status = error;
                        }
                    }
                }
                if let Err(error) = self.profile_set_cvar(cvar, &selected) {
                    self.console_status = error;
                }
                self.profile_preview_key = None;
            }
            let color_cvar = if hand == 0 { "color1" } else { "color2" };
            let rgb_cvar = if hand == 0 { "cp_sbRGB1" } else { "cp_sbRGB2" };
            let color = if hand == 0 { self.network.color1 } else { self.network.color2 };
            let packed = if hand == 0 { self.network.sb_rgb1 } else { self.network.sb_rgb2 };
            let packed = if i32::from(color) == SABER_RGB && packed != 0 { packed }
                else { base_saber_rgb_packed(color) };
            let mut rgb = [(packed & 255) as f32 / 255.0,
                ((packed >> 8) & 255) as f32 / 255.0,
                ((packed >> 16) & 255) as f32 / 255.0];
            let picker = ui.color_edit_button_rgb(&mut rgb)
                .on_hover_text("Blade color · Middle-click to restore blue");
            if picker.clicked() || picker.clicked_by(egui::PointerButton::Middle) {
                self.focus_profile_studio_camera(if hand == 0 { "saber:right" } else { "saber:left" });
            }
            if picker.changed() {
                let bytes = rgb.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u32);
                let packed = bytes[0] | bytes[1] << 8 | bytes[2] << 16;
                if let Err(error) = self.profile_set_cvar(rgb_cvar, &packed.to_string()) {
                    self.console_status = error;
                }
                let cosmetic = if hand == 0 { &self.network.global_hat } else { &self.network.global_cape };
                let value = crate::global_cosmetics::encode(SABER_RGB as u8, cosmetic);
                if let Err(error) = self.profile_set_cvar(color_cvar, &value) {
                    self.console_status = error;
                }
                self.profile_preview_key = None;
            }
            if picker.clicked_by(egui::PointerButton::Middle) {
                let cosmetic = if hand == 0 { &self.network.global_hat } else { &self.network.global_cape };
                let color = crate::global_cosmetics::encode(4, cosmetic);
                for (cvar, value) in [(color_cvar, color.as_str()), (rgb_cvar, "0")] {
                    if let Err(error) = self.profile_set_cvar(cvar, value) { self.console_status = error; }
                }
                self.profile_preview_key = None;
            }
        });
    }

    /// Projection matches Renderer::render_asset_preview's exact camera/FOV,
    /// viewport size and JKA-to-render coordinate conversion.
    fn studio_project_anchor(&self, rect: egui::Rect, anchor: usize) -> Option<egui::Pos2> {
        let point = self.profile_studio_world_anchors.get(anchor).copied().flatten()?;
        let render_point = crate::scene::render_position(point);
        let local = self.asset_preview_camera().project_to_screen(
            rect.width().max(1.0) as u32, rect.height().max(1.0) as u32,
            glam::Vec3::from_array(render_point))?;
        let screen = rect.min + local.to_vec2();
        rect.contains(screen).then_some(screen)
    }

    fn studio_character_stage(&mut self, ui: &mut egui::Ui, width: f32, height: f32, _elapsed: f32) {
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(width, height), egui::Sense::click_and_drag());
        self.set_asset_preview_viewport(rect, ui.ctx().pixels_per_point());
        let now = std::time::Instant::now();
        let dt = now.duration_since(self.profile_studio_last_frame).as_secs_f32().clamp(0.0, 0.08);
        self.profile_studio_last_frame = now;

        // No perpetual rotation. A mouse fling rotates the model and settles
        // with inertial damping; clicking the stage does not reset the camera.
        if response.dragged() {
            let dx = ui.ctx().input(|input| input.pointer.delta().x);
            if dx.abs() > 0.01 && dt > 0.0001 {
                self.profile_preview_yaw = (self.profile_preview_yaw + dx * 0.42).rem_euclid(360.0);
                self.profile_studio_camera.target_yaw = self.profile_preview_yaw;
                self.profile_studio_spin_velocity = (dx * 0.42 / dt).clamp(-560.0, 560.0);
                self.profile_preview_key = None;
            }
        } else if self.profile_studio_spin_velocity.abs() > 0.08 {
            let velocity = self.profile_studio_spin_velocity;
            self.profile_preview_yaw = (self.profile_preview_yaw + velocity * dt).rem_euclid(360.0);
            self.profile_studio_camera.target_yaw = self.profile_preview_yaw;
            self.profile_studio_spin_velocity *= (-3.4 * dt).exp();
            self.profile_preview_key = None;
        }
        if response.hovered() {
            let scroll = ui.ctx().input(|input| input.smooth_scroll_delta.y);
            if scroll.abs() > 0.01 {
                self.profile_preview_zoom = (self.profile_preview_zoom * (-scroll * 0.0015).exp())
                    .clamp(0.45, 2.5);
                self.profile_studio_camera.target_zoom = self.profile_preview_zoom;
                self.profile_preview_key = None;
            }
        }
        // Eye position, angular orbit and distance all interpolate together.
        // Keep the existing live preview/render frame cap as the clock source.
        // Any dragged or inertial orbit has already replaced the angular goal.
        self.tick_profile_studio_camera(dt);
        // JKA's Ghoul2 cranium/cervical post-multiplication path is applied in
        // present_profile_player_preview, after animation but before skinning.
        let target = if response.hovered() && !response.dragged() {
            let pos = ui.ctx().input(|input| input.pointer.hover_pos()).unwrap_or(rect.center());
            [((pos.x - rect.center().x) / (rect.width() * 0.5)).clamp(-1.0,1.0),
             ((pos.y - rect.center().y) / (rect.height() * 0.5)).clamp(-1.0,1.0)]
        } else { [0.0,0.0] };
        let look_gain = (dt * 5.5).min(1.0);
        for i in 0..2 {
            let next = self.profile_studio_head_look[i] +
                (target[i] - self.profile_studio_head_look[i]) * look_gain;
            if (next - self.profile_studio_head_look[i]).abs() > 0.0004 {
                self.profile_studio_head_look[i] = next;
                self.profile_preview_key = None;
            }
        }
        let p = ui.painter().clone();
        let frame = rect.shrink(5.0);
        p.rect_stroke(frame, 5.0, egui::Stroke::new(1.0, EDGE), egui::StrokeKind::Inside);
        let display = self.profile_model_staged.clone()
            .unwrap_or_else(|| self.solo_client_info.model_cvar());
        let model_name = display.split('/').next().unwrap_or(&display);
        p.text(frame.left_bottom() + egui::vec2(18.0, -18.0),
            egui::Align2::LEFT_BOTTOM, model_name.to_ascii_uppercase(),
            egui::FontId::proportional(19.0), theme::TEXT);

        // Keep the animated stage interactive; character editors belong to
        // their dedicated tabs and never cover the emote preview.
        if self.profile_selected_section==PROFILE_EMOTES {
            self.profile_studio_cosmetic_group=0;
            self.profile_body_highlight=None;
            return;
        }

        // Keep the stage editor out of the model's centre and clear selection
        // on genuine empty-stage clicks, not on clicks on floating controls.
        let shape_panel = egui::Rect::from_min_size(
            egui::pos2(rect.right() - 282.0, rect.top() + 137.0),
            egui::vec2(268.0, (rect.height() - 265.0).max(150.0)));
        if self.profile_selected_section == PROFILE_MODEL && response.clicked() {
            if let Some(pointer) = response.interact_pointer_pos() {
                let over_ui = shape_panel.contains(pointer)
                    || (pointer.y < rect.top() + 118.0
                        && (pointer.x < rect.left() + 250.0 || pointer.x > rect.right() - 280.0))
                    || pointer.y > rect.bottom() - 123.0;
                if !over_ui {
                    let camera = self.asset_preview_camera();
                    let viewport = [rect.width().max(1.0) as u32, rect.height().max(1.0) as u32];
                    let ndc = glam::Vec3::new(
                        2.0*(pointer.x-rect.left())/rect.width()-1.0,
                        1.0-2.0*(pointer.y-rect.top())/rect.height(), 0.5);
                    let point = camera.view_projection(viewport[0], viewport[1]).inverse().project_point3(ndc);
                    let direction = (point-camera.position).normalize_or_zero();
                    let info = crate::cgame::ClientInfo::solo_model(&display);
                    let picked = self.asset_preview_player_presenter.as_ref().and_then(|p|
                        p.pick_profile_body(&info, crate::scene::jka_position(camera.position.to_array()),
                            crate::scene::jka_position(direction.to_array())));
                    let mut editor = ui.ctx().data_mut(|d|d.get_temp::<BodyEditor>(body_editor_id())).unwrap_or_default();
                    // Empty space is Global. Only reset the search when the
                    // selection actually changes; clicking a part twice is stable.
                    let group = picked.unwrap_or_default();
                    if editor.group != group {
                        editor.group = group.to_owned();
                        editor.search.clear();
                        self.focus_profile_studio_camera(group);
                    }
                    ui.ctx().data_mut(|d|d.insert_temp(body_editor_id(),editor));
                    if picked.is_some() { self.profile_studio_spin_velocity=0.0; }
                }
            }
        }
        if self.profile_selected_section == PROFILE_MODEL {
            let editor=ui.ctx().data_mut(|d|d.get_temp::<BodyEditor>(body_editor_id())).unwrap_or_default();
            self.profile_body_highlight = (!editor.group.is_empty()
                && editor.group != "all" && editor.group != "facial actions").then(||editor.group.clone());
            let hint = if !editor.group.is_empty() { format!("Editing: {} · click empty space for Global",editor.group) }
                else { "Click a body part to edit · empty space = Global".to_owned() };
            p.text(frame.left_bottom()+egui::vec2(18.0,-43.0),egui::Align2::LEFT_BOTTOM,
                hint,egui::FontId::proportional(11.0),SKY_SOFT);
        } else { self.profile_body_highlight = None; }

        // Name editing lives in the stage header, not a separate navigation tab.
        let name_area = egui::Rect::from_min_size(frame.left_top() + egui::vec2(17.0, 13.0),
            egui::vec2(235.0, 58.0));
        ui.scope_builder(egui::UiBuilder::new().id_salt("studio_stage_name")
            .max_rect(name_area), |ui| {
            label(ui, "PLAYER NAME", 10.0, MUTED);
            ui.add_sized([215.0, 27.0], egui::TextEdit::singleline(&mut self.profile_name_input)
                .hint_text("Player name"));
        });

        // Tint is immediately visible on the 3D character. Keep it near the
        // name rather than conflating it with model-catalog visibility.
        let tint_area = egui::Rect::from_min_size(
            frame.left_top() + egui::vec2(17.0, 77.0), egui::vec2(204.0, 36.0));
        ui.scope_builder(egui::UiBuilder::new().id_salt("studio_stage_tint")
            .max_rect(tint_area), |ui| {
            egui::Frame::new().fill(egui::Color32::from_rgba_premultiplied(16, 24, 34, 235))
                .corner_radius(egui::CornerRadius::same(5))
                .stroke(egui::Stroke::new(1.0, EDGE))
                .inner_margin(egui::Margin::same(7))
                .show(ui, |ui| self.studio_character_tint(ui));
        });

        // Saber modes stay in the stage. Both hand dropdowns and their RGB
        // editors below edit the original, authoritative saber cvars directly.
        let mode_area = egui::Rect::from_min_size(
            egui::pos2(frame.right() - 267.0, frame.top() + 14.0), egui::vec2(251.0, 92.0));
        ui.scope_builder(egui::UiBuilder::new().id_salt("studio_stage_saber_mode")
            .max_rect(mode_area), |ui| {
            ui.horizontal(|ui| self.studio_saber_mode(ui));
            let toggle = ui.selectable_label(self.profile_studio_show_saber, "Show blades");
            if toggle.clicked() || toggle.clicked_by(egui::PointerButton::Middle) {
                self.profile_studio_show_saber = if toggle.clicked_by(egui::PointerButton::Middle) {
                    true
                } else { !self.profile_studio_show_saber };
                self.profile_preview_key = None;
            }
            ui.horizontal(|ui| {
                let mut active = crate::cgame::player_presenter::saber_preview_debug_enabled();
                if ui.checkbox(&mut active, "Saber debug").changed() {
                    crate::cgame::player_presenter::set_saber_preview_debug(active);
                    self.profile_preview_key = None;
                }
                if active && ui.small_button("Copy log").clicked() {
                    ui.ctx().copy_text(
                        crate::cgame::player_presenter::saber_preview_debug_snapshot());
                }
            });
        });
        if crate::cgame::player_presenter::saber_preview_debug_enabled() {
            let report = crate::cgame::player_presenter::saber_preview_debug_snapshot();
            let panel = egui::Rect::from_min_size(
                rect.left_top() + egui::vec2(17.0, 166.0),
                egui::vec2((rect.width() * 0.60).min(710.0).max(180.0), 207.0));
            ui.scope_builder(egui::UiBuilder::new()
                .id_salt("studio_saber_attachment_debug").max_rect(panel), |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_premultiplied(8, 16, 24, 240))
                    .stroke(egui::Stroke::new(1.0, SKY))
                    .inner_margin(egui::Margin::same(9))
                    .show(ui, |ui| {
                        ui.set_width((panel.width() - 18.0).max(1.0));
                        label(ui, "SABER BOLT -> HILT -> BLADE (WORLD SPACE)", 11.0, SKY_SOFT);
                        egui::ScrollArea::both().max_height(175.0).show(ui, |ui| {
                            ui.monospace(if report.is_empty() {
                                "Waiting for the next preview frame…"
                            } else { report.as_str() });
                        });
                    });
            });
        }

        // Callout positions remain legible. Their connector endpoints come
        // exclusively from animated Ghoul2 bolt matrices, projected through
        // the same preview camera used by WGPU (never hardcoded stage percents).
        if self.profile_selected_section==PROFILE_MODEL {
        for (title, bx, by, group, joint) in [
            ("HEADWEAR", 0.19, 0.25, 1u8, 0usize),
            ("FACE", 0.19, 0.35, 2u8, 1usize),
            ("BACK / CAPE", 0.19, 0.51, 3u8, 2usize),
        ] {
            let btn = egui::Rect::from_min_size(
                egui::pos2(rect.left() + rect.width() * bx - 106.0,
                    rect.top() + rect.height() * by - 13.0),
                egui::vec2(123.0, 28.0));
            if let Some(anchor) = self.studio_project_anchor(rect, joint) {
                p.line_segment([btn.right_center(), anchor],
                    egui::Stroke::new(1.1, SKY.linear_multiply(0.8)));
                p.circle_filled(anchor, 3.0, SKY);
            }
            let button = ui.put(btn, egui::Button::new(
                egui::RichText::new(format!("{title}  ▾")).size(10.0))
                .fill(egui::Color32::from_rgba_premultiplied(15, 22, 32, 235))
                .stroke(egui::Stroke::new(1.0, EDGE)))
                .on_hover_text("Middle-click to remove");
            if button.clicked_by(egui::PointerButton::Middle) {
                self.focus_profile_studio_camera(match group {
                    1 => "cosmetic:head", 2 => "cosmetic:face", _ => "cosmetic:back",
                });
                if let Err(error) = self.studio_set_cosmetic_group(group, None) {
                    self.console_status = error;
                }
                self.profile_preview_key = None;
            } else if button.clicked() {
                button.request_focus();
                self.focus_profile_studio_camera(match group {
                    1 => "cosmetic:head", 2 => "cosmetic:face", _ => "cosmetic:back",
                });
                self.profile_studio_cosmetic_group =
                    if self.profile_studio_cosmetic_group == group { 0 } else { group };
            }
        }

        }
        // One floating anatomy editor. Region picking uses the already skinned
        // Ghoul2 body; breast/glute offsets remain the original cvars and only
        // appear when their existing mesh anchors are available.
        if self.profile_selected_section == PROFILE_MODEL {
            let editor = ui.ctx().data_mut(|d|d.get_temp::<BodyEditor>(body_editor_id())).unwrap_or_default();
            // Retain the original model-space breast/glute callouts, now
            // connecting to their rows in the combined stage editor.
            for (joint, selected, row) in [
                (5usize, editor.group.is_empty() || editor.group == "all" || editor.group == "breast", 0usize),
                (6usize, editor.group.is_empty() || editor.group == "all" || editor.group == "buttocks", 1usize),
            ] {
                if !selected { continue; }
                if let Some(anchor) = self.studio_project_anchor(rect, joint) {
                    let endpoint = egui::pos2(shape_panel.left(), shape_panel.top() + 47.0 + row as f32 * 39.0);
                    p.line_segment([anchor, endpoint], egui::Stroke::new(1.1, SKY.linear_multiply(0.8)));
                    p.circle_filled(anchor, 3.0, SKY);
                }
            }
            let current = display.clone();
            ui.scope_builder(egui::UiBuilder::new()
                .id_salt("studio_anny_stage_panel").max_rect(shape_panel), |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_premultiplied(16, 24, 34, 245))
                    .corner_radius(egui::CornerRadius::same(5))
                    .stroke(egui::Stroke::new(1.0, EDGE))
                    .inner_margin(egui::Margin::same(9))
                    .show(ui, |ui| {
                        ui.set_width((shape_panel.width() - 18.0).max(1.0));
                        self.studio_body_shape_controls(ui, &current, shape_panel.height()-18.0, rect);
                    });
            });
        }

        // Both floating saber controls are full editors. Their connector
        // endpoints track the currently evaluated left/right hand tags.
        for (hand, fx, joint) in [(0usize, 0.055f32, 3usize), (1, 0.70, 4)] {
            let panel = egui::Rect::from_min_size(
                egui::pos2(rect.left() + rect.width() * fx,
                    rect.bottom() - 111.0), egui::vec2(227.0, 70.0));
            if let Some(anchor) = self.studio_project_anchor(rect, joint) {
                let start = egui::pos2(panel.center().x, panel.top());
                p.line_segment([start, anchor], egui::Stroke::new(1.1, SKY.linear_multiply(0.8)));
                p.circle_filled(anchor, 3.0, SKY);
            }
            ui.scope_builder(egui::UiBuilder::new()
                .id_salt(("studio_stage_saber", hand)).max_rect(panel), |ui| {
                egui::Frame::new().fill(egui::Color32::from_rgba_premultiplied(16, 24, 34, 245))
                    .corner_radius(egui::CornerRadius::same(5))
                    .stroke(egui::Stroke::new(1.0, EDGE))
                    .inner_margin(egui::Margin::same(7))
                    .show(ui, |ui| self.studio_saber_editor(ui, hand));
            });
        }
        if self.profile_selected_section==PROFILE_MODEL && self.profile_studio_cosmetic_group != 0 {
            let group=self.profile_studio_cosmetic_group;
            let movement = ui.input(|input| {
                if input.key_pressed(egui::Key::ArrowDown) { 1isize }
                else if input.key_pressed(egui::Key::ArrowUp) { -1isize }
                else { 0 }
            });
            if let Some(next) = studio_cosmetic_arrow(group, self.network.cosmetics, movement) {
                self.focus_profile_studio_camera(match group {
                    1 => "cosmetic:head", 2 => "cosmetic:face", _ => "cosmetic:back",
                });
                if let Err(error) = self.studio_set_cosmetic_group(group, next) {
                    self.console_status = error;
                }
                self.profile_preview_key = None;
            }
            let title=match group {1=>"Headwear",2=>"Face",_=>"Back & capes"};
            // Keep popover within the viewport, not over the main navigation.
            let popup=egui::pos2((rect.left()+rect.width()*0.27).min(rect.right()-285.0),
                (rect.top()+rect.height()*0.2).min(rect.bottom()-340.0).max(rect.top()+10.0));
            let mut open=true;
            egui::Window::new(title).id(egui::Id::new("studio_cosmetics_popup"))
                .fixed_pos(popup).default_width(244.0).max_width(285.0)
                .collapsible(false).resizable(false).open(&mut open)
                .show(ui.ctx(),|ui| {
                    egui::ScrollArea::vertical().max_height(300.0).show(ui,|ui| {
                        let choices: Vec<_>=std::iter::once(("None",None))
                            .chain(crate::japro_cg::cosmetics_in_group(group)
                                .map(|c|(c.label,Some(c.bit)))).collect();
                        for (title,bit) in choices {
                            let chosen=match bit {
                                Some(bit)=>self.network.cosmetics & bit !=0,
                                None=>!crate::japro_cg::cosmetics_in_group(group)
                                    .any(|item| self.network.cosmetics & item.bit !=0),
                            };
                            if ui.selectable_label(chosen,title).clicked() {
                                self.focus_profile_studio_camera(match group {
                                    1 => "cosmetic:head", 2 => "cosmetic:face", _ => "cosmetic:back",
                                });
                                if let Err(error)=self.studio_set_cosmetic_group(group,bit) {
                                    self.console_status=error;
                                }
                                self.profile_preview_key=None;
                                self.profile_studio_cosmetic_group=0;
                            }
                        }
                    });
                });
            if !open {self.profile_studio_cosmetic_group=0;}
        }
    }

    /// Emote sounds reuse the active CGame SoundPresenter and its Steam Audio
    /// HRTF/positional settings. Only the local player may own this channel.
    pub(in crate::app::egui_menu) fn studio_emote_audio(&mut self, _sound: Option<&str>) {
        if let Some(session) = self.game_session.as_mut() {
            let entity = session.presented_player_state.as_ref()
                .and_then(|state| state.field_i32("clientNum"))
                .and_then(|id| u16::try_from(id).ok());
            let source = entity.zip(session.player_position.map(|pos| pos.to_array()));
            if let Some(audio) = session.sound_presenter.as_mut() {
                audio.stop_emote_sound(None, None);
                if let Some(entity) = entity { audio.stop_emote_sound(Some(entity), None); }
                // Audio starts from SoundPresenter::frame according to the notify clock.
                let _ = source;
            }
        }
    }


}

impl App {
    /// The profile's existing jaPRO picker also selects the identical global
    /// MD3 basename on stock servers. This preserves the one Profile page and
    /// the `cp_cosmetics` behavior on jaPRO while broadcasting TaystJK c1/c2
    /// on servers that don't understand c5.
    fn studio_set_cosmetic_group(&mut self, group: u8, bit: Option<u32>) -> Result<(), String> {
        let new = crate::japro_cg::toggle_cosmetic(self.network.cosmetics, group, bit);
        self.profile_set_cvar("cp_cosmetics", &(new as i32).to_string())?;
        if self.active_server_mod() != crate::net::mod_support::ServerMod::Japro {
            let slot = if group == 3 { crate::global_cosmetics::Slot::Cape }
                else { crate::global_cosmetics::Slot::Hat };
            let selected = bit.filter(|value| new & value != 0)
                .and_then(|value| crate::japro_cg::COSMETICS.iter().find(|item| item.bit == value))
                .and_then(|item| std::path::Path::new(item.model).file_stem())
                .and_then(|stem| stem.to_str()).unwrap_or("");
            let (cvar, color, current) = match slot {
                crate::global_cosmetics::Slot::Hat => ("color1", self.network.color1, self.network.global_hat.as_str()),
                crate::global_cosmetics::Slot::Cape => ("color2", self.network.color2, self.network.global_cape.as_str()),
            };
            // Removing another jaPRO group must not accidentally clear a
            // different active global cosmetic on the shared hat slot.
            let should_clear = bit.is_none() && crate::japro_cg::cosmetics_in_group(group)
                .any(|item| std::path::Path::new(item.model).file_stem()
                    .and_then(|stem| stem.to_str()).is_some_and(|stem| stem.eq_ignore_ascii_case(current)));
            if !selected.is_empty() || should_clear {
                self.profile_set_cvar(cvar, &crate::global_cosmetics::encode(color, selected))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod profile_selector_tests {
    use super::*;

    #[test]
    fn cosmetic_arrows_follow_popup_order_and_wrap() {
        for group in [1, 2, 3] {
            let bits: Vec<u32> = crate::japro_cg::cosmetics_in_group(group)
                .map(|cosmetic| cosmetic.bit).collect();
            assert!(!bits.is_empty());
            assert_eq!(studio_cosmetic_arrow(group, 0, 1), Some(Some(bits[0])));
            assert_eq!(studio_cosmetic_arrow(group, 0, -1), Some(Some(*bits.last().unwrap())));
            assert_eq!(studio_cosmetic_arrow(group, bits[0], -1), Some(None));
            assert_eq!(studio_cosmetic_arrow(group, *bits.last().unwrap(), 1), Some(None));
        }
    }

    #[test]
    fn saber_colored_label_hides_caret_tokens_but_preserves_spans() {
        let job = jka_colored_text("^0(^8HALLOWEEN^0) ^7Knife", 12.0, theme::TEXT);
        assert_eq!(job.text, "(HALLOWEEN) Knife");
        assert!(job.sections.len() >= 3);
    }
}
