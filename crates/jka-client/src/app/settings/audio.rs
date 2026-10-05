//! Settings audio.
use crate::app::{App, FootprintMode};

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
        if let Some(playback) = &mut self.game_session {
            if let Some(sound) = &mut playback.sound_presenter {
                sound.set_mix(effects, voice, separation);
                sound.set_music_volume(music);
            }
        }
    }
}
