//! Per-map disk caches for sun-independent static bakes (sky openness, sky
//! transport). They depend only on static geometry, receiver normals and the
//! visibility mode, never on the sun/moon direction, so one file per map stays
//! valid across Time of Day, relight and removal edits.
//!
//! Lossless: exact f32 values, byte-plane shuffled (all first bytes, then all
//! second bytes, ...) so deflate sees long runs, stored in a PNG L8 container
//! with a checksum like the baked-sun removal cache. A hit reproduces the trace.
use image::{ImageDecoder, ImageEncoder};
use std::{hash::Hasher, io::Cursor, path::{Path, PathBuf}};

const MAGIC: &[u8; 8] = b"JKAFPC01";
const ROW: u32 = 4096;

pub(super) const OPENNESS: (&str, &str) = ("relighting", "skyopen");
pub(super) const TRANSPORT: (&str, &str) = ("relighting", "skytransport");

fn checksum(bytes: &[u8]) -> u64 {
    let mut hash = crate::scene::FoldHasher::default();
    hash.write(bytes);
    hash.finish()
}

/// `<cache>/relighting/<map>-<map hash>-<key>.<extension>`, alongside `.sunpng` files.
pub(super) fn path(
    cache: &crate::renderer::scene::StaticBspAoCacheInfo, (folder, extension): (&str, &str), key: u64,
) -> PathBuf {
    cache
        .directory
        .parent()
        .unwrap_or(&cache.directory)
        .join(folder)
        .join(format!("{}-{:016x}-{key:016x}.{extension}", safe_map_name(&cache.map_name), cache.map_hash))
}

pub(in crate::renderer) fn safe_map_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') { c } else { '_' })
        .collect()
}

/// Delete every other `<map>-<16 hex>-<16 hex>.<extension>` file of this map
/// in `keep`'s folder. A map needs one result; superseded keys (map edits,
/// settings, cache versions) are never read again. Other maps whose names
/// merely share a prefix are left alone.
pub(in crate::renderer) fn prune_map_siblings(keep: &Path, map: &str, extension: &str) -> usize {
    let (Some(directory), Some(kept)) = (keep.parent(), keep.file_name()) else { return 0; };
    let Ok(entries) = std::fs::read_dir(directory) else { return 0; };
    let hex16 = |s: &str| s.len() == 16 && s.bytes().all(|b| b.is_ascii_hexdigit());
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name == kept { continue; }
        let Some(name) = name.to_str() else { continue; };
        let Some(stem) = name.strip_suffix(extension).and_then(|s| s.strip_suffix('.')) else { continue; };
        let mut parts = stem.rsplitn(3, '-');
        let (Some(key), Some(map_hash), Some(owner)) = (parts.next(), parts.next(), parts.next()) else { continue; };
        if owner == map && hex16(key) && hex16(map_hash) && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// `prune_map_siblings` for a path built by [`path`].
pub(super) fn prune(keep: &Path, map: &str, (_, extension): (&str, &str)) -> usize {
    prune_map_siblings(keep, &safe_map_name(map), extension)
}

/// Pages of `channels` f32 values per luxel at `sizes`.
pub(super) fn read(path: &Path, key: u64, sizes: &[[u32; 2]], channels: usize) -> Option<Vec<Vec<f32>>> {
    let counts: Vec<usize> = sizes.iter().map(|[w, h]| *w as usize * *h as usize * channels).collect();
    let floats: usize = counts.iter().sum();
    let expected = 4 + sizes.len() * 8 + 4 + floats * 4;
    let file = std::fs::read(path).ok()?;
    if file.len() < 32 || &file[..8] != MAGIC || u64::from_le_bytes(file[8..16].try_into().ok()?) != key
        || u64::from_le_bytes(file[16..24].try_into().ok()?) != expected as u64 {
        return None;
    }
    let sum = u64::from_le_bytes(file[24..32].try_into().ok()?);
    let decoder = image::codecs::png::PngDecoder::new(Cursor::new(&file[32..])).ok()?;
    let height = u32::try_from(expected.div_ceil(ROW as usize)).ok()?;
    if decoder.dimensions() != (ROW, height) || decoder.color_type() != image::ColorType::L8 {
        return None;
    }
    let mut raw = vec![0; ROW as usize * height as usize];
    decoder.read_image(&mut raw).ok()?;
    raw.truncate(expected);
    if checksum(&raw) != sum { return None; }
    let word = |at: usize| u32::from_le_bytes([raw[at], raw[at + 1], raw[at + 2], raw[at + 3]]);
    if word(0) as usize != sizes.len() { return None; }
    for (i, [w, h]) in sizes.iter().enumerate() {
        if word(4 + i * 8) != *w || word(8 + i * 8) != *h { return None; }
    }
    if word(4 + sizes.len() * 8) as usize != channels { return None; }
    let planes = &raw[8 + sizes.len() * 8..];
    let mut values = (0..floats).map(|i| f32::from_le_bytes([
        planes[i], planes[floats + i], planes[2 * floats + i], planes[3 * floats + i],
    ]));
    Some(counts.iter().map(|&n| values.by_ref().take(n).collect()).collect())
}

pub(super) fn write(
    path: &Path, key: u64, sizes: &[[u32; 2]], channels: usize, pages: &[&[f32]],
) -> Result<usize, String> {
    use std::{fs, io::Write, sync::atomic::{AtomicU64, Ordering}};
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let floats: usize = pages.iter().map(|p| p.len()).sum();
    if pages.len() != sizes.len()
        || pages.iter().zip(sizes).any(|(p, [w, h])| p.len() != *w as usize * *h as usize * channels) {
        return Err("page sizes do not match".into());
    }
    let mut raw = Vec::with_capacity(8 + sizes.len() * 8 + floats * 4);
    raw.extend_from_slice(&(sizes.len() as u32).to_le_bytes());
    for [w, h] in sizes {
        raw.extend_from_slice(&w.to_le_bytes());
        raw.extend_from_slice(&h.to_le_bytes());
    }
    raw.extend_from_slice(&(channels as u32).to_le_bytes());
    for plane in 0..4 {
        raw.extend(pages.iter().flat_map(|p| p.iter()).map(|v| v.to_le_bytes()[plane]));
    }
    let raw_len = raw.len();
    let sum = checksum(&raw);
    let height = raw_len.div_ceil(ROW as usize) as u32;
    raw.resize(ROW as usize * height as usize, 0);
    let mut file = Vec::new();
    file.extend_from_slice(MAGIC);
    file.extend_from_slice(&key.to_le_bytes());
    file.extend_from_slice(&(raw_len as u64).to_le_bytes());
    file.extend_from_slice(&sum.to_le_bytes());
    image::codecs::png::PngEncoder::new_with_quality(
        &mut file,
        image::codecs::png::CompressionType::Default,
        image::codecs::png::FilterType::NoFilter,
    )
    .write_image(&raw, ROW, height, image::ExtendedColorType::L8)
    .map_err(|error| error.to_string())?;
    fs::create_dir_all(path.parent().ok_or("missing cache directory")?).map_err(|e| e.to_string())?;
    let temporary = path.with_extension(format!("{}-{}.tmp", std::process::id(), SERIAL.fetch_add(1, Ordering::Relaxed)));
    let result = (|| {
        let mut output = fs::OpenOptions::new().create_new(true).write(true).open(&temporary)?;
        output.write_all(&file)?;
        drop(output);
        fs::rename(&temporary, path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    Ok(file.len())
}
