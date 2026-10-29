//! Protocol 26 snapshot encoder for offline demo synthesis.
//!
//! The bitstream mirrors OpenJK MSG_WriteDeltaPlayerstate, MSG_WriteDeltaEntity
//! and SV_EmitPacketEntities. No rendering state or engine structs are serialized.
use std::collections::BTreeMap;
use crate::{
    demo::MAX_MESSAGE_BYTES,
    gamestate::{EntityState, ENTITY_FIELDS},
    message::{Error, ErrorKind, MessageWriter},
    player_fields::{PLAYER_FIELDS, PILOT_PLAYER_FIELDS, VEHICLE_PLAYER_FIELDS},
    server::{PlayerState, Snapshot, MAX_STATS, MAX_PERSISTANT, MAX_AMMO_TRANSMIT,
        MAX_POWERUPS, MAX_WEAPONS, STAT_WEAPONS, SVC_SNAPSHOT, SVC_SERVER_COMMAND, SVC_EOF},
};

fn float_field(writer: &mut MessageWriter, value: u32) -> Result<(), Error> {
    let f = f32::from_bits(value);
    let i = f as i32;
    if f.is_finite() && f == i as f32 && (-4096..4096).contains(&i) {
        writer.write_bits(0, 1)?;
        writer.write_bits(i + 4096, 13)
    } else {
        writer.write_bits(1, 1)?;
        writer.write_bits(value as i32, 32)
    }
}

const EF_NODRAW: i32 = 1 << 8;

/// OpenJK `_OPTIMIZED_VEHICLE_NETWORKING`: only a pilot riding *inside* a
/// vehicle (m_iVehicleNum && EF_NODRAW) uses the pilot schema.
fn is_pilot(ps: &PlayerState) -> bool {
    ps.field_i32("m_iVehicleNum").unwrap_or(0) != 0 && ps.field_i32("eFlags").unwrap_or(0) & EF_NODRAW != 0
}

fn player_schema(ps: &PlayerState, vehicle: bool) -> &'static [(&'static str, i8)] {
    if vehicle { VEHICLE_PLAYER_FIELDS } else if is_pilot(ps) { PILOT_PLAYER_FIELDS } else { PLAYER_FIELDS }
}

/// The value a stock reader reconstructs from one written netfield: integers
/// are masked to their width (negative widths sign-extend, as MSG_ReadBits),
/// and a float zero of either sign arrives as +0.0.
fn wire_value(value: u32, bits: i8) -> u32 {
    match bits {
        0 => if f32::from_bits(value) == 0.0 { 0 } else { value },
        32 | -32 => value,
        bits => {
            let width = u32::from(bits.unsigned_abs());
            let masked = value & ((1u32 << width) - 1);
            let sign_width = width - width % 8;
            if bits < 0 && sign_width > 0 && masked & (1 << (sign_width - 1)) != 0 {
                masked | (u32::MAX << sign_width)
            } else { masked }
        }
    }
}

fn canonicalize_player_state(ps: &mut PlayerState, vehicle: bool) {
    for &(name, bits) in player_schema(ps, vehicle) {
        if let Some(value) = ps.field_bits(name) { ps.set_field_bits(name, wire_value(value, bits)); }
    }
    for (i, value) in ps.stats.iter_mut().enumerate() {
        *value = if i == STAT_WEAPONS { *value & ((1 << MAX_WEAPONS) - 1) } else { i32::from(*value as i16) };
    }
    for value in ps.persistant.iter_mut().chain(ps.ammo.iter_mut()) { *value = i32::from(*value as i16); }
}

/// Normalize a snapshot to exactly what a stock client decodes from it, so
/// the writer's delta source and the client's stay identical. A vehicle
/// playerState only exists while `m_iVehicleNum` is set (CL_ParseSnapshot).
pub fn canonicalize_snapshot(snap: &mut Snapshot) {
    canonicalize_player_state(&mut snap.player_state, false);
    if snap.player_state.field_i32("m_iVehicleNum").unwrap_or(0) == 0 {
        snap.vehicle_player_state = None;
    } else if let Some(vehicle) = snap.vehicle_player_state.as_mut() {
        canonicalize_player_state(vehicle, true);
    }
    for entity in &mut snap.entities {
        for (value, &(_, bits)) in entity.fields.iter_mut().zip(ENTITY_FIELDS) {
            *value = wire_value(*value, bits);
        }
    }
}

fn write_delta_playerstate(
    writer: &mut MessageWriter, from: Option<&PlayerState>, to: &PlayerState, vehicle: bool,
) -> Result<(), Error> {
    if !vehicle { writer.write_bits(i32::from(is_pilot(to)), 1)?; }
    let schema = player_schema(to, vehicle);
    let last = schema.iter().rposition(|(name, _)|
        from.and_then(|p|p.field_bits(name)).unwrap_or(0) != to.field_bits(name).unwrap_or(0)
    ).map_or(0, |index|index+1);
    writer.write_bits(last as i32, 8)?;
    for &(name, bits) in schema.iter().take(last) {
        let value = to.field_bits(name).unwrap_or(0);
        let changed = value != from.and_then(|p|p.field_bits(name)).unwrap_or(0);
        writer.write_bits(i32::from(changed), 1)?;
        if changed {
            if bits == 0 { float_field(writer, value)?; }
            else { writer.write_bits(value as i32, bits)?; }
        }
    }
    let zero = PlayerState::default();
    let from = from.unwrap_or(&zero);
    let stats = mask(&from.stats, &to.stats);
    let persistant = mask(&from.persistant, &to.persistant);
    let ammo = mask(&from.ammo, &to.ammo);
    let powerups = mask(&from.powerups, &to.powerups);
    let changed = stats != 0 || persistant != 0 || ammo != 0 || powerups != 0;
    writer.write_bits(i32::from(changed), 1)?;
    if changed {
        writer.write_bits(i32::from(stats!=0), 1)?;
        if stats!=0 {
            writer.write_bits(stats as i32, MAX_STATS as i8)?;
            for (i, value) in to.stats.iter().enumerate() {
                if stats&(1<<i)!=0 {
                    writer.write_bits(*value, if i==STAT_WEAPONS {MAX_WEAPONS} else {16})?;
                }
            }
        }
        writer.write_bits(i32::from(persistant!=0), 1)?;
        if persistant!=0 {
            writer.write_bits(persistant as i32, MAX_PERSISTANT as i8)?;
            for (i, value) in to.persistant.iter().enumerate() {
                if persistant&(1<<i)!=0 {writer.write_bits(*value,16)?;}
            }
        }
        writer.write_bits(i32::from(ammo!=0), 1)?;
        if ammo!=0 {
            writer.write_bits(ammo as i32,MAX_AMMO_TRANSMIT as i8)?;
            for (i,value) in to.ammo.iter().enumerate() {
                if ammo&(1<<i)!=0 {writer.write_bits(*value,16)?;}
            }
        }
        writer.write_bits(i32::from(powerups!=0),1)?;
        if powerups!=0 {
            writer.write_bits(powerups as i32,MAX_POWERUPS as i8)?;
            for (i,value) in to.powerups.iter().enumerate() {
                if powerups&(1<<i)!=0 {writer.write_bits(*value,32)?;}
            }
        }
    }
    Ok(())
}

fn mask<const N: usize>(a: &[i32;N], b: &[i32;N]) -> u32 {
    a.iter().zip(b).enumerate().fold(0, |mask,(i,(x,y))|mask | (u32::from(x!=y)<<i))
}

/// `from=None` is the nulled entity state (or the gamestate baseline).
/// `force` sends a no-change marker even when every field equals `from`.
fn write_delta_entity(writer: &mut MessageWriter, from: Option<&EntityState>, to: Option<&EntityState>, force: bool)
    -> Result<(), Error> {
    let number = to.or(from).ok_or(Error{bit:writer.bit_position(),kind:ErrorKind::InvalidValue("entity delta endpoints")})?.number;
    let changed = to.is_some_and(|entity| ENTITY_FIELDS.iter().enumerate().any(|(i,_)|
        from.map_or(0,|old|old.fields[i]) != entity.fields[i]));
    if !changed && !force {return Ok(());}
    writer.write_bits(i32::from(number),10)?;
    let Some(to) = to else {return writer.write_bits(1,1);};
    writer.write_bits(0,1)?; // not removed
    writer.write_bits(i32::from(changed),1)?;
    if !changed {return Ok(());}
    let last = (0..ENTITY_FIELDS.len()).rfind(|&i|from.map_or(0,|old|old.fields[i])!=to.fields[i]).unwrap()+1;
    writer.write_bits(last as i32,8)?;
    for (i,&(_,bits)) in ENTITY_FIELDS.iter().take(last).enumerate() {
        let value=to.fields[i];
        let updated=from.map_or(0,|old|old.fields[i])!=value;
        writer.write_bits(i32::from(updated),1)?;
        if !updated {continue;}
        writer.write_bits(i32::from(value!=0),1)?;
        if value==0 {continue;}
        if bits==0 {float_field(writer,value)?;}
        else {writer.write_bits(value as i32,bits)?;}
    }
    Ok(())
}

fn packet_entities(writer: &mut MessageWriter, previous: Option<&Snapshot>, current: &Snapshot,
    baselines: &BTreeMap<u16,EntityState>) -> Result<(), Error> {
    let old=previous.map_or(&[][..],|s|s.entities.as_slice());
    let new=current.entities.as_slice();
    let (mut a,mut b)=(0,0);
    while a<old.len() || b<new.len() {
        let oldnum=old.get(a).map_or(1023,|e|e.number);
        let newnum=new.get(b).map_or(1023,|e|e.number);
        if oldnum==newnum && oldnum<1023 {
            write_delta_entity(writer,Some(&old[a]),Some(&new[b]),false)?;
            a+=1;b+=1;
        }else if newnum<oldnum {
            write_delta_entity(writer,baselines.get(&newnum),Some(&new[b]),true)?;
            b+=1;
        }else {
            write_delta_entity(writer,Some(&old[a]),None,true)?;
            a+=1;
        }
    }
    writer.write_bits(1023,10)
}

fn write_command(writer: &mut MessageWriter, sequence: i32, text: &[u8]) -> Result<(), Error> {
    if text.len()>=1024 || text.contains(&0) {
        return Err(Error{bit:writer.bit_position(),kind:ErrorKind::Limit("server command length")});
    }
    writer.write_bits(i32::from(SVC_SERVER_COMMAND),8)?;
    writer.write_bits(sequence,32)?;
    for &b in text {writer.write_bits(i32::from(b),8)?;}
    writer.write_bits(0,8)
}

/// Write a normal server->client snapshot packet, optionally preceded by
/// reliable commands. `previous` MUST be the snapshot at sequence-1, otherwise
/// no delta reference is used. Every frame can be independently decoded from
/// the most recent full snapshot and its preceding deltas.
///
/// `commands` must be monotonically numbered; the final counter is stored in
/// the decoder, not encoded in a snapshot directly.
pub fn synthesize_snapshot_payload(
    sequence: i32, reliable_ack: i32, previous: Option<&Snapshot>,
    current: &Snapshot, baselines: &BTreeMap<u16,EntityState>, commands: &[(i32,Vec<u8>)],
) -> Result<Vec<u8>, Error> {
    if current.entities.windows(2).any(|pair|pair[0].number>=pair[1].number)
        || current.entities.iter().any(|e|e.number>=1023) {
        return Err(Error{bit:0,kind:ErrorKind::InvalidValue("sorted unique snapshot entity numbers")});
    }
    let old=previous.filter(|old|old.message_num==sequence-1);
    let mut writer=MessageWriter::new(MAX_MESSAGE_BYTES)?;
    writer.write_bits(reliable_ack,32)?;
    for (number,text) in commands {write_command(&mut writer,*number,text)?;}
    writer.write_bits(i32::from(SVC_SNAPSHOT),8)?;
    writer.write_bits(current.server_time,32)?;
    writer.write_bits(if old.is_some(){1}else{0},8)?;
    writer.write_bits(i32::from(current.snap_flags),8)?;
    let mask_len=current.area_mask.iter().rposition(|&byte|byte!=0).map_or(0,|i|i+1);
    writer.write_bits(mask_len as i32,8)?;
    for &byte in current.area_mask.iter().take(mask_len) {writer.write_bits(i32::from(byte),8)?;}
    write_delta_playerstate(&mut writer,old.map(|s|&s.player_state),&current.player_state,false)?;
    if current.player_state.field_i32("m_iVehicleNum").unwrap_or(0)!=0 {
        let vehicle=current.vehicle_player_state.as_ref().ok_or(Error{
            bit:writer.bit_position(),kind:ErrorKind::InvalidValue("missing vehicle playerstate")})?;
        write_delta_playerstate(&mut writer,old.and_then(|s|s.vehicle_player_state.as_ref()),vehicle,true)?;
    }
    packet_entities(&mut writer,old,current,baselines)?;
    writer.write_bits(i32::from(SVC_EOF),8)?;
    Ok(writer.as_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::Decoder;

    #[test]
    fn snapshot_delta_and_removals_round_trip() {
        let mut decoder=Decoder::new();
        let mut a=Snapshot{server_time:1000,message_num:2,delta_num:-1,snap_flags:0,ping:0,
            server_command_num:0,area_mask:[0;32],player_state:PlayerState::default(),
            vehicle_player_state:None,entities:vec![]};
        a.player_state.set_field_bits("origin[0]",20.25f32.to_bits());
        a.player_state.set_field_bits("viewangles[1]",90f32.to_bits());
        a.player_state.stats[STAT_WEAPONS]=12345;
        a.entities.push(EntityState{number:7,fields:[0;ENTITY_FIELDS.len()]});
        a.entities[0].fields[0]=23;
        let initial=synthesize_snapshot_payload(2,0,None,&a,&BTreeMap::new(),&[]).unwrap();
        decoder.parse_packet(2,&initial).unwrap();
        assert_eq!(decoder.latest_snapshot().unwrap().player_state,a.player_state);
        assert_eq!(decoder.latest_snapshot().unwrap().entities,a.entities);
        let mut b=a.clone();b.message_num=3;b.server_time=1016;
        b.player_state.set_field_bits("origin[0]",32.75f32.to_bits());
        b.entities.clear();
        b.entities.push(EntityState{number:8,fields:[0;ENTITY_FIELDS.len()]});
        let payload=synthesize_snapshot_payload(3,0,Some(&a),&b,&BTreeMap::new(),&[]).unwrap();
        decoder.parse_packet(3,&payload).unwrap();
        assert_eq!(decoder.latest_snapshot().unwrap().player_state,b.player_state);
        assert_eq!(decoder.latest_snapshot().unwrap().entities,b.entities);
    }
    #[test]
    fn canonical_values_match_the_stock_reader() {
        let mut decoder=Decoder::new();
        let mut snap=Snapshot{server_time:100,message_num:2,delta_num:-1,snap_flags:0,ping:0,
            server_command_num:0,area_mask:[0;32],player_state:PlayerState::default(),
            vehicle_player_state:Some(PlayerState::default()),entities:vec![]};
        snap.player_state.set_field_bits("velocity[0]",(-0.0f32).to_bits());
        snap.player_state.set_field_bits("weaponTime",(-5i32) as u32);
        snap.player_state.set_field_bits("legsAnim",0x1_2345);
        snap.player_state.stats[2]=-3;
        snap.player_state.persistant[0]=70_000;
        let mut entity=EntityState{number:9,fields:[0;ENTITY_FIELDS.len()]};
        let save=ENTITY_FIELDS.iter().position(|(n,_)|*n=="saberMove").unwrap();
        entity.fields[save]=0x1ff;
        snap.entities.push(entity);
        canonicalize_snapshot(&mut snap);
        assert!(snap.vehicle_player_state.is_none());
        let payload=synthesize_snapshot_payload(2,0,None,&snap,&BTreeMap::new(),&[]).unwrap();
        decoder.parse_packet(2,&payload).unwrap();
        let decoded=decoder.latest_snapshot().unwrap();
        assert_eq!(decoded.player_state,snap.player_state);
        assert_eq!(decoded.entities,snap.entities);
    }
    #[test]
    fn pilot_schema_requires_nodraw() {
        let mut ps=PlayerState::default();
        ps.set_field_bits("m_iVehicleNum",40);
        assert!(!is_pilot(&ps));
        ps.set_field_bits("eFlags",EF_NODRAW as u32);
        assert!(is_pilot(&ps));
    }
    #[test]
    fn all_player_array_groups_round_trip() {
        let mut decoder=Decoder::new();
        let mut snap=Snapshot{server_time:100,message_num:2,delta_num:-1,snap_flags:0,ping:0,
            server_command_num:0,area_mask:[0;32],player_state:PlayerState::default(),
            vehicle_player_state:None,entities:vec![]};
        snap.player_state.persistant[3]=120;
        snap.player_state.ammo[4]=5;
        snap.player_state.powerups[5]=991000;
        snap.player_state.stats[7]=42;
        let payload=synthesize_snapshot_payload(2,1,None,&snap,&BTreeMap::new(),&[]).unwrap();
        decoder.parse_packet(2,&payload).unwrap();
        assert_eq!(decoder.latest_snapshot().unwrap().player_state,snap.player_state);
    }
}
