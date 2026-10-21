//! Virtualized emote grid and personal category management.
use super::{App, theme};
use crate::app::emote_wheel::Category;

impl App {
    pub(in crate::app::egui_menu) fn studio_background_picker(&mut self, ui: &mut egui::Ui) {
        if self.profile_background_maps.is_none() {
            self.profile_background_maps = Some(Vec::new());
            self.ui_catalog.request(
                &self.base,
                self.game.as_deref(),
                crate::app::ui_catalog::CatalogRequest::ProfileBackgroundMaps,
            );
        }
        let maps = self.profile_background_maps.clone().unwrap_or_default();
        if maps.is_empty() {
            ui.small("No installed map images found.");
            return;
        }
        let current = self
            .emote_wheel
            .profile_background
            .clone()
            .filter(|n| maps.contains(n))
            .or_else(|| maps.iter().find(|n| n.as_str() == "mp/ffa3").cloned())
            .unwrap_or_else(|| maps[0].clone());
        let mut selected = current.clone();
        ui.horizontal(|ui| {
            ui.small("Backdrop");
            egui::ComboBox::from_id_salt("profile_map_background")
                .truncate()
                .selected_text(&selected)
                .show_ui(ui, |ui| {
                    for name in &maps {
                        ui.selectable_value(&mut selected, name.clone(), name);
                    }
                });
        });
        if self.emote_wheel.profile_background.as_ref() != Some(&selected) {
            self.emote_wheel.profile_background = Some(selected.clone());
            self.save_emote_favorites();
        }
        if self.profile_background_requested.as_ref() != Some(&selected) {
            self.profile_background_requested = Some(selected.clone());
            self.profile_background_available = false;
            self.render_command(crate::renderer::RenderCommand::SetAssetPreviewBackground(
                None,
            ));
            self.ui_catalog.request(
                &self.base,
                self.game.as_deref(),
                crate::app::ui_catalog::CatalogRequest::ProfileBackground { map_name: selected },
            );
        }
    }
    fn edit_emote_categories(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Edit wheel categories").id_salt("emote_categories").show(ui,|ui|{
            let mut changed=false;let mut remove=None;
            let can_remove=self.emote_wheel.categories.len()>1;
            egui::ScrollArea::vertical().id_salt("emote_category_names").max_height(180.0).show(ui,|ui|{
            for category in &mut self.emote_wheel.categories{
                ui.push_id(&category.id,|ui|ui.horizontal(|ui|{
                    let response=ui.add(egui::TextEdit::singleline(&mut category.name).char_limit(64).desired_width(ui.available_width()-34.0));
                    if response.lost_focus(){
                        if category.name.trim().is_empty(){category.name="Category".into();}
                        changed=true;
                    }
                    if ui.add_enabled(can_remove,egui::Button::new("×")).on_hover_text("Remove category; keep its favorites").clicked(){remove=Some(category.id.clone());}
                }));
            }
            });
            if let Some(id)=remove{
                self.emote_wheel.categories.retain(|c|c.id!=id);
                self.emote_wheel.assignments.retain(|_,c|*c!=id);changed=true;
            }
            if ui.add_enabled(self.emote_wheel.categories.len()<32,egui::Button::new("+ Add category")).clicked(){
                let mut n=1;
                while self.emote_wheel.categories.iter().any(|c|c.id==format!("category-{n}")){n+=1;}
                self.emote_wheel.categories.push(Category{id:format!("category-{n}"),name:format!("Category {n}")});changed=true;
            }
            if changed{self.save_emote_favorites();}
            ui.small("Hold the wheel key and click a category. Click an emote to play; right-click to go back. Releasing the key cancels.");
        });
    }

    pub(in crate::app::egui_menu) fn studio_emotes(&mut self, ui: &mut egui::Ui, available_h: f32) {
        if self.game_session.is_none() && self.menu_emote_sound.is_none() {
            match jka_assets::pk3::AssetSearchPath::open_game(&self.base, self.game.as_deref()) {
                Ok(assets) => {
                    self.menu_emote_sound =
                        Some(crate::cgame::sound_presenter::SoundPresenter::new(
                            assets,
                            self.audio.steam_audio,
                            self.audio.steam_audio_binaural,
                            self.audio.steam_audio_environmental,
                        ))
                }
                Err(error) => self.console_status = format!("Emote audio: {error}"),
            }
        }
        ui.horizontal(|ui| {
            let mut cubic = self.video.emote_cubic;
            if ui.checkbox(&mut cubic, "Smooth emotes").changed() {
                let _ = self.profile_set_cvar("cg_emoteCubic", if cubic { "1" } else { "0" });
            }
            let mut effects = self.video.emote_effects;
            if ui.checkbox(&mut effects, "Effects").on_hover_text("Particles, smoke, sparkles and lights compiled from the emote's Niagara systems.").changed() {
                let _ = self.profile_set_cvar("cg_emoteEffects", if effects { "1" } else { "0" });
            }
            if ui.button("Stop").clicked() {
                crate::emotes::stop();
                self.studio_emote_audio(None);
                self.profile_preview_key = None;
            }
            if ui.button("Refresh").clicked() {
                crate::emotes::stop();
                crate::emotes::refresh_catalog();
            }
        });
        self.edit_emote_categories(ui);
        self.studio_background_picker(ui);
        let Ok(catalog) = crate::emotes::catalog(&self.base) else {
            ui.label("Loading emotes…");
            return;
        };
        let search_id = egui::Id::new("studio_emote_search");
        let filter_id = egui::Id::new("studio_emote_category_filter");
        let favorites_id = egui::Id::new("studio_emote_favorites_filter");
        let (mut search, mut filter, mut favorites_only) = ui.ctx().data_mut(|d| {
            (
                d.get_temp::<String>(search_id).unwrap_or_default(),
                d.get_temp::<String>(filter_id).unwrap_or_default(),
                d.get_temp::<bool>(favorites_id).unwrap_or(false),
            )
        });
        if !filter.is_empty() && !self.emote_wheel.categories.iter().any(|c| c.id == filter) {
            filter.clear();
        }
        ui.add(
            egui::TextEdit::singleline(&mut search)
                .hint_text("Search emotes")
                .desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            let title = self
                .emote_wheel
                .categories
                .iter()
                .find(|c| c.id == filter)
                .map(|c| c.name.as_str())
                .unwrap_or("All categories");
            egui::ComboBox::from_id_salt("emote_category_filter")
                .truncate()
                .selected_text(title)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut filter, String::new(), "All categories");
                    for c in &self.emote_wheel.categories {
                        ui.selectable_value(&mut filter, c.id.clone(), &c.name);
                    }
                });
            ui.checkbox(&mut favorites_only, "★ Favorites");
        });
        ui.ctx().data_mut(|d| {
            d.insert_temp(search_id, search.clone());
            d.insert_temp(filter_id, filter.clone());
            d.insert_temp(favorites_id, favorites_only);
        });
        let query = search.trim().to_lowercase();
        let is_favorite = |e: &crate::emotes::Emote| {
            self.emote_wheel.favorites.contains(&e.id)
                || e.catalog_aliases
                    .iter()
                    .any(|id| self.emote_wheel.favorites.contains(id))
                || e.variants
                    .iter()
                    .any(|v| self.emote_wheel.favorites.contains(&v.id))
        };
        let rows: Vec<_> = catalog
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                (query.is_empty()
                    || crate::emotes::display_title(e)
                        .to_lowercase()
                        .contains(&query)
                    || e.id.to_lowercase().contains(&query))
                    && (filter.is_empty() || self.emote_wheel.category_for(e) == filter)
                    && (!favorites_only || is_favorite(e))
            })
            .collect();
        ui.small(format!(
            "{} emotes · {} starred for the /emote wheel (LIBRARY) · hold {}",
            rows.len(),
            self.emote_wheel.favorites.len(),
            self.bindings.display_for_command("emote")
        ));
        let active = crate::emotes::active_preview().map(|a| a.emote.id);
        let selected = catalog.iter().find(|e| {
            active.as_deref() == Some(&e.id)
                || e.variants.iter().any(|v| active.as_deref() == Some(&v.id))
        });
        if let Some(emote) = selected {
            let mut variant = self
                .emote_wheel
                .variants
                .get(&emote.id)
                .copied()
                .filter(|i| *i < emote.variants.len())
                .unwrap_or_else(|| crate::emotes::default_variant_index(emote));
            let previous = variant;
            if emote.variants.len() > 1 {
                egui::ComboBox::from_id_salt("emote_selected_variant")
                    .selected_text(format!("Variation {}", variant + 1))
                    .show_ui(ui, |ui| {
                        for (index, v) in emote.variants.iter().enumerate() {
                            ui.selectable_value(
                                &mut variant,
                                index,
                                format!("{} · {}", index + 1, crate::emotes::audio_status_label(v)),
                            )
                            .on_hover_text(format!("{}\n{}", v.variant_name, v.audio_detail));
                        }
                    });
                if previous != variant {
                    self.emote_wheel.variants.insert(emote.id.clone(), variant);
                    self.save_emote_favorites();
                    crate::emotes::play(emote.variants[variant].clone());
                    self.profile_preview_key = None;
                }
            }
            let v = emote.variants.get(variant).unwrap_or(emote);
            if !v.presentation_detail.is_empty() {
                ui.label(
                    egui::RichText::new("Some props/effects need conversion")
                        .size(10.0)
                        .color(theme::WARNING),
                )
                .on_hover_text(&v.presentation_detail);
            }
        }
        let width = ui.available_width();
        let columns = ((width + 8.0) / 150.0).floor().max(1.0) as usize;
        let tile_width = (width - (columns - 1) as f32 * 8.0) / columns as f32;
        let height = (ui.available_height() - 8.0).min(available_h).max(80.0);
        egui::ScrollArea::vertical()
            .id_salt("studio_emote_grid")
            .auto_shrink([false, false])
            .max_height(height)
            .show_rows(ui, 112.0, rows.len().div_ceil(columns), |ui, range| {
                for row in range {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        for (index, emote) in rows.iter().skip(row * columns).take(columns) {
                            ui.push_id(&emote.id, |ui| {
                                let selected = active.as_deref() == Some(&emote.id)
                                    || emote
                                        .variants
                                        .iter()
                                        .any(|v| active.as_deref() == Some(&v.id));
                                egui::Frame::new()
                                    .fill(if selected {
                                        egui::Color32::from_rgb(26, 49, 70)
                                    } else {
                                        egui::Color32::from_rgb(21, 29, 40)
                                    })
                                    .stroke(egui::Stroke::new(
                                        1.0,
                                        if selected {
                                            theme::ACCENT
                                        } else {
                                            egui::Color32::from_rgb(45, 59, 75)
                                        },
                                    ))
                                    .corner_radius(5)
                                    .inner_margin(6)
                                    .show(ui, |ui| {
                                        ui.set_width((tile_width - 12.0).max(40.0));
                                        ui.with_layout(egui::Layout::top_down(egui::Align::Min),|ui|{
                                        let variant = self
                                            .emote_wheel
                                            .variants
                                            .get(&emote.id)
                                            .copied()
                                            .unwrap_or_else(|| {
                                                crate::emotes::default_variant_index(emote)
                                            });
                                        let v = emote.variants.get(variant).unwrap_or(emote);
                                        if ui
                                            .add_sized(
                                                [ui.available_width(), 44.0],
                                                egui::Button::new(
                                                    egui::RichText::new(format!(
                                                        "{}  {}",
                                                        index + 1,
                                                        crate::emotes::display_title(emote)
                                                    ))
                                                    .size(11.0),
                                                )
                                                .wrap()
                                                .frame(false),
                                            )
                                            .clicked()
                                        {
                                            crate::emotes::play(v.clone());
                                            self.profile_preview_key = None;
                                            self.studio_emote_audio(None);
                                        }
                                        let missing = v.audio_status != "available";
                                        ui.label(
                                            egui::RichText::new(if missing {
                                                match v.audio_status.as_str(){
                                                    "missing_source"=>"Missing audio",
                                                    "unmapped"=>"Audio unmapped",
                                                    "silent_mix"=>"Silent audio",
                                                    "partial"=>"Partial audio",
                                                    "foley_only"=>"Effects only",
                                                    _=>"No audio",
                                                }
                                            } else {
                                                " "
                                            })
                                            .size(9.0)
                                            .color(theme::WARNING),
                                        )
                                        .on_hover_text(&v.audio_detail);
                                        ui.horizontal(|ui| {
                                            let mut favorite =
                                                self.emote_wheel.favorites.contains(&emote.id)
                                                    || emote.catalog_aliases.iter().any(|id| {
                                                        self.emote_wheel.favorites.contains(id)
                                                    })
                                                    || emote.variants.iter().any(|v| {
                                                        self.emote_wheel.favorites.contains(&v.id)
                                                    });
                                            if ui.checkbox(&mut favorite, "★").changed() {
                                                self.emote_wheel.favorites.retain(|id| {
                                                    id != &emote.id
                                                        && !emote.catalog_aliases.contains(id)
                                                        && !emote
                                                            .variants
                                                            .iter()
                                                            .any(|v| &v.id == id)
                                                });
                                                if favorite {
                                                    self.emote_wheel
                                                        .favorites
                                                        .push(emote.id.clone());
                                                }
                                                self.save_emote_favorites();
                                            }
                                            let mut category = self.emote_wheel.category_for(emote);
                                            let previous = category.clone();
                                            let name = self
                                                .emote_wheel
                                                .categories
                                                .iter()
                                                .find(|c| c.id == category)
                                                .map(|c| c.name.clone())
                                                .unwrap_or_default();
                                            egui::ComboBox::from_id_salt("category")
                                        .truncate()
                                                .width((ui.available_width() - 12.0).max(40.0))
                                                .selected_text(name)
                                                .show_ui(ui, |ui| {
                                                    for c in &self.emote_wheel.categories {
                                                        ui.selectable_value(
                                                            &mut category,
                                                            c.id.clone(),
                                                            &c.name,
                                                        );
                                                    }
                                                });
                                            if previous != category {
                                                self.emote_wheel
                                                    .assignments
                                                    .insert(emote.id.clone(), category);
                                                self.save_emote_favorites();
                                            }
                                        });
                                        });
                                    });
                            });
                        }
                    });
                }
            });
    }
}
