//! Stateful decoder for stock JKA protocol-26 server messages.
//! The same decoder accepts payloads from demos today and netchan packets later.
//! Primary references: OpenJK codemp/client/cl_parse.cpp and codemp/qcommon/msg.cpp.

use std::collections::BTreeMap;

use crate::{
    gamestate::{read_delta_entity, EntityState, MAX_CONFIGSTRINGS, MAX_GAMESTATE_CHARS},
    message::{Error, ErrorKind, MessageReader},
    player_fields::{PILOT_PLAYER_FIELDS, PLAYER_FIELDS, VEHICLE_ONLY_FIELDS, VEHICLE_PLAYER_FIELDS},
};

pub const SVC_BAD: u8 = 0;
pub const SVC_NOP: u8 = 1;
pub const SVC_GAMESTATE: u8 = 2;
pub const SVC_CONFIGSTRING: u8 = 3;
pub const SVC_BASELINE: u8 = 4;
pub const SVC_SERVER_COMMAND: u8 = 5;
pub const SVC_DOWNLOAD: u8 = 6;
pub const SVC_SNAPSHOT: u8 = 7;
pub const SVC_SETGAME: u8 = 8;
pub const SVC_MAPCHANGE: u8 = 9;
pub const SVC_EOF: u8 = 10;

pub const PACKET_BACKUP: usize = 32;
pub const MAX_STATS: usize = 16;
pub const MAX_PERSISTANT: usize = 16;
pub const MAX_AMMO_TRANSMIT: usize = 16;
pub const MAX_POWERUPS: usize = 16;
pub const MAX_WEAPONS: i8 = 19;
pub const STAT_WEAPONS: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerCommand {
    pub sequence: i32,
    pub text: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerState {
    pub fields: Vec<u32>,
    /// Optimized pilot/vehicle fields that are not part of the ordinary schema.
    pub vehicle_fields: Vec<u32>,
    pub stats: [i32; MAX_STATS],
    pub persistant: [i32; MAX_PERSISTANT],
    pub ammo: [i32; MAX_AMMO_TRANSMIT],
    pub powerups: [i32; MAX_POWERUPS],
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            fields: vec![0; PLAYER_FIELDS.len()],
            vehicle_fields: vec![0; VEHICLE_ONLY_FIELDS.len()],
            stats: [0; MAX_STATS],
            persistant: [0; MAX_PERSISTANT],
            ammo: [0; MAX_AMMO_TRANSMIT],
            powerups: [0; MAX_POWERUPS],
        }
    }
}

impl PlayerState {
    pub fn field_bits(&self, name: &str) -> Option<u32> {
        if let Some(index) = PLAYER_FIELDS.iter().position(|(candidate, _)| *candidate == name) {
            return self.fields.get(index).copied();
        }
        VEHICLE_ONLY_FIELDS
            .iter()
            .position(|candidate| *candidate == name)
            .and_then(|index| self.vehicle_fields.get(index).copied())
    }

    fn set_field_bits(&mut self, name: &str, value: u32) -> bool {
        if let Some(index) = PLAYER_FIELDS.iter().position(|(candidate, _)| *candidate == name) {
            self.fields[index] = value;
            return true;
        }
        if let Some(index) = VEHICLE_ONLY_FIELDS.iter().position(|candidate| *candidate == name) {
            self.vehicle_fields[index] = value;
            return true;
        }
        false
    }

    pub fn field_i32(&self, name: &str) -> Option<i32> {
        self.field_bits(name).map(|bits| bits as i32)
    }

    pub fn field_f32(&self, name: &str) -> Option<f32> {
        self.field_bits(name).map(f32::from_bits)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub server_time: i32,
    pub message_num: i32,
    pub delta_num: i32,
    pub snap_flags: u8,
    pub server_command_num: i32,
    /// Portal-area visibility mask from svc_snapshot. Matching Raven/OpenJK
    /// refdef semantics, set bits are areas closed off from the current view.
    /// The wire format carries at most 32 bytes; bytes beyond the packet's
    /// advertised length remain zero (conservatively open).
    pub area_mask: [u8; 32],
    pub player_state: PlayerState,
    pub vehicle_player_state: Option<PlayerState>,
    pub entities: Vec<EntityState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Nop,
    Gamestate {
        server_command_sequence: i32,
        configstrings: usize,
        baselines: usize,
        client_number: i32,
        checksum_feed: u32,
    },
    ServerCommand(ServerCommand),
    Snapshot {
        server_time: i32,
        message_num: i32,
        delta_num: i32,
        entities: usize,
    },
    SetGame(Vec<u8>),
    MapChange,
    Download,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketResult {
    pub sequence: i32,
    pub reliable_acknowledge: i32,
    pub events: Vec<Event>,
    pub consumed_bits: usize,
    pub consumed_bytes: usize,
}

#[derive(Debug)]
pub struct Decoder {
    pub reliable_acknowledge: i32,
    pub server_command_sequence: i32,
    pub configstrings: BTreeMap<u16, Vec<u8>>,
    pub baselines: BTreeMap<u16, EntityState>,
    pub client_number: i32,
    pub checksum_feed: u32,
    snapshots: Vec<Option<Snapshot>>,
    latest_snapshot: Option<Snapshot>,
}

impl Default for Decoder {
    fn default() -> Self {
        Self {
            reliable_acknowledge: 0,
            server_command_sequence: 0,
            configstrings: BTreeMap::new(),
            baselines: BTreeMap::new(),
            client_number: -1,
            checksum_feed: 0,
            snapshots: vec![None; PACKET_BACKUP],
            latest_snapshot: None,
        }
    }
}

impl Decoder {
    pub fn new() -> Self { Self::default() }

    pub fn latest_snapshot(&self) -> Option<&Snapshot> { self.latest_snapshot.as_ref() }

    pub fn map_name(&self) -> Option<String> {
        let serverinfo = self.configstrings.get(&0)?;
        info_value(serverinfo, b"mapname").and_then(|value| String::from_utf8(value.to_vec()).ok())
    }

    pub fn parse_packet(&mut self, sequence: i32, payload: &[u8]) -> Result<PacketResult, Error> {
        let mut reader = MessageReader::new(payload)?;
        let reliable_acknowledge = reader.read_long()?;
        self.reliable_acknowledge = reliable_acknowledge;
        let mut events = Vec::new();

        loop {
            let opcode_bit = reader.bit_position();
            let opcode = reader.read_byte()?;
            match opcode {
                SVC_EOF => break,
                SVC_NOP => events.push(Event::Nop),
                SVC_SERVER_COMMAND => {
                    let command = ServerCommand {
                        sequence: reader.read_long()?,
                        text: reader.read_string(1024)?,
                    };
                    if command.sequence > self.server_command_sequence {
                        self.server_command_sequence = command.sequence;
                        events.push(Event::ServerCommand(command));
                    }
                }
                SVC_GAMESTATE => {
                    let event = self.read_gamestate(&mut reader)?;
                    events.push(event);
                }
                SVC_SNAPSHOT => {
                    if let Some(snapshot) = self.read_snapshot(&mut reader, sequence)? {
                        events.push(Event::Snapshot {
                            server_time: snapshot.server_time,
                            message_num: snapshot.message_num,
                            delta_num: snapshot.delta_num,
                            entities: snapshot.entities.len(),
                        });
                        self.latest_snapshot = Some(snapshot);
                    }
                }
                SVC_SETGAME => {
                    let mut game = Vec::new();
                    loop {
                        if game.len() >= 64 {
                            return Err(reader.error(ErrorKind::Limit("fs_game")));
                        }
                        let byte = reader.read_byte()?;
                        if byte == 0 { break; }
                        game.push(byte);
                    }
                    events.push(Event::SetGame(game));
                }
                SVC_MAPCHANGE => events.push(Event::MapChange),
                SVC_DOWNLOAD => {
                    // Downloads are legal server messages, but demo playback has no useful
                    // filesystem transfer semantics. Fail before guessing the variable body.
                    return Err(Error { bit: opcode_bit, kind: ErrorKind::Unsupported("svc_download") });
                }
                SVC_BAD | SVC_CONFIGSTRING | SVC_BASELINE => {
                    return Err(Error { bit: opcode_bit, kind: ErrorKind::InvalidValue("server service opcode outside gamestate") });
                }
                _ => return Err(Error { bit: opcode_bit, kind: ErrorKind::InvalidValue("server service opcode") }),
            }
        }

        Ok(PacketResult {
            sequence,
            reliable_acknowledge,
            events,
            consumed_bits: reader.bit_position(),
            consumed_bytes: reader.legacy_read_count(),
        })
    }

    fn read_gamestate(&mut self, reader: &mut MessageReader<'_>) -> Result<Event, Error> {
        self.server_command_sequence = reader.read_long()?;
        self.configstrings.clear();
        self.baselines.clear();
        self.snapshots.fill(None);
        self.latest_snapshot = None;
        let mut string_bytes = 1usize;
        loop {
            match reader.read_byte()? {
                SVC_EOF => break,
                SVC_CONFIGSTRING => {
                    let raw = reader.read_short()?;
                    if raw < 0 || raw as u16 >= MAX_CONFIGSTRINGS {
                        return Err(reader.error(ErrorKind::InvalidValue("configstring index")));
                    }
                    let index = raw as u16;
                    let text = reader.read_string(8192)?;
                    string_bytes += text.len() + 1;
                    if string_bytes > MAX_GAMESTATE_CHARS {
                        return Err(reader.error(ErrorKind::Limit("gamestate strings")));
                    }
                    self.configstrings.insert(index, text);
                }
                SVC_BASELINE => {
                    let number = reader.read_bits(10)?;
                    if !(0..1024).contains(&number) {
                        return Err(reader.error(ErrorKind::InvalidValue("baseline entity number")));
                    }
                    let entity = read_delta_entity(reader, None, number as u16)?
                        .ok_or_else(|| reader.error(ErrorKind::InvalidValue("removed gamestate baseline")))?;
                    self.baselines.insert(number as u16, entity);
                }
                _ => return Err(reader.error(ErrorKind::InvalidValue("gamestate service opcode"))),
            }
        }
        self.client_number = reader.read_long()?;
        self.checksum_feed = reader.read_long()? as u32;
        // OpenJK reads and discards the old RMG short unconditionally.
        let _legacy_rmg = reader.read_short()?;
        Ok(Event::Gamestate {
            server_command_sequence: self.server_command_sequence,
            configstrings: self.configstrings.len(),
            baselines: self.baselines.len(),
            client_number: self.client_number,
            checksum_feed: self.checksum_feed,
        })
    }

    fn read_snapshot(&mut self, reader: &mut MessageReader<'_>, sequence: i32) -> Result<Option<Snapshot>, Error> {
        let server_time = reader.read_long()?;
        let delta_distance = i32::from(reader.read_byte()?);
        let delta_num = if delta_distance == 0 { -1 } else { sequence - delta_distance };
        let snap_flags = reader.read_byte()?;

        let old = if delta_num <= 0 {
            None
        } else {
            self.snapshots[(delta_num as usize) & (PACKET_BACKUP - 1)]
                .as_ref()
                .filter(|snapshot| snapshot.message_num == delta_num)
                .cloned()
        };
        let valid = delta_num <= 0 || old.is_some();

        let area_len = usize::from(reader.read_byte()?);
        if area_len > 32 {
            return Err(reader.error(ErrorKind::InvalidValue("snapshot areamask length")));
        }
        let mut area_mask = [0_u8; 32];
        for byte in area_mask.iter_mut().take(area_len) {
            *byte = reader.read_byte()?;
        }

        let player_state = read_delta_playerstate(reader, old.as_ref().map(|s| &s.player_state), false)?;
        let vehicle_player_state = if player_state.field_i32("m_iVehicleNum").unwrap_or(0) != 0 {
            Some(read_delta_playerstate(reader, old.as_ref().and_then(|s| s.vehicle_player_state.as_ref()), true)?)
        } else { None };
        let entities = self.read_packet_entities(reader, old.as_ref())?;

        let snapshot = Snapshot {
            server_time,
            message_num: sequence,
            delta_num,
            snap_flags,
            server_command_num: self.server_command_sequence,
            area_mask,
            player_state,
            vehicle_player_state,
            entities,
        };
        if !valid {
            return Ok(None);
        }
        self.snapshots[(sequence as usize) & (PACKET_BACKUP - 1)] = Some(snapshot.clone());
        Ok(Some(snapshot))
    }

    fn read_packet_entities(&self, reader: &mut MessageReader<'_>, old: Option<&Snapshot>) -> Result<Vec<EntityState>, Error> {
        let old_entities = old.map(|s| s.entities.as_slice()).unwrap_or(&[]);
        let mut result = Vec::new();
        let mut old_index = 0usize;
        loop {
            let newnum = reader.read_bits(10)?;
            if newnum == 1023 { break; }
            if !(0..1024).contains(&newnum) {
                return Err(reader.error(ErrorKind::InvalidValue("packet entity number")));
            }
            let newnum = newnum as u16;
            while old_index < old_entities.len() && old_entities[old_index].number < newnum {
                result.push(old_entities[old_index].clone());
                old_index += 1;
            }
            if old_index < old_entities.len() && old_entities[old_index].number == newnum {
                let from = &old_entities[old_index];
                old_index += 1;
                if let Some(entity) = read_delta_entity(reader, Some(from), newnum)? {
                    result.push(entity);
                }
            } else {
                let baseline = self.baselines.get(&newnum);
                if let Some(entity) = read_delta_entity(reader, baseline, newnum)? {
                    result.push(entity);
                }
            }
        }
        result.extend_from_slice(&old_entities[old_index..]);
        Ok(result)
    }
}

fn read_delta_playerstate(reader: &mut MessageReader<'_>, from: Option<&PlayerState>, is_vehicle: bool) -> Result<PlayerState, Error> {
    // OpenJK _OPTIMIZED_VEHICLE_NETWORKING: the extra pilot bit exists only
    // for the ordinary playerState. A separate vehicle playerState goes straight
    // to vehPlayerStateFields without consuming that selector.
    let schema = if is_vehicle {
        VEHICLE_PLAYER_FIELDS
    } else if reader.read_bits(1)? != 0 {
        PILOT_PLAYER_FIELDS
    } else {
        PLAYER_FIELDS
    };
    let mut to = from.cloned().unwrap_or_default();
    let count = usize::from(reader.read_byte()?);
    if count > schema.len() {
        return Err(reader.error(ErrorKind::InvalidValue("playerstate field count")));
    }
    for &(name, bits) in schema.iter().take(count) {
        if reader.read_bits(1)? == 0 { continue; }
        let value = if bits == 0 {
            if reader.read_bits(1)? == 0 {
                ((reader.read_bits(13)? - 4096) as f32).to_bits()
            } else {
                reader.read_bits(32)? as u32
            }
        } else {
            reader.read_bits(bits)? as u32
        };
        if !to.set_field_bits(name, value) {
            return Err(reader.error(ErrorKind::InvalidValue("unknown playerstate field")));
        }
    }
    if reader.read_bits(1)? != 0 {
        if reader.read_bits(1)? != 0 {
            let mask = reader.read_bits(MAX_STATS as i8)? as u32;
            for i in 0..MAX_STATS {
                if mask & (1u32 << i) != 0 {
                    to.stats[i] = if i == STAT_WEAPONS { reader.read_bits(MAX_WEAPONS)? } else { i32::from(reader.read_short()?) };
                }
            }
        }
        if reader.read_bits(1)? != 0 {
            let mask = reader.read_bits(MAX_PERSISTANT as i8)? as u32;
            for i in 0..MAX_PERSISTANT { if mask & (1u32 << i) != 0 { to.persistant[i] = i32::from(reader.read_short()?); } }
        }
        if reader.read_bits(1)? != 0 {
            let mask = reader.read_bits(MAX_AMMO_TRANSMIT as i8)? as u32;
            for i in 0..MAX_AMMO_TRANSMIT { if mask & (1u32 << i) != 0 { to.ammo[i] = i32::from(reader.read_short()?); } }
        }
        if reader.read_bits(1)? != 0 {
            let mask = reader.read_bits(MAX_POWERUPS as i8)? as u32;
            for i in 0..MAX_POWERUPS { if mask & (1u32 << i) != 0 { to.powerups[i] = reader.read_long()?; } }
        }
    }
    Ok(to)
}

fn info_value<'a>(info: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let mut fields = info.split(|&b| b == b'\\');
    if info.first() == Some(&b'\\') { let _ = fields.next(); }
    loop {
        let candidate = fields.next()?;
        let value = fields.next()?;
        if candidate.eq_ignore_ascii_case(key) { return Some(value); }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    const GAMESTATE: &[u8] = include_bytes!("../tests/fixtures/gamestate.bin");

    #[test]
    fn shared_server_decoder_accepts_reference_gamestate_packet() {
        let mut decoder = Decoder::new();
        let packet = decoder.parse_packet(77, GAMESTATE).unwrap();
        assert_eq!(packet.reliable_acknowledge, 123);
        assert_eq!(decoder.client_number, 2);
        assert_eq!(decoder.checksum_feed, 0x89abcdef);
        assert_eq!(decoder.map_name().as_deref(), Some("mp/ffa3"));
        assert!(packet.events.iter().any(|event| matches!(event, Event::Gamestate { .. })));
    }

    #[test]
    fn info_string_lookup_is_case_insensitive() {
        assert_eq!(info_value(b"\\foo\\x\\MapName\\mp/duel1", b"mapname"), Some(b"mp/duel1".as_slice()));
    }

    #[test]
    fn optimized_pilot_playerstate_uses_pilot_schema_order() {
        use crate::message::MessageWriter;
        let target = PILOT_PLAYER_FIELDS
            .iter()
            .position(|(name, _)| *name == "m_iVehicleNum")
            .unwrap();
        let mut writer = MessageWriter::new(4096).unwrap();
        writer.write_bits(1, 1).unwrap(); // pilot selector
        writer.write_bits((target + 1) as i32, 8).unwrap();
        for index in 0..=target {
            writer.write_bits(if index == target { 1 } else { 0 }, 1).unwrap();
            if index == target {
                writer.write_bits(37, 10).unwrap();
            }
        }
        writer.write_bits(0, 1).unwrap(); // no stats/persistant/ammo/powerups
        let bytes = writer.as_bytes().to_vec();
        let mut reader = MessageReader::new(&bytes).unwrap();
        let state = read_delta_playerstate(&mut reader, None, false).unwrap();
        assert_eq!(state.field_i32("m_iVehicleNum"), Some(37));
    }

    #[test]
    fn optimized_pilot_playerstate_keeps_pilot_only_move_dir() {
        use crate::message::MessageWriter;
        let target = PILOT_PLAYER_FIELDS
            .iter()
            .position(|(name, _)| *name == "moveDir[1]")
            .unwrap();
        let mut writer = MessageWriter::new(4096).unwrap();
        writer.write_bits(1, 1).unwrap(); // pilot selector
        writer.write_bits((target + 1) as i32, 8).unwrap();
        for index in 0..=target {
            writer.write_bits(if index == target { 1 } else { 0 }, 1).unwrap();
            if index == target {
                writer.write_bits(0, 1).unwrap(); // integral float
                writer.write_bits(4096 + 5, 13).unwrap();
            }
        }
        writer.write_bits(0, 1).unwrap(); // no stats/persistant/ammo/powerups
        let bytes = writer.as_bytes().to_vec();
        let mut reader = MessageReader::new(&bytes).unwrap();
        let state = read_delta_playerstate(&mut reader, None, false).unwrap();
        assert_eq!(state.field_f32("moveDir[1]"), Some(5.0));
    }

    #[test]
    fn optimized_vehicle_playerstate_keeps_vehicle_only_fields() {
        use crate::message::MessageWriter;
        let target = VEHICLE_PLAYER_FIELDS
            .iter()
            .position(|(name, _)| *name == "vehOrientation[0]")
            .unwrap();
        let mut writer = MessageWriter::new(4096).unwrap();
        writer.write_bits((target + 1) as i32, 8).unwrap();
        for index in 0..=target {
            writer.write_bits(if index == target { 1 } else { 0 }, 1).unwrap();
            if index == target {
                writer.write_bits(0, 1).unwrap(); // integral float
                writer.write_bits(4096 + 12, 13).unwrap();
            }
        }
        writer.write_bits(0, 1).unwrap(); // no stats/persistant/ammo/powerups
        let bytes = writer.as_bytes().to_vec();
        let mut reader = MessageReader::new(&bytes).unwrap();
        let state = read_delta_playerstate(&mut reader, None, true).unwrap();
        assert_eq!(state.field_f32("vehOrientation[0]"), Some(12.0));
    }
}
