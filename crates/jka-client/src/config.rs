pub(crate) mod cvars;
use crate::camera::{SpectatorCameraSettings, ThirdPersonSettings, ViewBobSettings, DEFAULT_CG_FOV};
use crate::fx::{FX_FPS_LEGACY_JKA, FX_FPS_MAX, FX_FPS_MIN};
use crate::player::MouseInputSettings;
use crate::ui::{CrosshairSettings, HudLayout, MovementKeysSettings, PlayerNameSettings, StrafeHelperSettings, PvsMode, TextureFilter, DynamicShadowsMode, VideoSettings};
#[cfg(test)]
use crate::camera::SpectatorCameraMode;
#[cfg(test)]
use crate::ui::{ColorLutPreset, Ghoul2SkinningMode, Ghoul2BatchMode, SunVisibilityMode, CrtScope, CrtStyle, CrtQuality, CrtReferenceResolution, CrtMask, FullscreenMode, RendererBackend, VsyncMode, HudElementId, HudElementLayout, CloudRenderResolution};
use std::{fs, path::Path};

pub(crate) const GAMMA_MIN: f32 = 0.5;
pub(crate) const GAMMA_MAX: f32 = 3.0;
const FPS_CAP_MIN: u32 = 1;
pub const FPS_CAP_MAX: u32 = 10_000;

/// Saved frontend destination, applied only when starting without a map.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GameLaunchScreen {
    #[default]
    Main,
    Play,
    Solo,
    Servers,
    Demo,
    Profile,
    Controls,
    Settings,
    ChatLogs,
    Screenshots,
    DeveloperTools,
    Assets,
    Maps,
}

impl GameLaunchScreen {
    pub const OPTIONS: &[(Self, &str, &str)] = &[
        (Self::Main, "main", "Main menu"),
        (Self::Play, "play", "Play"),
        (Self::Solo, "solo", "Solo game"),
        (Self::Servers, "servers", "Join server"),
        (Self::Demo, "demo", "Play demo"),
        (Self::Profile, "profile", "Profile"),
        (Self::Controls, "controls", "Controls"),
        (Self::Settings, "setup", "Settings"),
        (Self::ChatLogs, "chatlogs", "Chat logs"),
        (Self::Screenshots, "screenshots", "Screenshots"),
        (Self::DeveloperTools, "devtools", "Developer tools"),
        (Self::Assets, "assets", "Asset viewer"),
        (Self::Maps, "maps", "Map viewer"),
    ];

    pub fn parse(value: &str) -> Option<Self> {
        Self::OPTIONS.iter().find(|(_, name, _)| name.eq_ignore_ascii_case(value.trim()))
            .map(|(screen, _, _)| *screen)
    }

    pub fn page_name(self) -> &'static str {
        Self::OPTIONS.iter().find(|(screen, _, _)| *screen == self).unwrap().1
    }

    pub fn label(self) -> &'static str {
        Self::OPTIONS.iter().find(|(screen, _, _)| *screen == self).unwrap().2
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClientPresentationSettings {
    pub game_launch_screen: GameLaunchScreen,
    pub third_person: ThirdPersonSettings,
    pub spectator_camera: SpectatorCameraSettings,
    pub first_person_lightsaber: bool,
    /// OpenJK cg_saberTrail: 0 disables saber swing trails, 1 is normal,
    /// 2 requests the legacy special/high-frequency mode.
    pub saber_trail: i32,
    /// TaystJK cg_saberTeamColors: force red/blue saber colors in ordinary team games.
    pub saber_team_colors: bool,
    /// TaystJK cg_saberStaffMultiColor: primary staff blades after blade 0 use c2.
    pub saber_staff_multi_color: bool,
    /// TaystJK cg_drawTeamOverlay family (styles/position/columns).
    pub team_overlay: crate::ui::TeamOverlaySettings,
    /// TaystJK cg_scoreDeaths: 0 off, 1 server deaths, 2 local fallback, 3 local count.
    pub score_deaths: i32,
    /// Heuristically identify spectators following the local player by matching
    /// three consecutive *distinct* ping samples. Cosmetic only.
    pub score_spectator_guess: bool,
    /// TaystJK cg_drawScores: 0 off, 1 classic, 2 coloured classic, 3 centred boxes.
    pub draw_scores: i32,
    /// TaystJK cg_drawScore (singular): score beside the player HUD.
    pub draw_score: i32,
    pub draw_score_x: f32,
    pub draw_score_y: f32,
    /// TaystJK cg_drawEnemyInfo: top-right opponent / leader panel.
    pub draw_enemy_info: bool,
    pub mouse: MouseInputSettings,
    /// TaystJK/OpenJK horizontal field of view on the 4:3 baseline.
    pub fov: f32,
    /// TaystJK cg_zoomFov: target FOV for the held +zoom bind.
    pub zoom_fov: f32,
    /// TaystJK cg_bobUp/Pitch/Roll + cg_runPitch/Roll (all default 0).
    pub view_bob: ViewBobSettings,
    /// TaystJK flipkick bind timing, counted in CGame frames.
    pub fk_duration: i32,
    pub fk_first_jump_duration: i32,
    pub fk_second_jump_delay: i32,
    /// DinurdoJK `cg_killfeed*`.
    pub killfeed: crate::ui::KillfeedSettings,
    /// DinurdoJK `cg_scoreboardAvatars` / `cg_scoreboardHeadToHead`.
    pub scoreboard: crate::ui::ScoreboardSettings,
    /// DinurdoJK `cg_chat*` chat box settings.
    pub chat: crate::ui::ChatSettings,
    pub model: String,
    /// DinurdoJK compact forced-player-model cvar: 0=off, model=all other players, ally,enemy=team split.
    pub force_model: String,
    pub crosshair: CrosshairSettings,
    pub player_names: PlayerNameSettings,
    pub hud_layout: HudLayout,
    pub movement_keys: MovementKeysSettings,
    pub strafe_helper: StrafeHelperSettings,
    /// jaPRO-compatible strafe-trail appearance, live tracing and recording settings.
    pub strafe_trail: crate::strafe_trail::Settings,
    /// Visual opacity of loaded race ghosts.
    pub race_ghost_alpha: f32,
    /// Draw the archive username/demo label above each synchronized race ghost.
    pub race_ghost_name: bool,
    /// Draw the recorded ghost route through the existing strafe-trail renderer.
    pub race_ghost_trail: bool,
    /// Append ghost speed minus local speed to the ghost's world-space label.
    pub race_ghost_velocity_delta: bool,
    /// Append the current 3D separation from the local racer to the ghost label.
    pub race_ghost_distance_delta: bool,
    /// Public race archive root; `/index/...` is derived from this value.
    pub race_ghost_demo_base_url: String,
    pub console_timestamps: bool,
    /// TaystJK con_scale: archived console character scale (0.2..10).
    pub console_scale: f32,
    /// TaystJK con_notifylines: recent console rows considered by Con_DrawNotify.
    pub console_notify_lines: i32,
    /// TaystJK con_notifywords: up to eight space-separated chat alert terms; -1 alerts every chat.
    pub console_notify_words: String,
    /// TaystJK con_notifyconnect: desktop alert for connect/disconnect server prints.
    pub console_notify_connect: bool,
    /// TaystJK con_notifyvote: desktop alert when a player calls a vote.
    pub console_notify_vote: bool,
    /// DinurdoJK extension: play the standard talk beep for alert-triggering notifications.
    pub console_notify_beep: bool,
    /// con_suggest: live command/cvar filter popup while typing in the console.
    pub console_suggest: bool,
    /// cg_chatboxCompletion: Tab in the chat input completes player names.
    pub chatbox_completion: bool,
    /// cg_cleanChatbox: 0 off, 1 strip body colors and filter spam, 2 recolor initial body color and filter spam.
    pub clean_chatbox: u8,
    pub streamer: crate::streamer::StreamerSettings,
    /// cl_chatLog: persist live server chat sessions as self-contained HTML.
    pub chat_log: bool,
    /// TaystJK ui_vgs: use the jaPRO VGS menu in place of stock team voice chat.
    pub ui_vgs: i32,
    /// r_jumpHeightShade: tint landing surfaces by jump height in jaPRO SP physics.
    pub jump_height_shade: bool,
    /// cg_screenShake: 0 off, 1 effect-driven shake (explosions, stomps), 2 also weapon-fire shake.
    pub screen_shake: u8,
    /// cg_headLook: 0 no automatic head tracking, 1 OpenJK, 2 gradual tracking.
    pub head_look: u8,
    /// cg_swimTilt: 0 = original swim presentation, 1 = full directional body tilt.
    pub swim_tilt: f32,
    /// cg_swimTurnBank: head-to-feet bank from actual velocity curvature (0..1).
    pub swim_turn_bank: f32,
    /// cg_moveLean: airborne acceleration/braking lean (0..1).
    pub move_lean: f32,
    /// Userinfo / network cvars (name, rate, snaps, cl_maxpackets, ...).
    pub network: crate::net::NetworkSettings,
    /// jaPRO cgame options: `cg_stylePlayer`, the race timer, spectator aids.
    pub japro: crate::japro_cg::JaproCgame,
    /// TaystJK-compatible server-browser masters (`sv_master1`..`sv_master5`).
    pub master_servers: [String; crate::server_browser::MAX_MASTER_SLOTS],
    /// `cl_serverFilters`: server-browser hide flags, max ping and mod filter.
    pub server_filters: String,
}

/// jaPRO/TaystJK client options (sounds, gibs, auto switch: `cg_jumpSounds` and friends). They apply
/// to any server, as in TaystJK. Defaults keep stock JKA behaviour where jaPRO's
/// own defaults would change it (jaPRO ships `cg_jumpSounds 0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameOptions {
    /// 0 off, 1 everyone, 2 other players only, 3 own jumps only.
    pub jump: u8,
    /// 0 off, 1 everyone, 2 other players only, 3 own rolls only.
    pub roll: u8,
    /// Suppress taunt voice lines.
    pub no_taunt: bool,
    /// 0 off, 1 sound + text, 2 sound only, 3 text only for the duel start.
    pub duel: u8,
    /// 0 off, 1 frag sound, 2 frag sound with a mid-air variant.
    pub kill: u8,
    /// TaystJK `cg_killMessage`: 0 off, 1 normal + FFA place/score, 2 kill
    /// only at normal height, 3 kill message at TaystJK's higher position.
    pub kill_message: u8,
    /// TaystJK `cg_drawRewards`: 0 off, 1 JKA/Mon Mothma awards, 2 Q3
    /// variants for impressive/excellent/humiliation/denied.
    pub draw_rewards: u8,
    /// 0 off, 1..=4 hit sound set; 5/6 choose saber-hit variants only.
    pub hit: u8,
    /// `cg_duelMusic`: play the duel track while you are in a duel.
    pub duel_music: bool,
    /// `cg_ambientSounds`: level ambience (`sound.txt` general/local sets).
    pub ambient: bool,
    /// jaPRO `cg_blood`: 0 no gibs (a death voice instead), 1 skull or brain,
    /// 2 full gibs. Lives here because the sound side reads it too.
    pub blood: u8,
    /// `cg_autoSwitch`: 0 never, 1 switch to a better *safe* weapon on pickup (and
    /// when one runs dry), 2 any better weapon.
    pub auto_switch: u8,
    /// `cg_scorePlums`: floating score numbers when you score.
    pub score_plums: bool,
    /// `cg_ghoul2Marks`: burn marks kept per player model (0 off; jaPRO's default is 16).
    pub g2_marks: u8,
    /// jaPRO `cg_raceSounds` bit mask; bit 0 keeps the race start-trigger sound.
    pub race_sounds: u8,
    /// jaPRO `cg_chatSounds`: 0 silent, 1 the legacy talk beep for every chat
    /// line, 2 distinct beeps for private messages and team chat.
    pub chat_sounds: u8,
    /// jaPRO `cg_footsteps`: 0 off, 1 sounds, 2 + material effects, 3 + prints
    /// (how they look is `r_footprints`), 4 the debugging "always" level.
    pub footsteps: u8,
}

impl Default for GameOptions {
    fn default() -> Self {
        Self {
            jump: 1,
            roll: 1,
            no_taunt: false,
            duel: 1,
            kill: 2,
            kill_message: 1,
            draw_rewards: 1,
            hit: 0,
            duel_music: true,
            ambient: true,
            blood: 0,
            auto_switch: 1,
            score_plums: true,
            g2_marks: 16,
            race_sounds: 1,
            chat_sounds: 0,
            footsteps: 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioSettings {
    pub effects_volume: f32,
    pub voice_volume: f32,
    pub music_volume: f32,
    pub emote_music_volume: f32,
    pub variometer: bool,
    pub variometer_style: u8,
    pub variometer_input: u8,
    /// Strafe-angle input: degrees from the optimal angle that stay silent.
    pub variometer_dead_zone: f32,
    /// Strafe-angle input: sensitivity multiplier (1 = full range 15 degrees past the dead zone).
    pub variometer_scale: f32,
    pub variometer_volume: f32,
    pub separation: f32,
    pub mute_when_unfocused: bool,
    /// Master gate for Valve Steam Audio integration. Off preserves the legacy
    /// OpenJK-compatible mixer and skips all acoustic scene/bake preparation.
    pub steam_audio: bool,
    /// Opt-in authored EAGLE/EAX reverb on maps with valid .eal.
    pub eal_enabled: bool,
    /// Independent OpenAL Soft HRTF preference for EAL-owned sources.
    pub eal_hrtf: bool,
    /// Ephemeral EAL troubleshooting mode (not saved in the user config).
    pub eal_debug_mode: u8,
    /// Live Steam Audio HRTF rendering for positional sounds.
    pub steam_audio_binaural: bool,
    /// Steam Audio HRTF direction interpolation: 0 = nearest, 1 = bilinear.
    pub steam_audio_hrtf_interpolation: u8,
    /// Live direct-path Steam Audio occlusion and transmission.
    pub steam_audio_environmental: bool,
    pub game: GameOptions,
}

impl Default for AudioSettings {
    fn default() -> Self {
        // OpenJK codemp/client/snd_dma.cpp defaults.
        Self {
            effects_volume: 0.5,
            voice_volume: 1.0,
            music_volume: 0.25,
            emote_music_volume: 0.5,
            variometer: false,
            variometer_style: 0,
            variometer_input: 0,
            variometer_dead_zone: 0.5,
            variometer_scale: 1.0,
            variometer_volume: 0.35,
            separation: 0.5,
            mute_when_unfocused: true,
            steam_audio: false,
            eal_enabled: false,
            eal_hrtf: false,
            eal_debug_mode: 0,
            steam_audio_binaural: true,
            steam_audio_hrtf_interpolation: 0,
            steam_audio_environmental: true,
            game: GameOptions::default(),
        }
    }
}

/// An integer cvar value clamped to `0..=max` (cvar `.integer` truncates).
pub(crate) fn int_in(value: &str, max: u8) -> Option<u8> {
    let number = value
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|number| number.is_finite())?;
    Some((number as i32).clamp(0, i32::from(max)) as u8)
}

/// Load the archived OpenJK sound controls that DinurdoJK already implements
/// (plus music volume so it is ready for the background-track presenter).
pub fn load_audio_settings(primary: &Path) -> AudioSettings {
    let mut settings = AudioSettings::default();
    let source = fs::read_to_string(primary).ok();
    let Some(text) = source else {
        return settings;
    };

    let text = strip_cfg_comments(&text);
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") || line.starts_with('#') {
            continue;
        }
        let words = split_cfg_words(line);
        if words.len() < 2 {
            continue;
        }
        let (name, value) = if matches!(
            words[0].to_ascii_lowercase().as_str(),
            "set" | "seta" | "sets" | "setu"
        ) {
            if words.len() < 3 {
                continue;
            }
            (words[1].as_str(), words[2].as_str())
        } else {
            (words[0].as_str(), words[1].as_str())
        };

        if let Some(load) = crate::app::cvar_definition(name).and_then(|def| def.config.load_audio) { load(&mut settings, name, value); }
    }
    settings
}

impl Default for ClientPresentationSettings {
    fn default() -> Self {
        Self {
            game_launch_screen: GameLaunchScreen::default(),
            third_person: ThirdPersonSettings::default(),
            spectator_camera: SpectatorCameraSettings::default(),
            first_person_lightsaber: true,
            saber_trail: 1,
            saber_team_colors: true,
            saber_staff_multi_color: false,
            team_overlay: crate::ui::TeamOverlaySettings::default(),
            score_deaths: 1,
            score_spectator_guess: true,
            draw_scores: 1,
            draw_score: 2,
            draw_score_x: 0.0,
            draw_score_y: 0.0,
            draw_enemy_info: true,
            mouse: MouseInputSettings::default(),
            fov: DEFAULT_CG_FOV,
            zoom_fov: 30.0,
            view_bob: ViewBobSettings::default(),
            fk_duration: 50,
            fk_first_jump_duration: 0,
            fk_second_jump_delay: 0,
            killfeed: crate::ui::KillfeedSettings::default(),
            scoreboard: crate::ui::ScoreboardSettings::default(),
            chat: crate::ui::ChatSettings::default(),
            model: "kyle".to_owned(),
            force_model: "0".to_owned(),
            crosshair: CrosshairSettings::default(),
            player_names: PlayerNameSettings::default(),
            hud_layout: HudLayout::default(),
            movement_keys: MovementKeysSettings::default(),
            strafe_helper: StrafeHelperSettings::default(),
            strafe_trail: crate::strafe_trail::Settings::default(),
            race_ghost_alpha: 0.35,
            race_ghost_name: false,
            race_ghost_trail: false,
            race_ghost_velocity_delta: false,
            race_ghost_distance_delta: false,
            race_ghost_demo_base_url: "http://s.playja.pro/races".to_owned(),
            console_timestamps: true,
            console_scale: 1.0,
            console_notify_lines: 3,
            console_notify_words: "0".to_owned(),
            console_notify_connect: false,
            console_notify_vote: true,
            console_notify_beep: true,
            console_suggest: true,
            chatbox_completion: true,
            clean_chatbox: 0,
            streamer: crate::streamer::StreamerSettings::default(),
            chat_log: true,
            ui_vgs: 1,
            jump_height_shade: true,
            screen_shake: 1,
            head_look: 1,
            swim_tilt: 0.9,
            swim_turn_bank: 1.0,
            move_lean: 0.75,
            network: crate::net::NetworkSettings::default(),
            japro: crate::japro_cg::JaproCgame::default(),
            master_servers: crate::server_browser::DEFAULT_MASTER_CVARS.map(str::to_owned),
            server_filters: String::new(),
        }
    }
}

/// Load local/POV presentation cvars. Stock OpenJK/TaystJK camera and mouse
/// controls live alongside DinurdoJK's presentation settings.
/// Unknown or malformed values retain the client defaults above.
pub fn load_client_presentation_settings(
    primary: &Path,
) -> ClientPresentationSettings {
    let mut settings = ClientPresentationSettings::default();
    let source = fs::read_to_string(primary).ok();
    let Some(text) = source else {
        return settings;
    };

    let text = strip_cfg_comments(&text);
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") || line.starts_with('#') {
            continue;
        }
        let words = split_cfg_words(line);
        if words.len() < 2 {
            continue;
        }
        let (name, value) = if matches!(
            words[0].to_ascii_lowercase().as_str(),
            "set" | "seta" | "sets" | "setu"
        ) {
            if words.len() < 3 {
                continue;
            }
            (words[1].as_str(), words[2].as_str())
        } else {
            (words[0].as_str(), words[1].as_str())
        };

        if let Some(load) = crate::app::cvar_definition(name).and_then(|def| def.config.load_presentation) { load(&mut settings, name, value); }
    }
    crate::ui::movement_components::normalize(&mut settings.strafe_helper.flags,
        &mut settings.japro.speedometer.flags);
    settings
}

pub fn load_video_settings(primary: &Path) -> VideoSettings {
    let mut settings = VideoSettings::default();
    let source = fs::read_to_string(primary).ok();
    let Some(text) = source else {
        return settings;
    };

    let mut context = cvars::VideoLoadState::default();
    let text = strip_cfg_comments(&text);
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") || line.starts_with('#') {
            continue;
        }
        let words = split_cfg_words(line);
        if words.len() < 2 {
            continue;
        }
        let (name, value) = if matches!(
            words[0].to_ascii_lowercase().as_str(),
            "set" | "seta" | "sets" | "setu"
        ) {
            if words.len() < 3 {
                continue;
            }
            (words[1].as_str(), words[2].as_str())
        } else {
            (words[0].as_str(), words[1].as_str())
        };

        if let Some(load) = crate::app::cvar_definition(name).and_then(|def| def.config.load_video) { load(&mut settings, name, value, &mut context); }
    }
    // Linked brightness sliders always mirror the master, whatever order the
    // config lines were read in.
    settings.sync_linked_brightness();
    // r_dynamicLights is the source of truth for all runtime-light techniques.
    if context.dynamic_shadows_mode_seen {
        settings.cascaded_shadows = settings.dynamic_shadows.is_csm();
    } else {
        settings.dynamic_shadows = if settings.cascaded_shadows {
            // Older boolean-only configs implied the full-scene CSM mode.
            DynamicShadowsMode::CascadedShadowMaps
        } else {
            DynamicShadowsMode::Off
        };
    }
    if settings.dynamic_shadows == DynamicShadowsMode::CascadedShadowMaps {
        settings.dynamic_shadows = DynamicShadowsMode::CascadedShadowMapsEntities;
        // Legacy full-scene behavior is retained by the new Relighting switch.
        // r_staticWorldCsm is archived after r_dynamicShadows in newly saved configs.
        // A separate flag below overrides this migration when present.
        if !context.static_world_csm_seen {
            settings.static_world_csm = true;
        }
    }

    settings.ocean_settings.wind = settings.weather_wind;
    if let (Some(width), Some(height)) = (context.custom_width, context.custom_height) {
        settings.resolution = [width, height];
    }
    if let (Some(x), Some(y)) = (context.window_x, context.window_y) {
        settings.window_position = Some([x, y]);
    }
    if context.novis == Some(true) {
        settings.pvs_mode = PvsMode::Off;
    } else if let Some(mode) = context.pvs_mode {
        settings.pvs_mode = mode;
    }
    if let Some(level) = context.anisotropy {
        settings.texture_filter = if level >= 12.0 {
            TextureFilter::Anisotropic16x
        } else if level >= 6.0 {
            TextureFilter::Anisotropic8x
        } else if level >= 3.0 {
            TextureFilter::Anisotropic4x
        } else if level > 1.0 {
            TextureFilter::Anisotropic2x
        } else {
            settings.texture_filter
        };
    }
    settings.ocean_settings = settings.ocean_settings.sanitize();
    settings
}

/// Group the archived settings by purpose without losing any registered archive
/// entry. Unknown/new cvars always remain in the Other section.
const CONFIG_SECTIONS: &[&str] = &[
    "Controls", "Mouse & Input", "Video & Display", "Textures & Materials",
    "World & Models", "Lighting & Shadows", "Postprocessing & Color",
    "Weather & Sky", "Ocean & Water", "Effects & Physics", "HUD & Crosshair",
    "Gameplay & Spectator", "Sound", "Network & Server Browser",
    "Console & Interface", "Diagnostics & Advanced", "Other",
];

fn config_section(name: &str) -> usize {
    let n = name.to_ascii_lowercase();
    let n = n.as_str();
    let matches = |needles: &[&str]| needles.iter().any(|needle| n.contains(*needle));
    if matches(&["sensitivity", "m_yaw", "m_pitch", "mouseaccel", "m_filter", "cl_input", "cl_timerresolution", "in_mouse"]) {
        1
    } else if matches(&["r_mode", "r_custom", "r_window", "r_fullscreen", "r_backend", "r_swapinterval", "r_maxframelatency", "r_earlysubmit", "r_presentcopy", "r_ext_multisample", "com_maxfps", "r_hdrpaperwhite", "r_hdrpeak", "r_hdrstyle"]) {
        2
    } else if matches(&["texture", "material", "anisotrop", "r_picmip", "r_pom", "r_parallax", "r_glass", "r_detail", "r_bump", "r_normalmap"]) {
        3
    } else if matches(&["r_ghoul2", "r_lod", "r_pvs", "r_novis", "r_faceplane", "r_world", "r_model", "r_grass", "r_terrain", "r_distancecull", "r_occlusion", "r_patch", "r_bsp", "r_foliage", "fs_allowasset"] ) {
        4
    } else if matches(&["r_sun", "r_shadow", "r_light", "r_rt", "r_ambient", "r_ssr", "r_reflect", "r_mirror", "r_cascade", "r_contactshadow", "r_staticbspao", "r_ssao", "r_entitysun"]) {
        5
    } else if matches(&["r_gamma", "r_hdr", "r_tonemap", "r_autoexposure", "r_glow", "r_bloom", "r_fxaa", "r_smaa", "r_taa", "r_halation", "r_color", "r_lut", "r_crt", "r_film", "r_saturation", "r_contrast", "r_vibrance", "r_exposure", "r_splittoning", "r_post", "r_brightness", "r_dof", "r_depthoffield", "r_motionblur"]) {
        6
    } else if matches(&["r_weather", "r_rain", "r_snow", "r_cloud", "r_fog", "r_sky", "r_wind", "r_puddle", "r_spacedust", "r_acidrain", "r_sand"]) {
        7
    } else if matches(&["r_ocean", "r_coastal", "r_water", "cg_waterwake", "r_caustic", "r_shore"]) {
        8
    } else if matches(&["r_fx", "cg_fx", "fx_", "r_footprint", "r_cloth", "r_jiggle", "r_ragdoll", "r_particle", "r_decal", "r_saber", "r_dismember", "r_physics"]) {
        9
    } else if matches(&["cg_hud", "cg_draw", "cg_crosshair", "cg_movementkeys", "cg_strafehelper", "cg_strafetrail", "cg_lagometer", "cg_racetimer", "cg_speedometer", "cg_chatbox", "cg_playername"]) {
        10
    } else if matches(&["cg_", "ui_vgs", "model", "name", "sex", "force", "handicap", "color1", "color2", "japro", "r_jumpheightshade"]) {
        11
    } else if n.starts_with("s_") || n.starts_with("snd_") {
        12
    } else if n.starts_with("cl_") || n.starts_with("snaps") || n.starts_with("rate") || n.starts_with("sv_master") || matches(&["net_", "rcon", "password", "protocol"]) {
        13
    } else if n.starts_with("con_") || n.starts_with("ui_") || matches(&["chatlog", "console"]) {
        14
    } else if matches(&["developer", "r_debug", "r_verbose", "r_show", "r_gpu", "r_perf", "r_pipeline", "r_wireframe", "com_", "fs_"]) {
        15
    } else {
        16
    }
}

fn concise_config_help(entry: &crate::console::Entry) -> String {
    let range = entry.range.trim();
    let description = entry.description.trim().trim_end_matches('.');
    // Preserve authored choice meanings whenever possible; descriptions in the
    // canonical registry often specify what 0/1/2/3 actually do.
    // A 0..1 domain may be a continuous slider (e.g. s_volume), not a
    // boolean. Do not invent off/on semantics for it.
    let domain = range;
    let source = if description.is_empty() { domain.to_owned() }
        else if domain.is_empty() { description.to_owned() }
        else { format!("{domain}; {description}") };
    // One line per setting, at most about two sentences. Truncate at a word
    // boundary, not in the middle of an enum/value token.
    const MAX_HELP_CHARS: usize = 176;
    if source.chars().count() <= MAX_HELP_CHARS { return source; }
    let boundary = source.char_indices().take_while(|(i, _)| *i <= MAX_HELP_CHARS)
        .filter(|(_, ch)| ch.is_whitespace()).map(|(i, _)| i).last();
    let end = boundary.unwrap_or_else(|| source.char_indices()
        .map(|(i, _)| i).take_while(|i| *i <= MAX_HELP_CHARS).last().unwrap_or(0));
    format!("{}…", source[..end].trim_end_matches(|ch: char| matches!(ch, ' ' | ',' | ';' | '.')))
}

fn format_archived_config(
    settings: VideoSettings,
    bindings: &crate::keybinds::Bindings,
    presentation: &ClientPresentationSettings,
    audio: &AudioSettings,
) -> String {
    use std::fmt::Write;
    let state = cvars::ArchiveState { video: &settings, presentation, audio };
    let mut sections: Vec<Vec<_>> = (0..CONFIG_SECTIONS.len()).map(|_| Vec::new()).collect();
    let mut definitions: Vec<_> = crate::app::cvar_definitions().iter()
        .filter(|def| def.runtime.flags.contains(crate::console::cvars::CvarFlags::ARCHIVE))
        .collect();
    // A newly registered cvar may accidentally claim ARCHIVE without providing
    // a snapshot getter. Do not panic on the background config writer: omit it
    // and report the registry error once, rather than spamming full backtraces.
    let unbound: Vec<_> = definitions.iter()
        .filter(|def| def.config.archive.is_none())
        .map(|def| def.runtime.entry.name)
        .collect();
    if !unbound.is_empty() {
        static WARN_ONCE: std::sync::Once = std::sync::Once::new();
        WARN_ONCE.call_once(|| eprintln!(
            "[CONFIG] Skipping archived cvar(s) without a config binding: {}. Fix the cvar registry.",
            unbound.join(", ")
        ));
        definitions.retain(|def| def.config.archive.is_some());
    }
    // Preserve declaration order for dependent values like r_novis/r_pvsMode.
    definitions.sort_by_key(|def| def.config.archive.map(|(order, _)| order).unwrap_or(usize::MAX));
    for def in definitions {
        if let Some((_, get)) = def.config.archive {
            sections[config_section(def.runtime.entry.name)].push((def.runtime.entry.name, get(&state)));
        }
    }
    let mut text = String::from("// DinurdoJK configuration - archived settings and bindings.\n");
    let _ = writeln!(text, "\n// ===== {} =====", CONFIG_SECTIONS[0]);
    bindings.write_cfg(&mut text);
    for (index, entries) in sections.iter().enumerate().skip(1) {
        if entries.is_empty() { continue; }
        let _ = writeln!(text, "\n// ===== {} =====", CONFIG_SECTIONS[index]);
        for (name, value) in entries {
            let _ = writeln!(text, "seta {name} \"{value}\"");
        }
    }
    text
}

/// Add opt-in explanations to an already flushed snapshot. This preserves the
/// exact serialized values (including latched/confirmed video settings) rather
/// than attempting to reconstruct them from currently mutable UI state.
fn bind_config_help(command: &str, spectator: bool) -> String {
    let mode = if spectator { "spectator override" } else { "gameplay" };
    if command.contains(';') {
        return format!("{mode}: command sequence");
    }
    let verb = command.split_whitespace().next().unwrap_or("");
    if verb.is_empty() {
        return format!("{mode}: empty bind");
    }
    if let Some(entry) = crate::console::find_command(verb, true) {
        let help = entry.description.trim().trim_end_matches('.');
        if !help.is_empty() {
            return format!("{mode}: {help}");
        }
    }
    format!("{mode}: run {verb}")
}

pub(crate) fn verbose_config_from_snapshot(snapshot: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(snapshot.len() * 2);
    for line in snapshot.lines() {
        if line.is_empty() || line.trim_start().starts_with("//") {
            let _ = writeln!(out, "{line}");
            continue;
        }
        let words = split_cfg_words(line);
        let note = match words.as_slice() {
            [verb, name, ..] if verb.eq_ignore_ascii_case("seta") =>
                crate::app::cvar_definition(name)
                    .map(|def| concise_config_help(&def.runtime.entry)),
            [verb, _, command, ..] if verb.eq_ignore_ascii_case("bind") => {
                Some(bind_config_help(command, false))
            }
            [verb, _, command, ..] if verb.eq_ignore_ascii_case("bindspec") => {
                Some(bind_config_help(command, true))
            }
            [verb] if verb.eq_ignore_ascii_case("unbindall") => Some("Clear normal bindings first".to_owned()),
            [verb] if verb.eq_ignore_ascii_case("unbindspecall") => Some("Clear spectator overrides first".to_owned()),
            _ => None,
        };
        if let Some(note) = note.filter(|note| !note.is_empty()) {
            let note = if note.chars().count() > 176 {
                let end = note.char_indices().filter(|(i, ch)| *i <= 173 && ch.is_whitespace())
                    .map(|(i, _)| i).last().unwrap_or_else(||
                        note.char_indices().map(|(i, _)| i).take_while(|i| *i <= 173).last().unwrap_or(0));
                format!("{}…", note[..end].trim_end())
            } else { note };
            let _ = writeln!(out, "{line} // {note}");
        } else {
            let _ = writeln!(out, "{line}");
        }
    }
    out
}

pub fn save_video_settings(
    path: &Path,
    settings: VideoSettings,
    bindings: &crate::keybinds::Bindings,
    presentation: &ClientPresentationSettings,
    audio: &AudioSettings,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let text = format_archived_config(settings, bindings, presentation, audio);
    fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn normalize_fps_cap(value: &str, fallback: u32) -> u32 {
    match value.trim().parse::<u32>() {
        Ok(0) => 0,
        Ok(cap) => cap.clamp(FPS_CAP_MIN, FPS_CAP_MAX),
        Err(_) => fallback,
    }
}


pub fn normalize_fx_fps_scope(value: &str, fallback: u32) -> u32 {
    value
        .trim()
        .parse::<u32>()
        .ok()
        .map(|scope| scope.min(crate::fx::FX_FPS_SCOPE_FRAME_DRIVEN))
        .unwrap_or(fallback)
}

pub fn normalize_fx_fps(value: &str, fallback: u32) -> u32 {
    match value.trim().parse::<u32>() {
        Ok(FX_FPS_LEGACY_JKA) => FX_FPS_LEGACY_JKA,
        Ok(value) => value.clamp(FX_FPS_MIN, FX_FPS_MAX),
        Err(_) => fallback,
    }
}

/// `fx_physics` 0..3. Stock level 1 ("non-expensive only") has no code path of
/// its own and behaves exactly like 0, so it is stored as 0.
pub fn normalize_fx_physics(value: &str, fallback: u32) -> u32 {
    match value.trim().parse::<u32>() {
        Ok(0 | 1) => 0,
        Ok(value) => value.min(3),
        Err(_) => fallback,
    }
}

/// `fx_lod` 0..2.
pub fn normalize_fx_lod(value: &str, fallback: u32) -> u32 {
    value
        .trim()
        .parse::<u32>()
        .map_or(fallback, |value| value.min(crate::fx::FX_LOD_ADAPTIVE))
}

/// `r_lodScale` / `r_fxLodScale`, kept in a sane positive range.
pub fn normalize_lod_scale(value: &str, fallback: f32) -> f32 {
    match value.trim().parse::<f32>() {
        Ok(value) if value.is_finite() => {
            value.clamp(crate::fx::LOD_SCALE_MIN, crate::fx::LOD_SCALE_MAX)
        }
        _ => fallback,
    }
}

/// `fx_countScale` 0..1 (stock never scales upward).
pub fn normalize_fx_count_scale(value: &str, fallback: f32) -> f32 {
    match value.trim().parse::<f32>() {
        Ok(value) if value.is_finite() => value.clamp(0.0, 1.0),
        _ => fallback,
    }
}

pub(crate) fn parse_rgba8(value: &str) -> Option<[u8; 4]> {
    let mut values = value.split_whitespace();
    let mut out = [0_u8; 4];
    for component in &mut out {
        let parsed = values.next()?.parse::<i32>().ok()?;
        *component = parsed.clamp(0, 255) as u8;
    }
    values.next().is_none().then_some(out)
}

pub(crate) fn parse_vec3(value: &str) -> Option<[f32; 3]> {
    let values = value
        .split_whitespace()
        .map(str::parse::<f32>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if values.len() != 3 || values.iter().any(|v| !v.is_finite()) {
        return None;
    }
    Some([values[0], values[1], values[2]])
}

pub(crate) fn parse_ocean_cascade(
    value: &str,
    current: crate::ocean::CascadeSettings,
) -> crate::ocean::CascadeSettings {
    let values = value
        .split_whitespace()
        .map(str::parse::<f32>)
        .collect::<Result<Vec<_>, _>>();
    let Ok(v) = values else {
        return current;
    };
    if v.len() != 12 || v.iter().any(|x| !x.is_finite()) {
        return current;
    }
    crate::ocean::CascadeSettings {
        tile_length: [v[0], v[1]],
        displacement_scale: v[2],
        normal_scale: v[3],
        wind_speed: v[4],
        wind_direction: v[5],
        fetch_length: v[6],
        swell: v[7],
        spread: v[8],
        detail: v[9],
        whitecap: v[10],
        foam_amount: v[11],
    }
}

pub(crate) fn parse_bool(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Some(true),
        "0" | "false" | "off" | "no" => Some(false),
        _ => None,
    }
}

/// Strip // and /* ... */ outside quotes, as in TaystJK Cbuf_Execute.
/// Also used by the native startup loaders, which do not execute `exec`.
/// Backslash-escaped quotes are supported for serialized bind arguments.
pub(crate) fn strip_cfg_comments(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut quoted = false;
    let mut escaped = false;
    let mut line_comment = false;
    let mut block_comment = false;
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        let next = chars.get(i + 1).copied();
        if line_comment {
            if ch == '\n' || ch == '\r' {
                out.push(ch);
                line_comment = false;
            }
            i += 1;
            continue;
        }
        if block_comment {
            if ch == '*' && next == Some('/') {
                block_comment = false;
                i += 2;
            } else {
                // Preserve line separation around multi-line comments.
                if ch == '\n' || ch == '\r' { out.push(ch); }
                i += 1;
            }
            continue;
        }
        if quoted && escaped {
            escaped = false;
        } else if quoted && ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            quoted = !quoted;
        } else if !quoted && ch == '/' && next == Some('/') {
            line_comment = true;
            i += 2;
            continue;
        } else if !quoted && ch == '/' && next == Some('*') {
            block_comment = true;
            i += 2;
            continue;
        }
        out.push(ch);
        i += 1;
    }
    out
}

fn split_cfg_words(line: &str) -> Vec<String> {
    let line = strip_cfg_comments(line);
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut token_started = false;
    for ch in line.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                token_started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if token_started {
                    out.push(std::mem::take(&mut current));
                    token_started = false;
                }
            }
            _ => {
                current.push(ch);
                token_started = true;
            }
        }
    }
    if token_started {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_launch_screen_round_trips_and_invalid_config_keeps_main_menu() {
        let path = std::env::temp_dir().join(format!(
            "jka-launch-screen-{}-{:?}.cfg", std::process::id(), std::thread::current().id()
        ));
        let mut presentation = ClientPresentationSettings::default();
        assert_eq!(presentation.game_launch_screen, GameLaunchScreen::Main);
        for &(screen, _, _) in GameLaunchScreen::OPTIONS {
            presentation.game_launch_screen = screen;
            save_video_settings(
                &path, VideoSettings::default(), &crate::keybinds::Bindings::default(),
                &presentation, &AudioSettings::default(),
            ).expect("save launch screen");
            assert_eq!(load_client_presentation_settings(&path).game_launch_screen, screen);
        }
        fs::write(&path, "seta ui_gameLaunchScreen \"obsolete-page\"\n").expect("write invalid screen");
        assert_eq!(load_client_presentation_settings(&path).game_launch_screen, GameLaunchScreen::Main);
        fs::write(&path, "seta ui_gameLaunchScreen \"SOLO\"\n").expect("write uppercase screen");
        assert_eq!(load_client_presentation_settings(&path).game_launch_screen, GameLaunchScreen::Solo);
        fs::write(&path, "// config from before launch-screen preference\n").expect("write old config");
        assert_eq!(load_client_presentation_settings(&path).game_launch_screen, GameLaunchScreen::Main);
        fs::remove_file(&path).expect("remove launch-screen test config");
    }

    #[test]
    fn cg_remaps_ignores_old_archived_config_on_startup() {
        let path = std::env::temp_dir().join(format!(
            "jka-remaps-session-{}-{:?}.cfg", std::process::id(), std::thread::current().id()
        ));
        // Existing configs may still contain cg_remaps from when it was archived.
        fs::write(&path, "seta cg_remaps \"0\"\n").expect("write legacy remaps config");
        assert_eq!(load_client_presentation_settings(&path).japro.remaps, 1);
        fs::remove_file(&path).expect("remove remaps test config");
    }

    #[test]
    fn config_comments_preserve_quoted_slashes_and_multiline_block_comments() {
        let input = concat!(
            "// heading\n",
            "seta cg_fov \"111\"// trailing note\n",
            "/* omitted;\n",
            "   seta cg_fov \"33\" */\n",
            "seta cg_rGhostDemoBaseUrl \"http://example.invalid/races\" // URL\n",
            "seta cg_fov /* between */ \"112\"\n",
        );
        let clean = strip_cfg_comments(input);
        assert_eq!(clean.matches("seta cg_fov").count(), 2);
        assert!(!clean.contains("33"));
        assert!(clean.contains("http://example.invalid/races"));
        assert_eq!(split_cfg_words("seta cg_fov \"110\"// help"), ["seta", "cg_fov", "110"]);
        assert_eq!(split_cfg_words("seta sv_master1 \"https://example.org:443/\" // help"),
            ["seta", "sv_master1", "https://example.org:443/"]);
    }

    #[test]
    fn organized_and_verbose_configs_restore_the_same_settings_and_binds() {
        let dir = std::env::temp_dir().join(format!(
            "jka-organized-config-{}-{:?}", std::process::id(), std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let normal = dir.join("normal.cfg");
        let verbose = dir.join("verbose.cfg");
        let mut video = VideoSettings::default();
        video.gamma = 2.25;
        let mut presentation = ClientPresentationSettings::default();
        presentation.fov = 111.0;
        presentation.mouse.sensitivity = 7.25;
        presentation.race_ghost_demo_base_url = "http://example.invalid/races".to_owned();
        let mut audio = AudioSettings::default();
        audio.effects_volume = 0.375;
        let bindings = crate::keybinds::Bindings::default();
        save_video_settings(&normal, video, &bindings, &presentation, &audio).expect("save");
        let compact = std::fs::read_to_string(&normal).expect("read");
        assert!(compact.contains("// ===== Controls ====="));
        assert!(compact.contains("// ===== Video & Display ====="));
        assert!(compact.contains("// ===== Sound ====="));
        assert!(compact.contains("// ===== Ocean & Water ====="));
        assert!(compact.contains("// ===== Mouse & Input ====="));
        assert!(compact.contains("unbindall"));
        assert!(compact.contains("unbindspecall"));
        assert_eq!(compact.lines().filter(|line| line.starts_with("seta ")).count(),
            crate::app::cvar_definitions().iter()
                .filter(|def| def.runtime.flags.contains(crate::console::cvars::CvarFlags::ARCHIVE))
                .count());

        let annotated = verbose_config_from_snapshot(&compact);
        assert!(annotated.contains("seta cg_fov \"111.000\" // "));
        assert!(annotated.contains("0..1; OpenJK game/effects volume"));
        assert!(annotated.lines().any(|line| line.starts_with("bind MOUSE1 ") && line.contains(" // ")));
        assert!(annotated.lines().any(|line| line.starts_with("bindspec ") && line.contains(" // ")));
        // Removing the explanations must produce the same executable lines.
        let commands = |text: &str| -> Vec<String> {
            strip_cfg_comments(text).lines().map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned).collect()
        };
        assert_eq!(commands(&compact), commands(&annotated));
        std::fs::write(&verbose, annotated).expect("write verbose");
        for path in [&normal, &verbose] {
            let loaded_video = load_video_settings(path);
            let loaded_presentation = load_client_presentation_settings(path);
            let loaded_audio = load_audio_settings(path);
            let loaded_binds = crate::keybinds::Bindings::load(path);
            assert!((loaded_video.gamma - 2.25).abs() < 0.0001);
            assert!((loaded_presentation.fov - 111.0).abs() < 0.0001);
            assert!((loaded_presentation.mouse.sensitivity - 7.25).abs() < 0.0001);
            assert_eq!(loaded_presentation.race_ghost_demo_base_url, "http://example.invalid/races");
            assert!((loaded_audio.effects_volume - 0.375).abs() < 0.0001);
            assert_eq!(loaded_binds.get(crate::keybinds::BindKey::Mouse(1)),
                bindings.get(crate::keybinds::BindKey::Mouse(1)));
            assert_eq!(loaded_binds.get_spectator(crate::keybinds::BindKey::Mouse(3)),
                bindings.get_spectator(crate::keybinds::BindKey::Mouse(3)));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gamma_methods_round_trip_and_ignore_obsolete_names() {
        use crate::gamma::GammaMethod;
        let path = std::env::temp_dir().join(format!(
            "dinurdojk-gamma-methods-{}-{:?}.cfg",
            std::process::id(),
            std::thread::current().id()
        ));
        let mut settings = VideoSettings::default();
        assert_eq!(settings.gamma_method, GammaMethod::Shader);
        settings.gamma = 2.25;
        for method in GammaMethod::ALL {
            settings.gamma_method = method;
            save_video_settings(
                &path,
                settings,
                &crate::keybinds::Bindings::default(),
                &ClientPresentationSettings::default(),
                &AudioSettings::default(),
            )
            .expect("save");
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(text.contains("seta r_gammaMethod"));
            assert!(!text.contains("r_bakedBrightness"));
            let loaded = load_video_settings(&path);
            assert_eq!(loaded.gamma_method, method);
            assert_eq!(loaded.gamma, 2.25);
        }
        for (text, expected) in [
            ("seta r_gamma \"1.5\"\n", GammaMethod::Shader),
            ("seta r_bakedBrightness \"invalid\"\n", GammaMethod::Shader),
            ("seta r_gammaMethod \"invalid\"\n", GammaMethod::Shader),
            ("seta r_bakedBrightness \"1\"\n", GammaMethod::Shader),
            ("seta r_gammaMethod \"2\"\n", GammaMethod::Hardware),
            ("seta r_gammaMethod \"3\"\n", GammaMethod::HardwareSceneOnly),
            (
                "seta r_bakedBrightness \"1\"\nseta r_gammaMethod \"hardware\"\n",
                GammaMethod::Hardware,
            ),
            (
                "seta r_gammaMethod \"shader\"\nseta r_bakedBrightness \"1\"\n",
                GammaMethod::Shader,
            ),
        ] {
            std::fs::write(&path, text).unwrap();
            assert_eq!(
                load_video_settings(&path).gamma_method,
                expected,
                "{text}"
            );
        }
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn split_toning_round_trips_and_defaults_off() {
        let path = std::env::temp_dir().join(format!(
            "dinurdojk-color-{}-{:?}.cfg",
            std::process::id(),
            std::thread::current().id()
        ));
        let mut settings = VideoSettings::default();
        assert!(!settings.split_toning.enabled);
        settings.split_toning = crate::color_grading::SplitToningSettings {
            enabled: true,
            strength: 0.375,
            shadow_hue: 225.0,
            shadow_saturation: 0.625,
            highlight_hue: 35.0,
            highlight_saturation: 0.125,
            balance: -0.25,
        };
        settings.color_lut = ColorLutPreset::KodakPortra400;
        settings.color_lut_strength = 0.25;
        settings.color_grade = crate::color_grading::ColorGradeSettings {
            exposure_ev: 0.75,
            contrast: 1.15,
            saturation: 0.8,
            vibrance: 0.35,
            temperature: 7200.0,
            tint: -0.2,
            shadows: 45.0,
            highlights: -35.0,
        };
        save_video_settings(
            &path,
            settings,
            &crate::keybinds::Bindings::default(),
            &ClientPresentationSettings::default(),
            &AudioSettings::default(),
        )
        .expect("save");
        let loaded = load_video_settings(&path);
        assert_eq!(loaded.split_toning, settings.split_toning);
        assert_eq!(loaded.color_lut, settings.color_lut);
        assert_eq!(loaded.color_lut_strength, settings.color_lut_strength);
        assert_eq!(loaded.color_grade, settings.color_grade);
        std::fs::write(
            &path,
            "seta r_splitToningStrength \"NaN\"\nseta r_splitToningShadowHue \"inf\"\n",
        )
        .unwrap();
        let loaded = load_video_settings(&path);
        assert_eq!(loaded.split_toning, VideoSettings::default().split_toning);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn static_csm_relighting_setting_and_legacy_csm_migrate() {
        use crate::ui::DynamicShadowsMode;
        let path = std::env::temp_dir().join(format!(
            "jka-static-world-csm-{}-{:?}.cfg",
            std::process::id(), std::thread::current().id()
        ));
        // The former full-scene CSM should retain its static caster setting.
        std::fs::write(&path, "seta r_dynamicShadows \"csm\"\nseta r_relightMap \"1\"\n").unwrap();
        let legacy = load_video_settings(&path);
        assert_eq!(legacy.dynamic_shadows, DynamicShadowsMode::CascadedShadowMapsEntities);
        assert!(legacy.static_world_csm);
        assert_eq!(legacy.effective_shadow_mode(), DynamicShadowsMode::CascadedShadowMaps);

        // New independent switch overrides legacy full-scene configs.
        std::fs::write(&path, "seta r_dynamicShadows \"csm\"\nseta r_staticWorldCsm \"0\"\nseta r_relightMap \"1\"\n").unwrap();
        let override_off = load_video_settings(&path);
        assert!(!override_off.static_world_csm);
        assert_eq!(override_off.effective_shadow_mode(), DynamicShadowsMode::CascadedShadowMapsEntities);

        // New entity-only CSM remains entity-only by default.
        std::fs::write(&path, "seta r_dynamicShadows \"csm_entities\"\nseta r_relightMap \"1\"\n").unwrap();
        assert!(!load_video_settings(&path).static_world_csm);

        // Save and reload two independent controls.
        let mut saved = VideoSettings::default();
        saved.sun_override = true;
        saved.dynamic_shadows = DynamicShadowsMode::CascadedShadowMapsEntities;
        saved.static_world_csm = true;
        save_video_settings(
            &path, saved, &crate::keybinds::Bindings::default(),
            &ClientPresentationSettings::default(), &AudioSettings::default(),
        ).unwrap();
        let round_trip = load_video_settings(&path);
        assert!(round_trip.static_world_csm);
        assert_eq!(round_trip.dynamic_shadows, DynamicShadowsMode::CascadedShadowMapsEntities);
        assert_eq!(round_trip.effective_shadow_mode(), DynamicShadowsMode::CascadedShadowMaps);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn cloth_controls_survive_config_round_trip() {
        let path =
            std::env::temp_dir().join(format!("jka-cloth-controls-{}.cfg", std::process::id()));
        let mut settings = VideoSettings::default();
        settings.cloth_air_resistance = 2.25;
        settings.cloth_turn_response = 2.75;
        settings.cloth_animation_influence = 0.2;
        settings.cloth_wind = true;
        save_video_settings(
            &path,
            settings,
            &crate::keybinds::Bindings::default(),
            &ClientPresentationSettings::default(),
            &AudioSettings::default(),
        )
        .unwrap();
        let loaded = load_video_settings(&path);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(loaded.cloth_air_resistance, settings.cloth_air_resistance);
        assert_eq!(loaded.cloth_turn_response, settings.cloth_turn_response);
        assert_eq!(
            loaded.cloth_animation_influence,
            settings.cloth_animation_influence
        );
        assert_eq!(loaded.cloth_wind, settings.cloth_wind);
    }

    #[test]
    fn ocean_authoring_cfg_roundtrip_preserves_units_and_seed() {
        let path = std::env::temp_dir().join(format!("ocean-authoring-{}.cfg", std::process::id()));
        let mut settings = VideoSettings::default();
        settings.ocean_settings.authored = crate::ocean::OceanAuthoring {
            amplitude: 512.0,
            wavelength: 8192.0,
            direction: -75.0,
            seed: u32::MAX,
            wind_chop: 2.5,
            foam_lifetime: 12.0,
            ..Default::default()
        };
        settings.weather_wind = crate::ocean::AtmosphericWind {
            speed: 800.0,
            direction: 160.0,
            gust: 0.5,
            shift: 30.0,
        };
        settings.ocean_settings.wind = settings.weather_wind;
        save_video_settings(
            &path,
            settings,
            &crate::keybinds::Bindings::default(),
            &ClientPresentationSettings::default(),
            &AudioSettings::default(),
        )
        .unwrap();
        let loaded = load_video_settings(&path);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            loaded.ocean_settings.authored,
            settings.ocean_settings.authored
        );
        assert_eq!(loaded.ocean_settings.wind, settings.ocean_settings.wind);
    }

    #[test]
    fn diagnostic_levels_round_trip() {
        let path = std::env::temp_dir().join(format!(
            "jka-diagnostic-levels-{}-{:?}.cfg",
            std::process::id(),
            std::thread::current().id()
        ));
        let mut settings = VideoSettings::default();
        settings.developer_level = 2;
        settings.developer_tools = true;
        settings.renderer_verbose = 3;
        save_video_settings(
            &path,
            settings,
            &crate::keybinds::Bindings::default(),
            &ClientPresentationSettings::default(),
            &AudioSettings::default(),
        )
        .expect("save");
        let loaded = load_video_settings(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(loaded.developer_level, 2);
        assert!(loaded.developer_tools);
        assert_eq!(loaded.renderer_verbose, 3);
    }

    #[test]
    fn cfg_words_preserve_empty_quoted_values() {
        assert_eq!(
            split_cfg_words(r#"seta sv_master1 """#),
            vec!["seta", "sv_master1", ""]
        );
    }

    #[test]
    fn master_server_cvars_round_trip_disabled_and_custom_slots() {
        let dir = std::env::temp_dir().join(format!(
            "jka-master-cfg-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("test.cfg");
        let mut presentation = ClientPresentationSettings::default();
        presentation.chat_log = true;
        presentation.master_servers[0].clear();
        presentation.master_servers[3] = "custom.example.org:29061".to_owned();
        save_video_settings(
            &path,
            VideoSettings::default(),
            &crate::keybinds::Bindings::default(),
            &presentation,
            &AudioSettings::default(),
        )
        .expect("save");
        let loaded = load_client_presentation_settings(&path);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(loaded.chat_log);
        assert_eq!(loaded.master_servers[0], "");
        assert_eq!(loaded.master_servers[1], "master.jkhub.org");
        assert_eq!(loaded.master_servers[2], "master.ouned.de");
        assert_eq!(loaded.master_servers[3], "custom.example.org:29061");
        assert_eq!(loaded.master_servers[4], "");
    }

    #[test]
    fn audio_settings_load_and_save_openjk_cvars() {
        let dir = std::env::temp_dir().join(format!(
            "jka-audio-cfg-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("test.cfg");
        let audio = AudioSettings {
            effects_volume: 0.375,
            variometer: true,
            variometer_style: 1,
            variometer_input: 1,
            variometer_volume: 0.425,
            voice_volume: 0.625,
            music_volume: 0.125,
            separation: 0.75,
            mute_when_unfocused: false,
            steam_audio: true,
            eal_hrtf: true,
            game: GameOptions {
                jump: 3,
                roll: 2,
                no_taunt: true,
                duel: 2,
                kill: 1,
                kill_message: 3,
                draw_rewards: 2,
                hit: 4,
                duel_music: false,
                ambient: false,
                blood: 2,
                auto_switch: 2,
                score_plums: false,
                g2_marks: 4,
                race_sounds: 0,
                chat_sounds: 2,
                footsteps: 1,
            },
            ..AudioSettings::default()
        };
        save_video_settings(
            &path,
            VideoSettings::default(),
            &crate::keybinds::Bindings::default(),
            &ClientPresentationSettings::default(),
            &audio,
        )
        .expect("save");
        let loaded = load_audio_settings(&path);
        let text = std::fs::read_to_string(&path).expect("read");
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(loaded, audio);
        assert!(text.contains("seta s_ealHrtf \"1\""));
        assert!(text.contains("seta s_volume \"0.375\""));
        assert!(text.contains("seta s_variometer \"1\""));
        assert!(text.contains("seta s_variometerStyle \"1\""));
        assert!(text.contains("seta s_variometerInput \"1\""));
        assert!(text.contains("seta s_variometerVolume \"0.425\""));
        assert!(text.contains("seta s_volumeVoice \"0.625\""));
        assert!(text.contains("seta s_musicvolume \"0.125\""));
        assert!(text.contains("seta s_separation \"0.750\""));
        assert!(text.contains("seta s_muteWhenUnfocused \"0\""));
        assert!(text.contains("seta s_steamAudio \"1\""));
        assert!(text.contains("seta s_steamAudioEnvironmental \"1\""));
        assert!(text.contains("seta cg_jumpSounds \"3\""));
        assert!(text.contains("seta cg_noTaunt \"1\""));
        assert!(text.contains("seta cg_killMessage \"3\""));
        assert!(text.contains("seta cg_drawRewards \"2\""));
        assert!(text.contains("seta cg_hitsounds \"4\""));
    }

    #[test]
    fn ghoul2_render_settings_survive_a_config_round_trip() {
        let mut settings = VideoSettings::default();
        settings.ghoul2_skinning = Ghoul2SkinningMode::CpuWorkers;
        settings.ghoul2_early_cull = false;
        settings.ghoul2_lod_bias = 2;
        settings.ghoul2_batch_draws = Ghoul2BatchMode::Force;
        settings.ghoul2_anim_smooth = 0.3;
        settings.emote_cubic = false;

        let dir = std::env::temp_dir().join(format!(
            "jka-ghoul2-cfg-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("test.cfg");
        save_video_settings(
            &path,
            settings,
            &crate::keybinds::Bindings::default(),
            &ClientPresentationSettings::default(),
            &AudioSettings::default(),
        )
        .expect("save");
        let loaded = load_video_settings(&path);
        let text = std::fs::read_to_string(&path).expect("read");
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(loaded.ghoul2_skinning, Ghoul2SkinningMode::CpuWorkers);
        assert!(!loaded.ghoul2_early_cull);
        assert_eq!(loaded.ghoul2_lod_bias, 2);
        assert_eq!(loaded.ghoul2_batch_draws, Ghoul2BatchMode::Force);
        assert_eq!(loaded.ghoul2_anim_smooth, 0.3);
        assert!(!loaded.emote_cubic);
        assert!(text.contains("seta r_ghoul2Skinning \"workers\""));
        assert!(text.contains("seta r_ghoul2EarlyCull \"0\""));
        assert!(text.contains("seta r_lodbias \"2\""));
        assert!(text.contains("seta r_ghoul2BatchDraws \"2\""));
        assert!(text.contains("seta r_ghoul2AnimSmooth \"0.3\""));
        assert!(text.contains("seta cg_emoteCubic \"0\""));
    }

    #[test]
    fn rt_samples_survive_config_round_trip_and_reject_invalid_values() {
        let dir = std::env::temp_dir().join(format!(
            "jka-rt-samples-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.cfg");
        for samples in [1, 2, 4] {
            let mut settings = VideoSettings::default();
            settings.rt_samples = samples;
            settings.rt_trace_rate = samples;
            save_video_settings(
                &path,
                settings,
                &crate::keybinds::Bindings::default(),
                &ClientPresentationSettings::default(),
                &AudioSettings::default(),
            )
            .unwrap();
            assert_eq!(load_video_settings(&path).rt_samples, samples);
            assert_eq!(
                load_video_settings(&path).rt_trace_rate,
                samples
            );
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("r_rtTraceRate"));
        assert!(!text.contains("r_rtResolution"));
        for value in ["0", "3", "999", "-1", "NaN"] {
            std::fs::write(&path, format!("seta r_rtSamples \"{value}\"\n")).unwrap();
            assert_eq!(load_video_settings(&path).rt_samples, 2);
        }
        for value in ["0", "3", "-1", "0.5", "NaN"] {
            std::fs::write(&path, format!("seta r_rtTraceRate \"{value}\"\n")).unwrap();
            assert_eq!(load_video_settings(&path).rt_trace_rate, 2);
        }
        for (value, rate) in [
            ("full", 1),
            ("half", 4),
            ("1", 1),
            ("0.5", 4),
            ("0.25", 2),
            ("invalid", 2),
        ] {
            std::fs::write(&path, format!("seta r_rtResolution \"{value}\"\n")).unwrap();
            assert_eq!(load_video_settings(&path).rt_trace_rate, rate);
        }
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn cloud_tuning_settings_survive_a_config_round_trip() {
        // Every cloud tuning knob has to appear in both the writer and the
        // parser. A knob present in only one silently resets to its default on
        // the next launch, which is very hard to notice while A/B testing.
        let mut settings = VideoSettings::default();
        settings.cloud_temporal_depth_fix = false;
        settings.cloud_shear = 0.75;
        settings.cloud_base_variation = 0.125;
        settings.weather_wind = crate::ocean::AtmosphericWind {
            speed: 720.0,
            direction: 135.0,
            gust: 0.6,
            shift: 28.0,
        };
        settings.wind_visuals = 0.67;
        settings.cloud_shape_evolution = true;
        settings.cloud_terrain_interaction = true;
        settings.cloud_empty_skip = true;
        settings.cloud_aerial = 0.25;
        settings.cloud_sky_ambient = false;
        settings.cloud_history_blend = 0.5;
        settings.cloud_motion_reject = 1.0;
        settings.cloud_history_depth_reject = false;
        settings.cloud_thickness_variation = 0.875;
        settings.cloud_size = 0.625;

        let dir = std::env::temp_dir().join(format!(
            "jka-cloud-cfg-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("test.cfg");
        save_video_settings(
            &path,
            settings,
            &crate::keybinds::Bindings::default(),
            &ClientPresentationSettings::default(),
            &AudioSettings::default(),
        )
        .expect("save");
        let loaded = load_video_settings(&path);
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(loaded.cloud_temporal_depth_fix, false);
        assert!((loaded.cloud_shear - 0.75).abs() < 1e-3);
        assert!((loaded.cloud_base_variation - 0.125).abs() < 1e-3);
        assert_eq!(loaded.weather_wind, settings.weather_wind);
        assert!((loaded.wind_visuals - 0.67).abs() < 1e-3);
        assert_eq!(loaded.cloud_shape_evolution, true);
        assert_eq!(loaded.cloud_terrain_interaction, true);
        assert_eq!(loaded.cloud_empty_skip, true);
        assert!((loaded.cloud_aerial - 0.25).abs() < 1e-3);
        assert_eq!(loaded.cloud_sky_ambient, false);
        assert!((loaded.cloud_history_blend - 0.5).abs() < 1e-3);
        assert!((loaded.cloud_motion_reject - 1.0).abs() < 1e-3);
        assert_eq!(loaded.cloud_history_depth_reject, false);
        assert!((loaded.cloud_thickness_variation - 0.875).abs() < 1e-3);
        assert!((loaded.cloud_size - 0.625).abs() < 1e-3);
    }

    #[test]
    fn asset_override_policy_survives_a_config_round_trip() {
        let mut settings = VideoSettings::default();
        settings.allow_asset_overrides = false;

        let dir = std::env::temp_dir().join(format!(
            "jka-asset-overrides-cfg-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("test.cfg");
        save_video_settings(
            &path,
            settings,
            &crate::keybinds::Bindings::default(),
            &ClientPresentationSettings::default(),
            &AudioSettings::default(),
        )
        .expect("save");
        let loaded = load_video_settings(&path);
        let _ = std::fs::remove_dir_all(&dir);

        assert!(!loaded.allow_asset_overrides);
    }

    #[test]
    fn sun_settings_survive_a_config_round_trip() {
        let mut settings = VideoSettings::default();
        settings.sun_override = true;
        settings.sun_yaw = 123.5;
        settings.sun_pitch = -17.25;
        settings.sun_intensity = 777.0;
        settings.sun_color = [1.0, 0.625, 0.25];
        settings.sun_visibility = SunVisibilityMode::Filtered;
        settings.entity_sun_lighting = true;

        let dir = std::env::temp_dir().join(format!(
            "jka-sun-cfg-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("test.cfg");
        save_video_settings(
            &path,
            settings,
            &crate::keybinds::Bindings::default(),
            &ClientPresentationSettings::default(),
            &AudioSettings::default(),
        )
        .expect("save");
        let loaded = load_video_settings(&path);
        let _ = std::fs::remove_dir_all(&dir);

        assert!(loaded.sun_override);
        assert!((loaded.sun_yaw - 123.5).abs() < 1e-3);
        assert!((loaded.sun_pitch + 17.25).abs() < 1e-3);
        assert!((loaded.sun_intensity - 777.0).abs() < 1e-3);
        assert_eq!(loaded.sun_color, [1.0, 0.625, 0.25]);
        assert_eq!(loaded.sun_visibility, SunVisibilityMode::Filtered);
        assert!(loaded.entity_sun_lighting);
    }

    #[test]
    fn third_person_loader_accepts_stock_openjk_cvars() {
        let dir = std::env::temp_dir().join(format!(
            "jka-thirdperson-load-cfg-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("test.cfg");
        std::fs::write(
            &path,
            concat!(
                "seta model \"rebel/default\"\n",
                "seta cg_forceModel \"rebel/default,stormtrooper/default\"\n",
                "seta sensitivity \"7.25\"\n",
                "seta m_yaw \"0.031\"\n",
                "seta m_pitch \"-0.019\"\n",
                "seta cl_mouseAccel \"0.45\"\n",
                "seta cg_fov \"110\"\n",
                "seta cg_thirdPerson \"2\"\n",
                "seta cg_specCameraMotion \"1\"\n",
                "seta cg_specOrbitRange \"220\"\n",
                "seta cg_fpls \"0\"\n",
                "seta r_jumpHeightShade \"1\"\n",
                "seta cg_saberTrail \"2\"\n",
                "seta cg_thirdPersonAlpha \"0.375\"\n",
                "seta cg_thirdPersonAngle \"25\"\n",
                "seta cg_thirdPersonCameraDamp \"0.42\"\n",
                "seta cg_thirdPersonHorzOffset \"7\"\n",
                "seta cg_thirdPersonPitchOffset \"-4\"\n",
                "seta cg_thirdPersonRange \"128\"\n",
                "seta cg_thirdPersonSpecialCam \"1\"\n",
                "seta cg_thirdPersonTargetDamp \"0.61\"\n",
                "seta cg_thirdPersonVertOffset \"21\"\n",
                "seta cg_drawCrosshair \"7\"\n",
                "seta cg_crosshairImage \"99\"\n",
                "seta cg_crosshairSize \"36\"\n",
                "seta cg_crosshairColor \"64 128 255 200\"\n",
                "seta cg_hudHealth \"bl 40 -72 1.25\"\n",
                "seta cg_hudShield \"bc -120 -40 0.75\"\n",
                "seta cg_hudAmmo \"br -32 -72 1.5\"\n",
                "seta cg_hudForce \"c 180 120 0.9\"\n",
                "seta cg_hudMovementKeys \"tl 30 -12 1.5\"\n",
                "seta cg_hudSnap \"0\"\n",
                "seta cg_hudGridSize \"12\"\n",
            ),
        )
        .expect("write");

        let loaded = load_client_presentation_settings(&path);
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(loaded.model, "rebel/default");
        assert_eq!(loaded.force_model, "rebel/default,stormtrooper/default");
        assert!((loaded.mouse.sensitivity - 7.25).abs() < 1e-6);
        assert!((loaded.mouse.yaw - 0.031).abs() < 1e-6);
        assert!((loaded.mouse.pitch + 0.019).abs() < 1e-6);
        assert!((loaded.mouse.accel - 0.45).abs() < 1e-6);
        assert!((loaded.fov - 110.0).abs() < 1e-6);
        assert!(loaded.third_person.enabled);
        assert_eq!(loaded.spectator_camera.mode, SpectatorCameraMode::Orbit);
        assert!(loaded.spectator_camera.motion_direction);
        assert!((loaded.spectator_camera.orbit_range - 220.0).abs() < 1e-6);
        assert!(!loaded.first_person_lightsaber);
        assert!(loaded.jump_height_shade);
        assert_eq!(loaded.saber_trail, 2);
        assert!((loaded.third_person.alpha - 0.375).abs() < 1e-6);
        assert!((loaded.third_person.angle - 25.0).abs() < 1e-6);
        assert!((loaded.third_person.camera_damp - 0.42).abs() < 1e-6);
        assert!((loaded.third_person.horz_offset - 7.0).abs() < 1e-6);
        assert!((loaded.third_person.pitch_offset + 4.0).abs() < 1e-6);
        assert!((loaded.third_person.range - 128.0).abs() < 1e-6);
        assert!(loaded.third_person.special_cam);
        assert!((loaded.third_person.target_damp - 0.61).abs() < 1e-6);
        assert!((loaded.third_person.vert_offset - 21.0).abs() < 1e-6);
        assert_eq!(loaded.crosshair.style, crate::ui::CROSSHAIR_STYLE_LINE);
        // An out-of-range image id is clamped to the last image crosshair.
        assert_eq!(loaded.crosshair.image, crate::ui::CROSSHAIR_IMAGE_COUNT);
        assert!((loaded.crosshair.size - 36.0).abs() < 1e-6);
        assert_eq!(loaded.crosshair.color, [64, 128, 255, 200]);
        assert_eq!(
            loaded.hud_layout.health.anchor,
            crate::ui::HudAnchor::BottomLeft
        );
        assert_eq!(loaded.hud_layout.health.offset, [40.0, -72.0]);
        assert!((loaded.hud_layout.health.scale - 1.25).abs() < 1e-6);
        assert_eq!(
            loaded.hud_layout.shield.anchor,
            crate::ui::HudAnchor::BottomCenter
        );
        assert_eq!(loaded.hud_layout.force.anchor, crate::ui::HudAnchor::Center);
        assert_eq!(loaded.hud_layout.movement_keys.offset, [30.0, -12.0]);
        assert!((loaded.hud_layout.movement_keys.scale - 1.5).abs() < 1e-6);
        assert!(!loaded.hud_layout.snap_to_grid);
        assert!((loaded.hud_layout.grid_size - 12.0).abs() < 1e-6);
    }

    #[test]
    fn client_presentation_writer_archives_camera_choices() {
        let dir = std::env::temp_dir().join(format!(
            "jka-thirdperson-save-cfg-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("test.cfg");
        let mut presentation = ClientPresentationSettings::default();
        presentation.model = "kyle/default".to_owned();
        presentation.force_model = "rebel/default,stormtrooper/default".to_owned();
        presentation.mouse.sensitivity = 6.5;
        presentation.mouse.yaw = 0.025;
        presentation.mouse.pitch = -0.022;
        presentation.mouse.accel = 0.3;
        presentation.fov = 105.0;
        presentation.third_person.enabled = true;
        presentation.spectator_camera.mode = SpectatorCameraMode::ThirdPerson;
        presentation.spectator_camera.motion_direction = true;
        presentation.spectator_camera.orbit_range = 256.0;
        presentation.first_person_lightsaber = false;
        presentation.jump_height_shade = true;
        presentation.saber_trail = 0;
        presentation.third_person.alpha = 0.25;
        presentation.third_person.angle = 15.0;
        presentation.third_person.camera_damp = 0.8;
        presentation.third_person.horz_offset = 6.0;
        presentation.third_person.pitch_offset = -3.0;
        presentation.third_person.range = 96.0;
        presentation.third_person.special_cam = true;
        presentation.third_person.target_damp = 0.7;
        presentation.third_person.vert_offset = 18.0;
        presentation.crosshair.style = 4;
        presentation.crosshair.image = 7;
        presentation.crosshair.size = 32.0;
        presentation.crosshair.color = [20, 140, 255, 210];
        presentation.hud_layout.health = HudElementLayout {
            anchor: crate::ui::HudAnchor::TopLeft,
            offset: [48.0, 64.0],
            scale: 1.25,
            extent: [1.0, 1.0],
        };
        presentation.hud_layout.fps = HudElementLayout {
            anchor: crate::ui::HudAnchor::TopLeft,
            offset: [-20.0, 10.0],
            scale: 0.75,
            extent: [1.0, 1.0],
        };
        presentation.hud_layout.snap_to_grid = false;
        presentation.hud_layout.grid_size = 16.0;

        save_video_settings(
            &path,
            VideoSettings::default(),
            &crate::keybinds::Bindings::default(),
            &presentation,
            &AudioSettings::default(),
        )
        .expect("save");
        let text = std::fs::read_to_string(&path).expect("read");
        let _ = std::fs::remove_dir_all(&dir);

        assert!(text.contains("seta model \"kyle/default\""));
        assert!(text.contains("seta cg_forceModel \"rebel/default,stormtrooper/default\""));
        assert!(text.contains("seta sensitivity \"6.500000\""));
        assert!(text.contains("seta m_yaw \"0.025000\""));
        assert!(text.contains("seta m_pitch \"-0.022000\""));
        assert!(text.contains("seta cl_mouseAccel \"0.300000\""));
        assert!(text.contains("seta cg_fov \"105.000\""));
        assert!(text.contains("seta cg_thirdPerson \"1\""));
        assert!(text.contains("seta cg_specCameraMotion \"1\""));
        assert!(text.contains("seta cg_specOrbitRange \"256.000\""));
        assert!(text.contains("seta cg_fpls \"0\""));
        assert!(text.contains("seta r_jumpHeightShade \"1\""));
        assert!(text.contains("seta cg_saberTrail \"0\""));
        assert!(text.contains("seta cg_fxFPS \"90\""));
        assert!(text.contains("seta cg_thirdPersonAlpha \"0.250\""));
        assert!(text.contains("seta cg_thirdPersonAngle \"15.000\""));
        assert!(text.contains("seta cg_thirdPersonCameraDamp \"0.800\""));
        assert!(text.contains("seta cg_thirdPersonHorzOffset \"6.000\""));
        assert!(text.contains("seta cg_thirdPersonPitchOffset \"-3.000\""));
        assert!(text.contains("seta cg_thirdPersonRange \"96.000\""));
        assert!(text.contains("seta cg_thirdPersonTargetDamp \"0.700\""));
        assert!(text.contains("seta cg_thirdPersonVertOffset \"18.000\""));
        assert!(text.contains("seta cg_drawCrosshair \"4\""));
        assert!(text.contains("seta cg_crosshairImage \"7\""));
        assert!(text.contains("seta cg_crosshairSize \"32.000\""));
        assert!(text.contains("seta cg_crosshairColor \"20 140 255 210\""));
        assert!(text.contains("seta cg_hudHealth \"tl 48.000 64.000 1.250 1.000 1.000\""));
        assert!(text.contains("seta cg_hudFps \"tl -20.000 10.000 0.750 1.000 1.000\""));
        assert!(text.contains("seta cg_hudChat \"tl 0.000 0.000 1.000 1.000 1.000\""));
        assert!(text.contains("seta cg_hudSnap \"0\""));
        assert!(text.contains("seta cg_hudGridSize \"16.000\""));

        // cg_thirdPersonSpecialCam remains the TaystJK runtime-only exception.
        // cg_fpls is archived; cg_thirdPersonSpecialCam remains runtime-only.
        assert!(!text.contains("seta cg_thirdPersonSpecialCam"));
    }

    #[test]
    fn client_presentation_defaults_include_dinurdo_view_choices() {
        let settings = ClientPresentationSettings::default();
        assert_eq!(settings.model, "kyle");
        assert!((settings.fov - DEFAULT_CG_FOV).abs() < 1e-6);
        assert!(!settings.third_person.enabled);
        assert_eq!(
            settings.spectator_camera,
            SpectatorCameraSettings::default()
        );
        assert!(settings.first_person_lightsaber);
        assert_eq!(settings.saber_trail, 1);
        assert_eq!(settings.crosshair, CrosshairSettings::default());
        assert!((settings.third_person.alpha - 1.0).abs() < 1e-6);
        assert!((settings.third_person.angle - 0.0).abs() < 1e-6);
        assert!((settings.third_person.camera_damp - 1.0).abs() < 1e-6);
        assert!((settings.third_person.horz_offset - 0.0).abs() < 1e-6);
        assert!((settings.third_person.pitch_offset - 0.0).abs() < 1e-6);
        assert!((settings.third_person.range - 100.0).abs() < 1e-6);
        assert!(!settings.third_person.special_cam);
        assert!((settings.third_person.target_damp - 1.0).abs() < 1e-6);
        assert!((settings.third_person.vert_offset - 16.0).abs() < 1e-6);
        assert!((settings.mouse.sensitivity - 5.0).abs() < 1e-6);
        assert!((settings.mouse.yaw - 0.022).abs() < 1e-6);
        assert!((settings.mouse.pitch - 0.022).abs() < 1e-6);
        assert_eq!(settings.mouse.accel, 0.0);
    }

    #[test]
    fn parses_jka_style_lines() {
        let words = split_cfg_words("seta r_textureMode \"GL_LINEAR_MIPMAP_LINEAR\"");
        assert_eq!(words, ["seta", "r_textureMode", "GL_LINEAR_MIPMAP_LINEAR"]);
    }

    #[test]
    fn fx_lod_defaults_to_adaptive_and_clamps() {
        assert_eq!(VideoSettings::default().fx_lod, crate::fx::FX_LOD_ADAPTIVE);
        assert_eq!(VideoSettings::default().fx_count_scale, 1.0);
        assert_eq!(VideoSettings::default().fx_lod_scale, 5.0);
        assert_eq!(VideoSettings::default().lod_scale, 5.0);
        assert_eq!(normalize_fx_lod("0", 2), 0);
        assert_eq!(normalize_fx_lod("9", 0), 2);
        assert_eq!(normalize_fx_lod("bad", 1), 1);
        assert_eq!(normalize_fx_count_scale("0.5", 1.0), 0.5);
        assert_eq!(normalize_fx_count_scale("3", 1.0), 1.0);
        assert_eq!(normalize_fx_count_scale("nan", 0.7), 0.7);
        assert_eq!(normalize_lod_scale("2.5", 5.0), 2.5);
        assert_eq!(normalize_lod_scale("0", 5.0), crate::fx::LOD_SCALE_MIN);
        assert_eq!(normalize_lod_scale("bad", 5.0), 5.0);
    }

    #[test]
    fn fx_physics_defaults_to_authored_and_folds_level_one_into_off() {
        assert_eq!(
            VideoSettings::default().fx_physics,
            crate::fx::FX_PHYSICS_AUTHORED
        );
        assert_eq!(normalize_fx_physics("0", 2), 0);
        assert_eq!(normalize_fx_physics("1", 2), 0);
        assert_eq!(normalize_fx_physics("2", 0), 2);
        assert_eq!(normalize_fx_physics("3", 0), 3);
        assert_eq!(normalize_fx_physics("9", 0), 3);
        assert_eq!(normalize_fx_physics("bad", 2), 2);
    }

    #[test]
    fn fx_fps_defaults_and_normalization_preserve_legacy_sentinel() {
        assert_eq!(VideoSettings::default().fx_fps, crate::fx::FX_FPS_DEFAULT);
        assert_eq!(normalize_fx_fps("0", 90), crate::fx::FX_FPS_LEGACY_JKA);
        assert_eq!(normalize_fx_fps("1", 90), crate::fx::FX_FPS_MIN);
        assert_eq!(normalize_fx_fps("137", 90), 137);
        assert_eq!(normalize_fx_fps("9999", 90), crate::fx::FX_FPS_MAX);
        assert_eq!(normalize_fx_fps("bad", 90), 90);
    }

    #[test]
    fn registry_hud_cvars_preserve_config_round_trip_and_legacy_clamping() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "dinurdo-registry-{}-{stamp}.cfg",
            std::process::id()
        ));
        let mut settings = VideoSettings::default();
        crate::app::video_cvar_registry()
            .set(&mut settings, "cg_drawFPS", "2")
            .unwrap();
        crate::app::video_cvar_registry()
            .set(&mut settings, "cg_drawTimer", "on")
            .unwrap();
        crate::app::video_cvar_registry()
            .set(&mut settings, "cg_draw2D", "0")
            .unwrap();
        crate::app::video_cvar_registry()
            .set(&mut settings, "cg_drawHud", "off")
            .unwrap();
        save_video_settings(
            &path,
            settings,
            &crate::keybinds::Bindings::default(),
            &ClientPresentationSettings::default(),
            &AudioSettings::default(),
        )
        .unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("seta cg_drawFPS").count(), 1);
        assert_eq!(text.matches("seta cg_drawTimer").count(), 1);
        assert_eq!(text.matches("seta cg_draw2D").count(), 1);
        assert_eq!(text.matches("seta cg_drawHud").count(), 1);
        let loaded = load_video_settings(&path);
        assert_eq!(loaded.draw_fps, 2);
        assert!(loaded.draw_timer);
        assert!(!loaded.draw_2d);
        assert!(!loaded.draw_hud);
        for (value, expected) in [("7", 2), ("-0", 1), ("256", 1), ("invalid", 1)] {
            fs::write(
                &path,
                format!("seta CG_DRAWFPS \"{value}\"\nseta cg_drawTimer \"yes\"\n"),
            )
            .unwrap();
            let loaded = load_video_settings(&path);
            assert_eq!(loaded.draw_fps, expected, "{value}");
            assert!(loaded.draw_timer);
            assert!(loaded.draw_2d); // absent cvar defaults to on
            assert!(loaded.draw_hud);
        }
        assert!(crate::app::video_cvar_registry()
            .set(&mut settings, "cg_drawFPS", "7")
            .is_err());
        assert!(crate::app::video_cvar_registry()
            .set(&mut settings, "cg_draw2D", "2")
            .is_err());
        assert!(crate::app::video_cvar_registry()
            .set(&mut settings, "cg_drawHud", "-1")
            .is_err());
        fs::remove_file(path).unwrap();
    }

}
