//! Console/log sink for crates split out of the client. The client installs
//! its logger at startup; until then (tests, tools) lines go to stdout/stderr.
use std::{fmt, sync::OnceLock};

pub type WriteLine = fn(error: bool, args: fmt::Arguments<'_>);

static SINK: OnceLock<WriteLine> = OnceLock::new();

/// First call wins; later calls are ignored.
pub fn install(write_line: WriteLine) {
    let _ = SINK.set(write_line);
}

pub fn write_line(error: bool, args: fmt::Arguments<'_>) {
    match SINK.get() {
        Some(sink) => sink(error, args),
        None if error => std::eprintln!("{args}"),
        None => std::println!("{args}"),
    }
}
