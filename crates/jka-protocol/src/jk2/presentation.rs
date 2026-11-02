// SPDX-License-Identifier: GPL-2.0-only
//! Offline presentation adapter. JK2 wire state remains owned by the JK2 decoder.
//! This maps semantic IDs into the existing renderer's representation and emits
//! an in-memory playback stream. It is never sent to a server or saved over the
//! source recording, and never runs movement prediction or invents animation frames.

use super::{presentation_ids as ids, Decoder, EntityState, Event, Snapshot, Version};
use crate::{
    commands::{tokenize, BigConfigOutcome, BigConfigString},
    demo,
    gamestate::{self, ENTITY_FIELDS},
    message::MessageWriter,
    server,
};
use std::{collections::BTreeMap, io::Cursor};

pub struct ConvertedDemo {
    pub bytes: Vec<u8>,
    pub version: Version,
    pub snapshots: usize,
    pub omitted_events: usize,
}

fn mapped(table: &[i32], value: i32) -> Option<i32> {
    usize::try_from(value)
        .ok()
        .and_then(|i| table.get(i))
        .copied()
        .filter(|&v| v >= 0)
}

pub fn animation(version: Version, value: i32) -> Result<i32, String> {
    let table = match version {
        Version::V102 => ids::ANIMATIONS_102,
        Version::V104 => ids::ANIMATIONS_104,
    };
    mapped(table, value & !2048)
        .ok_or_else(|| format!("Unknown JK2 {} animation {value}", version.name()))
}

fn bitmask(table: &[i32], value: i32) -> i32 {
    table
        .iter()
        .take(32)
        .enumerate()
        .fold(0, |bits, (i, &dest)| {
            if value as u32 & (1u32 << i) != 0 && (0..32).contains(&dest) {
                bits | (1u32 << dest) as i32
            } else {
                bits
            }
        })
}

fn flags(value: i32) -> i32 {
    ids::ENTITY_FLAGS
        .iter()
        .fold(0u32, |bits, &(source, dest)| {
            bits | if value as u32 & source != 0 { dest } else { 0 }
        }) as i32
}

pub fn configstring_index(index: u16) -> Option<u16> {
    match index {
        0..=25 | 27..=30 => Some(index),
        32..=287 => Some(298 + index - 32),
        288..=543 => Some(811 + index - 288),
        544..=575 => Some(1131 + index - 544),
        640..=703 => Some(1227 + index - 640),
        768..=831 => Some(1355 + index - 768),
        832..=1023 => Some(1419 + index - 832),
        _ => None,
    }
}

fn serverinfo(value: &[u8]) -> Result<Vec<u8>, String> {
    let parts: Vec<_> = value.split(|&b| b == b'\\').skip(1).collect();
    let mut result = Vec::new();
    for pair in parts.chunks_exact(2) {
        let mut v = pair[1].to_vec();
        if pair[0].eq_ignore_ascii_case(b"g_gametype") {
            v = match pair[1] {
                b"0" | b"1" | b"2" | b"3" => pair[1].to_vec(),
                b"4" => b"5".to_vec(),
                b"5" => b"6".to_vec(),
                b"7" => b"8".to_vec(),
                b"8" => b"9".to_vec(),
                _ => return Err("JK2 Saga/unknown gametype presentation is not implemented".into()),
            };
        }
        result.push(b'\\');
        result.extend_from_slice(pair[0]);
        result.push(b'\\');
        result.extend_from_slice(&v);
    }
    Ok(result)
}

pub(crate) fn configstring(index: u16, value: &[u8]) -> Result<Vec<u8>, String> {
    if index == 0 {
        return serverinfo(value);
    }
    if (544..576).contains(&index) && !value.is_empty() {
        // JK2 has one native saber, rather than JKA's named .sab loadout.
        let mut result = b"\\st\\__jk2_native\\st2\\none".to_vec();
        if value.first() != Some(&b'\\') {
            result.push(b'\\');
        }
        result.extend_from_slice(value);
        return Ok(result);
    }
    if index == 27 {
        let mut items = vec![b'0'; 51];
        for (i, &v) in value.iter().enumerate() {
            if let Some(dest) = mapped(ids::ITEMS, i as i32).filter(|&n| n < 51) {
                items[dest as usize] = v;
            }
        }
        return Ok(items);
    }
    Ok(value.to_vec())
}

#[derive(Default)]
pub(crate) struct Adapter {
    pub(crate) omitted_events: usize,
}

impl Adapter {
    fn event(&mut self, value: i32) -> i32 {
        match mapped(ids::EVENTS, value & !0x300) {
            Some(v) => v | (value & 0x300),
            None => {
                self.omitted_events += 1;
                0
            }
        }
    }

    fn event_parm(&self, raw_event: i32, parm: i32) -> i32 {
        let name = mapped(ids::EVENTS, raw_event & !0x300)
            .and_then(crate::entity_event::EntityEvent::from_i32);
        use crate::entity_event::EntityEvent;
        match name {
            Some(EntityEvent::EV_ITEM_PICKUP | EntityEvent::EV_GLOBAL_ITEM_PICKUP) => {
                mapped(ids::ITEMS, parm).unwrap_or(0)
            }
            Some(EntityEvent::EV_OBITUARY) => mapped(ids::DEATH_CAUSES, parm).unwrap_or(0),
            _ => parm,
        }
    }

    pub(crate) fn entity(
        &mut self,
        version: Version,
        raw: &EntityState,
    ) -> Result<gamestate::EntityState, String> {
        let mut result = gamestate::EntityState {
            number: raw.number,
            fields: [0; ENTITY_FIELDS.len()],
        };
        let ty = raw.fields.field_i32("eType").unwrap_or(0);
        let events_base = (ids::ENTITY_TYPES.len() - 1) as i32;
        let raw_event = if ty > events_base {
            ty - events_base
        } else {
            raw.fields.field_i32("event").unwrap_or(0)
        };
        for (i, &(name, _)) in ENTITY_FIELDS.iter().enumerate() {
            if name == "legsFlip" || name == "torsoFlip" {
                let source = if name == "legsFlip" {
                    "legsAnim"
                } else {
                    "torsoAnim"
                };
                result.fields[i] = u32::from(raw.fields.field_i32(source).unwrap_or(0) & 2048 != 0);
                continue;
            }
            if name == "saberHolstered" && (ty == 1 || mapped(ids::ENTITY_TYPES, ty) == Some(15)) {
                result.fields[i] = if raw.fields.field_i32("shouldtarget").unwrap_or(0) != 0 {
                    2
                } else {
                    0
                };
                continue;
            }
            let Some(value) = raw.fields.field_bits(name) else {
                continue;
            };
            let value = value as i32;
            let value = match name {
                "eType" if ty > events_base => 18 + self.event(raw_event),
                "eType" => mapped(ids::ENTITY_TYPES, ty)
                    .ok_or_else(|| format!("Unknown JK2 entity type {ty}"))?,
                "eFlags" => flags(value),
                "legsAnim" | "torsoAnim" => animation(version, value)?,
                "weapon" => mapped(ids::WEAPONS, value).unwrap_or(0),
                "saberMove" => {
                    if value < 0 {
                        value
                    } else {
                        mapped(ids::SABER_MOVES, value).unwrap_or(0)
                    }
                }
                "event" => self.event(value),
                "eventParm" => self.event_parm(raw_event, value),
                "powerups" => bitmask(ids::POWERUPS, value),
                "modelindex" if ty == 2 => mapped(ids::ITEMS, value)
                    .ok_or_else(|| format!("Unsupported JK2 item index {value}"))?,
                _ => value,
            };
            result.fields[i] = value as u32;
        }
        Ok(result)
    }

    pub(crate) fn snapshot(
        &mut self,
        version: Version,
        raw: &Snapshot,
        sequence: i32,
        command: i32,
    ) -> Result<server::Snapshot, String> {
        let ps = self.player_state(version, &raw.player_state)?;
        Ok(server::Snapshot {
            server_time: raw.server_time,
            message_num: sequence,
            delta_num: -1,
            snap_flags: raw.snap_flags,
            ping: 0,
            server_command_num: command,
            area_mask: raw.area_mask,
            player_state: ps,
            vehicle_player_state: None,
            entities: raw.entities.iter()
                .map(|entity| self.entity(version, entity)).collect::<Result<_, _>>()?,
        })
    }

    fn player_state(&mut self, version: Version, raw: &super::PlayerState)
        -> Result<server::PlayerState, String>
    {
        let mut ps = server::PlayerState::default();
        for (target, source) in [("legsFlip", "legsAnim"), ("torsoFlip", "torsoAnim")] {
            ps.set_field_bits(
                target,
                u32::from(raw.fields.field_i32(source).unwrap_or(0) & 2048 != 0),
            );
        }
        for &(name, bits) in crate::PLAYER_FIELDS {
            let Some(value) = raw.fields.field_bits(name) else {
                continue;
            };
            let value = value as i32;
            let value = match name {
                "legsAnim" | "torsoAnim" => animation(version, value)?,
                "eFlags" => flags(value),
                "pm_type" => mapped(ids::MOVEMENT_TYPES, value).unwrap_or(0),
                "weapon" => mapped(ids::WEAPONS, value).unwrap_or(0),
                "saberMove" => {
                    if value < 0 {
                        value
                    } else {
                        mapped(ids::SABER_MOVES, value).unwrap_or(0)
                    }
                }
                "forceHandExtend" => mapped(ids::HAND_ANIMS, value).unwrap_or(0),
                "events[0]" | "events[1]" | "externalEvent" => self.event(value),
                "eventParms[0]" => self.event_parm(
                    raw.fields.field_i32("events[0]").unwrap_or(0),
                    value,
                ),
                "eventParms[1]" => self.event_parm(
                    raw.fields.field_i32("events[1]").unwrap_or(0),
                    value,
                ),
                "externalEventParm" => self.event_parm(
                    raw
                        .fields
                        .field_i32("externalEvent")
                        .unwrap_or(0),
                    value,
                ),
                "speed" if bits == 0 => (value as f32).to_bits() as i32,
                _ => value,
            };
            ps.set_field_bits(name, value as u32);
        }
        ps.set_field_bits(
            "saberHolstered",
            if raw
                .fields
                .field_i32("saberActive")
                .unwrap_or(0)
                != 0
            {
                0
            } else {
                2
            },
        );
        ps.set_field_bits("standheight", 32);
        ps.set_field_bits("crouchheight", 16);
        for (i, &value) in raw.stats.iter().enumerate() {
            if let Some(dest) = mapped(ids::STATS, i as i32).filter(|&n| n < 16) {
                ps.stats[dest as usize] = value;
            }
        }
        ps.stats[1] = mapped(ids::ITEMS, raw.stats[1]).unwrap_or(0);
        ps.stats[2] = bitmask(ids::HOLDABLES, raw.stats[2]);
        ps.stats[4] = bitmask(ids::WEAPONS, raw.stats[4]);
        for (i, &value) in raw.persistant.iter().enumerate() {
            if let Some(dest) = mapped(ids::PERSISTANT, i as i32).filter(|&n| n < 16) {
                ps.persistant[dest as usize] = value;
            }
        }
        ps.ammo = raw.ammo;
        for (i, &value) in raw.powerups.iter().enumerate() {
            if let Some(dest) = mapped(ids::POWERUPS, i as i32).filter(|&n| n < 16) {
                ps.powerups[dest as usize] = value;
            }
        }
        Ok(ps)
    }
}

/// Map native JK2 output once. Native normalized state must pass V104 here,
/// including when the server wire version is V102.
pub fn player_state(version: Version, raw: &super::PlayerState) -> Result<server::PlayerState, String> {
    Adapter::default().player_state(version, raw)
}

fn queue_command(commands: &mut Vec<(i32, Vec<u8>)>, counter: &mut i32, text: Vec<u8>) {
    *counter += 1;
    commands.push((*counter, text));
}

fn config_commands(
    commands: &mut Vec<(i32, Vec<u8>)>,
    counter: &mut i32,
    index: u16,
    value: &[u8],
) {
    let prefix = format!("cs {index} \"");
    if prefix.len() + value.len() + 1 < 1024 {
        let mut text = prefix.into_bytes();
        text.extend_from_slice(value);
        text.push(b'"');
        queue_command(commands, counter, text);
    } else {
        let chunks: Vec<_> = value.chunks(800).collect();
        for (i, part) in chunks.iter().enumerate() {
            let verb = if i == 0 {
                "bcs0"
            } else if i + 1 == chunks.len() {
                "bcs2"
            } else {
                "bcs1"
            };
            let mut text = format!("{verb} {index} \"").into_bytes();
            text.extend_from_slice(part);
            text.push(b'"');
            queue_command(commands, counter, text);
        }
    }
}

/// Convert for renderer reuse only. Actual JK2 packets and playerstates retain
/// their own decoder/layout, and no prediction is performed in this adapter.
pub fn convert_demo(bytes: &[u8]) -> Result<ConvertedDemo, String> {
    let mut reader = demo::DemoReader::new(Cursor::new(bytes));
    let mut decoder = Decoder::new();
    let mut adapter = Adapter { omitted_events: 0 };
    let mut output = Vec::new();
    let mut version = None;
    let mut sequence = 0;
    let mut command_counter = 0;
    let mut commands = Vec::new();
    let mut big = BigConfigString::default();
    let mut baselines = BTreeMap::new();
    let mut previous = None;
    let mut snapshots = 0;
    while let Some(record) = reader.next_record().map_err(|e| e.to_string())? {
        let packet = decoder
            .parse_packet(record.sequence, &record.payload)
            .map_err(|e| format!("JK2 record {}: {e}", record.sequence))?;
        for event in packet.events {
            match event {
                Event::Gamestate => {
                    let current_version = decoder.version().unwrap();
                    if version.is_some_and(|v| v != current_version) {
                        return Err("A demo cannot change JK2 asset version during playback".into());
                    }
                    version = Some(current_version);
                    commands.clear();
                    command_counter = 0;
                    big = BigConfigString::default();
                    previous = None;
                    let mut target = server::Decoder::new();
                    for (&index, value) in &decoder.configstrings {
                        if let Some(dest) = configstring_index(index) {
                            target
                                .configstrings
                                .insert(dest, configstring(index, value)?);
                        }
                    }
                    for (&number, entity) in &decoder.baselines {
                        target
                            .baselines
                            .insert(number, adapter.entity(current_version, entity)?);
                    }
                    baselines = target.baselines.clone();
                    target.client_number = decoder.client_number;
                    target.checksum_feed = decoder.checksum_feed;
                    let payload = demo::synthesize_gamestate_payload(&target, 0)
                        .map_err(|e| e.to_string())?;
                    sequence += 1;
                    demo::write_record(&mut output, sequence, &payload)
                        .map_err(|e| e.to_string())?;
                }
                Event::ServerCommand(command) => {
                    let complete;
                    let text = match big.feed(&command.text) {
                        BigConfigOutcome::PassThrough => &command.text,
                        BigConfigOutcome::Pending => continue,
                        BigConfigOutcome::Complete(value) => {
                            complete = value;
                            &complete
                        }
                        BigConfigOutcome::Overflow => {
                            return Err("JK2 presentation configstring overflow".into())
                        }
                    };
                    let args = tokenize(text);
                    if args.first().map(Vec::as_slice) == Some(b"cs") {
                        let index = args
                            .get(1)
                            .and_then(|s| std::str::from_utf8(s).ok())
                            .and_then(|s| s.parse::<u16>().ok())
                            .ok_or("JK2 configstring command index")?;
                        if let Some(dest) = configstring_index(index) {
                            config_commands(
                                &mut commands,
                                &mut command_counter,
                                dest,
                                &configstring(
                                    index,
                                    args.get(2).map(Vec::as_slice).unwrap_or_default(),
                                )?,
                            );
                        }
                    } else {
                        queue_command(&mut commands, &mut command_counter, text.to_vec());
                    }
                }
                Event::Snapshot(raw) => {
                    sequence += 1;
                    let mut current = adapter.snapshot(
                        version.ok_or("Snapshot before JK2 version")?,
                        &raw,
                        sequence,
                        command_counter,
                    )?;
                    crate::demo_bake::canonicalize_snapshot(&mut current);
                    let payload = crate::demo_bake::synthesize_snapshot_payload(
                        sequence,
                        0,
                        previous.as_ref(),
                        &current,
                        &baselines,
                        &commands,
                    )
                    .map_err(|e| e.to_string())?;
                    demo::write_record(&mut output, sequence, &payload)
                        .map_err(|e| e.to_string())?;
                    previous = Some(current);
                    commands.clear();
                    snapshots += 1;
                }
                _ => {}
            }
        }
        if output.len() > 512 * 1024 * 1024 {
            return Err("JK2 presentation stream exceeds 512 MiB".into());
        }
    }
    if !commands.is_empty() {
        let mut w = MessageWriter::new(demo::MAX_MESSAGE_BYTES).map_err(|e| e.to_string())?;
        w.write_bits(0, 32).map_err(|e| e.to_string())?;
        for (n, text) in commands {
            w.write_bits(i32::from(server::SVC_SERVER_COMMAND), 8)
                .map_err(|e| e.to_string())?;
            w.write_bits(n, 32).map_err(|e| e.to_string())?;
            for byte in text.into_iter().chain([0]) {
                w.write_bits(i32::from(byte), 8)
                    .map_err(|e| e.to_string())?;
            }
        }
        w.write_bits(i32::from(server::SVC_EOF), 8)
            .map_err(|e| e.to_string())?;
        sequence += 1;
        demo::write_record(&mut output, sequence, w.as_bytes()).map_err(|e| e.to_string())?;
    }
    demo::write_end(&mut output).map_err(|e| e.to_string())?;
    Ok(ConvertedDemo {
        bytes: output,
        version: version.ok_or("JK2 demo has no gamestate")?,
        snapshots,
        omitted_events: adapter.omitted_events,
    })
}

/// Differential pose check against the authoritative decoded recording. Runs
/// only on explicit validation, never during rendering or JKA playback.
pub fn validate_conversion(source: &[u8], converted: &ConvertedDemo) -> Result<usize, String> {
    let mut raw_records = demo::DemoReader::new(Cursor::new(source));
    let mut raw_decoder = Decoder::new();
    let mut target_records = demo::DemoReader::new(Cursor::new(&converted.bytes));
    let mut target_decoder = server::Decoder::new();
    let mut count = 0;
    while let Some(record) = raw_records.next_record().map_err(|e| e.to_string())? {
        let packet = raw_decoder
            .parse_packet(record.sequence, &record.payload)
            .map_err(|e| e.to_string())?;
        for event in packet.events {
            let Event::Snapshot(raw) = event else {
                continue;
            };
            loop {
                let record = target_records
                    .next_record()
                    .map_err(|e| e.to_string())?
                    .ok_or("Presentation ended before the source snapshots")?;
                let result = target_decoder
                    .parse_packet(record.sequence, &record.payload)
                    .map_err(|e| e.to_string())?;
                if result
                    .events
                    .iter()
                    .any(|e| matches!(e, server::Event::Snapshot { .. }))
                {
                    break;
                }
            }
            let target = target_decoder
                .latest_snapshot()
                .ok_or("Presentation snapshot missing")?;
            for (anim, flip) in [("legsAnim", "legsFlip"), ("torsoAnim", "torsoFlip")] {
                let raw_value = raw.player_state.fields.field_i32(anim).unwrap();
                let expected_anim = animation(converted.version, raw_value)?;
                let expected_flip = i32::from(raw_value & 2048 != 0);
                if target.player_state.field_i32(anim) != Some(expected_anim)
                    || target.player_state.field_i32(flip) != Some(expected_flip)
                {
                    return Err(format!(
                        "Animation restart mapping diverged at source message {}",
                        raw.message_num
                    ));
                }
            }
            if target.server_time != raw.server_time || target.area_mask != raw.area_mask {
                return Err(format!(
                    "First presentation divergence: source message {}, snapshot time/areamask",
                    raw.message_num
                ));
            }
            for field in [
                "commandTime",
                "clientNum",
                "origin[0]",
                "origin[1]",
                "origin[2]",
                "velocity[0]",
                "velocity[1]",
                "velocity[2]",
                "viewangles[0]",
                "viewangles[1]",
                "viewangles[2]",
            ] {
                let expected = raw.player_state.fields.field_bits(field).unwrap();
                let actual = target.player_state.field_bits(field).unwrap();
                let both_zero = field.contains('[')
                    && f32::from_bits(expected) == 0.0
                    && f32::from_bits(actual) == 0.0;
                if expected != actual && !both_zero {
                    return Err(format!("First presentation divergence: source message {} field {field}: {expected:#010x} != {actual:#010x}", raw.message_num));
                }
            }
            if target.entities.len() != raw.entities.len() {
                return Err(format!(
                    "First presentation divergence: source message {} entity count",
                    raw.message_num
                ));
            }
            for (a, b) in raw.entities.iter().zip(&target.entities) {
                if a.number != b.number {
                    return Err(format!(
                        "Entity identity divergence at source message {}",
                        raw.message_num
                    ));
                }
                for field in [
                    "pos.trTime",
                    "pos.trType",
                    "pos.trBase[0]",
                    "pos.trBase[1]",
                    "pos.trBase[2]",
                    "pos.trDelta[0]",
                    "pos.trDelta[1]",
                    "pos.trDelta[2]",
                ] {
                    let expected = a.fields.field_bits(field).unwrap();
                    let actual = b.field_bits(field).unwrap();
                    let both_zero = field.contains('[')
                        && f32::from_bits(expected) == 0.0
                        && f32::from_bits(actual) == 0.0;
                    if expected != actual && !both_zero {
                        return Err(format!(
                            "Entity {} field {field} diverged at source message {}",
                            a.number, raw.message_num
                        ));
                    }
                }
            }
            count += 1;
        }
    }
    if count != converted.snapshots {
        return Err("Presentation snapshot count changed".into());
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_demos_keep_timing_pose_and_map_while_ids_are_mapped() {
        for bytes in [
            include_bytes!("../../tests/fixtures/jk2-102-prefix.dm_15").as_slice(),
            include_bytes!("../../tests/fixtures/jk2-104-prefix.dm_16").as_slice(),
        ] {
            let source = super::super::demo::inspect(bytes).unwrap();
            let result = convert_demo(bytes).unwrap();
            assert_eq!(
                validate_conversion(bytes, &result).unwrap(),
                result.snapshots
            );
            let mut records = demo::DemoReader::new(Cursor::new(&result.bytes));
            let mut decoder = server::Decoder::new();
            let mut first = None;
            let mut last = None;
            let mut count = 0;
            while let Some(record) = records.next_record().unwrap() {
                let packet = decoder
                    .parse_packet(record.sequence, &record.payload)
                    .unwrap();
                for event in packet.events {
                    if let server::Event::Snapshot { server_time, .. } = event {
                        first.get_or_insert(server_time);
                        last = Some(server_time);
                        count += 1;
                    }
                }
            }
            assert_eq!(count, result.snapshots);
            assert_eq!(first, source.segments[0].first_time);
            assert_eq!(last, source.segments[0].last_time);
            assert_eq!(
                decoder.map_name().as_deref(),
                Some(source.segments[0].map.as_str())
            );
            let origin = ["origin[0]", "origin[1]", "origin[2]"].map(|name| {
                decoder
                    .latest_snapshot()
                    .unwrap()
                    .player_state
                    .field_f32(name)
                    .unwrap()
            });
            assert_eq!(Some(origin), source.segments[0].last_origin);
            assert_eq!(configstring_index(544), Some(1131));
            assert!(decoder.configstrings.get(&1131).is_some());
            let mut raw_decoder = Decoder::new();
            let mut raw_records = demo::DemoReader::new(Cursor::new(bytes));
            while let Some(record) = raw_records.next_record().unwrap() {
                raw_decoder
                    .parse_packet(record.sequence, &record.payload)
                    .unwrap();
            }
            for slot in 0..32 {
                if let Some(info) = raw_decoder
                    .configstrings
                    .get(&(544 + slot))
                    .filter(|info| !info.is_empty())
                {
                    let target = decoder.configstrings.get(&(1131 + slot)).unwrap();
                    for key in [b"model".as_slice(), b"n".as_slice()] {
                        assert_eq!(
                            crate::commands::info_value(target, key),
                            crate::commands::info_value(info, key)
                        );
                    }
                }
            }
        }
    }
}
