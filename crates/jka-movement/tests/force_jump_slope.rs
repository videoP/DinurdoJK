#[allow(dead_code)]
mod support;
use jka_movement::*;
use support::{animations, player, Map};

fn ceiling() -> CollisionWorld {
    let mut brushes = vec![
        SourceCollisionBrush {
            mins: [-4096., -4096., -64.],
            maxs: [4096., 4096., 0.],
            contents: 1,
            planes: vec![
                SourceCollisionPlane {
                    normal: [0., 0., 1.],
                    distance: 0.,
                    surface_flags: 0,
                },
                SourceCollisionPlane {
                    normal: [0., 0., -1.],
                    distance: 64.,
                    surface_flags: 0,
                },
            ],
        },
        SourceCollisionBrush {
            mins: [-4096., -4096., 100.],
            maxs: [4096., 4096., 4096.],
            contents: 1,
            planes: vec![
                SourceCollisionPlane {
                    normal: [-0.15, 0., -0.988686],
                    distance: -100.,
                    surface_flags: 0,
                },
                SourceCollisionPlane {
                    normal: [0., 0., 1.],
                    distance: 4096.,
                    surface_flags: 0,
                },
            ],
        },
    ];
    for brush in &mut brushes {
        for normal in [[1., 0., 0.], [-1., 0., 0.], [0., 1., 0.], [0., -1., 0.]] {
            brush.planes.push(SourceCollisionPlane {
                normal,
                distance: 4096.,
                surface_flags: 0,
            });
        }
    }
    CollisionWorld::from_source_brushes(brushes).unwrap()
}

fn compare_unchanged(
    mut world: CollisionWorld,
    settings: PredictSettings,
    jump_level: u8,
    held: bool,
    prediction: bool,
) {
    let movement = animations();
    let mut old = player(24.125);
    old.configure(&settings).unwrap();
    old.set_force_jump_level(jump_level).unwrap();
    let mut new = old.clone();
    new.set_local_force_jump_slope_fix(true);
    for n in 1..=240 {
        let time = n * 1000 / 125;
        let cmd = UserCmd {
            server_time: time,
            weapon: 3,
            forward_move: 127,
            up_move: if n > 5 && (held || n < 8) { 127 } else { 0 },
            ..Default::default()
        };
        if prediction {
            movement
                .predict(&mut old, cmd, &settings, &mut world)
                .unwrap();
            movement
                .predict(&mut new, cmd, &settings, &mut world)
                .unwrap();
        } else {
            movement
                .step_command_with_msec(&mut old, cmd, 8, &mut world)
                .unwrap();
            movement
                .step_command_with_msec(&mut new, cmd, 8, &mut world)
                .unwrap();
        }
        assert_eq!(old.network(), new.network(), "state diverged at {time}ms");
    }
}

#[test]
fn flat_force_jumps_and_vertical_walls_are_bit_identical() {
    for level in 0..=3 {
        for held in [false, true] {
            compare_unchanged(
                Map::floor().world(),
                PredictSettings::default(),
                level,
                held,
                false,
            );
            compare_unchanged(
                Map::floor()
                    .box_brush([150., -400., 0.], [180., 400., 1000.], 1)
                    .world(),
                PredictSettings::default(),
                level,
                held,
                false,
            );
        }
    }
}

#[test]
fn ground_ramps_and_slope_sliding_are_bit_identical() {
    for angle in [15., 40., 60.] {
        for level in [0, 3] {
            compare_unchanged(
                support::ramp(angle),
                PredictSettings::default(),
                level,
                false,
                false,
            );
        }
    }
}

#[test]
fn remote_prediction_ignores_local_fix_for_both_backends_and_fixed_steps() {
    for backend in [0, 1] {
        for fixed in [0, 1] {
            for float in [0, 1, 2] {
                let settings = PredictSettings {
                    backend,
                    server_mod: backend,
                    pmove_fixed: fixed,
                    pmove_float: float,
                    ..Default::default()
                };
                compare_unchanged(ceiling(), settings, 3, true, true);
            }
        }
    }
}

#[test]
fn regular_jumps_do_not_gain_force_jump_impulses_on_overhead_slopes() {
    compare_unchanged(ceiling(), PredictSettings::default(), 0, true, false);
}

#[test]
fn correction_preserves_three_millisecond_reference_and_reduces_rate_dependence() {
    let movement = animations();
    let peak = |msec: i32, fix: bool| {
        let mut p = player(24.125);
        p.set_local_force_jump_slope_fix(fix);
        let mut world = ceiling();
        let mut maximum = 0_f32;
        for time in (msec..=1980).step_by(msec as usize) {
            let cmd = UserCmd {
                server_time: time,
                weapon: 3,
                forward_move: 127,
                up_move: 127,
                ..Default::default()
            };
            let v = movement
                .step_command_with_msec(&mut p, cmd, msec, &mut world)
                .unwrap();
            maximum = maximum.max(v.velocity[0].hypot(v.velocity[1]));
        }
        (maximum, p.network())
    };
    let high = peak(3, false);
    let corrected_high = peak(3, true);
    assert_eq!(
        high, corrected_high,
        "3ms reference must remain bit identical"
    );
    let low = peak(33, false).0;
    let corrected_low = peak(33, true).0;
    assert!(
        (corrected_low - high.0).abs() < (low - high.0).abs() * 0.1,
        "low={low}, corrected={corrected_low}, reference={}",
        high.0
    );
}

#[test]
fn qw_and_pjk_keep_their_existing_ramp_jump_branch() {
    let movement = animations();
    for style in [2, 5] {
        let mut old = player(24.125);
        old.configure(&PredictSettings {
            backend: 1,
            server_mod: 1,
            ..Default::default()
        })
        .unwrap();
        let mut net = old.network();
        net.stats[11] = 1;
        net.stats[13] = style;
        old.set_network(&net).unwrap();
        let mut new = old.clone();
        new.set_local_force_jump_slope_fix(true);
        let mut world = ceiling();
        for n in 1..=200 {
            let cmd = UserCmd {
                server_time: n * 8,
                forward_move: 127,
                up_move: if n > 5 { 127 } else { 0 },
                ..Default::default()
            };
            movement
                .step_command_with_msec(&mut old, cmd, 8, &mut world)
                .unwrap();
            movement
                .step_command_with_msec(&mut new, cmd, 8, &mut world)
                .unwrap();
            assert_eq!(old.network(), new.network());
        }
    }
}

#[test]
fn remote_scope_does_not_depend_on_the_local_testing_toggle() {
    let movement = animations();
    for style in (0..19).filter(|style| *style != 9) {
        let settings = PredictSettings {
            backend: 1,
            server_mod: 1,
            ..Default::default()
        };
        let mut old = player(24.125);
        old.configure(&settings).unwrap();
        let mut net = old.network();
        net.stats[11] = 1;
        net.stats[13] = style;
        old.set_network(&net).unwrap();
        let mut new = old.clone();
        new.set_local_force_jump_slope_fix(true);
        let mut world = ceiling();
        for n in 1..=100 {
            let cmd = UserCmd {
                server_time: n * 8,
                forward_move: 127,
                up_move: if n > 5 { 127 } else { 0 },
                ..Default::default()
            };
            movement
                .predict(&mut old, cmd, &settings, &mut world)
                .unwrap();
            movement
                .predict(&mut new, cmd, &settings, &mut world)
                .unwrap();
            assert_eq!(old.network(), new.network(), "style {style}, step {n}");
        }
    }
}

#[test]
fn prediction_correction_is_scoped_to_japro_race_mode() {
    let movement = animations();
    for (backend, server_mod, race, corrected) in [
        (0, 0, 0, false),
        (0, 0, 1, false),
        (1, 0, 1, false),
        (1, 2, 1, false),
        (1, 1, 0, false),
        (1, 1, 1, true),
    ] {
        let settings = PredictSettings {
            backend,
            server_mod,
            ..Default::default()
        };
        let mut authority = player(24.125);
        authority.configure(&settings).unwrap();
        let mut net = authority.network();
        net.stats[11] = race;
        net.stats[13] = 1;
        authority.set_network(&net).unwrap();
        let mut predicted = authority.clone();
        let mut legacy = authority.clone();
        authority.set_local_force_jump_slope_fix(corrected);
        let mut world = ceiling();
        let mut observed_change = false;
        for n in 1..=100 {
            let cmd = UserCmd {
                server_time: n * 8,
                weapon: 3,
                forward_move: 127,
                up_move: if n > 5 { 127 } else { 0 },
                ..Default::default()
            };
            let a = movement
                .step_command_with_msec(&mut authority, cmd, 8, &mut world)
                .unwrap();
            movement
                .predict(&mut predicted, cmd, &settings, &mut world)
                .unwrap();
            let p = predicted.view();
            let old = movement
                .step_command_with_msec(&mut legacy, cmd, 8, &mut world)
                .unwrap();
            assert_eq!(
                a.origin, p.origin,
                "scope backend={backend} mod={server_mod} race={race} at {n}"
            );
            assert_eq!(a.velocity, p.velocity);
            observed_change |= p.velocity != old.velocity;
        }
        assert_eq!(
            observed_change, corrected,
            "scope backend={backend} mod={server_mod} race={race}"
        );
    }
}
