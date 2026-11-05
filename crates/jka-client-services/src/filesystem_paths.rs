//! Filesystem roots in <install>/DinurdoJK/DinurdoJK-paths.ini.
//! The client data folder is a sibling of both game installations.
use std::path::{Path, PathBuf};
use jka_assets::pk3::FilesystemRoots;

const FILE: &str = "DinurdoJK-paths.ini";

/// Default JKA stock directory beneath the dedicated JKA installation root.
pub fn default_base_directory() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let directory = executable.parent().ok_or("Cannot locate executable")?;
    Ok(directory.join("JKA GameData").join("base"))
}

/// Stable client-owned path root, derived from the executable, not from a game.
pub fn installation_directory() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    Ok(executable.parent().ok_or("Cannot locate executable")?.to_path_buf())
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
    let contents = format!(
        "# Game roots contain base/ and mod folders. Relative paths use the executable directory.\n# fs_forcegame mounts DinurdoJK/<name>/ above shared/ without changing write locations.\nfs_basepath={}\nfs_homepath={}\nfs_cdpath={}\nfs_jk2basepath={}\nfs_jk2cdpath={}\nfs_forcegame={}\n",
        roots.basepath.as_ref().map_or(String::new(), |v| v.display().to_string()),
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
        let base = Path::new("/client/JKA GameData/base");
        let mut roots = FilesystemRoots::default();
        apply_startup_commands(base, &mut roots, &["set fs_jk2basepath /outcast".into()]).unwrap();
        assert_eq!(jka_assets::pk3::jk2_base_directory(base, &roots), PathBuf::from("/outcast/base"));
    }
}
