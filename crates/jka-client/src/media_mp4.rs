//! Minimal, allocation-free ISO BMFF metadata probe for H.264 `videoMap` promotions.
//!
//! This is not an MP4 decoder. In-process H.264 decoding uses the alhazen-core
//! crate on a worker through Windows Media Foundation. Dimensions and timing
//! are probed here so WGPU can allocate a texture before playback begins.

#[derive(Debug, Clone, Copy)]
pub struct Mp4Info {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
}

fn be_u32(data: &[u8]) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(..4)?.try_into().ok()?))
}

fn next_box(data: &[u8], offset: usize) -> Option<([u8; 4], &[u8], usize)> {
    let header = data.get(offset..offset.checked_add(8)?)?;
    let size32 = be_u32(header)?;
    let kind: [u8; 4] = header[4..8].try_into().ok()?;
    let (size, header_len) = match size32 {
        0 => (data.len().checked_sub(offset)?, 8),
        1 => {
            let bytes: [u8; 8] = data.get(offset + 8..offset + 16)?.try_into().ok()?;
            (usize::try_from(u64::from_be_bytes(bytes)).ok()?, 16)
        }
        n => (n as usize, 8),
    };
    if size < header_len {
        return None;
    }
    let end = offset.checked_add(size)?;
    Some((kind, data.get(offset + header_len..end)?, end))
}

fn find_box<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    let mut offset = 0;
    while offset < data.len() {
        let (tag, bytes, next) = next_box(data, offset)?;
        if &tag == kind {
            return Some(bytes);
        }
        offset = next;
    }
    None
}

fn h264_dimensions(stsd: &[u8]) -> Option<(u32, u32)> {
    // FullBox(4), entry_count(4), then ISO VisualSampleEntry records.
    let count = be_u32(stsd.get(4..8)?)?.min(32);
    let mut offset = 8;
    for _ in 0..count {
        let (tag, record, next) = next_box(stsd, offset)?;
        if &tag == b"avc1" || &tag == b"avc3" {
            // VisualSampleEntry: reserved(6), data_reference_index(2),
            // predefined/reserved(16), width(2), height(2).
            let w = record.get(24..26)?;
            let h = record.get(26..28)?;
            let width = u16::from_be_bytes([w[0], w[1]]) as u32;
            let height = u16::from_be_bytes([h[0], h[1]]) as u32;
            return Some((width, height));
        }
        offset = next;
    }
    None
}

fn video_track(mdia: &[u8]) -> Option<Mp4Info> {
    let handler = find_box(mdia, b"hdlr")?;
    if handler.get(8..12)? != b"vide" {
        return None;
    }
    let mdhd = find_box(mdia, b"mdhd")?;
    let timescale_at = if *mdhd.first()? == 1 { 20 } else { 12 };
    let timescale = be_u32(mdhd.get(timescale_at..timescale_at + 4)?)?;
    if timescale == 0 {
        return None;
    }
    let stbl = find_box(find_box(mdia, b"minf")?, b"stbl")?;
    let (width, height) = h264_dimensions(find_box(stbl, b"stsd")?)?;
    let stts = find_box(stbl, b"stts")?;
    let count = be_u32(stts.get(4..8)?)? as usize;
    // Bound work on adversarial map assets; ordinary MP4s use one entry.
    if count == 0 || count > 65_536 {
        return None;
    }
    let mut frames = 0u64;
    let mut ticks = 0u64;
    for i in 0..count {
        let at = 8usize.checked_add(i.checked_mul(8)?)?;
        let n = u64::from(be_u32(stts.get(at..at + 4)?)?);
        let duration = u64::from(be_u32(stts.get(at + 4..at + 8)?)?);
        frames = frames.checked_add(n)?;
        ticks = ticks.checked_add(n.checked_mul(duration)?)?;
    }
    if ticks == 0 || frames == 0 {
        return None;
    }
    let fps = frames as f64 * f64::from(timescale) / ticks as f64;
    if width == 0 || height == 0 || width > 4096 || height > 4096
        || !(1.0..=120.0).contains(&fps)
    {
        return None;
    }
    Some(Mp4Info { width, height, fps })
}

pub(crate) fn probe_h264_mp4(data: &[u8]) -> Result<Mp4Info, &'static str> {
    find_box(data, b"ftyp").ok_or("not an MP4 container")?;
    let moov = find_box(data, b"moov").ok_or("MP4 has no moov metadata")?;
    let mut offset = 0;
    while offset < moov.len() {
        let (tag, contents, next) = next_box(moov, offset).ok_or("malformed MP4 metadata")?;
        if &tag == b"trak" {
            if let Some(mdia) = find_box(contents, b"mdia") {
                if let Some(info) = video_track(mdia) {
                    return Ok(info);
                }
            }
        }
        offset = next;
    }
    Err("no supported H.264 video track with dimensions and frame timing")
}

/// Emergency fallback when MP4 and RoQ have different resolutions.
/// Only used after MP4 decoding fails, never during normal HD playback.
pub(crate) fn resize_rgba_nearest(
    src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32,
) -> Vec<u8> {
    let mut out = vec![0u8; dw as usize * dh as usize * 4];
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 { return out; }
    for y in 0..dh as usize {
        let sy = y * sh as usize / dh as usize;
        for x in 0..dw as usize {
            let sx = x * sw as usize / dw as usize;
            let from = (sy * sw as usize + sx) * 4;
            let to = (y * dw as usize + x) * 4;
            if let Some(p) = src.get(from..from+4) {
                out[to..to+4].copy_from_slice(p);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(tag: &[u8; 4], content: &[u8]) -> Vec<u8> {
        let mut result = ((content.len() + 8) as u32).to_be_bytes().to_vec();
        result.extend_from_slice(tag);
        result.extend_from_slice(content);
        result
    }

    #[test]
    fn reads_avc1_track_and_rate() {
        let ftyp = bx(b"ftyp", b"isom0000");
        let mut sample = vec![0u8; 28];
        sample[24..26].copy_from_slice(&1280u16.to_be_bytes());
        sample[26..28].copy_from_slice(&720u16.to_be_bytes());
        let avc1 = bx(b"avc1", &sample);
        let mut stsd = vec![0, 0, 0, 0, 0, 0, 0, 1];
        stsd.extend_from_slice(&avc1);
        let mut stts = vec![0, 0, 0, 0, 0, 0, 0, 1];
        stts.extend_from_slice(&300u32.to_be_bytes());
        stts.extend_from_slice(&1001u32.to_be_bytes());
        let mut mdhd = vec![0u8; 16];
        mdhd[12..16].copy_from_slice(&30_000u32.to_be_bytes());
        let mut hdlr = vec![0u8; 12];
        hdlr[8..12].copy_from_slice(b"vide");
        let mut stbl = bx(b"stsd", &stsd);
        stbl.extend_from_slice(&bx(b"stts", &stts));
        let mut mdia = bx(b"hdlr", &hdlr);
        mdia.extend_from_slice(&bx(b"mdhd", &mdhd));
        mdia.extend_from_slice(&bx(b"minf", &bx(b"stbl", &stbl)));
        let trak = bx(b"trak", &bx(b"mdia", &mdia));
        let mut file = ftyp;
        file.extend_from_slice(&bx(b"moov", &trak));
        let info = probe_h264_mp4(&file).unwrap();
        assert_eq!((info.width, info.height), (1280, 720));
        assert!((info.fps - 29.97003).abs() < 0.001);
    }

    #[test]
    fn reject_invalid_file() {
        assert!(probe_h264_mp4(b"garbage").is_err());
    }
}
