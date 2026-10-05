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
            Vec<jka_assets::siege::SiegeClassVisual>,
            PlayerPresenter,
            EntityPresenter,
            jka_assets::pk3::AssetSearchPath,
        ),
        String,
    > {
        let open = || {
            jka_assets::pk3::AssetSearchPath::open_game(base, game).map(|mut assets| {
                assets.set_allow_asset_overrides(allow_asset_overrides);
                assets
            })
        };
        let mut assets = open().map_err(|error| format!("ASSET PATH ERROR: {error}"))?;
        let siege_classes = jka_assets::siege::load_siege_class_visuals(&mut assets)
            .map_err(|error| format!("SIEGE CLASS LOAD ERROR: {error}"))?;
        devprintln!(
            1,
            "CGAME: loaded {} Siege class definition(s)",
            siege_classes.len()
        );
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
    pub(in crate::app) fn build_game_session(
        &self,
        qpath: String,
        source: String,
        demo_bytes: Vec<u8>,
    ) -> Result<GameSession, String> {
        let (siege_classes, player_presenter, entity_presenter, fx_assets) =
            Self::build_cgame_cpu_assets(
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
            self.forced_player_models.clone(),
        );
        session
            .weapon_fx
            .set_modern_sabers(self.video.modern_sabers);
        session
            .weapon_fx
            .set_saber_impact_fx(self.video.saber_impact_fx);
        session.weapon_fx.set_saber_marks(self.video.saber_marks);
        session
            .weapon_fx
            .set_collision_world(self.map_collision.clone());
        session
            .weapon_fx
            .set_mark_surfaces(self.map_mark_surfaces.clone());
        session
            .entity_presenter
            .set_static_models(Arc::clone(&self.map_static_models));
        session
            .player_presenter
            .set_collision_world(self.map_collision.clone());
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
        session.apply_client_options(
            self.screen_shake,
            self.audio.game,
            self.video.footprints,
            self.japro_cg,
            self.network.plugin_disable,
        );
        match jka_assets::pk3::AssetSearchPath::open_game(&self.base, self.game.as_deref()) {
            Ok(assets) => {
                let mut sound = crate::cgame::sound_presenter::SoundPresenter::new(
                    assets,
                    self.audio.steam_audio,
                    self.audio.steam_audio_binaural,
                    self.audio.steam_audio_environmental,
                );
                let (effects, voice, separation) = self.effective_audio_mix();
                sound.set_mix(effects, voice, separation);
                sound.set_game_sounds(self.audio.game);
                sound.set_music_volume(self.effective_music_volume());
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
        session
            .client_game
            .reset_gamestate(server.configstrings(), 0);
        Ok(session)
    }

    /// Enqueue the latest authoritative local state exactly where a decoded
    /// `svc_snapshot` would enter a network GameSession.
    pub(in crate::app) fn push_local_snapshot(&mut self, force: bool) {
        let snapshot = self
            .local_server
            .as_mut()
            .and_then(|server| server.take_snapshot(force));
        let Some(snapshot) = snapshot else { return };
        let Some(session) = self.game_session.as_mut().filter(|session| session.local) else {
            return;
        };
        if session.live_snapshots.len() >= 128 {
            session.live_snapshots.pop_front();
        }
        session.snapshots = session.snapshots.saturating_add(1);
        session.live_snapshots.push_back(snapshot);
    }

    pub(in crate::app) fn sync_local_client_info(&mut self) {
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
                let (effects, voice, separation) = self.effective_audio_mix();
                sound.set_mix(effects, voice, separation);
                sound.set_game_sounds(self.audio.game);
                sound.set_music_volume(self.effective_music_volume());
                sound.set_steam_audio_map(
                    self.steam_audio_acoustic_mesh.clone(),
                    self.steam_audio_bake.clone(),
                );
                session.sound_presenter = Some(sound);
            }
            Err(error) => eprintln!("AUDIO ASSET PATH UNAVAILABLE: {error}"),
        }
    }
}
