use jka_assets::ghoul2::GlaAnimation;
use serde_json::Value;
#[derive(Clone, Debug)]
pub struct Emote {
    pub id: String,
    /// Source identity, preserved when the UI groups/renames catalogue rows.
    pub network_key: String,
    pub label: String,
    pub catalog_parent: Option<String>,
    pub shared_montage_alias: bool,
    pub shared_montages: Vec<String>,
    pub catalog_aliases: Vec<String>,
    pub gla: String,
    pub first_frame: i32,
    /// Exclusive end frame, as required by Ghoul2Animator.
    pub end_frame: i32,
    pub fps: f32,
    pub looping: bool,
    pub loop_start_frame: Option<i32>,
    pub variant_name: String,
    pub variants: Vec<Emote>,
    /// Export/conversion diagnostics, shown per variation rather than guessed
    /// from another item's similar display title.
    pub audio_status: String,
    pub audio_detail: String,
    pub audio: Vec<AudioEvent>,
    pub pose: Option<std::sync::Arc<crate::PoseClip>>,
    pub props: Vec<crate::presentation::Prop>,
    pub presentation_detail: String,
    pub contact_track: Option<String>,
    pub contact_pose: Option<std::sync::Arc<crate::PoseClip>>,
    /// Baked particle effects (Niagara systems executed by the compiler).
    pub effects: Option<EffectSet>,
}

#[derive(Clone, Debug)]
pub struct EffectSet {
    pub track: String,
    pub renderers: Vec<crate::effects::Renderer>,
    pub data: Option<std::sync::Arc<crate::effects::Track>>,
}
impl EffectSet {
    fn parse(v: &Value) -> Result<Option<Self>, String> {
        if v.is_null() {
            return Ok(None);
        }
        let renderers = v["renderers"]
            .as_array()
            .ok_or("effects without renderers")?;
        if renderers.is_empty() || renderers.len() > crate::effects::MAX_RENDERERS {
            return Err("invalid effect renderer count".into());
        }
        Ok(Some(Self {
            track: qpath(v["track"].as_str().ok_or("effects without track")?)?,
            renderers: renderers
                .iter()
                .map(crate::effects::Renderer::parse)
                .collect::<Result<_, _>>()?,
            data: None,
        }))
    }
}

#[derive(Clone, Debug)]
pub struct Preview {
    pub emote: Emote,
    pub elapsed_ms: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioLane {
    Music,
    Foley,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioMode {
    Preview,
    World,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioAudience {
    Preview,
    World,
    Both,
}

#[derive(Clone, Debug)]
pub struct AudioEvent {
    pub at_ms: u64,
    pub lane: AudioLane,
    pub audience: AudioAudience,
    /// None is a timed stop for this lane; Some(path) starts a WAV/FLAC track.
    pub wav: Option<String>,
    pub repeat_on_loop: bool,
    /// Optional audio clock, independent of a short pose loop.
    pub repeat_period_ms: Option<u64>,
    /// Sample-clock loop point inside this decoded track; intro plays once.
    pub loop_start_sample: Option<u64>,
}

pub fn qpath(value: &str) -> Result<String, String> {
    let normalized = value.replace('\\', "/");
    if normalized.is_empty()
        || normalized.starts_with('/')
        || normalized.contains(':')
        || normalized
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || normalized.bytes().any(|b| b < 0x20)
    {
        return Err(format!("unsafe emote asset path {value:?}"));
    }
    Ok(normalized)
}

pub fn parse_manifest(root: &Value) -> Result<Vec<Emote>, String> {
    let rows = root
        .get("emotes")
        .and_then(Value::as_array)
        .ok_or("emote manifest requires an 'emotes' array")?;
    if rows.len() > 2048 {
        return Err("emote manifest exceeds 2048 entries".into());
    }
    let mut out = Vec::with_capacity(rows.len());
    let mut seen = std::collections::HashSet::new();
    for (index, row) in rows.iter().enumerate() {
        let field = |key: &str| -> Result<&str, String> {
            row.get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| format!("emotes[{index}] missing {key}"))
        };
        let id = field("id")?.to_owned();
        if !seen.insert(id.clone()) {
            return Err(format!("duplicate emote ID {id}"));
        }
        let label = field("name")?.to_owned();
        let gla = qpath(field("gla")?)?;
        if !gla.to_ascii_lowercase().ends_with(".gla") {
            return Err(format!("emote {id}: only Ghoul2 .gla assets are supported"));
        }
        let first_frame = row
            .get("first_frame")
            .and_then(Value::as_i64)
            .and_then(|n| i32::try_from(n).ok())
            .ok_or_else(|| format!("emote {id}: invalid first_frame"))?;
        let end_frame = row
            .get("end_frame")
            .and_then(Value::as_i64)
            .and_then(|n| i32::try_from(n).ok())
            .ok_or_else(|| format!("emote {id}: invalid end_frame"))?;
        let fps = row
            .get("fps")
            .and_then(Value::as_f64)
            .ok_or_else(|| format!("emote {id}: invalid fps"))? as f32;
        if first_frame < 0
            || end_frame <= first_frame
            || !fps.is_finite()
            || !(1.0..=240.0).contains(&fps)
        {
            return Err(format!("emote {id}: invalid frame range or FPS"));
        }
        let looping = row.get("loop").and_then(Value::as_bool).unwrap_or(false);
        let mut audio = Vec::new();
        if let Some(entries) = row.get("audio") {
            let entries = entries
                .as_array()
                .ok_or_else(|| format!("emote {id}: audio must be an array"))?;
            if entries.len() > 128 {
                return Err(format!("emote {id}: audio exceeds 128 events"));
            }
            for (n, event) in entries.iter().enumerate() {
                let at_ms = event
                    .get("at_ms")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| format!("emote {id}: audio[{n}] missing at_ms"))?;
                let duration =
                    ((end_frame - first_frame) as f64 * 1000.0 / f64::from(fps)).ceil() as u64;
                if at_ms > 3_600_000
                    || (at_ms > duration
                        && !(looping && event["repeat_period_ms"].as_u64().is_some()))
                {
                    return Err(format!("emote {id}: audio[{n}] outside animation duration"));
                }
                let lane = match event.get("lane").and_then(Value::as_str) {
                    Some("music") => AudioLane::Music,
                    Some("foley") => AudioLane::Foley,
                    _ => {
                        return Err(format!(
                            "emote {id}: audio[{n}] lane must be music or foley"
                        ))
                    }
                };
                let audience = match event.get("audience").and_then(Value::as_str) {
                    Some("1p") => AudioAudience::Preview,
                    Some("3p") => AudioAudience::World,
                    Some("both") => AudioAudience::Both,
                    _ => {
                        return Err(format!(
                            "emote {id}: audio[{n}] audience must be 1p, 3p, or both"
                        ))
                    }
                };
                let wav = event
                    .get("wav")
                    .and_then(Value::as_str)
                    .map(qpath)
                    .transpose()?;
                if let Some(wav) = &wav {
                    if ![".wav", ".flac", ".ogg"]
                        .iter()
                        .any(|ext| wav.to_ascii_lowercase().ends_with(ext))
                    {
                        return Err(format!(
                            "emote {id}: audio[{n}] must point to WAV, FLAC or Vorbis"
                        ));
                    }
                }
                if let Some(gain) = event.get("gain").and_then(Value::as_f64) {
                    if (gain - 1.0).abs() > 0.0001 {
                        return Err(format!(
                            "emote {id}: audio[{n}] gain needs offline WAV baking"
                        ));
                    }
                }
                if let Some(pitch) = event.get("pitch").and_then(Value::as_f64) {
                    if (pitch - 1.0).abs() > 0.0001 {
                        return Err(format!(
                            "emote {id}: audio[{n}] pitch needs offline WAV baking"
                        ));
                    }
                }
                if event
                    .get("source_offset_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    > 0
                    || event
                        .get("fade_out_ms")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        > 0
                {
                    return Err(format!(
                        "emote {id}: audio[{n}] seeking/fades need offline WAV baking"
                    ));
                }
                let repeat_on_loop = event
                    .get("repeat_on_loop")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                audio.push(AudioEvent {
                    at_ms,
                    lane,
                    audience,
                    repeat_on_loop,
                    loop_start_sample: event["loop_start_sample"]
                        .as_u64()
                        .map(|start| {
                            if !looping || wav.is_none() || start >= 8 * 1024 * 1024 {
                                Err("invalid sample-clock loop point")
                            } else {
                                Ok(start)
                            }
                        })
                        .transpose()?,
                    wav,
                    repeat_period_ms: event["repeat_period_ms"]
                        .as_u64()
                        .map(|period| {
                            if !(1..=3_600_000).contains(&period) {
                                Err("invalid audio repeat period")
                            } else {
                                Ok(period)
                            }
                        })
                        .transpose()?,
                });
            }
            audio.sort_by_key(|event| event.at_ms);
        }
        // Backward compatibility with the previous single-sound manifest.
        if audio.is_empty() {
            if let Some(sound) = row.get("sound").and_then(Value::as_str) {
                if !sound.is_empty() {
                    audio.push(AudioEvent {
                        at_ms: 0,
                        lane: AudioLane::Music,
                        audience: AudioAudience::Both,
                        wav: Some(qpath(sound)?),
                        repeat_on_loop: looping,
                        loop_start_sample: None,
                        repeat_period_ms: None,
                    });
                }
            }
        }
        let loop_start_frame = row["loop_start_frame"]
            .as_i64()
            .map(|n| i32::try_from(n).map_err(|_| "invalid loop start"))
            .transpose()?;
        if loop_start_frame.is_some_and(|start| start < first_frame || start >= end_frame) {
            return Err("loop start outside variant frame range".into());
        }
        let mut variants = Vec::new();
        if let Some(rows) = row["variants"].as_array() {
            if rows.len() > 128 {
                return Err("too many emote variants".into());
            }
            for variant in rows {
                let mut v = variant.clone();
                for key in ["catalog_parent", "shared_montage_alias", "shared_montages"] {
                    if v[key].is_null() {
                        v[key] = row[key].clone();
                    }
                }
                if !v["variants"].is_null() {
                    return Err("nested variants are not supported".into());
                }
                for (key, value) in [("id", &id), ("name", &label), ("gla", &gla)] {
                    v[key] = Value::String(value.clone());
                }
                variants.push(parse_manifest(&serde_json::json!({"emotes":[v]}))?.remove(0));
            }
        }
        out.push(Emote {
            network_key: format!("{}\t{}", id.to_ascii_lowercase(), row["variant_name"].as_str().unwrap_or("Default").to_ascii_lowercase()),
            id,
            label,
            catalog_parent: row["catalog_parent"].as_str().map(str::to_owned),
            shared_montage_alias: row["shared_montage_alias"].as_bool().unwrap_or(false),
            shared_montages: row["shared_montages"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
            catalog_aliases: Vec::new(),
            gla,
            first_frame,
            end_frame,
            fps,
            looping,
            contact_track: row["contact_track"].as_str().map(qpath).transpose()?,
            contact_pose: None,
            loop_start_frame,
            variant_name: row["variant_name"].as_str().unwrap_or("Default").to_owned(),
            variants,
            audio_status: row["audio_status"].as_str().unwrap_or("").to_owned(),
            audio_detail: row["audio_detail"].as_str().unwrap_or("").to_owned(),
            audio,
            pose: None,
            props: row["props"]
                .as_array()
                .into_iter()
                .flatten()
                .map(crate::presentation::Prop::parse)
                .collect::<Result<Vec<_>, _>>()?,
            presentation_detail: row["presentation_detail"].as_str().unwrap_or("").to_owned(),
            effects: EffectSet::parse(&row["effects"])?,
        });
    }
    Ok(out)
}

/// Reject imported GLAs that would corrupt GLM skin weights or attachment bolts.
/// A converter must bake into the *target* JKA skeleton, not rename 280 bones.
pub fn validate_gla(target: &GlaAnimation, clip: &GlaAnimation) -> Result<(), String> {
    if (target.scale - clip.scale).abs() > 0.001 {
        return Err("emote GLA scale does not match target".into());
    }
    if target.skeleton.len() != clip.skeleton.len() {
        return Err(format!(
            "emote has {} bones; player needs {}. Retarget and bake first",
            clip.skeleton.len(),
            target.skeleton.len()
        ));
    }
    for (index, (a, b)) in target.skeleton.iter().zip(&clip.skeleton).enumerate() {
        if !a.name.eq_ignore_ascii_case(&b.name) || a.parent != b.parent {
            return Err(format!(
                "emote skeleton mismatch at bone {index} ({}/{})",
                a.name, b.name
            ));
        }
        for row in 0..3 {
            for col in 0..4 {
                if (a.base_pose[row][col] - b.base_pose[row][col]).abs() > 0.001 {
                    return Err(format!(
                        "emote bind pose mismatch on {}: bake to target rest pose",
                        a.name
                    ));
                }
                if (a.base_pose_inv[row][col] - b.base_pose_inv[row][col]).abs() > 0.001 {
                    return Err(format!("emote inverse bind pose mismatch on {}", a.name));
                }
            }
        }
    }
    Ok(())
}
