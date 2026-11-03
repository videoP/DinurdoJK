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
fn raw_gate_settings_and_independent_gameplay_selection() {
    let mut snap = snapshot(Version::V104);
    assert!(predictable(&snap.player_state, 0), "free spectator");
    assert!(!predictable(&snap.player_state, 1), "another client's state");
    snap.player_state.fields.set_field_bits("pm_flags", 4096);
    assert!(!predictable(&snap.player_state, 0), "follow mode interpolates");
    snap.player_state.fields.set_field_bits("pm_flags", 0);
    snap.player_state.fields.set_field_bits("pm_type", 0);
    assert!(!predictable(&snap.player_state, 0), "spectator team before its first command");
    snap.player_state.persistant[3] = 0; // TEAM_FREE
    for pm_type in [0, 4, 5, 6] {
        // normal, dead, frozen, intermission: CG_PredictPlayerState runs Pmove
        snap.player_state.fields.set_field_bits("pm_type", pm_type);
        assert!(predictable(&snap.player_state, 0), "pm_type {pm_type}");
    }
    let mut config = BTreeMap::new();
    config.insert(0, b"\\dmflags\\32\\g_gametype\\4".to_vec());
    config.insert(1, b"\\pmove_fixed\\1\\pmove_msec\\99".to_vec());
    config.insert(544, b"\\mvgp\\2".to_vec());
    let playing = &snap.player_state;
    let settings = settings(Version::V104, &config, 0, 0, playing, true).unwrap();
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
    // JK2 MASK_PLAYERSOLID: CONTENTS_BODY is 0x100, not Q3's 0x2000000.
    assert_eq!(settings.tracemask, 0x111);
    assert_eq!((settings.spectator_server_timing, settings.predict_items), (0, 1));
    // cg_pmove.ps->pm_type == PM_DEAD (the previous prediction) drops bodies.
    assert_eq!(super::settings(Version::V104, &config, 0, 4, playing, true).unwrap().tracemask, 0x011);
    let spectator = snapshot(Version::V104);
    let settings = super::settings(Version::V104, &config, 0, 3, &spectator.player_state, false).unwrap();
    assert_eq!((settings.tracemask, settings.spectator_server_timing, settings.predict_items), (0x011, 1, 0));
    config.insert(544, b"\\mvgp\\3".to_vec());
    assert!(super::settings(Version::V102, &config, 0, 0, playing, true)
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
                player: Some(
                    native::Player::new(
                        native_version(version),
                        &animations(),
                        &import(version, &snap.player_state).unwrap(),
                    )
                    .unwrap(),
                ),
                ..Default::default()
            };
            let lock = native::Frame::default();
            let frame = predictor
                .replay(version, &snap, &commands, None, &settings, &lock, &mut EmptyWorld)
                .unwrap();
            assert!(frame.committed.field_f32("origin[0]").unwrap() > 0.0);
            let committed = predictor.player.as_ref().unwrap().network();
            // Provisional commands already carry JK2 wire selections.
            let preview = UserCmd {
                server_time: 1104,
                weapon: 2,
                generic_command: 20,
                force_selection: 255,
                inventory_selection: 255,
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
                    &lock,
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
                .replay(version, &snap, &commands, None, &settings, &lock, &mut EmptyWorld)
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
fn remote_context_transitions_every_wire_snapshot_in_order() {
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
        let (mut every, mut sparse) = (Predictor::default(), Predictor::default());
        let mut last = None;
        while let Some(record) = reader.next_record().unwrap() {
            for event in decoder.parse_packet(record.sequence, &record.payload).unwrap().events {
                let jk2::Event::Snapshot(snap) = event else { continue };
                let current = decoder.snapshot(snap.message_num).unwrap();
                every.update_context(version, &decoder, current, || Ok(animations())).unwrap();
                if snap.message_num % 3 == 0 {
                    sparse.update_context(version, &decoder, current, || Ok(animations())).unwrap();
                }
                last = Some(snap.message_num);
            }
        }
        let current = decoder.snapshot(last.unwrap()).unwrap();
        sparse.update_context(version, &decoder, current, || Ok(animations())).unwrap();
        let (mut every, sparse) = (every.player.unwrap(), sparse.player.unwrap());
        for slot in 0..32 {
            assert_eq!(every.context_entity(slot), sparse.context_entity(slot), "slot {slot}");
        }
        // These short fixtures carry no remote players: place a real wire
        // entity in a client slot. Present clients keep their exact wire
        // fields; only torso/legs IDs are normalized (1.02 -> 1.04), as
        // CG_TransitionEntity does.
        let mut client = current.entities.first().expect("fixture entity").clone();
        client.number = 5;
        let own = import(version, &current.player_state).unwrap();
        every.transition_context(&own, &[import_entity(version, &client).unwrap()]).unwrap();
        for entity in [&client] {
            let slot = every.context_entity(i32::from(entity.number)).unwrap();
            for (name, bits) in entity.fields.iter() {
                if version == Version::V102 && matches!(name, "torsoAnim" | "legsAnim") {
                    continue; // mapping covered by the native pinned-table test
                }
                assert_eq!(slot.field_bits(name), Some(bits), "{} {name}", entity.number);
            }
        }
        let own = current.player_state.fields.field_i32("clientNum").unwrap();
        let own_slot = every.context_entity(own).unwrap();
        assert_eq!(
            own_slot.field_bits("pos.trBase[0]"),
            current.player_state.fields.field_bits("origin[0]")
        );
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
    loopback(Version::V102, false);
}

// Production join path: `team free` through the reliable-command queue, then
// LiveInput's JK2 CGame selection and create_command_at, as the GUI does.
#[test]
#[ignore = "requires private JK2 1.02 server on loopback:28079"]
fn loopback_play_102() {
    loopback(Version::V102, true);
}

#[test]
#[ignore = "requires private JK2 1.04 server on loopback:28079"]
fn loopback_play_104() {
    loopback(Version::V104, true);
}

#[test]
#[ignore = "requires private JK2 1.04 server on loopback:28079"]
fn loopback_104() {
    loopback(Version::V104, false);
}

// Spawned player holds forward into walls in four directions and taps jump
// once a second: the reported "jump while running into a wall" case.
#[test]
#[ignore = "requires private JK2 1.04 server on loopback:28079"]
fn loopback_wall_jump_104() {
    WALL_JUMP.with(|wall| wall.set(true));
    loopback(Version::V104, true);
}

thread_local! {
    static WALL_JUMP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Offline repro for "run into a wall, tap jump": native JK2 Pmove on the real
/// ffa_bespin collision, comparing each provisional (display) preview with the
/// next committed command. Reports only; the server path is the same SDK.
#[test]
#[ignore = "requires fs_jk2path with JK2 1.04 assets"]
fn wall_jump_preview_probe() {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/release/base");
    let mut roots = crate::filesystem_paths::load(&base);
    assert!(roots.jk2path.is_some(), "configure fs_jk2path first");
    roots.active_jk2 = Some(4);
    jka_assets::pk3::configure_filesystem_roots(&base, roots);
    let mut assets = jka_assets::pk3::AssetSearchPath::open_game(&base, None).unwrap();
    let bsp_bytes = assets.read("maps/ffa_bespin.bsp", 256 * 1024 * 1024).unwrap().unwrap().bytes;
    let anims = assets.read("models/players/_humanoid/animation.cfg", 39998).unwrap().unwrap().bytes;
    let spawns = jka_assets::bsp::Bsp::parse(&bsp_bytes).unwrap().deathmatch_spawns();
    let mut world = CollisionWorld::from_jk2_bsp(&bsp_bytes).unwrap();
    let mut settings = native::Settings::new(native::Version::V104);
    settings.tracemask = 0x111;
    let f = |state: &native::State, name: &str| f32::from_bits(state.field_bits(name).unwrap());
    let i = |state: &native::State, name: &str| state.field_bits(name).unwrap() as i32;
    let pos = |s: &native::State| [f(s, "origin[0]"), f(s, "origin[1]"), f(s, "origin[2]")];
    let mut worst = (0.0_f32, String::new());
    for level in [1, 2, 3] {
        for spawn in spawns.iter().take(6) {
            for yaw in [0.0_f32, 90.0, 180.0, 270.0] {
                let mut state = native::State::zero(native::Version::V104);
                for (name, value) in [
                    ("commandTime", 1000_u32),
                    ("origin[0]", spawn.origin[0].to_bits()),
                    ("origin[1]", spawn.origin[1].to_bits()),
                    ("origin[2]", (spawn.origin[2] + 9.0).to_bits()),
                    ("groundEntityNum", 1023),
                    ("gravity", 800),
                    ("speed", 250),
                    ("viewheight", 36),
                    ("weapon", 2),
                    ("fd.forcePower", 100),
                    ("fd.forcePowersKnown", 1 << 1),
                    ("fd.forcePowerLevel[FP_LEVITATION]", level),
                ] {
                    state.set_field_bits(name, value).unwrap();
                }
                let _ = state.set_field_bits("basespeed", 250);
                state.stats[0] = 100;
                let mut player = native::Player::new(native::Version::V104, &anims, &state).unwrap();
                let solids = Vec::new();
                let mut trace = PredictionWorld { world: &mut world, solids: &solids, client_num: 0 };
                let mut time = 1000;
                let mut stuck = 0;
                let mut jump_at = None;
                for step in 0..700 {
                    time += 8;
                    let jumping = jump_at.map(|at| step - at);
                    // settle, run into the wall, then tap jump (2 commands) and keep pushing
                    let cmd = UserCmd {
                        server_time: time,
                        angles: [0, jka_movement::angle_to_short(yaw), 0],
                        forward_move: if step > 50 { 127 } else { 0 },
                        up_move: if jumping.is_some_and(|n| n < 2) { 127 } else { 0 },
                        weapon: 2,
                        ..Default::default()
                    };
                    let before = player.network();
                    let preview = player.try_clone().unwrap();
                    player.predict(movement_cmd(&cmd), &settings, &mut trace).unwrap();
                    let after = player.network();
                    // The frame drawn between these two commands.
                    for offset in [2, 5, 7] {
                        let mut provisional = preview.try_clone().unwrap();
                        let mut early = cmd;
                        early.server_time = time - 8 + offset;
                        provisional.predict(movement_cmd(&early), &settings, &mut trace).unwrap();
                        let shown = provisional.network();
                        let t = offset as f32 / 8.0;
                        let (a, b, p) = (pos(&before), pos(&after), pos(&shown));
                        let error = (0..3)
                            .map(|k| (p[k] - (a[k] + (b[k] - a[k]) * t)).powi(2))
                            .sum::<f32>()
                            .sqrt();
                        if error > 2.0 && offset == 5 {
                            println!(
                                "JK2-WALL-CASE lev={level} spawn={:?} yaw={yaw} step={step} jump+{:?} err={error:.2} before={a:?} after={b:?} shown={p:?} vz {:.1}->{:.1} shown {:.1} ground {}->{} shown {}",
                                spawn.origin, jumping, f(&before, "velocity[2]"), f(&after, "velocity[2]"), f(&shown, "velocity[2]"),
                                i(&before, "groundEntityNum"), i(&after, "groundEntityNum"), i(&shown, "groundEntityNum"),
                            );
                        }
                        if error > worst.0 {
                            worst = (error, format!(
                                "lev={level} spawn={:?} yaw={yaw} step={step} jump+{:?} off={offset} err={error:.3} legs {}->{} shown {} vz {:.1}->{:.1} shown {:.1}",
                                spawn.origin, jumping, i(&before, "legsAnim") & !2048, i(&after, "legsAnim") & !2048,
                                i(&shown, "legsAnim") & !2048, f(&before, "velocity[2]"), f(&after, "velocity[2]"), f(&shown, "velocity[2]"),
                            ));
                        }
                    }
                    let speed = f(&after, "velocity[0]").hypot(f(&after, "velocity[1]"));
                    if jump_at.is_none() && step > 60 && i(&after, "groundEntityNum") != 1023 {
                        stuck = if speed < 1.0 { stuck + 1 } else { 0 };
                        if stuck == 10 {
                            jump_at = Some(step + 1);
                        }
                    }
                    if jumping.is_some_and(|n| n > 150) {
                        break;
                    }
                }
            }
        }
    }
    println!("JK2-WALL-PROBE worst preview error: {}", worst.1);
}

fn loopback(version: Version, play: bool) {
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
    let wall = WALL_JUMP.with(|wall| wall.get());
    let options = NetworkSettings {
        prediction_debug: true,
        show_miss: wall,
        // The reported case: rank-6 loadout with level-3 Force jump.
        forcepowers: if wall { "6-2-030020300000030331".into() } else { NetworkSettings::default().forcepowers },
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
    let mut input = super::super::LiveInput::default();
    input.set_jk2(true);
    let mut team_sent = false;
    // WP_InitForcePowers returns a first spawn to TEAM_SPECTATOR with `nfr`;
    // the force menu's join resends the team after SetTeam's 5 s delay.
    let mut rejoin_at: Option<Instant> = None;
    let mut rejoins = 0;
    let mut spawned_at: Option<Instant> = None;
    let mut cycled = false;
    let (mut playing_frames, mut playing_first, mut playing_distance) = (0, None, 0_f32);
    let mut playing_compared = 0;
    let mut wall_reported = None;
    let run_for = Duration::from_secs(if wall { 16 } else if play { 10 } else { 6 });
    while start.elapsed() < Duration::from_secs(if play { 50 } else { 20 }) {
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
                    world = Some(CollisionWorld::from_jk2_bsp(&bsp.bytes).unwrap());
                    if let Some(policy) = roots.jk2_pure.as_ref() {
                        let reply = policy.reply(&assets, &map).unwrap();
                        net.send_reliable_now(&reply.command).unwrap();
                    }
                    let now = net.realtime();
                    net.session_mut().set_primed(now);
                }
                SessionEvent::ServerCommand(command)
                    if play
                        && (command.text.starts_with(b"print")
                            || command.text.starts_with(b"cp")
                            || command.text.starts_with(b"nfr")) =>
                {
                    println!("JK2-LIVE server: {}", String::from_utf8_lossy(&command.text).trim_end());
                    if command.text.starts_with(b"nfr") && spawned_at.is_none() && rejoins < 2 {
                        // App::handle_jk2_force_rank: legalize for the announced
                        // rank (this server is FFA, no weapon/force disables).
                        let rank = atoi(command.text.split(|&b| b == b' ').nth(1).unwrap_or(b"0"));
                        let rules = jka_movement::jk2::select::ForceRules {
                            max_rank: rank, free_saber: false, team_force: 0, gametype: 0, force_disable: 0,
                        };
                        let (legal, _) = jka_movement::jk2::select::legalize_force_powers(
                            native_version(version), native_version(version), &options.forcepowers, rules,
                        ).unwrap();
                        let mut info = options.jk2_userinfo("kyle/default");
                        jka_protocol::session::set_info_value(&mut info, b"forcepowers", legal.as_bytes());
                        net.session_mut()
                            .add_reliable_command(&[b"userinfo \"".as_slice(), &info, b"\""].concat(), false)
                            .unwrap();
                        rejoin_at = Some(Instant::now() + Duration::from_millis(5500));
                    }
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
                let phase = if play {
                    // Idle until spawned: spectator +attack would start following.
                    spawned_at.map_or(-1, |start: Instant| start.elapsed().as_millis() as i32 / 500)
                } else {
                    active.map_or(0, |start: Instant| start.elapsed().as_millis() as i32 / 500)
                };
                let cmd = if play {
                    let session = net.session();
                    let raw = session
                        .decoder()
                        .latest_snapshot()
                        .and_then(|snap| Some((snap, session.jk2_decoder()?.snapshot(snap.message_num)?)));
                    if let Some((presented, raw)) = raw {
                        let snap = import(version, &raw.player_state).unwrap();
                        let gameplay =
                            gameplay(version, &session.jk2_decoder().unwrap().configstrings, session.client_num())
                                .unwrap();
                        let predicted = predictor.predicted_state().unwrap_or_else(|| snap.clone());
                        let frame = super::super::Jk2Frame {
                            gameplay,
                            snap,
                            predicted,
                            time,
                            presented: presented.player_state.clone(),
                        };
                        input.sync_jk2(&frame);
                        if phase == 14 && !cycled {
                            cycled = true;
                            input.jk2_cycle_weapon(&frame, true).unwrap();
                        }
                    }
                    // forward, strafe, jump, crouch and attack phases while turning
                    let mut held = std::collections::HashSet::new();
                    let ms = spawned_at.map_or(-1, |start: Instant| start.elapsed().as_millis() as i32);
                    let actions = if wall {
                        // 4 s per direction: 1.5 s to reach a wall, then a short
                        // jump tap each second while still pushing forward.
                        [
                            ("+forward", ms >= 0),
                            ("+moveright", false),
                            ("+moveup", ms % 4000 >= 1500 && ms % 1000 < 40),
                            ("+movedown", false),
                            ("+attack", false),
                            ("+altattack", false),
                        ]
                    } else {
                        [
                            ("+forward", (0..16).contains(&phase)),
                            ("+moveright", phase % 4 == 1),
                            ("+moveup", phase % 6 == 2),
                            ("+movedown", phase % 6 == 4),
                            ("+attack", (12..14).contains(&phase)),
                            ("+altattack", (16..18).contains(&phase)),
                        ]
                    };
                    for (name, on) in actions {
                        if on {
                            held.insert(name.to_owned());
                        }
                    }
                    let yaw = if wall { (ms.max(0) / 4000) * 90 } else { phase.max(0) * 45 };
                    input.view_angles = [0.0, yaw as f32, 0.0];
                    input.create_cmd(&super::super::CommandButtons {
                        active: &held,
                        forced_moveup: false,
                        idrive: false,
                        any_key: false,
                        talking: false,
                    })
                } else {
                    UserCmd {
                        angles: [0, jka_movement::angle_to_short((phase * 90) as f32), 0],
                        forward_move: if phase < 8 { 127 } else { 0 },
                        right_move: if phase % 2 == 0 { 0 } else { -64 },
                        up_move: if phase % 4 < 2 { 64 } else { -64 },
                        ..Default::default()
                    }
                };
                net.session_mut().create_command_at(cmd, time);
                last_cmd = time;
            }
        }
        if net.state() == ConnectionState::Active {
            let active_start = *active.get_or_insert_with(Instant::now);
            // The join's `team spectator` starts SetTeam's 5 s switch delay.
            if play && !team_sent && active_start.elapsed() >= Duration::from_secs(6) {
                net.session_mut().add_reliable_command(b"team free", false).unwrap();
                team_sent = true;
            }
            if rejoin_at.is_some_and(|at| Instant::now() >= at) && spawned_at.is_none() {
                println!("JK2-LIVE resending `team free` after nfr");
                net.session_mut().add_reliable_command(b"team free", false).unwrap();
                rejoin_at = None;
                rejoins += 1;
            }
            let session = net.session();
            let snap = session.decoder().latest_snapshot().unwrap();
            let frame = match predictor.predict(
                session,
                snap,
                None,
                session.server_time(),
                world.as_mut().unwrap(),
                &[],
                &options,
                None,
                || Ok(animations.clone()),
            ) {
                Ok(frame) => frame,
                // The app falls back to snapshot interpolation; report it.
                Err(error) if wall => {
                    println!("JK2-LIVE prediction failed: {error}");
                    None
                }
                Err(error) => panic!("{error}"),
            };
            let raw_state = session
                .jk2_decoder()
                .unwrap()
                .snapshot(snap.message_num)
                .map(|raw| (raw.player_state.persistant[3], raw.player_state.fields.field_i32("pm_type")));
            if let (true, Some(frame), Some(raw), Some(spawn)) = (
                wall,
                &frame,
                session.jk2_decoder().unwrap().snapshot(snap.message_num),
                spawned_at,
            ) {
                // Once per wall, just before the jump taps: the wall gap of the
                // server's acknowledged origin and of the committed prediction.
                let ms = spawn.elapsed().as_millis() as i32;
                let segment = ms / 4000;
                if (1300..1500).contains(&(ms % 4000)) && wall_reported != Some(segment) {
                    wall_reported = Some(segment);
                    let bits = |name: &str| f32::from_bits(raw.player_state.fields.field_bits(name).unwrap());
                    let server = [bits("origin[0]"), bits("origin[1]"), bits("origin[2]")];
                    let predicted = super::super::origin(&frame.committed);
                    let yaw = ((segment * 90) as f32).to_radians();
                    let dir = [yaw.cos(), yaw.sin(), 0.0];
                    let world = world.as_mut().unwrap();
                    let mut gap = |o: [f32; 3]| {
                        let result = world.trace(jka_movement::TraceQuery {
                            start: std::array::from_fn(|k| o[k] - dir[k]),
                            end: std::array::from_fn(|k| o[k] + dir[k] * 4.0),
                            mins: [-15.0, -15.0, -24.0],
                            maxs: [15.0, 15.0, 40.0],
                            pass_entity: 0,
                            mask: 0x11,
                        });
                        // Travelled f*5 from 1 unit back, stopping 0.125 short.
                        (result.fraction * 5.0 - 1.0 + 0.125, result.start_solid)
                    };
                    println!(
                        "JK2-WALL-GAP yaw={} server={server:?} gap={:?} predicted={predicted:?} gap={:?} serverCommandTime={}",
                        segment * 90,
                        gap(server),
                        gap(predicted),
                        raw.player_state.fields.field_i32("commandTime").unwrap_or(0),
                    );
                }
            }
            if let (Some(frame), Some((team, Some(0)))) = (&frame, raw_state) {
                if team != 3 {
                    spawned_at.get_or_insert_with(Instant::now);
                    let position = super::super::origin(&frame.committed);
                    let first = *playing_first.get_or_insert(position);
                    playing_distance = playing_distance.max(
                        (0..3).map(|axis| (position[axis] - first[axis]).powi(2)).sum::<f32>().sqrt(),
                    );
                    if playing_frames == 0 {
                        playing_compared = predictor.compared;
                    }
                    playing_frames += 1;
                }
            }
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
            let done = if play {
                spawned_at.is_some_and(|spawn| spawn.elapsed() >= run_for)
                    || active_start.elapsed() >= Duration::from_secs(35)
            } else {
                active_start.elapsed() >= run_for
            };
            if done {
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
    if play {
        // Divergence while playing is reported (first field per snapshot) but
        // not asserted here: the server bot can deal damage and knockback that
        // client Pmove cannot author. Spawning and predicted movement are.
        println!(
            "JK2-LIVE-PLAY version={} playingFrames={playing_frames} playingDistance={playing_distance} comparedBeforeSpawn={playing_compared}",
            version.name()
        );
        assert!(
            playing_frames > 100 && playing_distance > 32.0,
            "`team free` did not produce a spawned, locally predicted, moving player"
        );
    } else {
        assert_eq!(
            predictor.diverged, 0,
            "server prediction exceeded the reported float bound or changed a discrete field; see first field/command above"
        );
    }
}
