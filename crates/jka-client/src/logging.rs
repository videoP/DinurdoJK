use std::{
    collections::{HashMap, VecDeque},
    fmt,
    fs::{File, OpenOptions},
    io::{self, IsTerminal, Write},
    path::PathBuf,
    sync::{
        mpsc::{self, Receiver, Sender},
        Mutex, OnceLock,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Error,
}

struct Logger {
    file: File,
    console_color: bool,
    ui_lines: VecDeque<LogRecord>,
    ui_sink: Option<Sender<LogRecord>>,
}

#[derive(Debug, Clone)]
pub struct LogPathLink {
    /// Exact text rendered in the console line for this trusted local path.
    pub label: String,
    /// Local filesystem target. This metadata is only attached by internal code;
    /// arbitrary console/server/chat text is never parsed into a link.
    pub target: PathBuf,
}

#[derive(Debug, Clone)]
pub struct LogRecord {
    pub text: String,
    pub local_time: [u16; 3],
    pub path_links: Vec<LogPathLink>,
}

const MAX_UI_LOG_LINES: usize = 10_000;

static LOGGER: OnceLock<Mutex<Logger>> = OnceLock::new();

pub fn init() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let directory = executable.parent().ok_or("Cannot locate executable")?;
    let path = directory.join("latest.log");
    // Truncate once, then keep an append handle: the crash reporter writes to
    // this file through its own handle, and appending keeps the two from
    // overwriting each other's lines.
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let file = OpenOptions::new()
        .append(true)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;

    let console_color = enable_console_color();
    let _ = LOGGER.set(Mutex::new(Logger {
        file,
        console_color,
        ui_lines: VecDeque::with_capacity(1024),
        ui_sink: None,
    }));

    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|location| {
                format!(
                    "{}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                )
            })
            .unwrap_or_else(|| "unknown location".to_owned());
        let payload = if let Some(message) = info.payload().downcast_ref::<&str>() {
            (*message).to_owned()
        } else if let Some(message) = info.payload().downcast_ref::<String>() {
            message.clone()
        } else {
            "non-string panic payload".to_owned()
        };
        write_line(
            Level::Error,
            format_args!("^1PANIC^7 at {location}: {payload}"),
        );
        let backtrace = std::backtrace::Backtrace::force_capture();
        write_line(Level::Error, format_args!("{backtrace}"));
    }));

    // wgpu and its HAL report driver warnings (DX12/Vulkan validation, device
    // loss reasons, surface problems) through the `log` facade; without a
    // logger they vanish.
    if log::set_logger(&FACADE_LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Warn);
    }

    Ok(path)
}

/// Distinct `log`-facade messages remembered for de-duplication.
const FACADE_MAX_DISTINCT: usize = 512;
/// Times one facade message is written before it is suppressed.
const FACADE_MAX_REPEATS: u32 = 5;

struct FacadeLogger;

static FACADE_LOGGER: FacadeLogger = FacadeLogger;
static FACADE_SEEN: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();

impl log::Log for FacadeLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        // naga reports shader failures through returned errors; its warnings
        // are per-shader noise.
        if metadata.target().starts_with("naga") {
            metadata.level() <= log::Level::Error
        } else {
            metadata.level() <= log::Level::Warn
        }
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let message = format!("{}: {}", record.target(), record.args());
        let repeats = {
            let Ok(mut seen) = FACADE_SEEN.get_or_init(Default::default).lock() else {
                return;
            };
            if seen.len() >= FACADE_MAX_DISTINCT && !seen.contains_key(&message) {
                return;
            }
            let count = seen.entry(message.clone()).or_insert(0);
            *count += 1;
            *count
        };
        if repeats > FACADE_MAX_REPEATS {
            return;
        }
        let (level, label) = if record.level() == log::Level::Error {
            (Level::Error, "ERROR")
        } else {
            (Level::Info, "WARNING")
        };
        let suffix = if repeats == FACADE_MAX_REPEATS {
            " (repeats suppressed)"
        } else {
            ""
        };
        write_line(level, format_args!("[{label} {message}]{suffix}"));
    }

    fn flush(&self) {}
}

pub fn write_line(level: Level, args: fmt::Arguments<'_>) {
    write_line_inner(level, args, Vec::new());
}

/// Write an engine-authored console/log line with an explicitly trusted local
/// filesystem path. The path is metadata, not something rediscovered by parsing
/// the rendered text, so external/server/chat strings cannot forge links.
pub fn write_line_with_path(level: Level, args: fmt::Arguments<'_>, path: impl Into<PathBuf>) {
    let target = path.into();
    let label = target.display().to_string();
    write_line_inner(level, args, vec![LogPathLink { label, target }]);
}

fn write_line_inner(level: Level, args: fmt::Arguments<'_>, path_links: Vec<LogPathLink>) {
    let message = args.to_string();
    let plain = strip_jka_colors(&message);

    if let Some(logger) = LOGGER.get() {
        if let Ok(mut logger) = logger.lock() {
            let _ = writeln!(logger.file, "{plain}");
            // Flush every line deliberately: latest.log is primarily a crash diagnostic.
            let _ = logger.file.flush();
            write_console(level, &message, logger.console_color);
            if logger.ui_lines.len() >= MAX_UI_LOG_LINES {
                logger.ui_lines.pop_front();
            }
            let record = LogRecord {
                text: message,
                local_time: local_hms(),
                path_links,
            };
            logger.ui_lines.push_back(record.clone());
            if let Some(sink) = &logger.ui_sink {
                if sink.send(record).is_err() {
                    logger.ui_sink = None;
                }
            }
            return;
        }
    }

    // Logging may be called before init() if startup fails very early.
    let mut stream: Box<dyn Write> = match level {
        Level::Info => Box::new(io::stdout()),
        Level::Error => Box::new(io::stderr()),
    };
    let _ = writeln!(stream, "{plain}");
}


#[cfg(windows)]
fn local_hms() -> [u16; 3] {
    #[repr(C)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }

    #[link(name = "Kernel32")]
    extern "system" {
        fn GetLocalTime(system_time: *mut SystemTime);
    }

    let mut time = SystemTime {
        year: 0,
        month: 0,
        day_of_week: 0,
        day: 0,
        hour: 0,
        minute: 0,
        second: 0,
        milliseconds: 0,
    };
    unsafe { GetLocalTime(&mut time) };
    [time.hour, time.minute, time.second]
}

#[cfg(not(windows))]
fn local_hms() -> [u16; 3] {
    // std has no cross-platform local-time conversion. Keep timestamps useful
    // on non-Windows builds with UTC rather than adding a runtime dependency.
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        % 86_400;
    [
        (seconds / 3_600) as u16,
        ((seconds / 60) % 60) as u16,
        (seconds % 60) as u16,
    ]
}

pub fn subscribe() -> Receiver<LogRecord> {
    let (tx, rx) = mpsc::channel();
    if let Some(logger) = LOGGER.get() {
        if let Ok(mut logger) = logger.lock() {
            for line in &logger.ui_lines {
                let _ = tx.send(line.clone());
            }
            logger.ui_sink = Some(tx);
        }
    }
    rx
}

pub(crate) fn strip_jka_colors(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '^' && chars.peek().is_some_and(|next| next.is_ascii_digit()) {
            chars.next();
            continue;
        }
        out.push(ch);
    }
    out
}

fn write_console(level: Level, text: &str, color_enabled: bool) {
    let use_stderr = level == Level::Error;
    let is_terminal = if use_stderr {
        io::stderr().is_terminal()
    } else {
        io::stdout().is_terminal()
    };

    if !color_enabled || !is_terminal {
        let plain = strip_jka_colors(text);
        if use_stderr {
            let mut stream = io::stderr().lock();
            let _ = writeln!(stream, "{plain}");
        } else {
            let mut stream = io::stdout().lock();
            let _ = writeln!(stream, "{plain}");
        }
        return;
    }

    let base = default_ansi(level, text);
    if use_stderr {
        let mut stream = io::stderr().lock();
        let _ = write_colored(&mut stream, text, base);
    } else {
        let mut stream = io::stdout().lock();
        let _ = write_colored(&mut stream, text, base);
    }
}

fn default_ansi(level: Level, text: &str) -> &'static str {
    if level == Level::Error || text.contains("ERROR") || text.contains("PANIC") {
        "\x1b[91m"
    } else if text.starts_with("Warning:") || text.contains("WARNING") {
        "\x1b[93m"
    } else if text.starts_with("[JKA PERF") || text.starts_with("[PLANAR DEBUG]") {
        "\x1b[96m"
    } else {
        "\x1b[97m"
    }
}

fn write_colored(mut out: impl Write, text: &str, base: &str) -> io::Result<()> {
    write!(out, "{base}")?;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '^' {
            if let Some(next) = chars.peek().copied() {
                if let Some(color) = jka_ansi(next) {
                    chars.next();
                    write!(out, "{color}")?;
                    continue;
                }
            }
        }
        write!(out, "{ch}")?;
    }
    writeln!(out, "\x1b[0m")?;
    out.flush()
}

fn jka_ansi(code: char) -> Option<&'static str> {
    Some(match code {
        '0' => "\x1b[90m", // black is made visible as dark gray in a terminal
        '1' => "\x1b[91m",
        '2' => "\x1b[92m",
        '3' => "\x1b[93m",
        '4' => "\x1b[94m",
        '5' => "\x1b[96m",
        '6' => "\x1b[95m",
        '7' => "\x1b[97m",
        '8' => "\x1b[38;5;208m",
        '9' => "\x1b[37m",
        _ => return None,
    })
}

#[cfg(windows)]
fn enable_console_color() -> bool {
    type Handle = *mut core::ffi::c_void;
    const STD_OUTPUT_HANDLE: u32 = (-11_i32) as u32;
    const STD_ERROR_HANDLE: u32 = (-12_i32) as u32;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

    #[link(name = "Kernel32")]
    extern "system" {
        fn GetStdHandle(n_std_handle: u32) -> Handle;
        fn GetConsoleMode(console_handle: Handle, mode: *mut u32) -> i32;
        fn SetConsoleMode(console_handle: Handle, mode: u32) -> i32;
    }

    unsafe fn enable(handle: Handle) -> bool {
        if handle.is_null() || handle as isize == -1 {
            return false;
        }
        let mut mode = 0_u32;
        if unsafe { GetConsoleMode(handle, &mut mode) } == 0 {
            return false;
        }
        (unsafe { SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) }) != 0
    }

    unsafe { enable(GetStdHandle(STD_OUTPUT_HANDLE)) | enable(GetStdHandle(STD_ERROR_HANDLE)) }
}

#[cfg(not(windows))]
fn enable_console_color() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::strip_jka_colors;

    #[test]
    fn strips_jka_color_codes_from_log_file_text() {
        assert_eq!(strip_jka_colors("^1error ^7normal ^2ok"), "error normal ok");
    }
}
