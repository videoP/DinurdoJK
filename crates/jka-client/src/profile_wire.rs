//! DinurdoJK client-side appearance overlay in *existing* protocol-26 color fields.
//! The numeric prefix remains the ordinary saber color. This is NOT a server
//! protocol extension: it works only if the game server relays c1/c2 verbatim.
//! Never treat a hash as a recoverable encoding of a name or of Anny values.
use jka_assets::body_shape::{anny::{Details, DETAIL_COUNT}, BodyShapeParams};
use crate::cgame::jiggle::JiggleRemeshTuning;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
const MAX_VALUE: usize = 220; // Conservative per-field safety bound, including saber digit.
const HASH_MASK: u32 = (1 << 20) - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PackedCosmetics {
    pub mask: u32,
    pub hat: u32,
    pub cape: u32,
}

/// Stable name-derived asset identifier, never an index into a client's PK3 list.
/// 0 is reserved for "none". Hash collisions are rejected when resolving assets.
pub fn cosmetic_id(name: &str) -> u32 {
    if !crate::global_cosmetics::valid_name(name) { return 0; }
    let mut hash: u32 = 0x811c9dc5;
    for byte in name.bytes().map(|b| b.to_ascii_lowercase()) {
        hash = (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193);
    }
    (hash & HASH_MASK).max(1)
}

fn encode64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity((bytes.len() * 4 + 2) / 3);
    for part in bytes.chunks(3) {
        let word = (u32::from(part[0]) << 16)
            | (u32::from(*part.get(1).unwrap_or(&0)) << 8)
            | u32::from(*part.get(2).unwrap_or(&0));
        out.push(ALPHABET[((word >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((word >> 12) & 63) as usize] as char);
        if part.len() > 1 { out.push(ALPHABET[((word >> 6) & 63) as usize] as char); }
        if part.len() > 2 { out.push(ALPHABET[(word & 63) as usize] as char); }
    }
    out
}

fn decode64(text: &[u8]) -> Option<Vec<u8>> {
    if text.is_empty() || text.len() % 4 == 1 { return None; }
    let mut data = Vec::with_capacity((text.len() * 3) / 4);
    let mut accum: u32 = 0;
    let mut bits = 0;
    for &ch in text {
        let v = ALPHABET.iter().position(|&b| b == ch)? as u32;
        accum = (accum << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            data.push((accum >> bits) as u8);
            accum &= (1 << bits) - 1;
        }
    }
    if accum != 0 { return None; } // Reject non-canonical trailing bits.
    Some(data)
}

pub fn encode_cosmetics(color: u8, value: PackedCosmetics) -> String {
    let word = (u128::from(value.mask) << 40)
        | (u128::from(value.hat & HASH_MASK) << 20)
        | u128::from(value.cape & HASH_MASK);
    let mut bytes = [0; 9];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = (word >> (8 * (8 - i))) as u8;
    }
    // 2-byte version discriminator + 12 base64url bytes = 14-char suffix.
    format!("{color}z1{}", encode64(&bytes))
}

pub fn decode_cosmetics(value: &[u8]) -> Option<(i32, PackedCosmetics)> {
    let (color, suffix) = split_color(value)?;
    let body = suffix.strip_prefix(b"z1".as_slice())?;
    if body.len() != 12 { return None; }
    let bytes = decode64(body)?;
    if bytes.len() != 9 { return None; }
    let word = bytes.iter().fold(0u128, |acc, byte| (acc << 8) | u128::from(*byte));
    Some((color, PackedCosmetics {
        mask: (word >> 40) as u32,
        hat: ((word >> 20) as u32) & HASH_MASK,
        cape: (word as u32) & HASH_MASK,
    }))
}

fn split_color(value: &[u8]) -> Option<(i32, &[u8])> {
    let digits = value.iter().take_while(|byte| byte.is_ascii_digit()).count();
    if digits == 0 || digits > 3 { return None; }
    let color = std::str::from_utf8(&value[..digits]).ok()?.parse::<i32>().ok()?;
    Some((color, &value[digits..]))
}

const CHANNELS: usize = 11;
const RANGES: [(f32, f32, f32); CHANNELS] = [
    (0., 1., 0.5), (0., 1., 0.5), (0., 1., 0.5), (0., 1., 0.5), (0., 1., 0.5),
    (-2., 3., 0.), (-2., 4., 0.),
    (-8., 8., 0.), (-8., 8., 0.), (-8., 8., 0.), (-8., 8., 0.),
];
fn quant(v: f32, lo: f32, hi: f32) -> u8 {
    (((v.clamp(lo, hi) - lo) / (hi - lo) * 63.).round() as u8).min(63)
}
fn dequant(q: u8, lo: f32, hi: f32, neutral: f32) -> f32 {
    if q == quant(neutral, lo, hi) { neutral } else { lo + f32::from(q) * (hi - lo) / 63. }
}

/// All geometric deformation controls, lossily quantized at six bits each.
/// Sparse Anny indices must be sorted/unique, and the decoder rejects malformed
/// or truncated payloads rather than applying a partial appearance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyWire {
    pub channels: [u8; CHANNELS],
    /// Two barycentric coordinates; third is inferred as 63 - a - b.
    /// None means the equal-weight default (including all-zero legacy input).
    pub ancestry: Option<[u8; 2]>,
    pub details: Vec<(u16, u8)>,
}

/// Anny normalizes the three ancestral artist-blend weights. Quantize their
/// simplex onto a 63-step triangular lattice using the largest-remainder
/// method, preserving the sum exactly and avoiding negative third weights.
fn pack_ancestry(values: [f32; 3]) -> Option<[u8; 2]> {
    // Every equal triplet renders the neutral 1/3-1/3-1/3 blend. Preserve
    // the actual neutral default on reception, without spending any bits.
    if values[0] == values[1] && values[1] == values[2] { return None; }
    let sum = values.iter().copied().sum::<f32>();
    if sum <= 0.0 { return None; }
    let normalized = values.map(|v| v.clamp(0.0, 1.0) / sum);
    let desired = normalized.map(|v| v * 63.0);
    let mut q = desired.map(|v| v.floor() as u8);
    let left = 63_u8.saturating_sub(q.iter().sum::<u8>());
    let mut order = [0usize, 1, 2];
    order.sort_by(|&i, &j| (desired[j] - f32::from(q[j]))
        .total_cmp(&(desired[i] - f32::from(q[i]))).then(i.cmp(&j)));
    for &i in order.iter().take(usize::from(left)) { q[i] += 1; }
    if q == [21, 21, 21] { None } else { Some([q[0], q[1]]) }
}
impl BodyWire {
    pub fn neutral() -> Self {
        Self { channels: std::array::from_fn(|i| {
            let (lo, hi, neutral) = RANGES[i]; quant(neutral, lo, hi)
        }), ancestry: None, details: Vec::new() }
    }
    pub fn is_neutral(&self) -> bool { *self == Self::neutral() }
    pub fn from_tuning(tuning: JiggleRemeshTuning) -> Self {
        let tuning = tuning.clamped();
        let p = tuning.body_shape;
        let values = [p.muscle, p.weight, p.proportions, p.height, p.age,
            tuning.breast_size, tuning.glute_size,
            tuning.breast_bottom_z, tuning.breast_top_z,
            tuning.glute_bottom_z, tuning.glute_top_z];
        let channels = std::array::from_fn(|i| quant(values[i], RANGES[i].0, RANGES[i].1));
        let defaults = Details::default();
        let clamped_details = tuning.body_details.clamped();
        let d = &clamped_details.0;
        let ancestry = pack_ancestry([d[3], d[4], d[5]]);
        let details = d.iter().enumerate().filter_map(|(i, &value)| {
            if (3..=5).contains(&i) { return None; } // Encoded together.
            let range = Details::range(i);
            let (lo, hi) = (*range.start(), *range.end());
            let q = quant(value, lo, hi);
            (q != quant(defaults.0[i], lo, hi)).then_some((i as u16, q))
        }).collect();
        Self { channels, ancestry, details }
    }
    pub fn to_tuning(&self) -> JiggleRemeshTuning {
        let mut output = JiggleRemeshTuning::default();
        let vals = std::array::from_fn::<_, CHANNELS, _>(|i| {
            dequant(self.channels[i], RANGES[i].0, RANGES[i].1, RANGES[i].2)
        });
        output.body_shape = BodyShapeParams {
            muscle: vals[0], weight: vals[1], proportions: vals[2], height: vals[3], age: vals[4],
        };
        output.breast_size = vals[5];
        output.glute_size = vals[6];
        output.breast_bottom_z = vals[7];
        output.breast_top_z = vals[8];
        output.glute_bottom_z = vals[9];
        output.glute_top_z = vals[10];
        if let Some([a, b]) = self.ancestry {
            let c = 63u8.saturating_sub(a).saturating_sub(b); // Decoder validates simplex bounds.
            output.body_details.0[3] = f32::from(a) / 63.0;
            output.body_details.0[4] = f32::from(b) / 63.0;
            output.body_details.0[5] = f32::from(c) / 63.0;
        }
        for &(index, q) in &self.details {
            let index = usize::from(index);
            if index >= DETAIL_COUNT { continue; }
            let range = Details::range(index);
            let neutral = Details::default().0[index];
            output.body_details.0[index] = dequant(q, *range.start(), *range.end(), neutral);
        }
        output
    }
    /// A stable presentation cache discriminator, not transmitted or decoded as
    /// a substitute for the actual values.
    pub fn fingerprint(&self) -> u64 {
        let bytes = self.payload();
        bytes.iter().fold(0xcbf29ce484222325_u64, |hash, &b| {
            (hash ^ u64::from(b)).wrapping_mul(0x100000001b3)
        })
    }
    fn payload(&self) -> Vec<u8> {
        // 11 * 6 = 66 bits. The six trailing bits carry a v2 tag and
        // the presence bit for the 12-bit ancestry simplex coordinates.
        let mut bits: u128 = 0;
        for &channel in &self.channels { bits = (bits << 6) | u128::from(channel & 63); }
        bits = (bits << 6) | (0b101100 | u128::from(self.ancestry.is_some()));
        let mut raw = Vec::with_capacity(9 + 2 * self.details.len() + if self.ancestry.is_some() { 2 } else { 0 });
        for i in (0..9).rev() { raw.push((bits >> (i * 8)) as u8); }
        if let Some([a, b]) = self.ancestry {
            // 6+6 bits, then four canonical zero padding bits.
            raw.push((a << 2) | (b >> 4));
            raw.push((b & 15) << 4);
        }
        for &(id, q) in &self.details {
            let packed = (id << 6) | u16::from(q & 63);
            raw.extend_from_slice(&packed.to_be_bytes());
        }
        raw
    }
    pub fn encode(&self, color: u8) -> Option<String> {
        let result = format!("{color}z3{}", encode64(&self.payload()));
        (result.len() <= MAX_VALUE).then_some(result)
    }
    pub fn decode(value: &[u8]) -> Option<(i32, Self)> {
        let (color, suffix) = split_color(value)?;
        if value.len() > MAX_VALUE { return None; }
        let data = suffix.strip_prefix(b"z3".as_slice())?;
        let raw = decode64(data)?;
        if raw.len() < 9 { return None; }
        let mut bits = raw[..9].iter().fold(0u128, |v, &b| (v << 8) | u128::from(b));
        let tag = (bits & 63) as u8;
        // Only the current two-coordinate ancestry payload is valid.
        if tag & 0b111110 != 0b101100 { return None; }
        let ancestry_present = tag & 1 != 0;
        bits >>= 6;
        let mut channels = [0; CHANNELS];
        for ch in channels.iter_mut().rev() { *ch = (bits & 63) as u8; bits >>= 6; }
        let mut cursor = 9;
        let ancestry = if ancestry_present {
            if raw.len() < 11 { return None; }
            let a = raw[9] >> 2;
            let b = ((raw[9] & 3) << 4) | (raw[10] >> 4);
            if raw[10] & 15 != 0 || u16::from(a) + u16::from(b) > 63 { return None; }
            cursor = 11;
            Some([a, b])
        } else { None };
        if (raw.len() - cursor) % 2 != 0 { return None; }
        let mut details = Vec::with_capacity((raw.len() - cursor) / 2);
        let mut last = None;
        for pair in raw[cursor..].chunks_exact(2) {
            let word = u16::from_be_bytes([pair[0], pair[1]]);
            let index = word >> 6;
            if usize::from(index) >= DETAIL_COUNT || last.is_some_and(|prev| index <= prev)
                || (3..=5).contains(&index) { return None; }
            let q = (word & 63) as u8;
            details.push((index, q));
            last = Some(index);
        }
        Some((color, Self { channels, ancestry, details }))
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cosmetics_in_fourteen_bytes_and_reversible() {
        let value = PackedCosmetics { mask: 0x80000001, hat: cosmetic_id("santahat"), cape: cosmetic_id("vadercape") };
        let text = encode_cosmetics(4, value);
        assert_eq!(text.len(), 15); // numeric saber + 14-character suffix
        assert_eq!(decode_cosmetics(text.as_bytes()), Some((4, value)));
        assert!(decode_cosmetics(b"4santahat").is_none());
    }
    #[test]
    fn compressed_ancestry_uses_two_coordinates_not_three_sparse_entries() {
        let mut a = JiggleRemeshTuning::default();
        a.body_details.0[3..6].copy_from_slice(&[0.1, 0.3, 0.6]);
        let wire = BodyWire::from_tuning(a);
        assert!(wire.ancestry.is_some());
        assert!(!wire.details.iter().any(|&(id,_)| (3..=5).contains(&id)));
        let encoded = wire.encode(4).unwrap();
        assert!(encoded.starts_with("4z3"));
        assert_eq!(encoded.len(), 18); // 9-byte header + 2-byte ancestry = 15 chars + z3 + color
        let decoded = BodyWire::decode(encoded.as_bytes()).unwrap().1.to_tuning();
        let d = &decoded.body_details.0[3..6];
        assert!((d.iter().sum::<f32>() - 1.0).abs() < 0.0001);
        for i in 0..3 { assert!((d[i] - a.body_details.0[3+i]).abs() <= 1.0 / 63.0); }
        a.body_details.0[3..6].copy_from_slice(&[0.2, 0.6, 1.2]);
        // Input is clamped to [0,1] before creating barycentric coordinates.
        let second = BodyWire::from_tuning(a).encode(4).unwrap();
        assert!(BodyWire::decode(second.as_bytes()).is_some());
    }
    #[test]
    fn neutral_and_scaled_equal_ancestry_require_zero_extra_bits() {
        let mut a = JiggleRemeshTuning::default();
        a.body_details.0[3..6].copy_from_slice(&[0.7, 0.7, 0.7]);
        assert_eq!(BodyWire::from_tuning(a), BodyWire::neutral());
        a.body_details.0[3..6].copy_from_slice(&[0.0, 0.0, 0.0]);
        assert_eq!(BodyWire::from_tuning(a), BodyWire::neutral());
        assert_eq!(BodyWire::neutral().encode(2).unwrap().len(), 15);
    }
    #[test]
    fn rejects_obsolete_z2_payloads() {
        let payload = BodyWire::neutral().payload();
        let old = format!("4z2{}", encode64(&payload));
        assert!(BodyWire::decode(old.as_bytes()).is_none());
    }
    #[test]
    fn rejects_individual_ancestry_entries_in_current_format() {
        let mut payload = BodyWire::neutral().payload();
        payload.extend_from_slice(&((3u16 << 6) | 31u16).to_be_bytes());
        let bad = format!("4z3{}", encode64(&payload));
        assert!(BodyWire::decode(bad.as_bytes()).is_none());
    }
    #[test]
    fn reject_non_simplex_or_noncanonical_ancestry() {
        let mut a = JiggleRemeshTuning::default();
        a.body_details.0[3] = 1.0;
        let mut raw = BodyWire::from_tuning(a).payload();
        raw[9] = 63 << 2;
        raw[10] = 1 << 4; // 63+1 exceeds simplex
        assert!(BodyWire::decode(format!("4z3{}", encode64(&raw)).as_bytes()).is_none());
        raw[9] = 0;
        raw[10] = 1; // noncanonical padding
        assert!(BodyWire::decode(format!("4z3{}", encode64(&raw)).as_bytes()).is_none());
    }
    #[test]
    fn body_neutral_and_sparse_roundtrip() {
        let neutral = BodyWire::neutral();
        let wire = neutral.encode(2).unwrap();
        assert_eq!(wire.len(), 15);
        assert_eq!(BodyWire::decode(wire.as_bytes()), Some((2, neutral.clone())));
        assert!(neutral.to_tuning().body_shape.is_neutral());
        let mut t = JiggleRemeshTuning::default();
        t.breast_size = 1.2;
        t.body_details.0[15] = 0.4;
        t.body_details.0[DETAIL_COUNT - 1] = 1.0;
        let encoded = BodyWire::from_tuning(t).encode(4).unwrap();
        let (_, decoded) = BodyWire::decode(encoded.as_bytes()).unwrap();
        assert!(decoded.to_tuning().breast_size > 1.0);
        assert!(decoded.to_tuning().body_details.0[15] > 0.3);
        assert!(decoded.to_tuning().body_details.0[DETAIL_COUNT - 1] > 0.95);
        let mut broken = encoded.into_bytes();
        broken.pop();
        assert!(BodyWire::decode(&broken).is_none());
    }
}
