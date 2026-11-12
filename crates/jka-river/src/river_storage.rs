//! `r_riverStorageMode`: the sparse tile pool's per-cell layout. Not an Unreal
//! system. Read when a sparse river spawns; every mode keeps the hydraulic
//! arithmetic in FP32 and the same stage bodies (shallow.wgsl).
//!
//! - 0 native: state A/B, geometry, PreSim history, WaterInfo, rendered
//!   surface and normals, 100 B/cell. The reference.
//! - 1 lean: PreSim history and WaterInfo are derived instead of stored
//!   (jka-water shallow_storage_lean.wgsl), so the PreSim snapshot pass is
//!   not dispatched; MatchSpline, an identity copy while spline matching is
//!   off (every sparse spawn), is not dispatched either; the remaining stages
//!   are recorded into one compute pass, PrepareSurface reads its smoothing
//!   neighbourhood from workgroup memory, and tiles that stay outside every
//!   occupied tile's guard ring are evicted (SparseSimulation::evict).
//!   80 B/cell. Bitwise identical state, surface and normals to mode 0.
//! - 2 packed: mode 1 plus ComputeNormals output in one octahedral 2x16 snorm
//!   word (jka-water normal_codec.wgsl), 68 B/cell. State and surface stay
//!   bitwise identical; normals are presentation-only and differ by ~1e-4 rad.
//!
//! The derivations rest on two facts of the sparse runtime: a committed bed
//! never changes (river_collisions.wgsl rewrites only collider and collision
//! velocity) and WaterInfo is never written (sparse rivers are simulation
//! first, spawned with match_spline off and no native sink boxes). A future
//! bed recapture or WaterInfo source must use mode 0.

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Storage {
    Native,
    Lean,
    Packed,
}

// Default 2: bitwise-identical water, ~35% less solver GPU time (measured).
static MODE: AtomicU8 = AtomicU8::new(2);

#[cfg(test)]
thread_local! {
    static OVERRIDE: std::cell::Cell<Option<Storage>> = const { std::cell::Cell::new(None) };
}

pub fn mode() -> u8 {
    MODE.load(Ordering::Relaxed)
}
pub fn set_mode(mode: u8) {
    MODE.store(mode.min(2), Ordering::Relaxed);
}
pub fn mode_name(mode: u8) -> &'static str {
    Storage::from_mode(mode).name()
}

/// Tests pin the layout per thread instead of the session cvar.
#[cfg(test)]
pub(crate) fn override_for_thread(storage: Option<Storage>) {
    OVERRIDE.with(|o| o.set(storage));
}

impl Storage {
    pub fn from_mode(mode: u8) -> Self {
        match mode {
            0 => Self::Native,
            1 => Self::Lean,
            _ => Self::Packed,
        }
    }

    /// The layout a sparse river spawned now uses.
    pub fn selected() -> Self {
        #[cfg(test)]
        if let Some(storage) = OVERRIDE.with(std::cell::Cell::get) {
            return storage;
        }
        Self::from_mode(mode())
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Lean => "lean",
            Self::Packed => "packed",
        }
    }

    /// PreSim history and WaterInfo are real buffers (and the snapshot runs).
    pub fn stores_history(self) -> bool {
        self == Self::Native
    }
    pub fn packed_normals(self) -> bool {
        self == Self::Packed
    }
    /// Bytes per cell of the normals buffer.
    pub fn normal_stride(self) -> u64 {
        if self.packed_normals() { 4 } else { 16 }
    }
    /// Pool bytes per cell over every per-cell buffer.
    pub fn bytes_per_cell(self) -> u64 {
        let state = 2 * 16;
        let geometry = 16;
        let surface = 16;
        let history_and_info = if self.stores_history() { 4 + 16 } else { 0 };
        state + geometry + surface + self.normal_stride() + history_and_info
    }

    /// Solver stages (shallow.wgsl) with this layout's storage prelude.
    pub fn stage_shader(self, worklist: bool) -> &'static str {
        use jka_water::river as w;
        match (self, worklist) {
            (Self::Native, false) => w::SPARSE_SHALLOW_SHADER,
            (Self::Native, true) => w::SPARSE_WORKLIST_SHALLOW_SHADER,
            (Self::Lean, false) => w::SPARSE_LEAN_SHALLOW_SHADER,
            (Self::Lean, true) => w::SPARSE_LEAN_WORKLIST_SHALLOW_SHADER,
            (Self::Packed, false) => w::SPARSE_PACKED_SHALLOW_SHADER,
            (Self::Packed, true) => w::SPARSE_PACKED_WORKLIST_SHALLOW_SHADER,
        }
    }

    /// Entry point of a stage. The derived layouts' shaders also carry
    /// workgroup-memory variants (shallow_sparse_shared.wgsl) of the stencil
    /// stages, bitwise identical to the native bodies.
    pub fn stage_entry(self, kernel: crate::river_gpu::Kernel) -> &'static str {
        use crate::river_gpu::Kernel;
        match (self, kernel) {
            (Self::Native, k) => k.entry(),
            (_, Kernel::PrepareSurface) => "prepare_surface_shared",
            (_, k) => k.entry(),
        }
    }

    /// river_tiles.wgsl (commit, window gather, inlet, void walls).
    pub fn tiles_shader(self, body: &str) -> String {
        let prelude = match self {
            Self::Native => TILES_NATIVE,
            Self::Lean => TILES_LEAN,
            Self::Packed => TILES_PACKED,
        };
        self.compose(prelude, body)
    }

    /// river_tile_schedule.wgsl (classify, worklist).
    pub fn schedule_shader(self, body: &str) -> String {
        let prelude = match self {
            Self::Native => SCHEDULE_NATIVE,
            Self::Lean => SCHEDULE_LEAN,
            Self::Packed => SCHEDULE_PACKED,
        };
        self.compose(prelude, body)
    }

    fn compose(self, prelude: &str, body: &str) -> String {
        let codec = if self.packed_normals() { jka_water::river::NORMAL_CODEC } else { "" };
        [prelude, codec, body].join("\n")
    }
}

/// CPU twin of `river_unpack_normal` (readback and tests).
pub fn unpack_normal(packed: u32) -> [f32; 3] {
    if packed == 0 {
        return [0.0, 0.0, 1.0];
    }
    let snorm = |bits: u32| (f32::from(bits as u16 as i16) / 32767.0).clamp(-1.0, 1.0);
    let e = [snorm(packed & 0xffff), snorm(packed >> 16)];
    let mut n = [e[0], e[1], 1.0 - e[0].abs() - e[1].abs()];
    let t = (-n[2]).max(0.0);
    for v in &mut n[..2] {
        *v += if *v >= 0.0 { -t } else { t };
    }
    let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    n.map(|v| v / length)
}

const TILES_NATIVE: &str = r"
@group(0) @binding(3) var<storage, read_write> commit_history: array<f32>;
@group(0) @binding(7) var<storage, read_write> commit_normals: array<vec4<f32>>;
@group(0) @binding(12) var<storage, read> window_pool_normals: array<vec4<f32>>;
fn commit_store_history(i: u32, bottom: f32) { commit_history[i] = bottom; }
fn commit_store_rest_normal(i: u32) { commit_normals[i] = vec4<f32>(0.0, 0.0, 1.0, 0.0); }
fn window_pool_normal(i: u32) -> vec4<f32> { return window_pool_normals[i]; }
";

// No history buffer: the stages derive it (shallow_storage_lean.wgsl).
const TILES_LEAN: &str = r"
@group(0) @binding(7) var<storage, read_write> commit_normals: array<vec4<f32>>;
@group(0) @binding(12) var<storage, read> window_pool_normals: array<vec4<f32>>;
fn commit_store_history(i: u32, bottom: f32) {}
fn commit_store_rest_normal(i: u32) { commit_normals[i] = vec4<f32>(0.0, 0.0, 1.0, 0.0); }
fn window_pool_normal(i: u32) -> vec4<f32> { return window_pool_normals[i]; }
";

// The window keeps FP32 normals for the material chain; only the pool packs.
const TILES_PACKED: &str = r"
@group(0) @binding(7) var<storage, read_write> commit_normals: array<u32>;
@group(0) @binding(12) var<storage, read> window_pool_normals: array<u32>;
fn commit_store_history(i: u32, bottom: f32) {}
fn commit_store_rest_normal(i: u32) { commit_normals[i] = 0u; }
fn window_pool_normal(i: u32) -> vec4<f32> { return vec4<f32>(river_unpack_normal(window_pool_normals[i]), 0.0); }
";

const SCHEDULE_NATIVE: &str = r"
@group(0) @binding(4) var<storage, read> history: array<f32>;
@group(0) @binding(6) var<storage, read> normals: array<vec4<f32>>;
fn history_rests(i: u32, bed: f32) -> bool { return history[i] == bed; }
fn normal_rests(i: u32) -> bool { return all(normals[i] == vec4<f32>(0.0, 0.0, 1.0, 0.0)); }
";

// Derived history is the bed except on the first solver frame, when it is the
// reset zero. Then both states are the reset zero too, and they equal
// (bed, 0, 0, 0) exactly when history 0 == bed would hold: the state test
// already decides it. Mode 0 also sees history 0 at the second frame's
// classify (its snapshot runs after it); the tiles that wakes are dry fixed
// points whose frame is the identity, so the pool is unchanged.
const SCHEDULE_LEAN: &str = r"
@group(0) @binding(6) var<storage, read> normals: array<vec4<f32>>;
fn history_rests(i: u32, bed: f32) -> bool { return true; }
fn normal_rests(i: u32) -> bool { return all(normals[i] == vec4<f32>(0.0, 0.0, 1.0, 0.0)); }
";

const SCHEDULE_PACKED: &str = r"
@group(0) @binding(6) var<storage, read> normals: array<u32>;
fn history_rests(i: u32, bed: f32) -> bool { return true; }
fn normal_rests(i: u32) -> bool { return normals[i] == 0u; }
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_bytes_per_cell() {
        assert_eq!(Storage::Native.bytes_per_cell(), 100);
        assert_eq!(Storage::Lean.bytes_per_cell(), 80);
        assert_eq!(Storage::Packed.bytes_per_cell(), 68);
    }

    #[test]
    fn rest_normal_round_trips_exactly() {
        assert_eq!(unpack_normal(0), [0.0, 0.0, 1.0]);
    }

    #[test]
    fn storage_shaders_validate() {
        for storage in [Storage::Native, Storage::Lean, Storage::Packed] {
            for source in [
                storage.stage_shader(false).to_string(),
                storage.stage_shader(true).to_string(),
                storage.tiles_shader(super::super::TILES_BODY),
                storage.schedule_shader(super::super::schedule::BODY),
            ] {
                let module = wgpu::naga::front::wgsl::parse_str(&source)
                    .unwrap_or_else(|e| panic!("{storage:?} WGSL parses: {}", e.emit_to_string(&source)));
                wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(), wgpu::naga::valid::Capabilities::default())
                    .validate(&module)
                    .unwrap_or_else(|e| panic!("{storage:?} WGSL validates: {e:?}"));
            }
        }
    }
}
