//! Stock, pre-trained Huffman message codec. This is not the adaptive codec
//! used for connection setup and does not process netchan/XOR headers.

use std::{fmt, sync::OnceLock};

use crate::{demo::MAX_MESSAGE_BYTES, huffman_codes::CODES};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    UnexpectedEnd,
    InvalidWidth(i8),
    InvalidHuffmanCode,
    Limit(&'static str),
    InvalidValue(&'static str),
    Unsupported(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub bit: usize,
    pub kind: ErrorKind,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "message bit {}: ", self.bit)?;
        match self.kind {
            ErrorKind::UnexpectedEnd => f.write_str("truncated message"),
            ErrorKind::InvalidWidth(width) => write!(f, "invalid field width {width}"),
            ErrorKind::InvalidHuffmanCode => f.write_str("invalid stock Huffman code"),
            ErrorKind::Limit(reason) => write!(f, "limit exceeded: {reason}"),
            ErrorKind::InvalidValue(reason) => write!(f, "invalid {reason}"),
            ErrorKind::Unsupported(reason) => write!(f, "unsupported {reason}"),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Default)]
struct Node {
    children: [Option<usize>; 2],
    symbol: Option<u8>,
}

fn tree() -> &'static [Node] {
    static TREE: OnceLock<Vec<Node>> = OnceLock::new();
    TREE.get_or_init(|| {
        let mut nodes = vec![Node::default()];
        for (symbol, &(code, width)) in CODES.iter().enumerate() {
            let mut index = 0;
            for shift in 0..width {
                assert!(nodes[index].symbol.is_none(), "code prefix collision");
                let branch = ((code >> shift) & 1) as usize;
                index = if let Some(child) = nodes[index].children[branch] {
                    child
                } else {
                    let child = nodes.len();
                    nodes.push(Node::default());
                    nodes[index].children[branch] = Some(child);
                    child
                };
            }
            assert!(nodes[index].children == [None, None] && nodes[index].symbol.is_none());
            nodes[index].symbol = Some(symbol as u8);
        }
        nodes
    })
}

fn width(bits: i8, position: usize) -> Result<u8, Error> {
    if bits == 0 || !(-31..=32).contains(&bits) {
        return Err(Error {
            bit: position,
            kind: ErrorKind::InvalidWidth(bits),
        });
    }
    Ok(bits.unsigned_abs())
}

fn raw_bit(data: &[u8], cursor: &mut usize) -> Result<u32, Error> {
    let byte = data.get(*cursor / 8).ok_or(Error {
        bit: *cursor,
        kind: ErrorKind::UnexpectedEnd,
    })?;
    let value = u32::from((byte >> (*cursor % 8)) & 1);
    *cursor += 1;
    Ok(value)
}

fn symbol(data: &[u8], cursor: &mut usize) -> Result<u32, Error> {
    let nodes = tree();
    let mut index = 0;
    loop {
        if let Some(value) = nodes[index].symbol {
            return Ok(u32::from(value));
        }
        let branch = raw_bit(data, cursor)? as usize;
        index = nodes[index].children[branch].ok_or(Error {
            bit: *cursor,
            kind: ErrorKind::InvalidHuffmanCode,
        })?;
    }
}

pub struct MessageReader<'a> {
    data: &'a [u8],
    bit: usize,
}

impl<'a> MessageReader<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, Error> {
        if data.len() > MAX_MESSAGE_BYTES {
            return Err(Error {
                bit: 0,
                kind: ErrorKind::Limit("message bytes"),
            });
        }
        Ok(Self { data, bit: 0 })
    }

    pub fn bit_position(&self) -> usize {
        self.bit
    }

    /// OpenJK's readcount convention includes its final padding byte.
    pub fn legacy_read_count(&self) -> usize {
        self.bit / 8 + 1
    }

    pub fn error(&self, kind: ErrorKind) -> Error {
        Error {
            bit: self.bit,
            kind,
        }
    }

    /// Reads low residual bits first, then Huffman byte symbols. A failed call
    /// leaves the cursor unchanged. Positive 32-bit fields return their i32 bits.
    /// Negative widths deliberately match stock MSG_ReadBits: its sign width
    /// is rounded down to whole bytes. -8 and -16 have normal signed behavior;
    /// non-byte negative widths inherit the reference's historical quirk.
    pub fn read_bits(&mut self, bits: i8) -> Result<i32, Error> {
        let count = width(bits, self.bit)?;
        let residual = count % 8;
        let mut cursor = self.bit;
        let mut value = 0u32;
        for shift in 0..residual {
            value |= raw_bit(self.data, &mut cursor)? << shift;
        }
        for shift in (residual..count).step_by(8) {
            value |= symbol(self.data, &mut cursor)? << shift;
        }
        let sign_width = count - residual;
        if bits < 0 && sign_width > 0 && value & (1 << (sign_width - 1)) != 0 {
            value |= u32::MAX << sign_width;
        }
        self.bit = cursor;
        Ok(value as i32)
    }

    fn read_scalar(&mut self, bits: i8) -> Result<i32, Error> {
        let value = self.read_bits(bits)?;
        if self.legacy_read_count() > self.data.len() {
            return Err(self.error(ErrorKind::UnexpectedEnd));
        }
        Ok(value)
    }

    pub fn read_byte(&mut self) -> Result<u8, Error> {
        Ok(self.read_scalar(8)? as u8)
    }
    pub fn read_short(&mut self) -> Result<i16, Error> {
        Ok(self.read_scalar(16)? as i16)
    }
    pub fn read_long(&mut self) -> Result<i32, Error> {
        self.read_scalar(32)
    }

    /// Read an exact raw byte count without applying C-string semantics.
    pub fn read_data(&mut self, len: usize) -> Result<Vec<u8>, Error> {
        if len > MAX_MESSAGE_BYTES {
            return Err(self.error(ErrorKind::Limit("data bytes")));
        }
        let mut value = Vec::with_capacity(len);
        for _ in 0..len {
            value.push(self.read_byte()?);
        }
        Ok(value)
    }

    /// `limit` includes the NUL terminator. Preserve wire bytes; display and
    /// command sanitization are responsibilities of higher layers.
    pub fn read_string(&mut self, limit: usize) -> Result<Vec<u8>, Error> {
        if limit == 0 || limit > 8192 {
            return Err(self.error(ErrorKind::Limit("string bound")));
        }
        let mut value = Vec::new();
        for _ in 0..limit {
            let byte = self.read_byte()?;
            if byte == 0 {
                return Ok(value);
            }
            value.push(byte);
        }
        Err(self.error(ErrorKind::Limit("unterminated or oversized string")))
    }
}

pub struct MessageWriter {
    data: Vec<u8>,
    bit: usize,
}

impl MessageWriter {
    pub fn new(max_bytes: usize) -> Result<Self, Error> {
        if max_bytes == 0 || max_bytes > MAX_MESSAGE_BYTES {
            return Err(Error {
                bit: 0,
                kind: ErrorKind::Limit("message capacity"),
            });
        }
        Ok(Self {
            data: vec![0; max_bytes],
            bit: 0,
        })
    }

    pub fn bit_position(&self) -> usize {
        self.bit
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.data[..if self.bit == 0 { 0 } else { self.bit / 8 + 1 }]
    }

    fn push_bits(&mut self, value: u32, count: u8) {
        for shift in 0..count {
            self.data[self.bit / 8] |= (((value >> shift) & 1) as u8) << (self.bit % 8);
            self.bit += 1;
        }
    }

    /// Like the reference writer, values are masked to the requested width.
    /// Capacity is checked before mutation, including the legacy padding byte.
    pub fn write_bits(&mut self, value: i32, bits: i8) -> Result<(), Error> {
        let count = width(bits, self.bit)?;
        let residual = count % 8;
        let value = value as u32;
        let needed = usize::from(residual)
            + (residual..count)
                .step_by(8)
                .map(|shift| usize::from(CODES[((value >> shift) & 255) as usize].1))
                .sum::<usize>();
        if (self.bit + needed) / 8 + 1 > self.data.len() {
            return Err(Error {
                bit: self.bit,
                kind: ErrorKind::Limit("message capacity"),
            });
        }
        self.push_bits(value, residual);
        for shift in (residual..count).step_by(8) {
            let (code, length) = CODES[((value >> shift) & 255) as usize];
            self.push_bits(u32::from(code), length);
        }
        Ok(())
    }
}
