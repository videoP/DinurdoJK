//! Opt-in fluid blood for stock saber impacts and on-demand testing.
#[path = "ninja_blood_walk.rs"]
mod walk;
pub(crate) use walk::Droplet;
use walk::Walker;
use jka_movement::{TraceQuery, TraceResult, TraceWorld, ENTITY_WORLD};
use crate::ninja_fire_debug::CM_PER_MAP_UNIT;
use bytemuck::{Pod, Zeroable};
use glam::{Vec2, Vec3};
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

// Each field is GPU-backed; keep this a bounded, opt-in budget.
// Recycle oldest fading slots rather than rejecting new hits when full.
const MAX_POOLS: usize = 32;
const MAX_FADING_POOLS: usize = 8;
const FADE_MS: i32 = 5000;
// An emitter injects airborne droplets, never a half-second ground blob.
pub(crate) const EMIT_INTERVAL_MS: i32 = 33;
pub(crate) const RAGDOLL_INTERVAL_MS: i32 = 66;
/// (previous collider bottom, current bottom, elapsed milliseconds, smear width in cm).
/// A zero width still disturbs existing fluid without injecting new blood.
pub(crate) type BloodDragSample = ([f32; 3], [f32; 3], i32, f32);
const CARRIED_STEPS: u8 = 4;
// First two prints carry most of the pigment; the last two fade by step.
// The mark's lifetime fade is independent of this per-step transfer.
const CARRIED_STRENGTH: [f32; CARRIED_STEPS as usize] = [1.0, 0.9, 0.7, 0.4];
// This threshold now gates only player-foot interaction, never receiver admission.
const MIN_FLOOR_NORMAL_Z: f32 = 0.7;
const MAX_DROPLETS: usize = 256;
const MAX_WALKERS: usize = 16;
const STEP: f32 = 1.0 / 60.0;
const WALK_DELAY: u64 = 4;
const MAX_TRAIL_SEGMENTS: usize = 256;
const MAX_STAMPS: usize = 128;
// Ground-contact smear strokes per field (one per 33 ms contact sample).
const MAX_SMEAR_SEGMENTS: usize = 768;
// Fresh width gives full pigment; thinner (drier, older) smears are fainter.
const SMEAR_FULL_STRENGTH_CM: f32 = 26.0;
// Contact samples are 33 ms apart; anything older belongs to a stopped part.
const CONTACT_CACHE_MS: i32 = 250;
const MAX_CACHED_CONTACTS: usize = 256;
const CONTENTS_SOLID: i32 = 1;
const SURF_NO_MARK: i32 = 0x0008_0000 | 0x0010_0000;
// BSP/JKA adapter: Overgrowth's XML particle assets are not supplied. Use
// JKA's world unit conversion for standard gravity and a bounded spray.
const GRAVITY: f32 = 981.0 / CM_PER_MAP_UNIT;
const METER: f32 = 100.0 / CM_PER_MAP_UNIT;
pub(crate) const POINT_SIZE_CM: f32 = 48.0;
const SPURT_IMPACT_SIZE_CM: f32 = 64.0;
pub(crate) const SMALL_HIT_SCALE: f32 = 0.65;
const PAIN_MATCH_DISTANCE: f32 = 48.0;
pub(crate) const FIRST_TICK_SECONDS: f64 = 1.0 / 60.0;
pub(crate) const ENABLED_BIT: u64 = 1;
pub(crate) fn pain_hit_scale(health: i32) -> f32 {
    0.80 + (1.0 - health.clamp(0, 100) as f32 / 100.0) * 0.65
}

pub(crate) fn blood_pigment(wetness: f32) -> [f32; 3] {
    // Overgrowth defaults.txt and decals.glsl: red pigment and wetness ramp.
    [0.4 * (0.15 + 0.25 * wetness.clamp(0.0, 1.0)), 0.0, 0.0]
}
static CONTROL: AtomicU64 = AtomicU64::new(0);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn token() -> u64 {
    CONTROL.load(Ordering::Relaxed)
}
pub(crate) fn enabled() -> bool {
    token() & ENABLED_BIT != 0
}
pub(crate) fn set_enabled(requested: bool) {
    let enabled = u64::from(requested && jka_ninja::blood_material_available());
    let _ = CONTROL.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
        ((old & ENABLED_BIT) != enabled)
            .then(|| (old & !ENABLED_BIT).wrapping_add(2) | enabled)
    });
}

pub(crate) fn is_stock_saber_effect(effect: &str) -> bool {
    [
        "saber/blood_sparks_mp.efx",
        "saber/blood_sparks_25_mp.efx",
        "saber/blood_sparks_50_mp.efx",
    ]
    .iter()
    .any(|name| effect.eq_ignore_ascii_case(name))
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct BloodVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
}

#[derive(Clone, Copy)]
pub(crate) enum SplatKind {
    Ordinary,
    SpurtImpact,
    Scatter,
}

#[derive(Clone, Copy)]
pub(crate) struct BloodStamp {
    pub uv: [f32; 2],
    pub seed: jka_ninja::BloodSeed,
    pub stretch: f32,
    pub kind: SplatKind,
}

/// A wide ink stroke left by a sliding gib or corpse contact, in pool UV.
/// Rasterized into the same Ninja density input as stamps and trails, then
/// moved by the original fluid contact brush; never a static decal.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BloodSmear {
    pub a: [f32; 2],
    pub b: [f32; 2],
    pub width_cm: f32,
    pub strength: f32,
}

/// Append-only smear strokes in fixed-size shared chunks. A pool clone (the
/// render snapshot keeps the previous `pools`) then copies at most one
/// partially filled chunk on the next push, not the whole stroke history.
#[derive(Clone, Default)]
pub(crate) struct SmearLog {
    chunks: Vec<Arc<Vec<BloodSmear>>>,
    len: usize,
}
impl SmearLog {
    const CHUNK: usize = 64;
    pub fn len(&self) -> usize { self.len }
    fn push(&mut self, smear: BloodSmear) {
        match self.chunks.last_mut().filter(|chunk| chunk.len() < Self::CHUNK) {
            Some(chunk) => Arc::make_mut(chunk).push(smear),
            None => {
                let mut chunk = Vec::with_capacity(Self::CHUNK);
                chunk.push(smear);
                self.chunks.push(Arc::new(chunk));
            }
        }
        self.len += 1;
    }
    pub fn iter(&self) -> impl Iterator<Item = &BloodSmear> { self.iter_from(0) }
    /// Strokes from index `start` onward, skipping whole consumed chunks.
    pub fn iter_from(&self, start: usize) -> impl Iterator<Item = &BloodSmear> {
        self.chunks.iter().skip(start / Self::CHUNK).flat_map(|chunk| chunk.iter())
            .skip(start % Self::CHUNK)
    }
}

#[derive(Clone)]
pub(crate) struct BloodPool {
    pub id: u64,
    pub origin: [f32; 3],
    pub tangent: Vec3,
    pub normal: Vec3,
    pub bitangent: Vec3,
    pub vertices: Arc<[BloodVertex]>,
    pub seed: jka_ninja::BloodSeed,
    /// Same-frame Ninja DataChannel contact brushes, bounded to the field budget.
    pub motion: Option<(i32, Vec<jka_ninja::BloodMotion>)>,
    pub fade_started_ms: Option<i32>,
    pub seeded_impact: bool,
    pub birth_ms: i32,
    pub last_ink_ms: i32,
    /// Append-only segments, in UV. Renderer acknowledges the consumed prefix.
    pub trail: Arc<Vec<[[f32; 2]; 2]>>,
    pub stretch: f32,
    pub kind: SplatKind,
    pub stamps: Arc<Vec<BloodStamp>>,
    /// Append-only, like `trail`; the renderer acknowledges the consumed prefix.
    pub smears: SmearLog,
    extent: Vec2,
}

impl BloodPool {
    pub fn project(
        floor: TraceResult,
        surfaces: &jka_assets::bsp::MarkSurfaces,
        impact_direction: Option<[f32; 3]>,
    ) -> Option<Self> {
        Self::project_surface(floor.end, floor.normal, surfaces, impact_direction)
    }

    fn project_surface(point: [f32; 3], surface_normal: [f32; 3],
        surfaces: &jka_assets::bsp::MarkSurfaces, impact_direction: Option<[f32; 3]>) -> Option<Self> {
        let center = Vec3::from_array(point);
        let normal = Vec3::from_array(surface_normal).try_normalize()?;
        if !normal.is_finite() || !center.is_finite() {
            return None;
        }
        let [width, height, _] = jka_ninja::blood_desc()?.extent_cm;
        let extent = Vec2::new(width, height) / CM_PER_MAP_UNIT;
        // Preserve the Ninja UV handedness but avoid a pole on ±Y walls.
        let helper = if normal.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
        let tangent = helper.cross(normal).normalize();
        let bitangent = tangent.cross(normal);
        let u = tangent * (extent.x * 0.5);
        let v = bitangent * (extent.y * 0.5);
        let corners = [
            center - u - v,
            center + u - v,
            center + u + v,
            center - u + v,
        ]
        .map(|p| p.to_array());
        let mut projected = jka_assets::bsp::MarkBuffer::default();
        surfaces.project_blood(&corners, (-normal * 20.0).to_array(), &mut projected);
        let mut vertices = Vec::new();
        for (fragment, surface_normal) in projected.iter() {
            let surface_normal = Vec3::from_array(surface_normal);
            if surface_normal.dot(normal) < 0.9
                || fragment
                    .iter()
                    .any(|&p| (Vec3::from_array(p) - center).dot(normal).abs() > 6.0)
            {
                continue;
            }
            for pair in fragment[1..].windows(2) {
                let a = Vec3::from_array(fragment[0]);
                let b = Vec3::from_array(pair[0]);
                let c = Vec3::from_array(pair[1]);
                let triangle = if (b - a).cross(c - a).dot(surface_normal) >= 0.0 {
                    [a, b, c]
                } else {
                    [a, c, b]
                };
                vertices.extend(triangle.map(|p| {
                    let d = p - center;
                    BloodVertex {
                        position: crate::scene::render_position(
                            (p + surface_normal * 0.15).to_array(),
                        ),
                        uv: (Vec2::new(d.dot(tangent), d.dot(bitangent)) / extent
                            + Vec2::splat(0.5))
                        .to_array(),
                    }
                }));
            }
        }
        if vertices.is_empty() {
            return None;
        }
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let direction_uv = impact_direction
            .map(Vec3::from_array)
            .filter(|dir| dir.is_finite())
            .and_then(|dir| Vec2::new(dir.dot(tangent), dir.dot(bitangent)).try_normalize())
            .unwrap_or(Vec2::ZERO)
            .to_array();
        Some(Self {
            id,
            seed: jka_ninja::BloodSeed {
                point_size_cm: POINT_SIZE_CM,
                variation: id,
                direction_uv,
                high_quality: normal.z < 0.7,
            },
            origin: point,
            tangent,
            normal,
            bitangent,
            vertices: vertices.into(),
            motion: None,
            fade_started_ms: None,
            seeded_impact: true,
            birth_ms: 0,
            last_ink_ms: 0,
            trail: Arc::new(Vec::new()),
            stretch: 1.0,
            kind: SplatKind::Ordinary,
            stamps: Arc::new(Vec::new()),
            smears: SmearLog::default(),
            extent,
        })
    }

    pub fn opacity(&self, time_ms: i32) -> f32 {
        self.fade_started_ms.map_or(1.0, |start| {
            1.0 - time_ms.saturating_sub(start).clamp(0, FADE_MS) as f32 / FADE_MS as f32
        })
    }

    fn uv(&self, p: Vec3) -> Vec2 {
        let d = p - Vec3::from_array(self.origin);
        Vec2::new(d.dot(self.tangent), d.dot(self.bitangent)) / self.extent + Vec2::splat(0.5)
    }

    fn contains(&self, p: Vec3, normal: Vec3) -> bool {
        let uv = self.uv(p);
        self.normal.dot(normal) > 0.98 && (p - Vec3::from_array(self.origin)).dot(self.normal).abs() < 1.0
            && uv.min_element() > 0.20 && uv.max_element() < 0.80
    }

    // CPU footprint pickup is conservatively restricted to known injection
    // sites, rather than treating an entire 5m Ninja field as covered in blood.
    // The physical density still belongs exclusively to the GPU Ninja field.
    fn wet_contact(&self, foot: Vec3, surface_normal: Vec3) -> bool {
        if self.fade_started_ms.is_some() || self.normal.dot(surface_normal) < 0.97
            || (foot - Vec3::from_array(self.origin)).dot(self.normal).abs() > 2.5
        { return false; }
        let uv = self.uv(foot);
        if uv.min_element() < 0.0 || uv.max_element() > 1.0 { return false; }
        let foot_radius_cm = 6.0 * CM_PER_MAP_UNIT;
        let near = |center: Vec2, cm: f32| ((uv - center) * self.extent).length_squared()
            <= (cm * 0.55 + foot_radius_cm).powi(2);
        if self.seeded_impact && near(Vec2::splat(0.5), self.seed.point_size_cm) { return true; }
        if self.stamps.iter().any(|s| near(Vec2::from_array(s.uv), s.seed.point_size_cm)) { return true; }
        let p = uv * self.extent;
        let segment_distance = |a: [f32; 2], b: [f32; 2]| {
            let a = Vec2::from_array(a) * self.extent;
            let b = Vec2::from_array(b) * self.extent;
            let d = b - a;
            let t = if d.length_squared() > 1.0e-5 {
                ((p-a).dot(d) / d.length_squared()).clamp(0.0, 1.0)
            } else { 0.0 };
            (p - a - d*t).length()
        };
        // `extent` is in map units; smear widths are centimetres.
        self.trail.iter().any(|segment| segment_distance(segment[0], segment[1]) < foot_radius_cm)
            || self.smears.iter().any(|smear| segment_distance(smear.a, smear.b)
                < smear.width_cm * 0.5 / CM_PER_MAP_UNIT + foot_radius_cm)
    }

    fn motion_at(&self, feet: Vec3, velocity: Vec3) -> Option<jka_ninja::BloodMotion> {
        if self.normal.z < MIN_FLOOR_NORMAL_Z { return None; }
        let offset = feet - Vec3::from_array(self.origin);
        if offset.dot(self.bitangent.cross(self.tangent)).abs() > 8.0 {
            return None;
        }
        let uv = Vec2::new(offset.dot(self.tangent), offset.dot(self.bitangent)) / self.extent
            + Vec2::splat(0.5);
        if uv.min_element() <= 0.0 || uv.max_element() >= 1.0 {
            return None;
        }
        let speed = Vec2::new(velocity.dot(self.tangent), velocity.dot(self.bitangent));
        (speed.length_squared() >= 8.0 * 8.0).then_some(jka_ninja::BloodMotion {
            uv: uv.to_array(),
            velocity_cm_s: (speed * CM_PER_MAP_UNIT * 0.375).clamp_length_max(120.0).to_array(),
        })
    }
}

#[derive(Clone)]
pub(crate) struct BloodScene {
    pub control: u64,
    pub time_ms: i32,
    pub pools: Arc<Vec<BloodPool>>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum BleedStyle {
    #[default]
    Leak,
    Gush,
}

struct BloodHit {
    victim: Option<u16>,
    time: i32,
    origin: [f32; 3],
    direction: [f32; 3],
    count: i32,
    scale: f32,
}

#[derive(Default)]
pub(crate) struct BloodEvents {
    control: u64,
    time_ms: i32,
    pools: Option<Arc<Vec<BloodPool>>>,
    pub local_emit_ms: Option<i32>,
    pub local_started_ms: Option<i32>,
    pub local_style: BleedStyle,
    carried: HashMap<(u16, bool), (u8, i32)>,
    pub ragdoll_emit_ms: Option<i32>,
    drops: Vec<Droplet>,
    hits: Vec<BloodHit>,
    pains: Vec<(u16, i32, [f32; 3], i32)>,
    spurts: crate::ninja_blood_air::SpurtEffects,
    walkers: Vec<Walker>,
    sim_ms: Option<i32>,
    accumulator: f32,
    sim_tick: u64,
    /// Floor traces of the last sampled contact points. Each drag sample
    /// starts exactly where that part's previous one ended, so its start
    /// contact is reused instead of traced again.
    contacts: Vec<([f32; 3], i32, Option<TraceResult>)>,
}
impl BloodEvents {
    pub(crate) fn load_spurt(&mut self, base: &std::path::Path, game: Option<&std::path::Path>) -> Result<(), String> {
        self.spurts.load(base, game)
    }

    pub(crate) fn disable(&mut self) {
        self.clear();
        self.spurts = Default::default();
    }

    pub fn clear(&mut self) {
        self.carried.clear();
        self.pools = None;
        self.local_emit_ms = None;
        self.local_started_ms = None;
        self.ragdoll_emit_ms = None;
        self.drops.clear();
        self.hits.clear();
        self.pains.clear();
        self.spurts.clear();
        self.walkers.clear();
        self.sim_ms = None;
        self.accumulator = 0.0;
        self.sim_tick = 0;
        self.contacts.clear();
    }
    pub fn begin_frame(&mut self, time_ms: i32) {
        let control = token();
        if control & ENABLED_BIT == 0 && control == self.control && self.pools.is_none() {
            return;
        }
        if control != self.control || time_ms < self.time_ms || control & ENABLED_BIT == 0 {
            self.clear();
        }
        self.control = control;
        self.time_ms = time_ms;
        if let Some(pools) = self.pools.as_mut() {
            if pools.iter().any(|pool| pool.opacity(time_ms) == 0.0) {
                Arc::make_mut(pools).retain(|pool| pool.opacity(time_ms) > 0.0);
            }
        }
    }
    pub fn record(&mut self, mut pool: BloodPool, time_ms: i32) -> bool {
        self.begin_frame(time_ms);
        if self.control & ENABLED_BIT == 0 {
            return false;
        }
        let pools = self
            .pools
            .get_or_insert_with(|| Arc::new(Vec::with_capacity(MAX_POOLS + MAX_FADING_POOLS)));
        let pools = Arc::make_mut(pools);
        // Keep a fixed GPU slot budget. If the fading reserve is full,
        // retire its oldest member so new blood can still land. The renderer
        // reuses that slot on the next scene handoff.
        if pools.len() >= MAX_POOLS + MAX_FADING_POOLS {
            let oldest_fading = pools.iter().enumerate()
                .filter_map(|(i, p)| p.fade_started_ms.map(|t| (i, t)))
                .min_by_key(|&(_, t)| t).map(|(i, _)| i);
            if let Some(i) = oldest_fading {
                pools.remove(i);
            } else {
                // Defensive fallback for a fully live set (also bounded).
                pools.remove(0);
            }
        }
        let live_count = pools.iter().filter(|p| p.fade_started_ms.is_none()).count();
        if live_count >= MAX_POOLS {
            if let Some(oldest) = pools.iter_mut().find(|p| p.fade_started_ms.is_none()) {
                oldest.fade_started_ms = Some(time_ms);
                oldest.motion = None;
            }
        }
        pool.birth_ms = time_ms;
        pool.last_ink_ms = time_ms;
        pools.push(pool);
        true
    }

    // Overgrowth's particle collision order is retained: emit above the
    // surface, integrate velocity and gravity, then make a Ninja density
    // injection only when a swept trace hits world geometry.
    pub fn bleed(&mut self, origin: [f32; 3], inherited_velocity: [f32; 3], forward: [f32; 3], style: BleedStyle) {
        self.bleed_pulsed(origin, inherited_velocity, forward, style, 1.0);
    }

    // Only the constant diagnostic gush uses the cardiac envelope. Actual
    // ragdoll leaks and saber impacts keep their independent cadence.
    pub fn bleed_pulsed(&mut self, origin: [f32; 3], inherited_velocity: [f32; 3], forward: [f32; 3], style: BleedStyle, pulse: f32) {
        if !enabled() { return; }
        let pulse = pulse.clamp(0.0, 1.0);
        if style == BleedStyle::Gush && pulse < 0.08 { return; }
        let origin = Vec3::from_array(origin);
        let inheritance = Vec3::from_array(inherited_velocity);
        let forward = Vec3::from_array(forward);
        if !origin.is_finite() || !inheritance.is_finite() || !forward.is_finite() { return; }
        let forward = Vec3::new(forward.x, forward.y, 0.0).normalize_or_zero();
        let mut rng = crate::fx::template::Rng::new(NEXT_ID.fetch_add(1, Ordering::Relaxed));
        let count = match style {
            BleedStyle::Leak => 2,
            BleedStyle::Gush => (1.0 + 4.0 * pulse).round() as usize,
        };
        let speed = match style { BleedStyle::Leak => (0.30, 0.80), BleedStyle::Gush => (1.0, 3.0) };
        let pulse_speed = if style == BleedStyle::Gush { 0.6 + 0.9 * pulse } else { 1.0 };
        for i in 0..count {
            if self.drops.len() >= MAX_DROPLETS { break; }
            let phi = rng.flrand(0.0, std::f32::consts::TAU);
            let lateral = Vec3::new(phi.cos(), phi.sin(), 0.0) * METER * rng.flrand(speed.0, speed.1);
            let horizontal = if style == BleedStyle::Gush && forward.length_squared() > 0.5 {
                forward * METER * rng.flrand(1.5, 3.0) + lateral * 0.25
            } else { lateral };
            let rise = match style {
                BleedStyle::Leak => rng.flrand(-0.3, 0.1),
                BleedStyle::Gush => rng.flrand(0.1, 1.2),
            };
            let velocity = inheritance * 0.35 + (horizontal + Vec3::Z * METER * rise) * pulse_speed;
            self.drops.push(Droplet::new(origin, velocity,
                if i == 0 { 15.0 } else { rng.flrand(4.0, 11.0) }, i == 0));
        }
    }

    // All contacts (player feet, grounded Rapier body parts and gib proxies)
    // enter the same original Ninja fluid-displacement brush path. A pool can
    // receive multiple contacts within one presentation frame; do not let the
    // first moving player/gib consume the only slot for every other body.
    pub fn disturb(&mut self, feet: [f32; 3], velocity: [f32; 3]) {
        let (feet, velocity) = (Vec3::from_array(feet), Vec3::from_array(velocity));
        if !feet.is_finite() || !velocity.is_finite() || velocity.length_squared() < 8.0 * 8.0 {
            return;
        }
        let Some(pools) = self.pools.as_mut() else { return; };
        for index in 0..pools.len() {
            let pool = &pools[index];
            if pool.fade_started_ms.is_some() { continue; }
            let Some(motion) = pool.motion_at(feet, velocity) else { continue; };
            let pool = &mut Arc::make_mut(pools)[index];
            let extent = pool.extent;
            match &mut pool.motion {
                Some((when, contacts)) if *when == self.time_ms => {
                    // Avoid double-disturbing a followed player submitted both
                    // as the predicted entity and as a presentation entity.
                    if contacts.len() < jka_ninja::BLOOD_MAX_CONTACTS
                        && !contacts.iter().any(|old| {
                            let du = (old.uv[0] - motion.uv[0]) * extent.x;
                            let dv = (old.uv[1] - motion.uv[1]) * extent.y;
                            du * du + dv * dv < 1.0
                                && old.velocity_cm_s == motion.velocity_cm_s
                        }) {
                        contacts.push(motion);
                    }
                }
                _ => pool.motion = Some((self.time_ms, vec![motion])),
            }
        }
    }

    // Real footfalls pick up pigment independently for each sole.
    pub fn footfall(&mut self, entity: u16, right_foot: bool, contact: [f32; 3], surface_normal: [f32; 3],
        velocity: [f32; 3], time_ms: i32) -> Option<f32> {
        self.begin_frame(time_ms);
        if !enabled() { return None; }
        let p = Vec3::from_array(contact);
        let n = Vec3::from_array(surface_normal).try_normalize()?;
        let v = Vec3::from_array(velocity);
        if !p.is_finite() || !v.is_finite() || n.z < MIN_FLOOR_NORMAL_Z { return None; }
        let mut contact_wet = false;
        if let Some(pools) = self.pools.as_mut() {
            for idx in 0..pools.len() {
                if pools[idx].fade_started_ms.is_some() { continue; }
                // An original stamp is a useful CPU pickup approximation,
                // but NOT the extent of the GPU-simulated stain. A foot may
                // smear redistributed blood anywhere in this live field.
                contact_wet |= pools[idx].wet_contact(p, n);
            }
        }
        // Reuse the same contact scheduling as sustained player movement.
        self.disturb(contact, velocity);
        let remaining = self.carried.entry((entity, right_foot)).or_insert((0, time_ms));
        // Expire stale carry on a demo seek, death or long gap between steps.
        if time_ms < remaining.1 || time_ms - remaining.1 > 2500 { remaining.0 = 0; }
        remaining.1 = time_ms;
        if contact_wet { remaining.0 = CARRIED_STEPS; }
        if remaining.0 == 0 { return None; }
        let step_index = (CARRIED_STEPS - remaining.0) as usize;
        let strength = CARRIED_STRENGTH[step_index];
        remaining.0 -= 1;
        Some(strength)
    }

    pub(crate) fn note_hit(&mut self, victim: Option<u16>, time: i32, origin: [f32; 3], direction: [f32; 3], count: i32) {
        if self.hits.len() < MAX_DROPLETS {
            self.hits.push(BloodHit { victim, time, origin, direction, count, scale: SMALL_HIT_SCALE });
        }
    }

    pub(crate) fn note_pain(&mut self, victim: u16, time: i32, origin: [f32; 3], health: i32) {
        if self.pains.len() < MAX_DROPLETS
            && !self.pains.iter().any(|&(number, stamp, _, _)| number == victim && stamp == time)
        {
            self.pains.push((victim, time, origin, health));
        }
    }

    pub(crate) fn emit_hits(&mut self) {
        let mut hits = std::mem::take(&mut self.hits);
        let mut pains = std::mem::take(&mut self.pains);
        for (victim, time, origin, health) in pains.drain(..) {
            let scale = pain_hit_scale(health);
            let available = |hit: &BloodHit| hit.time == time && hit.scale == SMALL_HIT_SCALE;
            let point = Vec3::from_array(origin);
            let index = hits.iter().rposition(|hit| available(hit) && hit.victim == Some(victim))
                .or_else(|| hits.iter().enumerate()
                    .filter(|(_, hit)| available(hit) && hit.victim.is_none())
                    .map(|(i, hit)| (i, point.distance_squared(Vec3::from_array(hit.origin))))
                    .filter(|(_, distance)| *distance <= PAIN_MATCH_DISTANCE.powi(2))
                    .min_by(|a, b| a.1.total_cmp(&b.1)).map(|(i, _)| i));
            if let Some(index) = index {
                hits[index].scale = scale;
            } else if hits.len() < MAX_DROPLETS {
                hits.push(BloodHit { victim: Some(victim), time, origin, direction: [0.0, 0.0, 1.0], count: 1, scale });
            }
        }
        for hit in hits.drain(..) {
            self.spray(hit.origin, hit.direction, hit.count, hit.scale);
        }
        self.hits = hits;
        self.pains = pains;
    }

    /// The event direction is a normal, not a speed. The host maps it to a
    /// small fan of initial velocities; Overgrowth owns the subsequent update
    /// order (position, inertia, gravity, sweep collision).
    fn spray(&mut self, origin: [f32; 3], direction: [f32; 3], count: i32, size_scale: f32) {
        if !enabled() { return; }
        let pos = Vec3::from_array(origin);
        if !pos.is_finite() { return; }
        let dir = Vec3::from_array(direction).try_normalize().unwrap_or(Vec3::Z);
        let helper = if dir.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
        let side = dir.cross(helper).normalize();
        let up = side.cross(dir).normalize();
        let mut rng = crate::fx::template::Rng::new(NEXT_ID.fetch_add(1, Ordering::Relaxed));
        let mut has_spurt = false;
        for i in 0..(count.clamp(1, 3) * 3) {
            if self.drops.len() >= MAX_DROPLETS { break; }
            let vel = (dir + side * rng.flrand(-0.65, 0.65) + up * rng.flrand(-0.65, 0.65))
                .normalize_or_zero() * (METER * rng.flrand(1.5, 4.5));
            let size_cm = if i == 0 { POINT_SIZE_CM } else { rng.flrand(8.0, 22.0) };
            let mut drop = Droplet::new(pos, vel, size_cm * size_scale, i == 0);
            if i == 0 {
                drop.spurt = self.spurts.spawn(pos, dir, self.time_ms, size_scale);
                has_spurt = drop.spurt.is_some();
                if has_spurt {
                    drop.size_cm = SPURT_IMPACT_SIZE_CM * size_scale;
                    drop.splat = SplatKind::SpurtImpact;
                }
            } else if has_spurt {
                drop.splat = SplatKind::Scatter;
            }
            self.drops.push(drop);
        }
    }

    pub fn update(&mut self, time_ms: i32, world: &mut impl TraceWorld, surfaces: &jka_assets::bsp::MarkSurfaces) {
        self.begin_frame(time_ms);
        if !enabled() || (self.drops.is_empty() && self.walkers.is_empty()) {
            self.sim_ms = Some(time_ms); self.accumulator = 0.0; return;
        }
        let last = self.sim_ms.replace(time_ms).unwrap_or(time_ms);
        self.accumulator += time_ms.saturating_sub(last).clamp(0, 100) as f32 / 1000.0;
        while self.accumulator >= STEP {
            self.accumulator -= STEP;
            self.sim_tick = self.sim_tick.wrapping_add(1);
            let mut i = 0;
            while i < self.drops.len() {
                let drop = &mut self.drops[i];
                let old = drop.pos;
                drop.pos += drop.velocity * STEP;
                drop.velocity -= Vec3::Z * GRAVITY * STEP;
                drop.remaining -= STEP;
                let trace = world.trace(TraceQuery { start: old.to_array(), end: drop.pos.to_array(),
                    mins: [0.0; 3], maxs: [0.0; 3], pass_entity: -1, mask: CONTENTS_SOLID });
                if trace.fraction < 1.0 || trace.start_solid != 0 || drop.remaining <= 0.0 {
                    let drop = self.drops.swap_remove(i);
                    let deposited = legal_hit(trace)
                        && self.impact(trace, drop.velocity, drop.size_cm, drop.may_walk, drop.splat, surfaces, time_ms);
                    if let Some(id) = drop.spurt {
                        let contact = deposited.then(|| (Vec3::from_array(trace.end), Vec3::from_array(trace.normal).normalize_or_zero()));
                        self.spurts.finish(id, contact, time_ms);
                    }
                } else {
                    if let Some(id) = drop.spurt {
                        self.spurts.move_to(id, drop.pos);
                    }
                    i += 1;
                }
            }
            if self.sim_tick % WALK_DELAY != 0 { continue; }
            let count = self.walkers.len();
            let mut segments = Vec::with_capacity(10);
            for i in (0..count).rev() {
                let mut walker = self.walkers.swap_remove(i);
                segments.clear();
                let result = walker.advance(surfaces, &mut segments);
                for &(a, b, normal) in &segments { self.add_trail(a, b, normal, surfaces, time_ms); }
                match result {
                    walk::Status::Continue => self.walkers.push(walker),
                    walk::Status::Done => {}
                    walk::Status::Drip(pos, normal) => {
                        if self.drops.len() < MAX_DROPLETS {
                            // Overgrowth starts detached blood with zero velocity.
                            self.drops.push(Droplet::new(pos + normal * 0.5, Vec3::ZERO, 12.0, false));
                        }
                    }
                }
            }
        }
    }

    pub fn impact(&mut self, trace: TraceResult, velocity: Vec3, size_cm: f32, may_walk: bool, kind: SplatKind,
        surfaces: &jka_assets::bsp::MarkSurfaces, time_ms: i32) -> bool {
        if !legal_hit(trace) { return false; }
        self.begin_frame(time_ms);
        if self.control & ENABLED_BIT == 0 { return false; }
        let p = Vec3::from_array(trace.end);
        let Some(normal) = Vec3::from_array(trace.normal).try_normalize() else { return false; };
        let stretch = (1.0 + (velocity - normal * velocity.dot(normal)).length()
            / velocity.dot(normal).abs().max(METER)).clamp(1.0, 4.0);
        // A cluster of particles on the same plane shares its density field.
        // A new plane or a hit outside the usable footprint gets a new slot.
        let found = self.pools.as_ref().and_then(|pools| pools.iter().rposition(|pool|
            pool.fade_started_ms.is_none() && pool.stamps.len() < MAX_STAMPS && pool.contains(p, normal)));
        if let Some(index) = found {
            let pool = &mut Arc::make_mut(self.pools.as_mut().unwrap())[index];
            let seed = jka_ninja::BloodSeed { point_size_cm: size_cm,
                variation: NEXT_ID.fetch_add(1, Ordering::Relaxed),
                direction_uv: Vec2::new(velocity.dot(pool.tangent), velocity.dot(pool.bitangent)).normalize_or_zero().to_array(),
                high_quality: pool.seed.high_quality };
            let stamp = BloodStamp { uv: pool.uv(p).to_array(), seed, stretch, kind };
            Arc::make_mut(&mut pool.stamps).push(stamp);
            pool.last_ink_ms = time_ms;
        } else {
            let Some(mut pool) = BloodPool::project(trace, surfaces, Some(velocity.to_array())) else { return false; };
            pool.seed.point_size_cm = size_cm;
            pool.stretch = stretch;
            pool.kind = kind;
            if !self.record(pool, time_ms) { return false; }
        }
        if may_walk && normal.z < 0.999 && self.walkers.len() < MAX_WALKERS {
            if let Some(w) = Walker::at(surfaces, Vec3::from_array(trace.end), normal) {
                self.walkers.push(w);
            }
        }
        true
    }

    /// Grounded Rapier/legacy motion uses *two original Ninja inputs*:
    /// 1. a wide smear stroke along the contact path is injected into the
    ///    field's density/ink channel, creating a floor pool on clean ground
    ///    while the fragment or corpse is still wet;
    /// 2. disturb() feeds the original foot brush to advect that density, so
    ///    the stroke is dragged and feathered by the fluid, not a flat decal.
    pub(crate) fn disturb_ground_motion(
        &mut self, start: [f32; 3], end: [f32; 3], elapsed_ms: i32,
        smear_width_cm: f32, world: &mut impl TraceWorld,
        surfaces: Option<&jka_assets::bsp::MarkSurfaces>, time: i32,
    ) -> bool {
        let cached = self.contacts.iter().position(|entry| entry.0 == start)
            .map(|i| self.contacts.swap_remove(i).2);
        let end_key = end;
        let (start, end) = (Vec3::from_array(start), Vec3::from_array(end));
        if !enabled() || !start.is_finite() || !end.is_finite()
            || !(1..=250).contains(&elapsed_ms)
        { return false; }
        let displacement = end - start;
        let horizontal = Vec3::new(displacement.x, displacement.y, 0.0);
        let distance = horizontal.length();
        // Ignore sleeping contacts, teleport jumps and high-speed airborne arcs.
        if !(0.35..=48.0).contains(&distance) { return false; }
        let a = cached.unwrap_or_else(|| gib_floor_contact(start, world));
        let b = gib_floor_contact(end, world);
        self.contacts.retain(|entry| time.saturating_sub(entry.1) <= CONTACT_CACHE_MS && entry.1 <= time);
        if self.contacts.len() >= MAX_CACHED_CONTACTS { self.contacts.remove(0); }
        self.contacts.push((end_key, time, b));
        let (Some(a), Some(b)) = (a, b) else { return false; };
        let na = Vec3::from_array(a.normal).normalize_or_zero();
        let nb = Vec3::from_array(b.normal).normalize_or_zero();
        if na.dot(nb) < 0.96 { return false; }
        self.begin_frame(time);
        let velocity = horizontal * (1000.0 / elapsed_ms as f32);
        if smear_width_cm.is_finite() && smear_width_cm >= 1.0 && distance >= 0.75 {
            if let Some(surfaces) = surfaces {
                let strength = (0.35 + 0.65 * smear_width_cm / SMEAR_FULL_STRENGTH_CM).min(1.0);
                self.add_smear(Vec3::from_array(a.end), Vec3::from_array(b.end), nb,
                    smear_width_cm.min(48.0), strength, surfaces, time);
            }
        }
        // Still displace wet puddles after this part's bleeding is exhausted.
        self.disturb(b.end, velocity.to_array());
        true
    }

    /// Append one contact-path stroke to the live floor field covering both
    /// ends. A saturated field skips new ink (the brush still moves it) rather
    /// than stacking a second overlapping GPU field on the same floor.
    fn add_smear(&mut self, a: Vec3, b: Vec3, n: Vec3, width_cm: f32, strength: f32,
        surfaces: &jka_assets::bsp::MarkSurfaces, time: i32) -> bool {
        let covering = |p: &BloodPool| p.fade_started_ms.is_none() && p.contains(a, n) && p.contains(b, n);
        let found = self.pools.as_ref().and_then(|pools| pools.iter().rposition(covering));
        let index = match found {
            Some(i) if self.pools.as_ref().unwrap()[i].smears.len() >= MAX_SMEAR_SEGMENTS => return false,
            Some(i) => i,
            None => {
                let Some(mut pool) = BloodPool::project_surface(((a + b) * 0.5).to_array(), n.to_array(), surfaces, None)
                else { return false; };
                pool.seeded_impact = false;
                if !self.record(pool, time) { return false; }
                self.pools.as_ref().unwrap().len() - 1
            }
        };
        let pool = &mut Arc::make_mut(self.pools.as_mut().unwrap())[index];
        let smear = BloodSmear { a: pool.uv(a).to_array(), b: pool.uv(b).to_array(), width_cm, strength };
        pool.smears.push(smear);
        pool.last_ink_ms = time;
        true
    }

    fn add_trail(&mut self, a: Vec3, b: Vec3, n: Vec3, surfaces: &jka_assets::bsp::MarkSurfaces, time: i32) {
        if a.distance_squared(b) < 1.0e-8 { return; }
        let found = self.pools.as_ref().and_then(|pools| pools.iter().rposition(|p|
            p.fade_started_ms.is_none() && p.trail.len() < MAX_TRAIL_SEGMENTS && p.contains(a, n) && p.contains(b, n)));
        let index = if let Some(i) = found { i } else {
            let Some(mut pool) = BloodPool::project_surface(((a+b)*0.5).to_array(), n.to_array(), surfaces, None) else { return; };
            pool.seeded_impact = false;
            if !self.record(pool, time) { return; }
            self.pools.as_ref().unwrap().len()-1
        };
        let pool = &mut Arc::make_mut(self.pools.as_mut().unwrap())[index];
        let segment = [pool.uv(a).to_array(), pool.uv(b).to_array()];
        Arc::make_mut(&mut pool.trail).push(segment);
        pool.last_ink_ms = time;
    }

    pub(crate) fn append_air(&mut self, frame: &mut crate::fx::system::FxFrame, time_ms: i32) {
        self.spurts.append(frame, time_ms, (self.accumulator / STEP).clamp(0.0, 1.0));
        let drops = self.drops.iter().filter(|drop| !drop.spurt.is_some_and(|id| self.spurts.follows(id)));
        crate::ninja_blood_air::append(frame, drops);
    }

    pub(crate) fn preview_spurt(&mut self, origin: Vec3, direction: Vec3, time_ms: i32) -> bool {
        self.begin_frame(time_ms);
        if self.drops.len() >= MAX_DROPLETS { return false; }
        let direction = direction.try_normalize().unwrap_or(Vec3::X);
        let Some(id) = self.spurts.spawn(origin, direction, time_ms, 1.0) else { return false; };
        let mut drop = Droplet::new(origin, direction * (2.0 * METER), SPURT_IMPACT_SIZE_CM, true);
        drop.spurt = Some(id);
        drop.splat = SplatKind::SpurtImpact;
        self.drops.push(drop);
        true
    }

    pub fn has_pools(&self) -> bool {
        self.control == token() && self.pools.is_some()
    }

    pub fn scene(&self) -> Option<BloodScene> {
        if self.control != token() || self.control & ENABLED_BIT == 0 {
            return None;
        }
        Some(BloodScene {
            control: self.control,
            time_ms: self.time_ms,
            pools: self.pools.as_ref()?.clone(),
        })
    }
}

// Ground contact must be close to the gib centre or corpse collider bottom,
// not simply some floor far underneath. Use the same JKA trace world and
// markability restrictions as ordinary Ninja blood impacts.
fn gib_floor_contact(point: Vec3, world: &mut impl TraceWorld) -> Option<TraceResult> {
    let trace = world.trace(TraceQuery {
        // Begin far enough above a slightly penetrated sleeping capsule.
        start: (point + Vec3::Z * 6.0).to_array(),
        end: (point - Vec3::Z * 10.0).to_array(),
        mins: [0.0; 3], maxs: [0.0; 3], pass_entity: -1,
        mask: CONTENTS_SOLID,
    });
    (legal_hit(trace) && trace.normal[2] >= MIN_FLOOR_NORMAL_Z
        // The gib's centre sits ~3 units above the floor, while corpse
        // capsule bottoms sit at zero. Allow minor Rapier/BSP differences.
        && (point.z - trace.end[2]).abs() <= 4.5).then_some(trace)
}

fn legal_hit(trace: TraceResult) -> bool {
    trace.fraction < 1.0 && trace.start_solid == 0 && trace.all_solid == 0 && trace.entity == ENTITY_WORLD
        && trace.surface_flags & SURF_NO_MARK == 0
}

// Two systolic spurts per 760 ms (~79 beats/min), with a pause between
// beats. Only shapes the diagnostic emitter's rate/velocity; Ninja remains
// the sole density and fluid solver. Time is measured from emitter start.
pub(crate) fn gush_heartbeat(elapsed_ms: i32) -> f32 {
    let phase = elapsed_ms.rem_euclid(760) as f32;
    let pulse = |center: f32, half_width: f32| (1.0 - (phase - center).abs() / half_width).max(0.0);
    pulse(100.0, 140.0).max(0.65 * pulse(310.0, 110.0))
}

#[cfg(test)]
mod blood_contact_tests {
    use super::*;

    fn empty_floor() -> BloodPool {
        BloodPool {
            id: 1, origin: [0.0; 3], tangent: Vec3::X, normal: Vec3::Z,
            bitangent: Vec3::Y, vertices: Arc::from(Vec::<BloodVertex>::new()),
            seed: jka_ninja::BloodSeed { point_size_cm: 48.0, variation: 1, direction_uv: [0.0; 2], high_quality: false },
            motion: None, fade_started_ms: None, seeded_impact: true, birth_ms: 0,
            last_ink_ms: 0, trail: Arc::new(Vec::new()), stretch: 1.0,
            kind: SplatKind::Ordinary, stamps: Arc::new(Vec::new()), smears: SmearLog::default(),
            extent: Vec2::splat(500.0 / CM_PER_MAP_UNIT),
        }
    }

    #[test]
    fn contact_is_at_the_splatter_not_the_entire_simulation_quad() {
        let pool = empty_floor();
        assert!(pool.wet_contact(Vec3::new(3.0, 1.0, 0.0), Vec3::Z));
        assert!(!pool.wet_contact(Vec3::new(75.0, 0.0, 0.0), Vec3::Z));
        assert!(!pool.wet_contact(Vec3::new(0.0, 0.0, 8.0), Vec3::Z));
        assert!(!pool.wet_contact(Vec3::ZERO, Vec3::X));
    }

    #[test]
    fn contacted_foot_slip_is_bounded() {
        let pool = empty_floor();
        let motion = pool.motion_at(Vec3::ZERO, Vec3::X * 1000.0).unwrap();
        assert!((Vec2::from_array(motion.velocity_cm_s).length() - 120.0).abs() < 0.01);
        let walking = pool.motion_at(Vec3::ZERO, Vec3::X * 100.0).unwrap();
        let walking_speed = Vec2::from_array(walking.velocity_cm_s).length();
        assert!((walking_speed - 100.0 * CM_PER_MAP_UNIT * 0.375).abs() < 0.01);
        assert!(pool.motion_at(Vec3::ZERO, Vec3::ZERO).is_none());
    }


    #[test]
    fn a_fluid_pool_accepts_multiple_contacts_but_respects_brush_budget() {
        let mut blood = BloodEvents::default();
        blood.time_ms = 1_000;
        blood.pools = Some(Arc::new(vec![empty_floor()]));
        for x in 0..20 {
            blood.disturb([x as f32 * 2.0, 0.0, 0.0], [100.0, 0.0, 0.0]);
        }
        let pool = &blood.pools.as_ref().unwrap()[0];
        let (_, motions) = pool.motion.as_ref().unwrap();
        assert_eq!(motions.len(), jka_ninja::BLOOD_MAX_CONTACTS);
        // A second contact from the same player/segment must not displace
        // the same patch of fluid twice during a single render frame.
        blood.time_ms += 1;
        blood.disturb([0.0; 3], [100.0, 0.0, 0.0]);
        blood.disturb([0.0; 3], [100.0, 0.0, 0.0]);
        let (_, motions) = blood.pools.as_ref().unwrap()[0].motion.as_ref().unwrap();
        assert_eq!(motions.len(), 1);
    }

    #[test]
    fn heartbeat_gush_has_double_peak_and_rest() {
        let first = gush_heartbeat(100);
        let second = gush_heartbeat(310);
        let rest = gush_heartbeat(570);
        assert!(first > 0.8 && second > 0.4);
        assert!(rest < 0.08);
        assert_eq!(gush_heartbeat(100), gush_heartbeat(860));
    }

    #[test]
    fn foot_transfer_preserves_stronger_first_two_steps() {
        assert_eq!(CARRIED_STRENGTH, [1.0, 0.9, 0.7, 0.4]);
        assert_eq!(CARRIED_STRENGTH.len(), CARRIED_STEPS as usize);
    }
}
