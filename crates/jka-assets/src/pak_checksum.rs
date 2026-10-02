//! JKA/OpenJK PK3 checksums used by pure-server and autodownload negotiation.
//!
//! OpenJK builds a little-endian array of CRC32 values for every non-empty ZIP
//! entry, then applies its historical MD4-based `Com_BlockChecksum`.  Keeping
//! that exact checksum here lets the Rust client compare `sv_referencedPaks`
//! rather than guessing from package filenames.

use std::{fs::File, path::Path};

pub fn pk3_checksum(path: &Path) -> Result<i32, Box<dyn std::error::Error>> {
    Ok(block_checksum(&pk3_crc_bytes(path)?) as i32)
}

/// Return both the ordinary and feed-keyed JKA/OpenJK checksums from one ZIP
/// directory walk. This is useful during sv_pure negotiation, where both forms
/// are needed for the same PK3.
pub fn pk3_checksums(
    path: &Path,
    checksum_feed: u32,
) -> Result<(i32, i32), Box<dyn std::error::Error>> {
    let crc_bytes = pk3_crc_bytes(path)?;
    let regular = block_checksum(&crc_bytes) as i32;
    let mut keyed = Vec::with_capacity(4 + crc_bytes.len());
    keyed.extend_from_slice(&checksum_feed.to_le_bytes());
    keyed.extend_from_slice(&crc_bytes);
    Ok((regular, block_checksum(&keyed) as i32))
}

/// Return the feed-keyed checksum used by JKA/OpenJK for sv_pure validation.
///
/// `Com_BlockChecksumKey` hashes the little-endian 32-bit checksum feed first,
/// followed by the exact CRC32 array used by the ordinary PK3 checksum.
pub fn pk3_pure_checksum(
    path: &Path,
    checksum_feed: u32,
) -> Result<i32, Box<dyn std::error::Error>> {
    pk3_checksums(path, checksum_feed).map(|(_, pure)| pure)
}

/// True when the PK3 contains any of `names` (case-insensitive, exact entry name).
/// Mirrors the server's `FS_FileIsInPAK("cgamex86.dll")` pure lookups.
pub fn pk3_has_entry(path: &Path, names: &[&str]) -> Result<bool, Box<dyn std::error::Error>> {
    let archive = zip::ZipArchive::new(File::open(path)?)?;
    let found = archive
        .file_names()
        .any(|entry| names.iter().any(|wanted| entry.eq_ignore_ascii_case(wanted)));
    Ok(found)
}

fn pk3_crc_bytes(path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let file = File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut crc_bytes = Vec::with_capacity(archive.len().saturating_mul(4));
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        if entry.size() > 0 {
            crc_bytes.extend_from_slice(&entry.crc32().to_le_bytes());
        }
    }
    Ok(crc_bytes)
}

fn block_checksum(bytes: &[u8]) -> u32 {
    let digest = md4(bytes);
    u32::from_le_bytes(digest[0..4].try_into().unwrap())
        ^ u32::from_le_bytes(digest[4..8].try_into().unwrap())
        ^ u32::from_le_bytes(digest[8..12].try_into().unwrap())
        ^ u32::from_le_bytes(digest[12..16].try_into().unwrap())
}

fn md4(input: &[u8]) -> [u8; 16] {
    let mut a = 0x6745_2301u32;
    let mut b = 0xefcd_ab89u32;
    let mut c = 0x98ba_dcfeu32;
    let mut d = 0x1032_5476u32;

    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity((input.len() + 72) & !63);
    padded.extend_from_slice(input);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_le_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut x = [0u32; 16];
        for (word, bytes) in x.iter_mut().zip(chunk.chunks_exact(4)) {
            *word = u32::from_le_bytes(bytes.try_into().unwrap());
        }

        let (aa, bb, cc, dd) = (a, b, c, d);
        macro_rules! r1 {
            ($a:ident, $b:ident, $c:ident, $d:ident, $k:expr, $s:expr) => {
                $a = $a
                    .wrapping_add(($b & $c) | (!$b & $d))
                    .wrapping_add(x[$k])
                    .rotate_left($s);
            };
        }
        macro_rules! r2 {
            ($a:ident, $b:ident, $c:ident, $d:ident, $k:expr, $s:expr) => {
                $a = $a
                    .wrapping_add(($b & $c) | ($b & $d) | ($c & $d))
                    .wrapping_add(x[$k])
                    .wrapping_add(0x5a82_7999)
                    .rotate_left($s);
            };
        }
        macro_rules! r3 {
            ($a:ident, $b:ident, $c:ident, $d:ident, $k:expr, $s:expr) => {
                $a = $a
                    .wrapping_add($b ^ $c ^ $d)
                    .wrapping_add(x[$k])
                    .wrapping_add(0x6ed9_eba1)
                    .rotate_left($s);
            };
        }

        r1!(a,b,c,d, 0, 3); r1!(d,a,b,c, 1, 7); r1!(c,d,a,b, 2,11); r1!(b,c,d,a, 3,19);
        r1!(a,b,c,d, 4, 3); r1!(d,a,b,c, 5, 7); r1!(c,d,a,b, 6,11); r1!(b,c,d,a, 7,19);
        r1!(a,b,c,d, 8, 3); r1!(d,a,b,c, 9, 7); r1!(c,d,a,b,10,11); r1!(b,c,d,a,11,19);
        r1!(a,b,c,d,12, 3); r1!(d,a,b,c,13, 7); r1!(c,d,a,b,14,11); r1!(b,c,d,a,15,19);

        r2!(a,b,c,d, 0, 3); r2!(d,a,b,c, 4, 5); r2!(c,d,a,b, 8, 9); r2!(b,c,d,a,12,13);
        r2!(a,b,c,d, 1, 3); r2!(d,a,b,c, 5, 5); r2!(c,d,a,b, 9, 9); r2!(b,c,d,a,13,13);
        r2!(a,b,c,d, 2, 3); r2!(d,a,b,c, 6, 5); r2!(c,d,a,b,10, 9); r2!(b,c,d,a,14,13);
        r2!(a,b,c,d, 3, 3); r2!(d,a,b,c, 7, 5); r2!(c,d,a,b,11, 9); r2!(b,c,d,a,15,13);

        r3!(a,b,c,d, 0, 3); r3!(d,a,b,c, 8, 9); r3!(c,d,a,b, 4,11); r3!(b,c,d,a,12,15);
        r3!(a,b,c,d, 2, 3); r3!(d,a,b,c,10, 9); r3!(c,d,a,b, 6,11); r3!(b,c,d,a,14,15);
        r3!(a,b,c,d, 1, 3); r3!(d,a,b,c, 9, 9); r3!(c,d,a,b, 5,11); r3!(b,c,d,a,13,15);
        r3!(a,b,c,d, 3, 3); r3!(d,a,b,c,11, 9); r3!(c,d,a,b, 7,11); r3!(b,c,d,a,15,15);

        a = a.wrapping_add(aa);
        b = b.wrapping_add(bb);
        c = c.wrapping_add(cc);
        d = d.wrapping_add(dd);
    }

    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a.to_le_bytes());
    out[4..8].copy_from_slice(&b.to_le_bytes());
    out[8..12].copy_from_slice(&c.to_le_bytes());
    out[12..16].copy_from_slice(&d.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn md4_matches_standard_vectors() {
        assert_eq!(hex(&md4(b"")), "31d6cfe0d16ae931b73c59d7e0c089c0");
        assert_eq!(hex(&md4(b"a")), "bde52cb31de33e46245e05fbdbd6fb24");
        assert_eq!(hex(&md4(b"abc")), "a448017aaf21d8525fc10ae87aa6729d");
    }
}
