//! Model files: URL rules, the on-disk cache and a resumable download.
//!
//! Only GGUF model data is ever fetched, never code. A download streams to
//! `<name>.part` and is renamed once complete, so a cached file is always a
//! whole one; an interrupted download resumes from the part file.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

/// Largest model file accepted. Chat models this client targets are 1-3 GB.
pub const MAX_MODEL_BYTES: u64 = 16 * 1024 * 1024 * 1024;
/// Space left on the drive after a download, so the game can still write.
const DISK_RESERVE_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    /// 0 until the server reports a size.
    pub total: AtomicU64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelUrl {
    /// Normalized address the file is fetched from.
    pub url: String,
    pub file_name: String,
    secure: bool,
    host: String,
    port: u16,
    path: String,
}

/// Accepts an `https://` link to a `.gguf` file. Hugging Face page links
/// (`/blob/`) are turned into their download form (`/resolve/`). Plain
/// `http://` is allowed for this machine only.
pub fn parse_url(input: &str) -> Result<ModelUrl, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("no model address set".to_owned());
    }
    if input.bytes().any(|byte| byte <= 0x20 || byte == 0x7f) || input.contains('\\') || input.contains('"') {
        return Err("model address contains spaces or unsafe characters".to_owned());
    }
    let (secure, rest, default_port) = if let Some(rest) = input.strip_prefix("https://") {
        (true, rest, 443)
    } else if let Some(rest) = input.strip_prefix("http://") {
        (false, rest, 80)
    } else {
        return Err("model address must start with https://".to_owned());
    };
    let rest = rest.split('#').next().unwrap_or(rest);
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    if authority.contains('@') {
        return Err("model address may not contain credentials".to_owned());
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => {
            (host, port.parse::<u16>().map_err(|_| "model address has an invalid port".to_owned())?)
        }
        _ => (authority, default_port),
    };
    let host = host.to_ascii_lowercase();
    if host.is_empty() || !host.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')) {
        return Err("model address has an invalid host".to_owned());
    }
    if !secure && !matches!(host.as_str(), "localhost" | "127.0.0.1") {
        return Err("model address must use https://".to_owned());
    }
    let mut path = path.to_owned();
    if host == "huggingface.co" || host.ends_with(".huggingface.co") || host == "hf.co" {
        // The page link people copy from the browser is not the file itself.
        path = path.replacen("/blob/", "/resolve/", 1);
    }
    let file_part = path.split('?').next().unwrap_or(&path);
    let file_name = file_part.rsplit('/').next().unwrap_or("");
    if !file_name.to_ascii_lowercase().ends_with(".gguf") {
        return Err("model address must point to a .gguf file".to_owned());
    }
    let file_name: String = file_name
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') { ch } else { '_' })
        .take(96)
        .collect();
    let scheme = if secure { "https" } else { "http" };
    let shown_port = if port == default_port { String::new() } else { format!(":{port}") };
    Ok(ModelUrl { url: format!("{scheme}://{host}{shown_port}{path}"), file_name, secure, host, port, path })
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Where a model lives in the cache. The address is part of the name, so two
/// repositories with the same file name do not collide.
pub fn cache_path(cache_dir: &Path, model: &ModelUrl) -> PathBuf {
    cache_dir.join(format!("{:016x}-{}", fnv1a64(model.url.as_bytes()), model.file_name))
}

fn part_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    target.with_file_name(name)
}

pub fn is_gguf(path: &Path) -> bool {
    use std::io::Read;
    let mut magic = [0u8; 4];
    fs::File::open(path).and_then(|mut file| file.read_exact(&mut magic)).is_ok() && &magic == b"GGUF"
}

/// Fetch `model` into `target`. Returns `Ok(false)` when `cancel` was set;
/// the part file is kept so a later attempt continues from it.
pub fn download(model: &ModelUrl, target: &Path, progress: &Progress, cancel: &AtomicBool) -> Result<bool, String> {
    let directory = target.parent().ok_or("model cache has no directory")?;
    fs::create_dir_all(directory).map_err(|error| format!("could not create the model cache: {error}"))?;
    let part = part_path(target);
    for attempt in 0..2 {
        let have = fs::metadata(&part).map_or(0, |meta| meta.len());
        progress.done.store(have, Ordering::Relaxed);
        match platform::fetch(model, &part, have, progress, cancel, directory)? {
            platform::Fetch::Complete => break,
            platform::Fetch::Cancelled => return Ok(false),
            // The server refused the resume point: start over once.
            platform::Fetch::Restart if attempt == 0 => {
                let _ = fs::remove_file(&part);
            }
            platform::Fetch::Restart => return Err(format!("{} would not resume the download", model.host)),
        }
    }
    if !is_gguf(&part) {
        let _ = fs::remove_file(&part);
        return Err(format!("{} did not send a GGUF model file", model.host));
    }
    let _ = fs::remove_file(target);
    fs::rename(&part, target).map_err(|error| format!("could not store the model: {error}"))?;
    Ok(true)
}

/// Remove a cached model and any partial download of it.
pub fn remove(target: &Path) -> Result<(), String> {
    let _ = fs::remove_file(part_path(target));
    match fs::remove_file(target) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("could not delete the model file: {error}")),
    }
}

fn append(part: &Path, restart: bool) -> Result<fs::File, String> {
    let mut options = fs::OpenOptions::new();
    options.create(true);
    if restart {
        options.write(true).truncate(true);
    } else {
        options.append(true);
    }
    options.open(part).map_err(|error| format!("could not write the model file: {error}"))
}

fn write_chunk(file: &mut fs::File, chunk: &[u8]) -> Result<(), String> {
    file.write_all(chunk).map_err(|error| format!("could not write the model file: {error}"))
}

#[cfg(windows)]
mod platform {
    use super::{append, write_chunk, ModelUrl, Progress, DISK_RESERVE_BYTES, MAX_MODEL_BYTES};
    use std::{
        ffi::c_void,
        os::windows::ffi::OsStrExt,
        path::Path,
        ptr,
        sync::atomic::{AtomicBool, Ordering},
    };

    pub(super) enum Fetch {
        Complete,
        Cancelled,
        Restart,
    }

    type HInternet = *mut c_void;
    const WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY: u32 = 4;
    const WINHTTP_FLAG_SECURE: u32 = 0x0080_0000;
    const WINHTTP_QUERY_STATUS_CODE: u32 = 19;
    const WINHTTP_QUERY_CONTENT_LENGTH: u32 = 5;
    const WINHTTP_QUERY_FLAG_NUMBER: u32 = 0x2000_0000;
    const WINHTTP_QUERY_FLAG_NUMBER64: u32 = 0x0800_0000;

    #[link(name = "winhttp")]
    unsafe extern "system" {
        fn WinHttpOpen(agent: *const u16, access: u32, proxy: *const u16, bypass: *const u16, flags: u32) -> HInternet;
        fn WinHttpConnect(session: HInternet, server: *const u16, port: u16, reserved: u32) -> HInternet;
        fn WinHttpOpenRequest(
            connect: HInternet,
            verb: *const u16,
            object: *const u16,
            version: *const u16,
            referrer: *const u16,
            accept: *const *const u16,
            flags: u32,
        ) -> HInternet;
        fn WinHttpSetTimeouts(handle: HInternet, resolve: i32, connect: i32, send: i32, receive: i32) -> i32;
        fn WinHttpSendRequest(
            request: HInternet,
            headers: *const u16,
            headers_length: u32,
            optional: *mut c_void,
            optional_length: u32,
            total_length: u32,
            context: usize,
        ) -> i32;
        fn WinHttpReceiveResponse(request: HInternet, reserved: *mut c_void) -> i32;
        fn WinHttpQueryHeaders(
            request: HInternet,
            info: u32,
            name: *const u16,
            buffer: *mut c_void,
            length: *mut u32,
            index: *mut u32,
        ) -> i32;
        fn WinHttpReadData(request: HInternet, buffer: *mut c_void, to_read: u32, read: *mut u32) -> i32;
        fn WinHttpCloseHandle(handle: HInternet) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDiskFreeSpaceExW(directory: *const u16, available: *mut u64, total: *mut u64, free: *mut u64) -> i32;
    }

    struct Handle(HInternet);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { WinHttpCloseHandle(self.0) };
            }
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn free_bytes(directory: &Path) -> Option<u64> {
        let wide: Vec<u16> = directory.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut available = 0u64;
        let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, ptr::null_mut(), ptr::null_mut()) };
        (ok != 0).then_some(available)
    }

    fn query_number(request: HInternet, info: u32) -> Option<u64> {
        let mut value = 0u64;
        let mut length = std::mem::size_of::<u64>() as u32;
        let ok = unsafe {
            WinHttpQueryHeaders(
                request,
                info | WINHTTP_QUERY_FLAG_NUMBER64,
                ptr::null(),
                (&mut value as *mut u64).cast(),
                &mut length,
                ptr::null_mut(),
            )
        };
        (ok != 0).then_some(value)
    }

    pub(super) fn fetch(
        model: &ModelUrl,
        part: &Path,
        have: u64,
        progress: &Progress,
        cancel: &AtomicBool,
        directory: &Path,
    ) -> Result<Fetch, String> {
        let host_name = &model.host;
        let agent = wide("DinurdoJK/0.1 model-download");
        let session = Handle(unsafe {
            WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, ptr::null(), ptr::null(), 0)
        });
        if session.0.is_null() {
            return Err("WinHTTP open failed".to_owned());
        }
        // Redirects stay on: Hugging Face hands file downloads to its CDN.
        // WinHTTP's default policy already refuses an https -> http redirect.
        unsafe { WinHttpSetTimeouts(session.0, 15_000, 15_000, 30_000, 60_000) };
        let host = wide(host_name);
        let connect = Handle(unsafe { WinHttpConnect(session.0, host.as_ptr(), model.port, 0) });
        if connect.0.is_null() {
            return Err(format!("could not connect to {host_name}"));
        }
        let verb = wide("GET");
        let path = wide(&model.path);
        let request = Handle(unsafe {
            WinHttpOpenRequest(
                connect.0,
                verb.as_ptr(),
                path.as_ptr(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                if model.secure { WINHTTP_FLAG_SECURE } else { 0 },
            )
        });
        if request.0.is_null() {
            return Err("WinHTTP request creation failed".to_owned());
        }
        let mut headers = String::from("Accept-Encoding: identity\r\n");
        if have > 0 {
            headers.push_str(&format!("Range: bytes={have}-\r\n"));
        }
        let headers = wide(&headers);
        let sent = unsafe { WinHttpSendRequest(request.0, headers.as_ptr(), u32::MAX, ptr::null_mut(), 0, 0, 0) };
        if sent == 0 || unsafe { WinHttpReceiveResponse(request.0, ptr::null_mut()) } == 0 {
            return Err(format!("no response from {host_name}"));
        }
        let mut status = 0u32;
        let mut status_len = std::mem::size_of::<u32>() as u32;
        if unsafe {
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                ptr::null(),
                (&mut status as *mut u32).cast(),
                &mut status_len,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(format!("unreadable response from {host_name}"));
        }
        let resumed = match status {
            206 if have > 0 => true,
            200 => false,
            416 if have > 0 => return Ok(Fetch::Restart),
            401 | 403 => {
                return Err(format!(
                    "{host_name} refused the download (HTTP {status}): the model is private or needs a licence accepted on its page"
                ))
            }
            404 => return Err(format!("{host_name} has no such model file (HTTP 404)")),
            other => return Err(format!("{host_name} answered HTTP {other}")),
        };
        let start = if resumed { have } else { 0 };
        let remaining = query_number(request.0, WINHTTP_QUERY_CONTENT_LENGTH).unwrap_or(0);
        let total = if remaining > 0 { start + remaining } else { 0 };
        if total > MAX_MODEL_BYTES {
            return Err(format!("the model file is larger than {} GB", MAX_MODEL_BYTES >> 30));
        }
        if let Some(free) = free_bytes(directory) {
            if remaining.saturating_add(DISK_RESERVE_BYTES) > free {
                return Err(format!(
                    "not enough disk space for the model: {:.1} GB needed, {:.1} GB free",
                    (remaining + DISK_RESERVE_BYTES) as f64 / 1e9,
                    free as f64 / 1e9
                ));
            }
        }
        progress.total.store(total, Ordering::Relaxed);
        progress.done.store(start, Ordering::Relaxed);
        let mut file = append(part, !resumed)?;
        let mut written = start;
        let mut buffer = vec![0u8; 256 * 1024];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Ok(Fetch::Cancelled);
            }
            let mut read = 0u32;
            if unsafe { WinHttpReadData(request.0, buffer.as_mut_ptr().cast(), buffer.len() as u32, &mut read) } == 0 {
                return Err(format!("the download from {host_name} was cut off"));
            }
            if read == 0 {
                break;
            }
            written += u64::from(read);
            if written > MAX_MODEL_BYTES {
                return Err(format!("the model file is larger than {} GB", MAX_MODEL_BYTES >> 30));
            }
            write_chunk(&mut file, &buffer[..read as usize])?;
            progress.done.store(written, Ordering::Relaxed);
        }
        if total > 0 && written != total {
            return Err(format!("the download from {host_name} ended early"));
        }
        file.sync_all().map_err(|error| format!("could not write the model file: {error}"))?;
        Ok(Fetch::Complete)
    }
}

#[cfg(not(windows))]
mod platform {
    use super::{ModelUrl, Progress};
    use std::{path::Path, sync::atomic::AtomicBool};

    pub(super) enum Fetch {
        Complete,
        Cancelled,
        Restart,
    }

    pub(super) fn fetch(
        _model: &ModelUrl,
        _part: &Path,
        _have: u64,
        _progress: &Progress,
        _cancel: &AtomicBool,
        _directory: &Path,
    ) -> Result<Fetch, String> {
        let _ = (Fetch::Complete, Fetch::Cancelled, Fetch::Restart, super::append, super::write_chunk);
        Err("model downloads need Windows WinHTTP in this build".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hugging_face_page_links_become_download_links() {
        let model = parse_url("https://huggingface.co/tencent/Hy-MT2-1.8B-GGUF/blob/main/Hy-MT2-1.8B-Q4_K_M.gguf").unwrap();
        assert_eq!(model.url, "https://huggingface.co/tencent/Hy-MT2-1.8B-GGUF/resolve/main/Hy-MT2-1.8B-Q4_K_M.gguf");
        assert_eq!(model.file_name, "Hy-MT2-1.8B-Q4_K_M.gguf");
        let direct = parse_url("https://huggingface.co/a/b/resolve/main/m.gguf?download=true").unwrap();
        assert_eq!(direct.file_name, "m.gguf");
        assert!(direct.path.ends_with("?download=true"));
    }

    #[test]
    fn only_https_gguf_links_are_accepted() {
        assert!(parse_url("").is_err());
        assert!(parse_url("ftp://host/m.gguf").is_err());
        assert!(parse_url("http://example.com/m.gguf").is_err());
        assert!(parse_url("http://localhost:8080/m.gguf").is_ok());
        assert!(parse_url("https://user:pw@host/m.gguf").is_err());
        assert!(parse_url("https://host/model.exe").is_err());
        assert!(parse_url("https://host/a b.gguf").is_err());
        assert!(parse_url("https://ho st/m.gguf").is_err());
    }

    #[test]
    fn cache_names_depend_on_the_whole_address() {
        let dir = Path::new("models");
        let a = cache_path(dir, &parse_url("https://huggingface.co/a/x/resolve/main/m.gguf").unwrap());
        let b = cache_path(dir, &parse_url("https://huggingface.co/b/x/resolve/main/m.gguf").unwrap());
        assert_ne!(a, b);
        assert!(a.file_name().unwrap().to_string_lossy().ends_with("-m.gguf"));
        assert_eq!(part_path(&a).extension().unwrap(), "part");
    }
}
