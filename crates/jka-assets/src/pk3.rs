//! Read-only JKA virtual asset search path with PK3/PK4, Rend2 companion ZIP, and loose-file support.
//!
//! Search order follows the useful core of JKA/OpenJK semantics for this client:
//! the active game/mod directory is searched before base, and within each game
//! directory PK3/PK4 files take precedence over loose files. Later-sorting
//! package names override earlier ones; extensions have no special priority.
//! Every caller uses package-relative qpaths such as
//! `maps/mp/ffa3.bsp`, `textures/foo/bar.tga`, or `shaders/common.shader`.
use std::{
    collections::{HashMap, HashSet},
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
    /// The physical file that supplied this asset: a `.pk3`/`.pk4` archive or a
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
    jk2_pure: Option<Arc<crate::jk2_pure::Jk2PurePolicy>>,
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
    resolve_fs_game_in(base.parent().unwrap_or(base), value)
}

/// Resolve a server-selected mod inside the active game's dedicated root.
/// No mod name is reserved or rewritten: JK2 and JKA have different roots.
pub fn resolve_fs_game_directory_for_roots(
    base: &Path, value: &[u8], roots: &FilesystemRoots,
) -> Result<Option<PathBuf>, String> {
    let game_root = if roots.active_jk2.is_some() { jk2_root_directory(base, roots) }
        else { base.parent().unwrap_or(base).to_path_buf() };
    resolve_fs_game_in(&game_root, value)
}

/// The remote fs_game is the physical mod directory name, unchanged.
pub fn fs_game_name(game: Option<&Path>) -> String {
    game.and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "base".to_owned())
}

/// The executable root supplied at bootstrap, or its layout-derived fallback
/// for tests/standalone asset tooling.
fn client_install_directory(base: &Path, roots: &FilesystemRoots) -> PathBuf {
    if let Some(install) = &roots.client_install { return install.clone(); }
    let game_root = base.parent().unwrap_or(base);
    if game_root.file_name().is_some_and(|name| name == "JKA GameData") {
        game_root.parent().unwrap_or(game_root).to_path_buf()
    } else {
        game_root.to_path_buf()
    }
}

/// Installed JK2 root (containing base/ and regular mod directories).
pub fn jk2_root_directory(base: &Path, roots: &FilesystemRoots) -> PathBuf {
    roots.jk2basepath.clone().unwrap_or_else(|| client_install_directory(base, roots).join("JK2 GameData"))
}

/// Optional writable overlays stay separate by game even if fs_homepath is set.
pub fn game_home_root(roots: &FilesystemRoots, jk2: bool) -> Option<PathBuf> {
    roots.homepath.as_ref().map(|home| home.join(if jk2 { "JK2 GameData" } else { "JKA GameData" }))
}

fn resolve_fs_game_in(game_data: &Path, value: &[u8]) -> Result<Option<PathBuf>, String> {
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

/// Filesystem roots configured by the client. Both game roots contain `base/`
/// and unprefixed mod folders; CD paths are base-only read-only fallbacks.
/// These settings never cause a PK3 to be unpacked or a DLL to be executed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilesystemRoots {
    /// Optional JKA installation root (containing base/), applied at startup.
    pub basepath: Option<PathBuf>,
    /// Executable installation root supplied at startup; never serialized.
    pub client_install: Option<PathBuf>,
    /// Optional writable overlay parent; separate per-game directories are appended.
    pub homepath: Option<PathBuf>,
    /// Extra, lowest-priority read-only root (usually a shared retail install).
    pub cdpath: Option<PathBuf>,
    /// JK2 installation root containing base/; defaults to JK2 GameData/.
    pub jk2basepath: Option<PathBuf>,
    /// Separate read-only JK2 retail GameData root; never mounted by JKA.
    pub jk2cdpath: Option<PathBuf>,
    /// Session-only JK2 asset version: 2 for 1.02, 4 for 1.04. Never persisted.
    pub active_jk2: Option<u8>,
    /// Immutable session-only JK2 pure policy; never saved in user settings.
    pub jk2_pure: Option<Arc<crate::jk2_pure::Jk2PurePolicy>>,
    /// Optional highest-priority, client-owned read-only asset layer: DinurdoJK/<name>/.
    pub forcegame: Option<String>,
}

#[derive(Debug, Clone)]
struct InstalledFilesystemRoots {
    base: PathBuf,
    roots: FilesystemRoots,
}

// A single executable owns one filesystem. Storing its active roots here lets
// map workers, the renderer and presenters keep using open_game(base, game)
// without silently bypassing newly configured layers.
static FILESYSTEM_ROOTS: std::sync::OnceLock<std::sync::RwLock<Option<InstalledFilesystemRoots>>> =
    std::sync::OnceLock::new();

pub fn configure_filesystem_roots(base: &Path, roots: FilesystemRoots) {
    let lock = FILESYSTEM_ROOTS.get_or_init(|| std::sync::RwLock::new(None));
    *lock.write().unwrap_or_else(PoisonError::into_inner) =
        Some(InstalledFilesystemRoots { base: base.to_path_buf(), roots: roots.clone() });
    configure_package_policy(base, &roots);
}

/// The JK2 version whose filesystem is installed (live JK2 or JK2 demo), if any.
pub fn active_jk2() -> Option<u8> {
    FILESYSTEM_ROOTS
        .get()
        .and_then(|lock| lock.read().ok())
        .and_then(|state| state.as_ref().and_then(|state| state.roots.active_jk2))
}

pub fn configured_filesystem_roots(base: &Path) -> FilesystemRoots {
    FILESYSTEM_ROOTS
        .get()
        .and_then(|lock| lock.read().ok())
        .and_then(|state| state.as_ref().filter(|state| state.base == base).map(|state| state.roots.clone()))
        .unwrap_or_default()
}

/// Inventory of an installed archive, including disabled ones. The inventory
/// scans directory entries only: opening this page does not decode any PK3s.
#[derive(Debug, Clone)]
pub struct PackageInfo {
    pub path: PathBuf,
    pub directory: PathBuf,
    pub bytes: u64,
    pub stock: bool,
    pub enabled: bool,
}

#[derive(Default)]
struct PackagePolicy {
    disabled: HashSet<PathBuf>,
    incompatible: HashSet<PathBuf>,
    jk2_pure: Option<Arc<crate::jk2_pure::Jk2PurePolicy>>,
    generation: u64,
}

fn package_policy() -> &'static std::sync::RwLock<PackagePolicy> {
    static POLICY: std::sync::OnceLock<std::sync::RwLock<PackagePolicy>> = std::sync::OnceLock::new();
    POLICY.get_or_init(Default::default)
}

/// A process-wide package policy shared by every VFS reader and the pure/
/// download scanners. Paths are fully-qualified to distinguish identically
/// named PK3s in different roots and game directories.
pub fn package_enabled(path: &Path) -> bool {
    let state = package_policy().read().unwrap_or_else(PoisonError::into_inner);
    !state.incompatible.contains(path) && (is_stock_asset_archive(path) || !state.disabled.contains(path))
        && state.jk2_pure.as_ref().is_none_or(|pure| pure.allows_package(path))
}

pub fn disabled_package_paths() -> HashSet<PathBuf> {
    package_policy().read().unwrap_or_else(PoisonError::into_inner).disabled.clone()
}

fn disabled_packages_path(base: &Path, roots: &FilesystemRoots) -> PathBuf {
    // A single per-installation policy, independent of current fs_game or
    // fs_forcegame. All sessions read the same enabled/disabled package set.
    dinurdojk_directory_for(base, roots).join("pk3-disabled.txt")
}

fn read_disabled_packages(path: &Path) -> HashSet<PathBuf> {
    let Ok(text) = fs::read_to_string(path) else { return HashSet::new() };
    text.lines().filter(|line| !line.trim().is_empty())
        .map(PathBuf::from).filter(|path| !is_stock_asset_archive(path)).collect()
}

/// Install the persisted policy before the first VFS prewarm. Reconfiguring
/// roots (mainly tests and standalone tools) re-reads the corresponding file.
fn configure_package_policy(base: &Path, roots: &FilesystemRoots) {
    let path = disabled_packages_path(base, roots);
    let disabled = read_disabled_packages(&path);
    let mut incompatible = HashSet::new();
    if roots.active_jk2 == Some(2) {
        for dir in filesystem_search_dirs(base, None, roots) {
            incompatible.insert(dir.join("assets2.pk3"));
            incompatible.insert(dir.join("assets5.pk3"));
        }
    }
    let mut state = package_policy().write().unwrap_or_else(PoisonError::into_inner);
    state.disabled = disabled;
    state.incompatible = incompatible;
    state.jk2_pure = if roots.active_jk2.is_some() { roots.jk2_pure.clone() } else { None };
    state.generation = state.generation.wrapping_add(1);
}

/// Persist and atomically apply a new disabled set. Reject stock archives and
/// unknown paths: remote-controlled filesystem names must never become writes.
/// The caller follows this with fs_restart to refresh existing readers.
pub fn apply_disabled_packages(base: &Path, game: Option<&Path>, requested: &HashSet<PathBuf>) -> Result<(), String> {
    let roots = configured_filesystem_roots(base);
    let directories = filesystem_search_dirs(base, game, &roots);
    let mut installed = HashSet::new();
    for dir in &directories {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|kind| kind.is_file()) && is_archive_package(&entry.path()) {
                    installed.insert(entry.path());
                }
            }
        }
    }
    // Keep disabled packages belonging to an inactive mod/root, provided they
    // were already saved. Their membership remains immutable from this menu.
    let current = disabled_package_paths();
    for path in requested {
        if is_stock_asset_archive(path) || (!installed.contains(path) && !current.contains(path)) {
            return Err(format!("cannot disable unknown or required archive: {}", path.display()));
        }
    }
    let path = disabled_packages_path(base, &roots);
    let mut lines = requested.iter().collect::<Vec<_>>();
    lines.sort();
    let contents = lines.into_iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join("\n");
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    let temporary = path.with_extension("txt.tmp");
    fs::write(&temporary, contents).map_err(|e| e.to_string())?;
    if let Err(error) = fs::rename(&temporary, &path) {
        // Windows cannot rename over an existing destination. A failed write
        // must not apply a policy that cannot be persisted.
        if path.exists() {
            fs::remove_file(&path).map_err(|e| e.to_string())?;
            fs::rename(&temporary, &path).map_err(|e| e.to_string())?;
        } else { return Err(error.to_string()); }
    }
    let mut state = package_policy().write().unwrap_or_else(PoisonError::into_inner);
    state.disabled = requested.clone();
    state.generation = state.generation.wrapping_add(1);
    Ok(())
}

/// Every visible PK3/PK4 (and Rend2 companion ZIP) in VFS order, including disabled packages.
pub fn installed_packages(base: &Path, game: Option<&Path>) -> Vec<PackageInfo> {
    let roots = configured_filesystem_roots(base);
    let mut packages = Vec::new();
    for directory in filesystem_search_dirs(base, game, &roots) {
        let Ok(entries) = fs::read_dir(&directory) else { continue };
        let mut layer = entries.flatten().filter_map(|entry| {
            if !entry.file_type().ok()?.is_file() || !is_archive_package(&entry.path()) { return None; }
            let path = entry.path();
            Some(PackageInfo { bytes: entry.metadata().ok()?.len(),
                stock: is_stock_asset_archive(&path), enabled: package_enabled(&path),
                directory: directory.clone(), path })
        }).collect::<Vec<_>>();
        layer.sort_by_key(|info| std::cmp::Reverse(info.path.file_name().map(|name| name.to_string_lossy().to_ascii_lowercase())));
        packages.extend(layer);
    }
    packages
}

/// Resolve game-owned writes independently of the client-only fs_forcegame
/// override. Configs, demos, screenshots and downloads retain legacy paths;
/// never write into DinurdoJK/<forcegame> or fs_cdpath.
pub fn game_write_directory(base: &Path, game: Option<&Path>) -> PathBuf {
    let roots = configured_filesystem_roots(base);
    filesystem_write_directory(base, game, &roots)
}

/// Pure counterpart for testing and for callers that already know the roots.
pub fn filesystem_write_directory(base: &Path, game: Option<&Path>, roots: &FilesystemRoots) -> PathBuf {
    let jk2 = roots.active_jk2.is_some();
    let root = game_home_root(roots, jk2).unwrap_or_else(|| if jk2 {
        jk2_root_directory(base, roots)
    } else {
        base.parent().unwrap_or(base).to_path_buf()
    });
    root.join(game.and_then(Path::file_name).unwrap_or_else(|| std::ffi::OsStr::new("base")))
}

/// Client-owned data next to JK2 GameData/ and JKA GameData/, not inside
/// either game's mod namespace. Independent of fs_game and fs_homepath.
pub fn dinurdojk_directory(base: &Path) -> PathBuf {
    dinurdojk_directory_for(base, &configured_filesystem_roots(base))
}

/// Client-owned directory name.
pub const DINURDOJK_DIRECTORY: &str = "DinurdoJK";

/// [`dinurdojk_directory`] for roots that are not installed yet (early startup).
pub fn dinurdojk_directory_for(base: &Path, _roots: &FilesystemRoots) -> PathBuf {
    client_install_directory(base, _roots).join(DINURDOJK_DIRECTORY)
}

/// Client resource layer; allows PK3 and loose assets without indexing cache/.
pub fn dinurdojk_shared_directory(base: &Path, roots: &FilesystemRoots) -> PathBuf {
    dinurdojk_directory_for(base, roots).join("shared")
}

pub(crate) fn is_package_only_dir(_dir: &Path) -> bool { false }

/// `<install>/DinurdoJK/cache`: everything regenerable (menu scene, shader and
/// pipeline caches, bakes, server lists).
pub fn dinurdojk_cache_directory(base: &Path) -> PathBuf {
    dinurdojk_directory(base).join("cache")
}

/// [`dinurdojk_directory`] for the filesystem installed by the client, for
/// worker code that was never handed `base`.
pub fn installed_dinurdojk_directory() -> Option<PathBuf> {
    let base = FILESYSTEM_ROOTS.get()?.read().ok()?.as_ref()?.base.clone();
    Some(dinurdojk_directory(&base))
}

/// JK2 stock base directory beneath fs_jk2basepath.
pub fn jk2_base_directory(base: &Path, roots: &FilesystemRoots) -> PathBuf {
    jk2_root_directory(base, roots).join("base")
}

/// Require retail assets across the *combined* local and JK2 CD base layers.
/// A partial local install may be completed by the optional retail fallback.
/// No JKA directory is eligible, even if it contains the same PK3 filenames.
pub fn missing_jk2_stock_archive(base: &Path, roots: &FilesystemRoots, version: u8) -> Option<&'static str> {
    let required: &[&str] = if version == 2 {
        &["assets0.pk3", "assets1.pk3"]
    } else {
        &["assets0.pk3", "assets1.pk3", "assets2.pk3", "assets5.pk3"]
    };
    let mut dirs = Vec::new();
    if let Some(home) = game_home_root(roots, true) {
        add_unique_directory(&mut dirs, home.join("base"));
    }
    add_unique_directory(&mut dirs, jk2_base_directory(base, roots));
    if let Some(cd) = &roots.jk2cdpath {
        add_unique_directory(&mut dirs, cd.join("base"));
    }
    required.iter().copied().find(|name| !dirs.iter().any(|dir| dir.join(name).is_file()))
}

fn add_unique_directory(dirs: &mut Vec<PathBuf>, path: PathBuf) {
    if !dirs.iter().any(|existing| existing == &path) { dirs.push(path); }
}

fn add_game_layer(dirs: &mut Vec<PathBuf>, local: &Path, home: Option<&Path>, game: &std::ffi::OsStr) {
    if let Some(home) = home {
        add_unique_directory(dirs, home.join(game));
    }
    add_unique_directory(dirs, local.join(game));
}

/// One game's own content, in homepath first then installation root. Used by
/// demo browser selections that must not inherit unrelated mods or base demos.
pub fn filesystem_game_layer_dirs(base: &Path, game: Option<&Path>, roots: &FilesystemRoots) -> Vec<PathBuf> {
    let jk2 = roots.active_jk2.is_some();
    let root = if jk2 { jk2_root_directory(base, roots) }
        else { base.parent().unwrap_or(base).to_path_buf() };
    let home = game_home_root(roots, jk2);
    let name = game.and_then(Path::file_name).unwrap_or_else(|| std::ffi::OsStr::new("base"));
    let mut dirs = Vec::new();
    add_game_layer(&mut dirs, &root, home.as_deref(), name);
    dirs
}

/// Mount order, highest first: DinurdoJK/<forcegame> (if set),
/// DinurdoJK/shared, active game mod, local base, CD base. forcegame is a
/// client-owned asset override only, *not* a game mod or writable directory.
/// CD roots never contribute mod folders or writes. JK2/JKA stay isolated.
pub fn filesystem_search_dirs(base: &Path, game: Option<&Path>, roots: &FilesystemRoots) -> Vec<PathBuf> {
    let jk2 = roots.active_jk2.is_some();
    let root = if jk2 { jk2_root_directory(base, roots) }
        else { base.parent().unwrap_or(base).to_path_buf() };
    let home = game_home_root(roots, jk2);
    let mut dirs = Vec::new();
    if let Some(forced) = &roots.forcegame {
        add_unique_directory(&mut dirs, dinurdojk_directory_for(base, roots).join(forced));
    }
    add_unique_directory(&mut dirs, dinurdojk_shared_directory(base, roots));
    if let Some(name) = game.and_then(Path::file_name) {
        if !name.to_string_lossy().eq_ignore_ascii_case("base") {
            add_game_layer(&mut dirs, &root, home.as_deref(), name);
        }
    }
    add_game_layer(&mut dirs, &root, home.as_deref(), std::ffi::OsStr::new("base"));
    let cd = if jk2 { &roots.jk2cdpath } else { &roots.cdpath };
    if let Some(cd) = cd { add_unique_directory(&mut dirs, cd.join("base")); }
    dirs
}

/// Indexed reusable JKA/OpenJK-style asset search path.
///
/// All `.pk3`/`.pk4` files, Rend2 `cubemaps.zip`, and loose files beneath each
/// game directory are visible through one package-relative namespace. PK4 is the
/// same ZIP format as PK3 but is ignored by vanilla jamp.exe. Search directories
/// are passed from highest to lowest priority. Within each directory, packages
/// retain the existing reverse-alphabetical order before loose files.

/// Opaque identity of one built VFS index; see [`AssetSearchPath::index_token`].
/// Weak, so a cached token never keeps a replaced index (and the idle archive
/// handles that lock its PK3 files) alive; it still pins the allocation, so the
/// address cannot be reused by a newer index.
#[derive(Clone)]
pub struct VfsIndexToken(std::sync::Weak<VfsIndex>);

impl PartialEq for VfsIndexToken {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Weak::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for VfsIndexToken {}

/// Source information from the indexed virtual filesystem, without reading file bytes.
#[derive(Debug, Clone)]
pub struct AssetProviderInfo {
    pub source: PathBuf,
    pub archive: bool,
    pub selected: bool,
}

pub struct AssetSearchPath {
    /// Captured when opened, so readers retain game identity across transitions.
    jk2_version: Option<u8>,
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
        let roots = configured_filesystem_roots(base);
        let mut assets = Self::open_search_dirs(filesystem_search_dirs(base, game, &roots))?;
        assets.jk2_version = roots.active_jk2;
        Ok(assets)
    }

    /// [`Self::open_game`] for a specific asset game (`None` = JKA, `Some(2|4)`
    /// = JK2 1.02/1.04) without installing it: menu browsers can list another
    /// game's files while the process-wide filesystem stays as it is.
    pub fn open_game_as(base: &Path, game: Option<&Path>, jk2: Option<u8>) -> Result<Self, Box<dyn std::error::Error>> {
        let mut roots = configured_filesystem_roots(base);
        if roots.active_jk2 != jk2 {
            roots.active_jk2 = jk2;
            roots.jk2_pure = None;
        }
        let mut assets = Self::open_search_dirs(filesystem_search_dirs(base, game, &roots))?;
        assets.jk2_version = jk2;
        Ok(assets)
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
            jk2_version: shared.jk2_pure.as_ref().map(|policy| policy.version),
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
        fork.jk2_version = self.jk2_version;
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

    pub fn jk2_version(&self) -> Option<u8> { self.jk2_version }
    pub fn jk2_pure_policy(&self) -> Option<&Arc<crate::jk2_pure::Jk2PurePolicy>> { self.shared.jk2_pure.as_ref() }

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
        let jk2_version = self.jk2_version;
        *self = Self::from_shared(shared, archives);
        self.jk2_version = jk2_version;
        self.allow_asset_overrides = allow_asset_overrides;
        Ok(())
    }

    /// Identity of the shared namespace index this view reads. Two views give
    /// equal tokens only while nothing beneath their search directories (or
    /// the package policy) has changed, so results derived purely from the VFS
    /// can be cached against it.
    pub fn index_token(&self) -> VfsIndexToken {
        VfsIndexToken(Arc::downgrade(&self.shared))
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.shared.entries.iter().map(|entry| entry.key.as_str())
    }

    /// Locate a stored member without loading its payload. Nested archives use
    /// this to seek directly into the package through the same VFS priority.
    pub fn stored_location(&mut self, name: &str) -> Result<Option<(PathBuf, Option<(u64,u64)>)>, Box<dyn Error>> {
        validate_asset_name(name)?;
        let Some(provider)=self.provider_for_ordinary_read(&normalize_asset_name(name)) else { return Ok(None) };
        match provider {
            Provider::Loose { path } => Ok(Some((path,None))),
            Provider::Archive { archive, entry } => {
                let path=self.shared.archive_paths[archive].clone();
                if let Some(pure) = &self.shared.jk2_pure { pure.record_read(&path, name); }
                let file=self.archive_handle(archive)?.by_index(entry)?;
                if normalize_asset_name(file.name()) != normalize_asset_name(name) { return Err("package changed since indexing".into()) }
                if file.compression()!=zip::CompressionMethod::Stored { return Err(format!("{name}: nested bundles must use ZIP Store").into()) }
                Ok(Some((path,Some((file.data_start().ok_or("package member has no offset")?,file.size())))))
            }
        }
    }

    /// Fast indexed existence check for an exact virtual qpath. This mirrors
    /// BG_FileExists-style lookups without opening the asset.
    pub fn contains_qpath(&self, name: &str) -> bool {
        validate_asset_name(name).is_ok()
            && self.shared.find(&normalize_asset_name(name)).is_some()
    }

    /// Read-only trace probe: physical provider indexed for this exact qpath.
    /// `stock_only` models the first pass of texture loading when asset
    /// overrides are disabled. The normal pass honors the usual VFS policy.
    /// Existence in the index does NOT mean reading or decoding will succeed.
    pub fn indexed_provider_path(&self, name: &str, stock_only: bool) -> Option<&Path> {
        validate_asset_name(name).ok()?;
        let entry = self.shared.find(&normalize_asset_name(name))?;
        let provider = if self.shared.jk2_pure.is_some() {
            &entry.first
        } else if stock_only {
            entry.providers().find(|provider| self.provider_is_stock(provider))?
        } else if !self.allow_asset_overrides {
            entry
                .providers()
                .find(|provider| self.provider_is_stock(provider))
                .unwrap_or(&entry.first)
        } else {
            &entry.first
        };
        match provider {
            Provider::Archive { archive, .. } => Some(self.shared.archive_paths[*archive].as_path()),
            Provider::Loose { path } => Some(path.as_path()),
        }
    }

    /// Every indexed provider for a qpath in virtual filesystem priority order.
    /// Unlike reading a file, this only consults the shared PK3/loose index.
    /// The first entry is the ordinary winner when overrides are enabled.
    pub fn indexed_providers(&self, name: &str) -> Vec<AssetProviderInfo> {
        if validate_asset_name(name).is_err() { return Vec::new(); }
        let Some(entry) = self.shared.find(&normalize_asset_name(name)) else {
            return Vec::new();
        };
        let selected = self.provider_for_ordinary_read(&entry.key);
        entry.providers().map(|provider| {
            let (source, archive) = match provider {
                Provider::Archive { archive, .. } => (self.shared.archive_paths[*archive].clone(), true),
                Provider::Loose { path } => (path.clone(), false),
            };
            AssetProviderInfo {
                source,
                archive,
                selected: selected.as_ref().is_some_and(|winner| match (winner, provider) {
                    (Provider::Archive { archive: a, entry: b }, Provider::Archive { archive: c, entry: d }) => a == c && b == d,
                    (Provider::Loose { path: a }, Provider::Loose { path: b }) => a == b,
                    _ => false,
                }),
            }
        }).collect()
    }

    /// Physical loose file selected by ordinary asset lookup, when the winning
    /// provider is editable on disk. Packaged assets return `None`.
    pub fn loose_file_path(&self, name: &str) -> Option<PathBuf> {
        validate_asset_name(name).ok()?;
        match self.provider_for_ordinary_read(&normalize_asset_name(name))? {
            Provider::Loose { path } => Some(path),
            Provider::Archive { .. } => None,
        }
    }

    /// Control whether ordinary VFS reads may shadow qpaths present in the
    /// retail JKA `assets0.pk3` through `assets3.pk3` archives. This does not
    /// affect [`Self::read_from_source`], because an active material explicitly
    /// requesting a dependency from its own provider is not a global override.
    pub fn set_allow_asset_overrides(&mut self, allow: bool) {
        self.allow_asset_overrides = allow;
    }

    pub fn asset_overrides_allowed(&self) -> bool {
        self.shared.jk2_pure.is_some() || self.allow_asset_overrides
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

    /// Read only a file's prefix through the ordinary VFS provider policy.
    /// Useful for catalog metadata: large animation payloads remain on disk.
    pub fn read_prefix(&mut self,name:&str,limit:usize)->Result<Option<Asset>,Box<dyn Error>> {
        validate_asset_name(name)?;
        let Some(provider)=self.provider_for_ordinary_read(&normalize_asset_name(name)) else {return Ok(None);};
        let (bytes,source)=match provider {
            Provider::Archive {archive,entry}=>{
                let source=self.shared.archive_paths[archive].clone();
                let file=self.archive_handle(archive)?.by_index(entry)?;
                if normalize_asset_name(file.name())!=normalize_asset_name(name) {return Err("archive entry changed since indexing".into());}
                let mut bytes=Vec::new();file.take(limit as u64).read_to_end(&mut bytes)?;(bytes,source)
            }
            Provider::Loose {path}=>{
                let mut bytes=Vec::new();File::open(&path)?.take(limit as u64).read_to_end(&mut bytes)?;(bytes,path)
            }
        };
        if let Some(pure) = &self.shared.jk2_pure {
            if pure.allows_package(&source) { pure.record_read(&source, name); }
        }
        Ok(Some(Asset {bytes,source}))
    }

    fn stock_provider(&self, key: &str) -> Option<Provider> {
        // A server's pure namespace owns precedence, including the renderer's
        // optional stock-first texture pass. Every indexed provider is allowed.
        if self.shared.jk2_pure.is_some() { return self.shared.find(key).map(|e| e.first.clone()); }
        self.shared
            .find(key)?
            .providers()
            .find(|provider| self.provider_is_stock(provider))
            .cloned()
    }

    fn provider_for_ordinary_read(&self, key: &str) -> Option<Provider> {
        if self.shared.jk2_pure.is_none() && !self.allow_asset_overrides {
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
                let read_result = (&mut file).take(limit as u64 + 1).read_to_end(&mut bytes);
                // Release the first ZipFile before reopening the same entry on
                // the same archive handle. Ordinary reads keep CRC validation.
                drop(file);
                let ignored_crc = match read_result {
                    Ok(_) => false,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::InvalidData
                            && error.to_string() == "Invalid checksum" =>
                    {
                        // zip 8.6.0 rejects mismatched ZIP CRCs at the end of
                        // an otherwise successful read. TaystJK/MiniZip keeps
                        // those decompressed bytes. Retry only this specific
                        // error without CRC validation; decompression errors
                        // and expanded-size checks still fail as normal.
                        bytes.clear();
                        let mut retry = handle.by_index_with_options(
                            entry,
                            zip::read::ZipReadOptions::new().ignore_crc32(true),
                        )?;
                        (&mut retry).take(limit as u64 + 1).read_to_end(&mut bytes)?;
                        true
                    }
                    Err(error) => return Err(error.into()),
                };
                if bytes.len() > limit || bytes.len() as u64 != declared {
                    return Err("invalid expanded asset size".into());
                }
                if ignored_crc {
                    log::warn!(
                        "PK3 CRC mismatch ignored: {name} in {} (decompression and size OK)",
                        shared.archive_paths[archive].display()
                    );
                }
                if let Some(pure) = &shared.jk2_pure { pure.record_read(&shared.archive_paths[archive], name); }
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
/// directory contributes its PK3/PK4 packages (later-sorting names first),
/// then its
/// loose files.
enum Source {
    Archive { path: PathBuf, size: u64 },
    Loose(PathBuf, Option<Arc<crate::jk2_pure::Jk2PurePolicy>>),
}

enum Scanned {
    /// The opened archive and its `(normalized key, zip entry number)` pairs,
    /// sorted by key.
    Archive(zip::ZipArchive<File>, Vec<(String, usize)>),
    /// `(normalized key, path)` pairs, sorted by key.
    Loose(Vec<(String, PathBuf)>),
}

fn plan_sources(dirs: &[PathBuf], pure: &Option<Arc<crate::jk2_pure::Jk2PurePolicy>>) -> Result<Vec<Source>, Box<dyn Error>> {
    // One consistent mount policy for this index build, even when the menu
    // applies a change while a background map worker is indexing packages.
    let disabled = disabled_package_paths();
    let incompatible = package_policy().read().unwrap_or_else(PoisonError::into_inner).incompatible.clone();
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
            if entry.file_type()?.is_file() && is_archive_package(&path)
                && !incompatible.contains(&path) && (is_stock_asset_archive(&path) || !disabled.contains(&path))
                && pure.as_ref().is_none_or(|policy| policy.allows_package(&path)) {
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
        if !is_package_only_dir(dir) {
            sources.push(Source::Loose(dir.clone(), pure.clone()));
        }
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
        Source::Loose(root, pure) => {
            let mut files = Vec::<(String, PathBuf)>::new();
            collect_loose_files(root, root, &mut files).map_err(|error| error.to_string())?;
            if let Some(pure) = pure { files.retain(|(key, _)| pure.allows_loose(key)); }
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
        Source::Loose(..) => std::cmp::Reverse(u64::MAX),
        Source::Archive { size, .. } => std::cmp::Reverse(*size),
    });
    let next = AtomicUsize::new(0);
    let results = (0..count).map(|_| Mutex::new(None)).collect::<Vec<_>>();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            std::thread::Builder::new().name("jka-pk3-index".into()).spawn_scoped(scope, || loop {
                let slot = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(&index) = order.get(slot) else {
                    break;
                };
                let scanned = scan_source(&sources[index]);
                *results[index].lock().unwrap_or_else(PoisonError::into_inner) = Some(scanned);
            }).expect("spawn named worker");
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
    let pure = package_policy().read().unwrap_or_else(PoisonError::into_inner).jk2_pure.clone()
        .filter(|pure| pure.directories == dirs);
    build_index_with_pure(dirs, pure)
}

fn build_index_with_pure(dirs: &[PathBuf], pure: Option<Arc<crate::jk2_pure::Jk2PurePolicy>>) -> Result<(VfsIndex, ArchiveHandles), Box<dyn Error>> {
    let sources = plan_sources(dirs, &pure)?;
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
            jk2_pure: pure,
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
    // A checkbox change invalidates the shared index even when no filesystem
    // modification time changes. Existing readers keep their old Arc until refreshed.
    package_policy().read().unwrap_or_else(PoisonError::into_inner)
        .generation.hash(&mut hasher);
    for dir in dirs {
        dir.hash(&mut hasher);
        fingerprint_tree(dir, true, !is_package_only_dir(dir), &mut hasher, &mut newest);
    }
    let racy = SystemTime::now()
        .duration_since(newest)
        .map_or(true, |age| age < RACY_WINDOW);
    Fingerprint {
        hash: hasher.finish(),
        racy,
    }
}

fn fingerprint_tree(dir: &Path, top: bool, recurse: bool, hasher: &mut DefaultHasher, newest: &mut SystemTime) {
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
        if file_type.is_symlink() || (file_type.is_dir() && !recurse) {
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
            fingerprint_tree(&dir.join(name), false, true, hasher, newest);
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
    let build_generation = package_policy().read().unwrap_or_else(PoisonError::into_inner).generation;
    let (mut index, mut handles) = build_index(dirs)?;
    let mut fingerprint = fingerprint;
    // If Apply raced the scan, retry against the new generation. Avoid walking
    // the entire filesystem a second time unless that race actually happened.
    if package_policy().read().unwrap_or_else(PoisonError::into_inner).generation != build_generation {
        fingerprint = fingerprint_search_dirs(dirs);
        (index, handles) = build_index(dirs)?;
    }
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
                "assets0.pk3" | "assets1.pk3" | "assets2.pk3" | "assets3.pk3" | "assets5.pk3"
            )
        })
}

fn is_archive_package(path: &Path) -> bool {
    if path
        .extension()
        .is_some_and(|ext| {
            let ext = ext.to_string_lossy();
            ext.eq_ignore_ascii_case("pk3") || ext.eq_ignore_ascii_case("pk4")
        })
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
    #[test]
    fn jk2_pure_filters_every_read_route_and_keeps_policy_identity() {
        use crate::jk2_pure::{parse_checksums, Jk2PurePolicy};
        let root = test_root("jk2-pure-policy");
        let base = root.join("base"); let game = root.join("mod");
        fs::create_dir_all(&base).unwrap(); fs::create_dir_all(&game).unwrap();
        let stock = base.join("assets0.pk3"); let patch = base.join("assets5.pk3");
        let allowed = base.join("z_allowed.pk3"); let duplicate = game.join("override.pk3");
        let forbidden = base.join("zz_forbidden.pk3"); let companion = base.join("zzz_companion.zip");
        write_package_entries(&stock, &[("vm/cgame.qvm", b"cgame"), ("vm/ui.qvm", b"ui"), ("maps/test.bsp", b"map"), ("image.jpg", b"stock")]);
        write_package_entries(&patch, &[("vm/cgame.qvm", b"patch-cgame"), ("vm/ui.qvm", b"patch-ui")]);
        write_package(&allowed, "image.jpg", b"allowed");
        write_package(&duplicate, "image.jpg", b"allowed");
        write_package_entries(&forbidden, &[("vm/cgame.qvm", b"wrong-module"), ("image.jpg", b"wrong-image")]);
        write_package(&companion, "image.jpg", b"companion");
        fs::write(base.join("loose.jpg"), b"not allowed").unwrap();
        fs::write(base.join("local.cfg"), b"local setting").unwrap();
        let sums = [&stock, &patch, &allowed].map(|path| crate::pak_checksum::pk3_checksum(path).unwrap());
        let dirs = vec![game.clone(), base.clone()];
        let policy = Arc::new(Jk2PurePolicy::resolve(dirs.clone(), 2, 1234, sums.to_vec()).unwrap());
        // Like JKA, local packages the server did not list still mount; they are
        // just never reported (`reply` below only names listed packages).
        assert!(policy.allows_package(&patch));
        assert!(policy.allows_package(&forbidden));
        let (index, handles) = build_index_with_pure(&dirs, Some(Arc::clone(&policy))).unwrap();
        let mut assets = AssetSearchPath::from_shared(Arc::new(index), Some(handles));
        assets.set_allow_asset_overrides(false);
        assert_eq!(assets.read("image.jpg", 64).unwrap().unwrap().source, duplicate);
        assert_eq!(assets.read_stock("image.jpg", 64).unwrap().unwrap().bytes, b"allowed");
        assert!(assets.read("loose.jpg", 64).unwrap().is_some());
        assert!(assets.contains_qpath("loose.jpg"));
        assert_eq!(assets.read_from_source("image.jpg", 64, &forbidden).unwrap().unwrap().bytes, b"wrong-image");
        assert!(assets.read_from_source("image.jpg", 64, &companion).unwrap().is_none());
        assert_eq!(assets.read_prefix("local.cfg", 5).unwrap().unwrap().bytes, b"local");
        // The unlisted package shadows the module locally, but the reply below
        // still reports the listed stock package that provides it.
        assert_eq!(assets.read("vm/cgame.qvm", 64).unwrap().unwrap().source, forbidden);
        assert_eq!(assets.fork().jk2_version(), Some(2));
        let reply = policy.reply(&assets, "test").unwrap();
        assert_eq!(reply.reported_paks, 2, "duplicate package checksum is reported once");
        let stock_pure = crate::pak_checksum::pk3_pure_checksum(&stock, 1234).unwrap();
        let addon_pure = crate::pak_checksum::pk3_pure_checksum(&allowed, 1234).unwrap();
        assert_eq!(String::from_utf8(reply.command).unwrap(), format!("cp {stock_pure} {stock_pure} @ {addon_pure} {stock_pure} {}", (1234u32 ^ addon_pure as u32 ^ stock_pure as u32 ^ 2) as i32));
        let next = Arc::new(Jk2PurePolicy::resolve(dirs.clone(), 4, 5678, sums.to_vec()).unwrap());
        let (next_index, next_handles) = build_index_with_pure(&dirs, Some(Arc::clone(&next))).unwrap();
        let mut next_assets = AssetSearchPath::from_shared(Arc::new(next_index), Some(next_handles));
        assert_eq!(next_assets.read("vm/cgame.qvm", 64).unwrap().unwrap().source, forbidden);
        assert!(next.reply(&assets, "test").is_err(), "old readers cannot produce a new-policy reply");
        assert_eq!(assets.read("vm/cgame.qvm", 64).unwrap().unwrap().source, forbidden);
        let missing = Arc::new(Jk2PurePolicy::resolve(dirs.clone(), 4, 5678, vec![sums[2]]).unwrap());
        let (missing_index, missing_handles) = build_index_with_pure(&dirs, Some(Arc::clone(&missing))).unwrap();
        let missing_assets = AssetSearchPath::from_shared(Arc::new(missing_index), Some(missing_handles));
        assert!(missing.reply(&missing_assets, "test").unwrap_err().contains("vm/cgame.qvm"));
        assert!(parse_checksums(b"1 -2 nope").is_err());
        assert!(parse_checksums(b"").is_err());
        assert_eq!(parse_checksums(b"1 -2 3").unwrap(), [1, -2, 3]);
        drop(missing_assets); drop(next_assets); drop(assets);
        // Only remove exact files created by this fixture; no recursive cleanup.
        for path in [&stock, &patch, &allowed, &duplicate, &forbidden, &companion, &base.join("loose.jpg"), &base.join("local.cfg")] { fs::remove_file(path).unwrap(); }
        fs::remove_dir(game).unwrap(); fs::remove_dir(base).unwrap(); fs::remove_dir(root).unwrap();
    }
    use std::io::Write;

    #[test]
    fn cinematic_metadata_prefix_reads_preserve_vfs_precedence_without_loading_payloads() {
        let root=test_root("cinematic-prefix");let base=root.join("base");let game=root.join("game");
        fs::create_dir_all(&base).unwrap();fs::create_dir_all(&game).unwrap();
        let qpath="models/players/_humanoid_test/_humanoid_test.gla";
        write_package(&base.join("assets1.pk3"),qpath,b"stock-header-large-payload");
        write_package(&game.join("override.pk3"),qpath,b"mod-header-large-payload");
        fs::write(game.join("loose.bin"),b"loose-header-large-payload").unwrap();
        let mut assets=AssetSearchPath::open_game(&base,Some(&game)).unwrap();
        assert!(assets.read(qpath,5).is_err());
        assert_eq!(assets.read_prefix(qpath,5).unwrap().unwrap().bytes,b"mod-h");
        assets.set_allow_asset_overrides(false);
        assert_eq!(assets.read_prefix(qpath,5).unwrap().unwrap().bytes,b"stock");
        assert_eq!(assets.fork().read_prefix(qpath,5).unwrap().unwrap().bytes,b"stock");
        assert_eq!(assets.read_prefix("loose.bin",5).unwrap().unwrap().bytes,b"loose");
        assert!(assets.read_prefix("missing.gla",100).unwrap().is_none());
        drop(assets);
        // Delete only the exact fixture files; no recursive workspace cleanup.
        fs::remove_file(game.join("loose.bin")).unwrap();fs::remove_file(game.join("override.pk3")).unwrap();
        fs::remove_file(base.join("assets1.pk3")).unwrap();fs::remove_dir(game).unwrap();fs::remove_dir(base).unwrap();fs::remove_dir(root).unwrap();
    }

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

    /// Simulate a PK3 whose ZIP CRC metadata is wrong but whose compressed
    /// bytes are intact. Change both local and central header values so the
    /// archive itself is still structurally consistent.
    fn break_first_entry_crc(path: &Path) {
        let mut contents = fs::read(path).unwrap();
        let local = contents.windows(4).position(|s| s == b"PK\x03\x04").unwrap();
        let central = contents.windows(4).position(|s| s == b"PK\x01\x02").unwrap();
        let actual = u32::from_le_bytes(contents[central + 16..central + 20].try_into().unwrap());
        let wrong = (actual ^ 0xffff_ffff).to_le_bytes();
        contents[local + 14..local + 18].copy_from_slice(&wrong);
        contents[central + 16..central + 20].copy_from_slice(&wrong);
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn bad_zip_crc_with_valid_compressed_data_is_readable() {
        let root = test_root("bad-entry-crc");
        let base = root.join("base");
        fs::create_dir_all(&base).unwrap();
        let package = base.join("bad_crc.pk3");
        let qpath = "textures/test/crc.jpg";
        let image_bytes = b"intact texture payload in a PK3 with a bad CRC";
        write_package(&package, qpath, image_bytes);
        break_first_entry_crc(&package);

        // The ZIP crate must still reject the checksum on its normal path.
        let mut plain_zip = zip::ZipArchive::new(File::open(&package).unwrap()).unwrap();
        let mut plain_file = plain_zip.by_index(0).unwrap();
        let mut plain_bytes = Vec::new();
        let err = plain_file.read_to_end(&mut plain_bytes).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(err.to_string(), "Invalid checksum");
        drop(plain_file);
        drop(plain_zip);

        // Our VFS retries CRC-only failures, keeping the actual image bytes.
        let mut assets = AssetSearchPath::open(&base).unwrap();
        let loaded = assets.read(qpath, 1024).unwrap().unwrap();
        assert_eq!(loaded.bytes, image_bytes);
        assert_eq!(loaded.source, package);
        // Even CRC-tolerant reads still enforce the expanded-size limit.
        assert!(assets.read(qpath, image_bytes.len() - 1).is_err());
        fs::remove_dir_all(root).unwrap();
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
    fn pk4_is_the_same_zip_package_as_pk3_with_unchanged_priority() {
        let root = test_root("pk4-package-priority");
        let base = root.join("base");
        let game = root.join("japro");
        fs::create_dir_all(&base).unwrap();
        fs::create_dir_all(&game).unwrap();
        let qpath = "models/players/example/model.glm";
        let base_pk3 = base.join("shared.pk3");
        let base_pk4 = base.join("shared.pk4");
        let mod_pk3 = game.join("addon.pk3");
        let mod_pk4 = game.join("z_enhanced.PK4");
        write_package(&base_pk3, qpath, b"base pk3");
        write_package(&base_pk4, qpath, b"base pk4");
        write_package(&mod_pk3, qpath, b"mod pk3");
        write_package_entries(&mod_pk4, &[
            (qpath, &b"mod pk4"[..]),
            ("models/players/example/enhanced.json", &b"enhanced metadata"[..]),
        ]);

        let mut assets = AssetSearchPath::open_search_dirs([&game, &base]).unwrap();
        assert_eq!(assets.read(qpath, 64).unwrap().unwrap().bytes, b"mod pk4");
        assert_eq!(assets.read("models/players/example/enhanced.json", 64).unwrap().unwrap().bytes,
            b"enhanced metadata");
        let providers = assets.indexed_providers(qpath);
        let paths = providers.iter().map(|info| info.source.as_path()).collect::<Vec<_>>();
        assert_eq!(paths, vec![mod_pk4.as_path(), mod_pk3.as_path(),
            base_pk4.as_path(), base_pk3.as_path()]);
        assert!(providers.iter().all(|info| info.archive));
        assert!(!assets.contains_qpath("shared.pk4"), "package files are mounts, not loose qpaths");
        assert!(is_archive_package(Path::new("example.PK4")));
        assert!(!is_archive_package(Path::new("example.zip")));
        drop(assets);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn disabling_package_and_refresh_reverts_to_lower_priority_provider() {
        let root = test_root("pk4-manager-remount");
        let base = root.join("base");
        fs::create_dir_all(&base).unwrap();
        let qpath = "textures/test/override.tga";
        let stock = base.join("assets0.pk3");
        let optional = base.join("zz_optional.pk4");
        write_package(&stock, qpath, b"stock");
        write_package(&optional, qpath, b"optional");
        let mut assets = AssetSearchPath::open(&base).unwrap();
        assert_eq!(assets.read(qpath, 32).unwrap().unwrap().bytes, b"optional");
        let mut disabled = HashSet::new();
        disabled.insert(optional.clone());
        apply_disabled_packages(&base, None, &disabled).unwrap();
        assert!(!package_enabled(&optional));
        assert!(package_enabled(&stock));
        assert_eq!(installed_packages(&base, None).len(), 2);
        // Existing index stays valid until its owner explicitly refreshes.
        assert_eq!(assets.read(qpath, 32).unwrap().unwrap().bytes, b"optional");
        assets.refresh().unwrap();
        assert_eq!(assets.read(qpath, 32).unwrap().unwrap().bytes, b"stock");
        assert!(apply_disabled_packages(&base, None, &HashSet::from([stock.clone()])).is_err());
        apply_disabled_packages(&base, None, &HashSet::new()).unwrap();
        assets.refresh().unwrap();
        assert_eq!(assets.read(qpath, 32).unwrap().unwrap().bytes, b"optional");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn which_index_lists_active_and_shadowed_pk3_sources() {
        let root = test_root("which-provenance");
        let base = root.join("base");
        let mod_dir = root.join("whichmod");
        fs::create_dir_all(&base).unwrap();
        fs::create_dir_all(&mod_dir).unwrap();
        let qpath = "models/players/test/model_default.skin";
        write_package(&base.join("assets0.pk3"), qpath, b"retail skin");
        write_package(&mod_dir.join("custom.pk3"), qpath, b"custom skin");
        let mut assets = AssetSearchPath::open_game(&base, Some(&mod_dir)).unwrap();
        let found = assets.indexed_providers("MODELS/PLAYERS/TEST/MODEL_DEFAULT.SKIN");
        assert_eq!(found.len(), 2);
        assert!(found[0].source.ends_with("custom.pk3") && found[0].archive && found[0].selected);
        assert!(found[1].source.ends_with("assets0.pk3") && !found[1].selected);
        assets.set_allow_asset_overrides(false);
        let stock = assets.indexed_providers(qpath);
        assert!(!stock[0].selected && stock[1].selected);
        assert!(assets.indexed_providers("../secret").is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn trace_index_provider_respects_stock_first_and_never_reads() {
        let root = test_root("trace-provider-priority");
        let base = root.join("base");
        let mod_dir = root.join("testmod");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&mod_dir).unwrap();
        write_package(&base.join("assets0.pk3"), "textures/test/base.tga", b"stock");
        write_package(&mod_dir.join("override.pk3"), "textures/test/base.tga", b"mod");
        let mut assets = AssetSearchPath::open_game(&base, Some(&mod_dir)).unwrap();
        assert!(assets.indexed_provider_path("textures/test/base.tga", false)
            .unwrap().ends_with("override.pk3"));
        assert!(assets.indexed_provider_path("textures/test/base.tga", true)
            .unwrap().ends_with("assets0.pk3"));
        assets.set_allow_asset_overrides(false);
        assert!(assets.indexed_provider_path("textures/test/base.tga", false)
            .unwrap().ends_with("assets0.pk3"));
        assert!(assets.indexed_provider_path("textures/test/missing.tga", false).is_none());
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

#[cfg(test)]
mod filesystem_roots_tests {
    use super::*;

    #[test]
    fn four_layers_and_both_game_roots_are_isolated() {
        let base = Path::new("/client/JKA GameData/base");
        let game = Path::new("/client/JKA GameData/jk2");
        let roots = FilesystemRoots {
            cdpath: Some("/retail-jka".into()), jk2cdpath: Some("/retail-jk2".into()),
            ..Default::default()
        };
        assert_eq!(filesystem_search_dirs(base, Some(game), &roots), vec![
            PathBuf::from("/client/DinurdoJK/shared"),
            PathBuf::from("/client/JKA GameData/jk2"),
            PathBuf::from("/client/JKA GameData/base"),
            PathBuf::from("/retail-jka/base"),
        ]);
        let jk2 = FilesystemRoots { active_jk2: Some(4), ..roots };
        let same_mod = Path::new("/client/JK2 GameData/jk2");
        assert_eq!(filesystem_search_dirs(base, Some(same_mod), &jk2), vec![
            PathBuf::from("/client/DinurdoJK/shared"),
            PathBuf::from("/client/JK2 GameData/jk2"),
            PathBuf::from("/client/JK2 GameData/base"),
            PathBuf::from("/retail-jk2/base"),
        ]);
        assert_eq!(jk2_base_directory(base, &jk2), PathBuf::from("/client/JK2 GameData/base"));
        assert_eq!(filesystem_write_directory(base, Some(same_mod), &jk2), same_mod);
    }

    #[test]
    fn custom_jk2_root_is_a_root_not_a_base_folder() {
        let base = Path::new("/client/JKA GameData/base");
        let roots = FilesystemRoots {
            jk2basepath: Some("/external/outcast".into()), active_jk2: Some(2),
            homepath: Some("/home".into()), jk2cdpath: Some("/cd".into()),
            forcegame: Some("my-mod".into()), ..Default::default()
        };
        assert_eq!(jk2_base_directory(base, &roots), PathBuf::from("/external/outcast/base"));
        assert_eq!(filesystem_search_dirs(base, Some(Path::new("/external/outcast/jk2")), &roots), vec![
            PathBuf::from("/client/DinurdoJK/my-mod"),
            PathBuf::from("/client/DinurdoJK/shared"),
            PathBuf::from("/home/JK2 GameData/jk2"), PathBuf::from("/external/outcast/jk2"),
            PathBuf::from("/home/JK2 GameData/base"), PathBuf::from("/external/outcast/base"),
            PathBuf::from("/cd/base"),
        ]);
        assert_eq!(filesystem_write_directory(base, None, &roots), PathBuf::from("/home/JK2 GameData/base"));
        assert_eq!(filesystem_game_layer_dirs(base, None, &roots), vec![
            PathBuf::from("/home/JK2 GameData/base"), PathBuf::from("/external/outcast/base"),
        ]);
    }

    #[test]
    fn forcegame_is_a_client_override_above_shared_without_redirecting_writes() {
        let base = Path::new("/client/JKA GameData/base");
        let roots = FilesystemRoots {
            forcegame: Some("Eternal".into()),
            cdpath: Some("/jka-cd".into()), jk2cdpath: Some("/jk2-cd".into()),
            ..Default::default()
        };
        let jka_mod = Path::new("/client/JKA GameData/japro");
        assert_eq!(filesystem_search_dirs(base, Some(jka_mod), &roots), vec![
            PathBuf::from("/client/DinurdoJK/Eternal"),
            PathBuf::from("/client/DinurdoJK/shared"),
            PathBuf::from("/client/JKA GameData/japro"),
            PathBuf::from("/client/JKA GameData/base"),
            PathBuf::from("/jka-cd/base"),
        ]);
        assert_eq!(filesystem_write_directory(base, Some(jka_mod), &roots), jka_mod);
        let jk2 = FilesystemRoots { active_jk2: Some(4), ..roots };
        let jk2_mod = Path::new("/client/JK2 GameData/jk2");
        assert_eq!(filesystem_search_dirs(base, Some(jk2_mod), &jk2), vec![
            PathBuf::from("/client/DinurdoJK/Eternal"),
            PathBuf::from("/client/DinurdoJK/shared"),
            PathBuf::from("/client/JK2 GameData/jk2"),
            PathBuf::from("/client/JK2 GameData/base"),
            PathBuf::from("/jk2-cd/base"),
        ]);
        assert_eq!(filesystem_write_directory(base, Some(jk2_mod), &jk2), jk2_mod);
    }

    #[test]
    fn literal_mod_names_are_not_reserved_or_escaped() {
        let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let install = std::env::temp_dir().join(format!("dinurdo-game-roots-{}-{unique}", std::process::id()));
        let base = install.join("JKA GameData/base");
        std::fs::create_dir_all(&base).unwrap();
        for name in ["jk2", "jk2_base", "jka_foo", "DinurdoJK"] {
            std::fs::create_dir_all(install.join("JKA GameData").join(name)).unwrap();
            std::fs::create_dir_all(install.join("JK2 GameData").join(name)).unwrap();
            assert_eq!(resolve_fs_game_directory_for_roots(&base, name.as_bytes(), &FilesystemRoots::default()).unwrap(),
                Some(install.join("JKA GameData").join(name)));
            let jk2 = FilesystemRoots { active_jk2: Some(2), ..Default::default() };
            assert_eq!(resolve_fs_game_directory_for_roots(&base, name.as_bytes(), &jk2).unwrap(),
                Some(install.join("JK2 GameData").join(name)));
        }
        assert!(resolve_fs_game_directory_for_roots(&base, b"../escape", &FilesystemRoots::default()).is_err());
        std::fs::remove_dir_all(install).unwrap();
    }

    #[test]
    fn stock_check_combines_only_jk2_local_and_cd_base() {
        let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let install = std::env::temp_dir().join(format!("dinurdo-stock-roots-{}-{unique}", std::process::id()));
        let base = install.join("JKA GameData/base");
        let local = install.join("JK2 GameData/base");
        let retail = install.join("retail/base");
        for dir in [&base, &local, &retail] { std::fs::create_dir_all(dir).unwrap(); }
        std::fs::write(base.join("assets0.pk3"), b"jka").unwrap();
        let roots = FilesystemRoots { jk2cdpath: Some(install.join("retail")), ..Default::default() };
        assert_eq!(missing_jk2_stock_archive(&base, &roots, 2), Some("assets0.pk3"));
        std::fs::write(local.join("assets0.pk3"), b"jk2").unwrap();
        std::fs::write(retail.join("assets1.pk3"), b"jk2").unwrap();
        assert_eq!(missing_jk2_stock_archive(&base, &roots, 2), None);
        assert_eq!(missing_jk2_stock_archive(&base, &roots, 4), Some("assets2.pk3"));
        std::fs::write(local.join("assets2.pk3"), b"jk2").unwrap();
        std::fs::write(retail.join("assets5.pk3"), b"jk2").unwrap();
        assert_eq!(missing_jk2_stock_archive(&base, &roots, 4), None);
        std::fs::remove_dir_all(install).unwrap();
    }

    #[test]
    fn shared_accepts_loose_files_without_scanning_client_cache() {
        let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let install = std::env::temp_dir().join(format!("dinurdo-shared-root-{}-{unique}", std::process::id()));
        let shared = install.join("DinurdoJK/shared");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("loose.txt"), b"content").unwrap();
        let mut assets = AssetSearchPath::open_search_dirs([shared]).unwrap();
        assert_eq!(assets.read("loose.txt", 1024).unwrap().unwrap().bytes, b"content");
        std::fs::remove_dir_all(install).unwrap();
    }
}
