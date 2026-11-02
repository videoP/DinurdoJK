// SPDX-License-Identifier: GPL-2.0-only
//! JK2 server-message decoding, isolated from the protocol-26 JKA decoder.
//!
//! Ported against JK2MV 7d601454c3db68492289d4d4e3dc30bff39e4246:
//! src/qcommon/msg.cpp and src/client/cl_parse.cpp. See LICENSE-JK2MV.
//! Wire fields remain raw bits in their own schema; no native struct casts or
//! JKA vehicle selectors occur here. Discard a decoder after a parsing error.

use std::collections::BTreeMap;

use crate::{
    commands::{info_value, tokenize, BigConfigOutcome, BigConfigString},
    message::{Error, ErrorKind, MessageReader},
    server::{DownloadBlock, ServerCommand},
};

pub mod demo;
pub mod fields;
pub mod presentation;
mod presentation_ids;

pub const MAX_MESSAGE_BYTES: usize = 16384;
pub const MAX_CONFIGSTRINGS: u16 = 1400;
pub const MAX_GAMESTATE_CHARS: usize = 16000;
pub const DEFAULT_PORT: u16 = 28070;
const PACKET_BACKUP: usize = 32;
const ENTITY_NONE: u16 = 1023;

const SVC_NOP: u8 = 1;
const SVC_GAMESTATE: u8 = 2;
const SVC_CONFIGSTRING: u8 = 3;
const SVC_BASELINE: u8 = 4;
const SVC_SERVER_COMMAND: u8 = 5;
const SVC_DOWNLOAD: u8 = 6;
const SVC_SNAPSHOT: u8 = 7;
const SVC_MAPCHANGE: u8 = 8;
const SVC_EOF: u8 = 9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    V102,
    V104,
}

impl Version {
    pub fn name(self) -> &'static str {
        match self {
            Self::V102 => "1.02",
            Self::V104 => "1.04",
        }
    }

    pub fn protocol(self) -> u32 {
        match self {
            Self::V102 => 15,
            Self::V104 => 16,
        }
    }

    pub fn entity_fields(self) -> &'static [(&'static str, i8)] {
        match self {
            Self::V102 => fields::ENTITY_FIELDS_102,
            Self::V104 => fields::ENTITY_FIELDS_104,
        }
    }

    pub fn player_fields(self) -> &'static [(&'static str, i8)] {
        match self {
            Self::V102 => fields::PLAYER_FIELDS_102,
            Self::V104 => fields::PLAYER_FIELDS_104,
        }
    }

    pub fn from_serverinfo(info: &[u8]) -> Result<Self, ErrorKind> {
        let version = info_value(info, b"version").unwrap_or_default();
        // JK2MV's CL_ServerVersionIs103: protocol 15 alone is ambiguous.
        if version.windows(5).any(|part| part == b"v1.03") {
            return Err(ErrorKind::Unsupported("JK2 1.03 demo/gameplay"));
        }
        match info_value(info, b"protocol") {
            Some(b"15") => Ok(Self::V102),
            Some(b"16") => Ok(Self::V104),
            _ => Err(ErrorKind::Unsupported(
                "missing or non-JK2 protocol in serverinfo",
            )),
        }
    }
}

/// An explicit wire schema and raw values. Floats retain their IEEE-754 bits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fields {
    schema: &'static [(&'static str, i8)],
    values: Vec<u32>,
}

impl Fields {
    fn zero(schema: &'static [(&'static str, i8)]) -> Self {
        Self {
            schema,
            values: vec![0; schema.len()],
        }
    }

    pub fn field_bits(&self, name: &str) -> Option<u32> {
        self.schema
            .iter()
            .position(|&(key, _)| key == name)
            .map(|i| self.values[i])
    }

    pub fn field_i32(&self, name: &str) -> Option<i32> {
        self.field_bits(name).map(|value| value as i32)
    }

    pub fn field_f32(&self, name: &str) -> Option<f32> {
        self.field_bits(name).map(f32::from_bits)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&'static str, u32)> + '_ {
        self.schema
            .iter()
            .zip(&self.values)
            .map(|(&(name, _), &value)| (name, value))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityState {
    pub number: u16,
    pub fields: Fields,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerState {
    pub fields: Fields,
    pub stats: [i32; 16],
    pub persistant: [i32; 16],
    pub ammo: [i32; 16],
    pub powerups: [i32; 16],
}

impl PlayerState {
    fn zero(version: Version) -> Self {
        Self {
            fields: Fields::zero(version.player_fields()),
            stats: [0; 16],
            persistant: [0; 16],
            ammo: [0; 16],
            powerups: [0; 16],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub message_num: i32,
    pub server_time: i32,
    pub delta_num: Option<i32>,
    pub snap_flags: u8,
    pub server_command_sequence: i32,
    pub area_mask: [u8; 32],
    pub player_state: PlayerState,
    pub entities: Vec<EntityState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Nop,
    Gamestate,
    ServerCommand(ServerCommand),
    Snapshot(Box<Snapshot>),
    /// The packet was consumed, but its required delta base was unavailable.
    DroppedSnapshot {
        message_num: i32,
        delta_num: i32,
    },
    Download(DownloadBlock),
    MapChange,
}

#[derive(Debug)]
pub struct PacketResult {
    pub reliable_acknowledge: i32,
    pub events: Vec<Event>,
    pub consumed_bits: usize,
}

#[derive(Debug, Default)]
pub struct Decoder {
    version: Option<Version>,
    pub reliable_acknowledge: i32,
    pub server_command_sequence: i32,
    pub configstrings: BTreeMap<u16, Vec<u8>>,
    pub baselines: BTreeMap<u16, EntityState>,
    pub client_number: i32,
    pub checksum_feed: u32,
    snapshots: [Option<Snapshot>; PACKET_BACKUP],
    big_config: BigConfigString,
    have_gamestate: bool,
}

impl Decoder {
    pub fn new() -> Self {
        Self {
            client_number: -1,
            ..Self::default()
        }
    }

    pub fn version(&self) -> Option<Version> {
        self.version
    }

    pub fn map_name(&self) -> Option<String> {
        let info = self.configstrings.get(&0)?;
        String::from_utf8(info_value(info, b"mapname")?.to_vec()).ok()
    }

    pub fn parse_packet(&mut self, sequence: i32, payload: &[u8]) -> Result<PacketResult, Error> {
        if payload.len() > MAX_MESSAGE_BYTES {
            return Err(Error {
                bit: 0,
                kind: ErrorKind::Limit("JK2 message bytes"),
            });
        }
        if sequence < 0 {
            return Err(Error {
                bit: 0,
                kind: ErrorKind::InvalidValue("JK2 message sequence"),
            });
        }
        let mut reader = MessageReader::new(payload)?;
        self.reliable_acknowledge = reader.read_long()?;
        let mut events = Vec::new();
        loop {
            match reader.read_byte()? {
                SVC_EOF => break,
                SVC_NOP => events.push(Event::Nop),
                SVC_GAMESTATE => {
                    self.read_gamestate(&mut reader)?;
                    events.push(Event::Gamestate);
                }
                SVC_SERVER_COMMAND => {
                    let command = ServerCommand {
                        sequence: reader.read_long()?,
                        text: reader.read_string(1024)?,
                    };
                    if command.sequence > self.server_command_sequence {
                        self.apply_config_command(&command.text, &reader)?;
                        self.server_command_sequence = command.sequence;
                        events.push(Event::ServerCommand(command));
                    }
                }
                SVC_SNAPSHOT => events.push(self.read_snapshot(&mut reader, sequence)?),
                SVC_MAPCHANGE => events.push(Event::MapChange),
                SVC_DOWNLOAD => {
                    let block = reader.read_short()? as u16;
                    let file_size = if block == 0 {
                        Some(reader.read_long()?)
                    } else {
                        None
                    };
                    let (error, data) = if file_size.is_some_and(|size| size < 0) {
                        (Some(reader.read_string(1024)?), Vec::new())
                    } else {
                        let size = reader.read_short()?;
                        if size < 0 || size as usize > MAX_MESSAGE_BYTES {
                            return Err(reader.error(ErrorKind::Limit("JK2 download block")));
                        }
                        (None, reader.read_data(size as usize)?)
                    };
                    events.push(Event::Download(DownloadBlock {
                        block,
                        file_size,
                        error,
                        data,
                    }));
                }
                _ => return Err(reader.error(ErrorKind::InvalidValue("JK2 service opcode"))),
            }
        }
        Ok(PacketResult {
            reliable_acknowledge: self.reliable_acknowledge,
            events,
            consumed_bits: reader.bit_position(),
        })
    }

    fn read_gamestate(&mut self, reader: &mut MessageReader<'_>) -> Result<(), Error> {
        self.server_command_sequence = reader.read_long()?;
        self.have_gamestate = false;
        self.version = None;
        self.configstrings.clear();
        self.baselines.clear();
        self.snapshots.fill(None);
        self.big_config = BigConfigString::default();
        let mut total = 1usize;
        loop {
            match reader.read_byte()? {
                SVC_EOF => break,
                SVC_CONFIGSTRING => {
                    let index = reader.read_short()? as u16;
                    if index >= MAX_CONFIGSTRINGS {
                        return Err(reader.error(ErrorKind::InvalidValue("JK2 configstring index")));
                    }
                    let value = reader.read_string(8192)?;
                    total += value.len() + 1;
                    if total > MAX_GAMESTATE_CHARS {
                        return Err(reader.error(ErrorKind::Limit("JK2 gamestate strings")));
                    }
                    if index == 0 {
                        self.version = Some(
                            Version::from_serverinfo(&value).map_err(|kind| reader.error(kind))?,
                        );
                    }
                    if self.configstrings.insert(index, value).is_some() {
                        return Err(
                            reader.error(ErrorKind::InvalidValue("duplicate JK2 configstring"))
                        );
                    }
                }
                SVC_BASELINE => {
                    let version = self.version.ok_or_else(|| {
                        reader.error(ErrorKind::Unsupported(
                            "JK2 baseline before version identification",
                        ))
                    })?;
                    let number = reader.read_bits(10)? as u16;
                    if number >= ENTITY_NONE {
                        return Err(
                            reader.error(ErrorKind::InvalidValue("JK2 baseline entity number"))
                        );
                    }
                    let entity = read_entity(reader, version, None, number)?.ok_or_else(|| {
                        reader.error(ErrorKind::InvalidValue("removed JK2 baseline"))
                    })?;
                    self.baselines.insert(number, entity);
                }
                _ => return Err(reader.error(ErrorKind::InvalidValue("JK2 gamestate opcode"))),
            }
        }
        self.version.ok_or_else(|| {
            reader.error(ErrorKind::Unsupported("JK2 gamestate without serverinfo"))
        })?;
        self.client_number = reader.read_long()?;
        self.checksum_feed = reader.read_long()? as u32;
        // JK2 has no JKA RMG short here.
        self.have_gamestate = true;
        Ok(())
    }

    fn apply_config_command(
        &mut self,
        text: &[u8],
        reader: &MessageReader<'_>,
    ) -> Result<(), Error> {
        let complete;
        let text = match self.big_config.feed(text) {
            BigConfigOutcome::PassThrough => text,
            BigConfigOutcome::Pending => return Ok(()),
            BigConfigOutcome::Complete(value) => {
                complete = value;
                &complete
            }
            BigConfigOutcome::Overflow => {
                return Err(reader.error(ErrorKind::Limit("JK2 split configstring")))
            }
        };
        let args = tokenize(text);
        if args.first().map(Vec::as_slice) != Some(b"cs") {
            return Ok(());
        }
        let index = args
            .get(1)
            .and_then(|v| std::str::from_utf8(v).ok())
            .and_then(|s| s.parse::<u16>().ok())
            .filter(|&n| n < MAX_CONFIGSTRINGS)
            .ok_or_else(|| reader.error(ErrorKind::InvalidValue("JK2 cs index")))?;
        let value = args
            .get(2)
            .ok_or_else(|| reader.error(ErrorKind::InvalidValue("JK2 cs value")))?;
        let total = 1
            + value.len()
            + 1
            + self
                .configstrings
                .iter()
                .filter(|(&key, _)| key != index)
                .map(|(_, text)| text.len() + 1)
                .sum::<usize>();
        if total > MAX_GAMESTATE_CHARS {
            return Err(reader.error(ErrorKind::Limit("JK2 updated configstrings")));
        }
        if index == 0
            && Some(Version::from_serverinfo(value).map_err(|kind| reader.error(kind))?)
                != self.version
        {
            return Err(reader.error(ErrorKind::InvalidValue(
                "JK2 version changed outside gamestate",
            )));
        }
        self.configstrings.insert(index, value.clone());
        Ok(())
    }

    fn read_snapshot(
        &mut self,
        reader: &mut MessageReader<'_>,
        sequence: i32,
    ) -> Result<Event, Error> {
        if !self.have_gamestate {
            return Err(reader.error(ErrorKind::InvalidValue("JK2 snapshot before gamestate")));
        }
        let version = self.version.unwrap();
        let server_time = reader.read_long()?;
        let distance = i32::from(reader.read_byte()?);
        let delta_num = (distance != 0).then_some(sequence - distance);
        let snap_flags = reader.read_byte()?;
        let old = delta_num
            .and_then(|n| {
                self.snapshots[(n as usize) & (PACKET_BACKUP - 1)]
                    .as_ref()
                    .filter(|s| s.message_num == n)
            })
            .cloned();
        let area_len = usize::from(reader.read_byte()?);
        if area_len > 32 {
            return Err(reader.error(ErrorKind::InvalidValue("JK2 areamask length")));
        }
        let mut area_mask = [0u8; 32];
        for byte in &mut area_mask[..area_len] {
            *byte = reader.read_byte()?;
        }
        let player_state = read_player(reader, version, old.as_ref().map(|s| &s.player_state))?;
        let entities = self.read_entities(reader, version, old.as_ref())?;
        if delta_num.is_some() && old.is_none() {
            return Ok(Event::DroppedSnapshot {
                message_num: sequence,
                delta_num: delta_num.unwrap(),
            });
        }
        let snapshot = Snapshot {
            message_num: sequence,
            server_time,
            delta_num,
            snap_flags,
            server_command_sequence: self.server_command_sequence,
            area_mask,
            player_state,
            entities,
        };
        self.snapshots[(sequence as usize) & (PACKET_BACKUP - 1)] = Some(snapshot.clone());
        Ok(Event::Snapshot(Box::new(snapshot)))
    }

    fn read_entities(
        &self,
        reader: &mut MessageReader<'_>,
        version: Version,
        old: Option<&Snapshot>,
    ) -> Result<Vec<EntityState>, Error> {
        let old_entities = old.map_or(&[][..], |s| s.entities.as_slice());
        let mut result = Vec::new();
        let mut old_index = 0;
        let mut previous = None;
        loop {
            let number = reader.read_bits(10)? as u16;
            if number == ENTITY_NONE {
                break;
            }
            if previous.is_some_and(|last| number <= last) {
                return Err(reader.error(ErrorKind::InvalidValue("unordered JK2 packet entities")));
            }
            previous = Some(number);
            while old_index < old_entities.len() && old_entities[old_index].number < number {
                result.push(old_entities[old_index].clone());
                old_index += 1;
            }
            let from = if old_index < old_entities.len() && old_entities[old_index].number == number
            {
                let entity = &old_entities[old_index];
                old_index += 1;
                Some(entity)
            } else {
                self.baselines.get(&number)
            };
            if let Some(entity) = read_entity(reader, version, from, number)? {
                result.push(entity);
            }
        }
        result.extend_from_slice(&old_entities[old_index..]);
        Ok(result)
    }
}

fn read_float(reader: &mut MessageReader<'_>) -> Result<u32, Error> {
    if reader.read_bits(1)? == 0 {
        Ok(((reader.read_bits(13)? - 4096) as f32).to_bits())
    } else {
        Ok(reader.read_bits(32)? as u32)
    }
}

fn read_entity(
    reader: &mut MessageReader<'_>,
    version: Version,
    from: Option<&EntityState>,
    number: u16,
) -> Result<Option<EntityState>, Error> {
    if reader.read_bits(1)? != 0 {
        return Ok(None);
    }
    let mut entity = from.cloned().unwrap_or_else(|| EntityState {
        number,
        fields: Fields::zero(version.entity_fields()),
    });
    entity.number = number;
    if reader.read_bits(1)? == 0 {
        return Ok(Some(entity));
    }
    let count = usize::from(reader.read_byte()?);
    if count > entity.fields.schema.len() {
        return Err(reader.error(ErrorKind::InvalidValue("JK2 entity field count")));
    }
    for (i, &(_, width)) in entity.fields.schema.iter().take(count).enumerate() {
        if reader.read_bits(1)? == 0 {
            continue;
        }
        entity.fields.values[i] = if reader.read_bits(1)? == 0 {
            0
        } else if width == 0 {
            read_float(reader)?
        } else {
            reader.read_bits(width)? as u32
        };
    }
    Ok(Some(entity))
}

fn read_player(
    reader: &mut MessageReader<'_>,
    version: Version,
    from: Option<&PlayerState>,
) -> Result<PlayerState, Error> {
    let mut state = from.cloned().unwrap_or_else(|| PlayerState::zero(version));
    // JK2 starts directly at the field count: no JKA pilot/vehicle selector.
    let count = usize::from(reader.read_byte()?);
    if count > state.fields.schema.len() {
        return Err(reader.error(ErrorKind::InvalidValue("JK2 player field count")));
    }
    for (i, &(_, width)) in state.fields.schema.iter().take(count).enumerate() {
        if reader.read_bits(1)? != 0 {
            state.fields.values[i] = if width == 0 {
                read_float(reader)?
            } else {
                reader.read_bits(width)? as u32
            };
        }
    }
    if reader.read_bits(1)? != 0 {
        // Unlike JKA, even STAT_WEAPONS is an ordinary signed short in JK2.
        for (values, width) in [
            (&mut state.stats, 16),
            (&mut state.persistant, 16),
            (&mut state.ammo, 16),
            (&mut state.powerups, 32),
        ] {
            if reader.read_bits(1)? == 0 {
                continue;
            }
            let mask = reader.read_bits(16)? as u32;
            for (i, value) in values.iter_mut().enumerate() {
                if mask & (1 << i) != 0 {
                    *value = if width == 16 {
                        i32::from(reader.read_short()?)
                    } else {
                        reader.read_long()?
                    };
                }
            }
        }
    }
    Ok(state)
}

#[cfg(test)]
mod tests;
