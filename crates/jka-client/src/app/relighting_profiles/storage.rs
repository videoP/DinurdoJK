//! Shared client-owned storage, independent of fs_game. Each look is a portable
//! JSON file; the small defaults index refers to relative paths, not runtime IDs.
use super::*;
use std::{
    collections::BTreeSet,
    fs,
    io::{Read, Write},
    path::Path,
};

#[derive(Serialize, Deserialize)]
struct SavedProfile {
    version: u32,
    #[serde(flatten)]
    profile: Profile,
}

#[derive(Serialize, Deserialize)]
struct Defaults {
    version: u32,
    map_defaults: BTreeMap<String, String>,
}

// Percent-encode unsafe bytes instead of letting a map/profile name become an
// OS path. Keep ordinary names readable, including internal spaces. Reserve
// underscore-prefixed directories for global, JK2 and source-map namespaces.
fn component(name: &str) -> String {
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'));
    let mut result = String::new();
    for (index, byte) in name.bytes().enumerate() {
        let ordinary = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b' ');
        if ordinary
            && !(index == 0 && (device || byte == b'_'))
            && !(matches!(byte, b'.' | b' ') && index + 1 == name.len())
            && name != "."
            && name != ".."
        {
            result.push(byte as char);
        } else {
            result.push_str(&format!("%{byte:02X}"));
        }
    }
    if result.is_empty() {
        result.push_str("%00");
    }
    result
}

pub(super) fn relative_path(profile: &Profile) -> Result<PathBuf, String> {
    let mut path = PathBuf::new();
    if let Some(map) = &profile.map {
        let (protocol, map) = map.split_once(':').ok_or("Invalid profile map identity")?;
        match protocol {
            "jk2" => path.push("_jk2"),
            "jka" => {}
            _ => return Err("Invalid profile game identity".into()),
        }
        if map.ends_with(".map") {
            path.push("_source");
        }
        let map = map
            .strip_suffix(".bsp")
            .or_else(|| map.strip_suffix(".map"))
            .unwrap_or(map);
        for part in map.split('/').filter(|part| !part.is_empty()) {
            path.push(component(part));
        }
    } else {
        path.push("_global");
    }
    path.push(format!("{}.json", component(&profile.name)));
    Ok(path)
}

fn portable(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        portable(a).eq_ignore_ascii_case(&portable(b))
    } else {
        a == b
    }
}

fn read_bytes(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err(format!("{} exceeds the profile size limit", path.display()));
    }
    Ok(bytes)
}

fn collect_files(
    root: &Path,
    folder: &Path,
    depth: usize,
    paths: &mut Vec<PathBuf>,
    visited: &mut usize,
) -> Result<(), String> {
    if depth > 16 {
        return Err("Relighting profile folders are nested too deeply".into());
    }
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && folder == root => return Ok(()),
        Err(e) => return Err(format!("{}: {e}", folder.display())),
    };
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        *visited += 1;
        if *visited > 4096 {
            return Err("Too many entries in light_profiles".into());
        }
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        let path = entry.path();
        if kind.is_dir() {
            collect_files(root, &path, depth + 1, paths, visited)?;
        } else if kind.is_file()
            && path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("json"))
            && path != root.join("_defaults.json")
        {
            paths.push(path);
            if paths.len() > MAX_PROFILES {
                return Err("Maximum of 256 relighting profiles reached".into());
            }
        }
    }
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("Missing relighting profile folder")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temporary = path.with_extension("json.tmp");
    let result = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|e| format!("Couldn't save {}: {e}", path.display()))
}

impl Library {
    pub(super) fn read(root: &Path) -> Result<Self, String> {
        let mut paths = Vec::new();
        let mut visited = 0;
        collect_files(root, root, 0, &mut paths, &mut visited)?;
        paths.sort();
        let mut library = Self::default();
        let mut total_bytes = 0;
        let mut by_path = BTreeMap::new();
        for (index, path) in paths.into_iter().enumerate() {
            let bytes = read_bytes(&path, 128 * 1024)?;
            total_bytes += bytes.len() as u64;
            if total_bytes > MAX_FILE_BYTES {
                return Err("Relighting profiles exceed 4 MiB".into());
            }
            let saved: SavedProfile =
                serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
            if saved.version != 1 {
                return Err(format!("Unsupported profile version in {}", path.display()));
            }
            let id = index as u64 + 1;
            let relative = path
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .to_path_buf();
            by_path.insert(portable(&relative), id);
            library.files.insert(id, relative);
            library.profiles.insert(id, saved.profile);
        }
        let index = root.join("_defaults.json");
        if index.try_exists().map_err(|e| e.to_string())? {
            let defaults: Defaults = serde_json::from_slice(&read_bytes(&index, 256 * 1024)?)
                .map_err(|e| format!("{}: {e}", index.display()))?;
            if defaults.version != 1 {
                return Err("Unsupported relighting map-default version".into());
            }
            // Removing a profile file manually simply drops its stale default.
            library.map_defaults = defaults
                .map_defaults
                .into_iter()
                .filter_map(|(map, path)| by_path.get(&path).copied().map(|id| (map, id)))
                .collect();
        }
        library.validate()?;
        Ok(library)
    }

    pub(super) fn write(&mut self, root: &Path) -> Result<(), String> {
        self.validate()?;
        let mut files = BTreeMap::new();
        let mut writes = Vec::new();
        let mut unique = BTreeSet::new();
        let mut total_bytes = 0;
        // Validate every destination and serialize before touching the disk.
        for (id, profile) in &self.profiles {
            let relative = relative_path(profile)?;
            if !unique.insert(portable(&relative).to_ascii_lowercase()) {
                return Err("Two profiles have the same map and name".into());
            }
            let path = root.join(&relative);
            if path.try_exists().map_err(|e| e.to_string())?
                && !self.files.values().any(|old| same_path(old, &relative))
            {
                return Err(format!(
                    "{} already exists; reopen the game to refresh profiles",
                    path.display()
                ));
            }
            let bytes = serde_json::to_vec_pretty(&SavedProfile {
                version: self.version,
                profile: profile.clone(),
            })
            .map_err(|e| e.to_string())?;
            total_bytes += bytes.len() as u64;
            files.insert(*id, relative);
            writes.push((path, bytes));
        }
        if total_bytes > MAX_FILE_BYTES {
            return Err("Relighting profiles exceed 4 MiB".into());
        }
        let defaults = Defaults {
            version: self.version,
            map_defaults: self
                .map_defaults
                .iter()
                .map(|(map, id)| (map.clone(), portable(&files[id])))
                .collect(),
        };
        let index = serde_json::to_vec_pretty(&defaults).map_err(|e| e.to_string())?;
        if index.len() > 256 * 1024 {
            return Err("Relighting map defaults exceed 256 KiB".into());
        }
        for (path, bytes) in writes {
            if fs::read(&path).ok().as_deref() != Some(bytes.as_slice()) {
                write_atomic(&path, &bytes)?;
            }
        }
        write_atomic(&root.join("_defaults.json"), &index)?;
        // Remove only files this catalog actually owned, after the new default
        // references are durable. Never remove untracked/imported user files.
        for old in self
            .files
            .values()
            .filter(|old| !files.values().any(|path| same_path(path, old)))
        {
            match fs::remove_file(root.join(old)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(format!(
                        "Couldn't remove old profile {}: {e}",
                        old.display()
                    ))
                }
            }
        }
        self.files = files;
        Ok(())
    }
}
