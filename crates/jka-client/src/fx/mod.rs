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

pub mod draw;
pub mod gp2;
pub mod system;
pub mod template;

