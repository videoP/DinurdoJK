//! Nonblocking actor for visual cloth. The worker exclusively owns the XPBD
//! state; cgame owns the current animation and only consumes secondary offsets.
//! No solver lock, thread join or channel receive may stall the render thread.
use super::cloth::{
    ClothCapsule, ClothConfig, ClothMotion, ClothOutput, ClothRenderTopology, ClothSurfaceFrame,
    ClothSystem,
};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

const QUEUE_CAPACITY: usize = 32;
/// A late solver result is still far better than snapping back to the
/// authored (unsimulated) garment for a frame: that alternation showed both
/// shapes at once under temporal AA. Only a long stall drops the result.
const MAX_RESULT_AGE_MS: i32 = 750;
/// Report a solve slower than this many fixed ticks (developer 1).
const SLOW_SOLVE_TICKS: f32 = 1.5;

struct Request {
    epoch: u64,
    entity: u16,
    model: String,
    lod: usize,
    surfaces: Vec<ClothSurfaceFrame>,
    capsules: Vec<ClothCapsule>,
    motion: ClothMotion,
    config: ClothConfig,
    wind: [f32; 3],
    time_ms: i32,
}

struct ResultFrame {
    epoch: u64,
    entity: u16,
    model: String,
    lod: usize,
    /// Request (presentation) time; used for staleness.
    time_ms: i32,
    /// Time the solver state actually represents: the request time minus the
    /// fixed-tick remainder not yet integrated. Interpolating by request time
    /// stalls and jumps whenever a request lands between ticks (frame rate and
    /// solver rate beating against each other).
    sim_time_ms: f64,
    // Store *secondary displacement* rather than the stale animation pose.
    // These displacements are applied to the CURRENT Ghoul2 vertex positions.
    offsets: HashMap<usize, Vec<[f32; 3]>>,
    topology: Option<Arc<ClothRenderTopology>>,
}

struct FramePair {
    previous: Option<ResultFrame>,
    latest: ResultFrame,
    /// Smoothed delay between a state's simulated time and its arrival here.
    /// Rendering that far behind (plus one result interval) keeps the
    /// interpolation moving continuously instead of holding at the newest
    /// result until the next arrives.
    lag_ms: f64,
}

/// Weight of each new arrival in the smoothed render delay.
const LAG_SMOOTHING: f64 = 0.1;

pub(crate) struct AsyncClothSystem {
    tx: Option<SyncSender<Request>>,
    rx: Option<Receiver<ResultFrame>>,
    config: ClothConfig,
    /// r_clothSolver 1: the renderer's gpu-cloth-sim port replaces this
    /// worker for GPU-skinned players.
    gpu_solver: bool,
    wind: [f32; 3],
    epoch: u64,
    generation: Arc<AtomicU64>,
    last_time: Option<i32>,
    pending: HashSet<u16>,
    last_sent: HashMap<u16, i32>,
    frames: HashMap<u16, FramePair>,
}

impl Default for AsyncClothSystem {
    fn default() -> Self {
        Self {
            tx: None,
            rx: None,
            config: ClothConfig::default(),
            gpu_solver: false,
            wind: [0.0; 3],
            epoch: 0,
            generation: Arc::new(AtomicU64::new(0)),
            last_time: None,
            pending: HashSet::new(),
            last_sent: HashMap::new(),
            frames: HashMap::new(),
        }
    }
}

impl AsyncClothSystem {
    fn ensure_worker(&mut self) {
        if self.tx.is_some() {
            return;
        }
        let (tx, requests) = mpsc::sync_channel::<Request>(QUEUE_CAPACITY);
        let (results, rx) = mpsc::channel();
        let generation = Arc::clone(&self.generation);
        std::thread::Builder::new()
            .name("jka-cloth-physics".into())
            .spawn(move || Self::run_worker(requests, results, generation))
            .expect("spawn JKA cloth physics worker");
        self.tx = Some(tx);
        self.rx = Some(rx);
    }

    fn run_worker(
        requests: Receiver<Request>,
        results: mpsc::Sender<ResultFrame>,
        generation: Arc<AtomicU64>,
    ) {
        let mut solver = ClothSystem::default();
        let mut epoch: Option<u64> = None;
        let mut last_slow_report: Option<std::time::Instant> = None;
        while let Ok(job) = requests.recv() {
            // A reset can have work queued; reject older generations.
            if job.epoch != generation.load(Ordering::Acquire) {
                continue;
            }
            if epoch != Some(job.epoch) {
                solver.reset();
                epoch = Some(job.epoch);
            }
            solver.set_config(job.config);
            solver.set_wind_velocity(job.wind);
            solver.begin_frame(job.time_ms);
            let started = std::time::Instant::now();
            let output = solver.simulate_garment(
                job.entity,
                &job.model,
                job.lod,
                &job.surfaces,
                &job.capsules,
                job.motion,
                job.time_ms,
            );
            let solve_ms = started.elapsed().as_secs_f32() * 1000.0;
            let tick_ms = 1000.0 / job.config.hz.max(1) as f32;
            if solve_ms > tick_ms * SLOW_SOLVE_TICKS
                && last_slow_report.is_none_or(|last| last.elapsed().as_secs_f32() >= 1.0)
            {
                last_slow_report = Some(std::time::Instant::now());
                devprintln!(
                    1,
                    "CLOTH: worker behind: {} solve took {:.1} ms (tick {:.1} ms, stiffness {:.1})",
                    job.model,
                    solve_ms,
                    tick_ms,
                    job.config.stiffness,
                );
            }
            let mut offsets = HashMap::new();
            let mut topology = None;
            if let Ok(output) = output {
                topology = solver.render_topology(job.entity);
                for surface in &job.surfaces {
                    if let Some(out) = output.get(&surface.surface_index) {
                        if out.positions.len() == surface.posed_positions.len() {
                            offsets.insert(
                                surface.surface_index,
                                out.positions
                                    .iter()
                                    .zip(&surface.posed_positions)
                                    .map(|(a, b)| [a[0] - b[0], a[1] - b[1], a[2] - b[2]])
                                    .collect(),
                            );
                        }
                    }
                }
            }
            // Invalidation may also happen during a solve. Do not
            // publish that result; the next current job resets state.
            if job.epoch != generation.load(Ordering::Acquire) {
                continue;
            }
            let pending_ms = solver.pending_seconds(job.entity).unwrap_or(0.0) as f64 * 1000.0;
            if results
                .send(ResultFrame {
                    epoch: job.epoch,
                    entity: job.entity,
                    model: job.model,
                    lod: job.lod,
                    time_ms: job.time_ms,
                    sim_time_ms: job.time_ms as f64 - pending_ms,
                    offsets,
                    topology,
                })
                .is_err()
            {
                break;
            }
        }
    }

    fn invalidate(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.generation.store(self.epoch, Ordering::Release);
        self.pending.clear();
        self.last_sent.clear();
        self.frames.clear();
    }

    pub(crate) fn reset(&mut self) {
        self.invalidate();
        self.last_time = None;
    }

    pub(crate) fn set_config(&mut self, config: ClothConfig) {
        if (self.config.enabled && !config.enabled)
            || self.config.animation_influence != config.animation_influence
            || self.config.body_clearance != config.body_clearance
            || self.config.body_collision != config.body_collision
            || self.config.stiffness != config.stiffness
            || self.config.damping != config.damping
            || self.config.stiffening != config.stiffening
            || self.config.hz != config.hz
        {
            self.invalidate();
        }
        self.config = config;
    }

    pub(crate) fn set_wind_velocity(&mut self, velocity: [f32; 3]) {
        self.wind = velocity;
    }

    pub(crate) fn set_gpu_solver(&mut self, enabled: bool) {
        self.gpu_solver = enabled;
    }

    /// GPU solver tuning (from the shared cloth settings) while cloth is on
    /// and r_clothSolver selects it.
    pub(crate) fn gpu_tuning(&self) -> Option<crate::renderer::ClothGpuTuning> {
        (self.gpu_solver && self.config.enabled)
            .then(|| super::cloth_gpu::tuning(&self.config))
    }

    pub(crate) fn config(&self) -> ClothConfig {
        self.config
    }

    pub(crate) fn enabled(&self) -> bool {
        self.config.enabled
    }

    pub(crate) fn begin_frame(&mut self, now_ms: i32) {
        if self.last_time.is_some_and(|last| now_ms < last) {
            self.invalidate();
        }
        self.last_time = Some(now_ms);
        self.poll(now_ms);
        self.frames
            .retain(|_, pair| now_ms.saturating_sub(pair.latest.time_ms) <= MAX_RESULT_AGE_MS);
    }

    fn poll(&mut self, now_ms: i32) {
        let Some(rx) = &self.rx else {
            return;
        };
        loop {
            match rx.try_recv() {
                Ok(frame) => {
                    if frame.epoch != self.epoch {
                        continue;
                    }
                    self.pending.remove(&frame.entity);
                    let lag = (now_ms as f64 - frame.sim_time_ms).max(0.0);
                    let entity = frame.entity;
                    let pair = match self.frames.remove(&entity) {
                        Some(mut pair)
                            if pair.latest.model == frame.model && pair.latest.lod == frame.lod =>
                        {
                            if frame.sim_time_ms <= pair.latest.sim_time_ms + 0.01 {
                                // No new fixed tick was integrated: same state.
                                // Keep the interpolation pair; only refresh age.
                                pair.latest.time_ms = pair.latest.time_ms.max(frame.time_ms);
                                self.frames.insert(entity, pair);
                                continue;
                            }
                            FramePair {
                                lag_ms: pair.lag_ms + (lag - pair.lag_ms) * LAG_SMOOTHING,
                                previous: Some(pair.latest),
                                latest: frame,
                            }
                        }
                        _ => FramePair {
                            previous: None,
                            latest: frame,
                            lag_ms: lag,
                        },
                    };
                    self.frames.insert(entity, pair);
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn simulate_garment(
        &mut self,
        entity: u16,
        model: &str,
        lod: usize,
        surfaces: Vec<ClothSurfaceFrame>,
        capsules: Vec<ClothCapsule>,
        motion: ClothMotion,
        now_ms: i32,
    ) -> Result<HashMap<usize, ClothOutput>, String> {
        if !self.enabled() || surfaces.is_empty() {
            return Ok(HashMap::new());
        }
        self.poll(now_ms);
        let body = self
            .config
            .body_collision
            .then_some((capsules.as_slice(), self.config.body_clearance));
        let outputs = self.latest_outputs_with_body(entity, model, lod, &surfaces, now_ms, body);
        // Dispatch only at the requested fixed rate. Avoid a large allocation
        // or a channel send at the thousands-of-FPS display cadence.
        let interval_ms = (1000 / self.config.hz.max(1)) as i32;
        let should_send = !self.pending.contains(&entity)
            && self
                .last_sent
                .get(&entity)
                .is_none_or(|last| now_ms < *last || now_ms - *last >= interval_ms);
        if should_send {
            self.ensure_worker();
            let job = Request {
                epoch: self.epoch,
                entity,
                model: model.to_owned(),
                lod,
                surfaces,
                capsules,
                motion,
                config: self.config,
                wind: self.wind,
                time_ms: now_ms,
            };
            match self.tx.as_ref().expect("worker initialized").try_send(job) {
                Ok(()) => {
                    self.pending.insert(entity);
                    self.last_sent.insert(entity, now_ms);
                }
                Err(TrySendError::Full(_)) => {} // Never wait for the worker.
                Err(TrySendError::Disconnected(_)) => {
                    return Err("cloth physics worker disconnected".into());
                }
            }
        }
        Ok(outputs)
    }

    #[cfg(test)]
    fn latest_outputs(
        &self,
        entity: u16,
        model: &str,
        lod: usize,
        surfaces: &[ClothSurfaceFrame],
        now_ms: i32,
    ) -> HashMap<usize, ClothOutput> {
        self.latest_outputs_with_body(entity, model, lod, surfaces, now_ms, None)
    }

    /// `body` re-projects the interpolated garment out of the CURRENT body
    /// pose; solver output is at least one tick old.
    fn latest_outputs_with_body(
        &self,
        entity: u16,
        model: &str,
        lod: usize,
        surfaces: &[ClothSurfaceFrame],
        now_ms: i32,
        body: Option<(&[ClothCapsule], f32)>,
    ) -> HashMap<usize, ClothOutput> {
        let Some(pair) = self.frames.get(&entity) else {
            return HashMap::new();
        };
        let latest = &pair.latest;
        let Some(topology) = &latest.topology else {
            return HashMap::new();
        };
        if latest.model != model
            || latest.lod != lod
            || now_ms < latest.time_ms
            || now_ms - latest.time_ms > MAX_RESULT_AGE_MS
            || !topology.matches(surfaces)
        {
            return HashMap::new();
        }
        let previous = pair.previous.as_ref().filter(|p| {
            p.model == model
                && p.lod == lod
                && p.sim_time_ms < latest.sim_time_ms
                && p.topology
                    .as_ref()
                    .is_some_and(|prior| Arc::ptr_eq(prior, topology))
        });
        // Interpolate secondary displacement (not old absolute positions) in
        // simulated time, one result interval behind the observed arrival
        // delay: alpha is 0 when a result arrives and reaches 1 about when
        // the next one is due. Never extrapolate solver output.
        let alpha = previous.map_or(1.0, |p| {
            let step_ms = (latest.sim_time_ms - p.sim_time_ms).max(0.001);
            let display = now_ms as f64 - pair.lag_ms - step_ms;
            ((display - p.sim_time_ms) / step_ms).clamp(0.0, 1.0) as f32
        });
        let mut interpolated = HashMap::new();
        for surface in surfaces {
            let Some(offsets) = latest.offsets.get(&surface.surface_index) else {
                continue;
            };
            if offsets.len() != surface.posed_positions.len() {
                continue;
            }
            let prior = previous
                .and_then(|p| p.offsets.get(&surface.surface_index))
                .filter(|v| v.len() == offsets.len());
            let offsets = offsets
                .iter()
                .enumerate()
                .map(|(i, offset)| {
                    let offset = *offset;
                    let offset = if let Some(prev) = prior {
                        std::array::from_fn(|j| prev[i][j] + (offset[j] - prev[i][j]) * alpha)
                    } else {
                        offset
                    };
                    offset
                })
                .collect();
            interpolated.insert(surface.surface_index, offsets);
        }
        topology.reconstruct_with_body(surfaces, &interpolated, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn motion() -> ClothMotion {
        ClothMotion {
            axis: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            origin: [0.0; 3],
        }
    }

    fn surface(
        index: usize,
        name: &str,
        positions: Vec<[f32; 3]>,
        triangles: Vec<[u32; 3]>,
    ) -> ClothSurfaceFrame {
        let count = positions.len();
        ClothSurfaceFrame {
            surface_index: index,
            surface_name: name.into(),
            bind_positions: positions.clone(),
            posed_positions: positions,
            posed_normals: vec![[0.0, -1.0, 0.0]; count],
            skin_transforms: vec![
                [
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0]
                ];
                count
            ],
            triangles,
        }
    }

    fn solved_frame(surfaces: &[ClothSurfaceFrame]) -> ResultFrame {
        let mut solver = ClothSystem::default();
        solver.set_config(ClothConfig {
            enabled: true,
            body_collision: false,
            animation_influence: 1.0,
            ..ClothConfig::default()
        });
        let output = solver
            .simulate_garment(8, "jedi", 0, surfaces, &[], motion(), 0)
            .unwrap();
        ResultFrame {
            epoch: 0,
            entity: 8,
            model: "jedi".into(),
            lod: 0,
            time_ms: 0,
            sim_time_ms: 0.0,
            offsets: surfaces
                .iter()
                .map(|s| {
                    (
                        s.surface_index,
                        output[&s.surface_index]
                            .positions
                            .iter()
                            .zip(&s.posed_positions)
                            .map(|(a, b)| std::array::from_fn(|j| a[j] - b[j]))
                            .collect(),
                    )
                })
                .collect(),
            topology: solver.render_topology(8),
        }
    }

    #[test]
    fn completed_cloth_displacements_follow_current_animation_without_blocking() {
        let mut worker = AsyncClothSystem::default();
        let mut surface = surface(
            7,
            "robe",
            vec![[10.0, 0.0, 0.0], [11.0, 0.0, 0.0], [10.0, 0.0, 1.0]],
            vec![[0, 1, 2]],
        );
        let topology = solved_frame(std::slice::from_ref(&surface)).topology;
        surface.posed_positions[0][0] = 20.0;
        let frame = |time_ms, offset| ResultFrame {
            epoch: 0,
            entity: 8,
            model: "jedi".into(),
            lod: 0,
            time_ms,
            sim_time_ms: f64::from(time_ms),
            offsets: HashMap::from([(7, vec![[offset, 0.0, 0.0]; 3])]),
            topology: topology.clone(),
        };
        worker.frames.insert(
            8,
            FramePair {
                previous: Some(frame(0, 1.0)),
                latest: frame(16, 3.0),
                lag_ms: 0.0,
            },
        );
        // At 16ms, target is 0ms (one tick latency): use the older offset.
        let a = worker.latest_outputs(8, "jedi", 0, std::slice::from_ref(&surface), 16);
        assert_eq!(a[&7].positions[0][0], 21.0);
        // At 32ms, target reaches the newest solved snapshot.
        surface.posed_positions[0][0] = 25.0;
        let b = worker.latest_outputs(8, "jedi", 0, &[surface], 32);
        assert_eq!(b[&7].positions[0][0], 28.0);
        worker.reset();
        assert!(worker.frames.is_empty());
    }

    #[test]
    fn welded_seams_follow_current_pose_during_offset_interpolation() {
        let mut surfaces = vec![
            surface(
                0,
                "torso_robe",
                vec![
                    [-5.0, 10.0, 10.0],
                    [5.0, 10.0, 10.0],
                    [-5.0, 10.0, 20.0],
                    [5.0, 10.0, 20.0],
                ],
                vec![[0, 1, 2], [1, 3, 2]],
            ),
            surface(
                1,
                "hips_robe",
                vec![
                    [-5.0, 11.0, 0.0],
                    [5.0, 11.0, 0.0],
                    [-5.0, 10.0, 10.4],
                    [5.0, 10.0, 10.4],
                ],
                vec![[0, 1, 2], [1, 3, 2]],
            ),
        ];
        let previous = solved_frame(&surfaces);
        let latest = ResultFrame {
            time_ms: 16,
            sim_time_ms: 16.0,
            topology: previous.topology.clone(),
            offsets: previous
                .offsets
                .iter()
                .map(|(&k, v)| (k, v.iter().map(|p| [p[0], p[1] + 1.0, p[2]]).collect()))
                .collect(),
            epoch: 0,
            entity: 8,
            model: "jedi".into(),
            lod: 0,
        };
        let mut worker = AsyncClothSystem::default();
        worker.frames.insert(
            8,
            FramePair {
                previous: Some(previous),
                latest,
                lag_ms: 0.0,
            },
        );
        surfaces[1].posed_positions[2][1] += 2.0;
        surfaces[1].posed_positions[3][1] += 2.0;
        for now in [16, 24, 32] {
            let output = worker.latest_outputs(8, "jedi", 0, &surfaces, now);
            for (upper, lower) in [(0, 2), (1, 3)] {
                assert_eq!(output[&0].positions[upper], output[&1].positions[lower]);
                assert_eq!(output[&0].normals[upper], output[&1].normals[lower]);
            }
        }
        // Preserve an authored crease even though its positions are welded.
        surfaces[1].posed_normals.fill([0.0, 0.0, 1.0]);
        surfaces[1].posed_positions[0][1] = 20.0;
        surfaces[1].posed_positions[1][1] = 20.0;
        let output = worker.latest_outputs(8, "jedi", 0, &surfaces, 32);
        let a = glam::Vec3::from_array(output[&0].normals[0]);
        let b = glam::Vec3::from_array(output[&1].normals[2]);
        assert!(a.dot(b) < 0.99);
        // A changed garment must wait for its own topology, not reuse a partial weld.
        assert!(worker
            .latest_outputs(8, "jedi", 0, &surfaces[..1], 32)
            .is_empty());
    }

    #[test]
    fn normals_follow_current_geometry() {
        let mut surfaces = vec![surface(
            0,
            "robe",
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            vec![[0, 1, 2]],
        )];
        let mut frame = solved_frame(&surfaces);
        frame.offsets.insert(0, vec![[0.0; 3]; 3]);
        let mut worker = AsyncClothSystem::default();
        worker.frames.insert(
            8,
            FramePair {
                previous: None,
                latest: frame,
                lag_ms: 0.0,
            },
        );
        surfaces[0].posed_positions = vec![[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        surfaces[0].posed_normals.fill([-1.0, 0.0, 0.0]);
        let output = worker.latest_outputs(8, "jedi", 0, &surfaces, 16);
        assert_eq!(output[&0].normals, vec![[-1.0, 0.0, 0.0]; 3]);
    }

    #[test]
    fn reset_skips_jobs_already_queued_before_the_worker_runs() {
        let mut worker = AsyncClothSystem::default();
        let (tx, requests) = mpsc::sync_channel(QUEUE_CAPACITY);
        let (results, rx) = mpsc::channel();
        let job = |epoch| Request {
            epoch,
            entity: 8,
            model: "jedi".into(),
            lod: 0,
            surfaces: vec![],
            capsules: vec![],
            motion: motion(),
            config: ClothConfig::default(),
            wind: [0.0; 3],
            time_ms: 0,
        };
        tx.send(job(0)).unwrap();
        worker.reset();
        tx.send(job(worker.epoch)).unwrap();
        drop(tx);
        AsyncClothSystem::run_worker(requests, results, Arc::clone(&worker.generation));
        let frames = rx.try_iter().collect::<Vec<_>>();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].epoch, worker.epoch);
    }
}
