//! Soundscape playback shares the game mixer and native room transport.
use super::{
    connect_player, spatialize_with_attenuation, AudioBackend, EalVoiceHandle, Gains, Player,
    RegisteredSound, Source, StereoSource,
};
use crate::echo_runtime::AmbienceVoice;
use jka_echofield::{
    geometry::{from_upstream, JKA_UNITS_PER_METER},
    profile::Tap,
    reverb::reflection_filter,
    soundscape::{Clip, Voice, Weather},
};
use std::{
    collections::HashMap,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

const MAX_LOOPS: usize = 32;
const MAX_TRIGGERED: usize = 8;
const MAX_REFLECTIONS: usize = 8;

struct Playing {
    target: Voice,
    sound: Arc<RegisteredSound>,
    dry: Player,
    wet: Option<EalVoiceHandle>,
    gains: Arc<Gains>,
}

impl Playing {
    fn new(backend: &AudioBackend, voice: AmbienceVoice, looping: bool) -> Self {
        let AmbienceVoice { voice, sound } = voice;
        let gains = Arc::new(Gains::new([0.0; 2]));
        gains.eal_track_position.store(true, Ordering::Release);
        let phase = if looping {
            (voice.id as usize) % sound.samples.len()
        } else {
            0
        };
        let source = if looping {
            StereoSource::looping(Arc::clone(&sound), Arc::clone(&gains), phase)
        } else {
            StereoSource::new(Arc::clone(&sound), Arc::clone(&gains))
        };
        let cutoff = spectral_cutoff(voice.gain_hf, sound.sample_rate.get());
        let dry = connect_player(&backend.mixer, source.low_pass(cutoff));
        let mut playing = Self {
            target: voice,
            sound,
            dry,
            wet: None,
            gains,
        };
        playing.respatialize(backend);
        playing.attach(backend, looping, phase);
        playing.set_paused(backend.rate == 0.0);
        playing
    }

    fn attach(&mut self, backend: &AudioBackend, looping: bool, phase: usize) {
        self.wet = backend.eal_bus.as_ref().and_then(|bus| {
            let name = match self.target.clip {
                Clip::Excitation(kind) => kind.name(),
                Clip::Emitter(..) => "echofield_emitter",
                Clip::Terrain(..) => "echofield_terrain",
            };
            bus.add_ambience(
                name,
                Arc::clone(&self.sound),
                Arc::clone(&self.gains),
                looping,
                phase,
            )
        });
        if let Some(wet) = &self.wet {
            wet.set_speed(self.target.speed);
        }
    }

    fn respatialize(&self, backend: &AudioBackend) {
        let origin = from_upstream(self.target.origin).map(|v| v as f32);
        let max_distance = self.target.range * JKA_UNITS_PER_METER as f32;
        let (full, attenuation) = match self.target.clip {
            Clip::Excitation(_) => (8.0 * JKA_UNITS_PER_METER as f32, 1.0 / max_distance),
            _ => (0.0, 1.0 / max_distance),
        };
        let spatial = spatialize_with_attenuation(
            origin,
            backend.listener,
            backend.separation,
            backend.jk2_mode,
            full,
            attenuation,
        );
        let volume = self.target.gain * backend.effects_volume;
        self.gains.set(spatial.stereo.map(|gain| gain * volume));
        self.gains.set_hrtf(
            spatial.mono * volume,
            spatial.direction,
            spatial.hrtf_spatializable,
        );
        self.gains.set_environment_source(origin, true);
        self.gains
            .ambience_gain_hf
            .store(self.target.gain_hf.max(1e-5).to_bits(), Ordering::Relaxed);
        self.gains
            .echo_max_distance
            .store(max_distance.to_bits(), Ordering::Relaxed);
    }

    fn set_paused(&self, paused: bool) {
        self.dry.set_speed(self.target.speed);
        if paused {
            self.dry.pause();
        } else {
            self.dry.play();
        }
        if let Some(wet) = &self.wet {
            if paused {
                wet.pause();
            } else {
                wet.play();
            }
        }
    }

    fn retire(self, backend: &AudioBackend) {
        backend.retire(self.dry, self.wet, &self.gains);
    }
}

fn spectral_cutoff(gain_hf: f32, rate: u32) -> u32 {
    (700.0 + 18_000.0 * gain_hf.clamp(0.0, 1.0)).min(rate as f32 * 0.45) as u32
}

struct Reflection {
    voice: Player,
    gains: Arc<Gains>,
    origin: [f32; 3],
    gain: f32,
}

impl Reflection {
    fn respatialize(&self, backend: &AudioBackend) {
        let spatial = spatialize_with_attenuation(
            self.origin,
            backend.listener,
            backend.separation,
            backend.jk2_mode,
            0.0,
            1.0 / (32.0 * JKA_UNITS_PER_METER as f32),
        );
        self.gains.set(spatial.stereo.map(|gain| gain * self.gain));
    }
}

#[derive(Default)]
pub(super) struct Ambience {
    loops: HashMap<u64, Playing>,
    events: Vec<Playing>,
    reflections: Vec<Reflection>,
    taps: Arc<[Tap]>,
    pub weather: Weather,
}

impl Ambience {
    pub fn active(&self) -> usize {
        self.loops.len() + self.events.len()
    }

    pub fn set_paused(&self, paused: bool) {
        for voice in self.loops.values().chain(&self.events) {
            voice.set_paused(paused);
        }
        for reflection in &self.reflections {
            if paused {
                reflection.voice.pause();
            } else {
                reflection.voice.play();
            }
        }
    }

    pub fn clear(&mut self, backend: &AudioBackend) {
        for (_, voice) in self.loops.drain() {
            voice.retire(backend);
        }
        for voice in self.events.drain(..) {
            voice.retire(backend);
        }
        for reflection in self.reflections.drain(..) {
            backend.retire(reflection.voice, None, &reflection.gains);
        }
        self.taps = Arc::from([]);
    }

    pub fn reflect(&mut self, backend: &AudioBackend, sound: &Arc<RegisteredSound>, level: f32) {
        if level <= 0.0
            || backend.rate == 0.0
            || !backend.echo_active
            || !backend.eal_enabled.load(Ordering::Relaxed)
            || self.weather.submerged
        {
            return;
        }
        self.reflections
            .retain(|reflection| !reflection.voice.empty());
        for tap in self.taps.iter() {
            let [gain, gain_hf] = reflection_filter(tap);
            let gain = (gain
                * tap.energy as f32
                * level
                * backend.echo_settings.reflection_intensity
                * backend.echo_settings.master_volume)
                .clamp(0.0, 1.0);
            if gain < 0.001 {
                continue;
            }
            if self.reflections.len() == MAX_REFLECTIONS {
                let Some((weakest, previous)) = self
                    .reflections
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| a.gain.total_cmp(&b.gain))
                else {
                    continue;
                };
                if previous.gain >= gain {
                    continue;
                }
                let retired = self.reflections.swap_remove(weakest);
                backend.retire(retired.voice, None, &retired.gains);
            }
            let gain_state = Arc::new(Gains::new([0.0; 2]));
            let source = StereoSource::new(Arc::clone(sound), Arc::clone(&gain_state))
                .low_pass(spectral_cutoff(gain_hf, sound.sample_rate.get()))
                .take_duration(Duration::from_secs(5))
                .delay(Duration::from_secs_f64(tap.delay));
            let player = connect_player(&backend.mixer, source);
            player.set_speed(backend.rate);
            let reflection = Reflection {
                voice: player,
                gains: gain_state,
                origin: from_upstream(tap.position).map(|v| v as f32),
                gain,
            };
            reflection.respatialize(backend);
            self.reflections.push(reflection);
        }
    }

    pub fn frame(&mut self, backend: &AudioBackend) {
        let Some(status) = &backend.eal_worker_status else {
            self.clear(backend);
            return;
        };
        if !backend.eal_enabled.load(Ordering::Relaxed)
            || !backend.echo_settings.enhancements
            || !status.running.load(Ordering::Acquire)
            || self.weather.submerged
        {
            self.clear(backend);
            if let Ok(mut scene) = status.ambience.try_lock() {
                scene.loops = None;
                scene.triggered.clear();
                scene.reflections = Arc::from([]);
            }
            return;
        }
        if backend.rate == 0.0 {
            return;
        }
        let (loops, events) = if let Ok(mut latest) = status.ambience.try_lock() {
            self.taps = Arc::clone(&latest.reflections);
            (latest.loops.take(), std::mem::take(&mut latest.triggered))
        } else {
            (None, Default::default())
        };
        if let Some(targets) = loops {
            let mut targets = targets;
            targets.truncate(MAX_LOOPS);
            for (_, voice) in self
                .loops
                .extract_if(|id, _| !targets.iter().any(|request| request.voice.id == *id))
            {
                voice.retire(backend);
            }
            for request in targets {
                let id = request.voice.id;
                if let Some(playing) = self.loops.get_mut(&id) {
                    playing.target = request.voice;
                    if playing.wet.as_ref().is_none_or(|wet| !wet.is_playing()) {
                        let phase =
                            f64::from_bits(playing.gains.eal_position.load(Ordering::Acquire));
                        playing.attach(backend, true, phase.max(0.0) as usize);
                    }
                } else {
                    self.loops.insert(id, Playing::new(backend, request, true));
                }
            }
        }
        self.events.retain(|voice| !voice.dry.empty());
        for request in events
            .into_iter()
            .take(MAX_TRIGGERED.saturating_sub(self.events.len()))
        {
            self.events.push(Playing::new(backend, request, false));
        }
        for voice in self.loops.values().chain(&self.events) {
            voice.respatialize(backend);
        }
        self.reflections
            .retain(|reflection| !reflection.voice.empty());
        for reflection in &self.reflections {
            reflection.respatialize(backend);
        }
    }
}
