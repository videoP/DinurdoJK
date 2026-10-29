//! Persistent filesystem roots. Kept beside the executable so moving the main
//! config to fs_homepath does not make its own location undiscoverable.
use std::path::{Path, PathBuf};
use jka_assets::pk3::FilesystemRoots;

const FILE: &str = "DinurdoJK-paths.ini";

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

pub(crate) fn load(base: &Path) -> FilesystemRoots {
    let Some(install) = base.parent() else { return FilesystemRoots::default() };
    let Ok(content) = std::fs::read_to_string(install.join(FILE)) else { return FilesystemRoots::default() };
    let mut roots = FilesystemRoots::default();
    for raw in content.lines() {
        let Some((key, value)) = raw.split_once('=') else { continue };
        let result: Result<(), String> = match key.trim().to_ascii_lowercase().as_str() {
            "fs_homepath" => parse_root(base, value).map(|v| { roots.homepath = v; }),
            "fs_cdpath" => parse_root(base, value).map(|v| { roots.cdpath = v; }),
            "fs_forcegame" => validate_game_name(value).map(|v| { roots.forcegame = v; }),
            _ => continue,
        };
        if let Err(error) = result { eprintln!("Ignoring invalid {key} in {FILE}: {error}"); }
    }
    roots
}

pub(crate) fn save(base: &Path, roots: &FilesystemRoots) -> Result<(), String> {
    let install = base.parent().ok_or("base has no installation root")?;
    let file = install.join(FILE);
    let contents = format!(
        "# Filesystem roots; relative paths are relative to the directory containing base/.\nfs_homepath={}\nfs_cdpath={}\nfs_forcegame={}\n",
        roots.homepath.as_ref().map_or(String::new(), |v| v.display().to_string()),
        roots.cdpath.as_ref().map_or(String::new(), |v| v.display().to_string()),
        roots.forcegame.as_deref().unwrap_or(""),
    );
    std::fs::write(&file, contents).map_err(|error| format!("{}: {error}", file.display()))
}

/// Recognize startup +set overrides before the first asset-index prewarm. The
/// normal console processing still sees the commands later, for cvar display.
pub(crate) fn apply_startup_commands(
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
            "fs_homepath" => roots.homepath = parse_root(base, value)?,
            "fs_cdpath" => roots.cdpath = parse_root(base, value)?,
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
    fn normalizes_paths_and_rejects_game_traversal() {
        let base = Path::new("/client/base");
        assert_eq!(parse_root(base, "../retail").unwrap(), Some(PathBuf::from("/client/../retail")));
        assert!(validate_game_name("../japro").is_err());
        assert!(validate_game_name("C:\\tools").is_err());
    }
}
