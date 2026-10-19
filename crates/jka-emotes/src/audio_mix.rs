//! Offline sound-graph rendering. Runtime needs only ordinary mono PCM WAVs.
use crate::compiler::{read, relative, text};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

struct Wave {
    rate: u32,
    samples: Vec<f32>,
}
struct Curve(Vec<(f64, f64, String, f64, f64)>);
impl Curve {
    fn parse(value: &Value, pitch: bool) -> Result<Self, String> {
        if value.is_null() {
            return Ok(Self(Vec::new()));
        }
        let keys = value
            .as_array()
            .filter(|a| a.len() <= 4096)
            .ok_or("invalid envelope keys")?;
        let mut curve = Vec::new();
        for key in keys {
            let t = key["Time"]
                .as_f64()
                .filter(|n| n.is_finite())
                .ok_or("invalid envelope time")?;
            let v = key["Value"]
                .as_f64()
                .filter(|n| n.is_finite() && *n >= if pitch { 0.01 } else { -64.0 } && *n <= 64.0)
                .ok_or("invalid envelope value")?;
            if curve
                .last()
                .is_some_and(|p: &(f64, f64, String, f64, f64)| p.0 >= t)
            {
                return Err("envelope keys must be increasing".into());
            }
            let tangent = |name: &str| key[name].as_f64().unwrap_or(0.0);
            let leave = tangent("LeaveTangent");
            let arrive = tangent("ArriveTangent");
            if !leave.is_finite() || !arrive.is_finite() {
                return Err("invalid envelope tangent".into());
            }
            curve.push((
                t,
                v,
                key["InterpMode"]
                    .as_str()
                    .unwrap_or("RCIM_Linear")
                    .to_owned(),
                leave,
                arrive,
            ));
        }
        Ok(Self(curve))
    }
    fn sample(&self, t: f64) -> f64 {
        let Some(first) = self.0.first() else {
            return 1.0;
        };
        if t <= first.0 {
            return first.1;
        }
        for pair in self.0.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            if t <= b.0 {
                let span = b.0 - a.0;
                let x = (t - a.0) / span;
                return match a.2.as_str() {
                    "RCIM_Constant" => a.1,
                    "RCIM_Cubic" => {
                        (2.0 * x * x * x - 3.0 * x * x + 1.0) * a.1
                            + (x * x * x - 2.0 * x * x + x) * span * a.3
                            + (-2.0 * x * x * x + 3.0 * x * x) * b.1
                            + (x * x * x - x * x) * span * b.4
                    }
                    _ => a.1 + (b.1 - a.1) * x,
                };
            }
        }
        self.0.last().unwrap().1
    }
}
fn decode(bytes: &[u8]) -> Result<Wave, String> {
    crate::validate_wav(bytes)?;
    let mut fmt = None;
    let mut data = None;
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let chunk = bytes
            .get(at + 8..at + 8 + size)
            .ok_or("truncated WAV chunk")?;
        match &bytes[at..at + 4] {
            b"fmt " => fmt = Some(chunk),
            b"data" => data = Some(chunk),
            _ => {}
        }
        at += 8 + size + size % 2;
    }
    let fmt = fmt.ok_or("missing WAV format")?;
    let u16at = |n| u16::from_le_bytes(fmt[n..n + 2].try_into().unwrap());
    let kind = u16at(0);
    let channels = u16at(2) as usize;
    let rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
    let bits = u16at(14);
    let stride = bits as usize / 8;
    let mut samples = Vec::new();
    for frame in data
        .ok_or("missing WAV data")?
        .chunks_exact(stride * channels)
    {
        let mut value = 0.0;
        for raw in frame.chunks_exact(stride) {
            value += match (kind, bits) {
                (1, 8) => (raw[0] as f32 - 128.0) / 128.0,
                (1, 16) => i16::from_le_bytes(raw.try_into().unwrap()) as f32 / 32768.0,
                (1, 24) => {
                    (i32::from_le_bytes([0, raw[0], raw[1], raw[2]]) >> 8) as f32 / 8388608.0
                }
                (1, 32) => i32::from_le_bytes(raw.try_into().unwrap()) as f32 / 2147483648.0,
                (3, 32) => f32::from_le_bytes(raw.try_into().unwrap()),
                _ => return Err("unsupported WAV mix encoding".into()),
            };
        }
        let value = value / channels as f32;
        if !value.is_finite() {
            return Err("non-finite WAV sample".into());
        }
        samples.push(value);
    }
    Ok(Wave { rate, samples })
}
pub fn duration(bytes: &[u8]) -> Result<f64, String> {
    let wave = decode(bytes)?;
    Ok(wave.samples.len() as f64 / wave.rate as f64)
}
pub fn is_silent(bytes: &[u8]) -> Result<bool, String> {
    Ok(decode(bytes)?
        .samples
        .iter()
        .all(|s| s.abs() < 1.0 / 32768.0))
}
/// Lossless compression of the cooked mono PCM16 mix; no external encoder.
pub fn compress(bytes: &[u8]) -> Result<Vec<u8>, String> {
    crate::compress_pcm16_mix(bytes)
}
pub fn render(source: &Path, mix: &Value) -> Result<Vec<u8>, String> {
    let duration = mix["duration_seconds"]
        .as_f64()
        .ok_or("mix needs duration_seconds")?;
    let limit = (crate::MAX_WAV_BYTES - 44) / 2;
    if !duration.is_finite() || duration <= 0.0 || duration > limit as f64 / 8000.0 {
        return Err("audio mix duration exceeds WAV limits".into());
    }
    let rate = 22050u32.min((limit as f64 / duration).floor() as u32);
    let mut samples = vec![0f32; (duration * rate as f64).ceil() as usize];
    let voices = mix["voices"]
        .as_array()
        .filter(|v| v.len() <= 4096)
        .ok_or("invalid mix voices")?;
    let mut cache = BTreeMap::new();
    for voice in voices {
        let path = text(voice, "wav")?;
        if !cache.contains_key(path) {
            cache.insert(
                path.to_owned(),
                decode(&read(&relative(source, path)?, crate::MAX_WAV_BYTES)?)?,
            );
        }
        let wave = &cache[path];
        if wave.samples.is_empty() {
            continue;
        }
        let start = voice["at_seconds"].as_f64().unwrap_or(0.0);
        let stop = voice["stop_seconds"]
            .as_f64()
            .unwrap_or(duration)
            .min(duration);
        let gain = voice["gain"].as_f64().unwrap_or(1.0);
        let pitch = voice["pitch"].as_f64().unwrap_or(1.0);
        if !start.is_finite()
            || start < 0.0
            || !stop.is_finite()
            || !gain.is_finite()
            || gain.abs() > 64.0
            || !pitch.is_finite()
            || !(0.01..=16.0).contains(&pitch)
        {
            return Err("invalid mix timing/gain/pitch".into());
        }
        let first = (start * rate as f64).round() as usize;
        let last = ((stop * rate as f64).ceil() as usize).min(samples.len());
        let looping = voice["looping"].as_bool().unwrap_or(false);
        let envelopes = voice["envelopes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|e| {
                Ok((
                    Curve::parse(&e["volume"], false)?,
                    Curve::parse(&e["pitch"], true)?,
                    e["offset"].as_f64().unwrap_or(0.0),
                    if e["cycle"].is_null() {
                        None
                    } else {
                        let start = e["cycle"]["start"]
                            .as_f64()
                            .ok_or("invalid envelope loop start")?;
                        let end = e["cycle"]["end"]
                            .as_f64()
                            .ok_or("invalid envelope loop end")?;
                        if !start.is_finite() || !end.is_finite() || start < 0.0 || end <= start {
                            return Err("invalid envelope loop interval".into());
                        }
                        Some((start, end, e["cycle"]["count"].as_u64()))
                    },
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let seek = voice["source_offset_seconds"].as_f64().unwrap_or(0.0);
        if !seek.is_finite() || seek < 0.0 {
            return Err("invalid mix source offset".into());
        }
        let mut phase = seek * wave.rate as f64 * pitch;
        for n in first..last {
            if looping
                && voice["loop_count"]
                    .as_u64()
                    .is_some_and(|count| phase >= count as f64 * wave.samples.len() as f64)
            {
                break;
            }
            let mut offset = phase;
            if looping {
                offset %= wave.samples.len() as f64;
            } else if offset >= wave.samples.len() as f64 {
                break;
            }
            let i = offset.floor() as usize;
            let next = if i + 1 < wave.samples.len() {
                i + 1
            } else if looping {
                0
            } else {
                i
            };
            let seconds = (n - first) as f64 / rate as f64 + seek;
            let envelope_gain = envelopes
                .iter()
                .map(|(v, _, at, cycle)| {
                    v.sample(envelope_time(seconds + at, *cycle))
                        .clamp(0.0, 64.0)
                })
                .product::<f64>();
            let envelope_pitch = envelopes
                .iter()
                .map(|(_, p, at, cycle)| {
                    p.sample(envelope_time(seconds + at, *cycle))
                        .clamp(0.01, 16.0)
                })
                .product::<f64>();
            samples[n] += (wave.samples[i]
                + (wave.samples[next] - wave.samples[i]) * (offset - i as f64) as f32)
                * (gain * envelope_gain) as f32;
            phase += wave.rate as f64 / rate as f64 * pitch * envelope_pitch;
        }
    }
    // Fixed per-track attenuation prevents clipping without altering timing.
    let peak = samples.iter().map(|n| n.abs()).fold(1.0f32, f32::max);
    let pcm: Vec<i16> = samples
        .into_iter()
        .map(|n| (n / peak * 32767.0).round() as i16)
        .collect();
    Ok(encode(rate, &pcm))
}
fn encode(rate: u32, pcm: &[i16]) -> Vec<u8> {
    let size = (pcm.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(size as usize + 44);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(size + 36).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&size.to_le_bytes());
    for n in pcm {
        bytes.extend_from_slice(&n.to_le_bytes());
    }
    bytes
}
fn envelope_time(time: f64, cycle: Option<(f64, f64, Option<u64>)>) -> f64 {
    let Some((start, end, count)) = cycle else {
        return time;
    };
    if time < end {
        return time;
    }
    let length = end - start;
    let completed = ((time - start) / length).floor() as u64;
    if count.is_none_or(|count| completed < count) {
        start + (time - start) % length
    } else {
        time - length * count.unwrap_or(0).saturating_sub(1) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    #[test]
    fn envelope_loops_keep_the_intro_and_resume_after_finite_cycles() {
        assert_eq!(envelope_time(0.5, Some((1.0, 3.0, None))), 0.5);
        assert_eq!(envelope_time(3.0, Some((1.0, 3.0, None))), 1.0);
        assert_eq!(envelope_time(5.0, Some((1.0, 3.0, None))), 1.0);
        assert_eq!(envelope_time(5.0, Some((1.0, 3.0, Some(2)))), 3.0);
        assert_eq!(envelope_time(6.0, Some((1.0, 3.0, Some(2)))), 4.0);
    }
    #[test]
    fn exported_linear_constant_and_cubic_envelopes_are_sampled() {
        let linear = Curve::parse(
            &json!([{"Time":0,"Value":0.1,"InterpMode":"RCIM_Linear"},{"Time":2,"Value":1}]),
            false,
        )
        .unwrap();
        assert!((linear.sample(1.0) - 0.55).abs() < 1e-6);
        let cubic = Curve::parse(
            &json!([{"Time":0,"Value":0,"InterpMode":"RCIM_Cubic"},{"Time":1,"Value":1}]),
            false,
        )
        .unwrap();
        assert!((cubic.sample(0.25) - 0.15625).abs() < 1e-6);
        let constant = Curve::parse(
            &json!([{"Time":0,"Value":0.1,"InterpMode":"RCIM_Constant"},{"Time":2,"Value":1}]),
            false,
        )
        .unwrap();
        assert!((constant.sample(1.0) - 0.1).abs() < 1e-6);
        let undershoot = Curve::parse(
            &json!([{"Time":0,"Value":-0.0007},{"Time":0.01,"Value":1}]),
            false,
        )
        .unwrap();
        assert_eq!(undershoot.sample(0.0).clamp(0.0, 64.0), 0.0);
    }
    #[test]
    fn mixer_preserves_delay_overlap_gain_pitch_and_loop_end() {
        let dir = std::env::temp_dir().join(format!("djk-mix-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        fs::write(dir.join("a.wav"), encode(8000, &vec![8192; 800])).unwrap();
        let mix = json!({"duration_seconds":0.4,"voices":[
            {"wav":"a.wav","at_seconds":0.1,"gain":0.5,"pitch":2.0},
            {"wav":"a.wav","at_seconds":0.12,"gain":0.25,"looping":true,"stop_seconds":0.3}]});
        let wave = decode(&render(&dir, &mix).unwrap()).unwrap();
        let sample = |seconds: f64| wave.samples[(seconds * wave.rate as f64) as usize];
        assert!(sample(0.05).abs() < 1e-4);
        assert!((sample(0.11) - 0.125).abs() < 0.001);
        assert!((sample(0.13) - 0.1875).abs() < 0.001);
        assert!((sample(0.25) - 0.0625).abs() < 0.001);
        assert!(sample(0.35).abs() < 1e-4);
        fs::remove_file(dir.join("a.wav")).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn continuous_music_render_applies_startup_envelope_only_once() {
        let dir = std::env::temp_dir().join(format!("djk-mix-startup-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        fs::write(dir.join("a.wav"), encode(8000, &vec![16384; 800])).unwrap();
        let wav = render(&dir, &json!({"duration_seconds":0.2,"voices":[
            {"wav":"a.wav","looping":true,"envelopes":[{"volume":[{"Time":0,"Value":0},{"Time":0.01,"Value":1}]}]}]})).unwrap();
        let wave = decode(&wav).unwrap();
        assert_eq!(wave.samples[0], 0.0);
        assert!((wave.samples[(wave.rate as f64 * 0.1).round() as usize] - 0.5).abs() < 0.001);
        fs::remove_file(dir.join("a.wav")).unwrap();
        fs::remove_dir(dir).unwrap();
    }
}
