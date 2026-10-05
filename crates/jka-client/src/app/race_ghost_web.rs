//! Remote race-ghost catalog/download support.
//!
//! The public race archive is intentionally treated as data, not as a web UI:
//! we read the per-map index JSON, then the selected course/style JSON. Course
//! start AABBs drive the in-game course suggestion, while `demo_url` entries are
//! resolved only against the configured archive origin. Network and disk work
//! always runs on App-owned worker threads.

use jka_protocol::{
    demo::DemoReader,
    server::{Decoder as ServerMessageDecoder, Event as ServerMessageEvent},
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Cursor,
    path::Path,
};
#[cfg(not(windows))]
use std::{io::{Read, Write}, net::TcpStream, time::Duration};

const MAX_JSON_BYTES: usize = 8 * 1024 * 1024;
const MAX_DEMO_BYTES: usize = 128 * 1024 * 1024;
#[cfg(not(windows))]
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct RemoteAabb {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct RemoteCourse {
    pub course: String,
    pub coursename: String,
    pub start_aabbs: Vec<RemoteAabb>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RemoteDemo {
    pub url: String,
    pub label: String,
}

#[derive(Debug)]
pub(super) enum WebResult {
    Courses {
        generation: u64,
        map: String,
        style: String,
        result: Result<Vec<RemoteCourse>, String>,
    },
    Demos {
        generation: u64,
        map: String,
        style: String,
        course: String,
        result: Result<Vec<RemoteDemo>, String>,
    },
}

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedUrl {
    secure: bool,
    host: String,
    port: u16,
    path: String,
}

pub(super) fn normalize_base_url(value: &str) -> Result<String, String> {
    let trimmed = value.trim().trim_matches('"').trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("demo base URL is empty".to_owned());
    }
    let mut url = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!("http://{trimmed}")
    };
    let parsed = parse_url(&url)?;
    if parsed.path.trim_matches('/').is_empty() {
        url = url.trim_end_matches('/').to_owned();
    }
    if url.to_ascii_lowercase().ends_with("/index") {
        url.truncate(url.len() - "/index".len());
    }
    Ok(url.trim_end_matches('/').to_owned())
}

pub(super) fn fetch_courses(
    base_url: &str,
    map: &str,
    style_id: i32,
    style_name: &str,
) -> Result<Vec<RemoteCourse>, String> {
    let url = format!(
        "{}/index/{}/index.json",
        normalize_base_url(base_url)?,
        encode_segment(map)
    );
    let value = fetch_json(&url)?;
    let indexes = value
        .get("indexes")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("map catalog {url} has no indexes array"))?;

    // The archive exposes one entry per course/style. Match the numeric jaPRO
    // STAT_MOVEMENTSTYLE value directly; style_name is only presentation/error
    // text and is not a second source of truth for movement prediction.
    let mut courses = BTreeMap::<String, RemoteCourse>::new();
    for entry in indexes {
        let Some(object) = entry.as_object() else { continue };
        if object
            .get("map")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.eq_ignore_ascii_case(map))
        {
            continue;
        }
        if object.get("style").and_then(Value::as_i64) != Some(i64::from(style_id)) {
            continue;
        }
        let Some(course) = object.get("course").and_then(Value::as_str) else { continue };
        let coursename = object
            .get("coursename")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| {
                if course.is_empty() {
                    map.to_owned()
                } else {
                    format!("{map} ({course})")
                }
            });
        courses.entry(course.to_owned()).or_insert_with(|| RemoteCourse {
            course: course.to_owned(),
            coursename,
            start_aabbs: parse_start_aabbs(object),
        });
    }
    if courses.is_empty() {
        return Err(format!("no {style_name} courses were found for {map}"));
    }
    Ok(courses.into_values().collect())
}

pub(super) fn fetch_demos(
    base_url: &str,
    map: &str,
    course: &str,
    style: &str,
) -> Result<Vec<RemoteDemo>, String> {
    let stem = if course.is_empty() {
        format!("-{}", encode_segment(style))
    } else {
        format!("{}-{}", encode_segment(course), encode_segment(style))
    };
    let base_url = normalize_base_url(base_url)?;
    let base_origin = parse_url(&base_url)?;
    let url = format!(
        "{}/index/{}/{}.json",
        base_url,
        encode_segment(map),
        stem
    );
    let value = fetch_json(&url)?;
    let courses = value
        .get("courses")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("course catalog {url} has no courses array"))?;

    let mut seen = BTreeSet::new();
    let mut demos = Vec::new();
    for course_entry in courses {
        let Some(object) = course_entry.as_object() else { continue };
        if object
            .get("map")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.eq_ignore_ascii_case(map))
        {
            continue;
        }
        if object.get("course").and_then(Value::as_str) != Some(course) {
            continue;
        }
        let Some(results) = object.get("results").and_then(Value::as_array) else { continue };
        for result in results {
            let Some(result) = result.as_object() else { continue };
            let Some(raw_url) = result.get("demo_url").and_then(Value::as_str) else { continue };
            let Ok(absolute_url) = resolve_demo_url(raw_url, &base_origin) else { continue };
            let Ok(parsed) = parse_url(&absolute_url) else { continue };
            if validate_demo_url(&parsed, &base_origin).is_err() {
                continue;
            }
            if !seen.insert(absolute_url.clone()) {
                continue;
            }
            let label = result
                .get("username")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .map(|name| name.chars().take(128).collect::<String>())
                .unwrap_or_else(|| demo_basename(&absolute_url));
            demos.push(RemoteDemo { url: absolute_url, label });
        }
    }
    if demos.is_empty() {
        return Err("this course/style has no downloadable demos".to_owned());
    }
    Ok(demos)
}

pub(super) fn nearest_course(courses: &[RemoteCourse], point: [f32; 3]) -> Option<&RemoteCourse> {
    let point = point.map(f64::from);
    let mut best: Option<(&RemoteCourse, bool, f64, f64)> = None;
    for course in courses {
        let mut course_best: Option<(bool, f64, f64)> = None;
        for aabb in &course.start_aabbs {
            let distance_sq = point_aabb_distance_sq(point, *aabb);
            let inside = distance_sq == 0.0;
            let volume = aabb_volume(*aabb);
            let replace = match course_best {
                None => true,
                Some((best_inside, best_distance, best_volume)) => {
                    (inside && !best_inside)
                        || (inside == best_inside
                            && if inside {
                                volume < best_volume
                            } else {
                                distance_sq < best_distance
                                    || (distance_sq == best_distance && volume < best_volume)
                            })
                }
            };
            if replace {
                course_best = Some((inside, distance_sq, volume));
            }
        }
        let Some((inside, distance_sq, volume)) = course_best else { continue };
        let replace = match best {
            None => true,
            Some((_, best_inside, best_distance, best_volume)) => {
                (inside && !best_inside)
                    || (inside == best_inside
                        && if inside {
                            volume < best_volume
                        } else {
                            distance_sq < best_distance
                                || (distance_sq == best_distance && volume < best_volume)
                        })
            }
        };
        if replace {
            best = Some((course, inside, distance_sq, volume));
        }
    }
    best.map(|(course, _, _, _)| course)
}

pub(super) fn fetch_demo_cached(base_url: &str, url: &str, cache_root: &Path) -> Result<Vec<u8>, String> {
    // Catalog root-relative URLs are resolved before reaching this layer. The
    // cache/download path always receives a complete, already-origin-bound URL.
    // Re-validate it anyway: untrusted catalog data must never turn the game
    // into an arbitrary HTTP client.
    let base_url = normalize_base_url(base_url)?;
    let base = parse_url(&base_url)?;
    let parsed = parse_url(url)?;
    validate_demo_url(&parsed, &base)?;
    fs::create_dir_all(cache_root)
        .map_err(|error| format!("create ghost cache {}: {error}", cache_root.display()))?;

    let hash = fnv1a64(url.as_bytes());
    let basename = demo_basename(url);
    let cache_path = cache_root.join(format!("{}-{hash:016x}.dm_26", sanitize_filename(&basename)));
    let meta_path = cache_path.with_extension("dm_26.meta.json");
    let meta = read_cache_meta(&meta_path);
    let metadata_matches_url = meta.get("url").and_then(Value::as_str) == Some(url);
    let mut cached_bytes = None;
    if cache_path.is_file() && metadata_matches_url {
        match fs::read(&cache_path) {
            Ok(bytes) if validate_demo_bytes(&bytes).is_ok() => cached_bytes = Some(bytes),
            Ok(_) | Err(_) => {
                // A partial/corrupt cache entry is never trusted simply because
                // its filename ends in .dm_26. Remove it and fetch a fresh copy.
                let _ = fs::remove_file(&cache_path);
                let _ = fs::remove_file(&meta_path);
            }
        }
    } else if cache_path.is_file() {
        // The URL hash is only a cache key, not a security boundary. If metadata
        // does not bind this file to the exact URL, discard it rather than risk
        // a collision/stale rename being treated as the requested demo.
        let _ = fs::remove_file(&cache_path);
        let _ = fs::remove_file(&meta_path);
    }

    let mut conditional = Vec::<(String, String)>::new();
    if cached_bytes.is_some() {
        if let Some(etag) = meta
            .get("etag")
            .and_then(Value::as_str)
            .filter(|value| valid_header_value(value))
        {
            conditional.push(("If-None-Match".to_owned(), etag.to_owned()));
        } else if let Some(modified) = meta
            .get("last_modified")
            .and_then(Value::as_str)
            .filter(|value| valid_header_value(value))
        {
            conditional.push(("If-Modified-Since".to_owned(), modified.to_owned()));
        } else {
            // Static hosts do not always provide validators. A previously
            // protocol-validated cache entry is still preferable to downloading
            // the same race every click; deleting cache/race_ghosts forces a
            // refresh in that case.
            return Ok(cached_bytes.expect("checked above"));
        }
    }

    let response = match http_get(url, &conditional, MAX_DEMO_BYTES) {
        Ok(response) => response,
        Err(error) => {
            if let Some(bytes) = cached_bytes {
                return Ok(bytes);
            }
            return Err(error);
        }
    };

    if response.status == 304 {
        return cached_bytes.ok_or_else(|| {
            format!("demo server returned 304 but no valid cached copy exists for {url}")
        });
    }
    if response.status != 200 {
        return Err(format!("demo server returned HTTP {} for {url}", response.status));
    }
    if let Some(length) = response
        .headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
    {
        if length != response.body.len() {
            return Err(format!(
                "demo download length mismatch: expected {length} bytes, received {}",
                response.body.len()
            ));
        }
    }

    // Validate the complete protocol-26 stream before it is persisted. This is
    // stronger than checking the extension/magic: JKA demos have no magic
    // header, so a real validation means framing every record and decoding its
    // server messages with the same bounded Rust parser used by playback.
    validate_demo_bytes(&response.body)?;

    let temp_path = cache_path.with_extension("dm_26.tmp");
    fs::write(&temp_path, &response.body)
        .map_err(|error| format!("write ghost cache {}: {error}", temp_path.display()))?;
    if cache_path.is_file() {
        let _ = fs::remove_file(&cache_path);
    }
    fs::rename(&temp_path, &cache_path)
        .map_err(|error| format!("install ghost cache {}: {error}", cache_path.display()))?;

    let meta = serde_json::json!({
        "url": url,
        "etag": response.headers.get("etag").cloned().unwrap_or_default(),
        "last_modified": response.headers.get("last-modified").cloned().unwrap_or_default(),
        "content_length": response.body.len(),
    });
    let _ = fs::write(&meta_path, meta.to_string());
    Ok(response.body)
}

pub(super) fn demo_basename(url: &str) -> String {
    parse_url(url)
        .ok()
        .and_then(|parsed| parsed.path.split('?').next().map(str::to_owned))
        .and_then(|path| path.rsplit('/').find(|part| !part.is_empty()).map(str::to_owned))
        .and_then(|name| name.strip_suffix(".dm_26").map(str::to_owned).or(Some(name)))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "remote-race".to_owned())
}

fn read_cache_meta(path: &Path) -> Value {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null)
}

fn fetch_json(url: &str) -> Result<Value, String> {
    let headers = [("Accept".to_owned(), "application/json".to_owned())];
    let response = http_get(url, &headers, MAX_JSON_BYTES)?;
    if response.status != 200 {
        return Err(format!("catalog server returned HTTP {} for {url}", response.status));
    }
    serde_json::from_slice(&response.body).map_err(|error| format!("invalid JSON from {url}: {error}"))
}

fn parse_start_aabbs(object: &serde_json::Map<String, Value>) -> Vec<RemoteAabb> {
    let Some(aabbs) = object
        .get("start_geometry")
        .and_then(Value::as_object)
        .and_then(|geometry| geometry.get("aabbs"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    aabbs
        .iter()
        .filter_map(|value| {
            let object = value.as_object()?;
            let mut min = parse_vec3(object.get("min")?)?;
            let mut max = parse_vec3(object.get("max")?)?;
            for axis in 0..3 {
                if min[axis] > max[axis] {
                    std::mem::swap(&mut min[axis], &mut max[axis]);
                }
            }
            Some(RemoteAabb { min, max })
        })
        .collect()
}

fn parse_vec3(value: &Value) -> Option<[f64; 3]> {
    let values = value.as_array()?;
    if values.len() != 3 {
        return None;
    }
    let mut out = [0.0; 3];
    for axis in 0..3 {
        let value = values[axis].as_f64()?;
        if !value.is_finite() {
            return None;
        }
        out[axis] = value;
    }
    Some(out)
}

fn point_aabb_distance_sq(point: [f64; 3], aabb: RemoteAabb) -> f64 {
    let mut distance_sq = 0.0;
    for axis in 0..3 {
        let delta = if point[axis] < aabb.min[axis] {
            aabb.min[axis] - point[axis]
        } else if point[axis] > aabb.max[axis] {
            point[axis] - aabb.max[axis]
        } else {
            0.0
        };
        distance_sq += delta * delta;
    }
    distance_sq
}

fn aabb_volume(aabb: RemoteAabb) -> f64 {
    (aabb.max[0] - aabb.min[0])
        * (aabb.max[1] - aabb.min[1])
        * (aabb.max[2] - aabb.min[2])
}

fn resolve_demo_url(value: &str, allowed_origin: &ParsedUrl) -> Result<String, String> {
    if value.starts_with("//") {
        return Err("remote demo URL may not use a scheme-relative host".to_owned());
    }
    if value.starts_with('/') {
        if value.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
            || value.contains('\\')
            || value.contains(' ')
        {
            return Err("race ghost URL contains unsafe characters".to_owned());
        }
        let scheme = if allowed_origin.secure { "https" } else { "http" };
        let host = if allowed_origin.host.contains(':') {
            format!("[{}]", allowed_origin.host)
        } else {
            allowed_origin.host.clone()
        };
        let default_port = if allowed_origin.secure { 443 } else { 80 };
        let authority = if allowed_origin.port == default_port {
            host
        } else {
            format!("{host}:{}", allowed_origin.port)
        };
        return Ok(format!("{scheme}://{authority}{value}"));
    }
    let parsed = parse_url(value)?;
    validate_demo_url(&parsed, allowed_origin)?;
    Ok(value.to_owned())
}

fn validate_demo_url(candidate: &ParsedUrl, allowed_origin: &ParsedUrl) -> Result<(), String> {
    if candidate.secure != allowed_origin.secure
        || !candidate.host.eq_ignore_ascii_case(&allowed_origin.host)
        || candidate.port != allowed_origin.port
    {
        return Err("remote demo URL must use the same scheme, host, and port as the configured demo base URL".to_owned());
    }
    let path = candidate.path.split('?').next().unwrap_or(&candidate.path);
    let base_path = allowed_origin
        .path
        .split('?')
        .next()
        .unwrap_or(&allowed_origin.path)
        .trim_end_matches('/');
    if !base_path.is_empty() && base_path != "/" {
        let prefix = format!("{base_path}/");
        if path != base_path && !path.starts_with(&prefix) {
            return Err("remote demo URL must stay under the configured demo base path".to_owned());
        }
    }
    if path.split('/').any(|segment| segment == "." || segment == "..") {
        return Err("remote demo URL may not contain dot path segments".to_owned());
    }
    if !path.to_ascii_lowercase().ends_with(".dm_26") {
        return Err("remote demo URL must end in .dm_26".to_owned());
    }
    Ok(())
}

fn validate_demo_bytes(bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() {
        return Err("downloaded demo is empty".to_owned());
    }
    if bytes.len() > MAX_DEMO_BYTES {
        return Err(format!(
            "downloaded demo exceeds the {} MiB safety limit",
            MAX_DEMO_BYTES / (1024 * 1024)
        ));
    }

    let mut reader = DemoReader::new(Cursor::new(bytes));
    let mut decoder = ServerMessageDecoder::new();
    let mut records = 0usize;
    let mut saw_gamestate = false;
    let mut saw_snapshot = false;
    while let Some(record) = reader
        .next_record()
        .map_err(|error| format!("invalid .dm_26 framing after record {records}: {error}"))?
    {
        let sequence = record.sequence;
        let packet = decoder.parse_packet(sequence, &record.payload).map_err(|error| {
            format!(
                "invalid protocol-26 message at record {records} sequence {sequence} byte~{} bit {}: {error}",
                error.bit / 8,
                error.bit,
            )
        })?;
        records += 1;
        for event in packet.events {
            match event {
                ServerMessageEvent::Gamestate { .. } => saw_gamestate = true,
                ServerMessageEvent::Snapshot { .. } => saw_snapshot = true,
                _ => {}
            }
        }
    }
    if records == 0 || !saw_gamestate || !saw_snapshot {
        return Err("downloaded file is not a usable Jedi Academy protocol-26 demo".to_owned());
    }
    Ok(())
}

fn valid_header_value(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte >= 0x20 && byte != 0x7f && byte != b'\r' && byte != b'\n')
}

fn sanitize_filename(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars().take(80) {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() { "remote-race".to_owned() } else { out }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn encode_segment(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn parse_url(url: &str) -> Result<ParsedUrl, String> {
    if url.is_empty()
        || url.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
        || url.contains('\\')
        || url.contains(' ')
    {
        return Err("race ghost URL contains unsafe characters".to_owned());
    }
    let (secure, rest, default_port) = if let Some(rest) = url.strip_prefix("https://") {
        (true, rest, 443)
    } else if let Some(rest) = url.strip_prefix("http://") {
        (false, rest, 80)
    } else {
        return Err("race ghost URLs must use http:// or https://".to_owned());
    };
    let rest = rest.split('#').next().unwrap_or(rest);
    let (authority, path) = rest
        .split_once('/')
        .map_or((rest, "/".to_owned()), |(authority, path)| (authority, format!("/{path}")));
    if authority.is_empty() {
        return Err("race ghost URL has no host".to_owned());
    }
    if authority.contains('@') {
        return Err("race ghost URLs may not contain credentials".to_owned());
    }
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let close = bracketed.find(']').ok_or_else(|| "race ghost URL has an invalid IPv6 host".to_owned())?;
        let host = &bracketed[..close];
        let suffix = &bracketed[close + 1..];
        let port = if suffix.is_empty() {
            default_port
        } else {
            suffix
                .strip_prefix(':')
                .ok_or_else(|| "race ghost URL has an invalid IPv6 authority".to_owned())?
                .parse::<u16>()
                .map_err(|_| "race ghost URL has an invalid port".to_owned())?
        };
        (host.to_owned(), port)
    } else if authority.matches(':').count() == 1 {
        let (host, port) = authority.rsplit_once(':').expect("one colon");
        let port = port.parse::<u16>().map_err(|_| "race ghost URL has an invalid port".to_owned())?;
        (host.to_owned(), port)
    } else if authority.contains(':') {
        return Err("IPv6 race ghost hosts must use [address] syntax".to_owned());
    } else {
        (authority.to_owned(), default_port)
    };
    if host.is_empty() {
        return Err("race ghost URL has no host".to_owned());
    }
    Ok(ParsedUrl { secure, host, port, path })
}

fn http_get(url: &str, headers: &[(String, String)], max_bytes: usize) -> Result<HttpResponse, String> {
    platform::http_get(url, headers, max_bytes)
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::{ffi::c_void, ptr};

    type HInternet = *mut c_void;
    const WINHTTP_ACCESS_TYPE_DEFAULT_PROXY: u32 = 0;
    const WINHTTP_FLAG_SECURE: u32 = 0x0080_0000;
    const WINHTTP_QUERY_STATUS_CODE: u32 = 19;
    const WINHTTP_QUERY_RAW_HEADERS_CRLF: u32 = 22;
    const WINHTTP_QUERY_FLAG_NUMBER: u32 = 0x2000_0000;
    const WINHTTP_OPTION_REDIRECT_POLICY: u32 = 88;
    const WINHTTP_OPTION_REDIRECT_POLICY_NEVER: u32 = 0;

    #[link(name = "winhttp")]
    unsafe extern "system" {
        fn WinHttpOpen(
            user_agent: *const u16,
            access_type: u32,
            proxy_name: *const u16,
            proxy_bypass: *const u16,
            flags: u32,
        ) -> HInternet;
        fn WinHttpConnect(session: HInternet, server_name: *const u16, port: u16, reserved: u32) -> HInternet;
        fn WinHttpOpenRequest(
            connect: HInternet,
            verb: *const u16,
            object_name: *const u16,
            version: *const u16,
            referrer: *const u16,
            accept_types: *const *const u16,
            flags: u32,
        ) -> HInternet;
        fn WinHttpSetTimeouts(
            handle: HInternet,
            resolve_timeout: i32,
            connect_timeout: i32,
            send_timeout: i32,
            receive_timeout: i32,
        ) -> i32;
        fn WinHttpSetOption(
            handle: HInternet,
            option: u32,
            buffer: *mut c_void,
            buffer_length: u32,
        ) -> i32;
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
            info_level: u32,
            name: *const u16,
            buffer: *mut c_void,
            buffer_length: *mut u32,
            index: *mut u32,
        ) -> i32;
        fn WinHttpReadData(request: HInternet, buffer: *mut c_void, bytes_to_read: u32, bytes_read: *mut u32) -> i32;
        fn WinHttpCloseHandle(handle: HInternet) -> i32;
    }

    struct Handle(HInternet);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { WinHttpCloseHandle(self.0); }
            }
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub(super) fn http_get(url: &str, headers: &[(String, String)], max_bytes: usize) -> Result<HttpResponse, String> {
        let parsed = parse_url(url)?;
        let agent = wide("DinurdoJK/0.1 race-ghost");
        let host = wide(&parsed.host);
        let verb = wide("GET");
        let path = wide(&parsed.path);
        let session = Handle(unsafe {
            WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, ptr::null(), ptr::null(), 0)
        });
        if session.0.is_null() { return Err("WinHTTP open failed".to_owned()); }
        unsafe { WinHttpSetTimeouts(session.0, 15_000, 15_000, 15_000, 15_000); }
        let connect = Handle(unsafe { WinHttpConnect(session.0, host.as_ptr(), parsed.port, 0) });
        if connect.0.is_null() { return Err(format!("WinHTTP connect to {}:{} failed", parsed.host, parsed.port)); }
        let request = Handle(unsafe {
            WinHttpOpenRequest(
                connect.0,
                verb.as_ptr(),
                path.as_ptr(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                if parsed.secure { WINHTTP_FLAG_SECURE } else { 0 },
            )
        });
        if request.0.is_null() { return Err("WinHTTP request creation failed".to_owned()); }
        // Do not let a catalog-controlled URL escape same-origin validation via
        // an HTTP redirect (for example, to localhost or another host).
        let mut redirect_policy = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
        if unsafe {
            WinHttpSetOption(
                request.0,
                WINHTTP_OPTION_REDIRECT_POLICY,
                (&mut redirect_policy as *mut u32).cast(),
                std::mem::size_of::<u32>() as u32,
            )
        } == 0
        {
            return Err("WinHTTP could not disable redirects for race-ghost request".to_owned());
        }

        let mut header_text = String::from("Accept-Encoding: identity\r\n");
        for (name, value) in headers {
            header_text.push_str(name);
            header_text.push_str(": ");
            header_text.push_str(value);
            header_text.push_str("\r\n");
        }
        let header_wide = wide(&header_text);
        let sent = unsafe {
            WinHttpSendRequest(
                request.0,
                header_wide.as_ptr(),
                u32::MAX,
                ptr::null_mut(),
                0,
                0,
                0,
            )
        };
        if sent == 0 { return Err(format!("WinHTTP send failed for {url}")); }
        if unsafe { WinHttpReceiveResponse(request.0, ptr::null_mut()) } == 0 {
            return Err(format!("WinHTTP receive failed for {url}"));
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
            return Err(format!("WinHTTP status query failed for {url}"));
        }

        let headers = query_raw_headers(request.0);
        let mut body = Vec::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            if unsafe {
                WinHttpReadData(
                    request.0,
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as u32,
                    &mut read,
                )
            } == 0
            {
                return Err(format!("WinHTTP body read failed for {url}"));
            }
            if read == 0 { break; }
            if body.len().saturating_add(read as usize) > max_bytes {
                return Err(format!("HTTP response exceeds {} MiB safety limit", max_bytes / (1024 * 1024)));
            }
            body.extend_from_slice(&buffer[..read as usize]);
        }
        Ok(HttpResponse { status: status as u16, headers, body })
    }

    fn query_raw_headers(request: HInternet) -> BTreeMap<String, String> {
        let mut bytes = 0u32;
        unsafe {
            WinHttpQueryHeaders(
                request,
                WINHTTP_QUERY_RAW_HEADERS_CRLF,
                ptr::null(),
                ptr::null_mut(),
                &mut bytes,
                ptr::null_mut(),
            );
        }
        if bytes < 2 { return BTreeMap::new(); }
        let mut buffer = vec![0u16; (bytes as usize + 1) / 2];
        if unsafe {
            WinHttpQueryHeaders(
                request,
                WINHTTP_QUERY_RAW_HEADERS_CRLF,
                ptr::null(),
                buffer.as_mut_ptr().cast(),
                &mut bytes,
                ptr::null_mut(),
            )
        } == 0
        {
            return BTreeMap::new();
        }
        let text = String::from_utf16_lossy(&buffer).trim_matches('\0').to_owned();
        parse_header_lines(&text)
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;
    use std::io::{BufRead, BufReader};

    pub(super) fn http_get(url: &str, headers: &[(String, String)], max_bytes: usize) -> Result<HttpResponse, String> {
        let parsed = parse_url(url)?;
        if parsed.secure {
            return Err("HTTPS race-ghost downloads currently use Windows WinHTTP; this build only has the plain HTTP fallback".to_owned());
        }
        let mut stream = TcpStream::connect((parsed.host.as_str(), parsed.port))
            .map_err(|error| format!("HTTP connect to {}:{} failed: {error}", parsed.host, parsed.port))?;
        stream.set_read_timeout(Some(HTTP_TIMEOUT)).map_err(|error| error.to_string())?;
        stream.set_write_timeout(Some(HTTP_TIMEOUT)).map_err(|error| error.to_string())?;
        let host_header = if parsed.port == 80 { parsed.host.clone() } else { format!("{}:{}", parsed.host, parsed.port) };
        write!(
            stream,
            "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: DinurdoJK/0.1 race-ghost\r\nAccept-Encoding: identity\r\n",
            parsed.path,
            host_header
        ).map_err(|error| error.to_string())?;
        for (name, value) in headers {
            write!(stream, "{name}: {value}\r\n").map_err(|error| error.to_string())?;
        }
        write!(stream, "Connection: close\r\n\r\n").map_err(|error| error.to_string())?;
        stream.flush().map_err(|error| error.to_string())?;

        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        reader.read_line(&mut status_line).map_err(|error| error.to_string())?;
        let status = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<u16>().ok())
            .ok_or_else(|| format!("invalid HTTP status: {status_line}"))?;
        let mut header_text = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).map_err(|error| error.to_string())?;
            if line == "\r\n" || line == "\n" || line.is_empty() { break; }
            header_text.push_str(&line);
        }
        let headers = parse_header_lines(&header_text);
        let mut body = Vec::new();
        reader
            .take(max_bytes as u64 + 1)
            .read_to_end(&mut body)
            .map_err(|error| error.to_string())?;
        if body.len() > max_bytes {
            return Err(format!("HTTP response exceeds {} MiB safety limit", max_bytes / (1024 * 1024)));
        }
        Ok(HttpResponse { status, headers, body })
    }
}

fn parse_header_lines(text: &str) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    for line in text.lines() {
        let Some((name, value)) = line.split_once(':') else { continue };
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
    }
    headers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_normalization_defaults_to_http_and_avoids_double_index() {
        assert_eq!(normalize_base_url("s.playja.pro/races").unwrap(), "http://s.playja.pro/races");
        assert_eq!(normalize_base_url("http://s.playja.pro/races/index/").unwrap(), "http://s.playja.pro/races");
        assert_eq!(normalize_base_url("https://example.test/races").unwrap(), "https://example.test/races");
    }

    #[test]
    fn course_parser_filters_known_styles_but_keeps_styleless_catalog_rows() {
        let value: Value = serde_json::from_str(r#"{
            "courses": [
                {"map":"racearena_pro","course":"dash1","coursename":"racearena_pro (dash1)","styles":["jka","cpm"]},
                {"map":"racearena_pro","course":"dash2","coursename":"racearena_pro (dash2)","styles":["cpm"]},
                {"map":"racearena_pro","course":"","coursename":"racearena_pro"}
            ]
        }"#).unwrap();
        let mut out = BTreeMap::new();
        collect_courses(&value, "racearena_pro", "jka", &mut out);
        assert!(out.contains_key("dash1"));
        assert!(out.contains_key(""));
        assert!(!out.contains_key("dash2"));
    }

    #[test]
    fn demo_parser_only_accepts_absolute_demo_urls() {
        let value: Value = serde_json::from_str(r#"{
            "entries":[
                {"name":"Alpha","demo_url":"http://example.test/a.dm_26"},
                {"name":"Beta","demo_url":null},
                {"name":"Gamma","demo_url":"relative/g.dm_26"}
            ]
        }"#).unwrap();
        let mut seen = BTreeSet::new();
        let mut demos = Vec::new();
        let origin = parse_url("http://example.test/races").unwrap();
        collect_demos(&value, &origin, &mut seen, &mut demos);
        assert_eq!(demos.len(), 1);
        assert_eq!(demos[0].label, "Alpha");
    }

    #[test]
    fn demo_urls_must_be_same_origin_dm26_files() {
        let origin = parse_url("http://s.playja.pro/races").unwrap();
        assert!(validate_demo_url(
            &parse_url("http://s.playja.pro/races/demos/a.dm_26").unwrap(),
            &origin
        ).is_ok());
        assert!(validate_demo_url(
            &parse_url("https://s.playja.pro/races/demos/a.dm_26").unwrap(),
            &origin
        ).is_err());
        assert!(validate_demo_url(
            &parse_url("http://evil.example/a.dm_26").unwrap(),
            &origin
        ).is_err());
        assert!(validate_demo_url(
            &parse_url("http://s.playja.pro/races/demos/not-a-demo.exe").unwrap(),
            &origin
        ).is_err());
    }

    #[test]
    fn url_parser_rejects_request_injection_and_credentials() {
        assert!(parse_url("http://example.test/a\r\nX-Evil: yes").is_err());
        assert!(parse_url("http://user:pass@example.test/a.dm_26").is_err());
        assert!(parse_url("http://example.test/a b.dm_26").is_err());
    }
}
