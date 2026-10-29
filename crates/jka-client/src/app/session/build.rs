//! Session build.
use crate::app::{App, Arc, EntityPresenter, GameSession, LocalServer, Path, PlayerPresenter};

impl App {
    pub(in crate::app) fn build_cgame_cpu_assets(
        base: &Path,
        game: Option<&Path>,
        pbr: bool,
        allow_asset_overrides: bool,
    ) -> Result<
        (
            jka_assets::siege::SiegeCatalog,
            Vec<jka_assets::siege::SiegeClassVisual>,
            PlayerPresenter,
            EntityPresenter,
            jka_assets::pk3::AssetSearchPath,
        ),
        String,
    > {
        Self::build_cgame_cpu_assets_cancellable(
            base,
            game,
            pbr,
            allow_asset_overrides,
            &std::sync::atomic::AtomicBool::new(false),
        )
    }

    /// `build_cgame_cpu_assets` for speculative workers: `cancel` is checked
    /// between the expensive stages so a superseded prep (for example the base
    /// prep started at connect before infoResponse named a mod) stops competing
    /// with the map loader for CPU and archive IO.
    pub(in crate::app) fn build_cgame_cpu_assets_cancellable(
        base: &Path,
        game: Option<&Path>,
        pbr: bool,
        allow_asset_overrides: bool,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<
        (
            jka_assets::siege::SiegeCatalog,
            Vec<jka_assets::siege::SiegeClassVisual>,
            PlayerPresenter,
            EntityPresenter,
            jka_assets::pk3::AssetSearchPath,
        ),
        String,
    > {
        let cancelled = || {
            cancel
                .load(std::sync::atomic::Ordering::Relaxed)
                .then(|| "CGame prep superseded".to_owned())
        };
        let open = || {
            jka_assets::pk3::AssetSearchPath::open_game(base, game).map(|mut assets| {
                assets.set_allow_asset_overrides(allow_asset_overrides);
                assets
            })
        };
        if let Some(error) = cancelled() { return Err(error); }
        let mut assets = open().map_err(|error| format!("ASSET PATH ERROR: {error}"))?;
        let siege_catalog = jka_assets::siege::load_siege_catalog(&mut assets)
            .map_err(|error| format!("SIEGE DATA LOAD ERROR: {error}"))?;
        let siege_classes = siege_catalog.visuals();
        devprintln!(
            1,
            "CGAME: loaded {} Siege classes, {} teams, {} maps",
            siege_catalog.classes.len(), siege_catalog.teams.len(), siege_catalog.maps.len()
        );
        if let Some(error) = cancelled() { return Err(error); }
        let player_presenter = PlayerPresenter::new(assets, pbr)
            .map_err(|error| format!("PLAYER ASSET ERROR: {error}"))?;
        if let Some(error) = cancelled() { return Err(error); }
        let entity_assets = open().map_err(|error| format!("ENTITY ASSET PATH ERROR: {error}"))?;
        let entity_presenter = EntityPresenter::new(entity_assets, pbr)
            .map_err(|error| format!("ENTITY ASSET ERROR: {error}"))?;
        if let Some(error) = cancelled() { return Err(error); }
        let fx_assets = open().map_err(|error| format!("FX ASSET PATH ERROR: {error}"))?;
        Ok((siege_catalog, siege_classes, player_presenter, entity_presenter, fx_assets))
    }

    /// CL_InitCGame's asset side: Siege classes and the player, entity, FX
    /// and sound presenters for a new CGame (demo or live).
    pub(in crate::app) fn build_game_session(
        &self,
        qpath: String,
        source: String,
        demo_bytes: Vec<u8>,
    ) -> Result<GameSession, String> {
        self.build_game_session_with_game(qpath, source, demo_bytes, self.game.as_deref())
    }

    pub(in crate::app) fn build_game_session_with_game(
        &self, qpath: String, source: String, demo_bytes: Vec<u8>, game: Option<&Path>,
    ) -> Result<GameSession, String> {
        let (siege_catalog, siege_classes, player_presenter, entity_presenter, fx_assets) =
            Self::build_cgame_cpu_assets(
                &self.base,
                game,
                self.video.pbr,
                self.video.allow_asset_overrides,
            )?;
        let mut session = GameSession::new(
            qpath,
            source,
            demo_bytes,
            siege_catalog,
            siege_classes,
            player_presenter,
            entity_presenter,
            crate::cgame::weapon_fx::WeaponFx::new(fx_assets),
            self.forced_player_models.clone(),
        );
        self.configure_game_session(&mut session, game, true);
        Ok(session)
    }

    /// Studio builds CPU assets/indexing on a worker, then installs owner-thread
    /// settings and audio here. Its map comes from its own loader, never gameplay.
    pub(in crate::app) fn configure_game_session(
        &self, session: &mut GameSession, game: Option<&Path>, inherit_map: bool,
    ) {
        session.water_wake_mode = self.video.water_wake;
        session.lake_wakes_enabled = self.video.ocean && self.video.ocean_settings.backend == 3 && self.video.ocean_settings.lake.shader == 1 && self.video.ocean_settings.lake.wake_strength > 0.0;
        session.ocean_interaction_enabled = self.video.ocean && self.video.ocean_settings.interaction && self.video.water_wake == 2;
        session.set_fx_light_promotion_catalog(Arc::clone(&self.fx_light_promotion_catalog));
        session
            .weapon_fx
            .set_modern_sabers(self.video.modern_sabers);
        session
            .weapon_fx
            .set_saber_impact_fx(self.video.saber_impact_fx);
        session.weapon_fx.set_saber_marks(self.video.saber_marks);
        session.weapon_fx.set_mark_count(self.video.mark_count as usize);
        if inherit_map { session
            .weapon_fx
            .set_collision_world(self.map_collision.clone());
        session
            .weapon_fx
            .set_mark_surfaces(self.map_mark_surfaces.clone());
        session
            .entity_presenter
            .set_static_models(Arc::clone(&self.map_static_models)); }
        session.entity_presenter.set_static_md3_instances(self.video.static_md3_instances);
        session.entity_presenter.set_static_md3_bvh(self.video.static_md3_bvh);
        session.entity_presenter.set_static_md3_pvs_fallback(self.video.static_md3_pvs_fallback);
        session.entity_presenter.set_occlusion_stats(self.video.occlusion_stats);
        session.weapon_fx.set_runner_culling(self.video.fx_occlusion == 1);
        if inherit_map { session
            .player_presenter
            .set_collision_world(self.map_collision.clone()); }
        session
            .player_presenter
            .set_head_look_mode(self.head_look);
        session.player_presenter.set_swim_tilt(self.swim_tilt);
        session.player_presenter.set_swim_turn_bank(self.swim_turn_bank);
        session.player_presenter.set_move_lean(self.move_lean);
        session
            .player_presenter
            .set_saber_team_colors(self.saber_team_colors);
        session
            .player_presenter
            .set_saber_staff_multi_color(self.saber_staff_multi_color);
        session.weapon_fx.set_saber_trail(self.saber_trail);
        session.weapon_fx.set_continuous_fx_fps(self.video.fx_fps);
        session.weapon_fx.set_fx_fps_scope(self.video.fx_fps_scope);
        session.weapon_fx.set_fx_physics(self.video.fx_physics);
        session.weapon_fx.set_fx_lod(
            self.video.fx_lod,
            self.video.fx_count_scale,
            self.video.fx_lod_scale,
        );
        session.promote_fx_lights = self.video.promote_fx_lights;
        session.apply_client_options(
            self.screen_shake,
            self.audio.game,
            self.video.footprints,
            self.japro_cg,
            self.network.plugin_disable,
        );
        session
            .client_game
            .set_predicted_event_correction(self.network.correct_predicted_events);
        match jka_assets::pk3::AssetSearchPath::open_game(&self.base, game) {
            Ok(assets) => {
                let mut sound = crate::cgame::sound_presenter::SoundPresenter::new(
                    assets,
                    self.audio.steam_audio,
                    self.audio.steam_audio_binaural,
                    self.audio.steam_audio_environmental,
                );
                sound.set_steam_audio_hrtf_interpolation(self.audio.steam_audio_hrtf_interpolation);
                sound.set_eal_hrtf_enabled(self.audio.eal_hrtf);
                sound.set_eal_enabled(self.audio.eal_enabled);
                sound.set_eal_debug_mode(self.audio.eal_debug_mode);
                let (effects, voice, separation) = self.effective_audio_mix();
                sound.set_mix(effects, voice, separation);
                sound.set_game_sounds(self.audio.game);
                sound.set_music_volume(self.effective_music_volume());
                sound.set_emote_music_volume(self.effective_emote_music_volume());
                if inherit_map { sound.set_steam_audio_map(
                    self.steam_audio_acoustic_mesh.clone(),
                    self.steam_audio_bake.clone(),
                ); }
                if let Some(map_name) = session.map_name.as_deref() { sound.set_eal_map(map_name); }
                session.sound_presenter = Some(sound);
            }
            Err(error) => eprintln!("AUDIO ASSET PATH UNAVAILABLE: {error}"),
        }
        if inherit_map { if let Err(error) = session
            .player_presenter
            .set_physics_map_mesh(&self.map_physics_collision)
        {
            eprintln!("RAPIER MAP COLLISION ERROR: {error}");
        } }
    }

    pub(in crate::app) fn load_local_saber_movement(
        &self,
    ) -> Result<[jka_movement::SaberMovementInfo; 2], String> {
        let mut assets =
            jka_assets::pk3::AssetSearchPath::open_game(&self.base, self.game.as_deref())
                .map_err(|error| format!("LOCAL SABER ASSET PATH ERROR: {error}"))?;
        assets.set_allow_asset_overrides(self.video.allow_asset_overrides);
        let definitions = jka_assets::saber::load_saber_definitions(&mut assets)
            .map_err(|error| format!("LOCAL SABER DEFINITION ERROR: {error}"))?;
        Ok(crate::local_server::saber_movement_loadout(
            &definitions,
            [
                &self.solo_client_info.saber_name,
                &self.solo_client_info.saber2_name,
            ],
            true,
        ))
    }

    /// Build CGame for the in-process one-client server. The local authority
    /// publishes decoded snapshots/configstrings at the same boundary as the
    /// network decoder, so presentation/FX never needs a solo-only branch.
    pub(in crate::app) fn build_local_game_session(
        &self,
        server: &LocalServer,
    ) -> Result<GameSession, String> {
        let mut session = self.build_game_session(
            format!("local:{}", server.map_name()),
            "in-process local server".to_owned(),
            Vec::new(),
        )?;
        session.live = true;
        session.local = true;
        session.map_name = Some(server.map_name().to_owned());
        // The generic session constructor runs before the local server's map
        // name is available. Initialize authored acoustics now, rather than
        // leaving a local game with an empty EAL state until the next map load.
        if let Some(sound) = session.sound_presenter.as_mut() {
            sound.set_eal_map(server.map_name());
        }
        session
            .client_game
            .reset_gamestate(server.configstrings(), 0);
        Ok(session)
    }

    /// Enqueue the latest authoritative local state exactly where a decoded
    /// `svc_snapshot` would enter a network GameSession.
    pub(in crate::app) fn push_local_snapshot(&mut self, force: bool) {
        // Keep the local authority on the same ordering as CL_ParseServerMessage:
        // reliable server commands are made visible to CGame before the snapshot
        // that advances `server_command_num` and executes them.
        if !self.game_session.as_ref().is_some_and(|session| session.local) {
            return;
        }
        let (commands, snapshot) = {
            let Some(server) = self.local_server.as_mut() else { return };
            let snapshot = server.take_snapshot(force);
            let commands = server.drain_server_commands();
            (commands, snapshot)
        };
        let session = self
            .game_session
            .as_mut()
            .expect("local game session checked above");
        for command in commands {
            session.client_game.queue_server_command(command);
        }
        let Some(snapshot) = snapshot else { return };
        if session.live_snapshots.len() >= 128 {
            session.live_snapshots.pop_front();
        }
        session.snapshots = session.snapshots.saturating_add(1);
        session.live_snapshots.push_back(snapshot);
    }

    pub(in crate::app) fn sync_local_client_info(&mut self) {
        self.solo_client_info.female = self.network.sex.as_bytes().first()
            .is_some_and(|byte| byte.eq_ignore_ascii_case(&b'f'));
        let saber_movement = self.load_local_saber_movement();
        let value = {
            let Some(server) = self.local_server.as_mut() else {
                return;
            };
            server.set_client_info(&self.solo_client_info);
            match saber_movement {
                Ok(sabers) => {
                    if let Err(error) = server.set_saber_movement_info(sabers) {
                        eprintln!("LOCAL SABER MOVEMENT UPDATE ERROR: {error}");
                    }
                }
                Err(error) => eprintln!("{error}"),
            }
            server.player_configstring().map(|value| value.to_vec())
        };
        if let (Some(value), Some(session)) = (
            value,
            self.game_session.as_mut().filter(|session| session.local),
        ) {
            session
                .client_game
                .set_configstring(crate::cgame::CS_PLAYERS, value);
        }
    }

    pub(in crate::app) fn attach_live_sound_presenter(&self, session: &mut GameSession) {
        match jka_assets::pk3::AssetSearchPath::open_game(&self.base, self.game.as_deref()) {
            Ok(assets) => {
                let mut sound = crate::cgame::sound_presenter::SoundPresenter::new(
                    assets,
                    self.audio.steam_audio,
                    self.audio.steam_audio_binaural,
                    self.audio.steam_audio_environmental,
                );
                sound.set_steam_audio_hrtf_interpolation(self.audio.steam_audio_hrtf_interpolation);
                sound.set_eal_hrtf_enabled(self.audio.eal_hrtf);
                sound.set_eal_enabled(self.audio.eal_enabled);
                sound.set_eal_debug_mode(self.audio.eal_debug_mode);
                let (effects, voice, separation) = self.effective_audio_mix();
                sound.set_mix(effects, voice, separation);
                sound.set_game_sounds(self.audio.game);
                sound.set_music_volume(self.effective_music_volume());
                sound.set_emote_music_volume(self.effective_emote_music_volume());
                sound.set_steam_audio_map(
                    self.steam_audio_acoustic_mesh.clone(),
                    self.steam_audio_bake.clone(),
                );
                if let Some(map_name) = session.map_name.as_deref() { sound.set_eal_map(map_name); }
                session.sound_presenter = Some(sound);
            }
            Err(error) => eprintln!("AUDIO ASSET PATH UNAVAILABLE: {error}"),
        }
    }
}
