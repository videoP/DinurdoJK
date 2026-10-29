//! Render Studio audio: non-destructive source clips, deterministic 48 kHz stereo
//! mixing, and asynchronous external-file decoding. No dependence on render FPS.
//! DM26 is an event log, NOT an embedded PCM track; it requires a separate
//! deterministic CGame event replay and is deliberately not silently substituted.
use rodio::Source;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc,
    },
    thread,
};

pub(super) const RATE: u32 = 48_000;
const MAX_SOURCE_SAMPLES: usize = 96_000_000; // bounded decoded-memory cache

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(super) enum StudioAudioFade {
    Linear,
    #[default]
    EqualPower,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct StudioGainKey {
    pub time: f64,
    pub gain: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct StudioAudioClip {
    pub path: String,
    pub start: f64,
    pub source_in: f64,
    pub source_out: f64,
    #[serde(default = "unity")]
    pub gain: f32,
    #[serde(default)]
    pub keys: Vec<StudioGainKey>,
    #[serde(default)]
    pub fade_in: f64,
    #[serde(default)]
    pub fade_out: f64,
    #[serde(default)]
    pub fade_curve: StudioAudioFade,
    /// A split preserves the original fade curve and its source clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<StudioAudioEnvelope>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(super) struct StudioAudioEnvelope {
    pub offset: f64,
    pub duration: f64,
    pub fade_in: f64,
    pub fade_out: f64,
}
fn unity() -> f32 {
    1.0
}
impl StudioAudioClip {
    pub fn duration(&self) -> f64 {
        (self.source_out - self.source_in).max(0.0)
    }
    pub fn end(&self) -> f64 {
        self.start + self.duration()
    }
    pub fn valid(&self) -> bool {
        !self.path.is_empty()
            && self.path.len() < 4096
            && self.start.is_finite()
            && self.start >= 0.0
            && self.source_in.is_finite()
            && self.source_in >= 0.0
            && self.source_out.is_finite()
            && self.source_out > self.source_in
            && self.end() <= 36000.0
            && self.gain.is_finite()
            && (0.0..=4.0).contains(&self.gain)
            && self.fade_in.is_finite()
            && self.fade_out.is_finite()
            && self.fade_in >= 0.0
            && self.fade_out >= 0.0
            && self.fade_in <= self.duration()
            && self.fade_out <= self.duration()
            && self.keys.len() <= 512
            && self.keys.iter().all(|k| {
                k.time.is_finite()
                    && k.time >= 0.0
                    && k.time <= self.duration()
                    && k.gain.is_finite()
                    && (0.0..=4.0).contains(&k.gain)
            })
            && self.keys.windows(2).all(|p| p[0].time < p[1].time)
            && self.envelope.is_none_or(|e| {
                e.offset.is_finite()
                    && e.offset >= 0.0
                    && e.duration.is_finite()
                    && e.duration > 0.0
                    && e.duration <= 36000.0
                    && e.offset + self.duration() <= e.duration + 0.00001
                    && e.fade_in.is_finite()
                    && (0.0..=e.duration).contains(&e.fade_in)
                    && e.fade_out.is_finite()
                    && (0.0..=e.duration).contains(&e.fade_out)
            })
    }
    /// Split without moving the source clock or changing the composite gain.
    pub fn split_at(&self, at: f64) -> Option<(Self, Self)> {
        if at <= self.start + 0.001 || at >= self.end() - 0.001 {
            return None;
        }
        if !at.is_finite() {
            return None;
        }
        let offset = at - self.start;
        let envelope = self.envelope.unwrap_or(StudioAudioEnvelope {
            offset: 0.0,
            duration: self.duration(),
            fade_in: self.fade_in,
            fade_out: self.fade_out,
        });
        let automation = self.automation_at(offset);
        let mut left = self.clone();
        left.source_out = self.source_in + offset;
        left.envelope = Some(envelope);
        left.fade_in = left.fade_in.min(left.duration());
        left.fade_out = (self.fade_out - (self.duration() - offset))
            .max(0.0)
            .min(offset);
        left.keys.retain(|k| k.time <= offset);
        if left.keys.is_empty() && !self.keys.is_empty() {
            left.add_key(0.0, self.automation_at(0.0));
        }
        left.add_key(offset, automation);
        let mut right = self.clone();
        right.start = at;
        right.source_in = left.source_out;
        right.envelope = Some(StudioAudioEnvelope {
            offset: envelope.offset + offset,
            ..envelope
        });
        right.fade_out = right.fade_out.min(right.duration());
        right.fade_in = (self.fade_in - offset).max(0.0).min(right.duration());
        right.keys.retain(|k| k.time >= offset);
        for key in &mut right.keys {
            key.time -= offset;
        }
        right.add_key(0.0, automation);
        Some((left, right))
    }
    /// Evaluate trims from the pointer-down snapshot, preserving source and
    /// automation clocks. Repeating the same drag position cannot accumulate.
    pub fn trimmed(&self, source_in: f64, source_out: f64) -> Self {
        let mut clip = self.clone();
        let source_in = source_in.clamp((self.source_in - self.start).max(0.0), source_out - 0.001);
        let delta = source_in - self.source_in;
        clip.start = self.start + delta;
        clip.source_in = source_in;
        clip.source_out = source_out.min(source_in + 36000.0 - clip.start);
        clip.keys = self
            .keys
            .iter()
            .filter_map(|key| {
                let time = key.time - delta;
                (time >= 0.0 && time <= clip.duration()).then_some(StudioGainKey {
                    time,
                    gain: key.gain,
                })
            })
            .collect();
        if !self.keys.is_empty() {
            clip.add_key(0.0, self.automation_at(delta));
        }
        let e = self.envelope.unwrap_or(StudioAudioEnvelope {
            offset: 0.0,
            duration: self.duration(),
            fade_in: self.fade_in,
            fade_out: self.fade_out,
        });
        clip.envelope = (e.offset + delta >= 0.0
            && e.offset + delta + clip.duration() <= e.duration + 0.000001)
            .then_some(StudioAudioEnvelope {
                offset: e.offset + delta,
                ..e
            });
        clip.fade_in = self.fade_in.min(clip.duration());
        clip.fade_out = self.fade_out.min(clip.duration());
        clip
    }
    pub fn add_key(&mut self, time: f64, gain: f32) -> usize {
        let time = time.clamp(0.0, self.duration());
        let gain = gain.clamp(0.0, 4.0);
        if let Some(i) = self
            .keys
            .iter()
            .position(|k| (k.time - time).abs() < 0.00001)
        {
            self.keys[i].gain = gain;
            return i;
        }
        if self.keys.is_empty() && time > 0.00001 {
            self.keys.push(StudioGainKey {
                time: 0.0,
                gain: 1.0,
            });
        }
        self.keys.push(StudioGainKey { time, gain });
        self.keys.sort_by(|a, b| a.time.total_cmp(&b.time));
        self.keys
            .iter()
            .position(|k| (k.time - time).abs() < 0.00001)
            .unwrap_or(0)
    }
    /// Automation excluding constant clip gain and fades; used when splitting
    /// so an edit does not accidentally bake a fade into new key values.
    pub fn automation_at(&self, local: f64) -> f32 {
        let t = local.clamp(0.0, self.duration());
        match self.keys.first() {
            None => 1.0,
            Some(first) if t <= first.time => first.gain,
            Some(_) => {
                let mut value = self.keys.last().map_or(1.0, |k| k.gain);
                for pair in self.keys.windows(2) {
                    if t <= pair[1].time {
                        let u = ((t - pair[0].time) / (pair[1].time - pair[0].time)) as f32;
                        value = pair[0].gain + (pair[1].gain - pair[0].gain) * u;
                        break;
                    }
                }
                value
            }
        }
    }
    pub fn gain_at(&self, absolute: f64) -> f32 {
        let t = absolute - self.start;
        if t < 0.0 || t >= self.duration() {
            return 0.0;
        }
        let automation = self.automation_at(t);
        let fade = |x: f64| -> f32 {
            let x = x.clamp(0.0, 1.0) as f32;
            match self.fade_curve {
                StudioAudioFade::Linear => x,
                StudioAudioFade::EqualPower => (x * std::f32::consts::FRAC_PI_2).sin(),
            }
        };
        let e = self.envelope.unwrap_or(StudioAudioEnvelope {
            offset: 0.0,
            duration: self.duration(),
            fade_in: self.fade_in,
            fade_out: self.fade_out,
        });
        let t = t + e.offset;
        let in_level = if e.fade_in > 0.0 {
            fade(t / e.fade_in)
        } else {
            1.0
        };
        let out_level = if e.fade_out > 0.0 {
            fade((e.duration - t) / e.fade_out)
        } else {
            1.0
        };
        self.gain * automation * in_level * out_level
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct StudioAudioTrack {
    pub name: String,
    #[serde(default = "unity")]
    pub gain: f32,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub solo: bool,
    #[serde(default)]
    pub clips: Vec<StudioAudioClip>,
}
impl StudioAudioTrack {
    pub fn new(number: usize) -> Self {
        Self {
            name: format!("Audio {number}"),
            gain: 1.0,
            mute: false,
            solo: false,
            clips: Vec::new(),
        }
    }
    pub fn valid(&self) -> bool {
        self.name.len() < 256
            && self.gain.is_finite()
            && (0.0..=4.0).contains(&self.gain)
            && self.clips.len() <= 512
            && self.clips.iter().all(StudioAudioClip::valid)
    }
}

pub(super) struct StudioDecodedAudio {
    pub rate: u32,
    /// Stereo f32 interleaved at the original source rate.
    pub samples: Vec<f32>,
    /// Low-resolution min/max thumbnail per 512 source frames.
    pub waveform: Vec<(f32, f32)>,
}
impl StudioDecodedAudio {
    pub fn duration(&self) -> f64 {
        self.samples.len() as f64 / 2.0 / self.rate as f64
    }
    fn at(&self, seconds: f64) -> (f32, f32) {
        let index = seconds * self.rate as f64;
        if index < 0.0 {
            return (0.0, 0.0);
        }
        let a = index.floor() as usize;
        let frames = self.samples.len() / 2;
        if a >= frames {
            return (0.0, 0.0);
        }
        let b = (a + 1).min(frames - 1);
        let f = (index - a as f64) as f32;
        (
            self.samples[a * 2] + (self.samples[b * 2] - self.samples[a * 2]) * f,
            self.samples[a * 2 + 1] + (self.samples[b * 2 + 1] - self.samples[a * 2 + 1]) * f,
        )
    }
}

fn decode(path: &Path) -> Result<StudioDecodedAudio, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut decoder = rodio::Decoder::builder()
        .with_data(std::io::BufReader::new(file))
        .with_gapless(true)
        .build()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let rate = decoder.sample_rate().get();
    let channels = usize::from(decoder.channels().get());
    if !(1..=2).contains(&channels) {
        return Err("Render Studio supports mono/stereo media".into());
    }
    let mut samples = Vec::new();
    let mut waveform = Vec::new();
    let mut low = 1.0f32;
    let mut high = -1.0f32;
    let mut frames = 0usize;
    loop {
        let Some(left) = decoder.next() else {
            break;
        };
        let right = if channels == 2 {
            decoder.next().unwrap_or(left)
        } else {
            left
        };
        if !left.is_finite() || !right.is_finite() {
            return Err("Non-finite decoded audio".into());
        }
        if samples.len() + 2 > MAX_SOURCE_SAMPLES {
            return Err("Source exceeds 96M decoded samples; split the media first".into());
        }
        samples.extend_from_slice(&[left, right]);
        low = low.min(left.min(right));
        high = high.max(left.max(right));
        frames += 1;
        if frames == 512 {
            waveform.push((low, high));
            low = 1.0;
            high = -1.0;
            frames = 0;
        }
    }
    if frames != 0 {
        waveform.push((low, high));
    }
    if samples.is_empty() {
        return Err("Audio file contains no samples".into());
    }
    Ok(StudioDecodedAudio {
        rate,
        samples,
        waveform,
    })
}

#[derive(Default)]
pub(super) struct StudioAudioRuntime {
    pub cache: HashMap<String, Arc<StudioDecodedAudio>>,
    pending: HashMap<String, mpsc::Receiver<Result<StudioDecodedAudio, String>>>,
    queued: VecDeque<String>,
    pub errors: HashMap<String, String>,
    pub import_path: String,
    /// When false, import copies into the project (the default).
    pub link_external: bool,
    pub pending_imports: VecDeque<(String, usize, f64)>,
    pub selected: Option<(usize, usize)>,
    pub selected_track: Option<usize>,
    pub selected_key: Option<usize>,
    pub drag: Option<(usize, usize, f32, f64)>,
    pub fade_drag: Option<(usize, usize, bool, f32, f64)>,
    /// Linked DM26 audio has its own fade handles, independent of PCM tracks.
    /// (demo index, fade-in?, original pointer X, original fade seconds).
    pub demo_fade_drag: Option<(usize, bool, f32, f64)>,
    /// (demo, key, pointer X/Y, original source time, original gain).
    pub demo_gain_drag: Option<(usize, usize, f32, f32, f64, f32)>,
    /// Inspector explicitly focused on a linked demo-audio clip.
    pub selected_demo_audio: Option<usize>,
    pub trim_drag: Option<(usize, usize, bool, f32, StudioAudioClip)>,
    pub key_drag: Option<(usize, usize, usize, f32, f32, f64, f32)>,
    pub pending_import: Option<(String, usize, f64)>,
    pub preview: Option<StudioAudioPreview>,
    pub preview_error: Option<String>,
    pub preview_transport: Option<(f64, super::Instant, bool)>,
}
impl StudioAudioRuntime {
    pub fn request(&mut self, path: &str) {
        if self.cache.contains_key(path)
            || self.pending.contains_key(path)
            || self.errors.contains_key(path)
            || self.queued.iter().any(|p| p == path)
        {
            return;
        }
        self.queued.push_back(path.to_owned());
    }
    pub fn poll(&mut self) {
        while self.pending.len() < 2 {
            let Some(key) = self.queued.pop_front() else {
                break;
            };
            let path = PathBuf::from(&key);
            let (sender, receiver) = mpsc::channel();
            thread::spawn(move || {
                let _ = sender.send(decode(&path));
            });
            self.pending.insert(key, receiver);
        }
        let mut ready = Vec::new();
        for (path, receiver) in &self.pending {
            match receiver.try_recv() {
                Ok(result) => ready.push((path.clone(), result)),
                Err(mpsc::TryRecvError::Disconnected) => {
                    ready.push((path.clone(), Err("Audio decode worker exited".into())))
                }
                Err(mpsc::TryRecvError::Empty) => (),
            }
        }
        for (path, result) in ready {
            self.pending.remove(&path);
            match result {
                Ok(audio)
                    if self.cache.values().map(|a| a.samples.len()).sum::<usize>()
                        + audio.samples.len()
                        <= MAX_SOURCE_SAMPLES =>
                {
                    self.cache.insert(path, Arc::new(audio));
                }
                Ok(_) => {
                    self.errors.insert(
                        path,
                        "Studio decoded audio cache exceeds 96M samples".into(),
                    );
                }
                Err(error) => {
                    self.errors.insert(path, error);
                }
            }
        }
    }
    pub fn mix(&self, tracks: &[StudioAudioTrack], begin: f64, frames: usize) -> Vec<f32> {
        let mut output = vec![0.0; frames * 2];
        let solo = tracks.iter().any(|t| t.solo);
        for track in tracks.iter().filter(|t| !t.mute && (!solo || t.solo)) {
            for clip in &track.clips {
                let Some(source) = self.cache.get(&clip.path) else {
                    continue;
                };
                let lo = (((clip.start - begin) * RATE as f64).ceil() as isize)
                    .clamp(0, frames as isize) as usize;
                let hi = (((clip.end() - begin) * RATE as f64).ceil() as isize)
                    .clamp(0, frames as isize) as usize;
                for i in lo..hi {
                    // Absolute sample index, not accumulated frame durations: no A/V drift.
                    let t = begin + (i as f64) / (RATE as f64);
                    let gain = track.gain * clip.gain_at(t);
                    let (l, r) = source.at(clip.source_in + t - clip.start);
                    output[2 * i] += l * gain;
                    output[2 * i + 1] += r * gain;
                }
            }
        }
        for sample in &mut output {
            *sample = sample.clamp(-1.0, 1.0);
        }
        output
    }
    pub fn all_ready(&self, tracks: &[StudioAudioTrack]) -> Result<(), String> {
        let solo = tracks.iter().any(|t| t.solo);
        for clip in tracks
            .iter()
            .filter(|t| !t.mute && (!solo || t.solo))
            .flat_map(|track| &track.clips)
        {
            if let Some(error) = self.errors.get(&clip.path) {
                return Err(format!("{}: {error}", clip.path));
            }
            if !self.cache.contains_key(&clip.path) {
                return Err(format!("Audio is not yet decoded: {}", clip.path));
            }
            if self.cache[&clip.path].duration() + 0.0001 < clip.source_out {
                return Err(format!("Source trim exceeds file length: {}", clip.path));
            }
        }
        Ok(())
    }
    /// Writes exact stereo 32-bit float WAV, beginning at the same IN as video.
    /// Bounded chunks avoid materializing an entire export in RAM.
    pub fn write_wav(
        &self,
        tracks: &[StudioAudioTrack],
        start: f64,
        frames: u64,
        path: &Path,
    ) -> Result<(), String> {
        let bytes = frames.checked_mul(8).ok_or("Audio WAV too long")?;
        if bytes > u32::MAX as u64 - 36 {
            return Err("Audio export exceeds WAV 4 GiB limit".into());
        }
        let mut file =
            std::io::BufWriter::new(std::fs::File::create(path).map_err(|e| e.to_string())?);
        file.write_all(b"RIFF").map_err(|e| e.to_string())?;
        file.write_all(&(bytes as u32 + 36).to_le_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(b"WAVEfmt ").map_err(|e| e.to_string())?;
        file.write_all(&16u32.to_le_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(&3u16.to_le_bytes())
            .map_err(|e| e.to_string())?; // IEEE_FLOAT
        file.write_all(&2u16.to_le_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(&RATE.to_le_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(&(RATE * 8).to_le_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(&8u16.to_le_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(&32u16.to_le_bytes())
            .map_err(|e| e.to_string())?;
        file.write_all(b"data").map_err(|e| e.to_string())?;
        file.write_all(&(bytes as u32).to_le_bytes())
            .map_err(|e| e.to_string())?;
        let mut written = 0u64;
        while written < frames {
            let block = (frames - written).min(8192) as usize;
            let mix = self.mix(tracks, start + written as f64 / RATE as f64, block);
            let mut buffer = Vec::with_capacity(block * 8);
            for s in mix {
                buffer.extend_from_slice(&s.to_le_bytes());
            }
            file.write_all(&buffer).map_err(|e| e.to_string())?;
            written += block as u64;
        }
        file.flush().map_err(|e| e.to_string())
    }
}

/// Allocate a seek-free, bounded stereo f32 WAV for native DM26 capture.
/// Frames are known from the output timeline, so its RIFF header is final.
pub(super) fn new_capture_wav(path:&Path,frames:u64)->Result<std::fs::File,String> {
    let bytes=frames.checked_mul(8).ok_or("Audio capture is too large")?;
    if bytes>u32::MAX as u64-36 {return Err("Audio capture exceeds WAV 4 GiB".into());}
    let mut file=std::fs::File::create(path).map_err(|e|e.to_string())?;
    file.write_all(b"RIFF").map_err(|e|e.to_string())?;
    file.write_all(&(bytes as u32+36).to_le_bytes()).map_err(|e|e.to_string())?;
    file.write_all(b"WAVEfmt ").map_err(|e|e.to_string())?;
    file.write_all(&16u32.to_le_bytes()).map_err(|e|e.to_string())?;
    file.write_all(&3u16.to_le_bytes()).map_err(|e|e.to_string())?;
    file.write_all(&2u16.to_le_bytes()).map_err(|e|e.to_string())?;
    file.write_all(&RATE.to_le_bytes()).map_err(|e|e.to_string())?;
    file.write_all(&(RATE*8).to_le_bytes()).map_err(|e|e.to_string())?;
    file.write_all(&8u16.to_le_bytes()).map_err(|e|e.to_string())?;
    file.write_all(&32u16.to_le_bytes()).map_err(|e|e.to_string())?;
    file.write_all(b"data").map_err(|e|e.to_string())?;
    file.write_all(&(bytes as u32).to_le_bytes()).map_err(|e|e.to_string())?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn clip() -> StudioAudioClip {
        StudioAudioClip {
            path: "x.wav".into(),
            start: 2.0,
            source_in: 0.0,
            source_out: 4.0,
            gain: 1.0,
            keys: Vec::new(),
            fade_in: 1.0,
            fade_out: 1.0,
            fade_curve: StudioAudioFade::Linear,
            envelope: None,
        }
    }
    #[test]
    fn automation_and_fades() {
        let mut c = clip();
        assert_eq!(c.gain_at(1.0), 0.0);
        assert!((c.gain_at(2.5) - 0.5).abs() < 1e-6);
        assert!((c.gain_at(5.5) - 0.5).abs() < 1e-6);
        c.add_key(2.0, 0.25);
        assert!((c.gain_at(4.0) - 0.25).abs() < 1e-6);
    }
    #[test]
    fn independent_overlapping_clips_mix() {
        let sample = Arc::new(StudioDecodedAudio {
            rate: RATE,
            samples: vec![0.25; RATE as usize * 2 * 4],
            waveform: vec![],
        });
        let mut rt = StudioAudioRuntime::default();
        rt.cache.insert("x.wav".into(), sample);
        let a = StudioAudioTrack {
            name: "A".into(),
            gain: 1.0,
            mute: false,
            solo: false,
            clips: vec![StudioAudioClip {
                fade_in: 0.0,
                fade_out: 0.0,
                ..clip()
            }],
        };
        let b = a.clone();
        let mix = rt.mix(&[a, b], 3.0, 32);
        assert!(mix.iter().all(|x| (*x - 0.5).abs() < 1e-6));
    }
    #[test]
    fn split_preserves_source_sample_and_curve() {
        let mut before = clip();
        before.fade_in = 0.0;
        before.fade_out = 0.0;
        before.add_key(1.0, 0.5);
        before.add_key(3.0, 1.0);
        let (a, b) = before.split_at(4.0).unwrap();
        assert_eq!(a.end(), b.start);
        assert!((a.automation_at(a.duration()) - b.automation_at(0.0)).abs() < 1e-6);
        assert!((a.source_out - b.source_in).abs() < 1e-9);
        for time in [2.1, 2.5, 3.0, 3.9, 4.1, 4.5, 5.0, 5.9] {
            let changed = if time < a.end() {
                a.gain_at(time)
            } else {
                b.gain_at(time)
            };
            assert!(
                (changed - before.gain_at(time)).abs() < 1e-5,
                "discontinuity at {time}"
            );
        }
    }
    #[test]
    fn solo_and_mute_track_routing() {
        let source = Arc::new(StudioDecodedAudio {
            rate: RATE,
            samples: vec![0.2; RATE as usize * 2 * 4],
            waveform: vec![],
        });
        let mut rt = StudioAudioRuntime::default();
        rt.cache.insert("x.wav".into(), source);
        let c = StudioAudioClip {
            fade_in: 0.0,
            fade_out: 0.0,
            ..clip()
        };
        let t = StudioAudioTrack {
            name: "test".into(),
            gain: 1.0,
            mute: false,
            solo: false,
            clips: vec![c],
        };
        let mut solo = t.clone();
        solo.solo = true;
        assert!((rt.mix(&[t.clone(), solo], 3.0, 1)[0] - 0.2).abs() < 1e-5);
        let mut muted = t;
        muted.mute = true;
        assert_eq!(rt.mix(&[muted], 3.0, 1)[0], 0.0);
    }
    #[test]
    fn split_inside_overlapping_fades_preserves_every_sample() {
        for curve in [StudioAudioFade::Linear, StudioAudioFade::EqualPower] {
            let mut original = clip();
            original.fade_in = 3.5;
            original.fade_out = 3.0;
            original.fade_curve = curve;
            original.add_key(1.0, 0.4);
            original.add_key(3.0, 1.2);
            let (a, b) = original.split_at(3.0).unwrap();
            let (b, c) = b.split_at(4.5).unwrap();
            assert!(a.valid() && b.valid() && c.valid());
            for i in 0..4000 {
                let t = 2.0 + i as f64 / 1000.0;
                let part = if t < a.end() {
                    &a
                } else if t < b.end() {
                    &b
                } else {
                    &c
                };
                assert!(
                    (original.gain_at(t) - part.gain_at(t)).abs() < 0.00001,
                    "fade discontinuity at {t}"
                );
            }
        }
    }
    #[test]
    fn wav_round_trip_uses_export_in_point_and_resamples() {
        let path =
            std::env::temp_dir().join(format!("djk-studio-audio-{}.wav", std::process::id()));
        let mut rt = StudioAudioRuntime::default();
        rt.cache.insert(
            "x.wav".into(),
            Arc::new(StudioDecodedAudio {
                rate: 24000,
                samples: (0..96000).flat_map(|i| [i as f32 / 96000.0; 2]).collect(),
                waveform: Vec::new(),
            }),
        );
        let c = StudioAudioClip {
            fade_in: 0.0,
            fade_out: 0.0,
            ..clip()
        };
        let tracks = [StudioAudioTrack {
            name: "test".into(),
            gain: 0.5,
            mute: false,
            solo: false,
            clips: vec![c],
        }];
        rt.write_wav(&tracks, 3.25, 4800, &path).unwrap();
        let decoded = decode(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        let expected = rt.mix(&tracks, 3.25, 4800);
        assert_eq!(decoded.rate, RATE);
        assert_eq!(decoded.samples.len(), expected.len());
        for (a, b) in decoded.samples.iter().zip(expected) {
            assert!((a - b).abs() < 0.000001);
        }
    }
    #[test]
    fn project_backcompat_and_audio_round_trip() {
        let old = r#"{"version":7,"model":"","with_sabers":false,"background":"black","background_map":"","clips":[],"camera":[]}"#;
        let mut project: super::super::render_studio_timeline::StudioProject =
            serde_json::from_str(old).unwrap();
        assert!(project.audio_tracks.is_empty());
        project.version = 8;
        let (left, right) = clip().split_at(2.5).unwrap();
        project.audio_tracks = vec![StudioAudioTrack {
            clips: vec![left, right],
            ..StudioAudioTrack::new(1)
        }];
        let json = serde_json::to_string(&project).unwrap();
        let loaded: super::super::render_studio_timeline::StudioProject =
            serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.audio_tracks.len(), 1);
        assert!(loaded.audio_tracks[0].valid());
        let t = 2.75;
        assert!((loaded.audio_tracks[0].clips[1].gain_at(t) - clip().gain_at(t)).abs() < 0.00001);
    }
    #[test]
    fn source_trim_preserves_automation_and_is_pointer_anchored() {
        let mut original = clip();
        original.add_key(0.0, 0.2);
        original.add_key(3.0, 1.4);
        let a = original.trimmed(0.5, 3.0);
        let b = original.trimmed(0.5, 3.0);
        assert!(a.valid());
        assert_eq!(a.start, b.start);
        assert_eq!(a.keys, b.keys);
        assert_eq!(a.start, 2.5);
        assert_eq!(a.end(), 5.0);
        for i in 0..2500 {
            let t = 2.5 + i as f64 / 1000.0;
            assert!((a.gain_at(t) - original.gain_at(t)).abs() < 0.00001);
        }
    }
}

/// Imported audio has its own output bus. Demo sounds belong to each Studio
/// session and are weighted by the demo transition, never the gameplay session.
pub(super) struct StudioAudioPreview {
    _device: rodio::MixerDeviceSink,
    player: rodio::Player,
    played: Arc<AtomicU64>,
    start: f64,
    enqueued_until: f64,
    last_clock: f64,
}
impl StudioAudioPreview {
    fn append(&mut self, samples: Vec<f32>) {
        let offset = ((self.enqueued_until - self.start) * RATE as f64).round() as u64;
        self.enqueued_until += samples.len() as f64 / (2.0 * RATE as f64);
        let played = Arc::clone(&self.played);
        let source = rodio::buffer::SamplesBuffer::new(
            rodio::nz!(2),
            std::num::NonZeroU32::new(RATE).unwrap(),
            samples,
        )
        .track_position()
        .periodic_access(std::time::Duration::from_millis(5), move |source| {
            played.store(
                offset + (source.get_pos().as_secs_f64() * RATE as f64) as u64,
                Ordering::Relaxed,
            );
        });
        self.player.append(source);
    }
    fn new(start: f64, samples: Vec<f32>) -> Result<Self, String> {
        let mut device =
            rodio::DeviceSinkBuilder::open_default_sink().map_err(|e| e.to_string())?;
        device.log_on_drop(false);
        let (player, output) = rodio::Player::new();
        let mut preview = Self {
            _device: device,
            player,
            played: Arc::new(AtomicU64::new(0)),
            start,
            enqueued_until: start,
            last_clock: start,
        };
        // Attach with real stereo samples already queued; no silent lead-in.
        preview.append(samples);
        preview._device.mixer().add(output);
        Ok(preview)
    }
}
impl StudioAudioRuntime {
    pub fn tick_preview(
        &mut self,
        tracks: &[StudioAudioTrack],
        playing: bool,
        time: f64,
        volume: f32,
    ) {
        if !playing || tracks.iter().all(|t| t.clips.is_empty()) {
            self.preview = None;
            self.preview_error = None;
            return;
        }
        if self.preview_error.is_some() {
            return;
        }
        let reset = self.preview.as_ref().is_none_or(|p| {
            time < p.last_clock - 0.002
                || time > p.enqueued_until + 0.025
                || (time - (p.start + p.played.load(Ordering::Relaxed) as f64 / RATE as f64)).abs()
                    > 0.12
        });
        if reset {
            match StudioAudioPreview::new(time, self.mix(tracks, time, 2400)) {
                Ok(preview) => self.preview = Some(preview),
                Err(error) => {
                    self.preview = None;
                    self.preview_error = Some(error);
                    return;
                }
            }
        }
        while self
            .preview
            .as_ref()
            .is_some_and(|p| p.enqueued_until < time + 0.10)
        {
            let begin = self.preview.as_ref().unwrap().enqueued_until;
            let samples = self.mix(tracks, begin, 2400);
            self.preview.as_mut().unwrap().append(samples);
        }
        if let Some(preview) = self.preview.as_mut() {
            preview.last_clock = time;
            preview.player.set_volume(volume);
        }
    }
}

impl super::App {
    pub(in crate::app) fn studio_queue_audio_files(&mut self, files: Vec<PathBuf>) {
        if self.render_studio.timeline.audio_tracks.is_empty() {
            self.render_studio.timeline.audio_tracks.push(StudioAudioTrack::new(1));
            self.render_studio.timeline.audio_expanded.push(false);
        }
        let track = self.render_studio.audio.selected_track
            .or_else(||self.render_studio.audio.selected.map(|s|s.0)).unwrap_or(0)
            .min(self.render_studio.timeline.audio_tracks.len()-1);
        let start = self.render_studio.timeline_time();
        let mut queued = 0;
        for source in files {
            let extension = source.extension().and_then(|e|e.to_str()).unwrap_or("").to_ascii_lowercase();
            if !["wav","mp3","flac","ogg"].contains(&extension.as_str()) || !source.is_file() {
                self.render_studio.status = format!("Unsupported audio source: {}",source.display());
                continue;
            }
            let result = if self.render_studio.audio.link_external {
                std::fs::canonicalize(&source).map_err(|e|e.to_string())
            } else {self.studio_copy_audio(&source)};
            match result {
                Ok(path) => {
                    let path=path.to_string_lossy().into_owned();
                    self.render_studio.audio.errors.remove(&path);
                    self.render_studio.audio.request(&path);
                    self.render_studio.audio.pending_imports.push_back((path,track,start));
                    queued+=1;
                }
                Err(error) => self.render_studio.status=format!("Audio import failed: {error}"),
            }
        }
        if queued>0 {self.render_studio.status=format!("Queued {queued} audio file(s) for decoding");}
    }

    pub(in crate::app) fn studio_poll_audio_import(&mut self) {
        self.render_studio.audio.poll();
        if self.render_studio.export.is_some() || self.render_studio.still.is_some() {return;}
        if self.render_studio.audio.pending_import.is_none() {
            self.render_studio.audio.pending_import=self.render_studio.audio.pending_imports.pop_front();
        }
        let Some((path, track, start)) = self.render_studio.audio.pending_import.clone() else {
            return;
        };
        if let Some(error) = self.render_studio.audio.errors.get(&path) {
            self.render_studio.status = format!("Audio import failed: {error}");
            self.render_studio.audio.pending_import = None;
        } else if let Some(source) = self.render_studio.audio.cache.get(&path) {
            let duration = source.duration().min((36000.0 - start).max(0.0));
            if duration > 0.001 {
                let clip = StudioAudioClip {
                    path: path.clone(),
                    start,
                    source_in: 0.0,
                    source_out: duration,
                    gain: 1.0,
                    keys: Vec::new(),
                    fade_in: 0.0,
                    fade_out: 0.0,
                    fade_curve: StudioAudioFade::EqualPower,
                    envelope: None,
                };
                if let Some(row) = self.render_studio.timeline.audio_tracks.get_mut(track) {
                    if row.clips.len()>=512 {
                        self.render_studio.status="Audio track already has 512 clips".into();
                        self.render_studio.audio.pending_import=None;return;
                    }
                    row.clips.push(clip);
                    self.render_studio.audio.selected = Some((track, row.clips.len() - 1));
                    self.render_studio.audio.selected_track = Some(track);
                    self.render_studio.status = format!("Imported {path} ({duration:.2}s)");
                    self.render_studio.timeline.clear_selection();self.render_studio.selected_background=false;
                }
            }
            self.render_studio.audio.pending_import = None;
        }
    }

    pub(in crate::app) fn tick_render_studio_audio(&mut self) {
        let active = self.front_end && self.frontend_page == super::FrontendPage::RenderStudio;
        if !active {
            self.render_studio.audio.preview_transport = None;
            self.render_studio.audio.preview = None;
            return;
        }
        self.studio_poll_audio_import();
        let clock = self.render_studio.timeline_time();
        let playing = self.render_studio.timeline.playing
            && !self.render_studio.timeline.demos.iter().any(|demo|demo.audio.solo)
            && self.render_studio.source_wait.is_none()
            && self.render_studio.export.is_none()
            && self.render_studio.still.is_none();
        // Imported tracks are not game music: the in-game Music slider
        // (s_musicvolume) must not silence them. Exports ignore it as well.
        let volume = if self.audio.mute_when_unfocused && !self.window_focused {
            0.0
        } else {
            self.render_studio.preview_volume
        };
        let studio = &mut self.render_studio;
        let transport = (
            studio.timeline.playhead,
            studio.timeline.play_started,
            studio.timeline.playing,
        );
        if studio.audio.preview_transport != Some(transport) {
            studio.audio.preview_transport = Some(transport);
            studio.audio.preview = None;
            studio.audio.preview_error = None;
        }
        let old_error = studio.audio.preview_error.clone();
        for clip in studio.timeline.audio_tracks.iter().flat_map(|t| &t.clips) {
            studio.audio.request(&clip.path);
        }
        studio
            .audio
            .tick_preview(&studio.timeline.audio_tracks, playing, clock, volume);
        if old_error.is_none() {
            if let Some(error) = studio.audio.preview_error.as_ref() {
                studio.status = format!("Audio preview unavailable: {error}");
            }
        }
    }
}
