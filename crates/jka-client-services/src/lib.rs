//! Client code that never runs inside a gameplay frame: server browser,
//! chat translation, JK2 demo diagnostics, gamma helper, screenshots, credentials, clipboard, protocol links, paths,
//! sv_pure, vote/VGS parsing and the neon menu layout. Kept out of the client
//! crate so edits on either side don't recompile the other. Lines print
//! through the client logger via `jka_render_core::log`.

macro_rules! println {
    () => { jka_render_core::log::write_line(false, format_args!("")) };
    ($($arg:tt)*) => { jka_render_core::log::write_line(false, format_args!($($arg)*)) };
}

macro_rules! eprintln {
    () => { jka_render_core::log::write_line(true, format_args!("")) };
    ($($arg:tt)*) => { jka_render_core::log::write_line(true, format_args!($($arg)*)) };
}

pub mod chat_translation;
pub mod clipboard;
pub mod credential_store;
pub mod display_gamma;
pub mod filesystem_paths;
pub mod frontend_cache_color;
pub mod jk2_diagnostics;
pub mod neon_menu;
pub mod protocol_link;
pub mod pure;
pub mod screenshot;
pub mod server_browser;
pub mod startup_timeline;
pub mod vgs;
pub mod vote;
