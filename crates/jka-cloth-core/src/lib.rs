#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

#[cfg(any(
    all(feature = "f32", feature = "f64"),
    not(any(feature = "f32", feature = "f64"))
))]
compile_error!("rapier-cloth: select exactly one precision feature: f32 or f64");

pub mod math;
pub use math::{Real, Vec3};
pub mod constraints;
pub mod error;
pub mod material;
pub mod mesh;
pub use error::ClothError;
pub use material::ClothMaterial;
pub use mesh::{ClothMesh, GridBuilder};
pub mod cloth;
pub mod contact;
pub mod diagnostics;
pub mod solver;
pub mod surface;
pub use cloth::{Cloth, ClothHandle, ClothSet};
pub use contact::{Contact, ContactKey, ContactSource, ContactStage, NoContacts};
pub use diagnostics::StepReport;
pub use solver::{Solver, SolverSettings, Target};
pub use surface::SurfaceView;
