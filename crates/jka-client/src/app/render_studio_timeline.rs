//! Deterministic Render Studio NLE timeline model, independent of egui/Taffy.
//! Times are seconds; source end frames are exclusive; motion-key times are
//! relative to the untrimmed source clip.  Output evaluation never depends on UI FPS.
use crate::emotes::{Emote, Preview};
use super::render_studio_camera::{StudioCameraRig,StudioCameraOrbit};
use super::render_studio_audio::StudioAudioTrack;
use super::render_studio_director::StudioDirectorShot;
use serde::{Deserialize, Serialize};
use std::time::Instant;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(in crate::app) struct StudioMotionKey {
    /// Time measured from source_first, not the truncated visible clip edge.
    pub time: f64,
    /// Character offset in JKA world units, relative to the preview stage origin.
    pub position: [f32; 3],
    pub yaw: f32,
}
impl Default for StudioMotionKey {
    fn default() -> Self { Self { time: 0.0, position: [0.0; 3], yaw: 0.0 } }
}

/// Deterministic, positive source-time speed ramp shared by animation and demo clips.
/// Keys are in untrimmed source seconds; linear speed interpolation is integrated
/// exactly in source space. Keys outside a visible trim remain intact.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(in crate::app) struct StudioSpeedKey {
    pub time: f64,
    pub rate: f64,
}
impl StudioSpeedKey {
    pub const MIN: f64 = 0.1;
    pub const MAX: f64 = 8.0;
}

pub(in crate::app) fn studio_speed_valid(keys: &[StudioSpeedKey], source_limit: f64) -> bool {
    keys.len() <= 256 && keys.iter().all(|k| k.time.is_finite() && k.rate.is_finite() &&
        (0.0..=source_limit + 0.000001).contains(&k.time) &&
        (StudioSpeedKey::MIN..=StudioSpeedKey::MAX).contains(&k.rate)) &&
        keys.windows(2).all(|w| w[0].time < w[1].time - 0.000001)
}
pub(in crate::app) fn studio_speed_at(keys: &[StudioSpeedKey], source: f64) -> f64 {
    let Some(first) = keys.first() else { return 1.0 };
    if source <= first.time { return first.rate; }
    // Keys are sorted (studio_speed_valid); first key at or after `source`.
    let i=keys.partition_point(|k|k.time<source);
    let Some(&b)=keys.get(i) else { return keys[keys.len()-1].rate };
    let a=keys[i-1];
    let t=((source-a.time)/(b.time-a.time).max(0.000001)).clamp(0.0,1.0);
    a.rate + (b.rate-a.rate)*t
}
/// Timeline seconds spent crossing one linear-rate segment: ∫dx/r.
fn studio_speed_segment_span(left:f64, right:f64, width:f64) -> f64 {
    if (left-right).abs()<0.00000001 {width/left} else {width*(right/left).ln()/(right-left)}
}
/// Source distance covered `elapsed` timeline seconds into a linear-rate
/// segment that starts at rate `from` and reaches `to` after `width`.
/// Exact inverse of studio_speed_segment_span: r0·(e^{m·t}−1)/m.
fn studio_speed_segment_advance(from:f64, to:f64, width:f64, elapsed:f64) -> f64 {
    if (from-to).abs()<0.00000001 {return elapsed*from;}
    let slope=(to-from)/width;
    from*(slope*elapsed).exp_m1()/slope
}
/// Keys strictly inside (a, b), in order.
fn studio_speed_inner(keys: &[StudioSpeedKey], a: f64, b: f64) -> &[StudioSpeedKey] {
    let start=keys.partition_point(|k|k.time<=a);
    let end=keys.partition_point(|k|k.time<b).max(start);
    &keys[start..end]
}
/// Timeline seconds elapsed while traversing a source interval. Integrating
/// 1/speed (rather than speed) makes trims and key interpolation accurate.
pub(in crate::app) fn studio_speed_span(keys: &[StudioSpeedKey], a: f64, b: f64) -> f64 {
    if (a-b).abs()<0.000000001 {return 0.0;}
    if b<a {return -studio_speed_span(keys,b,a);}
    let (mut previous,mut left)=(a,studio_speed_at(keys,a).max(StudioSpeedKey::MIN));
    let mut duration=0.0;
    for key in studio_speed_inner(keys,a,b) {
        let right=key.rate.max(StudioSpeedKey::MIN);
        duration+=studio_speed_segment_span(left,right,key.time-previous);
        (previous,left)=(key.time,right);
    }
    duration+studio_speed_segment_span(left,studio_speed_at(keys,b).max(StudioSpeedKey::MIN),b-previous)
}
/// Inverse of studio_speed_span: one ordered walk, closed form inside the
/// segment that contains `elapsed`. Exact and independent of frame rate.
pub(in crate::app) fn studio_speed_source_at(keys: &[StudioSpeedKey], source_in:f64,
    source_out:f64, elapsed:f64) -> f64 {
    if elapsed<=0.0 {return source_in;}
    if source_out-source_in<0.000000001 {return source_out;}
    // Ordinary playback and constant-speed clips have an exact inverse. Keep
    // pasted key times and loop endpoint samples free of accumulation noise.
    let rate=studio_speed_at(keys,source_in);
    if keys.iter().all(|key|key.rate==rate) {
        if elapsed>=studio_speed_span(keys,source_in,source_out) {return source_out;}
        return (source_in+elapsed*rate).clamp(source_in,source_out);
    }
    let (mut previous,mut left)=(source_in,rate.max(StudioSpeedKey::MIN));
    let mut remaining=elapsed;
    let inner=studio_speed_inner(keys,source_in,source_out).iter().map(|k|(k.time,k.rate))
        .chain(std::iter::once((source_out,studio_speed_at(keys,source_out))));
    for (bound,rate) in inner {
        let right=rate.max(StudioSpeedKey::MIN);
        let width=bound-previous;
        let span=studio_speed_segment_span(left,right,width);
        if remaining<span {
            return (previous+studio_speed_segment_advance(left,right,width,remaining)).clamp(previous,bound);
        }
        remaining-=span;
        (previous,left)=(bound,right);
    }
    source_out
}
/// Shift an edge by timeline seconds, allowing movement in either direction.
pub(in crate::app) fn studio_speed_shift_source(keys:&[StudioSpeedKey], initial:f64,
    delta:f64, minimum:f64, maximum:f64) -> f64 {
    if delta>=0.0 {return studio_speed_source_at(keys,initial,maximum,delta);}
    if initial-minimum<0.000000001 {return initial;}
    // Reverse walk from `initial` down to `minimum`.
    let (mut previous,mut right)=(initial,studio_speed_at(keys,initial).max(StudioSpeedKey::MIN));
    let mut remaining=-delta;
    let inner=studio_speed_inner(keys,minimum,initial).iter().rev().map(|k|(k.time,k.rate))
        .chain(std::iter::once((minimum,studio_speed_at(keys,minimum))));
    for (bound,rate) in inner {
        let left=rate.max(StudioSpeedKey::MIN);
        let width=previous-bound;
        let span=studio_speed_segment_span(left,right,width);
        if remaining<span {
            return (previous-studio_speed_segment_advance(right,left,width,remaining)).clamp(bound,previous);
        }
        remaining-=span;
        (previous,right)=(bound,left);
    }
    minimum
}
/// Adds/updates a key in source time. A first key away from zero seeds the
/// original 1x rate, so adding automation never unexpectedly retimes the past.
pub(in crate::app) fn studio_add_speed_key(keys:&mut Vec<StudioSpeedKey>,time:f64, rate:f64) {
    let rate=rate.clamp(StudioSpeedKey::MIN,StudioSpeedKey::MAX);
    if keys.is_empty() && time>0.00001 {keys.push(StudioSpeedKey{time:0.0,rate:1.0});}
    if let Some(existing)=keys.iter_mut().find(|k|(k.time-time).abs()<0.00001) {existing.rate=rate;return;}
    keys.push(StudioSpeedKey{time,rate});
    keys.sort_by(|a,b|a.time.total_cmp(&b.time));
}

#[derive(Clone)]
pub(in crate::app) struct StudioAnimationClip {
    pub name: String,
    pub emote: Option<Emote>,
    pub start: f64,
    pub first_frame: i32,
    pub end_frame: i32,
    pub source_fps: f64,
    pub lane: usize,
    pub source_first: i32,
    pub source_end: i32,
    pub motion: Vec<StudioMotionKey>,
    /// Extra frames after the authored clip. Rendered by holding or looping.
    pub tail_frames: i32,
    pub playback: StudioClipPlayback,
    /// Source-anchored variable playback speed; empty means normal 1x.
    pub speed_keys: Vec<StudioSpeedKey>,
    /// Smooth pose/position transitions with the touching clips of this track.
    pub blend: StudioClipBlend,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::app) enum StudioClipPlayback { #[default] Hold, Loop }

/// Cross-fade with the clip that touches this one on the same track:
/// `blend_in` from the previous clip's last pose, `blend_out` into the next
/// clip's first pose (seconds). Both sides set = one symmetric cross-fade.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(in crate::app) struct StudioClipBlend {
    #[serde(default)] pub blend_in: f64,
    #[serde(default)] pub blend_out: f64,
}
impl StudioClipBlend {
    pub const MAX: f64 = 3.0;
    pub fn valid(&self) -> bool {
        [self.blend_in, self.blend_out].iter().all(|v| v.is_finite() && (0.0..=Self::MAX).contains(v))
    }
}

/// The other half of a smoothed transition at some time: which clip, the
/// timeline time to sample it at, and its weight (0..1) in the blend.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) struct StudioPoseBlendPartner { pub clip: usize, pub sample_time: f64, pub weight: f32 }

impl StudioAnimationClip {
    pub fn source_in_seconds(&self) -> f64 {
        f64::from(self.first_frame-self.source_first)/self.source_fps.max(1.0)
    }
    pub fn source_out_seconds(&self) -> f64 {
        f64::from(self.end_frame-self.source_first+self.tail_frames.max(0))/self.source_fps.max(1.0)
    }
    pub fn duration(&self) -> f64 {
        studio_speed_span(&self.speed_keys,self.source_in_seconds(),self.source_out_seconds()).max(0.001)
    }
    pub fn source_duration(&self) -> f64 { f64::from(self.end_frame-self.first_frame).max(1.0)/self.source_fps.max(1.0) }
    pub fn end(&self) -> f64 { self.start + self.duration() }
    pub fn label(&self) -> &str { self.emote.as_ref().map_or(self.name.as_str(), |e| e.label.as_str()) }
    pub fn source_key_time(&self, timeline_time:f64) -> f64 {
        studio_speed_source_at(&self.speed_keys,self.source_in_seconds(),self.source_out_seconds(),
            (timeline_time-self.start).clamp(0.0,self.duration()))
    }
    pub fn timeline_at_source(&self, source_time:f64) -> f64 {
        self.start + studio_speed_span(&self.speed_keys,self.source_in_seconds(),source_time)
    }
    pub fn pose_time_ms(&self, time: f64) -> i32 {
        let elapsed=(self.source_key_time(time)-self.source_in_seconds()).max(0.0);
        let native=self.source_duration();
        let local=match self.playback {
            StudioClipPlayback::Hold => elapsed.min((native-0.00001).max(0.0)),
            StudioClipPlayback::Loop => elapsed.rem_euclid(native.max(0.00001)),
        };
        let frame=if let Some(emote)=&self.emote {
            f64::from(self.first_frame-emote.first_frame).max(0.0)
        } else {0.0};
        ((frame/self.source_fps.max(1.0)+local)*1000.0).round().clamp(0.0,i32::MAX as f64) as i32
    }
    pub fn preview(&self, time: f64) -> Option<Preview> {
        self.emote.clone().map(|mut emote| {
            // Studio owns cinematic trim/repeat clocks. Freeze at the last pose
            // when a rounded millisecond lands exactly on the source end.
            if super::render_studio_cinematics::StudioCinematicSource::from_emote(&emote).is_some() {
                emote.looping=false;emote.loop_start_frame=None;
            }
            Preview { emote, elapsed_ms: self.pose_time_ms(time) }
        })
    }
    pub fn motion_at(&self, absolute_time: f64, smooth: bool) -> StudioMotionKey {
        // Transform keys are independent of the pose playback mode. An actor
        // can walk while holding its last animation frame, or loop an emote
        // while following a non-looping camera/position path.
        let local=self.source_key_time(absolute_time);
        let Some(first) = self.motion.first().copied() else { return StudioMotionKey::default() };
        if local <= first.time { return first; }
        for keys in self.motion.windows(2) {
            let (a,b)=(keys[0],keys[1]);
            if local <= b.time {
                let mut t=((local-a.time)/(b.time-a.time).max(0.00001)).clamp(0.0,1.0) as f32;
                if smooth { t=t*t*(3.0-2.0*t); }
                let dyaw=(b.yaw-a.yaw+180.0).rem_euclid(360.0)-180.0;
                return StudioMotionKey {time: local,
                    position: std::array::from_fn(|i| a.position[i]+(b.position[i]-a.position[i])*t),
                    yaw: a.yaw+dyaw*t};
            }
        }
        *self.motion.last().unwrap_or(&first)
    }
    pub fn add_motion_key(&mut self, mut key: StudioMotionKey) -> usize {
        key.time=key.time.clamp(self.source_in_seconds(),self.source_out_seconds());
        if let Some(i)=self.motion.iter().position(|k|(k.time-key.time).abs()<0.00001) {
            self.motion[i]=key; return i;
        }
        self.motion.push(key);
        self.motion.sort_by(|a,b|a.time.total_cmp(&b.time));
        self.motion.iter().position(|k|(k.time-key.time).abs()<0.00001).unwrap_or(0)
    }
}

/// The incoming clip owns the edit. Zero overlap is an ordinary hard cut.
/// Pixel-space transitions are first-class so future entity/depth/angle masks
/// can be added without replacing the timeline or video source evaluator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::app) enum StudioDemoTransition {
    #[default] Cut, Dissolve, FadeThroughBlack, WipeLeft, WipeRight,
    WipeUp, WipeDown, Iris,
    // Append only: the renderer's numeric effect IDs must remain stable.
    DepthNearToFar, DepthFarToNear, DepthLayers,
    LumaBrightFirst, LumaDarkFirst, Mosaic, RadialRipple,
    PlayerSilhouette, EntityCascade,
}
impl StudioDemoTransition {
    pub fn requires_depth(self) -> bool {
        matches!(self, Self::DepthNearToFar | Self::DepthFarToNear | Self::DepthLayers)
    }
    pub const OPTIONS: [(Self,&'static str);17] = [
        (Self::Cut,"Hard cut"), (Self::Dissolve,"Dissolve"),
        (Self::FadeThroughBlack,"Fade through black"),
        (Self::WipeLeft,"Wipe from left"), (Self::WipeRight,"Wipe from right"),
        (Self::WipeUp,"Wipe from top"), (Self::WipeDown,"Wipe from bottom"),
        (Self::Iris,"Iris reveal"),
        (Self::DepthNearToFar,"Depth · near to far"),
        (Self::DepthFarToNear,"Depth · far to near"),
        (Self::DepthLayers,"Depth · stepped layers"),
        (Self::LumaBrightFirst,"Brightness · bright first"),
        (Self::LumaDarkFirst,"Brightness · dark first"),
        (Self::Mosaic,"Mosaic · randomized tiles"),
        (Self::RadialRipple,"Radial ripple"),
        (Self::PlayerSilhouette,"Geometry · player silhouette first"),
        (Self::EntityCascade,"Geometry · players, entities, world"),
    ];
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) struct StudioDemoBlend {
    pub outgoing: usize, pub incoming: usize, pub progress: f32,
    pub kind: StudioDemoTransition,
}

/// Native demo footage is not an animation. It owns a full BSP/world session
/// and uses the snapshot POV from the recording, not a stage actor camera.
/// A linked audio lane belongs to the demo, rather than an imported PCM file.
/// Keyframes are indexed in original DM26 source seconds. This ensures trims,
/// movement and speed ramps never detach the sound automation from the demo.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::app) struct StudioDemoAudio {
    #[serde(default = "demo_unity")] pub gain: f32,
    #[serde(default)] pub mute: bool,
    #[serde(default)] pub solo: bool,
    #[serde(default)] pub keys: Vec<super::render_studio_audio::StudioGainKey>,
    #[serde(default)] pub fade_in: f64,
    #[serde(default)] pub fade_out: f64,
    /// Older projects retain their original linear fade behavior.
    #[serde(default = "demo_linear_fade")]
    pub fade_curve: super::render_studio_audio::StudioAudioFade,
}
fn demo_linear_fade()->super::render_studio_audio::StudioAudioFade {
    super::render_studio_audio::StudioAudioFade::Linear
}
fn demo_unity()->f32 {1.0}
impl Default for StudioDemoAudio {
    fn default()->Self {Self{gain:1.0,mute:false,solo:false,keys:Vec::new(),fade_in:0.0,fade_out:0.0,
        fade_curve:demo_linear_fade()}}
}
impl StudioDemoAudio {
    pub fn valid(&self,source_duration:f64,edited_duration:f64)->bool {
        self.gain.is_finite() && (0.0..=4.0).contains(&self.gain) &&
        self.fade_in.is_finite() && self.fade_out.is_finite() && self.fade_in>=0.0 &&
        self.fade_out>=0.0 && self.fade_in<=edited_duration && self.fade_out<=edited_duration &&
        self.keys.len()<=512 && self.keys.iter().all(|k|k.time.is_finite() &&
            (0.0..=source_duration).contains(&k.time) && k.gain.is_finite() &&
            (0.0..=4.0).contains(&k.gain)) &&
        self.keys.windows(2).all(|pair|pair[0].time<pair[1].time)
    }
    pub fn add_key(&mut self,time:f64,gain:f32) {
        if let Some(k)=self.keys.iter_mut().find(|k|(k.time-time).abs()<0.00001) {
            k.gain=gain.clamp(0.0,4.0);return;
        }
        if self.keys.is_empty() && time>0.00001 {
            self.keys.push(super::render_studio_audio::StudioGainKey{time:0.0,gain:1.0});
        }
        self.keys.push(super::render_studio_audio::StudioGainKey{time,gain:gain.clamp(0.0,4.0)});
        self.keys.sort_by(|a,b|a.time.total_cmp(&b.time));
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::app) struct StudioDemoClip {
    /// A retained ghost-group presentation. Its explicit clock supports holds,
    /// reverse actor playback and section alignment without reversing the BSP decoder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparison: Option<Box<super::render_studio_race::RaceComparison>>,
    /// Name relative to demos/ without .dm_26.
    pub name: String,
    /// Game directory name (empty string is base).
    pub game: String,
    /// Project-managed recording, relative to render-studio/. Legacy clips use the JKA VFS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    pub start: f64,
    /// Validated duration of the original recording, milliseconds.
    pub duration_ms: i32,
    /// In/out are source-relative and the out marker is exclusive.
    pub in_ms: i32,
    pub out_ms: i32,
    #[serde(default)] pub speed_keys: Vec<StudioSpeedKey>,
    #[serde(default)] pub transition_in: StudioDemoTransition,
    #[serde(default)] pub audio: StudioDemoAudio,
    /// Parallel demo layer (0 = top). Overlap/transition rules apply within
    /// a layer; where layers overlap, the top visible layer is shown.
    #[serde(default)] pub layer: usize,
    /// Race actors composited only while this particular demo clip plays.
    /// Clip ownership survives timeline moves, trims, splits, and copies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub race_overlays: Vec<StudioRaceOverlay>,
    /// Recorded (authoritative) POV trail, scoped to this timeline clip.
    #[serde(default)] pub pov_trail: StudioPovTrail,
    /// Absent flag inherits the selected Studio HUD profile. No gameplay cvars.
    #[serde(default, skip_serializing_if="std::collections::BTreeMap::is_empty")]
    pub hud_overrides: std::collections::BTreeMap<String,bool>,
    /// Show only the recorded player (and the camera's subject / aim target):
    /// every other client in the recording is left out of this clip.
    #[serde(default, skip_serializing_if="std::ops::Not::not")]
    pub hide_other_players: bool,
}
impl StudioDemoClip {
    pub fn duration(&self) -> f64 {
        if let Some(race) = &self.comparison { return race.duration(); }
        studio_speed_span(&self.speed_keys,f64::from(self.in_ms)/1000.0,f64::from(self.out_ms)/1000.0).max(0.001)
    }
    pub fn end(&self) -> f64 { self.start+self.duration() }
    pub fn source_time_ms(&self, time: f64) -> i32 { self.source_time_ms_within(time,self.duration()) }
    /// source_time_ms with `duration()` already known (per-sample loops).
    fn source_time_ms_within(&self, time: f64, duration: f64) -> i32 {
        if let Some(race) = &self.comparison { return race.sample(time - self.start).world_ms; }
        (studio_speed_source_at(&self.speed_keys,f64::from(self.in_ms)/1000.0,
            f64::from(self.out_ms)/1000.0,(time-self.start).clamp(0.0,duration))*1000.0)
            .round().clamp(f64::from(self.in_ms),f64::from(self.out_ms)) as i32
    }
    pub fn timeline_at_source_ms(&self,source_ms:i32)->f64 {
        if let Some(race) = &self.comparison {
            return self.start + race.phases.iter().find(|p| p.world_to >= source_ms && p.world_to > p.world_from)
                .map_or(race.duration(), |p| p.start + p.duration * f64::from(source_ms - p.world_from).max(0.0) / f64::from(p.world_to - p.world_from));
        }
        self.start+studio_speed_span(&self.speed_keys,f64::from(self.in_ms)/1000.0,f64::from(source_ms)/1000.0)
    }
    pub fn audio_gain_at(&self,time:f64)->f32 { self.audio_gain_within(time,self.duration()) }
    /// audio_gain_at with `duration()` hoisted out of per-sample loops.
    pub fn audio_gain_within(&self,time:f64,duration:f64)->f32 {
        let end=self.start+duration;
        if self.comparison.as_ref().is_some_and(|r| {
            let sample=r.sample(time-self.start);
            !r.audio_active(&sample)
        }) { return 0.0; }
        if self.audio.mute || time<self.start || time>=end {return 0.0;}
        let source=f64::from(self.source_time_ms_within(time,duration))/1000.0;
        let keys=&self.audio.keys;
        let envelope=match keys.first() {
            None=>1.0,
            Some(first) if source<=first.time=>first.gain,
            Some(_)=>{
                let mut value=keys.last().map_or(1.0,|k|k.gain);
                for pair in keys.windows(2) {
                    if source<=pair[1].time {
                        let t=((source-pair[0].time)/(pair[1].time-pair[0].time)) as f32;
                        value=pair[0].gain+(pair[1].gain-pair[0].gain)*t;break;
                    }
                }
                value
            }
        };
        let elapsed=time-self.start;
        let fade_in=if self.audio.fade_in>0.0 {(elapsed/self.audio.fade_in).clamp(0.0,1.0)} else {1.0};
        let fade_out=if self.audio.fade_out>0.0 {((end-time)/self.audio.fade_out).clamp(0.0,1.0)} else {1.0};
        let fade=fade_in.min(fade_out) as f32;
        let shaped=match self.audio.fade_curve {
            super::render_studio_audio::StudioAudioFade::Linear=>fade,
            super::render_studio_audio::StudioAudioFade::EqualPower=>
                (fade*std::f32::consts::FRAC_PI_2).sin(),
        };
        self.audio.gain*envelope*shaped
    }
    pub fn valid(&self) -> bool {
        self.comparison.as_ref().is_none_or(|race| race.valid(self.duration_ms)) &&
        !self.name.is_empty() && self.name.len()<=240 &&
        !self.name.contains('\\') && !self.name.contains(':') &&
        self.name.split('/').all(|part|!part.is_empty() && part!="." && part!="..") &&
        (self.game.is_empty() || (self.game.len()<=100 && self.game!="." && self.game!=".." &&
            !self.game.contains('/') && !self.game.contains('\\') && !self.game.contains(':') &&
            !self.game.chars().any(char::is_control))) &&
        self.source_path.as_deref().is_none_or(|p|super::render_studio_media::media_path(std::path::Path::new("."),p).is_ok()) &&
        self.start.is_finite() && (0.0..=36000.0).contains(&self.start) &&
        self.duration_ms>0 && self.duration_ms<=36_000_000 &&
        self.in_ms>=0 && self.in_ms<self.out_ms && self.out_ms<=self.duration_ms &&
        studio_speed_valid(&self.speed_keys,f64::from(self.duration_ms)/1000.0) &&
        self.audio.valid(f64::from(self.duration_ms)/1000.0,self.duration()) && self.end()<=36000.0 &&
        self.hud_overrides.len()<=crate::app::render_studio_cfg::HUD_ELEMENTS.len() &&
        self.hud_overrides.keys().all(|key|crate::app::render_studio_cfg::studio_hud_key(key)) &&
        self.race_overlays.len()<=32 && self.race_overlays.iter().all(StudioRaceOverlay::valid) &&
        self.race_overlays.iter().enumerate().all(|(i,a)| self.race_overlays[..i].iter().all(|b|
            a.name != b.name || a.game != b.game))
    }
}

/// A camera key selects its video source as well as its pose. Binding is
/// held between keys (camera cuts do not crossfade unrelated world sessions).
/// Old project files default to the original animation-stage camera.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::app) enum StudioCameraSource {
    #[default] Animation,
    Demo { pov: StudioDemoPov, client: i32 },
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::app) enum StudioDemoPov { #[default] Recorded, Follow, Free }

/// Cinematic rig on a demo camera key, independent of the spectator's global
/// first/third person preferences. The playback path uses OpenJK's existing
/// collision-aware third person camera with zero temporal damping for accurate
/// seek/export evaluation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(in crate::app) enum StudioFollowPerspective { #[default] FirstPerson, ThirdPerson, Orbit }

/// Body points are taken from the already-evaluated Ghoul2 player pose during
/// demo presentation, not from a guessed eye-height skeleton. Eyes and origin
/// are available even if the character's model has not finished loading.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::app) enum StudioDemoAnchor {
    #[default] Eyes, Origin, HeadTop, Face, Chest, Pelvis, RightHand, LeftHand,
    /// Tip of the first blade of the primary saber (falls back to the eyes).
    SaberTip,
    /// Base of that blade (the hilt), held or thrown.
    SaberHilt,
}
impl StudioDemoAnchor {
    pub fn bone_name(self)->Option<&'static str> {
        match self {
            Self::Eyes|Self::Origin=>None,
            Self::HeadTop=>Some("*head_top"), Self::Face=>Some("*head_eyes"),
            Self::Chest=>Some("*chestg"), Self::Pelvis=>Some("pelvis"),
            Self::RightHand=>Some("*r_hand"), Self::LeftHand=>Some("*l_hand"),
            Self::SaberTip=>Some(crate::cgame::player_presenter::SABER_TIP_ANCHOR),
            Self::SaberHilt=>Some(crate::cgame::player_presenter::SABER_HILT_ANCHOR),
        }
    }
    pub fn label(self)->&'static str {
        match self {
            Self::Eyes=>"Eyes / viewheight", Self::Origin=>"Player origin",
            Self::HeadTop=>"Head top (*head_top)", Self::Face=>"Face (*head_eyes)",
            Self::Chest=>"Chest (*chestg)", Self::Pelvis=>"Pelvis",
            Self::RightHand=>"Right hand", Self::LeftHand=>"Left hand",
            Self::SaberTip=>"Saber tip", Self::SaberHilt=>"Saber hilt",
        }
    }
    pub const ALL:[Self;10]=[Self::Eyes,Self::Origin,Self::HeadTop,Self::Face,
        Self::Chest,Self::Pelvis,Self::RightHand,Self::LeftHand,Self::SaberTip,Self::SaberHilt];
}


/// When demo PVS cannot supply a tracked player, using a keyed camera is
/// repeatable during scrubbing/export. A "last seen" pose would depend on
/// playback history and is deliberately not offered here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::app) enum StudioFollowFallback { #[default] RecordedPov, KeyedCamera }

/// Per-move camera trajectory, offset perpendicular to the straight segment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::app) enum StudioFollowMovePath { #[default] Direct, ArcLeft, ArcRight, Lift }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::app) enum StudioFollowMoveAim { #[default] BlendAngles, LookAtSubjects }

/// Reusable, time-independent camera move evaluation. Y is up in renderer space.
/// `alpha` is already eased by the NLE's global interpolation setting.
pub(in crate::app) fn studio_follow_move_position(
    from:glam::Vec3,to:glam::Vec3,alpha:f32,path:StudioFollowMovePath,amount:f32,
)->glam::Vec3 {
    let t=alpha.clamp(0.0,1.0);
    if t<=0.0 {return from;}
    if t>=1.0 {return to;}
    let base=from.lerp(to,t);
    if path==StudioFollowMovePath::Direct || amount<=0.0 {return base;}
    let bump=(std::f32::consts::PI*t).sin()*amount;
    let direction=to-from;
    match path {
        StudioFollowMovePath::ArcLeft|StudioFollowMovePath::ArcRight=>{
            let flat=glam::Vec3::new(direction.x,0.0,direction.z).normalize_or_zero();
            // For perfectly stacked camera positions, use a stable horizontal axis.
            let flat=if flat.length_squared()<0.001 {glam::Vec3::X}else{flat};
            let left=glam::Vec3::Y.cross(flat);
            let sign=if path==StudioFollowMovePath::ArcLeft {1.0}else{-1.0};
            base+left*bump*sign
        }
        StudioFollowMovePath::Lift=>base+glam::Vec3::Y*bump,
        StudioFollowMovePath::Direct=>base,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(in crate::app) struct StudioDemoFollowRig {
    pub perspective: StudioFollowPerspective,
    pub range: f32,
    pub yaw_offset: f32,
    pub pitch_offset: f32,
    pub vertical_offset: f32,
    pub horizontal_offset: f32,
    /// TaystJK thirdPersonAngle=-1: orient chase camera from actor movement.
    pub motion_direction: bool,
    /// Demo camera position and aiming can use independent anatomical anchors.
    /// Actor-local offsets use the entity's world facing, not bone-local axes.
    pub position_anchor: StudioDemoAnchor,
    pub position_offset: [f32;3],
    pub position_local: bool,
    pub aim_enabled: bool,
    /// -1 = the followed player, otherwise an explicit demo client number.
    pub aim_client: i32,
    pub aim_anchor: StudioDemoAnchor,
    pub aim_offset: [f32;3],
    pub aim_local: bool,
    /// Orbit uses yaw/pitch offsets either relative to player facing or in world space.
    pub orbit_world_angles: bool,
    pub orbit_collision: bool,
    /// Camera-position interpolation from the preceding follow shot, in seconds.
    /// This is not a video crossfade or a blend of independently decoded demos.
    pub blend_seconds: f32,
    /// What to show when the tracked actor cannot be reconstructed from the demo.
    pub fallback: StudioFollowFallback,
    /// Movement between two shots is authored on the incoming follow key.
    pub move_path: StudioFollowMovePath,
    pub move_arc: f32,
    pub move_aim: StudioFollowMoveAim,
    /// Clamp the shot along BSP between the interpolated eyes and camera.
    pub avoid_walls: bool,
    /// Pull focus to the followed (or aimed-at) subject every frame.
    pub autofocus: bool,
    /// First person only: put the camera on the Look-at player's blade line,
    /// extended from the blade to the followed player's eyes (victim blade view).
    pub blade_view: bool,
    /// Orbit: put the camera behind the anchor on the line from the Look-at
    /// target (raised by Height), so the anchor stays in the foreground.
    pub chase_aim: bool,
}
impl Default for StudioDemoFollowRig {
    fn default()->Self {Self {perspective:StudioFollowPerspective::FirstPerson,
        range:100.0,yaw_offset:0.0,pitch_offset:0.0,
        vertical_offset:16.0,horizontal_offset:0.0,motion_direction:false,
        position_anchor:StudioDemoAnchor::Eyes,position_offset:[0.;3],position_local:false,
        aim_enabled:false,aim_client:-1,aim_anchor:StudioDemoAnchor::Eyes,
        aim_offset:[0.;3],aim_local:false,orbit_world_angles:false,orbit_collision:true,
        blend_seconds:0.0,
        fallback:StudioFollowFallback::RecordedPov,move_path:StudioFollowMovePath::Direct,
        move_arc:0.0,move_aim:StudioFollowMoveAim::BlendAngles,avoid_walls:false,autofocus:false,blade_view:false,chase_aim:false}}
}
impl StudioDemoFollowRig {
    pub fn valid(&self)->bool {
        (0.0..=2048.0).contains(&self.range) &&
        (-360.0..=360.0).contains(&self.yaw_offset) &&
        (-89.0..=89.0).contains(&self.pitch_offset) &&
        (-512.0..=512.0).contains(&self.vertical_offset) &&
        (-512.0..=512.0).contains(&self.horizontal_offset) &&
        (0.0..=20.0).contains(&self.blend_seconds) &&
        (0.0..=1024.0).contains(&self.move_arc) &&
        (-1..=1023).contains(&self.aim_client) &&
        self.position_offset.iter().chain(self.aim_offset.iter()).all(|v|v.is_finite() && v.abs()<=4096.0)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(in crate::app) struct StudioCameraKey {
    #[serde(default)] pub lens: crate::renderer::StudioLens,
    /// Renderer-space aim point. Also drives physical focus after rig resolution.
    #[serde(default,skip_serializing_if="Option::is_none")] pub focus_target: Option<[f32;3]>,
    pub time: f64, pub position: [f32;3], pub yaw: f32, pub pitch: f32, pub fov: f32,
    #[serde(default)] pub source: StudioCameraSource,
    #[serde(default)] pub demo_follow: StudioDemoFollowRig,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub rig_orbit: Option<StudioCameraOrbit>,
}
impl Default for StudioCameraKey {
    fn default() -> Self { Self { focus_target:None,lens:Default::default(),time: 0.0, position: [0.0,-11.0,0.0], yaw: 0.0, pitch: 5.0, fov: 58.0,source: StudioCameraSource::default(),demo_follow:StudioDemoFollowRig::default(),rig_orbit:None } }
}
impl StudioCameraKey {
    pub fn focus_point(self)->[f32;3] {
        self.focus_target.unwrap_or_else(|| {
            let camera=super::render_studio_spatial::camera_from_key(self);
            (camera.position+camera.forward()*self.lens.focus_distance).to_array()
        })
    }
    pub fn focus_target_valid(self)->bool {
        self.focus_target.is_none_or(|p|p.iter().all(|v|v.is_finite()&&v.abs()<=1_000_000.0))
    }
    pub fn lens_for_camera(self,camera:&mut crate::camera::Camera)->crate::renderer::StudioLens {
        let mut lens=self.lens;
        if let Some(target)=self.focus_target {
            let delta=glam::Vec3::from_array(target)-camera.position;
            if delta.is_finite()&&delta.length_squared()>1e-8 {
                camera.yaw=(-delta.z).atan2(delta.x);
                camera.pitch=delta.y.atan2(delta.x.hypot(delta.z)).clamp(-89.9_f32.to_radians(),89.9_f32.to_radians());
                lens.focus_distance=delta.length().clamp(1.0,65536.0);
            }
        }
        lens
    }
    pub fn resolve_focus_target(&mut self) {
        if self.focus_target.is_none(){return;}
        let mut camera=crate::camera::Camera::new_with_fov(self.position,self.yaw.to_radians(),self.fov);
        camera.pitch=self.pitch.to_radians();
        self.lens=self.lens_for_camera(&mut camera);
        self.yaw=camera.yaw.to_degrees();self.pitch=camera.pitch.to_degrees();
    }
    /// Rotate the current camera around the actor without resetting its framing.
    pub fn orbit(&mut self, target: [f32;3], delta: [f32;2]) {
        if delta==[0.0,0.0] {return;}
        let target=glam::Vec3::from_array(target);
        let offset=glam::Vec3::from_array(self.position)-target;
        let yaw_delta=delta[0]*0.25;
        let pitch=(self.pitch-delta[1]*0.25).clamp(-89.0,89.0);
        let pitch_delta=pitch-self.pitch;
        self.yaw=(self.yaw+yaw_delta).rem_euclid(360.0);
        let yaw=self.yaw.to_radians();
        let right=glam::Vec3::new(yaw.sin(),0.0,yaw.cos());
        let rotation=glam::Quat::from_axis_angle(right,pitch_delta.to_radians())
            *glam::Quat::from_rotation_y(yaw_delta.to_radians());
        self.position=(target+rotation*offset).to_array();
        self.pitch=pitch;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(in crate::app) enum StudioInterpolation { Linear, #[default] Smooth, Spline }

#[derive(Serialize, Deserialize)]
pub(in crate::app) struct StudioProjectClip {
    pub name: String, pub emote_id: Option<String>, pub start: f64,
    #[serde(default)] pub cinematic: Option<super::render_studio_cinematics::StudioCinematicSource>,
    pub first_frame: i32, pub end_frame: i32, pub source_fps: f64,
    #[serde(default)] pub lane: usize,
    #[serde(default)] pub source_first: Option<i32>,
    #[serde(default)] pub source_end: Option<i32>,
    #[serde(default)] pub motion: Vec<StudioMotionKey>,
    #[serde(default)] pub tail_frames: i32,
    #[serde(default)] pub playback: StudioClipPlayback,
    #[serde(default)] pub speed_keys: Vec<StudioSpeedKey>,
    #[serde(default)] pub blend: StudioClipBlend,
}
/// Per-clip presentation settings for the source recording's own race route.
/// Kept separate from composite actors and global gameplay strafe-trail cvars.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub(in crate::app) struct StudioPovTrail {
    #[serde(default)] pub enabled: bool,
    #[serde(default)] pub trail_ticks: bool,
    #[serde(default)] pub trail_tick_labels: crate::strafe_trail::StudioTickLabels,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub trail_color: Option<[u8; 3]>,
}
impl StudioPovTrail {
    pub fn auto_trail_color(&self, clip: &StudioDemoClip) -> [u8; 3] {
        studio_name_trail_color(&clip.name, &clip.game)
    }
    pub fn resolved_trail_color(&self, clip: &StudioDemoClip) -> [u8; 3] {
        self.trail_color.unwrap_or_else(|| self.auto_trail_color(clip))
    }
}

pub(in crate::app) fn studio_name_trail_color(name: &str, game: &str) -> [u8; 3] {
    // The exact same deterministic name/game hue as existing composite trails.
    let mut hash = 0x811c9dc5u32;
    for byte in game.bytes().chain(std::iter::once(b'/')).chain(name.bytes()) {
        hash = (hash ^ u32::from(byte)).wrapping_mul(0x01000193);
    }
    let hue = (hash % 3600) as f32 / 600.0;
    let x = 1.0 - ((hue % 2.0) - 1.0).abs();
    let (r,g,b) = match hue as u8 {0=>(1.0,x,0.0),1=>(x,1.0,0.0),
        2=>(0.0,1.0,x),3=>(0.0,x,1.0),4=>(x,0.0,1.0),_=>(1.0,0.0,x)};
    let channel = |v: f32| ((0.18 + v * 0.82) * 255.0).round() as u8;
    [channel(r),channel(g),channel(b)]
}

/// A secondary recording contributes only its race player to the authoritative
/// Studio world. Saved by name/game, never by a machine-specific absolute path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(in crate::app) struct StudioRaceOverlay {
    /// Project-relative copy of this composite recording.
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub source_path: Option<String>,
    pub name: String,
    #[serde(default)] pub game: String,
    #[serde(default = "studio_race_default_alpha")] pub alpha: f32,
    #[serde(default = "studio_race_default_enabled")] pub enabled: bool,
    /// A trail belongs to this assignment, not the globally configured race ghost.
    #[serde(default)] pub strafe_trail: bool,
    /// True = route clock ticks every full second (actual demo server times).
    #[serde(default)] pub trail_ticks: bool,
    /// Off / seconds / m:ss labels at each one-second notch.
    #[serde(default)] pub trail_tick_labels: crate::strafe_trail::StudioTickLabels,
    /// None uses the stable name/game hue; Some is a user-selected RGB tint.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub trail_color: Option<[u8; 3]>,
}
fn studio_race_default_alpha() -> f32 { 0.65 }
fn studio_race_default_enabled() -> bool { true }
impl StudioRaceOverlay {
    /// Stable FNV-1a identity and high-saturation hue, not the session-randomized
    /// DefaultHasher or the reused 32-slot legacy palette. Equal names stay equal
    /// across projects, exports and machines.
    pub fn auto_trail_color(&self) -> [u8; 3] {
        studio_name_trail_color(&self.name, &self.game)
    }
    pub fn resolved_trail_color(&self) -> [u8; 3] {
        self.trail_color.unwrap_or_else(|| self.auto_trail_color())
    }
    pub fn valid(&self) -> bool {
        self.source_path.as_deref().is_none_or(|p|super::render_studio_media::media_path(std::path::Path::new("."),p).is_ok()) &&
        !self.name.is_empty() && self.name.len() <= 240 &&
        !self.game.contains('/') && !self.game.contains('\\') &&
        !self.game.contains("..") &&
        !self.name.contains("..") &&
        self.name.split('/').all(|part| !part.is_empty() && part != ".") &&
        !self.name.contains('\\') &&
        self.alpha.is_finite() && (0.02..=1.0).contains(&self.alpha)
    }
}
/// A character track's saved saber loadout. Missing overrides use the profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::app) struct StudioSabers {
    pub enabled: bool,
    pub names: [String; 2],
    /// Resolved multiplayer colors, including packed custom RGB colors.
    pub colors: [i32; 2],
}
impl StudioSabers {
    pub fn apply(&self, info: &mut crate::cgame::ClientInfo) {
        info.saber_name = self.names[0].clone();
        info.saber2_name = self.names[1].clone();
        info.saber_color = self.colors[0];
        info.saber2_color = self.colors[1];
        info.definition_saber_colors = false;
    }
}
#[derive(Serialize, Deserialize)]
pub(in crate::app) struct StudioProject {
    pub version: u32, pub model: String, pub with_sabers: bool,
    #[serde(default)] pub demos: Vec<StudioDemoClip>,
    // Legacy projects used a project-wide list. On load we migrate it to each demo clip.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub race_overlays: Vec<StudioRaceOverlay>,
    #[serde(default)] pub director_shots: Vec<StudioDirectorShot>,
    #[serde(default)] pub audio_tracks: Vec<StudioAudioTrack>,
    #[serde(default)] pub light_layers: Vec<super::render_studio_lights::StudioLightLayer>,
    #[serde(default)] pub scene_lighting: super::render_studio_lights::StudioEnvironment,
    pub background: String, pub background_map: String,
    pub clips: Vec<StudioProjectClip>, pub camera: Vec<StudioCameraKey>,
    #[serde(default)] pub camera_rig: StudioCameraRig,
    #[serde(default = "default_lanes")] pub lanes: usize,
    #[serde(default)] pub in_point: f64,
    #[serde(default)] pub out_point: Option<f64>,
    #[serde(default)] pub interpolation: StudioInterpolation,
    #[serde(default)] pub lane_models: Vec<String>,
    #[serde(default)] pub lane_sabers: Vec<Option<StudioSabers>>,
    /// Per-character `char_color_*` RGB; None inherits the current Profile tint.
    #[serde(default)] pub lane_char_colors: Vec<Option<[u8; 3]>>,
    #[serde(default)] pub lane_visible: Vec<bool>,
    /// Studio graphics profile (base/render-studio/profiles/*.cfg).
    #[serde(default)] pub render_cfg: String,
    /// Demo layers hidden with their eye toggle.
    #[serde(default)] pub demo_layer_hidden: Vec<bool>,
}
fn default_lanes() -> usize { 1 }

/// Origin snapshots are used for pointer-anchored, non-incremental drags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum StudioDragKind { Clip, TrimLeft, TrimRight, Camera, Motion, In, Out }
/// Demo trims and moves belong to the timeline model, not the editor UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum StudioDemoDragKind { Move, In, Out }

#[derive(Clone, Copy, Debug)]
pub(in crate::app) struct StudioDrag {
    pub kind: StudioDragKind, pub index: usize, pub sub_index: usize,
    pub pointer_x: f32, pub initial_scroll: f32, pub initial_time: f64,
    pub initial_first: i32, pub initial_end: i32, pub initial_start: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(in crate::app) enum StudioItem { Clip(usize), Camera(usize), Motion(usize,usize) }

/// Speed automation belongs to a source clip (not to a global scene layer).
/// Selection is editor-only and does not change the serialized project format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::app) enum StudioSpeedSelection {
    Animation(usize,usize), Demo(usize,usize),
}

/// Visual track rows are calculated from expanded character/demo subtracks.
/// Expanded Speed rows mean actor IDs cannot be inferred from row arithmetic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum StudioTrackRow {
    Camera, LightLayer(usize), Light(usize,usize), Character(usize), Motion(usize), Speed(usize),
    Demo(usize), DemoSpeed(usize), DemoAudio(usize),
    Audio(usize), AudioVolume(usize),
}

/// Pointer anchor for editing a time-remap diamond. Always evaluate from the
/// initial keys to avoid feedback as the parent clip changes its own width.
#[derive(Clone, Debug)]
pub(in crate::app) struct StudioSpeedDrag {
    pub key: StudioSpeedSelection,
    pub pointer_x: f32,
    pub pointer_y: f32,
    pub scroll: f32,
    pub keys: Vec<StudioSpeedKey>,
    pub source_in: f64,
    pub source_out: f64,
    pub source_time: f64,
    pub rate: f64,
}

pub(in crate::app) struct StudioTimeline {
    pub light_layers: Vec<super::render_studio_lights::StudioLightLayer>,
    pub scene_lighting: super::render_studio_lights::StudioEnvironment,
    pub clips: Vec<StudioAnimationClip>, pub demos: Vec<StudioDemoClip>, pub camera: Vec<StudioCameraKey>,
    pub camera_rig: StudioCameraRig,
    /// Live Director shots; later entries win overlaps.
    pub director_shots: Vec<StudioDirectorShot>,
    pub audio_tracks: Vec<StudioAudioTrack>,
    pub lanes: usize,
    pub selected_clip: Option<usize>, pub selected_demo: Option<usize>, pub selected_camera: Option<usize>,
    pub selected_motion: Option<(usize,usize)>,
    pub selected_speed: Option<StudioSpeedSelection>,
    pub selection: Vec<StudioItem>,
    selection_anchor: Option<StudioItem>,
    /// Demo clips selected together with `selected_demo` (Ctrl/Shift+click).
    pub selected_demos: Vec<usize>,
    demo_anchor: Option<usize>,
    pub playhead: f64, pub playing: bool, pub play_started: Instant,
    pub zoom: f32, pub scroll: f32, pub snap: bool,
    pub in_point: f64, pub out_point: Option<f64>,
    pub interpolation: StudioInterpolation,
    /// Optional per-character model overrides. Empty means global default.
    pub lane_models: Vec<String>,
    pub lane_sabers: Vec<Option<StudioSabers>>,
    /// Optional JKA char_color RGB, in red/green/blue order, per actor track.
    pub lane_char_colors: Vec<Option<[u8; 3]>>,
    /// Per-character visibility/mute. Missing entries mean visible to preserve
    /// projects saved before the track visibility feature.
    pub lane_visible: Vec<bool>,
    /// View-only expansion state: a speed automation lane belongs to its parent.
    pub lane_speed_expanded: Vec<bool>,
    /// Character tracks shown while empty (added with "+ Character"). Other
    /// empty tracks are hidden until media is dropped onto the timeline.
    pub lane_pinned: Vec<bool>,
    /// Per demo layer: Speed subtrack expanded.
    pub demo_speed_expanded: Vec<bool>,
    /// View-only group fold: hide both linked audio and speed rows.
    pub demo_layer_collapsed: Vec<bool>,
    /// Demo layers shown while empty (added with Add track).
    pub demo_layer_pinned: Vec<bool>,
    /// Demo layers hidden from playback / export (eye toggle).
    pub demo_layer_hidden: Vec<bool>,
    pub audio_expanded: Vec<bool>,
    pub speed_drag: Option<StudioSpeedDrag>,
    pub drag: Option<StudioDrag>,
}
impl Default for StudioTimeline {
    fn default() -> Self { Self {
        clips: Vec::new(), demos: Vec::new(), camera: vec![StudioCameraKey::default()], lanes: 1,
        camera_rig: StudioCameraRig::default(), director_shots: Vec::new(), audio_tracks:Vec::new(), light_layers:Vec::new(),
        scene_lighting:Default::default(),
        selected_clip: None, selected_demo: None, selected_camera: None, selected_motion: None,
        selected_speed: None,
        selection: Vec::new(), selection_anchor: None, selected_demos: Vec::new(), demo_anchor: None,
        playhead: 0.0, playing: false, play_started: Instant::now(),
        zoom: 90.0, scroll: 0.0, snap: true, in_point: 0.0, out_point: None,
        interpolation: StudioInterpolation::Smooth, lane_models: vec![String::new()], lane_sabers: vec![None],
        lane_char_colors: vec![None], lane_visible: vec![true], lane_speed_expanded: vec![false], lane_pinned: vec![false],
        demo_speed_expanded: Vec::new(), demo_layer_collapsed: Vec::new(), demo_layer_pinned: Vec::new(), demo_layer_hidden: Vec::new(), audio_expanded:Vec::new(), speed_drag: None, drag: None,
    }}
}
impl StudioTimeline {
    fn primary_item(&self) -> Option<StudioItem> {
        self.selected_motion.map(|(i,k)|StudioItem::Motion(i,k))
            .or(self.selected_camera.map(StudioItem::Camera))
            .or(self.selected_clip.map(StudioItem::Clip))
    }
    pub fn item_time(&self,item:StudioItem) -> Option<f64> {
        match item {
            StudioItem::Clip(i)=>self.clips.get(i).map(|clip|clip.start),
            StudioItem::Camera(i)=>self.camera.get(i).map(|key|key.time),
            StudioItem::Motion(i,k)=>self.clips.get(i).and_then(|clip|clip.motion.get(k)
                .map(|key|clip.timeline_at_source(key.time))),
        }
    }
    pub fn selected_items(&self) -> Vec<StudioItem> {
        let Some(primary)=self.primary_item() else {return Vec::new()};
        if self.selection.contains(&primary) {
            self.selection.iter().copied().filter(|&item|self.item_time(item).is_some()).collect()
        } else {self.item_time(primary).map(|_|vec![primary]).unwrap_or_default()}
    }
    pub fn is_selected(&self,item:StudioItem) -> bool {
        let Some(primary)=self.primary_item() else {return false;};
        self.item_time(item).is_some() && if self.selection.contains(&primary) {self.selection.contains(&item)}else{primary==item}
    }
    fn focus_item(&mut self,item:Option<StudioItem>) {
        self.selected_clip=None;self.selected_camera=None;self.selected_motion=None;
        match item {
            Some(StudioItem::Clip(i))=>self.selected_clip=Some(i),
            Some(StudioItem::Camera(i))=>self.selected_camera=Some(i),
            Some(StudioItem::Motion(i,k))=>{self.selected_clip=Some(i);self.selected_motion=Some((i,k));}
            None=>{}
        }
    }
    pub fn clear_selection(&mut self) {
        self.selected_speed=None;
        self.selected_demo=None;
        self.selected_demos.clear();self.demo_anchor=None;
        self.selection.clear();self.selection_anchor=None;self.focus_item(None);
    }
    /// Selected demo clips: the multi-selection, else the single primary.
    pub fn selected_demo_indices(&self) -> Vec<usize> {
        let Some(primary)=self.selected_demo.filter(|&i|i<self.demos.len()) else {return Vec::new()};
        if self.selected_demos.contains(&primary) {
            self.selected_demos.iter().copied().filter(|&i|i<self.demos.len()).collect()
        } else {vec![primary]}
    }
    pub fn demo_selected(&self,i:usize) -> bool {self.selected_demo_indices().contains(&i)}
    /// Inspector edits affect the entire selection only when its primary clip
    /// is part of that selection; callers can still edit an unselected clip.
    pub fn demo_edit_targets(&self, index: usize) -> Vec<usize> {
        let selected = self.selected_demo_indices();
        if selected.contains(&index) { selected }
        else if index < self.demos.len() { vec![index] } else { Vec::new() }
    }
    pub fn set_demo_hud_override(&mut self, index: usize, key: &str, value: Option<bool>) {
        if !super::render_studio_cfg::studio_hud_key(key) { return; }
        for i in self.demo_edit_targets(index) {
            let overrides = &mut self.demos[i].hud_overrides;
            if let Some(value) = value { overrides.insert(key.to_owned(), value); }
            else { overrides.remove(key); }
        }
    }
    pub fn reset_demo_hud_overrides(&mut self, index: usize) {
        for i in self.demo_edit_targets(index) { self.demos[i].hud_overrides.clear(); }
    }
    /// Click selection of a demo clip: Ctrl toggles, Shift selects every demo
    /// clip from the anchor's start to this one's (inclusive, all layers).
    pub fn select_demo(&mut self,i:usize,ctrl:bool,shift:bool) {
        if i>=self.demos.len() {return;}
        let previous=self.selected_demo_indices();
        let anchor=self.demo_anchor.or(self.selected_demo).filter(|&a|a<self.demos.len()).unwrap_or(i);
        self.clear_selection();
        let mut selected=if ctrl {previous} else {Vec::new()};
        if shift {
            let (a,b)=(self.demos[anchor].start,self.demos[i].start);
            let mut range:Vec<usize>=(0..self.demos.len()).filter(|&j| {
                let t=self.demos[j].start;t>=a.min(b)-1e-8 && t<=a.max(b)+1e-8}).collect();
            range.sort_by(|&x,&y|self.demos[x].start.total_cmp(&self.demos[y].start));
            for j in range {if !selected.contains(&j) {selected.push(j);}}
        } else if ctrl && selected.contains(&i) {selected.retain(|&j|j!=i);}
        else {selected.push(i);}
        self.selected_demo=if selected.contains(&i) {Some(i)} else {selected.last().copied()};
        self.selected_demos=selected;
        self.demo_anchor=if shift {Some(anchor)} else {self.selected_demo};
    }
    pub fn select_all_demos(&mut self) {
        self.clear_selection();
        let mut all:Vec<usize>=(0..self.demos.len()).collect();
        all.sort_by(|&x,&y|self.demos[x].start.total_cmp(&self.demos[y].start));
        self.selected_demo=all.first().copied();
        self.demo_anchor=self.selected_demo;
        self.selected_demos=all;
    }
    /// Every animation clip and camera key (Ctrl+A on those tracks).
    pub fn select_all_items(&mut self) {
        let items=(0..self.clips.len()).map(StudioItem::Clip)
            .chain((0..self.camera.len()).map(StudioItem::Camera)).collect();
        self.set_selection(items);
    }
    /// Remove several demo clips (descending, so indices stay valid).
    pub fn remove_demos(&mut self,mut indices:Vec<usize>) -> usize {
        indices.sort_unstable();indices.dedup();
        let count=indices.iter().filter(|&&i|i<self.demos.len()).count();
        for i in indices.into_iter().rev() {self.remove_demo(i);}
        self.clear_selection();
        count
    }
    pub fn set_selection(&mut self,items:Vec<StudioItem>) {
        self.clear_selection();
        for item in items {
            if self.item_time(item).is_some() && !self.selection.contains(&item) {self.selection.push(item);}
        }
        self.selection_anchor=self.selection.first().copied();
        self.focus_item(self.selection.last().copied());
    }
    pub fn select_item(&mut self,item:StudioItem,ctrl:bool,shift:bool) {
        if self.item_time(item).is_none() {return;}
        self.selected_speed=None;
        self.selected_demo=None;
        let previous=self.selected_items();
        let anchor=if previous.is_empty() {item} else {self.selection_anchor.or(self.primary_item()).unwrap_or(item)};
        let mut selected=if ctrl {previous} else {Vec::new()};
        if shift {
            let a=self.item_time(anchor).unwrap_or_else(||self.item_time(item).unwrap());
            let b=self.item_time(item).unwrap();
            let mut candidates:Vec<_>=(0..self.clips.len()).map(StudioItem::Clip)
                .chain((0..self.camera.len()).map(StudioItem::Camera)).collect();
            for (i,clip) in self.clips.iter().enumerate() {
                for (k,_) in clip.motion.iter().enumerate() {
                    let key=StudioItem::Motion(i,k);
                    if self.item_time(key).is_some_and(|t|t>=clip.start-1e-8 && t<=clip.end()+1e-8) {candidates.push(key);}
                }
            }
            for candidate in candidates {
                if self.item_time(candidate).is_some_and(|t|t>=a.min(b)-1e-8 && t<=a.max(b)+1e-8)
                    && !selected.contains(&candidate) {selected.push(candidate);}
            }
        } else if ctrl && selected.contains(&item) {selected.retain(|&other|other!=item);}
        else {selected.push(item);}
        self.selection=selected;
        self.selection_anchor=Some(if shift {anchor}else{item});
        self.focus_item(if self.selection.contains(&item) {Some(item)}else{self.selection.last().copied()});
    }
    pub fn select_speed_key(&mut self, speed: StudioSpeedSelection) {
        let exists = match speed {
            StudioSpeedSelection::Animation(i,k) => self.clips.get(i).is_some_and(|c| k<c.speed_keys.len()),
            StudioSpeedSelection::Demo(i,k) => self.demos.get(i).is_some_and(|c| k<c.speed_keys.len()),
        };
        if !exists {return;}
        self.clear_selection();
        match speed {
            StudioSpeedSelection::Animation(i,_) => self.selected_clip=Some(i),
            StudioSpeedSelection::Demo(i,_) => self.selected_demo=Some(i),
        }
        self.selected_speed=Some(speed);
    }
    pub fn delete_selected_speed_key(&mut self) -> bool {
        let Some(selected)=self.selected_speed else {return false};
        // Removing an automation point changes output duration. Never silently
        // expand footage into another clip on the same source track.
        match selected {
            StudioSpeedSelection::Animation(i,k)=>{
                let Some(clip)=self.clips.get(i) else {self.selected_speed=None;return false};
                if k>=clip.speed_keys.len(){self.selected_speed=None;return false;}
                let saved=clip.speed_keys.clone();
                self.clips[i].speed_keys.remove(k);
                let start=self.clips[i].start;
                let end=self.clips[i].end();
                let lane=self.clips[i].lane;
                let collision=self.clips.iter().enumerate().any(|(j,c)|j!=i && c.lane==lane
                    && start<c.end()-0.00001 && end>c.start+0.00001);
                if collision || end>36000.0 {
                    self.clips[i].speed_keys=saved;return false;
                }
            }
            StudioSpeedSelection::Demo(i,k)=>{
                let Some(clip)=self.demos.get(i) else {self.selected_speed=None;return false};
                if k>=clip.speed_keys.len(){self.selected_speed=None;return false;}
                let saved=clip.speed_keys.clone();
                self.demos[i].speed_keys.remove(k);
                let start=self.demos[i].start;
                let end=self.demos[i].end();
                let layer=self.demos[i].layer;
                let collision=self.demos.iter().enumerate().any(|(j,c)|j!=i && c.layer==layer
                    && start<c.end()-0.00001 && end>c.start+0.00001);
                if collision || end>36000.0 {
                    self.demos[i].speed_keys=saved;return false;
                }
            }
        }
        self.selected_speed=None;
        true
    }
    /// Keep the time under the pointer fixed while zooming the timeline.
    pub fn zoom_at(&mut self, offset: f32, wheel: f32) {
        let offset=offset.max(0.0);
        let anchor=self.scroll+offset/self.zoom;
        self.zoom=(self.zoom*(wheel*0.003).clamp(-4.0,4.0).exp()).clamp(0.1,350.0);
        self.scroll=(anchor-offset/self.zoom).max(0.0);
    }
    pub fn duration(&self) -> f64 {
        self.clips.iter().map(StudioAnimationClip::end).chain(self.demos.iter().map(StudioDemoClip::end))
            .chain(self.audio_tracks.iter().flat_map(|t|t.clips.iter().map(|c|c.end())))
            .chain(self.director_shots.iter().map(StudioDirectorShot::end)).fold(
            self.light_layers.iter().flat_map(|l| &l.lights).map(|l| l.end()).fold(0.1,f64::max).max(
            self.camera.last().map_or(0.1, |k| k.time)),f64::max)
            .max(self.scene_lighting.keys.last().map_or(0.1,|k|k.time))
            .max(self.out_point.unwrap_or(0.1)).max(0.1)
    }
    /// Actor visibility applies to its animation and motion subtracks.
    pub fn lane_is_visible(&self,lane:usize)->bool {
        self.lane_visible.get(lane).copied().unwrap_or(true)
    }
    pub fn toggle_lane_visibility(&mut self,lane:usize) {
        if lane>=self.lanes {return;}
        self.lane_visible.resize(self.lanes,true);
        self.lane_visible[lane]=!self.lane_visible[lane];
    }
    /// Delete a character lane and its clips, compacting subsequent lane IDs.
    /// The recorded demo track is independent and is never removed.
    pub fn remove_lane(&mut self,lane:usize)->bool {
        if self.lanes<=1 || lane>=self.lanes {return false;}
        self.clips.retain(|c|c.lane!=lane);
        for clip in &mut self.clips {if clip.lane>lane {clip.lane-=1;}}
        self.lanes-=1;
        if lane<self.lane_models.len() {self.lane_models.remove(lane);}
        if lane<self.lane_sabers.len() {self.lane_sabers.remove(lane);}
        if lane<self.lane_char_colors.len() {self.lane_char_colors.remove(lane);}
        if lane<self.lane_visible.len() {self.lane_visible.remove(lane);}
        for binding in [&mut self.camera_rig.position,&mut self.camera_rig.target] {
            if binding.lane==lane {binding.enabled=false;binding.lane=0;}
            else if binding.lane>lane {binding.lane-=1;}
        }
        if lane<self.lane_speed_expanded.len() {self.lane_speed_expanded.remove(lane);}
        if lane<self.lane_pinned.len() {self.lane_pinned.remove(lane);}
        self.clear_selection();self.drag=None;self.speed_drag=None;
        true
    }
    pub fn speed_expanded(&self,lane:usize)->bool {
        self.lane_speed_expanded.get(lane).copied().unwrap_or(false)
    }
    pub fn toggle_speed_track(&mut self,lane:usize) {
        if lane>=self.lanes {return;}
        self.lane_speed_expanded.resize(self.lanes,false);
        self.lane_speed_expanded[lane]=!self.lane_speed_expanded[lane];
    }
    /// A character track has rows only while it holds clips or was added
    /// explicitly; empty tracks don't clutter the timeline.
    pub fn lane_shown(&self,lane:usize)->bool {
        lane<self.lanes && (self.lane_pinned.get(lane).copied().unwrap_or(false)
            || self.clips.iter().any(|c|c.lane==lane))
    }
    fn lane_rows(&self,lane:usize)->usize {
        if self.lane_shown(lane) {2+usize::from(self.speed_expanded(lane))} else {0}
    }
    pub fn character_row(&self,lane:usize)->usize {
        1+(0..lane.min(self.lanes)).map(|i|self.lane_rows(i)).sum::<usize>()
    }
    pub fn motion_row(&self,lane:usize)->usize {self.character_row(lane)+1}
    pub fn speed_row(&self,lane:usize)->Option<usize> {
        (self.lane_shown(lane) && self.speed_expanded(lane)).then(||self.character_row(lane)+2)
    }
    /// Demo layers in use (with clips, or added explicitly).
    pub fn demo_layer_count(&self)->usize {
        let clips=self.demos.iter().map(|c|c.layer+1).max().unwrap_or(0);
        let pinned=self.demo_layer_pinned.iter().rposition(|p|*p).map_or(0,|i|i+1);
        clips.max(pinned)
    }
    /// A demo layer has rows (Demo POV, [Speed], linked audio) only while it
    /// holds clips or was added explicitly.
    pub fn demo_layer_shown(&self,layer:usize)->bool {
        self.demos.iter().any(|c|c.layer==layer) || self.demo_layer_pinned.get(layer).copied().unwrap_or(false)
    }
    pub fn demo_layer_hidden(&self,layer:usize)->bool {self.demo_layer_hidden.get(layer).copied().unwrap_or(false)}
    pub fn demo_speed_expanded(&self,layer:usize)->bool {self.demo_speed_expanded.get(layer).copied().unwrap_or(false)}
    pub fn demo_layer_expanded(&self,layer:usize)->bool {!self.demo_layer_collapsed.get(layer).copied().unwrap_or(false)}
    pub fn toggle_demo_layer(&mut self,layer:usize) {
        self.demo_layer_collapsed.resize(self.demo_layer_collapsed.len().max(layer+1),false);
        self.demo_layer_collapsed[layer]=!self.demo_layer_collapsed[layer];
    }
    pub fn toggle_demo_speed(&mut self,layer:usize) {
        self.demo_speed_expanded.resize(self.demo_speed_expanded.len().max(layer+1),false);
        self.demo_speed_expanded[layer]=!self.demo_speed_expanded[layer];
        if self.demo_speed_expanded[layer] {
            self.demo_layer_collapsed.resize(self.demo_layer_collapsed.len().max(layer+1),false);
            self.demo_layer_collapsed[layer]=false;
        }
    }
    fn demo_layer_rows(&self,layer:usize)->usize {
        if self.demo_layer_shown(layer) {1+if self.demo_layer_expanded(layer) {1+usize::from(self.demo_speed_expanded(layer))} else {0}} else {0}
    }
    pub fn has_demo_track(&self)->bool {(0..self.demo_layer_count()).any(|l|self.demo_layer_shown(l))}
    /// Demo POV row of `layer` (a real row only while the layer is shown).
    pub fn demo_row(&self,layer:usize)->usize {
        self.character_row(self.lanes)+(0..layer).map(|l|self.demo_layer_rows(l)).sum::<usize>()
    }
    pub fn demo_speed_row(&self,layer:usize)->Option<usize> {
        (self.demo_layer_shown(layer) && self.demo_layer_expanded(layer) && self.demo_speed_expanded(layer)).then(||self.demo_row(layer)+1)
    }
    pub fn demo_audio_row(&self,layer:usize)->Option<usize> {
        (self.demo_layer_shown(layer) && self.demo_layer_expanded(layer)).then(||self.demo_row(layer)+1+usize::from(self.demo_speed_expanded(layer)))
    }
    pub fn demo_layer_for_row(&self,row:usize)->Option<usize> {
        match self.row_kind(row) {
            Some(StudioTrackRow::Demo(l)|StudioTrackRow::DemoSpeed(l)|StudioTrackRow::DemoAudio(l))=>Some(l),
            _=>None,
        }
    }
    /// Layer for a demo dropped below the tracks: first empty layer, else new.
    pub fn demo_layer_for_drop(&self)->usize {
        (0..self.demo_layer_count()).find(|&l|!self.demo_layer_shown(l)).unwrap_or(self.demo_layer_count())
    }
    pub fn audio_row(&self,track:usize)->usize {
        self.light_layer_row(self.light_layers.len())+
            (0..track.min(self.audio_tracks.len())).map(|i|1+usize::from(self.audio_expanded.get(i).copied().unwrap_or(false))).sum::<usize>()
    }
    pub fn audio_volume_row(&self,track:usize)->Option<usize> {
        self.audio_expanded.get(track).copied().unwrap_or(false).then(||self.audio_row(track)+1)
    }
    /// Rows in use. The slot just below them is a drop target that creates
    /// a new track (it is only drawn while media is dragged).
    pub fn track_count(&self)->usize {self.audio_row(self.audio_tracks.len())}
    /// Track for an animation dropped on the drop zone: the first empty
    /// (hidden) character track, else a new one. None at the 12-track cap.
    pub fn lane_for_drop(&mut self)->Option<usize> {
        if let Some(lane)=(0..self.lanes).find(|&lane|!self.lane_shown(lane)) {return Some(lane);}
        if self.lanes>=12 {return None;}
        self.lanes+=1;
        self.lane_models.resize(self.lanes,String::new());
        self.lane_sabers.resize(self.lanes,None);
        self.lane_char_colors.resize(self.lanes,None);
        self.lane_visible.resize(self.lanes,true);
        self.lane_speed_expanded.resize(self.lanes,false);
        self.lane_pinned.resize(self.lanes,false);
        Some(self.lanes-1)
    }
    pub fn row_kind(&self,row:usize)->Option<StudioTrackRow> {
        if row==0 {return Some(StudioTrackRow::Camera);}
        for lane in (0..self.lanes).filter(|&lane|self.lane_shown(lane)) {
            if row==self.character_row(lane) {return Some(StudioTrackRow::Character(lane));}
            if row==self.motion_row(lane) {return Some(StudioTrackRow::Motion(lane));}
            if self.speed_row(lane)==Some(row) {return Some(StudioTrackRow::Speed(lane));}
        }
        for layer in (0..self.demo_layer_count()).filter(|&l|self.demo_layer_shown(l)) {
            if row==self.demo_row(layer) {return Some(StudioTrackRow::Demo(layer));}
            if self.demo_speed_row(layer)==Some(row) {return Some(StudioTrackRow::DemoSpeed(layer));}
            if self.demo_audio_row(layer)==Some(row) {return Some(StudioTrackRow::DemoAudio(layer));}
        }
        for (layer, l) in self.light_layers.iter().enumerate() {
            if row==self.light_layer_row(layer) {return Some(StudioTrackRow::LightLayer(layer));}
            if l.expanded { for light in 0..l.lights.len() {
                if self.light_row(layer,light)==Some(row) {return Some(StudioTrackRow::Light(layer,light));}
            } }
        }
        for i in 0..self.audio_tracks.len() {
            if row==self.audio_row(i){return Some(StudioTrackRow::Audio(i));}
            if self.audio_volume_row(i)==Some(row){return Some(StudioTrackRow::AudioVolume(i));}
        }
        None
    }
    pub fn lane_for_row(&self,row:usize)->Option<usize> {
        match self.row_kind(row) {
            Some(StudioTrackRow::Character(i)|StudioTrackRow::Motion(i)|StudioTrackRow::Speed(i))=>Some(i),
            _=>None,
        }
    }
    /// Drag the entire speed diamond in source-time/log-speed space. Updates
    /// are transactional: invalid changes leave the old keys and width intact.
    pub fn move_speed_key(&mut self, drag:&StudioSpeedDrag, dx_seconds:f64,dy_pixels:f32)->bool {
        let k=match drag.key {
            StudioSpeedSelection::Animation(_,k)|StudioSpeedSelection::Demo(_,k)=>k,
        };
        if k>=drag.keys.len() {return false;}
        let initial_elapsed=studio_speed_span(&drag.keys,drag.source_in,drag.source_time);
        let source=studio_speed_source_at(&drag.keys,drag.source_in,drag.source_out,
            (initial_elapsed+dx_seconds).clamp(0.0,
                studio_speed_span(&drag.keys,drag.source_in,drag.source_out)));
        let lo=if k==0 {0.0}else{drag.keys[k-1].time+0.001};
        let source_limit=match drag.key {
            StudioSpeedSelection::Animation(i,_)=>self.clips.get(i).map(|c|
                f64::from(c.source_end-c.source_first)/c.source_fps.max(1.0)),
            StudioSpeedSelection::Demo(i,_)=>self.demos.get(i).map(|c|f64::from(c.duration_ms)/1000.0),
        };
        let Some(source_limit)=source_limit else {return false};
        let hi=if k+1<drag.keys.len() {drag.keys[k+1].time-0.001}else{source_limit};
        if hi<lo {return false;}
        let mut keys=drag.keys.clone();
        keys[k].time=source.clamp(lo,hi);
        keys[k].rate=(drag.rate*f64::from((-dy_pixels*(80.0_f32.ln()/22.0)).clamp(-8.0,8.0)).exp())
            .clamp(StudioSpeedKey::MIN,StudioSpeedKey::MAX);
        if !studio_speed_valid(&keys,source_limit) {return false;}
        match drag.key {
            StudioSpeedSelection::Animation(i,_)=>{
                if i>=self.clips.len(){return false;}
                let previous=std::mem::replace(&mut self.clips[i].speed_keys,keys);
                let clip=&self.clips[i];
                let collision=clip.end()>36000.0 || self.clips.iter().enumerate().any(|(j,c)|j!=i &&
                    c.lane==clip.lane && c.start<clip.end()-0.00001 && c.end()>clip.start+0.00001);
                if collision {self.clips[i].speed_keys=previous;return false;}
            }
            StudioSpeedSelection::Demo(i,_)=>{
                if i>=self.demos.len(){return false;}
                let previous=std::mem::replace(&mut self.demos[i].speed_keys,keys);
                let clip=&self.demos[i];
                let collision=clip.end()>36000.0 || self.demos.iter().enumerate().any(|(j,c)|j!=i && c.layer==clip.layer &&
                    c.start<clip.end()-0.00001 && c.end()>clip.start+0.00001);
                if collision {self.demos[i].speed_keys=previous;return false;}
            }
        }
        true
    }
    /// A cross-track move places the clip in the closest legal gap on its new lane.
    pub fn move_clip_to_lane(&mut self,index:usize,lane:usize,time:f64)->bool {
        if index>=self.clips.len() || lane>=self.lanes {return false;}
        self.clips[index].lane=lane;
        self.move_clip(index,time);
        true
    }
    pub fn export_range(&self) -> (f64,f64) {
        let end=self.out_point.unwrap_or_else(||self.duration()).max(0.001);
        let start=self.in_point.clamp(0.0,(end-0.001).max(0.0));
        (start,end)
    }
    pub fn set_in(&mut self, time: f64) {
        let (_,end)=self.export_range();
        self.in_point=self.snap_time(time).clamp(0.0,(end-0.001).max(0.0));
    }
    pub fn set_out(&mut self, time: f64) {
        self.out_point=Some(self.snap_time(time).clamp(self.in_point+0.001,36000.0));
    }
    pub fn snap_time(&self, time: f64) -> f64 {
        if self.snap { (time*30.0).round()/30.0 } else { time }
    }
    pub fn current_time(&self, export: Option<(u64,u32)>) -> f64 {
        if let Some((frame,fps))=export {
            let (start,end)=self.export_range();
            return (start+frame as f64/fps.max(1) as f64).min(end);
        }
        if self.playing { (self.playhead+self.play_started.elapsed().as_secs_f64()) % self.duration() }
        else { self.playhead.clamp(0.0,36000.0) }
    }
    pub fn seek(&mut self, seconds: f64) {
        // Paused editing can extend an existing shot or paste a new camera key
        // beyond the last media item. Playback/export retain their own bounds.
        self.playing=false; self.playhead=seconds.clamp(0.0,36000.0);
    }
    /// Clicking a new spot keeps the transport state: while playing, playback
    /// continues from there (sources that must seek hold the clock meanwhile).
    pub fn jump_to(&mut self, seconds: f64) {
        self.playhead=seconds.clamp(0.0,36000.0);
        if self.playing { self.play_started=Instant::now(); }
    }
    pub fn toggle_play(&mut self) {
        if self.playing { self.playhead=self.current_time(None); self.playing=false; }
        else { self.play_started=Instant::now(); self.playing=true; }
    }
    /// The same forward-only first-fit placement is used for both hover ghosts
    /// and actual media drops. Preview must never claim an occupied slot.
    pub fn drop_start(&self, desired: f64, duration: f64, lane: Option<usize>) -> f64 {
        self.demo_drop_start(desired,duration,lane,0)
    }
    /// `lane` None = demo layer `demo_layer`.
    pub fn demo_drop_start(&self, desired: f64, duration: f64, lane: Option<usize>, demo_layer: usize) -> f64 {
        let mut start=self.snap_time(desired).max(0.0);
        if duration <= 0.0 {return start;}
        loop {
            let next=match lane {
                Some(lane)=>self.clips.iter().filter(|c|c.lane==lane &&
                    start<c.end()-1e-8 && start+duration>c.start+1e-8)
                    .map(StudioAnimationClip::end).fold(None,|a:Option<f64>,b|Some(a.map_or(b,|v|v.max(b)))),
                None=>self.demos.iter().filter(|c|c.layer==demo_layer && start<c.end()-1e-8 && start+duration>c.start+1e-8)
                    .map(StudioDemoClip::end).fold(None,|a:Option<f64>,b|Some(a.map_or(b,|v|v.max(b)))),
            };
            if let Some(end)=next {start=end;} else {break;}
        }
        start
    }
    pub fn add_demo(&mut self, mut clip: StudioDemoClip) {
        if !clip.valid() {return;}
        // New imports use the next free hard-cut slot. A deliberate timeline
        // drag can then create an overlap/transition between demo clips.
        clip.transition_in=StudioDemoTransition::Cut;
        clip.start=self.demo_drop_start(clip.start,clip.duration(),None,clip.layer);
        self.clear_selection();
        self.selected_demo=Some(self.demos.len());
        self.demos.push(clip);
    }
    /// A deterministic foreground selection for normal preview/cuts. During
    /// exported transitions the evaluator explicitly requests both sources.
    pub fn demo_at(&self,time:f64)->Option<&StudioDemoClip> {
        self.demo_index_at(time).map(|i|&self.demos[i])
    }
    /// Top visible layer with footage at `time`.
    pub fn demo_top_layer_at(&self,time:f64)->Option<usize> {
        self.demos.iter().filter(|c|time>=c.start&&time<c.end()&&!self.demo_layer_hidden(c.layer))
            .map(|c|c.layer).min()
    }
    /// Index of the shown clip: top visible layer, then the incoming
    /// (latest-starting) clip of a transition within that layer.
    pub fn demo_index_at(&self,time:f64)->Option<usize> {
        let layer=self.demo_top_layer_at(time)?;
        self.demos.iter().enumerate().filter(|(_,c)|c.layer==layer&&time>=c.start&&time<c.end())
            .max_by(|a,b|a.1.start.total_cmp(&b.1.start)).map(|(i,_)|i)
    }
    /// Paint demo clips by their current timeline position, not their insertion
    /// order. The later (incoming) source must be painted on top of the
    /// outgoing source so its overlap/transition decoration stays visible
    /// when a clip is dragged from before another clip to after it.
    ///
    /// Return original indices, never reordered clips: selection, trim state,
    /// and egui interaction IDs must retain their identities during a drag.
    pub fn demo_draw_order(&self) -> Vec<usize> {
        let mut indices:Vec<_>=(0..self.demos.len()).collect();
        indices.sort_by(|&a,&b|self.demos[a].start.total_cmp(&self.demos[b].start)
            .then_with(||a.cmp(&b)));
        indices
    }
    /// Identify the two fully-rendered world sources at a transition instant.
    /// The blend interval is exactly the authored overlap of the clip edges.
    pub fn demo_blend_at(&self,time:f64)->Option<StudioDemoBlend> {
        let layer=self.demo_top_layer_at(time)?;
        let incoming=self.demos.iter().enumerate()
            .filter(|(_,c)|c.layer==layer&&time>=c.start&&time<c.end()&&c.transition_in!=StudioDemoTransition::Cut)
            .max_by(|a,b|a.1.start.total_cmp(&b.1.start))?;
        let (incoming_idx,b)=incoming;
        let (outgoing_idx,a)=self.demos.iter().enumerate()
            .filter(|(i,c)|*i!=incoming_idx && c.layer==layer && c.start < b.start && c.end()>b.start+0.00001)
            .max_by(|a,b|a.1.start.total_cmp(&b.1.start))?;
        let end=a.end().min(b.end());
        if time>=end || end<=b.start+0.00001 {return None;}
        Some(StudioDemoBlend {outgoing:outgoing_idx,incoming:incoming_idx,
            progress:((time-b.start)/(end-b.start)).clamp(0.0,1.0) as f32,
            kind:b.transition_in})
    }
    /// Set a transition on the incoming demo. Adjust its start to create a
    /// measured overlap; refuse triple overlaps or displacement of later clips.
    /// Non-destructive: the source in/out points are unchanged.
    pub fn set_demo_transition(&mut self,i:usize,kind:StudioDemoTransition,seconds:f64)->bool {
        let Some(incoming)=self.demos.get(i) else {return false;};
        // A successor's transition would change duration if we move this
        // clip; edit/remove the later transition first.
        let layer=incoming.layer;
        if self.demos.iter().enumerate().any(|(j,c)|j!=i && c.layer==layer
            && c.transition_in!=StudioDemoTransition::Cut
            && c.start>=incoming.start && c.start<incoming.end()) {return false;}
        let previous=self.demos.iter().enumerate()
            .filter(|(j,c)|*j!=i && c.layer==layer && c.start<incoming.start+0.00001)
            .max_by(|a,b|a.1.start.total_cmp(&b.1.start)).map(|(j,_)|j);
        let Some(previous)=previous else {return false;};
        let prev=&self.demos[previous];
        // Repeated changes must use the same previous clip even when the
        // incoming start currently overlaps its neighbor.
        let max_overlap=prev.duration().min(incoming.duration());
        if kind!=StudioDemoTransition::Cut && max_overlap<0.033 {return false;}
        let overlap=if kind==StudioDemoTransition::Cut {0.0} else {
            seconds.clamp(0.033,max_overlap)};
        let start=prev.end()-overlap;
        let end=start+incoming.duration();
        if start<0.0 || end>36000.0 || self.demos.iter().enumerate().any(|(j,c)|
            j!=i && j!=previous && c.layer==layer && start<c.end()-0.00001 && end>c.start+0.00001)
            || self.demos[previous].transition_in!=StudioDemoTransition::Cut
                && start<self.demos[previous].start+0.00001 {return false;}
        self.demos[i].start=start;
        self.demos[i].transition_in=kind;
        true
    }
    /// Reject malformed saved projects: overlap must belong to the incoming
    /// source, must be between two neighbors, and must not create a 3-way mix.
    pub fn demo_overlap_valid(demos:&[StudioDemoClip])->bool {
        let layers:std::collections::BTreeSet<usize>=demos.iter().map(|c|c.layer).collect();
        layers.into_iter().all(|layer| {
            let clips:Vec<StudioDemoClip>=demos.iter().filter(|c|c.layer==layer).cloned().collect();
            Self::demo_layer_overlap_valid(&clips)
        })
    }
    fn demo_layer_overlap_valid(demos:&[StudioDemoClip])->bool {
        let mut sorted:Vec<_>=demos.iter().collect();
        sorted.sort_by(|a,b|a.start.total_cmp(&b.start));
        for (i,c) in sorted.iter().enumerate() {
            if i==0 && c.transition_in!=StudioDemoTransition::Cut {return false;}
            if i>0 {
                let previous=sorted[i-1];
                let overlap=previous.end()-c.start;
                if overlap>0.00001 && (c.transition_in==StudioDemoTransition::Cut ||
                    c.start<=previous.start || overlap>=c.duration()-0.00001) {return false;}
                if overlap<=0.00001 && c.transition_in!=StudioDemoTransition::Cut {return false;}
                if i>1 && sorted[i-2].end()>c.start+0.00001 {return false;}
            }
        }
        true
    }
    /// Clean up neighbor bindings after deleting a source that provided an
    /// outgoing world. Never leave the next clip silently crossfading nothing.
    pub fn remove_demo(&mut self,i:usize) {
        if i>=self.demos.len(){return;}
        self.demos.remove(i);
        for j in 0..self.demos.len() {
            if self.demos[j].transition_in==StudioDemoTransition::Cut {continue;}
            let has_parent=self.demos.iter().enumerate().any(|(k,p)|
                j!=k && p.layer==self.demos[j].layer && p.start<self.demos[j].start && p.end()>self.demos[j].start+0.00001);
            if !has_parent {self.demos[j].transition_in=StudioDemoTransition::Cut;}
        }
    }
    pub fn demo_has_transition(&self,i:usize)->bool {
        self.demos.get(i).is_some_and(|clip|clip.transition_in!=StudioDemoTransition::Cut)
            || self.demos.iter().enumerate().any(|(j,c)| j!=i
                && self.demos.get(i).is_some_and(|p|p.layer==c.layer)
                && c.transition_in!=StudioDemoTransition::Cut
                && c.start>=self.demos.get(i).map_or(f64::INFINITY,|p|p.start)
                && c.start<self.demos.get(i).map_or(0.0,StudioDemoClip::end))
    }
    /// Validate a possible demo move as a single transaction. An overlapping
    /// incoming clip owns the transition, whose duration is the *actual* shared
    /// portion of the two clips. Non-overlapping neighbors revert to hard cuts.
    /// This intentionally never changes source trims or speed keys.
    fn demo_layout_after_move(&self, index:usize, start:f64)->Option<Vec<StudioDemoClip>> {
        if !start.is_finite() || start<0.0 || index>=self.demos.len() {return None;}
        let mut clips=self.demos.clone();
        clips[index].start=start;
        if clips[index].end()>36000.0 {return None;}
        let layer=clips[index].layer;
        let mut order:Vec<usize>=(0..clips.len()).filter(|&k|clips[k].layer==layer).collect();
        order.sort_by(|&a,&b|clips[a].start.total_cmp(&clips[b].start));
        for (position,&current) in order.iter().enumerate() {
            if position==0 {clips[current].transition_in=StudioDemoTransition::Cut;continue;}
            let previous=order[position-1];
            let overlap=clips[previous].end()-clips[current].start;
            if overlap>0.00001 {
                // A transition needs a real incoming/outgoing clip on each
                // side; complete coverage and same-start clips are invalid.
                if clips[current].start<=clips[previous].start+0.00001 ||
                    overlap>=clips[current].duration()-0.00001 ||
                    overlap>=clips[previous].duration()-0.00001 {return None;}
                if clips[current].transition_in==StudioDemoTransition::Cut {
                    clips[current].transition_in=StudioDemoTransition::Dissolve;
                }
            } else {
                clips[current].transition_in=StudioDemoTransition::Cut;
            }
        }
        Self::demo_overlap_valid(&clips).then_some(clips)
    }
    /// Move a recorded demo directly on the timeline. Edges magnetically snap
    /// together, and dragging farther creates/resizes an overlap transition.
    /// The overlap is measured from clip positions, never a separate duration.
    /// Invalid triple overlaps project to the nearest safe edge/gap instead.
    pub fn move_demo(&mut self,i:usize,time:f64) {
        if i>=self.demos.len() || !time.is_finite() {return;}
        let duration=self.demos[i].duration();
        let desired=self.snap_time(time).clamp(0.0,(36000.0-duration).max(0.0));
        // Eight screen pixels is enough to retain the existing butt-join
        // workflow, yet a deliberate further drag crosses into the dissolve.
        let magnet=if self.snap {8.0/f64::from(self.zoom.max(1.0))} else {0.0};
        let mut snapped=desired;
        let mut snap_distance=magnet;
        let layer=self.demos[i].layer;
        for (j,other) in self.demos.iter().enumerate() {
            if j==i || other.layer!=layer {continue;}
            for edge in [other.start-duration,other.end()] {
                let distance=(desired-edge).abs();
                if distance< snap_distance {snapped=edge;snap_distance=distance;}
            }
        }
        if let Some(layout)=self.demo_layout_after_move(i,snapped) {
            self.demos=layout;
            return;
        }
        // Prevent a dragged clip from vanishing/teleporting on collision.
        // Check the closest legal configurations at every other clip edge.
        let step=if self.snap {1.0/30.0} else {0.001};
        let mut candidates=vec![self.demos[i].start,0.0];
        for (j,other) in self.demos.iter().enumerate() {
            if j==i || other.layer!=layer {continue;}
            candidates.extend([
                other.start-duration,other.end(),
                other.start,other.end()-duration,
                other.start+step,other.start-duration+step,
                other.end()-step,other.end()-duration-step,
                other.end()-(duration.min(other.duration())-step),
                other.start+(duration.min(other.duration())-step)-duration,
            ]);
        }
        let mut best:Option<(f64,Vec<StudioDemoClip>)>=None;
        for candidate in candidates {
            let candidate=self.snap_time(candidate);
            if let Some(layout)=self.demo_layout_after_move(i,candidate) {
                let distance=(candidate-desired).abs();
                if best.as_ref().is_none_or(|(old,_)|distance<*old) {
                    best=Some((distance,layout));
                }
            }
        }
        if let Some((_,layout))=best {self.demos=layout;}
    }
    /// Move a demo onto another layer (and time). Falls back to staying on
    /// its layer when the target layer has no legal slot.
    pub fn move_demo_to_layer(&mut self,i:usize,layer:usize,time:f64) {
        if i>=self.demos.len() {return;}
        let (old_layer,old_start)=(self.demos[i].layer,self.demos[i].start);
        if layer!=old_layer {
            self.demos[i].layer=layer;
            self.demos[i].transition_in=StudioDemoTransition::Cut;
            self.move_demo(i,time);
            if Self::demo_overlap_valid(&self.demos) {return;}
            self.demos[i].layer=old_layer;
            self.demos[i].start=old_start;
        }
        self.move_demo(i,time);
    }
    /// The inspector changes the *type* of an already authored overlap, not
    /// the timeline geometry. Hard cut removes its overlap at the previous end.
    pub fn set_demo_transition_type(&mut self,i:usize,kind:StudioDemoTransition)->bool {
        let Some(clip)=self.demos.get(i) else {return false;};
        if kind==clip.transition_in {return true;}
        if kind==StudioDemoTransition::Cut {
            return self.set_demo_transition(i,StudioDemoTransition::Cut,0.0);
        }
        if !self.demos.iter().enumerate().any(|(j,c)|
            j!=i && c.layer==clip.layer && c.start<clip.start && c.end()>clip.start+0.00001) {return false;}
        self.demos[i].transition_in=kind;
        if Self::demo_overlap_valid(&self.demos) {true}
        else {self.demos[i].transition_in=StudioDemoTransition::Cut;false}
    }
    /// Keep overlap-driven transitions in sync with the actual trimmed edges.
    /// Preserve the selected effect while overlap remains; drop back to Cut
    /// when clips no longer overlap. No source may cover a whole neighbor or
    /// create a triple-overlap world.
    fn demo_layout_after_trim(&self, i:usize, kind:StudioDemoDragKind,
        initial_start:f64, initial_in:i32, initial_out:i32, seconds:f64) -> Option<Vec<StudioDemoClip>> {
        let original=self.demos.get(i)?;
        if original.comparison.is_some() {return None;}
        let mut clips=self.demos.clone();
        let keys=&original.speed_keys;
        let source_in=f64::from(initial_in)/1000.0;
        let source_out=f64::from(initial_out)/1000.0;
        match kind {
            StudioDemoDragKind::In => {
                let wanted=studio_speed_shift_source(keys,source_in,seconds,0.0,source_out);
                let new_in=((wanted*1000.0).round() as i32).clamp(0,initial_out-1);
                clips[i].in_ms=new_in;
                clips[i].start=initial_start+studio_speed_span(keys,source_in,f64::from(new_in)/1000.0);
            }
            StudioDemoDragKind::Out => {
                let max_source=f64::from(original.duration_ms)/1000.0;
                let wanted=studio_speed_shift_source(keys,source_out,seconds,source_in,max_source);
                clips[i].out_ms=((wanted*1000.0).round() as i32)
                    .clamp(initial_in+1,original.duration_ms);
            }
            StudioDemoDragKind::Move => return None,
        }
        // Audio fades are durations in OUTPUT seconds. A video trim can
        // shorten the output below the previous fade length; clamp the
        // linked envelope rather than silently refusing the video trim.
        let new_duration=clips[i].duration();
        clips[i].audio.fade_in=clips[i].audio.fade_in.min(new_duration);
        clips[i].audio.fade_out=clips[i].audio.fade_out.min(new_duration);
        if !clips[i].valid() {return None;}
        let layer=clips[i].layer;
        let mut order:Vec<_>=(0..clips.len()).filter(|&k|clips[k].layer==layer).collect();
        order.sort_by(|&a,&b|clips[a].start.total_cmp(&clips[b].start));
        for (position,&current) in order.iter().enumerate() {
            if position==0 {clips[current].transition_in=StudioDemoTransition::Cut;continue;}
            let previous=order[position-1];
            let overlap=clips[previous].end()-clips[current].start;
            if overlap>0.00001 {
                if clips[current].start<=clips[previous].start+0.00001 ||
                    overlap>=clips[previous].duration()-0.00001 ||
                    overlap>=clips[current].duration()-0.00001 {return None;}
                if clips[current].transition_in==StudioDemoTransition::Cut {
                    clips[current].transition_in=StudioDemoTransition::Dissolve;
                }
            }else{
                clips[current].transition_in=StudioDemoTransition::Cut;
            }
        }
        Self::demo_overlap_valid(&clips).then_some(clips)
    }
    /// Source trims remain anchored to the pointer-down in/out points, even
    /// while an overlapping transition grows or shrinks. Rather than forbid
    /// trimming either participant, clamp invalid triple/coverage layouts to
    /// the last valid trim boundary.
    pub fn trim_demo(&mut self, i:usize, kind:StudioDemoDragKind,
        initial_start:f64, initial_in:i32, initial_out:i32, seconds:f64) {
        if i>=self.demos.len() || !seconds.is_finite() {return;}
        let delta=if self.snap {(seconds*30.0).round()/30.0} else {seconds};
        if let Some(layout)=self.demo_layout_after_trim(i,kind,initial_start,initial_in,initial_out,delta) {
            self.demos=layout;
            return;
        }
        // Clamp at the first invalid layout (e.g. an entire neighboring clip
        // swallowed by the trim), while retaining the source-time anchor.
        if self.demo_layout_after_trim(i,kind,initial_start,initial_in,initial_out,0.0).is_none() {
            return;
        }
        let mut valid=0.0;
        let mut invalid=delta;
        for _ in 0..30 {
            let middle=(valid+invalid)*0.5;
            if self.demo_layout_after_trim(i,kind,initial_start,initial_in,initial_out,middle).is_some() {
                valid=middle;
            }else{
                invalid=middle;
            }
        }
        if let Some(layout)=self.demo_layout_after_trim(i,kind,initial_start,initial_in,initial_out,valid) {
            self.demos=layout;
        }
    }
    pub fn add_clip(&mut self, mut clip: StudioAnimationClip) {
        clip.lane=clip.lane.min(self.lanes.saturating_sub(1));
        clip.start=self.drop_start(clip.start,clip.duration(),Some(clip.lane));
        let i=self.clips.len(); self.clips.push(clip);
        self.select_item(StudioItem::Clip(i),false,false);
    }
    /// Preserve array identity while moving: egui IDs and selection do not jump.
    /// Overlapping positions are projected to the nearest available gap.
    pub fn move_clip(&mut self, index: usize, time: f64) {
        if index>=self.clips.len() {return;}
        let desired=self.snap_time(time).max(0.0);
        let duration=self.clips[index].duration();
        let lane=self.clips[index].lane;
        let mut obstacles:Vec<(f64,f64)>=self.clips.iter().enumerate()
            .filter(|(i,c)|*i!=index && c.lane==lane).map(|(_,c)|(c.start,c.end())).collect();
        obstacles.sort_by(|a,b|a.0.total_cmp(&b.0));
        let mut begin=0.0; let mut best=None::<(f64,f64)>;
        for (start,end) in obstacles.into_iter().chain(std::iter::once((f64::INFINITY,f64::INFINITY))) {
            if start-begin>=duration-1e-8 {
                let place=desired.clamp(begin,(start-duration).max(begin));
                let distance=(place-desired).abs();
                if best.is_none_or(|(_,old)|distance<old) { best=Some((place,distance)); }
            }
            begin=begin.max(end);
        }
        if let Some((position,_))=best {self.clips[index].start=position;}
    }
    pub fn trim_clip(&mut self, i:usize, kind:StudioDragKind, initial_start:f64,
        initial_first:i32, initial_end:i32, seconds:f64) {
        let Some(c)=self.clips.get(i) else {return};
        let fps=c.source_fps.max(1.0);
        let lane=c.lane;
        let original_in=f64::from(initial_first-c.source_first)/fps;
        let original_out=f64::from(initial_end-c.source_first)/fps;
        let keys=c.speed_keys.clone();
        let original_end=initial_start+studio_speed_span(&keys,original_in,original_out);
        let previous=self.clips.iter().enumerate().filter(|(j,p)|*j!=i&&p.lane==lane&&p.end()<=initial_start+1e-8)
            .map(|(_,clip)|clip.end()).fold(0.0,f64::max);
        let next=self.clips.iter().enumerate().filter(|(j,p)|*j!=i&&p.lane==lane&&p.start>=original_end-1e-8)
            .map(|(_,c)|c.start).fold(f64::INFINITY,f64::min);
        let c=&mut self.clips[i];
        match kind {
            StudioDragKind::TrimLeft => {
                let floor=0.0;
                let earliest=studio_speed_shift_source(&keys,original_in,previous-initial_start,floor,original_out);
                let target=studio_speed_shift_source(&keys,original_in,seconds,floor,original_out);
                let minimum=c.source_first+(earliest*fps).ceil() as i32;
                let requested=c.source_first+(target*fps).round() as i32;
                let frame=requested.clamp(minimum.min(c.end_frame-1),c.end_frame-1);
                c.first_frame=frame;
                c.start=initial_start+studio_speed_span(&keys,original_in,f64::from(frame-c.source_first)/fps);
            }
            StudioDragKind::TrimRight => {
                let maximum=if next.is_finite() {studio_speed_shift_source(&keys,original_in,next-initial_start,
                    original_in,f64::from(c.source_end-c.source_first+360000)/fps)}
                    else {f64::from(c.source_end-c.source_first+360000)/fps};
                let target=studio_speed_shift_source(&keys,original_out,seconds,original_in,maximum);
                let requested=(c.source_first+(target*fps).round() as i32)
                    .clamp(initial_first+1,(c.source_first+(maximum*fps).floor() as i32).max(initial_first+1));
                c.end_frame=requested.min(c.source_end).max(initial_first+1);
                c.tail_frames=(requested-c.end_frame).max(0);
            }
            _=>{}
        }
    }
    /// Split on a source frame boundary and preserve motion interpolation.
    pub fn split_clip(&mut self, i:usize, time:f64) -> bool {
        let Some(c)=self.clips.get(i).cloned() else {return false};
        if time<=c.start+1.0/c.source_fps || time>=c.start+c.source_duration()-1.0/c.source_fps {return false;}
        let frame=(c.source_first+(c.source_key_time(time)*c.source_fps).round() as i32)
            .clamp(c.first_frame+1,c.end_frame-1);
        let cut_time=c.timeline_at_source(f64::from(frame-c.source_first)/c.source_fps);
        let cut_offset=(frame-c.source_first) as f64/c.source_fps;
        let center=c.motion_at(cut_time,self.interpolation==StudioInterpolation::Smooth);
        let mut left=c.clone(); let mut right=c;
        left.end_frame=frame; left.tail_frames=0;
        right.first_frame=frame; right.start=cut_time;
        left.motion.retain(|k|k.time<cut_offset);
        left.motion.push(StudioMotionKey {time:cut_offset,..center});
        right.motion.retain(|k|k.time>cut_offset);
        right.motion.insert(0,StudioMotionKey {time:cut_offset,..center});
        self.clips[i]=left;
        let j=self.clips.len(); self.clips.push(right);
        self.select_item(StudioItem::Clip(j),false,false);
        true
    }
    pub fn duplicate_clip(&mut self,i:usize) {
        let Some(mut copy)=self.clips.get(i).cloned() else {return};
        copy.start=copy.end(); self.add_clip(copy);
    }
    pub fn active_actors(&self,time:f64) -> Vec<&StudioAnimationClip> {
        (0..self.lanes).filter(|&lane|self.lane_is_visible(lane))
            .filter_map(|lane|self.clips.iter()
                .find(|c|c.lane==lane && time>=c.start && time<c.end())).collect()
    }
    /// Clips on the same track that touch `index` (gap under 1/20 s).
    pub fn touching_clips(&self,index:usize) -> (Option<usize>,Option<usize>) {
        let Some(clip)=self.clips.get(index) else {return (None,None)};
        let near=|a:f64,b:f64|(a-b).abs()<=0.05;
        let prev=self.clips.iter().enumerate().position(|(j,c)|j!=index && c.lane==clip.lane && near(c.end(),clip.start));
        let next=self.clips.iter().enumerate().position(|(j,c)|j!=index && c.lane==clip.lane && near(c.start,clip.end()));
        (prev,next)
    }
    /// Smoothed transition partner of clip `index` at `time`, if inside a
    /// blend window. Pure in time: scrubbing and export agree.
    pub fn pose_blend_at(&self,index:usize,time:f64) -> Option<StudioPoseBlendPartner> {
        let clip=self.clips.get(index)?;
        let (prev,next)=self.touching_clips(index);
        let smooth=|u:f64|{let u=u.clamp(0.0,1.0);(u*u*(3.0-2.0*u)) as f32};
        if let Some(p)=prev {
            let other=&self.clips[p];
            let window=clip.blend.blend_in.min(clip.duration());
            if window>1e-4 && time<clip.start+window {
                // Shared boundary weight: half each when both sides smooth it.
                let edge=if other.blend.blend_out>1e-4 {0.5} else {1.0};
                return Some(StudioPoseBlendPartner{clip:p,sample_time:(other.end()-1e-4).max(other.start),
                    weight:edge*(1.0-smooth((time-clip.start)/window))});
            }
        }
        if let Some(n)=next {
            let other=&self.clips[n];
            let window=clip.blend.blend_out.min(clip.duration());
            if window>1e-4 && time>clip.end()-window {
                let edge=if other.blend.blend_in>1e-4 {0.5} else {1.0};
                return Some(StudioPoseBlendPartner{clip:n,sample_time:other.start,
                    weight:edge*smooth((time-(clip.end()-window))/window)});
            }
        }
        None
    }
    /// Motion (position / yaw) of clip `index` at `time`, blended with its
    /// transition partner like the pose.
    pub fn blended_motion_at(&self,index:usize,time:f64,smooth:bool) -> StudioMotionKey {
        let clip=&self.clips[index];
        let mut motion=clip.motion_at(time,smooth);
        if let Some(partner)=self.pose_blend_at(index,time) {
            let other=self.clips[partner.clip].motion_at(partner.sample_time,smooth);
            let w=partner.weight;
            for k in 0..3 {motion.position[k]+=(other.position[k]-motion.position[k])*w;}
            let delta=(other.yaw-motion.yaw+180.0).rem_euclid(360.0)-180.0;
            motion.yaw+=delta*w;
        }
        motion
    }
    pub fn clip_at_lane(&self,time:f64,lane:usize) -> Option<&StudioAnimationClip> {
        self.lane_is_visible(lane).then(||self.clips.iter()
            .find(|c| c.lane==lane && time>=c.start && time<c.end())).flatten()
    }
    pub fn delete_selection(&mut self) -> bool {
        let mut items=self.selected_items();
        if items.is_empty() {return false;}
        // Remove keys first, then clips, descending so indices remain valid.
        items.sort();items.dedup();
        for item in items.into_iter().rev() {
            match item {
                StudioItem::Motion(i,k)=>{if let Some(clip)=self.clips.get_mut(i) {if k<clip.motion.len(){clip.motion.remove(k);}}}
                StudioItem::Camera(i)=>{if i<self.camera.len(){self.camera.remove(i);}}
                StudioItem::Clip(i)=>{if i<self.clips.len(){self.clips.remove(i);}}
            }
        }
        self.clear_selection();self.drag=None;true
    }
    pub fn clip_at(&self,time:f64)->Option<&StudioAnimationClip> {
        // Compatibility with single-actor callers; the multi-actor renderer
        // evaluates active_actors() instead of picking a highest-priority lane.
        self.clip_at_lane(time,0)
    }
    pub fn add_camera_key(&mut self,mut key:StudioCameraKey) {
        key.time=self.snap_time(key.time.max(0.0));
        self.insert_camera_key(key);
    }
    /// Begin navigation from exactly the camera currently displayed. Selecting
    /// an earlier key must not rewind the playhead or restore its old pose.
    pub fn begin_camera_edit(&mut self) {
        let time=self.current_time(None);
        let mut key=self.camera_at(time);
        key.time=time;
        self.seek(time);
        // Navigation must not quantize an interpolated view to another time.
        self.insert_camera_key(key);
    }
    pub fn insert_camera_key(&mut self,key:StudioCameraKey) {
        if let Some(i)=self.camera.iter().position(|k|(k.time-key.time).abs()<1e-5) {
            self.camera[i]=key; self.selected_camera=Some(i);
        } else {
            self.camera.push(key);self.camera.sort_by(|a,b|a.time.total_cmp(&b.time));
            self.selected_camera=self.camera.iter().position(|k|(k.time-key.time).abs()<1e-5);
        }
        self.selected_clip=None;self.selected_motion=None;
        if let Some(i)=self.selected_camera {self.select_item(StudioItem::Camera(i),false,false);}
    }
    /// Only follow-to-follow camera cuts in the SAME demo can blend in this
    /// phase. Interpolation samples both tracked players at the same source
    /// timestamp; the incoming key defines the move duration. No hidden
    /// frame history is required, so playback, scrubbing and export agree.
    pub fn demo_follow_blend(&self,time:f64)->Option<(StudioCameraKey,StudioCameraKey,f32)> {
        // A newer key supersedes an unfinished earlier blend.
        let pair=self.camera.windows(2).rev().find(|pair| {
            let start=pair[1].time;
            let duration=f64::from(pair[1].demo_follow.blend_seconds);
            duration>0.0 && time>=start && time<start+duration
        })?;
        // A camera move must not mix player coordinates from two recordings.
        // Distinct clips can have different maps, entity IDs and timelines.
        if !self.demos.is_empty() {
            let a=self.demo_at(pair[0].time)?;
            let b=self.demo_at(pair[1].time)?;
            let active=self.demo_at(time)?;
            if !std::ptr::eq(a,b) || !std::ptr::eq(b,active) {return None;}
        }
        if !matches!(pair[0].source,StudioCameraSource::Demo {pov:StudioDemoPov::Follow,..}) ||
            !matches!(pair[1].source,StudioCameraSource::Demo {pov:StudioDemoPov::Follow,..}) {
            return None;
        }
        let mut alpha=((time-pair[1].time)/f64::from(pair[1].demo_follow.blend_seconds)) as f32;
        alpha=alpha.clamp(0.0,1.0);
        if self.interpolation!=StudioInterpolation::Linear {alpha=alpha*alpha*(3.0-2.0*alpha);}
        Some((pair[0],pair[1],alpha))
    }
    pub fn camera_at(&self,time:f64)->StudioCameraKey {
        let mut key=self.camera_transform_at(time);key.rig_orbit=self.orbit_at(time);key.resolve_focus_target();key
    }
    fn orbit_at(&self,time:f64)->Option<StudioCameraOrbit> {
        let keys:Vec<_>=self.camera.iter().filter_map(|k|k.rig_orbit.map(|o|(k.time,o))).collect();
        let &(first_time,first)=keys.first()?;
        if time<=first_time {return Some(first);}
        let angle=|a:f32,b:f32|(b-a+180.).rem_euclid(360.)-180.;
        for (i,pair) in keys.windows(2).enumerate() {
            let (ta,a)=pair[0];let (tb,b)=pair[1];if time>tb {continue;}
            let (tp,p)=if i>0 {keys[i-1]}else{pair[0]};
            let (tn,n)=keys.get(i+2).copied().unwrap_or(pair[1]);
            let u=((time-ta)/(tb-ta).max(1e-8)).clamp(0.,1.) as f32;
            let mix=|p:f32,a:f32,b:f32,n:f32| {
                if self.interpolation==StudioInterpolation::Spline {
                    let u2=u*u;let u3=u2*u;let dt=(tb-ta) as f32;
                    (2.*u3-3.*u2+1.)*a+(u3-2.*u2+u)*dt*(b-p)/(tb-tp).max(1e-6) as f32
                        +(-2.*u3+3.*u2)*b+(u3-u2)*dt*(n-a)/(tn-ta).max(1e-6) as f32
                } else {let t=if self.interpolation==StudioInterpolation::Smooth {u*u*(3.-2.*u)}else{u};a+(b-a)*t}
            };
            let by=a.yaw+angle(a.yaw,b.yaw);
            return Some(StudioCameraOrbit {
                distance:mix(p.distance,a.distance,b.distance,n.distance).max(0.),
                yaw:(mix(a.yaw-angle(p.yaw,a.yaw),a.yaw,by,by+angle(b.yaw,n.yaw))+180.).rem_euclid(360.)-180.,
                pitch:mix(p.pitch,a.pitch,b.pitch,n.pitch).clamp(-90.,90.),
            });
        }
        keys.last().map(|(_,orbit)|*orbit)
    }
    fn camera_transform_at(&self,time:f64)->StudioCameraKey {
        let Some(first)=self.camera.first().copied() else {return StudioCameraKey::default()};
        if time<=first.time {return first;}
        for (i,keys) in self.camera.windows(2).enumerate() {
            let (a,b)=(keys[0],keys[1]);
            if time<=b.time {
                // Metadata is a discrete shot cut; at the exact keyframe
                // take the NEW shot rather than an interpolated old source.
                if time>=b.time-1e-9 {return b;}
                let mut t=((time-a.time)/(b.time-a.time).max(1e-8)).clamp(0.0,1.0) as f32;
                if self.interpolation==StudioInterpolation::Smooth { t=t*t*(3.0-2.0*t); }
                let mix=|x:f32,y:f32|x+(y-x)*t;
                let dyaw=(b.yaw-a.yaw+180.0).rem_euclid(360.0)-180.0;
                // Time-aware cubic Hermite avoids the stop/start of independent
                // smoothstep segments and produces a curved camera path.
                let position=if self.interpolation==StudioInterpolation::Spline {
                    let prev=self.camera.get(i.wrapping_sub(1)).filter(|_|i>0).copied().unwrap_or(a);
                    let next=self.camera.get(i+2).copied().unwrap_or(b);
                    let dt=(b.time-a.time).max(1e-6) as f32;
                    let u=((time-a.time)/(b.time-a.time).max(1e-8)).clamp(0.0,1.0) as f32;
                    let u2=u*u;let u3=u2*u;
                    let h00=2.0*u3-3.0*u2+1.0;
                    let h10=u3-2.0*u2+u;
                    let h01=-2.0*u3+3.0*u2;
                    let h11=u3-u2;
                    std::array::from_fn(|axis| {
                        let m0=(b.position[axis]-prev.position[axis])/
                            (b.time-prev.time).max(1e-6) as f32;
                        let m1=(next.position[axis]-a.position[axis])/
                            (next.time-a.time).max(1e-6) as f32;
                        h00*a.position[axis]+h10*dt*m0+h01*b.position[axis]+h11*dt*m1
                    })
                } else {std::array::from_fn(|axis|mix(a.position[axis],b.position[axis]))};
                return StudioCameraKey {time,position,
                    focus_target:match (a.focus_target,b.focus_target) {
                        (Some(p),Some(q)) if a.source==b.source=>Some(std::array::from_fn(|i|mix(p[i],q[i]))),
                        _=>a.focus_target,
                    },
                    lens:if a.source==b.source{a.lens.interpolate(b.lens,t)}else{a.lens},
                    yaw:a.yaw+dyaw*t,pitch:mix(a.pitch,b.pitch),
                    // A scene/POV cut holds its outgoing lens. The incoming
                    // follow-key transition blends FOV after the cut instead.
                    fov:if a.source==b.source {mix(a.fov,b.fov)}else{a.fov},
                    source:a.source,demo_follow:a.demo_follow,rig_orbit:None};
            }
        }
        *self.camera.last().unwrap_or(&first)
    }
    pub fn move_camera_key(&mut self,index:usize,time:f64) {
        if index>=self.camera.len() {return;}
        let minimum=if index==0 {0.0} else {self.camera[index-1].time+1.0/30.0};
        let maximum=self.camera.get(index+1).map_or(36000.0,|k|k.time-1.0/30.0);
        self.camera[index].time=self.snap_time(time).clamp(minimum,maximum.max(minimum));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn clip(first:i32,end:i32,start:f64)->StudioAnimationClip {
        StudioAnimationClip{name:"BOTH_STAND1".into(),emote:None,start,first_frame:first,end_frame:end,
            source_fps:20.0,lane:0,source_first:0,source_end:100,motion:vec![StudioMotionKey::default()],tail_frames:0,playback:StudioClipPlayback::Hold,speed_keys:vec![], blend:Default::default()}
    }
    #[test] fn muted_lane_does_not_render_and_can_be_restored() {
        let mut t=StudioTimeline::default();
        t.add_clip(clip(0,20,0.0));
        assert_eq!(t.active_actors(0.5).len(),1);
        t.toggle_lane_visibility(0);
        assert_eq!(t.active_actors(0.5).len(),0);
        assert!(t.clip_at_lane(0.5,0).is_none());
        t.toggle_lane_visibility(0);
        assert_eq!(t.active_actors(0.5).len(),1);
        t.lane_visible.clear(); // legacy project without visibility flags
        assert_eq!(t.active_actors(0.5).len(),1);
    }
    #[test] fn removing_lane_reindexes_remaining_actors() {
        let mut t=StudioTimeline::default();
        t.lanes=3;
        t.lane_models=vec!["a".into(),"b".into(),"c".into()];
        let sabers=StudioSabers {enabled:true,names:["single_1".into(),"single_2".into()],colors:[0,4]};
        t.lane_sabers=vec![None,None,Some(sabers.clone())];
        t.lane_char_colors=vec![None,Some([20,40,60]),Some([10,100,250])];
        t.lane_visible=vec![true,false,true];
        for lane in 0..3 {let mut c=clip(0,20,0.0);c.lane=lane;t.add_clip(c);}
        assert!(t.remove_lane(1));
        assert_eq!(t.lanes,2);
        assert_eq!(t.clips.len(),2);
        assert_eq!(t.clips[1].lane,1);
        assert_eq!(t.lane_models,vec!["a","c"]);
        assert_eq!(t.lane_sabers,vec![None,Some(sabers)]);
        assert_eq!(t.lane_char_colors,vec![None,Some([10,100,250])]);
        assert_eq!(t.lane_visible,vec![true,true]);
        assert!(t.remove_lane(1));
        assert!(!t.remove_lane(0));
    }
    #[test] fn ctrl_toggle_and_shift_select_by_time_across_tracks() {
        let mut timeline=StudioTimeline::default();
        timeline.clips=vec![clip(0,20,4.0),clip(0,20,1.0)];
        timeline.clips[1].motion[0].time=0.5;
        timeline.camera=vec![StudioCameraKey::default(),StudioCameraKey {time:2.0,..Default::default()},StudioCameraKey {time:5.0,..Default::default()}];
        timeline.select_item(StudioItem::Clip(1),false,false);
        timeline.select_item(StudioItem::Camera(1),true,false);
        assert_eq!(timeline.selected_items().len(),2);
        timeline.select_item(StudioItem::Camera(1),true,false);
        assert_eq!(timeline.selected_items(),vec![StudioItem::Clip(1)]);
        timeline.select_item(StudioItem::Clip(1),false,false);
        timeline.select_item(StudioItem::Clip(0),false,true);
        assert_eq!(timeline.selected_items().len(),5);
        assert!(timeline.is_selected(StudioItem::Motion(1,0)));
        assert!(timeline.is_selected(StudioItem::Camera(1)));
        assert!(!timeline.is_selected(StudioItem::Camera(0)));
        assert!(!timeline.is_selected(StudioItem::Camera(2)));
        timeline.select_item(StudioItem::Camera(0),false,true);
        assert_eq!(timeline.selected_items().len(),2); // Original range anchor stays at 1s.
        timeline.select_item(StudioItem::Clip(0),false,false);
        timeline.select_item(StudioItem::Clip(0),true,false);
        assert!(timeline.selected_items().is_empty());
    }
    #[test] fn group_delete_keeps_unselected_items_and_indices_intact() {
        let mut timeline=StudioTimeline::default();
        timeline.clips=vec![clip(0,20,0.0),clip(0,20,2.0),clip(0,20,4.0)];
        timeline.clips[1].motion=vec![StudioMotionKey::default(),StudioMotionKey {time:0.25,..Default::default()},StudioMotionKey {time:0.5,..Default::default()}];
        timeline.camera=(0..3).map(|i|StudioCameraKey {time:i as f64,..Default::default()}).collect();
        timeline.set_selection(vec![StudioItem::Clip(0),StudioItem::Clip(2),StudioItem::Camera(0),StudioItem::Camera(2),StudioItem::Motion(1,0),StudioItem::Motion(1,2)]);
        assert!(timeline.delete_selection());
        assert_eq!(timeline.clips.len(),1);
        assert_eq!(timeline.clips[0].start,2.0);
        assert_eq!(timeline.clips[0].motion.len(),1);
        assert_eq!(timeline.clips[0].motion[0].time,0.25);
        assert_eq!(timeline.camera.len(),1);
        assert_eq!(timeline.camera[0].time,1.0);
        assert!(timeline.selected_items().is_empty());
        assert!(!timeline.delete_selection());
    }
    #[test] fn orbit_keeps_actor_centered_at_fixed_distance() {
        let target=[110.0,24.0,16.0];
        let mut camera=StudioCameraKey::default();
        let offset=glam::Vec3::from_array(target)-glam::Vec3::from_array(camera.position);
        camera.yaw=(-offset.z).atan2(offset.x).to_degrees();
        camera.pitch=offset.y.atan2(offset.x.hypot(offset.z)).to_degrees();
        let radius=offset.length();
        let original=camera.position;
        for delta in [[120.0,60.0],[-40.0,-2000.0],[800.0,4000.0]] {
            camera.orbit(target,delta);
            let offset=glam::Vec3::from_array(target)-glam::Vec3::from_array(camera.position);
            assert!((offset.length()-radius).abs()<0.001);
            let (yaw,pitch)=(camera.yaw.to_radians(),camera.pitch.to_radians());
            let forward=glam::Vec3::new(yaw.cos()*pitch.cos(),pitch.sin(),-yaw.sin()*pitch.cos());
            assert!(forward.dot(offset.normalize())>0.9999);
            assert!(camera.pitch.abs()<=89.0);
        }
        assert_ne!(camera.position,original);
    }
    #[test] fn orbit_after_pan_preserves_current_distance_angle_and_framing() {
        fn framing(camera:&StudioCameraKey,target:[f32;3])->glam::Vec3 {
            let offset=glam::Vec3::from_array(target)-glam::Vec3::from_array(camera.position);
            let (yaw,pitch)=(camera.yaw.to_radians(),camera.pitch.to_radians());
            let forward=glam::Vec3::new(yaw.cos()*pitch.cos(),pitch.sin(),-yaw.sin()*pitch.cos());
            let right=glam::Vec3::new(yaw.sin(),0.0,yaw.cos());
            glam::Vec3::new(offset.dot(right),offset.dot(right.cross(forward)),offset.dot(forward))
        }
        let target=[88.0,18.0,-22.0];
        let mut camera=StudioCameraKey {position:[-140.0,95.0,180.0],yaw:17.0,pitch:-12.0,..Default::default()};
        let original=camera;
        camera.orbit(target,[0.0,0.0]);
        assert_eq!(camera.position,original.position);
        assert_eq!(camera.yaw,original.yaw);
        assert_eq!(camera.pitch,original.pitch);
        let before=framing(&camera,target);
        camera.orbit(target,[0.1,-0.1]);
        assert!((glam::Vec3::from_array(camera.position)-glam::Vec3::from_array(original.position)).length()<0.5);
        assert!((camera.yaw-17.025).abs()<0.0001);
        assert!((camera.pitch+11.975).abs()<0.0001);
        assert!((framing(&camera,target)-before).length()<0.001);
        // Pan/dolly farther, release, and resume orbit from that new view.
        camera.position[0]-=250.0;camera.position[1]+=80.0;camera.position[2]+=120.0;
        let before=framing(&camera,target);
        for delta in [[120.0,80.0],[-60.0,-160.0],[0.0,0.0]] {
            camera.orbit(target,delta);
            assert!((framing(&camera,target)-before).length()<0.002);
        }
    }
    #[test] fn camera_navigation_starts_at_displayed_view_without_rewinding_or_snapping() {
        let mut timeline=StudioTimeline::default();
        timeline.camera.push(StudioCameraKey {time:4.0,position:[40.0,20.0,80.0],yaw:90.0,pitch:25.0,..Default::default()});
        timeline.selected_camera=Some(0);
        timeline.seek(1.2345);
        let displayed=timeline.camera_at(timeline.playhead);
        timeline.begin_camera_edit();
        let index=timeline.selected_camera.unwrap();
        assert_eq!(timeline.playhead,1.2345);
        assert_eq!(timeline.camera[index].time,1.2345);
        assert_eq!(timeline.camera[index].position,displayed.position);
        assert_eq!(timeline.camera[index].yaw,displayed.yaw);
        assert_eq!(timeline.camera[index].pitch,displayed.pitch);
        timeline.camera[index].position=[-120.0,45.0,210.0];
        timeline.begin_camera_edit();
        assert_eq!(timeline.camera.len(),3);
        assert_eq!(timeline.camera[timeline.selected_camera.unwrap()].position,[-120.0,45.0,210.0]);
    }
    #[test] fn drop_preview_matches_actual_forward_placement() {
        let mut tl=StudioTimeline::default();tl.snap=false;
        tl.add_clip(clip(0,60,0.0)); // 3 seconds
        let destination=tl.drop_start(1.0,1.0,Some(0));
        assert!((destination-3.0).abs()<1e-6);
        tl.add_clip(clip(0,20,1.0));
        assert!((tl.clips[1].start-destination).abs()<1e-6);
    }
    #[test] fn follow_rig_defaults_and_blend_are_deterministic() {
        let legacy:StudioCameraKey=serde_json::from_str(r#"{"time":0.0,"position":[0,0,0],"yaw":0,"pitch":0,"fov":60}"#).unwrap();
        assert_eq!(legacy.demo_follow,StudioDemoFollowRig::default());
        let mut tl=StudioTimeline::default();
        tl.camera=vec![
            StudioCameraKey {source:StudioCameraSource::Demo {pov:StudioDemoPov::Follow,client:1},..Default::default()},
            StudioCameraKey {time:2.0,source:StudioCameraSource::Demo {pov:StudioDemoPov::Follow,client:2},
                demo_follow:StudioDemoFollowRig {blend_seconds:1.0,range:175.0,..Default::default()},..Default::default()},
        ];
        let (a,b,mid)=tl.demo_follow_blend(2.5).unwrap();
        assert_eq!(a.source,tl.camera[0].source);
        assert_eq!(b.source,tl.camera[1].source);
        assert!((mid-0.5).abs()<1e-6);
        assert!(tl.demo_follow_blend(1.9).is_none());
        assert!(tl.demo_follow_blend(3.0).is_none());
        let restored:Vec<StudioCameraKey>=serde_json::from_str(&serde_json::to_string(&tl.camera).unwrap()).unwrap();
        assert_eq!(restored[1].demo_follow,tl.camera[1].demo_follow);
    }
    #[test] fn cinematic_follow_paths_have_exact_endpoints_and_mirrored_arcs() {
        let a=glam::Vec3::ZERO;
        let b=glam::Vec3::new(100.0,20.0,0.0);
        for mode in [StudioFollowMovePath::Direct,StudioFollowMovePath::ArcLeft,
            StudioFollowMovePath::ArcRight,StudioFollowMovePath::Lift] {
            assert!((studio_follow_move_position(a,b,0.0,mode,90.0)-a).length()<0.00001);
            assert!((studio_follow_move_position(a,b,1.0,mode,90.0)-b).length()<0.00002);
        }
        let left=studio_follow_move_position(a,b,0.5,StudioFollowMovePath::ArcLeft,90.0);
        let right=studio_follow_move_position(a,b,0.5,StudioFollowMovePath::ArcRight,90.0);
        assert!((left.z+right.z).abs()<0.0001 && left.z.abs()>80.0);
        assert!(studio_follow_move_position(a,b,0.5,StudioFollowMovePath::Lift,30.0).y>30.0);
        let still=studio_follow_move_position(a,a,0.5,StudioFollowMovePath::ArcLeft,12.0);
        assert!(still.is_finite());
    }
    #[test] fn demo_bone_rig_is_backward_compatible_and_serialized() {
        let legacy:StudioDemoFollowRig=serde_json::from_str(r#"{"perspective":"ThirdPerson","range":120.0}"#).unwrap();
        assert_eq!(legacy.position_anchor,StudioDemoAnchor::Eyes);
        assert_eq!(legacy.aim_anchor,StudioDemoAnchor::Eyes);
        assert!(!legacy.aim_enabled);
        assert_eq!(legacy.aim_client,-1);
        assert!(!legacy.orbit_world_angles);
        assert!(legacy.orbit_collision);
        let rig=StudioDemoFollowRig {
            perspective:StudioFollowPerspective::Orbit,
            position_anchor:StudioDemoAnchor::RightHand,
            position_offset:[-12.0,9.0,44.0],position_local:true,
            aim_enabled:true,aim_client:4,aim_anchor:StudioDemoAnchor::HeadTop,
            aim_offset:[0.0,4.0,0.0],aim_local:false,
            orbit_world_angles:true,orbit_collision:false,
            ..legacy
        };
        let saved=serde_json::to_string(&rig).unwrap();
        let restored:StudioDemoFollowRig=serde_json::from_str(&saved).unwrap();
        assert_eq!(restored,rig);
        assert!(restored.valid());
        assert_eq!(rig.position_anchor.bone_name(),Some("*r_hand"));
        assert_eq!(rig.aim_anchor.bone_name(),Some("*head_top"));
        assert!(!StudioDemoFollowRig {aim_client:1024,..rig}.valid());
        assert!(!StudioDemoFollowRig {aim_offset:[f32::NAN,0.,0.],..rig}.valid());
    }
    #[test] fn follow_move_and_fallback_settings_round_trip_and_validate() {
        let old:StudioDemoFollowRig=serde_json::from_str(r#"{"blend_seconds":2.0}"#).unwrap();
        assert_eq!(old.fallback,StudioFollowFallback::RecordedPov);
        assert_eq!(old.move_path,StudioFollowMovePath::Direct);
        let rig=StudioDemoFollowRig {fallback:StudioFollowFallback::KeyedCamera,
            move_path:StudioFollowMovePath::ArcRight,move_arc:155.0,
            move_aim:StudioFollowMoveAim::LookAtSubjects,avoid_walls:true,..Default::default()};
        let loaded:StudioDemoFollowRig=serde_json::from_str(&serde_json::to_string(&rig).unwrap()).unwrap();
        assert_eq!(loaded,rig);
        assert!(loaded.valid());
        assert!(!StudioDemoFollowRig {move_arc:f32::NAN,..rig}.valid());
    }
    #[test] fn camera_source_is_a_discrete_cut_and_survives_json() {
        let mut tl=StudioTimeline::default();
        tl.camera.push(StudioCameraKey {time:3.0,source:StudioCameraSource::Demo {
            pov:StudioDemoPov::Follow,client:7 },..Default::default()});
        assert_eq!(tl.camera_at(2.9).source,StudioCameraSource::Animation);
        assert_eq!(tl.camera_at(3.0).source,StudioCameraSource::Demo {pov:StudioDemoPov::Follow,client:7});
        let json=serde_json::to_string(&tl.camera).unwrap();
        let restored:Vec<StudioCameraKey>=serde_json::from_str(&json).unwrap();
        assert_eq!(restored[1].source,tl.camera[1].source);
        let legacy:StudioCameraKey=serde_json::from_str(r#"{"time":0.0,"position":[0.0,0.0,0.0],"yaw":0.0,"pitch":0.0,"fov":58.0}"#).unwrap();
        assert_eq!(legacy.source,StudioCameraSource::Animation);
    }
    #[test] fn timeline_zoom_preserves_pointer_time_and_clamps() {
        let mut timeline=StudioTimeline::default();
        timeline.scroll=3.0;
        let offset=420.0;
        let anchor=timeline.scroll+offset/timeline.zoom;
        timeline.zoom_at(offset,120.0);
        assert!(timeline.zoom>90.0);
        assert!((timeline.scroll+offset/timeline.zoom-anchor).abs()<0.0001);
        timeline.zoom_at(offset,-120.0);
        assert!((timeline.zoom-90.0).abs()<0.0001);
        timeline.zoom_at(0.0,10000.0);
        assert_eq!(timeline.zoom,350.0);
        for _ in 0..3 {timeline.zoom_at(0.0,-10000.0);}
        assert_eq!(timeline.zoom,0.1);
        assert!(timeline.scroll>=0.0);
    }
    #[test] fn interpolation_switch() {
        let mut tl=StudioTimeline::default();
        tl.camera=vec![StudioCameraKey {yaw:350.0,..Default::default()},
            StudioCameraKey {time:2.0,yaw:10.0,position:[10.,0.,0.],..Default::default()}];
        assert!((tl.camera_at(1.0).yaw-360.0).abs()<0.01);
        tl.interpolation=StudioInterpolation::Linear;
        assert!((tl.camera_at(0.5).position[0]-2.5).abs()<0.01);
    }
    #[test] fn trim_keeps_endpoint_and_source_frame() {
        let mut tl=StudioTimeline::default();tl.add_clip(clip(0,40,0.0));
        tl.trim_clip(0,StudioDragKind::TrimLeft,0.0,0,40,0.5);
        assert_eq!(tl.clips[0].first_frame,10);
        assert!((tl.clips[0].end()-2.0).abs()<1e-6);
    }
    #[test] fn split_is_contiguous() {
        let mut tl=StudioTimeline::default();tl.add_clip(clip(0,40,0.0));
        assert!(tl.split_clip(0,1.0));assert_eq!(tl.clips.len(),2);
        assert_eq!(tl.clips[0].end_frame,tl.clips[1].first_frame);
        assert!((tl.clips[0].end()-tl.clips[1].start).abs()<1e-6);
    }
    #[test] fn drag_can_cross_neighbor_without_reindexing() {
        let mut tl=StudioTimeline::default();tl.snap=false;
        tl.add_clip(clip(0,20,0.0));tl.add_clip(clip(0,20,1.0));
        tl.move_clip(0,4.0);
        assert_eq!(tl.clips[0].start,4.0);
        assert_eq!(tl.clips[1].start,1.0);
    }
    #[test] fn source_extension_hold_loop_and_trim() {
        let mut tl=StudioTimeline::default();tl.add_clip(clip(0,20,0.0));
        tl.trim_clip(0,StudioDragKind::TrimRight,0.0,0,20,2.0);
        assert_eq!(tl.clips[0].end_frame,60);
        assert_eq!(tl.clips[0].tail_frames,0);
        tl.trim_clip(0,StudioDragKind::TrimRight,0.0,0,60,3.0);
        assert_eq!(tl.clips[0].end_frame,100);
        assert_eq!(tl.clips[0].tail_frames,20);
        let c=&mut tl.clips[0];
        let held=c.pose_time_ms(5.5);
        assert_eq!(held,c.pose_time_ms(5.0));
        c.playback=StudioClipPlayback::Loop;
        assert!(c.pose_time_ms(5.5)<held);
    }
    #[test] fn first_camera_key_is_removable_and_retimable() {
        let mut tl=StudioTimeline::default();
        tl.move_camera_key(0,0.5);
        assert!((tl.camera[0].time-0.5).abs()<1e-6);
        tl.selected_camera=Some(0);
        assert!(tl.delete_selection());
        assert!(tl.camera.is_empty());
        assert_eq!(tl.camera_at(0.0).fov,58.0);
    }
    #[test] fn all_active_lanes_render() {
        let mut tl=StudioTimeline::default();tl.lanes=2;
        tl.add_clip(clip(0,20,0.0));
        let mut c=clip(0,20,0.0);c.lane=1;tl.add_clip(c);
        assert_eq!(tl.active_actors(0.5).len(),2);
        assert_eq!(tl.track_count(),5);
        assert_eq!(tl.character_row(1),3);
        assert_eq!(tl.motion_row(1),4);
        assert!(tl.move_clip_to_lane(0,1,3.0));
        assert_eq!(tl.clips[0].lane,1);
        assert_eq!(tl.active_actors(0.5).len(),1);
    }
    #[test] fn expanded_speed_rows_keep_every_drop_target_mapped_to_its_actor() {
        let mut tl=StudioTimeline::default();tl.lanes=3;tl.lane_pinned=vec![true;3];
        tl.toggle_speed_track(0);tl.toggle_speed_track(2);
        assert_eq!(tl.character_row(0),1);
        assert_eq!(tl.speed_row(0),Some(3));
        assert_eq!(tl.character_row(1),4);
        assert_eq!(tl.character_row(2),6);
        assert_eq!(tl.speed_row(2),Some(8));
        assert_eq!(tl.demo_row(0),9);
        assert_eq!(tl.lane_for_row(3),Some(0));
        assert_eq!(tl.lane_for_row(5),Some(1));
        assert_eq!(tl.lane_for_row(8),Some(2));
        assert_eq!(tl.lane_for_row(9),None);
        // No demo footage: no demo rows.
        tl.toggle_demo_speed(0);
        assert_eq!(tl.demo_speed_row(0),None);
        assert_eq!(tl.row_kind(9),None);
        assert_eq!(tl.track_count(),9);
        tl.remove_lane(0);
        assert_eq!(tl.character_row(0),1);
        assert_eq!(tl.speed_row(0),None);
        assert_eq!(tl.speed_row(1),Some(5));
    }
    #[test] fn demo_group_fold_hides_audio_and_speed_without_changing_playback() {
        let mut tl=StudioTimeline::default();
        tl.demo_layer_pinned=vec![true,true];
        tl.toggle_demo_speed(0);
        assert_eq!(tl.demo_speed_row(0),Some(2));
        assert_eq!(tl.demo_audio_row(0),Some(3));
        assert_eq!(tl.demo_row(1),4);
        let duration=tl.duration();
        tl.toggle_demo_layer(0);
        assert_eq!(tl.demo_speed_row(0),None);
        assert_eq!(tl.demo_audio_row(0),None);
        assert_eq!(tl.demo_row(1),2);
        assert_eq!(tl.row_kind(3),Some(StudioTrackRow::DemoAudio(1)));
        assert_eq!(tl.track_count(),4);
        assert_eq!(tl.duration(),duration);
        assert!(!tl.demo_layer_hidden(0));
        tl.toggle_demo_layer(0);
        assert_eq!(tl.demo_speed_row(0),Some(2));
        assert_eq!(tl.demo_audio_row(0),Some(3));
        assert_eq!(tl.row_kind(4),Some(StudioTrackRow::Demo(1)));
    }
    #[test] fn dragging_speed_key_changes_parent_duration_without_moving_its_start() {
        let mut tl=StudioTimeline::default();
        let mut first=clip(0,100,3.0);
        first.speed_keys=vec![StudioSpeedKey{time:0.0,rate:1.0}];
        tl.clips.push(first);
        let initial=tl.clips[0].duration();
        let drag=StudioSpeedDrag {key:StudioSpeedSelection::Animation(0,0),pointer_x:0.0,pointer_y:0.0,
            scroll:0.0,keys:tl.clips[0].speed_keys.clone(),source_in:0.0,source_out:5.0,
            source_time:0.0,rate:1.0};
        assert!(tl.move_speed_key(&drag,0.0,-4.0));
        assert_eq!(tl.clips[0].start,3.0);
        assert!(tl.clips[0].duration()<initial);
        let mut neighbour=clip(0,20,8.1);neighbour.lane=0;
        tl.clips.push(neighbour);
        assert!(!tl.move_speed_key(&drag,0.0,15.0));
        assert!(tl.clips[0].end()<8.1);
    }
        #[test] fn export_range_offsets_exact_frames() {
        let mut tl=StudioTimeline::default();tl.in_point=2.0;tl.out_point=Some(3.0);
        assert!((tl.current_time(Some((15,30)))-2.5).abs()<1e-6);
    }
}

#[cfg(test)]
mod demo_track_tests {
    use super::*;
    #[test]
    fn demo_is_independent_of_character_tracks_and_uses_ms() {
        let mut tl=StudioTimeline::default();tl.snap=false;
        tl.add_demo(StudioDemoClip { source_path: None, comparison: None, race_overlays: Vec::new(), pov_trail:Default::default(), hud_overrides:Default::default(), hide_other_players:false,layer:0,name:"race".into(),game:String::new(),start:3.0,
            duration_ms:5000,in_ms:1000,out_ms:4000,speed_keys:vec![],transition_in:Default::default(),audio:Default::default()});
        assert_eq!(tl.demo_row(0),1);
        assert_eq!(tl.demos[0].duration(),3.0);
        assert_eq!(tl.demos[0].source_time_ms(4.25),2250);
        assert_eq!(tl.demo_at(4.25).unwrap().name,"race");
        assert!(tl.demo_at(6.0).is_none());
        assert_eq!(tl.duration(),6.0);
        let second=StudioDemoClip { source_path: None, comparison: None, race_overlays: Vec::new(), pov_trail:Default::default(), hud_overrides:Default::default(), hide_other_players:false,layer:0,name:"next".into(),game:String::new(),start:4.0,
            duration_ms:4000,in_ms:0,out_ms:2000,speed_keys:vec![],transition_in:Default::default(),audio:Default::default()};
        tl.add_demo(second);
        assert_eq!(tl.demos[1].start,6.0);
        tl.move_demo(0,9.0);
        assert_eq!(tl.demos[0].start,9.0);
    }
    #[test]
    fn demo_trim_preserves_source_clock_and_original_end() {
        let mut tl=StudioTimeline::default();tl.snap=false;
        tl.add_demo(StudioDemoClip { source_path: None, comparison: None, race_overlays: Vec::new(), pov_trail:Default::default(), hud_overrides:Default::default(), hide_other_players:false,layer:0,name:"clip".into(),game:String::new(),start:3.0,
            duration_ms:10_000,in_ms:1_000,out_ms:5_000,speed_keys:vec![],transition_in:Default::default(),audio:Default::default()});
        tl.trim_demo(0,StudioDemoDragKind::In,3.0,1000,5000,0.5);
        assert_eq!(tl.demos[0].in_ms,1500);
        assert!((tl.demos[0].start-3.5).abs()<1e-6);
        assert!((tl.demos[0].end()-7.0).abs()<1e-6);
        assert_eq!(tl.demos[0].source_time_ms(3.5),1500);
        tl.trim_demo(0,StudioDemoDragKind::Out,3.5,1500,5000,1.0);
        assert_eq!(tl.demos[0].out_ms,6000);
        assert!((tl.demos[0].end()-8.0).abs()<1e-6);
        assert!(tl.demos[0].valid());
    }
    #[test]
    fn demo_trim_can_author_overlap_but_cannot_swallow_neighbors_or_source_bounds() {
        let mut tl=StudioTimeline::default();tl.snap=false;
        tl.add_demo(StudioDemoClip { source_path: None, comparison: None, race_overlays: Vec::new(), pov_trail:Default::default(), hud_overrides:Default::default(), hide_other_players:false,layer:0,name:"first".into(),game:String::new(),start:2.0,
            duration_ms:10_000,in_ms:1000,out_ms:3000,speed_keys:vec![],transition_in:Default::default(),audio:Default::default()});
        tl.add_demo(StudioDemoClip { source_path: None, comparison: None, race_overlays: Vec::new(), pov_trail:Default::default(), hud_overrides:Default::default(), hide_other_players:false,layer:0,name:"second".into(),game:String::new(),start:5.0,
            duration_ms:10_000,in_ms:2000,out_ms:4000,speed_keys:vec![],transition_in:Default::default(),audio:Default::default()});
        // A longer OUT trim may create a transition, but must stop before
        // engulfing the entire following demo, not jump to another position.
        tl.trim_demo(0,StudioDemoDragKind::Out,2.0,1000,3000,100.0);
        assert!(tl.demos[0].end()>5.0);
        assert!(tl.demos[0].end()<7.0);
        assert_eq!(tl.demos[1].transition_in,StudioDemoTransition::Dissolve);
        assert!(StudioTimeline::demo_overlap_valid(&tl.demos));
        let b=tl.demos[1].clone();
        // An IN edge cannot move past the limits imposed by a neighboring
        // transition (and cannot extend the source earlier than zero).
        tl.trim_demo(1,StudioDemoDragKind::In,b.start,b.in_ms,b.out_ms,-100.0);
        assert!(StudioTimeline::demo_overlap_valid(&tl.demos));
        let b=tl.demos[1].clone();
        tl.trim_demo(1,StudioDemoDragKind::Out,b.start,b.in_ms,b.out_ms,-100.0);
        assert!(StudioTimeline::demo_overlap_valid(&tl.demos));
        assert!(tl.demos.iter().all(StudioDemoClip::valid));
    }
    #[test]
    fn demo_path_validation_rejects_directory_traversal() {
        let clip=StudioDemoClip { source_path: None, comparison: None, race_overlays: Vec::new(), pov_trail:Default::default(), hud_overrides:Default::default(), hide_other_players:false,layer:0,name:"../danger".into(),game:String::new(),start:0.0,
            duration_ms:1000,in_ms:0,out_ms:1000,speed_keys:vec![],transition_in:Default::default(),audio:Default::default()};
        assert!(!clip.valid());
        assert!(StudioDemoClip { source_path: None, comparison: None, race_overlays: Vec::new(), pov_trail:Default::default(), hud_overrides:Default::default(), hide_other_players:false,layer:0,name:"good/subdir".into(),..clip}.valid());
    }
}

#[cfg(test)]
mod speed_remap_tests {
    use super::*;
    use super::tests::clip;
    fn animation() -> StudioAnimationClip {
        StudioAnimationClip { name:"BOTH_STAND1".into(),emote:None,start:3.0,
            first_frame:10,end_frame:50,source_fps:20.0,lane:0,
            source_first:0,source_end:60,motion:vec![StudioMotionKey::default()],
            tail_frames:0,playback:StudioClipPlayback::Hold,speed_keys:vec![], blend:Default::default() }
    }
    fn demo() -> StudioDemoClip {
        StudioDemoClip { source_path: None, comparison: None, race_overlays: Vec::new(), pov_trail:Default::default(), hud_overrides:Default::default(), hide_other_players:false,layer:0,name:"recording".into(),game:String::new(),start:2.0,
            duration_ms:5000,in_ms:1000,out_ms:4000,speed_keys:vec![],transition_in:Default::default(),audio:Default::default()}
    }
    #[test]
    fn constant_speed_changes_clip_length_and_frame_sampling() {
        let mut c=animation();
        assert_eq!(c.duration(),2.0);
        // Stock animation presentation already starts at first_frame. Its clock
        // is elapsed time inside the visible trim, not untrimmed GLA time.
        assert_eq!(c.pose_time_ms(3.5),500);
        studio_add_speed_key(&mut c.speed_keys,0.0,0.5);
        assert!((c.duration()-4.0).abs()<1e-8);
        assert_eq!(c.pose_time_ms(4.0),500);
        assert!((c.source_key_time(4.0)-1.0).abs()<0.000001);
        assert!((c.timeline_at_source(1.0)-4.0).abs()<0.000001);
        c.speed_keys[0].rate=2.0;
        assert!((c.duration()-1.0).abs()<1e-8);
        assert_eq!(c.pose_time_ms(3.5),1000);
    }
    /// Pre-walk O(K²)/bisection implementation, kept as the reference.
    mod speed_reference {
        use super::super::{StudioSpeedKey,studio_speed_at};
        pub fn span(keys:&[StudioSpeedKey],a:f64,b:f64)->f64 {
            if (a-b).abs()<0.000000001 {return 0.0;}
            if b<a {return -span(keys,b,a);}
            let (mut previous,mut duration)=(a,0.0);
            for bound in keys.iter().map(|key|key.time).filter(|&t|t>a && t<b).chain(std::iter::once(b)) {
                let left=studio_speed_at(keys,previous).max(StudioSpeedKey::MIN);
                let right=studio_speed_at(keys,bound).max(StudioSpeedKey::MIN);
                let width=bound-previous;
                duration+=if (left-right).abs()<0.00000001 {width/left} else {width*(right/left).ln()/(right-left)};
                previous=bound;
            }
            duration
        }
        pub fn speed_at(keys:&[StudioSpeedKey],source:f64)->f64 {
            let Some(first)=keys.first() else {return 1.0};
            if source<=first.time {return first.rate;}
            for pair in keys.windows(2) {
                if source<=pair[1].time {
                    let t=((source-pair[0].time)/(pair[1].time-pair[0].time).max(0.000001)).clamp(0.0,1.0);
                    return pair[0].rate+(pair[1].rate-pair[0].rate)*t;
                }
            }
            keys.last().map_or(1.0,|k|k.rate)
        }
        pub fn shift_back(keys:&[StudioSpeedKey],initial:f64,delta:f64,minimum:f64)->f64 {
            let desired=(-delta).min(span(keys,minimum,initial));
            let (mut low,mut high)=(minimum,initial);
            for _ in 0..60 {
                let mid=(low+high)*0.5;
                if span(keys,mid,initial)>desired {low=mid;} else {high=mid;}
            }
            (low+high)*0.5
        }
    }
    fn random_speed_keys(seed:&mut u64,limit:f64)->Vec<StudioSpeedKey> {
        let mut next=||{*seed^=*seed<<13;*seed^=*seed>>7;*seed^=*seed<<17;(*seed>>11) as f64/(1u64<<53) as f64};
        let count=(next()*120.0) as usize;
        let mut keys=Vec::new();
        for _ in 0..count {
            let rate=if next()<0.2 {1.0} else {StudioSpeedKey::MIN+next()*(StudioSpeedKey::MAX-StudioSpeedKey::MIN)};
            studio_add_speed_key(&mut keys,(next()*limit*1000.0).round()/1000.0,rate);
        }
        keys.dedup_by(|b,a|b.time<a.time+0.00001);
        keys
    }
    #[test]
    fn speed_walk_matches_reference_and_inverts_for_random_keys() {
        let mut seed=0x9e3779b97f4a7c15u64;
        let limit=780.0;
        for _ in 0..200 {
            let keys=random_speed_keys(&mut seed,limit);
            assert!(studio_speed_valid(&keys,limit));
            let total=studio_speed_span(&keys,0.0,limit);
            for step in 0..=64 {
                let source=limit*f64::from(step)/64.0+if step%3==1 {0.0123} else {0.0};
                let source=source.min(limit);
                assert!((studio_speed_at(&keys,source)-speed_reference::speed_at(&keys,source)).abs()<1e-12);
                let a=source*0.37;
                let span=studio_speed_span(&keys,a,source);
                let reference=speed_reference::span(&keys,a,source);
                assert!((span-reference).abs()<1e-9*reference.abs().max(1.0),"span {span} vs {reference}");
                let elapsed=studio_speed_span(&keys,0.0,source);
                let back=studio_speed_source_at(&keys,0.0,limit,elapsed);
                assert!((back-source).abs()<1e-7,"inverse {back} vs {source}");
                assert!(back>=0.0 && back<=limit);
                let delta=-(elapsed*0.41);
                let shifted=studio_speed_shift_source(&keys,source,delta,0.0,limit);
                let reference=speed_reference::shift_back(&keys,source,delta,0.0);
                assert!((shifted-reference).abs()<1e-7,"shift {shifted} vs {reference}");
                assert!((studio_speed_span(&keys,shifted,source)+delta).abs()<1e-7);
            }
            assert_eq!(studio_speed_source_at(&keys,0.0,limit,total+1.0),limit);
            assert_eq!(studio_speed_shift_source(&keys,limit,-(total+1.0),0.0,limit),0.0);
        }
    }
    #[test]
    fn ramps_integrate_in_source_space_and_invert_stably() {
        let keys=vec![StudioSpeedKey{time:0.0,rate:1.0},StudioSpeedKey{time:2.0,rate:2.0}];
        let total=studio_speed_span(&keys,0.0,2.0);
        assert!((total-2.0*2.0_f64.ln()).abs()<1e-10);
        for source in [0.1,0.5,1.0,1.5,1.99] {
            let elapsed=studio_speed_span(&keys,0.0,source);
            assert!((studio_speed_source_at(&keys,0.0,2.0,elapsed)-source).abs()<1e-8);
        }
        assert!((studio_speed_span(&keys,2.0,0.0)+total).abs()<1e-10);
    }
    #[test]
    fn demo_speed_controls_seek_and_trim_exactly() {
        let mut tl=StudioTimeline::default();tl.snap=false;
        let mut clip=demo();
        studio_add_speed_key(&mut clip.speed_keys,0.0,0.5);
        assert_eq!(clip.duration(),6.0);
        assert_eq!(clip.source_time_ms(4.0),2000);
        tl.add_demo(clip);
        tl.trim_demo(0,StudioDemoDragKind::In,2.0,1000,4000,2.0);
        assert_eq!(tl.demos[0].in_ms,2000);
        assert!((tl.demos[0].start-4.0).abs()<0.000001);
        assert!((tl.demos[0].end()-8.0).abs()<0.000001);
        tl.trim_demo(0,StudioDemoDragKind::Out,4.0,2000,4000,2.0);
        assert_eq!(tl.demos[0].out_ms,5000);
        assert!((tl.demos[0].end()-10.0).abs()<0.000001);
    }
    #[test]
    fn speed_keys_are_source_anchored_across_trims() {
        let mut c=animation();
        studio_add_speed_key(&mut c.speed_keys,0.0,1.0);
        studio_add_speed_key(&mut c.speed_keys,1.0,2.0);
        let before=c.speed_keys.clone();
        let mut timeline=StudioTimeline::default();timeline.snap=false;
        timeline.add_clip(c);
        timeline.trim_clip(0,StudioDragKind::TrimLeft,3.0,10,50,0.25);
        assert_eq!(timeline.clips[0].speed_keys,before);
        assert!(timeline.clips[0].first_frame>10);
        assert!(timeline.clips[0].source_key_time(timeline.clips[0].start)>0.5);
    }
    #[test]
    fn speed_key_selection_deletes_key_not_its_parent_clip() {
        let mut tl=StudioTimeline::default();
        let mut c=clip(0,20,0.0);
        studio_add_speed_key(&mut c.speed_keys,0.0,1.0);
        studio_add_speed_key(&mut c.speed_keys,0.5,1.5);
        tl.clips.push(c);
        tl.select_speed_key(StudioSpeedSelection::Animation(0,1));
        assert_eq!(tl.selected_speed,Some(StudioSpeedSelection::Animation(0,1)));
        assert_eq!(tl.selected_clip,Some(0));
        assert!(tl.delete_selected_speed_key());
        assert_eq!(tl.clips.len(),1);
        assert_eq!(tl.clips[0].speed_keys.len(),1);
        assert_eq!(tl.selected_speed,None);
    }
    #[test]
    fn removing_speed_key_cannot_expand_into_adjacent_animation() {
        let mut tl=StudioTimeline::default();
        let mut first=clip(0,20,0.0);
        first.speed_keys.push(StudioSpeedKey{time:0.0,rate:2.0});
        tl.clips=vec![first,clip(0,20,0.75)];
        tl.select_speed_key(StudioSpeedSelection::Animation(0,0));
        assert!(!tl.delete_selected_speed_key());
        assert_eq!(tl.clips[0].speed_keys.len(),1);
        assert_eq!(tl.selected_speed,Some(StudioSpeedSelection::Animation(0,0)));
    }
    #[test]
    fn demo_speed_key_selection_is_not_demo_clip_deletion() {
        let mut tl=StudioTimeline::default();
        let mut c=demo();
        c.speed_keys.push(StudioSpeedKey{time:0.0,rate:2.0});
        tl.demos.push(c);
        tl.select_speed_key(StudioSpeedSelection::Demo(0,0));
        assert!(tl.delete_selected_speed_key());
        assert_eq!(tl.demos.len(),1);
        assert!(tl.demos[0].speed_keys.is_empty());
    }
    #[test]
    fn legacy_demo_json_and_invalid_speed_keys() {
        let old=r#"{"name":"race","game":"","start":0.0,"duration_ms":1000,"in_ms":0,"out_ms":1000}"#;
        let parsed:StudioDemoClip=serde_json::from_str(old).unwrap();
        assert!(parsed.speed_keys.is_empty());
        assert_eq!(parsed.duration(),1.0);
        let mut broken=parsed.clone();
        broken.speed_keys=vec![StudioSpeedKey{time:0.0,rate:0.0}];
        assert!(!broken.valid());
        broken.speed_keys=vec![StudioSpeedKey{time:0.0,rate:2.0},StudioSpeedKey{time:0.0,rate:1.0}];
        assert!(!broken.valid());
    }
}

#[cfg(test)]
mod demo_transition_tests {
    use super::*;
    fn demo(name:&str,start:f64)->StudioDemoClip {
        StudioDemoClip { source_path: None, comparison: None, race_overlays: Vec::new(), pov_trail:Default::default(), hud_overrides:Default::default(), hide_other_players:false,layer:0,name:name.into(),game:String::new(),start,duration_ms:4000,
            in_ms:0,out_ms:4000,speed_keys:vec![],transition_in:StudioDemoTransition::Cut,audio:Default::default()}
    }
    #[test]
    fn overlap_evaluates_both_sources_and_source_time() {
        let mut tl=StudioTimeline::default();tl.snap=false;
        tl.add_demo(demo("outgoing",0.0));tl.add_demo(demo("incoming",4.0));
        assert!(tl.set_demo_transition(1,StudioDemoTransition::Dissolve,1.0));
        assert!((tl.demos[1].start-3.0).abs()<1e-7);
        let blend=tl.demo_blend_at(3.5).unwrap();
        assert_eq!((blend.outgoing,blend.incoming),(0,1));
        assert!((blend.progress-0.5).abs()<1e-7);
        assert_eq!(tl.demos[0].source_time_ms(3.5),3500);
        assert_eq!(tl.demos[1].source_time_ms(3.5),500);
        assert_eq!(tl.demo_at(3.5).unwrap().name,"incoming");
        assert!(StudioTimeline::demo_overlap_valid(&tl.demos));
        assert!(tl.demo_blend_at(4.0).is_none());
    }
    #[test]
    fn edits_are_transactional_and_cut_restores_nonoverlap() {
        let mut tl=StudioTimeline::default();tl.snap=false;
        for (name,start) in [("A",0.0),("B",4.0),("C",8.0)] {tl.add_demo(demo(name,start));}
        assert!(tl.set_demo_transition(1,StudioDemoTransition::WipeLeft,1.5));
        assert!(!tl.set_demo_transition(2,StudioDemoTransition::Iris,3.8)); // would hit A
        assert!((tl.demos[2].start-8.0).abs()<1e-8);
        assert!(tl.demo_has_transition(0));
        // Moving an outgoing clip now resizes the existing transition.
        tl.move_demo(0,0.5);assert_eq!(tl.demos[0].start,0.5);
        assert!(StudioTimeline::demo_overlap_valid(&tl.demos));
        tl.move_demo(0,0.0);
        assert!(tl.set_demo_transition(1,StudioDemoTransition::Cut,0.0));
        assert_eq!(tl.demos[1].start,4.0);
        assert!(StudioTimeline::demo_overlap_valid(&tl.demos));
    }
    #[test]
    fn dragging_incoming_demo_authors_and_resizes_overlap() {
        let mut tl=StudioTimeline::default();tl.snap=false;
        tl.add_demo(demo("A",0.0));tl.add_demo(demo("B",4.0));
        tl.move_demo(1,3.25);
        assert!((tl.demos[1].start-3.25).abs()<1e-7);
        assert_eq!(tl.demos[1].transition_in,StudioDemoTransition::Dissolve);
        let blend=tl.demo_blend_at(3.625).unwrap();
        assert!((blend.progress-0.5).abs()<1e-6);
        assert!(tl.set_demo_transition_type(1,StudioDemoTransition::WipeLeft));
        tl.move_demo(1,2.0);
        assert_eq!(tl.demos[1].transition_in,StudioDemoTransition::WipeLeft);
        assert!((tl.demo_blend_at(3.0).unwrap().progress-0.5).abs()<1e-6);
        tl.move_demo(1,4.0);
        assert_eq!(tl.demos[1].transition_in,StudioDemoTransition::Cut);
        assert!(tl.demo_blend_at(3.5).is_none());
        assert!(StudioTimeline::demo_overlap_valid(&tl.demos));
    }
    #[test]
    fn reversing_demo_order_preserves_visible_incoming_transition() {
        let mut tl=StudioTimeline::default();tl.snap=false;
        tl.add_demo(demo("first imported",0.0));
        tl.add_demo(demo("second imported",4.0));
        assert_eq!(tl.demo_draw_order(),vec![0,1]);
        // The first imported clip becomes the incoming clip, despite its
        // unchanged array index. Paint by start time or the outgoing body
        // will conceal the entire overlap decoration.
        tl.move_demo(0,5.0);
        assert_eq!(tl.demo_draw_order(),vec![1,0]);
        assert_eq!(tl.demos[0].transition_in,StudioDemoTransition::Dissolve);
        assert_eq!(tl.demos[1].transition_in,StudioDemoTransition::Cut);
        let blend=tl.demo_blend_at(6.5).expect("reversed overlap must blend");
        assert_eq!((blend.outgoing,blend.incoming),(1,0));
        assert!((blend.progress-0.5).abs()<1e-6);
        assert!(StudioTimeline::demo_overlap_valid(&tl.demos));
        // Resizing the new incoming source must not affect its draw priority.
        tl.trim_demo(0,StudioDemoDragKind::In,5.0,0,4000,0.5);
        assert_eq!(tl.demo_draw_order(),vec![1,0]);
        assert_eq!(tl.demos[0].transition_in,StudioDemoTransition::Dissolve);
        assert!(tl.demo_blend_at(6.5).is_some());
        // Switch which source comes last again, without changing their IDs.
        tl.move_demo(1,8.5);
        assert_eq!(tl.demo_draw_order(),vec![0,1]);
        assert_eq!(tl.demos[1].transition_in,StudioDemoTransition::Dissolve);
        assert_eq!(tl.demos[0].transition_in,StudioDemoTransition::Cut);
        assert_eq!(tl.demo_blend_at(8.75).unwrap().incoming,1);
        assert!(StudioTimeline::demo_overlap_valid(&tl.demos));
    }
    #[test]
    fn moving_outgoing_demo_also_authors_overlap_on_incoming() {
        let mut tl=StudioTimeline::default();tl.snap=false;
        tl.add_demo(demo("A",0.0));tl.add_demo(demo("B",4.0));
        tl.move_demo(0,1.0);
        assert_eq!(tl.demos[1].transition_in,StudioDemoTransition::Dissolve);
        assert!((tl.demo_blend_at(4.5).unwrap().progress-0.5).abs()<1e-6);
        tl.move_demo(0,0.0);
        assert_eq!(tl.demos[1].transition_in,StudioDemoTransition::Cut);
        assert!(StudioTimeline::demo_overlap_valid(&tl.demos));
    }
    #[test]
    fn drag_keeps_triple_overlaps_out_and_rejects_nonoverlap_type_edits() {
        let mut tl=StudioTimeline::default();tl.snap=false;
        for (name,start) in [("A",0.0),("B",4.0),("C",8.0)] {tl.add_demo(demo(name,start));}
        assert!(!tl.set_demo_transition_type(2,StudioDemoTransition::Iris));
        tl.move_demo(1,3.0); // A/B transition
        tl.move_demo(2,4.5); // would mix A, B and C near 4.5 if A still active (but A ends 4)
        assert!(StudioTimeline::demo_overlap_valid(&tl.demos));
        tl.move_demo(2,3.5); // actually intersects all three, project away
        assert!(StudioTimeline::demo_overlap_valid(&tl.demos));
        assert!(tl.demos[2].start>=4.0-1e-6);
    }
    #[test]
    fn trim_outer_edges_during_transition_resizes_not_moves_and_keeps_effect() {
        let mut tl=StudioTimeline::default();tl.snap=false;
        for (name,start) in [("A",0.0),("B",4.0)] {tl.add_demo(demo(name,start));}
        // Extend the sources so both outer edges have room to trim.
        for clip in &mut tl.demos {clip.duration_ms=10_000;}
        assert!(tl.set_demo_transition(1,StudioDemoTransition::WipeLeft,1.0));
        assert_eq!(tl.demos[0].start,0.0);
        assert_eq!(tl.demos[1].start,3.0);
        // Extend B's OUT while leaving B's start and the transition in place.
        tl.trim_demo(1,StudioDemoDragKind::Out,3.0,0,4000,1.0);
        assert!((tl.demos[1].end()-8.0).abs()<1e-7);
        assert!((tl.demos[1].start-3.0).abs()<1e-7);
        assert_eq!(tl.demos[1].transition_in,StudioDemoTransition::WipeLeft);
        // Shorten A's IN: its end remains fixed, and B must not move.
        tl.trim_demo(0,StudioDemoDragKind::In,0.0,0,4000,0.5);
        assert!((tl.demos[0].start-0.5).abs()<1e-7);
        assert!((tl.demos[0].end()-4.0).abs()<1e-7);
        assert!((tl.demos[1].start-3.0).abs()<1e-7);
        assert_eq!(tl.demos[1].transition_in,StudioDemoTransition::WipeLeft);
        assert!(tl.demo_blend_at(3.5).is_some());
        assert!(StudioTimeline::demo_overlap_valid(&tl.demos));
    }
    #[test]
    fn trim_inner_edges_updates_overlap_duration_and_reverts_to_cut() {
        let mut tl=StudioTimeline::default();tl.snap=false;
        tl.add_demo(demo("A",0.0));tl.add_demo(demo("B",4.0));
        assert!(tl.set_demo_transition(1,StudioDemoTransition::Iris,1.0));
        // OUT edge moves left 0.5s: overlap becomes 0.5s, Iris survives.
        tl.trim_demo(0,StudioDemoDragKind::Out,0.0,0,4000,-0.5);
        assert_eq!(tl.demos[1].transition_in,StudioDemoTransition::Iris);
        assert!((tl.demo_blend_at(3.25).unwrap().progress-0.5).abs()<1e-6);
        // OUT edge moves fully back before B starts: no transition anymore.
        tl.trim_demo(0,StudioDemoDragKind::Out,0.0,0,4000,-1.25);
        assert!(tl.demos[0].end()<tl.demos[1].start);
        assert_eq!(tl.demos[1].transition_in,StudioDemoTransition::Cut);
        assert!(tl.demo_blend_at(3.25).is_none());
        assert!(StudioTimeline::demo_overlap_valid(&tl.demos));
    }
    #[test]
    fn trim_rejects_triple_overlap_and_keeps_valid_timeline() {
        let mut tl=StudioTimeline::default();tl.snap=false;
        for (name,start) in [("A",0.0),("B",4.0),("C",8.0)] {tl.add_demo(demo(name,start));}
        assert!(tl.set_demo_transition(1,StudioDemoTransition::Dissolve,1.0));
        assert!(tl.set_demo_transition(2,StudioDemoTransition::WipeRight,1.0));
        for clip in &mut tl.demos {clip.duration_ms=10_000;}
        tl.trim_demo(0,StudioDemoDragKind::Out,0.0,0,4000,100.0);
        assert!(tl.demos[0].end()<=tl.demos[2].start+0.001);
        assert!(StudioTimeline::demo_overlap_valid(&tl.demos));
        assert!(tl.demos.iter().all(StudioDemoClip::valid));
    }
    #[test]
    fn snapped_demo_edges_remain_butt_joined() {
        let mut tl=StudioTimeline::default();
        tl.add_demo(demo("A",0.0));tl.add_demo(demo("B",4.0));
        tl.move_demo(1,3.95); // ~4.5px from the adjoining end at 90px/s
        assert!((tl.demos[1].start-4.0).abs()<1e-7);
        assert_eq!(tl.demos[1].transition_in,StudioDemoTransition::Cut);
        tl.move_demo(1,3.5);
        assert_eq!(tl.demos[1].transition_in,StudioDemoTransition::Dissolve);
    }
    #[test]
    fn studio_race_sources_round_trip_and_validate_local_paths() {
        let overlay = StudioRaceOverlay { source_path: None,
            name: "race/fast-run".into(), game: "japro".into(),
            alpha: 0.5, enabled: true, strafe_trail: true, trail_ticks: true, trail_tick_labels:Default::default(), trail_color: None,
        };
        assert!(overlay.valid());
        let copy: StudioRaceOverlay = serde_json::from_str(&serde_json::to_string(&overlay).unwrap()).unwrap();
        assert_eq!(copy, overlay);
        let mut bad = overlay;
        bad.name = "../outside".into();
        assert!(!bad.valid());
    }
    #[test]
    fn studio_race_trail_colors_are_deterministic_and_legacy_safe() {
        let actor = StudioRaceOverlay { source_path: None, name:"runner".into(), game:"japro".into(),
            alpha:0.65, enabled:true, strafe_trail:true, trail_ticks:true, trail_tick_labels:Default::default(), trail_color:None };
        let json = serde_json::to_string(&actor).unwrap();
        let loaded:StudioRaceOverlay = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.resolved_trail_color(),actor.resolved_trail_color());
        let different = StudioRaceOverlay {name:"another-runner".into(), ..actor.clone()};
        assert_ne!(different.resolved_trail_color(),actor.resolved_trail_color());
        let legacy:StudioRaceOverlay = serde_json::from_str(
            r#"{"name":"runner","game":"japro","alpha":0.65,"enabled":true}"#).unwrap();
        assert!(!legacy.strafe_trail && !legacy.trail_ticks && legacy.trail_color.is_none());
    }
    #[test]
    fn own_pov_trail_and_composite_labels_persist_without_changing_gameplay() {
        use crate::strafe_trail::StudioTickLabels;
        let mut clip=demo("local",0.0);
        clip.pov_trail.enabled=true;
        clip.pov_trail.trail_ticks=true;
        clip.pov_trail.trail_tick_labels=StudioTickLabels::MinutesSeconds;
        let pov_color=clip.pov_trail.resolved_trail_color(&clip);
        let actor=StudioRaceOverlay { source_path: None, name:clip.name.clone(), game:clip.game.clone(),
            alpha:0.65,enabled:true,strafe_trail:true,trail_ticks:true,
            trail_tick_labels:StudioTickLabels::Seconds,trail_color:None };
        assert_eq!(pov_color,actor.resolved_trail_color());
        clip.race_overlays.push(actor);
        let copy:StudioDemoClip=serde_json::from_str(&serde_json::to_string(&clip).unwrap()).unwrap();
        assert_eq!(copy.pov_trail,clip.pov_trail);
        assert_eq!(copy.race_overlays,clip.race_overlays);
        let mut legacy=serde_json::to_value(&clip).unwrap();
        legacy.as_object_mut().unwrap().remove("pov_trail");
        let legacy:StudioDemoClip=serde_json::from_value(legacy).unwrap();
        assert!(!legacy.pov_trail.enabled);
        assert_eq!(legacy.pov_trail.trail_tick_labels,StudioTickLabels::Off);
    }
    #[test]
    fn composite_assignments_are_scoped_to_one_demo_clip_and_persist() {
        let mut main = demo("main",0.0);
        let alt = demo("alternate",5.0);
        main.race_overlays.push(StudioRaceOverlay { source_path: None,
            name: "recordings/alternate-run".into(), game: "japro".into(),
            alpha: 0.42, enabled: true, strafe_trail: false, trail_ticks: false, trail_tick_labels:Default::default(), trail_color: Some([55, 110, 220]),
        });
        assert!(main.valid());
        let loaded: StudioDemoClip=serde_json::from_str(&serde_json::to_string(&main).unwrap()).unwrap();
        assert_eq!(loaded.race_overlays, main.race_overlays);
        assert!(alt.race_overlays.is_empty());
        assert_eq!(main.clone().race_overlays.len(),1);
        let mut invalid=main;
        invalid.race_overlays.push(invalid.race_overlays[0].clone());
        assert!(!invalid.valid());
    }
    #[test]
    fn legacy_demo_deserialization_defaults_composites_to_empty() {
        let mut value=serde_json::to_value(demo("main",0.0)).unwrap();
        value.as_object_mut().unwrap().remove("race_overlays");
        let loaded: StudioDemoClip=serde_json::from_value(value).unwrap();
        assert!(loaded.race_overlays.is_empty() && loaded.valid());
    }
    #[test]
    fn saved_json_defaults_old_clips_to_cuts() {
        let mut value=serde_json::to_value(demo("A",0.0)).unwrap();
        value.as_object_mut().unwrap().remove("transition_in");
        let loaded:StudioDemoClip=serde_json::from_value(value).unwrap();
        assert_eq!(loaded.transition_in,StudioDemoTransition::Cut);
    }
    #[test]
    fn reject_unmarked_overlap_and_three_way_worlds() {
        let mut demos=vec![demo("A",0.0),demo("B",3.0)];
        assert!(!StudioTimeline::demo_overlap_valid(&demos));
        demos[1].transition_in=StudioDemoTransition::Dissolve;
        assert!(StudioTimeline::demo_overlap_valid(&demos));
        demos.push(demo("C",3.5));demos[2].transition_in=StudioDemoTransition::Dissolve;
        assert!(!StudioTimeline::demo_overlap_valid(&demos));
    }
}

#[cfg(test)]
mod linked_demo_audio_tests {
    use super::*;

    fn demo() -> StudioDemoClip {
        StudioDemoClip { source_path: None, comparison: None, race_overlays: Vec::new(), pov_trail:Default::default(), hud_overrides:Default::default(), hide_other_players:false,layer:0,name:"linked".into(),game:String::new(),start:2.0,
            duration_ms:5000,in_ms:500,out_ms:4500,
            speed_keys:Vec::new(),transition_in:StudioDemoTransition::Cut,
            audio:StudioDemoAudio::default()}
    }

    #[test]
    fn backwards_compatible_demo_creates_linked_audio() {
        let old=r#"{"name":"linked","game":"","start":2.0,"duration_ms":5000,"in_ms":500,"out_ms":4500}"#;
        let clip:StudioDemoClip=serde_json::from_str(old).unwrap();
        assert!(clip.audio.valid(5.0,clip.duration()));
        assert!((clip.audio_gain_at(3.0)-1.0).abs()<0.00001);
        let restored:StudioDemoClip=serde_json::from_str(&serde_json::to_string(&clip).unwrap()).unwrap();
        assert_eq!(restored.audio.keys.len(),0);
        assert_eq!(restored.audio.fade_curve,crate::app::render_studio_audio::StudioAudioFade::Linear);
    }

    #[test]
    fn audio_key_is_anchored_to_the_same_source_clock_as_speed() {
        let mut clip=demo();
        clip.audio.add_key(0.5,0.0);
        clip.audio.add_key(2.5,1.0);
        let before=clip.timeline_at_source_ms(2500);
        assert!((clip.audio_gain_at(before)-1.0).abs()<0.001);
        clip.speed_keys=vec![StudioSpeedKey{time:0.0,rate:2.0}];
        let after=clip.timeline_at_source_ms(2500);
        assert!(after<before);
        assert!((clip.audio_gain_at(after)-1.0).abs()<0.001);
        assert!(clip.valid());
        clip.audio.mute=true;
        assert_eq!(clip.audio_gain_at(after),0.0);
    }

    #[test]
    fn linked_demo_audio_fade_in_out_use_edited_output_time() {
        let mut clip=demo();
        clip.audio.fade_in=1.0;
        clip.audio.fade_out=0.5;
        assert_eq!(clip.audio_gain_at(clip.start),0.0);
        assert!((clip.audio_gain_at(clip.start+0.25)-0.25).abs()<0.001);
        assert!((clip.audio_gain_at(clip.end()-0.25)-0.5).abs()<0.001);
        clip.speed_keys=vec![StudioSpeedKey{time:0.0,rate:2.0}];
        assert!((clip.audio_gain_at(clip.start+0.25)-0.25).abs()<0.001);
        assert!((clip.audio_gain_at(clip.end()-0.25)-0.5).abs()<0.001);
        assert!(clip.valid());
        clip.audio.fade_curve=crate::app::render_studio_audio::StudioAudioFade::EqualPower;
        let curved=clip.audio_gain_at(clip.start+0.25);
        assert!(curved>0.25 && curved<1.0);
    }

    #[test]
    fn trimming_a_demo_clamps_linked_audio_fades_to_new_duration() {
        let mut timeline=StudioTimeline::default();
        let mut clip=demo();
        clip.audio.fade_out=3.0;
        let start=clip.start;
        let in_ms=clip.in_ms;
        let out_ms=clip.out_ms;
        timeline.demos.push(clip);
        timeline.trim_demo(0,StudioDemoDragKind::Out,start,in_ms,out_ms,-2.0);
        assert!(timeline.demos[0].out_ms<out_ms);
        assert!(timeline.demos[0].audio.fade_out<=timeline.demos[0].duration());
        assert!(timeline.demos[0].valid());
    }

    #[test]
    fn moving_a_demo_keeps_its_audio_automatically_linked() {
        let mut clip=demo();
        clip.audio.fade_in=0.25;
        let initial=clip.audio_gain_at(clip.start+0.125);
        clip.start+=10.0;
        assert!((clip.audio_gain_at(clip.start+0.125)-initial).abs()<0.00001);
        assert!(clip.valid());
    }
}

#[cfg(test)]
mod demo_layer_tests {
    use super::*;
    fn demo(name:&str,start:f64,layer:usize)->StudioDemoClip {
        StudioDemoClip { source_path: None, comparison: None, race_overlays: Vec::new(), pov_trail:Default::default(), hud_overrides:Default::default(), hide_other_players:false,name:name.into(),game:String::new(),start,duration_ms:4000,in_ms:0,out_ms:4000,
            speed_keys:vec![],transition_in:Default::default(),audio:Default::default(),layer}
    }
    #[test]
    fn selected_demo_hud_edits_apply_only_the_changed_element_to_all_targets() {
        let mut tl=StudioTimeline::default();
        tl.demos=vec![demo("a",0.0,0),demo("b",4.0,0),demo("c",8.0,0)];
        tl.demos[0].hud_overrides.insert("speedometer".into(),true);
        tl.demos[1].hud_overrides.insert("speedometer".into(),false);
        tl.demos[1].hud_overrides.insert("strafe_helper".into(),false);
        tl.demos[2].hud_overrides.insert("strafe_helper".into(),false);
        tl.select_demo(0,false,false);
        tl.select_demo(1,true,false);
        tl.set_demo_hud_override(1,"strafe_helper",Some(true));
        assert_eq!(tl.demos[0].hud_overrides.get("strafe_helper"),Some(&true));
        assert_eq!(tl.demos[1].hud_overrides.get("strafe_helper"),Some(&true));
        assert_eq!(tl.demos[2].hud_overrides.get("strafe_helper"),Some(&false));
        assert_eq!(tl.demos[0].hud_overrides.get("speedometer"),Some(&true));
        assert_eq!(tl.demos[1].hud_overrides.get("speedometer"),Some(&false));
        tl.set_demo_hud_override(1,"strafe_helper",None);
        assert!(!tl.demos[0].hud_overrides.contains_key("strafe_helper"));
        assert!(!tl.demos[1].hud_overrides.contains_key("strafe_helper"));
        // An inspector explicitly editing an unselected clip remains local.
        tl.set_demo_hud_override(2,"core",Some(true));
        assert!(!tl.demos[0].hud_overrides.contains_key("core"));
        assert_eq!(tl.demos[2].hud_overrides.get("core"),Some(&true));
        let unselected=tl.demos[2].hud_overrides.clone();
        tl.reset_demo_hud_overrides(1);
        assert!(tl.demos[0].hud_overrides.is_empty() && tl.demos[1].hud_overrides.is_empty());
        assert_eq!(tl.demos[2].hud_overrides,unselected);
    }
    #[test] fn parallel_layers_overlap_and_the_top_visible_layer_plays() {
        let mut tl=StudioTimeline::default();tl.snap=false;
        tl.add_demo(demo("a",0.0,0));
        tl.add_demo(demo("b",1.0,1));
        // Layers don't push each other: b keeps its start on its own layer.
        assert_eq!(tl.demos[1].start,1.0);
        assert!(StudioTimeline::demo_overlap_valid(&tl.demos));
        assert_eq!(tl.demo_at(2.0).unwrap().name,"a");
        assert!(tl.demo_blend_at(2.0).is_none(),"no cross-layer transition");
        tl.demo_layer_hidden=vec![true];
        assert_eq!(tl.demo_at(2.0).unwrap().name,"b");
        assert!(tl.demo_at(0.5).is_none());
        // Rows: Camera, Demo 1, audio 1, Demo 2, audio 2.
        assert_eq!(tl.demo_row(0),1);
        assert_eq!(tl.demo_row(1),3);
        assert_eq!(tl.row_kind(4),Some(StudioTrackRow::DemoAudio(1)));
        assert_eq!(tl.track_count(),5);
        assert_eq!(tl.demo_layer_for_drop(),2);
    }
    #[test] fn moving_a_demo_to_a_busy_layer_falls_back() {
        let mut tl=StudioTimeline::default();tl.snap=false;
        tl.add_demo(demo("a",0.0,0));
        tl.add_demo(demo("b",10.0,1));
        tl.move_demo_to_layer(1,0,1.0);
        assert!(StudioTimeline::demo_overlap_valid(&tl.demos));
        tl.move_demo_to_layer(1,0,20.0);
        assert_eq!(tl.demos[1].layer,0);
        assert_eq!(tl.demos[1].start,20.0);
    }
}
