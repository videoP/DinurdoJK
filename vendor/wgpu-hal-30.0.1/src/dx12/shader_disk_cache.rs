//! On-disk cache of compiled DX12 shader bytecode (DinurdoJK patch).
//!
//! Turning naga's HLSL into DXIL with DXC costs ~120 ms per large fragment
//! shader and is repeated on every launch; the driver's own PSO cache cannot
//! help because the bytecode has to exist before the pipeline is created. The
//! directory comes from `DINURDOJK_DX12_SHADER_CACHE`; unset disables the cache.
//! Entries are keyed by compiler kind, target profile, entry point and the full
//! HLSL text, so any shader or naga change simply misses. A file is only used
//! if it is a DXBC container whose declared size equals its length (torn files
//! cannot pass), and it is written via temp file + rename.
use alloc::{format, vec::Vec};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::OnceLock;

const VERSION: u8 = 1;
/// Least-recently-used files are deleted past this many bytes (checked when the
/// cache is first used in a process). A hit refreshes the file's modified time.
const MAX_BYTES: u64 = 64 << 20;

/// Delete the oldest files until the directory fits `MAX_BYTES`.
fn evict_oldest(dir: &std::path::Path) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<_> = read
        .filter_map(Result::ok)
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            Some((meta.modified().ok()?, meta.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= MAX_BYTES {
        return;
    }
    files.sort_by_key(|f| f.0);
    for (_, len, path) in files {
        if total <= MAX_BYTES * 3 / 4 {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

fn directory() -> Option<&'static PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = PathBuf::from(std::env::var_os("DINURDOJK_DX12_SHADER_CACHE")?);
        std::fs::create_dir_all(&dir).ok()?;
        evict_oldest(&dir);
        Some(dir)
    })
    .as_ref()
}

fn entry_path(compiler: &str, profile: &str, entry: &str, source: &str) -> Option<PathBuf> {
    let dir = directory()?;
    let mut lanes = [0u64; 2];
    for (lane, seed) in lanes.iter_mut().zip([0x51u8, 0xa7]) {
        let mut hasher = DefaultHasher::new();
        (VERSION, seed).hash(&mut hasher);
        compiler.hash(&mut hasher);
        profile.hash(&mut hasher);
        entry.hash(&mut hasher);
        source.hash(&mut hasher);
        *lane = hasher.finish();
    }
    Some(dir.join(format!("{:016x}{:016x}.bin", lanes[0], lanes[1])))
}

/// A DXBC/DXIL container stores its total size at offset 24.
fn is_whole_container(bytes: &[u8]) -> bool {
    bytes.len() >= 32
        && &bytes[..4] == b"DXBC"
        && u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]) as usize == bytes.len()
}

pub(super) fn load(compiler: &str, profile: &str, entry: &str, source: &str) -> Option<Vec<u8>> {
    let path = entry_path(compiler, profile, entry, source)?;
    let bytes = std::fs::read(&path).ok()?;
    if !is_whole_container(&bytes) {
        return None;
    }
    // Mark as recently used so eviction drops what no map needs any more.
    if let Ok(file) = std::fs::File::options().write(true).open(&path) {
        let _ = file.set_modified(std::time::SystemTime::now());
    }
    Some(bytes)
}

pub(super) fn store(compiler: &str, profile: &str, entry: &str, source: &str, bytes: &[u8]) {
    if !is_whole_container(bytes) {
        return;
    }
    let Some(path) = entry_path(compiler, profile, entry, source) else {
        return;
    };
    // Unique per thread: several pipeline workers may compile the same shader.
    let temp = path.with_extension(format!("{:?}.tmp", std::thread::current().id()).replace(['(', ')'], ""));
    if std::fs::write(&temp, bytes)
        .and_then(|()| std::fs::rename(&temp, &path))
        .is_err()
    {
        let _ = std::fs::remove_file(&temp);
    }
}
