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

pub(super) fn scan_folders(root: Option<&Path>) -> Result<Vec<Folder>, String> {
    let root = root.ok_or("Set fs_jk2path to your JK2 GameData directory to browse recordings")?;
    if !root.is_dir() { return Err(format!("JK2 GameData directory does not exist: {}", root.display())); }
    let mut folders = Vec::new();
    let dirs = fs::read_dir(root).map_err(|err| format!("JK2 demo folder scan: {err}"))?;
    for game_dir in dirs.flatten() {
        if !game_dir.file_type().is_ok_and(|kind| kind.is_dir()) { continue; }
        let name = game_dir.file_name().to_string_lossy().into_owned();
        let path = game_dir.path().join("demos");
        if has_demos(&path) {
            folders.push(Folder { label: format!("{name}/demos"), path });
        }
    }
    folders.sort_by_key(|f| (!f.label.to_ascii_lowercase().starts_with("base/"), f.label.to_ascii_lowercase()));
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
    fn scanner_only_uses_standard_demos_folders() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("dinurdo-jk2-demos-{}-{nonce}", std::process::id()));
        let base = root.join("base");
        let modded = root.join("custommod");
        fs::create_dir_all(base.join("demos")).unwrap();
        fs::create_dir_all(base.join("jk2demos")).unwrap();
        fs::create_dir_all(modded.join("demos")).unwrap();
        fs::write(base.join("demos").join("official.dm_15"), b"demo").unwrap();
        fs::write(base.join("jk2demos").join("nonstandard.dm_16"), b"demo").unwrap();
        fs::write(modded.join("demos").join("modded.dm_16"), b"demo").unwrap();
        let result = scan_folders(Some(&root)).unwrap();
        assert_eq!(result.iter().map(|folder| folder.label.as_str()).collect::<Vec<_>>(),
            vec!["base/demos", "custommod/demos"]);
        assert!(result.iter().all(|folder| folder.path.file_name().and_then(|name| name.to_str()) == Some("demos")));
        assert_eq!(scan_recordings(&base.join("demos")).unwrap().len(), 1);
        // No process restart is necessary: the next scan reads the current
        // directory contents rather than reusing the previous list.
        fs::write(base.join("demos").join("new.dm_16"), b"demo").unwrap();
        assert_eq!(scan_recordings(&base.join("demos")).unwrap().len(), 2);
        fs::create_dir_all(root.join("newmod").join("demos")).unwrap();
        fs::write(root.join("newmod").join("demos").join("newmod.dm_15"), b"demo").unwrap();
        assert_eq!(scan_folders(Some(&root)).unwrap().len(), 3);
        fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    fn reset_invalidates_pending_results() {
        let mut b=Browser::default();
        b.loaded=true;b.pending=true;b.generation=2;
        b.reset();
        assert!(!b.loaded && !b.pending && b.generation==3);
    }
}
