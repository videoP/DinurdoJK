//! Per-pipeline on-disk Vulkan pipeline cache (DinurdoJK patch).
//!
//! A single `VkPipelineCache` blob is opaque and append-only: stale entries from
//! old builds are never evicted, so it could only be deleted whole once it grew
//! too large, after which every pipeline compiled cold again (a hardware-RT world
//! PSO costs seconds in the NVIDIA driver). Instead, each pipeline gets its own
//! small cache file, keyed by the SPIR-V of its stages, its entry points, its
//! fixed-function state and the device/driver validation key. Reading a file
//! refreshes its modified time and the least recently used files are deleted
//! when the directory exceeds `MAX_BYTES`.
//!
//! Used only for pipelines created without an explicit `cache` while
//! `DINURDOJK_VK_PIPELINE_CACHE` names a directory. The key is only a lookup
//! hint: the driver still validates every entry against the full create info,
//! so a collision or a driver update costs a compile, never a wrong pipeline.
//! A checksum trailer rejects torn files before the driver sees them.
use alloc::{format, vec::Vec};
use ash::vk;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

const VERSION: u8 = 1;
/// Least-recently-used files are deleted past this many bytes, down to 3/4.
/// Hardware-RT settings make even model pipelines ~1.4-2 MB each, and one cold
/// session of them wrote ~255 MiB; a smaller cap evicted entries still in use.
const MAX_BYTES: u64 = 1 << 30;
/// Re-run eviction after this many bytes were written in one process.
const EVICT_AFTER_WRITTEN: u64 = 32 << 20;

static WRITTEN: AtomicU64 = AtomicU64::new(0);

pub(super) fn spirv_hash(words: &[u32]) -> u64 {
    let mut hasher = DefaultHasher::new();
    words.hash(&mut hasher);
    hasher.finish()
}

/// Delete the oldest files until the directory fits `MAX_BYTES`.
fn evict_oldest(dir: &Path) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<_> = read
        .filter_map(Result::ok)
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            meta.is_file().then_some(())?;
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
        let dir = PathBuf::from(std::env::var_os("DINURDOJK_VK_PIPELINE_CACHE")?);
        std::fs::create_dir_all(&dir).ok()?;
        evict_oldest(&dir);
        Some(dir)
    })
    .as_ref()
}

fn checksum(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// A temporary cache seeded from one pipeline's file.
pub(super) struct Slot {
    pub(super) cache: vk::PipelineCache,
    path: PathBuf,
    loaded: Option<u64>,
}

/// `key` must hash everything that selects the pipeline: stage SPIR-V hashes,
/// entry points and fixed-function state.
pub(super) fn open(raw: &ash::Device, validation_key: [u8; 16], key: impl Hash) -> Option<Slot> {
    let dir = directory()?;
    let mut lanes = [0u64; 2];
    for (lane, seed) in lanes.iter_mut().zip([0x3cu8, 0xd9]) {
        let mut hasher = DefaultHasher::new();
        (VERSION, seed, validation_key).hash(&mut hasher);
        key.hash(&mut hasher);
        *lane = hasher.finish();
    }
    let path = dir.join(format!("{:016x}{:016x}.bin", lanes[0], lanes[1]));
    let mut data = std::fs::read(&path).ok().filter(|bytes| bytes.len() > 8);
    if let Some(bytes) = data.as_mut() {
        let tail = bytes.split_off(bytes.len() - 8);
        if u64::from_le_bytes(tail.try_into().unwrap()) != checksum(bytes) {
            data = None;
        } else if let Ok(file) = std::fs::File::options().write(true).open(&path) {
            // Mark as recently used so eviction drops what nothing needs any more.
            let _ = file.set_modified(std::time::SystemTime::now());
        }
    }
    let mut info = vk::PipelineCacheCreateInfo::default();
    if let Some(bytes) = data.as_deref() {
        info = info.initial_data(bytes);
    }
    let cache = unsafe { raw.create_pipeline_cache(&info, None) }.ok()?;
    Some(Slot {
        cache,
        path,
        loaded: data.as_deref().map(checksum),
    })
}

/// Persist the cache if the driver added to it (a miss), then destroy it.
pub(super) fn close(raw: &ash::Device, slot: Slot, pipeline_created: bool) {
    let data = pipeline_created.then(|| unsafe { raw.get_pipeline_cache_data(slot.cache) });
    unsafe { raw.destroy_pipeline_cache(slot.cache, None) };
    let Some(data) = data else {
        return;
    };
    let Ok(mut data) = data else {
        return;
    };
    let sum = checksum(&data);
    if data.is_empty() || slot.loaded == Some(sum) {
        return;
    }
    data.extend_from_slice(&sum.to_le_bytes());
    // Unique per thread: several pipeline workers may compile the same pipeline.
    let temp = slot.path.with_extension(
        format!("{:?}.tmp", std::thread::current().id()).replace(['(', ')'], ""),
    );
    if std::fs::write(&temp, &data)
        .and_then(|()| std::fs::rename(&temp, &slot.path))
        .is_err()
    {
        let _ = std::fs::remove_file(&temp);
        return;
    }
    let written = WRITTEN.fetch_add(data.len() as u64, Ordering::Relaxed) + data.len() as u64;
    if written / EVICT_AFTER_WRITTEN != (written - data.len() as u64) / EVICT_AFTER_WRITTEN {
        if let Some(dir) = directory() {
            evict_oldest(dir);
        }
    }
}
