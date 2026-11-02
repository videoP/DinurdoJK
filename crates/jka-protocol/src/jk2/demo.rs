//! Streaming JK2 demo inspection. This decodes the entire recording, without
//! constructing a JKA cgame session or loading any game assets/native physics.

use std::io::{self, Read};

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
