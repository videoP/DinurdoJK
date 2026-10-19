//! Compose exported montage segments into one source animation before
//! retargeting: trims, playback rates, repeats, section order and
//! already-resolved corrective poses. Montage *discovery* (reading a source
//! export's item and metadata files) lives with the Fortnite library tooling (`fn-library` follow-up).
use crate::compiler::{read, relative, text};
use crate::psa::{Key, Psa};
use crate::MAX_MEMBER_BYTES;
use serde_json::{json, Value};
use std::path::Path;

pub fn compose(source: &Path, variant: &Value) -> Result<Psa, String> {
    let segments = variant["segments"]
        .as_array()
        .ok_or("missing montage segments")?;
    let mut clips = segments
        .iter()
        .map(|s| {
            let path = relative(source, text(s, "psa")?)?;
            // Recipes that repair an additive reference handle it themselves.
            if s.get("repair_additive_reference").is_some() {
                Psa::parse(&read(&path, MAX_MEMBER_BYTES)?)
            } else {
                Psa::load(&path)
            }
        })
        .collect::<Result<Vec<_>, String>>()?;
    for (clip, segment) in clips.iter_mut().zip(segments) {
        if let Some(repair) = segment.get("repair_additive_reference") {
            let old = Psa::parse(&read(
                &relative(source, text(repair, "exported")?)?,
                MAX_MEMBER_BYTES,
            )?)?;
            let correct = Psa::parse(&read(
                &relative(source, text(repair, "correct")?)?,
                MAX_MEMBER_BYTES,
            )?)?;
            repair_additive_reference(clip, &old, &correct)?;
        }
    }
    let first = clips.first().ok_or("empty montage")?;
    for clip in &clips {
        if clip.bones.len() != first.bones.len()
            || clip
                .bones
                .iter()
                .zip(&first.bones)
                .any(|(a, b)| a.name != b.name || a.parent != b.parent)
        {
            return Err("montage combines incompatible source skeletons".into());
        }
    }
    let duration = variant["duration_seconds"]
        .as_f64()
        .ok_or("missing montage duration")?;
    if !duration.is_finite() || duration <= 0.0 || duration > 3600.0 {
        return Err("invalid montage duration".into());
    }
    let frames = (duration * 30.0).round().max(1.0) as usize;
    let bones = first.bones.len();
    if frames.checked_mul(bones).is_none_or(|n| n > 16_000_000) {
        return Err("montage pose allocation limit".into());
    }
    let mut result = Psa {
        bones: first.bones.clone(),
        fps: 30.0,
        frames,
        keys: Vec::with_capacity(frames * bones),
        scales: Vec::with_capacity(frames * bones),
    };
    for frame in 0..frames {
        let span = variant["spans"]
            .as_array()
            .ok_or("missing section spans")?
            .iter()
            .rev()
            .find(|s| frame >= (s["at"].as_f64().unwrap_or(0.0) * 30.0).round() as usize)
            .ok_or("section timeline gap")?;
        let from = span["from"].as_f64().unwrap_or(0.0);
        let to = span["to"].as_f64().ok_or("missing section end")?;
        let at = span["at"].as_f64().unwrap_or(0.0);
        let first_frame = (at * 30.0).round() as usize;
        let last_frame = ((at + to - from) * 30.0).round() as usize;
        // Quantize the boundaries once, then resample each section over that
        // span. A sub-frame loop start must not skip its opening pose.
        let t = from
            + (frame.saturating_sub(first_frame) as f64
                / last_frame.saturating_sub(first_frame).max(1) as f64)
                * (to - from);
        let mut chosen = None;
        for (index, s) in segments.iter().enumerate() {
            let start = s["start"].as_f64().unwrap_or(0.0);
            let from = s["from"].as_f64().unwrap_or(0.0);
            let to = s["to"].as_f64().ok_or("missing segment end")?;
            let rate = s["rate"].as_f64().unwrap_or(1.0);
            let repeats = s["repeats"].as_u64().unwrap_or(1);
            if !rate.is_finite()
                || rate == 0.0
                || to <= from
                || from < 0.0
                || repeats == 0
                || repeats > 4096
            {
                return Err("invalid montage segment timing".into());
            }
            let length = (to - from) / rate.abs();
            if t >= start - 0.00001 && t < start + length * repeats as f64 + 0.00001 {
                let phase = ((t - start).max(0.0) % length) * rate.abs();
                chosen = Some((index, if rate > 0.0 { from + phase } else { to - phase }));
            }
        }
        let (index, at) = chosen.ok_or_else(|| format!("montage segment gap at {t:.3}s"))?;
        let clip = &clips[index];
        // UE includes an endpoint at SequenceLength. The exporter writes
        // NumFrames/length as PSA rate, advancing a frame early at section jumps.
        let sample_rate = segments[index]["source_length_seconds"]
            .as_f64()
            .map(|length| (clip.frames - 1) as f64 / length)
            .unwrap_or(clip.fps as f64);
        let f = (at * sample_rate).clamp(0.0, (clip.frames - 1) as f64);
        let a = f.floor() as usize;
        let b = (a + 1).min(clip.frames - 1);
        let weight = f.fract() as f32;
        for bone in 0..bones {
            let x = clip.keys[a * bones + bone];
            let y = clip.keys[b * bones + bone];
            result.keys.push(Key {
                rotation: x.rotation.slerp(y.rotation, weight),
                position: x.position.lerp(y.position, weight),
            });
            result
                .scales
                .push(clip.scales[a * bones + bone].lerp(clip.scales[b * bones + bone], weight));
        }
    }
    Ok(result)
}

pub(crate) fn repair_additive_reference(
    clip: &mut Psa,
    old: &Psa,
    correct: &Psa,
) -> Result<(), String> {
    let bones = clip.bones.len();
    for reference in [old, correct] {
        if reference.bones.len() != bones
            || reference
                .bones
                .iter()
                .zip(&clip.bones)
                .any(|(a, b)| a.name != b.name || a.parent != b.parent)
        {
            return Err("additive reference skeleton mismatch".into());
        }
    }
    // CUE4Parse's ABPT_AnimScaled implementation uses frameIndex % refFrames,
    // rather than normalized time. Undo exactly that exported operation.
    for frame in 0..clip.frames {
        for bone in 0..bones {
            let i = frame * bones + bone;
            let a = (frame % old.frames) * bones + bone;
            let b = (frame % correct.frames) * bones + bone;
            clip.keys[i].rotation =
                (clip.keys[i].rotation * old.keys[a].rotation.inverse() * correct.keys[b].rotation)
                    .normalize();
            clip.keys[i].position += correct.keys[b].position - old.keys[a].position;
            for axis in 0..3 {
                if old.scales[a][axis].abs() < 1e-8 {
                    // Hidden weapon/prop helpers can be collapsed in both
                    // references. Their exact resulting scale is still zero;
                    // no inverse (or invented scale) is required.
                    if correct.scales[b][axis].abs() < 1e-8 && clip.scales[i][axis].abs() < 1e-8 {
                        continue;
                    }
                    return Err("cannot undo zero-scale additive reference".into());
                }
                clip.scales[i][axis] *= correct.scales[b][axis] / old.scales[a][axis];
            }
        }
    }
    Ok(())
}

pub fn bake(
    source: &Path,
    animation: &Value,
    target: &jka_assets::ghoul2::GlaAnimation,
) -> Result<(Vec<u8>, Vec<u8>, Value), String> {
    let mut locals = Vec::new();
    let mut pose = Vec::new();
    let mut frames = 0usize;
    let mut rows = Vec::new();
    for (index, variant) in animation["variants"]
        .as_array()
        .ok_or("invalid animation variants")?
        .iter()
        .enumerate()
    {
        let psa = compose(source, variant)?;
        let (gla, bytes, conversion) = crate::retarget::bake(&psa, target, &animation["retarget"])?;
        let gla = jka_assets::ghoul2::parse_gla(&gla)?;
        for f in 0..gla.num_frames {
            for b in 0..gla.num_bones() {
                let m = gla.decompress_bone(f, b)?;
                let matrix = glam::Mat3::from_cols_array(&[
                    m[0][0], m[1][0], m[2][0], m[0][1], m[1][1], m[2][1], m[0][2], m[1][2], m[2][2],
                ]);
                locals.push((
                    glam::Quat::from_mat3(&matrix).normalize(),
                    glam::Vec3::new(m[0][3], m[1][3], m[2][3]),
                ));
            }
        }
        pose.extend_from_slice(&bytes[16..]);
        let loop_start = variant["loop_start_seconds"]
            .as_f64()
            .map(|t| frames as i32 + (t * 30.0).round() as i32);
        let mut audio = variant["audio"].clone();
        // Section quantization can shorten a pose cycle by half a frame. Keep
        // section-owned cues on the resulting frame boundary, while music with
        // an independent period retains its exact source clock.
        let source_end = (variant["duration_seconds"].as_f64().unwrap() * 1000.0).round() as u64;
        let baked_end = (psa.frames as f64 * 1000.0 / 30.0).round() as u64;
        for event in audio.as_array_mut().into_iter().flatten() {
            if event["repeat_period_ms"].is_null() && event["at_ms"].as_u64() == Some(source_end) {
                event["at_ms"] = json!(baked_end);
            }
        }
        rows.push(json!({"variant_name":variant["name"],"first_frame":frames,"end_frame":frames+psa.frames,"fps":30,"loop":loop_start.is_some(),"loop_start_frame":loop_start,"audio":audio,"audio_status":variant["audio_status"],"audio_detail":variant["audio_detail"],"conversion":conversion,"variant_index":index}));
        frames += psa.frames;
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"DJKPOSE1");
    bytes.extend_from_slice(&(frames as u32).to_le_bytes());
    bytes.extend_from_slice(&(target.num_bones() as u32).to_le_bytes());
    bytes.extend(pose);
    crate::PoseClip::parse(&bytes)?;
    Ok((
        crate::retarget::encode(target, frames, &locals)?,
        bytes,
        json!({"method":"montage-sections","variants":rows}),
    ))
}
