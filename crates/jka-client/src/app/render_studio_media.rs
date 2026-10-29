//! Portable Render Studio media: one project per folder beside base/, never per fs_game.
//! Project JSON contains only relative managed-media paths. Linked audio is absolute.
use super::{render_studio::RenderStudio, App};
use sha2::{Digest, Sha256};
use std::{fs, io::{Read, Write}, path::{Component, Path, PathBuf}};

const MAX_DEMO_BYTES: u64 = 512 * 1024 * 1024;
const MAX_AUDIO_BYTES: u64 = 512 * 1024 * 1024;

/// Untrusted JSON cannot escape the selected project folder.
pub(super) fn media_path(project: &Path, relative: &str) -> Result<PathBuf, String> {
    let path = Path::new(relative);
    if !relative.starts_with("media/") || relative.contains('\\') ||
        !path.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(format!("Unsafe Render Studio media path: {relative}"));
    }
    Ok(project.join(path))
}

fn safe_stem(path: &Path) -> String {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let safe: String = stem.chars().take(60)
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if safe.is_empty() { "media".into() } else { safe }
}

fn digest_file(path: &Path, limit: u64) -> Result<[u8; 32], String> {
    let mut file = fs::File::open(path).map_err(|e|format!("{}: {e}", path.display()))?;
    let size = file.metadata().map_err(|e|e.to_string())?.len();
    if size == 0 || size > limit { return Err(format!("File must be between 1 byte and {} MiB", limit / 1048576)); }
    let mut hash = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let read = file.read(&mut buf).map_err(|e|e.to_string())?;
        if read == 0 {break;}
        hash.update(&buf[..read]);
    }
    Ok(hash.finalize().into())
}

fn name_for(source: &Path, bytes_hash: &[u8; 32], ext: &str) -> String {
    let short = bytes_hash[..12].iter().map(|b|format!("{b:02x}")).collect::<String>();
    format!("{}-{short}.{ext}", safe_stem(source))
}

impl App {
    /// Game installation root/render-studio, NOT base/render-studio or fs_game/render-studio.
    pub(in crate::app) fn studio_media_root(&self) -> PathBuf {
        self.base.parent().unwrap_or(&self.base).join("render-studio")
    }
    pub(in crate::app) fn studio_project_directory_for(&self, name: &str) -> PathBuf {
        let file = RenderStudio::project_file_name_for(name);
        self.studio_media_root().join(file.strip_suffix(".json").unwrap_or(&file))
    }
    pub(in crate::app) fn studio_project_directory(&self) -> PathBuf {
        self.studio_project_directory_for(&self.render_studio.project_name)
    }
    pub(in crate::app) fn studio_project_path_for(&self, name: &str) -> PathBuf {
        self.studio_project_directory_for(name).join("project.json")
    }
    pub(in crate::app) fn studio_project_path(&self) -> PathBuf {
        self.studio_project_path_for(&self.render_studio.project_name)
    }
    fn studio_media_directory(&self, kind: &str) -> PathBuf {
        self.studio_project_directory().join("media").join(kind)
    }
    /// Source bytes are deduplicated by hash; source files are never moved or modified.
    pub(in crate::app) fn studio_copy_audio(&self, source: &Path) -> Result<PathBuf, String> {
        let ext = source.extension().and_then(|s|s.to_str()).unwrap_or("").to_ascii_lowercase();
        if !["wav", "mp3", "flac", "ogg"].contains(&ext.as_str()) {
            return Err("Choose WAV, MP3, FLAC or OGG audio".into());
        }
        let hash = digest_file(source, MAX_AUDIO_BYTES)?;
        let dir = self.studio_media_directory("audio");
        fs::create_dir_all(&dir).map_err(|e|e.to_string())?;
        let target = dir.join(name_for(source, &hash, &ext));
        if target == source {return Ok(target);}
        if !target.exists() {
            let temp = target.with_extension(format!("{ext}.partial-{}", std::process::id()));
            let result = fs::copy(source, &temp).and_then(|_| fs::rename(&temp, &target));
            if let Err(e) = result { let _ = fs::remove_file(&temp); return Err(e.to_string()); }
        }
        Ok(target)
    }
    /// Store a demo's unmodified bytes. Return a project-relative media path.
    pub(in crate::app) fn studio_copy_demo_bytes(&self, source: &Path, bytes: &[u8]) -> Result<String, String> {
        if bytes.is_empty() || bytes.len() as u64 > MAX_DEMO_BYTES {return Err("Invalid or oversized DM26 recording".into());}
        let hash: [u8; 32] = Sha256::digest(bytes).into();
        let dir = self.studio_media_directory("demos");
        fs::create_dir_all(&dir).map_err(|e|e.to_string())?;
        let target = dir.join(name_for(source, &hash, "dm_26"));
        if !target.exists() {
            let temp = target.with_extension(format!("dm_26.partial-{}", std::process::id()));
            let result = (|| -> std::io::Result<()> {
                let mut out = fs::File::create(&temp)?;
                out.write_all(bytes)?;
                out.flush()?;
                fs::rename(&temp, &target)
            })();
            if let Err(e) = result { let _=fs::remove_file(&temp);return Err(e.to_string()); }
        }
        let relative = target.strip_prefix(self.studio_project_directory()).map_err(|e|e.to_string())?;
        Ok(relative.to_string_lossy().replace('\\', "/"))
    }
    /// Read a browser demo using the existing virtual filesystem, including PK3s.
    pub(in crate::app) fn studio_read_library_demo(&self, name: &str, game: &str) -> Result<Vec<u8>, String> {
        let source_game = (!game.is_empty()).then(||self.base.parent().unwrap_or(&self.base).join(game));
        let roots = jka_assets::pk3::configured_filesystem_roots(&self.base);
        let mut assets = jka_assets::pk3::AssetSearchPath::open_search_dirs(
            jka_assets::pk3::filesystem_game_layer_dirs(&self.base, source_game.as_deref(), &roots))
            .map_err(|e|e.to_string())?;
        let qpath = format!("demos/{name}.dm_26");
        Ok(assets.read(&qpath, MAX_DEMO_BYTES as usize).map_err(|e|e.to_string())?
            .ok_or_else(||format!("Demo not found: {qpath}"))?.bytes)
    }
    pub(in crate::app) fn studio_copy_library_demo(&self, name: &str, game: &str) -> Result<String, String> {
        let bytes = self.studio_read_library_demo(name, game)?;
        self.studio_copy_demo_bytes(Path::new(name), &bytes)
    }
    /// Save managed audio as a relative path; leave externally linked audio absolute.
    pub(in crate::app) fn studio_saved_audio_path(&self, path: &str) -> String {
        Path::new(path).strip_prefix(self.studio_project_directory()).ok()
            .map(|p|p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(||path.to_owned())
    }
    pub(in crate::app) fn studio_loaded_audio_path(&self, path: &str) -> Result<String, String> {
        if path.starts_with("media/") {
            let absolute = media_path(&self.studio_project_directory(), path)?;
            return Ok(absolute.to_string_lossy().into_owned());
        }
        if !Path::new(path).is_absolute() {return Err(format!("Unrecognized audio path: {path}"));}
        Ok(path.to_owned())
    }
    /// Save As: clone every referenced managed file into the new project before
    /// changing the project JSON. Linked audio remains external by design.
    pub(in crate::app) fn studio_rehome_media_for_save(&mut self) -> Result<(), String> {
        let old = self.studio_project_directory_for(&self.render_studio.saved_project_name);
        let new = self.studio_project_directory();
        if old == new {return Ok(());}
        let old_audio = self.render_studio.timeline.audio_tracks.iter().flat_map(|t|t.clips.iter())
            .filter(|c|Path::new(&c.path).starts_with(old.join("media/audio")))
            .map(|c|c.path.clone()).collect::<Vec<_>>();
        let mut replacements = std::collections::HashMap::new();
        for original in old_audio {
            replacements.insert(original.clone(), self.studio_copy_audio(Path::new(&original))?.to_string_lossy().into_owned());
        }
        let overlay_old = self.render_studio.timeline.demos.iter()
            .map(|c|c.race_overlays.iter().map(|o|o.source_path.clone()).collect::<Vec<_>>())
            .collect::<Vec<_>>();
        let mut overlay_next = Vec::with_capacity(overlay_old.len());
        for overlays in &overlay_old {
            let mut row = Vec::with_capacity(overlays.len());
            for source in overlays {
                let reference = source.as_deref().ok_or("Composite demo lacks imported recording")?;
                let bytes=fs::read(media_path(&old,reference)?).map_err(|e|format!("{reference}: {e}"))?;
                row.push(self.studio_copy_demo_bytes(Path::new(reference),&bytes)?);
            }
            overlay_next.push(row);
        }
        let demo_old = self.render_studio.timeline.demos.iter().map(|c|c.source_path.clone()).collect::<Vec<_>>();
        let mut demo_next = Vec::with_capacity(demo_old.len());
        for source in &demo_old {
            let reference = source.as_deref().ok_or("Demo is missing project-managed media path")?;
            let bytes=fs::read(media_path(&old,reference)?).map_err(|e|format!("{reference}: {e}"))?;
            demo_next.push(self.studio_copy_demo_bytes(Path::new(reference),&bytes)?);
        }
        for row in &mut self.render_studio.timeline.audio_tracks {
            for clip in &mut row.clips {
                if let Some(next)=replacements.get(&clip.path) {clip.path=next.clone();}
            }
        }
        for (clip,(next, overlays)) in self.render_studio.timeline.demos.iter_mut().zip(demo_next.into_iter().zip(overlay_next)) {
            clip.source_path=Some(next);
            for (overlay,path) in clip.race_overlays.iter_mut().zip(overlays) {overlay.source_path=Some(path);}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_cannot_escape_project() {
        let project=Path::new("C:/JKA/render-studio/MyMovie");
        assert!(media_path(project,"media/demos/shot.dm_26").is_ok());
        for path in ["../escape", "media/../escape", "media/./thing", "C:/file", "media/../../x", "media/\\evil"] {
            assert!(media_path(project,path).is_err(),"accepted: {path}");
        }
    }
}
