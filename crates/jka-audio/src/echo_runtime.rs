//! Static BSP EchoField fallback. Only the geometry worker performs queries.
use crate::audio::{decode_sound, RegisteredSound};
use crate::eal_native::{EchoFrame, EchoVoice, SceneFrame, MAX_SOURCES};
use jka_echofield::{
    geometry::{from_upstream, to_upstream, JKA_UNITS_PER_METER},
    motion::doppler_ratio,
    procedural::Category,
    profile::{self, Profiler},
    propagation::{self, Config, Source},
    reverb,
    soundscape::{self, Soundscape},
    world::{BspWorld, Contact},
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

// SPR's default updateMovingSounds is false. New voices still require an
// evaluation. Cap the host's work per tick without reducing rays or bounces.
const NEW_VOICES_PER_TICK: usize = 8;
#[derive(Clone, Copy, Default)]
struct Voice {
    id: u64,
    position: [f32; 3],
    max_distance: f32,
}
#[derive(Clone, Copy)]
struct Request {
    tick: u64,
    queued: Instant,
    listener: [f32; 3],
    ahead: [f32; 3],
    hrtf: bool,
    voices: [Voice; MAX_SOURCES],
}
#[derive(Default)]
struct Mailbox {
    request: Mutex<Option<Request>>,
    result: Mutex<Option<Result<EchoFrame, String>>>,
    result_ready: Condvar,
    stop: AtomicBool,
}
pub(crate) struct AmbienceVoice {
    pub voice: soundscape::Voice,
    pub sound: Arc<RegisteredSound>,
}

#[derive(Default)]
pub(crate) struct AmbienceScene {
    pub loops: Option<Vec<AmbienceVoice>>,
    pub triggered: VecDeque<AmbienceVoice>,
    pub reflections: Arc<[profile::Tap]>,
    pub diagnostics: Option<Diagnostics>,
}

#[derive(Clone, Copy, Debug)]
pub struct Diagnostics {
    pub movers: usize,
    pub contacts: usize,
    pub room_taps: usize,
    pub evaluated_voices: usize,
    pub enclosure: f32,
    pub wind_exposure: f32,
    pub rt60: f32,
    pub queue_ms: f32,
    pub compute_ms: f32,
}

const AMBIENCE_CACHE_LIMIT: usize = 72;
const MAX_RECORDING_BYTES: usize = 2 * 1024 * 1024;
/// Recordings packed in `DinurdoJK/shared/echofieldassets.pk3`, numbered from 1.
const RECORDING_ROOT: &str = "sound/japro";
const MAX_RECORDINGS_PER_SET: usize = 16;
/// Recordings are leveled to the procedural buffers' peak.
const RECORDING_PEAK: f32 = 0.75;

struct AmbienceBank {
    clips: HashMap<soundscape::Clip, (Arc<RegisteredSound>, u64)>,
    /// Game VFS reader handed over by the audio backend through WorkerStatus.
    reader: Option<jka_assets::pk3::AssetSearchPath>,
    recordings: HashMap<soundscape::Clip, Vec<String>>,
}

/// Packed recording set, then stock JKA sounds, per clip. A clip with neither
/// falls back to synthesis only where `Clip::generate` provides it.
fn recording_sources(clip: soundscape::Clip) -> Option<(&'static str, &'static [&'static str])> {
    use soundscape::Clip;
    Some(match clip {
        Clip::Excitation(jka_echofield::excitation::Kind::Wind) => {
            ("", &["sound/ambience/outdoor/windy1_lp.wav"])
        }
        Clip::Emitter(Category::WaterFlow, _) => (
            "flow",
            &["sound/ambience/water_med_lp.wav", "sound/ambience/water_fast_lp.wav"],
        ),
        Clip::Emitter(Category::WaterStill, _) => ("still", &[]),
        Clip::Emitter(Category::WaterDrip, _) => (
            "drip",
            &[
                "sound/ambience/yavin/drops2.mp3",
                "sound/ambience/yavin/drops3.mp3",
                "sound/ambience/yavin/drops4.mp3",
            ],
        ),
        Clip::Emitter(Category::WaterRain, _) => ("rain", &[]),
        Clip::Emitter(Category::Lava, _) => ("", &["sound/ambience/artus/artus_lava.wav"]),
        _ => return None,
    })
}

impl AmbienceBank {
    fn new() -> Self {
        Self {
            clips: HashMap::new(),
            reader: None,
            recordings: HashMap::new(),
        }
    }

    fn take_reader(&mut self, status: &crate::eal_worker::WorkerStatus) {
        if self.reader.is_none() {
            self.reader = status
                .ambience_assets
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
        }
    }

    fn read_recording(&mut self, clip: soundscape::Clip) -> Option<RegisteredSound> {
        let (set, stock) = recording_sources(clip)?;
        let reader = self.reader.as_mut()?;
        // Category-level key: every variant of a clip shares one path list.
        let key = match clip {
            soundscape::Clip::Emitter(category, _) => soundscape::Clip::Emitter(category, 0),
            clip => clip,
        };
        let paths = self.recordings.entry(key).or_insert_with(|| {
            let mut paths: Vec<String> = if set.is_empty() {
                Vec::new()
            } else {
                (1..=MAX_RECORDINGS_PER_SET)
                    .map(|index| format!("{RECORDING_ROOT}/{set}/{set}_{index:02}.ogg"))
                    .take_while(|path| reader.contains_qpath(path))
                    .collect()
            };
            if paths.is_empty() {
                paths = stock
                    .iter()
                    .filter(|path| reader.contains_qpath(path))
                    .map(|path| (*path).to_owned())
                    .collect();
            }
            paths
        });
        if paths.is_empty() {
            return None;
        }
        let index = match clip {
            soundscape::Clip::Emitter(category, variant) if category.info().2 => variant / 4,
            soundscape::Clip::Emitter(_, variant) => variant,
            _ => 0,
        };
        let path = paths[usize::from(index) % paths.len()].clone();
        let asset = reader.read(&path, MAX_RECORDING_BYTES).ok().flatten()?;
        decode_sound(asset.bytes)
            .ok()
            .map(|sound| sound.normalized(RECORDING_PEAK))
    }

    fn prepare(&mut self, voice: soundscape::Voice, tick: u64) -> Option<AmbienceVoice> {
        let sound = if let Some((sound, used)) = self.clips.get_mut(&voice.clip) {
            *used = tick;
            Arc::clone(sound)
        } else {
            let sound = Arc::new(
                self.read_recording(voice.clip)
                    .or_else(|| RegisteredSound::from_mono(voice.clip.generate()?, 44_100))?,
            );
            if self.clips.len() >= AMBIENCE_CACHE_LIMIT {
                if let Some((&oldest, _)) = self.clips.iter().min_by_key(|(_, (_, used))| *used) {
                    self.clips.remove(&oldest);
                }
            }
            self.clips.insert(voice.clip, (Arc::clone(&sound), tick));
            sound
        };
        Some(AmbienceVoice { voice, sound })
    }
}
pub struct EchoRuntime {
    mailbox: Arc<Mailbox>,
    thread: thread::Thread,
    latest: EchoFrame,
    last_tick: Option<u64>,
    ready: bool,
    rate: u32,
    offline: bool,
}
fn empty_frame() -> EchoFrame {
    EchoFrame {
        slots: reverb::base_slots(1., 1., 1.),
        voices: [EchoVoice::default(); MAX_SOURCES],
        disable_authored_diffraction: 0,
        reserved: 0,
    }
}
impl EchoRuntime {
    pub fn new(
        rate: u32,
        geometry: GeometryInput,
        static_reverb: bool,
        offline: bool,
        status: Arc<crate::eal_worker::WorkerStatus>,
    ) -> Result<Self, String> {
        let mailbox = Arc::new(Mailbox::default());
        let shared = Arc::clone(&mailbox);
        let handle = thread::Builder::new()
            .name("echofield-geometry".into())
            .spawn(move || {
                let cancelled = || shared.stop.load(Ordering::Acquire);
                let world = (|| -> Result<BspWorld, String> {
                    let (bsp, mesh) = geometry.load(&cancelled)?;
                    BspWorld::build_cancellable(bsp, mesh, &cancelled).map_err(str::to_owned)
                })();
                let mut world = match world {
                    Ok(w) => w,
                    Err(e) => {
                        if !cancelled() {
                            *shared.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(Err(e));
                            shared.result_ready.notify_one();
                        }
                        return;
                    }
                };
                let mut profiler = Profiler::default();
                let mut cache = [(
                    0_u64,
                    propagation::Environment::default(),
                    0_u64,
                    [0.0_f32; 3],
                ); MAX_SOURCES];
                let mut geometry_revision = 1_u64;
                let mut previous_settings = crate::EchoSettings::default();
                let mut movers = Vec::new();
                let mut previous_submerged = false;
                let mut previous_software_underwater = false;
                let mut path_listener = None;
                let mut previous_listener = None;
                let mut previous_tick = None;
                let mut motion = [(0_u64, [0.0; 3], 1.0_f32); MAX_SOURCES];
                let mut previous_hrtf = false;
                let mut round_robin = 0;
                let mut next_scan = 0;
                let mut contacts = Vec::<Contact>::new();
                let mut soundscape = Soundscape::default();
                let mut bank = AmbienceBank::new();
                let mut direct_reverb = reverb::DrProcessor::default();
                while !cancelled() {
                    let req = shared
                        .request
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .take();
                    let Some(req) = req else {
                        thread::park();
                        continue;
                    };
                    let started = Instant::now();
                    let queue_ms =
                        started.saturating_duration_since(req.queued).as_secs_f32() * 1000.0;
                    let listener = to_upstream(req.listener.map(f64::from));
                    let ahead = to_upstream(req.ahead.map(f64::from));
                    let settings = *status
                        .echo_settings
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if settings.enhancements && settings.doppler {
                        let mut pitch = HashMap::new();
                        if let Some((last_listener, last_tick)) =
                            previous_listener.zip(previous_tick)
                        {
                            let seconds = req.tick.saturating_sub(last_tick) as f64 / 20.0;
                            for (voice, sample) in req.voices.iter().zip(&mut motion) {
                                let position = to_upstream(voice.position.map(f64::from));
                                let ratio = if voice.id != 0 && voice.id == sample.0 {
                                    let target = doppler_ratio(
                                        last_listener,
                                        listener,
                                        sample.1,
                                        position,
                                        seconds,
                                    );
                                    let smoothed = sample.2 + (target - sample.2) * 0.5;
                                    if (smoothed - 1.0).abs() < 0.0005 {
                                        1.0
                                    } else {
                                        smoothed
                                    }
                                } else {
                                    1.0
                                };
                                if ratio != 1.0 {
                                    pitch.insert(voice.id, ratio);
                                }
                                *sample = (voice.id, position, ratio);
                            }
                        }
                        *status.doppler.lock().unwrap_or_else(|e| e.into_inner()) = Some(pitch);
                    } else if previous_settings.doppler && previous_settings.enhancements {
                        *status.doppler.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(HashMap::new());
                        motion.fill((0, [0.0; 3], 1.0));
                    }
                    previous_tick = Some(req.tick);
                    if settings != previous_settings {
                        previous_settings = settings;
                        geometry_revision = geometry_revision.wrapping_add(1);
                        next_scan = 0;
                    }
                    let pending = status
                        .movers
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .take();
                    if let Some(pending) = pending.filter(|pending| *pending != movers) {
                        world.set_movers(&pending);
                        movers = pending;
                        geometry_revision = geometry_revision.wrapping_add(1);
                    }
                    if (!movers.is_empty() || settings.resample_moving_sources)
                        && path_listener
                            .is_none_or(|old| profile::length(profile::sub(listener, old)) > 0.5)
                    {
                        path_listener = Some(listener);
                        geometry_revision = geometry_revision.wrapping_add(1);
                    }
                    let teleported = previous_listener
                        .is_some_and(|old| profile::length(profile::sub(listener, old)) > 32.);
                    if previous_hrtf != req.hrtf || teleported {
                        profiler.invalidate();
                        cache.fill((0, propagation::Environment::default(), 0, [0.0; 3]));
                        direct_reverb.clear();
                    }
                    previous_listener = Some(listener);
                    previous_hrtf = req.hrtf;
                    profiler.tick(&mut world, listener, req.tick);
                    let profile = profiler.current(req.tick);
                    if teleported {
                        soundscape = Soundscape::default();
                        status
                            .ambience
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .triggered
                            .clear();
                    }
                    if settings.enhancements && (req.tick >= next_scan || teleported) {
                        contacts = world.ambience_contacts(listener);
                        let river = status
                            .river_contacts
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        if let Some((at, regions)) = river.as_ref() {
                            if at.elapsed() < Duration::from_millis(650) {
                                contacts.extend(regions.iter().copied().filter(|contact| {
                                    profile::length(profile::sub(contact.position, listener)) < 32.0
                                }));
                            }
                        }
                        drop(river);
                        let terrain = if settings.terrain {
                            world.terrain_features(
                                listener,
                                &contacts,
                                f64::from(settings.terrain_range),
                            )
                        } else {
                            Vec::new()
                        };
                        soundscape.scan(listener, &profile, &contacts, terrain, settings);
                        next_scan = req.tick + 40;
                    }
                    let mut weather = *status.weather.lock().unwrap_or_else(|e| e.into_inner());
                    weather.submerged |= world.submerged(listener);
                    let software_underwater = status.software_underwater.load(Ordering::Relaxed);
                    if previous_submerged != weather.submerged
                        || previous_software_underwater != software_underwater
                    {
                        geometry_revision = geometry_revision.wrapping_add(1);
                        previous_submerged = weather.submerged;
                        previous_software_underwater = software_underwater;
                    }
                    bank.take_reader(&status);
                    let ambience =
                        soundscape.tick(req.tick, listener, &profile, &contacts, weather, settings);
                    let loops = ambience
                        .loops
                        .into_iter()
                        .filter_map(|voice| bank.prepare(voice, req.tick))
                        .collect();
                    let new_triggers: Vec<_> = ambience
                        .triggered
                        .into_iter()
                        .filter_map(|voice| bank.prepare(voice, req.tick))
                        .collect();
                    {
                        let mut latest = status.ambience.lock().unwrap_or_else(|e| e.into_inner());
                        latest.loops = Some(loops);
                        latest.reflections = if static_reverb
                            && req.hrtf
                            && !weather.submerged
                            && settings.enhancements
                            && settings.reflection_intensity > 0.0
                        {
                            reverb::reflection_candidates(&profile, listener, ahead)
                                .into_iter()
                                .take(8)
                                .collect::<Vec<_>>()
                                .into()
                        } else {
                            Arc::from([])
                        };
                        latest.triggered.extend(new_triggers);
                        while latest.triggered.len() > 16 {
                            latest.triggered.pop_front();
                        }
                    }
                    direct_reverb.cleanup(req.tick);
                    let yaw = (-ahead[0]).atan2(ahead[2]).to_degrees() as f32;
                    let mut result = empty_frame();
                    result.disable_authored_diffraction =
                        u32::from(settings.enhancements && !settings.diffraction);
                    if static_reverb && req.hrtf && settings.enhancements {
                        result.slots = reverb::override_slots(&profile, yaw, result.slots);
                    }
                    for (v, c) in req.voices.iter().zip(&mut cache) {
                        if v.id != c.0 {
                            c.0 = 0;
                        }
                    }
                    let mut budget = NEW_VOICES_PER_TICK;
                    for offset in 0..MAX_SOURCES {
                        if cancelled() {
                            return;
                        }
                        let i = (round_robin + offset) % MAX_SOURCES;
                        let v = req.voices[i];
                        let moved = settings.resample_moving_sources
                            && v.position
                                .iter()
                                .zip(cache[i].3)
                                .any(|(now, old)| (now - old).abs() > 1.0);
                        if v.id == 0
                            || (cache[i].0 == v.id && cache[i].2 == geometry_revision && !moved)
                        {
                            continue;
                        }
                        let pos = to_upstream(v.position.map(f64::from));
                        let source_underwater = !software_underwater && world.submerged(pos);
                        let listener_underwater = !software_underwater && weather.submerged;
                        let mut env = if static_reverb {
                            let source = Source {
                                position: pos,
                                max_distance: v.max_distance / JKA_UNITS_PER_METER as f32,
                                underwater: source_underwater,
                                block_sound: false,
                                voicechat: false,
                                aux_only: false,
                            };
                            let config = Config {
                                block_absorption: if settings.transmission { 1.0 } else { 0.0 },
                                direction_evaluation: settings.diffraction,
                                ..Config::default()
                            };
                            propagation::evaluate(
                                &mut world,
                                source,
                                listener,
                                listener_underwater,
                                config,
                            )
                        } else if settings.enhancements && settings.transmission {
                            // The native authored renderer arbitrates ownership.
                            // Here we describe moving geometry and liquid only.
                            let occlusion = world.dynamic_occlusion(pos, listener);
                            let mut cutoff = (-3.0 * occlusion).exp() as f32;
                            if source_underwater || listener_underwater {
                                cutoff *= 0.1;
                            }
                            propagation::Environment {
                                direct_cutoff: cutoff,
                                direct_gain: cutoff.powf(0.1),
                                ..propagation::Environment::default()
                            }
                        } else {
                            propagation::Environment::default()
                        };
                        // Later conflict-free upstream change: blend only at evaluation,
                        // not continuously for every cached voice on every listener tick.
                        if static_reverb && req.hrtf && settings.enhancements {
                            propagation::blend_profile(&mut env, &profile);
                        }
                        // Store a direction, not an absolute stale world position. The
                        // native worker reconstructs it at the current listener/range.
                        env.redirected_position = env
                            .redirected_position
                            .map(|p| profile::normalize(profile::sub(p, listener)));
                        cache[i] = (v.id, env, geometry_revision, v.position);
                        budget -= 1;
                        if budget == 0 {
                            round_robin = (i + 1) % MAX_SOURCES;
                            break;
                        }
                    }
                    for (i, v) in req.voices.iter().enumerate() {
                        if v.id == 0 || cache[i].0 != v.id {
                            continue;
                        }
                        let mut env = cache[i].1;
                        if static_reverb
                            && req.hrtf
                            && settings.enhancements
                            && settings.per_source_dr
                        {
                            let pos = to_upstream(v.position.map(f64::from));
                            let distance = profile::length(profile::sub(pos, listener)) as f32;
                            if let Some(multiplier) = direct_reverb.update(
                                v.id,
                                distance,
                                profile.critical_distance(),
                                req.tick,
                            ) {
                                env.send_gain = env.send_gain.map(|gain| gain * multiplier);
                            }
                        }
                        if static_reverb
                            && settings.air_absorption
                            && settings.enhancements
                            && !weather.submerged
                        {
                            let distance = profile::length(profile::sub(
                                to_upstream(v.position.map(f64::from)),
                                listener,
                            )) as f32;
                            let humidity = if settings.humidity > 0.0 {
                                settings.humidity
                            } else if weather.raining
                                || contacts.iter().any(|contact| contact.water())
                            {
                                75.0
                            } else if weather.temperature > 1.2 {
                                25.0
                            } else {
                                50.0
                            };
                            let [gain, cutoff] =
                                jka_echofield::acoustic::lowpass_parameters(distance, humidity);
                            env.direct_gain *= gain;
                            env.direct_cutoff *= cutoff;
                        }
                        let direction = env
                            .redirected_position
                            .map(|p| from_upstream(p).map(|x| (x / JKA_UNITS_PER_METER) as f32))
                            .unwrap_or([0.; 3]);
                        result.voices[i] = EchoVoice {
                            id: v.id,
                            position: direction,
                            redirected: u32::from(env.redirected_position.is_some()),
                            direct_gain: env.direct_gain,
                            direct_cutoff: env.direct_cutoff,
                            send_gain: env.send_gain,
                            send_cutoff: env.send_cutoff,
                        };
                    }
                    if !cancelled() {
                        status
                            .ambience
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .diagnostics = Some(Diagnostics {
                            movers: movers.len(),
                            contacts: contacts.len(),
                            room_taps: profile.taps.len(),
                            evaluated_voices: NEW_VOICES_PER_TICK - budget,
                            enclosure: profile.enclosure,
                            wind_exposure: profile.wind_exposure,
                            rt60: profile.rt60,
                            queue_ms,
                            compute_ms: started.elapsed().as_secs_f32() * 1000.0,
                        });
                        *shared.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(Ok(result));
                        shared.result_ready.notify_one();
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let thread = handle.thread().clone();
        Ok(Self {
            mailbox,
            thread,
            latest: empty_frame(),
            last_tick: None,
            ready: false,
            rate,
            offline,
        })
    }
    pub fn is_ready(&self) -> bool {
        self.ready
    }
    pub fn latest(&self) -> &EchoFrame {
        &self.latest
    }
    pub fn update(
        &mut self,
        sequence: u64,
        frame: &SceneFrame,
        verified_hrtf: bool,
    ) -> Result<&EchoFrame, String> {
        let tick = sequence
            .saturating_mul(crate::eal_native::FRAMES as u64)
            .saturating_mul(20)
            / u64::from(self.rate.max(1));
        // Live playback overwrites stale work. Offline capture consumes each
        // tick before its next sample block, independent of render speed.
        if self.last_tick != Some(tick) {
            let pending = if self.offline {
                Some(
                    self.mailbox
                        .request
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()),
                )
            } else {
                self.mailbox.request.try_lock().ok()
            };
            if let Some(mut pending) = pending {
                *pending = Some(Request {
                    tick,
                    queued: Instant::now(),
                    listener: frame.listener,
                    ahead: frame.ahead,
                    hrtf: frame.hrtf != 0 && verified_hrtf,
                    voices: std::array::from_fn(|i| {
                        let v = &frame.voices[i];
                        Voice {
                            id: if v.active != 0 { v.id } else { 0 },
                            position: v.position,
                            max_distance: v.max_distance,
                        }
                    }),
                });
                self.last_tick = Some(tick);
                self.thread.unpark();
                drop(pending);
                if self.offline {
                    let result = self
                        .mailbox
                        .result
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    let (mut result, _) = self
                        .mailbox
                        .result_ready
                        .wait_timeout_while(result, Duration::from_secs(4), |result| {
                            result.is_none()
                        })
                        .unwrap_or_else(|e| e.into_inner());
                    self.latest = result
                        .take()
                        .ok_or("Echofield offline geometry timed out")??;
                    self.ready = true;
                    return Ok(&self.latest);
                }
            }
        }
        if let Ok(mut result) = self.mailbox.result.try_lock() {
            if let Some(frame) = result.take() {
                self.latest = frame?;
                self.ready = true;
            }
        }
        Ok(&self.latest)
    }
}
impl Drop for EchoRuntime {
    fn drop(&mut self) {
        self.mailbox.stop.store(true, Ordering::Release);
        self.thread.unpark();
    }
}

/// Prepared map data shares the original parse and mesh. The deferred asset
/// variant is the live-enable/restart path without a preparation snapshot.
#[derive(Clone)]
pub enum GeometryInput {
    Asset {
        reader: Arc<Mutex<jka_assets::pk3::AssetSearchPath>>,
        path: String,
    },
    Prepared(
        Arc<jka_assets::bsp::AcousticCollision>,
        Arc<jka_assets::bsp::AcousticMesh>,
    ),
}
impl GeometryInput {
    pub(crate) fn asset(reader: jka_assets::pk3::AssetSearchPath, path: String) -> Self {
        Self::Asset {
            reader: Arc::new(Mutex::new(reader)),
            path,
        }
    }
    fn load(
        self,
        cancelled: &impl Fn() -> bool,
    ) -> Result<
        (
            Arc<jka_assets::bsp::AcousticCollision>,
            Arc<jka_assets::bsp::AcousticMesh>,
        ),
        String,
    > {
        if cancelled() {
            return Err("EchoField cancelled".into());
        }
        match self {
            Self::Prepared(collision, mesh) => Ok((collision, mesh)),
            Self::Asset { reader, path } => {
                let mut reader = reader.lock().unwrap_or_else(|e| e.into_inner()).fork();
                let asset = reader
                    .read(&path, jka_assets::bsp::MAX_FILE_BYTES)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("EchoField BSP not found: {path}"))?;
                if cancelled() {
                    return Err("EchoField cancelled".into());
                }
                let bsp = jka_assets::bsp::Bsp::parse(&asset.bytes).map_err(|e| e.to_string())?;
                drop(asset);
                if cancelled() {
                    return Err("EchoField cancelled".into());
                }
                let mesh = Arc::new(
                    bsp.world_acoustic_mesh_cancellable(4, cancelled)
                        .map_err(|e| e.to_string())?,
                );
                let collision = Arc::new(bsp.acoustic_collision().map_err(|e| e.to_string())?);
                Ok((collision, mesh))
            }
        }
    }
}
