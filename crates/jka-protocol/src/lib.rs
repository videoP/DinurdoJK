//! JKA wire data. No engine, renderer, filesystem, or socket dependencies.
//! Connectionless queries, bounded demo framing, and stock message decoding.

use std::fmt;

pub mod adaptive_huffman;
pub mod commands;
pub mod demo;
mod entity_fields;
pub mod entity_event;
pub mod gamestate;
mod huffman_codes;
mod player_fields;
pub mod message;
pub mod netchan;
pub mod server;
pub mod session;

pub const PROTOCOL_VERSION: u32 = 26;
pub const DEFAULT_PORT: u16 = 29070;
const MAX_INFO_STRING: usize = 1024;
const RESPONSE_PREFIX: &[u8] = b"\xff\xff\xff\xffinfoResponse\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidChallenge,
    UnexpectedPacket,
    OversizedInfo,
    MalformedInfo,
    DuplicateKey,
    ChallengeMismatch,
    InvalidProtocol,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidChallenge => "challenge must contain 1–32 ASCII letters or digits",
            Self::UnexpectedPacket => "expected a connectionless infoResponse",
            Self::OversizedInfo => "server info exceeds the JKA info-string limit",
            Self::MalformedInfo => "malformed server info string",
            Self::DuplicateKey => "ambiguous duplicate server info key",
            Self::ChallengeMismatch => "response does not match this query's challenge",
            Self::InvalidProtocol => "missing or invalid protocol number",
        };
        f.write_str(message)
    }
}

impl std::error::Error for Error {}

/// This query challenge is an echo token, separate from the connection handshake.
pub fn getinfo_request(challenge: &str) -> Result<Vec<u8>, Error> {
    if challenge.is_empty()
        || challenge.len() > 32
        || !challenge.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        return Err(Error::InvalidChallenge);
    }
    let mut packet = b"\xff\xff\xff\xffgetinfo ".to_vec();
    packet.extend_from_slice(challenge.as_bytes());
    Ok(packet)
}

/// Values remain bytes: legacy player/server text need not be UTF-8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    fields: Vec<(Vec<u8>, Vec<u8>)>,
    pub protocol: u32,
}

impl ServerInfo {
    pub fn get(&self, key: &[u8]) -> Option<&[u8]> {
        self.fields
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.as_slice())
    }

    /// A matching number is a prerequisite, not proof of gameplay compatibility.
    pub fn is_protocol_26(&self) -> bool {
        self.protocol == PROTOCOL_VERSION
    }
}

/// Parse the bounded stock OpenJK response format. Unknown keys are retained.
pub fn parse_info_response(packet: &[u8], challenge: &str) -> Result<ServerInfo, Error> {
    let info = packet
        .strip_prefix(RESPONSE_PREFIX)
        .ok_or(Error::UnexpectedPacket)?;
    // NET_OutOfBandPrint normally omits a NUL; tolerate one trailing terminator.
    let info = info.strip_suffix(b"\0").unwrap_or(info);
    if info.len() >= MAX_INFO_STRING {
        return Err(Error::OversizedInfo);
    }
    if info.iter().any(|&b| b < 32 || b == 127) {
        return Err(Error::MalformedInfo);
    }
    let body = info.strip_prefix(b"\\").ok_or(Error::MalformedInfo)?;
    let mut parts = body.split(|&b| b == b'\\');
    let mut result = ServerInfo {
        fields: Vec::new(),
        protocol: 0,
    };
    while let Some(key) = parts.next() {
        let value = parts.next().ok_or(Error::MalformedInfo)?;
        if key.is_empty() {
            return Err(Error::MalformedInfo);
        }
        if result.get(key).is_some() {
            return Err(Error::DuplicateKey);
        }
        result.fields.push((key.to_vec(), value.to_vec()));
    }
    if result.get(b"challenge") != Some(challenge.as_bytes()) {
        return Err(Error::ChallengeMismatch);
    }
    let protocol = result.get(b"protocol").ok_or(Error::InvalidProtocol)?;
    if protocol.is_empty() || !protocol.iter().all(u8::is_ascii_digit) {
        return Err(Error::InvalidProtocol);
    }
    result.protocol = std::str::from_utf8(protocol)
        .map_err(|_| Error::InvalidProtocol)?
        .parse()
        .map_err(|_| Error::InvalidProtocol)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &[u8] = b"\xff\xff\xff\xffinfoResponse\n\\challenge\\abc123\\protocol\\26\\hostname\\^2Test\\mapname\\mp/ffa3";

    #[test]
    fn query_matches_wire_bytes() {
        assert_eq!(
            getinfo_request("abc123").unwrap(),
            b"\xff\xff\xff\xffgetinfo abc123"
        );
        for challenge in [
            "",
            "a b",
            "a\n",
            "a\\b",
            "é",
            "abcdefghijklmnopqrstuvwxyz1234567",
        ] {
            assert_eq!(getinfo_request(challenge), Err(Error::InvalidChallenge));
        }
    }

    #[test]
    fn parses_stock_layout_and_optional_nul() {
        let info = parse_info_response(VALID, "abc123").unwrap();
        assert!(info.is_protocol_26());
        assert_eq!(info.get(b"HOSTNAME"), Some(b"^2Test".as_slice()));
        let mut terminated = VALID.to_vec();
        terminated.push(0);
        assert_eq!(parse_info_response(&terminated, "abc123").unwrap(), info);
    }

    #[test]
    fn preserves_legacy_text_and_reports_other_protocols() {
        let packet = b"\xff\xff\xff\xffinfoResponse\n\\challenge\\x\\protocol\\25\\hostname\\caf\xe9\\unknown\\";
        let info = parse_info_response(packet, "x").unwrap();
        assert!(!info.is_protocol_26());
        assert_eq!(info.get(b"hostname"), Some(b"caf\xe9".as_slice()));
        assert_eq!(info.get(b"unknown"), Some(b"".as_slice()));
    }

    #[test]
    fn rejects_malformed_and_ambiguous_packets() {
        assert_eq!(
            parse_info_response(VALID, "other"),
            Err(Error::ChallengeMismatch)
        );
        assert_eq!(
            parse_info_response(&VALID[4..], "abc123"),
            Err(Error::UnexpectedPacket)
        );
        for (suffix, expected) in [
            (b"\\orphan".as_slice(), Error::MalformedInfo),
            (b"\\Protocol\\25".as_slice(), Error::DuplicateKey),
            (b"\0junk".as_slice(), Error::MalformedInfo),
            (b"\\\\value".as_slice(), Error::MalformedInfo),
        ] {
            let mut packet = VALID.to_vec();
            packet.extend_from_slice(suffix);
            assert_eq!(parse_info_response(&packet, "abc123"), Err(expected));
        }
        let oversized = [RESPONSE_PREFIX, &vec![b'x'; MAX_INFO_STRING]].concat();
        assert_eq!(
            parse_info_response(&oversized, "abc123"),
            Err(Error::OversizedInfo)
        );
        for protocol in ["", "-1", "26x", "4294967296"] {
            let packet = [
                RESPONSE_PREFIX,
                format!("\\challenge\\x\\protocol\\{protocol}").as_bytes(),
            ]
            .concat();
            assert_eq!(
                parse_info_response(&packet, "x"),
                Err(Error::InvalidProtocol)
            );
        }
    }

    #[test]
    fn truncations_never_panic() {
        // Some prefixes are complete info strings, so successful parses are valid.
        for length in 0..VALID.len() {
            let _ = parse_info_response(&VALID[..length], "abc123");
        }
    }
}
