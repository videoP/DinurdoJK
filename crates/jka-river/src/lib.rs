//! Unreal Engine river port: source-filled water simulation, Single Layer
//! Water body, foam, capture and composite passes. Split out of the client
//! crate so river edits and client edits don't recompile each other. The
//! GPU integration tests that need the client renderer stay in the client
//! (`river_port_tests`).

macro_rules! eprintln {
    () => { jka_render_core::log::write_line(true, format_args!("")) };
    ($($arg:tt)*) => { jka_render_core::log::write_line(true, format_args!($($arg)*)) };
}

pub mod river_authoring;
pub mod river_capture;
pub mod river_composite;
pub mod river_discharge;
pub mod river_foam;
pub mod river_gpu;
pub mod river_grid_mesh;
pub mod river_map;
pub mod river_material;
pub mod river_ocean;
pub mod river_runtime;
pub mod river_settings;
pub mod river_slw;
pub mod river_surface;
pub mod river_tiles;
pub mod river_water_info;
pub mod river_water_info_capture;
