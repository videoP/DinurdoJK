//! Frontend rain ambience uses the effects bus, independent of map music.
use super::{App, FrontendPage, OverlayMode};
use crate::audio::{AudioBackend, AudioInfo, LoopRequest, RegisteredSound, SoundAssets, SoundOrigin};
use jka_assets::pk3::AssetSearchPath;
use std::{sync::Arc, time::Instant};

const RAIN_LOOPS: [&str; 2] = [
    "sound/japro/rainloop_00.mp3",
    "sound/ambience/outdoor/rainy_lp.wav",
];

#[derive(Default)]
pub(super) struct MenuAmbience {
    backend: Option<AudioBackend>,
    attempted: bool,
    fade: MenuRainFade,
}

#[derive(Default)]
struct MenuRainFade {
    active: bool,
    started: Option<Instant>,
    from: f32,
    level: f32,
}

impl MenuRainFade {
    fn update(&mut self, now: Instant, shown_at: Option<Instant>) -> f32 {
        let active = shown_at.is_some();
        if active != self.active {
            self.from = self.level;
            self.started = Some(shown_at.unwrap_or(now));
            self.active = active;
        }
        let t = self.started.map_or(1.0, |started| {
            (now.saturating_duration_since(started).as_secs_f32() / crate::neon_menu::REVEAL_SECS).clamp(0.0, 1.0)
        });
        let target = if active { 1.0 } else { 0.0 };
        self.level = self.from + (target - self.from) * t;
        self.level
    }
}

impl MenuAmbience {
    pub(in crate::app) fn info(&self) -> Option<AudioInfo> {
        self.backend.as_ref().map(AudioBackend::info)
    }

    fn set_mix(backend: &mut AudioBackend, effects: f32) {
        // Local loops use separation as their per-ear gain. Zero silences them.
        backend.set_mix(effects, 0.0, 1.0);
    }

    fn start_loop(backend: &mut AudioBackend, effects: f32, qpath: &str, sound: Arc<RegisteredSound>) {
        Self::set_mix(backend, effects);
        backend.set_loops(vec![(
            LoopRequest {
                emitter: 0,
                qpath: qpath.to_owned(),
                origin: SoundOrigin::Local,
                volume: 1.0,
            },
            sound,
        )]);
    }

    fn open(assets: AssetSearchPath, effects: f32) -> Result<AudioBackend, String> {
        let mut assets = SoundAssets::new(assets);
        let mut failures = Vec::new();
        for qpath in RAIN_LOOPS {
            match assets.register_ambient_loop(qpath) {
                Ok(sound) => {
                    // Local stereo ambience needs neither HRTF nor map acoustics.
                    let mut backend = AudioBackend::open(false, false, false)?;
                    Self::start_loop(&mut backend, effects, qpath, sound);
                    return Ok(backend);
                }
                Err(error) => failures.push(format!("{qpath}: {error}")),
            }
        }
        Err(failures.join("; "))
    }
}

impl App {
    pub(in crate::app) fn stop_menu_ambience(&mut self) {
        self.menu_ambience = MenuAmbience::default();
        self.neon_menu.shown_at = None;
    }

    pub(in crate::app) fn tick_menu_ambience(&mut self) {
        // A console map request can retain Main/Settings frontend state until
        // upload finishes. Never carry its fade tail into the loading screen.
        if self.loading.is_some() {
            self.stop_menu_ambience();
            return;
        }
        let now = Instant::now();
        // Follow Main's reveal clock; other pages fade the rain away.
        let active = self.front_end
            && self.frontend_page == FrontendPage::Main
            && self.egui_display_overlay() != OverlayMode::Video;
        let shown_at = if active { self.neon_menu.shown_at } else { None };
        let level = self.menu_ambience.fade.update(now, shown_at);
        if shown_at.is_none() && level <= 0.0 {
            self.menu_ambience = MenuAmbience::default();
            return;
        }

        let effects = self.effective_audio_mix().0;
        if shown_at.is_some() && !self.menu_ambience.attempted {
            self.menu_ambience.attempted = true;
            let result = AssetSearchPath::open_game(&self.base, self.game.as_deref())
                .map_err(|error| error.to_string())
                .and_then(|assets| MenuAmbience::open(assets, 0.0));
            match result {
                Ok(backend) => self.menu_ambience.backend = Some(backend),
                Err(error) => {
                    self.push_console_line(format!("^3Menu rain unavailable:^7 {error}"));
                }
            }
        }
        if let Some(backend) = &mut self.menu_ambience.backend {
            MenuAmbience::set_mix(backend, effects * level);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assets() -> AssetSearchPath {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        AssetSearchPath::open(&path).expect("bundled game assets")
    }

    #[test]
    fn menu_rain_is_audible_and_follows_effects_mute_and_unmute() {
        let sound = SoundAssets::new(assets()).register_ambient_loop(RAIN_LOOPS[0]).unwrap();
        let (mut backend, mut output) = AudioBackend::offline();
        MenuAmbience::start_loop(&mut backend, 0.17, RAIN_LOOPS[0], sound);
        let peak = |output: &mut rodio::mixer::MixerSource| {
            output.take(96_000).fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
        };
        assert!(peak(&mut output) > 0.001, "rain must produce audible PCM");
        MenuAmbience::set_mix(&mut backend, 0.0);
        peak(&mut output); // Drain the gain glide and mixer buffering.
        assert!(peak(&mut output) < 0.000001, "SFX mute must silence rain");
        MenuAmbience::set_mix(&mut backend, 0.17);
        assert!(peak(&mut output) > 0.001, "unmuting must restore rain");
        assert_eq!(backend.info().active_loops, 1);
    }

    #[test]
    #[ignore = "opens the default output device and plays two seconds of menu rain"]
    fn menu_rain_device_emits_audio() {
        let backend = MenuAmbience::open(assets(), 0.17).expect("menu rain output device");
        std::thread::sleep(std::time::Duration::from_secs(2));
        let info = backend.info();
        assert_eq!(info.active_loops, 1);
        assert!(info.peak_before_limiter > 0.001, "device must consume audible rain PCM: {info:?}");
        println!("Menu rain device: {} channels, {} Hz, peak {:.4}", info.channels, info.sample_rate, info.peak_before_limiter);
    }

    #[test]
    fn menu_rain_fades_with_reveal_and_finishes_before_retiring() {
        let now = Instant::now();
        let half = std::time::Duration::from_secs_f32(crate::neon_menu::REVEAL_SECS / 2.0);
        let mut fade = MenuRainFade::default();
        assert_eq!(fade.update(now, Some(now)), 0.0);
        assert!((fade.update(now + half, Some(now)) - 0.5).abs() < 0.001);
        assert_eq!(fade.update(now + half * 2, Some(now)), 1.0);
        assert_eq!(fade.update(now + half * 2, None), 1.0);
        assert!((fade.update(now + half * 3, None) - 0.5).abs() < 0.001);
        // Re-entering midway through fade-out must not cut or restart the sound.
        assert!((fade.update(now + half * 3, Some(now + half * 3)) - 0.5).abs() < 0.001);
        assert_eq!(fade.update(now + half * 5, Some(now + half * 3)), 1.0);
        fade.update(now + half * 5, None);
        assert_eq!(fade.update(now + half * 7, None), 0.0);
    }
}
