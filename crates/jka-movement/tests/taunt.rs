use jka_assets::animation::animation_index;
use jka_movement::{taunt_should_cancel, BUTTON_ALT_ATTACK, BUTTON_ATTACK, BUTTON_WALKING};
#[allow(dead_code)]
mod support;

#[test]
fn native_taunt_input_cancels_attacks_and_vertical_movement() {
    for name in ["BOTH_GESTURE1", "BOTH_DUAL_TAUNT", "BOTH_STAFF_TAUNT"] {
        let anim = animation_index(name).unwrap() as i32;
        assert!(!taunt_should_cancel(anim, anim, 0, 0));
        assert!(!taunt_should_cancel(anim, anim, BUTTON_WALKING, 0));
        assert!(taunt_should_cancel(anim, anim, BUTTON_ATTACK, 0));
        assert!(taunt_should_cancel(anim, anim, BUTTON_ALT_ATTACK, 0));
        // OpenJK q_shared.h: grip, selected Force power, lightning, and drain.
        for force_button in [64, 512, 1024, 2048] {
            assert!(taunt_should_cancel(anim, anim, force_button, 0));
        }
        assert!(taunt_should_cancel(anim, anim, 0, 127));
        assert!(taunt_should_cancel(anim | 0x8000, anim | 0x8000, 0, -127));
    }
}

#[test]
fn torso_only_taunts_do_not_use_the_full_body_cancel_rule() {
    let engage = animation_index("BOTH_ENGAGETAUNT").unwrap() as i32;
    let dual = animation_index("BOTH_DUAL_TAUNT").unwrap() as i32;
    let stand = animation_index("BOTH_STAND1").unwrap() as i32;
    let crouch = animation_index("BOTH_CROUCH1").unwrap() as i32;
    for (torso, legs) in [
        (engage, engage),
        (engage, stand),
        (engage, crouch),
        (dual, stand),
    ] {
        assert!(!taunt_should_cancel(torso, legs, 0, -127));
        assert!(!taunt_should_cancel(torso, legs, BUTTON_ATTACK, 0));
    }
}

#[test]
fn pmove_crouch_keeps_torso_taunts_but_cancels_full_body_taunts() {
    for backend in [0, 1] {
        let movement = support::animations();
        for (name, full_body) in [
            ("BOTH_ENGAGETAUNT", false),
            ("BOTH_DUAL_TAUNT", true),
            ("BOTH_STAFF_TAUNT", true),
        ] {
            let mut world = support::Map::floor().world();
            let mut player = support::player(24.125);
            player
                .configure(&jka_movement::PredictSettings {
                    backend,
                    ..Default::default()
                })
                .unwrap();
            support::tick(&movement, &mut player, &mut world, 0, 0, 0);
            let mut state = player.network();
            let anim = animation_index(name).unwrap() as u32;
            for (key, value) in [
                ("torsoAnim", anim),
                (
                    "legsAnim",
                    if full_body {
                        anim
                    } else {
                        animation_index("BOTH_STAND1").unwrap() as u32
                    },
                ),
                ("torsoTimer", 1),
                ("legsTimer", u32::from(full_body)),
                ("forceHandExtend", 10),
                ("forceDodgeAnim", anim),
            ] {
                state.fields[jka_protocol::PLAYER_FIELDS
                    .iter()
                    .position(|(field, _)| *field == key)
                    .unwrap()] = value;
            }
            player.set_network(&state).unwrap();
            let view = support::tick(&movement, &mut player, &mut world, 0, 0, -127);
            assert_eq!(
                player.entity_view().force_hand_extend == 10,
                !full_body,
                "{name}, backend={backend}: {view:?}"
            );
            if !full_body {
                assert_eq!(view.torso_anim, anim as i32);
            }
        }
    }
}
