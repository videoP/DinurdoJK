//! Profile.
use crate::app::egui_menu::{
    profile_name_layout, theme, ui_catalog, App, BTreeMap, Instant, ProfileForceConfig,
    ProfileModelEntry, ProfileSaberEntry, FORCE_DARK_ORDER, FORCE_LIGHT_ORDER, FORCE_MASTERY_NAMES,
    FORCE_NAMES, FORCE_NEUTRAL_ORDER, PROFILE_COSMETICS, PROFILE_FORCE, PROFILE_IDENTITY,
    PROFILE_MODEL, PROFILE_SABER, PROFILE_SECTIONS,
};

impl App {
    // -------------------------------------------------------------- pages --

    pub(in crate::app::egui_menu) fn profile_readout(ui: &mut egui::Ui, label: &str, value: &str) {
        ui.horizontal(|ui| {
            ui.set_min_height(22.0);
            theme::label(ui, theme::plain(label, 11.5, theme::TEXT_FAINT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme::label(ui, theme::plain(value, 11.5, theme::TEXT));
            });
        });
    }

    pub(in crate::app::egui_menu) fn profile_model_icon_texture(
        &mut self,
        entry: &ProfileModelEntry,
    ) -> Option<egui::TextureHandle> {
        let key = entry.value.to_ascii_lowercase();
        if let Some(texture) = self.profile_model_icon_textures.get(&key) {
            return Some(texture.clone());
        }
        if self.ui_catalog.icons_missing.contains(&key)
            || self.ui_catalog.icons_inflight.contains(&key)
        {
            return None;
        }
        self.ui_catalog.icons_inflight.insert(key.clone());
        self.ui_catalog.request(
            &self.base,
            self.game.as_deref(),
            ui_catalog::CatalogRequest::ProfileIcon {
                key,
                model: entry.model_name.clone(),
                skin: entry.skin_name.clone(),
            },
        );
        None
    }

    pub(in crate::app::egui_menu) fn profile_model_matches_team_filter(
        entry: &ProfileModelEntry,
        filter: u8,
    ) -> bool {
        if filter == 0 {
            return true;
        }
        let skin = entry.skin_name.to_ascii_lowercase();
        let wanted = if filter == 1 { "red" } else { "blue" };
        skin == wanted
            || skin.starts_with(&format!("{wanted}_"))
            || skin.starts_with(&format!("{wanted}-"))
            || skin.starts_with(wanted)
            || skin.ends_with(&format!("_{wanted}"))
            || skin.ends_with(&format!("-{wanted}"))
    }

    pub(in crate::app::egui_menu) fn profile_saber_label(entry: &ProfileSaberEntry) -> &str {
        // Stock saber files may use @MENUS_* localization tokens. Until the
        // string-package browser is wired into egui, the block identifier is a
        // cleaner fallback than exposing the raw token.
        if entry.display_name.is_empty() || entry.display_name.starts_with('@') {
            &entry.name
        } else {
            &entry.display_name
        }
    }

    pub(in crate::app::egui_menu) fn profile_saber_is_single(entry: &ProfileSaberEntry) -> bool {
        entry.saber_type.eq_ignore_ascii_case("SABER_SINGLE")
    }

    pub(in crate::app::egui_menu) fn profile_saber_is_staff(entry: &ProfileSaberEntry) -> bool {
        entry.saber_type.eq_ignore_ascii_case("SABER_STAFF")
    }

    pub(in crate::app::egui_menu) fn profile_server_force_limits(
        &self,
    ) -> (u8, u32, i32, bool, Option<u8>) {
        let info = if let Some(net) = self.net.as_ref() {
            crate::net::mod_support::server_info(&net.session().decoder().configstrings)
        } else if let Some(server) = self.local_server.as_ref() {
            server
                .configstrings()
                .get(&crate::cgame::CS_SERVERINFO)
                .map(Vec::as_slice)
                .unwrap_or_default()
        } else {
            &[]
        };
        let value = |key: &[u8], fallback: i32| {
            jka_protocol::commands::info_value(info, key)
                .map(jka_protocol::commands::atoi)
                .unwrap_or(fallback)
        };
        let max_rank = value(b"g_maxForceRank", 7).clamp(0, 7) as u8;
        let disabled = value(b"g_forcePowerDisable", 0) as u32;
        let gametype = value(b"g_gametype", 0);

        // TaystJK UI_HasSetSaberOnly, ported verbatim in behavior. Jedi Master
        // never grants the free saber; duel modes use g_duelWeaponDisable and
        // other modes use g_weaponDisable. WP_NONE=0, WP_SABER=3, WP_NUM=19.
        let free_saber = if gametype == 2 {
            false
        } else {
            let weapon_disable = if matches!(gametype, 3 | 4) {
                value(b"g_duelWeaponDisable", 0)
            } else {
                value(b"g_weaponDisable", 0)
            } as u32;
            (0..19).all(|weapon| {
                weapon == 0 || weapon == 3 || (weapon_disable & (1u32 << weapon)) != 0
            })
        };

        // UI_DrawForceSide: force-based teams pin Red to Dark and Blue to Light.
        let forced_side = if value(b"g_forceBasedTeams", 0) != 0 {
            self.game_session.as_ref().and_then(|session| {
                let client_num = session
                    .current_snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.player_state.field_i32("clientNum"))
                    .and_then(|client| usize::try_from(client).ok())?;
                let team = session
                    .client_game
                    .client_info(client_num, &session.siege_classes)?
                    .team;
                match team {
                    1 => Some(2), // TEAM_RED -> FORCE_DARKSIDE
                    2 => Some(1), // TEAM_BLUE -> FORCE_LIGHTSIDE
                    _ => None,
                }
            })
        } else {
            None
        };
        (max_rank, disabled, gametype, free_saber, forced_side)
    }

    pub(in crate::app::egui_menu) fn egui_profile_page(&mut self, ui: &mut egui::Ui) {
        self.ensure_profile_catalog();
        theme::page_title(
            ui,
            "PROFILE",
            "Protocol-26 player identity, appearance, Force loadout and saber selection.",
        );

        // jaPRO's player menu gates everything on APPLY (setForce -> forcechanged
        // + userinfo). It lives above the section tabs so it is always reachable.
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let pending = self.profile_has_pending_changes();
            let response = if pending {
                theme::primary_button(ui, "APPLY")
            } else {
                theme::ghost_button(ui, "APPLY")
            };
            if response.clicked() {
                self.apply_profile_changes();
            }
            theme::label(
                ui,
                if pending {
                    theme::plain("Changes are local until applied.", 11.0, theme::WARNING)
                } else {
                    theme::plain("All changes applied.", 11.0, theme::TEXT_FAINT)
                },
            );
        });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            for (index, label) in PROFILE_SECTIONS.iter().enumerate() {
                if theme::chip(ui, label, self.profile_selected_section == index).clicked() {
                    self.profile_selected_section = index;
                    self.profile_preview_key = None;
                }
            }
        });
        ui.add_space(10.0);

        let height = ui.available_height().max(280.0);
        ui.horizontal(|ui| {
            let controls_w = (ui.available_width() * 0.46).clamp(390.0, 610.0);
            ui.allocate_ui_with_layout(
                egui::vec2(controls_w, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::Frame::new()
                        .fill(theme::PANEL_FILL)
                        .stroke(egui::Stroke::new(1.0_f32, theme::LINE))
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            ui.set_min_width((controls_w - 24.0).max(1.0));
                            ui.set_min_height((height - 24.0).max(1.0));
                            egui::ScrollArea::vertical()
                                .id_salt("profile_controls")
                                .auto_shrink([false, false])
                                .show(ui, |ui| match self.profile_selected_section {
                                    PROFILE_IDENTITY => self.egui_profile_identity(ui),
                                    PROFILE_MODEL => self.egui_profile_model(ui),
                                    PROFILE_FORCE => self.egui_profile_force(ui),
                                    PROFILE_SABER => self.egui_profile_saber(ui),
                                    PROFILE_COSMETICS => self.egui_profile_cosmetics(ui),
                                    _ => {}
                                });
                        });
                },
            );

            ui.add_space(14.0);

            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    let saber = self.profile_selected_section == PROFILE_SABER;
                    // The preview frame starts on the same line as the controls
                    // frame and shares its height; its heading is painted
                    // inside the frame so the two panes read as a matched pair.
                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), height),
                        egui::Sense::drag(),
                    );
                    ui.painter().rect_stroke(
                        rect,
                        egui::CornerRadius::ZERO,
                        egui::Stroke::new(1.0_f32, theme::LINE),
                        egui::StrokeKind::Inside,
                    );
                    if response.dragged() {
                        let delta = ui.input(|input| input.pointer.delta());
                        if delta.x.abs() > f32::EPSILON {
                            self.profile_preview_yaw =
                                (self.profile_preview_yaw + delta.x * 0.45).rem_euclid(360.0);
                            self.profile_preview_key = None;
                        }
                    }
                    if response.hovered() {
                        let scroll = ui.ctx().input(|input| input.smooth_scroll_delta.y);
                        if scroll.abs() > f32::EPSILON {
                            self.profile_preview_zoom = (self.profile_preview_zoom
                                * (-scroll * 0.0015).exp())
                            .clamp(0.45, 2.5);
                            self.profile_preview_key = None;
                        }
                    }
                    self.set_asset_preview_viewport(rect, ui.ctx().pixels_per_point());
                    let painter = ui.painter().clone();
                    theme::glow_text(
                        &painter,
                        rect.left_top() + egui::vec2(14.0, 14.0),
                        egui::Align2::LEFT_TOP,
                        if saber {
                            "SABER PREVIEW"
                        } else {
                            "PLAYER PREVIEW"
                        },
                        egui::FontId::proportional(12.5),
                        theme::ACCENT,
                    );
                    theme::glow_text(
                        &painter,
                        rect.left_top() + egui::vec2(14.0, 34.0),
                        egui::Align2::LEFT_TOP,
                        "Drag to rotate · wheel to zoom",
                        egui::FontId::proportional(11.5),
                        theme::TEXT_FAINT,
                    );
                    let caption = if saber {
                        &self.network.saber1
                    } else {
                        &self.solo_client_info.model_name
                    };
                    theme::glow_text(
                        &painter,
                        rect.left_bottom() + egui::vec2(14.0, -12.0),
                        egui::Align2::LEFT_BOTTOM,
                        caption,
                        egui::FontId::proportional(11.5),
                        theme::TEXT_DIM,
                    );
                },
            );
        });
        // The Profile idle is intentionally capped to ~30 Hz rather than
        // forcing egui to repaint at the uncapped game/render rate.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
        self.update_profile_preview();
    }

    pub(in crate::app::egui_menu) fn egui_profile_identity(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "IDENTITY", "Userinfo sent to protocol-26 servers");
        theme::label(ui, theme::plain("NAME", 11.5, theme::TEXT_FAINT));
        ui.add_space(4.0);
        let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
            let mut job = profile_name_layout(buffer.as_str(), 14.0, theme::TEXT);
            job.wrap.max_width = wrap_width;
            ui.fonts_mut(|fonts| fonts.layout_job(job))
        };
        let edit = egui::TextEdit::singleline(&mut self.profile_name_input)
            .desired_width(ui.available_width())
            .hint_text("Player name")
            .layouter(&mut layouter);
        let response = ui.add(edit);
        if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
            self.apply_profile_changes();
        }
        ui.add_space(8.0);

        theme::section(
            ui,
            "CURRENT LOADOUT",
            "Uses the same cvars as the stock client",
        );
        Self::profile_readout(ui, "MODEL", &self.solo_client_info.model_cvar());
        Self::profile_readout(ui, "PRIMARY SABER", &self.network.saber1);
        Self::profile_readout(ui, "SECONDARY SABER", &self.network.saber2);
        Self::profile_readout(ui, "FORCE", &self.network.forcepowers);
        ui.add_space(8.0);
        self.egui_profile_skin_tint(ui);
    }

    /// `char_color_red/green/blue`: the tint servers relay as `customRGBA`. Only
    /// the parts of a model whose shader reads the entity colour follow it (for
    /// example the armour plates of jedi_zf), so 255/255/255 leaves every model as authored.
    pub(in crate::app::egui_menu) fn egui_profile_skin_tint(&mut self, ui: &mut egui::Ui) {
        theme::section(
            ui,
            "SKIN TINT",
            "Tints the entity-coloured parts of models that support it. 255 / 255 / 255 is untinted.",
        );
        let mut channels = self.network.char_color.map(f32::from);
        let mut picked = None;
        for (channel, (cvar, label)) in [
            ("char_color_red", "RED"),
            ("char_color_green", "GREEN"),
            ("char_color_blue", "BLUE"),
        ]
        .into_iter()
        .enumerate()
        {
            theme::row(
                ui,
                label,
                "Player tint channel, 0-255.",
                theme::Reset::None,
                |ui| {
                    let readout = format!("{:.0}", channels[channel]);
                    if theme::slider(ui, &mut channels[channel], 0.0..=255.0, &readout) {
                        picked = Some((cvar, channels[channel].round() as u8));
                    }
                },
            );
        }
        let swatch =
            egui::Color32::from_rgb(channels[0] as u8, channels[1] as u8, channels[2] as u8);
        let mut reset = false;
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(46.0, 16.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 3.0, swatch);
            theme::label(
                ui,
                theme::plain(
                    &format!("#{:02X}{:02X}{:02X}", swatch.r(), swatch.g(), swatch.b()),
                    11.5,
                    theme::TEXT_DIM,
                ),
            );
            reset = theme::chip(ui, "RESET", false).clicked();
        });
        // Applied to the preview at once; the userinfo goes out with APPLY.
        if reset {
            for cvar in ["char_color_red", "char_color_green", "char_color_blue"] {
                let _ = self.profile_set_cvar(cvar, "255");
            }
        } else if let Some((cvar, value)) = picked {
            if let Err(error) = self.profile_set_cvar(cvar, &value.to_string()) {
                self.console_status = error;
            }
        }
    }

    pub(in crate::app::egui_menu) fn egui_profile_model(&mut self, ui: &mut egui::Ui) {
        theme::section(
            ui,
            "PLAYER MODEL",
            "One tile per humanoid model · hover a tile to choose its skin variants",
        );

        ui.horizontal(|ui| {
            let filters_width = 154.0;
            ui.add_sized(
                [(ui.available_width() - filters_width).max(120.0), 24.0],
                egui::TextEdit::singleline(&mut self.profile_model_search)
                    .hint_text("Search model or skin…"),
            );
            if theme::chip(ui, "ALL", self.profile_model_team_filter == 0).clicked() {
                self.profile_model_team_filter = 0;
            }
            if theme::chip(ui, "RED", self.profile_model_team_filter == 1).clicked() {
                self.profile_model_team_filter = 1;
            }
            if theme::chip(ui, "BLUE", self.profile_model_team_filter == 2).clicked() {
                self.profile_model_team_filter = 2;
            }
        });

        if let Some(error) = &self.profile_catalog_error {
            ui.add_space(8.0);
            theme::banner(ui, error, theme::WARNING);
        }
        ui.add_space(10.0);

        let needle = self.profile_model_search.trim().to_ascii_lowercase();
        let mut models = BTreeMap::<String, Vec<ProfileModelEntry>>::new();
        for entry in self.profile_models.iter().filter(|entry| {
            Self::profile_model_matches_team_filter(entry, self.profile_model_team_filter)
        }) {
            models
                .entry(entry.model_name.clone())
                .or_default()
                .push(entry.clone());
        }
        models.retain(|model_name, variants| {
            if needle.is_empty() || model_name.to_ascii_lowercase().contains(&needle) {
                true
            } else {
                variants.iter().any(|entry| {
                    entry.skin_name.to_ascii_lowercase().contains(&needle)
                        || entry.value.to_ascii_lowercase().contains(&needle)
                })
            }
        });

        let current = self.solo_client_info.model_cvar();
        let current_model = self.solo_client_info.model_name.clone();
        let columns = ((ui.available_width() + 8.0) / (92.0 + 8.0))
            .floor()
            .max(1.0) as usize;
        // Tiles grow (up to 20%) to absorb the slack, so the grid spans the
        // pane instead of leaving a ragged strip down its right side.
        let tile_w = ((ui.available_width() + 8.0) / columns as f32 - 8.0).clamp(92.0, 110.0);
        let tile = egui::vec2(tile_w, 116.0 * tile_w / 92.0);
        let mut pending_model: Option<String> = None;
        let mut visible_count = 0usize;

        egui::Grid::new("profile-model-grid")
            .num_columns(columns)
            .spacing(egui::vec2(8.0, 10.0))
            .show(ui, |ui| {
                for (model_name, mut variants) in models {
                    variants.sort_by(|a, b| a.skin_name.cmp(&b.skin_name));
                    let representative = variants
                        .iter()
                        .find(|entry| entry.value.eq_ignore_ascii_case(&current))
                        .or_else(|| {
                            variants
                                .iter()
                                .find(|entry| entry.skin_name.eq_ignore_ascii_case("default"))
                        })
                        .unwrap_or(&variants[0])
                        .clone();
                    let selected_model = model_name.eq_ignore_ascii_case(&current_model);
                    let (rect, response) = ui.allocate_exact_size(tile, egui::Sense::click());
                    ui.painter().rect_filled(
                        rect,
                        egui::CornerRadius::same(3),
                        if selected_model {
                            theme::PANEL_FILL
                        } else {
                            egui::Color32::TRANSPARENT
                        },
                    );
                    ui.painter().rect_stroke(
                        rect,
                        egui::CornerRadius::same(3),
                        egui::Stroke::new(
                            if selected_model { 2.0_f32 } else { 1.0_f32 },
                            if selected_model {
                                theme::ACCENT
                            } else {
                                theme::LINE
                            },
                        ),
                        egui::StrokeKind::Inside,
                    );
                    let image_rect = egui::Rect::from_min_max(
                        rect.min + egui::vec2(4.0, 4.0),
                        egui::pos2(rect.max.x - 4.0, rect.max.y - 25.0),
                    );
                    if let Some(texture) = self.profile_model_icon_texture(&representative) {
                        ui.painter().image(
                            texture.id(),
                            image_rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    } else {
                        ui.painter().rect_filled(
                            image_rect,
                            egui::CornerRadius::same(2),
                            theme::PANEL_FILL,
                        );
                        ui.painter().text(
                            image_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "NO ICON",
                            egui::FontId::proportional(9.0),
                            theme::TEXT_FAINT,
                        );
                    }
                    ui.painter().text(
                        egui::pos2(rect.center().x, rect.max.y - 12.0),
                        egui::Align2::CENTER_CENTER,
                        &model_name,
                        egui::FontId::proportional(10.0),
                        if selected_model {
                            theme::TEXT
                        } else {
                            theme::TEXT_DIM
                        },
                    );

                    let response = response.on_hover_ui(|ui| {
                        ui.set_min_width(220.0);
                        theme::label(
                            ui,
                            theme::plain(&model_name.to_ascii_uppercase(), 10.5, theme::TEXT_FAINT),
                        );
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            for entry in &variants {
                                let variant_size = egui::vec2(60.0, 80.0);
                                let (vrect, vresponse) =
                                    ui.allocate_exact_size(variant_size, egui::Sense::click());
                                let selected = entry.value.eq_ignore_ascii_case(&current);
                                ui.painter().rect_stroke(
                                    vrect,
                                    egui::CornerRadius::same(2),
                                    egui::Stroke::new(
                                        if selected { 2.0_f32 } else { 1.0_f32 },
                                        if selected { theme::ACCENT } else { theme::LINE },
                                    ),
                                    egui::StrokeKind::Inside,
                                );
                                let vimage = egui::Rect::from_min_max(
                                    vrect.min + egui::vec2(3.0, 3.0),
                                    egui::pos2(vrect.max.x - 3.0, vrect.max.y - 20.0),
                                );
                                if let Some(texture) = self.profile_model_icon_texture(entry) {
                                    ui.painter().image(
                                        texture.id(),
                                        vimage,
                                        egui::Rect::from_min_max(
                                            egui::pos2(0.0, 0.0),
                                            egui::pos2(1.0, 1.0),
                                        ),
                                        egui::Color32::WHITE,
                                    );
                                }
                                ui.painter().text(
                                    egui::pos2(vrect.center().x, vrect.max.y - 9.0),
                                    egui::Align2::CENTER_CENTER,
                                    &entry.skin_name,
                                    egui::FontId::proportional(8.5),
                                    theme::TEXT_DIM,
                                );
                                if vresponse.clicked() {
                                    pending_model = Some(entry.value.clone());
                                }
                            }
                        });
                    });
                    if response.clicked() {
                        pending_model = Some(representative.value.clone());
                    }

                    visible_count += 1;
                    if visible_count % columns == 0 {
                        ui.end_row();
                    }
                }
            });

        if let Some(value) = pending_model {
            if !value.eq_ignore_ascii_case(&current) {
                if let Err(error) = self.profile_set_cvar("model", &value) {
                    self.console_status = error;
                } else {
                    self.profile_preview_key = None;
                }
            }
        }

        if self.ui_catalog.pending.profile {
            theme::label(
                ui,
                theme::plain("Scanning player models...", 11.0, theme::TEXT_FAINT),
            );
        } else if self.profile_models.is_empty() {
            theme::banner(
                ui,
                "No humanoid models/players/*/model.glm assets were found.",
                theme::WARNING,
            );
        } else if visible_count == 0 {
            theme::label(
                ui,
                theme::plain("No models match this filter.", 11.0, theme::TEXT_FAINT),
            );
        }

        theme::section(
            ui,
            "FORCED PLAYER MODELS",
            "Client-side only · preserves your own selected model",
        );
        let parsed = crate::cgame::ForcedPlayerModels::parse(&self.force_model)
            .ok()
            .flatten();
        let mut mode = match &parsed {
            None => 0u8,
            Some(models) if models.split => 2,
            Some(_) => 1,
        };
        let own = self.solo_client_info.model_cvar();
        let mut ally = parsed
            .as_ref()
            .map(|models| models.ally.clone())
            .unwrap_or_else(|| own.clone());
        let mut enemy = parsed
            .as_ref()
            .map(|models| models.enemy.clone())
            .unwrap_or_else(|| own.clone());

        ui.horizontal(|ui| {
            if theme::chip(ui, "OFF", mode == 0).clicked() {
                mode = 0;
            }
            if theme::chip(ui, "ALL", mode == 1).clicked() {
                if mode == 0 {
                    ally = own.clone();
                    enemy = own.clone();
                } else if mode == 2 {
                    enemy = ally.clone();
                }
                mode = 1;
            }
            if theme::chip(ui, "ALLY / ENEMY", mode == 2).clicked() {
                if mode == 0 {
                    ally = own.clone();
                    enemy = own.clone();
                }
                mode = 2;
            }
        });

        let mut choices = self
            .profile_models
            .iter()
            .map(|entry| entry.value.clone())
            .collect::<Vec<_>>();
        choices.sort_by_key(|value| value.to_ascii_lowercase());
        choices.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        let model_combo = |ui: &mut egui::Ui, id: &str, label: &str, value: &mut String| {
            ui.horizontal(|ui| {
                theme::label(ui, theme::plain(label, 10.5, theme::TEXT_FAINT));
                egui::ComboBox::from_id_salt(id)
                    .selected_text(value.as_str())
                    .width((ui.available_width() - 4.0).max(150.0))
                    .show_ui(ui, |ui| {
                        for candidate in &choices {
                            ui.selectable_value(value, candidate.clone(), candidate.as_str());
                        }
                    });
            });
        };
        match mode {
            1 => model_combo(ui, "profile-force-model-all", "MODEL", &mut ally),
            2 => {
                model_combo(ui, "profile-force-model-ally", "ALLY", &mut ally);
                model_combo(ui, "profile-force-model-enemy", "ENEMY", &mut enemy);
            }
            _ => {
                theme::label(
                    ui,
                    theme::plain(
                        "Players use their own advertised models.",
                        10.5,
                        theme::TEXT_FAINT,
                    ),
                );
            }
        }

        let desired = match mode {
            0 => "0".to_owned(),
            1 => ally.clone(),
            _ => format!("{},{}", ally, enemy),
        };
        if !desired.eq_ignore_ascii_case(&self.force_model) {
            if let Err(error) = self.set_console_cvar("cg_forceModel", &desired) {
                self.console_status = error;
            }
        }
        Self::profile_readout(ui, "CVAR", &self.force_model);
    }

    pub(in crate::app::egui_menu) fn egui_profile_force(&mut self, ui: &mut egui::Ui) {
        let (max_rank, disabled, gametype, free_saber, forced_side) =
            self.profile_server_force_limits();
        let mut force = ProfileForceConfig::parse(&self.network.forcepowers);
        force.rank = max_rank;
        if let Some(side) = forced_side {
            force.side = side;
        }
        force.normalize(max_rank, disabled, gametype, free_saber);
        let before = force.serialize();

        theme::section(
            ui,
            "ALIGNMENT",
            "Light and Dark powers follow stock JKA restrictions",
        );
        ui.horizontal(|ui| {
            let light = theme::chip(ui, "LIGHT", force.side == 1);
            let dark = theme::chip(ui, "DARK", force.side == 2);
            if forced_side.is_none() {
                if light.clicked() {
                    force.side = 1;
                }
                if dark.clicked() {
                    force.side = 2;
                }
            }
        });
        if forced_side.is_some() {
            theme::label(
                ui,
                theme::plain(
                    "Side is fixed by g_forceBasedTeams for your current team.",
                    10.5,
                    theme::TEXT_FAINT,
                ),
            );
        }

        theme::section(
            ui,
            "SERVER FORCE BUDGET",
            "g_maxForceRank controls the available points",
        );
        force.normalize(max_rank, disabled, gametype, free_saber);
        let used = force.used(free_saber);
        let budget = force.budget();
        Self::profile_readout(
            ui,
            "MASTERY",
            &format!("{} ({max_rank})", FORCE_MASTERY_NAMES[max_rank as usize]),
        );
        Self::profile_readout(ui, "POINTS", &format!("{used} / {budget}"));

        theme::section(
            ui,
            "FORCE POWERS",
            "Neutral powers first, then the selected alignment",
        );
        let aligned = if force.side == 2 {
            &FORCE_DARK_ORDER[..]
        } else {
            &FORCE_LIGHT_ORDER[..]
        };
        let order = FORCE_NEUTRAL_ORDER
            .iter()
            .copied()
            .chain(aligned.iter().copied());
        let mut clicked_power = None;
        for power in order {
            let server_disabled = disabled & (1u32 << power) != 0 && !matches!(power, 1 | 15 | 16);
            let team_disabled = gametype < 6 && matches!(power, 11 | 12);
            let saber_dependency = matches!(power, 16 | 17) && force.powers[15] == 0;
            let locked = server_disabled || team_disabled || saber_dependency;
            ui.horizontal(|ui| {
                ui.set_width(ui.available_width());
                let color = if locked {
                    theme::TEXT_DISABLED
                } else {
                    theme::TEXT
                };
                theme::label(ui, theme::plain(FORCE_NAMES[power], 11.5, color));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let min_level = if power == 1 || (free_saber && matches!(power, 15 | 16)) {
                        1
                    } else {
                        0
                    };
                    for level in (min_level..=3).rev() {
                        let response =
                            theme::chip(ui, &level.to_string(), force.powers[power] == level as u8);
                        if response.clicked() && !locked && force.powers[power] != level as u8 {
                            force.powers[power] = level as u8;
                            clicked_power = Some(power);
                        }
                    }
                });
            });
            ui.add_space(3.0);
        }

        if let Some(power) = clicked_power {
            // Delay the animation until allocation clicks settle. Repeated level
            // clicks for one power therefore restart a single preview, not a
            // full Ghoul2 animation setup on every UI event.
            self.profile_force_preview_pending = Some((power, Instant::now()));
        }

        force.normalize(max_rank, disabled, gametype, free_saber);
        let after = force.serialize();
        if after != before {
            if let Err(error) = self.profile_set_cvar("forcepowers", &after) {
                self.console_status = error;
            } else {
                self.profile_force_pending = true;
            }
        }
    }

    pub(in crate::app::egui_menu) fn egui_profile_saber(&mut self, ui: &mut egui::Ui) {
        let singles = self
            .profile_sabers
            .iter()
            .filter(|entry| Self::profile_saber_is_single(entry))
            .cloned()
            .collect::<Vec<_>>();
        let staffs = self
            .profile_sabers
            .iter()
            .filter(|entry| Self::profile_saber_is_staff(entry))
            .cloned()
            .collect::<Vec<_>>();

        let current_primary = self.network.saber1.clone();
        let current_secondary = self.network.saber2.clone();
        let secondary_active = !current_secondary.is_empty()
            && !current_secondary.eq_ignore_ascii_case("none")
            && !current_secondary.eq_ignore_ascii_case("remove");
        let primary_is_staff = staffs
            .iter()
            .any(|entry| entry.name.eq_ignore_ascii_case(&current_primary));
        let mut mode = if secondary_active {
            1u8
        } else if primary_is_staff {
            2u8
        } else {
            0u8
        };
        let mut primary = current_primary.clone();
        let mut secondary = current_secondary.clone();

        let default_single = singles
            .iter()
            .find(|entry| entry.name.eq_ignore_ascii_case("single_1"))
            .or_else(|| singles.first())
            .map(|entry| entry.name.clone());
        let default_staff = staffs
            .iter()
            .find(|entry| entry.name.eq_ignore_ascii_case("dual_1"))
            .or_else(|| staffs.first())
            .map(|entry| entry.name.clone());

        theme::section(
            ui,
            "SABER CONFIGURATION",
            "Stock saber types, presented as a modern compact selector",
        );
        ui.horizontal(|ui| {
            if theme::chip(ui, "SINGLE", mode == 0).clicked() && mode != 0 {
                mode = 0;
                if !singles
                    .iter()
                    .any(|entry| entry.name.eq_ignore_ascii_case(&primary))
                {
                    if let Some(name) = &default_single {
                        primary = name.clone();
                    }
                }
                secondary = "none".to_owned();
            }
            if theme::chip(ui, "DUAL", mode == 1).clicked() && mode != 1 {
                mode = 1;
                if !singles
                    .iter()
                    .any(|entry| entry.name.eq_ignore_ascii_case(&primary))
                {
                    if let Some(name) = &default_single {
                        primary = name.clone();
                    }
                }
                if !singles
                    .iter()
                    .any(|entry| entry.name.eq_ignore_ascii_case(&secondary))
                {
                    secondary = primary.clone();
                }
            }
            if theme::chip(ui, "STAFF", mode == 2).clicked() && mode != 2 {
                mode = 2;
                if !staffs
                    .iter()
                    .any(|entry| entry.name.eq_ignore_ascii_case(&primary))
                {
                    if let Some(name) = &default_staff {
                        primary = name.clone();
                    }
                }
                secondary = "none".to_owned();
            }
        });

        ui.add_space(8.0);
        let combo = |ui: &mut egui::Ui,
                     id: &str,
                     label: &str,
                     selected: &mut String,
                     entries: &[ProfileSaberEntry]| {
            ui.horizontal(|ui| {
                theme::label(ui, theme::plain(label, 11.0, theme::TEXT_FAINT));
                let selected_text = entries
                    .iter()
                    .find(|entry| entry.name.eq_ignore_ascii_case(selected))
                    .map(Self::profile_saber_label)
                    .unwrap_or(selected.as_str())
                    .to_owned();
                egui::ComboBox::from_id_salt(id)
                    .selected_text(selected_text)
                    .width((ui.available_width() - 4.0).max(150.0))
                    .show_ui(ui, |ui| {
                        for entry in entries {
                            ui.selectable_value(
                                selected,
                                entry.name.clone(),
                                Self::profile_saber_label(entry),
                            );
                        }
                    });
            });
        };

        match mode {
            1 => {
                combo(
                    ui,
                    "profile-saber-right",
                    "RIGHT HAND",
                    &mut primary,
                    &singles,
                );
                combo(
                    ui,
                    "profile-saber-left",
                    "LEFT HAND",
                    &mut secondary,
                    &singles,
                );
            }
            2 => combo(
                ui,
                "profile-saber-staff",
                "STAFF HILT",
                &mut primary,
                &staffs,
            ),
            _ => combo(ui, "profile-saber-single", "HILT", &mut primary, &singles),
        }

        let desired_secondary = if mode == 1 {
            secondary.as_str()
        } else {
            "none"
        };
        if primary != current_primary {
            if let Err(error) = self.profile_set_cvar("saber1", &primary) {
                self.console_status = error;
            } else {
                self.profile_preview_key = None;
            }
        }
        if !desired_secondary.eq_ignore_ascii_case(&current_secondary) {
            if let Err(error) = self.profile_set_cvar("saber2", desired_secondary) {
                self.console_status = error;
            } else {
                self.profile_preview_key = None;
            }
        }

        self.egui_profile_saber_colors(ui, mode == 1);

        if let Some(entry) = self
            .profile_sabers
            .iter()
            .find(|entry| entry.name.eq_ignore_ascii_case(&primary))
        {
            theme::section(ui, "SELECTED", "Parsed directly from ext_data/sabers");
            Self::profile_readout(ui, "NAME", Self::profile_saber_label(entry));
            Self::profile_readout(ui, "TYPE", &entry.saber_type);
            Self::profile_readout(ui, "MODEL", &entry.model);
            Self::profile_readout(
                ui,
                "SKIN",
                entry.custom_skin.as_deref().unwrap_or("default"),
            );
            Self::profile_readout(ui, "BLADES", &entry.num_blades.to_string());
        }

        if self.profile_sabers.is_empty() {
            theme::banner(ui, "No saber definitions were found.", theme::WARNING);
        } else if singles.is_empty() {
            theme::banner(
                ui,
                "No SABER_SINGLE definitions were found.",
                theme::WARNING,
            );
        } else if mode == 2 && staffs.is_empty() {
            theme::banner(ui, "No SABER_STAFF definitions were found.", theme::WARNING);
        }
    }

    /// jaPRO UI_UpdateSaberCvars: `color1`/`color2` pick a stock blade colour, or
    /// `SABER_RGB` with the colour in `cp_sbRGB1`/`cp_sbRGB2`. Only jaPRO and JA+
    /// servers draw RGB; others get the closest stock colour (see userinfo_for_mod).
    pub(in crate::app::egui_menu) fn egui_profile_saber_colors(
        &mut self,
        ui: &mut egui::Ui,
        dual: bool,
    ) {
        use crate::net::{base_saber_rgb_packed, SABER_RGB};
        const COLORS: [&str; 6] = ["Red", "Orange", "Yellow", "Green", "Blue", "Purple"];

        theme::section(
            ui,
            "BLADE COLOUR",
            "Stock colours work on every server. RGB is drawn on jaPRO and JA+ servers; elsewhere the closest stock colour is sent.",
        );
        let hands: &[(usize, &str)] = if dual {
            &[(0, "RIGHT HAND"), (1, "LEFT HAND")]
        } else {
            &[(0, "BLADE")]
        };
        for &(hand, label) in hands {
            let (color_cvar, rgb_cvar) = if hand == 0 {
                ("color1", "cp_sbRGB1")
            } else {
                ("color2", "cp_sbRGB2")
            };
            let color = if hand == 0 {
                self.network.color1
            } else {
                self.network.color2
            };
            let packed = if hand == 0 {
                self.network.sb_rgb1
            } else {
                self.network.sb_rgb2
            };
            let current = i32::from(color);

            theme::label(ui, theme::plain(label, 11.0, theme::TEXT_FAINT));
            let mut picked = None;
            ui.horizontal_wrapped(|ui| {
                for (index, name) in COLORS.iter().enumerate() {
                    if theme::chip(ui, name, current == index as i32).clicked() {
                        picked = Some(index as u8);
                    }
                    ui.add_space(3.0);
                }
                if theme::chip(ui, "RGB", current == SABER_RGB).clicked() {
                    picked = Some(SABER_RGB as u8);
                }
            });
            if let Some(index) = picked.filter(|&index| index != color) {
                // Start the RGB picker from the colour being replaced.
                if i32::from(index) == SABER_RGB && packed == 0 {
                    let _ =
                        self.profile_set_cvar(rgb_cvar, &base_saber_rgb_packed(color).to_string());
                }
                if let Err(error) = self.profile_set_cvar(color_cvar, &index.to_string()) {
                    self.console_status = error;
                }
                self.profile_preview_key = None;
            }

            if current == SABER_RGB {
                // jaPRO reads an unset (0) colour as pure red. Keep its packed
                // r | g << 8 | b << 16 storage, but present the same egui RGB
                // picker used by the Sunlight Override instead of three sliders.
                let packed = if packed == 0 { 255 } else { packed };
                let mut rgb = [
                    (packed & 255) as f32 / 255.0,
                    ((packed >> 8) & 255) as f32 / 255.0,
                    ((packed >> 16) & 255) as f32 / 255.0,
                ];
                let mut changed = false;
                theme::row(
                    ui,
                    "Color",
                    "Custom saber blade color. Stored as jaPRO cp_sbRGB packed RGB.",
                    theme::Reset::None,
                    |ui| {
                        changed = ui.color_edit_button_rgb(&mut rgb).changed();
                        ui.add_space(10.0);
                        let to_u8 = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
                        theme::glow_label(
                            ui,
                            &format!("{}  {}  {}", to_u8(rgb[0]), to_u8(rgb[1]), to_u8(rgb[2])),
                            12.5,
                            theme::TEXT_FAINT,
                        );
                    },
                );
                if changed {
                    let to_u8 = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
                    let value = to_u8(rgb[0]) | (to_u8(rgb[1]) << 8) | (to_u8(rgb[2]) << 16);
                    // Picking updates the preview and config at once; userinfo
                    // is sent by APPLY.
                    match self
                        .network
                        .set_cvar(rgb_cvar, &value.to_string())
                        .unwrap_or(Ok(false))
                    {
                        Ok(true) => self.profile_userinfo_pending = true,
                        Ok(false) => {}
                        Err(error) => self.console_status = error,
                    }
                    self.mark_config_dirty();
                    self.profile_preview_key = None;
                }
            }
            ui.add_space(6.0);
        }
    }
}
