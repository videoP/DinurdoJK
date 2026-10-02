//! CG_PredictPlayerState's saber bookkeeping: `cgs.clientinfo[].saber[]` for
//! BG_MySaber and the client-derived, non-networked `fd.saberAnimLevelBase`.
#[allow(dead_code)]
mod support;
use jka_movement::{
    animation_name, NetworkPlayerState, PlayerState, PredictSettings, SaberMovementInfo, UserCmd, BUTTON_ALT_ATTACK,
    BUTTON_ATTACK, WP_SABER,
};
use support::{animations, player, Map};

const SS_MEDIUM: u32 = 2;
const MAX_CLIENTS: u32 = 32;

fn field_index(name: &str) -> usize {
    use jka_protocol::server::PlayerState as Protocol;
    let mut probe = Protocol::default();
    for index in 0..probe.fields.len() {
        probe.fields[index] = 0x5A5A_1234;
        if probe.field_bits(name) == Some(0x5A5A_1234) {
            return index;
        }
        probe.fields[index] = 0;
    }
    panic!("no playerState field named {name}");
}

fn set(state: &mut NetworkPlayerState, name: &str, value: u32) {
    state.fields[field_index(name)] = value;
}

fn get(player: &PlayerState, name: &str) -> i32 {
    player.network().fields[field_index(name)] as i32
}

/// A snapshot playerState of a grounded player holding the saber with one
/// half of the blades off (`saberHolstered == 1`), medium style.
fn half_lit_saber_player() -> PlayerState {
    let mut player = player(24.125);
    let mut state = player.network();
    set(&mut state, "weapon", u32::from(WP_SABER));
    set(&mut state, "saberHolstered", 1);
    set(&mut state, "saberEntityNum", MAX_CLIENTS + 19);
    set(&mut state, "fd.saberAnimLevel", SS_MEDIUM);
    set(&mut state, "fd.saberDrawAnimLevel", SS_MEDIUM);
    set(&mut state, "clientNum", 0);
    player.set_network(&state).unwrap();
    player
}

fn staff() -> [SaberMovementInfo; 2] {
    [
        SaberMovementInfo { num_blades: 2, ..SaberMovementInfo::equipped_default() }.for_prediction(),
        SaberMovementInfo::default().for_prediction(),
    ]
}

fn run(player: &mut PlayerState, buttons: i32, in_flight: bool) {
    run_with(player, buttons, in_flight, PredictSettings::default());
}

fn run_with(player: &mut PlayerState, buttons: i32, in_flight: bool, settings: PredictSettings) {
    let movement = animations();
    let mut world = Map::floor().world();
    if in_flight {
        let mut state = player.network();
        set(&mut state, "saberInFlight", 1);
        player.set_network(&state).unwrap();
    }
    let mut time = player.view().command_time;
    for _ in 0..20 {
        time += 8;
        let cmd = UserCmd { server_time: time, buttons, ..Default::default() };
        movement
            .predict(player, cmd, &settings, &mut world)
            .unwrap();
    }
}

#[test]
fn half_lit_staff_stays_lit_when_attacking_with_equipment_installed() {
    let mut player = half_lit_saber_player();
    for (slot, info) in staff().into_iter().enumerate() {
        player.set_saber_movement_info(slot, info).unwrap();
    }
    run(&mut player, BUTTON_ATTACK, false);
    assert_eq!(
        get(&player, "saberHolstered"),
        1,
        "prediction must not re-ignite the extra blades of a single-blade staff"
    );
    assert!(get(&player, "saberMove") > 1, "an attack move is predicted");
}

#[test]
fn half_lit_staff_stays_lit_while_guiding_a_thrown_saber() {
    let mut player = half_lit_saber_player();
    for (slot, info) in staff().into_iter().enumerate() {
        player.set_saber_movement_info(slot, info).unwrap();
    }
    run(&mut player, BUTTON_ALT_ATTACK, true);
    assert_eq!(get(&player, "saberHolstered"), 1);
    assert_eq!(
        animation_name(get(&player, "torsoAnim")),
        Some("BOTH_SABERPULL"),
        "the arm-extended guiding pose continues while the saber is out"
    );
}

#[test]
fn prediction_does_not_claim_the_snapshot_saber_entity() {
    let mut player = half_lit_saber_player();
    let before = get(&player, "saberEntityNum");
    for (slot, info) in staff().into_iter().enumerate() {
        player.set_saber_movement_info(slot, info).unwrap();
    }
    assert_eq!(get(&player, "saberEntityNum"), before);
}

#[test]
fn second_saber_alone_selects_the_dual_base_without_a_primary_blade_count() {
    // ci->saber[0].numBlades > 0 wins first, exactly as in cg_predict.c; this
    // guards the fallback branch: no primary saber, a second saber present.
    let mut player = half_lit_saber_player();
    let mut second = SaberMovementInfo::equipped_default().for_prediction();
    second.num_blades = 1;
    player
        .set_saber_movement_info(0, SaberMovementInfo::default().for_prediction())
        .unwrap();
    player.set_saber_movement_info(1, second).unwrap();
    run(&mut player, BUTTON_ATTACK, false);
    assert_eq!(get(&player, "saberHolstered"), 1);
}

#[test]
fn japro_backend_predicts_half_lit_staff_the_same_way() {
    let japro = PredictSettings { server_mod: 1, ..PredictSettings::default() };
    let mut player = half_lit_saber_player();
    player.configure(&japro).unwrap();
    for (slot, info) in staff().into_iter().enumerate() {
        player.set_saber_movement_info(slot, info).unwrap();
    }
    run_with(&mut player, BUTTON_ATTACK, false, japro);
    assert_eq!(get(&player, "saberHolstered"), 1);
    assert!(get(&player, "saberMove") > 1);
}
