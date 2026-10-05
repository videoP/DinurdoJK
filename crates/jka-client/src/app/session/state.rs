//! Session state.
use crate::app::{
    build_demo_index, Arc, BTreeMap, ClientFramePerf, ClientGameState, Cursor, DemoReader,
    EntityPresenter, EventPresenter, FootprintMode, ForcedPlayerModels, GameSession, HashMap,
    PlayerPresenter, ServerMessageDecoder, SessionPhase, VecDeque,
};

impl GameSession {
    /// Push the app-level client options every presenter of this session reads.
    pub(in crate::app) fn apply_client_options(
        &mut self,
        screen_shake: u8,
        game: crate::config::GameOptions,
        footprints: FootprintMode,
        japro: crate::japro_cg::JaproCgame,
        plugin_disable: i32,
    ) {
        self.screen_shake_level = screen_shake;
        self.weapon_fx.set_footprint_mode(footprints);
        self.weapon_fx.set_gib_level(game.blood);
        self.weapon_fx.set_score_plums(game.score_plums);
        self.player_presenter
            .set_gore_limit(usize::from(game.g2_marks));
        self.player_presenter
            .set_footstep_level(game.footsteps, footprints != FootprintMode::Off);
        self.player_presenter.set_japro_options(japro);
        self.client_game.set_plugin_disable(plugin_disable);
        self.japro_cg = japro;
    }

    /// jaPRO `DF_RaceTimer`: refresh this frame's race readout.
    pub(in crate::app) fn update_race_timer(
        &mut self,
        now: i32,
        predicted: Option<&jka_protocol::server::PlayerState>,
    ) {
        let ps = predicted.or_else(|| {
            self.current_snapshot
                .as_ref()
                .map(|snapshot| &snapshot.player_state)
        });
        let ui = ps.and_then(|ps| {
            let racemode =
                self.client_game.is_japro() && ps.stats[crate::japro_cg::STAT_RACEMODE] != 0;
            let duel_time = ps.field_i32("duelTime").unwrap_or(0);
            let speed = ps
                .field_f32("velocity[0]")
                .zip(ps.field_f32("velocity[1]"))
                .map_or(0.0, |(x, y)| x.hypot(y));
            // `speed` is a float netfield; reading it as i32 yields its raw bits.
            let move_speed = ps.field_f32("speed").unwrap_or(250.0);
            self.race_stats
                .frame(&self.japro_cg, racemode, duel_time, now, speed, move_speed)
        });
        self.race_timer_ui = ui;
    }

    /// The equipped sabers CG_PredictPlayerState's Pmove sees through
    /// BG_MySaber (`cgs.clientinfo[clientNum].saber[]`). Re-derived only when
    /// the client's userinfo configstring changes.
    pub(in crate::app) fn predicted_saber_movement(
        &mut self,
        client_num: i32,
    ) -> [jka_movement::SaberMovementInfo; 2] {
        let Ok(client) = usize::try_from(client_num) else {
            return Default::default();
        };
        let raw = crate::cgame::CS_PLAYERS
            .checked_add(client as u16)
            .and_then(|index| self.client_game.configstring(index))
            .unwrap_or_default();
        if let Some((key, cached)) = &self.predicted_saber_cache {
            if key.as_slice() == raw {
                return *cached;
            }
        }
        let key = raw.to_vec();
        let loadout = self
            .client_game
            .client_info(client, &self.siege_classes)
            .map(|info| {
                self.player_presenter
                    .saber_movement_loadout([&info.saber_name, &info.saber2_name], true)
            })
            .unwrap_or_default();
        self.predicted_saber_cache = Some((key, loadout));
        loadout
    }

    pub(in crate::app) fn new(
        qpath: String,
        source: String,
        bytes: Vec<u8>,
        siege_classes: Vec<jka_assets::siege::SiegeClassVisual>,
        player_presenter: PlayerPresenter,
        entity_presenter: EntityPresenter,
        weapon_fx: crate::cgame::weapon_fx::WeaponFx,
        forced_player_models: Option<ForcedPlayerModels>,
    ) -> Self {
        let demo_bytes: Arc<[u8]> = Arc::from(bytes);
        let demo_index = if demo_bytes.is_empty() {
            None
        } else {
            match build_demo_index(&demo_bytes) {
                Ok(index) => Some(index),
                Err(error) => {
                    eprintln!("DEMO INDEX WARNING: {error}");
                    None
                }
            }
        };
        Self {
            live: false,
            local: false,
            live_snapshots: VecDeque::new(),
            qpath,
            source,
            reader: DemoReader::new(Cursor::new(demo_bytes.clone())),
            demo_bytes,
            demo_index,
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
            seeking_demo: false,
            suppress_audio: false,
            demo_hidden_view_client: None,
            demo_spectator_camera_mode: None,
            client_deaths: [0; 32],
            player_position: None,
            client_game: ClientGameState::new(),
            siege_classes,
            presented_entities: Vec::new(),
            audio_followed_entity: None,
            predicted_saber_cache: None,
            forced_player_models,
            player_presenter,
            entity_presenter,
            event_presenter: EventPresenter::default(),
            pending_out_of_ammo: Vec::new(),
            view_offset_z: 0.0,
            item_pickup: None,
            timer_bar: None,
            reward_active: None,
            reward_queue: VecDeque::new(),
            force_flash_until: None,
            item_pickup_until: HashMap::new(),
            pending_weapon_select: Vec::new(),
            screen_shake_level: 1,
            japro_cg: crate::japro_cg::JaproCgame::default(),
            race_stats: crate::japro_cg::RaceStats::default(),
            race_timer_ui: None,
            race_ghosts: Vec::new(),
            race_ghost_alpha: 0.35,
            race_ghost_live_sample: None,
            lagometer: crate::lagometer::Lagometer::default(),
            map_change: false,
            vote_ui: None,
            follow_fastest: crate::japro_cg::FollowFastest::default(),
            sound_presenter: None,
            weapon_fx,
            fx_draws: Vec::new(),
            fx_lights: Arc::new(Vec::new()),
            fx_blades: Arc::new(Vec::new()),
            fx_surfaces: Arc::new(Vec::new()),
            screen_fx: Arc::new(Vec::new()),
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

    pub(in crate::app) fn push_event_debug_line(&mut self, line: String) {
        if self.event_debug_lines.len() >= 512 {
            self.event_debug_lines.pop_front();
        }
        self.event_debug_lines.push_back(line);
    }
}
