//! Live local river: the native Unreal river solver on a patch of map collision
//! geometry, fed by one native source box (simulation-first: no spline,
//! spline matching off, dry WaterInfo). Built for the developer "place water"
//! command; drawing is the renderer's job and reads `surface()`.
use crate::river_capture::{BspCapture, DepthCaptures};
#[path = "river_solve.rs"]
mod river_solve;
#[path = "river_wet_probe.rs"]
mod river_wet_probe;
use river_solve::SolveConvergence;
use river_wet_probe::WetSpotProbe;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::time::{Duration, Instant};

// A 1-pixel GPU test of the *solved*, displaced water column feeds the
// listener's existing s_underwater path. All traffic is nonblocking atomics:
// audio publishes the current ear (JKA coordinates); the renderer publishes
// completed readbacks only, with a position/time so stale or distant results
// cannot muffle audio after teleports, map changes or spectator transitions.
static AUDIO_RIVERS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static AUDIO_EYE: [std::sync::atomic::AtomicU32; 3] = [
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
];
static AUDIO_FOOT: [std::sync::atomic::AtomicU32; 3] = [
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
];
static AUDIO_EYE_TIME: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static AUDIO_EYE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static AUDIO_RESULT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static AUDIO_RESULT_EYE: [std::sync::atomic::AtomicU32; 3] = [
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
];
static AUDIO_RESULT_FOOT: [std::sync::atomic::AtomicU32; 3] = [
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
];
static AUDIO_RESULT_FOOT_WET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static AUDIO_RESULT_TIME: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static AUDIO_RESULT_WET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn audio_clock_ms() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

/// Render-thread river lifecycle. Invalidates all results on map unload.
pub fn set_audio_rivers_present(present: bool) {
    AUDIO_RIVERS.store(present, Ordering::Release);
    if !present {
        AUDIO_EYE_TIME.store(0, Ordering::Release);
        AUDIO_RESULT_SEQ.fetch_add(1, Ordering::AcqRel);
        AUDIO_RESULT_TIME.store(0, Ordering::Relaxed);
        AUDIO_RESULT_WET.store(false, Ordering::Relaxed);
        AUDIO_RESULT_FOOT_WET.store(false, Ordering::Relaxed);
        AUDIO_RESULT_SEQ.fetch_add(1, Ordering::Release);
    }
}

/// The probe tracks the listener's eye (audio filtering) and foot (JKA
/// water-touch transition) independently, against the same displaced surface.
#[derive(Clone, Copy, Debug)]
pub struct AudioListenerProbe {
    pub eye: [f32; 3],
    pub foot: [f32; 3],
}

#[derive(Clone, Copy, Debug)]
pub struct RiverAudioContact {
    pub eye: bool,
    pub foot: bool,
}

pub fn audio_rivers_present() -> bool { AUDIO_RIVERS.load(Ordering::Acquire) }

/// Session thread: publish one listener probe and consume the newest fresh
/// asynchronous result. Unavailable/outdated samples return None, not dry;
/// callers must not synthesize water-leave sounds from missing GPU results.
pub fn listener_water_contact(eye: [f32; 3], foot: [f32; 3]) -> Option<RiverAudioContact> {
    if !audio_rivers_present() || !eye.iter().chain(foot.iter()).all(|v| v.is_finite()) {
        return None;
    }
    AUDIO_EYE_SEQ.fetch_add(1, Ordering::AcqRel);
    for (slot, value) in AUDIO_EYE.iter().zip(eye) { slot.store(value.to_bits(), Ordering::Relaxed); }
    for (slot, value) in AUDIO_FOOT.iter().zip(foot) { slot.store(value.to_bits(), Ordering::Relaxed); }
    let now = audio_clock_ms();
    AUDIO_EYE_TIME.store(now, Ordering::Relaxed);
    AUDIO_EYE_SEQ.fetch_add(1, Ordering::Release);
    let start = AUDIO_RESULT_SEQ.load(Ordering::Acquire);
    if start & 1 != 0 { return None; }
    let time = AUDIO_RESULT_TIME.load(Ordering::Relaxed);
    let eye_wet = AUDIO_RESULT_WET.load(Ordering::Relaxed);
    let foot_wet = AUDIO_RESULT_FOOT_WET.load(Ordering::Relaxed);
    let sampled_eye = AUDIO_RESULT_EYE.each_ref().map(|v| f32::from_bits(v.load(Ordering::Relaxed)));
    let sampled_foot = AUDIO_RESULT_FOOT.each_ref().map(|v| f32::from_bits(v.load(Ordering::Relaxed)));
    if start != AUDIO_RESULT_SEQ.load(Ordering::Acquire) || time == 0 || now.saturating_sub(time) > 500 {
        return None;
    }
    // Reject results from another POV/location (teleports, demo seeks,
    // third-person offsets); normal ~100ms async latency is tolerated.
    let nearby = |a: [f32; 3], b: [f32; 3]| -> bool {
        a.iter().zip(b).map(|(x,y)| (x-y)*(x-y)).sum::<f32>() < 48.0*48.0
    };
    (nearby(sampled_eye, eye) && nearby(sampled_foot, foot)).then_some(RiverAudioContact {
        eye: eye_wet,
        foot: foot_wet,
    })
}

/// Demo listeners have no reliable feet: preserve the existing eye-only path.
pub fn listener_in_river(eye: [f32; 3]) -> bool {
    listener_water_contact(eye, eye).is_some_and(|water| water.eye)
}

/// Render thread: returns None when audio isn't querying river water.
pub fn audio_listener_probe() -> Option<AudioListenerProbe> {
    if !audio_rivers_present() { return None; }
    let now = audio_clock_ms();
    let start = AUDIO_EYE_SEQ.load(Ordering::Acquire);
    if start & 1 != 0 { return None; }
    let time = AUDIO_EYE_TIME.load(Ordering::Relaxed);
    let eye = AUDIO_EYE.each_ref().map(|v| f32::from_bits(v.load(Ordering::Relaxed)));
    let foot = AUDIO_FOOT.each_ref().map(|v| f32::from_bits(v.load(Ordering::Relaxed)));
    if start != AUDIO_EYE_SEQ.load(Ordering::Acquire) || time == 0 || now.saturating_sub(time) > 300 { return None; }
    eye.iter().chain(foot.iter()).all(|v| v.is_finite()).then_some(AudioListenerProbe {eye, foot})
}

pub fn publish_audio_result(sample: AudioListenerProbe, water: RiverAudioContact) {
    if !audio_rivers_present() { return; }
    AUDIO_RESULT_SEQ.fetch_add(1, Ordering::AcqRel);
    for (slot, value) in AUDIO_RESULT_EYE.iter().zip(sample.eye) { slot.store(value.to_bits(), Ordering::Relaxed); }
    for (slot, value) in AUDIO_RESULT_FOOT.iter().zip(sample.foot) { slot.store(value.to_bits(), Ordering::Relaxed); }
    AUDIO_RESULT_WET.store(water.eye, Ordering::Relaxed);
    AUDIO_RESULT_FOOT_WET.store(water.foot, Ordering::Relaxed);
    AUDIO_RESULT_TIME.store(audio_clock_ms(), Ordering::Relaxed);
    AUDIO_RESULT_SEQ.fetch_add(1, Ordering::Release);
}

// Session-only console cvar. Both the app and render threads read this; with
// mode 0 the simulation performs no diagnostic allocation, logging or timing.
static RIVER_DEBUG: AtomicU8 = AtomicU8::new(0);

pub fn debug_mode() -> u8 {
    RIVER_DEBUG.load(Ordering::Relaxed)
}

pub fn set_debug_mode(mode: u8) {
    RIVER_DEBUG.store(mode.min(2), Ordering::Relaxed);
}

// r_riverPause (session-only A/B): no solver frames run, so no simulated
// time accumulates; presentation (FFT detail, foam, window, body draw)
// keeps running on the last solved state. Not an Unreal control.
static RIVER_PAUSE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn paused() -> bool {
    RIVER_PAUSE.load(Ordering::Relaxed)
}

pub fn set_paused(paused: bool) {
    RIVER_PAUSE.store(paused, Ordering::Relaxed);
}

// Render-height A/B (River menu, r_riverSmoothing / r_riverSmoothingCutoff /
// r_riverBedClamp). Smoothing is upstream's Grid2D_SW_ExtrapolateRenderHeight
// SmoothingMethod (0 none, 1 edge-preserving, 2 the cached River value, 3
// plain) and SmoothingHeightCutoff (cm). The bed clamp is not Unreal: wet
// render heights stay on or above their bed, and steep vertices lift to the
// upper water so pourovers fall in front of the lip instead of under it.
static SMOOTHING_METHOD: AtomicU8 = AtomicU8::new(2);
static SMOOTHING_CUTOFF_CM: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x43FA_0000); // 500.0
static BED_CLAMP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn smoothing_method() -> u8 {
    SMOOTHING_METHOD.load(Ordering::Relaxed)
}
pub fn set_smoothing_method(method: u8) {
    SMOOTHING_METHOD.store(method.min(3), Ordering::Relaxed);
}
pub fn smoothing_cutoff_cm() -> f32 {
    f32::from_bits(SMOOTHING_CUTOFF_CM.load(Ordering::Relaxed))
}
pub fn set_smoothing_cutoff_cm(cm: f32) {
    let cm = if cm.is_finite() { cm.max(0.0) } else { 500.0 };
    SMOOTHING_CUTOFF_CM.store(cm.to_bits(), Ordering::Relaxed);
}
pub fn bed_clamp() -> bool {
    BED_CLAMP.load(Ordering::Relaxed)
}
pub fn set_bed_clamp(enabled: bool) {
    BED_CLAMP.store(enabled, Ordering::Relaxed);
}

// Render-only fragment detail control; independent of the solver and r_riverAdaptiveMesh.
// 0: native wave/foam (A), 1: screen-space advected-wave sampling,
// 2: also screen-space bicubic-to-bilinear foam field reconstruction.
static MATERIAL_LOD: AtomicU8 = AtomicU8::new(0);

pub fn material_lod() -> u8 {
    MATERIAL_LOD.load(Ordering::Relaxed)
}
pub fn set_material_lod(mode: u8) {
    MATERIAL_LOD.store(mode.min(2), Ordering::Relaxed);
}

// r_riverMaterialLODCutoff: advected-wave mip (bias included) at which modes 1/2
// take the cheap path; foam switches 3.415 mips earlier (1.5 cells/pixel at 4),
// never below 1 cell/pixel.
static MATERIAL_LOD_CUTOFF: AtomicU32 = AtomicU32::new(0x4000_0000); // 2.0

pub fn material_lod_cutoff() -> f32 {
    f32::from_bits(MATERIAL_LOD_CUTOFF.load(Ordering::Relaxed))
}
pub fn set_material_lod_cutoff(mip: f32) {
    let mip = if mip.is_finite() { mip.clamp(0.0, 8.0) } else { 2.0 };
    MATERIAL_LOD_CUTOFF.store(mip.to_bits(), Ordering::Relaxed);
}

// r_riverWhitewater: Unreal's SW_River_Secondary_Emitter (on in LiveSim).
// Off frees the particle buffer; on respawns the burst.
static WHITEWATER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn whitewater() -> bool {
    WHITEWATER.load(Ordering::Relaxed)
}
pub fn set_whitewater(enabled: bool) {
    WHITEWATER.store(enabled, Ordering::Relaxed);
}

/// A moving player hull in map units for the river's PreSim collisions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapBody {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    /// Map units per second.
    pub velocity: [f32; 3],
}

struct DebugStats {
    created: Instant,
    last_report: Instant,
    active: bool,
    frames: u64,
    zero_step_frames: u64,
    steps: u64,
    capped_frames: u64,
    dropped_seconds: f64,
    cpu_encode_ms: f64,
    last_steps: u32,
}

impl DebugStats {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            created: now, last_report: now, active: false, frames: 0,
            zero_step_frames: 0, steps: 0, capped_frames: 0,
            dropped_seconds: 0.0, cpu_encode_ms: 0.0, last_steps: 0,
        }
    }

    fn record(&mut self, steps: u32, dropped_seconds: f64, cpu_encode_ms: f64) {
        self.frames += 1;
        self.steps += u64::from(steps);
        self.zero_step_frames += if steps == 0 { 1 } else { 0 };
        self.capped_frames += if dropped_seconds > 0.0 { 1 } else { 0 };
        self.dropped_seconds += dropped_seconds;
        self.cpu_encode_ms += cpu_encode_ms;
        self.last_steps = steps;
    }

    fn reset_interval(&mut self, now: Instant) {
        self.last_report = now;
        self.frames = 0;
        self.zero_step_frames = 0;
        self.steps = 0;
        self.capped_frames = 0;
        self.dropped_seconds = 0.0;
        self.cpu_encode_ms = 0.0;
    }
}

/// Diagnostic estimate of the collision surface directly below the source.
/// Only used when r_riverDebug=2 at spawn; it never changes the simulation.
fn collision_heights_at(collision: &[jka_assets::bsp::river::CollisionSurface], x: f32, y: f32, level: f32) -> (Option<f32>, Option<f32>, usize) {
    let mut below: Option<f32> = None;
    let mut above: Option<f32> = None;
    let mut hits = 0;
    for surface in collision {
        // Match the actual riverbed selection: don't report a roof as a
        // candidate ground hit when its entire solid is above the water.
        if surface.solid_bottom_z.is_some_and(|z| z > level) { continue; }
        let tri = &surface.triangle;
        let a = tri[0]; let b = tri[1]; let c = tri[2];
        let det = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
        if !det.is_finite() || det.abs() < 1e-6 { continue; }
        let u = ((b[1] - c[1]) * (x - c[0]) + (c[0] - b[0]) * (y - c[1])) / det;
        let v = ((c[1] - a[1]) * (x - c[0]) + (a[0] - c[0]) * (y - c[1])) / det;
        if u < -1e-4 || v < -1e-4 || u + v > 1.0001 { continue; }
        let z = u * a[2] + v * b[2] + (1.0 - u - v) * c[2];
        if !z.is_finite() { continue; }
        hits += 1;
        if z <= level { below = Some(below.map_or(z, |old| old.max(z))); }
        else { above = Some(above.map_or(z, |old| old.min(z))); }
    }
    (below, above, hits)
}

use crate::river_gpu::{BoundaryInput, Simulation};
use crate::river_tiles::{Bins, SparseSimulation, Spawn, Window};
use jka_water::river::authoring::{Domain, Endpoint};
use jka_water::river::boundary::{Controls as BoundaryControls, SourceBox, WaterInfo};
use jka_water::river::capture::{Camera, Channel, Settings};
use jka_water::river::control::{Controls, Frame, Grid};
use jka_water::river::grid_mesh::GridMesh;
use jka_water::river::{Presentation, Transport, CENTIMETRES_PER_MAP_UNIT};

/// Authored inlet control, independent of whether the inlet has a flow limit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SourceMode {
    #[default]
    WaterLevel,
    Discharge,
}

/// One test source, in JKA map units (Z up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalSource {
    /// Where the water comes in (e.g. the crosshair hit point).
    pub center: [f32; 3],
    /// Half the side of the square simulated patch around `center`.
    pub radius: f32,
    /// Water surface height the source holds (the native box top).
    pub water_level: f32,
    /// Side of the square source box.
    pub source_width: f32,
    /// Native ResolutionMaxAxis: cells along the longer axis.
    pub resolution: u32,
    /// Simulated seconds per real second: 1 is real time, higher fast-fills.
    pub rate: f32,
    /// Which control the author selected. A water-level inlet may also carry
    /// an optional CFS cap; mode must not be inferred from `discharge_cfs`.
    pub mode: SourceMode,
    /// Explicit volume per simulated second. None keeps the native held-level
    /// source in WaterLevel mode.
    pub discharge_cfs: Option<f32>,
}

/// A developer drain (spawndrain / River menu), JKA map units. Not Unreal:
/// water standing above `level` inside the circle leaves the simulation,
/// optionally at a capped rate. Drains apply to every live river.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalDrain {
    /// Placement point (e.g. the crosshair hit).
    pub center: [f32; 3],
    /// Radius of the drain circle (map units).
    pub radius: f32,
    /// Overflow drain: the water surface height it lowers water to (map z).
    /// None is a floor drain that empties its cells completely.
    pub level: Option<f32>,
    /// Most water removed per simulated second, in CFS; None is unlimited.
    pub max_cfs: Option<f32>,
}

impl LocalDrain {
    pub fn validate(&self) -> Result<(), String> {
        if self.center.iter().chain(self.level.iter()).any(|v| !v.is_finite()) {
            return Err("drain position and level must be finite".into());
        }
        if !(self.radius.is_finite() && (1.0..=8192.0).contains(&self.radius)) {
            return Err("drain radius must be 1 to 8192 map units".into());
        }
        if self.max_cfs.is_some_and(|q| !(q.is_finite() && q > 0.0 && q <= 1_000_000.0)) {
            return Err("drain max flow must be above 0 and at most 1,000,000 CFS".into());
        }
        Ok(())
    }
}

/// Solver frames one rendered frame may run; a hitch drops the excess
/// instead of snowballing.
const MAX_STEPS_PER_FRAME: u32 = 32;

impl LocalSource {
    pub fn validate(self) -> Result<(), String> {
        let finite = self
            .center
            .iter()
            .chain([
                &self.radius,
                &self.water_level,
                &self.source_width,
                &self.rate,
            ])
            .all(|v| v.is_finite());
        if !finite
            || self.radius < 64.0
            || self.source_width <= 0.0
            || !(16..=1024).contains(&self.resolution)
            || !(self.rate > 0.0 && self.rate <= 100.0)
            || self
                .discharge_cfs
                .is_some_and(|q| !q.is_finite() || !(0.0..=1_000_000.0).contains(&q) || q == 0.0)
            || (self.mode == SourceMode::Discharge && self.discharge_cfs.is_none())
        {
            return Err(
                "invalid river source: radius >= 64, width > 0, resolution 16..1024, rate (0,100], discharge CFS (0,1000000] when enabled"
                    .into(),
            );
        }
        Ok(())
    }
}

/// Count source-covered cells using the same rotated source-box test as the GPU.
fn source_cell_count(grid: Grid, grid_min: [f32; 3], source: SourceBox) -> u32 {
    let (sin, cos) = source.angle.sin_cos();
    let mut count = 0;
    for y in 0..grid.dimensions[1] {
        for x in 0..grid.dimensions[0] {
            let local_x = grid_min[0] + (x as f32 + 0.5) * grid.cell_size[0] - source.position[0];
            let local_y = grid_min[1] + (y as f32 + 0.5) * grid.cell_size[1] - source.position[1];
            let local_z = grid_min[2] - source.position[2];
            let rotated_x = cos * local_x + sin * local_y;
            let rotated_y = -sin * local_x + cos * local_y;
            if rotated_x >= -0.5 * source.size[0] && rotated_x <= 0.5 * source.size[0]
                && rotated_y >= -0.5 * source.size[1] && rotated_y <= 0.5 * source.size[1]
                && local_z >= -0.5 * source.size[2] && local_z <= 0.5 * source.size[2]
            { count += 1; }
        }
    }
    count
}

/// Convert the requested volumetric rate into the depth added to each covered
/// source cell during one solver step.
fn source_rise_per_step_cm(cfs: f32, step_seconds: f64, cells: u32, cell_area_cm2: f64) -> Result<f32, String> {
    if cells == 0 { return Err("river source box covers no simulation cells; increase source width or grid resolution".into()); }
    let cm3_per_second = f64::from(cfs) * crate::river_discharge::CM3_PER_CUBIC_FOOT;
    let lift = cm3_per_second * step_seconds / (f64::from(cells) * cell_area_cm2);
    if !lift.is_finite() || lift <= 0.0 || lift > f64::from(f32::MAX) {
        return Err("river CFS cannot be represented at this grid resolution".into());
    }
    Ok(lift as f32)
}

pub struct LiveRiver {
    pub id: crate::river_authoring::Id,
    /// r_riverDebug: (menu deformation toggle, presented actors) last frame.
    pub body_source: (bool, usize),
    /// r_riverDebug: bodies used last frame and the first one (lattice cm).
    body_debug: (usize, Option<crate::river_tiles::CollisionBody>),
    /// Water-mesh quadtree tiles beyond the camera window (sparse only).
    far: Option<crate::river_far::FarMesh>,
    /// RenderControls last written to the solver (NaN: never).
    render_controls: [f32; 8],
    pub authored: wgpu::Buffer,
    foam_minimum_depth: std::cell::Cell<f32>,
    pub source: LocalSource,
    /// Native actor origin (XY centre, minimum Z), centimetres.
    pub origin_cm: [f64; 3],
    pub grid: Grid,
    inlet: SourceBox,
    controls: Controls,
    solver: Solver,
    material: Option<crate::river_material::Presentation>,
    pub mesh: crate::river_grid_mesh::Mesh,
    pub simulated_seconds: f64,
    /// Scaled real time not yet simulated (less than one solver frame).
    pending_seconds: f64,
    /// DiffuseIndirectLighting under the classic sky, built once at spawn.
    ambient: crate::river_slw::ClassicAmbient,
    /// Native FoamGrid (stages 4/16), created with the material fields.
    foam: Option<crate::river_foam::Field>,
    /// Live window blocks (river_blocks), rebuilt when SimGridRT is.
    blocks: Option<crate::river_blocks::Cull>,
    /// The water body's group for each FoamRT parity.
    body_groups: Option<[wgpu::BindGroup; 2]>,
    /// Whitewater (SW_River_Secondary_Emitter) particles, while r_riverWhitewater.
    spray: Option<crate::river_secondary::Emitter>,
    /// This frame's camera for the ViewRecycler; None skips the emitter.
    spray_view: Option<crate::river_secondary::SprayView>,
    /// Sprite group over the particles, with the surface's texture generation.
    spray_group: Option<(u32, wgpu::BindGroup)>,
    debug: DebugStats,
    /// Accelerator releases to 1x after steady hydraulic flow.
    settled: bool,
    convergence: Option<SolveConvergence>,
    /// Debug-only readback of GPU water and reconstructed bed near source.
    wet_spot_probe: Option<WetSpotProbe>,
}

impl LiveRiver {
    /// Replace this river's developer drains (at most river_tiles::MAX_DRAINS).
    /// Live: the bed, water and foam are kept.
    pub fn set_drains(&mut self, queue: &wgpu::Queue, drains: &[LocalDrain]) {
        let cm = f64::from(CENTIMETRES_PER_MAP_UNIT);
        let (origin, grid_min, dx) = match &self.solver {
            Solver::Dense(_) => (
                self.origin_cm,
                [-0.5 * f64::from(self.grid.world_size[0]), -0.5 * f64::from(self.grid.world_size[1])],
                f64::from(self.grid.cell_size[0]),
            ),
            Solver::Sparse(sparse) => (sparse.solver_origin_cm, [0.0; 2], f64::from(sparse.dx_cm)),
        };
        // The drain runs once per solver frame, which advances this many
        // hydraulic seconds (the CFS inlet's conversion).
        let seconds = Frame::new(Grid { cell_size: [dx as f32; 2], ..self.grid }, self.controls, 1.0 / 60.0)
            .map_or(1.0 / 60.0, Frame::hydraulic_duration);
        let local: Vec<jka_water::river::boundary::Drain> = drains
            .iter()
            .filter(|d| d.validate().is_ok())
            .map(|d| {
                let center = [f64::from(d.center[0]) * cm - origin[0], f64::from(d.center[1]) * cm - origin[1]];
                // At least one cell centre always falls inside.
                let radius = (f64::from(d.radius) * cm).max(0.75 * dx);
                let max_removal = d.max_cfs.map_or(0.0, |q| {
                    let mut cells = 0u32;
                    let range = |a: usize| {
                        let lo = ((center[a] - radius - grid_min[a]) / dx).floor() as i64;
                        let hi = ((center[a] + radius - grid_min[a]) / dx).ceil() as i64;
                        lo..=hi
                    };
                    for y in range(1) {
                        for x in range(0) {
                            let px = grid_min[0] + (x as f64 + 0.5) * dx - center[0];
                            let py = grid_min[1] + (y as f64 + 0.5) * dx - center[1];
                            cells += u32::from(px * px + py * py <= radius * radius);
                        }
                    }
                    (f64::from(q) * crate::river_discharge::CM3_PER_CUBIC_FOOT * seconds / (f64::from(cells.max(1)) * dx * dx)) as f32
                });
                jka_water::river::boundary::Drain {
                    center: center.map(|v| v as f32),
                    radius: radius as f32,
                    // Floor drain: a level far below any bed (the bed wins).
                    level: d.level.map_or(-1.0e9, |z| (f64::from(z) * cm - origin[2]) as f32),
                    max_removal,
                }
            })
            .collect();
        match &mut self.solver {
            Solver::Dense(simulation) => simulation.set_drains(queue, &local),
            Solver::Sparse(sparse) => sparse.simulation.set_drains(queue, &local),
        }
    }

    /// `riverdump` console diagnostics: a synchronous readback of the sparse
    /// pool plus the lattice corner (solver cm, XY) and the source box (for
    /// the CPU replay budget). Dense patches: Err.
    pub fn dump(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<(crate::river_tiles::PoolDump, [f64; 2], jka_water::river::boundary::SourceBox), String> {
        match &self.solver {
            Solver::Sparse(sparse) => Ok((
                sparse.simulation.dump_pool(device, queue)?,
                [sparse.solver_origin_cm[0], sparse.solver_origin_cm[1]],
                self.inlet,
            )),
            Solver::Dense(_) => Err("riverdump reads sparse rivers only (r_riverSparse 1, then place the river again)".into()),
        }
    }
}

/// The native single rectangle, or the sparse tile pool for long rivers.
enum Solver {
    Dense(Simulation),
    Sparse(Box<SparseRiver>),
}

/// Sparse solver plus the camera window the dense presentation chain reads.
struct SparseRiver {
    simulation: SparseSimulation,
    window: Window,
    /// Lattice corner XY and river Z (cm); solver positions are relative to it.
    solver_origin_cm: [f64; 3],
    dx_cm: f32,
    /// The window moved since the last gather.
    shifted: bool,
    /// The window holds at least one completed solver frame.
    presented: bool,
    throttled_frames: u64,
    /// Tests pin r_riverTileSchedule per river instead of the session cvar.
    schedule_override: Option<u8>,
}

impl SparseRiver {
    /// Presentation origin: the window centre, rebased in Z by its offset.
    fn window_origin_cm(&self) -> [f64; 3] {
        let cells = f64::from(self.window.cells());
        let o = self.window.origin_cell().unwrap_or([0, 0]);
        let dx = f64::from(self.dx_cm);
        [
            self.solver_origin_cm[0] + (f64::from(o[0]) + 0.5 * cells) * dx,
            self.solver_origin_cm[1] + (f64::from(o[1]) + 0.5 * cells) * dx,
            self.solver_origin_cm[2] + f64::from(self.window.height_offset_cm()),
        ]
    }

    fn cell_of(&self, map: [f32; 2]) -> [i32; 2] {
        let cm = f64::from(CENTIMETRES_PER_MAP_UNIT);
        std::array::from_fn(|a| ((f64::from(map[a]) * cm - self.solver_origin_cm[a]) / f64::from(self.dx_cm)).floor() as i32)
    }
}

/// Count lattice cells inside a source box, using the GPU's rotated-box test,
/// over a bounded rectangle (`grid_min` is the lattice corner, zero).
fn source_cell_count_in(dx: f32, source: SourceBox, rect: [i32; 4]) -> u32 {
    let (sin, cos) = source.angle.sin_cos();
    let mut count = 0;
    for y in rect[1]..rect[3] {
        for x in rect[0]..rect[2] {
            let local_x = (x as f32 + 0.5) * dx - source.position[0];
            let local_y = (y as f32 + 0.5) * dx - source.position[1];
            let local_z = -source.position[2];
            let rotated_x = cos * local_x + sin * local_y;
            let rotated_y = -sin * local_x + cos * local_y;
            if rotated_x >= -0.5 * source.size[0] && rotated_x <= 0.5 * source.size[0]
                && rotated_y >= -0.5 * source.size[1] && rotated_y <= 0.5 * source.size[1]
                && local_z >= -0.5 * source.size[2] && local_z <= 0.5 * source.size[2]
            { count += 1; }
        }
    }
    count
}

/// Classic lightgrid ambient for the presentation window at a quarter of its
/// resolution (the field is smooth and sampled bilinearly by UV).
fn window_ambient(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    origin_cm: [f64; 3],
    grid: Grid,
    level: f32,
    light: impl FnMut([f32; 3]) -> Option<jka_render_core::light::ClassicEntityLight>,
    map_toward_sun: Option<[f32; 3]>,
) -> Result<crate::river_slw::ClassicAmbient, String> {
    let n = (grid.dimensions[0] / 4).max(16);
    let size = grid.world_size.map(|v| v / CENTIMETRES_PER_MAP_UNIT);
    let cm = f64::from(CENTIMETRES_PER_MAP_UNIT);
    let centre = [(origin_cm[0] / cm) as f32, (origin_cm[1] / cm) as f32];
    let texels = crate::river_slw::classic_ambient_texels(
        [n, n],
        |x, y| [
            centre[0] + ((x as f32 + 0.5) / n as f32 - 0.5) * size[0],
            centre[1] + ((y as f32 + 0.5) / n as f32 - 0.5) * size[1],
            level,
        ],
        light,
        map_toward_sun,
    );
    crate::river_slw::ClassicAmbient::upload(device, queue, &texels)
}

// Keep ordinary map anchors intact, but avoid sending huge source heights to
// the RGBA16Float render textures. Solver and collision geometry remain F32.
fn river_renderable_origin_z(native_bottom_cm: f64, target_z_cm: f64) -> f64 {
    const MAX_SOURCE_HEIGHT_CM: f64 = 60_000.0;
    const SOURCE_ANCHOR_CM: f64 = 1_024.0;
    if target_z_cm - native_bottom_cm > MAX_SOURCE_HEIGHT_CM {
        target_z_cm - SOURCE_ANCHOR_CM
    } else {
        native_bottom_cm
    }
}

impl LiveRiver {
    /// Capture the patch's collision bed, place the source and create the
    /// solver. All GPU preparation is submitted once here; no readback.
    /// `light` samples the classic lightgrid at a JKA point (None: no data)
    /// and `map_toward_sun` is its baked sun, stripped from every sample.
    /// `capture_geometry` is the map's `r_riverCaptureMode 1` collision; it is
    /// used only while that mode is on (read here, at placement).
    pub fn spawn(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        collision: &[jka_assets::bsp::river::CollisionSurface],
        capture_geometry: Option<&jka_assets::bsp::river::CaptureGeometry>,
        source: LocalSource,
        light: impl FnMut([f32; 3]) -> Option<jka_render_core::light::ClassicEntityLight>,
        map_toward_sun: Option<[f32; 3]>,
    ) -> Result<Self, String> {
        source.validate()?;
        let capture_geometry = match (crate::river_reach::capture_mode(), capture_geometry) {
            (0, _) => None,
            (_, Some(geometry)) => Some(geometry),
            (_, None) => return Err("r_riverCaptureMode 1 has no capture geometry for this map; place the river again".into()),
        };
        if crate::river_tiles::sparse_enabled() {
            return Self::spawn_sparse(device, queue, collision, capture_geometry, source, light, map_toward_sun);
        }
        let debug = debug_mode();
        let started = (debug >= 2).then(Instant::now);
        let cm = f64::from(CENTIMETRES_PER_MAP_UNIT);
        let [cx, cy, cz] = source.center.map(f64::from);
        let r = f64::from(source.radius);
        // Mode 1 classifies over the very XY grid the Domain below gets (its
        // grid depends only on the XY bounds and the max-axis resolution).
        let classified = match capture_geometry {
            Some(geometry) => {
                let xy = Domain::from_bounds(
                    [(cx - r) * cm, (cy - r) * cm, cz * cm - 1.0],
                    [(cx + r) * cm, (cy + r) * cm, cz * cm + 1.0],
                    source.resolution,
                )?;
                let size = xy.grid.world_size.map(|v| f64::from(v) / cm);
                let region = crate::river_reach::Region {
                    min: [xy.origin[0] / cm - 0.5 * size[0], xy.origin[1] / cm - 0.5 * size[1]],
                    cell: f64::from(xy.grid.cell_size[0]) / cm,
                    dims: xy.grid.dimensions,
                };
                let seed = [source.center[0], source.center[1], source.center[2] + 2.0];
                let classified = crate::river_reach::classify(geometry, region, seed, source.water_level)?;
                if debug > 0 {
                    let s = classified.stats;
                    eprintln!("[RIVER DBG CAPTURE MODE 1] dense items={} columns={} reached={} void={} stacked={} actors={} landscape_tris={} actor_tris={} classify_cpu={:.2}ms",
                        s.items, s.columns, s.reached, s.voids, s.stacked, s.actor_items, s.landscape_triangles, s.actor_triangles, s.millis);
                }
                Some(classified)
            }
            None => None,
        };
        // Include tops of grounded obstacles while excluding entire overhead brushes.
        let (mut lo, mut hi) = (
            cz.min(f64::from(source.water_level)),
            cz.max(f64::from(source.water_level)),
        );
        let overlaps = |t: &[[f32; 3]; 3]| (0..2).all(|axis| t.iter().map(|p| p[axis]).fold(f32::INFINITY, f32::min) <= source.center[axis] + source.radius
            && t.iter().map(|p| p[axis]).fold(f32::NEG_INFINITY, f32::max) >= source.center[axis] - source.radius);
        if let Some(classified) = &classified {
            // The reachable bed's heights; actors only widen the cameras.
            for t in classified.landscape.iter().filter(|t| overlaps(t)) {
                for p in t { lo = lo.min(f64::from(p[2])); hi = hi.max(f64::from(p[2])); }
            }
        } else {
            let mut excluded_overhead = 0usize;
            let mut kept_high_obstacles = 0usize;
            for surface in collision {
                let t = &surface.triangle;
                if !overlaps(t) { continue; }
                if surface.solid_bottom_z.is_some_and(|z| z > source.water_level)
                    || (surface.solid_bottom_z.is_none() && t.iter().all(|p| p[2] > source.water_level))
                { excluded_overhead += 1; continue; }
                if surface.solid_bottom_z.is_some() && t.iter().any(|p| p[2] > source.water_level) { kept_high_obstacles += 1; }
                for p in t { lo = lo.min(f64::from(p[2])); hi = hi.max(f64::from(p[2])); }
            }
            if debug >= 2 { eprintln!("[RIVER DBG COLLISION] overhead_surfaces_excluded={} grounded_above_level_kept={}", excluded_overhead, kept_high_obstacles); }
        }
        let mut domain = Domain::from_bounds(
            [(cx - r) * cm, (cy - r) * cm, (lo - 8.0) * cm],
            [(cx + r) * cm, (cy + r) * cm, (hi + 8.0) * cm],
            source.resolution,
        )?;
        let native_bottom_cm = domain.origin[2];
        domain.origin[2] = river_renderable_origin_z(
            native_bottom_cm,
            f64::from(source.water_level) * cm,
        );
        if debug > 0 && native_bottom_cm != domain.origin[2] {
            eprintln!("[RIVER DBG FP16] rebase_z={:.1} -> {:.1} map units; source_local_height={:.1}cm; solver/collision retained",
                native_bottom_cm / cm, domain.origin[2] / cm,
                f64::from(source.water_level) * cm - domain.origin[2]);
        }
        let grid = domain.grid;
        let width = f64::from(source.source_width) * cm;
        let source_box: SourceBox = domain.boundary(
            Endpoint {
                position: [cx * cm, cy * cm, f64::from(source.water_level) * cm],
                tangent: [0.0; 3],
                width: width as f32,
            },
            false,
            width as f32,
        )?;
        let origin_map = domain.origin.map(|v| (v / cm) as f32);
        let half = grid.world_size.map(|v| 0.5 * v / CENTIMETRES_PER_MAP_UNIT);
        let capture = match &classified {
            Some(c) => BspCapture::from_classified(&c.landscape, &c.actors, origin_map, half)?,
            None => BspCapture::from_collision_for_source(collision, origin_map, half, source.water_level, grid.cell_size[0] / CENTIMETRES_PER_MAP_UNIT)?,
        };
        let [bottom, top] = capture.vertical_bounds();
        if debug > 0 {
            let triangles = capture.meshes().iter().map(|m| m.indices.len() / 3).sum::<usize>();
            eprintln!("[RIVER DBG SPAWN] at=({:.1},{:.1},{:.1}) level={:.1} width={:.1} radius={:.1} grid={}x{} cell={:.1}u collision_total={} captured={} captured_z=[{:.1},{:.1}] source_level_minus_top={:.1}u",
                source.center[0], source.center[1], source.center[2], source.water_level,
                source.source_width, source.radius, grid.dimensions[0], grid.dimensions[1],
                grid.cell_size[0] / CENTIMETRES_PER_MAP_UNIT, collision.len(), triangles,
                f64::from(bottom) / cm + domain.origin[2] / cm,
                f64::from(top) / cm + domain.origin[2] / cm,
                f64::from(source.water_level) - (f64::from(top) / cm + domain.origin[2] / cm));
            if debug >= 2 {
                eprintln!("[RIVER DBG CAPTURE] water_level={:.1}u: grounded obstacles retained; overhead brushes excluded; terrain triangles clipped", source.water_level);
                let (below, above, hits) = collision_heights_at(collision, source.center[0], source.center[1], source.water_level);
                eprintln!("[RIVER DBG BED] triangle_hits_under_source={} highest_bed_below_level={:?} nearest_geometry_above_level={:?} spawn_cpu_ms={:.2}", hits, below, above, started.map_or(0.0, |s| s.elapsed().as_secs_f64() * 1000.0));
                let mut below_samples = 0u8; let mut above_samples = 0u8; let mut missing_samples = 0u8;
                let mut min_clearance = f32::INFINITY; let mut max_clearance = f32::NEG_INFINITY;
                let offset = source.source_width * 0.35;
                for dy in [-offset, 0.0, offset] { for dx in [-offset, 0.0, offset] {
                    let (local_below, local_above, _) = collision_heights_at(collision, source.center[0] + dx, source.center[1] + dy, source.water_level);
                    if let Some(bed) = local_below { below_samples += 1; let clearance = source.water_level - bed; min_clearance = min_clearance.min(clearance); max_clearance = max_clearance.max(clearance); } else { missing_samples += 1; }
                    if local_above.is_some() { above_samples += 1; }
                }}
                eprintln!("[RIVER DBG FOOTPRINT] below_level_bed={}/9 missing_bed={}/9 overhead_geometry={}/9 clearance_range={:?}..{:?}u source_cells_across={:.2} box_top={:.1}u",
                    below_samples, missing_samples, above_samples, (below_samples > 0).then_some(min_clearance), (below_samples > 0).then_some(max_clearance), source.source_width * CENTIMETRES_PER_MAP_UNIT / grid.cell_size[0], source.water_level);
            }
        }
        let headroom = crate::river_tiles::CAPTURE_HEADROOM_CM;
        let cameras = std::array::from_fn(|i| Camera {
            height: if Channel::ALL[i].top_down() {
                top + headroom
            } else {
                bottom - headroom
            },
            near: 0.0,
            far: top - bottom + 2.0 * headroom,
        });
        let settings = Settings {
            dimensions: grid.dimensions,
            world_size: grid.world_size,
            capture_width: grid.world_size[0].max(grid.world_size[1]),
            dx: grid.cell_size[0],
            bottom_depth: bottom - headroom,
            dilation: 0.0,
            cameras,
        };
        let count = (grid.dimensions[0] * grid.dimensions[1]) as usize;
        let info = vec![WaterInfo::default(); count];
        // Unreal's SimSpeed 10 advances a fixed 1/6 s of water per rendered
        // frame, whatever the frame rate. Speed 1 keeps the native timestep and
        // limits for a 1/60 s solver frame; encode_frame paces those frames to
        // real time (times `rate`).
        let controls = Controls {
            speed: 1.0,
            ..Controls::default()
        };
        let frame = Frame::new(grid, controls, 1.0 / 60.0)?;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("local river spawn"),
        });
        let mut depths = DepthCaptures::new(
            device,
            grid.dimensions[0].max(grid.dimensions[1]),
            settings.capture_width,
            cameras,
        )?;
        depths.encode_capture(device, &mut encoder, &capture.meshes())?;
        let bed = match &classified {
            None => depths.encode_prepare(device, &mut encoder, settings, &info)?,
            Some(c) => {
                // Unreal's stage 6 compares actor undersides with the river
                // spline's WaterInfo height; mode 1 gives it the reachable-floor
                // reference. Only bed selection reads it: the solver keeps the
                // dry field, whose identity the prepared bed then carries.
                let [w, _] = grid.dimensions;
                let reference: Vec<WaterInfo> = (0..count)
                    .map(|i| WaterInfo {
                        height: c.references.local_cm(
                            [(i % w as usize) as i64, (i / w as usize) as i64],
                            domain.origin[2],
                            settings.floor(),
                        ),
                        ..WaterInfo::default()
                    })
                    .collect();
                let mut bed = depths.encode_prepare(device, &mut encoder, settings, &reference)?;
                bed.water_info_hash = crate::river_water_info::Field::identity_of(&info);
                bed
            }
        };
        // The void (no captured geometry) is a wall, never water.
        crate::river_tiles::encode_void_walls(device, &mut encoder, &bed.geometry, count as u32, settings.floor(), cameras[0].height);
        let mut boundary_controls =
            BoundaryControls::cached_river_variant(1.0 / 60.0, domain.grid_min());
        boundary_controls.match_spline = false;
        if let (SourceMode::WaterLevel, Some(cfs)) = (source.mode, source.discharge_cfs) {
            let cells = source_cell_count(grid, domain.grid_min(), source_box);
            let step_seconds = frame.hydraulic_duration();
            let cell_area_cm2 = f64::from(grid.cell_size[0]) * f64::from(grid.cell_size[1]);
            boundary_controls.source_rise_limit_cm = source_rise_per_step_cm(cfs, step_seconds, cells, cell_area_cm2)?;
            if debug > 0 {
                eprintln!("[RIVER DBG FLOW] mode=height+cfs target_z={:.1}u limit={:.3} CFS active_cells={} max_rise={:.3}cm/step sim_rate={}x",
                    source.water_level, cfs, cells, boundary_controls.source_rise_limit_cm, source.rate);
            }
        }
        // Standalone discharge uses the existing volumetric GPU adapter instead
        // of a held-level box. Only Water Height mode enforces an elevation cap.
        let mut simulation = Simulation::from_prepared_bed(
            device,
            bed,
            frame.parameters(),
            crate::river_tiles::spawn_transport(),
            Presentation::default(),
            BoundaryInput {
                water_info: &info,
                // Retain the actual box even when inactive, so switching back
                // to height mode can use it without a rebuild.
                boxes: std::slice::from_ref(&source_box),
                controls: boundary_controls,
            },
        )?;
        simulation.set_height_source(source.mode == SourceMode::WaterLevel, boundary_controls.source_rise_limit_cm);
        if source.mode == SourceMode::Discharge {
            let cfs = source.discharge_cfs.expect("validated discharge mode has a CFS rate");
            simulation.enable_discharge(
                device, &mut encoder, grid, width as f32,
                (f64::from(source.water_level) * cm - domain.origin[2]) as f32,
                cfs,
            );
            if debug > 0 {
                eprintln!("[RIVER DBG FLOW] mode=discharge inlet_z={:.1}u rate={:.3} CFS; no water-height clamp",
                    source.water_level, cfs);
            }
        }
        let convergence = (source.rate != 1.0).then(|| SolveConvergence::new(device, &simulation));
        let mesh = crate::river_grid_mesh::encode(
            device,
            &mut encoder,
            GridMesh::new(grid.dimensions, grid.world_size)?,
        )?;
        queue.submit(Some(encoder.finish()));
        // Lightgrid at each cell centre, at the source water level while the
        // surface is still being solved.
        let [w, h] = grid.dimensions;
        let size = grid.world_size.map(|v| v / CENTIMETRES_PER_MAP_UNIT);
        let texels = crate::river_slw::classic_ambient_texels(
            grid.dimensions,
            |x, y| {
                [
                    origin_map[0] + ((x as f32 + 0.5) / w as f32 - 0.5) * size[0],
                    origin_map[1] + ((y as f32 + 0.5) / h as f32 - 0.5) * size[1],
                    source.water_level,
                ]
            },
            light,
            map_toward_sun,
        );
        let ambient = crate::river_slw::ClassicAmbient::upload(device, queue, &texels)?;
        use wgpu::util::DeviceExt;
        let authored = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("river independent material"),
            contents: bytemuck::cast_slice(&crate::river_settings::Settings::default().uniform()),
            usage: wgpu::BufferUsages::UNIFORM
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
        });
        Ok(Self {
            id: 0,
            body_source: (false, 0),
            body_debug: (0, None),
            far: None,
            render_controls: [f32::NAN; 8],
            authored,
            foam_minimum_depth: std::cell::Cell::new(60.0),
            source,
            origin_cm: domain.origin,
            grid,
            inlet: source_box,
            controls,
            solver: Solver::Dense(simulation),
            material: None,
            mesh,
            simulated_seconds: 0.0,
            pending_seconds: 0.0,
            ambient,
            foam: None,
            blocks: None,
            body_groups: None,
            spray: None,
            spray_view: None,
            spray_group: None,
            debug: DebugStats::new(),
            settled: false,
            convergence,
            wet_spot_probe: None,
        })
    }

    /// Long-distance river: the same native stages over resident tiles of a
    /// lattice covering the riverbed candidates of the map (river_tiles.rs).
    /// The authored radius/resolution only set the native cell size; water
    /// may then flow as far as the bed allows. Presentation reads a dense
    /// camera window gathered from the pool.
    fn spawn_sparse(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        collision: &[jka_assets::bsp::river::CollisionSurface],
        capture_geometry: Option<&jka_assets::bsp::river::CaptureGeometry>,
        source: LocalSource,
        light: impl FnMut([f32; 3]) -> Option<jka_render_core::light::ClassicEntityLight>,
        map_toward_sun: Option<[f32; 3]>,
    ) -> Result<Self, String> {
        let debug = debug_mode();
        let started = (debug >= 1).then(Instant::now);
        let cm = f64::from(CENTIMETRES_PER_MAP_UNIT);
        let [cx, cy, _] = source.center.map(f64::from);
        // ResolutionMaxAxis across the authored square: the dense cell size.
        let side = (2.0 * f64::from(source.radius) * cm) as f32;
        let dx_cm = Grid::max_axis([side, side], source.resolution)?.cell_size[0];
        let tile = crate::river_tiles::tile_size();
        let guard = crate::river_tiles::guard_tiles();
        let bins = match capture_geometry {
            None => Bins::new(collision, source.water_level, f64::from(dx_cm) / cm, tile, [cx, cy])?,
            Some(geometry) => {
                let seed = [source.center[0], source.center[1], source.center[2] + 2.0];
                let (bins, s) = Bins::new_classified(geometry, seed, source.water_level, f64::from(dx_cm) / cm, tile, [cx, cy])?;
                if debug > 0 {
                    eprintln!("[RIVER DBG CAPTURE MODE 1] sparse items={} columns={} reached={} void={} stacked={} actors={} landscape_tris={} actor_tris={} classify_cpu={:.2}ms",
                        s.items, s.columns, s.reached, s.voids, s.stacked, s.actor_items, s.landscape_triangles, s.actor_triangles, s.millis);
                }
                bins
            }
        };
        let [zmin, zmax] = bins.z_range();
        let native_bottom_cm = (f64::from(zmin.min(source.water_level)) - 8.0) * cm;
        let origin_z = river_renderable_origin_z(native_bottom_cm, f64::from(source.water_level) * cm);
        let min_map = bins.min_map();
        let solver_origin = [min_map[0] * cm, min_map[1] * cm, origin_z];
        let lattice = bins.lattice();
        let dims = lattice.dimensions();
        let lattice_grid = Grid {
            dimensions: dims,
            world_size: dims.map(|n| n as f32 * dx_cm),
            cell_size: [dx_cm; 2],
        };
        let controls = Controls { speed: 1.0, ..Controls::default() };
        let frame = Frame::new(lattice_grid, controls, 1.0 / 60.0)?;
        // The native Rebuild source-box conversion, local to the lattice corner.
        let top_cm = f64::from(zmax.max(source.water_level)) * cm + 8.0 * cm;
        let domain = Domain {
            origin: solver_origin,
            vertical_extent: ((top_cm - origin_z) * 0.5).max(1.0),
            grid: lattice_grid,
        };
        let width = f64::from(source.source_width) * cm;
        let source_box: SourceBox = domain.boundary(
            Endpoint {
                position: [cx * cm, cy * cm, f64::from(source.water_level) * cm],
                tangent: [0.0; 3],
                width: width as f32,
            },
            false,
            width as f32,
        )?;
        let centre_cm = [source_box.position[0], source_box.position[1]];
        let reach = (0.5 * width as f32 / dx_cm).ceil() as i32 + 2;
        let centre_cell = centre_cm.map(|v| (v / dx_cm).floor() as i32);
        let source_cells = [
            centre_cell[0] - reach,
            centre_cell[1] - reach,
            centre_cell[0] + reach + 1,
            centre_cell[1] + reach + 1,
        ];
        let mut boundary_controls = BoundaryControls::cached_river_variant(1.0 / 60.0, [0.0; 3]);
        boundary_controls.match_spline = false;
        if let (SourceMode::WaterLevel, Some(cfs)) = (source.mode, source.discharge_cfs) {
            let cells = source_cell_count_in(dx_cm, source_box, source_cells);
            boundary_controls.source_rise_limit_cm =
                source_rise_per_step_cm(cfs, frame.hydraulic_duration(), cells, f64::from(dx_cm) * f64::from(dx_cm))?;
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("sparse river spawn") });
        let mut simulation = SparseSimulation::new(
            device,
            &mut encoder,
            Spawn {
                bins,
                dx_cm,
                origin_z_cm: origin_z,
                parameters: frame.parameters(),
                substeps: frame.substeps(),
                transport: crate::river_tiles::spawn_transport(),
                presentation: Presentation::default(),
                boxes: std::slice::from_ref(&source_box),
                controls: boundary_controls,
                source_cells,
                guard,
                storage: crate::river_tiles::Storage::selected(),
            },
        )?;
        simulation.set_height_source(source.mode == SourceMode::WaterLevel, boundary_controls.source_rise_limit_cm);
        let level_cm = (f64::from(source.water_level) * cm - origin_z) as f32;
        if source.mode == SourceMode::Discharge {
            let cfs = source.discharge_cfs.expect("validated discharge mode has a CFS rate");
            let rect = simulation.inlet_rect(centre_cm, width as f32);
            simulation.enable_inlet(device, rect, level_cm, cfs);
        }
        let cells = (crate::river_tiles::window_cells() / tile).max(4) * tile;
        let mut window = Window::new(device, cells, simulation.storage());
        window.follow(&simulation, centre_cell);
        let grid = window.grid(dx_cm);
        let mesh = crate::river_grid_mesh::encode(device, &mut encoder, GridMesh::new(grid.dimensions, grid.world_size)?)?;
        queue.submit(Some(encoder.finish()));
        let sparse = SparseRiver {
            simulation,
            window,
            solver_origin_cm: solver_origin,
            dx_cm,
            shifted: true,
            presented: false,
            throttled_frames: 0,
            schedule_override: None,
        };
        let origin_cm = sparse.window_origin_cm();
        let ambient = window_ambient(device, queue, origin_cm, grid, source.water_level, light, map_toward_sun)?;
        if debug > 0 {
            let tiles = lattice.tiles();
            eprintln!("[RIVER DBG SPARSE] at=({:.1},{:.1},{:.1}) level={:.1} cell={:.2}u tile={} cells guard={} lattice={}x{} tiles ({}x{} cells) resident={} window={}x{} cells bed_z=[{:.1},{:.1}] origin_z={:.1}u spawn_cpu={:.2}ms",
                source.center[0], source.center[1], source.center[2], source.water_level,
                f64::from(dx_cm) / cm, tile, guard, tiles[0], tiles[1], dims[0], dims[1],
                sparse.simulation.resident_tiles(), cells, cells, zmin, zmax, origin_z / cm,
                started.map_or(0.0, |s| s.elapsed().as_secs_f64() * 1000.0));
        }
        use wgpu::util::DeviceExt;
        let authored = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("river independent material"),
            contents: bytemuck::cast_slice(&crate::river_settings::Settings::default().uniform()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        });
        Ok(Self {
            id: 0,
            body_source: (false, 0),
            body_debug: (0, None),
            far: None,
            render_controls: [f32::NAN; 8],
            authored,
            foam_minimum_depth: std::cell::Cell::new(60.0),
            source,
            origin_cm,
            grid,
            inlet: source_box,
            controls,
            solver: Solver::Sparse(Box::new(sparse)),
            material: None,
            mesh,
            simulated_seconds: 0.0,
            pending_seconds: 0.0,
            ambient,
            foam: None,
            blocks: None,
            body_groups: None,
            spray: None,
            spray_view: None,
            spray_group: None,
            debug: DebugStats::new(),
            settled: false,
            // Steady-state detection reads the dense state; sparse rivers keep
            // the authored rate (still bounded by the residency margin).
            convergence: None,
            wet_spot_probe: None,
        })
    }

    /// Sparse rivers: keep the presentation window around the camera. A move
    /// rebuilds the window ambient and drops foam history and body bindings,
    /// which are tied to the window placement. Dense rivers ignore this.
    pub fn far_mesh(&self) -> Option<&crate::river_far::FarMesh> {
        self.far.as_ref().filter(|_| crate::river_far::enabled())
    }

    /// The Water plugin's water mesh beyond the camera window: rebuild the
    /// quadtree if the wet tiles or the window changed, traverse for this
    /// view and upload. Sparse rivers only.
    pub fn prepare_far(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        water: &crate::river_surface::RiverSurface,
        view: &crate::river_far::FarView<'_>,
        light: &mut dyn FnMut([f32; 3]) -> Option<jka_render_core::light::ClassicEntityLight>,
        map_toward_sun: Option<[f32; 3]>,
    ) {
        let Solver::Sparse(sparse) = &self.solver else { return };
        if !crate::river_far::enabled() || !sparse.presented {
            return;
        }
        let simulation = &sparse.simulation;
        let far = self.far.get_or_insert_with(|| crate::river_far::FarMesh::new(device, simulation.lattice().tile()));
        let (table, surface, normals) = simulation.presentation_buffers();
        let bins = simulation.bins();
        let tile_min_z = |t: [i32; 2]| bins.min_z(t, t);
        let inputs = crate::river_far::FarInputs {
            wet_tiles: simulation.wet_tiles(),
            wet_generation: simulation.wet_generation(),
            pool_generation: simulation.pool_generation(),
            table,
            surface,
            normals,
            normals_packed: simulation.storage().packed_normals(),
            lattice: simulation.lattice(),
            dx_cm: simulation.dx_cm(),
            solver_origin_cm: sparse.solver_origin_cm,
            window: sparse.window.origin_cell().map(|o| (o, sparse.window.cells())),
            tile_min_z: &tile_min_z,
            water_level: self.source.water_level,
        };
        far.prepare(device, queue, water.far_layout(), &self.authored, &inputs, view, light, map_toward_sun);
    }

    /// Moving player hulls for the PreSim collision stage (stage 5), map
    /// units: absolute box corners and velocity in units/s. Sparse rivers
    /// only; the native single rectangle keeps its static colliders. Bodies
    /// outside the lattice are dropped; at most MAX_BODIES are used.
    /// `velocity_scale` multiplies the body velocity (1 = native).
    pub fn set_bodies(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, bodies: &[MapBody], velocity_scale: f32) {
        let Solver::Sparse(sparse) = &mut self.solver else { return };
        let cm = f64::from(CENTIMETRES_PER_MAP_UNIT);
        let dims = sparse.simulation.lattice().dimensions();
        let size = dims.map(|n| f64::from(n) * f64::from(sparse.dx_cm));
        let margin = 64.0 * cm;
        let origin = sparse.solver_origin_cm;
        let converted: Vec<crate::river_tiles::CollisionBody> = bodies
            .iter()
            .filter(|b| b.mins.iter().chain(&b.maxs).chain(&b.velocity).all(|v| v.is_finite()))
            .map(|b| crate::river_tiles::CollisionBody {
                centre: std::array::from_fn(|a| ((f64::from(b.mins[a]) + f64::from(b.maxs[a])) * 0.5 * cm - origin[a]) as f32),
                extent: std::array::from_fn(|a| (b.maxs[a] - b.mins[a]).abs() * CENTIMETRES_PER_MAP_UNIT),
                velocity: b.velocity.map(|v| v * CENTIMETRES_PER_MAP_UNIT),
            })
            .filter(|b| (0..2).all(|a| f64::from(b.centre[a]) > -margin && f64::from(b.centre[a]) < size[a] + margin))
            .take(crate::river_tiles::MAX_BODIES)
            .collect();
        self.body_debug = (converted.len(), converted.first().copied());
        // r_riverAdaptiveMesh: wake blocks keep the tile resolution.
        if let (Some(blocks), Some(window)) = (self.blocks.as_mut(), sparse.window.origin_cell()) {
            let dx = sparse.dx_cm;
            let margin = crate::river_blocks::WAKE_MARGIN_CELLS;
            let cell = |v: f32, a: usize| (v / dx).floor() as i32 - window[a];
            blocks.set_wake(queue, converted.iter().map(|b| {
                let lo = |a: usize| cell(b.centre[a] - 0.5 * b.extent[a] - margin * dx, a);
                let hi = |a: usize| cell(b.centre[a] + 0.5 * b.extent[a] + margin * dx, a);
                [lo(0), lo(1), hi(0), hi(1)]
            }));
        }
        sparse.simulation.set_bodies(device, queue, &converted, velocity_scale);
    }

    pub fn update_view(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        eye_map: [f32; 3],
        light: impl FnMut([f32; 3]) -> Option<jka_render_core::light::ClassicEntityLight>,
        map_toward_sun: Option<[f32; 3]>,
    ) {
        let Solver::Sparse(sparse) = &mut self.solver else { return };
        if !eye_map.iter().all(|v| v.is_finite()) {
            return;
        }
        let eye_cell = sparse.cell_of([eye_map[0], eye_map[1]]);
        if !sparse.window.follow(&sparse.simulation, eye_cell) {
            return;
        }
        self.origin_cm = sparse.window_origin_cm();
        sparse.shifted = true;
        self.foam = None;
        self.body_groups = None;
        match window_ambient(device, queue, self.origin_cm, self.grid, self.source.water_level, light, map_toward_sun) {
            Ok(ambient) => self.ambient = ambient,
            Err(error) => eprintln!("[RIVER] window lighting failed: {error}"),
        }
        if debug_mode() > 0 {
            eprintln!("[RIVER DBG SPARSE] window moved to cell {:?}; height offset {:.1}cm", sparse.window.origin_cell(), sparse.window.height_offset_cm());
        }
    }

    /// The native frames (sources, substeps, surface/normals) that real time
    /// times `rate` has accumulated, the live material fields when the water
    /// moved, and foam stage 16 every frame on the scaled clock. A zero or
    /// negative delta (paused) records nothing. `world_time` is the material
    /// clock in seconds.
    pub fn encode_frame(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        engine_delta: f32,
        world_time: f64,
        water: Option<&crate::river_surface::RiverSurface>,
        solve_timestamp: &mut dyn FnMut(&mut wgpu::CommandEncoder, bool),
    ) -> Result<(), String> {
        // Live render-height controls (River menu A/B).
        let presentation = Presentation {
            smoothing_method: u32::from(smoothing_method()),
            height_cutoff: smoothing_cutoff_cm(),
            ..Presentation::default()
        };
        let words = crate::river_gpu::render_control_words(presentation, bed_clamp());
        if words != self.render_controls {
            match &mut self.solver {
                Solver::Dense(simulation) => simulation.set_render_controls(device, encoder, &words),
                Solver::Sparse(sparse) => sparse.simulation.set_render_controls(device, encoder, &words),
            }
            self.render_controls = words;
        }
        let diag = debug_mode();
        if diag < 2 || !matches!(self.solver, Solver::Dense(_)) {
            self.wet_spot_probe = None;
        } else {
            if self.wet_spot_probe.is_none() {
                self.wet_spot_probe = Some(WetSpotProbe::new(device, self.grid.dimensions));
            }
            if let Some(probe) = self.wet_spot_probe.as_mut() {
                probe.poll(device, self.origin_cm[2], self.source.water_level);
            }
        }
        let became_stable = self.convergence.as_mut().is_some_and(|probe| probe.poll(device, self.source.rate, diag));
        if became_stable {
            self.settled = true;
            self.convergence = None;
            eprintln!("[RIVER SOLVE] STABLE: {}x solve speed -> 1x real-time flow; solver stays active", self.source.rate);
        }
        if diag == 0 { self.debug.active = false; }
        let start = (diag >= 2).then(Instant::now);
        if !(engine_delta > 0.0) || !engine_delta.is_finite() {
            if diag > 0 { self.debug.record(0, 0.0, 0.0); }
            return Ok(());
        }
        let effective_rate = if self.settled { 1.0 } else { self.source.rate };
        let scaled = engine_delta.min(0.1) * effective_rate;
        // Sinks read the engine delta: give each solver frame its own span.
        let step = Frame::new(self.grid, self.controls, scaled)?.hydraulic_duration();
        let frame = Frame::new(self.grid, self.controls, step as f32)?;
        // Paused: no solver frames and no simulated time; presentation below
        // still runs on the last solved state.
        let paused = paused();
        let available = if paused { 0.0 } else { self.pending_seconds + f64::from(scaled) };
        let limit = step * f64::from(MAX_STEPS_PER_FRAME);
        let dropped = (available - limit).max(0.0);
        self.pending_seconds = available.min(limit);
        let mut moved = false;
        let mut steps = 0u32;
        if let Solver::Sparse(sparse) = &mut self.solver {
            // Completed occupancy samples grow residency before any new frame.
            sparse.simulation.update_residency(device, encoder)?;
            // r_riverTileSchedule A/B, live: exact at any solver-frame boundary.
            let mode = sparse.schedule_override.unwrap_or_else(crate::river_tiles::tile_schedule);
            sparse.simulation.set_tile_schedule(device, mode);
        }
        solve_timestamp(encoder, false);
        while !paused && self.pending_seconds >= step {
            match &mut self.solver {
                Solver::Dense(simulation) => simulation.encode_reference_frame(device, encoder, frame)?,
                Solver::Sparse(sparse) => {
                    // Never outrun the proven guard margin; wait for residency.
                    if sparse.simulation.frames_available() == 0 {
                        sparse.throttled_frames += 1;
                        break;
                    }
                    sparse.simulation.encode_reference_frame(device, encoder, frame)?;
                }
            }
            self.pending_seconds -= step;
            self.simulated_seconds += step;
            steps += 1;
            moved = true;
        }
        solve_timestamp(encoder, true);
        let mut shifted = false;
        match &mut self.solver {
            Solver::Dense(simulation) => {
                if moved {
                    if let Some(probe) = self.convergence.as_mut() {
                        probe.encode_sample(encoder, simulation, self.simulated_seconds);
                    }
                    if let Some(probe) = self.wet_spot_probe.as_mut() {
                        probe.encode_sample(encoder, simulation);
                    }
                }
            }
            Solver::Sparse(sparse) => {
                if moved {
                    sparse.simulation.encode_occupancy(device, encoder);
                }
                shifted = std::mem::take(&mut sparse.shifted);
                if (moved || shifted) && sparse.simulation.started() {
                    let profile = sparse.simulation.profile_slot("gather_window");
                    sparse.window.encode_gather(device, encoder, &sparse.simulation, profile);
                    sparse.presented = true;
                } else if shifted {
                    sparse.shifted = true;
                }
            }
        }
        if self.material.is_none() {
            let fields = match &self.solver {
                Solver::Dense(simulation) => simulation.output_fields(),
                Solver::Sparse(sparse) => sparse.presented.then(|| sparse.window.fields()),
            };
            if let Some(fields) = fields {
                self.material = Some(crate::river_material::Presentation::new(device, &fields)?);
            }
        }
        let Some(material) = &mut self.material else {
            if diag > 0 {
                self.debug.record(steps, dropped, start.map_or(0.0, |s| s.elapsed().as_secs_f64() * 1000.0));
            }
            return Ok(());
        };
        let surface_written = moved || shifted || material.textures().is_none();
        if surface_written {
            material.encode(encoder);
        }
        // Live window blocks over the SimGridRT just written (river_blocks).
        if let (Some(water), Some(textures)) = (water, material.textures()) {
            if self.blocks.is_none() {
                let cell_cm = self.mesh.geometry.size()[0] / self.mesh.geometry.cells()[0].max(1) as f32;
                self.blocks = Some(crate::river_blocks::Cull::new(device, water.block_pipelines(), &textures.surface, self.mesh.geometry.cells(), cell_cm)?);
            }
            if let Some(blocks) = self.blocks.as_mut() {
                blocks.encode(water.block_pipelines(), device, encoder, surface_written);
            }
        }
        if self.foam.is_none() {
            use jka_water::river::foam_field::{native_sim_res, Placement, Setup};
            let placement = Placement::unrotated(self.origin_cm, &self.grid)?;
            let world = self.grid.world_size.map(f64::from);
            let resolution = match &self.solver {
                Solver::Dense(_) => self.source.resolution as i32,
                Solver::Sparse(sparse) => sparse.window.cells() as i32,
            };
            let setup = Setup::native(&self.grid, placement, native_sim_res(world, resolution));
            let state = match &self.solver {
                Solver::Dense(simulation) => simulation.state_textures(),
                Solver::Sparse(sparse) => sparse.window.state_textures(),
            };
            let inputs = crate::river_foam::FieldInputs {
                state,
                normal_grid: material.normal_grid(),
                blocks: self.blocks.as_ref().map(|blocks| crate::river_foam::FoamBlocks { flags: blocks.flags(), grid: blocks.grid() }),
            };
            let mut field = crate::river_foam::Field::new(device, inputs, &setup)?;
            field.encode_reset(encoder);
            self.foam = Some(field);
        }
        let foam = self.foam.as_mut().expect("created above");
        let frame = jka_water::river::foam_field::Frame::new(scaled, world_time);
        foam.set_minimum_depth(self.foam_minimum_depth.get() * CENTIMETRES_PER_MAP_UNIT);
        let state_index = match &self.solver {
            Solver::Dense(simulation) => simulation.state_index(),
            Solver::Sparse(_) => 0,
        };
        foam.encode_frame(device, encoder, state_index, frame)?;
        // Whitewater (SW_River_Secondary_Emitter) over the foam stage's reader
        // fields, on the real frame clock (particles are presentation).
        if !whitewater() {
            self.spray = None;
            self.spray_group = None;
        } else if let Some(view) = self.spray_view {
            let spray = self.spray.get_or_insert_with(|| crate::river_secondary::Emitter::new(device));
            if !spray.is_bound() {
                let state = match &self.solver {
                    Solver::Dense(simulation) => simulation.state_textures(),
                    Solver::Sparse(sparse) => sparse.window.state_textures(),
                };
                spray.bind(device, crate::river_secondary::Inputs { state, normal_grid: material.normal_grid() });
            }
            spray.set_minimum_depth(self.foam_minimum_depth.get() * CENTIMETRES_PER_MAP_UNIT);
            let resolution = match &self.solver {
                Solver::Dense(_) => self.source.resolution as i32,
                Solver::Sparse(sparse) => sparse.window.cells() as i32,
            };
            let placement = crate::river_secondary::Placement {
                origin_cm: self.origin_cm,
                world_extent: self.grid.world_size,
                sim_res: jka_water::river::foam_field::native_sim_res(self.grid.world_size.map(f64::from), resolution),
                dx: self.grid.cell_size[0],
            };
            spray.encode(device, encoder, state_index, placement, view, engine_delta.min(0.1))?;
            if let Some(water) = water {
                let generation = water.spray_generation();
                if self.spray_group.as_ref().map(|(g, _)| *g) != Some(generation) {
                    self.spray_group = Some((generation, water.spray_group(device, spray.particles())));
                }
            }
        }
        if self.body_groups.is_none() {
            if let (Some(water), Some(textures), Some(blocks)) = (water, material.textures(), self.blocks.as_ref()) {
                let (pair, _) = foam.foam_pair();
                self.body_groups = Some([0, 1].map(|i| {
                    water.body_group(
                        device,
                        self.origin_cm,
                        self.mesh.geometry,
                        textures,
                        &self.ambient,
                        &pair[i],
                        &self.authored,
                        &blocks.blocks,
                    )
                }));
            }
        }
        if diag > 0 { self.debug.record(steps, dropped, start.map_or(0.0, |s| s.elapsed().as_secs_f64() * 1000.0)); }
        Ok(())
    }

    /// Once-per-second progress counters without GPU readback or synchronization.
    pub fn debug_report(&mut self, index: usize, mode: u8, copyable: bool, pipeline_ready: bool) {
        if mode == 0 { self.debug.active = false; return; }
        let now = Instant::now();
        if !self.debug.active { self.debug.active = true; self.debug.reset_interval(now); }
        else if now.duration_since(self.debug.last_report) < Duration::from_secs(1) { return; }
        let age = now.duration_since(self.debug.created).as_secs_f64();
        let interval = now.duration_since(self.debug.last_report).as_secs_f64().max(0.001);
        let body_ready = self.body_groups.is_some();
        let status = if !copyable { "NOT DRAWN: scene not copyable" } else if !pipeline_ready { "NOT DRAWN: scene copy/pipeline unavailable" } else if !body_ready { "NOT DRAWN: material/foam/body bindings not ready" } else { "DRAW SUBMITTED (GPU depth/visibility unknown)" };
        eprintln!("[RIVER DBG] patch={} at=({:.0},{:.0},{:.0}) age={:.1}s sim={:.2}s pending={:.4}s rate={}x solve_target={}x solve_state={} frames={} steps={} ({:.1}/s) zero_step={} capped={} dropped_sim={:.3}s material={} foam={} body={} status={}", index, self.source.center[0], self.source.center[1], self.source.center[2], age, self.simulated_seconds, self.pending_seconds, if self.settled { 1.0 } else { self.source.rate }, self.source.rate, if self.settled { "NORMAL" } else if self.convergence.is_some() { "SOLVING" } else { "NORMAL" }, self.debug.frames, self.debug.steps, self.debug.steps as f64 / interval, self.debug.zero_step_frames, self.debug.capped_frames, self.debug.dropped_seconds, self.material.is_some(), self.foam.is_some(), body_ready, status);
        if let Solver::Sparse(sparse) = &self.solver {
            let tiles = sparse.simulation.lattice().tiles();
            eprintln!("[RIVER DBG SPARSE] patch={} resident_tiles={} queued={} lattice={}x{} solver_frames={} frames_available={} throttled_render_frames={} window_cell={:?} pool={:.1}MiB storage={} ({}B/cell, {} slots allocated, {} evicted) window={:.1}MiB", index, sparse.simulation.resident_tiles(), sparse.simulation.queued_tiles(), tiles[0], tiles[1], sparse.simulation.solver_frames(), sparse.simulation.frames_available(), sparse.throttled_frames, sparse.window.origin_cell(), sparse.simulation.logical_bytes() as f64 / 1048576.0, sparse.simulation.storage().name(), sparse.simulation.storage().bytes_per_cell(), sparse.simulation.pool_capacity(), sparse.simulation.evicted_tiles(), sparse.window.logical_bytes() as f64 / 1048576.0);
            if let Some(line) = sparse.simulation.tile_schedule_report() {
                eprintln!("[RIVER DBG SCHEDULE] patch={} {}", index, line);
            }
        }
        if let Solver::Sparse(sparse) = &self.solver {
            let level_cm = f64::from(self.source.water_level) * f64::from(CENTIMETRES_PER_MAP_UNIT) - sparse.solver_origin_cm[2];
            let dims = sparse.simulation.lattice().dimensions();
            if let Some(far) = &self.far { let st = far.stats; eprintln!("[RIVER DBG WATERMESH] patch={} enabled={} leaves={} nodes={} depth={} instances={} per_density={:?} rebuilds={}", index, crate::river_far::enabled(), st.leaves, st.nodes, st.tree_depth, st.instances, far.draws.iter().map(|d| d.1).collect::<Vec<_>>(), st.rebuilds); }
            eprintln!("[RIVER DBG BODIES] patch={} deformation={} actors={} used={} first={:?} first_cell={:?} lattice={}x{} source_level_cm={:.0} dx_cm={:.1}", index, self.body_source.0, self.body_source.1, self.body_debug.0, self.body_debug.1, self.body_debug.1.map(|b| [(b.centre[0] / sparse.dx_cm).floor() as i32, (b.centre[1] / sparse.dx_cm).floor() as i32]), dims[0], dims[1], level_cm, sparse.dx_cm);
        }
        if mode >= 2 { eprintln!("[RIVER DBG PERF] patch={} CPU_encode={:.3}ms/s {:.4}ms/render-frame last_solver_steps={} grid={}x{} estimated_solver_kernel_dispatches={}", index, self.debug.cpu_encode_ms / interval, self.debug.cpu_encode_ms / self.debug.frames.max(1) as f64, self.debug.last_steps, self.grid.dimensions[0], self.grid.dimensions[1], self.debug.steps * (4 + u64::from(self.controls.substeps) * 6)); }
        self.debug.reset_interval(now);
    }

    pub fn set_settings(&self, queue: &wgpu::Queue, settings: crate::river_settings::Settings) {
        self.foam_minimum_depth.set(settings.foam_minimum_depth);
        queue.write_buffer(&self.authored, 0, bytemuck::cast_slice(&settings.uniform()));
    }

    /// This frame's camera for the whitewater ViewRecycler (None: not run).
    pub fn set_spray_view(&mut self, view: Option<crate::river_secondary::SprayView>) {
        self.spray_view = view;
    }

    /// The whitewater sprite group once particles exist (r_riverWhitewater).
    pub fn spray_group(&self) -> Option<&wgpu::BindGroup> {
        self.spray.as_ref().filter(|spray| spray.ready())?;
        self.spray_group.as_ref().map(|(_, group)| group)
    }

    /// The window grid's live block list and indirect draw (river_blocks).
    pub fn block_cull(&self) -> Option<&crate::river_blocks::Cull> {
        self.blocks.as_ref()
    }

    /// The water body's group for the current FoamRT, once the fields exist.
    pub fn body_group(&self) -> Option<&wgpu::BindGroup> {
        let (_, current) = self.foam.as_ref()?.foam_pair();
        self.body_groups.as_ref().map(|groups| &groups[current])
    }

    /// Coarse admission only; the shader checks the solved bed, wet mask and
    /// exact displaced mesh triangle before applying any submerged effect.
    pub fn contains_eye_xy(&self, camera: &jka_render_core::camera::Camera) -> bool {
        let eye = jka_render_core::coordinates::jka_position(camera.position.to_array());
        (0..2).all(|axis| {
            let center = self.origin_cm[axis] / f64::from(CENTIMETRES_PER_MAP_UNIT);
            let half = f64::from(self.mesh.geometry.size()[axis]) / (2.0 * f64::from(CENTIMETRES_PER_MAP_UNIT));
            (f64::from(eye[axis]) - center).abs() < half
        })
    }

    pub fn set_live_controls(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, controls: crate::river_authoring::LiveControls) -> Result<(), String> {
        let source = controls.apply(self.source)?;
        if source == self.source { return Ok(()); }
        let cm = f64::from(CENTIMETRES_PER_MAP_UNIT);
        let frame = Frame::new(self.grid, self.controls, 1.0 / 60.0)?;
        let mut rise_limit = 0.0;
        let area = f64::from(self.grid.cell_size[0]) * f64::from(self.grid.cell_size[1]);
        let cfs = if source.mode == SourceMode::Discharge { source.discharge_cfs } else { None };
        match &mut self.solver {
            Solver::Dense(simulation) => {
                if let (SourceMode::WaterLevel, Some(q)) = (source.mode, source.discharge_cfs) {
                    let grid_min = [-self.grid.world_size[0] * 0.5, -self.grid.world_size[1] * 0.5, 0.0];
                    let cells = source_cell_count(self.grid, grid_min, self.inlet);
                    rise_limit = source_rise_per_step_cm(q, frame.hydraulic_duration(), cells, area)?;
                }
                simulation.set_height_source(source.mode == SourceMode::WaterLevel, rise_limit);
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("river live inlet controls") });
                simulation.update_discharge(device, &mut encoder, self.grid,
                    source.source_width * CENTIMETRES_PER_MAP_UNIT,
                    (f64::from(source.water_level) * cm - self.origin_cm[2]) as f32,
                    cfs);
                queue.submit(Some(encoder.finish()));
            }
            Solver::Sparse(sparse) => {
                let dx = sparse.dx_cm;
                let centre = [self.inlet.position[0], self.inlet.position[1]];
                let reach = (0.5 * source.source_width * CENTIMETRES_PER_MAP_UNIT / dx).ceil() as i32 + 2;
                let cell = centre.map(|v| (v / dx).floor() as i32);
                if let (SourceMode::WaterLevel, Some(q)) = (source.mode, source.discharge_cfs) {
                    let rect = [cell[0] - reach, cell[1] - reach, cell[0] + reach + 1, cell[1] + reach + 1];
                    let cells = source_cell_count_in(dx, self.inlet, rect);
                    rise_limit = source_rise_per_step_cm(q, frame.hydraulic_duration(), cells, area)?;
                }
                sparse.simulation.set_height_source(source.mode == SourceMode::WaterLevel, rise_limit);
                match cfs {
                    Some(q) if !sparse.simulation.has_inlet() => {
                        let rect = sparse.simulation.inlet_rect(centre, source.source_width * CENTIMETRES_PER_MAP_UNIT);
                        let level = (f64::from(source.water_level) * cm - sparse.solver_origin_cm[2]) as f32;
                        sparse.simulation.enable_inlet(device, rect, level, q);
                    }
                    other => sparse.simulation.set_inlet(other),
                }
            }
        }
        self.source = source;
        self.settled = false;
        let Solver::Dense(simulation) = &self.solver else {
            self.convergence = None;
            return Ok(());
        };
        if source.rate != 1.0 {
            if self.convergence.is_none() {
                self.convergence = Some(SolveConvergence::new(device, simulation));
            }
            let probe = self.convergence.as_mut().expect("created above");
            probe.restart(self.simulated_seconds);
        } else {
            self.convergence = None;
        }
        Ok(())
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn state(&self) -> &wgpu::Texture {
        match &self.solver {
            Solver::Dense(simulation) => simulation.state(),
            Solver::Sparse(sparse) => &sparse.window.state_textures()[0],
        }
    }

    /// SimGridRT (height/depth/velocity, cm) and NormalsRT once available.
    pub fn surface(&self) -> Option<&crate::river_material::Textures> {
        self.material.as_ref().and_then(|m| m.textures())
    }
}

#[cfg(test)]
mod source_cfs_tests {
    use super::*;

    #[test]
    fn local_river_rebases_only_when_source_height_exceeds_fp16_range() {
        let cm = f64::from(CENTIMETRES_PER_MAP_UNIT);
        let target = 7_880.125 * cm;
        let extreme_bottom = -56_968.0 * cm;
        let rebased = river_renderable_origin_z(extreme_bottom, target);
        assert!((target - rebased - 1_024.0).abs() < 1e-6);
        assert!(target - rebased < 65_504.0);
        let ordinary_bottom = 7_000.0 * cm;
        assert_eq!(river_renderable_origin_z(ordinary_bottom, target), ordinary_bottom);
    }

    #[test]
    fn requested_discharge_is_distributed_across_source_cells() {
        // 1 CFS for 1s over 100 square centimetres * 10 cells.
        let lift = source_rise_per_step_cm(1.0, 1.0, 10, 100.0).unwrap();
        assert!((f64::from(lift) * 10.0 * 100.0
            - crate::river_discharge::CM3_PER_CUBIC_FOOT).abs() < 0.01);
        assert!(source_rise_per_step_cm(1.0, 1.0, 0, 100.0).is_err());
    }
}

#[cfg(test)]
#[path = "river_sparse_gpu_tests.rs"]
mod sparse_gpu_tests;
