//! Services the client provides to audio: its logger, the `developer` level
//! and emote bundle reads. Unset (tests, tools), audio logs to stdout/stderr
//! and sees no emote bundles.
use std::{fmt, sync::OnceLock};

#[derive(Clone, Copy)]
pub struct Host {
    pub write_line: fn(error: bool, args: fmt::Arguments<'_>),
    pub developer_enabled: fn(level: u8) -> bool,
    /// `Ok(None)` when `qpath` is not inside a mounted emote bundle.
    pub read_emote_asset: fn(qpath: &str, limit: usize) -> Result<Option<Vec<u8>>, String>,
    /// Favourite emote sounds outlive ordinary ones in the SFX cache.
    pub favorite_emote_asset: fn(key: &str) -> bool,
}

impl Host {
    pub const STANDALONE: Self = Self {
        write_line: |error, args| {
            if error {
                std::eprintln!("{args}");
            } else {
                std::println!("{args}");
            }
        },
        developer_enabled: |_| false,
        read_emote_asset: |_, _| Ok(None),
        favorite_emote_asset: |_| false,
    };
}

static HOST: OnceLock<Host> = OnceLock::new();

/// First call wins; later calls are ignored.
pub fn install(host: Host) {
    let _ = HOST.set(host);
}

fn host() -> &'static Host {
    HOST.get().unwrap_or(&Host::STANDALONE)
}

pub fn write_line(error: bool, args: fmt::Arguments<'_>) {
    (host().write_line)(error, args)
}

pub fn developer_enabled(level: u8) -> bool {
    (host().developer_enabled)(level)
}

pub fn read_emote_asset(qpath: &str, limit: usize) -> Result<Option<Vec<u8>>, String> {
    (host().read_emote_asset)(qpath, limit)
}

pub fn favorite_emote_asset(key: &str) -> bool {
    (host().favorite_emote_asset)(key)
}
