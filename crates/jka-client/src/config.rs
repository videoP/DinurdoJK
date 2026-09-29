use crate::camera::{ThirdPersonSettings, DEFAULT_CG_FOV, MAX_CG_FOV, MIN_CG_FOV};
use crate::player::{LocalPresentationSettings, MouseInputSettings};
use crate::fx::{FX_FPS_LEGACY_JKA, FX_FPS_MAX, FX_FPS_MIN};
use crate::ui::{
    CloudRenderResolution, CloudType, ColorLutPreset, DetailTextureMode, DofQuality, DynamicLightsMode,
    DynamicShadowsMode, EntityAmbientLightingMode, FogMode, FootprintMode, FullscreenMode,
    CrosshairSettings, FxGeometryMode, Ghoul2BatchMode, Ghoul2SkinningMode, HudElementId, HudElementLayout, HudLayout,
    MovementKeysSettings, StrafeHelperSettings,
    PvsMode, RainIntensity, ReflectionQuality, RendererBackend, SaberMarkMode, SunVisibilityMode, TextureFilter,
    VideoSettings, VsyncMode, CLOUD_HEIGHT_MAX, CLOUD_HEIGHT_MIN, CLOUD_THICKNESS_MAX,
    CLOUD_THICKNESS_MIN, MAX_DISTANCE_CULL_SCALE, MAX_FOG_STRENGTH,
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
    pub first_person_lightsaber: bool,
    /// OpenJK cg_saberTrail: 0 disables saber swing trails, 1 is normal,
    /// 2 requests the legacy special/high-frequency mode.
    pub saber_trail: i32,
    pub smoothing: LocalPresentationSettings,
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
    pub hud_layout: HudLayout,
    pub movement_keys: MovementKeysSettings,
    pub strafe_helper: StrafeHelperSettings,
    pub console_timestamps: bool,
    /// con_suggest: live command/cvar filter popup while typing in the console.
    pub console_suggest: bool,
    /// TaystJK ui_vgs: use the jaPRO VGS menu in place of stock team voice chat.
    pub ui_vgs: i32,
    /// r_jumpHeightShade: tint landing surfaces by jump height in jaPRO SP physics.
    pub jump_height_shade: bool,
    /// Userinfo / network cvars (name, rate, snaps, cl_maxpackets, ...).
    pub network: crate::net::NetworkSettings,
    /// TaystJK-compatible server-browser masters (`sv_master1`..`sv_master5`).
    pub master_servers: [String; crate::server_browser::MAX_MASTER_SLOTS],
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
        }
    }
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
        let finite = || value.parse::<f32>().ok().filter(|number| number.is_finite());
        match name.to_ascii_lowercase().as_str() {
            "s_volume" => {
                settings.effects_volume = finite().unwrap_or(settings.effects_volume).clamp(0.0, 1.0)
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
                settings.steam_audio_binaural = parse_bool(value).unwrap_or(settings.steam_audio_binaural)
            }
            "s_steamaudioenvironmental" => {
                settings.steam_audio_environmental = parse_bool(value).unwrap_or(settings.steam_audio_environmental)
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
            first_person_lightsaber: true,
            saber_trail: 1,
            smoothing: LocalPresentationSettings::default(),
            mouse: MouseInputSettings::default(),
            fov: DEFAULT_CG_FOV,
            zoom_fov: 30.0,
            fk_duration: 50,
            fk_first_jump_duration: 0,
            fk_second_jump_delay: 0,
            model: "kyle".to_owned(),
            force_model: "0".to_owned(),
            crosshair: CrosshairSettings::default(),
            hud_layout: HudLayout::default(),
            movement_keys: MovementKeysSettings::default(),
            strafe_helper: StrafeHelperSettings::default(),
            console_timestamps: true,
            console_suggest: true,
            ui_vgs: 1,
            jump_height_shade: false,
            network: crate::net::NetworkSettings::default(),
            master_servers: crate::server_browser::DEFAULT_MASTER_CVARS.map(str::to_owned),
        }
    }
}

/// Load local/POV presentation cvars. Stock OpenJK/TaystJK camera and mouse
/// controls live alongside DinurdoJK's presentation-only smoothing switches.
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
        let finite = || value.parse::<f32>().ok().filter(|number| number.is_finite());
        match name.to_ascii_lowercase().as_str() {
            "model" if !value.trim().is_empty() => settings.model = value.trim().to_owned(),
            "cg_forcemodel" => settings.force_model = value.trim().to_owned(),
            "cg_drawcrosshair" => {
                if let Ok(style) = value.trim().parse::<u8>() {
                    settings.crosshair.style = style.min(6);
                }
            }
            "cg_crosshairsize" => {
                settings.crosshair.size = finite()
                    .unwrap_or(settings.crosshair.size)
                    .clamp(4.0, 96.0);
            }
            "cg_crosshaircolor" => {
                if let Some(color) = parse_rgba8(value) {
                    settings.crosshair.color = color;
                }
            }
            "cg_hudhealth" => {
                if let Some(layout) = HudElementLayout::from_config(value) {
                    settings.hud_layout.health = layout;
                }
            }
            "cg_hudshield" => {
                if let Some(layout) = HudElementLayout::from_config(value) {
                    settings.hud_layout.shield = layout;
                }
            }
            "cg_hudammo" => {
                if let Some(layout) = HudElementLayout::from_config(value) {
                    settings.hud_layout.ammo = layout;
                }
            }
            "cg_hudforce" => {
                if let Some(layout) = HudElementLayout::from_config(value) {
                    settings.hud_layout.force = layout;
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
                if let Ok(mode) = value.trim().parse::<u8>() { settings.movement_keys.mode = mode.min(4); }
            }
            "cg_movementkeysx" => settings.movement_keys.x = finite().unwrap_or(settings.movement_keys.x).clamp(-640.0, 640.0),
            "cg_movementkeysy" => settings.movement_keys.y = finite().unwrap_or(settings.movement_keys.y).clamp(-480.0, 480.0),
            "cg_movementkeyssize" => settings.movement_keys.size = finite().unwrap_or(settings.movement_keys.size).clamp(0.25, 4.0),
            "cg_movementkeyswalk" => settings.movement_keys.walk = parse_bool(value).unwrap_or(settings.movement_keys.walk),
            "cg_strafehelper" => {
                if let Ok(flags) = value.trim().parse::<u32>() { settings.strafe_helper.flags = flags; }
            }
            "cg_strafehelper_fps" => settings.strafe_helper.fps = finite().unwrap_or(settings.strafe_helper.fps).clamp(0.0, 1000.0),
            "cg_strafehelperoffset" => settings.strafe_helper.offset = finite().unwrap_or(settings.strafe_helper.offset).clamp(-1000.0, 1000.0),
            "cg_strafehelperlinewidth" => settings.strafe_helper.line_width = finite().unwrap_or(settings.strafe_helper.line_width).clamp(0.25, 5.0),
            "cg_strafehelperprecision" => settings.strafe_helper.precision = value.trim().parse::<u32>().unwrap_or(settings.strafe_helper.precision).clamp(100, 10000),
            "cg_strafehelpercutoff" => settings.strafe_helper.cutoff = finite().unwrap_or(settings.strafe_helper.cutoff).clamp(0.0, 480.0),
            "cg_strafehelperactivecolor" => {
                if let Some(color) = parse_rgba8(value) { settings.strafe_helper.active_color = color; }
            }
            "cg_strafehelperinactivealpha" => {
                if let Ok(alpha) = value.trim().parse::<i32>() { settings.strafe_helper.inactive_alpha = alpha.clamp(0, 255) as u8; }
            }
            "con_timestamps" => {
                settings.console_timestamps =
                    parse_bool(value).unwrap_or(settings.console_timestamps)
            }
            "con_suggest" => {
                settings.console_suggest =
                    parse_bool(value).unwrap_or(settings.console_suggest)
            }
            "ui_vgs" => {
                settings.ui_vgs = value.trim().parse::<i32>().unwrap_or(settings.ui_vgs)
            }
            "r_jumpheightshade" => {
                settings.jump_height_shade =
                    parse_bool(value).unwrap_or(settings.jump_height_shade)
            }
            "sv_master1" => settings.master_servers[0] = value.trim().to_owned(),
            "sv_master2" => settings.master_servers[1] = value.trim().to_owned(),
            "sv_master3" => settings.master_servers[2] = value.trim().to_owned(),
            "sv_master4" => settings.master_servers[3] = value.trim().to_owned(),
            "sv_master5" => settings.master_servers[4] = value.trim().to_owned(),
            other if settings.network.cvar_value(other).is_some() => {
                let _ = settings.network.set_cvar(other, value);
            }
            "sensitivity" => {
                settings.mouse.sensitivity = finite().unwrap_or(settings.mouse.sensitivity)
            }
            "m_yaw" => settings.mouse.yaw = finite().unwrap_or(settings.mouse.yaw),
            "m_pitch" => settings.mouse.pitch = finite().unwrap_or(settings.mouse.pitch),
            "cl_mouseaccel" => {
                settings.mouse.accel = finite().unwrap_or(settings.mouse.accel)
            }
            "cg_fov" => {
                settings.fov = finite()
                    .unwrap_or(settings.fov)
                    .clamp(MIN_CG_FOV, MAX_CG_FOV)
            }
            "cg_zoomfov" => {
                settings.zoom_fov = finite().unwrap_or(settings.zoom_fov)
            }
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
            "cg_smoothplayerorigin" => {
                settings.smoothing.smooth_player_origin =
                    parse_bool(value).unwrap_or(settings.smoothing.smooth_player_origin)
            }
            "cg_smooththirdpersonorigin" => {
                settings.smoothing.smooth_third_person_origin =
                    parse_bool(value).unwrap_or(settings.smoothing.smooth_third_person_origin)
            }
            "cg_smoothplayeranimation" => {
                settings.smoothing.smooth_player_animation =
                    parse_bool(value).unwrap_or(settings.smoothing.smooth_player_animation)
            }
            "cg_subframeplayerangles" => {
                settings.smoothing.subframe_player_angles =
                    parse_bool(value).unwrap_or(settings.smoothing.subframe_player_angles)
            }
            "cg_smooththirdpersontime" => {
                settings.smoothing.smooth_third_person_time =
                    parse_bool(value).unwrap_or(settings.smoothing.smooth_third_person_time)
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
                settings.detail_texture_fade = parse_bool(value).unwrap_or(settings.detail_texture_fade);
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
                    settings.wireframe_mask = if enabled { crate::ui::wireframe::MAP } else { 0 };
                }
            }
            "r_skipui" => {
                settings.skip_ui = parse_bool(value).unwrap_or(settings.skip_ui);
            }
            "developer" => {
                settings.developer_tools = parse_bool(value).unwrap_or(settings.developer_tools);
            }
            "r_perftrace" => {
                settings.perf_trace = parse_bool(value).unwrap_or(settings.perf_trace);
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
            "com_maxfps" => {
                settings.fps_cap = normalize_fps_cap(value, settings.fps_cap);
            }
            "cg_fxfps" => {
                settings.fx_fps = normalize_fx_fps(value, settings.fx_fps);
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
            "pmove_msec" => {
                settings.physics_msec = normalize_physics_msec(value, settings.physics_msec);
            }
            "cl_input_subframe" => {
                settings.input_subframe = parse_bool(value).unwrap_or(settings.input_subframe);
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
                settings.client_physics_ccd = parse_bool(value).unwrap_or(settings.client_physics_ccd);
            }
            "r_physicssleeping" => {
                settings.client_physics_sleeping = parse_bool(value).unwrap_or(settings.client_physics_sleeping);
            }
            "r_ragdolls" => settings.ragdolls = parse_bool(value).unwrap_or(settings.ragdolls),
            "r_ragdollmax" => {
                if let Ok(requested) = value.parse::<u32>() {
                    const VALUES: [u32; 5] = [2, 4, 8, 16, 32];
                    settings.ragdoll_max = *VALUES.iter().min_by_key(|&&v| v.abs_diff(requested)).unwrap_or(&8);
                }
            }
            "r_ragdolllifetime" => {
                if let Ok(requested) = value.parse::<u32>() {
                    const VALUES: [u32; 5] = [5, 10, 20, 30, 60];
                    settings.ragdoll_lifetime = *VALUES.iter().min_by_key(|&&v| v.abs_diff(requested)).unwrap_or(&20) as f32;
                }
            }
            "r_ragdollselfcollision" => {
                settings.ragdoll_self_collision = parse_bool(value).unwrap_or(settings.ragdoll_self_collision);
            }
            "r_physicsprops" => settings.physics_props = parse_bool(value).unwrap_or(settings.physics_props),
            "r_physicspropmax" => {
                if let Ok(requested) = value.parse::<u32>() {
                    const VALUES: [u32; 5] = [32, 64, 96, 192, 384];
                    settings.physics_prop_max = *VALUES.iter().min_by_key(|&&v| v.abs_diff(requested)).unwrap_or(&96);
                }
            }
            "r_physicsdebris" => settings.physics_debris = parse_bool(value).unwrap_or(settings.physics_debris),
            "r_physicsdebrismax" => {
                if let Ok(requested) = value.parse::<u32>() {
                    const VALUES: [u32; 5] = [64, 128, 192, 384, 768];
                    settings.physics_debris_max = *VALUES.iter().min_by_key(|&&v| v.abs_diff(requested)).unwrap_or(&192);
                }
            }
            "r_physicsdebrislifetime" => {
                if let Ok(requested) = value.parse::<u32>() {
                    const VALUES: [u32; 5] = [2, 5, 10, 20, 30];
                    settings.physics_debris_lifetime = *VALUES.iter().min_by_key(|&&v| v.abs_diff(requested)).unwrap_or(&10) as f32;
                }
            }
            "r_physicsplayerpush" => settings.physics_player_push = parse_bool(value).unwrap_or(settings.physics_player_push),
            "r_physicsweaponimpulses" => settings.physics_weapon_impulses = parse_bool(value).unwrap_or(settings.physics_weapon_impulses),
            "r_physicsexplosionimpulses" => settings.physics_explosion_impulses = parse_bool(value).unwrap_or(settings.physics_explosion_impulses),
            "r_physicsforceimpulses" => settings.physics_force_impulses = parse_bool(value).unwrap_or(settings.physics_force_impulses),
            "r_physicsdebug" => settings.physics_debug_draw = parse_bool(value).unwrap_or(settings.physics_debug_draw),
            "r_physicsstats" => settings.physics_stats = parse_bool(value).unwrap_or(settings.physics_stats),
            "r_novis" => novis = parse_bool(value),
            "r_pvsmode" => {
                pvs_mode = match value.to_ascii_lowercase().as_str() {
                    "off" | "0" => Some(PvsMode::Off),
                    "minimal" | "1" => Some(PvsMode::Minimal),
                    "full" | "2" => Some(PvsMode::Full),
                    "auto" | "3" => Some(PvsMode::Auto),
                    "auto2" | "4" => Some(PvsMode::Auto2),
                    "auto3" | "5" => Some(PvsMode::Auto3),
                    "auto4" | "batched" | "pvsbatched" | "6" => Some(PvsMode::Auto4),
                    _ => pvs_mode,
                };
            }
            "r_hdr" => settings.hdr = parse_bool(value).unwrap_or(settings.hdr),
            "r_floatlightmap" => settings.float_lightmap = parse_bool(value).unwrap_or(settings.float_lightmap),
            "r_autoexposure" => settings.auto_exposure = parse_bool(value).unwrap_or(settings.auto_exposure),
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
                    settings.static_bsp_ao_strength = *VALUES.iter().min_by_key(|&&candidate| candidate.abs_diff(value)).unwrap_or(&75);
                }
            }
            "r_staticbspaorange" => {
                if let Ok(value) = value.parse::<u32>() {
                    const VALUES: [u32; 4] = [50, 100, 150, 200];
                    settings.static_bsp_ao_range = *VALUES.iter().min_by_key(|&&candidate| candidate.abs_diff(value)).unwrap_or(&100);
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
            "r_drawfog" => {
                settings.fog_mode = match value {
                    "0" => FogMode::Off,
                    "1" => FogMode::LegacyDrawFog1,
                    "2" => FogMode::LegacyDrawFog2,
                    _ => settings.fog_mode,
                }
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
                settings.sun_visibility = SunVisibilityMode::from_config(value)
                    .unwrap_or(settings.sun_visibility);
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
                settings.cloud_shape_evolution = parse_bool(value).unwrap_or(settings.cloud_shape_evolution)
            }
            "r_cloudterraininteraction" => {
                settings.cloud_terrain_interaction = parse_bool(value).unwrap_or(settings.cloud_terrain_interaction)
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
                            amplitude:f[0], wavelength:f[1], speed:f[2], direction:f[3], steepness:f[4], slosh:f[5],
                            wind_chop:f[6], foam:f[7], foam_lifetime:f[8], spray:f[9], seed,
                        }.sanitize();
                    }
                }
            }
            "r_weatherwind" => {
                let v: Option<Vec<f32>> = value.split_whitespace().map(|s| s.parse().ok()).collect();
                if let Some(v) = v.filter(|v| v.len() == 4) {
                    settings.weather_wind = crate::ocean::OceanWind {
                        speed: v[0], direction: v[1], gust: v[2], shift: v[3],
                    }.sanitize();
                    weather_wind_explicit = true;
                }
            }
            // Legacy ocean weather cvar. Preserve the old cloud speed/direction
            // when present, but migrate ocean gust/shift into the shared wind.
            "r_oceanweather" => {
                if !weather_wind_explicit {
                    let v: Option<Vec<f32>> = value.split_whitespace().map(|s| s.parse().ok()).collect();
                    if let Some(v) = v.filter(|v| v.len() == 4) {
                        let old = settings.weather_wind;
                        let mut wind = crate::ocean::OceanWind { speed:v[0],direction:v[1],gust:v[2],shift:v[3] }.sanitize();
                        if legacy_cloud_wind_speed_seen { wind.speed = old.speed; }
                        if legacy_cloud_wind_direction_seen { wind.direction = old.direction; }
                        settings.weather_wind = wind;
                    }
                }
            }
            "r_oceanmapsize" => {
                if let Ok(v)=value.parse::<u32>() { settings.ocean_settings.map_size=v; }
            }
            "r_oceanmeshquality" => {
                if let Ok(v)=value.parse::<u8>() { settings.ocean_settings.mesh_quality=v; }
            }
            "r_oceanupdates" => {
                if let Ok(v)=value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.updates_per_second=v; } }
            }
            "r_oceanroughness" => {
                if let Ok(v)=value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.roughness=v; } }
            }
            "r_oceanfogcolor" => settings.ocean_settings.optics.fog_color = parse_vec3(value).unwrap_or(settings.ocean_settings.optics.fog_color),
            "r_oceanfogdistance" | "r_oceantransparency" | "r_oceandepthdarkening" |
            "r_oceanrefraction" | "r_oceancaustics" | "r_oceanunderwatercull" => {
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
                if let Ok(v)=value.parse::<f32>() { if v.is_finite() { settings.ocean_settings.normal_strength=v; } }
            }
            "r_oceanseaspray" => {
                settings.ocean_settings.sea_spray = parse_bool(value).unwrap_or(settings.ocean_settings.sea_spray);
            }
            "r_oceanwindfoam" => {
                settings.ocean_settings.wind_foam_streaks = parse_bool(value).unwrap_or(settings.ocean_settings.wind_foam_streaks);
            }
            "r_oceanwatercolor" => {
                settings.ocean_settings.water_color = parse_vec3(value).unwrap_or(settings.ocean_settings.water_color);
            }
            "r_oceanfoamcolor" => {
                settings.ocean_settings.foam_color = parse_vec3(value).unwrap_or(settings.ocean_settings.foam_color);
            }
            "r_oceancascade1" => settings.ocean_settings.cascades[0] = parse_ocean_cascade(value, settings.ocean_settings.cascades[0]),
            "r_oceancascade2" => settings.ocean_settings.cascades[1] = parse_ocean_cascade(value, settings.ocean_settings.cascades[1]),
            "r_oceancascade3" => settings.ocean_settings.cascades[2] = parse_ocean_cascade(value, settings.ocean_settings.cascades[2]),
            "r_rainintensity" => {
                settings.rain_intensity =
                    RainIntensity::from_config(value).unwrap_or(settings.rain_intensity)
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
                settings.reflection_quality = ReflectionQuality::from_config(value)
                    .unwrap_or(settings.reflection_quality)
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
            "r_dofquality" => {
                settings.dof_quality =
                    DofQuality::from_config(value).unwrap_or(settings.dof_quality)
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
            "r_rtresolution" => {
                match value.to_ascii_lowercase().as_str() {
                    "full" | "1" => settings.rt_half_resolution = false,
                    "half" | "0.5" => settings.rt_half_resolution = true,
                    _ => {},
                }
            }
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
                settings.saber_marks = SaberMarkMode::from_config(value).unwrap_or(settings.saber_marks)
            }
            "r_pbr" => settings.pbr = parse_bool(value).unwrap_or(settings.pbr),
            "fs_allowassetoverrides" => {
                settings.allow_asset_overrides =
                    parse_bool(value).unwrap_or(settings.allow_asset_overrides)
            }
            "r_gennormalmaps" => settings.gen_normal_maps = parse_bool(value).unwrap_or(settings.gen_normal_maps),
            "r_deluxemapping" => settings.deluxe_mapping = parse_bool(value).unwrap_or(settings.deluxe_mapping),
            "r_deluxespecular" => {
                settings.deluxe_specular = value.parse::<f32>().ok().filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 1.0)).unwrap_or(settings.deluxe_specular);
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
            "r_locallightshadows" => {
                settings.local_light_shadows =
                    parse_bool(value).unwrap_or(settings.local_light_shadows)
            }
            "r_cascadedshadows" => {
                settings.cascaded_shadows = parse_bool(value).unwrap_or(settings.cascaded_shadows)
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
        settings.cascaded_shadows = matches!(
            settings.dynamic_shadows,
            DynamicShadowsMode::CascadedShadowMaps | DynamicShadowsMode::CascadedShadowMapsBevy
        );
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
    if !settings.input_subframe {
        settings.input_latelatch = false;
    }
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
seta r_ext_texture_filter_anisotropic \"{}\"\n\
seta r_detailTextures \"{}\"\n\
seta r_detailTextureFade \"{}\"\n\
seta r_detailTextureFadeDistance \"{:.0}\"\n\
seta r_showtris \"{}\"\n\
seta r_skipUi \"{}\"\n\
seta developer \"{}\"\n\
seta r_perfTrace \"{}\"\n\
seta r_gpuTimings \"{}\"\n\
seta r_ghoul2Skinning \"{}\"\n\
seta r_ghoul2EarlyCull \"{}\"\n\
seta r_lodbias \"{}\"\n\
seta r_ghoul2BatchDraws \"{}\"\n\
seta r_novis \"{}\"\n\
seta r_pvsMode \"{}\"\n\
seta com_maxfps \"{}\"\n\
seta cg_drawFPS \"{}\"\n\
seta pmove_msec \"{}\"\n\
seta cl_input_subframe \"{}\"\n\
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
seta r_fogMode \"{}\"\n\
seta r_fogStrength \"{:.3}\"\n\
seta r_sunOverride \"{}\"\n\
seta r_sunVisibility \"{}\"\n\
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
        anisotropy,
        settings.detail_textures.config_value(),
        u8::from(settings.detail_texture_fade),
        settings.detail_texture_fade_distance,
        settings.wireframe_mask,
        u8::from(settings.skip_ui),
        u8::from(settings.developer_tools),
        u8::from(settings.perf_trace),
        u8::from(settings.gpu_timings),
        settings.ghoul2_skinning.config_value(),
        u8::from(settings.ghoul2_early_cull),
        settings.ghoul2_lod_bias,
        settings.ghoul2_batch_draws.config_value(),
        u8::from(settings.pvs_mode == PvsMode::Off),
        pvs_mode,
        settings.fps_cap,
        settings.draw_fps,
        settings.physics_msec,
        u8::from(settings.input_subframe),
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
        settings.fog_mode.config_value(),
        settings.fog_strength,
        u8::from(settings.sun_override),
        settings.sun_visibility.config_value(),
        settings.sun_yaw,
        settings.sun_pitch,
        settings.sun_intensity,
        settings.sun_color[0], settings.sun_color[1], settings.sun_color[2],
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
        u8::from(settings.grass),
        u8::from(settings.ocean),
        settings.ocean_settings.map_size,
        settings.ocean_settings.mesh_quality,
        settings.ocean_settings.updates_per_second,
        settings.ocean_settings.roughness,
        settings.ocean_settings.normal_strength,
        u8::from(settings.ocean_settings.sea_spray),
        u8::from(settings.ocean_settings.wind_foam_streaks),
        settings.ocean_settings.water_color[0], settings.ocean_settings.water_color[1], settings.ocean_settings.water_color[2],
        settings.ocean_settings.foam_color[0], settings.ocean_settings.foam_color[1], settings.ocean_settings.foam_color[2],
        settings.ocean_settings.cascades[0].tile_length[0], settings.ocean_settings.cascades[0].tile_length[1], settings.ocean_settings.cascades[0].displacement_scale, settings.ocean_settings.cascades[0].normal_scale, settings.ocean_settings.cascades[0].wind_speed, settings.ocean_settings.cascades[0].wind_direction, settings.ocean_settings.cascades[0].fetch_length, settings.ocean_settings.cascades[0].swell, settings.ocean_settings.cascades[0].spread, settings.ocean_settings.cascades[0].detail, settings.ocean_settings.cascades[0].whitecap, settings.ocean_settings.cascades[0].foam_amount,
        settings.ocean_settings.cascades[1].tile_length[0], settings.ocean_settings.cascades[1].tile_length[1], settings.ocean_settings.cascades[1].displacement_scale, settings.ocean_settings.cascades[1].normal_scale, settings.ocean_settings.cascades[1].wind_speed, settings.ocean_settings.cascades[1].wind_direction, settings.ocean_settings.cascades[1].fetch_length, settings.ocean_settings.cascades[1].swell, settings.ocean_settings.cascades[1].spread, settings.ocean_settings.cascades[1].detail, settings.ocean_settings.cascades[1].whitecap, settings.ocean_settings.cascades[1].foam_amount,
        settings.ocean_settings.cascades[2].tile_length[0], settings.ocean_settings.cascades[2].tile_length[1], settings.ocean_settings.cascades[2].displacement_scale, settings.ocean_settings.cascades[2].normal_scale, settings.ocean_settings.cascades[2].wind_speed, settings.ocean_settings.cascades[2].wind_direction, settings.ocean_settings.cascades[2].fetch_length, settings.ocean_settings.cascades[2].swell, settings.ocean_settings.cascades[2].spread, settings.ocean_settings.cascades[2].detail, settings.ocean_settings.cascades[2].whitecap, settings.ocean_settings.cascades[2].foam_amount,
        settings.footprints.config_value(),
        settings.reflection_quality.config_value(),
        settings.chromatic_aberration,
        u8::from(settings.vignette),
        settings.film_grain_strength,
        settings.motion_blur_strength,
        settings.depth_of_field_strength,
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
    );
    use std::fmt::Write as _;
    let mut text = text;
    writeln!(text, "seta r_rtSamples \"{}\"", settings.rt_samples).unwrap();
    writeln!(text, "seta r_rtResolution \"{}\"", if settings.rt_half_resolution { "half" } else { "full" }).unwrap();
    text.push_str(&format!(
        "seta r_fullbright \"{}\"\nseta r_vertexLight \"{}\"\nseta r_lightmap \"{}\"\n",
        u8::from(!settings.world_lighting),
        u8::from(settings.vertex_lighting),
        u8::from(settings.lightmap_only),
    ));
    let _ = writeln!(text, "seta cg_fxFPS \"{}\"", settings.fx_fps);
    let _ = writeln!(text, "seta r_fxGeometry \"{}\"", settings.fx_geometry.config_value());
    let _ = writeln!(text, "seta r_fxZeroAlphaDiscard \"{}\"", u8::from(settings.fx_zero_alpha_discard));
    let _ = writeln!(text, "seta r_drawMapModels \"{}\"", u8::from(settings.draw_map_models));
    let _ = writeln!(text, "seta r_modelBrightness \"{:.3}\"", settings.model_brightness);
    let _ = writeln!(text, "seta r_modelBrightnessLock \"{}\"", u8::from(settings.model_brightness_locked));
    let _ = writeln!(text, "seta r_dynamicLightBrightness \"{:.3}\"", settings.dynamic_light_brightness);
    let _ = writeln!(text, "seta r_dynamicLightBrightnessLock \"{}\"", u8::from(settings.dynamic_light_brightness_locked));
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
    let _ = writeln!(text, "seta cg_drawCrosshair \"{}\"", presentation.crosshair.style);
    let _ = writeln!(text, "seta cg_crosshairSize \"{:.3}\"", presentation.crosshair.size);
    let [r, g, b, a] = presentation.crosshair.color;
    let _ = writeln!(text, "seta cg_crosshairColor \"{r} {g} {b} {a}\"");
    for id in HudElementId::ALL {
        let _ = writeln!(
            text,
            "seta {} \"{}\"",
            id.cvar_name(),
            presentation.hud_layout.element(id).to_config()
        );
    }
    let _ = writeln!(text, "seta cg_hudSnap \"{}\"", u8::from(presentation.hud_layout.snap_to_grid));
    let _ = writeln!(text, "seta cg_hudGridSize \"{:.3}\"", presentation.hud_layout.grid_size);
    let _ = writeln!(text, "seta cg_movementKeys \"{}\"", presentation.movement_keys.mode);
    let _ = writeln!(text, "seta cg_movementKeysX \"{:.3}\"", presentation.movement_keys.x);
    let _ = writeln!(text, "seta cg_movementKeysY \"{:.3}\"", presentation.movement_keys.y);
    let _ = writeln!(text, "seta cg_movementKeysSize \"{:.3}\"", presentation.movement_keys.size);
    let _ = writeln!(text, "seta cg_movementKeysWalk \"{}\"", u8::from(presentation.movement_keys.walk));
    let _ = writeln!(text, "seta cg_strafeHelper \"{}\"", presentation.strafe_helper.flags);
    let _ = writeln!(text, "seta cg_strafeHelper_FPS \"{:.3}\"", presentation.strafe_helper.fps);
    let _ = writeln!(text, "seta cg_strafeHelperOffset \"{:.3}\"", presentation.strafe_helper.offset);
    let _ = writeln!(text, "seta cg_strafeHelperLineWidth \"{:.3}\"", presentation.strafe_helper.line_width);
    let _ = writeln!(text, "seta cg_strafeHelperPrecision \"{}\"", presentation.strafe_helper.precision);
    let _ = writeln!(text, "seta cg_strafeHelperCutoff \"{:.3}\"", presentation.strafe_helper.cutoff);
    let [sr, sg, sb, sa] = presentation.strafe_helper.active_color;
    let _ = writeln!(text, "seta cg_strafeHelperActiveColor \"{sr} {sg} {sb} {sa}\"");
    let _ = writeln!(text, "seta cg_strafeHelperInactiveAlpha \"{}\"", presentation.strafe_helper.inactive_alpha);
    let _ = writeln!(text, "seta con_timestamps \"{}\"", u8::from(presentation.console_timestamps));
    let _ = writeln!(text, "seta con_suggest \"{}\"", u8::from(presentation.console_suggest));
    let _ = writeln!(text, "seta ui_vgs \"{}\"", presentation.ui_vgs);
    let _ = writeln!(text, "seta r_jumpHeightShade \"{}\"", u8::from(presentation.jump_height_shade));
    let _ = writeln!(text, "seta cg_zoomFov \"{:.3}\"", presentation.zoom_fov);
    let _ = writeln!(text, "seta cg_fkDuration \"{}\"", presentation.fk_duration);
    let _ = writeln!(text, "seta cg_fkFirstJumpDuration \"{}\"", presentation.fk_first_jump_duration);
    let _ = writeln!(text, "seta cg_fkSecondJumpDelay \"{}\"", presentation.fk_second_jump_delay);
    let _ = writeln!(text, "seta cg_fov \"{:.3}\"", presentation.fov);
    for (index, master) in presentation.master_servers.iter().enumerate() {
        let _ = writeln!(text, "seta sv_master{} \"{}\"", index + 1, master);
    }
    presentation.network.write_cfg(&mut text);
    let _ = writeln!(text, "seta sensitivity \"{:.6}\"", presentation.mouse.sensitivity);
    let _ = writeln!(text, "seta m_yaw \"{:.6}\"", presentation.mouse.yaw);
    let _ = writeln!(text, "seta m_pitch \"{:.6}\"", presentation.mouse.pitch);
    let _ = writeln!(text, "seta cl_mouseAccel \"{:.6}\"", presentation.mouse.accel);
    let _ = writeln!(text, "seta cg_thirdPerson \"{}\"", u8::from(presentation.third_person.enabled));
    let _ = writeln!(text, "seta cg_fpls \"{}\"", u8::from(presentation.first_person_lightsaber));
    let _ = writeln!(text, "seta cg_saberTrail \"{}\"", presentation.saber_trail);
    let _ = writeln!(text, "seta cg_smoothPlayerOrigin \"{}\"", u8::from(presentation.smoothing.smooth_player_origin));
    let _ = writeln!(text, "seta cg_smoothThirdPersonOrigin \"{}\"", u8::from(presentation.smoothing.smooth_third_person_origin));
    let _ = writeln!(text, "seta cg_smoothPlayerAnimation \"{}\"", u8::from(presentation.smoothing.smooth_player_animation));
    let _ = writeln!(text, "seta cg_subframePlayerAngles \"{}\"", u8::from(presentation.smoothing.subframe_player_angles));
    let _ = writeln!(text, "seta cg_smoothThirdPersonTime \"{}\"", u8::from(presentation.smoothing.smooth_third_person_time));
    let _ = writeln!(text, "seta cg_thirdPersonAlpha \"{:.3}\"", presentation.third_person.alpha);
    let _ = writeln!(text, "seta cg_thirdPersonAngle \"{:.3}\"", presentation.third_person.angle);
    let _ = writeln!(text, "seta cg_thirdPersonCameraDamp \"{:.3}\"", presentation.third_person.camera_damp);
    let _ = writeln!(text, "seta cg_thirdPersonHorzOffset \"{:.3}\"", presentation.third_person.horz_offset);
    let _ = writeln!(text, "seta cg_thirdPersonPitchOffset \"{:.3}\"", presentation.third_person.pitch_offset);
    let _ = writeln!(text, "seta cg_thirdPersonRange \"{:.3}\"", presentation.third_person.range);
    let _ = writeln!(text, "seta cg_thirdPersonTargetDamp \"{:.3}\"", presentation.third_person.target_damp);
    let _ = writeln!(text, "seta cg_thirdPersonVertOffset \"{:.3}\"", presentation.third_person.vert_offset);
    let _ = writeln!(text, "seta s_volume \"{:.3}\"", audio.effects_volume);
    let _ = writeln!(text, "seta s_volumeVoice \"{:.3}\"", audio.voice_volume);
    let _ = writeln!(text, "seta s_musicvolume \"{:.3}\"", audio.music_volume);
    let _ = writeln!(text, "seta s_separation \"{:.3}\"", audio.separation);
    let _ = writeln!(text, "seta s_muteWhenUnfocused \"{}\"", u8::from(audio.mute_when_unfocused));
    let _ = writeln!(text, "seta s_steamAudio \"{}\"", u8::from(audio.steam_audio));
    let _ = writeln!(text, "seta s_steamAudioBinaural \"{}\"", u8::from(audio.steam_audio_binaural));
    let _ = writeln!(text, "seta s_steamAudioEnvironmental \"{}\"", u8::from(audio.steam_audio_environmental));
    bindings.write_cfg(&mut text);
    let a = settings.ocean_settings.authored;
    let o = settings.ocean_settings.optics;
    let _ = writeln!(text, "seta r_oceanFogColor \"{} {} {}\"", o.fog_color[0], o.fog_color[1], o.fog_color[2]);
    for (name, value) in [("FogDistance", o.fog_distance), ("Transparency", o.transparency),
        ("DepthDarkening", o.depth_darkening), ("Refraction", o.refraction),
        ("Caustics", o.caustics), ("UnderwaterCull", o.underwater_cull)] {
        let _ = writeln!(text, "seta r_ocean{name} \"{value}\"");
    }
    let _ = writeln!(text, "seta r_oceanAuthoring \"{} {} {} {} {} {} {} {} {} {} {}\"", a.amplitude,a.wavelength,a.speed,a.direction,a.steepness,a.slosh,a.wind_chop,a.foam,a.foam_lifetime,a.spray,a.seed);
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

pub fn normalize_fx_fps(value: &str, fallback: u32) -> u32 {
    match value.trim().parse::<u32>() {
        Ok(FX_FPS_LEGACY_JKA) => FX_FPS_LEGACY_JKA,
        Ok(value) => value.clamp(FX_FPS_MIN, FX_FPS_MAX),
        Err(_) => fallback,
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

fn parse_vec3(value:&str)->Option<[f32;3]> {
    let values=value.split_whitespace().map(str::parse::<f32>).collect::<Result<Vec<_>,_>>().ok()?;
    if values.len()!=3 || values.iter().any(|v| !v.is_finite()) { return None; }
    Some([values[0],values[1],values[2]])
}

fn parse_ocean_cascade(value:&str, current:crate::ocean::CascadeSettings)->crate::ocean::CascadeSettings {
    let values=value.split_whitespace().map(str::parse::<f32>).collect::<Result<Vec<_>,_>>();
    let Ok(v)=values else { return current; };
    if v.len()!=12 || v.iter().any(|x| !x.is_finite()) { return current; }
    crate::ocean::CascadeSettings {
        tile_length:[v[0],v[1]], displacement_scale:v[2], normal_scale:v[3], wind_speed:v[4], wind_direction:v[5],
        fetch_length:v[6], swell:v[7], spread:v[8], detail:v[9], whitecap:v[10], foam_amount:v[11],
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
    fn ocean_authoring_cfg_roundtrip_preserves_units_and_seed() {
        let path = std::env::temp_dir().join(format!("ocean-authoring-{}.cfg",std::process::id()));
        let mut settings = VideoSettings::default();
        settings.ocean_settings.authored = crate::ocean::OceanAuthoring {
            amplitude: 512.0, wavelength: 8192.0, direction: -75.0, seed: u32::MAX,
            wind_chop: 2.5, foam_lifetime: 12.0, ..Default::default()
        };
        settings.weather_wind = crate::ocean::OceanWind {
            speed: 800.0, direction: 160.0, gust: 0.5, shift: 30.0,
        };
        settings.ocean_settings.wind = settings.weather_wind;
        save_video_settings(&path, settings, &crate::keybinds::Bindings::default(),
            &ClientPresentationSettings::default(), &AudioSettings::default()).unwrap();
        let loaded = load_video_settings(&path, None);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(loaded.ocean_settings.authored, settings.ocean_settings.authored);
        assert_eq!(loaded.ocean_settings.wind, settings.ocean_settings.wind);
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
    }

    #[test]
    fn ghoul2_render_settings_survive_a_config_round_trip() {
        let mut settings = VideoSettings::default();
        settings.ghoul2_skinning = Ghoul2SkinningMode::CpuWorkers;
        settings.ghoul2_early_cull = false;
        settings.ghoul2_lod_bias = 2;
        settings.ghoul2_batch_draws = Ghoul2BatchMode::Force;

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
        assert!(text.contains("seta r_ghoul2Skinning \"workers\""));
        assert!(text.contains("seta r_ghoul2EarlyCull \"0\""));
        assert!(text.contains("seta r_lodbias \"2\""));
        assert!(text.contains("seta r_ghoul2BatchDraws \"2\""));
    }

    #[test]
    fn rt_samples_survive_config_round_trip_and_reject_invalid_values() {
        let dir = std::env::temp_dir().join(format!("jka-rt-samples-{}-{:?}", std::process::id(), std::thread::current().id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.cfg");
        for samples in [1, 2, 4] {
            let mut settings = VideoSettings::default();
            settings.rt_samples = samples;
            settings.rt_half_resolution = samples != 1;
            save_video_settings(&path, settings, &crate::keybinds::Bindings::default(),
                &ClientPresentationSettings::default(), &AudioSettings::default()).unwrap();
            assert_eq!(load_video_settings(&path, None).rt_samples, samples);
            assert_eq!(load_video_settings(&path, None).rt_half_resolution, samples != 1);
        }
        for value in ["0", "3", "999", "-1", "NaN"] {
            std::fs::write(&path, format!("seta r_rtSamples \"{value}\"\n")).unwrap();
            assert_eq!(load_video_settings(&path, None).rt_samples, 1);
        }
        for (value, half) in [("full", false), ("half", true), ("1", false), ("0.5", true), ("0.25", false), ("invalid", false)] {
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
        settings.weather_wind = crate::ocean::OceanWind { speed: 720.0, direction: 135.0, gust: 0.6, shift: 28.0 };
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
                "seta cg_fpls \"0\"\n",
                "seta r_jumpHeightShade \"1\"\n",
                "seta cg_saberTrail \"2\"\n",
                "seta cg_smoothPlayerOrigin \"0\"\n",
                "seta cg_smoothThirdPersonOrigin \"0\"\n",
                "seta cg_smoothPlayerAnimation \"0\"\n",
                "seta cg_subframePlayerAngles \"0\"\n",
                "seta cg_smoothThirdPersonTime \"0\"\n",
                "seta cg_thirdPersonAlpha \"0.375\"\n",
                "seta cg_thirdPersonAngle \"25\"\n",
                "seta cg_thirdPersonCameraDamp \"0.42\"\n",
                "seta cg_thirdPersonHorzOffset \"7\"\n",
                "seta cg_thirdPersonPitchOffset \"-4\"\n",
                "seta cg_thirdPersonRange \"128\"\n",
                "seta cg_thirdPersonSpecialCam \"1\"\n",
                "seta cg_thirdPersonTargetDamp \"0.61\"\n",
                "seta cg_thirdPersonVertOffset \"21\"\n",
                "seta cg_drawCrosshair \"5\"\n",
                "seta cg_crosshairSize \"36\"\n",
                "seta cg_crosshairColor \"64 128 255 200\"\n",
                "seta cg_hudHealth \"bl 40 -72 1.25\"\n",
                "seta cg_hudShield \"bc -120 -40 0.75\"\n",
                "seta cg_hudAmmo \"br -32 -72 1.5\"\n",
                "seta cg_hudForce \"c 180 120 0.9\"\n",
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
        assert!(!loaded.first_person_lightsaber);
        assert!(loaded.jump_height_shade);
        assert_eq!(loaded.saber_trail, 2);
        assert!(!loaded.smoothing.smooth_player_origin);
        assert!(!loaded.smoothing.smooth_third_person_origin);
        assert!(!loaded.smoothing.smooth_player_animation);
        assert!(!loaded.smoothing.subframe_player_angles);
        assert!(!loaded.smoothing.smooth_third_person_time);
        assert!((loaded.third_person.alpha - 0.375).abs() < 1e-6);
        assert!((loaded.third_person.angle - 25.0).abs() < 1e-6);
        assert!((loaded.third_person.camera_damp - 0.42).abs() < 1e-6);
        assert!((loaded.third_person.horz_offset - 7.0).abs() < 1e-6);
        assert!((loaded.third_person.pitch_offset + 4.0).abs() < 1e-6);
        assert!((loaded.third_person.range - 128.0).abs() < 1e-6);
        assert!(loaded.third_person.special_cam);
        assert!((loaded.third_person.target_damp - 0.61).abs() < 1e-6);
        assert!((loaded.third_person.vert_offset - 21.0).abs() < 1e-6);
        assert_eq!(loaded.crosshair.style, 5);
        assert!((loaded.crosshair.size - 36.0).abs() < 1e-6);
        assert_eq!(loaded.crosshair.color, [64, 128, 255, 200]);
        assert_eq!(loaded.hud_layout.health.anchor, crate::ui::HudAnchor::BottomLeft);
        assert_eq!(loaded.hud_layout.health.offset, [40.0, -72.0]);
        assert!((loaded.hud_layout.health.scale - 1.25).abs() < 1e-6);
        assert_eq!(loaded.hud_layout.shield.anchor, crate::ui::HudAnchor::BottomCenter);
        assert_eq!(loaded.hud_layout.force.anchor, crate::ui::HudAnchor::Center);
        assert!(!loaded.hud_layout.snap_to_grid);
        assert!((loaded.hud_layout.grid_size - 12.0).abs() < 1e-6);
    }

    #[test]
    fn client_presentation_writer_archives_camera_and_smoothing_choices() {
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
        presentation.first_person_lightsaber = false;
        presentation.jump_height_shade = true;
        presentation.saber_trail = 0;
        presentation.smoothing.smooth_player_origin = false;
        presentation.smoothing.smooth_third_person_origin = false;
        presentation.smoothing.smooth_player_animation = false;
        presentation.smoothing.subframe_player_angles = false;
        presentation.smoothing.smooth_third_person_time = false;
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
        presentation.crosshair.size = 32.0;
        presentation.crosshair.color = [20, 140, 255, 210];
        presentation.hud_layout.health = HudElementLayout {
            anchor: crate::ui::HudAnchor::TopLeft,
            offset: [48.0, 64.0],
            scale: 1.25,
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
        assert!(text.contains("seta cg_fpls \"0\""));
        assert!(text.contains("seta r_jumpHeightShade \"1\""));
        assert!(text.contains("seta cg_saberTrail \"0\""));
        assert!(text.contains("seta cg_fxFPS \"90\""));
        assert!(text.contains("seta cg_smoothPlayerOrigin \"0\""));
        assert!(text.contains("seta cg_smoothThirdPersonOrigin \"0\""));
        assert!(text.contains("seta cg_smoothPlayerAnimation \"0\""));
        assert!(text.contains("seta cg_subframePlayerAngles \"0\""));
        assert!(text.contains("seta cg_smoothThirdPersonTime \"0\""));
        assert!(text.contains("seta cg_thirdPersonAlpha \"0.250\""));
        assert!(text.contains("seta cg_thirdPersonAngle \"15.000\""));
        assert!(text.contains("seta cg_thirdPersonCameraDamp \"0.800\""));
        assert!(text.contains("seta cg_thirdPersonHorzOffset \"6.000\""));
        assert!(text.contains("seta cg_thirdPersonPitchOffset \"-3.000\""));
        assert!(text.contains("seta cg_thirdPersonRange \"96.000\""));
        assert!(text.contains("seta cg_thirdPersonTargetDamp \"0.700\""));
        assert!(text.contains("seta cg_thirdPersonVertOffset \"18.000\""));
        assert!(text.contains("seta cg_drawCrosshair \"4\""));
        assert!(text.contains("seta cg_crosshairSize \"32.000\""));
        assert!(text.contains("seta cg_crosshairColor \"20 140 255 210\""));
        assert!(text.contains("seta cg_hudHealth \"tl 48.000 64.000 1.250\""));
        assert!(text.contains("seta cg_hudSnap \"0\""));
        assert!(text.contains("seta cg_hudGridSize \"16.000\""));

        // cg_thirdPersonSpecialCam remains the TaystJK runtime-only exception.
        // DinurdoJK archives cg_fpls and its render-only smoothing A/B switches.
        assert!(!text.contains("seta cg_thirdPersonSpecialCam"));
    }

    #[test]
    fn client_presentation_defaults_include_dinurdo_view_choices() {
        let settings = ClientPresentationSettings::default();
        assert_eq!(settings.model, "kyle");
        assert!((settings.fov - DEFAULT_CG_FOV).abs() < 1e-6);
        assert!(!settings.third_person.enabled);
        assert!(settings.first_person_lightsaber);
        assert_eq!(settings.saber_trail, 1);
        assert!(settings.smoothing.smooth_player_origin);
        assert!(settings.smoothing.smooth_third_person_origin);
        assert!(settings.smoothing.smooth_player_animation);
        assert!(settings.smoothing.subframe_player_angles);
        assert!(settings.smoothing.smooth_third_person_time);
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
