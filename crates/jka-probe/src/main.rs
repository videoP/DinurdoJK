use std::env;
use std::fs::File;
use std::io::BufReader;
use std::net::SocketAddrV4;
use std::process::ExitCode;
use std::time::Duration;

fn inspect_map(path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    use jka_assets::bsp::{Bsp, SurfaceKind, MAX_FILE_BYTES};
    use std::io::Read;
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    let map = Bsp::parse(&bytes)?;
    let mesh = map.world_mesh(4)?;
    let spawns = map.deathmatch_spawns();
    let patches = map
        .surfaces
        .iter()
        .filter(|s| s.kind == SurfaceKind::Patch)
        .count();
    println!("Map: {}\nRBSP version 1: {} bytes\nShaders: {}\nModels: {}\nSurfaces: {} ({} patches)\nSource vertices: {}\nWorld mesh: {} vertices, {} triangles, {} batches\nLightmap pages: {}\nBrushes: {}\nPlanes: {}\nEntities: {}\nFFA spawn candidates: {}",
        path.display(), bytes.len(), map.shaders.len(), map.models.len(), map.surfaces.len(), patches,
        map.vertices.len(), mesh.vertices.len(), mesh.indices.len() / 3, mesh.batches.len(),
        map.lightmaps.len() / (128 * 128 * 3), map.brushes.len(), map.planes.len(), map.entities.len(), spawns.len());
    if let Some(spawn) = spawns.first() {
        println!("First spawn: {:?}, yaw {}", spawn.origin, spawn.yaw);
    }
    println!("Geometry loaded. Rendering, collision traces and walking are not implemented yet.");
    Ok(())
}

fn imported_map(name: &str) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    if name.is_empty()
        || name.split('/').any(|part| {
            part.is_empty()
                || !part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
    {
        return Err("map name must look like mp/ffa3, without an extension".into());
    }
    let executable = env::current_exe()?;
    let target = executable
        .parent()
        .and_then(std::path::Path::parent)
        .ok_or("cannot locate development assets")?;
    let path = target
        .join("dev-assets")
        .join("maps")
        .join(name)
        .with_extension("bsp");
    if !path.is_file() {
        return Err(format!(
            "map is not imported; run scripts/import-map.ps1 -Map {name}, or use map <path.bsp>"
        )
        .into());
    }
    Ok(path)
}

fn help() {
    println!("JKA Rust diagnostic console\n  help                         Show commands\n  <IPv4:port>                  Query a server\n  demo <path.dm_26>            Inspect demo framing\n  gamestate <path.dm_26>       Decode initial game state\n  gamestate-dump <path.dm_26>  Dump state for reference comparison\n  map <path.bsp>               Load RBSP geometry\n  mapinfo mp/ffa3              Load a development-imported map\n  quit                         Close console\n\nCLI: jka-probe <command> [argument]\nNo arguments opens this console. Paths with spaces may be quoted.\nThis build has no renderer, walking, or live game session yet.");
}

fn console() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;
    help();
    let stdin = std::io::stdin();
    loop {
        print!("jka> ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        if stdin.read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "quit" || line == "exit" {
            break;
        }
        let (command, argument) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let argument = argument.trim();
        let argument = argument
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(argument);
        let mut args = vec![command.to_string()];
        if !argument.is_empty() {
            args.push(argument.to_string());
        }
        if let Err(error) = dispatch(&args) {
            eprintln!("jka-probe: {error}");
        }
    }
    Ok(())
}

fn inspect_gamestate(path: &str, dump: bool) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;
    let mut reader = jka_protocol::demo::DemoReader::new(BufReader::new(File::open(path)?));
    let record = reader.next_record()?.ok_or("demo has no messages")?;
    let state = jka_protocol::gamestate::decode_initial_gamestate(&record.payload)?;
    if dump {
        let mut out = std::io::BufWriter::new(std::io::stdout().lock());
        writeln!(
            out,
            "meta\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            record.sequence,
            state.reliable_acknowledge,
            state.server_command_sequence,
            state.client_number,
            state.checksum_feed,
            state.consumed_bits,
            state.consumed_bytes
        )?;
        for command in &state.preceding_commands {
            write!(out, "cmd\t{}\t", command.sequence)?;
            for byte in &command.text {
                write!(out, "{byte:02x}")?;
            }
            writeln!(out)?;
        }
        for (index, text) in &state.configstrings {
            write!(out, "cs\t{index}\t")?;
            for byte in text {
                write!(out, "{byte:02x}")?;
            }
            writeln!(out)?;
        }
        for (number, entity) in &state.baselines {
            write!(out, "base\t{number}")?;
            for value in entity.fields {
                write!(out, "\t{value:08x}")?;
            }
            writeln!(out)?;
        }
        out.flush()?;
    } else {
        println!("Initial message sequence: {}\nConfigstrings: {}\nEntity baselines: {}\nClient number: {}\nChecksum feed: {}\nConsumed: {} bits, {} / {} bytes",
            record.sequence, state.configstrings.len(), state.baselines.len(), state.client_number,
            state.checksum_feed, state.consumed_bits, state.consumed_bytes, record.payload.len());
        println!("Initial gamestate only; subsequent snapshots have not been decoded.");
    }
    Ok(())
}

/// Headless live-session check: handshake, gamestate, "primed" without a map,
/// idle usercmds, optional reliable commands, then a clean disconnect.
fn live_connect(target: &str, seconds: u64, commands: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use jka_protocol::{netchan::UserCmd, session::{ClientSession, ConnectionState, SessionEvent}};
    use std::net::{ToSocketAddrs, UdpSocket};
    use std::time::Instant;

    let with_port = if target.contains(':') { target.to_owned() } else { format!("{target}:{}", jka_protocol::DEFAULT_PORT) };
    let server = with_port.to_socket_addrs()?.find(|a| a.is_ipv4()).ok_or("no IPv4 address")?;
    println!("{target} resolved to {server}");
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_read_timeout(Some(Duration::from_millis(5)))?;
    let start = Instant::now();
    let now = || start.elapsed().as_millis() as i32;
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.subsec_nanos();
    let userinfo = br"\name\DinurdoProbe\rate\25000\snaps\40\model\kyle/default\forcepowers\7-1-032330000000001333\color1\4\color2\4\handicap\100\sex\male\cg_predictItems\1\saber1\single_1\saber2\none\char_color_red\255\char_color_green\255\char_color_blue\255\teamtask\0";
    let mut session = ClientSession::connect(server, userinfo.to_vec(), (seed & 0xffff) as u16, (seed >> 1) as i32 & 0x7fff_ffff, now());
    let mut buffer = [0u8; 65536];
    let (mut snapshots, mut sent_commands, mut active_at) = (0u64, false, None::<i32>);
    let deadline = seconds as i32 * 1000;
    loop {
        for packet in session.take_outgoing() {
            socket.send_to(&packet, session.server())?;
        }
        match socket.recv_from(&mut buffer) {
            Ok((len, from)) => {
                if std::env::var_os("JKA_SHOWPACKETS").is_some() && len >= 8 && now() < 3000 {
                    let raw = u32::from_le_bytes(buffer[..4].try_into().unwrap());
                    let fragment = (raw & (1 << 31) != 0).then(|| (u16::from_le_bytes([buffer[4], buffer[5]]), u16::from_le_bytes([buffer[6], buffer[7]])));
                    println!("[{:>6}ms] recv {len} bytes seq={} fragment={fragment:?}", now(), raw & 0x7fff_ffff);
                }
                session.packet_event(from, &buffer[..len], now());
            }
            Err(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(error) => return Err(error.into()),
        }
        session.frame(now(), 30_000);
        for event in session.take_events() {
            match event {
                SessionEvent::StateChanged(state) => println!("[{:>6}ms] state -> {state:?}", now()),
                SessionEvent::ServerInfo(info) => {
                    let map = info
                        .get(b"mapname")
                        .map(String::from_utf8_lossy)
                        .unwrap_or_else(|| "<unknown>".into());
                    println!(
                        "[{:>6}ms] infoResponse: protocol={} map={map}",
                        now(), info.protocol
                    );
                }
                SessionEvent::Print(text) => print!("[print] {}", String::from_utf8_lossy(&text)),
                SessionEvent::Gamestate => {
                    let decoder = session.decoder();
                    println!(
                        "[{:>6}ms] gamestate: map={:?} clientNum={} configstrings={} baselines={} serverId={} sv_pure={}",
                        now(), decoder.map_name(), decoder.client_number, decoder.configstrings.len(),
                        decoder.baselines.len(), session.server_id(), session.sv_pure()
                    );
                    if std::env::var_os("JKA_SHOWINFO").is_some() {
                        for index in [0u16, 1] {
                            let text = session.decoder().configstrings.get(&index).cloned().unwrap_or_default();
                            println!("CS{index}: {}", String::from_utf8_lossy(&text));
                        }
                    }
                    session.set_primed(now());
                }
                SessionEvent::ServerCommand(command) => {
                    println!("[{:>6}ms] svcmd #{}: {}", now(), command.sequence, String::from_utf8_lossy(&command.text).trim_end());
                }
                SessionEvent::Snapshot(snapshot) => {
                    snapshots += 1;
                    if snapshots == 1 || snapshots % 100 == 0 {
                        println!(
                            "[{:>6}ms] snapshot #{snapshots}: serverTime={} entities={} flags={} ping={} commandTime={:?}",
                            now(), snapshot.server_time, snapshot.entities.len(), snapshot.snap_flags,
                            session.ping(), snapshot.player_state.field_i32("commandTime")
                        );
                    }
                }
                SessionEvent::DemoMessage { .. } => {}
                SessionEvent::Download(block) => println!("[{:>6}ms] download block {} ({} bytes)", now(), block.block, block.data.len()),
                SessionEvent::MapChange => println!("[{:>6}ms] svc_mapchange", now()),
                SessionEvent::Disconnected(reason) => {
                    println!("[{:>6}ms] DISCONNECTED: {reason}", now());
                    return Ok(());
                }
            }
        }
        if session.set_cgame_time(now(), 0).is_some() {
            active_at.get_or_insert(now());
            session.create_command(UserCmd::default());
        } else if session.state() == ConnectionState::Primed {
            session.create_command(UserCmd::default());
        }
        session.send_commands(now(), 30);
        if let Some(active) = active_at {
            if !sent_commands && now() - active > 2000 {
                for command in commands {
                    println!("[{:>6}ms] > {command}", now());
                    session.add_reliable_command(command.as_bytes(), false)?;
                }
                sent_commands = true;
            }
        }
        if now() > deadline {
            println!("[{:>6}ms] disconnecting after {snapshots} snapshots, ping {}", now(), session.ping());
            session.disconnect(now());
            for packet in session.take_outgoing() {
                socket.send_to(&packet, session.server())?;
            }
            return Ok(());
        }
    }
}

fn inspect_demo(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut reader = jka_protocol::demo::DemoReader::new(BufReader::new(File::open(path)?));
    let mut count = 0u64;
    let mut first_sequence = None;
    let mut last_sequence = None;
    let mut min_size = usize::MAX;
    let mut max_size = 0;
    while let Some(record) = reader.next_record()? {
        first_sequence.get_or_insert(record.sequence);
        last_sequence = Some(record.sequence);
        count += 1;
        min_size = min_size.min(record.payload.len());
        max_size = max_size.max(record.payload.len());
    }
    println!("Demo records: {count}");
    if let (Some(first), Some(last)) = (first_sequence, last_sequence) {
        println!("Sequence range: {first}..{last}\nPayload bytes: {min_size}..{max_size}");
    }
    println!("End marker found. Framing only; game messages are not decoded yet.");
    Ok(())
}

fn dispatch(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args == ["--help"] || args == ["-h"] || args == ["help"] {
        help();
        return Ok(());
    }
    if args.len() == 2 && args[0] == "map" {
        return inspect_map(std::path::Path::new(&args[1]));
    }
    if args.len() == 2 && args[0] == "mapinfo" {
        return inspect_map(&imported_map(&args[1])?);
    }
    if args.len() == 2 && args[0] == "demo" {
        return inspect_demo(&args[1]);
    }
    if args.len() == 2 && (args[0] == "gamestate" || args[0] == "gamestate-dump") {
        return inspect_gamestate(&args[1], args[0] == "gamestate-dump");
    }
    if args.len() >= 2 && args[0] == "connect" {
        let seconds = args.get(2).map(|s| s.parse()).transpose()?.unwrap_or(10u64);
        return live_connect(&args[1], seconds, &args[3.min(args.len())..]);
    }
    if args.len() != 1 {
        return Err("usage: jka-probe <IPv4:port>".into());
    }
    let address: SocketAddrV4 = args[0].parse()?;
    let info = jka_probe::query_info(address, Duration::from_secs(3))?;
    println!("Server: {address}\nProtocol: {}", info.protocol);
    for key in [
        "hostname",
        "mapname",
        "clients",
        "sv_maxclients",
        "gametype",
    ] {
        if let Some(value) = info.get(key.as_bytes()) {
            // Escape control characters before showing untrusted legacy text.
            let display: String = String::from_utf8_lossy(value)
                .chars()
                .flat_map(char::escape_debug)
                .collect();
            println!("{key}: {display}");
        }
    }
    if !info.is_protocol_26() {
        return Err("server does not advertise the initial protocol-26 target".into());
    }
    println!("Protocol 26 advertised; gameplay compatibility is not yet implemented.");
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<_> = env::args().skip(1).collect();
    let result = if args.is_empty() {
        console()
    } else {
        dispatch(&args)
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("jka-probe: {error}");
            ExitCode::FAILURE
        }
    }
}
