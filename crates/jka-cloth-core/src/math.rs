//! Precision aliases independent of Rapier and Parry.

#[cfg(not(feature = "f64"))]
pub type Real = f32;
#[cfg(feature = "f64")]
pub type Real = f64;
#[cfg(feature = "f64")]
pub use glam::DVec3 as Vec3;
#[cfg(not(feature = "f64"))]
pub use glam::Vec3;

/// Absolute length tolerance used by metre-scale reference tests.
#[cfg(not(feature = "f64"))]
pub const LENGTH_EPSILON: Real = 1.0e-5;
#[cfg(feature = "f64")]
pub const LENGTH_EPSILON: Real = 1.0e-9;
