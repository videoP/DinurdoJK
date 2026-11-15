//! Named lighting snapshots and optional map defaults. Profiles are data, never
//! console scripts. Active profiles overlay normal lighting; ordinary config
//! saves and the next map recover the baseline without changing other settings.
use super::{App, RenderCommand, VideoSettings};
use crate::config::{self, cvars::ArchiveState};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};
mod storage;

#[cfg(test)]
mod tests;

type Values = BTreeMap<String, String>;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_PROFILES: usize = 256;

// Explicit ownership avoids silently acquiring renderer/debug settings when a
// new cvar is added. Sky/cloud appearance uses its archived, live video cvars.
const LIGHTING_CVARS: &[&str] = &[
    "r_relightMap",
    "r_environmentMode",
    "r_skySunCustom",
    "r_sunTimeLocks",
    "r_relightTime",
    "r_relightMatchSystemTime",
    "r_sunYaw",
    "r_sunPitch",
    "r_sunIntensity",
    "r_sunColor",
    "r_sunVisibility",
    "r_sunLighting",
    "r_entitySunLighting",
    "r_sunShadowStrength",
    "r_dynamicShadows",
    "r_staticWorldCsm",
    "r_entityAmbientLighting",
    "r_entityShadowLight",
    "r_bakedSunRemoval",
    "r_bakedSunRemovalAuto",
    "r_bakedSunRemovalContour",
    "r_bakedSunCache",
    "r_bakedSunAA",
    "r_bakedSunShadowMap",
    "r_bakedSunEdgeSoftness",
    "r_bakedSunDirectLimit",
    "r_bakedIndirectScale",
    "r_bakedIndirectSaturation",
    "r_bakedIndirectPower",
    "r_bakedIndirectHighlightPreserve",
    "r_skyWorldGainMode",
    "r_skyWorldLightIsolation",
    "r_promoteShaderGlow",
    "r_promoteShaderAdditive",
    "r_promoteShaderEmissive",
    "r_emissiveAreaLights",
    "r_bakedLocalGlow",
    "r_bakedLocalAdditive",
    "r_bakedLocalEmissive",
    "r_bakedLocalSurfacelight",
    "r_bakedLocalFx",
    "r_bakedLocalFlares",
    "r_skyBackend",
    "r_fastsky",
    "r_skyFractal",
];

fn profile_cvar(name: &str) -> bool {
    let Some(def) = super::cvar_definition(name) else {
        return false;
    };
    profile_definition(def)
}

fn profile_definition(def: &super::commands::registry::CvarDefinition) -> bool {
    let name = def.runtime.entry.name;
    (LIGHTING_CVARS.contains(&name)
        || name.starts_with("r_cloud")
        || name.starts_with("r_blueSky")
        || name == "r_blueCloudQuality")
        && name != "r_blueSkyCubeAB"
        && def.runtime.entry.latch == crate::console::LatchScope::None
        && def.config.archive.is_some()
        && def.config.load_video.is_some()
}

fn capture(video: &VideoSettings) -> Values {
    let presentation = config::ClientPresentationSettings::default();
    let audio = config::AudioSettings::default();
    let state = ArchiveState {
        video,
        presentation: &presentation,
        audio: &audio,
    };
    super::cvar_definitions()
        .iter()
        .filter(|def| profile_definition(def))
        .map(|def| {
            (
                def.runtime.entry.name.to_owned(),
                (def.config.archive.unwrap().1)(&state),
            )
        })
        .collect()
}

/// Copy just owned fields, through their existing config adapters. This is also
/// used for the archive overlay, so unrelated settings keep their live values.
fn copy_lighting(target: &mut VideoSettings, source: &VideoSettings) {
    let mut context = config::cvars::VideoLoadState::default();
    for (name, value) in capture(source) {
        super::cvar_definition(&name)
            .unwrap()
            .load_video(target, &value, &mut context);
    }
    target.cascaded_shadows = target.dynamic_shadows.is_csm();
}

fn values_match(current: &Values, saved: &Values) -> bool {
    let clock = current
        .get("r_relightMatchSystemTime")
        .is_some_and(|v| v == "1")
        && saved
            .get("r_relightMatchSystemTime")
            .is_some_and(|v| v == "1");
    // A clock update is not an edit only for axes still controlled by time.
    // Absolute manual overrides must remain detectable as unsaved changes.
    let locks = saved.get("r_sunTimeLocks")
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(7);
    current.len() == saved.len()
        && saved.iter().all(|(name, value)| {
            (clock && match name.as_str() {
                "r_sunYaw" => locks & VideoSettings::SUN_LOCK_YAW != 0,
                "r_sunPitch" => locks & VideoSettings::SUN_LOCK_PITCH != 0,
                "r_sunIntensity" => locks & VideoSettings::SUN_LOCK_INTENSITY != 0,
                "r_blueSkyMoonElevation" | "r_blueSkyMoonAzimuth" | "r_blueSkyMoonCustom" => true,
                _ => false,
            }) || current.get(name) == Some(value)
        })
}

fn map_key(name: &str, jk2: bool) -> String {
    let name = name.trim().replace('\\', "/").to_ascii_lowercase();
    let name = name.strip_prefix("maps/").unwrap_or(&name);
    let name = if name.ends_with(".bsp") || name.ends_with(".map") {
        name.to_owned()
    } else {
        format!("{name}.bsp")
    };
    format!("{}:{name}", if jk2 { "jk2" } else { "jka" })
}

#[derive(Clone, Serialize, Deserialize)]
pub(in crate::app) struct Profile {
    pub name: String,
    pub map: Option<String>,
    values: Values,
}

#[derive(Clone)]
pub(in crate::app) struct Library {
    version: u32,
    pub profiles: BTreeMap<u64, Profile>,
    pub map_defaults: BTreeMap<String, u64>,
    files: BTreeMap<u64, PathBuf>,
}

impl Default for Library {
    fn default() -> Self {
        Self {
            version: 1,
            profiles: BTreeMap::new(),
            map_defaults: BTreeMap::new(),
            files: BTreeMap::new(),
        }
    }
}

impl Library {
    fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err(format!(
                "Unsupported relighting profile version {}",
                self.version
            ));
        }
        if self.profiles.len() > MAX_PROFILES || self.map_defaults.len() > 2048 {
            return Err("Relighting profile library is too large".into());
        }
        for profile in self.profiles.values() {
            validate_name(&profile.name)?;
            storage::relative_path(profile)?;
            if profile.map.as_ref().is_some_and(|map| map.len() > 1024) {
                return Err("Profile map name is too long".into());
            }
            validate_values(&profile.values)?;
        }
        if self
            .map_defaults
            .iter()
            .any(|(map, id)| map.len() > 1024 || !self.profiles.contains_key(id))
        {
            return Err("Invalid map default in relighting profile library".into());
        }
        Ok(())
    }

    fn check_unique_name(
        &self,
        name: &str,
        map: Option<&str>,
        except: Option<u64>,
    ) -> Result<(), String> {
        validate_name(name)?;
        if self.profiles.iter().any(|(id, p)| {
            Some(*id) != except && p.map.as_deref() == map && p.name.eq_ignore_ascii_case(name)
        }) {
            return Err("A relighting profile for this map already has that name".into());
        }
        Ok(())
    }

    fn remove(&mut self, id: u64) {
        self.profiles.remove(&id);
        self.map_defaults.retain(|_, profile| *profile != id);
    }
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() || name.len() > 80 || name.chars().any(char::is_control) {
        Err("Use a profile name of 1–80 bytes without control characters".into())
    } else {
        Ok(())
    }
}

fn validate_values(values: &Values) -> Result<(), String> {
    if values.is_empty() || values.len() > 256 {
        return Err("Invalid number of lighting settings".into());
    }
    for (name, value) in values {
        if !profile_cvar(name)
            || value.len() > 256
            || super::cvar_definition(name).is_some_and(|def| def.runtime.entry.name != name)
        {
            return Err(format!(
                "{name} is not a supported relighting profile setting"
            ));
        }
        super::cvar_definition(name).unwrap().runtime.parse(value)?;
    }
    Ok(())
}

#[derive(Default)]
pub(in crate::app) struct RelightingProfiles {
    pub library: Option<Library>,
    pub read_error: Option<String>,
    pub selected: Option<u64>,
    pub active: Option<u64>,
    loaded_values: Option<Values>,
    baseline: Option<Baseline>,
    pub map: Option<String>,
    pub reload_request: Option<u64>,
    pub name_input: String,
    pub delete_pending: bool,
    pub status: String,
}

#[derive(Clone, Copy)]
struct Baseline {
    video: VideoSettings,
    sun_initialized: bool,
    clock: Option<f32>,
}

impl App {
    fn relighting_library_path(&self) -> PathBuf {
        jka_assets::pk3::dinurdojk_directory_for(&self.base, &self.fs_roots).join("light_profiles")
    }

    pub(in crate::app) fn ensure_relighting_profiles(&mut self) {
        if self.relighting_profiles.library.is_some()
            || self.relighting_profiles.read_error.is_some()
        {
            return;
        }
        match Library::read(&self.relighting_library_path()) {
            Ok(library) => self.relighting_profiles.library = Some(library),
            Err(error) => {
                self.push_console_line(format!("^3RELIGHTING PROFILES:^7 {error}"));
                self.relighting_profiles.read_error = Some(error);
            }
        }
    }

    fn commit_relighting_library(&mut self, mut library: Library) -> Result<(), String> {
        library.write(&self.relighting_library_path())?;
        self.relighting_profiles.library = Some(library);
        Ok(())
    }

    pub(in crate::app) fn relighting_profile_modified(&self) -> bool {
        self.relighting_profiles
            .loaded_values
            .as_ref()
            .is_some_and(|saved| !values_match(&capture(&self.video), saved))
    }

    /// Mode/time setters have deliberate side effects (map seeding, CSM default,
    /// sun/moon arc). Apply them first, then restore explicit custom positions
    /// and shadow choices. Clock matching goes last.
    fn set_relighting_values(&mut self, values: &Values) -> Result<(), String> {
        self.set_console_cvar("r_relightMatchSystemTime", "0")?;
        for name in ["r_relightMap", "r_environmentMode", "r_relightTime"] {
            if let Some(value) = values.get(name) {
                self.set_console_cvar(name, value)?;
            }
        }
        for (name, value) in values {
            if !matches!(
                name.as_str(),
                "r_relightMap" | "r_environmentMode" | "r_relightTime" | "r_relightMatchSystemTime"
                    | "r_sunTimeLocks" | "r_skySunCustom"
            ) {
                self.set_console_cvar(name, value)?;
            }
        }
        // Manual sun setters intentionally unlock their own axis. Reapply the
        // saved time-lock mask afterward so absolute overrides and automatic
        // axes both survive profile load. skySunCustom goes last because the
        // time/lock setters enable the visual sun while editing.
        for name in ["r_sunTimeLocks", "r_skySunCustom", "r_relightMatchSystemTime"] {
            if let Some(value) = values.get(name) {
                self.set_console_cvar(name, value)?;
            }
        }
        Ok(())
    }

    fn apply_relighting_values(&mut self, values: &Values) -> Result<(), String> {
        validate_values(values)?;
        let previous = self.video;
        let clock = self.relight_clock_effective;
        let initialized = self.sun_custom_initialized;
        let rollback = capture(&previous);
        self.begin_settings_batch("Relighting profile");
        self.render_command(RenderCommand::SetRelightBakeDeferred(true));
        let result = self.set_relighting_values(values);
        if result.is_err() {
            // Custom text setters validate beyond CvarType::String. Roll back
            // the entire supported snapshot if any one rejects a saved value.
            if let Err(error) = self.set_relighting_values(&rollback) {
                eprintln!("Relighting profile rollback failed: {error}");
            }
            self.video = previous;
            self.relight_clock_effective = clock;
            self.sun_custom_initialized = initialized;
            self.sync_post_effects();
            self.sync_cascaded_shadows();
        }
        self.sun_ray_until = None;
        self.baked_sun_debug_view = 0;
        self.sync_sun_ray();
        self.render_command(RenderCommand::SetRelightBakeDeferred(
            self.relight_bake_deferred,
        ));
        self.end_settings_batch();
        result
    }

    pub(in crate::app) fn load_relighting_profile(&mut self, id: u64) -> Result<(), String> {
        if self.render_studio.graphics_gameplay.is_some() {
            return Err("Leave Render Studio before loading a gameplay relighting profile".into());
        }
        self.ensure_relighting_profiles();
        let profile = self
            .relighting_profiles
            .library
            .as_ref()
            .and_then(|l| l.profiles.get(&id))
            .ok_or("Relighting profile not found")?
            .clone();
        let before = Baseline {
            video: self.video,
            sun_initialized: self.sun_custom_initialized,
            clock: self.relight_clock_effective,
        };
        // An older snapshot may lack a newly introduced control. Resolve it
        // against normal lighting rather than inheriting another profile.
        let mut values = capture(
            &self
                .relighting_profiles
                .baseline
                .map_or(before.video, |b| b.video),
        );
        values.extend(profile.values);
        self.apply_relighting_values(&values)?;
        self.relighting_profiles.baseline.get_or_insert(before);
        self.relighting_profiles.active = Some(id);
        self.relighting_profiles.selected = Some(id);
        self.relighting_profiles.loaded_values = Some(capture(&self.video));
        self.relighting_profiles.name_input = profile.name.clone();
        self.relighting_profiles.delete_pending = false;
        self.relighting_profiles.status = format!("Loaded {}", profile.name);
        Ok(())
    }

    pub(in crate::app) fn save_relighting_profile(&mut self, save_as: bool) -> Result<(), String> {
        self.ensure_relighting_profiles();
        let mut library = self
            .relighting_profiles
            .library
            .clone()
            .ok_or("Profile library is unavailable")?;
        let values = capture(&self.video);
        let (id, name) = if save_as {
            let name = self.relighting_profiles.name_input.trim().to_owned();
            library.check_unique_name(&name, self.relighting_profiles.map.as_deref(), None)?;
            if library.profiles.len() >= MAX_PROFILES {
                return Err("Maximum of 256 relighting profiles reached".into());
            }
            let id = library
                .profiles
                .keys()
                .next_back()
                .copied()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or("No profile IDs available")?;
            library.profiles.insert(
                id,
                Profile {
                    name: name.clone(),
                    map: self.relighting_profiles.map.clone(),
                    values: values.clone(),
                },
            );
            (id, name)
        } else {
            let id = self
                .relighting_profiles
                .active
                .ok_or("Load a profile before saving it")?;
            let profile = library
                .profiles
                .get_mut(&id)
                .ok_or("Relighting profile not found")?;
            profile.values = values.clone();
            (id, profile.name.clone())
        };
        self.commit_relighting_library(library)?;
        self.relighting_profiles.baseline.get_or_insert(Baseline {
            video: self.video,
            sun_initialized: self.sun_custom_initialized,
            clock: self.relight_clock_effective,
        });
        self.relighting_profiles.active = Some(id);
        self.relighting_profiles.selected = Some(id);
        self.relighting_profiles.loaded_values = Some(values);
        self.relighting_profiles.name_input = name.clone();
        self.relighting_profiles.status = format!("Saved {name}");
        self.mark_config_dirty();
        Ok(())
    }

    pub(in crate::app) fn rename_relighting_profile(&mut self, id: u64) -> Result<(), String> {
        let mut library = self
            .relighting_profiles
            .library
            .clone()
            .ok_or("Profile library is unavailable")?;
        let name = self.relighting_profiles.name_input.trim().to_owned();
        let map = library
            .profiles
            .get(&id)
            .ok_or("Relighting profile not found")?
            .map
            .clone();
        library.check_unique_name(&name, map.as_deref(), Some(id))?;
        library
            .profiles
            .get_mut(&id)
            .ok_or("Relighting profile not found")?
            .name = name.clone();
        self.commit_relighting_library(library)?;
        self.relighting_profiles.status = format!("Renamed profile to {name}");
        Ok(())
    }

    pub(in crate::app) fn delete_relighting_profile(&mut self, id: u64) -> Result<(), String> {
        let mut library = self
            .relighting_profiles
            .library
            .clone()
            .ok_or("Profile library is unavailable")?;
        library.remove(id);
        self.commit_relighting_library(library)?;
        self.relighting_profiles.selected = None;
        if self.relighting_profiles.active == Some(id) {
            // Keep the current look and baseline until explicitly restored or
            // the next map; deleting a file should not change the scene.
            self.relighting_profiles.active = None;
            self.relighting_profiles.loaded_values = None;
        }
        self.relighting_profiles.delete_pending = false;
        self.relighting_profiles.status =
            "Deleted profile and its map defaults; current lighting retained".into();
        Ok(())
    }

    pub(in crate::app) fn set_relighting_map_default(
        &mut self,
        id: Option<u64>,
    ) -> Result<(), String> {
        let map = self
            .relighting_profiles
            .map
            .clone()
            .ok_or("Load a map before assigning a default")?;
        let mut library = self
            .relighting_profiles
            .library
            .clone()
            .ok_or("Profile library is unavailable")?;
        if let Some(id) = id {
            if !library.profiles.contains_key(&id) {
                return Err("Relighting profile not found".into());
            }
            library.map_defaults.insert(map, id);
        } else {
            library.map_defaults.remove(&map);
        }
        self.commit_relighting_library(library)?;
        self.relighting_profiles.status = if id.is_some() {
            "Default saved; it will load automatically on the next visit to this map".into()
        } else {
            "Map default cleared".into()
        };
        Ok(())
    }

    pub(in crate::app) fn restore_normal_relighting(&mut self) -> Result<(), String> {
        if let Some(baseline) = self.relighting_profiles.baseline {
            self.apply_relighting_values(&capture(&baseline.video))?;
            self.sun_custom_initialized = baseline.sun_initialized;
            self.relight_clock_effective = baseline.clock;
            self.relighting_profiles.baseline = None;
        }
        self.relighting_profiles.active = None;
        self.relighting_profiles.loaded_values = None;
        self.relighting_profiles.status = "Restored normal lighting".into();
        Ok(())
    }

    pub(in crate::app) fn has_relighting_overlay(&self) -> bool {
        self.relighting_profiles.baseline.is_some()
    }

    pub(in crate::app) fn archive_normal_relighting(&self, settings: &mut VideoSettings) {
        if let Some(baseline) = &self.relighting_profiles.baseline {
            copy_lighting(settings, &baseline.video);
        }
    }

    pub(in crate::app) fn relighting_profiles_disconnected(&mut self) {
        if self.render_studio.graphics_gameplay.is_some() {
            return;
        }
        if let Err(error) = self.restore_normal_relighting() {
            self.push_console_line(format!("^1RELIGHTING PROFILES:^7 {error}"));
        }
        self.relighting_profiles.map = None;
        self.relighting_profiles.reload_request = None;
    }

    pub(in crate::app) fn relighting_profile_map_loaded(&mut self, name: &str, request_id: u64) {
        let key = map_key(
            name,
            self.live_input.is_jk2() || self.fs_roots.active_jk2.is_some(),
        );
        // Renderer restarts and same-map remap uploads retain unsaved edits.
        if self.relighting_profiles.map.as_deref() == Some(&key)
            && self.relighting_profiles.reload_request != Some(request_id)
        {
            return;
        }
        if let Err(error) = self.restore_normal_relighting() {
            self.push_console_line(format!("^1RELIGHTING PROFILES:^7 {error}"));
            return;
        }
        self.relighting_profiles.map = Some(key.clone());
        self.relighting_profiles.reload_request = None;
        self.ensure_relighting_profiles();
        let default = self
            .relighting_profiles
            .library
            .as_ref()
            .and_then(|l| l.map_defaults.get(&key))
            .copied();
        if let Some(id) = default {
            if let Err(error) = self.load_relighting_profile(id) {
                self.relighting_profiles.status = format!("Map profile failed to load: {error}");
                self.push_console_line(format!(
                    "^1RELIGHTING PROFILES:^7 {}",
                    self.relighting_profiles.status
                ));
            }
        }
    }
}
