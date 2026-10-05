//! Read-only JKA virtual asset search path with PK3, Rend2 companion ZIP, and loose-file support.
//!
//! Search order follows the useful core of JKA/OpenJK semantics for this client:
//! the active game/mod directory is searched before base, and within each game
//! directory PK3 files take precedence over loose files. Later-sorting PK3 names
//! override earlier ones. Every caller uses package-relative qpaths such as
//! `maps/mp/ffa3.bsp`, `textures/foo/bar.tga`, or `shaders/common.shader`.
use std::{
    collections::HashMap,
    error::Error,
    ffi::OsString,
    fs::{self, File},
    hash::{DefaultHasher, Hash, Hasher},
    io::Read,
    path::{Path, PathBuf},
    sync::{
        atomic::AtomicUsize,
        Arc, Mutex, PoisonError,
    },
    time::{Duration, SystemTime},
};

#[derive(Debug)]
pub struct Asset {
    pub bytes: Vec<u8>,
    /// The physical file that supplied this asset: either a `.pk3` archive or a
    /// loose file beneath the active game/base directory.
    pub source: PathBuf,
}

#[derive(Debug, Clone)]
enum Provider {
    /// `entry` is the zip entry number, so reads skip the name hash lookup and
    /// the index does not need to keep a second copy of every entry name.
    Archive { archive: usize, entry: usize },
    Loose { path: PathBuf },
}

/// One normalized qpath and every provider of it, highest priority first.
#[derive(Debug)]
struct IndexEntry {
    key: String,
    first: Provider,
    rest: Vec<Provider>,
}

impl IndexEntry {
    fn providers(&self) -> impl Iterator<Item = &Provider> {
        std::iter::once(&self.first).chain(self.rest.iter())
    }
}

/// The immutable, shareable namespace of a set of search directories. Building
/// it reads every PK3 central directory, which dominates startup, so it is built
/// in parallel and cached process-wide (see [`shared_index`]).
#[derive(Debug)]
struct VfsIndex {
    archive_paths: Vec<PathBuf>,
    /// Entry count of each archive when it was indexed, to detect an archive
    /// that was replaced on disk before a lazily opened handle reads from it.
    archive_lens: Vec<usize>,
    /// Whether each archive is a retail `assets0..assets3.pk3`.
    archive_stock: Vec<bool>,
    /// Sorted by key, keys unique.
    entries: Vec<IndexEntry>,
    search_dirs: Vec<PathBuf>,
    /// Parsed archive handles returned by dropped `AssetSearchPath`s, per
    /// archive. Parsing a large PK3's central directory costs tens of
    /// milliseconds, so short-lived views reuse a parsed handle instead.
    idle_archives: Vec<Mutex<Vec<zip::ZipArchive<File>>>>,
}

/// Idle handles kept per archive; more concurrent readers just open their own.
/// Opening one parses the archive's whole central directory, so enough are kept
/// for every map worker to read textures at once.
const MAX_IDLE_ARCHIVE_HANDLES: usize = 16;

/// Process-wide count and total time of archive handles opened (as opposed to
/// reused from an idle pool); see [`archive_open_stats`].
static ARCHIVE_OPENS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static ARCHIVE_OPEN_MICROS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// `(handles opened, milliseconds spent opening them)` since process start.
pub fn archive_open_stats() -> (u64, f64) {
    (
        ARCHIVE_OPENS.load(std::sync::atomic::Ordering::Relaxed),
        ARCHIVE_OPEN_MICROS.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1000.0,
    )
}

impl VfsIndex {
    fn find(&self, key: &str) -> Option<&IndexEntry> {
        self.entries
            .binary_search_by(|entry| entry.key.as_str().cmp(key))
            .ok()
            .map(|index| &self.entries[index])
    }
}

/// The physical game directory that owns writable game-relative output.
///
/// This is deliberately the same active-game decision used by the VFS: an
/// active `fs_game` owns screenshots, demos, downloaded content and caches;
/// otherwise those writes belong to `base`. Engine-global state such as the
/// process log is intentionally outside this policy.
pub fn active_game_directory<'a>(base: &'a Path, game: Option<&'a Path>) -> &'a Path {
    game.unwrap_or(base)
}

/// Resolve a server/demo `fs_game` value to a sibling of the base game directory.
///
/// JKA treats `fs_game` as a single game-directory name, not an arbitrary path.
/// Keeping that rule here prevents a remote server from making the client search
/// outside GameData. Empty/`base` means the stock base game.
pub fn resolve_fs_game_directory(base: &Path, value: &[u8]) -> Result<Option<PathBuf>, String> {
    let name = std::str::from_utf8(value)
        .map_err(|_| "fs_game is not valid UTF-8".to_owned())?
        .trim();
    if name.is_empty() || name.eq_ignore_ascii_case("base") {
        return Ok(None);
    }
    if name.len() > 63
        || name == "."
        || name == ".."
        || name.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
        || name.contains(['/', '\\', ':'])
    {
        return Err(format!("invalid fs_game directory: {name:?}"));
    }

    let game_data = base.parent().unwrap_or(base);
    let candidate = game_data.join(name);

    // Look up the on-disk spelling first. Windows resolves paths
    // case-insensitively at the OS level, so a direct `candidate.is_dir()`
    // check would silently accept the requested casing even when it differs
    // from the real directory name; scanning here keeps the resolved path
    // faithful to what is actually on disk on every host.
    if let Ok(entries) = fs::read_dir(game_data) {
        for entry in entries.flatten() {
            if entry
                .file_type()
                .is_ok_and(|kind| kind.is_dir())
                && entry.file_name().to_string_lossy().eq_ignore_ascii_case(name)
            {
                return Ok(Some(entry.path()));
            }
        }
    }

    if candidate.is_dir() {
        return Ok(Some(candidate));
    }
    Ok(Some(candidate))
}

/// Indexed reusable JKA/OpenJK-style asset search path.
///
/// All `.pk3` files, Rend2 `cubemaps.zip`, and loose files beneath each game directory are visible
/// through one package-relative namespace. Search directories are passed from
/// highest to lowest priority. For each directory, PK3s are mounted first and
/// then loose files, matching JKA's normal package-before-directory behavior.

pub struct AssetSearchPath {
    shared: Arc<VfsIndex>,
    /// Per-instance archive handles, opened lazily on the first read from each
    /// archive (a `ZipArchive` is not shareable and needs `&mut` to read). The
    /// instance that built the index starts with its handles already open.
    archives: Vec<Option<zip::ZipArchive<File>>>,
    /// Normal JKA/OpenJK VFS behavior allows later/higher-priority packages to
    /// shadow stock assets. When false, ordinary reads prefer the stock
    /// assets0..assets3 provider whenever the qpath exists there. Explicit
    /// material-source reads intentionally bypass this protection.
    allow_asset_overrides: bool,
}

impl AssetSearchPath {
    /// Open a base game directory (normally `GameData/base`).
    pub fn open(base: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        Self::open_search_dirs([base])
    }

    /// Open an optional active mod/game directory above base priority.
    pub fn open_game(base: &Path, game: Option<&Path>) -> Result<Self, Box<dyn std::error::Error>> {
        let mut dirs = Vec::with_capacity(2);
        if let Some(game) = game.filter(|game| *game != base) {
            dirs.push(game);
        }
        dirs.push(base);
        Self::open_search_dirs(dirs)
    }

    /// Open the search directories (highest priority first).
    ///
    /// The namespace index is shared process-wide: opening the same directories
    /// again is nearly free while no PK3 or directory beneath them has changed
    /// (see [`shared_index`]), so callers can keep opening short-lived views.
    pub fn open_search_dirs<I, P>(dirs: I) -> Result<Self, Box<dyn std::error::Error>>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let dirs = dirs
            .into_iter()
            .map(|dir| dir.as_ref().to_path_buf())
            .collect::<Vec<_>>();
        let (shared, archives) = shared_index(&dirs)?;
        Ok(Self::from_shared(shared, archives))
    }

    fn from_shared(
        shared: Arc<VfsIndex>,
        archives: Option<Vec<Option<zip::ZipArchive<File>>>>,
    ) -> Self {
        let archives = archives
            .unwrap_or_else(|| (0..shared.archive_paths.len()).map(|_| None).collect());
        Self {
            shared,
            archives,
            allow_asset_overrides: true,
        }
    }

    /// A second reader over the same index, for another thread. It costs an
    /// `Arc` clone: no directory scan, and archive handles open lazily (reusing
    /// idle ones) on its first read from each archive.
    pub fn fork(&self) -> Self {
        let mut fork = Self::from_shared(Arc::clone(&self.shared), None);
        fork.allow_asset_overrides = self.allow_asset_overrides;
        fork
    }

    /// Open a handle on every retail assets0..assets3 archive now. Dropping this
    /// reader parks the handles in the shared idle pool, so later readers (for
    /// example parallel texture loads) skip the central-directory parse.
    pub fn warm_stock_archives(&mut self) {
        for index in 0..self.archives.len() {
            if self.shared.archive_stock[index] {
                let _ = self.archive_handle(index);
            }
        }
    }

    /// Directories this search path was opened from, highest priority first.
    pub fn search_dirs(&self) -> &[PathBuf] {
        &self.shared.search_dirs
    }

    /// Re-scan the same game directories and re-open every mounted archive.
    ///
    /// Ordinary loose-file reads already reopen their physical file, so this is
    /// only needed when the VFS namespace itself may have changed: new/deleted
    /// loose files, or added/replaced PK3/ZIP packages. Existing callers keep
    /// their `AssetSearchPath` allocation while its indexed view is swapped
    /// atomically after the replacement view opens successfully. The shared
    /// index is revalidated against the filesystem, so a refresh with nothing
    /// changed on disk does not re-read any archive.
    pub fn refresh(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let (shared, archives) = shared_index(&self.shared.search_dirs)?;
        let allow_asset_overrides = self.allow_asset_overrides;
        *self = Self::from_shared(shared, archives);
        self.allow_asset_overrides = allow_asset_overrides;
        Ok(())
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.shared.entries.iter().map(|entry| entry.key.as_str())
    }

    /// Fast indexed existence check for an exact virtual qpath. This mirrors
    /// BG_FileExists-style lookups without opening the asset.
    pub fn contains_qpath(&self, name: &str) -> bool {
        validate_asset_name(name).is_ok()
            && self.shared.find(&normalize_asset_name(name)).is_some()
    }

    /// Control whether ordinary VFS reads may shadow qpaths present in the
    /// retail JKA `assets0.pk3` through `assets3.pk3` archives. This does not
    /// affect [`Self::read_from_source`], because an active material explicitly
    /// requesting a dependency from its own provider is not a global override.
    pub fn set_allow_asset_overrides(&mut self, allow: bool) {
        self.allow_asset_overrides = allow;
    }

    pub fn asset_overrides_allowed(&self) -> bool {
        self.allow_asset_overrides
    }

    /// Read only from a retail assets0..assets3 provider for this exact qpath.
    /// Texture loaders use this before extension fallbacks so an addon `foo.jpg`
    /// cannot shadow a retail `foo.tga` when overrides are disabled.
    pub fn read_stock(
        &mut self,
        name: &str,
        limit: usize,
    ) -> Result<Option<Asset>, Box<dyn std::error::Error>> {
        validate_asset_name(name)?;
        let provider = self.stock_provider(&normalize_asset_name(name));
        let Some(provider) = provider else {
            return Ok(None);
        };
        self.read_provider(name, limit, provider).map(Some)
    }

    pub fn read(
        &mut self,
        name: &str,
        limit: usize,
    ) -> Result<Option<Asset>, Box<dyn std::error::Error>> {
        validate_asset_name(name)?;
        let key = normalize_asset_name(name);
        let Some(provider) = self.provider_for_ordinary_read(&key) else {
            return Ok(None);
        };
        self.read_provider(name, limit, provider).map(Some)
    }

    fn stock_provider(&self, key: &str) -> Option<Provider> {
        self.shared
            .find(key)?
            .providers()
            .find(|provider| self.provider_is_stock(provider))
            .cloned()
    }

    fn provider_for_ordinary_read(&self, key: &str) -> Option<Provider> {
        if !self.allow_asset_overrides {
            if let Some(stock) = self.stock_provider(key) {
                return Some(stock);
            }
        }
        self.shared.find(key).map(|entry| entry.first.clone())
    }

    fn provider_is_stock(&self, provider: &Provider) -> bool {
        match provider {
            Provider::Archive { archive, .. } => self.shared.archive_stock[*archive],
            Provider::Loose { .. } => false,
        }
    }

    /// Read this exact qpath from the same physical provider that supplied a
    /// material definition. This is used by active Rend2 `.mtr` materials so
    /// their base/normal/RMO/etc. images stay bound to the material package
    /// instead of being accidentally sourced from another VFS override.
    pub fn read_from_source(
        &mut self,
        name: &str,
        limit: usize,
        source: &Path,
    ) -> Result<Option<Asset>, Box<dyn std::error::Error>> {
        validate_asset_name(name)?;
        let provider = self
            .shared
            .find(&normalize_asset_name(name))
            .and_then(|entry| {
                entry
                    .providers()
                    .find(|provider| self.provider_matches_source(provider, source))
            })
            .cloned();
        match provider {
            Some(provider) => self.read_provider(name, limit, provider).map(Some),
            None => Ok(None),
        }
    }

    fn provider_matches_source(&self, provider: &Provider, source: &Path) -> bool {
        match provider {
            Provider::Archive { archive, .. } => self.shared.archive_paths[*archive].as_path() == source,
            Provider::Loose { path } => path.as_path() == source,
        }
    }

    /// The open handle for archive `index`, opening it on first use.
    fn archive_handle(
        &mut self,
        index: usize,
    ) -> Result<&mut zip::ZipArchive<File>, Box<dyn std::error::Error>> {
        if self.archives[index].is_none() {
            let idle = self.shared.idle_archives[index]
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop();
            let archive = match idle {
                Some(archive) => archive,
                None => {
                    let path = &self.shared.archive_paths[index];
                    let opened = std::time::Instant::now();
                    let archive = zip::ZipArchive::new(File::open(path)?)?;
                    ARCHIVE_OPENS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    ARCHIVE_OPEN_MICROS
                        .fetch_add(opened.elapsed().as_micros() as u64, std::sync::atomic::Ordering::Relaxed);
                    if archive.len() != self.shared.archive_lens[index] {
                        return Err(
                            format!("{} changed on disk since it was indexed", path.display()).into(),
                        );
                    }
                    archive
                }
            };
            self.archives[index] = Some(archive);
        }
        Ok(self.archives[index].as_mut().expect("archive handle just opened"))
    }

    fn read_provider(
        &mut self,
        name: &str,
        limit: usize,
        provider: Provider,
    ) -> Result<Asset, Box<dyn std::error::Error>> {
        match provider {
            Provider::Archive { archive, entry } => {
                // Clone the Arc so the paths stay readable while `file` holds
                // the mutable borrow of the archive handle.
                let shared = Arc::clone(&self.shared);
                let handle = self.archive_handle(archive)?;
                let mut file = handle.by_index(entry)?;
                if normalize_asset_name(file.name()) != normalize_asset_name(name) {
                    return Err(format!(
                        "{name}: {} changed on disk since it was indexed",
                        shared.archive_paths[archive].display()
                    )
                    .into());
                }
                let declared = file.size();
                if declared > limit as u64 {
                    return Err(format!("{name} exceeds asset size limit").into());
                }
                let mut bytes = Vec::new();
                (&mut file).take(limit as u64 + 1).read_to_end(&mut bytes)?;
                if bytes.len() > limit || bytes.len() as u64 != declared {
                    return Err("invalid expanded asset size".into());
                }
                Ok(Asset {
                    bytes,
                    source: shared.archive_paths[archive].clone(),
                })
            }
            Provider::Loose { path } => {
                let metadata = fs::metadata(&path)?;
                if metadata.len() > limit as u64 {
                    return Err(format!("{name} exceeds asset size limit").into());
                }
                let mut file = File::open(&path)?;
                let mut bytes = Vec::new();
                (&mut file).take(limit as u64 + 1).read_to_end(&mut bytes)?;
                if bytes.len() > limit || bytes.len() as u64 != metadata.len() {
                    return Err("invalid loose asset size".into());
                }
                Ok(Asset {
                    bytes,
                    source: path,
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Index construction and the process-wide index cache
// ---------------------------------------------------------------------------

type ArchiveHandles = Vec<Option<zip::ZipArchive<File>>>;

/// A directory whose newest timestamp is younger than this cannot be trusted as
/// "unchanged": a modification in the same timestamp tick would not move the
/// fingerprint. Such an index is rebuilt on its next open instead of reused.
const RACY_WINDOW: Duration = Duration::from_secs(3);

/// One thing to mount, listed in priority order (highest first): each search
/// directory contributes its PK3s (later-sorting names first) and then its
/// loose files.
enum Source {
    Archive { path: PathBuf, size: u64 },
    Loose(PathBuf),
}

enum Scanned {
    /// The opened archive and its `(normalized key, zip entry number)` pairs,
    /// sorted by key.
    Archive(zip::ZipArchive<File>, Vec<(String, usize)>),
    /// `(normalized key, path)` pairs, sorted by key.
    Loose(Vec<(String, PathBuf)>),
}

fn plan_sources(dirs: &[PathBuf]) -> Result<Vec<Source>, Box<dyn Error>> {
    let mut sources = Vec::new();
    for dir in dirs {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let mut packages = Vec::new();
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_file() && is_archive_package(&path) {
                let size = entry.metadata().map_or(0, |metadata| metadata.len());
                packages.push((path, size));
            }
        }
        let name_of = |path: &Path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("")
                .to_ascii_lowercase()
        };
        packages.sort_by(|a, b| name_of(&a.0).cmp(&name_of(&b.0)).reverse());
        sources.extend(
            packages
                .into_iter()
                .map(|(path, size)| Source::Archive { path, size }),
        );
        // JKA's normal search path checks the loose directory after that game
        // directory's PK3s, but before falling through to the lower-priority
        // base game. Indexing the directory here gives every asset class the
        // same behavior without renderer-specific filesystem fallbacks.
        sources.push(Source::Loose(dir.clone()));
    }
    Ok(sources)
}

fn scan_source(source: &Source) -> Result<Scanned, String> {
    match source {
        Source::Archive { path, .. } => {
            let file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
            let archive = zip::ZipArchive::new(file)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            let mut keys = Vec::with_capacity(archive.len());
            for index in 0..archive.len() {
                let name = archive
                    .name_for_index(index)
                    .ok_or_else(|| format!("{}: unreadable entry {index}", path.display()))?;
                keys.push((normalize_asset_name(name), index));
            }
            keys.sort_by(|a, b| a.0.cmp(&b.0));
            if let Some(pair) = keys.windows(2).find(|pair| pair[0].0 == pair[1].0) {
                let name = archive.name_for_index(pair[1].1).unwrap_or(&pair[1].0);
                return Err(format!(
                    "ambiguous case-insensitive entry {name} in {}",
                    path.display()
                ));
            }
            Ok(Scanned::Archive(archive, keys))
        }
        Source::Loose(root) => {
            let mut files = Vec::<(String, PathBuf)>::new();
            collect_loose_files(root, root, &mut files).map_err(|error| error.to_string())?;
            files.sort_by(|a, b| a.0.cmp(&b.0));
            if let Some(pair) = files.windows(2).find(|pair| pair[0].0 == pair[1].0) {
                return Err(format!(
                    "ambiguous case-insensitive loose asset {} under {}",
                    pair[1].0,
                    root.display()
                ));
            }
            Ok(Scanned::Loose(files))
        }
    }
}

/// Scan every source, in parallel, returning results in `sources` order. The
/// biggest archives start first so the slowest central directory is not left
/// to the end.
fn scan_sources(sources: &[Source]) -> Vec<Result<Scanned, String>> {
    let count = sources.len();
    let workers = std::thread::available_parallelism()
        .map_or(4, |parallelism| parallelism.get())
        .min(8)
        .min(count);
    if workers <= 1 {
        return sources.iter().map(scan_source).collect();
    }
    let mut order = (0..count).collect::<Vec<_>>();
    order.sort_by_key(|&index| match &sources[index] {
        Source::Loose(_) => std::cmp::Reverse(u64::MAX),
        Source::Archive { size, .. } => std::cmp::Reverse(*size),
    });
    let next = AtomicUsize::new(0);
    let results = (0..count).map(|_| Mutex::new(None)).collect::<Vec<_>>();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let slot = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(&index) = order.get(slot) else {
                    break;
                };
                let scanned = scan_source(&sources[index]);
                *results[index].lock().unwrap_or_else(PoisonError::into_inner) = Some(scanned);
            });
        }
    });
    results
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .unwrap_or_else(PoisonError::into_inner)
                .unwrap_or_else(|| Err("asset scan worker did not finish".to_owned()))
        })
        .collect()
}

fn build_index(dirs: &[PathBuf]) -> Result<(VfsIndex, ArchiveHandles), Box<dyn Error>> {
    let sources = plan_sources(dirs)?;
    let scans = scan_sources(&sources);

    let total = scans
        .iter()
        .map(|scan| match scan {
            Ok(Scanned::Archive(_, keys)) => keys.len(),
            Ok(Scanned::Loose(files)) => files.len(),
            Err(_) => 0,
        })
        .sum();
    let mut archive_paths = Vec::new();
    let mut archive_lens = Vec::new();
    let mut handles = ArchiveHandles::new();
    let mut flat = Vec::<(String, Provider)>::with_capacity(total);
    for (source, scan) in sources.iter().zip(scans) {
        match (source, scan?) {
            (Source::Archive { path, .. }, Scanned::Archive(archive, keys)) => {
                let archive_index = archive_paths.len();
                archive_paths.push(path.clone());
                archive_lens.push(archive.len());
                handles.push(Some(archive));
                flat.extend(keys.into_iter().map(|(key, entry)| {
                    (
                        key,
                        Provider::Archive {
                            archive: archive_index,
                            entry,
                        },
                    )
                }));
            }
            (_, Scanned::Loose(files)) => {
                flat.extend(
                    files
                        .into_iter()
                        .map(|(key, path)| (key, Provider::Loose { path })),
                );
            }
            _ => unreachable!("scan result matches its source"),
        }
    }

    // `flat` is in priority order and the sort is stable, so equal keys keep
    // their priority order: the first provider of an asset wins.
    flat.sort_by(|a, b| a.0.cmp(&b.0));
    let mut entries = Vec::<IndexEntry>::with_capacity(flat.len());
    for (key, provider) in flat {
        match entries.last_mut() {
            Some(last) if last.key == key => last.rest.push(provider),
            _ => entries.push(IndexEntry {
                key,
                first: provider,
                rest: Vec::new(),
            }),
        }
    }
    let archive_stock = archive_paths
        .iter()
        .map(|path| is_stock_asset_archive(path))
        .collect();
    let idle_archives = archive_paths.iter().map(|_| Mutex::default()).collect();
    Ok((
        VfsIndex {
            archive_paths,
            archive_lens,
            archive_stock,
            entries,
            search_dirs: dirs.to_vec(),
            idle_archives,
        },
        handles,
    ))
}

struct Fingerprint {
    hash: u64,
    racy: bool,
}

/// Cheap summary of everything the index is derived from: every directory's
/// modification time and listing beneath the search directories, plus the size
/// and modification time of each mounted package. Adding, removing or renaming
/// a PK3 or loose file moves it; walking it costs a few `read_dir` calls
/// instead of parsing every package.
fn fingerprint_search_dirs(dirs: &[PathBuf]) -> Fingerprint {
    let mut hasher = DefaultHasher::new();
    let mut newest = SystemTime::UNIX_EPOCH;
    for dir in dirs {
        dir.hash(&mut hasher);
        fingerprint_tree(dir, true, &mut hasher, &mut newest);
    }
    let racy = SystemTime::now()
        .duration_since(newest)
        .map_or(true, |age| age < RACY_WINDOW);
    Fingerprint {
        hash: hasher.finish(),
        racy,
    }
}

fn fingerprint_tree(dir: &Path, top: bool, hasher: &mut DefaultHasher, newest: &mut SystemTime) {
    let modified = fs::metadata(dir).and_then(|metadata| metadata.modified()).ok();
    modified.hash(hasher);
    if let Some(modified) = modified {
        *newest = (*newest).max(modified);
    }
    let Ok(read) = fs::read_dir(dir) else {
        0u8.hash(hasher);
        return;
    };
    let mut children = Vec::<(OsString, bool, Option<(u64, Option<SystemTime>)>)>::new();
    for entry in read.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let stamp = (top && file_type.is_file() && is_archive_package(&entry.path()))
            .then(|| entry.metadata().ok())
            .flatten()
            .map(|metadata| (metadata.len(), metadata.modified().ok()));
        if let Some((_, Some(modified))) = stamp {
            *newest = (*newest).max(modified);
        }
        children.push((entry.file_name(), file_type.is_dir(), stamp));
    }
    children.sort_by(|a, b| a.0.cmp(&b.0));
    children.len().hash(hasher);
    for (name, is_dir, stamp) in &children {
        name.hash(hasher);
        is_dir.hash(hasher);
        stamp.hash(hasher);
    }
    for (name, is_dir, _) in &children {
        if *is_dir {
            fingerprint_tree(&dir.join(name), false, hasher, newest);
        }
    }
}

struct CachedIndex {
    fingerprint: u64,
    racy: bool,
    index: Arc<VfsIndex>,
}

type CacheSlot = Arc<Mutex<Option<CachedIndex>>>;

fn index_cache() -> &'static Mutex<HashMap<Vec<PathBuf>, CacheSlot>> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<Vec<PathBuf>, CacheSlot>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// `JKA_VFS_CACHE=0` rebuilds the index on every open (the pre-cache behavior),
/// as an escape hatch for file systems whose directory timestamps are unreliable.
fn index_cache_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("JKA_VFS_CACHE").map_or(true, |value| value != "0"))
}

/// The index for `dirs`, from the process-wide cache while the file system still
/// matches the fingerprint it was built under, otherwise freshly built (in
/// parallel). The archive handles are returned only when this call built the
/// index, so the first caller does not reopen them.
///
/// A concurrent open of the same directories (for example the startup prewarm
/// thread and the renderer) waits on the slot and then reuses the result rather
/// than building twice.
fn shared_index(dirs: &[PathBuf]) -> Result<(Arc<VfsIndex>, Option<ArchiveHandles>), Box<dyn Error>> {
    if !index_cache_enabled() {
        let (index, handles) = build_index(dirs)?;
        return Ok((Arc::new(index), Some(handles)));
    }
    let slot = {
        let mut cache = index_cache().lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(cache.entry(dirs.to_vec()).or_default())
    };
    let mut cached = slot.lock().unwrap_or_else(PoisonError::into_inner);
    let fingerprint = fingerprint_search_dirs(dirs);
    if let Some(entry) = cached.as_ref() {
        if !entry.racy && entry.fingerprint == fingerprint.hash {
            return Ok((Arc::clone(&entry.index), None));
        }
    }
    let (index, handles) = build_index(dirs)?;
    let index = Arc::new(index);
    *cached = Some(CachedIndex {
        fingerprint: fingerprint.hash,
        racy: fingerprint.racy,
        index: Arc::clone(&index),
    });
    Ok((index, Some(handles)))
}

impl Drop for AssetSearchPath {
    fn drop(&mut self) {
        for (index, archive) in self.archives.drain(..).enumerate() {
            let Some(archive) = archive else {
                continue;
            };
            let mut idle = self.shared.idle_archives[index]
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if idle.len() < MAX_IDLE_ARCHIVE_HANDLES {
                idle.push(archive);
            }
        }
    }
}

/// Backwards-compatible name for callers outside this workspace. New code should
/// use `AssetSearchPath`, which now represents PK3 and loose assets together.
pub type StockPackages = AssetSearchPath;

fn is_stock_asset_archive(path: &Path) -> bool {
    let in_base = path
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("base"));
    in_base
        && path.file_name().is_some_and(|name| {
            matches!(
                name.to_string_lossy().to_ascii_lowercase().as_str(),
                "assets0.pk3" | "assets1.pk3" | "assets2.pk3" | "assets3.pk3"
            )
        })
}

fn is_archive_package(path: &Path) -> bool {
    if path
        .extension()
        .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case("pk3"))
    {
        return true;
    }
    // Rend2 map packs commonly keep generated reflection probes in a sibling
    // `cubemaps.zip` rather than renaming the generated archive to .pk3. Stock
    // JKA treats arbitrary .zip files as opaque files, so keep this narrowly
    // scoped instead of mounting every ZIP in GameData.
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("cubemaps.zip"))
}

fn collect_loose_files(
    root: &Path,
    directory: &Path,
    output: &mut Vec<(String, PathBuf)>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase());

    for entry in entries {
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_loose_files(root, &path, output)?;
            continue;
        }
        if !file_type.is_file() || is_archive_package(&path) {
            continue;
        }
        let relative = path.strip_prefix(root)?;
        let name = relative.to_string_lossy().replace('\\', "/");
        validate_asset_name(&name)?;
        output.push((normalize_asset_name(&name), path));
    }
    Ok(())
}

fn normalize_asset_name(name: &str) -> String {
    name.replace('\\', "/").to_ascii_lowercase()
}

fn validate_asset_name(entry: &str) -> Result<(), Box<dyn std::error::Error>> {
    if entry.is_empty()
        || entry
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
        || entry.contains(['\\', ':', '\0'])
    {
        return Err("invalid package-relative asset name".into());
    }
    Ok(())
}

pub fn read_stock(
    base: &Path,
    entry: &str,
    max_bytes: usize,
) -> Result<Asset, Box<dyn std::error::Error>> {
    validate_asset_name(entry)?;
    let mut assets = AssetSearchPath::open(base)?;
    assets
        .read(entry, max_bytes)?
        .ok_or_else(|| format!("{entry} not found under {}", base.display()).into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_package(path: &Path, name: &str, content: &[u8]) {
        write_package_entries(path, &[(name, content)]);
    }

    fn write_package_entries(path: &Path, entries: &[(&str, &[u8])]) {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        for (name, content) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(content).unwrap();
        }
        zip.finish().unwrap();
    }

    fn test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "jka-assets-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ))
    }

    #[test]
    fn arbitrary_pk3_names_and_priority_are_indexed() {
        let root = test_root("pk3-priority");
        let base = root.join("base");
        let mod_dir = root.join("mymod");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&mod_dir).unwrap();
        write_package(&base.join("assets0.pk3"), "maps/mp/test.bsp", b"stock");
        write_package(&base.join("zz_patch.pk3"), "MAPS/MP/TEST.BSP", b"patch");
        write_package(&mod_dir.join("custom.pk3"), "maps/mp/test.bsp", b"mod");
        let mut assets = AssetSearchPath::open_game(&base, Some(&mod_dir)).unwrap();
        let asset = assets.read("maps/mp/test.bsp", 16).unwrap().unwrap();
        assert_eq!(asset.bytes, b"mod");
        assert!(asset.source.ends_with("custom.pk3"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn loose_files_participate_in_jka_search_order() {
        let root = test_root("loose-priority");
        let base = root.join("base");
        let mod_dir = root.join("mymod");
        std::fs::create_dir_all(base.join("maps")).unwrap();
        std::fs::create_dir_all(mod_dir.join("maps")).unwrap();
        std::fs::create_dir_all(mod_dir.join("shaders")).unwrap();

        write_package(&base.join("assets0.pk3"), "maps/test.bsp", b"base-pk3");
        std::fs::write(base.join("maps/test.bsp"), b"base-loose").unwrap();
        std::fs::write(mod_dir.join("maps/test.bsp"), b"mod-loose").unwrap();
        std::fs::write(mod_dir.join("maps/source.map"), b"source-map").unwrap();
        std::fs::write(mod_dir.join("shaders/dev.shader"), b"shader").unwrap();

        let mut assets = AssetSearchPath::open_game(&base, Some(&mod_dir)).unwrap();
        let bsp = assets.read("maps/test.bsp", 32).unwrap().unwrap();
        assert_eq!(bsp.bytes, b"mod-loose");
        assert!(bsp.source.ends_with("mymod/maps/test.bsp"));

        let map = assets.read("maps/source.map", 32).unwrap().unwrap();
        assert_eq!(map.bytes, b"source-map");
        assert!(assets.names().any(|name| name == "shaders/dev.shader"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refresh_discovers_new_loose_files_and_packages() {
        let root = test_root("refresh-search-path");
        let base = root.join("base");
        std::fs::create_dir_all(base.join("maps")).unwrap();

        let mut assets = AssetSearchPath::open(&base).unwrap();
        assert!(assets.read("maps/new.map", 32).unwrap().is_none());
        assert!(assets.read("textures/new/image.tga", 32).unwrap().is_none());

        std::fs::write(base.join("maps/new.map"), b"loose-new").unwrap();
        write_package(
            &base.join("zz_new.pk3"),
            "textures/new/image.tga",
            b"packed-new",
        );

        // The old index remains stable until the explicit refresh boundary.
        assert!(assets.read("maps/new.map", 32).unwrap().is_none());
        assert!(assets.read("textures/new/image.tga", 32).unwrap().is_none());

        assets.refresh().unwrap();
        assert_eq!(
            assets.read("maps/new.map", 32).unwrap().unwrap().bytes,
            b"loose-new"
        );
        assert_eq!(
            assets
                .read("textures/new/image.tga", 32)
                .unwrap()
                .unwrap()
                .bytes,
            b"packed-new"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pk3_beats_loose_file_within_the_same_game_directory() {
        let root = test_root("pk3-before-loose");
        let base = root.join("base");
        std::fs::create_dir_all(base.join("textures/test")).unwrap();
        write_package(
            &base.join("assets0.pk3"),
            "textures/test/wall.tga",
            b"packed",
        );
        std::fs::write(base.join("textures/test/wall.tga"), b"loose").unwrap();

        let mut assets = AssetSearchPath::open(&base).unwrap();
        let asset = assets.read("textures/test/wall.tga", 32).unwrap().unwrap();
        assert_eq!(asset.bytes, b"packed");
        assert!(asset.source.ends_with("assets0.pk3"));

        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn read_from_source_selects_material_package() {
        let root = test_root("material-source");
        let base = root.join("base");
        std::fs::create_dir_all(&base).unwrap();
        write_package(&base.join("assets0.pk3"), "textures/test/base.jpg", b"stock");
        write_package_entries(
            &base.join("zz_pbr.pk3"),
            &[
                ("textures/test/base.jpg", b"pbr" as &[u8]),
                ("shaders/test.mtr", b"material" as &[u8]),
            ],
        );

        let mut assets = AssetSearchPath::open(&base).unwrap();
        let material = assets.read("shaders/test.mtr", 32).unwrap().unwrap();
        assert!(material.source.ends_with("zz_pbr.pk3"));
        let image = assets
            .read_from_source("textures/test/base.jpg", 16, &material.source)
            .unwrap()
            .unwrap();
        assert_eq!(image.bytes, b"pbr");
        assert_eq!(image.source, material.source);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn asset_override_protection_prefers_stock_but_material_source_can_override() {
        let root = test_root("asset-override-policy");
        let base = root.join("base");
        std::fs::create_dir_all(&base).unwrap();
        write_package(&base.join("assets1.pk3"), "textures/test/base.jpg", b"stock");
        write_package_entries(
            &base.join("zz_pbr.pk3"),
            &[
                ("textures/test/base.jpg", b"pbr" as &[u8]),
                ("textures/test/new.jpg", b"new" as &[u8]),
                ("shaders/test.mtr", b"material" as &[u8]),
            ],
        );

        let mut assets = AssetSearchPath::open(&base).unwrap();
        assert_eq!(
            assets.read("textures/test/base.jpg", 16).unwrap().unwrap().bytes,
            b"pbr"
        );
        assets.set_allow_asset_overrides(false);
        let protected = assets.read("textures/test/base.jpg", 16).unwrap().unwrap();
        assert_eq!(protected.bytes, b"stock");
        assert!(protected.source.ends_with("assets1.pk3"));
        assert_eq!(
            assets.read("textures/test/new.jpg", 16).unwrap().unwrap().bytes,
            b"new"
        );

        let material = assets.read("shaders/test.mtr", 32).unwrap().unwrap();
        let material_owned = assets
            .read_from_source("textures/test/base.jpg", 16, &material.source)
            .unwrap()
            .unwrap();
        assert_eq!(material_owned.bytes, b"pbr");
        assert!(material_owned.source.ends_with("zz_pbr.pk3"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn writable_game_directory_follows_active_fs_game_even_before_it_exists() {
        let root = test_root("active-game-write-root");
        let base = root.join("base");
        let mod_dir = root.join("japro");
        std::fs::create_dir_all(&base).unwrap();

        assert_eq!(active_game_directory(&base, None), base.as_path());
        assert_eq!(
            active_game_directory(&base, Some(&mod_dir)),
            mod_dir.as_path()
        );

        // A remote fs_game may name a valid sibling that is not installed yet.
        // Writable output is still allowed to create that directory later.
        assert!(!mod_dir.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fs_game_resolves_only_gamedata_siblings() {
        let root = test_root("fs-game-resolve");
        let base = root.join("base");
        let mod_dir = root.join("JAPlus");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&mod_dir).unwrap();

        assert_eq!(resolve_fs_game_directory(&base, b"").unwrap(), None);
        assert_eq!(resolve_fs_game_directory(&base, b"base").unwrap(), None);
        assert_eq!(
            resolve_fs_game_directory(&base, b"japlus").unwrap(),
            Some(mod_dir.clone())
        );
        for bad in [b"../evil".as_slice(), b"foo/bar", b"foo\\bar", b"C:evil", b".."] {
            assert!(resolve_fs_game_directory(&base, bad).is_err(), "accepted {bad:?}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

}
