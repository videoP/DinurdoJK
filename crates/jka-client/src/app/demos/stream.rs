//! Demo stream.
use crate::app::{
    DemoCameraSample, DemoTimeline, GameSession, Instant, ProtocolSnapshot, ServerMessageEvent,
    SessionPhase,
};

impl GameSession {
    pub(in crate::app) fn read_next_message(&mut self) -> Result<Option<ProtocolSnapshot>, String> {
        let record = match self.reader.next_record() {
            Ok(Some(record)) => record,
            Ok(None) => {
                self.eof = true;
                if !self.seeking_demo {
                    devprintln!(
                        1,
                        "DEMO EOF: {} messages, {} valid snapshots",
                        self.messages,
                        self.snapshots
                    );
                }
                return Ok(None);
            }
            Err(error) => {
                return Err(format!(
                    "DEMO FRAMING ERROR AFTER MESSAGE {}: {error}",
                    self.messages
                ));
            }
        };

        let message_index = self.messages;
        let sequence = record.sequence;
        let packet = self.decoder.parse_packet(sequence, &record.payload).map_err(|error| {
            format!(
                "DEMO PROTOCOL ERROR: message {message_index} sequence {sequence} byte~{} bit {}: {}",
                error.bit / 8,
                error.bit,
                error
            )
        })?;
        self.messages += 1;

        let mut decoded_snapshot = None;
        for event in &packet.events {
            match event {
                ServerMessageEvent::Nop => {}
                ServerMessageEvent::Gamestate {
                    server_command_sequence,
                    configstrings,
                    baselines,
                    client_number,
                    checksum_feed,
                } => {
                    if !self.seeking_demo {
                        devprintln!(
                            2,
                            "DEMO svc_gamestate: sequence={} serverCommandSequence={} configstrings={} baselines={} clientNum={} checksumFeed=0x{:08x}",
                            sequence,
                            server_command_sequence,
                            configstrings,
                            baselines,
                            client_number,
                            checksum_feed
                        );
                    }
                    if let Some(map) = self.decoder.map_name() {
                        if self.map_name.as_deref() != Some(map.as_str()) {
                            if !self.seeking_demo {
                                devprintln!(1, "DEMO map: {map}");
                            }
                        }
                        self.map_name = Some(map);
                    }
                    self.client_game
                        .reset_gamestate(&self.decoder.configstrings, *server_command_sequence);
                    self.event_presenter.clear();
                    if let Some(sound) = &mut self.sound_presenter {
                        sound.clear();
                    }
                }
                ServerMessageEvent::ServerCommand(command) => {
                    self.client_game.queue_server_command(command.clone());
                    if !self.seeking_demo {
                        devprintln!(
                            2,
                            "DEMO svc_serverCommand #{}: {}",
                            command.sequence,
                            String::from_utf8_lossy(&command.text)
                        );
                    }
                }
                ServerMessageEvent::Snapshot {
                    server_time,
                    message_num,
                    delta_num,
                    entities,
                } => {
                    self.snapshots += 1;
                    if !self.seeking_demo && (self.snapshots == 1 || self.snapshots % 60 == 0) {
                        devprintln!(
                            3,
                            "DEMO svc_snapshot: serverTime={} message={} delta={} entities={} decodedSnapshots={}",
                            server_time,
                            message_num,
                            delta_num,
                            entities,
                            self.snapshots
                        );
                    }
                    decoded_snapshot = self.decoder.latest_snapshot().cloned();
                }
                ServerMessageEvent::SetGame(game) => {
                    if !self.seeking_demo {
                        devprintln!(2, "DEMO svc_setgame: {}", String::from_utf8_lossy(game));
                    }
                }
                ServerMessageEvent::MapChange => {
                    if !self.seeking_demo {
                        devprintln!(2, "DEMO svc_mapchange");
                    }
                }
                ServerMessageEvent::Download(_) => {
                    if !self.seeking_demo {
                        devprintln!(2, "DEMO svc_download");
                    }
                }
            }
        }

        Ok(decoded_snapshot)
    }

    /// CG_ReadNextSnapshot: every snapshot cgame reads is logged for the lagometer.
    pub(in crate::app) fn read_next_snapshot(
        &mut self,
    ) -> Result<Option<ProtocolSnapshot>, String> {
        let snapshot = self.read_next_snapshot_unlogged()?;
        if let Some(snapshot) = &snapshot {
            // Demos carry no ping: cgame derives one from the serverinfo's sv_fps.
            let demo = !self.live;
            let svfps = if demo {
                self.client_game.server_fps()
            } else {
                20
            };
            self.lagometer.add_snapshot_info(
                &crate::lagometer::SnapshotInfo {
                    message_num: snapshot.message_num,
                    server_time: snapshot.server_time,
                    command_time: snapshot.player_state.field_i32("commandTime").unwrap_or(0),
                    ping: snapshot.ping,
                    flags: snapshot.snap_flags,
                },
                demo,
                svfps,
            );
        }
        Ok(snapshot)
    }

    pub(in crate::app) fn read_next_snapshot_unlogged(
        &mut self,
    ) -> Result<Option<ProtocolSnapshot>, String> {
        if self.live {
            // CG_ReadNextSnapshot: nothing new yet simply means extrapolate.
            return Ok(self.live_snapshots.pop_front());
        }
        loop {
            if self.eof {
                return Ok(None);
            }
            if let Some(snapshot) = self.read_next_message()? {
                return Ok(Some(snapshot));
            }
        }
    }

    pub(in crate::app) fn begin_after_map_load(
        &mut self,
        now: Instant,
    ) -> Result<DemoCameraSample, String> {
        const SNAPFLAG_NOT_ACTIVE: u8 = 1 << 1;

        // CL_FirstSnapshot ignores connection/zombie snapshots carrying
        // SNAPFLAG_NOT_ACTIVE. Do the same before establishing our playback time
        // base so the first visible frame is an actually active game snapshot.
        let first = loop {
            let candidate = if let Some(snapshot) = self.pending_snapshot.take() {
                Some(snapshot)
            } else {
                self.read_next_snapshot()?
            };
            let snapshot =
                candidate.ok_or_else(|| format!("DEMO HAS NO ACTIVE SNAPSHOT: {}", self.qpath))?;
            if snapshot.snap_flags & SNAPFLAG_NOT_ACTIVE == 0 {
                break snapshot;
            }
        };
        self.first_server_time = Some(first.server_time);
        self.client_game.set_initial_snapshot(&first)?;
        self.current_snapshot = Some(first);
        self.next_snapshot = self.read_next_snapshot()?;
        self.client_game
            .set_next_snapshot(self.next_snapshot.as_ref())?;
        self.timeline = Some(DemoTimeline::new(self.first_server_time.unwrap(), now));
        self.phase = SessionPhase::Playing;
        self.sample_at(
            f64::from(self.first_server_time.unwrap()),
            self.first_server_time.unwrap(),
            false,
        )
        .ok_or_else(|| "DEMO FIRST SNAPSHOT HAS NO CAMERA PLAYERSTATE".to_owned())
    }

    /// Live CG_Init/CG_ProcessSnapshots start: begin from the newest active
    /// snapshot received while the map loaded. Returns false until one exists.
    pub(in crate::app) fn begin_live(&mut self) -> Result<bool, String> {
        const SNAPFLAG_NOT_ACTIVE: u8 = 1 << 1;
        let Some(index) = self
            .live_snapshots
            .iter()
            .rposition(|snapshot| snapshot.snap_flags & SNAPFLAG_NOT_ACTIVE == 0)
        else {
            // Inactive snapshots only mean the server has not entered us yet.
            let keep_from = self.live_snapshots.len().saturating_sub(1);
            self.live_snapshots.drain(..keep_from);
            return Ok(false);
        };
        self.live_snapshots.drain(..index);
        let first = self.live_snapshots.pop_front().expect("indexed above");
        self.first_server_time = Some(first.server_time);
        self.client_game.set_initial_snapshot(&first)?;
        self.current_snapshot = Some(first);
        self.next_snapshot = self.read_next_snapshot()?;
        self.client_game
            .set_next_snapshot(self.next_snapshot.as_ref())?;
        self.phase = SessionPhase::Playing;
        Ok(true)
    }
}
