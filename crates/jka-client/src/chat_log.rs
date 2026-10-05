//! Session-oriented live chat logging.
//!
//! The gameplay thread only enqueues small typed events. All directory creation,
//! HTML generation and file I/O happens on `chatlog-writer`, so a slow filesystem
//! cannot hitch rendering/input. Message delivery uses a bounded `try_send`: in
//! the pathological case where the writer cannot keep up, chat logging drops a
//! log entry instead of ever blocking the frame thread.

use crate::cgame::ChatKind;
use std::{
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// User-space chat data is pushed to the OS at most this far apart. This is not
/// an fsync/FlushFileBuffers cadence: the OS remains free to coalesce physical
/// disk writes. Clean disconnect/quit always flushes immediately.
const FLUSH_INTERVAL: Duration = Duration::from_secs(10);
const QUEUE_CAPACITY: usize = 1024;
const WRITER_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct SessionMetadata {
    pub directory: PathBuf,
    pub started_unix_ms: u64,
    pub player_name: String,
    pub server_address: SocketAddr,
    pub hostname: String,
    pub fs_game: String,
    pub map_name: String,
}

impl SessionMetadata {
    fn update(&mut self, update: SessionUpdate) -> MetadataEffect {
        let directory_changed = self.directory != update.directory;
        let previous_map = std::mem::replace(&mut self.map_name, update.map_name);
        self.directory = update.directory;
        self.hostname = update.hostname;
        self.fs_game = update.fs_game;
        MetadataEffect {
            directory_changed,
            previous_map,
            current_map: self.map_name.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionUpdate {
    pub directory: PathBuf,
    pub hostname: String,
    pub fs_game: String,
    pub map_name: String,
}

struct MetadataEffect {
    directory_changed: bool,
    previous_map: String,
    current_map: String,
}

#[derive(Debug)]
struct ChatMessage {
    kind: ChatKind,
    team: bool,
    text: String,
    unix_ms: u64,
    server_time: Option<i32>,
}

#[derive(Debug)]
enum Command {
    Begin(SessionMetadata),
    Update(SessionUpdate),
    Message(ChatMessage),
    End { reason: String },
    Shutdown { ack: SyncSender<()> },
}

/// Main-thread handle. The writer itself owns all filesystem state.
pub struct ChatLog {
    tx: Option<SyncSender<Command>>,
    worker: Option<JoinHandle<()>>,
    dropped_messages: Arc<AtomicU64>,
}

impl ChatLog {
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::sync_channel(QUEUE_CAPACITY);
        let dropped_messages = Arc::new(AtomicU64::new(0));
        let worker_dropped = Arc::clone(&dropped_messages);
        match thread::Builder::new()
            .name("chatlog-writer".into())
            .spawn(move || writer_main(rx, worker_dropped))
        {
            Ok(worker) => Self {
                tx: Some(tx),
                worker: Some(worker),
                dropped_messages,
            },
            Err(error) => {
                eprintln!("CHATLOG: could not start writer thread: {error}");
                Self {
                    tx: None,
                    worker: None,
                    dropped_messages,
                }
            }
        }
    }

    /// Lifecycle and message delivery are both nonblocking. With 1024 entries,
    /// dropping a boundary should only be possible after an extreme writer or
    /// filesystem failure; preserving the game thread still takes precedence.
    pub fn begin(&self, metadata: SessionMetadata) {
        self.send_lifecycle(Command::Begin(metadata));
    }

    pub fn update(&self, update: SessionUpdate) {
        self.send_lifecycle(Command::Update(update));
    }

    pub fn end(&self, reason: impl Into<String>) {
        self.send_lifecycle(Command::End {
            reason: reason.into(),
        });
    }

    pub fn message(&self, kind: ChatKind, team: bool, text: String, server_time: Option<i32>) {
        let Some(tx) = &self.tx else { return };
        let command = Command::Message(ChatMessage {
            kind,
            team,
            text,
            unix_ms: unix_ms_now(),
            server_time,
        });
        match tx.try_send(command) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.dropped_messages.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    fn send_lifecycle(&self, command: Command) {
        let Some(tx) = &self.tx else { return };
        match tx.try_send(command) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => {}
            Err(TrySendError::Full(_)) => {
                // Lifecycle transitions are extremely rare. If the bounded queue
                // is saturated, protecting the game thread still wins over a
                // perfect log boundary. Keep even this failure path lock/I/O free.
                self.dropped_messages.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Clean quit gets a short chance to append the footer and push the current
    /// buffer to the OS before the client's fast TerminateProcess path runs.
    /// We deliberately do not wait forever on a wedged disk/driver.
    pub fn shutdown(&mut self, timeout: Duration) {
        let Some(tx) = self.tx.take() else { return };
        let (ack_tx, ack_rx) = mpsc::sync_channel(0);
        if tx.try_send(Command::Shutdown { ack: ack_tx }).is_ok()
            && ack_rx.recv_timeout(timeout).is_ok()
        {
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }
}

impl Drop for ChatLog {
    fn drop(&mut self) {
        // Normal gameplay shutdown uses `shutdown`. This fallback is deliberately
        // nonblocking so dropping App can never stall on filesystem I/O.
        if let Some(tx) = self.tx.take() {
            let (ack, _rx) = mpsc::sync_channel(0);
            let _ = tx.try_send(Command::Shutdown { ack });
        }
    }
}

struct ActiveSession {
    meta: SessionMetadata,
    file: Option<SessionFile>,
}

struct SessionFile {
    writer: BufWriter<File>,
    path: PathBuf,
    dirty: bool,
    messages: u64,
}

fn writer_main(rx: Receiver<Command>, dropped_messages: Arc<AtomicU64>) {
    let mut active: Option<ActiveSession> = None;
    let mut last_flush = Instant::now();

    loop {
        let wait = FLUSH_INTERVAL.saturating_sub(last_flush.elapsed());
        match rx.recv_timeout(wait) {
            Ok(Command::Begin(meta)) => {
                if let Some(session) = active.as_mut() {
                    finish_file(session, "New connection");
                }
                active = Some(ActiveSession { meta, file: None });
            }
            Ok(Command::Update(update)) => {
                let Some(session) = active.as_mut() else { continue };
                let effect = session.meta.update(update);
                if effect.directory_changed && session.file.is_some() {
                    // fs_game changed under a live connection. Keep artifacts in
                    // the directory that owned them and lazily begin another file
                    // in the new active game directory on the next chat line.
                    finish_file(session, "fs_game changed");
                } else if !effect.previous_map.is_empty()
                    && effect.previous_map != effect.current_map
                    && session.file.is_some()
                {
                    if let Err(error) = write_session_event(
                        session.file.as_mut().expect("checked above"),
                        unix_ms_now(),
                        &format!("Map changed to {}", effect.current_map),
                    ) {
                        fail_file(session, error);
                    }
                }
            }
            Ok(Command::Message(message)) => {
                let Some(session) = active.as_mut() else { continue };
                let first_message = session.file.is_none();
                if first_message {
                    match open_session_file(&session.meta) {
                        Ok(file) => session.file = Some(file),
                        Err(error) => {
                            eprintln!("CHATLOG: {error}");
                            continue;
                        }
                    }
                }
                let dropped = dropped_messages.swap(0, Ordering::Relaxed);
                let Some(file) = session.file.as_mut() else { continue };
                let result = (|| -> std::io::Result<()> {
                    if dropped != 0 {
                        write_session_event(
                            file,
                            message.unix_ms,
                            &format!("{dropped} chat log entr{} dropped because the writer queue was full", if dropped == 1 { "y was" } else { "ies were" }),
                        )?;
                    }
                    write_message(file, &message)?;
                    // The first chat writes the HTML shell + first message to the
                    // OS immediately. Afterwards we batch user-space flushes for
                    // up to ten seconds; clean End/Shutdown also flushes.
                    if first_message {
                        file.writer.flush()?;
                        file.dirty = false;
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    fail_file(session, error);
                }
            }
            Ok(Command::End { reason }) => {
                if let Some(mut session) = active.take() {
                    finish_file(&mut session, &reason);
                }
            }
            Ok(Command::Shutdown { ack }) => {
                if let Some(mut session) = active.take() {
                    finish_file(&mut session, "Client quit");
                }
                let _ = ack.send(());
                break;
            }
            Err(RecvTimeoutError::Timeout) => {
                flush_active(&mut active);
                last_flush = Instant::now();
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(mut session) = active.take() {
                    finish_file(&mut session, "Client closed");
                }
                break;
            }
        }

        if last_flush.elapsed() >= FLUSH_INTERVAL {
            flush_active(&mut active);
            last_flush = Instant::now();
        }
    }
}

fn flush_active(active: &mut Option<ActiveSession>) {
    let Some(session) = active.as_mut() else { return };
    let Some(file) = session.file.as_mut() else { return };
    if !file.dirty {
        return;
    }
    match file.writer.flush() {
        Ok(()) => file.dirty = false,
        Err(error) => fail_file(session, error),
    }
}

fn fail_file(session: &mut ActiveSession, error: std::io::Error) {
    if let Some(file) = session.file.take() {
        eprintln!("CHATLOG: writer failed for {}: {error}", file.path.display());
    }
}

fn finish_file(session: &mut ActiveSession, reason: &str) {
    let Some(mut file) = session.file.take() else { return };
    let ended_ms = unix_ms_now();
    let _ = write_session_event(&mut file, ended_ms, reason);
    let ended = format_local_timestamp(ended_ms);
    let _ = writeln!(file.writer, "</section>");
    let _ = writeln!(
        file.writer,
        "<footer>Session ended {} &middot; {} message{}</footer></main></body></html>",
        escape_html(&ended),
        file.messages,
        if file.messages == 1 { "" } else { "s" }
    );
    if let Err(error) = file.writer.flush() {
        eprintln!("CHATLOG: final flush failed for {}: {error}", file.path.display());
    }
}

fn open_session_file(meta: &SessionMetadata) -> Result<SessionFile, String> {
    let started = local_datetime_parts(meta.started_unix_ms);
    let month = format!("{:04}-{:02}", started.year, started.month);
    let directory = meta.directory.join(month);
    fs::create_dir_all(&directory)
        .map_err(|error| format!("couldn't create {}: {error}", directory.display()))?;

    let safe_address = sanitize_filename_component(&meta.server_address.to_string());
    let stem = format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}__{}",
        started.year,
        started.month,
        started.day,
        started.hour,
        started.minute,
        started.second,
        safe_address
    );
    let (file, path) = create_unique(&directory, &stem)
        .map_err(|error| format!("couldn't create chat log in {}: {error}", directory.display()))?;
    let mut writer = BufWriter::with_capacity(WRITER_BUFFER_BYTES, file);
    write_header(&mut writer, meta)
        .map_err(|error| format!("couldn't initialize {}: {error}", path.display()))?;

    println!("CHATLOG: {}", path.display());
    Ok(SessionFile {
        writer,
        path,
        dirty: true,
        messages: 0,
    })
}

fn create_unique(directory: &Path, stem: &str) -> std::io::Result<(File, PathBuf)> {
    for suffix in 0..1000_u32 {
        let name = if suffix == 0 {
            format!("{stem}.html")
        } else {
            format!("{stem}-{suffix}.html")
        };
        let path = directory.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((file, path)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "too many chat logs with the same timestamp",
    ))
}

fn write_header(out: &mut impl Write, meta: &SessionMetadata) -> std::io::Result<()> {
    const HEAD: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>DinurdoJK Chat Log</title>
<style>
:root{color-scheme:dark;background:#111318;color:#e7e9ee;font-family:system-ui,-apple-system,"Segoe UI",sans-serif}body{margin:0}.wrap{max-width:1120px;margin:0 auto;padding:28px 24px 48px}h1{font-size:24px;margin:0 0 6px}.sub{color:#9298a7;margin-bottom:20px}.meta{display:grid;grid-template-columns:max-content 1fr;gap:5px 14px;background:#181b22;border:1px solid #292e39;border-radius:10px;padding:14px 16px;margin-bottom:14px}.meta b{color:#aeb5c4}.toolbar{position:sticky;top:0;z-index:3;display:flex;flex-wrap:wrap;gap:10px 16px;align-items:center;background:#111318ee;backdrop-filter:blur(8px);padding:12px 0;border-bottom:1px solid #292e39}.toolbar label{font-size:13px;white-space:nowrap}.toolbar input[type=search]{min-width:220px;flex:1;background:#1b1f27;color:#fff;border:1px solid #343b49;border-radius:7px;padding:7px 10px}.stats{color:#9298a7;font-size:12px;margin:12px 0}.entry{display:grid;grid-template-columns:100px 72px 1fr;gap:10px;padding:5px 8px;border-radius:5px;font-family:"Cascadia Mono",Consolas,monospace;font-size:13px;line-height:1.45}.entry:nth-child(even){background:#15181e}.entry time{color:#757d8c}.badge{font:600 10px/20px system-ui;text-align:center;border-radius:4px;background:#292f3a;color:#bdc5d4;height:20px}.entry[data-kind=team] .badge{background:#263d31;color:#9de2b8}.entry[data-kind=located] .badge{background:#31374d;color:#b9c6ff}.entry[data-kind=voice] .badge{background:#4b3427;color:#ffd0a5}.entry[data-kind=session]{grid-template-columns:100px 1fr;margin:7px 0;color:#9ca5b5}.entry[data-kind=session] .session-text{border-left:2px solid #596273;padding-left:12px}.hidden{display:none!important}.no-colors .jka{color:inherit!important}.no-times time{visibility:hidden}footer{color:#737b89;font-size:12px;border-top:1px solid #292e39;margin-top:18px;padding-top:12px}
.c0{color:#555}.c1{color:#ff6262}.c2{color:#70e070}.c3{color:#f0df69}.c4{color:#7296ff}.c5{color:#63dbe6}.c6{color:#e878e8}.c7{color:#f1f1f1}.c8{color:#f39a45}.c9{color:#b6bbc5}
</style>
<script>
function applyFilters(){const q=document.getElementById('q').value.toLowerCase();const enabled=new Set([...document.querySelectorAll('[data-filter]:checked')].map(x=>x.dataset.filter));let shown=0,total=0;document.querySelectorAll('.entry').forEach(e=>{total++;const ok=enabled.has(e.dataset.kind)&&(!q||e.textContent.toLowerCase().includes(q));e.classList.toggle('hidden',!ok);if(ok)shown++;});document.getElementById('stats').textContent=`${shown} of ${total} entries shown`;document.body.classList.toggle('no-colors',!document.getElementById('colors').checked);document.body.classList.toggle('no-times',!document.getElementById('times').checked)}
addEventListener('DOMContentLoaded',()=>{document.querySelectorAll('input').forEach(x=>x.addEventListener('input',applyFilters));applyFilters()});
</script></head><body>"#;
    out.write_all(HEAD.as_bytes())?;
    writeln!(
        out,
        "<main class=\"wrap\" data-session-start-ms=\"{}\"><h1>DinurdoJK Chat Log</h1>",
        meta.started_unix_ms
    )?;
    writeln!(out, "<div class=\"sub\">Live server chat session</div>")?;
    writeln!(out, "<section class=\"meta\">")?;
    metadata_row(out, "Server", &meta.hostname)?;
    metadata_row(out, "Address", &meta.server_address.to_string())?;
    metadata_row(out, "Player", &meta.player_name)?;
    metadata_row(out, "Mod", &meta.fs_game)?;
    metadata_row(out, "Initial map", &meta.map_name)?;
    metadata_row(out, "Started", &format_local_timestamp(meta.started_unix_ms))?;
    writeln!(out, "</section>")?;
    out.write_all(br#"<div class="toolbar">
<label><input type="checkbox" data-filter="say" checked> Global</label>
<label><input type="checkbox" data-filter="team" checked> Team</label>
<label><input type="checkbox" data-filter="located" checked> Located</label>
<label><input type="checkbox" data-filter="voice" checked> Voice</label>
<label><input type="checkbox" data-filter="session" checked> Session</label>
<label><input id="colors" type="checkbox" checked> Colors</label>
<label><input id="times" type="checkbox" checked> Timestamps</label>
<input id="q" type="search" placeholder="Search messages or player names...">
</div><div id="stats" class="stats"></div><section id="transcript">"#)?;
    Ok(())
}

fn metadata_row(out: &mut impl Write, label: &str, value: &str) -> std::io::Result<()> {
    writeln!(
        out,
        "<b>{}</b><span>{}</span>",
        escape_html(label),
        escape_html(if value.is_empty() { "—" } else { value })
    )
}

fn write_message(file: &mut SessionFile, message: &ChatMessage) -> std::io::Result<()> {
    let (kind, label) = match message.kind {
        ChatKind::Say => ("say", "GLOBAL"),
        ChatKind::Team => ("team", "TEAM"),
        ChatKind::Located => ("located", "LOCATED"),
        ChatKind::Voice => ("voice", "VOICE"),
    };
    let timestamp = format_local_hms_ms(message.unix_ms);
    let server_time = message
        .server_time
        .map(|value| value.to_string())
        .unwrap_or_default();
    writeln!(
        file.writer,
        "<div class=\"entry\" data-kind=\"{kind}\" data-team=\"{}\" data-epoch-ms=\"{}\" data-server-time=\"{}\"><time>{}</time><span class=\"badge\">{label}</span><span class=\"text\">{}</span></div>",
        u8::from(message.team),
        message.unix_ms,
        server_time,
        escape_html(&timestamp),
        jka_to_html(&message.text)
    )?;
    file.messages += 1;
    file.dirty = true;
    Ok(())
}

fn write_session_event(file: &mut SessionFile, unix_ms: u64, text: &str) -> std::io::Result<()> {
    writeln!(
        file.writer,
        "<div class=\"entry\" data-kind=\"session\" data-epoch-ms=\"{unix_ms}\"><time>{}</time><span class=\"session-text\">{}</span></div>",
        escape_html(&format_local_hms_ms(unix_ms)),
        escape_html(text)
    )?;
    file.dirty = true;
    Ok(())
}

fn jka_to_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 32);
    let mut chars = text.chars().peekable();
    let mut span_open = false;
    while let Some(ch) = chars.next() {
        if ch == '^' {
            if let Some(code @ '0'..='9') = chars.peek().copied() {
                chars.next();
                if span_open {
                    out.push_str("</span>");
                }
                out.push_str("<span class=\"jka c");
                out.push(code);
                out.push_str("\">");
                span_open = true;
                continue;
            }
        }
        push_html_char(&mut out, ch);
    }
    if span_open {
        out.push_str("</span>");
    }
    out
}

fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        push_html_char(&mut out, ch);
    }
    out
}

fn push_html_char(out: &mut String, ch: char) {
    match ch {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '"' => out.push_str("&quot;"),
        '\'' => out.push_str("&#39;"),
        _ => out.push(ch),
    }
}

fn sanitize_filename_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() { "server".into() } else { out }
}

pub(crate) fn unix_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}


// -----------------------------------------------------------------------------
// Native chat-log browser support.
// -----------------------------------------------------------------------------

/// Typed transcript rows exposed to the native main-menu browser. The HTML log
/// remains the durable/shareable format; these structs are only an in-memory
/// projection used by egui.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrowserEntryKind {
    Say,
    Team,
    Located,
    Voice,
    Session,
}

impl BrowserEntryKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Say => "GLOBAL",
            Self::Team => "TEAM",
            Self::Located => "LOCATED",
            Self::Voice => "VOICE",
            Self::Session => "SESSION",
        }
    }

    fn from_attr(value: &str) -> Option<Self> {
        match value {
            "say" => Some(Self::Say),
            "team" => Some(Self::Team),
            "located" => Some(Self::Located),
            "voice" => Some(Self::Voice),
            "session" => Some(Self::Session),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct BrowserTranscriptEntry {
    pub kind: BrowserEntryKind,
    pub unix_ms: u64,
    pub timestamp: String,
    /// Restored JKA ^0..^9 escapes so the normal egui colored-text renderer can
    /// display the transcript exactly like chat/console output.
    pub text: String,
    pub plain_lower: String,
}

#[derive(Debug, Clone)]
pub(crate) struct BrowserDetail {
    pub path: PathBuf,
    pub mod_name: String,
    pub server: String,
    pub address: String,
    pub player: String,
    pub initial_map: String,
    pub started_label: String,
    pub started_unix_ms: u64,
    pub ended_unix_ms: u64,
    pub cleanly_closed: bool,
    pub entries: Vec<BrowserTranscriptEntry>,
}

#[derive(Debug, Clone)]
pub(crate) struct BrowserIndexEntry {
    pub path: PathBuf,
    pub mod_name: String,
    pub server: String,
    pub address: String,
    pub initial_map: String,
    pub started_label: String,
    pub started_date: String,
    pub ended_date: String,
    pub started_unix_ms: u64,
    pub ended_unix_ms: u64,
    pub message_count: usize,
    pub cleanly_closed: bool,
    /// Lowercase, color-stripped metadata + transcript. This makes a global
    /// search a cheap contains() on the UI thread instead of reopening files.
    pub search_text: String,
}

impl BrowserIndexEntry {
    pub(crate) fn server_key(&self) -> &str {
        if self.address.is_empty() {
            &self.server
        } else {
            &self.address
        }
    }

    pub(crate) fn server_label(&self) -> String {
        match (self.server.is_empty(), self.address.is_empty()) {
            (false, false) => format!("{} - {}", self.server, self.address),
            (false, true) => self.server.clone(),
            (true, false) => self.address.clone(),
            (true, true) => "Unknown server".to_owned(),
        }
    }
}

/// Scan every sibling game directory under GameData for `<mod>/chatlogs/**/*.html`.
/// This intentionally runs on the existing UI catalog worker, never the egui or
/// render thread. Individual corrupt/foreign HTML files are skipped while valid
/// DinurdoJK logs continue to appear.
pub(crate) fn scan_browser_index(base: &Path) -> Result<Vec<BrowserIndexEntry>, String> {
    let game_root = base.parent().unwrap_or(base);
    let dirs = fs::read_dir(game_root)
        .map_err(|error| format!("could not scan {}: {error}", game_root.display()))?;
    let mut logs = Vec::new();

    for game_dir in dirs.flatten() {
        let game_path = game_dir.path();
        if !game_path.is_dir() {
            continue;
        }
        let chatlogs = game_path.join("chatlogs");
        if !chatlogs.is_dir() {
            continue;
        }
        let mod_name = game_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "base".to_owned());
        let mut files = Vec::new();
        collect_html_files(&chatlogs, &mut files, 0);
        for path in files {
            match parse_browser_detail(&path, Some(&mod_name)) {
                Ok(detail) => logs.push(index_from_detail(detail)),
                Err(error) => devprintln!(2, "Chat log browser skipped {}: {error}", path.display()),
            }
        }
    }

    logs.sort_by(|a, b| {
        b.started_unix_ms
            .cmp(&a.started_unix_ms)
            .then_with(|| b.path.cmp(&a.path))
    });
    Ok(logs)
}

pub(crate) fn read_browser_detail(path: &Path) -> Result<BrowserDetail, String> {
    parse_browser_detail(path, None)
}

fn collect_html_files(directory: &Path, out: &mut Vec<PathBuf>, depth: u8) {
    // Generated logs are chatlogs/YYYY-MM/file.html. Allow a little room for
    // future organization, but never follow symlinks or recurse without bound.
    if depth > 3 {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else { return };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else { continue };
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_html_files(&path, out, depth + 1);
        } else if file_type.is_file()
            && path
                .extension()
                .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case("html"))
        {
            out.push(path);
        }
    }
}

fn index_from_detail(detail: BrowserDetail) -> BrowserIndexEntry {
    let mut search = String::new();
    for value in [
        detail.mod_name.as_str(),
        detail.server.as_str(),
        detail.address.as_str(),
        detail.player.as_str(),
        detail.initial_map.as_str(),
        detail.started_label.as_str(),
    ] {
        if !value.is_empty() {
            search.push_str(value);
            search.push('\n');
        }
    }
    let message_count = detail
        .entries
        .iter()
        .filter(|entry| entry.kind != BrowserEntryKind::Session)
        .count();
    for entry in &detail.entries {
        search.push_str(&crate::logging::strip_jka_colors(&entry.text));
        search.push('\n');
    }
    let started_date = format_local_date(detail.started_unix_ms);
    let ended_date = format_local_date(detail.ended_unix_ms);
    BrowserIndexEntry {
        path: detail.path,
        mod_name: detail.mod_name,
        server: detail.server,
        address: detail.address,
        initial_map: detail.initial_map,
        started_label: detail.started_label,
        started_date,
        ended_date,
        started_unix_ms: detail.started_unix_ms,
        ended_unix_ms: detail.ended_unix_ms,
        message_count,
        cleanly_closed: detail.cleanly_closed,
        search_text: search.to_lowercase(),
    }
}

fn parse_browser_detail(path: &Path, mod_hint: Option<&str>) -> Result<BrowserDetail, String> {
    let html = fs::read_to_string(path)
        .map_err(|error| format!("could not read chat log: {error}"))?;
    if !html.contains("DinurdoJK Chat Log") {
        return Err("not a DinurdoJK chat log".to_owned());
    }

    let mod_name = metadata_value(&html, "Mod")
        .filter(|value| !value.is_empty() && value != "—")
        .or_else(|| mod_hint.map(str::to_owned))
        .unwrap_or_else(|| "base".to_owned());
    let server = clean_metadata_value(metadata_value(&html, "Server"));
    let address = clean_metadata_value(metadata_value(&html, "Address"));
    let player = clean_metadata_value(metadata_value(&html, "Player"));
    let initial_map = clean_metadata_value(metadata_value(&html, "Initial map"));
    let started_label = clean_metadata_value(metadata_value(&html, "Started"));
    let entries = parse_transcript_entries(&html);

    let started_unix_ms = attribute_after(&html, "data-session-start-ms=\"")
        .ok_or_else(|| "chat log is missing session-start metadata".to_owned())?
        .parse::<u64>()
        .map_err(|_| "chat log has invalid session-start metadata".to_owned())?;
    let ended_unix_ms = entries
        .last()
        .map(|entry| entry.unix_ms)
        .unwrap_or(started_unix_ms);

    Ok(BrowserDetail {
        path: path.to_path_buf(),
        mod_name,
        server,
        address,
        player,
        initial_map,
        started_label,
        started_unix_ms,
        ended_unix_ms,
        cleanly_closed: html.contains("</main></body></html>"),
        entries,
    })
}

fn clean_metadata_value(value: Option<String>) -> String {
    value
        .filter(|value| value != "—")
        .unwrap_or_default()
}

fn metadata_value(html: &str, label: &str) -> Option<String> {
    let marker = format!("<b>{label}</b><span>");
    let start = html.find(&marker)? + marker.len();
    let rest = &html[start..];
    let end = rest.find("</span>")?;
    Some(decode_html_fragment(&rest[..end], false))
}

fn parse_transcript_entries(html: &str) -> Vec<BrowserTranscriptEntry> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(relative) = html[cursor..].find("<div class=\"entry\"") {
        let start = cursor + relative;
        let Some(open_relative_end) = html[start..].find('>') else { break };
        let open_end = start + open_relative_end;
        let open_tag = &html[start..=open_end];
        let Some(kind_name) = attribute_after(open_tag, "data-kind=\"") else {
            cursor = open_end + 1;
            continue;
        };
        let Some(kind) = BrowserEntryKind::from_attr(kind_name) else {
            cursor = open_end + 1;
            continue;
        };
        let unix_ms = attribute_after(open_tag, "data-epoch-ms=\"")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        let rest = &html[open_end + 1..];
        let Some(close_relative) = rest.find("</div>") else { break };
        let body = &rest[..close_relative];
        let timestamp = extract_tag_text(body, "time").unwrap_or_default();
        let class = if kind == BrowserEntryKind::Session {
            "session-text"
        } else {
            "text"
        };
        let text = extract_span_body(body, class)
            .map(|fragment| decode_html_fragment(fragment, true))
            .unwrap_or_default();
        let plain_lower = crate::logging::strip_jka_colors(&text).to_lowercase();
        out.push(BrowserTranscriptEntry {
            kind,
            unix_ms,
            timestamp,
            text,
            plain_lower,
        });
        cursor = open_end + 1 + close_relative + "</div>".len();
    }
    out
}

/// Return the quoted attribute value immediately following `needle`.
fn attribute_after<'a>(text: &'a str, needle: &str) -> Option<&'a str> {
    let start = text.find(needle)? + needle.len();
    let rest = &text[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

fn extract_tag_text(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(decode_html_fragment(&body[start..end], false))
}

fn extract_span_body<'a>(body: &'a str, class: &str) -> Option<&'a str> {
    let marker = format!("<span class=\"{class}\">");
    let start = body.find(&marker)? + marker.len();
    // Message bodies may contain nested JKA color spans, so the matching outer
    // span is the last </span> in this entry, not the first one.
    let end = body.rfind("</span>")?;
    (end >= start).then_some(&body[start..end])
}

fn decode_html_fragment(fragment: &str, restore_jka_colors: bool) -> String {
    let mut out = String::with_capacity(fragment.len());
    let mut cursor = 0usize;
    while cursor < fragment.len() {
        let Some(tag_relative) = fragment[cursor..].find('<') else {
            push_decoded_entities(&mut out, &fragment[cursor..]);
            break;
        };
        let tag_start = cursor + tag_relative;
        push_decoded_entities(&mut out, &fragment[cursor..tag_start]);
        let Some(tag_end_relative) = fragment[tag_start..].find('>') else {
            push_decoded_entities(&mut out, &fragment[tag_start..]);
            break;
        };
        let tag_end = tag_start + tag_end_relative;
        if restore_jka_colors {
            let tag = &fragment[tag_start..=tag_end];
            if let Some(class_pos) = tag.find("class=\"jka c") {
                let code_pos = class_pos + "class=\"jka c".len();
                if let Some(code) = tag[code_pos..].chars().next().filter(|ch| ch.is_ascii_digit()) {
                    out.push('^');
                    out.push(code);
                }
            }
        }
        cursor = tag_end + 1;
    }
    out
}

fn push_decoded_entities(out: &mut String, text: &str) {
    let mut cursor = 0usize;
    while cursor < text.len() {
        let Some(relative) = text[cursor..].find('&') else {
            out.push_str(&text[cursor..]);
            break;
        };
        let start = cursor + relative;
        out.push_str(&text[cursor..start]);
        let Some(end_relative) = text[start..].find(';') else {
            out.push_str(&text[start..]);
            break;
        };
        let end = start + end_relative;
        match &text[start..=end] {
            "&amp;" => out.push('&'),
            "&lt;" => out.push('<'),
            "&gt;" => out.push('>'),
            "&quot;" => out.push('"'),
            "&#39;" => out.push('\''),
            entity => out.push_str(entity),
        }
        cursor = end + 1;
    }
}

pub(crate) fn format_local_date(unix_ms: u64) -> String {
    let time = local_datetime_parts(unix_ms);
    format!("{:04}-{:02}-{:02}", time.year, time.month, time.day)
}

#[derive(Clone, Copy)]
struct DateTimeParts {
    year: u16,
    month: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    millis: u16,
}

fn format_local_timestamp(unix_ms: u64) -> String {
    let time = local_datetime_parts(unix_ms);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        time.year, time.month, time.day, time.hour, time.minute, time.second, time.millis
    )
}

fn format_local_hms_ms(unix_ms: u64) -> String {
    let time = local_datetime_parts(unix_ms);
    format!("{:02}:{:02}:{:02}.{:03}", time.hour, time.minute, time.second, time.millis)
}

#[cfg(windows)]
fn local_datetime_parts(unix_ms: u64) -> DateTimeParts {
    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[repr(C)]
    struct WinSystemTime {
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
        fn FileTimeToSystemTime(file_time: *const FileTime, system_time: *mut WinSystemTime) -> i32;
        fn SystemTimeToTzSpecificLocalTime(
            timezone: *const core::ffi::c_void,
            universal_time: *const WinSystemTime,
            local_time: *mut WinSystemTime,
        ) -> i32;
    }

    const WINDOWS_EPOCH_MS: u64 = 11_644_473_600_000;
    let ticks = unix_ms
        .saturating_add(WINDOWS_EPOCH_MS)
        .saturating_mul(10_000);
    let file_time = FileTime {
        low: ticks as u32,
        high: (ticks >> 32) as u32,
    };
    let mut utc = std::mem::MaybeUninit::<WinSystemTime>::uninit();
    let mut local = std::mem::MaybeUninit::<WinSystemTime>::uninit();
    unsafe {
        if FileTimeToSystemTime(&file_time, utc.as_mut_ptr()) != 0 {
            let utc = utc.assume_init();
            if SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, local.as_mut_ptr()) != 0 {
                let local = local.assume_init();
                return DateTimeParts {
                    year: local.year,
                    month: local.month,
                    day: local.day,
                    hour: local.hour,
                    minute: local.minute,
                    second: local.second,
                    millis: local.milliseconds,
                };
            }
        }
    }
    utc_datetime_parts(unix_ms)
}

#[cfg(not(windows))]
fn local_datetime_parts(unix_ms: u64) -> DateTimeParts {
    // std has no dependency-free local timezone conversion. Windows is the
    // shipping target; other platforms retain exact UTC timestamps.
    utc_datetime_parts(unix_ms)
}

fn utc_datetime_parts(unix_ms: u64) -> DateTimeParts {
    let seconds = unix_ms / 1000;
    let days = (seconds / 86_400) as i64;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    DateTimeParts {
        year: year as u16,
        month: month as u16,
        day: day as u16,
        hour: (seconds_of_day / 3_600) as u16,
        minute: ((seconds_of_day / 60) % 60) as u16,
        second: (seconds_of_day % 60) as u16,
        millis: (unix_ms % 1000) as u16,
    }
}

// Howard Hinnant's civil_from_days, with day 0 = 1970-01-01.
fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    y += if m <= 2 { 1 } else { 0 };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_chat_and_preserves_jka_colors_as_spans() {
        let html = jka_to_html("^1Red <script>^7 & safe");
        assert!(html.contains("<span class=\"jka c1\">Red &lt;script&gt;</span>"));
        assert!(html.contains("<span class=\"jka c7\"> &amp; safe</span>"));
        assert!(!html.contains("<script>"));
    }

    #[test]
    fn unix_epoch_calendar_conversion_is_stable() {
        let parts = utc_datetime_parts(0);
        assert_eq!((parts.year, parts.month, parts.day), (1970, 1, 1));
        let parts = utc_datetime_parts(1_767_225_599_999); // 2025-12-31 23:59:59.999 UTC
        assert_eq!((parts.year, parts.month, parts.day), (2025, 12, 31));
        assert_eq!((parts.hour, parts.minute, parts.second, parts.millis), (23, 59, 59, 999));
    }
}
