//! Fluid Ninja lake wake for the Source lake renderer (backend 3): one
//! player-following window of the private dense-water solver (`jka_ninja::LakeField`,
//! authority Water_Dense_Lake) over the lake the camera is on, driven by the
//! shared actor wake (`river_ninja::wake_points`). The lake shader samples its
//! velocity/pressure for normals and the Lake material's velocity foam.
//! Source lakes are flat planes, so contact is a CPU band test against the plane.
use jka_ninja::{LakeDesc, LakeField, LakeFrame, WakeTrack, WAKE_MAX_TRACKS};
use jka_river::river_ninja::{wake_points, ActorHistory, ActorSample, WakePoint, timeline_epoch};
use super::{OceanMasks, OCEAN_MAX_SURFACES, OCEAN_UNITS_PER_METER};

/// Window side (m) and solver cells: ~10 cm texels, Lake's per-texel step kept
/// by the solver's scale-invariant path.
const EXTENT_M: f32 = 50.0;
const RESOLUTION: u32 = 512;
const CM_PER_UNIT: f32 = 100.0 / OCEAN_UNITS_PER_METER;
/// The camera must be within this many map units of a lake plane to run the field.
const PLANE_REACH: f32 = 800.0;

/// One candidate lake: pool index, plane height and render-space XZ bounds.
pub(super) struct LakePool { pub index: usize, pub plane: f32, pub bounds: [f32; 4] }

pub(super) struct NinjaLake {
    field: Option<LakeField>,
    unavailable: bool,
    history: ActorHistory,
    epoch: u64,
    last_time: Option<f32>,
    /// Active pool and plane; a change rebuilds the field.
    pool: Option<(usize, f32)>,
    /// Absolute window centre in field cells (JKA x/y).
    origin_cells: [i32; 2],
    flags: wgpu::Buffer,
    params: wgpu::Buffer,
    tracks: Vec<WakeTrack>,
    flag_data: Vec<u32>,
    points: Vec<WakePoint>,
    active: bool,
}

impl NinjaLake {
    pub fn new(device: &wgpu::Device) -> Self {
        let buffer = |label, size, usage| device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label), size, usage, mapped_at_creation: false });
        Self {
            field: None, unavailable: false, history: ActorHistory::default(),
            epoch: timeline_epoch(), last_time: None, pool: None, origin_cells: [0; 2],
            flags: buffer("Ninja lake contact flags", (WAKE_MAX_TRACKS * 4) as u64,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST),
            params: buffer("Ninja lake wake window",
                std::mem::size_of::<blue_water_gpu::lakes::NinjaLakeWakeUniform>() as u64,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST),
            tracks: Vec::new(), flag_data: Vec::new(), points: Vec::new(), active: false,
        }
    }

    /// Stop drawing and forget the simulation (setting off, map change, no lake near).
    pub fn idle(&mut self, queue: &wgpu::Queue) {
        if self.active || self.field.is_some() {
            queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&blue_water_gpu::lakes::NinjaLakeWakeUniform::default()));
        }
        self.active = false;
        self.field = None;
        self.pool = None;
        self.history.reset();
        self.last_time = None;
    }

    /// Advance the field for this frame. `eye` is the camera in render space
    /// (x, y up, z). Returns the views for the lake bindings while drawing.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder,
        time: f32, eye: [f32; 3], actors: &[ActorSample], pools: &[LakePool], masks: &OceanMasks,
        assets: &super::lakes::NinjaLakeAssets,
    ) -> Option<(wgpu::TextureView, wgpu::TextureView, wgpu::TextureView, &wgpu::Buffer)> {
        if self.unavailable { return None; }
        // The lake under or around the camera, nearest plane first.
        let margin = EXTENT_M * OCEAN_UNITS_PER_METER * 0.5;
        let pool = pools.iter()
            .filter(|p| p.index < OCEAN_MAX_SURFACES && (eye[1] - p.plane).abs() <= PLANE_REACH
                && eye[0] >= p.bounds[0] - margin && eye[0] <= p.bounds[2] + margin
                && eye[2] >= p.bounds[1] - margin && eye[2] <= p.bounds[3] + margin)
            .min_by(|a, b| (eye[1] - a.plane).abs().total_cmp(&(eye[1] - b.plane).abs()));
        let Some(pool) = pool else { self.idle(queue); return None; };
        let epoch = timeline_epoch();
        let delta = self.last_time.map_or(0.0, |t| f64::from(time - t));
        let reset = self.epoch != epoch || !delta.is_finite() || !(0.0..=0.5).contains(&delta)
            || self.pool != Some((pool.index, pool.plane));
        self.last_time = Some(time);
        self.epoch = epoch;
        if reset { self.field = None; self.history.reset(); }
        self.pool = Some((pool.index, pool.plane));
        // Window centre in JKA cm (x, y = -render z).
        let eye_cm = [eye[0] * CM_PER_UNIT, -eye[2] * CM_PER_UNIT];
        if self.field.is_none() {
            let sim = &assets.simulation;
            let desc = LakeDesc { resolution: RESOLUTION, extent_cm: EXTENT_M * 100.0, max_tracks: WAKE_MAX_TRACKS as u32 };
            match LakeField::new(device, &desc, sim.each_ref()) {
                Ok(field) => {
                    let cell = field.cell_cm();
                    self.origin_cells = eye_cm.map(|v| (v / cell).round() as i32);
                    self.field = Some(field);
                }
                Err(e) => { eprintln!("[LAKE NINJA] wake unavailable: {e}"); self.unavailable = true; return None; }
            }
        }
        let field = self.field.as_mut().expect("created above");
        let cell = field.cell_cm();
        let target = eye_cm.map(|v| (v / cell).round() as i32);
        let shift = [target[0] - self.origin_cells[0], target[1] - self.origin_cells[1]];
        if shift != [0, 0] {
            field.shift(shift);
            self.origin_cells = target;
        }
        let centre = self.origin_cells.map(|c| c as f32 * cell);
        // Shared wake points, relative to the window centre, z from the plane.
        self.history.update(actors.iter().copied());
        self.tracks.clear();
        self.flag_data.clear();
        let half_cm = EXTENT_M * 50.0;
        let plane = pool.plane;
        for track in self.history.tracks() {
            self.points.clear();
            wake_points(track, &mut self.points);
            for point in &self.points {
                let rel = |q: [f32; 3]| [q[0] * CM_PER_UNIT - centre[0], q[1] * CM_PER_UNIT - centre[1], (q[2] - plane) * CM_PER_UNIT];
                let position = rel(point.position_map);
                if position[0].abs().max(position[1].abs()) >= half_cm { continue; }
                if self.tracks.len() >= WAKE_MAX_TRACKS { break; }
                let q = point.position_map;
                // Flat plane: the point's contact band straddles it, over this pool's water.
                let wet = point.mins_map[2] <= plane && plane <= point.maxs_map[2]
                    && masks.contains(pool.index, [q[0], -q[1]]);
                self.tracks.push(point.track(rel));
                self.flag_data.push(u32::from(wet));
            }
        }
        if !self.flag_data.is_empty() {
            queue.write_buffer(&self.flags, 0, bytemuck::cast_slice(&self.flag_data));
        }
        if let Err(e) = field.encode(device, queue, encoder, &LakeFrame { game_delta: delta, tracks: &self.tracks,
            // The window follows the player: wakes are relative to its motion (UE pawn-attached sim).
            frame_velocity_cm_s: jka_river::river_ninja::wake_owner_velocity_cm_s(self.history.tracks()),
            contacts: Some(&self.flags) }) {
            eprintln!("[LAKE NINJA] wake disabled: {e}");
            self.unavailable = true;
            self.idle(queue);
            return None;
        }
        let o = field.outputs();
        let (Some(vd), Some(pd)) = (o.velocity_density, o.pressure_divergence) else {
            return None;
        };
        let (vd, pd) = (vd.clone(), pd.clone());
        let uniform = blue_water_gpu::lakes::NinjaLakeWakeUniform {
            window: [centre[0], centre[1], 1.0 / (EXTENT_M * 100.0), 1.0],
            params: [0.0; 4],
        };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&uniform));
        self.active = true;
        let flow = assets.flow.clone();
        Some((vd, pd, flow, &self.params))
    }
}
