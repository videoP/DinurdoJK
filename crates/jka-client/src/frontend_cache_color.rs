//! Pixel transfer for reusable frontend snapshots. All PNG/cache image bytes are
//! sRGB, just as egui's ColorImage uploads expect. The screenshot readback is
//! taken AFTER display gamma, so undo its gamma on the screenshot worker and
//! apply the *current* gamma once when decoding the PNG on the cache worker.
//! Never run this code from the gameplay frame loop.

#[inline]
fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.04045 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
}

#[inline]
fn linear_to_srgb(value: f32) -> f32 {
    if value <= 0.0031308 { value * 12.92 } else { 1.055 * value.powf(1.0 / 2.4) - 0.055 }
}

/// `exponent` operates in the fragment shader's *linear* output values.
/// At capture: exponent = the captured gamma (undo `pow(linear, 1/gamma)`).
/// At load: exponent = 1/current gamma. For non-sRGB UNORM readbacks,
/// convert from linear UNORM to the sRGB byte encoding required by egui.
/// Premultiply/alpha is never touched: the menu screenshot is opaque.
pub(crate) fn transform_rgba(bytes: &mut [u8], exponent: f32, input_is_srgb: bool) {
    let exponent = if exponent.is_finite() { exponent.clamp(0.05, 8.0) } else { 1.0 };
    if (exponent - 1.0).abs() < 0.00001 && input_is_srgb {
        return;
    }
    let mut lookup = [0u8; 256];
    for (i, output) in lookup.iter_mut().enumerate() {
        let encoded = i as f32 / 255.0;
        let linear = if input_is_srgb { srgb_to_linear(encoded) } else { encoded };
        let adjusted = linear.powf(exponent);
        *output = (linear_to_srgb(adjusted.clamp(0.0, 1.0)) * 255.0).round() as u8;
    }
    for pixel in bytes.chunks_exact_mut(4) {
        pixel[0] = lookup[pixel[0] as usize];
        pixel[1] = lookup[pixel[1] as usize];
        pixel[2] = lookup[pixel[2] as usize];
    }
}

// The menu photograph cache is intentionally tiny: it is only a loading
// placeholder, not a screenshot gallery. Only v7 PNGs count toward this cap;
// each evicted PNG loses its matching metadata sidecar as well.
const MAX_FRONTEND_SCENE_SNAPSHOTS: usize = 8;

// Scene selection happens on the main thread. Maintenance runs exclusively on
// background decode/capture workers; the main thread only updates this pointer
// (no directory enumeration, timestamp writes or cleanup on the frame path).
static ACTIVE_FRONTEND_SCENE: std::sync::Mutex<Option<std::path::PathBuf>> =
    std::sync::Mutex::new(None);
static FRONTEND_CACHE_MAINTENANCE: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn set_active_frontend_scene(path: &std::path::Path) {
    if let Ok(mut active) = ACTIVE_FRONTEND_SCENE.lock() {
        *active = Some(path.to_path_buf());
    }
}

fn is_frontend_snapshot(path: &std::path::Path) -> bool {
    path.file_name().and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("v7_"))
        .and_then(|name| name.strip_suffix(".png"))
        .is_some_and(|hash| hash.len() == 16 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn touch_frontend_snapshot(path: &std::path::Path) {
    // Last-modified is our portable LRU clock. Opening for write is needed to
    // update file timestamps on Windows; no PNG bytes are changed.
    if let Ok(file) = std::fs::OpenOptions::new().write(true).open(path) {
        let times = std::fs::FileTimes::new().set_modified(std::time::SystemTime::now());
        if let Err(error) = file.set_times(times) {
            eprintln!("[MENU CACHE] could not update last-use time for {}: {error}", path.display());
        }
    }
}

/// Called only from background workers after a successful cache read, cache
/// selection with a resident texture, or a completed PNG publish. Also prunes
/// old entries on startup even when the requested image is a cache miss.
pub(crate) fn maintain_frontend_scene_cache(path: &std::path::Path, touch: bool) {
    let Ok(_maintenance) = FRONTEND_CACHE_MAINTENANCE.lock() else { return; };
    if touch {
        touch_frontend_snapshot(path);
    }
    let Some(dir) = path.parent() else { return; };
    let Ok(dir_entries) = std::fs::read_dir(dir) else { return; };
    let mut entries: Vec<_> = dir_entries.flatten().filter_map(|entry| {
        let candidate = entry.path();
        if !is_frontend_snapshot(&candidate) { return None; }
        let metadata = entry.metadata().ok()?;
        if !metadata.is_file() { return None; }
        let modified = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
        Some((modified, candidate))
    }).collect();
    if entries.len() <= MAX_FRONTEND_SCENE_SNAPSHOTS { return; }

    // Stable filename tie-break for filesystems with coarse timestamp precision.
    entries.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let mut remaining = entries.len();
    let mut evicted = 0;
    for (_, candidate) in entries {
        if remaining <= MAX_FRONTEND_SCENE_SNAPSHOTS { break; }
        // Always protect the just-loaded/just-written file, even when a new
        // scene was selected while its worker was still running.
        if candidate == path { continue; }
        // Hold this tiny lock through deletion: scene selection can never
        // change the active entry between our check and the remove_file call.
        let Ok(active) = ACTIVE_FRONTEND_SCENE.lock() else { break; };
        if active.as_deref() == Some(candidate.as_path()) { continue; }
        if std::fs::remove_file(&candidate).is_ok() {
            let _ = std::fs::remove_file(candidate.with_extension("json"));
            remaining -= 1;
            evicted += 1;
        }
    }
    if evicted > 0 {
        println!("[MENU CACHE] evicted {evicted} old scene snapshot(s), {remaining} remaining (max {MAX_FRONTEND_SCENE_SNAPSHOTS})");
    }
}

/// v7 cache metadata is written before the PNG is atomically published, so
/// readers never accept a partially created entry. File identity is also
/// verified against the current display/scene-settings hash in the filename.
pub(crate) fn cache_metadata_valid(path: &std::path::Path, neutral: bool) -> bool {
    let Ok(bytes) = std::fs::read(path.with_extension("json")) else { return false; };
    let Ok(metadata) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return false; };
    metadata["version"].as_u64() == Some(7)
        && metadata["neutral_gamma"].as_bool() == Some(neutral)
        && metadata["cache_file"].as_str() == path.file_name().and_then(|s| s.to_str())
        && metadata["transfer"].as_str() == Some("srgb8")
}

pub(crate) fn write_cache_metadata(
    path: &std::path::Path,
    neutral: bool,
    captured_output_gamma: f32,
    width: u32,
    height: u32,
) -> Result<(), String> {
    let metadata = serde_json::json!({
        "version": 7,
        "cache_file": path.file_name().and_then(|s| s.to_str()),
        "transfer": "srgb8",
        "neutral_gamma": neutral,
        "captured_output_gamma": captured_output_gamma,
        "width": width,
        "height": height,
        "note": "postprocessed PNG approximately gamma-neutralized for shader gamma when neutral_gamma=true; exact-settings cache otherwise",
    });
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, serde_json::to_vec_pretty(&metadata).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    std::fs::rename(temp, path.with_extension("json")).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_cache_keeps_eight_recent_screenshots_and_the_active_scene() {
        use std::time::{Duration, SystemTime};
        let unique = SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("jka-frontend-lru-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let paths: Vec<_> = (0..11).map(|i| {
            let path = dir.join(format!("v7_{i:016x}.png"));
            std::fs::write(&path, b"sample").unwrap();
            std::fs::write(path.with_extension("json"), b"{}").unwrap();
            let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            file.set_times(std::fs::FileTimes::new().set_modified(
                std::time::UNIX_EPOCH + Duration::from_secs(10_000 + i),
            )).unwrap();
            path
        }).collect();

        // Keep the oldest entry because it is displayed; give another old
        // entry a recent hit, and pin the just-written entry as well.
        set_active_frontend_scene(&paths[0]);
        maintain_frontend_scene_cache(&paths[1], true);
        maintain_frontend_scene_cache(&paths[10], false);
        let remaining = paths.iter().filter(|path| path.exists()).count();
        assert_eq!(remaining, MAX_FRONTEND_SCENE_SNAPSHOTS);
        for keep in [0, 1, 6, 7, 8, 9, 10] {
            assert!(paths[keep].exists(), "unexpected eviction of {keep}");
        }
        for evicted in [2, 3, 4] {
            assert!(!paths[evicted].exists(), "old PNG {evicted} survived");
            assert!(!paths[evicted].with_extension("json").exists(), "old sidecar survived");
        }
        set_active_frontend_scene(&dir.join("no-active-scene.png"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cache_replay_tracks_the_output_gamma_curve() {
        for gamma in [0.5, 0.8, 1.0, 1.3, 2.0, 3.0] {
            let mut rgba = Vec::new();
            for value in (48..=240u8).step_by(8) { rgba.extend([value, value, value, 255]); }
            let mut expected = rgba.clone();
            transform_rgba(&mut expected, 1.0 / gamma, true); // true shader output
            transform_rgba(&mut rgba, 1.0 / gamma, true); // screenshot readback
            transform_rgba(&mut rgba, gamma, true); // neutral cache at capture
            transform_rgba(&mut rgba, 1.0 / gamma, true); // regrade at load
            // sRGB8 quantizes before the readback, so exact recovery at the
            // blackest levels is impossible. Midtones should match closely.
            for (actual, target) in rgba.iter().zip(expected.iter()) {
                assert!(actual.abs_diff(*target) <= 3, "gamma={gamma}: {actual} vs {target}");
            }
        }
    }

    #[test]
    fn neutral_srgb_png_keeps_original_at_gamma_one() {
        let mut rgba = vec![0, 45, 128, 255, 200, 250, 17, 255];
        let original = rgba.clone();
        transform_rgba(&mut rgba, 1.0, true);
        assert_eq!(rgba, original);
    }

    #[test]
    fn unorm_capture_converts_linear_bytes_to_srgb() {
        let mut rgba = [128, 128, 128, 255];
        transform_rgba(&mut rgba, 1.0, false);
        assert!(rgba[0] > 180 && rgba[0] < 191);
        assert_eq!(rgba[3], 255);
    }
}
