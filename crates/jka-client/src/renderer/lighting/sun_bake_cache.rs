//! Versioned, bounded, checksummed lossless PNG container for sunlight results.
use super::original_sun::BakedSunCalibration;
use super::sun_bake::BakeData;
use image::{ImageDecoder, ImageEncoder};
use std::{hash::Hasher, io::Cursor, path::Path};

const MAGIC: &[u8; 8] = b"JKASUN01";
const ROW: u32 = 4096;

fn checksum(bytes: &[u8]) -> u64 {
    let mut hash = crate::scene::FoldHasher::default();
    hash.write(bytes);
    hash.finish()
}

pub(super) fn encode(key: u64, data: &BakeData) -> Result<Vec<u8>, String> {
    let mut raw = Vec::new();
    for value in data.calibration.radiance {
        raw.extend_from_slice(&value.to_le_bytes());
    }
    raw.extend_from_slice(&(data.calibration.surfaces.len() as u32).to_le_bytes());
    for fit in &data.calibration.surfaces {
        for value in fit {
            raw.extend_from_slice(&value.to_le_bytes());
        }
    }
    let masks = data.masks.as_deref().unwrap_or(&[]);
    raw.extend_from_slice(&(masks.len() as u32).to_le_bytes());
    for mask in masks {
        raw.extend_from_slice(&(mask.len() as u64).to_le_bytes());
        raw.extend_from_slice(mask);
    }
    let raw_len = raw.len();
    let sum = checksum(&raw);
    let height = raw_len.div_ceil(ROW as usize) as u32;
    raw.resize(ROW as usize * height as usize, 0);
    let mut file = Vec::new();
    file.extend_from_slice(MAGIC);
    file.extend_from_slice(&key.to_le_bytes());
    file.extend_from_slice(&(raw_len as u64).to_le_bytes());
    file.extend_from_slice(&sum.to_le_bytes());
    image::codecs::png::PngEncoder::new_with_quality(
        &mut file,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::NoFilter,
    )
    .write_image(&raw, ROW, height, image::ExtendedColorType::L8)
    .map_err(|error| error.to_string())?;
    Ok(file)
}

fn max_payload(surface_count: usize, sizes: &[[u32; 2]]) -> Option<usize> {
    sizes.iter().try_fold(
        20usize.checked_add(surface_count.checked_mul(16)?)?,
        |bytes, size| {
            bytes
                .checked_add(8)?
                .checked_add((size[0] as usize).checked_mul(size[1] as usize)?)
        },
    )
}

struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, size: usize) -> Option<&'a [u8]> {
        let value = self.0.get(..size)?;
        self.0 = &self.0[size..];
        Some(value)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn float(&mut self) -> Option<f32> {
        let value = f32::from_bits(self.u32()?);
        value.is_finite().then_some(value)
    }
}

pub(super) fn decode(
    file: &[u8],
    key: u64,
    surfaces: usize,
    sizes: &[[u32; 2]],
) -> Option<BakeData> {
    let mut header = Reader(file);
    if header.take(8)? != MAGIC || header.u64()? != key {
        return None;
    }
    let len = usize::try_from(header.u64()?).ok()?;
    let sum = header.u64()?;
    if len < 20 || len > max_payload(surfaces, sizes)? {
        return None;
    }
    let decoder = image::codecs::png::PngDecoder::new(Cursor::new(header.0)).ok()?;
    let height = u32::try_from(len.div_ceil(ROW as usize)).ok()?;
    if decoder.dimensions() != (ROW, height) || decoder.color_type() != image::ColorType::L8 {
        return None;
    }
    let mut raw = vec![0; ROW as usize * height as usize];
    decoder.read_image(&mut raw).ok()?;
    raw.truncate(len);
    if checksum(&raw) != sum {
        return None;
    }
    let mut reader = Reader(&raw);
    let radiance = [reader.float()?, reader.float()?, reader.float()?];
    if reader.u32()? as usize != surfaces {
        return None;
    }
    let mut fits = Vec::with_capacity(surfaces);
    for _ in 0..surfaces {
        fits.push([
            reader.float()?,
            reader.float()?,
            reader.float()?,
            reader.float()?,
        ]);
    }
    let count = reader.u32()? as usize;
    if count != 0 && count != sizes.len() {
        return None;
    }
    let masks = if count == 0 {
        None
    } else {
        let mut masks = Vec::with_capacity(count);
        for size in sizes {
            let expected = (size[0] as usize).checked_mul(size[1] as usize)?;
            if reader.u64()? != expected as u64 {
                return None;
            }
            masks.push(reader.take(expected)?.to_vec());
        }
        Some(masks)
    };
    if !reader.0.is_empty() {
        return None;
    }
    Some(BakeData {
        calibration: BakedSunCalibration {
            radiance,
            surfaces: fits,
        },
        masks,
    })
}

pub(super) fn read(path: &Path, key: u64, surfaces: usize, sizes: &[[u32; 2]]) -> Option<BakeData> {
    let limit = max_payload(surfaces, sizes)?
        .checked_mul(2)?
        .checked_add(1 << 20)?;
    if std::fs::metadata(path).ok()?.len() > limit as u64 {
        return None;
    }
    decode(&std::fs::read(path).ok()?, key, surfaces, sizes)
}

pub(super) fn write(path: &Path, key: u64, data: &BakeData) -> Result<usize, String> {
    use std::{
        fs,
        io::Write,
        sync::atomic::{AtomicU64, Ordering},
    };
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let file = encode(key, data)?;
    fs::create_dir_all(path.parent().ok_or("missing sunlight cache directory")?)
        .map_err(|e| e.to_string())?;
    let temporary = path.with_extension(format!(
        "{}-{}.tmp",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut output = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        output.write_all(&file)?;
        drop(output);
        fs::rename(&temporary, path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    Ok(file.len())
}
