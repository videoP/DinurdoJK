//! Audio.
use crate::app::egui_menu::{theme, App};

impl App {
    pub(in crate::app::egui_menu) fn audio_choice_row(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        tip: &str,
        cvar: &str,
        current: u8,
        options: &[(u8, &str)],
    ) {
        let mut picked = None;
        theme::row(ui, label, tip, theme::Reset::None, |ui| {
            for &(value, text) in options {
                if theme::chip(ui, text, value == current).clicked() && value != current {
                    picked = Some(value);
                }
                ui.add_space(3.0);
            }
        });
        if let Some(value) = picked {
            let _ = self.set_console_cvar(cvar, &value.to_string());
        }
    }

    pub(in crate::app::egui_menu) fn egui_audio_page(&mut self, ui: &mut egui::Ui) {
        theme::page_title(
            ui,
            "AUDIO",
            "OpenJK mixer levels, Steam Audio spatial acoustics and native output diagnostics.",
        );

        egui::ScrollArea::vertical()
            .id_salt("jka_audio_settings")
            .auto_shrink([false, false])
            .show(ui, |ui| self.egui_audio_page_contents(ui));
    }

    pub(in crate::app::egui_menu) fn egui_audio_page_contents(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "MIX", "Changes are live and archived to DinurdoJK.cfg.");
        macro_rules! audio_slider {
            ($label:literal, $tip:literal, $field:ident, $cvar:literal) => {
                theme::row(ui, $label, $tip, theme::Reset::None, |ui| {
                    let mut value = self.audio.$field;
                    let readout = format!("{:.0}%", value * 100.0);
                    if theme::slider(ui, &mut value, 0.0..=1.0, &readout) {
                        let _ = self.set_console_cvar($cvar, &value.to_string());
                    }
                });
            };
        }
        audio_slider!(
            "Effects / game",
            "s_volume. OpenJK game/effects volume; stock default is 0.5.",
            effects_volume,
            "s_volume"
        );
        audio_slider!(
            "Voice",
            "s_volumeVoice. OpenJK voice-channel level; stock default is 1.0.",
            voice_volume,
            "s_volumeVoice"
        );
        audio_slider!(
            "Music",
            "s_musicvolume. Level music (the map's music track) and the duel track.",
            music_volume,
            "s_musicvolume"
        );
        theme::row(
            ui,
            "Mute when unfocused",
            "s_muteWhenUnfocused. Mutes game and voice audio while another window has focus.",
            theme::Reset::None,
            |ui| {
                if let Some(enabled) = theme::switch(ui, self.audio.mute_when_unfocused) {
                    let _ = self
                        .set_console_cvar("s_muteWhenUnfocused", if enabled { "1" } else { "0" });
                }
            },
        );

        theme::section(
            ui,
            "GAMEPLAY SOUNDS",
            "jaPRO / TaystJK voice and feedback options. They apply on every server and are archived to DinurdoJK.cfg.",
        );
        const WHO: [(u8, &str); 4] = [(0, "Off"), (1, "Everyone"), (2, "Others"), (3, "Only me")];
        self.audio_choice_row(
            ui,
            "Jump voice",
            "cg_jumpSounds. Voice line on jumps: off, everyone, other players only, or only your own. jaPRO itself defaults to off; DinurdoJK keeps the stock JKA behaviour (everyone).",
            "cg_jumpSounds",
            self.audio.game.jump,
            &WHO,
        );
        self.audio_choice_row(
            ui,
            "Roll voice",
            "cg_rollSounds. The model's roll voice (falls back to its jump voice) when rolling: off, everyone, other players only, or only you.",
            "cg_rollSounds",
            self.audio.game.roll,
            &WHO,
        );
        theme::row(
            ui,
            "Silence taunts",
            "cg_noTaunt. Mutes taunt voice lines.",
            theme::Reset::None,
            |ui| {
                if let Some(enabled) = theme::switch(ui, self.audio.game.no_taunt) {
                    let _ = self.set_console_cvar("cg_noTaunt", if enabled { "1" } else { "0" });
                }
            },
        );
        self.audio_choice_row(
            ui,
            "Footsteps",
            "cg_footsteps. The surface's footstep sound for each step of a player's walk and run animation. Footprints are a visual setting (Environment > Surface).",
            "cg_footsteps",
            u8::from(self.audio.game.footsteps > 0) * 3,
            &[(0, "Off"), (3, "On")],
        );
        self.audio_choice_row(
            ui,
            "Chat beep",
            "cg_chatSounds. Off by default. Legacy: the classic talk beep on every chat line. Distinct: separate beeps for private messages and team chat (jaPRO).",
            "cg_chatSounds",
            self.audio.game.chat_sounds,
            &[(0, "Off"), (1, "Legacy"), (2, "Distinct")],
        );
        theme::row(
            ui,
            "Race start sound",
            "cg_raceSounds (bit 1). jaPRO race mode: the sound the start trigger plays when your run begins.",
            theme::Reset::None,
            |ui| {
                let on = self.audio.game.race_sounds & 1 != 0;
                if let Some(enabled) = theme::switch(ui, on) {
                    let mask = if enabled { self.audio.game.race_sounds | 1 } else { self.audio.game.race_sounds & !1 };
                    let _ = self.set_console_cvar("cg_raceSounds", &mask.to_string());
                }
            },
        );
        theme::row(
            ui,
            "Level ambience",
            "cg_ambientSounds. The map's ambient sound sets: the worldspawn soundSet playing around you and local ambient emitters.",
            theme::Reset::None,
            |ui| {
                if let Some(enabled) = theme::switch(ui, self.audio.game.ambient) {
                    let _ = self.set_console_cvar("cg_ambientSounds", if enabled { "1" } else { "0" });
                }
            },
        );
        theme::row(
            ui,
            "Duel music",
            "cg_duelMusic. Plays the duel track while you are in a duel, then returns to the level music.",
            theme::Reset::None,
            |ui| {
                if let Some(enabled) = theme::switch(ui, self.audio.game.duel_music) {
                    let _ = self.set_console_cvar("cg_duelMusic", if enabled { "1" } else { "0" });
                }
            },
        );
        self.audio_choice_row(
            ui,
            "Duel start",
            "cg_duelSounds. The countdown sound and BEGIN DUEL text when your duel starts.",
            "cg_duelSounds",
            self.audio.game.duel,
            &[
                (0, "Off"),
                (1, "Sound + text"),
                (2, "Sound only"),
                (3, "Text only"),
            ],
        );
        self.audio_choice_row(
            ui,
            "Kill sound",
            "cg_killSounds. Frag sound when you kill someone; 'Mid-air' also plays a special sound for rocket, conc, bowcaster, alt repeater and saber kills on airborne targets. Needs the jaPRO sound/frag assets.",
            "cg_killSounds",
            self.audio.game.kill,
            &[(0, "Off"), (1, "Frag"), (2, "Frag + mid-air")],
        );
        self.audio_choice_row(
            ui,
            "Kill message",
            "cg_killMessage. TaystJK center-screen kill confirmation. Normal includes your FFA place/score; Kill only suppresses that footer; High moves the message upward.",
            "cg_killMessage",
            self.audio.game.kill_message,
            &[(0, "Off"), (1, "Normal + score"), (2, "Kill only"), (3, "High")],
        );
        self.audio_choice_row(
            ui,
            "Awards",
            "cg_drawRewards. TaystJK/JKA reward medals and announcer. Quake 3 swaps the supported Excellent, Impressive, Humiliation and Denied presentation to the Q3 variants.",
            "cg_drawRewards",
            self.audio.game.draw_rewards,
            &[(0, "Off"), (1, "JKA"), (2, "Quake 3")],
        );
        self.audio_choice_row(
            ui,
            "Hit sound",
            "cg_hitsounds. Feedback when you damage an enemy (a team sound plays for teammates). Sets 1-4 need the jaPRO sound/effects/hitsound assets; 5 uses only the plain saber-hit sound, 6 any saber-hit variant.",
            "cg_hitsounds",
            self.audio.game.hit,
            &[(0, "Off"), (1, "Set 1"), (2, "Set 2"), (3, "Set 3"), (4, "Set 4"), (5, "Saber plain"), (6, "Saber any")],
        );

        theme::section(
            ui,
            "SPATIAL AUDIO",
            "Valve Steam Audio is optional. When disabled, DinurdoJK keeps the legacy OpenJK-compatible spatial mixer and skips acoustic BSP/bake preparation entirely.",
        );
        theme::row(
            ui,
            "Steam Audio",
            "s_steamAudio. Master gate for Steam Audio HRTF/occlusion/reflections/pathing. Turning this off prevents the Steam Audio runtime from being used and prevents acoustic geometry/bake work on subsequent map loads. Enabling it while a map is already loaded takes full environmental acoustics on the next map load.",
            theme::Reset::None,
            |ui| {
                if let Some(enabled) = theme::switch(ui, self.audio.steam_audio) {
                    let _ = self.set_console_cvar("s_steamAudio", if enabled { "1" } else { "0" });
                }
            },
        );
        theme::row(
            ui,
            "Binaural HRTF",
            "s_steamAudioBinaural. Uses Steam Audio HRTF rendering for positional sounds. This is a live setting and does not require a map reload.",
            theme::Reset::None,
            |ui| {
                if let Some(enabled) = theme::switch(ui, self.audio.steam_audio_binaural) {
                    let _ = self.set_console_cvar("s_steamAudioBinaural", if enabled { "1" } else { "0" });
                }
            },
        );
        theme::row(
            ui,
            "Environmental Acoustics",
            "s_steamAudioEnvironmental. Live Steam Audio direct-path acoustics for positional world sounds: wall occlusion plus frequency-dependent material transmission. Simulation runs on a dedicated worker at a bounded rate; the audio callback only applies the latest result. No map reload or rebake is required once the map's Steam Audio scene is ready.",
            theme::Reset::None,
            |ui| {
                if let Some(enabled) = theme::switch(ui, self.audio.steam_audio_environmental) {
                    let _ = self.set_console_cvar(
                        "s_steamAudioEnvironmental",
                        if enabled { "1" } else { "0" },
                    );
                }
            },
        );

        theme::section(
            ui,
            "OUTPUT",
            "The active device opens with the CGame/demo sound presenter.",
        );
        let info = self
            .game_session
            .as_ref()
            .and_then(|playback| playback.sound_presenter.as_ref())
            .and_then(|sound| sound.info());
        if let Some(info) = info {
            theme::row(
                ui,
                "Device format",
                "Actual CPAL output format. Source WAV/MP3 sample rates are retained and Rodio resamples them to this device rate.",
                theme::Reset::None,
                |ui| {
                    theme::hint(
                        ui,
                        &format!("{}ch · {} Hz · {}", info.channels, info.sample_rate, info.sample_format),
                        theme::TEXT,
                    );
                },
            );
            theme::row(
                ui,
                "Buffer",
                "CPAL/Rodio output buffer selected for the current device.",
                theme::Reset::None,
                |ui| theme::hint(ui, &info.buffer_size, theme::TEXT_DIM),
            );
            let overload = info.overload_samples > 0;
            theme::row(
                ui,
                "Mix headroom",
                "Peak is measured before the final safety limiter. Values above 1.0 mean the floating-point game mix would clip without output protection.",
                theme::Reset::None,
                |ui| {
                    theme::hint(
                        ui,
                        &format!(
                            "peak {:.2} · {} overload sample{} · {} voices",
                            info.peak_before_limiter,
                            info.overload_samples,
                            if info.overload_samples == 1 { "" } else { "s" },
                            info.active_voices
                        ),
                        if overload { theme::WARNING } else { theme::TEXT_DIM },
                    );
                },
            );
            theme::row(
                ui,
                "Steam Audio state",
                "Master feature gate plus the currently attached BSP acoustic geometry. Geometry ready is only stage one; the rows below separately report offline bake readiness and audible DSP activation.",
                theme::Reset::None,
                |ui| {
                    let state = if !info.steam_audio_enabled {
                        "Disabled".to_string()
                    } else if info.steam_audio_scene_triangles == 0 {
                        "Enabled · waiting for next RBSP map load".to_string()
                    } else {
                        format!("Geometry ready · {} acoustic tris", info.steam_audio_scene_triangles)
                    };
                    theme::hint(
                        ui,
                        &state,
                        if info.steam_audio_enabled { theme::TEXT } else { theme::TEXT_FAINT },
                    );
                },
            );
            theme::row(
                ui,
                "Offline acoustic bake",
                "Steam Audio floor probes plus baked reflections/reverb and pathing. Cache hits attach immediately; first-time bakes run in the background after CPU map preparation.",
                theme::Reset::None,
                |ui| {
                    let (state, color) = if !info.steam_audio_enabled {
                        ("Disabled".to_string(), theme::TEXT_FAINT)
                    } else if let Some(progress) = self.steam_audio_bake_progress {
                        (format!("Baking in background · {:.0}%", progress * 100.0), theme::TEXT)
                    } else if let Some(error) = &self.steam_audio_bake_error {
                        (format!("Bake failed · {error}"), theme::WARNING)
                    } else if info.steam_audio_bake_ready {
                        (
                            format!(
                                "Ready · {} probes · {:.2} MiB · {}{}",
                                info.steam_audio_probe_count,
                                info.steam_audio_bake_bytes as f64 / (1024.0 * 1024.0),
                                if info.steam_audio_runtime_validated { "validated" } else { "not validated" },
                                if info.steam_audio_bake_cache_hit { " · cache hit" } else { " · freshly baked" },
                            ),
                            theme::TEXT,
                        )
                    } else if info.steam_audio_scene_triangles > 0 {
                        ("Geometry ready · bake queued/not yet attached".to_string(), theme::TEXT_DIM)
                    } else {
                        ("Waiting for map load".to_string(), theme::TEXT_DIM)
                    };
                    theme::hint(ui, &state, color);
                },
            );
            theme::row(
                ui,
                "Direct environmental DSP",
                "Live Steam Audio raycast occlusion and material transmission. Scene queries run on a dedicated worker at up to 30 Hz; the fixed-block audio DSP applies the latest coefficients.",
                theme::Reset::None,
                |ui| {
                    let (state, color) = if !info.steam_audio_enabled {
                        ("Disabled by Steam Audio master gate".to_owned(), theme::TEXT_FAINT)
                    } else if !info.steam_audio_environmental_enabled {
                        ("Off · direct sound unchanged".to_owned(), theme::TEXT_DIM)
                    } else if let Some(error) = &info.steam_audio_environmental_error {
                        (format!("Unavailable · {error}"), theme::WARNING)
                    } else if info.steam_audio_environmental_active {
                        (format!("Active · {} sources · {} updates · last {:.2} ms", info.steam_audio_environment_voice_count, info.steam_audio_environment_updates, info.steam_audio_environment_last_ms), theme::TEXT)
                    } else if info.steam_audio_scene_triangles > 0 {
                        ("Initializing direct simulator…".to_owned(), theme::TEXT_DIM)
                    } else {
                        ("Waiting for map acoustic scene".to_owned(), theme::TEXT_DIM)
                    };
                    theme::hint(ui, &state, color);
                },
            );
            theme::row(
                ui,
                "Baked reflections / pathing DSP",
                "Probe-baked reflections, reverb, and pathing are reported separately from live direct occlusion and transmission.",
                theme::Reset::None,
                |ui| {
                    theme::hint(ui, if info.steam_audio_bake_ready { "Pending · baked probe data ready" } else { "Pending · waiting for acoustic bake" }, theme::TEXT_DIM);
                },
            );
            theme::row(
                ui,
                "Binaural HRTF",
                "Live Steam Audio headphone positioning for positional sound sources.",
                theme::Reset::None,
                |ui| {
                    let state = if let Some(error) = &info.steam_audio_hrtf_error {
                        format!("Unavailable · {error}")
                    } else if info.steam_audio_binaural_active {
                        format!(
                            "Active · {} positional voices",
                            info.steam_audio_hrtf_voice_count
                        )
                    } else if info.steam_audio_binaural_enabled {
                        "Requested · waiting for Steam Audio master gate".to_owned()
                    } else {
                        "Disabled".to_owned()
                    };
                    theme::hint(
                        ui,
                        &state,
                        if info.steam_audio_binaural_active {
                            theme::TEXT
                        } else {
                            theme::TEXT_DIM
                        },
                    );
                },
            );
        } else {
            theme::row(
                ui,
                "Device format",
                "Audio output has not been opened by the active CGame/demo presenter yet.",
                theme::Reset::None,
                |ui| theme::hint(ui, "Not active", theme::TEXT_FAINT),
            );
        }
    }
}
