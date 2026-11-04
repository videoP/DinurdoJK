//! Persistent filesystem roots, stored in `<install>/DinurdoJK/` next to `base/`
//! (not under fs_homepath) so moving the main config to fs_homepath does not
//! make this file's own location undiscoverable.
use std::path::{Path, PathBuf};
use jka_assets::pk3::FilesystemRoots;

const FILE: &str = "DinurdoJK-paths.ini";

/// The `base` folder the executable runs against: `base` next to the exe, else
/// the development layout (`target\..\base`), else the (missing) adjacent one.
pub(crate) fn default_base_directory() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let directory = executable.parent().ok_or("Cannot locate executable")?;
    let adjacent = directory.join("base");
    let development = directory.parent().unwrap_or(directory).join("base");
    Ok(if adjacent.is_dir() {
        adjacent
    } else if development.is_dir() {
        development
    } else {
        adjacent
    })
}

pub(crate) fn validate_game_name(value: &str) -> Result<Option<String>, String> {
    let name = value.trim();
    if name.is_empty() { return Ok(None); }
    if name.len() > 63 || name == "." || name == ".."
        || name.bytes().any(|c| c < 0x20 || c == 0x7f || matches!(c, b'/' | b'\\' | b':' | b';' | b'"'))
    {
        return Err("expected one game directory name (not a path, or 'base')".into());
    }
    Ok(Some(name.into()))
}

pub(crate) fn parse_root(base: &Path, value: &str) -> Result<Option<PathBuf>, String> {
    let path = value.trim().trim_matches('"');
    if path.is_empty() { return Ok(None); }
    if path.contains(['\n','\r','\0']) || path.contains('"') {
        return Err("invalid filesystem root: control characters or quotes".into());
    }
    let given = PathBuf::from(path);
    let install = base.parent().ok_or("base has no installation root")?;
    Ok(Some(if given.is_absolute() { given } else { install.join(given) }))
}

/// `<install>/DinurdoJK/DinurdoJK-paths.ini`. Anchored to the folder containing
/// `base/`, never to fs_homepath, so it stays discoverable before any root is
/// known (fs_homepath itself is stored in it).
fn paths_file(install: &Path) -> PathBuf {
    install.join(jka_assets::pk3::DINURDOJK_DIRECTORY).join(FILE)
}

/// One-time move of the file from its old place beside the executable's
/// `base/`. If it cannot be moved, `load` still reads the old copy.
fn migrate_legacy_file(install: &Path) {
    let old = install.join(FILE);
    let new = paths_file(install);
    if !old.is_file() || new.exists() { return; }
    if let Some(parent) = new.parent() { let _ = std::fs::create_dir_all(parent); }
    if let Err(error) = std::fs::rename(&old, &new) {
        eprintln!("Could not move {} to {}: {error}", old.display(), new.display());
    }
}

pub(crate) fn load(base: &Path) -> FilesystemRoots {
    let Some(install) = base.parent() else { return FilesystemRoots::default() };
    migrate_legacy_file(install);
    let Ok(content) = std::fs::read_to_string(paths_file(install))
        .or_else(|_| std::fs::read_to_string(install.join(FILE))) else { return FilesystemRoots::default() };
    let mut roots = FilesystemRoots::default();
    // Read a previous fs_jk2path from an older bootstrap INI once, while
    // allowing the new fs_jk2cdpath to win regardless of line ordering.
    let has_cdpath = content.lines().any(|line| line.split_once('=')
        .is_some_and(|(key, _)| key.trim().eq_ignore_ascii_case("fs_jk2cdpath")));
    for raw in content.lines() {
        let Some((key, value)) = raw.split_once('=') else { continue };
        let result: Result<(), String> = match key.trim().to_ascii_lowercase().as_str() {
            "fs_homepath" => parse_root(base, value).map(|v| { roots.homepath = v; }),
            "fs_cdpath" => parse_root(base, value).map(|v| { roots.cdpath = v; }),
            "fs_jk2basepath" => parse_root(base, value).map(|v| { roots.jk2basepath = v; }),
            "fs_jk2cdpath" => parse_root(base, value).map(|v| { roots.jk2cdpath = v; }),
            "fs_jk2path" if !has_cdpath => parse_root(base, value).map(|v| { roots.jk2cdpath = v; }),
            "fs_forcegame" => validate_game_name(value).map(|v| { roots.forcegame = v; }),
            _ => continue,
        };
        if let Err(error) = result { eprintln!("Ignoring invalid {key} in {FILE}: {error}"); }
    }
    roots
}

pub(crate) fn save(base: &Path, roots: &FilesystemRoots) -> Result<(), String> {
    let install = base.parent().ok_or("base has no installation root")?;
    let file = paths_file(install);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    let contents = format!(
        "# Filesystem roots; relative paths are relative to the directory containing base/.\nfs_homepath={}\nfs_cdpath={}\nfs_jk2basepath={}\nfs_jk2cdpath={}\nfs_forcegame={}\n",
        roots.homepath.as_ref().map_or(String::new(), |v| v.display().to_string()),
        roots.cdpath.as_ref().map_or(String::new(), |v| v.display().to_string()),
        roots.jk2basepath.as_ref().map_or(String::new(), |v| v.display().to_string()),
        roots.jk2cdpath.as_ref().map_or(String::new(), |v| v.display().to_string()),
        roots.forcegame.as_deref().unwrap_or(""),
    );
    std::fs::write(&file, contents).map_err(|error| format!("{}: {error}", file.display()))
}

/// Recognize startup +set overrides before the first asset-index prewarm. The
/// normal console processing still sees the commands later, for cvar display.
pub(crate) fn apply_startup_commands(
    base: &Path, roots: &mut FilesystemRoots, commands: &[String],
) -> Result<(), String> {
    let has_jk2cdpath = commands.iter().any(|command| {
        let mut words = command.split_whitespace();
        words.next().is_some_and(|word| word.eq_ignore_ascii_case("set") || word.eq_ignore_ascii_case("seta"))
            && words.next().is_some_and(|name| name.eq_ignore_ascii_case("fs_jk2cdpath"))
    });
    for command in commands {
        let mut words = command.splitn(3, char::is_whitespace);
        if !words.next().is_some_and(|word| word.eq_ignore_ascii_case("set") || word.eq_ignore_ascii_case("seta")) {
            continue;
        }
        let Some(name) = words.next() else { continue };
        let Some(value) = words.next() else { continue };
        let value = value.trim().trim_matches('"');
        match name.to_ascii_lowercase().as_str() {
            "fs_homepath" => roots.homepath = parse_root(base, value)?,
            "fs_cdpath" => roots.cdpath = parse_root(base, value)?,
            "fs_jk2basepath" => roots.jk2basepath = parse_root(base, value)?,
            "fs_jk2cdpath" => roots.jk2cdpath = parse_root(base, value)?,
            "fs_jk2path" if !has_jk2cdpath => roots.jk2cdpath = parse_root(base, value)?,
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
    fn migrates_legacy_jk2_install_to_cdpath_without_losing_default_base() {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let install = std::env::temp_dir().join(format!("jk2-path-migrate-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&install).unwrap();
        let base = install.join("base");
        // A file left beside base/ by an older build moves into DinurdoJK/.
        let legacy = install.join(FILE);
        let ini = paths_file(&install);
        std::fs::write(&legacy, "fs_jk2path=old-retail\n").unwrap();
        let roots = load(&base);
        assert!(!legacy.exists() && ini.is_file());
        assert_eq!(roots.jk2cdpath, Some(install.join("old-retail")));
        assert_eq!(jka_assets::pk3::jk2_base_directory(&base, &roots), install.join("base_jk2"));
        save(&base, &roots).unwrap();
        let saved = std::fs::read_to_string(&ini).unwrap();
        assert!(saved.contains("fs_jk2cdpath="));
        assert!(!saved.contains("fs_jk2path="));
        std::fs::write(&ini, "fs_jk2path=old-retail\nfs_jk2cdpath=new-retail\nfs_jk2basepath=custom-base\n").unwrap();
        let roots = load(&base);
        assert_eq!(roots.jk2cdpath, Some(install.join("new-retail")));
        assert_eq!(jka_assets::pk3::jk2_base_directory(&base, &roots), install.join("custom-base"));
        std::fs::remove_dir_all(install).unwrap();
    }

    #[test]
    fn modern_startup_jk2_cdpath_overrides_legacy_spelling() {
        let base = Path::new("/client/base");
        let mut roots = FilesystemRoots::default();
        apply_startup_commands(base, &mut roots, &[
            "seta fs_jk2cdpath new-retail".to_owned(),
            "seta fs_jk2path old-retail".to_owned(),
            "set fs_jk2basepath base_jk2".to_owned(),
        ]).unwrap();
        assert_eq!(roots.jk2cdpath, Some(PathBuf::from("/client/new-retail")));
        assert_eq!(roots.jk2basepath, Some(PathBuf::from("/client/base_jk2")));
    }

    #[test]
    fn normalizes_paths_and_rejects_game_traversal() {
        let base = Path::new("/client/base");
        assert_eq!(parse_root(base, "../retail").unwrap(), Some(PathBuf::from("/client/../retail")));
        assert!(validate_game_name("../japro").is_err());
        assert!(validate_game_name("C:\\tools").is_err());
    }
}
