//! Demo framing only; payloads still require JKA message/Huffman decoding.
//! Reference: OpenJK CL_WriteDemoMessage and CL_ReadDemoMessage.

use std::io::{self, Read, Write};

use crate::{
    gamestate::{EntityState, ENTITY_FIELDS},
    message::{Error as MessageError, MessageWriter},
    server::{Decoder, SVC_BASELINE, SVC_CONFIGSTRING, SVC_EOF, SVC_GAMESTATE},
};

pub const MAX_MESSAGE_BYTES: usize = 49152;

pub fn write_record<W: Write>(writer: &mut W, sequence: i32, payload: &[u8]) -> io::Result<()> {
    if payload.len() > MAX_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "demo message exceeds 49152 bytes",
        ));
    }
    writer.write_all(&sequence.to_le_bytes())?;
    writer.write_all(&(payload.len() as i32).to_le_bytes())?;
    writer.write_all(payload)
}

pub fn write_end<W: Write>(writer: &mut W) -> io::Result<()> {
    writer.write_all(&(-1_i32).to_le_bytes())?;
    writer.write_all(&(-1_i32).to_le_bytes())
}

fn write_string(writer: &mut MessageWriter, value: &[u8]) -> Result<(), MessageError> {
    for &byte in value {
        writer.write_bits(i32::from(byte), 8)?;
    }
    writer.write_bits(0, 8)
}

fn write_delta_entity_from_null(
    writer: &mut MessageWriter,
    entity: &EntityState,
) -> Result<(), MessageError> {
    let last_changed = entity
        .fields
        .iter()
        .rposition(|&value| value != 0)
        .map_or(0, |index| index + 1);

    // MSG_WriteDeltaEntity(from = null): not removed, then whether any field changed.
    writer.write_bits(0, 1)?;
    if last_changed == 0 {
        writer.write_bits(0, 1)?;
        return Ok(());
    }
    writer.write_bits(1, 1)?;
    writer.write_bits(last_changed as i32, 8)?;

    for (index, &(_, bits)) in ENTITY_FIELDS.iter().take(last_changed).enumerate() {
        let value = entity.fields[index];
        if value == 0 {
            writer.write_bits(0, 1)?;
            continue;
        }
        writer.write_bits(1, 1)?;
        writer.write_bits(1, 1)?;
        if bits != 0 {
            writer.write_bits(value as i32, bits)?;
            continue;
        }

        let number = f32::from_bits(value);
        let integral = number as i32;
        if number.is_finite()
            && number == integral as f32
            && (-4096..4096).contains(&integral)
        {
            writer.write_bits(0, 1)?;
            writer.write_bits(integral + 4096, 13)?;
        } else {
            writer.write_bits(1, 1)?;
            writer.write_bits(value as i32, 32)?;
        }
    }
    Ok(())
}

/// Build the synthetic first message written by OpenJK's `record` command.
/// It snapshots the current reliable/gamestate state so a demo started in the
/// middle of a match can be decoded without any packets from before recording.
pub fn synthesize_gamestate_payload(
    decoder: &Decoder,
    reliable_sequence: i32,
) -> Result<Vec<u8>, MessageError> {
    let mut writer = MessageWriter::new(MAX_MESSAGE_BYTES)?;
    writer.write_bits(reliable_sequence, 32)?;
    writer.write_bits(i32::from(SVC_GAMESTATE), 8)?;
    writer.write_bits(decoder.server_command_sequence, 32)?;

    for (&index, value) in &decoder.configstrings {
        writer.write_bits(i32::from(SVC_CONFIGSTRING), 8)?;
        writer.write_bits(i32::from(index), 16)?;
        write_string(&mut writer, value)?;
    }
    for (&number, entity) in &decoder.baselines {
        writer.write_bits(i32::from(SVC_BASELINE), 8)?;
        writer.write_bits(i32::from(number), 10)?;
        write_delta_entity_from_null(&mut writer, entity)?;
    }

    writer.write_bits(i32::from(SVC_EOF), 8)?;
    writer.write_bits(decoder.client_number, 32)?;
    writer.write_bits(decoder.checksum_feed as i32, 32)?;
    // Historical RMG short retained by the protocol-26 wire format.
    writer.write_bits(0, 16)?;
    writer.write_bits(i32::from(SVC_EOF), 8)?;
    Ok(writer.as_bytes().to_vec())
}


#[derive(Debug, PartialEq, Eq)]
pub struct Record {
    pub sequence: i32,
    pub payload: Vec<u8>,
}

/// A streaming reader with at most one bounded payload allocation per record.
/// Missing end markers are reported as truncation rather than successful EOF.
pub struct DemoReader<R> {
    reader: R,
    ended: bool,
    offset: u64,
}

impl<R: Read> DemoReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            ended: false,
            offset: 0,
        }
    }

    pub fn next_record(&mut self) -> io::Result<Option<Record>> {
        if self.ended {
            return Ok(None);
        }
        // An error is terminal: never try to reinterpret a partial payload as a header.
        self.ended = true;
        let mut header = [0; 8];
        self.reader.read_exact(&mut header).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("demo header/end marker at byte {}: {error}", self.offset),
            )
        })?;
        let sequence = i32::from_le_bytes(header[..4].try_into().unwrap());
        let length = i32::from_le_bytes(header[4..].try_into().unwrap());
        if length == -1 {
            return Ok(None);
        }
        if length < 0 || length as usize > MAX_MESSAGE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "demo message length is outside 0..=49152",
            ));
        }
        let mut payload = vec![0; length as usize];
        self.reader.read_exact(&mut payload).map_err(|error| {
            io::Error::new(error.kind(), format!("demo payload at byte {} (sequence {sequence}, expected {length} bytes): {error}", self.offset + 8))
        })?;
        self.offset += 8 + length as u64;
        self.ended = false;
        Ok(Some(Record { sequence, payload }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEMO: &[u8] = &[
        42, 0, 0, 0, 3, 0, 0, 0, 0x12, 0x34, 0x56, 43, 0, 0, 0, 1, 0, 0, 0, 0x78, 255, 255, 255,
        255, 255, 255, 255, 255,
    ];

    #[test]
    fn reads_little_endian_records_and_end_marker() {
        let mut reader = DemoReader::new(DEMO);
        assert_eq!(
            reader.next_record().unwrap(),
            Some(Record {
                sequence: 42,
                payload: vec![0x12, 0x34, 0x56]
            })
        );
        assert_eq!(
            reader.next_record().unwrap(),
            Some(Record {
                sequence: 43,
                payload: vec![0x78]
            })
        );
        assert_eq!(reader.next_record().unwrap(), None);
        assert_eq!(reader.next_record().unwrap(), None);
    }

    #[test]
    fn every_truncated_prefix_is_reported() {
        for length in 0..DEMO.len() {
            let mut reader = DemoReader::new(&DEMO[..length]);
            loop {
                match reader.next_record() {
                    Ok(Some(_)) => continue,
                    Ok(None) => panic!("truncated prefix {length} accepted"),
                    Err(error) => {
                        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
                        break;
                    }
                }
            }
        }
    }

    #[test]
    fn rejects_bad_lengths_before_reading_payload() {
        for length in [-2i32, 49153, i32::MAX] {
            let bytes = [42i32.to_le_bytes(), length.to_le_bytes()].concat();
            assert_eq!(
                DemoReader::new(bytes.as_slice())
                    .next_record()
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
        let mut bytes = [
            42i32.to_le_bytes(),
            (MAX_MESSAGE_BYTES as i32).to_le_bytes(),
        ]
        .concat();
        bytes.resize(8 + MAX_MESSAGE_BYTES, 0);
        let record = DemoReader::new(bytes.as_slice())
            .next_record()
            .unwrap()
            .unwrap();
        assert_eq!(record.payload.len(), MAX_MESSAGE_BYTES);
    }

    #[test]
    fn writer_round_trips_records_and_end_marker() {
        let mut bytes = Vec::new();
        write_record(&mut bytes, 12, b"abc").unwrap();
        write_end(&mut bytes).unwrap();
        let mut reader = DemoReader::new(bytes.as_slice());
        assert_eq!(
            reader.next_record().unwrap(),
            Some(Record { sequence: 12, payload: b"abc".to_vec() })
        );
        assert_eq!(reader.next_record().unwrap(), None);
    }

    #[test]
    fn synthesized_gamestate_decodes() {
        let mut decoder = Decoder::new();
        decoder.reliable_acknowledge = 7;
        decoder.server_command_sequence = 13;
        decoder.configstrings.insert(0, br"\mapname\mp/ffa3".to_vec());
        decoder.client_number = 2;
        decoder.checksum_feed = 0x12345678;
        let mut baseline = EntityState { number: 5, fields: [0; ENTITY_FIELDS.len()] };
        baseline.fields[0] = 17;
        decoder.baselines.insert(5, baseline.clone());

        let payload = synthesize_gamestate_payload(&decoder, 99).unwrap();
        let decoded = crate::gamestate::decode_initial_gamestate(&payload).unwrap();
        assert_eq!(decoded.reliable_acknowledge, 99);
        assert_eq!(decoded.server_command_sequence, 13);
        assert_eq!(decoded.configstrings.get(&0).unwrap(), br"\mapname\mp/ffa3");
        assert_eq!(decoded.baselines.get(&5), Some(&baseline));
        assert_eq!(decoded.client_number, 2);
        assert_eq!(decoded.checksum_feed, 0x12345678);
    }

}
