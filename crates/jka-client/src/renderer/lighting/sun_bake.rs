//! Demand-driven sunlight reconstruction. No tracing or disk I/O in map upload.
use super::{
    original_sun::{self, BakedSunCalibration},
    sun_bake_cache as cache,
    sun_bake_upload::MaskUpload,
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
    sun: scene::DirectionalSun,
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
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

pub(in crate::renderer) struct SunBake {
    source: Option<Arc<Source>>,
    pending: Option<Pending>,
    pub(in crate::renderer) ready: bool,
    masks: Option<Vec<Vec<u8>>>,
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
            .zip(sun)
            .filter(|_| lightmaps.iter().any(|image| image.rgba16f.is_none()))
            .map(|(ao, sun)| {
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
        Self {
            source,
            pending: None,
            ready: false,
            masks: None,
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
        Some(
            self.upload
                .as_ref()?
                .apply(device, queue, images, bases, self.masks.as_ref()?),
        )
    }

    /// AO can finish before or after sunlight; patch only its output alpha.
    pub(in crate::renderer) fn patch_alpha(
        &self,
        texture: usize,
        rgba: &mut [u8],
        image: [u32; 2],
        rect: [u32; 4],
    ) {
        let Some(mask) = self.masks.as_ref().and_then(|masks| masks.get(texture)) else {
            return;
        };
        let Some(&base) = self.mask_sizes.get(texture) else {
            return;
        };
        patch_mask(mask, base, rgba, image, rect);
    }

    pub(in crate::renderer) fn has_masks(&self) -> bool {
        self.masks.is_some()
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

fn fingerprint(source: &Source) -> u64 {
    // Same deterministic FoldHasher on disk and across runs. Bump the cache
    // magic/version whenever reconstruction math changes.
    let mut hash = scene::FoldHasher::default();
    hash.write(b"baked-sun-v1");
    hash.write(&source.cache.map_hash.to_le_bytes());
    hash.write(bytemuck::cast_slice(&source.vertices));
    hash.write(bytemuck::cast_slice(&source.surface_ids));
    hash.write(format!("{:?}{:?}{:?}", source.sun, source.encoding, source.surfaces).as_bytes());
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
) -> Result<Completed, String> {
    lower_priority();
    let started = Instant::now();
    let key = fingerprint(source);
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
        let workers = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .saturating_sub(2)
            .clamp(1, 4);
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
            let mut images = if supported && source.encoding.is_some() {
                source.lightmaps.as_ref().clone()
            } else {
                Vec::new()
            };
            let projected = source.encoding.filter(|_| supported).and_then(|encoding| {
                original_sun::bake_authored_sun_masks(
                    &source.vertices,
                    &source.batches,
                    &source.textures,
                    &mut images,
                    &source.surface_ids,
                    source.surfaces.len(),
                    source.sun,
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
                    source.sun,
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

impl Renderer {
    /// One settings/ready/channel check in ordinary frames. Hashing, cache I/O,
    /// tracing and pipeline compilation stay on the background worker.
    pub(in crate::renderer) fn sync_sun_bake(&mut self) {
        let desired = requested(self.baked_sun_removal, self.baked_sun_debug_mode);
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
                Ok(result) => Some((!pending.cancelled.load(Ordering::Relaxed), result)),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some((true, Err("worker disconnected".into())))
                }
            });
        if let Some((current, outcome)) = outcome {
            drop(state.pending.take());
            if desired && current {
                match outcome {
                    Ok(result) => self.install_sun_bake(result),
                    Err(error) => {
                        let state = &mut self.world.as_mut().unwrap().sun_bake;
                        state.ready = true;
                        state.source = None;
                        eprintln!("[BAKED SUN] background preparation failed; authored lighting preserved: {error}");
                    }
                }
            }
        }
        if !desired {
            return;
        }
        let state = &mut self.world.as_mut().unwrap().sun_bake;
        if state.ready || state.pending.is_some() {
            return;
        }
        let Some(source) = state.source.clone() else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&cancelled);
        let device = self.device.clone();
        match std::thread::Builder::new()
            .name("jka-baked-sun-cache".into())
            .spawn(move || {
                let result = run(&source, &signal, &device);
                let _ = tx.send(result);
            }) {
            Ok(_) => state.pending = Some(Pending { rx, cancelled }),
            Err(error) => {
                state.ready = true;
                state.source = None;
                eprintln!("[BAKED SUN] could not start background worker: {error}");
            }
        }
    }

    fn install_sun_bake(&mut self, result: Completed) {
        let Some(mut world) = self.world.take() else {
            return;
        };
        let Some(data) = result.data else {
            world.sun_bake.ready = true;
            world.sun_bake.source = None;
            self.world = Some(world);
            return;
        };
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
                &masks,
            );
            world.baked_sun_encoding = world
                .sun_bake
                .source
                .as_ref()
                .and_then(|source| source.encoding);
            world.sun_bake.masks = Some(masks);
            world.sun_bake.upload = Some(upload);
            self.rebuild_world_surface_bind_groups(&mut world);
        }
        if !data.calibration.surfaces.is_empty() {
            self.queue.write_buffer(
                &world.legacy_dlight_surface_mask_buffer,
                (world.legacy_dlight_surfaces.len().max(1) * 4) as u64,
                bytemuck::cast_slice(&data.calibration.surfaces),
            );
        }
        world.baked_sun_radiance = Some(data.calibration.radiance);
        world.sun_bake.ready = true;
        // The worker's full tracing/material snapshot is no longer needed.
        // Retain only byte masks and dimensions for later AO updates.
        world.sun_bake.source = None;
        self.world = Some(world);
        self.history_valid = false;
        self.update_lighting_settings();
        self.sync_baked_brightness_assets();
        println!(
            "[BAKED SUN] {} ready after {:.1} ms in background; installed at frame boundary",
            if result.cache_hit {
                "lossless cache"
            } else {
                "reconstruction"
            },
            result.elapsed_ms
        );
    }
}

#[cfg(test)]
#[path = "sun_bake_tests.rs"]
mod tests;
