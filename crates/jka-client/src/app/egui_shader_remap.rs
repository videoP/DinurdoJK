//! Map shader remap editor. Materials are resolved through the existing
//! R_RemapShader state; this page only runs on the UI thread and never changes
//! the world draw path. Texture previews are decoded by ui_catalog's worker.
use super::egui_theme as theme;
use super::*;

#[derive(Default)]
pub(in crate::app) struct ShaderRemapUi {
    pub map_label: String,
    pub sources: Vec<String>,
    pub targets: Vec<String>,
    /// Implicit material names scanned from game-wide image assets.
    pub implicit_targets: Vec<String>,
    pub catalog_requested: bool,
    pub search: String,
    pub remapped_only: bool,
    pub selected: Option<String>,
    pub target: String,
    /// Disabling a redirect removes it from the live renderer but remembers
    /// its target here, so re-enabling does not require typing it again.
    pub disabled: BTreeMap<String, String>,
    pub thumbnails: HashMap<String, egui::TextureHandle>,
    pub thumbnails_pending: HashSet<String>,
    pub thumbnails_missing: HashSet<String>,
    pub status: String,
}

#[derive(Debug)]
enum RemapAction {
    Select(String),
    Toggle(String, bool),
    Apply,
    Reset,
    Close,
}

/// Reuse the map-material thumbnail cache for both source and replacement
/// shader previews. The only synchronous work here is drawing a cached egui
/// texture or queuing a request; disk reads and decoding stay on ui-catalog.
fn show_remap_thumbnail(
    ui: &mut egui::Ui,
    state: &mut ShaderRemapUi,
    shader_name: &str,
    image_path: &str,
    size: f32,
    requests: &mut Vec<(String, String)>,
) {
    if let Some(texture) = state.thumbnails.get(shader_name) {
        ui.add(egui::Image::new(texture).fit_to_exact_size(egui::vec2(size, size)));
        return;
    }

    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    ui.painter().rect_filled(rect, 3.0, theme::PANEL_FILL);
    if state.thumbnails_missing.contains(shader_name) {
        ui.painter().text(
            rect.center(), egui::Align2::CENTER_CENTER, "NO IMAGE",
            egui::FontId::proportional(10.0), theme::TEXT_FAINT,
        );
    } else if state.thumbnails_pending.insert(shader_name.to_owned()) {
        requests.push((shader_name.to_owned(), image_path.to_owned()));
    }
}

impl App {
    /// Open from the Dev wheel or from one Trace record. Selecting an exact
    /// traced material works even when it is not in the BSP shader table
    /// (e.g. an MD3 surface registered by the entity presenter).
    pub(in crate::app) fn open_shader_remap(&mut self, source: Option<String>) {
        let map_label = self.prepared_map_cache.as_ref()
            .map(|cache| cache.label.clone()).unwrap_or_default();
        if self.shader_remap_ui.map_label != map_label {
            self.shader_remap_ui = ShaderRemapUi {
                map_label,
                ..ShaderRemapUi::default()
            };
        }
        if let Some(cache) = self.prepared_map_cache.as_ref() {
            let ui = &mut self.shader_remap_ui;
            ui.sources = cache.map.material_debug.entries.iter()
                .map(|entry| scene::canonical_shader_name(&entry.name))
                .filter(|name| !name.is_empty()).collect();
            // Authored game shader definitions are parsed during map load;
            // this includes definitions unused by the current BSP.
            ui.targets = cache.map.shader_library.keys().cloned().collect();
            ui.targets.extend(ui.sources.iter().cloned());
            ui.targets.extend(ui.implicit_targets.iter().cloned());
            ui.sources.sort_unstable();
            ui.sources.dedup();
            ui.targets.sort_unstable();
            ui.targets.dedup();
        }
        for (name, target) in self.shader_remaps.redirects() {
            self.shader_remap_ui.sources.push(name.to_owned());
            self.shader_remap_ui.targets.push(target.to_owned());
        }
        for (name, target) in &self.shader_remap_ui.disabled {
            self.shader_remap_ui.sources.push(name.clone());
            self.shader_remap_ui.targets.push(target.clone());
        }
        if let Some(source) = source {
            let source = scene::canonical_shader_name(&source);
            if !source.is_empty() {
                self.shader_remap_ui.sources.push(source.clone());
                self.shader_remap_ui.search.clear();
                self.select_shader_remap(source);
            }
        }
        self.shader_remap_ui.sources.sort_unstable();
        self.shader_remap_ui.sources.dedup();
        self.shader_remap_ui.targets.sort_unstable();
        self.shader_remap_ui.targets.dedup();
        // Scan implicit image-based shader names on the UI worker, not while
        // rendering. The compiled definition library above is usable at once.
        if !self.shader_remap_ui.catalog_requested {
            self.shader_remap_ui.catalog_requested = true;
            self.ui_catalog.request(&self.base, self.game.as_deref(),
                ui_catalog::CatalogRequest::ShaderNames);
        }
        if self.overlay == OverlayMode::Trace {
            self.render_command(RenderCommand::ClearSurfaceInspection);
        }
        self.set_overlay(OverlayMode::ShaderRemap);
    }

    fn select_shader_remap(&mut self, source: String) {
        let source = scene::canonical_shader_name(&source);
        if source.is_empty() { return; }
        let target = self.shader_remap_ui.disabled.get(&source).cloned()
            .or_else(|| self.shader_remaps.redirects().find(|(name, _)| *name == source)
                .map(|(_, value)| value.to_owned()))
            .unwrap_or_else(|| source.clone());
        self.shader_remap_ui.selected = Some(source);
        self.shader_remap_ui.target = target;
        self.shader_remap_ui.status.clear();
    }

    fn apply_remap_editor_action(&mut self, action: RemapAction) {
        if matches!(&action, RemapAction::Toggle(..) | RemapAction::Apply | RemapAction::Reset) {
            self.placement_checkpoint();
        }
        match action {
            RemapAction::Close => self.set_overlay(OverlayMode::None),
            RemapAction::Select(source) => self.select_shader_remap(source),
            RemapAction::Toggle(source, enabled) => {
                if enabled {
                    if let Some(target) = self.shader_remap_ui.disabled.remove(&source) {
                        self.local_remap_shader(&source, &target);
                        self.model_placement.authored_remaps.insert(source.clone(), target);
                    }
                } else {
                    // Own the replacement before mutating `self`: the iterator
                    // otherwise keeps `shader_remaps` immutably borrowed.
                    let live_target = self.shader_remaps.redirects()
                        .find(|(name, _)| *name == source)
                        .map(|(_, target)| target.to_owned());
                    if let Some(target) = live_target {
                        self.shader_remap_ui.disabled.insert(source.clone(), target);
                        // old == new clears the live redirect in the existing
                        // renderer, preserving the original material unchanged.
                        self.local_remap_shader(&source, &source);
                        self.model_placement.authored_remaps.remove(&source);
                    }
                }
                if self.shader_remap_ui.selected.as_deref() == Some(&source) {
                    self.select_shader_remap(source);
                }
            }
            RemapAction::Apply => {
                let Some(source) = self.shader_remap_ui.selected.clone() else { return; };
                let target = scene::canonical_shader_name(&self.shader_remap_ui.target);
                if target.is_empty() {
                    self.shader_remap_ui.status = "Enter a replacement shader name.".to_owned();
                    return;
                }
                if self.shader_remap_ui.targets.binary_search(&target).is_err()
                    && self.shader_remap_ui.sources.binary_search(&target).is_err() {
                    self.shader_remap_ui.status = format!(
                        "Unknown shader: {target}. Choose a target from autocomplete."
                    );
                    return;
                }
                if self.shader_remap_ui.disabled.contains_key(&source) && source != target {
                    self.shader_remap_ui.disabled.insert(source.clone(), target.clone());
                    self.model_placement.authored_remaps.remove(&source);
                    self.shader_remap_ui.status = "Replacement saved. Enable this remap to apply it.".to_owned();
                } else {
                    self.shader_remap_ui.disabled.remove(&source);
                    self.local_remap_shader(&source, &target);
                    if source == target {
                        self.model_placement.authored_remaps.remove(&source);
                    } else {
                        self.model_placement.authored_remaps.insert(source.clone(), target.clone());
                    }
                    self.shader_remap_ui.status = if source == target {
                        "Original shader restored.".to_owned()
                    } else { format!("Remapping {source} → {target}") };
                }
            }
            RemapAction::Reset => {
                let Some(source) = self.shader_remap_ui.selected.clone() else { return; };
                self.shader_remap_ui.disabled.remove(&source);
                self.local_remap_shader(&source, &source);
                self.model_placement.authored_remaps.remove(&source);
                self.shader_remap_ui.target = source;
                self.shader_remap_ui.status = "Original shader restored.".to_owned();
            }
        }
    }

    pub(super) fn egui_shader_remap(&mut self, root: &mut egui::Ui) {
        let remaps: BTreeMap<String, String> = self.shader_remaps.redirects()
            .map(|(source, target)| (source.to_owned(), target.to_owned())).collect();
        let mut action: Option<RemapAction> = None;
        let mut request_images: Vec<(String, String)> = Vec::new();
        let map = self.prepared_map_cache.as_ref().map(|cache| cache.map.as_ref());
        // The same shader can appear as a map source and as an autocomplete
        // replacement. Resolve its representative image once using exactly
        // the same rule on both sides (editor image, then first real stage).
        let thumbnail_path = |name: &str| -> String {
            map.and_then(|map| map.shader_library.get(name))
                .and_then(|shader| {
                    let authored = shader.definition_text.as_deref()
                        .and_then(|text| text.lines().find_map(|line| {
                            let mut parts = line.split_whitespace();
                            if parts.next()?.eq_ignore_ascii_case("qer_editorimage") {
                                parts.next()
                            } else { None }
                        }));
                    authored.or_else(|| shader.primary().map(|stage| stage.image.as_str()))
                })
                .filter(|path| !path.starts_with('$') && !path.is_empty())
                .unwrap_or(name)
                .to_owned()
        };
        let state = &mut self.shader_remap_ui;
        let area = root.max_rect().shrink2(egui::vec2(28.0, 26.0));
        egui::Window::new("SHADER REMAPS")
            .id(egui::Id::new("jka_shader_remap_editor"))
            .default_pos(area.min)
            .default_size(egui::vec2(area.width().min(990.0), area.height().min(730.0)))
            .min_width(630.0)
            .resizable(true)
            .collapsible(false)
            .show(root.ctx(), |ui| {
                ui.horizontal(|ui| {
                    theme::label(ui, theme::plain("MAP MATERIALS", 15.0, theme::TEXT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::ghost_button(ui, "CLOSE").clicked() {
                            action = Some(RemapAction::Close);
                        }
                    });
                });
                ui.label(format!("{} map materials  ·  {} active redirects  ·  {} known targets",
                    state.sources.len(), remaps.len(), state.targets.len()));
                ui.separator();
                if map.is_none() {
                    ui.label("Load a map to browse its shader materials.");
                    return;
                }
                ui.horizontal(|ui| {
                    ui.add_sized([ui.available_width() * 0.70, 28.0],
                        egui::TextEdit::singleline(&mut state.search)
                            .hint_text("Search map shader name or current replacement…"));
                    ui.checkbox(&mut state.remapped_only, "Remapped only");
                });
                let term = state.search.trim().to_ascii_lowercase();
                let visible: Vec<String> = state.sources.iter().filter(|source| {
                    let mapped = remaps.get(*source)
                        .or_else(|| state.disabled.get(*source));
                    (!state.remapped_only || mapped.is_some())
                        && (term.is_empty() || source.contains(&term)
                            || mapped.is_some_and(|to| to.contains(&term)))
                }).cloned().collect();
                ui.add_space(5.0);
                let h = ui.available_height().max(260.0);
                ui.columns(2, |columns| {
                    columns[0].set_width((columns[0].available_width() * 1.08).max(320.0));
                    egui::ScrollArea::vertical().id_salt("remap_sources")
                        .auto_shrink([false, false]).max_height(h - 10.0)
                        .show_rows(&mut columns[0], 64.0, visible.len(), |ui, rows| {
                            for index in rows {
                                let source = &visible[index];
                                let to = remaps.get(source)
                                    .or_else(|| state.disabled.get(source)).cloned();
                                let active = remaps.contains_key(source);
                                let selected = state.selected.as_deref() == Some(source);
                                ui.horizontal(|ui| {
                                    // Only decode visible map rows; reuse the
                                    // same cache for the detail/target previews.
                                    let path = thumbnail_path(source);
                                    show_remap_thumbnail(ui, state, source, &path, 52.0,
                                        &mut request_images);
                                    ui.vertical(|ui| {
                                        if ui.selectable_label(selected, source).clicked() {
                                            action = Some(RemapAction::Select(source.clone()));
                                        }
                                        ui.label(egui::RichText::new(match to.as_deref() {
                                            Some(to) if active => format!("→ {to}"),
                                            Some(to) => format!("OFF → {to}"),
                                            None => "Original (not remapped)".to_owned(),
                                        }).small().color(theme::TEXT_FAINT));
                                    });
                                    if to.is_some() {
                                        let mut enabled = active;
                                        if ui.checkbox(&mut enabled, "").changed() {
                                            action = Some(RemapAction::Toggle(source.clone(), enabled));
                                        }
                                    }
                                });
                                ui.separator();
                            }
                        });
                    columns[1].vertical(|ui| {
                        if let Some(source) = state.selected.clone() {
                            theme::label(ui, theme::plain("SOURCE SHADER", 11.0, theme::TEXT_FAINT));
                            ui.label(egui::RichText::new(&source).monospace().strong());
                            ui.add_space(11.0);
                            theme::label(ui, theme::plain("REPLACEMENT SHADER", 11.0, theme::TEXT_FAINT));
                            ui.add_sized([ui.available_width(), 29.0],
                                egui::TextEdit::singleline(&mut state.target)
                                    .hint_text("Start typing any loaded shader…"));
                            let typed = scene::canonical_shader_name(&state.target);
                            // Preview the exact typed target once it resolves to
                            // a known shader; incomplete names must not queue
                            // new texture reads on every keystroke.
                            let target_is_known = !typed.is_empty()
                                && (state.targets.binary_search(&typed).is_ok()
                                    || state.sources.binary_search(&typed).is_ok());
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(egui::RichText::new("ORIGINAL").small()
                                        .color(theme::TEXT_FAINT));
                                    let path = thumbnail_path(&source);
                                    show_remap_thumbnail(ui, state, &source, &path, 96.0,
                                        &mut request_images);
                                });
                                ui.add_space(12.0);
                                ui.vertical(|ui| {
                                    ui.label(egui::RichText::new("REPLACEMENT PREVIEW").small()
                                        .color(theme::TEXT_FAINT));
                                    if target_is_known {
                                        let path = thumbnail_path(&typed);
                                        show_remap_thumbnail(ui, state, &typed, &path, 112.0,
                                            &mut request_images);
                                    } else {
                                        let (rect, _) = ui.allocate_exact_size(
                                            egui::vec2(112.0, 112.0), egui::Sense::hover());
                                        ui.painter().rect_filled(rect, 3.0, theme::PANEL_FILL);
                                        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER,
                                            "SELECT SHADER", egui::FontId::proportional(10.0),
                                            theme::TEXT_FAINT);
                                    }
                                });
                            });
                            ui.label(egui::RichText::new(
                                "Static preview: editor image or first texture stage."
                            ).small().color(theme::TEXT_FAINT));
                            if !typed.is_empty() {
                                // Prefix matches first, then substring matches. No
                                // shader-specific hacks or runtime renderer lookups.
                                let prefix = state.targets.iter().filter(|name| name.starts_with(&typed));
                                let partial = state.targets.iter()
                                    .filter(|name| !name.starts_with(&typed) && name.contains(&typed));
                                let matches: Vec<String> = prefix.chain(partial).take(40).cloned().collect();
                                egui::ScrollArea::vertical().id_salt("remap_targets")
                                    .max_height(180.0).show(ui, |ui| {
                                        for name in matches {
                                            if ui.selectable_label(false, &name).clicked() {
                                                state.target = name;
                                            }
                                        }
                                    });
                            }
                            ui.horizontal(|ui| {
                                if theme::primary_button(ui, "APPLY REMAP").clicked() {
                                    action = Some(RemapAction::Apply);
                                }
                                if theme::ghost_button(ui, "RESTORE ORIGINAL").clicked() {
                                    action = Some(RemapAction::Reset);
                                }
                            });
                            let actual = remaps.get(&source);
                            let disabled = state.disabled.contains_key(&source);
                            ui.label(if disabled { "Status: disabled (replacement remembered)" }
                                else if actual.is_some() { "Status: enabled" }
                                else { "Status: original" });
                            if !state.status.is_empty() { ui.label(&state.status); }
                            ui.add_space(10.0);
                            ui.label(egui::RichText::new(
                                "Changes use the existing local /remapShader path. \
                                 Remapping changes appearance, not BSP collision or geometry."
                            ).small().color(theme::TEXT_FAINT));
                        } else {
                            ui.label("Select a map material to edit its replacement.");
                        }
                    });
                });
            });
        for (source, image_path) in request_images {
            self.ui_catalog.request(&self.base, self.game.as_deref(),
                ui_catalog::CatalogRequest::ShaderThumbnail {
                    map_label: self.shader_remap_ui.map_label.clone(), source, image_path });
        }
        if let Some(action) = action { self.apply_remap_editor_action(action); }
    }
}
