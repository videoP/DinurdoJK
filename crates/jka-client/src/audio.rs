//! Native audio device/mixer and registered SFX cache. No CGame event IDs here.
use std::{
    collections::{HashMap, HashSet},
    io::Cursor,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc::{self, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use audionimbus::{
    AudioBufferMut, AudioBufferRef, AudioEffectState, AudioSettings as SteamAudioSignalSettings,
    BinauralEffect, BinauralEffectParams, BinauralEffectSettings, Context as SteamAudioContext,
    CoordinateSystem, DefaultRayTracer, Direct, DirectEffect, DirectEffectParams,
    DirectEffectSettings, DirectSimulationParameters, DirectSimulationSettings, Direction, Equalizer,
    Hrtf, HrtfInterpolation, HrtfSettings, Occlusion, OcclusionAlgorithm, Scene, SerializedObject,
    SimulationInputs, SimulationParameters, SimulationSettings, SimulationSharedInputs, Simulator,
    Source as SteamAudioSimulationSource, Transmission, TransmissionParameters,
};
use jka_assets::pk3::AssetSearchPath;
use jka_assets::bsp::AcousticMesh;
use crate::steam_audio::{build_scene_from_acoustic_mesh, jka_to_steam_point, SteamAudioBakeData};
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
// 256 samples is ~5.3 ms at 48 kHz: short enough for responsive head rotation,
// while still giving Steam Audio a fixed block size as required by its HRTF DSP.
const STEAM_AUDIO_HRTF_FRAME_SIZE: u32 = 256;
// Direct occlusion/transmission is a scene query, not audio-rate DSP. Keep it
// on its own thread at a bounded update rate; the audio callback only reads the
// most recent atomically published result.
const STEAM_AUDIO_DIRECT_HZ: u64 = 30;
const STEAM_AUDIO_DIRECT_MIN_INTERVAL: Duration = Duration::from_micros(1_000_000 / STEAM_AUDIO_DIRECT_HZ);
// AudioNimbus retains Steam Audio's historical `num_transmission_rays` field
// name, but the value is the maximum number of occluding surfaces whose
// material transmission coefficients are accumulated along the direct path.
const STEAM_AUDIO_TRANSMISSION_RAYS: u32 = 8;

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
    /// JKA-world unit vectors for the final audible camera orientation.
    pub ahead: [f32; 3],
    pub up: [f32; 3],
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
struct Gains {
    target: [AtomicU32; 2],
    mono_target: AtomicU32,
    direction: [AtomicU32; 3],
    hrtf_spatializable: AtomicBool,
    /// World-space JKA source position used by the dedicated Steam Audio
    /// direct-simulation thread. Audio callbacks never touch scene queries.
    environment_source: [AtomicU32; 3],
    environment_source_valid: AtomicBool,
    /// Most recently published Steam Audio direct-path result. These atomics
    /// are read at the next fixed audio block without taking a mutex.
    environment_occlusion: AtomicU32,
    environment_transmission: [AtomicU32; 3],
    /// 0 = none, 1 = frequency independent, 2 = frequency dependent.
    environment_transmission_mode: AtomicU32,
    environment_result_valid: AtomicBool,
    stopping: AtomicBool,
}
impl Gains {
    fn new(value: [f32; 2]) -> Self {
        Self {
            target: value.map(|v| AtomicU32::new(v.to_bits())),
            mono_target: AtomicU32::new(value[0].max(value[1]).to_bits()),
            // Steam Audio listener-local convention: +X right, +Y up, +Z behind.
            // A centered/non-spatial source defaults to directly ahead (-Z).
            direction: [0.0_f32, 0.0, -1.0].map(|v| AtomicU32::new(v.to_bits())),
            hrtf_spatializable: AtomicBool::new(false),
            environment_source: [0.0_f32; 3].map(|v| AtomicU32::new(v.to_bits())),
            environment_source_valid: AtomicBool::new(false),
            environment_occlusion: AtomicU32::new(1.0_f32.to_bits()),
            environment_transmission: [1.0_f32; 3].map(|v| AtomicU32::new(v.to_bits())),
            environment_transmission_mode: AtomicU32::new(0),
            environment_result_valid: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
        }
    }
    fn get(&self) -> [f32; 2] {
        std::array::from_fn(|i| f32::from_bits(self.target[i].load(Ordering::Relaxed)))
    }
    fn set(&self, value: [f32; 2]) {
        for (gain, value) in self.target.iter().zip(value) { gain.store(value.to_bits(), Ordering::Relaxed); }
    }
    fn set_hrtf(&self, mono_gain: f32, direction: [f32; 3], spatializable: bool) {
        self.mono_target.store(mono_gain.to_bits(), Ordering::Relaxed);
        for (slot, value) in self.direction.iter().zip(direction) {
            slot.store(value.to_bits(), Ordering::Relaxed);
        }
        self.hrtf_spatializable.store(spatializable, Ordering::Relaxed);
    }
    fn hrtf(&self) -> (f32, [f32; 3], bool) {
        (
            f32::from_bits(self.mono_target.load(Ordering::Relaxed)),
            std::array::from_fn(|i| f32::from_bits(self.direction[i].load(Ordering::Relaxed))),
            self.hrtf_spatializable.load(Ordering::Relaxed),
        )
    }
    fn set_environment_source(&self, origin: [f32; 3], valid: bool) {
        for (slot, value) in self.environment_source.iter().zip(origin) {
            slot.store(value.to_bits(), Ordering::Relaxed);
        }
        self.environment_source_valid.store(valid, Ordering::Relaxed);
        if !valid {
            self.reset_environment_result();
        }
    }
    fn environment_source(&self) -> Option<[f32; 3]> {
        self.environment_source_valid
            .load(Ordering::Relaxed)
            .then(|| std::array::from_fn(|i| {
                f32::from_bits(self.environment_source[i].load(Ordering::Relaxed))
            }))
    }
    fn set_environment_result(
        &self,
        occlusion: f32,
        transmission_mode: u32,
        transmission: [f32; 3],
    ) {
        let finite_unit = |value: f32| if value.is_finite() { value.clamp(0.0, 1.0) } else { 1.0 };
        self.environment_occlusion
            .store(finite_unit(occlusion).to_bits(), Ordering::Relaxed);
        for (slot, value) in self.environment_transmission.iter().zip(transmission) {
            slot.store(finite_unit(value).to_bits(), Ordering::Relaxed);
        }
        self.environment_transmission_mode
            .store(transmission_mode.min(2), Ordering::Relaxed);
        self.environment_result_valid.store(true, Ordering::Release);
    }
    fn reset_environment_result(&self) {
        self.environment_result_valid.store(false, Ordering::Release);
        self.environment_occlusion.store(1.0_f32.to_bits(), Ordering::Relaxed);
        for slot in &self.environment_transmission {
            slot.store(1.0_f32.to_bits(), Ordering::Relaxed);
        }
        self.environment_transmission_mode.store(0, Ordering::Relaxed);
    }
    fn environment_params(&self) -> Option<DirectEffectParams> {
        if !self.environment_result_valid.load(Ordering::Acquire) {
            return None;
        }
        let transmission_values = Equalizer(std::array::from_fn(|i| {
            f32::from_bits(self.environment_transmission[i].load(Ordering::Relaxed))
        }));
        let transmission = match self.environment_transmission_mode.load(Ordering::Relaxed) {
            1 => Some(Transmission::FrequencyIndependent(transmission_values)),
            2 => Some(Transmission::FrequencyDependent(transmission_values)),
            _ => None,
        };
        Some(DirectEffectParams {
            // OpenJK's established distance/channel attenuation remains the
            // gameplay authority. Steam Audio contributes only wall effects.
            distance_attenuation: None,
            air_absorption: None,
            directivity: None,
            occlusion: Some(f32::from_bits(
                self.environment_occlusion.load(Ordering::Relaxed),
            )),
            transmission,
        })
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

/// Shared Steam Audio signal-processing objects. The default HRTF and the
/// direct-path DSP both use one fixed block size at the physical device rate.
/// Everything here is created on the game thread; Rodio's callback only calls
/// already-created effects and reads atomically published simulation results.
struct SteamAudioSignalRuntime {
    context: SteamAudioContext,
    settings: SteamAudioSignalSettings,
    hrtf: Option<Hrtf>,
    hrtf_error: Option<String>,
}
impl SteamAudioSignalRuntime {
    fn new(sample_rate: u32) -> Self {
        let context = SteamAudioContext::default();
        let settings = SteamAudioSignalSettings {
            sampling_rate: sample_rate,
            frame_size: STEAM_AUDIO_HRTF_FRAME_SIZE,
        };
        let (hrtf, hrtf_error) = match Hrtf::try_new(&context, &settings, &HrtfSettings::default()) {
            Ok(hrtf) => (Some(hrtf), None),
            Err(error) => (
                None,
                Some(format!("could not create Steam Audio HRTF: {error}")),
            ),
        };
        Self { context, settings, hrtf, hrtf_error }
    }

    fn new_binaural_effect(&self) -> Result<Option<BinauralEffect>, String> {
        let Some(hrtf) = self.hrtf.as_ref() else { return Ok(None); };
        BinauralEffect::try_new(
            &self.context,
            &self.settings,
            &BinauralEffectSettings { hrtf: hrtf.clone() },
        )
        .map(Some)
        .map_err(|error| format!("could not create Steam Audio binaural effect: {error}"))
    }

    fn new_direct_effect(&self) -> Result<DirectEffect, String> {
        DirectEffect::try_new(
            &self.context,
            &self.settings,
            &DirectEffectSettings { num_channels: 1 },
        )
        .map_err(|error| format!("could not create Steam Audio direct effect: {error}"))
    }
}

#[derive(Clone)]
struct EnvironmentalSourceFrame {
    id: u64,
    origin: [f32; 3],
    gains: Arc<Gains>,
}

struct EnvironmentalFrame {
    listener: [f32; 3],
    sources: Vec<EnvironmentalSourceFrame>,
}

/// Direct occlusion/transmission scene queries are deliberately kept off both
/// the main thread and the real-time audio callback. The game thread publishes
/// at most one latest frame into this single-slot queue; the worker owns the
/// Steam Audio simulator and atomically publishes its newest per-source result.
struct SteamAudioEnvironmentalRuntime {
    sender: SyncSender<EnvironmentalFrame>,
    ready: Arc<AtomicBool>,
    update_count: Arc<AtomicU64>,
    source_count: Arc<AtomicU32>,
    last_micros: Arc<AtomicU64>,
    error: Arc<Mutex<Option<String>>>,
}
impl SteamAudioEnvironmentalRuntime {
    fn try_start(
        mesh: Arc<AcousticMesh>,
        bake: Option<Arc<SteamAudioBakeData>>,
        sample_rate: u32,
    ) -> Result<Self, String> {
        let (sender, receiver) = mpsc::sync_channel::<EnvironmentalFrame>(1);
        let ready = Arc::new(AtomicBool::new(false));
        let update_count = Arc::new(AtomicU64::new(0));
        let source_count = Arc::new(AtomicU32::new(0));
        let last_micros = Arc::new(AtomicU64::new(0));
        let error = Arc::new(Mutex::new(None));

        let thread_ready = Arc::clone(&ready);
        let thread_updates = Arc::clone(&update_count);
        let thread_source_count = Arc::clone(&source_count);
        let thread_last_micros = Arc::clone(&last_micros);
        let thread_error = Arc::clone(&error);
        thread::Builder::new()
            .name("steam-audio-direct".into())
            .spawn(move || {
                let result = run_steam_audio_environment_thread(
                    mesh,
                    bake,
                    sample_rate,
                    receiver,
                    &thread_ready,
                    &thread_updates,
                    &thread_source_count,
                    &thread_last_micros,
                );
                thread_ready.store(false, Ordering::Release);
                thread_source_count.store(0, Ordering::Relaxed);
                if let Err(message) = result {
                    eprintln!("STEAM AUDIO DIRECT SIMULATION STOPPED: {message}");
                    if let Ok(mut slot) = thread_error.lock() { *slot = Some(message); }
                }
            })
            .map_err(|error| format!("could not start Steam Audio direct simulation thread: {error}"))?;

        Ok(Self { sender, ready, update_count, source_count, last_micros, error })
    }

    fn submit(&self, frame: EnvironmentalFrame) {
        match self.sender.try_send(frame) {
            Ok(()) | Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {
                if let Ok(mut slot) = self.error.lock() {
                    if slot.is_none() {
                        *slot = Some("Steam Audio direct simulation thread disconnected".into());
                    }
                }
            }
        }
    }

    fn ready(&self) -> bool { self.ready.load(Ordering::Acquire) }
    fn update_count(&self) -> u64 { self.update_count.load(Ordering::Relaxed) }
    fn source_count(&self) -> usize { self.source_count.load(Ordering::Relaxed) as usize }
    fn last_ms(&self) -> f32 { self.last_micros.load(Ordering::Relaxed) as f32 / 1000.0 }
    fn error(&self) -> Option<String> { self.error.lock().ok().and_then(|slot| slot.clone()) }
}

fn run_steam_audio_environment_thread(
    mesh: Arc<AcousticMesh>,
    bake: Option<Arc<SteamAudioBakeData>>,
    sample_rate: u32,
    receiver: mpsc::Receiver<EnvironmentalFrame>,
    ready: &AtomicBool,
    update_count: &AtomicU64,
    source_count: &AtomicU32,
    last_micros: &AtomicU64,
) -> Result<(), String> {
    let context = SteamAudioContext::default();
    let (scene, scene_source) = if let Some(bake) = bake.filter(|bake| bake.runtime_validated) {
        let serialized_scene = SerializedObject::try_with_buffer(&context, bake.scene_bytes.to_vec())
            .map_err(|error| format!("could not wrap baked Steam Audio scene: {error}"))?;
        let scene = Scene::<DefaultRayTracer>::load(&context, &serialized_scene)
            .map_err(|error| format!("could not deserialize baked Steam Audio scene: {error}"))?;
        (scene, "serialized bake/cache scene")
    } else {
        (
            build_scene_from_acoustic_mesh(&context, &mesh)?,
            "prepared BSP acoustic mesh",
        )
    };
    let audio_settings = SteamAudioSignalSettings {
        sampling_rate: sample_rate,
        frame_size: STEAM_AUDIO_HRTF_FRAME_SIZE,
    };
    let simulation_settings = SimulationSettings::new(&audio_settings).with_direct(
        DirectSimulationSettings { max_num_occlusion_samples: 1 },
    );
    let mut simulator = Simulator::try_new(&context, &simulation_settings)
        .map_err(|error| format!("could not create direct simulator: {error}"))?;
    simulator.set_scene(&scene);
    simulator.commit();

    let mut sources: HashMap<u64, SteamAudioSimulationSource<Direct>> = HashMap::new();
    ready.store(true, Ordering::Release);
    println!(
        "STEAM AUDIO DIRECT SIM: ready from {scene_source}: {} acoustic tris; {} Hz; raycast occlusion + material transmission (up to {} occluding surfaces); {} Hz updates",
        mesh.triangles.len(),
        sample_rate,
        STEAM_AUDIO_TRANSMISSION_RAYS,
        STEAM_AUDIO_DIRECT_HZ,
    );

    while let Ok(mut frame) = receiver.recv() {
        // If the game thread got ahead while we were tracing, discard stale
        // positions and process only the newest available frame.
        while let Ok(newer) = receiver.try_recv() { frame = newer; }
        let started = Instant::now();
        let wanted: HashSet<u64> = frame.sources.iter().map(|source| source.id).collect();
        let stale: Vec<u64> = sources.keys().copied().filter(|id| !wanted.contains(id)).collect();
        let mut membership_changed = false;
        for id in stale {
            if let Some(source) = sources.remove(&id) {
                simulator.remove_source(&source);
                membership_changed = true;
            }
        }
        for source in &frame.sources {
            if !sources.contains_key(&source.id) {
                let sim_source = SteamAudioSimulationSource::<Direct>::try_new(&simulator)
                    .map_err(|error| format!("could not create direct source: {error}"))?;
                simulator.add_source(&sim_source);
                sources.insert(source.id, sim_source);
                membership_changed = true;
            }
        }
        if membership_changed { simulator.commit(); }

        let shared = SimulationSharedInputs::new(CoordinateSystem {
            origin: jka_to_steam_point(frame.listener),
            ..CoordinateSystem::default()
        });
        simulator
            .set_shared_direct_inputs(&shared)
            .map_err(|error| format!("invalid direct listener inputs: {error}"))?;

        let mut configured_sources = HashSet::with_capacity(frame.sources.len());
        for source in &frame.sources {
            let Some(sim_source) = sources.get(&source.id) else { continue; };
            let inputs = SimulationInputs {
                source: CoordinateSystem {
                    origin: jka_to_steam_point(source.origin),
                    ..CoordinateSystem::default()
                },
                parameters: SimulationParameters::new().with_direct(
                    DirectSimulationParameters::new().with_occlusion(
                        Occlusion::new(OcclusionAlgorithm::Raycast).with_transmission(
                            TransmissionParameters {
                                num_transmission_rays: STEAM_AUDIO_TRANSMISSION_RAYS,
                            },
                        ),
                    ),
                ),
            };
            match sim_source.set_direct_inputs(&inputs) {
                Ok(()) => { configured_sources.insert(source.id); }
                Err(error) => {
                    source.gains.reset_environment_result();
                    eprintln!("STEAM AUDIO DIRECT INPUT WARNING: {error}");
                }
            }
        }

        simulator.run_direct();
        for source in &frame.sources {
            if !configured_sources.contains(&source.id) { continue; }
            let Some(sim_source) = sources.get(&source.id) else { continue; };
            match sim_source.get_direct_outputs() {
                Ok(params) => {
                    let occlusion = params.occlusion.unwrap_or(1.0);
                    let (mode, transmission) = match params.transmission {
                        Some(Transmission::FrequencyIndependent(values)) => (1, values.0),
                        Some(Transmission::FrequencyDependent(values)) => (2, values.0),
                        None => (0, [1.0; 3]),
                    };
                    source.gains.set_environment_result(occlusion, mode, transmission);
                }
                Err(error) => {
                    source.gains.reset_environment_result();
                    eprintln!("STEAM AUDIO DIRECT OUTPUT WARNING: {error}");
                }
            }
        }

        source_count.store(frame.sources.len().min(u32::MAX as usize) as u32, Ordering::Relaxed);
        update_count.fetch_add(1, Ordering::Relaxed);
        last_micros.store(
            started.elapsed().as_micros().min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
    }
    Ok(())
}

/// Fixed-rate source used for every positional Steam-Audio-capable voice while
/// the master gate is enabled. It can independently A/B direct environmental
/// filtering and HRTF at block boundaries, so neither sub-feature needs a map
/// reload or a voice rebuild.
struct SteamAudioSource {
    sound: Arc<RegisteredSound>,
    gains: Arc<Gains>,
    runtime: Arc<SteamAudioSignalRuntime>,
    binaural_effect: Option<BinauralEffect>,
    direct_effect: DirectEffect,
    binaural_enabled: Arc<AtomicBool>,
    environmental_enabled: Arc<AtomicBool>,
    output_rate: rodio::SampleRate,
    source_position: f64,
    current_stereo: [f32; 2],
    current_mono: f32,
    glide: f32,
    fade: f32,
    fade_step: f32,
    looping: bool,
    input_done: bool,
    finished: bool,
    last_block_hrtf: bool,
    direct_was_active: bool,
    hrtf_was_active: bool,
    finish_after_block: bool,
    input: Vec<f32>,
    direct: Vec<f32>,
    hrtf_input: Vec<f32>,
    left_gain: Vec<f32>,
    right_gain: Vec<f32>,
    mono_gain: Vec<f32>,
    deinterleaved: Vec<f32>,
    output: Vec<f32>,
    output_cursor: usize,
}
impl SteamAudioSource {
    fn new(
        sound: Arc<RegisteredSound>,
        gains: Arc<Gains>,
        runtime: Arc<SteamAudioSignalRuntime>,
        binaural_enabled: Arc<AtomicBool>,
        environmental_enabled: Arc<AtomicBool>,
        output_rate: rodio::SampleRate,
    ) -> Result<Self, String> {
        let binaural_effect = runtime.new_binaural_effect()?;
        let direct_effect = runtime.new_direct_effect()?;
        let frame_size = runtime.settings.frame_size as usize;
        let (mono, _, _) = gains.hrtf();
        let rate = output_rate.get() as f32;
        Ok(Self {
            current_stereo: gains.get(),
            current_mono: mono,
            glide: 1.0 - (-1.0 / (GAIN_GLIDE_SECONDS * rate)).exp(),
            fade: 1.0,
            fade_step: 1.0 / (STOP_FADE_SECONDS * rate).max(1.0),
            looping: false,
            input_done: false,
            finished: false,
            last_block_hrtf: false,
            direct_was_active: false,
            hrtf_was_active: false,
            finish_after_block: false,
            input: vec![0.0; frame_size],
            direct: vec![0.0; frame_size],
            hrtf_input: vec![0.0; frame_size],
            left_gain: vec![0.0; frame_size],
            right_gain: vec![0.0; frame_size],
            mono_gain: vec![0.0; frame_size],
            deinterleaved: vec![0.0; frame_size * 2],
            output: vec![0.0; frame_size * 2],
            output_cursor: frame_size * 2,
            sound,
            gains,
            runtime,
            binaural_effect,
            direct_effect,
            binaural_enabled,
            environmental_enabled,
            output_rate,
            source_position: 0.0,
        })
    }

    fn looping(
        sound: Arc<RegisteredSound>,
        gains: Arc<Gains>,
        runtime: Arc<SteamAudioSignalRuntime>,
        binaural_enabled: Arc<AtomicBool>,
        environmental_enabled: Arc<AtomicBool>,
        output_rate: rodio::SampleRate,
        start_frame: usize,
    ) -> Result<Self, String> {
        let mut source = Self::new(
            sound,
            gains,
            runtime,
            binaural_enabled,
            environmental_enabled,
            output_rate,
        )?;
        source.looping = true;
        source.current_stereo = [0.0; 2];
        source.current_mono = 0.0;
        source.source_position = (start_frame % source.sound.samples.len()) as f64;
        Ok(source)
    }

    fn next_source_sample(&mut self) -> Option<f32> {
        if self.sound.samples.is_empty() { return None; }
        let len = self.sound.samples.len();
        if !self.looping && self.source_position >= len as f64 { return None; }
        if self.looping && self.source_position >= len as f64 { self.source_position %= len as f64; }
        let i0 = self.source_position.floor() as usize;
        let frac = (self.source_position - i0 as f64) as f32;
        let i1 = if i0 + 1 < len { i0 + 1 } else if self.looping { 0 } else { i0 };
        let a = self.sound.samples[i0.min(len - 1)];
        let b = self.sound.samples[i1.min(len - 1)];
        let sample = a + (b - a) * frac;
        self.source_position += self.sound.sample_rate.get() as f64 / self.output_rate.get() as f64;
        Some(sample)
    }

    fn fill_tail_block(&mut self) -> bool {
        let Some(effect) = self.binaural_effect.as_mut() else {
            self.finished = true;
            return false;
        };
        self.deinterleaved.fill(0.0);
        let state = {
            let Ok(mut output) = AudioBufferMut::try_new(&mut self.deinterleaved[..], 2) else {
                self.finished = true;
                return false;
            };
            match effect.tail(&mut output) {
                Ok(state) => state,
                Err(_) => {
                    self.finished = true;
                    return false;
                }
            }
        };
        let n = self.input.len();
        for i in 0..n {
            self.output[i * 2] = self.deinterleaved[i];
            self.output[i * 2 + 1] = self.deinterleaved[n + i];
        }
        self.output_cursor = 0;
        self.finish_after_block = state == AudioEffectState::TailComplete;
        true
    }

    fn fill_block(&mut self) -> bool {
        if self.finished { return false; }
        if self.finish_after_block {
            self.finished = true;
            return false;
        }
        if self.input_done {
            if self.last_block_hrtf { return self.fill_tail_block(); }
            self.finished = true;
            return false;
        }

        self.input.fill(0.0);
        self.direct.fill(0.0);
        self.output.fill(0.0);
        let (target_mono, direction, spatializable) = self.gains.hrtf();
        let direction_length = direction.iter().map(|value| value * value).sum::<f32>().sqrt();
        let direction = if direction_length > 0.0001 {
            direction.map(|component| component / direction_length)
        } else {
            [0.0, 0.0, -1.0]
        };
        let target_stereo = self.gains.get();

        for i in 0..self.input.len() {
            for (current, target) in self.current_stereo.iter_mut().zip(target_stereo) {
                *current += (target - *current) * self.glide;
            }
            self.current_mono += (target_mono - self.current_mono) * self.glide;
            if self.gains.stopping.load(Ordering::Relaxed) {
                self.fade = (self.fade - self.fade_step).max(0.0);
                if self.fade <= 0.0 { self.input_done = true; }
            }
            let sample = if self.input_done {
                0.0
            } else if let Some(sample) = self.next_source_sample() {
                sample
            } else {
                self.input_done = true;
                0.0
            };
            self.input[i] = sample * self.fade;
            self.left_gain[i] = self.current_stereo[0];
            self.right_gain[i] = self.current_stereo[1];
            self.mono_gain[i] = self.current_mono;
        }

        let environment_params = if self.environmental_enabled.load(Ordering::Relaxed) && spatializable {
            self.gains.environment_params()
        } else {
            None
        };
        let direct_active = environment_params.is_some();
        if direct_active != self.direct_was_active {
            // A live A/B toggle must not resume filter history from before the
            // bypass interval (or carry the old map/source state into enable).
            self.direct_effect.reset();
            self.direct_was_active = direct_active;
        }
        let direct_applied = if let Some(params) = environment_params {
            if let Ok(input) = AudioBufferRef::try_from(&self.input[..]) {
                if let Ok(mut output) = AudioBufferMut::try_new(&mut self.direct[..], 1) {
                    self.direct_effect.apply(&params, &input, &mut output).is_ok()
                } else { false }
            } else { false }
        } else {
            false
        };

        // Build the exact legacy pan fallback from the direct-processed mono
        // block. Environmental acoustics therefore work even when HRTF is off.
        for i in 0..self.input.len() {
            let processed = if direct_applied { self.direct[i] } else { self.input[i] };
            self.output[i * 2] = processed * self.left_gain[i];
            self.output[i * 2 + 1] = processed * self.right_gain[i];
        }

        self.last_block_hrtf = false;
        let use_hrtf = self.binaural_enabled.load(Ordering::Relaxed)
            && spatializable
            && self.runtime.hrtf.is_some()
            && self.binaural_effect.is_some();
        if use_hrtf != self.hrtf_was_active {
            if let Some(effect) = self.binaural_effect.as_mut() { effect.reset(); }
            self.hrtf_was_active = use_hrtf;
        }
        if use_hrtf {
            for i in 0..self.input.len() {
                let processed = if direct_applied { self.direct[i] } else { self.input[i] };
                self.hrtf_input[i] = processed * self.mono_gain[i];
            }
            self.deinterleaved.fill(0.0);
            let applied = if let (Some(effect), Some(hrtf)) =
                (self.binaural_effect.as_mut(), self.runtime.hrtf.as_ref())
            {
                if let Ok(input) = AudioBufferRef::try_from(&self.hrtf_input[..]) {
                    if let Ok(mut output) = AudioBufferMut::try_new(&mut self.deinterleaved[..], 2) {
                        let params = BinauralEffectParams {
                            direction: Direction::new(direction[0], direction[1], direction[2]),
                            interpolation: HrtfInterpolation::Nearest,
                            spatial_blend: 1.0,
                            hrtf: hrtf.clone(),
                            peak_delays: None,
                        };
                        effect.apply(&params, &input, &mut output).is_ok()
                    } else { false }
                } else { false }
            } else { false };
            if applied {
                let n = self.input.len();
                for i in 0..n {
                    self.output[i * 2] = self.deinterleaved[i];
                    self.output[i * 2 + 1] = self.deinterleaved[n + i];
                }
                self.last_block_hrtf = true;
            }
        }
        self.output_cursor = 0;
        true
    }
}
impl Iterator for SteamAudioSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.output_cursor >= self.output.len() && !self.fill_block() { return None; }
        let sample = self.output[self.output_cursor];
        self.output_cursor += 1;
        Some(sample)
    }
}
impl Source for SteamAudioSource {
    fn current_span_len(&self) -> Option<usize> { if self.finished { Some(0) } else { None } }
    fn channels(&self) -> rodio::ChannelCount { rodio::nz!(2) }
    fn sample_rate(&self) -> rodio::SampleRate { self.output_rate }
    fn total_duration(&self) -> Option<Duration> {
        if self.looping { return None; }
        Some(Duration::from_secs_f64(
            self.sound.samples.len() as f64 / self.sound.sample_rate.get() as f64,
        ))
    }
}

struct Voice { player: Player, gains: Arc<Gains>, request: SoundRequest, steam_audio_capable: bool }
struct LoopVoice { player: Player, gains: Arc<Gains>, steam_audio_capable: bool }

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
    /// Master Steam Audio gate. The map loader attaches the validated acoustic
    /// scene/bake; direct environmental simulation and HRTF are live sub-features.
    pub steam_audio_enabled: bool,
    pub steam_audio_scene_triangles: usize,
    pub steam_audio_bake_ready: bool,
    pub steam_audio_probe_count: usize,
    pub steam_audio_bake_bytes: usize,
    pub steam_audio_bake_cache_hit: bool,
    pub steam_audio_runtime_validated: bool,
    pub steam_audio_binaural_enabled: bool,
    pub steam_audio_binaural_active: bool,
    pub steam_audio_hrtf_voice_count: usize,
    pub steam_audio_hrtf_error: Option<String>,
    pub steam_audio_environmental_enabled: bool,
    pub steam_audio_environmental_active: bool,
    pub steam_audio_environment_voice_count: usize,
    pub steam_audio_environment_updates: u64,
    pub steam_audio_environment_last_ms: f32,
    pub steam_audio_environmental_error: Option<String>,
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
    steam_audio_binaural_enabled: bool,
    steam_audio_environmental_enabled: bool,
    steam_audio_binaural_switch: Arc<AtomicBool>,
    steam_audio_environmental_switch: Arc<AtomicBool>,
    steam_audio_signal_runtime: Option<Arc<SteamAudioSignalRuntime>>,
    steam_audio_hrtf_error: Option<String>,
    steam_audio_environmental_runtime: Option<SteamAudioEnvironmentalRuntime>,
    steam_audio_environmental_error: Option<String>,
    last_environment_submit: Instant,
    steam_audio_acoustic_mesh: Option<Arc<AcousticMesh>>,
    steam_audio_bake: Option<Arc<SteamAudioBakeData>>,
}

impl AudioBackend {
    pub fn open(
        steam_audio_enabled: bool,
        steam_audio_binaural_enabled: bool,
        steam_audio_environmental_enabled: bool,
    ) -> Result<Self, String> {
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
        let master_player = connect_player(device.mixer(), tap.limit(LimitSettings::default()));

        // Build the fixed-block Steam Audio signal runtime whenever the master
        // feature is enabled. HRTF is optional inside this runtime; direct wall
        // filtering can still work on a device where HRTF creation fails.
        let steam_audio_binaural_switch = Arc::new(AtomicBool::new(false));
        let steam_audio_environmental_switch = Arc::new(AtomicBool::new(false));
        let (steam_audio_signal_runtime, steam_audio_hrtf_error) = if steam_audio_enabled {
            let runtime = SteamAudioSignalRuntime::new(output_sample_rate.get());
            let hrtf_error = runtime.hrtf_error.clone();
            if runtime.hrtf.is_some() {
                println!(
                    "STEAM AUDIO SIGNAL DSP: ready at {} Hz / {}-sample blocks (HRTF available)",
                    output_sample_rate.get(),
                    runtime.settings.frame_size,
                );
                steam_audio_binaural_switch.store(steam_audio_binaural_enabled, Ordering::Relaxed);
            } else if let Some(error) = &hrtf_error {
                eprintln!("STEAM AUDIO HRTF UNAVAILABLE: {error}; direct environmental DSP remains available");
            }
            (Some(Arc::new(runtime)), hrtf_error)
        } else {
            (None, None)
        };

        Ok(Self {
            voices: Vec::new(),
            loops: Vec::new(),
            loop_voices: HashMap::new(),
            loop_epoch: Instant::now(),
            mixer,
            _master_player: Some(master_player),
            _device: Some(device),
            origins: HashMap::new(),
            listener: Listener { entity: 0, origin: [0.0; 3], ahead: [1.0, 0.0, 0.0], up: [0.0, 0.0, 1.0], left: [0.0, 1.0, 0.0] },
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
            steam_audio_binaural_enabled,
            steam_audio_environmental_enabled,
            steam_audio_binaural_switch,
            steam_audio_environmental_switch,
            steam_audio_signal_runtime,
            steam_audio_hrtf_error,
            steam_audio_environmental_runtime: None,
            steam_audio_environmental_error: None,
            last_environment_submit: Instant::now(),
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
            listener: Listener { entity: 0, origin: [0.0; 3], ahead: [1.0, 0.0, 0.0], up: [0.0, 0.0, 1.0], left: [0.0, 1.0, 0.0] },
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
            steam_audio_binaural_enabled: true,
            steam_audio_environmental_enabled: true,
            steam_audio_binaural_switch: Arc::new(AtomicBool::new(false)),
            steam_audio_environmental_switch: Arc::new(AtomicBool::new(false)),
            steam_audio_signal_runtime: None,
            steam_audio_hrtf_error: None,
            steam_audio_environmental_runtime: None,
            steam_audio_environmental_error: None,
            last_environment_submit: Instant::now(),
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
        let environmental_runtime_error = self
            .steam_audio_environmental_runtime
            .as_ref()
            .and_then(SteamAudioEnvironmentalRuntime::error)
            .or_else(|| self.steam_audio_environmental_error.clone());
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
            steam_audio_binaural_enabled: self.steam_audio_binaural_enabled,
            steam_audio_binaural_active: self.steam_audio_binaural_active(),
            steam_audio_hrtf_voice_count: self.voices.iter().filter(|voice| voice.steam_audio_capable && voice.gains.hrtf().2).count()
                + self.loop_voices.values().filter(|voice| voice.steam_audio_capable && voice.gains.hrtf().2).count(),
            steam_audio_hrtf_error: self.steam_audio_hrtf_error.clone(),
            steam_audio_environmental_enabled: self.steam_audio_environmental_enabled,
            steam_audio_environmental_active: self.steam_audio_environmental_active(),
            steam_audio_environment_voice_count: self.steam_audio_environmental_runtime.as_ref().map_or(0, SteamAudioEnvironmentalRuntime::source_count),
            steam_audio_environment_updates: self.steam_audio_environmental_runtime.as_ref().map_or(0, SteamAudioEnvironmentalRuntime::update_count),
            steam_audio_environment_last_ms: self.steam_audio_environmental_runtime.as_ref().map_or(0.0, SteamAudioEnvironmentalRuntime::last_ms),
            steam_audio_environmental_error: environmental_runtime_error,
        }
    }

    fn steam_audio_binaural_active(&self) -> bool {
        self.steam_audio_enabled && self.steam_audio_binaural_enabled
            && self.steam_audio_signal_runtime.as_ref().is_some_and(|runtime| runtime.hrtf.is_some())
            && self.steam_audio_binaural_switch.load(Ordering::Relaxed)
    }

    fn steam_audio_environmental_active(&self) -> bool {
        self.steam_audio_enabled && self.steam_audio_environmental_enabled
            && self.steam_audio_environmental_runtime.as_ref().is_some_and(SteamAudioEnvironmentalRuntime::ready)
            && self.steam_audio_environmental_switch.load(Ordering::Relaxed)
    }

    pub fn set_steam_audio_enabled(&mut self, enabled: bool) {
        let had_signal_runtime = self.steam_audio_signal_runtime.is_some();
        self.steam_audio_enabled = enabled;
        if !enabled {
            self.steam_audio_acoustic_mesh = None;
            self.steam_audio_bake = None;
            self.steam_audio_environmental_runtime = None;
        }
        self.refresh_steam_audio_signal_runtime();
        self.refresh_steam_audio_environmental_runtime();
        if had_signal_runtime != self.steam_audio_signal_runtime.is_some() {
            self.drop_loop_players_for_steam_rebuild();
        }
    }

    pub fn set_steam_audio_binaural_enabled(&mut self, enabled: bool) {
        self.steam_audio_binaural_enabled = enabled;
        self.refresh_steam_audio_signal_runtime();
    }

    pub fn set_steam_audio_environmental_enabled(&mut self, enabled: bool) {
        self.steam_audio_environmental_enabled = enabled;
        if !enabled { self.reset_environment_results(); }
        self.refresh_steam_audio_environmental_runtime();
    }

    fn refresh_steam_audio_signal_runtime(&mut self) {
        if !self.steam_audio_enabled {
            self.steam_audio_binaural_switch.store(false, Ordering::Relaxed);
            self.steam_audio_environmental_switch.store(false, Ordering::Relaxed);
            self.steam_audio_signal_runtime = None;
            self.steam_audio_hrtf_error = None;
            return;
        }
        if self.steam_audio_signal_runtime.is_none() {
            let runtime = SteamAudioSignalRuntime::new(self.output_sample_rate);
            self.steam_audio_hrtf_error = runtime.hrtf_error.clone();
            self.steam_audio_signal_runtime = Some(Arc::new(runtime));
        }
        let hrtf_available = self.steam_audio_signal_runtime.as_ref().is_some_and(|runtime| runtime.hrtf.is_some());
        self.steam_audio_binaural_switch.store(self.steam_audio_binaural_enabled && hrtf_available, Ordering::Relaxed);
    }

    fn reset_environment_results(&self) {
        for voice in &self.voices { voice.gains.reset_environment_result(); }
        for voice in self.loop_voices.values() { voice.gains.reset_environment_result(); }
    }

    fn refresh_steam_audio_environmental_runtime(&mut self) {
        if !self.steam_audio_enabled || !self.steam_audio_environmental_enabled {
            self.steam_audio_environmental_switch.store(false, Ordering::Relaxed);
            self.steam_audio_environmental_runtime = None;
            self.steam_audio_environmental_error = None;
            self.reset_environment_results();
            return;
        }
        let Some(mesh) = self.steam_audio_acoustic_mesh.as_ref().cloned() else {
            self.steam_audio_environmental_switch.store(false, Ordering::Relaxed);
            self.steam_audio_environmental_runtime = None;
            self.reset_environment_results();
            return;
        };
        if self.steam_audio_environmental_runtime.is_none() {
            let bake = self.steam_audio_bake.as_ref().filter(|bake| bake.runtime_validated).cloned();
            match SteamAudioEnvironmentalRuntime::try_start(mesh, bake, self.output_sample_rate) {
                Ok(runtime) => {
                    self.steam_audio_environmental_runtime = Some(runtime);
                    self.steam_audio_environmental_error = None;
                }
                Err(error) => self.steam_audio_environmental_error = Some(error),
            }
        }
        let ready = self.steam_audio_environmental_runtime.as_ref().is_some_and(SteamAudioEnvironmentalRuntime::ready);
        self.steam_audio_environmental_switch.store(ready, Ordering::Relaxed);
    }

    fn drop_loop_players_for_steam_rebuild(&mut self) {
        for (_, voice) in std::mem::take(&mut self.loop_voices) { self.retire(voice.player, &voice.gains); }
    }

    pub fn set_steam_audio_map(
        &mut self,
        mesh: Option<Arc<AcousticMesh>>,
        bake: Option<Arc<SteamAudioBakeData>>,
    ) {
        let same_scene = match (&self.steam_audio_acoustic_mesh, &mesh) {
            (Some(current), Some(next)) => Arc::ptr_eq(current, next),
            (None, None) => true,
            _ => false,
        };
        if !same_scene {
            self.steam_audio_environmental_switch.store(false, Ordering::Relaxed);
            self.steam_audio_environmental_runtime = None;
            self.reset_environment_results();
        }
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
        self.refresh_steam_audio_environmental_runtime();
    }

    fn submit_environment_frame(&mut self) {
        if !self.steam_audio_environmental_active()
            || self.last_environment_submit.elapsed() < STEAM_AUDIO_DIRECT_MIN_INTERVAL
        {
            return;
        }
        let mut sources = Vec::with_capacity(self.voices.len() + self.loop_voices.len());
        for voice in &self.voices {
            if !voice.steam_audio_capable { continue; }
            if let Some(origin) = voice.gains.environment_source() {
                sources.push(EnvironmentalSourceFrame {
                    id: Arc::as_ptr(&voice.gains) as usize as u64,
                    origin,
                    gains: Arc::clone(&voice.gains),
                });
            }
        }
        for voice in self.loop_voices.values() {
            if !voice.steam_audio_capable { continue; }
            if let Some(origin) = voice.gains.environment_source() {
                sources.push(EnvironmentalSourceFrame {
                    id: Arc::as_ptr(&voice.gains) as usize as u64,
                    origin,
                    gains: Arc::clone(&voice.gains),
                });
            }
        }
        if let Some(runtime) = &self.steam_audio_environmental_runtime {
            runtime.submit(EnvironmentalFrame { listener: self.listener.origin, sources });
            self.last_environment_submit = Instant::now();
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
        // Direct simulator initialization is asynchronous; this promotes its
        // live switch the first frame after the worker reports ready.
        self.refresh_steam_audio_environmental_runtime();
        self.refresh_voice_gains();
        self.submit_environment_frame();
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
            let spatial = sound_spatialization(&item.request, listener, effects, voice, separation);
            for (previous, next) in item.gains.get().into_iter().zip(spatial.stereo) {
                self.largest_gain_step = self.largest_gain_step.max((previous - next).abs());
            }
            item.gains.set(spatial.stereo);
            item.gains.set_hrtf(spatial.mono, spatial.direction, spatial.hrtf_spatializable);
            let environment_origin = match item.request.origin {
                SoundOrigin::Entity(origin) | SoundOrigin::Fixed(origin)
                    if item.request.entity != listener.entity && spatial.hrtf_spatializable => Some(origin),
                _ => None,
            };
            item.gains.set_environment_source(
                environment_origin.unwrap_or([0.0; 3]),
                environment_origin.is_some(),
            );
        }
        self.refresh_loop_gains();
    }

    /// OpenJK S_AddLoopSounds: spatialize every loop request as CHAN_AUTO
    /// with full master volume, sum per sfx, clamp to SOUND_MAXVOL, then the
    /// paint pass applies s_volume. The listener-owned 0.75 scale of
    /// S_StartSound does not apply to loops.
    ///
    /// OpenJK intentionally merges every emitter using the same looping sfx
    /// into one channel. HRTF therefore cannot retain N independent directions
    /// without changing JKA channel semantics. For an all-positional merged
    /// loop we use a level-weighted mean direction; mixed head-locked + world
    /// emitters stay on the exact legacy stereo path.
    fn refresh_loop_gains(&mut self) {
        #[derive(Default)]
        struct LoopTotal {
            stereo: [f32; 2],
            mono: f32,
            direction_sum: [f32; 3],
            has_spatial: bool,
            has_nonspatial: bool,
            spatial_count: usize,
            environment_origin: Option<[f32; 3]>,
        }

        let mut totals: HashMap<&str, LoopTotal> = HashMap::with_capacity(self.loop_voices.len());
        for request in &self.loops {
            let spatial = match request.origin {
                SoundOrigin::Local => Spatialization {
                    // Preserve the old call to spatialize(listener.origin): at
                    // distance zero dot=0, so each ear gets s_separation.
                    stereo: [self.separation; 2],
                    mono: 1.0,
                    direction: [0.0, 0.0, -1.0],
                    hrtf_spatializable: false,
                },
                SoundOrigin::Entity(origin) | SoundOrigin::Fixed(origin) => {
                    spatialize_detail(origin, self.listener, CHAN_AUTO, self.separation)
                }
            };
            let total = totals.entry(request.qpath.as_str()).or_default();
            total.stereo[0] += spatial.stereo[0];
            total.stereo[1] += spatial.stereo[1];
            total.mono += spatial.mono;
            if spatial.hrtf_spatializable {
                total.has_spatial = true;
                total.spatial_count += 1;
                if total.spatial_count == 1 {
                    total.environment_origin = match request.origin {
                        SoundOrigin::Entity(origin) | SoundOrigin::Fixed(origin) => Some(origin),
                        SoundOrigin::Local => None,
                    };
                }
                for (sum, component) in total.direction_sum.iter_mut().zip(spatial.direction) {
                    *sum += component * spatial.mono;
                }
            } else {
                total.has_nonspatial = true;
            }
        }
        for (qpath, voice) in &self.loop_voices {
            let total = totals.get(qpath.as_str());
            let stereo = total.map_or([0.0; 2], |total| total.stereo);
            let next = stereo.map(|gain| gain.min(1.0) * self.effects_volume);
            for (previous, next) in voice.gains.get().into_iter().zip(next) {
                self.largest_gain_step = self.largest_gain_step.max((previous - next).abs());
            }
            voice.gains.set(next);

            let (mono, direction, spatializable) = if let Some(total) = total {
                let length = total.direction_sum.iter().map(|v| v * v).sum::<f32>().sqrt();
                if total.has_spatial && !total.has_nonspatial && length > 0.0001 {
                    (
                        total.mono.min(1.0) * self.effects_volume,
                        total.direction_sum.map(|component| component / length),
                        true,
                    )
                } else {
                    (total.mono.min(1.0) * self.effects_volume, [0.0, 0.0, -1.0], false)
                }
            } else {
                (0.0, [0.0, 0.0, -1.0], false)
            };
            voice.gains.set_hrtf(mono, direction, spatializable);
            // OpenJK merges all emitters of one looping sfx into one channel.
            // A single direct ray only has a well-defined source position when
            // exactly one positional emitter contributes to that merged loop.
            let environment_origin = total.and_then(|total| {
                (total.spatial_count == 1 && !total.has_nonspatial)
                    .then_some(total.environment_origin)
                    .flatten()
            });
            voice.gains.set_environment_source(
                environment_origin.unwrap_or([0.0; 3]),
                environment_origin.is_some(),
            );
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
            let (player, steam_audio_capable) = if self.steam_audio_enabled {
                if let Some(runtime) = self.steam_audio_signal_runtime.clone() {
                    match SteamAudioSource::looping(
                        Arc::clone(&sound),
                        Arc::clone(&gains),
                        runtime,
                        Arc::clone(&self.steam_audio_binaural_switch),
                        Arc::clone(&self.steam_audio_environmental_switch),
                        rodio::SampleRate::new(self.output_sample_rate).expect("audio sample rate is non-zero"),
                        phase,
                    ) {
                        Ok(source) => (connect_player(&self.mixer, source), true),
                        Err(error) => {
                            eprintln!("STEAM AUDIO LOOP FALLBACK: {error}");
                            (connect_player(&self.mixer, StereoSource::looping(sound, Arc::clone(&gains), phase)), false)
                        }
                    }
                } else {
                    (connect_player(&self.mixer, StereoSource::looping(sound, Arc::clone(&gains), phase)), false)
                }
            } else {
                (connect_player(&self.mixer, StereoSource::looping(sound, Arc::clone(&gains), phase)), false)
            };
            if self.rate == 0.0 { player.pause(); }
            self.loop_voices.insert(qpath, LoopVoice { player, gains, steam_audio_capable });
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
        let spatial = sound_spatialization(
            &request,
            self.listener,
            self.effects_volume,
            self.voice_volume,
            self.separation,
        );
        let gains = Arc::new(Gains::new(spatial.stereo));
        gains.set_hrtf(spatial.mono, spatial.direction, spatial.hrtf_spatializable);

        let environment_origin = match request.origin {
            SoundOrigin::Entity(origin) | SoundOrigin::Fixed(origin)
                if request.entity != self.listener.entity && spatial.hrtf_spatializable => Some(origin),
            _ => None,
        };
        gains.set_environment_source(
            environment_origin.unwrap_or([0.0; 3]),
            environment_origin.is_some(),
        );

        // Head-locked/local sounds retain JKA's legacy centered path. Every
        // remote world-positioned one-shot uses the fixed-block Steam Audio
        // source whenever the master feature is enabled, even if it begins
        // outside JKA's audible radius or both live sub-features are currently
        // off. That keeps HRTF/environmental A/B switching correct if either
        // the listener or an entity moves while the voice is still playing.
        let world_positional = matches!(request.origin, SoundOrigin::Entity(_) | SoundOrigin::Fixed(_))
            && request.entity != self.listener.entity;
        let (player, steam_audio_capable) = if world_positional && self.steam_audio_enabled {
            if let Some(runtime) = self.steam_audio_signal_runtime.clone() {
                match SteamAudioSource::new(
                    Arc::clone(&sound),
                    Arc::clone(&gains),
                    runtime,
                    Arc::clone(&self.steam_audio_binaural_switch),
                    Arc::clone(&self.steam_audio_environmental_switch),
                    rodio::SampleRate::new(self.output_sample_rate).expect("audio sample rate is non-zero"),
                ) {
                    Ok(source) => (connect_player(&self.mixer, source), true),
                    Err(error) => {
                        eprintln!("STEAM AUDIO VOICE FALLBACK: {error}");
                        (connect_player(&self.mixer, StereoSource::new(sound, Arc::clone(&gains))), false)
                    }
                }
            } else {
                (connect_player(&self.mixer, StereoSource::new(sound, Arc::clone(&gains))), false)
            }
        } else {
            (connect_player(&self.mixer, StereoSource::new(sound, Arc::clone(&gains))), false)
        };
        if self.rate == 0.0 { player.pause(); } else { player.set_speed(self.rate); }
        self.voices.push(Voice { player, gains, request, steam_audio_capable });
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

#[derive(Clone, Copy, Debug)]
struct Spatialization {
    stereo: [f32; 2],
    /// OpenJK distance/channel/master gain before legacy L/R panning. This is
    /// the mono level fed to Steam Audio's HRTF path.
    mono: f32,
    /// Steam Audio listener-local direction: +X right, +Y up, +Z behind.
    direction: [f32; 3],
    hrtf_spatializable: bool,
}

/// OpenJK codemp/client/snd_dma.cpp S_SpatializeOrigin. Volume is applied
/// later in OpenJK's ChannelPaint; multiplying it here is algebraically the
/// same for our normalized floating-point samples. HRTF reuses the exact same
/// channel attenuation/master volume but replaces only the final stereo pan.
fn sound_spatialization(
    request: &SoundRequest,
    listener: Listener,
    effects_volume: f32,
    voice_volume: f32,
    separation: f32,
) -> Spatialization {
    let volume = if is_voice_channel(request.channel) { voice_volume } else { effects_volume };
    if matches!(request.origin, SoundOrigin::Local) || request.entity == listener.entity {
        // S_StartSound scales listener-owned body/weapon/voice sounds to
        // SOUND_FMAXVOL (0.75) for channels below CHAN_AMBIENT. These sounds
        // are head-locked by JKA semantics and intentionally bypass HRTF.
        let local_scale = if request.channel < 7 { 0.75 } else { 1.0 };
        let gain = volume * local_scale;
        return Spatialization {
            stereo: [gain; 2],
            mono: gain,
            direction: [0.0, 0.0, -1.0],
            hrtf_spatializable: false,
        };
    }

    let origin = match request.origin {
        SoundOrigin::Entity(p) | SoundOrigin::Fixed(p) => p,
        SoundOrigin::Local => unreachable!(),
    };
    let mut spatial = spatialize_detail(origin, listener, request.channel, separation);
    spatial.stereo = spatial.stereo.map(|gain| gain * volume);
    spatial.mono *= volume;
    spatial
}

#[cfg(test)]
fn sound_gains(
    request: &SoundRequest,
    listener: Listener,
    effects_volume: f32,
    voice_volume: f32,
    separation: f32,
) -> [f32; 2] {
    sound_spatialization(request, listener, effects_volume, voice_volume, separation).stereo
}

/// S_SpatializeOrigin with master_vol = 1 plus the listener-local direction
/// needed by Steam Audio. The legacy stereo result remains byte-for-byte the
/// same formula as before; HRTF consumes `mono` + `direction` instead.
fn spatialize_detail(origin: [f32; 3], listener: Listener, channel: i32, separation: f32) -> Spatialization {
    let delta = std::array::from_fn::<_, 3, _>(|i| origin[i] - listener.origin[i]);
    let distance = delta.iter().map(|v| v * v).sum::<f32>().sqrt();
    let source_dir = if distance > 0.0001 {
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

    let direction = if distance > 0.0001 {
        // JKA listener axes: left/ahead/up. Steam Audio's local convention is
        // +X right, +Y up, +Z behind, so front is -Z.
        let right = listener.left.map(|value| -value);
        let x = right.iter().zip(source_dir).map(|(a, b)| a * b).sum::<f32>();
        let y = listener.up.iter().zip(source_dir).map(|(a, b)| a * b).sum::<f32>();
        let z = -listener.ahead.iter().zip(source_dir).map(|(a, b)| a * b).sum::<f32>();
        let length = (x * x + y * y + z * z).sqrt();
        if length > 0.0001 { [x / length, y / length, z / length] } else { [0.0, 0.0, -1.0] }
    } else {
        [0.0, 0.0, -1.0]
    };

    Spatialization {
        stereo: [distance_gain * left_scale, distance_gain * right_scale],
        mono: distance_gain,
        direction,
        hrtf_spatializable: distance > 0.0001 && distance_gain > 0.0,
    }
}

/// Kept as a focused OpenJK-compatible helper for tests and the merged loop
/// path. New HRTF code must use `spatialize_detail` so it cannot accidentally
/// derive its direction from already-panned stereo gains.
#[cfg(test)]
fn spatialize(origin: [f32; 3], listener: Listener, channel: i32, separation: f32) -> [f32; 2] {
    spatialize_detail(origin, listener, channel, separation).stereo
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
        let listener = Listener { entity: 0, origin: [0.0; 3], ahead: [1.0, 0.0, 0.0], up: [0.0, 0.0, 1.0], left: [0.0, 1.0, 0.0] };
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
    fn environmental_result_is_atomically_published_without_steam_distance_attenuation() {
        let gains = Gains::new([0.5, 0.5]);
        assert!(gains.environment_params().is_none());
        gains.set_environment_source([10.0, 20.0, 30.0], true);
        assert_eq!(gains.environment_source(), Some([10.0, 20.0, 30.0]));
        gains.set_environment_result(0.25, 2, [0.9, 0.6, 0.2]);
        let params = gains.environment_params().expect("published direct params");
        assert!(params.distance_attenuation.is_none());
        assert!(params.air_absorption.is_none());
        assert!(params.directivity.is_none());
        assert_eq!(params.occlusion, Some(0.25));
        match params.transmission {
            Some(Transmission::FrequencyDependent(values)) => {
                assert_eq!(values.0, [0.9, 0.6, 0.2]);
            }
            other => panic!("unexpected transmission mode: {other:?}"),
        }
        gains.set_environment_source([0.0; 3], false);
        assert!(gains.environment_source().is_none());
        assert!(gains.environment_params().is_none());
    }

    #[test]
    fn hrtf_direction_uses_final_listener_axes_without_changing_openjk_gain() {
        let listener = Listener {
            entity: 0,
            origin: [0.0; 3],
            ahead: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            left: [0.0, 1.0, 0.0],
        };
        let right = spatialize_detail([0.0, -100.0, 0.0], listener, CHAN_AUTO, 0.5);
        assert_eq!(right.stereo, [0.0, 1.0]);
        assert_eq!(right.direction, [1.0, 0.0, 0.0]);
        assert!(right.hrtf_spatializable);

        let front = spatialize_detail([100.0, 0.0, 0.0], listener, CHAN_AUTO, 0.5);
        assert_eq!(front.stereo, [0.5, 0.5]);
        assert_eq!(front.direction, [0.0, 0.0, -1.0]);

        let above = spatialize_detail([0.0, 0.0, 100.0], listener, CHAN_AUTO, 0.5);
        assert_eq!(above.direction, [0.0, 1.0, 0.0]);
    }

    #[test]
    fn openjk_voice_volume_separation_and_channel_stomp_rules() {
        let listener = Listener { entity: 0, origin: [0.0; 3], ahead: [1.0, 0.0, 0.0], up: [0.0, 0.0, 1.0], left: [0.0, 1.0, 0.0] };
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
        let mut backend = AudioBackend::open(false, true, true).expect("audio output device");
        backend.set_mix(0.02, 0.02, 0.5);
        backend.play(SoundRequest { origin: SoundOrigin::Local, ..request(0) }, sound);
        std::thread::sleep(Duration::from_millis(200));
        backend.clear();
    }
}
