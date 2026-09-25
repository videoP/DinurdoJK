//! The native prediction bridge must agree with the Rust wire schema.
use jka_movement::{network_field_count, weapon_info, NetworkPlayerState, PlayerState};

#[test]
fn bridge_uses_the_protocol_playerstate_schema() {
    let schema = jka_protocol::server::PlayerState::default();
    assert_eq!(network_field_count(), schema.fields.len());
}

#[test]
fn every_network_slot_round_trips_through_playerstate_t() {
    let count = network_field_count();
    let state = NetworkPlayerState {
        fields: (0..count as u32).map(|i| i.wrapping_mul(0x0101_0101).wrapping_add(7)).collect(),
        stats: std::array::from_fn(|i| i as i32 * 3 - 20),
        persistant: std::array::from_fn(|i| i as i32 + 100),
        ammo: std::array::from_fn(|i| i as i32 * 11),
        powerups: std::array::from_fn(|i| -(i as i32)),
    };
    // Overlapping offsets or a short slot would corrupt a neighbour here.
    let player = PlayerState::from_network(&state).unwrap();
    assert_eq!(player.network(), state);
}

#[test]
fn weapon_data_matches_stock_table() {
    // WP_SABER uses no ammo; WP_BLASTER draws 2 blaster packs per shot.
    assert_eq!(weapon_info(3).map(|w| w.1), Some(0));
    let blaster = weapon_info(5).unwrap();
    assert_eq!((blaster.1, blaster.2), (2, 3));
    assert_eq!(weapon_info(19), None);
}
