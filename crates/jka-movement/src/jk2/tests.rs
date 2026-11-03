use super::*;

fn animations() -> Vec<u8> {
    // Synthetic timing fixture. Retail animation.cfg is used by the executable
    // diagnostic, without distributing retail assets with these unit tests.
    include_str!("../../vendor/mvsdk/code/cgame/animtable.h")
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

#[test]
fn native_wire_descriptors_match_decoder() {
    for (version, schema) in [
        (Version::V102, jka_protocol::jk2::fields::PLAYER_FIELDS_102),
        (Version::V104, jka_protocol::jk2::fields::PLAYER_FIELDS_104),
    ] {
        assert_eq!(
            field_names(version),
            schema.iter().map(|&(name, _)| name).collect::<Vec<_>>()
        );
    }
}

#[test]
fn native_roundtrip_and_validation_are_transactional() {
    let mut state = State::zero(Version::V104);
    for (i, &(name, bits)) in jka_protocol::jk2::fields::PLAYER_FIELDS_104
        .iter()
        .enumerate()
    {
        state
            .set_field_bits(
                name,
                if bits == 0 {
                    (i as f32 + 0.25).to_bits()
                } else {
                    i as u32
                },
            )
            .unwrap();
    }
    for (name, value) in [
        ("clientNum", 1),
        ("pm_type", 0),
        ("weapon", 2),
        ("fd.forcePowerLevel[FP_LEVITATION]", 3),
        ("fd.forcePowerLevel[FP_SEE]", 2),
    ] {
        state.set_field_bits(name, value).unwrap();
    }
    state
        .set_field_bits("origin[0]", 123.5f32.to_bits())
        .unwrap();
    state.stats[0] = 100;
    state.ammo[3] = 99;
    let mut player = Player::new(state.version, &animations(), &state).unwrap();
    assert_eq!(player.network(), state);
    let mut invalid = state.clone();
    invalid
        .set_field_bits("origin[0]", f32::NAN.to_bits())
        .unwrap();
    assert!(player.set_network(&invalid).is_err());
    assert_eq!(player.network(), state);
    invalid = state.clone();
    invalid.set_field_bits("clientNum", 32).unwrap();
    assert!(player.set_network(&invalid).is_err());
    assert_eq!(player.network(), state);
}

#[test]
fn native_102_animation_and_block_conversion_matches_pinned_tables() {
    unsafe extern "C" {
        static mvsdk_animMappingTable_1_02_to_1_04: i32;
        static mvsdk_animMappingTable_1_04_to_1_02: i32;
    }
    let toggle = unsafe { jk2_contract(9) } as u32;
    let mut state = State::zero(Version::V102);
    let mut player = Player::new(state.version, &animations(), &state).unwrap();
    for id in 0..unsafe { jk2_contract(5) } {
        for toggle in [0, toggle] {
            let input = id as u32 | toggle;
            state.set_field_bits("legsAnim", input).unwrap();
            state.set_field_bits("torsoAnim", input).unwrap();
            state.set_field_bits("forceHandExtend", 10).unwrap(); // HANDEXTEND_TAUNT
            state.set_field_bits("forceDodgeAnim", input).unwrap();
            state.set_field_bits("saberBlocked", 3).unwrap();
            player.set_network(&state).unwrap();
            // Indices are bounded by native enum counts; these are immutable
            // upstream C enum arrays, not a gameplay struct-layout cast.
            let mapped = unsafe {
                *std::ptr::addr_of!(mvsdk_animMappingTable_1_02_to_1_04).add(id as usize)
            } as u32;
            let normalized = player.normalized();
            for name in ["legsAnim", "torsoAnim", "forceDodgeAnim"] {
                assert_eq!(
                    normalized.field_bits(name),
                    Some(mapped | toggle),
                    "{name}, id={id}"
                );
            }
            assert_eq!(normalized.field_bits("saberBlocked"), Some(4));
            let returned = unsafe {
                *std::ptr::addr_of!(mvsdk_animMappingTable_1_04_to_1_02).add(mapped as usize)
            } as u32;
            assert_eq!(
                player.network().field_bits("legsAnim"),
                Some(returned | toggle)
            );
            assert_eq!(player.network().field_bits("saberBlocked"), Some(3));
        }
    }
}

#[test]
fn native_command_replays_match_direct_mvsdk() {
    let report = diagnostics::run(&animations()).unwrap();
    assert!(report.contains("96 cases"), "{report}");
}

#[test]
fn native_server_spectator_rounds_time_without_fixed_subdivision() {
    for version in [Version::V102, Version::V104] {
        let mut state = diagnostics::seed(version, 2);
        state.set_field_bits("pm_type", 3).unwrap();
        state.set_field_bits("origin[2]", 64_f32.to_bits()).unwrap();
        let seed = Player::new(version, &animations(), &state).unwrap();
        let mut server = seed.try_clone().unwrap();
        let mut expected = seed.try_clone().unwrap();
        let mut cgame = seed.try_clone().unwrap();
        let mut settings = Settings::new(version);
        settings.pmove_fixed = 1;
        settings.spectator_server_timing = 1;
        let cmd = UserCmd {
            server_time: 1017,
            weapon: 2,
            forward_move: 127,
            up_move: 64,
            ..Default::default()
        };
        server
            .predict(cmd, &settings, &mut diagnostics::FixtureWorld::default())
            .unwrap();
        // g_active.c rounds to 1024 before entering SpectatorThink, which
        // zeroes pmove_t and calls native Pmove with variable subdivision.
        expected
            .predict(
                UserCmd {
                    server_time: 1024,
                    ..cmd
                },
                &Settings::new(version),
                &mut diagnostics::FixtureWorld::default(),
            )
            .unwrap();
        assert_eq!(server.native_bytes(), expected.native_bytes());
        settings.spectator_server_timing = 0;
        cgame
            .predict(cmd, &settings, &mut diagnostics::FixtureWorld::default())
            .unwrap();
        assert_ne!(
            server.network().field_bits("origin[0]"),
            cgame.network().field_bits("origin[0]")
        );
        settings.spectator_server_timing = 1;
        verify_replay(&seed, &[cmd], &settings, diagnostics::FixtureWorld::default).unwrap();
        let normal = diagnostics::seed(version, 2);
        server.set_network(&normal).unwrap();
        let before = server.native_bytes();
        assert!(server
            .predict(cmd, &settings, &mut diagnostics::FixtureWorld::default())
            .unwrap_err()
            .contains("PM_SPECTATOR"));
        assert_eq!(server.native_bytes(), before);
    }
}

#[test]
fn native_collision_failure_rolls_back_complete_state() {
    struct BadWorld;
    impl TraceWorld for BadWorld {
        fn trace(&mut self, q: crate::TraceQuery) -> TraceResult {
            let mut t = TraceResult::clear(q.end);
            t.fraction = f32::NAN;
            t
        }
        fn point_contents(&mut self, _: [f32; 3], _: i32) -> i32 {
            0
        }
    }
    for version in [Version::V102, Version::V104] {
        let mut player =
            Player::new(version, &animations(), &diagnostics::seed(version, 2)).unwrap();
        let before = player.native_bytes();
        let error = player
            .predict(
                diagnostics::commands(2)[0],
                &Settings::new(version),
                &mut BadWorld,
            )
            .unwrap_err();
        assert!(error.contains("Invalid collision result"), "{error}");
        assert_eq!(player.native_bytes(), before);
    }
}

#[test]
fn native_callback_panics_and_reentry_cannot_escape() {
    struct ReentryWorld {
        checked: bool,
    }
    impl TraceWorld for ReentryWorld {
        fn trace(&mut self, _: crate::TraceQuery) -> TraceResult {
            match Player::new(Version::V104, b"x", &State::zero(Version::V104)) {
                Err(e) => assert!(e.contains("callback"), "{e}"),
                Ok(_) => panic!("unexpected nested native host"),
            }
            self.checked = true;
            panic!("intentional collision panic");
        }
        fn point_contents(&mut self, _: [f32; 3], _: i32) -> i32 {
            0
        }
    }
    let version = Version::V104;
    let mut player = Player::new(version, &animations(), &diagnostics::seed(version, 2)).unwrap();
    let before = player.native_bytes();
    let mut world = ReentryWorld { checked: false };
    let error = player
        .predict(
            diagnostics::commands(2)[0],
            &Settings::new(version),
            &mut world,
        )
        .unwrap_err();
    assert!(world.checked);
    assert!(error.contains("panicked"), "{error}");
    assert_eq!(player.native_bytes(), before);
}

#[test]
fn native_version_switching_isolated_between_players() {
    let cfg = animations();
    let a_settings = Settings::new(Version::V102);
    let b_settings = Settings::new(Version::V104);
    let seed = diagnostics::seed(Version::V102, 2);
    let mut expected = Player::new(Version::V102, &cfg, &seed).unwrap();
    let commands = diagnostics::commands(2);
    for &cmd in &commands {
        expected
            .predict(cmd, &a_settings, &mut diagnostics::FixtureWorld::default())
            .unwrap();
    }
    let mut a = Player::new(Version::V102, &cfg, &seed).unwrap();
    let mut b = Player::new(Version::V104, &cfg, &diagnostics::seed(Version::V104, 4)).unwrap();
    for &cmd in &commands {
        a.predict(cmd, &a_settings, &mut diagnostics::FixtureWorld::default())
            .unwrap();
        b.predict(
            UserCmd { weapon: 4, ..cmd },
            &b_settings,
            &mut diagnostics::FixtureWorld::default(),
        )
        .unwrap();
    }
    assert_eq!(a.native_bytes(), expected.native_bytes());
}

#[test]
fn native_diagnostic_names_internal_fields_and_first_divergence() {
    {
        let _guard = ffi::NativeGuard::new();
        for word in (0..unsafe { jk2_contract(3) }).step_by(4) {
            let name = unsafe { CStr::from_ptr(jk2_native_word_name(word)) }
                .to_str()
                .unwrap();
            assert_ne!(
                name, "native padding",
                "missing native member at offset {word}"
            );
        }
    }
    let seed = Player::new(
        Version::V104,
        &animations(),
        &diagnostics::seed(Version::V104, 2),
    )
    .unwrap();
    let mut calls = 0;
    let error = verify_replay(
        &seed,
        &diagnostics::commands(2),
        &Settings::new(Version::V104),
        || {
            calls += 1;
            diagnostics::FixtureWorld {
                water: false,
                wall: calls == 2,
            }
        },
    )
    .unwrap_err();
    assert!(
        error.contains("First JK2 replay divergence: command"),
        "{error}"
    );
    assert!(
        error.contains("field origin[") || error.contains("field velocity["),
        "{error}"
    );
}

/// Native 1.04 animation enum value, in pinned animtable.h order. Commented
/// `#sep ENUM2STRING(` section markers are not enum entries.
fn anim(name: &str) -> u32 {
    include_str!("../../vendor/mvsdk/code/cgame/animtable.h")
        .split("animTable_1_02")
        .next()
        .unwrap()
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("ENUM2STRING("))
        .position(|line| line.split(')').next() == Some(name))
        .unwrap_or_else(|| panic!("{name} missing from animtable.h")) as u32
}

fn locked_pair(version: Version) -> (State, EntityState, Frame) {
    let mut own = diagnostics::seed(version, 2);
    for (name, bits) in [
        ("clientNum", 0),
        ("saberLockTime", 5000),
        ("saberLockEnemy", 1),
        ("saberLockFrame", 5),
        ("saberLockAdvance", 1),
        ("torsoAnim", anim("BOTH_BF2LOCK")),
        ("legsAnim", anim("BOTH_BF2LOCK")),
    ] {
        own.set_field_bits(name, bits).unwrap();
    }
    let mut enemy = EntityState::zero(version, 1);
    for (name, bits) in [
        ("eType", 1), // ET_PLAYER
        ("clientNum", 1),
        ("weapon", 2),
        ("forceFrame", 7),
        ("groundEntityNum", 1022),
        ("torsoAnim", anim("BOTH_BF1LOCK")),
        ("legsAnim", anim("BOTH_BF1LOCK")),
        ("pos.trBase[0]", 40f32.to_bits()),
        ("pos.trBase[2]", 24f32.to_bits()),
    ] {
        enemy.set_field_bits(name, bits).unwrap();
    }
    let frame = Frame {
        cg_time: 1010,
        lock_time: 5000,
        lock_enemy: 1,
        has_lock_origin: 1,
        snap_origin: [0.0, 0.0, 24.0],
        lock_origin: [40.0, 0.0, 24.0],
    };
    (own, enemy, frame)
}

fn lock_cmd(time: i32) -> UserCmd {
    UserCmd { server_time: time, weapon: 2, buttons: 1, ..Default::default() }
}

#[test]
fn native_context_projects_remote_clients_and_writes_back_lock_state() {
    let version = Version::V104;
    let (own, enemy, frame) = locked_pair(version);
    let settings = Settings::new(version);
    let mut player = Player::new(version, &animations(), &own).unwrap();
    // No transitioned snapshot: the explicit missing-context failure remains.
    let before = player.native_bytes();
    let error = player
        .predict(lock_cmd(1016), &settings, &mut diagnostics::FixtureWorld::default())
        .unwrap_err();
    assert!(error.contains("requires remote client state"), "{error}");
    assert_eq!(player.native_bytes(), before);

    player.transition_context(&own, &[enemy.clone()]).unwrap();
    player.set_frame(&frame);
    let self_slot = player.context_entity(0).unwrap();
    assert_eq!(self_slot.field_bits("eType"), Some(1));
    assert_eq!(self_slot.field_bits("pos.trBase[2]"), Some(24f32.to_bits()));
    assert_eq!(player.context_entity(1).unwrap().field_bits("forceFrame"), Some(7));

    // The direct oracle runs the same context projection/writeback schedule.
    verify_replay(&player, &[lock_cmd(1016), lock_cmd(1032)], &settings, diagnostics::FixtureWorld::default)
        .unwrap();

    // An unsent preview never commits opponent lock state.
    let mut preview = player.try_clone().unwrap();
    preview
        .predict(lock_cmd(1016), &settings, &mut diagnostics::FixtureWorld::default())
        .unwrap();
    assert_eq!(player.context_entity(1).unwrap().field_bits("forceFrame"), Some(7));

    // PM_SaberLocked: own frame 5 -> 4 (strength 1), so the BF1LOCK opponent
    // is set to firstFrame + remaining; CGame writes it back to forceFrame.
    player
        .predict(lock_cmd(1016), &settings, &mut diagnostics::FixtureWorld::default())
        .unwrap();
    let after = player.network();
    let written = player.context_entity(1).unwrap();
    assert_eq!(
        after.field_bits("saberLockFrame"),
        Some(4),
        "lockTime={:?} torso={:?} saberMove={:?} commandTime={:?} enemy torso={:?} forceFrame={:?} saberMove={:?}",
        after.field_bits("saberLockTime"),
        after.field_bits("torsoAnim"),
        after.field_bits("saberMove"),
        after.field_bits("commandTime"),
        written.field_bits("torsoAnim"),
        written.field_bits("forceFrame"),
        written.field_bits("saberMove"),
    );
    assert_eq!(written.field_bits("forceFrame"), Some(4));
    assert_eq!(preview.context_entity(1), Some(written.clone()));
    // Only the four CGame writeback fields change; the rest stays wire state.
    assert_eq!(written.field_bits("pos.trBase[0]"), Some(40f32.to_bits()));
    // Lock facing comes from the opponent's lerped origin (+X: yaw 0).
    assert_eq!(player.network().field_bits("viewangles[1]"), Some(0f32.to_bits()));
}

#[test]
fn native_context_frame_transaction_rolls_back_remote_writeback() {
    struct FailSecond(u32);
    impl TraceWorld for FailSecond {
        fn trace(&mut self, q: crate::TraceQuery) -> TraceResult {
            self.0 += 1;
            if self.0 > 40 {
                panic!("intentional collision panic");
            }
            diagnostics::FixtureWorld::default().trace(q)
        }
        fn point_contents(&mut self, p: [f32; 3], pass: i32) -> i32 {
            diagnostics::FixtureWorld::default().point_contents(p, pass)
        }
    }
    let version = Version::V104;
    let (own, enemy, frame) = locked_pair(version);
    let settings = Settings::new(version);
    let mut player = Player::new(version, &animations(), &own).unwrap();
    player.transition_context(&own, &[enemy]).unwrap();
    player.set_frame(&frame);
    let before = player.context_entity(1).unwrap();
    let mut world = FailSecond(0);
    let error = player
        .context_transaction(|player| {
            for time in (1016..2000).step_by(16) {
                player.predict(lock_cmd(time), &settings, &mut world)?;
            }
            Ok(())
        })
        .unwrap_err();
    assert!(error.contains("panicked"), "{error}");
    assert!(world.0 > 40);
    assert_eq!(player.context_entity(1), Some(before));
}

#[test]
fn native_context_keeps_absent_clients_maps_102_and_resets_on_invalid_input() {
    unsafe extern "C" {
        static mvsdk_animMappingTable_1_04_to_1_02: i32;
    }
    let to_102 = |id: u32| unsafe {
        *std::ptr::addr_of!(mvsdk_animMappingTable_1_04_to_1_02).add(id as usize)
    } as u32;
    let version = Version::V102;
    let (own, mut enemy, _) = locked_pair(version);
    let mut own = own;
    for name in ["torsoAnim", "legsAnim"] {
        own.set_field_bits(name, to_102(anim("BOTH_BF2LOCK"))).unwrap();
        enemy.set_field_bits(name, to_102(anim("BOTH_BF1LOCK"))).unwrap();
    }
    let mut player = Player::new(version, &animations(), &own).unwrap();
    player.transition_context(&own, &[enemy.clone()]).unwrap();
    let slot = player.context_entity(1).unwrap();
    assert_eq!(slot.field_bits("torsoAnim"), Some(anim("BOTH_BF1LOCK")));
    assert_eq!(player.context_entity(0).unwrap().field_bits("torsoAnim"), Some(anim("BOTH_BF2LOCK")));
    // CG_TransitionSnapshot leaves clients absent from the new snapshot alone.
    player.transition_context(&own, &[]).unwrap();
    assert_eq!(player.context_entity(1), Some(slot));
    // Strict import: an out-of-range 1.02 animation is not reinterpreted.
    enemy.set_field_bits("torsoAnim", 5000).unwrap();
    assert!(player.transition_context(&own, &[enemy]).is_err());
    assert_eq!(player.context_entity(1), Some(EntityState::zero(Version::V104, 1)));
    let error = player
        .predict(lock_cmd(1016), &Settings::new(version), &mut diagnostics::FixtureWorld::default())
        .unwrap_err();
    assert!(error.contains("requires remote client state"), "{error}");
}

#[test]
fn native_entity_descriptors_match_decoder() {
    for (version, schema) in [
        (Version::V102, jka_protocol::jk2::fields::ENTITY_FIELDS_102),
        (Version::V104, jka_protocol::jk2::fields::ENTITY_FIELDS_104),
    ] {
        assert_eq!(
            entity_field_names(version),
            schema.iter().map(|&(name, _)| name).collect::<Vec<_>>()
        );
    }
}
#[test]
fn native_cgame_selection_matches_pinned_commands() {
    use select::{cycle_force, cycle_inventory, cycle_weapon, out_of_ammo, weapon_slot, Outcome};
    for version in [Version::V102, Version::V104] {
        let mut snap = diagnostics::seed(version, 2); // drawn saber
        snap.stats[4] = (1 << 2) | (1 << 3) | (1 << 4); // saber, bryar, blaster
        let predicted = snap.clone();
        let mut select = 2;
        assert_eq!(weapon_slot(version, &snap, &predicted, &mut select, 1), Ok(Outcome::SaberSwitch));
        assert_eq!(weapon_slot(version, &snap, &predicted, &mut select, 2), Ok(Outcome::Selected));
        assert_eq!(select, 3, "slot 2 is WP_BRYAR_PISTOL");
        assert_eq!(weapon_slot(version, &snap, &predicted, &mut select, 3), Ok(Outcome::Selected));
        assert_eq!(select, 4, "slot 3 is WP_BLASTER");
        assert_eq!(weapon_slot(version, &snap, &predicted, &mut select, 4), Ok(Outcome::Unchanged));
        assert_eq!(select, 4, "unowned disruptor");
        assert_eq!(weapon_slot(version, &snap, &predicted, &mut select, 16), Ok(Outcome::Ignored));
        for expected in [2, 3, 4] {
            assert_eq!(cycle_weapon(version, &snap, &predicted, &mut select, true), Ok(Outcome::Selected));
            assert_eq!(select, expected);
        }
        assert_eq!(cycle_weapon(version, &snap, &predicted, &mut select, false), Ok(Outcome::Selected));
        assert_eq!(select, 3);
        // CG_OutOfAmmoChange (cg_autoswitch 1) from the blaster: best other safe weapon.
        out_of_ammo(version, &predicted, &mut select, 4, 1).unwrap();
        assert_eq!(select, 3);

        // Force: forcePowersKnown = heal, speed, push; levitation never selectable.
        snap.set_field_bits("fd.forcePowersKnown", (1 << 0) | (1 << 1) | (1 << 2) | (1 << 3)).unwrap();
        snap.set_field_bits("fd.forcePowerSelected", 0).unwrap();
        let mut force = -1;
        assert_eq!(cycle_force(version, &snap, &mut force, true), Ok(Outcome::Selected));
        assert!(matches!(force, 2 | 3), "{force}");
        let next = force;
        assert_eq!(cycle_force(version, &snap, &mut force, true), Ok(Outcome::Selected));
        assert!(matches!(force, 2 | 3) && force != next, "{force}");
        assert_eq!(cycle_force(version, &snap, &mut force, false), Ok(Outcome::Selected));
        assert_eq!(force, next);

        // Inventory tags: seeker (1) and medpac (3) owned; -1 starts from STAT_HOLDABLE_ITEM.
        snap.stats[2] = (1 << 1) | (1 << 3);
        snap.stats[1] = 0;
        let mut item = -1;
        assert_eq!(cycle_inventory(version, &snap, &mut item, true), Ok(Outcome::Selected));
        assert_eq!(item, 1);
        assert_eq!(cycle_inventory(version, &snap, &mut item, true), Ok(Outcome::Selected));
        assert_eq!(item, 3);
        assert_eq!(cycle_inventory(version, &snap, &mut item, true), Ok(Outcome::Selected));
        assert_eq!(item, 1);

        // Follow mode ignores every selection command.
        snap.set_field_bits("pm_flags", 4096).unwrap();
        let before = (select, force, item);
        assert_eq!(cycle_weapon(version, &snap, &predicted, &mut select, true), Ok(Outcome::Ignored));
        assert_eq!(cycle_force(version, &snap, &mut force, true), Ok(Outcome::Ignored));
        assert_eq!(cycle_inventory(version, &snap, &mut item, true), Ok(Outcome::Ignored));
        assert_eq!((select, force, item), before);
    }
}
#[test]
fn native_force_power_legalization_matches_server_rank_check() {
    use select::{legalize_force_powers, ForceRules};
    let rules = ForceRules { max_rank: 6, free_saber: false, team_force: 0, gametype: 0, force_disable: 0 };
    for version in [Version::V102, Version::V104] {
        // The default rank-7 configuration is capped for an `nfr 6` server...
        let (legal, was_legal) = legalize_force_powers(version, version, "7-1-032330000000001333", rules).unwrap();
        assert!(!was_legal);
        assert!(legal.starts_with("6-"), "{legal}");
        // ...and the rewritten userinfo passes WP_InitForcePowers' next check.
        assert_eq!(legalize_force_powers(version, version, &legal, rules).unwrap(), (legal.clone(), true));
    }
    assert!(legalize_force_powers(Version::V104, Version::V104, &"9".repeat(200), rules).is_err());
}
#[test]
fn native_snap_vector_matches_jk2mv_engine() {
    unsafe extern "C" {
        fn mvsdk_trap_SnapVector(v: *mut f32);
    }
    // jk2mv Sys_SnapVector: round-half-even through an integer, never -0.0.
    let mut v = [-0.4_f32, 2.5, 3.5];
    unsafe { mvsdk_trap_SnapVector(v.as_mut_ptr()) };
    assert_eq!(v.map(f32::to_bits), [0.0_f32, 2.0, 4.0].map(f32::to_bits));
    let mut v = [-2.5_f32, -1.6, 123456.7];
    unsafe { mvsdk_trap_SnapVector(v.as_mut_ptr()) };
    assert_eq!(v, [-2.0, -2.0, 123457.0]);
}
/// FixtureWorld plus inline models: `solid` lists models whose box trace
/// starts solid; `panic_model` makes CM_BoxTrace fail.
#[derive(Clone, Default)]
struct TriggerWorld {
    solid: Vec<i32>,
    panic_model: bool,
}
impl TraceWorld for TriggerWorld {
    fn trace(&mut self, q: crate::TraceQuery) -> TraceResult {
        diagnostics::FixtureWorld::default().trace(q)
    }
    fn point_contents(&mut self, p: [f32; 3], pass: i32) -> i32 {
        diagnostics::FixtureWorld::default().point_contents(p, pass)
    }
    fn trace_model(&mut self, q: crate::TraceQuery, index: i32) -> TraceResult {
        assert_eq!((q.start, q.end, q.mask), (q.end, q.start, -1));
        if self.panic_model {
            panic!("intentional model trace panic");
        }
        let mut t = TraceResult::clear(q.end);
        t.start_solid = i32::from(self.solid.contains(&index));
        t
    }
}

const PAD: i32 = 40;
const TELEPORTER: i32 = 41;
const ITEM: i32 = 42;

/// A jump pad (model *5), a teleporter (model *6) and a blaster item at the
/// seed player's origin, as wire entities.
fn trigger_entities(version: Version) -> (Vec<EntityState>, i32) {
    let blaster = (1..64)
        .find(|&i| item(i).is_some_and(|item| item.info.classname == "weapon_blaster"))
        .unwrap();
    let entity = |number, fields: &[(&str, u32)]| {
        let mut state = EntityState::zero(version, number);
        for &(name, bits) in fields {
            state.set_field_bits(name, bits).unwrap();
        }
        state
    };
    let bmodel = 0xffffff; // SOLID_BMODEL
    (
        vec![
            entity(PAD, &[("eType", 10), ("solid", bmodel), ("modelindex", 5), ("origin2[2]", 900f32.to_bits())]),
            entity(TELEPORTER, &[("eType", 11), ("solid", bmodel), ("modelindex", 6)]),
            entity(ITEM, &[("eType", 2), ("modelindex", blaster as u32), ("pos.trBase[2]", 24f32.to_bits())]),
        ],
        blaster,
    )
}

#[test]
fn native_trigger_touch_predicts_jump_pad_teleporter_and_item_pickup() {
    for version in [Version::V102, Version::V104] {
        let mut own = diagnostics::seed(version, 2);
        own.stats[4] = 1 << 2; // STAT_WEAPONS: saber only
        let settings = Settings::new(version);
        let cmd = |time| UserCmd { server_time: time, weapon: 2, ..Default::default() };
        let (entities, blaster) = trigger_entities(version);
        let blaster_tag = item(blaster).unwrap().info.tag;
        let mut player = Player::new(version, &animations(), &own).unwrap();
        player.transition_context(&own, &[]).unwrap();
        player.transition_world(&entities, &[], 0).unwrap();
        player.set_frame(&Frame { cg_time: 1010, ..Default::default() });
        player.set_triggers(&[ITEM, 7, PAD, TELEPORTER]);
        let seed = player.try_clone().unwrap();
        let pad_only = TriggerWorld { solid: vec![5], ..Default::default() };

        // Host and direct oracle run the same per-command touch schedule.
        verify_replay(&seed, &[cmd(1016), cmd(1032)], &settings, || pad_only.clone()).unwrap();

        // An unsent preview never commits the pickup.
        let mut preview = seed.try_clone().unwrap();
        preview.predict(cmd(1016), &settings, &mut pad_only.clone()).unwrap();
        assert_eq!(preview.predicted_nodraw(), [ITEM]);
        assert!(player.predicted_nodraw().is_empty());

        player.predict(cmd(1016), &settings, &mut pad_only.clone()).unwrap();
        let after = player.network();
        assert_eq!(after.field_bits("velocity[2]"), Some(900f32.to_bits()));
        assert_eq!(after.field_bits("jumppad_ent"), Some(PAD as u32));
        assert!(!player.hyperspace());
        assert_eq!(player.predicted_nodraw(), [ITEM]);
        assert_ne!(after.stats[4] & (1 << blaster_tag), 0);
        assert_eq!(after.ammo[blaster_tag as usize], 999);
        let pickups = |state: &State| {
            (0..2).filter(|slot| state.field_bits(&format!("eventParms[{slot}]")) == Some(ITEM as u32)).count()
        };
        assert_eq!(pickups(&after), 1);
        // EF_NODRAW keeps the item from being picked up again.
        player.predict(cmd(1032), &settings, &mut pad_only.clone()).unwrap();
        assert_eq!(player.network().field_bits("eventSequence"), after.field_bits("eventSequence"));

        // A failing model trace rolls back the touch with the playerstate.
        let mut failing = seed.try_clone().unwrap();
        let before = failing.native_bytes();
        let error = failing
            .predict(cmd(1016), &settings, &mut TriggerWorld { panic_model: true, ..Default::default() })
            .unwrap_err();
        assert!(error.contains("panicked"), "{error}");
        assert_eq!(failing.native_bytes(), before);
        assert!(failing.predicted_nodraw().is_empty());

        // Teleporter contact is cg.hyperspace for this frame only.
        let mut teleported = seed.try_clone().unwrap();
        teleported
            .predict(cmd(1016), &settings, &mut TriggerWorld { solid: vec![6], ..Default::default() })
            .unwrap();
        assert!(teleported.hyperspace());
        teleported.set_frame(&Frame { cg_time: 1030, ..Default::default() });
        assert!(!teleported.hyperspace());

        // The server's next transition of the item replaces its currentState.
        player.transition_world(&entities[2..], &[], 0).unwrap();
        assert!(player.predicted_nodraw().is_empty());
        // A slot transitioned to another type stops being a trigger.
        player.transition_world(&[], &[PAD], 0).unwrap();
        player.set_triggers(&[PAD]);
        player.predict(cmd(1048), &settings, &mut pad_only.clone()).unwrap();
        assert_eq!(player.network().field_bits("jumppad_ent"), Some(0));
    }
}
