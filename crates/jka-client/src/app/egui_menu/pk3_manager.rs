//! Mounted archive manager. Package edits are staged locally and then applied
//! through the same fs_restart path used by the console command.
use super::{theme, App};
use std::{collections::HashSet, fs::File, path::{Path, PathBuf}};

/// A newly disabled archive can leave stale successful registrations alive;
/// enabling archives alone only affects future lookups and failed assets.
fn pk3_apply_needs_rebuild(applied: &HashSet<PathBuf>, staged: &HashSet<PathBuf>) -> bool {
    staged.difference(applied).next().is_some()
}

#[cfg(test)]
mod apply_tests {
    use super::*;

    #[test]
    fn enable_only_uses_fs_restart_but_any_disable_requires_fs_rebuild() {
        let original = HashSet::from([PathBuf::from("a.pk3")]);
        assert!(!pk3_apply_needs_rebuild(&original, &HashSet::new()));
        assert!(!pk3_apply_needs_rebuild(&original, &original));
        let changed = HashSet::from([PathBuf::from("b.pk3")]);
        assert!(pk3_apply_needs_rebuild(&original, &changed));
        assert!(pk3_apply_needs_rebuild(&HashSet::new(), &changed));
    }
}

// The pane fills are deliberately subdued, so package status is readable
// without competing with the blue selection highlight or other Setup pages.
const DISABLED_PANE: egui::Color32 = egui::Color32::from_rgb(0x36, 0x24, 0x20);
const ENABLED_PANE: egui::Color32 = egui::Color32::from_rgb(0x26, 0x33, 0x25);
const DISABLED_ACCENT: egui::Color32 = theme::DANGER;
const ENABLED_ACCENT: egui::Color32 = theme::ACCENT;

/// A package's selected state is separate from its staged enabled state.
/// The arrow stages a move, while clicking its filename only changes the
/// inspector selection. Both lists retain actual VFS precedence order.
fn package_pane(
    ui: &mut egui::Ui,
    packages: &[jka_assets::pk3::PackageInfo],
    disabled: &HashSet<PathBuf>,
    applied: &HashSet<PathBuf>,
    selected: Option<&Path>,
    search: &str,
    is_disabled: bool,
    width: f32,
    height: f32,
) -> (Option<PathBuf>, Option<(PathBuf, bool)>) {
    let mut inspect = None;
    let mut move_package = None;
    let count = packages.iter().filter(|p| disabled.contains(&p.path) == is_disabled).count();
    let visible_count = packages.iter().filter(|p| {
        disabled.contains(&p.path) == is_disabled &&
        (search.is_empty() || p.path.file_name().is_some_and(|name| name.to_string_lossy().to_ascii_lowercase().contains(search))
            || p.directory.to_string_lossy().to_ascii_lowercase().contains(search))
    }).count();
    let (color, fill, title) = if is_disabled {
        (DISABLED_ACCENT, DISABLED_PANE, "DISABLED")
    } else {
        (ENABLED_ACCENT, ENABLED_PANE, "ENABLED")
    };

    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            egui::Frame::new()
                .fill(fill)
                .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.45)))
                .corner_radius(egui::CornerRadius::same(5))
                .inner_margin(egui::Margin::symmetric(10, 10))
                .show(ui, |ui| {
                    ui.set_min_size(egui::vec2((width - 20.0).max(1.0), (height - 20.0).max(1.0)));
                    ui.horizontal(|ui| {
                        ui.colored_label(color, egui::RichText::new(title).strong().size(13.0));
                        ui.label(format!("{visible_count} / {count}"));
                    });
                    ui.separator();
                    let scroll_h = (height - 64.0).max(70.0);
                    egui::ScrollArea::vertical()
                        .id_salt(if is_disabled { "pk3_disabled_list" } else { "pk3_enabled_list" })
                        .auto_shrink([false, false])
                        .max_height(scroll_h)
                        .show(ui, |ui| {
                            if visible_count == 0 {
                                ui.add_space(12.0);
                                ui.colored_label(theme::TEXT_FAINT, if search.is_empty() && is_disabled {
                                    "No disabled packages"
                                } else { "No matching packages" });
                            }
                            for package in packages {
                                if disabled.contains(&package.path) != is_disabled { continue; }
                                let name = package.path.file_name()
                                    .map(|name| name.to_string_lossy()).unwrap_or_default();
                                if !search.is_empty()
                                    && !name.to_ascii_lowercase().contains(search)
                                    && !package.directory.to_string_lossy().to_ascii_lowercase().contains(search) { continue; }
                                let is_selected = selected == Some(package.path.as_path());
                                let is_pending = disabled.contains(&package.path) != applied.contains(&package.path);
                                let source = package.directory.file_name().map(|s| s.to_string_lossy()).unwrap_or_default();
                                let secondary = if is_pending { " • PENDING" } else { "" };
                                let caption = format!("{:.1} MiB · {source}{secondary}", package.bytes as f64 / (1024.0 * 1024.0));
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 5.0;
                                    if !is_disabled {
                                        if package.stock {
                                            ui.add_sized([27.0, 35.0], egui::Label::new(egui::RichText::new("LOCK").size(9.0)).selectable(false))
                                                .on_hover_text("Stock JKA archive: cannot disable");
                                        } else if ui.add_sized([27.0, 35.0], egui::Button::new("←"))
                                            .on_hover_text(format!("Disable {name} (staged until Apply)")).clicked() {
                                            move_package = Some((package.path.clone(), false));
                                        }
                                    }
                                    let row_width = (ui.available_width() - if is_disabled { 33.0 } else { 0.0 }).max(50.0);
                                    let text = format!("{name}\n{caption}");
                                    if ui.add_sized([row_width, 39.0], egui::Button::new(text)
                                        .selected(is_selected).truncate())
                                        .on_hover_text(package.path.display().to_string())
                                        .clicked() {
                                        inspect = Some(package.path.clone());
                                    }
                                    if is_disabled && ui.add_sized([27.0, 35.0], egui::Button::new("→"))
                                        .on_hover_text(format!("Enable {name} (staged until Apply)")).clicked() {
                                        move_package = Some((package.path.clone(), true));
                                    }
                                });
                            }
                        });
                });
        },
    );
    (inspect, move_package)
}

impl App {
    fn pk3_manager_inventory(&mut self) {
        if self.pk3_manager_loaded { return; }
        let previously_selected = self.pk3_manager_selected.take();
        self.pk3_manager_packages = jka_assets::pk3::installed_packages(&self.base, self.game.as_deref());
        self.pk3_manager_disabled = jka_assets::pk3::disabled_package_paths();
        self.pk3_manager_loaded = true;
        self.pk3_manager_entries.clear();
        self.pk3_manager_entries_error = None;
        self.pk3_manager_entries_search.clear();
        // Rescan/Apply should not throw away the user's inspector selection
        // if the package still exists in an installed filesystem layer.
        if let Some(path) = previously_selected.filter(|path| {
            self.pk3_manager_packages.iter().any(|p| &p.path == path)
        }) {
            self.pk3_manager_select(&path);
        }
    }

    fn pk3_manager_select(&mut self, path: &Path) {
        if self.pk3_manager_selected.as_deref() == Some(path) { return; }
        self.pk3_manager_selected = Some(path.to_owned());
        self.pk3_manager_entries.clear();
        self.pk3_manager_entries_error = None;
        self.pk3_manager_entries_search.clear();
        let result = (|| -> Result<Vec<String>, String> {
            let archive = zip::ZipArchive::new(File::open(path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            let mut names = (0..archive.len()).filter_map(|i| archive.name_for_index(i))
                .map(str::to_owned).collect::<Vec<_>>();
            names.sort_unstable_by_key(|name| name.to_ascii_lowercase());
            Ok(names)
        })();
        match result {
            Ok(entries) => self.pk3_manager_entries = entries,
            Err(error) => self.pk3_manager_entries_error = Some(error),
        }
    }

    fn pk3_inspector(&mut self, ui: &mut egui::Ui, width: f32, height: f32, applied: &HashSet<PathBuf>) {
        ui.allocate_ui_with_layout(
            egui::vec2(width, height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                egui::Frame::new()
                    .fill(theme::RAIL_FILL)
                    .stroke(egui::Stroke::new(1.0, theme::LINE_STRONG))
                    .corner_radius(egui::CornerRadius::same(5))
                    .inner_margin(egui::Margin::symmetric(10, 10))
                    .show(ui, |ui| {
                        ui.set_min_size(egui::vec2((width - 20.0).max(1.0), (height - 20.0).max(1.0)));
                        ui.label(egui::RichText::new("PACKAGE INSPECTOR").strong().size(13.0));
                        ui.separator();
                        let Some(path) = &self.pk3_manager_selected else {
                            ui.add_space(12.0);
                            ui.colored_label(theme::TEXT_DIM, "Select a PK3 from either pane to inspect its contents.");
                            return;
                        };
                        let Some((priority, package)) = self.pk3_manager_packages.iter().enumerate()
                            .find(|(_, package)| &package.path == path) else { return; };
                        let enabled = !self.pk3_manager_disabled.contains(path);
                        let pending = self.pk3_manager_disabled.contains(path) != applied.contains(path);
                        let name = path.file_name().map(|s| s.to_string_lossy()).unwrap_or_default();
                        ui.label(egui::RichText::new(name.as_ref()).strong().size(14.0));
                        ui.horizontal(|ui| {
                            ui.colored_label(if enabled { ENABLED_ACCENT } else { DISABLED_ACCENT },
                                if enabled { "ENABLED" } else { "DISABLED" });
                            if pending { ui.colored_label(theme::WARNING, "PENDING APPLY"); }
                            if package.stock { ui.colored_label(theme::TEXT_DIM, "STOCK / LOCKED"); }
                        });
                        ui.label(format!("{:.1} MiB · priority #{} · {}", package.bytes as f64 / (1024.0 * 1024.0),
                            priority + 1, package.directory.display()));
                        ui.add(egui::Label::new(egui::RichText::new(path.display().to_string())
                            .size(10.5).color(theme::TEXT_FAINT)).wrap());
                        ui.add_space(4.0);
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.label(format!("CONTENTS · {} entries", self.pk3_manager_entries.len()));
                        });
                        ui.add(egui::TextEdit::singleline(&mut self.pk3_manager_entries_search)
                            .hint_text("Filter member paths...").desired_width(f32::INFINITY));
                        if let Some(error) = &self.pk3_manager_entries_error {
                            ui.colored_label(theme::DANGER, error);
                        }
                        let search = self.pk3_manager_entries_search.to_ascii_lowercase();
                        let visible = self.pk3_manager_entries.iter().enumerate()
                            .filter_map(|(i, entry)| (search.is_empty() || entry.to_ascii_lowercase().contains(&search)).then_some(i))
                            .collect::<Vec<_>>();
                        let scroll_height = (height - 200.0).max(70.0);
                        egui::ScrollArea::vertical().id_salt("pk3_manager_contents")
                            .auto_shrink([false, false])
                            .max_height(scroll_height)
                            .show_rows(ui, 19.0, visible.len(), |ui, range| {
                                for index in range {
                                    let name = &self.pk3_manager_entries[visible[index]];
                                    ui.add(egui::Label::new(name.as_str()).truncate()).on_hover_text(name);
                                }
                            });
                    });
            },
        );
    }

    pub(in crate::app::egui_menu) fn egui_pk3_manager_page(&mut self, ui: &mut egui::Ui) {
        self.pk3_manager_inventory();
        theme::page_title(ui, "PK3 MANAGER", "Move packages between Disabled and Enabled. Changes are staged until Apply.");
        ui.horizontal(|ui| {
            ui.label("Search PK3s:");
            ui.add(egui::TextEdit::singleline(&mut self.pk3_manager_search)
                .hint_text("Filename or source directory...").desired_width(360.0));
            if ui.small_button("Clear").clicked() { self.pk3_manager_search.clear(); }
            if ui.small_button("Rescan list").on_hover_text("Refresh installed archives (discards un-applied changes)").clicked() {
                self.pk3_manager_loaded = false;
                self.pk3_manager_inventory();
            }
        });
        let applied = jka_assets::pk3::disabled_package_paths();
        let search = self.pk3_manager_search.to_ascii_lowercase();
        let total = ui.available_width().max(1.0);
        let gap = 10.0;
        let inner = (total - 2.0 * gap).max(1.0);
        let disabled_w = inner * 0.28;
        let enabled_w = inner * 0.34;
        let inspector_w = inner - disabled_w - enabled_w;
        let body_height = (ui.available_height() - 104.0).max(220.0);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            let (selection_left, moved_left) = package_pane(
                ui, &self.pk3_manager_packages, &self.pk3_manager_disabled, &applied,
                self.pk3_manager_selected.as_deref(), &search, true, disabled_w, body_height,
            );
            let (selection_middle, moved_middle) = package_pane(
                ui, &self.pk3_manager_packages, &self.pk3_manager_disabled, &applied,
                self.pk3_manager_selected.as_deref(), &search, false, enabled_w, body_height,
            );
            if let Some((path, enable)) = moved_left.or(moved_middle) {
                if enable { self.pk3_manager_disabled.remove(&path); }
                else { self.pk3_manager_disabled.insert(path); }
            }
            if let Some(path) = selection_left.or(selection_middle) {
                self.pk3_manager_select(&path);
            }
            self.pk3_inspector(ui, inspector_w, body_height, &applied);
        });
        ui.add_space(8.0);
        ui.separator();
        let changed = self.pk3_manager_disabled != applied;
        let needs_rebuild = pk3_apply_needs_rebuild(&applied, &self.pk3_manager_disabled);
        let enabled_count = self.pk3_manager_packages.iter()
            .filter(|p| !self.pk3_manager_disabled.contains(&p.path)).count();
        let disabled_count = self.pk3_manager_packages.len() - enabled_count;
        let staged = self.pk3_manager_packages.iter().filter(|p|
            self.pk3_manager_disabled.contains(&p.path) != applied.contains(&p.path)).count();
        ui.horizontal(|ui| {
            ui.label(format!("{} enabled · {} disabled · {} pending change(s)", enabled_count, disabled_count, staged));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if needs_rebuild { "Apply (fs_rebuild)" } else { "Apply (fs_restart)" };
                if ui.add_enabled(changed, egui::Button::new(label)).clicked() {
                    let pending = self.pk3_manager_disabled.clone();
                    match jka_assets::pk3::apply_disabled_packages(&self.base, self.game.as_deref(), &pending) {
                        Ok(()) => {
                            let refresh_result = if needs_rebuild {
                                self.rebuild_filesystem()
                            } else {
                                self.restart_filesystem();
                                Ok(())
                            };
                            match refresh_result {
                                Ok(()) => {
                                    self.pk3_manager_status = Some(if needs_rebuild {
                                        "Applied. Full CGame, sound, world and GPU rebuild requested; network session preserved.".into()
                                    } else {
                                        "Applied. Additive PK3 mounts re-indexed (fs_restart); existing assets retained.".into()
                                    });
                                }
                                Err(error) => {
                                    let rollback = jka_assets::pk3::apply_disabled_packages(
                                        &self.base, self.game.as_deref(), &applied
                                    );
                                    self.restart_filesystem();
                                    self.pk3_manager_status = Some(match rollback {
                                        Ok(()) => format!("Could not rebuild; package changes reverted: {error}"),
                                        Err(rollback_error) => format!("Could not rebuild: {error}; also failed to restore package choices: {rollback_error}"),
                                    });
                                }
                            }
                            self.pk3_manager_loaded = false;
                            self.pk3_manager_inventory();
                        }
                        Err(error) => self.pk3_manager_status = Some(format!("Could not apply: {error}")),
                    }
                }
                if ui.add_enabled(changed, egui::Button::new("Revert pending")).clicked() {
                    self.pk3_manager_disabled = applied.clone();
                }
            });
        });
        if let Some(status) = &self.pk3_manager_status { ui.label(status); }
        if self.game_session.is_some() || self.live_cgame_prepared.is_some() {
            ui.colored_label(theme::TEXT_DIM, "Connected: the full rebuild retains your session, but pure servers may require a reconnect.");
        }
    }
}
