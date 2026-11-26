//! DinurdoJK screenshot metadata and filesystem catalog.
//!
//! `/screenshot` remains an ordinary JPEG and `/screenshot_png` an ordinary
//! PNG. Both carry one standard XMP packet: a JPEG APP1 segment after the JFIF
//! APP0 header, or a PNG `XML:com.adobe.xmp` iTXt chunk after IHDR. Game data
//! lives in the DinurdoJK namespace beside standard `xmp`/`dc` fields, and
//! equirectangular panoramas add Google GPano fields to the same packet, so
//! exiftool, Lightroom, digiKam and panorama viewers read it without sidecars.

use quick_xml::{
    events::{BytesStart, Event},
    name::{NamespaceResolver, ResolveResult},
    NsReader,
};
use std::{
    fmt::Write as _,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

const JPEG_SOI: [u8; 2] = [0xff, 0xd8];
const JPEG_APP0: u8 = 0xe0;
const JPEG_APP1: u8 = 0xe1;
const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
const PNG_ITXT: [u8; 4] = *b"iTXt";
const JPEG_XMP_SIGNATURE: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const PNG_XMP_KEYWORD: &[u8] = b"XML:com.adobe.xmp";
/// One JPEG APP1 segment (65533 payload bytes) minus the XMP signature. PNG
/// uses the same bound so the catalog's front-of-file read always covers it.
const MAX_XMP_PACKET: usize = 65_533 - JPEG_XMP_SIGNATURE.len();
/// Whitespace after the packet lets XMP editors change tags in place.
const XMP_PADDING: usize = 2048;
const XPACKET_END: &str = "<?xpacket end=\"w\"?>";

/// Permanent identity of the DinurdoJK screenshot namespace. Never change it;
/// evolve the schema additively and bump `METADATA_VERSION` instead.
pub const XMP_NAMESPACE: &str = "https://github.com/videop/DinurdoJK/ns/screenshot/";
pub const METADATA_VERSION: u32 = 2;

const RDF_NS: &[u8] = b"http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const XML_NS: &[u8] = b"http://www.w3.org/XML/1998/namespace";
const XMP_NS: &[u8] = b"http://ns.adobe.com/xap/1.0/";
const JK_NS: &[u8] = XMP_NAMESPACE.as_bytes();

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

/// Screenshot-page capture settings; absent for plain `/screenshot` frames.
#[derive(Debug, Clone, Default)]
pub struct ScreenshotCapture {
    pub panorama: bool,
    /// Projection name for panoramas (`equirectangular`, `cross`, ...).
    pub projection: Option<String>,
    pub hud_visible: bool,
    pub weapon_visible: bool,
    pub player_visible: bool,
    pub max_msaa: bool,
    pub supersample: u32,
}

#[derive(Debug, Clone)]
pub struct ScreenshotMetadata {
    pub version: u32,
    /// ISO 8601 with milliseconds and UTC offset (`xmp:CreateDate`).
    pub captured_at: String,
    pub captured_unix_ms: u64,
    pub client_version: String,
    /// `JKA` or `JK2`.
    pub game_family: String,
    /// `base` for the base game, otherwise the fs_game directory leaf.
    pub game: String,
    pub game_type: Option<String>,
    pub demo_playback: bool,
    pub spectating: bool,
    pub map_name: Option<String>,
    pub server_name: Option<String>,
    pub server_address: Option<String>,
    pub player_name: String,
    pub client_num: Option<i32>,
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
    pub capture: Option<ScreenshotCapture>,
    /// Screenshot-page "Embed game metadata". When false the writer omits the
    /// server, player names, scoreboard, timers and the crosshair target's
    /// identity; map, mod, position and view (needed for Go To Spot) remain.
    pub game_details: bool,
}

impl Default for ScreenshotMetadata {
    fn default() -> Self {
        let captured_unix_ms = unix_time_ms();
        Self {
            version: METADATA_VERSION,
            captured_at: iso8601_utc(captured_unix_ms),
            captured_unix_ms,
            client_version: String::new(),
            game_family: "JKA".into(),
            game: "base".into(),
            game_type: None,
            demo_playback: false,
            spectating: false,
            map_name: None,
            server_name: None,
            server_address: None,
            player_name: String::new(),
            client_num: None,
            players: Vec::new(),
            camera_origin: None,
            player_origin: None,
            view_angles: None,
            map_time_ms: None,
            server_time_ms: None,
            fov: 90.0,
            third_person: false,
            crosshair: None,
            capture: None,
            game_details: true,
        }
    }
}

impl ScreenshotMetadata {
    pub fn can_go_to_spot(&self) -> bool {
        self.map_name.as_deref().is_some_and(|map| !map.trim().is_empty())
            && self.view_angles.is_some()
            && (self.player_origin.is_some() || self.camera_origin.is_some())
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

// ---------------------------------------------------------------------------
// XMP writer
// ---------------------------------------------------------------------------

/// Embed the metadata as the image's single XMP packet. `panorama` is the
/// full equirectangular size; GPano fields are added only for a 2:1 sphere.
pub fn embed_xmp(
    image: &mut Vec<u8>,
    metadata: &ScreenshotMetadata,
    panorama: Option<[u32; 2]>,
) -> Result<(), String> {
    let panorama = panorama.filter(|[width, height]| *width == height.saturating_mul(2));
    let mut packet = xmp_packet(metadata, panorama, true);
    if packet.len() + XPACKET_END.len() > MAX_XMP_PACKET {
        // A full server of very long names must not cost the screenshot.
        packet = xmp_packet(metadata, panorama, false);
    }
    if packet.len() + XPACKET_END.len() > MAX_XMP_PACKET {
        return Err(format!("screenshot XMP is too large ({} bytes)", packet.len()));
    }
    let padding = XMP_PADDING.min(MAX_XMP_PACKET - packet.len() - XPACKET_END.len());
    for _ in 0..padding / 100 {
        packet.push_str(&" ".repeat(99));
        packet.push('\n');
    }
    packet.push_str(XPACKET_END);

    if image.starts_with(&JPEG_SOI) {
        let mut segment = Vec::with_capacity(4 + JPEG_XMP_SIGNATURE.len() + packet.len());
        let length = u16::try_from(2 + JPEG_XMP_SIGNATURE.len() + packet.len())
            .map_err(|_| "screenshot XMP segment is too large".to_owned())?;
        segment.extend_from_slice(&[0xff, JPEG_APP1]);
        segment.extend_from_slice(&length.to_be_bytes());
        segment.extend_from_slice(JPEG_XMP_SIGNATURE);
        segment.extend_from_slice(packet.as_bytes());
        let at = jpeg_xmp_offset(image);
        image.splice(at..at, segment);
        Ok(())
    } else if image.starts_with(&PNG_SIGNATURE) {
        if image.len() < 33 || &image[12..16] != b"IHDR" {
            return Err("encoded screenshot has no valid PNG IHDR".into());
        }
        let ihdr_len = u32::from_be_bytes([image[8], image[9], image[10], image[11]]) as usize;
        let at = 20usize
            .checked_add(ihdr_len)
            .filter(|at| *at <= image.len())
            .ok_or_else(|| "encoded screenshot has a truncated PNG IHDR".to_owned())?;
        // iTXt: keyword NUL, uncompressed flag/method, empty language tag,
        // empty translated keyword, then the UTF-8 packet.
        let mut data = Vec::with_capacity(PNG_XMP_KEYWORD.len() + 5 + packet.len());
        data.extend_from_slice(PNG_XMP_KEYWORD);
        data.extend_from_slice(&[0, 0, 0, 0, 0]);
        data.extend_from_slice(packet.as_bytes());
        let mut chunk = Vec::with_capacity(data.len() + 12);
        chunk.extend_from_slice(&(data.len() as u32).to_be_bytes());
        chunk.extend_from_slice(&PNG_ITXT);
        chunk.extend_from_slice(&data);
        chunk.extend_from_slice(&png_crc32(&PNG_ITXT, &data).to_be_bytes());
        image.splice(at..at, chunk);
        Ok(())
    } else {
        Err("encoded screenshot is neither JPEG nor PNG".into())
    }
}

/// JFIF requires APP0 directly after SOI; place XMP after it when present.
fn jpeg_xmp_offset(jpeg: &[u8]) -> usize {
    if jpeg.len() >= 6 && jpeg[2] == 0xff && jpeg[3] == JPEG_APP0 {
        let end = 4 + u16::from_be_bytes([jpeg[4], jpeg[5]]) as usize;
        if end <= jpeg.len() {
            return end;
        }
    }
    2
}

struct PacketWriter {
    out: String,
    depth: usize,
}

impl PacketWriter {
    fn indent(&mut self) {
        for _ in 0..self.depth {
            self.out.push_str("  ");
        }
    }

    fn text(&mut self, name: &str, value: &str) {
        self.indent();
        let _ = write!(self.out, "<{name}>");
        escape_xml_text(&mut self.out, value);
        let _ = writeln!(self.out, "</{name}>");
    }

    fn value(&mut self, name: &str, value: impl std::fmt::Display) {
        self.text(name, &value.to_string());
    }

    fn boolean(&mut self, name: &str, value: bool) {
        self.text(name, if value { "True" } else { "False" });
    }

    fn open(&mut self, tag: &str) {
        self.indent();
        let _ = writeln!(self.out, "<{tag}>");
        self.depth += 1;
    }

    fn open_struct(&mut self, name: &str) {
        self.open(&format!("{name} rdf:parseType=\"Resource\""));
    }

    fn close(&mut self, name: &str) {
        self.depth -= 1;
        self.indent();
        let _ = writeln!(self.out, "</{name}>");
    }

    fn vec3(&mut self, name: &str, labels: [&str; 3], value: Option<[f32; 3]>) {
        let Some(value) = value.filter(|v| v.iter().all(|c| c.is_finite())) else { return };
        self.open_struct(name);
        for (label, component) in labels.into_iter().zip(value) {
            self.value(label, component);
        }
        self.close(name);
    }
}

/// XML 1.0 forbids most C0 controls even as character references, and game
/// names are arbitrary bytes, so strip them rather than emit invalid XML.
fn escape_xml_text(out: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\t' | '\n' | '\r' => out.push(ch),
            ch if (ch as u32) < 0x20 || ch == '\u{fffe}' || ch == '\u{ffff}' => {}
            ch => out.push(ch),
        }
    }
}

/// The packet up to (not including) padding and the closing xpacket PI.
fn xmp_packet(metadata: &ScreenshotMetadata, panorama: Option<[u32; 2]>, players: bool) -> String {
    let details = metadata.game_details;
    let mut w = PacketWriter { out: String::with_capacity(8 * 1024), depth: 0 };
    w.out.push_str("<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n");
    let _ = writeln!(w.out, "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"DinurdoJK {}\">", metadata.client_version);
    w.depth = 1;
    w.open("rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"");
    w.indent();
    w.out.push_str("<rdf:Description rdf:about=\"\"\n");
    w.indent();
    w.out.push_str("    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n");
    w.indent();
    w.out.push_str("    xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n");
    w.indent();
    let _ = write!(w.out, "    xmlns:JKGame=\"{XMP_NAMESPACE}\"");
    if panorama.is_some() {
        w.out.push('\n');
        w.indent();
        w.out.push_str("    xmlns:GPano=\"http://ns.google.com/photos/1.0/panorama/\"");
    }
    w.out.push_str(">\n");
    w.depth += 1;

    // Standard fields: searchable by Explorer, Lightroom and digiKam.
    w.text("xmp:CreateDate", &metadata.captured_at);
    let creator = format!("DinurdoJK {}", metadata.client_version);
    w.text("xmp:CreatorTool", creator.trim_end());
    if let Some(map) = metadata.map_name.as_deref() {
        w.open("dc:title");
        w.open("rdf:Alt");
        w.indent();
        w.out.push_str("<rdf:li xml:lang=\"x-default\">");
        escape_xml_text(&mut w.out, map);
        w.out.push_str("</rdf:li>\n");
        w.close("rdf:Alt");
        w.close("dc:title");
    }
    w.open("dc:subject");
    w.open("rdf:Bag");
    for keyword in ["DinurdoJK", metadata.game_family.as_str(), metadata.game.as_str()]
        .into_iter()
        .chain(metadata.map_name.as_deref())
        .filter(|keyword| !keyword.is_empty())
    {
        w.text("rdf:li", keyword);
    }
    w.close("rdf:Bag");
    w.close("dc:subject");

    w.value("JKGame:SchemaVersion", metadata.version);
    w.value("JKGame:CapturedUnixMs", metadata.captured_unix_ms);
    if !metadata.client_version.is_empty() {
        w.text("JKGame:ClientVersion", &metadata.client_version);
    }
    w.text("JKGame:Game", &metadata.game_family);
    w.text("JKGame:Mod", &metadata.game);
    if let Some(map) = metadata.map_name.as_deref() {
        w.text("JKGame:MapName", map);
    }
    if let Some(game_type) = metadata.game_type.as_deref() {
        w.text("JKGame:GameType", game_type);
    }
    w.boolean("JKGame:DemoPlayback", metadata.demo_playback);
    w.boolean("JKGame:GameDetails", details);

    if details {
        if let Some(name) = metadata.server_name.as_deref() {
            w.text("JKGame:ServerName", name);
        }
        if let Some(address) = metadata.server_address.as_deref() {
            w.text("JKGame:ServerAddress", address);
        }
        if let Some(time) = metadata.server_time_ms {
            w.value("JKGame:ServerTimeMs", time);
        }
        if let Some(time) = metadata.map_time_ms {
            w.value("JKGame:MapTimeMs", time);
        }
        if !metadata.player_name.is_empty() {
            w.text("JKGame:PlayerName", &metadata.player_name);
        }
        if let Some(client_num) = metadata.client_num {
            w.value("JKGame:ClientNum", client_num);
        }
    }
    w.boolean("JKGame:Spectating", metadata.spectating);
    w.vec3("JKGame:PlayerOrigin", ["JKGame:X", "JKGame:Y", "JKGame:Z"], metadata.player_origin);
    w.vec3("JKGame:CameraOrigin", ["JKGame:X", "JKGame:Y", "JKGame:Z"], metadata.camera_origin);
    w.vec3("JKGame:ViewAngles", ["JKGame:Pitch", "JKGame:Yaw", "JKGame:Roll"], metadata.view_angles);
    if metadata.fov.is_finite() {
        w.value("JKGame:FOV", metadata.fov);
    }
    w.boolean("JKGame:ThirdPerson", metadata.third_person);

    if let Some(crosshair) = &metadata.crosshair {
        w.open_struct("JKGame:Crosshair");
        w.text("JKGame:Kind", &crosshair.kind);
        if details && !crosshair.title.is_empty() {
            w.text("JKGame:Title", &crosshair.title);
        }
        if let Some(material) = crosshair.material.as_deref() {
            w.text("JKGame:Material", material);
        }
        if let Some(distance) = crosshair.distance.as_deref() {
            w.text("JKGame:Distance", distance);
        }
        if let Some(entity_num) = crosshair.entity_num.filter(|_| details) {
            w.value("JKGame:EntityNum", entity_num);
        }
        w.close("JKGame:Crosshair");
    }

    if details && players && !metadata.players.is_empty() {
        w.open("JKGame:Players");
        w.open("rdf:Seq");
        for player in &metadata.players {
            w.open_struct("rdf:li");
            w.value("JKGame:ClientNum", player.client_num);
            w.text("JKGame:Name", &player.name);
            w.value("JKGame:Team", player.team);
            if let Some(score) = player.score {
                w.value("JKGame:Score", score);
            }
            if let Some(ping) = player.ping {
                w.value("JKGame:Ping", ping);
            }
            if let Some(minutes) = player.time_minutes {
                w.value("JKGame:TimeMinutes", minutes);
            }
            w.close("rdf:li");
        }
        w.close("rdf:Seq");
        w.close("JKGame:Players");
    }

    if let Some(capture) = &metadata.capture {
        w.open_struct("JKGame:Capture");
        w.text("JKGame:Mode", if capture.panorama { "panorama" } else { "standard" });
        if let Some(projection) = capture.projection.as_deref() {
            w.text("JKGame:Projection", projection);
        }
        w.boolean("JKGame:HUDVisible", capture.hud_visible);
        w.boolean("JKGame:WeaponVisible", capture.weapon_visible);
        w.boolean("JKGame:PlayerVisible", capture.player_visible);
        w.boolean("JKGame:MaxMSAA", capture.max_msaa);
        w.value("JKGame:Supersampling", capture.supersample);
        w.close("JKGame:Capture");
    }

    if let Some([width, height]) = panorama {
        w.boolean("GPano:UsePanoramaViewer", true);
        w.text("GPano:ProjectionType", "equirectangular");
        w.value("GPano:FullPanoWidthPixels", width);
        w.value("GPano:FullPanoHeightPixels", height);
        w.value("GPano:CroppedAreaImageWidthPixels", width);
        w.value("GPano:CroppedAreaImageHeightPixels", height);
        w.value("GPano:CroppedAreaLeftPixels", 0);
        w.value("GPano:CroppedAreaTopPixels", 0);
    }

    w.close("rdf:Description");
    w.close("rdf:RDF");
    w.out.push_str("</x:xmpmeta>\n");
    w.out
}

/// Return the native screenshot XMP packet with GPano fields for a full sphere.
/// Used by streaming PNG capture paths that cannot assemble the image in memory.
pub fn panorama_xmp(metadata: &ScreenshotMetadata, width: u32, height: u32) -> Result<String, String> {
    if width != height.saturating_mul(2) {
        return Err("GPano requires a 2:1 full sphere".into());
    }
    streaming_xmp(metadata, Some([width, height]))
}

/// Same single packet as `embed_xmp`, for streaming encoders that write the
/// segment/chunk before image data. `panorama` adds GPano for a 2:1 sphere.
pub fn streaming_xmp(metadata: &ScreenshotMetadata, panorama: Option<[u32; 2]>) -> Result<String, String> {
    let panorama = panorama.filter(|[width, height]| *width == height.saturating_mul(2));
    let mut packet = xmp_packet(metadata, panorama, true);
    if packet.len() + XPACKET_END.len() > MAX_XMP_PACKET {
        packet = xmp_packet(metadata, panorama, false);
    }
    if packet.len() + XPACKET_END.len() > MAX_XMP_PACKET {
        return Err("screenshot XMP is too large".into());
    }
    packet.push_str(XPACKET_END);
    Ok(packet)
}

fn png_crc32(chunk_type: &[u8; 4], data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in chunk_type.iter().chain(data.iter()) {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

// ---------------------------------------------------------------------------
// XMP reader
// ---------------------------------------------------------------------------

fn find_jpeg_xmp(bytes: &[u8]) -> Result<Option<&[u8]>, String> {
    let mut cursor = 2usize;
    while cursor + 4 <= bytes.len() {
        if bytes[cursor] != 0xff {
            break;
        }
        while cursor < bytes.len() && bytes[cursor] == 0xff {
            cursor += 1;
        }
        let Some(&marker) = bytes.get(cursor) else { break };
        cursor += 1;
        if marker == 0xda || marker == 0xd9 {
            break;
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        if cursor + 2 > bytes.len() {
            break;
        }
        let length = u16::from_be_bytes([bytes[cursor], bytes[cursor + 1]]) as usize;
        if length < 2 {
            return Err("invalid JPEG marker length".into());
        }
        let payload_start = cursor + 2;
        let payload_end = payload_start.saturating_add(length - 2);
        if payload_end > bytes.len() {
            break;
        }
        if marker == JPEG_APP1 {
            if let Some(packet) = bytes[payload_start..payload_end].strip_prefix(JPEG_XMP_SIGNATURE) {
                return Ok(Some(packet));
            }
        }
        cursor = payload_end;
    }
    Ok(None)
}

fn find_png_xmp(bytes: &[u8]) -> Result<Option<&[u8]>, String> {
    let mut cursor = PNG_SIGNATURE.len();
    while cursor + 12 <= bytes.len() {
        let length = u32::from_be_bytes([
            bytes[cursor],
            bytes[cursor + 1],
            bytes[cursor + 2],
            bytes[cursor + 3],
        ]) as usize;
        let chunk_type_start = cursor + 4;
        let data_start = chunk_type_start + 4;
        let Some(chunk_end) = data_start.checked_add(length).and_then(|end| end.checked_add(4)) else {
            return Err("PNG chunk size overflowed usize".into());
        };
        if chunk_end > bytes.len() {
            break;
        }
        let chunk_type = &bytes[chunk_type_start..data_start];
        if chunk_type == PNG_ITXT.as_slice() {
            let data = &bytes[data_start..data_start + length];
            if let Some(fields) = data
                .strip_prefix(PNG_XMP_KEYWORD)
                .and_then(|rest| rest.strip_prefix(&[0]))
            {
                if fields.len() < 4 {
                    return Err("invalid PNG XMP iTXt chunk".into());
                }
                if fields[0] != 0 {
                    return Err("compressed PNG XMP is unsupported".into());
                }
                // Skip compression method, language tag and translated keyword.
                let mut rest = &fields[2..];
                for _ in 0..2 {
                    let end = rest
                        .iter()
                        .position(|&byte| byte == 0)
                        .ok_or_else(|| "invalid PNG XMP iTXt header".to_owned())?;
                    rest = &rest[end + 1..];
                }
                return Ok(Some(rest));
            }
        }
        if chunk_type == b"IDAT" || chunk_type == b"IEND" {
            break;
        }
        cursor = chunk_end;
    }
    Ok(None)
}

pub fn read_metadata(bytes: &[u8]) -> Result<Option<ScreenshotMetadata>, String> {
    let packet = if bytes.starts_with(&JPEG_SOI) {
        find_jpeg_xmp(bytes)?
    } else if bytes.starts_with(&PNG_SIGNATURE) {
        find_png_xmp(bytes)?
    } else {
        None
    };
    match packet {
        Some(packet) => parse_xmp(packet),
        None => Ok(None),
    }
}

/// Metadata is placed at the front of either file type (after JPEG APP0 or
/// PNG IHDR), so there is no reason to read a multi-megabyte image just to
/// index it. 96 KiB covers the maximum XMP packet.
pub fn read_metadata_file(path: &Path) -> Result<Option<ScreenshotMetadata>, String> {
    let mut file = File::open(path).map_err(|error| format!("could not open {}: {error}", path.display()))?;
    let mut bytes = Vec::with_capacity(96 * 1024);
    file.by_ref().take(96 * 1024).read_to_end(&mut bytes)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    read_metadata(&bytes)
}

#[derive(Default)]
struct XmlNode {
    ns: Vec<u8>,
    name: Vec<u8>,
    attrs: Vec<(Vec<u8>, Vec<u8>, String)>,
    children: Vec<XmlNode>,
    text: String,
}

impl XmlNode {
    fn is(&self, ns: &[u8], name: &[u8]) -> bool {
        self.ns == ns && self.name == name
    }

    fn attr(&self, ns: &[u8], name: &[u8]) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(attr_ns, attr_name, _)| attr_ns == ns && attr_name == name)
            .map(|(_, _, value)| value.as_str())
    }
}

enum XmpValue {
    Text(String),
    Struct(Vec<XmpProp>),
    Array(Vec<XmpValue>),
}

struct XmpProp {
    ns: Vec<u8>,
    name: Vec<u8>,
    value: XmpValue,
}

impl XmpValue {
    fn text(&self) -> Option<&str> {
        match self { Self::Text(text) => Some(text), _ => None }
    }
    fn fields(&self) -> Option<&[XmpProp]> {
        match self { Self::Struct(fields) => Some(fields), _ => None }
    }
    fn items(&self) -> Option<&[XmpValue]> {
        match self { Self::Array(items) => Some(items), _ => None }
    }
}

fn bound_ns(result: ResolveResult<'_>) -> Vec<u8> {
    match result {
        ResolveResult::Bound(ns) => ns.as_ref().to_vec(),
        _ => Vec::new(),
    }
}

fn open_node(resolver: &NamespaceResolver, start: &BytesStart<'_>) -> Result<XmlNode, String> {
    let (ns, local) = resolver.resolve_element(start.name());
    let mut node = XmlNode { ns: bound_ns(ns), name: local.as_ref().to_vec(), ..XmlNode::default() };
    for attr in start.attributes() {
        let attr = attr.map_err(|error| format!("invalid XMP attribute: {error}"))?;
        let key = attr.key.as_ref();
        if key == b"xmlns" || key.starts_with(b"xmlns:") {
            continue;
        }
        let (ns, local) = resolver.resolve_attribute(attr.key);
        let value = attr
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|error| format!("invalid XMP attribute value: {error}"))?
            .into_owned();
        node.attrs.push((bound_ns(ns), local.as_ref().to_vec(), value));
    }
    Ok(node)
}

fn parse_xml(xml: &str) -> Result<XmlNode, String> {
    let mut reader = NsReader::from_str(xml);
    let mut stack = vec![XmlNode::default()];
    let invalid = |error: &dyn std::fmt::Display| format!("invalid screenshot XMP: {error}");
    loop {
        let event = reader.read_event().map_err(|error| invalid(&error))?;
        match event {
            Event::Start(start) => stack.push(open_node(reader.resolver(), &start)?),
            Event::Empty(start) => {
                let node = open_node(reader.resolver(), &start)?;
                stack.last_mut().expect("root node").children.push(node);
            }
            Event::End(_) => {
                if stack.len() < 2 {
                    return Err(invalid(&"unbalanced end tag"));
                }
                let node = stack.pop().expect("open node");
                stack.last_mut().expect("root node").children.push(node);
            }
            Event::Text(text) => {
                let text = text.xml10_content().map_err(|error| invalid(&error))?;
                stack.last_mut().expect("root node").text.push_str(&text);
            }
            Event::CData(data) => {
                let data = data.decode().map_err(|error| invalid(&error))?;
                stack.last_mut().expect("root node").text.push_str(&data);
            }
            Event::GeneralRef(reference) => {
                let ch = match reference.resolve_char_ref().map_err(|error| invalid(&error))? {
                    Some(ch) => ch,
                    None => match &*reference.decode().map_err(|error| invalid(&error))? {
                        "amp" => '&',
                        "lt" => '<',
                        "gt" => '>',
                        "quot" => '"',
                        "apos" => '\'',
                        other => return Err(invalid(&format!("unknown entity &{other};"))),
                    },
                };
                stack.last_mut().expect("root node").text.push(ch);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if stack.len() != 1 {
        return Err(invalid(&"unclosed element"));
    }
    Ok(stack.pop().expect("root node"))
}

fn is_property_ns(ns: &[u8]) -> bool {
    !ns.is_empty() && ns != RDF_NS && ns != XML_NS
}

/// Properties of a description: both attribute shorthand (as written by
/// exiftool/Lightroom) and element form.
fn node_props(node: &XmlNode, out: &mut Vec<XmpProp>) {
    for (ns, name, value) in &node.attrs {
        if is_property_ns(ns) {
            out.push(XmpProp { ns: ns.clone(), name: name.clone(), value: XmpValue::Text(value.clone()) });
        }
    }
    for child in &node.children {
        if is_property_ns(&child.ns) {
            out.push(XmpProp { ns: child.ns.clone(), name: child.name.clone(), value: element_value(child) });
        }
    }
}

fn element_value(node: &XmlNode) -> XmpValue {
    let mut fields = Vec::new();
    if node.attr(RDF_NS, b"parseType") == Some("Resource") {
        node_props(node, &mut fields);
        return XmpValue::Struct(fields);
    }
    if let Some(container) = node.children.iter().find(|child| {
        child.ns == RDF_NS && matches!(child.name.as_slice(), b"Seq" | b"Bag" | b"Alt")
    }) {
        return XmpValue::Array(
            container.children.iter().filter(|item| item.is(RDF_NS, b"li")).map(element_value).collect(),
        );
    }
    if let Some(description) = node.children.iter().find(|child| child.is(RDF_NS, b"Description")) {
        node_props(description, &mut fields);
        return XmpValue::Struct(fields);
    }
    if let Some(resource) = node.attr(RDF_NS, b"resource") {
        return XmpValue::Text(resource.to_owned());
    }
    if node.text.trim().is_empty() && node.attrs.iter().any(|(ns, _, _)| is_property_ns(ns)) {
        node_props(node, &mut fields);
        return XmpValue::Struct(fields);
    }
    XmpValue::Text(node.text.clone())
}

fn find_rdf(node: &XmlNode) -> Option<&XmlNode> {
    if node.is(RDF_NS, b"RDF") {
        return Some(node);
    }
    node.children.iter().find_map(find_rdf)
}

fn prop<'a>(props: &'a [XmpProp], ns: &[u8], name: &str) -> Option<&'a XmpValue> {
    props.iter().find(|prop| prop.ns == ns && prop.name == name.as_bytes()).map(|prop| &prop.value)
}

fn jk_text(props: &[XmpProp], name: &str) -> Option<String> {
    prop(props, JK_NS, name)?.text().map(str::to_owned)
}

fn jk_parse<T: FromStr>(props: &[XmpProp], name: &str) -> Option<T> {
    prop(props, JK_NS, name)?.text()?.trim().parse().ok()
}

fn jk_bool(props: &[XmpProp], name: &str) -> Option<bool> {
    match prop(props, JK_NS, name)?.text()?.trim() {
        "True" | "true" | "1" => Some(true),
        "False" | "false" | "0" => Some(false),
        _ => None,
    }
}

fn jk_vec3(props: &[XmpProp], name: &str, labels: [&str; 3]) -> Option<[f32; 3]> {
    let fields = prop(props, JK_NS, name)?.fields()?;
    Some([jk_parse(fields, labels[0])?, jk_parse(fields, labels[1])?, jk_parse(fields, labels[2])?])
}

fn parse_xmp(packet: &[u8]) -> Result<Option<ScreenshotMetadata>, String> {
    let xml = std::str::from_utf8(packet).map_err(|_| "screenshot XMP is not UTF-8".to_owned())?;
    let root = parse_xml(xml)?;
    let Some(rdf) = find_rdf(&root) else { return Ok(None) };
    // Tools may split properties across several descriptions.
    let mut props = Vec::new();
    for description in rdf.children.iter().filter(|child| child.is(RDF_NS, b"Description")) {
        node_props(description, &mut props);
    }
    let Some(version) = jk_parse::<u32>(&props, "SchemaVersion") else {
        return Ok(None);
    };
    if version == 0 || version > METADATA_VERSION {
        return Err(format!("unsupported screenshot metadata version {version}"));
    }

    let players = prop(&props, JK_NS, "Players")
        .and_then(XmpValue::items)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let fields = item.fields()?;
                    Some(ScreenshotPlayer {
                        client_num: jk_parse(fields, "ClientNum").unwrap_or(-1),
                        name: jk_text(fields, "Name").unwrap_or_default(),
                        team: jk_parse(fields, "Team").unwrap_or(0),
                        score: jk_parse(fields, "Score"),
                        ping: jk_parse(fields, "Ping"),
                        time_minutes: jk_parse(fields, "TimeMinutes"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let crosshair = prop(&props, JK_NS, "Crosshair").and_then(XmpValue::fields).map(|fields| ScreenshotCrosshair {
        kind: jk_text(fields, "Kind").unwrap_or_default(),
        title: jk_text(fields, "Title").unwrap_or_default(),
        material: jk_text(fields, "Material"),
        distance: jk_text(fields, "Distance"),
        entity_num: jk_parse(fields, "EntityNum"),
    });
    let capture = prop(&props, JK_NS, "Capture").and_then(XmpValue::fields).map(|fields| ScreenshotCapture {
        panorama: jk_text(fields, "Mode").is_some_and(|mode| mode.eq_ignore_ascii_case("panorama")),
        projection: jk_text(fields, "Projection"),
        hud_visible: jk_bool(fields, "HUDVisible").unwrap_or(false),
        weapon_visible: jk_bool(fields, "WeaponVisible").unwrap_or(false),
        player_visible: jk_bool(fields, "PlayerVisible").unwrap_or(true),
        max_msaa: jk_bool(fields, "MaxMSAA").unwrap_or(false),
        supersample: jk_parse(fields, "Supersampling").unwrap_or(1),
    });
    let captured_unix_ms = jk_parse(&props, "CapturedUnixMs").unwrap_or(0);

    Ok(Some(ScreenshotMetadata {
        version,
        captured_at: prop(&props, XMP_NS, "CreateDate")
            .and_then(XmpValue::text)
            .map(str::to_owned)
            .unwrap_or_else(|| iso8601_utc(captured_unix_ms)),
        captured_unix_ms,
        client_version: jk_text(&props, "ClientVersion").unwrap_or_default(),
        game_family: jk_text(&props, "Game").unwrap_or_else(|| "JKA".into()),
        game: jk_text(&props, "Mod").unwrap_or_else(|| "base".into()),
        game_type: jk_text(&props, "GameType"),
        demo_playback: jk_bool(&props, "DemoPlayback").unwrap_or(false),
        spectating: jk_bool(&props, "Spectating").unwrap_or(false),
        map_name: jk_text(&props, "MapName"),
        server_name: jk_text(&props, "ServerName"),
        server_address: jk_text(&props, "ServerAddress"),
        player_name: jk_text(&props, "PlayerName").unwrap_or_default(),
        client_num: jk_parse(&props, "ClientNum"),
        players,
        camera_origin: jk_vec3(&props, "CameraOrigin", ["X", "Y", "Z"]),
        player_origin: jk_vec3(&props, "PlayerOrigin", ["X", "Y", "Z"]),
        view_angles: jk_vec3(&props, "ViewAngles", ["Pitch", "Yaw", "Roll"]),
        map_time_ms: jk_parse(&props, "MapTimeMs"),
        server_time_ms: jk_parse(&props, "ServerTimeMs"),
        fov: jk_parse(&props, "FOV").unwrap_or(90.0),
        third_person: jk_bool(&props, "ThirdPerson").unwrap_or(false),
        crosshair,
        capture,
        game_details: jk_bool(&props, "GameDetails").unwrap_or(true),
    }))
}

// ---------------------------------------------------------------------------
// Catalog
// ---------------------------------------------------------------------------

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

/// `YYYY-MM-DDTHH:MM:SS.mmmZ` for a Unix millisecond timestamp.
pub fn iso8601_utc(unix_ms: u64) -> String {
    let seconds = unix_ms / 1000;
    let days = (seconds / 86_400) as i64;
    let of_day = seconds % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        of_day / 3600,
        of_day / 60 % 60,
        of_day % 60,
        unix_ms % 1000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ScreenshotMetadata {
        let mut metadata = ScreenshotMetadata::default();
        metadata.map_name = Some("mp/ffa3".into());
        metadata.server_name = Some("^2Example <&> Server".into());
        metadata.player_name = "^7Pad\u{1}awan".into();
        metadata.camera_origin = Some([1.0, 2.0, 3.0]);
        metadata.player_origin = Some([1200.5, -800.0, 230.0]);
        metadata.view_angles = Some([-10.0, 275.0, 0.0]);
        metadata.players.push(ScreenshotPlayer {
            client_num: 2,
            name: "Jawa".into(),
            score: Some(12),
            ..Default::default()
        });
        metadata.crosshair = Some(ScreenshotCrosshair {
            kind: "player".into(),
            title: "Jawa".into(),
            entity_num: Some(2),
            ..Default::default()
        });
        metadata
    }

    #[test]
    fn jpeg_xmp_round_trips_after_jfif() {
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, b'J', b'F', 0xff, 0xd9];
        embed_xmp(&mut jpeg, &sample(), Some([8192, 4096])).unwrap();
        assert_eq!(&jpeg[2..4], &[0xff, 0xe0]);
        assert_eq!(&jpeg[8..10], &[0xff, JPEG_APP1]);
        let decoded = read_metadata(&jpeg).unwrap().unwrap();
        assert_eq!(decoded.map_name.as_deref(), Some("mp/ffa3"));
        assert_eq!(decoded.server_name.as_deref(), Some("^2Example <&> Server"));
        assert_eq!(decoded.player_name, "^7Padawan");
        assert_eq!(decoded.player_origin, Some([1200.5, -800.0, 230.0]));
        assert_eq!(decoded.players[0].score, Some(12));
        assert_eq!(decoded.crosshair.unwrap().entity_num, Some(2));
        assert!(decoded.can_go_to_spot());
    }

    #[test]
    fn png_xmp_round_trips() {
        use image::ImageEncoder;

        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&[10, 20, 30, 255], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        embed_xmp(&mut png, &sample(), None).unwrap();
        let decoded = read_metadata(&png).unwrap().unwrap();
        assert_eq!(decoded.view_angles, Some([-10.0, 275.0, 0.0]));
        assert!(decoded.can_go_to_spot());
        assert!(image::load_from_memory_with_format(&png, image::ImageFormat::Png).is_ok());
    }

    #[test]
    fn game_details_off_keeps_spot_only() {
        let mut metadata = sample();
        metadata.game_details = false;
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xd9];
        embed_xmp(&mut jpeg, &metadata, None).unwrap();
        let decoded = read_metadata(&jpeg).unwrap().unwrap();
        assert!(!decoded.game_details);
        assert!(decoded.server_name.is_none() && decoded.player_name.is_empty());
        assert!(decoded.players.is_empty());
        let crosshair = decoded.crosshair.unwrap();
        assert!(crosshair.title.is_empty() && crosshair.entity_num.is_none());
        assert!(decoded.can_go_to_spot());
    }

    #[test]
    fn attribute_form_xmp_is_read() {
        let xmp = format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:jk="{XMP_NAMESPACE}" jk:SchemaVersion="2" jk:MapName="mp/duel1"><jk:ViewAngles jk:Pitch="1" jk:Yaw="2" jk:Roll="0"/><jk:CameraOrigin><rdf:Description jk:X="4" jk:Y="5" jk:Z="6"/></jk:CameraOrigin></rdf:Description></rdf:RDF></x:xmpmeta>"#
        );
        let decoded = parse_xmp(xmp.as_bytes()).unwrap().unwrap();
        assert_eq!(decoded.map_name.as_deref(), Some("mp/duel1"));
        assert_eq!(decoded.camera_origin, Some([4.0, 5.0, 6.0]));
        assert!(decoded.can_go_to_spot());
    }

    #[test]
    fn plain_jpeg_has_no_metadata() {
        let jpeg = [0xff, 0xd8, 0xff, 0xd9];
        assert!(read_metadata(&jpeg).unwrap().is_none());
    }

    #[test]
    fn iso8601_utc_formats_epoch_offsets() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso8601_utc(951_782_400_123), "2000-02-29T00:00:00.123Z");
    }
}
