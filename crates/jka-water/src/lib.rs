//! Renderer-independent water simulation primitives for JKA.
//!
//! Regime-specific implementations are namespaced rather than implying that
//! one solver fits oceans, coasts, and rivers. Rendering, WGPU resources,
//! client scheduling, and gameplay state remain in `jka-client`.

pub mod coastal;
pub mod river;
