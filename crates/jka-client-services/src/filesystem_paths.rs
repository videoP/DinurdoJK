//! Filesystem roots in <install>/DinurdoJK/DinurdoJK-paths.ini.
//! The client data folder is a sibling of both game installations.
use std::path::{Path, PathBuf};
use jka_assets::pk3::FilesystemRoots;

const FILE: &str = "DinurdoJK-paths.ini";

/// With no fs_basepath configured the executable directory is the JKA game
/// root, so the default base is `<exe>/base`. fs_cdpath/base is the optional
/// read-only fallback; a missing install opens the Asset Mapping screen.
pub fn default_base_directory() -> Result<PathBuf, String> {
    Ok(installation_directory()?.join("base"))
}

/// Stable client-owned path root, derived from the executable, not from a game.
pub fn installation_directory() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    Ok(executable.parent().ok_or("Cannot locate executable")?.to_path_buf())
}

/// Lowercased components, so containment is case-insensitive like Windows.
fn components_lower(path: &Path) -> Vec<String> {
    path.components().map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase()).collect()
}

/// `inner` is `outer` or lies beneath it.
fn is_within(inner: &Path, outer: &Path) -> bool {
    let (inner, outer) = (components_lower(inner), components_lower(outer));
    inner.starts_with(&outer)
}

/// Canonicalize the deepest existing ancestor and re-append the rest, so paths
/// that do not exist yet (a fresh install has no base/) still compare against
/// existing ones in the same spelling. Plain `canonicalize` fails on them and
/// would leave `\\?\C:\x` and `C:\x` forms that never match.
fn normalize(path: &Path) -> PathBuf {
    let mut existing = path.to_path_buf();
    let mut tail = Vec::new();
    loop {
        if let Ok(real) = existing.canonicalize() {
            return tail.iter().rev().fold(real, |acc, part| acc.join(part));
        }
        match (existing.file_name().map(|n| n.to_os_string()), existing.parent()) {
            (Some(name), Some(parent)) => { tail.push(name); existing = parent.to_path_buf(); }
            _ => return path.to_path_buf(),
        }
    }
}

/// Keep JKA, JK2 and the client-owned DinurdoJK/ folder from overlapping.
/// By default the executable directory is the JKA root, so JK2's default root
/// (and DinurdoJK/) are its direct children; that layout is valid.
pub fn validate_root_layout(jka_base: &Path, jk2_root: &Path, client_install: &Path) -> Result<(), String> {
    let jka_base = normalize(jka_base);
    let jka_root = jka_base.parent().unwrap_or(&jka_base).to_path_buf();
    let jk2_root = normalize(jk2_root);
    let install = normalize(client_install);
    let client_root = install.join(jka_assets::pk3::DINURDOJK_DIRECTORY);

    let separate = || format!(
        "JKA and JK2 roots must be separate folders: {} / {}", jka_root.display(), jk2_root.display());
    // JK2 containing JKA (or the same folder) would expose one game's folders
    // as the other's mods.
    if is_within(&jka_root, &jk2_root) { return Err(separate()); }
    // JK2 beneath JKA is only the default arrangement: JKA rooted at the
    // executable directory with JK2 as a direct child folder.
    if is_within(&jk2_root, &jka_root) {
        let direct_child = jka_root == install && jk2_root.parent() == Some(install.as_path());
        let reserved = jk2_root.file_name().is_some_and(|name| {
            let name = name.to_string_lossy();
            name.eq_ignore_ascii_case("base") || name.eq_ignore_ascii_case(jka_assets::pk3::DINURDOJK_DIRECTORY)
        });
        if !direct_child || reserved { return Err(separate()); }
    }
    // Game roots and bases must stay out of DinurdoJK/ (and vice versa); the
    // executable directory itself may of course contain it.
    let jk2_base = jk2_root.join("base");
    let overlaps = |game: &Path| is_within(game, &client_root) || is_within(&client_root, game);
    if is_within(&jka_root, &client_root) || is_within(&jk2_root, &client_root)
        || overlaps(&jka_base) || overlaps(&jk2_base) || is_within(&client_root, &jk2_root)
    {
        return Err(format!(
            "A game root overlaps the {} folder; choose independent roots", jka_assets::pk3::DINURDOJK_DIRECTORY));
    }
    Ok(())
}

pub fn validate_game_name(value: &str) -> Result<Option<String>, String> {
    let name = value.trim();
    if name.is_empty() { return Ok(None); }
    if name.len() > 63 || name == "." || name == ".."
        || name.bytes().any(|c| c < 0x20 || c == 0x7f || matches!(c, b'/' | b'\\' | b':' | b';' | b'"'))
    {
        return Err("expected one game directory name (not a path, or 'base')".into());
    }
    Ok(Some(name.into()))
}

pub fn parse_root(_base: &Path, value: &str) -> Result<Option<PathBuf>, String> {
    let path = value.trim().trim_matches('"');
    if path.is_empty() { return Ok(None); }
    if path.contains(['\n','\r','\0']) || path.contains('"') {
        return Err("invalid filesystem root: control characters or quotes".into());
    }
    let given = PathBuf::from(path);
    let install = installation_directory()?;
    Ok(Some(if given.is_absolute() { given } else { install.join(given) }))
}

/// Always resolved next to the executable; never beneath either game's root.
fn paths_file(install: &Path) -> PathBuf {
    install.join(jka_assets::pk3::DINURDOJK_DIRECTORY).join(FILE)
}

pub fn load(base: &Path) -> FilesystemRoots {
    let Ok(install) = installation_directory() else { return FilesystemRoots::default() };
    let Ok(content) = std::fs::read_to_string(paths_file(&install)) else { return FilesystemRoots::default() };
    let mut roots = FilesystemRoots::default();
    for raw in content.lines() {
        let Some((key, value)) = raw.split_once('=') else { continue };
        let result: Result<(), String> = match key.trim().to_ascii_lowercase().as_str() {
            "fs_basepath" => parse_root(base, value).map(|v| { roots.basepath = v; }),
            "fs_homepath" => parse_root(base, value).map(|v| { roots.homepath = v; }),
            "fs_cdpath" => parse_root(base, value).map(|v| { roots.cdpath = v; }),
            "fs_jk2basepath" => parse_root(base, value).map(|v| { roots.jk2basepath = v; }),
            "fs_jk2cdpath" => parse_root(base, value).map(|v| { roots.jk2cdpath = v; }),
            "fs_forcegame" => validate_game_name(value).map(|v| { roots.forcegame = v; }),
            _ => continue,
        };
        if let Err(error) = result { eprintln!("Ignoring invalid {key} in {FILE}: {error}"); }
    }
    roots
}

pub fn save(_base: &Path, roots: &FilesystemRoots) -> Result<(), String> {
    let install = installation_directory()?;
    let file = paths_file(&install);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    // Roots beneath the executable are stored relative to it, so moving the
    // whole install folder keeps working. Blank means "use the default".
    let show = |root: &Option<PathBuf>| root.as_ref().map_or(String::new(), |path| {
        match path.strip_prefix(&install) {
            Ok(relative) if !relative.as_os_str().is_empty() => relative.display().to_string(),
            _ => path.display().to_string(),
        }
    });
    let contents = format!(
        "# Game roots contain base/ and mod folders. Relative paths use the executable directory.\n# Blank fs_basepath = the executable directory; blank fs_jk2basepath = <executable directory>/JK2 GameData.\n# fs_forcegame mounts DinurdoJK/<name>/ above shared/ without changing write locations.\nfs_basepath={}\nfs_homepath={}\nfs_cdpath={}\nfs_jk2basepath={}\nfs_jk2cdpath={}\nfs_forcegame={}\n",
        show(&roots.basepath), show(&roots.homepath), show(&roots.cdpath),
        show(&roots.jk2basepath), show(&roots.jk2cdpath),
        roots.forcegame.as_deref().unwrap_or(""),
    );
    std::fs::write(&file, contents).map_err(|error| format!("{}: {error}", file.display()))
}

/// Recognize startup +set overrides before the first asset-index prewarm. The
/// normal console processing still sees the commands later, for cvar display.
pub fn apply_startup_commands(
    base: &Path, roots: &mut FilesystemRoots, commands: &[String],
) -> Result<(), String> {
    for command in commands {
        let mut words = command.splitn(3, char::is_whitespace);
        if !words.next().is_some_and(|word| word.eq_ignore_ascii_case("set") || word.eq_ignore_ascii_case("seta")) {
            continue;
        }
        let Some(name) = words.next() else { continue };
        let Some(value) = words.next() else { continue };
        let value = value.trim().trim_matches('"');
        match name.to_ascii_lowercase().as_str() {
            "fs_basepath" => roots.basepath = parse_root(base, value)?,
            "fs_homepath" => roots.homepath = parse_root(base, value)?,
            "fs_cdpath" => roots.cdpath = parse_root(base, value)?,
            "fs_jk2basepath" => roots.jk2basepath = parse_root(base, value)?,
            "fs_jk2cdpath" => roots.jk2cdpath = parse_root(base, value)?,
            "fs_forcegame" => roots.forcegame = validate_game_name(value)?,
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roots_are_inclusive_game_roots_and_names_reject_traversal() {
        assert!(validate_game_name("../escape").is_err());
        assert!(validate_game_name("C:\\folder").is_err());
        let base = Path::new("/client/base");
        let mut roots = FilesystemRoots::default();
        apply_startup_commands(base, &mut roots, &["set fs_jk2basepath /outcast".into()]).unwrap();
        assert_eq!(jka_assets::pk3::jk2_base_directory(base, &roots), PathBuf::from("/outcast/base"));
    }

    #[test]
    fn portable_layout_allows_game_base_next_to_client_and_separate_jk2_root() {
        // None of these folders exist: a fresh install has no base/ yet.
        let install = std::env::temp_dir().join("djk-portable-layout-validation");
        let jk2 = |name: &str| install.join(name);
        assert!(validate_root_layout(&install.join("base"), &jk2("JK2 GameData"), &install).is_ok());
        assert!(validate_root_layout(&install.join("base"), &jk2("JK2-install"), &install).is_ok());
        assert!(validate_root_layout(&install.join("base"), &install, &install).is_err());
        assert!(validate_root_layout(&install.join("base"), &jk2("base"), &install).is_err());
        assert!(validate_root_layout(&install.join("base"), &jk2("DinurdoJK"), &install).is_err());
    }

    #[test]
    fn root_layout_rejects_nested_or_client_private_game_roots() {
        let install = std::env::temp_dir().join("djk-portable-layout-validation");
        assert!(validate_root_layout(&install.join("JKA/base"), &install.join("JKA/JK2"), &install).is_err());
        assert!(validate_root_layout(&install.join("JKA/base"), &install, &install).is_err());
        assert!(validate_root_layout(&install.join("DinurdoJK/base"), &install.join("JK2 GameData"), &install).is_err());
        assert!(validate_root_layout(&install.join("base"), &install.join("DinurdoJK/JK2"), &install).is_err());
        // JKA elsewhere is fine, as is the default JK2 folder beside the exe.
        let elsewhere = std::env::temp_dir().join("djk-elsewhere-jka");
        assert!(validate_root_layout(&elsewhere.join("base"), &install.join("JK2 GameData"), &install).is_ok());
    }
}
