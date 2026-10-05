//! Demo probe.
use crate::app::{Cursor, DemoReader, ServerMessageDecoder, ServerMessageEvent};

pub(in crate::app) struct DemoGamestateProbe {
    pub(in crate::app) fs_game: Vec<u8>,
    pub(in crate::app) map_name: String,
}

/// Read only as far as the first gamestate, mirroring CL_PlayDemo_f's
/// pre-CA_PRIMED message pump. Besides fs_game, retain the map name so the
/// loading-information screen can become authoritative before CGame assets are
/// built or any old rendered world is replaced.
pub(in crate::app) fn probe_demo_gamestate(bytes: &[u8]) -> Result<DemoGamestateProbe, String> {
    let mut reader = DemoReader::new(Cursor::new(bytes));
    let mut decoder = ServerMessageDecoder::new();
    let mut setgame = Vec::new();
    let mut messages = 0usize;
    loop {
        let record = reader
            .next_record()
            .map_err(|error| format!("DEMO GAMESTATE PROBE FRAMING ERROR: {error}"))?
            .ok_or_else(|| "DEMO REACHED EOF BEFORE GAMESTATE".to_owned())?;
        let packet = decoder
            .parse_packet(record.sequence, &record.payload)
            .map_err(|error| {
                format!(
                    "DEMO GAMESTATE PROBE PROTOCOL ERROR: message {messages} sequence {}: {error}",
                    record.sequence
                )
            })?;
        messages += 1;
        for event in &packet.events {
            match event {
                ServerMessageEvent::SetGame(game) => setgame = game.clone(),
                ServerMessageEvent::Gamestate { .. } => {
                    let system_game = decoder
                        .configstrings
                        .get(&jka_protocol::session::CS_SYSTEMINFO)
                        .and_then(|info| jka_protocol::commands::info_value(info, b"fs_game"))
                        .unwrap_or_default();
                    let fs_game = if system_game.is_empty() {
                        setgame
                    } else {
                        system_game.to_vec()
                    };
                    let map_name = decoder
                        .map_name()
                        .ok_or_else(|| "DEMO GAMESTATE HAS NO MAPNAME".to_owned())?;
                    return Ok(DemoGamestateProbe { fs_game, map_name });
                }
                _ => {}
            }
        }
        if messages >= 4096 {
            return Err("DEMO GAMESTATE PROBE DID NOT REACH A GAMESTATE".to_owned());
        }
    }
}
