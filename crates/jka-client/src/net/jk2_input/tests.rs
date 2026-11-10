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
    input.queue_generic_command(27); // JKA-only taunt must not go onto the wire
    assert_eq!(input.create_cmd(&buttons(&active)).generic_command, 0);
    // JK2 /taunt instead pulses BUTTON_GESTURE through the normal button
    // latch; it must not send genericCommand 27 or stay held indefinitely.
    input.note_pressed("+button3");
    let taunt = input.create_cmd(&buttons(&active));
    assert_ne!(taunt.buttons & (1 << 3), 0);
    assert_eq!(taunt.generic_command, 0);
    let next = input.create_cmd(&buttons(&active));
    assert_eq!(next.buttons & (1 << 3), 0);
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

#[test]
fn jk2_weaplast_toggles_native_weapons_and_uses_server_equipped_on_first_use() {
    let mut input = LiveInput::default();
    input.set_jk2(true);
    input.sync_jk2(&frame(1000, |_| {}));
    let f = frame(1100, |_| {});

    // First use with a pending selection returns to the server-equipped saber,
    // as in JKA's CG_LastWeapon_f, not an arbitrary high-numbered weapon.
    assert_eq!(input.jk2_weapon_slot(&f, 3), Ok(Outcome::Selected));
    assert_eq!(input.jk2_selection().unwrap().weapon, 4); // JK2 blaster
    assert_eq!(input.jk2_last_weapon(&f), Ok(Outcome::Selected));
    assert_eq!(input.jk2_selection().unwrap().weapon, 2); // JK2 saber
    assert_eq!(input.preview_cmd(&buttons(&HashSet::new())).weapon, 2);
    assert_eq!(input.weapon_select, 3, "HUD mirrors JK2 saber as JKA weapon 3");
    assert_eq!(input.jk2_last_weapon(&f), Ok(Outcome::Selected));
    assert_eq!(input.jk2_selection().unwrap().weapon, 4);
    assert_eq!(input.preview_cmd(&buttons(&HashSet::new())).weapon, 4);
}

#[test]
fn jk2_weaplast_first_use_fallback_and_respawn_history() {
    let mut input = LiveInput::default();
    input.set_jk2(true);
    input.sync_jk2(&frame(1000, |_| {}));
    let f = frame(1100, |_| {});
    assert_eq!(input.jk2_last_weapon(&f), Ok(Outcome::Selected));
    assert_eq!(input.jk2_selection().unwrap().weapon, 4, "highest owned selectable weapon");
    assert_eq!(input.jk2_last_weapon(&f), Ok(Outcome::Selected));
    assert_eq!(input.jk2_selection().unwrap().weapon, 2);

    // A fresh spawn must not retain the previous spawn's toggle history.
    let respawn = frame(1500, |s| {
        s.persistant[4] = 2;
        s.set_field_bits("weapon", 3).unwrap();
    });
    input.sync_jk2(&respawn);
    assert_eq!(input.jk2_last_weapon(&respawn), Ok(Outcome::Selected));
    assert_eq!(input.jk2_selection().unwrap().weapon, 4);
}

#[test]
fn jk2_weaplast_respects_native_selectability_and_spectator_guards() {
    let mut input = LiveInput::default();
    input.set_jk2(true);
    input.sync_jk2(&frame(1000, |_| {}));
    let only_saber = frame(1100, |s| s.stats[4] = 1 << 2);
    assert_eq!(input.jk2_last_weapon(&only_saber), Ok(Outcome::Selected));
    assert_eq!(input.jk2_selection().unwrap().weapon, 2);

    for guarded in ["pm_flags", "pm_type", "emplacedIndex"] {
        let f = frame(1200, |s| {
            s.set_field_bits(guarded, match guarded {
                "pm_flags" => 4096, // PMF_FOLLOW
                "pm_type" => 3, // JK2 PM_SPECTATOR
                _ => 1,
            }).unwrap();
        });
        assert_eq!(input.jk2_last_weapon(&f), Ok(Outcome::Ignored), "{guarded}");
        assert_eq!(input.jk2_selection().unwrap().weapon, 2);
    }
    let spectator = frame(1300, |s| s.persistant[3] = 3);
    assert_eq!(input.jk2_last_weapon(&spectator), Ok(Outcome::Ignored));
}
