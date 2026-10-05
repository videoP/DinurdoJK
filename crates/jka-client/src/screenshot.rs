//! DinurdoJK screenshot metadata and filesystem catalog.
//!
//! `/screenshot` remains an ordinary JPEG.  A small APP15 segment immediately
//! after SOI carries versioned JSON for DinurdoJK-aware tools; normal image
//! viewers ignore it.  Keeping the metadata in the image avoids sidecars that
//! can be separated from the screenshot when it is copied or uploaded.

use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const JPEG_SOI: [u8; 2] = [0xff, 0xd8];
const JPEG_APP15: [u8; 2] = [0xff, 0xef];
const SIGNATURE: &[u8] = b"DinurdoJK-Screenshot\0";
const MAX_APP_PAYLOAD: usize = 65_533;
pub const METADATA_VERSION: u32 = 1;

#[derive(Debug, Clone, Default)]
pub struct ScreenshotPlayer {
    pub client_num: i32,
    pub name: String,
    pub team: i32,
    pub score: Option<i32>,
    pub ping: Option<i32>,
    pub time_minutes: Option<i32>,
}

#[derive(Debug, Clone, Default)]
pub struct ScreenshotCrosshair {
    pub kind: String,
    pub title: String,
    pub material: Option<String>,
    pub distance: Option<String>,
    pub entity_num: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct ScreenshotMetadata {
    pub version: u32,
    pub captured_at: String,
    pub captured_unix_ms: u64,
    /// `base` for the base game, otherwise the fs_game directory leaf.
    pub game: String,
    pub map_name: Option<String>,
    pub server_name: Option<String>,
    pub server_address: Option<String>,
    pub player_name: String,
    pub players: Vec<ScreenshotPlayer>,
    /// Rendered camera/view origin in native JKA coordinates (Z up).
    pub camera_origin: Option<[f32; 3]>,
    /// Authoritative/predicted player origin when available.  This is the
    /// preferred teleport target because third-person camera origin is offset.
    pub player_origin: Option<[f32; 3]>,
    /// [pitch, yaw, roll], degrees, native JKA convention.
    pub view_angles: Option<[f32; 3]>,
    pub map_time_ms: Option<i32>,
    pub server_time_ms: Option<i32>,
    pub fov: f32,
    pub third_person: bool,
    pub crosshair: Option<ScreenshotCrosshair>,
}

impl Default for ScreenshotMetadata {
    fn default() -> Self {
        Self {
            version: METADATA_VERSION,
            captured_at: capture_timestamp_local(),
            captured_unix_ms: unix_time_ms(),
            game: "base".into(),
            map_name: None,
            server_name: None,
            server_address: None,
            player_name: String::new(),
            players: Vec::new(),
            camera_origin: None,
            player_origin: None,
            view_angles: None,
            map_time_ms: None,
            server_time_ms: None,
            fov: 90.0,
            third_person: false,
            crosshair: None,
        }
    }
}

impl ScreenshotMetadata {
    pub fn can_go_to_spot(&self) -> bool {
        self.map_name.as_deref().is_some_and(|map| !map.trim().is_empty())
            && self.view_angles.is_some()
            && (self.player_origin.is_some() || self.camera_origin.is_some())
    }

    fn to_value(&self) -> Value {
        let players = self.players.iter().map(|player| json!({
            "client_num": player.client_num,
            "name": player.name.as_str(),
            "team": player.team,
            "score": player.score,
            "ping": player.ping,
            "time_minutes": player.time_minutes,
        })).collect::<Vec<_>>();
        let crosshair = self.crosshair.as_ref().map(|crosshair| json!({
            "kind": crosshair.kind.as_str(),
            "title": crosshair.title.as_str(),
            "material": crosshair.material.as_deref(),
            "distance": crosshair.distance.as_deref(),
            "entity_num": crosshair.entity_num,
        }));
        json!({
            "version": self.version,
            "captured_at": self.captured_at.as_str(),
            "captured_unix_ms": self.captured_unix_ms,
            "game": self.game.as_str(),
            "map_name": self.map_name.as_deref(),
            "server_name": self.server_name.as_deref(),
            "server_address": self.server_address.as_deref(),
            "player_name": self.player_name.as_str(),
            "players": players,
            "camera_origin": self.camera_origin,
            "player_origin": self.player_origin,
            "view_angles": self.view_angles,
            "map_time_ms": self.map_time_ms,
            "server_time_ms": self.server_time_ms,
            "fov": self.fov,
            "third_person": self.third_person,
            "crosshair": crosshair,
        })
    }

    fn from_value(value: &Value) -> Result<Self, String> {
        let object = value.as_object().ok_or("screenshot metadata root is not an object")?;
        let version = object.get("version").and_then(Value::as_u64).unwrap_or(0) as u32;
        if version == 0 || version > METADATA_VERSION {
            return Err(format!("unsupported screenshot metadata version {version}"));
        }
        let string = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_owned);
        let integer = |key: &str| object.get(key).and_then(Value::as_i64).and_then(|v| i32::try_from(v).ok());
        let vec3 = |key: &str| -> Option<[f32; 3]> {
            let values = object.get(key)?.as_array()?;
            if values.len() != 3 { return None; }
            Some([
                values[0].as_f64()? as f32,
                values[1].as_f64()? as f32,
                values[2].as_f64()? as f32,
            ])
        };
        let players = object.get("players").and_then(Value::as_array).map(|players| {
            players.iter().filter_map(|value| {
                let player = value.as_object()?;
                Some(ScreenshotPlayer {
                    client_num: player.get("client_num").and_then(Value::as_i64).and_then(|v| i32::try_from(v).ok()).unwrap_or(-1),
                    name: player.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
                    team: player.get("team").and_then(Value::as_i64).and_then(|v| i32::try_from(v).ok()).unwrap_or(0),
                    score: player.get("score").and_then(Value::as_i64).and_then(|v| i32::try_from(v).ok()),
                    ping: player.get("ping").and_then(Value::as_i64).and_then(|v| i32::try_from(v).ok()),
                    time_minutes: player.get("time_minutes").and_then(Value::as_i64).and_then(|v| i32::try_from(v).ok()),
                })
            }).collect::<Vec<_>>()
        }).unwrap_or_default();
        let crosshair = object.get("crosshair").and_then(Value::as_object).map(|crosshair| ScreenshotCrosshair {
            kind: crosshair.get("kind").and_then(Value::as_str).unwrap_or_default().to_owned(),
            title: crosshair.get("title").and_then(Value::as_str).unwrap_or_default().to_owned(),
            material: crosshair.get("material").and_then(Value::as_str).map(str::to_owned),
            distance: crosshair.get("distance").and_then(Value::as_str).map(str::to_owned),
            entity_num: crosshair.get("entity_num").and_then(Value::as_u64).and_then(|v| u16::try_from(v).ok()),
        });
        Ok(Self {
            version,
            captured_at: string("captured_at").unwrap_or_default(),
            captured_unix_ms: object.get("captured_unix_ms").and_then(Value::as_u64).unwrap_or(0),
            game: string("game").unwrap_or_else(|| "base".into()),
            map_name: string("map_name"),
            server_name: string("server_name"),
            server_address: string("server_address"),
            player_name: string("player_name").unwrap_or_default(),
            players,
            camera_origin: vec3("camera_origin"),
            player_origin: vec3("player_origin"),
            view_angles: vec3("view_angles"),
            map_time_ms: integer("map_time_ms"),
            server_time_ms: integer("server_time_ms"),
            fov: object.get("fov").and_then(Value::as_f64).unwrap_or(90.0) as f32,
            third_person: object.get("third_person").and_then(Value::as_bool).unwrap_or(false),
            crosshair,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ScreenshotEntry {
    pub path: PathBuf,
    pub game: String,
    pub file_name: String,
    pub relative_name: String,
    pub modified_unix_ms: u64,
    pub metadata: Option<ScreenshotMetadata>,
    pub metadata_error: Option<String>,
}


pub fn embed_metadata(jpeg: &mut Vec<u8>, metadata: &ScreenshotMetadata) -> Result<(), String> {
    if !jpeg.starts_with(&JPEG_SOI) {
        return Err("encoded screenshot is not a JPEG".into());
    }
    let json = serde_json::to_vec(&metadata.to_value())
        .map_err(|error| format!("screenshot metadata encode failed: {error}"))?;
    let payload_len = SIGNATURE.len().saturating_add(json.len());
    if payload_len > MAX_APP_PAYLOAD {
        return Err(format!("screenshot metadata is too large ({payload_len} bytes)"));
    }
    let marker_len = u16::try_from(payload_len + 2)
        .map_err(|_| "screenshot metadata marker is too large".to_owned())?;
    let mut segment = Vec::with_capacity(payload_len + 4);
    segment.extend_from_slice(&JPEG_APP15);
    segment.extend_from_slice(&marker_len.to_be_bytes());
    segment.extend_from_slice(SIGNATURE);
    segment.extend_from_slice(&json);
    jpeg.splice(2..2, segment);
    Ok(())
}

pub fn read_metadata(bytes: &[u8]) -> Result<Option<ScreenshotMetadata>, String> {
    if !bytes.starts_with(&JPEG_SOI) {
        return Ok(None);
    }
    let mut cursor = 2usize;
    while cursor + 4 <= bytes.len() {
        if bytes[cursor] != 0xff {
            break;
        }
        while cursor < bytes.len() && bytes[cursor] == 0xff { cursor += 1; }
        let Some(&marker) = bytes.get(cursor) else { break };
        cursor += 1;
        if marker == 0xda || marker == 0xd9 { // SOS / EOI
            break;
        }
        // Standalone markers have no length field.
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        if cursor + 2 > bytes.len() { break; }
        let length = u16::from_be_bytes([bytes[cursor], bytes[cursor + 1]]) as usize;
        if length < 2 { return Err("invalid JPEG marker length".into()); }
        let payload_start = cursor + 2;
        let payload_end = payload_start.saturating_add(length - 2);
        if payload_end > bytes.len() { break; }
        if marker == 0xef {
            let payload = &bytes[payload_start..payload_end];
            if let Some(json) = payload.strip_prefix(SIGNATURE) {
                let value: Value = serde_json::from_slice(json)
                    .map_err(|error| format!("invalid DinurdoJK screenshot metadata: {error}"))?;
                return ScreenshotMetadata::from_value(&value).map(Some);
            }
        }
        cursor = payload_end;
    }
    Ok(None)
}

/// Metadata is placed immediately after SOI, so there is no reason to read a
/// multi-megabyte image just to index it.  96 KiB covers the maximum APP marker
/// plus ordinary JFIF/EXIF headers from copied/re-encoded files.
pub fn read_metadata_file(path: &Path) -> Result<Option<ScreenshotMetadata>, String> {
    let mut file = File::open(path).map_err(|error| format!("could not open {}: {error}", path.display()))?;
    let mut bytes = Vec::with_capacity(96 * 1024);
    file.by_ref().take(96 * 1024).read_to_end(&mut bytes)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    read_metadata(&bytes)
}

pub fn scan_screenshots(base: &Path) -> Result<Vec<ScreenshotEntry>, String> {
    let root = base.parent().unwrap_or(base);
    let mut game_dirs = Vec::new();
    if base.is_dir() { game_dirs.push(("base".to_owned(), base.to_path_buf())); }
    let entries = fs::read_dir(root).map_err(|error| format!("could not scan {}: {error}", root.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() || path == base { continue; }
        let screenshots = path.join("screenshots");
        if !screenshots.is_dir() { continue; }
        let game = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        if !game.is_empty() { game_dirs.push((game, path)); }
    }

    let mut screenshots = Vec::new();
    for (game, directory) in game_dirs {
        let screenshot_dir = directory.join("screenshots");
        if !screenshot_dir.is_dir() { continue; }
        scan_directory(&screenshot_dir, &screenshot_dir, &game, &mut screenshots)?;
    }
    screenshots.sort_by(|a, b| b.modified_unix_ms.cmp(&a.modified_unix_ms).then_with(|| b.path.cmp(&a.path)));
    Ok(screenshots)
}

fn scan_directory(
    root: &Path,
    directory: &Path,
    game: &str,
    output: &mut Vec<ScreenshotEntry>,
) -> Result<(), String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("could not scan {}: {error}", directory.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let file_type = match entry.file_type() { Ok(kind) => kind, Err(_) => continue };
        if file_type.is_dir() {
            scan_directory(root, &path, game, output)?;
            continue;
        }
        if !file_type.is_file() { continue; }
        let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or_default();
        if !matches!(extension.to_ascii_lowercase().as_str(), "jpg" | "jpeg" | "png") { continue; }
        let modified_unix_ms = entry.metadata().ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_millis().min(u64::MAX as u128) as u64);
        let (metadata, metadata_error) = match read_metadata_file(&path) {
            Ok(metadata) => (metadata, None),
            Err(error) => (None, Some(error)),
        };
        let relative_name = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
        let file_name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| relative_name.clone());
        output.push(ScreenshotEntry {
            path,
            game: game.to_owned(),
            file_name,
            relative_name,
            modified_unix_ms,
            metadata,
            metadata_error,
        });
    }
    Ok(())
}

pub fn read_image_rgba(path: &Path) -> Result<([usize; 2], Vec<u8>), String> {
    let image = image::open(path).map_err(|error| format!("could not decode {}: {error}", path.display()))?;
    let rgba = image.into_rgba8();
    Ok(([rgba.width() as usize, rgba.height() as usize], rgba.into_raw()))
}

pub fn unix_time_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis().min(u64::MAX as u128) as u64)
}

#[cfg(windows)]
pub fn capture_timestamp_local() -> String {
    #[repr(C)]
    struct WinSystemTime {
        year: u16, month: u16, day_of_week: u16, day: u16,
        hour: u16, minute: u16, second: u16, milliseconds: u16,
    }
    #[link(name = "Kernel32")]
    extern "system" { fn GetLocalTime(system_time: *mut WinSystemTime); }
    let mut local = std::mem::MaybeUninit::<WinSystemTime>::uninit();
    unsafe {
        GetLocalTime(local.as_mut_ptr());
        let local = local.assume_init();
        format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
            local.year, local.month, local.day, local.hour, local.minute, local.second, local.milliseconds)
    }
}

#[cfg(not(windows))]
pub fn capture_timestamp_local() -> String {
    format!("unix:{}", unix_time_ms())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_app15_metadata_round_trips() {
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xd9];
        let mut metadata = ScreenshotMetadata::default();
        metadata.map_name = Some("mp/ffa3".into());
        metadata.camera_origin = Some([1.0, 2.0, 3.0]);
        metadata.view_angles = Some([4.0, 5.0, 0.0]);
        metadata.players.push(ScreenshotPlayer {
            client_num: 2,
            name: "Jawa".into(),
            score: Some(12),
            ..Default::default()
        });
        embed_metadata(&mut jpeg, &metadata).unwrap();
        let decoded = read_metadata(&jpeg).unwrap().unwrap();
        assert_eq!(decoded.map_name.as_deref(), Some("mp/ffa3"));
        assert_eq!(decoded.camera_origin, Some([1.0, 2.0, 3.0]));
        assert_eq!(decoded.players[0].score, Some(12));
        assert!(decoded.can_go_to_spot());
    }

    #[test]
    fn plain_jpeg_has_no_metadata() {
        let jpeg = [0xff, 0xd8, 0xff, 0xd9];
        assert!(read_metadata(&jpeg).unwrap().is_none());
    }
}
