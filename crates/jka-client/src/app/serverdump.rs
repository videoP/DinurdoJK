//! `serverdump`: everything useful about the connected server in one place, saved so servers can
//! be compared. Prints to the console, writes `<game>/serverinfo/<addr>_<time>.txt` and appends a
//! one-line summary to `<game>/serverinfo/servers.log`.

use super::*;
use std::{fmt::Write as _, io::Write as _};

/// An info string is `\key\value\key\value` with a leading backslash. Values may be empty
/// (`\fs_game\\`), so empty fields must be kept or every pair after one shifts by a field.
fn parse_info(bytes: &[u8]) -> Vec<(String, String)> {
    let parts: Vec<&[u8]> = bytes
        .split(|&b| b == b'\\')
        .skip(usize::from(bytes.first() == Some(&b'\\')))
        .collect();
    parts
        .chunks(2)
        .filter(|pair| !pair[0].is_empty())
        .map(|pair| {
            (
                String::from_utf8_lossy(pair[0]).into_owned(),
                pair.get(1).map(|value| String::from_utf8_lossy(value).into_owned()).unwrap_or_default(),
            )
        })
        .collect()
}

/// Drop `^7`-style colour codes so a hostname stays readable in a log line.
fn strip_colors(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '^' && chars.peek().is_some_and(|next| next.is_ascii_digit()) {
            chars.next();
        } else {
            out.push(c);
        }
    }
    out
}

impl App {
    pub(super) fn server_dump(&mut self) {
        use jka_protocol::session::ConnectionState;
        let Some(net) = self.net.as_ref().filter(|net| net.state() >= ConnectionState::Connected) else {
            self.push_console_line("^3serverdump: not connected to a server".to_owned());
            return;
        };
        let session = net.session();
        let configstrings = &session.decoder().configstrings;
        let server = parse_info(configstrings.get(&0).map(Vec::as_slice).unwrap_or_default());
        let system = parse_info(configstrings.get(&1).map(Vec::as_slice).unwrap_or_default());
        let value = |key: &str| -> Option<&str> {
            server
                .iter()
                .chain(system.iter())
                .find(|(name, _)| name.eq_ignore_ascii_case(key))
                .map(|(_, value)| value.as_str())
        };
        let or_dash = |key: &str| value(key).unwrap_or("-").to_owned();

        let address = session.server();
        let stats = session.snapshot_stats();
        let command_lag = session.snapshot_command_lag();
        let ping = session.ping();
        let backend = match crate::net::mod_support::ServerMod::detect(crate::net::mod_support::server_info(configstrings)) {
            crate::net::mod_support::ServerMod::Japro => "jaPRO backend",
            _ => "stock backend",
        };

        // sv_fps is a CVAR_TEMP on most servers and not advertised, so estimate it. Snapshots are
        // only built on server frames, so a gap is a whole number of server frame times; the
        // small end of the distribution is the frame time unless our `snaps` rate floors it.
        let snaps = self.network.snaps.max(1);
        let snaps_floor_ms = 1000.0 / snaps as f32;
        let (fps_text, fps_estimate) = if stats.samples >= 8 && stats.p10_ms > 0 {
            let frame_ms = stats.p10_ms as f32;
            let fps = 1000.0 / frame_ms;
            if frame_ms <= snaps_floor_ms + 2.0 {
                (
                    format!("sv_fps >= {fps:.0} (snapshot gaps hit the {snaps_floor_ms:.0} ms floor of your snaps={snaps}; true value may be higher)"),
                    format!(">={fps:.0}"),
                )
            } else {
                (format!("sv_fps ~ {fps:.0} (smallest common snapshot gap {} ms)", stats.p10_ms), format!("~{fps:.0}"))
            }
        } else {
            ("sv_fps unknown (need ~8+ snapshots; try again in a moment)".to_owned(), "?".to_owned())
        };
        let advertised_fps: Vec<String> = server
            .iter()
            .chain(system.iter())
            .filter(|(name, _)| name.to_ascii_lowercase().contains("fps"))
            .map(|(name, value)| format!("{name}={value}"))
            .collect();

        // Crowding: connected client slots from the configstrings, and how many other players are
        // in your snapshot and how close (other players are solid in stock prediction, and at
        // ~120 ms their positions are old, so a crowded server mispredicts more).
        let players_connected = (0..32u16)
            .filter(|slot| {
                configstrings
                    .get(&(crate::cgame::CS_PLAYERS + slot))
                    .is_some_and(|value| !value.is_empty())
            })
            .count();
        const ET_PLAYER: i32 = 1;
        let (others_in_snapshot, nearest_player) = match self.game_session.as_ref() {
            Some(playback) => {
                let viewer = playback.current_snapshot.as_ref().map(|snapshot| {
                    let ps = &snapshot.player_state;
                    (
                        ps.field_i32("clientNum").unwrap_or(-1),
                        [
                            ps.field_f32("origin[0]").unwrap_or(0.0),
                            ps.field_f32("origin[1]").unwrap_or(0.0),
                            ps.field_f32("origin[2]").unwrap_or(0.0),
                        ],
                    )
                });
                match viewer {
                    Some((client, origin)) => {
                        let mut count = 0usize;
                        let mut nearest = f32::INFINITY;
                        for entity in playback
                            .presented_entities
                            .iter()
                            .filter(|entity| entity.entity_type == ET_PLAYER && i32::from(entity.number) != client)
                        {
                            count += 1;
                            let distance = (0..3)
                                .map(|axis| (entity.origin[axis] - origin[axis]).powi(2))
                                .sum::<f32>()
                                .sqrt();
                            nearest = nearest.min(distance);
                        }
                        (count, nearest.is_finite().then_some(nearest))
                    }
                    None => (0, None),
                }
            }
            None => (0, None),
        };
        let nearest_text = nearest_player.map_or_else(|| "-".to_owned(), |distance| format!("{distance:.0}u"));

        // Fingerprint what the server really runs. The `version` string is trivially spoofed, but
        // some serverinfo keys come from the engine itself: OpenJK-lineage engines advertise
        // sv_fps / sv_autoDemo / sv_floodProtectSlow / sv_minRate and a genuine retail 2003 engine
        // does not. Pk3s the server loads but never sends to clients (sv_pakNames minus
        // sv_referencedPakNames) are server-side code or data, i.e. a possible game mod (a QVM or
        // replaced game module is how a "base" server can run modified movement).
        let has_key = |key: &str| value(key).is_some();
        let openjk_keys: Vec<&str> = ["sv_fps", "sv_autoDemo", "sv_floodProtectSlow", "sv_minRate"]
            .into_iter()
            .filter(|key| has_key(key))
            .collect();
        let engine_guess = if openjk_keys.len() >= 3 { "openjk-lineage" } else { "retail-style" };
        let referenced = or_dash("sv_referencedPakNames");
        let server_only_paks: Vec<String> = value("sv_pakNames")
            .unwrap_or_default()
            .split_whitespace()
            .map(|name| name.trim_start_matches("base/").to_owned())
            .filter(|name| {
                !name.to_ascii_lowercase().starts_with("assets")
                    && !referenced.split_whitespace().any(|sent| sent.trim_start_matches("base/") == name)
            })
            .collect();
        let vm_modules = ["vm_game", "vm_cgame", "vm_ui"]
            .into_iter()
            .filter_map(|key| value(key).map(|v| format!("{key}={v}")))
            .collect::<Vec<_>>()
            .join(" ");

        let mut lines: Vec<String> = Vec::new();
        lines.push(format!("serverdump  {address}  {}", strip_colors(&or_dash("sv_hostname"))));
        lines.push(format!(
            "  game: gamename={} map={} gametype={} maxclients={} pure={}  -> {backend}",
            or_dash("gamename"),
            or_dash("mapname"),
            or_dash("g_gametype"),
            or_dash("sv_maxclients"),
            or_dash("sv_pure"),
        ));
        lines.push(format!("  {fps_text}"));
        lines.push(format!(
            "  engine: {engine_guess} (has {openjk_keys:?}); version string \"{}\"{}",
            or_dash("version"),
            if engine_guess == "openjk-lineage" && or_dash("version").contains("2003") {
                "  <-- claims a 2003 retail build but advertises OpenJK engine keys: the version is not genuine"
            } else {
                ""
            },
        ));
        lines.push(format!(
            "  server-only paks (loaded, not sent to clients): {}{}",
            if server_only_paks.is_empty() { "none".to_owned() } else { server_only_paks.join(", ") },
            if vm_modules.is_empty() { String::new() } else { format!("   {vm_modules}") },
        ));
        lines.push(format!(
            "  players: {players_connected} connected slots, {others_in_snapshot} other players in your snapshot, nearest {nearest_text}"
        ));
        if !advertised_fps.is_empty() {
            lines.push(format!("  advertised: {}", advertised_fps.join(" ")));
        }
        lines.push(format!(
            "  snapshots: {} gaps  min {} p10 {} median {} max {} ms  common {:?}  received {:.1}/s",
            stats.samples, stats.min_ms, stats.p10_ms, stats.median_ms, stats.max_ms, stats.common_gaps,
            stats.received_per_second,
        ));
        lines.push(format!(
            "  latency: ping {ping} ms  command lag (server time - last processed cmd) {command_lag} ms"
        ));
        lines.push(format!(
            "  you: rate={} snaps={} cl_maxpackets={} cl_commandRate={} cl_timeNudge={} packetdup={}",
            self.network.rate,
            self.network.snaps,
            self.network.max_packets,
            self.network.command_rate,
            self.network.time_nudge,
            self.network.packet_dup,
        ));
        lines.push(format!(
            "  movement: pmove_fixed={} pmove_float={} pmove_msec={} g_speed={} g_gravity={} g_stepSlideFix={} jcinfo={} restricts={}",
            or_dash("pmove_fixed"),
            or_dash("pmove_float"),
            or_dash("pmove_msec"),
            or_dash("g_speed"),
            or_dash("g_gravity"),
            or_dash("g_stepSlideFix"),
            or_dash("jcinfo"),
            or_dash("restricts"),
        ));
        lines.push("  --- serverinfo ---".to_owned());
        let mut sorted_server = server.clone();
        sorted_server.sort_by_key(|(name, _)| name.to_ascii_lowercase());
        lines.extend(sorted_server.iter().map(|(key, value)| format!("  {key:<24} {value}")));
        lines.push("  --- systeminfo ---".to_owned());
        let mut sorted_system = system.clone();
        sorted_system.sort_by_key(|(name, _)| name.to_ascii_lowercase());
        lines.extend(sorted_system.iter().map(|(key, value)| format!("  {key:<24} {value}")));

        let srvpaks = if server_only_paks.is_empty() { "-".to_owned() } else { server_only_paks.join("+") };
        let timestamp = demo_recording_timestamp();
        let summary = format!(
            "{timestamp} addr={address} host=\"{}\" gamename={} {backend_short} map={} gt={} maxclients={} engine={engine_guess} srvpaks={srvpaks} sv_fps{fps_estimate} players={players_connected} near={nearest_text} snapgap_med={}ms snaps_recv={:.1}/s ping={ping} cmdlag={command_lag} pmove_fixed={} pmove_float={} pmove_msec={} g_speed={} g_gravity={} stepslide={} jcinfo={}",
            strip_colors(&or_dash("sv_hostname")),
            or_dash("gamename"),
            or_dash("mapname"),
            or_dash("g_gametype"),
            or_dash("sv_maxclients"),
            stats.median_ms,
            stats.received_per_second,
            or_dash("pmove_fixed"),
            or_dash("pmove_float"),
            or_dash("pmove_msec"),
            or_dash("g_speed"),
            or_dash("g_gravity"),
            or_dash("g_stepSlideFix"),
            or_dash("jcinfo"),
            backend_short = if backend.starts_with("jaPRO") { "japro" } else { "stock" },
        );

        for line in &lines {
            self.push_console_line(line.clone());
        }
        self.push_console_line(format!("^3SERVER-SUMMARY^7 {summary}"));

        let safe_address: String = address.to_string().chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' { c } else { '_' }).collect();
        let directory = self.game_write_path("serverinfo");
        let path = directory.join(format!("{safe_address}_{timestamp}.txt"));
        let mut report = String::new();
        for line in &lines {
            let _ = writeln!(report, "{line}");
        }
        let result = std::fs::create_dir_all(&directory)
            .and_then(|()| std::fs::write(&path, report))
            .and_then(|()| {
                let mut log = std::fs::OpenOptions::new().create(true).append(true).open(directory.join("servers.log"))?;
                writeln!(log, "{summary}")
            });
        match result {
            Ok(()) => self.push_console_path_line(
                format!("^2serverdump: wrote {} and appended to servers.log", path.display()),
                path,
            ),
            Err(error) => self.push_console_line(format!("^1serverdump: couldn't write files: {error}")),
        }
    }
}
