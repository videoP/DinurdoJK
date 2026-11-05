//! Game audio: rodio mixer, EAX/EAL reverb and Steam Audio. Kept out of the
//! client crate so audio edits and client edits don't recompile each other.
//! The client wires logging and emote bundle lookups in through [`host`].

pub mod host;

macro_rules! println {
    () => { crate::host::write_line(false, format_args!("")) };
    ($($arg:tt)*) => { crate::host::write_line(false, format_args!($($arg)*)) };
}

macro_rules! eprintln {
    () => { crate::host::write_line(true, format_args!("")) };
    ($($arg:tt)*) => { crate::host::write_line(true, format_args!($($arg)*)) };
}

/// Same gating as the client's `devprintln!`: `developer` level 1..=3.
macro_rules! devprintln {
    ($level:expr, $($arg:tt)*) => {{
        if crate::host::developer_enabled($level) {
            println!($($arg)*);
        }
    }};
}

pub mod audio;
pub mod eal;
pub mod eal_native;
#[cfg(test)]
mod eal_openal;
pub mod eal_worker;
pub mod steam_audio;
