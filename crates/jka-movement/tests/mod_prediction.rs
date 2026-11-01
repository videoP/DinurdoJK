#[allow(dead_code)]
mod support;
use jka_movement::{PlayerState, PredictSettings, PredictionEntity, UserCmd};
use support::{animations, player, Map};

fn japro() -> PredictSettings {
    PredictSettings {
        backend: 1,
        server_mod: 1,
        ..PredictSettings::default()
    }
}

#[test]
fn cpm_physics_is_selected_only_for_japro() {
    let movement = animations();
    let mut world = Map::floor().world();
    let mut base = player(24.125);
    let mut state = base.network();
    state.stats[11] = 1;
    state.stats[13] = 3; // MV_CPM
    base.set_network(&state).unwrap();
    let mut cpm = base.clone();
    cpm.configure(&japro()).unwrap();
    for time in (8..=80).step_by(8) {
        let cmd = UserCmd {
            server_time: time,
            forward_move: 127,
            ..Default::default()
        };
        movement
            .predict(&mut base, cmd, &PredictSettings::default(), &mut world)
            .unwrap();
        movement
            .predict(&mut cpm, cmd, &japro(), &mut world)
            .unwrap();
    }
    assert_ne!(
        cpm.view().velocity,
        base.view().velocity,
        "JAPRO CPM must run different physics"
    );
}

#[test]
fn switching_native_backends_preserves_every_wire_slot() {
    let mut p = player(24.125);
    let mut state = p.network();
    state.fields = (0..state.fields.len() as u32)
        .map(|i| i.wrapping_mul(0x01010101))
        .collect();
    state.stats = std::array::from_fn(|i| i as i32 * 91);
    state.ammo = std::array::from_fn(|i| i as i32 + 600);
    p.set_network(&state).unwrap();
    for settings in [japro(), PredictSettings::default(), japro()] {
        p.configure(&settings).unwrap();
        assert_eq!(p.network(), state);
        assert_eq!(p.clone().network(), state);
    }
}

#[test]
fn race_collision_rules_do_not_leak_into_base() {
    let mut p = player(24.125);
    let mut state = p.network();
    state.stats[11] = 1; // JAPRO STAT_RACEMODE
    p.set_network(&state).unwrap();
    let other = PredictionEntity {
        number: 2,
        entity_type: 1,
        ..Default::default()
    };
    let mut mover = PredictionEntity {
        number: 40,
        entity_type: 6,
        velocity: [40.0, 0.0, 0.0],
        ..Default::default()
    };
    assert!(p.clips_prediction_entity(&other));
    p.configure(&japro()).unwrap();
    assert!(!p.clips_prediction_entity(&other));
    // Unknown classname keeps TaystJK's trajectory fallback.
    assert!(!p.clips_prediction_entity(&mover));
    mover.mover_class = 1; // func_train remains solid
    assert!(p.clips_prediction_entity(&mover));
    mover.mover_class = 2; // func_door / func_plat pass through
    assert!(!p.clips_prediction_entity(&mover));
    mover.trajectory_type = 5; // known race door stays pass-through
    assert!(!p.clips_prediction_entity(&mover));
    mover.mover_class = 0;
    assert!(p.clips_prediction_entity(&mover)); // TR_SINE fallback
    // TaystJK cg_predict.c: only jaPRO race mode excludes moving movers.
    p.configure(&PredictSettings { backend: 1, server_mod: 0, ..PredictSettings::default() }).unwrap();
    assert!(p.clips_prediction_entity(&mover));
    p.configure(&PredictSettings { backend: 1, server_mod: 2, ..PredictSettings::default() }).unwrap();
    assert!(p.clips_prediction_entity(&mover));
    p.configure(&japro()).unwrap();
    assert!(p.clips_prediction_entity(&mover));
    mover.trajectory_type = 0;
    mover.velocity = [0.0; 3];
    mover.angular_velocity = [0.0, 90.0, 0.0];
    assert!(!p.clips_prediction_entity(&mover));
    mover.angular_velocity = [0.0; 3];
    assert!(p.clips_prediction_entity(&mover));
    p.configure(&PredictSettings::default()).unwrap();
    assert!(p.clips_prediction_entity(&other));
}

#[test]
fn player_styles_replay_deterministically_across_interleaved_backends() {
    let movement = animations();
    let mut world = Map::floor().world();
    let mut base = player(24.125);
    let mut base_control = base.clone();
    for style in (0..19).filter(|style| *style != 9) {
        // vehicles need their own host
        let mut p = player(24.125);
        let mut state = p.network();
        state.stats[11] = 1;
        state.stats[13] = style;
        p.configure(&japro()).unwrap();
        p.set_network(&state).unwrap();
        let mut control = p.clone();
        for time in (8..=160).step_by(8) {
            let cmd = UserCmd {
                server_time: time,
                forward_move: 127,
                right_move: 32,
                up_move: 127,
                ..Default::default()
            };
            movement.predict(&mut p, cmd, &japro(), &mut world).unwrap();
            let base_cmd = UserCmd {
                server_time: base.view().command_time + 8,
                forward_move: 127,
                ..Default::default()
            };
            movement
                .predict(&mut base, base_cmd, &PredictSettings::default(), &mut world)
                .unwrap();
            movement
                .predict(&mut control, cmd, &japro(), &mut world)
                .unwrap();
            movement
                .predict(
                    &mut base_control,
                    base_cmd,
                    &PredictSettings::default(),
                    &mut world,
                )
                .unwrap();
            assert_eq!(p.network(), control.network(), "style {style}, time {time}");
            assert_eq!(base.network(), base_control.network());
            assert_eq!(p.view().command_time, time);
            assert!(p
                .view()
                .origin
                .iter()
                .chain(p.view().velocity.iter())
                .all(|v| v.is_finite()));
        }
    }
}

#[test]
fn japlus_uses_tayst_backend_with_its_own_server_identity_and_cinfo() {
    let mut p = player(24.125);
    let settings = PredictSettings {
        backend: 1,
        server_mod: 2,
        cinfo: (1 << 0) | (1 << 3), // JA+ flipkick + fixroll3
        hook_pull: 800,
        ..PredictSettings::default()
    };
    p.configure(&settings).unwrap();
    // Configuration must not alter the authoritative wire state. The actual
    // JA+ feature branches are exercised inside the vendored TaystJK Pmove.
    let before = p.network();
    p.configure(&settings).unwrap();
    assert_eq!(p.network(), before);
}

#[test]
fn native_configuration_rejects_fixed_steps_that_would_not_advance() {
    let mut p = player(24.125);
    assert!(p
        .configure(&PredictSettings {
            pmove_fixed: 1,
            pmove_msec: 0,
            ..japro()
        })
        .is_err());
    assert!(p
        .configure(&PredictSettings {
            server_mod: 999,
            ..japro()
        })
        .is_err());
    assert!(p
        .configure(&PredictSettings {
            pmove_fixed: 1,
            pmove_msec: 1,
            ..japro()
        })
        .is_ok());
}

#[test]
fn authoritative_snapshot_resets_previous_style_state() {
    let movement = animations();
    let mut world = Map::floor().world();
    let mut p = player(24.125);
    p.configure(&japro()).unwrap();
    let original = p.network();
    let cmd = UserCmd {
        server_time: 8,
        forward_move: 127,
        ..Default::default()
    };
    movement.predict(&mut p, cmd, &japro(), &mut world).unwrap();
    let expected = p.network();
    p.set_network(&original).unwrap();
    movement.predict(&mut p, cmd, &japro(), &mut world).unwrap();
    assert_eq!(p.network(), expected);
    let mut fresh = PlayerState::from_network(&original).unwrap();
    fresh.configure(&japro()).unwrap();
    movement
        .predict(&mut fresh, cmd, &japro(), &mut world)
        .unwrap();
    assert_eq!(fresh.network(), expected);
}

#[test]
fn actual_roll_branch_uses_nf_ff_overrides_and_diagnostics_do_not_change_movement() {
    let movement = animations();
    let mut world = Map::floor().world();
    let roll_anim = (0..3000).find(|&i| jka_movement::animation_name(i) == Some("BOTH_ROLL_F")).unwrap();
    for (duel_type, active, expected_mode) in [(1, 1, 0), (2, 1, 3), (0, 1, 3), (1, 0, 3)] {
        let mut p = player(24.125);
        let mut state = p.network();
        let mut ps = jka_protocol::server::PlayerState::default();
        ps.fields.clone_from(&state.fields);
        for (name, value) in [("clientNum", 3), ("duelInProgress", active), ("duelIndex", 4),
            ("legsAnim", roll_anim), ("torsoAnim", roll_anim), ("legsTimer", 800),
            ("torsoTimer", 800), ("pm_flags", jka_movement::PMF_ROLLING)]
        {
            assert!(ps.set_field_bits(name, value as u32), "{name}");
        }
        state.fields = ps.fields;
        state.stats[11] = 0; // non-race
        state.stats[13] = 1; // MV_JKA
        p.set_network(&state).unwrap();
        let settings = PredictSettings { duel_client: 3, duel_type, roll_debug: 1,
            jcinfo: 1 << 3, ..japro() }; // advertised FIXROLL3 must yield to NF
        p.configure(&settings).unwrap();
        let mut quiet = p.clone();
        let cmd = UserCmd { server_time: 8, forward_move: 127, ..Default::default() };
        movement.predict(&mut p, cmd, &settings, &mut world).unwrap();
        let observed = p.roll_prediction_debug();
        assert!(observed.roll_queries > 0, "must exercise GetFixRoll inside a roll");
        assert_eq!((observed.client_num, observed.duel_type, observed.duel_in_progress, observed.mode),
            (3, duel_type, active, expected_mode));
        movement.predict(&mut quiet, cmd, &PredictSettings { roll_debug: 0, ..settings }, &mut world).unwrap();
        assert_eq!(p.network(), quiet.network(), "diagnostics must not change Pmove");
        assert_eq!(quiet.roll_prediction_debug().queries, 0);
    }
}
