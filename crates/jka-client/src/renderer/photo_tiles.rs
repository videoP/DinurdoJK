//! World sweep for every panorama plan, plus the bounded disk fallback.
//! Whole faces and GPU tiles are copied into a GPU cube in queue order; disk
//! tiles go through a 3-slot readback ring into a tile-major page file that a
//! parallel CPU projector samples while the encoder streams rows.
use super::{plan::{CapturePlan, WorldPlan, PAGE, READBACK_SLOTS}, readback::ReadbackRing, stream, CapturePreparation, CaptureSceneState, GpuWatch,
    PhotoCaptureRequest, PhotoPerf, Renderer, ScreenshotOutput, UserEvent, wait_capture_preparation, poll_capture_preparation, check_cancel, progress};
use crate::camera::Camera;
use rayon::prelude::*;
use std::{collections::{BTreeMap, HashMap, VecDeque}, fs::{self, File, OpenOptions}, path::PathBuf,
    sync::{atomic::{AtomicU64, Ordering}, Arc, Mutex}, time::{Duration, Instant}};
use winit::event_loop::EventLoopProxy;

const PAGE_BYTES: usize = PAGE as usize * PAGE as usize * 3;
/// GPU-cube copies recorded ahead of the oldest unfinished world render.
const IN_FLIGHT: usize = 2;
const BAND: u32 = 64;
const MAX_RESTARTS: u32 = 128;

fn io_err(e: impl std::fmt::Display) -> String { format!("Tiled screenshot: {e}") }

// A private scratch directory (not indexed as a screenshot) owns all partial
// files. Dropping it removes data on success, error, or Rust unwind.
struct Scratch(PathBuf);
impl Drop for Scratch { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

#[derive(Clone, Copy)]
pub(super) enum Target<'a> { Cube(&'a wgpu::Texture), Disk(&'a PageStore) }

#[derive(Clone, Copy)]
struct TileTag { face: u32, tx: u32, ty: u32, w: u32, h: u32 }

fn wait_submission(source: &Renderer, index: wgpu::SubmissionIndex, perf: &mut PhotoPerf) -> Result<(), String> {
    let started = Instant::now();
    let result = source.device.poll(wgpu::PollType::Wait { submission_index: Some(index), timeout: Some(Duration::from_secs(60)) })
        .map(|_| ()).map_err(|e| format!("Screenshot GPU wait failed: {e:?}"));
    perf.gpu_wait += started.elapsed();
    result
}

/// Render every face/tile of `plan` into `target`. A complete stable sweep is
/// the commit unit: any preparation change restarts it from the first tile.
/// Readiness is read after each render (CPU-recorded transitions) and again
/// after all of the sweep's GPU work completed (completion-driven ones).
pub(super) fn sweep(
    source: &mut Renderer, request: &PhotoCaptureRequest, mut camera: Camera, faces: [(f32, f32, &str); 6],
    plan: &CapturePlan, scene: &CaptureSceneState, render: &impl Fn(&mut Renderer, &Camera) -> Result<(), String>,
    target: Target<'_>, proxy: &EventLoopProxy<UserEvent>, perf: &mut PhotoPerf, watch: &mut GpuWatch,
) -> Result<(), String> {
    let (n, core, guard, side) = (plan.tiles_per_side, plan.core, plan.guard, plan.face);
    let total = u64::from(plan.world_renders());
    let pitch = super::plan::padded_row(core).ok_or("Tile row pitch overflow")?;
    let mut ring = match target {
        Target::Disk(_) => Some(ReadbackRing::<TileTag>::new(&source.device, READBACK_SLOTS, u64::from(pitch) * u64::from(core), "Photo tile readback")),
        Target::Cube(_) => None,
    };
    if let Some(ring) = &ring { perf.peak_gpu_buffers = perf.peak_gpu_buffers.max(ring.bytes()); }
    let bgra = source.offscreen_output.as_ref().is_some_and(|t| matches!(t.format(), wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb));
    let mut consume = |tag: TileTag, bytes: &[u8]| -> Result<(), String> {
        match target { Target::Disk(store) => store.write_tile(tag, bytes, pitch, bgra), Target::Cube(_) => Ok(()) }
    };
    let yaw = camera.yaw;
    let label = if plan.world == WorldPlan::Faces { "cube faces" } else { "cube tiles" };
    let mut last_progress = Instant::now();
    let mut restarts = 0u32;
    'prepare: loop {
        check_cancel(&request.cancel)?;
        let started = Instant::now();
        poll_capture_preparation(source);
        let before = CapturePreparation::read(source);
        let mut window = VecDeque::new();
        let mut done = 0u64;
        let mut restart = |source: &mut Renderer, ring: &mut Option<ReadbackRing<TileTag>>, window: &mut VecDeque<wgpu::SubmissionIndex>, done: u64, perf: &mut PhotoPerf, reason: &'static str| -> Result<(), String> {
            restarts += 1;
            perf.restarts += 1;
            perf.discarded_renders += done;
            perf.restart_reason = reason;
            if restarts == 1 { eprintln!("[PHOTO] preparing live scene resources before panorama ({reason})"); }
            if restarts > MAX_RESTARTS { return Err("Panorama capture resources never stabilized; no image saved".into()); }
            if let Some(ring) = ring { ring.discard(&source.device)?; }
            if let Some(last) = window.drain(..).last() { wait_submission(source, last, perf)?; }
            let wait = Instant::now();
            let result = wait_capture_preparation(source, Instant::now() + Duration::from_secs(300), &request.cancel);
            perf.preparation_wait += wait.elapsed();
            result
        };
        for (face, (offset, pitch_angle, _)) in faces.into_iter().enumerate() {
            camera.yaw = yaw + offset;
            camera.pitch = pitch_angle;
            for ty in 0..n { for tx in 0..n {
                check_cancel(&request.cancel)?;
                let (x0, y0) = (tx * core, ty * core);
                let (w, h) = (core.min(side - x0), core.min(side - y0));
                camera.set_photo_tile(side, x0 as i32 - guard as i32, y0 as i32 - guard as i32, plan.render);
                let render_started = Instant::now();
                render(source, &camera)?;
                perf.render_cpu += render_started.elapsed();
                perf.world_renders += 1;
                // Stop at the first failed allocation, not after a sweep of
                // invalid resources; the caller retries with a smaller plan.
                watch.check()?;
                perf.sample_vram(source);
                let after = CapturePreparation::read(source);
                if before.pending || after.pending || before != after {
                    restart(source, &mut ring, &mut window, done + 1, perf, if after.pending { "pending resources" } else { "resource generation changed" })?;
                    continue 'prepare;
                }
                let image = source.offscreen_output.as_ref().ok_or("Missing capture output")?;
                if image.width() != plan.render || image.height() != plan.render { return Err("Incorrect tile render dimensions".into()); }
                let mut encoder = source.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Photo tile copy") });
                let origin = wgpu::TexelCopyTextureInfo { texture: image, mip_level: 0, origin: wgpu::Origin3d { x: guard, y: guard, z: 0 }, aspect: wgpu::TextureAspect::All };
                let extent = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
                match target {
                    Target::Cube(cube) => {
                        encoder.copy_texture_to_texture(origin, wgpu::TexelCopyTextureInfo { texture: cube, mip_level: 0,
                            origin: wgpu::Origin3d { x: x0, y: y0, z: face as u32 }, aspect: wgpu::TextureAspect::All }, extent);
                        window.push_back(source.queue.submit([encoder.finish()]));
                        while window.len() > IN_FLIGHT {
                            let oldest = window.pop_front().unwrap();
                            wait_submission(source, oldest, perf)?;
                        }
                    }
                    Target::Disk(_) => {
                        let ring = ring.as_mut().unwrap();
                        let slot = ring.acquire(&source.device, &mut consume)?;
                        encoder.copy_texture_to_buffer(origin, wgpu::TexelCopyBufferInfo { buffer: ring.buffer(slot),
                            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(pitch), rows_per_image: Some(h) } }, extent);
                        let submission = source.queue.submit([encoder.finish()]);
                        ring.submitted(slot, TileTag { face: face as u32, tx, ty, w, h }, submission);
                        ring.poll(&source.device, &mut consume)?;
                    }
                }
                done += 1;
                if last_progress.elapsed() >= Duration::from_millis(500) || done == total {
                    progress(proxy, format!("PHOTO: rendering {label} {done}/{total} ({})", plan.label()), done, total, started);
                    last_progress = Instant::now();
                }
            }}
        }
        // Complete the sweep's GPU work, then re-read readiness: a pool may be
        // discovered only by a tile facing it, or settle on GPU completion.
        if let Some(ring) = ring.as_mut() { ring.drain(&source.device, &mut consume)?; }
        if let Some(last) = window.drain(..).last() { wait_submission(source, last, perf)?; }
        poll_capture_preparation(source);
        let after = CapturePreparation::read(source);
        if after.pending || before != after || !scene.ninja_pools_ready(source) {
            restart(source, &mut ring, &mut window, done, perf, if after.pending || before != after { "settled after sweep" } else { "ocean pools" })?;
            continue 'prepare;
        }
        scene.verify_ready(source)?;
        perf.sweep += started.elapsed();
        break;
    }
    if let Some(ring) = &ring {
        perf.readback_wait += ring.wait;
        perf.store_write += ring.consume;
    }
    Ok(())
}

/// Tile-major RGB8 page file: each world tile's pages are contiguous, so a
/// tile is one sequential write. Offsets are u64 throughout.
pub(super) struct PageStore {
    file: File,
    side: u32,
    core: u32,
    n: u32,
    k: u32,
    scratch: Mutex<Vec<u8>>,
    pub(super) written: AtomicU64,
    pub(super) read: AtomicU64,
}
impl PageStore {
    fn create(path: &std::path::Path, plan: &CapturePlan) -> Result<Self, String> {
        let file = OpenOptions::new().create_new(true).read(true).write(true).open(path).map_err(io_err)?;
        let k = plan.page_tiles();
        if k == 0 || plan.core % PAGE != 0 { return Err("Invalid disk tile layout".into()); }
        Ok(Self { file, side: plan.face, core: plan.core, n: plan.tiles_per_side, k, scratch: Mutex::new(Vec::new()), written: AtomicU64::new(0), read: AtomicU64::new(0) })
    }
    fn tile_key(&self, face: u32, tx: u32, ty: u32) -> u64 {
        ((u64::from(face) * u64::from(self.n) + u64::from(ty)) * u64::from(self.n) + u64::from(tx)) * u64::from(self.k) * u64::from(self.k)
    }
    /// Page key and byte offset inside the page for an in-face texel.
    fn locate(&self, face: u32, x: u32, y: u32) -> (u64, usize) {
        let (tx, ty, lx, ly) = (x / self.core, y / self.core, x % self.core, y % self.core);
        let key = self.tile_key(face, tx, ty) + u64::from(ly / PAGE) * u64::from(self.k) + u64::from(lx / PAGE);
        (key, (((ly % PAGE) * PAGE + lx % PAGE) * 3) as usize)
    }
    fn write_tile(&self, tag: TileTag, bytes: &[u8], pitch: u32, bgra: bool) -> Result<(), String> {
        let k = self.k as usize;
        let mut data = self.scratch.lock().map_err(|_| "Tile scratch poisoned")?;
        data.clear();
        data.resize(k * k * PAGE_BYTES, 0);
        data.par_chunks_mut(PAGE_BYTES).enumerate().for_each(|(index, page)| {
            let (px, py) = ((index % k) as u32 * PAGE, (index / k) as u32 * PAGE);
            for row in 0..PAGE.min(tag.h.saturating_sub(py)) {
                let cols = PAGE.min(tag.w.saturating_sub(px)) as usize;
                let src = &bytes[((py + row) * pitch + px * 4) as usize..][..cols * 4];
                let dst = &mut page[(row * PAGE * 3) as usize..][..cols * 3];
                for (d, s) in dst.chunks_exact_mut(3).zip(src.chunks_exact(4)) {
                    if bgra { d.copy_from_slice(&[s[2], s[1], s[0]]); } else { d.copy_from_slice(&s[..3]); }
                }
            }
        });
        write_at(&self.file, &data, self.tile_key(tag.face, tag.tx, tag.ty) * PAGE_BYTES as u64)?;
        self.written.fetch_add(data.len() as u64, Ordering::Relaxed);
        Ok(())
    }
    fn read_page(&self, key: u64, out: &mut [u8]) -> Result<(), String> {
        read_at(&self.file, out, key * PAGE_BYTES as u64)?;
        self.read.fetch_add(out.len() as u64, Ordering::Relaxed);
        Ok(())
    }
}

// Positional I/O: independent offsets, no shared seek cursor between threads.
#[cfg(windows)]
fn write_at(file: &File, mut data: &[u8], mut offset: u64) -> Result<(), String> {
    use std::os::windows::fs::FileExt;
    while !data.is_empty() {
        let n = file.seek_write(data, offset).map_err(io_err)?;
        if n == 0 { return Err(io_err("short write")); }
        data = &data[n..]; offset += n as u64;
    }
    Ok(())
}
#[cfg(windows)]
fn read_at(file: &File, mut out: &mut [u8], mut offset: u64) -> Result<(), String> {
    use std::os::windows::fs::FileExt;
    while !out.is_empty() {
        let n = file.seek_read(out, offset).map_err(io_err)?;
        if n == 0 { return Err(io_err("short read")); }
        out = &mut std::mem::take(&mut out)[n..]; offset += n as u64;
    }
    Ok(())
}
#[cfg(unix)]
fn write_at(file: &File, data: &[u8], offset: u64) -> Result<(), String> {
    std::os::unix::fs::FileExt::write_all_at(file, data, offset).map_err(io_err)
}
#[cfg(unix)]
fn read_at(file: &File, out: &mut [u8], offset: u64) -> Result<(), String> {
    std::os::unix::fs::FileExt::read_exact_at(file, out, offset).map_err(io_err)
}

#[cfg(windows)]
fn free_space(path: &std::path::Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available = 0u64;
    // SAFETY: NUL-terminated path; out pointer to a local u64.
    let ok = unsafe { windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(available)
}
#[cfg(not(windows))]
fn free_space(_: &std::path::Path) -> Option<u64> { None }

/// Shared bounded LRU of immutable pages. Workers keep a few `Arc` handles
/// locally, so the lock is taken per page transition, not per texel.
struct PageCache<'a> {
    store: &'a PageStore,
    lru: Mutex<Lru>,
    capacity: usize,
}
#[derive(Default)]
struct Lru { pages: HashMap<u64, (u64, Arc<[u8]>)>, order: BTreeMap<u64, u64>, tick: u64, hits: u64, misses: u64, evictions: u64 }
impl PageCache<'_> {
    fn get(&self, key: u64) -> Result<Arc<[u8]>, String> {
        {
            let mut guard = self.lru.lock().map_err(|_| "Page cache poisoned")?;
            let lru = &mut *guard;
            if let Some((stamp, page)) = lru.pages.get_mut(&key) {
                lru.order.remove(stamp);
                lru.tick += 1;
                *stamp = lru.tick;
                lru.order.insert(lru.tick, key);
                lru.hits += 1;
                return Ok(Arc::clone(page));
            }
        }
        let mut data = vec![0u8; PAGE_BYTES];
        self.store.read_page(key, &mut data)?;
        let page: Arc<[u8]> = data.into();
        let mut guard = self.lru.lock().map_err(|_| "Page cache poisoned")?;
        let lru = &mut *guard;
        lru.misses += 1;
        if let Some((_, existing)) = lru.pages.get(&key) { return Ok(Arc::clone(existing)); }
        lru.tick += 1;
        lru.pages.insert(key, (lru.tick, Arc::clone(&page)));
        lru.order.insert(lru.tick, key);
        while lru.pages.len() > self.capacity {
            let Some((_, old)) = lru.order.pop_first() else { break; };
            lru.pages.remove(&old);
            lru.evictions += 1;
        }
        Ok(page)
    }
}

struct Local<'c, 'a> { cache: &'c PageCache<'a>, slots: [(u64, Option<Arc<[u8]>>); 4], next: usize }
impl<'c, 'a> Local<'c, 'a> {
    fn new(cache: &'c PageCache<'a>) -> Self { Self { cache, slots: Default::default(), next: 0 } }
    fn texel(&mut self, face: u32, x: i32, y: i32) -> Result<[u8; 3], String> {
        let side = self.cache.store.side;
        // Across a cube edge remap the filter tap into the adjacent face, not
        // clamp it to a duplicate edge pixel. Same convention as photo_projection.wgsl.
        let (face, x, y) = if x < 0 || y < 0 || x >= side as i32 || y >= side as i32 {
            let p = 2.0 * (f64::from(x) + 0.5) / f64::from(side) - 1.0;
            let q = 2.0 * (f64::from(y) + 0.5) / f64::from(side) - 1.0;
            let (f, u, v) = direction_to_face(face_direction(face, p, q));
            (f, (u * f64::from(side)).floor().clamp(0.0, f64::from(side - 1)) as u32,
                (v * f64::from(side)).floor().clamp(0.0, f64::from(side - 1)) as u32)
        } else { (face, x as u32, y as u32) };
        let (key, offset) = self.cache.store.locate(face, x, y);
        let index = match self.slots.iter().position(|(k, page)| *k == key && page.is_some()) {
            Some(index) => index,
            None => {
                let index = self.next;
                self.next = (self.next + 1) % self.slots.len();
                self.slots[index] = (key, Some(self.cache.get(key)?));
                index
            }
        };
        let page = self.slots[index].1.as_ref().unwrap();
        Ok([page[offset], page[offset + 1], page[offset + 2]])
    }
    fn sample(&mut self, direction: [f64; 3], decode: &[f32; 256]) -> Result<[f32; 3], String> {
        let side = f64::from(self.cache.store.side);
        let (face, u, v) = direction_to_face(direction);
        let x = u * side - 0.5;
        let y = v * side - 0.5;
        let (ix, iy) = (x.floor() as i32, y.floor() as i32);
        let (a, b) = ((x - x.floor()) as f32, (y - y.floor()) as f32);
        let taps = [self.texel(face, ix, iy)?, self.texel(face, ix + 1, iy)?, self.texel(face, ix, iy + 1)?, self.texel(face, ix + 1, iy + 1)?];
        let weights = [(1.0 - a) * (1.0 - b), a * (1.0 - b), (1.0 - a) * b, a * b];
        Ok(std::array::from_fn(|c| (0..4).map(|i| weights[i] * decode[taps[i][c] as usize]).sum()))
    }
}

fn face_direction(face: u32, p: f64, q: f64) -> [f64; 3] {
    match face { 0 => [1., -q, -p], 1 => [-1., -q, p], 2 => [p, 1., q], 3 => [p, -1., -q], 4 => [p, -q, 1.], _ => [-p, -q, -1.] }
}
fn direction_to_face([x, y, z]: [f64; 3]) -> (u32, f64, f64) {
    let (a, b, c) = (x.abs(), y.abs(), z.abs());
    let (f, p, q) = if a >= b && a >= c { if x >= 0. { (0, -z / a, -y / a) } else { (1, z / a, -y / a) } }
        else if b >= c { if y >= 0. { (2, x / b, z / b) } else { (3, x / b, -z / b) } }
        else if z >= 0. { (4, x / c, -y / c) } else { (5, -x / c, -y / c) };
    (f, (p + 1.) * 0.5, (q + 1.) * 0.5)
}

/// Equirectangular bands projected in parallel (one Rayon task per row) and
/// handed to the encoder in order. Same sample placement and linear-light
/// bilinear/SSAA resolve as the GPU shader.
fn project(store: &PageStore, plan: &CapturePlan, request: &PhotoCaptureRequest, srgb: bool, sender: &mut stream::BandSender) -> Result<(u64, u64, u64), String> {
    let [width, height] = plan.image;
    let ss = request.options.supersample;
    let cache = PageCache { store, lru: Mutex::new(Lru::default()), capacity: (plan.cache_bytes / PAGE_BYTES as u64).max(64) as usize };
    let decode: [f32; 256] = std::array::from_fn(|i| { let v = i as f32 / 255.; if !srgb { v } else if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) } });
    let encode_channel = |v: f32| -> u8 { let v = v.clamp(0., 1.); let v = if !srgb { v } else if v <= 0.0031308 { 12.92 * v } else { 1.055 * v.powf(1. / 2.4) - 0.055 }; (v * 255.).round() as u8 };
    let cancel = &*request.cancel;
    let lon: Vec<(f64, f64)> = (0..width * ss).map(|i| (((f64::from(i) + 0.5) / f64::from(width * ss) - 0.5) * 2. * std::f64::consts::PI).sin_cos()).collect();
    for y0 in (0..height).step_by(BAND as usize) {
        check_cancel(cancel)?;
        let rows = BAND.min(height - y0);
        let lat: Vec<(f64, f64)> = (0..rows * ss).map(|i| ((0.5 - (f64::from(y0 * ss + i) + 0.5) / f64::from(height * ss)) * std::f64::consts::PI).sin_cos()).collect();
        let mut rgb = sender.buffer(rows);
        rgb.par_chunks_mut(width as usize * 3).enumerate().try_for_each(|(y, out)| -> Result<(), String> {
            check_cancel(cancel)?;
            let mut local = Local::new(&cache);
            for x in 0..width {
                let mut color = [0f32; 3];
                for sy in 0..ss { for sx in 0..ss {
                    let (sl, cl) = lon[(x * ss + sx) as usize];
                    let (sp, cp) = lat[(y as u32 * ss + sy) as usize];
                    let sample = local.sample([sl * cp, sp, cl * cp], &decode)?;
                    for c in 0..3 { color[c] += sample[c]; }
                }}
                let i = x as usize * 3;
                for c in 0..3 { out[i + c] = encode_channel(color[c] / (ss * ss) as f32); }
            }
            Ok(())
        })?;
        sender.send(stream::Band { rows, rgb }).map_err(|_| stream::ENCODER_STOPPED.to_owned())?;
    }
    let lru = cache.lru.lock().map_err(|_| "Page cache poisoned")?;
    Ok((lru.hits, lru.misses, lru.evictions))
}

/// Disk plan: sweep into pages, then project + encode concurrently.
pub(super) fn capture_disk(
    source: &mut Renderer, request: &PhotoCaptureRequest, camera: Camera, faces: [(f32, f32, &str); 6],
    plan: &CapturePlan, scene: &CaptureSceneState, render: &impl Fn(&mut Renderer, &Camera) -> Result<(), String>,
    proxy: &EventLoopProxy<UserEvent>, perf: &mut PhotoPerf, watch: &mut GpuWatch,
) -> Result<ScreenshotOutput, String> {
    fs::create_dir_all(&request.directory).map_err(io_err)?;
    let (stem, extension) = super::strips::stem(request, "equirectangular");
    let scratch = Scratch(request.directory.join(format!(".{stem}.tiles")));
    fs::create_dir(&scratch.0).map_err(io_err)?;
    // Pages plus a raw-size bound on the output (compressed is smaller).
    let needed = plan.disk_bytes + u64::from(plan.image[0]) * u64::from(plan.image[1]) * 3;
    if let Some(free) = free_space(&scratch.0) {
        if free < needed {
            return Err(format!("Panorama needs ~{:.1} GB free disk space for temporary tiles and output ({:.1} GB free)", needed as f64 / 1e9, free as f64 / 1e9));
        }
    }
    let store = PageStore::create(&scratch.0.join("cube.pages"), plan)?;
    progress(proxy, format!("PHOTO: {}", plan.describe()), 0, u64::from(plan.world_renders()), Instant::now());
    sweep(source, request, camera, faces, plan, scene, render, Target::Disk(&store), proxy, perf, watch)?;
    watch.check()?;
    check_cancel(&request.cancel)?;
    let srgb = source.offscreen_output.as_ref().is_some_and(|t| t.format().is_srgb());
    let [width, height] = plan.image;
    let part = scratch.0.join(format!("{stem}.{extension}.part"));
    let job = stream::EncodeJob { path: part.clone(), jpeg: extension == "jpg", width, height,
        xmp: crate::screenshot::panorama_xmp(&request.metadata, width, height)?,
        cancel: Arc::clone(&request.cancel), proxy: proxy.clone() };
    let started = Instant::now();
    let mut cache_stats = (0, 0, 0);
    let (stats, send_wait) = stream::run(job, |sender| {
        cache_stats = project(&store, plan, request, srgb, sender)?;
        Ok(())
    })?;
    perf.projection += started.elapsed();
    perf.cache = cache_stats;
    perf.store_written = store.written.load(Ordering::Relaxed);
    perf.store_read = store.read.load(Ordering::Relaxed);
    perf.add_encode(stats, send_wait);
    drop(store);
    super::strips::finish_part(part, request.directory.join(format!("{stem}.{extension}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn face_roundtrip() {
        for face in 0..6 { for p in [-0.8,0.,0.8] { for q in [-0.8,0.,0.8] {
            let (f,u,v)=direction_to_face(face_direction(face,p,q));
            assert_eq!(f,face); assert!((u-(p+1.)*0.5).abs()<1e-12); assert!((v-(q+1.)*0.5).abs()<1e-12);
        }}}
    }
    #[test] fn disk_offsets_exceed_u32_without_wrap() {
        // 131072-wide plan: last page offset is far beyond u32 and exact.
        let n = 14u64; let k = 12u64;
        let last = ((5 * n + n - 1) * n + n - 1) * k * k + k * k - 1;
        assert!(last * PAGE_BYTES as u64 > u64::from(u32::MAX));
        assert_eq!(last * PAGE_BYTES as u64 / PAGE_BYTES as u64, last);
    }
}
