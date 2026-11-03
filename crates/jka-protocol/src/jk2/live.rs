// SPDX-License-Identifier: GPL-2.0-only
//! Live JK2 decoding and presentation boundary. Wire command text and sequence
//! numbers stay intact for acknowledgements / XOR keys; only CGame sees mapped
//! configstring indices. No JKA movement implementation receives JK2 state.

use super::{presentation::{self, Adapter}, Decoder, Event, Version};
use crate::{commands::{atoi, tokenize, BigConfigOutcome, BigConfigString}, server};

pub(crate) struct LiveDecoder {
    pub version: Version,
    pub wire: Decoder,
    adapter: Adapter,
    big: BigConfigString,
    /// The connection's one default `team spectator` request was queued.
    pub default_team_sent: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_and_offline_presentation_match_real_wire_snapshots() {
        for (version, bytes) in [
            (Version::V102, include_bytes!("../../tests/fixtures/jk2-102-prefix.dm_15").as_slice()),
            (Version::V104, include_bytes!("../../tests/fixtures/jk2-104-prefix.dm_16").as_slice()),
        ] {
            let mut live = LiveDecoder::new(version);
            let mut view = server::Decoder::new();
            let converted = presentation::convert_demo(bytes).unwrap();
            let mut expected_records = crate::demo::DemoReader::new(std::io::Cursor::new(&converted.bytes));
            let mut expected = server::Decoder::new();
            let mut records = crate::demo::DemoReader::new(std::io::Cursor::new(bytes));
            let mut count = 0;
            while let Some(record) = records.next_record().unwrap() {
                let packet = live.parse(record.sequence, &record.payload, &mut view).unwrap();
                for event in packet.events {
                    match event {
                        server::Event::ServerCommand(command) => { live.command(command, &mut view).unwrap(); }
                        server::Event::Snapshot { .. } => {
                            loop {
                                let record = expected_records.next_record().unwrap().unwrap();
                                if expected.parse_packet(record.sequence, &record.payload).unwrap().events.iter()
                                    .any(|e| matches!(e, server::Event::Snapshot { .. })) { break; }
                            }
                            let actual = view.latest_snapshot().unwrap();
                            let expected = expected.latest_snapshot().unwrap();
                            assert_eq!(actual.server_time, expected.server_time);
                            assert_eq!(actual.player_state, expected.player_state);
                            assert_eq!(actual.entities, expected.entities);
                            assert_eq!(view.configstrings, expected_configstrings(bytes, actual.server_time));
                            count += 1;
                        }
                        _ => {}
                    }
                }
            }
            assert_eq!(count, converted.snapshots);
        }
    }

    fn expected_configstrings(bytes: &[u8], time: i32) -> std::collections::BTreeMap<u16, Vec<u8>> {
        let mut raw = Decoder::new();
        let mut records = crate::demo::DemoReader::new(std::io::Cursor::new(bytes));
        while let Some(record) = records.next_record().unwrap() {
            let packet = raw.parse_packet(record.sequence, &record.payload).unwrap();
            if packet.events.iter().any(|e| matches!(e, Event::Snapshot(s) if s.server_time == time)) { break; }
        }
        raw.configstrings.iter().filter_map(|(&i, v)| presentation::configstring_index(i)
            .map(|dest| (dest, presentation::configstring(i, v).unwrap()))).collect()
    }

    #[test]
    fn config_commands_translate_indices_but_leave_wire_text_intact() {
        let mut live = LiveDecoder::new(Version::V104);
        let mut view = server::Decoder::new();
        let wire = server::ServerCommand { sequence: 7, text: b"cs 544 \"n\\New Name\\model\\kyle/blue\"".to_vec() };
        let mapped = live.command(wire.clone(), &mut view).unwrap().unwrap();
        assert_eq!(wire.text, b"cs 544 \"n\\New Name\\model\\kyle/blue\"");
        assert!(mapped.text.starts_with(b"cs 1131 "));
        assert_eq!(mapped.sequence, wire.sequence);
        assert!(view.configstrings.contains_key(&1131));
        let start = server::ServerCommand { sequence: 8, text: b"bcs0 288 \"sound/\"".to_vec() };
        assert!(live.command(start, &mut view).unwrap().is_none());
        let end = server::ServerCommand { sequence: 9, text: b"bcs2 288 \"test.wav\"".to_vec() };
        assert_eq!(live.command(end, &mut view).unwrap().unwrap().text, b"cs 811 \"sound/test.wav\"");
    }
}

impl LiveDecoder {
    pub fn new(version: Version) -> Self {
        Self { version, wire: Decoder::for_live(version), adapter: Adapter::default(), big: BigConfigString::default(), default_team_sent: false }
    }

    pub fn parse(&mut self, sequence: i32, payload: &[u8], view: &mut server::Decoder)
        -> Result<server::PacketResult, String>
    {
        let packet = self.wire.parse_packet(sequence, payload).map_err(|e| e.to_string())?;
        let mut events = Vec::new();
        for event in packet.events {
            match event {
                Event::Gamestate => {
                    if self.wire.version() != Some(self.version) {
                        return Err(format!("Selected JK2 {} but gamestate reports another version", self.version.name()));
                    }
                    *view = server::Decoder::new();
                    self.big = BigConfigString::default();
                    for (&index, value) in &self.wire.configstrings {
                        if let Some(dest) = presentation::configstring_index(index) {
                            view.configstrings.insert(dest, presentation::configstring(index, value)?);
                        }
                    }
                    for (&number, entity) in &self.wire.baselines {
                        view.baselines.insert(number, self.adapter.entity(self.version, entity)?);
                    }
                    view.client_number = self.wire.client_number;
                    view.checksum_feed = self.wire.checksum_feed;
                    view.server_command_sequence = self.wire.server_command_sequence;
                    events.push(server::Event::Gamestate {
                        server_command_sequence: view.server_command_sequence,
                        configstrings: view.configstrings.len(), baselines: view.baselines.len(),
                        client_number: view.client_number, checksum_feed: view.checksum_feed,
                    });
                }
                Event::ServerCommand(command) => {
                    // The session stores this exact text before translating it.
                    view.server_command_sequence = command.sequence;
                    events.push(server::Event::ServerCommand(command));
                }
                Event::Snapshot(raw) => {
                    let mut snapshot = self.adapter.snapshot(self.version, &raw, sequence, raw.server_command_sequence)?;
                    snapshot.delta_num = raw.delta_num.unwrap_or(-1);
                    events.push(server::Event::Snapshot {
                        server_time: snapshot.server_time, message_num: sequence,
                        delta_num: snapshot.delta_num, entities: snapshot.entities.len(),
                    });
                    view.set_presentation_snapshot(snapshot);
                }
                Event::Download(block) => events.push(server::Event::Download(block)),
                Event::MapChange => events.push(server::Event::MapChange),
                Event::Nop | Event::DroppedSnapshot { .. } | Event::UnprimedSnapshot { .. } => {}
            }
        }
        view.reliable_acknowledge = packet.reliable_acknowledge;
        Ok(server::PacketResult { sequence, reliable_acknowledge: packet.reliable_acknowledge,
            events, consumed_bits: packet.consumed_bits, consumed_bytes: (packet.consumed_bits + 7) / 8 })
    }

    pub fn command(&mut self, command: server::ServerCommand, view: &mut server::Decoder)
        -> Result<Option<server::ServerCommand>, String>
    {
        let text = match self.big.feed(&command.text) {
            BigConfigOutcome::PassThrough => command.text,
            BigConfigOutcome::Pending => return Ok(None),
            BigConfigOutcome::Complete(text) => text,
            BigConfigOutcome::Overflow => return Err("JK2 configstring overflow".into()),
        };
        let args = tokenize(&text);
        if args.first().map(Vec::as_slice) != Some(b"cs") {
            return Ok(Some(server::ServerCommand { sequence: command.sequence, text }));
        }
        let index = args.get(1).map_or(-1, |s| atoi(s));
        let Some(dest) = u16::try_from(index).ok().and_then(presentation::configstring_index) else { return Ok(None) };
        let value = presentation::configstring(index as u16, args.get(2).map(Vec::as_slice).unwrap_or_default())?;
        view.configstrings.insert(dest, value.clone());
        let mut text = format!("cs {dest} \"").into_bytes();
        text.extend_from_slice(&value);
        text.push(b'"');
        Ok(Some(server::ServerCommand { sequence: command.sequence, text }))
    }
}
