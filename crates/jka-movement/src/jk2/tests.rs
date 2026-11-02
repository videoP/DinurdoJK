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
