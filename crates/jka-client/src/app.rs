mod egui_menu;
mod egui_settings;
mod frontend;
mod egui_theme;
mod quality;

use egui_menu::VideoSection;
use frontend::{DemoEntry, FrontendPage, SoloMapEntry};

use crate::{
    camera::{offset_third_person_view, Camera, ThirdPersonCameraState, ThirdPersonSettings, ThirdPersonViewInput},
    cgame::{
        entity_presenter::EntityPresenter,
        event_debug,
        event_presenter::{EventDispatchClass, EventPresenter},
        player_presenter::{Ghoul2PresentationView, PlayerPresenter},
        ragdoll::{PhysicsMapMesh, RagdollConfig},
        presented_openjk_player, snapshot_discontinuity,
        view::{local_player_alpha, rendering_third_person, PlayerViewPolicyState},
        ClientGameState, ClientInfo, EventCheckDisposition, PresentedEntity, ET_PLAYER,
    },
    config,
    keybinds::{self, BindKey, Bindings},
    player::{LocalPlayer, LocalPresentationSettings, MouseInputSettings},
    renderer::{
        ClientFramePerf, DynamicModelSurface, EguiRenderData, InputLatencySample, PostEffects,
        RenderCommand, RenderSnapshot, RenderThread,
    },
    runtime::{SurfaceInspectorInfo, UserEvent},
    scene::{self, SpawnPoint},
    server_browser::{self, BrowserCommand, BrowserEvent, BrowserUiState, ServerSource},
    ui::{
        self, ChatMode, CloudRenderResolution, CloudType, ColorLutPreset, ConsoleSearchMatch,
        ConsoleSelection, ConsoleSize, CullDebugMode, DofQuality, DynamicLightsMode,
        DynamicShadowsMode, EntityAmbientLightingMode, FogMode, FootprintMode, FullscreenMode,
        Ghoul2BatchMode, Ghoul2SkinningMode, HudElementId, HudLayout, HudState, MapLoadingBar, MapLoadingUi, OverlayMode, PerfStats,
        PlanarReflectionDebugMode, PvsMode, RainIntensity, ReflectionQuality, RendererBackend,
        SunVisibilityMode, TextureFilter, ThreadPerfStats, UiChatLine, UiScoreEntry, UiScoreboard, UiSnapshot,
        VideoSettings, VsyncMode,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet, VecDeque},
    fs::File,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    dpi::{PhysicalPosition, PhysicalSize},
    event::{DeviceEvent, ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy},
    keyboard::{KeyCode, ModifiersState, PhysicalKey},
    window::{CursorGrabMode, Fullscreen, Window, WindowId},
};

const FALLBACK_RESOLUTIONS: [[u32; 2]; 5] = [
    [1280, 800],
    [1600, 900],
    [1920, 1080],
    [2560, 1440],
    [3840, 2160],
];
const VIDEO_CONFIRM_TIMEOUT_SECS: u64 = 15;

/// The frontend is a real rendered world, not a 2D levelshot. Keep the map
/// fixed for now so menu art direction is deterministic; later this can become
/// a cvar or an authored list of menu scenes without changing the load path.
const FRONTEND_BACKGROUND_MAP: &str = "mp/duel3";
const FRONTEND_CAMERA_EYE_HEIGHT: f32 = 26.0;

use jka_movement::{CollisionWorld, JoinMode};
use jka_protocol::{
    demo::{self, DemoReader},
    server::{
        Decoder as ServerMessageDecoder, Event as ServerMessageEvent, Snapshot as ProtocolSnapshot,
    },
};

#[cfg(windows)]
fn demo_recording_timestamp() -> String {
    #[repr(C)]
    struct WinSystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    #[link(name = "Kernel32")]
    extern "system" {
        fn GetLocalTime(system_time: *mut WinSystemTime);
    }
    let mut local = std::mem::MaybeUninit::<WinSystemTime>::uninit();
    unsafe {
        GetLocalTime(local.as_mut_ptr());
        let local = local.assume_init();
        format!(
            "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
            local.year, local.month, local.day, local.hour, local.minute, local.second
        )
    }
}

#[cfg(not(windows))]
fn demo_recording_timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
        .to_string()
}

fn normalize_cfg_qpath(name: &str) -> Result<PathBuf, String> {
    let name = name.trim().trim_matches('"');
    if name.is_empty() {
        return Err("config filename is empty".to_owned());
    }
    let mut path = PathBuf::from(name.replace('\\', "/"));
    if path.extension().is_none() {
        path.set_extension("cfg");
    }
    if !path
        .extension()
        .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case("cfg"))
    {
        return Err("Only the .cfg extension is supported".to_owned());
    }
    if path.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        )
    }) {
        return Err("config path must stay inside the active game directory".to_owned());
    }
    Ok(path)
}

const MAX_CONSOLE_COMMAND_BUFFER_BYTES: usize = 128 * 1024;
const MAX_CONSOLE_COMMANDS_PER_FRAME: usize = 4096;

fn split_console_script(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut line_comment = false;
    let mut block_comment = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if line_comment {
            if c == '\n' || c == '\r' {
                line_comment = false;
                let command = current.trim();
                if !command.is_empty() {
                    out.push(command.to_owned());
                }
                current.clear();
            }
            i += 1;
            continue;
        }
        if block_comment {
            if c == '*' && next == Some('/') {
                block_comment = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if !quoted && c == '/' && next == Some('/') {
            line_comment = true;
            i += 2;
            continue;
        }
        if !quoted && c == '/' && next == Some('*') {
            block_comment = true;
            i += 2;
            continue;
        }
        if c == '"' {
            quoted = !quoted;
            current.push(c);
            i += 1;
            continue;
        }
        if !quoted && (c == ';' || c == '\n' || c == '\r') {
            let command = current.trim();
            if !command.is_empty() {
                out.push(command.to_owned());
            }
            current.clear();
            i += 1;
            continue;
        }
        current.push(c);
        i += 1;
    }
    let command = current.trim();
    if !command.is_empty() {
        out.push(command.to_owned());
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AppliedVideoMode {
    fullscreen: FullscreenMode,
    renderer_backend: RendererBackend,
    resolution: [u32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApplyLatchedScope {
    MapLoad,
    VidRestart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConsoleCvarSetResult {
    Applied,
    Latched,
    Unchanged,
}

#[derive(Debug, Clone, Copy)]
struct VideoConfirmation {
    previous: AppliedVideoMode,
    deadline: Instant,
    shown_seconds: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingRendererRestartStage {
    RecreateWindow,
    ApplyTargetDisplayMode,
    SpawnRenderer,
}

#[derive(Debug, Clone, Copy)]
struct PendingRendererRestart {
    previous: AppliedVideoMode,
    target: AppliedVideoMode,
    confirm_on_change: bool,
    stage: PendingRendererRestartStage,
    ready_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MapLoadPurpose {
    Gameplay,
    LivePrefetch,
    FrontendBackground,
}

#[derive(Debug, Clone, Copy)]
struct FrontendCinematic {
    position: [f32; 3],
    yaw: f32,
    started: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AntiAliasingChoice {
    Off,
    Fxaa,
    Smaa,
    Taa,
    Msaa(u32),
}

fn normalize_anti_aliasing(video: &mut VideoSettings) -> bool {
    let before = (video.msaa_samples, video.fxaa, video.smaa, video.taa);
    if video.taa {
        video.fxaa = false;
        video.smaa = false;
        video.msaa_samples = 1;
    } else if video.smaa {
        video.fxaa = false;
        video.msaa_samples = 1;
    } else if video.fxaa {
        video.msaa_samples = 1;
    }
    before != (video.msaa_samples, video.fxaa, video.smaa, video.taa)
}

struct MapLoadRequest {
    request_id: u64,
    started: Instant,
    game: Option<PathBuf>,
    source: scene::MapSource,
    prepare_options: scene::MapPrepareOptions,
}

struct PreparedMapCache {
    label: String,
    prepare_options: scene::MapPrepareOptions,
    map: Box<scene::PreparedMap>,
}

#[derive(Debug, Clone, Copy)]
struct MapLoadingBarState {
    task: crate::thread_activity::Task,
    label: &'static str,
    completed: u32,
    total: u32,
    skipped: bool,
}

#[derive(Debug, Clone)]
struct MapLoadingState {
    request_id: u64,
    name: String,
    preparation_finished: bool,
    bars: [MapLoadingBarState; 8],
}

impl MapLoadingState {
    fn new(request_id: u64, name: String) -> Self {
        use crate::thread_activity::Task;
        Self {
            request_id,
            name,
            preparation_finished: false,
            bars: [
                MapLoadingBarState {
                    task: Task::MapBspParse,
                    label: "BSP PARSE",
                    completed: 0,
                    total: 0,
                    skipped: false,
                },
                MapLoadingBarState {
                    task: Task::MapCollision,
                    label: "COLLISION",
                    completed: 0,
                    total: 0,
                    skipped: false,
                },
                MapLoadingBarState {
                    task: Task::MapShaderParse,
                    label: "SHADERS",
                    completed: 0,
                    total: 0,
                    skipped: false,
                },
                MapLoadingBarState {
                    task: Task::MapTextureDecode,
                    label: "TEXTURES",
                    completed: 0,
                    total: 0,
                    skipped: false,
                },
                MapLoadingBarState {
                    task: Task::MapGrass,
                    label: "GRASS",
                    completed: 0,
                    total: 0,
                    skipped: false,
                },
                MapLoadingBarState {
                    task: Task::MapGi,
                    label: "VOXEL GI",
                    completed: 0,
                    total: 0,
                    skipped: false,
                },
                MapLoadingBarState {
                    task: Task::MapOcean,
                    label: "OCEAN MESH",
                    completed: 0,
                    total: 0,
                    skipped: false,
                },
                MapLoadingBarState {
                    task: Task::MapAcoustics,
                    label: "ACOUSTICS",
                    completed: 0,
                    total: 0,
                    skipped: false,
                },
            ],
        }
    }

    fn skip_task(&mut self, task: crate::thread_activity::Task) {
        if let Some(bar) = self.bars.iter_mut().find(|bar| bar.task == task) {
            bar.skipped = true;
        }
    }

    fn update_worker(
        &mut self,
        task: crate::thread_activity::Task,
        completed: u32,
        total: u32,
    ) {
        if let Some(bar) = self.bars.iter_mut().find(|bar| bar.task == task) {
            bar.completed = completed.min(total);
            bar.total = total;
            bar.skipped = false;
        }
    }

    fn begin_upload(&mut self) {
        self.preparation_finished = true;
    }

    fn set_optional_task_presence(
        &mut self,
        task: crate::thread_activity::Task,
        present: bool,
        completed: bool,
    ) {
        let Some(bar) = self.bars.iter_mut().find(|bar| bar.task == task) else {
            return;
        };
        if !present {
            bar.completed = 0;
            bar.total = 0;
            bar.skipped = true;
            return;
        }
        bar.skipped = false;
        if bar.total == 0 {
            bar.total = 1;
            bar.completed = if completed { 1 } else { 0 };
        }
    }

    fn ui(&self) -> MapLoadingUi {
        use crate::thread_activity::Task;
        MapLoadingUi {
            map_name: self.name.clone(),
            preparation_finished: self.preparation_finished,
            bars: self
                .bars
                .iter()
                .map(|bar| MapLoadingBar {
                    label: bar.label,
                    completed: bar.completed,
                    total: bar.total,
                    // Grass/ocean/acoustics are optional or map-content-dependent.
                    // Do not show empty placeholder rows while unavailable.
                    skipped: bar.skipped
                        || (bar.total == 0
                            && matches!(
                                bar.task,
                                Task::MapGrass | Task::MapOcean | Task::MapAcoustics
                            )),
                })
                .collect(),
        }
    }
}

fn prepared_map_has_ocean_surface(map: &scene::PreparedMap) -> bool {
    map.batches.iter().any(|batch| {
        if !batch.water_primary {
            return false;
        }
        let start = batch.vertices.start as usize;
        let end = batch.vertices.end as usize;
        let Some(vertices) = map.vertices.get(start..end) else {
            return false;
        };
        let mut lowest = f32::INFINITY;
        let mut highest = f32::NEG_INFINITY;
        let mut facing = 0.0f32;
        for vertex in vertices {
            lowest = lowest.min(vertex.position[1]);
            highest = highest.max(vertex.position[1]);
            facing += vertex.normal[1];
        }
        // Keep this in lock-step with renderer::is_upward_water_face: only the
        // upward horizontal face of a water brush receives an FFT ocean mesh.
        lowest.is_finite() && facing > 0.0 && lowest >= highest - 1.0
    })
}

struct ChatRecord {
    text: String,
    created: Instant,
}

struct CenterPrintRecord {
    text: String,
    created: Instant,
}

struct DemoRecording {
    file: File,
    path: PathBuf,
    waiting_for_full_snapshot: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ConsolePoint {
    line: usize,
    col: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionPhase {
    WaitingForMap,
    /// Live only: map loaded and usercmds flowing (CA_PRIMED), waiting for
    /// the first snapshot without SNAPFLAG_NOT_ACTIVE.
    WaitingForSnapshot,
    Playing,
}

#[derive(Debug, Clone, Copy)]
struct DemoCameraSample {
    server_time: i32,
    /// Actor origin in native JKA coordinates. OpenJK third-person starts here.
    native_origin: [f32; 3],
    eye_position: [f32; 3],
    player_position: [f32; 3],
    view_angles: [f32; 3],
    view_height: i32,
    dead_yaw: f32,
    client_num: i32,
    policy: PlayerViewPolicyState,
    teleported: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DemoAdvance {
    Running,
    Completed,
}

#[derive(Debug, Clone, Copy, Default)]
struct DemoEventStat {
    count: u64,
    handled: u64,
    partial: u64,
    unhandled: u64,
    first_server_time: i32,
    last_server_time: i32,
}

#[derive(Debug, Clone, Copy)]
struct DemoTimeline {
    wall_anchor: Instant,
    demo_anchor_ms: f64,
    rate: f64,
    resume_rate: f64,
}

impl DemoTimeline {
    fn new(first_server_time: i32, now: Instant) -> Self {
        Self {
            wall_anchor: now,
            demo_anchor_ms: f64::from(first_server_time),
            rate: 1.0,
            resume_rate: 1.0,
        }
    }

    fn target_time(self, now: Instant) -> i32 {
        let elapsed_ms = now.saturating_duration_since(self.wall_anchor).as_secs_f64() * 1000.0;
        let value = self.demo_anchor_ms + elapsed_ms * self.rate;
        value.clamp(f64::from(i32::MIN), f64::from(i32::MAX)).round() as i32
    }

    fn set_rate(&mut self, now: Instant, rate: f64) {
        self.demo_anchor_ms = f64::from(self.target_time(now));
        self.wall_anchor = now;
        self.rate = rate;
        if rate > 0.0 {
            self.resume_rate = rate;
        }
    }

    fn toggle_pause(&mut self, now: Instant) {
        if self.rate == 0.0 {
            self.set_rate(now, self.resume_rate.max(0.01));
        } else {
            self.set_rate(now, 0.0);
        }
    }
}

/// One CGame lifetime (OpenJK CL_InitCGame .. CL_ShutdownCGame) fed either by
/// a demo file or by a live connection. Both go through the same snapshot
/// transition, event, and presentation path.
/// Live prediction keeps the display-only provisional command separate from
/// the committed `cg.predictedPlayerState`. OpenJK transitions predictable
/// events from the committed state; presentation/camera may still use the
/// provisional state for sub-command-frame smoothness.
struct LivePredictionFrame {
    display: jka_protocol::server::PlayerState,
    committed: jka_protocol::server::PlayerState,
    previous_committed: jka_protocol::server::PlayerState,
    error: [f32; 3],
}

struct GameSession {
    /// Live sessions take snapshots from `live_snapshots` (filled by the
    /// network pump) instead of the demo reader, and their clock comes from
    /// CL_SetCGameTime rather than the demo timeline.
    live: bool,
    live_snapshots: VecDeque<ProtocolSnapshot>,
    qpath: String,
    source: String,
    reader: DemoReader<Cursor<Vec<u8>>>,
    decoder: ServerMessageDecoder,
    messages: usize,
    snapshots: usize,
    phase: SessionPhase,
    map_name: Option<String>,
    pending_snapshot: Option<ProtocolSnapshot>,
    current_snapshot: Option<ProtocolSnapshot>,
    next_snapshot: Option<ProtocolSnapshot>,
    first_server_time: Option<i32>,
    timeline: Option<DemoTimeline>,
    eof: bool,
    player_position: Option<glam::Vec3>,
    client_game: ClientGameState,
    siege_classes: Vec<jka_assets::siege::SiegeClassVisual>,
    presented_entities: Vec<PresentedEntity>,
    player_presenter: PlayerPresenter,
    entity_presenter: EntityPresenter,
    event_presenter: EventPresenter,
    sound_presenter: Option<crate::cgame::sound_presenter::SoundPresenter>,
    /// OpenJK FX system driven by CGame (missile trails, impacts, effect events).
    weapon_fx: crate::cgame::weapon_fx::WeaponFx,
    /// This frame's FX primitives, tessellated once the final camera is known.
    fx_draws: Vec<crate::fx::system::FxDraw>,
    fx_surfaces: Arc<Vec<DynamicModelSurface>>,
    dynamic_models: Arc<Vec<DynamicModelSurface>>,
    client_perf: ClientFramePerf,
    /// CG_Mover inline BSP models for the current presentation frame.
    inline_models: Arc<Vec<crate::renderer::InlineModelInstance>>,
    logged_entity_summary: bool,
    logged_dispatch_summary: bool,
    event_debug_lines: VecDeque<String>,
    event_stats: BTreeMap<i32, DemoEventStat>,
    event_suppressed_duplicate: u64,
    event_suppressed_zero: u64,
}

/// CPU-heavy CGame assets prepared while the challenge/gamestate handshake is
/// still in flight. Sound device creation intentionally remains on the main
/// thread; the expensive VFS/material/presenter work is safe to overlap.
struct LiveCgamePrepared {
    siege_classes: Vec<jka_assets::siege::SiegeClassVisual>,
    player_presenter: PlayerPresenter,
    entity_presenter: EntityPresenter,
    fx_assets: jka_assets::pk3::AssetSearchPath,
    stringed: Option<crate::cgame::stringed::StringEd>,
    elapsed_ms: f64,
}

#[derive(Debug, Clone)]
struct LiveJoinTiming {
    started: Instant,
    server_info_ms: Option<f64>,
    cgame_ready_ms: Option<f64>,
    gamestate_ms: Option<f64>,
    first_world_frame_ms: Option<f64>,
    prefetched_map: Option<String>,
}

impl LiveJoinTiming {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            server_info_ms: None,
            cgame_ready_ms: None,
            gamestate_ms: None,
            first_world_frame_ms: None,
            prefetched_map: None,
        }
    }

    fn elapsed_ms(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1000.0
    }
}

fn probe_demo_fs_game(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut reader = DemoReader::new(Cursor::new(bytes));
    let mut decoder = ServerMessageDecoder::new();
    let mut setgame = Vec::new();
    let mut messages = 0usize;
    loop {
        let record = reader
            .next_record()
            .map_err(|error| format!("DEMO FS_GAME PROBE FRAMING ERROR: {error}"))?
            .ok_or_else(|| "DEMO REACHED EOF BEFORE GAMESTATE".to_owned())?;
        let packet = decoder.parse_packet(record.sequence, &record.payload).map_err(|error| {
            format!(
                "DEMO FS_GAME PROBE PROTOCOL ERROR: message {messages} sequence {}: {error}",
                record.sequence
            )
        })?;
        messages += 1;
        for event in &packet.events {
            match event {
                ServerMessageEvent::SetGame(game) => setgame = game.clone(),
                ServerMessageEvent::Gamestate { .. } => {
                    let system_game = decoder
                        .configstrings
                        .get(&jka_protocol::session::CS_SYSTEMINFO)
                        .and_then(|info| jka_protocol::commands::info_value(info, b"fs_game"))
                        .unwrap_or_default();
                    return Ok(if system_game.is_empty() {
                        setgame
                    } else {
                        system_game.to_vec()
                    });
                }
                _ => {}
            }
        }
        if messages >= 4096 {
            return Err("DEMO FS_GAME PROBE DID NOT REACH A GAMESTATE".to_owned());
        }
    }
}

impl GameSession {
    fn new(
        qpath: String,
        source: String,
        bytes: Vec<u8>,
        siege_classes: Vec<jka_assets::siege::SiegeClassVisual>,
        player_presenter: PlayerPresenter,
        entity_presenter: EntityPresenter,
        weapon_fx: crate::cgame::weapon_fx::WeaponFx,
    ) -> Self {
        Self {
            live: false,
            live_snapshots: VecDeque::new(),
            qpath,
            source,
            reader: DemoReader::new(Cursor::new(bytes)),
            decoder: ServerMessageDecoder::new(),
            messages: 0,
            snapshots: 0,
            phase: SessionPhase::WaitingForMap,
            map_name: None,
            pending_snapshot: None,
            current_snapshot: None,
            next_snapshot: None,
            first_server_time: None,
            timeline: None,
            eof: false,
            player_position: None,
            client_game: ClientGameState::new(),
            siege_classes,
            presented_entities: Vec::new(),
            player_presenter,
            entity_presenter,
            event_presenter: EventPresenter::default(),
            sound_presenter: None,
            weapon_fx,
            fx_draws: Vec::new(),
            fx_surfaces: Arc::new(Vec::new()),
            dynamic_models: Arc::new(Vec::new()),
            client_perf: ClientFramePerf::default(),
            inline_models: Arc::new(Vec::new()),
            logged_entity_summary: false,
            logged_dispatch_summary: false,
            event_debug_lines: VecDeque::with_capacity(256),
            event_stats: BTreeMap::new(),
            event_suppressed_duplicate: 0,
            event_suppressed_zero: 0,
        }
    }

    fn push_event_debug_line(&mut self, line: String) {
        if self.event_debug_lines.len() >= 512 {
            self.event_debug_lines.pop_front();
        }
        self.event_debug_lines.push_back(line);
    }

    fn ridden_vehicle_definition(&self, vehicle_num: i32) -> Option<jka_assets::vehicle::VehicleDefinition> {
        if vehicle_num <= 0 {
            return None;
        }
        let entity = self
            .presented_entities
            .iter()
            .find(|entity| i32::from(entity.number) == vehicle_num)?;
        let model_index = entity.state.field_i32("modelindex").unwrap_or(0);
        let requested = self.client_game.model_qpath(model_index)?;
        self.player_presenter
            .vehicle_definition_for_model_request(&requested)
            .cloned()
    }

    fn take_event_debug_lines(&mut self) -> Vec<String> {
        self.event_debug_lines.drain(..).collect()
    }

    fn record_event_stat(&mut self, event: i32, server_time: i32, class: EventDispatchClass) {
        let stat = self.event_stats.entry(event).or_insert(DemoEventStat {
            count: 0,
            handled: 0,
            partial: 0,
            unhandled: 0,
            first_server_time: server_time,
            last_server_time: server_time,
        });
        stat.count = stat.count.saturating_add(1);
        match class {
            EventDispatchClass::Handled => stat.handled = stat.handled.saturating_add(1),
            EventDispatchClass::Partial => stat.partial = stat.partial.saturating_add(1),
            EventDispatchClass::Unhandled => stat.unhandled = stat.unhandled.saturating_add(1),
        }
        stat.last_server_time = server_time;
    }

    fn event_stats_lines(&self) -> Vec<String> {
        if self.event_stats.is_empty() {
            return vec!["^3CG EVENT STATS:^7 no accepted events observed yet".into()];
        }
        let total: u64 = self.event_stats.values().map(|stat| stat.count).sum();
        let mut rows: Vec<_> = self.event_stats.iter().collect();
        rows.sort_by(|(event_a, stat_a), (event_b, stat_b)| {
            stat_b
                .count
                .cmp(&stat_a.count)
                .then_with(|| event_a.cmp(event_b))
        });
        let mut lines = vec![format!(
            "^3CG EVENT STATS:^7 accepted={} unique={} suppressedDuplicate={} suppressedZero={}  H/P/U columns show presentation result",
            total,
            rows.len(),
            self.event_suppressed_duplicate,
            self.event_suppressed_zero,
        )];
        lines.extend(rows.into_iter().map(|(&event, stat)| {
            format!(
                "  {:>5}  {:<28} id={:<3} cat={:<10} H/P/U={}/{}/{} first={} last={}",
                stat.count,
                event_debug::event_name(event),
                event,
                event_debug::event_category(event),
                stat.handled,
                stat.partial,
                stat.unhandled,
                stat.first_server_time,
                stat.last_server_time,
            )
        }));
        lines
    }

    fn clear_event_stats(&mut self) {
        self.event_stats.clear();
        self.event_suppressed_duplicate = 0;
        self.event_suppressed_zero = 0;
    }

    fn read_next_message(&mut self) -> Result<Option<ProtocolSnapshot>, String> {
        let record = match self.reader.next_record() {
            Ok(Some(record)) => record,
            Ok(None) => {
                self.eof = true;
                println!(
                    "DEMO EOF: {} messages, {} valid snapshots",
                    self.messages, self.snapshots
                );
                return Ok(None);
            }
            Err(error) => {
                return Err(format!(
                    "DEMO FRAMING ERROR AFTER MESSAGE {}: {error}",
                    self.messages
                ));
            }
        };

        let message_index = self.messages;
        let sequence = record.sequence;
        let packet = self.decoder.parse_packet(sequence, &record.payload).map_err(|error| {
            format!(
                "DEMO PROTOCOL ERROR: message {message_index} sequence {sequence} byte~{} bit {}: {}",
                error.bit / 8,
                error.bit,
                error
            )
        })?;
        self.messages += 1;

        let mut decoded_snapshot = None;
        for event in &packet.events {
            match event {
                ServerMessageEvent::Nop => {}
                ServerMessageEvent::Gamestate {
                    server_command_sequence,
                    configstrings,
                    baselines,
                    client_number,
                    checksum_feed,
                } => {
                    println!(
                        "DEMO svc_gamestate: sequence={} serverCommandSequence={} configstrings={} baselines={} clientNum={} checksumFeed=0x{:08x}",
                        sequence,
                        server_command_sequence,
                        configstrings,
                        baselines,
                        client_number,
                        checksum_feed
                    );
                    if let Some(map) = self.decoder.map_name() {
                        if self.map_name.as_deref() != Some(map.as_str()) {
                            println!("DEMO map: {map}");
                        }
                        self.map_name = Some(map);
                    }
                    self.client_game.reset_gamestate(
                        &self.decoder.configstrings,
                        *server_command_sequence,
                    );
                    self.event_presenter.clear();
                    if let Some(sound) = &mut self.sound_presenter { sound.clear(); }
                }
                ServerMessageEvent::ServerCommand(command) => {
                    self.client_game.queue_server_command(command.clone());
                    println!(
                        "DEMO svc_serverCommand #{}: {}",
                        command.sequence,
                        String::from_utf8_lossy(&command.text)
                    );
                }
                ServerMessageEvent::Snapshot {
                    server_time,
                    message_num,
                    delta_num,
                    entities,
                } => {
                    self.snapshots += 1;
                    if self.snapshots == 1 || self.snapshots % 60 == 0 {
                        println!(
                            "DEMO svc_snapshot: serverTime={} message={} delta={} entities={} decodedSnapshots={}",
                            server_time,
                            message_num,
                            delta_num,
                            entities,
                            self.snapshots
                        );
                    }
                    decoded_snapshot = self.decoder.latest_snapshot().cloned();
                }
                ServerMessageEvent::SetGame(game) => {
                    println!("DEMO svc_setgame: {}", String::from_utf8_lossy(game));
                }
                ServerMessageEvent::MapChange => println!("DEMO svc_mapchange"),
                ServerMessageEvent::Download => println!("DEMO svc_download"),
            }
        }

        Ok(decoded_snapshot)
    }

    fn set_playback_rate(&mut self, now: Instant, rate: f64) -> Result<(), String> {
        if !rate.is_finite() || rate < 0.0 || rate > 100.0 {
            return Err("DEMO SPEED MUST BE BETWEEN 0 AND 100 (REVERSE NEEDS CHECKPOINTS)".into());
        }
        let timeline = self
            .timeline
            .as_mut()
            .ok_or_else(|| "DEMO PLAYBACK CLOCK WAS NOT INITIALIZED".to_owned())?;
        timeline.set_rate(now, rate);
        if let Some(sound) = &mut self.sound_presenter { sound.set_rate(rate as f32); }
        Ok(())
    }

    fn toggle_pause(&mut self, now: Instant) -> Result<f64, String> {
        let timeline = self
            .timeline
            .as_mut()
            .ok_or_else(|| "DEMO PLAYBACK CLOCK WAS NOT INITIALIZED".to_owned())?;
        timeline.toggle_pause(now);
        if let Some(sound) = &mut self.sound_presenter { sound.set_rate(timeline.rate as f32); }
        Ok(timeline.rate)
    }

    fn playback_rate(&self) -> Option<f64> {
        self.timeline.map(|timeline| timeline.rate)
    }

    fn read_until_map(&mut self) -> Result<String, String> {
        loop {
            // A normal JKA gamestate packet ends before the first snapshot, but
            // preserve a snapshot if a compatible producer places both in one
            // server message. The shared decoder has already consumed the whole
            // packet, so dropping that snapshot here would move playback's first
            // visible frame forward by one message.
            if let Some(snapshot) = self.read_next_message()? {
                if self.pending_snapshot.is_none() {
                    self.pending_snapshot = Some(snapshot);
                }
            }
            if let Some(map) = self.map_name.clone().or_else(|| self.decoder.map_name()) {
                self.map_name = Some(map.clone());
                return Ok(map);
            }
            if self.eof {
                return Err(format!("DEMO REACHED EOF BEFORE GAMESTATE MAP: {}", self.qpath));
            }
        }
    }

    fn read_next_snapshot(&mut self) -> Result<Option<ProtocolSnapshot>, String> {
        if self.live {
            // CG_ReadNextSnapshot: nothing new yet simply means extrapolate.
            return Ok(self.live_snapshots.pop_front());
        }
        loop {
            if self.eof {
                return Ok(None);
            }
            if let Some(snapshot) = self.read_next_message()? {
                return Ok(Some(snapshot));
            }
        }
    }

    fn begin_after_map_load(&mut self, now: Instant) -> Result<DemoCameraSample, String> {
        const SNAPFLAG_NOT_ACTIVE: u8 = 1 << 1;

        // CL_FirstSnapshot ignores connection/zombie snapshots carrying
        // SNAPFLAG_NOT_ACTIVE. Do the same before establishing our playback time
        // base so the first visible frame is an actually active game snapshot.
        let first = loop {
            let candidate = if let Some(snapshot) = self.pending_snapshot.take() {
                Some(snapshot)
            } else {
                self.read_next_snapshot()?
            };
            let snapshot = candidate
                .ok_or_else(|| format!("DEMO HAS NO ACTIVE SNAPSHOT: {}", self.qpath))?;
            if snapshot.snap_flags & SNAPFLAG_NOT_ACTIVE == 0 {
                break snapshot;
            }
        };
        self.first_server_time = Some(first.server_time);
        self.client_game.set_initial_snapshot(&first)?;
        self.current_snapshot = Some(first);
        self.next_snapshot = self.read_next_snapshot()?;
        self.client_game.set_next_snapshot(self.next_snapshot.as_ref())?;
        self.timeline = Some(DemoTimeline::new(self.first_server_time.unwrap(), now));
        self.phase = SessionPhase::Playing;
        self.sample_at(self.first_server_time.unwrap(), false)
            .ok_or_else(|| "DEMO FIRST SNAPSHOT HAS NO CAMERA PLAYERSTATE".to_owned())
    }

    /// Live CG_Init/CG_ProcessSnapshots start: begin from the newest active
    /// snapshot received while the map loaded. Returns false until one exists.
    fn begin_live(&mut self) -> Result<bool, String> {
        const SNAPFLAG_NOT_ACTIVE: u8 = 1 << 1;
        let Some(index) = self
            .live_snapshots
            .iter()
            .rposition(|snapshot| snapshot.snap_flags & SNAPFLAG_NOT_ACTIVE == 0)
        else {
            // Inactive snapshots only mean the server has not entered us yet.
            let keep_from = self.live_snapshots.len().saturating_sub(1);
            self.live_snapshots.drain(..keep_from);
            return Ok(false);
        };
        self.live_snapshots.drain(..index);
        let first = self.live_snapshots.pop_front().expect("indexed above");
        self.first_server_time = Some(first.server_time);
        self.client_game.set_initial_snapshot(&first)?;
        self.current_snapshot = Some(first);
        self.next_snapshot = self.read_next_snapshot()?;
        self.client_game.set_next_snapshot(self.next_snapshot.as_ref())?;
        self.phase = SessionPhase::Playing;
        Ok(true)
    }

    fn advance(
        &mut self,
        now: Instant,
        third_person: ThirdPersonSettings,
        first_person_lightsaber: bool,
        debug_events: u8,
        ghoul2_view: Option<Ghoul2PresentationView>,
    ) -> Result<(DemoAdvance, DemoCameraSample), String> {
        if self.phase != SessionPhase::Playing {
            return Err("DEMO PLAYBACK ADVANCED BEFORE MAP LOAD".to_owned());
        }
        let target_server_time = self
            .timeline
            .ok_or_else(|| "DEMO PLAYBACK CLOCK WAS NOT INITIALIZED".to_owned())?
            .target_time(now);
        self.advance_to(
            target_server_time,
            third_person,
            first_person_lightsaber,
            debug_events,
            ghoul2_view,
        )
    }

    fn advance_to(
        &mut self,
        target_server_time: i32,
        third_person: ThirdPersonSettings,
        first_person_lightsaber: bool,
        debug_events: u8,
        ghoul2_view: Option<Ghoul2PresentationView>,
    ) -> Result<(DemoAdvance, DemoCameraSample), String> {
        self.advance_to_with_prediction(
            target_server_time,
            third_person,
            first_person_lightsaber,
            debug_events,
            ghoul2_view,
            None,
        )
    }

    /// `predict` is CG_PredictPlayerState: given this frame's snap/nextSnap it
    /// returns cg.predictedPlayerState plus the decaying view error.
    fn advance_to_with_prediction(
        &mut self,
        target_server_time: i32,
        third_person: ThirdPersonSettings,
        first_person_lightsaber: bool,
        debug_events: u8,
        ghoul2_view: Option<Ghoul2PresentationView>,
        predict: Option<&mut dyn FnMut(&ProtocolSnapshot, Option<&ProtocolSnapshot>, &[PresentedEntity]) -> Result<Option<LivePredictionFrame>, String>>,
    ) -> Result<(DemoAdvance, DemoCameraSample), String> {
        self.client_perf = ClientFramePerf::default();
        self.player_presenter.begin_perf_frame();
        self.client_perf.ghoul2_skinning_mode = self.player_presenter.skinning_mode();
        let snapshot_started = Instant::now();
        let current_time = self
            .current_snapshot
            .as_ref()
            .map(|snapshot| snapshot.server_time)
            .ok_or_else(|| "DEMO HAS NO CURRENT SNAPSHOT".to_owned())?;
        // Live cl.serverTime can trail the newest snapshot right after the
        // first one or a time reset; CG_ProcessSnapshots simply holds there.
        let target_server_time = if self.live { target_server_time.max(current_time) } else { target_server_time };
        if target_server_time < current_time {
            return Err(format!(
                "DEMO REVERSE/SEEK REQUIRES CHECKPOINT RESTORE: target {target_server_time} < current snapshot {current_time}"
            ));
        }

        if let (Some(current), Some(next)) = (&self.current_snapshot, &self.next_snapshot) {
            if next.server_time < current.server_time {
                return Err(format!(
                    "DEMO SERVER TIME WENT BACKWARDS: message {} time {} after message {} time {}",
                    next.message_num, next.server_time, current.message_num, current.server_time
                ));
            }
        }

        // CG_ProcessSnapshots: with no nextSnap, try to read one before
        // deciding to extrapolate. Live snapshots arrive between frames.
        if self.next_snapshot.is_none() {
            self.next_snapshot = self.read_next_snapshot()?;
            if self.next_snapshot.is_some() {
                self.client_game.set_next_snapshot(self.next_snapshot.as_ref())?;
            }
        }

        let mut camera_teleported = false;
        while self
            .next_snapshot
            .as_ref()
            .is_some_and(|snapshot| target_server_time >= snapshot.server_time)
        {
            if let (Some(current), Some(next)) = (&self.current_snapshot, &self.next_snapshot) {
                camera_teleported |= snapshot_discontinuity(current, next);
            }
            self.client_game.transition_snapshot(target_server_time)?;
            self.current_snapshot = self.next_snapshot.take();
            self.next_snapshot = self.read_next_snapshot()?;
            self.client_game.set_next_snapshot(self.next_snapshot.as_ref())?;
        }

        // FX_AdjustTime: effects spawned by this frame's events and entities
        // start at cg.time.
        self.weapon_fx.begin_frame(target_server_time);
        self.presented_entities = self.client_game.present_entities(target_server_time)?;
        let predicted = match (predict, self.current_snapshot.as_ref()) {
            (Some(predict), Some(current)) => predict(current, self.next_snapshot.as_ref(), &self.presented_entities)?,
            _ => None,
        };
        if let Some(prediction) = &predicted {
            // CG_TransitionPlayerState / CG_CheckPlayerstateEvents: use the
            // committed prediction, never the provisional display-only cmd.
            self.client_game.transition_predicted_player_state(
                &prediction.committed,
                &prediction.previous_committed,
                target_server_time,
            )?;
        }
        let sample = match &predicted {
            Some(prediction) => sample_player_state(
                &prediction.display,
                target_server_time,
                camera_teleported,
                prediction.error,
            ),
            None => self.sample_at(target_server_time, camera_teleported),
        }
        .ok_or_else(|| "DEMO CURRENT SNAPSHOT HAS NO CAMERA PLAYERSTATE".to_owned())?;
        self.client_perf.snapshot_ms = snapshot_started.elapsed().as_secs_f64() * 1000.0;
        // Presentation needs a renderer view before App::apply_demo_camera runs.
        // First person can use this frame's exact sampled eye immediately. Third
        // person needs collision/orbit state owned by App, so it uses the previous
        // rendered third-person camera as a one-frame hint. Never reuse that hint
        // across a teleport/snapshot discontinuity.
        let ghoul2_view = if sample.teleported {
            None
        } else if !rendering_third_person(third_person, first_person_lightsaber, sample.policy) {
            ghoul2_view.map(|view| {
                let yaw = sample.view_angles[1].to_radians();
                let pitch = -sample.view_angles[0].to_radians();
                let (sy, cy) = yaw.sin_cos();
                let (sp, cp) = pitch.sin_cos();
                view.with_pose(sample.eye_position, [cy * cp, sp, -sy * cp])
            })
        } else {
            ghoul2_view
        };
        let audio_started = Instant::now();
        if let Some(sound) = &mut self.sound_presenter {
            sound.update_loops(
                &self.client_game,
                &self.siege_classes,
                &self.presented_entities,
                sample.client_num as u16,
            );
            let yaw = sample.view_angles[1].to_radians();
            sound.frame(crate::audio::Listener {
                entity: sample.client_num as u16,
                origin: scene::jka_position(sample.eye_position),
                left: [-yaw.sin(), yaw.cos(), 0.0],
            }, &self.presented_entities);
        }
        self.client_perf.audio_ms += audio_started.elapsed().as_secs_f64() * 1000.0;

        let events_started = Instant::now();
        // OpenJK's cg_debugEvents only prints accepted CG_EntityEvent cases.
        // Keep level 1 similarly compact; level 2 additionally explains why
        // candidate events were suppressed by CG_CheckEvents.
        let traces = self.client_game.drain_event_check_traces();
        for trace in &traces {
            match trace.disposition {
                EventCheckDisposition::Duplicate => {
                    self.event_suppressed_duplicate =
                        self.event_suppressed_duplicate.saturating_add(1);
                }
                EventCheckDisposition::Zero => {
                    self.event_suppressed_zero = self.event_suppressed_zero.saturating_add(1);
                }
                EventCheckDisposition::Accepted | EventCheckDisposition::NoEvent => {}
            }
        }
        if debug_events >= 2 {
            for trace in &traces {
                if let Some(line) = event_debug::trace_line(trace) {
                    self.push_event_debug_line(line);
                }
            }
        }

        for event in self.client_game.drain_presentation_events() {
            let fx_dispatch = self.weapon_fx.entity_event(&event, &self.client_game);
            let sound_dispatch_started = Instant::now();
            let sound_dispatch = self.sound_presenter.as_mut()
                .and_then(|sound| sound.dispatch(&event, &self.client_game, &self.siege_classes, target_server_time));
            self.client_perf.audio_ms += sound_dispatch_started.elapsed().as_secs_f64() * 1000.0;
            let dispatch = sound_dispatch
                .or(fx_dispatch)
                .unwrap_or_else(|| self.event_presenter.dispatch(&event, &self.client_game));
            self.record_event_stat(event.event, event.server_time, dispatch.class());
            if debug_events >= 1 {
                self.push_event_debug_line(event_debug::accepted_line(&event, &dispatch.status()));
            }
            if debug_events >= 3 {
                for line in event_debug::verbose_lines(&event, &self.client_game) {
                    self.push_event_debug_line(line);
                }
            }
        }
        if !self.logged_entity_summary {
            let summary = self.client_game.summarize_entities();
            println!(
                "DEMO ENTITY TYPES: total={} players={} npcs={} movers={} missiles={} items={} eventEntities={} other={}",
                summary.total,
                summary.players,
                summary.npcs,
                summary.movers,
                summary.missiles,
                summary.items,
                summary.event_entities,
                summary.other,
            );
            println!(
                "DEMO GAME RULES: gametype={} weaponDisable=0x{:08x}",
                self.client_game.gametype(),
                self.client_game.weapon_disable_mask() as u32,
            );
            for entity in self
                .presented_entities
                .iter()
                .filter(|entity| entity.entity_type == crate::cgame::ET_PLAYER)
            {
                let client_num = entity
                    .state
                    .field_i32("clientNum")
                    .unwrap_or(i32::from(entity.number));
                if client_num < 0 {
                    continue;
                }
                match self
                    .client_game
                    .client_info(client_num as usize, &self.siege_classes)
                {
                    Some(info) => println!(
                        "  ET_PLAYER entity={} clientNum={} name={:?} model={}/{} glm={} skin={} siegeclass={:?}",
                        entity.number,
                        client_num,
                        info.name,
                        info.model_name,
                        info.skin_name,
                        info.model_qpath(),
                        info.skin_qpath(),
                        info.siege_class,
                    ),
                    None => println!(
                        "  ET_PLAYER entity={} clientNum={} has no CS_PLAYERS configstring",
                        entity.number, client_num
                    ),
                }
            }
            self.logged_entity_summary = true;
        }
        self.client_perf.events_ms = events_started.elapsed().as_secs_f64() * 1000.0;

        self.player_position = Some(glam::Vec3::from_array(sample.player_position));

        // CG_AddEntities adds cg.predictedPlayerEntity for the local player.
        let followed_entity = match &predicted {
            Some(prediction) => crate::cgame::presented_player_state_entity(&prediction.display),
            None => self.client_game.present_followed_player(target_server_time),
        };
        let followed_entity_num = followed_entity.as_ref().map(|entity| entity.number);
        // forceGripCripple is one of the few exact victim-side grip signals in
        // vanilla playerState. Remote entityState does not transmit
        // forceGripEntityNum, so PlayerPresenter reconstructs those targets
        // from each active gripper's view trace.
        let local_force_gripped = predicted
            .as_ref()
            .map(|prediction| prediction.display.field_i32("fd.forceGripCripple").unwrap_or(0) != 0)
            .or_else(|| {
                self.current_snapshot.as_ref().map(|snapshot| {
                    snapshot.player_state.field_i32("fd.forceGripCripple").unwrap_or(0) != 0
                })
            })
            .unwrap_or(false);
        self.player_presenter.prepare_force_grip_targets(
            &self.presented_entities,
            followed_entity.as_ref(),
            local_force_gripped,
            target_server_time,
        );

        // Broadsword historically advanced ragdoll state immediately before
        // Ghoul2 submission. Keep the same presentation ownership: Rapier is
        // stepped once here, while OpenJK snapshot/pmove state stays untouched.
        self.player_presenter.begin_physics_frame(target_server_time);

        let entity_present_started = Instant::now();
        let (mut dynamic_models, inline_models, dispatch_summary) = self
            .entity_presenter
            .present_snapshot_entities(
                &self.presented_entities,
                &self.client_game,
                target_server_time,
                &mut self.player_presenter,
                &mut self.weapon_fx,
            );
        self.client_perf.entity_present_ms = entity_present_started.elapsed().as_secs_f64() * 1000.0;
        self.inline_models = Arc::new(inline_models);
        if !self.logged_dispatch_summary {
            println!(
                "DEMO ENTITY DISPATCH: general={} movers={} items={} missiles={} npcs={} fx={} other={} renderedSurfaces={}",
                dispatch_summary.general,
                dispatch_summary.movers,
                dispatch_summary.items,
                dispatch_summary.missiles,
                dispatch_summary.npcs,
                dispatch_summary.fx,
                dispatch_summary.other,
                dispatch_summary.rendered_surfaces,
            );
            self.logged_dispatch_summary = true;
        }
        let player_present_started = Instant::now();
        dynamic_models.extend(self.player_presenter.present_snapshot_players(
            &self.presented_entities,
            &self.client_game,
            &self.siege_classes,
            target_server_time,
            followed_entity_num,
            ghoul2_view,
        ));
        self.client_perf.player_present_ms = player_present_started.elapsed().as_secs_f64() * 1000.0;
        // OpenJK always runs CG_Player for the local/predicted player. In first
        // person it marks the refEntity RF_THIRD_PERSON so the body is not
        // submitted to the main view, but its animation/Ghoul2 state continues
        // to advance. Mirror that ownership here instead of recreating the local
        // player only when cg_thirdPerson is enabled.
        let render_followed_body =
            rendering_third_person(third_person, first_person_lightsaber, sample.policy);
        let followed_started = Instant::now();
        if let Some(entity) = followed_entity {
            let client_num = entity.state.field_i32("clientNum").unwrap_or(sample.client_num);
            if client_num >= 0 {
                if let Some(info) = self.client_game.client_info(client_num as usize, &self.siege_classes) {
                    let look_target_origin = if entity.state.field_i32("hasLookTarget").unwrap_or(0) != 0 {
                        let look_target = entity.state.field_i32("lookTarget").unwrap_or(-1);
                        self.presented_entities
                            .iter()
                            .find(|candidate| i32::from(candidate.number) == look_target)
                            .map(|candidate| candidate.origin)
                    } else {
                        None
                    };
                    dynamic_models.extend(self.player_presenter.present_player_entity(
                        &entity,
                        &info,
                        target_server_time,
                        local_player_alpha(third_person.alpha),
                        look_target_origin,
                        sample.teleported,
                        render_followed_body,
                    )?);
                    // Snapshot players already submit their thrown sabers in
                    // present_snapshot_players. The local/followed player can
                    // instead be synthesized from playerState (and therefore
                    // absent from the packet-entity list), so mirror OpenJK's
                    // CG_Player saberEntityNum path here only for that case.
                    if !self
                        .presented_entities
                        .iter()
                        .any(|candidate| candidate.number == entity.number)
                    {
                        dynamic_models.extend(self.player_presenter.present_thrown_saber_for_player(
                            &entity,
                            &info,
                            &self.presented_entities,
                            &self.client_game,
                            target_server_time,
                            local_player_alpha(third_person.alpha),
                        )?);
                    }
                }
            }
        }
        self.client_perf.followed_player_ms = followed_started.elapsed().as_secs_f64() * 1000.0;
        let fx_started = Instant::now();
        dynamic_models.extend(self.event_presenter.present(target_server_time));
        self.dynamic_models = Arc::new(dynamic_models);
        // Force-power visuals CG_Player queued for this frame.
        for request in self.player_presenter.drain_fx_requests() {
            self.weapon_fx.player_fx(&request);
        }
        // FX_AddScheduledEffects + FX_Add after every CGame submission.
        let fx_frame = self.weapon_fx.end_frame();
        self.fx_draws = fx_frame.draws;
        if let Some(sound) = &mut self.sound_presenter {
            for fx_sound in &fx_frame.sounds {
                sound.play_fx_sound(&fx_sound.qpath, fx_sound.origin);
            }
        }
        self.client_perf.fx_ms = fx_started.elapsed().as_secs_f64() * 1000.0;
        let ghoul2 = self.player_presenter.perf_stats();
        self.client_perf.ghoul2_pose_ms = ghoul2.pose_ms;
        self.client_perf.ghoul2_motion_pose_ms = ghoul2.motion_pose_ms;
        self.client_perf.ghoul2_skin_ms = ghoul2.skin_ms;
        self.client_perf.ghoul2_bolt_ms = ghoul2.bolt_ms;
        self.client_perf.ghoul2_pose_evals = ghoul2.pose_evals;
        self.client_perf.ghoul2_motion_pose_evals = ghoul2.motion_pose_evals;
        self.client_perf.ghoul2_bolt_queries = ghoul2.bolt_queries;
        self.client_perf.ghoul2_surfaces = ghoul2.surfaces_skinned;
        self.client_perf.ghoul2_vertices = ghoul2.vertices_skinned;
        self.client_perf.ghoul2_frustum_tests = ghoul2.frustum_tests;
        self.client_perf.ghoul2_frustum_culled = ghoul2.frustum_culled;
        self.client_perf.ghoul2_lod_counts = ghoul2.lod_counts;
        self.client_perf.dynamic_surfaces = u32::try_from(self.dynamic_models.len()).unwrap_or(u32::MAX);
        self.client_perf.dynamic_vertices = self
            .dynamic_models
            .iter()
            .map(|surface| surface.vertex_count() as u64)
            .sum();
        self.client_perf.dynamic_indices = self
            .dynamic_models
            .iter()
            .map(|surface| surface.index_count() as u64)
            .sum();

        let completed = self.eof
            && self.next_snapshot.is_none()
            && self
                .current_snapshot
                .as_ref()
                .is_some_and(|snapshot| target_server_time >= snapshot.server_time);
        Ok((
            if completed {
                DemoAdvance::Completed
            } else {
                DemoAdvance::Running
            },
            sample,
        ))
    }

    fn sample_at(&self, server_time: i32, teleported: bool) -> Option<DemoCameraSample> {
        let current = self.current_snapshot.as_ref()?;
        let next = self.next_snapshot.as_ref();
        // Match CG_SetNextSnap's demo interpolation guards: abrupt teleports,
        // follow-target changes, and map_restart server-count toggles must not
        // blend the recorded camera through space.
        let interpolation_discontinuity =
            next.is_some_and(|next| snapshot_discontinuity(current, next));
        let alpha = if interpolation_discontinuity {
            0.0
        } else {
            next.filter(|next| next.server_time > current.server_time)
                .map(|next| {
                    (server_time - current.server_time) as f32
                        / (next.server_time - current.server_time) as f32
                })
                .unwrap_or(0.0)
                .clamp(0.0, 1.0)
        };

        let current_origin = playerstate_vec3(&current.player_state, "origin")?;
        let current_angles = playerstate_vec3(&current.player_state, "viewangles")?;
        let current_viewheight = current
            .player_state
            .field_i32("viewheight")
            .unwrap_or(0) as f32;

        let (origin, angles, viewheight) = if let Some(next) = next {
            let next_origin = playerstate_vec3(&next.player_state, "origin")
                .unwrap_or(current_origin);
            let next_angles = playerstate_vec3(&next.player_state, "viewangles")
                .unwrap_or(current_angles);
            let next_viewheight = next
                .player_state
                .field_i32("viewheight")
                .unwrap_or(current_viewheight as i32) as f32;
            (
                lerp_vec3(current_origin, next_origin, alpha),
                [
                    lerp_angle_degrees(current_angles[0], next_angles[0], alpha),
                    lerp_angle_degrees(current_angles[1], next_angles[1], alpha),
                    lerp_angle_degrees(current_angles[2], next_angles[2], alpha),
                ],
                current_viewheight + (next_viewheight - current_viewheight) * alpha,
            )
        } else {
            (current_origin, current_angles, current_viewheight)
        };

        let mut eye = origin;
        eye[2] += viewheight;
        Some(DemoCameraSample {
            server_time,
            native_origin: origin,
            eye_position: scene::render_position(eye),
            player_position: scene::render_position(origin),
            view_angles: angles,
            view_height: viewheight.round() as i32,
            dead_yaw: current.player_state.stats.get(6).copied().unwrap_or(0) as f32,
            client_num: current.player_state.field_i32("clientNum").unwrap_or(0),
            policy: PlayerViewPolicyState::from_player_state(&current.player_state),
            teleported,
        })
    }
}

/// Camera sample for cg.predictedPlayerState (CG_CalcViewValues adds the
/// decaying prediction error to the view origin only).
fn sample_player_state(
    ps: &jka_protocol::server::PlayerState,
    server_time: i32,
    teleported: bool,
    error: [f32; 3],
) -> Option<DemoCameraSample> {
    let origin = playerstate_vec3(ps, "origin")?;
    let view_angles = playerstate_vec3(ps, "viewangles")?;
    let view_height = ps.field_i32("viewheight").unwrap_or(0);
    let view_origin = [origin[0] + error[0], origin[1] + error[1], origin[2] + error[2]];
    let mut eye = view_origin;
    eye[2] += view_height as f32;
    Some(DemoCameraSample {
        server_time,
        native_origin: view_origin,
        eye_position: scene::render_position(eye),
        player_position: scene::render_position(origin),
        view_angles,
        view_height,
        dead_yaw: ps.stats.get(6).copied().unwrap_or(0) as f32,
        client_num: ps.field_i32("clientNum").unwrap_or(0),
        policy: PlayerViewPolicyState::from_player_state(ps),
        teleported,
    })
}

fn playerstate_vec3(state: &jka_protocol::server::PlayerState, base: &str) -> Option<[f32; 3]> {
    Some([
        state.field_f32(&format!("{base}[0]"))?,
        state.field_f32(&format!("{base}[1]"))?,
        state.field_f32(&format!("{base}[2]"))?,
    ])
}

fn lerp_vec3(a: [f32; 3], b: [f32; 3], alpha: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * alpha,
        a[1] + (b[1] - a[1]) * alpha,
        a[2] + (b[2] - a[2]) * alpha,
    ]
}

fn lerp_angle_degrees(a: f32, b: f32, alpha: f32) -> f32 {
    let delta = (b - a + 180.0).rem_euclid(360.0) - 180.0;
    a + delta * alpha
}

pub struct App {
    base: PathBuf,
    /// Command-line/local game directory. Session fs_game is restored to this on disconnect.
    startup_game: Option<PathBuf>,
    /// Active asset game directory. Remote/demo fs_game may replace this for the session.
    game: Option<PathBuf>,
    initial_source: scene::MapSource,
    front_end: bool,
    frontend_page: FrontendPage,
    solo_maps: Vec<SoloMapEntry>,
    solo_map_selected: usize,
    solo_catalog_loaded: bool,
    solo_catalog_error: Option<String>,
    solo_levelshot_texture: Option<egui::TextureHandle>,
    solo_levelshot_texture_map: Option<String>,
    demo_entries: Vec<DemoEntry>,
    demo_selected: usize,
    demo_catalog_loaded: bool,
    demo_catalog_error: Option<String>,
    server_browser_tx: Sender<BrowserCommand>,
    server_browser_rx: Receiver<BrowserEvent>,
    server_browser: BrowserUiState,
    game_session: Option<GameSession>,
    /// True while live CGame/networking are active without a locally loaded BSP.
    live_without_world: bool,
    /// Live server connection (OpenJK clc/cls networking).
    net: Option<crate::net::NetClient>,
    /// Presenter/material prep kicked off at connect time and consumed when the
    /// gamestate arrives. Receiver absence means there is no worker in flight.
    live_cgame_prep_rx: Option<Receiver<Result<LiveCgamePrepared, String>>>,
    live_cgame_prepared: Option<LiveCgamePrepared>,
    pending_live_gamestate: bool,
    /// Reliable server commands that arrive while the overlapped CGame worker
    /// is still finishing. They are replayed through ClientGameState after the
    /// gamestate baseline is installed, preserving OpenJK command sequencing.
    pending_live_server_commands: VecDeque<jka_protocol::server::ServerCommand>,
    live_join_timing: Option<LiveJoinTiming>,
    network: crate::net::NetworkSettings,
    live_input: crate::net::LiveInput,
    /// Lowercased `+command` kbuttons currently held (IN_KeyDown state).
    live_buttons: HashSet<String>,
    predictor: crate::net::Predictor,
    /// cl_reconnectArgs.
    reconnect_target: Option<String>,
    /// Active native dm_26 recording, if /record is running.
    demo_recording: Option<DemoRecording>,
    /// Pmove context for the loaded map (humanoid animation.cfg timings).
    map_movement: Option<jka_movement::PmoveContext>,
    last_center_print: Vec<u8>,
    center_print: Option<CenterPrintRecord>,
    last_center_refresh: Instant,
    /// Most recently parsed OpenJK `scores` response. Kept cached while hidden.
    scoreboard: Option<UiScoreboard>,
    /// CG `showScores`: true while +scores is held (or explicitly invoked).
    scores_showing: bool,
    /// CG `scoresRequestTime`: score refreshes are throttled to roughly 2 seconds.
    last_scores_request: Option<Instant>,
    /// When the last usercmd was created (cl_commandRate pacing).
    last_command_at: Option<Instant>,
    /// This frame's unsent usercmd for display-only prediction.
    live_provisional: Option<jka_protocol::netchan::UserCmd>,
    /// StringEd table for @@@ server text, loaded on first use.
    stringed: Option<crate::cgame::stringed::StringEd>,
    /// `+command` lines from the command line, run after the first frame.
    startup_commands: Vec<String>,
    audio: config::AudioSettings,
    window_focused: bool,
    pending_frontend_map_launch: bool,
    /// Request id of the passive map currently being prepared for the main menu.
    /// It deliberately does not become a game session or local player.
    frontend_background_request_id: Option<u64>,
    frontend_cinematic: Option<FrontendCinematic>,
    proxy: EventLoopProxy<UserEvent>,
    loader_tx: Sender<MapLoadRequest>,
    window: Option<Arc<Window>>,
    render: Option<RenderThread>,
    startup_first_frame_seen: bool,
    camera: Camera,
    local_player: Option<LocalPlayer>,
    map_collision: Option<CollisionWorld>,
    steam_audio_acoustic_mesh: Option<Arc<jka_assets::bsp::AcousticMesh>>,
    steam_audio_bake: Option<Arc<crate::steam_audio::SteamAudioBakeData>>,
    steam_audio_bake_progress: Option<f32>,
    steam_audio_bake_error: Option<String>,
    /// Static BSP surface mesh cached for client-side Rapier visuals.
    map_physics_collision: PhysicsMapMesh,
    third_person: ThirdPersonSettings,
    first_person_lightsaber: bool,
    cg_debug_events: u8,
    crosshair: ui::CrosshairSettings,
    movement_keys_hud: ui::MovementKeysSettings,
    strafe_helper: ui::StrafeHelperSettings,
    hud_layout: HudLayout,
    hud_edit_selected: Option<HudElementId>,
    hud_edit_drag_origin: Option<[f32; 2]>,
    hud_edit_drag_delta: [f32; 2],
    presentation_smoothing: LocalPresentationSettings,
    third_person_camera: ThirdPersonCameraState,
    local_player_presenter: Option<PlayerPresenter>,
    solo_dynamic_models: Arc<Vec<DynamicModelSurface>>,
    solo_client_info: ClientInfo,
    player_model_input: String,
    player_model_editing: bool,
    menu_selected: usize,
    setup_selected: usize,
    keys: HashSet<KeyCode>,
    movement_keys: HashSet<KeyCode>,
    bindings: Bindings,
    controls_selected: usize,
    controls_waiting_for_key: bool,
    mouse_buttons_down: HashSet<u8>,
    modifiers: ModifiersState,
    captured: bool,
    mouse_input: MouseInputSettings,
    mouse_delta: (f64, f64),
    last_mouse_motion_at: Option<Instant>,
    noclip_primary_down: bool,
    noclip_alt_down: bool,
    mouse_input_sequence: u64,
    pending_mouse_input: Option<(u64, Instant)>,
    last_simulated_mouse_input: Option<InputLatencySample>,
    cursor_position: (f64, f64),
    previous_tick: Instant,
    overlay: OverlayMode,
    overlay_before_console: OverlayMode,
    console_input: String,
    /// UTF-8 byte offset of the editable console caret inside `console_input`.
    console_cursor: usize,
    console_status: String,
    console_lines: VecDeque<String>,
    console_timestamps: bool,
    log_rx: Receiver<crate::logging::LogRecord>,
    console_history: Vec<String>,
    console_history_index: Option<usize>,
    /// OpenJK Cbuf-style pending commands. `exec` and `vstr` insert at the
    /// front; interactive/startup commands append at the back.
    console_command_buffer: VecDeque<String>,
    /// OpenJK cmd_wait countdown. A value of 1 delays the remainder until the
    /// next main/client frame.
    console_wait_frames: u32,
    /// OpenJK-style CVAR_LATCH pending values. The live value remains unchanged
    /// until the owning subsystem is restarted.
    latched_console_cvars: BTreeMap<String, String>,
    console_scroll: usize,
    console_size: ConsoleSize,
    console_selection_anchor: Option<ConsolePoint>,
    console_selection_focus: Option<ConsolePoint>,
    console_selecting: bool,
    console_last_click: Option<(Instant, ConsolePoint)>,
    console_click_count: u8,
    console_search_open: bool,
    console_search_input: String,
    console_search_matches: Vec<ConsoleSearchMatch>,
    console_search_index: Option<usize>,
    chat_mode: ChatMode,
    chat_input: String,
    chat_lines: VecDeque<ChatRecord>,
    last_chat_refresh: Instant,
    last_hud_state: Option<HudState>,
    video_selected: usize,
    /// Which section of Setup -> Video the left rail is showing.
    video_section: VideoSection,
    environment_selected: usize,
    ocean_settings_open: bool,
    authored_oceans: Vec<crate::ocean::authoring::AuthoredOcean>,
    map_authored_oceans: Vec<crate::ocean::authoring::AuthoredOcean>,
    authored_ocean_selected: usize,
    authored_ocean_preview: bool,
    ocean_selected: usize,
    cloud_tuning_open: bool,
    cloud_tuning_selected: usize,
    video: VideoSettings,
    applied_fullscreen: FullscreenMode,
    applied_renderer_backend: RendererBackend,
    applied_resolution: [u32; 2],
    applied_grass: bool,
    applied_ocean: bool,
    applied_reflection_quality: ReflectionQuality,
    video_confirmation: Option<VideoConfirmation>,
    /// A requested video mode/backend that has spawned but has not yet produced
    /// a visible world frame. The 15-second keep/revert clock must not run while
    /// the replacement renderer is still initializing.
    pending_video_confirmation: Option<AppliedVideoMode>,
    /// Fullscreen transitions on Windows must be allowed to settle before a new
    /// WGPU surface is created. In particular DX12 logical Exclusive is actually
    /// a borderless flip-model window, while Vulkan Exclusive uses Winit's native
    /// monitor-mode switch. Creating the replacement surface in the same event-loop
    /// callback as that transition can leave the swapchain occluded or invalid.
    pending_renderer_restart: Option<PendingRendererRestart>,
    /// Set while a restart has hidden the native window. Launching straight into
    /// Vulkan exclusive works because the window is created hidden and only shown
    /// once the swapchain has presented; a restart has to do the same or Windows
    /// leaves the new surface composited behind the old one.
    window_hidden_for_restart: bool,
    supported_msaa: Vec<u32>,
    wireframe_supported: bool,
    spawns: Vec<SpawnPoint>,
    spawn_index: usize,
    map_name: String,
    triangles: usize,
    /// Authored worldspawn distanceCull, or JKA's default when absent.
    map_distance_cull: f32,
    /// Strongest q3map sun referenced by the current map's sky shaders.
    map_authored_sun: Option<scene::DirectionalSun>,
    /// Keeps Map shader <-> Custom useful for A/B: seed from the map only the
    /// first time Custom is selected, then preserve the user's custom values.
    sun_custom_initialized: bool,
    latest_request_id: u64,
    latest_prepare_options: scene::MapPrepareOptions,
    prepared_map_cache: Option<PreparedMapCache>,
    loading: Option<MapLoadingState>,
    static_ao_progress: Option<(u32, u32)>,
    preserve_game_state_on_next_map_upload: bool,
    renderer_restart_started: Option<Instant>,
    perf: PerfStats,
    threads: [ThreadPerfStats; crate::thread_activity::SLOT_COUNT],
    surface_inspector: Option<SurfaceInspectorInfo>,
    puddle_debug_visualization: bool,
    dof_focus_target: f32,
    config_path: PathBuf,
    config_dirty: bool,
    last_config_write: Instant,
    quit_requested: bool,
    fps_cap_input: String,
    fps_cap_editing: bool,
    fps_cap_replace_on_type: bool,
    physics_fps_input: String,
    physics_fps_editing: bool,
    physics_fps_replace_on_type: bool,
    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    egui_last_frame: Instant,
    egui_repaint_requested: bool,
    egui_renderer_active: bool,
    egui_apply_video_requested: bool,
}

impl App {
    pub fn new(
        base: PathBuf,
        game: Option<PathBuf>,
        initial_source: scene::MapSource,
        launch_map: bool,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Result<Self, String> {
        let loader_tx = spawn_map_loader(base.clone(), proxy.clone())?;
        let log_rx = crate::logging::subscribe();
        let settings_dir = jka_assets::pk3::active_game_directory(&base, game.as_deref());
        let config_path = settings_dir.join("jka-rust.cfg");
        let legacy_config = settings_dir.join("jampconfig.cfg");
        let mut video = config::load_video_settings(&config_path, Some(&legacy_config));
        let audio = config::load_audio_settings(&config_path, Some(&legacy_config));
        let presentation =
            config::load_client_presentation_settings(&config_path, Some(&legacy_config));
        // AO modes are mutually exclusive so startup/config A/B tests never pay for both.
        if video.static_bsp_ao {
            video.ssao = false;
            // BAKED is the hybrid HQ path; keep the user's bake tuning.
            video.static_bsp_ao_lightmap = true;
        }
        // AA is a single user-facing choice. Normalize legacy configs that may
        // have stacked MSAA with FXAA/TAA so disabled modes do not keep hidden cost.
        let aa_normalized = normalize_anti_aliasing(&mut video);
        let mut bindings = Bindings::load(&config_path, Some(&legacy_config));
        // OpenJK's scoreboard is normally on Tab. Preserve an existing Tab bind,
        // but make +scores the default when this config has no Tab assignment.
        let tab = BindKey::Keyboard(KeyCode::Tab);
        if bindings.get(tab).is_none() {
            bindings.set(tab, "+scores");
        }
        let player_model_input = presentation.model.clone();
        let fps_cap_input = video.fps_cap.to_string();
        let physics_fps_input = config::physics_fps_from_msec(video.physics_msec).to_string();
        let default_video = VideoSettings::default();
        let sun_custom_initialized = video.sun_override
            || (video.sun_yaw - default_video.sun_yaw).abs() > 0.0001
            || (video.sun_pitch - default_video.sun_pitch).abs() > 0.0001
            || (video.sun_intensity - default_video.sun_intensity).abs() > 0.0001
            || video.sun_color != default_video.sun_color;
        // If no native client config exists yet, persist the effective defaults
        // (or imported jampconfig values) shortly after startup. Also rewrite
        // legacy configs that stacked multiple AA methods.
        let config_dirty = !config_path.is_file() || aa_normalized;
        let fresh_install = !config_path.is_file();
        let initial_label = initial_source.label();
        let front_end = !launch_map;
        let (server_browser_tx, server_browser_rx) = server_browser::spawn()?;
        let server_browser = BrowserUiState::new(&settings_dir, presentation.master_servers.clone());
        let mut app = Self {
            base,
            startup_game: game.clone(),
            game,
            map_name: initial_label.clone(),
            initial_source,
            front_end,
            frontend_page: FrontendPage::Main,
            solo_maps: Vec::new(),
            solo_map_selected: 0,
            solo_catalog_loaded: false,
            solo_catalog_error: None,
            solo_levelshot_texture: None,
            solo_levelshot_texture_map: None,
            demo_entries: Vec::new(),
            demo_selected: 0,
            demo_catalog_loaded: false,
            demo_catalog_error: None,
            server_browser_tx,
            server_browser_rx,
            server_browser,
            game_session: None,
            live_without_world: false,
            net: None,
            live_cgame_prep_rx: None,
            live_cgame_prepared: None,
            pending_live_gamestate: false,
            pending_live_server_commands: VecDeque::new(),
            live_join_timing: None,
            network: presentation.network.clone(),
            live_input: crate::net::LiveInput::default(),
            live_buttons: HashSet::new(),
            predictor: crate::net::Predictor::default(),
            reconnect_target: None,
            demo_recording: None,
            map_movement: None,
            last_center_print: Vec::new(),
            center_print: None,
            last_center_refresh: Instant::now(),
            scoreboard: None,
            scores_showing: false,
            last_scores_request: None,
            last_command_at: None,
            live_provisional: None,
            stringed: None,
            startup_commands: Vec::new(),
            audio,
            window_focused: true,
            pending_frontend_map_launch: false,
            frontend_background_request_id: None,
            frontend_cinematic: None,
            proxy,
            loader_tx,
            window: None,
            render: None,
            startup_first_frame_seen: false,
            camera: Camera::new_with_fov([0.0, 0.0, 0.0], 0.0, presentation.fov),
            local_player: None,
            map_collision: None,
            steam_audio_acoustic_mesh: None,
            steam_audio_bake: None,
            steam_audio_bake_progress: None,
            steam_audio_bake_error: None,
            map_physics_collision: PhysicsMapMesh::default(),
            third_person: presentation.third_person,
            first_person_lightsaber: presentation.first_person_lightsaber,
            cg_debug_events: 0,
            crosshair: presentation.crosshair,
            movement_keys_hud: presentation.movement_keys,
            strafe_helper: presentation.strafe_helper,
            hud_layout: presentation.hud_layout,
            hud_edit_selected: None,
            hud_edit_drag_origin: None,
            hud_edit_drag_delta: [0.0, 0.0],
            presentation_smoothing: presentation.smoothing,
            third_person_camera: ThirdPersonCameraState::default(),
            local_player_presenter: None,
            solo_dynamic_models: Arc::new(Vec::new()),
            solo_client_info: ClientInfo::solo_model(&presentation.model),
            player_model_input,
            player_model_editing: false,
            menu_selected: 0,
            setup_selected: 1,
            keys: HashSet::new(),
            movement_keys: HashSet::new(),
            bindings,
            controls_selected: 0,
            controls_waiting_for_key: false,
            mouse_buttons_down: HashSet::new(),
            modifiers: ModifiersState::empty(),
            captured: false,
            mouse_input: presentation.mouse,
            mouse_delta: (0.0, 0.0),
            last_mouse_motion_at: None,
            noclip_primary_down: false,
            noclip_alt_down: false,
            mouse_input_sequence: 0,
            pending_mouse_input: None,
            last_simulated_mouse_input: None,
            cursor_position: (0.0, 0.0),
            previous_tick: Instant::now(),
            overlay: OverlayMode::Game,
            overlay_before_console: OverlayMode::Game,
            console_input: String::new(),
            console_cursor: 0,
            console_status: if front_end { "MAIN MENU".into() } else { "STARTING".into() },
            console_lines: VecDeque::with_capacity(1024),
            console_timestamps: presentation.console_timestamps,
            log_rx,
            console_history: Vec::with_capacity(128),
            console_history_index: None,
            console_command_buffer: VecDeque::new(),
            console_wait_frames: 0,
            latched_console_cvars: BTreeMap::new(),
            console_scroll: 0,
            console_size: ConsoleSize::Normal,
            console_selection_anchor: None,
            console_selection_focus: None,
            console_selecting: false,
            console_last_click: None,
            console_click_count: 0,
            console_search_open: false,
            console_search_input: String::new(),
            console_search_matches: Vec::new(),
            console_search_index: None,
            chat_mode: ChatMode::Global,
            chat_input: String::new(),
            chat_lines: VecDeque::with_capacity(16),
            last_chat_refresh: Instant::now(),
            last_hud_state: None,
            video_selected: 0,
            video_section: VideoSection::Display,
            environment_selected: 0,
            ocean_settings_open: false,
            authored_oceans: Vec::new(),
            map_authored_oceans: Vec::new(),
            authored_ocean_selected: 0,
            authored_ocean_preview: false,
            ocean_selected: 0,
            cloud_tuning_open: false,
            cloud_tuning_selected: 0,
            video,
            applied_fullscreen: video.fullscreen,
            applied_renderer_backend: video.renderer_backend,
            applied_resolution: video.resolution,
            applied_grass: video.grass,
            applied_ocean: video.ocean,
            applied_reflection_quality: video.reflection_quality,
            video_confirmation: None,
            pending_video_confirmation: None,
            pending_renderer_restart: None,
            window_hidden_for_restart: false,
            supported_msaa: vec![1],
            wireframe_supported: false,
            spawns: Vec::new(),
            spawn_index: 0,
            triangles: 0,
            map_distance_cull: crate::camera::DEFAULT_DISTANCE_CULL,
            map_authored_sun: None,
            sun_custom_initialized,
            latest_request_id: 0,
            latest_prepare_options: scene::MapPrepareOptions {
                grass: video.grass,
                voxel_probe_gi: video.voxel_probe_gi,
                ocean: video.ocean,
                client_physics: video.client_physics,
                gen_normal_maps: video.gen_normal_maps,
                float_lightmap: video.float_lightmap && video.hdr,
                planar_reflections: video.reflection_quality.planar_slot_budget() > 0,
                pbr_materials: video.pbr,
                allow_asset_overrides: video.allow_asset_overrides,
                steam_audio: audio.steam_audio,
            },
            prepared_map_cache: None,
            loading: launch_map.then(|| MapLoadingState::new(0, initial_label)),
            static_ao_progress: None,
            preserve_game_state_on_next_map_upload: false,
            renderer_restart_started: None,
            perf: PerfStats::default(),
            threads: [
                ThreadPerfStats {
                    name: "MAIN",
                    task: "IDLE",
                    active: false,
                    busy_percent: 0.0,
                },
                ThreadPerfStats {
                    name: "RENDER",
                    task: "IDLE",
                    active: false,
                    busy_percent: 0.0,
                },
                ThreadPerfStats {
                    name: "MAP LOADER",
                    task: "IDLE",
                    active: false,
                    busy_percent: 0.0,
                },
                ThreadPerfStats {
                    name: "WORKER 0",
                    task: "IDLE",
                    active: false,
                    busy_percent: 0.0,
                },
                ThreadPerfStats {
                    name: "WORKER 1",
                    task: "IDLE",
                    active: false,
                    busy_percent: 0.0,
                },
                ThreadPerfStats {
                    name: "WORKER 2",
                    task: "IDLE",
                    active: false,
                    busy_percent: 0.0,
                },
                ThreadPerfStats {
                    name: "WORKER 3",
                    task: "IDLE",
                    active: false,
                    busy_percent: 0.0,
                },
                ThreadPerfStats {
                    name: "WORKER 4",
                    task: "IDLE",
                    active: false,
                    busy_percent: 0.0,
                },
                ThreadPerfStats {
                    name: "WORKER 5",
                    task: "IDLE",
                    active: false,
                    busy_percent: 0.0,
                },
                ThreadPerfStats {
                    name: "WORKER 6",
                    task: "IDLE",
                    active: false,
                    busy_percent: 0.0,
                },
                ThreadPerfStats {
                    name: "WORKER 7",
                    task: "IDLE",
                    active: false,
                    busy_percent: 0.0,
                },
            ],
            surface_inspector: None,
            puddle_debug_visualization: false,
            dof_focus_target: 4096.0,
            config_path,
            config_dirty,
            last_config_write: Instant::now(),
            quit_requested: false,
            fps_cap_input,
            fps_cap_editing: false,
            fps_cap_replace_on_type: false,
            physics_fps_input,
            physics_fps_editing: false,
            physics_fps_replace_on_type: false,
            egui_ctx: egui::Context::default(),
            egui_state: None,
            egui_last_frame: Instant::now(),
            egui_repaint_requested: true,
            egui_renderer_active: false,
            egui_apply_video_requested: false,
        };
        // A first run has no opinion yet, so start somewhere that runs well on
        // modest hardware instead of at the engine's uncurated field defaults.
        if fresh_install {
            app.apply_quality_preset(quality::QualityPreset::Low);
        }
        Ok(app)
    }

    fn ui_snapshot(&self) -> UiSnapshot {
        // Keep the retained UI snapshot bounded even after a very long console session.
        // Full-screen 4K needs far fewer than 384 atlas-text rows, so this leaves
        // ample scroll context without cloning the entire 10k-line ring every keypress.
        let console_scroll = self.console_scroll.min(self.console_max_scroll());
        let console_end = self.console_lines.len().saturating_sub(console_scroll);
        let console_start = console_end.saturating_sub(384);
        let console_lines: Vec<String> = self
            .console_lines
            .iter()
            .skip(console_start)
            .take(console_end - console_start)
            .cloned()
            .collect();
        let console_selection = match (self.console_selection_anchor, self.console_selection_focus) {
            (Some(mut start), Some(mut end)) if console_start < console_end => {
                if (start.line, start.col) > (end.line, end.col) {
                    std::mem::swap(&mut start, &mut end);
                }
                if end.line < console_start || start.line >= console_end {
                    None
                } else {
                    if start.line < console_start {
                        start = ConsolePoint {
                            line: console_start,
                            col: 0,
                        };
                    }
                    if end.line >= console_end {
                        let line = console_end - 1;
                        let col = self
                            .console_lines
                            .get(line)
                            .map(|raw| crate::logging::strip_jka_colors(raw).chars().count())
                            .unwrap_or(0);
                        end = ConsolePoint { line, col };
                    }
                    Some(ConsoleSelection {
                        start_line: start.line - console_start,
                        start_col: start.col,
                        end_line: end.line - console_start,
                        end_col: end.col,
                    })
                }
            }
            _ => None,
        };

        let console_search_matches: Vec<ConsoleSearchMatch> = self
            .console_search_matches
            .iter()
            .filter(|hit| hit.line >= console_start && hit.line < console_end)
            .map(|hit| ConsoleSearchMatch {
                line: hit.line - console_start,
                start_col: hit.start_col,
                end_col: hit.end_col,
            })
            .collect();
        let console_search_active = self
            .console_search_index
            .and_then(|index| self.console_search_matches.get(index).copied())
            .and_then(|hit| {
                (hit.line >= console_start && hit.line < console_end).then_some(
                    ConsoleSearchMatch {
                        line: hit.line - console_start,
                        start_col: hit.start_col,
                        end_col: hit.end_col,
                    },
                )
            });

        let now = Instant::now();
        let chat_lines = self
            .chat_lines
            .iter()
            .filter_map(|line| {
                let age = now.saturating_duration_since(line.created);
                // Stock MP cg_chatBox defaults to 10000 ms. Preserve this
                // client's requested 1-second tail fade inside that lifetime.
                if age >= Duration::from_secs(10) {
                    return None;
                }
                let alpha = if age <= Duration::from_secs(9) {
                    1.0
                } else {
                    1.0 - (age.as_secs_f32() - 9.0)
                };
                Some(UiChatLine {
                    text: line.text.clone(),
                    alpha: alpha.clamp(0.0, 1.0),
                })
            })
            .collect();

        let center_print = self.center_print.as_ref().and_then(|print| {
            const CENTER_TIME: Duration = Duration::from_secs(3);
            const FADE_TIME: Duration = Duration::from_millis(200);
            let age = now.saturating_duration_since(print.created);
            if age >= CENTER_TIME {
                return None;
            }
            let remaining = CENTER_TIME.saturating_sub(age);
            let alpha = if remaining < FADE_TIME {
                remaining.as_secs_f32() / FADE_TIME.as_secs_f32()
            } else {
                1.0
            };
            Some(ui::UiCenterPrint { text: print.text.clone(), alpha })
        });

        let hud = self.current_hud_state();
        let follow_name = self.current_follow_name();

        UiSnapshot {
            mode: self.overlay,
            setup_selected: self.setup_selected,
            console_input: self.console_input.clone(),
            console_cursor: self.console_cursor,
            console_status: self.console_status.clone(),
            console_lines,
            console_scroll: 0,
            console_size: self.console_size,
            console_selection,
            console_search_open: self.console_search_open,
            console_search_query: self.console_search_input.clone(),
            console_search_total: self.console_search_matches.len(),
            console_search_index: self.console_search_index,
            console_search_matches,
            console_search_active,
            chat_mode: self.chat_mode,
            chat_input: self.chat_input.clone(),
            chat_lines,
            center_print,
            follow_name,
            scoreboard: self.scores_showing.then(|| self.scoreboard.clone()).flatten(),
            hud,
            hud_layout: self.hud_layout,
            crosshair: self.crosshair,
            movement_keys: self.movement_keys_hud,
            strafe_helper: self.strafe_helper,
            movement_hud: self.current_movement_hud_state(),
            video: self.video,
            perf: self.perf,
            threads: self.threads,
            surface_inspector: self.surface_inspector.clone(),
            loading: self.loading.as_ref().map(MapLoadingState::ui),
            static_ao_progress: self.static_ao_progress.map(|(completed, total)| MapLoadingBar {
                label: "BAKED AO",
                completed,
                total,
                skipped: false,
            }),
        }
    }

    fn render_snapshot(&self) -> RenderSnapshot {
        let demo_player_position = self
            .game_session
            .as_ref()
            .and_then(|playback| playback.player_position);
        let dynamic_models = self
            .game_session
            .as_ref()
            .map(|playback| {
                if playback.fx_surfaces.is_empty() {
                    Arc::clone(&playback.dynamic_models)
                } else {
                    let mut merged = Vec::with_capacity(playback.dynamic_models.len() + playback.fx_surfaces.len());
                    merged.extend(playback.dynamic_models.iter().cloned());
                    merged.extend(playback.fx_surfaces.iter().cloned());
                    Arc::new(merged)
                }
            })
            .unwrap_or_else(|| Arc::clone(&self.solo_dynamic_models));
        // Only a running demo's CGame owns mover state; before its first
        // snapshot (and in solo) inline models keep their compiled pose.
        let inline_models = self
            .game_session
            .as_ref()
            .filter(|playback| playback.current_snapshot.is_some())
            .map(|playback| Arc::clone(&playback.inline_models));
        RenderSnapshot {
            camera: self.camera,
            player_position: demo_player_position
                .or_else(|| self.local_player.as_ref().map(LocalPlayer::world_position)),
            area_mask: self
                .game_session
                .as_ref()
                .and_then(|playback| playback.current_snapshot.as_ref())
                .map(|snapshot| snapshot.area_mask),
            dynamic_models,
            inline_models,
            dof_focus_target: self.dof_focus_target,
            input_latency: self.last_simulated_mouse_input,
            client_perf: self
                .game_session
                .as_ref()
                .map_or_else(ClientFramePerf::default, |playback| playback.client_perf),
        }
    }

    fn publish_snapshot(&self) {
        if let Some(render) = &self.render {
            render.publish(self.render_snapshot());
        }
    }

    fn publish_ui(&self) {
        self.render_command(RenderCommand::SetUi(self.ui_snapshot()));
    }

    fn render_command(&self, command: RenderCommand) {
        if let Some(render) = &self.render {
            render.command(command);
        }
    }

    fn set_capture(&mut self, captured: bool) {
        if !captured {
            self.noclip_primary_down = false;
            self.noclip_alt_down = false;
        }
        if self.captured == captured {
            return;
        }
        let Some(window) = &self.window else {
            return;
        };
        if captured && self.overlay == OverlayMode::None {
            let grabbed = window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
            if grabbed.is_ok() {
                window.set_cursor_visible(false);
                self.captured = true;
            }
        } else {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
            window.set_cursor_visible(true);
            self.captured = false;
        }
    }

    fn set_overlay(&mut self, overlay: OverlayMode) {
        let leaving_console =
            self.overlay == OverlayMode::Console && overlay != OverlayMode::Console;
        self.overlay = overlay;
        self.egui_repaint_requested = true;
        if leaving_console {
            self.console_input.clear();
            self.console_cursor = 0;
            self.console_history_index = None;
        }
        if overlay != OverlayMode::Console {
            self.console_search_open = false;
        }
        self.mouse_delta = (0.0, 0.0);
        self.last_mouse_motion_at = None;
        self.pending_mouse_input = None;
        if let Some(player) = &mut self.local_player {
            player.pause();
        }
        if overlay != OverlayMode::None {
            self.keys.clear();
            self.movement_keys.clear();
            self.mouse_buttons_down.clear();
            self.noclip_primary_down = false;
            self.noclip_alt_down = false;
            self.set_capture(false);
        } else {
            self.set_capture(true);
        }
        self.publish_ui();
    }

    fn controls_page_active(&self) -> bool {
        self.overlay == OverlayMode::Game
            && if self.front_end {
                self.frontend_page == FrontendPage::Controls
            } else {
                self.menu_selected == 3
            }
    }

    fn overlay_after_console(&self) -> OverlayMode {
        if self.front_end && self.overlay_before_console == OverlayMode::None {
            OverlayMode::Game
        } else {
            self.overlay_before_console
        }
    }

    fn frontend_back(&mut self) {
        if !self.front_end {
            return;
        }
        if self.overlay == OverlayMode::Video {
            self.frontend_page = FrontendPage::Main;
            self.set_overlay(OverlayMode::Game);
            return;
        }
        self.frontend_page = match self.frontend_page {
            FrontendPage::Main => FrontendPage::Main,
            FrontendPage::Play | FrontendPage::Controls => FrontendPage::Main,
            FrontendPage::ServerBrowser | FrontendPage::SoloGame | FrontendPage::PlayDemo => FrontendPage::Play,
        };
        self.egui_repaint_requested = true;
        self.publish_ui();
    }

    fn mark_config_dirty(&mut self) {
        self.config_dirty = true;
        self.last_config_write = Instant::now();
    }

    fn refresh_bound_state(&mut self) {
        let mut movement_keys = HashSet::with_capacity(8);
        let mut primary = false;
        let mut alt = false;
        let mut live_buttons = HashSet::new();

        let mut apply = |binding: &str| {
            for command in keybinds::split_binding_commands(binding) {
                let verb = command.split_whitespace().next().unwrap_or("");
                if verb.starts_with('+') {
                    live_buttons.insert(verb.to_ascii_lowercase());
                }
                match verb.to_ascii_lowercase().as_str() {
                    "+forward" => { movement_keys.insert(KeyCode::KeyW); }
                    "+back" => { movement_keys.insert(KeyCode::KeyS); }
                    "+moveleft" => { movement_keys.insert(KeyCode::KeyA); }
                    "+moveright" => { movement_keys.insert(KeyCode::KeyD); }
                    "+moveup" => { movement_keys.insert(KeyCode::Space); }
                    "+movedown" => { movement_keys.insert(KeyCode::ControlLeft); }
                    "+speed" => { movement_keys.insert(KeyCode::ShiftLeft); }
                    "+attack" => primary = true,
                    "+altattack" => alt = true,
                    _ => {}
                }
            }
        };

        for code in &self.keys {
            if let Some(binding) = self.bindings.get(keybinds::bind_key_for_code(*code)) {
                apply(binding);
            }
        }
        for button in &self.mouse_buttons_down {
            if let Some(binding) = self.bindings.get(BindKey::Mouse(*button)) { apply(binding); }
        }

        let was_scores_showing = self.scores_showing;
        self.movement_keys = movement_keys;
        self.live_buttons = live_buttons;
        self.noclip_primary_down = primary;
        self.noclip_alt_down = alt;
        self.scores_showing = self.live_buttons.contains("+scores");
        if self.scores_showing && !was_scores_showing {
            // CG_ScoresDown_f clears stale contents when issuing a new request.
            self.request_scores_if_due(true);
        }
        if self.scores_showing != was_scores_showing {
            self.publish_ui();
        }
    }

    fn execute_binding_press(&mut self, key: BindKey) {
        let Some(binding) = self.bindings.get(key).map(str::to_owned) else { return; };
        let mut buffered = Vec::new();
        for command in keybinds::split_binding_commands(&binding) {
            if command.starts_with('+') {
                // +scores is a local CG kbutton, not a usercmd button bit. The
                // held state is derived by refresh_bound_state above.
                if command
                    .split_whitespace()
                    .next()
                    .is_some_and(|verb| verb.eq_ignore_ascii_case("+scores"))
                {
                    continue;
                }
                // IN_KeyDown wasPressed: a tap shorter than a frame still fires.
                if let Some(verb) = command.split_whitespace().next() {
                    self.live_input.note_pressed(verb);
                }
                continue;
            }
            buffered.push(command.to_owned());
        }
        // Non-button commands use the same Cbuf path as console scripts, so a
        // binding like `echo one; wait; echo two` observes frame-aware wait.
        self.append_console_commands(buffered);
    }

    fn bind_control_key(&mut self, key: BindKey) {
        let Some(action) = keybinds::CONTROL_ACTIONS.get(self.controls_selected) else { return; };
        let command = action.command;
        self.bindings.unbind_command(command);
        self.bindings.set(key, command);
        self.controls_waiting_for_key = false;
        self.refresh_bound_state();
        self.mark_config_dirty();
        self.console_status = format!("BOUND {} TO {}", keybinds::key_name(key), command);
        self.publish_ui();
    }

    fn unbind_selected_control(&mut self) {
        if let Some(action) = keybinds::CONTROL_ACTIONS.get(self.controls_selected) {
            let command = action.command;
            if self.bindings.unbind_command(command) > 0 {
                self.refresh_bound_state();
                self.mark_config_dirty();
            }
        }
        self.controls_waiting_for_key = false;
        self.publish_ui();
    }

    fn handle_bind_console_command(&mut self, command: &str) -> bool {
        let words = keybinds::split_command_words(command.trim_start_matches('/'));
        let Some(verb) = words.first() else { return false; };
        if verb.eq_ignore_ascii_case("bind") {
            match words.as_slice() {
                [_] => self.push_console_line("^3bind <key> [command]^7 - attach a command to a key"),
                [_, key_name] => match keybinds::parse_key(key_name) {
                    Some(key) => match self.bindings.get(key).map(str::to_owned) {
                        Some(binding) => self.push_console_line(format!("^7\"{}\" = \"{}\"", keybinds::key_name(key), binding)),
                        None => self.push_console_line(format!("^7\"{}\" is not bound", keybinds::key_name(key))),
                    },
                    None => self.push_console_line(format!("^1\"{key_name}\" isn't a valid key")),
                },
                [_, key_name, rest @ ..] => match keybinds::parse_key(key_name) {
                    Some(key) => {
                        self.bindings.set(key, rest.join(" "));
                        self.refresh_bound_state();
                        self.mark_config_dirty();
                    }
                    None => self.push_console_line(format!("^1\"{key_name}\" isn't a valid key")),
                },
                [] => unreachable!(),
            }
            self.publish_ui();
            return true;
        }
        if verb.eq_ignore_ascii_case("unbind") {
            if words.len() != 2 {
                self.push_console_line("^3unbind <key>^7 - remove a key binding");
            } else if let Some(key) = keybinds::parse_key(&words[1]) {
                self.bindings.unbind(key);
                self.refresh_bound_state();
                self.mark_config_dirty();
            } else {
                self.push_console_line(format!("^1\"{}\" isn't a valid key", words[1]));
            }
            self.publish_ui();
            return true;
        }
        if verb.eq_ignore_ascii_case("unbindall") {
            self.bindings.clear();
            self.refresh_bound_state();
            self.mark_config_dirty();
            self.publish_ui();
            return true;
        }
        if verb.eq_ignore_ascii_case("bindlist") {
            for (key, binding) in self.bindings.sorted() {
                self.push_console_line(format!("^7{} \"{}\"", keybinds::key_name(key), binding));
            }
            return true;
        }
        false
    }

    fn flush_config(&mut self) {
        if !self.config_dirty {
            return;
        }

        // Restart-sensitive settings are latched. Never persist a merely selected
        // resolution/fullscreen/backend, and never persist an unconfirmed mode.
        // If the process dies while the keep/revert dialog is visible, the next
        // launch therefore comes back in the last known-good video mode.
        let stable = self
            .video_confirmation
            .map(|confirmation| confirmation.previous)
            .or(self.pending_video_confirmation)
            .or_else(|| {
                self.pending_renderer_restart.map(|pending| {
                    if pending.confirm_on_change {
                        pending.previous
                    } else {
                        // A non-confirming restart is a rollback/recovery. Its
                        // target is already the last known-good mode.
                        pending.target
                    }
                })
            })
            .unwrap_or_else(|| self.applied_video_mode());
        let mut settings = self.video;
        settings.fullscreen = stable.fullscreen;
        settings.renderer_backend = stable.renderer_backend;
        settings.resolution = stable.resolution;
        // Grass/ocean can be disabled live. Preserve a pending enable as latched
        // until the renderer has actually been restarted with the required map
        // resources, but persist live disables immediately.
        settings.grass = if self.video.grass && !self.applied_grass {
            self.applied_grass
        } else {
            self.video.grass
        };
        settings.ocean = if self.video.ocean && !self.applied_ocean {
            self.applied_ocean
        } else {
            self.video.ocean
        };
        // OpenJK archives the pending CVAR_LATCH value. Overlay only console
        // latches here; unconfirmed menu display changes above remain last-good.
        self.apply_latched_values_to_settings(&mut settings);

        let presentation = config::ClientPresentationSettings {
            third_person: self.third_person,
            first_person_lightsaber: self.first_person_lightsaber,
            smoothing: self.presentation_smoothing,
            mouse: self.mouse_input,
            fov: self.camera.cg_fov(),
            model: self.solo_client_info.model_cvar(),
            crosshair: self.crosshair,
            hud_layout: self.hud_layout,
            movement_keys: self.movement_keys_hud,
            strafe_helper: self.strafe_helper,
            console_timestamps: self.console_timestamps,
            network: self.network.clone(),
            master_servers: self.server_browser.master_servers.clone(),
        };
        match config::save_video_settings(
            &self.config_path,
            settings,
            &self.bindings,
            &presentation,
            &self.audio,
        ) {
            Ok(()) => self.config_dirty = false,
            Err(error) => eprintln!("Config save failed: {error}"),
        }
    }

    /// Refresh rate of the monitor the window is on, in Hz.
    fn monitor_refresh_hz(&self) -> Option<f64> {
        let hz = self
            .window
            .as_ref()?
            .current_monitor()?
            .refresh_rate_millihertz()? as f64
            / 1000.0;
        (hz >= 1.0).then_some(hz)
    }

    /// Highest FPS the present path can actually reach, when something other than
    /// the GPU is the limit.
    ///
    /// Only DX12 with FAST vsync has one: wgpu presents Mailbox without the DXGI
    /// tearing flag, so presents retire at vblank and at most
    /// [`crate::renderer::MAX_FRAME_LATENCY`] frames can retire per refresh. VSync OFF
    /// (Immediate) sets the tearing flag and is uncapped, ON/Adaptive are
    /// refresh-bound by definition on every backend, and Vulkan's Mailbox lets
    /// the app render ahead freely.
    fn present_fps_ceiling(&self) -> Option<u32> {
        if self.applied_renderer_backend != RendererBackend::Dx12
            || self.video.vsync != VsyncMode::Fast
        {
            return None;
        }
        let refresh = self.monitor_refresh_hz()?;
        Some((refresh * f64::from(crate::renderer::MAX_FRAME_LATENCY)) as u32)
    }

    /// Highest cap that means anything here: the DX12 FAST present ceiling when
    /// there is one, otherwise the cvar's own maximum.
    fn fps_cap_limit(&self) -> u32 {
        self.present_fps_ceiling()
            .unwrap_or(config::FPS_CAP_MAX)
    }

    /// Resolve a requested cap to the number that will actually be honoured.
    ///
    /// "Unlimited" is not shown as 0 anywhere, because 0 tells the player nothing
    /// about the frame rate they will get. It resolves to whatever the real limit
    /// is, so the displayed cap is always the true one on every backend and vsync
    /// mode rather than only on the DX12 path that has a hard ceiling.
    fn resolved_fps_cap(&self, requested: u32) -> u32 {
        let limit = self.fps_cap_limit();
        if requested == 0 || requested > limit {
            limit
        } else {
            requested
        }
    }

    fn effective_fps_cap(&self) -> u32 {
        self.resolved_fps_cap(self.video.fps_cap)
    }

    fn frontend_refresh_fps_cap(&self) -> Option<u32> {
        self.monitor_refresh_hz().map(|hz| hz.round().clamp(1.0, u32::MAX as f64) as u32)
    }

    fn active_render_fps_cap(&self) -> u32 {
        if self.front_end {
            self.frontend_refresh_fps_cap()
                .unwrap_or_else(|| self.effective_fps_cap())
        } else {
            self.effective_fps_cap()
        }
    }

    fn sync_render_fps_cap(&self) {
        self.render_command(RenderCommand::SetFpsCap(self.active_render_fps_cap()));
    }

    fn set_fps_cap(&mut self, fps_cap: u32) {
        let requested = fps_cap;
        let fps_cap = self.resolved_fps_cap(requested);
        // Only the DX12 present ceiling is worth explaining. Rounding 0 or an
        // over-range number down to the cvar maximum is not a surprise.
        if let Some(ceiling) = self.present_fps_ceiling() {
            if requested > ceiling {
                self.console_status = format!(
                    "DX12 + FAST VSYNC TOPS OUT AT {ceiling} FPS ({} FRAMES x {:.0} HZ); \
                     CAP SNAPPED BACK. USE VSYNC OFF FOR UNCAPPED.",
                    crate::renderer::MAX_FRAME_LATENCY,
                    self.monitor_refresh_hz().unwrap_or_default()
                );
                self.push_console_line(format!("^3{}", self.console_status));
            }
        }
        self.video.fps_cap = fps_cap;
        if !self.fps_cap_editing {
            self.fps_cap_input = fps_cap.to_string();
        }
        self.sync_render_fps_cap();
        self.mark_config_dirty();
        self.publish_ui();
    }

    fn begin_fps_cap_edit(&mut self) {
        if !self.fps_cap_editing {
            self.fps_cap_editing = true;
            self.fps_cap_input = self.video.fps_cap.to_string();
            self.fps_cap_replace_on_type = true;
        }
        self.publish_ui();
    }

    fn set_physics_msec(&mut self, physics_msec: u32) {
        self.video.physics_msec = physics_msec;
        if !self.physics_fps_editing {
            self.physics_fps_input = config::physics_fps_from_msec(physics_msec).to_string();
        }
        if let Some(player) = &mut self.local_player {
            if let Err(error) = player.set_physics_tick_msec(physics_msec) {
                self.console_status = format!("PHYSICS FPS ERROR: {error}");
            }
        }
        self.mark_config_dirty();
        self.publish_ui();
    }

    fn begin_physics_fps_edit(&mut self) {
        if !self.physics_fps_editing {
            self.physics_fps_editing = true;
            self.physics_fps_input =
                config::physics_fps_from_msec(self.video.physics_msec).to_string();
            self.physics_fps_replace_on_type = true;
        }
        self.publish_ui();
    }

    fn set_gamma(&mut self, gamma: f32) {
        self.video.gamma = gamma.clamp(0.5, 3.0);
        self.render_command(RenderCommand::SetGamma(self.video.gamma));
        self.mark_config_dirty();
        self.publish_ui();
    }

    fn set_chromatic_aberration_strength(&mut self, strength: f32) {
        self.video.chromatic_aberration = strength.max(0.0);
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    fn set_film_grain_strength(&mut self, strength: f32) {
        self.video.film_grain_strength = strength.clamp(0.0, 1.0);
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    fn set_motion_blur_strength(&mut self, strength: f32) {
        self.video.motion_blur_strength = strength.clamp(0.0, 1.0);
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    fn set_depth_of_field_strength(&mut self, strength: f32) {
        self.video.depth_of_field_strength = strength.clamp(0.0, 1.0);
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    fn cycle_dof_quality(&mut self, direction: i32) {
        let current = DofQuality::ALL
            .iter()
            .position(|quality| *quality == self.video.dof_quality)
            .unwrap_or(1) as i32;
        let next = (current + direction).rem_euclid(DofQuality::ALL.len() as i32) as usize;
        self.video.dof_quality = DofQuality::ALL[next];
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    fn set_color_lut_strength(&mut self, strength: f32) {
        self.video.color_lut_strength = strength.clamp(0.0, 1.0);
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    fn set_color_lut(&mut self, preset: ColorLutPreset) {
        self.video.color_lut = preset;
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    fn set_fog_strength(&mut self, strength: f32) {
        self.video.fog_strength = strength.clamp(0.0, ui::MAX_FOG_STRENGTH);
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    fn map_sun_editor_values(&self) -> Option<(f32, f32, f32, [f32; 3])> {
        self.map_authored_sun.map(|sun| {
            let [yaw, pitch] = sun.q3_angles();
            let max_channel = sun.color.into_iter().fold(0.0_f32, f32::max);
            let color = if max_channel > 1e-6 {
                sun.color.map(|channel| (channel / max_channel).clamp(0.0, 1.0))
            } else {
                [1.0, 1.0, 1.0]
            };
            (yaw, pitch, sun.intensity, color)
        })
    }

    fn set_sun_override(&mut self, enabled: bool, seed_from_map: bool) {
        if enabled && !self.video.sun_override && seed_from_map && !self.sun_custom_initialized {
            if let Some((yaw, pitch, intensity, color)) = self.map_sun_editor_values() {
                self.video.sun_yaw = yaw;
                self.video.sun_pitch = pitch;
                self.video.sun_intensity = intensity;
                self.video.sun_color = color;
            }
        }
        if enabled {
            self.sun_custom_initialized = true;
        }
        self.video.sun_override = enabled;
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    fn set_sun_yaw(&mut self, yaw: f32) {
        self.video.sun_yaw = yaw.rem_euclid(360.0);
        self.sun_custom_initialized = true;
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    fn set_sun_pitch(&mut self, pitch: f32) {
        self.video.sun_pitch = pitch.clamp(-90.0, 90.0);
        self.sun_custom_initialized = true;
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    fn set_sun_intensity(&mut self, intensity: f32) {
        self.video.sun_intensity = intensity.max(0.0);
        self.sun_custom_initialized = true;
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    fn set_sun_color(&mut self, color: [f32; 3]) {
        self.video.sun_color = color.map(|channel| channel.clamp(0.0, 1.0));
        self.sun_custom_initialized = true;
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    fn effective_distance_cull(&self) -> f32 {
        let scale = self.video.distance_cull_scale;
        if scale <= 0.001 {
            self.map_distance_cull
        } else {
            self.map_distance_cull * scale
        }
    }

    fn apply_distance_cull(&mut self) {
        self.camera.set_distance_cull(self.effective_distance_cull());
    }

    fn set_distance_cull_scale(&mut self, scale: f32) {
        self.video.distance_cull_scale = scale.clamp(0.0, ui::MAX_DISTANCE_CULL_SCALE);
        self.apply_distance_cull();
        self.mark_config_dirty();
        self.publish_ui();
    }

    fn set_cloud_quality(&mut self, value: f32) {
        self.video.cloud_quality = value.clamp(0.0, 1.0);
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    fn set_cloud_coverage(&mut self, value: f32) {
        self.video.cloud_coverage = value.clamp(0.0, 1.0);
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    /// Commit one CLOUD TUNING row. Every row on that page is either a toggle
    /// or a 0..1 slider, so one entry point covers keyboard, mouse and reset.
    fn apply_cloud_tuning(&mut self, row: usize, toggle: bool, slider: Option<f32>) {
        let set = |value: Option<f32>, current: f32| value.unwrap_or(current).clamp(0.0, 1.0);
        match row {
            ui::CLOUD_ROW_TEMPORAL_DEPTH_FIX => {
                if toggle {
                    self.video.cloud_temporal_depth_fix = !self.video.cloud_temporal_depth_fix;
                }
            }
            ui::CLOUD_ROW_SHEAR => {
                self.video.cloud_shear = set(slider, self.video.cloud_shear);
            }
            ui::CLOUD_ROW_BASE_VARIATION => {
                self.video.cloud_base_variation = set(slider, self.video.cloud_base_variation);
            }
            ui::CLOUD_ROW_WIND_VARIATION => {
                if toggle {
                    self.video.cloud_wind_variation = !self.video.cloud_wind_variation;
                }
            }
            ui::CLOUD_ROW_SHAPE_EVOLUTION => {
                if toggle {
                    self.video.cloud_shape_evolution = !self.video.cloud_shape_evolution;
                }
            }
            ui::CLOUD_ROW_TERRAIN_INTERACTION => {
                if toggle {
                    self.video.cloud_terrain_interaction = !self.video.cloud_terrain_interaction;
                }
            }
            ui::CLOUD_ROW_EMPTY_SKIP => {
                if toggle {
                    self.video.cloud_empty_skip = !self.video.cloud_empty_skip;
                }
            }
            ui::CLOUD_ROW_AERIAL => {
                self.video.cloud_aerial = set(slider, self.video.cloud_aerial);
            }
            ui::CLOUD_ROW_SKY_AMBIENT => {
                if toggle {
                    self.video.cloud_sky_ambient = !self.video.cloud_sky_ambient;
                }
            }
            ui::CLOUD_ROW_HISTORY_BLEND => {
                self.video.cloud_history_blend = set(slider, self.video.cloud_history_blend);
            }
            ui::CLOUD_ROW_MOTION_REJECT => {
                self.video.cloud_motion_reject = set(slider, self.video.cloud_motion_reject);
            }
            ui::CLOUD_ROW_THICKNESS_VARIATION => {
                self.video.cloud_thickness_variation =
                    set(slider, self.video.cloud_thickness_variation);
            }
            ui::CLOUD_ROW_SIZE => {
                self.video.cloud_size = set(slider, self.video.cloud_size);
            }
            _ => {
                if toggle {
                    self.video.cloud_history_depth_reject =
                        !self.video.cloud_history_depth_reject;
                }
            }
        }
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    fn set_cloud_height(&mut self, value: f32) {
        self.video.cloud_height = value.clamp(ui::CLOUD_HEIGHT_MIN, ui::CLOUD_HEIGHT_MAX);
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    fn set_cloud_thickness(&mut self, value: f32) {
        self.video.cloud_thickness = value.clamp(ui::CLOUD_THICKNESS_MIN, ui::CLOUD_THICKNESS_MAX);
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    fn set_cloud_wind_speed(&mut self, value: f32) {
        self.video.cloud_wind_speed = value.clamp(0.0, ui::CLOUD_WIND_SPEED_MAX);
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    fn set_cloud_wind_direction(&mut self, value: f32) {
        self.video.cloud_wind_direction = value.rem_euclid(360.0);
        self.sync_post_effects();
        self.mark_config_dirty();
        self.publish_ui();
    }

    fn sync_post_effects(&self) {
        self.render_command(RenderCommand::SetPostEffects(PostEffects {
            hdr: self.video.hdr,
            auto_exposure: self.video.auto_exposure,
            tone_mapping: self.video.tone_mapping,
            bloom: self.video.bloom,
            halation: self.video.halation,
            ssao: self.video.ssao,
            static_bsp_ao: self.video.static_bsp_ao,
            static_bsp_ao_lightmap: self.video.static_bsp_ao_lightmap,
            static_bsp_ao_samples: self.video.static_bsp_ao_samples,
            static_bsp_ao_resolution: self.video.static_bsp_ao_resolution,
            static_bsp_ao_strength: self.video.static_bsp_ao_strength,
            static_bsp_ao_range: self.video.static_bsp_ao_range,
            static_bsp_ao_current_cell: self.video.static_bsp_ao_current_cell,
            fxaa: self.video.fxaa,
            smaa: self.video.smaa,
            taa: self.video.taa,
            contact_shadows: self.video.contact_shadows,
            fog_mode: self.video.fog_mode,
            fog_strength: self.video.fog_strength,
            sun_override: self.video.sun_override,
            sun_yaw: self.video.sun_yaw,
            sun_pitch: self.video.sun_pitch,
            sun_intensity: self.video.sun_intensity,
            sun_color: self.video.sun_color,
            sun_visibility: self.video.sun_visibility,
            clouds: self.video.clouds,
            cloud_type: self.video.cloud_type,
            cloud_quality: self.video.cloud_quality,
            cloud_coverage: self.video.cloud_coverage,
            cloud_height: self.video.cloud_height,
            cloud_thickness: self.video.cloud_thickness,
            cloud_wind_speed: self.video.cloud_wind_speed,
            cloud_wind_direction: self.video.cloud_wind_direction,
            cloud_shadows: self.video.cloud_shadows,
            cloud_render_resolution: self.video.cloud_render_resolution,
            cloud_temporal: self.video.cloud_temporal,
            cloud_temporal_depth_fix: self.video.cloud_temporal_depth_fix,
            cloud_shear: self.video.cloud_shear,
            cloud_base_variation: self.video.cloud_base_variation,
            cloud_wind_variation: self.video.cloud_wind_variation,
            cloud_shape_evolution: self.video.cloud_shape_evolution,
            cloud_terrain_interaction: self.video.cloud_terrain_interaction,
            cloud_empty_skip: self.video.cloud_empty_skip,
            cloud_aerial: self.video.cloud_aerial,
            cloud_sky_ambient: self.video.cloud_sky_ambient,
            cloud_history_blend: self.video.cloud_history_blend,
            cloud_motion_reject: self.video.cloud_motion_reject,
            cloud_history_depth_reject: self.video.cloud_history_depth_reject,
            cloud_thickness_variation: self.video.cloud_thickness_variation,
            cloud_size: self.video.cloud_size,
            rain: self.video.rain,
            rain_intensity: self.video.rain_intensity,
            // Reflection quality is restart-latched because planar eligibility
            // changes BSP batching/topology. Keep the live renderer on the last
            // applied value until vid_restart rebuilds the map/renderer.
            reflection_quality: self.applied_reflection_quality,
            reflection_debug: self.video.reflection_debug,
            chromatic_aberration: self.video.chromatic_aberration,
            vignette: self.video.vignette,
            film_grain_strength: self.video.film_grain_strength,
            motion_blur_strength: self.video.motion_blur_strength,
            depth_of_field_strength: self.video.depth_of_field_strength,
            dof_quality: self.video.dof_quality,
            color_lut: self.video.color_lut,
            color_lut_strength: self.video.color_lut_strength,
        }));
    }

    fn sync_gpu_visibility(&self) {
        self.render_command(RenderCommand::SetGpuVisibility {
            gpu_driven: self.video.gpu_driven,
            hiz_occlusion: self.video.hiz_occlusion,
        });
    }

    fn sync_clustered_lighting(&self) {
        self.render_command(RenderCommand::SetClusteredLighting(
            self.video.clustered_lighting,
        ));
    }

    fn sync_entity_ambient_lighting(&self) {
        self.render_command(RenderCommand::SetIrradianceVolume(matches!(
            self.video.entity_ambient_lighting,
            EntityAmbientLightingMode::BevyIrradianceVolume
        )));
    }

    fn sync_emissive_area_lights(&self) {
        self.render_command(RenderCommand::SetEmissiveAreaLights(
            self.video.emissive_area_lights,
        ));
    }

    fn sync_voxel_probe_gi(&self) {
        self.render_command(RenderCommand::SetVoxelProbeGi(self.video.voxel_probe_gi));
    }

    fn sync_local_light_shadows(&self) {
        self.render_command(RenderCommand::SetLocalLightShadows(
            self.video.local_light_shadows,
        ));
    }

    fn sync_pbr(&self) {
        self.render_command(RenderCommand::SetPbrSettings {
            enabled: self.video.pbr,
            deluxe_mapping: self.video.deluxe_mapping,
            deluxe_specular: self.video.deluxe_specular,
        });
    }

    fn sync_cascaded_shadows(&self) {
        self.render_command(RenderCommand::SetCascadedShadows(
            self.video.dynamic_shadows,
        ));
    }

    fn sync_cull_debug(&self) {
        self.render_command(RenderCommand::SetCullDebug(self.video.cull_debug));
    }

    fn sync_planar_reflection_debug(&self) {
        self.render_command(RenderCommand::SetPlanarReflectionDebug(
            self.video.planar_reflection_debug,
        ));
    }

    fn append_console_line(&mut self, line: String) {
        const MAX_CONSOLE_LINES: usize = 10_000;
        let preserve_scroll = self.console_scroll > 0;
        if self.console_lines.len() >= MAX_CONSOLE_LINES {
            self.console_lines.pop_front();

            let shift_point = |point: Option<ConsolePoint>| {
                point.map(|point| ConsolePoint {
                    line: point.line.saturating_sub(1),
                    col: if point.line == 0 { 0 } else { point.col },
                })
            };
            self.console_selection_anchor = shift_point(self.console_selection_anchor);
            self.console_selection_focus = shift_point(self.console_selection_focus);
            self.console_last_click = self.console_last_click.and_then(|(when, point)| {
                (point.line > 0).then_some((
                    when,
                    ConsolePoint {
                        line: point.line - 1,
                        col: point.col,
                    },
                ))
            });
        }
        self.console_lines.push_back(line);

        // If the user has scrolled up to inspect/select older output, keep the
        // viewport anchored on that same content as new lines arrive. At the
        // live bottom, remain pinned to the newest line as before.
        if preserve_scroll {
            self.console_scroll = (self.console_scroll + 1).min(self.console_max_scroll());
        } else {
            self.console_scroll = 0;
        }
    }

    fn sync_console_log(&mut self) -> bool {
        let mut changed = false;
        while let Ok(record) = self.log_rx.try_recv() {
            let line = if self.console_timestamps {
                format!(
                    "^8[{:02}:{:02}:{:02}]^7 {}",
                    record.local_time[0], record.local_time[1], record.local_time[2], record.text
                )
            } else {
                record.text
            };
            self.append_console_line(line);
            changed = true;
        }
        if changed && self.console_search_open {
            self.rebuild_console_search(false);
        }
        changed
    }

    fn push_console_line(&mut self, line: impl Into<String>) {
        crate::logging::write_line(crate::logging::Level::Info, format_args!("{}", line.into()));
        let _ = self.sync_console_log();
    }

    fn console_point_at(&self, width: u32, height: u32, x: f64, y: f64) -> Option<ConsolePoint> {
        let (row, col) = ui::console_text_hit(width, height, self.console_size, x, y)?;
        let capacity = ui::console_visible_line_capacity(height, self.console_size);
        let scroll = self.console_scroll.min(self.console_max_scroll());
        let end = self.console_lines.len().saturating_sub(scroll);
        let start = end.saturating_sub(capacity);
        let line = start + row;
        if line >= end {
            return None;
        }
        let visible_len = crate::logging::strip_jka_colors(&self.console_lines[line])
            .chars()
            .count();
        Some(ConsolePoint {
            line,
            col: col.min(visible_len),
        })
    }

    fn console_max_scroll(&self) -> usize {
        let visible_lines = self
            .window
            .as_ref()
            .map(|window| {
                ui::console_visible_line_capacity(window.inner_size().height, self.console_size)
            })
            .unwrap_or(1)
            .max(1);
        self.console_lines.len().saturating_sub(visible_lines)
    }

    fn scroll_console(&mut self, rows: i32) {
        if rows > 0 {
            self.console_scroll =
                (self.console_scroll + rows as usize).min(self.console_max_scroll());
        } else if rows < 0 {
            self.console_scroll = self.console_scroll.saturating_sub((-rows) as usize);
        }
    }

    fn console_word_selection(&self, point: ConsolePoint) -> Option<(ConsolePoint, ConsolePoint)> {
        let raw = self.console_lines.get(point.line)?;
        let plain = crate::logging::strip_jka_colors(raw);
        let chars: Vec<char> = plain.chars().collect();
        if chars.is_empty() {
            return None;
        }

        let index = point.col.min(chars.len().saturating_sub(1));
        if chars[index].is_whitespace() {
            return None;
        }

        fn word_char(ch: char) -> bool {
            ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | '\\' | ':' | '@')
        }

        let class_is_word = word_char(chars[index]);
        let same_class = |ch: char| !ch.is_whitespace() && word_char(ch) == class_is_word;
        let mut start = index;
        while start > 0 && same_class(chars[start - 1]) {
            start -= 1;
        }
        let mut end = index + 1;
        while end < chars.len() && same_class(chars[end]) {
            end += 1;
        }
        Some((
            ConsolePoint {
                line: point.line,
                col: start,
            },
            ConsolePoint {
                line: point.line,
                col: end,
            },
        ))
    }

    fn console_line_selection(&self, point: ConsolePoint) -> Option<(ConsolePoint, ConsolePoint)> {
        let raw = self.console_lines.get(point.line)?;
        let len = crate::logging::strip_jka_colors(raw).chars().count();
        (len > 0).then_some((
            ConsolePoint {
                line: point.line,
                col: 0,
            },
            ConsolePoint {
                line: point.line,
                col: len,
            },
        ))
    }

    fn begin_console_selection(&mut self, point: ConsolePoint) {
        const MULTI_CLICK_WINDOW: Duration = Duration::from_millis(450);
        let now = Instant::now();
        let continues = self
            .console_last_click
            .is_some_and(|(last_time, last_point)| {
                now.saturating_duration_since(last_time) <= MULTI_CLICK_WINDOW
                    && last_point.line == point.line
                    && last_point.col.abs_diff(point.col) <= 1
            });
        self.console_click_count = if continues && self.console_click_count < 3 {
            self.console_click_count + 1
        } else {
            1
        };
        self.console_last_click = Some((now, point));

        match self.console_click_count {
            2 => {
                if let Some((start, end)) = self.console_word_selection(point) {
                    self.console_selection_anchor = Some(start);
                    self.console_selection_focus = Some(end);
                    self.console_selecting = false;
                } else {
                    self.console_selection_anchor = Some(point);
                    self.console_selection_focus = Some(point);
                    self.console_selecting = true;
                }
            }
            3 => {
                if let Some((start, end)) = self.console_line_selection(point) {
                    self.console_selection_anchor = Some(start);
                    self.console_selection_focus = Some(end);
                } else {
                    self.console_selection_anchor = Some(point);
                    self.console_selection_focus = Some(point);
                }
                self.console_selecting = false;
            }
            _ => {
                self.console_selection_anchor = Some(point);
                self.console_selection_focus = Some(point);
                self.console_selecting = true;
            }
        }
    }

    fn select_all_console(&mut self) {
        let Some(last_line) = self.console_lines.len().checked_sub(1) else {
            self.console_selection_anchor = None;
            self.console_selection_focus = None;
            self.console_selecting = false;
            return;
        };
        let last_col = self
            .console_lines
            .get(last_line)
            .map(|raw| crate::logging::strip_jka_colors(raw).chars().count())
            .unwrap_or(0);
        self.console_selection_anchor = Some(ConsolePoint { line: 0, col: 0 });
        self.console_selection_focus = Some(ConsolePoint {
            line: last_line,
            col: last_col,
        });
        self.console_selecting = false;
    }

    fn selected_console_text(&self) -> Option<String> {
        let (mut start, mut end) = (
            self.console_selection_anchor?,
            self.console_selection_focus?,
        );
        if (start.line, start.col) > (end.line, end.col) {
            std::mem::swap(&mut start, &mut end);
        }
        if start == end {
            return None;
        }
        let mut out = String::new();
        for line_index in start.line..=end.line {
            let Some(raw) = self.console_lines.get(line_index) else {
                break;
            };
            let plain = crate::logging::strip_jka_colors(raw);
            let chars: Vec<char> = plain.chars().collect();
            let left = if line_index == start.line {
                start.col.min(chars.len())
            } else {
                0
            };
            let right = if line_index == end.line {
                end.col.min(chars.len())
            } else {
                chars.len()
            };
            if right > left {
                out.extend(chars[left..right].iter());
            }
            if line_index != end.line {
                out.push('\n');
            }
        }
        (!out.is_empty()).then_some(out)
    }

    fn console_visible_range(&self) -> (usize, usize) {
        let capacity = self
            .window
            .as_ref()
            .map(|window| {
                ui::console_visible_line_capacity(window.inner_size().height, self.console_size)
            })
            .unwrap_or(1)
            .max(1);
        let scroll = self.console_scroll.min(self.console_max_scroll());
        let end = self.console_lines.len().saturating_sub(scroll);
        (end.saturating_sub(capacity), end)
    }

    fn reveal_console_search_match(&mut self) {
        let Some(hit) = self
            .console_search_index
            .and_then(|index| self.console_search_matches.get(index).copied())
        else {
            return;
        };
        let (visible_start, visible_end) = self.console_visible_range();
        if hit.line >= visible_start && hit.line < visible_end {
            return;
        }

        let capacity = self
            .window
            .as_ref()
            .map(|window| {
                ui::console_visible_line_capacity(window.inner_size().height, self.console_size)
            })
            .unwrap_or(1)
            .max(1);
        let centered_start = hit.line.saturating_sub(capacity / 2);
        let centered_end = (centered_start + capacity).min(self.console_lines.len());
        self.console_scroll = self
            .console_lines
            .len()
            .saturating_sub(centered_end)
            .min(self.console_max_scroll());
    }

    fn rebuild_console_search(&mut self, reveal: bool) {
        let previous = self
            .console_search_index
            .and_then(|index| self.console_search_matches.get(index).copied());
        let visible_start = self.console_visible_range().0;
        let needle = self.console_search_input.to_ascii_lowercase();
        let mut matches = Vec::new();

        if !needle.is_empty() {
            for (line, raw) in self.console_lines.iter().enumerate() {
                let plain = crate::logging::strip_jka_colors(raw);
                let haystack = plain.to_ascii_lowercase();
                for (byte_start, _) in haystack.match_indices(&needle) {
                    let byte_end = byte_start + needle.len();
                    let start_col = plain[..byte_start].chars().count();
                    let end_col = start_col + plain[byte_start..byte_end].chars().count();
                    matches.push(ConsoleSearchMatch {
                        line,
                        start_col,
                        end_col,
                    });
                }
            }
        }

        self.console_search_matches = matches;
        self.console_search_index = if self.console_search_matches.is_empty() {
            None
        } else if let Some(previous) = previous {
            self.console_search_matches
                .iter()
                .position(|hit| hit.line == previous.line && hit.start_col == previous.start_col)
                .or_else(|| {
                    self.console_search_matches
                        .iter()
                        .position(|hit| hit.line >= visible_start)
                })
                .or(Some(0))
        } else {
            self.console_search_matches
                .iter()
                .position(|hit| hit.line >= visible_start)
                .or(Some(0))
        };

        if reveal {
            self.reveal_console_search_match();
        }
    }

    fn open_console_search(&mut self) {
        if !self.console_search_open {
            if let Some(selected) = self.selected_console_text() {
                if !selected.contains('\n') && selected.chars().count() <= 128 {
                    self.console_search_input = selected;
                    self.console_search_matches.clear();
                    self.console_search_index = None;
                }
            }
            self.console_search_open = true;
            self.console_selection_anchor = None;
            self.console_selection_focus = None;
            self.console_selecting = false;
            self.rebuild_console_search(true);
        }
        self.publish_ui();
    }

    fn step_console_search(&mut self, backwards: bool) {
        if self.console_search_matches.is_empty() {
            return;
        }
        let len = self.console_search_matches.len();
        self.console_search_index = Some(match self.console_search_index {
            Some(index) if backwards => (index + len - 1) % len,
            Some(index) => (index + 1) % len,
            None if backwards => len - 1,
            None => 0,
        });
        self.reveal_console_search_match();
    }

    fn paste_into(
        target: &mut String,
        max_len: usize,
        reject_console_toggle: bool,
    ) -> Result<bool, String> {
        let value = crate::clipboard::get_text()?;
        let before = target.len();
        for ch in value.chars() {
            if ch.is_control() || (reject_console_toggle && matches!(ch, '`' | '~')) {
                continue;
            }
            if target.len() >= max_len {
                break;
            }
            target.push(ch);
        }
        Ok(target.len() != before)
    }

    fn begin_chat(&mut self, mode: ChatMode) {
        self.chat_mode = mode;
        self.chat_input.clear();
        self.set_overlay(OverlayMode::Chat);
    }

    fn push_chat_line(&mut self, mode: ChatMode, message: &str) {
        let rendered = match mode {
            // Match JKA/OpenJK chat coloring: the name/punctuation finish in
            // white, then the server inserts the chat-mode color before text.
            ChatMode::Global => format!("^7YOU:^2 {message}"),
            ChatMode::Team => format!("^5(TEAM) ^7YOU:^5 {message}"),
        };
        self.push_console_line(rendered.clone());
        if self.chat_lines.len() >= 16 {
            self.chat_lines.pop_front();
        }
        self.chat_lines.push_back(ChatRecord {
            text: rendered,
            created: Instant::now(),
        });
        self.last_chat_refresh = Instant::now();
    }

    fn submit_chat(&mut self) {
        let message = std::mem::take(&mut self.chat_input);
        let message = message.trim();
        if !message.is_empty() {
            if self.live_connected() {
                // The server echoes chat back as a `chat`/`tchat` command.
                let verb = if self.chat_mode == ChatMode::Team { "say_team" } else { "say" };
                self.forward_command_to_server(&format!("{verb} {message}"));
            } else {
                self.push_chat_line(self.chat_mode, message);
            }
        }
        self.set_overlay(OverlayMode::None);
        self.publish_ui();
    }

    fn console_cvar_value(&self, name: &str) -> Option<String> {
        if let Some(value) = self.network.cvar_value(name) {
            return Some(value);
        }
        let bool_value = |value: bool| if value { "1" } else { "0" }.to_owned();
        Some(match name.to_ascii_lowercase().as_str() {
            "com_maxfps" => self.effective_fps_cap().to_string(),
            "cg_drawfps" => self.video.draw_fps.to_string(),
            "cg_debugevents" => self.cg_debug_events.to_string(),
            "cg_drawcrosshair" => self.crosshair.style.to_string(),
            "cg_crosshairsize" => format!("{:.3}", self.crosshair.size),
            "cg_crosshaircolor" => {
                let [r, g, b, a] = self.crosshair.color;
                format!("{r} {g} {b} {a}")
            }
            "cg_movementkeys" => self.movement_keys_hud.mode.to_string(),
            "cg_movementkeysx" => format!("{:.3}", self.movement_keys_hud.x),
            "cg_movementkeysy" => format!("{:.3}", self.movement_keys_hud.y),
            "cg_movementkeyssize" => format!("{:.3}", self.movement_keys_hud.size),
            "cg_movementkeyswalk" => bool_value(self.movement_keys_hud.walk),
            "cg_strafehelper" => self.strafe_helper.flags.to_string(),
            "cg_strafehelper_fps" => format!("{:.3}", self.strafe_helper.fps),
            "cg_strafehelperoffset" => format!("{:.3}", self.strafe_helper.offset),
            "cg_strafehelperlinewidth" => format!("{:.3}", self.strafe_helper.line_width),
            "cg_strafehelperprecision" => self.strafe_helper.precision.to_string(),
            "cg_strafehelpercutoff" => format!("{:.3}", self.strafe_helper.cutoff),
            "cg_strafehelperactivecolor" => {
                let [r, g, b, a] = self.strafe_helper.active_color;
                format!("{r} {g} {b} {a}")
            }
            "cg_strafehelperinactivealpha" => self.strafe_helper.inactive_alpha.to_string(),
            "cg_hudhealth" => self.hud_layout.health.to_config(),
            "cg_hudshield" => self.hud_layout.shield.to_config(),
            "cg_hudammo" => self.hud_layout.ammo.to_config(),
            "cg_hudforce" => self.hud_layout.force.to_config(),
            "cg_hudsnap" => bool_value(self.hud_layout.snap_to_grid),
            "cg_hudgridsize" => format!("{:.3}", self.hud_layout.grid_size),
            "model" => self.solo_client_info.model_cvar(),
            "con_timestamps" => bool_value(self.console_timestamps),
            "sv_master1" => self.server_browser.master_servers[0].clone(),
            "sv_master2" => self.server_browser.master_servers[1].clone(),
            "sv_master3" => self.server_browser.master_servers[2].clone(),
            "sv_master4" => self.server_browser.master_servers[3].clone(),
            "sv_master5" => self.server_browser.master_servers[4].clone(),
            "sensitivity" => format!("{:.6}", self.mouse_input.sensitivity),
            "m_yaw" => format!("{:.6}", self.mouse_input.yaw),
            "m_pitch" => format!("{:.6}", self.mouse_input.pitch),
            "cl_mouseaccel" => format!("{:.6}", self.mouse_input.accel),
            "cg_fov" => format!("{:.3}", self.camera.cg_fov()),
            "s_volume" => format!("{:.3}", self.audio.effects_volume),
            "s_volumevoice" => format!("{:.3}", self.audio.voice_volume),
            "s_musicvolume" => format!("{:.3}", self.audio.music_volume),
            "s_separation" => format!("{:.3}", self.audio.separation),
            "s_mutewhenunfocused" => bool_value(self.audio.mute_when_unfocused),
            "s_steamaudio" => bool_value(self.audio.steam_audio),
            "cg_thirdperson" => bool_value(self.third_person.enabled),
            "cg_fpls" => bool_value(self.first_person_lightsaber),
            "cg_smoothplayerorigin" => bool_value(self.presentation_smoothing.smooth_player_origin),
            "cg_smooththirdpersonorigin" => bool_value(self.presentation_smoothing.smooth_third_person_origin),
            "cg_smoothplayeranimation" => bool_value(self.presentation_smoothing.smooth_player_animation),
            "cg_subframeplayerangles" => bool_value(self.presentation_smoothing.subframe_player_angles),
            "cg_smooththirdpersontime" => bool_value(self.presentation_smoothing.smooth_third_person_time),
            "cg_thirdpersonalpha" => format!("{:.3}", self.third_person.alpha),
            "cg_thirdpersonangle" => format!("{:.3}", self.third_person.angle),
            "cg_thirdpersoncameradamp" => format!("{:.3}", self.third_person.camera_damp),
            "cg_thirdpersonhorzoffset" => format!("{:.3}", self.third_person.horz_offset),
            "cg_thirdpersonpitchoffset" => format!("{:.3}", self.third_person.pitch_offset),
            "cg_thirdpersonrange" => format!("{:.3}", self.third_person.range),
            "cg_thirdpersonspecialcam" => bool_value(self.third_person.special_cam),
            "cg_thirdpersontargetdamp" => format!("{:.3}", self.third_person.target_damp),
            "cg_thirdpersonvertoffset" => format!("{:.3}", self.third_person.vert_offset),
            "pmove_msec" => self.video.physics_msec.to_string(),
            "cl_input_subframe" => bool_value(self.video.input_subframe),
            "r_physics" => bool_value(self.video.client_physics),
            "r_physicshz" => self.video.client_physics_hz.to_string(),
            "r_physicsmaxsubsteps" => self.video.client_physics_max_substeps.to_string(),
            "r_physicsccd" => bool_value(self.video.client_physics_ccd),
            "r_physicssleeping" => bool_value(self.video.client_physics_sleeping),
            "r_ragdolls" => bool_value(self.video.ragdolls),
            "r_ragdollmax" => self.video.ragdoll_max.to_string(),
            "r_ragdolllifetime" => format!("{:.1}", self.video.ragdoll_lifetime),
            "r_ragdollselfcollision" => bool_value(self.video.ragdoll_self_collision),
            "r_physicsprops" => bool_value(self.video.physics_props),
            "r_physicspropmax" => self.video.physics_prop_max.to_string(),
            "r_physicsdebris" => bool_value(self.video.physics_debris),
            "r_physicsdebrismax" => self.video.physics_debris_max.to_string(),
            "r_physicsdebrislifetime" => format!("{:.1}", self.video.physics_debris_lifetime),
            "r_physicsplayerpush" => bool_value(self.video.physics_player_push),
            "r_physicsweaponimpulses" => bool_value(self.video.physics_weapon_impulses),
            "r_physicsexplosionimpulses" => bool_value(self.video.physics_explosion_impulses),
            "r_physicsforceimpulses" => bool_value(self.video.physics_force_impulses),
            "r_physicsdebug" => bool_value(self.video.physics_debug_draw),
            "r_physicsstats" => bool_value(self.video.physics_stats),
            "r_fullscreen" => self.video.fullscreen.config_value().to_string(),
            "r_backend" => self.video.renderer_backend.config_value().to_owned(),
            "r_swapinterval" => self.video.vsync.config_value().to_string(),
            "r_ext_multisample" => {
                if self.video.msaa_samples <= 1 {
                    "0".into()
                } else {
                    self.video.msaa_samples.to_string()
                }
            }
            "r_texturemode" => match self.video.texture_filter {
                TextureFilter::Nearest => "GL_NEAREST".into(),
                TextureFilter::Bilinear => "GL_LINEAR_MIPMAP_NEAREST".into(),
                _ => "GL_LINEAR_MIPMAP_LINEAR".into(),
            },
            "r_ext_texture_filter_anisotropic" => match self.video.texture_filter {
                TextureFilter::Anisotropic2x => "2".into(),
                TextureFilter::Anisotropic4x => "4".into(),
                TextureFilter::Anisotropic8x => "8".into(),
                TextureFilter::Anisotropic16x => "16".into(),
                _ => "0".into(),
            },
            "r_customwidth" => self.video.resolution[0].to_string(),
            "r_customheight" => self.video.resolution[1].to_string(),
            "r_windowx" => self
                .video
                .window_position
                .map_or_else(|| "auto".into(), |p| p[0].to_string()),
            "r_windowy" => self
                .video
                .window_position
                .map_or_else(|| "auto".into(), |p| p[1].to_string()),
            "r_windowmaximized" => bool_value(self.video.window_maximized),
            "r_gamma" => format!("{:.3}", self.video.gamma),
            "r_hdr" => bool_value(self.video.hdr),
            "r_floatlightmap" => bool_value(self.video.float_lightmap),
            "r_tonemap" => bool_value(self.video.tone_mapping),
            "r_autoexposure" => bool_value(self.video.auto_exposure),
            "r_bloom" => bool_value(self.video.bloom),
            "r_halation" => bool_value(self.video.halation),
            "r_ssao" => bool_value(self.video.ssao),
            "r_staticbspao" => bool_value(self.video.static_bsp_ao),
            "r_staticbspaomode" => if self.video.static_bsp_ao_lightmap {
                "lightmap".into()
            } else {
                "vertex".into()
            },
            "r_staticbspaosamples" => self.video.static_bsp_ao_samples.to_string(),
            "r_staticbspaoresolution" => self.video.static_bsp_ao_resolution.to_string(),
            "r_staticbspaostrength" => self.video.static_bsp_ao_strength.to_string(),
            "r_staticbspaorange" => self.video.static_bsp_ao_range.to_string(),
            "r_staticbspaocurrentcell" => bool_value(self.video.static_bsp_ao_current_cell),
            "r_fxaa" => bool_value(self.video.fxaa),
            "r_smaa" => bool_value(self.video.smaa),
            "r_taa" => bool_value(self.video.taa),
            "r_contactshadows" => bool_value(self.video.contact_shadows),
            "r_cloudshadows" => bool_value(self.video.cloud_shadows),
            "r_cloudrenderresolution" => {
                self.video.cloud_render_resolution.config_value().to_owned()
            }
            "r_cloudtemporal" => bool_value(self.video.cloud_temporal),
            "r_cloudwindvariation" => bool_value(self.video.cloud_wind_variation),
            "r_cloudshapeevolution" => bool_value(self.video.cloud_shape_evolution),
            "r_cloudterraininteraction" => bool_value(self.video.cloud_terrain_interaction),
            "r_cloudemptyskip" => bool_value(self.video.cloud_empty_skip),
            "r_rain" => bool_value(self.video.rain),
            "r_rainintensity" => self.video.rain_intensity.config_value().to_owned(),
            "r_footprints" => self.video.footprints.config_value().to_owned(),
            "r_grass" => bool_value(self.video.grass),
            "r_clouds" => bool_value(self.video.clouds),
            "r_ocean" => bool_value(self.video.ocean),
            "r_grassprecompute" => bool_value(self.video.grass_precompute),
            "r_grassmidlod" => bool_value(self.video.grass_mid_lod),
            "r_grassfronttoback" => bool_value(self.video.grass_front_to_back),
            "r_fogmode" => self.video.fog_mode.config_value().to_owned(),
            "r_fogstrength" => format!("{:.3}", self.video.fog_strength),
            "r_sunoverride" => bool_value(self.video.sun_override),
            "r_sunvisibility" => self.video.sun_visibility.config_value().to_owned(),
            "r_sunyaw" => format!("{:.3}", self.video.sun_yaw),
            "r_sunpitch" => format!("{:.3}", self.video.sun_pitch),
            "r_sunintensity" => format!("{:.3}", self.video.sun_intensity),
            "r_suncolor" => format!(
                "{:.4} {:.4} {:.4}",
                self.video.sun_color[0], self.video.sun_color[1], self.video.sun_color[2]
            ),
            "r_distancecullscale" => format!("{:.3}", self.video.distance_cull_scale),
            "r_reflectionquality" => self.video.reflection_quality.config_value().to_owned(),
            "r_reflectiondebug" => bool_value(self.video.reflection_debug),
            "r_chromaticaberration" => format!("{:.3}", self.video.chromatic_aberration),
            "r_vignette" => bool_value(self.video.vignette),
            "r_filmgrain" => format!("{:.3}", self.video.film_grain_strength),
            "r_motionblur" => format!("{:.3}", self.video.motion_blur_strength),
            "r_depthoffield" => format!("{:.3}", self.video.depth_of_field_strength),
            "r_dofquality" => self.video.dof_quality.config_value().to_owned(),
            "r_colorlut" => self.video.color_lut.config_value().to_owned(),
            "r_colorlutstrength" => format!("{:.3}", self.video.color_lut_strength),
            "r_gpudriven" => bool_value(self.video.gpu_driven),
            "r_hizocclusion" => bool_value(self.video.hiz_occlusion),
            "r_entityambientlighting" => self.video.entity_ambient_lighting.config_value().to_owned(),
            "r_dynamiclights" => self.video.dynamic_lights.config_value().to_owned(),
            "r_modernsabers" => bool_value(self.video.modern_sabers),
            "r_dynamicshadows" => self.video.dynamic_shadows.config_value().to_owned(),
            "r_clusteredlighting" => bool_value(self.video.clustered_lighting),
            "r_emissivearealights" => bool_value(self.video.emissive_area_lights),
            "r_voxelprobegi" => bool_value(self.video.voxel_probe_gi),
            "r_locallightshadows" => bool_value(self.video.local_light_shadows),
            "r_pbr" => bool_value(self.video.pbr),
            "fs_allowassetoverrides" => bool_value(self.video.allow_asset_overrides),
            "r_gennormalmaps" => bool_value(self.video.gen_normal_maps),
            "r_deluxemapping" => bool_value(self.video.deluxe_mapping),
            "r_deluxespecular" => format!("{:.3}", self.video.deluxe_specular),
            "r_cascadedshadows" => bool_value(self.video.cascaded_shadows),
            "r_planardebug" => self
                .video
                .planar_reflection_debug
                .label()
                .to_ascii_lowercase(),
            "r_showtris" => bool_value(self.video.show_wireframe),
            "r_skipui" => bool_value(self.video.skip_ui),
            "developer" => bool_value(self.video.developer_tools),
            "r_perftrace" => bool_value(self.video.perf_trace),
            "r_gputimings" => bool_value(self.video.gpu_timings),
            "r_ghoul2skinning" => self.video.ghoul2_skinning.config_value().to_owned(),
            "r_ghoul2earlycull" => bool_value(self.video.ghoul2_early_cull),
            "r_lodbias" => self.video.ghoul2_lod_bias.to_string(),
            "r_ghoul2batchdraws" => self.video.ghoul2_batch_draws.config_value().to_owned(),
            "r_novis" => bool_value(self.video.pvs_mode == PvsMode::Off),
            "r_pvsmode" => self.video.pvs_mode.label().to_ascii_lowercase(),
            _ => return None,
        })
    }

    fn effective_audio_mix(&self) -> (f32, f32, f32) {
        let muted = self.audio.mute_when_unfocused && !self.window_focused;
        (
            if muted { 0.0 } else { self.audio.effects_volume },
            if muted { 0.0 } else { self.audio.voice_volume },
            self.audio.separation,
        )
    }

    fn apply_audio_mix(&mut self) {
        let (effects, voice, separation) = self.effective_audio_mix();
        if let Some(playback) = &mut self.game_session {
            if let Some(sound) = &mut playback.sound_presenter {
                sound.set_mix(effects, voice, separation);
            }
        }
    }

    fn set_input_subframe(&mut self, enabled: bool) {
        if enabled == self.video.input_subframe {
            return;
        }
        self.video.input_subframe = enabled;
        self.mouse_delta = (0.0, 0.0);
        self.last_mouse_motion_at = None;
        self.pending_mouse_input = None;
        self.last_simulated_mouse_input = None;
        if let Some(player) = &self.local_player {
            if enabled {
                player.camera_subframe(&mut self.camera);
            } else {
                player.camera(&mut self.camera);
            }
        }
        self.mark_config_dirty();
        self.publish_snapshot();
        self.publish_ui();
    }

    fn parse_console_bool(value: &str) -> Option<bool> {
        match value.trim().to_ascii_lowercase().as_str() {
            "1" | "on" | "true" | "yes" => Some(true),
            "0" | "off" | "false" | "no" => Some(false),
            _ => None,
        }
    }

    fn normalize_latched_console_value(&self, name: &str, value: &str) -> Result<String, String> {
        let lower = name.to_ascii_lowercase();
        let boolean = || {
            Self::parse_console_bool(value)
                .map(|enabled| if enabled { "1" } else { "0" }.to_owned())
                .ok_or_else(|| format!("{name}: expected 0/1, off/on, false/true"))
        };
        match lower.as_str() {
            "r_pbr" | "r_gennormalmaps" | "r_floatlightmap" | "fs_allowassetoverrides" => boolean(),
            "r_fullscreen" => FullscreenMode::from_config(value)
                .map(|mode| mode.config_value().to_string())
                .ok_or_else(|| format!("{name}: expected 0 (windowed), 1 (borderless), or 2 (exclusive)")),
            "r_backend" => RendererBackend::from_config(value)
                .map(|backend| backend.config_value().to_owned())
                .ok_or_else(|| format!("{name}: expected vulkan or dx12")),
            "r_customwidth" => value
                .trim()
                .parse::<u32>()
                .map(|pixels| pixels.max(320).to_string())
                .map_err(|_| format!("{name}: expected an integer pixel size")),
            "r_customheight" => value
                .trim()
                .parse::<u32>()
                .map(|pixels| pixels.max(240).to_string())
                .map_err(|_| format!("{name}: expected an integer pixel size")),
            _ => Ok(value.trim().to_owned()),
        }
    }

    fn set_console_cvar_from_console(
        &mut self,
        name: &str,
        value: &str,
    ) -> Result<ConsoleCvarSetResult, String> {
        let entry = crate::console::find(name)
            .ok_or_else(|| format!("Cvar {name} does not exist."))?;
        if entry.latch == crate::console::LatchScope::None {
            self.set_console_cvar(name, value)?;
            return Ok(ConsoleCvarSetResult::Applied);
        }

        let normalized = self.normalize_latched_console_value(entry.name, value)?;
        let key = entry.name.to_ascii_lowercase();
        let current = self
            .console_cvar_value(entry.name)
            .ok_or_else(|| format!("{}: cvar is not writable here", entry.name))?;

        // OpenJK Cvar_Set2 cancels a pending latch when the requested value is
        // set back to the current live value.
        if normalized.eq_ignore_ascii_case(&current) {
            let removed = self.latched_console_cvars.remove(&key).is_some();
            if removed {
                self.mark_config_dirty();
            }
            return Ok(ConsoleCvarSetResult::Unchanged);
        }
        if self
            .latched_console_cvars
            .get(&key)
            .is_some_and(|pending| pending.eq_ignore_ascii_case(&normalized))
        {
            return Ok(ConsoleCvarSetResult::Unchanged);
        }

        self.latched_console_cvars.insert(key, normalized);
        self.mark_config_dirty();
        Ok(ConsoleCvarSetResult::Latched)
    }

    fn apply_latched_console_cvars(&mut self, scope: ApplyLatchedScope) {
        let ready: Vec<_> = self
            .latched_console_cvars
            .iter()
            .filter_map(|(name, value)| {
                let entry = crate::console::find(name)?;
                let apply = match (scope, entry.latch) {
                    (
                        ApplyLatchedScope::MapLoad,
                        crate::console::LatchScope::MapLoadOrVidRestart,
                    ) => true,
                    (
                        ApplyLatchedScope::VidRestart,
                        crate::console::LatchScope::VidRestart
                        | crate::console::LatchScope::MapLoadOrVidRestart,
                    ) => true,
                    _ => false,
                };
                apply.then(|| (entry.name.to_owned(), value.clone()))
            })
            .collect();

        for (name, value) in ready {
            self.latched_console_cvars.remove(&name.to_ascii_lowercase());
            if let Err(error) = self.set_console_cvar(&name, &value) {
                self.push_console_line(format!("^1{error}"));
            }
        }
    }

    fn apply_latched_values_to_settings(&self, settings: &mut VideoSettings) {
        for (name, value) in &self.latched_console_cvars {
            match name.as_str() {
                "r_fullscreen" => {
                    if let Some(mode) = FullscreenMode::from_config(value) {
                        settings.fullscreen = mode;
                    }
                }
                "r_backend" => {
                    if let Some(backend) = RendererBackend::from_config(value) {
                        settings.renderer_backend = backend;
                    }
                }
                "r_customwidth" => {
                    if let Ok(width) = value.parse::<u32>() {
                        settings.resolution[0] = width.max(320);
                    }
                }
                "r_customheight" => {
                    if let Ok(height) = value.parse::<u32>() {
                        settings.resolution[1] = height.max(240);
                    }
                }
                "r_pbr" => settings.pbr = Self::parse_console_bool(value).unwrap_or(settings.pbr),
                "fs_allowassetoverrides" => {
                    settings.allow_asset_overrides =
                        Self::parse_console_bool(value).unwrap_or(settings.allow_asset_overrides)
                }
                "r_gennormalmaps" => {
                    settings.gen_normal_maps =
                        Self::parse_console_bool(value).unwrap_or(settings.gen_normal_maps)
                }
                "r_floatlightmap" => {
                    settings.float_lightmap =
                        Self::parse_console_bool(value).unwrap_or(settings.float_lightmap)
                }
                _ => {}
            }
        }
    }

    fn set_console_cvar(&mut self, name: &str, value: &str) -> Result<(), String> {
        if let Some(result) = self.network.set_cvar(name, value) {
            let userinfo_changed = result?;
            self.mark_config_dirty();
            if userinfo_changed {
                self.send_userinfo();
            }
            return Ok(());
        }
        if name.eq_ignore_ascii_case("model") {
            let result = self.set_console_cvar_inner(name, value);
            if result.is_ok() {
                self.send_userinfo();
            }
            return result;
        }
        self.set_console_cvar_inner(name, value)
    }

    fn set_console_cvar_inner(&mut self, name: &str, value: &str) -> Result<(), String> {
        let lower = name.to_ascii_lowercase();
        let boolean = || {
            Self::parse_console_bool(value)
                .ok_or_else(|| format!("{name}: expected 0/1, off/on, false/true"))
        };
        let finite_number = || {
            value
                .trim()
                .parse::<f32>()
                .ok()
                .filter(|number| number.is_finite())
                .ok_or_else(|| format!("{name}: expected a finite number"))
        };
        match lower.as_str() {
            "con_timestamps" => {
                self.console_timestamps = boolean()?;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "sv_master1" | "sv_master2" | "sv_master3" | "sv_master4" | "sv_master5" => {
                let slot = lower
                    .strip_prefix("sv_master")
                    .and_then(|number| number.parse::<usize>().ok())
                    .and_then(|number| number.checked_sub(1))
                    .filter(|slot| *slot < server_browser::MAX_MASTER_SLOTS)
                    .ok_or_else(|| format!("{name}: invalid master slot"))?;
                let master = value.trim();
                if master.len() > 255 {
                    return Err(format!("{name}: master address is too long"));
                }
                self.server_browser.master_servers[slot] = master.to_owned();
                if !master.is_empty() {
                    self.server_browser.master_drafts[slot] = master.to_owned();
                }
                self.server_browser.status_text =
                    "Master server settings changed — press Refresh to query them".to_owned();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_drawcrosshair" => {
                let style = value
                    .trim()
                    .parse::<u8>()
                    .map_err(|_| "cg_drawCrosshair: expected an integer from 0 to 6".to_owned())?;
                if style > 6 {
                    return Err("cg_drawCrosshair: expected an integer from 0 to 6".to_owned());
                }
                self.crosshair.style = style;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_crosshairsize" => {
                self.crosshair.size = finite_number()?.clamp(4.0, 96.0);
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_crosshaircolor" => {
                let values = value
                    .split_whitespace()
                    .map(str::parse::<i32>)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| "cg_crosshairColor: expected R G B A values from 0 to 255".to_owned())?;
                if values.len() != 4 {
                    return Err("cg_crosshairColor: expected R G B A values from 0 to 255".to_owned());
                }
                self.crosshair.color = [
                    values[0].clamp(0, 255) as u8,
                    values[1].clamp(0, 255) as u8,
                    values[2].clamp(0, 255) as u8,
                    values[3].clamp(0, 255) as u8,
                ];
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_hudhealth" | "cg_hudshield" | "cg_hudammo" | "cg_hudforce" => {
                let layout = ui::HudElementLayout::from_config(value).ok_or_else(|| {
                    format!("{name}: expected <anchor> <x> <y> <scale>, e.g. bl 24 -58 1")
                })?;
                let id = match lower.as_str() {
                    "cg_hudhealth" => HudElementId::Health,
                    "cg_hudshield" => HudElementId::Shield,
                    "cg_hudammo" => HudElementId::Ammo,
                    _ => HudElementId::Force,
                };
                *self.hud_layout.element_mut(id) = layout;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_hudsnap" => {
                self.hud_layout.snap_to_grid = boolean()?;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_hudgridsize" => {
                self.hud_layout.grid_size = finite_number()?.clamp(1.0, 64.0);
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_movementkeys" => {
                let mode = value.trim().parse::<u8>().map_err(|_| "cg_movementKeys: expected 0..4".to_owned())?;
                if mode > 4 { return Err("cg_movementKeys: expected 0..4".to_owned()); }
                self.movement_keys_hud.mode = mode;
                self.mark_config_dirty(); self.publish_ui();
            }
            "cg_movementkeysx" => { self.movement_keys_hud.x = finite_number()?.clamp(-640.0, 640.0); self.mark_config_dirty(); self.publish_ui(); }
            "cg_movementkeysy" => { self.movement_keys_hud.y = finite_number()?.clamp(-480.0, 480.0); self.mark_config_dirty(); self.publish_ui(); }
            "cg_movementkeyssize" => { self.movement_keys_hud.size = finite_number()?.clamp(0.25, 4.0); self.mark_config_dirty(); self.publish_ui(); }
            "cg_movementkeyswalk" => { self.movement_keys_hud.walk = boolean()?; self.mark_config_dirty(); self.publish_ui(); }
            "cg_strafehelper" => {
                self.strafe_helper.flags = value.trim().parse::<u32>().map_err(|_| "cg_strafeHelper: expected a non-negative bitmask".to_owned())?;
                self.mark_config_dirty(); self.publish_ui();
            }
            "cg_strafehelper_fps" => { self.strafe_helper.fps = finite_number()?.clamp(0.0, 1000.0); self.mark_config_dirty(); self.publish_ui(); }
            "cg_strafehelperoffset" => { self.strafe_helper.offset = finite_number()?.clamp(-1000.0, 1000.0); self.mark_config_dirty(); self.publish_ui(); }
            "cg_strafehelperlinewidth" => { self.strafe_helper.line_width = finite_number()?.clamp(0.25, 5.0); self.mark_config_dirty(); self.publish_ui(); }
            "cg_strafehelperprecision" => {
                self.strafe_helper.precision = value.trim().parse::<u32>().map_err(|_| "cg_strafeHelperPrecision: expected 100..10000".to_owned())?.clamp(100, 10000);
                self.mark_config_dirty(); self.publish_ui();
            }
            "cg_strafehelpercutoff" => { self.strafe_helper.cutoff = finite_number()?.clamp(0.0, 480.0); self.mark_config_dirty(); self.publish_ui(); }
            "cg_strafehelperactivecolor" => {
                let values = value.split_whitespace().map(str::parse::<i32>).collect::<Result<Vec<_>, _>>()
                    .map_err(|_| "cg_strafeHelperActiveColor: expected R G B A values from 0 to 255".to_owned())?;
                if values.len() != 4 { return Err("cg_strafeHelperActiveColor: expected R G B A values from 0 to 255".to_owned()); }
                self.strafe_helper.active_color = [values[0].clamp(0,255) as u8, values[1].clamp(0,255) as u8, values[2].clamp(0,255) as u8, values[3].clamp(0,255) as u8];
                self.mark_config_dirty(); self.publish_ui();
            }
            "cg_strafehelperinactivealpha" => {
                self.strafe_helper.inactive_alpha = value.trim().parse::<i32>().map_err(|_| "cg_strafeHelperInactiveAlpha: expected 0..255".to_owned())?.clamp(0,255) as u8;
                self.mark_config_dirty(); self.publish_ui();
            }
            "model" => {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    return Err("model: expected model[/skin]".to_owned());
                }
                self.solo_client_info = ClientInfo::solo_model(trimmed);
                self.mark_config_dirty();
                self.update_solo_player_view_and_presentation();
                self.publish_snapshot();
                self.publish_ui();
            }
            "sensitivity" => {
                self.mouse_input.sensitivity = finite_number()?;
                if let Some(player) = &mut self.local_player {
                    player.set_mouse_input_settings(self.mouse_input);
                }
                self.mark_config_dirty();
                self.publish_ui();
            }
            "m_yaw" => {
                self.mouse_input.yaw = finite_number()?;
                if let Some(player) = &mut self.local_player {
                    player.set_mouse_input_settings(self.mouse_input);
                }
                self.mark_config_dirty();
                self.publish_ui();
            }
            "m_pitch" => {
                self.mouse_input.pitch = finite_number()?;
                if let Some(player) = &mut self.local_player {
                    player.set_mouse_input_settings(self.mouse_input);
                }
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cl_mouseaccel" => {
                self.mouse_input.accel = finite_number()?;
                if let Some(player) = &mut self.local_player {
                    player.set_mouse_input_settings(self.mouse_input);
                }
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_fov" => {
                let fov = finite_number()?;
                if !(crate::camera::MIN_CG_FOV..=crate::camera::MAX_CG_FOV).contains(&fov) {
                    return Err(format!(
                        "{name}: expected {}..{} degrees",
                        crate::camera::MIN_CG_FOV,
                        crate::camera::MAX_CG_FOV
                    ));
                }
                self.camera.set_cg_fov(fov);
                self.mark_config_dirty();
                self.publish_snapshot();
                self.publish_ui();
            }
            "s_volume" => {
                self.audio.effects_volume = finite_number()?.clamp(0.0, 1.0);
                self.apply_audio_mix();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "s_volumevoice" => {
                self.audio.voice_volume = finite_number()?.clamp(0.0, 1.0);
                self.apply_audio_mix();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "s_musicvolume" => {
                self.audio.music_volume = finite_number()?.clamp(0.0, 1.0);
                self.mark_config_dirty();
                self.publish_ui();
            }
            "s_separation" => {
                self.audio.separation = finite_number()?.clamp(0.0, 1.0);
                self.apply_audio_mix();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "s_mutewhenunfocused" => {
                self.audio.mute_when_unfocused = boolean()?;
                self.apply_audio_mix();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "s_steamaudio" => {
                let enabled = boolean()?;
                self.audio.steam_audio = enabled;
                if !enabled {
                    self.steam_audio_bake = None;
                    self.steam_audio_bake_progress = None;
                    self.steam_audio_bake_error = None;
                }
                let acoustic_mesh = self.steam_audio_acoustic_mesh.clone();
                let bake = self.steam_audio_bake.clone();
                if let Some(sound) = self
                    .game_session
                    .as_mut()
                    .and_then(|session| session.sound_presenter.as_mut())
                {
                    sound.set_steam_audio_enabled(enabled);
                    sound.set_steam_audio_map(acoustic_mesh, bake);
                }
                println!(
                    "Steam Audio: {} ({})",
                    if enabled { "enabled" } else { "disabled" },
                    if enabled {
                        "acoustic geometry/cache lookup will run on the next map load; first-time bakes continue in the background"
                    } else {
                        "runtime use and future acoustic BSP/bake preparation gated off"
                    }
                );
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_thirdperson" => {
                self.third_person.enabled = boolean()?;
                self.third_person_camera.reset();
                self.mark_config_dirty();
                self.update_solo_player_view_and_presentation();
                self.publish_snapshot();
                self.publish_ui();
            }
            "cg_fpls" => {
                self.first_person_lightsaber = boolean()?;
                self.third_person_camera.reset();
                self.mark_config_dirty();
                self.update_solo_player_view_and_presentation();
                self.publish_snapshot();
                self.publish_ui();
            }
            "cg_smoothplayerorigin" => {
                self.presentation_smoothing.smooth_player_origin = boolean()?;
                self.mark_config_dirty();
                self.update_solo_player_view_and_presentation();
                self.publish_snapshot();
                self.publish_ui();
            }
            "cg_smooththirdpersonorigin" => {
                self.presentation_smoothing.smooth_third_person_origin = boolean()?;
                self.third_person_camera.reset();
                self.mark_config_dirty();
                self.update_solo_player_view_and_presentation();
                self.publish_snapshot();
                self.publish_ui();
            }
            "cg_smoothplayeranimation" => {
                self.presentation_smoothing.smooth_player_animation = boolean()?;
                self.mark_config_dirty();
                self.update_solo_player_view_and_presentation();
                self.publish_snapshot();
                self.publish_ui();
            }
            "cg_subframeplayerangles" => {
                self.presentation_smoothing.subframe_player_angles = boolean()?;
                self.mark_config_dirty();
                self.update_solo_player_view_and_presentation();
                self.publish_snapshot();
                self.publish_ui();
            }
            "cg_smooththirdpersontime" => {
                self.presentation_smoothing.smooth_third_person_time = boolean()?;
                self.third_person_camera.reset();
                self.mark_config_dirty();
                self.update_solo_player_view_and_presentation();
                self.publish_snapshot();
                self.publish_ui();
            }
            "cg_thirdpersonalpha" => {
                self.third_person.alpha = value.trim().parse::<f32>().map_err(|_| format!("{name}: expected a number"))?;
                self.mark_config_dirty();
            }
            "cg_thirdpersonangle" => {
                self.third_person.angle = value.trim().parse::<f32>().map_err(|_| format!("{name}: expected a number"))?;
                self.mark_config_dirty();
            }
            "cg_thirdpersoncameradamp" => {
                self.third_person.camera_damp = value.trim().parse::<f32>().map_err(|_| format!("{name}: expected a number"))?;
                self.mark_config_dirty();
            }
            "cg_thirdpersonhorzoffset" => {
                self.third_person.horz_offset = value.trim().parse::<f32>().map_err(|_| format!("{name}: expected a number"))?;
                self.mark_config_dirty();
            }
            "cg_thirdpersonpitchoffset" => {
                self.third_person.pitch_offset = value.trim().parse::<f32>().map_err(|_| format!("{name}: expected a number"))?;
                self.mark_config_dirty();
            }
            "cg_thirdpersonrange" => {
                self.third_person.range = value.trim().parse::<f32>().map_err(|_| format!("{name}: expected a number"))?;
                self.mark_config_dirty();
            }
            "cg_thirdpersonspecialcam" => {
                self.third_person.special_cam = boolean()?;
                // CVAR_NONE in TaystJK/OpenJK: runtime only, not archived.
                self.publish_ui();
            }
            "cg_thirdpersontargetdamp" => {
                self.third_person.target_damp = value.trim().parse::<f32>().map_err(|_| format!("{name}: expected a number"))?;
                self.mark_config_dirty();
            }
            "cg_thirdpersonvertoffset" => {
                self.third_person.vert_offset = value.trim().parse::<f32>().map_err(|_| format!("{name}: expected a number"))?;
                self.mark_config_dirty();
            }
            "com_maxfps" => {
                let cap = config::normalize_fps_cap(value, self.video.fps_cap);
                if value.trim().parse::<u32>().is_err() {
                    return Err(format!("{name}: expected an integer FPS cap"));
                }
                self.set_fps_cap(cap);
            }
            "cg_drawfps" => {
                let mode = value
                    .trim()
                    .parse::<u8>()
                    .map_err(|_| format!("{name}: expected 0, 1, or 2"))?;
                if mode > 2 {
                    return Err(format!("{name}: expected 0, 1, or 2"));
                }
                self.video.draw_fps = mode;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "cg_debugevents" => {
                let mode = value
                    .trim()
                    .parse::<u8>()
                    .map_err(|_| format!("{name}: expected 0, 1, 2, or 3"))?;
                if mode > 3 {
                    return Err(format!("{name}: expected 0, 1, 2, or 3"));
                }
                self.cg_debug_events = mode;
                self.push_console_line(match mode {
                    0 => "^3cg_debugEvents:^7 OFF".to_owned(),
                    1 => "^3cg_debugEvents:^7 accepted events + current dispatch status".to_owned(),
                    2 => "^3cg_debugEvents:^7 accepted events + suppressed duplicate/zero candidates".to_owned(),
                    _ => "^3cg_debugEvents:^7 verbose entity/resource diagnostics".to_owned(),
                });
            }
            "pmove_msec" => {
                let parsed = value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| format!("{name}: expected an integer timestep in milliseconds"))?;
                self.set_physics_msec(parsed.clamp(1, 33));
            }
            "cl_input_subframe" => {
                let enabled = boolean()?;
                self.set_input_subframe(enabled);
            }
            "r_physics" | "r_physicsccd" | "r_physicssleeping" | "r_ragdolls"
            | "r_ragdollselfcollision" | "r_physicsprops" | "r_physicsdebris"
            | "r_physicsplayerpush" | "r_physicsweaponimpulses"
            | "r_physicsexplosionimpulses" | "r_physicsforceimpulses"
            | "r_physicsdebug" | "r_physicsstats" => {
                let enabled = boolean()?;
                match lower.as_str() {
                    "r_physics" => self.video.client_physics = enabled,
                    "r_physicsccd" => self.video.client_physics_ccd = enabled,
                    "r_physicssleeping" => self.video.client_physics_sleeping = enabled,
                    "r_ragdolls" => self.video.ragdolls = enabled,
                    "r_ragdollselfcollision" => self.video.ragdoll_self_collision = enabled,
                    "r_physicsprops" => self.video.physics_props = enabled,
                    "r_physicsdebris" => self.video.physics_debris = enabled,
                    "r_physicsplayerpush" => self.video.physics_player_push = enabled,
                    "r_physicsweaponimpulses" => self.video.physics_weapon_impulses = enabled,
                    "r_physicsexplosionimpulses" => self.video.physics_explosion_impulses = enabled,
                    "r_physicsforceimpulses" => self.video.physics_force_impulses = enabled,
                    "r_physicsdebug" => self.video.physics_debug_draw = enabled,
                    "r_physicsstats" => self.video.physics_stats = enabled,
                    _ => unreachable!(),
                }
                self.mark_config_dirty();
                if lower == "r_physics" && self.map_prepare_restart_required() {
                    self.console_status =
                        "CLIENT PHYSICS: ON - PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART TO BUILD MAP COLLISION".into();
                }
                self.publish_ui();
            }
            "r_physicshz" => {
                let requested = value.trim().parse::<u32>()
                    .map_err(|_| format!("{name}: expected 30, 60, 120, or 240"))?;
                if ![30_u32, 60, 120, 240].contains(&requested) {
                    return Err(format!("{name}: expected 30, 60, 120, or 240"));
                }
                self.video.client_physics_hz = requested;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_physicsmaxsubsteps" => {
                let requested = value.trim().parse::<u32>()
                    .map_err(|_| format!("{name}: expected 1, 2, 4, or 8"))?;
                if ![1_u32, 2, 4, 8].contains(&requested) {
                    return Err(format!("{name}: expected 1, 2, 4, or 8"));
                }
                self.video.client_physics_max_substeps = requested;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_ragdollmax" => {
                let requested = value.trim().parse::<u32>()
                    .map_err(|_| format!("{name}: expected 2, 4, 8, 16, or 32"))?;
                if ![2_u32, 4, 8, 16, 32].contains(&requested) {
                    return Err(format!("{name}: expected 2, 4, 8, 16, or 32"));
                }
                self.video.ragdoll_max = requested;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_ragdolllifetime" => {
                let requested = value.trim().parse::<u32>()
                    .map_err(|_| format!("{name}: expected 5, 10, 20, 30, or 60"))?;
                if ![5_u32, 10, 20, 30, 60].contains(&requested) {
                    return Err(format!("{name}: expected 5, 10, 20, 30, or 60"));
                }
                self.video.ragdoll_lifetime = requested as f32;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_physicspropmax" => {
                let requested = value.trim().parse::<u32>()
                    .map_err(|_| format!("{name}: expected 32, 64, 96, 192, or 384"))?;
                if ![32_u32, 64, 96, 192, 384].contains(&requested) {
                    return Err(format!("{name}: expected 32, 64, 96, 192, or 384"));
                }
                self.video.physics_prop_max = requested;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_physicsdebrismax" => {
                let requested = value.trim().parse::<u32>()
                    .map_err(|_| format!("{name}: expected 64, 128, 192, 384, or 768"))?;
                if ![64_u32, 128, 192, 384, 768].contains(&requested) {
                    return Err(format!("{name}: expected 64, 128, 192, 384, or 768"));
                }
                self.video.physics_debris_max = requested;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_physicsdebrislifetime" => {
                let requested = value.trim().parse::<u32>()
                    .map_err(|_| format!("{name}: expected 2, 5, 10, 20, or 30"))?;
                if ![2_u32, 5, 10, 20, 30].contains(&requested) {
                    return Err(format!("{name}: expected 2, 5, 10, 20, or 30"));
                }
                self.video.physics_debris_lifetime = requested as f32;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_fullscreen" => {
                let mode = FullscreenMode::from_config(value).ok_or_else(|| {
                    format!("{name}: expected 0 (windowed), 1 (borderless), or 2 (exclusive)")
                })?;
                if mode != self.video.fullscreen {
                    self.set_fullscreen_mode(mode);
                }
            }
            "r_backend" => {
                self.video.renderer_backend = RendererBackend::from_config(value)
                    .ok_or_else(|| format!("{name}: expected vulkan or dx12"))?;
                self.mark_config_dirty();
                self.console_status = format!(
                    "RENDER BACKEND: {} (RUN VID_RESTART TO APPLY)",
                    self.video.renderer_backend.label()
                );
                self.publish_ui();
            }
            "r_swapinterval" => {
                let previous_vsync = self.video.vsync;
                self.video.vsync = VsyncMode::from_config(value).ok_or_else(|| {
                    format!("{name}: expected 0/off, 1/on, 2/fast, or 3/adaptive")
                })?;
                self.render_command(RenderCommand::SetVsync(self.video.vsync));
                self.mark_config_dirty();
                if self.video.vsync != previous_vsync {
                    // Entering FAST on DX12 can make the standing cap unreachable.
                    self.set_fps_cap(self.video.fps_cap);
                }
                self.publish_ui();
            }
            "r_ext_multisample" => {
                let samples = value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| format!("{name}: expected an integer sample count"))?;
                let samples = if samples <= 1 { 1 } else { samples };
                if !self.supported_msaa.contains(&samples) {
                    return Err(format!(
                        "{name}: unsupported sample count {samples}; supported {:?}",
                        self.supported_msaa
                    ));
                }
                self.video.msaa_samples = samples;
                if samples > 1 {
                    self.video.fxaa = false;
                    self.video.smaa = false;
                    self.video.taa = false;
                }
                self.render_command(RenderCommand::SetMsaa(samples));
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_texturemode" => {
                self.video.texture_filter = match value.trim().to_ascii_uppercase().as_str() {
                    "GL_NEAREST" | "GL_NEAREST_MIPMAP_NEAREST" | "GL_NEAREST_MIPMAP_LINEAR" => {
                        TextureFilter::Nearest
                    }
                    "GL_LINEAR" | "GL_LINEAR_MIPMAP_NEAREST" => TextureFilter::Bilinear,
                    "GL_LINEAR_MIPMAP_LINEAR" => TextureFilter::Trilinear,
                    _ => return Err(format!("{name}: unknown texture mode")),
                };
                self.render_command(RenderCommand::SetTextureFilter(self.video.texture_filter));
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_ext_texture_filter_anisotropic" => {
                let level = value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| format!("{name}: expected 0, 2, 4, 8, or 16"))?;
                self.video.texture_filter = match level {
                    0 | 1 => TextureFilter::Trilinear,
                    2 => TextureFilter::Anisotropic2x,
                    4 => TextureFilter::Anisotropic4x,
                    8 => TextureFilter::Anisotropic8x,
                    16 => TextureFilter::Anisotropic16x,
                    _ => return Err(format!("{name}: expected 0, 2, 4, 8, or 16")),
                };
                self.render_command(RenderCommand::SetTextureFilter(self.video.texture_filter));
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_customwidth" | "r_customheight" => {
                let parsed = value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| format!("{name}: expected an integer pixel size"))?;
                if lower == "r_customwidth" {
                    self.video.resolution[0] = parsed.max(320);
                } else {
                    self.video.resolution[1] = parsed.max(240);
                }
                self.mark_config_dirty();
                self.console_status = format!(
                    "RESOLUTION: {}X{} (PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART)",
                    self.video.resolution[0], self.video.resolution[1]
                );
                self.publish_ui();
            }
            "r_windowx" | "r_windowy" => {
                if value.eq_ignore_ascii_case("auto") {
                    self.video.window_position = None;
                } else {
                    let parsed = value.trim().parse::<i32>().map_err(|_| {
                        format!("{name}: expected a desktop pixel coordinate or auto")
                    })?;
                    let mut position = self.video.window_position.unwrap_or([0, 0]);
                    if lower == "r_windowx" {
                        position[0] = parsed;
                    } else {
                        position[1] = parsed;
                    }
                    self.video.window_position = Some(position);
                    if self.applied_fullscreen.is_windowed() {
                        if let Some(window) = &self.window {
                            window.set_outer_position(PhysicalPosition::new(
                                position[0],
                                position[1],
                            ));
                        }
                    }
                }
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_windowmaximized" => {
                self.video.window_maximized = boolean()?;
                if self.applied_fullscreen.is_windowed() {
                    if let Some(window) = &self.window {
                        window.set_maximized(self.video.window_maximized);
                    }
                }
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_gamma" => {
                let gamma = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected a number"))?;
                self.set_gamma(gamma);
            }
            "r_showtris" => {
                let enabled = boolean()?;
                if enabled && !self.wireframe_supported {
                    return Err("r_showtris: wireframe overlay is not supported by this GPU".into());
                }
                self.video.show_wireframe = enabled;
                self.render_command(RenderCommand::SetWireframe(enabled));
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_skipui" => {
                self.video.skip_ui = boolean()?;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "developer" => {
                self.video.developer_tools = boolean()?;
                if !self.video.developer_tools {
                    self.surface_inspector = None;
                    self.render_command(RenderCommand::ClearSurfaceInspection);
                }
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_novis" => {
                self.video.pvs_mode = if boolean()? {
                    PvsMode::Off
                } else {
                    PvsMode::Auto
                };
                self.render_command(RenderCommand::SetPvsMode(self.video.pvs_mode));
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_pvsmode" => {
                self.video.pvs_mode = match value.trim().to_ascii_lowercase().as_str() {
                    "off" | "0" => PvsMode::Off,
                    "minimal" | "1" => PvsMode::Minimal,
                    "full" | "2" => PvsMode::Full,
                    "auto" | "3" => PvsMode::Auto,
                    _ => return Err(format!("{name}: expected off|minimal|full|auto")),
                };
                self.render_command(RenderCommand::SetPvsMode(self.video.pvs_mode));
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_grass" => {
                self.video.grass = boolean()?;
                self.mark_config_dirty();
                self.console_status = format!(
                    "PROCEDURAL GRASS: {} (PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART)",
                    if self.video.grass { "ON" } else { "OFF" }
                );
                self.publish_ui();
            }
            "r_clouds" => {
                self.video.clouds = boolean()?;
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_ocean" => {
                self.video.ocean = boolean()?;
                // OFF releases the FFT resources immediately, and ON can be
                // restored live only when the loaded world already carries its
                // promoted ocean clipmap.
                if !self.video.ocean || self.applied_ocean {
                    self.render_command(RenderCommand::SetOceanEnabled(self.video.ocean));
                }
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_perftrace" => {
                self.video.perf_trace = boolean()?;
                self.render_command(RenderCommand::SetPerfTrace(self.video.perf_trace));
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_gputimings" => {
                self.video.gpu_timings = boolean()?;
                self.render_command(RenderCommand::SetGpuTimings(self.video.gpu_timings));
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_ghoul2skinning" => {
                self.video.ghoul2_skinning = Ghoul2SkinningMode::from_config(value)
                    .ok_or_else(|| format!("{name}: expected cpu|workers|gpu"))?;
                self.mark_config_dirty();
                self.publish_ui();
                self.console_status = format!(
                    "GHOUL2 SKINNING: {}",
                    self.video.ghoul2_skinning.label()
                );
            }
            "r_ghoul2earlycull" => {
                self.video.ghoul2_early_cull = boolean()?;
                self.mark_config_dirty();
                self.publish_ui();
                self.console_status = format!(
                    "GHOUL2 EARLY CULL: {}",
                    if self.video.ghoul2_early_cull { "ON" } else { "OFF" }
                );
            }
            "r_lodbias" => {
                let bias = value
                    .trim()
                    .parse::<i32>()
                    .map_err(|_| format!("{name}: expected a non-negative integer"))?;
                self.video.ghoul2_lod_bias = bias.max(0);
                self.mark_config_dirty();
                self.publish_ui();
                self.console_status = format!("MODEL LOD BIAS: {}", self.video.ghoul2_lod_bias);
            }
            "r_ghoul2batchdraws" => {
                self.video.ghoul2_batch_draws = Ghoul2BatchMode::from_config(value)
                    .ok_or_else(|| format!("{name}: expected 0|1|2 or off|adaptive|force"))?;
                self.render_command(RenderCommand::SetGhoul2BatchDraws(
                    self.video.ghoul2_batch_draws,
                ));
                self.mark_config_dirty();
                self.publish_ui();
                self.console_status = format!(
                    "GHOUL2 BATCH DRAWS: {}",
                    self.video.ghoul2_batch_draws.label()
                );
            }
            "r_grassprecompute" => {
                self.video.grass_precompute = boolean()?;
                self.render_command(RenderCommand::SetGrassPrecompute(self.video.grass_precompute));
                println!("Grass A/B: precompute={}", u8::from(self.video.grass_precompute));
            }
            "r_grassmidlod" => {
                self.video.grass_mid_lod = boolean()?;
                self.render_command(RenderCommand::SetGrassMidLod(self.video.grass_mid_lod));
                println!("Grass A/B: mid_lod={}", u8::from(self.video.grass_mid_lod));
            }
            "r_grassfronttoback" => {
                self.video.grass_front_to_back = boolean()?;
                self.render_command(RenderCommand::SetGrassFrontToBack(
                    self.video.grass_front_to_back,
                ));
                println!(
                    "Grass A/B: front_to_back={}",
                    u8::from(self.video.grass_front_to_back)
                );
            }
            "r_staticbspaomode" => {
                self.video.static_bsp_ao_lightmap = match value.to_ascii_lowercase().as_str() {
                    "lightmap" | "lightmap-space" | "1" => true,
                    "vertex" | "per-vertex" | "0" => false,
                    _ => return Err("usage: r_staticBspAoMode <vertex|lightmap>".into()),
                };
                if self.video.static_bsp_ao {
                    self.video.ssao = false;
                }
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
                return Ok(());
            }
            "r_staticbspaosamples" => {
                let requested = value
                    .parse::<u32>()
                    .map_err(|_| "usage: r_staticBspAoSamples <8|16|32|64|128>".to_string())?;
                const VALUES: [u32; 5] = [8, 16, 32, 64, 128];
                let Some(&samples) = VALUES.iter().find(|&&samples| samples == requested) else {
                    return Err("usage: r_staticBspAoSamples <8|16|32|64|128>".into());
                };
                self.video.static_bsp_ao_samples = samples;
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
                return Ok(());
            }
            "r_staticbspaoresolution" => {
                let requested = value
                    .parse::<u32>()
                    .map_err(|_| "usage: r_staticBspAoResolution <1|3|5>".to_string())?;
                let Some(&scale) = [1_u32, 3, 5].iter().find(|&&scale| scale == requested) else {
                    return Err("usage: r_staticBspAoResolution <1|3|5>".into());
                };
                self.video.static_bsp_ao_resolution = scale;
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
                return Ok(());
            }
            "r_staticbspaostrength" => {
                let requested = value.parse::<u32>().map_err(|_| "usage: r_staticBspAoStrength <25|50|75|100>".to_string())?;
                let Some(&strength) = [25_u32, 50, 75, 100].iter().find(|&&v| v == requested) else {
                    return Err("usage: r_staticBspAoStrength <25|50|75|100>".into());
                };
                self.video.static_bsp_ao_strength = strength;
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
                return Ok(());
            }
            "r_staticbspaorange" => {
                let requested = value.parse::<u32>().map_err(|_| "usage: r_staticBspAoRange <50|100|150|200>".to_string())?;
                let Some(&range) = [50_u32, 100, 150, 200].iter().find(|&&v| v == requested) else {
                    return Err("usage: r_staticBspAoRange <50|100|150|200>".into());
                };
                self.video.static_bsp_ao_range = range;
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
                return Ok(());
            }
            "r_staticbspaocurrentcell" => {
                self.video.static_bsp_ao_current_cell = boolean()?;
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
                return Ok(());
            }
            "r_hdr" | "r_tonemap" | "r_autoexposure" | "r_bloom" | "r_halation" | "r_ssao" | "r_staticbspao" | "r_fxaa" | "r_smaa"
            | "r_taa" | "r_contactshadows" | "r_cloudshadows" | "r_cloudtemporal"
            | "r_cloudwindvariation" | "r_cloudshapeevolution" | "r_cloudterraininteraction" | "r_cloudemptyskip" | "r_rain"
            | "r_sunoverride" | "r_reflectiondebug" | "r_vignette" => {
                let enabled = boolean()?;
                match lower.as_str() {
                    "r_hdr" => self.video.hdr = enabled,
                    "r_tonemap" => self.video.tone_mapping = enabled,
                    "r_autoexposure" => self.video.auto_exposure = enabled,
                    "r_bloom" => self.video.bloom = enabled,
                    "r_halation" => self.video.halation = enabled,
                    "r_ssao" => {
                        self.video.ssao = enabled;
                        if enabled {
                            self.video.static_bsp_ao = false;
                        }
                    }
                    "r_staticbspao" => {
                        self.video.static_bsp_ao = enabled;
                        if enabled {
                            self.video.ssao = false;
                        }
                    }
                    "r_fxaa" => {
                        self.video.fxaa = enabled;
                        if enabled {
                            self.video.smaa = false;
                            self.video.taa = false;
                            self.video.msaa_samples = 1;
                            self.render_command(RenderCommand::SetMsaa(1));
                        }
                    }
                    "r_smaa" => {
                        self.video.smaa = enabled;
                        if enabled {
                            self.video.fxaa = false;
                            self.video.taa = false;
                            self.video.msaa_samples = 1;
                            self.render_command(RenderCommand::SetMsaa(1));
                        }
                    }
                    "r_taa" => {
                        self.video.taa = enabled;
                        if enabled {
                            self.video.fxaa = false;
                            self.video.smaa = false;
                            self.video.msaa_samples = 1;
                            self.render_command(RenderCommand::SetMsaa(1));
                        }
                    }
                    "r_contactshadows" => self.video.contact_shadows = enabled,
                    "r_cloudshadows" => self.video.cloud_shadows = enabled,
                    "r_cloudtemporal" => self.video.cloud_temporal = enabled,
                    "r_cloudwindvariation" => self.video.cloud_wind_variation = enabled,
                    "r_cloudshapeevolution" => self.video.cloud_shape_evolution = enabled,
                    "r_cloudterraininteraction" => self.video.cloud_terrain_interaction = enabled,
                    "r_cloudemptyskip" => self.video.cloud_empty_skip = enabled,
                    "r_rain" => self.video.rain = enabled,
                    "r_sunoverride" => {
                        self.set_sun_override(enabled, enabled);
                        return Ok(());
                    }
                    "r_reflectiondebug" => self.video.reflection_debug = enabled,
                    "r_vignette" => self.video.vignette = enabled,
                    _ => unreachable!(),
                }
                self.sync_post_effects();
                if lower != "r_reflectiondebug" {
                    self.mark_config_dirty();
                }
                // r_floatLightmap is only effective while HDR is active, matching
                // Rend2. If that changes the prepared lightmap representation,
                // video_restart_required() exposes the normal renderer-restart path.
                self.publish_ui();
            }
            "r_reflectionquality" => {
                self.video.reflection_quality = ReflectionQuality::from_config(value)
                    .ok_or_else(|| format!("{name}: expected off|low|medium|high|ultra"))?;
                self.mark_config_dirty();
                self.console_status = format!(
                    "REFLECTION QUALITY: {} - RUN VID_RESTART TO APPLY",
                    self.video.reflection_quality.label()
                );
                self.publish_ui();
                return Ok(());
            }
            "r_sunvisibility" => {
                self.video.sun_visibility = SunVisibilityMode::from_config(value)
                    .ok_or_else(|| format!("{name}: expected legacy|sky|filtered"))?;
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_cloudrenderresolution" => {
                self.video.cloud_render_resolution = CloudRenderResolution::from_config(value)
                    .ok_or_else(|| format!("{name}: expected full|half|quarter"))?;
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_rainintensity" => {
                self.video.rain_intensity = RainIntensity::from_config(value)
                    .ok_or_else(|| format!("{name}: expected light|rain|heavy"))?;
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_footprints" => {
                self.video.footprints = FootprintMode::from_config(value)
                    .ok_or_else(|| format!("{name}: expected off|2d|3d"))?;
                self.render_command(RenderCommand::SetFootprintMode(self.video.footprints));
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_chromaticaberration" => {
                let strength = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected a non-negative number"))?;
                self.set_chromatic_aberration_strength(strength);
            }
            "r_fogmode" => {
                self.video.fog_mode = match value.trim().to_ascii_lowercase().as_str() {
                    "off" | "0" => FogMode::Off,
                    "legacy" | "1" => FogMode::Legacy,
                    "volumetric" | "2" => FogMode::Volumetric,
                    _ => return Err(format!("{name}: expected off|legacy|volumetric")),
                };
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_sunyaw" => {
                let yaw = value.trim().parse::<f32>()
                    .map_err(|_| format!("{name}: expected degrees"))?;
                if !yaw.is_finite() { return Err(format!("{name}: expected finite degrees")); }
                self.set_sun_yaw(yaw);
            }
            "r_sunpitch" => {
                let pitch = value.trim().parse::<f32>()
                    .map_err(|_| format!("{name}: expected degrees"))?;
                if !pitch.is_finite() { return Err(format!("{name}: expected finite degrees")); }
                self.set_sun_pitch(pitch);
            }
            "r_sunintensity" => {
                let intensity = value.trim().parse::<f32>()
                    .map_err(|_| format!("{name}: expected a non-negative number"))?;
                if !intensity.is_finite() || intensity < 0.0 {
                    return Err(format!("{name}: expected a non-negative finite number"));
                }
                self.set_sun_intensity(intensity);
            }
            "r_suncolor" => {
                let values = value
                    .split([',', ' '])
                    .filter(|part| !part.is_empty())
                    .map(str::parse::<f32>)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| format!("{name}: expected R G B in the 0..1 range"))?;
                if values.len() != 3 || values.iter().any(|v| !v.is_finite()) {
                    return Err(format!("{name}: expected R G B in the 0..1 range"));
                }
                self.set_sun_color([
                    values[0].clamp(0.0, 1.0),
                    values[1].clamp(0.0, 1.0),
                    values[2].clamp(0.0, 1.0),
                ]);
            }
            "r_fogstrength" => {
                let strength = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected a number"))?;
                self.set_fog_strength(strength);
            }
            "r_distancecullscale" => {
                let scale = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected a number"))?;
                self.set_distance_cull_scale(scale);
            }
            "r_filmgrain" => {
                let strength = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected 0..1"))?;
                self.set_film_grain_strength(strength);
            }
            "r_motionblur" => {
                let strength = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected 0..1"))?;
                self.set_motion_blur_strength(strength);
            }
            "r_depthoffield" => {
                let strength = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected 0..1"))?;
                self.set_depth_of_field_strength(strength);
            }
            "r_dofquality" => {
                self.video.dof_quality = DofQuality::from_config(value)
                    .ok_or_else(|| format!("{name}: expected performance|adaptive|high"))?;
                self.sync_post_effects();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_colorlut" => {
                let preset = ColorLutPreset::from_config(value)
                    .ok_or_else(|| format!("{name}: unknown LUT preset"))?;
                self.set_color_lut(preset);
            }
            "r_colorlutstrength" => {
                let strength = value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("{name}: expected 0..1"))?;
                self.set_color_lut_strength(strength);
            }
            "r_gpudriven" | "r_hizocclusion" => {
                let enabled = boolean()?;
                if lower == "r_gpudriven" {
                    self.video.gpu_driven = enabled;
                } else {
                    self.video.hiz_occlusion = enabled;
                }
                self.sync_gpu_visibility();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_entityambientlighting" => {
                self.video.entity_ambient_lighting = EntityAmbientLightingMode::from_config(value)
                    .ok_or_else(|| format!("{name}: expected off|bsp_lightgrid|bevy_irradiance_volume"))?;
                self.sync_entity_ambient_lighting();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_dynamiclights" => {
                self.video.dynamic_lights = DynamicLightsMode::from_config(value)
                    .ok_or_else(|| format!("{name}: expected off|per_vertex|forward_plus|ray_traced"))?;
                self.video.clustered_lighting =
                    matches!(self.video.dynamic_lights, DynamicLightsMode::PerPixelForwardPlus);
                self.sync_clustered_lighting();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_modernsabers" => {
                self.video.modern_sabers = boolean()?;
                self.mark_config_dirty();
                self.publish_ui();
                self.console_status = format!(
                    "MODERN SABER RENDERING: {}",
                    if self.video.modern_sabers { "ON" } else { "OFF (OPENJK)" }
                );
            }
            "r_dynamicshadows" => {
                self.video.dynamic_shadows = DynamicShadowsMode::from_config(value)
                    .ok_or_else(|| format!("{name}: expected off|blob_stencil|csm|csm_bevy|ray_traced"))?;
                self.video.cascaded_shadows = matches!(
                    self.video.dynamic_shadows,
                    DynamicShadowsMode::CascadedShadowMaps | DynamicShadowsMode::CascadedShadowMapsBevy
                );
                self.sync_cascaded_shadows();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_clusteredlighting" => {
                self.video.clustered_lighting = boolean()?;
                self.video.dynamic_lights = if self.video.clustered_lighting {
                    DynamicLightsMode::PerPixelForwardPlus
                } else {
                    DynamicLightsMode::Off
                };
                self.sync_clustered_lighting();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_emissivearealights" => {
                self.video.emissive_area_lights = boolean()?;
                self.sync_emissive_area_lights();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_voxelprobegi" => {
                self.video.voxel_probe_gi = boolean()?;
                self.sync_voxel_probe_gi();
                self.mark_config_dirty();
                if self.map_prepare_restart_required() {
                    self.console_status =
                        "VOXEL / PROBE GI: PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART".into();
                }
                self.publish_ui();
            }
            "r_locallightshadows" => {
                self.video.local_light_shadows = boolean()?;
                self.sync_local_light_shadows();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_pbr" => {
                self.video.pbr = boolean()?;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "fs_allowassetoverrides" => {
                self.video.allow_asset_overrides = boolean()?;
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_gennormalmaps" => {
                self.video.gen_normal_maps = boolean()?;
                self.mark_config_dirty();
                if self.map_prepare_restart_required() {
                    self.console_status =
                        "GENERATED NORMAL MAPS: PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART".into();
                }
                self.publish_ui();
            }
            "r_floatlightmap" => {
                self.video.float_lightmap = boolean()?;
                self.mark_config_dirty();
                if self.map_prepare_restart_required() {
                    self.console_status =
                        "FLOAT LIGHTMAPS: PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART".into();
                }
                self.publish_ui();
            }
            "r_deluxemapping" => {
                self.video.deluxe_mapping = boolean()?;
                self.sync_pbr();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_deluxespecular" => {
                let scale = value.parse::<f32>().map_err(|_| format!("{name}: expected 0..1"))?;
                if !scale.is_finite() {
                    return Err(format!("{name}: expected 0..1"));
                }
                self.video.deluxe_specular = scale.clamp(0.0, 1.0);
                self.sync_pbr();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_cascadedshadows" => {
                self.video.cascaded_shadows = boolean()?;
                self.video.dynamic_shadows = if self.video.cascaded_shadows {
                    DynamicShadowsMode::CascadedShadowMaps
                } else {
                    DynamicShadowsMode::Off
                };
                self.sync_cascaded_shadows();
                self.mark_config_dirty();
                self.publish_ui();
            }
            "r_planardebug" => {
                self.video.planar_reflection_debug = PlanarReflectionDebugMode::from_config(value)
                    .ok_or_else(|| {
                        format!("{name}: expected off|candidates|selected|texture|applied|binding")
                    })?;
                self.sync_planar_reflection_debug();
                self.publish_ui();
            }
            _ => return Err(format!("{name}: cvar is registered but has no setter yet")),
        }
        Ok(())
    }

    fn format_console_entry(&self, entry: &crate::console::Entry) -> String {
        match entry.kind {
            crate::console::EntryKind::Command => {
                format!("^3{}^7 [command] - {}", entry.name, entry.description)
            }
            crate::console::EntryKind::ServerCommand => {
                format!("^3{}^7 [server] - {}", entry.name, entry.description)
            }
            crate::console::EntryKind::Cvar => {
                let value = self
                    .console_cvar_value(entry.name)
                    .unwrap_or_else(|| "?".into());
                let latched = self
                    .latched_console_cvars
                    .get(&entry.name.to_ascii_lowercase())
                    .map(|value| format!("  latched: \"{}\"", value))
                    .unwrap_or_default();
                if entry.range.is_empty() {
                    format!(
                        "^3{}^7 = \"{}\"{}  default: \"{}\"  - {}",
                        entry.name, value, latched, entry.default, entry.description
                    )
                } else {
                    format!(
                        "^3{}^7 = \"{}\"{}  default: \"{}\"  range: {}  - {}",
                        entry.name, value, latched, entry.default, entry.range, entry.description
                    )
                }
            }
        }
    }

    fn console_first_token_span(input: &str) -> Option<(usize, usize)> {
        let start = input.find(|ch: char| !ch.is_whitespace())?;
        let end = input[start..]
            .find(char::is_whitespace)
            .map_or(input.len(), |offset| start + offset);
        Some((start, end))
    }

    fn replace_console_first_token(&mut self, replacement: &str, append_space: bool) {
        let Some((start, end)) = Self::console_first_token_span(&self.console_input) else {
            return;
        };
        self.console_input.replace_range(start..end, replacement);
        if append_space && self.console_input.len() == start + replacement.len() {
            self.console_input.push(' ');
        }
        self.console_cursor = self.console_input.len();
        self.console_history_index = None;
        self.console_scroll = 0;
    }

    fn complete_console_input(&mut self) {
        let Some((start, end)) = Self::console_first_token_span(&self.console_input) else {
            self.push_console_line("^7Type a command/cvar prefix, then press Tab to complete.");
            self.publish_ui();
            return;
        };
        let prefix = self.console_input[start..end].to_owned();
        if prefix.is_empty() {
            return;
        }

        // An exact registered name always wins, even if it is itself a prefix of
        // another command. Otherwise a single prefix match is unambiguous.
        let connected = self.live_connected();
        if let Some(entry) = crate::console::find_command(&prefix, connected)
            .or_else(|| crate::console::unique_prefix(&prefix, connected))
        {
            self.replace_console_first_token(entry.name, true);
            self.publish_ui();
            return;
        }

        let mut matches: Vec<_> = crate::console::prefix(&prefix, connected).copied().collect();
        if matches.is_empty() {
            self.push_console_line(format!("^1No commands or cvars start with '{prefix}'."));
            self.publish_ui();
            return;
        }

        if let Some(common) = crate::console::common_prefix(&prefix, connected) {
            if common.len() > prefix.len() || common != prefix {
                self.replace_console_first_token(&common, false);
            }
        }

        matches.sort_by_key(|entry| entry.name.to_ascii_lowercase());
        self.push_console_line(format!("^5{} match(es) for '{}':^7", matches.len(), prefix));
        for entry in matches {
            let line = self.format_console_entry(&entry);
            self.push_console_line(line);
        }
        self.publish_ui();
    }

    fn complete_unique_console_command(&mut self) {
        let Some((start, end)) = Self::console_first_token_span(&self.console_input) else {
            return;
        };
        let prefix = self.console_input[start..end].to_owned();
        let connected = self.live_connected();
        if crate::console::find_command(&prefix, connected).is_some() {
            return;
        }
        if let Some(entry) = crate::console::unique_prefix(&prefix, connected) {
            self.replace_console_first_token(entry.name, false);
        }
    }

    fn browse_console_history(&mut self, direction: i32) {
        if self.console_history.is_empty() {
            return;
        }
        let len = self.console_history.len();
        let next = match (self.console_history_index, direction.signum()) {
            (None, -1) => Some(len - 1),
            (Some(index), -1) => Some(index.saturating_sub(1)),
            (Some(index), 1) if index + 1 < len => Some(index + 1),
            (Some(_), 1) => None,
            (index, _) => index,
        };
        self.console_history_index = next;
        self.console_input = next
            .map(|index| self.console_history[index].clone())
            .unwrap_or_default();
        self.console_cursor = self.console_input.len();
        self.publish_ui();
    }

    fn request_quit(&mut self) {
        if self.demo_recording.is_some() {
            self.stop_demo_recording();
        }
        // Hide the native window before the render thread begins shutting down.
        // Otherwise Windows can expose one more renderer frame during teardown,
        // which may briefly show the startup/loading splash again.
        self.quit_requested = true;
        if let Some(window) = &self.window {
            window.set_visible(false);
        }
    }

    fn disconnect_to_main_menu(&mut self) {
        if self.demo_recording.is_some() {
            self.stop_demo_recording();
        }
        // Invalidate any map preparation still in flight. Map worker results carry
        // the request id, so a late completion from the disconnected session will
        // be ignored instead of repopulating the renderer behind the front end.
        self.latest_request_id = self.latest_request_id.wrapping_add(1);
        self.pending_frontend_map_launch = false;
        self.frontend_background_request_id = None;
        self.frontend_cinematic = None;
        self.preserve_game_state_on_next_map_upload = false;
        self.loading = None;
        self.static_ao_progress = None;
        self.prepared_map_cache = None;
        self.game_session = None;
        self.live_without_world = false;
        self.live_cgame_prep_rx = None;
        self.live_cgame_prepared = None;
        self.pending_live_gamestate = false;
        self.pending_live_server_commands.clear();
        self.live_join_timing = None;
        self.restore_startup_game();
        if let Some(mut net) = self.net.take() {
            net.disconnect();
        }
        self.predictor.reset();
        self.live_input = crate::net::LiveInput::default();
        self.scoreboard = None;
        self.scores_showing = false;
        self.last_scores_request = None;
        self.center_print = None;
        self.last_center_print.clear();

        self.local_player = None;
        self.map_collision = None;
        self.map_physics_collision = PhysicsMapMesh::default();
        self.map_movement = None;
        self.third_person_camera.reset();
        self.solo_dynamic_models = Arc::new(Vec::new());
        self.spawns.clear();
        self.spawn_index = 0;
        self.map_name = "MAIN MENU".into();
        self.triangles = 0;
        self.map_distance_cull = crate::camera::DEFAULT_DISTANCE_CULL;
        self.map_authored_sun = None;
        self.map_authored_oceans.clear();
        self.authored_oceans.clear();
        self.authored_ocean_preview = false;
        self.render_command(RenderCommand::SetAuthoredOceans(Vec::new()));
        self.surface_inspector = None;
        self.keys.clear();
        self.movement_keys.clear();
        self.mouse_buttons_down.clear();
        self.noclip_primary_down = false;
        self.noclip_alt_down = false;
        self.mouse_delta = (0.0, 0.0);
        self.pending_mouse_input = None;

        self.front_end = true;
        self.frontend_page = FrontendPage::Main;
        self.menu_selected = 0;
        self.controls_waiting_for_key = false;
        self.render_command(RenderCommand::UnloadMap);
        self.sync_render_fps_cap();
        self.set_overlay(OverlayMode::Game);
        self.console_status = "DISCONNECTED - MAIN MENU".into();
        self.push_console_line("^2DISCONNECTED - RETURNED TO MAIN MENU".to_owned());
        self.publish_snapshot();
        self.publish_ui();
        self.request_frontend_background();
    }

    fn reset_asset_catalogs_for_game_change(&mut self) {
        self.prepared_map_cache = None;
        self.stringed = None;
        self.solo_catalog_loaded = false;
        self.solo_catalog_error = None;
        self.solo_maps.clear();
        self.solo_levelshot_texture = None;
        self.solo_levelshot_texture_map = None;
        self.demo_catalog_loaded = false;
        self.demo_catalog_error = None;
        self.demo_entries.clear();
    }

    /// Resolve a game-relative write without letting the feature decide whether
    /// it belongs to base or the current server/demo fs_game.
    fn game_write_path(&self, relative: impl AsRef<Path>) -> PathBuf {
        jka_assets::pk3::active_game_directory(&self.base, self.game.as_deref())
            .join(relative)
    }

    fn start_demo_recording(&mut self, requested_name: Option<&str>) {
        use jka_protocol::session::ConnectionState;

        if self.demo_recording.is_some() {
            self.push_console_line("^3Already recording.".to_owned());
            return;
        }
        let active = self
            .net
            .as_ref()
            .is_some_and(|net| net.session().state() == ConnectionState::Active);
        if !active {
            self.push_console_line("^3You must be in a level to record.".to_owned());
            return;
        }

        let mut stem = requested_name
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(|name| name.trim_matches('"').replace('\\', "/"))
            .unwrap_or_else(|| format!("demo{}", demo_recording_timestamp()));
        if let Some(stripped) = stem.strip_prefix("demos/") {
            stem = stripped.to_owned();
        }
        if let Some(stripped) = stem.strip_suffix(".dm_26") {
            stem = stripped.to_owned();
        }
        let relative = PathBuf::from(&stem);
        if stem.is_empty()
            || relative.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            self.push_console_line("^1record name must stay inside demos/".to_owned());
            return;
        }

        let qpath = PathBuf::from("demos").join(format!("{stem}.dm_26"));
        let destination = self.game_write_path(&qpath);
        if let Some(parent) = destination.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                self.push_console_line(format!("^1Couldn't create {}: {error}", parent.display()));
                return;
            }
        }

        let initial = self.net.as_ref().map(|net| {
            let session = net.session();
            demo::synthesize_gamestate_payload(session.decoder(), session.reliable_sequence())
                .map(|payload| (session.server_message_sequence().wrapping_sub(1), payload))
        });
        let (sequence, payload) = match initial {
            Some(Ok(initial)) => initial,
            Some(Err(error)) => {
                self.push_console_line(format!("^1Could not build demo gamestate: {error}"));
                return;
            }
            None => {
                self.push_console_line("^3You must be in a level to record.".to_owned());
                return;
            }
        };

        let mut file = match File::create(&destination) {
            Ok(file) => file,
            Err(error) => {
                self.push_console_line(format!("^1ERROR: couldn't open {}: {error}", destination.display()));
                return;
            }
        };
        if let Err(error) = demo::write_record(&mut file, sequence, &payload) {
            self.push_console_line(format!("^1ERROR: couldn't write {}: {error}", destination.display()));
            return;
        }

        self.demo_recording = Some(DemoRecording {
            file,
            path: destination.clone(),
            waiting_for_full_snapshot: true,
        });
        if let Some(net) = self.net.as_mut() {
            net.session_mut().set_demo_capture(true);
        }
        self.push_console_line(format!("^2recording to {}.", destination.display()));
    }

    fn record_demo_message(&mut self, sequence: i32, payload: Vec<u8>, full_snapshot: bool) {
        let Some(recording) = self.demo_recording.as_mut() else {
            return;
        };
        if recording.waiting_for_full_snapshot {
            if !full_snapshot {
                return;
            }
            recording.waiting_for_full_snapshot = false;
        }
        if let Err(error) = demo::write_record(&mut recording.file, sequence, &payload) {
            let path = recording.path.clone();
            self.demo_recording = None;
            if let Some(net) = self.net.as_mut() {
                net.session_mut().set_demo_capture(false);
            }
            self.push_console_line(format!("^1Demo recording failed for {}: {error}", path.display()));
        }
    }

    fn stop_demo_recording(&mut self) {
        let Some(mut recording) = self.demo_recording.take() else {
            self.push_console_line("^3Not recording a demo.".to_owned());
            return;
        };
        if let Some(net) = self.net.as_mut() {
            net.session_mut().set_demo_capture(false);
        }
        let result = demo::write_end(&mut recording.file).and_then(|()| recording.file.flush());
        match result {
            Ok(()) => self.push_console_line(format!("^2Stopped demo: {}", recording.path.display())),
            Err(error) => self.push_console_line(format!(
                "^1Error finishing demo {}: {error}",
                recording.path.display()
            )),
        }
    }

    fn request_screenshot(&mut self) {
        self.render_command(RenderCommand::Screenshot {
            directory: self.game_write_path("screenshots"),
        });
    }

    fn set_session_fs_game(&mut self, value: &[u8], source: &str) -> Result<bool, String> {
        let game = jka_assets::pk3::resolve_fs_game_directory(&self.base, value)?;
        if self.game == game {
            return Ok(false);
        }
        let missing_game = game
            .as_ref()
            .filter(|path| !path.is_dir())
            .map(|path| path.display().to_string());
        let previous = self
            .game
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "base".to_owned());
        let next = game
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "base".to_owned());
        self.game = game;
        self.latest_request_id = self.latest_request_id.wrapping_add(1);
        self.loading = None;
        self.static_ao_progress = None;
        self.reset_asset_catalogs_for_game_change();
        self.map_collision = None;
        self.map_movement = None;
        self.spawns.clear();
        self.live_without_world = false;
        self.map_name = "MAIN MENU".into();
        self.render_command(RenderCommand::UnloadMap);
        println!("FS_GAME: {source}: {previous} -> {next}");
        self.push_console_line(format!("^5FS_GAME:^7 {next} ^8({source})"));
        if let Some(path) = missing_game {
            self.push_console_line(format!(
                "^3FS_GAME:^7 mod directory is not installed locally: {path}; base fallback remains available"
            ));
        }
        Ok(true)
    }

    fn restore_startup_game(&mut self) {
        if self.game == self.startup_game {
            return;
        }
        self.game = self.startup_game.clone();
        self.reset_asset_catalogs_for_game_change();
        let name = self
            .game
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "base".to_owned());
        println!("FS_GAME: restored startup game {name}");
    }

    fn build_cgame_cpu_assets(
        base: &Path,
        game: Option<&Path>,
        pbr: bool,
        allow_asset_overrides: bool,
    ) -> Result<(
        Vec<jka_assets::siege::SiegeClassVisual>,
        PlayerPresenter,
        EntityPresenter,
        jka_assets::pk3::AssetSearchPath,
    ), String> {
        let open = || {
            jka_assets::pk3::AssetSearchPath::open_game(base, game).map(|mut assets| {
                assets.set_allow_asset_overrides(allow_asset_overrides);
                assets
            })
        };
        let mut assets = open().map_err(|error| format!("ASSET PATH ERROR: {error}"))?;
        let siege_classes = jka_assets::siege::load_siege_class_visuals(&mut assets)
            .map_err(|error| format!("SIEGE CLASS LOAD ERROR: {error}"))?;
        println!("CGAME: loaded {} Siege class definition(s)", siege_classes.len());
        let player_presenter = PlayerPresenter::new(assets, pbr)
            .map_err(|error| format!("PLAYER ASSET ERROR: {error}"))?;
        let entity_assets = open().map_err(|error| format!("ENTITY ASSET PATH ERROR: {error}"))?;
        let entity_presenter = EntityPresenter::new(entity_assets, pbr)
            .map_err(|error| format!("ENTITY ASSET ERROR: {error}"))?;
        let fx_assets = open().map_err(|error| format!("FX ASSET PATH ERROR: {error}"))?;
        Ok((siege_classes, player_presenter, entity_presenter, fx_assets))
    }

    /// CL_InitCGame's asset side: Siege classes and the player, entity, FX
    /// and sound presenters for a new CGame (demo or live).
    fn build_game_session(&self, qpath: String, source: String, demo_bytes: Vec<u8>) -> Result<GameSession, String> {
        let (siege_classes, player_presenter, entity_presenter, fx_assets) = Self::build_cgame_cpu_assets(
            &self.base,
            self.game.as_deref(),
            self.video.pbr,
            self.video.allow_asset_overrides,
        )?;
        let mut session = GameSession::new(
            qpath,
            source,
            demo_bytes,
            siege_classes,
            player_presenter,
            entity_presenter,
            crate::cgame::weapon_fx::WeaponFx::new(fx_assets),
        );
        match jka_assets::pk3::AssetSearchPath::open_game(&self.base, self.game.as_deref()) {
            Ok(assets) => {
                let mut sound = crate::cgame::sound_presenter::SoundPresenter::new(
                    assets,
                    self.audio.steam_audio,
                );
                let (effects, voice, separation) = self.effective_audio_mix();
                sound.set_mix(effects, voice, separation);
                sound.set_steam_audio_map(
                    self.steam_audio_acoustic_mesh.clone(),
                    self.steam_audio_bake.clone(),
                );
                session.sound_presenter = Some(sound);
            }
            Err(error) => eprintln!("AUDIO ASSET PATH UNAVAILABLE: {error}"),
        }
        if let Err(error) = session
            .player_presenter
            .set_physics_map_mesh(&self.map_physics_collision)
        {
            eprintln!("RAPIER MAP COLLISION ERROR: {error}");
        }
        Ok(session)
    }

    fn attach_live_sound_presenter(&self, session: &mut GameSession) {
        match jka_assets::pk3::AssetSearchPath::open_game(&self.base, self.game.as_deref()) {
            Ok(assets) => {
                let mut sound = crate::cgame::sound_presenter::SoundPresenter::new(
                    assets,
                    self.audio.steam_audio,
                );
                let (effects, voice, separation) = self.effective_audio_mix();
                sound.set_mix(effects, voice, separation);
                sound.set_steam_audio_map(
                    self.steam_audio_acoustic_mesh.clone(),
                    self.steam_audio_bake.clone(),
                );
                session.sound_presenter = Some(sound);
            }
            Err(error) => eprintln!("AUDIO ASSET PATH UNAVAILABLE: {error}"),
        }
    }

    fn play_selected_demo(&mut self) {
        // Demo presentation assets are opened before the demo's map request, so
        // apply map-scoped CVAR_LATCH values at this boundary too.
        self.apply_latched_console_cvars(ApplyLatchedScope::MapLoad);
        self.sync_pbr();
        if self.demo_entries.is_empty() {
            self.console_status = "NO DEMO SELECTED".into();
            return;
        }
        self.demo_selected = self.demo_selected.min(self.demo_entries.len() - 1);
        let demo_name = self.demo_entries[self.demo_selected].demo_name.clone();
        self.play_demo_named(&demo_name);
    }

    fn play_demo_named(&mut self, argument: &str) {
        let argument = argument.trim().trim_matches('"');
        if argument.is_empty() {
            self.push_console_line("^3demo <demoname>".to_owned());
            return;
        }
        let mut name = argument.replace('\\', "/");
        if let Some(stripped) = name.strip_prefix("demos/") {
            name = stripped.to_owned();
        }
        let demo_name = name
            .strip_suffix(".dm_26")
            .unwrap_or(&name)
            .to_owned();
        if demo_name.is_empty()
            || Path::new(&demo_name).components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            self.push_console_line("^1demo name must stay inside demos/".to_owned());
            return;
        }
        let qpath = format!("demos/{demo_name}.dm_26");
        const MAX_DEMO_FILE_BYTES: usize = 512 * 1024 * 1024;

        // Open through the *current* VFS before disconnecting. This preserves
        // mod-local demo lookup when /demo is issued while connected to that mod.
        println!("DEMO: {qpath}");
        let mut assets = match jka_assets::pk3::AssetSearchPath::open_game(&self.base, self.game.as_deref()) {
            Ok(assets) => assets,
            Err(error) => {
                self.console_status = format!("DEMO ASSET PATH ERROR: {error}");
                self.push_console_line(format!("^1{}", self.console_status));
                return;
            }
        };
        let asset = match assets.read(&qpath, MAX_DEMO_FILE_BYTES) {
            Ok(Some(asset)) => asset,
            Ok(None) => {
                self.console_status = format!("DEMO NOT FOUND: {qpath}");
                self.push_console_line(format!("^1{}", self.console_status));
                return;
            }
            Err(error) => {
                self.console_status = format!("DEMO READ ERROR: {error}");
                self.push_console_line(format!("^1{}", self.console_status));
                return;
            }
        };
        let source = asset.source.display().to_string();
        println!("DEMO SOURCE: {source} ({} bytes)", asset.bytes.len());
        let fs_game = match probe_demo_fs_game(&asset.bytes) {
            Ok(fs_game) => fs_game,
            Err(error) => {
                self.console_status = error;
                self.push_console_line(format!("^1{}", self.console_status));
                return;
            }
        };
        // CL_PlayDemo_f disconnects the current session before installing demo
        // state. Do it after the read above so an active mod's demos remain visible.
        if self.net.is_some() || self.game_session.is_some() || self.local_player.is_some() || !self.front_end {
            self.disconnect_to_main_menu();
        }
        if let Err(error) = self.set_session_fs_game(&fs_game, "demo gamestate") {
            self.console_status = format!("DEMO FS_GAME ERROR: {error}");
            self.push_console_line(format!("^1{}", self.console_status));
            return;
        }
        let mut playback = match self.build_game_session(qpath.clone(), source, asset.bytes) {
            Ok(playback) => playback,
            Err(error) => {
                self.console_status = error;
                self.push_console_line(format!("^1{}", self.console_status));
                return;
            }
        };
        let map_name = match playback.read_until_map() {
            Ok(map_name) => map_name,
            Err(error) => {
                self.console_status = error;
                self.push_console_line(format!("^1{}", self.console_status));
                eprintln!("{}", self.console_status);
                return;
            }
        };

        self.console_status = format!("DEMO GAMESTATE READY: {qpath} -> {map_name}");
        self.push_console_line(format!("^2{}", self.console_status));
        println!("{}", self.console_status);
        self.game_session = Some(playback);
        self.request_map(scene::MapSource::Bsp(map_name));
    }

    fn start_demo_after_map_load(&mut self, loaded_map: &str) {
        if self.game_session.as_ref().is_some_and(|session| session.live) {
            self.prime_live_session(loaded_map);
            return;
        }
        let should_start = self.game_session.as_ref().is_some_and(|playback| {
            let loaded_qpath = loaded_map.strip_suffix(".bsp").unwrap_or(loaded_map);
            playback.phase == SessionPhase::WaitingForMap
                && playback
                    .map_name
                    .as_deref()
                    .is_some_and(|map| map.eq_ignore_ascii_case(loaded_qpath))
        });
        if !should_start {
            return;
        }

        let now = Instant::now();
        let result = {
            let playback = self.game_session.as_mut().expect("checked above");
            match playback.begin_after_map_load(now) {
                Ok(sample) => Ok((sample, playback.qpath.clone(), playback.source.clone())),
                Err(error) => Err(error),
            }
        };
        match result {
            Ok((sample, qpath, source)) => {
                // Demo playback is authoritative. Do not run the local movement
                // simulator on top of the recorded playerstate.
                self.local_player = None;
                self.apply_demo_camera(sample);
                self.previous_tick = now;
                self.console_status = format!(
                    "DEMO PLAYING: {qpath} serverTime={} source={source}",
                    sample.server_time
                );
                self.push_console_line(format!("^2{}", self.console_status));
                println!("{}", self.console_status);
            }
            Err(error) => {
                self.console_status = error;
                self.push_console_line(format!("^1{}", self.console_status));
                eprintln!("{}", self.console_status);
                self.game_session = None;
            }
        }
    }

    fn apply_demo_camera(&mut self, sample: DemoCameraSample) {
        if rendering_third_person(
            self.third_person,
            self.first_person_lightsaber,
            sample.policy,
        ) {
            let mut third_person = self.third_person;
            if sample.policy.vehicle_num != 0 {
                // OpenJK bypasses ordinary target/location damping while riding
                // a vehicle, then applies any cameraOverride authored by the
                // resolved ext_data/vehicles/*.veh definition.
                third_person.target_damp = 1.0;
                third_person.camera_damp = 1.0;
                if let Some(vehicle) = self
                    .game_session
                    .as_ref()
                    .and_then(|session| session.ridden_vehicle_definition(sample.policy.vehicle_num))
                {
                    let camera = vehicle.camera;
                    if camera.override_enabled {
                        third_person.range = camera.range;
                        third_person.horz_offset = camera.horz_offset;
                        if camera.pitch_dependent_vert_offset {
                            // Match OpenJK's AT-ST-style paired vertical +
                            // pitch hacks when this legacy vehicle flag is set.
                            let pitch = sample.view_angles[0];
                            third_person.pitch_offset = pitch * -0.75;
                            third_person.vert_offset = if pitch > 0.0 {
                                (130.0 - pitch * 10.0).max(-170.0)
                            } else if pitch < 0.0 {
                                (130.0 - pitch * 5.0).min(130.0)
                            } else {
                                30.0
                            };
                        } else {
                            third_person.pitch_offset = camera.pitch_offset;
                            third_person.vert_offset = camera.vert_offset;
                        }
                    } else if vehicle
                        .vehicle_type
                        .as_deref()
                        .is_some_and(|kind| kind.eq_ignore_ascii_case("VH_ANIMAL"))
                    {
                        // OpenJK puts animal riders at zero vertical offset
                        // when the vehicle does not author a cameraOverride.
                        third_person.vert_offset = 0.0;
                    }
                }
            }
            if let Some(world) = self.map_collision.as_mut() {
                let view = offset_third_person_view(
                    world,
                    third_person,
                    &mut self.third_person_camera,
                    ThirdPersonViewInput {
                        origin: sample.native_origin,
                        view_angles: sample.view_angles,
                        view_height: sample.view_height,
                        health: sample.policy.health,
                        dead_yaw: sample.dead_yaw,
                        client_num: sample.client_num,
                        time: sample.server_time,
                        teleported: sample.teleported,
                    },
                );
                self.camera.position = glam::Vec3::from_array(scene::render_position(view.origin));
                self.camera.yaw = view.angles[1].to_radians();
                self.camera.pitch = -view.angles[0].to_radians();
                return;
            }
        }
        self.camera.position = glam::Vec3::from_array(sample.eye_position);
        self.camera.yaw = sample.view_angles[1].to_radians();
        self.camera.pitch = -sample.view_angles[0].to_radians();
    }

    fn tick_demo_playback(&mut self, now: Instant) -> Result<DemoAdvance, String> {
        let client_frame_started = Instant::now();
        // Player presentation happens inside GameSession::advance, before the
        // final camera is applied for this frame. Pass the current rendered view
        // shape/pose as a hint: advance() replaces its pose with the exact current
        // sample in first person, while third person retains this previous-frame
        // collision/orbit camera because that state lives here in App.
        let ghoul2_view = self.window.as_ref().map(|window| {
            let size = window.inner_size();
            let aspect = size.width.max(1) as f32 / size.height.max(1) as f32;
            Ghoul2PresentationView::new(
                self.camera.position.to_array(),
                self.camera.forward().to_array(),
                self.camera.fov_y_for_viewport(size.width, size.height),
                aspect,
            )
        });
        let (advance, sample, debug_lines) = {
            let playback = self
                .game_session
                .as_mut()
                .ok_or_else(|| "DEMO PLAYBACK STATE DISAPPEARED".to_owned())?;
            playback.weapon_fx.set_modern_sabers(self.video.modern_sabers);
            playback.player_presenter.set_skinning_mode(self.video.ghoul2_skinning);
            playback
                .player_presenter
                .set_early_frustum_cull(self.video.ghoul2_early_cull);
            playback
                .player_presenter
                .set_lod_bias(self.video.ghoul2_lod_bias);
            playback.player_presenter.set_ragdoll_config(RagdollConfig {
                enabled: self.video.client_physics && self.video.ragdolls,
                hz: self.video.client_physics_hz,
                max_substeps: self.video.client_physics_max_substeps,
                ccd: self.video.client_physics_ccd,
                sleeping: self.video.client_physics_sleeping,
                max_ragdolls: self.video.ragdoll_max,
                lifetime_seconds: self.video.ragdoll_lifetime,
                self_collision: self.video.ragdoll_self_collision,
                debug: self.video.physics_debug_draw,
                stats: self.video.physics_stats,
            });
            let (advance, sample) = if playback.live {
                let Some(net) = self.net.as_ref() else {
                    return Err("LIVE SESSION HAS NO CONNECTION".into());
                };
                let server_time = net.session().server_time();
                let predictor = &mut self.predictor;
                let movement = self.map_movement.as_ref();
                let world = &mut self.map_collision;
                let settings = &self.network;
                let session = net.session();
                let provisional = self.live_provisional;
                let mut logged_failure = false;
                let mut predict = |snap: &ProtocolSnapshot, next: Option<&ProtocolSnapshot>, entities: &[PresentedEntity]| {
                    // OpenJK vehicle pmove is a separate path. Until that is ported,
                    // never feed a piloting playerstate through the on-foot predictor;
                    // use snapshot interpolation instead and discard stale prediction.
                    if snap.player_state.field_i32("m_iVehicleNum").unwrap_or(0) != 0 {
                        predictor.reset();
                        return Ok(None);
                    }
                    let (Some(movement), Some(world)) = (movement, world.as_mut()) else {
                        return Ok(None);
                    };
                    let input = crate::net::PredictionInput {
                        session,
                        snap,
                        next,
                        time: server_time,
                        movement,
                        world,
                        entities,
                        settings,
                        provisional,
                    };
                    let previous_committed = predictor
                        .committed_predicted()
                        .cloned()
                        .unwrap_or_else(|| snap.player_state.clone());
                    if let Err(error) = predictor.predict(input) {
                        if !logged_failure {
                            eprintln!("PREDICTION FAILED (falling back to interpolation): {error}");
                            logged_failure = true;
                        }
                        return Ok(None);
                    }
                    let error = predictor.view_error(server_time, settings.error_decay);
                    let Some(display) = predictor.predicted().cloned() else {
                        return Ok(None);
                    };
                    let committed = predictor
                        .committed_predicted()
                        .cloned()
                        .unwrap_or_else(|| display.clone());
                    Ok(Some(LivePredictionFrame {
                        display,
                        committed,
                        previous_committed,
                        error,
                    }))
                };
                playback.advance_to_with_prediction(
                    server_time,
                    self.third_person,
                    self.first_person_lightsaber,
                    self.cg_debug_events,
                    ghoul2_view,
                    Some(&mut predict),
                )?
            } else {
                playback.advance(
                    now,
                    self.third_person,
                    self.first_person_lightsaber,
                    self.cg_debug_events,
                    ghoul2_view,
                )?
            };
            (advance, sample, playback.take_event_debug_lines())
        };
        for line in debug_lines {
            self.push_console_line(line);
        }
        let mut sample = sample;
        if let Some(angles) = self.live_view_angles() {
            sample.view_angles = angles;
        }
        self.apply_demo_camera(sample);
        // Use the final camera (including third-person orbit) for spatial audio.
        if let Some(playback) = &mut self.game_session {
            // FX sprites/lines face the final render view.
            let view = crate::fx::draw::FxView::from_camera(&self.camera);
            playback.weapon_fx.set_view(view.origin, view.axis[1]);
            let fx_tessellate_started = Instant::now();
            let entity_presenter = &mut playback.entity_presenter;
            playback.fx_surfaces = Arc::new(crate::fx::draw::tessellate(
                &playback.fx_draws,
                &view,
                &mut |shader| entity_presenter.fx_material(shader),
            ));
            playback.client_perf.fx_tessellate_ms =
                fx_tessellate_started.elapsed().as_secs_f64() * 1000.0;
            let audio_started = Instant::now();
            if let Some(sound) = &mut playback.sound_presenter {
                let yaw = self.camera.yaw;
                sound.frame(crate::audio::Listener {
                    entity: sample.client_num as u16,
                    origin: scene::jka_position(self.camera.position.to_array()),
                    left: [-yaw.sin(), yaw.cos(), 0.0],
                }, &playback.presented_entities);
            }
            playback.client_perf.audio_ms += audio_started.elapsed().as_secs_f64() * 1000.0;
            playback.client_perf.total_ms = client_frame_started.elapsed().as_secs_f64() * 1000.0;
        }
        Ok(advance)
    }

    fn ensure_local_player_presenter(&mut self) -> Result<(), String> {
        if self.local_player_presenter.is_some() {
            return Ok(());
        }
        let mut assets = jka_assets::pk3::AssetSearchPath::open_game(&self.base, self.game.as_deref())
            .map_err(|error| format!("SOLO PLAYER ASSET PATH ERROR: {error}"))?;
        assets.set_allow_asset_overrides(self.video.allow_asset_overrides);
        self.local_player_presenter = Some(
            PlayerPresenter::new(assets, self.video.pbr)
                .map_err(|error| format!("SOLO PLAYER ASSET ERROR: {error}"))?,
        );
        Ok(())
    }

    fn update_solo_player_view_and_presentation(&mut self) {
        let mut first_person_camera = self.camera;
        let subframe = self.video.input_subframe;
        let Some((mode, player_view, entity_view, view_angles, presentation_origin, presentation_time)) =
            self.local_player.as_ref().map(|player| {
                if subframe {
                    player.camera_subframe(&mut first_person_camera);
                } else {
                    player.camera(&mut first_person_camera);
                }
                let player_view = player.view();
                let view_angles = if subframe {
                    player.subframe_view_angles()
                } else {
                    player_view.view_angles
                };
                (
                    player.mode,
                    player_view,
                    player.entity_view(),
                    view_angles,
                    player.presentation_origin(),
                    player.presentation_time(),
                )
            })
        else {
            self.solo_dynamic_models = Arc::new(Vec::new());
            return;
        };
        let render_third_person = mode == JoinMode::Player
            && rendering_third_person(
                self.third_person,
                self.first_person_lightsaber,
                PlayerViewPolicyState::from_entity_view(entity_view, player_view.legs_timer),
            );

        if render_third_person {
            if let Some(world) = self.map_collision.as_mut() {
                let view = offset_third_person_view(
                    world,
                    self.third_person,
                    &mut self.third_person_camera,
                    ThirdPersonViewInput {
                        origin: if self.presentation_smoothing.smooth_third_person_origin {
                            presentation_origin
                        } else {
                            entity_view.origin
                        },
                        view_angles,
                        view_height: entity_view.view_height,
                        health: entity_view.health,
                        dead_yaw: entity_view.dead_yaw as f32,
                        client_num: entity_view.client_num,
                        time: if self.presentation_smoothing.smooth_third_person_time {
                            presentation_time
                        } else {
                            player_view.command_time
                        },
                        teleported: false,
                    },
                );
                self.camera.position = glam::Vec3::from_array(scene::render_position(view.origin));
                self.camera.yaw = view.angles[1].to_radians();
                self.camera.pitch = -view.angles[0].to_radians();
            } else {
                self.camera = first_person_camera;
            }
        } else {
            self.camera = first_person_camera;
        }

        // As in OpenJK CG_Player, keep the local player animation/Ghoul2 state
        // running even in first person. Geometry submission is the only thing
        // suppressed there (RF_THIRD_PERSON in OpenJK).
        if mode == JoinMode::Player && entity_view.entity_type == ET_PLAYER {
            if let Err(error) = self.ensure_local_player_presenter() {
                if self.console_status != error {
                    self.console_status = error.clone();
                    self.push_console_line(format!("^1{error}"));
                }
                self.solo_dynamic_models = Arc::new(Vec::new());
                return;
            }
            let mut render_entity_view = entity_view;
            if self.presentation_smoothing.smooth_player_origin {
                render_entity_view.origin = presentation_origin;
            }
            if self.presentation_smoothing.subframe_player_angles {
                render_entity_view.angles = view_angles;
            }
            if let Some(entity) = presented_openjk_player(render_entity_view) {
                let player_presentation_time = if self.presentation_smoothing.smooth_player_animation {
                    presentation_time
                } else {
                    player_view.command_time
                };
                let presenter = self
                    .local_player_presenter
                    .as_mut()
                    .expect("local player presenter initialized");
                presenter.set_skinning_mode(self.video.ghoul2_skinning);
                presenter.set_early_frustum_cull(self.video.ghoul2_early_cull);
                presenter.set_lod_bias(self.video.ghoul2_lod_bias);
                let result = presenter.present_player_entity(
                        &entity,
                        &self.solo_client_info,
                        player_presentation_time,
                        local_player_alpha(self.third_person.alpha),
                        None,
                        false,
                        render_third_person,
                    );
                match result {
                    Ok(draws) => self.solo_dynamic_models = Arc::new(draws),
                    Err(error) => {
                        self.solo_dynamic_models = Arc::new(Vec::new());
                        if self.console_status != error {
                            self.console_status = error.clone();
                            self.push_console_line(format!("^1SOLO PLAYER PRESENTATION: {error}"));
                        }
                    }
                }
            } else {
                self.solo_dynamic_models = Arc::new(Vec::new());
            }
        } else {
            self.solo_dynamic_models = Arc::new(Vec::new());
        }
    }

    fn current_map_prepare_options(&self) -> scene::MapPrepareOptions {
        scene::MapPrepareOptions {
            grass: self.video.grass,
            voxel_probe_gi: self.video.voxel_probe_gi,
            ocean: self.video.ocean,
            client_physics: self.video.client_physics,
            gen_normal_maps: self.video.gen_normal_maps,
            float_lightmap: self.video.float_lightmap && self.video.hdr,
            planar_reflections: self.video.reflection_quality.planar_slot_budget() > 0,
            pbr_materials: self.video.pbr,
            allow_asset_overrides: self.video.allow_asset_overrides,
            steam_audio: self.audio.steam_audio,
        }
    }

    fn request_map(&mut self, source: scene::MapSource) {
        self.request_map_internal(source, MapLoadPurpose::Gameplay);
    }

    /// Start map preparation without committing the UI to leave the front end.
    /// Used by the speculative live-server getinfo path while the authenticated
    /// connection/gamestate handshake is still in progress.
    fn prefetch_live_map(&mut self, source: scene::MapSource) {
        self.request_map_internal(source, MapLoadPurpose::LivePrefetch);
    }

    fn request_frontend_background(&mut self) {
        self.request_map_internal(
            scene::MapSource::Bsp(FRONTEND_BACKGROUND_MAP.to_owned()),
            MapLoadPurpose::FrontendBackground,
        );
    }

    fn frontend_background_prepare_options(&self) -> scene::MapPrepareOptions {
        let mut options = self.current_map_prepare_options();
        // The menu scene is visual-only. Do not build gameplay-only collision or
        // kick an offline Steam Audio bake simply because duel3 is on screen.
        options.client_physics = false;
        options.steam_audio = false;
        options
    }

    fn active_map_prepare_options(&self) -> scene::MapPrepareOptions {
        if self.front_end && self.frontend_cinematic.is_some() {
            self.frontend_background_prepare_options()
        } else {
            self.current_map_prepare_options()
        }
    }

    fn request_map_internal(&mut self, source: scene::MapSource, purpose: MapLoadPurpose) {
        self.apply_latched_console_cvars(ApplyLatchedScope::MapLoad);
        // PBR material selection is map-load scoped; synchronize the renderer at
        // the same boundary rather than when a latched cvar is typed. Rebuild the
        // local presentation asset cache so its VFS/material policy matches too.
        self.sync_pbr();
        self.local_player_presenter = None;
        self.pending_frontend_map_launch = self.front_end && purpose == MapLoadPurpose::Gameplay;
        self.initial_source = source.clone();
        self.latest_request_id = self.latest_request_id.wrapping_add(1);
        let request_id = self.latest_request_id;
        self.frontend_background_request_id =
            (purpose == MapLoadPurpose::FrontendBackground).then_some(request_id);
        if purpose != MapLoadPurpose::FrontendBackground {
            self.frontend_cinematic = None;
        }
        let label = source.label();
        self.static_ao_progress = None;
        let prepare_options = if purpose == MapLoadPurpose::FrontendBackground {
            self.frontend_background_prepare_options()
        } else {
            self.current_map_prepare_options()
        };
        self.latest_prepare_options = prepare_options;
        let mut loading = MapLoadingState::new(request_id, label.clone());
        if !prepare_options.grass {
            loading.skip_task(crate::thread_activity::Task::MapGrass);
        }
        if !prepare_options.voxel_probe_gi {
            loading.skip_task(crate::thread_activity::Task::MapGi);
        }
        if !prepare_options.ocean {
            loading.skip_task(crate::thread_activity::Task::MapOcean);
        }
        if !prepare_options.steam_audio {
            loading.skip_task(crate::thread_activity::Task::MapAcoustics);
        }
        // Frontend scenery is speculative/non-blocking: keep the menu usable
        // while duel3 prepares, then the world simply appears behind it.
        self.loading = (purpose != MapLoadPurpose::FrontendBackground).then_some(loading);
        self.console_status = if purpose == MapLoadPurpose::FrontendBackground {
            format!("MAIN MENU - PREPARING 3D BACKGROUND {label}...")
        } else {
            format!("LOADING {label} ON MAP WORKER...")
        };
        println!("map load {label}: background preparation started");
        if self
            .loader_tx
            .send(MapLoadRequest {
                request_id,
                started: Instant::now(),
                game: self.game.clone(),
                source,
                prepare_options,
            })
            .is_err()
        {
            self.console_status = "MAP WORKER IS NOT AVAILABLE".into();
            self.loading = None;
        }
        self.publish_ui();
    }

    /// Re-upload the already prepared CPU map after a renderer/device restart.
    ///
    /// Backend changes invalidate every GPU resource, but they do not invalidate
    /// BSP parsing, collision, decoded textures, grass placement, probe data, or
    /// any of the other CPU preparation. Keeping one prepared copy avoids doing
    /// the full map-loader job again just because Vulkan/DX12 changed.
    fn reupload_cached_map(&mut self) -> bool {
        let label = self.initial_source.label();
        let prepare_options = self.active_map_prepare_options();
        let clone_started = Instant::now();
        let Some(mut map) = self.prepared_map_cache.as_ref().and_then(|cache| {
            (cache.label == label && cache.prepare_options == prepare_options)
                .then(|| Box::new(cache.map.as_ref().clone()))
        }) else {
            return false;
        };
        let clone_ms = clone_started.elapsed().as_secs_f64() * 1000.0;

        // These timings describe CPU preparation from the original load. A cached
        // renderer restart did none of that work, so keep the restart load report
        // honest and show only its fresh GPU upload/first-frame cost.
        map.load_timings = scene::MapLoadTimings::default();

        self.latest_request_id = self.latest_request_id.wrapping_add(1);
        let request_id = self.latest_request_id;
        let started = Instant::now();
        self.static_ao_progress = None;
        self.preserve_game_state_on_next_map_upload = false;
        let mut loading = MapLoadingState::new(request_id, label.clone());
        let has_ocean = prepare_options.ocean && prepared_map_has_ocean_surface(map.as_ref());
        loading.set_optional_task_presence(
            crate::thread_activity::Task::MapOcean,
            has_ocean,
            false,
        );
        loading.begin_upload();
        self.loading = Some(loading);
        self.console_status = format!(
            "RE-UPLOADING CACHED {label} TO {}...",
            self.video.renderer_backend.label()
        );
        println!(
            "[VID_RESTART] reused prepared map cache; CPU clone {:.1} ms (skipped BSP/assets/collision/shaders/textures/grass/GI/ocean preparation)",
            clone_ms
        );
        self.publish_ui();
        self.render_command(RenderCommand::LoadMap {
            request_id,
            started,
            name: label,
            map,
        });
        self.publish_snapshot();
        true
    }

    fn write_named_config(&mut self, name: &str) {
        let qpath = match normalize_cfg_qpath(name) {
            Ok(path) => path,
            Err(error) => {
                self.push_console_line(format!("^1{error}"));
                return;
            }
        };

        let file = qpath.file_name().map(|name| name.to_string_lossy());
        if file.as_deref().is_some_and(|name| {
            name.eq_ignore_ascii_case("default.cfg") || name.eq_ignore_ascii_case("mpdefault.cfg")
        }) {
            self.push_console_line(format!("^3The filename {} is reserved", qpath.display()));
            return;
        }

        // Mirror OpenJK writeconfig: bindings + archived cvars. The native
        // jka-rust.cfg already contains exactly that state, so flush it first
        // and then copy the generated snapshot into the active game directory.
        self.config_dirty = true;
        self.flush_config();
        let destination = self.game_write_path(&qpath);
        if destination == self.config_path {
            self.push_console_line(format!("^2Writing {}", destination.display()));
            return;
        }
        if let Some(parent) = destination.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                self.push_console_line(format!(
                    "^1Couldn't write {}: {error}",
                    destination.display()
                ));
                return;
            }
        }
        match std::fs::copy(&self.config_path, &destination) {
            Ok(_) => self.push_console_line(format!("^2Writing {}", destination.display())),
            Err(error) => self.push_console_line(format!(
                "^1Couldn't write {}: {error}",
                destination.display()
            )),
        }
    }

    fn console_buffer_bytes(&self) -> usize {
        self.console_command_buffer
            .iter()
            .map(|command| command.len().saturating_add(1))
            .sum()
    }

    fn insert_console_commands_front(&mut self, commands: Vec<String>) -> bool {
        let added = commands
            .iter()
            .map(|command| command.len().saturating_add(1))
            .sum::<usize>();
        if self.console_buffer_bytes().saturating_add(added) >= MAX_CONSOLE_COMMAND_BUFFER_BYTES {
            self.push_console_line("^1Cbuf_InsertText overflowed".to_owned());
            return false;
        }
        for command in commands.into_iter().rev() {
            self.console_command_buffer.push_front(command);
        }
        true
    }

    fn append_console_commands(&mut self, commands: Vec<String>) -> bool {
        let added = commands
            .iter()
            .map(|command| command.len().saturating_add(1))
            .sum::<usize>();
        if self.console_buffer_bytes().saturating_add(added) >= MAX_CONSOLE_COMMAND_BUFFER_BYTES {
            self.push_console_line("^1Cbuf_AddText: overflow".to_owned());
            return false;
        }
        self.console_command_buffer.extend(commands);
        true
    }

    /// OpenJK Cbuf_Execute semantics: commands execute in order until the
    /// buffer empties or `wait` asks us to leave the remainder for later.
    fn process_console_command_buffer(&mut self) -> bool {
        let mut executed = 0usize;
        loop {
            if self.console_wait_frames > 0 {
                self.console_wait_frames -= 1;
                break;
            }
            let Some(command) = self.console_command_buffer.pop_front() else {
                break;
            };
            self.execute_command_line(&command);
            executed += 1;
            // OpenJK has a fixed 128 KiB byte buffer but a self-reinserting
            // vstr/exec can otherwise spin forever in one host frame. Keep the
            // same ordering while yielding pathological scripts safely.
            if executed >= MAX_CONSOLE_COMMANDS_PER_FRAME {
                self.push_console_line(
                    "^3Cbuf_Execute:^7 command budget exhausted; remaining commands deferred"
                        .to_owned(),
                );
                break;
            }
        }
        executed != 0
    }

    fn exec_cfg_file(&mut self, name: &str, quiet: bool) {
        let qpath = match normalize_cfg_qpath(name) {
            Ok(path) => path,
            Err(error) => {
                self.push_console_line(format!("^1{error}"));
                return;
            }
        };
        let qpath_string = qpath.to_string_lossy().replace('\\', "/");
        const MAX_CFG_BYTES: usize = 4 * 1024 * 1024;
        let mut assets = match jka_assets::pk3::AssetSearchPath::open_game(
            &self.base,
            self.game.as_deref(),
        ) {
            Ok(assets) => assets,
            Err(error) => {
                self.push_console_line(format!("^1couldn't exec {qpath_string}: {error}"));
                return;
            }
        };
        let asset = match assets.read(&qpath_string, MAX_CFG_BYTES) {
            Ok(Some(asset)) => asset,
            Ok(None) => {
                self.push_console_line(format!("^1couldn't exec {qpath_string}"));
                return;
            }
            Err(error) => {
                self.push_console_line(format!("^1couldn't exec {qpath_string}: {error}"));
                return;
            }
        };
        let text = String::from_utf8_lossy(&asset.bytes);
        if !quiet {
            self.push_console_line(format!("^2execing {qpath_string}"));
        }
        // Cbuf_InsertText: the script runs before commands that were already
        // waiting behind the `exec` invocation.
        self.insert_console_commands_front(split_console_script(&text));
    }

    fn execute_console(&mut self) {
        // OpenJK-style convenience: Enter accepts a uniquely abbreviated first
        // command/cvar token, while ambiguous abbreviations remain untouched.
        self.complete_unique_console_command();
        let command = std::mem::take(&mut self.console_input);
        self.console_cursor = 0;
        let command = command.trim().to_owned();
        if command.is_empty() {
            return;
        }

        self.push_console_line(format!("^7] {command}"));
        if self
            .console_history
            .last()
            .map_or(true, |previous| previous != &command)
        {
            self.console_history.push(command.clone());
            if self.console_history.len() > 512 {
                self.console_history.remove(0);
            }
        }
        self.console_history_index = None;
        self.append_console_commands(split_console_script(&command));
    }

    fn execute_command_line(&mut self, command: &str) {
        self.console_status.clear();
        // Cbuf/Cmd: a leading slash or backslash is ignored.
        let command = command.trim().trim_start_matches(['/', '\\']).trim_start();
        if command.is_empty() {
            return;
        }

        if self.handle_bind_console_command(command) {
            return;
        }
        // Cvar_Set_f family: `set <name> <value ...>` is `<name> <value>`.
        let set_form = command
            .split_once(char::is_whitespace)
            .filter(|(verb, _)| ["set", "seta", "sets", "setu"].iter().any(|v| verb.eq_ignore_ascii_case(v)))
            .map(|(_, rest)| rest.trim().to_owned());
        if let Some(rest) = set_form {
            let Some((name, value)) = rest.split_once(char::is_whitespace) else {
                self.push_console_line("^3usage:^7 set <variable> <value>");
                return;
            };
            let value = value.trim().trim_matches('"');
            match self.set_console_cvar(name, value) {
                Ok(()) => {}
                Err(error) => self.push_console_line(format!("^1{error}")),
            }
            return;
        }
        if self.execute_network_command(command) {
            return;
        }

        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [verb] if verb.eq_ignore_ascii_case("hudedit") => {
                if self.front_end {
                    self.push_console_line("^3hudedit:^7 enter a game before editing the HUD".to_owned());
                } else {
                    self.hud_edit_selected = None;
                    self.hud_edit_drag_origin = None;
                    self.hud_edit_drag_delta = [0.0, 0.0];
                    self.set_overlay(OverlayMode::HudEdit);
                }
                return;
            }
            [verb, ..] if verb.eq_ignore_ascii_case("echo") => {
                let message = command
                    .split_once(char::is_whitespace)
                    .map(|(_, args)| args.trim())
                    .unwrap_or("");
                let message = message
                    .strip_prefix('"')
                    .and_then(|value| value.strip_suffix('"'))
                    .unwrap_or(message);
                self.push_console_line(message.to_owned());
                return;
            }
            [verb, name] if verb.eq_ignore_ascii_case("vstr") => {
                let Some(value) = self.console_cvar_value(name) else {
                    self.push_console_line(format!("^1Cvar {name} does not exist."));
                    return;
                };
                // Cbuf_InsertText: execute the variable contents before the
                // remainder of the current command buffer.
                self.insert_console_commands_front(split_console_script(&value));
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("vstr") => {
                self.push_console_line("^3vstr <variablename>^7 : execute a variable command".to_owned());
                return;
            }
            [verb, argument] if verb.eq_ignore_ascii_case("wait") => {
                let parsed = argument.parse::<i32>().unwrap_or(0);
                self.console_wait_frames = if parsed < 0 { 1 } else { parsed as u32 };
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("wait") => {
                self.console_wait_frames = 1;
                return;
            }
            [verb, ..] if verb.eq_ignore_ascii_case("wait") => {
                // OpenJK treats argc != 2 exactly like bare `wait`.
                self.console_wait_frames = 1;
                return;
            }
            [verb, filter @ ..] if verb.eq_ignore_ascii_case("cmdlist") => {
                let pattern = filter.first().copied();
                let mut all: Vec<_> = crate::console::commands(self.live_connected()).collect();
                all.sort_by_key(|entry| entry.name.to_ascii_lowercase());
                let total = all.len();
                let matches: Vec<_> = all
                    .into_iter()
                    .filter(|entry| pattern.is_none_or(|pattern| crate::console::filter_match(pattern, entry.name)))
                    .collect();
                for entry in &matches {
                    if entry.description.is_empty() {
                        self.push_console_line(format!(" ^7{}", entry.name));
                    } else {
                        self.push_console_line(format!(" ^7{}^2 - {}", entry.name, entry.description));
                    }
                }
                self.push_console_line(format!("^7{total} total commands"));
                if matches.len() != total {
                    self.push_console_line(format!("^7{} matching commands", matches.len()));
                }
                return;
            }
            [verb, name] if verb.eq_ignore_ascii_case("help") => {
                let entry = crate::console::commands(self.live_connected())
                    .find(|entry| entry.name.eq_ignore_ascii_case(name));
                if let Some(entry) = entry {
                    if entry.description.is_empty() {
                        self.push_console_line(format!("^8Cmd ^7{}", entry.name));
                    } else {
                        self.push_console_line(format!("^8Cmd ^7{}^2 - {}", entry.name, entry.description));
                    }
                } else {
                    self.push_console_line(format!("^1Command {name} does not exist."));
                }
                return;
            }
            [verb, ..] if verb.eq_ignore_ascii_case("help") => {
                self.push_console_line("^3usage:^7 help <command or alias>".to_owned());
                return;
            }
            [verb, demo_name] if verb.eq_ignore_ascii_case("record") => {
                self.start_demo_recording(Some(demo_name));
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("record") => {
                self.start_demo_recording(None);
                return;
            }
            [verb, ..] if verb.eq_ignore_ascii_case("record") => {
                self.push_console_line("^3record [demoname]".to_owned());
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("stoprecord") => {
                self.stop_demo_recording();
                return;
            }
            [verb, ..] if verb.eq_ignore_ascii_case("stoprecord") => {
                self.push_console_line("^3stoprecord".to_owned());
                return;
            }
            [verb, demo_name] if verb.eq_ignore_ascii_case("demo") => {
                self.play_demo_named(demo_name);
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("demo") => {
                self.push_console_line("^3demo <demoname>".to_owned());
                return;
            }
            [verb, filename]
                if verb.eq_ignore_ascii_case("exec") || verb.eq_ignore_ascii_case("execq") =>
            {
                self.exec_cfg_file(filename, verb.eq_ignore_ascii_case("execq"));
                return;
            }
            [verb]
                if verb.eq_ignore_ascii_case("exec") || verb.eq_ignore_ascii_case("execq") =>
            {
                self.push_console_line(format!(
                    "^3{} <filename>^7 : execute a script file{}",
                    verb,
                    if verb.eq_ignore_ascii_case("execq") {
                        " without notification"
                    } else {
                        ""
                    }
                ));
                return;
            }
            [verb, filename]
                if verb.eq_ignore_ascii_case("write")
                    || verb.eq_ignore_ascii_case("writeconfig") =>
            {
                self.write_named_config(filename);
                return;
            }
            [verb]
                if verb.eq_ignore_ascii_case("write")
                    || verb.eq_ignore_ascii_case("writeconfig") =>
            {
                self.push_console_line(
                    "^3usage:^7 writeconfig <filename>  (write is an alias)".to_owned(),
                );
                return;
            }
            _ => {}
        }
        if let [name] = words.as_slice() {
            if let Some(entry) = crate::console::find(name) {
                if entry.kind == crate::console::EntryKind::Cvar {
                    let line = self.format_console_entry(entry);
                    self.push_console_line(line);
                    return;
                }
            }
        }
        if words.len() >= 2 && words[1] == "!" {
            let name = words[0];
            if let Some(entry) = crate::console::find(name) {
                if entry.kind == crate::console::EntryKind::Cvar {
                    let Some(current) = self.console_cvar_value(name) else {
                        self.push_console_line(format!("^1Cvar {name} is not writable here."));
                        return;
                    };
                    // OpenJK Cvar_Command semantics: a first argument of `!`
                    // sets the numeric cvar to logical-not(current value). This
                    // is what makes binds such as `cg_thirdPerson !` toggles.
                    let numeric = current.parse::<f32>().unwrap_or(0.0);
                    let target = if numeric == 0.0 { "1" } else { "0" };
                    match self.set_console_cvar(name, target) {
                        Ok(()) => self.push_console_line(self.format_console_entry(entry)),
                        Err(error) => self.push_console_line(format!("^1{error}")),
                    }
                    return;
                }
            }
        }
        // Cvar_Command semantics: if the first token is a cvar, the entire
        // remainder is its value. This is important for string cvars such as
        // packed HUD layouts and also matches normal JKA console behavior.
        if let Some((name, raw_value)) = command.split_once(char::is_whitespace) {
            if let Some(entry) = crate::console::find(name) {
                if entry.kind == crate::console::EntryKind::Cvar {
                    let value = raw_value.trim().trim_matches('"');
                    match self.set_console_cvar_from_console(name, value) {
                        Ok(ConsoleCvarSetResult::Applied) => {
                            let line = self.format_console_entry(entry);
                            self.push_console_line(line);
                        }
                        Ok(ConsoleCvarSetResult::Latched) => self.push_console_line(format!(
                            "^7{} will be changed upon restarting.",
                            entry.name
                        )),
                        Ok(ConsoleCvarSetResult::Unchanged) => {}
                        Err(error) => self.push_console_line(format!("^1{error}")),
                    }
                    return;
                }
            }
        }

        if words
            .first()
            .is_some_and(|verb| verb.eq_ignore_ascii_case("toggle"))
        {
            if words.len() < 2 {
                self.push_console_line("^3usage:^7 toggle <variable> [value1 value2 ...]".to_owned());
                return;
            }
            let name = words[1];
            let Some(entry) = crate::console::find(name) else {
                self.push_console_line(format!("^1Cvar {name} does not exist."));
                return;
            };
            if entry.kind != crate::console::EntryKind::Cvar {
                self.push_console_line(format!("^1{name} is not a cvar."));
                return;
            }
            let Some(current) = self.console_cvar_value(name) else {
                self.push_console_line(format!("^1Cvar {name} is not writable here."));
                return;
            };
            let target = if words.len() == 2 {
                let numeric = current.parse::<f32>().unwrap_or(0.0);
                if numeric == 0.0 { "1".to_owned() } else { "0".to_owned() }
            } else if words.len() == 3 {
                self.push_console_line("^3toggle:^7 nothing to toggle to".to_owned());
                return;
            } else {
                let values = &words[2..];
                let next = values
                    .iter()
                    .take(values.len().saturating_sub(1))
                    .position(|candidate| *candidate == current.as_str())
                    .map(|index| values[index + 1])
                    .unwrap_or(values[0]);
                next.to_owned()
            };
            match self.set_console_cvar_from_console(name, &target) {
                Ok(ConsoleCvarSetResult::Applied) => {
                    self.push_console_line(self.format_console_entry(entry))
                }
                Ok(ConsoleCvarSetResult::Latched) => self.push_console_line(format!(
                    "^7{} will be changed upon restarting.",
                    entry.name
                )),
                Ok(ConsoleCvarSetResult::Unchanged) => {}
                Err(error) => self.push_console_line(format!("^1{error}")),
            }
            return;
        }

        if let Some((verb, message)) = command.split_once(char::is_whitespace) {
            let message = message.trim();
            if verb.eq_ignore_ascii_case("say") || verb.eq_ignore_ascii_case("say_team") {
                if message.is_empty() {
                    self.push_console_line(format!(
                        "^3{}^7 [command] - {}",
                        verb, "Message text is required."
                    ));
                } else {
                    self.push_chat_line(
                        if verb.eq_ignore_ascii_case("say_team") {
                            ChatMode::Team
                        } else {
                            ChatMode::Global
                        },
                        message,
                    );
                }
                self.publish_ui();
                return;
            }
        }

        match words.as_slice() {
            [verb, name]
                if verb.eq_ignore_ascii_case("devmap") || verb.eq_ignore_ascii_case("map") =>
            {
                match scene::MapSource::from_map_argument(name) {
                    Ok(source) => match scene::verify_map_source_exists(
                        &self.base,
                        self.game.as_deref(),
                        &source,
                    ) {
                        Ok(()) => {
                            // A local map/devmap launch owns the client state. Tear down any
                            // live netchan first so server snapshots/commands cannot continue
                            // arriving while the local map is being prepared.
                            if self.net.is_some() {
                                self.disconnect_to_main_menu();
                            }
                            self.request_map(source);
                        },
                        Err(error) => {
                            self.console_status = error.to_ascii_uppercase();
                        }
                    },
                    Err(error) => {
                        self.console_status = error.to_ascii_uppercase();
                    }
                }
            }
            [verb] if verb.eq_ignore_ascii_case("devmap") || verb.eq_ignore_ascii_case("map") => {
                self.console_status = "USAGE: DEVMAP MP/FFA3[.BSP|.MAP]".into();
            }
            [verb] if verb.eq_ignore_ascii_case("cg_eventstats") => {
                let lines = self
                    .game_session
                    .as_ref()
                    .map(GameSession::event_stats_lines)
                    .unwrap_or_else(|| vec!["^3CG EVENT STATS:^7 no demo is playing".into()]);
                for line in lines {
                    self.push_console_line(line);
                }
                return;
            }
            [verb, argument]
                if verb.eq_ignore_ascii_case("cg_eventstats")
                    && argument.eq_ignore_ascii_case("clear") =>
            {
                if let Some(playback) = self.game_session.as_mut() {
                    playback.clear_event_stats();
                    self.console_status = "CG EVENT STATS CLEARED".into();
                } else {
                    self.console_status = "NO DEMO IS PLAYING".into();
                }
            }
            [verb]
                if verb.eq_ignore_ascii_case("r_dumpmaterials")
                    || verb.eq_ignore_ascii_case("dumpmaterials") =>
            {
                self.render_command(RenderCommand::DumpMaterials {
                    filter: None,
                    all: false,
                });
                self.console_status =
                    "MATERIAL DEBUG DUMPED TO TERMINAL (ENHANCED MATERIALS ONLY)".into();
            }
            [verb, argument]
                if verb.eq_ignore_ascii_case("r_dumpmaterials")
                    || verb.eq_ignore_ascii_case("dumpmaterials") =>
            {
                self.render_command(RenderCommand::DumpMaterials {
                    filter: (!argument.eq_ignore_ascii_case("all")).then(|| (*argument).to_owned()),
                    all: argument.eq_ignore_ascii_case("all"),
                });
                self.console_status = if argument.eq_ignore_ascii_case("all") {
                    "ALL MATERIAL DEBUG DUMPED TO TERMINAL".into()
                } else {
                    format!("MATERIAL DEBUG FILTER '{argument}' DUMPED TO TERMINAL")
                };
            }
            [verb, argument] if verb.eq_ignore_ascii_case("r_planardebug") => {
                if let Some(mode) = PlanarReflectionDebugMode::from_config(argument) {
                    self.video.planar_reflection_debug = mode;
                    self.sync_planar_reflection_debug();
                    self.console_status = format!("PLANAR REFLECTION DEBUG: {}", mode.label());
                    self.publish_ui();
                } else {
                    self.console_status =
                        "R_PLANARDEBUG: OFF | CANDIDATES | SELECTED | TEXTURE | APPLIED".into();
                }
            }
            [verb] if verb.eq_ignore_ascii_case("+scores") => {
                let was_showing = self.scores_showing;
                self.scores_showing = true;
                if !was_showing {
                    self.request_scores_if_due(true);
                }
                self.publish_ui();
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("-scores") => {
                self.scores_showing = false;
                self.publish_ui();
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("messagemode") => {
                self.begin_chat(ChatMode::Global);
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("messagemode2") => {
                self.begin_chat(ChatMode::Team);
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("force_speed") => {
                if let Some(player) = &mut self.local_player {
                    player.request_power(jka_movement::MovementPower::Speed);
                }
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("force_rage") => {
                if let Some(player) = &mut self.local_player {
                    player.request_power(jka_movement::MovementPower::Rage);
                }
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("puddle_debug") => {
                self.puddle_debug_visualization = !self.puddle_debug_visualization;
                self.render_command(RenderCommand::SetPuddleDebug(self.puddle_debug_visualization));
                self.console_status = if self.puddle_debug_visualization {
                    "PUDDLE DEBUG ON (blue=puddle, green=wet film)".into()
                } else {
                    "PUDDLE DEBUG OFF".into()
                };
            }
            [verb] if verb.eq_ignore_ascii_case("trace") => {
                self.trace_surface_center();
            }
            [verb] if verb.eq_ignore_ascii_case("trace_clear") => {
                self.clear_surface_inspection();
            }
            [verb] if verb.eq_ignore_ascii_case("cycle_spawn") => {
                self.cycle_spawn();
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("noclip") => {
                self.toggle_noclip();
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("vid_restart") => {
                self.restart_renderer();
                return;
            }
            [verb]
                if verb.eq_ignore_ascii_case("screenshot")
                    || verb.eq_ignore_ascii_case("/screenshot") =>
            {
                self.request_screenshot();
                self.console_status = "SCREENSHOT QUEUED".into();
            }
            [verb] if verb.eq_ignore_ascii_case("say") || verb.eq_ignore_ascii_case("say_team") => {
                self.console_status = format!("USAGE: {} <MESSAGE>", verb.to_ascii_uppercase());
            }
            [verb] if verb.eq_ignore_ascii_case("disconnect") => {
                if self.net.is_some() {
                    self.push_console_line("^3Disconnected from server.");
                }
                self.disconnect_to_main_menu();
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("demo_pause") => {
                self.console_status = match self.game_session.as_mut() {
                    Some(playback) => match playback.toggle_pause(Instant::now()) {
                        Ok(0.0) => "DEMO PAUSED".into(),
                        Ok(rate) => format!("DEMO PLAYING AT {rate}X"),
                        Err(error) => error,
                    },
                    None => "NO DEMO IS PLAYING".into(),
                };
            }
            [verb] if verb.eq_ignore_ascii_case("fxinfo") => {
                match self.game_session.as_ref() {
                    Some(playback) => {
                        let stats = playback.weapon_fx.stats();
                        self.push_console_line(format!(
                            "^3FX:^7 effects {} | live primitives {} | scheduled {} | dropped {} | unsupported spawns {} | surfaces {}",
                            stats.registered,
                            stats.active,
                            stats.scheduled,
                            stats.dropped,
                            stats.unsupported_spawns,
                            playback.fx_surfaces.len(),
                        ));
                    }
                    None => self.push_console_line("^3FX:^7 no active CGame (start a demo first)."),
                }
                return;
            }
            [verb] if verb.eq_ignore_ascii_case("soundinfo") => {
                let sound_state = self
                    .game_session
                    .as_ref()
                    .and_then(|playback| playback.sound_presenter.as_ref())
                    .and_then(|sound| sound.info().map(|info| (info, sound.source_rate_summary())));
                if let Some((info, source_rates)) = sound_state {
                    self.push_console_line(format!(
                        "^3AUDIO OUTPUT:^7 {} ch | {} Hz | {} | buffer {}",
                        info.channels, info.sample_rate, info.sample_format, info.buffer_size
                    ));
                    self.push_console_line(format!(
                        "^3AUDIO MIX:^7 voices {} / 32 | loops {} ({} requests) | pre-limit peak {:.3} | overload samples {}",
                        info.active_voices, info.active_loops, info.loop_requests, info.peak_before_limiter, info.overload_samples
                    ));
                    self.push_console_line(format!(
                        "^3AUDIO SOURCES:^7 {} (each retains its decoded rate; mixer converts to {} Hz output)",
                        source_rates, info.sample_rate
                    ));
                    self.push_console_line(format!(
                        "^3AUDIO LEVELS:^7 s_volume {:.3} | s_volumeVoice {:.3} | s_musicvolume {:.3} | s_separation {:.3}",
                        self.audio.effects_volume,
                        self.audio.voice_volume,
                        self.audio.music_volume,
                        self.audio.separation
                    ));
                    self.push_console_line(format!(
                        "^3AUDIO PAN:^7 largest per-frame gain step {:.3} (glided per sample, not stepped)",
                        info.largest_gain_step
                    ));
                    if info.overload_samples > 0 {
                        self.push_console_line(
                            "^3AUDIO NOTE:^7 the game mix exceeded full scale before the output limiter; this is a likely crackle/clipping source.",
                        );
                    }
                } else {
                    self.push_console_line(
                        "^3AUDIO:^7 no active CGame audio backend (start a demo/map presentation first).",
                    );
                }
                return;
            }
            [verb, rate] if verb.eq_ignore_ascii_case("demo_speed") => {
                self.console_status = match rate.parse::<f64>() {
                    Ok(rate) => match self.game_session.as_mut() {
                        Some(playback) => match playback.set_playback_rate(Instant::now(), rate) {
                            Ok(()) if rate == 0.0 => "DEMO PAUSED".into(),
                            Ok(()) => format!("DEMO SPEED: {rate}X"),
                            Err(error) => error,
                        },
                        None => "NO DEMO IS PLAYING".into(),
                    },
                    Err(_) => "USAGE: demo_speed <0..100>".into(),
                };
            }
            [verb] if verb.eq_ignore_ascii_case("quit") || verb.eq_ignore_ascii_case("exit") => {
                self.request_quit();
            }
            _ => {
                if self.live_connected() {
                    // CL_ForwardCommandToServer: anything unrecognised locally.
                    self.forward_command_to_server(command);
                    return;
                }
                self.console_status = format!("^1UNKNOWN COMMAND:^7 {command}");
            }
        }
        if !self.console_status.is_empty() {
            self.push_console_line(self.console_status.clone());
        }
    }

    /// OpenJK CG_ScoresDown_f request cadence. While held, refresh the reliable
    /// `score` command every ~2 seconds; a newly requested stale board is cleared
    /// so an old match/player list is not presented as current.
    fn request_scores_if_due(&mut self, clear_stale: bool) {
        let active = self
            .net
            .as_ref()
            .is_some_and(|net| net.state() == jka_protocol::session::ConnectionState::Active);
        if !active {
            return;
        }
        let now = Instant::now();
        let due = self
            .last_scores_request
            .is_none_or(|last| now.saturating_duration_since(last) >= Duration::from_secs(2));
        if !due {
            return;
        }
        if clear_stale {
            self.scoreboard = None;
        }
        self.last_scores_request = Some(now);
        if let Some(net) = self.net.as_mut() {
            if let Err(error) = net.session_mut().add_reliable_command(b"score", false) {
                self.push_console_line(format!("^1score request failed: {error}"));
            }
        }
    }

    /// cls.state >= CA_CONNECTED on a live server.
    fn live_connected(&self) -> bool {
        self.net
            .as_ref()
            .is_some_and(|net| net.state() >= jka_protocol::session::ConnectionState::Connected)
    }

    /// The server marks spectator follow snapshots with PMF_FOLLOW and puts the
    /// followed player's client number in ps.clientNum. Resolve that through the
    /// same CS_PLAYERS client info used by player presentation.
    fn current_follow_name(&self) -> Option<String> {
        const PMF_FOLLOW: i32 = 4096;
        let session = self.game_session.as_ref()?;
        let ps = if session.live {
            self.predictor
                .predicted()
                .or_else(|| session.current_snapshot.as_ref().map(|snapshot| &snapshot.player_state))
        } else {
            session.current_snapshot.as_ref().map(|snapshot| &snapshot.player_state)
        }?;
        if ps.field_i32("pm_flags").unwrap_or(0) & PMF_FOLLOW == 0 {
            return None;
        }
        let client_num = usize::try_from(ps.field_i32("clientNum")?).ok()?;
        session
            .client_game
            .client_info(client_num, &session.siege_classes)
            .map(|info| info.name)
            .filter(|name| !name.is_empty())
    }

    fn current_movement_hud_state(&self) -> ui::MovementHudState {
        if let Some(ps) = self.live_player_state() {
            let mut state = ui::MovementHudState::default();
            if let Some(cmd) = self.live_provisional {
                state.forward_move = cmd.forward_move;
                state.right_move = cmd.right_move;
                state.up_move = cmd.up_move;
                state.buttons = cmd.buttons;
            }
            state.velocity = playerstate_vec3(ps, "velocity").unwrap_or([0.0; 3]);
            state.view_yaw = self
                .live_view_angles()
                .or_else(|| playerstate_vec3(ps, "viewangles"))
                .map(|angles| angles[1])
                .unwrap_or(0.0);
            state.player_speed = ps.field_i32("speed").unwrap_or(250).max(1) as f32;
            state.grounded = ps.field_i32("groundEntityNum").is_some_and(|entity| entity != 1023);
            state.fov_x = self.camera.cg_fov();
            return state;
        }

        let mut state = ui::MovementHudState::default();
        if let Some(player) = &self.local_player {
            let view = player.view();
            state.velocity = view.velocity;
            state.view_yaw = player.subframe_view_angles()[1];
            state.grounded = view.ground_entity != 1023;
        } else {
            state.view_yaw = self.camera.yaw.to_degrees();
        }
        let axis = |positive: KeyCode, negative: KeyCode| -> i8 {
            match (self.movement_keys.contains(&positive), self.movement_keys.contains(&negative)) {
                (true, false) => 127,
                (false, true) => -127,
                _ => 0,
            }
        };
        state.forward_move = axis(KeyCode::KeyW, KeyCode::KeyS);
        state.right_move = axis(KeyCode::KeyD, KeyCode::KeyA);
        state.up_move = axis(KeyCode::Space, KeyCode::ControlLeft);
        if self.noclip_primary_down { state.buttons |= jka_movement::BUTTON_ATTACK; }
        if self.noclip_alt_down { state.buttons |= jka_movement::BUTTON_ALT_ATTACK; }
        if self.movement_keys.contains(&KeyCode::ShiftLeft) { state.buttons |= jka_movement::BUTTON_WALKING; }
        state.fov_x = self.camera.cg_fov();
        state
    }

    /// Live: the predicted playerstate; solo: the offline Pmove host.
    fn current_hud_state(&self) -> Option<HudState> {
        if let Some(ps) = self.live_player_state() {
            let weapon = ps.field_i32("weapon").unwrap_or(0);
            let ammo = jka_movement::weapon_info(weapon)
                .filter(|(ammo_index, _, _)| *ammo_index > 0)
                .and_then(|(ammo_index, _, _)| ps.ammo.get(ammo_index as usize).copied());
            return Some(HudState {
                health: ps.stats[0],
                max_health: ps.stats[8].max(1),
                armor: ps.stats[5],
                force_power: ps.field_i32("fd.forcePower").unwrap_or(0),
                force_power_max: 100,
                ammo,
            });
        }
        self.local_player.as_ref().and_then(|player| {
            (player.mode == JoinMode::Player).then(|| {
                let view = player.view();
                HudState {
                    health: view.health,
                    max_health: view.max_health.max(1),
                    armor: view.armor,
                    force_power: view.force_power,
                    force_power_max: view.force_power_max.max(1),
                    ammo: (view.ammo >= 0).then_some(view.ammo),
                }
            })
        })
    }

    /// cg.predictedPlayerState (or the latest snapshot's) while live.
    fn live_player_state(&self) -> Option<&jka_protocol::server::PlayerState> {
        let session = self.game_session.as_ref().filter(|session| session.live)?;
        self.predictor
            .predicted()
            .or_else(|| session.current_snapshot.as_ref().map(|snapshot| &snapshot.player_state))
    }

    /// View angles for this render frame: the predicted playerstate's angles
    /// (PM_UpdateViewAngles for the newest usercmd) advanced by whatever mouse
    /// motion cl.viewangles has accumulated since that command was built. The
    /// camera then turns at frame rate while usercmds stay paced by
    /// cl_commandRate. Not applied when following, dead or when prediction is
    /// off, where the view belongs to the server.
    fn live_view_angles(&self) -> Option<[f32; 3]> {
        const PMF_FOLLOW: i32 = 4096;
        const PM_DEAD: i32 = 5;
        if self.network.no_predict {
            return None;
        }
        let net = self.net.as_ref()?;
        let ps = self.predictor.predicted()?;
        let pm_type = ps.field_i32("pm_type").unwrap_or(0);
        if ps.field_i32("pm_flags").unwrap_or(0) & PMF_FOLLOW != 0 || pm_type >= PM_DEAD {
            return None;
        }
        let _ = net;
        let predicted_with = self.predictor.predicted_command_angles()?;
        let short_delta = |axis: usize| {
            let now = jka_movement::angle_to_short(self.live_input.view_angles[axis]);
            (now.wrapping_sub(predicted_with[axis]) as i16) as f32 * (360.0 / 65536.0)
        };
        let pitch = (ps.field_f32("viewangles[0]")? + short_delta(0))
            .clamp(-16000.0 * 360.0 / 65536.0, 16000.0 * 360.0 / 65536.0);
        Some([
            pitch,
            ps.field_f32("viewangles[1]")? + short_delta(1),
            ps.field_f32("viewangles[2]")?,
        ])
    }

    /// CL_ForwardCommandToServer.
    fn forward_command_to_server(&mut self, line: &str) {
        let line = line.trim();
        let verb = line.split_whitespace().next().unwrap_or("");
        if verb.starts_with('-') {
            return;
        }
        if verb.starts_with('+') || !self.live_connected() {
            self.push_console_line(format!("^1Unknown command \"{verb}^7\""));
            return;
        }
        let text = if line.split_whitespace().nth(1).is_some() { line } else { verb };
        if let Some(net) = self.net.as_mut() {
            if let Err(error) = net.session_mut().add_reliable_command(text.as_bytes(), false) {
                self.push_console_line(format!("^1{error}"));
            }
        }
    }

    /// CL_CheckUserinfo: resend userinfo after a CVAR_USERINFO change.
    fn send_userinfo(&mut self) {
        if !self.live_connected() {
            return;
        }
        let info = self.network.userinfo(&self.solo_client_info.model_cvar());
        let command = [b"userinfo \"".as_slice(), &info, b"\""].concat();
        if let Some(net) = self.net.as_mut() {
            let _ = net.session_mut().add_reliable_command(&command, false);
        }
    }

    /// Engine and cgame commands that only exist for a live connection.
    /// Returns true when the line was consumed.
    fn execute_network_command(&mut self, line: &str) -> bool {
        let words: Vec<&str> = line.split_whitespace().collect();
        let Some(&verb) = words.first() else { return false };
        let verb_lower = verb.to_ascii_lowercase();
        match verb_lower.as_str() {
            "connect" => {
                match words.get(1) {
                    Some(target) if words.len() == 2 => self.connect_to_server(target),
                    _ => self.push_console_line("^3usage:^7 connect [server]"),
                }
                return true;
            }
            "reconnect" => {
                if let Some(target) = self.reconnect_target.clone() {
                    self.connect_to_server(&target);
                }
                return true;
            }
            "cmd" => {
                // CL_ForwardToServer_f: requires CA_ACTIVE; argv(0) is not sent.
                let active = self
                    .net
                    .as_ref()
                    .is_some_and(|net| net.state() == jka_protocol::session::ConnectionState::Active);
                if !active {
                    self.push_console_line("Not connected to a server.");
                } else if let Some(rest) = line.split_once(char::is_whitespace).map(|(_, rest)| rest.trim()) {
                    if !rest.is_empty() {
                        if let Some(net) = self.net.as_mut() {
                            let _ = net.session_mut().add_reliable_command(rest.as_bytes(), false);
                        }
                    }
                }
                return true;
            }
            "userinfo" => {
                let info = self.network.userinfo(&self.solo_client_info.model_cvar());
                self.push_console_line(format!("^3userinfo:^7 {}", String::from_utf8_lossy(&info)));
                return true;
            }
            "serverinfo" | "systeminfo" if self.live_connected() => {
                let index = if verb_lower == "serverinfo" { 0 } else { 1 };
                let text = self
                    .net
                    .as_ref()
                    .and_then(|net| net.session().decoder().configstrings.get(&index).cloned())
                    .unwrap_or_default();
                for pair in text.split(|&b| b == b'\\').filter(|part| !part.is_empty()).collect::<Vec<_>>().chunks(2) {
                    let key = String::from_utf8_lossy(pair[0]);
                    let value = pair.get(1).map(|v| String::from_utf8_lossy(v).into_owned()).unwrap_or_default();
                    self.push_console_line(format!("{key:<24} {value}"));
                }
                return true;
            }
            _ => {}
        }
        if !self.live_connected() {
            return false;
        }
        // cgame commands that only change the local usercmd.
        if let Some(value) = crate::net::generic_command(&verb_lower) {
            self.live_input.queue_generic_command(value);
            return true;
        }
        if matches!(verb_lower.as_str(), "weapnext" | "weapprev" | "weapon") {
            if let Some(ps) = self.live_player_state().cloned() {
                match verb_lower.as_str() {
                    "weapnext" => self.live_input.cycle_weapon(&ps, true),
                    "weapprev" => self.live_input.cycle_weapon(&ps, false),
                    _ => {
                        if let Some(slot) = words.get(1) {
                            self.live_input.select_weapon_slot(&ps, slot);
                        }
                    }
                }
            }
            return true;
        }
        // gcmds are registered with no handler: always forwarded while connected.
        if crate::console::is_server_command(verb) {
            self.forward_command_to_server(line);
            return true;
        }
        false
    }

    fn start_live_cgame_prep(&mut self) {
        self.live_cgame_prep_rx = None;
        self.live_cgame_prepared = None;
        self.pending_live_gamestate = false;

        let base = self.base.clone();
        let game = self.game.clone();
        let pbr = self.video.pbr;
        let allow_asset_overrides = self.video.allow_asset_overrides;
        let load_stringed = self.stringed.is_none();
        let (tx, rx) = mpsc::channel();
        let spawn = thread::Builder::new()
            .name("jka-live-cgame-prep".into())
            .spawn(move || {
                let started = Instant::now();
                let result =
                    App::build_cgame_cpu_assets(&base, game.as_deref(), pbr, allow_asset_overrides).map(
                    |(siege_classes, player_presenter, entity_presenter, fx_assets)| {
                        let stringed = if load_stringed {
                            jka_assets::pk3::AssetSearchPath::open_game(&base, game.as_deref())
                                .ok()
                                .map(|mut assets| crate::cgame::stringed::StringEd::load(&mut assets))
                        } else {
                            None
                        };
                        LiveCgamePrepared {
                            siege_classes,
                            player_presenter,
                            entity_presenter,
                            fx_assets,
                            stringed,
                            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                        }
                    },
                );
                let _ = tx.send(result);
            });
        match spawn {
            Ok(_) => self.live_cgame_prep_rx = Some(rx),
            Err(error) => {
                eprintln!("CGAME PREP: could not start worker: {error}");
                self.push_console_line(format!(
                    "^3CGAME PREP:^7 worker unavailable ({error}); will build at gamestate"
                ));
            }
        }
    }

    fn poll_live_cgame_prep(&mut self) {
        let result = match self.live_cgame_prep_rx.as_ref() {
            Some(rx) => rx.try_recv(),
            None => return,
        };
        match result {
            Ok(Ok(mut prepared)) => {
                self.live_cgame_prep_rx = None;
                if self.stringed.is_none() {
                    if let Some(table) = prepared.stringed.take() {
                        println!("STRINGED: {} strings preloaded on CGame worker", table.len());
                        self.stringed = Some(table);
                    }
                }
                if let Some(timing) = self.live_join_timing.as_mut() {
                    let elapsed = timing.elapsed_ms();
                    timing.cgame_ready_ms = Some(elapsed);
                }
                println!("[JOIN] CGame CPU prep ready in {:.1} ms", prepared.elapsed_ms);
                self.live_cgame_prepared = Some(prepared);
            }
            Ok(Err(error)) => {
                self.live_cgame_prep_rx = None;
                eprintln!("CGAME PREP: {error}");
                self.push_console_line(format!(
                    "^3CGAME PREP:^7 {error}; falling back to synchronous init"
                ));
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.live_cgame_prep_rx = None;
                self.push_console_line(
                    "^3CGAME PREP:^7 worker ended without a result; falling back to synchronous init"
                        .to_owned(),
                );
            }
        }
    }

    fn normalized_bsp_name(name: &str) -> String {
        let normalized = name.trim().replace('\\', "/");
        let normalized = normalized.strip_prefix("maps/").unwrap_or(&normalized);
        let normalized = normalized.strip_suffix(".bsp").unwrap_or(normalized);
        normalized.to_ascii_lowercase()
    }

    fn loading_map_matches(&self, map_name: &str) -> bool {
        let wanted = Self::normalized_bsp_name(map_name);
        self.loading
            .as_ref()
            .is_some_and(|loading| Self::normalized_bsp_name(&loading.name) == wanted)
    }

    fn loaded_map_matches(&self, map_name: &str) -> bool {
        !self.live_without_world
            && self.loading.is_none()
            && Self::normalized_bsp_name(&self.map_name) == Self::normalized_bsp_name(map_name)
    }

    fn handle_live_server_info(&mut self, info: jka_protocol::ServerInfo) {
        let elapsed_ms = self.live_join_timing.as_ref().map(LiveJoinTiming::elapsed_ms);
        if let Some(timing) = self.live_join_timing.as_mut() {
            timing.server_info_ms = elapsed_ms;
        }
        let Some(map_bytes) = info.get(b"mapname") else {
            return;
        };
        let map_name = String::from_utf8_lossy(map_bytes).into_owned();
        println!(
            "[JOIN] infoResponse protocol={} map={} at +{:.1} ms",
            info.protocol,
            map_name,
            elapsed_ms.unwrap_or(0.0)
        );
        if !info.is_protocol_26() {
            return;
        }
        let advertised_game = info.get(b"game").unwrap_or_default();
        match self.set_session_fs_game(advertised_game, "server info") {
            Ok(true) => self.start_live_cgame_prep(),
            Ok(false) => {}
            Err(error) => {
                self.push_console_line(format!("^1Rejected server fs_game: {error}"));
                self.disconnect_to_main_menu();
                return;
            }
        }
        if self.loaded_map_matches(&map_name) || self.loading_map_matches(&map_name) {
            println!("[JOIN] map {map_name} already loaded/in flight; no prefetch restart");
            return;
        }
        let source = scene::MapSource::Bsp(map_name.clone());
        match scene::verify_map_source_exists(&self.base, self.game.as_deref(), &source) {
            Ok(()) => {
                if let Some(timing) = self.live_join_timing.as_mut() {
                    timing.prefetched_map = Some(map_name.clone());
                }
                self.push_console_line(format!("^5[JOIN]^7 prefetching {map_name} during handshake"));
                self.prefetch_live_map(source);
            }
            Err(error) => {
                println!("[JOIN] map prefetch skipped: {error}");
                self.push_console_line(format!(
                    "^3[JOIN]^7 map prefetch skipped; {error}"
                ));
            }
        }
    }

    fn connect_to_server(&mut self, target: &str) {
        if self.net.is_some() || self.game_session.is_some() {
            self.disconnect_to_main_menu();
        }
        // Start before DNS/socket setup so the join metric really measures from
        // the user's connect action to the first rendered world frame.
        self.live_join_timing = Some(LiveJoinTiming::new());
        let userinfo = self.network.userinfo(&self.solo_client_info.model_cvar());
        match crate::net::NetClient::connect(target, userinfo) {
            Ok(net) => {
                let resolved_server = net.session().server();
                self.push_console_line(format!(
                    "^7{target} resolved to {}",
                    resolved_server
                ));
                self.console_status = format!("CONNECTING TO {}...", target.to_ascii_uppercase());
                self.reconnect_target = Some(target.to_owned());
                self.live_input = crate::net::LiveInput::default();
                self.predictor.reset();
                self.pending_frontend_map_launch = false;
                self.pending_live_server_commands.clear();
                self.start_live_cgame_prep();
                self.net = Some(net);
                self.publish_ui();
            }
            Err(error) => {
                self.live_join_timing = None;
                self.console_status = error.clone();
                self.push_console_line(format!("^1{error}"));
            }
        }
    }

    /// Per-frame network work: CL_PacketEvent for every queued datagram,
    /// CL_SetCGameTime, CL_CreateNewCommands and CL_SendCmd.
    fn tick_network(&mut self, frame: Duration) {
        let Some(net) = self.net.as_mut() else { return };
        let events = net.pump();
        self.poll_live_cgame_prep();
        for event in events {
            self.handle_session_event(event);
            if self.net.is_none() {
                return;
            }
        }
        self.poll_live_cgame_prep();
        if self.pending_live_gamestate && self.live_cgame_prep_rx.is_none() {
            self.pending_live_gamestate = false;
            self.start_live_gamestate();
            if self.net.is_none() {
                return;
            }
        }
        if self
            .game_session
            .as_ref()
            .is_some_and(|session| session.live && session.phase == SessionPhase::WaitingForSnapshot)
        {
            self.begin_live_session();
        }
        if self.scores_showing {
            self.request_scores_if_due(false);
        }
        let Some(net) = self.net.as_mut() else { return };
        let realtime = net.realtime();
        net.session_mut().set_cgame_time(realtime, self.network.time_nudge);
        if net.state() >= jka_protocol::session::ConnectionState::Primed {
            // CL_MouseMove: accumulate this frame's mouse into cl.viewangles.
            let playing = self.overlay == OverlayMode::None && self.captured;
            if playing && (self.mouse_delta.0 != 0.0 || self.mouse_delta.1 != 0.0) {
                let (mx, my) = self.mouse_input.scale_mouse(self.mouse_delta, frame);
                self.live_input.apply_mouse(self.mouse_input.yaw * mx, self.mouse_input.pitch * my);
            }
            if let Some(ps) = self.game_session.as_ref().filter(|s| s.live).and_then(|s| s.current_snapshot.as_ref()) {
                let ps = ps.player_state.clone();
                self.live_input.sync_selection(&ps);
            }
            let empty = HashSet::new();
            let buttons = crate::net::CommandButtons {
                active: if playing { &self.live_buttons } else { &empty },
                any_key: playing && (!self.keys.is_empty() || !self.mouse_buttons_down.is_empty()),
                talking: !playing,
            };
            // CL_CreateNewCommands runs once per client frame; our tick rate
            // follows input events, so pace commands explicitly. Mouse keeps
            // accumulating in cl.viewangles between commands.
            let now = Instant::now();
            let interval = Duration::from_secs_f64(1.0 / f64::from(self.network.command_rate.max(15)));
            let due = self
                .last_command_at
                .is_none_or(|last| now.saturating_duration_since(last) >= interval);
            if due {
                self.last_command_at = Some(match self.last_command_at {
                    // Keep a steady cadence without accumulating backlog.
                    Some(last) if now.saturating_duration_since(last) < interval * 2 => last + interval,
                    _ => now,
                });
                let cmd = self.live_input.create_cmd(&buttons);
                let net = self.net.as_mut().expect("checked above");
                net.session_mut().create_command(cmd);
            }
            let server_time = self.net.as_ref().expect("checked above").session().server_time();
            let mut provisional = self.live_input.preview_cmd(&buttons);
            provisional.server_time = server_time;
            self.live_provisional = Some(provisional);
        } else {
            self.live_provisional = None;
        }
        let net = self.net.as_mut().expect("checked above");
        net.session_mut().send_commands(realtime, self.network.max_packets as i32);
        net.flush();
    }

    fn handle_session_event(&mut self, event: jka_protocol::session::SessionEvent) {
        use jka_protocol::session::{ConnectionState, SessionEvent};
        match event {
            SessionEvent::StateChanged(state) => {
                let text = match state {
                    ConnectionState::Connecting => "Awaiting challenge...",
                    ConnectionState::Challenging => "Awaiting connection...",
                    ConnectionState::Connected => "Awaiting gamestate...",
                    ConnectionState::Primed => "Awaiting snapshot...",
                    ConnectionState::Active => "Connected.",
                    ConnectionState::Disconnected => "Disconnected.",
                };
                self.console_status = text.to_ascii_uppercase();
                println!("NET: {state:?} - {text}");
                self.push_console_line(format!("^5{text}"));
                if state == ConnectionState::Connected {
                    if let Some(address) = self.net.as_ref().map(|net| net.session().server()) {
                        if let Err(error) = self.server_browser.remember_history(address) {
                            eprintln!("SERVER BROWSER: could not save history: {error}");
                        }
                    }
                }
            }
            SessionEvent::ServerInfo(info) => self.handle_live_server_info(info),
            SessionEvent::Print(text) => {
                let text = self.translate_server_text(&text);
                for line in text.lines().filter(|line| !line.is_empty()) {
                    self.push_console_line(line.to_owned());
                }
            }
            SessionEvent::Gamestate => {
                let elapsed = self.live_join_timing.as_ref().map(LiveJoinTiming::elapsed_ms);
                if let (Some(timing), Some(elapsed)) = (self.live_join_timing.as_mut(), elapsed) {
                    if timing.gamestate_ms.is_none() {
                        timing.gamestate_ms = Some(elapsed);
                        println!("[JOIN] gamestate received at +{elapsed:.1} ms");
                    }
                }
                self.start_live_gamestate();
            }
            SessionEvent::ServerCommand(command) => {
                if let Some(session) = self.game_session.as_mut().filter(|session| session.live) {
                    session.client_game.queue_server_command(command);
                } else if self.net.is_some() {
                    // A gamestate can beat the CPU prep worker. Keep reliable
                    // commands received in that overlap window instead of
                    // dropping them just because GameSession is not built yet.
                    if self.pending_live_server_commands.len() >= 128 {
                        self.pending_live_server_commands.pop_front();
                    }
                    self.pending_live_server_commands.push_back(command);
                }
            }
            SessionEvent::Snapshot(snapshot) => {
                if let Some(session) = self.game_session.as_mut().filter(|session| session.live) {
                    if session.live_snapshots.len() >= 128 {
                        session.live_snapshots.pop_front();
                    }
                    session.live_snapshots.push_back(snapshot);
                }
            }
            SessionEvent::DemoMessage { sequence, payload, full_snapshot } => {
                self.record_demo_message(sequence, payload, full_snapshot);
            }
            SessionEvent::MapChange => {}
            SessionEvent::Disconnected(reason) => {
                let reason = self.translate_server_text(reason.as_bytes());
                println!("NET: disconnected: {reason}");
                self.push_console_line(format!("^1{reason}"));
                self.net = None;
                self.disconnect_to_main_menu();
                self.console_status = reason.to_ascii_uppercase();
                self.push_console_line(format!("^3Use ^7reconnect^3 to rejoin."));
            }
        }
    }

    /// Enter CA_PRIMED without loading world geometry. This is the live equivalent
    /// of finishing downloads with no BSP: CGame/entities keep running, while
    /// prediction falls back to snapshot interpolation because map collision is absent.
    fn prime_live_session_without_world(&mut self, map_name: &str, reason: &str) {
        // Invalidate any previous/front-end map job so a late worker result cannot
        // repopulate a stale world behind this live session.
        self.latest_request_id = self.latest_request_id.wrapping_add(1);
        self.pending_frontend_map_launch = false;
        self.frontend_background_request_id = None;
        self.frontend_cinematic = None;
        self.preserve_game_state_on_next_map_upload = false;
        self.loading = None;
        self.static_ao_progress = None;
        self.prepared_map_cache = None;

        self.local_player = None;
        self.map_collision = None;
        self.map_physics_collision = PhysicsMapMesh::default();
        self.map_movement = None;
        self.third_person_camera.reset();
        self.solo_dynamic_models = Arc::new(Vec::new());
        self.spawns.clear();
        self.spawn_index = 0;
        self.map_name = format!("{map_name} (WORLD MISSING)");
        self.triangles = 0;
        self.map_distance_cull = crate::camera::DEFAULT_DISTANCE_CULL;
        self.map_authored_sun = None;
        self.map_authored_oceans.clear();
        self.authored_oceans.clear();
        self.authored_ocean_preview = false;
        self.render_command(RenderCommand::SetAuthoredOceans(Vec::new()));
        self.surface_inspector = None;
        self.front_end = false;
        self.frontend_page = FrontendPage::Main;
        self.menu_selected = 0;
        self.live_without_world = true;
        self.render_command(RenderCommand::UnloadMap);
        self.sync_render_fps_cap();

        self.console_status = format!("CONNECTED WITHOUT MAP: {map_name}");
        self.push_console_line(format!(
            "^3Map unavailable locally; continuing without world geometry:^7 {reason}"
        ));
        self.push_console_line(
            "^3Prediction is disabled until a local BSP is loaded; player/entity presentation remains active."
                .to_owned(),
        );
        self.prime_live_session(map_name);
        self.publish_snapshot();
        self.publish_ui();
    }

    /// CL_ParseGamestate -> CL_InitCGame: a new CGame for the new gamestate.
    fn start_live_gamestate(&mut self) {
        // CL_SystemInfoChanged: the gamestate systeminfo is authoritative for
        // fs_game. Inspect it before accepting any overlapped presenter/map prep
        // that may have been built speculatively from infoResponse.
        let (map_name, configstrings, command_sequence, client_num, pure, server_name, fs_game) = {
            let Some(net) = self.net.as_ref() else { return };
            let decoder = net.session().decoder();
            let Some(map_name) = decoder.map_name() else {
                self.push_console_line("^1Gamestate has no mapname.");
                return;
            };
            let fs_game = decoder
                .configstrings
                .get(&jka_protocol::session::CS_SYSTEMINFO)
                .and_then(|info| jka_protocol::commands::info_value(info, b"fs_game"))
                .unwrap_or_default()
                .to_vec();
            (
                map_name,
                decoder.configstrings.clone(),
                decoder.server_command_sequence,
                decoder.client_number,
                net.session().sv_pure(),
                net.server_name.clone(),
                fs_game,
            )
        };
        match self.set_session_fs_game(&fs_game, "gamestate systeminfo") {
            Ok(true) => self.start_live_cgame_prep(),
            Ok(false) => {}
            Err(error) => {
                self.push_console_line(format!("^1Rejected server fs_game: {error}"));
                self.disconnect_to_main_menu();
                return;
            }
        }

        self.poll_live_cgame_prep();
        if self.live_cgame_prepared.is_none() && self.live_cgame_prep_rx.is_some() {
            if !self.pending_live_gamestate {
                println!("[JOIN] gamestate arrived before CGame prep; waiting for worker");
                self.push_console_line("^5[JOIN]^7 gamestate received; finishing overlapped CGame prep".to_owned());
            }
            self.pending_live_gamestate = true;
            return;
        }
        self.pending_live_gamestate = false;

        if pure {
            self.push_console_line("^3Server is sv_pure 1; pure checksums are not sent yet and it may drop this client.");
        }

        let mut session = if let Some(prepared) = self.live_cgame_prepared.take() {
            let mut session = GameSession::new(
                format!("live:{server_name}"),
                server_name.clone(),
                Vec::new(),
                prepared.siege_classes,
                prepared.player_presenter,
                prepared.entity_presenter,
                crate::cgame::weapon_fx::WeaponFx::new(prepared.fx_assets),
            );
            self.attach_live_sound_presenter(&mut session);
            session
        } else {
            match self.build_game_session(format!("live:{server_name}"), server_name.clone(), Vec::new()) {
                Ok(session) => session,
                Err(error) => {
                    self.push_console_line(format!("^1{error}"));
                    self.disconnect_to_main_menu();
                    return;
                }
            }
        };
        session.live = true;
        session.map_name = Some(map_name.clone());
        if let Err(error) = session
            .player_presenter
            .set_physics_map_mesh(&self.map_physics_collision)
        {
            eprintln!("RAPIER MAP COLLISION ERROR: {error}");
        }
        session.client_game.reset_gamestate(&configstrings, command_sequence);
        while let Some(command) = self.pending_live_server_commands.pop_front() {
            session.client_game.queue_server_command(command);
        }
        self.game_session = Some(session);
        self.predictor.reset();
        self.local_player = None;
        println!("NET: gamestate {map_name} clientNum={client_num} configstrings={}", configstrings.len());
        self.push_console_line(format!("^2Gamestate: {map_name} (client {client_num})"));

        if self.loaded_map_matches(&map_name) {
            if self.front_end {
                self.pending_frontend_map_launch = false;
                self.front_end = false;
                self.frontend_page = FrontendPage::Main;
                self.menu_selected = 0;
                self.sync_render_fps_cap();
            }
            println!("[JOIN] adopting already-rendered prefetched map {map_name}");
            self.prime_live_session(&map_name);
            self.publish_snapshot();
            self.publish_ui();
            return;
        }

        if self.loading_map_matches(&map_name) {
            self.live_without_world = false;
            // If MapPrepared has not fired yet this follows the normal launch
            // path. If it already fired, WorldUploaded below performs the same
            // front-end transition before priming.
            if self.front_end {
                self.pending_frontend_map_launch = true;
            }
            println!("[JOIN] reusing in-flight map prefetch for {map_name}");
            self.push_console_line(format!("^5[JOIN]^7 reusing in-flight {map_name} prefetch"));
            return;
        }

        let source = scene::MapSource::Bsp(map_name.clone());
        match scene::verify_map_source_exists(&self.base, self.game.as_deref(), &source) {
            Ok(()) => {
                self.live_without_world = false;
                self.request_map(source);
            }
            Err(error) if self.network.allow_missing_map => {
                self.prime_live_session_without_world(&map_name, &error);
            }
            Err(error) => {
                self.console_status = format!("MAP REQUIRED: {error}");
                self.push_console_line(format!("^1{}", self.console_status));
                self.push_console_line(
                    "^3Set cl_allowMissingMap 1 to connect without world geometry.".to_owned(),
                );
                self.disconnect_to_main_menu();
            }
        }
    }

    /// CL_DownloadsComplete after the map is up: CA_PRIMED, usercmds start.
    fn prime_live_session(&mut self, loaded_map: &str) {
        let Some(session) = self.game_session.as_mut().filter(|session| session.live) else { return };
        let loaded_qpath = loaded_map.strip_suffix(".bsp").unwrap_or(loaded_map);
        if session.phase != SessionPhase::WaitingForMap
            || !session.map_name.as_deref().is_some_and(|map| map.eq_ignore_ascii_case(loaded_qpath))
        {
            return;
        }
        session.phase = SessionPhase::WaitingForSnapshot;
        self.local_player = None;
        if let Some(net) = self.net.as_mut() {
            let realtime = net.realtime();
            net.session_mut().set_primed(realtime);
            net.flush();
        }
        self.set_overlay(OverlayMode::None);
    }

    fn begin_live_session(&mut self) {
        let Some(session) = self.game_session.as_mut() else { return };
        match session.begin_live() {
            Ok(true) => {
                self.previous_tick = Instant::now();
                println!("NET: first active snapshot; CGame running");
                self.push_console_line("^2Entered the game.");
                let summary = if let Some(timing) = self.live_join_timing.as_mut() {
                    let active_ms = timing.elapsed_ms();
                    Some(format!(
                        "[JOIN] active +{active_ms:.1} ms | info {} | cgame {} | gamestate {} | first-world {}",
                        timing.server_info_ms.map_or_else(|| "n/a".into(), |v| format!("{v:.1}")),
                        timing.cgame_ready_ms.map_or_else(|| "n/a".into(), |v| format!("{v:.1}")),
                        timing.gamestate_ms.map_or_else(|| "n/a".into(), |v| format!("{v:.1}")),
                        timing.first_world_frame_ms.map_or_else(|| "n/a".into(), |v| format!("{v:.1}")),
                    ))
                } else {
                    None
                };
                if let Some(summary) = summary {
                    println!("{summary}");
                    self.push_console_line(format!("^5{}", summary));
                }
            }
            Ok(false) => {}
            Err(error) => {
                self.push_console_line(format!("^1{error}"));
                self.disconnect_to_main_menu();
            }
        }
    }

    /// CG_CheckSVStringEdRef against the lazily loaded StringEd table.
    fn translate_server_text(&mut self, text: &[u8]) -> String {
        if self.stringed.is_none() {
            let table = jka_assets::pk3::AssetSearchPath::open_game(&self.base, self.game.as_deref())
                .map(|mut assets| crate::cgame::stringed::StringEd::load(&mut assets))
                .unwrap_or_default();
            println!("STRINGED: {} strings loaded", table.len());
            self.stringed = Some(table);
        }
        let translated = self.stringed.as_ref().expect("loaded above").translate_server_text(text);
        crate::cgame::bytes_to_lossless_ascii(&translated)
    }

    /// CG_ServerCommand text output for the console and chat box.
    fn drain_cgame_notices(&mut self) {
        let Some(session) = self.game_session.as_mut() else { return };
        let gametype = session.client_game.gametype();
        let notices = session.client_game.drain_notices();
        let misses = std::mem::take(&mut self.predictor.misses);
        for miss in misses {
            self.push_console_line(miss);
        }
        for notice in notices {
            match notice {
                crate::cgame::CgameNotice::Print(text) => {
                    let text = self.translate_server_text(&text);
                    for line in text.lines().filter(|line| !line.is_empty()) {
                        self.push_console_line(line.to_owned());
                    }
                }
                crate::cgame::CgameNotice::Chat { text, .. } => {
                    let text = crate::cgame::bytes_to_lossless_ascii(&text);
                    self.push_console_line(text.clone());
                    if self.chat_lines.len() >= 16 {
                        self.chat_lines.pop_front();
                    }
                    self.chat_lines.push_back(ChatRecord { text, created: Instant::now() });
                    self.last_chat_refresh = Instant::now();
                    self.publish_ui();
                }
                crate::cgame::CgameNotice::CenterPrint(text) => {
                    // Every cp resets CG_CenterPrint time, including repeats.
                    // Keep console de-duplication separate from presentation.
                    let rendered = self.translate_server_text(&text);
                    self.center_print = Some(CenterPrintRecord {
                        text: rendered.clone(),
                        created: Instant::now(),
                    });
                    self.last_center_refresh = Instant::now();
                    if text != self.last_center_print {
                        for line in rendered.lines().filter(|line| !line.trim().is_empty()) {
                            self.push_console_line(line.to_owned());
                        }
                        self.last_center_print = text;
                    }
                    self.publish_ui();
                }
                crate::cgame::CgameNotice::Scores { team_scores, entries } => {
                    self.scoreboard = Some(UiScoreboard {
                        team_scores,
                        // JKA team modes start at GT_TEAM (6): Team FFA, Siege,
                        // CTF and CTY. Duel/FFA scoreboards do not need team UI.
                        team_game: gametype >= 6,
                        entries: entries
                            .into_iter()
                            .map(|entry| UiScoreEntry {
                                name: entry.name,
                                score: entry.score,
                                ping: entry.ping,
                                time: entry.time,
                                team: entry.team,
                            })
                            .collect(),
                    });
                    if self.scores_showing {
                        self.publish_ui();
                    }
                }
                crate::cgame::CgameNotice::MapRestart => {
                    self.predictor.reset();
                    self.push_console_line("^3map_restart");
                }
            }
        }
    }

    pub fn queue_startup_commands(&mut self, commands: Vec<String>) {
        self.startup_commands = commands;
    }

    fn toggle_noclip(&mut self) {
        let result = if let Some(player) = &mut self.local_player {
            player.toggle_noclip()
        } else {
            Err("NOCLIP: NO LOCAL PLAYER".into())
        };

        match result {
            Ok(true) => {
                self.console_status =
                    "NOCLIP ON  |  MOUSE1/MOUSE2 = 10X SPEED EACH  |  BOTH = 100X".into();
                self.push_console_line(format!("^2{}", self.console_status));
            }
            Ok(false) => {
                self.console_status = "NOCLIP OFF".into();
                self.push_console_line(format!("^3{}", self.console_status));
            }
            Err(error) => {
                self.console_status = error;
                self.push_console_line(format!("^1{}", self.console_status));
            }
        }
        self.update_solo_player_view_and_presentation();
        self.publish_snapshot();
        self.publish_ui();
    }

    fn cycle_spawn(&mut self) {
        if self.spawns.is_empty() {
            return;
        }
        self.spawn_index = (self.spawn_index + 1) % self.spawns.len();
        let spawn = self.spawns[self.spawn_index];
        if let Some(player) = &mut self.local_player {
            if let Err(error) = player.respawn(spawn) {
                self.console_status = error;
            }
        }
        self.third_person_camera.reset();
        self.update_solo_player_view_and_presentation();
        self.publish_snapshot();
    }

    fn trace_surface_center(&mut self) {
        let size = self
            .window
            .as_ref()
            .map(|window| window.inner_size())
            .unwrap_or_default();
        if size.width == 0 || size.height == 0 {
            self.console_status = "SURFACE TRACE UNAVAILABLE: WINDOW HAS NO RENDER SIZE".into();
            self.publish_ui();
            return;
        }

        self.render_command(RenderCommand::InspectSurface {
            x: size.width as f32 * 0.5,
            y: size.height as f32 * 0.5,
            width: size.width,
            height: size.height,
        });
    }

    fn clear_surface_inspection(&mut self) {
        self.surface_inspector = None;
        self.render_command(RenderCommand::ClearSurfaceInspection);
        self.publish_ui();
    }

    fn copy_surface_inspector_text(&mut self) {
        let Some(info) = &self.surface_inspector else {
            return;
        };
        let mut text = String::from("SURFACE INSPECTOR\n");
        text.push_str(&info.title);
        text.push('\n');
        for line in &info.lines {
            text.push_str(line);
            text.push('\n');
        }
        match crate::clipboard::set_text(text.trim_end()) {
            Ok(()) => {
                self.console_status = "SURFACE INSPECTOR COPIED TO CLIPBOARD".into();
            }
            Err(error) => {
                self.console_status = format!("CLIPBOARD ERROR: {error}");
                self.push_console_line(format!("^1{}", self.console_status));
            }
        }
        self.publish_ui();
    }

    /// Keyboard for the Game-side menus. egui owns navigation and activation;
    /// only the two things egui cannot do land here: backing out of the menu,
    /// and capturing a raw key/button while rebinding a control.
    fn handle_game_key(&mut self, event: &KeyEvent, code: KeyCode) {
        if event.state != ElementState::Pressed {
            return;
        }

        if self.controls_page_active() && self.controls_waiting_for_key {
            if event.repeat {
                return;
            }
            if code == KeyCode::Escape {
                self.controls_waiting_for_key = false;
                self.egui_repaint_requested = true;
                self.publish_ui();
            } else {
                self.bind_control_key(keybinds::bind_key_for_code(code));
            }
            return;
        }

        if code != KeyCode::Escape {
            return;
        }

        if self.front_end {
            self.frontend_back();
        } else {
            self.set_overlay(OverlayMode::None);
        }
    }

    fn exclusive_video_mode(
        window: &Window,
        resolution: [u32; 2],
    ) -> Option<winit::monitor::VideoModeHandle> {
        let monitor = window
            .current_monitor()
            .or_else(|| window.primary_monitor())?;
        let target_size = PhysicalSize::new(resolution[0], resolution[1]);
        let target_refresh = monitor.refresh_rate_millihertz().unwrap_or(0);
        monitor
            .video_modes()
            .filter(|mode| mode.size() == target_size)
            .min_by_key(|mode| {
                (
                    target_refresh.abs_diff(mode.refresh_rate_millihertz()),
                    std::cmp::Reverse(mode.bit_depth()),
                )
            })
    }

    fn native_fullscreen_mode(mode: FullscreenMode, backend: RendererBackend) -> FullscreenMode {
        if mode == FullscreenMode::Exclusive && backend == RendererBackend::Dx12 {
            FullscreenMode::Borderless
        } else {
            mode
        }
    }

    fn apply_fullscreen_mode(
        window: &Window,
        mode: FullscreenMode,
        resolution: [u32; 2],
        backend: RendererBackend,
    ) -> Result<(), String> {
        match mode {
            FullscreenMode::Windowed => {
                window.set_fullscreen(None);
                Ok(())
            }
            FullscreenMode::Borderless => {
                window.set_fullscreen(Some(Fullscreen::Borderless(None)));
                Ok(())
            }
            FullscreenMode::Exclusive if backend == RendererBackend::Dx12 => {
                // Direct3D 12 does not expose classic fullscreen-exclusive mode.
                // wgpu's DX12 surface is an HWND flip-model swapchain, and putting
                // that HWND into Winit's legacy Exclusive state makes
                // CreateSwapChainForHwnd/Surface::configure invalid on Windows.
                // Use the D3D12-native fullscreen path instead: a monitor-sized
                // borderless HWND that Windows may promote through FSO/DirectFlip/
                // Independent Flip. Keep the logical "Exclusive" selection so
                // Vulkan can still use a real monitor-mode switch when selected.
                window.set_fullscreen(Some(Fullscreen::Borderless(None)));
                Ok(())
            }
            FullscreenMode::Exclusive => {
                let video_mode =
                    Self::exclusive_video_mode(window, resolution).ok_or_else(|| {
                        format!(
                            "No exclusive fullscreen video mode matches {}x{}",
                            resolution[0], resolution[1]
                        )
                    })?;
                window.set_fullscreen(Some(Fullscreen::Exclusive(video_mode)));
                Ok(())
            }
        }
    }

    fn set_fullscreen_mode(&mut self, mode: FullscreenMode) {
        if mode == self.video.fullscreen {
            return;
        }

        self.video.fullscreen = mode;
        self.mark_config_dirty();
        self.console_status = format!(
            "FULLSCREEN: {} (PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART)",
            mode.label()
        );
        self.publish_ui();
    }

    fn cycle_fullscreen(&mut self, direction: i32) {
        let modes: &[FullscreenMode] = &[
            FullscreenMode::Windowed,
            FullscreenMode::Borderless,
            FullscreenMode::Exclusive,
        ];
        let current = modes
            .iter()
            .position(|mode| *mode == self.video.fullscreen)
            .unwrap_or(0) as i32;
        let next = (current + direction).rem_euclid(modes.len() as i32) as usize;
        self.set_fullscreen_mode(modes[next]);
    }

    fn available_resolutions(window: Option<&Window>, current: [u32; 2]) -> Vec<[u32; 2]> {
        let mut unique = BTreeSet::new();

        if let Some(window) = window {
            if let Some(monitor) = window
                .current_monitor()
                .or_else(|| window.primary_monitor())
            {
                // A monitor usually reports the same pixel dimensions once per
                // refresh-rate / bit-depth combination. Collapse those down to
                // one menu entry per actual resolution. Keep tiny legacy modes
                // out of the normal list, but always retain the monitor's native
                // size below.
                for mode in monitor.video_modes() {
                    let size = mode.size();
                    if size.width >= 1024 && size.height >= 720 {
                        unique.insert((size.width, size.height));
                    }
                }

                let native = monitor.size();
                if native.width > 0 && native.height > 0 {
                    unique.insert((native.width, native.height));
                }
            }
        }

        if unique.is_empty() {
            unique.extend(
                FALLBACK_RESOLUTIONS
                    .iter()
                    .map(|resolution| (resolution[0], resolution[1])),
            );
        }

        // Preserve a custom/configured window size even when it is not an
        // advertised exclusive display mode, so the first arrow press advances
        // naturally from what the user currently has selected.
        unique.insert((current[0], current[1]));

        let mut resolutions: Vec<_> = unique
            .into_iter()
            .map(|(width, height)| [width, height])
            .collect();
        resolutions.sort_unstable_by_key(|resolution| {
            (
                u64::from(resolution[0]) * u64::from(resolution[1]),
                resolution[0],
                resolution[1],
            )
        });
        resolutions
    }

    fn cycle_resolution(&mut self, direction: i32) {
        let resolutions =
            Self::available_resolutions(self.window.as_deref(), self.video.resolution);
        let current = resolutions
            .iter()
            .position(|value| *value == self.video.resolution)
            .unwrap_or(0) as i32;
        let next = (current + direction).rem_euclid(resolutions.len() as i32) as usize;
        self.video.resolution = resolutions[next];
        self.mark_config_dirty();
        self.console_status = format!(
            "RESOLUTION: {}X{} (PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART)",
            self.video.resolution[0], self.video.resolution[1]
        );
    }

    /// The anti-aliasing ladder, ordered by the menu's intended quality scale:
    /// Off/FXAA first, then the adapter-supported MSAA levels, then SMAA and TAA.
    /// TAA remains the final option because it also resolves shader/specular
    /// aliasing across frames. MSAA levels are probed per adapter, so that part
    /// of the ladder is built fresh rather than kept in a const.
    pub(super) fn anti_aliasing_ladder(&self) -> Vec<(AntiAliasingChoice, String)> {
        let mut choices = vec![
            (AntiAliasingChoice::Off, "Off".to_owned()),
            (AntiAliasingChoice::Fxaa, "FXAA".to_owned()),
        ];
        choices.extend(
            self.supported_msaa
                .iter()
                .copied()
                .filter(|samples| *samples > 1)
                .map(|samples| (AntiAliasingChoice::Msaa(samples), format!("{samples}× MSAA"))),
        );
        choices.push((AntiAliasingChoice::Smaa, "SMAA".to_owned()));
        choices.push((AntiAliasingChoice::Taa, "TAA".to_owned()));
        choices
    }

    pub(super) fn current_anti_aliasing(&self) -> AntiAliasingChoice {
        if self.video.taa {
            AntiAliasingChoice::Taa
        } else if self.video.smaa {
            AntiAliasingChoice::Smaa
        } else if self.video.fxaa {
            AntiAliasingChoice::Fxaa
        } else if self.video.msaa_samples > 1 {
            AntiAliasingChoice::Msaa(self.video.msaa_samples)
        } else {
            AntiAliasingChoice::Off
        }
    }

    fn cycle_anti_aliasing(&mut self, direction: i32) {
        let choices: Vec<AntiAliasingChoice> = self
            .anti_aliasing_ladder()
            .into_iter()
            .map(|(choice, _)| choice)
            .collect();
        let current = self.current_anti_aliasing();
        let current_index = choices
            .iter()
            .position(|choice| *choice == current)
            .unwrap_or(0) as i32;
        let next_index = (current_index + direction).rem_euclid(choices.len() as i32) as usize;
        let next = choices[next_index];

        let previous_msaa = self.video.msaa_samples;
        self.video.msaa_samples = 1;
        self.video.fxaa = false;
        self.video.smaa = false;
        self.video.taa = false;
        match next {
            AntiAliasingChoice::Off => {}
            AntiAliasingChoice::Fxaa => self.video.fxaa = true,
            AntiAliasingChoice::Smaa => self.video.smaa = true,
            AntiAliasingChoice::Taa => self.video.taa = true,
            AntiAliasingChoice::Msaa(samples) => self.video.msaa_samples = samples,
        }

        if self.video.msaa_samples != previous_msaa {
            self.render_command(RenderCommand::SetMsaa(self.video.msaa_samples));
        }
        self.sync_post_effects();
        self.mark_config_dirty();
    }

    fn cycle_texture_filter(&mut self, direction: i32) {
        const MODES: [TextureFilter; 7] = [
            TextureFilter::Nearest,
            TextureFilter::Bilinear,
            TextureFilter::Trilinear,
            TextureFilter::Anisotropic2x,
            TextureFilter::Anisotropic4x,
            TextureFilter::Anisotropic8x,
            TextureFilter::Anisotropic16x,
        ];
        let current = MODES
            .iter()
            .position(|mode| *mode == self.video.texture_filter)
            .unwrap_or(1) as i32;
        let next = (current + direction).rem_euclid(MODES.len() as i32) as usize;
        self.video.texture_filter = MODES[next];
        self.render_command(RenderCommand::SetTextureFilter(self.video.texture_filter));
        self.mark_config_dirty();
    }

    fn cycle_renderer_backend(&mut self, direction: i32) {
        const BACKENDS: [RendererBackend; 2] = [RendererBackend::Vulkan, RendererBackend::Dx12];
        let current = BACKENDS
            .iter()
            .position(|backend| *backend == self.video.renderer_backend)
            .unwrap_or(0) as i32;
        let next = (current + direction).rem_euclid(BACKENDS.len() as i32) as usize;
        self.video.renderer_backend = BACKENDS[next];
        self.mark_config_dirty();
        self.console_status = format!(
            "RENDER BACKEND: {} (PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART)",
            self.video.renderer_backend.label()
        );
    }

    fn applied_video_mode(&self) -> AppliedVideoMode {
        AppliedVideoMode {
            fullscreen: self.applied_fullscreen,
            renderer_backend: self.applied_renderer_backend,
            resolution: self.applied_resolution,
        }
    }

    fn map_prepare_restart_required(&self) -> bool {
        if self.front_end && self.frontend_cinematic.is_none() {
            return false;
        }
        let label = self.initial_source.label();
        let Some(prepared) = self
            .prepared_map_cache
            .as_ref()
            .filter(|cache| cache.label == label)
            .map(|cache| cache.prepare_options)
        else {
            return false;
        };
        let requested = self.active_map_prepare_options();

        // GI can be disabled live because its prepared volume remains resident;
        // only enabling it on a map that was prepared without GI needs a rebuild.
        // Generated normals, FP16 lightmaps, PBR material-library selection, and
        // asset-override policy alter prepared material/texture data in both
        // directions, so any mismatch is restart-sensitive.
        (requested.voxel_probe_gi && !prepared.voxel_probe_gi)
            || (requested.client_physics && !prepared.client_physics)
            || requested.gen_normal_maps != prepared.gen_normal_maps
            || requested.float_lightmap != prepared.float_lightmap
            || requested.planar_reflections != prepared.planar_reflections
            || requested.pbr_materials != prepared.pbr_materials
            || requested.allow_asset_overrides != prepared.allow_asset_overrides
    }

    fn video_restart_required(&self) -> bool {
        self.video.fullscreen != self.applied_fullscreen
            || self.video.renderer_backend != self.applied_renderer_backend
            || self.video.resolution != self.applied_resolution
            || self.video.reflection_quality != self.applied_reflection_quality
            // Disabling these effects is a live operation. Enabling still needs
            // a restart only when this renderer/map was started without the
            // resources needed to draw them.
            || (self.video.grass && !self.applied_grass)
            || (self.video.ocean && !self.applied_ocean)
            || self.map_prepare_restart_required()
            || !self.latched_console_cvars.is_empty()
    }

    fn video_confirmation_seconds(&self) -> Option<u32> {
        let confirmation = self.video_confirmation?;
        let remaining = confirmation
            .deadline
            .saturating_duration_since(Instant::now());
        let seconds = remaining.as_secs() + if remaining.subsec_nanos() != 0 { 1 } else { 0 };
        Some(seconds.min(u32::MAX as u64) as u32)
    }

    fn begin_pending_video_confirmation(&mut self) {
        let Some(previous) = self.pending_video_confirmation.take() else {
            return;
        };
        let seconds = VIDEO_CONFIRM_TIMEOUT_SECS as u32;
        self.video_confirmation = Some(VideoConfirmation {
            previous,
            deadline: Instant::now() + Duration::from_secs(VIDEO_CONFIRM_TIMEOUT_SECS),
            shown_seconds: seconds,
        });
        let applied = self.applied_video_mode();
        self.console_status = format!(
            "VIDEO CHANGED TO {}X{} {} / {} - CONFIRM WITHIN {seconds} SECONDS",
            applied.resolution[0],
            applied.resolution[1],
            applied.fullscreen.label(),
            applied.renderer_backend.label()
        );
        self.push_console_line(format!("^3{}", self.console_status));
        self.egui_repaint_requested = true;
    }

    fn normalize_video_selection(&mut self) {
        if !self.video_restart_required() && self.video_selected == ui::VIDEO_ROW_VID_RESTART {
            self.video_selected = ui::VIDEO_ROW_RENDER_BACKEND;
        }
        if !self.video_restart_required()
            && self.environment_selected == ui::ENV_ROW_VID_RESTART
        {
            self.environment_selected = ui::ENV_ROW_OCEAN_SETTINGS;
        }
    }

    fn confirm_video_settings(&mut self) {
        if self.video_confirmation.take().is_none() {
            return;
        }
        self.normalize_video_selection();
        self.console_status = format!(
            "VIDEO SETTINGS KEPT: {}X{} {} / {}",
            self.applied_resolution[0],
            self.applied_resolution[1],
            self.applied_fullscreen.label(),
            self.applied_renderer_backend.label()
        );
        self.push_console_line(format!("^2{}", self.console_status));
        self.mark_config_dirty();
        self.flush_config();
        if self.overlay == OverlayMode::None {
            self.set_capture(true);
        }
        self.publish_ui();
    }

    fn revert_video_settings(&mut self, reason: &str) {
        // Persist any unrelated dirty settings while the confirmation object still
        // identifies the last known-good video mode. Do not lie to the runtime by
        // overwriting applied_* before the HWND/display mode has actually rolled back.
        self.flush_config();
        let previous = if let Some(confirmation) = self.video_confirmation.take() {
            confirmation.previous
        } else if let Some(previous) = self.pending_video_confirmation.take() {
            previous
        } else {
            return;
        };
        self.video.fullscreen = previous.fullscreen;
        self.video.renderer_backend = previous.renderer_backend;
        self.video.resolution = previous.resolution;
        self.normalize_video_selection();
        self.console_status = format!("VIDEO SETTINGS {reason}; REVERTING...");
        self.push_console_line(format!("^3{}", self.console_status));
        self.restart_renderer_internal(false);
    }

    fn apply_pending_display_settings(
        &mut self,
        window: &Window,
        mode: FullscreenMode,
    ) -> Result<(), String> {
        match mode {
            FullscreenMode::Windowed => {
                window.set_fullscreen(None);
                if let Some([x, y]) = self.video.window_position {
                    window.set_outer_position(PhysicalPosition::new(x, y));
                }
                if self.video.window_maximized {
                    window.set_maximized(true);
                } else {
                    window.set_maximized(false);
                    let _ = window.request_inner_size(PhysicalSize::new(
                        self.video.resolution[0],
                        self.video.resolution[1],
                    ));
                }
            }
            _ => Self::apply_fullscreen_mode(
                window,
                mode,
                self.video.resolution,
                self.video.renderer_backend,
            )?,
        }

        self.applied_fullscreen = mode;
        self.applied_resolution = self.video.resolution;
        Ok(())
    }

    fn restart_renderer(&mut self) {
        if self.video_confirmation.is_some()
            || self.pending_video_confirmation.is_some()
            || self.pending_renderer_restart.is_some()
        {
            self.console_status =
                "VIDEO SETTINGS ARE STILL APPLYING OR AWAITING CONFIRMATION".into();
            self.publish_ui();
            return;
        }
        self.apply_latched_console_cvars(ApplyLatchedScope::VidRestart);
        self.restart_renderer_internal(true);
    }

    /// Reveal the window that a restart hid. Safe to call at any time; it only
    /// acts when a restart is actually holding the window hidden.
    fn show_window_after_restart(&mut self) {
        if !self.window_hidden_for_restart {
            return;
        }
        self.window_hidden_for_restart = false;
        if let Some(window) = &self.window {
            window.set_visible(true);
            window.focus_window();
            window.request_redraw();
        }
    }

    /// True between the start of a renderer/display restart and the moment the
    /// user has kept or reverted the resulting mode.
    fn video_restart_in_flight(&self) -> bool {
        self.pending_renderer_restart.is_some()
            || self.pending_video_confirmation.is_some()
            || self.video_confirmation.is_some()
    }

    fn set_requested_video_mode(&mut self, target: AppliedVideoMode) {
        self.video.fullscreen = target.fullscreen;
        self.video.renderer_backend = target.renderer_backend;
        self.video.resolution = target.resolution;
    }

    fn restart_display_apply_failed(
        &mut self,
        previous: AppliedVideoMode,
        confirm_on_change: bool,
        error: String,
    ) {
        self.renderer_restart_started = None;
        self.show_window_after_restart();
        self.console_status = format!("VID_RESTART DISPLAY APPLY FAILED: {error}");
        self.push_console_line(format!("^1{}", self.console_status));
        if confirm_on_change {
            self.set_requested_video_mode(previous);
            self.push_console_line("^3RESTORING PREVIOUS VIDEO SETTINGS...".to_owned());
            self.restart_renderer_internal(false);
        } else {
            self.publish_ui();
        }
    }

    fn spawn_restarted_renderer(
        &mut self,
        window: Arc<Window>,
        previous: AppliedVideoMode,
        confirm_on_change: bool,
    ) {
        self.perf = PerfStats::default();
        self.supported_msaa = vec![1];
        self.wireframe_supported = false;
        self.reset_egui_for_window(&window);
        let initial = self.render_snapshot();
        let initial_ui = self.ui_snapshot();
        match RenderThread::spawn(
            window,
            self.proxy.clone(),
            self.video.renderer_backend,
            initial,
            initial_ui,
            self.base.clone(),
            self.game.clone(),
        ) {
            Ok(render) => {
                self.render = Some(render);
                self.applied_renderer_backend = self.video.renderer_backend;
                self.applied_grass = self.video.grass;
                self.applied_ocean = self.video.ocean;
                self.applied_reflection_quality = self.video.reflection_quality;
                // Only DX12 has a present ceiling, so switching backends changes
                // what the cap resolves to.
                self.set_fps_cap(self.video.fps_cap);
                let applied = self.applied_video_mode();
                if confirm_on_change && applied != previous {
                    // Do not start the safety countdown merely because the render
                    // thread was spawned. Device/surface creation and world upload
                    // are asynchronous; WorldUploaded starts the timer after the
                    // replacement renderer has actually drawn the map.
                    self.pending_video_confirmation = Some(previous);
                    self.set_capture(false);
                    self.console_status = format!(
                        "VIDEO INITIALIZING {}X{} {} / {}...",
                        applied.resolution[0],
                        applied.resolution[1],
                        applied.fullscreen.label(),
                        applied.renderer_backend.label()
                    );
                    self.push_console_line(format!("^5{}", self.console_status));
                } else {
                    self.console_status = format!(
                        "VIDEO APPLIED: {}X{} {} / {}",
                        applied.resolution[0],
                        applied.resolution[1],
                        applied.fullscreen.label(),
                        applied.renderer_backend.label()
                    );
                    self.push_console_line(format!("^2{}", self.console_status));
                    self.mark_config_dirty();
                    self.flush_config();
                }
                self.normalize_video_selection();
                self.publish_ui();
                // GPU resources live entirely on the renderer thread, but the CPU
                // map preparation is backend-independent. Reuse the retained
                // prepared map whenever its feature-preparation options still match.
                // The frontend now owns a passive duel3 world too, so restore it
                // exactly like a gameplay world but never create a local player.
                if self.live_without_world {
                    println!("[VID_RESTART] live session has no local BSP; keeping renderer worldless");
                } else if self.front_end && self.frontend_cinematic.is_some() {
                    if !self.reupload_cached_map() {
                        println!(
                            "[VID_RESTART] frontend map cache unavailable/incompatible; rebuilding cinematic background"
                        );
                        self.request_frontend_background();
                    }
                } else if !self.front_end && !self.reupload_cached_map() {
                    self.preserve_game_state_on_next_map_upload = true;
                    let source = self.initial_source.clone();
                    println!(
                        "[VID_RESTART] prepared map cache unavailable/incompatible; rebuilding CPU map data"
                    );
                    self.request_map(source);
                }
            }
            Err(error) => {
                self.renderer_restart_started = None;
                self.preserve_game_state_on_next_map_upload = false;
                self.show_window_after_restart();
                self.console_status = format!("VID_RESTART FAILED: {error}");
                self.push_console_line(format!("^1{}", self.console_status));
                if confirm_on_change {
                    // A backend or surface restart can fail before the confirmation
                    // dialog can be shown. Restore the last renderer/mode that was
                    // known to work instead of leaving the user on a dead screen.
                    self.set_requested_video_mode(previous);
                    self.push_console_line("^3RESTORING PREVIOUS VIDEO SETTINGS...".to_owned());
                    self.restart_renderer_internal(false);
                } else {
                    self.publish_ui();
                }
            }
        }
    }

    /// Build a fresh OS window for a backend switch, mirroring startup exactly:
    /// hidden, sized to the target mode, fullscreen applied before any surface is
    /// created. The old window is dropped here, which destroys the HWND still
    /// associated with the outgoing backend's presentation path.
    fn recreate_window_for_backend(
        &mut self,
        event_loop: &ActiveEventLoop,
        target: AppliedVideoMode,
    ) -> Option<Arc<Window>> {
        let mut attributes = Window::default_attributes()
            .with_title("DinurdoJK")
            .with_visible(false)
            .with_inner_size(PhysicalSize::new(target.resolution[0], target.resolution[1]));
        if target.fullscreen.is_windowed() {
            if let Some([x, y]) = self.video.window_position {
                attributes = attributes.with_position(PhysicalPosition::new(x, y));
            }
        }
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("[VID_RESTART] window recreation failed: {error}");
                return None;
            }
        };
        if !target.fullscreen.is_windowed() {
            if let Err(error) = Self::apply_fullscreen_mode(
                &window,
                target.fullscreen,
                target.resolution,
                target.renderer_backend,
            ) {
                eprintln!("[VID_RESTART] fullscreen setup failed on new window: {error}");
            }
        } else if self.video.window_maximized {
            window.set_maximized(true);
        }
        self.applied_fullscreen = target.fullscreen;
        self.applied_resolution = target.resolution;
        self.window_hidden_for_restart = true;
        self.window = Some(Arc::clone(&window));
        self.reset_egui_for_window(&window);
        Some(window)
    }

    fn continue_pending_renderer_restart(&mut self, event_loop: &ActiveEventLoop) {
        let Some(mut pending) = self.pending_renderer_restart.take() else {
            return;
        };
        let now = Instant::now();
        if now < pending.ready_at {
            self.pending_renderer_restart = Some(pending);
            return;
        }
        let Some(window) = self.window.clone() else {
            self.renderer_restart_started = None;
            self.console_status = "VID_RESTART: WINDOW DISAPPEARED DURING TRANSITION".into();
            self.push_console_line(format!("^1{}", self.console_status));
            self.publish_ui();
            return;
        };

        // Transient Win32 resize/move messages while leaving fullscreen must not
        // overwrite the requested resolution/backend before we finish the staged
        // transition. Reassert the latched target at each stage.
        self.set_requested_video_mode(pending.target);

        match pending.stage {
            PendingRendererRestartStage::RecreateWindow => {
                drop(window);
                let Some(fresh) = self.recreate_window_for_backend(event_loop, pending.target)
                else {
                    self.restart_display_apply_failed(
                        pending.previous,
                        pending.confirm_on_change,
                        "could not create a window for the new backend".into(),
                    );
                    return;
                };
                self.spawn_restarted_renderer(
                    fresh,
                    pending.previous,
                    pending.confirm_on_change,
                );
            }
            PendingRendererRestartStage::ApplyTargetDisplayMode => {
                if let Err(error) =
                    self.apply_pending_display_settings(&window, pending.target.fullscreen)
                {
                    self.restart_display_apply_failed(
                        pending.previous,
                        pending.confirm_on_change,
                        error,
                    );
                    return;
                }
                pending.stage = PendingRendererRestartStage::SpawnRenderer;
                pending.ready_at = now + Duration::from_millis(16);
                self.pending_renderer_restart = Some(pending);
                println!(
                    "[VID_RESTART] target Win32 display mode applied; waiting one frame before WGPU surface creation"
                );
            }
            PendingRendererRestartStage::SpawnRenderer => {
                println!(
                    "[VID_RESTART] Win32 display transition settled; creating {} renderer",
                    pending.target.renderer_backend.label()
                );
                self.spawn_restarted_renderer(
                    window,
                    pending.previous,
                    pending.confirm_on_change,
                );
            }
        }
    }

    fn restart_renderer_internal(&mut self, confirm_on_change: bool) {
        let previous = self.applied_video_mode();
        let Some(window) = self.window.clone() else {
            self.console_status = "VID_RESTART: WINDOW IS NOT READY".into();
            self.publish_ui();
            return;
        };

        let mut mode = self.video.fullscreen;
        if mode == FullscreenMode::Exclusive
            && self.video.renderer_backend != RendererBackend::Dx12
            && Self::exclusive_video_mode(&window, self.video.resolution).is_none()
        {
            if confirm_on_change {
                self.console_status = format!(
                    "VID_RESTART: NO EXCLUSIVE VIDEO MODE MATCHES {}X{}",
                    self.video.resolution[0], self.video.resolution[1]
                );
                self.push_console_line(format!("^1{}", self.console_status));
                self.publish_ui();
                return;
            }

            // The monitor may have changed while the confirmation timer was
            // active. If the exact old exclusive mode no longer exists, prefer a
            // safe windowed rollback instead of leaving the user stuck.
            self.video.fullscreen = FullscreenMode::Windowed;
            mode = FullscreenMode::Windowed;
            self.push_console_line(
                "^3PREVIOUS EXCLUSIVE MODE IS UNAVAILABLE; REVERTING TO WINDOWED".to_owned(),
            );
        }

        let target = AppliedVideoMode {
            fullscreen: mode,
            renderer_backend: self.video.renderer_backend,
            resolution: self.video.resolution,
        };

        // Preserve the current window placement before leaving windowed mode,
        // but do not overwrite the newly selected pending resolution.
        if self.applied_fullscreen.is_windowed() && !mode.is_windowed() {
            self.video.window_maximized = window.is_maximized();
            if !self.video.window_maximized {
                if let Ok(position) = window.outer_position() {
                    self.video.window_position = Some([position.x, position.y]);
                }
            }
            self.mark_config_dirty();
        }

        let restart_started = Instant::now();
        self.renderer_restart_started = Some(restart_started);

        // flush_config deliberately writes the last known-good restart-sensitive
        // values. The newly requested mode is only persisted after confirmation.
        self.flush_config();
        self.surface_inspector = None;
        self.console_status = format!(
            "VID_RESTART: APPLYING {}X{} {} / {}...",
            target.resolution[0],
            target.resolution[1],
            target.fullscreen.label(),
            target.renderer_backend.label()
        );
        self.push_console_line(format!("^5{}", self.console_status));
        self.publish_ui();

        // Display mode and renderer backend are a single latched transaction.
        // Stop presenting first so a fullscreen-mode switch cannot invalidate a
        // live WGPU surface underneath the render thread.
        if let Some(mut render) = self.render.take() {
            let shutdown_started = Instant::now();
            render.shutdown();
            println!(
                "[VID_RESTART] old renderer shutdown {:.1} ms",
                shutdown_started.elapsed().as_secs_f64() * 1000.0
            );
        }

        if previous.renderer_backend != target.renderer_backend {
            // An HWND keeps the presentation association of the backend that first
            // drew to it, so a Vulkan swapchain created on a window that already
            // hosted a DX12 one renders at full speed into something Windows never
            // shows. Every same-backend transition works, including real exclusive
            // mode switches, and launching straight into either backend works
            // because the window is new. So give the new backend a new window.
            self.pending_renderer_restart = Some(PendingRendererRestart {
                previous,
                target,
                confirm_on_change,
                stage: PendingRendererRestartStage::RecreateWindow,
                ready_at: Instant::now(),
            });
            println!(
                "[VID_RESTART] backend {} -> {}; recreating the OS window",
                previous.renderer_backend.label(),
                target.renderer_backend.label()
            );
            return;
        }

        let previous_native =
            Self::native_fullscreen_mode(previous.fullscreen, previous.renderer_backend);
        let requested_native =
            Self::native_fullscreen_mode(target.fullscreen, target.renderer_backend);
        let display_state_changed = previous_native != requested_native
            || (requested_native != FullscreenMode::Borderless
                && previous.resolution != target.resolution);

        if display_state_changed {
            // Winit's Windows fullscreen transition and WGPU/DXGI/Vulkan surface
            // creation must not happen in one event-loop callback. This was the
            // source of the DX12 -> Vulkan frozen presentation and the invalid
            // DX12 surface on rollback: the new surface could be created while
            // the HWND was still changing native fullscreen state.
            //
            // First release any native fullscreen/borderless state. Then let the
            // Win32 message queue run, apply the requested target mode, let it run
            // once more, and only then create the replacement WGPU surface.
            window.set_fullscreen(None);
            self.applied_fullscreen = FullscreenMode::Windowed;
            let size = window.inner_size();
            if size.width > 0 && size.height > 0 {
                self.applied_resolution = [size.width, size.height];
            }
            self.pending_renderer_restart = Some(PendingRendererRestart {
                previous,
                target,
                confirm_on_change,
                stage: PendingRendererRestartStage::ApplyTargetDisplayMode,
                ready_at: Instant::now() + Duration::from_millis(16),
            });
            println!(
                "[VID_RESTART] released old Win32 fullscreen state; deferring target mode for one frame"
            );
            return;
        }

        // Backend-only switches between identical native window modes can reuse
        // the settled HWND immediately. The old render thread has already joined,
        // so its WGPU surface/device are gone before the replacement is created.
        self.applied_fullscreen = target.fullscreen;
        self.applied_resolution = target.resolution;
        println!("[VID_RESTART] display mode unchanged; skipped Win32 mode transition");
        self.spawn_restarted_renderer(window, previous, confirm_on_change);
    }

    fn change_video_setting(&mut self, direction: i32) {
        match self.video_selected {
            ui::VIDEO_ROW_FULLSCREEN => self.cycle_fullscreen(direction),
            ui::VIDEO_ROW_RESOLUTION => self.cycle_resolution(direction),
            ui::VIDEO_ROW_VSYNC => {
                let index = VsyncMode::ALL
                    .iter()
                    .position(|mode| *mode == self.video.vsync)
                    .unwrap_or(0);
                let next = (index as i32 + direction).rem_euclid(VsyncMode::ALL.len() as i32);
                self.video.vsync = VsyncMode::ALL[next as usize];
                self.render_command(RenderCommand::SetVsync(self.video.vsync));
                self.mark_config_dirty();
                // Entering FAST on DX12 can make the standing cap unreachable.
                self.set_fps_cap(self.video.fps_cap);
            }
            ui::VIDEO_ROW_FPS_CAP => self.begin_fps_cap_edit(),
            ui::VIDEO_ROW_PHYSICS_FPS => self.begin_physics_fps_edit(),
            ui::VIDEO_ROW_BRIGHTNESS => self.set_gamma(self.video.gamma + direction as f32 * 0.05),
            ui::VIDEO_ROW_ANTI_ALIASING => self.cycle_anti_aliasing(direction),
            ui::VIDEO_ROW_TEXTURE_FILTER => self.cycle_texture_filter(direction),
            ui::VIDEO_ROW_PVS => {
                const MODES: [PvsMode; 4] = [
                    PvsMode::Off,
                    PvsMode::Minimal,
                    PvsMode::Full,
                    PvsMode::Auto,
                ];
                let current = MODES
                    .iter()
                    .position(|mode| *mode == self.video.pvs_mode)
                    .unwrap_or(3) as i32;
                let next = (current + direction).rem_euclid(MODES.len() as i32) as usize;
                self.video.pvs_mode = MODES[next];
                self.render_command(RenderCommand::SetPvsMode(self.video.pvs_mode));
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_DISTANCE_CULL => {
                self.set_distance_cull_scale(
                    self.video.distance_cull_scale + direction as f32 * 0.5,
                );
            }
            ui::VIDEO_ROW_GPU_DRIVEN => {
                self.video.gpu_driven = !self.video.gpu_driven;
                self.sync_gpu_visibility();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_HIZ => {
                self.video.hiz_occlusion = !self.video.hiz_occlusion;
                self.sync_gpu_visibility();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_ENTITY_AMBIENT_LIGHTING => {
                let current = EntityAmbientLightingMode::ALL
                    .iter()
                    .position(|mode| *mode == self.video.entity_ambient_lighting)
                    .unwrap_or(0) as i32;
                let next = (current + direction)
                    .rem_euclid(EntityAmbientLightingMode::ALL.len() as i32) as usize;
                self.video.entity_ambient_lighting = EntityAmbientLightingMode::ALL[next];
                self.sync_entity_ambient_lighting();
                self.mark_config_dirty();
                self.console_status = format!(
                    "ENTITY AMBIENT LIGHTING: {}",
                    self.video.entity_ambient_lighting.label()
                );
            }
            ui::VIDEO_ROW_PBR => {
                self.video.pbr = !self.video.pbr;
                self.mark_config_dirty();
                self.console_status = if self.map_prepare_restart_required() {
                    format!(
                        "PHYSICALLY BASED RENDERING (PBR): {} - MATERIAL SOURCE CHANGES ON APPLY / VID_RESTART",
                        if self.video.pbr { "ON" } else { "OFF" }
                    )
                } else {
                    format!(
                        "PHYSICALLY BASED RENDERING (PBR): {}",
                        if self.video.pbr { "ON" } else { "OFF" }
                    )
                };
            }
            ui::VIDEO_ROW_ASSET_OVERRIDES => {
                self.video.allow_asset_overrides = !self.video.allow_asset_overrides;
                self.mark_config_dirty();
                self.console_status = format!(
                    "ALLOW ASSET OVERRIDES: {}{}",
                    if self.video.allow_asset_overrides {
                        "ON"
                    } else {
                        "OFF - RETAIL ASSETS PROTECTED"
                    },
                    if self.map_prepare_restart_required() {
                        " - APPLY VIDEO SETTINGS OR RUN VID_RESTART"
                    } else {
                        ""
                    }
                );
            }
            ui::VIDEO_ROW_GEN_NORMAL_MAPS => {
                self.video.gen_normal_maps = !self.video.gen_normal_maps;
                self.mark_config_dirty();
                if self.map_prepare_restart_required() {
                    self.console_status =
                        "GENERATED NORMAL MAPS: PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART".into();
                }
            }
            ui::VIDEO_ROW_DELUXE_MAPPING => {
                self.video.deluxe_mapping = !self.video.deluxe_mapping;
                self.sync_pbr();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_DELUXE_SPECULAR => {
                self.video.deluxe_specular =
                    (self.video.deluxe_specular + direction as f32 * 0.1).clamp(0.0, 1.0);
                self.sync_pbr();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_DYNAMIC_LIGHTS => {
                let current = DynamicLightsMode::ALL
                    .iter()
                    .position(|mode| *mode == self.video.dynamic_lights)
                    .unwrap_or(0) as i32;
                let next = (current + direction).rem_euclid(DynamicLightsMode::ALL.len() as i32)
                    as usize;
                self.video.dynamic_lights = DynamicLightsMode::ALL[next];
                self.video.clustered_lighting =
                    matches!(self.video.dynamic_lights, DynamicLightsMode::PerPixelForwardPlus);
                self.sync_clustered_lighting();
                self.mark_config_dirty();
                if matches!(
                    self.video.dynamic_lights,
                    DynamicLightsMode::PerVertexLegacy | DynamicLightsMode::RayTracedHardware
                ) {
                    self.console_status = format!(
                        "DYNAMIC LIGHTS: {} - WIP, CURRENTLY BEHAVES AS OFF",
                        self.video.dynamic_lights.label()
                    );
                }
            }
            ui::VIDEO_ROW_MODERN_SABERS => {
                self.video.modern_sabers = !self.video.modern_sabers;
                self.mark_config_dirty();
                self.console_status = format!(
                    "MODERN SABER RENDERING: {}",
                    if self.video.modern_sabers { "ON" } else { "OFF (OPENJK)" }
                );
            }
            ui::VIDEO_ROW_EMISSIVE_AREA_LIGHTS => {
                self.video.emissive_area_lights = !self.video.emissive_area_lights;
                self.sync_emissive_area_lights();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_AMBIENT_OCCLUSION => {
                // Final user-facing AO selector: OFF / REALTIME / BAKED.
                // BAKED always uses the hybrid HQ path at the fixed quality target.
                let mode = if self.video.ssao {
                    1_i32
                } else if self.video.static_bsp_ao {
                    2
                } else {
                    0
                };
                match (mode + direction).rem_euclid(3) {
                    0 => {
                        self.video.ssao = false;
                        self.video.static_bsp_ao = false;
                    }
                    1 => {
                        self.video.ssao = true;
                        self.video.static_bsp_ao = false;
                    }
                    _ => {
                        self.video.ssao = false;
                        self.video.static_bsp_ao = true;
                        self.video.static_bsp_ao_lightmap = true;
                    }
                }
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_VOXEL_PROBE_GI => {
                self.video.voxel_probe_gi = !self.video.voxel_probe_gi;
                self.sync_voxel_probe_gi();
                self.mark_config_dirty();
                if self.map_prepare_restart_required() {
                    self.console_status =
                        "VOXEL / PROBE GI: PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART".into();
                }
            }
            ui::VIDEO_ROW_DYNAMIC_SHADOWS => {
                let current = DynamicShadowsMode::ALL
                    .iter()
                    .position(|mode| *mode == self.video.dynamic_shadows)
                    .unwrap_or(0) as i32;
                let next = (current + direction).rem_euclid(DynamicShadowsMode::ALL.len() as i32)
                    as usize;
                self.video.dynamic_shadows = DynamicShadowsMode::ALL[next];
                self.video.cascaded_shadows = matches!(
                    self.video.dynamic_shadows,
                    DynamicShadowsMode::CascadedShadowMaps | DynamicShadowsMode::CascadedShadowMapsBevy
                );
                self.sync_cascaded_shadows();
                self.mark_config_dirty();
                if matches!(
                    self.video.dynamic_shadows,
                    DynamicShadowsMode::BlobStencilLegacy | DynamicShadowsMode::RayTraced
                ) {
                    self.console_status = format!(
                        "DYNAMIC SHADOWS: {} - WIP, CURRENTLY BEHAVES AS OFF",
                        self.video.dynamic_shadows.label()
                    );
                }
            }
            ui::VIDEO_ROW_LOCAL_LIGHT_SHADOWS => {
                self.video.local_light_shadows = !self.video.local_light_shadows;
                self.sync_local_light_shadows();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_CONTACT_SHADOWS => {
                self.video.contact_shadows = !self.video.contact_shadows;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_SSR => {
                let current = ReflectionQuality::ALL
                    .iter()
                    .position(|quality| *quality == self.video.reflection_quality)
                    .unwrap_or(3) as i32;
                let next = (current + direction).clamp(0, ReflectionQuality::ALL.len() as i32 - 1)
                    as usize;
                self.video.reflection_quality = ReflectionQuality::ALL[next];
                self.mark_config_dirty();
                self.console_status = format!(
                    "REFLECTION QUALITY: {} - PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART",
                    self.video.reflection_quality.label()
                );
            }
            ui::VIDEO_ROW_PLANAR_REFLECTIONS => {
                self.video.reflection_debug = !self.video.reflection_debug;
                self.sync_post_effects();
            }
            ui::VIDEO_ROW_PLANAR_REFLECTION_DEBUG => {
                let current = PlanarReflectionDebugMode::ALL
                    .iter()
                    .position(|mode| *mode == self.video.planar_reflection_debug)
                    .unwrap_or(0) as i32;
                let next = (current + direction)
                    .rem_euclid(PlanarReflectionDebugMode::ALL.len() as i32)
                    as usize;
                self.video.planar_reflection_debug = PlanarReflectionDebugMode::ALL[next];
                self.sync_planar_reflection_debug();
            }
            ui::VIDEO_ROW_HDR => {
                self.video.hdr = !self.video.hdr;
                self.sync_post_effects();
                self.mark_config_dirty();
                if self.map_prepare_restart_required() {
                    self.console_status =
                        "HDR / FLOAT LIGHTMAP FORMAT: PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART".into();
                }
            }
            ui::VIDEO_ROW_FLOAT_LIGHTMAP => {
                self.video.float_lightmap = !self.video.float_lightmap;
                self.mark_config_dirty();
                if self.map_prepare_restart_required() {
                    self.console_status =
                        "FLOAT LIGHTMAPS: PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART".into();
                }
            }
            ui::VIDEO_ROW_TONE_MAPPING => {
                self.video.tone_mapping = !self.video.tone_mapping;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_AUTO_EXPOSURE => {
                self.video.auto_exposure = !self.video.auto_exposure;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_BLOOM => {
                self.video.bloom = !self.video.bloom;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_MOTION_BLUR => self.set_motion_blur_strength(
                self.video.motion_blur_strength + direction as f32 * 0.05,
            ),
            ui::VIDEO_ROW_DEPTH_OF_FIELD => self.set_depth_of_field_strength(
                self.video.depth_of_field_strength + direction as f32 * 0.05,
            ),
            ui::VIDEO_ROW_DOF_QUALITY => self.cycle_dof_quality(direction),
            ui::VIDEO_ROW_HALATION => {
                self.video.halation = !self.video.halation;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_CHROMATIC_ABERRATION => {
                let step = if self.video.chromatic_aberration < 1.0 {
                    0.05
                } else if self.video.chromatic_aberration < 10.0 {
                    0.5
                } else {
                    5.0
                };
                self.set_chromatic_aberration_strength(
                    self.video.chromatic_aberration + direction as f32 * step,
                );
            }
            ui::VIDEO_ROW_VIGNETTE => {
                self.video.vignette = !self.video.vignette;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_FILM_GRAIN => self
                .set_film_grain_strength(self.video.film_grain_strength + direction as f32 * 0.05),
            ui::VIDEO_ROW_COLOR_LUT => {
                let current = ColorLutPreset::ALL
                    .iter()
                    .position(|preset| *preset == self.video.color_lut)
                    .unwrap_or(0) as i32;
                let next =
                    (current + direction).rem_euclid(ColorLutPreset::ALL.len() as i32) as usize;
                self.set_color_lut(ColorLutPreset::ALL[next]);
            }
            ui::VIDEO_ROW_LUT_STRENGTH => {
                self.set_color_lut_strength(self.video.color_lut_strength + direction as f32 * 0.05)
            }
            ui::VIDEO_ROW_WIREFRAME => {
                if self.wireframe_supported {
                    self.video.show_wireframe = !self.video.show_wireframe;
                    self.render_command(RenderCommand::SetWireframe(self.video.show_wireframe));
                    self.mark_config_dirty();
                } else {
                    self.console_status = "WIREFRAME OVERLAY IS NOT SUPPORTED BY THIS GPU".into();
                }
            }
            ui::VIDEO_ROW_CULL_DEBUG => {
                self.video.cull_debug = match self.video.cull_debug {
                    CullDebugMode::Off => CullDebugMode::RejectionReasons,
                    CullDebugMode::RejectionReasons => CullDebugMode::Off,
                };
                self.sync_cull_debug();
            }
            ui::VIDEO_ROW_DRAW_FPS => {
                self.video.draw_fps =
                    ((self.video.draw_fps as i32 + direction).rem_euclid(3)) as u8;
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_DEVELOPER_TOOLS => {
                self.video.developer_tools = !self.video.developer_tools;
                if !self.video.developer_tools {
                    self.surface_inspector = None;
                    self.render_command(RenderCommand::ClearSurfaceInspection);
                }
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_PERF_TRACE => {
                self.video.perf_trace = !self.video.perf_trace;
                self.render_command(RenderCommand::SetPerfTrace(self.video.perf_trace));
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_GPU_TIMINGS => {
                self.video.gpu_timings = !self.video.gpu_timings;
                self.render_command(RenderCommand::SetGpuTimings(self.video.gpu_timings));
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_GHOUL2_SKINNING => {
                let current = Ghoul2SkinningMode::ALL
                    .iter()
                    .position(|mode| *mode == self.video.ghoul2_skinning)
                    .unwrap_or(0) as i32;
                let next = (current + direction)
                    .rem_euclid(Ghoul2SkinningMode::ALL.len() as i32) as usize;
                self.video.ghoul2_skinning = Ghoul2SkinningMode::ALL[next];
                self.mark_config_dirty();
                self.console_status = format!(
                    "GHOUL2 SKINNING: {}",
                    self.video.ghoul2_skinning.label()
                );
            }
            ui::VIDEO_ROW_GHOUL2_LOD_BIAS => {
                self.video.ghoul2_lod_bias =
                    (self.video.ghoul2_lod_bias + direction).clamp(0, 8);
                self.mark_config_dirty();
                self.console_status = format!("MODEL LOD BIAS: {}", self.video.ghoul2_lod_bias);
            }
            ui::VIDEO_ROW_GHOUL2_BATCH_DRAWS => {
                let current = Ghoul2BatchMode::ALL
                    .iter()
                    .position(|mode| *mode == self.video.ghoul2_batch_draws)
                    .unwrap_or(1) as i32;
                let next = (current + direction)
                    .rem_euclid(Ghoul2BatchMode::ALL.len() as i32) as usize;
                self.video.ghoul2_batch_draws = Ghoul2BatchMode::ALL[next];
                self.render_command(RenderCommand::SetGhoul2BatchDraws(
                    self.video.ghoul2_batch_draws,
                ));
                self.mark_config_dirty();
                self.console_status = format!(
                    "GHOUL2 BATCH DRAWS: {}",
                    self.video.ghoul2_batch_draws.label()
                );
            }
            ui::VIDEO_ROW_GHOUL2_EARLY_CULL => {
                self.video.ghoul2_early_cull = !self.video.ghoul2_early_cull;
                self.mark_config_dirty();
                self.console_status = format!(
                    "GHOUL2 EARLY CULL: {}",
                    if self.video.ghoul2_early_cull { "ON" } else { "OFF" }
                );
            }
            ui::VIDEO_ROW_RENDER_BACKEND => self.cycle_renderer_backend(direction),
            ui::VIDEO_ROW_BAKED_AO_SAMPLES => {
                const VALUES: [u32; 5] = [8, 16, 32, 64, 128];
                let current = VALUES.iter().position(|&v| v == self.video.static_bsp_ao_samples).unwrap_or(2) as i32;
                self.video.static_bsp_ao_samples = VALUES[(current + direction).rem_euclid(VALUES.len() as i32) as usize];
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_BAKED_AO_RESOLUTION => {
                // Odd scales keep original lightmap texel centers on the refined lattice,
                // so AO=1 cannot brighten/smooth the authored lightmap like 2x/4x did.
                const VALUES: [u32; 3] = [1, 3, 5];
                let current = VALUES.iter().position(|&v| v == self.video.static_bsp_ao_resolution).unwrap_or(1) as i32;
                self.video.static_bsp_ao_resolution = VALUES[(current + direction).rem_euclid(VALUES.len() as i32) as usize];
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_BAKED_AO_STRENGTH => {
                const VALUES: [u32; 4] = [25, 50, 75, 100];
                let current = VALUES.iter().position(|&v| v == self.video.static_bsp_ao_strength).unwrap_or(2) as i32;
                self.video.static_bsp_ao_strength = VALUES[(current + direction).rem_euclid(VALUES.len() as i32) as usize];
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_BAKED_AO_RANGE => {
                const VALUES: [u32; 4] = [50, 100, 150, 200];
                let current = VALUES.iter().position(|&v| v == self.video.static_bsp_ao_range).unwrap_or(1) as i32;
                self.video.static_bsp_ao_range = VALUES[(current + direction).rem_euclid(VALUES.len() as i32) as usize];
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_BAKED_AO_CURRENT_CELL => {
                self.video.static_bsp_ao_current_cell = !self.video.static_bsp_ao_current_cell;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::VIDEO_ROW_VID_RESTART if self.video_restart_required() => {
                self.restart_renderer();
                return;
            }
            ui::VIDEO_ROW_VID_RESTART => {}
            _ => {}
        }
        self.publish_ui();
    }

    fn set_environment_quality_segment(&mut self, row: usize, target: usize) {
        self.environment_selected = row;
        match row {
            ui::ENV_ROW_CLOUD_RENDER_RESOLUTION => {
                const VALUES: [CloudRenderResolution; 4] = [
                    CloudRenderResolution::Quarter,
                    CloudRenderResolution::Half,
                    CloudRenderResolution::ThreeQuarter,
                    CloudRenderResolution::Full,
                ];
                let Some(value) = VALUES.get(target).copied() else { return; };
                if self.video.cloud_render_resolution != value {
                    self.video.cloud_render_resolution = value;
                    self.sync_post_effects();
                    self.mark_config_dirty();
                }
            }
            ui::ENV_ROW_FOOTPRINTS => {
                let Some(value) = FootprintMode::ALL.get(target).copied() else { return; };
                if self.video.footprints != value {
                    self.video.footprints = value;
                    self.render_command(RenderCommand::SetFootprintMode(self.video.footprints));
                    self.mark_config_dirty();
                }
            }
            _ => return,
        }
        self.publish_ui();
    }

    fn commit_ocean_settings(&mut self) {
        self.video.ocean_settings = self.video.ocean_settings.sanitize();
        self.render_command(RenderCommand::SetOceanSettings(self.video.ocean_settings));
        self.publish_authored_oceans();
        self.mark_config_dirty();
        self.publish_ui();
    }

    fn publish_authored_oceans(&mut self) {
        let mut oceans = self.authored_oceans.clone();
        if self.authored_ocean_preview {
            if let Some(ocean) = oceans.get_mut(self.authored_ocean_selected) {
                ocean.waves = self.video.ocean_settings.authored;
                let weather = ocean.weather;
                for ocean in &mut oceans {
                    if ocean.weather == weather { ocean.wind = self.video.ocean_settings.wind; }
                }
            }
        }
        self.render_command(RenderCommand::SetAuthoredOceans(oceans));
    }

    fn refresh_authored_oceans(&mut self) {
        use crate::ocean::authoring::{AuthoredOcean, CS_OCEANS, CS_WEATHER};
        let mut oceans = if self.game_session.is_none() { self.map_authored_oceans.clone() } else { Vec::new() };
        if let Some(session) = &self.game_session {
            for index in 0..8 {
                let Some(bytes) = session.client_game.configstring(CS_OCEANS + index as u16) else { continue; };
                if let Some(mut ocean) = AuthoredOcean::parse(index, bytes) {
                    if let Some(weather) = session.client_game.configstring(CS_WEATHER + ocean.weather as u16) {
                        ocean.apply_weather(weather);
                    }
                    oceans.push(ocean);
                }
            }
        }
        if oceans != self.authored_oceans {
            self.authored_oceans = oceans;
            self.authored_ocean_selected = self.authored_ocean_selected.min(self.authored_oceans.len().saturating_sub(1));
            if self.authored_oceans.is_empty() { self.authored_ocean_preview = false; }
            self.publish_authored_oceans();
        }
        if let Some(net) = &self.net {
            self.render_command(RenderCommand::SetOceanTime(net.session().server_time() as f32 * 0.001));
        } else if let Some(time) = self.game_session.as_ref().and_then(|s| s.timeline.as_ref()).map(|t| t.target_time(Instant::now())) {
            self.render_command(RenderCommand::SetOceanTime(time as f32 * 0.001));
        }
    }

    fn change_environment_setting(&mut self, direction: i32) {
        match self.environment_selected {
            ui::ENV_ROW_FOG_MODE => {
                self.video.fog_mode = match self.video.fog_mode {
                    FogMode::Off => FogMode::Legacy,
                    FogMode::Legacy => FogMode::Volumetric,
                    FogMode::Volumetric => FogMode::Off,
                };
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_FOG_STRENGTH => {
                self.set_fog_strength(self.video.fog_strength + direction as f32 * 0.5);
            }
            ui::ENV_ROW_CLOUDS => {
                self.video.clouds = !self.video.clouds;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_CLOUD_TYPE => {
                let current = CloudType::ALL
                    .iter()
                    .position(|kind| *kind == self.video.cloud_type)
                    .unwrap_or(0) as i32;
                let next = (current + direction).rem_euclid(CloudType::ALL.len() as i32) as usize;
                self.video.cloud_type = CloudType::ALL[next];
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_CLOUD_QUALITY => {
                self.set_cloud_quality(self.video.cloud_quality + direction as f32 * 0.05)
            }
            ui::ENV_ROW_CLOUD_COVERAGE => {
                self.set_cloud_coverage(self.video.cloud_coverage + direction as f32 * 0.05)
            }
            ui::ENV_ROW_CLOUD_HEIGHT => {
                self.set_cloud_height(self.video.cloud_height + direction as f32 * 128.0)
            }
            ui::ENV_ROW_CLOUD_THICKNESS => {
                self.set_cloud_thickness(self.video.cloud_thickness + direction as f32 * 128.0)
            }
            ui::ENV_ROW_CLOUD_WIND_SPEED => {
                self.set_cloud_wind_speed(self.video.cloud_wind_speed + direction as f32 * 20.0)
            }
            ui::ENV_ROW_CLOUD_WIND_DIRECTION => self.set_cloud_wind_direction(
                self.video.cloud_wind_direction + direction as f32 * 15.0,
            ),
            ui::ENV_ROW_CLOUD_SHADOWS => {
                self.video.cloud_shadows = !self.video.cloud_shadows;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_CLOUD_RENDER_RESOLUTION => {
                let current = CloudRenderResolution::ALL
                    .iter()
                    .position(|mode| *mode == self.video.cloud_render_resolution)
                    .unwrap_or(1) as i32;
                let next = (current + direction).rem_euclid(CloudRenderResolution::ALL.len() as i32)
                    as usize;
                self.video.cloud_render_resolution = CloudRenderResolution::ALL[next];
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_CLOUD_TEMPORAL => {
                self.video.cloud_temporal = !self.video.cloud_temporal;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_RAIN => {
                self.video.rain = !self.video.rain;
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_RAIN_INTENSITY => {
                let current = RainIntensity::ALL
                    .iter()
                    .position(|intensity| *intensity == self.video.rain_intensity)
                    .unwrap_or(1) as i32;
                let next =
                    (current + direction).rem_euclid(RainIntensity::ALL.len() as i32) as usize;
                self.video.rain_intensity = RainIntensity::ALL[next];
                self.sync_post_effects();
                self.mark_config_dirty();
            }
            ui::ENV_ROW_FOOTPRINTS => {
                let current = FootprintMode::ALL
                    .iter()
                    .position(|mode| *mode == self.video.footprints)
                    .unwrap_or(2) as i32;
                let next =
                    (current + direction).rem_euclid(FootprintMode::ALL.len() as i32) as usize;
                self.video.footprints = FootprintMode::ALL[next];
                self.render_command(RenderCommand::SetFootprintMode(self.video.footprints));
                self.mark_config_dirty();
            }
            ui::ENV_ROW_GRASS => {
                self.video.grass = !self.video.grass;
                // The grass GPU/map data is retained while disabled, so OFF is
                // always instant and ON is instant when this renderer started
                // with grass resources available.
                if !self.video.grass || self.applied_grass {
                    self.render_command(RenderCommand::SetGrassEnabled(self.video.grass));
                }
                self.mark_config_dirty();
                self.console_status = if self.video.grass && !self.applied_grass {
                    "PROCEDURAL GRASS: ON (PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART)".into()
                } else {
                    format!(
                        "PROCEDURAL GRASS: {} (APPLIED LIVE)",
                        if self.video.grass { "ON" } else { "OFF" }
                    )
                };
            }
            ui::ENV_ROW_OCEAN => {
                self.video.ocean = !self.video.ocean;
                // OFF releases the FFT resources immediately. If the current
                // world already carries its promoted ocean clipmap, ON can also
                // be restored live without rebuilding the renderer.
                if !self.video.ocean || self.applied_ocean {
                    self.render_command(RenderCommand::SetOceanEnabled(self.video.ocean));
                }
                self.mark_config_dirty();
                self.console_status = if self.video.ocean && !self.applied_ocean {
                    "GODOT OCEAN WAVES: ON (PRESS APPLY VIDEO SETTINGS OR RUN VID_RESTART)".into()
                } else {
                    format!(
                        "GODOT OCEAN WAVES: {} (APPLIED LIVE)",
                        if self.video.ocean { "ON" } else { "OFF" }
                    )
                };
            }
            ui::ENV_ROW_CLOUD_TUNING => {
                self.cloud_tuning_open = true;
                self.cloud_tuning_selected = 0;
                self.publish_ui();
            }
            ui::ENV_ROW_OCEAN_SETTINGS => {
                self.ocean_settings_open = true;
                self.ocean_selected = 0;
            }
            ui::ENV_ROW_VID_RESTART if self.video_restart_required() => {
                self.restart_renderer();
                return;
            }
            _ => {}
        }
        self.publish_ui();
    }

    fn console_prev_boundary(input: &str, cursor: usize) -> usize {
        let cursor = cursor.min(input.len());
        input[..cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn console_next_boundary(input: &str, cursor: usize) -> usize {
        let cursor = cursor.min(input.len());
        input[cursor..]
            .chars()
            .next()
            .map_or(cursor, |ch| cursor + ch.len_utf8())
    }

    fn console_word_left(input: &str, cursor: usize) -> usize {
        let mut cursor = cursor.min(input.len());
        while cursor > 0 {
            let previous = Self::console_prev_boundary(input, cursor);
            let ch = input[previous..cursor].chars().next().unwrap();
            if !ch.is_whitespace() {
                break;
            }
            cursor = previous;
        }
        while cursor > 0 {
            let previous = Self::console_prev_boundary(input, cursor);
            let ch = input[previous..cursor].chars().next().unwrap();
            if ch.is_whitespace() {
                break;
            }
            cursor = previous;
        }
        cursor
    }

    fn console_word_right(input: &str, cursor: usize) -> usize {
        let mut cursor = cursor.min(input.len());
        while cursor < input.len() {
            let next = Self::console_next_boundary(input, cursor);
            let ch = input[cursor..next].chars().next().unwrap();
            if ch.is_whitespace() {
                break;
            }
            cursor = next;
        }
        while cursor < input.len() {
            let next = Self::console_next_boundary(input, cursor);
            let ch = input[cursor..next].chars().next().unwrap();
            if !ch.is_whitespace() {
                break;
            }
            cursor = next;
        }
        cursor
    }

    fn insert_console_text(&mut self, value: &str) -> bool {
        self.console_cursor = self.console_cursor.min(self.console_input.len());
        let before = self.console_input.len();
        for ch in value.chars() {
            if ch.is_control() || matches!(ch, '`' | '~') {
                continue;
            }
            if self.console_input.len() + ch.len_utf8() > 160 {
                break;
            }
            self.console_input.insert(self.console_cursor, ch);
            self.console_cursor += ch.len_utf8();
        }
        self.console_input.len() != before
    }

    fn handle_console_key(&mut self, event: &KeyEvent, code: KeyCode) {
        if event.state != ElementState::Pressed {
            return;
        }

        if self.modifiers.control_key() && code == KeyCode::KeyF && !event.repeat {
            self.open_console_search();
            return;
        }

        if self.console_search_open {
            if self.modifiers.control_key() && code == KeyCode::KeyV && !event.repeat {
                match Self::paste_into(&mut self.console_search_input, 128, false) {
                    Ok(true) => self.rebuild_console_search(true),
                    Ok(false) => {}
                    Err(error) => {
                        self.console_status = format!("CLIPBOARD ERROR: {error}");
                        self.push_console_line(format!("^1{}", self.console_status));
                    }
                }
                self.publish_ui();
                return;
            }

            match code {
                KeyCode::Escape => {
                    self.console_search_open = false;
                    self.publish_ui();
                }
                KeyCode::Enter if !event.repeat => {
                    self.step_console_search(self.modifiers.shift_key());
                    self.publish_ui();
                }
                KeyCode::F3 if !event.repeat => {
                    self.step_console_search(self.modifiers.shift_key());
                    self.publish_ui();
                }
                KeyCode::Backspace => {
                    self.console_search_input.pop();
                    self.rebuild_console_search(true);
                    self.publish_ui();
                }
                _ => {
                    if !self.modifiers.control_key() {
                        if let Some(value) = &event.text {
                            let mut changed = false;
                            for ch in value.chars() {
                                if !ch.is_control() && self.console_search_input.len() < 128 {
                                    self.console_search_input.push(ch);
                                    changed = true;
                                }
                            }
                            if changed {
                                self.rebuild_console_search(true);
                                self.publish_ui();
                            }
                        }
                    }
                }
            }
            return;
        }

        if self.modifiers.control_key() && !event.repeat {
            match code {
                KeyCode::KeyA => {
                    self.select_all_console();
                    self.publish_ui();
                    return;
                }
                KeyCode::KeyC => {
                    if let Some(text) = self.selected_console_text() {
                        if let Err(error) = crate::clipboard::set_text(&text) {
                            self.console_status = format!("CLIPBOARD ERROR: {error}");
                            self.push_console_line(format!("^1{}", self.console_status));
                        }
                    }
                    self.publish_ui();
                    return;
                }
                KeyCode::KeyV => {
                    match crate::clipboard::get_text() {
                        Ok(value) => {
                            if self.insert_console_text(&value) {
                                self.console_history_index = None;
                                self.console_scroll = 0;
                            }
                        }
                        Err(error) => {
                            self.console_status = format!("CLIPBOARD ERROR: {error}");
                            self.push_console_line(format!("^1{}", self.console_status));
                        }
                    }
                    self.publish_ui();
                    return;
                }
                _ => {}
            }
        }
        match code {
            KeyCode::Escape => self.set_overlay(self.overlay_after_console()),
            KeyCode::Enter if !event.repeat => {
                self.execute_console();
                self.publish_ui();
            }
            KeyCode::Tab if !event.repeat => self.complete_console_input(),
            KeyCode::ArrowUp => self.browse_console_history(-1),
            KeyCode::ArrowDown => self.browse_console_history(1),
            KeyCode::ArrowLeft => {
                self.console_cursor = if self.modifiers.control_key() {
                    Self::console_word_left(&self.console_input, self.console_cursor)
                } else {
                    Self::console_prev_boundary(&self.console_input, self.console_cursor)
                };
                self.publish_ui();
            }
            KeyCode::ArrowRight => {
                self.console_cursor = if self.modifiers.control_key() {
                    Self::console_word_right(&self.console_input, self.console_cursor)
                } else {
                    Self::console_next_boundary(&self.console_input, self.console_cursor)
                };
                self.publish_ui();
            }
            KeyCode::Home => {
                self.console_cursor = 0;
                self.publish_ui();
            }
            KeyCode::End => {
                self.console_cursor = self.console_input.len();
                self.publish_ui();
            }
            KeyCode::PageUp => {
                self.scroll_console(8);
                self.publish_ui();
            }
            KeyCode::PageDown => {
                self.console_scroll = self.console_scroll.saturating_sub(8);
                self.publish_ui();
            }
            KeyCode::Backspace => {
                self.console_history_index = None;
                if self.console_cursor > 0 {
                    let previous = Self::console_prev_boundary(&self.console_input, self.console_cursor);
                    self.console_input.drain(previous..self.console_cursor);
                    self.console_cursor = previous;
                }
                self.publish_ui();
            }
            KeyCode::Delete => {
                self.console_history_index = None;
                if self.console_cursor < self.console_input.len() {
                    let next = Self::console_next_boundary(&self.console_input, self.console_cursor);
                    self.console_input.drain(self.console_cursor..next);
                }
                self.publish_ui();
            }
            _ => {
                if let Some(value) = &event.text {
                    if self.insert_console_text(value) {
                        self.console_history_index = None;
                        self.console_scroll = 0;
                        self.publish_ui();
                    }
                }
            }
        }
    }

    fn handle_chat_key(&mut self, event: &KeyEvent, code: KeyCode) {
        if event.state != ElementState::Pressed {
            return;
        }
        if self.modifiers.control_key() && code == KeyCode::KeyV && !event.repeat {
            if let Err(error) = Self::paste_into(&mut self.chat_input, 160, false) {
                self.console_status = format!("CLIPBOARD ERROR: {error}");
                self.push_console_line(format!("^1{}", self.console_status));
            }
            self.publish_ui();
            return;
        }
        match code {
            KeyCode::Escape => {
                self.chat_input.clear();
                self.set_overlay(OverlayMode::None);
            }
            KeyCode::Enter if !event.repeat => self.submit_chat(),
            KeyCode::Backspace => {
                self.chat_input.pop();
                self.publish_ui();
            }
            _ => {
                if self.modifiers.control_key() {
                    return;
                }
                if let Some(value) = &event.text {
                    let mut changed = false;
                    for ch in value.chars() {
                        if !ch.is_control() && self.chat_input.len() < 160 {
                            self.chat_input.push(ch);
                            changed = true;
                        }
                    }
                    if changed {
                        self.publish_ui();
                    }
                }
            }
        }
    }

    /// Keyboard for Setup. egui owns every control on the page, so the only
    /// key left here is the one that backs out to the Game menu.
    fn handle_video_key(&mut self, event: &KeyEvent, code: KeyCode) {
        if event.state != ElementState::Pressed || code != KeyCode::Escape {
            return;
        }
        if self.front_end {
            self.frontend_back();
        } else {
            self.menu_selected = 4;
            self.set_overlay(OverlayMode::Game);
        }
    }

    fn tick_frontend_cinematic(&mut self, now: Instant) {
        if !self.front_end || self.loading.is_some() {
            return;
        }
        let Some(cinematic) = self.frontend_cinematic else {
            return;
        };

        // Keep the camera safely at a real duel spawn and animate only the view.
        // This gives the menu a living 3D background without pathing a camera
        // through BSP walls or requiring gameplay collision/input.
        let seconds = now.saturating_duration_since(cinematic.started).as_secs_f32();
        self.camera.position = glam::Vec3::from_array(cinematic.position);
        self.camera.position.y += (seconds * 0.32).sin() * 0.65;
        self.camera.yaw = cinematic.yaw + (seconds * 0.115).sin() * 0.28;
        self.camera.pitch = (-2.0_f32).to_radians() + (seconds * 0.09).sin() * 0.025;
    }

    fn tick(&mut self) {
        let console_changed = self.sync_console_log();
        if console_changed && self.overlay == OverlayMode::Console {
            self.publish_ui();
        }
        let commands_executed = self.process_console_command_buffer();
        if commands_executed && self.overlay == OverlayMode::Console {
            self.publish_ui();
        }
        let _activity = crate::thread_activity::activity(
            crate::thread_activity::ThreadSlot::Main,
            crate::thread_activity::Task::MainTick,
        );
        let now = Instant::now();
        let dt = now.duration_since(self.previous_tick);
        self.previous_tick = now;

        if let Some(confirmation) = self.video_confirmation {
            let remaining = confirmation.deadline.saturating_duration_since(now);
            let seconds = (remaining.as_secs() + if remaining.subsec_nanos() != 0 { 1 } else { 0 })
                .min(u32::MAX as u64) as u32;
            if seconds == 0 {
                self.revert_video_settings("NOT CONFIRMED IN TIME");
                return;
            }
            if seconds != confirmation.shown_seconds {
                if let Some(active) = &mut self.video_confirmation {
                    active.shown_seconds = seconds;
                }
                self.publish_ui();
            }
        }

        self.tick_network(dt);
        let demo_is_playing = self
            .game_session
            .as_ref()
            .is_some_and(|playback| playback.phase == SessionPhase::Playing);
        if demo_is_playing {
            match self.tick_demo_playback(now) {
                Ok(DemoAdvance::Running) => {}
                Ok(DemoAdvance::Completed) => {
                    let summary = self.game_session.as_ref().map(|playback| {
                        format!(
                            "DEMO COMPLETE: {} messages, {} snapshots",
                            playback.messages, playback.snapshots
                        )
                    });
                    if let Some(summary) = summary {
                        println!("{summary}");
                        self.push_console_line(format!("^2{summary}"));
                    }
                    self.disconnect_to_main_menu();
                    return;
                }
                Err(error) => {
                    self.console_status = error;
                    self.push_console_line(format!("^1{}", self.console_status));
                    eprintln!("{}", self.console_status);
                    if self.net.is_some() {
                        // ERR_DROP: a CGame failure ends the live session too.
                        self.disconnect_to_main_menu();
                        return;
                    }
                    self.game_session = None;
                    self.set_overlay(OverlayMode::Game);
                    self.publish_ui();
                }
            }
            self.drain_cgame_notices();
        }

        self.refresh_authored_oceans();
        let mut deformation_stamps = Vec::new();
        if !demo_is_playing && self.overlay == OverlayMode::None && self.captured {
            if let Some(player) = &mut self.local_player {
                player.set_mouse_input_settings(self.mouse_input);
                let subframe = self.video.input_subframe;
                let mouse_input = if subframe {
                    None
                } else {
                    self.pending_mouse_input.take()
                };
                let simulation_at = Instant::now();
                let mouse_delta = if subframe {
                    (0.0, 0.0)
                } else {
                    self.mouse_delta
                };
                let result = player.update(
                    dt,
                    &self.movement_keys,
                    mouse_delta,
                    self.noclip_primary_down,
                    self.noclip_alt_down,
                );
                deformation_stamps = player.take_deformation_stamps();
                if let Some((sequence, event_at)) = mouse_input {
                    self.last_simulated_mouse_input = Some(InputLatencySample {
                        sequence,
                        event_at,
                        simulation_at,
                    });
                }
                if let Err(error) = result {
                    self.console_status = format!("MOVEMENT ERROR: {error}");
                    self.set_overlay(OverlayMode::Game);
                }
            }
        }
        if !demo_is_playing {
            self.update_solo_player_view_and_presentation();
            self.tick_frontend_cinematic(now);
            if self.video.depth_of_field_strength > 0.001 {
                if let Some(player) = &mut self.local_player {
                    self.dof_focus_target = player.autofocus_distance(&self.camera, 16384.0);
                }
            }
        }
        for stamp in deformation_stamps {
            crate::surface_deformation::trace_stamp(stamp);
            self.render_command(RenderCommand::AddSurfaceDeformation(stamp));
        }
        self.mouse_delta = (0.0, 0.0);
        self.publish_snapshot();

        let current_hud = self.current_hud_state();
        let hud_changed = current_hud != self.last_hud_state;
        if hud_changed {
            self.last_hud_state = current_hud;
        }

        let refresh_chat = !self.chat_lines.is_empty()
            && now.duration_since(self.last_chat_refresh) >= Duration::from_millis(250);
        if refresh_chat {
            self.chat_lines.retain(|line| {
                now.saturating_duration_since(line.created) < Duration::from_secs(10)
            });
            self.last_chat_refresh = now;
        }
        let refresh_center = self.center_print.is_some()
            && now.duration_since(self.last_center_refresh) >= Duration::from_millis(50);
        if refresh_center {
            if self.center_print.as_ref().is_some_and(|print| {
                now.saturating_duration_since(print.created) >= Duration::from_secs(3)
            }) {
                self.center_print = None;
            }
            self.last_center_refresh = now;
        }
        let movement_hud_active = self.movement_keys_hud.mode != 0
            || self.strafe_helper.flags & ui::SHELPER_STYLE_MASK != 0;
        if hud_changed || refresh_chat || refresh_center || movement_hud_active {
            // Strafehelper is velocity/view-angle driven and therefore needs a
            // fresh retained HUD snapshot while enabled. Keep this cost fully
            // gated off for the default HUD.
            self.publish_ui();
        }

        self.tick_egui_menu();

        if self.config_dirty && self.last_config_write.elapsed() >= Duration::from_millis(250) {
            self.flush_config();
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // Keep the native window hidden until wgpu has successfully presented the
        // splash/loading UI once. Otherwise Windows exposes the unpainted swapchain
        // area as a white rectangle while the renderer is still building pipelines.
        let mut attributes = Window::default_attributes()
            .with_title("DinurdoJK")
            .with_visible(false)
            .with_inner_size(PhysicalSize::new(
                self.video.resolution[0],
                self.video.resolution[1],
            ));
        if let Some([x, y]) = self.video.window_position {
            attributes = attributes.with_position(PhysicalPosition::new(x, y));
        }
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("Window creation failed: {error}");
                event_loop.exit();
                return;
            }
        };
        if !self.video.fullscreen.is_windowed() {
            if let Err(error) = Self::apply_fullscreen_mode(
                &window,
                self.video.fullscreen,
                self.video.resolution,
                self.video.renderer_backend,
            ) {
                eprintln!("Fullscreen setup failed: {error}; falling back to windowed mode");
                self.video.fullscreen = FullscreenMode::Windowed;
            }
        }
        if self.video.fullscreen.is_windowed() && self.video.window_maximized {
            window.set_maximized(true);
        }
        self.applied_fullscreen = self.video.fullscreen;
        self.applied_renderer_backend = self.video.renderer_backend;
        self.applied_resolution = self.video.resolution;
        self.applied_grass = self.video.grass;
        self.applied_ocean = self.video.ocean;
        self.applied_reflection_quality = self.video.reflection_quality;
        self.reset_egui_for_window(&window);
        let initial = self.render_snapshot();
        let initial_ui = self.ui_snapshot();
        let render = match RenderThread::spawn(
            window.clone(),
            self.proxy.clone(),
            self.video.renderer_backend,
            initial,
            initial_ui,
            self.base.clone(),
            self.game.clone(),
        ) {
            Ok(render) => render,
            Err(error) => {
                eprintln!("{error}");
                event_loop.exit();
                return;
            }
        };
        self.window = Some(window);
        self.render = Some(render);
        self.previous_tick = Instant::now();
        // A config written before this rule, or by hand, can still say 0. Resolve
        // it now that the window exists and the monitor refresh rate is readable.
        self.set_fps_cap(self.video.fps_cap);
        // The initial map request is intentionally deferred until RendererFirstFrame.
        // That gives the startup splash a real presented frame and prevents the fast
        // map worker from finishing invisibly while the renderer is still initializing.
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        let _activity = crate::thread_activity::activity(
            crate::thread_activity::ThreadSlot::Main,
            crate::thread_activity::Task::Event,
        );
        match event {
            UserEvent::ConsoleCommand(command) => {
                // Route terminal input through the exact in-game console parser while
                // preserving any partially typed line in the graphical console.
                let pending_input = std::mem::replace(&mut self.console_input, command);
                let pending_cursor = std::mem::replace(&mut self.console_cursor, 0);
                self.console_cursor = self.console_input.len();
                let pending_history_index = self.console_history_index;
                self.execute_console();
                self.console_input = pending_input;
                self.console_cursor = pending_cursor.min(self.console_input.len());
                self.console_history_index = pending_history_index;
                self.publish_ui();
            }
            UserEvent::ScreenshotFinished(result) => {
                match result {
                    Ok((path, None)) => {
                        self.console_status =
                            format!("WROTE {} + COPIED TO CLIPBOARD", path.display());
                        self.push_console_line(format!("^2{}", self.console_status));
                    }
                    Ok((path, Some(clipboard_error))) => {
                        self.console_status = format!(
                            "WROTE {} (CLIPBOARD FAILED: {clipboard_error})",
                            path.display()
                        );
                        self.push_console_line(format!("^3{}", self.console_status));
                    }
                    Err(error) => {
                        self.console_status = format!("SCREENSHOT FAILED: {error}");
                        self.push_console_line(format!("^1{}", self.console_status));
                    }
                }
                self.publish_ui();
            }
            UserEvent::RendererReady {
                supported_msaa,
                wireframe_supported,
            } => {
                self.supported_msaa = supported_msaa;
                self.wireframe_supported = wireframe_supported;
                if !self.supported_msaa.contains(&self.video.msaa_samples) {
                    self.video.msaa_samples = 1;
                    self.mark_config_dirty();
                }
                if !self.wireframe_supported && self.video.show_wireframe {
                    self.video.show_wireframe = false;
                    self.mark_config_dirty();
                }
                self.console_status = format!(
                    "RENDER THREAD READY | MSAA {:?} | WIREFRAME {}",
                    self.supported_msaa,
                    if self.wireframe_supported {
                        "SUPPORTED"
                    } else {
                        "UNAVAILABLE"
                    }
                );
                self.push_console_line(format!("^2{}", self.console_status));
                self.publish_ui();
            }
            UserEvent::RendererFirstFrame => {
                // A restarted renderer has now presented, so the window is safe to
                // reveal with a fully formed swapchain behind it.
                self.show_window_after_restart();
                if self.startup_first_frame_seen {
                    if self.front_end || self.live_without_world {
                        if let Some(restart_started) = self.renderer_restart_started.take() {
                            let total_ms = restart_started.elapsed().as_secs_f64() * 1000.0;
                            println!(
                                "[VID_RESTART] backend/display restart to first {} frame {:.1} ms",
                                if self.front_end { "menu" } else { "worldless live" },
                                total_ms
                            );
                        }
                        self.begin_pending_video_confirmation();
                        self.publish_ui();
                    }
                    return;
                }
                self.startup_first_frame_seen = true;
                if let Some(window) = &self.window {
                    window.set_visible(true);
                    window.request_redraw();
                }
                if self.front_end {
                    self.console_status = "MAIN MENU".into();
                    self.publish_ui();
                    self.request_frontend_background();
                } else {
                    let initial_source = self.initial_source.clone();
                    self.request_map(initial_source);
                }
                for command in std::mem::take(&mut self.startup_commands) {
                    println!("STARTUP COMMAND: {command}");
                    self.push_console_line(format!("^7] {command}"));
                    self.append_console_commands(split_console_script(&command));
                }
            }
            UserEvent::RenderStats(stats) => {
                self.video.msaa_samples = stats.msaa_samples;
                self.video.vsync = stats.vsync;
                let client = self
                    .game_session
                    .as_ref()
                    .map_or_else(ClientFramePerf::default, |playback| playback.client_perf);
                self.perf = PerfStats {
                    fps: stats.fps,
                    frame_ms: stats.frame_ms,
                    cpu_prepare_ms: stats.cpu_prepare_ms,
                    cpu_acquire_ms: stats.cpu_acquire_ms,
                    cpu_encode_ms: stats.cpu_encode_ms,
                    cpu_submit_ms: stats.cpu_submit_ms,
                    dynamic_model_prepare_ms: stats.dynamic_model_prepare_ms,
                    dynamic_model_surfaces: stats.dynamic_model_surfaces,
                    dynamic_model_vertices: stats.dynamic_model_vertices,
                    dynamic_model_indices: stats.dynamic_model_indices,
                    client_total_ms: client.total_ms,
                    client_snapshot_ms: client.snapshot_ms,
                    client_audio_ms: client.audio_ms,
                    client_events_ms: client.events_ms,
                    client_entity_present_ms: client.entity_present_ms,
                    client_player_present_ms: client.player_present_ms,
                    client_followed_player_ms: client.followed_player_ms,
                    client_fx_ms: client.fx_ms,
                    client_fx_tessellate_ms: client.fx_tessellate_ms,
                    ghoul2_pose_ms: client.ghoul2_pose_ms,
                    ghoul2_motion_pose_ms: client.ghoul2_motion_pose_ms,
                    ghoul2_skin_ms: client.ghoul2_skin_ms,
                    ghoul2_bolt_ms: client.ghoul2_bolt_ms,
                    ghoul2_pose_evals: client.ghoul2_pose_evals,
                    ghoul2_motion_pose_evals: client.ghoul2_motion_pose_evals,
                    ghoul2_bolt_queries: client.ghoul2_bolt_queries,
                    ghoul2_surfaces: client.ghoul2_surfaces,
                    ghoul2_vertices: client.ghoul2_vertices,
                    ghoul2_frustum_tests: client.ghoul2_frustum_tests,
                    ghoul2_frustum_culled: client.ghoul2_frustum_culled,
                    ghoul2_lod_counts: client.ghoul2_lod_counts,
                    gpu_ms: stats.gpu_ms,
                    gpu_depth_ms: stats.gpu_depth_ms,
                    gpu_hiz_ms: stats.gpu_hiz_ms,
                    gpu_cull_ms: stats.gpu_cull_ms,
                    gpu_cluster_ms: stats.gpu_cluster_ms,
                    gpu_world_ms: stats.gpu_world_ms,
                    gpu_post_ms: stats.gpu_post_ms,
                    gpu_ui_ms: stats.gpu_ui_ms,
                    cull_visible: stats.cull_visible,
                    cull_frustum_rejected: stats.cull_frustum_rejected,
                    cull_hiz_rejected: stats.cull_hiz_rejected,
                    cull_pvs_rejected: stats.cull_pvs_rejected,
                    cull_area_rejected: stats.cull_area_rejected,
                    input_event_to_sim_ms: stats.input_event_to_sim_ms,
                    input_sim_to_render_ms: stats.input_sim_to_render_ms,
                    input_event_to_present_ms: stats.input_event_to_present_ms,
                    input_event_to_present_max_ms: stats.input_event_to_present_max_ms,
                    input_latency_samples: stats.input_latency_samples,
                };
                self.threads = stats.threads.map(|thread| ThreadPerfStats {
                    name: crate::thread_activity::slot_label(thread.slot),
                    task: thread.task.label(),
                    active: thread.active,
                    busy_percent: thread.busy_percent,
                });
                self.publish_ui();
            }
            UserEvent::RendererError(error) => {
                self.renderer_restart_started = None;
                self.show_window_after_restart();
                eprintln!("Renderer: {error}");
                self.console_status = format!("RENDERER ERROR: {error}");
                self.push_console_line(format!("^1{}", self.console_status));

                // A renderer restart can fail asynchronously after RenderThread::spawn
                // has returned (for example a DXGI surface/configuration failure).
                // Do not leave the user staring at a dead renderer until the normal
                // 15-second confirmation timeout expires; restore the last known-good
                // backend/display mode immediately.
                if self.video_confirmation.is_some() || self.pending_video_confirmation.is_some() {
                    self.revert_video_settings("FAILED");
                    return;
                }
                self.publish_ui();
            }
            UserEvent::SurfaceInspected(info) => {
                self.surface_inspector = info;
                self.publish_ui();
            }
            UserEvent::MapLoadProgress {
                request_id,
                task,
                completed,
                total,
            } => {
                if request_id != self.latest_request_id {
                    return;
                }
                let mut changed = false;
                if let Some(loading) = &mut self.loading {
                    if loading.request_id == request_id {
                        loading.update_worker(task, completed, total);
                        changed = true;
                    }
                }
                if changed {
                    self.publish_ui();
                }
            }
            UserEvent::StaticAoProgress { completed, total } => {
                self.static_ao_progress = if total == 0 || completed >= total {
                    None
                } else {
                    Some((completed, total))
                };
                self.publish_ui();
            }
            UserEvent::SteamAudioBakeProgress {
                request_id,
                map_name,
                progress,
            } => {
                if request_id != self.latest_request_id || !self.audio.steam_audio {
                    return;
                }
                let progress = progress.clamp(0.0, 1.0);
                self.steam_audio_bake_progress = if progress >= 1.0 {
                    None
                } else {
                    Some(progress)
                };
                self.steam_audio_bake_error = None;
                if progress <= f32::EPSILON {
                    println!(
                        "{map_name}: Steam Audio background bake started; the legacy mixer remains authoritative until baked assets are ready"
                    );
                }
                self.publish_ui();
            }
            UserEvent::SteamAudioBakeFinished {
                request_id,
                map_name,
                result,
                elapsed_ms,
            } => {
                if request_id != self.latest_request_id {
                    return;
                }
                self.steam_audio_bake_progress = None;
                match result {
                    Ok(data) => {
                        println!(
                            "{map_name}: Steam Audio background bake finished in {:.1} ms: {} probes / {:.2} MiB / {}",
                            elapsed_ms,
                            data.probe_count,
                            data.serialized_bytes() as f64 / (1024.0 * 1024.0),
                            if data.runtime_validated { "validated" } else { "NOT validated" },
                        );
                        if self.audio.steam_audio {
                            let data = Arc::new(data);
                            self.steam_audio_bake = Some(Arc::clone(&data));
                            self.steam_audio_bake_error = None;
                            if let Some(cache) = &mut self.prepared_map_cache {
                                cache.map.steam_audio_bake = Some(Arc::clone(&data));
                                cache.map.steam_audio_bake_request = None;
                            }
                            if let Some(sound) = self
                                .game_session
                                .as_mut()
                                .and_then(|session| session.sound_presenter.as_mut())
                            {
                                sound.set_steam_audio_map(
                                    self.steam_audio_acoustic_mesh.clone(),
                                    Some(data),
                                );
                            }
                        } else {
                            println!(
                                "{map_name}: Steam Audio bake was cached, but s_steamAudio is now off; runtime attachment skipped"
                            );
                        }
                    }
                    Err(error) => {
                        eprintln!("{map_name}: Steam Audio background bake failed: {error}");
                        if self.audio.steam_audio {
                            self.steam_audio_bake = None;
                            self.steam_audio_bake_error = Some(error);
                        }
                    }
                }
                self.publish_ui();
            }
            UserEvent::MapPrepared {
                request_id,
                started,
                name,
                mut map,
            } => {
                if request_id != self.latest_request_id {
                    return;
                }
                let frontend_background = self.front_end
                    && self.frontend_background_request_id == Some(request_id);
                if let Some(net) = &self.net {
                    use crate::ocean::authoring::{AuthoredOcean, CS_OCEANS, CS_WEATHER};
                    let strings = &net.session().decoder().configstrings;
                    let mut oceans = Vec::new();
                    for index in 0..8 {
                        if let Some(mut ocean) = strings.get(&(CS_OCEANS + index as u16)).and_then(|s| AuthoredOcean::parse(index,s)) {
                            if let Some(weather) = strings.get(&(CS_WEATHER + ocean.weather as u16)) { ocean.apply_weather(weather); }
                            oceans.push(ocean);
                        }
                    }
                    if !oceans.is_empty() {
                        let map = map.as_mut();
                        scene::append_authored_ocean_planes(&oceans, &mut map.vertices, &mut map.batches, &mut map.pvs_batches);
                        map.authored_oceans = oceans;
                    }
                }
                let has_grass = !map.grass_patches.is_empty();
                let has_ocean = self.latest_prepare_options.ocean
                    && prepared_map_has_ocean_surface(map.as_ref());
                let has_acoustics = map.steam_audio_acoustic_mesh.is_some();
                if let Some(loading) = &mut self.loading {
                    if loading.request_id == request_id {
                        loading.set_optional_task_presence(
                            crate::thread_activity::Task::MapGrass,
                            has_grass,
                            true,
                        );
                        loading.set_optional_task_presence(
                            crate::thread_activity::Task::MapOcean,
                            has_ocean,
                            false,
                        );
                        loading.set_optional_task_presence(
                            crate::thread_activity::Task::MapAcoustics,
                            has_acoustics,
                            true,
                        );
                    }
                }
                let cache_started = Instant::now();
                self.prepared_map_cache = Some(PreparedMapCache {
                    label: name.clone(),
                    prepare_options: self.latest_prepare_options,
                    map: Box::new(map.as_ref().clone()),
                });
                println!(
                    "Map restart cache: retained prepared CPU map in {:.1} ms",
                    cache_started.elapsed().as_secs_f64() * 1000.0
                );
                self.surface_inspector = None;
                println!(
                    "{}: {} triangles, {} draw batches, {} textures, {} lightmap pages; source {}",
                    name,
                    map.triangles,
                    map.batches.len(),
                    map.textures.len(),
                    map.lightmap_pages,
                    map.source.display()
                );
                if let Some(distance_cull) = map.distance_cull {
                    println!(
                        "{}: worldspawn distanceCull {:.0} map units -> zFar cap {:.0}",
                        name,
                        distance_cull,
                        crate::camera::far_distance_for_cull(distance_cull),
                    );
                } else {
                    println!(
                        "{}: no worldspawn distanceCull; using JKA default {:.0} -> zFar cap {:.0}",
                        name,
                        crate::camera::DEFAULT_DISTANCE_CULL,
                        crate::camera::far_distance_for_cull(crate::camera::DEFAULT_DISTANCE_CULL),
                    );
                }
                if let Some(stats) = map.map_file_stats {
                    println!(
                        "Source .map: {} entities, {} brushes parsed ({} worldspawn + {} func_group static), {} rendered faces, {} runtime/non-static entity brushes skipped, {} patches skipped, {} degenerate faces, {} invalid brushes skipped",
                        stats.entities,
                        stats.brushes,
                        stats.world_brushes,
                        stats.grouped_world_brushes,
                        stats.rendered_faces,
                        stats.skipped_entity_brushes,
                        stats.patches_skipped,
                        stats.degenerate_faces,
                        stats.skipped_brushes,
                    );
                }
                for warning in &map.warnings {
                    if map.map_file_stats.is_some() {
                        println!("Warning: {warning}");
                    } else {
                        println!("Material: {warning}");
                    }
                }
                self.live_without_world = false;
                self.map_name = name.clone();
                self.triangles = map.triangles;
                self.map_distance_cull = map
                    .distance_cull
                    .unwrap_or(crate::camera::DEFAULT_DISTANCE_CULL);
                self.map_authored_sun = map.sun;
                self.map_authored_oceans = map.authored_oceans.clone();
                self.authored_ocean_preview = false;
                self.refresh_authored_oceans();
                self.map_collision = map.collision.clone();
                self.map_physics_collision = map.physics_collision.clone();
                self.map_movement = map.movement.clone();
                self.steam_audio_acoustic_mesh = map.steam_audio_acoustic_mesh.clone();
                self.steam_audio_bake = map.steam_audio_bake.clone();
                self.steam_audio_bake_progress = None;
                self.steam_audio_bake_error = None;
                let acoustic_mesh = self.steam_audio_acoustic_mesh.clone();
                let bake = self.steam_audio_bake.clone();
                if let Some(session) = self.game_session.as_mut() {
                    if let Err(error) = session
                        .player_presenter
                        .set_physics_map_mesh(&self.map_physics_collision)
                    {
                        eprintln!("RAPIER MAP COLLISION ERROR: {error}");
                    }
                    if let Some(sound) = session.sound_presenter.as_mut() {
                        sound.set_steam_audio_map(acoustic_mesh, bake);
                    }
                }
                self.third_person_camera.reset();
                self.solo_dynamic_models = Arc::new(Vec::new());
                if frontend_background {
                    self.preserve_game_state_on_next_map_upload = false;
                    self.local_player = None;
                    self.spawns = map.spawns.clone();
                    self.spawn_index = 0;
                    if let Some(spawn) = self.spawns.first().copied() {
                        let mut position = spawn.position;
                        position[1] += FRONTEND_CAMERA_EYE_HEIGHT;
                        self.camera = Camera::new_with_fov(
                            position,
                            spawn.yaw,
                            self.camera.cg_fov(),
                        );
                        self.frontend_cinematic = Some(FrontendCinematic {
                            position,
                            yaw: spawn.yaw,
                            started: Instant::now(),
                        });
                    } else {
                        self.frontend_cinematic = None;
                    }
                    self.keys.clear();
                    self.movement_keys.clear();
                    self.mouse_buttons_down.clear();
                    self.noclip_primary_down = false;
                    self.noclip_alt_down = false;
                    self.mouse_delta = (0.0, 0.0);
                    self.last_mouse_motion_at = None;
                    self.previous_tick = Instant::now();
                    self.console_status = format!("MAIN MENU BACKGROUND: {name}");
                    println!("Frontend cinematic: loaded {name} with no local player/session");
                } else if self.preserve_game_state_on_next_map_upload {
                    self.preserve_game_state_on_next_map_upload = false;
                } else {
                    self.spawns = map.spawns.clone();
                    self.spawn_index = 0;
                    if let Some(spawn) = self.spawns.first().copied() {
                        self.camera = Camera::new_with_fov(
                            spawn.position,
                            spawn.yaw,
                            self.camera.cg_fov(),
                        );
                        match LocalPlayer::new(map.movement.clone(), map.collision.clone(), spawn) {
                            Ok(mut player) => {
                                player.set_mouse_input_settings(self.mouse_input);
                                if let Err(error) =
                                    player.set_physics_tick_msec(self.video.physics_msec)
                                {
                                    self.console_status = format!("PHYSICS FPS ERROR: {error}");
                                }
                                player.camera(&mut self.camera);
                                self.dof_focus_target =
                                    player.autofocus_distance(&self.camera, 16384.0);
                                self.local_player = Some(player);
                            }
                            Err(error) => {
                                self.local_player = None;
                                eprintln!("Player setup: {error}");
                            }
                        }
                        self.keys.clear();
                        self.movement_keys.clear();
                        self.mouse_buttons_down.clear();
                        self.noclip_primary_down = false;
                        self.noclip_alt_down = false;
                        self.mouse_delta = (0.0, 0.0);
                        self.last_mouse_motion_at = None;
                        self.previous_tick = Instant::now();
                    }
                }
                if self.pending_frontend_map_launch {
                    self.pending_frontend_map_launch = false;
                    self.frontend_background_request_id = None;
                    self.frontend_cinematic = None;
                    self.front_end = false;
                    self.frontend_page = FrontendPage::Main;
                    self.menu_selected = 0;
                    self.set_overlay(OverlayMode::None);
                    self.sync_render_fps_cap();
                }
                self.apply_distance_cull();
                self.console_status = format!("UPLOADING {name} TO RENDER THREAD...");
                if let Some(loading) = &mut self.loading {
                    if loading.request_id == request_id {
                        loading.begin_upload();
                    }
                }
                self.publish_ui();
                self.render_command(RenderCommand::LoadMap {
                    request_id,
                    started,
                    name,
                    map,
                });
                self.publish_snapshot();
            }
            UserEvent::MapFailed {
                request_id,
                started,
                name,
                error,
            } => {
                if request_id != self.latest_request_id {
                    return;
                }
                self.pending_frontend_map_launch = false;
                self.preserve_game_state_on_next_map_upload = false;
                self.renderer_restart_started = None;
                eprintln!(
                    "Map load failed for {name} after {:.1} ms: {error}",
                    started.elapsed().as_secs_f64() * 1000.0
                );
                self.loading = None;

                if self.front_end && self.frontend_background_request_id == Some(request_id) {
                    self.frontend_background_request_id = None;
                    self.frontend_cinematic = None;
                    self.console_status =
                        format!("MAIN MENU - 3D BACKGROUND UNAVAILABLE: {error}");
                    self.push_console_line(format!(
                        "^3Frontend background {name} could not be loaded; keeping the menu usable without it."
                    ));
                    self.publish_ui();
                    return;
                }

                let live_map = self
                    .game_session
                    .as_ref()
                    .filter(|session| session.live)
                    .and_then(|session| session.map_name.clone());
                if let Some(map_name) = live_map {
                    if self.network.allow_missing_map {
                        self.prime_live_session_without_world(&map_name, &error);
                    } else {
                        self.console_status = format!("MAP LOAD FAILED: {error}");
                        self.push_console_line(format!("^1{}", self.console_status));
                        self.push_console_line(
                            "^3Live session closed cleanly; set cl_allowMissingMap 1 to continue without the BSP."
                                .to_owned(),
                        );
                        self.disconnect_to_main_menu();
                    }
                    return;
                }

                self.game_session = None;
                self.console_status = format!("MAP LOAD FAILED: {error}");
                self.push_console_line(format!("^1{}", self.console_status));
                if self.video_confirmation.is_some() || self.pending_video_confirmation.is_some() {
                    self.revert_video_settings("FAILED");
                    return;
                }
                self.publish_ui();
            }
            UserEvent::WorldUploaded {
                request_id,
                name,
                triangles,
                batches,
                upload_ms,
                upload_timings,
                first_frame_ms,
                timings,
                bsp_stats,
            } => {
                if request_id != self.latest_request_id {
                    return;
                }
                if self.frontend_background_request_id == Some(request_id) {
                    self.frontend_background_request_id = None;
                }
                let join_first_world = self.live_join_timing.is_some()
                    && (self
                        .live_join_timing
                        .as_ref()
                        .and_then(|timing| timing.prefetched_map.as_deref())
                        .is_some_and(|map| Self::normalized_bsp_name(map) == Self::normalized_bsp_name(&name))
                        || self.game_session.as_ref().is_some_and(|session| {
                            session.live
                                && session.map_name.as_deref().is_some_and(|map| {
                                    Self::normalized_bsp_name(map) == Self::normalized_bsp_name(&name)
                                })
                        }));
                if join_first_world {
                    let elapsed = self.live_join_timing.as_ref().map(LiveJoinTiming::elapsed_ms).unwrap_or(0.0);
                    if let Some(timing) = self.live_join_timing.as_mut() {
                        timing.first_world_frame_ms = Some(elapsed);
                    }
                    println!("[JOIN] connect -> first rendered world frame {elapsed:.1} ms ({name})");
                    self.push_console_line(format!(
                        "^5[JOIN]^7 connect -> first rendered world frame {elapsed:.1} ms"
                    ));
                }
                if let Some(restart_started) = self.renderer_restart_started.take() {
                    let total_ms = restart_started.elapsed().as_secs_f64() * 1000.0;
                    println!("[VID_RESTART] backend/display restart to first world frame {:.1} ms", total_ms);
                    self.push_console_line(format!(
                        "^5[VID_RESTART]^7 total {:.1} ms to first world frame",
                        total_ms
                    ));
                }
                self.push_console_line(format!(
                    "^5[MAP LOAD]^7 {name} {:.1} ms first-frame | prep {:.1} | bsp {:.1} | collision {:.1} | shaders {:.1} | textures decode {:.1} | geometry {:.1} | render-upload {:.1} | workers {}",
                    first_frame_ms,
                    timings.prepare_wall_ms,
                    timings.bsp_parse_ms,
                    timings.collision_ms,
                    timings.shader_parse_wall_ms,
                    timings.texture_decode_ms,
                    timings.geometry_ms,
                    upload_ms,
                    timings.worker_count,
                ));
                let post_upload_wait_ms =
                    (first_frame_ms - timings.prepare_wall_ms - upload_ms).max(0.0);
                self.push_console_line(format!(
                    "^6[MAP FIRST FRAME]^7 prep {:.1} ms | render upload {:.1} ms | queue/present wait {:.1} ms",
                    timings.prepare_wall_ms,
                    upload_ms,
                    post_upload_wait_ms,
                ));
                self.push_console_line(format!(
                    "^6[MAP PREP DETAIL]^7 grass wall {:.1} ms | grass worker CPU {:.1} ms | gi {:.1} ms | ocean mesh {:.1} ms | acoustics {:.1} ms",
                    timings.grass_ms,
                    timings.grass_cpu_ms,
                    timings.gi_ms,
                    timings.ocean_ms,
                    timings.steam_audio_ms,
                ));
                self.push_console_line(format!(
                    "^6[OCEAN MESH]^7 promoted water surfaces {} | clipmap {} verts / {} tris, inner cell {:.0}u, outer cell {:.0}u",
                    upload_timings.ocean_water_batches,
                    upload_timings.ocean_clipmap_vertices,
                    upload_timings.ocean_clipmap_tris,
                    upload_timings.ocean_clipmap_spacing,
                    upload_timings.ocean_coarsest_spacing,
                ));
                let upload_accounted_ms = upload_timings.vertex_buffer_ms
                    + upload_timings.texture_upload_ms
                    + upload_timings.lightmap_upload_ms
                    + upload_timings.reflection_probe_upload_ms
                    + upload_timings.batch_bind_groups_ms
                    + upload_timings.cull_resources_ms
                    + upload_timings.lighting_resources_ms
                    + upload_timings.pipeline_create_ms
                    + upload_timings.visibility_tables_ms
                    + upload_timings.grass_upload_ms
                    + upload_timings.finalize_ms;
                let upload_other_ms = (upload_ms - upload_accounted_ms).max(0.0);
                self.push_console_line(format!(
                    "^6[MAP RENDER-UPLOAD]^7 vertex {:.1} | textures {:.1} | lightmaps {:.1} | probes {:.1} | batches {:.1} | cull {:.1} | lighting {:.1} | pipelines {:.1} | visibility {:.1} | grass {:.1} | finalize {:.1} | other {:.1}",
                    upload_timings.vertex_buffer_ms,
                    upload_timings.texture_upload_ms,
                    upload_timings.lightmap_upload_ms,
                    upload_timings.reflection_probe_upload_ms,
                    upload_timings.batch_bind_groups_ms,
                    upload_timings.cull_resources_ms,
                    upload_timings.lighting_resources_ms,
                    upload_timings.pipeline_create_ms,
                    upload_timings.visibility_tables_ms,
                    upload_timings.grass_upload_ms,
                    upload_timings.finalize_ms,
                    upload_other_ms,
                ));
                if let (Some(stats), Some(sound)) = (
                    bsp_stats.as_ref(),
                    self.game_session
                        .as_mut()
                        .and_then(|playback| playback.sound_presenter.as_mut()),
                ) {
                    sound.set_inline_model_midpoints(Arc::clone(&stats.inline_model_midpoints));
                }
                if let Some(stats) = bsp_stats {
                    self.push_console_line(format!(
                        "^5[MAP STATS]^7 brushes {} | brushsides {} | clipping planes {} | surfaces {} | bsp verts {} | bsp indices {} | render tris {} | batches {} | nodes {} | leaves {} | clusters {}",
                        stats.brushes, stats.brush_sides, stats.planes, stats.surfaces,
                        stats.vertices, stats.indices, triangles, batches, stats.collision_nodes,
                        stats.collision_leaves, stats.pvs_clusters,
                    ));
                }
                self.console_status =
                    format!("LOADED {name}: {:.3} seconds", first_frame_ms / 1000.0);
                self.push_console_line(format!("^2{}", self.console_status));
                self.loading = None;

                if self
                    .game_session
                    .as_ref()
                    .is_some_and(|session| session.live && session.map_name.as_deref().is_some_and(|map| {
                        Self::normalized_bsp_name(map) == Self::normalized_bsp_name(&name)
                    }))
                    && self.front_end
                {
                    self.pending_frontend_map_launch = false;
                    self.front_end = false;
                    self.frontend_page = FrontendPage::Main;
                    self.menu_selected = 0;
                    self.sync_render_fps_cap();
                }

                // OpenJK deliberately avoids advancing a demo across level-load
                // latency. Our equivalent boundary is WorldUploaded: the render
                // thread has produced the first real world frame, so initialize
                // the demo clock from the first snapshot now.
                self.start_demo_after_map_load(&name);

                // Start the keep/revert clock only after a real world frame has
                // been rendered. This gives the user the full timeout to inspect
                // the new mode instead of spending it on adapter/device/pipeline
                // creation and map upload.
                self.begin_pending_video_confirmation();
                self.publish_ui();
            }
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let window_matches = match self.window.as_ref() {
            Some(window) => window.id() == window_id,
            None => false,
        };
        if !window_matches {
            return;
        }
        let _activity = crate::thread_activity::activity(
            crate::thread_activity::ThreadSlot::Main,
            crate::thread_activity::Task::Input,
        );
        if self.route_egui_window_event(&event) {
            return;
        }
        if let WindowEvent::MouseInput { state, button, .. } = &event {
            if self.overlay == OverlayMode::None && self.captured {
                let down = *state == ElementState::Pressed;
                let button_number = match button {
                    MouseButton::Left => Some(1),
                    MouseButton::Right => Some(2),
                    MouseButton::Middle => Some(3),
                    MouseButton::Back => Some(4),
                    MouseButton::Forward => Some(5),
                    _ => None,
                };
                if let Some(button_number) = button_number {
                    if down { self.mouse_buttons_down.insert(button_number); }
                    else { self.mouse_buttons_down.remove(&button_number); }
                    self.refresh_bound_state();
                    if down { self.execute_binding_press(BindKey::Mouse(button_number)); }
                }
            }
        }

        match event {
            WindowEvent::CloseRequested => self.request_quit(),
            WindowEvent::Resized(size) => {
                if self.pending_renderer_restart.is_none()
                    && self.applied_fullscreen.is_windowed()
                    && size.width > 0
                    && size.height > 0
                {
                    self.video.window_maximized = self.window.as_ref().is_some_and(|window| window.is_maximized());
                    if !self.video.window_maximized {
                        self.video.resolution = [size.width, size.height];
                        self.applied_resolution = [size.width, size.height];
                    }
                    self.mark_config_dirty();
                }
                self.render_command(RenderCommand::Resize(size));
                self.publish_ui();
            }
            WindowEvent::Moved(position) => {
                if self.pending_renderer_restart.is_none()
                    && self.applied_fullscreen.is_windowed()
                    && !self.window.as_ref().is_some_and(|window| window.is_maximized())
                {
                    self.video.window_position = Some([position.x, position.y]);
                    self.mark_config_dirty();
                }
                if self.front_end {
                    self.sync_render_fps_cap();
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::KeyboardInput { event, .. } if self.video_confirmation.is_some() => {
                if event.state != ElementState::Pressed || event.repeat {
                    return;
                }
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                match code {
                    KeyCode::Enter | KeyCode::KeyY => self.confirm_video_settings(),
                    KeyCode::Escape | KeyCode::KeyN => self.revert_video_settings("REJECTED"),
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. }
                if self.controls_page_active() && self.controls_waiting_for_key =>
            {
                let key = match delta {
                    MouseScrollDelta::LineDelta(_, y) if y > 0.0 => Some(BindKey::WheelUp),
                    MouseScrollDelta::LineDelta(_, y) if y < 0.0 => Some(BindKey::WheelDown),
                    MouseScrollDelta::PixelDelta(pos) if pos.y > 0.0 => Some(BindKey::WheelUp),
                    MouseScrollDelta::PixelDelta(pos) if pos.y < 0.0 => Some(BindKey::WheelDown),
                    _ => None,
                };
                if let Some(key) = key { self.bind_control_key(key); }
            }
            WindowEvent::MouseWheel { delta, .. } if self.overlay == OverlayMode::None && self.captured => {
                let key = match delta {
                    MouseScrollDelta::LineDelta(_, y) if y > 0.0 => Some(BindKey::WheelUp),
                    MouseScrollDelta::LineDelta(_, y) if y < 0.0 => Some(BindKey::WheelDown),
                    MouseScrollDelta::PixelDelta(pos) if pos.y > 0.0 => Some(BindKey::WheelUp),
                    MouseScrollDelta::PixelDelta(pos) if pos.y < 0.0 => Some(BindKey::WheelDown),
                    _ => None,
                };
                if let Some(key) = key { self.execute_binding_press(key); }
            }
            WindowEvent::MouseWheel { delta, .. } if self.overlay == OverlayMode::Console => {
                let rows = match delta {
                    MouseScrollDelta::LineDelta(_, y) => (y * 4.0).round() as i32,
                    MouseScrollDelta::PixelDelta(position) => (position.y / 24.0).round() as i32,
                };
                self.scroll_console(rows);
                self.publish_ui();
            }
            WindowEvent::Focused(focused) => {
                self.window_focused = focused;
                self.apply_audio_mix();

                if focused {
                    return;
                }

                // True exclusive fullscreen keeps owning the monitor after focus loss
                // unless the window gets out of the way. Match normal game behavior on
                // Alt+Tab; Windows restores it when the user switches back. Entering a
                // fullscreen mode produces a transient focus loss of its own, so skip
                // this while a video change is waiting to be kept or reverted.
                if self.applied_fullscreen == FullscreenMode::Exclusive
                    && self.applied_renderer_backend != RendererBackend::Dx12
                    && !self.video_restart_in_flight()
                {
                    if let Some(window) = self.window.as_ref() { window.set_minimized(true); }
                }

                self.keys.clear();
                self.movement_keys.clear();
                self.mouse_buttons_down.clear();
                self.noclip_primary_down = false;
                self.noclip_alt_down = false;
                self.modifiers = ModifiersState::empty();
                self.set_capture(false);
                self.mouse_delta = (0.0, 0.0);
                if let Some(player) = &mut self.local_player {
                    player.pause();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_position = (position.x, position.y);
                if self.overlay == OverlayMode::Console && self.console_selecting {
                    let size = self.window.as_ref().map(|window| window.inner_size()).unwrap_or_default();
                    if let Some(point) =
                        self.console_point_at(size.width, size.height, position.x, position.y)
                    {
                        self.console_selection_focus = Some(point);
                        self.publish_ui();
                    }
                }
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button, .. }
                if self.controls_page_active() && self.controls_waiting_for_key =>
            {
                let key = match button {
                    MouseButton::Left => Some(BindKey::Mouse(1)),
                    MouseButton::Right => Some(BindKey::Mouse(2)),
                    MouseButton::Middle => Some(BindKey::Mouse(3)),
                    MouseButton::Back => Some(BindKey::Mouse(4)),
                    MouseButton::Forward => Some(BindKey::Mouse(5)),
                    _ => None,
                };
                if let Some(key) = key { self.bind_control_key(key); }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } if self.overlay == OverlayMode::Console => {
                let size = self.window.as_ref().map(|window| window.inner_size()).unwrap_or_default();
                if let Some(point) = self.console_point_at(
                    size.width,
                    size.height,
                    self.cursor_position.0,
                    self.cursor_position.1,
                ) {
                    self.begin_console_selection(point);
                } else {
                    self.console_selection_anchor = None;
                    self.console_selection_focus = None;
                    self.console_selecting = false;
                    self.console_last_click = None;
                    self.console_click_count = 0;
                }
                self.publish_ui();
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } if self.overlay == OverlayMode::Console => {
                self.console_selecting = false;
                if self.console_selection_anchor == self.console_selection_focus {
                    self.console_selection_anchor = None;
                    self.console_selection_focus = None;
                }
                self.publish_ui();
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } if self.overlay == OverlayMode::None => self.set_capture(true),
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                // Windows' desktop PrintScreen capture can be stale in Vulkan
                // exclusive fullscreen. Capture the actual WGPU surface instead;
                // the screenshot worker saves it and replaces the clipboard image.
                if code == KeyCode::PrintScreen {
                    // Trigger on key-up so Windows has already performed its own
                    // PrintScreen clipboard update. Our worker writes the fresh
                    // GPU frame afterwards and therefore wins the clipboard race.
                    if event.state == ElementState::Released {
                        self.request_screenshot();
                    }
                    return;
                }
                if event.state == ElementState::Pressed
                    && !event.repeat
                    && code == KeyCode::Backquote
                {
                    let desired_size = if self.modifiers.control_key() {
                        ConsoleSize::Full
                    } else if self.modifiers.shift_key() {
                        ConsoleSize::Half
                    } else {
                        ConsoleSize::Normal
                    };
                    if self.overlay == OverlayMode::Console && self.console_size == desired_size {
                        self.set_overlay(self.overlay_after_console());
                    } else {
                        if self.overlay != OverlayMode::Console {
                            self.overlay_before_console = self.overlay;
                        }
                        self.console_size = desired_size;
                        self.set_overlay(OverlayMode::Console);
                    }
                    return;
                }
                if event.state == ElementState::Pressed
                    && !event.repeat
                    && self.modifiers.control_key()
                    && code == KeyCode::KeyC
                    && matches!(
                        self.overlay,
                        OverlayMode::None | OverlayMode::Game | OverlayMode::Video
                    )
                    && self.surface_inspector.is_some()
                {
                    self.copy_surface_inspector_text();
                    return;
                }
                match self.overlay {
                    OverlayMode::Game => {
                        self.handle_game_key(&event, code);
                        return;
                    }
                    OverlayMode::Chat => {
                        self.handle_chat_key(&event, code);
                        return;
                    }
                    OverlayMode::Console => {
                        self.handle_console_key(&event, code);
                        return;
                    }
                    OverlayMode::Video => {
                        self.handle_video_key(&event, code);
                        return;
                    }
                    OverlayMode::HudEdit => {
                        if event.state == ElementState::Pressed && !event.repeat && code == KeyCode::Escape {
                            self.hud_edit_drag_origin = None;
                            self.hud_edit_drag_delta = [0.0, 0.0];
                            self.set_overlay(OverlayMode::None);
                        }
                        return;
                    }
                    OverlayMode::None => {}
                }

                match event.state {
                    ElementState::Pressed => { self.keys.insert(code); }
                    ElementState::Released => { self.keys.remove(&code); }
                }
                self.refresh_bound_state();
                if event.state == ElementState::Pressed && !event.repeat {
                    if code == KeyCode::Escape {
                        self.set_overlay(OverlayMode::Game);
                        return;
                    }
                    if code == KeyCode::KeyN && self.modifiers.shift_key() {
                        self.cycle_spawn();
                        return;
                    }
                    if let Some(binding) = self.bindings.get(keybinds::bind_key_for_code(code)).map(str::to_owned) {
                        if let Some(player) = &mut self.local_player {
                            for command in keybinds::split_binding_commands(&binding) {
                                let canonical = match command.split_whitespace().next().unwrap_or("").to_ascii_lowercase().as_str() {
                                    "+forward" => Some(KeyCode::KeyW),
                                    "+back" => Some(KeyCode::KeyS),
                                    "+moveright" => Some(KeyCode::KeyD),
                                    "+moveleft" => Some(KeyCode::KeyA),
                                    "+moveup" => Some(KeyCode::Space),
                                    "+movedown" => Some(KeyCode::ControlLeft),
                                    _ => None,
                                };
                                if let Some(canonical) = canonical { player.note_movement_key_press(canonical); }
                            }
                        }
                    }
                    self.execute_binding_press(keybinds::bind_key_for_code(code));
                }
            }
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        if !self.captured || self.overlay != OverlayMode::None {
            return;
        }
        if let DeviceEvent::MouseMotion { delta } = event {
            let event_at = Instant::now();
            self.mouse_input_sequence = self.mouse_input_sequence.wrapping_add(1);
            if self.mouse_input_sequence == 0 {
                self.mouse_input_sequence = 1;
            }

            // Live play samples accumulated mouse in CL_CreateCmd every frame.
            if self.video.input_subframe && self.net.is_none() {
                let mut applied = false;
                if let Some(player) = &mut self.local_player {
                    let simulation_at = Instant::now();
                    let elapsed = self
                        .last_mouse_motion_at
                        .replace(event_at)
                        .map(|last| event_at.saturating_duration_since(last))
                        .unwrap_or(Duration::from_millis(1));
                    player.set_mouse_input_settings(self.mouse_input);
                    player.apply_mouse_look_timed(delta, elapsed);
                    self.last_simulated_mouse_input = Some(InputLatencySample {
                        sequence: self.mouse_input_sequence,
                        event_at,
                        simulation_at,
                    });
                    applied = true;
                }
                if applied {
                    self.update_solo_player_view_and_presentation();
                    // Publish immediately so the render thread can use this exact
                    // mouse event without waiting for the next client/physics tick.
                    self.publish_snapshot();
                }
            } else {
                self.last_mouse_motion_at = Some(event_at);
                self.mouse_delta.0 += delta.0;
                self.mouse_delta.1 += delta.1;
                self.pending_mouse_input = Some((self.mouse_input_sequence, event_at));
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Fullscreen/backend restarts are deliberately staged across event-loop
        // turns so Windows can finish changing the HWND/display mode before the
        // next WGPU surface is created.
        self.continue_pending_renderer_restart(event_loop);
        self.tick();
        if self.quit_requested {
            self.flush_config();
            event_loop.exit();
            return;
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(1),
        ));
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.flush_config();
        // CL_Disconnect on quit: tell the server instead of timing out.
        if let Some(mut net) = self.net.take() {
            net.disconnect();
        }

        // Do not let the render thread outlive winit's event loop. The renderer
        // owns Arc<Window> references through the WGPU surface/renderer state;
        // detaching here can make the final Window drop happen on the render
        // thread after the Win32 event-loop message target has already gone away.
        // Winit then tries to PostMessage its fullscreen cleanup back to that
        // dead target and panics during process shutdown.
        if let Some(mut render) = self.render.take() {
            render.shutdown();
        }

        // With the render thread joined, this is now the last application-owned
        // Window reference. Drop it on the winit/event-loop thread while the
        // ActiveEventLoop is still alive so native fullscreen/window teardown is
        // deterministic instead of racing process exit.
        self.window.take();
    }
}

fn spawn_map_loader(
    base: PathBuf,
    proxy: EventLoopProxy<UserEvent>,
) -> Result<Sender<MapLoadRequest>, String> {
    let (tx, rx) = mpsc::channel::<MapLoadRequest>();
    thread::Builder::new()
        .name("jka-map-loader".into())
        .spawn(move || {
            let progress_proxy = proxy.clone();
            let jobs = match crate::map_jobs::MapJobPool::new_with_progress(move |progress| {
                let _ = progress_proxy.send_event(UserEvent::MapLoadProgress {
                    request_id: progress.request_id,
                    task: progress.task,
                    completed: progress.completed,
                    total: progress.total,
                });
            }) {
                Ok(jobs) => jobs,
                Err(error) => {
                    eprintln!("Map worker pool: {error}");
                    return;
                }
            };
            println!(
                "Map loader: {} worker job thread(s) available",
                jobs.worker_count()
            );
            while let Ok(request) = rx.recv() {
                jobs.begin_request(request.request_id);
                let name = request.source.label();
                let _activity = crate::thread_activity::activity(
                    crate::thread_activity::ThreadSlot::MapLoader,
                    crate::thread_activity::Task::MapPrepare,
                );
                match scene::prepare_source_with_jobs(
                    &base,
                    request.game.as_deref(),
                    &request.source,
                    &jobs,
                    request.prepare_options,
                ) {
                    Ok(mut map) => {
                        // A cache hit is already attached to `map`. A cache miss
                        // becomes a detached map-worker job only after the prepared
                        // world has been delivered, so first-time offline baking can
                        // never hold up joining a live server.
                        let bake_request = map.steam_audio_bake_request.take();
                        if proxy
                            .send_event(UserEvent::MapPrepared {
                                request_id: request.request_id,
                                started: request.started,
                                name: name.clone(),
                                map: Box::new(map),
                            })
                            .is_err()
                        {
                            return;
                        }

                        if let Some(bake_request) = bake_request {
                            let request_id = request.request_id;
                            let map_name = name.clone();
                            let progress_proxy = proxy.clone();
                            let finish_proxy = proxy.clone();
                            let started = Instant::now();
                            let submit_result = jobs.submit(
                                crate::thread_activity::Task::MapAudioBake,
                                move || {
                                    let _ = progress_proxy.send_event(
                                        UserEvent::SteamAudioBakeProgress {
                                            request_id,
                                            map_name: map_name.clone(),
                                            progress: 0.0,
                                        },
                                    );
                                    let callback_proxy = progress_proxy.clone();
                                    let callback_map_name = map_name.clone();
                                    let progress: crate::steam_audio::SteamAudioBakeProgress =
                                        Arc::new(move |fraction| {
                                            let _ = callback_proxy.send_event(
                                                UserEvent::SteamAudioBakeProgress {
                                                    request_id,
                                                    map_name: callback_map_name.clone(),
                                                    progress: fraction,
                                                },
                                            );
                                        });
                                    let result = std::panic::catch_unwind(
                                        std::panic::AssertUnwindSafe(|| {
                                            crate::steam_audio::load_or_bake(
                                                bake_request.mesh.as_ref(),
                                                &bake_request.cache,
                                                bake_request.num_threads,
                                                Some(progress),
                                            )
                                        }),
                                    )
                                    .map_err(|_| {
                                        "Steam Audio bake worker panicked".to_string()
                                    })
                                    .and_then(|result| result);
                                    let _ = finish_proxy.send_event(
                                        UserEvent::SteamAudioBakeFinished {
                                            request_id,
                                            map_name,
                                            result,
                                            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                                        },
                                    );
                                },
                            );
                            if let Err(error) = submit_result {
                                let _ = proxy.send_event(UserEvent::SteamAudioBakeFinished {
                                    request_id,
                                    map_name: name,
                                    result: Err(format!(
                                        "could not queue Steam Audio background bake: {error}"
                                    )),
                                    elapsed_ms: 0.0,
                                });
                            }
                        }
                    }
                    Err(error) => {
                        if proxy
                            .send_event(UserEvent::MapFailed {
                                request_id: request.request_id,
                                started: request.started,
                                name,
                                error,
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            }
        })
        .map_err(|e| format!("Could not start map loader: {e}"))?;
    Ok(tx)
}
