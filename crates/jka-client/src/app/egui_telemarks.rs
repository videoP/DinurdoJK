//! Map-scoped client-side bookmarks. Teleport via jaPRO amTele, not setviewpos;
//! these bookmarks do not replace the server's single amTeleMark.
use super::{egui_theme as theme, playerstate_vec3, App};
use crate::ui::OverlayMode;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path};

const DATABASE_FILE: &str = "telemarks.json";
const DATABASE_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Telemark {
    name: String,
    /// JKA world coordinates of the player's origin (NOT the eye camera).
    origin: [f32; 3],
    /// JKA degrees in pitch/yaw/roll order.
    angles: [f32; 3],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct TelemarkDatabase {
    version: u32,
    maps: BTreeMap<String, Vec<Telemark>>,
}

impl Default for TelemarkDatabase {
    fn default() -> Self {
        Self { version: DATABASE_VERSION, maps: BTreeMap::new() }
    }
}

impl TelemarkDatabase {
    fn load(directory: &Path) -> Result<Self, String> {
        let path = directory.join(DATABASE_FILE);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(format!("Read {}: {error}", path.display())),
        };
        let db: Self = serde_json::from_slice(&bytes)
            .map_err(|error| format!("Parse {}: {error}", path.display()))?;
        if db.version != DATABASE_VERSION {
            return Err(format!("Unsupported telemark JSON version {} (expected {}). The file was not modified.", db.version, DATABASE_VERSION));
        }
        for (map, marks) in &db.maps {
            if map.is_empty() || marks.iter().any(|mark| {
                mark.name.trim().is_empty()
                    || !mark.origin.iter().chain(&mark.angles).all(|number| number.is_finite())
            }) {
                return Err(format!("Invalid telemark data for map {map:?}; file left unchanged."));
            }
        }
        Ok(db)
    }

    /// Commit a full replacement on disk before swapping the in-memory DB.
    /// Windows cannot rename over an existing file: use a backup and restore on failure.
    fn save(&self, directory: &Path) -> Result<(), String> {
        fs::create_dir_all(directory).map_err(|error| format!("Create {}: {error}", directory.display()))?;
        let path = directory.join(DATABASE_FILE);
        let temp = directory.join("telemarks.json.tmp");
        let backup = directory.join("telemarks.json.bak");
        let data = serde_json::to_vec_pretty(self).map_err(|error| error.to_string())?;
        fs::write(&temp, data).map_err(|error| format!("Write {}: {error}", temp.display()))?;
        let had_previous = path.exists();
        if had_previous {
            if backup.exists() {
                fs::remove_file(&backup).map_err(|error| format!("Remove stale {}: {error}", backup.display()))?;
            }
            fs::rename(&path, &backup).map_err(|error| format!("Back up {}: {error}", path.display()))?;
        }
        if let Err(error) = fs::rename(&temp, &path) {
            if had_previous {
                let _ = fs::rename(&backup, &path);
            }
            return Err(format!("Save {}: {error}", path.display()));
        }
        if had_previous { let _ = fs::remove_file(backup); }
        Ok(())
    }
}

/// Prefer the server-reported map name to prevent sharing spots between maps
/// with the same basename, and normalize map extension/case consistently.
fn map_key(map: &str) -> String {
    let map = map.trim().replace('\\', "/").to_ascii_lowercase();
    let map = map.strip_prefix("maps/").unwrap_or(&map);
    let map = map.strip_suffix(".bsp").or_else(|| map.strip_suffix(".map")).unwrap_or(map);
    map.to_owned()
}

#[derive(Default)]
pub(super) struct TelemarkUi {
    database: Option<TelemarkDatabase>,
    name_input: String,
    rename_input: String,
    search: String,
    selected: Option<usize>,
    selected_map: String,
    confirm_delete: Option<usize>,
    error: Option<String>,
    status: String,
}

#[derive(Clone, Copy)]
enum Action {
    Save,
    Update(usize),
    Rename(usize),
    Delete(usize),
    Teleport(usize),
    TeleportAdminYaw(usize),
    Copy(usize),
}

/// jaPRO parses teleport coordinates with atoi(): round to the nearest
/// whole game unit before constructing a server command.
fn amtele_coords(mark: &Telemark) -> [i32; 3] {
    mark.origin.map(|value| value.round() as i32)
}

impl App {
    fn telemark_directory(&self) -> std::path::PathBuf {
        jka_assets::pk3::game_write_directory(&self.base, None)
    }

    fn telemark_map(&self) -> Option<String> {
        self.game_session.as_ref()
            .filter(|session| session.live)
            .and_then(|session| session.map_name.as_deref())
            .map(map_key).filter(|map| !map.is_empty())
    }

    fn telemark_player_pose(&self) -> Option<([f32; 3], [f32; 3])> {
        let state = self.live_player_state()?;
        let origin = playerstate_vec3(state, "origin")?;
        let angles = playerstate_vec3(state, "viewangles")?;
        if !origin.iter().chain(&angles).all(|value| value.is_finite()) {
            return None;
        }
        Some((origin, angles))
    }

    pub(super) fn open_telemarks(&mut self) {
        if self.overlay == OverlayMode::Telemarks {
            self.set_overlay(OverlayMode::None);
            return;
        }
        let dir = self.telemark_directory();
        self.telemark_ui.database = match TelemarkDatabase::load(&dir) {
            Ok(database) => { self.telemark_ui.error = None; Some(database) }
            Err(error) => { self.telemark_ui.error = Some(error); None }
        };
        self.telemark_ui.status.clear();
        self.telemark_ui.confirm_delete = None;
        self.telemark_ui.selected = None;
        self.telemark_ui.selected_map.clear();
        self.set_overlay(OverlayMode::Telemarks);
    }

    fn telemark_apply(&mut self, map: &str, action: Action) {
        let directory = self.telemark_directory();
        let Some(database) = self.telemark_ui.database.as_ref() else { return; };
        let marks = database.maps.get(map).cloned().unwrap_or_default();
        let selected = match action {
            Action::Update(index) | Action::Rename(index) | Action::Delete(index)
            | Action::Teleport(index) | Action::TeleportAdminYaw(index) | Action::Copy(index) => marks.get(index).cloned(),
            Action::Save => None,
        };
        match action {
            Action::Teleport(_) | Action::TeleportAdminYaw(_) | Action::Copy(_) => {
                let Some(mark) = selected else { return; };
                // TaystJK's Cmd_RaceTele_f and Cmd_Amtele_f use atoi() for
                // coordinates. Send integers, with standard 3-coordinate
                // amTele so race-practice users do not need admin access.
                let xyz = amtele_coords(&mark);
                let basic = format!("amTele {} {} {}", xyz[0], xyz[1], xyz[2]);
                if matches!(action, Action::Copy(_)) {
                    match crate::clipboard::set_text(&basic) {
                        Ok(()) => {
                            self.telemark_ui.error = None;
                            self.telemark_ui.status = format!("Copied {basic}");
                        }
                        Err(error) => self.telemark_ui.error = Some(format!("Copy failed: {error}")),
                    }
                    return;
                }
                if !self.live_connected() && self.local_server.is_none() {
                    self.telemark_ui.error = Some("Join a game to use amTele.".to_owned());
                    return;
                }
                let cmd = if matches!(action, Action::TeleportAdminYaw(_)) {
                    let yaw = mark.angles[1].round() as i32;
                    if self.local_server.is_some() {
                        // Solo has no remote client ID; the local amTele shim
                        // supports the X Y Z YAW form without ambiguity.
                        format!("{basic} {yaw}")
                    } else {
                        // TaystJK's amTele X Y Z YAW may parse X as a client
                        // number. Explicitly target SELF in the 6-token admin
                        // form: amTele <player> X Y Z YAW. NEVER teleport an
                        // unrelated player if X happens to be a client number.
                        let Some(client) = self.net.as_ref().map(|net| net.session().decoder().client_number) else {
                            self.telemark_ui.error = Some("No client ID available for admin teleport.".to_owned());
                            return;
                        };
                        if !(0..64).contains(&client) {
                            self.telemark_ui.error = Some("Invalid local client ID for admin teleport.".to_owned());
                            return;
                        }
                        format!("amTele {client} {} {} {} {yaw}", xyz[0], xyz[1], xyz[2])
                    }
                } else {
                    basic
                };
                // Use the ordinary console dispatch for amTele: remote games
                // forward the command to their server; local games run the
                // explicit amTele shim. No setviewpos path is involved.
                self.set_overlay(OverlayMode::None);
                self.execute_command_line(&cmd);
                return;
            }
            _ => {}
        }

        let mut candidate = database.clone();
        let items = candidate.maps.entry(map.to_owned()).or_default();
        let new_name = self.telemark_ui.name_input.trim();
        let rename = self.telemark_ui.rename_input.trim();
        match action {
            Action::Save => {
                if new_name.is_empty() || new_name.chars().count() > 80 {
                    self.telemark_ui.error = Some("Enter a name between 1 and 80 characters.".to_owned()); return;
                }
                if items.iter().any(|item| item.name.eq_ignore_ascii_case(new_name)) {
                    self.telemark_ui.error = Some("Name exists. Select it and use Update position, or choose another name.".to_owned()); return;
                }
                let Some((origin, angles)) = self.telemark_player_pose() else {
                    self.telemark_ui.error = Some("No current player position available.".to_owned()); return;
                };
                items.push(Telemark {name: new_name.to_owned(), origin, angles});
                self.telemark_ui.status = format!("Saved {new_name} on {map}.");
            }
            Action::Update(index) => {
                let Some((origin, angles)) = self.telemark_player_pose() else {
                    self.telemark_ui.error = Some("No current player position available.".to_owned()); return;
                };
                let Some(mark) = items.get_mut(index) else { return; };
                mark.origin = origin;
                mark.angles = angles;
                self.telemark_ui.status = format!("Updated position for {}.", mark.name);
            }
            Action::Rename(index) => {
                if rename.is_empty() || rename.chars().count() > 80 {
                    self.telemark_ui.error = Some("Enter a name between 1 and 80 characters.".to_owned()); return;
                }
                if items.iter().enumerate().any(|(i, item)| i != index && item.name.eq_ignore_ascii_case(rename)) {
                    self.telemark_ui.error = Some("A telemark with that name already exists.".to_owned()); return;
                }
                let Some(mark) = items.get_mut(index) else { return; };
                mark.name = rename.to_owned();
                self.telemark_ui.status = format!("Renamed telemark to {rename}.");
            }
            Action::Delete(index) => {
                if index >= items.len() { return; }
                let removed = items.remove(index);
                self.telemark_ui.status = format!("Deleted {}.", removed.name);
                self.telemark_ui.selected = None;
                self.telemark_ui.confirm_delete = None;
            }
            Action::Teleport(_) | Action::TeleportAdminYaw(_) | Action::Copy(_) => unreachable!(),
        }
        if items.is_empty() { candidate.maps.remove(map); }
        match candidate.save(&directory) {
            Ok(()) => {
                self.telemark_ui.database = Some(candidate);
                self.telemark_ui.error = None;
                if matches!(action, Action::Save) { self.telemark_ui.name_input.clear(); }
            }
            Err(error) => {
                self.telemark_ui.status.clear();
                self.telemark_ui.error = Some(error);
            }
        }
    }

    pub(super) fn egui_telemarks(&mut self, root: &mut egui::Ui) {
        let current_map = self.telemark_map();
        if let Some(map) = current_map.as_ref() {
            if self.telemark_ui.selected_map != *map {
                self.telemark_ui.selected_map = map.clone();
                self.telemark_ui.selected = None;
                self.telemark_ui.confirm_delete = None;
                self.telemark_ui.rename_input.clear();
            }
        } else {
            self.telemark_ui.selected_map.clear();
            self.telemark_ui.selected = None;
        }
        let marks = current_map.as_ref()
            .and_then(|map| self.telemark_ui.database.as_ref()?.maps.get(map))
            .cloned().unwrap_or_default();
        let pose = self.telemark_player_pose();
        let mut close = false;
        let mut action = None;
        egui::CentralPanel::default().show(root, |ui| {
            ui.horizontal(|ui| {
                ui.heading("TELEMARKS");
                ui.label(egui::RichText::new("Saved teleport locations per map").weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() { close = true; }
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("MAP").color(theme::TEXT_DIM).strong());
                ui.label(egui::RichText::new(current_map.as_deref().unwrap_or("No active gameplay map")).strong());
                ui.label(egui::RichText::new(format!("{} saved", marks.len())).weak());
            });
            if let Some((origin, angles)) = pose {
                ui.label(egui::RichText::new(format!("Current: ({:.1}, {:.1}, {:.1})  yaw {:.1} / pitch {:.1}",
                    origin[0], origin[1], origin[2], angles[1], angles[0])).weak().small());
            }
            if let Some(error) = &self.telemark_ui.error {
                ui.colored_label(theme::DANGER, error);
            }
            if !self.telemark_ui.status.is_empty() {
                ui.label(egui::RichText::new(&self.telemark_ui.status).color(theme::ACCENT));
            }
            ui.add_space(8.0);
            let can_edit = self.telemark_ui.database.is_some() && current_map.is_some();
            ui.add_enabled_ui(can_edit, |ui| {
                ui.horizontal(|ui| {
                    ui.label("New mark");
                    ui.add(egui::TextEdit::singleline(&mut self.telemark_ui.name_input)
                        .hint_text("Name this location").desired_width(250.0));
                    if ui.add_enabled(pose.is_some(), egui::Button::new("Save current position")).clicked() {
                        action = Some(Action::Save);
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label("Find");
                ui.add(egui::TextEdit::singleline(&mut self.telemark_ui.search)
                    .hint_text("Filter saved telemarks").desired_width(260.0));
            });
            ui.separator();
            ui.columns(2, |columns| {
                let left = &mut columns[0];
                left.heading("Locations");
                let filter = self.telemark_ui.search.trim().to_ascii_lowercase();
                egui::ScrollArea::vertical().id_salt("telemark_list").max_height(400.0).show(left, |ui| {
                    if marks.is_empty() { ui.label("No bookmarks saved for this map."); }
                    for (index, mark) in marks.iter().enumerate() {
                        if !mark.name.to_ascii_lowercase().contains(&filter) { continue; }
                        if ui.selectable_label(self.telemark_ui.selected == Some(index), &mark.name).clicked() {
                            self.telemark_ui.selected = Some(index);
                            self.telemark_ui.rename_input = mark.name.clone();
                            self.telemark_ui.confirm_delete = None;
                        }
                    }
                });
                let right = &mut columns[1];
                right.heading("Selected location");
                if let Some((index, mark)) = self.telemark_ui.selected.and_then(|index| marks.get(index).map(|mark| (index, mark))) {
                    right.label(egui::RichText::new(&mark.name).strong());
                    right.label(format!("Position: {:.2}  {:.2}  {:.2}", mark.origin[0], mark.origin[1], mark.origin[2]));
                    right.label(format!("Angles: pitch {:.1}  yaw {:.1}", mark.angles[0], mark.angles[1]));
                    right.add_space(10.0);
                    right.horizontal(|ui| {
                        if ui.button("Teleport (amTele)").clicked() {
                            action = Some(Action::Teleport(index));
                        }
                        if ui.button("Copy amTele").clicked() { action = Some(Action::Copy(index)); }
                    });
                    if right.button("Teleport with yaw (admin)").clicked() {
                        action = Some(Action::TeleportAdminYaw(index));
                    }
                    right.label(egui::RichText::new(
                        "Teleport uses amTele X Y Z (rounded to whole units); the server controls permissions. \
                         The optional yaw command needs admin access on multiplayer servers. \
                         Pitch cannot be restored through coordinate-based amTele."
                    ).color(theme::TEXT_DIM).small());
                    right.separator();
                    right.horizontal(|ui| {
                        ui.label("Name");
                        ui.add(egui::TextEdit::singleline(&mut self.telemark_ui.rename_input).desired_width(200.0));
                        if ui.button("Rename").clicked() { action = Some(Action::Rename(index)); }
                    });
                    if right.add_enabled(pose.is_some(), egui::Button::new("Update position to current spot")).clicked() {
                        action = Some(Action::Update(index));
                    }
                    right.add_space(8.0);
                    if self.telemark_ui.confirm_delete == Some(index) {
                        right.horizontal(|ui| {
                            ui.colored_label(theme::WARNING, "Delete this telemark?");
                            if ui.button("Confirm delete").clicked() { action = Some(Action::Delete(index)); }
                            if ui.button("Cancel").clicked() { self.telemark_ui.confirm_delete = None; }
                        });
                    } else if right.button("Delete...").clicked() {
                        self.telemark_ui.confirm_delete = Some(index);
                    }
                } else {
                    right.label("Select a saved location on the left.");
                }
            });
            ui.add_space(8.0);
            ui.label(egui::RichText::new(format!("Stored in {}", self.telemark_directory().join(DATABASE_FILE).display()))
                .small().color(theme::TEXT_DIM));
        });
        if let (Some(map), Some(action)) = (current_map.as_deref(), action) {
            self.telemark_apply(map, action);
        }
        if close { self.set_overlay(OverlayMode::None); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn map_keys_preserve_subdirectories() {
        assert_eq!(map_key("maps/MP/FFA3.BSP"), "mp/ffa3");
        assert_eq!(map_key("MyMap.map"), "mymap");
        assert_ne!(map_key("mp/ffa3"), map_key("test/ffa3"));
    }
    #[test]
    fn amtele_coordinates_round_like_server_integer_inputs() {
        let mark = Telemark {
            name: "Test".into(),
            origin: [101.7, -45.6, 128.2],
            angles: [-10.0, 90.0, 0.0],
        };
        assert_eq!(amtele_coords(&mark), [102, -46, 128]);
    }
    #[test]
    fn json_round_trip() {
        let mut db = TelemarkDatabase::default();
        db.maps.insert("mp/ffa3".into(), vec![Telemark {
            name: "Roof".into(), origin: [1., 2., 3.], angles: [-20., 180., 0.],
        }]);
        let parsed: TelemarkDatabase = serde_json::from_slice(&serde_json::to_vec(&db).unwrap()).unwrap();
        assert_eq!(parsed.maps["mp/ffa3"][0].origin, [1., 2., 3.]);
        assert_eq!(parsed.version, DATABASE_VERSION);
    }
}
