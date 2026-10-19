#![doc = include_str!("../README.md")]
pub mod audio_mix;
pub mod compact;
pub mod compiler;
pub mod effects;
pub mod manifest;
pub mod montage;
pub mod package;
pub mod pose;
pub mod presentation;
pub mod prop_mesh;
pub mod psa;
pub mod retarget;
pub mod source;
pub use pose::{Interpolation, PoseClip};
pub use source::{BundleSource, FileSource};

use serde_json::Value;
use std::num::{NonZeroU32, NonZeroU8};
pub fn compress_pcm16_vorbis(wav: &[u8]) -> Result<Vec<u8>, String> {
    validate_wav(wav)?;
    if wav.len() < 44 || wav[20..24] != [1, 0, 1, 0] || &wav[36..40] != b"data" {
        return Err("shipping encoder requires canonical mono PCM16".into());
    }
    let rate = u32::from_le_bytes(wav[24..28].try_into().unwrap());
    let mut encoder = vorbis_rs::VorbisEncoderBuilder::new_with_serial(
        NonZeroU32::new(rate).unwrap(),
        NonZeroU8::new(1).unwrap(),
        Vec::new(),
        1,
    );
    encoder.bitrate_management_strategy(vorbis_rs::VorbisBitrateManagementStrategy::Vbr {
        target_bitrate: NonZeroU32::new(48_000.min(rate * 2)).unwrap(),
    });
    let mut encoder = encoder.build().map_err(|e| e.to_string())?;
    for block in wav[44..].chunks(8192) {
        let samples: Vec<f32> = block
            .chunks_exact(2)
            .map(|s| i16::from_le_bytes(s.try_into().unwrap()) as f32 / 32768.0)
            .collect();
        encoder
            .encode_audio_block([samples.as_slice()])
            .map_err(|e| e.to_string())?;
    }
    encoder.finish().map_err(|e| e.to_string())
}

use std::{
    collections::{BTreeMap, HashSet},
    fs::File,
    hash::{Hash, Hasher},
    io::{Read, Write},
    path::Path,
};

pub const MAX_MEMBER_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_BUNDLE_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
pub const MAX_WAV_BYTES: usize = 64 * 1024 * 1024;
/// Encode the compiler's canonical mono PCM16 mix without external tools.
pub fn compress_pcm16_mix(bytes: &[u8]) -> Result<Vec<u8>, String> {
    use flacenc::{component::BitRepr, error::Verify};
    validate_wav(bytes)?;
    if bytes.len() < 44
        || bytes[20..24] != [1, 0, 1, 0]
        || bytes[34..36] != [16, 0]
        || &bytes[36..40] != b"data"
    {
        return Err("FLAC mix requires canonical mono PCM16 WAV".into());
    }
    let rate = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
    let samples: Vec<i32> = bytes[44..]
        .chunks_exact(2)
        .map(|s| i16::from_le_bytes(s.try_into().unwrap()) as i32)
        .collect();
    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|e| format!("FLAC config: {e:?}"))?;
    let source = flacenc::source::MemSource::from_samples(&samples, 1, 16, rate as usize);
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .map_err(|e| format!("FLAC encode: {e}"))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|e| format!("FLAC write: {e}"))?;
    let mut bytes = sink.as_slice().to_vec();
    // Fixed-block streams declare one block size; the short final block is
    // excluded from STREAMINFO's minimum. flacenc includes that last block,
    // which strict demuxers interpret as a variable-block stream and reject.
    let block = [bytes[10], bytes[11]];
    bytes[8..10].copy_from_slice(&block);
    Ok(bytes)
}

#[derive(Clone, Debug)]
pub struct Bundle {
    source: std::sync::Arc<dyn BundleSource>,
    pub manifest: Value,
    identity: u64,
    pub pose: Option<std::sync::Arc<PoseClip>>,
}

impl Bundle {
    pub fn open(path: &Path) -> Result<Self, String> {
        Self::from_source(Self::file_source(path)?, true)
    }
    /// Read the bounded directory and manifest for a large catalogue. Playback
    /// must use `open` before consuming an indexed bundle's assets.
    pub fn open_index(path: &Path) -> Result<Self, String> {
        Self::from_source(Self::file_source(path)?, false)
    }
    fn file_source(path: &Path) -> Result<std::sync::Arc<dyn BundleSource>, String> {
        let source = FileSource::file(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(std::sync::Arc::new(source))
    }
    /// Open a bundle from any [`BundleSource`]. With `validate_assets` false only
    /// the ZIP directory, manifest and asset references are read (catalogue
    /// use); with true the animation, pose and audio are decoded and checked.
    pub fn from_source(
        source: std::sync::Arc<dyn BundleSource>,
        validate_assets: bool,
    ) -> Result<Self, String> {
        let file = source
            .open()
            .map_err(|e| format!("{}: {e}", source.describe()))?;
        if source.len() > MAX_BUNDLE_BYTES {
            return Err("emote bundle exceeds 512 MiB".into());
        }
        let mut zip =
            zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", source.describe()))?;
        if zip.len() > 4096 {
            return Err("bundle exceeds 4096 members".into());
        }
        let mut total = 0u64;
        let mut names = HashSet::new();
        let mut identity = std::hash::DefaultHasher::new();
        // Identity includes every member's CRC and size, so refreshed assets cannot
        // reuse a stale renderer/audio cache key even if their EID is unchanged.
        source.key().hash(&mut identity);
        for n in 0..zip.len() {
            let entry = zip.by_index(n).map_err(|e| e.to_string())?;
            let name = manifest::qpath(entry.name())?;
            if name != entry.name() || entry.is_dir() || entry.is_symlink() {
                return Err(format!("invalid bundle member {}", entry.name()));
            }
            if !names.insert(name.to_ascii_lowercase()) {
                return Err(format!("duplicate bundle member {name}"));
            }
            if entry.size() > MAX_MEMBER_BYTES as u64 {
                return Err(format!("bundle member {name} exceeds 128 MiB"));
            }
            total = total
                .checked_add(entry.size())
                .ok_or("bundle size overflow")?;
            if total > MAX_BUNDLE_BYTES {
                return Err("expanded bundle exceeds 512 MiB".into());
            }
            name.hash(&mut identity);
            entry.size().hash(&mut identity);
            entry.crc32().hash(&mut identity);
        }
        let bytes = read_zip(&mut zip, "manifest.json", MAX_MANIFEST_BYTES)?;
        let root: Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("bundle manifest: {e}"))?;
        if root["format"] != "djk_fnemote" || root["version"].as_u64() != Some(1) {
            return Err("unsupported .djk_fnemote format/version (expected version 1)".into());
        }
        let emotes = manifest::parse_manifest(&root)?;
        if emotes.len() != 1 {
            return Err("a bundle must contain exactly one emote".into());
        }
        let emote = &emotes[0];
        for name in std::iter::once(emote.gla.as_str())
            .chain(root["emotes"][0]["pose"].as_str())
            .chain(
                emote
                    .audio
                    .iter()
                    .chain(emote.variants.iter().flat_map(|v| v.audio.iter()))
                    .filter_map(|e| e.wav.as_deref()),
            )
        {
            manifest::qpath(name)?;
            let entry = zip
                .by_name(name)
                .map_err(|e| format!("missing bundle asset {name}: {e}"))?;
            let limit = if name == emote.gla || Some(name) == root["emotes"][0]["pose"].as_str() {
                MAX_MEMBER_BYTES
            } else {
                MAX_WAV_BYTES
            };
            if entry.size() > limit as u64 {
                return Err(format!("bundle asset {name} exceeds size limit"));
            }
        }
        if !validate_assets {
            return Ok(Self {
                source,
                manifest: root,
                identity: identity.finish(),
                pose: None,
            });
        }
        let gla =
            jka_assets::ghoul2::parse_gla(&read_zip(&mut zip, &emote.gla, MAX_MEMBER_BYTES)?)?;
        let rig_only = root["rig_only"] == true;
        if !rig_only && emote.end_frame as usize > gla.num_frames {
            return Err("emote frame range exceeds bundled GLA".into());
        }
        gla.compose_frame(0)?; // Also rejects parent cycles.
        let pose = if let Some(name) = root["emotes"][0]["pose"].as_str() {
            manifest::qpath(name)?;
            let clip = PoseClip::parse(&read_zip(&mut zip, name, MAX_MEMBER_BYTES)?)?;
            if (!rig_only && clip.frames != gla.num_frames)
                || clip.bones != gla.skeleton.len()
                || emote.end_frame as usize > clip.frames
                || emote
                    .variants
                    .iter()
                    .any(|v| v.end_frame as usize > clip.frames)
            {
                return Err("baked pose dimensions/frame range do not match GLA".into());
            }
            Some(std::sync::Arc::new(clip))
        } else {
            if rig_only {
                return Err("rig-only bundle requires a baked pose".into());
            }
            None
        };
        let mut checked = BTreeMap::new();
        for cue in emote
            .audio
            .iter()
            .chain(emote.variants.iter().flat_map(|v| v.audio.iter()))
        {
            if let Some(wav) = &cue.wav {
                if !checked.contains_key(wav) {
                    checked.insert(
                        wav.clone(),
                        audio_frame_count(&read_zip(&mut zip, wav, MAX_WAV_BYTES)?)?,
                    );
                }
                if cue
                    .loop_start_sample
                    .is_some_and(|start| start >= checked[wav])
                {
                    return Err("audio loop point exceeds decoded track".into());
                }
            }
        }
        Ok(Self {
            source,
            manifest: root,
            identity: identity.finish(),
            pose,
        })
    }

    pub fn open_packed_index(path: &Path, member: &str) -> Result<Self, String> {
        Self::open_packed(path, member, false)
    }
    /// Use a location already validated by the VFS, avoiding one outer central
    /// directory parse per emote in a large shipping package.
    pub fn open_located_index(
        path: &Path,
        member: &str,
        range: Option<(u64, u64)>,
    ) -> Result<Self, String> {
        let member = manifest::qpath(member)?;
        let source = match range {
            Some((start, size)) => FileSource::packed(path, &member, start, size),
            None => FileSource::file(path),
        }
        .map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_source(std::sync::Arc::new(source), false)
    }
    fn open_packed(path: &Path, member: &str, validate: bool) -> Result<Self, String> {
        manifest::qpath(member)?;
        let file = File::open(path).map_err(|e| e.to_string())?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        let name = archive
            .file_names()
            .find(|n| n.eq_ignore_ascii_case(member))
            .ok_or("packed bundle missing")?
            .to_owned();
        let entry = archive.by_name(&name).map_err(|e| e.to_string())?;
        if entry.compression() != zip::CompressionMethod::Stored || entry.size() > MAX_BUNDLE_BYTES
        {
            return Err("packed emotes must be stored, bounded .djk_fnemote members".into());
        }
        let offset = entry.data_start().ok_or("packed bundle has no offset")?;
        let source =
            FileSource::packed(path, member, offset, entry.size()).map_err(|e| e.to_string())?;
        Self::from_source(std::sync::Arc::new(source), validate)
    }
    pub fn runtime_prefix(&self) -> String {
        format!("emotes/.bundles/{:016x}", self.identity)
    }
    pub fn validate_for_playback(&self) -> Result<Self, String> {
        let loaded = Self::from_source(self.source.clone(), true)?;
        if loaded.identity != self.identity {
            return Err("emote bundle changed; refresh the catalogue".into());
        }
        Ok(loaded)
    }
    pub fn read(&self, member: &str, limit: usize) -> Result<Vec<u8>, String> {
        manifest::qpath(member)?;
        let file = self.source.open().map_err(|e| e.to_string())?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        read_zip(&mut zip, member, limit.min(MAX_MEMBER_BYTES))
    }
}

fn read_zip<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
    limit: usize,
) -> Result<Vec<u8>, String> {
    let mut entry = zip
        .by_name(name)
        .map_err(|e| format!("bundle member {name}: {e}"))?;
    if entry.size() > limit as u64 {
        return Err(format!("bundle member {name} exceeds size limit"));
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    (&mut entry)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 != entry.size() || bytes.len() > limit {
        return Err("invalid bundle member size".into());
    }
    Ok(bytes)
}

/// Bound the decoded size before the runtime codec sees a compressed track.
pub fn audio_frame_count(bytes: &[u8]) -> Result<u64, String> {
    validate_audio(bytes)?;
    if bytes.starts_with(b"OggS") {
        return validate_vorbis(bytes);
    }
    if bytes.starts_with(b"fLaC") {
        return Ok(u64::from_be_bytes(bytes[18..26].try_into().unwrap()) & 0xf_ffff_ffff);
    }
    let mut at = 12;
    let mut align = 0;
    while at + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        if &bytes[at..at + 4] == b"fmt " {
            align = u16::from_le_bytes(bytes[at + 20..at + 22].try_into().unwrap()) as usize;
        }
        if &bytes[at..at + 4] == b"data" && align > 0 {
            return Ok((size / align) as u64);
        }
        at += 8 + size + size % 2;
    }
    Err("missing PCM frame count".into())
}

pub fn validate_audio(bytes: &[u8]) -> Result<(), String> {
    if bytes.starts_with(b"OggS") {
        return validate_vorbis(bytes).map(|_| ());
    }
    if !bytes.starts_with(b"fLaC") {
        return validate_wav(bytes);
    }
    if bytes.len() < 42
        || bytes.len() > MAX_WAV_BYTES
        || bytes[4] & 0x7f != 0
        || bytes[5..8] != [0, 0, 34]
    {
        return Err("invalid FLAC STREAMINFO".into());
    }
    let word = u64::from_be_bytes(bytes[18..26].try_into().unwrap());
    let rate = word >> 44;
    let channels = ((word >> 41) & 7) + 1;
    let bits = ((word >> 36) & 31) + 1;
    let frames = word & 0xf_ffff_ffff;
    if !(8000..=192000).contains(&rate)
        || channels > 2
        || bits > 32
        || frames == 0
        || frames * channels * 2 > (MAX_WAV_BYTES - 44) as u64
    {
        return Err("FLAC decoded track exceeds sound limits".into());
    }
    Ok(())
}

fn validate_vorbis(bytes: &[u8]) -> Result<u64, String> {
    if bytes.len() > MAX_WAV_BYTES {
        return Err("Vorbis exceeds compressed sound limit".into());
    }
    let mut at = 0usize;
    let mut serial = None;
    let mut frames = None;
    let mut rate = 0u32;
    let mut channels = 0u8;
    while at < bytes.len() {
        let header = bytes.get(at..at + 27).ok_or("truncated Ogg page")?;
        if &header[..4] != b"OggS" || header[4] != 0 {
            return Err("invalid Ogg page".into());
        }
        let id = u32::from_le_bytes(header[14..18].try_into().unwrap());
        if serial.is_some_and(|s| s != id) {
            return Err("chained Ogg streams unsupported".into());
        }
        serial = Some(id);
        let table_end = at + 27 + header[26] as usize;
        let table = bytes
            .get(at + 27..table_end)
            .ok_or("truncated Ogg segment table")?;
        let size: usize = table.iter().map(|&n| n as usize).sum();
        let body = bytes
            .get(table_end..table_end + size)
            .ok_or("truncated Ogg body")?;
        if at == 0 {
            if header[5] & 2 == 0
                || body.len() < 30
                || &body[..7] != b"\x01vorbis"
                || body[7..11] != [0; 4]
            {
                return Err("expected Vorbis identification header".into());
            }
            channels = body[11];
            rate = u32::from_le_bytes(body[12..16].try_into().unwrap());
        }
        if header[5] & 4 != 0 {
            frames = Some(u64::from_le_bytes(header[6..14].try_into().unwrap()));
            if table_end + size != bytes.len() {
                return Err("data after Ogg end page".into());
            }
        }
        at = table_end + size;
    }
    let frames = frames.ok_or("missing Ogg end page")?;
    if !(8000..=192000).contains(&rate)
        || !(1..=2).contains(&channels)
        || frames == 0
        || frames > ((MAX_WAV_BYTES - 44) / 2 / channels as usize) as u64
    {
        return Err("Vorbis decoded track exceeds sound limits".into());
    }
    Ok(frames)
}

/// Legacy PCM source validation used by offline sound-graph rendering.
pub fn validate_wav(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > MAX_WAV_BYTES {
        return Err("WAV exceeds the 64 MiB emote sound limit".into());
    }
    if bytes.len() < 44 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("expected decoded RIFF/WAVE audio".into());
    }
    let size = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize + 8;
    if size != bytes.len() {
        return Err("WAV RIFF length mismatch".into());
    }
    let mut at = 12;
    let mut format = None;
    let mut data_size = None;
    while at < bytes.len() {
        if at + 8 > bytes.len() {
            return Err("truncated WAV chunk".into());
        }
        let n = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let start = at + 8;
        let end = start
            .checked_add(n)
            .filter(|&n| n <= bytes.len())
            .ok_or("WAV chunk outside file")?;
        match &bytes[at..at + 4] {
            b"fmt " => {
                if n < 16 {
                    return Err("short WAV format".into());
                }
                let u16_at = |offset| {
                    u16::from_le_bytes(
                        bytes[start + offset..start + offset + 2]
                            .try_into()
                            .unwrap(),
                    )
                };
                let encoding = u16_at(0);
                let channels = u16_at(2);
                let rate = u32::from_le_bytes(bytes[start + 4..start + 8].try_into().unwrap());
                let align = u16_at(12);
                let bits = u16_at(14);
                if !matches!(encoding, 1 | 3)
                    || !(1..=2).contains(&channels)
                    || !(8000..=192000).contains(&rate)
                    || !matches!(bits, 8 | 16 | 24 | 32)
                    || (encoding == 3 && bits != 32)
                    || align != channels * (bits / 8)
                {
                    return Err("unsupported WAV sample format".into());
                }
                format = Some(align as usize);
            }
            b"data" => data_size = Some(n),
            _ => {}
        }
        at = end + (n & 1);
    }
    match (format, data_size) {
        (Some(align), Some(n)) if n > 0 && n % align == 0 && n / align <= 8 * 1024 * 1024 => Ok(()),
        _ => Err("WAV needs nonempty aligned PCM samples and fmt chunk".into()),
    }
}

/// Creates a deterministic ZIP container, then verifies it using the runtime reader.
/// An existing output is never overwritten. Failed compilation leaves no final file.
pub fn write_bundle(
    path: &Path,
    root: &Value,
    members: &BTreeMap<String, Vec<u8>>,
) -> Result<(), String> {
    write_bundle_impl(path, root, members, false)
}
pub fn write_shipping_bundle(
    path: &Path,
    root: &Value,
    members: &BTreeMap<String, Vec<u8>>,
) -> Result<(), String> {
    write_bundle_impl(path, root, members, true)
}
fn write_bundle_impl(
    path: &Path,
    root: &Value,
    members: &BTreeMap<String, Vec<u8>>,
    shipping: bool,
) -> Result<(), String> {
    if path.exists() {
        return Err(format!("output already exists: {}", path.display()));
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .ok_or("missing output filename")?
            .to_string_lossy(),
        std::process::id()
    ));
    let file = File::options()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| e.to_string())?;
    let result = (|| {
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(if shipping {
                zip::CompressionMethod::Zstd
            } else {
                zip::CompressionMethod::Deflated
            })
            .compression_level(Some(if shipping { 15 } else { 6 }));
        zip.start_file(
            "manifest.json",
            if shipping {
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored)
            } else {
                opts
            },
        )
        .map_err(|e| e.to_string())?;
        zip.write_all(&serde_json::to_vec_pretty(root).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        for (name, bytes) in members {
            manifest::qpath(name)?;
            if name.eq_ignore_ascii_case("manifest.json") {
                return Err("reserved member name manifest.json".into());
            }
            let member_opts = if bytes.starts_with(b"fLaC") || bytes.starts_with(b"OggS") {
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored)
            } else {
                opts
            };
            zip.start_file(name, member_opts)
                .map_err(|e| e.to_string())?;
            zip.write_all(bytes).map_err(|e| e.to_string())?;
        }
        zip.finish()
            .map_err(|e| e.to_string())?
            .sync_all()
            .map_err(|e| e.to_string())?;
        Bundle::open(&temp)?;
        // Hard-link gives create-new semantics even if another writer raced us.
        std::fs::hard_link(&temp, path).map_err(|e| e.to_string())?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&temp);
    result
}
