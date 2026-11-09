//! JK2-only Play -> Demo archive, isolated from the JKA VFS and Render Studio
//! demo catalog. Disk scans and full decoder inspection run on UiCatalog's worker.
use std::{collections::BTreeMap, fs::{self, File}, io::BufReader, path::{Path, PathBuf}};

#[derive(Debug, Clone)]
pub(super) struct Folder {
    pub label: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub(super) struct Recording {
    pub filename: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub(super) struct Details {
    pub protocol: String,
    pub map: String,
    pub server: String,
    pub mod_name: String,
    pub duration_ms: i64,
    pub snapshots: u64,
    pub dropped: u64,
    pub segments: usize,
    pub records: u64,
    pub bytes: u64,
}

#[derive(Debug, Default)]
pub(super) struct Browser {
    pub folders: Vec<Folder>,
    pub folder: Option<PathBuf>,
    pub entries: Vec<Recording>,
    pub selected: usize,
    pub list_scroll: usize,
    pub detail_scroll: usize,
    pub folder_open: bool,
    pub folder_scroll: usize,
    pub inspector_tab: usize,
    pub loaded: bool,
    pub pending: bool,
    pub generation: u64,
    pub error: Option<String>,
    pub details: BTreeMap<PathBuf, Result<Details, String>>,
    pub details_pending: Option<PathBuf>,
}

impl Browser {
    pub fn reset(&mut self) {
        self.loaded = false;
        self.pending = false;
        self.generation = self.generation.wrapping_add(1);
        self.error = None;
        self.entries.clear();
        self.selected = 0;
        self.list_scroll = 0;
        self.detail_scroll = 0;
        self.folder_scroll = 0;
        self.folder_open = false;
        self.details.clear();
        self.details_pending = None;
    }
}

fn is_jk2_demo(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| {
        ext.eq_ignore_ascii_case("dm_15") || ext.eq_ignore_ascii_case("dm_16")
    })
}

fn has_demos(folder: &Path) -> bool {
    fs::read_dir(folder).is_ok_and(|entries| entries.flatten().any(|entry| {
        entry.file_type().is_ok_and(|ty| ty.is_file()) && is_jk2_demo(&entry.path())
    }))
}

/// Browse JK2 base/ and its unprefixed sibling mod folders.
/// CD fallback is read-only stock assets; demos there are still discoverable.
pub(super) fn scan_folders(
    local_base: &Path, home_base: Option<&Path>, retail_root: Option<&Path>,
) -> Result<Vec<Folder>, String> {
    let mut folders = Vec::new();
    for (root, suffix) in [(Some(local_base), ""), (home_base, " (home)")] {
        let Some(root) = root else { continue };
        if folders.iter().any(|f: &Folder| f.path == root.join("demos")) { continue; }
        let base_name = root.file_name().map_or_else(|| "base".to_owned(), |n| n.to_string_lossy().into_owned());
        let local_demos = root.join("demos");
        if has_demos(&local_demos) {
            folders.push(Folder { label: format!("{base_name}/demos{suffix}"), path: local_demos });
        }
        if let Some(parent) = root.parent() {
            for mod_dir in fs::read_dir(parent).into_iter().flatten().flatten() {
                if !mod_dir.file_type().is_ok_and(|kind| kind.is_dir()) { continue; }
                let name = mod_dir.file_name().to_string_lossy().into_owned();
                if name.eq_ignore_ascii_case("base") || mod_dir.path() == root { continue; }
                let path = mod_dir.path().join("demos");
                if has_demos(&path) && !folders.iter().any(|item| item.path == path) {
                    folders.push(Folder { label: format!("{name}/demos{suffix}"), path });
                }
            }
        }
    }
    // Optional read-only JK2 retail GameData remains a separate source.
    if let Some(root) = retail_root {
        if !root.is_dir() && !local_base.is_dir() && !home_base.is_some_and(Path::is_dir) {
            return Err(format!("JK2 retail directory does not exist: {}", root.display()));
        }
        if let Ok(dirs) = fs::read_dir(root) {
            for game_dir in dirs.flatten() {
                if !game_dir.file_type().is_ok_and(|kind| kind.is_dir()) { continue; }
                let name = game_dir.file_name().to_string_lossy().into_owned();
                let path = game_dir.path().join("demos");
                if has_demos(&path) && !folders.iter().any(|item| item.path == path) {
                    folders.push(Folder { label: format!("{name}/demos"), path });
                }
            }
        }
    }
    folders.sort_by_key(|f| (
        !f.label.eq_ignore_ascii_case("base/demos"),
        !f.label.eq_ignore_ascii_case("base/demos (home)"),
        !f.label.eq_ignore_ascii_case("base/demos"),
        f.label.to_ascii_lowercase(),
    ));
    Ok(folders)
}

pub(super) fn scan_recordings(folder: &Path) -> Result<Vec<Recording>, String> {
    let mut demos = Vec::new();
    for entry in fs::read_dir(folder).map_err(|err| format!("{}: {err}", folder.display()))? {
        let entry = entry.map_err(|err| format!("JK2 demo folder: {err}"))?;
        if !entry.file_type().is_ok_and(|ty| ty.is_file()) || !is_jk2_demo(&entry.path()) { continue; }
        demos.push(Recording { filename: entry.file_name().to_string_lossy().into_owned(), path: entry.path() });
    }
    demos.sort_by_key(|item| item.filename.to_ascii_lowercase());
    Ok(demos)
}

pub(super) fn inspect_recording(path: &Path) -> Result<Details, String> {
    let file = File::open(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let summary = jka_protocol::jk2::demo::inspect(BufReader::new(file))
        .map_err(|err| format!("JK2 demo inspection: {err}"))?;
    let first = summary.segments.first();
    let protocol = first.map(|s| format!("JK2 {} / {}", s.version.name(), s.version.protocol()))
        .unwrap_or_else(|| "JK2 / unknown protocol".to_owned());
    Ok(Details {
        protocol,
        map: first.map(|s| s.map.clone()).unwrap_or_default(),
        server: first.map(|s| s.hostname.clone()).unwrap_or_default(),
        mod_name: first.map(|s| s.game.clone()).unwrap_or_default(),
        duration_ms: summary.segments.iter().map(|s| s.duration_ms().max(0)).sum(),
        snapshots: summary.segments.iter().map(|s| s.snapshots).sum(),
        dropped: summary.segments.iter().map(|s| s.dropped_snapshots).sum(),
        segments: summary.segments.len(),
        records: summary.records,
        bytes: fs::metadata(path).map(|meta| meta.len()).unwrap_or(summary.payload_bytes),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn jk2_extensions_only() {
        assert!(is_jk2_demo(Path::new("one.dm_15")));
        assert!(is_jk2_demo(Path::new("two.DM_16")));
        assert!(!is_jk2_demo(Path::new("three.dm_26")));
    }
    #[test]
    fn scanner_uses_unprefixed_mod_names_and_distinct_roots() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let install = std::env::temp_dir().join(format!("dinurdo-jk2-demos-{}-{nonce}", std::process::id()));
        let local = install.join("JK2 GameData");
        let cd = install.join("retail-jk2");
        let home = install.join("home/JK2 GameData");
        for folder in [local.join("base/demos"), local.join("jk2/demos"),
            home.join("base/demos"), home.join("jka_mod/demos"), cd.join("base/demos"),
            install.join("jka/jk2/demos")] {
            fs::create_dir_all(&folder).unwrap();
            fs::write(folder.join("recording.dm_15"), b"demo").unwrap();
        }
        let folders = scan_folders(&local.join("base"), Some(&home.join("base")), Some(&cd)).unwrap();
        assert!(folders.iter().any(|folder| folder.label == "base/demos"));
        assert!(folders.iter().any(|folder| folder.label == "jk2/demos"));
        assert!(folders.iter().any(|folder| folder.label == "jka_mod/demos (home)"));
        assert!(folders.iter().all(|folder| !folder.path.starts_with(install.join("jka"))));
        assert_eq!(scan_recordings(&local.join("base/demos")).unwrap().len(), 1);
        fs::remove_dir_all(install).unwrap();
    }
    #[test]
    fn reset_invalidates_pending_results() {
        let mut b=Browser::default();
        b.loaded=true;b.pending=true;b.generation=2;
        b.reset();
        assert!(!b.loaded && !b.pending && b.generation==3);
    }
}
