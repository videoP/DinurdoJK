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
use jka_movement::{TraceQuery, TraceWorld};
use std::{collections::{HashMap, HashSet}, sync::Arc};

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
/// MASK_SOLID: CONTENTS_SOLID | CONTENTS_TERRAIN.
const MASK_SOLID: i32 = 0x0000_0001 | 0x0000_1000;
/// SURF_NOIMPACT (surfaceflags.h): no impact effect on this surface.
const SURF_NOIMPACT: i32 = 0x0008_0000;

type Axis = [[f32; 3]; 3];

/// Presentation ownership for a spawned FX tree. Child/death/emitter effects
/// inherit the root class so runtime A/B switches suppress the authored tree
/// without guessing from shader names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FxClass {
    #[default]
    General,
    SaberImpact,
    /// Stable ET_FX source. Descendant/death/emitter effects inherit this class.
    MapRunner(u16),
}

/// Effect ids a primitive refers to, resolved once at registration.
#[derive(Clone, Debug, Default)]
struct Links {
    impact: Vec<EffectId>,
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
    auto_light_driver: bool,
}

#[derive(Clone, Copy, Debug)]
struct AutoLightRuntime {
    effect: EffectId,
    score: i16,
    spot: Option<FxSpotLight>,
}

/// One live primitive (CParticle/CLine/CTail/... state).
#[derive(Clone, Debug)]
struct Primitive {
    /// GPU candidate identity, assigned at birth only for semantically safe sprites.
    gpu_id: Option<u64>,
    /// Immutable birth parameters retained for rare CPU handback after an
    /// already GPU-owned particle outlives a pipeline rebuild/toggle.
    gpu_spawn: Option<super::gpu_sparks::GpuSparkSpawn>,
    gpu_sleeping: bool,
    class: FxClass,
    kind: PrimType,
    auto_light: Option<AutoLightRuntime>,
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
    // CParticle physics state (only consulted when `flags` has FX_APPLY_PHYSICS).
    impact_fx: EffectId,
    elasticity: f32,
    mins: [f32; 3],
    maxs: [f32; 3],
    // Emitter state.
    emitter_fx: EffectId,
    density: f32,
    variance: f32,
    old_time: i32,
    old_velocity: [f32; 3],
    angles: [f32; 3],
    angle_delta: [f32; 3],
}

/// An authored `Decal {}` request emitted once by the FX worker. Actual BSP
/// clipping is performed by the presentation layer's R_MarkFragments port.
/// Defer it rather than drawing a billboard: it must never float through a wall.
#[derive(Clone, Debug)]
pub struct FxDecalSpawn {
    pub origin: [f32; 3],
    pub normal: [f32; 3],
    pub radius: f32,
    pub rotation: f32,
    pub shader: String,
    pub rgba: [u8; 4],
    pub life_ms: i32,
    pub class: FxClass,
    /// TaystJK FX_GHOUL2_DECALS: also project onto intersected skinned models.
    pub ghoul2: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FxDraw {
    /// RT_SPRITE: view-facing quad of half-extent `radius`.
    /// `shader_time` is refEntity_t::shaderTime in seconds: zero uses the
    /// global shader clock, otherwise stages run from that time (setShaderTime).
    Sprite { shader_time: f32, origin: [f32; 3], radius: f32, rotation: f32, rgba: [u8; 4], shader: String },
    /// RT_SABER_GLOW: JKA's renderer-side chain of view-facing glow sprites.
    /// Keeping this as one primitive preserves the stock split between CGame's
    /// blade flicker and RB_SurfaceSaberGlow's own hilt pulse/randomness.
    SaberGlow {
        origin: [f32; 3],
        direction: [f32; 3],
        length: f32,
        radius: f32,
        hilt_radius: f32,
        rgba: [u8; 4],
        shader: String,
    },
    /// RT_ORIENTED_QUAD: quad in axis[1]/axis[2] of half-extent `radius`.
    OrientedQuad { shader_time: f32, origin: [f32; 3], axis: Axis, radius: f32, rotation: f32, rgba: [u8; 4], shader: String },
    /// Camera-facing (or velocity-aligned) quad with independent half extents
    /// and a texture sub-rectangle, for baked flipbook particles.
    /// `facing` places the quad in the plane perpendicular to it (custom facing).
    /// `ground`: world z of the floor under the effect. Sprites that reach
    /// it fade out towards it instead of being cut by a hard depth edge.
    Billboard { origin: [f32; 3], half: [f32; 2], rotation: f32, uv: [f32; 4], rgba: [u8; 4], align: Option<[f32; 3]>, facing: Option<[f32; 3]>, ground: Option<f32>, pivot: [f32; 2], shader: String },
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
    /// Opaque, depth-writing relief mesh with real normals and no texture (vertex
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
    /// Inferred from an emissive legacy EFX that had no authored Light{} stage.
    PromotedEffect,
    /// TaystJK CG_Missile's registered weaponInfo_t missileDlight.
    Missile,
    /// TaystJK CG_EntityEffects packed entityState_t.constantLight.
    EntityConstant,
    /// CG_PlayerPowerups flag-carrier / quad light.
    PlayerPowerup,
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
    /// Illumination/falloff radius, matching RE_AddLightToScene semantics.
    pub radius: f32,
    /// Physical emitter radius used only by finite-emitter RT shadow sampling.
    /// Zero preserves the authored/JKA mathematical point-light behavior.
    pub source_radius: f32,
    pub rgb: [f32; 3],
    /// Optional projected/spot-light cone in JKA space. Point and segment
    /// lights leave this unset.
    pub spot: Option<FxSpotLight>,
    /// Optional rendered blade endpoints in JKA space. Legacy modes use origin.
    pub segment: Option<[[f32; 3]; 2]>,
    pub blade_segments: Option<std::sync::Arc<[FxLightSegment]>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FxSpotLight {
    pub direction: [f32; 3],
    pub inner_cone_cos: f32,
    pub outer_cone_cos: f32,
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

/// A lit saber blade this frame, kept apart from its FX draws so the renderer
/// can keep the volumetric clouds from compositing over it (the blade is
/// additive light in front of the sky, but sits in no depth buffer).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FxBlade {
    pub start: [f32; 3],
    pub end: [f32; 3],
    /// The authored glow radius in world units.
    pub radius: f32,
}

#[derive(Default, Debug)]
pub struct FxFrame {
    pub draws: Vec<FxDraw>,
    /// Only newly spawned author-written decal primitives, not persistent draws.
    pub decals: Vec<FxDecalSpawn>,
    /// Authored decals requesting Ghoul2 mesh projection, after world projection.
    pub ghoul2_decals: Vec<FxDecalSpawn>,
    /// Original draw indices for map-runner descendants; populated only in the opt-in path.
    pub runner_draws: Vec<(usize, u16)>,
    /// Optional once-per-spawn GPU staging; the existing CPU draw path remains
    /// authoritative until renderer material-stage eligibility is connected.
    pub gpu_sparks: Option<super::gpu_sparks::GpuSparkCommands>,
    /// `(draw_index, stable_particle_id)` for ordinary CPU sprite fallbacks.
    /// The app promotes these by material and leaves every unaccepted draw intact.
    pub gpu_sprite_draw_indices: Vec<(usize, u64)>,
    pub blades: Vec<FxBlade>,
    pub lights: Vec<FxLight>,
    /// Material-aware second-stage candidates for inferred lights. These are
    /// intentionally kept separate from authored lights until the main thread
    /// can inspect the resolved shader and decoded texture.
    pub auto_light_candidates: Vec<FxAutoLightCandidate>,
    pub sounds: Vec<FxSound>,
    pub shakes: Vec<FxShake>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FxAutoLightCandidate {
    pub effect: EffectId,
    pub score: i16,
    pub kind: PrimType,
    pub origin: [f32; 3],
    pub radius: f32,
    pub rgba: [u8; 4],
    pub shader: String,
    /// Original EFX lifetime; does not modify authored visual simulation.
    pub life_ms: i32,
    pub spot: Option<FxSpotLight>,
    pub segment: Option<[[f32; 3]; 2]>,
}

/// A `CameraShake` primitive that spawned: `CG_DoCameraShake` inputs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FxShake {
    pub origin: [f32; 3],
    /// The primitive's elasticity value.
    pub intensity: f32,
    pub radius: i32,
    pub time_ms: i32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FxStats {
    pub registered: usize,
    pub active: usize,
    /// Actual FX primitives skipping their CPU tick (not just GPU candidates).
    pub gpu_offloaded: usize,
    pub scheduled: usize,
    pub dropped: u64,
    /// Primitive kinds that are parsed but not simulated/drawn yet.
    pub unsupported_spawns: u64,
    /// Primitives skipped by authored `cullRange` (`fx_lod` >= 1).
    pub lod_culled: u64,
    /// Particle/tail spawns removed by screen-size density scaling (`fx_lod` 2).
    pub lod_saved: u64,
}

/// `fx_lod` / `fx_countScale` state plus the last view the FX thread was given.
#[derive(Clone, Copy, Debug)]
struct LodState {
    mode: u32,
    /// `fx_countScale`, clamped to 0..=1 (stock never scales upward).
    count_scale: f32,
    /// `r_fxLodScale`: multiplies authored cullRange; scales adaptive density.
    lod_scale: f32,
    /// View origin and projected pixels per world unit at depth 1.
    view: Option<([f32; 3], f32)>,
}

impl LodState {
    const fn new() -> Self {
        Self { mode: super::FX_LOD_DEFAULT, count_scale: 1.0, lod_scale: super::LOD_SCALE_DEFAULT, view: None }
    }
}

/// How many times to spawn `prim` for one `PlayEffect`, or `None` when it is
/// culled outright. Mode 0 with `fx_countScale` 1 is CFxScheduler::PlayEffect
/// exactly, including the order of random draws.
fn primitive_spawn_count(
    prim: &PrimitiveTemplate,
    lod: &LodState,
    origin: [f32; 3],
    rng: &mut Rng,
    culled: &mut u64,
    saved: &mut u64,
) -> Option<i32> {
    let view = lod.view.filter(|_| lod.mode >= super::FX_LOD_AUTHORED);
    let dist_sq = view.map(|(eye, _)| {
        let d = sub(origin, eye);
        d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
    });
    if let Some(dist_sq) = dist_sq {
        // Sound and CameraShake are semantic, not density; never cull them.
        let visual = !matches!(prim.kind, PrimType::Sound | PrimType::CameraShake);
        if visual && prim.cull_range_sq > 0.0 && dist_sq > prim.cull_range_sq * lod.lod_scale * lod.lod_scale {
            *culled += 1;
            return None;
        }
    }

    // fx_countScale: only a count range wider than 1 is treated as scalable.
    let mut value = prim.spawn_count.get(rng);
    if (prim.spawn_count.max - prim.spawn_count.min).abs() > 1.0 {
        value *= lod.count_scale;
    }
    let stock = round(value);
    let mut count = stock;

    // Screen-size density: populations of small elements only, decided here.
    let populated = matches!(prim.kind, PrimType::Particle | PrimType::OrientedParticle | PrimType::Tail)
        && prim.spawn_count.max >= 3.0;
    if let (true, Some(dist_sq), Some((_, px_per_unit))) = (populated, dist_sq, view) {
        if lod.mode >= super::FX_LOD_ADAPTIVE {
            let px = prim.lod_size() * px_per_unit * (lod.lod_scale / super::LOD_SCALE_DEFAULT) / dist_sq.sqrt().max(1.0);
            let density = super::lod_density(px);
            if density < 1.0 {
                // Stochastic rounding keeps the expected density exact.
                let scaled = value * density;
                let base = scaled.floor();
                count = base as i32 + i32::from(rng.flrand(0.0, 1.0) < scaled - base);
            }
        }
    }

    if prim.spawn_count.min >= 1.0 && count < 1 {
        count = 1;
    }
    *saved += (stock - count).max(0) as u64;
    Some(count)
}

pub struct FxSystem {
    effects: Vec<LoadedEffect>,
    ids: HashMap<String, EffectId>,
    failed: HashSet<String>,
    scheduled: Vec<Scheduled>,
    active: Vec<Primitive>,
    // Stage-only transport for GPU particle ownership; disabled in production
    // until the corresponding unified-renderer consumer has been validated.
    gpu_staging: bool,
    gpu_generation: u64,
    gpu_next_id: u64,
    gpu_pending_reset: bool,
    gpu_pending_spawns: Vec<super::gpu_sparks::GpuSparkSpawn>,
    /// Only IDs accepted by the material resolver AND compiled resident PSOs.
    /// Set through the ordered FX worker command queue, not GPU readback.
    gpu_owned: HashSet<u64>,
    gpu_offloaded_last_frame: usize,
    sounds: Vec<FxSound>,
    shakes: Vec<FxShake>,
    decal_spawns: Vec<FxDecalSpawn>,
    /// FxRunner PlayEffect calls made while creating a primitive.
    runner_queue: Vec<(EffectId, [f32; 3], Axis, FxClass)>,
    time: i32,
    old_time: i32,
    frame_time: i32,
    real_time: f32,
    rng: Rng,
    dropped: u64,
    unsupported_spawns: u64,
    lod: LodState,
    lod_culled: u64,
    lod_saved: u64,
    /// `fx_physics` (see `fx::FX_PHYSICS_*`).
    physics_mode: u32,
    /// Presentation-only world for CParticle physics traces. Without one,
    /// particles fly freely exactly as they did before `fx_physics` existed.
    collision: Option<Box<dyn TraceWorld + Send>>,
    /// Promotion metadata prepared by the map worker. Runtime EFX parsing stays
    /// lazy for normal OpenJK behavior, but it no longer scans materials/images.
    light_promotion: Arc<super::light_promotion::FxLightPromotionCatalog>,
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
            gpu_staging: false,
            gpu_generation: 0,
            gpu_next_id: 0,
            gpu_pending_reset: false,
            gpu_pending_spawns: Vec::new(),
            gpu_owned: HashSet::new(),
            gpu_offloaded_last_frame: 0,
            sounds: Vec::new(),
            shakes: Vec::new(),
            decal_spawns: Vec::new(),
            runner_queue: Vec::new(),
            time: 0,
            old_time: 0,
            frame_time: 0,
            real_time: 0.0,
            rng: Rng::new(0x5EED_F00D),
            dropped: 0,
            unsupported_spawns: 0,
            lod: LodState::new(),
            lod_culled: 0,
            lod_saved: 0,
            physics_mode: super::FX_PHYSICS_DEFAULT,
            collision: None,
            light_promotion: Arc::new(super::light_promotion::FxLightPromotionCatalog::default()),
        }
    }

    /// `fx_lod` (see `fx::FX_LOD_*`), the stock `fx_countScale` cvar and `r_fxLodScale`.
    pub fn set_lod(&mut self, mode: u32, count_scale: f32, lod_scale: f32) {
        self.lod.lod_scale = if lod_scale.is_finite() {
            lod_scale.clamp(super::LOD_SCALE_MIN, super::LOD_SCALE_MAX)
        } else {
            super::LOD_SCALE_DEFAULT
        };
        self.lod.mode = mode.min(super::FX_LOD_ADAPTIVE);
        self.lod.count_scale = if count_scale.is_finite() { count_scale.clamp(0.0, 1.0) } else { 1.0 };
    }

    /// Latest render view for LOD decisions: origin and projected pixels per
    /// world unit at depth 1 (`viewport_height / (2 * tan(fov_y / 2))`).
    pub fn set_lod_view(&mut self, origin: [f32; 3], px_per_unit: f32) {
        self.lod.view = (px_per_unit.is_finite() && px_per_unit > 0.0).then_some((origin, px_per_unit));
    }

    /// `fx_physics`: 0 off, 1 non-expensive only (inert, like stock), 2 authored
    /// `expensivePhysics`, 3 force the trace on every physics primitive.
    pub fn set_physics_mode(&mut self, mode: u32) {
        self.physics_mode = mode.min(super::FX_PHYSICS_ALL);
    }

    /// World used by `fx_physics` traces (`None` disables collision).
    pub fn set_collision(&mut self, world: Option<Box<dyn TraceWorld + Send>>) {
        self.collision = world;
    }

    /// Install the immutable promotion profiles prepared during map loading.
    /// Existing registered templates are patched too because a live CGame can
    /// be created before the overlapped map worker finishes.
    pub fn set_light_promotion_catalog(
        &mut self,
        catalog: Arc<super::light_promotion::FxLightPromotionCatalog>,
    ) {
        self.light_promotion = catalog;
        for effect in &mut self.effects {
            effect.template.auto_light = self
                .light_promotion
                .effect_profile(&effect.template.name);
        }
    }

    /// Backend integration switch, deliberately not registered as a user cvar
    /// until a GPU resident renderer handles spawn/clear/material fallback.
    /// Staging does not steal ownership from CPU simulation or drawing.
    pub fn set_gpu_staging(&mut self, enabled: bool) {
        if self.gpu_staging != enabled {
            self.gpu_staging = enabled;
            self.gpu_generation = self.gpu_generation.wrapping_add(1);
            self.gpu_pending_reset = true;
            self.gpu_pending_spawns.clear();
            self.gpu_owned.clear();
        }
    }

    /// Material- and pipeline-validated GPU ownership from presentation. A
    /// missing ID always returns to the reference CPU evaluator next tick.
    pub fn set_gpu_owned(&mut self, ids: HashSet<u64>) {
        self.gpu_owned = if self.gpu_staging { ids } else { HashSet::new() };
    }

    pub fn stats(&self) -> FxStats {
        FxStats {
            registered: self.effects.len(),
            active: self.active.len(),
            gpu_offloaded: self.gpu_offloaded_last_frame,
            scheduled: self.scheduled.len(),
            dropped: self.dropped,
            unsupported_spawns: self.unsupported_spawns,
            lod_culled: self.lod_culled,
            lod_saved: self.lod_saved,
        }
    }

    /// CFxScheduler::Clean: drop live and scheduled primitives (seek/restart).
    pub fn clear_active(&mut self) {
        self.gpu_generation = self.gpu_generation.wrapping_add(1);
        self.gpu_pending_reset = true;
        self.gpu_pending_spawns.clear();
        self.gpu_owned.clear();
        self.gpu_offloaded_last_frame = 0;
        self.active.clear();
        self.scheduled.clear();
        self.sounds.clear();
        self.decal_spawns.clear();
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
        let mut template = parse_effect_without_auto_light(&key, &gp2::parse(&bytes));
        template.auto_light = self.light_promotion.effect_profile(&key);
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
            links.push(Links {
                impact: resolve(impact),
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
        let lod = self.lod;
        let count_per_prim: Vec<(usize, i32, f32, bool)> = effect
            .template
            .primitives
            .iter()
            .enumerate()
            .filter_map(|(index, prim)| {
                let count = primitive_spawn_count(
                    prim,
                    &lod,
                    origin,
                    &mut self.rng,
                    &mut self.lod_culled,
                    &mut self.lod_saved,
                )?;
                let even = prim.spawn_flags & FX_EVEN_DISTRIBUTION != 0;
                let factor = if even {
                    (prim.spawn_delay.max - prim.spawn_delay.min).abs() / count.max(1) as f32
                } else {
                    0.0
                };
                Some((index, count, factor, even))
            })
            .collect();
        let auto_light_profile = self.effects[(id - 1) as usize].template.auto_light;
        for (index, count, factor, even) in count_per_prim {
            for t in 0..count {
                let auto_light_driver = t == 0
                    && auto_light_profile
                        .is_some_and(|profile| profile.driver_primitive == index);
                let delay = if even {
                    (t as f32 * factor) as i32
                } else {
                    self.effects[(id - 1) as usize].template.primitives[index].spawn_delay.get(&mut self.rng) as i32
                };
                if delay < 1 {
                    self.create_effect(id, index, origin, axis, -delay, class, auto_light_driver);
                } else if self.scheduled.len() < MAX_SCHEDULED {
                    self.scheduled.push(Scheduled {
                        start_time: self.time + delay,
                        effect: id,
                        primitive: index,
                        origin,
                        axis,
                        class,
                        auto_light_driver,
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
        self.frame_impl::<false>(saber_impact_fx)
    }

    pub fn frame_with_runner_tracking(&mut self, saber_impact_fx: bool) -> FxFrame {
        self.frame_impl::<true>(saber_impact_fx)
    }

    // Separate monomorphizations: OFF does not inspect runner classes or record indices.
    fn frame_impl<const TRACK_RUNNERS: bool>(&mut self, saber_impact_fx: bool) -> FxFrame {
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
                scheduled.auto_light_driver,
            );
        }

        let mut out = FxFrame {
            decals: std::mem::take(&mut self.decal_spawns),
            sounds: std::mem::take(&mut self.sounds),
            shakes: std::mem::take(&mut self.shakes),
            ..FxFrame::default()
        };
        let mut deaths = Vec::new();
        let mut emits = Vec::new();
        let (time, real_time, frame_time) = (self.time, self.real_time, self.frame_time);
        let rng = &mut self.rng;
        let physics_mode = self.physics_mode;
        let collision = &mut self.collision;
        let owned = &self.gpu_owned;
        let mut gpu_offloaded = 0;
        self.active.retain_mut(|prim| {
            if time > prim.kill_time {
                if prim.flags & FX_DEATH_RUNS_FX != 0 && prim.flags & FX_KILL_ON_IMPACT == 0 && prim.death_fx != 0 {
                    // CParticle::Die: death effect in a random direction.
                    let dir = normalize([rng.flrand(-1.0, 1.0), rng.flrand(-1.0, 1.0), rng.flrand(-1.0, 1.0)]);
                    deaths.push((prim.death_fx, prim.origin, dir, prim.class));
                }
                return false;
            }
            // Eligible resident sprites have no per-tick callbacks, collision,
            // RNG draws or promoted lights. Once the renderer ACKs an ID, the
            // GPU evaluates its transform/colour/lifetime and the worker skips
            // update_primitive entirely (including the CPU sprite allocation).
            let gpu_owned = prim.gpu_id.is_some_and(|id| owned.contains(&id))
                && prim.gpu_spawn.as_ref().is_some_and(|spawn| {
                    super::gpu_sparks::primitive_eligibility(
                        prim.kind, prim.flags, physics_mode,
                        prim.impact_fx, prim.death_fx, prim.emitter_fx,
                        prim.auto_light.is_some(), spawn.values_finite(),
                    ).is_ok()
                });
            if gpu_owned {
                gpu_offloaded += 1;
                prim.gpu_sleeping = true;
                return true;
            }
            // Switching off, physics mode changes and pipeline invalidation
            // all return the exact ID to the CPU path. Restore an analytic
            // previous-frame state rather than resuming a frozen old origin.
            if prim.gpu_sleeping {
                if let Some(spawn) = &prim.gpu_spawn {
                    let prev_ms = time.saturating_sub(frame_time).max(spawn.start_ms);
                    let dt = (prev_ms - spawn.start_ms) as f32 * 0.001;
                    prim.origin = std::array::from_fn(|i| spawn.origin[i]
                        + spawn.velocity[i] * dt + 0.5 * spawn.acceleration[i] * dt * dt);
                    prim.vel = std::array::from_fn(|i| spawn.velocity[i] + spawn.acceleration[i] * dt);
                    prim.rotation = spawn.start_rotation + spawn.rotation_delta * (10.0 / 0.7)
                        * (1.0 - (-0.7 * dt).exp());
                    prim.rotation_delta = spawn.rotation_delta * (-0.7 * dt).exp();
                }
                prim.gpu_sleeping = false;
            }
            let visible = saber_impact_fx || prim.class != FxClass::SaberImpact;
            let world = collision.as_deref_mut().map(|world| world as &mut dyn TraceWorld);
            let first_draw = if TRACK_RUNNERS { out.draws.len() } else { 0 };
            let alive = update_primitive(prim, time, real_time, frame_time, rng, &mut out, &mut emits, visible, physics_mode, world);
            if TRACK_RUNNERS {
                if let FxClass::MapRunner(id) = prim.class {
                    out.runner_draws.extend((first_draw..out.draws.len()).map(|i| (i, id)));
                }
            }
            alive
        });
        self.gpu_offloaded_last_frame = gpu_offloaded;
        for (id, origin, dir, class) in deaths {
            self.play_effect_dir_class(id, origin, dir, class);
        }
        for (id, origin, axis, class) in emits {
            self.play_effect_class(id, origin, axis, class);
        }
        while let Some((id, origin, axis, class)) = self.runner_queue.pop() {
            self.play_effect_class(id, origin, axis, class);
        }
        if self.gpu_staging {
            out.gpu_sparks = Some(super::gpu_sparks::GpuSparkCommands {
                generation: self.gpu_generation,
                time_ms: self.time,
                reset: std::mem::take(&mut self.gpu_pending_reset),
                spawns: std::mem::take(&mut self.gpu_pending_spawns),
            });
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
        auto_light_driver: bool,
    ) {
        // FX_Add* refuse new primitives while paused (mFrameTime < 1).
        if self.frame_time < 1 {
            return;
        }
        let effect = &self.effects[(id - 1) as usize];
        let fx = effect.template.primitives[index].clone();
        let links = effect.links.get(index).cloned().unwrap_or_default();
        let auto_light_profile = auto_light_driver
            .then_some(effect.template.auto_light)
            .flatten();
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

        // Freeze the inferred spot direction at spawn from the authored mean
        // emission axis. Do not use the randomized driver's live velocity: that
        // would make a flame-jet cone wobble as particle variance/gravity changes.
        let auto_light = auto_light_profile.map(|profile| AutoLightRuntime {
            effect: id,
            score: profile.score,
            spot: profile.spot.and_then(|spot| {
                let direction = if fx.spawn_flags & FX_VEL_IS_ABSOLUTE != 0 {
                    normalize(spot.local_direction)
                } else {
                    normalize(combine(ax, spot.local_direction))
                };
                (dot(direction, direction) > 0.5).then_some(FxSpotLight {
                    direction,
                    inner_cone_cos: spot.inner_cone_cos,
                    outer_cone_cos: spot.outer_cone_cos,
                })
            }),
        });

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
            PrimType::CameraShake => {
                // FxScheduler: theFxHelper.CameraShake(org, elasticity, radius, life).
                self.shakes.push(FxShake {
                    origin: org,
                    intensity: fx.elasticity.get(rng),
                    radius: fx.radius.get(rng) as i32,
                    time_ms: fx.life.get(rng) as i32,
                });
                return;
            }
            PrimType::Decal => {
                // CFxScheduler::CreateDecal: an authored decal is a one-shot
                // projection, *not* an animated screen-facing FX particle.
                // Keep its shader/radius/tint and project onto markable BSP
                // surfaces on the presentation side when the worker flushes.
                if let Some(shader) = shader {
                    let size = fx.size_start.get(rng).max(0.0);
                    if size > 0.0 {
                        self.decal_spawns.push(FxDecalSpawn {
                            origin: org,
                            normal: normalize(ax[0]),
                            radius: size,
                            rotation: fx.rotation.get(rng),
                            shader,
                            rgba: [
                                (rgb_start[0].clamp(0.0, 1.0) * 255.0) as u8,
                                (rgb_start[1].clamp(0.0, 1.0) * 255.0) as u8,
                                (rgb_start[2].clamp(0.0, 1.0) * 255.0) as u8,
                                // An authored mark is a persistent poly, not a
                                // zero-alpha refEntity. Keep it visible even
                                // when the primitive omits `useAlpha`.
                                if flags & FX_USE_ALPHA != 0 {
                                    (fx.alpha_start.get(rng).clamp(0.0, 1.0) * 255.0) as u8
                                } else { 255 },
                            ],
                            // FxScheduler.cpp calls AddDecalToScene without
                            // forwarding mLife; the renderer owns decal lifetime.
                            life_ms: 10_000,
                            class,
                            ghoul2: fx.flags & FX_GHOUL2_DECALS != 0,
                        });
                    }
                }
                return;
            }
            PrimType::ScreenFlash => {
                self.unsupported_spawns += 1;
                return;
            }
            PrimType::Electricity => {}
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
        let mut primitive = Primitive {
            gpu_id: None,
            gpu_spawn: None,
            gpu_sleeping: false,
            class,
            kind: fx.kind,
            auto_light,
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
            impact_fx: pick(&links.impact, rng),
            elasticity: fx.elasticity.get(rng),
            mins: fx.min,
            maxs: fx.max,
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
        if self.gpu_staging {
            // No silent promotion yet. This is intentionally a staging stream
            // while the stock FX simulation and drawing remain unchanged.
            let spawn = super::gpu_sparks::GpuSparkSpawn {
                id: self.gpu_next_id,
                generation: self.gpu_generation,
                class: primitive.class,
                shader: primitive.shader.clone().unwrap_or_default(),
                start_ms: primitive.time_start,
                end_ms: primitive.time_end,
                origin: primitive.origin,
                velocity: primitive.vel,
                acceleration: primitive.accel,
                start_rotation: primitive.rotation,
                rotation_delta: primitive.rotation_delta,
                rgb_start: primitive.rgb_start,
                rgb_end: primitive.rgb_end,
                alpha: primitive.alpha,
                size: primitive.size,
                rgb_parm: primitive.rgb_parm,
                flags: primitive.flags,
            };
            self.gpu_next_id = self.gpu_next_id.wrapping_add(1);
            if super::gpu_sparks::primitive_eligibility(
                primitive.kind,
                primitive.flags,
                self.physics_mode,
                primitive.impact_fx,
                primitive.death_fx,
                primitive.emitter_fx,
                primitive.auto_light.is_some(),
                spawn.values_finite(),
            ).is_ok() {
                primitive.gpu_id = Some(spawn.id);
                primitive.gpu_spawn = Some(spawn.clone());
                self.gpu_pending_spawns.push(spawn);
            }
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
    physics_mode: u32,
    world: Option<&mut dyn TraceWorld>,
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
        // CParticle::UpdateOrigin: v += a*dt; predict o += v*dt, then let the
        // world veto/redirect the move when physics applies.
        prim.vel = add(prim.vel, scale(prim.accel, real_time));
        let predicted = add(prim.origin, scale(prim.vel, real_time));
        match physics_step(prim, predicted, real_time, physics_mode, world, emits) {
            PhysicsStep::Die => return false,
            PhysicsStep::Handled => {}
            PhysicsStep::Free => prim.origin = predicted,
        }
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
                out.lights.push(FxLight { kind: FxLightKind::AuthoredEffect, origin: prim.origin, radius, source_radius: 0.0, rgb, spot: None, segment: None, blade_segments: None });
            }
        }
        _ => {
            let shader = prim.shader.clone().unwrap_or_default();
            let radius = percent(prim.size.2, prim.size.0, prim.size.1, FX_SIZE_SHIFT, rng);
            let rgba = rgba_at(prim, time, rng);
            if visible && !shader.is_empty() {
                if let Some(auto) = prim.auto_light {
                    let segment = if matches!(prim.kind, PrimType::Line | PrimType::Electricity) {
                        Some([prim.origin, prim.origin2])
                    } else {
                        None
                    };
                    let spot = auto.spot;
                    let light_origin = if spot.is_some() { prim.old_origin } else { prim.origin };
                    out.auto_light_candidates.push(FxAutoLightCandidate {
                        effect: auto.effect,
                        score: auto.score,
                        kind: prim.kind,
                        origin: light_origin,
                        radius,
                        rgba,
                        shader: shader.clone(),
                        life_ms: prim.time_end.saturating_sub(prim.time_start),
                        spot,
                        segment,
                    });
                }
            }
            // CEffect::SetTimeStart: FX_SET_SHADER_TIME starts animMaps and
            // waves at the primitive's own birth instead of the map clock.
            let shader_time = if prim.flags & FX_SET_SHADER_TIME != 0 { prim.time_start as f32 * 0.001 } else { 0.0 };
            match prim.kind {
                PrimType::Particle => {
                    rotate(prim, frame_time);
                    if visible {
                        if let Some(id) = prim.gpu_id {
                            out.gpu_sprite_draw_indices.push((out.draws.len(), id));
                        }
                        out.draws.push(FxDraw::Sprite { shader_time, origin: prim.origin, radius, rotation: prim.rotation, rgba, shader });
                    }
                }
                PrimType::OrientedParticle => {
                    rotate(prim, frame_time);
                    if visible {
                        let (right, up) = make_normal_vectors(prim.normal);
                        out.draws.push(FxDraw::OrientedQuad {
                            shader_time,
                            origin: prim.origin,
                            axis: [prim.normal, right, up],
                            radius,
                            rotation: prim.rotation,
                            rgba,
                            shader,
                        });
                    }
                }
                PrimType::Line => {
                    if visible {
                        out.draws.push(FxDraw::Line { start: prim.origin, end: prim.origin2, width: radius, rgba, shader });
                    }
                }
                PrimType::Electricity => {
                    if visible {
                        for (start, end) in electricity_segments(prim.origin, prim.origin2, prim.elasticity, rng) {
                            out.draws.push(FxDraw::Line { start, end, width: radius, rgba, shader: shader.clone() });
                        }
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

enum PhysicsStep {
    /// No physics applied: move to the predicted origin.
    Free,
    /// Physics already set origin/velocity for this frame.
    Handled,
    /// Impact-kill: free the primitive without running its death effect.
    Die,
}

/// The `fx_physics` block of CParticle::UpdateOrigin (TaystJK/jaPRO), run on
/// the CM world. Ghoul2 entity traces are not available to the FX thread, so
/// `ghoul2Collision` primitives use the plain world trace.
fn physics_step(
    prim: &mut Primitive,
    predicted: [f32; 3],
    real_time: f32,
    mode: u32,
    world: Option<&mut dyn TraceWorld>,
    emits: &mut Vec<(EffectId, [f32; 3], Axis, FxClass)>,
) -> PhysicsStep {
    if mode == 0 || prim.flags & FX_APPLY_PHYSICS == 0 || prim.flags & FX_PLAYER_VIEW != 0 {
        return PhysicsStep::Free;
    }
    if mode <= 1 || (prim.flags & FX_EXPENSIVE_PHYSICS == 0 && mode <= 2) {
        return PhysicsStep::Free;
    }
    let Some(world) = world else {
        return PhysicsStep::Free;
    };
    // CollisionWorld::trace asserts on non-finite input.
    if !prim.origin.iter().chain(&predicted).all(|v| v.is_finite()) {
        return PhysicsStep::Free;
    }
    let (mins, maxs) = if prim.flags & FX_USE_BBOX != 0 { (prim.mins, prim.maxs) } else { ([0.0; 3], [0.0; 3]) };
    let trace = world.trace(TraceQuery {
        start: prim.origin,
        mins,
        maxs,
        end: predicted,
        pass_entity: -1,
        mask: MASK_SOLID,
    });

    if trace.start_solid != 0 || trace.all_solid != 0 {
        prim.vel = [0.0; 3];
        prim.accel = [0.0; 3];
        if prim.flags & FX_GHOUL2_TRACE != 0 && prim.flags & FX_IMPACT_RUNS_FX != 0 && prim.impact_fx != 0 {
            emits.push((prim.impact_fx, trace.end, impact_axis([0.0, 1.0, 0.0]), prim.class));
        }
        prim.flags &= !(FX_APPLY_PHYSICS | FX_IMPACT_RUNS_FX);
        return PhysicsStep::Handled;
    }
    if trace.fraction >= 1.0 {
        return PhysicsStep::Free;
    }

    if prim.flags & FX_IMPACT_RUNS_FX != 0 && trace.surface_flags & SURF_NOIMPACT == 0 && prim.impact_fx != 0 {
        emits.push((prim.impact_fx, trace.end, impact_axis(trace.normal), prim.class));
    }
    if prim.flags & FX_KILL_ON_IMPACT != 0 {
        return PhysicsStep::Die;
    }
    prim.vel = add(prim.vel, scale(prim.accel, real_time * trace.fraction));
    let along = dot(prim.vel, trace.normal);
    prim.vel = add(prim.vel, scale(trace.normal, -2.0 * along));
    prim.vel = scale(prim.vel, prim.elasticity);
    prim.elasticity *= 0.5;
    // Too slow to matter: stop simulating it instead of tracing forever.
    if dot(prim.vel, prim.vel) < 100.0 {
        prim.vel = [0.0; 3];
        prim.accel = [0.0; 3];
        prim.flags &= !(FX_APPLY_PHYSICS | FX_IMPACT_RUNS_FX);
    }
    // Rest one unit off the surface at the exact impact point.
    prim.origin = add(trace.end, trace.normal);
    PhysicsStep::Handled
}

/// PlayEffect(id, origin, normal): the impact normal is the forward axis.
fn impact_axis(normal: [f32; 3]) -> Axis {
    let (right, up) = make_normal_vectors(normal);
    [normal, right, up]
}

/// The shared UpdateSize/RGB/Alpha/Length percentage: 1 at spawn, 0 at end.
pub(super) fn interpolation(flags: u32, parm: f32, start: i32, end: i32, time: i32, rng: &mut Rng) -> f32 {
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

/// rd-vanilla `ApplyShape`: recursively kinks a segment into a lightning-bolt
/// "Z" (q3's `LIGHTNING_RECURSION_LEVEL` is 1, so a single kink per call). Two
/// quasi-random points `sh1`/`sh2` (one biased to each side of the ideal line,
/// matching `CreateShape`) split the segment into three: start->point1,
/// point2->point1, point2->end. RNG only needs to be plausible, not bit-exact.
fn apply_shape(start: [f32; 3], end: [f32; 3], rng: &mut Rng, depth: i32, out: &mut Vec<([f32; 3], [f32; 3])>) {
    if depth < 1 {
        out.push((start, end));
        return;
    }
    let fwd_vec = sub(end, start);
    let dis = dot(fwd_vec, fwd_vec).sqrt() * 0.7;
    let (rt, up) = make_normal_vectors(normalize(fwd_vec));

    let sh1 = [
        0.66 + rng.flrand(-1.0, 1.0) * 0.1,
        0.07 + rng.flrand(-1.0, 1.0) * 0.025,
        0.07 + rng.flrand(-1.0, 1.0) * 0.025,
    ];
    // sh2 is forced onto the opposite side of the line from sh1.
    let sh2 =
        [0.33 + rng.flrand(-1.0, 1.0) * 0.1, -sh1[1] + rng.flrand(-1.0, 1.0) * 0.02, -sh1[2] + rng.flrand(-1.0, 1.0) * 0.02];

    let lerp_offset = |perc: f32, side: [f32; 3]| {
        add(add(scale(start, perc), scale(end, 1.0 - perc)), add(scale(rt, dis * side[1]), scale(up, dis * side[2])))
    };
    let point1 = lerp_offset(sh1[0], sh1);
    let point2 = lerp_offset(sh2[0], sh2);

    apply_shape(start, point1, rng, depth - 1, out);
    apply_shape(point2, point1, rng, depth - 1, out);
    apply_shape(point2, end, rng, depth - 1, out);
}

/// rd-vanilla `DoBoltSeg`: walks `start`->`end` in 20-unit steps, each step
/// drifting further from the ideal line by a `chaos`-scaled random walk (q3's
/// `e->axis[0][0]`, authored on an Electricity primitive as `elasticity`; 0
/// when unauthored, same as the engine's default), then kinks every step with
/// `apply_shape`. Below 20 units the engine's own loop draws nothing, so
/// neither does this. Taper/grow/branch are never authored through the
/// generic `.efx` template (only hardcoded C++ FX_AddElectricity callers use
/// them), so they are not ported here.
fn electricity_segments(start: [f32; 3], end: [f32; 3], chaos: f32, rng: &mut Rng) -> Vec<([f32; 3], [f32; 3])> {
    const STEP: f32 = 20.0;
    const LIGHTNING_RECURSION_LEVEL: i32 = 1;
    let mut out = Vec::new();
    let fwd_vec = sub(end, start);
    let dis = dot(fwd_vec, fwd_vec).sqrt();
    if dis < STEP {
        return out;
    }
    let fwd = normalize(fwd_vec);
    let (rt, up) = make_normal_vectors(fwd);
    let mut old = start;
    let mut off = [10.0_f32; 3];
    let mut i = STEP;
    while i <= dis {
        let perc = if i + STEP > dis { 1.0 } else { i / dis };
        let temp = add(
            scale(fwd, rng.flrand(-1.0, 1.0) * 3.0),
            add(scale(rt, rng.flrand(-1.0, 1.0) * 7.0 * chaos), scale(up, rng.flrand(-1.0, 1.0) * 7.0 * chaos)),
        );
        off = add(off, temp);
        let cur = add(scale(add(start, off), 1.0 - perc), scale(end, perc));
        apply_shape(cur, old, rng, LIGHTNING_RECURSION_LEVEL, &mut out);
        old = cur;
        i += STEP;
    }
    out
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
    fn authored_decals_flush_once_with_shader_and_size() {
        const MARK: &str = "decal\n{\nlife 4000\nsize\n{\nstart 8\n}\nshader\n[\ngfx/damage/bullet_mrk\n]\n}\n";
        let mut fx = FxSystem::new();
        let id = fx.register("test/mark", &mut read_fixture(&[("effects/test/mark.efx", MARK)]));
        assert_ne!(id, 0);
        fx.adjust_time(1000);
        fx.adjust_time(1016);
        fx.play_effect_dir(id, [12.0, 4.0, 8.0], [0.0, 0.0, 1.0]);
        let first = fx.frame();
        assert_eq!(first.decals.len(), 1, "decal is a one-shot projection request");
        assert!(first.draws.is_empty(), "EFX decals must not become billboards");
        let mark = &first.decals[0];
        assert_eq!(mark.origin, [12.0, 4.0, 8.0]);
        assert_eq!(mark.normal, [0.0, 0.0, 1.0]);
        assert_eq!(mark.radius, 8.0);
        assert_eq!(mark.life_ms, 10_000, "author life must not expire a persistent decal");
        assert_eq!(mark.shader, "gfx/damage/bullet_mrk");
        assert_eq!(mark.rgba[3], 255);
        assert!(!mark.ghoul2);
        fx.adjust_time(1032);
        assert!(fx.frame().decals.is_empty(), "do not respawn the decal each frame");
    }

    #[test]
    fn ghoul2decal_flag_is_carried_from_efx_template() {
        const MARK: &str = "decal\n{\nflags ghoul2decals\nsize\n{\nstart 5\n}\nshader\n[\ngfx/damage/bodyburnmark1\n]\n}\n";
        let mut fx = FxSystem::new();
        let id = fx.register("test/g2", &mut read_fixture(&[("effects/test/g2.efx", MARK)]));
        fx.adjust_time(1000);
        fx.adjust_time(1016);
        fx.play_effect_dir(id, [0.0; 3], [0.0, 0.0, 1.0]);
        assert!(fx.frame().decals.iter().any(|decal| decal.ghoul2));
    }

    #[test]
    fn runner_tracking_preserves_simulation_lights_sounds_and_nested_ownership() {
        const ROOT: &str = "fxRunner\n{\nplayFx\n[\nblaster/shot\n]\n}\nlight\n{\nlife 1000\nsize\n{\nstart 64\n}\n}\nsound\n{\nshader\n[\nsound/test.wav\n]\n}\n";
        fn system() -> FxSystem {
            let mut fx = FxSystem::new();
            let id = fx.register("root", &mut read_fixture(&[("effects/root.efx", ROOT), ("effects/blaster/shot.efx", SHOT)]));
            assert_ne!(id, 0);
            fx.adjust_time(1000); fx.adjust_time(1016);
            fx.play_effect_dir_class(id, [-600.0,0.0,0.0], [1.0,0.0,0.0], FxClass::MapRunner(17));
            fx
        }
        let mut baseline = system(); let mut tracked = system();
        for time in [1016,1032,1080] {
            baseline.adjust_time(time); tracked.adjust_time(time);
            let a = baseline.frame_with_visibility(true);
            let b = tracked.frame_with_runner_tracking(true);
            assert_eq!(a.draws, b.draws);
            assert_eq!(a.lights, b.lights);
            assert_eq!(a.sounds, b.sounds);
            assert_eq!(a.shakes, b.shakes);
            assert!(a.runner_draws.is_empty());
            assert_eq!(b.runner_draws.len(), b.draws.len());
            assert!(b.runner_draws.iter().all(|(_, id)| *id == 17), "nested fxRunner must inherit its root entity");
        }
        let off = tracked.frame_with_visibility(true);
        assert!(off.runner_draws.is_empty());
        assert_eq!(baseline.stats().active, tracked.stats().active);
    }

    #[test]
    fn electricity_shorter_than_one_step_draws_nothing() {
        // rd-vanilla's `for (i = 20; i <= dis; i += 20)` never runs below 20 units.
        let mut rng = Rng::new(1);
        let segments = electricity_segments([0.0, 0.0, 0.0], [10.0, 0.0, 0.0], 0.0, &mut rng);
        assert!(segments.is_empty());
    }

    #[test]
    fn electricity_bolt_is_not_a_straight_line() {
        let mut rng = Rng::new(1);
        let start = [0.0, 0.0, 0.0];
        let end = [500.0, 0.0, 0.0];
        // Unauthored elasticity (chaos=0), matching drain_japro.efx: DoBoltSeg's
        // own random walk contributes nothing, but ApplyShape's fractal kink
        // still fires every step, so the bolt must still not reduce to a
        // single straight segment end to end.
        let segments = electricity_segments(start, end, 0.0, &mut rng);
        assert!(segments.len() > 1, "a 500-unit bolt spans multiple 20-unit steps, each kinked into 3 segments");
        let axis = normalize(sub(end, start));
        let off_axis = |point: [f32; 3]| {
            let v = sub(point, start);
            let along = dot(v, axis);
            dot(v, v) - along * along // squared perpendicular distance from the straight line
        };
        assert!(
            segments.iter().any(|&(a, b)| off_axis(a).max(off_axis(b)) > 1.0),
            "ApplyShape's kink must displace at least one endpoint off the straight line"
        );
    }

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
    fn gpu_ownership_sleep_and_cpu_handback_preserve_live_efx() {
        let mut fx = FxSystem::new();
        let id = fx.register("blaster/shot", &mut read_fixture(&[("effects/blaster/shot.efx", SHOT)]));
        fx.adjust_time(1000);
        fx.adjust_time(1016);
        fx.set_gpu_staging(true);
        fx.play_effect_dir(id, [0.0; 3], [1.0, 0.0, 0.0]);
        let birth = fx.frame();
        let spark_id = birth.gpu_sparks.unwrap().spawns[0].id;
        assert_eq!(birth.draws.len(), 2);
        assert_eq!(birth.gpu_sprite_draw_indices.len(), 1);
        fx.set_gpu_owned([spark_id].into_iter().collect());
        fx.adjust_time(1032);
        let owned = fx.frame();
        assert_eq!(owned.draws.len(), 1, "only the Line stays on CPU");
        assert!(owned.gpu_sprite_draw_indices.is_empty(), "owned sprite is not built");
        assert_eq!(fx.stats().active, 2, "GPU ownership must not evict live FX slots");
        let id_state = fx.active.iter().find(|p| p.gpu_id == Some(spark_id)).unwrap();
        assert!(id_state.gpu_sleeping);
        fx.adjust_time(1048);
        let owned_again = fx.frame();
        assert_eq!(owned_again.draws.len(), 1);
        assert!(owned_again.gpu_sparks.unwrap().spawns.is_empty(), "upload at birth only");
        fx.set_gpu_owned(HashSet::new());
        fx.adjust_time(1064);
        let resumed = fx.frame();
        assert_eq!(resumed.draws.len(), 2, "cold PSO must restore CPU spark");
        let FxDraw::Sprite { origin, .. } = &resumed.draws[1] else { panic!("missing CPU sprite") };
        assert!(origin[0] > 0.0, "CPU state must be rehydrated, not frozen at spawn");
        assert!(!fx.active.iter().find(|p| p.gpu_id == Some(spark_id)).unwrap().gpu_sleeping);
        fx.adjust_time(1200);
        assert!(fx.frame().draws.is_empty());
        assert_eq!(fx.stats().active, 0);
    }

    #[test]
    fn ownership_never_overrides_newly_required_collision() {
        let mut fx = FxSystem::new();
        // This authored visual has collision enabled but only traces at mode 3.
        let fixture = "particle\n{\n life 100\n flags usePhysics\n shader\n [\n gfx/effects/whiteGlow\n ]\n }\n";
        let mut read = |_path: &str| Some(fixture.as_bytes().to_vec());
        let id = fx.register("fx/collision", &mut read);
        fx.adjust_time(1000);
        fx.adjust_time(1016);
        fx.set_gpu_staging(true);
        fx.set_physics_mode(2);
        fx.play_effect_dir(id, [0.0; 3], [1.0, 0.0, 0.0]);
        let birth = fx.frame();
        assert_eq!(birth.gpu_sparks.unwrap().spawns.len(), 1);
        let spark_id = fx.active[0].gpu_id.unwrap();
        fx.set_gpu_owned([spark_id].into_iter().collect());
        fx.adjust_time(1032);
        assert!(fx.frame().draws.is_empty(), "mode 2 can offload collisionless subset");
        fx.set_physics_mode(3);
        fx.adjust_time(1048);
        assert_eq!(fx.frame().draws.len(), 1, "mode 3 must resume CPU physics");
        assert!(!fx.active[0].gpu_sleeping);
    }

    #[test]
    fn gpu_staging_emits_once_without_stealing_cpu_draws_and_resets_on_seek() {
        let mut fx = FxSystem::new();
        let id = fx.register("blaster/shot", &mut read_fixture(&[("effects/blaster/shot.efx", SHOT)]));
        fx.adjust_time(1000);
        fx.adjust_time(1016);
        fx.play_effect_dir(id, [0.0; 3], [1.0, 0.0, 0.0]);
        assert!(fx.frame().gpu_sparks.is_none(), "disabled means zero staging cost");
        fx.set_gpu_staging(true);
        fx.play_effect_dir(id, [0.0; 3], [1.0, 0.0, 0.0]);
        let first = fx.frame();
        assert_eq!(first.draws.len(), 4, "staging is invisible to the CPU reference renderer");
        let commands = first.gpu_sparks.expect("enabled staging clock");
        assert!(commands.reset);
        assert_eq!(commands.time_ms, 1016);
        assert_eq!(commands.spawns.len(), 1, "only the collisionless sprite, not the line");
        assert!(commands.spawns[0].values_finite());
        assert_eq!(commands.spawns[0].generation, commands.generation);
        fx.adjust_time(1032);
        let second = fx.frame().gpu_sparks.expect("enabled staging clock");
        assert!(!second.reset);
        assert!(second.spawns.is_empty(), "no duplicate spawn uploads on repeated FX frames");
        fx.adjust_time(500);
        let seek = fx.frame().gpu_sparks.expect("seek must clear resident GPU states");
        assert!(seek.reset);
        assert!(seek.generation != commands.generation);
        assert!(seek.spawns.is_empty());
        fx.adjust_time(500);
        let paused = fx.frame().gpu_sparks.expect("paused FX clock is sent every frame");
        assert_eq!(paused.time_ms, 500);
        assert!(!paused.reset);
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

    /// Infinite solid half-space below z = 0.
    struct Floor;
    impl TraceWorld for Floor {
        fn trace(&mut self, q: TraceQuery) -> jka_movement::TraceResult {
            let mut result = jka_movement::TraceResult::clear(q.end);
            if q.start[2] > 0.0 && q.end[2] <= 0.0 {
                let fraction = q.start[2] / (q.start[2] - q.end[2]);
                result.fraction = fraction;
                result.end = std::array::from_fn(|i| q.start[i] + fraction * (q.end[i] - q.start[i]));
                result.normal = [0.0, 0.0, 1.0];
            }
            result
        }
        fn point_contents(&mut self, _point: [f32; 3], _pass_entity: i32) -> i32 {
            0
        }
    }

    fn bouncer(flags: &str, extra: &str) -> String {
        format!(
            "particle\n{{\n\tflags {flags}\n\tbounce 0.5\n\tlife 5000\n\tspawnFlags absoluteVel\n\torigin 0 0 30\n\tvelocity 0 0 -300\n{extra}\tshader\n\t[\n\t\tgfx/x\n\t]\n}}\n"
        )
    }

    /// Steps a bouncer for 60 frames of 16 ms; returns (min z, saw upward velocity).
    fn run_bouncer(mode: u32, efx: &str, world: bool) -> (f32, bool, FxSystem) {
        let mut fx = FxSystem::new();
        let body: &'static str = Box::leak(efx.to_owned().into_boxed_str());
        let files: &'static [(&'static str, &'static str)] = Box::leak(Box::new([("effects/t/b.efx", body)]));
        let id = fx.register("t/b", &mut read_fixture(files));
        fx.set_physics_mode(mode);
        if world {
            fx.set_collision(Some(Box::new(Floor)));
        }
        fx.adjust_time(1000);
        fx.adjust_time(1016);
        let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        fx.play_effect(id, [0.0; 3], identity);
        let mut min_z = f32::MAX;
        let mut went_up = false;
        for step in 0..60 {
            fx.adjust_time(1032 + step * 16);
            fx.frame();
            if let Some(prim) = fx.active.first() {
                min_z = min_z.min(prim.origin[2]);
                went_up |= prim.vel[2] > 0.0;
            }
        }
        (min_z, went_up, fx)
    }

    #[test]
    fn fx_physics_gates_world_collision_like_taystjk() {
        let authored = bouncer("usePhysics expensivePhysics", "");
        let cheap = bouncer("usePhysics", "");

        let (z, up, _) = run_bouncer(2, &authored, true);
        assert!(up && z >= 0.0, "level 2 bounces an expensivePhysics particle off the floor");

        let (z, up, _) = run_bouncer(2, &cheap, true);
        assert!(!up && z < 0.0, "level 2 ignores usePhysics without expensivePhysics");

        let (z, up, _) = run_bouncer(3, &cheap, true);
        assert!(up && z >= 0.0, "level 3 forces the trace on every physics primitive");

        for mode in [0, 1] {
            let (z, up, _) = run_bouncer(mode, &authored, true);
            assert!(!up && z < 0.0, "levels 0 and 1 never trace (mode {mode})");
        }

        let (z, up, _) = run_bouncer(3, &authored, false);
        assert!(!up && z < 0.0, "no collision world: particles fly freely");
    }

    #[test]
    fn impact_kills_and_impact_fx_follow_the_trace() {
        let (_, _, fx) = run_bouncer(2, &bouncer("usePhysics expensivePhysics impactKills", ""), true);
        assert_eq!(fx.stats().active, 0, "impactKills frees the particle on first contact");

        let files: &'static [(&'static str, &'static str)] = &[
            (
                "effects/t/hit.efx",
                "particle\n{\n\tlife 5000\n\tshader\n\t[\n\t\tgfx/spark\n\t]\n}\n",
            ),
            (
                "effects/t/b.efx",
                "particle\n{\n\tflags usePhysics expensivePhysics impactKills\n\tlife 5000\n\tspawnFlags absoluteVel\n\torigin 0 0 30\n\tvelocity 0 0 -300\n\tshader\n\t[\n\t\tgfx/x\n\t]\n\timpactfx\n\t[\n\t\tt/hit\n\t]\n}\n",
            ),
        ];
        let mut fx = FxSystem::new();
        let id = fx.register("t/b", &mut read_fixture(files));
        fx.set_collision(Some(Box::new(Floor)));
        fx.adjust_time(1000);
        fx.adjust_time(1016);
        let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        fx.play_effect(id, [0.0; 3], identity);
        for step in 0..10 {
            fx.adjust_time(1032 + step * 16);
            fx.frame();
        }
        let hit_sprites: Vec<_> = fx.active.iter().filter(|prim| prim.shader.as_deref() == Some("gfx/spark")).collect();
        assert_eq!(hit_sprites.len(), 1, "one impact effect spawned at the contact");
        assert!(hit_sprites[0].origin[2].abs() < 1.0, "impact effect plays at the trace endpoint");
        assert!(!fx.active.iter().any(|prim| prim.shader.as_deref() == Some("gfx/x")), "impactKills removed the projectile");
    }

    const SMOKE: &str = "Particle
{
	count 20
	cullrange 500
	life 1000
	size
	{
		start 4
	}
	shader
	[
		gfx/smoke
	]
}
Sound
{
	count 1
	cullrange 500
	sounds
	[
		sound/x.wav
	]
}
Particle
{
	count 8
	size
	{
		start 64
	}
	shader
	[
		gfx/big
	]
}
";

    /// Spawn `SMOKE` once from `dist` units away and return (smoke, big, sounds)
    /// counts after the spawn frame.
    fn smoke_at(mode: u32, dist: f32) -> (usize, usize, usize) {
        smoke_at_scale(mode, dist, crate::fx::LOD_SCALE_DEFAULT)
    }

    fn smoke_at_scale(mode: u32, dist: f32, scale: f32) -> (usize, usize, usize) {
        let mut fx = FxSystem::new();
        let id = fx.register("t/smoke", &mut read_fixture(&[("effects/t/smoke.efx", SMOKE)]));
        fx.set_lod(mode, 1.0, scale);
        // 720p, 90 degree vertical fov.
        fx.set_lod_view([0.0; 3], 360.0);
        fx.adjust_time(1000);
        fx.adjust_time(1016);
        fx.play_effect_dir(id, [dist, 0.0, 0.0], [0.0, 0.0, 1.0]);
        let frame = fx.frame();
        let count = |shader: &str| {
            fx.active.iter().filter(|prim| prim.shader.as_deref() == Some(shader)).count()
        };
        (count("gfx/smoke"), count("gfx/big"), frame.sounds.len())
    }

    #[test]
    fn lod_off_is_stock_and_ignores_cullrange() {
        assert_eq!(smoke_at(crate::fx::FX_LOD_OFF, 100.0), (20, 8, 1));
        assert_eq!(smoke_at(crate::fx::FX_LOD_OFF, 5000.0), (20, 8, 1));
    }

    #[test]
    fn authored_lod_culls_visual_primitives_but_not_sounds() {
        // Authored range 500 at scale 1.
        assert_eq!(smoke_at_scale(crate::fx::FX_LOD_AUTHORED, 400.0, 1.0), (20, 8, 1));
        assert_eq!(smoke_at_scale(crate::fx::FX_LOD_AUTHORED, 600.0, 1.0), (0, 8, 1));
        // The default scale of 5 reaches 2500 units.
        assert_eq!(smoke_at(crate::fx::FX_LOD_AUTHORED, 2400.0).0, 20);
        assert_eq!(smoke_at(crate::fx::FX_LOD_AUTHORED, 2600.0).0, 0);
    }

    #[test]
    fn adaptive_lod_thins_small_distant_populations_only() {
        // 4 unit sprites at 400 units: 3.6 px, floored density 0.25.
        let (smoke, big, sounds) = smoke_at(crate::fx::FX_LOD_ADAPTIVE, 400.0);
        assert!(smoke < 20, "distant small particles are thinned ({smoke})");
        assert_eq!((big, sounds), (8, 1), "64 unit sprites at 400 units (57 px) and sounds are untouched");
        assert_eq!(smoke_at(crate::fx::FX_LOD_ADAPTIVE, 20.0).0, 20, "near effects keep authored density");
    }

    #[test]
    fn adaptive_lod_stochastic_rounding_preserves_average_density() {
        let mut fx = FxSystem::new();
        let id = fx.register("t/smoke", &mut read_fixture(&[("effects/t/smoke.efx", SMOKE)]));
        fx.set_lod(crate::fx::FX_LOD_ADAPTIVE, 1.0, crate::fx::LOD_SCALE_DEFAULT);
        fx.set_lod_view([0.0; 3], 360.0);
        fx.adjust_time(1000);
        fx.adjust_time(1016);
        // 4 units * 360 / 300 = 4.8 px -> density ~0.262 of 20 = ~5.23 particles:
        // plain rounding would give exactly 5, stochastic rounding keeps the fraction.
        let spawns = 100;
        for _ in 0..spawns {
            fx.play_effect_dir(id, [300.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        }
        let smoke = fx.active.iter().filter(|prim| prim.shader.as_deref() == Some("gfx/smoke")).count();
        assert!(fx.stats().dropped == 0 && smoke < MAX_ACTIVE);
        let mean = smoke as f32 / spawns as f32;
        assert!(mean > 4.95 && mean < 5.55, "mean {mean} should stay near the 5.23 expected density");
    }

    #[test]
    fn count_scale_follows_stock_wide_range_rule() {
        let files: &'static [(&str, &str)] = &[(
            "effects/t/scale.efx",
            "Particle
{
	count 10 20
	shader
	[
		gfx/wide
	]
}
Particle
{
	count 10
	shader
	[
		gfx/fixed
	]
}
Particle
{
	count 1 4
	shader
	[
		gfx/min
	]
}
",
        )];
        let mut fx = FxSystem::new();
        let id = fx.register("t/scale", &mut read_fixture(files));
        fx.set_lod(crate::fx::FX_LOD_OFF, 0.0, crate::fx::LOD_SCALE_DEFAULT);
        fx.adjust_time(1000);
        fx.adjust_time(1016);
        fx.play_effect_dir(id, [0.0; 3], [0.0, 0.0, 1.0]);
        let count = |shader: &str| fx.active.iter().filter(|prim| prim.shader.as_deref() == Some(shader)).count();
        assert_eq!(count("gfx/wide"), 1, "scaled to zero but keeps the at-least-one guarantee");
        assert_eq!(count("gfx/fixed"), 10, "fixed counts are not scalable");
        assert_eq!(count("gfx/min"), 1, "min >= 1 keeps one spawn after scaling to zero");
    }

    #[test]
    fn lod_density_curve_is_monotonic_and_bounded() {
        use crate::fx::lod_density;
        assert_eq!(lod_density(100.0), 1.0);
        assert_eq!(lod_density(1.0), 0.25);
        let mut last = 0.0;
        for px in (1..=60).map(|px| px as f32) {
            let density = lod_density(px);
            assert!(density >= last && (0.25..=1.0).contains(&density));
            last = density;
        }
        assert!((lod_density(12.0) - 0.56).abs() < 0.03 && (lod_density(6.0) - 0.30).abs() < 0.03);
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
