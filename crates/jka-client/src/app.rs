//! app module facade. Implementations are grouped by responsibility.
// Retain the existing facade API, including entry points used by external callers or tests.
#[allow(unused_imports)]
pub(crate) use display::AntiAliasingChoice;
mod capture;
mod chat;
mod commands;
mod configuration;
mod connection;
mod console_completion;
mod console_state;
mod constants;
mod demos;
mod display;
mod gamma;
mod filesystem;
mod frame;
mod frontend_previews;
mod hud;
mod initialize;
mod input;
mod inspector;
mod lifecycle;
mod local;
mod map_loading;
mod movement_hud;
mod notices;
mod performance;
mod race_ghost_runtime;
mod session;
mod settings;
mod text_input;
mod tick;
use commands::buffer::{random_unit, split_console_script};
use connection::types::{
    LiveDownloadState, LiveDownloadTransfer, LiveJoinUiPhase, LiveJoinUiState, MissingMapPrompt,
    ServerPasswordPrompt,
};
use console_state::{ConsolePathLink, ConsolePoint};
use constants::{
    DEMO_SCRUB_PREVIEW_INTERVAL, FALLBACK_RESOLUTIONS, FRONTEND_BACKGROUND_MAP,
    FRONTEND_CAMERA_EYE_HEIGHT, FRONTEND_FOREGROUND_MODEL, FRONTEND_FOREGROUND_MODEL_DISTANCE,
    FRONTEND_FOREGROUND_MODEL_HEAD_HEIGHT, FRONTEND_FOREGROUND_MODEL_SIDE_OFFSET,
    VIDEO_CONFIRM_TIMEOUT_SECS,
};
use demos::probe::probe_demo_gamestate;
use demos::recording::{demo_recording_timestamp, DemoRecording};
use demos::types::{
    DemoAdvance, DemoCameraSample, DemoConsoleFilter, DemoEventStat, DemoFakeView,
    DemoMetadataResult, DemoTimeline, DemoTimelineHit, DemoViewMode, RaceGhostLoadResult,
    SessionPhase, DEMO_FREE_SPEEDS, DEMO_FREE_SPEED_DEFAULT_INDEX,
};
use display::{
    normalize_anti_aliasing, AppliedVideoMode, ApplyLatchedScope, ApplyVideoPath,
    ConsoleCvarSetResult, FrontendCinematic, MapLoadPurpose, PendingRendererRestart,
    VideoConfirmation,
};
use frontend_previews::AssetPreviewFxState;
use hud::generic_timer_bar;
use map_loading::{
    prepared_map_has_ocean_surface, spawn_map_loader, MapLoadRequest, MapLoadingState, PerfSample,
    PreparedMapCache,
};
use notices::{
    charset_glyph_uv, ActiveReward, CenterPrintRecord, ChatPlayerCompletionCycle, ChatRecord,
    CrosshairSeen, RewardSpec, REWARD_BLOB_MS, REWARD_ICON_SIZE, REWARD_TIME_MS,
};
use session::sampling::{playerstate_vec3, sample_player_state, strafehelper_walking_anim};
use session::types::{
    snapshot_owns_playerstate_events, GameSession, LiveCgamePrepared, LiveJoinTiming,
    LivePredictionFrame,
};

mod companion;
mod egui_entity_graph;
mod egui_menu;
mod egui_pages;
mod egui_race_ghosts;
mod egui_settings;
mod egui_strafe_trails;
mod egui_theme;
mod egui_trace_menu;
mod frontend;
mod hitch;
mod map_editor;
mod quality;
mod race_ghost;
mod race_ghost_web;
mod serverdump;
mod session_tools;
mod ui_catalog;

use egui_entity_graph::EntityGraphUi;
use egui_menu::VideoSection;
use frontend::{
    build_demo_index, AssetDetail, AssetEntry, AssetFilter, AssetKind, ChatLogRangeFilter,
    DemoConsoleKind, DemoEntry, DemoIndex, DemoMetadata, FrontendPage, ProfileModelEntry,
    ProfileSaberEntry, SoloMapEntry,
};
use map_editor::{ExternalMapChange, MapEditor};

use crate::{
    camera::{
        offset_third_person_view, Camera, SpectatorCameraMode, SpectatorCameraSettings,
        SpectatorCameraState, ThirdPersonCameraState, ThirdPersonSettings, ThirdPersonViewInput,
        MAX_SPECTATOR_ORBIT_RANGE, MIN_SPECTATOR_ORBIT_RANGE,
    },
    cgame::{
        cloth::ClothConfig,
        entity_presenter::EntityPresenter,
        event_debug,
        event_presenter::{EventDispatchClass, EventPresenter},
        event_workers,
        player_presenter::{Ghoul2PresentationView, PlayerFxRequest, PlayerPresenter},
        presented_openjk_player,
        ragdoll::{PhysicsMapMesh, RagdollConfig},
        snapshot_discontinuity,
        view::{local_player_alpha, rendering_third_person, PlayerViewPolicyState},
        ClientGameState, ClientInfo, EntityPresentationKind, EventCheckDisposition,
        ForcedPlayerModels, PresentationViewer, PresentedEntity, ET_PLAYER,
    },
    chat_log::{BrowserDetail as ChatLogBrowserDetail, BrowserIndexEntry as ChatLogBrowserEntry},
    config,
    keybinds::{self, BindKey, Bindings},
    local_server::LocalServer,
    player::MouseInputSettings,
    renderer::{
        ClientFramePerf, DynamicModelSurface, EguiRenderData, InputLatencySample,
        InspectorEntityHint, PostEffects, RenderCommand, RenderSnapshot, RenderThread,
        TransientLight, ViewLatchMode,
    },
    runtime::{
        RenderStats, ScreenshotOutput, SurfaceInspectorInfo, SurfaceInspectorSection, UserEvent,
    },
    scene::{self, SpawnPoint},
    server_browser::{self, BrowserCommand, BrowserEvent, BrowserUiState, ServerSource},
    ui::{
        self, ChatMode, CloudRenderResolution, CloudType, ColorLutPreset, ConsolePathLinkUi,
        ConsoleSearchMatch, ConsoleSelection, ConsoleSize, CullDebugMode, DemoKillMarkerUi,
        DemoTimelineUi, DetailTextureMode, DofQuality, DynamicLightsMode, DynamicShadowsMode,
        EntityAmbientLightingMode, EntityShadowLight, FogMode, FootprintMode, FullscreenMode,
        FxGeometryMode, Ghoul2BatchMode, Ghoul2SkinningMode, HudElementId, HudElementLayout,
        HudLayout, HudState,
        MapLoadingBar, MapLoadingUi, OverlayMode, PerfStats, PlanarReflectionDebugMode,
        PredictionDebugUi, PuddleQuality, PvsMode, RainIntensity, ReflectionQuality,
        RendererBackend, SunVisibilityMode, TextureFilter, ThreadPerfStats, UiChatLine,
        UiScoreEntry, UiScoreboard, UiSnapshot, VideoSettings, VsyncMode,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    fs::File,
    io::Cursor,
    net::SocketAddr,
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

use jka_movement::{CollisionWorld, JoinMode};
use jka_protocol::{
    demo::{self, DemoReader},
    entity_event::EntityEvent,
    server::{
        Decoder as ServerMessageDecoder, Event as ServerMessageEvent, Snapshot as ProtocolSnapshot,
    },
};

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
    source_maps: Vec<SoloMapEntry>,
    source_map_selected: usize,
    source_map_catalog_loaded: bool,
    source_map_catalog_error: Option<String>,
    source_map_levelshot_texture: Option<egui::TextureHandle>,
    source_map_levelshot_texture_map: Option<String>,
    asset_viewer_entries: Vec<AssetEntry>,
    asset_viewer_catalog_loaded: bool,
    asset_viewer_catalog_error: Option<String>,
    asset_viewer_selected: Option<String>,
    asset_viewer_filter: AssetFilter,
    asset_viewer_search: String,
    asset_viewer_folder: String,
    /// Highlighted autocomplete row while the folder filter owns focus.
    asset_viewer_folder_suggestion: usize,
    /// Optional physical .shader file filter. Shader rows themselves represent
    /// individual definitions inside these files. Empty means all files.
    asset_viewer_shader_file: String,
    asset_viewer_detail_path: Option<String>,
    asset_viewer_detail: Option<Result<AssetDetail, String>>,
    asset_viewer_shader_name: Option<String>,
    asset_viewer_model_yaw: f32,
    asset_viewer_model_pitch: f32,
    asset_viewer_model_zoom: f32,
    screenshot_entries: Vec<crate::screenshot::ScreenshotEntry>,
    screenshot_catalog_loaded: bool,
    screenshot_catalog_error: Option<String>,
    screenshot_selected: Option<PathBuf>,
    screenshot_search: String,
    /// Empty means every game/mod folder.
    screenshot_game_filter: String,
    screenshot_preview_texture: Option<egui::TextureHandle>,
    screenshot_preview_path: Option<PathBuf>,
    screenshot_preview_pending: Option<PathBuf>,
    screenshot_preview_error: Option<String>,
    /// Set by the browser or EXE screenshot launch and consumed after the
    /// matching local map has created its solo authority.
    pending_screenshot_jump: Option<crate::screenshot::ScreenshotMetadata>,
    /// Startup drag/drop can ask the browser to select a plain/non-jumpable image.
    startup_screenshot_select: Option<PathBuf>,
    chat_log_browser_entries: Vec<ChatLogBrowserEntry>,
    chat_log_browser_catalog_loaded: bool,
    chat_log_browser_catalog_error: Option<String>,
    chat_log_browser_selected: Option<PathBuf>,
    chat_log_browser_detail_path: Option<PathBuf>,
    chat_log_browser_detail: Option<Result<ChatLogBrowserDetail, String>>,
    chat_log_browser_mod_filter: String,
    chat_log_browser_server_filter: String,
    chat_log_browser_search: String,
    chat_log_browser_range: ChatLogRangeFilter,
    chat_log_browser_date_from: String,
    chat_log_browser_date_to: String,
    chat_log_browser_show_global: bool,
    chat_log_browser_show_team: bool,
    chat_log_browser_show_located: bool,
    chat_log_browser_show_voice: bool,
    chat_log_browser_show_session: bool,
    asset_preview_player_presenter: Option<PlayerPresenter>,
    asset_preview_entity_presenter: Option<EntityPresenter>,
    asset_preview_model_key: Option<(String, i32, i32, i32)>,
    asset_preview_shader_key: Option<(String, String, i32)>,
    asset_preview_fx: Option<AssetPreviewFxState>,
    asset_preview_viewport_key: Option<[u32; 4]>,
    profile_models: Vec<ProfileModelEntry>,
    profile_sabers: Vec<ProfileSaberEntry>,
    profile_catalog_loaded: bool,
    profile_catalog_error: Option<String>,
    profile_selected_section: usize,
    profile_model_search: String,
    /// 0=all, 1=red variants, 2=blue variants.
    profile_model_team_filter: u8,
    profile_model_icon_textures: HashMap<String, egui::TextureHandle>,
    /// Settings-page thumbnails of the `gfx/2d/crosshair{a..j}` image crosshairs,
    /// keyed by `cg_crosshairImage` (1..=10).
    crosshair_image_textures: HashMap<u8, egui::TextureHandle>,
    profile_name_input: String,
    /// Debounced force-power preview request: (power index, last click time).
    profile_force_preview_pending: Option<(usize, Instant)>,
    /// Force power currently driving the Profile character animation.
    profile_force_preview_active: Option<usize>,
    profile_force_preview_started: Instant,
    profile_preview_yaw: f32,
    profile_preview_zoom: f32,
    profile_preview_started: Instant,
    profile_preview_key: Option<(String, i32, i32, i32)>,
    /// Profile edits change the local cvars (so the preview and config follow)
    /// but nothing reaches the server until APPLY, which avoids the server's
    /// "Too many info changes" throttle. `profile_defer_send` is set only
    /// while a Profile control writes a cvar.
    profile_defer_send: bool,
    /// A userinfo cvar was edited on the Profile page and not yet sent.
    profile_userinfo_pending: bool,
    /// True while the console command buffer runs. Userinfo cvars set by one
    /// run (an `exec`'d cfg sets dozens) are collapsed into a single send in
    /// `console_userinfo_dirty`, instead of tripping the server's
    /// "Too many info changes" throttle.
    console_buffer_running: bool,
    console_userinfo_dirty: bool,
    /// The Force loadout changed; APPLY sends `cmd forcechanged` (UI_UpdateClientForcePowers).
    profile_force_pending: bool,
    profile_dynamic_models: Arc<Vec<DynamicModelSurface>>,
    profile_preview_active: bool,
    demo_entries: Vec<DemoEntry>,
    demo_selected: usize,
    demo_catalog_loaded: bool,
    demo_catalog_error: Option<String>,
    demo_metadata_cache: BTreeMap<String, DemoMetadata>,
    demo_metadata_errors: BTreeMap<String, String>,
    demo_metadata_generation: u64,
    demo_metadata_inflight: HashSet<String>,
    demo_metadata_tx: Sender<DemoMetadataResult>,
    demo_metadata_rx: Receiver<DemoMetadataResult>,
    race_ghost_load_generation: u64,
    race_ghost_tx: Sender<RaceGhostLoadResult>,
    race_ghost_rx: Receiver<RaceGhostLoadResult>,
    race_ghost_alpha: f32,
    race_ghost_name: bool,
    race_ghost_trail: bool,
    race_ghost_velocity_delta: bool,
    race_ghost_distance_delta: bool,
    race_ghost_demo_base_url: String,
    race_ghost_demo_base_url_input: String,
    race_ghost_web_generation: u64,
    race_ghost_web_tx: Sender<race_ghost_web::WebResult>,
    race_ghost_web_rx: Receiver<race_ghost_web::WebResult>,
    race_ghost_web_courses: Vec<race_ghost_web::RemoteCourse>,
    race_ghost_web_demos: Vec<race_ghost_web::RemoteDemo>,
    race_ghost_web_selected_course: Option<String>,
    race_ghost_web_suggested_course: Option<String>,
    race_ghost_web_active_map: Option<String>,
    race_ghost_web_active_style: Option<String>,
    race_ghost_web_catalog_pending: bool,
    race_ghost_web_demos_pending: bool,
    race_ghost_web_pending_demos: HashSet<String>,
    race_ghost_web_error: Option<String>,
    ui_catalog: ui_catalog::UiCatalog,
    strafe_trails: crate::strafe_trail::Manager,
    strafe_trail_filter: String,
    strafe_trail_name_input: String,
    strafe_trail_log_input: String,
    strafe_trail_slot: u8,
    demo_console_filter: DemoConsoleFilter,
    demo_launch_seek_ms: Option<i32>,
    demo_view_mode: DemoViewMode,
    /// POV we detached from when entering demo Free Spec. Attack/alt-attack
    /// cycle relative to this client, matching normal spectator expectations.
    demo_free_follow_anchor: Option<i32>,
    demo_view_outside_authoritative: bool,
    /// Demo-only noclip speed. weapnext/weapprev step this like a 3D editor wheel.
    demo_free_speed_index: usize,
    server_browser_tx: Sender<BrowserCommand>,
    server_browser_rx: Receiver<BrowserEvent>,
    server_browser: BrowserUiState,
    game_session: Option<GameSession>,
    /// True while live CGame/networking are active without a locally loaded BSP.
    live_without_world: bool,
    /// True while demo playback is active without the demo's locally loaded BSP.
    demo_without_world: bool,
    /// Server-sent reason for the most recent involuntary disconnect (kick/ban/drop),
    /// shown as a dismissible popup over the main menu. Mirrors jaPRO/stock JKA's
    /// `com_errorMessage` -> `error_popmenu` behavior instead of leaving the reason
    /// buried in a console the player may never open.
    disconnect_notice: Option<String>,
    /// Password entry/retry dialog for a protected server. TaystJK checks
    /// `needpass` before joining from the browser; we also reuse the same prompt
    /// when a direct `/connect` is rejected because its password is missing/wrong.
    server_password_prompt: Option<ServerPasswordPrompt>,
    /// Blocking pre-connect choice shown when infoResponse names a BSP that is not installed.
    missing_map_prompt: Option<MissingMapPrompt>,
    /// Blocking demo-playback choice shown when the recorded BSP is not installed.
    demo_missing_map_prompt: Option<MissingMapPrompt>,
    /// One-connection authorization from the prompt's "Connect anyway" action.
    live_missing_map_authorized: bool,
    /// One-connection request to resolve missing referenced PK3s before map init.
    live_auto_download_requested: bool,
    /// HTTP download root learned from TaystJK-compatible mvhttp/mvhttpurl infoResponse fields.
    live_http_base: Option<String>,
    live_download: Option<LiveDownloadState>,
    /// Full-screen visual continuity for an autodownload-assisted join. This
    /// deliberately outlives the transfer itself so the main menu and the
    /// ordinary map splash cannot flash between download/gamestate/map phases.
    live_join_ui: Option<LiveJoinUiState>,
    /// Live server connection (OpenJK clc/cls networking).
    net: Option<crate::net::NetClient>,
    /// Presenter/material prep kicked off at connect time and consumed when the
    /// gamestate arrives. Receiver absence means there is no worker in flight.
    live_cgame_prep_rx: Option<Receiver<Result<LiveCgamePrepared, String>>>,
    live_cgame_prepared: Option<LiveCgamePrepared>,
    pending_live_gamestate: bool,
    /// Socket of the last `rcon`, kept so the server's `print` replies arrive.
    rcon: Option<crate::net::RconChannel>,
    /// Reliable server commands that arrive while the overlapped CGame worker
    /// is still finishing. They are replayed through ClientGameState after the
    /// gamestate baseline is installed, preserving OpenJK command sequencing.
    pending_live_server_commands: VecDeque<jka_protocol::server::ServerCommand>,
    live_join_timing: Option<LiveJoinTiming>,
    network: crate::net::NetworkSettings,
    /// Secure jaPRO credentials discovered in the OS credential store. Secrets
    /// themselves are never cached here; this list contains only endpoint/user.
    japro_saved_logins: Vec<crate::credential_store::SavedLogin>,
    japro_credential_error: Option<String>,
    /// Exact resolved endpoint already auto-login-attempted for this connection.
    japro_autologin_attempted: Option<SocketAddr>,
    /// jaPRO cgame options (`cg_stylePlayer`, race timer, spectator aids).
    japro_cg: crate::japro_cg::JaproCgame,
    /// The camera editor's undo state while `OverlayMode::CameraEdit` is open.
    camera_edit: Option<egui_pages::CameraEditState>,
    /// The Vote page's forms.
    vote_menu: egui_pages::VoteMenuState,
    live_input: crate::net::LiveInput,
    /// Lowercased `+command` kbuttons currently held (IN_KeyDown state).
    live_buttons: HashSet<String>,
    predictor: crate::net::Predictor,
    /// Rolling per-frame prediction/view history behind `hitchmark`.
    hitch: hitch::HitchRecorder,
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
    /// `cl.serverTime` (ms, fractional for rates that do not divide 1000) the next
    /// usercmd is due at under `cl_commandPacing 1`.
    next_command_time: Option<f64>,
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
    /// Render-only mapping from the newest real local viewangles onto the camera.
    /// Third-person data is produced by the existing OpenJK camera port.
    render_view_latch: ViewLatchMode,
    local_server: Option<LocalServer>,
    map_collision: Option<CollisionWorld>,
    /// Drawn/markable world surfaces for OpenJK-style R_MarkFragments marks.
    map_mark_surfaces: Option<std::sync::Arc<jka_assets::bsp::MarkSurfaces>>,
    /// The map's `misc_model_static` props (client-placed MD3s, jaPRO cgame).
    map_static_models: Arc<Vec<jka_assets::bsp::StaticModel>>,
    map_visibility: Option<jka_assets::bsp::Visibility>,
    steam_audio_acoustic_mesh: Option<Arc<jka_assets::bsp::AcousticMesh>>,
    steam_audio_bake: Option<Arc<crate::steam_audio::SteamAudioBakeData>>,
    steam_audio_bake_progress: Option<f32>,
    steam_audio_bake_error: Option<String>,
    /// Static BSP surface mesh cached for client-side Rapier visuals.
    map_physics_collision: PhysicsMapMesh,
    third_person: ThirdPersonSettings,
    spectator_camera: SpectatorCameraSettings,
    spectator_camera_state: SpectatorCameraState,
    first_person_lightsaber: bool,
    saber_trail: i32,
    saber_team_colors: bool,
    saber_staff_multi_color: bool,
    team_overlay: ui::TeamOverlaySettings,
    /// TaystJK cg_scoreDeaths (0..3).
    score_deaths: i32,
    /// TaystJK cg_drawScores (0..3).
    draw_scores: i32,
    mini_scores_shown: Option<ui::UiMiniScores>,
    team_overlay_shown: Option<ui::TeamOverlayUi>,
    cg_debug_events: u8,
    /// A/B switch: side-effect-free event semantic preparation on the dedicated Rayon pool.
    cg_event_workers: bool,
    crosshair: ui::CrosshairSettings,
    /// jaPRO cg.crosshairClientNum / cg.crosshairClientTime, plus the scan that set them.
    crosshair_target: ui::UiCrosshairTarget,
    /// World-space endpoint of the current crosshair trace (render coordinates).
    /// Shared by dynamic-crosshair projection and DOF autofocus.
    crosshair_hit_world: Option<[f32; 3]>,
    crosshair_seen: Option<CrosshairSeen>,
    crosshair_scan_at: Instant,
    /// `cg.forceSelectTime`: selector remains visible for OpenJK WEAPON_SELECT_TIME.
    force_select_until: Option<Instant>,
    /// TaystJK cg_drawPlayerNames. Occlusion is sampled at 30 Hz; the render
    /// thread still projects accepted labels every frame for smooth late-latched motion.
    player_names: ui::PlayerNameSettings,
    player_name_visible: [bool; 32],
    race_ghost_label_visible: [bool; 128],
    player_name_scan_at: Instant,
    /// The follow name last sent to the renderer; CG_DrawFollow re-reads it every
    /// frame, so a change of followed player must reach the screen on its own.
    follow_name_shown: Option<String>,
    /// The last whole-second classic game timer text sent to the renderer.
    game_timer_shown: Option<String>,
    /// The race readout last sent to the renderer.
    race_timer_shown: Option<crate::japro_cg::RaceTimerUi>,
    /// The vote summary last sent to the renderer.
    vote_line_shown: Option<String>,
    movement_keys_hud: ui::MovementKeysSettings,
    strafe_helper: ui::StrafeHelperSettings,
    hud_layout: HudLayout,
    hud_edit_selected: Option<HudElementId>,
    hud_edit_drag_origin: Option<[f32; 2]>,
    hud_edit_drag_delta: [f32; 2],
    /// Chat corner-resize snapshot. Resize uses total drag distance from this
    /// immutable start layout, rather than accumulating per-frame deltas onto
    /// an already-mutated rectangle.
    hud_edit_chat_resize_origin: Option<HudElementLayout>,
    /// Active chat resize corner as [move_left_edge, move_top_edge].
    hud_edit_chat_resize_corner: Option<[bool; 2]>,
    third_person_camera: ThirdPersonCameraState,
    local_player_presenter: Option<PlayerPresenter>,
    solo_dynamic_models: Arc<Vec<DynamicModelSurface>>,
    solo_client_info: ClientInfo,
    menu_selected: usize,
    setup_selected: usize,
    keys: HashSet<KeyCode>,
    movement_keys: HashSet<KeyCode>,
    bindings: Bindings,
    controls_selected: usize,
    controls_waiting_for_key: bool,
    /// Which Controls tab is showing (index into the action groups, then Mouse).
    controls_section: usize,
    mouse_buttons_down: HashSet<u8>,
    modifiers: ModifiersState,
    captured: bool,
    mouse_input: MouseInputSettings,
    last_mouse_motion_at: Option<Instant>,
    noclip_primary_down: bool,
    noclip_alt_down: bool,
    mouse_input_sequence: u64,
    last_simulated_mouse_input: Option<InputLatencySample>,
    cursor_position: (f64, f64),
    demo_scrub_dragging: bool,
    demo_scrub_fraction: Option<f32>,
    demo_scrub_resume_rate: f64,
    demo_scrub_last_seek: Option<Instant>,
    demo_scrub_last_applied_fraction: Option<f32>,
    last_demo_timeline_refresh: Instant,
    previous_tick: Instant,
    overlay: OverlayMode,
    overlay_before_console: OverlayMode,
    console_input: String,
    /// UTF-8 byte offset of the editable console caret inside `console_input`.
    console_cursor: usize,
    console_status: String,
    console_lines: VecDeque<String>,
    /// Per-line trusted local filesystem links. Kept parallel to `console_lines`.
    /// Only explicit engine metadata can populate this; console text is never parsed
    /// speculatively for path-looking strings.
    console_path_links: VecDeque<Vec<ConsolePathLink>>,
    console_timestamps: bool,
    /// `con_suggest`: live command/cvar filter popup above the input line.
    console_suggest: bool,
    /// `cg_chatboxCompletion`: Tab in the chat input completes player names.
    chatbox_completion: bool,
    /// `cl_chatLog`: session-based HTML logging for live server chat.
    chat_log_enabled: bool,
    chat_log: crate::chat_log::ChatLog,
    /// Highlighted popup row; only meaningful while `console_suggest_picked`
    /// (otherwise row 0 is the implicit Tab target).
    console_suggest_index: usize,
    console_suggest_picked: bool,
    /// Esc / history browsing hides the popup until the next edit so Up/Down
    /// keep meaning "history" and Esc can still close the console.
    console_suggest_hidden: bool,
    /// Pointer is over the console header's `?` button (shows the shortcut card).
    console_help_hover: bool,
    ui_vgs: i32,
    /// r_jumpHeightShade: jump-height landing tint, drawn only in jaPRO SP physics.
    jump_height_shade: bool,
    /// cg_screenShake.
    screen_shake: u8,
    /// DinurdoJK compact player-model visibility override. `0` disables; one
    /// model applies to all other players; `ally,enemy` splits team relations.
    force_model: String,
    forced_player_models: Option<ForcedPlayerModels>,
    /// TaystJK cg_zoomFov and transient CG_ZoomDown/CG_ZoomUp state.
    japro_zoom_fov: f32,
    japro_zoom_transition_at: Option<Instant>,
    /// TaystJK CG_DoAsync flipkick state/cvars.
    japro_fk_duration: i32,
    japro_fk_first_jump_duration: i32,
    japro_fk_second_jump_delay: i32,
    japro_flipkick_frames: i32,
    japro_flipkick_jumps: i32,
    japro_flipkick_moveup: bool,
    vgs_menu: crate::vgs::Menu,
    log_rx: Receiver<crate::logging::LogRecord>,
    console_history: Vec<String>,
    console_history_index: Option<usize>,
    /// OpenJK Cbuf-style pending commands. `exec` and `vstr` insert at the
    /// front; interactive/startup commands append at the back.
    console_command_buffer: VecDeque<String>,
    /// OpenJK cmd_wait countdown. A value of 1 delays the remainder until the
    /// next main/client frame.
    console_wait_frames: u32,
    /// OpenJK `map`/`devmap` block inside Cbuf_Execute until the level is
    /// spawned, so the rest of a script (`+devmap x +setviewpos ...`) runs
    /// against the new world. Map preparation here is asynchronous; while this
    /// is set the buffer yields until the pending load finishes or fails.
    console_map_barrier: bool,
    /// Active `perfsample`: holds the command buffer like `wait` until the
    /// sampling window closes, then prints one summary line.
    perf_sample: Option<PerfSample>,
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
    chat_player_completion: Option<ChatPlayerCompletionCycle>,
    chat_lines: VecDeque<ChatRecord>,
    last_chat_refresh: Instant,
    video_selected: usize,
    /// Which section of Setup -> Video the left rail is showing.
    video_section: VideoSection,
    environment_selected: usize,
    /// While the sun yaw/pitch is being edited the renderer draws a beam from the
    /// sun to the player's head; this is when that beam should be hidden again.
    sun_ray_until: Option<Instant>,
    sun_ray_sent: bool,
    ocean_settings_open: bool,
    authored_oceans: Vec<crate::ocean::authoring::AuthoredOcean>,
    map_authored_oceans: Vec<crate::ocean::authoring::AuthoredOcean>,
    authored_ocean_selected: usize,
    authored_ocean_preview: bool,
    ocean_selected: usize,
    cloud_tuning_open: bool,
    cloud_tuning_selected: usize,
    video: VideoSettings,
    gamma_runtime: gamma::GammaRuntime,
    /// Owns the paired timeBeginPeriod(1)/timeEndPeriod(1) request while enabled.
    windows_timer_resolution: crate::windows_timer::TimerResolutionGuard,
    /// Last quality preset explicitly applied, plus the canonical cvar values
    /// produced by its setters. This makes preset highlighting follow the
    /// actual applied state instead of brittle literal table values.
    quality_preset_selected: Option<quality::QualityPreset>,
    quality_preset_values: Vec<(String, String)>,
    applied_fullscreen: FullscreenMode,
    applied_renderer_backend: RendererBackend,
    applied_resolution: [u32; 2],
    applied_grass: bool,
    applied_ocean: bool,
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
    /// Old render thread being torn down during a renderer restart.
    retiring_render: Option<(RenderThread, Instant)>,
    /// Set while a restart has hidden the native window. Launching straight into
    /// Vulkan exclusive works because the window is created hidden and only shown
    /// once the swapchain has presented; a restart has to do the same or Windows
    /// leaves the new surface composited behind the old one.
    window_hidden_for_restart: bool,
    supported_msaa: Vec<u32>,
    wireframe_supported: bool,
    spawns: Vec<SpawnPoint>,
    spawn_index: usize,
    /// JKA gives a local client its first player spawn the "initial" spot
    /// (`SelectInitialSpawnPoint`); every later spawn avoids the current origin.
    solo_initial_spawn_pending: bool,
    /// A quality preset is mid-apply: `publish_ui` is coalesced into one send.
    settings_batch_open: bool,
    ui_publish_pending: std::cell::Cell<bool>,
    strafe_ground: std::cell::Cell<crate::strafehelper::GroundTracker>,
    /// jaPRO `cg_speedometer` state machine, its latest draw list and the last one published.
    speedometer: crate::speedometer::Speedometer,
    speedometer_ui: Option<crate::speedometer::Ui>,
    speedometer_shown: Option<crate::speedometer::Ui>,
    speedometer_clock: Instant,
    speedometer_last_frame: Option<Instant>,
    /// jaPRO `CG_DrawLagometer`'s latest draw list and the last one published.
    lagometer_ui: Option<crate::lagometer::Ui>,
    lagometer_shown: Option<crate::lagometer::Ui>,
    map_name: String,
    /// Lightweight source-.map brush editor state. Present only for loose writable .map sources.
    map_editor: Option<MapEditor>,
    /// Entity blueprint of the loaded map, for the `entities` overlay.
    entity_graph: Option<std::sync::Arc<crate::entity_graph::EntityGraph>>,
    entity_graph_ui: EntityGraphUi,
    /// Set by Developer Tools -> Map Viewer when EDIT SOURCE MAP launches a map.
    source_map_edit_on_load: bool,
    /// Background rebuild of the editor's unsaved working document. This is a
    /// transient source-world refresh: it never replaces editor state or writes disk.
    map_edit_preview_request_id: Option<u64>,
    /// Movement collision prepared for the same preview request. Commit it only
    /// once the renderer confirms that world was actually presented, keeping
    /// visible geometry and gameplay collision on the same revision.
    map_edit_preview_pending_collision: Option<CollisionWorld>,
    /// A snapped editor gesture changed again while a textured preview rebuild
    /// was already in flight. Keep only one expensive full source-map rebuild
    /// active at a time; when it lands, immediately request the newest working
    /// document instead of queuing stale mouse positions.
    map_edit_preview_dirty: bool,
    /// Last painted toolbar bounds in egui points. Raw 3D editor input must not
    /// leak through these controls (egui-winit does not mark every pointer
    /// button event consumed just because it lands on a foreground Area).
    map_edit_toolbar_rect: Option<egui::Rect>,
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
    /// Map label whose Rapier world mesh is present or being built by a worker.
    /// Stops repeated toggles from queueing duplicate builds.
    physics_mesh_label: Option<String>,
    /// While set, resize/move events are transient fullscreen-transition noise
    /// from a live display change and must not overwrite the requested mode.
    display_transition_until: Option<Instant>,
    loading: Option<MapLoadingState>,
    static_ao_progress: Option<(u32, u32)>,
    preserve_game_state_on_next_map_upload: bool,
    renderer_restart_started: Option<Instant>,
    perf: PerfStats,
    threads: [ThreadPerfStats; crate::thread_activity::SLOT_COUNT],
    surface_inspector: Option<SurfaceInspectorInfo>,
    /// History for the `/trace` menu (`OverlayMode::Trace`), newest last.
    /// `surface_inspector` always mirrors `trace_menu_entries[trace_menu_selected]`.
    trace_menu_entries: Vec<SurfaceInspectorInfo>,
    trace_menu_selected: Option<usize>,
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
    /// Optional native auxiliary workspace. Its WGPU surface lives on a separate
    /// sleeping render worker so desktop/compositor waits never enter the primary loop.
    companion: Option<companion::CompanionWindowState>,
    companion_target_enabled: bool,
    /// Client number shown by the companion's secondary spectator POV. `None`
    /// keeps the passive dashboard. Selection is made in the primary window so
    /// the companion never needs mouse/keyboard focus.
    companion_scene_target: Option<i32>,
}

#[cfg(test)]
#[path = "app/tests/snapshot_playerstate_transition_tests.rs"]
mod snapshot_playerstate_transition_tests;

#[cfg(test)]
#[path = "app/tests/reflection_prep_tests.rs"]
mod reflection_prep_tests;
