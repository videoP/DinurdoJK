//! Session types.
use crate::app::{
    race_ghost, ActiveReward, Arc, BTreeMap, ClientFramePerf, ClientGameState, Cursor,
    DemoEventStat, DemoIndex, DemoReader, DemoTimeline, DynamicModelSurface, EntityEvent,
    EntityPresenter, EventPresenter, ForcedPlayerModels, HashMap, Instant, PlayerPresenter,
    PresentedEntity, ProtocolSnapshot, RewardSpec, ServerMessageDecoder, SessionPhase,
    SpectatorCameraMode, TransientLight, VecDeque,
};

/// One CGame lifetime (OpenJK CL_InitCGame .. CL_ShutdownCGame) fed either by
/// a demo file or by a live connection. Both go through the same snapshot
/// transition, event, and presentation path.
/// Live prediction's display-only provisional state, used by presentation and
/// camera for sub-command-frame smoothness.
pub(in crate::app) struct LivePredictionFrame {
    pub(in crate::app) display: jka_protocol::server::PlayerState,
    /// The state presented on the previous frame (CG_TransitionPlayerState's ops).
    pub(in crate::app) previous_display: jka_protocol::server::PlayerState,
    pub(in crate::app) error: [f32; 3],
}

pub(in crate::app) fn snapshot_owns_playerstate_events(
    live: bool,
    local: bool,
    no_predict: bool,
    synchronous_clients: bool,
    player_state: &jka_protocol::server::PlayerState,
) -> bool {
    const PMF_FOLLOW: i32 = 4096;
    local
        || !live
        || no_predict
        || synchronous_clients
        || player_state.field_i32("pm_flags").unwrap_or(0) & PMF_FOLLOW != 0
}

pub(in crate::app) struct GameSession {
    /// Live sessions take snapshots from `live_snapshots` (filled by the
    /// network pump) instead of the demo reader, and their clock comes from
    /// CL_SetCGameTime rather than the demo timeline.
    pub(in crate::app) live: bool,
    /// In-process `/map`/`/devmap` source. It uses the same streamed snapshot
    /// queue as networking, but has no netchan or client-side predictor.
    pub(in crate::app) local: bool,
    pub(in crate::app) live_snapshots: VecDeque<ProtocolSnapshot>,
    pub(in crate::app) qpath: String,
    pub(in crate::app) source: String,
    pub(in crate::app) reader: DemoReader<Cursor<Arc<[u8]>>>,
    pub(in crate::app) demo_bytes: Arc<[u8]>,
    pub(in crate::app) demo_index: Option<DemoIndex>,
    pub(in crate::app) decoder: ServerMessageDecoder,
    pub(in crate::app) messages: usize,
    pub(in crate::app) snapshots: usize,
    pub(in crate::app) phase: SessionPhase,
    pub(in crate::app) map_name: Option<String>,
    pub(in crate::app) pending_snapshot: Option<ProtocolSnapshot>,
    pub(in crate::app) current_snapshot: Option<ProtocolSnapshot>,
    pub(in crate::app) next_snapshot: Option<ProtocolSnapshot>,
    pub(in crate::app) first_server_time: Option<i32>,
    pub(in crate::app) timeline: Option<DemoTimeline>,
    pub(in crate::app) eof: bool,
    pub(in crate::app) seeking_demo: bool,
    /// Presentation-only audio gate used while interactively scrubbing a demo.
    /// Keep the backend alive/cached, but do not feed it transient replay audio.
    pub(in crate::app) suppress_audio: bool,
    /// Remote ET_PLAYER to animate but not submit while faking its first-person POV.
    pub(in crate::app) demo_hidden_view_client: Option<i32>,
    /// Camera mode mirrored from App for non-live demo follow/recorded POV presentation.
    pub(in crate::app) demo_spectator_camera_mode: Option<SpectatorCameraMode>,
    /// Client-side obituary counts used by TaystJK cg_scoreDeaths modes 2/3.
    pub(in crate::app) client_deaths: [i32; 32],
    pub(in crate::app) player_position: Option<glam::Vec3>,
    pub(in crate::app) client_game: ClientGameState,
    pub(in crate::app) siege_classes: Vec<jka_assets::siege::SiegeClassVisual>,
    pub(in crate::app) presented_entities: Vec<PresentedEntity>,
    /// The predicted/followed player is intentionally absent from
    /// `presented_entities`; keep the synthesized entity for this frame so
    /// spatial audio can follow it through the final-camera respatialization.
    pub(in crate::app) audio_followed_entity: Option<PresentedEntity>,
    /// Raw CS_PLAYERS configstring the predicted client's saber loadout was last
    /// derived from, with the resulting `cgs.clientinfo[].saber[]` pair.
    pub(in crate::app) predicted_saber_cache:
        Option<(Vec<u8>, [jka_movement::SaberMovementInfo; 2])>,
    /// Client-side forced-model presentation override. Stored on the CGame lifetime so
    /// demo/live/local sessions all use the same visual-only rule.
    pub(in crate::app) forced_player_models: Option<ForcedPlayerModels>,
    pub(in crate::app) player_presenter: PlayerPresenter,
    pub(in crate::app) entity_presenter: EntityPresenter,
    pub(in crate::app) event_presenter: EventPresenter,
    /// EV_NOAMMO for the local player: (weapon to move off, weapon that ran dry).
    pub(in crate::app) pending_out_of_ammo: Vec<(i32, i32)>,
    /// This frame's `CG_StepOffset` / landing dip, added to the render camera's height.
    pub(in crate::app) view_offset_z: f32,
    /// The last item the viewer picked up (icon qpath, when) for the HUD.
    pub(in crate::app) item_pickup: Option<(String, Instant)>,
    /// EV_LOCALTIMER generic timing bar: (started, duration ms).
    pub(in crate::app) timer_bar: Option<(Instant, i32)>,
    /// TaystJK `cg.reward*`: the currently displayed medal and its FIFO.
    pub(in crate::app) reward_active: Option<ActiveReward>,
    pub(in crate::app) reward_queue: VecDeque<RewardSpec>,
    pub(in crate::app) force_flash_until: Option<Instant>,
    /// `cg_entities[item].weapon`: item entities ignore a second pickup for 500 ms.
    pub(in crate::app) item_pickup_until: HashMap<i32, i32>,
    /// Weapons (bg_itemlist giTag) the viewer just picked up, for auto switching.
    pub(in crate::app) pending_weapon_select: Vec<i32>,
    /// cg_screenShake, mirrored from the app.
    pub(in crate::app) screen_shake_level: u8,
    /// jaPRO cgame options mirrored from the app (`cg_stylePlayer`, race timer...).
    pub(in crate::app) japro_cg: crate::japro_cg::JaproCgame,
    /// `cg_raceTimer` speed statistics for the current run.
    pub(in crate::app) race_stats: crate::japro_cg::RaceStats,
    /// The race readout for this frame (`None` outside a timed run).
    pub(in crate::app) race_timer_ui: Option<crate::japro_cg::RaceTimerUi>,
    /// Visual-only recorded race references. Kept as a Vec from day one so
    /// multi-ghost presentation does not require changing session ownership.
    pub(in crate::app) race_ghosts: Vec<race_ghost::RaceGhost>,
    pub(in crate::app) race_ghost_alpha: f32,
    /// Exact predicted/followed sample used to compare the live racer against
    /// the synchronized ghost labels on this frame.
    pub(in crate::app) race_ghost_live_sample: Option<race_ghost::RaceGhostVisualSample>,
    /// jaPRO `lagometer`: frame / snapshot sample rings, fed as cgame reads snapshots
    /// and presents frames.
    pub(in crate::app) lagometer: crate::lagometer::Lagometer,
    /// `cg.mMapChange`: the server announced a map change (svc_mapchange).
    pub(in crate::app) map_change: bool,
    /// The open server vote, refreshed every frame.
    pub(in crate::app) vote_ui: Option<crate::vote::VoteUi>,
    /// Debounce state for the spectator "follow fastest" option.
    pub(in crate::app) follow_fastest: crate::japro_cg::FollowFastest,
    pub(in crate::app) sound_presenter: Option<crate::cgame::sound_presenter::SoundPresenter>,
    /// OpenJK FX system driven by CGame (missile trails, impacts, effect events).
    pub(in crate::app) weapon_fx: crate::cgame::weapon_fx::WeaponFx,
    /// This frame's FX primitives, tessellated once the final camera is known.
    pub(in crate::app) fx_draws: Vec<crate::fx::system::FxDraw>,
    /// RE_AddLightToScene-style FX lights in renderer coordinates.
    pub(in crate::app) fx_lights: Arc<Vec<TransientLight>>,
    /// Lit saber blades in renderer coordinates; clouds are kept off them.
    pub(in crate::app) fx_blades: Arc<Vec<crate::renderer::CloudForegroundBlade>>,
    pub(in crate::app) fx_surfaces: Arc<Vec<DynamicModelSurface>>,
    /// OpenJK CGame 2D effect stages (currently CG_SaberClashFlare).
    pub(in crate::app) screen_fx: Arc<Vec<crate::fx::draw::ScreenFxDraw>>,
    pub(in crate::app) dynamic_models: Arc<Vec<DynamicModelSurface>>,
    pub(in crate::app) client_perf: ClientFramePerf,
    /// CG_Mover inline BSP models for the current presentation frame.
    pub(in crate::app) inline_models: Arc<Vec<crate::renderer::InlineModelInstance>>,
    pub(in crate::app) logged_entity_summary: bool,
    pub(in crate::app) logged_dispatch_summary: bool,
    pub(in crate::app) event_debug_lines: VecDeque<String>,
    pub(in crate::app) event_stats: BTreeMap<EntityEvent, DemoEventStat>,
    pub(in crate::app) event_suppressed_duplicate: u64,
    pub(in crate::app) event_suppressed_zero: u64,
}

/// CPU-heavy CGame assets prepared while the challenge/gamestate handshake is
/// still in flight. Sound device creation intentionally remains on the main
/// thread; the expensive VFS/material/presenter work is safe to overlap.
pub(in crate::app) struct LiveCgamePrepared {
    pub(in crate::app) siege_classes: Vec<jka_assets::siege::SiegeClassVisual>,
    pub(in crate::app) player_presenter: PlayerPresenter,
    pub(in crate::app) entity_presenter: EntityPresenter,
    pub(in crate::app) fx_assets: jka_assets::pk3::AssetSearchPath,
    pub(in crate::app) stringed: Option<crate::cgame::stringed::StringEd>,
    pub(in crate::app) elapsed_ms: f64,
}

#[derive(Debug, Clone)]
pub(in crate::app) struct LiveJoinTiming {
    pub(in crate::app) started: Instant,
    pub(in crate::app) server_info_ms: Option<f64>,
    pub(in crate::app) cgame_ready_ms: Option<f64>,
    pub(in crate::app) gamestate_ms: Option<f64>,
    pub(in crate::app) first_world_frame_ms: Option<f64>,
    pub(in crate::app) prefetched_map: Option<String>,
}

impl LiveJoinTiming {
    pub(in crate::app) fn new() -> Self {
        Self {
            started: Instant::now(),
            server_info_ms: None,
            cgame_ready_ms: None,
            gamestate_ms: None,
            first_world_frame_ms: None,
            prefetched_map: None,
        }
    }

    pub(in crate::app) fn elapsed_ms(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1000.0
    }
}
