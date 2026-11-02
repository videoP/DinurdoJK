//! OpenJK FxTemplate.cpp: `.efx` effect and primitive templates.

use super::gp2::{Group, Pair};

// Generic group flags (FxPrimitives.h), shifted per group.
pub const FX_LINEAR: u32 = 0x1;
pub const FX_RAND: u32 = 0x2;
pub const FX_NONLINEAR: u32 = 0x4;
pub const FX_WAVE: u32 = 0x8;
pub const FX_CLAMP: u32 = 0xC;
pub const FX_PARM_MASK: u32 = 0xC;
pub const FX_ALPHA_SHIFT: u32 = 0;
pub const FX_RGB_SHIFT: u32 = 4;
pub const FX_SIZE_SHIFT: u32 = 8;
pub const FX_LENGTH_SHIFT: u32 = 12;
pub const FX_SIZE2_SHIFT: u32 = 16;

// Feature flags.
pub const FX_DEPTH_HACK: u32 = 0x0010_0000;
pub const FX_RELATIVE: u32 = 0x0020_0000;
pub const FX_SET_SHADER_TIME: u32 = 0x0040_0000;
pub const FX_EXPENSIVE_PHYSICS: u32 = 0x0080_0000;
pub const FX_GHOUL2_TRACE: u32 = 0x0002_0000;
pub const FX_GHOUL2_DECALS: u32 = 0x0004_0000;
pub const FX_PAPER_PHYSICS: u32 = 0x0001_0000;
/// Shares FX_PAPER_PHYSICS' bit (FX_PLAYER_VIEW); CParticle::UpdateOrigin
/// never applies physics to it.
pub const FX_PLAYER_VIEW: u32 = 0x0001_0000;
pub const FX_ATTACHED_MODEL: u32 = 0x0100_0000;
pub const FX_APPLY_PHYSICS: u32 = 0x0200_0000;
pub const FX_USE_BBOX: u32 = 0x0400_0000;
pub const FX_USE_ALPHA: u32 = 0x0800_0000;
pub const FX_EMIT_FX: u32 = 0x1000_0000;
pub const FX_DEATH_RUNS_FX: u32 = 0x2000_0000;
pub const FX_KILL_ON_IMPACT: u32 = 0x4000_0000;
pub const FX_IMPACT_RUNS_FX: u32 = 0x8000_0000;

// Spawn flags (FxScheduler.h).
pub const FX_ORG_ON_SPHERE: u32 = 0x00001;
pub const FX_AXIS_FROM_SPHERE: u32 = 0x00002;
pub const FX_ORG_ON_CYLINDER: u32 = 0x00004;
pub const FX_ORG2_FROM_TRACE: u32 = 0x00010;
pub const FX_TRACE_IMPACT_FX: u32 = 0x00020;
pub const FX_ORG2_IS_OFFSET: u32 = 0x00040;
pub const FX_CHEAP_ORG_CALC: u32 = 0x00100;
pub const FX_CHEAP_ORG2_CALC: u32 = 0x00200;
pub const FX_VEL_IS_ABSOLUTE: u32 = 0x00400;
pub const FX_ACCEL_IS_ABSOLUTE: u32 = 0x00800;
pub const FX_RAND_ROT_AROUND_FWD: u32 = 0x01000;
pub const FX_EVEN_DISTRIBUTION: u32 = 0x02000;
pub const FX_RGB_COMPONENT_INTERP: u32 = 0x04000;
pub const FX_AFFECTED_BY_WIND: u32 = 0x10000;

/// CFxRange.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Range {
    pub min: f32,
    pub max: f32,
}

impl Range {
    pub const fn fixed(value: f32) -> Self {
        Self { min: value, max: value }
    }
    /// GetVal(): uniform in [min, max].
    pub fn get(&self, rng: &mut Rng) -> f32 {
        if self.min != self.max { rng.flrand(self.min, self.max) } else { self.min }
    }
    /// GetVal(fraction).
    pub fn at(&self, fraction: f32) -> f32 {
        if self.min != self.max { self.min + fraction * (self.max - self.min) } else { self.min }
    }
}

/// Q_flrand/Q_irand stand-in. FX randomness only has to be plausible, not
/// bit-identical to OpenJK's generator.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }
    fn next_u32(&mut self) -> u32 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }
    pub fn flrand(&mut self, min: f32, max: f32) -> f32 {
        min + (self.next_u32() as f32 / u32::MAX as f32) * (max - min)
    }
    pub fn irand(&mut self, min: i32, max: i32) -> i32 {
        if max <= min {
            return min;
        }
        min + (self.next_u32() % (max - min + 1) as u32) as i32
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PrimType {
    Particle,
    Line,
    Tail,
    Cylinder,
    Emitter,
    Sound,
    Decal,
    OrientedParticle,
    Electricity,
    FxRunner,
    Light,
    CameraShake,
    ScreenFlash,
}

impl PrimType {
    /// primitiveTypes[] in FxScheduler.cpp.
    fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "particle" => Self::Particle,
            "line" => Self::Line,
            "tail" => Self::Tail,
            "sound" => Self::Sound,
            "cylinder" => Self::Cylinder,
            "electricity" => Self::Electricity,
            "emitter" => Self::Emitter,
            "decal" => Self::Decal,
            "orientedparticle" => Self::OrientedParticle,
            "fxrunner" => Self::FxRunner,
            "light" => Self::Light,
            "camerashake" => Self::CameraShake,
            "flash" => Self::ScreenFlash,
            _ => return None,
        })
    }
}

/// CPrimitiveTemplate with OpenJK's constructor defaults.
#[derive(Clone, Debug)]
pub struct PrimitiveTemplate {
    pub kind: PrimType,
    pub name: String,
    pub spawn_delay: Range,
    pub spawn_count: Range,
    pub life: Range,
    /// Shader, sound or model names (CMediaHandles); one is picked per spawn.
    pub media: Vec<String>,
    pub impact_fx: Vec<String>,
    pub death_fx: Vec<String>,
    pub emitter_fx: Vec<String>,
    pub play_fx: Vec<String>,
    pub flags: u32,
    pub spawn_flags: u32,
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub origin1: [Range; 3],
    pub origin2: [Range; 3],
    pub radius: Range,
    pub height: Range,
    pub wind_modifier: Range,
    pub rotation: Range,
    pub rotation_delta: Range,
    pub angle: [Range; 3],
    pub angle_delta: [Range; 3],
    pub velocity: [Range; 3],
    pub acceleration: [Range; 3],
    pub gravity: Range,
    pub density: Range,
    pub variance: Range,
    pub rgb_start: [Range; 3],
    pub rgb_end: [Range; 3],
    pub rgb_parm: Range,
    pub alpha_start: Range,
    pub alpha_end: Range,
    pub alpha_parm: Range,
    pub size_start: Range,
    pub size_end: Range,
    pub size_parm: Range,
    pub size2_start: Range,
    pub size2_end: Range,
    pub size2_parm: Range,
    pub length_start: Range,
    pub length_end: Range,
    pub length_parm: Range,
    pub elasticity: Range,
    /// Authored `cullRange` squared (like OpenJK's parse would have); 0 = none.
    pub cull_range_sq: f32,
}

impl PrimitiveTemplate {
    /// Largest authored on-screen extent of a single spawned element, in world
    /// units: sprite size for particles, width/length for tails.
    pub fn lod_size(&self) -> f32 {
        let size = self.size_start.min.abs().max(self.size_start.max.abs()).max(self.size_end.min.abs()).max(self.size_end.max.abs());
        if self.kind == PrimType::Tail {
            let length = self
                .length_start
                .min
                .abs()
                .max(self.length_start.max.abs())
                .max(self.length_end.min.abs())
                .max(self.length_end.max.abs());
            size.max(length)
        } else {
            size
        }
    }

    fn new(kind: PrimType) -> Self {
        let one = Range::fixed(1.0);
        let zero = Range::default();
        Self {
            kind,
            name: String::new(),
            spawn_delay: zero,
            spawn_count: one,
            life: Range::fixed(50.0),
            media: Vec::new(),
            impact_fx: Vec::new(),
            death_fx: Vec::new(),
            emitter_fx: Vec::new(),
            play_fx: Vec::new(),
            flags: 0,
            spawn_flags: 0,
            min: [0.0; 3],
            max: [0.0; 3],
            origin1: [zero; 3],
            origin2: [zero; 3],
            radius: Range::fixed(10.0),
            height: Range::fixed(10.0),
            wind_modifier: one,
            rotation: zero,
            rotation_delta: zero,
            angle: [zero; 3],
            angle_delta: [zero; 3],
            velocity: [zero; 3],
            acceleration: [zero; 3],
            gravity: zero,
            density: Range::fixed(10.0),
            variance: one,
            rgb_start: [one; 3],
            rgb_end: [one; 3],
            rgb_parm: zero,
            alpha_start: one,
            alpha_end: one,
            alpha_parm: zero,
            size_start: one,
            size_end: one,
            size_parm: zero,
            size2_start: one,
            size2_end: one,
            size2_parm: zero,
            length_start: one,
            length_end: one,
            length_parm: zero,
            elasticity: Range::fixed(0.1),
            cull_range_sq: 0.0,
        }
    }

    /// CPrimitiveTemplate::ParsePrimitive.
    fn parse(kind: PrimType, group: &Group) -> Self {
        let mut prim = Self::new(kind);
        for pair in &group.pairs {
            let value = pair.top();
            match pair.name.to_ascii_lowercase().as_str() {
                "count" => set_range(&mut prim.spawn_count, value),
                "shaders" | "shader" | "models" | "model" | "sounds" | "sound" => prim.media.extend(list(pair)),
                "impactfx" => {
                    let names: Vec<_> = list(pair).collect();
                    // OpenJK ParseImpactFxStrings: a non-empty list is itself a
                    // request for impact effects, which need physics.
                    if !names.is_empty() {
                        prim.flags |= FX_IMPACT_RUNS_FX | FX_APPLY_PHYSICS;
                    }
                    prim.impact_fx.extend(names);
                }
                "deathfx" => {
                    let names: Vec<_> = list(pair).collect();
                    // OpenJK ParseDeathFxStrings.
                    if !names.is_empty() {
                        prim.flags |= FX_DEATH_RUNS_FX;
                    }
                    prim.death_fx.extend(names);
                }
                "emitfx" => {
                    let names: Vec<_> = list(pair).collect();
                    // OpenJK ParseEmitterFxStrings.
                    if !names.is_empty() {
                        prim.flags |= FX_EMIT_FX;
                    }
                    prim.emitter_fx.extend(names);
                }
                "playfx" => prim.play_fx.extend(list(pair)),
                "life" => set_range(&mut prim.life, value),
                "delay" => set_range(&mut prim.spawn_delay, value),
                "cullrange" => {
                    // atoi, then squared so the spawn check avoids a sqrt.
                    let range = value.trim().parse::<f32>().map(f32::trunc).unwrap_or(0.0);
                    prim.cull_range_sq = range * range;
                }
                "bounce" | "intensity" => {
                    set_range(&mut prim.elasticity, value);
                    // OpenJK ParseElasticity: authoring bounce/intensity is an
                    // implicit request for primitive physics.
                    prim.flags |= FX_APPLY_PHYSICS;
                }
                "min" => {
                    if let Some((min, _)) = parse_vector(value) {
                        prim.min = min;
                        // OpenJK ParseMin: a bounding box implies physics.
                        prim.flags |= FX_USE_BBOX | FX_APPLY_PHYSICS;
                    }
                }
                "max" => {
                    if let Some((max, _)) = parse_vector(value) {
                        prim.max = max;
                        // OpenJK ParseMax: a bounding box implies physics.
                        prim.flags |= FX_USE_BBOX | FX_APPLY_PHYSICS;
                    }
                }
                "angle" | "angles" => set_vector(&mut prim.angle, value),
                "angledelta" => set_vector(&mut prim.angle_delta, value),
                "velocity" | "vel" => set_vector(&mut prim.velocity, value),
                "acceleration" | "accel" => set_vector(&mut prim.acceleration, value),
                "gravity" => set_range(&mut prim.gravity, value),
                "density" => set_range(&mut prim.density, value),
                "variance" => set_range(&mut prim.variance, value),
                "origin" => set_vector(&mut prim.origin1, value),
                "origin2" => set_vector(&mut prim.origin2, value),
                "radius" => set_range(&mut prim.radius, value),
                "height" => set_range(&mut prim.height, value),
                "wind" => set_range(&mut prim.wind_modifier, value),
                "rotation" => set_range(&mut prim.rotation, value),
                "rotationdelta" => set_range(&mut prim.rotation_delta, value),
                "flags" | "flag" => prim.flags |= feature_flags(value),
                "spawnflags" | "spawnflag" => prim.spawn_flags |= spawn_flags(value),
                "name" => prim.name = value.to_owned(),
                _ => {}
            }
        }
        for sub in &group.groups {
            let (start, end, parm, shift): (&mut [Range], &mut [Range], &mut Range, u32) =
                match sub.name.to_ascii_lowercase().as_str() {
                    "rgb" => (&mut prim.rgb_start, &mut prim.rgb_end, &mut prim.rgb_parm, FX_RGB_SHIFT),
                    "alpha" => (
                        std::slice::from_mut(&mut prim.alpha_start),
                        std::slice::from_mut(&mut prim.alpha_end),
                        &mut prim.alpha_parm,
                        FX_ALPHA_SHIFT,
                    ),
                    "size" | "width" => (
                        std::slice::from_mut(&mut prim.size_start),
                        std::slice::from_mut(&mut prim.size_end),
                        &mut prim.size_parm,
                        FX_SIZE_SHIFT,
                    ),
                    "size2" | "width2" => (
                        std::slice::from_mut(&mut prim.size2_start),
                        std::slice::from_mut(&mut prim.size2_end),
                        &mut prim.size2_parm,
                        FX_SIZE2_SHIFT,
                    ),
                    "length" | "height" => (
                        std::slice::from_mut(&mut prim.length_start),
                        std::slice::from_mut(&mut prim.length_end),
                        &mut prim.length_parm,
                        FX_LENGTH_SHIFT,
                    ),
                    _ => continue,
                };
            let mut group_flags = 0;
            for pair in &sub.pairs {
                let value = pair.top();
                match pair.name.to_ascii_lowercase().as_str() {
                    "start" => set_group_range(start, value),
                    "end" => set_group_range(end, value),
                    "parm" | "parms" => set_range(parm, value),
                    // ParseRGBFlags & co. apply nothing if any word is unknown.
                    "flags" | "flag" => group_flags |= generic_flags(value).unwrap_or(0),
                    _ => {}
                }
            }
            prim.flags |= group_flags << shift;
        }
        prim
    }
}

/// SEffectTemplate.
#[derive(Clone, Debug)]
pub struct EffectTemplate {
    #[cfg_attr(not(test), allow(dead_code))]
    pub name: String,
    /// Looping effects (PlayEffect with iLoopTime) are not scheduled yet.
    #[allow(dead_code)]
    pub repeat_delay: i32,
    pub primitives: Vec<PrimitiveTemplate>,
    /// DinurdoJK inference profile for legacy EFX that have no authored Light{}
    /// primitive. This is metadata only; the renderer still decides whether the
    /// chosen shader/material is actually emissive enough to promote.
    pub auto_light: Option<AutoLightProfile>,
}

/// Load-time heuristic result for an effect that plausibly emits light despite
/// not containing an authored `Light {}` primitive.  We intentionally select a
/// single visual primitive as the driver so particle counts never turn into
/// one dynamic light per sprite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoLightProfile {
    pub driver_primitive: usize,
    /// Semantic confidence before material/shader inspection.
    pub score: i16,
    /// Directional emission inferred from the authored primitive.  Line and
    /// electricity primitives deliberately remain segment emitters; this is
    /// only for point-like visuals whose authored motion forms a coherent cone.
    pub spot: Option<AutoLightSpotProfile>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoLightSpotProfile {
    /// Mean authored emission direction in the effect's local axis basis.
    pub local_direction: [f32; 3],
    /// Cosines are stored so the runtime/shader path never needs trig.
    pub inner_cone_cos: f32,
    pub outer_cone_cos: f32,
}

/// FX_MAX_EFFECT_COMPONENTS.
const MAX_EFFECT_COMPONENTS: usize = 24;

/// CFxScheduler::ParseEffect. Tooling/map-load callers use this form so the
/// promotion heuristic is evaluated while assets are already being prepared.
pub fn parse_effect(name: &str, root: &Group) -> EffectTemplate {
    parse_effect_impl(name, root, true)
}

/// Runtime FX registration keeps OpenJK's normal lazy EFX parse but deliberately
/// skips promotion inference: map loading already built that metadata on a
/// worker, and `FxSystem` installs the catalog result after parsing.
pub fn parse_effect_without_auto_light(name: &str, root: &Group) -> EffectTemplate {
    parse_effect_impl(name, root, false)
}

fn parse_effect_impl(name: &str, root: &Group, infer_auto_light: bool) -> EffectTemplate {
    // Only the first root pair is examined, exactly as OpenJK does.
    let repeat_delay = root
        .pairs
        .first()
        .filter(|pair| pair.name.eq_ignore_ascii_case("repeatDelay"))
        .and_then(|pair| pair.top().trim().parse::<i32>().ok())
        .unwrap_or(0);
    let primitives: Vec<_> = root
        .groups
        .iter()
        .filter_map(|group| PrimType::from_name(&group.name).map(|kind| PrimitiveTemplate::parse(kind, group)))
        .take(MAX_EFFECT_COMPONENTS)
        .collect();
    let auto_light = infer_auto_light
        .then(|| infer_auto_light_profile(name, &primitives))
        .flatten();
    EffectTemplate { name: name.to_owned(), repeat_delay, primitives, auto_light }
}

const AUTO_LIGHT_SEMANTIC_THRESHOLD: i16 = 0;

fn infer_auto_light_profile(name: &str, primitives: &[PrimitiveTemplate]) -> Option<AutoLightProfile> {
    // Authored JKA/Raven light stages always win. Never augment an effect that
    // already made an explicit lighting decision.
    if primitives.iter().any(|primitive| primitive.kind == PrimType::Light) {
        return None;
    }

    let effect_score = token_score(name, true);
    let flash_bonus = if primitives.iter().any(|primitive| primitive.kind == PrimType::ScreenFlash) { 2 } else { 0 };
    let mut best: Option<AutoLightProfile> = None;

    for (index, primitive) in primitives.iter().enumerate() {
        if !matches!(
            primitive.kind,
            PrimType::Particle
                | PrimType::OrientedParticle
                | PrimType::Line
                | PrimType::Tail
                | PrimType::Cylinder
                | PrimType::Electricity
        ) {
            continue;
        }

        let mut score = effect_score + flash_bonus;
        if primitive.kind == PrimType::Electricity {
            score += 6;
        }
        for medium in &primitive.media {
            score += token_score(medium, false);
        }

        // Authored component colour is only supporting evidence. White is very
        // common in FX because the texture itself carries the hue.
        let rgb = primitive.rgb_start.map(|range| 0.5 * (range.min + range.max));
        let max = rgb[0].max(rgb[1]).max(rgb[2]);
        let min = rgb[0].min(rgb[1]).min(rgb[2]);
        if max >= 0.70 {
            score += 1;
        }
        if max >= 0.45 && max - min >= 0.25 {
            score += 1;
        }

        let spot = infer_auto_light_spot(name, primitive);
        if spot.is_some() {
            score += 2;
        }
        let candidate = AutoLightProfile {
            driver_primitive: index,
            score,
            spot,
        };
        if best.map_or(true, |current| candidate.score > current.score) {
            best = Some(candidate);
        }
    }

    best.filter(|profile| profile.score >= AUTO_LIGHT_SEMANTIC_THRESHOLD)
}

fn infer_auto_light_spot(effect_name: &str, primitive: &PrimitiveTemplate) -> Option<AutoLightSpotProfile> {
    // A Line/Electricity/Tail source is luminous along its length, not a
    // projector. Keep those on the segment/capsule path even if the name says
    // "beam" or "jet".
    if matches!(primitive.kind, PrimType::Line | PrimType::Electricity | PrimType::Tail) {
        return None;
    }

    // Some Raven effects author the projector itself as a tapered Cylinder.
    // Example: rockettrooper/light_cone.efx has a tiny size2 ring at the source,
    // a much wider size ring at the far end, and an explicit length. That is a
    // stronger spot-light signal than any string heuristic.
    if primitive.kind == PrimType::Cylinder {
        let max_abs = |range: Range| range.min.abs().max(range.max.abs());
        let near_radius = max_abs(primitive.size2_start).max(max_abs(primitive.size2_end));
        let far_radius = max_abs(primitive.size_start).max(max_abs(primitive.size_end));
        let length_value = if max_abs(primitive.length_start) >= max_abs(primitive.length_end) {
            0.5 * (primitive.length_start.min + primitive.length_start.max)
        } else {
            0.5 * (primitive.length_end.min + primitive.length_end.max)
        };
        let length = length_value.abs();
        if length >= 8.0 && far_radius >= near_radius.max(0.5) * 1.35 {
            let mut outer = far_radius.atan2(length);
            outer = outer.clamp(6.0_f32.to_radians(), 60.0_f32.to_radians());
            let inner = (outer * 0.72).clamp(3.0_f32.to_radians(), outer * 0.92);
            return Some(AutoLightSpotProfile {
                local_direction: [length_value.signum().max(-1.0), 0.0, 0.0],
                inner_cone_cos: inner.cos(),
                outer_cone_cos: outer.cos(),
            });
        }
        return None;
    }

    if !matches!(primitive.kind, PrimType::Particle | PrimType::OrientedParticle) {
        return None;
    }

    let mean = primitive.velocity.map(|range| 0.5 * (range.min + range.max));
    let mean_len_sq = mean.iter().map(|v| v * v).sum::<f32>();
    let mean_len = mean_len_sq.sqrt();
    if !mean_len.is_finite() || mean_len < 8.0 {
        return None;
    }
    let local_direction = mean.map(|value| value / mean_len);

    // Velocity ranges are independent uniform distributions. Only variance
    // perpendicular to the mean direction widens the light cone; speed spread
    // along the axis does not.
    let mut perpendicular_variance = 0.0_f32;
    let mut total_variance = 0.0_f32;
    for axis in 0..3 {
        let span = (primitive.velocity[axis].max - primitive.velocity[axis].min).abs();
        let variance = span * span / 12.0;
        total_variance += variance;
        perpendicular_variance += variance * (1.0 - local_direction[axis] * local_direction[axis]);
    }
    let rms_perpendicular = perpendicular_variance.max(0.0).sqrt();
    let coherence = mean_len / (mean_len_sq + total_variance).sqrt().max(1.0e-4);

    // Strong authored motion is the primary signal. Directional naming can
    // relax the threshold slightly, but never creates a cone on its own.
    let lower = effect_name.to_ascii_lowercase();
    let directional_name = [
        "jet", "flamethrow", "fthrw", "thrower", "muzzle", "exhaust", "thruster",
        "searchlight", "spotlight", "light_cone",
    ]
    .iter()
    .any(|token| lower.contains(token));

    // Derive angular spread from authored velocity dispersion and from how wide
    // the visual becomes over its lifetime. The latter matters for flame jets:
    // their velocity cone is often narrow while the sprite billboards grow.
    let velocity_half_angle = rms_perpendicular.atan2(mean_len);
    let life_seconds = (0.5 * (primitive.life.min + primitive.life.max)).abs() * 0.001;
    let visual_radius = primitive
        .size_start
        .min
        .abs()
        .max(primitive.size_start.max.abs())
        .max(primitive.size_end.min.abs())
        .max(primitive.size_end.max.abs());
    let travel = (mean_len * life_seconds).max(1.0);
    let jet_aspect = travel / visual_radius.max(1.0);
    let required_coherence = if directional_name { 0.72 } else { 0.92 };
    let required_aspect = if directional_name { 1.25 } else { 4.0 };
    if coherence < required_coherence || jet_aspect < required_aspect {
        return None;
    }

    let visual_half_angle = visual_radius.atan2(travel);
    let mut outer = velocity_half_angle.max(visual_half_angle) * 1.35;
    outer = outer.clamp(10.0_f32.to_radians(), 55.0_f32.to_radians());
    let inner = (outer * 0.62).clamp(5.0_f32.to_radians(), outer * 0.90);

    Some(AutoLightSpotProfile {
        local_direction,
        inner_cone_cos: inner.cos(),
        outer_cone_cos: outer.cos(),
    })
}

pub(super) fn token_score(value: &str, effect_name: bool) -> i16 {
    let value = value.to_ascii_lowercase();
    let contains_any = |tokens: &[&str]| tokens.iter().any(|token| value.contains(token));

    // Negative classes are deliberately strong. A smoke sprite may share an
    // explosion EFX with a flash, but it must never become the selected driver.
    if contains_any(&[
        "smoke", "steam", "dust", "dirt", "blood", "gore", "debris", "chunk",
        "water", "splash", "bubble", "mist", "fog", "snow", "rain", "decal", "mark",
    ]) {
        return -7;
    }

    let mut score = 0;
    if contains_any(&[
        "fire", "flame", "glow", "flare", "lightning", "electric", "plasma", "energy",
        "laser", "spark", "hot", "core",
    ]) {
        score += if effect_name { 2 } else { 4 };
    }
    if contains_any(&[
        "explosion", "explode", "muzzle", "shot", "bolt", "beam", "jet", "flash", "burn",
        "exhaust", "thruster", "light_cone", "spotlight",
    ]) {
        score += if effect_name { 2 } else { 3 };
    }
    score
}

fn list(pair: &Pair) -> impl Iterator<Item = String> + '_ {
    pair.values.iter().filter(|value| !value.is_empty()).cloned()
}

/// sscanf("%f %f"): leading floats only; one value means min == max.
fn floats(value: &str, limit: usize) -> Vec<f32> {
    value
        .split_whitespace()
        .map_while(|token| token.parse::<f32>().ok())
        .take(limit)
        .collect()
}

/// ParseFloat.
fn set_range(range: &mut Range, value: &str) {
    match floats(value, 2).as_slice() {
        [min] => *range = Range::fixed(*min),
        [min, max] => *range = Range { min: *min, max: *max },
        _ => {}
    }
}

/// ParseVector: 3 values (min = max) or 6 values; anything else is rejected.
fn parse_vector(value: &str) -> Option<([f32; 3], [f32; 3])> {
    match floats(value, 6).as_slice() {
        [x, y, z] => Some(([*x, *y, *z], [*x, *y, *z])),
        [x, y, z, a, b, c] => Some(([*x, *y, *z], [*a, *b, *c])),
        _ => None,
    }
}

fn set_vector(ranges: &mut [Range; 3], value: &str) {
    if let Some((min, max)) = parse_vector(value) {
        for axis in 0..3 {
            ranges[axis] = Range { min: min[axis], max: max[axis] };
        }
    }
}

/// Group start/end: RGB uses ParseVector, scalar groups ParseFloat.
fn set_group_range(ranges: &mut [Range], value: &str) {
    if ranges.len() == 3 {
        if let Some((min, max)) = parse_vector(value) {
            for axis in 0..3 {
                ranges[axis] = Range { min: min[axis], max: max[axis] };
            }
        }
    } else {
        set_range(&mut ranges[0], value);
    }
}

/// ParseGroupFlags (first four words); `None` when any word is unknown.
fn generic_flags(value: &str) -> Option<u32> {
    value
        .split_whitespace()
        .take(4)
        .map(|word| match word.to_ascii_lowercase().as_str() {
            "linear" => Some(FX_LINEAR),
            "nonlinear" => Some(FX_NONLINEAR),
            "wave" => Some(FX_WAVE),
            "random" => Some(FX_RAND),
            "clamp" => Some(FX_CLAMP),
            _ => None,
        })
        .try_fold(0, |flags, flag| Some(flags | flag?))
}

/// ParseFlags (first seven words).
fn feature_flags(value: &str) -> u32 {
    value
        .split_whitespace()
        .take(7)
        .map(|word| match word.to_ascii_lowercase().as_str() {
            "usemodel" => FX_ATTACHED_MODEL,
            "usebbox" => FX_USE_BBOX,
            "usephysics" => FX_APPLY_PHYSICS,
            "expensivephysics" => FX_EXPENSIVE_PHYSICS,
            "ghoul2collision" => FX_GHOUL2_TRACE | FX_APPLY_PHYSICS | FX_EXPENSIVE_PHYSICS,
            "ghoul2decals" => FX_GHOUL2_DECALS,
            "impactkills" => FX_KILL_ON_IMPACT,
            "impactfx" => FX_IMPACT_RUNS_FX,
            "deathfx" => FX_DEATH_RUNS_FX,
            "usealpha" => FX_USE_ALPHA,
            "emitfx" => FX_EMIT_FX,
            "depthhack" => FX_DEPTH_HACK,
            "relative" => FX_RELATIVE,
            "setshadertime" => FX_SET_SHADER_TIME,
            "paperphysics" | "localizedflash" | "playerview" => FX_PAPER_PHYSICS,
            _ => 0,
        })
        .fold(0, |flags, flag| flags | flag)
}

/// ParseSpawnFlags (first seven words).
fn spawn_flags(value: &str) -> u32 {
    value
        .split_whitespace()
        .take(7)
        .map(|word| match word.to_ascii_lowercase().as_str() {
            "org2fromtrace" => FX_ORG2_FROM_TRACE,
            "traceimpactfx" => FX_TRACE_IMPACT_FX,
            "org2isoffset" => FX_ORG2_IS_OFFSET,
            "cheaporgcalc" => FX_CHEAP_ORG_CALC,
            "cheaporg2calc" => FX_CHEAP_ORG2_CALC,
            "absolutevel" => FX_VEL_IS_ABSOLUTE,
            "absoluteaccel" => FX_ACCEL_IS_ABSOLUTE,
            "orgonsphere" => FX_ORG_ON_SPHERE,
            "orgoncylinder" => FX_ORG_ON_CYLINDER,
            "axisfromsphere" => FX_AXIS_FROM_SPHERE,
            "randrotaroundfwd" => FX_RAND_ROT_AROUND_FWD,
            "evendistribution" => FX_EVEN_DISTRIBUTION,
            "rgbcomponentinterpolation" => FX_RGB_COMPONENT_INTERP,
            "affectedbywind" => FX_AFFECTED_BY_WIND,
            _ => 0,
        })
        .fold(0, |flags, flag| flags | flag)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::gp2;

    const BLASTER_SHOT: &[u8] = b"// Simple blaster effect\n\nLine\n{\n\torigin\t8 0 0\n\torigin2\t-80 0 0\n\tflags\tuseAlpha\n\n\twidth\n\t{\n\t\tstart \t1.4 \t1.6\n\t}\n\n\talpha\n\t{\n\t\tstart\t0.6\t0.7\n\t}\n\n\tshader\n\t[\n\t\tgfx/effects/blaster_blob\n\t]\n}\n\nLine\n{\n\torigin\t16 0 0\n\torigin2\t-90 0 0\n\n\twidth\n\t{\n\t\tstart \t4.0 \t5.0\n\t}\n\n\trgb\n\t{\n\t\tstart\t0.7 0.0 0.0 \t0.8 0.2 0.2\n\t\tflags linear nonsense\n\t}\n\n\tshader\n\t[\n\t\tgfx/effects/whiteGlow\n\t]\n}\n\nparticle\n{\n\torigin\t-5 0 0\n\tspawnFlags orgOnSphere evenDistribution\n\tsize\n\t{\n\t\tstart \t1.6\t1.8\n\t}\n\tshader\n\t[\n\t\tgfx/effects/whiteGlow\n\t]\n}\n";

    #[test]
    fn stock_blaster_shot_parses_with_openjk_defaults() {
        let effect = parse_effect("blaster/shot", &gp2::parse(BLASTER_SHOT));
        assert_eq!(effect.primitives.len(), 3);
        let core = &effect.primitives[0];
        assert_eq!(core.kind, PrimType::Line);
        assert_eq!(core.origin1[0], Range::fixed(8.0));
        assert_eq!(core.origin2[0], Range::fixed(-80.0));
        assert_eq!(core.flags, FX_USE_ALPHA);
        assert_eq!(core.size_start, Range { min: 1.4, max: 1.6 });
        assert_eq!(core.alpha_start, Range { min: 0.6, max: 0.7 });
        assert_eq!(core.alpha_end, Range::fixed(1.0), "default end");
        assert_eq!(core.life, Range::fixed(50.0), "default life");
        assert_eq!(core.media, ["gfx/effects/blaster_blob"]);
        let glow = &effect.primitives[1];
        assert_eq!(glow.rgb_start[1], Range { min: 0.0, max: 0.2 });
        assert_eq!(glow.flags, 0, "an unknown group flag word voids the whole flags line");
        let particle = &effect.primitives[2];
        assert_eq!(particle.kind, PrimType::Particle);
        assert_eq!(particle.spawn_flags, FX_ORG_ON_SPHERE | FX_EVEN_DISTRIBUTION);
        let auto = effect.auto_light.expect("glowing blaster visuals should be a promotion candidate");
        assert_eq!(auto.driver_primitive, 1, "whiteGlow line should beat the weaker blob line");
    }

    #[test]
    fn authored_light_disables_inference() {
        let bytes = b"particle\n{\n shader\n [\n  gfx/effects/fire\n ]\n}\nLight\n{\n size\n {\n  start 160\n }\n rgb\n {\n  start 1 .4 .1\n }\n}\n";
        let effect = parse_effect("env/fire", &gp2::parse(bytes));
        assert!(effect.auto_light.is_none());
    }

    #[test]
    fn smoke_is_not_selected_over_fire() {
        let bytes = b"particle\n{\n shader\n [\n  gfx/effects/smoke\n ]\n}\nparticle\n{\n shader\n [\n  gfx/effects/fire2\n ]\n}\n";
        let effect = parse_effect("env/flamejet", &gp2::parse(bytes));
        let auto = effect.auto_light.expect("fire primitive should be inferred");
        assert_eq!(auto.driver_primitive, 1);
    }

    #[test]
    fn coherent_flame_velocity_infers_spot_cone_but_lines_remain_segments() {
        let bytes = b"Particle
{
 life 400
 velocity 160 -10 -10 280 10 10
 size
 {
  start 3 4
  end 12 30
 }
 shader
 [
  gfx/effects/fire2
 ]
}
";
        let effect = parse_effect("bespin/flamejet", &gp2::parse(bytes));
        let auto = effect.auto_light.expect("flame jet should be promotable");
        let spot = auto.spot.expect("coherent authored velocity should infer a spot light");
        assert!(spot.local_direction[0] > 0.99);
        assert!(spot.inner_cone_cos > spot.outer_cone_cos);
        assert!(spot.outer_cone_cos > 55.0_f32.to_radians().cos());

        let line = parse_effect(
            "weapon/beamjet",
            &gp2::parse(b"Line
{
 origin 0 0 0
 origin2 128 0 0
 shader
 [
  gfx/effects/whiteGlow
 ]
}
"),
        );
        assert!(line.auto_light.expect("glowing line should promote").spot.is_none());

        let cone = parse_effect(
            "rockettrooper/light_cone",
            &gp2::parse(b"Cylinder
{
 size
 {
  start 30
 }
 size2
 {
  start 2
 }
 length
 {
  start 150
 }
 shaders
 [
  gfx/effects/light_cone
 ]
}
"),
        );
        let cone_spot = cone
            .auto_light
            .expect("tapered emissive cylinder should promote")
            .spot
            .expect("tapered cylinder should infer a spot light");
        assert!(cone_spot.local_direction[0] > 0.99);
        assert!(cone_spot.inner_cone_cos > cone_spot.outer_cone_cos);
        let expected_outer = (30.0_f32 / 150.0).atan().cos();
        assert!((cone_spot.outer_cone_cos - expected_outer).abs() < 0.02);
    }

    #[test]
    fn bounds_and_elasticity_imply_openjk_physics_flags() {
        let bytes = b"particle\n{\n min -1 -2 -3\n max 1 2 3\n bounce 0.4\n}\n";
        let effect = parse_effect("physics_flags", &gp2::parse(bytes));
        let prim = &effect.primitives[0];
        assert_eq!(prim.min, [-1.0, -2.0, -3.0]);
        assert_eq!(prim.max, [1.0, 2.0, 3.0]);
        assert_eq!(prim.elasticity, Range::fixed(0.4));
        assert_eq!(prim.flags & (FX_USE_BBOX | FX_APPLY_PHYSICS), FX_USE_BBOX | FX_APPLY_PHYSICS);
    }

    #[test]
    fn fx_lists_imply_their_runtime_flags() {
        let bytes = b"particle\n{\n impactfx\n [\n  chunks/rockimpact\n ]\n}\n\
emitter\n{\n deathfx\n [\n  chunks/puff\n ]\n emitfx\n [\n  env/smoke\n ]\n}\n";
        let effect = parse_effect("fx_lists", &gp2::parse(bytes));
        let impact = &effect.primitives[0];
        assert_eq!(impact.impact_fx, ["chunks/rockimpact"]);
        assert_eq!(impact.flags, FX_IMPACT_RUNS_FX | FX_APPLY_PHYSICS);
        let emitter = &effect.primitives[1];
        assert_eq!(emitter.death_fx, ["chunks/puff"]);
        assert_eq!(emitter.emitter_fx, ["env/smoke"]);
        assert_eq!(emitter.flags, FX_DEATH_RUNS_FX | FX_EMIT_FX);
    }

    #[test]
    fn vectors_need_three_or_six_values_and_floats_accept_one_or_two() {
        let mut ranges = [Range::fixed(9.0); 3];
        set_vector(&mut ranges, "1 2 3 4");
        assert_eq!(ranges[0], Range::fixed(9.0), "four values are rejected");
        set_vector(&mut ranges, "1 2 3 4 5 6");
        assert_eq!(ranges[2], Range { min: 3.0, max: 6.0 });
        let mut range = Range::default();
        set_range(&mut range, "5 extra words");
        assert_eq!(range, Range::fixed(5.0));
    }
}
