//! NWH/JK2MV RGB escape syntax, as implemented by NWH's Q_parseColorHex.
//! Only the renderer's JK2 1.02 NWH session opts into displaying these codes.

/// At the `^`, consume an NWH RGB/RGBA escape, including the marker.
/// NWH's server/client parser is lenient: non-hex bytes in an otherwise
/// complete escape contribute zero. Incomplete sequences are not escapes.
/// The input colour's alpha is the UI's fade, which must survive ^x/^X and
/// multiply ^y/^Y rather than being discarded mid-fade.
#[inline]
pub(crate) fn parse_hex_escape(bytes: &[u8], at: usize, fade: f32) -> Option<(usize, [f32; 4])> {
    if bytes.get(at) != Some(&b'^') { return None; }
    let (digits, short, rgba) = match bytes.get(at + 1).copied()? {
        b'x' => (3, true, false),
        b'X' => (6, false, false),
        b'y' => (4, true, true),
        b'Y' => (8, false, true),
        _ => return None,
    };
    let payload = bytes.get(at + 2..at + 2 + digits)?;
    // NWH requires non-NUL bytes, but treats non-hex bytes as zero.
    if payload.contains(&0) { return None; }
    let mut components = [1.0f32; 4];
    if short {
        for (index, byte) in payload.iter().enumerate() {
            components[index] = hex_nibble(*byte) as f32 / 15.0;
        }
    } else {
        for (index, pair) in payload.chunks_exact(2).enumerate() {
            components[index] = ((hex_nibble(pair[0]) << 4)
                | hex_nibble(pair[1])) as f32 / 255.0;
        }
    }
    components[3] = if rgba { components[3] * fade } else { fade };
    Some((2 + digits, components))
}

#[inline]
fn hex_nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => 0, // NWH's lenient parser
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nwh_four_formats_and_alpha() {
        assert_eq!(parse_hex_escape(b"^xF80Nick", 0, 0.5), Some((5, [1.0, 8.0/15.0, 0.0, 0.5])));
        assert_eq!(parse_hex_escape(b"^XFF8000Nick", 0, 0.5), Some((8, [1.0, 128.0/255.0, 0.0, 0.5])));
        assert_eq!(parse_hex_escape(b"^yF808Nick", 0, 0.5), Some((6, [1.0, 8.0/15.0, 0.0, 8.0/15.0 * 0.5])));
        assert_eq!(parse_hex_escape(b"^YFF800080Nick", 0, 0.5), Some((10, [1.0, 128.0/255.0, 0.0, 128.0/255.0 * 0.5])));
    }

    #[test]
    fn lenient_invalid_digits_and_incomplete_sequences() {
        assert_eq!(parse_hex_escape(b"^xZZFPlayer", 0, 1.0), Some((5, [0.0, 0.0, 1.0, 1.0])));
        assert_eq!(parse_hex_escape(b"^xF8", 0, 1.0), None);
        assert_eq!(parse_hex_escape(b"^X12345", 0, 1.0), None);
        assert_eq!(parse_hex_escape(b"^xF\n0", 0, 1.0), Some((5, [1.0, 0.0, 0.0, 1.0])));
        assert_eq!(parse_hex_escape(b"^3Name", 0, 1.0), None);
    }
}
