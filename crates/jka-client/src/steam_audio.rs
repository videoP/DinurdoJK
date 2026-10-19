use audionimbus::{
    BakedDataIdentifier, BakedDataVariation, Context, DefaultRayTracer, Material, Matrix4,
    PathBakeParams, PathBaker, Point, ProbeArray, ProbeBatch, ProbeGenerationParams,
    ProgressCallback, ReflectionsBakeFlags, ReflectionsBakeParams, ReflectionsBaker, Scene,
    SerializedObject, StaticMesh, StaticMeshSettings, Triangle,
};
use jka_assets::bsp::AcousticMesh;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

/// Steam Audio works in meters. JKA does not define a canonical physical meter,
/// so keep the conversion explicit, versioned, and easy to tune. 32 units/m puts
/// the common 56-unit JKA player height at roughly 1.75 m.
pub const JKA_UNITS_PER_METER: f32 = 32.0;

const CACHE_SCHEMA_VERSION: u32 = 1;
const BAKE_PROFILE_VERSION: u32 = 1;
const PROBE_SPACING_M: f32 = 3.0;
const PROBE_HEIGHT_M: f32 = 1.5;
const PROBE_BOUNDS_PADDING_M: f32 = 1.0;
const MAX_PROBES: usize = 65_536;
/// Process private-memory growth allowed while a bake runs. Huge race maps
/// (racepack4: 32k probes) grew the process to 17 GB, mostly after the
/// reflections stage. Past this the bake is cancelled and the map is marked so
/// later loads skip it; direct acoustics stay active either way.
/// `JKA_STEAM_AUDIO_BAKE_BUDGET_MIB` overrides it (testing, big-RAM machines).
const DEFAULT_BAKE_MEMORY_BUDGET_MIB: u64 = 2048;
const BAKE_MEMORY_POLL: Duration = Duration::from_millis(100);
/// The bake is queued while the map's own upload is still allocating (racepack4:
/// 1.2 -> 5.4 GB in the same seconds), so growth is measured from the first
/// second-long window that grows less than this, or from the timeout below.
const BAKE_MEMORY_SETTLE_WINDOW: Duration = Duration::from_secs(1);
const BAKE_MEMORY_SETTLE_BYTES: u64 = 32 * 1024 * 1024;
const BAKE_MEMORY_SETTLE_TIMEOUT: Duration = Duration::from_secs(20);
/// Bumped when the map that queued a bake is no longer current; its result is
/// discarded anyway, so the running bake is cancelled without a marker.
static BAKE_CANCEL_GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn cancel_background_bake() {
    BAKE_CANCEL_GENERATION.fetch_add(1, Ordering::AcqRel);
}
const OVER_BUDGET_MARKER: &str = "over_budget.json";

fn bake_memory_budget_mib() -> u64 {
    std::env::var("JKA_STEAM_AUDIO_BAKE_BUDGET_MIB")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(DEFAULT_BAKE_MEMORY_BUDGET_MIB)
}

// Initial offline-quality profile. The expensive work is cached per BSP hash.
const REFLECTION_RAYS: u32 = 4_096;
const REFLECTION_DIFFUSE_SAMPLES: u32 = 128;
const REFLECTION_BOUNCES: u32 = 32;
const REFLECTION_SIMULATED_DURATION_S: f32 = 2.0;
const REFLECTION_SAVED_DURATION_S: f32 = 0.75;
const REFLECTION_AMBISONIC_ORDER: u32 = 1;
const REFLECTION_IRRADIANCE_MIN_DISTANCE_M: f32 = 1.0;
const REFLECTION_BAKE_BATCH_SIZE: u32 = 16;

// Valve/AudioNimbus' pathing example uses this basic dynamic-path profile.
const PATH_NUM_SAMPLES: u32 = 1;
const PATH_RADIUS_M: f32 = 1.0;
const PATH_VISIBILITY_THRESHOLD: f32 = 0.5;
const PATH_VISIBILITY_RANGE_M: f32 = 50.0;
const PATH_RANGE_M: f32 = 100.0;

pub type SteamAudioBakeProgress = Arc<dyn Fn(f32) + Send + Sync + 'static>;

#[derive(Debug, Clone)]
pub struct SteamAudioBakeCacheInfo {
    pub directory: PathBuf,
    pub map_name: String,
    pub map_hash: u64,
}

#[derive(Debug, Clone)]
pub struct SteamAudioBakeRequest {
    pub mesh: Arc<AcousticMesh>,
    pub cache: SteamAudioBakeCacheInfo,
    pub num_threads: usize,
}

#[derive(Debug, Clone)]
pub struct SteamAudioBakeData {
    pub scene_bytes: Arc<[u8]>,
    pub probe_batch_bytes: Arc<[u8]>,
    pub probe_count: usize,
    pub reflection_data_bytes: usize,
    pub pathing_data_bytes: usize,
    pub cache_hit: bool,
    /// The serialized scene and probe batch were deserialized through Steam
    /// Audio successfully after loading/baking. This is deliberately stronger
    /// than merely checking that the cache files exist.
    pub runtime_validated: bool,
}

impl SteamAudioBakeData {
    pub fn serialized_bytes(&self) -> usize {
        self.scene_bytes.len() + self.probe_batch_bytes.len()
    }
}

pub fn cache_info(
    root: &Path,
    game: Option<&Path>,
    map_name: &str,
    map_hash: u64,
) -> SteamAudioBakeCacheInfo {
    let safe_name = sanitize_map_name(map_name);
    SteamAudioBakeCacheInfo {
        directory: game
            .unwrap_or(root)
            .join("jka-rust-cache")
            .join("steam-audio")
            .join(format!(
                "{safe_name}-{map_hash:016x}-v{CACHE_SCHEMA_VERSION}-p{BAKE_PROFILE_VERSION}"
            )),
        map_name: map_name.to_owned(),
        map_hash,
    }
}

/// Fast path used during map preparation. It only reads and validates an
/// existing cache; it never starts an expensive bake. First-time bakes are
/// deliberately scheduled after CPU map preparation so the prepared-map handoff is not
/// held hostage by an offline-quality reflections/pathing job.
pub fn load_cached_bake(
    cache: &SteamAudioBakeCacheInfo,
) -> Result<Option<SteamAudioBakeData>, String> {
    let Some(mut cached) = load_cache(cache)? else {
        return Ok(None);
    };
    let validation = validate_serialized_bake(&cached)?;
    cached.runtime_validated = true;
    devprintln!(
        1,
        "{}: Steam Audio bake cache HIT: {} probes, {:.2} MiB serialized; runtime validation OK (reflections {:.2} MiB, pathing {:.2} MiB)",
        cache.map_name,
        validation.probe_count,
        cached.serialized_bytes() as f64 / (1024.0 * 1024.0),
        validation.reflection_data_bytes as f64 / (1024.0 * 1024.0),
        validation.pathing_data_bytes as f64 / (1024.0 * 1024.0),
    );
    Ok(Some(cached))
}

pub fn load_or_bake(
    mesh: &AcousticMesh,
    cache: &SteamAudioBakeCacheInfo,
    num_threads: usize,
    progress: Option<SteamAudioBakeProgress>,
) -> Result<SteamAudioBakeData, String> {
    match load_cached_bake(cache) {
        Ok(Some(cached)) => {
            if let Some(progress) = &progress {
                progress(1.0);
            }
            return Ok(cached);
        }
        Ok(None) => {
            devprintln!(
                1,
                "{}: Steam Audio bake cache MISS: preparing scene/probes",
                cache.map_name
            );
        }
        Err(error) => {
            // A damaged/stale cache must never make a map unloadable. The
            // background baker will reconstruct it from BSP geometry.
            eprintln!("{}: Steam Audio cache ignored: {error}", cache.map_name);
        }
    }
    if let Some(reason) = over_budget_skip(cache) {
        return Err(reason);
    }

    let threads = num_threads.clamp(1, 8) as u32;
    let started = Instant::now();
    let cancel_generation = BAKE_CANCEL_GENERATION.load(Ordering::Acquire);
    let context = Context::default();
    let scene = build_scene_from_acoustic_mesh(&context, mesh)?;
    let vertices = mesh
        .vertices
        .iter()
        .copied()
        .map(jka_to_steam_point)
        .collect::<Vec<_>>();
    let (min, max) = point_bounds(&vertices)?;
    let transform = probe_volume_transform(min, max);
    let mut probe_array = ProbeArray::try_new(&context)
        .map_err(|error| format!("could not create Steam Audio probe array: {error}"))?;
    probe_array.generate_probes(
        &scene,
        &ProbeGenerationParams::UniformFloor {
            spacing: PROBE_SPACING_M,
            height: PROBE_HEIGHT_M,
            transform,
        },
    );
    let probe_count = probe_array.num_probes();
    if probe_count == 0 {
        return Err("Steam Audio generated zero floor probes for this BSP".into());
    }
    if probe_count > MAX_PROBES {
        return Err(format!(
            "Steam Audio generated {probe_count} probes, over the safety limit of {MAX_PROBES}"
        ));
    }

    let mut probe_batch = ProbeBatch::try_new(&context)
        .map_err(|error| format!("could not create Steam Audio probe batch: {error}"))?;
    probe_batch.add_probe_array(&probe_array);
    probe_batch.commit();

    devprintln!(
        1,
        "{}: Steam Audio probes ready: {} floor probes @ {:.1} m spacing / {:.1} m height; {} bake thread(s)",
        cache.map_name, probe_count, PROBE_SPACING_M, PROBE_HEIGHT_M, threads,
    );

    // A cancelled Steam Audio bake returns normally, so the watchdog's flag is
    // what tells a budget cancel apart from a finished stage.
    let watchdog = BakeMemoryWatchdog::default();
    let bake_result = std::thread::scope(|scope| {
        let (watch_context, watchdog) = (context.clone(), &watchdog);
        let spawned = std::thread::Builder::new()
            .name("jka-steam-audio-memory".into())
            .spawn_scoped(scope, move || watchdog.run(&watch_context, cancel_generation));
        if let Err(error) = spawned {
            eprintln!("{}: Steam Audio bake memory watchdog unavailable: {error}", cache.map_name);
        }
        let result = (|| -> Result<(), String> {
            let reverb_id = reverb_identifier();
            devprintln!(
                1,
                "{}: Steam Audio reflections bake starting ({} rays, {} diffuse samples, {} bounces)",
                cache.map_name, REFLECTION_RAYS, REFLECTION_DIFFUSE_SAMPLES, REFLECTION_BOUNCES
            );
            ReflectionsBaker::<DefaultRayTracer>::new()
                .bake_with_progress_callback(
                    &context,
                    &mut probe_batch,
                    &scene,
                    ReflectionsBakeParams {
                        identifier: reverb_id,
                        bake_flags: ReflectionsBakeFlags::BAKE_CONVOLUTION
                            | ReflectionsBakeFlags::BAKE_PARAMETRIC,
                        num_rays: REFLECTION_RAYS,
                        num_diffuse_samples: REFLECTION_DIFFUSE_SAMPLES,
                        num_bounces: REFLECTION_BOUNCES,
                        simulated_duration: REFLECTION_SIMULATED_DURATION_S,
                        saved_duration: REFLECTION_SAVED_DURATION_S,
                        order: REFLECTION_AMBISONIC_ORDER,
                        num_threads: threads,
                        irradiance_min_distance: REFLECTION_IRRADIANCE_MIN_DISTANCE_M,
                        bake_batch_size: REFLECTION_BAKE_BATCH_SIZE,
                    },
                    bake_progress_callback(
                        cache.map_name.clone(),
                        "reflections",
                        0.0,
                        0.85,
                        progress.clone(),
                    ),
                )
                .map_err(|error| format!("Steam Audio reflections bake failed: {error}"))?;
            if watchdog.cancelled() {
                return Ok(());
            }

            let path_id = path_identifier();
            devprintln!(1, "{}: Steam Audio pathing bake starting", cache.map_name);
            PathBaker::<DefaultRayTracer>::new()
                .bake_with_progress_callback(
                    &context,
                    &mut probe_batch,
                    &scene,
                    PathBakeParams {
                        identifier: path_id,
                        num_samples: PATH_NUM_SAMPLES,
                        radius: PATH_RADIUS_M,
                        threshold: PATH_VISIBILITY_THRESHOLD,
                        visibility_range: PATH_VISIBILITY_RANGE_M,
                        path_range: PATH_RANGE_M,
                        num_threads: threads,
                    },
                    bake_progress_callback(
                        cache.map_name.clone(),
                        "pathing",
                        0.85,
                        0.15,
                        progress.clone(),
                    ),
                )
                .map_err(|error| format!("Steam Audio pathing bake failed: {error}"))?;
            Ok(())
        })();
        watchdog.done.store(true, Ordering::Release);
        result
    });
    if watchdog.stale.load(Ordering::Acquire) {
        return Err("Steam Audio bake cancelled: its map is no longer current".into());
    }
    if watchdog.exceeded.load(Ordering::Acquire) {
        let growth_mib = watchdog.peak_growth_bytes.load(Ordering::Relaxed) / (1024 * 1024);
        let reason = format!(
            "Steam Audio bake cancelled: process memory grew {growth_mib} MiB, over the {} MiB bake budget ({probe_count} probes); this map will skip the bake",
            bake_memory_budget_mib(),
        );
        if let Err(error) = write_over_budget_marker(cache, probe_count, growth_mib) {
            eprintln!("{}: {error}", cache.map_name);
        }
        return Err(reason);
    }
    bake_result?;

    let reflection_data_bytes = probe_batch.data_size(reverb_identifier());
    let pathing_data_bytes = probe_batch.data_size(path_identifier());
    // Keep both scene and probe serialization on AudioNimbus' public Steam
    // Audio serialization APIs. This lets runtime validation exercise exactly
    // the bytes that will be cached and later consumed by the DSP layer.
    let scene_bytes = scene
        .try_save(&context)
        .map_err(|error| format!("could not serialize Steam Audio scene: {error}"))?
        .to_vec();
    let probe_batch_bytes = probe_batch
        .try_save(&context)
        .map_err(|error| format!("could not serialize Steam Audio probe batch: {error}"))?
        .to_vec();

    let mut data = SteamAudioBakeData {
        scene_bytes: Arc::from(scene_bytes),
        probe_batch_bytes: Arc::from(probe_batch_bytes),
        probe_count,
        reflection_data_bytes,
        pathing_data_bytes,
        cache_hit: false,
        runtime_validated: false,
    };
    let validation = validate_serialized_bake(&data)
        .map_err(|error| format!("fresh Steam Audio bake failed runtime validation: {error}"))?;
    data.runtime_validated = true;
    devprintln!(
        1,
        "{}: Steam Audio runtime assets validated: scene + {} probes; reflections {:.2} MiB, pathing {:.2} MiB",
        cache.map_name,
        validation.probe_count,
        validation.reflection_data_bytes as f64 / (1024.0 * 1024.0),
        validation.pathing_data_bytes as f64 / (1024.0 * 1024.0),
    );
    if let Err(error) = write_cache(cache, &data) {
        // The freshly baked bytes are still usable for this session. Treat cache
        // persistence as an optimization, not a requirement for map loading.
        eprintln!("{}: Steam Audio bake cache write failed: {error}", cache.map_name);
    }
    if let Some(progress) = &progress {
        progress(1.0);
    }
    devprintln!(
        1,
        "{}: Steam Audio bake complete in {:.1} s: reflections {:.2} MiB, pathing {:.2} MiB, serialized {:.2} MiB",
        cache.map_name,
        started.elapsed().as_secs_f64(),
        reflection_data_bytes as f64 / (1024.0 * 1024.0),
        pathing_data_bytes as f64 / (1024.0 * 1024.0),
        data.serialized_bytes() as f64 / (1024.0 * 1024.0),
    );
    Ok(data)
}

#[derive(Debug, Clone, Copy)]
struct RuntimeValidation {
    probe_count: usize,
    reflection_data_bytes: usize,
    pathing_data_bytes: usize,
}

/// Prove that the exact bytes we will hand to the runtime are accepted by
/// Steam Audio itself. A JSON manifest and non-empty files are not enough: a
/// partial/corrupt cache can otherwise look valid until the first DSP setup.
fn validate_serialized_bake(data: &SteamAudioBakeData) -> Result<RuntimeValidation, String> {
    let context = Context::default();

    let serialized_scene = SerializedObject::try_with_buffer(&context, data.scene_bytes.to_vec())
        .map_err(|error| format!("could not wrap serialized scene: {error}"))?;
    let _scene = Scene::<DefaultRayTracer>::load(&context, &serialized_scene)
        .map_err(|error| format!("could not deserialize scene: {error}"))?;

    let mut serialized_probes =
        SerializedObject::try_with_buffer(&context, data.probe_batch_bytes.to_vec())
            .map_err(|error| format!("could not wrap serialized probe batch: {error}"))?;
    let probe_batch = ProbeBatch::load(&context, &mut serialized_probes)
        .map_err(|error| format!("could not deserialize probe batch: {error}"))?;

    let probe_count = probe_batch.num_probes();
    if probe_count == 0 || probe_count > MAX_PROBES {
        return Err(format!("deserialized invalid probe count {probe_count}"));
    }
    if probe_count != data.probe_count {
        return Err(format!(
            "probe count mismatch after deserialize: manifest/bake {} vs Steam Audio {}",
            data.probe_count, probe_count
        ));
    }

    let reflection_data_bytes = probe_batch.data_size(reverb_identifier());
    let pathing_data_bytes = probe_batch.data_size(path_identifier());
    if reflection_data_bytes == 0 {
        return Err("deserialized probe batch has no baked reflection/reverb data".into());
    }
    if pathing_data_bytes == 0 {
        return Err("deserialized probe batch has no baked pathing data".into());
    }

    Ok(RuntimeValidation {
        probe_count,
        reflection_data_bytes,
        pathing_data_bytes,
    })
}

fn reverb_identifier() -> BakedDataIdentifier {
    BakedDataIdentifier::Reflections {
        variation: BakedDataVariation::Reverb,
    }
}

fn path_identifier() -> BakedDataIdentifier {
    BakedDataIdentifier::Pathing {
        variation: BakedDataVariation::Dynamic,
    }
}

fn bake_progress_callback(
    map_name: String,
    stage: &'static str,
    overall_start: f32,
    overall_span: f32,
    overall_progress: Option<SteamAudioBakeProgress>,
) -> ProgressCallback {
    let last_bucket = AtomicU8::new(u8::MAX);
    ProgressCallback::new(move |stage_progress| {
        let stage_progress = stage_progress.clamp(0.0, 1.0);
        if let Some(progress) = &overall_progress {
            progress((overall_start + stage_progress * overall_span).clamp(0.0, 1.0));
        }
        let percent = (stage_progress * 100.0).round() as u8;
        let bucket = (percent / 10).min(10);
        let previous = last_bucket.swap(bucket, Ordering::Relaxed);
        if previous != bucket {
            devprintln!(
                2,
                "{map_name}: Steam Audio {stage} bake {}%",
                u16::from(bucket) * 10
            );
        }
    })
}

#[derive(Default)]
struct BakeMemoryWatchdog {
    done: AtomicBool,
    exceeded: AtomicBool,
    stale: AtomicBool,
    peak_growth_bytes: AtomicU64,
}

impl BakeMemoryWatchdog {
    fn cancelled(&self) -> bool {
        self.exceeded.load(Ordering::Acquire) || self.stale.load(Ordering::Acquire)
    }

    fn run(&self, context: &Context, cancel_generation: u64) {
        let budget = bake_memory_budget_mib() * 1024 * 1024;
        let started = Instant::now();
        let mut baseline = None;
        let mut window = (Instant::now(), process_private_bytes().unwrap_or(0));
        while !self.done.load(Ordering::Acquire) {
            if BAKE_CANCEL_GENERATION.load(Ordering::Acquire) != cancel_generation {
                self.stale.store(true, Ordering::Release);
            }
            if let Some(now_bytes) = process_private_bytes() {
                match baseline {
                    Some(baseline) => {
                        let growth = now_bytes.saturating_sub(baseline);
                        self.peak_growth_bytes.fetch_max(growth, Ordering::Relaxed);
                        if growth > budget {
                            self.exceeded.store(true, Ordering::Release);
                        }
                    }
                    None if window.0.elapsed() >= BAKE_MEMORY_SETTLE_WINDOW => {
                        let settled = now_bytes.saturating_sub(window.1) < BAKE_MEMORY_SETTLE_BYTES;
                        if settled || started.elapsed() >= BAKE_MEMORY_SETTLE_TIMEOUT {
                            baseline = Some(now_bytes);
                        }
                        window = (Instant::now(), now_bytes);
                    }
                    None => {}
                }
            }
            // Re-sent every poll: a cancel that lands between the two stages
            // must still stop whichever baker starts next.
            if self.cancelled() {
                ReflectionsBaker::<DefaultRayTracer>::new().cancel_bake(context);
                PathBaker::<DefaultRayTracer>::new().cancel_bake(context);
            }
            std::thread::sleep(BAKE_MEMORY_POLL);
        }
    }
}

#[cfg(windows)]
fn process_private_bytes() -> Option<u64> {
    use windows_sys::Win32::System::{
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX},
        Threading::GetCurrentProcess,
    };
    // SAFETY: plain-old-data out struct sized by `cb`; the pseudo-handle needs no close.
    let mut counters: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    let ok = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(),
            counters.cb,
        )
    };
    (ok != 0).then_some(counters.PrivateUsage as u64)
}

#[cfg(not(windows))]
fn process_private_bytes() -> Option<u64> {
    None
}

/// `Some(reason)` when an earlier bake of this exact map/profile was cancelled
/// for exceeding a budget at least as large as the current one.
pub fn over_budget_skip(cache: &SteamAudioBakeCacheInfo) -> Option<String> {
    let path = cache.directory.join(OVER_BUDGET_MARKER);
    let marker: Value = serde_json::from_str(&fs::read_to_string(&path).ok()?).ok()?;
    if marker.get("bake_signature").and_then(Value::as_str) != Some(bake_signature()) {
        return None;
    }
    let budget = marker.get("budget_mib").and_then(Value::as_u64)?;
    if budget < bake_memory_budget_mib() {
        return None;
    }
    Some(format!(
        "Steam Audio bake skipped: an earlier bake of this map grew {} MiB with {} probes, over the {budget} MiB budget; delete {} to retry",
        marker.get("growth_mib").and_then(Value::as_u64).unwrap_or(0),
        marker.get("probe_count").and_then(Value::as_u64).unwrap_or(0),
        path.display(),
    ))
}

fn write_over_budget_marker(
    cache: &SteamAudioBakeCacheInfo,
    probe_count: usize,
    growth_mib: u64,
) -> Result<(), String> {
    fs::create_dir_all(&cache.directory).map_err(|error| {
        format!("could not create Steam Audio cache {}: {error}", cache.directory.display())
    })?;
    let marker = json!({
        "map_name": &cache.map_name,
        "map_hash": format!("{:016x}", cache.map_hash),
        "bake_signature": bake_signature(),
        "budget_mib": bake_memory_budget_mib(),
        "growth_mib": growth_mib,
        "probe_count": probe_count,
    });
    let bytes = serde_json::to_vec_pretty(&marker)
        .map_err(|error| format!("could not serialize Steam Audio over-budget marker: {error}"))?;
    atomic_write(&cache.directory.join(OVER_BUDGET_MARKER), &bytes)
}

/// Builds the runtime Steam Audio scene directly from the already-prepared BSP
/// acoustic mesh. Direct occlusion/transmission can use this immediately; it
/// does not need to wait for the slower reflections/pathing probe bake to finish.
pub(crate) fn build_scene_from_acoustic_mesh(
    context: &Context,
    mesh: &AcousticMesh,
) -> Result<Scene<DefaultRayTracer>, String> {
    if mesh.vertices.is_empty() || mesh.triangles.is_empty() {
        return Err("acoustic mesh is empty".into());
    }
    if mesh.materials.len() != mesh.triangles.len() {
        return Err(format!(
            "acoustic mesh material/triangle mismatch: {} materials for {} triangles",
            mesh.materials.len(),
            mesh.triangles.len()
        ));
    }

    let mut scene = Scene::<DefaultRayTracer>::try_new(context)
        .map_err(|error| format!("could not create Steam Audio scene: {error}"))?;
    let vertices = mesh
        .vertices
        .iter()
        .copied()
        .map(jka_to_steam_point)
        .collect::<Vec<_>>();
    let triangles = mesh
        .triangles
        .iter()
        .map(|triangle| {
            let a = i32::try_from(triangle[0]).map_err(|_| "acoustic vertex index overflow")?;
            let b = i32::try_from(triangle[1]).map_err(|_| "acoustic vertex index overflow")?;
            let c = i32::try_from(triangle[2]).map_err(|_| "acoustic vertex index overflow")?;
            Ok(Triangle::new(a, b, c))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let materials = steam_material_table();
    let material_indices = mesh
        .materials
        .iter()
        .map(|&material| usize::from(material.min(31)))
        .collect::<Vec<_>>();
    let static_mesh = StaticMesh::try_new(
        &scene,
        &StaticMeshSettings {
            vertices: &vertices,
            triangles: &triangles,
            material_indices: &material_indices,
            materials: &materials,
        },
    )
    .map_err(|error| format!("could not create Steam Audio static mesh: {error}"))?;
    scene.add_static_mesh(static_mesh);
    scene.commit();
    Ok(scene)
}

pub(crate) fn jka_to_steam_point(position: [f32; 3]) -> Point {
    // Same handedness-preserving basis used by the renderer: JKA +Z becomes
    // Steam Audio +Y, and JKA +Y becomes Steam Audio -Z.
    Point::new(
        position[0] / JKA_UNITS_PER_METER,
        position[2] / JKA_UNITS_PER_METER,
        -position[1] / JKA_UNITS_PER_METER,
    )
}

fn point_bounds(points: &[Point]) -> Result<([f32; 3], [f32; 3]), String> {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for point in points {
        let values = [point.x, point.y, point.z];
        for axis in 0..3 {
            if !values[axis].is_finite() {
                return Err("acoustic scene contains a non-finite vertex".into());
            }
            min[axis] = min[axis].min(values[axis]);
            max[axis] = max[axis].max(values[axis]);
        }
    }
    if min[0] == f32::INFINITY {
        return Err("acoustic scene contains no vertices".into());
    }
    Ok((min, max))
}

fn probe_volume_transform(min: [f32; 3], max: [f32; 3]) -> Matrix4 {
    let mut padded_min = min;
    let mut padded_max = max;
    for axis in 0..3 {
        padded_min[axis] -= PROBE_BOUNDS_PADDING_M;
        padded_max[axis] += PROBE_BOUNDS_PADDING_M;
        if padded_max[axis] - padded_min[axis] < 1.0 {
            padded_max[axis] = padded_min[axis] + 1.0;
        }
    }
    Matrix4::new([
        [padded_max[0] - padded_min[0], 0.0, 0.0, padded_min[0]],
        [0.0, padded_max[1] - padded_min[1], 0.0, padded_min[1]],
        [0.0, 0.0, padded_max[2] - padded_min[2], padded_min[2]],
        [0.0, 0.0, 0.0, 1.0],
    ])
}

/// Mapping uses Valve/AudioNimbus' built-in material presets only. JKA
/// materials without a direct preset intentionally fall back to GENERIC until
/// we have measured/source-backed coefficients instead of inventing values.
fn steam_material_table() -> [Material; 32] {
    [
        Material::GENERIC,  // 0 none
        Material::WOOD,     // 1 solid wood
        Material::WOOD,     // 2 hollow wood
        Material::METAL,    // 3 solid metal
        Material::METAL,    // 4 hollow metal
        Material::GENERIC,  // 5 short grass
        Material::GENERIC,  // 6 long grass
        Material::GENERIC,  // 7 dirt
        Material::GENERIC,  // 8 sand
        Material::GRAVEL,   // 9 gravel
        Material::GLASS,    // 10 glass
        Material::CONCRETE, // 11 concrete
        Material::CERAMIC,  // 12 marble
        Material::GENERIC,  // 13 water
        Material::GENERIC,  // 14 snow
        Material::GENERIC,  // 15 ice
        Material::GENERIC,  // 16 flesh
        Material::GENERIC,  // 17 mud
        Material::GLASS,    // 18 bulletproof glass
        Material::GENERIC,  // 19 dry leaves
        Material::GENERIC,  // 20 green leaves
        Material::CARPET,   // 21 fabric
        Material::CARPET,   // 22 canvas
        Material::ROCK,     // 23 rock
        Material::GENERIC,  // 24 rubber
        Material::GENERIC,  // 25 plastic
        Material::CERAMIC,  // 26 tiles
        Material::CARPET,   // 27 carpet
        Material::PLASTER,  // 28 plaster
        Material::GLASS,    // 29 shatter glass
        Material::METAL,    // 30 armor
        Material::METAL,    // 31 computer
    ]
}

fn bake_signature() -> &'static str {
    "jka32m-probe3.0x1.5-ref4096x128x32-2.0s-0.75s-o1-path1-r1-v50-p100-t0.5"
}

fn load_cache(cache: &SteamAudioBakeCacheInfo) -> Result<Option<SteamAudioBakeData>, String> {
    let manifest_path = cache.directory.join("manifest.json");
    let scene_path = cache.directory.join("scene.bin");
    let probes_path = cache.directory.join("probes.bin");
    let manifest_text = match fs::read_to_string(&manifest_path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "could not read Steam Audio cache {}: {error}",
                manifest_path.display()
            ));
        }
    };
    let manifest: Value = serde_json::from_str(&manifest_text)
        .map_err(|error| format!("invalid Steam Audio cache manifest: {error}"))?;
    let expected_map_hash = format!("{:016x}", cache.map_hash);
    let valid = manifest.get("schema_version").and_then(Value::as_u64)
        == Some(u64::from(CACHE_SCHEMA_VERSION))
        && manifest.get("bake_profile_version").and_then(Value::as_u64)
            == Some(u64::from(BAKE_PROFILE_VERSION))
        && manifest.get("map_hash").and_then(Value::as_str) == Some(expected_map_hash.as_str())
        && manifest.get("bake_signature").and_then(Value::as_str) == Some(bake_signature());
    if !valid {
        return Ok(None);
    }

    let scene_bytes = match fs::read(&scene_path) {
        Ok(bytes) if !bytes.is_empty() => bytes,
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("could not read {}: {error}", scene_path.display())),
    };
    let probe_batch_bytes = match fs::read(&probes_path) {
        Ok(bytes) if !bytes.is_empty() => bytes,
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("could not read {}: {error}", probes_path.display())),
    };

    let probe_count = manifest
        .get("probe_count")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(0);
    if probe_count == 0 || probe_count > MAX_PROBES {
        return Ok(None);
    }
    let reflection_data_bytes = manifest
        .get("reflection_data_bytes")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(0);
    let pathing_data_bytes = manifest
        .get("pathing_data_bytes")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(0);

    Ok(Some(SteamAudioBakeData {
        scene_bytes: Arc::from(scene_bytes),
        probe_batch_bytes: Arc::from(probe_batch_bytes),
        probe_count,
        reflection_data_bytes,
        pathing_data_bytes,
        cache_hit: true,
        runtime_validated: false,
    }))
}

fn write_cache(cache: &SteamAudioBakeCacheInfo, data: &SteamAudioBakeData) -> Result<(), String> {
    fs::create_dir_all(&cache.directory).map_err(|error| {
        format!(
            "could not create Steam Audio cache {}: {error}",
            cache.directory.display()
        )
    })?;
    atomic_write(&cache.directory.join("scene.bin"), data.scene_bytes.as_ref())?;
    atomic_write(
        &cache.directory.join("probes.bin"),
        data.probe_batch_bytes.as_ref(),
    )?;
    let manifest = json!({
        "schema_version": CACHE_SCHEMA_VERSION,
        "bake_profile_version": BAKE_PROFILE_VERSION,
        "steam_audio_version": "4.8.1",
        "audionimbus_version": "0.16.0",
        "map_name": &cache.map_name,
        "map_hash": format!("{:016x}", cache.map_hash),
        "bake_signature": bake_signature(),
        "jka_units_per_meter": JKA_UNITS_PER_METER,
        "probe_spacing_m": PROBE_SPACING_M,
        "probe_height_m": PROBE_HEIGHT_M,
        "probe_count": data.probe_count,
        "reflection_data_bytes": data.reflection_data_bytes,
        "pathing_data_bytes": data.pathing_data_bytes,
        "scene_serialized_bytes": data.scene_bytes.len(),
        "probe_batch_serialized_bytes": data.probe_batch_bytes.len(),
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| format!("could not serialize Steam Audio cache manifest: {error}"))?;
    atomic_write(&cache.directory.join("manifest.json"), &manifest_bytes)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or("cache")
    ));
    fs::write(&tmp, bytes).map_err(|error| format!("could not write {}: {error}", tmp.display()))?;
    if path.exists() {
        fs::remove_file(path)
            .map_err(|error| format!("could not replace {}: {error}", path.display()))?;
    }
    fs::rename(&tmp, path)
        .map_err(|error| format!("could not finalize {}: {error}", path.display()))
}

fn sanitize_map_name(name: &str) -> String {
    let mut safe = String::with_capacity(name.len());
    for character in name.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
            safe.push(character);
        } else {
            safe.push('_');
        }
    }
    if safe.is_empty() {
        "map".into()
    } else {
        safe
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_name_is_safe_for_cache_directory() {
        assert_eq!(sanitize_map_name("mp/ffa3"), "mp_ffa3");
        assert_eq!(sanitize_map_name("../bad:map"), "___bad_map");
    }

    #[test]
    fn jka_basis_is_meter_scaled_and_y_up() {
        let point = jka_to_steam_point([32.0, 64.0, 96.0]);
        assert_eq!([point.x, point.y, point.z], [1.0, 3.0, -2.0]);
    }
}
