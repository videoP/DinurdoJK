//! Compact local TRS dictionary plus exact affine residuals on the DJKPOSE3 grid.
//! Residuals retain shear, singular transforms and every shipped matrix bit.
use super::PoseClip;
use glam::{DMat4, DVec4};
use jka_assets::ghoul2::{GlaAnimation, Matrix3x4};
use std::collections::HashMap;

const RECORD: usize = 24;
const Q: f64 = 32767.0;
const T: f64 = 1024.0;
const S: f64 = 4096.0;

#[derive(Default, Debug)]
pub struct Stats {
    pub samples: usize,
    pub transforms: usize,
    pub absolute_predictions: usize,
    pub residual_bytes: usize,
}

fn matrix(m: &Matrix3x4) -> DMat4 {
    DMat4::from_cols(
        DVec4::new(m[0][0] as f64, m[1][0] as f64, m[2][0] as f64, 0.0),
        DVec4::new(m[0][1] as f64, m[1][1] as f64, m[2][1] as f64, 0.0),
        DVec4::new(m[0][2] as f64, m[1][2] as f64, m[2][2] as f64, 0.0),
        DVec4::new(m[0][3] as f64, m[1][3] as f64, m[2][3] as f64, 1.0),
    )
}

fn order(parents: &[i32]) -> Result<Vec<usize>, String> {
    let mut result = Vec::with_capacity(parents.len());
    let mut children = vec![Vec::new(); parents.len()];
    for (b, &p) in parents.iter().enumerate() {
        if p < -1 || p >= parents.len() as i32 {
            return Err("pooled pose parent outside skeleton".into());
        }
        if p >= 0 {
            children[p as usize].push(b);
        } else {
            result.push(b);
        }
    }
    let mut next = 0;
    while next < result.len() {
        let b = result[next];
        result.extend_from_slice(&children[b]);
        next += 1;
    }
    if result.len() != parents.len() {
        return Err("pooled pose parent cycle".into());
    }
    Ok(result)
}

fn pack(local: DMat4) -> [u8; RECORD] {
    let mut out = [0u8; RECORD];
    if !local.is_finite() || local.determinant().abs() < 1e-12 {
        out[0] = 1;
        return out;
    }
    let (s, mut q, t) = local.to_scale_rotation_translation();
    if !s.is_finite() || !q.is_finite() || !t.is_finite() {
        out[0] = 1;
        return out;
    }
    q = q.normalize();
    if !q.is_finite() {
        out[0] = 1;
        return out;
    }
    if q.w < 0.0 {
        q = -q;
    }
    for (i, v) in q.to_array().iter().enumerate() {
        out[1 + i * 2..3 + i * 2].copy_from_slice(&((v * Q).round() as i16).to_le_bytes());
    }
    for (i, v) in t
        .to_array()
        .into_iter()
        .map(|v| v * T)
        .chain(s.to_array().into_iter().map(|v| (v - 1.0) * S))
        .enumerate()
    {
        let (low, high) = if i < 3 {
            (-8388608.0, 8388607.0)
        } else {
            (-32768.0, 32767.0)
        };
        if !v.is_finite() || v.round() < low || v.round() > high {
            let mut escape = [0u8; RECORD];
            escape[0] = 1;
            return escape;
        }
        if i < 3 {
            out[9 + i * 3..12 + i * 3].copy_from_slice(&(v.round() as i32).to_le_bytes()[..3]);
        } else {
            out[18 + (i - 3) * 2..20 + (i - 3) * 2]
                .copy_from_slice(&(v.round() as i16).to_le_bytes());
        }
    }
    out
}

#[inline(always)]
fn unpack(record: &[u8]) -> [[f64; 4]; 3] {
    if record[0] == 1 {
        return [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ];
    }
    let q: [f64; 4] = std::array::from_fn(|i| {
        i16::from_le_bytes(record[1 + i * 2..3 + i * 2].try_into().unwrap()) as f64 / Q
    });
    let values: [f64; 6] = std::array::from_fn(|i| {
        if i < 3 {
            let at = 9 + i * 3;
            let v = i32::from_le_bytes([
                record[at],
                record[at + 1],
                record[at + 2],
                if record[at + 2] & 128 != 0 { 255 } else { 0 },
            ]);
            v as f64 / T
        } else {
            let at = 18 + (i - 3) * 2;
            1.0 + i16::from_le_bytes(record[at..at + 2].try_into().unwrap()) as f64 / S
        }
    });
    let [x, y, z, w] = q;
    [
        [
            (1.0 - 2.0 * (y * y + z * z)) * values[3],
            2.0 * (x * y - z * w) * values[4],
            2.0 * (x * z + y * w) * values[5],
            values[0],
        ],
        [
            2.0 * (x * y + z * w) * values[3],
            (1.0 - 2.0 * (x * x + z * z)) * values[4],
            2.0 * (y * z - x * w) * values[5],
            values[1],
        ],
        [
            2.0 * (x * z - y * w) * values[3],
            2.0 * (y * z + x * w) * values[4],
            (1.0 - 2.0 * (x * x + y * y)) * values[5],
            values[2],
        ],
    ]
}

#[inline(always)]
fn predict(record: &[u8], parent: Option<&Matrix3x4>) -> [[f64; 4]; 3] {
    let local = unpack(record);
    if record[0] == 1 || parent.is_none() {
        return local;
    }
    compose(&local, parent.unwrap())
}

#[inline(always)]
fn compose(local: &[[f64; 4]; 3], p: &Matrix3x4) -> [[f64; 4]; 3] {
    std::array::from_fn(|r| {
        std::array::from_fn(|c| {
            p[r][0] as f64 * local[0][c]
                + p[r][1] as f64 * local[1][c]
                + p[r][2] as f64 * local[2][c]
                + if c == 3 { p[r][3] as f64 } else { 0.0 }
        })
    })
}

// Valid records and bounded parent coefficients keep predictions below 2^50.
// Truncate and inspect the exact fractional remainder to implement round's
// ties-away rule without a per-coefficient CRT rounding call on SSE2 targets.
#[inline(always)]
fn round_prediction(value: f64) -> i64 {
    let integer = value as i64;
    let fraction = value - integer as f64;
    integer + i64::from(fraction >= 0.5) - i64::from(fraction <= -0.5)
}

fn put_varint(out: &mut Vec<u8>, value: i64) {
    let mut n = ((value << 1) ^ (value >> 63)) as u64;
    while n >= 128 {
        out.push(n as u8 | 128);
        n >>= 7;
    }
    out.push(n as u8);
}
fn get_varint(bytes: &[u8], cursor: &mut usize, end: usize) -> Result<i64, String> {
    let mut n = 0u64;
    for shift in (0..70).step_by(7) {
        if *cursor >= end {
            return Err("truncated pooled pose residual".into());
        }
        let b = bytes[*cursor];
        *cursor += 1;
        if shift == 63 && b > 1 {
            return Err("pooled pose residual overflow".into());
        }
        n |= ((b & 127) as u64) << shift;
        if b < 128 {
            return Ok(((n >> 1) as i64) ^ -((n & 1) as i64));
        }
    }
    Err("invalid pooled pose residual".into())
}
fn grid(c: usize) -> f64 {
    if c == 3 {
        1000.0
    } else {
        100000.0
    }
}

/// Encode only when the input is exactly representable on the existing shipping
/// grid. The packager keeps the original member for any other input.
pub fn encode(clip: &PoseClip, gla: &GlaAnimation) -> Result<(Vec<u8>, Stats), String> {
    if clip.bones != gla.skeleton.len() {
        return Err("pooled pose skeleton mismatch".into());
    }
    let parents: Vec<_> = gla.skeleton.iter().map(|b| b.parent).collect();
    let order = order(&parents)?;
    let count = clip.frames * clip.bones;
    let mut pool = Vec::<[u8; RECORD]>::new();
    let mut lookup = HashMap::new();
    let mut indices = vec![0u32; count];
    let mut residuals = vec![Vec::<u8>::new(); clip.bones * 12];
    let mut stats = Stats {
        samples: count,
        ..Stats::default()
    };
    for &bone in &order {
        for frame in 0..clip.frames {
            let source = &clip.matrices()[frame * clip.bones..(frame + 1) * clip.bones];
            let parent = (parents[bone] >= 0).then(|| &source[parents[bone] as usize]);
            let local = if let Some(p) = parent {
                let p = matrix(p);
                if p.determinant().abs() < 1e-12 {
                    DMat4::ZERO
                } else {
                    p.inverse() * matrix(&source[bone])
                }
            } else {
                matrix(&source[bone])
            };
            let record = pack(local);
            stats.absolute_predictions += usize::from(record[0] == 1);
            let predicted = predict(&record, parent);
            let next = pool.len() as u32;
            let index = *lookup.entry(record).or_insert_with(|| {
                pool.push(record);
                next
            });
            if index > 0xffffff {
                return Err("pooled pose transform pool too large".into());
            }
            indices[frame * clip.bones + bone] = index;
            for r in 0..3 {
                for c in 0..4 {
                    let scale = grid(c);
                    let value = source[bone][r][c];
                    let integer = (value as f64 * scale).round() as i64;
                    if (integer as f32 / scale as f32).to_bits() != value.to_bits() {
                        return Err("pooled pose input is not on the exact shipping grid".into());
                    }
                    let p = (predicted[r][c] * scale).round();
                    if !p.is_finite() || p.abs() > 1e15 {
                        return Err("pooled pose predictor outside bounds".into());
                    }
                    put_varint(&mut residuals[bone * 12 + r * 4 + c], integer - p as i64);
                }
            }
        }
    }
    stats.transforms = pool.len();
    let mut out = Vec::new();
    out.extend_from_slice(b"DJKPOSE4");
    for value in [clip.frames as u32, clip.bones as u32, pool.len() as u32] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for p in parents {
        out.extend_from_slice(&p.to_le_bytes());
    }
    let indices_at = out.len();
    out.resize(indices_at + count * 3, 0);
    for bone in 0..clip.bones {
        let mut previous = 0u32;
        for frame in 0..clip.frames {
            let index = indices[frame * clip.bones + bone];
            let delta = index.wrapping_sub(previous);
            previous = index;
            for plane in 0..3 {
                out[indices_at + (bone * 3 + plane) * clip.frames + frame] =
                    delta.to_le_bytes()[plane];
            }
        }
    }
    for lane in 0..RECORD {
        let mut previous = 0u8;
        for record in &pool {
            out.push(record[lane].wrapping_sub(previous));
            previous = record[lane];
        }
    }
    for stream in &residuals {
        out.extend_from_slice(&(stream.len() as u32).to_le_bytes());
    }
    for stream in residuals {
        stats.residual_bytes += stream.len();
        out.extend_from_slice(&stream);
    }
    if out.len() > crate::MAX_MEMBER_BYTES {
        return Err("pooled pose member exceeds size limit".into());
    }
    Ok((out, stats))
}

pub(super) fn decode(bytes: &[u8]) -> Result<PoseClip, String> {
    let read = |at: usize| -> Result<usize, String> {
        bytes
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize)
            .ok_or("truncated pooled pose header".into())
    };
    let frames = read(8)?;
    let bones = read(12)?;
    let pool_count = read(16)?;
    if frames == 0 || bones == 0 || bones > 4096 || frames > 1_000_000 {
        return Err("invalid pooled pose dimensions".into());
    }
    let count = frames
        .checked_mul(bones)
        .ok_or("pooled pose size overflow")?;
    if count
        .checked_mul(48)
        .and_then(|n| n.checked_add(16))
        .is_none_or(|n| n > crate::MAX_MEMBER_BYTES)
    {
        return Err("decoded pooled pose exceeds size limit".into());
    }
    if pool_count == 0 || pool_count > count || pool_count > 0x1000000 {
        return Err("invalid pooled pose pool count".into());
    }
    let indices_at = 20 + bones * 4;
    let pool_at = indices_at + count * 3;
    let lengths_at = pool_at + pool_count * RECORD;
    let residual_at = lengths_at + bones * 12 * 4;
    if residual_at > bytes.len() {
        return Err("pooled pose data truncated".into());
    }
    let parents: Vec<i32> = (0..bones)
        .map(|b| read(20 + b * 4).map(|p| p as u32 as i32))
        .collect::<Result<_, _>>()?;
    let order = order(&parents)?;
    let mut cursors = Vec::with_capacity(bones * 12);
    let mut ends = Vec::with_capacity(bones * 12);
    let mut at = residual_at;
    for ch in 0..bones * 12 {
        cursors.push(at);
        at = at
            .checked_add(read(lengths_at + ch * 4)?)
            .filter(|&n| n <= bytes.len())
            .ok_or("pooled pose residual outside data")?;
        ends.push(at);
    }
    if at != bytes.len() {
        return Err("pooled pose trailing bytes".into());
    }
    let mut pool = vec![[0u8; RECORD]; pool_count];
    for lane in 0..RECORD {
        let mut previous = 0u8;
        for (i, record) in pool.iter_mut().enumerate() {
            previous = previous.wrapping_add(bytes[pool_at + lane * pool_count + i]);
            record[lane] = previous;
        }
    }
    if pool.iter().any(|record| record[0] > 1) {
        return Err("invalid pooled pose prediction mode".into());
    }
    let mut matrices = vec![[[0.0f32; 4]; 3]; count];
    for &bone in &order {
        let starts: [usize; 12] = std::array::from_fn(|ch| cursors[bone * 12 + ch]);
        let simple = (0..12).all(|ch| ends[bone * 12 + ch] - starts[ch] == frames)
            && (0..12).all(|ch| {
                bytes[starts[ch]..ends[bone * 12 + ch]]
                    .iter()
                    .all(|&b| b < 128)
            });
        let mut previous_index = 0u32;
        let mut cached_index = usize::MAX;
        let mut cached_local = [[0.0f64; 4]; 3];
        for frame in 0..frames {
            let sample = frame * bones + bone;
            let delta = u32::from_le_bytes(std::array::from_fn(|plane| {
                if plane < 3 {
                    bytes[indices_at + (bone * 3 + plane) * frames + frame]
                } else {
                    0
                }
            }));
            let index = previous_index.wrapping_add(delta) & 0xffffff;
            previous_index = index;
            let index = index as usize;
            if index >= pool_count {
                return Err("pooled pose transform index outside pool".into());
            }
            let parent =
                (parents[bone] >= 0).then(|| &matrices[frame * bones + parents[bone] as usize]);
            if index != cached_index {
                cached_local = unpack(&pool[index]);
                cached_index = index;
            }
            let predicted = match parent {
                Some(p) if pool[index][0] == 0 => compose(&cached_local, p),
                _ => cached_local,
            };
            let mut m = [[0.0f32; 4]; 3];
            if simple {
                for r in 0..3 {
                    for c in 0..4 {
                        let p = round_prediction(predicted[r][c] * grid(c));
                        let b = bytes[starts[r * 4 + c] + frame];
                        let residual = ((b >> 1) as i64) ^ -((b & 1) as i64);
                        m[r][c] = (p + residual) as f32 / grid(c) as f32;
                    }
                }
            } else {
                for r in 0..3 {
                    for c in 0..4 {
                        let ch = bone * 12 + r * 4 + c;
                        let p = round_prediction(predicted[r][c] * grid(c));
                        let residual = get_varint(bytes, &mut cursors[ch], ends[ch])?;
                        let value = p
                            .checked_add(residual)
                            .ok_or("pooled pose coefficient overflow")?;
                        m[r][c] = value as f32 / grid(c) as f32;
                    }
                }
            }
            if m.iter()
                .flatten()
                .any(|f| !f.is_finite() || f.abs() > 10000.0)
            {
                return Err("invalid pooled pose coefficient".into());
            }
            matrices[sample] = m;
        }
        if simple {
            cursors[bone * 12..bone * 12 + 12].copy_from_slice(&ends[bone * 12..bone * 12 + 12]);
        }
    }
    if cursors != ends {
        return Err("pooled pose unused residual bytes".into());
    }
    Ok(PoseClip {
        frames,
        bones,
        matrices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fast_rounding_preserves_ties_and_adjacent_floats() {
        for integer in -10000..10000 {
            for fraction in [
                -0.999999999,
                -0.5,
                -0.499999999,
                0.0,
                0.499999999,
                0.5,
                0.999999999,
            ] {
                let v = integer as f64 + fraction;
                assert_eq!(round_prediction(v), v.round() as i64);
            }
        }
        for v in [0.5f64, -0.5, 1.5, -1.5, 1e14, -1e14] {
            for bits in [v.to_bits() - 1, v.to_bits(), v.to_bits() + 1] {
                let n = f64::from_bits(bits);
                assert_eq!(round_prediction(n), n.round() as i64);
            }
        }
    }
    fn fixture() -> (PoseClip, GlaAnimation) {
        let mut gla = jka_assets::ghoul2::openjk_default_gla().unwrap();
        gla.skeleton = vec![gla.skeleton[0].clone(); 3];
        // The real JK2 skeleton also contains parents after their children.
        gla.skeleton[0].parent = 2;
        gla.skeleton[1].parent = 0;
        gla.skeleton[2].parent = -1;
        let mut matrices = Vec::new();
        for frame in 0..6 {
            for bone in 0..3 {
                let scale = if frame == 2 {
                    0.0
                } else {
                    1.0 + frame as f32 * 0.07
                };
                matrices.push([
                    [scale, 0.17, 0.0, frame as f32 * 0.13],
                    [0.0, -1.2, 0.03, bone as f32 * 0.31],
                    [0.01, 0.0, 1.4, 2.0],
                ]);
            }
        }
        let raw = PoseClip::encode(6, 3, &matrices).unwrap();
        (
            PoseClip::parse(&PoseClip::pack_compact(&raw).unwrap()).unwrap(),
            gla,
        )
    }
    #[test]
    fn affine_residual_restores_scale_shear_singular_and_interpolated_poses_exactly() {
        let (clip, gla) = fixture();
        let (bytes, _) = encode(&clip, &gla).unwrap();
        let restored = PoseClip::parse(&bytes).unwrap();
        for (a, b) in clip
            .matrices()
            .iter()
            .flatten()
            .flatten()
            .zip(restored.matrices().iter().flatten().flatten())
        {
            assert_eq!(a.to_bits(), b.to_bits());
        }
        for ms in [0, 1, 16, 33, 80, 145, 199, 333] {
            for interpolation in [
                super::super::Interpolation::Linear,
                super::super::Interpolation::Cubic,
            ] {
                assert_eq!(
                    clip.sample_range_interpolated(ms, 30.0, 1, 6, Some(2), interpolation),
                    restored.sample_range_interpolated(ms, 30.0, 1, 6, Some(2), interpolation)
                );
            }
        }
    }
    #[test]
    fn absolute_prediction_preserves_collapsed_and_out_of_range_transforms() {
        let (mut clip, gla) = fixture();
        clip.matrices[2] = [[0.0; 4]; 3];
        clip.matrices[0][0][3] = 9000.0;
        let (bytes, stats) = encode(&clip, &gla).unwrap();
        assert!(stats.absolute_predictions > 0);
        let restored = decode(&bytes).unwrap();
        assert_eq!(clip.matrices(), restored.matrices());
    }
    #[test]
    fn off_grid_input_uses_the_callers_legacy_fallback() {
        let (mut clip, gla) = fixture();
        clip.matrices[0][0][0] = 0.12345678;
        assert!(encode(&clip, &gla)
            .unwrap_err()
            .contains("exact shipping grid"));
    }
    #[test]
    fn malformed_dimensions_hierarchy_indices_and_residuals_are_rejected() {
        let (clip, gla) = fixture();
        let (bytes, stats) = encode(&clip, &gla).unwrap();
        for len in 0..bytes.len() {
            assert!(PoseClip::parse(&bytes[..len]).is_err());
        }
        let mut bad = bytes.clone();
        bad[8..12].copy_from_slice(&1_000_000u32.to_le_bytes());
        assert!(decode(&bad).is_err());
        let mut bad = bytes.clone();
        bad[20..24].copy_from_slice(&0i32.to_le_bytes());
        assert!(decode(&bad).is_err());
        let mut bad = bytes.clone();
        let indices = 20 + clip.bones * 4;
        bad[indices] = 255;
        assert!(decode(&bad).is_err());
        let mut bad = bytes.clone();
        let pool = indices + clip.frames * clip.bones * 3;
        bad[pool] = 2;
        assert!(decode(&bad).is_err());
        let mut bad = bytes.clone();
        let lengths = pool + stats.transforms * RECORD;
        bad[lengths..lengths + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&bad).is_err());
        let mut bad = bytes;
        bad.push(0);
        assert!(decode(&bad).is_err());
        assert!(get_varint(&[255; 10], &mut 0, 10).is_err());
    }
}
