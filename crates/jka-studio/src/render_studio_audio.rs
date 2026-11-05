//! Render Studio audio project data: clips, gain keys, fades and tracks.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StudioAudioFade {
    Linear,
    #[default]
    EqualPower,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StudioGainKey {
    pub time: f64,
    pub gain: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StudioAudioClip {
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
pub struct StudioAudioEnvelope {
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
pub struct StudioAudioTrack {
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

