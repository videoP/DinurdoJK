//! Sans-IO client connection (OpenJK `clc`/`cl` networking state).
//!
//! The owner supplies received datagrams and a millisecond clock
//! (`cls.realtime`) and sends whatever [`ClientSession::take_outgoing`] returns.
//! References: OpenJK codemp `client/cl_main.cpp` (CL_Connect_f,
//! CL_CheckForResend, CL_ConnectionlessPacket, CL_PacketEvent,
//! CL_CheckTimeout, CL_Disconnect, CL_AddReliableCommand), `cl_parse.cpp`
//! (CL_ParseServerMessage, CL_ParseSnapshot, CL_ParseGamestate,
//! CL_SystemInfoChanged), `cl_input.cpp` (CL_CreateNewCommands,
//! CL_ReadyToSendPacket, CL_WritePacket) and `cl_cgame.cpp`
//! (CL_SetCGameTime, CL_AdjustTimeDelta, CL_FirstSnapshot).

use std::{collections::VecDeque, net::SocketAddr};

use crate::{
    commands::{atoi, info_value, tokenize, BigConfigOutcome, BigConfigString},
    netchan::{
        self, ClientMoves, ClientPacket, Netchan, UserCmd, MAX_PACKET_USERCMDS,
        MAX_RELIABLE_COMMANDS, MAX_STRING_CHARS,
    },
    server::{Decoder, DownloadBlock, Event, ServerCommand, Snapshot},
};

pub const RETRANSMIT_TIMEOUT: i32 = 3000;
/// Bound unsuccessful admission attempts. This is separate from the much longer
/// idle timeout of an established connection (which can be downloading a map).
const JOIN_TIMEOUT: i32 = 30_000;
/// Client usercmd history. Vanilla OpenJK uses 64, which only covers 64 ms at
/// 1000 commands/s; prediction then stalls ("exceeded PACKET_BACKUP on
/// commands") whenever latency exceeds it. TaystJK/JAPro raise it with
/// cl_commandsize (max 512). The ring is client-local, not part of the protocol.
pub const CMD_BACKUP: usize = 512;
/// Client-side PACKET_BACKUP (used for outPackets / ping estimation). Vanilla's 32 covers
/// 256 ms at the stock cl_maxpackets cap of 125, but only 32 ms at cl_maxpackets 1000, so the
/// ping search ran off the end (999) whenever latency exceeded that. The ring is client-local.
pub const PACKET_BACKUP: usize = 512;
const RESET_TIME: i32 = 500;
pub const SNAPFLAG_NOT_ACTIVE: u8 = 2;
pub const CS_SYSTEMINFO: u16 = 1;

/// OpenJK connstate_t, restricted to the networked states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConnectionState {
    Disconnected,
    /// Sending `getchallenge`.
    Connecting,
    /// Sending `connect` with the server's challenge.
    Challenging,
    /// Netchan up; waiting for / holding a gamestate while the map loads.
    Connected,
    /// Map loaded; usercmds flow, waiting for the first active snapshot.
    Primed,
    Active,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    StateChanged(ConnectionState),
    /// Public serverinfo returned by the speculative `getinfo` query sent
    /// alongside `getchallenge` so map loading can overlap the handshake.
    ServerInfo(crate::ServerInfo),
    /// A connectionless `print` (e.g. "Server is full.").
    Print(Vec<u8>),
    /// A new gamestate is in [`ClientSession::decoder`]; the map must (re)load
    /// and [`ClientSession::set_primed`] be called afterwards.
    Gamestate,
    ServerCommand(ServerCommand),
    /// A valid snapshot, in message order.
    Snapshot(Snapshot),
    /// Decoded server-message bytes suitable for CL_WriteDemoMessage framing.
    /// `full_snapshot` is true when this same message established a non-delta snapshot.
    DemoMessage { sequence: i32, payload: Vec<u8>, full_snapshot: bool },
    MapChange,
    Download(DownloadBlock),
    Disconnected(String),
}

#[derive(Debug, Clone, Copy, Default)]
struct OutPacket {
    realtime: i32,
    server_time: i32,
    cmd_number: i32,
    reliable_sequence: i32,
}

/// Client-side packet preparation counters. These do not prove server receipt;
/// UDP send outcomes are recorded separately by the transport.
#[derive(Debug, Clone, Copy, Default)]
pub struct PacketDebug {
    pub packets: u64,
    pub empty_packets: u64,
    pub command_transmissions: u64,
    pub last_sequence: i32,
    pub last_realtime: i32,
    pub last_command_count: usize,
    pub first_command_time: i32,
    pub last_command_time: i32,
}

/// See [`ClientSession::snapshot_stats`]. Gaps are in server milliseconds.
#[derive(Debug, Clone, Default)]
pub struct SnapshotStats {
    pub samples: usize,
    pub min_ms: i32,
    pub p10_ms: i32,
    pub median_ms: i32,
    pub max_ms: i32,
    /// Snapshots actually received per real second.
    pub received_per_second: f32,
    /// The most frequent gaps, `(ms, count)`.
    pub common_gaps: Vec<(i32, usize)>,
}

pub struct ClientSession {
    server: SocketAddr,
    state: ConnectionState,
    userinfo: Vec<u8>,
    qport: u16,
    /// clc.challenge: ours until challengeResponse, then the server's.
    challenge: i32,
    /// Independent echo token for the public `getinfo` query.
    info_challenge: String,
    server_info_seen: bool,
    /// Hold the challenge->connect transition until the owner has inspected
    /// infoResponse (mapname/autodownload preflight).
    connect_preflight_paused: bool,
    preflight_wait_started: Option<i32>,
    connect_time: i32,
    connect_packet_count: i32,
    connect_started_at: i32,
    /// A `connectResponse` alone is not an accepted gameplay gamestate.
    gamestate_received: bool,
    awaiting_gamestate_since: Option<i32>,
    netchan: Option<Netchan>,
    last_packet_time: i32,
    last_packet_sent_time: i32,

    decoder: Decoder,
    /// Only JK2 connections allocate a separate wire decoder / presentation map.
    jk2: Option<Box<crate::jk2::live::LiveDecoder>>,
    server_message_sequence: i32,
    server_command_sequence: i32,
    server_commands: Vec<Vec<u8>>,
    reliable_sequence: i32,
    reliable_acknowledge: i32,
    reliable_commands: Vec<Vec<u8>>,
    big_config: BigConfigString,
    server_id: i32,
    sv_pure: bool,

    cmds: Vec<UserCmd>,
    cmd_number: i32,
    /// Effective cl_commandsize (64/128/256/512). Physical storage remains
    /// CMD_BACKUP so AUTO changes never remap live command slots.
    command_window: usize,
    out_packets: [OutPacket; PACKET_BACKUP],
    packet_debug: PacketDebug,
    /// cl_packetdup: how many earlier packets' usercmds each packet repeats (0..=5).
    packet_dup: i32,
    /// cl.snap: the newest valid snapshot's bookkeeping.
    snap_valid: bool,
    snap_message_num: i32,
    snap_server_time: i32,
    /// `(server time, our realtime)` of recently received snapshots, for `snapshot_stats`.
    snap_times: VecDeque<(i32, i32)>,
    snap_flags: u8,
    snap_command_time: i32,
    ping: i32,
    new_snapshots: bool,
    server_time: i32,
    old_server_time: i32,
    old_frame_server_time: i32,
    server_time_delta: i32,
    extrapolated_snapshot: bool,

    outgoing: Vec<Vec<u8>>,
    events: VecDeque<SessionEvent>,
    /// Snapshots decoded but not yet validated by `CL_ParseSnapshot`.
    pending_snapshot: bool,
    /// Avoid carrying decoded packet payloads through SessionEvent unless recording.
    demo_capture: bool,
    /// OpenJK `demowaiting`: force clc_moveNoDelta until the recording gets a full snapshot.
    demo_waiting_for_full_snapshot: bool,
}

impl ClientSession {
    /// CL_Connect_f for a remote server. `client_challenge` is the random
    /// value OpenJK derives from rand()/Com_Milliseconds; `qport` is net_qport.
    pub fn connect(server: SocketAddr, userinfo: Vec<u8>, qport: u16, client_challenge: i32, realtime: i32) -> Self {
        Self::connect_inner(server, userinfo, qport, client_challenge, realtime, false, None)
    }

    /// Same handshake, but pause before sending the gameplay `connect` packet
    /// so the owner can inspect the speculative infoResponse first.
    pub fn connect_preflight(server: SocketAddr, userinfo: Vec<u8>, qport: u16, client_challenge: i32, realtime: i32) -> Self {
        Self::connect_inner(server, userinfo, qport, client_challenge, realtime, true, None)
    }

    pub fn connect_jk2_preflight(server: SocketAddr, mut userinfo: Vec<u8>, qport: u16,
        client_challenge: i32, realtime: i32, version: crate::jk2::Version) -> Self
    {
        set_info_value(&mut userinfo, b"team", b"spectator");
        Self::connect_inner(server, userinfo, qport, client_challenge, realtime, true, Some(version))
    }

    fn connect_inner(server: SocketAddr, userinfo: Vec<u8>, qport: u16, client_challenge: i32, realtime: i32, pause_preflight: bool, jk2: Option<crate::jk2::Version>) -> Self {
        let info_challenge = format!("{:08x}", client_challenge as u32);
        let mut session = Self {
            server,
            state: ConnectionState::Connecting,
            userinfo,
            qport,
            challenge: client_challenge,
            info_challenge,
            server_info_seen: false,
            connect_preflight_paused: pause_preflight,
            preflight_wait_started: None,
            connect_time: -99999,
            connect_packet_count: 0,
            connect_started_at: realtime,
            gamestate_received: false,
            awaiting_gamestate_since: None,
            netchan: None,
            last_packet_time: realtime,
            last_packet_sent_time: -9999,
            decoder: Decoder::new(),
            jk2: jk2.map(|version| Box::new(crate::jk2::live::LiveDecoder::new(version))),
            server_message_sequence: 0,
            server_command_sequence: 0,
            server_commands: vec![Vec::new(); MAX_RELIABLE_COMMANDS],
            reliable_sequence: 0,
            reliable_acknowledge: 0,
            reliable_commands: vec![Vec::new(); MAX_RELIABLE_COMMANDS],
            big_config: BigConfigString::default(),
            server_id: 0,
            sv_pure: false,
            cmds: vec![UserCmd::default(); CMD_BACKUP],
            cmd_number: 0,
            command_window: 64,
            out_packets: [OutPacket::default(); PACKET_BACKUP],
            packet_debug: PacketDebug::default(),
            packet_dup: 1,
            snap_valid: false,
            snap_message_num: 0,
            snap_server_time: 0,
            snap_times: VecDeque::new(),
            snap_flags: 0,
            snap_command_time: 0,
            ping: 999,
            new_snapshots: false,
            server_time: 0,
            old_server_time: 0,
            old_frame_server_time: 0,
            server_time_delta: 0,
            extrapolated_snapshot: false,
            outgoing: Vec::new(),
            events: VecDeque::new(),
            pending_snapshot: false,
            demo_capture: false,
            demo_waiting_for_full_snapshot: false,
        };
        session.events.push_back(SessionEvent::StateChanged(ConnectionState::Connecting));
        session.check_for_resend(realtime);
        session
    }

    pub fn server(&self) -> SocketAddr { self.server }
    pub fn state(&self) -> ConnectionState { self.state }
    pub fn has_gamestate(&self) -> bool { self.gamestate_received }
    pub fn decoder(&self) -> &Decoder { &self.decoder }
    pub fn jk2_version(&self) -> Option<crate::jk2::Version> { self.jk2.as_ref().map(|state| state.version) }
    pub fn server_time(&self) -> i32 { self.server_time }
    /// Server time minus the commandTime the newest snapshot acknowledged: how far behind the
    /// server's clock our commands are being processed (about the round trip plus a server frame).
    pub fn snapshot_command_lag(&self) -> i32 { self.snap_server_time - self.snap_command_time }

    /// How often snapshots arrive, from the server times of the recent ones. The gap between two
    /// snapshots is a whole number of server frames, so the smallest common gap approximates the
    /// server frame time (1000 / sv_fps), but only while our `snaps` setting does not floor it.
    pub fn snapshot_stats(&self) -> SnapshotStats {
        let mut gaps: Vec<i32> = self
            .snap_times
            .iter()
            .zip(self.snap_times.iter().skip(1))
            .map(|(a, b)| b.0 - a.0)
            .filter(|gap| *gap > 0 && *gap <= 1000)
            .collect();
        let mut stats = SnapshotStats { samples: gaps.len(), ..SnapshotStats::default() };
        if gaps.is_empty() {
            return stats;
        }
        let mut counts: std::collections::BTreeMap<i32, usize> = std::collections::BTreeMap::new();
        for gap in &gaps {
            *counts.entry(*gap).or_default() += 1;
        }
        let mut common: Vec<(i32, usize)> = counts.into_iter().collect();
        common.sort_by(|a, b| b.1.cmp(&a.1));
        common.truncate(4);
        stats.common_gaps = common;
        gaps.sort_unstable();
        stats.min_ms = gaps[0];
        stats.p10_ms = gaps[gaps.len() / 10];
        stats.median_ms = gaps[gaps.len() / 2];
        stats.max_ms = gaps[gaps.len() - 1];
        if let (Some(first), Some(last)) = (self.snap_times.front(), self.snap_times.back()) {
            let real_ms = last.1 - first.1;
            if real_ms > 0 {
                stats.received_per_second = (self.snap_times.len() - 1) as f32 * 1000.0 / real_ms as f32;
            }
        }
        stats
    }
    pub fn ping(&self) -> i32 { self.ping }
    pub fn cmd_number(&self) -> i32 { self.cmd_number }
    pub fn command_window(&self) -> usize { self.command_window }

    /// TaystJK CL_GetUserCmd / CL_WritePacket share REAL_CMD_MASK.
    /// Use a logical window instead of physically remasking the 512-slot ring
    /// so changing AUTO during a session cannot reinterpret old ring slots.
    pub fn set_command_window(&mut self, requested: usize) {
        self.command_window = requested.clamp(64, CMD_BACKUP).next_power_of_two();
    }
    pub fn client_num(&self) -> i32 { self.decoder.client_number }
    pub fn server_id(&self) -> i32 { self.server_id }
    pub fn server_message_sequence(&self) -> i32 { self.server_message_sequence }
    pub fn reliable_sequence(&self) -> i32 { self.reliable_sequence }
    pub fn sv_pure(&self) -> bool { self.sv_pure }
    pub fn dropped_packets(&self) -> i32 { self.netchan.as_ref().map_or(0, |chan| chan.dropped) }
    pub fn packet_debug(&self) -> PacketDebug { self.packet_debug }
    pub fn connect_preflight_paused(&self) -> bool { self.connect_preflight_paused }

    pub fn resume_connect(&mut self, realtime: i32) {
        if !self.connect_preflight_paused {
            return;
        }
        self.connect_preflight_paused = false;
        self.preflight_wait_started = None;
        // Time spent on the user's missing-map dialog is not a stalled server.
        if self.server_info_seen && self.state == ConnectionState::Challenging {
            self.connect_started_at = realtime;
        }
        self.connect_time = -99999;
        self.check_for_resend(realtime);
    }

    /// Flush a newly queued reliable command immediately. Downloads use this
    /// for `download`, `nextdl`, `stopdl` and `donedl` acknowledgements.
    pub fn write_packet_now(&mut self, realtime: i32) {
        if self.state >= ConnectionState::Connected {
            self.write_packet(realtime);
        }
    }

    /// CL_GetUserCmd: do not expose commands past the active cl_commandsize
    /// history window, even though the backing ring retains 512 slots.
    pub fn command(&self, number: i32) -> Option<UserCmd> {
        if number <= 0 || number > self.cmd_number || self.cmd_number - number >= self.command_window as i32 {
            return None;
        }
        Some(self.cmds[number as usize & (CMD_BACKUP - 1)])
    }

    pub fn take_outgoing(&mut self) -> Vec<Vec<u8>> { std::mem::take(&mut self.outgoing) }
    pub fn take_events(&mut self) -> Vec<SessionEvent> { self.events.drain(..).collect() }
    pub fn set_demo_capture(&mut self, enabled: bool) {
        self.demo_capture = enabled;
        self.demo_waiting_for_full_snapshot = enabled;
    }

    fn set_state(&mut self, state: ConnectionState) {
        if self.state != state {
            self.state = state;
            self.events.push_back(SessionEvent::StateChanged(state));
        }
    }

    fn drop_connection(&mut self, reason: String) {
        if self.state == ConnectionState::Disconnected {
            return;
        }
        self.state = ConnectionState::Disconnected;
        self.netchan = None;
        self.outgoing.clear(); // Do not flush a queued connect resend after rejection.
        self.events.push_back(SessionEvent::Disconnected(reason));
    }

    fn send_oob(&mut self, packet: Vec<u8>) {
        self.outgoing.push(packet);
    }

    /// CL_CheckForResend plus an independent public `getinfo` query. The query
    /// is not part of the gameplay handshake; it exists only to surface mapname
    /// early enough for the map worker to overlap challenge/gamestate latency.
    fn check_for_resend(&mut self, realtime: i32) {
        if !matches!(self.state, ConnectionState::Connecting | ConnectionState::Challenging) {
            return;
        }
        if realtime - self.connect_time < RETRANSMIT_TIMEOUT {
            return;
        }
        self.connect_time = realtime;
        self.connect_packet_count += 1;
        match self.state {
            ConnectionState::Connecting => {
                let packet = netchan::getchallenge_packet(self.challenge);
                self.send_oob(packet);
                if let Ok(packet) = crate::getinfo_request(&self.info_challenge) {
                    self.send_oob(packet);
                }
            }
            ConnectionState::Challenging => {
                if self.connect_preflight_paused {
                    return;
                }
                let mut info = self.userinfo.clone();
                for (key, value) in [
                    ("protocol", self.jk2_version().map_or(crate::PROTOCOL_VERSION, |v| v.protocol()).to_string()),
                    ("qport", self.qport.to_string()),
                    ("challenge", self.challenge.to_string()),
                ] {
                    set_info_value(&mut info, key.as_bytes(), value.as_bytes());
                }
                match netchan::connect_packet(&info) {
                    Ok(packet) => self.send_oob(packet),
                    Err(error) => self.drop_connection(format!("cannot build connect packet: {error}")),
                }
            }
            _ => unreachable!(),
        }
    }

    /// Per-frame housekeeping: CL_CheckForResend and CL_CheckTimeout.
    pub fn frame(&mut self, realtime: i32, timeout_ms: i32) {
        // Some legacy/proxied servers do not answer getinfo. Do not make them
        // unjoinable: after a short preflight window, continue the normal JKA
        // handshake. If infoResponse did arrive, the UI owns the decision and
        // this timer deliberately does not bypass the missing-map prompt.
        if self.connect_preflight_paused && !self.server_info_seen
            && self.preflight_wait_started.is_some_and(|started| realtime - started >= 1500)
        {
            self.resume_connect(realtime);
        }
        self.check_for_resend(realtime);
        // Stock OpenJK retries the handshake indefinitely. With speculative BSP
        // loading that can leave an apparently complete world on screen after a
        // rejection, so bound admission independently of the in-game timeout.
        // A missing-map/password preflight dialog can remain open by choice.
        if !(self.connect_preflight_paused && self.server_info_seen
            && self.state == ConnectionState::Challenging)
            && realtime.saturating_sub(self.connect_started_at) >= JOIN_TIMEOUT
        {
            match self.state {
                ConnectionState::Connecting => self.drop_connection("Timed out awaiting challenge.".into()),
                ConnectionState::Challenging => self.drop_connection("Timed out awaiting connection.".into()),
                _ => {}
            }
        }
        if self.state == ConnectionState::Connected && !self.gamestate_received
            && self.awaiting_gamestate_since.is_some_and(|since| {
                realtime.saturating_sub(since) >= JOIN_TIMEOUT
            })
        {
            self.drop_connection("Timed out awaiting gamestate.".into());
        }
        if self.state >= ConnectionState::Connected && realtime - self.last_packet_time > timeout_ms {
            self.drop_connection("Server connection timed out.".into());
        }
    }

    /// CL_PacketEvent.
    pub fn packet_event(&mut self, from: SocketAddr, data: &[u8], realtime: i32) {
        if self.state == ConnectionState::Disconnected {
            return;
        }
        if netchan::is_connectionless(data) {
            self.connectionless_packet(from, data, realtime);
            return;
        }
        if self.state < ConnectionState::Connected || data.len() < 4 || from != self.server {
            return;
        }
        let Some(chan) = self.netchan.as_mut() else { return; };
        let Some(mut message) = chan.process(data) else { return; };
        self.last_packet_time = realtime;
        // CL_Netchan_Decode keys on our own reliable command that this message
        // acknowledges.
        let reliable = &self.reliable_commands;
        let key_text = |ack: i32| reliable[ack as usize & (MAX_RELIABLE_COMMANDS - 1)].as_slice();
        let decoded = if self.jk2.is_some() {
            netchan::decode_jk2_server_message(&mut message.payload, message.sequence, self.challenge, key_text)
        } else { netchan::decode_server_message(&mut message.payload, message.sequence, self.challenge, key_text) };
        if let Err(error) = decoded {
            self.drop_connection(format!("CL_Netchan_Decode: {error}"));
            return;
        }
        self.server_message_sequence = message.sequence;
        self.parse_server_message(message.sequence, &message.payload, realtime);
        if self.demo_capture && self.state != ConnectionState::Disconnected {
            let full_snapshot = self.decoder.latest_snapshot().is_some_and(|snapshot| {
                snapshot.message_num == message.sequence && snapshot.delta_num < 0
            });
            if full_snapshot {
                self.demo_waiting_for_full_snapshot = false;
            }
            self.events.push_back(SessionEvent::DemoMessage {
                sequence: message.sequence,
                payload: message.payload,
                full_snapshot,
            });
        }
    }

    fn connectionless_packet(&mut self, from: SocketAddr, data: &[u8], realtime: i32) {
        let Some(packet) = netchan::parse_connectionless(data) else { return; };
        let command = packet.command().to_ascii_lowercase();
        match command.as_slice() {
            b"inforesponse" => {
                if from != self.server || self.server_info_seen {
                    return;
                }
                if let Ok(info) = crate::parse_info_response(data, &self.info_challenge) {
                    if let Some(version) = self.jk2_version() {
                        if info.protocol != version.protocol() || info.get(b"version").is_some_and(|v| v.windows(5).any(|s| s == b"v1.03")) {
                            self.drop_connection(format!("Server does not match selected JK2 {}", version.name()));
                            return;
                        }
                    }
                    self.server_info_seen = true;
                    self.events.push_back(SessionEvent::ServerInfo(info));
                }
            }
            b"challengeresponse" => {
                if self.state != ConnectionState::Connecting {
                    return;
                }
                let echoed = packet.arg(2);
                if from != self.server && (echoed.is_empty() || atoi(echoed) != self.challenge) {
                    return;
                }
                self.challenge = atoi(packet.arg(1));
                self.connect_packet_count = 0;
                self.connect_time = -99999;
                // A proxy may hand the connection to another address.
                self.server = from;
                self.set_state(ConnectionState::Challenging);
                if self.connect_preflight_paused && !self.server_info_seen {
                    self.preflight_wait_started = Some(realtime);
                }
                self.check_for_resend(realtime);
            }
            b"connectresponse" => {
                if self.state != ConnectionState::Challenging || from != self.server {
                    return;
                }
                self.netchan = Some(if self.jk2.is_some() { Netchan::new_jk2(self.qport) } else { Netchan::new(self.qport) });
                self.last_packet_sent_time = -9999;
                self.last_packet_time = realtime;
                self.awaiting_gamestate_since = Some(realtime);
                self.set_state(ConnectionState::Connected);
                // CL_Frame sends immediately; the first packet makes the server
                // (re)send the gamestate it queued on connect.
                self.write_packet(realtime);
            }
            b"disconnect" => {
                // CL_DisconnectPacket: ignore possible spoofs while traffic flows.
                if from == self.server && self.state >= ConnectionState::Connected && realtime - self.last_packet_time >= 3000 {
                    self.drop_connection("Server disconnected for unknown reason".into());
                }
            }
            b"print" => {
                if from == self.server {
                    if !self.gamestate_received {
                        // SV_DirectConnect uses connectionless `print` for
                        // refusals (IP limit, full/banned server, bad password,
                        // etc.). It never sends a gamestate for that attempt.
                        // Preserve the server's text instead of retrying while
                        // the speculative map continues loading.
                        let reason = String::from_utf8_lossy(&packet.rest)
                            .trim_matches(|c: char| c == '\0' || c.is_whitespace())
                            .to_owned();
                        self.drop_connection(if reason.is_empty() {
                            "Server refused the connection.".into()
                        } else {
                            reason
                        });
                    } else {
                        self.events.push_back(SessionEvent::Print(packet.rest.clone()));
                    }
                }
            }
            b"echo" => {
                let reply = [b"\xff\xff\xff\xff".as_slice(), packet.arg(1)].concat();
                self.send_oob(reply);
            }
            _ => {}
        }
    }

    /// CL_ParseServerMessage over the shared decoder.
    fn parse_server_message(&mut self, sequence: i32, payload: &[u8], realtime: i32) {
        let parsed = if let Some(jk2) = &mut self.jk2 { jk2.parse(sequence, payload, &mut self.decoder) }
            else { self.decoder.parse_packet(sequence, payload).map_err(|e| e.to_string()) };
        let result = match parsed {
            Ok(result) => result,
            Err(error) => {
                self.drop_connection(format!("CL_ParseServerMessage: {error}"));
                return;
            }
        };
        self.reliable_acknowledge = result.reliable_acknowledge;
        if self.reliable_acknowledge < self.reliable_sequence - MAX_RELIABLE_COMMANDS as i32 {
            self.reliable_acknowledge = self.reliable_sequence;
        }
        for event in result.events {
            match event {
                Event::Nop => {}
                Event::ServerCommand(command) => self.server_command(command),
                Event::Gamestate { server_command_sequence, .. } => {
                    self.server_command_sequence = server_command_sequence;
                    self.gamestate_parsed();
                    if self.jk2.is_some() {
                        if let Err(error) = self.add_reliable_command(b"team spectator", false) {
                            self.drop_connection(error);
                            return;
                        }
                    }
                }
                Event::Snapshot { .. } => {
                    if let Some(snapshot) = self.decoder.latest_snapshot().cloned() {
                        self.snapshot_parsed(snapshot, realtime);
                    }
                }
                Event::SetGame(_) => {}
                Event::MapChange => self.events.push_back(SessionEvent::MapChange),
                Event::Download(block) => self.events.push_back(SessionEvent::Download(block)),
            }
            if self.state == ConnectionState::Disconnected {
                return;
            }
        }
    }

    /// CL_ParseCommandString (dedupe already done by the decoder) plus the
    /// client-level parts of CL_GetServerCommand that must not wait for CGame.
    fn server_command(&mut self, command: ServerCommand) {
        self.server_command_sequence = command.sequence;
        let mut stored = command.text.clone();
        stored.truncate(MAX_STRING_CHARS - 1);
        self.server_commands[command.sequence as usize & (MAX_RELIABLE_COMMANDS - 1)] = stored;

        let args = tokenize(&command.text);
        if args.first().is_some_and(|name| name == b"disconnect") {
            let reason = args.get(1).map(|r| String::from_utf8_lossy(r).into_owned()).unwrap_or_default();
            self.events.push_back(SessionEvent::ServerCommand(command));
            self.drop_connection(format!("Server disconnected: {reason}"));
            return;
        }
        // Track sv_serverid as soon as systeminfo changes: every packet we
        // send afterwards must carry it or the server ignores our usercmds.
        let cs = match self.big_config.feed(&command.text) {
            BigConfigOutcome::PassThrough => Some(command.text.clone()),
            BigConfigOutcome::Complete(text) => Some(text),
            BigConfigOutcome::Pending => None,
            BigConfigOutcome::Overflow => {
                self.drop_connection("bcs exceeded BIG_INFO_STRING".into());
                return;
            }
        };
        if let Some(text) = cs {
            let args = tokenize(&text);
            if args.len() >= 3 && args[0] == b"cs" && atoi(&args[1]) == i32::from(CS_SYSTEMINFO) {
                self.system_info_changed(&args[2..].join(&b' '));
            }
        }
        if let Some(jk2) = &mut self.jk2 {
            match jk2.command(command, &mut self.decoder) {
                Ok(Some(command)) => self.events.push_back(SessionEvent::ServerCommand(command)),
                Ok(None) => {}
                Err(error) => self.drop_connection(error),
            }
        } else { self.events.push_back(SessionEvent::ServerCommand(command)); }
    }

    fn system_info_changed(&mut self, info: &[u8]) {
        self.server_id = info_value(info, b"sv_serverid").map_or(0, atoi);
        self.sv_pure = info_value(info, b"sv_pure").is_some_and(|value| atoi(value) != 0);
    }

    /// CL_ParseGamestate's client-state side (CL_ClearState + serverId).
    fn gamestate_parsed(&mut self) {
        self.gamestate_received = true;
        self.awaiting_gamestate_since = None;
        let info = self.decoder.configstrings.get(&CS_SYSTEMINFO).cloned().unwrap_or_default();
        self.system_info_changed(&info);
        self.connect_packet_count = 0;
        self.cmds.fill(UserCmd::default());
        self.cmd_number = 0;
        self.out_packets = [OutPacket::default(); PACKET_BACKUP];
        self.packet_debug = PacketDebug::default();
        self.snap_times.clear();
        self.snap_valid = false;
        self.snap_message_num = 0;
        self.new_snapshots = false;
        self.server_time = 0;
        self.old_server_time = 0;
        self.old_frame_server_time = 0;
        self.server_time_delta = 0;
        self.big_config = BigConfigString::default();
        self.pending_snapshot = false;
        // CL_InitDownloads -> CL_DownloadsComplete: CA_LOADING until the
        // owner finishes loading the map and calls set_primed().
        self.state = ConnectionState::Connected;
        self.events.push_back(SessionEvent::Gamestate);
    }

    /// CL_ParseSnapshot bookkeeping after the decoder accepted a valid frame.
    fn snapshot_parsed(&mut self, mut snapshot: Snapshot, realtime: i32) {
        self.snap_valid = true;
        self.snap_message_num = snapshot.message_num;
        self.snap_server_time = snapshot.server_time;
        if self.snap_times.back().map_or(true, |&(time, _)| time != snapshot.server_time) {
            self.snap_times.push_back((snapshot.server_time, realtime));
            while self.snap_times.len() > 256 {
                self.snap_times.pop_front();
            }
        }
        self.snap_flags = snapshot.snap_flags;
        self.snap_command_time = snapshot.player_state.field_i32("commandTime").unwrap_or(0);
        self.ping = 999;
        if let Some(chan) = &self.netchan {
            for i in 0..PACKET_BACKUP as i32 {
                let packet = self.out_packets[(chan.outgoing_sequence - 1 - i) as usize & (PACKET_BACKUP - 1)];
                if self.snap_command_time >= packet.server_time {
                    self.ping = realtime - packet.realtime;
                    break;
                }
            }
        }
        snapshot.ping = self.ping;
        self.new_snapshots = true;
        self.pending_snapshot = true;
        self.events.push_back(SessionEvent::Snapshot(snapshot));
    }

    /// CL_DownloadsComplete after CL_InitCGame: the map is loaded.
    pub fn set_primed(&mut self, realtime: i32) {
        if self.state != ConnectionState::Connected {
            return;
        }
        self.set_state(ConnectionState::Primed);
        for _ in 0..3 {
            self.write_packet(realtime);
        }
    }

    /// CL_AddReliableCommand. Fails on overflow (ERR_DROP "Client command overflow").
    pub fn add_reliable_command(&mut self, text: &[u8], is_disconnect: bool) -> Result<(), String> {
        if self.jk2.is_some() {
            let args = tokenize(text);
            if args.first().is_some_and(|s| s.eq_ignore_ascii_case(b"team"))
                && args.get(1).is_some_and(|s| !s.eq_ignore_ascii_case(b"spectator") && !s.eq_ignore_ascii_case(b"s")) {
                return Err("JK2 live milestone supports spectating; native MVSDK prediction is not integrated yet".into());
            }
        }
        let unacknowledged = self.reliable_sequence - self.reliable_acknowledge;
        let limit = MAX_RELIABLE_COMMANDS as i32;
        if (is_disconnect && unacknowledged > limit) || (!is_disconnect && unacknowledged >= limit) {
            self.drop_connection("Client command overflow".into());
            return Err("Client command overflow".into());
        }
        self.reliable_sequence += 1;
        let mut stored = text.to_vec();
        stored.truncate(MAX_STRING_CHARS - 1);
        self.reliable_commands[self.reliable_sequence as usize & (MAX_RELIABLE_COMMANDS - 1)] = stored;
        Ok(())
    }

    /// CL_CreateNewCommands for one frame; the caller fills everything except
    /// serverTime, which is cl.serverTime (CL_FinishMove).
    pub fn create_command(&mut self, cmd: UserCmd) -> Option<i32> {
        let server_time = self.server_time;
        self.create_command_at(cmd, server_time)
    }

    /// Like [`Self::create_command`], stamped with an explicit `cl.serverTime`
    /// (callers keep it at or before the current server time and past the previous
    /// command's, so the stream stays monotonic).
    pub fn create_command_at(&mut self, mut cmd: UserCmd, server_time: i32) -> Option<i32> {
        if self.state < ConnectionState::Primed {
            return None;
        }
        cmd.server_time = server_time;
        if self.jk2.is_some() {
            // Spectator navigation / follow only. JKA selection/generic IDs
            // must never be sent as JK2 gameplay IDs.
            cmd.weapon = 0;
            cmd.force_selection = 0;
            cmd.inventory_selection = 0;
            cmd.generic_command = 0;
        }
        self.cmd_number += 1;
        self.cmds[self.cmd_number as usize & (CMD_BACKUP - 1)] = cmd;
        Some(self.cmd_number)
    }

    /// cl_packetdup (clamped to OpenJK's 0..=5).
    pub fn set_packet_dup(&mut self, dup: i32) {
        self.packet_dup = dup.clamp(0, 5);
    }

    /// CL_ReadyToSendPacket + CL_WritePacket.
    pub fn send_commands(&mut self, realtime: i32, max_packets: i32) {
        if self.state < ConnectionState::Connected {
            return;
        }
        if self.state < ConnectionState::Primed && realtime - self.last_packet_sent_time < 1000 {
            return;
        }
        if self.state >= ConnectionState::Primed {
            let Some(chan) = &self.netchan else { return; };
            let max_packets = max_packets.clamp(15, 1000);
            let old = self.out_packets[(chan.outgoing_sequence - 1) as usize & (PACKET_BACKUP - 1)];
            if realtime - old.realtime < 1000 / max_packets {
                return;
            }
            // CL_SendCmd normally runs on command-producing client frames.
            // Our app ticks independently of cl_commandRate: sending here on
            // every tick advances the packet history with empty messages and
            // reduces cl_packetdup's command recovery window to 1-2 ms at a
            // high cl_maxpackets, even when commands are 8-17 ms apart.
            // During active play send with new commands or a newly queued
            // reliable command. Repeat pending reliable commands with those
            // packets, and retain an idle keepalive if commands stop.
            if self.state == ConnectionState::Active
                && self.cmd_number > 0
                && self.cmd_number == old.cmd_number
                && self.reliable_sequence <= old.reliable_sequence
                && realtime - old.realtime < 1000
            {
                return;
            }
        }
        self.write_packet(realtime);
    }

    fn write_packet(&mut self, realtime: i32) {
        let Some(outgoing_sequence) = self.netchan.as_ref().map(|chan| chan.outgoing_sequence) else { return; };
        let reliable: Vec<(i32, &[u8])> = ((self.reliable_acknowledge + 1)..=self.reliable_sequence)
            .map(|sequence| (sequence, self.reliable_commands[sequence as usize & (MAX_RELIABLE_COMMANDS - 1)].as_slice()))
            .collect();
        // cl_packetdup N: resend the commands of the previous N packets too.
        let old_packet = self.out_packets[(outgoing_sequence - 1 - self.packet_dup) as usize & (PACKET_BACKUP - 1)];
        // TaystJK serializes commands through its cl_commandsize ring mask.
        // Since MAX_PACKET_USERCMDS (32) < our minimum window (64), normal
        // packet command counts remain identical. Keep the explicit bound for
        // correctness if protocol limits or the window change in future.
        let count = (self.cmd_number - old_packet.cmd_number)
            .clamp(0, MAX_PACKET_USERCMDS.min(self.command_window) as i32) as usize;
        let commands: Vec<UserCmd> = (0..count)
            .map(|i| self.cmds[(self.cmd_number - count as i32 + i as i32 + 1) as usize & (CMD_BACKUP - 1)])
            .collect();
        let acknowledged_server_command =
            self.server_commands[self.server_command_sequence as usize & (MAX_RELIABLE_COMMANDS - 1)].clone();
        let delta = self.snap_valid
            && self.server_message_sequence == self.snap_message_num
            && !self.demo_waiting_for_full_snapshot;
        let packet = ClientPacket {
            server_id: self.server_id,
            message_acknowledge: self.server_message_sequence,
            command_acknowledge: self.server_command_sequence,
            reliable_commands: &reliable,
            moves: (count > 0).then(|| ClientMoves {
                delta,
                checksum_feed: self.decoder.checksum_feed as i32,
                acknowledged_server_command: &acknowledged_server_command,
                commands: &commands,
            }),
        };
        let encoded = if self.jk2.is_some() { netchan::write_jk2_client_message(&packet) }
            else { netchan::write_client_message(&packet) };
        let mut data = match encoded {
            Ok(data) => data,
            Err(error) => {
                self.drop_connection(format!("CL_WritePacket: {error}"));
                return;
            }
        };
        let server_commands = &self.server_commands;
        let key_text = |ack: i32| server_commands[ack as usize & (MAX_RELIABLE_COMMANDS - 1)].as_slice();
        let scrambled = if self.jk2.is_some() { netchan::encode_jk2_client_message(&mut data, self.challenge, key_text) }
            else { netchan::encode_client_message(&mut data, self.challenge, key_text) };
        if let Err(error) = scrambled {
            self.drop_connection(format!("CL_Netchan_Encode: {error}"));
            return;
        }
        let packet_num = outgoing_sequence as usize & (PACKET_BACKUP - 1);
        self.out_packets[packet_num] = OutPacket {
            realtime,
            // CL_WritePacket: p_serverTime is the newest command's time even when this packet
            // carries no new commands. Taking it from `commands` made empty packets (common at a
            // high cl_maxpackets) record 0, so the ping search matched the newest packet and
            // reported the time since the last send (1-5 ms) instead of the round trip.
            server_time: if self.cmd_number > 0 {
                self.cmds[self.cmd_number as usize & (CMD_BACKUP - 1)].server_time
            } else {
                0
            },
            cmd_number: self.cmd_number,
            reliable_sequence: self.reliable_sequence,
        };
        self.last_packet_sent_time = realtime;
        let chan = self.netchan.as_mut().expect("checked above");
        let datagrams = chan.transmit(&data);
        self.packet_debug.packets += 1;
        self.packet_debug.empty_packets += u64::from(count == 0);
        self.packet_debug.command_transmissions += count as u64;
        self.packet_debug.last_sequence = outgoing_sequence;
        self.packet_debug.last_realtime = realtime;
        self.packet_debug.last_command_count = count;
        self.packet_debug.first_command_time = commands.first().map_or(0, |cmd| cmd.server_time);
        self.packet_debug.last_command_time = commands.last().map_or(0, |cmd| cmd.server_time);
        self.outgoing.extend(datagrams);
    }

    /// CL_Disconnect's network part: queue "disconnect" three times.
    pub fn disconnect(&mut self, realtime: i32) {
        if self.state >= ConnectionState::Connected {
            let _ = self.add_reliable_command(b"disconnect", true);
            for _ in 0..3 {
                self.write_packet(realtime);
            }
        }
        self.state = ConnectionState::Disconnected;
        self.netchan = None;
    }

    /// CL_SetCGameTime (non-demo). Returns cl.serverTime once active.
    pub fn set_cgame_time(&mut self, realtime: i32, time_nudge: i32) -> Option<i32> {
        if self.state != ConnectionState::Active {
            if self.state != ConnectionState::Primed {
                return None;
            }
            if self.new_snapshots {
                self.new_snapshots = false;
                // CL_FirstSnapshot ignores snapshots without entities.
                if self.snap_flags & SNAPFLAG_NOT_ACTIVE == 0 {
                    self.set_state(ConnectionState::Active);
                    self.server_time_delta = self.snap_server_time - realtime;
                    self.old_server_time = self.snap_server_time;
                }
            }
            if self.state != ConnectionState::Active {
                return None;
            }
        }
        if !self.snap_valid {
            self.drop_connection("CL_SetCGameTime: !cl.snap.valid".into());
            return None;
        }
        if self.snap_server_time < self.old_frame_server_time {
            self.drop_connection("cl.snap.serverTime < cl.oldFrameServerTime".into());
            return None;
        }
        self.old_frame_server_time = self.snap_server_time;
        let time_nudge = time_nudge.clamp(-900, 900);
        self.server_time = realtime + self.server_time_delta - time_nudge;
        if self.server_time < self.old_server_time {
            self.server_time = self.old_server_time;
        }
        self.old_server_time = self.server_time;
        if realtime + self.server_time_delta >= self.snap_server_time - 5 {
            self.extrapolated_snapshot = true;
        }
        if self.new_snapshots {
            self.adjust_time_delta(realtime);
        }
        Some(self.server_time)
    }

    fn adjust_time_delta(&mut self, realtime: i32) {
        self.new_snapshots = false;
        let new_delta = self.snap_server_time - realtime;
        let delta_delta = (new_delta - self.server_time_delta).abs();
        if delta_delta > RESET_TIME {
            self.server_time_delta = new_delta;
            self.old_server_time = self.snap_server_time;
            self.server_time = self.snap_server_time;
        } else if delta_delta > 100 {
            self.server_time_delta = (self.server_time_delta + new_delta) >> 1;
        } else if self.extrapolated_snapshot {
            self.extrapolated_snapshot = false;
            self.server_time_delta -= 2;
        } else {
            self.server_time_delta += 1;
        }
    }
}

/// Info_SetValueForKey: refuse `\ ; "`, remove any existing key, then
/// prepend `\key\value` (empty values only remove). Returns false if refused.
pub fn set_info_value(info: &mut Vec<u8>, key: &[u8], value: &[u8]) -> bool {
    if key.iter().chain(value).any(|b| matches!(b, b'\\' | b';' | b'"')) {
        return false;
    }
    let mut pairs: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut fields = info.split(|&b| b == b'\\');
    if info.first() == Some(&b'\\') {
        let _ = fields.next();
    }
    while let (Some(k), Some(v)) = (fields.next(), fields.next()) {
        if !k.is_empty() && !k.eq_ignore_ascii_case(key) {
            pairs.push((k.to_vec(), v.to_vec()));
        }
    }
    info.clear();
    if !value.is_empty() {
        info.push(b'\\');
        info.extend_from_slice(key);
        info.push(b'\\');
        info.extend_from_slice(value);
    }
    for (k, v) in pairs {
        info.push(b'\\');
        info.extend_from_slice(&k);
        info.push(b'\\');
        info.extend_from_slice(&v);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adaptive_huffman;

    fn addr() -> SocketAddr { "10.0.0.1:29070".parse().unwrap() }

    const USERINFO: &[u8] = br"\name\Padawan\rate\25000\snaps\40\model\kyle/default\forcepowers\7-1-032330000000001333\color1\4\color2\4\handicap\100\sex\male\saber1\single_1\saber2\none";

    fn active_session() -> ClientSession {
        let mut session = ClientSession::connect(addr(), USERINFO.to_vec(), 4321, 777, 0);
        session.take_outgoing();
        session.state = ConnectionState::Active;
        session.netchan = Some(Netchan::new(4321));
        session
    }

    #[test]
    fn commandsize_window_limits_prediction_and_preserves_ring_indices_on_resize() {
        let mut session = active_session();
        // The default mirrors the cl_commandsize minimum, not the 512-slot
        // backing capacity. Both directions of a live AUTO change must work.
        assert_eq!(session.command_window(), 64);
        for time in 1..=140 {
            session.create_command_at(UserCmd::default(), time);
        }
        assert!(session.command(140).is_some());
        assert!(session.command(77).is_some(), "newest 64 commands remain visible");
        assert!(session.command(76).is_none(), "older commands are not exposed to prediction");
        session.set_command_window(100); // snap upward, just like the cvar setter
        assert_eq!(session.command_window(), 128);
        assert_eq!(session.command(76).unwrap().server_time, 76);
        session.set_command_window(512);
        assert_eq!(session.command(20).unwrap().server_time, 20);
        session.set_command_window(1);
        assert_eq!(session.command_window(), 64);
        assert!(session.command(76).is_none());
    }

    #[test]
    fn commandsize_change_does_not_corrupt_wire_command_selection() {
        let mut session = active_session();
        for time in 1..=580 {
            session.create_command_at(UserCmd::default(), time);
        }
        session.set_command_window(128);
        session.send_commands(1000, 1000);
        let debug = session.packet_debug();
        assert_eq!(debug.last_command_count, MAX_PACKET_USERCMDS);
        assert_eq!(debug.first_command_time, 580 - MAX_PACKET_USERCMDS as i32 + 1);
        assert_eq!(debug.last_command_time, 580);
    }

    #[test]
    fn high_packet_cap_preserves_command_redundancy_between_app_ticks() {
        let mut session = active_session();
        session.create_command_at(UserCmd::default(), 1000);
        session.send_commands(1000, 1000);
        assert_eq!(session.take_outgoing().len(), 1);
        assert_eq!(session.packet_debug().last_command_count, 1);
        for time in 1001..1017 {
            session.send_commands(time, 1000);
        }
        assert!(session.take_outgoing().is_empty());
        session.create_command_at(UserCmd::default(), 1017);
        session.send_commands(1017, 1000);
        assert_eq!(session.take_outgoing().len(), 1);
        let debug = session.packet_debug();
        assert_eq!(debug.last_command_count, 2, "previous command remains recoverable");
        assert_eq!(debug.first_command_time, 1000);
        assert_eq!(debug.last_command_time, 1017);
        assert_eq!(debug.empty_packets, 0);
    }

    #[test]
    fn lower_packet_cap_batches_commands_without_changing_their_times() {
        let mut session = active_session();
        for time in [1000, 1008, 1016] {
            session.create_command_at(UserCmd::default(), time);
            session.send_commands(time, 60);
        }
        assert_eq!(session.take_outgoing().len(), 2);
        let debug = session.packet_debug();
        assert_eq!(debug.last_command_count, 3);
        assert_eq!(debug.first_command_time, 1000);
        assert_eq!(debug.last_command_time, 1016);
        assert_eq!(session.cmd_number(), 3);
    }

    #[test]
    fn reliable_commands_and_idle_keepalive_do_not_need_a_new_usercmd() {
        let mut session = active_session();
        session.create_command_at(UserCmd::default(), 1000);
        session.send_commands(1000, 1000);
        session.take_outgoing();
        session.add_reliable_command(b"score", false).unwrap();
        session.send_commands(1001, 1000);
        assert_eq!(session.take_outgoing().len(), 1);
        session.send_commands(1002, 1000);
        assert!(session.take_outgoing().is_empty());
        session.write_packet_now(1002);
        assert_eq!(session.take_outgoing().len(), 1, "explicit reliable flush remains immediate");
        session.send_commands(2002, 1000);
        assert_eq!(session.take_outgoing().len(), 1, "retain an idle keepalive");
    }

    #[test]
    fn handshake_follows_openjk_order() {
        let mut session = ClientSession::connect(addr(), USERINFO.to_vec(), 4321, 777, 0);
        assert_eq!(
            session.take_outgoing(),
            vec![
                b"\xff\xff\xff\xffgetchallenge 777".to_vec(),
                b"\xff\xff\xff\xffgetinfo 00000309".to_vec(),
            ]
        );
        // Nothing is resent before RETRANSMIT_TIMEOUT.
        session.frame(2999, 200_000);
        assert!(session.take_outgoing().is_empty());
        session.frame(3000, 200_000);
        assert_eq!(session.take_outgoing().len(), 2);

        // Wrong source and mismatched echo are ignored.
        let other: SocketAddr = "10.0.0.2:29070".parse().unwrap();
        session.packet_event(other, b"\xff\xff\xff\xffchallengeResponse 5 1", 3100);
        assert_eq!(session.state(), ConnectionState::Connecting);

        session.packet_event(addr(), b"\xff\xff\xff\xffchallengeResponse -12345 777", 3100);
        assert_eq!(session.state(), ConnectionState::Challenging);
        let connect = session.take_outgoing();
        assert_eq!(connect.len(), 1);
        let plain = adaptive_huffman::decompress(&connect[0], 12, 16384);
        let text = String::from_utf8_lossy(&plain[12..]).into_owned();
        assert!(text.starts_with("\"\\challenge\\-12345\\qport\\4321\\protocol\\26\\name\\Padawan"), "{text}");
        assert!(text.ends_with("\\saber2\\none\""), "{text}");

        session.packet_event(addr(), b"\xff\xff\xff\xffconnectResponse", 3200);
        assert_eq!(session.state(), ConnectionState::Connected);
        let first = session.take_outgoing();
        assert_eq!(first.len(), 1);
        // sequence 1, qport 4321.
        assert_eq!(&first[0][..6], &[1, 0, 0, 0, 0xe1, 0x10]);
        // While loading, at most one packet per second.
        session.send_commands(3300, 30);
        assert!(session.take_outgoing().is_empty());
        session.send_commands(4200, 30);
        assert_eq!(session.take_outgoing().len(), 1);
    }

    #[test]
    fn jk2_handshake_selects_protocol_and_keeps_wire_keys_before_presentation() {
        for version in [crate::jk2::Version::V102, crate::jk2::Version::V104] {
            let mut session = ClientSession::connect_jk2_preflight(addr(), USERINFO.to_vec(), 4321, 777, 0, version);
            session.take_outgoing();
            session.packet_event(addr(), b"\xff\xff\xff\xffchallengeResponse -12345", 10);
            assert!(session.take_outgoing().is_empty());
            session.resume_connect(20);
            let outgoing = session.take_outgoing();
            let plain = adaptive_huffman::decompress(&outgoing[0], 12, 16384);
            assert!(plain.windows(12).any(|s| s == format!("\\protocol\\{}", version.protocol()).as_bytes()));
            session.packet_event(addr(), b"\xff\xff\xff\xffconnectResponse", 30);
            session.take_outgoing();
            let command = ServerCommand { sequence: 8, text: b"cs 544 \"n\\Changed\\model\\kyle/blue\"".to_vec() };
            session.take_events();
            session.server_command(command.clone());
            assert_eq!(session.server_commands[8], command.text);
            assert!(session.take_events().iter().any(|e| matches!(e,
                SessionEvent::ServerCommand(c) if c.sequence == 8 && c.text.starts_with(b"cs 1131 "))));
            assert!(session.add_reliable_command(b"team red", false).is_err());
            assert!(session.add_reliable_command(b"team spectator", false).is_ok());
            session.set_primed(40);
            let number = session.create_command(UserCmd { weapon: 4, generic_command: 7, forward_move: 127, ..Default::default() }).unwrap();
            let cmd = session.command(number).unwrap();
            assert_eq!(cmd.weapon, 0);
            assert_eq!(cmd.generic_command, 0);
            assert_eq!(cmd.forward_move, 127);
        }
    }

    #[test]
    fn jk2_bootstrap_snapshot_is_acknowledged_before_the_first_gamestate() {
        use crate::message::{MessageReader, MessageWriter};
        for version in [crate::jk2::Version::V102, crate::jk2::Version::V104] {
            let mut session = ClientSession::connect_jk2_preflight(addr(), USERINFO.to_vec(), 4321, 777, 0, version);
            session.take_outgoing();
            session.packet_event(addr(), b"\xff\xff\xff\xffchallengeResponse -12345", 10);
            session.resume_connect(20);
            session.take_outgoing();
            session.packet_event(addr(), b"\xff\xff\xff\xffconnectResponse", 30);
            session.take_outgoing(); session.take_events();
            let send = |session: &mut ClientSession, sequence: i32, mut payload: Vec<u8>| {
                // Server XOR is symmetric; there are no prior client commands.
                netchan::decode_jk2_server_message(&mut payload, sequence, -12345, |_| b"").unwrap();
                let mut datagram = sequence.to_le_bytes().to_vec(); datagram.extend(payload);
                session.packet_event(addr(), &datagram, 30 + sequence);
            };
            let mut w = MessageWriter::new(16384).unwrap();
            for (value, bits) in [(0,32), (7,8), (1000,32), (0,8), (2,8), (0,8), (0,8), (0,1), (1023,10), (9,8)] {
                w.write_bits(value, bits).unwrap();
            }
            send(&mut session, 1, w.as_bytes().to_vec());
            assert_eq!(session.state(), ConnectionState::Connected);
            assert!(!session.gamestate_received);
            assert!(!session.snap_valid);
            assert!(session.decoder().latest_snapshot().is_none());
            assert_eq!(session.server_message_sequence(), 1);
            assert!(session.take_events().is_empty());
            session.write_packet_now(40);
            let outgoing = session.take_outgoing();
            let mut payload = outgoing.last().unwrap()[6..].to_vec();
            netchan::encode_jk2_client_message(&mut payload, -12345, |_| b"").unwrap();
            let mut reader = MessageReader::new(&payload).unwrap();
            assert_eq!(reader.read_long().unwrap(), 0); // no server ID yet
            assert_eq!(reader.read_long().unwrap(), 1); // early packet acknowledged

            let mut w = MessageWriter::new(16384).unwrap();
            w.write_bits(0,32).unwrap(); w.write_bits(2,8).unwrap(); w.write_bits(0,32).unwrap();
            for (index, text) in [(0, format!("\\protocol\\{}\\version\\JK2MP: v{}\\mapname\\ffa_bespin", version.protocol(), version.name())),
                (1, "\\sv_pure\\0\\sv_serverid\\99".into())] {
                w.write_bits(3,8).unwrap(); w.write_bits(index,16).unwrap();
                netchan::write_string(&mut w, text.as_bytes()).unwrap();
            }
            w.write_bits(9,8).unwrap(); w.write_bits(0,32).unwrap(); w.write_bits(42,32).unwrap(); w.write_bits(9,8).unwrap();
            send(&mut session, 2, w.as_bytes().to_vec());
            assert_eq!(session.state(), ConnectionState::Connected);
            assert!(session.gamestate_received);
            assert_eq!(session.server_id, 99);
            assert!(session.take_events().iter().any(|event| matches!(event, SessionEvent::Gamestate)));
        }
    }

    #[test]
    fn jk2_preflight_rejects_protocol_mismatch_before_connect() {
        let mut session = ClientSession::connect_jk2_preflight(addr(), USERINFO.to_vec(), 1, 777, 0, crate::jk2::Version::V104);
        session.take_outgoing();
        session.packet_event(addr(), b"\xff\xff\xff\xffinfoResponse\n\\challenge\\00000309\\protocol\\15", 10);
        assert_eq!(session.state(), ConnectionState::Disconnected);
        assert!(session.take_outgoing().is_empty());
    }

    #[test]
    fn jk2_pure_gamestate_reaches_owner_without_sending_jka_checksums() {
        let mut session = ClientSession::connect_jk2_preflight(addr(), USERINFO.to_vec(), 1, 777, 0, crate::jk2::Version::V104);
        session.take_outgoing();
        session.state = ConnectionState::Connected;
        session.take_events();
        let mut w = crate::message::MessageWriter::new(16384).unwrap();
        w.write_bits(0, 32).unwrap();
        w.write_bits(2, 8).unwrap(); // JK2 svc_gamestate
        w.write_bits(0, 32).unwrap();
        for (index, text) in [(0, b"\\protocol\\16\\version\\JK2MP: v1.04\\mapname\\ffa_bespin".as_slice()), (1, b"\\sv_pure\\1\\sv_serverid\\99")] {
            w.write_bits(3, 8).unwrap();
            w.write_bits(index, 16).unwrap();
            netchan::write_string(&mut w, text).unwrap();
        }
        w.write_bits(9, 8).unwrap(); // JK2 svc_EOF inside gamestate
        w.write_bits(0, 32).unwrap();
        w.write_bits(42, 32).unwrap();
        w.write_bits(9, 8).unwrap();
        session.parse_server_message(1, w.as_bytes(), 100);
        assert_eq!(session.state(), ConnectionState::Connected);
        let events = session.take_events();
        assert!(events.iter().any(|e| matches!(e, SessionEvent::Gamestate)));
        assert!(session.sv_pure());
        assert_eq!(session.reliable_sequence, 1);
        assert_eq!(session.reliable_commands[1], b"team spectator");
        assert!(session.take_outgoing().is_empty());
    }

    #[test]
    fn server_rejection_before_gamestate_ends_join_with_exact_reason() {
        let mut session = ClientSession::connect(addr(), USERINFO.to_vec(), 4321, 777, 0);
        session.take_outgoing();
        session.packet_event(addr(), b"\xff\xff\xff\xffchallengeResponse -12345 777", 10);
        session.take_events();
        session.packet_event(addr(), b"\xff\xff\xff\xffprint\nToo many connections from the same IP\n", 20);
        assert_eq!(session.state(), ConnectionState::Disconnected);
        assert!(session.take_events().iter().any(|event| matches!(event,
            SessionEvent::Disconnected(reason) if reason == "Too many connections from the same IP"
        )));
        session.frame(40_000, 200_000);
        assert!(session.take_outgoing().is_empty(), "a refused connection must not retry");
    }

    #[test]
    fn unsuccessful_join_times_out_at_each_handshake_stage() {
        let mut challenge = ClientSession::connect(addr(), USERINFO.to_vec(), 4321, 777, 0);
        challenge.take_outgoing();
        challenge.frame(JOIN_TIMEOUT, 200_000);
        assert_eq!(challenge.state(), ConnectionState::Disconnected);
        assert!(challenge.take_events().iter().any(|event| matches!(event,
            SessionEvent::Disconnected(reason) if reason.contains("awaiting challenge")
        )));

        let mut connect = ClientSession::connect(addr(), USERINFO.to_vec(), 4321, 777, 0);
        connect.packet_event(addr(), b"\xff\xff\xff\xffchallengeResponse -12345 777", 10);
        connect.frame(JOIN_TIMEOUT, 200_000);
        assert_eq!(connect.state(), ConnectionState::Disconnected);

        let mut gamestate = ClientSession::connect(addr(), USERINFO.to_vec(), 4321, 777, 0);
        gamestate.packet_event(addr(), b"\xff\xff\xff\xffchallengeResponse -12345 777", 10);
        gamestate.packet_event(addr(), b"\xff\xff\xff\xffconnectResponse", 20);
        gamestate.frame(20 + JOIN_TIMEOUT, 200_000);
        assert_eq!(gamestate.state(), ConnectionState::Disconnected);
        assert!(gamestate.take_events().iter().any(|event| matches!(event,
            SessionEvent::Disconnected(reason) if reason.contains("awaiting gamestate")
        )));
    }

    #[test]
    fn preflight_prompt_does_not_time_out_while_waiting_for_user() {
        let mut session = ClientSession::connect_preflight(addr(), USERINFO.to_vec(), 4321, 777, 0);
        session.packet_event(addr(), b"\xff\xff\xff\xffchallengeResponse -12345 777", 10);
        session.server_info_seen = true;
        session.frame(JOIN_TIMEOUT + 1, 200_000);
        assert_eq!(session.state(), ConnectionState::Challenging);
    }

    #[test]
    fn preflight_pauses_connect_packet_until_resumed() {
        let mut session = ClientSession::connect_preflight(addr(), USERINFO.to_vec(), 4321, 777, 0);
        let _ = session.take_outgoing();
        session.packet_event(addr(), b"\xff\xff\xff\xffchallengeResponse -12345 777", 10);
        assert_eq!(session.state(), ConnectionState::Challenging);
        assert!(session.take_outgoing().is_empty());
        session.resume_connect(20);
        assert_eq!(session.take_outgoing().len(), 1);
    }

    #[test]
    fn speculative_getinfo_surfaces_server_info_once() {
        let mut session = ClientSession::connect(addr(), USERINFO.to_vec(), 4321, 777, 0);
        let _ = session.take_outgoing();
        let response = b"\xff\xff\xff\xffinfoResponse\n\\challenge\\00000309\\protocol\\26\\hostname\\Test\\mapname\\mp/ffa3";
        session.packet_event(addr(), response, 10);
        let events = session.take_events();
        let info = events.into_iter().find_map(|event| match event {
            SessionEvent::ServerInfo(info) => Some(info),
            _ => None,
        }).expect("server info event");
        assert_eq!(info.get(b"mapname"), Some(b"mp/ffa3".as_slice()));

        // Retransmitted/duplicate infoResponse packets do not restart client work.
        session.packet_event(addr(), response, 11);
        assert!(session.take_events().is_empty());
    }

    #[test]
    fn info_values_are_replaced_not_duplicated() {
        let mut info = br"\name\a\qport\1".to_vec();
        assert!(set_info_value(&mut info, b"QPORT", b"9"));
        assert!(set_info_value(&mut info, b"challenge", b"-3"));
        assert_eq!(info, br"\challenge\-3\QPORT\9\name\a");
        assert!(!set_info_value(&mut info, b"name", b"a;quit"));
        assert!(set_info_value(&mut info, b"name", b""));
        assert_eq!(info, br"\challenge\-3\QPORT\9");
    }

    #[test]
    fn reliable_commands_overflow_like_openjk() {
        let mut session = ClientSession::connect(addr(), USERINFO.to_vec(), 1, 1, 0);
        for _ in 0..MAX_RELIABLE_COMMANDS {
            session.add_reliable_command(b"say hi", false).unwrap();
        }
        assert!(session.add_reliable_command(b"say hi", false).is_err());
        assert_eq!(session.state(), ConnectionState::Disconnected);
    }
}
