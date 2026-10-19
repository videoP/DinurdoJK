//! Settings audio.
use crate::{app::{App, FootprintMode}, runtime::UserEvent, scene};
use std::{sync::Arc, thread, time::Instant};

impl App {
    pub(in crate::app) fn effective_audio_mix(&self) -> (f32, f32, f32) {
        let muted = self.audio.mute_when_unfocused && !self.window_focused;
        (
            if muted {
                0.0
            } else {
                self.audio.effects_volume
            },
            if muted { 0.0 } else { self.audio.voice_volume },
            self.audio.separation,
        )
    }

    /// Footprints are a video setting independent of the `cg_footsteps` sound
    /// level; this just pushes the new style to the running game.
    pub(in crate::app) fn footprints_chosen(&mut self) {
        self.apply_game_sounds();
    }

    pub(in crate::app) fn apply_game_sounds(&mut self) {
        let options = self.audio.game;
        let footprints = self.video.footprints;
        if let Some(playback) = &mut self.game_session {
            playback.weapon_fx.set_footprint_mode(footprints);
            playback.weapon_fx.set_gib_level(options.blood);
            playback.weapon_fx.set_score_plums(options.score_plums);
            playback
                .player_presenter
                .set_gore_limit(usize::from(options.g2_marks));
            playback
                .player_presenter
                .set_footstep_level(options.footsteps, footprints != FootprintMode::Off);
            if let Some(sound) = &mut playback.sound_presenter {
                sound.set_game_sounds(options);
            }
        }
    }

    pub(in crate::app) fn effective_music_volume(&self) -> f32 {
        if self.audio.mute_when_unfocused && !self.window_focused {
            0.0
        } else {
            self.audio.music_volume
        }
    }

    pub(in crate::app) fn apply_audio_mix(&mut self) {
        let (effects, voice, separation) = self.effective_audio_mix();
        let music = self.effective_music_volume();
        let emote_music = self.effective_emote_music_volume();
        if let Some(playback) = &mut self.game_session {
            if let Some(sound) = &mut playback.sound_presenter {
                sound.set_mix(effects, voice, separation);
                sound.set_music_volume(music);
                sound.set_emote_music_volume(emote_music);
            }
        }
        if let Some(sound) = &mut self.menu_emote_sound { sound.set_emote_music_volume(emote_music); }
    }

    pub(in crate::app) fn effective_emote_music_volume(&self) -> f32 {
        if self.audio.mute_when_unfocused && !self.window_focused { 0. } else { self.audio.emote_music_volume }
    }

    /// TaystJK/OpenJK `snd_restart`: reopen the physical audio subsystem and
    /// restore the current client-side sound configuration without touching
    /// the renderer or reconnecting/reloading the map.
    pub(in crate::app) fn restart_sound_subsystem(&mut self) {
        let (effects, voice, separation) = self.effective_audio_mix();
        let music = self.effective_music_volume();
        let settings = self.audio;
        let acoustic_mesh = self.steam_audio_acoustic_mesh.clone();
        let bake = self.steam_audio_bake.clone();

        let result = self
            .game_session
            .as_mut()
            .and_then(|session| session.sound_presenter.as_mut())
            .ok_or_else(|| "no active sound presenter".to_owned())
            .and_then(|sound| {
                sound.restart_backend(
                    settings.steam_audio,
                    settings.steam_audio_binaural,
                    settings.steam_audio_environmental,
                    settings.steam_audio_hrtf_interpolation,
                    effects,
                    voice,
                    separation,
                    music,
                    acoustic_mesh,
                    bake,
                )
            });

        match result {
            Ok(info) => {
                self.push_console_line(format!(
                    "^2Sound restarted:^7 {}ch {} Hz {}",
                    info.channels, info.sample_rate, info.sample_format
                ));
                if settings.steam_audio && self.steam_audio_acoustic_mesh.is_none() {
                    self.prepare_current_steam_audio_scene_for_restart();
                }
            }
            Err(error) => {
                self.push_console_error(format!("^1snd_restart failed:^7 {error}"));
            }
        }
        self.publish_ui();
    }

    /// If Steam Audio was disabled when this BSP was loaded, normal map prep
    /// intentionally skipped its acoustic mesh. `snd_restart` fills only that
    /// missing piece on a worker and leaves the already-uploaded world alone.
    fn prepare_current_steam_audio_scene_for_restart(&mut self) {
        if self.steam_audio_bake_progress.is_some() {
            self.push_console_line(
                "^3Steam Audio:^7 current-map acoustic preparation is already running".to_owned(),
            );
            return;
        }
        let scene::MapSource::Bsp(map_name) = &self.initial_source else {
            self.push_console_line(
                "^3Steam Audio:^7 current source-map preview has no compiled BSP acoustic scene; it will be prepared on the next BSP load"
                    .to_owned(),
            );
            return;
        };

        let map_name = map_name.clone();
        let display_name = format!("{map_name}.bsp");
        let request_id = self.latest_request_id;
        let base = self.base.clone();
        let game = self.game.clone();
        let allow_asset_overrides = self.video.allow_asset_overrides;
        let proxy = self.proxy.clone();
        self.steam_audio_bake_progress = Some(0.0);
        self.steam_audio_bake_error = None;
        self.push_console_line(format!(
            "^3Steam Audio:^7 preparing current-map acoustics for {display_name}"
        ));

        let spawn = thread::Builder::new()
            .name("jka-snd-restart-acoustics".into())
            .spawn(move || {
                let prepared = scene::prepare_steam_audio_scene(
                    &base,
                    game.as_deref(),
                    &map_name,
                    allow_asset_overrides,
                );
                match prepared {
                    Ok(prepared) => {
                        let bake_request = prepared.bake_request.clone();
                        if proxy
                            .send_event(UserEvent::SteamAudioScenePrepared {
                                request_id,
                                map_name: display_name.clone(),
                                result: Ok(prepared),
                            })
                            .is_err()
                        {
                            return;
                        }

                        if let Some(bake_request) = bake_request {
                            let _ = proxy.send_event(UserEvent::SteamAudioBakeProgress {
                                request_id,
                                map_name: display_name.clone(),
                                progress: 0.0,
                            });
                            let progress_proxy = proxy.clone();
                            let progress_map_name = display_name.clone();
                            let progress: crate::steam_audio::SteamAudioBakeProgress =
                                Arc::new(move |fraction| {
                                    let _ = progress_proxy.send_event(
                                        UserEvent::SteamAudioBakeProgress {
                                            request_id,
                                            map_name: progress_map_name.clone(),
                                            progress: fraction,
                                        },
                                    );
                                });
                            let started = Instant::now();
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
                            .map_err(|_| "Steam Audio bake worker panicked".to_owned())
                            .and_then(|result| result);
                            let _ = proxy.send_event(UserEvent::SteamAudioBakeFinished {
                                request_id,
                                map_name: display_name,
                                result,
                                elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                            });
                        }
                    }
                    Err(error) => {
                        let _ = proxy.send_event(UserEvent::SteamAudioScenePrepared {
                            request_id,
                            map_name: display_name,
                            result: Err(error),
                        });
                    }
                }
            });

        if let Err(error) = spawn {
            self.steam_audio_bake_progress = None;
            self.steam_audio_bake_error = Some(error.to_string());
            self.push_console_error(format!(
                "^1Steam Audio:^7 could not start current-map acoustic worker: {error}"
            ));
        }
    }
}
