//! Visible footprint of authored EFX flame and smoke sprites. Everything is derived
//! from the existing OpenJK-compatible primitive templates, never from effect
//! filenames or screenshot-sized multipliers. Unsupported motion is rejected,
//! leaving the stock flame visible instead of silently guessing a replacement.
//!
//! The footprint is the region a typical sprite actually covers while it is
//! visible: mean spawn parameters and the engine's own size/alpha interpolation
//! (`system::interpolation`). Smoke also carries a representative sprite radius
//! so the fluid painter can fill that footprint at map scale.

use super::system::interpolation;
use super::template::{
    EffectTemplate, PrimType, Range, Rng, FX_ACCEL_IS_ABSOLUTE, FX_ALPHA_SHIFT,
    FX_APPLY_PHYSICS, FX_CHEAP_ORG_CALC, FX_ORG_ON_CYLINDER, FX_ORG_ON_SPHERE, FX_PARM_MASK,
    FX_RAND, FX_RAND_ROT_AROUND_FWD, FX_RELATIVE, FX_SIZE_SHIFT, FX_VEL_IS_ABSOLUTE, FX_WAVE,
};

/// Sprites below this opacity do not contribute to the visible footprint.
const VISIBLE_ALPHA: f32 = 0.5;
/// Lifetime samples per layer; the size/alpha curves are piecewise linear.
const LIFETIME_SAMPLES: u32 = 32;

#[derive(Clone, Copy, Debug)]
struct FlameLayer {
    origin: [Range; 3],
    velocity: [Range; 3],
    acceleration: [Range; 3],
    gravity: Range,
    life_ms: Range,
    /// Group flags (alpha/size interpolation modes), as parsed.
    prim_flags: u32,
    /// start, end, parm
    size: [Range; 3],
    alpha: [Range; 3],
    /// Spawn flags (axis handling).
    flags: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct FlameGeometry {
    layers: Vec<FlameLayer>,
    smoke: bool,
    pub(crate) cull_range_sq: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FlameBounds {
    /// EFX-relative JKA coordinates; the caller converts to renderer axes.
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub smoke: Option<SmokeMotion>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SmokeMotion {
    pub origin: [f32; 3],
    pub radius: f32,
    /// Vertical travel in map units per second.
    pub rise_speed: f32,
    pub lifetime: f32,
}

impl FlameGeometry {
    pub(crate) fn from_template(template: &EffectTemplate) -> Option<Self> {
        Self::from_layers(template, crate::ninja_fire_debug::original_flame_shader, false)
    }

    pub(crate) fn from_smoke_template(template: &EffectTemplate) -> Option<Self> {
        // Linked effects share the root runner's draw ownership. Keep those
        // graphs on the authored path so a child using the same shader is safe.
        if template.primitives.iter().any(|prim| {
            !prim.play_fx.is_empty() || !prim.impact_fx.is_empty()
                || !prim.death_fx.is_empty() || !prim.emitter_fx.is_empty()
        }) {
            return None;
        }
        Self::from_layers(template, crate::ninja_fire_debug::original_smoke_shader, true)
    }

    fn from_layers(
        template: &EffectTemplate, matches_shader: fn(&str) -> bool, smoke: bool,
    ) -> Option<Self> {
        let mut layers = Vec::new();
        let mut cull_range_sq = f32::INFINITY;
        for prim in &template.primitives {
            if smoke && prim.kind != PrimType::Particle { continue; }
            let matching = prim.media.iter().filter(|shader| matches_shader(shader)).count();
            if matching == 0 || prim.spawn_count.max <= 0.0 { continue; }
            if prim.kind != PrimType::Particle || matching != prim.media.len() ||
                (smoke && prim.flags & (FX_APPLY_PHYSICS | FX_RELATIVE) != 0) ||
                prim.spawn_flags & (FX_ORG_ON_SPHERE | FX_ORG_ON_CYLINDER | FX_RAND_ROT_AROUND_FWD) != 0 {
                return None;
            }
            let largest_size = [prim.size_start.min, prim.size_start.max,
                prim.size_end.min, prim.size_end.max]
                .into_iter().map(f32::abs).fold(0.0_f32, f32::max);
            let life_ok = prim.life.min.max(prim.life.max) > 0.0
                && prim.life.min.is_finite() && prim.life.max.is_finite();
            if !(life_ok && largest_size > 0.0 && largest_size.is_finite()) {
                return None;
            }
            if prim.cull_range_sq > 0.0 {
                cull_range_sq = cull_range_sq.min(prim.cull_range_sq);
            }
            layers.push(FlameLayer {
                origin: prim.origin1, velocity: prim.velocity,
                acceleration: prim.acceleration, gravity: prim.gravity,
                life_ms: prim.life, prim_flags: prim.flags,
                size: [prim.size_start, prim.size_end, prim.size_parm],
                alpha: [prim.alpha_start, prim.alpha_end, prim.alpha_parm],
                flags: prim.spawn_flags,
            });
        }
        (!layers.is_empty()).then_some(Self { layers, smoke, cull_range_sq })
    }

    /// Union of visible sprite quads across the layers. `axis` is the FX
    /// scheduler's orientation basis, in JKA coordinates.
    pub(crate) fn visible_bounds(&self, axis: [[f32; 3]; 3]) -> Option<FlameBounds> {
        let visible_alpha = if self.smoke { 0.05 } else { VISIBLE_ALPHA };
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        let mut weighted_radius_sq = 0.0;
        let mut weighted_origin = [0.0; 3];
        let mut weighted_rise_speed = 0.0;
        let mut weighted_lifetime = 0.0;
        let mut total_weight = 0.0;
        // Only the deterministic interpolation modes are evaluated, so the
        // generator is never advanced.
        let mut rng = Rng::new(1);
        for layer in &self.layers {
            let origin_axis = if layer.flags & FX_CHEAP_ORG_CALC != 0 { identity() } else { axis };
            let velocity_axis = if layer.flags & FX_VEL_IS_ABSOLUTE != 0 { identity() } else { axis };
            let acceleration_axis = if layer.flags & FX_ACCEL_IS_ABSOLUTE != 0 { identity() } else { axis };
            let origin = midpoint(transform_ranges(layer.origin, origin_axis));
            let velocity = midpoint(transform_ranges(layer.velocity, velocity_axis));
            let mut acceleration = midpoint(transform_ranges(layer.acceleration, acceleration_axis));
            acceleration[2] += mid(layer.gravity);
            let life_ms = mid(layer.life_ms).round() as i32;
            if life_ms <= 0 { continue; }
            if self.smoke {
                let t = life_ms as f32 * 0.001;
                let motion = std::array::from_fn::<_, 3, _>(|i| {
                    velocity[i] * t + 0.5 * acceleration[i] * t * t
                });
                // The Ninja volume paints an upright plume. Leave horizontal
                // jets, downwash and similar directional effects authored.
                if motion[2] < motion[0].hypot(motion[1]) {
                    return None;
                }
            }
            // Same parameter mapping as the spawner: a percentage of the
            // lifetime, except wave (radians per ms) and unflagged (absolute).
            let parm = |range: Range, shift: u32| {
                let value = mid(range);
                match (layer.prim_flags >> shift) & FX_PARM_MASK {
                    FX_WAVE => value * std::f32::consts::PI * 0.001,
                    0 => value,
                    _ => value * 0.01 * life_ms as f32,
                }
            };
            let flags = |shift: u32| (layer.prim_flags >> shift) & 0xF & !FX_RAND;
            let (size_flags, alpha_flags) = (flags(FX_SIZE_SHIFT), flags(FX_ALPHA_SHIFT));
            let size_parm = parm(layer.size[2], FX_SIZE_SHIFT);
            let alpha_parm = parm(layer.alpha[2], FX_ALPHA_SHIFT);
            for step in 0..LIFETIME_SAMPLES {
                let time_ms = ((step as f32 + 0.5) * life_ms as f32 / LIFETIME_SAMPLES as f32) as i32;
                let a = interpolation(alpha_flags, alpha_parm, 0, life_ms, time_ms, &mut rng);
                let alpha = (mid(layer.alpha[0]) * a + mid(layer.alpha[1]) * (1.0 - a)).clamp(0.0, 1.0);
                if alpha < visible_alpha { continue; }
                let s = interpolation(size_flags, size_parm, 0, life_ms, time_ms, &mut rng);
                let radius = (mid(layer.size[0]) * s + mid(layer.size[1]) * (1.0 - s)).abs();
                let t = time_ms as f32 * 0.001;
                if self.smoke {
                    // Average the projected area over visible time, weighted
                    // by alpha so faint late sprites contribute less.
                    let weight = alpha * life_ms as f32;
                    weighted_radius_sq += weight * radius * radius;
                    weighted_origin = std::array::from_fn(|i| weighted_origin[i] + weight * origin[i]);
                    weighted_rise_speed += weight * (velocity[2] + acceleration[2] * t);
                    weighted_lifetime += weight * life_ms as f32 * 0.001;
                    total_weight += weight;
                }
                for i in 0..3 {
                    let centre = origin[i] + velocity[i] * t + 0.5 * acceleration[i] * t * t;
                    min[i] = min[i].min(centre - radius);
                    max[i] = max[i].max(centre + radius);
                }
            }
        }
        let smoke = if self.smoke {
            let radius = (weighted_radius_sq / total_weight).sqrt();
            let origin = weighted_origin.map(|axis| axis / total_weight);
            let rise_speed = weighted_rise_speed / total_weight;
            let lifetime = weighted_lifetime / total_weight;
            if !radius.is_finite() || radius <= 0.0
                || !origin.iter().all(|axis| axis.is_finite()) || !rise_speed.is_finite()
                || !lifetime.is_finite() || lifetime <= 0.0
            {
                return None;
            }
            Some(SmokeMotion { origin, radius, rise_speed, lifetime })
        } else {
            None
        };
        (min.iter().chain(max.iter()).all(|x| x.is_finite())
            && (0..3).all(|i| max[i] > min[i]))
            .then_some(FlameBounds { min, max, smoke })
    }
}

fn mid(range: Range) -> f32 {
    0.5 * (range.min + range.max)
}

fn midpoint((lo, hi): ([f32; 3], [f32; 3])) -> [f32; 3] {
    std::array::from_fn(|i| 0.5 * (lo[i] + hi[i]))
}

fn identity() -> [[f32; 3]; 3] {
    [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
}

fn transform_ranges(ranges: [Range; 3], axis: [[f32; 3]; 3]) -> ([f32; 3], [f32; 3]) {
    let mut lo = [0.0; 3];
    let mut hi = [0.0; 3];
    for i in 0..3 {
        for j in 0..3 {
            let a = ranges[j].min * axis[j][i];
            let b = ranges[j].max * axis[j][i];
            lo[i] += a.min(b);
            hi[i] += a.max(b);
        }
    }
    (lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::{gp2, template::parse_effect_without_auto_light};

    fn footprint(source: &str) -> FlameBounds {
        let effect = parse_effect_without_auto_light("example", &gp2::parse(source.as_bytes()));
        FlameGeometry::from_template(&effect).unwrap().visible_bounds(identity()).unwrap()
    }

    #[test]
    fn footprint_follows_authored_motion_size_and_alpha() {
        // Constant size 5, rising 10 u/s for 1 s, no alpha fade: every sample
        // is visible, so the quad union spans the whole path.
        let b = footprint(r#"
Particle
{
    life 1000
    velocity 0 0 10 0 0 10
    size
    {
        start 5
        end 5
    }
    shaders
    [
        gfx/effects/fire2
    ]
}
"#);
        assert_eq!(b.min[0], -5.0);
        assert!((b.max[2] - 15.0).abs() < 0.5, "{b:?}");
    }

    #[test]
    fn sprites_that_have_faded_out_do_not_extend_the_footprint() {
        // Alpha reaches zero at the end of life, so the last samples (and the
        // furthest travel) are excluded from the visible footprint.
        let visible = footprint(r#"
Particle
{
    life 1000
    velocity 100 0 0 100 0 0
    alpha
    {
        end 0
        flags linear
    }
    size
    {
        start 1
        end 1
    }
    shaders
    [
        gfx/effects/fire2
    ]
}
"#);
        assert!(visible.max[0] < 100.0, "{visible:?}");
        assert!(visible.max[0] > 40.0, "{visible:?}");
    }

    #[test]
    fn change_in_effect_geometry_changes_footprint_without_filename_override() {
        let parse = |radius| {
            footprint(&format!("Particle\n{{\n life 1000\n size\n {{\n  start {radius}\n  end {radius}\n }}\n shaders\n [\n  gfx/effects/fire2\n ]\n}}\n"))
        };
        assert_eq!(parse(4).max[0], 4.0);
        assert_eq!(parse(20).max[0], 20.0);
    }

    #[test]
    fn shrinking_smoke_uses_visible_particle_area_for_brush_size() {
        let source = r#"
Particle
{
    life 500 1400
    velocity 0 0 12
    gravity 20 30
    size
    {
        start 10 12
        end 3 4
        flags linear
    }
    shaders
    [
        gfx/effects/alpha_smoke
    ]
}
"#;
        let effect = parse_effect_without_auto_light("custom", &gp2::parse(source.as_bytes()));
        let bounds = FlameGeometry::from_smoke_template(&effect)
            .unwrap()
            .visible_bounds(identity())
            .unwrap();
        let smoke = bounds.smoke.unwrap();
        assert!((7.5..7.7).contains(&smoke.radius), "{bounds:?}");
        assert!((23.5..24.2).contains(&smoke.rise_speed), "{bounds:?}");
        assert!((smoke.lifetime - 0.95).abs() < 0.001, "{bounds:?}");
        assert!(bounds.max[2] > 24.0, "{bounds:?}");
    }
}
