//! Stream already-compressed bundles into a seekable, installable PK3.
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::Path,
};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

pub fn library(input: &Path, native_assets: Option<&Path>, output: &Path) -> Result<usize, String> {
    library_with_overrides(input, None, native_assets, output)
}
pub fn library_with_overrides(
    input: &Path,
    overrides: Option<&Path>,
    native_assets: Option<&Path>,
    output: &Path,
) -> Result<usize, String> {
    let mut files = fs::read_dir(input)
        .map_err(|e| e.to_string())?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    files.retain(|p| {
        p.extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("djk_fnemote"))
    });
    files.sort();
    if let Some(overrides) = overrides {
        for file in &mut files {
            let replacement = overrides.join(file.file_name().ok_or("bundle has no filename")?);
            if replacement.is_file() {
                *file = replacement;
            }
        }
    }
    if files.is_empty() {
        return Err("no shipping bundles found".into());
    }
    // Validate catalogue metadata before publishing any output.
    for path in &files {
        super::Bundle::open_index(path)?;
    }
    let temporary = output.with_extension("pk3.partial");
    if output.exists() || temporary.exists() {
        return Err("package output already exists".into());
    }
    let result = (|| {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        let mut writer = ZipWriter::new(file);
        let mut names = std::collections::HashSet::new();
        if let Some(path) = native_assets {
            let mut archive = ZipArchive::new(File::open(path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            for i in 0..archive.len() {
                let entry = archive.by_index(i).map_err(|e| e.to_string())?;
                let name = super::manifest::qpath(entry.name())?;
                if name.starts_with("emotes/") {
                    return Err("native asset package already contains an emote library".into());
                }
                if !names.insert(name.to_lowercase()) {
                    return Err("duplicate native package member".into());
                }
                writer.raw_copy_file(entry).map_err(|e| e.to_string())?;
            }
        }
        for path in &files {
            let name = format!(
                "emotes/{}",
                path.file_name()
                    .and_then(|n| n.to_str())
                    .ok_or("invalid bundle filename")?
            );
            super::manifest::qpath(&name)?;
            if !names.insert(name.to_lowercase()) {
                return Err("duplicate bundle filename".into());
            }
            let mut file = File::open(path).map_err(|e| e.to_string())?;
            let size = file.metadata().map_err(|e| e.to_string())?.len();
            writer
                .start_file(
                    name,
                    SimpleFileOptions::default()
                        .compression_method(CompressionMethod::Stored)
                        .large_file(size >= u32::MAX as u64),
                )
                .map_err(|e| e.to_string())?;
            let copied = io::copy(&mut file, &mut writer).map_err(|e| e.to_string())?;
            if copied != size {
                return Err("bundle changed while packaging".into());
            }
        }
        // Generated particle shaders/textures staged by compile-presentation.
        if let Some(native) = overrides
            .map(|o| o.join("fx-native"))
            .filter(|p| p.is_dir())
        {
            let mut stack = vec![native.clone()];
            let mut members = Vec::new();
            while let Some(dir) = stack.pop() {
                for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
                    let path = entry.map_err(|e| e.to_string())?.path();
                    if path.is_dir() {
                        stack.push(path);
                    } else {
                        members.push(path);
                    }
                }
            }
            members.sort();
            for path in members {
                let relative = path
                    .strip_prefix(&native)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/");
                let name = super::manifest::qpath(&relative)?;
                if !names.insert(name.to_lowercase()) {
                    return Err(format!("duplicate effect asset {name}"));
                }
                let bytes = fs::read(&path).map_err(|e| e.to_string())?;
                let method = if name.ends_with(".png") {
                    CompressionMethod::Stored
                } else {
                    CompressionMethod::Deflated
                };
                writer
                    .start_file(
                        name,
                        SimpleFileOptions::default().compression_method(method),
                    )
                    .map_err(|e| e.to_string())?;
                writer.write_all(&bytes).map_err(|e| e.to_string())?;
            }
        }
        for name in ["catalog-links.json", "readme.md"] {
            let path = input.join(name);
            if !path.is_file() {
                continue;
            }
            let bytes = fs::read(path).map_err(|e| e.to_string())?;
            if bytes.len() > super::MAX_MANIFEST_BYTES {
                return Err("catalogue metadata exceeds limit".into());
            }
            writer
                .start_file(
                    format!("emotes/{name}"),
                    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
                )
                .map_err(|e| e.to_string())?;
            writer.write_all(&bytes).map_err(|e| e.to_string())?;
        }
        writer
            .finish()
            .map_err(|e| e.to_string())?
            .sync_all()
            .map_err(|e| e.to_string())?;
        // Do not overwrite an installation created while the build was running.
        fs::hard_link(&temporary, output).map_err(|e| e.to_string())?;
        fs::remove_file(&temporary).map_err(|e| e.to_string())?;
        Ok(files.len())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
