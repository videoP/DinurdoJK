//! Read-only JKA virtual asset search path with PK3, Rend2 companion ZIP, and loose-file support.
//!
//! Search order follows the useful core of JKA/OpenJK semantics for this client:
//! the active game/mod directory is searched before base, and within each game
//! directory PK3 files take precedence over loose files. Later-sorting PK3 names
//! override earlier ones. Every caller uses package-relative qpaths such as
//! `maps/mp/ffa3.bsp`, `textures/foo/bar.tga`, or `shaders/common.shader`.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
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
    Archive { archive: usize, entry: String },
    Loose { path: PathBuf },
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
    archives: Vec<(PathBuf, zip::ZipArchive<File>)>,
    index: BTreeMap<String, Provider>,
    /// Normal JKA/OpenJK VFS behavior allows later/higher-priority packages to
    /// shadow stock assets. When false, ordinary reads prefer the stock
    /// assets0..assets3 provider whenever the qpath exists there. Explicit
    /// material-source reads intentionally bypass this protection.
    allow_asset_overrides: bool,
    /// Every provider for a qpath in normal VFS priority order. `index` keeps
    /// the fast winning-provider lookup while this list lets material-aware
    /// callers deliberately fall through one package without changing global
    /// JKA search-path semantics.
    providers: BTreeMap<String, Vec<Provider>>,
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

    pub fn open_search_dirs<I, P>(dirs: I) -> Result<Self, Box<dyn std::error::Error>>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut result = Self {
            archives: Vec::new(),
            index: BTreeMap::new(),
            allow_asset_overrides: true,
            providers: BTreeMap::new(),
        };
        for dir in dirs {
            result.mount_directory(dir.as_ref())?;
        }
        Ok(result)
    }

    fn mount_directory(&mut self, dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };

        let mut packages = Vec::new();
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_file() && is_archive_package(&path) {
                packages.push(path);
            }
        }
        packages.sort_by(|a, b| {
            a.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("")
                .to_ascii_lowercase()
                .cmp(
                    &b.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase(),
                )
                .reverse()
        });
        for path in packages {
            self.mount_archive(path)?;
        }

        // JKA's normal search path checks the loose directory after that game
        // directory's PK3s, but before falling through to the lower-priority
        // base game. Indexing the directory here gives every asset class the
        // same behavior without renderer-specific filesystem fallbacks.
        self.mount_loose_files(dir)?;
        Ok(())
    }

    fn mount_archive(&mut self, path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
        let file = File::open(&path)?;
        let archive = zip::ZipArchive::new(file)?;
        let archive_index = self.archives.len();
        let mut seen = BTreeSet::new();
        for name in archive.file_names() {
            let key = normalize_asset_name(name);
            if !seen.insert(key.clone()) {
                return Err(format!(
                    "ambiguous case-insensitive entry {name} in {}",
                    path.display()
                )
                .into());
            }
            // Higher-priority search directories and later-sorting packages are
            // mounted first, so the first provider of an asset wins.
            let provider = Provider::Archive {
                archive: archive_index,
                entry: name.to_owned(),
            };
            self.providers
                .entry(key.clone())
                .or_default()
                .push(provider.clone());
            self.index.entry(key).or_insert(provider);
        }
        self.archives.push((path, archive));
        Ok(())
    }

    fn mount_loose_files(&mut self, root: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let mut files = Vec::<(String, PathBuf)>::new();
        collect_loose_files(root, root, &mut files)?;
        files.sort_by(|a, b| a.0.cmp(&b.0));

        let mut seen = BTreeSet::new();
        for (key, path) in files {
            if !seen.insert(key.clone()) {
                return Err(format!(
                    "ambiguous case-insensitive loose asset {key} under {}",
                    root.display()
                )
                .into());
            }
            let provider = Provider::Loose { path };
            self.providers
                .entry(key.clone())
                .or_default()
                .push(provider.clone());
            self.index.entry(key).or_insert(provider);
        }
        Ok(())
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.index.keys().map(String::as_str)
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
        let provider = self
            .providers
            .get(&normalize_asset_name(name))
            .and_then(|providers| providers.iter().find(|provider| self.provider_is_stock(provider)))
            .cloned();
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

    fn provider_for_ordinary_read(&self, key: &str) -> Option<Provider> {
        if !self.allow_asset_overrides {
            if let Some(stock) = self
                .providers
                .get(key)
                .and_then(|providers| providers.iter().find(|provider| self.provider_is_stock(provider)))
            {
                return Some(stock.clone());
            }
        }
        self.index.get(key).cloned()
    }

    fn provider_is_stock(&self, provider: &Provider) -> bool {
        match provider {
            Provider::Archive { archive, .. } => is_stock_asset_archive(&self.archives[*archive].0),
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
        let providers = self
            .providers
            .get(&normalize_asset_name(name))
            .cloned()
            .unwrap_or_default();
        for provider in providers {
            if self.provider_matches_source(&provider, source) {
                return self.read_provider(name, limit, provider).map(Some);
            }
        }
        Ok(None)
    }

    fn provider_matches_source(&self, provider: &Provider, source: &Path) -> bool {
        match provider {
            Provider::Archive { archive, .. } => self.archives[*archive].0.as_path() == source,
            Provider::Loose { path } => path.as_path() == source,
        }
    }

    fn read_provider(
        &mut self,
        name: &str,
        limit: usize,
        provider: Provider,
    ) -> Result<Asset, Box<dyn std::error::Error>> {
        match provider {
            Provider::Archive { archive, entry } => {
                let (path, archive) = &mut self.archives[archive];
                let mut file = archive.by_name(&entry)?;
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
                    source: path.clone(),
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
