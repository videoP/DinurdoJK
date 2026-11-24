//! Fluid Ninja Live 2 river and flame presentation.
//!
//! Public facade. The implementation is private: the DinurdoJK-ninja
//! repository checked out beside this one (see build.rs). Without it this
//! crate compiles `stub.rs`, whose constructors report that Fluid Ninja is not
//! included in the build, so public clones and CI without the secret still
//! build and run with the feature unavailable.
//!
//! Contract: every item a consumer (jka-river, jka-client) uses must exist in
//! BOTH the private implementation and `stub.rs` with the same signature.
//! Keep that surface small and high-level; solver internals stay private.

pub mod blood_api;
pub mod flame_api;
pub mod fog_api;
pub mod grass_api;
pub mod river_api;
pub mod snow_api;
pub mod ocean_api;
pub mod coast_api;
pub mod wake_api;
pub mod lake_api;
pub use blood_api::*;
pub use flame_api::*;
pub use fog_api::*;
pub use grass_api::*;
pub use river_api::*;
pub use snow_api::*;
pub use ocean_api::*;
pub use coast_api::*;
pub use wake_api::*;
pub use lake_api::*;

#[cfg(djk_ninja_private)]
#[path = "../../../../DinurdoJK-ninja/src/lib.rs"]
mod imp;

#[cfg(not(djk_ninja_private))]
#[path = "stub.rs"]
mod imp;

pub use imp::*;

/// True when the private implementation is compiled in.
pub const INCLUDED: bool = cfg!(djk_ninja_private);
