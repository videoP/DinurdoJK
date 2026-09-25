//! Native audio device/mixer and registered SFX cache. No CGame event IDs here.
use std::{
    collections::{HashMap, HashSet},
    io::Cursor,
    sync::{atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering}, Arc},
    time::{Duration, Instant},
};
use jka_assets::pk3::AssetSearchPath;
use jka_assets::bsp::AcousticMesh;
use crate::steam_audio::SteamAudioBakeData;
use rodio::{
    mixer::{Mixer, MixerSource},
    source::LimitSettings,
    Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, Source,
};

const MAX_SOUND_BYTES: usize = 16 * 1024 * 1024;
const MAX_DECODED_SAMPLES: usize = 8 * 1024 * 1024;
const MAX_CACHE_SAMPLES: usize = 32 * 1024 * 1024;
// OpenJK's software mixer has 32 simultaneously paintable channels. Keeping the
// same ceiling prevents a burst of demo events from stacking 100+ full-scale
// float sources in Rodio before the CGame channel semantics are complete.
const MAX_VOICES: usize = 32;
// OpenJK respatializes once per client frame and paints the next mix-ahead
// chunk at the new left/right volume. With s_separation below 1.0 a nearby
// entity or a turning third-person camera moves gains by large steps each
// frame, and those steps are audible as zipper crackle in the float mixer.
// Glide toward each frame's target instead; 10 ms is shorter than a 60 Hz
// frame, so localisation still follows OpenJK's per-frame spatialization.
const GAIN_GLIDE_SECONDS: f32 = 0.010;
// OpenJK cuts stomped/stolen channels mid-waveform. A 4 ms fade is inaudible
// as a semantic change but removes the step discontinuity of a hard stop.
const STOP_FADE_SECONDS: f32 = 0.004;
// S_AddLoopSounds merges every loop of one sfx into a single channel. Vanilla
// takes those channels from the same 32-channel pool as one-shots; merged loops
// get their own ceiling here so a hum never steals an event sound mid-play.
const MAX_LOOP_VOICES: usize = 32;
const CHAN_AUTO: i32 = 0;

#[derive(Clone, Copy, Debug)]
pub enum SoundOrigin {
    Local,
    Entity([f32; 3]), // fallback position until an entity update arrives
    Fixed([f32; 3]),
}

#[derive(Clone, Debug)]
pub struct SoundRequest {
    pub qpath: String,
    pub entity: u16,
    pub channel: i32,
    pub origin: SoundOrigin,
}

/// One CGame `S_AddLoopingSound` call. CGame re-adds loops every frame, so
/// the origin is already this frame's position. `SoundOrigin::Local` means the
/// listener's own view origin (the local player's saber hum soundSpot).
#[derive(Clone, Debug)]
pub struct LoopRequest {
    pub qpath: String,
    pub origin: SoundOrigin,
}

#[derive(Clone, Copy)]
pub struct Listener {
    pub entity: u16,
    pub origin: [f32; 3],
    pub left: [f32; 3],
}

pub struct RegisteredSound {
    samples: Arc<[f32]>, // mono SFX, as in the legacy software sound path
    sample_rate: rodio::SampleRate,
}

pub struct SoundAssets {
    assets: AssetSearchPath,
    cache: HashMap<String, Arc<RegisteredSound>>,
    failed: HashSet<String>,
    cached_samples: usize,
    sample_rate_counts: HashMap<u32, usize>,
}

impl SoundAssets {
    pub fn new(assets: AssetSearchPath) -> Self {
        Self {
            assets,
            cache: HashMap::new(),
            failed: HashSet::new(),
            cached_samples: 0,
            sample_rate_counts: HashMap::new(),
        }
    }

    pub fn register(&mut self, qpath: &str) -> Result<Arc<RegisteredSound>, String> {
        let key = qpath.replace('\\', "/").to_ascii_lowercase();
        if let Some(sound) = self.cache.get(&key) { return Ok(Arc::clone(sound)); }
        if self.failed.contains(&key) { return Err(format!("sound previously failed: {key}")); }
        let result = self.load(&key);
        match result {
            Ok(sound) => {
                let sound = Arc::new(sound);
                self.cached_samples += sound.samples.len();
                *self.sample_rate_counts.entry(sound.sample_rate.get()).or_insert(0) += 1;
                self.cache.insert(key, Arc::clone(&sound));
                Ok(sound)
            }
            Err(error) => {
                if self.failed.len() < 4096 { self.failed.insert(key); }
                Err(error)
            }
        }
    }

    /// Small text sidecar reader used by CGame media registration (for example
    /// models/players/<model>/sounds*.cfg). This deliberately bypasses the
    /// decoded-sound cache because the file is metadata, not an SFX.
    pub fn read_text(&mut self, qpath: &str, max_bytes: usize) -> Result<Option<String>, String> {
        let Some(asset) = self.assets.read(qpath, max_bytes).map_err(|error| error.to_string())? else {
            return Ok(None);
        };
        Ok(Some(String::from_utf8_lossy(&asset.bytes).into_owned()))
    }

    pub fn sample_rate_summary(&self) -> String {
        if self.sample_rate_counts.is_empty() {
            return "none loaded".into();
        }
        let mut rates: Vec<_> = self.sample_rate_counts.iter().map(|(&rate, &count)| (rate, count)).collect();
        rates.sort_unstable_by_key(|&(rate, _)| rate);
        rates
            .into_iter()
            .map(|(rate, count)| format!("{rate}Hz x{count}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn load(&mut self, qpath: &str) -> Result<RegisteredSound, String> {
        // JKA commonly requests a .wav whose shipped asset is an .mp3.
        let stem = qpath.strip_suffix(".wav").or_else(|| qpath.strip_suffix(".mp3")).unwrap_or(qpath);
        let candidates = if qpath.ends_with(".wav") || qpath.ends_with(".mp3") {
            vec![qpath.to_owned(), format!("{stem}.wav"), format!("{stem}.mp3")]
        } else { vec![format!("{stem}.wav"), format!("{stem}.mp3")] };
        for candidate in candidates {
            if let Some(asset) = self.assets.read(&candidate, MAX_SOUND_BYTES).map_err(|e| e.to_string())? {
                let sound = decode_sound(asset.bytes)?;
                if self.cached_samples + sound.samples.len() > MAX_CACHE_SAMPLES {
                    return Err("SFX cache limit reached (128 MiB)".into());
                }
                return Ok(sound);
            }
        }
        Err(format!("missing sound {qpath}"))
    }
}

fn decode_sound(bytes: Vec<u8>) -> Result<RegisteredSound, String> {
    let mut decoder = Decoder::try_from(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let sample_rate = decoder.sample_rate();
    let channels = usize::from(decoder.channels().get());
    if channels > 2 { return Err("SFX must be mono or stereo".into()); }
    let mut samples = Vec::new();
    while let Some(first) = decoder.next() {
        let sample = if channels == 2 { (first + decoder.next().unwrap_or(0.0)) * 0.5 } else { first };
        if !sample.is_finite() { return Err("non-finite sound sample".into()); }
        if samples.len() == MAX_DECODED_SAMPLES { return Err("decoded SFX size limit".into()); }
        samples.push(sample);
    }
    if samples.is_empty() { return Err("empty sound".into()); }
    Ok(RegisteredSound { samples: samples.into(), sample_rate })
}

/// Per-frame spatialization targets written by the game thread. The audio
/// callback glides toward them; `stopping` requests a short fade-out.
struct Gains { target: [AtomicU32; 2], stopping: AtomicBool }
impl Gains {
    fn new(value: [f32; 2]) -> Self {
        Self { target: value.map(|v| AtomicU32::new(v.to_bits())), stopping: AtomicBool::new(false) }
    }
    fn get(&self) -> [f32; 2] {
        std::array::from_fn(|i| f32::from_bits(self.target[i].load(Ordering::Relaxed)))
    }
    fn set(&self, value: [f32; 2]) {
        for (gain, value) in self.target.iter().zip(value) { gain.store(value.to_bits(), Ordering::Relaxed); }
    }
}

// A gain adapter, not a mixer: Rodio supplies mixing, resampling and playback.
// Its callback only reads immutable PCM and atomics, never VFS or CGame state.
struct StereoSource {
    sound: Arc<RegisteredSound>,
    gains: Arc<Gains>,
    cursor: usize,
    current: [f32; 2],
    glide: f32,
    fade: f32,
    fade_step: f32,
    finished: bool,
    looping: bool,
}
impl StereoSource {
    fn new(sound: Arc<RegisteredSound>, gains: Arc<Gains>) -> Self {
        let rate = sound.sample_rate.get() as f32;
        Self {
            current: gains.get(), // start at the spatialized level, no fade-in
            glide: 1.0 - (-1.0 / (GAIN_GLIDE_SECONDS * rate)).exp(),
            fade: 1.0,
            fade_step: 1.0 / (STOP_FADE_SECONDS * rate).max(1.0),
            finished: false,
            looping: false,
            sound,
            gains,
            cursor: 0,
        }
    }

    /// A merged loop channel. It starts `start_frame` into the sample (the
    /// shared loop clock) and glides in from silence, because joining a
    /// waveform mid-cycle at full level would itself be a click.
    fn looping(sound: Arc<RegisteredSound>, gains: Arc<Gains>, start_frame: usize) -> Self {
        let mut source = Self::new(sound, gains);
        source.looping = true;
        source.current = [0.0; 2];
        source.cursor = (start_frame % source.sound.samples.len()) * 2;
        source
    }
}
impl Iterator for StereoSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.finished { return None; }
        if self.cursor % 2 == 0 {
            if self.cursor / 2 >= self.sound.samples.len() {
                if !self.looping {
                    self.finished = true;
                    return None;
                }
                self.cursor = 0;
            }
            // Advance gains once per stereo frame so left/right stay aligned.
            let target = self.gains.get();
            for (current, target) in self.current.iter_mut().zip(target) {
                *current += (target - *current) * self.glide;
            }
            if self.gains.stopping.load(Ordering::Relaxed) {
                self.fade -= self.fade_step;
                if self.fade <= 0.0 {
                    self.finished = true;
                    return None;
                }
            }
        }
        let sample = self.sound.samples[self.cursor / 2];
        let gain = self.current[self.cursor % 2] * self.fade;
        self.cursor += 1;
        Some(sample * gain)
    }
}
impl Source for StereoSource {
    fn current_span_len(&self) -> Option<usize> {
        let ended = !self.looping && self.cursor / 2 >= self.sound.samples.len();
        if self.finished || ended { Some(0) } else { None }
    }
    fn channels(&self) -> rodio::ChannelCount { rodio::nz!(2) }
    fn sample_rate(&self) -> rodio::SampleRate { self.sound.sample_rate }
    fn total_duration(&self) -> Option<Duration> {
        if self.looping { return None; }
        Some(Duration::from_secs_f64(self.sound.samples.len() as f64 / self.sound.sample_rate.get() as f64))
    }
}

struct Voice { player: Player, gains: Arc<Gains>, request: SoundRequest }
struct LoopVoice { player: Player, gains: Arc<Gains> }

/// Queue `source` on a new Player *before* handing the Player's output to the
/// mixer. `Mixer::add` builds its channel/rate converter from the metadata the
/// queue reports at that instant; `Player::connect_new` + `append` lets it see
/// the empty queue (1 channel, 512-sample span), so the first 512 samples of
/// every stereo voice were read as mono: time-stretched 2x with left and right
/// gains alternating per sample. With s_separation < 1 that is a buzz/crackle
/// on every sound onset; at 1.0 (equal gains) it was nearly inaudible.
fn connect_player<S>(mixer: &Mixer, source: S) -> Player
where
    S: Source + Send + 'static,
{
    let (player, output) = Player::new();
    player.append(source);
    mixer.add(output);
    player
}

#[derive(Clone, Debug)]
pub struct AudioInfo {
    pub channels: u16,
    pub sample_rate: u32,
    pub sample_format: String,
    pub buffer_size: String,
    pub active_voices: usize,
    /// Merged loop channels (one per distinct looping sfx this frame).
    pub active_loops: usize,
    /// `S_AddLoopingSound` calls this frame, before per-sfx merging.
    pub loop_requests: usize,
    pub overload_samples: u64,
    pub peak_before_limiter: f32,
    /// Largest single-frame change of any voice's left/right target gain.
    /// Large values mean the listener/entity pan moved abruptly; those steps
    /// are what the per-sample gain glide smooths out.
    pub largest_gain_step: f32,
    /// Runtime gate only. Environmental data is prepared separately by the map
    /// loader and will be attached to this backend as Steam Audio integration lands.
    pub steam_audio_enabled: bool,
    pub steam_audio_scene_triangles: usize,
    pub steam_audio_bake_ready: bool,
    pub steam_audio_probe_count: usize,
    pub steam_audio_bake_bytes: usize,
    pub steam_audio_bake_cache_hit: bool,
    pub steam_audio_runtime_validated: bool,
    /// This remains false until the fixed-rate block DSP path is connected.
    /// Keeping it explicit prevents "enabled" from being mistaken for audible
    /// Steam Audio processing.
    pub steam_audio_dsp_active: bool,
}

/// Final game-mix tap. Rodio's dynamic mixer sums floating-point sources and
/// intentionally allows the result to exceed +/-1.0. OpenJK's software mixer,
/// by contrast, clamps the completed paint buffer before it reaches the DMA
/// device. Keep this source alive even while the game mixer is temporarily
/// empty, measure overloads for `soundinfo`, then feed it through Rodio's
/// final safety limiter before the physical device mixer.
struct MasterTap {
    inner: MixerSource,
    overload_samples: Arc<AtomicU64>,
    peak_bits: Arc<AtomicU32>,
}
impl Iterator for MasterTap {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let sample = self.inner.next().unwrap_or(0.0);
        let magnitude = sample.abs();
        if magnitude > 1.0 {
            self.overload_samples.fetch_add(1, Ordering::Relaxed);
        }
        let bits = magnitude.to_bits();
        let mut current = self.peak_bits.load(Ordering::Relaxed);
        while bits > current {
            match self.peak_bits.compare_exchange_weak(
                current,
                bits,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
        Some(sample)
    }
}
impl Source for MasterTap {
    fn current_span_len(&self) -> Option<usize> { None }
    fn channels(&self) -> rodio::ChannelCount { self.inner.channels() }
    fn sample_rate(&self) -> rodio::SampleRate { self.inner.sample_rate() }
    fn total_duration(&self) -> Option<Duration> { None }
}

pub struct AudioBackend {
    voices: Vec<Voice>,
    loops: Vec<LoopRequest>,
    loop_voices: HashMap<String, LoopVoice>,
    // OpenJK paints loops at `s_paintedtime % length`: every loop of one sfx
    // shares a cycle, and a loop that restarts resumes at that clock's phase.
    loop_epoch: Instant,
    mixer: Mixer,
    _master_player: Option<Player>,
    _device: Option<MixerDeviceSink>,
    listener: Listener,
    origins: HashMap<u16, [f32; 3]>,
    effects_volume: f32,
    voice_volume: f32,
    separation: f32,
    rate: f32,
    output_channels: u16,
    output_sample_rate: u32,
    output_sample_format: String,
    output_buffer_size: String,
    overload_samples: Arc<AtomicU64>,
    peak_bits: Arc<AtomicU32>,
    largest_gain_step: f32,
    steam_audio_enabled: bool,
    steam_audio_acoustic_mesh: Option<Arc<AcousticMesh>>,
    steam_audio_bake: Option<Arc<SteamAudioBakeData>>,
}

impl AudioBackend {
    pub fn open(steam_audio_enabled: bool) -> Result<Self, String> {
        let mut device = DeviceSinkBuilder::open_default_sink().map_err(|e| e.to_string())?;
        device.log_on_drop(false);
        let output_channels = device.config().channel_count().get();
        let output_sample_rate = device.config().sample_rate();
        let output_sample_format = format!("{:?}", device.config().sample_format());
        let output_buffer_size = format!("{:?}", device.config().buffer_size());

        // Use the physical device's native rate. Each RegisteredSound advertises
        // its real source rate, so Rodio resamples 11/22/44 kHz JKA assets to this
        // rate rather than treating them as if they were already 48 kHz.
        let (mixer, mixed) = rodio::mixer::mixer(rodio::nz!(2), output_sample_rate);
        let overload_samples = Arc::new(AtomicU64::new(0));
        let peak_bits = Arc::new(AtomicU32::new(0.0_f32.to_bits()));
        let tap = MasterTap {
            inner: mixed,
            overload_samples: Arc::clone(&overload_samples),
            peak_bits: Arc::clone(&peak_bits),
        };

        // OpenJK hard-clamps its completed 16-bit paintbuffer. A transparent
        // -1 dB limiter gives the float backend equivalent overload protection
        // without turning every crowded saber clash into hard digital clipping.
        let master_player = connect_player(device.mixer(), tap.limit(LimitSettings::default()));

        Ok(Self {
            voices: Vec::new(),
            loops: Vec::new(),
            loop_voices: HashMap::new(),
            loop_epoch: Instant::now(),
            mixer,
            _master_player: Some(master_player),
            _device: Some(device),
            origins: HashMap::new(),
            listener: Listener { entity: 0, origin: [0.0; 3], left: [0.0, 1.0, 0.0] },
            effects_volume: 0.5,
            voice_volume: 1.0,
            separation: 0.5,
            rate: 1.0,
            output_channels,
            output_sample_rate: output_sample_rate.get(),
            output_sample_format,
            output_buffer_size,
            overload_samples,
            peak_bits,
            largest_gain_step: 0.0,
            steam_audio_enabled,
            steam_audio_acoustic_mesh: None,
            steam_audio_bake: None,
        })
    }

    #[cfg(test)]
    fn from_mixer(mixer: Mixer) -> Self {
        Self {
            voices: Vec::new(),
            loops: Vec::new(),
            loop_voices: HashMap::new(),
            loop_epoch: Instant::now(),
            mixer,
            _master_player: None,
            _device: None,
            origins: HashMap::new(),
            listener: Listener { entity: 0, origin: [0.0; 3], left: [0.0, 1.0, 0.0] },
            effects_volume: 0.5,
            voice_volume: 1.0,
            separation: 0.5,
            rate: 1.0,
            output_channels: 2,
            output_sample_rate: 48_000,
            output_sample_format: "offline f32".into(),
            output_buffer_size: "offline".into(),
            overload_samples: Arc::new(AtomicU64::new(0)),
            peak_bits: Arc::new(AtomicU32::new(0.0_f32.to_bits())),
            largest_gain_step: 0.0,
            steam_audio_enabled: false,
            steam_audio_acoustic_mesh: None,
            steam_audio_bake: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn offline() -> (Self, rodio::mixer::MixerSource) {
        let (mixer, output) = rodio::mixer::mixer(rodio::nz!(2), rodio::nz!(48000));
        (Self::from_mixer(mixer), output)
    }

    pub fn info(&self) -> AudioInfo {
        AudioInfo {
            channels: self.output_channels,
            sample_rate: self.output_sample_rate,
            sample_format: self.output_sample_format.clone(),
            buffer_size: self.output_buffer_size.clone(),
            active_voices: self.voices.len(),
            active_loops: self.loop_voices.len(),
            loop_requests: self.loops.len(),
            overload_samples: self.overload_samples.load(Ordering::Relaxed),
            peak_before_limiter: f32::from_bits(self.peak_bits.load(Ordering::Relaxed)),
            largest_gain_step: self.largest_gain_step,
            steam_audio_enabled: self.steam_audio_enabled,
            steam_audio_scene_triangles: self
                .steam_audio_acoustic_mesh
                .as_ref()
                .map_or(0, |mesh| mesh.triangles.len()),
            steam_audio_bake_ready: self.steam_audio_bake.is_some(),
            steam_audio_probe_count: self
                .steam_audio_bake
                .as_ref()
                .map_or(0, |bake| bake.probe_count),
            steam_audio_bake_bytes: self
                .steam_audio_bake
                .as_ref()
                .map_or(0, |bake| bake.serialized_bytes()),
            steam_audio_bake_cache_hit: self
                .steam_audio_bake
                .as_ref()
                .is_some_and(|bake| bake.cache_hit),
            steam_audio_runtime_validated: self
                .steam_audio_bake
                .as_ref()
                .is_some_and(|bake| bake.runtime_validated),
            steam_audio_dsp_active: false,
        }
    }

    pub fn set_steam_audio_enabled(&mut self, enabled: bool) {
        self.steam_audio_enabled = enabled;
        if !enabled {
            self.steam_audio_acoustic_mesh = None;
            self.steam_audio_bake = None;
        }
    }

    pub fn set_steam_audio_map(
        &mut self,
        mesh: Option<Arc<AcousticMesh>>,
        bake: Option<Arc<SteamAudioBakeData>>,
    ) {
        if self.steam_audio_enabled {
            self.steam_audio_acoustic_mesh = mesh;
            self.steam_audio_bake = bake;
            if let Some(bake) = &self.steam_audio_bake {
                println!(
                    "STEAM AUDIO RUNTIME ASSETS: attached {} probes / {:.2} MiB / {}; audible DSP pending",
                    bake.probe_count,
                    bake.serialized_bytes() as f64 / (1024.0 * 1024.0),
                    if bake.runtime_validated { "validated" } else { "NOT validated" },
                );
            }
        } else {
            self.steam_audio_acoustic_mesh = None;
            self.steam_audio_bake = None;
        }
    }

    pub fn frame(&mut self, listener: Listener, origins: impl Iterator<Item = (u16, [f32; 3])>) {
        self.listener = listener;
        self.origins.clear();
        self.origins.extend(origins);
        self.voices.retain(|voice| !voice.player.empty());
        for voice in &mut self.voices {
            if let SoundOrigin::Entity(ref mut origin) = voice.request.origin {
                if let Some(current) = self.origins.get(&voice.request.entity) { *origin = *current; }
            }
        }
        self.refresh_voice_gains();
    }

    pub fn set_mix(&mut self, effects_volume: f32, voice_volume: f32, separation: f32) {
        self.effects_volume = effects_volume.clamp(0.0, 1.0);
        self.voice_volume = voice_volume.clamp(0.0, 1.0);
        self.separation = separation.clamp(0.0, 1.0);
        self.refresh_voice_gains();
    }

    fn refresh_voice_gains(&mut self) {
        let listener = self.listener;
        let effects = self.effects_volume;
        let voice = self.voice_volume;
        let separation = self.separation;
        for item in &self.voices {
            let next = sound_gains(&item.request, listener, effects, voice, separation);
            for (previous, next) in item.gains.get().into_iter().zip(next) {
                self.largest_gain_step = self.largest_gain_step.max((previous - next).abs());
            }
            item.gains.set(next);
        }
        self.refresh_loop_gains();
    }

    /// OpenJK S_AddLoopSounds: spatialize every loop request as CHAN_AUTO
    /// with full master volume, sum per sfx, clamp to SOUND_MAXVOL, then the
    /// paint pass applies s_volume. The listener-owned 0.75 scale of
    /// S_StartSound does not apply to loops.
    fn refresh_loop_gains(&mut self) {
        let mut totals: HashMap<&str, [f32; 2]> = HashMap::with_capacity(self.loop_voices.len());
        for request in &self.loops {
            let origin = match request.origin {
                SoundOrigin::Local => self.listener.origin,
                SoundOrigin::Entity(origin) | SoundOrigin::Fixed(origin) => origin,
            };
            let gains = spatialize(origin, self.listener, CHAN_AUTO, self.separation);
            let total = totals.entry(request.qpath.as_str()).or_insert([0.0; 2]);
            total[0] += gains[0];
            total[1] += gains[1];
        }
        for (qpath, voice) in &self.loop_voices {
            let total = totals.get(qpath.as_str()).copied().unwrap_or([0.0; 2]);
            let next = total.map(|gain| gain.min(1.0) * self.effects_volume);
            for (previous, next) in voice.gains.get().into_iter().zip(next) {
                self.largest_gain_step = self.largest_gain_step.max((previous - next).abs());
            }
            voice.gains.set(next);
        }
    }

    /// Replace this frame's loop set (CGame's S_ClearLoopingSounds followed by
    /// its S_AddLoopingSound calls). Each request carries its registered sfx.
    pub fn set_loops(&mut self, requests: Vec<(LoopRequest, Arc<RegisteredSound>)>) {
        self.loops.clear();
        let mut wanted: HashMap<String, Arc<RegisteredSound>> = HashMap::new();
        for (request, sound) in requests {
            wanted.entry(request.qpath.clone()).or_insert(sound);
            self.loops.push(request);
        }
        let stale: Vec<String> = self
            .loop_voices
            .keys()
            .filter(|qpath| !wanted.contains_key(*qpath))
            .cloned()
            .collect();
        for qpath in stale {
            if let Some(voice) = self.loop_voices.remove(&qpath) {
                self.retire(voice.player, &voice.gains);
            }
        }
        let elapsed = self.loop_epoch.elapsed().as_secs_f64();
        for (qpath, sound) in wanted {
            if self.loop_voices.contains_key(&qpath) || self.loop_voices.len() >= MAX_LOOP_VOICES {
                continue;
            }
            let phase = (elapsed * f64::from(sound.sample_rate.get())) as usize;
            let gains = Arc::new(Gains::new([0.0; 2]));
            let player = connect_player(&self.mixer, StereoSource::looping(sound, Arc::clone(&gains), phase));
            if self.rate == 0.0 { player.pause(); }
            self.loop_voices.insert(qpath, LoopVoice { player, gains });
        }
        self.refresh_loop_gains();
    }

    /// Stop a voice without a hard waveform cut. A paused Rodio player emits
    /// silence forever, so a paused voice is stopped outright (it is silent).
    fn retire(&self, player: Player, gains: &Gains) {
        if self.rate == 0.0 {
            player.stop();
        } else {
            gains.stopping.store(true, Ordering::Relaxed);
            player.detach(); // the source ends itself after the fade
        }
    }

    pub fn set_rate(&mut self, rate: f32) {
        self.rate = rate;
        for voice in &self.voices {
            if rate == 0.0 { voice.player.pause(); }
            else { voice.player.set_speed(rate); voice.player.play(); }
        }
        // OpenJK loops follow the shared paint clock rather than the demo
        // timescale, so they only pause; they are never pitch-shifted.
        for voice in self.loop_voices.values() {
            if rate == 0.0 { voice.player.pause(); } else { voice.player.play(); }
        }
    }

    pub fn play(&mut self, mut request: SoundRequest, sound: Arc<RegisteredSound>) {
        self.voices.retain(|voice| !voice.player.empty());
        if !matches!(request.channel, 0 | 10) { // CHAN_AUTO / CHAN_LESS_ATTEN auto-pick
            let (stomped, kept): (Vec<_>, Vec<_>) =
                std::mem::take(&mut self.voices).into_iter().partition(|voice| {
                    voice.request.entity == request.entity
                        && channels_stomp(voice.request.channel, request.channel)
                });
            self.voices = kept;
            for voice in stomped { self.retire(voice.player, &voice.gains); }
        }
        if self.voices.len() >= MAX_VOICES {
            // Match the software mixer's S_PickChannel preference as far as the
            // currently implemented one-shot channels allow: a remote sound
            // should kick the oldest remote sound before it kicks one belonging
            // to the listener. (Tracked/looping channels will add the other
            // OpenJK protection rule when that presenter lands.)
            let victim = if request.entity != self.listener.entity {
                self.voices
                    .iter()
                    .position(|voice| voice.request.entity != self.listener.entity)
                    .unwrap_or(0)
            } else {
                0
            };
            let voice = self.voices.remove(victim);
            self.retire(voice.player, &voice.gains);
        }
        if let SoundOrigin::Entity(ref mut origin) = request.origin {
            if let Some(current) = self.origins.get(&request.entity) { *origin = *current; }
        }
        let gains = Arc::new(Gains::new(sound_gains(
            &request,
            self.listener,
            self.effects_volume,
            self.voice_volume,
            self.separation,
        )));
        let player = connect_player(&self.mixer, StereoSource::new(sound, Arc::clone(&gains)));
        if self.rate == 0.0 { player.pause(); } else { player.set_speed(self.rate); }
        self.voices.push(Voice { player, gains, request });
    }

    pub fn clear(&mut self) {
        for voice in std::mem::take(&mut self.voices) { self.retire(voice.player, &voice.gains); }
        for (_, voice) in std::mem::take(&mut self.loop_voices) { self.retire(voice.player, &voice.gains); }
        self.loops.clear();
        self.origins.clear();
    }
}
impl Drop for AudioBackend { fn drop(&mut self) { self.clear(); } }

fn is_voice_channel(channel: i32) -> bool { matches!(channel, 3 | 4 | 12) }

/// OpenJK S_CheckChannelStomp: the three voice channels mutually replace one
/// another for the same entity, while all other non-auto channels only replace
/// the identical channel.
fn channels_stomp(a: i32, b: i32) -> bool {
    a == b || (is_voice_channel(a) && is_voice_channel(b))
}

/// OpenJK codemp/client/snd_dma.cpp S_SpatializeOrigin. Volume is applied
/// later in OpenJK's ChannelPaint; multiplying it here is algebraically the
/// same for our normalized floating-point samples.
fn sound_gains(
    request: &SoundRequest,
    listener: Listener,
    effects_volume: f32,
    voice_volume: f32,
    separation: f32,
) -> [f32; 2] {
    let volume = if is_voice_channel(request.channel) { voice_volume } else { effects_volume };
    if matches!(request.origin, SoundOrigin::Local) || request.entity == listener.entity {
        // S_StartSound scales listener-owned body/weapon/voice sounds to
        // SOUND_FMAXVOL (0.75) for channels below CHAN_AMBIENT.
        let local_scale = if request.channel < 7 { 0.75 } else { 1.0 };
        return [volume * local_scale; 2];
    }

    let origin = match request.origin {
        SoundOrigin::Entity(p) | SoundOrigin::Fixed(p) => p,
        SoundOrigin::Local => unreachable!(),
    };
    spatialize(origin, listener, request.channel, separation).map(|gain| gain * volume)
}

/// S_SpatializeOrigin with master_vol = 1: distance attenuation and the
/// s_separation stereo split for one source position.
fn spatialize(origin: [f32; 3], listener: Listener, channel: i32, separation: f32) -> [f32; 2] {
    let delta = std::array::from_fn::<_, 3, _>(|i| origin[i] - listener.origin[i]);
    let distance = delta.iter().map(|v| v * v).sum::<f32>().sqrt();
    let source_dir = if distance > 0.0 {
        delta.map(|component| component / distance)
    } else {
        [0.0; 3]
    };

    let (full_volume_distance, attenuation) = match channel {
        3 => (256.0 * 3.0, 0.0008),       // CHAN_VOICE
        4 => (256.0 * 1.35, 0.004),       // CHAN_VOICE_ATTEN
        10 => (256.0 * 8.0, 0.0008),      // CHAN_LESS_ATTEN
        12 => (f32::MAX, 0.0008),         // CHAN_VOICE_GLOBAL
        _ => (256.0, 0.0008),
    };
    let distance_loss = ((distance - full_volume_distance).max(0.0) * attenuation).max(0.0);
    let distance_gain = (1.0 - distance_loss).max(0.0);

    // OpenJK: dot = -DotProduct(listener_axis[1], source_vec). The App listener
    // stores that same left axis, and s_separation defaults to 0.5.
    let dot = -listener.left.iter().zip(source_dir).map(|(a, b)| a * b).sum::<f32>();
    let separation = separation.clamp(0.0, 1.0);
    let right_scale = (separation + (1.0 - separation) * dot).max(0.0);
    let left_scale = (separation - (1.0 - separation) * dot).max(0.0);
    [distance_gain * left_scale, distance_gain * right_scale]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Arc<RegisteredSound> {
        // A real PCM WAV exercises decoding without copyrighted game assets.
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF"); wav.extend_from_slice(&40u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt "); wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&48000u32.to_le_bytes()); wav.extend_from_slice(&96000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes()); wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data"); wav.extend_from_slice(&4u32.to_le_bytes());
        wav.extend_from_slice(&16384i16.to_le_bytes()); wav.extend_from_slice(&(-16384i16).to_le_bytes());
        Arc::new(decode_sound(wav).unwrap())
    }

    fn request(channel: i32) -> SoundRequest {
        SoundRequest { qpath: "test.wav".into(), entity: 2, channel, origin: SoundOrigin::Entity([0.0, -100.0, 0.0]) }
    }

    #[test]
    fn wav_decode_and_openjk_attenuation_pan_and_local_rules() {
        let sound = fixture();
        assert_eq!(&*sound.samples, &[0.5, -0.5]);
        let listener = Listener { entity: 0, origin: [0.0; 3], left: [0.0, 1.0, 0.0] };
        assert_eq!(sound_gains(&request(0), listener, 1.0, 1.0, 0.5), [0.0, 1.0]);
        let mut req = request(0);
        req.origin = SoundOrigin::Entity([0.0, 100.0, 0.0]);
        assert_eq!(sound_gains(&req, listener, 1.0, 1.0, 0.5), [1.0, 0.0]);
        req.origin = SoundOrigin::Entity([2000.0, 0.0, 0.0]);
        assert_eq!(sound_gains(&req, listener, 1.0, 1.0, 0.5), [0.0, 0.0]);
        req.origin = SoundOrigin::Local;
        assert_eq!(sound_gains(&req, listener, 0.5, 1.0, 0.5), [0.375, 0.375]);
        req.origin = SoundOrigin::Entity([2000.0, 0.0, 0.0]); req.entity = 0;
        assert_eq!(sound_gains(&req, listener, 0.5, 1.0, 0.5), [0.375, 0.375]);
        assert!(decode_sound(b"bad wav".to_vec()).is_err());
    }

    #[test]
    fn openjk_voice_volume_separation_and_channel_stomp_rules() {
        let listener = Listener { entity: 0, origin: [0.0; 3], left: [0.0, 1.0, 0.0] };
        let mut voice = request(3);
        voice.origin = SoundOrigin::Local;
        assert_eq!(sound_gains(&voice, listener, 0.1, 0.8, 0.5), [0.6, 0.6]);

        let mut menu = request(11);
        menu.origin = SoundOrigin::Local;
        assert_eq!(sound_gains(&menu, listener, 0.5, 0.8, 0.5), [0.5, 0.5]);

        let positional = request(0);
        assert_eq!(sound_gains(&positional, listener, 1.0, 1.0, 1.0), [1.0, 1.0]);
        assert!(channels_stomp(3, 4));
        assert!(channels_stomp(4, 12));
        assert!(channels_stomp(2, 2));
        assert!(!channels_stomp(2, 3));
    }

    #[test]
    fn offline_mixer_outputs_pcm_and_tracks_entity_movement() {
        let (mut backend, mut output) = AudioBackend::offline();
        backend.set_mix(1.0, 1.0, 0.5);
        backend.play(request(0), fixture());
        let mixed: Vec<_> = output.by_ref().take(4).collect();
        assert_eq!(mixed, vec![0.0, 0.5, 0.0, -0.5]);
        backend.play(request(0), fixture());
        backend.frame(backend.listener, [(2, [0.0, 100.0, 0.0])].into_iter());
        assert_eq!(backend.voices.last().unwrap().gains.get(), [1.0, 0.0]);
        assert_eq!(backend.info().largest_gain_step, 1.0);
    }

    #[test]
    fn channels_pause_speed_and_stop_have_explicit_lifetimes() {
        let (mut backend, _output) = AudioBackend::offline();
        backend.play(request(2), fixture());
        backend.play(request(2), fixture());
        assert_eq!(backend.voices.len(), 1);
        backend.play(request(0), fixture()); backend.play(request(0), fixture());
        assert_eq!(backend.voices.len(), 3);
        backend.play(request(10), fixture()); backend.play(request(10), fixture());
        assert_eq!(backend.voices.len(), 5);
        backend.set_rate(0.0);
        assert!(backend.voices.iter().all(|voice| voice.player.is_paused()));
        backend.set_rate(2.0);
        assert!(backend.voices.iter().all(|voice| !voice.player.is_paused() && voice.player.speed() == 2.0));
        backend.clear();
        assert!(backend.voices.is_empty());
    }

    #[test]
    fn hard_panned_voice_onset_is_not_read_as_mono_by_the_mixer() {
        // Regression: connect-then-append let the mixer size its converter from
        // the empty queue (mono), smearing the first 512 samples across L/R.
        let (mut backend, mut output) = AudioBackend::offline();
        backend.set_mix(1.0, 1.0, 0.5);
        let sound = Arc::new(RegisteredSound { samples: vec![0.25; 2048].into(), sample_rate: rodio::nz!(48000) });
        backend.play(request(0), sound); // entity at -Y: fully right at separation 0.5
        let mixed: Vec<f32> = output.by_ref().take(2048).collect();
        assert!(mixed.chunks(2).all(|frame| frame == [0.0, 0.25]), "onset leaked into left channel");
    }

    #[test]
    fn loops_merge_per_sfx_share_a_cycle_and_fade_when_dropped() {
        let (mut backend, _output) = AudioBackend::offline();
        backend.set_mix(0.5, 1.0, 0.5);
        let hum = Arc::new(RegisteredSound { samples: vec![0.1; 4410].into(), sample_rate: rodio::nz!(44100) });
        let at = |qpath: &str, origin| (LoopRequest { qpath: qpath.into(), origin }, Arc::clone(&hum));
        // Two sabers humming at the listener plus one hard right: one channel.
        backend.set_loops(vec![
            at("hum.wav", SoundOrigin::Local),
            at("hum.wav", SoundOrigin::Local),
            at("hum.wav", SoundOrigin::Fixed([0.0, -100.0, 0.0])),
            at("mover.wav", SoundOrigin::Fixed([0.0, 100.0, 0.0])),
        ]);
        assert_eq!(backend.info().active_loops, 2);
        assert_eq!(backend.info().loop_requests, 4);
        // Local: dot 0 -> 0.5 per ear each; right source adds 1.0 right.
        // Sums clamp at SOUND_MAXVOL before s_volume (0.5) is applied, and
        // the listener-owned 0.75 one-shot scale does not apply.
        assert_eq!(backend.loop_voices["hum.wav"].gains.get(), [0.5, 0.5]);
        assert_eq!(backend.loop_voices["mover.wav"].gains.get(), [0.5, 0.0]);

        let dropped = Arc::clone(&backend.loop_voices["mover.wav"].gains);
        backend.set_loops(vec![at("hum.wav", SoundOrigin::Local)]);
        assert_eq!(backend.info().active_loops, 1);
        assert!(dropped.stopping.load(Ordering::Relaxed), "dropped loop must fade, not cut");
        assert_eq!(backend.loop_voices["hum.wav"].gains.get(), [0.25, 0.25]);

        backend.clear();
        assert_eq!(backend.info().active_loops, 0);
    }

    #[test]
    fn looping_source_wraps_on_frame_boundaries_and_glides_in() {
        let sound = Arc::new(RegisteredSound { samples: vec![1.0, 0.5, 0.25].into(), sample_rate: rodio::nz!(48000) });
        let gains = Arc::new(Gains::new([1.0, 1.0]));
        let mut source = StereoSource::looping(Arc::clone(&sound), Arc::clone(&gains), 4); // 4 % 3 = frame 1
        assert_eq!(source.current_span_len(), None);
        assert_eq!(source.total_duration(), None);
        let out: Vec<f32> = source.by_ref().take(2 * 3000).collect();
        assert!(out[0] > 0.0 && out[0] < 0.01, "loop must glide in, got {}", out[0]);
        // Stays stereo-aligned across many wraps: L == R every frame.
        assert!(out.chunks(2).all(|frame| frame[0] == frame[1]));
        assert!(source.next().is_some(), "a loop never ends on its own");
    }

    #[test]
    fn gain_changes_glide_and_stops_fade_instead_of_stepping() {
        let sound = Arc::new(RegisteredSound { samples: vec![1.0; 4800].into(), sample_rate: rodio::nz!(48000) });
        let gains = Arc::new(Gains::new([1.0, 1.0]));
        let mut source = StereoSource::new(Arc::clone(&sound), Arc::clone(&gains));
        assert_eq!(source.by_ref().take(2).collect::<Vec<_>>(), vec![1.0, 1.0]);

        // A hard pan (one frame's target step of 1.0) must ramp, not jump.
        gains.set([0.0, 1.0]);
        let left: Vec<f32> = source.by_ref().take(4800).step_by(2).collect();
        assert!(left[0] > 0.99, "first sample after a step moved too far: {}", left[0]);
        assert!(left.windows(2).all(|w| w[1] <= w[0] && w[0] - w[1] < 0.01));
        assert!(*left.last().unwrap() < 0.02, "glide never approached target");

        // A stomp/steal fades out within STOP_FADE_SECONDS and then ends.
        gains.stopping.store(true, Ordering::Relaxed);
        let tail: Vec<f32> = source.by_ref().skip(1).step_by(2).collect();
        assert!(tail.len() <= (STOP_FADE_SECONDS * 48000.0) as usize + 1);
        assert!(tail.windows(2).all(|w| w[1] <= w[0]));
        assert_eq!(source.current_span_len(), Some(0));
    }

    #[test]
    #[ignore = "requires JKA_TEST_BASE with stock WAV/MP3 assets"]
    fn stock_wav_mp3_cache_and_mixer() {
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let mut cache = SoundAssets::new(AssetSearchPath::open(std::path::Path::new(&base)).unwrap());
        let wav = cache.register("sound/weapons/saber/saberon.wav").unwrap();
        let again = cache.register("sound/weapons/saber/saberon.wav").unwrap();
        assert!(Arc::ptr_eq(&wav, &again));
        let mp3 = cache.register("sound/weapons/force/jump.mp3").unwrap();
        assert!(!mp3.samples.is_empty());
        let (mut backend, mut output) = AudioBackend::offline();
        backend.play(SoundRequest { origin: SoundOrigin::Local, ..request(0) }, mp3);
        assert!(output.by_ref().take(48000).any(|sample| sample.abs() > 0.0001));
    }

    #[test]
    #[ignore = "opens the default audio device and plays 200ms of quiet stock SFX; needs JKA_TEST_BASE"]
    fn windows_audio_device_smoke() {
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let mut cache = SoundAssets::new(AssetSearchPath::open(std::path::Path::new(&base)).unwrap());
        let sound = cache.register("sound/weapons/force/jump.mp3").unwrap();
        let mut backend = AudioBackend::open(false).expect("audio output device");
        backend.set_mix(0.02, 0.02, 0.5);
        backend.play(SoundRequest { origin: SoundOrigin::Local, ..request(0) }, sound);
        std::thread::sleep(Duration::from_millis(200));
        backend.clear();
    }
}
