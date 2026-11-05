//! Small renderer building blocks shared by the client and the subsystem
//! crates split out of it (river): camera, coordinate conversion, classic
//! entity light, color helpers and a log sink the client installs.

pub mod camera;
pub mod color;
pub mod coordinates;
pub mod light;
pub mod log;
