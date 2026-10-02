//! OpenJK effects system (codemp/client/Fx*.cpp): `.efx` templates, the
//! scheduler and primitive simulation, plus view-dependent tessellation for
//! the WGPU dynamic-model pass.

/// `cg_fxFPS` values. A zero value intentionally means the original Jedi
/// Academy behavior: continuous projectile EFX are invoked once per
/// presentation frame and therefore scale with render FPS.
pub const FX_FPS_LEGACY_JKA: u32 = 0;
pub const FX_FPS_DEFAULT: u32 = 90;
pub const FX_FPS_MIN: u32 = 15;
pub const FX_FPS_MAX: u32 = 250;

/// `fx_physics` values (TaystJK/jaPRO `fx_physics` cvar semantics).
///
/// * `0` disables all FX particle physics.
/// * `1` is "non-expensive physics only". Stock never had any non-expensive
///   collision path, so it behaves exactly like `0`; it is accepted for config
///   compatibility.
/// * `2` (default) traces primitives that carry the authored `expensivePhysics`
///   flag.
/// * `3` forces the world trace on every primitive that has physics enabled.
pub const FX_PHYSICS_OFF: u32 = 0;
pub const FX_PHYSICS_AUTHORED: u32 = 2;
pub const FX_PHYSICS_ALL: u32 = 3;
pub const FX_PHYSICS_DEFAULT: u32 = FX_PHYSICS_AUTHORED;

pub fn physics_label(mode: u32) -> &'static str {
    match mode {
        FX_PHYSICS_OFF | 1 => "OFF",
        FX_PHYSICS_AUTHORED => "AUTHORED (EXPENSIVEPHYSICS FLAG)",
        _ => "ALL (FORCE WORLD TRACE ON EVERY PHYSICS PRIMITIVE)",
    }
}

/// `fx_lod` values (DinurdoJK; spawn-time EFX level of detail).
///
/// * `0` is stock behavior: every authored primitive spawns at its authored count.
/// * `1` also honors authored `cullRange` (distance culling). OpenJK/JAPRO
///   parse the key but have it commented out, so this differs from stock.
/// * `2` (default) adds screen-size density scaling for populations of
///   Particle/OrientedParticle/Tail primitives whose individual elements
///   project to only a few pixels.
///
/// Decisions are made once, when a primitive is scheduled; live particles are
/// never thinned or removed.
pub const FX_LOD_OFF: u32 = 0;
pub const FX_LOD_AUTHORED: u32 = 1;
pub const FX_LOD_ADAPTIVE: u32 = 2;
pub const FX_LOD_DEFAULT: u32 = FX_LOD_ADAPTIVE;

/// `r_fxLodScale` / `r_lodScale` default (OpenJK's `r_lodscale` is 5). Like
/// `r_lodscale`, larger keeps more detail to greater distances. For EFX it
/// multiplies authored `cullRange`; the authored values are short, so the
/// default reaches 5x as far. The adaptive density curve is tuned at this
/// default and shifts proportionally (`scale / default`).
pub const LOD_SCALE_DEFAULT: f32 = 5.0;
pub const LOD_SCALE_MIN: f32 = 0.1;
pub const LOD_SCALE_MAX: f32 = 100.0;

pub fn lod_label(mode: u32) -> &'static str {
    match mode {
        FX_LOD_OFF => "OFF (STOCK)",
        FX_LOD_AUTHORED => "AUTHORED (CULLRANGE)",
        _ => "ADAPTIVE (CULLRANGE + SCREEN-SIZE DENSITY)",
    }
}

/// Fraction of an authored particle population kept when one element projects
/// to `px` pixels. Smoothstep over log size: 1.0 at 48 px and above, about
/// 0.56 at 12 px, about 0.30 at 6 px, floored at 0.25.
pub fn lod_density(px: f32) -> f32 {
    const FULL_PX: f32 = 48.0;
    const FLOOR_PX: f32 = 4.0;
    const FLOOR: f32 = 0.25;
    if px >= FULL_PX {
        return 1.0;
    }
    if px <= FLOOR_PX {
        return FLOOR;
    }
    let t = ((px / FLOOR_PX).ln() / (FULL_PX / FLOOR_PX).ln()).clamp(0.0, 1.0);
    FLOOR + (1.0 - FLOOR) * t * t * (3.0 - 2.0 * t)
}

pub mod draw;
pub mod gp2;
pub mod system;
pub mod template;

