//! On-disk GPU pipeline cache (`base/cache/pipeline_cache_*.bin`).
//!
//! Superseded on Vulkan by the per-pipeline files of the patched wgpu-hal
//! (`vulkan/pipeline_disk_cache.rs`, see `install`); this blob remains the
//! `JKA_PIPELINE_CACHE_BLOB=1` fallback.
//!
//! Vulkan only: wgpu's DX12/GL backends have no pipeline cache, and every
//! `cache:` field then stays `None`. The cache object lives in a process-wide
//! slot so the ~100 pipeline builders (many of them free functions running on
//! pipeline-job worker threads) can pass it without threading a handle through.
//! wgpu validates the blob against the adapter/driver and `fallback: true`
//! turns a stale one into an empty cache; the checksum trailer below only
//! guards against torn or truncated files, which the driver must never see.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, RwLock};

/// The driver blob is opaque and append-only (stale entries from old builds are
/// never evicted, and it reached 840 MiB), so it cannot be trimmed oldest-first.
/// Past this size it is dropped and rebuilt instead.
const MAX_BLOB_BYTES: usize = 256 << 20;

struct Slot {
    cache: wgpu::PipelineCache,
    path: PathBuf,
    /// Checksum of the bytes currently on disk, to skip identical rewrites.
    on_disk: Option<u64>,
}

static SLOT: RwLock<Option<Slot>> = RwLock::new(None);
/// A pipeline was created since the last save, so the blob may have grown.
static DIRTY: AtomicBool = AtomicBool::new(false);
/// Serialises writers (periodic save thread vs shutdown save).
static WRITE: Mutex<()> = Mutex::new(());

fn checksum(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for chunk in bytes.chunks(8) {
        let mut word = [0u8; 8];
        word[..chunk.len()].copy_from_slice(chunk);
        hash ^= u64::from_le_bytes(word);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3).rotate_left(23);
    }
    hash ^ bytes.len() as u64
}

fn read_blob(path: &Path) -> Option<(Vec<u8>, u64)> {
    let mut bytes = std::fs::read(path).ok()?;
    if bytes.len() < 8 {
        return None;
    }
    let tail = bytes.split_off(bytes.len() - 8);
    let stored = u64::from_le_bytes(tail.try_into().ok()?);
    let sum = checksum(&bytes);
    (sum == stored && !bytes.is_empty()).then_some((bytes, sum))
}

/// Create the cache for this device. Call once per device, right after it is
/// created; a later call (backend restart) replaces the slot.
pub(crate) fn install(device: &wgpu::Device, info: &wgpu::AdapterInfo, base: &Path) {
    if !device.features().contains(wgpu::Features::PIPELINE_CACHE) {
        return;
    }
    let Some(key) = wgpu::util::pipeline_cache_key(info) else { return; };
    // Default: the patched wgpu-hal keeps one small cache file per pipeline in
    // base/cache/vk_pipelines/<adapter> and deletes the least recently used
    // ones past its size cap, instead of this single blob, which could only be
    // deleted whole (every pipeline then compiled cold, seconds each with RT).
    // Pipelines get `cache: None` (the slot stays empty) and the HAL picks the
    // file. Must run before the first pipeline: the HAL reads the variable once.
    // `JKA_PIPELINE_CACHE_BLOB=1` restores the single blob for A/B.
    if info.backend == wgpu::Backend::Vulkan
        && std::env::var("JKA_PIPELINE_CACHE_BLOB").map_or(true, |value| value.trim() != "1")
    {
        let dir = base.join("cache").join("vk_pipelines").join(&key);
        rverbose!(1, "[PIPELINE CACHE] per-pipeline files in {}", dir.display());
        std::env::set_var("DINURDOJK_VK_PIPELINE_CACHE", dir);
        *SLOT.write().unwrap_or_else(|e| e.into_inner()) = None;
        return;
    }
    let path = base.join("cache").join(format!("{key}.bin"));
    let started = std::time::Instant::now();
    let mut loaded = read_blob(&path);
    if loaded.as_ref().is_some_and(|(bytes, _)| bytes.len() > MAX_BLOB_BYTES) {
        rverbose!(1, "[PIPELINE CACHE] {} is over {} MiB; starting empty", path.display(), MAX_BLOB_BYTES >> 20);
        loaded = None;
    }
    let size = loaded.as_ref().map_or(0, |(bytes, _)| bytes.len());
    // SAFETY: `data` is either None or bytes previously returned by
    // `PipelineCache::get_data` that passed the checksum above; wgpu itself
    // rejects data from a different adapter or driver (fallback: true).
    let cache = unsafe {
        device.create_pipeline_cache(&wgpu::PipelineCacheDescriptor {
            label: Some("DinurdoJK pipeline cache"),
            data: loaded.as_ref().map(|(bytes, _)| bytes.as_slice()),
            fallback: true,
        })
    };
    rverbose!(
        1,
        "[PIPELINE CACHE] {} ({} KiB) {:.1} ms",
        path.display(), size / 1024, started.elapsed().as_secs_f64() * 1000.0
    );
    DIRTY.store(false, Ordering::Relaxed);
    let slot = Slot { cache, path, on_disk: loaded.map(|(_, sum)| sum) };
    *SLOT.write().unwrap_or_else(|e| e.into_inner()) = Some(slot);
}

/// The cache to put in a pipeline descriptor (`cache: pipeline_cache::get().as_ref()`).
/// Cheap: a read lock and a handle clone. Marks the cache dirty because the
/// caller is about to create a pipeline.
pub(crate) fn get() -> Option<wgpu::PipelineCache> {
    let slot = SLOT.read().unwrap_or_else(|e| e.into_inner());
    let cache = slot.as_ref().map(|slot| slot.cache.clone());
    if cache.is_some() {
        DIRTY.store(true, Ordering::Relaxed);
    }
    cache
}

/// Write the cache to disk if new pipelines were created since the last save.
/// Atomic (temp file + rename) so a crash never leaves a torn blob.
pub(crate) fn save() {
    if !DIRTY.swap(false, Ordering::Relaxed) {
        return;
    }
    let _writer = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let (cache, path, on_disk) = {
        let slot = SLOT.read().unwrap_or_else(|e| e.into_inner());
        let Some(slot) = slot.as_ref() else { return; };
        (slot.cache.clone(), slot.path.clone(), slot.on_disk)
    };
    let Some(mut data) = cache.get_data() else { return; };
    if data.len() > MAX_BLOB_BYTES {
        // Reset: the next launch rebuilds a fresh, small cache.
        let _ = std::fs::remove_file(&path);
        rverbose!(1, "[PIPELINE CACHE] {} KiB exceeds the cap; removed {}", data.len() / 1024, path.display());
        if let Some(slot) = SLOT.write().unwrap_or_else(|e| e.into_inner()).as_mut() {
            slot.on_disk = None;
        }
        return;
    }
    let sum = checksum(&data);
    if on_disk == Some(sum) {
        return;
    }
    data.extend_from_slice(&sum.to_le_bytes());
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let temp = path.with_extension("tmp");
    let written = std::fs::write(&temp, &data).and_then(|()| std::fs::rename(&temp, &path));
    match written {
        Ok(()) => {
            rverbose!(1, "[PIPELINE CACHE] saved {} KiB", data.len() / 1024);
            if let Some(slot) = SLOT.write().unwrap_or_else(|e| e.into_inner()).as_mut() {
                if slot.path == path {
                    slot.on_disk = Some(sum);
                }
            }
        }
        Err(error) => {
            eprintln!("[PIPELINE CACHE] could not write {}: {error}", path.display());
            let _ = std::fs::remove_file(&temp);
            DIRTY.store(true, Ordering::Relaxed);
        }
    }
}

/// `save()` off the render thread (get_data + a multi-MiB write).
pub(crate) fn save_in_background() {
    if !DIRTY.load(Ordering::Relaxed) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("pipeline-cache-save".into())
        .spawn(save);
}

/// Save and release the cache so it no longer keeps the device alive.
pub(crate) fn shutdown() {
    save();
    *SLOT.write().unwrap_or_else(|e| e.into_inner()) = None;
}
