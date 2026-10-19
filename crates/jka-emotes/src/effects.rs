//! Baked particle tracks for emote effects (`effects/*.fxt` bundle members).
//!
//! The compiler executes the exported Niagara systems on the composed source
//! animation and records the renderer-bound particle attributes in the target
//! model's space. Playback interpolates matching particle IDs between samples;
//! no Unreal VM runs in the game.
use serde_json::{json, Value};

const MAGIC: &[u8; 4] = b"DJKX";
const VERSION: u16 = 2;
pub const MAX_RENDERERS: usize = 256;
/// Recording rate of compiled tracks; playback interpolates between samples.
pub const TRACK_FPS: f32 = 15.;
const MAX_FRAMES: usize = 15 * 60 * 10;
const MAX_RECORDS: usize = 4_000_000;
/// Positions are stored as i16 at this resolution (game units).
const POSITION_STEP: f32 = 1. / 8.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Sprite,
    Mesh,
    Ribbon,
    Light,
}
impl Kind {
    fn code(self) -> u8 {
        self as u8
    }
    pub fn name(self) -> &'static str {
        match self {
            Kind::Sprite => "sprite",
            Kind::Mesh => "mesh",
            Kind::Ribbon => "ribbon",
            Kind::Light => "light",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "sprite" => Kind::Sprite,
            "mesh" => Kind::Mesh,
            "ribbon" => Kind::Ribbon,
            "light" => Kind::Light,
            _ => return None,
        })
    }
}

/// Which way a sprite quad faces (`ENiagaraSpriteFacingMode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Facing {
    Camera,
    /// Quad lies in the plane perpendicular to the particle's facing vector.
    Custom,
}
/// Long-axis alignment of a sprite (`ENiagaraSpriteAlignment`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    None,
    Velocity,
    Custom,
}

#[derive(Clone, Debug)]
pub struct Renderer {
    pub kind: Kind,
    /// Shader for sprites/ribbons; model qpath for meshes.
    pub shader: String,
    pub models: Vec<String>,
    pub sub_image: [u8; 2],
    pub facing: Facing,
    pub align: Align,
    pub sort: i32,
    pub name: String,
    /// Sprite PivotInUVSpace: the UV point placed on the particle position
    /// ((0.5, 0.5) = centred; aura flames use (0.5, 0.9) to rise from it).
    pub pivot: [f32; 2],
}
impl Renderer {
    pub fn to_json(&self) -> Value {
        json!({"kind":self.kind.name(),"shader":self.shader,"models":self.models,"sub_image":self.sub_image,"facing":match self.facing{Facing::Camera=>"camera",Facing::Custom=>"custom"},"align":match self.align{Align::None=>"none",Align::Velocity=>"velocity",Align::Custom=>"custom"},"sort":self.sort,"name":self.name,"pivot":self.pivot})
    }
    pub fn parse(v: &Value) -> Result<Self, String> {
        let kind = v["kind"]
            .as_str()
            .and_then(Kind::parse)
            .ok_or("invalid effect renderer kind")?;
        let shader = v["shader"].as_str().unwrap_or("").to_owned();
        if !shader.is_empty() {
            crate::manifest::qpath(&shader)?;
        }
        let models = v["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(crate::manifest::qpath)
            .collect::<Result<Vec<_>, _>>()?;
        if models.len() > 64 {
            return Err("too many effect meshes".into());
        }
        let sub = |i: usize| v["sub_image"][i].as_u64().unwrap_or(1).clamp(1, 64) as u8;
        Ok(Self {
            kind,
            shader,
            models,
            sub_image: [sub(0), sub(1)],
            facing: if v["facing"] == "custom" {
                Facing::Custom
            } else {
                Facing::Camera
            },
            // Version 1 manifests stored velocity alignment as a facing mode.
            align: match v["align"].as_str().or(v["facing"].as_str()) {
                Some("velocity") => Align::Velocity,
                Some("custom") if v["align"].is_string() => Align::Custom,
                _ => Align::None,
            },
            sort: v["sort"].as_i64().unwrap_or(0) as i32,
            name: v["name"].as_str().unwrap_or("").chars().take(96).collect(),
            pivot: [0, 1].map(|i| {
                v["pivot"][i]
                    .as_f64()
                    .filter(|n| n.is_finite())
                    .map_or(0.5, |n| n.clamp(-1., 2.) as f32)
            }),
        })
    }
}

/// One recorded particle in target model space (game units, JKA axes).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Particle {
    pub renderer: u8,
    pub id: u16,
    pub position: [f32; 3],
    /// Display color (0..1); HDR intensity is folded into rgb.
    pub rgba: [u8; 4],
    pub size: [f32; 2],
    pub rotation: f32,
    pub sub_image: u8,
    /// Unit direction: velocity or custom alignment (long axis) for sprites/ribbons.
    pub direction: [f32; 3],
    /// Unit facing vector for custom-facing sprites.
    pub normal: [f32; 3],
    pub orientation: [f32; 4],
    pub scale: [f32; 3],
    pub mesh: u8,
    pub ribbon: u16,
    pub link: f32,
    pub radius: f32,
}

fn f16(v: f32) -> [u8; 2] {
    half::f16::from_f32(v).to_bits().to_le_bytes()
}
fn snorm(v: f32) -> [u8; 2] {
    ((v.clamp(-1., 1.) * 32767.).round() as i16).to_le_bytes()
}

/// Encoder for a whole timeline.
pub struct Writer {
    pub fps: f32,
    pub renderers: Vec<Renderer>,
    frames: Vec<Vec<u8>>,
}
impl Writer {
    pub fn new(fps: f32, renderers: Vec<Renderer>) -> Self {
        Self {
            fps,
            renderers,
            frames: Vec::new(),
        }
    }
    pub fn push_frame(&mut self, particles: &[Particle]) {
        let mut out = Vec::with_capacity(particles.len() * 22 + 4);
        out.extend_from_slice(&(particles.len() as u32).to_le_bytes());
        for p in particles {
            let kind = self.renderers[p.renderer as usize].kind;
            out.push(p.renderer);
            out.extend_from_slice(&p.id.to_le_bytes());
            for c in p.position {
                out.extend_from_slice(
                    &((c / POSITION_STEP).round().clamp(-32767., 32767.) as i16).to_le_bytes(),
                );
            }
            out.extend_from_slice(&p.rgba);
            match kind {
                Kind::Sprite => {
                    out.extend(f16(p.size[0]));
                    out.extend(f16(p.size[1]));
                    out.extend(f16(p.rotation));
                    out.push(p.sub_image);
                    for c in p.direction.into_iter().chain(p.normal) {
                        out.push(((c.clamp(-1., 1.) * 127.).round() as i8) as u8);
                    }
                }
                Kind::Mesh => {
                    for c in p.orientation {
                        out.extend(snorm(c));
                    }
                    for c in p.scale {
                        out.extend(f16(c));
                    }
                    out.push(p.mesh);
                }
                Kind::Ribbon => {
                    out.extend(f16(p.size[0]));
                    out.extend_from_slice(&p.ribbon.to_le_bytes());
                    out.extend_from_slice(&p.link.to_le_bytes());
                    for c in p.direction {
                        out.push(((c.clamp(-1., 1.) * 127.).round() as i8) as u8);
                    }
                }
                Kind::Light => out.extend(f16(p.radius)),
            }
        }
        self.frames.push(out);
    }
    pub fn frames(&self) -> usize {
        self.frames.len()
    }
    pub fn is_empty(&self) -> bool {
        self.frames.iter().all(|f| f[..4] == [0, 0, 0, 0])
    }
    pub fn finish(self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&(self.renderers.len() as u16).to_le_bytes());
        out.extend_from_slice(&(self.frames.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.fps.to_le_bytes());
        for r in &self.renderers {
            out.push(r.kind.code());
        }
        for f in &self.frames {
            out.extend_from_slice(f);
        }
        out
    }
}

/// Decoded track. Frames index into one flat particle array.
#[derive(Debug)]
pub struct Track {
    pub fps: f32,
    pub kinds: Vec<Kind>,
    frames: Vec<(u32, u32)>,
    particles: Vec<Particle>,
}
impl Track {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut at = 0usize;
        let take = |at: &mut usize, n: usize| -> Result<&[u8], String> {
            let s = bytes.get(*at..*at + n).ok_or("truncated effect track")?;
            *at += n;
            Ok(s)
        };
        if take(&mut at, 4)? != MAGIC {
            return Err("not an effect track".into());
        }
        let version = u16::from_le_bytes(take(&mut at, 2)?.try_into().unwrap());
        if !(1..=VERSION).contains(&version) {
            return Err("unsupported effect track version".into());
        }
        let renderers = u16::from_le_bytes(take(&mut at, 2)?.try_into().unwrap()) as usize;
        let frames = u32::from_le_bytes(take(&mut at, 4)?.try_into().unwrap()) as usize;
        let fps = f32::from_le_bytes(take(&mut at, 4)?.try_into().unwrap());
        if renderers == 0
            || renderers > MAX_RENDERERS
            || frames == 0
            || frames > MAX_FRAMES
            || !(1. ..=120.).contains(&fps)
        {
            return Err("invalid effect track header".into());
        }
        let kinds = take(&mut at, renderers)?
            .iter()
            .map(|k| match k {
                0 => Ok(Kind::Sprite),
                1 => Ok(Kind::Mesh),
                2 => Ok(Kind::Ribbon),
                3 => Ok(Kind::Light),
                _ => Err("invalid effect renderer kind".to_string()),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut table = Vec::with_capacity(frames);
        let mut particles = Vec::new();
        let h = |b: &[u8]| half::f16::from_bits(u16::from_le_bytes([b[0], b[1]])).to_f32();
        let s8 = |b: u8| (b as i8) as f32 / 127.;
        for _ in 0..frames {
            let count = u32::from_le_bytes(take(&mut at, 4)?.try_into().unwrap()) as usize;
            if particles.len() + count > MAX_RECORDS {
                return Err("effect track exceeds particle budget".into());
            }
            let start = particles.len() as u32;
            for _ in 0..count {
                let head = take(&mut at, 13)?;
                let renderer = head[0];
                let kind = *kinds
                    .get(renderer as usize)
                    .ok_or("effect particle renderer out of range")?;
                let mut p = Particle {
                    renderer,
                    id: u16::from_le_bytes([head[1], head[2]]),
                    position: std::array::from_fn(|i| {
                        i16::from_le_bytes([head[3 + 2 * i], head[4 + 2 * i]]) as f32
                            * POSITION_STEP
                    }),
                    rgba: [head[9], head[10], head[11], head[12]],
                    orientation: [0., 0., 0., 1.],
                    scale: [1.; 3],
                    ..Particle::default()
                };
                match kind {
                    Kind::Sprite => {
                        let b = take(&mut at, if version >= 2 { 13 } else { 10 })?;
                        p.size = [h(&b[0..2]), h(&b[2..4])];
                        p.rotation = h(&b[4..6]);
                        p.sub_image = b[6];
                        p.direction = [s8(b[7]), s8(b[8]), s8(b[9])];
                        if version >= 2 {
                            p.normal = [s8(b[10]), s8(b[11]), s8(b[12])];
                        }
                    }
                    Kind::Mesh => {
                        let b = take(&mut at, 15)?;
                        p.orientation = std::array::from_fn(|i| {
                            i16::from_le_bytes([b[2 * i], b[2 * i + 1]]) as f32 / 32767.
                        });
                        p.scale = std::array::from_fn(|i| h(&b[8 + 2 * i..10 + 2 * i]));
                        p.mesh = b[14];
                    }
                    Kind::Ribbon => {
                        let b = take(&mut at, 11)?;
                        p.size = [h(&b[0..2]), 0.];
                        p.ribbon = u16::from_le_bytes([b[2], b[3]]);
                        p.link = f32::from_le_bytes(b[4..8].try_into().unwrap());
                        p.direction = [s8(b[8]), s8(b[9]), s8(b[10])];
                    }
                    Kind::Light => {
                        let b = take(&mut at, 2)?;
                        p.radius = h(b);
                    }
                }
                if !p
                    .position
                    .iter()
                    .chain(&p.size)
                    .chain(&p.scale)
                    .all(|n| n.is_finite())
                {
                    return Err("non-finite effect particle".into());
                }
                particles.push(p);
            }
            table.push((start, count as u32));
        }
        if at != bytes.len() {
            return Err("trailing effect track data".into());
        }
        Ok(Self {
            fps,
            kinds,
            frames: table,
            particles,
        })
    }
    pub fn frames(&self) -> usize {
        self.frames.len()
    }
    pub fn frame(&self, index: usize) -> &[Particle] {
        let (start, count) = self.frames[index.min(self.frames.len() - 1)];
        &self.particles[start as usize..(start + count) as usize]
    }
    /// Interpolated particles at `seconds` on the variant clock. `loop_start`
    /// is the first looping sample; past the end the track wraps there.
    pub fn sample(&self, seconds: f32, loop_start: Option<usize>, out: &mut Vec<Particle>) {
        out.clear();
        let n = self.frames.len();
        let t = (seconds.max(0.) * self.fps).min(n as f32 - 1e-3);
        let a = (t.floor() as usize).min(n - 1);
        let b = if a + 1 < n {
            a + 1
        } else {
            loop_start.filter(|&s| s < n).unwrap_or(a)
        };
        let w = t - a as f32;
        let next = self.frame(b);
        for p in self.frame(a) {
            let mut q = *p;
            // Particles in each frame are sorted by (renderer, id).
            if let Ok(k) = next.binary_search_by(|o| (o.renderer, o.id).cmp(&(p.renderer, p.id))) {
                let o = &next[k];
                let lerp = |x: f32, y: f32| x + (y - x) * w;
                q.position = std::array::from_fn(|i| lerp(p.position[i], o.position[i]));
                q.size = [lerp(p.size[0], o.size[0]), lerp(p.size[1], o.size[1])];
                q.rgba =
                    std::array::from_fn(|i| lerp(p.rgba[i] as f32, o.rgba[i] as f32).round() as u8);
                q.scale = std::array::from_fn(|i| lerp(p.scale[i], o.scale[i]));
                q.radius = lerp(p.radius, o.radius);
                q.direction = std::array::from_fn(|i| lerp(p.direction[i], o.direction[i]));
                q.normal = std::array::from_fn(|i| lerp(p.normal[i], o.normal[i]));
                q.link = lerp(p.link, o.link);
                let mut d = p.rotation - o.rotation;
                d -= 360. * (d / 360.).round();
                q.rotation = p.rotation - d * w;
                let dot: f32 = (0..4).map(|i| p.orientation[i] * o.orientation[i]).sum();
                let sign = if dot < 0. { -1. } else { 1. };
                let mut r: [f32; 4] =
                    std::array::from_fn(|i| lerp(p.orientation[i], sign * o.orientation[i]));
                let len = r.iter().map(|c| c * c).sum::<f32>().sqrt().max(1e-6);
                r.iter_mut().for_each(|c| *c /= len);
                q.orientation = r;
            } else if w > 0.5 {
                // Dying particle: fade out instead of popping mid-interval.
                q.rgba[3] = (q.rgba[3] as f32 * (1. - w) * 2.) as u8;
            }
            out.push(q);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn renderers() -> Vec<Renderer> {
        vec![
            Renderer {
                kind: Kind::Sprite,
                shader: "fx/a".into(),
                models: vec![],
                sub_image: [4, 4],
                facing: Facing::Custom,
                align: Align::Velocity,
                sort: 0,
                name: String::new(),
                pivot: [0.5, 0.9],
            },
            Renderer {
                kind: Kind::Mesh,
                shader: String::new(),
                models: vec!["models/a.md3".into()],
                sub_image: [1, 1],
                facing: Facing::Camera,
                align: Align::None,
                sort: 0,
                name: String::new(),
                pivot: [0.5, 0.5],
            },
        ]
    }
    #[test]
    fn round_trips_and_interpolates_matching_ids() {
        let mut w = Writer::new(15., renderers());
        let a = Particle {
            renderer: 0,
            id: 3,
            position: [1., 2., 3.],
            rgba: [255, 0, 0, 255],
            size: [4., 4.],
            normal: [0., 0., 1.],
            ..Default::default()
        };
        let m = Particle {
            renderer: 1,
            id: 1,
            position: [0., 0., 8.],
            orientation: [0., 0., 0., 1.],
            scale: [1., 2., 3.],
            ..Default::default()
        };
        w.push_frame(&[a, m]);
        w.push_frame(&[Particle {
            position: [3., 2., 3.],
            ..a
        }]);
        let t = Track::parse(&w.finish()).unwrap();
        assert_eq!(t.frames(), 2);
        assert_eq!(t.frame(0)[1].scale, [1., 2., 3.]);
        assert_eq!(t.frame(0)[0].normal, [0., 0., 1.]);
        let mut out = Vec::new();
        t.sample(0.5 / 15., None, &mut out);
        assert_eq!(out[0].position, [2., 2., 3.]);
        assert_eq!(out.len(), 2);
    }
    #[test]
    fn rejects_truncated_tracks() {
        let mut w = Writer::new(15., renderers());
        w.push_frame(&[Particle::default()]);
        let mut bytes = w.finish();
        bytes.pop();
        assert!(Track::parse(&bytes).is_err());
    }
}
