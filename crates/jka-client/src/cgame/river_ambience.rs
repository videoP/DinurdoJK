//! Authored river recordings controlled by world-space hydraulic samples.
//! Independent of the underwater switch and the selected acoustic backend.
use crate::audio::{
    AudioBackend, LoopRequest, MAX_RIVER_LOOP_VOICES, RIVER_LOOP_BANKS, RIVER_DRIP_PATHS,
    RegisteredSound, SoundAssets,
    SoundDecodeResult, SoundOrigin,
};
use jka_river::river_ambience::{self, Region};
use std::{
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

// Bank order is shared with the audio backend's exact-path allowlist.
const LAYERS: usize = RIVER_LOOP_BANKS.len();
const DRIPS: usize = LAYERS;
const BANKS: usize = LAYERS + 1;
const CLIP_COUNT: usize = {
    let mut count = RIVER_DRIP_PATHS.len();
    let mut layer = 0;
    while layer < LAYERS {
        count += RIVER_LOOP_BANKS[layer].len();
        layer += 1;
    }
    count
};
// One of each role first, then the remaining variants. Drips are events,
// never loop requests. Every packed flow/still/drip recording is reachable.
fn bank_path(index: usize) -> Option<(usize, &'static str)> {
    (0..RIVER_DRIP_PATHS.len())
        .flat_map(|variant| (0..BANKS).filter_map(move |layer| {
            let path = if layer == DRIPS {
                RIVER_DRIP_PATHS.get(variant).copied()
            } else {
                RIVER_LOOP_BANKS[layer].get(variant).copied()
            };
            path.map(|path| (layer, path))
        }))
        .nth(index)
}
const INTERVAL: Duration = Duration::from_millis(100);
const TARGET_VOICES: usize = MAX_RIVER_LOOP_VOICES - 4; // leave room for outgoing crossfades

#[derive(Clone)]
struct Clip {
    path: String,
    sound: Arc<RegisteredSound>,
    level: f32,
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
    clips: [Vec<Clip>; BANKS],
    next_clip: usize,
    pending: Option<(usize, String, mpsc::Receiver<SoundDecodeResult>)>,
    voices: Vec<Voice>,
    next_drip: Option<Instant>,
    drip_sequence: u64,
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
            next_drip: None,
            drip_sequence: 0,
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
// Keep the original speed thresholds and hydraulic power budget. Fold
// specialist layers into the closest real recording family, not a synthetic
// rapids/eddy loop. A cascade requires drop evidence; bubbles require agitation.
fn powers(r: &Region) -> [f32; LAYERS] {
    let v = r.speed.max(0.0);
    let t = r.turbulence.clamp(0.0, 1.0);
    let curl = r.curl.clamp(0.0, 1.0);
    let fall = r.drop.clamp(0.0, 1.0);
    let a = smooth(0.04, 0.35, v);
    let b = smooth(0.35, 1.25, v);
    let c = smooth(1.25, 3.0, v);
    let base = (1.0 - t * 0.85) * (1.0 - fall * 0.85);
    let fast = smooth(0.5, 2.5, v);
    // Wave recordings are a poor fit for a narrow, motionless trickle.
    let broad = 0.15 + 0.85 * smooth(0.15, 0.8, r.coverage);
    let mut p = [
        (1.0 - a) * base * broad,
        a * (1.0 - b) * base,
        b * (1.0 - c) * base + t * (1.0 - fast) * 0.65,
        c * base + t * fast * (1.0 - fall * 0.6),
        fall * 1.5,
        t * curl * (1.0 - fall) * 0.55 + t * curl * smooth(0.55, 0.95, t) * fast * 0.8,
    ];
    let sum = p.iter().sum::<f32>().max(1.0);
    let intensity = 0.10 + 0.22 * smooth(0.08, 2.5, v) + 0.25 * t + 0.40 * fall;
    for x in &mut p {
        *x *= r.coverage.clamp(0.0, 1.0) * intensity * intensity / sum;
    }
    p
}

// Small, narrow drop arrivals can make distinct droplets audible. Do not
// trigger these from still water, ordinary flat flow or a broad waterfall.
fn drip_strength(r: &Region) -> f32 {
    smooth(0.04, 0.23, r.drop)
        * (1.0 - smooth(0.32, 0.65, r.drop))
        * (1.0 - smooth(0.10, 0.38, r.coverage))
        * smooth(0.01, 0.07, r.coverage)
        * (1.0 - smooth(0.8, 2.0, r.speed))
        * (1.0 - smooth(0.45, 0.85, r.turbulence))
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
        self.next_drip = None;
        self.drip_sequence = 0;
        self.last_eye = None;
        self.last = None;
        self.last_frame = None;
        if let Some(b) = backend {
            b.set_river_loops(Vec::new());
            b.clear_river_drips();
        }
    }
    pub(super) fn retry_assets(&mut self) {
        self.clips = std::array::from_fn(|_| Vec::new());
        self.next_clip = 0;
        self.pending = None;
        self.last = None;
    }
    fn accept_clip(&mut self, layer: usize, path: String, assets: &mut SoundAssets) {
        match assets.register(&path) {
            Ok(sound) => {
                // The field recordings share a peak target, not an RMS target.
                // Equalize average loop loudness to -24 dBFS, within +/-12 dB,
                // without copying PCM or flattening the waveform's dynamics.
                let level = if layer == DRIPS { 1.0 } else {
                    (0.0631 / sound.rms().max(0.0001)).clamp(0.25, 4.0)
                };
                self.clips[layer].push(Clip { path, sound, level });
            }
            Err(e) => eprintln!("RIVER AUDIO: {path}: {e}"),
        }
    }
    fn poll_bank(&mut self, assets: &mut SoundAssets) {
        if let Some((layer, path, rx)) = self.pending.take() {
            match rx.try_recv() {
                Ok(result) => {
                    assets.queue_predecoded([result]);
                    self.accept_clip(layer, path, assets);
                }
                Err(mpsc::TryRecvError::Empty) => {
                    self.pending = Some((layer, path, rx));
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    eprintln!("RIVER AUDIO: decode worker disconnected: {path}");
                }
            }
        }
        // One new VFS read per control tick; decode on the existing worker.
        let Some((layer, path)) = bank_path(self.next_clip) else { return; };
        self.next_clip += 1;
        let path = path.to_owned();
        if let Some(job) = assets.stage_decode(&path) {
            let (tx, rx) = mpsc::channel();
            match std::thread::Builder::new()
                .name("river-audio-decode".into())
                .spawn(move || {
                    let _ = tx.send(job.decode());
                }) {
                Ok(_) => self.pending = Some((layer, path, rx)),
                Err(error) => eprintln!("RIVER AUDIO: cannot start decoder for {path}: {error}"),
            }
        } else {
            self.accept_clip(layer, path, assets);
        }
    }
    pub(super) fn update(
        &mut self,
        assets: &mut SoundAssets,
        backend: &mut AudioBackend,
        eye: [f32; 3],
    ) {
        if self.volume <= 0.0 || !crate::river_runtime::audio_rivers_present() {
            if !self.voices.is_empty() || self.next_drip.is_some() {
                self.clear(Some(backend));
            }
            return;
        }
        if self.next_clip >= CLIP_COUNT
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
            let mut drip = None::<(&Region, f32, f32)>;
            if let Some(snapshot) = snapshot {
                for r in snapshot.regions.iter() {
                    let d = distance2(r.origin, eye).sqrt();
                    let attenuation = (1.0 - (d - 256.0).max(0.0) * 0.0008).max(0.0);
                    if attenuation <= 0.0 {
                        continue;
                    }
                    let id = emitter(r);
                    let mut p = powers(r);
                    // Missing optional recordings degrade only to a compatible
                    // current texture. Never substitute still surf for drops,
                    // or bubbles for a calm pool. No legacy placeholder reads.
                    for (layer, fallback) in [(1, 2), (2, 1), (3, 2), (4, 3), (5, 2)] {
                        if self.clips[layer].is_empty() && !self.clips[fallback].is_empty() {
                            p[fallback] += p[layer];
                            p[layer] = 0.0;
                        }
                    }
                    if d < 768.0 && !self.clips[DRIPS].is_empty() {
                        let strength = drip_strength(r);
                        let score = strength * attenuation * (1.0 - d / 768.0);
                        if score > 0.05 && drip.is_none_or(|(_, _, best)| score > best) {
                            drip = Some((r, strength, score));
                        }
                    }
                    for (layer, power) in p.into_iter().enumerate() {
                        let clips = &self.clips[layer];
                        let gain = power.sqrt() * self.volume;
                        if clips.is_empty() || gain * attenuation < 0.008 {
                            continue;
                        }
                        // Prefer the same recording for this region once the
                        // bank is complete. Newly decoded variants can replace
                        // the initial one via the normal fade, not a hard cut.
                        let clip = &clips[(mix64(id ^ layer as u64) % clips.len() as u64) as usize];
                        let previous = self.voices.iter().find(|v| {
                            v.emitter == id && v.layer == layer && v.clip.path == clip.path
                        });
                        let score =
                            gain * attenuation * if previous.is_some() { 1.18 } else { 1.0 };
                        candidates.push(Voice {
                            emitter: id,
                            layer,
                            clip: clip.clone(),
                            origin: r.origin,
                            target_origin: r.origin,
                            gain: 0.0,
                            target: gain,
                            score,
                        });
                    }
                }
                if backend.rate() > 0.0 && self.next_drip.is_none_or(|at| now >= at) {
                    if let Some((r, strength, _)) = drip {
                        let clips = &self.clips[DRIPS];
                        let seed = mix64(emitter(r) ^ self.drip_sequence);
                        // Cycle through the available variants; jitter spacing
                        // deterministically to avoid a clock-like repeated drip.
                        let index = (self.drip_sequence % clips.len() as u64) as usize;
                        let clip = &clips[index];
                        if backend.try_play_river_drip(&clip.path, Arc::clone(&clip.sound), r.origin,
                            self.volume * (0.15 + 0.30 * strength)) {
                            self.drip_sequence = self.drip_sequence.wrapping_add(1);
                        }
                        let jitter = (seed & 255) as f32 / 255.0;
                        self.next_drip = Some(now + Duration::from_secs_f32(1.2 + (1.0 - strength) * 2.5 + jitter));
                    }
                }
            }
            candidates.sort_by(|a, b| {
                b.score
                    .total_cmp(&a.score)
                    .then(a.emitter.cmp(&b.emitter))
                    .then(a.layer.cmp(&b.layer))
                    .then(a.clip.path.cmp(&b.clip.path))
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
                    .find(|v| v.emitter == next.emitter && v.layer == next.layer && v.clip.path == next.clip.path)
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
                            volume: (v.gain * v.clip.level).clamp(0.0, 1.0),
                        },
                        Arc::clone(&v.clip.sound),
                    )
                })
                .collect(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn region(speed: f32, coverage: f32, turbulence: f32, curl: f32, drop: f32) -> Region {
        Region { river: 1, bin: [0, 0], origin: [0.0; 3], coverage, speed, turbulence, curl, drop }
    }

    #[test]
    fn every_recording_is_staged_once() {
        let paths: HashSet<_> = (0..CLIP_COUNT).map(|i| bank_path(i).unwrap().1).collect();
        assert_eq!(paths.len(), CLIP_COUNT);
        assert_eq!(CLIP_COUNT, 21);
        assert!(bank_path(CLIP_COUNT).is_none());
        for path in RIVER_LOOP_BANKS.iter().flat_map(|bank| bank.iter().copied()).chain(RIVER_DRIP_PATHS) {
            assert!(paths.contains(path));
        }
    }

    #[test]
    fn hydraulic_roles_follow_the_solved_water() {
        let still = powers(&region(0.0, 1.0, 0.0, 0.0, 0.0));
        assert!(still[0] > 0.0 && still[1..].iter().all(|p| *p == 0.0));
        let calm = powers(&region(0.35, 1.0, 0.0, 0.0, 0.0));
        assert!(calm[1] > 0.0 && calm[4] == 0.0 && calm[5] == 0.0);
        let fast = powers(&region(3.0, 1.0, 1.0, 0.0, 0.0));
        assert!(fast[3] > fast[2] && fast[4] == 0.0 && fast[5] == 0.0);
        let falls = powers(&region(1.0, 1.0, 0.8, 0.0, 1.0));
        assert!(falls[4] > falls[3]);
        let eddy = powers(&region(0.5, 1.0, 0.8, 0.9, 0.0));
        assert!(eddy[5] > 0.0 && eddy[4] == 0.0);
        assert_eq!(powers(&region(3.0, 0.0, 1.0, 1.0, 1.0)), [0.0; LAYERS]);
    }

    #[test]
    fn drops_are_sparse_events_not_generic_water_loops() {
        assert!(drip_strength(&region(0.4, 0.15, 0.2, 0.0, 0.2)) > 0.1);
        assert_eq!(drip_strength(&region(0.4, 0.15, 0.2, 0.0, 0.0)), 0.0);
        assert_eq!(drip_strength(&region(0.4, 1.0, 0.2, 0.0, 0.2)), 0.0);
        assert_eq!(drip_strength(&region(3.0, 0.15, 1.0, 1.0, 1.0)), 0.0);
    }
}
