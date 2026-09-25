//! Initial stock gamestate and entity-baseline decoding. Snapshot replay and
//! non-stock netfield overrides are deliberately outside this module's scope.

use std::collections::BTreeMap;

pub use crate::entity_fields::ENTITY_FIELDS;
use crate::message::{Error, ErrorKind, MessageReader};

pub const MAX_CONFIGSTRINGS: u16 = 1700;
pub const MAX_GAMESTATE_CHARS: usize = 16000;

/// Explicit wire-schema slots, never a cast of an engine/C structure.
/// Float fields retain their exact IEEE-754 bits, including negative zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityState {
    pub number: u16,
    pub fields: [u32; ENTITY_FIELDS.len()],
}

impl EntityState {
    pub fn field_bits(&self, name: &str) -> Option<u32> {
        ENTITY_FIELDS
            .iter()
            .position(|(candidate, _)| *candidate == name)
            .map(|index| self.fields[index])
    }

    pub fn field_i32(&self, name: &str) -> Option<i32> {
        self.field_bits(name).map(|bits| bits as i32)
    }

    pub fn field_f32(&self, name: &str) -> Option<f32> {
        self.field_bits(name).map(f32::from_bits)
    }
}

pub fn read_delta_entity(
    reader: &mut MessageReader<'_>,
    from: Option<&EntityState>,
    number: u16,
) -> Result<Option<EntityState>, Error> {
    if number >= 1024 {
        return Err(reader.error(ErrorKind::InvalidValue("entity number")));
    }
    if reader.read_bits(1)? != 0 {
        return Ok(None);
    }
    let mut entity = from.cloned().unwrap_or(EntityState {
        number,
        fields: [0; ENTITY_FIELDS.len()],
    });
    entity.number = number;
    if reader.read_bits(1)? == 0 {
        return Ok(Some(entity));
    }
    let count = usize::from(reader.read_byte()?);
    if count > ENTITY_FIELDS.len() {
        return Err(reader.error(ErrorKind::InvalidValue("entity field count")));
    }
    for (index, &(_, bits)) in ENTITY_FIELDS.iter().take(count).enumerate() {
        if reader.read_bits(1)? == 0 {
            continue;
        }
        entity.fields[index] = if reader.read_bits(1)? == 0 {
            0
        } else if bits != 0 {
            reader.read_bits(bits)? as u32
        } else if reader.read_bits(1)? == 0 {
            ((reader.read_bits(13)? - 4096) as f32).to_bits()
        } else {
            reader.read_bits(32)? as u32
        };
    }
    Ok(Some(entity))
}

#[derive(Debug)]
pub struct Gamestate {
    pub reliable_acknowledge: i32,
    pub preceding_commands: Vec<ServerCommand>,
    pub server_command_sequence: i32,
    pub configstrings: BTreeMap<u16, Vec<u8>>,
    pub baselines: BTreeMap<u16, EntityState>,
    pub client_number: i32,
    pub checksum_feed: u32,
    pub consumed_bits: usize,
    pub consumed_bytes: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ServerCommand {
    pub sequence: i32,
    pub text: Vec<u8>,
}

/// Expects an initial message with optional nops/server commands before the
/// gamestate, followed by outer EOF. Commands are retained, never executed.
/// No state is published until the complete message is successfully validated.
pub fn decode_initial_gamestate(payload: &[u8]) -> Result<Gamestate, Error> {
    let mut reader = MessageReader::new(payload)?;
    let reliable_acknowledge = reader.read_long()?;
    let mut preceding_commands = Vec::new();
    loop {
        match reader.read_byte()? {
            2 => break,
            1 => continue,
            5 => {
                if preceding_commands.len() >= 128 {
                    return Err(reader.error(ErrorKind::Limit("preceding server commands")));
                }
                preceding_commands.push(ServerCommand {
                    sequence: reader.read_long()?,
                    text: reader.read_string(1024)?,
                });
            }
            _ => {
                return Err(reader.error(ErrorKind::Unsupported(
                    "initial message without svc_gamestate",
                )))
            }
        }
    }
    let server_command_sequence = reader.read_long()?;
    let mut configstrings = BTreeMap::new();
    let mut baselines = BTreeMap::new();
    let mut string_bytes = 1usize;
    loop {
        match reader.read_byte()? {
            10 => break,
            3 => {
                let index = reader.read_short()? as u16;
                if index >= MAX_CONFIGSTRINGS {
                    return Err(reader.error(ErrorKind::InvalidValue("configstring index")));
                }
                let text = reader.read_string(8192)?;
                string_bytes += text.len() + 1;
                if string_bytes > MAX_GAMESTATE_CHARS {
                    return Err(reader.error(ErrorKind::Limit("gamestate strings")));
                }
                configstrings.insert(index, text);
            }
            4 => {
                let number = reader.read_bits(10)? as u16;
                let entity = read_delta_entity(&mut reader, None, number)?.ok_or_else(|| {
                    reader.error(ErrorKind::InvalidValue("removed gamestate baseline"))
                })?;
                baselines.insert(number, entity);
            }
            _ => return Err(reader.error(ErrorKind::InvalidValue("gamestate service opcode"))),
        }
    }
    let client_number = reader.read_long()?;
    let checksum_feed = reader.read_long()? as u32;
    // Stock OpenJK unconditionally reads and discards this old RMG short.
    let _legacy_rmg = reader.read_short()?;
    if reader.read_byte()? != 10 {
        return Err(reader.error(ErrorKind::Unsupported("messages after initial gamestate")));
    }
    Ok(Gamestate {
        reliable_acknowledge,
        preceding_commands,
        server_command_sequence,
        configstrings,
        baselines,
        client_number,
        checksum_feed,
        consumed_bits: reader.bit_position(),
        consumed_bytes: reader.legacy_read_count(),
    })
}
