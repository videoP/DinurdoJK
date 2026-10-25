#[allow(dead_code)]
mod support;

use jka_movement::{
    CollisionWorld, EntityClip, PredictSettings, SourceCollisionBrush, SourceCollisionPlane,
    TraceQuery, TraceResult, TraceWorld, UserCmd,
};
use support::{animations, player, Map};

fn query() -> TraceQuery {
    TraceQuery {
        start: [0.0, 0.0, 128.0], end: [16.0, 0.0, 128.0],
        mins: [-15.0, -15.0, -24.0], maxs: [15.0, 15.0, 40.0],
        pass_entity: 0, mask: 1,
    }
}

#[test]
fn invalid_collision_coordinates_are_blocked_without_panicking() {
    let source = SourceCollisionBrush {
        mins: [-64.0; 3], maxs: [64.0; 3], contents: 1,
        planes: (0..6).map(|side| {
            let mut normal = [0.0; 3];
            normal[side / 2] = if side % 2 == 0 { -1.0 } else { 1.0 };
            SourceCollisionPlane { normal, distance: 64.0, surface_flags: 0 }
        }).collect(),
    };
    let worlds = [Map::floor().world(), CollisionWorld::from_source_brushes(vec![source]).unwrap()];
    for mut world in worlds {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for field in 0..4 {
                for axis in 0..3 {
                    let mut q = query();
                    match field {
                        0 => q.start[axis] = value,
                        1 => q.end[axis] = value,
                        2 => q.mins[axis] = value,
                        _ => q.maxs[axis] = value,
                    }
                    let result = world.trace(q);
                    assert_eq!(result.fraction, 0.0);
                    assert_eq!(result.all_solid, 1);
                    assert_eq!(result.start_solid, 1);
                    assert!(result.end.iter().all(|v| v.is_finite()));
                    if field != 0 { assert_eq!(result.end, q.start); }
                    let result = world.trace_entity(q, EntityClip::Box {
                        mins: [-8.0; 3], maxs: [8.0; 3], origin: [0.0; 3],
                    });
                    assert_eq!(result.fraction, 0.0);
                    assert!(result.end.iter().all(|v| v.is_finite()));
                }
            }
            assert_eq!(world.point_contents([value, 0.0, 0.0], 0), 1);
            assert_eq!(world.inline_model_contents([value, 0.0, 0.0], 1, [0.0; 3], [0.0; 3]), 1);
        }
        assert_eq!(world.trace(query()).fraction, 1.0);
    }
}

#[test]
fn invalid_entity_transforms_do_not_reach_native_collision() {
    let mut world = Map::floor().world();
    for clip in [
        EntityClip::Box { mins: [-8.0; 3], maxs: [8.0; 3], origin: [f32::NAN; 3] },
        EntityClip::InlineModel { index: 1, origin: [0.0; 3], angles: [f32::INFINITY; 3] },
    ] {
        assert_eq!(world.trace_entity(query(), clip), TraceResult::clear(query().end));
    }
    assert_eq!(world.inline_model_contents([0.0; 3], 1, [f32::NAN; 3], [0.0; 3]), 0);
}

#[test]
fn non_finite_snapshot_movement_fails_before_any_collision_callback() {
    struct NoCollision;
    impl TraceWorld for NoCollision {
        fn trace(&mut self, _: TraceQuery) -> TraceResult { panic!("invalid state reached collision") }
        fn point_contents(&mut self, _: [f32; 3], _: i32) -> i32 { panic!("invalid state reached contents") }
    }
    let movement = animations();
    for backend in [0, 1] {
        for field in ["origin[0]", "velocity[2]", "viewangles[1]"] {
            let mut p = player(80.0);
            let mut state = p.network();
            let mut schema = jka_protocol::server::PlayerState::default();
            schema.fields.clone_from(&state.fields);
            assert!(schema.set_field_bits(field, f32::NAN.to_bits()));
            state.fields.clone_from(&schema.fields);
            p.configure(&PredictSettings { backend, ..Default::default() }).unwrap();
            p.set_network(&state).unwrap();
            let error = movement.predict(&mut p, UserCmd { server_time: 8, ..Default::default() },
                &PredictSettings { backend, ..Default::default() }, &mut NoCollision).unwrap_err();
            assert!(error.contains("Non-finite movement state"), "{error}");
            // A subsequent authoritative correction can recover the same player.
            assert!(schema.set_field_bits(field, 0.0f32.to_bits()));
            state.fields.clone_from(&schema.fields);
            p.set_network(&state).unwrap();
            movement.predict(&mut p, UserCmd { server_time: 8, ..Default::default() },
                &PredictSettings { backend, ..Default::default() }, &mut Map::floor().world()).unwrap();
        }
    }
}
