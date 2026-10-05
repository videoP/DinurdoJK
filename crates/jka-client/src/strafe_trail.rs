//! jaPRO/TaystJK-compatible strafe trails with a Rust-native async loader.
//!
//! The disk format intentionally stays compatible with jaPRO: whitespace-separated
//! `x y z` point triples in `strafetrails/<name>.cfg`. Parsing and filesystem IO
//! run on a dedicated worker. Loaded trails are immutable segment sets with a small
//! 2D spatial index, while live trails use bounded per-client sample deques.

use crate::fx::system::FxDraw;
use jka_assets::pk3::AssetSearchPath;
use std::{
    collections::{HashMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

pub const DEFAULT_SLOT: u8 = 28;
pub const MAX_CLIENTS: usize = 32;
const MAX_TRAIL_BYTES: usize = 16 * 1024 * 1024;
const MAX_POINTS: usize = 1_000_000;
const MAX_LINK_DISTANCE: f32 = 512.0;
const MAX_LIVE_LINK_DISTANCE: f32 = 192.0;
const CELL_SIZE: f32 = 2048.0;
const LOD_TOLERANCES: [f32; 5] = [0.0, 0.5, 2.0, 8.0, 32.0];
const LOD_PIXEL_ERROR: f32 = 1.0;
const TRAIL_SHADER: &str = "gfx/misc/whiteline2";
const PLUM_DISTANCE: f32 = 4096.0;
const PLUM_NUMBER_SIZE: f32 = 8.0;
const NUMBER_SHADERS: [&str; 10] = [
    "gfx/2d/numbers/zero",
    "gfx/2d/numbers/one",
    "gfx/2d/numbers/two",
    "gfx/2d/numbers/three",
    "gfx/2d/numbers/four",
    "gfx/2d/numbers/five",
    "gfx/2d/numbers/six",
    "gfx/2d/numbers/seven",
    "gfx/2d/numbers/eight",
    "gfx/2d/numbers/nine",
];

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// jaPRO `cg_strafeTrailRadius` (full line diameter in game units).
    pub radius: f32,
    /// jaPRO `cg_strafeTrailLife`, used by live player trails in seconds.
    pub life_seconds: f32,
    /// jaPRO `cg_strafeTrailFPS`: SV_FPS the loaded recording was captured at.
    /// This is metadata for trail markers/plums; it must not throttle live snapshot sampling.
    pub fps: f32,
    /// jaPRO `cg_strafeTrailPlums`: show whole-second markers on loaded trails.
    pub plums: bool,
    /// jaPRO `cg_strafeTrailGhost`: render trails translucently.
    pub ghost: bool,
    /// jaPRO `cg_strafeTrailPlayers` bitmask.
    pub players: u32,
    /// jaPRO `cg_logStrafeTrail`: `0` means disabled, otherwise the destination stem.
    pub log_name: String,
    /// DinurdoJK extension. Legacy code hard-culls at 16384 units; make it explicit.
    pub draw_distance: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            radius: 2.0,
            life_seconds: 5.0,
            fps: 40.0,
            plums: false,
            ghost: true,
            players: 0,
            log_name: "0".to_owned(),
            draw_distance: 16_384.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Segment {
    start: [f32; 3],
    end: [f32; 3],
    midpoint: [f32; 3],
}

impl Segment {
    fn new(start: [f32; 3], end: [f32; 3]) -> Self {
        Self {
            start,
            end,
            midpoint: [
                (start[0] + end[0]) * 0.5,
                (start[1] + end[1]) * 0.5,
                (start[2] + end[2]) * 0.5,
            ],
        }
    }
}

#[derive(Debug)]
struct TrailCell {
    center: [f32; 3],
    radius: f32,
    lods: [Vec<Segment>; LOD_TOLERANCES.len()],
}

impl TrailCell {
    fn new(segments: Vec<Segment>) -> Self {
        let mut minimum = [f32::INFINITY; 3];
        let mut maximum = [f32::NEG_INFINITY; 3];
        for segment in &segments {
            for point in [segment.start, segment.end] {
                for axis in 0..3 {
                    minimum[axis] = minimum[axis].min(point[axis]);
                    maximum[axis] = maximum[axis].max(point[axis]);
                }
            }
        }
        let center = [
            (minimum[0] + maximum[0]) * 0.5,
            (minimum[1] + maximum[1]) * 0.5,
            (minimum[2] + maximum[2]) * 0.5,
        ];
        let radius = squared_distance(center, maximum).sqrt();
        let lods = std::array::from_fn(|level| {
            if level == 0 {
                segments.clone()
            } else {
                simplify_segments(&segments, LOD_TOLERANCES[level])
            }
        });
        Self { center, radius, lods }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TrailView {
    pub origin: [f32; 3],
    pub forward: [f32; 3],
    pub left: [f32; 3],
    pub up: [f32; 3],
    pub tan_half_fov_x: f32,
    pub tan_half_fov_y: f32,
    pub focal_length_pixels: f32,
}

impl TrailView {
    fn sphere_visible(self, center: [f32; 3], radius: f32) -> bool {
        let relative = [
            center[0] - self.origin[0],
            center[1] - self.origin[1],
            center[2] - self.origin[2],
        ];
        let depth = dot(relative, self.forward);
        let horizontal = dot(relative, self.left).abs();
        let vertical = dot(relative, self.up).abs();
        let horizontal_radius = radius * (1.0 + self.tan_half_fov_x * self.tan_half_fov_x).sqrt();
        let vertical_radius = radius * (1.0 + self.tan_half_fov_y * self.tan_half_fov_y).sqrt();
        horizontal <= depth * self.tan_half_fov_x + horizontal_radius
            && vertical <= depth * self.tan_half_fov_y + vertical_radius
    }

    fn lod_level(self, center: [f32; 3], radius: f32) -> usize {
        let distance = (squared_distance(self.origin, center).sqrt() - radius).max(1.0);
        let world_error = LOD_PIXEL_ERROR * distance / self.focal_length_pixels.max(1.0);
        LOD_TOLERANCES
            .iter()
            .rposition(|tolerance| *tolerance <= world_error)
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct TimeMarker {
    origin: [f32; 3],
    seconds: u32,
}

#[derive(Debug)]
pub struct LoadedTrail {
    pub name: String,
    pub slot: u8,
    pub point_count: usize,
    pub segment_count: usize,
    cells: HashMap<(i32, i32), TrailCell>,
    source_points: Box<[[f32; 3]]>,
    marker_fps: u16,
    markers: Vec<TimeMarker>,
}

impl LoadedTrail {
    fn new(name: String, slot: u8, points: &[[f32; 3]]) -> Self {
        let mut raw_cells: HashMap<(i32, i32), Vec<Segment>> = HashMap::new();
        let max_link_sq = MAX_LINK_DISTANCE * MAX_LINK_DISTANCE;
        let mut segment_count = 0usize;
        for pair in points.windows(2) {
            let start = pair[0];
            let end = pair[1];
            let distance_sq = squared_distance(start, end);
            // jaPRO deliberately breaks the line across teleport/discontinuity gaps.
            if distance_sq <= f32::EPSILON || distance_sq >= max_link_sq {
                continue;
            }
            let segment = Segment::new(start, end);
            raw_cells.entry(cell(segment.midpoint)).or_default().push(segment);
            segment_count += 1;
        }
        // LOD generation runs on the existing trail loader thread. Each spatial
        // cell keeps a few progressively simplified line lists, so the frame path
        // only does a frustum test + one cheap LOD selection per visible cell.
        let cells = raw_cells
            .into_iter()
            .map(|(key, segments)| (key, TrailCell::new(segments)))
            .collect();
        Self {
            name,
            slot,
            point_count: points.len(),
            segment_count,
            cells,
            source_points: points.to_vec().into_boxed_slice(),
            marker_fps: 0,
            markers: Vec::new(),
        }
    }

    fn refresh_markers(&mut self, fps: usize) {
        let fps = fps.clamp(1, 1000);
        if self.marker_fps == fps as u16 {
            return;
        }
        self.marker_fps = fps as u16;
        self.markers.clear();
        self.markers.reserve(self.source_points.len() / fps);

        // TaystJK numbers trail rows from one and creates a plum whenever that
        // ordinal is divisible by cg_strafeTrailFPS. Keep the intended timing,
        // but anchor to that sample directly instead of inheriting its reversed
        // start/end argument's one-sample placement quirk.
        for number in (fps..=self.source_points.len()).step_by(fps) {
            let mut origin = self.source_points[number - 1];
            origin[2] += 8.0;
            self.markers.push(TimeMarker { origin, seconds: (number / fps) as u32 });
        }
    }

    /// Build an indexed trail from points already resident in memory. Race
    /// ghosts use this on their demo-loader thread so the render thread gets the
    /// same spatial cells and angular LODs as an ordinary `loadTrail` asset.
    pub(crate) fn from_points(name: String, slot: u8, points: &[[f32; 3]]) -> Self {
        Self::new(name, slot, points)
    }

    pub(crate) fn set_slot(&mut self, slot: u8) {
        self.slot = slot.min((MAX_CLIENTS - 1) as u8);
    }

    fn append_draws(
        &mut self,
        settings: &Settings,
        view: TrailView,
        include_plums: bool,
        draws: &mut Vec<FxDraw>,
    ) {
        let camera = view.origin;
        let distance = settings.draw_distance.clamp(256.0, 131_072.0);
        let distance_sq = distance * distance;
        let width = settings.radius.clamp(0.1, 100.0) * 0.5;
        let alpha = if settings.ghost { 120 } else { 255 };
        let radius_cells = ((distance + MAX_LINK_DISTANCE) / CELL_SIZE).ceil() as i32;
        let center = cell(camera);
        let rgb = slot_color(self.slot);

        for y in (center.1 - radius_cells)..=(center.1 + radius_cells) {
            for x in (center.0 - radius_cells)..=(center.0 + radius_cells) {
                let Some(cell) = self.cells.get(&(x, y)) else { continue };
                let max_distance = distance + cell.radius;
                if squared_distance(cell.center, camera) > max_distance * max_distance
                    || !view.sphere_visible(cell.center, cell.radius)
                {
                    continue;
                }
                let lod = view.lod_level(cell.center, cell.radius);
                for segment in &cell.lods[lod] {
                    if squared_distance(segment.midpoint, camera) > distance_sq {
                        continue;
                    }
                    draws.push(FxDraw::Line {
                        start: segment.start,
                        end: segment.end,
                        width,
                        rgba: [rgb[0], rgb[1], rgb[2], alpha],
                        shader: TRAIL_SHADER.to_owned(),
                    });
                }
            }
        }

        if include_plums && settings.plums {
            let plum_fps = settings.fps.clamp(1.0, 1000.0) as usize;
            let plum_distance = distance.min(PLUM_DISTANCE);
            let plum_distance_sq = plum_distance * plum_distance;
            self.refresh_markers(plum_fps);
            for marker in &self.markers {
                let dz = camera[2] - marker.origin[2];
                if dz > 2048.0 || dz < -8192.0 || squared_distance(marker.origin, camera) > plum_distance_sq {
                    continue;
                }
                if view.sphere_visible(marker.origin, PLUM_NUMBER_SIZE) {
                    append_time_marker(draws, *marker, camera);
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct LiveSample {
    origin: [f32; 3],
    time_ms: i32,
}

#[derive(Debug, Default)]
struct LiveTrail {
    points: VecDeque<LiveSample>,
    last_sample_ms: Option<i32>,
}

#[derive(Debug)]
enum Request {
    Scan { base: PathBuf, game: Option<PathBuf> },
    Load { base: PathBuf, game: Option<PathBuf>, name: String, slot: u8 },
    BeginLog { directory: PathBuf, name: String },
    LogPoint([f32; 3]),
    FlushLog,
    EndLog,
    Shutdown,
}

#[derive(Debug)]
enum Response {
    Scan(Result<Vec<String>, String>),
    Load(Result<LoadedTrail, String>),
    Log(Result<String, String>),
}

pub struct Manager {
    tx: Sender<Request>,
    rx: Receiver<Response>,
    pub settings: Settings,
    loaded: Vec<LoadedTrail>,
    live: [LiveTrail; MAX_CLIENTS],
    available: Vec<String>,
    catalog_error: Option<String>,
    catalog_pending: bool,
    log_active_name: Option<String>,
    log_last_sample: Option<LiveSample>,
    last_frame_time_ms: Option<i32>,
}

impl Manager {
    pub fn spawn(settings: Settings) -> Result<Self, String> {
        let (tx, worker_rx) = mpsc::channel();
        let (worker_tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("strafe-trails".into())
            .spawn(move || worker(worker_rx, worker_tx))
            .map_err(|error| format!("could not start strafe-trail worker: {error}"))?;
        Ok(Self {
            tx,
            rx,
            settings,
            loaded: Vec::new(),
            live: std::array::from_fn(|_| LiveTrail::default()),
            available: Vec::new(),
            catalog_error: None,
            catalog_pending: false,
            log_active_name: None,
            log_last_sample: None,
            last_frame_time_ms: None,
        })
    }

    pub fn available(&self) -> &[String] { &self.available }
    pub fn catalog_error(&self) -> Option<&str> { self.catalog_error.as_deref() }
    pub fn catalog_pending(&self) -> bool { self.catalog_pending }
    pub fn loaded(&self) -> impl Iterator<Item = &LoadedTrail> { self.loaded.iter() }
    pub fn is_tracing(&self, client: usize) -> bool {
        client < MAX_CLIENTS && self.settings.players & (1u32 << client) != 0
    }

    pub fn request_scan(&mut self, base: &Path, game: Option<&Path>) -> Result<(), String> {
        self.catalog_pending = true;
        self.catalog_error = None;
        self.tx.send(Request::Scan { base: base.to_owned(), game: game.map(Path::to_owned) })
            .map_err(|_| "strafe-trail worker stopped".to_owned())
    }

    pub fn request_load(&self, base: &Path, game: Option<&Path>, name: &str, slot: u8) -> Result<(), String> {
        let name = sanitize_name(name)?;
        self.tx.send(Request::Load {
            base: base.to_owned(),
            game: game.map(Path::to_owned),
            name,
            slot: slot.min((MAX_CLIENTS - 1) as u8),
        }).map_err(|_| "strafe-trail worker stopped".to_owned())
    }

    pub fn clear(&mut self, slot: i32) -> usize {
        if slot == -1 {
            let count = self.loaded.len();
            self.loaded.clear();
            self.clear_live(None);
            return count;
        }
        let Ok(slot) = u8::try_from(slot) else { return 0 };
        if usize::from(slot) < MAX_CLIENTS {
            // TaystJK's CG_RemoveStrafeTrail removes existing geometry for this
            // client/slot. Keep the live trace bit itself enabled, so new
            // snapshot segments can begin appearing again immediately.
            self.clear_live(Some(usize::from(slot)));
        }
        let before = self.loaded.len();
        self.loaded.retain(|trail| trail.slot != slot);
        before - self.loaded.len()
    }

    pub fn clear_live(&mut self, client: Option<usize>) {
        if let Some(client) = client.filter(|client| *client < MAX_CLIENTS) {
            self.live[client] = LiveTrail::default();
        } else if client.is_none() {
            self.live = std::array::from_fn(|_| LiveTrail::default());
        }
    }

    pub fn set_players_mask(&mut self, mask: u32) {
        let disabled = self.settings.players & !mask;
        self.settings.players = mask;
        for client in 0..MAX_CLIENTS {
            if disabled & (1u32 << client) != 0 {
                self.live[client] = LiveTrail::default();
            }
        }
    }

    pub fn toggle_player(&mut self, client: i32) -> Result<u32, String> {
        if client == -1 {
            // TaystJK semantics: -1 means all OFF if anything is enabled,
            // otherwise all ON. Do not invert every individual bit.
            let mask = if self.settings.players == 0 { u32::MAX } else { 0 };
            self.set_players_mask(mask);
            return Ok(self.settings.players);
        }
        let client = usize::try_from(client).map_err(|_| "client number must be -1 or 0..31".to_owned())?;
        if client >= MAX_CLIENTS {
            return Err("client number must be -1 or 0..31".to_owned());
        }
        self.settings.players ^= 1u32 << client;
        if !self.is_tracing(client) {
            self.live[client] = LiveTrail::default();
        }
        Ok(self.settings.players)
    }

    /// Poll completed worker work. Returned lines are suitable for the console.
    pub fn poll(&mut self) -> Vec<String> {
        let mut lines = Vec::new();
        while let Ok(response) = self.rx.try_recv() {
            match response {
                Response::Scan(result) => {
                    self.catalog_pending = false;
                    match result {
                        Ok(entries) => {
                            let count = entries.len();
                            self.available = entries;
                            self.catalog_error = None;
                            lines.push(format!("^2Strafe trails:^7 indexed {count} file(s)."));
                        }
                        Err(error) => {
                            self.catalog_error = Some(error.clone());
                            lines.push(format!("^1Strafe trail scan failed:^7 {error}"));
                        }
                    }
                }
                Response::Load(result) => match result {
                    Ok(trail) => {
                        let slot = trail.slot;
                        let name = trail.name.clone();
                        let points = trail.point_count;
                        let segments = trail.segment_count;
                        // Legacy loadTrail does not replace an existing trail with the
                        // same client/color slot; clearTrail <slot> removes all geometry
                        // tagged with that slot. Preserve that useful behavior while
                        // keeping each parsed trail as one immutable indexed asset.
                        self.loaded.push(trail);
                        lines.push(format!("^2Loaded trail^7 {name} into slot {slot}: {points} points, {segments} segments."));
                    }
                    Err(error) => lines.push(format!("^1loadTrail:^7 {error}")),
                },
                Response::Log(result) => match result {
                    Ok(line) => lines.push(format!("^2{line}")),
                    Err(error) => {
                        // A failed open/write means the worker is no longer recording.
                        // Mirror that state locally so we do not keep queueing points.
                        self.log_active_name = None;
                        self.settings.log_name = "0".to_owned();
                        self.log_last_sample = None;
                        lines.push(format!("^1Strafe trail log:^7 {error}"));
                    }
                },
            }
        }
        lines
    }

    pub fn set_log_name(&mut self, active_dir: &Path, value: &str) -> Result<(), String> {
        let value = value.trim();
        if value.is_empty() || value == "0" {
            self.settings.log_name = "0".to_owned();
            self.log_active_name = None;
            self.log_last_sample = None;
            self.tx.send(Request::EndLog).map_err(|_| "strafe-trail worker stopped".to_owned())?;
            return Ok(());
        }
        let name = sanitize_name(value)?;
        self.settings.log_name = name.clone();
        self.log_active_name = Some(name.clone());
        self.log_last_sample = None;
        self.tx.send(Request::BeginLog { directory: active_dir.join("strafetrails"), name })
            .map_err(|_| "strafe-trail worker stopped".to_owned())
    }

    pub fn log_point_if_active(&mut self, origin: [f32; 3], now_ms: i32, race_active: bool) {
        if !race_active {
            // The legacy client closes the file when a timed run ends. Keep our
            // worker handle warm for the next run, but flush at the same boundary
            // so a completed run is durable without blocking the game thread.
            if self.log_last_sample.take().is_some() {
                let _ = self.tx.send(Request::FlushLog);
            }
            return;
        }
        if self.log_active_name.is_none() || !origin.iter().all(|v| v.is_finite()) {
            self.log_last_sample = None;
            return;
        }
        // CG_LogStrafeTrail runs on snapshot transitions. Mirror that exactly:
        // one candidate point per new serverTime, independent of render FPS and
        // independent of cg_strafeTrailFPS (which describes recorded SV_FPS).
        if self.log_last_sample.is_some_and(|last| {
            last.time_ms == now_ms || squared_distance(last.origin, origin) < 0.25
        }) {
            return;
        }
        self.log_last_sample = Some(LiveSample { origin, time_ms: now_ms });
        let _ = self.tx.send(Request::LogPoint(origin));
    }

    pub fn sample_player(&mut self, client: usize, origin: [f32; 3], now_ms: i32) {
        if client >= MAX_CLIENTS || !self.is_tracing(client) || !origin.iter().all(|v| v.is_finite()) {
            return;
        }
        let trail = &mut self.live[client];
        // TaystJK adds live trail segments from CG_TransitionSnapshot, not every
        // rendered frame. presented_entities can be visited many times for one
        // snapshot, so serverTime is our zero-allocation snapshot discriminator.
        if trail.last_sample_ms == Some(now_ms) {
            return;
        }
        if trail.points.back().is_some_and(|last| squared_distance(last.origin, origin) < 0.25) {
            trail.last_sample_ms = Some(now_ms);
            return;
        }
        trail.last_sample_ms = Some(now_ms);
        trail.points.push_back(LiveSample { origin, time_ms: now_ms });
        let life_ms = (self.settings.life_seconds.clamp(0.1, 3600.0) * 1000.0) as i32;
        while trail.points.front().is_some_and(|sample| now_ms.saturating_sub(sample.time_ms) > life_ms) {
            trail.points.pop_front();
        }
    }

    /// Submit a pre-indexed, in-memory trail (for example a race ghost route)
    /// with the user's normal strafe-trail appearance/culling settings. Plums
    /// are deliberately omitted because the ghost demo has its own race clock.
    pub(crate) fn append_prebuilt_draws(
        &self,
        trail: &mut LoadedTrail,
        view: TrailView,
        draws: &mut Vec<FxDraw>,
    ) {
        trail.append_draws(&self.settings, view, false, draws);
    }

    pub fn append_draws(&mut self, view: TrailView, now_ms: i32, draws: &mut Vec<FxDraw>) {
        // Demo seeks/map restarts can move serverTime backwards. A live trail is
        // temporal state, so discard it instead of pinning samples in the future.
        if self.last_frame_time_ms.is_some_and(|last| now_ms < last) {
            self.clear_live(None);
            self.log_last_sample = None;
        }
        self.last_frame_time_ms = Some(now_ms);

        let distance = self.settings.draw_distance.clamp(256.0, 131_072.0);
        let distance_sq = distance * distance;
        let width = self.settings.radius.clamp(0.1, 100.0) * 0.5;
        let alpha = if self.settings.ghost { 120 } else { 255 };
        for trail in &mut self.loaded {
            trail.append_draws(&self.settings, view, true, draws);
        }

        // Expire live samples even when a tracked player disappears from the
        // current snapshot (disconnect, PVS transition, etc.). Legacy local
        // entities handled this implicitly via endTime; our deque needs to do it.
        let players_mask = self.settings.players;
        let life_ms = (self.settings.life_seconds.clamp(0.1, 3600.0) * 1000.0) as i32;
        for (client, live) in self.live.iter_mut().enumerate() {
            while live
                .points
                .front()
                .is_some_and(|sample| now_ms.saturating_sub(sample.time_ms) > life_ms)
            {
                live.points.pop_front();
            }
            if players_mask & (1u32 << client) == 0 {
                continue;
            }
            let rgb = slot_color(client as u8);
            let (first, second) = live.points.as_slices();
            if second.is_empty() {
                for pair in first.windows(2) {
                    append_live_segment(draws, pair[0], pair[1], view, distance_sq, width, rgb, alpha);
                }
            } else {
                // A wrapped VecDeque has one bridge segment between its slices;
                // flatten only in this uncommon case so the normal path allocates nothing.
                let points: Vec<_> = live.points.iter().copied().collect();
                for pair in points.windows(2) {
                    append_live_segment(draws, pair[0], pair[1], view, distance_sq, width, rgb, alpha);
                }
            }
        }
    }
}

impl Drop for Manager {
    fn drop(&mut self) { let _ = self.tx.send(Request::Shutdown); }
}

fn append_time_marker(draws: &mut Vec<FxDraw>, marker: TimeMarker, camera: [f32; 3]) {
    let dir = [
        camera[0] - marker.origin[0],
        camera[1] - marker.origin[1],
        camera[2] - marker.origin[2],
    ];
    // Same camera-relative horizontal layout used by JKA/TaystJK score plums.
    let mut side = [dir[1], -dir[0], 0.0];
    let side_len = (side[0] * side[0] + side[1] * side[1]).sqrt();
    if side_len > f32::EPSILON {
        side[0] /= side_len;
        side[1] /= side_len;
    } else {
        side = [1.0, 0.0, 0.0];
    }

    let text = marker.seconds.to_string();
    let count = text.len() as f32;
    // TaystJK's trail-number flavor of CG_AddScorePlum has c == 0, leaving a
    // constant -10-unit camera-relative side offset before laying out digits.
    let base = [
        marker.origin[0] - side[0] * 10.0,
        marker.origin[1] - side[1] * 10.0,
        marker.origin[2],
    ];
    for (index, byte) in text.bytes().enumerate() {
        let Some(digit) = byte.checked_sub(b'0').filter(|digit| *digit <= 9) else { continue };
        let offset = (count * 0.5 - index as f32) * PLUM_NUMBER_SIZE;
        draws.push(FxDraw::Sprite {
            origin: [base[0] + side[0] * offset, base[1] + side[1] * offset, base[2]],
            radius: PLUM_NUMBER_SIZE * 0.5,
            rotation: 0.0,
            // TaystJK explicitly overrides trail-plum score sprites to red.
            rgba: [255, 0, 0, 255],
            shader: NUMBER_SHADERS[digit as usize].to_owned(),
        });
    }
}

fn append_live_segment(
    draws: &mut Vec<FxDraw>,
    a: LiveSample,
    b: LiveSample,
    view: TrailView,
    distance_sq: f32,
    width: f32,
    rgb: [u8; 3],
    alpha: u8,
) {
    let camera = view.origin;
    if squared_distance(a.origin, b.origin) >= MAX_LIVE_LINK_DISTANCE * MAX_LIVE_LINK_DISTANCE { return; }
    let midpoint = [
        (a.origin[0] + b.origin[0]) * 0.5,
        (a.origin[1] + b.origin[1]) * 0.5,
        (a.origin[2] + b.origin[2]) * 0.5,
    ];
    let half_length = squared_distance(a.origin, b.origin).sqrt() * 0.5;
    if squared_distance(midpoint, camera) > distance_sq || !view.sphere_visible(midpoint, half_length) { return; }
    draws.push(FxDraw::Line {
        start: a.origin,
        end: b.origin,
        width,
        rgba: [rgb[0], rgb[1], rgb[2], alpha],
        shader: TRAIL_SHADER.to_owned(),
    });
}

fn simplify_segments(segments: &[Segment], tolerance: f32) -> Vec<Segment> {
    if tolerance <= 0.0 || segments.len() < 2 {
        return segments.to_vec();
    }
    let mut output = Vec::with_capacity(segments.len());
    let mut run = Vec::new();

    let flush = |run: &mut Vec<[f32; 3]>, output: &mut Vec<Segment>| {
        if run.len() < 2 {
            run.clear();
            return;
        }
        let simplified = simplify_points(run, tolerance);
        output.extend(simplified.windows(2).map(|pair| Segment::new(pair[0], pair[1])));
        run.clear();
    };

    for segment in segments {
        if run.is_empty() {
            run.push(segment.start);
            run.push(segment.end);
        } else if run.last().is_some_and(|last| squared_distance(*last, segment.start) <= f32::EPSILON) {
            run.push(segment.end);
        } else {
            flush(&mut run, &mut output);
            run.push(segment.start);
            run.push(segment.end);
        }
    }
    flush(&mut run, &mut output);
    output
}

fn simplify_points(points: &[[f32; 3]], tolerance: f32) -> Vec<[f32; 3]> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let tolerance_sq = tolerance * tolerance;
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0usize, points.len() - 1)];

    while let Some((first, last)) = stack.pop() {
        if last <= first + 1 {
            continue;
        }
        let mut best_index = None;
        let mut best_distance = 0.0f32;
        for index in first + 1..last {
            let distance = point_segment_distance_sq(points[index], points[first], points[last]);
            if distance > best_distance {
                best_distance = distance;
                best_index = Some(index);
            }
        }
        if best_distance > tolerance_sq {
            let index = best_index.expect("non-empty RDP span");
            keep[index] = true;
            stack.push((first, index));
            stack.push((index, last));
        }
    }

    points
        .iter()
        .zip(keep)
        .filter_map(|(point, keep)| keep.then_some(*point))
        .collect()
}

fn point_segment_distance_sq(point: [f32; 3], start: [f32; 3], end: [f32; 3]) -> f32 {
    let edge = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
    let to_point = [point[0] - start[0], point[1] - start[1], point[2] - start[2]];
    let edge_sq = dot(edge, edge);
    if edge_sq <= f32::EPSILON {
        return dot(to_point, to_point);
    }
    let t = (dot(to_point, edge) / edge_sq).clamp(0.0, 1.0);
    let closest = [start[0] + edge[0] * t, start[1] + edge[1] * t, start[2] + edge[2] * t];
    squared_distance(point, closest)
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn worker(rx: Receiver<Request>, tx: Sender<Response>) {
    let mut writer: Option<(String, BufWriter<File>)> = None;
    while let Ok(request) = rx.recv() {
        match request {
            Request::Scan { base, game } => {
                let result = scan(&base, game.as_deref());
                let _ = tx.send(Response::Scan(result));
            }
            Request::Load { base, game, name, slot } => {
                let result = load(&base, game.as_deref(), &name, slot);
                let _ = tx.send(Response::Load(result));
            }
            Request::BeginLog { directory, name } => {
                let result = (|| {
                    if let Some((_, mut old)) = writer.take() {
                        old.flush().map_err(|e| format!("could not flush previous strafe trail: {e}"))?;
                    }
                    fs::create_dir_all(&directory).map_err(|e| format!("could not create {}: {e}", directory.display()))?;
                    let path = directory.join(format!("{name}.cfg"));
                    // jaPRO opens the trail with FS_APPEND: multiple recorded runs can
                    // share a file without destroying an existing recording.
                    let file = OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&path)
                        .map_err(|e| format!("could not open {}: {e}", path.display()))?;
                    writer = Some((name.clone(), BufWriter::new(file)));
                    Ok(format!("Recording strafe trail to {}", path.display()))
                })();
                let _ = tx.send(Response::Log(result));
            }
            Request::LogPoint(point) => {
                if let Some((name, out)) = writer.as_mut() {
                    if let Err(error) = writeln!(out, "{} {} {}", point[0] as i32, point[1] as i32, point[2] as i32) {
                        let failed = name.clone();
                        writer = None;
                        let _ = tx.send(Response::Log(Err(format!("write to {failed}.cfg failed: {error}"))));
                    }
                }
            }
            Request::FlushLog => {
                if let Some((name, out)) = writer.as_mut() {
                    if let Err(error) = out.flush() {
                        let failed = name.clone();
                        writer = None;
                        let _ = tx.send(Response::Log(Err(format!("flush {failed}.cfg failed: {error}"))));
                    }
                }
            }
            Request::EndLog => {
                if let Some((name, mut out)) = writer.take() {
                    let result = out.flush()
                        .map(|_| format!("Stopped recording strafe trail {name}.cfg"))
                        .map_err(|e| format!("flush {name}.cfg failed: {e}"));
                    let _ = tx.send(Response::Log(result));
                }
            }
            Request::Shutdown => {
                if let Some((_, mut out)) = writer.take() {
                    let _ = out.flush();
                }
                break;
            }
        }
    }
}

fn scan(base: &Path, game: Option<&Path>) -> Result<Vec<String>, String> {
    let assets = AssetSearchPath::open_game(base, game).map_err(|e| e.to_string())?;
    let mut names = assets.names()
        .filter_map(|qpath| {
            let lower = qpath.to_ascii_lowercase();
            if !lower.starts_with("strafetrails/") || !lower.ends_with(".cfg") { return None; }
            let stem = qpath.get("strafetrails/".len()..qpath.len().saturating_sub(4))?;
            sanitize_name(stem).ok()
        })
        .collect::<Vec<_>>();
    names.sort_by_key(|name| name.to_ascii_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    Ok(names)
}

fn load(base: &Path, game: Option<&Path>, name: &str, slot: u8) -> Result<LoadedTrail, String> {
    let mut assets = AssetSearchPath::open_game(base, game).map_err(|e| e.to_string())?;
    let qpath = format!("strafetrails/{name}.cfg");
    let asset = assets.read(&qpath, MAX_TRAIL_BYTES).map_err(|e| e.to_string())?
        .ok_or_else(|| format!("{qpath} not found"))?;
    let points = parse_points(&asset.bytes)?;
    if points.len() < 2 {
        return Err(format!("{qpath} contains fewer than two valid points"));
    }
    Ok(LoadedTrail::new(name.to_owned(), slot, &points))
}

fn parse_points(bytes: &[u8]) -> Result<Vec<[f32; 3]>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "trail CFG is not valid UTF-8/ASCII".to_owned())?;
    let tokens = text.split_ascii_whitespace().collect::<Vec<_>>();
    if tokens.len() % 3 != 0 {
        return Err(format!("expected XYZ triples; found {} scalar values", tokens.len()));
    }
    let count = tokens.len() / 3;
    if count > MAX_POINTS {
        return Err(format!("trail has {count} points; limit is {MAX_POINTS}"));
    }
    let mut points = Vec::with_capacity(count);
    for (index, xyz) in tokens.chunks_exact(3).enumerate() {
        let parse = |value: &str| value.parse::<f32>().ok().filter(|v| v.is_finite());
        let Some(x) = parse(xyz[0]) else { return Err(format!("invalid X at point {index}: {:?}", xyz[0])) };
        let Some(y) = parse(xyz[1]) else { return Err(format!("invalid Y at point {index}: {:?}", xyz[1])) };
        let Some(z) = parse(xyz[2]) else { return Err(format!("invalid Z at point {index}: {:?}", xyz[2])) };
        points.push([x, y, z]);
    }
    Ok(points)
}

fn sanitize_name(raw: &str) -> Result<String, String> {
    let name = raw.trim().trim_end_matches(".cfg");
    if name.is_empty() || name == "." || name == ".." || name.len() > 128 {
        return Err("trail name must be 1..128 characters".to_owned());
    }
    // TaystJK strips these before building strafetrails/<name>.cfg. Reject them
    // instead: callers get a useful error and Windows paths stay safe.
    if name.contains("..")
        || name.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '/' | '\\' | ':' | ';' | '?' | '*' | '<' | '>' | '|' | '"'))
    {
        return Err("trail name may not contain paths, whitespace, '..', or \\ / : ; ? * < > | \"".to_owned());
    }
    Ok(name.to_owned())
}

fn cell(point: [f32; 3]) -> (i32, i32) {
    ((point[0] / CELL_SIZE).floor() as i32, (point[1] / CELL_SIZE).floor() as i32)
}

fn squared_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    dx * dx + dy * dy + dz * dz
}

fn slot_color(slot: u8) -> [u8; 3] {
    // TaystJK GetTrailColorByClientNum for non-team clients. Loaded trails use
    // slot 28 by default, which intentionally lands on the legacy orange.
    let rgb = match slot {
        0 => 0xFFFF00,
        1 => 0xFF00FF,
        2 => 0x66CCCC,
        3 => 0x663366,
        4 => 0xCC6633,
        5 => 0x66FF00,
        6 => 0xFFFFFF,
        7 => 0x00CC33,
        8 => 0x99FFFF,
        9 => 0xCC66FF,
        10 => 0x330066,
        11..=19 => 0xFFFF00,
        _ => 0xFF9900,
    };
    // Legacy packed colours are read low byte first by shaderRGBA.
    [(rgb & 0xff) as u8, ((rgb >> 8) & 0xff) as u8, ((rgb >> 16) & 0xff) as u8]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_legacy_integer_cfg_and_float_cfg() {
        assert_eq!(parse_points(b"1 2 3\n4.5 -6 7\n").unwrap(), vec![[1.0, 2.0, 3.0], [4.5, -6.0, 7.0]]);
    }

    #[test]
    fn loaded_trail_breaks_teleports() {
        let points = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1000.0, 0.0, 0.0], [1001.0, 0.0, 0.0]];
        let trail = LoadedTrail::new("test".into(), 0, &points);
        assert_eq!(trail.segment_count, 2);
    }

    #[test]
    fn builds_one_second_plums_from_recorded_fps() {
        let points = (0..=80).map(|x| [x as f32, 0.0, 4.0]).collect::<Vec<_>>();
        let mut trail = LoadedTrail::new("test".into(), 0, &points);
        trail.refresh_markers(40);
        assert_eq!(trail.markers.len(), 2);
        assert_eq!(trail.markers[0], TimeMarker { origin: [39.0, 0.0, 12.0], seconds: 1 });
        assert_eq!(trail.markers[1], TimeMarker { origin: [79.0, 0.0, 12.0], seconds: 2 });

        trail.refresh_markers(20);
        assert_eq!(trail.markers.len(), 4);
        assert_eq!(trail.markers[3].seconds, 4);
    }

    #[test]
    fn rdp_lod_keeps_turns_and_drops_collinear_points() {
        let points = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 2.0, 0.0]];
        let simplified = simplify_points(&points, 0.25);
        assert_eq!(simplified, vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 2.0, 0.0]]);
    }

    #[test]
    fn trail_view_rejects_spheres_behind_camera() {
        let view = TrailView {
            origin: [0.0, 0.0, 0.0],
            forward: [1.0, 0.0, 0.0],
            left: [0.0, 1.0, 0.0],
            up: [0.0, 0.0, 1.0],
            tan_half_fov_x: 1.0,
            tan_half_fov_y: 1.0,
            focal_length_pixels: 500.0,
        };
        assert!(view.sphere_visible([100.0, 0.0, 0.0], 1.0));
        assert!(!view.sphere_visible([-100.0, 0.0, 0.0], 1.0));
    }

    #[test]
    fn refuses_path_traversal_names() {
        assert!(sanitize_name("../evil").is_err());
        assert!(sanitize_name("folder/evil").is_err());
        assert_eq!(sanitize_name("dash1-jka.cfg").unwrap(), "dash1-jka");
    }
}
