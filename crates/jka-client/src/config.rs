use crate::camera::{
    SpectatorCameraMode, SpectatorCameraSettings, ThirdPersonSettings, DEFAULT_CG_FOV, MAX_CG_FOV,
    MAX_SPECTATOR_ORBIT_RANGE, MIN_CG_FOV, MIN_SPECTATOR_ORBIT_RANGE,
};
use crate::fx::{FX_FPS_LEGACY_JKA, FX_FPS_MAX, FX_FPS_MIN};
use crate::player::MouseInputSettings;
use crate::ui::{
    CloudRenderResolution, CloudType, ColorLutPreset, CrosshairSettings, DetailTextureMode,
    DofQuality, DynamicLightsMode, DynamicShadowsMode, EntityAmbientLightingMode,
    EntityShadowLight, FogMode, FootprintMode, FullscreenMode, FxGeometryMode, Ghoul2BatchMode,
    Ghoul2SkinningMode, HudElementId, HudElementLayout, HudLayout, MovementKeysSettings,
    PlayerNameSettings, PuddleQuality, PvsMode, RainIntensity, ReflectionQuality, RendererBackend,
    SaberMarkMode, StrafeHelperSettings, SunVisibilityMode, TextureFilter, VideoSettings,
    VsyncMode, CLOUD_HEIGHT_MAX, CLOUD_HEIGHT_MIN, CLOUD_THICKNESS_MAX, CLOUD_THICKNESS_MIN,
    MAX_DISTANCE_CULL_SCALE, MAX_FOG_STRENGTH,
};
use std::{fs, path::Path};

const GAMMA_MIN: f32 = 0.5;
const GAMMA_MAX: f32 = 3.0;
const FPS_CAP_MIN: u32 = 1;
pub const FPS_CAP_MAX: u32 = 10_000;
const PHYSICS_MSEC_MIN: u32 = 1;
const PHYSICS_MSEC_MAX: u32 = 33;

#[derive(Debug, Clone, PartialEq)]
pub struct ClientPresentationSettings {
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
    /// TaystJK cg_drawScores: 0 off, 1 classic, 2 coloured classic, 3 centred boxes.
    pub draw_scores: i32,
    pub mouse: MouseInputSettings,
    /// TaystJK/OpenJK horizontal field of view on the 4:3 baseline.
    pub fov: f32,
    /// TaystJK cg_zoomFov: target FOV for the held +zoom bind.
    pub zoom_fov: f32,
    /// TaystJK flipkick bind timing, counted in CGame frames.
    pub fk_duration: i32,
    pub fk_first_jump_duration: i32,
    pub fk_second_jump_delay: i32,
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
    /// con_suggest: live command/cvar filter popup while typing in the console.
    pub console_suggest: bool,
    /// cg_chatboxCompletion: Tab in the chat input completes player names.
    pub chatbox_completion: bool,
    /// cl_chatLog: persist live server chat sessions as self-contained HTML.
    pub chat_log: bool,
    /// TaystJK ui_vgs: use the jaPRO VGS menu in place of stock team voice chat.
    pub ui_vgs: i32,
    /// r_jumpHeightShade: tint landing surfaces by jump height in jaPRO SP physics.
    pub jump_height_shade: bool,
    /// cg_screenShake: 0 off, 1 effect-driven shake (explosions, stomps), 2 also weapon-fire shake.
    pub screen_shake: u8,
    /// Userinfo / network cvars (name, rate, snaps, cl_maxpackets, ...).
    pub network: crate::net::NetworkSettings,
    /// jaPRO cgame options: `cg_stylePlayer`, the race timer, spectator aids.
    pub japro: crate::japro_cg::JaproCgame,
    /// TaystJK-compatible server-browser masters (`sv_master1`..`sv_master5`).
    pub master_servers: [String; crate::server_browser::MAX_MASTER_SLOTS],
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
    pub separation: f32,
    pub mute_when_unfocused: bool,
    /// Master gate for Valve Steam Audio integration. Off preserves the legacy
    /// OpenJK-compatible mixer and skips all acoustic scene/bake preparation.
    pub steam_audio: bool,
    /// Live Steam Audio HRTF rendering for positional sounds.
    pub steam_audio_binaural: bool,
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
            separation: 0.5,
            mute_when_unfocused: true,
            steam_audio: false,
            steam_audio_binaural: true,
            steam_audio_environmental: true,
            game: GameOptions::default(),
        }
    }
}

/// An integer cvar value clamped to `0..=max` (cvar `.integer` truncates).
fn int_in(value: &str, max: u8) -> Option<u8> {
    let number = value
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|number| number.is_finite())?;
    Some((number as i32).clamp(0, i32::from(max)) as u8)
}

/// Load the archived OpenJK sound controls that DinurdoJK already implements
/// (plus music volume so it is ready for the background-track presenter).
pub fn load_audio_settings(primary: &Path, fallback: Option<&Path>) -> AudioSettings {
    let mut settings = AudioSettings::default();
    let source = fs::read_to_string(primary)
        .ok()
        .or_else(|| fallback.and_then(|path| fs::read_to_string(path).ok()));
    let Some(text) = source else {
        return settings;
    };

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
        let finite = || {
            value
                .parse::<f32>()
                .ok()
                .filter(|number| number.is_finite())
        };
        match name.to_ascii_lowercase().as_str() {
            "s_volume" => {
                settings.effects_volume =
                    finite().unwrap_or(settings.effects_volume).clamp(0.0, 1.0)
            }
            "s_volumevoice" => {
                settings.voice_volume = finite().unwrap_or(settings.voice_volume).clamp(0.0, 1.0)
            }
            "s_musicvolume" => {
                settings.music_volume = finite().unwrap_or(settings.music_volume).clamp(0.0, 1.0)
            }
            "s_separation" => {
                settings.separation = finite().unwrap_or(settings.separation).clamp(0.0, 1.0)
            }
            "s_mutewhenunfocused" => {
                settings.mute_when_unfocused =
                    parse_bool(value).unwrap_or(settings.mute_when_unfocused)
            }
            "s_steamaudio" => {
                settings.steam_audio = parse_bool(value).unwrap_or(settings.steam_audio)
            }
            "s_steamaudiobinaural" => {
                settings.steam_audio_binaural =
                    parse_bool(value).unwrap_or(settings.steam_audio_binaural)
            }
            "s_steamaudioenvironmental" => {
                settings.steam_audio_environmental =
                    parse_bool(value).unwrap_or(settings.steam_audio_environmental)
            }
            "cg_jumpsounds" => settings.game.jump = int_in(value, 3).unwrap_or(settings.game.jump),
            "cg_rollsounds" => settings.game.roll = int_in(value, 3).unwrap_or(settings.game.roll),
            "cg_notaunt" => {
                settings.game.no_taunt = parse_bool(value).unwrap_or(settings.game.no_taunt)
            }
            "cg_duelsounds" => settings.game.duel = int_in(value, 3).unwrap_or(settings.game.duel),
            "cg_killsounds" => settings.game.kill = int_in(value, 2).unwrap_or(settings.game.kill),
            "cg_killmessage" => {
                settings.game.kill_message = int_in(value, 3).unwrap_or(settings.game.kill_message)
            }
            "cg_drawrewards" => {
                settings.game.draw_rewards = int_in(value, 2).unwrap_or(settings.game.draw_rewards)
            }
            "cg_hitsounds" => settings.game.hit = int_in(value, 6).unwrap_or(settings.game.hit),
            "cg_duelmusic" => {
                settings.game.duel_music = parse_bool(value).unwrap_or(settings.game.duel_music)
            }
            "cg_ambientsounds" => {
                settings.game.ambient = parse_bool(value).unwrap_or(settings.game.ambient)
            }
            "cg_blood" => settings.game.blood = int_in(value, 2).unwrap_or(settings.game.blood),
            "cg_autoswitch" => {
                settings.game.auto_switch = int_in(value, 2).unwrap_or(settings.game.auto_switch)
            }
            "cg_scoreplums" => {
                settings.game.score_plums = parse_bool(value).unwrap_or(settings.game.score_plums)
            }
            "cg_ghoul2marks" => {
                settings.game.g2_marks = int_in(value, 64).unwrap_or(settings.game.g2_marks)
            }
            "cg_racesounds" => {
                settings.game.race_sounds = int_in(value, 255).unwrap_or(settings.game.race_sounds)
            }
            "cg_chatsounds" => {
                settings.game.chat_sounds = int_in(value, 2).unwrap_or(settings.game.chat_sounds)
            }
            "cg_footsteps" => {
                settings.game.footsteps = int_in(value, 4).unwrap_or(settings.game.footsteps)
            }
            _ => {}
        }
    }
    settings
}

impl Default for ClientPresentationSettings {
    fn default() -> Self {
        Self {
            third_person: ThirdPersonSettings::default(),
            spectator_camera: SpectatorCameraSettings::default(),
            first_person_lightsaber: true,
            saber_trail: 1,
            saber_team_colors: true,
            saber_staff_multi_color: false,
            team_overlay: crate::ui::TeamOverlaySettings::default(),
            score_deaths: 1,
            draw_scores: 1,
            mouse: MouseInputSettings::default(),
            fov: DEFAULT_CG_FOV,
            zoom_fov: 30.0,
            fk_duration: 50,
            fk_first_jump_duration: 0,
            fk_second_jump_delay: 0,
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
            console_suggest: true,
            chatbox_completion: true,
            chat_log: true,
            ui_vgs: 1,
            jump_height_shade: true,
            screen_shake: 1,
            network: crate::net::NetworkSettings::default(),
            japro: crate::japro_cg::JaproCgame::default(),
            master_servers: crate::server_browser::DEFAULT_MASTER_CVARS.map(str::to_owned),
        }
    }
}

/// Load local/POV presentation cvars. Stock OpenJK/TaystJK camera and mouse
/// controls live alongside DinurdoJK's presentation settings.
/// Unknown or malformed values retain the client defaults above.
pub fn load_client_presentation_settings(
    primary: &Path,
    fallback: Option<&Path>,
) -> ClientPresentationSettings {
    let mut settings = ClientPresentationSettings::default();
    let source = fs::read_to_string(primary)
        .ok()
        .or_else(|| fallback.and_then(|path| fs::read_to_string(path).ok()));
    let Some(text) = source else {
        return settings;
    };

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
        let finite = || {
            value
                .parse::<f32>()
                .ok()
                .filter(|number| number.is_finite())
        };
        match name.to_ascii_lowercase().as_str() {
            "model" if !value.trim().is_empty() => settings.model = value.trim().to_owned(),
            "cg_forcemodel" => settings.force_model = value.trim().to_owned(),
            "cg_drawcrosshair" => {
                if let Ok(style) = value.trim().parse::<u8>() {
                    settings.crosshair.style = style.min(crate::ui::CROSSHAIR_STYLE_MAX);
                }
            }
            "cg_crosshairimage" => {
                if let Ok(image) = value.trim().parse::<u8>() {
                    settings.crosshair.image = image.min(crate::ui::CROSSHAIR_IMAGE_COUNT);
                }
            }
            "cg_crosshairsize" => {
                settings.crosshair.size =
                    finite().unwrap_or(settings.crosshair.size).clamp(4.0, 96.0);
            }
            "cg_crosshairstrength" => {
                settings.crosshair.strength = finite()
                    .unwrap_or(settings.crosshair.strength)
                    .clamp(0.0, crate::ui::CROSSHAIR_STRENGTH_MAX);
            }
            "cg_crosshaircolor" => {
                if let Some(color) = parse_rgba8(value) {
                    settings.crosshair.color = color;
                }
            }
            "cg_dynamiccrosshair" => {
                if let Ok(mode) = value.trim().parse::<u8>() {
                    settings.crosshair.dynamic = mode.min(2);
                }
            }
            "cg_crosshairidentifytarget" => {
                settings.crosshair.identify_target =
                    parse_bool(value).unwrap_or(settings.crosshair.identify_target)
            }
            "cg_drawcrosshairnames" => {
                settings.crosshair.names = finite()
                    .unwrap_or(settings.crosshair.names)
                    .clamp(-1000.0, 1000.0);
            }
            "cg_drawcrosshairnamescolours" => {
                settings.crosshair.names_colours =
                    parse_bool(value).unwrap_or(settings.crosshair.names_colours)
            }
            "cg_drawcrosshairnamesopacity" => {
                settings.crosshair.names_opacity = finite()
                    .unwrap_or(settings.crosshair.names_opacity)
                    .clamp(0.0, 1.0);
            }
            "cg_drawplayernames" => {
                settings.player_names.mode = value
                    .trim()
                    .parse::<i32>()
                    .unwrap_or(settings.player_names.mode)
                    .clamp(0, 2);
            }
            "cg_drawplayernamesscale" => {
                settings.player_names.scale = finite()
                    .unwrap_or(settings.player_names.scale)
                    .clamp(0.05, 4.0);
            }
            hud_cvar if HudElementId::from_cvar(hud_cvar).is_some() => {
                if let (Some(id), Some(layout)) = (
                    HudElementId::from_cvar(hud_cvar),
                    HudElementLayout::from_config(value),
                ) {
                    *settings.hud_layout.element_mut(id) = layout;
                }
            }
            "cg_hudsnap" => {
                settings.hud_layout.snap_to_grid =
                    parse_bool(value).unwrap_or(settings.hud_layout.snap_to_grid)
            }
            "cg_hudgridsize" => {
                settings.hud_layout.grid_size = finite()
                    .unwrap_or(settings.hud_layout.grid_size)
                    .clamp(1.0, 64.0);
            }
            "cg_movementkeys" => {
                if let Ok(mode) = value.trim().parse::<u8>() {
                    settings.movement_keys.mode = mode.min(4);
                }
            }
            "cg_movementkeysx" => {
                settings.movement_keys.x = finite()
                    .unwrap_or(settings.movement_keys.x)
                    .clamp(-640.0, 640.0)
            }
            "cg_movementkeysy" => {
                settings.movement_keys.y = finite()
                    .unwrap_or(settings.movement_keys.y)
                    .clamp(-480.0, 480.0)
            }
            "cg_movementkeyssize" => {
                settings.movement_keys.size = finite()
                    .unwrap_or(settings.movement_keys.size)
                    .clamp(0.25, 4.0)
            }
            "cg_movementkeyswalk" => {
                settings.movement_keys.walk =
                    parse_bool(value).unwrap_or(settings.movement_keys.walk)
            }
            "cg_strafehelper" => {
                if let Ok(flags) = value.trim().parse::<u32>() {
                    settings.strafe_helper.flags = flags;
                }
            }
            "cg_strafehelper_fps" => {
                settings.strafe_helper.fps = finite()
                    .unwrap_or(settings.strafe_helper.fps)
                    .clamp(0.0, 1000.0)
            }
            "cg_strafehelperoffset" => {
                settings.strafe_helper.offset = finite()
                    .unwrap_or(settings.strafe_helper.offset)
                    .clamp(-1000.0, 1000.0)
            }
            "cg_strafehelperlinewidth" => {
                settings.strafe_helper.line_width = finite()
                    .unwrap_or(settings.strafe_helper.line_width)
                    .clamp(0.25, 5.0)
            }
            "cg_strafehelperprecision" => {
                settings.strafe_helper.precision = value
                    .trim()
                    .parse::<u32>()
                    .unwrap_or(settings.strafe_helper.precision)
                    .clamp(100, 10000)
            }
            "cg_strafehelpercutoff" => {
                settings.strafe_helper.cutoff = finite()
                    .unwrap_or(settings.strafe_helper.cutoff)
                    .clamp(0.0, 480.0)
            }
            "cg_strafehelperactivecolor" => {
                if let Some(color) = parse_rgba8(value) {
                    settings.strafe_helper.active_color = color;
                }
            }
            "cg_strafehelperinactivealpha" => {
                if let Ok(alpha) = value.trim().parse::<i32>() {
                    settings.strafe_helper.inactive_alpha = alpha.clamp(0, 255) as u8;
                }
            }
            "cg_strafetrailradius" => {
                settings.strafe_trail.radius = finite()
                    .unwrap_or(settings.strafe_trail.radius)
                    .clamp(0.1, 100.0)
            }
            "cg_strafetraillife" => {
                settings.strafe_trail.life_seconds = finite()
                    .unwrap_or(settings.strafe_trail.life_seconds)
                    .clamp(0.1, 3600.0)
            }
            "cg_strafetrailfps" => {
                settings.strafe_trail.fps = finite()
                    .unwrap_or(settings.strafe_trail.fps)
                    .clamp(1.0, 1000.0)
            }
            "cg_strafetrailplums" => {
                settings.strafe_trail.plums =
                    parse_bool(value).unwrap_or(settings.strafe_trail.plums)
            }
            "cg_strafetrailghost" => {
                settings.strafe_trail.ghost =
                    parse_bool(value).unwrap_or(settings.strafe_trail.ghost)
            }
            "cg_strafetrailplayers" => {
                settings.strafe_trail.players = value
                    .trim()
                    .parse::<u32>()
                    .unwrap_or(settings.strafe_trail.players)
            }
            "cg_logstrafetrail" => settings.strafe_trail.log_name = value.trim().to_owned(),
            "cg_strafetraildistance" => {
                settings.strafe_trail.draw_distance = finite()
                    .unwrap_or(settings.strafe_trail.draw_distance)
                    .clamp(256.0, 131072.0)
            }
            "cg_rghostalpha" => {
                settings.race_ghost_alpha = finite()
                    .unwrap_or(settings.race_ghost_alpha)
                    .clamp(0.02, 1.0)
            }
            "cg_rghostname" => {
                settings.race_ghost_name = parse_bool(value).unwrap_or(settings.race_ghost_name)
            }
            "cg_rghosttrail" => {
                settings.race_ghost_trail = parse_bool(value).unwrap_or(settings.race_ghost_trail)
            }
            "cg_rghostvelocitydelta" => {
                settings.race_ghost_velocity_delta =
                    parse_bool(value).unwrap_or(settings.race_ghost_velocity_delta)
            }
            "cg_rghostdistancedelta" => {
                settings.race_ghost_distance_delta =
                    parse_bool(value).unwrap_or(settings.race_ghost_distance_delta)
            }
            "cg_rghostdemobaseurl" if !value.trim().is_empty() => {
                settings.race_ghost_demo_base_url = value.trim().to_owned()
            }
            "con_timestamps" => {
                settings.console_timestamps =
                    parse_bool(value).unwrap_or(settings.console_timestamps)
            }
            "con_suggest" => {
                settings.console_suggest = parse_bool(value).unwrap_or(settings.console_suggest)
            }
            "cg_chatboxcompletion" => {
                settings.chatbox_completion =
                    parse_bool(value).unwrap_or(settings.chatbox_completion)
            }
            "cl_chatlog" => settings.chat_log = parse_bool(value).unwrap_or(settings.chat_log),
            "ui_vgs" => settings.ui_vgs = value.trim().parse::<i32>().unwrap_or(settings.ui_vgs),
            "cg_screenshake" => {
                settings.screen_shake = int_in(value, 2).unwrap_or(settings.screen_shake)
            }
            "r_jumpheightshade" => {
                settings.jump_height_shade = parse_bool(value).unwrap_or(settings.jump_height_shade)
            }
            "sv_master1" => settings.master_servers[0] = value.trim().to_owned(),
            "sv_master2" => settings.master_servers[1] = value.trim().to_owned(),
            "sv_master3" => settings.master_servers[2] = value.trim().to_owned(),
            "sv_master4" => settings.master_servers[3] = value.trim().to_owned(),
            "sv_master5" => settings.master_servers[4] = value.trim().to_owned(),
            other if settings.japro.cvar_value(other).is_some() => {
                let _ = settings.japro.set_cvar(other, value);
            }
            other if settings.network.cvar_value(other).is_some() => {
                let _ = settings.network.set_cvar(other, value);
            }
            "sensitivity" => {
                settings.mouse.sensitivity = finite().unwrap_or(settings.mouse.sensitivity)
            }
            "m_yaw" => settings.mouse.yaw = finite().unwrap_or(settings.mouse.yaw),
            "m_pitch" => settings.mouse.pitch = finite().unwrap_or(settings.mouse.pitch),
            "cl_mouseaccel" => settings.mouse.accel = finite().unwrap_or(settings.mouse.accel),
            "cg_fov" => {
                settings.fov = finite()
                    .unwrap_or(settings.fov)
                    .clamp(MIN_CG_FOV, MAX_CG_FOV)
            }
            "cg_zoomfov" => settings.zoom_fov = finite().unwrap_or(settings.zoom_fov),
            "cg_fkduration" => {
                settings.fk_duration = value.trim().parse::<i32>().unwrap_or(settings.fk_duration)
            }
            "cg_fkfirstjumpduration" => {
                settings.fk_first_jump_duration = value
                    .trim()
                    .parse::<i32>()
                    .unwrap_or(settings.fk_first_jump_duration)
            }
            "cg_fksecondjumpdelay" => {
                settings.fk_second_jump_delay = value
                    .trim()
                    .parse::<i32>()
                    .unwrap_or(settings.fk_second_jump_delay)
            }
            "cg_thirdperson" => {
                settings.third_person.enabled =
                    parse_bool(value).unwrap_or(settings.third_person.enabled)
            }
            "cg_speccamera" => {
                let mode = value
                    .trim()
                    .parse::<i32>()
                    .unwrap_or(settings.spectator_camera.mode.as_i32());
                settings.spectator_camera.mode = SpectatorCameraMode::from_i32(mode);
            }
            "cg_speccameramotion" => {
                settings.spectator_camera.motion_direction =
                    parse_bool(value).unwrap_or(settings.spectator_camera.motion_direction)
            }
            "cg_specorbitrange" => {
                settings.spectator_camera.orbit_range = finite()
                    .unwrap_or(settings.spectator_camera.orbit_range)
                    .clamp(MIN_SPECTATOR_ORBIT_RANGE, MAX_SPECTATOR_ORBIT_RANGE)
            }
            "cg_fpls" => {
                settings.first_person_lightsaber =
                    parse_bool(value).unwrap_or(settings.first_person_lightsaber)
            }
            "cg_sabertrail" => {
                settings.saber_trail = value
                    .trim()
                    .parse::<i32>()
                    .ok()
                    .map(|value| value.clamp(0, 2))
                    .unwrap_or(settings.saber_trail)
            }
            "cg_saberteamcolors" => {
                settings.saber_team_colors = parse_bool(value).unwrap_or(settings.saber_team_colors)
            }
            "cg_saberstaffmulticolor" => {
                settings.saber_staff_multi_color =
                    parse_bool(value).unwrap_or(settings.saber_staff_multi_color)
            }
            "cg_drawteamoverlay" => {
                settings.team_overlay.mode = value
                    .trim()
                    .parse::<i32>()
                    .ok()
                    .map(|v| v.clamp(0, 6))
                    .unwrap_or(settings.team_overlay.mode)
            }
            "cg_drawteamoverlayx" => {
                settings.team_overlay.x = value.trim().parse().unwrap_or(settings.team_overlay.x)
            }
            "cg_drawteamoverlayy" => {
                settings.team_overlay.y = value.trim().parse().unwrap_or(settings.team_overlay.y)
            }
            "cg_drawteamoverlayweapons" => {
                settings.team_overlay.weapons =
                    parse_bool(value).unwrap_or(settings.team_overlay.weapons)
            }
            "cg_drawteamoverlayscale" => {
                settings.team_overlay.scale = finite()
                    .unwrap_or(settings.team_overlay.scale)
                    .clamp(0.5, 2.5)
            }
            "cg_drawteamoverlaymaxhp" => {
                settings.team_overlay.max_hp =
                    finite().unwrap_or(settings.team_overlay.max_hp).max(1.0)
            }
            "cg_drawteamoverlayforce" => {
                settings.team_overlay.force =
                    parse_bool(value).unwrap_or(settings.team_overlay.force)
            }
            "cg_scoredeaths" => {
                settings.score_deaths = value
                    .trim()
                    .parse::<i32>()
                    .ok()
                    .map(|v| v.clamp(0, 3))
                    .unwrap_or(settings.score_deaths)
            }
            "cg_drawscores" => {
                settings.draw_scores = value
                    .trim()
                    .parse::<i32>()
                    .ok()
                    .map(|v| v.clamp(0, 3))
                    .unwrap_or(settings.draw_scores)
            }
            "cg_thirdpersonalpha" => {
                settings.third_person.alpha = finite().unwrap_or(settings.third_person.alpha)
            }
            "cg_thirdpersonangle" => {
                settings.third_person.angle = finite().unwrap_or(settings.third_person.angle)
            }
            "cg_thirdpersoncameradamp" => {
                settings.third_person.camera_damp =
                    finite().unwrap_or(settings.third_person.camera_damp)
            }
            "cg_thirdpersonhorzoffset" => {
                settings.third_person.horz_offset =
                    finite().unwrap_or(settings.third_person.horz_offset)
            }
            "cg_thirdpersonpitchoffset" => {
                settings.third_person.pitch_offset =
                    finite().unwrap_or(settings.third_person.pitch_offset)
            }
            "cg_thirdpersonrange" => {
                settings.third_person.range = finite().unwrap_or(settings.third_person.range)
            }
            "cg_thirdpersonspecialcam" => {
                settings.third_person.special_cam =
                    parse_bool(value).unwrap_or(settings.third_person.special_cam)
            }
            "cg_thirdpersontargetdamp" => {
                settings.third_person.target_damp =
                    finite().unwrap_or(settings.third_person.target_damp)
            }
            "cg_thirdpersonvertoffset" => {
                settings.third_person.vert_offset =
                    finite().unwrap_or(settings.third_person.vert_offset)
            }
            _ => {}
        }
    }
    settings
}

pub fn load_video_settings(primary: &Path, fallback: Option<&Path>) -> VideoSettings {
    let mut settings = VideoSettings::default();
    let mut explicit_gamma_method = false;
    let source = fs::read_to_string(primary)
        .ok()
        .or_else(|| fallback.and_then(|path| fs::read_to_string(path).ok()));
    let Some(text) = source else {
        return settings;
    };

    let mut custom_width = None;
    let mut custom_height = None;
    let mut window_x = None;
    let mut window_y = None;
    let mut novis = None;
    let mut pvs_mode = None;
    let mut anisotropy = None;
    let mut tone_mapping_seen = false;
    let mut dynamic_shadows_mode_seen = false;
    let mut weather_wind_explicit = false;
    let mut legacy_cloud_wind_speed_seen = false;
    let mut legacy_cloud_wind_direction_seen = false;
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
        match name.to_ascii_lowercase().as_str() {
            "r_fullscreen" => {
                settings.fullscreen =
                    FullscreenMode::from_config(value).unwrap_or(settings.fullscreen)
            }
            "r_backend" => {
                settings.renderer_backend =
                    RendererBackend::from_config(value).unwrap_or(settings.renderer_backend)
            }
            "r_swapinterval" => {
                settings.vsync = VsyncMode::from_config(value).unwrap_or(settings.vsync)
            }
            "r_maxframelatency" => {
                if let Ok(value) = value.parse::<u32>() {
                    if (1..=3).contains(&value) {
                        settings.max_frame_latency = value;
                    }
                }
            }
            "r_customwidth" => custom_width = value.parse::<u32>().ok().filter(|v| *v >= 320),
            "r_customheight" => custom_height = value.parse::<u32>().ok().filter(|v| *v >= 240),
            "r_windowx" => window_x = value.parse::<i32>().ok(),
            "r_windowy" => window_y = value.parse::<i32>().ok(),
            "r_windowmaximized" => {
                settings.window_maximized = parse_bool(value).unwrap_or(settings.window_maximized)
            }
            "r_ext_multisample" => {
                if let Ok(samples) = value.parse::<u32>() {
                    settings.msaa_samples = samples.max(1);
                }
            }
            "r_texturemode" => {
                settings.texture_filter = match value.to_ascii_uppercase().as_str() {
                    "GL_NEAREST" | "GL_NEAREST_MIPMAP_NEAREST" | "GL_NEAREST_MIPMAP_LINEAR" => {
                        TextureFilter::Nearest
                    }
                    "GL_LINEAR" | "GL_LINEAR_MIPMAP_NEAREST" => TextureFilter::Bilinear,
                    "GL_LINEAR_MIPMAP_LINEAR" => TextureFilter::Trilinear,
                    _ => settings.texture_filter,
                };
            }
            "r_picmip" => {
                if let Ok(picmip) = value.parse::<u32>() {
                    settings.picmip = picmip.min(16);
                }
            }
            // Keep the familiar OpenJK/JKA cvar name. wgpu exposes anisotropy
            // directly on the sampler; values above 1 require linear min/mag/mip
            // filtering, so the UI represents AF as a refinement of trilinear.
            "r_ext_texture_filter_anisotropic" => {
                anisotropy = value.parse::<f32>().ok().filter(|v| v.is_finite());
            }
            "r_detailtextures" => {
                settings.detail_textures =
                    DetailTextureMode::from_config(value).unwrap_or(settings.detail_textures);
            }
            "r_detailtexturefade" => {
                settings.detail_texture_fade =
                    parse_bool(value).unwrap_or(settings.detail_texture_fade);
            }
            "r_detailtexturefadedistance" => {
                if let Ok(distance) = value.parse::<f32>() {
                    if distance.is_finite() {
                        settings.detail_texture_fade_distance = distance.clamp(64.0, 8192.0);
                    }
                }
            }
            "r_showtris" => {
                if let Ok(mask) = value.trim().parse::<u32>() {
                    settings.wireframe_mask = mask & crate::ui::wireframe::ALL;
                } else if let Some(enabled) = parse_bool(value) {
                    settings.wireframe_mask = if enabled {
                        crate::ui::wireframe::MAP
                    } else {
                        0
                    };
                }
            }
            "r_skipui" => {
                settings.skip_ui = parse_bool(value).unwrap_or(settings.skip_ui);
            }
            "developer" => {
                let level = value
                    .trim()
                    .parse::<u8>()
                    .ok()
                    .map(|level| level.min(3))
                    .or_else(|| parse_bool(value).map(u8::from))
                    .unwrap_or(settings.developer_level);
                settings.developer_level = level;
                settings.developer_tools = level != 0;
            }
            "r_verbose" => {
                settings.renderer_verbose = value
                    .trim()
                    .parse::<u8>()
                    .ok()
                    .map(|level| level.min(3))
                    .or_else(|| parse_bool(value).map(u8::from))
                    .unwrap_or(settings.renderer_verbose);
            }
            "r_perftrace" => {
                settings.perf_trace = parse_bool(value).unwrap_or(settings.perf_trace);
            }
            "r_worldpath" => {
                settings.force_unified_world = value.trim().eq_ignore_ascii_case("unified");
            }
            "r_gputimings" => {
                settings.gpu_timings = parse_bool(value).unwrap_or(settings.gpu_timings);
            }
            "r_ghoul2skinning" => {
                settings.ghoul2_skinning =
                    Ghoul2SkinningMode::from_config(value).unwrap_or(settings.ghoul2_skinning);
            }
            "r_ghoul2earlycull" => {
                settings.ghoul2_early_cull =
                    parse_bool(value).unwrap_or(settings.ghoul2_early_cull);
            }
            "r_lodbias" => {
                settings.ghoul2_lod_bias = value
                    .parse::<i32>()
                    .ok()
                    .map(|bias| bias.max(0))
                    .unwrap_or(settings.ghoul2_lod_bias);
            }
            "r_ghoul2batchdraws" => {
                settings.ghoul2_batch_draws =
                    Ghoul2BatchMode::from_config(value).unwrap_or(settings.ghoul2_batch_draws);
            }
            "r_ghoul2animsmooth" => {
                // jaPRO stores this cvar unclamped and only activates smoothing
                // when it reads strictly inside (0, 1) at use; 1.0+ is a no-op
                // there, not "maximum". Don't clamp the upper bound here either.
                settings.ghoul2_anim_smooth = value
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .map(|v| v.max(0.0))
                    .unwrap_or(settings.ghoul2_anim_smooth);
            }
            "com_maxfps" => {
                settings.fps_cap = normalize_fps_cap(value, settings.fps_cap);
            }
            "cg_fxfps" => {
                settings.fx_fps = normalize_fx_fps(value, settings.fx_fps);
            }
            "cg_fxfpsscope" => {
                settings.fx_fps_scope = normalize_fx_fps_scope(value, settings.fx_fps_scope);
            }
            "fx_physics" => {
                settings.fx_physics = normalize_fx_physics(value, settings.fx_physics);
            }
            "fx_lod" => {
                settings.fx_lod = normalize_fx_lod(value, settings.fx_lod);
            }
            "r_fxlodscale" => {
                settings.fx_lod_scale = normalize_lod_scale(value, settings.fx_lod_scale);
            }
            "r_lodscale" => {
                settings.lod_scale = normalize_lod_scale(value, settings.lod_scale);
            }
            "fx_countscale" => {
                settings.fx_count_scale = normalize_fx_count_scale(value, settings.fx_count_scale);
            }
            "r_fxgeometry" => {
                settings.fx_geometry =
                    FxGeometryMode::from_config(value).unwrap_or(settings.fx_geometry);
            }
            "r_fxzeroalphadiscard" => {
                settings.fx_zero_alpha_discard =
                    parse_bool(value).unwrap_or(settings.fx_zero_alpha_discard);
            }
            "cg_drawfps" => {
                if let Ok(mode) = value.parse::<u8>() {
                    settings.draw_fps = mode.min(2);
                }
            }
            "cg_drawtimer" => {
                settings.draw_timer = parse_bool(value).unwrap_or(settings.draw_timer);
            }
            "pmove_msec" => {
                settings.physics_msec = normalize_physics_msec(value, settings.physics_msec);
            }
            "cl_timerresolution1ms" => {
                settings.timer_resolution_1ms =
                    parse_bool(value).unwrap_or(settings.timer_resolution_1ms);
            }
            "cl_input_latelatch" => {
                settings.input_latelatch = parse_bool(value).unwrap_or(settings.input_latelatch);
            }
            "r_physics" => {
                settings.client_physics = parse_bool(value).unwrap_or(settings.client_physics);
            }
            "r_physicshz" => {
                if let Ok(requested) = value.parse::<u32>() {
                    const VALUES: [u32; 4] = [30, 60, 120, 240];
                    settings.client_physics_hz = *VALUES
                        .iter()
                        .min_by_key(|&&candidate| candidate.abs_diff(requested))
                        .unwrap_or(&60);
                }
            }
            "r_physicsmaxsubsteps" => {
                if let Ok(requested) = value.parse::<u32>() {
                    const VALUES: [u32; 4] = [1, 2, 4, 8];
                    settings.client_physics_max_substeps = *VALUES
                        .iter()
                        .min_by_key(|&&candidate| candidate.abs_diff(requested))
                        .unwrap_or(&4);
                }
            }
            "r_physicsccd" => {
                settings.client_physics_ccd =
                    parse_bool(value).unwrap_or(settings.client_physics_ccd);
            }
            "r_physicssleeping" => {
                settings.client_physics_sleeping =
                    parse_bool(value).unwrap_or(settings.client_physics_sleeping);
            }
            "r_ragdolls" => settings.ragdolls = parse_bool(value).unwrap_or(settings.ragdolls),
            "r_ragdollmax" => {
                if let Ok(requested) = value.parse::<u32>() {
                    const VALUES: [u32; 5] = [2, 4, 8, 16, 32];
                    settings.ragdoll_max = *VALUES
                        .iter()
                        .min_by_key(|&&v| v.abs_diff(requested))
                        .unwrap_or(&8);
                }
            }
            "r_ragdolllifetime" => {
                if let Ok(requested) = value.parse::<u32>() {
                    const VALUES: [u32; 5] = [5, 10, 20, 30, 60];
                    settings.ragdoll_lifetime = *VALUES
                        .iter()
                        .min_by_key(|&&v| v.abs_diff(requested))
                        .unwrap_or(&20) as f32;
                }
            }
            "r_ragdollselfcollision" => {
                settings.ragdoll_self_collision =
                    parse_bool(value).unwrap_or(settings.ragdoll_self_collision);
            }
            "cg_dismember" => {
                if let Ok(value) = value.parse::<u8>() {
                    settings.dismemberment = value.min(2);
                }
            }
            "r_dismembermax" => {
                if let Ok(value) = value.parse::<u32>() {
                    settings.dismember_max = value.clamp(1, 128);
                }
            }
            "r_dismemberlifetime" => {
                if let Some(value) = value.parse::<f32>().ok().filter(|value| value.is_finite()) {
                    settings.dismember_lifetime = value.clamp(1.0, 300.0);
                }
            }
            "r_jigglephysics" => {
                settings.jiggle_physics = parse_bool(value).unwrap_or(settings.jiggle_physics);
            }
            "r_jigglesolver" => {
                let normalized = value.trim().to_ascii_lowercase();
                settings.jiggle_solver = match normalized.as_str() {
                    "0" | "kawaii" | "kawaiiphysics" => 0,
                    "1" | "jiggle" | "jigglephysics" | "naelstrof" => 1,
                    _ => settings.jiggle_solver,
                };
            }
            "r_jigglestrength" => {
                if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                    settings.jiggle_strength = v.clamp(0.0, 2.0);
                }
            }
            "r_jigglebreaststrength" => {
                if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                    settings.jiggle_breast_strength = v.clamp(0.0, 2.0);
                }
            }
            "r_jiggleglutestrength" => {
                if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                    settings.jiggle_glute_strength = v.clamp(0.0, 2.0);
                }
            }
            "r_jigglestiffness" => {
                if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                    settings.jiggle_stiffness = v.clamp(0.0, 3.0);
                }
            }
            "r_jiggledamping" => {
                if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                    settings.jiggle_damping = v.clamp(0.0, 3.0);
                }
            }
            "r_jiggleglutelift" => {
                if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                    settings.jiggle_glute_lift = v.clamp(-0.4, 0.6);
                }
            }
            "r_jigglejpstiffness" => {
                if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                    settings.jiggle_jp_stiffness = v.clamp(0.0, 1.0);
                }
            }
            "r_jigglejpdrag" => {
                if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                    settings.jiggle_jp_drag = v.clamp(0.0, 1.0);
                }
            }
            "r_jigglejpairdrag" => {
                if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                    settings.jiggle_jp_air_drag = v.clamp(0.0, 1.0);
                }
            }
            "r_jigglejpstretch" => {
                if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                    settings.jiggle_jp_stretch = v.clamp(0.0, 1.0);
                }
            }
            "r_jigglejpsoften" => {
                if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                    settings.jiggle_jp_soften = v.clamp(0.0, 1.0);
                }
            }
            "r_jigglejpgravity" => {
                if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                    settings.jiggle_jp_gravity = v.clamp(0.0, 2.0);
                }
            }
            "r_clothphysics" => {
                settings.cloth_physics = parse_bool(value).unwrap_or(settings.cloth_physics);
            }
            "r_clothbodycollision" => {
                settings.cloth_body_collision =
                    parse_bool(value).unwrap_or(settings.cloth_body_collision);
            }
            "r_clothbodyclearance" => {
                if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                    settings.cloth_body_clearance = v.clamp(0.0, 4.0);
                }
            }
            "r_clothairresistance" => {
                if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                    settings.cloth_air_resistance = v.clamp(0.0, 4.0);
                }
            }
            "r_clothturnresponse" => {
                if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                    settings.cloth_turn_response = v.clamp(0.0, 4.0);
                }
            }
            "r_clothanimationinfluence" => {
                if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                    settings.cloth_animation_influence = v.clamp(0.0, 1.0);
                }
            }
            "r_clothwind" => settings.cloth_wind = parse_bool(value).unwrap_or(settings.cloth_wind),
            "r_physicsprops" => {
                settings.physics_props = parse_bool(value).unwrap_or(settings.physics_props)
            }
            "r_physicspropmax" => {
                if let Ok(requested) = value.parse::<u32>() {
                    const VALUES: [u32; 5] = [32, 64, 96, 192, 384];
                    settings.physics_prop_max = *VALUES
                        .iter()
                        .min_by_key(|&&v| v.abs_diff(requested))
                        .unwrap_or(&96);
                }
            }
            "r_physicsdebris" => {
                settings.physics_debris = parse_bool(value).unwrap_or(settings.physics_debris)
            }
            "r_physicsdebrismax" => {
                if let Ok(requested) = value.parse::<u32>() {
                    const VALUES: [u32; 5] = [64, 128, 192, 384, 768];
                    settings.physics_debris_max = *VALUES
                        .iter()
                        .min_by_key(|&&v| v.abs_diff(requested))
                        .unwrap_or(&192);
                }
            }
            "r_physicsdebrislifetime" => {
                if let Ok(requested) = value.parse::<u32>() {
                    const VALUES: [u32; 5] = [2, 5, 10, 20, 30];
                    settings.physics_debris_lifetime = *VALUES
                        .iter()
                        .min_by_key(|&&v| v.abs_diff(requested))
                        .unwrap_or(&10)
                        as f32;
                }
            }
            "r_physicsplayerpush" => {
                settings.physics_player_push =
                    parse_bool(value).unwrap_or(settings.physics_player_push)
            }
            "r_physicsweaponimpulses" => {
                settings.physics_weapon_impulses =
                    parse_bool(value).unwrap_or(settings.physics_weapon_impulses)
            }
            "r_physicsexplosionimpulses" => {
                settings.physics_explosion_impulses =
                    parse_bool(value).unwrap_or(settings.physics_explosion_impulses)
            }
            "r_physicsforceimpulses" => {
                settings.physics_force_impulses =
                    parse_bool(value).unwrap_or(settings.physics_force_impulses)
            }
            "r_physicsdebug" => {
                settings.physics_debug_draw =
                    parse_bool(value).unwrap_or(settings.physics_debug_draw)
            }
            "r_physicsstats" => {
                settings.physics_stats = parse_bool(value).unwrap_or(settings.physics_stats)
            }
            "r_novis" => novis = parse_bool(value),
            "r_pvsmode" => {
                pvs_mode = match value.to_ascii_lowercase().as_str() {
                    "off" | "0" => Some(PvsMode::Off),
                    "minimal" | "1" => Some(PvsMode::Minimal),
                    "full" | "2" => Some(PvsMode::Full),
                    // Auto 1-3 were removed; Auto 4 became Auto.
                    "auto" | "3" | "auto2" | "4" | "auto3" | "5" | "auto4" | "batched"
                    | "pvsbatched" | "6" => Some(PvsMode::Auto),
                    _ => pvs_mode,
                };
            }
            "r_hdr" => settings.hdr = parse_bool(value).unwrap_or(settings.hdr),
            "r_floatlightmap" => {
                settings.float_lightmap = parse_bool(value).unwrap_or(settings.float_lightmap)
            }
            "r_autoexposure" => {
                settings.auto_exposure = parse_bool(value).unwrap_or(settings.auto_exposure)
            }
            "r_tonemap" => {
                tone_mapping_seen = true;
                settings.tone_mapping = parse_bool(value).unwrap_or(settings.tone_mapping);
            }
            "r_bloom" => settings.bloom = parse_bool(value).unwrap_or(settings.bloom),
            "r_halation" => settings.halation = parse_bool(value).unwrap_or(settings.halation),
            "r_ssao" => settings.ssao = parse_bool(value).unwrap_or(settings.ssao),
            "r_staticbspao" => {
                settings.static_bsp_ao = parse_bool(value).unwrap_or(settings.static_bsp_ao)
            }
            "r_staticbspaomode" => {
                settings.static_bsp_ao_lightmap = match value.to_ascii_lowercase().as_str() {
                    "lightmap" | "lightmap-space" | "1" => true,
                    "vertex" | "per-vertex" | "0" => false,
                    _ => settings.static_bsp_ao_lightmap,
                };
            }
            "r_staticbspaosamples" => {
                if let Ok(samples) = value.parse::<u32>() {
                    const VALUES: [u32; 5] = [8, 16, 32, 64, 128];
                    settings.static_bsp_ao_samples = *VALUES
                        .iter()
                        .min_by_key(|&&candidate| candidate.abs_diff(samples))
                        .unwrap_or(&8);
                }
            }
            "r_staticbspaoresolution" => {
                if let Ok(scale) = value.parse::<u32>() {
                    const VALUES: [u32; 3] = [1, 3, 5];
                    settings.static_bsp_ao_resolution = *VALUES
                        .iter()
                        .min_by_key(|&&candidate| candidate.abs_diff(scale))
                        .unwrap_or(&3);
                }
            }
            "r_staticbspaostrength" => {
                if let Ok(value) = value.parse::<u32>() {
                    const VALUES: [u32; 4] = [25, 50, 75, 100];
                    settings.static_bsp_ao_strength = *VALUES
                        .iter()
                        .min_by_key(|&&candidate| candidate.abs_diff(value))
                        .unwrap_or(&75);
                }
            }
            "r_staticbspaorange" => {
                if let Ok(value) = value.parse::<u32>() {
                    const VALUES: [u32; 4] = [50, 100, 150, 200];
                    settings.static_bsp_ao_range = *VALUES
                        .iter()
                        .min_by_key(|&&candidate| candidate.abs_diff(value))
                        .unwrap_or(&100);
                }
            }
            "r_staticbspaocurrentcell" => {
                settings.static_bsp_ao_current_cell =
                    parse_bool(value).unwrap_or(settings.static_bsp_ao_current_cell);
            }
            "r_fxaa" => settings.fxaa = parse_bool(value).unwrap_or(settings.fxaa),
            "r_smaa" => settings.smaa = parse_bool(value).unwrap_or(settings.smaa),
            "r_taa" => settings.taa = parse_bool(value).unwrap_or(settings.taa),
            "r_contactshadows" => {
                settings.contact_shadows = parse_bool(value).unwrap_or(settings.contact_shadows)
            }
            // Read-only alias: configs written before r_fogMode was folded into
            // r_drawfog. It is never written back.
            "r_fogmode" => {
                settings.fog_mode = match value.to_ascii_lowercase().as_str() {
                    "off" | "0" => FogMode::Off,
                    // Keep the old "legacy"/"1" spelling pointed at JKA's default
                    // r_drawfog 2 behavior so existing DinurdoJK.cfg files retain
                    // their intended visual mode.
                    "legacy" | "legacy2" | "1" => FogMode::LegacyDrawFog2,
                    "legacy1" => FogMode::LegacyDrawFog1,
                    "volumetric" | "2" => FogMode::Volumetric,
                    _ => settings.fog_mode,
                }
            }
            // Preserve OpenJK's real r_drawfog split:
            //   1 = explicit fog redraw after the material stages
            //   2 = global fog during shader stages; local brush fog still redraws
            // DinurdoJK adds 3 = volumetric.
            "r_drawfog" => {
                settings.fog_mode = FogMode::from_drawfog(value).unwrap_or(settings.fog_mode);
            }
            "r_fogstrength" => {
                if let Ok(strength) = value.parse::<f32>() {
                    if strength.is_finite() {
                        settings.fog_strength = strength.clamp(0.0, MAX_FOG_STRENGTH);
                    }
                }
            }
            "r_sunoverride" => {
                settings.sun_override = parse_bool(value).unwrap_or(settings.sun_override)
            }
            "r_sunvisibility" => {
                settings.sun_visibility =
                    SunVisibilityMode::from_config(value).unwrap_or(settings.sun_visibility);
            }
            "r_entitysunlighting" => {
                settings.entity_sun_lighting =
                    parse_bool(value).unwrap_or(settings.entity_sun_lighting)
            }
            "r_sunyaw" => {
                if let Ok(yaw) = value.parse::<f32>() {
                    if yaw.is_finite() {
                        settings.sun_yaw = yaw.rem_euclid(360.0);
                    }
                }
            }
            "r_sunpitch" => {
                if let Ok(pitch) = value.parse::<f32>() {
                    if pitch.is_finite() {
                        settings.sun_pitch = pitch.clamp(-90.0, 90.0);
                    }
                }
            }
            "r_sunintensity" => {
                if let Ok(intensity) = value.parse::<f32>() {
                    if intensity.is_finite() {
                        settings.sun_intensity = intensity.max(0.0);
                    }
                }
            }
            "r_suncolor" => {
                if let Some(color) = parse_vec3(value) {
                    settings.sun_color = color.map(|channel| channel.clamp(0.0, 1.0));
                }
            }
            "r_distancecullscale" => {
                if let Ok(scale) = value.parse::<f32>() {
                    if scale.is_finite() {
                        settings.distance_cull_scale = scale.clamp(0.0, MAX_DISTANCE_CULL_SCALE);
                    }
                }
            }
            "r_clouds" => settings.clouds = parse_bool(value).unwrap_or(settings.clouds),
            "r_cloudtype" => {
                settings.cloud_type = CloudType::from_config(value).unwrap_or(settings.cloud_type)
            }
            "r_cloudquality" => {
                settings.cloud_quality = value
                    .parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .map(|v| v.clamp(0.0, 1.0))
                    .unwrap_or(settings.cloud_quality)
            }
            "r_cloudcoverage" => {
                settings.cloud_coverage = value
                    .parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .map(|v| v.clamp(0.0, 1.0))
                    .unwrap_or(settings.cloud_coverage)
            }
            "r_cloudheight" => {
                settings.cloud_height = value
                    .parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .map(|v| v.clamp(CLOUD_HEIGHT_MIN, CLOUD_HEIGHT_MAX))
                    .unwrap_or(settings.cloud_height)
            }
            "r_cloudthickness" => {
                settings.cloud_thickness = value
                    .parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .map(|v| v.clamp(CLOUD_THICKNESS_MIN, CLOUD_THICKNESS_MAX))
                    .unwrap_or(settings.cloud_thickness)
            }
            // Legacy aliases from before all atmospheric systems shared one wind.
            "r_cloudwindspeed" => {
                if !weather_wind_explicit {
                    if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                        settings.weather_wind.speed = v.clamp(0.0, 8192.0);
                        legacy_cloud_wind_speed_seen = true;
                    }
                }
            }
            "r_cloudwinddirection" => {
                if !weather_wind_explicit {
                    if let Some(v) = value.parse::<f32>().ok().filter(|v| v.is_finite()) {
                        settings.weather_wind.direction = v.rem_euclid(360.0);
                        legacy_cloud_wind_direction_seen = true;
                    }
                }
            }
            "r_cloudshadows" => {
                settings.cloud_shadows = parse_bool(value).unwrap_or(settings.cloud_shadows)
            }
            "r_cloudrenderresolution" => {
                settings.cloud_render_resolution = CloudRenderResolution::from_config(value)
                    .unwrap_or(settings.cloud_render_resolution)
            }
            "r_cloudtemporal" => {
                settings.cloud_temporal = parse_bool(value).unwrap_or(settings.cloud_temporal)
            }
            "r_cloudtemporaldepthfix" => {
                settings.cloud_temporal_depth_fix =
                    parse_bool(value).unwrap_or(settings.cloud_temporal_depth_fix)
            }
            "r_cloudshear" => {
                settings.cloud_shear = value
                    .parse::<f32>()
                    .ok()
                    .map(|v| v.clamp(0.0, 1.0))
                    .unwrap_or(settings.cloud_shear)
            }
            "r_cloudbasevariation" => {
                settings.cloud_base_variation = value
                    .parse::<f32>()
                    .ok()
                    .map(|v| v.clamp(0.0, 1.0))
                    .unwrap_or(settings.cloud_base_variation)
            }
            // Obsolete boolean predecessor of weather gust/direction-variation.
            "r_cloudwindvariation" => {}
            "r_cloudshapeevolution" => {
                settings.cloud_shape_evolution =
                    parse_bool(value).unwrap_or(settings.cloud_shape_evolution)
            }
            "r_cloudterraininteraction" => {
                settings.cloud_terrain_interaction =
                    parse_bool(value).unwrap_or(settings.cloud_terrain_interaction)
            }
            "r_cloudemptyskip" => {
                settings.cloud_empty_skip = parse_bool(value).unwrap_or(settings.cloud_empty_skip)
            }
            "r_cloudaerial" => {
                settings.cloud_aerial = value
                    .parse::<f32>()
                    .ok()
                    .map(|v| v.clamp(0.0, 1.0))
                    .unwrap_or(settings.cloud_aerial)
            }
            "r_cloudskyambient" => {
                settings.cloud_sky_ambient = parse_bool(value).unwrap_or(settings.cloud_sky_ambient)
            }
            "r_cloudhistoryblend" => {
                settings.cloud_history_blend = value
                    .parse::<f32>()
                    .ok()
                    .map(|v| v.clamp(0.0, 0.98))
                    .unwrap_or(settings.cloud_history_blend)
            }
            "r_cloudmotionreject" => {
                settings.cloud_motion_reject = value
                    .parse::<f32>()
                    .ok()
                    .map(|v| v.clamp(0.0, 1.0))
                    .unwrap_or(settings.cloud_motion_reject)
            }
            "r_cloudhistorydepthreject" => {
                settings.cloud_history_depth_reject =
                    parse_bool(value).unwrap_or(settings.cloud_history_depth_reject)
            }
            "r_cloudsize" => {
                settings.cloud_size = value
                    .parse::<f32>()
                    .ok()
                    .map(|v| v.clamp(0.0, 1.0))
                    .unwrap_or(settings.cloud_size)
            }
            "r_cloudthicknessvariation" => {
                settings.cloud_thickness_variation = value
                    .parse::<f32>()
                    .ok()
                    .map(|v| v.clamp(0.0, 1.0))
                    .unwrap_or(settings.cloud_thickness_variation)
            }
            "r_rain" => settings.rain = parse_bool(value).unwrap_or(settings.rain),
            "r_grass" => settings.grass = parse_bool(value).unwrap_or(settings.grass),
            "r_ocean" => settings.ocean = parse_bool(value).unwrap_or(settings.ocean),
            "r_oceanauthoring" => {
                let v: Vec<_> = value.split_whitespace().collect();
                if v.len() == 11 {
                    let floats: Option<Vec<f32>> = v[..10].iter().map(|s| s.parse().ok()).collect();
                    if let (Some(f), Ok(seed)) = (floats, v[10].parse()) {
                        settings.ocean_settings.authored = crate::ocean::OceanAuthoring {
                            amplitude: f[0],
                            wavelength: f[1],
                            speed: f[2],
                            direction: f[3],
                            steepness: f[4],
                            slosh: f[5],
                            wind_chop: f[6],
                            foam: f[7],
                            foam_lifetime: f[8],
                            spray: f[9],
                            seed,
                        }
                        .sanitize();
                    }
                }
            }
            "r_weatherwind" => {
                let v: Option<Vec<f32>> =
                    value.split_whitespace().map(|s| s.parse().ok()).collect();
                if let Some(v) = v.filter(|v| v.len() == 4) {
                    settings.weather_wind = crate::ocean::OceanWind {
                        speed: v[0],
                        direction: v[1],
                        gust: v[2],
                        shift: v[3],
                    }
                    .sanitize();
                    weather_wind_explicit = true;
                }
            }
            // Legacy ocean weather cvar. Preserve the old cloud speed/direction
            // when present, but migrate ocean gust/shift into the shared wind.
            "r_oceanweather" => {
                if !weather_wind_explicit {
                    let v: Option<Vec<f32>> =
                        value.split_whitespace().map(|s| s.parse().ok()).collect();
                    if let Some(v) = v.filter(|v| v.len() == 4) {
                        let old = settings.weather_wind;
                        let mut wind = crate::ocean::OceanWind {
                            speed: v[0],
                            direction: v[1],
                            gust: v[2],
                            shift: v[3],
                        }
                        .sanitize();
                        if legacy_cloud_wind_speed_seen {
                            wind.speed = old.speed;
                        }
                        if legacy_cloud_wind_direction_seen {
                            wind.direction = old.direction;
                        }
                        settings.weather_wind = wind;
                    }
                }
            }
            "r_oceanmapsize" => {
                if let Ok(v) = value.parse::<u32>() {
                    settings.ocean_settings.map_size = v;
                }
            }
            "r_oceanmeshquality" => {
                if let Ok(v) = value.parse::<u8>() {
                    settings.ocean_settings.mesh_quality = v;
                }
            }
            "r_oceanupdates" => {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() {
                        settings.ocean_settings.updates_per_second = v;
                    }
                }
            }
            "r_oceanroughness" => {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() {
                        settings.ocean_settings.roughness = v;
                    }
                }
            }
            "r_oceanfogcolor" => {
                settings.ocean_settings.optics.fog_color =
                    parse_vec3(value).unwrap_or(settings.ocean_settings.optics.fog_color)
            }
            "r_oceanfogdistance"
            | "r_oceantransparency"
            | "r_oceandepthdarkening"
            | "r_oceanrefraction"
            | "r_oceancaustics"
            | "r_oceanunderwatercull" => {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() {
                        let o = &mut settings.ocean_settings.optics;
                        match name.to_ascii_lowercase().as_str() {
                            "r_oceanfogdistance" => o.fog_distance = v,
                            "r_oceantransparency" => o.transparency = v,
                            "r_oceandepthdarkening" => o.depth_darkening = v,
                            "r_oceanrefraction" => o.refraction = v,
                            "r_oceancaustics" => o.caustics = v,
                            _ => o.underwater_cull = v,
                        }
                        *o = o.sanitize();
                    }
                }
            }
            "r_oceannormalstrength" => {
                if let Ok(v) = value.parse::<f32>() {
                    if v.is_finite() {
                        settings.ocean_settings.normal_strength = v;
                    }
                }
            }
            "r_oceanseaspray" => {
                settings.ocean_settings.sea_spray =
                    parse_bool(value).unwrap_or(settings.ocean_settings.sea_spray);
            }
            "r_oceanwindfoam" => {
                settings.ocean_settings.wind_foam_streaks =
                    parse_bool(value).unwrap_or(settings.ocean_settings.wind_foam_streaks);
            }
            "r_oceanwatercolor" => {
                settings.ocean_settings.water_color =
                    parse_vec3(value).unwrap_or(settings.ocean_settings.water_color);
            }
            "r_oceanfoamcolor" => {
                settings.ocean_settings.foam_color =
                    parse_vec3(value).unwrap_or(settings.ocean_settings.foam_color);
            }
            "r_oceancascade1" => {
                settings.ocean_settings.cascades[0] =
                    parse_ocean_cascade(value, settings.ocean_settings.cascades[0])
            }
            "r_oceancascade2" => {
                settings.ocean_settings.cascades[1] =
                    parse_ocean_cascade(value, settings.ocean_settings.cascades[1])
            }
            "r_oceancascade3" => {
                settings.ocean_settings.cascades[2] =
                    parse_ocean_cascade(value, settings.ocean_settings.cascades[2])
            }
            "r_rainintensity" => {
                settings.rain_intensity =
                    RainIntensity::from_config(value).unwrap_or(settings.rain_intensity)
            }
            "r_puddlequality" => {
                settings.puddle_quality =
                    PuddleQuality::from_config(value).unwrap_or(settings.puddle_quality)
            }
            "r_puddlescatter" => {
                settings.puddle_scatter = value
                    .parse::<f32>()
                    .ok()
                    .filter(|amount| amount.is_finite())
                    .map(|amount| amount.clamp(0.0, 1.0))
                    .unwrap_or(settings.puddle_scatter)
            }
            "r_raingrade" => {
                settings.rain_grade = value
                    .parse::<f32>()
                    .ok()
                    .filter(|strength| strength.is_finite())
                    .map(|strength| strength.clamp(0.0, 1.0))
                    .unwrap_or(settings.rain_grade)
            }
            "r_footprints" => {
                settings.footprints =
                    FootprintMode::from_config(value).unwrap_or(settings.footprints)
            }
            // Keep the previous boolean cvar working for existing configs.
            "r_volumetricfog" => {
                settings.fog_mode = if parse_bool(value).unwrap_or(false) {
                    FogMode::Volumetric
                } else {
                    FogMode::Off
                }
            }
            "r_reflectionquality" => {
                settings.reflection_quality =
                    ReflectionQuality::from_config(value).unwrap_or(settings.reflection_quality)
            }
            "r_chromaticaberration" => {
                settings.chromatic_aberration = value
                    .parse::<f32>()
                    .ok()
                    .filter(|strength| strength.is_finite())
                    .map(|strength| strength.max(0.0))
                    .or_else(|| parse_bool(value).map(|enabled| if enabled { 1.0 } else { 0.0 }))
                    .unwrap_or(settings.chromatic_aberration)
            }
            "r_vignette" => settings.vignette = parse_bool(value).unwrap_or(settings.vignette),
            "r_filmgrain" => {
                settings.film_grain_strength = value
                    .parse::<f32>()
                    .ok()
                    .filter(|strength| strength.is_finite())
                    .map(|strength| strength.clamp(0.0, 1.0))
                    .or_else(|| parse_bool(value).map(|enabled| if enabled { 0.35 } else { 0.0 }))
                    .unwrap_or(settings.film_grain_strength)
            }
            "r_motionblur" => {
                settings.motion_blur_strength = value
                    .parse::<f32>()
                    .ok()
                    .filter(|strength| strength.is_finite())
                    .map(|strength| strength.clamp(0.0, 1.0))
                    .unwrap_or(settings.motion_blur_strength)
            }
            "r_depthoffield" => {
                settings.depth_of_field_strength = value
                    .parse::<f32>()
                    .ok()
                    .filter(|strength| strength.is_finite())
                    .map(|strength| strength.clamp(0.0, 1.0))
                    .unwrap_or(settings.depth_of_field_strength)
            }
            "r_dofautofocus" => {
                settings.dof_autofocus = parse_bool(value).unwrap_or(settings.dof_autofocus)
            }
            "r_dofquality" => {
                settings.dof_quality =
                    DofQuality::from_config(value).unwrap_or(settings.dof_quality)
            }
            "r_splittoning" => {
                settings.split_toning.enabled =
                    parse_bool(value).unwrap_or(settings.split_toning.enabled)
            }
            "r_splittoningstrength" => {
                settings.split_toning.strength = value
                    .parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .map(|v| v.clamp(0.0, 1.0))
                    .unwrap_or(settings.split_toning.strength)
            }
            "r_splittoningshadowhue" => {
                settings.split_toning.shadow_hue = value
                    .parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .map(|v| v.clamp(0.0, 360.0))
                    .unwrap_or(settings.split_toning.shadow_hue)
            }
            "r_splittoningshadowsaturation" => {
                settings.split_toning.shadow_saturation = value
                    .parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .map(|v| v.clamp(0.0, 1.0))
                    .unwrap_or(settings.split_toning.shadow_saturation)
            }
            "r_splittoninghighlighthue" => {
                settings.split_toning.highlight_hue = value
                    .parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .map(|v| v.clamp(0.0, 360.0))
                    .unwrap_or(settings.split_toning.highlight_hue)
            }
            "r_splittoninghighlightsaturation" => {
                settings.split_toning.highlight_saturation = value
                    .parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .map(|v| v.clamp(0.0, 1.0))
                    .unwrap_or(settings.split_toning.highlight_saturation)
            }
            "r_splittoningbalance" => {
                settings.split_toning.balance = value
                    .parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .map(|v| v.clamp(-1.0, 1.0))
                    .unwrap_or(settings.split_toning.balance)
            }
            "r_colorlut" => {
                settings.color_lut =
                    ColorLutPreset::from_config(value).unwrap_or(settings.color_lut);
            }
            "r_colorlutstrength" => {
                settings.color_lut_strength = value
                    .parse::<f32>()
                    .ok()
                    .filter(|strength| strength.is_finite())
                    .map(|strength| strength.clamp(0.0, 1.0))
                    .unwrap_or(settings.color_lut_strength);
            }
            "r_gpudriven" => settings.gpu_driven = parse_bool(value).unwrap_or(settings.gpu_driven),
            "r_hizocclusion" => {
                settings.hiz_occlusion = parse_bool(value).unwrap_or(settings.hiz_occlusion)
            }
            "r_entityambientlighting" => {
                settings.entity_ambient_lighting = EntityAmbientLightingMode::from_config(value)
                    .unwrap_or(settings.entity_ambient_lighting)
            }
            "r_dynamiclights" => {
                if let Some(mode) = DynamicLightsMode::from_config(value) {
                    settings.dynamic_lights = mode;
                }
            }
            "r_rtsamples" => {
                if let Ok(samples @ (1 | 2 | 4)) = value.parse::<u32>() {
                    settings.rt_samples = samples;
                }
            }
            "r_dynamiclightfalloff" => {
                if let Ok(mode @ (0 | 1)) = value.parse::<u32>() {
                    settings.dynamic_light_falloff = mode;
                }
            }
            "r_rtresolution" => match value.to_ascii_lowercase().as_str() {
                "full" | "1" => settings.rt_half_resolution = false,
                "half" | "0.5" => settings.rt_half_resolution = true,
                _ => {}
            },
            "r_maplightsimulation" => {
                settings.map_light_simulation =
                    parse_bool(value).unwrap_or(settings.map_light_simulation);
            }
            "r_fullbright" => {
                settings.world_lighting = !parse_bool(value).unwrap_or(!settings.world_lighting);
            }
            "r_vertexlight" => {
                settings.vertex_lighting = parse_bool(value).unwrap_or(settings.vertex_lighting);
            }
            "r_lightmap" => {
                settings.lightmap_only = parse_bool(value).unwrap_or(settings.lightmap_only);
            }
            "r_modernsabers" => {
                settings.modern_sabers = parse_bool(value).unwrap_or(settings.modern_sabers)
            }
            "r_flares" => settings.flares = parse_bool(value).unwrap_or(settings.flares),
            "r_saberimpactfx" => {
                settings.saber_impact_fx = parse_bool(value).unwrap_or(settings.saber_impact_fx)
            }
            "r_sabermarks" => {
                settings.saber_marks =
                    SaberMarkMode::from_config(value).unwrap_or(settings.saber_marks)
            }
            "r_pbr" => settings.pbr = parse_bool(value).unwrap_or(settings.pbr),
            "fs_allowassetoverrides" => {
                settings.allow_asset_overrides =
                    parse_bool(value).unwrap_or(settings.allow_asset_overrides)
            }
            "r_gennormalmaps" => {
                settings.gen_normal_maps = parse_bool(value).unwrap_or(settings.gen_normal_maps)
            }
            "r_deluxemapping" => {
                settings.deluxe_mapping = parse_bool(value).unwrap_or(settings.deluxe_mapping)
            }
            "r_deluxespecular" => {
                settings.deluxe_specular = value
                    .parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .map(|v| v.clamp(0.0, 1.0))
                    .unwrap_or(settings.deluxe_specular);
            }
            "r_dynamicshadows" => {
                if let Some(mode) = DynamicShadowsMode::from_config(value) {
                    settings.dynamic_shadows = mode;
                    dynamic_shadows_mode_seen = true;
                }
            }
            "r_emissivearealights" => {
                settings.emissive_area_lights =
                    parse_bool(value).unwrap_or(settings.emissive_area_lights)
            }
            "r_voxelprobegi" => {
                settings.voxel_probe_gi = parse_bool(value).unwrap_or(settings.voxel_probe_gi)
            }
            "r_entityshadowlight" => {
                settings.entity_shadow_light =
                    EntityShadowLight::from_config(value).unwrap_or(settings.entity_shadow_light)
            }
            "r_locallightshadows" => {
                settings.local_light_shadows =
                    parse_bool(value).unwrap_or(settings.local_light_shadows)
            }
            "r_cascadedshadows" => {
                settings.cascaded_shadows = parse_bool(value).unwrap_or(settings.cascaded_shadows)
            }
            "r_bakedbrightness" => {
                // Migration only. The archive now writes a single r_gammaMethod.
                if !explicit_gamma_method {
                    if let Some(enabled) = parse_bool(value) {
                        settings.gamma_method = if enabled {
                            crate::gamma::GammaMethod::Baked
                        } else {
                            crate::gamma::GammaMethod::Shader
                        };
                    }
                }
            }
            "r_gammamethod" => {
                if let Some(method) = crate::gamma::GammaMethod::parse(value) {
                    settings.gamma_method = method;
                    explicit_gamma_method = true;
                }
            }
            "r_gamma" => {
                if let Ok(gamma) = value.parse::<f32>() {
                    if gamma.is_finite() {
                        settings.gamma = gamma.clamp(GAMMA_MIN, GAMMA_MAX);
                    }
                }
            }
            "r_modelbrightness" => {
                if let Ok(value) = value.parse::<f32>() {
                    if value.is_finite() {
                        settings.model_brightness = value.clamp(GAMMA_MIN, GAMMA_MAX);
                    }
                }
            }
            "r_dynamiclightbrightness" => {
                if let Ok(value) = value.parse::<f32>() {
                    if value.is_finite() {
                        settings.dynamic_light_brightness = value.clamp(GAMMA_MIN, GAMMA_MAX);
                    }
                }
            }
            "r_modelbrightnesslock" => {
                settings.model_brightness_locked =
                    parse_bool(value).unwrap_or(settings.model_brightness_locked);
            }
            "r_dynamiclightbrightnesslock" => {
                settings.dynamic_light_brightness_locked =
                    parse_bool(value).unwrap_or(settings.dynamic_light_brightness_locked);
            }
            "r_drawmapmodels" => {
                settings.draw_map_models = parse_bool(value).unwrap_or(settings.draw_map_models);
            }
            _ => {}
        }
    }
    // Linked brightness sliders always mirror the master, whatever order the
    // config lines were read in.
    settings.sync_linked_brightness();
    // r_dynamicLights is the source of truth for all runtime-light techniques.
    if dynamic_shadows_mode_seen {
        settings.cascaded_shadows =
            settings.dynamic_shadows == DynamicShadowsMode::CascadedShadowMaps;
    } else {
        settings.dynamic_shadows = if settings.cascaded_shadows {
            DynamicShadowsMode::CascadedShadowMaps
        } else {
            DynamicShadowsMode::Off
        };
    }

    // Before r_tonemap existed, r_hdr controlled both the floating-point
    // working buffer and ACES. Preserve that visual behavior once when loading
    // an older config; the next save archives the two settings separately.
    if !tone_mapping_seen {
        settings.tone_mapping = settings.hdr;
    }
    settings.ocean_settings.wind = settings.weather_wind;
    if let (Some(width), Some(height)) = (custom_width, custom_height) {
        settings.resolution = [width, height];
    }
    if let (Some(x), Some(y)) = (window_x, window_y) {
        settings.window_position = Some([x, y]);
    }
    if novis == Some(true) {
        settings.pvs_mode = PvsMode::Off;
    } else if let Some(mode) = pvs_mode {
        settings.pvs_mode = mode;
    }
    if let Some(level) = anisotropy {
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
    let texture_mode = match settings.texture_filter {
        TextureFilter::Nearest => "GL_NEAREST",
        TextureFilter::Bilinear => "GL_LINEAR_MIPMAP_NEAREST",
        TextureFilter::Trilinear
        | TextureFilter::Anisotropic2x
        | TextureFilter::Anisotropic4x
        | TextureFilter::Anisotropic8x
        | TextureFilter::Anisotropic16x => "GL_LINEAR_MIPMAP_LINEAR",
    };
    let anisotropy = match settings.texture_filter {
        TextureFilter::Anisotropic2x => 2,
        TextureFilter::Anisotropic4x => 4,
        TextureFilter::Anisotropic8x => 8,
        TextureFilter::Anisotropic16x => 16,
        _ => 0,
    };
    let msaa = if settings.msaa_samples <= 1 {
        0
    } else {
        settings.msaa_samples
    };
    let pvs_mode = settings.pvs_mode.config_value();
    let window_x = settings
        .window_position
        .map_or_else(|| "auto".to_owned(), |position| position[0].to_string());
    let window_y = settings
        .window_position
        .map_or_else(|| "auto".to_owned(), |position| position[1].to_string());
    let text = format!(
        "// JKA Rust client video settings. JKA-style archived cvars.\n\
seta r_mode \"-1\"\n\
seta r_customwidth \"{}\"\n\
seta r_customheight \"{}\"\n\
seta r_windowX \"{}\"\n\
seta r_windowY \"{}\"\n\
seta r_windowMaximized \"{}\"\n\
seta r_fullscreen \"{}\"\n\
seta r_backend \"{}\"\n\
seta r_swapInterval \"{}\"\n\
seta r_maxFrameLatency \"{}\"\n\
seta r_ext_multisample \"{}\"\n\
seta r_textureMode \"{}\"\n\
seta r_picmip \"{}\"\n\
seta r_ext_texture_filter_anisotropic \"{}\"\n\
seta r_detailTextures \"{}\"\n\
seta r_detailTextureFade \"{}\"\n\
seta r_detailTextureFadeDistance \"{:.0}\"\n\
seta r_showtris \"{}\"\n\
seta r_skipUi \"{}\"\n\
seta developer \"{}\"\n\
seta r_verbose \"{}\"\n\
seta r_perfTrace \"{}\"\n\
seta r_worldPath \"{}\"\n\
seta r_gpuTimings \"{}\"\n\
seta r_ghoul2Skinning \"{}\"\n\
seta r_ghoul2EarlyCull \"{}\"\n\
seta r_lodbias \"{}\"\n\
seta r_ghoul2BatchDraws \"{}\"\n\
seta r_ghoul2AnimSmooth \"{}\"\n\
seta r_novis \"{}\"\n\
seta r_pvsMode \"{}\"\n\
seta com_maxfps \"{}\"\n\
seta cg_drawFPS \"{}\"\n\
seta cg_drawTimer \"{}\"\n\
seta pmove_msec \"{}\"\n\
seta cl_timerResolution1ms \"{}\"\n\
seta cl_input_latelatch \"{}\"\n\
seta r_gamma \"{:.3}\"\n\
seta r_hdr \"{}\"\n\
seta r_floatLightmap \"{}\"\n\
seta r_tonemap \"{}\"\n\
seta r_autoExposure \"{}\"\n\
seta r_bloom \"{}\"\n\
seta r_halation \"{}\"\n\
seta r_ssao \"{}\"\n\
seta r_staticBspAo \"{}\"\n\
seta r_staticBspAoSamples \"{}\"\n\
seta r_staticBspAoResolution \"{}\"\n\
seta r_staticBspAoStrength \"{}\"\n\
seta r_staticBspAoRange \"{}\"\n\
seta r_staticBspAoCurrentCell \"{}\"\n\
seta r_fxaa \"{}\"\n\
seta r_smaa \"{}\"\n\
seta r_taa \"{}\"\n\
seta r_contactShadows \"{}\"\n\
seta r_drawfog \"{}\"\n\
seta r_fogStrength \"{:.3}\"\n\
seta r_sunOverride \"{}\"\n\
seta r_sunVisibility \"{}\"\n\
seta r_entitySunLighting \"{}\"\n\
seta r_sunYaw \"{:.3}\"\n\
seta r_sunPitch \"{:.3}\"\n\
seta r_sunIntensity \"{:.3}\"\n\
seta r_sunColor \"{:.4} {:.4} {:.4}\"\n\
seta r_distanceCullScale \"{:.3}\"\n\
seta r_clouds \"{}\"\n\
seta r_cloudType \"{}\"\n\
seta r_cloudQuality \"{:.3}\"\n\
seta r_cloudCoverage \"{:.3}\"\n\
seta r_cloudHeight \"{:.1}\"\n\
seta r_cloudThickness \"{:.1}\"\n\
seta r_weatherWind \"{:.2} {:.1} {:.3} {:.1}\"\n\
seta r_cloudShadows \"{}\"\n\
seta r_cloudRenderResolution \"{}\"\n\
seta r_cloudTemporal \"{}\"\n\
seta r_cloudTemporalDepthFix \"{}\"\n\
seta r_cloudShear \"{:.3}\"\n\
seta r_cloudBaseVariation \"{:.3}\"\n\
seta r_cloudShapeEvolution \"{}\"\n\
seta r_cloudTerrainInteraction \"{}\"\n\
seta r_cloudEmptySkip \"{}\"\n\
seta r_cloudAerial \"{:.3}\"\n\
seta r_cloudSkyAmbient \"{}\"\n\
seta r_cloudHistoryBlend \"{:.3}\"\n\
seta r_cloudMotionReject \"{:.3}\"\n\
seta r_cloudHistoryDepthReject \"{}\"\n\
seta r_cloudThicknessVariation \"{:.3}\"\n\
seta r_cloudSize \"{:.3}\"\n\
seta r_rain \"{}\"\n\
seta r_rainIntensity \"{}\"\n\
seta r_puddleQuality \"{}\"\n\
seta r_puddleScatter \"{:.3}\"\n\
seta r_rainGrade \"{:.3}\"\n\
seta r_grass \"{}\"\n\
seta r_ocean \"{}\"\n\
seta r_oceanMapSize \"{}\"\n\
seta r_oceanMeshQuality \"{}\"\n\
seta r_oceanUpdates \"{:.2}\"\n\
seta r_oceanRoughness \"{:.3}\"\n\
seta r_oceanNormalStrength \"{:.3}\"\n\
seta r_oceanSeaSpray \"{}\"\n\
seta r_oceanWindFoam \"{}\"\n\
seta r_oceanWaterColor \"{:.4} {:.4} {:.4}\"\n\
seta r_oceanFoamColor \"{:.4} {:.4} {:.4}\"\n\
seta r_oceanCascade1 \"{:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4}\"\n\
seta r_oceanCascade2 \"{:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4}\"\n\
seta r_oceanCascade3 \"{:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4}\"\n\
seta r_footprints \"{}\"\n\
seta r_reflectionQuality \"{}\"\n\
seta r_chromaticAberration \"{:.3}\"\n\
seta r_vignette \"{}\"\n\
seta r_filmGrain \"{:.3}\"\n\
seta r_motionBlur \"{:.3}\"\n\
seta r_depthOfField \"{:.3}\"\n\
seta r_dofAutoFocus \"{}\"\n\
seta r_dofQuality \"{}\"\n\
seta r_colorLut \"{}\"\n\
seta r_colorLutStrength \"{:.3}\"\n\
seta r_gpuDriven \"{}\"\n\
seta r_hizOcclusion \"{}\"\n\
seta r_entityAmbientLighting \"{}\"\n\
seta r_dynamicLights \"{}\"\n\
seta r_mapLightSimulation \"{}\"\n\
seta r_modernSabers \"{}\"\n\
seta r_flares \"{}\"\n\
seta r_saberImpactFx \"{}\"\n\
seta r_saberMarks \"{}\"\n\
seta r_pbr \"{}\"\n\
seta fs_allowAssetOverrides \"{}\"\n\
seta r_genNormalMaps \"{}\"\n\
seta r_deluxeMapping \"{}\"\n\
seta r_deluxeSpecular \"{:.3}\"\n\
seta r_dynamicShadows \"{}\"\n\
seta r_emissiveAreaLights \"{}\"\n\
seta r_voxelProbeGI \"{}\"\n\
seta r_localLightShadows \"{}\"\n\
seta r_cascadedShadows \"{}\"\n\
seta r_entityShadowLight \"{}\"\n\
",
        settings.resolution[0],
        settings.resolution[1],
        window_x,
        window_y,
        u8::from(settings.window_maximized),
        settings.fullscreen.config_value(),
        settings.renderer_backend.config_value(),
        settings.vsync.config_value(),
        settings.max_frame_latency,
        msaa,
        texture_mode,
        settings.picmip,
        anisotropy,
        settings.detail_textures.config_value(),
        u8::from(settings.detail_texture_fade),
        settings.detail_texture_fade_distance,
        settings.wireframe_mask,
        u8::from(settings.skip_ui),
        settings.developer_level,
        settings.renderer_verbose,
        u8::from(settings.perf_trace),
        if settings.force_unified_world {
            "unified"
        } else {
            "auto"
        },
        u8::from(settings.gpu_timings),
        settings.ghoul2_skinning.config_value(),
        u8::from(settings.ghoul2_early_cull),
        settings.ghoul2_lod_bias,
        settings.ghoul2_batch_draws.config_value(),
        settings.ghoul2_anim_smooth,
        u8::from(settings.pvs_mode == PvsMode::Off),
        pvs_mode,
        settings.fps_cap,
        settings.draw_fps,
        u8::from(settings.draw_timer),
        settings.physics_msec,
        u8::from(settings.timer_resolution_1ms),
        u8::from(settings.input_latelatch),
        settings.gamma,
        u8::from(settings.hdr),
        u8::from(settings.float_lightmap),
        u8::from(settings.tone_mapping),
        u8::from(settings.auto_exposure),
        u8::from(settings.bloom),
        u8::from(settings.halation),
        u8::from(settings.ssao),
        u8::from(settings.static_bsp_ao),
        settings.static_bsp_ao_samples,
        settings.static_bsp_ao_resolution,
        settings.static_bsp_ao_strength,
        settings.static_bsp_ao_range,
        u8::from(settings.static_bsp_ao_current_cell),
        u8::from(settings.fxaa),
        u8::from(settings.smaa),
        u8::from(settings.taa),
        u8::from(settings.contact_shadows),
        settings.fog_mode.drawfog_value(),
        settings.fog_strength,
        u8::from(settings.sun_override),
        settings.sun_visibility.config_value(),
        u8::from(settings.entity_sun_lighting),
        settings.sun_yaw,
        settings.sun_pitch,
        settings.sun_intensity,
        settings.sun_color[0],
        settings.sun_color[1],
        settings.sun_color[2],
        settings.distance_cull_scale,
        u8::from(settings.clouds),
        settings.cloud_type.config_value(),
        settings.cloud_quality,
        settings.cloud_coverage,
        settings.cloud_height,
        settings.cloud_thickness,
        settings.weather_wind.speed,
        settings.weather_wind.direction,
        settings.weather_wind.gust,
        settings.weather_wind.shift,
        u8::from(settings.cloud_shadows),
        settings.cloud_render_resolution.config_value(),
        u8::from(settings.cloud_temporal),
        u8::from(settings.cloud_temporal_depth_fix),
        settings.cloud_shear,
        settings.cloud_base_variation,
        u8::from(settings.cloud_shape_evolution),
        u8::from(settings.cloud_terrain_interaction),
        u8::from(settings.cloud_empty_skip),
        settings.cloud_aerial,
        u8::from(settings.cloud_sky_ambient),
        settings.cloud_history_blend,
        settings.cloud_motion_reject,
        u8::from(settings.cloud_history_depth_reject),
        settings.cloud_thickness_variation,
        settings.cloud_size,
        u8::from(settings.rain),
        settings.rain_intensity.config_value(),
        settings.puddle_quality.config_value(),
        settings.puddle_scatter,
        settings.rain_grade,
        u8::from(settings.grass),
        u8::from(settings.ocean),
        settings.ocean_settings.map_size,
        settings.ocean_settings.mesh_quality,
        settings.ocean_settings.updates_per_second,
        settings.ocean_settings.roughness,
        settings.ocean_settings.normal_strength,
        u8::from(settings.ocean_settings.sea_spray),
        u8::from(settings.ocean_settings.wind_foam_streaks),
        settings.ocean_settings.water_color[0],
        settings.ocean_settings.water_color[1],
        settings.ocean_settings.water_color[2],
        settings.ocean_settings.foam_color[0],
        settings.ocean_settings.foam_color[1],
        settings.ocean_settings.foam_color[2],
        settings.ocean_settings.cascades[0].tile_length[0],
        settings.ocean_settings.cascades[0].tile_length[1],
        settings.ocean_settings.cascades[0].displacement_scale,
        settings.ocean_settings.cascades[0].normal_scale,
        settings.ocean_settings.cascades[0].wind_speed,
        settings.ocean_settings.cascades[0].wind_direction,
        settings.ocean_settings.cascades[0].fetch_length,
        settings.ocean_settings.cascades[0].swell,
        settings.ocean_settings.cascades[0].spread,
        settings.ocean_settings.cascades[0].detail,
        settings.ocean_settings.cascades[0].whitecap,
        settings.ocean_settings.cascades[0].foam_amount,
        settings.ocean_settings.cascades[1].tile_length[0],
        settings.ocean_settings.cascades[1].tile_length[1],
        settings.ocean_settings.cascades[1].displacement_scale,
        settings.ocean_settings.cascades[1].normal_scale,
        settings.ocean_settings.cascades[1].wind_speed,
        settings.ocean_settings.cascades[1].wind_direction,
        settings.ocean_settings.cascades[1].fetch_length,
        settings.ocean_settings.cascades[1].swell,
        settings.ocean_settings.cascades[1].spread,
        settings.ocean_settings.cascades[1].detail,
        settings.ocean_settings.cascades[1].whitecap,
        settings.ocean_settings.cascades[1].foam_amount,
        settings.ocean_settings.cascades[2].tile_length[0],
        settings.ocean_settings.cascades[2].tile_length[1],
        settings.ocean_settings.cascades[2].displacement_scale,
        settings.ocean_settings.cascades[2].normal_scale,
        settings.ocean_settings.cascades[2].wind_speed,
        settings.ocean_settings.cascades[2].wind_direction,
        settings.ocean_settings.cascades[2].fetch_length,
        settings.ocean_settings.cascades[2].swell,
        settings.ocean_settings.cascades[2].spread,
        settings.ocean_settings.cascades[2].detail,
        settings.ocean_settings.cascades[2].whitecap,
        settings.ocean_settings.cascades[2].foam_amount,
        settings.footprints.config_value(),
        settings.reflection_quality.config_value(),
        settings.chromatic_aberration,
        u8::from(settings.vignette),
        settings.film_grain_strength,
        settings.motion_blur_strength,
        settings.depth_of_field_strength,
        u8::from(settings.dof_autofocus),
        settings.dof_quality.config_value(),
        settings.color_lut.config_value(),
        settings.color_lut_strength,
        u8::from(settings.gpu_driven),
        u8::from(settings.hiz_occlusion),
        settings.entity_ambient_lighting.config_value(),
        settings.dynamic_lights.config_value(),
        u8::from(settings.map_light_simulation),
        u8::from(settings.modern_sabers),
        u8::from(settings.flares),
        u8::from(settings.saber_impact_fx),
        settings.saber_marks.config_value(),
        u8::from(settings.pbr),
        u8::from(settings.allow_asset_overrides),
        u8::from(settings.gen_normal_maps),
        u8::from(settings.deluxe_mapping),
        settings.deluxe_specular,
        settings.dynamic_shadows.config_value(),
        u8::from(settings.emissive_area_lights),
        u8::from(settings.voxel_probe_gi),
        u8::from(settings.local_light_shadows),
        u8::from(settings.cascaded_shadows),
        settings.entity_shadow_light.config_value(),
    );
    use std::fmt::Write as _;
    let mut text = text;
    let _ = writeln!(
        text,
        "seta r_gammaMethod \"{}\"",
        settings.gamma_method.config_value()
    );
    let _ = writeln!(
        text,
        "seta r_splitToning \"{}\"",
        u8::from(settings.split_toning.enabled)
    );
    let _ = writeln!(
        text,
        "seta r_splitToningStrength \"{:.3}\"",
        settings.split_toning.strength
    );
    let _ = writeln!(
        text,
        "seta r_splitToningShadowHue \"{:.3}\"",
        settings.split_toning.shadow_hue
    );
    let _ = writeln!(
        text,
        "seta r_splitToningShadowSaturation \"{:.3}\"",
        settings.split_toning.shadow_saturation
    );
    let _ = writeln!(
        text,
        "seta r_splitToningHighlightHue \"{:.3}\"",
        settings.split_toning.highlight_hue
    );
    let _ = writeln!(
        text,
        "seta r_splitToningHighlightSaturation \"{:.3}\"",
        settings.split_toning.highlight_saturation
    );
    let _ = writeln!(
        text,
        "seta r_splitToningBalance \"{:.3}\"",
        settings.split_toning.balance
    );
    writeln!(text, "seta r_rtSamples \"{}\"", settings.rt_samples).unwrap();
    writeln!(
        text,
        "seta r_dynamicLightFalloff \"{}\"",
        settings.dynamic_light_falloff
    )
    .unwrap();
    writeln!(
        text,
        "seta r_rtResolution \"{}\"",
        if settings.rt_half_resolution {
            "half"
        } else {
            "full"
        }
    )
    .unwrap();
    text.push_str(&format!(
        "seta r_fullbright \"{}\"\nseta r_vertexLight \"{}\"\nseta r_lightmap \"{}\"\n",
        u8::from(!settings.world_lighting),
        u8::from(settings.vertex_lighting),
        u8::from(settings.lightmap_only),
    ));
    let _ = writeln!(text, "seta cg_fxFPS \"{}\"", settings.fx_fps);
    let _ = writeln!(text, "seta cg_fxFPSScope \"{}\"", settings.fx_fps_scope);
    let _ = writeln!(text, "seta fx_physics \"{}\"", settings.fx_physics);
    let _ = writeln!(text, "seta fx_lod \"{}\"", settings.fx_lod);
    let _ = writeln!(text, "seta fx_countScale \"{}\"", settings.fx_count_scale);
    let _ = writeln!(text, "seta r_fxLodScale \"{}\"", settings.fx_lod_scale);
    let _ = writeln!(text, "seta r_lodScale \"{}\"", settings.lod_scale);
    let _ = writeln!(
        text,
        "seta r_fxGeometry \"{}\"",
        settings.fx_geometry.config_value()
    );
    let _ = writeln!(
        text,
        "seta r_fxZeroAlphaDiscard \"{}\"",
        u8::from(settings.fx_zero_alpha_discard)
    );
    let _ = writeln!(
        text,
        "seta r_drawMapModels \"{}\"",
        u8::from(settings.draw_map_models)
    );
    let _ = writeln!(
        text,
        "seta r_modelBrightness \"{:.3}\"",
        settings.model_brightness
    );
    let _ = writeln!(
        text,
        "seta r_modelBrightnessLock \"{}\"",
        u8::from(settings.model_brightness_locked)
    );
    let _ = writeln!(
        text,
        "seta r_dynamicLightBrightness \"{:.3}\"",
        settings.dynamic_light_brightness
    );
    let _ = writeln!(
        text,
        "seta r_dynamicLightBrightnessLock \"{}\"",
        u8::from(settings.dynamic_light_brightness_locked)
    );
    let _ = writeln!(text, "seta r_jiggleSolver \"{}\"", settings.jiggle_solver);
    let _ = writeln!(
        text,
        "seta r_jiggleStrength \"{:.3}\"",
        settings.jiggle_strength
    );
    let _ = writeln!(
        text,
        "seta r_jiggleBreastStrength \"{:.3}\"",
        settings.jiggle_breast_strength
    );
    let _ = writeln!(
        text,
        "seta r_jiggleGluteStrength \"{:.3}\"",
        settings.jiggle_glute_strength
    );
    let _ = writeln!(
        text,
        "seta r_jiggleStiffness \"{:.3}\"",
        settings.jiggle_stiffness
    );
    let _ = writeln!(
        text,
        "seta r_jiggleDamping \"{:.3}\"",
        settings.jiggle_damping
    );
    let _ = writeln!(
        text,
        "seta r_jiggleGluteLift \"{:.3}\"",
        settings.jiggle_glute_lift
    );
    let _ = writeln!(
        text,
        "seta r_jiggleJPStiffness \"{:.3}\"",
        settings.jiggle_jp_stiffness
    );
    let _ = writeln!(
        text,
        "seta r_jiggleJPDrag \"{:.3}\"",
        settings.jiggle_jp_drag
    );
    let _ = writeln!(
        text,
        "seta r_jiggleJPAirDrag \"{:.3}\"",
        settings.jiggle_jp_air_drag
    );
    let _ = writeln!(
        text,
        "seta r_jiggleJPStretch \"{:.3}\"",
        settings.jiggle_jp_stretch
    );
    let _ = writeln!(
        text,
        "seta r_jiggleJPSoften \"{:.3}\"",
        settings.jiggle_jp_soften
    );
    let _ = writeln!(
        text,
        "seta r_jiggleJPGravity \"{:.3}\"",
        settings.jiggle_jp_gravity
    );
    let _ = writeln!(
        text,
        "seta r_clothBodyClearance \"{:.3}\"",
        settings.cloth_body_clearance
    );
    let _ = writeln!(
        text,
        "seta r_clothAirResistance \"{:.3}\"",
        settings.cloth_air_resistance
    );
    let _ = writeln!(
        text,
        "seta r_clothTurnResponse \"{:.3}\"",
        settings.cloth_turn_response
    );
    let _ = writeln!(
        text,
        "seta r_clothAnimationInfluence \"{:.3}\"",
        settings.cloth_animation_influence
    );
    let _ = writeln!(
        text,
        "seta r_clothWind \"{}\"",
        u8::from(settings.cloth_wind)
    );
    let _ = writeln!(text, "seta cg_dismember \"{}\"", settings.dismemberment);
    let _ = writeln!(text, "seta r_dismemberMax \"{}\"", settings.dismember_max);
    let _ = writeln!(
        text,
        "seta r_dismemberLifetime \"{:.1}\"",
        settings.dismember_lifetime
    );
    text.push_str(&format!(
        "seta r_physics \"{}\"\n\
seta r_physicsHz \"{}\"\n\
seta r_physicsMaxSubsteps \"{}\"\n\
seta r_physicsCCD \"{}\"\n\
seta r_physicsSleeping \"{}\"\n\
seta r_ragdolls \"{}\"\n\
seta r_ragdollMax \"{}\"\n\
seta r_ragdollLifetime \"{:.1}\"\n\
seta r_ragdollSelfCollision \"{}\"\n\
seta r_jigglePhysics \"{}\"\n\
seta r_clothPhysics \"{}\"\n\
seta r_clothBodyCollision \"{}\"\n\
seta r_physicsProps \"{}\"\n\
seta r_physicsPropMax \"{}\"\n\
seta r_physicsDebris \"{}\"\n\
seta r_physicsDebrisMax \"{}\"\n\
seta r_physicsDebrisLifetime \"{:.1}\"\n\
seta r_physicsPlayerPush \"{}\"\n\
seta r_physicsWeaponImpulses \"{}\"\n\
seta r_physicsExplosionImpulses \"{}\"\n\
seta r_physicsForceImpulses \"{}\"\n\
seta r_physicsDebug \"{}\"\n\
seta r_physicsStats \"{}\"\n",
        u8::from(settings.client_physics),
        settings.client_physics_hz,
        settings.client_physics_max_substeps,
        u8::from(settings.client_physics_ccd),
        u8::from(settings.client_physics_sleeping),
        u8::from(settings.ragdolls),
        settings.ragdoll_max,
        settings.ragdoll_lifetime,
        u8::from(settings.ragdoll_self_collision),
        u8::from(settings.jiggle_physics),
        u8::from(settings.cloth_physics),
        u8::from(settings.cloth_body_collision),
        u8::from(settings.physics_props),
        settings.physics_prop_max,
        u8::from(settings.physics_debris),
        settings.physics_debris_max,
        settings.physics_debris_lifetime,
        u8::from(settings.physics_player_push),
        u8::from(settings.physics_weapon_impulses),
        u8::from(settings.physics_explosion_impulses),
        u8::from(settings.physics_force_impulses),
        u8::from(settings.physics_debug_draw),
        u8::from(settings.physics_stats),
    ));
    let _ = writeln!(text, "seta model \"{}\"", presentation.model);
    let _ = writeln!(text, "seta cg_forceModel \"{}\"", presentation.force_model);
    let _ = writeln!(
        text,
        "seta cg_drawCrosshair \"{}\"",
        presentation.crosshair.style
    );
    let _ = writeln!(
        text,
        "seta cg_crosshairImage \"{}\"",
        presentation.crosshair.image
    );
    let _ = writeln!(
        text,
        "seta cg_crosshairSize \"{:.3}\"",
        presentation.crosshair.size
    );
    let _ = writeln!(
        text,
        "seta cg_crosshairStrength \"{:.3}\"",
        presentation.crosshair.strength
    );
    let [r, g, b, a] = presentation.crosshair.color;
    let _ = writeln!(text, "seta cg_crosshairColor \"{r} {g} {b} {a}\"");
    let _ = writeln!(
        text,
        "seta cg_dynamicCrosshair \"{}\"",
        presentation.crosshair.dynamic
    );
    let _ = writeln!(
        text,
        "seta cg_crosshairIdentifyTarget \"{}\"",
        u8::from(presentation.crosshair.identify_target)
    );
    let _ = writeln!(
        text,
        "seta cg_drawCrosshairNames \"{}\"",
        presentation.crosshair.names
    );
    let _ = writeln!(
        text,
        "seta cg_drawCrosshairNamesColours \"{}\"",
        u8::from(presentation.crosshair.names_colours)
    );
    let _ = writeln!(
        text,
        "seta cg_drawCrosshairNamesOpacity \"{}\"",
        presentation.crosshair.names_opacity
    );
    let _ = writeln!(
        text,
        "seta cg_drawPlayerNames \"{}\"",
        presentation.player_names.mode
    );
    let _ = writeln!(
        text,
        "seta cg_drawPlayerNamesScale \"{:.3}\"",
        presentation.player_names.scale
    );
    for id in HudElementId::ALL {
        let _ = writeln!(
            text,
            "seta {} \"{}\"",
            id.cvar_name(),
            presentation.hud_layout.element(id).to_config()
        );
    }
    let _ = writeln!(
        text,
        "seta cg_hudSnap \"{}\"",
        u8::from(presentation.hud_layout.snap_to_grid)
    );
    let _ = writeln!(
        text,
        "seta cg_hudGridSize \"{:.3}\"",
        presentation.hud_layout.grid_size
    );
    let _ = writeln!(
        text,
        "seta cg_movementKeys \"{}\"",
        presentation.movement_keys.mode
    );
    let _ = writeln!(
        text,
        "seta cg_movementKeysX \"{:.3}\"",
        presentation.movement_keys.x
    );
    let _ = writeln!(
        text,
        "seta cg_movementKeysY \"{:.3}\"",
        presentation.movement_keys.y
    );
    let _ = writeln!(
        text,
        "seta cg_movementKeysSize \"{:.3}\"",
        presentation.movement_keys.size
    );
    let _ = writeln!(
        text,
        "seta cg_movementKeysWalk \"{}\"",
        u8::from(presentation.movement_keys.walk)
    );
    let _ = writeln!(
        text,
        "seta cg_strafeHelper \"{}\"",
        presentation.strafe_helper.flags
    );
    let _ = writeln!(
        text,
        "seta cg_strafeHelper_FPS \"{:.3}\"",
        presentation.strafe_helper.fps
    );
    let _ = writeln!(
        text,
        "seta cg_strafeHelperOffset \"{:.3}\"",
        presentation.strafe_helper.offset
    );
    let _ = writeln!(
        text,
        "seta cg_strafeHelperLineWidth \"{:.3}\"",
        presentation.strafe_helper.line_width
    );
    let _ = writeln!(
        text,
        "seta cg_strafeHelperPrecision \"{}\"",
        presentation.strafe_helper.precision
    );
    let _ = writeln!(
        text,
        "seta cg_strafeHelperCutoff \"{:.3}\"",
        presentation.strafe_helper.cutoff
    );
    let [sr, sg, sb, sa] = presentation.strafe_helper.active_color;
    let _ = writeln!(
        text,
        "seta cg_strafeHelperActiveColor \"{sr} {sg} {sb} {sa}\""
    );
    let _ = writeln!(
        text,
        "seta cg_strafeHelperInactiveAlpha \"{}\"",
        presentation.strafe_helper.inactive_alpha
    );
    let _ = writeln!(
        text,
        "seta cg_strafeTrailRadius \"{:.3}\"",
        presentation.strafe_trail.radius
    );
    let _ = writeln!(
        text,
        "seta cg_strafeTrailLife \"{:.3}\"",
        presentation.strafe_trail.life_seconds
    );
    let _ = writeln!(
        text,
        "seta cg_strafeTrailFPS \"{:.3}\"",
        presentation.strafe_trail.fps
    );
    let _ = writeln!(
        text,
        "seta cg_strafeTrailPlums \"{}\"",
        u8::from(presentation.strafe_trail.plums)
    );
    let _ = writeln!(
        text,
        "seta cg_strafeTrailGhost \"{}\"",
        u8::from(presentation.strafe_trail.ghost)
    );
    let _ = writeln!(
        text,
        "seta cg_strafeTrailPlayers \"{}\"",
        presentation.strafe_trail.players
    );
    let _ = writeln!(
        text,
        "seta cg_logStrafeTrail \"{}\"",
        presentation.strafe_trail.log_name
    );
    let _ = writeln!(
        text,
        "seta cg_strafeTrailDistance \"{:.1}\"",
        presentation.strafe_trail.draw_distance
    );
    let _ = writeln!(
        text,
        "seta cg_rGhostAlpha \"{:.3}\"",
        presentation.race_ghost_alpha
    );
    let _ = writeln!(
        text,
        "seta cg_rGhostName \"{}\"",
        u8::from(presentation.race_ghost_name)
    );
    let _ = writeln!(
        text,
        "seta cg_rGhostTrail \"{}\"",
        u8::from(presentation.race_ghost_trail)
    );
    let _ = writeln!(
        text,
        "seta cg_rGhostVelocityDelta \"{}\"",
        u8::from(presentation.race_ghost_velocity_delta)
    );
    let _ = writeln!(
        text,
        "seta cg_rGhostDistanceDelta \"{}\"",
        u8::from(presentation.race_ghost_distance_delta)
    );
    let _ = writeln!(
        text,
        "seta cg_rGhostDemoBaseUrl \"{}\"",
        presentation.race_ghost_demo_base_url
    );
    let _ = writeln!(
        text,
        "seta con_timestamps \"{}\"",
        u8::from(presentation.console_timestamps)
    );
    let _ = writeln!(
        text,
        "seta con_suggest \"{}\"",
        u8::from(presentation.console_suggest)
    );
    let _ = writeln!(
        text,
        "seta cg_chatboxCompletion \"{}\"",
        u8::from(presentation.chatbox_completion)
    );
    let _ = writeln!(
        text,
        "seta cl_chatLog \"{}\"",
        u8::from(presentation.chat_log)
    );
    let _ = writeln!(text, "seta ui_vgs \"{}\"", presentation.ui_vgs);
    let _ = writeln!(
        text,
        "seta r_jumpHeightShade \"{}\"",
        u8::from(presentation.jump_height_shade)
    );
    let _ = writeln!(
        text,
        "seta cg_screenShake \"{}\"",
        presentation.screen_shake
    );
    let _ = writeln!(text, "seta cg_zoomFov \"{:.3}\"", presentation.zoom_fov);
    let _ = writeln!(text, "seta cg_fkDuration \"{}\"", presentation.fk_duration);
    let _ = writeln!(
        text,
        "seta cg_fkFirstJumpDuration \"{}\"",
        presentation.fk_first_jump_duration
    );
    let _ = writeln!(
        text,
        "seta cg_fkSecondJumpDelay \"{}\"",
        presentation.fk_second_jump_delay
    );
    let _ = writeln!(text, "seta cg_fov \"{:.3}\"", presentation.fov);
    for (index, master) in presentation.master_servers.iter().enumerate() {
        let _ = writeln!(text, "seta sv_master{} \"{}\"", index + 1, master);
    }
    presentation.network.write_cfg(&mut text);
    presentation.japro.write_cfg(&mut text);
    let _ = writeln!(
        text,
        "seta sensitivity \"{:.6}\"",
        presentation.mouse.sensitivity
    );
    let _ = writeln!(text, "seta m_yaw \"{:.6}\"", presentation.mouse.yaw);
    let _ = writeln!(text, "seta m_pitch \"{:.6}\"", presentation.mouse.pitch);
    let _ = writeln!(
        text,
        "seta cl_mouseAccel \"{:.6}\"",
        presentation.mouse.accel
    );
    let _ = writeln!(
        text,
        "seta cg_thirdPerson \"{}\"",
        u8::from(presentation.third_person.enabled)
    );
    let _ = writeln!(
        text,
        "seta cg_specCamera \"{}\"",
        presentation.spectator_camera.mode.as_i32()
    );
    let _ = writeln!(
        text,
        "seta cg_specCameraMotion \"{}\"",
        u8::from(presentation.spectator_camera.motion_direction)
    );
    let _ = writeln!(
        text,
        "seta cg_specOrbitRange \"{:.3}\"",
        presentation.spectator_camera.orbit_range
    );
    let _ = writeln!(
        text,
        "seta cg_fpls \"{}\"",
        u8::from(presentation.first_person_lightsaber)
    );
    let _ = writeln!(text, "seta cg_saberTrail \"{}\"", presentation.saber_trail);
    let _ = writeln!(
        text,
        "seta cg_saberTeamColors \"{}\"",
        u8::from(presentation.saber_team_colors)
    );
    let _ = writeln!(
        text,
        "seta cg_saberStaffMultiColor \"{}\"",
        u8::from(presentation.saber_staff_multi_color)
    );
    let _ = writeln!(
        text,
        "seta cg_drawTeamOverlay \"{}\"",
        presentation.team_overlay.mode
    );
    let _ = writeln!(
        text,
        "seta cg_drawTeamOverlayX \"{}\"",
        presentation.team_overlay.x
    );
    let _ = writeln!(
        text,
        "seta cg_drawTeamOverlayY \"{}\"",
        presentation.team_overlay.y
    );
    let _ = writeln!(
        text,
        "seta cg_drawTeamOverlayWeapons \"{}\"",
        u8::from(presentation.team_overlay.weapons)
    );
    let _ = writeln!(
        text,
        "seta cg_drawTeamOverlayScale \"{:.3}\"",
        presentation.team_overlay.scale
    );
    let _ = writeln!(
        text,
        "seta cg_drawTeamOverlayMaxHP \"{:.3}\"",
        presentation.team_overlay.max_hp
    );
    let _ = writeln!(
        text,
        "seta cg_drawTeamOverlayForce \"{}\"",
        u8::from(presentation.team_overlay.force)
    );
    let _ = writeln!(
        text,
        "seta cg_scoreDeaths \"{}\"",
        presentation.score_deaths
    );
    let _ = writeln!(text, "seta cg_drawScores \"{}\"", presentation.draw_scores);
    let _ = writeln!(
        text,
        "seta cg_thirdPersonAlpha \"{:.3}\"",
        presentation.third_person.alpha
    );
    let _ = writeln!(
        text,
        "seta cg_thirdPersonAngle \"{:.3}\"",
        presentation.third_person.angle
    );
    let _ = writeln!(
        text,
        "seta cg_thirdPersonCameraDamp \"{:.3}\"",
        presentation.third_person.camera_damp
    );
    let _ = writeln!(
        text,
        "seta cg_thirdPersonHorzOffset \"{:.3}\"",
        presentation.third_person.horz_offset
    );
    let _ = writeln!(
        text,
        "seta cg_thirdPersonPitchOffset \"{:.3}\"",
        presentation.third_person.pitch_offset
    );
    let _ = writeln!(
        text,
        "seta cg_thirdPersonRange \"{:.3}\"",
        presentation.third_person.range
    );
    let _ = writeln!(
        text,
        "seta cg_thirdPersonTargetDamp \"{:.3}\"",
        presentation.third_person.target_damp
    );
    let _ = writeln!(
        text,
        "seta cg_thirdPersonVertOffset \"{:.3}\"",
        presentation.third_person.vert_offset
    );
    let _ = writeln!(text, "seta s_volume \"{:.3}\"", audio.effects_volume);
    let _ = writeln!(text, "seta s_volumeVoice \"{:.3}\"", audio.voice_volume);
    let _ = writeln!(text, "seta s_musicvolume \"{:.3}\"", audio.music_volume);
    let _ = writeln!(text, "seta s_separation \"{:.3}\"", audio.separation);
    let _ = writeln!(
        text,
        "seta s_muteWhenUnfocused \"{}\"",
        u8::from(audio.mute_when_unfocused)
    );
    let _ = writeln!(
        text,
        "seta s_steamAudio \"{}\"",
        u8::from(audio.steam_audio)
    );
    let _ = writeln!(
        text,
        "seta s_steamAudioBinaural \"{}\"",
        u8::from(audio.steam_audio_binaural)
    );
    let _ = writeln!(
        text,
        "seta s_steamAudioEnvironmental \"{}\"",
        u8::from(audio.steam_audio_environmental)
    );
    let _ = writeln!(text, "seta cg_jumpSounds \"{}\"", audio.game.jump);
    let _ = writeln!(text, "seta cg_rollSounds \"{}\"", audio.game.roll);
    let _ = writeln!(
        text,
        "seta cg_noTaunt \"{}\"",
        u8::from(audio.game.no_taunt)
    );
    let _ = writeln!(text, "seta cg_duelSounds \"{}\"", audio.game.duel);
    let _ = writeln!(text, "seta cg_killSounds \"{}\"", audio.game.kill);
    let _ = writeln!(text, "seta cg_killMessage \"{}\"", audio.game.kill_message);
    let _ = writeln!(text, "seta cg_drawRewards \"{}\"", audio.game.draw_rewards);
    let _ = writeln!(text, "seta cg_hitsounds \"{}\"", audio.game.hit);
    let _ = writeln!(
        text,
        "seta cg_duelMusic \"{}\"",
        u8::from(audio.game.duel_music)
    );
    let _ = writeln!(
        text,
        "seta cg_ambientSounds \"{}\"",
        u8::from(audio.game.ambient)
    );
    let _ = writeln!(text, "seta cg_blood \"{}\"", audio.game.blood);
    let _ = writeln!(text, "seta cg_autoSwitch \"{}\"", audio.game.auto_switch);
    let _ = writeln!(
        text,
        "seta cg_scorePlums \"{}\"",
        u8::from(audio.game.score_plums)
    );
    let _ = writeln!(text, "seta cg_ghoul2Marks \"{}\"", audio.game.g2_marks);
    let _ = writeln!(text, "seta cg_raceSounds \"{}\"", audio.game.race_sounds);
    let _ = writeln!(text, "seta cg_chatSounds \"{}\"", audio.game.chat_sounds);
    let _ = writeln!(text, "seta cg_footsteps \"{}\"", audio.game.footsteps);
    bindings.write_cfg(&mut text);
    let a = settings.ocean_settings.authored;
    let o = settings.ocean_settings.optics;
    let _ = writeln!(
        text,
        "seta r_oceanFogColor \"{} {} {}\"",
        o.fog_color[0], o.fog_color[1], o.fog_color[2]
    );
    for (name, value) in [
        ("FogDistance", o.fog_distance),
        ("Transparency", o.transparency),
        ("DepthDarkening", o.depth_darkening),
        ("Refraction", o.refraction),
        ("Caustics", o.caustics),
        ("UnderwaterCull", o.underwater_cull),
    ] {
        let _ = writeln!(text, "seta r_ocean{name} \"{value}\"");
    }
    let _ = writeln!(
        text,
        "seta r_oceanAuthoring \"{} {} {} {} {} {} {} {} {} {} {}\"",
        a.amplitude,
        a.wavelength,
        a.speed,
        a.direction,
        a.steepness,
        a.slosh,
        a.wind_chop,
        a.foam,
        a.foam_lifetime,
        a.spray,
        a.seed
    );
    fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn normalize_fps_cap(value: &str, fallback: u32) -> u32 {
    match value.trim().parse::<u32>() {
        Ok(0) => 0,
        Ok(cap) => cap.clamp(FPS_CAP_MIN, FPS_CAP_MAX),
        Err(_) => fallback,
    }
}

pub fn normalize_physics_msec(value: &str, fallback: u32) -> u32 {
    value
        .trim()
        .parse::<u32>()
        .ok()
        .map(|msec| msec.clamp(PHYSICS_MSEC_MIN, PHYSICS_MSEC_MAX))
        .unwrap_or_else(|| fallback.clamp(PHYSICS_MSEC_MIN, PHYSICS_MSEC_MAX))
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

pub fn physics_msec_from_fps(value: &str, fallback_msec: u32) -> u32 {
    let fallback = fallback_msec.clamp(PHYSICS_MSEC_MIN, PHYSICS_MSEC_MAX);
    let Ok(fps) = value.trim().parse::<u32>() else {
        return fallback;
    };
    if fps == 0 {
        return fallback;
    }
    let fps = u64::from(fps);
    (PHYSICS_MSEC_MIN..=PHYSICS_MSEC_MAX)
        .min_by(|a, b| {
            let a_error = 1000_u64.abs_diff(fps * u64::from(*a));
            let b_error = 1000_u64.abs_diff(fps * u64::from(*b));
            (a_error * u64::from(*b))
                .cmp(&(b_error * u64::from(*a)))
                .then_with(|| a.cmp(b))
        })
        .unwrap_or(fallback)
}

pub fn physics_fps_from_msec(msec: u32) -> u32 {
    1000 / msec.clamp(PHYSICS_MSEC_MIN, PHYSICS_MSEC_MAX)
}

fn parse_rgba8(value: &str) -> Option<[u8; 4]> {
    let mut values = value.split_whitespace();
    let mut out = [0_u8; 4];
    for component in &mut out {
        let parsed = values.next()?.parse::<i32>().ok()?;
        *component = parsed.clamp(0, 255) as u8;
    }
    values.next().is_none().then_some(out)
}

fn parse_vec3(value: &str) -> Option<[f32; 3]> {
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

fn parse_ocean_cascade(
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

fn parse_bool(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Some(true),
        "0" | "false" | "off" | "no" => Some(false),
        _ => None,
    }
}

fn split_cfg_words(line: &str) -> Vec<String> {
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
    fn gamma_methods_round_trip_and_migrate_legacy_configs() {
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
            let loaded = load_video_settings(&path, None);
            assert_eq!(loaded.gamma_method, method);
            assert_eq!(loaded.gamma, 2.25);
        }
        for (text, expected) in [
            ("seta r_gamma \"1.5\"\n", GammaMethod::Shader),
            ("seta r_bakedBrightness \"invalid\"\n", GammaMethod::Shader),
            ("seta r_gammaMethod \"invalid\"\n", GammaMethod::Shader),
            ("seta r_bakedBrightness \"1\"\n", GammaMethod::Baked),
            ("seta r_gammaMethod \"2\"\n", GammaMethod::Hardware),
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
                load_video_settings(&path, None).gamma_method,
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
        save_video_settings(
            &path,
            settings,
            &crate::keybinds::Bindings::default(),
            &ClientPresentationSettings::default(),
            &AudioSettings::default(),
        )
        .expect("save");
        let loaded = load_video_settings(&path, None);
        assert_eq!(loaded.split_toning, settings.split_toning);
        assert_eq!(loaded.color_lut, settings.color_lut);
        assert_eq!(loaded.color_lut_strength, settings.color_lut_strength);
        std::fs::write(
            &path,
            "seta r_splitToningStrength \"NaN\"\nseta r_splitToningShadowHue \"inf\"\n",
        )
        .unwrap();
        let loaded = load_video_settings(&path, None);
        assert_eq!(loaded.split_toning, VideoSettings::default().split_toning);
        std::fs::remove_file(&path).unwrap();
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
        let loaded = load_video_settings(&path, None);
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
        settings.weather_wind = crate::ocean::OceanWind {
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
        let loaded = load_video_settings(&path, None);
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
        let loaded = load_video_settings(&path, None);
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
        let loaded = load_client_presentation_settings(&path, None);
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
            voice_volume: 0.625,
            music_volume: 0.125,
            separation: 0.75,
            mute_when_unfocused: false,
            steam_audio: true,
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
        let loaded = load_audio_settings(&path, None);
        let text = std::fs::read_to_string(&path).expect("read");
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(loaded, audio);
        assert!(text.contains("seta s_volume \"0.375\""));
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
        let loaded = load_video_settings(&path, None);
        let text = std::fs::read_to_string(&path).expect("read");
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(loaded.ghoul2_skinning, Ghoul2SkinningMode::CpuWorkers);
        assert!(!loaded.ghoul2_early_cull);
        assert_eq!(loaded.ghoul2_lod_bias, 2);
        assert_eq!(loaded.ghoul2_batch_draws, Ghoul2BatchMode::Force);
        assert_eq!(loaded.ghoul2_anim_smooth, 0.3);
        assert!(text.contains("seta r_ghoul2Skinning \"workers\""));
        assert!(text.contains("seta r_ghoul2EarlyCull \"0\""));
        assert!(text.contains("seta r_lodbias \"2\""));
        assert!(text.contains("seta r_ghoul2BatchDraws \"2\""));
        assert!(text.contains("seta r_ghoul2AnimSmooth \"0.3\""));
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
            settings.rt_half_resolution = samples != 1;
            save_video_settings(
                &path,
                settings,
                &crate::keybinds::Bindings::default(),
                &ClientPresentationSettings::default(),
                &AudioSettings::default(),
            )
            .unwrap();
            assert_eq!(load_video_settings(&path, None).rt_samples, samples);
            assert_eq!(
                load_video_settings(&path, None).rt_half_resolution,
                samples != 1
            );
        }
        for value in ["0", "3", "999", "-1", "NaN"] {
            std::fs::write(&path, format!("seta r_rtSamples \"{value}\"\n")).unwrap();
            assert_eq!(load_video_settings(&path, None).rt_samples, 1);
        }
        for (value, half) in [
            ("full", false),
            ("half", true),
            ("1", false),
            ("0.5", true),
            ("0.25", false),
            ("invalid", false),
        ] {
            std::fs::write(&path, format!("seta r_rtResolution \"{value}\"\n")).unwrap();
            assert_eq!(load_video_settings(&path, None).rt_half_resolution, half);
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
        settings.weather_wind = crate::ocean::OceanWind {
            speed: 720.0,
            direction: 135.0,
            gust: 0.6,
            shift: 28.0,
        };
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
        let loaded = load_video_settings(&path, None);
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(loaded.cloud_temporal_depth_fix, false);
        assert!((loaded.cloud_shear - 0.75).abs() < 1e-3);
        assert!((loaded.cloud_base_variation - 0.125).abs() < 1e-3);
        assert_eq!(loaded.weather_wind, settings.weather_wind);
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
        let loaded = load_video_settings(&path, None);
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
        let loaded = load_video_settings(&path, None);
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
                "seta cg_thirdPerson \"1\"\n",
                "seta cg_specCamera \"2\"\n",
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

        let loaded = load_client_presentation_settings(&path, None);
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
        assert!(text.contains("seta cg_specCamera \"1\""));
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
        assert!(text.contains("seta cg_hudHealth \"tl 48.000 64.000 1.250\""));
        assert!(text.contains("seta cg_hudFps \"tl -20.000 10.000 0.750\""));
        assert!(text.contains("seta cg_hudChat \"tl 0.000 0.000 1.000\""));
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
    fn physics_fps_snaps_to_integer_millisecond_ticks() {
        assert_eq!(physics_msec_from_fps("125", 8), 8);
        assert_eq!(physics_msec_from_fps("144", 8), 7);
        assert_eq!(physics_fps_from_msec(7), 142);
        assert_eq!(physics_msec_from_fps("120", 8), 8);
        assert_eq!(physics_fps_from_msec(8), 125);
        assert_eq!(physics_msec_from_fps("154", 8), 7);
        assert_eq!(physics_fps_from_msec(7), 142);
        assert_eq!(physics_msec_from_fps("99999", 8), 1);
        assert_eq!(physics_msec_from_fps("1", 8), 33);
    }
}
