//! Disk-backed, scene-specific *visual* placeholders for the egui frontend.
//! This cache never replaces the live WGPU scene or controls gameplay state.
use super::App;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Cached-scene startup fade (black -> intended brightness).
pub(super) const FRONTEND_STAGE_FADE_IN_SECS: f32 = 0.9;
/// Menu chrome fades in over the same clock, a little ahead of the scene.
pub(super) const FRONTEND_CHROME_FADE_IN_SECS: f32 = 0.38;
/// If no presentation ack ever arrives, start anyway rather than stay black.
const FRONTEND_STAGE_ACK_TIMEOUT: Duration = Duration::from_millis(2500);
/// Last-resort escape if the decoder thread never returns (I/O or worker failure).
const FRONTEND_CACHE_DECODE_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) fn smoothstep01(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The one clock behind the menu's startup darkness. It starts when the
/// renderer reports that a frame carrying the (dark) stage was presented, not
/// when the CPU built it, so decode/BSP/GPU stalls cannot consume the fade.
#[derive(Default)]
pub(super) struct FrontendStage {
    /// Bumped by every scene selection; doubles as the present token.
    epoch: u64,
    /// Snapshot PNG is being decoded; the stage stays black until it lands.
    pub(super) cache_pending: bool,
    /// A scene has been selected, so there is something to fade in.
    scene_selected: bool,
    first_built: Option<Instant>,
    pub(super) presented_at: Option<Instant>,
}

impl FrontendStage {
    pub(super) fn begin_scene(&mut self, cache_pending: bool) {
        self.epoch = self.epoch.wrapping_add(1).max(1);
        self.cache_pending = cache_pending;
        self.scene_selected = true;
        self.first_built = None;
        self.presented_at = None;
    }

    /// Token to attach to the next egui frame, once the stage is meaningful to
    /// present: a scene is selected and its snapshot decode (if any) resolved.
    pub(super) fn token_for_frame(&mut self, now: Instant) -> Option<u64> {
        if self.presented_at.is_some() {
            return None;
        }
        let first = *self.first_built.get_or_insert(now);
        // Give a real snapshot time to decode. A hard timeout prevents a
        // failed worker or stalled drive from trapping the menu on black.
        if self.cache_pending {
            if now.saturating_duration_since(first) < FRONTEND_CACHE_DECODE_TIMEOUT {
                return None;
            }
            eprintln!("[MENU CACHE] decoder timeout; continuing with live scene");
            self.cache_pending = false;
        }
        if !self.scene_selected && now.saturating_duration_since(first) < FRONTEND_STAGE_ACK_TIMEOUT {
            return None;
        }
        Some(self.epoch)
    }

    /// Returns true when this ack started the clock.
    pub(super) fn on_presented(&mut self, token: u64, now: Instant) -> bool {
        if token != self.epoch || self.presented_at.is_some() {
            return false;
        }
        self.presented_at = Some(now);
        true
    }

    pub(super) fn elapsed_secs(&self, now: Instant) -> Option<f32> {
        self.presented_at.map(|at| now.saturating_duration_since(at).as_secs_f32())
    }

    /// 0 until the first presented dark frame, then eased up to 1.
    pub(super) fn brightness(&self, now: Instant, fade_secs: f32) -> f32 {
        self.elapsed_secs(now).map_or(0.0, |secs| smoothstep01(secs / fade_secs))
    }

    /// Still animating, or waiting to start: callers must keep repainting.
    pub(super) fn animating(&self, now: Instant) -> bool {
        self.elapsed_secs(now).is_none_or(|secs| secs < FRONTEND_STAGE_FADE_IN_SECS)
    }
}

/// Stable per-scene key, deliberately independent of any hardcoded menu map.
/// The selected map and active mod namespace both participate in the key.
fn scene_hash(scene: &str, mod_name: &str, video: &crate::ui::VideoSettings) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in scene.bytes().chain([0])
        .chain(mod_name.bytes()).chain([0])
        .chain(super::FRONTEND_FOREGROUND_MODEL.bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    // Cache identity includes the scene-affecting settings rather than just the
    // map name. The normalized Debug representation also includes new fields
    // added to VideoSettings in future revisions (conservative invalidation).
    let mut normalized = *video;
    if gamma_independent_cache(video) {
        normalized.gamma = 1.0;
        normalized.sync_linked_brightness();
    }
    // Present pacing, window placement and the HUD are not in the scene-only
    // readback. Do not waste disk space on snapshots for different FPS caps,
    // VSync settings or debug-overlay visibility.
    let defaults = crate::ui::VideoSettings::default();
    normalized.fps_cap = defaults.fps_cap;
    normalized.vsync = defaults.vsync;
    normalized.max_frame_latency = defaults.max_frame_latency;
    normalized.early_submit = defaults.early_submit;
    normalized.present_copy = defaults.present_copy;
    normalized.fullscreen = defaults.fullscreen;
    normalized.window_position = defaults.window_position;
    normalized.window_maximized = defaults.window_maximized;
    normalized.draw_fps = defaults.draw_fps;
    normalized.draw_timer = defaults.draw_timer;
    normalized.draw_2d = defaults.draw_2d;
    normalized.draw_hud = defaults.draw_hud;
    normalized.skip_ui = defaults.skip_ui;
    normalized.perf_thread_page = defaults.perf_thread_page;
    normalized.developer_level = defaults.developer_level;
    normalized.developer_tools = defaults.developer_tools;
    normalized.renderer_verbose = defaults.renderer_verbose;
    normalized.pipeline_trace = defaults.pipeline_trace;
    normalized.perf_trace = defaults.perf_trace;
    normalized.gpu_timings = defaults.gpu_timings;
    normalized.timer_resolution_1ms = defaults.timer_resolution_1ms;
    for byte in format!("{normalized:?}").bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// A cached scene is gamma-independent when gamma does not change the
/// *lighting* of that scene. Shader gamma is a display operation, so the PNG
/// can be approximately normalized on the screenshot worker and regraded on
/// decode even if vignette, LUT, weather grading, etc. are enabled. Those
/// effects follow gamma in the shader, so reconstruction from an 8-bit final
/// image is NOT exact; an exact solution needs a pre-grading capture.
///
/// Hardware gamma acts on the entire desktop output *after* our readback:
/// both cached egui and live 3D receive the same hardware ramp, so no pixel
/// transform is necessary. Baked gamma and unlocked brightness multipliers
/// alter actual scene lighting and must retain separate cache identities.
pub(super) fn gamma_independent_cache(video: &crate::ui::VideoSettings) -> bool {
    video.hdr_output == 0
        && video.model_brightness_locked
        && video.dynamic_light_brightness_locked
        && matches!(video.gamma_method, crate::gamma::GammaMethod::Shader | crate::gamma::GammaMethod::Hardware)
}

/// Shader-gamma image neutralization is performed only on workers, never as
/// part of a gameplay frame. Hardware gamma is already display-independent.
pub(super) fn shader_gamma_reusable(video: &crate::ui::VideoSettings) -> bool {
    gamma_independent_cache(video) && video.gamma_method == crate::gamma::GammaMethod::Shader
}

pub(super) fn path(base: &Path, game: Option<&Path>, scene: &str, video: &crate::ui::VideoSettings) -> PathBuf {
    let mod_name = game.map_or("base", |path| path.to_str().unwrap_or("mod"));
    base.join("cache").join("frontend_scenes").join(format!(
        "v7_{:016x}.png", scene_hash(scene, mod_name, video),
    ))
}

impl App {
    pub(super) fn load_frontend_scene_cache(&mut self) {
        self.frontend_scene_texture = None;
        self.render_command(crate::renderer::RenderCommand::SetNeonMenuCache(None));
        self.frontend_scene_cache_generation = self.frontend_scene_cache_generation.wrapping_add(1);
        let generation = self.frontend_scene_cache_generation;
        let Some(path) = self.frontend_scene_cache_path.clone() else { return; };
        self.frontend_stage.cache_pending = path.exists();
        let neutral = shader_gamma_reusable(&self.video);
        println!("[MENU CACHE] load={} gamma={:.3} method={} neutral={} reusable={} hit={}",
            path.display(), self.video.gamma, self.video.gamma_method.config_value(),
            neutral, gamma_independent_cache(&self.video), path.exists());
        let gamma = if neutral { self.video.gamma } else { 1.0 };
        let proxy = self.proxy.clone();
        // PNG decoding and filesystem access must never hold up the first
        // clickable menu frame. Only texture registration happens on main.
        let spawn = std::thread::Builder::new()
            .name("frontend-scene-cache".into())
            .spawn(move || {
                remove_legacy_snapshots(&path);
                let validated = crate::frontend_cache_color::cache_metadata_valid(&path, neutral);
                let bytes = validated.then(|| std::fs::read(&path).ok()).flatten();
                let image = bytes.as_ref()
                    .and_then(|bytes| image::load_from_memory(bytes).ok())
                    .map(|image| {
                        let mut rgba = image.to_rgba8();
                        // The cached PNG contains pre-display-gamma sRGB bytes.
                        // Apply current gamma once on this decode worker, NOT on
                        // the frame/render thread (and never through egui tint).
                        crate::frontend_cache_color::transform_rgba(
                            rgba.as_mut(), 1.0 / gamma.max(0.05), true,
                        );
                        let size = [rgba.width() as usize, rgba.height() as usize];
                        egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())
                    });
                if path.exists() && image.is_none() {
                    // A corrupt entry must not block a replacement capture.
                    // Report the failure explicitly before removing it, so
                    // startup issues can be diagnosed from the console log.
                    eprintln!("[MENU CACHE] invalid or unreadable image (recapturing): {}", path.display());
                    let _ = std::fs::remove_file(&path);
                    let _ = std::fs::remove_file(path.with_extension("json"));
                }
                let _ = proxy.send_event(crate::runtime::UserEvent::FrontendSceneCacheReady {
                    path, generation, image,
                });
            });
        if let Err(error) = spawn {
            eprintln!("[MENU CACHE] failed to spawn decoder: {error}");
            self.frontend_stage.cache_pending = false;
        }
    }

    /// Independent background handoff. UI/neon animation must never wait for it.
    pub(super) fn frontend_scene_handoff(&self, now: Instant) -> f32 {
        self.frontend_cinematic
            .and_then(|cinematic| cinematic.fade_started)
            .map_or(0.0, |started| {
                let started = self.frontend_visual_handoff_start(started);
                smoothstep01(now.saturating_duration_since(started).as_secs_f32()
                    / crate::app::egui_menu::FRONTEND_SCENE_FADE_IN_SECS)
            })
    }

    /// Match the photograph-to-live handoff to the *presented* cached scene,
    /// not to an earlier CPU-side readiness flag. Camera/model animation uses
    /// this same time, so extending the 2D hold cannot desynchronize the pose.
    pub(super) fn frontend_visual_handoff_start(&self, ready: Instant) -> Instant {
        if self.frontend_scene_texture.is_some() {
            self.frontend_stage.presented_at
                .map(|presented| ready.max(presented + Duration::from_millis(850)))
                .unwrap_or(ready)
        } else {
            ready
        }
    }

    /// Only called by a gamma setting change, not by the frame loop. Keep the
    /// current texture until the asynchronously decoded replacement arrives.
    pub(super) fn refresh_frontend_cache_for_gamma(&mut self) {
        if !self.front_end || self.frontend_scene_cache_path.is_none() {
            return;
        }
        // Baked gamma or scene lighting changes require a separate snapshot.
        // A post-effects grade can only be approximately reconstructed from
        // 8-bit PNG bytes, but must NOT cause another cache for plain r_gamma.
        if !shader_gamma_reusable(&self.video) {
            return;
        }
        let Some(path) = self.frontend_scene_cache_path.clone() else { return; };
        if !path.exists() || self.frontend_cinematic.is_some_and(|cinematic| {
            cinematic.fade_started.is_some_and(|since| {
                since.elapsed().as_secs_f32() >= crate::app::egui_menu::FRONTEND_SCENE_FADE_IN_SECS
            })
        }) {
            // The live 3D menu is already fully visible. Never decode a large
            // PNG just to update an image that has faded to alpha zero.
            return;
        }
        let generation = self.frontend_scene_cache_generation.wrapping_add(1);
        self.frontend_scene_cache_generation = generation;
        let proxy = self.proxy.clone();
        let gamma = self.video.gamma;
        let _ = std::thread::Builder::new()
            .name("frontend-scene-gamma".into())
            .spawn(move || {
                let image = crate::frontend_cache_color::cache_metadata_valid(&path, true)
                    .then(|| std::fs::read(&path).ok())
                    .flatten()
                    .and_then(|bytes| image::load_from_memory(&bytes).ok())
                    .map(|image| {
                        let mut rgba = image.to_rgba8();
                        crate::frontend_cache_color::transform_rgba(
                            rgba.as_mut(), 1.0 / gamma.max(0.05), true,
                        );
                        let size = [rgba.width() as usize, rgba.height() as usize];
                        egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())
                    });
                if image.is_none() && path.exists() {
                    eprintln!("[MENU CACHE] refresh decode failed: {}", path.display());
                }
                let _ = proxy.send_event(crate::runtime::UserEvent::FrontendSceneCacheReady {
                    path, generation, image,
                });
            });
    }

    pub(super) fn select_frontend_scene(&mut self, scene: &str) {
        let path = path(&self.base, self.game.as_deref(), scene, &self.video);
        // Every entry to the menu scene restarts the one stage timeline.
        self.frontend_stage.begin_scene(false);
        if self.frontend_scene_cache_path.as_ref() != Some(&path) {
            self.frontend_scene_cache_path = Some(path);
            self.load_frontend_scene_cache();
        } else if self.frontend_scene_texture.is_none() && path.exists() {
            // The very first visit just populated this cache. Keep its cold
            // reveal undisturbed; load it when the menu is next requested.
            self.load_frontend_scene_cache();
        }
        // A texture that is already resident is not pending.
        self.frontend_stage.cache_pending &= self.frontend_scene_texture.is_none();
        // Prevent reusing the previous world's animation phase while the
        // replacement BSP is still loading in the background.
        self.frontend_cinematic = None;
    }

    /// Token for the egui frame being built (see `FrontendStage`).
    pub(super) fn frontend_stage_token(&mut self) -> Option<u64> {
        if !self.front_end {
            return None;
        }
        self.frontend_stage.token_for_frame(Instant::now())
    }

    pub(super) fn on_frontend_stage_presented(&mut self, token: u64) {
        let now = Instant::now();
        if self.frontend_stage.on_presented(token, now) {
            println!(
                "[MENU FADE] first dark frame presented: cached_texture={} cinematic_fade_started={}",
                self.frontend_scene_texture.is_some(),
                self.frontend_cinematic.is_some_and(|c| c.fade_started.is_some()),
            );
            self.egui_repaint_requested = true;
        }
    }
}

/// Best-effort removal of snapshots written by older cache formats.
/// v4 inherited gameplay FOV and can no longer be used for the fixed menu
/// projection; v3 and earlier could contain console/HUD pixels.
fn remove_legacy_snapshots(current: &Path) {
    let Some(dir) = current.parent() else { return; };
    let Ok(entries) = std::fs::read_dir(dir) else { return; };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if (name.starts_with("v5_") || name.starts_with("v4_") || name.starts_with("v3_")
            || name.starts_with("v2_") || name.starts_with("v1_"))
            && name.ends_with(".jpg")
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shader_gamma_reuses_scene_cache_even_with_display_grading() {
        let mut video = crate::ui::VideoSettings::default();
        assert!(shader_gamma_reusable(&video));
        video.vignette = true;
        video.rain = true;
        video.film_grain_strength = 0.25;
        video.color_lut = crate::ui::ColorLutPreset::KodakVision3_250d;
        let first = path(Path::new("base"), None, "mp/duel3", &video);
        video.gamma = 1.7;
        video.sync_linked_brightness();
        assert!(shader_gamma_reusable(&video));
        assert_eq!(first, path(Path::new("base"), None, "mp/duel3", &video));
    }

    #[test]
    fn baked_and_upstream_lighting_changes_do_not_reuse_gamma_cache() {
        let mut video = crate::ui::VideoSettings::default();
        let first = path(Path::new("base"), None, "mp/duel3", &video);
        video.model_brightness_locked = false;
        video.gamma = 1.7;
        video.sync_linked_brightness();
        assert!(!gamma_independent_cache(&video));
        assert_ne!(first, path(Path::new("base"), None, "mp/duel3", &video));
        video = crate::ui::VideoSettings::default();
        video.gamma_method = crate::gamma::GammaMethod::Baked;
        let baked = path(Path::new("base"), None, "mp/duel3", &video);
        video.gamma = 1.7;
        video.sync_linked_brightness();
        assert_ne!(baked, path(Path::new("base"), None, "mp/duel3", &video));
    }

    #[test]
    fn hardware_gamma_reuses_without_png_transform() {
        let mut video = crate::ui::VideoSettings::default();
        video.gamma_method = crate::gamma::GammaMethod::Hardware;
        assert!(gamma_independent_cache(&video));
        assert!(!shader_gamma_reusable(&video));
        let first = path(Path::new("base"), None, "mp/duel3", &video);
        video.gamma = 2.0;
        video.sync_linked_brightness();
        assert_eq!(first, path(Path::new("base"), None, "mp/duel3", &video));
    }

    #[test]
    fn presentation_controls_do_not_multiply_scene_caches() {
        let mut video = crate::ui::VideoSettings::default();
        let first = path(Path::new("base"), None, "mp/duel3", &video);
        video.fps_cap = 144;
        video.draw_fps = 2;
        video.max_frame_latency = 1;
        video.present_copy = true;
        assert_eq!(first, path(Path::new("base"), None, "mp/duel3", &video));
    }

    #[test]
    fn stage_clock_starts_only_on_matching_presentation() {
        let t0 = Instant::now();
        let mut stage = FrontendStage::default();
        assert_eq!(stage.token_for_frame(t0), None, "no scene selected yet");
        assert!(stage.token_for_frame(t0 + Duration::from_secs(3)).is_some(), "timeout fallback");
        stage.begin_scene(true);
        assert_eq!(stage.token_for_frame(t0), None, "waiting on snapshot decode");
        stage.cache_pending = false;
        let token = stage.token_for_frame(t0).expect("armed once snapshot resolved");
        assert_eq!(stage.brightness(t0, 0.9), 0.0);
        assert!(!stage.on_presented(token + 1, t0), "stale token ignored");
        assert!(stage.on_presented(token, t0));
        assert!(!stage.on_presented(token, t0), "clock never restarts");
        assert_eq!(stage.brightness(t0, 0.9), 0.0, "first presented frame is dark");
        let mid = stage.brightness(t0 + Duration::from_millis(450), 0.9);
        assert!((mid - 0.5).abs() < 1e-4);
        assert_eq!(stage.brightness(t0 + Duration::from_secs(2), 0.9), 1.0);
        assert_eq!(stage.token_for_frame(t0), None);
    }

    #[test]
    fn stage_waits_for_actual_snapshot_decode_even_after_old_timeout() {
        let t0 = Instant::now();
        let mut stage = FrontendStage::default();
        stage.begin_scene(true);
        assert_eq!(stage.token_for_frame(t0), None);
        assert_eq!(stage.token_for_frame(t0 + Duration::from_secs(3)), None);
        assert!(stage.token_for_frame(t0 + Duration::from_secs(11)).is_some());
        assert!(!stage.cache_pending);
    }

    #[test]
    fn new_scene_restarts_the_clock() {
        let t0 = Instant::now();
        let mut stage = FrontendStage::default();
        stage.begin_scene(false);
        let first = stage.token_for_frame(t0).unwrap();
        stage.on_presented(first, t0);
        stage.begin_scene(false);
        assert_eq!(stage.brightness(t0 + Duration::from_secs(5), 0.9), 0.0);
        assert_ne!(stage.token_for_frame(t0), Some(first));
    }
    #[test]
    fn cache_keys_are_scene_and_mod_specific() {
        let settings = crate::ui::VideoSettings::default();
        assert_eq!(scene_hash("mp/duel3", "base", &settings), scene_hash("mp/duel3", "base", &settings));
        assert_ne!(scene_hash("mp/duel3", "base", &settings), scene_hash("mp/duel1", "base", &settings));
        assert_ne!(scene_hash("mp/duel3", "base", &settings), scene_hash("mp/duel3", "japro", &settings));
    }
}
