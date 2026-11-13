//! Demand-driven sunlight reconstruction. No tracing or disk I/O in map upload.
use super::{
    original_sun::{self, BakedSunCalibration},
    sun_bake_cache as cache,
    sun_bake_upload::MaskUpload,
    sun_direction_fit::{self, DirectionFit},
};
use crate::renderer::{
    scene, Arc, DrawBatch, GpuImage, GpuVertex, Renderer, StaticAoWorldSource, TextureData,
};
use std::{
    hash::Hasher,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Instant,
};

struct Source {
    cache: scene::StaticBspAoCacheInfo,
    vertices: Arc<Vec<GpuVertex>>,
    batches: Vec<DrawBatch>,
    textures: Arc<Vec<TextureData>>,
    lightmaps: Arc<Vec<TextureData>>,
    surface_ids: Vec<u32>,
    surfaces: Vec<scene::LegacyDlightSurface>,
    sun: Option<scene::DirectionalSun>,
    encoding: Option<scene::BakedSunEncoding>,
}

pub(super) struct BakeData {
    pub(super) calibration: BakedSunCalibration,
    pub(super) masks: Option<Vec<Vec<u8>>>,
}

struct Completed {
    data: Option<BakeData>,
    upload: Option<MaskUpload>,
    cache_hit: bool,
    elapsed_ms: f64,
}

struct Pending {
    rx: mpsc::Receiver<Result<Completed, String>>,
    cancelled: Arc<AtomicBool>,
    started: Instant,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

struct DirectionPending {
    rx: mpsc::Receiver<Result<DirectionFit, String>>,
    cancelled: Arc<AtomicBool>,
}
impl Drop for DirectionPending {
    fn drop(&mut self) { self.cancelled.store(true, Ordering::Relaxed); }
}

pub(in crate::renderer) struct SunBake {
    source: Option<Arc<Source>>,
    pending: Option<Pending>,
    direction_pending: Option<DirectionPending>,
    direction_status: crate::renderer::BakedSunDirectionStatus,
    fitted_direction: Option<DirectionFit>,
    using_detected: bool,
    selected_sun: Option<scene::DirectionalSun>,
    selected_encoding: Option<scene::BakedSunEncoding>,
    pub(in crate::renderer) ready: bool,
    pub(in crate::renderer) auto_strength: Option<f32>,
    masks: Option<Vec<Vec<u8>>>,
    pub(in crate::renderer) contour_masks: Option<Vec<Vec<u8>>>,
    active_contour: bool,
    mask_sizes: Vec<[u32; 2]>,
    upload: Option<MaskUpload>,
}

fn requested(removal: f32, debug: u32) -> bool {
    removal > 0.0 || matches!(debug, 1 | 2)
}

impl SunBake {
    pub(in crate::renderer) fn new(
        ao: Option<&StaticAoWorldSource>,
        sun: Option<scene::DirectionalSun>,
        encoding: Option<scene::BakedSunEncoding>,
        batches: Vec<DrawBatch>,
        textures: Arc<Vec<TextureData>>,
        lightmaps: Arc<Vec<TextureData>>,
        surface_ids: &[u32],
        surfaces: &[scene::LegacyDlightSurface],
    ) -> Self {
        let source = ao
            .filter(|_| lightmaps.iter().any(|image| image.rgba16f.is_none()))
            .map(|ao| {
                Arc::new(Source {
                    cache: ao.cache.clone(),
                    vertices: Arc::clone(&ao.vertices),
                    batches,
                    textures,
                    lightmaps,
                    surface_ids: surface_ids.to_vec(),
                    surfaces: surfaces.to_vec(),
                    sun,
                    encoding,
                })
            });
        let mask_sizes = source.as_ref().map_or_else(Vec::new, |source| {
            source
                .lightmaps
                .iter()
                .map(|image| [image.width, image.height])
                .collect()
        });
        let supported = source.is_some();
        Self {
            source,
            pending: None,
            direction_pending: None,
            direction_status: if supported {
                crate::renderer::BakedSunDirectionStatus::Idle
            } else {
                crate::renderer::BakedSunDirectionStatus::Unavailable
            },
            fitted_direction: None,
            using_detected: false,
            selected_sun: sun,
            selected_encoding: encoding,
            ready: false,
            auto_strength: None,
            masks: None,
            contour_masks: None,
            active_contour: false,
            mask_sizes,
            upload: None,
        }
    }

    pub(in crate::renderer) fn mask_images(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        images: &[GpuImage],
        bases: &[TextureData],
    ) -> Option<Vec<GpuImage>> {
        Some(self.upload.as_ref()?.apply(device, queue, images, bases, self.active_mask()?))
    }

    /// AO can finish before or after sunlight; patch only its output alpha.
    pub(in crate::renderer) fn patch_alpha(
        &self,
        texture: usize,
        rgba: &mut [u8],
        image: [u32; 2],
        rect: [u32; 4],
    ) {
        let Some(mask) = self.active_mask().and_then(|masks| masks.get(texture)) else {
            return;
        };
        let Some(&base) = self.mask_sizes.get(texture) else {
            return;
        };
        patch_mask(mask, base, rgba, image, rect);
    }

    /// The original baked direction can differ from the current/edited map sun.
    pub(in crate::renderer) fn baked_direction(&self) -> Option<scene::DirectionalSun> {
        self.selected_sun
    }

    pub(in crate::renderer) fn has_masks(&self) -> bool {
        self.masks.is_some()
    }

    fn active_mask(&self) -> Option<&[Vec<u8>]> {
        if self.active_contour {
            self.contour_masks.as_deref()
        } else {
            self.masks.as_deref()
        }
    }
}

fn patch_mask(mask: &[u8], base: [u32; 2], rgba: &mut [u8], image: [u32; 2], rect: [u32; 4]) {
    let [x0, y0, width, height] = rect;
    if base.contains(&0)
        || image.contains(&0)
        || mask.len() != base[0] as usize * base[1] as usize
        || rgba.len() != width as usize * height as usize * 4
    {
        return;
    }
    for y in 0..height {
        for x in 0..width {
            let sx = (x0 + x) as f32 + 0.5;
            let sy = (y0 + y) as f32 + 0.5;
            let sx = sx * base[0] as f32 / image[0] as f32 - 0.5;
            let sy = sy * base[1] as f32 / image[1] as f32 - 0.5;
            let ix = sx.floor() as i32;
            let iy = sy.floor() as i32;
            let fx = sx - sx.floor();
            let fy = sy - sy.floor();
            let sample = |dx: i32, dy: i32| {
                let bx = (ix + dx).clamp(0, base[0] as i32 - 1) as usize;
                let by = (iy + dy).clamp(0, base[1] as i32 - 1) as usize;
                f32::from(mask[by * base[0] as usize + bx])
            };
            let alpha = (sample(0, 0) * (1.0 - fx) + sample(1, 0) * fx) * (1.0 - fy)
                + (sample(0, 1) * (1.0 - fx) + sample(1, 1) * fx) * fy;
            rgba[(y as usize * width as usize + x as usize) * 4 + 3] =
                alpha.round().clamp(0.0, 255.0) as u8;
        }
    }
}

fn fingerprint(source: &Source, sun: scene::DirectionalSun, encoding: Option<scene::BakedSunEncoding>) -> u64 {
    // Same deterministic FoldHasher on disk and across runs. Bump the cache
    // magic/version whenever reconstruction math changes.
    let mut hash = scene::FoldHasher::default();
    hash.write(b"baked-sun-v6-direction-search");
    hash.write(&source.cache.map_hash.to_le_bytes());
    hash.write(bytemuck::cast_slice(&source.vertices));
    hash.write(bytemuck::cast_slice(&source.surface_ids));
    hash.write(format!("{:?}{:?}{:?}", sun, encoding, source.surfaces).as_bytes());
    let mut filtered = std::collections::BTreeSet::new();
    for batch in &source.batches {
        hash.write(format!("{batch:?}").as_bytes());
        if batch.alpha_shadow || batch.light_filter {
            filtered.extend(batch.baked_shadow_texture);
        }
    }
    for index in filtered {
        hash.write(&(index as u64).to_le_bytes());
        if let Some(image) = source.textures.get(index) {
            hash.write(&image.width.to_le_bytes());
            hash.write(&image.height.to_le_bytes());
            hash.write(&image.rgba);
        }
    }
    for image in source.lightmaps.iter() {
        hash.write(&image.width.to_le_bytes());
        hash.write(&image.height.to_le_bytes());
        hash.write(&[u8::from(image.srgb)]);
        hash.write(&image.mip_level_count.to_le_bytes());
        hash.write(&image.rgba);
        if let Some(pixels) = image.rgba16f.as_ref() {
            hash.write(bytemuck::cast_slice(pixels));
        }
    }
    hash.finish()
}

fn cache_path(source: &Source, key: u64) -> PathBuf {
    let name: String = source
        .cache
        .map_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    source
        .cache
        .directory
        .parent()
        .unwrap_or(&source.cache.directory)
        .join("baked-sun")
        .join(format!(
            "{name}-{:016x}-{key:016x}.sunpng",
            source.cache.map_hash
        ))
}

fn lower_priority() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
        };
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

fn run(
    source: &Source,
    cancelled: &AtomicBool,
    device: &wgpu::Device,
    worker_override: u8,
    sun: scene::DirectionalSun,
    encoding: Option<scene::BakedSunEncoding>,
) -> Result<Completed, String> {
    lower_priority();
    let started = Instant::now();
    let key = fingerprint(source, sun, encoding);
    let path = cache_path(source, key);
    let sizes: Vec<_> = source
        .lightmaps
        .iter()
        .map(|image| [image.width, image.height])
        .collect();
    if cancelled.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    let cached = cache::read(&path, key, source.surfaces.len(), &sizes);
    let cache_hit = cached.is_some();
    let data = if let Some(data) = cached {
        Some(data)
    } else {
        let workers = super::lightmap_sun::bake_worker_count(worker_override, 2);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .thread_name(|index| format!("jka-baked-sun-{index}"))
            .start_handler(|_| lower_priority())
            .build()
            .map_err(|e| e.to_string())?;
        println!(
            "[BAKED SUN] background cache miss: {} worker(s), {}",
            workers, source.cache.map_name
        );
        let data = pool.install(|| {
            let supported = source
                .lightmaps
                .iter()
                .all(|image| image.rgba16f.is_none() && image.mip_level_count == 1);
            let mut images = if supported && encoding.is_some() {
                source.lightmaps.as_ref().clone()
            } else {
                Vec::new()
            };
            let projected = encoding.filter(|_| supported).and_then(|encoding| {
                original_sun::bake_authored_sun_masks(
                    &source.vertices,
                    &source.batches,
                    &source.textures,
                    &mut images,
                    &source.surface_ids,
                    source.surfaces.len(),
                    sun,
                    encoding,
                    cancelled,
                )
            });
            let masks = projected.as_ref().map(|_| {
                images
                    .iter()
                    .map(|image| image.rgba.chunks_exact(4).map(|pixel| pixel[3]).collect())
                    .collect()
            });
            let calibration = projected.or_else(|| {
                if cancelled.load(Ordering::Relaxed) {
                    return None;
                }
                original_sun::estimate_baked_sun_radiance(
                    &source.vertices,
                    &source.batches,
                    &source.lightmaps,
                    &source.surface_ids,
                    &source.surfaces,
                    sun,
                    cancelled,
                )
            });
            calibration.map(|calibration| BakeData { calibration, masks })
        });
        if cancelled.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        if let Some(data) = data.as_ref() {
            match cache::write(&path, key, data) {
                Ok(bytes) => println!(
                    "[BAKED SUN] lossless cache saved: {:.2} MiB {}",
                    bytes as f64 / 1048576.0,
                    path.display()
                ),
                Err(error) => {
                    eprintln!("[BAKED SUN] cache write failed (using completed result): {error}")
                }
            }
        }
        data
    };
    if cancelled.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    let upload = data
        .as_ref()
        .and_then(|data| data.masks.as_ref())
        .map(|_| MaskUpload::new(device));
    Ok(Completed {
        data,
        upload,
        cache_hit,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
    })
}

fn direction_selection_status(fit: Option<DirectionFit>, using_detected: bool)
    -> crate::renderer::BakedSunDirectionStatus
{
    fit.map_or(crate::renderer::BakedSunDirectionStatus::Idle, |fit| {
        crate::renderer::BakedSunDirectionStatus::Found {
            azimuth: fit.azimuth,
            elevation: fit.elevation,
            confidence: fit.confidence,
            using_detected,
        }
    })
}

impl Renderer {
    pub(in crate::renderer) fn baked_sun_direction_status(&self) -> crate::renderer::BakedSunDirectionStatus {
        self.world.as_ref().filter(|world| !world.source_map)
            .map_or(crate::renderer::BakedSunDirectionStatus::Unavailable,
                |world| world.sun_bake.direction_status)
    }

    /// Explicit one-shot request. Only the worker traces candidate shadows.
    /// The rendering thread merely installs a finished fit at a frame boundary.
    pub(in crate::renderer) fn find_baked_sun_direction(&mut self) {
        let Some(world) = self.world.as_mut().filter(|world| !world.source_map) else { return; };
        let state = &mut world.sun_bake;
        if state.direction_pending.is_some() { return; }
        let Some(source) = state.source.clone() else {
            state.direction_status = crate::renderer::BakedSunDirectionStatus::Unavailable;
            return;
        };
        let (tx, rx) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&cancelled);
        match std::thread::Builder::new().name("jka-sun-direction-search".into()).spawn(move || {
            lower_priority();
            let result = sun_direction_fit::search(&source.vertices, &source.batches, &source.lightmaps, &signal);
            let _ = tx.send(result);
        }) {
            Ok(_) => {
                state.direction_pending = Some(DirectionPending { rx, cancelled });
                state.direction_status = crate::renderer::BakedSunDirectionStatus::Searching;
                println!("[BAKED SUN] Searching low-resolution shadow gradients for original yaw/pitch");
            }
            Err(error) => {
                state.direction_status = crate::renderer::BakedSunDirectionStatus::Failed;
                eprintln!("[BAKED SUN] Could not launch direction search: {error}");
            }
        }
    }

    /// Switch the *baked* sun source, never the live sky. A detected fit is
    /// retained for the current map so both sources can be selected repeatedly
    /// without running the expensive directional search again.
    pub(in crate::renderer) fn select_baked_sun_direction(&mut self, detected: bool) {
        self.select_baked_sun_direction_impl(detected, false);
    }

    fn select_baked_sun_direction_impl(&mut self, detected: bool, fit_changed: bool) {
        let Some(world) = self.world.as_mut().filter(|world| !world.source_map) else { return; };
        let state = &mut world.sun_bake;
        let Some(source) = state.source.as_ref() else { return; };
        let (sun, encoding) = if detected {
            let Some(fit) = state.fitted_direction else { return; };
            let color = source.sun.map_or([1.0; 3], |sun| sun.color);
            let intensity = source.sun.map_or(350.0, |sun| sun.intensity.max(1.0));
            (Some(scene::DirectionalSun::from_q3_angles(color, intensity, fit.azimuth, fit.elevation)),
                Some(source.encoding.unwrap_or(scene::BakedSunEncoding {
                    exposure: 0.0, gamma: 1.0, compensate: 1.0, brightness: 1.0,
                    deviance: 0.0, samples: 1,
                })))
        } else {
            let Some(sun) = source.sun else { return; };
            (Some(sun), source.encoding)
        };
        if state.using_detected == detected && !fit_changed {
            // The selected source is already active. If a search is still
            // running, selecting it again only cancels that search, retaining
            // the finished calibration and currently bound masks.
            if state.direction_pending.take().is_some() {
                state.direction_status = direction_selection_status(state.fitted_direction, detected);
            }
            return;
        }
        // Cancel outstanding search/reconstruction. A cancelled receiver cannot
        // later install its old direction or mask after the user switches back.
        state.direction_pending.take();
        state.pending.take();
        state.selected_sun = sun;
        state.selected_encoding = encoding;
        state.using_detected = detected;
        state.direction_status = direction_selection_status(state.fitted_direction, detected);
        state.ready = false;
        state.auto_strength = None;
        let had_masks = state.masks.take().is_some() | state.contour_masks.take().is_some();
        state.active_contour = false;
        // A previous fit may have installed its alpha mask into the original
        // lightmaps. Restore their source alpha, preserving the current AO RGB,
        // before preparing another fit. No per-frame GPU work is introduced.
        if had_masks {
            if let Some(upload) = state.upload.as_ref() {
                let alphas: Vec<Vec<u8>> = world.lightmap_base_data.iter().map(|base| {
                    base.rgba.chunks_exact(4).map(|pixel| pixel[3]).collect()
                }).collect();
                let source_images = world.baked_sun_cache.as_ref()
                    .map_or(&world.lightmaps, |cache| &cache.original);
                let restored = upload.apply(&self.device, &self.queue, source_images,
                    &world.lightmap_base_data, &alphas);
                if let Some(cache) = world.baked_sun_cache.as_mut() {
                    cache.original = restored.clone();
                    cache.active = false;
                    cache.strength = None;
                }
                world.lightmaps = restored;
            }
        }
        // Keep the warmed mask-upload pipeline for an eventual next install.
        world.baked_sun_radiance = None;
        world.baked_sun_encoding = None;
        let source_name = if detected { "detected" } else { "authored map" };
        println!("[BAKED SUN] Using {source_name} direction; refreshing sunlight calibration");
        // The lightmap source can have changed even when the composition is
        // active; always rebind immediately after a one-shot alpha restoration.
        self.rebuild_frame_plan();
        self.sync_runtime_variant_flags();
        // update_lighting_settings and binding rebuild are performed below.
        let mut world = self.world.take().unwrap();
        self.rebuild_world_surface_bind_groups(&mut world);
        self.world = Some(world);
        self.update_lighting_settings();
    }

    /// Read-only: the UI receives a notification only when this result changes.
    /// Never expose a previous map's fitted value through the saved manual cvar.
    pub(in crate::renderer) fn baked_sun_auto_status(&self) -> crate::renderer::BakedSunAutoStatus {
        use crate::renderer::BakedSunAutoStatus;
        let Some(bake) = self.world.as_ref().map(|world| &world.sun_bake) else {
            return BakedSunAutoStatus::Unavailable;
        };
        if bake.ready {
            if self.baked_sun_removal_contour {
                return if bake.contour_masks.is_some() { BakedSunAutoStatus::ContourGuided }
                    else { BakedSunAutoStatus::Unavailable };
            }
            return match bake.auto_strength.filter(|value| value.is_finite() && *value >= 0.0) {
                Some(value) => BakedSunAutoStatus::Calibrated(value),
                None => BakedSunAutoStatus::Unavailable,
            };
        }
        if bake.selected_sun.is_none() || (bake.source.is_none() && bake.pending.is_none()) {
            BakedSunAutoStatus::Unavailable
        } else {
            BakedSunAutoStatus::Calibrating
        }
    }

    /// One settings/ready/channel check in ordinary frames. Hashing, cache I/O,
    /// tracing and pipeline compilation stay on the background worker.
    pub(in crate::renderer) fn sync_sun_bake(&mut self) {
        let direction_result = self.world.as_mut().and_then(|world| {
            let state = &mut world.sun_bake;
            let result = state.direction_pending.as_ref().and_then(|pending| match pending.rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => Some(Err("direction worker disconnected".into())),
                Err(mpsc::TryRecvError::Empty) => None,
            });
            if result.is_some() { state.direction_pending.take(); }
            result
        });
        if let Some(result) = direction_result {
            match result {
                Ok(fit) => {
                    self.world.as_mut().unwrap().sun_bake.fitted_direction = Some(fit);
                    self.select_baked_sun_direction_impl(true, true);
                    println!("[BAKED SUN] Matched original sun: yaw {:.2}°, pitch {:.2}°, score {:.3}, {} shadow edges. Rebuilding removal masks.",
                        fit.azimuth, fit.elevation, fit.confidence, fit.transitions);
                }
                Err(error) => {
                    if let Some(world) = self.world.as_mut() {
                        let state = &mut world.sun_bake;
                        state.direction_status = if state.fitted_direction.is_some() {
                            direction_selection_status(state.fitted_direction, state.using_detected)
                        } else {
                            crate::renderer::BakedSunDirectionStatus::Failed
                        };
                    }
                    eprintln!("[BAKED SUN] Sun-direction search rejected: {error}; original lightmaps preserved");
                }
            }
        }
        let desired = self.baked_sun_removal_auto || self.baked_sun_removal_contour
            || requested(self.effective_baked_sun_removal(), self.baked_sun_debug_mode);
        let worker_override = self.bake_threads;
        let Some(world) = self.world.as_mut() else {
            return;
        };
        let state = &mut world.sun_bake;
        if !desired {
            if let Some(pending) = state.pending.as_ref() {
                pending.cancelled.store(true, Ordering::Relaxed);
            }
        }
        let outcome = state
            .pending
            .as_ref()
            .and_then(|pending| match pending.rx.try_recv() {
                Ok(result) => Some((!pending.cancelled.load(Ordering::Relaxed), result, pending.started)),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some((true, Err("worker disconnected".into()), pending.started))
                }
            });
        if let Some((current, outcome, started)) = outcome {
            drop(state.pending.take());
            if desired && current {
                match outcome {
                    Ok(result) => self.install_sun_bake(result, started),
                    Err(error) => {
                        let state = &mut self.world.as_mut().unwrap().sun_bake;
                        state.ready = true;
                        eprintln!(
                            "[BAKED SUN] background preparation failed after {:.2}s; authored lighting preserved: {error}",
                            started.elapsed().as_secs_f64()
                        );
                    }
                }
            }
        }
        // Switching Auto/Contour Guided is a one-shot alpha reinstallation,
        // never a retrace, per-frame pixel branch or new world shader variant.
        let switch = self.world.as_ref().is_some_and(|world| {
            let bake = &world.sun_bake;
            let use_contour = self.baked_sun_removal_contour && self.baked_sun_debug_mode != 2
                && bake.contour_masks.is_some();
            bake.ready && bake.masks.is_some() && bake.active_contour != use_contour
        });
        if switch { self.switch_sun_removal_mask(); }
        if !desired {
            return;
        }
        let state = &mut self.world.as_mut().unwrap().sun_bake;
        if state.ready || state.pending.is_some() || state.direction_pending.is_some() {
            return;
        }
        let Some(sun) = state.selected_sun else { return; };
        let encoding = state.selected_encoding;
        let Some(source) = state.source.clone() else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&cancelled);
        let device = self.device.clone();
        let started = Instant::now();
        match std::thread::Builder::new()
            .name("jka-baked-sun-cache".into())
            .spawn(move || {
                let result = run(&source, &signal, &device, worker_override, sun, encoding);
                let _ = tx.send(result);
            }) {
            Ok(_) => state.pending = Some(Pending { rx, cancelled, started }),
            Err(error) => {
                state.ready = true;
                eprintln!("[BAKED SUN] could not start background worker: {error}");
            }
        }
    }

    fn switch_sun_removal_mask(&mut self) {
        let Some(mut world) = self.world.take() else { return; };
        let contour = self.baked_sun_removal_contour && self.baked_sun_debug_mode != 2
            && world.sun_bake.contour_masks.is_some();
        if let (Some(upload), Some(masks)) = (world.sun_bake.upload.as_ref(),
            if contour { world.sun_bake.contour_masks.as_deref() } else { world.sun_bake.masks.as_deref() }) {
            // Preserve the warm compositing pipeline, chart textures, and sky
            // resources. Only exchange the immutable alpha source and force a
            // one-shot recomposition; never rebuild a pipeline on mode changes.
            let source = world.baked_sun_cache.as_ref().map(|cache| &cache.original)
                .unwrap_or(&world.lightmaps);
            let replacement = upload.apply(&self.device, &self.queue, source,
                &world.lightmap_base_data, masks);
            if let Some(cache) = world.baked_sun_cache.as_mut() {
                cache.original = replacement.clone();
                cache.strength = None;
            }
            world.lightmaps = replacement;
            world.sun_bake.active_contour = contour;
            self.rebuild_world_surface_bind_groups(&mut world);
        }
        self.world = Some(world);
        self.history_valid = false;
        self.update_lighting_settings();
        println!("[BAKED SUN] switched cached removal mask to {}", if contour { "contour guided" } else { "original" });
    }

    fn install_sun_bake(&mut self, result: Completed, started: Instant) {
        let Some(mut world) = self.world.take() else {
            return;
        };
        let Some(data) = result.data else {
            world.sun_bake.ready = true;
            // Keep source for the explicit search/retry button.
            self.world = Some(world);
            println!(
                "[BAKED SUN] calibration unavailable after {:.2}s (background {:.2}s); authored lighting retained",
                started.elapsed().as_secs_f64(), result.elapsed_ms / 1000.0
            );
            return;
        };
        let mut calibration = data.calibration;
        world.sun_bake.contour_masks = calibration.contour_masks.take();
        let contour = self.baked_sun_removal_contour && self.baked_sun_debug_mode != 2
            && world.sun_bake.contour_masks.is_some();
        if let (Some(masks), Some(upload)) = (data.masks, result.upload) {
            let images = world
                .baked_sun_cache
                .take()
                .map(|cache| cache.original)
                .unwrap_or_else(|| std::mem::take(&mut world.lightmaps));
            world.lightmaps = upload.apply(
                &self.device,
                &self.queue,
                &images,
                &world.lightmap_base_data,
                if contour { world.sun_bake.contour_masks.as_ref().unwrap() } else { &masks },
            );
            world.sun_bake.active_contour = contour;
            world.baked_sun_encoding = world.sun_bake.selected_encoding;
            world.sun_bake.masks = Some(masks);
            world.sun_bake.upload = Some(upload);
            self.rebuild_world_surface_bind_groups(&mut world);
        }
        if !calibration.surfaces.is_empty() {
            self.queue.write_buffer(
                &world.legacy_dlight_surface_mask_buffer,
                (world.legacy_dlight_surfaces.len().max(1) * 4) as u64,
                bytemuck::cast_slice(&calibration.surfaces),
            );
        }
        world.sun_bake.auto_strength = calibration.auto_strength;
        if self.baked_sun_removal_auto {
            match calibration.auto_strength {
                Some(strength) => println!("[BAKED SUN AUTO] applying calibrated removal {strength:.3}x"),
                None => println!("[BAKED SUN AUTO] insufficient reliable shadow evidence; preserving authored lightmap"),
            }
        }
        if self.baked_sun_removal_contour {
            println!("[BAKED SUN CONTOUR] {}", if contour {
                format!("{} local contour regions ready", calibration.contour_regions)
            } else { "insufficient valid contour evidence; original lighting preserved".to_string() });
        }
        world.baked_sun_radiance = Some(calibration.radiance);
        world.sun_bake.ready = true;
        // Retain the immutable source for explicit correction/retry. Vertices
        // and lightmaps are Arc-shared; no per-frame tracing is added.
        self.world = Some(world);
        self.history_valid = false;
        self.update_lighting_settings();
        self.sync_baked_brightness_assets();
        println!(
            "[BAKED SUN] {} ready in {:.2}s total (background {:.2}s); installed at frame boundary",
            if result.cache_hit { "lossless cache" } else { "reconstruction" },
            started.elapsed().as_secs_f64(),
            result.elapsed_ms / 1000.0
        );
    }
}

#[cfg(test)]
#[path = "sun_bake_tests.rs"]
mod tests;
