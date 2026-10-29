use super::*;

fn path() -> DirectorPath {
    let samples = (0..=240)
        .map(|i| PathSample {
            ms: i * 50,
            origin: Vec3::ZERO,
            velocity: Vec3::ZERO,
            ground: true,
            eye_height: 36.0,
            cut: false,
            client: 0,
            yaw: 0.0,
            pitch: 0.0,
        })
        .collect();
    let players = [
        (1, Vec3::new(150.0, 0.0, 0.0)),
        (2, Vec3::new(180.0, 35.0, 0.0)),
        (3, Vec3::new(2400.0, 0.0, 0.0)),
        (4, Vec3::new(2500.0, 0.0, 0.0)),
    ]
    .into_iter()
    .map(|(client, p)| (client, (0..=240).map(|i| (i * 50, p)).collect()))
    .collect();
    DirectorPath {
        map: "test".into(),
        samples,
        players,
        ..Default::default()
    }
}

fn kill(ms: i32, attacker: i32, victim: i32, position: Vec3) -> CombatEvent {
    CombatEvent {
        ms,
        kind: CombatKind::Kill,
        attacker,
        victim,
        position,
        means: MOD_SABER,
    }
}

fn job(path: DirectorPath) -> ClipJob {
    let clip = StudioDemoClip { source_path: None,
        comparison: None,
        race_overlays: Vec::new(),
        pov_trail: Default::default(),
        hud_overrides: Default::default(),
        hide_other_players: false,
        name: "combat-test".into(),
        game: String::new(),
        start: 0.0,
        duration_ms: 12000,
        in_ms: 0,
        out_ms: 12000,
        speed_keys: Vec::new(),
        transition_in: Default::default(),
        audio: Default::default(),
        layer: 0,
    };
    ClipJob {
        clip,
        path: Arc::new(path),
        role: ClipRole::Whole,
    }
}

fn director() -> AutoDirector {
    AutoDirector::new(None, 90.0, StudioDirectorPacing::Balanced, 7)
}

#[test]
fn simultaneous_unrelated_kill_cannot_steal_the_shot_or_slow_motion() {
    let mut path = path();
    // The remote event comes first, including at exactly the same millisecond.
    path.events = vec![
        kill(5000, 3, 4, Vec3::new(2500.0, 0.0, 0.0)),
        kill(5000, 0, 1, Vec3::new(150.0, 0.0, 0.0)),
    ];
    let job = job(path);
    let mut auto = director();
    auto.prepare_focus(&job.path, 0, 12000);
    let intent = auto.intent(&job.path, 3800, 6500, Beat::Kill);
    assert_eq!(intent.kills.len(), 1);
    assert_eq!(intent.primary().unwrap().victim, 1);
    let (_, _, count) = auto.combat_beats(&job);
    assert_eq!(count, 1);
    let shot = auto
        .build_kill(P::Orbit, 3800, 6500, &job, &mut Rng::new(&[7]), &intent)
        .unwrap();
    assert_eq!(shot.client, 1);
    let slowed: Vec<_> = auto
        .engagements
        .iter()
        .flat_map(|fight| fight.kills.iter().map(|kill| kill.victim))
        .collect();
    assert_eq!(slowed, vec![1]);
}

#[test]
fn close_kills_are_counted_and_held_as_one_sequence_through_the_last_impact() {
    let mut path = path();
    path.events = vec![
        kill(5000, 0, 1, Vec3::new(150.0, 0.0, 0.0)),
        kill(5700, 0, 2, Vec3::new(180.0, 35.0, 0.0)),
    ];
    let job = job(path);
    let mut auto = director();
    auto.prepare_focus(&job.path, 0, 12000);
    let (beats, _, count) = auto.combat_beats(&job);
    assert_eq!(count, 2);
    let sequences: Vec<_> = beats.iter().filter(|b| b.2 == Beat::Kill).collect();
    assert_eq!(sequences.len(), 1);
    assert!(sequences[0].0 < 5000 && sequences[0].1 >= 7200);
    let intent = auto.intent(&job.path, sequences[0].0, sequences[0].1, Beat::Kill);
    assert_eq!(intent.kills.len(), 2);
    let keys = slowmo_keys(&[5000, 5700], &HashMap::new(), &job.clip);
    assert!(keys.windows(2).all(|w| w[0].time < w[1].time));
    // The low-speed plateau lasts beyond the final kill.
    assert!(keys.iter().any(|key| key.time >= 6.4 && key.rate == 0.3));
    let shots = auto.direct_combat_clip(&job).0;
    assert!(!shots
        .iter()
        .any(|shot| shot.start > 5.0 && shot.start < 5.7));
    assert!(shots.iter().all(StudioDirectorShot::valid));
}

#[test]
fn mandatory_teleport_and_subject_cuts_survive_short_shot_and_camera_merging() {
    let mut path = path();
    for sample in &mut path.samples {
        if sample.ms >= 7500 {
            sample.origin.x = 1000.0;
        }
        if sample.ms == 7500 {
            sample.cut = true;
        }
        if sample.ms >= 7600 {
            sample.client = 1;
        }
    }
    let job = job(path);
    let mut auto = director();
    let shots = auto.direct_beats(
        &job,
        vec![
            (0, 7100, Beat::Cruise),
            (7100, 7800, Beat::Exchange),
            (7800, 12000, Beat::Cruise),
        ],
    );
    for boundary in [7.5, 7.6] {
        assert!(shots
            .iter()
            .any(|shot| (shot.start - boundary).abs() < 1e-6));
        assert!(!shots
            .iter()
            .any(|shot| shot.start < boundary - 1e-6 && shot.end() > boundary + 1e-6));
    }
    assert!(shots
        .windows(2)
        .all(|w| (w[0].end() - w[1].start).abs() < 1e-6));
    assert_eq!(shots.first().unwrap().start, 0.0);
    assert!((shots.last().unwrap().end() - 12.0).abs() < 1e-6);
    assert!(shots.iter().all(StudioDirectorShot::valid));
}

#[test]
fn visibility_linger_holds_a_short_gap_and_expires_without_using_future_positions() {
    let mut path = path();
    path.players[0]
        .1
        .retain(|(ms, _)| *ms < 4500 || (*ms > 5100 && *ms <= 6000));
    let mut auto = director();
    auto.set_focus(StudioDirectorFocus::Player(1));
    assert!(player_at(&path, 1, 4800).is_none());
    assert_eq!(auto.subject_at(&path, 4800), 1);
    assert_eq!(auto.subject_at(&path, 5400), 1);
    assert_eq!(auto.subject_at(&path, 6800), 0);
    path.players[0].1.retain(|(ms, _)| *ms >= 5500);
    assert!(
        player_at(&path, 1, 5400).is_none(),
        "do not anticipate a spawn"
    );
    assert!(lingering_player(&path, 1, 5400).is_none());
}

#[test]
fn follow_camera_holds_its_baked_composition_during_a_brief_missing_actor() {
    let mut path = path();
    path.players[0].1.retain(|(ms, _)| *ms < 4500 || *ms > 5100);
    let mut shot = StudioDirectorShot::new(0, P::FollowCam, 1, 3.0, 3.0);
    let mut auto = director();
    assert!(auto.plan_follow(
        &mut shot,
        &path,
        3000,
        6000,
        1,
        Some(2),
        [180.0, 60.0, 40.0, 0.4],
        &[]
    ));
    let a = shot.path_at((4800.0 - 3000.0) / 3000.0).unwrap();
    let b = shot.path_at((4900.0 - 3000.0) / 3000.0).unwrap();
    assert_eq!(a, b);
}

#[test]
fn remote_player_teleport_is_never_interpolated_across() {
    let mut path = path();
    path.players[0].1 = vec![(1000, Vec3::ZERO), (1100, Vec3::new(3000.0, 0.0, 0.0))];
    assert_eq!(player_at(&path, 1, 1050), Some(Vec3::ZERO));
    assert_eq!(player_at(&path, 1, 1100), Some(Vec3::new(3000.0, 0.0, 0.0)));
}

#[test]
fn camera_rest_region_and_speed_limits_are_deterministic() {
    fn run() -> (Vec3, Vec3) {
        let mut aim = CameraAim::default();
        let camera = Vec3::ZERO;
        aim.step(camera, Vec3::X * 100.0, 0.05);
        for _ in 0..30 {
            aim.step(camera, forward(5.0, 0.0) * 100.0, 0.05);
        }
        let resting = aim.direction.unwrap();
        let mut speed = aim.speed;
        for _ in 0..60 {
            let previous = aim.direction.unwrap();
            aim.step(camera, forward(70.0, 0.0) * 100.0, 0.05);
            assert!(aim.speed <= 90.0f32.to_radians() + 1e-5);
            assert!((aim.speed - speed).abs() <= 180.0f32.to_radians() * 0.05 + 1e-5);
            assert!(
                previous.angle_between(aim.direction.unwrap())
                    <= 90.0f32.to_radians() * 0.05 + 1e-4
            );
            speed = aim.speed;
        }
        (resting, aim.direction.unwrap())
    }
    let a = run();
    assert_eq!(a, run());
    assert_eq!(a.0, Vec3::X);
    assert!(a.1.angle_between(forward(70.0, 0.0)) < 3.0f32.to_radians());
}

#[test]
fn rectangular_projection_rejects_vertical_overflow_and_values_readable_size() {
    let camera = Vec3::ZERO;
    let look = Vec3::X;
    assert!(
        !frame_point(camera, look, 90.0, Vec3::new(100.0, 0.0, 90.0))
            .unwrap()
            .inside()
    );
    assert!(frame_point(camera, look, 90.0, Vec3::new(100.0, 0.0, 30.0))
        .unwrap()
        .inside());
    assert!(frame_point(camera, look, 90.0, Vec3::new(-100.0, 0.0, 0.0)).is_none());
    let near = frame_point(camera, look, 90.0, Vec3::X * 300.0).unwrap();
    let far = frame_point(camera, look, 90.0, Vec3::X * 3000.0).unwrap();
    assert!(near.height > far.height * 9.0);
    assert!(frame_point(camera, -Vec3::Z, 90.0, -Vec3::Z * 100.0)
        .unwrap()
        .inside());
}

#[test]
fn framing_checks_each_exact_impact_in_a_chain() {
    let mut path = path();
    // At 5371 ms the second victim is outside the view, between regular samples.
    path.players[1].1 = vec![
        (5000, Vec3::new(180.0, 35.0, 0.0)),
        (5371, Vec3::new(180.0, 900.0, 0.0)),
        (5800, Vec3::new(180.0, 35.0, 0.0)),
    ];
    let kills = vec![
        kill(5000, 0, 1, Vec3::new(150.0, 0.0, 0.0)),
        kill(5371, 0, 2, Vec3::new(180.0, 900.0, 0.0)),
    ];
    assert!(shot_sample_times(4000, 6500, 7, &kills).contains(&5371));
    let intent = ShotIntent { subject: 0, kills };
    let mut shot = StudioDirectorShot::new(0, P::Static, -1, 4.0, 2.5);
    shot.position_override = Some(render(Vec3::new(-300.0, 0.0, 50.0)));
    shot.focus_target = Some(render(Vec3::new(100.0, 0.0, 30.0)));
    shot.fov = 70.0;
    assert!(!director().frames_ok(&path, &shot, Beat::Kill, 4000, 6500, &intent));
}

#[test]
fn slowmo_chains_stay_sorted_and_inside_a_trimmed_clip() {
    let mut clip = job(path()).clip;
    clip.in_ms = 4500;
    clip.out_ms = 7300;
    let keys = slowmo_keys(
        &[7200, 5000, 5700, 5700],
        &HashMap::from([(7200, 100.0)]),
        &clip,
    );
    assert!(keys.iter().all(|key| key.time >= 4.5 && key.time <= 7.3));
    assert!(keys.windows(2).all(|w| w[0].time < w[1].time));
    assert!(keys.iter().any(|key| key.rate == 0.2));
    assert_eq!(keys.last().unwrap().rate, 1.0);
}

#[test]
fn generated_subframe_boundaries_remain_valid_after_speed_ramps() {
    let mut shot = StudioDirectorShot::new(0, P::RecordedPov, -1, 1.0, 0.001);
    assert!(!shot.valid());
    shot.auto = true;
    assert!(shot.valid());
}

#[test]
fn best_fight_does_not_switch_back_during_a_short_visibility_gap() {
    let mut path = path();
    path.players[0].1.retain(|(ms, _)| *ms < 4500 || *ms > 5100);
    path.events = vec![kill(4000, 1, 2, Vec3::new(180.0, 35.0, 0.0))];
    let track = best_combat_track(&path, 0, 6500, &HashMap::new());
    assert!(track.iter().any(|&(_, client)| client == 1));
    assert!(!track
        .iter()
        .any(|&(ms, client)| (4500..=5100).contains(&ms) && client == 0));
    path.players.reverse();
    assert_eq!(
        track,
        best_combat_track(&path, 0, 6500, &HashMap::new()),
        "ties must not depend on player ordering"
    );
}

fn visibility_fixture() -> jka_assets::bsp::Visibility {
    fn ints(values: &[i32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
    let mut lumps = vec![Vec::new(); 18];
    lumps[2] = [1.0f32, 0.0, 0.0, 0.0]
        .into_iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    lumps[3] = ints(&[0, -1, -2, 0, 0, 0, 0, 0, 0]);
    lumps[4] = ints(&[
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ]);
    lumps[7] = vec![0; 40];
    lumps[16] = ints(&[2, 1]);
    lumps[16].extend([3, 2]); // cluster 0 sees both; cluster 1 sees only itself.
    let mut bsp = vec![0u8; 152];
    bsp[..4].copy_from_slice(b"RBSP");
    bsp[4..8].copy_from_slice(&1i32.to_le_bytes());
    for (i, lump) in lumps.into_iter().enumerate() {
        let offset = bsp.len() as i32;
        bsp[8 + i * 8..12 + i * 8].copy_from_slice(&offset.to_le_bytes());
        bsp[12 + i * 8..16 + i * 8].copy_from_slice(&(lump.len() as i32).to_le_bytes());
        bsp.extend(lump);
    }
    jka_assets::bsp::Bsp::parse(&bsp)
        .unwrap()
        .visibility
        .unwrap()
}

#[test]
fn planning_respects_recorded_pvs_and_snapshot_portal_mask_changes() {
    let mut path = path();
    for sample in &mut path.samples {
        sample.origin.x = 100.0;
    }
    let mut auto = director();
    auto.visibility = Some(Arc::new(visibility_fixture()));
    let mut closed = [0u8; 32];
    closed[0] = 1 << 1;
    path.area_masks = vec![(0, [0; 32]), (6000, closed)];
    let other_area = Vec3::new(-100.0, 0.0, 40.0);
    assert!(auto.recorded_visible(&path, 5999, other_area));
    assert!(!auto.recorded_visible(&path, 6000, other_area));
    assert!(auto.recorded_visible(&path, 6000, Vec3::new(100.0, 0.0, 40.0)));
    for sample in &mut path.samples {
        sample.origin.x = -100.0;
    }
    assert!(
        !auto.recorded_visible(&path, 1000, Vec3::new(100.0, 0.0, 40.0)),
        "outside the recorded PVS"
    );
}

#[test]
fn nearby_saber_exchange_without_hit_events_is_never_downtime() {
    let path = path();
    assert!(combat_busy(&path, 0, 6000));
    assert!(!engagements(&path, 0, 12000, &|_| 0).is_empty());
    let clip = job(path.clone()).clip;
    assert!(downtime_keys(&path, &[], &[], &[], &clip, &|_| 0).is_empty());
}

#[test]
fn respawn_speed_starts_on_the_health_edge_without_waiting_for_a_jump() {
    use super::super::render_studio_timeline::studio_speed_at;
    let mut path = path();
    path.players.clear();
    path.alive
        .insert(0, vec![(0, true), (500, false), (1000, true)]);
    path.events.push(kill(500, 3, 0, Vec3::ZERO));
    for sample in &mut path.samples {
        if sample.ms >= 1000 {
            sample.origin.x = 200.0;
        }
    }
    let respawns = find_respawns(&path);
    assert_eq!(respawns.get(&0).unwrap(), &vec![1000]);
    let clip = job(path.clone()).clip;
    let keys = downtime_keys(&path, &[(0, 1500)], &[500], &[1000], &clip, &|_| 0);
    assert_eq!(studio_speed_at(&keys, 1.0), 3.0);
    assert_eq!(studio_speed_at(&keys, 0.999), 1.0);
    let mut auto = director();
    auto.prepare_focus(&path, 0, 12000);
    assert_eq!(
        auto.subject_at(&path, 1000),
        0,
        "return to the selected player immediately at spawn"
    );
}

#[test]
fn slow_motion_tail_ends_at_respawn_even_when_one_coverage_shot_spans_it() {
    use super::super::render_studio_timeline::studio_speed_at;
    let path = path();
    let clip = job(path.clone()).clip;
    let shot = StudioDirectorShot::new(1, P::Chase, 0, 0.0, 12.0);
    let kills = [kill(5000, 0, 1, Vec3::ZERO)];
    let keys = slowmo_for_edit(
        &path,
        &kills,
        &HashMap::new(),
        &clip,
        &[shot],
        &HashMap::from([(0, vec![5600])]),
    );
    assert!(studio_speed_at(&keys, 5.2) < 1.0);
    assert_eq!(studio_speed_at(&keys, 5.6), 1.0);
    assert_eq!(studio_speed_at(&keys, 6.0), 1.0);
}

#[test]
fn leaving_a_remote_player_at_respawn_speeds_the_actual_recorded_view_immediately() {
    use super::super::render_studio_timeline::studio_speed_at;
    let mut path = path();
    path.players.retain(|(c, _)| *c == 1);
    path.players[0].1.retain(|(t, _)| *t < 1000);
    path.alive
        .insert(0, vec![(0, true), (500, false), (1000, true)]);
    path.events.push(kill(500, 3, 0, Vec3::ZERO));
    for sample in &mut path.samples {
        if sample.ms >= 1000 {
            sample.origin.x = 5000.0;
        }
        if sample.ms == 1000 {
            sample.cut = true;
        }
    }
    let clip = job(path.clone()).clip;
    let shots = [StudioDirectorShot::new(1, P::Chase, 1, 0.0, 12.0)];
    assert!(
        player_at(&path, 1, 1000).is_some(),
        "ordinary interpolation still has a short hold"
    );
    assert_eq!(
        viewed_subject(&path, &clip, &shots, 1000),
        0,
        "the old PVS subject is not the rendered camera"
    );
    let keys = downtime_keys(&path, &[], &[], &[1000], &clip, &|ms| {
        viewed_subject(&path, &clip, &shots, ms)
    });
    assert_eq!(studio_speed_at(&keys, 1.0), 3.0);
}

#[test]
fn gripped_float_state_is_alive_and_does_not_create_a_respawn() {
    let mut state = PlayerState::default();
    state.stats[0] = 100;
    state.set_field_bits("clientNum", 0);
    state.set_field_bits("pm_type", 2);
    let mut path = DirectorPath::default();
    let mut teleport = None;
    path.push_state(0, &state, &mut teleport);
    state.set_field_bits("pm_type", 0);
    path.push_state(100, &state, &mut teleport);
    assert_eq!(path.alive.get(&0).unwrap(), &vec![(0, true)]);
    state.set_field_bits("pm_type", 5);
    path.push_state(200, &state, &mut teleport);
    assert!(!alive_at(&path, 0, 200));
}

#[test]
fn selected_remote_players_fight_blocks_speedup_even_far_from_the_recorder() {
    let mut path = path();
    path.players[0]
        .1
        .iter_mut()
        .for_each(|p| p.1 = Vec3::new(4000.0, 0.0, 0.0));
    path.players[1]
        .1
        .iter_mut()
        .for_each(|p| p.1 = Vec3::new(4200.0, 0.0, 0.0));
    let clip = job(path.clone()).clip;
    assert!(downtime_keys(&path, &[], &[], &[], &clip, &|_| 1).is_empty());
}

#[test]
fn dead_players_near_an_empty_spawn_do_not_block_fast_forward() {
    use super::super::render_studio_timeline::studio_speed_at;
    let mut path = path();
    path.players.retain(|(client, _)| *client == 1);
    path.alive.insert(0, vec![(0, false), (1000, true)]);
    path.alive.insert(1, vec![(0, false)]);
    let clip = job(path.clone()).clip;
    let keys = downtime_keys(&path, &[], &[], &[1000], &clip, &|_| 0);
    assert_eq!(studio_speed_at(&keys, 1.0), 3.0);
}

#[test]
fn low_aim_pitch_is_corrected_while_chase_keeps_the_recorded_yaw() {
    let mut path = path();
    for sample in &mut path.samples {
        sample.pitch = 80.0;
        sample.yaw = 30.0;
    }
    let mut shot = StudioDirectorShot::new(0, P::Chase, -1, 0.0, 3.0);
    shot.rig.range = 230.0;
    shot.rig.pitch_offset = 10.0;
    assert!(director().plan_chase(&mut shot, &path, 0, 3000, 0, Some(1)));
    for key in &shot.path_keys {
        let camera = Vec3::from_array(super::super::scene::jka_position(key.position));
        let look = Vec3::from_array(super::super::scene::jka_position(key.look));
        let direction = look - camera;
        assert!((heading_of(direction) - 30.0).abs() < 0.01);
        let pitch = (-direction.z)
            .atan2(direction.truncate().length())
            .to_degrees();
        assert!(
            pitch < 45.0,
            "floor aim should not dominate the shot: {pitch}"
        );
    }
}

#[test]
fn combat_coverage_uses_third_person_and_excludes_sideways_tracking() {
    let job = job(path());
    let shots = director().direct_combat_clip(&job).0;
    assert!(!shots.is_empty());
    assert!(shots.iter().all(|shot| shot.preset != P::Tracking));
    let first = shots
        .iter()
        .filter(|shot| {
            matches!(
                shot.preset,
                P::RecordedPov | P::PlayerEyes | P::VictimPov | P::VictimBlade
            )
        })
        .count();
    assert_eq!(first, 0, "automatic combat must stay outside the fighters");
}

#[test]
fn combat_reset_cluster_keeps_one_third_person_rig_instead_of_flash_shots() {
    let mut path = path();
    for sample in &mut path.samples {
        if sample.ms >= 7500 {
            sample.origin.x = 1000.0;
        }
        if sample.ms == 7500 {
            sample.cut = true;
        }
        if sample.ms >= 7600 {
            sample.client = 1;
        }
    }
    let job = job(path);
    let mut auto = director();
    auto.combat = true;
    let shots = auto.direct_beats(
        &job,
        vec![
            (0, 7100, Beat::Cruise),
            (7100, 7800, Beat::Exchange),
            (7800, 12000, Beat::Cruise),
        ],
    );
    assert!(shots.iter().all(|shot| shot.duration >= COMBAT_MIN_SHOT));
    let bridge = shots
        .iter()
        .find(|shot| shot.start <= 7.5 && shot.end() > 7.6)
        .unwrap();
    assert_eq!(bridge.preset, P::Chase);
    assert!(
        bridge.path_keys.is_empty(),
        "never spline through a source teleport"
    );
    assert!(shots
        .windows(2)
        .all(|w| (w[0].end() - w[1].start).abs() < 1e-6));
    assert_eq!(shots.first().unwrap().start, 0.0);
    assert!((shots.last().unwrap().end() - job.clip.end()).abs() < 1e-6);
}

#[test]
fn damage_and_death_hold_the_composition_including_late_focus_suggestions() {
    let mut path = path();
    path.events = vec![
        CombatEvent {
            kind: CombatKind::Pain,
            ..kill(4500, 1, 0, Vec3::ZERO)
        },
        kill(6000, 1, 0, Vec3::ZERO),
    ];
    let job = job(path);
    let mut auto = director();
    auto.combat = true;
    auto.track = vec![(5900, 1)];
    let shots = auto.direct_beats(
        &job,
        vec![
            (0, 4000, Beat::Exchange),
            (4000, 5900, Beat::Kill),
            (5900, 6500, Beat::Kill),
            (6500, 12000, Beat::Cruise),
        ],
    );
    let holds = auto.combat_holds(&job.path, 0, 12000);
    for shot in shots {
        let source = job.clip.source_time_ms(shot.start);
        assert!(
            !holds.iter().any(|&(s, e)| source > s && source < e),
            "cut inside damage/death at {source}"
        );
    }
}

#[test]
fn playback_speed_and_visibility_flicker_cannot_create_combat_microshots() {
    use super::super::render_studio_timeline::StudioSpeedKey;
    let mut path = path();
    path.players[0]
        .1
        .retain(|(ms, _)| *ms < 4000 || (*ms >= 5100 && *ms < 5200) || *ms >= 6500);
    let mut job = job(path);
    job.clip.speed_keys = vec![StudioSpeedKey {
        time: 0.0,
        rate: 3.0,
    }];
    for seed in 0..12 {
        let mut auto = AutoDirector::new(None, 100.0, StudioDirectorPacing::Energetic, seed);
        auto.set_focus(StudioDirectorFocus::Player(1));
        let shots = auto.direct_combat_clip(&job).0;
        assert!(shots
            .iter()
            .all(|shot| shot.duration + 1e-6 >= COMBAT_MIN_SHOT));
        assert!(shots
            .iter()
            .all(|shot| shot.preset != P::Static || shot.duration >= COMBAT_MIN_STATIC));
        assert!(shots.iter().all(|shot| !matches!(
            shot.preset,
            P::RecordedPov | P::PlayerEyes | P::SaberCam | P::VictimPov | P::VictimBlade
        )));
        assert!(shots
            .windows(2)
            .all(|w| (w[0].end() - w[1].start).abs() < 1e-6));
        assert!((shots.last().unwrap().end() - job.clip.end()).abs() < 1e-6);
    }
}

#[test]
fn best_combat_never_enters_on_a_victims_final_damage_or_death() {
    let mut path = path();
    path.players[0]
        .1
        .iter_mut()
        .for_each(|p| p.1 = Vec3::X * 2400.0);
    path.players[1]
        .1
        .iter_mut()
        .for_each(|p| p.1 = Vec3::X * 2500.0);
    path.events = vec![
        CombatEvent {
            kind: CombatKind::Pain,
            ..kill(5000, 2, 1, Vec3::X * 2400.0)
        },
        kill(6000, 2, 1, Vec3::X * 2400.0),
    ];
    let track = best_combat_track(&path, 4500, 10000, &HashMap::new());
    assert!(!track.iter().any(|&(ms, client)| client == 1 && ms < 9000));
    assert!(
        track.iter().any(|&(_, client)| client == 2),
        "the living attacker remains a valid story"
    );
}

#[test]
fn kill_shot_starts_with_context_before_the_impact() {
    let mut path = path();
    path.events.push(kill(5000, 0, 1, Vec3::X * 150.0));
    let job = job(path);
    let mut auto = director();
    auto.prepare_focus(&job.path, 0, 12000);
    let (beats, _, _) = auto.combat_beats(&job);
    let kill = beats.iter().find(|beat| beat.2 == Beat::Kill).unwrap();
    assert!(kill.0 <= 2800, "at least 2.2 seconds of lead-in");
}

#[test]
fn baked_combat_camera_follows_actor_time_through_slow_motion() {
    use super::super::render_studio_timeline::StudioSpeedKey as K;
    let mut path = path();
    path.players.clear();
    for sample in &mut path.samples {
        sample.origin.x = sample.ms as f32 * 0.1;
    }
    let mut job = job(path);
    job.clip.speed_keys = vec![
        K {
            time: 0.0,
            rate: 1.0,
        },
        K {
            time: 4.0,
            rate: 1.0,
        },
        K {
            time: 4.1,
            rate: 0.3,
        },
        K {
            time: 7.0,
            rate: 0.3,
        },
        K {
            time: 7.1,
            rate: 1.0,
        },
        K {
            time: 12.0,
            rate: 1.0,
        },
    ];
    let shot = StudioDirectorShot::new(1, P::Chase, 0, 0.0, job.clip.duration());
    let mut auto = director();
    auto.combat = true;
    let camera = auto
        .regenerate(&shot, &job, [None, None], Some(P::Chase))
        .unwrap();
    for ms in [2000, 5000, 6500, 9000] {
        let time = job.clip.timeline_at_source_ms(ms);
        let u = ((time - camera.start) / camera.duration) as f32;
        let (_, look) = camera.path_at(u).unwrap();
        let look = Vec3::from_array(super::super::scene::jka_position(look));
        assert!(
            (look.x - (ms as f32 * 0.1 + 256.0)).abs() < 0.2,
            "camera leads/lags the actor at source {ms}: {look:?}"
        );
    }
}

#[test]
fn combining_slow_and_fast_ramps_never_discards_the_slow_motion_boundary() {
    use super::super::render_studio_timeline::{studio_speed_at, StudioSpeedKey as K};
    let slow = vec![
        K {
            time: 1.0,
            rate: 1.0,
        },
        K {
            time: 2.0,
            rate: 0.3,
        },
        K {
            time: 3.0,
            rate: 0.3,
        },
        K {
            time: 4.0,
            rate: 1.0,
        },
    ];
    let fast = vec![
        K {
            time: 3.5,
            rate: 1.0,
        },
        K {
            time: 4.5,
            rate: 3.0,
        },
        K {
            time: 6.0,
            rate: 3.0,
        },
        K {
            time: 6.5,
            rate: 1.0,
        },
    ];
    let keys = combine_speed_keys(slow, fast);
    assert!(studio_speed_at(&keys, 3.5) < 1.0);
    assert!(keys.windows(2).all(|w| w[0].time < w[1].time));
}

#[test]
#[ignore = "requires the user's SABEREXAMPLE project and col_vs40s4 recording; set DJK_COMBAT_TEST_BASE"]
fn saberexample_speed_and_camera_regression() {
    use super::super::render_studio_timeline::studio_speed_at;
    let base = std::path::PathBuf::from(
        std::env::var_os("DJK_COMBAT_TEST_BASE").expect("DJK_COMBAT_TEST_BASE"),
    );
    let project: serde_json::Value = serde_json::from_slice(
        &std::fs::read(base.join("render-studio/SABEREXAMPLE.json")).unwrap(),
    )
    .unwrap();
    let original: StudioDemoClip = serde_json::from_value(project["demos"][0].clone()).unwrap();
    let paths = StudioPathCache::default();
    let path = load_path(&base, &paths, &original.name, &original.game).unwrap();
    let old_source = original.source_time_ms(163.0);
    eprintln!(
        "SABEREXAMPLE 2:43 -> source {old_source} ms, busy={}, nearby={:?}",
        combat_busy(&path, path.sample(old_source).client, old_source),
        path.players
            .iter()
            .filter_map(|(c, _)| player_at(&path, *c, old_source)
                .map(|p| (*c, p.distance(path.sample(old_source).origin))))
            .collect::<Vec<_>>()
    );
    let ctx = DirectorWorkerContext {
        project_dir: base.parent().unwrap_or(&base).join("render-studio/Test"),
        base,
        paths,
        collision: Default::default(),
        loaded: None,
        fov: 112.32,
        pacing: StudioDirectorPacing::Balanced,
        seed: 2,
        focus: StudioDirectorFocus::Recorded,
    };
    let plan =
        combat_view_plan(ctx, vec![(0, original.clone(), f64::INFINITY)], true, true).unwrap();
    let keys = &plan.speed_keys[0].1;
    assert!(
        studio_speed_at(keys, f64::from(old_source) / 1000.0) <= 1.0,
        "the reported fight must not speed up"
    );
    for ms in (original.in_ms..original.out_ms).step_by(50) {
        if studio_speed_at(keys, f64::from(ms) / 1000.0) > 1.001 {
            assert!(
                !combat_busy(&path, path.sample(ms).client, ms),
                "sped up active combat at source {ms}"
            );
        }
    }
    assert!(plan.added.iter().all(|shot| shot.preset != P::Tracking));
    let first = plan
        .added
        .iter()
        .filter(|shot| shot.preset == P::RecordedPov)
        .count();
    let zoom = plan
        .added
        .iter()
        .filter(|shot| shot.fov <= 45.0 && shot.rig.range >= 550.0)
        .count();
    eprintln!(
        "SABEREXAMPLE: {} shots, {first} recorded POV, {zoom} telephoto, {} speed keys",
        plan.added.len(),
        keys.len()
    );
    assert!(
        first * 4 < plan.added.len(),
        "excessive recorded first-person fallbacks"
    );
    assert!(zoom > 0, "the map should admit some long-lens coverage");
}

#[test]
#[ignore = "requires SaberExample2 and col_vs40s4; set DJK_COMBAT_TEST_BASE"]
fn saberexample2_final_edit_regression() {
    let base = std::path::PathBuf::from(
        std::env::var_os("DJK_COMBAT_TEST_BASE").expect("DJK_COMBAT_TEST_BASE"),
    );
    let mut project: serde_json::Value = serde_json::from_slice(
        &std::fs::read(base.join("render-studio/SaberExample2.json")).unwrap(),
    )
    .unwrap();
    let clip: StudioDemoClip = serde_json::from_value(project["demos"][0].clone()).unwrap();
    let paths = StudioPathCache::default();
    let path = load_path(&base, &paths, &clip.name, &clip.game).unwrap();
    let mut ctx = DirectorWorkerContext {
        project_dir: base.parent().unwrap_or(&base).join("render-studio/Test"),
        base: base.clone(),
        paths,
        collision: Default::default(),
        loaded: None,
        fov: 112.32,
        pacing: StudioDirectorPacing::Balanced,
        seed: 1,
        focus: StudioDirectorFocus::BestCombat,
    };
    let mut auto = ctx.director(&clip.game, &path.map);
    assert!(
        auto.has_collision(),
        "exercise real map placement and fallback decisions"
    );
    let job = ClipJob {
        clip,
        path,
        role: ClipRole::Whole,
    };
    let (mut shots, _, _) = auto.direct_combat_clip(&job);
    let holds = auto.combat_holds(&job.path, job.clip.in_ms, job.clip.out_ms);
    let resets = auto.combat_resets(&job.path, job.clip.in_ms, job.clip.out_ms);
    for shot in &shots {
        assert!(shot.valid());
        assert!(
            shot.duration + 1e-6 >= COMBAT_MIN_SHOT,
            "microshot: {:?} at {} for {}",
            shot.preset,
            shot.start,
            shot.duration
        );
        assert!(!matches!(
            shot.preset,
            P::RecordedPov
                | P::PlayerEyes
                | P::SaberCam
                | P::VictimPov
                | P::VictimBlade
                | P::Tracking
        ));
        assert!(shot.preset != P::Static || shot.duration + 1e-6 >= COMBAT_MIN_STATIC);
        let ms = job.clip.source_time_ms(shot.start);
        if resets.iter().all(|&r| (r - ms).abs() > 1) {
            assert!(
                !holds.iter().any(|&(s, e)| ms > s + 1 && ms < e - 1),
                "optional cut inside damage/death at {ms}"
            );
        }
    }
    assert_eq!(shots.first().unwrap().start, job.clip.start);
    assert!((shots.last().unwrap().end() - job.clip.end()).abs() < 1e-6);
    assert!(shots
        .windows(2)
        .all(|w| (w[0].end() - w[1].start).abs() < 1e-6));
    for old_time in [96.0 + 37.0 / 60.0, 104.0 + 26.0 / 60.0, 138.0] {
        let ms = job.clip.source_time_ms(old_time);
        eprintln!(
            "SaberExample2 {old_time:.3}s -> source {ms}ms; events {:?}",
            job.path
                .events
                .iter()
                .filter(|ev| (ev.ms - ms).abs() < 2000
                    && !matches!(ev.kind, CombatKind::Shot | CombatKind::Throw))
                .collect::<Vec<_>>()
        );
        eprintln!(
            "new coverage {:?}",
            shots
                .iter()
                .filter(|shot| shot.end() > old_time - 2.0 && shot.start < old_time + 2.0)
                .map(|shot| (shot.preset, shot.client, shot.start, shot.duration))
                .collect::<Vec<_>>()
        );
    }
    let zoom = shots
        .iter()
        .filter(|shot| shot.preset == P::Static && shot.fov <= 45.0)
        .count();
    eprintln!(
        "SaberExample2: {} shots; minimum {:.3}s; zero first-person/blade shots; {zoom} telephoto",
        shots.len(),
        shots
            .iter()
            .map(|shot| shot.duration)
            .fold(f64::INFINITY, f64::min)
    );
    assert!(zoom > 0);
}

#[test]
#[ignore = "requires col_vs40s4 and map; set DJK_COMBAT_TEST_BASE"]
fn saberexample_camera_bound_speed_and_respawn_regression() {
    use super::super::render_studio_timeline::studio_speed_at;
    let base = std::path::PathBuf::from(std::env::var_os("DJK_COMBAT_TEST_BASE").unwrap());
    let mut project: serde_json::Value = serde_json::from_slice(
        &std::fs::read(base.join("render-studio/SaberExample2.json")).unwrap(),
    )
    .unwrap();
    let mut clip: StudioDemoClip = serde_json::from_value(project["demos"][0].clone()).unwrap();
    clip.start = 1.0 / 30.0;
    let paths = StudioPathCache::default();
    let path = load_path(&base, &paths, &clip.name, &clip.game).unwrap();
    eprintln!(
        "RECORDED ALIVE {:?}",
        path.alive
            .get(&path.samples[0].client)
            .map(|s| s.iter().filter(|s| s.0 < 60000).collect::<Vec<_>>())
    );
    eprintln!(
        "EARLY RESPAWNS {:?}",
        find_respawns(&path)
            .iter()
            .map(|(c, t)| (*c, t.iter().filter(|&&t| t < 60000).collect::<Vec<_>>()))
            .collect::<Vec<_>>()
    );
    eprintln!(
        "EARLY CUTS {:?}",
        path.samples
            .iter()
            .filter(|s| s.cut && s.ms < 60000)
            .map(|s| (s.ms, s.client, s.origin))
            .collect::<Vec<_>>()
    );
    let mut ctx = DirectorWorkerContext {
        project_dir: base.parent().unwrap_or(&base).join("render-studio/Test"),
        base,
        paths,
        collision: Default::default(),
        loaded: None,
        fov: 112.32,
        pacing: StudioDirectorPacing::Balanced,
        seed: 2,
        focus: StudioDirectorFocus::BestCombat,
    };
    let mut audit = ctx.director(&clip.game, &path.map);
    // Preserve the source moments from the reported edit before repairing its
    // speed map, so GPU checks still visit the same actions after retiming.
    let fights = audit.fights(&ClipJob {
        clip: clip.clone(),
        path: path.clone(),
        role: ClipRole::Whole,
    });
    let fight_kills: Vec<_> = audit
        .engagements
        .iter()
        .flat_map(|f| f.kills.iter().map(|k| k.ms))
        .collect();
    let all_kills: Vec<_> = path
        .events
        .iter()
        .filter(|k| k.kind == CombatKind::Kill)
        .map(|k| k.ms)
        .collect();
    let mut spawn_times = Vec::new();
    for (&client, times) in &audit.respawns {
        spawn_times.extend(
            times
                .iter()
                .copied()
                .filter(|&ms| audit.subject_at(&path, ms) == client),
        );
    }
    let mut reported = clip.clone();
    reported.speed_keys = combine_speed_keys(
        slowmo_keys(&fight_kills, &audit.kill_air, &clip),
        downtime_keys(&path, &fights, &all_kills, &spawn_times, &clip, &|ms| {
            audit.subject_at(&path, ms)
        }),
    );
    let reported_sources: Vec<_> = [
        11.15, 18.183, 39.133, 77.783, 87.75, 111.517, 123.233, 136.5,
    ]
    .into_iter()
    .map(|time| reported.source_time_ms(time))
    .collect();
    eprintln!("REPORTED SOURCE MOMENTS {reported_sources:?}");
    let plan = combat_view_plan(ctx, vec![(0, clip.clone(), f64::INFINITY)], true, true).unwrap();
    clip.speed_keys = plan.speed_keys[0].1.clone();
    let mut shots = plan.added;
    for shot in &shots {
        assert!(shot.valid());
        assert!(shot.duration + 1e-6 >= COMBAT_MIN_SHOT);
        assert!(shot.preset != P::Static || shot.duration + 1e-6 >= COMBAT_MIN_STATIC);
    }
    let covered: Vec<_> = path
        .events
        .iter()
        .filter(|k| k.kind == CombatKind::Kill)
        .filter(|k| audit.covered_kill(&path, &clip, &shots, k))
        .collect();
    for ms in (clip.in_ms..clip.out_ms).step_by(50) {
        let rate = studio_speed_at(&clip.speed_keys, f64::from(ms) / 1000.0);
        if rate < 0.999 {
            assert!(
                covered
                    .iter()
                    .any(|k| ms >= k.ms - 600 && ms <= k.ms + 1800),
                "uncovered slow motion at {ms}"
            );
        }
        if rate > 1.001 {
            let client = viewed_subject(&path, &clip, &shots, ms);
            assert!(
                !combat_busy(&path, client, ms),
                "sped up the visible player's fight at {ms}, client {client}"
            );
        }
    }
    let respawns = find_respawns(&path);
    for (&client, times) in &respawns {
        for &r in times {
            if viewed_subject(&path, &clip, &shots, r) != client {
                continue;
            }
            let rate = studio_speed_at(&clip.speed_keys, f64::from(r) / 1000.0);
            let busy = combat_busy(&path, client, r);
            if r < 60000 {
                eprintln!("RESPAWN source {r} client {client} rate {rate} busy {busy}; origin {:?}; nearest {:?}",
                player_at(&path,client,r),path.players.iter().filter(|(id,_)|*id!=client&&alive_at(&path,*id,r))
                    .filter_map(|(id,_)|Some((*id,player_at(&path,*id,r)?.distance(player_at(&path,client,r)?))))
                    .min_by(|a,b|a.1.total_cmp(&b.1)));
            }
            if !busy
                && (r..r + 1700).step_by(50).all(|ms| {
                    ms < clip.out_ms
                        && viewed_subject(&path, &clip, &shots, ms) == client
                        && !combat_busy(&path, client, ms)
                })
            {
                assert!(
                    rate >= 2.5,
                    "empty respawn still waits at source {r}: {rate}"
                );
            }
        }
    }
    eprintln!(
        "CAMERA-BOUND SPEED: {} shots, {} covered kills with slow motion",
        shots.len(),
        covered.len()
    );
    if let Some(output) = std::env::var_os("DJK_COMBAT_REVIEW_PROJECT") {
        std::fs::create_dir_all("artifacts").unwrap();
        std::fs::write(
            "artifacts/studio-combat-source-times.json",
            serde_json::to_vec(&reported_sources).unwrap(),
        )
        .unwrap();
        for (i, shot) in shots.iter_mut().enumerate() {
            shot.id = i as u64 + 1;
        }
        project["demos"][0] = serde_json::to_value(clip).unwrap();
        project["director_shots"] = serde_json::to_value(shots).unwrap();
        std::fs::write(output, serde_json::to_vec_pretty(&project).unwrap()).unwrap();
    }
}
