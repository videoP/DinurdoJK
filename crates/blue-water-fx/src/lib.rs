//! Preserved Blue Water ocean effects on the JKA device: the dispersive ship wake field, trail foam and slick,
//! torpedo tracks, bow waves, hull contact foam and splash/impact rings (all driven through `ShipWake`), shaded by the
//! upstream surface's `wake.wgsl` sampler. Source is the vendored `naval-ocean` crate, compiled in place.
//! Copyright (c) 2026 Bill Yang. MIT; see vendor/blue-water-sea-and-sky/LICENSE.
pub use blue_water_core::{QualityTier, gusts, optics, spectrum, whitecaps};
pub mod shaders;
pub mod spindrift;
#[path = "../../../vendor/blue-water-sea-and-sky/ocean/src/hull_profile.rs"]
pub mod hull_profile;
#[path = "../../../vendor/blue-water-sea-and-sky/ocean/src/hull_foam.rs"]
pub mod hull_foam;
#[path = "../../../vendor/blue-water-sea-and-sky/ocean/src/bow_waves.rs"]
pub mod bow_waves;
#[path = "../../../vendor/blue-water-sea-and-sky/ocean/src/trail.rs"]
pub mod trail;
#[path = "../../../vendor/blue-water-sea-and-sky/ocean/src/torpedo_tracks.rs"]
pub mod torpedo_tracks;
#[path = "../../../vendor/blue-water-sea-and-sky/ocean/src/stamps.rs"]
pub mod stamps;
#[path = "../../../vendor/blue-water-sea-and-sky/ocean/src/wake/mod.rs"]
pub mod wake;
#[path = "../../../vendor/blue-water-sea-and-sky/ocean/src/ship_wake.rs"]
pub mod ship_wake;
