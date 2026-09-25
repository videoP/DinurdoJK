//! Client side of the JKA network channel.
//!
//! References (OpenJK codemp): `qcommon/net_chan.cpp` (Netchan_Transmit,
//! Netchan_TransmitNextFragment, Netchan_Process), `client/cl_net_chan.cpp`
//! (CL_Netchan_Encode/Decode), `qcommon/msg.cpp` (MSG_WriteDeltaUsercmdKey),
//! `client/cl_input.cpp` (CL_WritePacket) and `client/cl_main.cpp`
//! (CL_CheckForResend, CL_ConnectionlessPacket).
//!
//! Headers are raw little-endian ("OOB" mode); everything after them is the
//! trained Huffman bitstream from `message.rs`, XOR-scrambled after a fixed
//! prefix. Nothing here owns a socket or a clock.

use crate::{
    adaptive_huffman,
    commands::tokenize,
    message::{Error, MessageReader, MessageWriter},
};

pub const MAX_PACKETLEN: usize = 1400;
pub const FRAGMENT_SIZE: usize = MAX_PACKETLEN - 100;
pub const FRAGMENT_BIT: u32 = 1 << 31;
pub const MAX_MSGLEN: usize = 49152;
pub const MAX_RELIABLE_COMMANDS: usize = 128;
pub const MAX_PACKET_USERCMDS: usize = 32;
/// Stock MAX_STRING_CHARS: MSG_WriteString refuses strings this long or longer.
pub const MAX_STRING_CHARS: usize = 1024;

pub const CLC_NOP: u8 = 1;
pub const CLC_MOVE: u8 = 2;
pub const CLC_MOVE_NO_DELTA: u8 = 3;
pub const CLC_CLIENT_COMMAND: u8 = 4;
pub const CLC_EOF: u8 = 5;

const CL_ENCODE_START: usize = 12;
const CL_DECODE_START: usize = 4;
const OOB_PREFIX: &[u8] = b"\xff\xff\xff\xff";

/// One netchan endpoint as seen from the client (`NS_CLIENT`): outgoing
/// packets carry the qport, incoming ones do not.
#[derive(Debug, Clone)]
pub struct Netchan {
    pub qport: u16,
    pub incoming_sequence: i32,
    pub outgoing_sequence: i32,
    /// Packets skipped by the most recent accepted sequence.
    pub dropped: i32,
    fragment_sequence: i32,
    fragment_buffer: Vec<u8>,
}

/// A complete, still-scrambled server message: `sequence` plus everything
/// after the netchan header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingMessage {
    pub sequence: i32,
    pub payload: Vec<u8>,
}

impl Netchan {
    /// Netchan_Setup.
    pub fn new(qport: u16) -> Self {
        Self {
            qport,
            incoming_sequence: 0,
            outgoing_sequence: 1,
            dropped: 0,
            fragment_sequence: 0,
            fragment_buffer: Vec::new(),
        }
    }

    /// Netchan_Transmit followed by CL_WritePacket's "fire all fragments at
    /// once" loop. Returns the datagrams in send order.
    pub fn transmit(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        assert!(data.len() <= MAX_MSGLEN, "Netchan_Transmit: length = {}", data.len());
        if data.len() < FRAGMENT_SIZE {
            let mut packet = Vec::with_capacity(6 + data.len());
            packet.extend_from_slice(&self.outgoing_sequence.to_le_bytes());
            packet.extend_from_slice(&self.qport.to_le_bytes());
            packet.extend_from_slice(data);
            self.outgoing_sequence += 1;
            return vec![packet];
        }
        // Netchan_TransmitNextFragment: a message that is an exact multiple of
        // FRAGMENT_SIZE ends with an empty fragment so the receiver knows.
        let mut packets = Vec::new();
        let mut start = 0usize;
        loop {
            let length = FRAGMENT_SIZE.min(data.len() - start);
            let mut packet = Vec::with_capacity(10 + length);
            packet.extend_from_slice(&((self.outgoing_sequence as u32) | FRAGMENT_BIT).to_le_bytes());
            packet.extend_from_slice(&self.qport.to_le_bytes());
            packet.extend_from_slice(&(start as u16).to_le_bytes());
            packet.extend_from_slice(&(length as u16).to_le_bytes());
            packet.extend_from_slice(&data[start..start + length]);
            packets.push(packet);
            start += length;
            if start == data.len() && length != FRAGMENT_SIZE {
                self.outgoing_sequence += 1;
                return packets;
            }
        }
    }

    /// Netchan_Process. `None` means out of order, duplicated, malformed, or a
    /// non-final fragment.
    pub fn process(&mut self, packet: &[u8]) -> Option<IncomingMessage> {
        let raw = u32::from_le_bytes(packet.get(..4)?.try_into().ok()?);
        let fragmented = raw & FRAGMENT_BIT != 0;
        let sequence = (raw & !FRAGMENT_BIT) as i32;
        let mut cursor = 4usize;
        let (fragment_start, fragment_length) = if fragmented {
            let start = u16::from_le_bytes(packet.get(4..6)?.try_into().ok()?);
            let length = u16::from_le_bytes(packet.get(6..8)?.try_into().ok()?);
            cursor = 8;
            (usize::from(start), usize::from(length))
        } else {
            (0, 0)
        };

        if sequence <= self.incoming_sequence {
            return None;
        }
        self.dropped = sequence - (self.incoming_sequence + 1);

        if fragmented {
            if sequence != self.fragment_sequence {
                self.fragment_sequence = sequence;
                self.fragment_buffer.clear();
            }
            // A missed fragment dumps this packet; OpenJK deliberately keeps
            // what it has so far rather than resetting fragmentLength.
            if fragment_start != self.fragment_buffer.len() {
                return None;
            }
            if cursor + fragment_length > packet.len()
                || self.fragment_buffer.len() + fragment_length > MAX_MSGLEN
            {
                return None;
            }
            self.fragment_buffer
                .extend_from_slice(&packet[cursor..cursor + fragment_length]);
            if fragment_length == FRAGMENT_SIZE {
                return None;
            }
            if self.fragment_buffer.len() + 4 > MAX_MSGLEN {
                return None;
            }
            // As in the reference, a completed fragmented message does not
            // advance incomingSequence ("but I am a wuss -mw").
            let payload = std::mem::take(&mut self.fragment_buffer);
            return Some(IncomingMessage { sequence, payload });
        }

        self.incoming_sequence = sequence;
        Some(IncomingMessage {
            sequence,
            payload: packet[cursor..].to_vec(),
        })
    }
}

/// The byte-sized rolling XOR shared by CL/SV_Netchan_Encode/Decode.
/// `absolute` is the byte index used for `(i & 1)` in the reference loop.
fn scramble(data: &mut [u8], start: usize, absolute_bias: usize, mut key: u8, string: &[u8]) {
    let string = &string[..string.iter().position(|&b| b == 0).unwrap_or(string.len())];
    let mut index = 0usize;
    for i in start..data.len() {
        if index >= string.len() {
            index = 0;
        }
        let ch = string.get(index).copied().unwrap_or(0);
        let shift = (i + absolute_bias) & 1;
        let value = if ch == b'%' { b'.' } else { ch };
        key ^= ((u32::from(value)) << shift) as u8;
        index += 1;
        data[i] ^= key;
    }
}

/// CL_Netchan_Decode. `payload` is the message after the netchan header,
/// which always begins 4 bytes into the reference `msg->data`. The key string
/// is the client's own reliable command numbered by the message's
/// reliableAcknowledge.
pub fn decode_server_message<'a>(
    payload: &mut [u8],
    sequence: i32,
    challenge: i32,
    reliable_command: impl FnOnce(i32) -> &'a [u8],
) -> Result<(), Error> {
    let reliable_acknowledge = MessageReader::new(payload)?.read_long()?;
    let key = (challenge ^ sequence) as u8;
    let string = reliable_command(reliable_acknowledge);
    scramble(payload, CL_DECODE_START, 4, key, string);
    Ok(())
}

/// CL_Netchan_Encode on a complete client message (after clc_EOF). The key
/// string is the last server command this message acknowledges.
pub fn encode_client_message<'a>(
    data: &mut [u8],
    challenge: i32,
    server_command: impl FnOnce(i32) -> &'a [u8],
) -> Result<(), Error> {
    if data.len() <= CL_ENCODE_START {
        return Ok(());
    }
    let mut reader = MessageReader::new(data)?;
    let server_id = reader.read_long()?;
    let message_acknowledge = reader.read_long()?;
    let reliable_acknowledge = reader.read_long()?;
    let key = (challenge ^ server_id ^ message_acknowledge) as u8;
    let string = server_command(reliable_acknowledge);
    scramble(data, CL_ENCODE_START, 0, key, string);
    Ok(())
}

/// Wire usercmd_t. Angles are 16-bit ANGLE2SHORT values stored in ints.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UserCmd {
    pub server_time: i32,
    pub angles: [i32; 3],
    pub buttons: i32,
    pub weapon: u8,
    pub force_selection: u8,
    pub inventory_selection: u8,
    pub generic_command: u8,
    pub forward_move: i8,
    pub right_move: i8,
    pub up_move: i8,
}

impl UserCmd {
    fn unchanged_from(&self, from: &Self) -> bool {
        from.angles == self.angles
            && from.forward_move == self.forward_move
            && from.right_move == self.right_move
            && from.up_move == self.up_move
            && from.buttons == self.buttons
            && from.weapon == self.weapon
            && from.force_selection == self.force_selection
            && from.inventory_selection == self.inventory_selection
            && from.generic_command == self.generic_command
    }
}

fn write_delta_key(w: &mut MessageWriter, key: i32, old: i32, new: i32, bits: i8) -> Result<(), Error> {
    if old == new {
        return w.write_bits(0, 1);
    }
    w.write_bits(1, 1)?;
    w.write_bits((new ^ key) & ((1i64 << bits) - 1) as i32, bits)
}

/// MSG_WriteDeltaUsercmdKey.
pub fn write_delta_usercmd_key(w: &mut MessageWriter, key: i32, from: &UserCmd, to: &UserCmd) -> Result<(), Error> {
    let delta_time = to.server_time.wrapping_sub(from.server_time);
    if delta_time < 256 {
        w.write_bits(1, 1)?;
        w.write_bits(delta_time, 8)?;
    } else {
        w.write_bits(0, 1)?;
        w.write_bits(to.server_time, 32)?;
    }
    if to.unchanged_from(from) {
        return w.write_bits(0, 1);
    }
    let key = key ^ to.server_time;
    w.write_bits(1, 1)?;
    for axis in 0..3 {
        write_delta_key(w, key, from.angles[axis], to.angles[axis], 16)?;
    }
    // signed char fields promote with sign extension before the XOR/mask.
    write_delta_key(w, key, from.forward_move.into(), to.forward_move.into(), 8)?;
    write_delta_key(w, key, from.right_move.into(), to.right_move.into(), 8)?;
    write_delta_key(w, key, from.up_move.into(), to.up_move.into(), 8)?;
    write_delta_key(w, key, from.buttons, to.buttons, 16)?;
    write_delta_key(w, key, from.weapon.into(), to.weapon.into(), 8)?;
    write_delta_key(w, key, from.force_selection.into(), to.force_selection.into(), 8)?;
    write_delta_key(w, key, from.inventory_selection.into(), to.inventory_selection.into(), 8)?;
    write_delta_key(w, key, from.generic_command.into(), to.generic_command.into(), 8)
}

fn read_delta_key(r: &mut MessageReader<'_>, key: i32, old: i32, bits: i8) -> Result<i32, Error> {
    if r.read_bits(1)? != 0 {
        Ok((r.read_bits(bits)? ^ key) & ((1i64 << bits) - 1) as i32)
    } else {
        Ok(old)
    }
}

/// MSG_ReadDeltaUsercmdKey (server side); used to verify outgoing packets.
pub fn read_delta_usercmd_key(r: &mut MessageReader<'_>, key: i32, from: &UserCmd) -> Result<UserCmd, Error> {
    let mut to = *from;
    to.server_time = if r.read_bits(1)? != 0 {
        from.server_time.wrapping_add(r.read_bits(8)?)
    } else {
        r.read_bits(32)?
    };
    if r.read_bits(1)? == 0 {
        return Ok(to);
    }
    let key = key ^ to.server_time;
    for axis in 0..3 {
        to.angles[axis] = read_delta_key(r, key, from.angles[axis], 16)?;
    }
    let signed = |value: i32| -> i8 { match value as u8 as i8 { -128 => -127, v => v } };
    to.forward_move = signed(read_delta_key(r, key, from.forward_move.into(), 8)?);
    to.right_move = signed(read_delta_key(r, key, from.right_move.into(), 8)?);
    to.up_move = signed(read_delta_key(r, key, from.up_move.into(), 8)?);
    to.buttons = read_delta_key(r, key, from.buttons, 16)?;
    to.weapon = read_delta_key(r, key, from.weapon.into(), 8)? as u8;
    to.force_selection = read_delta_key(r, key, from.force_selection.into(), 8)? as u8;
    to.inventory_selection = read_delta_key(r, key, from.inventory_selection.into(), 8)? as u8;
    to.generic_command = read_delta_key(r, key, from.generic_command.into(), 8)? as u8;
    Ok(to)
}

/// Com_HashKey, including C's signed `char` promotion on x86/MSVC.
pub fn hash_key(string: &[u8], max_len: usize) -> i32 {
    let mut hash = 0i32;
    for (i, &byte) in string.iter().take(max_len).enumerate() {
        if byte == 0 {
            break;
        }
        hash = hash.wrapping_add(i32::from(byte as i8).wrapping_mul(119 + i as i32));
    }
    hash ^ (hash >> 10) ^ (hash >> 20)
}

/// MSG_WriteString: strings of MAX_STRING_CHARS or more are replaced by "".
pub fn write_string(w: &mut MessageWriter, text: &[u8]) -> Result<(), Error> {
    let text = &text[..text.iter().position(|&b| b == 0).unwrap_or(text.len())];
    if text.len() < MAX_STRING_CHARS {
        for &byte in text {
            w.write_bits(i32::from(byte), 8)?;
        }
    }
    w.write_bits(0, 8)
}

/// The usercmd block of CL_WritePacket.
pub struct ClientMoves<'a> {
    /// clc_move when the last received snapshot is valid and current.
    pub delta: bool,
    pub checksum_feed: i32,
    /// Text of the acknowledged server command, hashed into the key.
    pub acknowledged_server_command: &'a [u8],
    pub commands: &'a [UserCmd],
}

pub struct ClientPacket<'a> {
    pub server_id: i32,
    /// clc.serverMessageSequence.
    pub message_acknowledge: i32,
    /// clc.serverCommandSequence.
    pub command_acknowledge: i32,
    /// Unacknowledged reliable commands with their sequence numbers.
    pub reliable_commands: &'a [(i32, &'a [u8])],
    pub moves: Option<ClientMoves<'a>>,
}

/// CL_WritePacket + CL_Netchan_Transmit's clc_EOF, before XOR encoding.
pub fn write_client_message(packet: &ClientPacket<'_>) -> Result<Vec<u8>, Error> {
    let mut w = MessageWriter::new(MAX_MSGLEN)?;
    w.write_bits(packet.server_id, 32)?;
    w.write_bits(packet.message_acknowledge, 32)?;
    w.write_bits(packet.command_acknowledge, 32)?;
    for &(sequence, text) in packet.reliable_commands {
        w.write_bits(i32::from(CLC_CLIENT_COMMAND), 8)?;
        w.write_bits(sequence, 32)?;
        write_string(&mut w, text)?;
    }
    if let Some(moves) = &packet.moves {
        let count = moves.commands.len().min(MAX_PACKET_USERCMDS);
        if count > 0 {
            w.write_bits(i32::from(if moves.delta { CLC_MOVE } else { CLC_MOVE_NO_DELTA }), 8)?;
            w.write_bits(count as i32, 8)?;
            let key = moves.checksum_feed
                ^ packet.message_acknowledge
                ^ hash_key(moves.acknowledged_server_command, 32);
            let mut old = UserCmd::default();
            for cmd in &moves.commands[moves.commands.len() - count..] {
                write_delta_usercmd_key(&mut w, key, &old, cmd)?;
                old = *cmd;
            }
        }
    }
    w.write_bits(i32::from(CLC_EOF), 8)?;
    Ok(w.as_bytes().to_vec())
}

/// `getchallenge <clientChallenge>` (NET_OutOfBandPrint, no terminator).
pub fn getchallenge_packet(client_challenge: i32) -> Vec<u8> {
    [OOB_PREFIX, format!("getchallenge {client_challenge}").as_bytes()].concat()
}

/// `connect "<userinfo>"` sent through NET_OutOfBandData, which applies
/// adaptive Huffman compression after byte 12 (the `\xff*4connect ` prefix).
/// Fails when the userinfo is too short/random to fit Huff_Compress's bit
/// budget, which would yield a packet the server cannot decompress.
pub fn connect_packet(userinfo: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut packet = OOB_PREFIX.to_vec();
    packet.extend_from_slice(b"connect \"");
    packet.extend_from_slice(userinfo);
    packet.push(b'"');
    match adaptive_huffman::compress_checked(&packet, 12) {
        (compressed, false) => Ok(compressed),
        (_, true) => Err("userinfo does not fit Huff_Compress's bit budget"),
    }
}

/// A parsed connectionless (`-1` sequence) packet: CL_ConnectionlessPacket
/// tokenizes the first line; `print` then reads the remaining string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connectionless {
    pub args: Vec<Vec<u8>>,
    /// Bytes after the first line (up to a NUL), for `print`.
    pub rest: Vec<u8>,
}

impl Connectionless {
    pub fn command(&self) -> &[u8] {
        self.args.first().map_or(&[], Vec::as_slice)
    }
    pub fn arg(&self, index: usize) -> &[u8] {
        self.args.get(index).map_or(&[], Vec::as_slice)
    }
}

pub fn is_connectionless(packet: &[u8]) -> bool {
    packet.len() >= 4 && packet[..4] == *OOB_PREFIX
}

pub fn parse_connectionless(packet: &[u8]) -> Option<Connectionless> {
    let body = packet.strip_prefix(OOB_PREFIX)?;
    // MSG_ReadStringLine stops at '\n', NUL or the end.
    let line_end = body.iter().position(|&b| b == b'\n' || b == 0).unwrap_or(body.len());
    let line = &body[..line_end];
    let rest_start = (line_end + 1).min(body.len());
    let rest = &body[rest_start..];
    let rest = &rest[..rest.iter().position(|&b| b == 0).unwrap_or(rest.len())];
    Some(Connectionless {
        args: tokenize(line),
        rest: rest.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_messages_carry_sequence_and_qport() {
        let mut chan = Netchan::new(0x1234);
        let packets = chan.transmit(b"abc");
        assert_eq!(packets, vec![b"\x01\x00\x00\x00\x34\x12abc".to_vec()]);
        assert_eq!(chan.outgoing_sequence, 2);
    }

    #[test]
    fn exact_fragment_multiple_ends_with_empty_fragment() {
        let mut chan = Netchan::new(7);
        let data = vec![9u8; FRAGMENT_SIZE * 2];
        let packets = chan.transmit(&data);
        assert_eq!(packets.len(), 3);
        assert_eq!(&packets[2][10..], b"");
        assert_eq!(u16::from_le_bytes([packets[2][8], packets[2][9]]), 0);
        assert_eq!(u16::from_le_bytes([packets[2][6], packets[2][7]]) as usize, FRAGMENT_SIZE * 2);
        assert_eq!(chan.outgoing_sequence, 2);
    }

    fn server_packet(sequence: u32, fragment: Option<(u16, &[u8])>, data: &[u8]) -> Vec<u8> {
        let mut packet = Vec::new();
        match fragment {
            Some((start, chunk)) => {
                packet.extend_from_slice(&(sequence | FRAGMENT_BIT).to_le_bytes());
                packet.extend_from_slice(&start.to_le_bytes());
                packet.extend_from_slice(&(chunk.len() as u16).to_le_bytes());
                packet.extend_from_slice(chunk);
            }
            None => {
                packet.extend_from_slice(&sequence.to_le_bytes());
                packet.extend_from_slice(data);
            }
        }
        packet
    }

    #[test]
    fn reassembles_fragments_and_drops_stale_sequences() {
        let mut chan = Netchan::new(1);
        let message: Vec<u8> = (0..(FRAGMENT_SIZE + 10)).map(|i| i as u8).collect();
        assert!(chan.process(&server_packet(5, Some((0, &message[..FRAGMENT_SIZE])), &[])).is_none());
        let done = chan
            .process(&server_packet(5, Some((FRAGMENT_SIZE as u16, &message[FRAGMENT_SIZE..])), &[]))
            .unwrap();
        assert_eq!(done.sequence, 5);
        assert_eq!(done.payload, message);
        assert_eq!(chan.incoming_sequence, 0, "fragments do not advance incomingSequence");
        assert!(chan.process(&server_packet(6, None, b"x")).is_some());
        assert!(chan.process(&server_packet(6, None, b"x")).is_none());
        assert!(chan.process(&server_packet(4, None, b"x")).is_none());
        assert_eq!(chan.process(&server_packet(9, None, b"y")).unwrap().payload, b"y");
        assert_eq!(chan.dropped, 2);
    }

    #[test]
    fn missing_fragment_discards_the_message() {
        let mut chan = Netchan::new(1);
        let message = vec![3u8; FRAGMENT_SIZE + 5];
        assert!(chan.process(&server_packet(8, Some((FRAGMENT_SIZE as u16, &message[FRAGMENT_SIZE..])), &[])).is_none());
    }

    #[test]
    fn scramble_is_symmetric_and_skips_the_prefix() {
        let original: Vec<u8> = (0..64u8).collect();
        let mut data = original.clone();
        scramble(&mut data, 12, 0, 0x5a, b"say %hi");
        assert_eq!(&data[..12], &original[..12]);
        assert_ne!(data, original);
        scramble(&mut data, 12, 0, 0x5a, b"say %hi");
        assert_eq!(data, original);
    }

    #[test]
    fn client_message_round_trips_through_server_reader() {
        let cmds = [
            UserCmd { server_time: 1000, angles: [1, 2, 65535], forward_move: 127, buttons: 1, weapon: 3, ..UserCmd::default() },
            UserCmd { server_time: 1016, angles: [1, 2, 65535], forward_move: 127, buttons: 1, weapon: 3, ..UserCmd::default() },
            UserCmd { server_time: 1300, angles: [9, 2, 0], forward_move: -127, right_move: -64, up_move: 127, weapon: 3, generic_command: 4, ..UserCmd::default() },
        ];
        let server_command = b"print \"hello\"\n".as_slice();
        let reliable: [(i32, &[u8]); 1] = [(7, b"team f")];
        let packet = ClientPacket {
            server_id: 0x1234567,
            message_acknowledge: 812,
            command_acknowledge: 33,
            reliable_commands: &reliable,
            moves: Some(ClientMoves { delta: true, checksum_feed: -99, acknowledged_server_command: server_command, commands: &cmds }),
        };
        let mut data = write_client_message(&packet).unwrap();
        let plain = data.clone();
        encode_client_message(&mut data, 4242, |_| server_command).unwrap();
        assert_ne!(data, plain);
        // SV_Netchan_Decode applies the identical XOR from the same offset.
        let mut reader = MessageReader::new(&data).unwrap();
        let server_id = reader.read_long().unwrap();
        let ack = reader.read_long().unwrap();
        let _ = reader.read_long().unwrap();
        scramble(&mut data, CL_ENCODE_START, 0, (4242 ^ server_id ^ ack) as u8, server_command);
        assert_eq!(data, plain);

        let mut r = MessageReader::new(&data).unwrap();
        assert_eq!(r.read_long().unwrap(), 0x1234567);
        assert_eq!(r.read_long().unwrap(), 812);
        assert_eq!(r.read_long().unwrap(), 33);
        assert_eq!(r.read_byte().unwrap(), CLC_CLIENT_COMMAND);
        assert_eq!(r.read_long().unwrap(), 7);
        assert_eq!(r.read_string(1024).unwrap(), b"team f");
        assert_eq!(r.read_byte().unwrap(), CLC_MOVE);
        assert_eq!(r.read_byte().unwrap(), 3);
        let key = -99 ^ 812 ^ hash_key(server_command, 32);
        let mut old = UserCmd::default();
        for cmd in &cmds {
            let decoded = read_delta_usercmd_key(&mut r, key, &old).unwrap();
            assert_eq!(decoded, *cmd);
            old = decoded;
        }
        assert_eq!(r.read_byte().unwrap(), CLC_EOF);
    }

    #[test]
    fn hash_key_uses_signed_chars() {
        assert_eq!(hash_key(b"", 32), 0);
        let h = 97 * 119 + 98 * 120;
        assert_eq!(hash_key(b"ab", 32), h ^ (h >> 10) ^ (h >> 20));
        let h = -1 * 119;
        assert_eq!(hash_key(b"\xff", 32), h ^ (h >> 10) ^ (h >> 20));
    }

    #[test]
    fn connectionless_parsing_matches_readstringline() {
        let packet = b"\xff\xff\xff\xffchallengeResponse 1234 -55";
        let parsed = parse_connectionless(packet).unwrap();
        assert_eq!(parsed.command(), b"challengeResponse");
        assert_eq!(parsed.arg(1), b"1234");
        assert_eq!(parsed.arg(2), b"-55");
        let print = parse_connectionless(b"\xff\xff\xff\xffprint\nServer is full.\n").unwrap();
        assert_eq!(print.command(), b"print");
        assert_eq!(print.rest, b"Server is full.\n");
        assert_eq!(getchallenge_packet(-3), b"\xff\xff\xff\xffgetchallenge -3");
        let info = br"\name\Padawan\rate\25000\snaps\40\model\kyle/default\sex\male\saber1\single_1\saber2\none\protocol\26\qport\1234\challenge\-5501";
        let connect = connect_packet(info).unwrap();
        assert!(connect_packet(b"x").is_err());
        assert_eq!(&connect[..12], b"\xff\xff\xff\xffconnect ");
        assert_eq!(
            adaptive_huffman::decompress(&connect, 12, MAX_MSGLEN),
            [b"\xff\xff\xff\xffconnect \"".as_slice(), info, b"\""].concat()
        );
    }
}
