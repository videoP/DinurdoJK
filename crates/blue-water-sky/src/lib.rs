//! Original Blue Water atmosphere and celestial renderer, on JKA host services.
pub use glam as math;
mod sky_mean;
pub use sky_mean::SkyMeanSampler;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/aerial.rs"]
pub mod aerial;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/cover.rs"]
mod cover;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/environment.rs"]
pub mod environment;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/gpu.rs"]
mod gpu;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/half.rs"]
mod half;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/haze.rs"]
pub mod haze;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/model.rs"]
pub mod model;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/shaders.rs"]
pub mod shaders;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/sky.rs"]
mod sky;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/stars.rs"]
pub mod stars;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/state.rs"]
pub mod state;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/tables.rs"]
mod tables;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/uniforms.rs"]
mod uniforms;
pub use cover::{CloudCover, CloudCoverFrame};
pub use environment::*;
pub use shaders::add_sky_modules;
pub use sky::{CloudHooks, SeaLevel, Sky, SkyPipelines, SkyQuality, SkyTargets, SkyView};
pub use state::{
    CloudGenus, CloudScene, SkyAngles, DEFAULT_SKY_SCENE, HighCloudGenus, HighClouds, SkyScene, WeatherScene,
};
pub use uniforms::SkyUniforms;
