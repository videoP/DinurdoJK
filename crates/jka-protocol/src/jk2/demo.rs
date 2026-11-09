//! Streaming JK2 demo inspection. This decodes the entire recording, without
//! constructing a JKA cgame session or loading any game assets/native physics.

use std::io::{self, Read};

use crate::message::{Error as MessageError, MessageWriter};

/// JK2MV CL_Record_f: construct the initial server message from the *JK2 wire*
/// gamestate when /record begins mid-match. Never use the mapped JKA decoder;
/// JK2 1.02 and 1.04 have their own field tables, configstring indices, and
/// gamestate footer (no protocol-26 RMG short).
///
/// Reference: mvdevs/jk2mv src/client/cl_main.cpp, CL_Record_f;
/// src/qcommon/msg.cpp, MSG_WriteDeltaEntity.
pub fn synthesize_gamestate_payload(
    decoder: &super::Decoder,
    reliable_sequence: i32,
) -> Result<Vec<u8>, MessageError> {
    let version = decoder.version().ok_or(MessageError {
        bit: 0,
        kind: crate::message::ErrorKind::InvalidValue("JK2 gamestate version"),
    })?;
    let mut writer = MessageWriter::new(super::MAX_MESSAGE_BYTES)?;
    writer.write_bits(reliable_sequence, 32)?;
    writer.write_bits(i32::from(super::SVC_GAMESTATE), 8)?;
    writer.write_bits(decoder.server_command_sequence, 32)?;

    // MSG_WriteBigString / MSG_WriteShort: retain raw JK2 configstring IDs.
    for (&index, value) in &decoder.configstrings {
        writer.write_bits(i32::from(super::SVC_CONFIGSTRING), 8)?;
        writer.write_bits(i32::from(index), 16)?;
        for &byte in value {
            writer.write_bits(i32::from(byte), 8)?;
        }
        writer.write_bits(0, 8)?;
    }

    // JK2MV serializes the entity baselines as deltas from a null entity,
    // using the selected JK2 version's exact field widths/order.
    for (&number, entity) in &decoder.baselines {
        if number >= super::ENTITY_NONE || entity.number != number {
            return Err(MessageError {
                bit: writer.bit_position(),
                kind: crate::message::ErrorKind::InvalidValue("JK2 baseline number"),
            });
        }
        writer.write_bits(i32::from(super::SVC_BASELINE), 8)?;
        writer.write_bits(i32::from(number), 10)?;
        write_baseline_delta(&mut writer, entity, version)?;
    }

    writer.write_bits(i32::from(super::SVC_EOF), 8)?;
    writer.write_bits(decoder.client_number, 32)?;
    writer.write_bits(decoder.checksum_feed as i32, 32)?;
    // Unlike the JKA gamestate, JK2 has no RMG 16-bit field.
    writer.write_bits(i32::from(super::SVC_EOF), 8)?;
    Ok(writer.as_bytes().to_vec())
}

fn write_baseline_delta(
    writer: &mut MessageWriter,
    entity: &super::EntityState,
    version: super::Version,
) -> Result<(), MessageError> {
    let values = entity.fields.iter().collect::<Vec<_>>();
    let schema = version.entity_fields();
    if values.len() != schema.len() {
        return Err(MessageError {
            bit: writer.bit_position(),
            kind: crate::message::ErrorKind::InvalidValue("JK2 baseline schema"),
        });
    }
    // MSG_WriteDeltaEntity(from = null, force = true): removed=0,
    // fieldsChanged=0 if every field is zero, else lastChanged and delta bits.
    writer.write_bits(0, 1)?;
    let last_changed = values.iter().rposition(|(_, value)| *value != 0)
        .map_or(0, |index| index + 1);
    if last_changed == 0 {
        writer.write_bits(0, 1)?;
        return Ok(());
    }
    writer.write_bits(1, 1)?;
    writer.write_bits(last_changed as i32, 8)?;
    for (index, &(_, value)) in values.iter().take(last_changed).enumerate() {
        if value == 0 {
            writer.write_bits(0, 1)?;
            continue;
        }
        writer.write_bits(1, 1)?; // field changed
        writer.write_bits(1, 1)?; // nonzero
        let width = schema[index].1;
        if width != 0 {
            writer.write_bits(value as i32, width)?;
        } else {
            // JK2MV MSG_WriteDeltaEntity's FLOAT_INT_BITS / FLOAT_INT_BIAS.
            // Preserve raw non-integral floats (including negative zero).
            let number = f32::from_bits(value);
            let integral = number as i32;
            if number.is_finite() && number == integral as f32
                && (-4096..4096).contains(&integral)
                && number.to_bits() == (integral as f32).to_bits()
            {
                writer.write_bits(0, 1)?;
                writer.write_bits(integral + 4096, 13)?;
            } else {
                writer.write_bits(1, 1)?;
                writer.write_bits(value as i32, 32)?;
            }
        }
    }
    Ok(())
}

use super::{Decoder, Event, Snapshot, Version};
use crate::commands::info_value;

#[derive(Debug, Clone)]
pub struct Segment {
    pub version: Version,
    pub map: String,
    pub server_version: String,
    pub hostname: String,
    pub game: String,
    pub client_number: i32,
    pub configstrings: usize,
    pub baselines: usize,
    pub first_time: Option<i32>,
    pub last_time: Option<i32>,
    pub snapshots: u64,
    pub full_snapshots: u64,
    pub dropped_snapshots: u64,
    pub max_entities: usize,
    pub nonfinite_snapshots: u64,
    pub first_origin: Option<[f32; 3]>,
    pub last_origin: Option<[f32; 3]>,
}

impl Segment {
    pub fn duration_ms(&self) -> i64 {
        match (self.first_time, self.last_time) {
            (Some(first), Some(last)) => i64::from(last) - i64::from(first),
            _ => 0,
        }
    }
}

#[derive(Debug, Default)]
pub struct Summary {
    pub records: u64,
    pub payload_bytes: u64,
    pub server_commands: u64,
    pub segments: Vec<Segment>,
}

impl Summary {
    /// Legacy server text must not introduce terminal control sequences.
    pub fn report(&self) -> String {
        let safe = |text: &str| {
            text.chars()
                .flat_map(char::escape_debug)
                .collect::<String>()
        };
        let mut lines = vec![format!(
            "JK2 demo: {} records, {} payload bytes, {} server commands, {} gamestates",
            self.records,
            self.payload_bytes,
            self.server_commands,
            self.segments.len(),
        )];
        for (i, segment) in self.segments.iter().enumerate() {
            lines.push(format!(
                "  {}. JK2 {} / protocol {} / map {}",
                i + 1,
                segment.version.name(),
                segment.version.protocol(),
                safe(&segment.map)
            ));
            lines.push(format!(
                "     Server: {} | {} | {}",
                safe(&segment.hostname),
                safe(&segment.server_version),
                safe(&segment.game)
            ));
            lines.push(format!(
                "     Client {} | {} configstrings | {} baselines",
                segment.client_number, segment.configstrings, segment.baselines
            ));
            lines.push(format!(
                "     {} snapshots ({} full, {} delta), {} dropped, max {} entities",
                segment.snapshots,
                segment.full_snapshots,
                segment.snapshots - segment.full_snapshots,
                segment.dropped_snapshots,
                segment.max_entities
            ));
            lines.push(format!(
                "     Server time {:?}..{:?}, duration {:.3}s, nonfinite poses {}",
                segment.first_time,
                segment.last_time,
                segment.duration_ms() as f64 / 1000.0,
                segment.nonfinite_snapshots
            ));
            lines.push(format!(
                "     First origin {:?} | last origin {:?}",
                segment.first_origin, segment.last_origin
            ));
        }
        lines.push("End marker found; all message payloads decoded. Rendering and prediction are not exercised.".into());
        lines.join("\n")
    }
}

pub fn inspect<R: Read>(reader: R) -> io::Result<Summary> {
    inspect_with(reader, |_, _| Ok(()))
}

/// The observer can export exact raw fields or command-by-command poses without
/// retaining a second full demo/snapshot tree in memory.
pub fn inspect_with<R: Read>(
    reader: R,
    mut on_snapshot: impl FnMut(&Decoder, &Snapshot) -> io::Result<()>,
) -> io::Result<Summary> {
    let mut records = crate::demo::DemoReader::new(reader);
    let mut decoder = Decoder::new();
    let mut summary = Summary::default();
    while let Some(record) = records.next_record()? {
        summary.records += 1;
        summary.payload_bytes += record.payload.len() as u64;
        let packet = decoder
            .parse_packet(record.sequence, &record.payload)
            .map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "JK2 demo record {} (sequence {}): {error}",
                        summary.records, record.sequence
                    ),
                )
            })?;
        for event in packet.events {
            match event {
                Event::Gamestate => {
                    if summary.segments.len() >= 4096 {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "too many JK2 demo gamestates",
                        ));
                    }
                    let info = decoder
                        .configstrings
                        .get(&0)
                        .map(Vec::as_slice)
                        .unwrap_or_default();
                    let text = |key| {
                        String::from_utf8_lossy(info_value(info, key).unwrap_or_default())
                            .into_owned()
                    };
                    summary.segments.push(Segment {
                        version: decoder.version().unwrap(),
                        map: decoder.map_name().unwrap_or_default(),
                        server_version: text(b"version"),
                        hostname: text(b"sv_hostname"),
                        game: text(b"gamename"),
                        client_number: decoder.client_number,
                        configstrings: decoder.configstrings.len(),
                        baselines: decoder.baselines.len(),
                        first_time: None,
                        last_time: None,
                        snapshots: 0,
                        full_snapshots: 0,
                        dropped_snapshots: 0,
                        max_entities: 0,
                        nonfinite_snapshots: 0,
                        first_origin: None,
                        last_origin: None,
                    });
                }
                Event::Snapshot(snapshot) => {
                    let segment = summary.segments.last_mut().ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            "JK2 demo snapshot without gamestate",
                        )
                    })?;
                    segment.snapshots += 1;
                    segment.full_snapshots += u64::from(snapshot.delta_num.is_none());
                    segment.first_time.get_or_insert(snapshot.server_time);
                    segment.last_time = Some(snapshot.server_time);
                    segment.max_entities = segment.max_entities.max(snapshot.entities.len());
                    let fields = &snapshot.player_state.fields;
                    let origin = ["origin[0]", "origin[1]", "origin[2]"]
                        .map(|key| fields.field_f32(key).unwrap());
                    let velocity = ["velocity[0]", "velocity[1]", "velocity[2]"]
                        .map(|key| fields.field_f32(key).unwrap());
                    if origin
                        .iter()
                        .chain(&velocity)
                        .any(|value| !value.is_finite())
                    {
                        segment.nonfinite_snapshots += 1;
                    }
                    segment.first_origin.get_or_insert(origin);
                    segment.last_origin = Some(origin);
                    on_snapshot(&decoder, &snapshot)?;
                }
                Event::DroppedSnapshot { .. } => {
                    if let Some(segment) = summary.segments.last_mut() {
                        segment.dropped_snapshots += 1;
                    }
                }
                Event::ServerCommand(_) => summary.server_commands += 1,
                _ => {}
            }
        }
    }
    if summary.segments.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "JK2 demo contains no gamestate",
        ));
    }
    Ok(summary)
}

#[cfg(test)]
mod recording_tests {
    use super::*;
    use crate::demo::{write_end, write_record, DemoReader};

    /// Rebuild JK2MV's synthetic starting gamestate from real 1.02/1.04
    /// recordings, then decode the resulting native .dm_15/.dm_16 stream.
    /// This also verifies that the entire baseline delta is lossless.
    #[test]
    fn synthetic_gamestate_roundtrips_both_jk2_wire_protocols() {
        for (version, source) in [
            (Version::V102, include_bytes!("../../tests/fixtures/jk2-102-prefix.dm_15").as_slice()),
            (Version::V104, include_bytes!("../../tests/fixtures/jk2-104-prefix.dm_16").as_slice()),
        ] {
            let mut input = DemoReader::new(source);
            let mut original = Decoder::new();
            let initial_sequence = loop {
                let record = input.next_record().unwrap().expect("JK2 fixture needs a gamestate");
                let packet = original.parse_packet(record.sequence, &record.payload).unwrap();
                if packet.events.iter().any(|event| matches!(event, Event::Gamestate)) {
                    break record.sequence;
                }
            };
            assert_eq!(original.version(), Some(version));
            let payload = synthesize_gamestate_payload(&original, 17).unwrap();
            assert!(payload.len() <= super::super::MAX_MESSAGE_BYTES);
            let mut bytes = Vec::new();
            write_record(&mut bytes, initial_sequence, &payload).unwrap();
            write_end(&mut bytes).unwrap();
            let mut recorded = DemoReader::new(bytes.as_slice());
            let initial = recorded.next_record().unwrap().unwrap();
            assert_eq!(initial.sequence, initial_sequence);
            let mut decoded = Decoder::new();
            let packet = decoded.parse_packet(initial.sequence, &initial.payload).unwrap();
            assert!(packet.events.iter().any(|event| matches!(event, Event::Gamestate)));
            assert_eq!(packet.reliable_acknowledge, 17);
            assert_eq!(decoded.version(), original.version());
            assert_eq!(decoded.server_command_sequence, original.server_command_sequence);
            assert_eq!(decoded.client_number, original.client_number);
            assert_eq!(decoded.checksum_feed, original.checksum_feed);
            assert_eq!(decoded.configstrings, original.configstrings);
            assert_eq!(decoded.baselines, original.baselines);
            assert!(recorded.next_record().unwrap().is_none());
        }
    }
}
