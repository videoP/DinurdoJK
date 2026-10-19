//! Optional, local-only MD3 fallback LODs. TaystJK authored _1/_2 files win.
//! Geometry simplification/cache I/O run only on this worker, never on draw.
use jka_assets::md3::{Model, Surface};
use std::{collections::HashMap, fs, hash::{Hash, Hasher}, io::{Read, Write}, path::PathBuf,
    sync::{mpsc::{self, Receiver, SyncSender}, Arc}, thread};

pub(super) const LODS: usize = 3; // TaystJK / Quake 3 MD3_MAX_LODS
const MAGIC: &[u8; 8] = b"DJMD3L03"; // invalidate old caches when algorithm changes
const CACHE_MAX_BYTES: u64 = 16 * 1024 * 1024;

pub(super) struct Job {
    pub key: String,
    pub hash: u64,
    pub model: Arc<Model>,
    pub missing: [bool; 2],
}
pub(super) struct ResultJob {
    pub key: String,
    pub hash: u64,
    pub lods: [Option<Model>; 2],
    pub source: &'static str,
}
pub(super) struct Worker {
    tx: SyncSender<Job>,
    rx: Receiver<ResultJob>,
}
impl Worker {
    pub fn new() -> Self {
        let (tx, jobs) = mpsc::sync_channel::<Job>(32);
        let (results, rx) = mpsc::channel();
        // One serial worker per presenter: no unbounded thread-per-MD3 creation.
        let _ = thread::Builder::new().name("md3-lod-cache".into()).spawn(move || {
            while let Ok(job) = jobs.recv() {
                let (lods, source) = run(&job);
                if results.send(ResultJob { key: job.key, hash: job.hash, lods, source }).is_err() { break; }
            }
        });
        Self { tx, rx }
    }
    pub fn queue(&self, job: Job) -> bool { self.tx.try_send(job).is_ok() }
    pub fn pop(&self) -> Option<ResultJob> { self.rx.try_recv().ok() }
}

pub(super) fn content_hash(data: &[u8]) -> u64 {
    // Stable FNV-1a, independent of per-process hash seeds, includes all PK3 overrides.
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in data { hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3); }
    hash
}
fn cache_path(job: &Job, lod: usize) -> PathBuf {
    let root = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    job.key.hash(&mut hasher);
    root.join("DinurdoJK").join("md3-lod-v2")
        .join(format!("{:016x}-{:016x}-{lod}.bin", hasher.finish(), job.hash))
}
fn read_cache(path: &PathBuf, model: &Model) -> Option<Vec<Vec<u32>>> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.len() > CACHE_MAX_BYTES { return None; }
    let mut data = Vec::with_capacity(metadata.len() as usize);
    fs::File::open(path).ok()?.take(CACHE_MAX_BYTES + 1).read_to_end(&mut data).ok()?;
    if !data.starts_with(MAGIC) { return None; }
    let mut pos = 8;
    let mut next = || -> Option<u32> {
        let bytes = data.get(pos..pos + 4)?;
        pos += 4;
        Some(u32::from_le_bytes(bytes.try_into().ok()?))
    };
    if next()? as usize != model.surfaces.len() { return None; }
    let mut output = Vec::with_capacity(model.surfaces.len());
    for surface in &model.surfaces {
        let count = next()? as usize;
        if count > surface.indices.len() || count % 3 != 0 { return None; }
        let mut indices = Vec::with_capacity(count);
        for _ in 0..count {
            let index = next()?;
            if index as usize >= surface.vertices.len() { return None; }
            indices.push(index);
        }
        output.push(indices);
    }
    (pos == data.len()).then_some(output)
}
fn write_cache(path: &PathBuf, indices: &[Vec<u32>]) {
    let Some(parent) = path.parent() else { return; };
    if fs::create_dir_all(parent).is_err() { return; }
    let mut data = Vec::new();
    data.extend_from_slice(MAGIC);
    data.extend_from_slice(&(indices.len() as u32).to_le_bytes());
    for surface in indices {
        data.extend_from_slice(&(surface.len() as u32).to_le_bytes());
        for index in surface { data.extend_from_slice(&index.to_le_bytes()); }
    }
    if data.len() as u64 > CACHE_MAX_BYTES { return; }
    let tmp = path.with_extension("tmp");
    if fs::File::create(&tmp).and_then(|mut f| f.write_all(&data)).is_ok() {
        let _ = fs::rename(&tmp, path);
    } else { let _ = fs::remove_file(tmp); }
}
/// meshoptimizer's own extent: the largest axis span of the positions.
fn extent(positions: impl Iterator<Item = [f32; 3]>) -> f32 {
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for p in positions {
        for axis in 0..3 { min[axis] = min[axis].min(p[axis]); max[axis] = max[axis].max(p[axis]); }
    }
    (0..3).map(|axis| max[axis] - min[axis]).fold(0.0_f32, f32::max)
}
/// `model_extent` makes the error budget model-relative: prop MD3s are many
/// tiny surfaces (2-tri foliage cards, vine strips), and an error relative to
/// each surface's own extent never lets a far LOD drop or collapse them.
fn reduced_indices(surface: &Surface, lod: usize, model_extent: f32) -> Vec<u32> {
    let original = &surface.indices;
    if original.len() < 6 || surface.frames.is_empty() { return original.clone(); }
    // Do not guess animated shape changes from frame 0. Authored animated MD3
    // variants continue to work; generated fallback is conservative here.
    if surface.frames.len() > 1 && surface.frames.iter().skip(1).any(|frame| {
        frame.len() != surface.vertices.len() || frame.iter().zip(&surface.vertices)
            .any(|(a,b)| a.position != b.position)
    }) { return original.clone(); }
    let positions: Vec<[f32; 3]> = surface.vertices.iter().map(|v| v.position).collect();
    // Normals and UVs belong to the original vertex IDs, including seams.
    // Weight their error to retain lighting and textured silhouettes.
    let attributes: Vec<[f32; 5]> = surface.vertices.iter().map(|v| [
        v.normal[0], v.normal[1], v.normal[2], v.uv[0], v.uv[1],
    ]).collect();
    // LOD2 is only seen far away: light seam weights let hard-edged props
    // (split normals/UVs on every face) collapse at all.
    let weights = if lod == 1 { [0.5_f32, 0.5, 0.5, 1.0, 1.0] } else { [0.05, 0.05, 0.05, 0.2, 0.2] };
    let ratio = if lod == 1 { 0.5 } else { 0.2 };
    // Fraction of the whole model's extent, converted to meshoptimizer's
    // surface-relative error (attributes keep their usual relative weighting).
    let model_error = if lod == 1 { 0.02 } else { 0.05 };
    let surface_extent = extent(positions.iter().copied());
    if surface_extent <= 0.0 { return original.clone(); }
    let error = model_error * model_extent.max(surface_extent) / surface_extent;
    // Prune drops disconnected pieces smaller than the error (far foliage
    // cards); Permissive lets LOD2 collapse across attribute seams.
    let options = meshopt::SimplifyOptions::Prune.bits()
        | if lod == 1 { 0 } else { meshopt::SimplifyOptions::Permissive.bits() };
    // meshoptimizer respects topology, borders and seams; never use 'sloppy'.
    // Packing [f32;3] guarantees the FFI stride and alignment.
    let mut output = vec![0u32; original.len()];
    let count = unsafe {
        meshopt::ffi::meshopt_simplifyWithAttributes(
            output.as_mut_ptr(), original.as_ptr(), original.len(),
            positions.as_ptr().cast::<f32>(), positions.len(), 12,
            attributes.as_ptr().cast::<f32>(), 20,
            weights.as_ptr(), weights.len(), std::ptr::null(),
            ((original.len() as f32 * ratio) as usize / 3 * 3).max(3),
            error, options, std::ptr::null_mut(),
        )
    };
    output.truncate(count);
    // Zero is valid: a far LOD may prune a whole small surface.
    if output.len() < original.len() && output.len() % 3 == 0 {
        output
    } else { original.clone() }
}
fn compact_surface(src: &Surface, indices: &[u32]) -> Surface {
    // Every animation frame retains precisely the same original vertex IDs.
    let mut remap = HashMap::new();
    let mut old_ids = Vec::new();
    let remapped: Vec<u32> = indices.iter().map(|&id| *remap.entry(id).or_insert_with(|| {
        let n = old_ids.len() as u32;
        old_ids.push(id as usize);
        n
    })).collect();
    let frames = src.frames.iter().map(|frame| old_ids.iter().map(|&i| frame[i]).collect()).collect();
    let vertices = old_ids.iter().map(|&i| src.vertices[i]).collect();
    Surface { name: src.name.clone(), shader: src.shader.clone(), vertices, frames, indices: remapped }
}
fn build_model(base: &Model, index_sets: Vec<Vec<u32>>) -> Model {
    let surfaces = base.surfaces.iter().zip(index_sets.iter())
        .map(|(src, indices)| compact_surface(src, indices)).collect();
    Model { frames: base.frames.clone(), tags: base.tags.clone(), surfaces }
}
/// Below this many triangles in the whole model a fallback LOD is not worth
/// a second copy.
const MIN_MODEL_TRIS: usize = 32;
fn run(job: &Job) -> ([Option<Model>; 2], &'static str) {
    let mut output: [Option<Model>; 2] = [None, None];
    let model_tris: usize = job.model.surfaces.iter().map(|s| s.indices.len() / 3).sum();
    if model_tris < MIN_MODEL_TRIS { return (output, "too small"); }
    let model_extent = job.model.frames.first()
        .map(|frame| (0..3).map(|axis| frame.maxs[axis] - frame.mins[axis]).fold(0.0_f32, f32::max))
        .filter(|extent| *extent > 0.0)
        .unwrap_or_else(|| extent(job.model.surfaces.iter()
            .flat_map(|s| s.vertices.iter().map(|v| v.position))));
    let mut all_cache = true;
    for lod in 1..LODS {
        if !job.missing[lod - 1] { continue; }
        let path = cache_path(job, lod);
        let indices = if let Some(cached) = read_cache(&path, &job.model) { cached }
        else {
            all_cache = false;
            let indices: Vec<Vec<u32>> = job.model.surfaces.iter()
                .map(|surface| reduced_indices(surface, lod, model_extent)).collect();
            write_cache(&path, &indices);
            indices
        };
        // No reduction -> don't register a second GPU/model copy.
        if indices.iter().zip(&job.model.surfaces).any(|(i, s)| i.len() < s.indices.len()) {
            output[lod - 1] = Some(build_model(&job.model, indices));
        }
    }
    (output, if all_cache { "disk cache" } else { "meshoptimizer" })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn content_hash_changes() { assert_ne!(content_hash(b"a"), content_hash(b"b")); }
    #[test]
    #[ignore = "requires JKA_TEST_BASE with stock PK3s"]
    fn prop_foliage_reduces_at_far_lods() {
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let mut assets = jka_assets::pk3::AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        for path in ["models/map_objects/yavin/tree09_b.md3", "models/map_objects/imperial/imp_light.md3"] {
            let data = assets.read(path, 16 * 1024 * 1024).unwrap().unwrap().bytes;
            let model = Arc::new(jka_assets::md3::parse(&data).unwrap());
            let tris = |lod: usize| -> usize {
                let frame = &model.frames[0];
                let extent = (0..3).map(|a| frame.maxs[a] - frame.mins[a]).fold(0.0_f32, f32::max);
                if lod == 0 { return model.surfaces.iter().map(|s| s.indices.len() / 3).sum(); }
                model.surfaces.iter().map(|s| reduced_indices(s, lod, extent).len() / 3).sum()
            };
            let counts = [tris(0), tris(1), tris(2)];
            println!("{path}: tris LOD0/1/2 = {counts:?}");
            assert!(counts[1] <= counts[0] && counts[2] < counts[1], "{path}: {counts:?}");
        }
    }

    #[test]
    fn compact_keeps_all_animation_frames() {
        use jka_assets::md3::Vertex;
        let make = |x| Vertex { position: [x,0.0,0.0], normal: [0.0,0.0,1.0], uv: [0.0,0.0] };
        let src = Surface { name: "s".into(), shader: "t".into(), vertices: vec![make(0.0),make(1.0),make(2.0),make(3.0)], frames: vec![vec![make(0.0),make(1.0),make(2.0),make(3.0)], vec![make(4.0),make(5.0),make(6.0),make(7.0)]], indices: vec![0,1,2,0,2,3] };
        let out = compact_surface(&src, &[3,1,2]);
        assert_eq!(out.indices, vec![0,1,2]);
        assert_eq!(out.frames[1][0].position[0], 7.0);
    }
}
