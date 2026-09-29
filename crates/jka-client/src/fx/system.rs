//! OpenJK FxScheduler.cpp / FxPrimitives.cpp / FxUtil.cpp: effect
//! registration, scheduling and per-frame primitive simulation.
//!
//! Output is renderer-neutral: sprites, oriented quads, lines, cylinders and
//! lights in JKA world space, plus sounds for the audio presenter. Camera-
//! facing geometry is built later by `fx::draw` with the final render view.

use super::{
    gp2,
    template::*,
};
use std::collections::{HashMap, HashSet};

pub type EffectId = u32; // 0 = none, like OpenJK handles

/// FX_MAX_EFFECTS.
const MAX_EFFECTS: usize = 256;
/// MAX_EFFECTS in FxPrimitives.h (live primitive pool).
const MAX_ACTIVE: usize = 1800;
/// Scheduled-effect pool size (FxScheduler.h PoolAllocator of 1024).
const MAX_SCHEDULED: usize = 1024;
/// FX_MAX_TRACE_DIST.
const MAX_TRACE_DIST: f32 = 16384.0;
/// CEmitter trail think rate.
const TRAIL_RATE: i32 = 12;

type Axis = [[f32; 3]; 3];

/// Presentation ownership for a spawned FX tree. Child/death/emitter effects
/// inherit the root class so runtime A/B switches suppress the authored tree
/// without guessing from shader names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FxClass {
    #[default]
    General,
    SaberImpact,
}

/// Effect ids a primitive refers to, resolved once at registration.
#[derive(Clone, Debug, Default)]
struct Links {
    death: Vec<EffectId>,
    emitter: Vec<EffectId>,
    play: Vec<EffectId>,
}

struct LoadedEffect {
    template: EffectTemplate,
    links: Vec<Links>,
}

#[derive(Clone, Copy, Debug)]
struct Scheduled {
    start_time: i32,
    effect: EffectId,
    primitive: usize,
    origin: [f32; 3],
    axis: Axis,
    class: FxClass,
}

/// One live primitive (CParticle/CLine/CTail/... state).
#[derive(Clone, Debug)]
struct Primitive {
    class: FxClass,
    kind: PrimType,
    flags: u32,
    time_start: i32,
    time_end: i32,
    kill_time: i32,
    shader: Option<String>,
    origin: [f32; 3],
    origin2: [f32; 3],
    old_origin: [f32; 3],
    vel: [f32; 3],
    accel: [f32; 3],
    normal: [f32; 3],
    axis: Axis,
    size: (f32, f32, f32),
    size2: (f32, f32, f32),
    length: (f32, f32, f32),
    alpha: (f32, f32, f32),
    rgb_start: [f32; 3],
    rgb_end: [f32; 3],
    rgb_parm: f32,
    rotation: f32,
    rotation_delta: f32,
    death_fx: EffectId,
    // Emitter state.
    emitter_fx: EffectId,
    density: f32,
    variance: f32,
    old_time: i32,
    old_velocity: [f32; 3],
    angles: [f32; 3],
    angle_delta: [f32; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub enum FxDraw {
    /// RT_SPRITE: view-facing quad of half-extent `radius`.
    Sprite { origin: [f32; 3], radius: f32, rotation: f32, rgba: [u8; 4], shader: String },
    /// RT_ORIENTED_QUAD: quad in axis[1]/axis[2] of half-extent `radius`.
    OrientedQuad { origin: [f32; 3], axis: Axis, radius: f32, rotation: f32, rgba: [u8; 4], shader: String },
    /// RT_LINE: view-facing strip of half-width `width` from start to end.
    Line { start: [f32; 3], end: [f32; 3], width: f32, rgba: [u8; 4], shader: String },
    /// Arbitrary two-sided textured quad. OpenJK saber trails use exactly this
    /// shape to connect the current blade base/tip to the previous sample.
    Quad { positions: [[f32; 3]; 4], uvs: [[f32; 2]; 4], rgba: [u8; 4], shader: String },
    /// Arbitrary indexed two-sided mesh with per-vertex colour. This is used by
    /// smooth cosmetic FX that need more than a quad (for example enhanced
    /// saber melt relief) without adding a bespoke renderer hot path.
    Mesh {
        positions: Vec<[f32; 3]>,
        uvs: Vec<[f32; 2]>,
        rgba: Vec<[u8; 4]>,
        indices: Vec<u32>,
        shader: String,
    },
    /// Lit, alpha-blended relief mesh with real normals and no texture (vertex
    /// colour only). `lighting_origin` picks the BSP light-grid sample. Used by
    /// the enhanced saber melt so raised slag is shaded by the scene lights.
    LitMesh {
        positions: Vec<[f32; 3]>,
        normals: Vec<[f32; 3]>,
        rgba: Vec<[u8; 4]>,
        indices: Vec<u32>,
        lighting_origin: [f32; 3],
    },
    /// RT_CYLINDER: `start_radius` ring at start, `end_radius` ring at end.
    Cylinder { start: [f32; 3], end: [f32; 3], axis: [f32; 3], start_radius: f32, end_radius: f32, rgba: [u8; 4], shader: String },
}

/// Origin class for RE_AddLightToScene-style transient lights.  This is
/// diagnostic metadata only; it must not affect authored light behavior.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FxLightKind {
    /// Authored `Light {}` primitive from an .efx file.
    #[default]
    AuthoredEffect,
    /// CG_DoSaber / CG_DoSaberLight blade light.
    Saber,
    /// DinurdoJK enhanced saber-mark heat light.
    SaberMark,
}

/// RE_AddLightToScene from an FX light primitive.
#[derive(Clone, Debug, PartialEq)]
pub struct FxLight {
    pub kind: FxLightKind,
    pub origin: [f32; 3],
    pub radius: f32,
    pub rgb: [f32; 3],
    /// Optional rendered blade endpoints in JKA space. Legacy modes use origin.
    pub segment: Option<[[f32; 3]; 2]>,
    pub blade_segments: Option<std::sync::Arc<[FxLightSegment]>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FxLightSegment {
    pub endpoints: [[f32; 3]; 2],
    pub rgb: [f32; 3],
    pub weight: f32,
}

/// A Sound primitive (S_StartSound at `origin`, CHAN_AUTO).
#[derive(Clone, Debug, PartialEq)]
pub struct FxSound {
    pub origin: [f32; 3],
    pub qpath: String,
}

#[derive(Default, Debug)]
pub struct FxFrame {
    pub draws: Vec<FxDraw>,
    pub lights: Vec<FxLight>,
    pub sounds: Vec<FxSound>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FxStats {
    pub registered: usize,
    pub active: usize,
    pub scheduled: usize,
    pub dropped: u64,
    /// Primitive kinds that are parsed but not simulated/drawn yet.
    pub unsupported_spawns: u64,
}

pub struct FxSystem {
    effects: Vec<LoadedEffect>,
    ids: HashMap<String, EffectId>,
    failed: HashSet<String>,
    scheduled: Vec<Scheduled>,
    active: Vec<Primitive>,
    sounds: Vec<FxSound>,
    /// FxRunner PlayEffect calls made while creating a primitive.
    runner_queue: Vec<(EffectId, [f32; 3], Axis, FxClass)>,
    time: i32,
    old_time: i32,
    frame_time: i32,
    real_time: f32,
    rng: Rng,
    dropped: u64,
    unsupported_spawns: u64,
}

impl Default for FxSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl FxSystem {
    pub fn new() -> Self {
        Self {
            effects: Vec::new(),
            ids: HashMap::new(),
            failed: HashSet::new(),
            scheduled: Vec::new(),
            active: Vec::new(),
            sounds: Vec::new(),
            runner_queue: Vec::new(),
            time: 0,
            old_time: 0,
            frame_time: 0,
            real_time: 0.0,
            rng: Rng::new(0x5EED_F00D),
            dropped: 0,
            unsupported_spawns: 0,
        }
    }

    pub fn stats(&self) -> FxStats {
        FxStats {
            registered: self.effects.len(),
            active: self.active.len(),
            scheduled: self.scheduled.len(),
            dropped: self.dropped,
            unsupported_spawns: self.unsupported_spawns,
        }
    }

    /// CFxScheduler::Clean: drop live and scheduled primitives (seek/restart).
    pub fn clear_active(&mut self) {
        self.active.clear();
        self.scheduled.clear();
        self.sounds.clear();
    }

    /// Reset the scheduler clock and transient primitives at a demo seek boundary.
    pub fn reset_time(&mut self, time: i32) {
        self.clear_active();
        self.time = time;
        self.old_time = time;
        self.frame_time = 0;
        self.real_time = 0.0;
    }

    /// CFxScheduler::RegisterEffect. `read` returns a file's bytes by qpath.
    pub fn register(&mut self, file: &str, read: &mut dyn FnMut(&str) -> Option<Vec<u8>>) -> EffectId {
        let file = file.replace('\\', "/");
        let key = strip_extension(&file).to_ascii_lowercase();
        if let Some(&id) = self.ids.get(&key) {
            return id;
        }
        if self.failed.contains(&key) || self.effects.len() + 1 >= MAX_EFFECTS {
            return 0;
        }
        let mut path = file.to_ascii_lowercase();
        if !path.contains('.') {
            path.push_str(".efx");
        }
        if !path.starts_with("effects") {
            path = format!("effects/{path}");
        }
        let Some(bytes) = read(&path) else {
            self.failed.insert(key);
            return 0;
        };
        let template = parse_effect(&key, &gp2::parse(&bytes));
        // Reserve the id before resolving links so self-references terminate.
        let id = (self.effects.len() + 1) as EffectId;
        self.ids.insert(key, id);
        let names: Vec<_> = template
            .primitives
            .iter()
            .map(|prim| (prim.impact_fx.clone(), prim.death_fx.clone(), prim.emitter_fx.clone(), prim.play_fx.clone()))
            .collect();
        self.effects.push(LoadedEffect { template, links: Vec::new() });
        let mut links = Vec::with_capacity(names.len());
        for (impact, death, emitter, play) in names {
            let mut resolve = |list: Vec<String>| {
                list.iter().map(|name| self.register(name, read)).filter(|&id| id != 0).collect::<Vec<_>>()
            };
            // Impact effects are registered (precached) like OpenJK, but FX
            // particles do not collide with the world yet, so none can fire.
            resolve(impact);
            links.push(Links {
                death: resolve(death),
                emitter: resolve(emitter),
                play: resolve(play),
            });
        }
        self.effects[(id - 1) as usize].links = links;
        id
    }

    #[cfg(test)]
    pub fn effect_name(&self, id: EffectId) -> Option<&str> {
        self.effects.get((id as usize).checked_sub(1)?).map(|effect| effect.template.name.as_str())
    }

    /// FX_AdjustTime: advance the FX clock to `cg.time`.
    pub fn adjust_time(&mut self, time: i32) {
        if time < self.time {
            // Demo seek/restart: OpenJK clears the scene on a time reset.
            self.clear_active();
            self.old_time = time;
        } else {
            self.old_time = self.time;
        }
        self.time = time;
        self.frame_time = self.time - self.old_time;
        self.real_time = self.frame_time as f32 * 0.001;
    }

    /// CFxScheduler::PlayEffect(id, origin, forward).
    pub fn play_effect_dir(&mut self, id: EffectId, origin: [f32; 3], forward: [f32; 3]) {
        self.play_effect_dir_class(id, origin, forward, FxClass::General);
    }

    pub fn play_effect_dir_class(
        &mut self,
        id: EffectId,
        origin: [f32; 3],
        forward: [f32; 3],
        class: FxClass,
    ) {
        let (right, up) = make_normal_vectors(forward);
        self.play_effect_class(id, origin, [forward, right, up], class);
    }

    /// CFxScheduler::PlayEffect(id, origin, axis) for unbolted effects.
    pub fn play_effect(&mut self, id: EffectId, origin: [f32; 3], axis: Axis) {
        self.play_effect_class(id, origin, axis, FxClass::General);
    }

    fn play_effect_class(&mut self, id: EffectId, origin: [f32; 3], axis: Axis, class: FxClass) {
        let Some(effect) = (id as usize).checked_sub(1).and_then(|index| self.effects.get(index)) else {
            return;
        };
        let count_per_prim: Vec<(usize, i32, f32, bool)> = effect
            .template
            .primitives
            .iter()
            .enumerate()
            .map(|(index, prim)| {
                // fx_countScale defaults to 1.
                let mut count = round(prim.spawn_count.get(&mut self.rng));
                if prim.spawn_count.min >= 1.0 && count < 1 {
                    count = 1;
                }
                let even = prim.spawn_flags & FX_EVEN_DISTRIBUTION != 0;
                let factor = if even {
                    (prim.spawn_delay.max - prim.spawn_delay.min).abs() / count.max(1) as f32
                } else {
                    0.0
                };
                (index, count, factor, even)
            })
            .collect();
        for (index, count, factor, even) in count_per_prim {
            for t in 0..count {
                let delay = if even {
                    (t as f32 * factor) as i32
                } else {
                    self.effects[(id - 1) as usize].template.primitives[index].spawn_delay.get(&mut self.rng) as i32
                };
                if delay < 1 {
                    self.create_effect(id, index, origin, axis, -delay, class);
                } else if self.scheduled.len() < MAX_SCHEDULED {
                    self.scheduled.push(Scheduled {
                        start_time: self.time + delay,
                        effect: id,
                        primitive: index,
                        origin,
                        axis,
                        class,
                    });
                } else {
                    self.dropped += 1;
                }
            }
        }
        while let Some((id, origin, axis, class)) = self.runner_queue.pop() {
            self.play_effect_class(id, origin, axis, class);
        }
    }

    /// AddScheduledEffects + FX_Add: spawn due primitives, simulate every live
    /// primitive for this frame and return what it submits.
    pub fn frame(&mut self) -> FxFrame {
        self.frame_with_visibility(true)
    }

    /// Simulate the normal OpenJK FX frame while optionally suppressing the
    /// rendered output of saber-impact trees. The simulation is intentionally
    /// retained so a paused demo can toggle the same frozen population off/on.
    pub fn frame_with_visibility(&mut self, saber_impact_fx: bool) -> FxFrame {
        let now = self.time;
        let mut due = Vec::new();
        self.scheduled.retain(|scheduled| {
            if scheduled.start_time <= now {
                due.push(*scheduled);
                false
            } else {
                true
            }
        });
        for scheduled in due {
            self.create_effect(
                scheduled.effect,
                scheduled.primitive,
                scheduled.origin,
                scheduled.axis,
                0,
                scheduled.class,
            );
        }

        let mut out = FxFrame { sounds: std::mem::take(&mut self.sounds), ..FxFrame::default() };
        let mut deaths = Vec::new();
        let mut emits = Vec::new();
        let (time, real_time, frame_time) = (self.time, self.real_time, self.frame_time);
        let rng = &mut self.rng;
        self.active.retain_mut(|prim| {
            if time > prim.kill_time {
                if prim.flags & FX_DEATH_RUNS_FX != 0 && prim.flags & FX_KILL_ON_IMPACT == 0 && prim.death_fx != 0 {
                    // CParticle::Die: death effect in a random direction.
                    let dir = normalize([rng.flrand(-1.0, 1.0), rng.flrand(-1.0, 1.0), rng.flrand(-1.0, 1.0)]);
                    deaths.push((prim.death_fx, prim.origin, dir, prim.class));
                }
                return false;
            }
            let visible = saber_impact_fx || prim.class != FxClass::SaberImpact;
            update_primitive(prim, time, real_time, frame_time, rng, &mut out, &mut emits, visible)
        });
        for (id, origin, dir, class) in deaths {
            self.play_effect_dir_class(id, origin, dir, class);
        }
        for (id, origin, axis, class) in emits {
            self.play_effect_class(id, origin, axis, class);
        }
        while let Some((id, origin, axis, class)) = self.runner_queue.pop() {
            self.play_effect_class(id, origin, axis, class);
        }
        // Death/emitter effects spawned above are drawn from next frame on,
        // like OpenJK's FX_Add iterating a list that grows behind it.
        out
    }

    /// CFxScheduler::CreateEffect.
    fn create_effect(
        &mut self,
        id: EffectId,
        index: usize,
        origin: [f32; 3],
        axis: Axis,
        late_time: i32,
        class: FxClass,
    ) {
        // FX_Add* refuse new primitives while paused (mFrameTime < 1).
        if self.frame_time < 1 {
            return;
        }
        let effect = &self.effects[(id - 1) as usize];
        let fx = effect.template.primitives[index].clone();
        let links = effect.links.get(index).cloned().unwrap_or_default();
        let rng = &mut self.rng;
        let mut ax = axis;
        let flags = fx.flags;

        if fx.spawn_flags & FX_RAND_ROT_AROUND_FWD != 0 {
            ax[1] = rotate_point_around_vector(ax[0], axis[1], rng.flrand(0.0, 360.0));
            ax[2] = cross(ax[0], ax[1]);
        }

        let o1 = [fx.origin1[0].get(rng), fx.origin1[1].get(rng), fx.origin1[2].get(rng)];
        let mut org = if fx.spawn_flags & FX_CHEAP_ORG_CALC != 0 {
            o1
        } else {
            combine(ax, o1)
        };
        org = add(org, origin);

        if fx.spawn_flags & FX_ORG_ON_SPHERE != 0 {
            let x = rng.flrand(0.0, 360.0).to_radians();
            let y = rng.flrand(0.0, 180.0).to_radians();
            let width = fx.radius.get(rng);
            let height = fx.height.get(rng);
            let temp = [x.sin() * width * y.sin(), x.cos() * width * y.sin(), y.cos() * height];
            org = add(org, temp);
            if fx.spawn_flags & FX_AXIS_FROM_SPHERE != 0 {
                ax[0] = normalize(temp);
                let (right, up) = make_normal_vectors(ax[0]);
                ax[1] = right;
                ax[2] = up;
            }
        } else if fx.spawn_flags & FX_ORG_ON_CYLINDER != 0 {
            let radius = fx.radius.get(rng);
            let along = rng.flrand(-1.0, 1.0) * 0.5 * fx.height.get(rng);
            let pt = add(scale(ax[1], radius), scale(ax[0], along));
            let temp = rotate_point_around_vector(ax[0], pt, rng.flrand(0.0, 360.0));
            org = add(org, temp);
            if fx.spawn_flags & FX_AXIS_FROM_SPHERE != 0 {
                ax[0] = normalize(temp);
                let up = if ax[0][2] == 1.0 { [0.0, 1.0, 0.0] } else { [0.0, 0.0, 1.0] };
                ax[1] = cross(up, ax[0]);
                ax[2] = cross(ax[0], ax[1]);
            }
        }

        let mut vel = [0.0; 3];
        let mut accel = [0.0; 3];
        if matches!(fx.kind, PrimType::Particle | PrimType::OrientedParticle | PrimType::Tail | PrimType::Emitter) {
            let v = [fx.velocity[0].get(rng), fx.velocity[1].get(rng), fx.velocity[2].get(rng)];
            vel = if fx.spawn_flags & FX_VEL_IS_ABSOLUTE != 0 { v } else { combine(ax, v) };
            let a = [fx.acceleration[0].get(rng), fx.acceleration[1].get(rng), fx.acceleration[2].get(rng)];
            accel = if fx.spawn_flags & FX_ACCEL_IS_ABSOLUTE != 0 { a } else { combine(ax, a) };
            accel[2] += fx.gravity.get(rng);
            if late_time > 0 {
                let ftime = late_time as f32 * 0.001;
                let time2 = ftime * ftime * 0.5;
                vel = add(vel, scale(accel, ftime));
                for i in 0..3 {
                    org[i] += ftime * vel[i] + time2 * vel[i];
                }
            }
        }

        let mut org2 = [0.0; 3];
        if matches!(fx.kind, PrimType::Line | PrimType::Electricity) {
            if fx.spawn_flags & FX_ORG2_FROM_TRACE != 0 {
                // No world trace from the FX thread: use the untraced endpoint.
                org2 = add(org, scale(ax[0], MAX_TRACE_DIST));
            } else {
                let o2 = [fx.origin2[0].get(rng), fx.origin2[1].get(rng), fx.origin2[2].get(rng)];
                org2 = if fx.spawn_flags & FX_CHEAP_ORG2_CALC != 0 { o2 } else { combine(ax, o2) };
                org2 = add(org2, origin);
            }
        }

        let (rgb_start, rgb_end) = if fx.spawn_flags & FX_RGB_COMPONENT_INTERP != 0 {
            let percent = rng.flrand(0.0, 1.0);
            (
                [fx.rgb_start[0].at(percent), fx.rgb_start[1].at(percent), fx.rgb_start[2].at(percent)],
                [fx.rgb_end[0].at(percent), fx.rgb_end[1].at(percent), fx.rgb_end[2].at(percent)],
            )
        } else {
            (
                [fx.rgb_start[0].get(rng), fx.rgb_start[1].get(rng), fx.rgb_start[2].get(rng)],
                [fx.rgb_end[0].get(rng), fx.rgb_end[1].get(rng), fx.rgb_end[2].get(rng)],
            )
        };
        let pick = |list: &[EffectId], rng: &mut Rng| -> EffectId {
            if list.is_empty() { 0 } else { list[rng.irand(0, list.len() as i32 - 1) as usize] }
        };
        let shader = (!fx.media.is_empty()).then(|| fx.media[rng.irand(0, fx.media.len() as i32 - 1) as usize].clone());

        match fx.kind {
            PrimType::Sound => {
                if let Some(qpath) = shader {
                    self.sounds.push(FxSound { origin: org, qpath });
                }
                return;
            }
            PrimType::FxRunner => {
                let target = pick(&links.play, rng);
                if target != 0 {
                    self.runner_queue.push((target, org, ax, class));
                }
                return;
            }
            PrimType::Decal | PrimType::CameraShake | PrimType::ScreenFlash | PrimType::Electricity => {
                // Electricity is drawn as a straight line until RT_ELECTRICITY
                // (chaos/branching) is ported; the rest need systems that do
                // not exist yet (marks, view shake, 2D flash).
                if fx.kind != PrimType::Electricity {
                    self.unsupported_spawns += 1;
                    return;
                }
            }
            _ => {}
        }

        let life = fx.life.get(rng) as i32; // killTime is an int in FX_Add*
        let time = self.time;
        let parm = |value: f32, shift: u32| -> f32 {
            match (flags >> shift) & FX_PARM_MASK {
                FX_WAVE => value * std::f32::consts::PI * 0.001,
                0 => value,
                _ => value * 0.01 * life as f32 + time as f32,
            }
        };
        let mut angles = [fx.angle[0].get(rng), fx.angle[1].get(rng), fx.angle[2].get(rng)];
        if fx.kind == PrimType::Emitter {
            angles = add(angles, vectoangles(ax[0]));
        }
        let size = (fx.size_start.get(rng), fx.size_end.get(rng), parm(fx.size_parm.get(rng), FX_SIZE_SHIFT));
        let size2 = (fx.size2_start.get(rng), fx.size2_end.get(rng), parm(fx.size2_parm.get(rng), FX_SIZE2_SHIFT));
        let length = (fx.length_start.get(rng), fx.length_end.get(rng), parm(fx.length_parm.get(rng), FX_LENGTH_SHIFT));
        let alpha = (fx.alpha_start.get(rng), fx.alpha_end.get(rng), parm(fx.alpha_parm.get(rng), FX_ALPHA_SHIFT));
        let rgb_parm = parm(fx.rgb_parm.get(rng), FX_RGB_SHIFT);
        let primitive = Primitive {
            class,
            kind: fx.kind,
            flags,
            time_start: time,
            time_end: time + life,
            kill_time: time + life,
            shader,
            origin: org,
            origin2: org2,
            old_origin: org,
            vel,
            accel,
            normal: ax[0],
            axis: ax,
            size,
            size2,
            length,
            alpha,
            rgb_start,
            rgb_end,
            rgb_parm,
            rotation: fx.rotation.get(rng),
            rotation_delta: fx.rotation_delta.get(rng),
            death_fx: pick(&links.death, rng),
            emitter_fx: pick(&links.emitter, rng),
            density: fx.density.get(rng),
            variance: fx.variance.get(rng),
            old_time: time,
            old_velocity: vel,
            angles,
            angle_delta: [fx.angle_delta[0].get(rng), fx.angle_delta[1].get(rng), fx.angle_delta[2].get(rng)],
        };
        if self.active.len() >= MAX_ACTIVE {
            self.dropped += 1;
            return;
        }
        self.active.push(primitive);
    }
}

/// One frame of a live primitive. Returns false when it should be freed.
fn update_primitive(
    prim: &mut Primitive,
    time: i32,
    real_time: f32,
    frame_time: i32,
    rng: &mut Rng,
    out: &mut FxFrame,
    emits: &mut Vec<(EffectId, [f32; 3], Axis, FxClass)>,
    visible: bool,
) -> bool {
    if prim.time_start > time {
        return false;
    }
    let moving = matches!(prim.kind, PrimType::Particle | PrimType::OrientedParticle | PrimType::Tail | PrimType::Emitter);
    if prim.kind == PrimType::Tail || prim.kind == PrimType::Emitter {
        prim.old_origin = prim.origin;
        prim.old_velocity = prim.vel;
    }
    if moving && prim.time_start < time {
        // CParticle::UpdateOrigin without physics: v += a*dt; o += v*dt.
        prim.vel = add(prim.vel, scale(prim.accel, real_time));
        prim.origin = add(prim.origin, scale(prim.vel, real_time));
    }
    let percent = |parm: f32, start: f32, end: f32, shift: u32, rng: &mut Rng| -> f32 {
        let perc = interpolation(prim.flags >> shift, parm, prim.time_start, prim.time_end, time, rng);
        start * perc + end * (1.0 - perc)
    };
    match prim.kind {
        PrimType::Emitter => {
            if prim.old_origin == prim.origin {
                prim.angle_delta = scale(prim.angle_delta, 0.7);
            }
            prim.angles = add(prim.angles, scale(prim.angle_delta, frame_time as f32 * 0.01));
            prim.axis = angles_to_axis(prim.angles);
            if prim.flags & FX_EMIT_FX != 0 && prim.emitter_fx != 0 {
                let mut step = prim.density + rng.flrand(-prim.variance, prim.variance);
                step *= step;
                let mut dif = 0;
                let mut t = prim.old_time;
                while t <= time {
                    dif += TRAIL_RATE;
                    let v = add(prim.old_velocity, scale(prim.accel, dif as f32 * 0.001));
                    let ftime = dif as f32 * 0.001;
                    let time2 = ftime * ftime * 0.5;
                    let org = std::array::from_fn(|i| prim.old_origin[i] + ftime * v[i] + time2 * v[i]);
                    if distance_squared(org, prim.old_origin) >= step {
                        step = prim.density + rng.flrand(-prim.variance, prim.variance);
                        step *= step;
                        emits.push((prim.emitter_fx, org, prim.axis, prim.class));
                        prim.old_origin = org;
                        prim.old_velocity = v;
                        dif = 0;
                        prim.old_time = t;
                    }
                    t += TRAIL_RATE;
                }
            }
        }
        PrimType::Light => {
            let radius = percent(prim.size.2, prim.size.0, prim.size.1, FX_SIZE_SHIFT, rng);
            let rgb = rgb_at(prim, time, rng);
            if visible {
                out.lights.push(FxLight { kind: FxLightKind::AuthoredEffect, origin: prim.origin, radius, rgb, segment: None, blade_segments: None });
            }
        }
        _ => {
            let shader = prim.shader.clone().unwrap_or_default();
            let radius = percent(prim.size.2, prim.size.0, prim.size.1, FX_SIZE_SHIFT, rng);
            let rgba = rgba_at(prim, time, rng);
            match prim.kind {
                PrimType::Particle => {
                    rotate(prim, frame_time);
                    if visible {
                        out.draws.push(FxDraw::Sprite { origin: prim.origin, radius, rotation: prim.rotation, rgba, shader });
                    }
                }
                PrimType::OrientedParticle => {
                    rotate(prim, frame_time);
                    if visible {
                        let (right, up) = make_normal_vectors(prim.normal);
                        out.draws.push(FxDraw::OrientedQuad {
                            origin: prim.origin,
                            axis: [prim.normal, right, up],
                            radius,
                            rotation: prim.rotation,
                            rgba,
                            shader,
                        });
                    }
                }
                PrimType::Line | PrimType::Electricity => {
                    if visible {
                        out.draws.push(FxDraw::Line { start: prim.origin, end: prim.origin2, width: radius, rgba, shader });
                    }
                }
                PrimType::Tail => {
                    let length = percent(prim.length.2, prim.length.0, prim.length.1, FX_LENGTH_SHIFT, rng);
                    // CTail::CalcNewEndpoint: extend back along the last motion.
                    let back = normalize(sub(prim.old_origin, prim.origin));
                    let end = add(prim.origin, scale(back, length));
                    if visible {
                        out.draws.push(FxDraw::Line { start: prim.origin, end, width: radius, rgba, shader });
                    }
                }
                PrimType::Cylinder => {
                    let size2 = percent(prim.size2.2, prim.size2.0, prim.size2.1, FX_SIZE2_SHIFT, rng);
                    let length = percent(prim.length.2, prim.length.0, prim.length.1, FX_LENGTH_SHIFT, rng);
                    let axis = prim.axis[0];
                    if visible {
                        out.draws.push(FxDraw::Cylinder {
                            start: prim.origin,
                            end: add(prim.origin, scale(axis, length)),
                            axis,
                            // RB_SurfaceCylinder: size2 ring at origin, size1 at oldorigin.
                            start_radius: size2,
                            end_radius: radius,
                            rgba,
                            shader,
                        });
                    }
                }
                _ => {}
            }
        }
    }
    true
}

/// The shared UpdateSize/RGB/Alpha/Length percentage: 1 at spawn, 0 at end.
fn interpolation(flags: u32, parm: f32, start: i32, end: i32, time: i32, rng: &mut Rng) -> f32 {
    let mut perc1 = 1.0f32;
    let mut perc2 = 1.0f32;
    let span = (end - start) as f32;
    if flags & FX_LINEAR != 0 {
        perc1 = 1.0 - (time - start) as f32 / span;
    }
    match flags & FX_PARM_MASK {
        FX_NONLINEAR => {
            if time as f32 > parm {
                perc2 = 1.0 - (time as f32 - parm) / (end as f32 - parm);
            }
            perc1 = if flags & FX_LINEAR != 0 { perc1 * 0.5 + perc2 * 0.5 } else { perc2 };
        }
        FX_WAVE => perc1 *= ((time - start) as f32 * parm).cos(),
        FX_CLAMP => {
            perc2 = if (time as f32) < parm { (parm - time as f32) / (parm - start as f32) } else { 0.0 };
            perc1 = if flags & FX_LINEAR != 0 { perc1 * 0.5 + perc2 * 0.5 } else { perc2 };
        }
        _ => {}
    }
    // Alpha applies its random pick after clamping; size/rgb/length before.
    if flags & FX_RAND != 0 {
        perc1 = rng.flrand(0.0, perc1);
    }
    perc1
}

fn rgb_at(prim: &Primitive, time: i32, rng: &mut Rng) -> [f32; 3] {
    let perc = interpolation(prim.flags >> FX_RGB_SHIFT, prim.rgb_parm, prim.time_start, prim.time_end, time, rng);
    std::array::from_fn(|i| prim.rgb_start[i] * perc + prim.rgb_end[i] * (1.0 - perc))
}

/// UpdateRGB + UpdateAlpha into shaderRGBA bytes. Without useAlpha, alpha
/// fades the color and the alpha byte stays 0 (the zeroed refEntity).
fn rgba_at(prim: &Primitive, time: i32, rng: &mut Rng) -> [u8; 4] {
    let rgb = rgb_at(prim, time, rng);
    let mut rgba = [0u8; 4];
    for i in 0..3 {
        rgba[i] = ((rgb[i] * 255.0) as i32).clamp(0, 255) as u8;
    }
    // UpdateAlpha picks its random fraction after clamping the value.
    let alpha_flags = (prim.flags >> FX_ALPHA_SHIFT) & 0xF;
    let perc = interpolation(alpha_flags & !FX_RAND, prim.alpha.2, prim.time_start, prim.time_end, time, rng);
    let mut value = (prim.alpha.0 * perc + prim.alpha.1 * (1.0 - perc)).clamp(0.0, 1.0);
    if alpha_flags & FX_RAND != 0 {
        value = rng.flrand(0.0, value);
    }
    let alpha = ((value * 255.0) as i32).clamp(0, 255);
    if prim.flags & FX_USE_ALPHA != 0 {
        rgba[3] = alpha as u8;
    } else {
        for channel in &mut rgba[..3] {
            *channel = ((i32::from(*channel) * alpha) >> 8) as u8;
        }
    }
    rgba
}

/// CParticle::UpdateRotation.
fn rotate(prim: &mut Primitive, frame_time: i32) {
    prim.rotation += frame_time as f32 * 0.01 * prim.rotation_delta;
    prim.rotation_delta *= 1.0 - frame_time as f32 * 0.0007;
}

fn strip_extension(file: &str) -> &str {
    match file.rfind('.') {
        Some(dot) if !file[dot..].contains('/') => &file[..dot],
        _ => file,
    }
}

/// C `Round`: nearest integer, halves away from zero.
fn round(value: f32) -> i32 {
    value.round() as i32
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + b[i])
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    a.map(|v| v * s)
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn normalize(a: [f32; 3]) -> [f32; 3] {
    let length = dot(a, a).sqrt();
    if length > 0.0 { scale(a, 1.0 / length) } else { a }
}
fn distance_squared(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = sub(a, b);
    dot(d, d)
}
/// axis[0]*v0 + axis[1]*v1 + axis[2]*v2.
fn combine(axis: Axis, v: [f32; 3]) -> [f32; 3] {
    add(add(scale(axis[0], v[0]), scale(axis[1], v[1])), scale(axis[2], v[2]))
}

/// q_math MakeNormalVectors: right/up perpendicular to `forward`.
pub fn make_normal_vectors(forward: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    // Rotate forward around a bit to get a vector off its axis.
    let right0 = [forward[2], -forward[0], forward[1]];
    let d = dot(right0, forward);
    let right = normalize(sub(right0, scale(forward, d)));
    let up = cross(right, forward);
    (right, up)
}

/// q_math RotatePointAroundVector (degrees).
fn rotate_point_around_vector(dir: [f32; 3], point: [f32; 3], degrees: f32) -> [f32; 3] {
    let (s, c) = degrees.to_radians().sin_cos();
    let d = normalize(dir);
    // Rodrigues' rotation formula; identical to OpenJK's matrix form.
    let k_cross_p = cross(d, point);
    let k_dot_p = dot(d, point);
    std::array::from_fn(|i| point[i] * c + k_cross_p[i] * s + d[i] * k_dot_p * (1.0 - c))
}

/// q_math vectoangles.
fn vectoangles(value: [f32; 3]) -> [f32; 3] {
    if value[1] == 0.0 && value[0] == 0.0 {
        return [if value[2] > 0.0 { 270.0 } else { 90.0 }, 0.0, 0.0];
    }
    let mut yaw = if value[0] != 0.0 {
        value[1].atan2(value[0]).to_degrees()
    } else if value[1] > 0.0 {
        90.0
    } else {
        270.0
    };
    if yaw < 0.0 {
        yaw += 360.0;
    }
    let forward = (value[0] * value[0] + value[1] * value[1]).sqrt();
    let mut pitch = value[2].atan2(forward).to_degrees();
    if pitch < 0.0 {
        pitch += 360.0;
    }
    [-pitch, yaw, 0.0]
}

fn angles_to_axis(angles: [f32; 3]) -> Axis {
    let (sp, cp) = angles[0].to_radians().sin_cos();
    let (sy, cy) = angles[1].to_radians().sin_cos();
    let (sr, cr) = angles[2].to_radians().sin_cos();
    let forward = [cp * cy, cp * sy, -sp];
    let right = [-sr * sp * cy + cr * sy, -sr * sp * sy - cr * cy, -sr * cp];
    let up = [cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp];
    [forward, scale(right, -1.0), up]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_fixture(files: &'static [(&'static str, &'static str)]) -> impl FnMut(&str) -> Option<Vec<u8>> {
        move |path| files.iter().find(|(name, _)| *name == path).map(|(_, body)| body.as_bytes().to_vec())
    }

    const SHOT: &str = "Line\n{\n\torigin\t8 0 0\n\torigin2\t-80 0 0\n\tflags\tuseAlpha\n\twidth\n\t{\n\t\tstart\t1.5\n\t}\n\talpha\n\t{\n\t\tstart\t0.6\n\t}\n\tshader\n\t[\n\t\tgfx/effects/blaster_blob\n\t]\n}\nparticle\n{\n\torigin\t-5 0 0\n\tlife 100\n\tvelocity 100 0 0\n\tsize\n\t{\n\t\tstart\t2\n\t\tend 0\n\t\tflags linear\n\t}\n\tshader\n\t[\n\t\tgfx/effects/whiteGlow\n\t]\n}\n";

    #[test]
    fn registers_by_stripped_lowercase_name_with_effects_prefix() {
        let mut fx = FxSystem::new();
        let mut read = read_fixture(&[("effects/blaster/shot.efx", SHOT)]);
        let id = fx.register("Blaster/Shot", &mut read);
        assert_eq!(id, 1);
        assert_eq!(fx.register("blaster/shot.efx", &mut read), 1, "extension is stripped from the key");
        // OpenJK keys on the stripped path as given, so the prefixed spelling
        // is a second template of the same file.
        assert_eq!(fx.register("effects/blaster/shot.efx", &mut read), 2);
        assert_eq!(fx.register("missing/effect", &mut read), 0);
        assert_eq!(fx.effect_name(1), Some("blaster/shot"));
    }

    #[test]
    fn play_effect_spawns_oriented_line_and_moving_linear_particle() {
        let mut fx = FxSystem::new();
        let id = fx.register("blaster/shot", &mut read_fixture(&[("effects/blaster/shot.efx", SHOT)]));
        fx.adjust_time(1000);
        fx.adjust_time(1016);
        // Flying along +Y: template origins are along the forward axis.
        fx.play_effect_dir(id, [100.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let frame = fx.frame();
        assert_eq!(frame.draws.len(), 2, "drawn on the frame it spawns");
        let FxDraw::Line { start, end, width, rgba, shader } = &frame.draws[0] else { panic!() };
        assert!((start[1] - 8.0).abs() < 1e-5 && (end[1] + 80.0).abs() < 1e-5);
        assert_eq!(*width, 1.5);
        assert_eq!(rgba[3], 153, "useAlpha writes 0.6 into the alpha byte");
        assert_eq!(shader, "gfx/effects/blaster_blob");
        let FxDraw::Sprite { origin, radius, rgba, .. } = &frame.draws[1] else { panic!() };
        assert!((origin[1] + 5.0).abs() < 1e-5, "no motion on the spawn frame");
        assert_eq!(*radius, 2.0);
        assert_eq!(rgba[3], 0, "no useAlpha: alpha byte stays zero");

        fx.adjust_time(1066);
        assert_eq!(fx.frame().draws.len(), 2, "freed only once time > killTime");
        fx.adjust_time(1067);
        let frame = fx.frame();
        assert_eq!(frame.draws.len(), 1, "the 50 ms line died after its life");
        let FxDraw::Sprite { origin, radius, .. } = &frame.draws[0] else { panic!() };
        assert!((origin[1] - (-5.0 + 100.0 * 0.051)).abs() < 1e-3, "moved 51 ms at 100 u/s");
        assert!((radius - 0.98).abs() < 1e-3, "linear size 51% through its 100 ms life");

        fx.adjust_time(1200);
        assert!(fx.frame().draws.is_empty());
    }

    #[test]
    fn paused_clock_refuses_new_primitives_and_seek_back_clears() {
        let mut fx = FxSystem::new();
        let id = fx.register("blaster/shot", &mut read_fixture(&[("effects/blaster/shot.efx", SHOT)]));
        fx.adjust_time(500);
        fx.adjust_time(500);
        fx.play_effect_dir(id, [0.0; 3], [1.0, 0.0, 0.0]);
        assert!(fx.frame().draws.is_empty(), "FX_Add* refuse spawns while paused");
        fx.adjust_time(516);
        fx.play_effect_dir(id, [0.0; 3], [1.0, 0.0, 0.0]);
        assert_eq!(fx.stats().active, 2);
        fx.adjust_time(100);
        assert_eq!(fx.stats().active, 0);
    }

    /// Every stock effect must go through GP2 + templates + link resolution.
    #[test]
    #[ignore = "requires JKA_TEST_BASE with stock PK3s"]
    fn all_stock_effects_register() {
        use jka_assets::pk3::AssetSearchPath;
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let mut assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let names: Vec<String> = assets
            .names()
            .filter(|name| name.starts_with("effects/") && name.ends_with(".efx"))
            .map(str::to_owned)
            .collect();
        let mut kinds = std::collections::BTreeMap::<String, usize>::new();
        let mut failed = Vec::new();
        let mut total = 0;
        for chunk in names.chunks(200) {
            // FX_MAX_EFFECTS bounds one system; use a fresh one per chunk.
            let mut fx = FxSystem::new();
            for name in chunk {
                let id = fx.register(name, &mut |path| assets.read(path, 65536).ok().flatten().map(|asset| asset.bytes));
                if id == 0 {
                    failed.push(name.clone());
                    continue;
                }
                total += 1;
                for prim in &fx.effects[(id - 1) as usize].template.primitives {
                    *kinds.entry(format!("{:?}", prim.kind)).or_insert(0) += 1;
                }
            }
        }
        println!("FX REGISTRY: files={} registered={total} failed={failed:?} kinds={kinds:?}", names.len());
        assert!(names.len() > 300);
        assert!(failed.len() <= names.len() / 50, "too many unparsable stock effects: {failed:?}");
    }

    #[test]
    fn interpolation_modes_match_fxprimitives() {
        let mut rng = Rng::new(1);
        // Linear: 1 at start, 0 at end.
        assert_eq!(interpolation(FX_LINEAR, 0.0, 0, 100, 25, &mut rng), 0.75);
        // Nonlinear with parm at 50%: full until parm, then linear to 0.
        assert_eq!(interpolation(FX_NONLINEAR, 50.0, 0, 100, 25, &mut rng), 1.0);
        assert_eq!(interpolation(FX_NONLINEAR, 50.0, 0, 100, 75, &mut rng), 0.5);
        // Clamp: falls to 0 at parm and stays there.
        assert_eq!(interpolation(FX_CLAMP, 50.0, 0, 100, 25, &mut rng), 0.5);
        assert_eq!(interpolation(FX_CLAMP, 50.0, 0, 100, 60, &mut rng), 0.0);
        // No flags: start value throughout.
        assert_eq!(interpolation(0, 0.0, 0, 100, 60, &mut rng), 1.0);
    }

    #[test]
    fn normal_vectors_are_orthonormal() {
        for forward in [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], normalize([0.3, -0.5, 0.8])] {
            let (right, up) = make_normal_vectors(forward);
            assert!(dot(right, forward).abs() < 1e-5 && dot(up, forward).abs() < 1e-5 && dot(right, up).abs() < 1e-5);
            assert!((dot(right, right) - 1.0).abs() < 1e-5 && (dot(up, up) - 1.0).abs() < 1e-5);
        }
    }
}
