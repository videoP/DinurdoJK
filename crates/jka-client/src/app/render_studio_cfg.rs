//! Render Studio graphics profiles: named renderer CFGs that apply only while
//! the Studio is open, so preview/export quality never rewrites gameplay
//! settings. Only live-applied, archived renderer cvars are included, and a
//! profile is parsed as `seta` lines only, never executed as a console script.
//!
//! On entering the Studio the gameplay values are flushed to DinurdoJK.cfg and
//! snapshotted; normal config saves are suspended until the Studio is left,
//! when the snapshot is restored. A crash inside the Studio therefore leaves
//! the gameplay CFG exactly as it was.
use super::{App, FrontendPage};
use std::collections::BTreeMap;
use serde::{Serialize,Deserialize};

/// Explicit Studio-only HUD allowlist. No gameplay cvar is written or applied.
pub(in crate::app) const HUD_ELEMENTS: &[(&str, &str)] = &[
    ("core", "Health / armor / ammo / force"),
    ("crosshair", "Crosshair"),
    ("race_timer", "Race timer"),
    ("game_timer", "Game timer"),
    ("speedometer", "Speedometer"),
    ("strafe_helper", "Strafe helper"),
    ("movement_keys", "Movement keys"),
    ("follow", "Followed player"),
    ("score", "Player score"),
    ("mini_scores", "Team / duel scores (cg_drawScores)"),
    ("enemy_info", "Enemy / duel info"),
    ("crosshair_name", "Crosshair name"),
    ("player_names", "Player names"),
    ("player_names_cinematic", "Cinematic player names"),
];

pub(in crate::app) fn studio_hud_key(key: &str) -> bool {
    HUD_ELEMENTS.iter().any(|(id,_)| *id == key)
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::app) struct StudioHudSettings {
    #[serde(default)] pub values: BTreeMap<String, bool>,
}
impl StudioHudSettings {
    pub fn enabled(&self, key: &str) -> bool { self.values.get(key).copied().unwrap_or(false) }
    pub fn set(&mut self, key: &str, enabled: bool) {
        if studio_hud_key(key) { self.values.insert(key.to_owned(),enabled); }
    }
    pub fn preset(name:&str)->Self {
        let mut hud=Self::default();
        let keys: &[&str] = match name {
            "Gameplay"=> &["core","crosshair","race_timer","game_timer","speedometer","strafe_helper","movement_keys","follow","score","mini_scores","enemy_info","crosshair_name","player_names"],
            "Race"=> &["race_timer","speedometer","strafe_helper","movement_keys"],
            _=> &[],
        };
        for &key in keys {hud.set(key,true);}
        hud
    }
    pub fn effective(&self, overrides: &BTreeMap<String,bool>) -> Self {
        let mut resolved=self.clone();
        for (key,value) in overrides {resolved.set(key,*value);}
        resolved
    }
}

use std::path::PathBuf;

pub(in crate::app) const DEFAULT_PROFILE: &str = "studio_default.cfg";

#[derive(Clone, Debug)]
pub(in crate::app) struct StudioGraphicsSnapshot {
    pub values: BTreeMap<String, String>,
    pub was_dirty: bool,
}

/// Window, swapchain, adapter, frame pacing and diagnostics stay the user's
/// gameplay preferences even inside the Studio.
const EXCLUDED: &[&str] = &[
    "r_fullscreen", "r_mode", "r_customwidth", "r_customheight", "r_backend",
    "r_gpuadapter", "r_swapinterval", "r_maxframelatency", "r_earlysubmit",
    "r_presentcopy", "r_windowmaximized", "r_windowx", "r_windowy", "r_skipui",
    "r_verbose", "r_gputimings", "r_perftrace", "r_pipelinetrace", "r_perfthreadpage",
];

/// Name filter only (no registry lookup), so presets can test names cheaply.
pub(in crate::app) fn studio_graphics_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    (lower.starts_with("r_") || lower.starts_with("fx_")) && !EXCLUDED.contains(&lower.as_str())
}

/// A Studio profile cvar: renderer-named, archived and applied live. Latched
/// cvars (vid_restart / map reload) are excluded: switching them per workspace
/// would force a renderer restart on every enter and exit.
pub(in crate::app) fn studio_graphics_cvar(name: &str) -> bool {
    studio_graphics_name(name)
        && crate::console::find(name).is_some_and(|entry| entry.latch == crate::console::LatchScope::None)
        && super::commands::registry::cvar_registry().find(name)
            .is_some_and(|def| def.flags.contains(crate::console::cvars::CvarFlags::ARCHIVE))
}

/// Only simple basename.cfg files are accepted. No paths, traversal, command
/// separators or other extensions.
pub(in crate::app) fn studio_cfg_filename(input: &str) -> Option<String> {
    let stem = input.trim().strip_suffix(".cfg").unwrap_or(input.trim());
    if stem.is_empty() || stem.len() > 60 || !stem.bytes().all(|b|
        b.is_ascii_alphanumeric() || b == b'-' || b == b'_') { return None; }
    Some(format!("{stem}.cfg"))
}

/// Parse a profile: `seta <cvar> "<json string>"` lines and // comments.
fn parse_profile(data: &str, valid: impl Fn(&str) -> Option<String>) -> Result<BTreeMap<String, String>, String> {
    let mut values = BTreeMap::new();
    for (line_number, line) in data.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") { continue; }
        let n = line_number + 1;
        let rest = line.strip_prefix("seta ").ok_or_else(|| format!("line {n}: only 'seta' is allowed"))?;
        let (key, raw) = rest.trim().split_once(char::is_whitespace)
            .ok_or_else(|| format!("line {n}: missing value"))?;
        let canonical = valid(key).ok_or_else(|| format!("line {n}: {key} is not a Studio renderer cvar"))?;
        let value: String = serde_json::from_str(raw.trim()).map_err(|e| format!("line {n}: {e}"))?;
        values.insert(canonical, value);
    }
    if values.is_empty() { return Err("profile contains no renderer settings".into()); }
    Ok(values)
}

impl App {
    fn studio_cfg_folder(&self) -> PathBuf {
        // Explicitly base: profiles should not silently change with fs_game.
        self.studio_media_root().join("profiles")
    }

    fn studio_graphics_snapshot(&self) -> StudioGraphicsSnapshot {
        let values = super::commands::registry::cvar_registry().iter()
            .filter(|def| studio_graphics_cvar(def.entry.name))
            .filter_map(|def| self.console_cvar_value(def.entry.name)
                .map(|value| (def.entry.name.to_owned(), value)))
            .collect();
        StudioGraphicsSnapshot { values, was_dirty: self.config_dirty }
    }

    /// Apply changed values only, in one renderer settings batch, so opening
    /// the Studio with an unchanged profile costs nothing.
    fn studio_apply_graphics(&mut self, values: &BTreeMap<String, String>) -> Result<(), String> {
        if let Some(name) = values.keys().find(|name| !studio_graphics_cvar(name)) {
            return Err(format!("{name} is not a Studio renderer cvar"));
        }
        let changes: Vec<_> = values.iter().filter(|(name, value)|
            self.console_cvar_value(name).as_deref() != Some(value.as_str())).collect();
        if changes.is_empty() { return Ok(()); }
        self.begin_settings_batch("Render Studio renderer profile");
        let mut failure = None;
        for (name, value) in changes {
            if let Err(error) = self.set_console_cvar(name, value) {
                failure = Some(format!("{name}: {error}"));
                break;
            }
        }
        self.end_settings_batch();
        failure.map_or(Ok(()), Err)
    }

    fn studio_cfg_read(&self, filename: &str) -> Result<(BTreeMap<String, String>, StudioHudSettings, crate::ui::HudLayout), String> {
        if studio_cfg_filename(filename).as_deref() != Some(filename) { return Err("Invalid profile filename".into()); }
        let path = self.studio_cfg_folder().join(filename);
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if bytes.len() > 128 * 1024 { return Err("profile exceeds 128 KiB".into()); }
        let data = String::from_utf8(bytes).map_err(|e| e.to_string())?;
        let mut parsed=parse_profile(&data, |key| {
            if let Some(id)=key.strip_prefix("studio_hud_") {
                return studio_hud_key(id).then(|| key.to_owned());
            }
            if let Some(id)=key.strip_prefix("studio_layout_") {
                return (id=="snap" || id=="grid" ||
                    crate::ui::HudElementId::from_cvar(id).is_some()).then(|| key.to_owned());
            }
            studio_graphics_cvar(key).then(|| super::commands::registry::cvar_registry()
                .find(key).map(|def| def.entry.name.to_owned())).flatten()
        }).map_err(|e|format!("{filename} {e}"))?;
        let mut hud=StudioHudSettings::default();
        for &(id,_) in HUD_ELEMENTS {
            if let Some(value)=parsed.remove(&format!("studio_hud_{id}")) {
                match value.as_str() {
                    "0"=>hud.set(id,false), "1"=>hud.set(id,true),
                    _=>return Err(format!("{filename}: studio_hud_{id} must be 0 or 1")),
                }
            }
        }
        // Older profiles without layout rows start with stock HUD positions,
        // NOT whichever cg_hud* layout happens to be active in gameplay.
        let mut layout=crate::ui::HudLayout::default();
        if let Some(value)=parsed.remove("studio_layout_snap") {
            layout.snap_to_grid=match value.as_str() {"0"=>false,"1"=>true,
                _=>return Err(format!("{filename}: studio_layout_snap must be 0 or 1"))};
        }
        if let Some(value)=parsed.remove("studio_layout_grid") {
            let size=value.parse::<f32>().map_err(|_|format!("{filename}: invalid HUD grid size"))?;
            if !size.is_finite() || !(1.0..=64.0).contains(&size) {
                return Err(format!("{filename}: Studio HUD grid must be between 1 and 64"));
            }
            layout.grid_size=size;
        }
        for id in crate::ui::HudElementId::ALL {
            if let Some(value)=parsed.remove(&format!("studio_layout_{}",id.cvar_name())) {
                *layout.element_mut(id)=crate::ui::HudElementLayout::from_config(&value)
                    .ok_or_else(||format!("{filename}: invalid Studio layout for {}",id.label()))?;
            }
        }
        Ok((parsed,hud,layout))
    }

    pub(in crate::app) fn studio_cfg_refresh(&mut self) {
        let mut profiles: Vec<String> = std::fs::read_dir(self.studio_cfg_folder()).ok().into_iter().flatten().flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| studio_cfg_filename(name).as_deref() == Some(name.as_str()))
            .collect();
        profiles.sort_unstable();
        self.render_studio.graphics_profiles = profiles;
    }

    /// Write the live Studio renderer values to the selected profile.
    pub(in crate::app) fn studio_cfg_save(&mut self) -> Result<(), String> {
        let filename = self.render_studio.graphics_profile.clone();
        if studio_cfg_filename(&filename).as_deref() != Some(filename.as_str()) {
            return Err("Invalid Studio profile name".into());
        }
        let folder = self.studio_cfg_folder();
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let mut content = String::from("// DinurdoJK Render Studio graphics + HUD profile\n// All studio_layout_* values are virtual 1280x720 offsets, independent of gameplay cg_hud* cvars.\n");
        for (name, value) in self.studio_graphics_snapshot().values {
            // JSON quoting encodes quotes/newlines safely for the Studio parser.
            content.push_str(&format!("seta {name} {}\n", serde_json::to_string(&value).map_err(|e| e.to_string())?));
        }
        for &(id,_) in HUD_ELEMENTS {
            let value=if self.render_studio.hud_settings.enabled(id){"1"}else{"0"};
            content.push_str(&format!("seta studio_hud_{id} \"{value}\"\n"));
        }
        content.push_str(&format!("seta studio_layout_snap \"{}\"\n",u8::from(self.render_studio.hud_layout.snap_to_grid)));
        content.push_str(&format!("seta studio_layout_grid \"{}\"\n",self.render_studio.hud_layout.grid_size));
        for id in crate::ui::HudElementId::ALL {
            let name=format!("studio_layout_{}",id.cvar_name());
            let value=self.render_studio.hud_layout.element(id).to_config();
            content.push_str(&format!("seta {name} {}\n",serde_json::to_string(&value).map_err(|e|e.to_string())?));
        }
        let path = folder.join(&filename);
        let temporary = folder.join(format!("{filename}.tmp"));
        std::fs::write(&temporary, &content).map_err(|e| e.to_string())?;
        if std::fs::rename(&temporary, &path).is_err() {
            // Never delete the old profile before a replacement exists.
            std::fs::write(&path, &content).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_file(&temporary);
        }
        self.render_studio.graphics_saved = self.studio_graphics_snapshot().values;
        self.render_studio.hud_saved = self.render_studio.hud_settings.clone();
        self.render_studio.hud_layout_saved = self.render_studio.hud_layout;
        self.studio_cfg_refresh();
        Ok(())
    }

    /// Live Studio renderer values differ from the selected profile on disk.
    pub(in crate::app) fn studio_cfg_unsaved(&self) -> bool {
        self.render_studio.graphics_gameplay.is_some()
            && (self.studio_graphics_snapshot().values != self.render_studio.graphics_saved
                || self.render_studio.hud_settings != self.render_studio.hud_saved
                || self.render_studio.hud_layout != self.render_studio.hud_layout_saved)
    }

    fn studio_cfg_remember(&self) {
        let _ = std::fs::write(self.studio_cfg_folder().join("selected.txt"), &self.render_studio.graphics_profile);
    }

    /// Switch profile. Unsaved edits to the current profile are written first
    /// so switching never silently drops them.
    pub(in crate::app) fn studio_cfg_select(&mut self, name: &str) -> Result<(), String> {
        if self.render_studio.graphics_gameplay.is_none() {
            return Err("Render Studio must be open to load a profile".into());
        }
        let (values,hud,layout) = self.studio_cfg_read(name)?;
        if name != self.render_studio.graphics_profile && self.studio_cfg_unsaved() {
            self.studio_cfg_save()?;
        }
        let previous = self.studio_graphics_snapshot();
        if let Err(err) = self.studio_apply_graphics(&values) {
            let _ = self.studio_apply_graphics(&previous.values);
            return Err(err);
        }
        self.render_studio.hud_settings = hud;
        self.render_studio.hud_layout = layout;
        self.render_studio.hud_selected = None;
        self.render_studio.hud_saved = self.render_studio.hud_settings.clone();
        self.render_studio.hud_layout_saved = self.render_studio.hud_layout;
        self.render_studio.graphics_profile = name.to_owned();
        self.render_studio.graphics_confirm_delete = false;
        self.render_studio.graphics_saved = self.studio_graphics_snapshot().values;
        self.studio_cfg_remember();
        Ok(())
    }

    /// New profile from the live Studio values (duplicate current).
    pub(in crate::app) fn studio_cfg_create(&mut self, entered: &str) -> Result<(), String> {
        let filename = studio_cfg_filename(entered).ok_or("Use letters, numbers, _ or - for profile names")?;
        if self.studio_cfg_folder().join(&filename).exists() {
            return Err(format!("{filename} already exists"));
        }
        let previous = std::mem::replace(&mut self.render_studio.graphics_profile, filename);
        if let Err(error) = self.studio_cfg_save() {
            self.render_studio.graphics_profile = previous;
            return Err(error);
        }
        self.render_studio.graphics_confirm_delete = false;
        self.studio_cfg_remember();
        Ok(())
    }

    /// Reset the live Studio values to the gameplay renderer settings.
    pub(in crate::app) fn studio_cfg_match_gameplay(&mut self) -> Result<(), String> {
        let values = self.render_studio.graphics_gameplay.as_ref().map(|s| s.values.clone())
            .ok_or("Render Studio is not open")?;
        self.studio_apply_graphics(&values)
    }

    pub(in crate::app) fn studio_cfg_delete(&mut self) -> Result<(), String> {
        if self.render_studio.graphics_profiles.len() <= 1 {
            return Err("Keep at least one Render Studio profile".into());
        }
        let current = self.render_studio.graphics_profile.clone();
        if studio_cfg_filename(&current).as_deref() != Some(current.as_str()) { return Err("Invalid profile name".into()); }
        std::fs::remove_file(self.studio_cfg_folder().join(&current)).map_err(|e| e.to_string())?;
        // The deleted profile's live edits must not be auto-saved elsewhere.
        self.render_studio.graphics_saved = self.studio_graphics_snapshot().values;
        self.render_studio.hud_saved = self.render_studio.hud_settings.clone();
        self.render_studio.hud_layout_saved = self.render_studio.hud_layout;
        self.studio_cfg_refresh();
        let replacement = self.render_studio.graphics_profiles.first().cloned().ok_or("No available Studio profile")?;
        self.studio_cfg_select(&replacement)
    }

    /// Called on opening the workspace, before any Studio export or editor UI.
    pub(in crate::app) fn studio_cfg_enter(&mut self) {
        if self.render_studio.graphics_gameplay.is_some() { return; }
        // Persist gameplay changes *before* overriding the renderer; while in
        // the Studio the normal DinurdoJK.cfg save path is suspended.
        self.flush_config();
        self.render_studio.graphics_gameplay = Some(self.studio_graphics_snapshot());
        self.studio_cfg_refresh();
        if let Ok(last) = std::fs::read_to_string(self.studio_cfg_folder().join("selected.txt")) {
            let last = last.trim();
            if self.render_studio.graphics_profiles.iter().any(|p| p == last) {
                self.render_studio.graphics_profile = last.to_owned();
            }
        }
        if self.render_studio.graphics_profiles.is_empty() {
            // First use: the default profile starts as a copy of gameplay.
            self.render_studio.graphics_profile = DEFAULT_PROFILE.into();
            if let Err(error) = self.studio_cfg_save() {
                self.render_studio.status = format!("Studio graphics profile creation failed: {error}");
            }
            return;
        }
        if !self.render_studio.graphics_profiles.contains(&self.render_studio.graphics_profile) {
            self.render_studio.graphics_profile = self.render_studio.graphics_profiles[0].clone();
        }
        let name = self.render_studio.graphics_profile.clone();
        if let Err(error) = self.studio_cfg_select(&name) {
            self.render_studio.status = format!("Studio graphics profile failed to load: {error}");
        }
    }

    /// Restore the gameplay renderer values whenever the Studio is left.
    /// Unsaved profile edits are kept: they are written to the profile.
    pub(in crate::app) fn studio_cfg_exit(&mut self) {
        if self.render_studio.graphics_gameplay.is_none() { return; }
        if self.studio_cfg_unsaved() {
            if let Err(error) = self.studio_cfg_save() {
                eprintln!("Studio graphics profile auto-save failed: {error}");
            }
        }
        let Some(snapshot) = self.render_studio.graphics_gameplay.take() else { return };
        self.render_studio.graphics_settings_open = false;
        if let Err(error) = self.studio_apply_graphics(&snapshot.values) {
            eprintln!("Studio: restoring gameplay renderer settings failed: {error}");
        }
        // Restoring marks the config dirty; the save then rewrites the same
        // gameplay values, plus anything else changed while in the Studio.
        self.config_dirty |= snapshot.was_dirty;
    }

    /// Safety net for every route out of the Studio page (console, map load).
    pub(in crate::app) fn studio_cfg_check_exit(&mut self) {
        if self.render_studio.graphics_gameplay.is_some()
            && (!self.front_end || self.frontend_page != FrontendPage::RenderStudio) {
            self.studio_cfg_exit();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filenames_cannot_escape_profile_directory() {
        assert_eq!(studio_cfg_filename("cinematic"), Some("cinematic.cfg".into()));
        assert_eq!(studio_cfg_filename("studio_ultra.cfg"), Some("studio_ultra.cfg".into()));
        for name in ["../base", "test;quit", "a/b", "a\\b", "test.txt", "", "quoted\"name"] {
            assert!(studio_cfg_filename(name).is_none(), "{name}");
        }
    }
    #[test]
    fn renderer_filter_excludes_window_latched_and_gameplay() {
        assert!(studio_graphics_name("r_ocean"));
        assert!(studio_graphics_name("fx_countScale"));
        for name in ["r_fullscreen", "r_gpuAdapter", "r_backend", "r_windowX", "cg_fov", "rate"] {
            assert!(!studio_graphics_name(name), "{name}");
        }
        // Restart-latched renderer cvars never enter a profile.
        assert!(!studio_graphics_cvar("r_picmip"));
    }
    #[test]
    fn hud_profile_preset_and_overrides_are_bounded() {
        let clean=StudioHudSettings::preset("Clean");
        assert!(!clean.enabled("core"));
        let race=StudioHudSettings::preset("Race");
        assert!(race.enabled("race_timer"));
        assert!(!race.enabled("crosshair"));
        let mut overrides=BTreeMap::new();
        overrides.insert("race_timer".to_owned(),false);
        overrides.insert("crosshair".to_owned(),true);
        overrides.insert("cg_draw2D".to_owned(),true);
        let result=race.effective(&overrides);
        assert!(!result.enabled("race_timer"));
        assert!(result.enabled("crosshair"));
        assert!(!result.values.contains_key("cg_draw2D"));
    }
    #[test]
    fn profiles_parse_only_quoted_seta_lines() {
        let valid = |key: &str| (key == "r_ocean").then(|| key.to_owned());
        let parsed = parse_profile("// c\nseta r_ocean \"2\"\n", valid).unwrap();
        assert_eq!(parsed.get("r_ocean").map(String::as_str), Some("2"));
        assert!(parse_profile("set r_ocean \"2\"", valid).is_err());
        assert!(parse_profile("seta r_ocean 2; quit", valid).is_err());
        assert!(parse_profile("seta cg_fov \"90\"", valid).is_err());
        assert!(parse_profile("// only comments", valid).is_err());
    }
}
