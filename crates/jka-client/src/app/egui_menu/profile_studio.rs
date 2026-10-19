//! The single Profile page. Uses the real player, saber, Force and PK3 systems.
//! No parallel model loader, invented skeleton or duplicate userinfo cvars.
use crate::app::egui_menu::{
    theme, App, BTreeMap, ProfileModelEntry,
    PROFILE_COSMETICS, PROFILE_EMOTES, PROFILE_FORCE, PROFILE_MODEL,
};

const INK: egui::Color32 = egui::Color32::from_rgb(10, 15, 23);
const PLATE: egui::Color32 = egui::Color32::from_rgb(17, 24, 35);
const PLATE_LIFT: egui::Color32 = egui::Color32::from_rgb(25, 34, 48);
const EDGE: egui::Color32 = egui::Color32::from_rgb(48, 66, 86);
const SKY: egui::Color32 = theme::ACCENT;
const SKY_SOFT: egui::Color32 = egui::Color32::from_rgb(125, 199, 255);
const MUTED: egui::Color32 = egui::Color32::from_rgb(142, 162, 183);
const STUDIO_NAV: [(usize, &str); 4] = [
    (PROFILE_MODEL, "LOOK"),
    (PROFILE_FORCE, "FORCE"),
    (PROFILE_COSMETICS, "COSMETICS"),
    (PROFILE_EMOTES, "EMOTES"),
];

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
        if !matches!(self.profile_selected_section, PROFILE_MODEL | PROFILE_FORCE | PROFILE_COSMETICS | PROFILE_EMOTES) {
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
                // The tabs, appearance options and search never scroll. Only the
                // model library owns a ScrollArea, within its remaining height.
                let nav_w = (ui.available_width() - 18.0) / 4.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    for (section, title) in STUDIO_NAV {
                        if studio_button(ui, title, self.profile_selected_section == section, false, nav_w) {
                            self.profile_selected_section = section;
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
                    PROFILE_COSMETICS => self.studio_visibility(ui),
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
        self.studio_body_shape_controls(ui, &current);
        ui.add_space(11.0);
        cap(ui, "SHOW SKINS", "");
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            for (index, title) in ["Original", "Red", "Blue", "RGB"].into_iter().enumerate() {
                ui.checkbox(&mut self.profile_studio_skin_visible[index], title);
            }
        });
        ui.add_space(11.0);
        ui.add_space(5.0);
        cap(ui, "MODEL LIBRARY", "");
        ui.add_sized([ui.available_width(), 30.0],
            egui::TextEdit::singleline(&mut self.profile_model_search).hint_text("Search models or skins…"));
        ui.add_space(8.0);
        let needle = self.profile_model_search.trim().to_ascii_lowercase();
        let mut models = BTreeMap::<String, Vec<ProfileModelEntry>>::new();
        // Filter *skins* before grouping models. A character with Original,
        // Red and RGB skins remains in the library when one category is hidden,
        // while its hover chooser shows only visible variants. TaystJK does
        // RGB discovery by skin filename, so there is no runtime shader scan.
        for entry in &self.profile_models {
            if !self.profile_studio_skin_visible[Self::studio_skin_class(entry)] {
                continue;
            }
            models.entry(entry.model_name.clone()).or_default().push(entry.clone());
        }
        // Searching for a model leaves its complete set of *visible* variants
        // available in the hover chooser instead of silently trimming them.
        models.retain(|name, variants| {
            needle.is_empty() || name.to_ascii_lowercase().contains(&needle)
                || variants.iter().any(|entry| {
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
                        for variants in model_rows.iter().skip(row * columns).take(columns) {
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
                                if multipart { None } else { Some(variants) }) {
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
            self.studio_stage_model(&value);
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
                            if (i + 1) % cols == 0 { ui.end_row(); }
                        }
                    });
            });
            if let Some(value) = pick { self.studio_stage_model(&value); }
            response.clicked()
        } else {
            response.on_hover_text(format!("{} / {}", entry.model_name, entry.skin_name)).clicked()
        }
    }

    /// JKA model userinfo: `jedi_hm/head_a1|torso_d1|lower_a1`.
    /// The skin loader inserts the leading pipe for `RE_SplitSkins`.
    /// Only advertise combinations represented by genuine installed skin icons.
    fn studio_multipart_skin(&mut self, ui: &mut egui::Ui, model: &str, active: &str) {
        let mut parts: [Vec<ProfileModelEntry>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for entry in self.profile_models.iter().filter(|e| e.model_name.eq_ignore_ascii_case(model)) {
            for (i, prefix) in ["head_", "torso_", "lower_"].into_iter().enumerate() {
                if entry.skin_name.starts_with(prefix) { parts[i].push(entry.clone()); }
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
                if dropdown.clicked_by(egui::PointerButton::Middle) {
                    selected[i] = parts[i][0].skin_name.clone();
                    changed = true;
                }
            });
        }
        if changed {
            self.studio_stage_model(&format!("{model}/{}|{}|{}", selected[0],selected[1],selected[2]));
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

    fn studio_stage_model(&mut self, value: &str) {
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
             "Peak glute mass in JKA units; negative reduces. Middle-click to reset.")
        } else {
            ("BREAST SIZE", "r_jiggleBreastSize", 3.0,
             self.video.jiggle_breast_size,
             "Peak chest mass in JKA units; negative reduces. Middle-click to reset.")
        };
        label(ui, title, 10.0, SKY_SOFT);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            let mut size = current;
            let response = ui.add(egui::Slider::new(&mut size, -2.0..=max)
                .step_by(0.10).show_value(false))
                .on_hover_text(help);
            if response.clicked_by(egui::PointerButton::Middle) {
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

    fn studio_body_shape_controls(&mut self, ui: &mut egui::Ui, current: &str) {
        let info = crate::cgame::ClientInfo::solo_model(current);
        let available = self.asset_preview_player_presenter.as_ref()
            .is_some_and(|presenter| presenter.profile_body_shape_available(&info));
        cap(ui, "BODY", "");
        ui.add_enabled_ui(available, |ui| {
            for (title, cvar, current, default) in [
                ("Muscle", "r_bodyMuscle", self.video.body_muscle, 0.5),
                ("Weight", "r_bodyWeight", self.video.body_weight, 0.5),
                ("Proportions", "r_bodyProportions", self.video.body_proportions, 0.5),
                ("Age", "r_bodyAge", self.video.body_age, 0.5),
            ] {
                let id = ui.id().with(cvar);
                let mut value = ui.ctx().data_mut(|data| data.get_temp::<f32>(id)).unwrap_or(current);
                let response = ui.add(egui::Slider::new(&mut value, 0.0..=1.0)
                    .step_by(0.01).text(title).show_value(false))
                    .on_hover_text("Release to update the character. Middle-click to reset.");
                if response.clicked_by(egui::PointerButton::Middle) { value = default; }
                if response.dragged() {
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
        });
        if !available { label(ui, "Body shaping is unavailable for this character.", 10.0, MUTED); }
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
            let combo = egui::ComboBox::from_id_salt(("studio_saber_hilt", hand))
                .selected_text(if hand == 1 && !dual { "None" } else { &entry_label })
                .width(141.0).show_ui(ui, |ui| {
                    if hand == 1 {
                        ui.selectable_value(&mut selected, "none".to_owned(), "None");
                    }
                    egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                        for saber in &self.profile_sabers {
                            if if hand == 1 { Self::profile_saber_is_single(saber) }
                                else if staff { Self::profile_saber_is_staff(saber) }
                                else { Self::profile_saber_is_single(saber) } {
                                ui.selectable_value(&mut selected, saber.name.clone(),
                                    Self::profile_saber_label(saber));
                            }
                        }
                    });
                }).response;
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
            egui::vec2(width, height), egui::Sense::drag());
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
                self.profile_studio_spin_velocity = (dx * 0.42 / dt).clamp(-560.0, 560.0);
                self.profile_preview_key = None;
            }
        } else if self.profile_studio_spin_velocity.abs() > 0.08 {
            let velocity = self.profile_studio_spin_velocity;
            self.profile_preview_yaw = (self.profile_preview_yaw + velocity * dt).rem_euclid(360.0);
            self.profile_studio_spin_velocity *= (-3.4 * dt).exp();
            self.profile_preview_key = None;
        }
        if response.hovered() {
            let scroll = ui.ctx().input(|input| input.smooth_scroll_delta.y);
            if scroll.abs() > 0.01 {
                self.profile_preview_zoom = (self.profile_preview_zoom * (-scroll * 0.0015).exp())
                    .clamp(0.45, 2.5);
                self.profile_preview_key = None;
            }
        }
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
            return;
        }

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
            egui::pos2(frame.right() - 267.0, frame.top() + 14.0), egui::vec2(251.0, 58.0));
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
        });

        // Callout positions remain legible. Their connector endpoints come
        // exclusively from animated Ghoul2 bolt matrices, projected through
        // the same preview camera used by WGPU (never hardcoded stage percents).
        if self.profile_selected_section==PROFILE_COSMETICS {
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
                if let Err(error) = self.studio_set_cosmetic_group(group, None) {
                    self.console_status = error;
                }
                self.profile_preview_key = None;
            } else if button.clicked() {
                self.profile_studio_cosmetic_group =
                    if self.profile_studio_cosmetic_group == group { 0 } else { group };
            }
        }

        // Anatomy sliders use the same animated-profile anchor projection as
        // saber/cosmetic callouts, with endpoints fitted to the actual model's
        // soft-tissue regions (including Source 1 imports) rather than guessing
        // fixed body heights. No marker is shown when a model has no region.
        for (glute, joint, relative_y) in [(false, 5usize, 0.35f32), (true, 6, 0.57)] {
            // Missing region means this model has nothing for the slider to
            // deform. Keep both the slider and its line completely absent.
            let Some(anchor) = self.studio_project_anchor(rect, joint) else {
                continue;
            };
            let panel = egui::Rect::from_min_size(
                egui::pos2(rect.right() - 202.0,
                    rect.top() + rect.height() * relative_y),
                egui::vec2(188.0, 68.0));
            p.line_segment([panel.left_center(), anchor],
                egui::Stroke::new(1.1, SKY.linear_multiply(0.8)));
            p.circle_filled(anchor, 3.0, SKY);
            ui.scope_builder(egui::UiBuilder::new()
                .id_salt(("studio_body_size", glute)).max_rect(panel), |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_premultiplied(16, 24, 34, 245))
                    .corner_radius(egui::CornerRadius::same(5))
                    .stroke(egui::Stroke::new(1.0, EDGE))
                    .inner_margin(egui::Margin::same(7))
                    .show(ui, |ui| {
                        ui.set_width(174.0);
                        self.studio_body_size_slider(ui, glute);
                    });
            });
        }

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
        if self.profile_selected_section==PROFILE_COSMETICS && self.profile_studio_cosmetic_group != 0 {
            let group=self.profile_studio_cosmetic_group;
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
