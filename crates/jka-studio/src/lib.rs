//! Render Studio project model and editing logic: timeline, camera director
//! and auto director, race comparison data, light rigs, audio clips, camera
//! rigs and export profiling. Kept out of the client crate so Studio edits
//! and client edits don't recompile each other.
//!
//! Everything that plays demos, drives the renderer or draws UI stays in the
//! client and calls into this crate; nothing here runs during gameplay.

// Only the auto-director tests print today.
#[cfg_attr(not(test), allow(unused_macros))]
macro_rules! eprintln {
    () => { jka_render_core::log::write_line(true, format_args!("")) };
    ($($arg:tt)*) => { jka_render_core::log::write_line(true, format_args!($($arg)*)) };
}

pub mod attachment;
pub mod lens;
pub mod render_studio_audio;
pub mod render_studio_auto_director;
pub mod render_studio_camera;
pub mod render_studio_cfg;
pub mod render_studio_cinematics;
pub mod render_studio_director;
pub mod render_studio_lights;
pub mod render_studio_media;
pub mod render_studio_profile;
pub mod render_studio_race;
pub mod render_studio_spatial;
pub mod render_studio_timeline;
pub mod render_studio_transitions;
pub mod trail;
