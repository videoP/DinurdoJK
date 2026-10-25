//! Emote catalogue and cosmetic playback, retargeted to Ghoul2 player models.
//!
//! Loads compiled .djk_fnemote bundles and legacy GLA manifests. Nothing here
//! writes userinfo, reliable commands or authoritative entityState animations.
use serde_json::{Value, json};
pub mod network;
mod taunt;
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::Instant,
};

pub use jka_emotes::manifest::{AudioAudience, AudioLane, AudioMode, Emote, Preview};
static CUBIC_INTERPOLATION:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(true);
pub fn set_cubic_interpolation(enabled:bool){CUBIC_INTERPOLATION.store(enabled,std::sync::atomic::Ordering::Relaxed);}
static EFFECTS:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(true);
pub fn set_effects_enabled(enabled:bool){EFFECTS.store(enabled,std::sync::atomic::Ordering::Relaxed);}
pub fn effects_enabled()->bool{EFFECTS.load(std::sync::atomic::Ordering::Relaxed)}
pub fn interpolation()->jka_emotes::Interpolation{if CUBIC_INTERPOLATION.load(std::sync::atomic::Ordering::Relaxed){jka_emotes::Interpolation::Cubic}else{jka_emotes::Interpolation::Linear}}
pub fn sample_body(clip:&jka_emotes::PoseClip,active:&Preview)->Vec<jka_assets::ghoul2::Matrix3x4>{
    let e=&active.emote;clip.sample_range_interpolated(active.elapsed_ms,e.fps,e.first_frame as usize,e.end_frame as usize,e.loop_start_frame.map(|n|n as usize).or(e.looping.then_some(e.first_frame as usize)),interpolation())
}
static FAVORITE_ASSETS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
type PreparedProp=(String,std::sync::Arc<jka_emotes::prop_mesh::Mesh>,usize);
static PROP_MESHES:OnceLock<Mutex<Vec<PreparedProp>>>=OnceLock::new();
type PreparedStatic=(String,std::sync::Arc<jka_assets::md3::Model>,usize);
static PROP_STATIC:OnceLock<Mutex<Vec<PreparedStatic>>>=OnceLock::new();
pub fn prepared_prop_md3(path:&str)->Option<std::sync::Arc<jka_assets::md3::Model>>{
    PROP_STATIC.get_or_init(Default::default).lock().ok()?.iter().find(|p|p.0==path).map(|p|p.1.clone())
}
fn prepare_static_prop(path:&str)->Result<(),String>{
    if prepared_prop_md3(path).is_some(){return Ok(());}
    let base=CATALOG.get_or_init(Default::default).lock().map_err(|_|"catalogue lock")?.base.clone();
    let mut assets=jka_assets::pk3::AssetSearchPath::open(&base).map_err(|e|e.to_string())?;
    let bytes=assets.read(path,32*1024*1024).map_err(|e|e.to_string())?.ok_or("missing static prop geometry")?.bytes;
    let size=bytes.len()*4;let mesh=std::sync::Arc::new(jka_assets::md3::parse(&bytes)?);
    let mut cache=PROP_STATIC.get_or_init(Default::default).lock().map_err(|_|"prop cache lock")?;
    while !cache.is_empty()&&cache.iter().map(|p|p.2).sum::<usize>()+size>64*1024*1024{let i=cache.iter().position(|p|!favorite_asset(&p.0)).unwrap_or(0);cache.remove(i);}
    if size<=64*1024*1024{cache.push((path.into(),mesh,size));}Ok(())
}
pub fn prepared_prop_mesh(path:&str)->Option<std::sync::Arc<jka_emotes::prop_mesh::Mesh>>{
    PROP_MESHES.get_or_init(Default::default).lock().ok()?.iter().find(|p|p.0==path).map(|p|p.1.clone())
}
fn prepare_prop_mesh(path:&str)->Result<(),String>{
    if prepared_prop_mesh(path).is_some(){return Ok(());}
    let base=CATALOG.get_or_init(Default::default).lock().map_err(|_|"catalogue lock")?.base.clone();
    let mut assets=jka_assets::pk3::AssetSearchPath::open(&base).map_err(|e|e.to_string())?;
    let bytes=assets.read(path,32*1024*1024).map_err(|e|e.to_string())?.ok_or("missing skeletal prop geometry")?.bytes;
    let mesh=std::sync::Arc::new(jka_emotes::prop_mesh::Mesh::parse(&bytes)?);
    let size=mesh.vertices.len()*std::mem::size_of::<jka_emotes::prop_mesh::Vertex>()+mesh.indices.len()*4;
    let mut cache=PROP_MESHES.get_or_init(Default::default).lock().map_err(|_|"prop cache lock")?;
    while !cache.is_empty()&&cache.iter().map(|p|p.2).sum::<usize>()+size>64*1024*1024{let i=cache.iter().position(|p|!favorite_asset(&p.0)).unwrap_or(0);cache.remove(i);}
    if size<=64*1024*1024{cache.push((path.into(),mesh,size));}Ok(())
}
pub fn update_favorite_assets(catalog: &[Emote], ids: &[String]) {
    if let Ok(mut prefixes) = FAVORITE_ASSETS.get_or_init(Default::default).lock() {
        *prefixes = catalog
            .iter()
            .filter(|e| ids.contains(&e.id) || e.catalog_aliases.iter().any(|id|ids.contains(id)) || e.variants.iter().any(|v| ids.contains(&v.id)))
            .flat_map(|e| std::iter::once(e).chain(e.variants.iter()))
            .flat_map(|e| e.gla.rsplit_once('/').map(|p| format!("{}/", p.0)).into_iter().chain(e.props.iter().map(|p|p.model.clone())))
            .collect();
    }
}
pub fn favorite_asset(key: &str) -> bool {
    FAVORITE_ASSETS
        .get_or_init(Default::default)
        .lock()
        .is_ok_and(|p| p.iter().any(|prefix| key.starts_with(prefix)))
}
pub fn pose_plan(
    source: &std::sync::Arc<jka_assets::ghoul2::GlaAnimation>,
    target: &std::sync::Arc<jka_assets::ghoul2::GlaAnimation>,
) -> Result<std::sync::Arc<jka_assets::skeleton::PoseRetargeter>, String> {
    type Entry = (
        std::sync::Arc<jka_assets::ghoul2::GlaAnimation>,
        std::sync::Arc<jka_assets::ghoul2::GlaAnimation>,
        std::sync::Arc<jka_assets::skeleton::PoseRetargeter>,
    );
    static PLANS: OnceLock<Mutex<Vec<Entry>>> = OnceLock::new();
    let mut plans = PLANS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "emote rig cache poisoned")?;
    if let Some(i) = plans
        .iter()
        .position(|p| std::sync::Arc::ptr_eq(source, &p.0) && std::sync::Arc::ptr_eq(target, &p.1))
    {
        let entry = plans.remove(i);
        let result = entry.2.clone();
        plans.push(entry);
        return Ok(result);
    }
    let plan = jka_assets::skeleton::cached_retargeter(&source.skeleton, &target.skeleton)?;
    if plans.len() >= 8 {
        plans.remove(0);
    }
    plans.push((source.clone(), target.clone(), plan.clone()));
    Ok(plan)
}
/// Old compiler labels appended a PSA asset name. Keep it out of display rows.
pub fn display_title(emote: &Emote) -> &str {
    let stem = emote.id.split_once("__").map(|(_, stem)| stem);
    stem.and_then(|stem| emote.label.strip_suffix(&format!(" [{stem}]")))
        .unwrap_or(&emote.label)
}

pub fn audio_status_label(emote: &Emote) -> &str {
    match emote.audio_status.as_str() {
        "missing_source" => "No audio · source assets missing",
        "unmapped" => "No audio · mapping unresolved",
        "silent_mix" => "No audio · rendered track silent",
        "partial" => "Partial audio · unresolved sources",
        "foley_only" => "Sound effects only · no music",
        _ if !emote.audio.iter().any(|a| a.wav.is_some()) => "No audio · not scheduled",
        _ => "Audio available",
    }
}

pub fn default_variant_index(emote: &Emote) -> usize {
    emote
        .variants
        .iter()
        .position(|v| {
            v.gla == emote.gla
                && v.first_frame == emote.first_frame
                && v.end_frame == emote.end_frame
        })
        .unwrap_or(0)
}

fn annotate_audio(row: &mut Value, source_clips: usize) {
    let music = row["audio"].as_array().is_some_and(|a| {
        a.iter()
            .any(|e| e["wav"].is_string() && e["lane"] == "music")
    });
    let any = row["audio"]
        .as_array()
        .is_some_and(|a| a.iter().any(|e| e["wav"].is_string()));
    if row["audio_status"].as_str().is_none_or(|s| s.is_empty()) {
        let (status, detail) = if music {
            ("available", "Music is scheduled.")
        } else if any {
            (
                "foley_only",
                "Only sound effects are scheduled; this variation has no music track.",
            )
        } else if source_clips == 0 {
            (
                "missing_source",
                "No decoded source WAVs are contained in this bundle. Inspect/re-export the source packages.",
            )
        } else {
            (
                "unmapped",
                "Decoded source WAVs are contained in the bundle, but none are scheduled for this variation. Inspect the source notify/graph mapping.",
            )
        };
        row["audio_status"] = json!(status);
        row["audio_detail"] = json!(detail);
    }
    if let Some(variants) = row["variants"].as_array_mut() {
        for variant in variants {
            annotate_audio(variant, source_clips);
        }
    }
}
#[derive(Debug)]
pub enum AudioCommand {
    StopAll,
    StopLane(AudioLane),
    Play {
        wav: String,
        lane: AudioLane,
        mode: AudioMode,
        loop_start_sample: Option<u64>,
    },
}

struct Playing {
    emote: Emote,
    started: Instant,
    mode: AudioMode,
    // Next absolute playback-clock millisecond for each resolved audio event.
    next_audio_ms: Vec<u64>,
}

#[derive(Default)]
struct PlaybackState {
    requested_mode: Option<AudioMode>,
    world_taunt: Option<taunt::Taunt>,
    playing: Option<Playing>,
    // The logical animation start survives asynchronous bundle/audio loading.
    // It may precede this client's play command when joining a remote dance.
    requested_start: Option<Instant>,
    stop_pending: bool,
    generation: u64,
    prepared: Option<(u64, Emote, AudioMode, Vec<crate::audio::SoundDecodeResult>)>,
}

static PLAYING: OnceLock<Mutex<PlaybackState>> = OnceLock::new();
#[derive(Default)]
struct CatalogueCache {
    base: PathBuf,
    generation: u64,
    pending: bool,
    result: Option<Result<std::sync::Arc<Vec<Emote>>, String>>,
}
static CATALOG: OnceLock<Mutex<CatalogueCache>> = OnceLock::new();

type PreparationJob = (u64, Emote, AudioMode);
static PREPARER: OnceLock<(Mutex<Option<PreparationJob>>, std::sync::Condvar)> = OnceLock::new();

fn prepare_emote(job: PreparationJob) {
    // One worker, one replaceable pending request. Old selections cannot start
    // after a stop/new selection, and input never performs I/O or codec work.
    let queue = PREPARER.get_or_init(|| {
        std::thread::Builder::new()
            .name("emote-playback-assets".into())
            .spawn(|| {
                let mut cache: Vec<(
                    String,
                    jka_emotes::Bundle,
                    Vec<crate::audio::SoundDecodeResult>,
                    usize,
                )> = Vec::new();
                loop {
                    let Some((lock, wake)) = PREPARER.get() else {
                        std::thread::yield_now();
                        continue;
                    };
                    let mut pending = lock.lock().unwrap();
                    while pending.is_none() {
                        pending = wake.wait(pending).unwrap();
                    }
                    let (generation, mut emote, mode) = pending.take().unwrap();
                    drop(pending);
                    let result = (|| {
                        let prefix = emote
                            .gla
                            .rsplit_once('/')
                            .ok_or("missing bundle prefix")?
                            .0
                            .to_owned();
                        let bundle = BUNDLES
                            .get_or_init(Default::default)
                            .lock()
                            .map_err(|_| "bundle lock poisoned")?
                            .get(&prefix)
                            .cloned()
                            .ok_or("unmounted emote bundle")?;
                        let cache_key = format!("{prefix}/{mode:?}/{}", emote.id);
                        if let Some(i) = cache.iter().position(|e| e.0 == cache_key) {
                            let entry = cache.remove(i);
                            let result = (entry.1.clone(), entry.2.clone());
                            cache.push(entry);
                            return Ok(result);
                        }
                        let bundle = bundle.validate_for_playback()?;
                        let mut decoded = Vec::new();
                        let mut seen = std::collections::HashSet::new();
                        for cue in &emote.audio {
                            if cue.audience != AudioAudience::Both
                                && cue.audience
                                    != if mode == AudioMode::World {
                                        AudioAudience::World
                                    } else {
                                        AudioAudience::Preview
                                    }
                            {
                                continue;
                            }
                            if let Some(path) = &cue.wav {
                                if seen.insert(path.clone()) {
                                    let bytes = read_asset(path, jka_emotes::MAX_WAV_BYTES)?
                                        .ok_or("missing bundled audio")?;
                                    decoded.push(
                                        crate::audio::SoundDecodeJob::from_emote(
                                            path.clone(),
                                            bytes,
                                        )
                                        .decode(),
                                    );
                                }
                            }
                        }
                        let bytes = bundle.pose.as_ref().map_or(0, |p| p.frames * p.bones * 48)
                            + decoded.iter().map(|r| r.memory_bytes()).sum::<usize>();
                        if bytes <= 64 * 1024 * 1024 {
                            while !cache.is_empty()
                                && (cache.len() >= 8
                                    || cache.iter().map(|e| e.3).sum::<usize>() + bytes
                                        > 64 * 1024 * 1024)
                            {
                                let oldest = cache
                                    .iter()
                                    .position(|e| !favorite_asset(&e.0))
                                    .unwrap_or(0);
                                cache.remove(oldest);
                            }
                            cache.push((cache_key, bundle.clone(), decoded.clone(), bytes));
                        }
                        Ok::<_, String>((bundle, decoded))
                    })();
                    match result {
                        Ok((bundle, audio)) => {
                            for warning in attach_visuals(&bundle, &mut emote) {
                                eprintln!("{warning}");
                            }
                            emote.pose = bundle.pose;
                            if let Ok(mut state) = playing().lock() {
                                if state.generation == generation {
                                    state.prepared = Some((generation, emote, mode, audio));
                                }
                            }
                        }
                        Err(error) => eprintln!("Cannot prepare emote {}: {error}", emote.id),
                    }
                }
            })
            .expect("emote asset worker");
        (Mutex::new(None), std::sync::Condvar::new())
    });
    *queue.0.lock().unwrap() = Some(job);
    queue.1.notify_one();
}

/// Read an emote's pose, contact, prop and effect tracks from its bundle.
/// Problems are returned (playback logs them; the gallery reports them).
fn attach_visuals(bundle: &jka_emotes::Bundle, emote: &mut Emote) -> Vec<String> {
    let mut warnings = Vec::new();
    if let Some(track)=&emote.contact_track {
        match bundle.read(track,16*1024*1024).and_then(|b|jka_emotes::PoseClip::parse(&b)) {
            Ok(clip) if clip.bones==1 && clip.frames==(emote.end_frame-emote.first_frame) as usize=>emote.contact_pose=Some(std::sync::Arc::new(clip)),
            _=>warnings.push(format!("EMOTE {}: invalid contact height track",emote.id)),
        }
    }
    for prop in &mut emote.props {
        let prepared=if prop.model.ends_with(".djk_prop"){prepare_prop_mesh(&prop.model)}else{prepare_static_prop(&prop.model)};
        if let Err(error)=prepared{warnings.push(format!("EMOTE PROP {}: {error}",prop.name));}
        if let Some(track)=&prop.skin_track {
            match bundle.read(track,32*1024*1024).and_then(|bytes|jka_emotes::PoseClip::parse(&bytes)){
                Ok(clip) if clip.frames==(emote.end_frame-emote.first_frame) as usize=>prop.skin_pose=Some(std::sync::Arc::new(clip)),
                _=>warnings.push(format!("EMOTE PROP {}: invalid independent animation",prop.name)),
            }
        }
        match bundle.read(&prop.track,16*1024*1024).and_then(|bytes|jka_emotes::PoseClip::parse(&bytes)) {
            Ok(clip) if clip.bones==1 && clip.frames==(emote.end_frame-emote.first_frame) as usize => prop.pose=Some(std::sync::Arc::new(clip)),
            Ok(_) => warnings.push(format!("EMOTE PROP {}: timeline mismatch",prop.name)),
            Err(error) => warnings.push(format!("EMOTE PROP {}: {error}",prop.name)),
        }
    }
    if let Some(effects)=&mut emote.effects {
        let expected=((emote.end_frame-emote.first_frame).max(1) as f32/emote.fps.max(1.)*jka_emotes::effects::TRACK_FPS).ceil() as usize;
        match bundle.read(&effects.track,64*1024*1024).and_then(|bytes|jka_emotes::effects::Track::parse(&bytes)) {
            Ok(track) if track.frames().abs_diff(expected)<=1 && track.kinds.len()==effects.renderers.len() => {
                for renderer in &effects.renderers {
                    for model in &renderer.models {
                        if let Err(error)=prepare_static_prop(model){warnings.push(format!("EMOTE EFFECT MESH {model}: {error}"));}
                    }
                }
                effects.data=Some(std::sync::Arc::new(track));
            }
            Ok(_) => warnings.push(format!("EMOTE {}: effect track timeline mismatch",emote.id)),
            Err(error) => warnings.push(format!("EMOTE {}: effect track {error}",emote.id)),
        }
    }
    emote.pose = bundle.pose.clone();
    warnings
}

/// Synchronously prepare an emote's visual assets (no audio), for offline
/// rendering such as `emote_gallery`.
pub fn prepare_visuals(mut emote: Emote) -> Result<(Emote, Vec<String>), String> {
    let prefix = emote.gla.rsplit_once('/').ok_or("missing bundle prefix")?.0.to_owned();
    let bundle = BUNDLES.get_or_init(Default::default).lock().map_err(|_| "bundle lock poisoned")?
        .get(&prefix).cloned().ok_or("unmounted emote bundle")?;
    let bundle = bundle.validate_for_playback()?;
    let warnings = attach_visuals(&bundle, &mut emote);
    Ok((emote, warnings))
}

/// Install decoded tracks before starting the shared animation/audio clock.
pub(crate) fn poll_prepared_audio() -> Vec<crate::audio::SoundDecodeResult> {
    let Ok(mut state) = playing().lock() else {
        return Vec::new();
    };
    let Some((generation, emote, mode, audio)) = state.prepared.take() else {
        return Vec::new();
    };
    if generation != state.generation {
        return Vec::new();
    }
    let matched_start = state.requested_start.take();
    let started = matched_start.unwrap_or_else(Instant::now);
    // Preserve existing first-cue behavior for ordinary dances. Only a match
    // skips past cues: replaying their music would double the target's beat.
    let next_audio_ms = if matched_start.is_some() {
        audio_times_after(&emote, started.elapsed().as_millis() as u64)
    } else {
        emote.audio.iter().map(|cue| cue.at_ms).collect()
    };
    state.playing = Some(Playing {
        emote,
        mode,
        started,
        next_audio_ms,
    });
    state.stop_pending = true;
    audio
}

fn playing() -> &'static Mutex<PlaybackState> {
    PLAYING.get_or_init(|| Mutex::new(PlaybackState::default()))
}

/// Profile-preview variant uses listener-local (1P) WAVs.
pub fn play(emote: Emote) {
    play_mode(emote, AudioMode::Preview, 0);
}
/// Cosmetic world playback; App owns the associated game taunt transmission.
pub fn play_world(emote: Emote) {
    play_mode(emote, AudioMode::World, 0);
}
/// Start this emote at the same point in its timeline as an existing player.
/// `elapsed_ms` is taken from the remote player's *presented* server timeline,
/// not from the time at which our bundle finishes asynchronously loading.
pub fn play_world_matched(emote: Emote, elapsed_ms: i32) {
    play_mode(emote, AudioMode::World, elapsed_ms.max(0) as u64);
}
fn play_mode(mut emote: Emote, mode: AudioMode, elapsed_ms: u64) {
    let now = Instant::now();
    let requested_start = now.checked_sub(std::time::Duration::from_millis(elapsed_ms)).unwrap_or(now);
    // Some exports include only one camera audience. This local client can use
    // that available PCM track for either camera rather than silently dropping it.
    for lane in [AudioLane::Music, AudioLane::Foley] {
        let target = if mode == AudioMode::World {
            AudioAudience::World
        } else {
            AudioAudience::Preview
        };
        let available = emote.audio.iter().any(|e| {
            e.lane == lane
                && e.wav.is_some()
                && (e.audience == target || e.audience == AudioAudience::Both)
        });
        if !available {
            for cue in emote.audio.iter_mut().filter(|e| e.lane == lane) {
                cue.audience = AudioAudience::Both;
            }
        }
    }
    if emote.gla.starts_with("emotes/.bundles/") {
        let generation = {
            let Ok(mut state) = playing().lock() else {
                return;
            };
            state.requested_mode = Some(mode);
            state.requested_start = (elapsed_ms > 0).then_some(requested_start);
            state.world_taunt = None;
            state.generation = state.generation.wrapping_add(1);
            state.playing = None;
            state.prepared = None;
            state.stop_pending = true;
            state.generation
        };
        prepare_emote((generation, emote, mode));
        return;
    }
    if let Ok(mut state) = playing().lock() {
        state.requested_mode = Some(mode);
        state.requested_start = None;
        state.world_taunt = None;
        state.generation = state.generation.wrapping_add(1);
        state.prepared = None;
        let next_audio_ms = if elapsed_ms > 0 {
            audio_times_after(&emote, requested_start.elapsed().as_millis() as u64)
        } else {
            emote.audio.iter().map(|cue| cue.at_ms).collect()
        };
        state.playing = Some(Playing {
            emote,
            started: requested_start,
            mode,
            next_audio_ms,
        });
        state.stop_pending = true; // stop previous emote before first new notify
    }
}

impl PlaybackState {
    fn stop(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.requested_mode = None;
        self.requested_start = None;
        self.world_taunt = None;
        self.prepared = None;
        self.playing = None;
        self.stop_pending = true;
    }

    fn cancel_world_taunt(&mut self, torso: i32, legs: i32, buttons: i32, up_move: i8) -> bool {
        if self.requested_mode != Some(AudioMode::World) {
            return false;
        }
        // Wait for the actual server/Pmove animation rather than guessing from
        // saber equipment. Keep this binding when that taunt returns to idle.
        if self.world_taunt.is_none() {
            self.world_taunt = taunt::Taunt::from_animations(torso, legs);
        }
        let cancel = match self.world_taunt.as_mut() {
            Some(taunt) => taunt.interrupted(torso, legs) || taunt.cancel_input(buttons, up_move),
            // The local shim does not execute native taunt commands. Also
            // cover inputs arriving before a remote server accepts a taunt:
            // absence of a binding must never disable attack/Force cancellation.
            None => jka_movement::taunt_action_should_cancel(buttons),
        };
        if cancel {
            self.stop();
            return true;
        }
        false
    }
}

/// World emotes retain the accepted native taunt's animation channels and cancel rule.
/// Menu previews do not bind to game animations.
/// Invalidating the generation also prevents an in-flight preparation restarting it.
pub fn cancel_world_taunt(torso: i32, legs: i32, buttons: i32, up_move: i8) -> bool {
    if let Ok(mut state) = playing().lock() {
        return state.cancel_world_taunt(torso, legs, buttons, up_move);
    }
    false
}

pub fn stop() {
    if let Ok(mut state) = playing().lock() {
        state.stop();
    }
}

/// End an in-game performance without touching a Profile preview.
pub fn stop_world() -> bool {
    if let Ok(mut state) = playing().lock() {
        if state.requested_mode == Some(AudioMode::World) {
            state.stop();
            return true;
        }
    }
    false
}

/// Called by SoundPresenter::frame on the audio-owning thread, using the same
/// Instant as animation. Scheduling is frame-granular, not DSP sample-accurate.
pub fn poll_audio() -> Vec<AudioCommand> {
    let Ok(mut state) = playing().lock() else {
        return Vec::new();
    };
    let now = state.playing.as_ref().map_or(0, |active| active.started.elapsed().as_millis().min(u64::MAX as u128) as u64);
    poll_state_audio(&mut state, now)
}

fn poll_state_audio(state: &mut PlaybackState, now: u64) -> Vec<AudioCommand> {
    let mut commands = Vec::new();
    if state.stop_pending {
        commands.push(AudioCommand::StopAll);
        state.stop_pending = false;
    }
    let Some(active) = state.playing.as_mut() else {
        return commands;
    };
    let duration = clip_duration_ms(&active.emote);
    let period = loop_period_ms(&active.emote);
    let finished = !active.emote.looping && now >= duration;
    // Gather due events in timeline order. A stalled frame never replays an
    // unlimited backlog. Late audio cannot be properly seeked by this backend.
    let mut due = Vec::new();
    for (index, cue) in active.emote.audio.iter().enumerate() {
        if cue.audience != AudioAudience::Both
            && !matches!(
                (cue.audience, active.mode),
                (AudioAudience::Preview, AudioMode::Preview)
                    | (AudioAudience::World, AudioMode::World)
            )
        {
            continue;
        }
        let mut timestamp = active.next_audio_ms[index];
        if timestamp <= now {
            // Loading/first-frame stalls must not discard the only music cue.
            due.push((timestamp, index));
            if active.emote.looping && cue.repeat_on_loop && duration > 0 {
                let audio_period = cue.repeat_period_ms.unwrap_or(period);
                let missed = (now - timestamp) / audio_period + 1;
                timestamp = timestamp.saturating_add(audio_period.saturating_mul(missed));
            } else {
                timestamp = u64::MAX;
            }
            active.next_audio_ms[index] = timestamp;
        }
    }
    due.sort_unstable();
    for (_, index) in due {
        let cue = &active.emote.audio[index];
        if let Some(wav) = &cue.wav {
            commands.push(AudioCommand::Play {
                wav: wav.clone(),
                lane: cue.lane,
                mode: active.mode,
                loop_start_sample: cue.loop_start_sample,
            });
        } else {
            commands.push(AudioCommand::StopLane(cue.lane));
        }
    }
    if finished {
        state.playing = None;
        state.stop_pending = false;
        commands.push(AudioCommand::StopAll);
    }
    commands
}
/// Skip audio events already passed on the joined timeline. Otherwise a late
/// join would restart the target's soundtrack from its beginning and produce
/// a badly offset second beat. Repeating cues may play on their *next* loop.
fn audio_times_after(emote: &Emote, elapsed_ms: u64) -> Vec<u64> {
    let period = loop_period_ms(emote);
    emote.audio.iter().map(|cue| {
        if cue.at_ms >= elapsed_ms {
            return cue.at_ms;
        }
        if !emote.looping || !cue.repeat_on_loop {
            return u64::MAX;
        }
        let step = cue.repeat_period_ms.unwrap_or(period).max(1);
        let missed = elapsed_ms.saturating_sub(cue.at_ms) / step + 1;
        cue.at_ms.saturating_add(step.saturating_mul(missed))
    }).collect()
}

fn clip_duration_ms(emote: &Emote) -> u64 {
    (((emote.end_frame - emote.first_frame) as f64 * 1000.0 / f64::from(emote.fps)).round() as u64)
        .max(1)
}

fn loop_period_ms(emote: &Emote) -> u64 {
    (((emote.end_frame - emote.loop_start_frame.unwrap_or(emote.first_frame)) as f64 * 1000.0
        / emote.fps as f64)
        .round() as u64)
        .max(1)
}

/// This is a rendering request, not a game-state animation override.
pub fn active_preview() -> Option<Preview> {
    let mut state = playing().lock().ok()?;
    let elapsed_ms = state.playing.as_ref()?.started.elapsed().as_millis().min(i32::MAX as u128) as i32;
    preview_at(&mut state, elapsed_ms)
}

fn preview_at(state: &mut PlaybackState, elapsed_ms: i32) -> Option<Preview> {
    let playing = state.playing.as_ref()?;
    let duration_ms = clip_duration_ms(&playing.emote);
    if !playing.emote.looping && u64::try_from(elapsed_ms).unwrap_or(0) >= duration_ms {
        state.playing = None;
        state.stop_pending = true;
        return None;
    }
    let elapsed_ms = if playing.emote.looping && elapsed_ms as u64 >= duration_ms {
        let period = loop_period_ms(&playing.emote);
        (duration_ms - period + (elapsed_ms as u64 - duration_ms) % period) as i32
    } else {
        elapsed_ms
    };
    Some(Preview {
        emote: playing.emote.clone(),
        elapsed_ms,
    })
}

pub fn refresh_catalog() {
    network::reset();
    if let Ok(mut cache) = CATALOG.get_or_init(Default::default).lock() {
        cache.generation = cache.generation.wrapping_add(1);
        cache.pending = false;
        cache.result = None;
    }
}

/// Never scan files or clone the entire catalogue on a UI/input frame.
pub fn start_catalog(base: &Path) {
    let cache = CATALOG.get_or_init(Default::default);
    let Ok(mut state) = cache.lock() else {
        return;
    };
    if state.base == base && (state.pending || state.result.is_some()) {
        return;
    }
    state.generation = state.generation.wrapping_add(1);
    state.base = base.to_owned();
    state.pending = true;
    state.result = None;
    let generation = state.generation;
    let base = base.to_owned();
    let result = std::thread::Builder::new()
        .name("emote-catalogue".into())
        .spawn(move || {
            let result = read_catalog(&base);
            let ids = std::fs::read(base.join("emotes/favorites.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|v| {
                    v["favorites"].as_array().map(|a| {
                        a.iter()
                            .filter_map(|id| id.as_str().map(str::to_owned))
                            .collect::<Vec<_>>()
                    })
                })
                .unwrap_or_default();
            let mut old_mounts = None;
            if let Ok(mut state) = cache.lock() {
                if state.generation != generation {
                    return;
                }
                state.pending = false;
                state.result = Some(result.map(|(entries, mounted)| {
                    if let Ok(mut mounts) = BUNDLES.get_or_init(Default::default).lock() {
                        old_mounts = Some(std::mem::replace(&mut *mounts, mounted));
                    }
                    update_favorite_assets(&entries, &ids);
                    std::sync::Arc::new(entries)
                }));
            }
            // Destruction of an old large catalogue must not block the UI lock.
            drop(old_mounts);
        });
    if let Err(error) = result {
        state.pending = false;
        state.result = Some(Err(error.to_string()));
    }
}

pub fn catalog(base: &Path) -> Result<std::sync::Arc<Vec<Emote>>, String> {
    start_catalog(base);
    CATALOG
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "emote catalogue lock poisoned")?
        .result
        .clone()
        .unwrap_or_else(|| Err("Loading emotes...".into()))
}

type BundleMounts = std::collections::HashMap<String, jka_emotes::Bundle>;
/// One bundle's catalogue rows, the runtime prefix its members are mounted
/// under, and the opened bundle. Any problem is local to this bundle.
fn read_bundle(assets: &mut jka_assets::pk3::AssetSearchPath, name: &str, links: &Value) -> Result<(Vec<Emote>, String, jka_emotes::Bundle), String> {
    let (file,range)=assets.stored_location(name).map_err(|e|e.to_string())?.ok_or("emote provider disappeared")?;
    let bundle = jka_emotes::Bundle::open_located_index(&file,name,range)?;
    let mut root = bundle.manifest.clone();
    let source_clips = root["source_audio"].as_object().map_or(0, |a| a.len());
    annotate_audio(&mut root["emotes"][0], source_clips);
    let prefix = bundle.runtime_prefix();
    let row = &mut root["emotes"][0];
    let link = &links["links"][row["id"].as_str().unwrap_or("")];
    if let (Some(title), Some(parent)) = (link["title"].as_str(),link["parent"].as_str()) {
        row["name"] = json!(title);
        row["catalog_parent"] = json!(parent);
        row["shared_montage_alias"] = json!(link["reasons"].as_array().is_some_and(|r|r.iter().any(|r|r=="shared_montage")));
        row["shared_montages"] = link["shared_montages"].clone();
    }
    for key in ["gla", "sound"] {
        if let Some(path) = row[key].as_str() {
            row[key] = Value::String(format!("{prefix}/{path}"));
        }
    }
    if let Some(variants) = row["variants"].as_array_mut() {
        for variant in variants {
            if let Some(events) = variant.get_mut("audio").and_then(Value::as_array_mut) {
                for event in events {
                    if let Some(path) = event["wav"].as_str() {
                        event["wav"] = Value::String(format!("{prefix}/{path}"));
                    }
                }
            }
        }
    }
    if let Some(events) = row.get_mut("audio").and_then(Value::as_array_mut) {
        for event in events {
            if let Some(path) = event["wav"].as_str() {
                event["wav"] = Value::String(format!("{prefix}/{path}"));
            }
        }
    }
    let mut emotes = jka_emotes::manifest::parse_manifest(&root)?;
    for parent in &mut emotes {
        let snapshot = parent.variants.clone();
        for variant in &mut parent.variants {
            if variant.audio.is_empty() {
                let identity =
                    |name: &str| name.to_ascii_lowercase().replace("_cmf", "_cmm");
                if let Some(donor) = snapshot.iter().find(|v| {
                    !v.audio.is_empty()
                        && identity(&v.variant_name) == identity(&variant.variant_name)
                        && v.end_frame - v.first_frame
                            == variant.end_frame - variant.first_frame
                        && v.looping == variant.looping
                        && v.loop_start_frame.map(|n| n - v.first_frame)
                            == variant.loop_start_frame.map(|n| n - variant.first_frame)
                }) {
                    variant.audio = donor.audio.clone();
                    variant.audio_status = donor.audio_status.clone();
                    variant.audio_detail = format!(
                        "Compatible gender variation audio from {}.\n{}",
                        donor.variant_name, donor.audio_detail
                    );
                }
            }
        }
        if parent.audio.is_empty() {
            if let Some(default) = parent
                .variants
                .iter()
                .find(|v| !v.audio.is_empty())
                .cloned()
            {
                let variants = std::mem::take(&mut parent.variants);
                *parent = default;
                parent.variants = variants;
            }
        }
    }
    emotes[0].pose = bundle.pose.clone();
    Ok((emotes, prefix, bundle))
}

fn read_catalog(base: &Path) -> Result<(Vec<Emote>, BundleMounts), String> {
    let mut assets=jka_assets::pk3::AssetSearchPath::open(base).map_err(|e|e.to_string())?;
    let mut out = Vec::new();
    let mut mounted = std::collections::HashMap::new();
    let links: Value = if let Some(asset)=assets.read("emotes/catalog-links.json",jka_emotes::MAX_MANIFEST_BYTES).map_err(|e|e.to_string())? {
        serde_json::from_slice(&asset.bytes).map_err(|e|format!("emote catalog links: {e}"))?
    } else { Value::Null };
    {
        let files=assets.names().filter(|n|n.starts_with("emotes/") && n.ends_with(".djk_fnemote")).map(str::to_owned).collect::<Vec<_>>();
        for name in files {
            // One damaged or incompatible bundle must not take the whole library
            // down with it: skip it and say why.
            match read_bundle(&mut assets, &name, &links) {
                Ok((emotes, prefix, bundle)) => {
                    out.extend(emotes);
                    mounted.insert(prefix, bundle);
                }
                Err(error) => log::warn!("emotes: skipping {name}: {error}"),
            }
        }
    }
    if let Some(asset)=assets.read("emotes/manifest.json",jka_emotes::MAX_MANIFEST_BYTES).map_err(|e|e.to_string())? {
        let root: Value = serde_json::from_slice(&asset.bytes).map_err(|e| e.to_string())?;
        out.extend(jka_emotes::manifest::parse_manifest(&root)?);
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|emote| {
        let first = seen.insert(emote.id.clone());
        if !first {
            log::warn!("emotes: duplicate emote ID {} ignored", emote.id);
        }
        first
    });
    out.sort_by(|a, b| {
        display_title(a)
            .cmp(display_title(b))
            .then_with(|| a.id.cmp(&b.id))
    });
    // Only shared montage aliases with the identical clip clock may inherit
    // audio. A group follower with different motion remains explicitly silent.
    let donors: Vec<_> = out.iter().flat_map(|e|std::iter::once(e).chain(&e.variants)).filter(|e|!e.audio.is_empty()).cloned().collect();
    for parent in &mut out {
            let family = parent.catalog_parent.clone();
            if parent.shared_montage_alias {
                let apply = |row:&mut Emote| {
                    if row.audio.is_empty() && row.shared_montages.iter().any(|n| row.variant_name.split(" / ").next().is_some_and(|v|v.eq_ignore_ascii_case(n))) {
                        if let Some(donor) = donors.iter().find(|d| Some(&d.id)==family.as_ref() && d.variant_name==row.variant_name && d.fps==row.fps && d.end_frame-d.first_frame==row.end_frame-row.first_frame && d.looping==row.looping && d.loop_start_frame.map(|n|n-d.first_frame)==row.loop_start_frame.map(|n|n-row.first_frame)) {
                            row.audio=donor.audio.clone();
                            row.audio_status=donor.audio_status.clone();
                            row.audio_detail=format!("Audio from the same source montage in {}.\n{}",donor.id,donor.audio_detail);
                        }
                    }
                };
                apply(parent);
                for row in &mut parent.variants { apply(row); }
            }
    }
    Ok((group_titles(out), mounted))
}

/// Keep commands and saved favorites addressing an internal role after grouping.
pub fn resolve_catalog_id(entries: &[Emote], id: &str) -> Option<Emote> {
    if let Some(entry)=entries.iter().find(|e|e.id.eq_ignore_ascii_case(id) || e.catalog_aliases.iter().any(|alias|alias.eq_ignore_ascii_case(id))) { return Some(entry.clone()); }
    for parent in entries {
        let variants: Vec<_>=parent.variants.iter().filter(|v|v.id.eq_ignore_ascii_case(id)).cloned().collect();
        if let Some(first)=variants.first() {
            let mut result=first.clone();
            result.variants=variants;
            return Some(result);
        }
    }
    None
}

/// Exported follower/owned/instrument items can share an emote title. Keep
/// their complete bundles mounted, but expose their roles as variations of
/// one catalogue row. Prefer the ordinary audible item as the default.
fn group_titles(mut entries: Vec<Emote>) -> Vec<Emote> {
    entries.sort_by_key(|e| {
        (
            display_title(e).to_lowercase(),
            e.catalog_parent.is_some(),
            e.id.to_lowercase().contains("follower") || e.id.to_lowercase().contains("joiner"),
            e.audio.is_empty(),
            e.id.clone(),
        )
    });
    let mut output: Vec<Emote> = Vec::new();
    for entry in entries {
        if let Some(parent) = output
            .last_mut()
            .filter(|p| display_title(p).eq_ignore_ascii_case(display_title(&entry)))
        {
            if entry.shared_montage_alias {
                let rows: Vec<_> = if entry.variants.is_empty() {vec![&entry]} else {entry.variants.iter().collect()};
                if rows.iter().all(|v|v.shared_montages.iter().any(|n|v.variant_name.split(" / ").next().is_some_and(|s|s.eq_ignore_ascii_case(n))) && parent.variants.iter().chain(std::iter::once(&*parent)).any(|p|p.variant_name==v.variant_name && p.fps==v.fps && p.end_frame-p.first_frame==v.end_frame-v.first_frame && p.looping==v.looping && p.loop_start_frame.map(|n|n-p.first_frame)==v.loop_start_frame.map(|n|n-v.first_frame))) {
                    parent.catalog_aliases.push(entry.id);
                    continue;
                }
            }
            if parent.variants.is_empty() {
                let mut primary = parent.clone();
                primary.variants.clear();
                parent.variants.push(primary);
            }
            if entry.variants.is_empty() {
                parent.variants.push(entry);
            } else {
                parent.variants.extend(entry.variants);
            }
        } else {
            output.push(entry);
        }
    }
    // Some group items are request/waiting intros; the source-linked owned or
    // leader role contains the performance. Select the complete audible role,
    // preserving its pose and clock together rather than borrowing its sound.
    for parent in &mut output {
        if !parent.audio.is_empty() { continue; }
        let audible = |v:&&Emote| !v.audio.is_empty();
        let best=parent.variants.iter().filter(audible).find(|v|!v.id.to_ascii_lowercase().contains("follower") && !v.id.to_ascii_lowercase().contains("joiner"))
            .or_else(||parent.variants.iter().find(|v|!v.audio.is_empty())).cloned();
        if let Some(mut best)=best {
            let role=best.id.clone();
            best.id=parent.id.clone();
            best.label=parent.label.clone();
            best.catalog_parent=parent.catalog_parent.clone();
            best.catalog_aliases=std::mem::take(&mut parent.catalog_aliases);
            best.variants=std::mem::take(&mut parent.variants);
            best.audio_detail=format!("Default performance from source-linked role {role}.\n{}",best.audio_detail);
            *parent=best;
        }
    }
    output
}

static BUNDLES: OnceLock<Mutex<std::collections::HashMap<String, jka_emotes::Bundle>>> =
    OnceLock::new();

/// Read a mounted bundle member directly; no extraction or second asset tree.
pub fn read_asset(qpath: &str, limit: usize) -> Result<Option<Vec<u8>>, String> {
    if !qpath.starts_with("emotes/.bundles/") {
        return Ok(None);
    }
    let mut parts = qpath.splitn(4, '/');
    parts.next();
    parts.next();
    let id = parts.next().ok_or("missing bundle ID")?;
    let member = parts.next().ok_or("missing bundle member")?;
    let prefix = format!("emotes/.bundles/{id}");
    let bundle = BUNDLES
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "emote bundle lock poisoned")?
        .get(&prefix)
        .cloned()
        .ok_or_else(|| format!("unmounted emote bundle {prefix}"))?;
    // No disk I/O while holding the catalogue mount lock.
    bundle.read(member, limit).map(Some)
}

#[cfg(test)]
mod tests {
    #[test]
    fn unbound_world_emote_cancels_pending_preparation_on_attack() {
        use super::*;
        let idle = jka_assets::animation::animation_index("BOTH_STAND1").unwrap() as i32;
        let emote = jka_emotes::manifest::parse_manifest(&json!({"emotes":[{
            "id":"pending", "name":"Pending", "gla":"test.gla", "first_frame":0, "end_frame":30, "fps":30
        }]})).unwrap().remove(0);
        let mut state = PlaybackState {
            requested_mode: Some(AudioMode::World), generation: 7,
            prepared: Some((7, emote, AudioMode::World, Vec::new())),
            ..Default::default()
        };
        assert!(!state.cancel_world_taunt(idle, idle, 0, -127));
        assert!(state.world_taunt.is_none());
        assert!(state.cancel_world_taunt(idle, idle, jka_movement::BUTTON_ATTACK, 0));
        assert_eq!(state.generation, 8);
        assert!(state.prepared.is_none());
        assert!(state.stop_pending);
    }

    #[test]
    fn native_taunt_cancellation_invalidates_world_preparation_but_keeps_preview() {
        use super::*;
        let dual = jka_assets::animation::animation_index("BOTH_DUAL_TAUNT").unwrap() as i32;
        let emote = jka_emotes::manifest::parse_manifest(&serde_json::json!({"emotes":[{
            "id":"test", "name":"Test", "gla":"a.gla", "first_frame":0, "end_frame":30, "fps":30
        }]})).unwrap().remove(0);
        let mut state = PlaybackState {
            requested_mode: Some(AudioMode::World), generation: 7,
            prepared: Some((7, emote.clone(), AudioMode::World, Vec::new())),
            ..Default::default()
        };
        state.cancel_world_taunt(dual, dual, jka_movement::BUTTON_WALKING, 0);
        assert!(state.prepared.is_some());
        state.cancel_world_taunt(dual, dual, jka_movement::BUTTON_ATTACK, 0);
        assert_eq!(state.generation, 8);
        assert!(state.prepared.is_none());
        assert!(state.playing.is_none());
        assert!(state.stop_pending);

        state.requested_mode = Some(AudioMode::Preview);
        state.prepared = Some((8, emote.clone(), AudioMode::Preview, Vec::new()));
        state.cancel_world_taunt(dual, dual, jka_movement::BUTTON_ATTACK, 127);
        assert_eq!(state.generation, 8);
        assert!(state.prepared.is_some());

        state.requested_mode = Some(AudioMode::World);
        state.playing = Some(Playing { emote, started: Instant::now(), mode: AudioMode::World, next_audio_ms: Vec::new() });
        state.cancel_world_taunt(dual, dual, 0, -127);
        assert!(state.playing.is_none());
        assert!(state.prepared.is_none());
    }

    #[test]
    fn world_taunt_remembers_its_channels_after_native_expiry() {
        let anim = |name| jka_assets::animation::animation_index(name).unwrap() as i32;
        let idle = anim("BOTH_STAND1");
        let crouch = anim("BOTH_CROUCH1");
        let attack = anim("BOTH_ATTACK1");
        let engage = anim("BOTH_ENGAGETAUNT");
        let dual = anim("BOTH_DUAL_TAUNT");
        let mut torso = PlaybackState { requested_mode: Some(AudioMode::World), ..Default::default() };
        // Crouch before the server accepts the taunt must not guess a saber style.
        assert!(!torso.cancel_world_taunt(idle, crouch, 0, -127));
        assert!(!torso.cancel_world_taunt(engage, idle, 0, 0));
        assert!(!torso.cancel_world_taunt(engage, crouch, 0, -127));
        // A torso-locked attack input does not end the native taunt either.
        assert!(!torso.cancel_world_taunt(engage, crouch, jka_movement::BUTTON_ATTACK, 0));
        assert!(!torso.cancel_world_taunt(idle, idle, 0, 0));
        assert!(!torso.cancel_world_taunt(idle, crouch, 0, -127));
        assert!(torso.cancel_world_taunt(attack, crouch, 0, 0));
        assert!(torso.world_taunt.is_none());

        let mut full = PlaybackState { requested_mode: Some(AudioMode::World), ..Default::default() };
        // A sliding dual taunt becomes full-body when the player comes to rest.
        assert!(!full.cancel_world_taunt(dual, idle, 0, 0));
        assert!(!full.cancel_world_taunt(dual, dual, 0, 0));
        assert!(!full.cancel_world_taunt(idle, idle, 0, 0));
        assert!(full.cancel_world_taunt(idle, idle, 0, -127));
    }
    use super::*;
    fn wait_for_prepared() {
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        while active_preview().is_none() {
            let _ = poll_prepared_audio();
            assert!(Instant::now() < deadline, "emote preparation timed out");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    #[test]
    fn matched_dance_starts_at_remote_phase_and_never_replays_past_music() {
        let emote = jka_emotes::manifest::parse_manifest(&json!({"emotes":[{
            "id":"match_test", "name":"Match", "gla":"a.gla",
            "first_frame":0, "end_frame":300, "fps":30, "loop":true,
            "audio":[
                {"at_ms":0, "lane":"music", "audience":"both", "wav":"music.wav"},
                {"at_ms":6000, "lane":"foley", "audience":"both", "wav":"foot.wav"}
            ]
        }]})).unwrap().remove(0);
        let times = audio_times_after(&emote, 5500);
        // The target's music is already audible; don't double it at t=0.
        assert_eq!(times[0], u64::MAX);
        assert_eq!(times[1], 6000);
        let mut state = PlaybackState::default();
        state.playing = Some(Playing {
            emote,
            started: Instant::now(),
            mode: AudioMode::World,
            next_audio_ms: times,
        });
        assert_eq!(preview_at(&mut state, 5500).unwrap().elapsed_ms, 5500);
        assert_eq!(preview_at(&mut state, 12000).unwrap().elapsed_ms, 2000);
    }

    #[test]
    fn audio_diagnostics_distinguish_missing_assets_from_unmapped_variations() {
        let make = || json!({"id":"x","name":"X","gla":"a.gla","first_frame":0,"end_frame":30,"fps":30,"variants":[{"first_frame":30,"end_frame":60,"fps":30,"audio":[]}]});
        for (clips, status, label) in [
            (0, "missing_source", "source assets missing"),
            (2, "unmapped", "mapping unresolved"),
        ] {
            let mut row = make();
            annotate_audio(&mut row, clips);
            let e = jka_emotes::manifest::parse_manifest(&json!({"emotes":[row]}))
                .unwrap()
                .remove(0);
            assert_eq!(e.variants[0].audio_status, status);
            assert!(audio_status_label(&e.variants[0]).contains(label));
        }
        let mut row = make();
        row["audio"] = json!([{"at_ms":0,"lane":"foley","audience":"both","wav":"sound.wav"}]);
        annotate_audio(&mut row, 2);
        assert_eq!(row["audio_status"], "foley_only");
        assert_eq!(row["variants"][0]["audio_status"], "unmapped");
    }
    #[test]
    fn preferred_default_follows_the_audible_catalogue_variation() {
        let root = json!({"emotes":[{"id":"x","name":"X","gla":"x.gla","first_frame":30,"end_frame":60,"fps":30,"audio":[{"at_ms":0,"lane":"music","audience":"both","wav":"x.wav"}],"variants":[{"first_frame":0,"end_frame":30,"fps":30,"audio":[]},{"first_frame":30,"end_frame":60,"fps":30,"audio":[{"at_ms":0,"lane":"music","audience":"both","wav":"x.wav"}]}]}]});
        let e = jka_emotes::manifest::parse_manifest(&root)
            .unwrap()
            .remove(0);
        assert_eq!(default_variant_index(&e), 1);
        assert!(
            e.variants[default_variant_index(&e)]
                .audio
                .iter()
                .any(|a| a.wav.is_some())
        );
    }
    #[test]
    fn catalogue_uses_packed_bundles_once_when_loose_copies_also_exist(){
        let fixture=crate::cvar_compatibility::Fixture::new();let folder=fixture.0.join("emotes");std::fs::create_dir_all(&folder).unwrap();
        let target=jka_assets::ghoul2::openjk_default_gla().unwrap();
        let gla=jka_emotes::retarget::encode(&target,1,&[(glam::Quat::IDENTITY,glam::Vec3::ZERO)]).unwrap();
        let manifest=json!({"format":"djk_fnemote","version":1,"emotes":[{"id":"test","name":"Test","gla":"a.gla","first_frame":0,"end_frame":1,"fps":30,"loop":false}]});
        let bundle=folder.join("EID_Test.djk_fnemote");jka_emotes::write_bundle(&bundle,&manifest,&std::collections::BTreeMap::from([("a.gla".into(),gla)])).unwrap();
        jka_emotes::package::library(&folder,None,&fixture.0.join("library.pk3")).unwrap();
        let (entries,mounts)=read_catalog(&fixture.0).unwrap();assert_eq!(entries.len(),1);assert_eq!(mounts.len(),1);assert!(!entries[0].looping);
        std::fs::remove_file(bundle).unwrap();
        let (entries,mounts)=read_catalog(&fixture.0).unwrap();assert_eq!(entries.len(),1);mounts.values().next().unwrap().validate_for_playback().unwrap();
    }
    #[test]
    fn catalogue_groups_roles_under_one_title_without_losing_bundles() {
        let root = serde_json::json!({"emotes":[
            {"id":"dance_follower","name":"Dance","gla":"b.gla","first_frame":0,"end_frame":30,"fps":30},
            {"id":"dance","name":"Dance","gla":"a.gla","first_frame":0,"end_frame":30,"fps":30,"sound":"a.wav"},
            {"id":"other","name":"Other","gla":"c.gla","first_frame":0,"end_frame":30,"fps":30}]});
        let grouped = group_titles(jka_emotes::manifest::parse_manifest(&root).unwrap());
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].id, "dance");
        assert_eq!(grouped[0].variants.len(), 2);
        assert_eq!(grouped[0].variants[1].gla, "b.gla");
    }
    #[test]
    fn grouped_internal_role_keeps_saved_id_and_favorite_assets() {
        let root=json!({"emotes":[
            {"id":"main","name":"Dance","gla":"emotes/.bundles/main/animation.gla","first_frame":0,"end_frame":30,"fps":30},
            {"id":"internal","name":"Dance","catalog_parent":"main","gla":"emotes/.bundles/internal/animation.gla","first_frame":0,"end_frame":30,"fps":30}]});
        let grouped=group_titles(jka_emotes::manifest::parse_manifest(&root).unwrap());
        assert_eq!(grouped.len(),1);
        assert_eq!(grouped[0].id,"main");
        let role=resolve_catalog_id(&grouped,"internal").unwrap();
        assert_eq!(role.id,"internal");
        assert!(role.gla.contains("/internal/"));
        update_favorite_assets(&grouped,&["internal".into()]);
        assert!(favorite_asset("emotes/.bundles/internal/animation.pose"));
    }
    #[test]
    fn identical_source_alias_does_not_create_duplicate_variation() {
        let root=json!({"emotes":[
            {"id":"main","name":"Dance","variant_name":"Main_M","gla":"a.gla","first_frame":0,"end_frame":60,"fps":30,"loop":true},
            {"id":"copy","name":"Dance","catalog_parent":"main","shared_montage_alias":true,"shared_montages":["Main_M"],"variant_name":"Main_M","gla":"b.gla","first_frame":120,"end_frame":180,"fps":30,"loop":true}]});
        let grouped=group_titles(jka_emotes::manifest::parse_manifest(&root).unwrap());
        assert_eq!(grouped.len(),1);
        assert!(grouped[0].variants.is_empty());
        assert_eq!(grouped[0].catalog_aliases,vec!["copy"]);
        assert_eq!(resolve_catalog_id(&grouped,"copy").unwrap().gla,"a.gla");
    }
    #[test]
    fn silent_group_request_defaults_to_the_complete_audible_owner_role() {
        let root=json!({"emotes":[
            {"id":"main","name":"Dance","gla":"intro.gla","first_frame":0,"end_frame":30,"fps":30},
            {"id":"owned","name":"Dance","catalog_parent":"main","gla":"body.gla","first_frame":0,"end_frame":90,"fps":30,"sound":"music.wav"}]});
        let grouped=group_titles(jka_emotes::manifest::parse_manifest(&root).unwrap());
        assert_eq!(grouped[0].id,"main");
        assert_eq!(grouped[0].gla,"body.gla");
        assert_eq!(grouped[0].end_frame,90);
        assert_eq!(default_variant_index(&grouped[0]),1);
        assert_eq!(resolve_catalog_id(&grouped,"main").unwrap().gla,"body.gla");
        assert_eq!(resolve_catalog_id(&grouped,"owned").unwrap().gla,"body.gla");
    }
    #[test]
    fn disallow_escaping_asset_root() {
        assert!(jka_emotes::manifest::qpath("../models/x.gla").is_err());
        assert!(jka_emotes::manifest::qpath("C:/models/x.gla").is_err());
        assert!(jka_emotes::manifest::qpath("models/players/_humanoid/dance.gla").is_ok());
    }
    #[test]
    fn playback_clock_filters_audience_repeats_and_stops() {
        let root = serde_json::json!({"emotes":[{"id":"clock","name":"Clock","gla":"a.gla",
            "first_frame":0,"end_frame":30,"fps":30,"loop":true,"audio":[
                {"at_ms":0,"lane":"music","audience":"1p","wav":"one.wav","repeat_on_loop":true},
                {"at_ms":0,"lane":"music","audience":"3p","wav":"three.wav","repeat_on_loop":true},
                {"at_ms":500,"lane":"foley","audience":"both"}]}]});
        let emote = jka_emotes::manifest::parse_manifest(&root)
            .unwrap()
            .remove(0);
        play(emote.clone());
        playing().lock().unwrap().playing.as_mut().unwrap().started =
            Instant::now() - std::time::Duration::from_millis(400);
        let first = poll_audio();
        assert!(matches!(first.first(), Some(AudioCommand::StopAll)));
        assert!(
            first
                .iter()
                .any(|c| matches!(c,AudioCommand::Play{wav,..} if wav=="one.wav"))
        );
        assert!(
            !first
                .iter()
                .any(|c| matches!(c,AudioCommand::Play{wav,..} if wav=="three.wav"))
        );
        assert!(poll_audio().is_empty());
        playing().lock().unwrap().playing.as_mut().unwrap().started =
            Instant::now() - std::time::Duration::from_millis(1050);
        assert!(
            poll_audio()
                .iter()
                .any(|c| matches!(c,AudioCommand::Play{wav,..} if wav=="one.wav"))
        );
        play_world(emote);
        assert!(
            poll_audio()
                .iter()
                .any(|c| matches!(c,AudioCommand::Play{wav,..} if wav=="three.wav"))
        );
        stop();
        assert!(matches!(poll_audio().as_slice(), [AudioCommand::StopAll]));
        assert!(active_preview().is_none());
    }

    #[test]
    #[ignore = "requires DJK_EMOTE_TEST_BASE containing a compiled Dance Moves bundle"]
    fn real_bundle_catalog_reads_and_decodes_embedded_assets() {
        let base = std::env::var("DJK_EMOTE_TEST_BASE").expect("DJK_EMOTE_TEST_BASE");
        let base = Path::new(&base);
        refresh_catalog();
        let deadline = Instant::now() + std::time::Duration::from_secs(30);
        let entries = loop {
            if let Ok(entries) = catalog(base) {
                break entries;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let bundle_count = std::fs::read_dir(base.join("emotes"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| {
                e.path()
                    .extension()
                    .is_some_and(|v| v.eq_ignore_ascii_case("djk_fnemote"))
            })
            .count();
        if !base.join("emotes/manifest.json").exists() {
            assert!(entries.len() <= bundle_count);
            let titles: std::collections::HashSet<_> = entries
                .iter()
                .map(|e| display_title(e).to_lowercase())
                .collect();
            assert_eq!(titles.len(), entries.len());
        }
        assert!(entries.iter().all(|e| e.pose.is_none()));
        let emote = entries
            .iter()
            .cloned()
            .find(|e| e.id == "EID_DanceMoves")
            .unwrap();
        assert!(emote.pose.is_none()); // Catalogue keeps only descriptors.
        play(emote.clone());
        wait_for_prepared();
        assert!(active_preview().unwrap().emote.pose.is_some());
        stop();
        let gla = read_asset(&emote.gla, jka_emotes::MAX_MEMBER_BYTES)
            .unwrap()
            .unwrap();
        assert!(matches!(
            jka_assets::ghoul2::parse_gla(&gla).unwrap().num_frames,
            1 | 204
        ));
        let mut sound =
            crate::audio::SoundAssets::new(jka_assets::pk3::AssetSearchPath::open(base).unwrap());
        sound
            .register(emote.audio[0].wav.as_ref().unwrap())
            .unwrap();
        for id in [
            "EID_LaughTrack",
            "EID_DriedSilk",
            "EID_Worm",
            "EID_Floss",
            "EID_OneArmFloss",
            "EID_WindmillFloss",
            "EID_FlossYawn",
        ] {
            if !entries.iter().any(|e| e.id == id) {
                continue;
            }
            let entry = entries.iter().find(|e| e.id == id).unwrap().clone();
            play_world(entry.clone());
            wait_for_prepared();
            assert!(active_preview().unwrap().emote.pose.is_some());
            let commands = poll_audio();
            assert!(
                commands.iter().any(|c| matches!(
                    c,
                    AudioCommand::Play {
                        mode: AudioMode::World,
                        ..
                    }
                )),
                "missing world audio for {id}"
            );
            for cue in &entry.audio {
                sound.register(cue.wav.as_ref().unwrap()).unwrap();
            }
            stop();
            for variant in entry.variants {
                for cue in &variant.audio {
                    if let Some(path) = &cue.wav {
                        sound.register(path).unwrap();
                    }
                }
                play_world(variant.clone());
                wait_for_prepared();
                let preview = active_preview().unwrap();
                let pose = preview.emote.pose.as_ref().unwrap();
                let sampled = pose.sample_range(
                    0.0 as i32,
                    variant.fps,
                    variant.first_frame as usize,
                    variant.end_frame as usize,
                    variant.loop_start_frame.map(|n| n as usize),
                );
                assert!(sampled.iter().flatten().flatten().all(|n| n.is_finite()));
                assert!(
                    poll_audio().iter().any(|c| matches!(
                        c,
                        AudioCommand::Play {
                            mode: AudioMode::World,
                            ..
                        }
                    )),
                    "missing audio for {}",
                    variant.variant_name
                );
                stop();
            }
        }
        if let Some(baller) = entries.iter().find(|e| e.id == "EID_Basketball") {
            assert!(baller.looping);
            assert_eq!(
                baller.loop_start_frame.map(|f| f - baller.first_frame),
                Some(10)
            );
            play_world(baller.clone());
            wait_for_prepared();
            let first = poll_audio();
            assert!(first.iter().any(|c| matches!(
                c,
                AudioCommand::Play {
                    loop_start_sample: Some(_),
                    ..
                }
            )));
            for time in [20_000, 90_000] {
                playing().lock().unwrap().playing.as_mut().unwrap().started =
                    Instant::now() - std::time::Duration::from_millis(time);
                let commands = poll_audio();
                assert!(
                    !commands.iter().any(|c| matches!(
                        c,
                        AudioCommand::Play {
                            lane: AudioLane::Music,
                            ..
                        } | AudioCommand::StopAll
                    )),
                    "sample-clock music must keep its existing voice"
                );
                assert!(active_preview().is_some());
            }
            stop();
        }
        if let Some(balletic) = entries.iter().find(|e| e.id == "EID_BalletJumps") {
            assert_eq!(balletic.audio_status, "missing_source");
            assert!(audio_status_label(balletic).contains("No audio"));
        }
    }
}
