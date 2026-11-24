//! Blue Water's depth/fetch/surf model. Separate data, worker and controls from the JKA coastal solver.
use super::OCEAN_UNITS_PER_METER;
use blue_water_gpu::shore::ShoreMap;
use std::sync::{Arc, Mutex};
type Footprints = Vec<(
    u32,
    f32,
    Vec<jka_assets::bsp::bathymetry::FootprintTriangle>,
)>;
pub type Maps = Vec<(usize, Arc<ShoreMap>)>;
struct Terrain {
    slot: usize,
    heights: Vec<f32>,
    columns: usize,
    rows: usize,
    cell: f64,
    origin: [f64; 2],
}
#[derive(Default)]
struct State {
    requested: u32,
    working: bool,
    failed: bool,
    terrain: Option<Arc<Vec<Terrain>>>,
    ready: Option<(u32, Arc<Maps>)>,
}
pub struct Source {
    bsp: Arc<jka_assets::bsp::Bsp>,
    footprints: Footprints,
    state: Mutex<State>,
    /// Fluid Ninja Coast bed patches (`super::ninja_coast`), built on request.
    ninja: Mutex<NinjaPatchState>,
    /// Built once, by the first bed tile the disk cache cannot supply.
    ninja_geometry: std::sync::OnceLock<Result<Arc<jka_assets::bsp::bathymetry::Geometry>, String>>,
    /// Per-map bed tile files under `cache_directory`.
    ninja_disk: Mutex<NinjaDisk>,
    cache_directory: Option<std::path::PathBuf>,
}
/// Fluid Ninja Coast bed samples are cached in world-anchored square tiles of
/// `NINJA_TILE` cells; a cell is `cell` map units and tile `t` covers cells
/// `t*NINJA_TILE ..`. Values come from `Geometry::upstream_shore_heights` at
/// cell centres, relative to the pool level, NaN for unknown columns.
pub const NINJA_TILE: i32 = 64;
type TileKey = (usize, u32, [i32; 2]);
#[derive(Default)]
struct NinjaPatchState {
    working: bool,
    failed: bool,
    tiles: std::collections::HashMap<TileKey, Arc<Vec<f32>>>,
    /// Missing tiles, nearest to the latest window first.
    wanted: Vec<TileKey>,
    /// Tiles a worker thread is reading or building now.
    in_flight: std::collections::HashSet<TileKey>,
}
/// Bump when `Geometry::upstream_shore_heights` (beach raster) output changes.
/// The containing `cache/coastal/v*-<map>-<footprints>` directory already keys
/// the BSP, the promoted pool footprints and their levels; the file name keys
/// the pool and cell size, the header the tile size and level.
const NINJA_BED_CACHE_VERSION: u32 = 1;
const NINJA_DISK_MAGIC: u32 = u32::from_le_bytes(*b"NJBD");
const NINJA_DISK_RECORD: u64 = 8 + (NINJA_TILE * NINJA_TILE) as u64 * 4;
#[derive(Default)]
struct NinjaDisk {
    files: std::collections::HashMap<(usize, u32), NinjaDiskFile>,
    failed: std::collections::HashSet<(usize, u32)>,
}
struct NinjaDiskFile {
    file: std::fs::File,
    index: std::collections::HashMap<[i32; 2], u64>,
    end: u64,
}
/// One assembled window: `heights[j * size + i]` is cell `origin + (i, j)`.
pub struct CoastPatch {
    pub level: f32,
    pub heights: Vec<f32>,
}
impl Source {
    pub fn report(&self, at: [f64; 2], settings: &super::OceanSettings) -> Vec<String> {
        let mut lines = vec![format!(
            "BLUE WATER COAST: enabled={} shelter={} backend={} | Hs {:.2}m, wavelength {:.2}m, wind {:.2}m/s, direction {:.1}deg | eye XZ {:.1},{:.1}m",
            settings.blue_water_coast, settings.blue_water_coast_shelter, settings.backend,
            settings.blue_water_waves.height, settings.blue_water_waves.wavelength,
            settings.blue_water_waves.wind_speed, settings.blue_water_waves.direction, at[0], at[1])];
        lines.push(self.status().into());
        let ready = self.state.try_lock().ok().and_then(|s| s.ready.clone());
        let Some((direction, maps)) = ready else {
            return lines;
        };
        let params = settings.blue_water_waves.parameters();
        let sea = blue_water_core::sea_state::drawn_sea(&params, false);
        let cascades = blue_water_core::sea_state::sea_state_cascades(
            &blue_water_core::QualityTier::High.cascades(),
            sea.peak_wavelength,
        );
        let spectrum = blue_water_core::spectrum::build_spectrum(&cascades, &sea);
        let mut stats = blue_water_gpu::surface_waves::SpectrumStats::from_spectrum(&spectrum);
        stats.set_shore_tables(&spectrum, sea.wind_speed);
        let swell = super::hybrid_swell_uniform(settings, 0.0);
        for (slot, map) in maps.iter() {
            let depth_only;
            let map = if settings.blue_water_coast_shelter {
                map.as_ref()
            } else {
                depth_only = blue_water_gpu::surface_waves::coast_depth_only(map);
                &depth_only
            };
            let sample = stats.sample_shore(
                map,
                at,
                swell[1][2],
                4.0 * swell[0][3].abs() / std::f32::consts::SQRT_2,
            );
            lines.push(format!("Pool {}: map direction {:.1}deg; depth {}; fetch wind {:.1}m / swell {:.1}m; arriving Hs {:.4}m (break depth {:.2}m); kept FFT {:.4}% / {:.4}% / {:.4}%, analytic {:.4}%",
                slot+1, f32::from_bits(direction).to_degrees(),
                sample.depth.map_or("unknown or outside height map (deep/open fallback)".into(), |d| format!("{d:.3}m")),
                sample.wind_fetch, sample.swell_fetch, sample.arriving, sample.arriving / blue_water_gpu::shore::BREAKING,
                sample.scales[0]*100.0, sample.scales[1]*100.0, sample.scales[2]*100.0, sample.scales[3]*100.0));
            if sample.scales[..3].iter().all(|s| *s < 0.005) {
                lines.push("Almost all FFT motion is attenuated here. Try r_blueWaterCoastShelter 0 to isolate shelter, or r_blueWaterCoast 0 to bypass the coast.".into());
            }
        }
        lines.push("CPU estimate at the eye's horizontal position; no GPU readback. Open water outside the coastal map retains the open-ocean field. Source-raster unknowns use deep/open fallback.".into());
        lines
    }
    pub fn status(&self) -> &'static str {
        let Ok(state) = self.state.try_lock() else {
            return "Preparing Blue Water shores...";
        };
        if state.failed {
            "Blue Water shores unavailable; using open-ocean waves."
        } else if state.working {
            "Preparing Blue Water shores..."
        } else if state.ready.is_some() {
            "Blue Water shores ready."
        } else {
            "Blue Water shores will prepare when the ocean is active."
        }
    }
    pub fn new(bsp: Arc<jka_assets::bsp::Bsp>, footprints: Footprints, cache_directory: Option<std::path::PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            bsp,
            footprints,
            state: Mutex::new(State::default()),
            ninja: Mutex::new(NinjaPatchState::default()),
            ninja_geometry: std::sync::OnceLock::new(),
            ninja_disk: Mutex::new(NinjaDisk::default()),
            cache_directory,
        })
    }
    fn ninja_level(&self, slot: usize, plane: f32) -> f32 {
        self.footprints.iter().find(|(s, _, _)| *s as usize == slot).map_or(plane, |(_, level, _)| *level)
    }
    /// Nonblocking render-thread request for the Fluid Ninja Coast bed window of
    /// `size`^2 cells starting at cell `origin`. Missing tiles (plus a one-tile
    /// margin) are queued for the worker; `None` until the window's tiles exist.
    pub fn ninja_bed_window(self: &Arc<Self>, slot: usize, plane: f32, cell: f32, size: usize, origin: [i32; 2])
        -> Option<CoastPatch> {
        let mut state = self.ninja.try_lock().ok()?;
        if state.failed {
            return None;
        }
        let tile_of = |c: i32| c.div_euclid(NINJA_TILE);
        let lo = origin.map(tile_of);
        let hi = origin.map(|o| tile_of(o + size as i32 - 1));
        let centre = [(lo[0] + hi[0]) as f32 * 0.5, (lo[1] + hi[1]) as f32 * 0.5];
        let mut missing = Vec::new();
        let mut complete = true;
        for ty in lo[1] - 1..=hi[1] + 1 {
            for tx in lo[0] - 1..=hi[0] + 1 {
                let key = (slot, cell.to_bits(), [tx, ty]);
                if !state.tiles.contains_key(&key) {
                    missing.push(key);
                    complete &= tx < lo[0] || tx > hi[0] || ty < lo[1] || ty > hi[1];
                }
            }
        }
        let distance = |k: &TileKey| (k.2[0] as f32 - centre[0]).abs().max((k.2[1] as f32 - centre[1]).abs());
        missing.sort_by(|a, b| distance(a).total_cmp(&distance(b)));
        state.wanted = missing;
        // Bound the cache: forget tiles far from this window.
        if state.tiles.len() > 1024 {
            state.tiles.retain(|k, _| k.0 != slot || distance(k) <= 24.0);
        }
        if !state.wanted.is_empty() && !state.working {
            state.working = true;
            let source = Arc::clone(self);
            if let Err(e) = std::thread::Builder::new()
                .name("ninja-coast-bed".into())
                .spawn(move || source.ninja_work())
            {
                state.working = false;
                state.failed = true;
                eprintln!("NINJA COAST: bed worker failed: {e}");
            }
        }
        if !complete {
            return None;
        }
        let n = size;
        let mut heights = vec![f32::NAN; n * n];
        for j in 0..n {
            let cy = origin[1] + j as i32;
            for i in 0..n {
                let cx = origin[0] + i as i32;
                let tile = &state.tiles[&(slot, cell.to_bits(), [tile_of(cx), tile_of(cy)])];
                let (lx, ly) = (cx.rem_euclid(NINJA_TILE) as usize, cy.rem_euclid(NINJA_TILE) as usize);
                heights[j * n + i] = tile[ly * NINJA_TILE as usize + lx];
            }
        }
        Some(CoastPatch { level: self.ninja_level(slot, plane), heights })
    }
    /// Bed worker: a few scoped threads drain `wanted` nearest-first. Each tile
    /// is read from the per-map disk cache when present, else rasterized and
    /// appended to it.
    fn ninja_work(self: Arc<Self>) {
        let started = std::time::Instant::now();
        let built = std::sync::atomic::AtomicUsize::new(0);
        let loaded = std::sync::atomic::AtomicUsize::new(0);
        let threads = std::thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(1)).clamp(1, 8);
        let failure = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..threads).map(|_| scope.spawn(|| {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.ninja_drain(&built, &loaded)))
            })).collect();
            handles.into_iter().map(|h| match h.join() {
                Ok(Ok(Ok(()))) => None,
                Ok(Ok(Err(e))) => Some(e),
                _ => Some("worker panicked".to_owned()),
            }).fold(None, |a: Option<String>, b| a.or(b))
        });
        let Ok(mut state) = self.ninja.lock() else { return };
        state.working = false;
        state.in_flight.clear();
        if let Some(e) = failure {
            eprintln!("NINJA COAST: bed unavailable; pools keep authored water: {e}");
            state.failed = true;
            return;
        }
        let (built, loaded) = (built.into_inner(), loaded.into_inner());
        if built + loaded > 0 {
            eprintln!("NINJA COAST: {built} bed tiles built, {loaded} from cache, {threads} threads, {:.0} ms ({} cached)",
                started.elapsed().as_secs_f64() * 1000.0, state.tiles.len());
        }
    }
    fn ninja_drain(&self, built: &std::sync::atomic::AtomicUsize, loaded: &std::sync::atomic::AtomicUsize) -> Result<(), String> {
        use std::sync::atomic::Ordering::Relaxed;
        loop {
            let key = {
                let mut state = self.ninja.lock().map_err(|_| "ninja bed state poisoned")?;
                let Some(at) = state.wanted.iter().position(|k| !state.tiles.contains_key(k) && !state.in_flight.contains(k)) else {
                    return Ok(());
                };
                let key = state.wanted.remove(at);
                state.in_flight.insert(key);
                key
            };
            let (slot, cell, tile) = (key.0, f32::from_bits(key.1), key.2);
            let heights = match self.footprints.iter().find(|(s, _, _)| *s as usize == slot) {
                // A pool without a promoted footprint has no bed: every column stays unknown.
                None => vec![f32::NAN; (NINJA_TILE * NINJA_TILE) as usize],
                Some((_, level, triangles)) => match self.ninja_disk_read(key, *level) {
                    Some(h) => { loaded.fetch_add(1, Relaxed); h }
                    None => {
                        let geometry = self.ninja_geometry.get_or_init(|| {
                            jka_assets::bsp::bathymetry::Geometry::from_bsp(&self.bsp).map(Arc::new)
                        }).clone()?;
                        let n = NINJA_TILE as usize;
                        let origin = tile.map(|t| (t * NINJA_TILE) as f32 * cell + cell * 0.5);
                        let h = geometry.upstream_shore_heights(origin, [n; 2], cell, *level, triangles, true)?;
                        self.ninja_disk_write(key, *level, &h);
                        built.fetch_add(1, Relaxed);
                        h
                    }
                },
            };
            let mut state = self.ninja.lock().map_err(|_| "ninja bed state poisoned")?;
            state.in_flight.remove(&key);
            state.tiles.insert(key, Arc::new(heights));
        }
    }
    /// The per-(pool, cell) tile file, opened (or reset when its header does not
    /// match) on first use. Records are append-only `[tx, ty, heights]`; a torn
    /// trailing record is truncated.
    fn ninja_disk_file<'a>(&self, disk: &'a mut NinjaDisk, slot: usize, cell: u32, level: f32) -> Option<&'a mut NinjaDiskFile> {
        use std::io::{Read, Seek, SeekFrom, Write};
        let directory = self.cache_directory.as_ref()?;
        if !disk.files.contains_key(&(slot, cell)) {
            if disk.failed.contains(&(slot, cell)) {
                return None;
            }
            let header: Vec<u8> = [NINJA_DISK_MAGIC, NINJA_BED_CACHE_VERSION, NINJA_TILE as u32, slot as u32, cell, level.to_bits()]
                .iter().flat_map(|v| v.to_le_bytes()).collect();
            let path = directory.join(format!("ninja-bed-v{NINJA_BED_CACHE_VERSION}-pool{slot}-cell{cell:08x}.bin"));
            let opened = (|| -> std::io::Result<NinjaDiskFile> {
                std::fs::create_dir_all(directory)?;
                let mut file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path)?;
                let len = file.metadata()?.len();
                let mut existing = vec![0u8; header.len()];
                let valid = len >= header.len() as u64 && file.read_exact(&mut existing).is_ok() && existing == header;
                let mut index = std::collections::HashMap::new();
                let mut end = header.len() as u64;
                if valid {
                    let mut key = [0u8; 8];
                    while end + NINJA_DISK_RECORD <= len {
                        file.seek(SeekFrom::Start(end))?;
                        file.read_exact(&mut key)?;
                        let tx = i32::from_le_bytes([key[0], key[1], key[2], key[3]]);
                        let ty = i32::from_le_bytes([key[4], key[5], key[6], key[7]]);
                        index.insert([tx, ty], end);
                        end += NINJA_DISK_RECORD;
                    }
                    if end != len {
                        file.set_len(end)?;
                    }
                } else {
                    file.set_len(0)?;
                    file.seek(SeekFrom::Start(0))?;
                    file.write_all(&header)?;
                }
                Ok(NinjaDiskFile { file, index, end })
            })();
            match opened {
                Ok(f) => { disk.files.insert((slot, cell), f); }
                Err(e) => {
                    eprintln!("NINJA COAST: bed cache {} unavailable: {e}", path.display());
                    disk.failed.insert((slot, cell));
                    return None;
                }
            }
        }
        disk.files.get_mut(&(slot, cell))
    }
    fn ninja_disk_read(&self, key: TileKey, level: f32) -> Option<Vec<f32>> {
        use std::io::{Read, Seek, SeekFrom};
        let mut disk = self.ninja_disk.lock().ok()?;
        let entry = self.ninja_disk_file(&mut disk, key.0, key.1, level)?;
        let offset = *entry.index.get(&key.2)?;
        let mut bytes = vec![0u8; NINJA_DISK_RECORD as usize];
        entry.file.seek(SeekFrom::Start(offset)).ok()?;
        entry.file.read_exact(&mut bytes).ok()?;
        let word = |i: usize| [bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]];
        if [i32::from_le_bytes(word(0)), i32::from_le_bytes(word(4))] != key.2 {
            return None;
        }
        Some((8..bytes.len()).step_by(4).map(|i| f32::from_le_bytes(word(i))).collect())
    }
    fn ninja_disk_write(&self, key: TileKey, level: f32, heights: &[f32]) {
        use std::io::{Seek, SeekFrom, Write};
        let Ok(mut disk) = self.ninja_disk.lock() else { return };
        let Some(entry) = self.ninja_disk_file(&mut disk, key.0, key.1, level) else { return };
        if entry.index.contains_key(&key.2) || heights.len() != (NINJA_TILE * NINJA_TILE) as usize {
            return;
        }
        let mut record = Vec::with_capacity(NINJA_DISK_RECORD as usize);
        record.extend(key.2[0].to_le_bytes());
        record.extend(key.2[1].to_le_bytes());
        for h in heights {
            record.extend(h.to_le_bytes());
        }
        match entry.file.seek(SeekFrom::Start(entry.end)).and_then(|_| entry.file.write_all(&record)) {
            Ok(()) => {
                entry.index.insert(key.2, entry.end);
                entry.end += NINJA_DISK_RECORD;
            }
            Err(e) => eprintln!("NINJA COAST: bed cache write failed: {e}"),
        }
    }
    /// Nonblocking render-thread request. A changed wind replaces stale exposure; the last request wins.
    pub fn request(self: &Arc<Self>, direction: f32) -> Option<Arc<Maps>> {
        let key = direction.to_bits();
        let mut state = self.state.try_lock().ok()?;
        state.requested = key;
        if let Some((ready, maps)) = &state.ready {
            if *ready == key {
                return Some(Arc::clone(maps));
            }
        }
        if state.working || state.failed {
            return None;
        }
        state.working = true;
        let source = Arc::clone(self);
        if let Err(e) = std::thread::Builder::new()
            .name("blue-water-shore".into())
            .spawn(move || source.work())
        {
            state.working = false;
            state.failed = true;
            eprintln!("BLUE WATER COAST: worker failed: {e}");
        }
        None
    }
    fn terrain(&self) -> Result<Vec<Terrain>, String> {
        let geometry = jka_assets::bsp::bathymetry::Geometry::from_bsp(&self.bsp)?;
        let mut result = Vec::new();
        for (slot, level, triangles) in &self.footprints {
            if triangles.is_empty() {
                continue;
            }
            let mut low = [f32::INFINITY; 2];
            let mut high = [f32::NEG_INFINITY; 2];
            for p in triangles.iter().flatten() {
                for a in 0..2 {
                    low[a] = low[a].min(p[a]);
                    high[a] = high[a].max(p[a]);
                }
            }
            // Include real banks/islands outside the water outline for wind shelter. Texture edge means open sea.
            let halo = 128.0 * OCEAN_UNITS_PER_METER;
            for a in 0..2 {
                low[a] -= halo;
                high[a] += halo;
            }
            let cell = ((high[0] - low[0]).max(high[1] - low[1]) / 512.0).max(64.0);
            let size =
                [0, 1].map(|a| (((high[a] - low[a]) / cell).ceil() as usize + 1).clamp(2, 513));
            // `beach`: sand over a water brush that extends under it is land, not an unknown
            // column. Unknown columns keep full waves, which raised a wave film over dry beach
            // (costadelsol mode 4 outside the Coast window, 2026-11-20).
            let mut heights =
                geometry.upstream_shore_heights(low, size, cell, *level, triangles, true)?;
            let known = heights.iter().filter(|h| h.is_finite()).count();
            // Preserve NaNs until upload so the filter can exclude unknown columns.
            for h in &mut heights {
                if h.is_finite() {
                    *h = (*h / OCEAN_UNITS_PER_METER).clamp(-10000.0, 10000.0);
                }
            }
            // JKA +Y maps to renderer -Z.
            let mut flipped = Vec::with_capacity(heights.len());
            for row in heights.chunks_exact(size[0]).rev() {
                flipped.extend_from_slice(row);
            }
            eprintln!(
                "BLUE WATER COAST: pool {} raster {}x{}, {:.2}m/cell, {known}/{} known columns",
                slot + 1,
                size[0],
                size[1],
                cell / OCEAN_UNITS_PER_METER,
                heights.len()
            );
            result.push(Terrain {
                slot: *slot as usize,
                heights: flipped,
                columns: size[0],
                rows: size[1],
                cell: f64::from(cell / OCEAN_UNITS_PER_METER),
                origin: [
                    f64::from(low[0] / OCEAN_UNITS_PER_METER),
                    f64::from(-(low[1] + (size[1] - 1) as f32 * cell) / OCEAN_UNITS_PER_METER),
                ],
            });
        }
        Ok(result)
    }
    fn work(self: Arc<Self>) {
        let work =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), String> {
                let terrain = self
                    .state
                    .lock()
                    .map_err(|_| "shore state poisoned")?
                    .terrain
                    .clone();
                let terrain = match terrain {
                    Some(t) => t,
                    None => Arc::new(self.terrain()?),
                };
                self.state
                    .lock()
                    .map_err(|_| "shore state poisoned")?
                    .terrain = Some(Arc::clone(&terrain));
                loop {
                    let key = self
                        .state
                        .lock()
                        .map_err(|_| "shore state poisoned")?
                        .requested;
                    let maps: Maps = terrain
                        .iter()
                        .map(|t| {
                            (
                                t.slot,
                                Arc::new(ShoreMap::build(
                                    t.heights.clone(),
                                    t.columns,
                                    t.rows,
                                    t.cell,
                                    t.origin,
                                    f64::from(f32::from_bits(key)),
                                )),
                            )
                        })
                        .collect();
                    let mut state = self.state.lock().map_err(|_| "shore state poisoned")?;
                    if state.requested != key {
                        continue;
                    }
                    eprintln!(
                        "BLUE WATER COAST: {} pool maps ready; wave direction {:.1} degrees",
                        maps.len(),
                        f32::from_bits(key).to_degrees()
                    );
                    state.ready = Some((key, Arc::new(maps)));
                    state.working = false;
                    return Ok(());
                }
            }));
        if !matches!(work, Ok(Ok(()))) {
            eprintln!(
                "BLUE WATER COAST: terrain unavailable; retaining open-water waves: {}",
                match work {
                    Ok(Err(e)) => e,
                    _ => "worker panicked".into(),
                }
            );
            if let Ok(mut state) = self.state.lock() {
                state.working = false;
                state.failed = true;
            }
        }
    }
}
