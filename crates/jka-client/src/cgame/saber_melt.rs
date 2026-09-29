//! Enhanced saber-mark melt: a presentation-only molten-surface simulation.
//!
//! A blade held against a wall injects heat into a small height/heat field that
//! is anchored to the struck plane (think Qui-Gon burning through the blast
//! door). Per field node the sim tracks:
//!
//! * `heat`  - cools exponentially; drives glow, fluidity and melting.
//! * `carve` - how far the surface has melted away. Saturates at 1.0, so a held
//!   blade digs a groove to a fixed maximum depth and then only keeps it hot.
//! * `mass`  - displaced molten material. Pushed out to the melt front (lips),
//!   it flows while hot (down the wall under tangent gravity, in rivulets),
//!   and freezes in place as the heat leaves, leaving slag lips and runs.
//! * `soot`  - permanent scorch discolouration around the heat.
//!
//! Surfaces are sparse chunked grids so an arbitrarily long cut keeps a fixed
//! cost. Each chunk is probed against the collision world once when created so
//! the relief conforms to the real surface and never spills past a door edge.
//! After the metal has sat cold for a while every chunk eases back to the
//! original geometry and is dropped.
//!
//! Rendering is split in two: a lit alpha-blended relief skin (real raised
//! geometry with proper normals, so grid/dynamic lights shade it) and an
//! additive emissive layer. Nothing here touches gameplay or prediction.

use crate::fx::system::{FxDraw, FxLight, FxLightKind};
use jka_movement::{TraceQuery, TraceWorld, ENTITY_NONE, ENTITY_WORLD};
use std::collections::HashMap;

type Vec3 = [f32; 3];

const CONTENTS_SOLID: i32 = 0x0000_0001;
const CONTENTS_TERRAIN: i32 = 0x0000_1000;

// ---- Field layout -----------------------------------------------------------
/// Node spacing in world units.
const CELL: f32 = 0.75;
/// Nodes per chunk edge.
const CH: usize = 16;
const CH_I: i32 = CH as i32;
const N: usize = CH * CH;
/// Padded gather covers nodes -OFF..=CH+OFF: two smoothing passes plus the
/// normal stencil each reach one node beyond the vertices.
const OFF: usize = 3;
const OFF_I: i32 = OFF as i32;
const PAD: usize = CH + 2 * OFF + 1;
const PP: usize = PAD * PAD;
/// World-unit spacing of the collision probes that make the relief conform.
const PROBE_STRIDE: usize = 4;
const PROBES: usize = CH / PROBE_STRIDE + 1;
const PROBE_REACH: f32 = 3.0;
const PROBE_TOLERANCE: f32 = 1.6;

const F_HEAT: usize = 0;
const F_CARVE: usize = 1;
const F_MASS: usize = 2;
const F_SOOT: usize = 3;
const F_BASE: usize = 4;

// ---- Simulation tuning ------------------------------------------------------
const STEP_MS: i32 = 20;
const STEP_S: f32 = STEP_MS as f32 * 0.001;
const MAX_STEPS_PER_UPDATE: i32 = 8;
const HEAT_MAX: f32 = 1.6;
/// Heat per second injected at the blade centre.
const HEAT_RATE: f32 = 1.3;
/// Minimum heat per blade-width of travel, so fast swipes still melt a line.
const LINE_HEAT: f32 = 0.20;
/// The blade footprint starts as a thin line and widens as heat soaks in.
const SIGMA_START: f32 = 0.42;
const SIGMA_FULL: f32 = 1.10;
const SIGMA_WIDEN_MS: f32 = 1800.0;
const HEAT_DIFFUSE: f32 = 0.06;
const COOL_TAU_S: f32 = 2.3;
const MELT_START: f32 = 0.30;
const MELT_FULL: f32 = 0.95;
const CARVE_RATE: f32 = 3.2;
const FLUID_START: f32 = 0.16;
const FLUID_FULL: f32 = 0.65;
const FLOW_SPREAD: f32 = 7.0;
const FLOW_GRAVITY: f32 = 2.4;
const COLD_PEAK: f32 = 0.03;
const ACTIVE_PEAK: f32 = 0.008;
const COLD_HOLD_MS: f32 = 14_000.0;
const REVERT_MS: f32 = 12_000.0;
const EVICT_REVERT_MS: f32 = 2_500.0;
const STROKE_BREAK_MS: i32 = 140;
const SURFACE_PLANE_TOLERANCE: f32 = 0.9;
const SURFACE_NORMAL_DOT: f32 = 0.985;

const MAX_SURFACES: usize = 16;
const MAX_CHUNKS: usize = 200;
const HARD_MAX_CHUNKS: usize = 320;
const MAX_BUBBLES_PER_CHUNK: usize = 14;
const MAX_DROPLETS: usize = 24;
const MAX_EVENTS_PER_UPDATE: usize = 6;

// ---- Look tuning ------------------------------------------------------------
/// World units of relief per unit of frozen/molten mass.
const LIP_HEIGHT: f32 = 3.4;
/// Groove depth used for shading normals (the skin itself cannot dip below the
/// wall, so the cavity is conveyed by lighting, darkening and the raised lips).
const GROOVE_DEPTH: f32 = 1.9;
const SKIN_BIAS: f32 = 0.22;
const GLOW_LIFT: f32 = 0.05;
const EMIT_RANGE: f32 = 6000.0;
const DROPLET_GRAVITY: f32 = 620.0;
const DROPLET_LIFE_MS: i32 = 1200;

#[inline]
fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
#[inline]
fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
#[inline]
fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
#[inline]
fn madd(a: Vec3, d: Vec3, s: f32) -> Vec3 {
    [a[0] + d[0] * s, a[1] + d[1] * s, a[2] + d[2] * s]
}
fn normalize(a: Vec3) -> Vec3 {
    let l = dot(a, a).sqrt();
    if l > 1.0e-6 { scale(a, 1.0 / l) } else { [0.0, 0.0, 1.0] }
}
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
fn lerp3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)]
}

fn hash2(i: i32, j: i32) -> f32 {
    let mut h = (i as u32).wrapping_mul(0x9E37_79B1) ^ (j as u32).wrapping_mul(0x85EB_CA6B);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h & 0x00FF_FFFF) as f32 / 16_777_216.0
}

fn value_noise(x: f32, y: f32) -> f32 {
    let (xi, yi) = (x.floor(), y.floor());
    let (fx, fy) = (x - xi, y - yi);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let (xi, yi) = (xi as i32, yi as i32);
    let a = lerp(hash2(xi, yi), hash2(xi + 1, yi), sx);
    let b = lerp(hash2(xi, yi + 1), hash2(xi + 1, yi + 1), sx);
    lerp(a, b, sy)
}

struct Rng(u32);
impl Rng {
    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x >> 8) as f32 / 16_777_216.0
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }
}

/// Blackbody-ish ramp: dull red -> orange -> yellow -> hot white.
fn blackbody(t: f32) -> Vec3 {
    // Steel: it works up through a dull red glow, cherry, orange, then yellow,
    // and only reaches white-hot at the very top of the range.
    const STOPS: [(f32, Vec3); 8] = [
        (0.0, [0.0, 0.0, 0.0]),
        (0.12, [0.07, 0.004, 0.0]),
        (0.30, [0.32, 0.028, 0.002]),
        (0.55, [0.70, 0.10, 0.006]),
        (0.85, [1.0, 0.30, 0.03]),
        (1.15, [1.0, 0.58, 0.12]),
        (1.42, [1.0, 0.84, 0.46]),
        (1.60, [1.0, 0.96, 0.80]),
    ];
    let t = t.clamp(0.0, 1.6);
    for pair in STOPS.windows(2) {
        let (t0, c0) = pair[0];
        let (t1, c1) = pair[1];
        if t <= t1 {
            return lerp3(c0, c1, (t - t0) / (t1 - t0));
        }
    }
    STOPS[STOPS.len() - 1].1
}

// ---- Data -------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Bubble {
    /// Chunk-local node coordinates.
    x: f32,
    y: f32,
    born: i32,
    life: i32,
    radius: f32,
    amp: f32,
}

struct Chunk {
    f: [[f32; N]; 5],
    valid: [bool; N],
    /// Peak heat after the last step.
    peak: f32,
    cold_ms: f32,
    /// 0 = fully present, 1 = fully reverted.
    revert: f32,
    bubbles: Vec<Bubble>,
    last_pop_ms: i32,
    touched_ms: i32,
    /// Over the chunk budget: skip the cold hold and revert quickly, then drop.
    evicting: bool,
}

impl Chunk {
    fn empty(now: i32) -> Self {
        Self {
            f: [[0.0; N]; 5],
            valid: [true; N],
            peak: 0.0,
            cold_ms: 0.0,
            revert: 0.0,
            bubbles: Vec::new(),
            last_pop_ms: i32::MIN / 2,
            touched_ms: now,
            evicting: false,
        }
    }
}

struct Surface {
    origin: Vec3,
    n: Vec3,
    u: Vec3,
    v: Vec3,
    /// Tangent-plane gravity in (u, v); zero on floors/ceilings.
    grav: [f32; 2],
    chunks: HashMap<(i32, i32), Chunk>,
    touched_ms: i32,
}

impl Surface {
    fn new(origin: Vec3, n: Vec3, now: i32) -> Self {
        let up = [0.0, 0.0, 1.0];
        let mut u = cross(up, n);
        if dot(u, u) < 0.04 {
            u = [1.0, 0.0, 0.0];
            u = normalize(sub(u, scale(n, dot(u, n))));
        } else {
            u = normalize(u);
        }
        let mut v = cross(n, u);
        if dot(v, up) < 0.0 && dot(n, up).abs() < 0.7 {
            u = scale(u, -1.0);
            v = cross(n, u);
        }
        let g = sub([0.0, 0.0, -1.0], scale(n, dot([0.0, 0.0, -1.0], n)));
        let mag = dot(g, g).sqrt();
        let grav = if mag > 0.25 { [dot(g, u), dot(g, v)] } else { [0.0, 0.0] };
        Self { origin, n, u, v, grav, chunks: HashMap::new(), touched_ms: now }
    }

    fn node_pos(&self, gx: f32, gy: f32, h: f32) -> Vec3 {
        add(add(self.origin, scale(self.u, gx * CELL)), add(scale(self.v, gy * CELL), scale(self.n, h)))
    }

    fn to_node(&self, p: Vec3) -> (f32, f32, f32) {
        let d = sub(p, self.origin);
        (dot(d, self.u) / CELL, dot(d, self.v) / CELL, dot(d, self.n))
    }

    fn matches(&self, p: Vec3, n: Vec3) -> bool {
        dot(self.n, n) >= SURFACE_NORMAL_DOT && dot(sub(p, self.origin), self.n).abs() <= SURFACE_PLANE_TOLERANCE
    }
}

#[derive(Clone, Copy)]
struct StrokeState {
    surface: usize,
    x: f32,
    y: f32,
    last_ms: i32,
    born_ms: i32,
}

#[derive(Clone, Copy, Debug)]
struct Droplet {
    pos: Vec3,
    vel: Vec3,
    born: i32,
}

/// Something the caller should turn into a regular FX (sparks).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeltSpark {
    pub origin: Vec3,
    pub normal: Vec3,
}

pub struct SaberMelt {
    surfaces: Vec<Surface>,
    strokes: HashMap<u32, StrokeState>,
    droplets: Vec<Droplet>,
    rng: Rng,
    sim_ms: i32,
    last_update_ms: i32,
    started: bool,
}

impl Default for SaberMelt {
    fn default() -> Self {
        Self::new()
    }
}

/// 3x3 chunk neighbourhood used for gathers.
struct Neighborhood<'a> {
    nb: [Option<&'a Chunk>; 9],
}

impl<'a> Neighborhood<'a> {
    fn new(surface: &'a Surface, key: (i32, i32)) -> Self {
        let mut nb = [None; 9];
        for dy in -1..=1 {
            for dx in -1..=1 {
                nb[((dy + 1) * 3 + dx + 1) as usize] = surface.chunks.get(&(key.0 + dx, key.1 + dy));
            }
        }
        Self { nb }
    }

    /// Chunk + linear node index for a node in this chunk's local (possibly
    /// out-of-range by one) coordinates.
    fn at(&self, lx: i32, ly: i32) -> Option<(&'a Chunk, usize)> {
        let cx = if lx < 0 { 0 } else if lx >= CH_I { 2 } else { 1 };
        let cy = if ly < 0 { 0 } else if ly >= CH_I { 2 } else { 1 };
        let chunk = self.nb[cy * 3 + cx]?;
        let ix = lx.rem_euclid(CH_I) as usize;
        let iy = ly.rem_euclid(CH_I) as usize;
        Some((chunk, iy * CH + ix))
    }
}

struct Padded {
    heat: [f32; PP],
    carve: [f32; PP],
    mass: [f32; PP],
    soot: [f32; PP],
    base: [f32; PP],
    valid: [bool; PP],
    revert: [f32; PP],
}

impl Padded {
    fn gather(nh: &Neighborhood) -> Self {
        let mut p = Self {
            heat: [0.0; PP],
            carve: [0.0; PP],
            mass: [0.0; PP],
            soot: [0.0; PP],
            base: [0.0; PP],
            valid: [false; PP],
            revert: [1.0; PP],
        };
        for py in 0..PAD {
            for px in 0..PAD {
                let i = py * PAD + px;
                if let Some((c, n)) = nh.at(px as i32 - OFF_I, py as i32 - OFF_I) {
                    p.heat[i] = c.f[F_HEAT][n];
                    p.carve[i] = c.f[F_CARVE][n];
                    p.mass[i] = c.f[F_MASS][n];
                    p.soot[i] = c.f[F_SOOT][n];
                    p.base[i] = c.f[F_BASE][n];
                    p.valid[i] = c.valid[n];
                    p.revert[i] = c.revert;
                }
            }
        }
        p
    }
}

fn displaced(heat: f32, carve: f32) -> f32 {
    (smoothstep(MELT_START, MELT_FULL, heat) - carve).max(0.0) * (1.0 - (-CARVE_RATE * STEP_S).exp())
}

impl SaberMelt {
    pub fn new() -> Self {
        Self {
            surfaces: Vec::new(),
            strokes: HashMap::new(),
            droplets: Vec::new(),
            rng: Rng(0x2F6E_2B1D),
            sim_ms: 0,
            last_update_ms: 0,
            started: false,
        }
    }

    pub fn clear(&mut self) {
        self.surfaces.clear();
        self.strokes.clear();
        self.droplets.clear();
        self.started = false;
    }

    pub fn chunk_count(&self) -> usize {
        self.surfaces.iter().map(|s| s.chunks.len()).sum()
    }

    /// The blade stopped touching a wall (or left the world).
    pub fn end_stroke(&mut self, stroke: u32) {
        self.strokes.remove(&stroke);
    }

    // ---- Contact ------------------------------------------------------------

    /// Feed one blade/world contact. Returns the heat now at the contact node.
    pub fn contact<W: TraceWorld>(
        &mut self,
        stroke: u32,
        position: Vec3,
        normal: Vec3,
        now: i32,
        mut world: Option<&mut W>,
    ) -> f32 {
        let normal = normalize(normal);
        let previous = self.strokes.get(&stroke).copied();
        let si = match previous {
            Some(state)
                if now.saturating_sub(state.last_ms) <= STROKE_BREAK_MS
                    && self.surfaces.get(state.surface).is_some_and(|s| s.matches(position, normal)) =>
            {
                state.surface
            }
            _ => self.surface_for(position, normal, now),
        };
        let (x, y, _) = self.surfaces[si].to_node(position);
        let dt_ms = previous
            .filter(|s| s.surface == si)
            .map_or(16, |s| now.saturating_sub(s.last_ms).clamp(4, 50));
        let amount = HEAT_RATE * dt_ms as f32 * 0.001;

        let born_ms = match previous {
            Some(s) if s.surface == si && now.saturating_sub(s.last_ms) <= STROKE_BREAK_MS => s.born_ms,
            _ => now,
        };
        let widen = smoothstep(0.0, SIGMA_WIDEN_MS, now.saturating_sub(born_ms) as f32);
        let sigma = lerp(SIGMA_START, SIGMA_FULL, widen) / CELL;
        let (x0, y0) = match previous {
            Some(s) if s.surface == si && now.saturating_sub(s.last_ms) <= STROKE_BREAK_MS => (s.x, s.y),
            _ => (x, y),
        };
        let length = ((x - x0).powi(2) + (y - y0).powi(2)).sqrt();
        let substeps = ((length / (sigma * 0.6)).ceil() as usize).clamp(1, 96);
        // A quick swipe would otherwise spread the frame's heat over a long path
        // and barely warm it. Guarantee a minimum heat per unit of travel so the
        // blade always leaves at least a melted line.
        let per_sub = if length > 0.05 {
            let travel = (length / substeps as f32 / (sigma * 0.6)).min(1.0);
            (amount / substeps as f32).max(LINE_HEAT * travel)
        } else {
            amount
        };
        for k in 1..=substeps {
            let t = k as f32 / substeps as f32;
            self.deposit(si, lerp(x0, x, t), lerp(y0, y, t), per_sub, sigma, now, world.as_deref_mut());
        }
        self.strokes.insert(stroke, StrokeState { surface: si, x, y, last_ms: now, born_ms });
        self.surfaces[si].touched_ms = now;
        self.heat_at(si, x, y)
    }

    fn surface_for(&mut self, position: Vec3, normal: Vec3, now: i32) -> usize {
        if let Some(i) = self.surfaces.iter().position(|s| s.matches(position, normal)) {
            return i;
        }
        if self.surfaces.len() >= MAX_SURFACES {
            // Drop the coldest, least recently touched surface and fix strokes.
            let victim = (0..self.surfaces.len())
                .min_by_key(|&i| self.surfaces[i].touched_ms)
                .unwrap_or(0);
            self.surfaces.swap_remove(victim);
            let moved = self.surfaces.len();
            self.strokes.retain(|_, s| s.surface != victim);
            for s in self.strokes.values_mut() {
                if s.surface == moved {
                    s.surface = victim;
                }
            }
        }
        self.surfaces.push(Surface::new(position, normal, now));
        self.surfaces.len() - 1
    }

    fn heat_at(&self, si: usize, x: f32, y: f32) -> f32 {
        let (gi, gj) = (x.round() as i32, y.round() as i32);
        self.surfaces[si]
            .chunks
            .get(&(gi.div_euclid(CH_I), gj.div_euclid(CH_I)))
            .map_or(0.0, |c| c.f[F_HEAT][gj.rem_euclid(CH_I) as usize * CH + gi.rem_euclid(CH_I) as usize])
    }

    fn deposit<W: TraceWorld>(
        &mut self,
        si: usize,
        cx: f32,
        cy: f32,
        amount: f32,
        sigma: f32,
        now: i32,
        mut world: Option<&mut W>,
    ) {
        let reach = (3.0 * sigma).ceil() as i32;
        let (ci, cj) = (cx.round() as i32, cy.round() as i32);
        let (lo_x, hi_x) = ((ci - reach).div_euclid(CH_I), (ci + reach).div_euclid(CH_I));
        let (lo_y, hi_y) = ((cj - reach).div_euclid(CH_I), (cj + reach).div_euclid(CH_I));
        for ky in lo_y..=hi_y {
            for kx in lo_x..=hi_x {
                self.ensure_chunk(si, (kx, ky), now, world.as_deref_mut());
            }
        }
        let two_sigma_sq = 2.0 * sigma * sigma;
        let max_d2 = (3.0 * sigma).powi(2);
        let surface = &mut self.surfaces[si];
        for gj in (cj - reach)..=(cj + reach) {
            for gi in (ci - reach)..=(ci + reach) {
                let d2 = (gi as f32 - cx).powi(2) + (gj as f32 - cy).powi(2);
                if d2 > max_d2 {
                    continue;
                }
                let key = (gi.div_euclid(CH_I), gj.div_euclid(CH_I));
                let Some(chunk) = surface.chunks.get_mut(&key) else { continue };
                let idx = gj.rem_euclid(CH_I) as usize * CH + gi.rem_euclid(CH_I) as usize;
                if !chunk.valid[idx] {
                    continue;
                }
                let heat = &mut chunk.f[F_HEAT][idx];
                *heat = (*heat + amount * (-d2 / two_sigma_sq).exp()).min(HEAT_MAX);
                chunk.peak = chunk.peak.max(*heat);
                chunk.cold_ms = 0.0;
                chunk.evicting = false;
                chunk.touched_ms = now;
            }
        }
    }

    fn ensure_chunk<W: TraceWorld>(&mut self, si: usize, key: (i32, i32), now: i32, world: Option<&mut W>) {
        if self.surfaces[si].chunks.contains_key(&key) {
            return;
        }
        if self.chunk_count() >= MAX_CHUNKS {
            self.evict_chunk();
        }
        // Fading chunks still count; never let a runaway stroke grow unbounded.
        while self.chunk_count() >= HARD_MAX_CHUNKS {
            let before = self.chunk_count();
            self.drop_coldest_now();
            if self.chunk_count() == before {
                break;
            }
        }
        let chunk = make_chunk(&self.surfaces[si], key, now, world);
        self.surfaces[si].chunks.insert(key, chunk);
    }

    fn drop_coldest_now(&mut self) {
        let mut victim: Option<(usize, (i32, i32), i32)> = None;
        for (si, s) in self.surfaces.iter().enumerate() {
            for (key, c) in &s.chunks {
                let rank = c.touched_ms;
                if victim.is_none_or(|(_, _, best)| rank < best) {
                    victim = Some((si, *key, rank));
                }
            }
        }
        if let Some((si, key, _)) = victim {
            self.surfaces[si].chunks.remove(&key);
        }
    }

    /// Ask the coldest, oldest chunk to fade out quickly (no popping).
    fn evict_chunk(&mut self) {
        let mut victim: Option<(usize, (i32, i32), (i32, i32))> = None; // surface, key, (quantised peak, touched)
        for (si, s) in self.surfaces.iter().enumerate() {
            for (key, c) in &s.chunks {
                if c.evicting {
                    continue;
                }
                let rank = ((c.peak * 1000.0) as i32, c.touched_ms);
                if victim.is_none_or(|(_, _, best)| rank < best) {
                    victim = Some((si, *key, rank));
                }
            }
        }
        if let Some((si, key, _)) = victim {
            if let Some(c) = self.surfaces[si].chunks.get_mut(&key) {
                c.evicting = true;
            }
        }
    }

    // ---- Simulation ---------------------------------------------------------

    /// Advance the fixed-step simulation to `now` and animate droplets.
    /// Returns sparks the caller should spawn.
    pub fn update<W: TraceWorld>(&mut self, now: i32, mut world: Option<&mut W>) -> Vec<MeltSpark> {
        let mut sparks = Vec::new();
        if self.started && now < self.sim_ms - 100 {
            self.clear();
        }
        if !self.started {
            self.started = true;
            self.sim_ms = now;
            self.last_update_ms = now;
        }
        if now - self.sim_ms > STEP_MS * MAX_STEPS_PER_UPDATE {
            self.sim_ms = now - STEP_MS * MAX_STEPS_PER_UPDATE;
        }
        while self.sim_ms + STEP_MS <= now {
            self.sim_ms += STEP_MS;
            self.step(self.sim_ms, &mut sparks, world.as_deref_mut());
        }
        self.update_droplets(now, &mut sparks, world);
        self.last_update_ms = now;

        self.strokes.retain(|_, s| now.saturating_sub(s.last_ms) <= 400);
        if self.surfaces.iter().any(|s| s.chunks.is_empty()) {
            // Strokes reference surface indices; only drop empty surfaces that
            // no live stroke points at, keeping indices stable.
            let mut i = self.surfaces.len();
            while i > 0 {
                i -= 1;
                if self.surfaces[i].chunks.is_empty() && !self.strokes.values().any(|s| s.surface == i) {
                    let last = self.surfaces.len() - 1;
                    self.surfaces.swap_remove(i);
                    for s in self.strokes.values_mut() {
                        if s.surface == last {
                            s.surface = i;
                        }
                    }
                }
            }
        }
        sparks.truncate(MAX_EVENTS_PER_UPDATE);
        sparks
    }

    fn step<W: TraceWorld>(&mut self, now: i32, sparks: &mut Vec<MeltSpark>, mut world: Option<&mut W>) {
        let decay = (-STEP_S / COOL_TAU_S).exp();
        let mut wanted: Vec<(usize, (i32, i32))> = Vec::new();
        for si in 0..self.surfaces.len() {
            let surface = &self.surfaces[si];
            let active: Vec<(i32, i32)> = surface
                .chunks
                .keys()
                .copied()
                .filter(|&(kx, ky)| {
                    (-1..=1).any(|dy| {
                        (-1..=1).any(|dx| {
                            surface.chunks.get(&(kx + dx, ky + dy)).is_some_and(|c| c.peak >= ACTIVE_PEAK)
                        })
                    })
                })
                .collect();

            let mut results: Vec<((i32, i32), Box<[[f32; N]; 4]>)> = Vec::with_capacity(active.len());
            for &key in &active {
                let nh = Neighborhood::new(surface, key);
                let p = Padded::gather(&nh);
                results.push((key, step_chunk(surface, key, &p, decay)));
            }

            let surface = &mut self.surfaces[si];
            for (key, out) in results {
                let chunk = surface.chunks.get_mut(&key).expect("active chunk exists");
                chunk.f[F_HEAT] = out[0];
                chunk.f[F_CARVE] = out[1];
                chunk.f[F_MASS] = out[2];
                chunk.f[F_SOOT] = out[3];
                chunk.peak = chunk.f[F_HEAT].iter().copied().fold(0.0, f32::max);
                if chunk.peak < ACTIVE_PEAK {
                    chunk.peak = 0.0;
                }
                // Grow into neighbours when molten mass or heat reaches an edge.
                let live = |n: usize| chunk.f[F_MASS][n] > 0.05 || chunk.f[F_HEAT][n] > 0.05;
                let (mut left, mut right, mut down, mut up) = (false, false, false, false);
                for i in 0..CH {
                    left |= live(i * CH);
                    right |= live(i * CH + CH - 1);
                    down |= live(i);
                    up |= live((CH - 1) * CH + i);
                }
                for (hit, dx, dy) in [(left, -1, 0), (right, 1, 0), (down, 0, -1), (up, 0, 1)] {
                    if hit {
                        wanted.push((si, (key.0 + dx, key.1 + dy)));
                    }
                }
            }

            // Bubbles, droplets, revert timers.
            let keys: Vec<(i32, i32)> = surface.chunks.keys().copied().collect();
            let mut drop_keys = Vec::new();
            for key in keys {
                let (gravity_mag, n_world) = (surface.grav[0].hypot(surface.grav[1]), surface.n);
                let (u, v, origin) = (surface.u, surface.v, surface.origin);
                let grav = surface.grav;
                let chunk = surface.chunks.get_mut(&key).expect("key exists");
                // Pop expired bubbles.
                let mut popped = Vec::new();
                chunk.bubbles.retain(|b| {
                    if now >= b.born + b.life {
                        popped.push(*b);
                        false
                    } else {
                        true
                    }
                });
                for b in popped {
                    if now - chunk.last_pop_ms >= 70 && sparks.len() < MAX_EVENTS_PER_UPDATE {
                        chunk.last_pop_ms = now;
                        let gx = (key.0 * CH_I) as f32 + b.x;
                        let gy = (key.1 * CH_I) as f32 + b.y;
                        let p = add(add(origin, scale(u, gx * CELL)), scale(v, gy * CELL));
                        sparks.push(MeltSpark { origin: madd(p, n_world, SKIN_BIAS + 0.6), normal: n_world });
                    }
                }
                if chunk.peak >= 0.55 {
                    for _ in 0..2 {
                        let lx = 3 + (self.rng.next() * (CH as f32 - 6.0)) as usize;
                        let ly = 3 + (self.rng.next() * (CH as f32 - 6.0)) as usize;
                        let n = ly * CH + lx;
                        let (h, c, m) = (chunk.f[F_HEAT][n], chunk.f[F_CARVE][n], chunk.f[F_MASS][n]);
                        if h > 0.62 && c > 0.35 && chunk.bubbles.len() < MAX_BUBBLES_PER_CHUNK && self.rng.next() < 0.10 {
                            let life = self.rng.range(320.0, 780.0) as i32;
                            chunk.bubbles.push(Bubble {
                                x: lx as f32,
                                y: ly as f32,
                                born: now,
                                life,
                                radius: self.rng.range(1.3, 2.6),
                                amp: self.rng.range(0.35, 0.85) * h.min(1.2),
                            });
                        }
                        if gravity_mag > 0.4
                            && m > 0.5
                            && h > 0.5
                            && self.droplets.len() < MAX_DROPLETS
                            && self.rng.next() < 0.05
                        {
                            chunk.f[F_MASS][n] -= 0.22;
                            let gx = (key.0 * CH_I) as f32 + lx as f32;
                            let gy = (key.1 * CH_I) as f32 + ly as f32;
                            let p = add(add(origin, scale(u, gx * CELL)), scale(v, gy * CELL));
                            let down = normalize(add(scale(u, grav[0]), scale(v, grav[1])));
                            self.droplets.push(Droplet {
                                pos: madd(p, n_world, SKIN_BIAS + 0.5),
                                vel: madd(scale(down, 26.0), n_world, 6.0),
                                born: now,
                            });
                        }
                    }
                }
                // Cold timers / revert.
                if chunk.peak >= COLD_PEAK || !chunk.bubbles.is_empty() {
                    chunk.cold_ms = 0.0;
                    chunk.revert = (chunk.revert - STEP_S * 1.5).max(0.0);
                } else {
                    chunk.cold_ms += STEP_MS as f32;
                    if chunk.evicting || chunk.cold_ms > COLD_HOLD_MS {
                        let span = if chunk.evicting { EVICT_REVERT_MS } else { REVERT_MS };
                        chunk.revert += STEP_MS as f32 / span;
                        if chunk.revert >= 1.0 {
                            drop_keys.push(key);
                        }
                    }
                }
            }
            for key in drop_keys {
                surface.chunks.remove(&key);
            }
        }

        wanted.sort_unstable();
        wanted.dedup();
        for (si, key) in wanted {
            if si < self.surfaces.len() && !self.surfaces[si].chunks.contains_key(&key) {
                self.ensure_chunk(si, key, now, world.as_deref_mut());
            }
        }
    }

    fn update_droplets<W: TraceWorld>(&mut self, now: i32, sparks: &mut Vec<MeltSpark>, mut world: Option<&mut W>) {
        let dt = (now - self.last_update_ms).clamp(0, 100) as f32 * 0.001;
        if dt <= 0.0 {
            return;
        }
        self.droplets.retain_mut(|d| {
            if now - d.born > DROPLET_LIFE_MS {
                return false;
            }
            d.vel[2] -= DROPLET_GRAVITY * dt;
            let next = madd(d.pos, d.vel, dt);
            if let Some(w) = world.as_deref_mut() {
                let hit = w.trace(TraceQuery {
                    start: d.pos,
                    mins: [0.0; 3],
                    maxs: [0.0; 3],
                    end: next,
                    pass_entity: ENTITY_NONE,
                    mask: CONTENTS_SOLID | CONTENTS_TERRAIN,
                });
                if hit.fraction < 1.0 {
                    if sparks.len() < MAX_EVENTS_PER_UPDATE {
                        sparks.push(MeltSpark { origin: madd(hit.end, hit.normal, 0.3), normal: hit.normal });
                    }
                    return false;
                }
            }
            d.pos = next;
            true
        });
    }

    // ---- Presentation -------------------------------------------------------

    /// Append relief/glow geometry, droplets and heat lights for `now`.
    pub fn emit(&self, now: i32, view: Vec3, draws: &mut Vec<FxDraw>, lights: &mut Vec<FxLight>) {
        let mut hot: Vec<(f32, Vec3)> = Vec::new();
        for surface in &self.surfaces {
            for (&key, chunk) in &surface.chunks {
                let centre = surface.node_pos(
                    (key.0 * CH_I) as f32 + CH as f32 * 0.5,
                    (key.1 * CH_I) as f32 + CH as f32 * 0.5,
                    0.0,
                );
                let d = sub(centre, view);
                if dot(d, d) > EMIT_RANGE * EMIT_RANGE {
                    continue;
                }
                emit_chunk(surface, key, chunk, now, draws);
                if chunk.peak > 0.22 {
                    let (mut best, mut at) = (0.0_f32, 0usize);
                    for (i, &h) in chunk.f[F_HEAT].iter().enumerate() {
                        if h > best {
                            best = h;
                            at = i;
                        }
                    }
                    let gx = (key.0 * CH_I) as f32 + (at % CH) as f32;
                    let gy = (key.1 * CH_I) as f32 + (at / CH) as f32;
                    hot.push((best, surface.node_pos(gx, gy, 1.6)));
                }
            }
        }
        hot.sort_by(|a, b| b.0.total_cmp(&a.0));
        for &(heat, origin) in hot.iter().take(4) {
            let h = (heat / 1.25).clamp(0.0, 1.0);
            let flicker = 0.94 + (now as f32 * 0.031 + origin[0] * 0.7).sin() * 0.06;
            lights.push(FxLight {
                kind: FxLightKind::SaberMark,
                origin,
                segment: None,
                blade_segments: None,
                radius: 34.0 + 84.0 * h,
                rgb: [flicker * h.powf(1.5), (0.16 + 0.22 * h) * flicker * h.powf(1.5), 0.03 * flicker * h.powf(1.5)],
            });
        }
        for d in &self.droplets {
            let tail = madd(d.pos, d.vel, -0.035);
            draws.push(FxDraw::Line {
                start: tail,
                end: d.pos,
                width: 0.32,
                rgba: [255, 150, 40, 255],
                shader: "gfx/effects/whiteGlow".to_owned(),
            });
            draws.push(FxDraw::Sprite {
                origin: d.pos,
                radius: 0.8,
                rotation: 0.0,
                rgba: [255, 190, 90, 255],
                shader: "gfx/effects/whiteGlow".to_owned(),
            });
        }
    }
}

// ---- Chunk creation (collision conformance) -----------------------------------

fn probe<W: TraceWorld>(s: &Surface, world: &mut W, gi: i32, gj: i32) -> Option<f32> {
    let p = s.node_pos(gi as f32, gj as f32, 0.0);
    let hit = world.trace(TraceQuery {
        start: madd(p, s.n, PROBE_REACH),
        mins: [0.0; 3],
        maxs: [0.0; 3],
        end: madd(p, s.n, -PROBE_REACH),
        pass_entity: ENTITY_NONE,
        mask: CONTENTS_SOLID | CONTENTS_TERRAIN,
    });
    if hit.fraction >= 1.0 || hit.start_solid != 0 || hit.all_solid != 0 || hit.entity != ENTITY_WORLD {
        return None;
    }
    if dot(normalize(hit.normal), s.n) < 0.85 {
        return None;
    }
    let offset = PROBE_REACH - hit.fraction * 2.0 * PROBE_REACH;
    (offset.abs() <= PROBE_TOLERANCE).then_some(offset)
}

fn make_chunk<W: TraceWorld>(s: &Surface, key: (i32, i32), now: i32, world: Option<&mut W>) -> Chunk {
    let mut chunk = Chunk::empty(now);
    let Some(world) = world else { return chunk };
    let mut grid = [[None::<f32>; PROBES]; PROBES];
    for (b, row) in grid.iter_mut().enumerate() {
        for (a, cell) in row.iter_mut().enumerate() {
            *cell = probe(
                s,
                world,
                key.0 * CH_I + (a * PROBE_STRIDE) as i32,
                key.1 * CH_I + (b * PROBE_STRIDE) as i32,
            );
        }
    }
    for y in 0..CH {
        for x in 0..CH {
            let (ca, cb) = (x / PROBE_STRIDE, y / PROBE_STRIDE);
            let corners = [grid[cb][ca], grid[cb][ca + 1], grid[cb + 1][ca], grid[cb + 1][ca + 1]];
            let idx = y * CH + x;
            match corners {
                [Some(c00), Some(c10), Some(c01), Some(c11)] => {
                    let fx = (x % PROBE_STRIDE) as f32 / PROBE_STRIDE as f32;
                    let fy = (y % PROBE_STRIDE) as f32 / PROBE_STRIDE as f32;
                    chunk.f[F_BASE][idx] = lerp(lerp(c00, c10, fx), lerp(c01, c11, fx), fy);
                }
                _ => chunk.valid[idx] = false,
            }
        }
    }
    chunk
}

// ---- One chunk simulation step ------------------------------------------------

fn step_chunk(surface: &Surface, key: (i32, i32), p: &Padded, decay: f32) -> Box<[[f32; N]; 4]> {
    let mut out = Box::new([[0.0_f32; N]; 4]);
    let grav = surface.grav;
    let dirs: [(isize, f32, f32); 4] = [(-1, -1.0, 0.0), (1, 1.0, 0.0), (-(PAD as isize), 0.0, -1.0), (PAD as isize, 0.0, 1.0)];

    let flux = |from: usize, to: usize, dx: f32, dy: f32| -> f32 {
        let m = p.mass[from];
        if m <= 0.0 || !p.valid[to] {
            return 0.0;
        }
        let fluid = smoothstep(FLUID_START, FLUID_FULL, p.heat[from]);
        if fluid <= 0.0 {
            return 0.0;
        }
        let gi = key.0 * CH_I + (from % PAD) as i32 - OFF_I;
        let gj = key.1 * CH_I + (from / PAD) as i32 - OFF_I;
        // Rivulets: noise varies across the gravity direction, slowly along it.
        let (gx, gy) = (grav[0], grav[1]);
        let glen = gx.hypot(gy);
        let visc = if glen > 0.01 {
            let (ax, ay) = (gx / glen, gy / glen);
            let along = gi as f32 * ax + gj as f32 * ay;
            let across = -(gi as f32) * ay + gj as f32 * ax;
            0.70 + 0.65 * value_noise(across * 0.40, along * 0.03)
        } else {
            1.0
        };
        let spread = FLOW_SPREAD * (m - p.mass[to]).max(0.0);
        let pull = FLOW_GRAVITY * (gx * dx + gy * dy).max(0.0) * m * visc;
        ((spread + pull) * fluid * STEP_S).min(m * 0.24)
    };

    for y in 0..CH {
        for x in 0..CH {
            let n = y * CH + x;
            let i = (y + OFF) * PAD + (x + OFF);
            if !p.valid[i] {
                continue;
            }
            let h = p.heat[i];
            // Heat: diffusion (insulated at invalid neighbours) and cooling.
            let mut sum = 0.0;
            for &(o, _, _) in &dirs {
                let j = (i as isize + o) as usize;
                sum += if p.valid[j] { p.heat[j] } else { h };
            }
            let mut nh = (h + HEAT_DIFFUSE * (sum * 0.25 - h)) * decay;
            if nh < 0.004 {
                nh = 0.0;
            }

            // Carve saturates at 1.0: melting stops at the maximum level.
            let c = p.carve[i];
            let target = smoothstep(MELT_START, MELT_FULL, h);
            let nc = c + (target - c).max(0.0) * (1.0 - (-CARVE_RATE * STEP_S).exp());

            // Mass: displaced material pushed in from melting neighbours, then
            // fluid flow between neighbours.
            let mut mass = p.mass[i];
            let share_here = 0.40 * (0.15 + 1.35 * (1.0 - c).powi(2));
            for &(o, dx, dy) in &dirs {
                let j = (i as isize + o) as usize;
                if p.valid[j] {
                    mass += displaced(p.heat[j], p.carve[j]) * share_here;
                    mass += flux(j, i, -dx, -dy);
                }
                mass -= flux(i, j, dx, dy);
            }
            mass = mass.clamp(0.0, 2.4);
            if mass < 0.0015 {
                mass = 0.0;
            }

            let soot = p.soot[i].max(smoothstep(0.16, 0.65, h)).max(nc);
            out[0][n] = nh;
            out[1][n] = nc;
            out[2][n] = mass;
            out[3][n] = soot;
        }
    }
    out
}

// ---- Emission ---------------------------------------------------------------------

struct Vertex {
    pos: Vec3,
    normal: Vec3,
    skin: [u8; 4],
    glow: [u8; 3],
    skin_a: f32,
    glow_a: f32,
}

/// Separable [1 2 1]/4 blur on the padded grid (edges keep their value).
fn blur3x3(f: &mut [f32; PP]) {
    let src = *f;
    for y in 1..PAD - 1 {
        for x in 1..PAD - 1 {
            let i = y * PAD + x;
            f[i] = (src[i - 1] + 2.0 * src[i] + src[i + 1]) * 0.25;
        }
    }
    let src = *f;
    for y in 1..PAD - 1 {
        for x in 1..PAD - 1 {
            let i = y * PAD + x;
            f[i] = (src[i - PAD] + 2.0 * src[i] + src[i + PAD]) * 0.25;
        }
    }
}

fn emit_chunk(surface: &Surface, key: (i32, i32), chunk: &Chunk, now: i32, draws: &mut Vec<FxDraw>) {
    let nh = Neighborhood::new(surface, key);
    let p = Padded::gather(&nh);
    let (kx, ky) = (key.0 * CH_I, key.1 * CH_I);

    // Per-node presence (staggered revert), solid height, and total height.
    let mut amp = [0.0_f32; PP];
    let mut hs = [0.0_f32; PP];
    for py in 0..PAD {
        for px in 0..PAD {
            let i = py * PAD + px;
            let noise = hash2(kx + px as i32 - OFF_I, ky + py as i32 - OFF_I);
            let r = (p.revert[i] * 1.35 - noise * 0.35).clamp(0.0, 1.0);
            amp[i] = 1.0 - r * r * (3.0 - 2.0 * r);
            hs[i] = p.mass[i] * LIP_HEIGHT;
        }
    }
    // Bubbles inflate then pop; they only exist in this chunk's interior.
    let mut bubble_glow = [0.0_f32; PP];
    for b in &chunk.bubbles {
        let tau = ((now - b.born) as f32 / b.life as f32).clamp(0.0, 1.0);
        let swell = (tau.powf(0.55) * std::f32::consts::FRAC_PI_2).sin();
        let r = b.radius.ceil() as i32 + 1;
        for oy in -r..=r {
            for ox in -r..=r {
                let (nx, ny) = (b.x.round() as i32 + ox, b.y.round() as i32 + oy);
                if !(-OFF_I..=CH_I + OFF_I).contains(&nx) || !(-OFF_I..=CH_I + OFF_I).contains(&ny) {
                    continue;
                }
                let d2 = (nx as f32 - b.x).powi(2) + (ny as f32 - b.y).powi(2);
                let k = 1.0 - d2 / (b.radius * b.radius);
                if k <= 0.0 {
                    continue;
                }
                let i = (ny + OFF_I) as usize * PAD + (nx + OFF_I) as usize;
                let dome = b.amp * swell * k * k;
                hs[i] += dome * amp[i];
                bubble_glow[i] += dome;
            }
        }
    }
    let mut ht = [0.0_f32; PP];
    for i in 0..PP {
        ht[i] = hs[i] - p.carve[i] * GROOVE_DEPTH * amp[i];
    }
    // The relief has to stay cheap, so instead of more geometry we blur the
    // field: two binomial passes give a soft, rounded berm rather than a
    // lumpy per-node one. Padding covers the stencil so chunk seams match.
    for _ in 0..2 {
        blur3x3(&mut hs);
        blur3x3(&mut ht);
    }

    let light_dir = normalize([0.35, 0.45, 0.82]);
    let width = CH + 1;
    let mut verts: Vec<Vertex> = Vec::with_capacity(width * width);
    for y in 0..=CH {
        for x in 0..=CH {
            let i = (y + OFF) * PAD + (x + OFF);
            let valid = p.valid[i];
            let a = amp[i];
            let heat = p.heat[i] * a;
            let carve = p.carve[i];
            let mass = p.mass[i];

            let dx = (ht[i + 1] - ht[i - 1]) / (2.0 * CELL);
            let dy = (ht[i + PAD] - ht[i - PAD]) / (2.0 * CELL);
            let nt = normalize([-dx, -dy, 1.0]);
            let normal = normalize(add(add(scale(surface.u, nt[0]), scale(surface.v, nt[1])), scale(surface.n, nt[2])));

            let gx = (kx + x as i32) as f32;
            let gy = (ky + y as i32) as f32;
            let lift = hs[i].max(0.0);
            let structure = (p.soot[i] * 0.8).max(carve * 1.2).max((mass * 3.0).min(1.0));
            // The z-fight bias only applies where the skin is actually visible, so
            // the berm meets the wall exactly at its outer edge and never appears
            // to hover off it when seen from the side.
            let bias = SKIN_BIAS * smoothstep(0.02, 0.35, structure * a);
            let pos = surface.node_pos(gx, gy, p.base[i] + lift + bias);

            let skin_a = if valid { (structure.clamp(0.0, 1.0) * a).min(1.0) } else { 0.0 };

            // Cooled slag albedo: dark scorch, greyer on the raised crests.
            let grain = value_noise(gx * 0.9, gy * 0.9);
            let crest = smoothstep(0.1, 1.4, lift);
            let mut albedo = lerp3([0.030, 0.026, 0.024], [0.085, 0.074, 0.066], (crest * 0.55 + grain * 0.25).min(1.0));
            albedo = lerp3([0.014, 0.012, 0.011], albedo, smoothstep(0.08, 0.5, carve + mass));
            let cavity = 1.0 - 0.35 * carve;
            let relief = dot(sub(normal, surface.n), light_dir);
            let shade = (1.0 + 3.0 * relief).clamp(0.35, 1.7) * cavity;
            let ember = blackbody(heat);
            let warm = (heat * 1.2).clamp(0.0, 1.0);
            let rgb = lerp3(scale(albedo, shade), scale(ember, 0.55), warm);
            let skin = [
                (rgb[0] * 255.0).clamp(0.0, 255.0) as u8,
                (rgb[1] * 255.0).clamp(0.0, 255.0) as u8,
                (rgb[2] * 255.0).clamp(0.0, 255.0) as u8,
                (skin_a * 255.0) as u8,
            ];

            // Emissive: heat plus a hotter bulge where molten mass and bubbles are.
            let gate = smoothstep(0.02, 0.30, heat);
            let fluid = smoothstep(FLUID_START, FLUID_FULL, heat);
            let e = blackbody(heat + mass * 0.10 * fluid + bubble_glow[i] * 0.18 * fluid);
            let intensity = gate * (0.92 + 0.08 * (now as f32 * 0.02 + gx * 1.7 + gy * 1.3).sin());
            let glow = [
                (e[0] * intensity * 255.0).clamp(0.0, 255.0) as u8,
                (e[1] * intensity * 255.0).clamp(0.0, 255.0) as u8,
                (e[2] * intensity * 255.0).clamp(0.0, 255.0) as u8,
            ];
            let glow_a = if valid { intensity } else { 0.0 };
            verts.push(Vertex { pos, normal, skin, glow, skin_a, glow_a });
        }
    }

    let build = |use_glow: bool| -> (Vec<usize>, Vec<u32>) {
        let mut remap = vec![u32::MAX; width * width];
        let mut order: Vec<usize> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        for b in 0..CH {
            for a in 0..CH {
                let ids = [b * width + a, b * width + a + 1, (b + 1) * width + a, (b + 1) * width + a + 1];
                let visible = ids.iter().any(|&v| {
                    if use_glow { verts[v].glow_a > 0.012 } else { verts[v].skin_a > 0.01 }
                });
                if !visible {
                    continue;
                }
                let mut local = [0u32; 4];
                for (slot, &v) in ids.iter().enumerate() {
                    if remap[v] == u32::MAX {
                        remap[v] = order.len() as u32;
                        order.push(v);
                    }
                    local[slot] = remap[v];
                }
                let [v00, v10, v01, v11] = local;
                // Both windings: the dynamic pipeline back-face culls.
                let tris: [[u32; 3]; 2] = if (a + b) % 2 == 0 {
                    [[v00, v10, v11], [v00, v11, v01]]
                } else {
                    [[v00, v10, v01], [v10, v11, v01]]
                };
                for t in tris {
                    indices.extend_from_slice(&[t[0], t[1], t[2], t[0], t[2], t[1]]);
                }
            }
        }
        (order, indices)
    };

    let (order, indices) = build(false);
    if !indices.is_empty() {
        let centre = surface.node_pos((kx + CH_I / 2) as f32, (ky + CH_I / 2) as f32, 3.0);
        draws.push(FxDraw::LitMesh {
            positions: order.iter().map(|&v| verts[v].pos).collect(),
            normals: order.iter().map(|&v| verts[v].normal).collect(),
            rgba: order.iter().map(|&v| verts[v].skin).collect(),
            indices,
            lighting_origin: centre,
        });
    }
    let (order, indices) = build(true);
    if !indices.is_empty() {
        draws.push(FxDraw::Mesh {
            positions: order.iter().map(|&v| madd(verts[v].pos, surface.n, GLOW_LIFT)).collect(),
            uvs: vec![[0.5, 0.5]; order.len()],
            rgba: order.iter().map(|&v| [verts[v].glow[0], verts[v].glow[1], verts[v].glow[2], 255]).collect(),
            indices,
            shader: "$melt_glow".to_owned(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jka_movement::TraceResult;

    /// Infinite wall facing +X at x = 0.
    struct Wall;
    impl TraceWorld for Wall {
        fn trace(&mut self, q: TraceQuery) -> TraceResult {
            let mut r = TraceResult::clear(q.end);
            let (s, e) = (q.start[0], q.end[0]);
            if (s > 0.0) != (e > 0.0) || s == 0.0 {
                let t = if (s - e).abs() < 1.0e-6 { 0.0 } else { s / (s - e) };
                r.fraction = t.clamp(0.0, 1.0);
                r.end = [0.0, q.start[1] + (q.end[1] - q.start[1]) * t, q.start[2] + (q.end[2] - q.start[2]) * t];
                r.normal = [1.0, 0.0, 0.0];
                r.entity = ENTITY_WORLD;
            }
            r
        }
        fn point_contents(&mut self, _p: Vec3, _e: i32) -> i32 {
            0
        }
    }

    fn hold(melt: &mut SaberMelt, from_ms: i32, to_ms: i32) {
        let mut world = Wall;
        let mut t = from_ms;
        while t < to_ms {
            melt.contact(1, [0.0, 10.0, 50.0], [1.0, 0.0, 0.0], t, Some(&mut world));
            melt.update(t, Some(&mut world));
            t += 16;
        }
    }

    fn max_field(melt: &SaberMelt, field: usize) -> f32 {
        melt.surfaces
            .iter()
            .flat_map(|s| s.chunks.values())
            .flat_map(|c| c.f[field].iter().copied())
            .fold(0.0, f32::max)
    }

    #[test]
    fn held_blade_melts_to_a_bounded_depth_and_raises_lips() {
        let mut melt = SaberMelt::new();
        hold(&mut melt, 1000, 4000);
        let carve = max_field(&melt, F_CARVE);
        let mass = max_field(&melt, F_MASS);
        assert!(carve > 0.9, "dwell should reach the melt depth, got {carve}");
        assert!(carve <= 1.0 + 1.0e-4, "melting must saturate");
        assert!(mass > 0.05, "displaced material should form lips, got {mass}");
        let mut world = Wall;
        assert!(melt.chunk_count() > 0);
        melt.update(4000, Some(&mut world));
    }

    #[test]
    fn cools_holds_then_reverts_to_nothing() {
        let mut melt = SaberMelt::new();
        hold(&mut melt, 1000, 4000);
        assert!(max_field(&melt, F_HEAT) > 0.5);
        let mut world = Wall;
        let mut t = 4000;
        let mut saw_cool = false;
        while t < 4000 + 40_000 {
            t += 20;
            melt.update(t, Some(&mut world));
            if t == 4000 + 12_000 {
                assert!(max_field(&melt, F_HEAT) < COLD_PEAK, "should be cold after 12 s");
                assert!(max_field(&melt, F_CARVE) > 0.5, "cooled relief persists for a while");
                saw_cool = true;
            }
        }
        assert!(saw_cool);
        assert_eq!(melt.chunk_count(), 0, "everything reverts to the original surface");
    }

    #[test]
    fn emits_lit_relief_and_glow_while_hot() {
        let mut melt = SaberMelt::new();
        hold(&mut melt, 1000, 2500);
        let mut draws = Vec::new();
        let mut lights = Vec::new();
        melt.emit(2500, [200.0, 0.0, 50.0], &mut draws, &mut lights);
        assert!(draws.iter().any(|d| matches!(d, FxDraw::LitMesh { .. })));
        assert!(draws.iter().any(|d| matches!(d, FxDraw::Mesh { shader, .. } if shader == "$melt_glow")));
        assert!(!lights.is_empty());
        for d in &draws {
            if let FxDraw::LitMesh { positions, normals, rgba, indices, .. } = d {
                assert_eq!(positions.len(), normals.len());
                assert_eq!(positions.len(), rgba.len());
                assert!(indices.iter().all(|&i| (i as usize) < positions.len()));
                // The wall is the plane x = 0 and faces +X: the skin stays in front of it.
                assert!(positions.iter().all(|p| p[0] > 0.0));
            }
        }
    }

    #[test]
    fn no_geometry_past_the_edge_of_the_surface() {
        struct Ledge;
        impl TraceWorld for Ledge {
            fn trace(&mut self, q: TraceQuery) -> TraceResult {
                let mut r = Wall.trace(q);
                if r.fraction < 1.0 && r.end[1] > 12.0 {
                    r = TraceResult::clear(q.end);
                }
                r
            }
            fn point_contents(&mut self, _p: Vec3, _e: i32) -> i32 {
                0
            }
        }
        let mut melt = SaberMelt::new();
        let mut world = Ledge;
        for t in (1000..2500).step_by(16) {
            melt.contact(1, [0.0, 11.0, 50.0], [1.0, 0.0, 0.0], t, Some(&mut world));
            melt.update(t, Some(&mut world));
        }
        let mut draws = Vec::new();
        let mut lights = Vec::new();
        melt.emit(2500, [200.0, 0.0, 50.0], &mut draws, &mut lights);
        for d in &draws {
            if let FxDraw::LitMesh { positions, rgba, .. } = d {
                for (p, c) in positions.iter().zip(rgba) {
                    if p[1] > 13.5 {
                        assert_eq!(c[3], 0, "skin must be transparent beyond the ledge");
                    }
                }
            }
        }
    }
}
