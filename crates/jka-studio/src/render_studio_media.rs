//! Project-relative managed-media paths.
use std::path::{Component, Path, PathBuf};

/// Untrusted JSON cannot escape the selected project folder.
pub fn media_path(project: &Path, relative: &str) -> Result<PathBuf, String> {
    let path = Path::new(relative);
    if !relative.starts_with("media/") || relative.contains('\\') ||
        !path.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(format!("Unsafe Render Studio media path: {relative}"));
    }
    Ok(project.join(path))
}

