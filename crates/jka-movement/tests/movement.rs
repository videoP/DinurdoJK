mod support;
use jka_movement::{JoinMode, PlayerState, TraceQuery, TraceWorld, PMF_DUCKED, PMF_ROLLING};
use support::*;

#[test]
fn ground_acceleration_friction_and_crouch() {
    let movement = animations();
    let mut world = Map::floor().world();
    let mut p = player(24.125);
    let initial = tick(&movement, &mut p, &mut world, 0, 0, 0);
    assert!(initial.grounded());
    let first = tick(&movement, &mut p, &mut world, 127, 0, 0);
    assert_eq!(first.velocity, [20.0, 0.0, 0.0]);
    for _ in 0..125 {
        tick(&movement, &mut p, &mut world, 127, 0, 0);
    }
    assert_eq!(p.view().velocity[0], 250.0);
    assert_eq!(p.view().origin[2], initial.origin[2]);
    for _ in 0..125 {
        tick(&movement, &mut p, &mut world, 0, 0, 0);
    }
    assert_eq!(p.view().velocity, [0.0; 3]);
    let duck = tick(&movement, &mut p, &mut world, 0, 0, -127);
    assert_ne!(duck.pm_flags & PMF_DUCKED, 0);
    assert_eq!(
        (duck.mins[2], duck.maxs[2], duck.view_height),
        (-24.0, 16.0, 12)
    );
    let stand = tick(&movement, &mut p, &mut world, 0, 0, 0);
    assert_eq!((stand.maxs[2], stand.view_height), (40.0, 36));
}

#[test]
fn solid_wall_stops_forward_motion_and_allows_sliding() {
    let movement = animations();
    let mut world = Map::floor()
        .box_brush([100.0, -4096.0, 0.0], [120.0, 4096.0, 1024.0], 1)
        .world();
    let mut p = player(24.125);
    for _ in 0..125 {
        tick(&movement, &mut p, &mut world, 127, 0, 0);
    }
    assert!((84.7..=85.0).contains(&p.view().origin[0]));
    for _ in 0..125 {
        tick(&movement, &mut p, &mut world, 127, 127, 0);
    }
    assert!(p.view().origin[0] <= 85.0);
    assert!(p.view().origin[1] < -100.0);
}

#[test]
fn steps_up_18_units_but_not_32() {
    let movement = animations();
    for (height, crosses) in [(18.0, true), (32.0, false)] {
        let mut world = Map::floor()
            .box_brush([80.0, -256.0, 0.0], [512.0, 256.0, height], 1)
            .world();
        let mut p = player(24.125);
        for _ in 0..125 {
            tick(&movement, &mut p, &mut world, 127, 0, 0);
        }
        assert_eq!(p.view().origin[0] > 100.0, crosses, "{:?}", p.view());
        assert!(p.view().grounded());
    }
}

#[test]
fn falls_lands_force_jumps_and_releases_jump() {
    let movement = animations();
    let mut world = Map::floor().world();
    let mut p = player(300.0);
    let first = tick(&movement, &mut p, &mut world, 0, 0, 0);
    assert!(!first.grounded());
    assert!(first.velocity[2] < 0.0);
    for _ in 0..200 {
        tick(&movement, &mut p, &mut world, 0, 0, 0);
    }
    assert!(p.view().grounded());
    let ground = p.view().origin[2];
    let mut max_z = ground;
    let mut jumped = false;
    for i in 0..600 {
        let view = tick(
            &movement,
            &mut p,
            &mut world,
            0,
            0,
            if i < 200 { 127 } else { 0 },
        );
        max_z = max_z.max(view.origin[2]);
        jumped |= !view.grounded();
    }
    assert!(jumped && p.view().grounded());
    assert!(max_z - ground > 350.0, "{}", max_z - ground);
    assert!(p.view().force_power < 100);
}

#[test]
fn running_crouch_uses_stock_roll_state() {
    let movement = animations();
    let mut world = Map::floor().world();
    let mut p = player(24.125);
    for _ in 0..70 {
        tick(&movement, &mut p, &mut world, 127, 0, 0);
    }
    let roll = tick(&movement, &mut p, &mut world, 127, 0, -127);
    assert_ne!(roll.pm_flags & PMF_ROLLING, 0, "{roll:?}");
    assert!(roll.legs_timer > 0);
    let start = roll.origin[0];
    for _ in 0..20 {
        tick(&movement, &mut p, &mut world, 0, 0, 0);
    }
    assert!(p.view().origin[0] > start);
}

#[test]
fn water_and_spectator_use_original_pmove_branches() {
    let movement = animations();
    let mut world = Map::floor()
        .box_brush([-512.0, -512.0, 0.0], [512.0, 512.0, 160.0], 4)
        .world();
    assert_eq!(world.point_contents([0.0, 0.0, 80.0], 0), 4);
    let mut p = player(80.0);
    let water = tick(&movement, &mut p, &mut world, 127, 0, 0);
    assert_eq!(water.water_level, 3);
    let mut spectator = PlayerState::spawn([0.0, 0.0, 300.0], 0.0, JoinMode::Spectator).unwrap();
    // OpenJK ClientSpawn mirrors sess.sessionTeam into PERS_TEAM.
    assert_eq!(spectator.entity_view().team, 3);
    for _ in 0..125 {
        tick(&movement, &mut spectator, &mut world, 127, 0, 0);
    }
    assert_eq!(spectator.view().origin[2], 300.0);
    assert!(spectator.view().origin[0] > 100.0);
}

#[test]
fn cloned_full_state_replays_exactly_and_worlds_are_independent() {
    let movement = animations();
    let mut a = Map::floor().world();
    let mut b = Map::floor()
        .box_brush([200.0, -512.0, 0.0], [300.0, 512.0, 512.0], 1)
        .world();
    let mut first = player(24.125);
    for _ in 0..30 {
        tick(&movement, &mut first, &mut a, 127, 0, 0);
    }
    let mut second = first.clone();
    for i in 0..200 {
        let up = if i < 70 { 127 } else { 0 };
        let x = tick(&movement, &mut first, &mut a, 127, 0, up);
        let y = tick(&movement, &mut second, &mut a, 127, 0, up);
        assert_eq!(x, y);
        assert_ne!(
            b.point_contents([250.0, 0.0, 100.0], 0),
            a.point_contents([250.0, 0.0, 100.0], 0)
        );
    }
}

#[test]
fn compiled_brush_trace_reports_startsolid_and_ignores_water_mask() {
    let mut world = Map::floor().world();
    let query = TraceQuery {
        start: [0.0, 0.0, -10.0],
        end: [0.0, 0.0, -10.0],
        mins: [0.0; 3],
        maxs: [0.0; 3],
        pass_entity: 0,
        mask: 1,
    };
    let trace = world.trace(query);
    assert_eq!(trace.start_solid, 1);
    assert_eq!(trace.all_solid, 1);
    assert_eq!(world.trace(TraceQuery { mask: 4, ..query }).fraction, 1.0);
}

#[test]
fn wall_run_and_wall_flip_preserve_native_animation_transitions() {
    let movement = animations();
    for (forward, name) in [(127, "BOTH_WALL_RUN_RIGHT"), (0, "BOTH_WALL_FLIP_RIGHT")] {
        let mut world = Map::floor()
            .box_brush([-1024.0, -64.0, 0.0], [1024.0, -20.0, 1024.0], 1)
            .world();
        let mut p = player(24.125);
        tick(&movement, &mut p, &mut world, 0, 0, 0);
        let view = tick(&movement, &mut p, &mut world, forward, 127, 127);
        assert_eq!(view.legs_animation_name(), name, "{view:?}");
        assert!(view.velocity[2] > 300.0);
        for _ in 0..20 {
            tick(&movement, &mut p, &mut world, forward, 127, 0);
        }
        assert!(p.view().origin[2] > 24.125);
    }
}

#[test]
fn normal_jump_reference_and_knockback_timer() {
    let movement = animations();
    let mut world = Map::floor().world();
    let mut p = player(24.125);
    p.set_force_jump_level(0).unwrap();
    tick(&movement, &mut p, &mut world, 0, 0, 0);
    let mut apex = 0.0f32;
    let mut apex_tick = 0;
    let mut land_tick = 0;
    for i in 1..=125 {
        let view = tick(
            &movement,
            &mut p,
            &mut world,
            0,
            0,
            if i == 1 { 127 } else { 0 },
        );
        if view.origin[2] > apex {
            apex = view.origin[2];
            apex_tick = i;
        }
        if view.grounded() && land_tick == 0 {
            land_tick = i;
        }
    }
    // Reference: unmodified OpenJK 1a6a6434, fixed 8 ms, integer velocity snapping.
    // Preserve the snapping quirk rather than replacing this with a ballistic formula.
    assert!(((apex - 24.125) - 33.6848).abs() < 0.0001);
    assert_eq!((apex_tick, land_tick), (37, 75));
    p.apply_knockback([500.0, 0.0, 100.0], 200).unwrap();
    for _ in 0..25 {
        tick(&movement, &mut p, &mut world, 0, 0, 0);
    }
    assert_eq!(p.view().pm_time, 0);
    assert_eq!(p.view().pm_flags & 64, 0);
}

#[test]
fn offline_speed_rage_and_regeneration_are_separate_from_prediction() {
    use jka_movement::{MovementPower, TICK_MSEC};
    let movement = animations();
    let mut world = Map::floor().world();
    let mut p = player(24.125);
    p.offline_force_tick(TICK_MSEC, Some(MovementPower::Speed))
        .unwrap();
    assert_ne!(p.view().active_powers & (1 << 2), 0);
    assert!(p.view().force_power < 100);
    for _ in 0..125 {
        tick(&movement, &mut p, &mut world, 127, 0, 0);
    }
    assert!(p.view().velocity[0] > 400.0);
    for _ in 0..80 {
        tick(&movement, &mut p, &mut world, 0, 0, 0);
    }
    p.offline_force_tick(
        p.view().command_time + TICK_MSEC,
        Some(MovementPower::Speed),
    )
    .unwrap();
    assert_eq!(p.view().active_powers & (1 << 2), 0);
    tick(&movement, &mut p, &mut world, 0, 0, 0);
    p.offline_force_tick(p.view().command_time + TICK_MSEC, Some(MovementPower::Rage))
        .unwrap();
    assert_ne!(p.view().active_powers & (1 << 8), 0);
    let health = p.view().health;
    for _ in 0..210 {
        tick(&movement, &mut p, &mut world, 127, 0, 0);
        p.offline_force_tick(p.view().command_time + TICK_MSEC, None)
            .unwrap();
    }
    assert!(p.view().health < health);
    p.offline_force_tick(p.view().command_time + TICK_MSEC, Some(MovementPower::Rage))
        .unwrap();
    assert_eq!(p.view().active_powers & (1 << 8), 0);
    assert!(p.view().rage_recovery > p.view().command_time);
    let power = p.view().force_power;
    for _ in 0..500 {
        tick(&movement, &mut p, &mut world, 0, 0, 0);
        p.offline_force_tick(p.view().command_time + TICK_MSEC, None)
            .unwrap();
    }
    assert!(p.view().force_power > power);
}

#[test]
fn offline_force_regenerates_from_exactly_zero_at_50ms_cadence() {
    use jka_movement::{MovementPower, TICK_MSEC};
    let movement = animations();
    let mut world = Map::floor().world();
    let mut p = player(24.125);

    // Two level-three Force Speed activations cost 50 each. Advance Pmove
    // without an authority tick between toggles so regeneration cannot mask
    // the exhausted-resource boundary being tested here.
    p.offline_force_tick(TICK_MSEC, Some(MovementPower::Speed)).unwrap();
    assert_eq!(p.view().force_power, 50);
    tick(&movement, &mut p, &mut world, 0, 0, 0);
    for _ in 0..200 {
        tick(&movement, &mut p, &mut world, 0, 0, 0);
    }
    p.offline_force_tick(p.view().command_time + TICK_MSEC, Some(MovementPower::Speed)).unwrap();
    assert_eq!(p.view().active_powers & (1 << 2), 0);
    tick(&movement, &mut p, &mut world, 0, 0, 0);
    p.offline_force_tick(p.view().command_time + TICK_MSEC, Some(MovementPower::Speed)).unwrap();
    assert_eq!(p.view().force_power, 0);
    tick(&movement, &mut p, &mut world, 0, 0, 0);

    // Speed is allowed to stay active at zero (stock behaviour) until it is
    // toggled off or expires; after stopping, power must recover from zero.
    for _ in 0..200 {
        tick(&movement, &mut p, &mut world, 0, 0, 0);
    }
    p.offline_force_tick(p.view().command_time + TICK_MSEC, Some(MovementPower::Speed)).unwrap();
    assert_eq!(p.view().active_powers & (1 << 2), 0);
    tick(&movement, &mut p, &mut world, 0, 0, 0);

    for _ in 0..125 {
        let next = p.view().command_time + TICK_MSEC;
        p.offline_force_tick(next, None).unwrap();
        tick(&movement, &mut p, &mut world, 0, 0, 0);
    }
    // 1000ms of idle server time should restore ~20 points, not the 5
    // points from the old hardcoded 200ms regeneration interval.
    assert!((19..=22).contains(&p.view().force_power), "{:?}", p.view());
}

#[test]
fn invalid_collision_references_are_rejected_even_without_pvs() {
    let bytes = Map::floor().bytes();
    let offset = |lump: usize| {
        i32::from_le_bytes(bytes[8 + lump * 8..12 + lump * 8].try_into().unwrap()) as usize
    };
    let mut bad_leaf = bytes.clone();
    bad_leaf[offset(6)..offset(6) + 4].copy_from_slice(&99i32.to_le_bytes());
    assert!(jka_movement::CollisionWorld::from_bsp(&bad_leaf).is_err());
    let mut cycle = bytes.clone();
    cycle[offset(3) + 4..offset(3) + 8].copy_from_slice(&0i32.to_le_bytes());
    assert!(jka_movement::CollisionWorld::from_bsp(&cycle).is_err());
}

#[test]
fn stock_patch_collision_is_used_instead_of_render_triangles() {
    let mut world = patch();
    let trace = world.trace(TraceQuery {
        start: [0.0, 0.0, 200.0],
        end: [0.0, 0.0, -20.0],
        mins: [0.0; 3],
        maxs: [0.0; 3],
        pass_entity: 0,
        mask: 1,
    });
    assert!((60.0..70.0).contains(&trace.end[2]), "{trace:?}");
    assert!(trace.normal[2] > 0.9);
    let movement = animations();
    let mut p = player(160.0);
    for _ in 0..125 {
        tick(&movement, &mut p, &mut world, 0, 0, 0);
    }
    assert!(p.view().grounded());
    assert!(p.view().origin[2] > 80.0);
}

#[test]
fn slope_limit_uses_stock_walkable_normal() {
    let movement = animations();
    for (angle, walkable) in [(30.0, true), (60.0, false)] {
        let mut world = ramp(angle);
        let start = [128.0, 0.0, 400.0];
        let trace = world.trace(TraceQuery {
            start,
            end: [128.0, 0.0, 0.0],
            mins: [-15.0, -15.0, -24.0],
            maxs: [15.0, 15.0, 40.0],
            pass_entity: 0,
            mask: 1,
        });
        assert_eq!(trace.normal[2] >= 0.7, walkable);
        let mut p = PlayerState::spawn(trace.end, 0.0, JoinMode::Player).unwrap();
        let view = tick(&movement, &mut p, &mut world, 0, 0, 0);
        assert_eq!(view.grounded(), walkable, "{view:?}");
    }
}

#[test]
fn cannot_stand_up_into_a_low_ceiling() {
    let movement = animations();
    let mut world = Map::floor().world();
    let mut p = player(24.125);
    tick(&movement, &mut p, &mut world, 0, 0, -127);
    let mut ceiling = Map::floor()
        .box_brush([-128.0, -128.0, 48.0], [128.0, 128.0, 80.0], 1)
        .world();
    let view = tick(&movement, &mut p, &mut ceiling, 0, 0, 0);
    assert_eq!(view.maxs[2], 16.0);
    assert_ne!(view.pm_flags & PMF_DUCKED, 0);
}

#[test]
fn equipped_saber_drives_stock_idle_stance_style_and_crouch_torso() {
    use jka_movement::{SaberMovementInfo, UserCmd, GENCMD_SABERATTACKCYCLE, TICK_MSEC};

    let movement = animations();
    let mut world = Map::floor().world();
    let mut p = player(24.125);
    p.set_saber_movement_info(0, SaberMovementInfo::equipped_default())
        .unwrap();
    p.set_saber_movement_info(1, SaberMovementInfo::default())
        .unwrap();
    assert_ne!(p.entity_view().saber_entity_num, 0);

    let medium = tick(&movement, &mut p, &mut world, 0, 0, 0);
    assert_eq!(medium.torso_animation_name(), "BOTH_STAND2");

    let cmd = UserCmd {
        server_time: p.view().command_time + TICK_MSEC,
        generic_command: GENCMD_SABERATTACKCYCLE,
        ..UserCmd::default()
    };
    movement.step(&mut p, cmd, &mut world).unwrap();
    let strong = tick(&movement, &mut p, &mut world, 0, 0, 0);
    assert_eq!(strong.torso_animation_name(), "BOTH_SABERSLOW_STANCE");

    // Let the stock 300 ms generic-command debounce expire, then strong -> fast.
    for _ in 0..40 {
        tick(&movement, &mut p, &mut world, 0, 0, 0);
    }
    let cmd = UserCmd {
        server_time: p.view().command_time + TICK_MSEC,
        generic_command: GENCMD_SABERATTACKCYCLE,
        ..UserCmd::default()
    };
    movement.step(&mut p, cmd, &mut world).unwrap();
    let fast = tick(&movement, &mut p, &mut world, 0, 0, 0);
    assert_eq!(fast.torso_animation_name(), "BOTH_SABERFAST_STANCE");

    // Crouch locomotion owns the legs, but OpenJK deliberately keeps the
    // current saber stance on the torso while crouch-walking. Give the fixed
    // simulation a few ticks to settle the locomotion transition.
    let mut crouch = tick(&movement, &mut p, &mut world, 20, 0, -127);
    for _ in 0..3 {
        crouch = tick(&movement, &mut p, &mut world, 20, 0, -127);
    }
    assert_eq!(crouch.legs_animation_name(), "BOTH_CROUCH1WALK");
    assert_eq!(crouch.torso_animation_name(), "BOTH_SABERFAST_STANCE");
}

mod local_saber_stance {
    use super::support::*;
    use jka_movement::CollisionWorld;
    use jka_movement::{
        PlayerState, SaberMovementInfo, UserCmd, BUTTON_ATTACK, GENCMD_SABERATTACKCYCLE,
        GENCMD_SABERSWITCH, TICK_MSEC,
    };

    // protocol-26 playerStateFields slots.
    const SABER_ANIM_LEVEL: usize = 23;
    const SABER_DRAW_ANIM_LEVEL: usize = 25;
    const SS_FAST: u32 = 1;
    const SS_MEDIUM: u32 = 2;
    const SS_STRONG: u32 = 3;
    const SS_DUAL: u32 = 6;
    const SS_STAFF: u32 = 7;
    const SFL_TWO_HANDED: i32 = 1 << 4;

    fn levels(p: &PlayerState) -> (u32, u32) {
        let fields = p.network().fields;
        (fields[SABER_ANIM_LEVEL], fields[SABER_DRAW_ANIM_LEVEL])
    }

    fn holstered(p: &PlayerState) -> i32 {
        p.entity_view().saber_holstered
    }

    fn cycle(movement: &jka_movement::PmoveContext, p: &mut PlayerState, world: &mut CollisionWorld) {
        let cmd = UserCmd {
            server_time: p.view().command_time + TICK_MSEC,
            generic_command: GENCMD_SABERATTACKCYCLE,
            ..UserCmd::default()
        };
        movement.step(p, cmd, world).unwrap();
        // Let the stock 300 ms generic-command debounce expire and settle.
        for _ in 0..40 {
            tick(movement, p, world, 0, 0, 0);
        }
    }

    fn equip(p: &mut PlayerState, first: SaberMovementInfo, second: SaberMovementInfo) {
        p.set_saber_movement_info(0, first).unwrap();
        p.set_saber_movement_info(1, second).unwrap();
    }

    #[test]
    fn weapon_one_generic_command_retracts_and_relights_the_saber() {
        let movement = animations();
        let mut world = Map::floor().world();
        let mut p = player(24.125);
        equip(&mut p, SaberMovementInfo::equipped_default(), SaberMovementInfo::default());
        assert_eq!(holstered(&p), 0);

        let toggle = |p: &PlayerState| UserCmd {
            server_time: p.view().command_time + TICK_MSEC,
            generic_command: GENCMD_SABERSWITCH,
            ..UserCmd::default()
        };
        let cmd = toggle(&p);
        movement.step(&mut p, cmd, &mut world).unwrap();
        assert_eq!(holstered(&p), 2);

        // Cmd_ToggleSaber_f applies a 400 ms weapon lockout when holstering.
        // Let both that and the generic-command debounce expire before relighting.
        for _ in 0..55 {
            tick(&movement, &mut p, &mut world, 0, 0, 0);
        }
        let cmd = toggle(&p);
        movement.step(&mut p, cmd, &mut world).unwrap();
        assert_eq!(holstered(&p), 0);
    }

    #[test]
    fn dual_loadout_starts_in_dual_and_cycle_toggles_the_second_saber() {
        let movement = animations();
        let mut world = Map::floor().world();
        let mut p = player(24.125);
        equip(&mut p, SaberMovementInfo::equipped_default(), SaberMovementInfo::equipped_default());
        assert_eq!(levels(&p), (SS_DUAL, SS_DUAL));

        tick(&movement, &mut p, &mut world, 0, 0, 0);
        assert_eq!((holstered(&p), levels(&p)), (0, (SS_DUAL, SS_DUAL)));

        cycle(&movement, &mut p, &mut world);
        assert_eq!((holstered(&p), levels(&p)), (1, (SS_FAST, SS_FAST)));

        cycle(&movement, &mut p, &mut world);
        assert_eq!((holstered(&p), levels(&p)), (0, (SS_DUAL, SS_DUAL)));
    }

    #[test]
    fn dual_second_saber_with_no_manual_deactivate_stays_lit() {
        let movement = animations();
        let mut world = Map::floor().world();
        let mut p = player(24.125);
        equip(
            &mut p,
            SaberMovementInfo::equipped_default(),
            SaberMovementInfo { no_manual_deactivate: 1, ..SaberMovementInfo::equipped_default() },
        );
        cycle(&movement, &mut p, &mut world);
        assert_eq!((holstered(&p), levels(&p)), (0, (SS_DUAL, SS_DUAL)));
    }

    #[test]
    fn staff_cycle_toggles_the_second_blade_and_uses_single_blade_style() {
        let movement = animations();
        let mut world = Map::floor().world();
        let mut p = player(24.125);
        let staff = SaberMovementInfo {
            num_blades: 2,
            styles_learned: 1 << SS_STAFF,
            saber_flags: SFL_TWO_HANDED,
            single_blade_style: SS_MEDIUM as i32,
            ..SaberMovementInfo::equipped_default()
        };
        equip(&mut p, staff, SaberMovementInfo::default());
        assert_eq!(levels(&p), (SS_STAFF, SS_STAFF));

        cycle(&movement, &mut p, &mut world);
        assert_eq!((holstered(&p), levels(&p)), (1, (SS_MEDIUM, SS_MEDIUM)));

        cycle(&movement, &mut p, &mut world);
        assert_eq!((holstered(&p), levels(&p)), (0, (SS_STAFF, SS_STAFF)));
    }

    #[test]
    fn switching_back_to_a_single_saber_restores_a_single_stance() {
        let mut p = player(24.125);
        equip(&mut p, SaberMovementInfo::equipped_default(), SaberMovementInfo::default());
        assert_eq!(levels(&p), (SS_MEDIUM, SS_MEDIUM));
        equip(&mut p, SaberMovementInfo::equipped_default(), SaberMovementInfo::equipped_default());
        assert_eq!(levels(&p), (SS_DUAL, SS_DUAL));
        equip(&mut p, SaberMovementInfo::equipped_default(), SaberMovementInfo::default());
        assert_eq!(levels(&p), (SS_MEDIUM, SS_MEDIUM));
    }

    #[test]
    fn cycling_mid_swing_shows_at_once_but_applies_after_the_swing() {
        let movement = animations();
        let mut world = Map::floor().world();
        let mut p = player(24.125);
        equip(&mut p, SaberMovementInfo::equipped_default(), SaberMovementInfo::default());
        tick(&movement, &mut p, &mut world, 0, 0, 0);

        // Start a swing and keep the button held so weaponTime stays non-zero.
        let attack = |p: &PlayerState, generic_command| UserCmd {
            server_time: p.view().command_time + TICK_MSEC,
            buttons: BUTTON_ATTACK,
            generic_command,
            ..UserCmd::default()
        };
        for _ in 0..4 {
            let cmd = attack(&p, 0);
            movement.step(&mut p, cmd, &mut world).unwrap();
        }
        let cmd = attack(&p, GENCMD_SABERATTACKCYCLE);
        movement.step(&mut p, cmd, &mut world).unwrap();

        // Draw level is the queued style right away; the real stance waits.
        assert_eq!(levels(&p), (SS_MEDIUM, SS_STRONG));

        // Once the swing is over the queued stance is applied.
        for _ in 0..500 {
            tick(&movement, &mut p, &mut world, 0, 0, 0);
        }
        assert_eq!(levels(&p), (SS_STRONG, SS_STRONG));
    }
}

#[test]
fn snapshot_inline_model_handles_are_checked_against_the_loaded_bsp() {
    use jka_movement::{CollisionWorld, EntityClip, ENTITY_NONE};

    // Reproduce the join crash: nine BSP models, but a snapshot references *9.
    let bytes = rewrite_lumps(&Map::floor().bytes(), |lumps| {
        let model = lumps[7].clone();
        for _ in 1..9 {
            lumps[7].extend_from_slice(&model);
        }
    });
    let mut world = CollisionWorld::from_bsp(&bytes).unwrap();
    let query = TraceQuery {
        start: [0.0, 0.0, 100.0],
        end: [0.0, 0.0, -10.0],
        mins: [0.0; 3],
        maxs: [0.0; 3],
        pass_entity: ENTITY_NONE,
        mask: 1,
    };
    for index in [9, 10, -1, 0, 510, 511, 512, i32::MAX] {
        let trace = world.trace_entity(
            query,
            EntityClip::InlineModel {
                index,
                origin: [0.0; 3],
                angles: [0.0; 3],
            },
        );
        assert_eq!(
            trace.fraction, 1.0,
            "invalid model {index} must not collide"
        );
        assert_eq!(trace.end, query.end);
        assert_eq!(trace.entity, ENTITY_NONE);
        assert_eq!(
            (trace.start_solid, trace.all_solid, trace.contents),
            (0, 0, 0)
        );
        assert_eq!(
            world.inline_model_contents(query.end, index, [0.0; 3], [0.0; 3]),
            0
        );
    }
    // The last valid model still clips and reports contents, including a transform.
    let origin = [0.0, 0.0, 20.0];
    let trace = world.trace_entity(
        query,
        EntityClip::InlineModel {
            index: 8,
            origin,
            angles: [0.0; 3],
        },
    );
    assert!(trace.fraction < 1.0);
    assert!((trace.end[2] - 20.125).abs() < 0.001);
    assert_eq!(
        world.inline_model_contents(query.end, 8, origin, [0.0; 3]),
        1
    );
    assert!(
        world.trace(query).fraction < 1.0,
        "world collision remains available"
    );
}

#[test]
fn misc_bsp_handles_include_imported_world_and_inline_models() {
    use jka_movement::{CollisionWorld, EntityClip, ENTITY_NONE};

    let main = rewrite_lumps(&Map::floor().bytes(), |lumps| {
        let model = lumps[7].clone();
        for _ in 1..9 {
            lumps[7].extend_from_slice(&model);
        }
    });
    let child = rewrite_lumps(
        &Map::default()
            .box_brush([-8.0, -16.0, -24.0], [8.0, 16.0, 24.0], 4)
            .bytes(),
        |lumps| {
            let model = lumps[7].clone();
            lumps[7].extend_from_slice(&model);
        },
    );
    let second = Map::default().box_brush([-12.0; 3], [12.0; 3], 1).bytes();
    let mut world = CollisionWorld::from_bsp_with_sub_bsps(
        &main,
        &[
            ("maps/test_child.bsp", &child),
            ("maps/test_second.bsp", &second),
        ],
    )
    .unwrap();
    let mut clone = world.clone();
    let query = TraceQuery {
        start: [250.0, 0.0, 100.0],
        end: [200.0, 0.0, 100.0],
        mins: [0.0; 3],
        maxs: [0.0; 3],
        pass_entity: ENTITY_NONE,
        mask: 4,
    };
    let origin = [200.0, 0.0, 100.0];
    let angles = [0.0, 90.0, 0.0];
    assert_eq!(
        world.trace(query).fraction,
        1.0,
        "imported BSP is not static world geometry"
    );
    for index in [9, 10] {
        let hit = world.trace_entity(
            query,
            EntityClip::InlineModel {
                index,
                origin,
                angles,
            },
        );
        assert!((hit.end[0] - 216.125).abs() < 0.001, "{hit:?}");
        assert_eq!(hit.contents, 4);
        assert_eq!(
            world.inline_model_contents(origin, index, origin, angles),
            4
        );
    }
    let hit = world.trace_entity(
        TraceQuery { mask: 1, ..query },
        EntityClip::InlineModel {
            index: 11,
            origin,
            angles: [0.0; 3],
        },
    );
    assert!((hit.end[0] - 212.125).abs() < 0.001, "{hit:?}");
    assert_eq!(world.inline_model_contents(origin, 12, origin, angles), 0);

    // Loading and freeing a different world must not lose this world's submaps
    // or leave the global temporary box hull pointing at the freed arena.
    let other = Map::floor().world();
    drop(other);
    let box_query = TraceQuery {
        mask: 0x100,
        ..query
    };
    let bbox = EntityClip::Box {
        mins: [-8.0; 3],
        maxs: [8.0; 3],
        origin,
    };
    let box_hit = clone.trace_entity(box_query, bbox);
    assert!((box_hit.end[0] - 208.125).abs() < 0.001, "{box_hit:?}");
    assert_eq!(clone.inline_model_contents(origin, 9, origin, angles), 4);
    assert!(
        world
            .trace_entity(
                query,
                EntityClip::InlineModel {
                    index: 10,
                    origin,
                    angles
                }
            )
            .fraction
            < 1.0
    );
}

#[test]
fn misc_bsp_input_is_validated_before_native_loading() {
    use jka_movement::CollisionWorld;
    let main = Map::floor().bytes();
    assert!(
        CollisionWorld::from_bsp_with_sub_bsps(&main, &[("maps/bad.bsp", b"invalid")]).is_err()
    );
    assert!(CollisionWorld::from_bsp_with_sub_bsps(
        &main,
        &[("maps/child.bsp", &main), ("MAPS/CHILD.BSP", &main)]
    )
    .is_err());
    let too_many_models = rewrite_lumps(&main, |lumps| {
        let model = lumps[7].clone();
        for _ in 1..510 {
            lumps[7].extend_from_slice(&model);
        }
    });
    assert!(
        CollisionWorld::from_bsp_with_sub_bsps(&main, &[("maps/large.bsp", &too_many_models)])
            .is_err()
    );
}

#[test]
fn collision_union_preparation_merges_only_matching_axis_aligned_solids() {
    // Use the stock RBSP builder with adjacent worldspawn solids, no GPU/UI.
    // The map must remain loadable even when the runtime A/B switch is OFF.
    let map = Map::floor()
        .box_brush([0.0, 0.0, 16.0], [64.0, 64.0, 80.0], 1)
        .box_brush([64.0, 0.0, 16.0], [128.0, 64.0, 80.0], 1)
        .box_brush([128.0, 0.0, 16.0], [192.0, 64.0, 80.0], 4);
    let mut world = map.world();
    assert_eq!(world.merged_static_brush_count(), 1);
    // The content 4 volume must not be absorbed into the solid 1 brush.
    assert_eq!(world.point_contents([160.0, 32.0, 40.0], 0), 4);
    assert_eq!(world.point_contents([96.0, 32.0, 40.0], 0), 1);
    // With the cvar at its default (OFF), collision traces use the stock
    // OpenJK map, even though the alternate collision representation exists.
    let trace = world.trace(TraceQuery {
        start: [96.0, 32.0, 150.0],
        end: [96.0, 32.0, 0.0],
        mins: [0.0; 3],
        maxs: [0.0; 3],
        pass_entity: 0,
        mask: 1,
    });
    assert!((trace.end[2] - 80.125).abs() < 0.001, "{trace:?}");
}
