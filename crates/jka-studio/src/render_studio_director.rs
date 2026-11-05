//! Camera Director: live, parametric demo shots. A shot is a time range on
//! the output timeline plus a camera behavior around a demo client: rig shots
//! that ride with the subject (chase, tracking, close-up, overhead, ...) and
//! world-anchored shots placed in the map (establishing, static, pan, fly-by,
//! crane). It is evaluated on demand from the output time only, never from
//! frame-to-frame state, so playback, scrubbing in any order and offline
//! export produce identical poses.
//!
//! A shot produces an ordinary `StudioCameraKey` (Demo source + follow rig),
//! which the existing native DM26 sampler resolves with its anchors, chase
//! collision and recorded-PVS fallback. No second camera path exists.
//! Automatic shot choice and placement live in `render_studio_auto_director`.
use super::render_studio_timeline::{StudioCameraKey, StudioCameraSource, StudioDemoAnchor, StudioDemoFollowRig,
    StudioDemoPov, StudioFollowPerspective, StudioTimeline};
use serde::{Deserialize, Serialize};

/// Length of a manually added shot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StudioShotLength {
    #[default] ToClipEnd,
    WholeClip,
    FourSeconds,
}

/// Background Auto Director work (route view, regenerate, placement).
pub struct StudioDirectorJob {
    pub worker: std::thread::JoinHandle<Result<StudioDirectorPlan, String>>,
    pub signature: String,
}

/// Result of an Auto Director job, committed atomically.
#[derive(Default)]
pub struct StudioDirectorPlan {
    /// (demo index, in_ms, out_ms) source trims.
    pub trims: Vec<(usize, i32, i32)>,
    /// Output ranges whose existing shots the new ones replace.
    pub clear_ranges: Vec<(f64, f64)>,
    /// Shots replaced in place by id.
    pub replaced: Vec<StudioDirectorShot>,
    /// New shots (ids assigned on commit).
    pub added: Vec<StudioDirectorShot>,
    /// (demo index, speed keys): slow motion around kills.
    pub speed_keys: Vec<(usize, Vec<super::render_studio_timeline::StudioSpeedKey>)>,
    pub summary: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StudioDirectorPreset {
    #[default] Chase,
    Orbit,
    /// First-person view through any client's eyes (not only the recorder).
    PlayerEyes,
    RecordedPov,
    /// Side-on camera travelling with the subject (truck / dolly track).
    Tracking,
    CloseUp,
    LowAngle,
    Overhead,
    /// Dolly in: the rig closes distance and tightens the lens.
    PushIn,
    /// Vertigo: dolly in while widening the lens, subject size held.
    DollyZoom,
    /// Wide, high world camera that sets up the space.
    Establishing,
    /// Locked-off world camera: the subject moves through a fixed frame.
    Static,
    /// Tripod pan: fixed world position, turns to follow the subject.
    Pan,
    /// Camera just off the line: the subject rushes past.
    FlyBy,
    /// World camera rising (or settling) while it follows the subject.
    Crane,
    /// Behind one fighter's shoulder, aimed at the opponent (Look-at client).
    OverShoulder,
    /// Side-on framing of both fighters across the line between them.
    TwoShot,
    /// World camera gliding on a curve while its view drifts from one area
    /// of action to another (planned, not tracking anyone).
    Glide,
    /// Rides just behind the saber tip, aimed at the opponent (Look-at client).
    SaberCam,
    /// First person through the victim's eyes, aimed at the attacker's saber tip.
    VictimPov,
    /// On the attacker's blade line at the victim's distance, victim hidden,
    /// aimed back at the attacker.
    VictimBlade,
    /// Chases the thrown saber's hilt toward its target (Look-at client).
    ThrowCam,
    /// Planned, damped chase: a smoothed camera path behind/beside the
    /// player keeping them and their opponent in frame (path_keys).
    FollowCam,
}

impl StudioDirectorPreset {
    /// Shots that ride with the subject.
    pub const FOLLOW: [Self; 17] = [Self::Chase, Self::FollowCam, Self::Tracking, Self::CloseUp, Self::LowAngle,
        Self::Overhead, Self::OverShoulder, Self::Orbit, Self::PushIn, Self::DollyZoom, Self::TwoShot, Self::SaberCam,
        Self::VictimPov, Self::VictimBlade, Self::ThrowCam, Self::PlayerEyes, Self::RecordedPov];
    /// Shots placed in the map.
    pub const WORLD: [Self; 6] = [Self::Establishing, Self::Static, Self::Pan, Self::FlyBy, Self::Crane, Self::Glide];

    /// One-line explanation shown under the behavior picker.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Chase => "Third-person chase camera behind the subject.",
            Self::Orbit => "Circles the subject; Sweep turns it across the shot.",
            Self::PlayerEyes => "First person from the subject's eyes: any player, not only the recorder.",
            Self::RecordedPov => "The demo's original camera, exactly as recorded (spectator switches, zoom, death cam).",
            Self::Tracking => "Side-on camera travelling alongside the subject, like a dolly on a track.",
            Self::OverShoulder => "Behind the subject's shoulder, looking at the opponent (set Look-at to the other player).",
            Self::TwoShot => "Side-on wide framing that keeps both fighters in shot.",
            Self::SaberCam => "Camera riding just behind the saber tip, looking at the opponent (set Look-at to them).",
            Self::VictimPov => "First person from the victim (Player), looking at the attacker's saber tip (Look-at = attacker).",
            Self::VictimBlade => "First person from the victim, watching the attacker's hilt as the swing comes in (Look-at = attacker).",
            Self::ThrowCam => "Chases the saber hilt (great on throws) toward the Look-at player, saber kept in the foreground.",
            Self::FollowCam => "Loose, damped chase planned from the recording: big offsets and lag, keeping the player and their opponent in frame.",
            Self::CloseUp => "Tight, long-lens framing of the face with shallow focus.",
            Self::LowAngle => "Low camera looking up at the subject: the hero angle.",
            Self::Overhead => "High top-down view that shows the line being taken.",
            Self::PushIn => "Dolly in: the camera closes in and the lens tightens over the shot.",
            Self::DollyZoom => "Vertigo effect: the camera moves in while the lens widens, subject size held.",
            Self::Establishing => "Wide, high camera in the map that sets up the space, drifting slowly.",
            Self::Static => "Locked-off camera in the map: the subject moves through a fixed frame.",
            Self::Pan => "Camera fixed in the map that pans to follow the subject.",
            Self::FlyBy => "Camera just off the line: the subject rushes past the lens.",
            Self::Crane => "Camera in the map that rises while it follows the subject.",
            Self::Glide => "Camera gliding along a curve in the map while its view drifts from one area of action to the next.",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Chase => "Chase",
            Self::Orbit => "Orbit",
            Self::PlayerEyes => "Player eyes",
            Self::RecordedPov => "Recorded camera",
            Self::Tracking => "Tracking",
            Self::OverShoulder => "Over-the-shoulder",
            Self::TwoShot => "Two-shot",
            Self::SaberCam => "Saber cam",
            Self::VictimPov => "Victim POV",
            Self::VictimBlade => "Victim blade view",
            Self::ThrowCam => "Throw cam",
            Self::FollowCam => "Follow cam",
            Self::CloseUp => "Close-up",
            Self::LowAngle => "Low angle",
            Self::Overhead => "Overhead",
            Self::PushIn => "Push-in",
            Self::DollyZoom => "Dolly zoom",
            Self::Establishing => "Establishing",
            Self::Static => "Static",
            Self::Pan => "Pan",
            Self::FlyBy => "Fly-by",
            Self::Crane => "Crane",
            Self::Glide => "Glide",
        }
    }
    /// Placed at a world position (`position_override`) instead of riding a rig.
    pub fn world_anchored(self) -> bool { Self::WORLD.contains(&self) }
    /// Rig perspective a follow shot evaluates with.
    pub fn perspective(self) -> StudioFollowPerspective {
        match self {
            Self::Chase | Self::OverShoulder | Self::FollowCam => StudioFollowPerspective::ThirdPerson,
            Self::PlayerEyes | Self::RecordedPov | Self::VictimPov | Self::VictimBlade => StudioFollowPerspective::FirstPerson,
            _ => StudioFollowPerspective::Orbit,
        }
    }
    /// Default rig for a new shot (or when the preset is switched). World
    /// shots also get an orbit rig: it is their fallback until placed.
    pub fn seed(self, rig: &mut StudioDemoFollowRig) {
        let fallback = rig.fallback;
        *rig = StudioDemoFollowRig { fallback, ..StudioDemoFollowRig::default() };
        rig.perspective = self.perspective();
        rig.orbit_collision = true;
        match self {
            Self::Chase => { rig.range = 120.0; rig.vertical_offset = 16.0; rig.pitch_offset = 10.0; }
            Self::Orbit => { rig.range = 160.0; rig.yaw_offset = -45.0; rig.pitch_offset = 15.0; }
            Self::PlayerEyes | Self::RecordedPov => {}
            Self::Tracking => { rig.range = 210.0; rig.yaw_offset = -90.0; rig.pitch_offset = 6.0; rig.vertical_offset = 0.0; }
            Self::CloseUp => {
                rig.range = 60.0; rig.yaw_offset = 165.0; rig.pitch_offset = 4.0; rig.vertical_offset = 0.0;
                rig.aim_enabled = true; rig.aim_anchor = StudioDemoAnchor::Face;
            }
            Self::LowAngle => {
                rig.range = 170.0; rig.yaw_offset = 150.0; rig.pitch_offset = -4.0; rig.vertical_offset = 0.0;
                rig.aim_enabled = true; rig.aim_anchor = StudioDemoAnchor::HeadTop;
            }
            Self::Overhead => {
                rig.range = 440.0; rig.yaw_offset = 0.0; rig.pitch_offset = 70.0; rig.vertical_offset = 0.0;
                rig.orbit_world_angles = true;
            }
            Self::PushIn => { rig.range = 260.0; rig.yaw_offset = 140.0; rig.pitch_offset = 8.0; rig.vertical_offset = 0.0; }
            Self::DollyZoom => { rig.range = 320.0; rig.yaw_offset = 180.0; rig.pitch_offset = 4.0; rig.vertical_offset = 0.0; }
            Self::Establishing => { rig.range = 640.0; rig.yaw_offset = -35.0; rig.pitch_offset = 28.0; rig.vertical_offset = 0.0; }
            Self::OverShoulder => {
                rig.range = 80.0; rig.vertical_offset = 8.0; rig.horizontal_offset = 22.0; rig.pitch_offset = 4.0;
                rig.aim_enabled = true; rig.aim_anchor = StudioDemoAnchor::Chest;
            }
            Self::TwoShot => { rig.range = 240.0; rig.yaw_offset = 90.0; rig.pitch_offset = 8.0; rig.vertical_offset = 0.0; }
            Self::VictimPov => {
                rig.aim_enabled = true; rig.aim_anchor = StudioDemoAnchor::SaberTip;
            }
            Self::VictimBlade => {
                // In the victim's own eyes (never moved out of the hidden
                // body), watching the swing come in from the attacker's hilt.
                rig.aim_enabled = true; rig.aim_anchor = StudioDemoAnchor::SaberHilt;
            }
            Self::SaberCam => {
                // Orbit perspective keeps the body visible; the camera sits
                // behind the tip on the line from the opponent, a little
                // higher, so the blade tip stays in the lower foreground.
                rig.position_anchor = StudioDemoAnchor::SaberTip;
                rig.range = 30.0; rig.yaw_offset = 0.0; rig.pitch_offset = 6.0; rig.vertical_offset = 8.0;
                rig.orbit_collision = false; rig.chase_aim = true;
                rig.aim_enabled = true; rig.aim_anchor = StudioDemoAnchor::Chest;
            }
            Self::FollowCam => { rig.range = 170.0; rig.vertical_offset = 60.0; rig.pitch_offset = 12.0; rig.autofocus = true; }
            Self::ThrowCam => {
                rig.position_anchor = StudioDemoAnchor::SaberHilt;
                rig.range = 64.0; rig.pitch_offset = 8.0; rig.vertical_offset = 14.0;
                rig.orbit_collision = true; rig.chase_aim = true;
                rig.aim_enabled = true; rig.aim_anchor = StudioDemoAnchor::Chest;
            }
            Self::Static | Self::Pan | Self::FlyBy | Self::Crane | Self::Glide => {
                rig.range = 220.0; rig.yaw_offset = -60.0; rig.pitch_offset = 10.0; rig.vertical_offset = 0.0;
            }
        }
        if self.world_anchored() {
            rig.aim_enabled = true;
            rig.aim_anchor = StudioDemoAnchor::Chest;
        }
        rig.autofocus = rig.aim_enabled || matches!(self, Self::Tracking | Self::PushIn | Self::DollyZoom | Self::Orbit);
    }
    /// Default lens and motion for a shot of this type. `base_fov` is the
    /// player's cg_fov, kept for the first-person and chase shots.
    pub fn seed_shot(self, shot: &mut StudioDirectorShot, base_fov: f32) {
        self.seed(&mut shot.rig);
        shot.preset = self;
        shot.sweep = 0.0;
        shot.range_end = None;
        shot.fov_end = None;
        shot.zoom_lock = false;
        shot.position_override = None;
        shot.position_end = None;
        shot.position_mid = None;
        shot.focus_end = None;
        shot.path_keys.clear();
        shot.focus_target = None;
        let (fov, aperture) = match self {
            Self::Chase | Self::PlayerEyes | Self::RecordedPov => (base_fov, None),
            Self::Orbit => (65.0, Some(4.0)),
            Self::Tracking => (58.0, Some(4.0)),
            Self::CloseUp => (32.0, Some(1.8)),
            Self::LowAngle => (68.0, Some(2.8)),
            Self::Overhead => (72.0, None),
            Self::PushIn => (55.0, Some(2.8)),
            Self::DollyZoom => (32.0, Some(4.0)),
            Self::Establishing => (88.0, None),
            // Focus sits on the framed action point.
            Self::Static => (70.0, Some(4.0)),
            // Focus follows the panned-to player every frame.
            Self::Pan => (58.0, Some(2.8)),
            Self::FlyBy => (85.0, None),
            Self::Crane => (70.0, Some(4.0)),
            Self::Glide => (68.0, Some(4.0)),
            // Focus on the opponent, the near shoulder soft.
            Self::OverShoulder => (55.0, Some(2.8)),
            // Two people at different depths: a gentler aperture.
            Self::TwoShot => (66.0, Some(5.6)),
            Self::SaberCam => (84.0, None),
            Self::VictimPov => (90.0, None),
            Self::VictimBlade => (86.0, None),
            Self::ThrowCam => (78.0, None),
            Self::FollowCam => (64.0, Some(4.0)),
        };
        shot.fov = fov;
        shot.lens.aperture = aperture;
        shot.lens.focus_distance = shot.rig.range.max(16.0);
        match self {
            Self::Orbit => shot.sweep = 90.0,
            Self::PushIn => { shot.range_end = Some(120.0); shot.fov_end = Some(42.0); }
            Self::DollyZoom => { shot.range_end = Some(100.0); shot.zoom_lock = true; }
            Self::Establishing => shot.sweep = 20.0,
            _ => {}
        }
    }
}

fn yes() -> bool { true }
fn default_fov() -> f32 { 80.0 }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StudioDirectorShot {
    #[serde(default, skip_serializing_if="Option::is_none")]
    pub comparison_label: Option<String>,
    #[serde(default, skip_serializing_if="Option::is_none")]
    pub comparison_camera: Option<super::render_studio_race::RaceCameraEdit>,
    #[serde(default)] pub lens: crate::lens::StudioLens,
    #[serde(default,skip_serializing_if="Option::is_none")] pub focus_target: Option<[f32;3]>,
    /// World camera position (JKA space, Z up): placed shots, or a dragged handle.
    #[serde(default,skip_serializing_if="Option::is_none")] pub position_override: Option<[f32;3]>,
    /// World camera position at the end of the shot (crane, drift).
    #[serde(default,skip_serializing_if="Option::is_none")] pub position_end: Option<[f32;3]>,
    /// Curve control point between position_override and position_end.
    #[serde(default,skip_serializing_if="Option::is_none")] pub position_mid: Option<[f32;3]>,
    /// Look-at point at the end of the shot (focus_target glides to it).
    #[serde(default,skip_serializing_if="Option::is_none")] pub focus_end: Option<[f32;3]>,
    /// Stable identity for selection and regeneration.
    #[serde(default)] pub id: u64,
    #[serde(default = "yes")] pub enabled: bool,
    pub preset: StudioDirectorPreset,
    /// Output-timeline seconds (not demo source time).
    pub start: f64,
    pub duration: f64,
    /// Followed demo client; -1 = the recorded player (follows the POV).
    pub client: i32,
    #[serde(default = "default_fov")] pub fov: f32,
    #[serde(default)] pub rig: StudioDemoFollowRig,
    /// Orbit/chase: degrees added to the rig yaw across the shot.
    #[serde(default)] pub sweep: f32,
    /// Ease the shot's motion in and out (smoothstep over normalized time).
    #[serde(default = "yes")] pub ease: bool,
    /// Rig distance at the end of the shot (push-in / pull-out).
    #[serde(default,skip_serializing_if="Option::is_none")] pub range_end: Option<f32>,
    /// Field of view at the end of the shot (zoom).
    #[serde(default,skip_serializing_if="Option::is_none")] pub fov_end: Option<f32>,
    /// Dolly zoom: the lens follows the distance so the subject keeps its size.
    #[serde(default)] pub zoom_lock: bool,
    /// Variation counter: Regenerate advances it for a different decision.
    #[serde(default)] pub seed: u32,
    /// Made by the Auto Director (from the subject's recorded path).
    #[serde(default)] pub auto: bool,
    /// Over animation footage (no demo): the character track it follows.
    #[serde(default)] pub actor_lane: usize,
    /// Planned camera path (Follow cam): position + look point keys over
    /// normalized shot time, played through a Catmull-Rom spline.
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub path_keys: Vec<StudioPathKey>,
}

/// One key of a planned camera path (JKA space, Z up).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StudioPathKey {
    /// 0..1 across the shot.
    pub t: f32,
    pub position: [f32; 3],
    pub look: [f32; 3],
}

impl StudioDirectorShot {
    pub fn new(id: u64, preset: StudioDirectorPreset, client: i32, start: f64, duration: f64) -> Self {
        let mut shot = Self { comparison_label: None, comparison_camera: None, focus_target: None, position_override: None, position_end: None, position_mid: None, focus_end: None, lens: Default::default(),
            id, enabled: true, preset, start, duration, client, fov: default_fov(), rig: StudioDemoFollowRig::default(),
            sweep: 0.0, ease: true, range_end: None, fov_end: None, zoom_lock: false, seed: 0, auto: false, actor_lane: 0, path_keys: Vec::new() };
        preset.seed(&mut shot.rig);
        if preset == StudioDirectorPreset::Orbit { shot.sweep = 90.0; }
        shot
    }
    pub fn end(&self) -> f64 { self.start + self.duration }
    pub fn contains(&self, time: f64) -> bool { time >= self.start && time < self.end() }
    pub fn valid(&self) -> bool {
        let world = |p: &Option<[f32; 3]>| p.is_none_or(|p| p.iter().all(|v| v.is_finite() && v.abs() <= 50000.0));
        self.comparison_camera.is_none_or(|e| e.valid())
            && self.focus_target.is_none_or(|p|p.iter().all(|v|v.is_finite()&&v.abs()<=1_000_000.0))
            && world(&self.position_override) && world(&self.position_end) && world(&self.position_mid)
            && self.focus_end.is_none_or(|p|p.iter().all(|v|v.is_finite()&&v.abs()<=1_000_000.0))
            && self.lens.valid() && self.start.is_finite() && self.duration.is_finite() && self.start >= 0.0
            // Generated cuts must preserve even a very short teleport or
            // subject boundary (also after source-time speed ramps).
            && self.duration > 0.0 && self.duration <= 36000.0
            && (self.auto || self.duration >= 0.05) && self.end() <= 36000.0
            && (-1..=1023).contains(&self.client)
            && self.fov.is_finite() && (10.0..=140.0).contains(&self.fov)
            && self.fov_end.is_none_or(|f| f.is_finite() && (10.0..=140.0).contains(&f))
            && self.range_end.is_none_or(|r| r.is_finite() && (0.0..=2048.0).contains(&r))
            && self.sweep.is_finite() && self.sweep.abs() <= 3600.0
            && self.actor_lane < 64
            && self.path_keys.len() <= 4096
            && self.path_keys.iter().all(|k| k.t.is_finite() && (0.0..=1.0).contains(&k.t)
                && k.position.iter().chain(&k.look).all(|v| v.is_finite() && v.abs() <= 1_000_000.0))
            && self.rig.valid()
    }
    /// Planned path sample at normalized time `u`: Catmull-Rom through the
    /// position keys, the same through the look keys.
    pub fn path_at(&self, u: f32) -> Option<([f32; 3], [f32; 3])> {
        let keys = &self.path_keys;
        match keys.len() {
            0 => return None,
            1 => return Some((keys[0].position, keys[0].look)),
            _ => {}
        }
        let n = keys.len();
        let i = keys.partition_point(|k| k.t <= u).clamp(1, n - 1) - 1;
        let (a, b) = (keys[i], keys[i + 1]);
        let s = ((u - a.t) / (b.t - a.t).max(1e-6)).clamp(0.0, 1.0);
        let (p0, p3) = (keys[i.saturating_sub(1)], keys[(i + 2).min(n - 1)]);
        let spline = |p0: [f32; 3], p1: [f32; 3], p2: [f32; 3], p3: [f32; 3]| -> [f32; 3] {
            std::array::from_fn(|k| 0.5 * (2.0 * p1[k] + (p2[k] - p0[k]) * s
                + (2.0 * p0[k] - 5.0 * p1[k] + 4.0 * p2[k] - p3[k]) * s * s
                + (3.0 * p1[k] - p0[k] - 3.0 * p2[k] + p3[k]) * s * s * s))
        };
        Some((spline(p0.position, a.position, b.position, p3.position), spline(p0.look, a.look, b.look, p3.look)))
    }
    /// Eased normalized shot time at `time` (pure).
    pub fn progress(&self, time: f64) -> f32 {
        let u = ((time - self.start) / self.duration.max(1e-6)).clamp(0.0, 1.0) as f32;
        if self.ease { u * u * (3.0 - 2.0 * u) } else { u }
    }
    /// Orbit yaw at `time`, from normalized shot time only (pure).
    pub fn yaw_at(&self, time: f64) -> f32 {
        (self.rig.yaw_offset + self.sweep * self.progress(time) + 180.0).rem_euclid(360.0) - 180.0
    }
    pub fn range_at(&self, e: f32) -> f32 {
        self.range_end.map_or(self.rig.range, |end| self.rig.range + (end - self.rig.range) * e)
    }
    pub fn fov_at(&self, e: f32) -> f32 {
        if self.zoom_lock && self.range_end.is_some() {
            let (from, now) = (self.rig.range.max(1.0), self.range_at(e).max(1.0));
            let half = (self.fov.to_radians() * 0.5).tan() * from / now;
            return (2.0 * half.atan()).to_degrees().clamp(10.0, 140.0);
        }
        self.fov_end.map_or(self.fov, |end| self.fov + (end - self.fov) * e)
    }
    /// The transient camera key this shot evaluates to at `time`. Position /
    /// yaw / pitch of `base` are kept for a keyed-camera fallback.
    pub fn camera_key(&self, time: f64, base: StudioCameraKey) -> StudioCameraKey {
        let e = self.progress(time);
        let mut key = base;
        key.time = time;
        key.rig_orbit = None;
        key.fov = self.fov_at(e);
        key.lens = self.lens;
        key.focus_target = match (self.focus_target, self.focus_end) {
            (Some(a), Some(b)) => Some(std::array::from_fn(|k| a[k] + (b[k] - a[k]) * e)),
            (a, _) => a,
        };
        // Planned path: a smooth, pre-damped camera move (no live tracking).
        if let Some((position, look)) = self.path_at(((time - self.start) / self.duration.max(1e-6)).clamp(0.0, 1.0) as f32) {
            key.position = position;
            key.focus_target = Some(look);
            key.source = StudioCameraSource::Demo { pov: StudioDemoPov::Free, client: self.client };
            let mut rig = self.rig;
            rig.aim_enabled = false;
            rig.blend_seconds = 0.0;
            key.demo_follow = rig;
            key.resolve_focus_target();
            return key;
        }
        if self.preset == StudioDirectorPreset::RecordedPov {
            key.source = StudioCameraSource::Demo { pov: StudioDemoPov::Recorded, client: 0 };
            key.demo_follow.perspective = StudioFollowPerspective::FirstPerson;
            if let Some(position)=self.position_override {
                key.position=position;key.source=StudioCameraSource::Demo{pov:StudioDemoPov::Free,client:self.client};
            }
            key.resolve_focus_target();return key;
        }
        let mut rig = self.rig;
        // A live shot is a cut; keyed follow moves don't blend into it.
        rig.blend_seconds = 0.0;
        rig.perspective = self.preset.perspective();
        rig.range = self.range_at(e);
        if rig.perspective != StudioFollowPerspective::FirstPerson && self.sweep != 0.0 {
            rig.yaw_offset = self.yaw_at(time);
        }
        key.source = StudioCameraSource::Demo { pov: StudioDemoPov::Follow, client: self.client };
        key.demo_follow = rig;
        if let Some(from)=self.position_override {
            let to = self.position_end.unwrap_or(from);
            key.position = match self.position_mid {
                // Quadratic curve through the control point.
                Some(mid) => std::array::from_fn(|k| (1.0 - e) * (1.0 - e) * from[k] + 2.0 * (1.0 - e) * e * mid[k] + e * e * to[k]),
                None => std::array::from_fn(|k| from[k] + (to[k] - from[k]) * e),
            };
            key.source=StudioCameraSource::Demo{pov:StudioDemoPov::Free,client:self.client};
        }
        key.resolve_focus_target();key
    }
}

impl StudioTimeline {
    /// Active shot at `time`: only where demo footage plays; on overlap the
    /// shot latest in the list (front-most) wins.
    pub fn director_at(&self, time: f64) -> Option<&StudioDirectorShot> {
        self.demo_at(time)?;
        self.director_shots.iter().rev().find(|shot| shot.enabled && shot.contains(time))
    }
    /// Camera for the native demo sampler: keyed camera, overridden by a
    /// live Director shot where one is active.
    pub fn effective_demo_camera_at(&self, time: f64) -> StudioCameraKey {
        let base = self.camera_at(time);
        self.director_at(time).map_or(base, |shot| shot.camera_key(time, base))
    }
    pub fn next_director_id(&self) -> u64 {
        self.director_shots.iter().map(|s| s.id).max().map_or(1, |id| id + 1)
    }
    /// Footage under `time` a shot can cover: (start, end, character track).
    /// Demo footage first; else an animation clip, preferring `lane`.
    pub fn footage_at(&self, time: f64, lane: Option<usize>) -> Option<(f64, f64, Option<usize>)> {
        if let Some(demo) = self.demo_at(time) { return Some((demo.start, demo.end(), None)); }
        let clip = lane.and_then(|lane| self.clip_at_lane(time, lane))
            .or_else(|| self.active_actors(time).into_iter().next())?;
        Some((clip.start, clip.end(), Some(clip.lane)))
    }
    /// New shot at the playhead, up to four seconds, clamped to the demo
    /// (or animation) clip under it. Returns its index.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn add_director_shot(&mut self, preset: StudioDirectorPreset, client: i32, time: f64) -> Option<usize> {
        self.add_director_shot_on(preset, client, time, None)
    }
    pub fn add_director_shot_on(&mut self, preset: StudioDirectorPreset, client: i32, time: f64, lane: Option<usize>)
        -> Option<usize> {
        let (_, end, actor) = self.footage_at(time, lane)?;
        let duration = (end - time).min(4.0);
        if duration < 0.1 { return None; }
        let id = self.next_director_id();
        let mut shot = StudioDirectorShot::new(id, preset, client.clamp(-1, 1023), time, duration);
        shot.actor_lane = actor.unwrap_or(0);
        self.director_shots.push(shot);
        Some(self.director_shots.len() - 1)
    }

    // ---- Character stage (animation footage, no demo) ---------------------

    /// Active shot over animation footage: no demo at `time`, and its
    /// character track has a clip there. Later shots win overlaps.
    pub fn stage_director_at(&self, time: f64) -> Option<&StudioDirectorShot> {
        if self.demo_at(time).is_some() { return None; }
        self.director_shots.iter().rev()
            .find(|shot| shot.enabled && shot.contains(time) && self.clip_at_lane(time, shot.actor_lane).is_some())
    }
    /// Actor on `lane` at `time`: stage origin (JKA space, Z up) and
    /// facing yaw (degrees), including smoothed transitions.
    pub fn stage_actor_at(&self, lane: usize, time: f64, zoom: f32, base_yaw: f32) -> Option<(glam::Vec3, f32)> {
        if !self.lane_is_visible(lane) { return None; }
        let index = self.clips.iter().position(|c| c.lane == lane && time >= c.start && time < c.end())?;
        let motion = self.blended_motion_at(index, time,
            self.interpolation != super::render_studio_timeline::StudioInterpolation::Linear);
        Some((glam::Vec3::from_array(super::render_studio_spatial::motion_origin(motion.position, lane, zoom)),
            base_yaw + motion.yaw))
    }
    /// Stage camera of the active Director shot (None: keyed camera applies).
    /// Pure in `time`, like the demo path.
    pub fn stage_camera_at(&self, time: f64, zoom: f32, base_yaw: f32) -> Option<StudioCameraKey> {
        let shot = self.stage_director_at(time)?;
        let base = self.camera_at(time);
        let mut key = shot.camera_key(time, base);
        if shot.preset == StudioDirectorPreset::RecordedPov { return Some(key); }
        let (origin, yaw) = self.stage_actor_at(shot.actor_lane, time, zoom, base_yaw)?;
        let rig = shot.rig;
        let point = |anchor: StudioDemoAnchor, offset: [f32; 3], local: bool| {
            let delta = glam::Vec3::from_array(offset);
            let delta = if local {
                let (sn, cs) = yaw.to_radians().sin_cos();
                glam::Vec3::new(cs * delta.x - sn * delta.y, sn * delta.x + cs * delta.y, delta.z)
            } else { delta };
            origin + glam::Vec3::Z * stage_anchor_height(anchor) + delta
        };
        let anchor = point(rig.position_anchor, rig.position_offset, rig.position_local);
        let aim = if rig.aim_enabled { point(rig.aim_anchor, rig.aim_offset, rig.aim_local) } else { anchor };
        let e = shot.progress(time);
        let camera = if shot.position_override.is_some() {
            let position = glam::Vec3::from_array(key.position);
            if shot.focus_target.is_none() && rig.aim_enabled { look_at(&mut key, aim); }
            position
        } else {
            let range = shot.range_at(e);
            let yaw_offset = if shot.sweep != 0.0 { shot.yaw_at(time) } else { rig.yaw_offset };
            match shot.preset.perspective() {
                StudioFollowPerspective::FirstPerson => {
                    key.position = anchor.to_array();
                    key.yaw = yaw + yaw_offset;
                    key.pitch = rig.pitch_offset;
                    key.resolve_focus_target();
                    anchor
                }
                StudioFollowPerspective::ThirdPerson => {
                    let heading = if rig.motion_direction {
                        self.stage_actor_at(shot.actor_lane, (time - 0.15).max(0.0), zoom, base_yaw)
                            .map(|(before, _)| origin - before).filter(|d| d.x.hypot(d.y) > 0.5)
                            .map_or(yaw, |d| d.y.atan2(d.x).to_degrees())
                    } else { yaw + yaw_offset };
                    let position = anchor - stage_forward(heading, rig.pitch_offset) * range
                        + glam::Vec3::Z * rig.vertical_offset;
                    key.position = position.to_array();
                    look_at(&mut key, aim);
                    position
                }
                StudioFollowPerspective::Orbit => {
                    let orbit_yaw = if rig.orbit_world_angles { yaw_offset } else { yaw + yaw_offset };
                    let r = orbit_yaw.to_radians();
                    let right = glam::Vec3::new(r.sin(), -r.cos(), 0.0);
                    let position = anchor - stage_forward(orbit_yaw, rig.pitch_offset) * range
                        + glam::Vec3::Z * rig.vertical_offset + right * rig.horizontal_offset;
                    key.position = position.to_array();
                    look_at(&mut key, aim);
                    position
                }
            }
        };
        // A pinned look-at target wins over the rig's aim (and sets focus).
        if shot.focus_target.is_some() { key.resolve_focus_target(); }
        if rig.autofocus && shot.focus_target.is_none() {
            key.lens.focus_distance = camera.distance(aim).clamp(1.0, 65536.0);
        }
        Some(key)
    }
    /// Give legacy / hand-edited shots unique ids (loaded projects).
    pub fn normalize_director_ids(&mut self) {
        let mut seen = std::collections::BTreeSet::new();
        let mut next = self.director_shots.iter().map(|s| s.id).max().unwrap_or(0) + 1;
        for shot in &mut self.director_shots {
            if shot.id == 0 || !seen.insert(shot.id) {
                shot.id = next;
                seen.insert(next);
                next += 1;
            }
        }
    }
}

/// Stage height of a body point above the actor origin (no bone sampling on
/// the isolated stage: JKA humanoid proportions, viewheight 36).
pub fn stage_anchor_height(anchor: StudioDemoAnchor) -> f32 {
    match anchor {
        StudioDemoAnchor::Eyes | StudioDemoAnchor::Face => 36.0,
        StudioDemoAnchor::HeadTop => 44.0,
        StudioDemoAnchor::Chest => 20.0,
        StudioDemoAnchor::Pelvis => 2.0,
        StudioDemoAnchor::Origin => 0.0,
        StudioDemoAnchor::RightHand | StudioDemoAnchor::LeftHand => 10.0,
        StudioDemoAnchor::SaberTip => 30.0,
        StudioDemoAnchor::SaberHilt => 20.0,
    }
}

/// JKA-space forward for a JKA yaw/pitch (positive pitch looks down),
/// the same convention as the demo sampler's orbit rig.
pub fn stage_forward(yaw: f32, pitch: f32) -> glam::Vec3 {
    let (sy, cy) = yaw.to_radians().sin_cos();
    let (sp, cp) = pitch.to_radians().sin_cos();
    glam::Vec3::new(cy * cp, sy * cp, -sp)
}

/// Point a camera key at `target` (JKA space).
pub fn look_at(key: &mut StudioCameraKey, target: glam::Vec3) {
    let delta = target - glam::Vec3::from_array(key.position);
    if delta.length_squared() > 1e-6 {
        key.yaw = delta.y.atan2(delta.x).to_degrees();
        key.pitch = (-delta.z).atan2(delta.x.hypot(delta.y)).to_degrees().clamp(-89.0, 89.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_studio_timeline::StudioDemoClip;

    fn timeline() -> StudioTimeline {
        let mut tl = StudioTimeline::default();
        tl.demos.push(StudioDemoClip { demo_protocol:26, source_path: None, comparison: None, race_overlays: Vec::new(), pov_trail:Default::default(), hud_overrides:Default::default(), hide_other_players:false, name: "a".into(), game: String::new(), start: 0.0, duration_ms: 20_000,
            in_ms: 0, out_ms: 20_000, speed_keys: vec![], transition_in: Default::default(), audio: Default::default(), layer: 0 });
        tl
    }

    #[test]
    fn orbit_pose_depends_only_on_output_time() {
        let mut tl = timeline();
        let i = tl.add_director_shot(StudioDirectorPreset::Orbit, 3, 5.0).unwrap();
        tl.director_shots[i].sweep = 360.0;
        let order = [7.7, 5.2, 8.9, 7.7, 5.2];
        let yaws: Vec<f32> = order.iter().map(|&t| tl.effective_demo_camera_at(t).demo_follow.yaw_offset).collect();
        assert_eq!(yaws[0], yaws[3]);
        assert_eq!(yaws[1], yaws[4]);
        assert_eq!(tl.effective_demo_camera_at(6.0).source, StudioCameraSource::Demo { pov: StudioDemoPov::Follow, client: 3 });
    }

    #[test]
    fn interval_is_half_open_and_keys_return_after() {
        let mut tl = timeline();
        tl.add_director_shot(StudioDirectorPreset::Chase, 1, 5.0).unwrap();
        assert!(tl.director_at(4.999).is_none());
        assert!(tl.director_at(5.0).is_some());
        assert!(tl.director_at(8.999).is_some());
        assert!(tl.director_at(9.0).is_none());
        assert_eq!(format!("{:?}", tl.effective_demo_camera_at(9.0)), format!("{:?}", tl.camera_at(9.0)));
    }

    #[test]
    fn front_most_enabled_shot_wins_overlaps() {
        let mut tl = timeline();
        tl.add_director_shot(StudioDirectorPreset::Chase, 1, 2.0).unwrap();
        let top = tl.add_director_shot(StudioDirectorPreset::Orbit, 2, 3.0).unwrap();
        assert_eq!(tl.director_at(3.5).unwrap().client, 2);
        tl.director_shots[top].enabled = false;
        assert_eq!(tl.director_at(3.5).unwrap().client, 1);
    }

    #[test]
    fn shots_need_demo_footage_and_clamp_to_the_clip() {
        let mut tl = timeline();
        assert!(tl.add_director_shot(StudioDirectorPreset::Chase, 0, 25.0).is_none());
        let i = tl.add_director_shot(StudioDirectorPreset::Chase, 0, 18.5).unwrap();
        assert!((tl.director_shots[i].duration - 1.5).abs() < 1e-9);
        assert_ne!(tl.director_shots[0].id, 0);
    }

    #[test]
    fn json_round_trip_and_legacy_defaults() {
        let shot = StudioDirectorShot::new(4, StudioDirectorPreset::Orbit, 7, 1.0, 2.0);
        let text = serde_json::to_string(&shot).unwrap();
        assert_eq!(serde_json::from_str::<StudioDirectorShot>(&text).unwrap(), shot);
        let legacy: StudioDirectorShot = serde_json::from_str(
            r#"{"preset":"Chase","start":1.0,"duration":2.0,"client":3}"#).unwrap();
        assert!(legacy.enabled && legacy.valid() && legacy.id == 0);
    }
}
