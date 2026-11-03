use super::*;
use crate::net::{CommandButtons, BUTTON_FORCEPOWER, BUTTON_USE, BUTTON_USE_HOLDABLE, BUTTON_WALKING};
use std::collections::HashSet;

fn frame(time: i32, adjust: impl FnOnce(&mut native::State)) -> Jk2Frame {
    let mut snap = native::State::zero(native::Version::V104);
    for (name, value) in [("weapon", 2), ("clientNum", 0), ("fd.forcePowerSelected", 0)] {
        snap.set_field_bits(name, value).unwrap();
    }
    snap.stats[0] = 100;
    snap.stats[4] = (1 << 2) | (1 << 3) | (1 << 4); // saber, bryar, blaster
    snap.ammo.fill(999);
    snap.persistant[4] = 1;
    adjust(&mut snap);
    Jk2Frame {
        gameplay: native::Version::V104,
        predicted: snap.clone(),
        snap,
        time,
        presented: PlayerState::default(),
    }
}

fn buttons(active: &HashSet<String>) -> CommandButtons<'_> {
    CommandButtons { active, forced_moveup: false, idrive: false, any_key: false, talking: false }
}

#[test]
fn jk2_usercmds_carry_native_selection_and_engine_rules() {
    let mut input = LiveInput::default();
    input.set_jk2(true);
    let active: HashSet<String> = ["+speed", "+forward", "+useforce", "+use"].map(String::from).into();
    // CG_Init: WP_BRYAR_PISTOL, forceSelect/itemSelect -1 (sent as 255).
    let cmd = input.preview_cmd(&buttons(&active));
    assert_eq!((cmd.weapon, cmd.force_selection, cmd.inventory_selection), (3, 255, 255));
    // jk2mv CL_KeyMove walks at 46 and CL_CmdButtons keeps FORCEPOWER+USE.
    assert_eq!(cmd.forward_move, 46);
    assert_ne!(cmd.buttons & BUTTON_WALKING, 0);
    assert_eq!(cmd.buttons & (BUTTON_FORCEPOWER | BUTTON_USE), BUTTON_FORCEPOWER | BUTTON_USE);
    assert_eq!(cmd.buttons & BUTTON_USE_HOLDABLE, 0);
    // Generic commands are converted once, at CL_FinishMove.
    input.queue_generic_command(26); // JKA GENCMD_SABERATTACKCYCLE
    assert_eq!(input.create_cmd(&buttons(&active)).generic_command, 20);
    input.queue_generic_command(4); // GENCMD_FORCE_SPEED is shared
    assert_eq!(input.create_cmd(&buttons(&active)).generic_command, 4);
    input.queue_generic_command(27); // JKA-only taunt
    assert_eq!(input.create_cmd(&buttons(&active)).generic_command, 0);
    assert!(!input.queue_emote_taunt(1), "JK2 never repurposes cmd angles for emotes");
    // JKA keeps its own rules.
    let mut jka = LiveInput::default();
    let cmd = jka.create_cmd(&buttons(&active));
    assert_eq!(cmd.forward_move, 64);
    assert_ne!(cmd.buttons & BUTTON_USE_HOLDABLE, 0);
}

#[test]
fn jk2_respawn_slots_cycles_and_hud_mirrors() {
    let mut input = LiveInput::default();
    input.set_jk2(true);
    input.sync_jk2(&frame(1000, |_| {}));
    // CG_Respawn: cg.weaponSelect = ps.weapon (JK2 saber 2, presented as 3).
    assert_eq!(input.jk2_selection().unwrap().weapon, 2);
    assert_eq!(input.weapon_select, 3);
    let f = frame(1100, |_| {});
    assert_eq!(input.jk2_weapon_slot(&f, 3), Ok(Outcome::Selected));
    assert_eq!(input.jk2_selection().unwrap().weapon, 4);
    assert_eq!(input.weapon_select, 5, "HUD sees presentation WP_BLASTER");
    assert_eq!(input.preview_cmd(&buttons(&HashSet::new())).weapon, 4, "wire is JK2 WP_BLASTER");
    assert_eq!(input.jk2_cycle_weapon(&f, true), Ok(Outcome::Selected));
    assert_eq!(input.jk2_selection().unwrap().weapon, 2, "wraps past unowned weapons");
    assert_eq!(input.jk2_weapon_slot(&f, 1), Ok(Outcome::SaberSwitch));
    assert_eq!(input.create_cmd(&buttons(&HashSet::new())).generic_command, 1);
    // A later sync on the same spawn keeps the player's choice.
    input.sync_jk2(&frame(1200, |_| {}));
    assert_eq!(input.jk2_selection().unwrap().weapon, 2);
}

#[test]
fn jk2_force_selection_returns_to_server_after_its_display_expires() {
    let mut input = LiveInput::default();
    input.set_jk2(true);
    let known = |s: &mut native::State| {
        s.set_field_bits("fd.forcePowersKnown", (1 << 0) | (1 << 2) | (1 << 3)).unwrap();
    };
    input.sync_jk2(&frame(1000, known));
    assert_eq!(input.jk2_cycle_force(&frame(1100, known), true), Ok(Outcome::Selected));
    let chosen = input.jk2_selection().unwrap().force;
    assert!(matches!(chosen, 2 | 3));
    assert_eq!(input.preview_cmd(&buttons(&HashSet::new())).force_selection, chosen as u8);
    let server = |s: &mut native::State| {
        known(s);
        s.set_field_bits("fd.forcePowerSelected", 0).unwrap();
    };
    input.sync_jk2(&frame(2400, server));
    assert_eq!(input.jk2_selection().unwrap().force, chosen, "still displayed");
    input.sync_jk2(&frame(2600, server));
    assert_eq!(input.jk2_selection().unwrap().force, 0, "CG_DrawForceSelect expiry");
}

#[test]
fn jk2_out_of_ammo_and_pickup_follow_cgame_events() {
    let mut input = LiveInput::default();
    input.set_jk2(true);
    let blaster = |s: &mut native::State| s.set_field_bits("weapon", 4).unwrap();
    input.sync_jk2(&frame(1000, blaster));
    // EV_NOAMMO parm < WP_NUM_WEAPONS: the current weapon ran dry.
    input.jk2_out_of_ammo(&frame(1100, blaster), 4, 1).unwrap();
    assert_eq!(input.jk2_selection().unwrap().weapon, 3);
    // parm >= WP_NUM_WEAPONS encodes the old weapon (here the bryar, 3 + 16).
    input.jk2_out_of_ammo(&frame(1200, blaster), 19, 1).unwrap();
    assert_eq!(input.jk2_selection().unwrap().weapon, 4);
    // CG_ItemPickup: presentation WP_REPEATER (8) is JK2 7; level 3 never switches.
    input.jk2_pickup(&frame(1300, blaster), 8, 3);
    assert_eq!(input.jk2_selection().unwrap().weapon, 4);
    input.jk2_pickup(&frame(1300, blaster), 8, 2);
    assert_eq!(input.jk2_selection().unwrap().weapon, 7);
    assert_eq!(input.weapon_select, 8);
}
