use super::*;
use jka_movement::{TraceQuery, TraceResult};

struct EmptyWorld;
impl TraceWorld for EmptyWorld {
    fn trace(&mut self, query: TraceQuery) -> TraceResult {
        TraceResult::clear(query.end)
    }
    fn point_contents(&mut self, _: [f32; 3], _: i32) -> i32 {
        0
    }
}

fn animations() -> Vec<u8> {
    include_str!("../../../../jka-movement/vendor/mvsdk/code/cgame/animtable.h")
        .split("animTable_1_02")
        .next()
        .unwrap()
        .lines()
        .filter_map(|line| line.split("ENUM2STRING(").nth(1))
        .filter_map(|line| line.split(')').next())
        .map(|name| format!("{name} 0 10 -1 20\n"))
        .collect::<String>()
        .into_bytes()
}

fn snapshot(version: Version) -> jk2::Snapshot {
    let mut ps = jk2::PlayerState::zero(version);
    for (name, value) in [
        ("commandTime", 1000),
        ("pm_type", 3),
        ("speed", 400),
        ("basespeed", 400),
        ("groundEntityNum", 1023),
        ("weapon", 2),
        ("viewheight", 26),
    ] {
        assert!(ps.fields.set_field_bits(name, value));
    }
    ps.persistant[3] = 3;
    jk2::Snapshot {
        message_num: 1,
        server_time: 1000,
        delta_num: None,
        snap_flags: 0,
        server_command_sequence: 0,
        area_mask: [0; 32],
        player_state: ps,
        entities: vec![],
    }
}

#[test]
fn raw_spectator_gate_and_independent_gameplay_selection() {
    let mut snap = snapshot(Version::V104);
    assert!(free_spectator(&snap.player_state, 0));
    assert!(!free_spectator(&snap.player_state, 1));
    snap.player_state.fields.set_field_bits("pm_flags", 4096);
    assert!(!free_spectator(&snap.player_state, 0));
    snap.player_state.fields.set_field_bits("pm_flags", 0);
    snap.player_state.fields.set_field_bits("pm_type", 4); // JKA spectator, JK2 dead
    assert!(!free_spectator(&snap.player_state, 0));
    let mut config = BTreeMap::new();
    config.insert(0, b"\\dmflags\\32\\g_gametype\\4".to_vec());
    config.insert(1, b"\\pmove_fixed\\1\\pmove_msec\\99".to_vec());
    config.insert(544, b"\\mvgp\\2".to_vec());
    let settings = settings(Version::V104, &config, 0).unwrap();
    assert_eq!(settings.gameplay_version, native::Version::V102);
    assert_eq!(
        (
            settings.pmove_fixed,
            settings.pmove_msec,
            settings.no_footsteps,
            settings.gametype
        ),
        (1, 33, 1, 4)
    );
    assert_eq!(settings.tracemask & 0x0200_0000, 0);
    config.insert(544, b"\\mvgp\\3".to_vec());
    assert!(super::settings(Version::V102, &config, 0)
        .unwrap_err()
        .contains("1.03"));
}

#[test]
fn real_demo_native_identifiers_map_exactly_once() {
    for (version, bytes) in [
        (
            Version::V102,
            include_bytes!("../../../../jka-protocol/tests/fixtures/jk2-102-prefix.dm_15")
                .as_slice(),
        ),
        (
            Version::V104,
            include_bytes!("../../../../jka-protocol/tests/fixtures/jk2-104-prefix.dm_16")
                .as_slice(),
        ),
    ] {
        let mut decoder = jk2::Decoder::new();
        let mut reader = jka_protocol::demo::DemoReader::new(std::io::Cursor::new(bytes));
        let mut count = 0;
        while let Some(record) = reader.next_record().unwrap() {
            for event in decoder
                .parse_packet(record.sequence, &record.payload)
                .unwrap()
                .events
            {
                if let jk2::Event::Snapshot(snap) = event {
                    let wire = import(version, &snap.player_state).unwrap();
                    let player =
                        native::Player::new(native_version(version), &animations(), &wire).unwrap();
                    assert_eq!(
                        present(&player.normalized()).unwrap(),
                        presentation::player_state(version, &snap.player_state).unwrap()
                    );
                    count += 1;
                }
            }
        }
        assert!(count > 5);
    }
}

#[test]
fn replay_rebases_reports_first_error_and_keeps_preview_uncommitted() {
    for version in [Version::V102, Version::V104] {
        for fixed in [0, 1] {
            let mut snap = snapshot(version);
            let mut settings = native::Settings::new(native_version(version));
            settings.pmove_fixed = fixed;
            settings.spectator_server_timing = 1;
            let commands: Vec<_> = (1..=12)
                .map(|n| {
                    (
                        n,
                        UserCmd {
                            server_time: 1000 + n * 8,
                            weapon: 0,
                            forward_move: 127,
                            up_move: 25,
                            ..Default::default()
                        },
                    )
                })
                .collect();
            let mut predictor = Predictor {
                animations: Some(animations()),
                ..Default::default()
            };
            let frame = predictor
                .replay(version, &snap, &commands, None, &settings, &mut EmptyWorld)
                .unwrap();
            assert!(frame.committed.field_f32("origin[0]").unwrap() > 0.0);
            let committed = predictor.player.as_ref().unwrap().network();
            let preview = UserCmd {
                server_time: 1104,
                weapon: 19,
                generic_command: 27,
                force_selection: 30,
                inventory_selection: 30,
                forward_move: 127,
                ..Default::default()
            };
            let with_preview = predictor
                .replay(
                    version,
                    &snap,
                    &commands,
                    Some(preview),
                    &settings,
                    &mut EmptyWorld,
                )
                .unwrap();
            assert_eq!(with_preview.committed, frame.committed);
            assert_ne!(with_preview.display, with_preview.committed);
            assert_eq!(predictor.player.as_ref().unwrap().network(), committed);
            let expected = predictor.records[5].1.clone();
            for (&name, &value) in native::field_names(expected.version)
                .iter()
                .zip(&expected.fields)
            {
                assert!(snap.player_state.fields.set_field_bits(name, value));
            }
            snap.message_num = 2;
            predictor.compare(&snap, version, true).unwrap();
            assert_eq!((predictor.matched, predictor.compared), (1, 1));
            let replay = predictor
                .replay(version, &snap, &commands, None, &settings, &mut EmptyWorld)
                .unwrap();
            assert_eq!(replay.committed, frame.committed);
            snap.message_num = 3;
            for (&name, &value) in native::field_names(committed.version)
                .iter()
                .zip(&committed.fields)
            {
                snap.player_state.fields.set_field_bits(name, value);
            }
            snap.player_state
                .fields
                .set_field_bits("origin[0]", 999_f32.to_bits());
            predictor.compare(&snap, version, true).unwrap();
            assert_eq!((predictor.matched, predictor.compared), (1, 2));
            assert_eq!(predictor.diverged, 1);
            assert!(predictor.messages.last().unwrap().contains("origin[0]"));
            predictor.invalidate();
            assert!(predictor.records.is_empty() && predictor.previous.is_none());
        }
    }
}

#[test]
fn comparison_reports_small_float_errors_and_rejects_discrete_changes() {
    let mut snap = snapshot(Version::V104);
    snap.player_state
        .fields
        .set_field_bits("velocity[0]", 100_f32.to_bits());
    let mut predicted = import(Version::V104, &snap.player_state).unwrap();
    predicted
        .set_field_bits("velocity[0]", 100_f32.to_bits() + 1)
        .unwrap();
    let before = predicted.clone();
    let mut predictor = Predictor::default();
    predictor.records.push_back((1, predicted.clone()));
    predictor.compare(&snap, Version::V104, true).unwrap();
    assert_eq!(
        (predictor.matched, predictor.compared, predictor.diverged),
        (0, 1, 0)
    );
    assert!(predictor.max_error[1] > 0.0 && predictor.max_error[1] < FLOAT_ERROR_LIMIT);
    assert!(predictor.messages.last().unwrap().contains("velocity[0]"));
    assert_eq!(predicted, before);
    predictor.records.push_back((1, predicted));
    snap.message_num += 1;
    snap.player_state.fields.set_field_bits("pm_flags", 1);
    predictor.compare(&snap, Version::V104, true).unwrap();
    assert_eq!(predictor.diverged, 1);
    assert!(predictor
        .messages
        .last()
        .unwrap()
        .contains("discreteMismatch=true"));
}

// Opt-in operational tests: start scripts/jk2-test-server.ps1 with the matching
// version and its default port. Real usercmds, retail BSP collision,
// live raw snapshots and the production predictor; no window/input simulation.
#[test]
#[ignore = "requires private JK2 1.02 server on loopback:28079"]
fn loopback_102() {
    loopback(Version::V102);
}

#[test]
#[ignore = "requires private JK2 1.04 server on loopback:28079"]
fn loopback_104() {
    loopback(Version::V104);
}

fn loopback(version: Version) {
    use super::super::{NetClient, NetworkSettings};
    use jka_protocol::session::{ConnectionState, SessionEvent};
    use std::time::{Duration, Instant};
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/release/base");
    let mut roots = crate::filesystem_paths::load(&base);
    assert!(roots.jk2path.is_some(), "configure fs_jk2path first");
    roots.active_jk2 = Some(match version {
        Version::V102 => 2,
        Version::V104 => 4,
    });
    let mut animations = Vec::new();
    let options = NetworkSettings {
        prediction_debug: true,
        ..Default::default()
    };
    let mut net = NetClient::connect_jk2_preflight(
        "127.0.0.1:28079",
        options.jk2_userinfo("kyle/default"),
        0,
        version,
    )
    .unwrap();
    net.resume_connect();
    let mut predictor = Predictor::default();
    let mut world = None;
    let start = Instant::now();
    let mut active = None;
    let mut last_cmd = -8; // time-zero command primes the server's first active snapshot
    let mut frames = 0;
    let mut first_origin = None;
    let mut max_distance = 0_f32;
    let mut waiting_state = None;
    while start.elapsed() < Duration::from_secs(20) {
        for event in net.pump() {
            match event {
                SessionEvent::StateChanged(state) => println!("JK2-LIVE state={state:?}"),
                SessionEvent::Gamestate => {
                    let map = net.session().jk2_decoder().unwrap().map_name().unwrap();
                    let config = &net.session().jk2_decoder().unwrap().configstrings;
                    let info = config.get(&1).map(Vec::as_slice).unwrap_or_default();
                    let game = info_value(info, b"fs_game")
                        .filter(|value| !value.is_empty())
                        .map(|value| {
                            base.parent()
                                .unwrap()
                                .join(String::from_utf8_lossy(value).as_ref())
                        });
                    roots.jk2_pure = if net.session().sv_pure() {
                        use jka_assets::jk2_pure::{parse_checksums, Jk2PurePolicy};
                        let dirs =
                            jka_assets::pk3::filesystem_search_dirs(&base, game.as_deref(), &roots);
                        Some(std::sync::Arc::new(
                            Jk2PurePolicy::resolve(
                                dirs,
                                roots.active_jk2.unwrap(),
                                net.session().decoder().checksum_feed,
                                parse_checksums(info_value(info, b"sv_paks").unwrap()).unwrap(),
                            )
                            .unwrap(),
                        ))
                    } else {
                        None
                    };
                    jka_assets::pk3::configure_filesystem_roots(&base, roots.clone());
                    let mut assets =
                        jka_assets::pk3::AssetSearchPath::open_game(&base, game.as_deref())
                            .unwrap();
                    let bsp = assets
                        .read(&format!("maps/{map}.bsp"), 256 * 1024 * 1024)
                        .unwrap()
                        .unwrap();
                    animations = assets
                        .read("models/players/_humanoid/animation.cfg", 39998)
                        .unwrap()
                        .unwrap()
                        .bytes;
                    world = Some(CollisionWorld::from_bsp(&bsp.bytes).unwrap());
                    if let Some(policy) = roots.jk2_pure.as_ref() {
                        let reply = policy.reply(&assets, &map).unwrap();
                        net.send_reliable_now(&reply.command).unwrap();
                    }
                    let now = net.realtime();
                    net.session_mut().set_primed(now);
                }
                SessionEvent::Disconnected(error) => panic!("loopback disconnected: {error}"),
                _ => {}
            }
        }
        let now = net.realtime();
        net.session_mut().set_cgame_time(now, 0);
        if net.state() >= ConnectionState::Primed {
            let time = net.session().server_time();
            if time - last_cmd >= 8 {
                let phase =
                    active.map_or(0, |start: Instant| start.elapsed().as_millis() as i32 / 500);
                let cmd = UserCmd {
                    angles: [0, jka_movement::angle_to_short((phase * 90) as f32), 0],
                    forward_move: if phase < 8 { 127 } else { 0 },
                    right_move: if phase % 2 == 0 { 0 } else { -64 },
                    up_move: if phase % 4 < 2 { 64 } else { -64 },
                    ..Default::default()
                };
                net.session_mut().create_command_at(cmd, time);
                last_cmd = time;
            }
        }
        if net.state() == ConnectionState::Active {
            let active_start = *active.get_or_insert_with(Instant::now);
            let session = net.session();
            let snap = session.decoder().latest_snapshot().unwrap();
            let frame = predictor
                .predict(
                    session,
                    snap,
                    None,
                    session.server_time(),
                    world.as_mut().unwrap(),
                    &[],
                    &options,
                    None,
                    || Ok(animations.clone()),
                )
                .unwrap();
            if let Some(frame) = frame {
                let position = super::super::origin(&frame.committed);
                let first = *first_origin.get_or_insert(position);
                max_distance = max_distance.max(
                    (0..3)
                        .map(|axis| (position[axis] - first[axis]).powi(2))
                        .sum::<f32>()
                        .sqrt(),
                );
                frames += 1;
            } else if let Some(raw) = session.jk2_decoder().unwrap().snapshot(snap.message_num) {
                let ps = &raw.player_state;
                let key = (
                    ps.fields.field_i32("pm_type"),
                    ps.persistant[3],
                    ps.fields.field_i32("pm_flags"),
                    ps.fields.field_i32("clientNum"),
                );
                if waiting_state != Some(key) {
                    println!(
                        "JK2-LIVE snapshot fallback type/team/flags/client={key:?} local={}",
                        session.client_num()
                    );
                    waiting_state = Some(key);
                }
            }
            if active_start.elapsed() >= Duration::from_secs(6) {
                break;
            }
        }
        let now = net.realtime();
        net.session_mut().send_commands(now, 125);
        net.flush();
        std::thread::sleep(Duration::from_millis(2));
    }
    net.disconnect();
    for message in &predictor.messages {
        println!("{message}");
    }
    println!(
        "JK2-LIVE version={} frames={frames} compared={} matched={} diverged={} maxAbsOriginVelocityAngles={:?} floatLimit={} maxDistance={max_distance}",
        version.name(),
        predictor.compared,
        predictor.matched,
        predictor.diverged,
        predictor.max_error,
        FLOAT_ERROR_LIMIT
    );
    assert!(
        frames > 100 && max_distance > 32.0,
        "no meaningful live command movement"
    );
    assert!(
        predictor.compared >= 30,
        "no acknowledged-command comparison coverage"
    );
    assert_eq!(
        predictor.diverged, 0,
        "server prediction exceeded the reported float bound or changed a discrete field; see first field/command above"
    );
}
