//! Authored river recordings controlled by world-space hydraulic samples.
//! Independent of the underwater switch and the selected acoustic backend.
use crate::audio::{
    AudioBackend, LoopRequest, MAX_RIVER_LOOP_VOICES, RIVER_LOOP_NAMES, RIVER_LOOP_ROOT,
    RegisteredSound, SoundAssets,
    SoundDecodeResult, SoundOrigin,
};
use jka_river::river_ambience::{self, Region};
use std::{
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

// Six required layers, then three optional specialist textures. All are mono
// positional loops from the ordinary mounted assets (WAV only: the audio crate
// recognises river loops by name under sound/japro/).
const NAMES: [&str; 9] = RIVER_LOOP_NAMES;
const VARIANTS: usize = 3;
const INTERVAL: Duration = Duration::from_millis(100);
const TARGET_VOICES: usize = MAX_RIVER_LOOP_VOICES - 4; // leave room for outgoing crossfades

#[derive(Clone)]
struct Clip {
    path: String,
    sound: Arc<RegisteredSound>,
}
#[derive(Clone)]
struct Voice {
    emitter: u64,
    layer: usize,
    clip: Clip,
    origin: [f32; 3],
    target_origin: [f32; 3],
    gain: f32,
    target: f32,
    score: f32,
}

pub(super) struct RiverAmbience {
    volume: f32,
    last: Option<Instant>,
    last_frame: Option<Instant>,
    epoch: u64,
    last_eye: Option<[f32; 3]>,
    clips: [Vec<Clip>; 9],
    next_clip: usize,
    pending: Option<(usize, String, mpsc::Receiver<SoundDecodeResult>)>,
    voices: Vec<Voice>,
}
impl Default for RiverAmbience {
    fn default() -> Self {
        Self {
            volume: 0.0,
            last: None,
            last_frame: None,
            epoch: river_ambience::epoch(),
            last_eye: None,
            clips: std::array::from_fn(|_| Vec::new()),
            next_clip: 0,
            pending: None,
            voices: Vec::new(),
        }
    }
}
fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
fn mix64(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}
fn emitter(r: &Region) -> u64 {
    let xy = (u64::from(r.bin[0] as u32) << 32) | u64::from(r.bin[1] as u32);
    mix64(mix64(r.river) ^ mix64(xy)) | (1u64 << 63)
}
fn distance2(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.iter().zip(b).map(|(a, b)| (a - b).powi(2)).sum()
}
// Adjacent base textures crossfade by power, rather than adding four full
// recordings. Turbulence trades smooth flow for whitewater; rotational shear
// and strong drop arrivals provide independent specialist contributions.
fn powers(r: &Region) -> [f32; 9] {
    let v = r.speed.max(0.0);
    let t = r.turbulence.clamp(0.0, 1.0);
    let curl = r.curl.clamp(0.0, 1.0);
    let fall = r.drop.clamp(0.0, 1.0);
    let a = smooth(0.04, 0.35, v);
    let b = smooth(0.35, 1.25, v);
    let c = smooth(1.25, 3.0, v);
    let base = (1.0 - t * 0.85) * (1.0 - fall * 0.85);
    let fast = smooth(0.5, 2.5, v);
    let mut p = [
        (1.0 - a) * base,
        a * (1.0 - b) * base,
        b * (1.0 - c) * base,
        c * base,
        t * fast * (1.0 - fall * 0.6),
        fall * 1.5,
        t * (1.0 - fast) * 0.65,
        t * curl * (1.0 - fall) * 0.55,
        t * curl * smooth(0.55, 0.95, t) * fast * 0.8,
    ];
    // Recordings should be loudness matched; these are acoustic-design
    // coefficients, not measured sound pressure levels. Small wet fractions
    // contribute less power than broad water surfaces.
    let sum = p.iter().sum::<f32>().max(1.0);
    let intensity = 0.10 + 0.22 * smooth(0.08, 2.5, v) + 0.25 * t + 0.40 * fall;
    for x in &mut p {
        *x *= r.coverage.clamp(0.0, 1.0) * intensity * intensity / sum;
    }
    p
}

impl RiverAmbience {
    pub(super) fn set_volume(&mut self, volume: f32, backend: Option<&mut AudioBackend>) {
        self.volume = volume.clamp(0.0, 1.0);
        self.last = None;
        if self.volume == 0.0 {
            river_ambience::invalidate();
            self.clear(backend);
        }
    }
    pub(super) fn clear(&mut self, backend: Option<&mut AudioBackend>) {
        self.voices.clear();
        self.last_eye = None;
        self.last = None;
        self.last_frame = None;
        if let Some(b) = backend {
            b.set_river_loops(Vec::new());
        }
    }
    pub(super) fn retry_assets(&mut self) {
        self.clips = std::array::from_fn(|_| Vec::new());
        self.next_clip = 0;
        self.pending = None;
        self.last = None;
    }
    fn accept_clip(&mut self, index: usize, path: String, assets: &mut SoundAssets) {
        match assets.register(&path) {
            Ok(sound) => self.clips[index % NAMES.len()].push(Clip { path, sound }),
            Err(e) if index < 6 => eprintln!("RIVER AUDIO: {path}: {e}"),
            Err(_) => {} // specialist and variation recordings are optional
        }
    }
    fn poll_bank(&mut self, assets: &mut SoundAssets) {
        if let Some((index, path, rx)) = self.pending.take() {
            match rx.try_recv() {
                Ok(result) => {
                    assets.queue_predecoded([result]);
                    self.accept_clip(index, path, assets);
                }
                Err(mpsc::TryRecvError::Empty) => {
                    self.pending = Some((index, path, rx));
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    eprintln!("RIVER AUDIO: decode worker disconnected: {path}");
                }
            }
        }
        // At most one new VFS read per control tick; codec runs off the game
        // thread, registered PCM uses the existing bounded sound cache.
        if self.next_clip >= NAMES.len() * VARIANTS {
            return;
        }
        let index = self.next_clip;
        self.next_clip += 1;
        let layer = index % NAMES.len();
        let variant = index / NAMES.len();
        let path = if variant == 0 {
            format!("{RIVER_LOOP_ROOT}{}.wav", NAMES[layer])
        } else {
            format!("{RIVER_LOOP_ROOT}{}_{}.wav", NAMES[layer], variant + 1)
        };
        if let Some(job) = assets.stage_decode(&path) {
            let (tx, rx) = mpsc::channel();
            match std::thread::Builder::new()
                .name("river-audio-decode".into())
                .spawn(move || {
                    let _ = tx.send(job.decode());
                }) {
                Ok(_) => self.pending = Some((index, path, rx)),
                Err(error) => eprintln!("RIVER AUDIO: cannot start decoder for {path}: {error}"),
            }
        } else {
            self.accept_clip(index, path, assets);
        }
    }
    pub(super) fn update(
        &mut self,
        assets: &mut SoundAssets,
        backend: &mut AudioBackend,
        eye: [f32; 3],
    ) {
        if self.volume <= 0.0 || !crate::river_runtime::audio_rivers_present() {
            if !self.voices.is_empty() {
                self.clear(Some(backend));
            }
            return;
        }
        if self.next_clip >= NAMES.len() * VARIANTS
            && self.pending.is_none()
            && self.clips.iter().all(Vec::is_empty)
        {
            return;
        }
        let now = Instant::now();
        if self
            .last_eye
            .is_some_and(|p| distance2(p, eye) > 384.0 * 384.0)
        {
            river_ambience::invalidate();
        }
        let epoch = river_ambience::epoch();
        if self.epoch != epoch {
            self.clear(Some(backend));
            self.epoch = epoch;
        }
        let dt = self
            .last_frame
            .replace(now)
            .map_or(0.0, |last| now.duration_since(last).as_secs_f32().min(0.25));
        let alpha = 1.0 - (-dt / 0.22).exp();
        // Hydraulic readback and asset staging stay at 10 Hz; audible envelopes
        // and emitter positions glide on every presentation frame.
        if self
            .last
            .is_none_or(|last| now.duration_since(last) >= INTERVAL)
        {
            self.last = Some(now);
            self.last_eye = Some(eye);
            let snapshot = river_ambience::request(eye);
            backend.set_echofield_river_contacts(
                snapshot.iter().flat_map(|snapshot| snapshot.regions.iter())
                    .filter(|region| region.coverage >= 0.35 && region.speed <= 0.65
                        && distance2(region.origin, eye) < 1024.0 * 1024.0)
                    .map(|region| region.origin)
            );
            if snapshot.as_ref().is_some_and(|s| !s.regions.is_empty()) || self.pending.is_some() {
                self.poll_bank(assets);
            }
            let mut candidates = Vec::new();
            if let Some(snapshot) = snapshot {
                for r in snapshot.regions.iter() {
                    let d = distance2(r.origin, eye).sqrt();
                    let attenuation = (1.0 - (d - 256.0).max(0.0) * 0.0008).max(0.0);
                    if attenuation <= 0.0 {
                        continue;
                    }
                    let id = emitter(r);
                    let mut p = powers(r);
                    // Optional specialist layers fall back to their closest core
                    // texture, accumulating power instead of doubling the same PCM.
                    for (layer, fallback) in [(6, 4), (7, 2), (8, 4)] {
                        if self.clips[layer].is_empty() {
                            p[fallback] += p[layer];
                            p[layer] = 0.0;
                        }
                    }
                    for (layer, power) in p.into_iter().enumerate() {
                        let clips = &self.clips[layer];
                        let gain = power.sqrt() * self.volume;
                        if clips.is_empty() || gain * attenuation < 0.008 {
                            continue;
                        }
                        let previous = self
                            .voices
                            .iter()
                            .find(|v| v.emitter == id && v.layer == layer);
                        let clip = previous.map(|v| v.clip.clone()).unwrap_or_else(|| {
                            clips[(mix64(id ^ layer as u64) % clips.len() as u64) as usize].clone()
                        });
                        let score =
                            gain * attenuation * if previous.is_some() { 1.18 } else { 1.0 };
                        candidates.push(Voice {
                            emitter: id,
                            layer,
                            clip,
                            origin: r.origin,
                            target_origin: r.origin,
                            gain: 0.0,
                            target: gain,
                            score,
                        });
                    }
                }
            }
            candidates.sort_by(|a, b| {
                b.score
                    .total_cmp(&a.score)
                    .then(a.emitter.cmp(&b.emitter))
                    .then(a.layer.cmp(&b.layer))
            });
            candidates.truncate(TARGET_VOICES);
            // Normalize only unusually loud nearby mixtures, retaining quiet pools.
            let energy = candidates
                .iter()
                .map(|v| {
                    let a = (1.0 - (distance2(v.origin, eye).sqrt() - 256.0).max(0.0) * 0.0008)
                        .max(0.0);
                    (v.target * a).powi(2)
                })
                .sum::<f32>()
                .sqrt();
            let scale = 0.85 / energy.max(0.85);
            for voice in &mut self.voices {
                voice.target = 0.0;
            }
            for mut next in candidates {
                next.target *= scale;
                if let Some(v) = self
                    .voices
                    .iter_mut()
                    .find(|v| v.emitter == next.emitter && v.layer == next.layer)
                {
                    v.target = next.target;
                    v.score = next.score;
                    v.target_origin = next.origin;
                } else {
                    self.voices.push(next);
                }
            }
        }
        for v in &mut self.voices {
            v.gain += alpha * (v.target - v.gain);
            for (p, q) in v.origin.iter_mut().zip(v.target_origin) {
                *p += alpha * (q - *p);
            }
        }
        self.voices.retain(|v| v.target > 0.0 || v.gain > 0.004);
        self.voices.sort_by(|a, b| {
            (b.target > 0.0)
                .cmp(&(a.target > 0.0))
                .then(b.gain.total_cmp(&a.gain))
                .then(a.emitter.cmp(&b.emitter))
        });
        self.voices.truncate(MAX_RIVER_LOOP_VOICES);
        backend.set_river_loops(
            self.voices
                .iter()
                .map(|v| {
                    (
                        LoopRequest {
                            emitter: v.emitter,
                            qpath: v.clip.path.clone(),
                            origin: SoundOrigin::Fixed(v.origin),
                            volume: v.gain.clamp(0.0, 1.0),
                        },
                        Arc::clone(&v.clip.sound),
                    )
                })
                .collect(),
        );
    }
}
