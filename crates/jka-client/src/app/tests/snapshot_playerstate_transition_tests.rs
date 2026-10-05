use super::*;

#[test]
fn followed_live_playerstate_events_are_snapshot_owned() {
    let mut ps = jka_protocol::server::PlayerState::default();
    ps.set_field_bits("pm_flags", 4096);
    assert!(snapshot_owns_playerstate_events(
        true, false, false, false, &ps
    ));
}

#[test]
fn ordinary_predicted_live_playerstate_events_are_not_snapshot_owned() {
    let ps = jka_protocol::server::PlayerState::default();
    assert!(!snapshot_owns_playerstate_events(
        true, false, false, false, &ps
    ));
}

#[test]
fn non_prediction_paths_are_snapshot_owned() {
    let ps = jka_protocol::server::PlayerState::default();
    assert!(snapshot_owns_playerstate_events(
        false, false, false, false, &ps
    ));
    assert!(snapshot_owns_playerstate_events(
        true, true, false, false, &ps
    ));
    assert!(snapshot_owns_playerstate_events(
        true, false, true, false, &ps
    ));
    assert!(snapshot_owns_playerstate_events(
        true, false, false, true, &ps
    ));
}
