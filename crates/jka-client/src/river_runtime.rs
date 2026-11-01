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
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

// Session-only console cvar. Both the app and render threads read this; with
// mode 0 the simulation performs no diagnostic allocation, logging or timing.
static RIVER_DEBUG: AtomicU8 = AtomicU8::new(0);

pub(crate) fn debug_mode() -> u8 {
    RIVER_DEBUG.load(Ordering::Relaxed)
}

pub(crate) fn set_debug_mode(mode: u8) {
    RIVER_DEBUG.store(mode.min(2), Ordering::Relaxed);
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
use jka_water::river::authoring::{Domain, Endpoint};
use jka_water::river::boundary::{Controls as BoundaryControls, SourceBox, WaterInfo};
use jka_water::river::capture::{Camera, Channel, Settings};
use jka_water::river::control::{Controls, Frame, Grid};
use jka_water::river::grid_mesh::GridMesh;
use jka_water::river::{Presentation, Transport, CENTIMETRES_PER_MAP_UNIT};

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
    /// Explicit volume per simulated second; None keeps the native held-level source.
    pub discharge_cfs: Option<f32>,
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
        {
            return Err(
                "invalid river source: radius >= 64, width > 0, resolution 16..1024, rate 0..100"
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
    pub authored: wgpu::Buffer,
    pub source: LocalSource,
    /// Native actor origin (XY centre, minimum Z), centimetres.
    pub origin_cm: [f64; 3],
    pub grid: Grid,
    controls: Controls,
    simulation: Simulation,
    material: Option<crate::river_material::Presentation>,
    pub mesh: crate::river_grid_mesh::Mesh,
    pub simulated_seconds: f64,
    /// Scaled real time not yet simulated (less than one solver frame).
    pending_seconds: f64,
    /// DiffuseIndirectLighting under the classic sky, built once at spawn.
    ambient: crate::river_slw::ClassicAmbient,
    /// Native FoamGrid (stages 4/16), created with the material fields.
    foam: Option<crate::river_foam::Field>,
    /// The water body's group for each FoamRT parity.
    body_groups: Option<[wgpu::BindGroup; 2]>,
    debug: DebugStats,
    /// Accelerator releases to 1x after steady hydraulic flow.
    settled: bool,
    convergence: Option<SolveConvergence>,
    /// Debug-only readback of GPU water and reconstructed bed near source.
    wet_spot_probe: Option<WetSpotProbe>,
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
    pub fn spawn(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        collision: &[jka_assets::bsp::river::CollisionSurface],
        source: LocalSource,
        light: impl FnMut([f32; 3]) -> Option<crate::scene::ClassicEntityLight>,
        map_toward_sun: Option<[f32; 3]>,
    ) -> Result<Self, String> {
        source.validate()?;
        let debug = debug_mode();
        let started = (debug >= 2).then(Instant::now);
        let cm = f64::from(CENTIMETRES_PER_MAP_UNIT);
        let [cx, cy, cz] = source.center.map(f64::from);
        let r = f64::from(source.radius);
        // Include tops of grounded obstacles while excluding entire overhead brushes.
        let (mut lo, mut hi) = (
            cz.min(f64::from(source.water_level)),
            cz.max(f64::from(source.water_level)),
        );
        let mut excluded_overhead = 0usize;
        let mut kept_high_obstacles = 0usize;
        for surface in collision {
            let t = &surface.triangle;
            let overlaps = (0..2).all(|axis| t.iter().map(|p| p[axis]).fold(f32::INFINITY, f32::min) <= source.center[axis] + source.radius
                && t.iter().map(|p| p[axis]).fold(f32::NEG_INFINITY, f32::max) >= source.center[axis] - source.radius);
            if !overlaps { continue; }
            if surface.solid_bottom_z.is_some_and(|z| z > source.water_level)
                || (surface.solid_bottom_z.is_none() && t.iter().all(|p| p[2] > source.water_level))
            { excluded_overhead += 1; continue; }
            if surface.solid_bottom_z.is_some() && t.iter().any(|p| p[2] > source.water_level) { kept_high_obstacles += 1; }
            for p in t { lo = lo.min(f64::from(p[2])); hi = hi.max(f64::from(p[2])); }
        }
        if debug >= 2 { eprintln!("[RIVER DBG COLLISION] overhead_surfaces_excluded={} grounded_above_level_kept={}", excluded_overhead, kept_high_obstacles); }
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
        let capture = BspCapture::from_collision_for_source(collision, origin_map, half, source.water_level)?;
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
        let headroom = 100.0;
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
        let bed = depths.encode_prepare(device, &mut encoder, settings, &info)?;
        let mut boundary_controls =
            BoundaryControls::cached_river_variant(1.0 / 60.0, domain.grid_min());
        boundary_controls.match_spline = false;
        if let Some(cfs) = source.discharge_cfs {
            let cells = source_cell_count(grid, domain.grid_min(), source_box);
            let step_seconds = frame.hydraulic_duration();
            let cell_area_cm2 = f64::from(grid.cell_size[0]) * f64::from(grid.cell_size[1]);
            boundary_controls.source_rise_limit_cm = source_rise_per_step_cm(cfs, step_seconds, cells, cell_area_cm2)?;
            if debug > 0 {
                eprintln!("[RIVER DBG FLOW] mode=height+cfs target_z={:.1}u limit={:.3} CFS active_cells={} max_rise={:.3}cm/step sim_rate={}x",
                    source.water_level, cfs, cells, boundary_controls.source_rise_limit_cm, source.rate);
            }
        }
        let simulation = Simulation::from_prepared_bed(
            device,
            bed,
            frame.parameters(),
            Transport::default(),
            Presentation::default(),
            BoundaryInput {
                water_info: &info,
                boxes: std::slice::from_ref(&source_box),
                controls: boundary_controls,
            },
        )?;
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
            authored,
            source,
            origin_cm: domain.origin,
            grid,
            controls,
            simulation,
            material: None,
            mesh,
            simulated_seconds: 0.0,
            pending_seconds: 0.0,
            ambient,
            foam: None,
            body_groups: None,
            debug: DebugStats::new(),
            settled: false,
            convergence,
            wet_spot_probe: None,
        })
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
    ) -> Result<(), String> {
        let diag = debug_mode();
        if diag < 2 {
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
        let available = self.pending_seconds + f64::from(scaled);
        let limit = step * f64::from(MAX_STEPS_PER_FRAME);
        let dropped = (available - limit).max(0.0);
        self.pending_seconds = available.min(limit);
        let mut moved = false;
        let mut steps = 0u32;
        while self.pending_seconds >= step {
            self.simulation
                .encode_reference_frame(device, encoder, frame)?;
            self.pending_seconds -= step;
            self.simulated_seconds += step;
            steps += 1;
            moved = true;
        }
        if moved {
            if let Some(probe) = self.convergence.as_mut() {
                probe.encode_sample(encoder, &self.simulation, self.simulated_seconds);
            }
            if let Some(probe) = self.wet_spot_probe.as_mut() {
                probe.encode_sample(encoder, &self.simulation);
            }
        }
        if self.material.is_none() {
            if let Some(fields) = self.simulation.output_fields() {
                self.material = Some(crate::river_material::Presentation::new(device, &fields)?);
            }
        }
        let Some(material) = &mut self.material else {
            if diag > 0 {
                self.debug.record(steps, dropped, start.map_or(0.0, |s| s.elapsed().as_secs_f64() * 1000.0));
            }
            return Ok(());
        };
        if moved || material.textures().is_none() {
            material.encode(encoder);
        }
        if self.foam.is_none() {
            use jka_water::river::foam_field::{native_sim_res, Placement, Setup};
            let placement = Placement::unrotated(self.origin_cm, &self.grid)?;
            let world = self.grid.world_size.map(f64::from);
            let setup = Setup::native(
                &self.grid,
                placement,
                native_sim_res(world, self.source.resolution as i32),
            );
            let inputs = crate::river_foam::FieldInputs {
                state: self.simulation.state_textures(),
                normal_grid: material.normal_grid(),
            };
            let mut field = crate::river_foam::Field::new(device, inputs, &setup)?;
            field.encode_reset(encoder);
            self.foam = Some(field);
        }
        let foam = self.foam.as_mut().expect("created above");
        let frame = jka_water::river::foam_field::Frame::new(scaled, world_time);
        foam.encode_frame(device, encoder, self.simulation.state_index(), frame)?;
        if self.body_groups.is_none() {
            if let (Some(water), Some(textures)) = (water, material.textures()) {
                let (pair, _) = foam.foam_pair();
                self.body_groups = Some([0, 1].map(|i| {
                    water.body_group(
                        device,
                        self.origin_cm,
                        textures,
                        &self.ambient,
                        &pair[i],
                        &self.authored,
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
        if mode >= 2 { eprintln!("[RIVER DBG PERF] patch={} CPU_encode={:.3}ms/s {:.4}ms/render-frame last_solver_steps={} grid={}x{} estimated_solver_kernel_dispatches={}", index, self.debug.cpu_encode_ms / interval, self.debug.cpu_encode_ms / self.debug.frames.max(1) as f64, self.debug.last_steps, self.grid.dimensions[0], self.grid.dimensions[1], self.debug.steps * (4 + u64::from(self.controls.substeps) * 6)); }
        self.debug.reset_interval(now);
    }

    pub fn set_settings(&self, queue: &wgpu::Queue, settings: crate::river_settings::Settings) {
        queue.write_buffer(&self.authored, 0, bytemuck::cast_slice(&settings.uniform()));
    }

    /// The water body's group for the current FoamRT, once the fields exist.
    pub fn body_group(&self) -> Option<&wgpu::BindGroup> {
        let (_, current) = self.foam.as_ref()?.foam_pair();
        self.body_groups.as_ref().map(|groups| &groups[current])
    }

    #[cfg(test)]
    pub fn state(&self) -> &wgpu::Texture {
        self.simulation.state()
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
