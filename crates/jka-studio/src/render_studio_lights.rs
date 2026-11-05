//! Recording-independent, deterministic light rigs on the output timeline.
//! Conversion to renderer lights stays in the client.
use serde::{Deserialize, Serialize};

pub const MAX_LIGHTS: usize = 32;
pub const MAX_LAYERS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StudioLightKind { Point, Spot }
impl StudioLightKind {
    pub fn label(self) -> &'static str { match self { Self::Point => "Point", Self::Spot => "Spot" } }
}

// Render Studio light emission is a per-light timeline modifier, not a renderer
// light shape. These waveforms follow JKA/Quake's genFunc family (sin, triangle,
// square, sawtooth, inversesawtooth, noise); strobe duty and stepped flicker are
// editor conveniences. Never integrate phase frame-by-frame: seeking and offline
// export must give the same result for the same output timestamp.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StudioLightEmitterKind {
    #[default]
    Continuous,
    Pulse,
    Triangle,
    Strobe,
    Sawtooth,
    InverseSawtooth,
    Noise,
    Flicker,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StudioLightEmitter {
    pub kind: StudioLightEmitterKind,
    /// Cycles/second, relative to the light's start time.
    pub frequency: f32,
    /// Fraction of the cycle offset, corresponding to JKA's waveform phase.
    pub phase: f32,
    /// Minimum output fraction, multiplied by the keyed intensity.
    pub floor: f32,
    /// Illuminated fraction per strobe cycle (square wave otherwise uses 50%).
    pub duty_cycle: f32,
}
impl Default for StudioLightEmitter {
    fn default() -> Self { Self { kind: StudioLightEmitterKind::Continuous,
        frequency: 1.0, phase: 0.0, floor: 0.0, duty_cycle: 0.5 } }
}
impl StudioLightEmitter {
    pub fn valid(self) -> bool {
        self.frequency.is_finite() && (0.05..=60.0).contains(&self.frequency)
            && self.phase.is_finite() && (0.0..=1.0).contains(&self.phase)
            && self.floor.is_finite() && (0.0..=1.0).contains(&self.floor)
            && self.duty_cycle.is_finite() && (0.01..=0.99).contains(&self.duty_cycle)
    }

    /// 0..1 multiplier at absolute output time. Timeline-local evaluation makes
    /// this deterministic in interactive preview, reverse scrubs and export.
    pub fn level(self, elapsed: f64) -> f32 {
        if self.kind == StudioLightEmitterKind::Continuous { return 1.0; }
        let x = elapsed * f64::from(self.frequency) + f64::from(self.phase);
        let cycle = x.rem_euclid(1.0) as f32;
        let wave = match self.kind {
            StudioLightEmitterKind::Continuous => 1.0,
            StudioLightEmitterKind::Pulse => (1.0 + (std::f32::consts::TAU * cycle).sin()) * 0.5,
            // The JKA triangle waveform starts at zero, reaches +1 at 1/4,
            // -1 at 3/4. Map its signed amplitude into light output 0..1.
            StudioLightEmitterKind::Triangle => {
                let signed = if cycle < 0.25 { cycle * 4.0 }
                    else if cycle < 0.75 { 2.0 - cycle * 4.0 }
                    else { cycle * 4.0 - 4.0 };
                (signed + 1.0) * 0.5
            }
            StudioLightEmitterKind::Strobe => if cycle < self.duty_cycle { 1.0 } else { 0.0 },
            StudioLightEmitterKind::Sawtooth => cycle,
            StudioLightEmitterKind::InverseSawtooth => 1.0 - cycle,
            StudioLightEmitterKind::Noise | StudioLightEmitterKind::Flicker => {
                // Stable value noise (smooth) / sample-and-hold (stepped), not
                // global random state. Both are reproducible in offline export.
                fn hash(n: i64) -> f32 {
                    let mut v = n as u64;
                    v ^= v >> 30; v = v.wrapping_mul(0xbf58476d1ce4e5b9);
                    v ^= v >> 27; v = v.wrapping_mul(0x94d049bb133111eb);
                    v ^= v >> 31;
                    (v >> 40) as f32 / ((1u32 << 24) - 1) as f32
                }
                let step = x.floor() as i64;
                let a = hash(step);
                if self.kind == StudioLightEmitterKind::Flicker { a }
                else { let t = cycle * cycle * (3.0 - 2.0 * cycle);
                    a + (hash(step.saturating_add(1)) - a) * t }
            }
        };
        self.floor + (1.0 - self.floor) * wave.clamp(0.0, 1.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StudioLightKey {
    /// Local output seconds from the light's start; independent of demo speed.
    pub time: f64,
    /// JKA coordinates, shared with the camera and motion inspectors.
    pub position: [f32; 3],
    pub target: [f32; 3],
    pub color: [f32; 3],
    pub intensity: f32,
    pub radius: f32,
    pub source_radius: f32,
    /// Cone half angles, degrees.
    pub inner_angle: f32,
    pub outer_angle: f32,
}
impl Default for StudioLightKey {
    fn default() -> Self { Self { time: 0.0, position: [64.0, -64.0, 96.0], target: [0.0, 0.0, 40.0],
        color: [1.0; 3], intensity: 2.0, radius: 350.0, source_radius: 0.0, inner_angle: 20.0, outer_angle: 35.0 } }
}
pub const SPOT_HANDLE_LENGTH:f32=64.0;
impl StudioLightKey {
    pub fn aim_handle(self)->[f32;3] {
        let eye=glam::Vec3::from_array(self.position);
        let direction=(glam::Vec3::from_array(self.target)-eye).try_normalize().unwrap_or(glam::Vec3::X);
        (eye+direction*SPOT_HANDLE_LENGTH).to_array()
    }
    pub fn move_position(&mut self,position:[f32;3]) {
        let direction=glam::Vec3::from_array(self.aim_handle())-glam::Vec3::from_array(self.position);
        self.position=position;self.target=(glam::Vec3::from_array(position)+direction).to_array();
    }
    pub fn aim_at(&mut self,point:[f32;3]) {
        let direction=glam::Vec3::from_array(point)-glam::Vec3::from_array(self.position);
        if let Some(direction)=direction.try_normalize(){self.target=(glam::Vec3::from_array(self.position)+direction*SPOT_HANDLE_LENGTH).to_array();}
    }
    pub fn valid(&self) -> bool {
        self.time.is_finite() && (0.0..=36000.0).contains(&self.time)
            && self.position.iter().chain(&self.target).all(|v| v.is_finite() && v.abs() <= 1_000_000.0)
            && self.color.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v))
            && self.intensity.is_finite() && (0.0..=64.0).contains(&self.intensity)
            && self.radius.is_finite() && (1.0..=65536.0).contains(&self.radius)
            && self.source_radius.is_finite() && (0.0..=self.radius).contains(&self.source_radius)
            && self.inner_angle.is_finite() && self.outer_angle.is_finite()
            && (1.0..=89.0).contains(&self.outer_angle) && (0.0..=self.outer_angle).contains(&self.inner_angle)
    }
    fn lerp(self, other: Self, t: f32) -> Self {
        let scalar = |a: f32, b: f32| a + (b - a) * t;
        let vector = |a: [f32; 3], b: [f32; 3]| std::array::from_fn(|i| scalar(a[i], b[i]));
        Self { time: self.time + (other.time - self.time) * f64::from(t),
            position: vector(self.position, other.position), target: vector(self.target, other.target),
            color: vector(self.color, other.color), intensity: scalar(self.intensity, other.intensity),
            radius: scalar(self.radius, other.radius), source_radius: scalar(self.source_radius, other.source_radius),
            inner_angle: scalar(self.inner_angle, other.inner_angle), outer_angle: scalar(self.outer_angle, other.outer_angle) }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StudioLight {
    pub name: String,
    pub kind: StudioLightKind,
    /// Light emission waveform; missing in older projects => continuous.
    #[serde(default)]
    pub emitter: StudioLightEmitter,
    pub enabled: bool,
    pub start: f64,
    pub duration: f64,
    pub keys: Vec<StudioLightKey>,
}
impl StudioLight {
    pub fn end(&self) -> f64 { self.start + self.duration }
    pub fn valid(&self) -> bool {
        self.name.len() <= 128 && self.emitter.valid() && self.start.is_finite() && self.start >= 0.0
            && self.duration.is_finite() && self.duration >= 0.01 && self.end() <= 36000.0
            && !self.keys.is_empty() && self.keys.len() <= 256 && self.keys.iter().all(StudioLightKey::valid)
            && self.keys.windows(2).all(|p| p[0].time + 0.000001 < p[1].time)
    }
    pub fn sample(&self, time: f64, smooth: bool) -> StudioLightKey {
        let local = (time - self.start).clamp(0.0, self.duration);
        let mut sampled = self.keys.first().copied().unwrap_or_default();
        if local > sampled.time {
            sampled = *self.keys.last().unwrap_or(&sampled);
            for pair in self.keys.windows(2) {
                if local <= pair[1].time {
                    let mut t = ((local - pair[0].time) / (pair[1].time - pair[0].time).max(0.000001)).clamp(0.0, 1.0) as f32;
                    if smooth { t = t * t * (3.0 - 2.0 * t); }
                    sampled = pair[0].lerp(pair[1], t);
                    break;
                }
            }
        }
        sampled.time = local;
        sampled
    }
    pub fn add_key(&mut self, key: StudioLightKey) -> Option<usize> {
        if !key.valid() { return None; }
        if let Some(i) = self.keys.iter().position(|k| (k.time - key.time).abs() < 0.00001) {
            self.keys[i] = key; return Some(i);
        }
        if self.keys.len() >= 256 { return None; }
        self.keys.push(key); self.keys.sort_by(|a, b| a.time.total_cmp(&b.time));
        self.keys.iter().position(|k| k.time == key.time)
    }
}


/// Isolated Render Studio illumination, evaluated in OUTPUT timeline seconds.
/// No BSP lightgrid, gameplay model brightness or other fallback illumination
/// may survive a zero ambient/sun/local-light rig.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StudioEnvironmentKey {
    pub time: f64,
    pub ambient_color: [f32;3],
    pub ambient_intensity: f32,
    pub sun_color: [f32;3],
    pub sun_intensity: f32,
    /// JKA coordinates; unit vector from the surface TOWARD the sun.
    pub sun_direction: [f32;3],
}
impl Default for StudioEnvironmentKey {
    fn default() -> Self { Self {time:0.0,ambient_color:[1.0;3],ambient_intensity:1.0,
        sun_color:[1.0;3],sun_intensity:0.0,sun_direction:[0.4,-0.3,0.85] } }
}
impl StudioEnvironmentKey {
    pub fn valid(&self) -> bool {
        self.time.is_finite() && (0.0..=36000.0).contains(&self.time)
        && self.ambient_intensity.is_finite() && (0.0..=16.0).contains(&self.ambient_intensity)
        && self.sun_intensity.is_finite() && (0.0..=16.0).contains(&self.sun_intensity)
        && self.ambient_color.iter().chain(self.sun_color.iter()).all(|x|x.is_finite()&&(0.0..=1.0).contains(x))
        && self.sun_direction.iter().all(|x|x.is_finite()&&x.abs()<=1.0)
        && glam::Vec3::from_array(self.sun_direction).length_squared()>0.000001
    }
    fn lerp(self, next: Self, t: f32) -> Self {
        let v=|a:[f32;3],b:[f32;3]|std::array::from_fn(|i|a[i]+(b[i]-a[i])*t);
        Self {time:self.time+(next.time-self.time)*f64::from(t),
            ambient_color:v(self.ambient_color,next.ambient_color),
            ambient_intensity:self.ambient_intensity+(next.ambient_intensity-self.ambient_intensity)*t,
            sun_color:v(self.sun_color,next.sun_color),
            sun_intensity:self.sun_intensity+(next.sun_intensity-self.sun_intensity)*t,
            sun_direction:v(self.sun_direction,next.sun_direction) }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StudioEnvironment {
    pub keys: Vec<StudioEnvironmentKey>,
}
impl Default for StudioEnvironment {
    fn default()->Self {Self {keys:vec![StudioEnvironmentKey::default()]}}
}
impl StudioEnvironment {
    pub fn valid(&self)->bool {
        !self.keys.is_empty()&&self.keys.len()<=128
            && self.keys.iter().all(StudioEnvironmentKey::valid)
            && self.keys.windows(2).all(|p|p[0].time+0.000001<p[1].time)
    }
    pub fn sample(&self,time:f64,smooth:bool)->StudioEnvironmentKey {
        let first=self.keys.first().copied().unwrap_or_default();
        let mut selected=first;
        if time>first.time {
            selected=*self.keys.last().unwrap_or(&first);
            for pair in self.keys.windows(2) {
                if time<=pair[1].time {
                    let mut t=((time-pair[0].time)/(pair[1].time-pair[0].time).max(0.000001)).clamp(0.0,1.0) as f32;
                    if smooth {t=t*t*(3.0-2.0*t);}
                    selected=pair[0].lerp(pair[1],t);
                    break;
                }
            }
        }
        selected.time=time.clamp(0.0,36000.0);
        selected
    }
    pub fn add_key(&mut self,key:StudioEnvironmentKey)->Option<usize> {
        if !key.valid(){return None;}
        if let Some(i)=self.keys.iter().position(|k|(k.time-key.time).abs()<0.000001) {
            self.keys[i]=key;return Some(i);
        }
        if self.keys.len()>=128 {return None;}
        self.keys.push(key);self.keys.sort_by(|a,b|a.time.total_cmp(&b.time));
        self.keys.iter().position(|k|k.time==key.time)
    }
}

fn expanded() -> bool { true }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StudioLightLayer {
    pub name: String,
    pub enabled: bool,
    pub lights: Vec<StudioLight>,
    #[serde(skip, default = "expanded")]
    pub expanded: bool,
}
impl StudioLightLayer {
    pub fn new(number: usize) -> Self { Self { name: format!("Lights {number}"), enabled: true, lights: vec![], expanded: true } }
}
pub fn valid_layers(layers: &[StudioLightLayer]) -> bool {
    layers.len() <= MAX_LAYERS && layers.iter().map(|l| l.lights.len()).sum::<usize>() <= MAX_LIGHTS
        && layers.iter().all(|l| l.name.len() <= 128 && l.lights.iter().all(StudioLight::valid))
}


impl crate::render_studio_timeline::StudioTimeline {
    pub fn light_layer_row(&self, layer: usize) -> usize {
        self.demo_row(self.demo_layer_count()) + self.light_layers.iter().take(layer)
            .map(|l| 1 + if l.expanded { l.lights.len() } else { 0 }).sum::<usize>()
    }
    pub fn light_row(&self, layer: usize, light: usize) -> Option<usize> {
        self.light_layers.get(layer).filter(|l| l.expanded && light < l.lights.len())
            .map(|_| self.light_layer_row(layer) + 1 + light)
    }
}
