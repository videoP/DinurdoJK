//! JK2 CGame selection state and usercmd selection fields. The pinned MVSDK
//! selection commands run natively; this owns cg.weaponSelect / forceSelect /
//! itemSelect (native JK2 IDs, -1 = none) and their display times, and mirrors
//! them into the presentation fields the existing HUD reads.
use super::{inventory_tag_for_item_index, LiveInput};
use jka_movement::jk2::{self as native, select::{self, Outcome}};
use jka_protocol::{jk2::presentation, server::PlayerState};

const WEAPON_SELECT_TIME: i32 = 1400; // cg_local.h
const WP_SABER: i32 = 2;
const WP_BRYAR_PISTOL: i32 = 3;
const WP_ROCKET_LAUNCHER: i32 = 10;
const WP_THERMAL: i32 = 11;
const WP_TRIP_MINE: i32 = 12;
const WP_DET_PACK: i32 = 13;
const WP_NUM_WEAPONS: i32 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Jk2Selection {
    pub weapon: i32,
    pub force: i32,
    pub item: i32,
    /// JKA /weaplast history, stored as raw JK2 weapon IDs.
    last_weapon_select: [i32; 2],
    weapon_time: i32,
    force_time: i32,
    item_time: i32,
    spawn_count: Option<i32>,
}

impl Default for Jk2Selection {
    /// CG_Init: weaponSelect WP_BRYAR_PISTOL, forceSelect / itemSelect -1.
    fn default() -> Self {
        Self {
            weapon: WP_BRYAR_PISTOL,
            force: -1,
            item: -1,
            last_weapon_select: [0; 2],
            weapon_time: 0,
            force_time: 0,
            item_time: 0,
            spawn_count: None,
        }
    }
}

/// One CGame frame's selection inputs: cg.snap->ps and cg.predictedPlayerState
/// as raw wire states, cg.time, and the mapped state the HUD presents.
pub(crate) struct Jk2Frame {
    pub gameplay: native::Version,
    pub snap: native::State,
    pub predicted: native::State,
    pub time: i32,
    pub presented: PlayerState,
}

fn field(state: &native::State, name: &str) -> i32 {
    state.field_bits(name).unwrap_or(0) as i32
}

impl LiveInput {
    /// Select JK2 CGame selection and usercmd rules for this connection.
    pub fn set_jk2(&mut self, enabled: bool) {
        self.jk2 = enabled.then(Jk2Selection::default);
    }

    pub fn is_jk2(&self) -> bool {
        self.jk2.is_some()
    }

    #[cfg(test)]
    pub(crate) fn jk2_selection(&self) -> Option<Jk2Selection> {
        self.jk2
    }

    /// CG_Respawn on a new spawn count, and CG_Draw2D's expired
    /// CG_DrawForceSelect taking the server's selected power again.
    pub(crate) fn sync_jk2(&mut self, frame: &Jk2Frame) {
        let Some(selection) = self.jk2.as_mut() else { return };
        let ps = &frame.snap;
        if selection.spawn_count != Some(ps.persistant[4]) {
            selection.spawn_count = Some(ps.persistant[4]);
            selection.weapon = field(ps, "weapon");
            selection.last_weapon_select = [0; 2];
            selection.weapon_time = frame.time;
        }
        // Non-spectator, alive status branch: only the most recent selector
        // draws, and only CG_DrawForceSelect resets its selection on expiry.
        let inventory_newer = selection.item_time + WEAPON_SELECT_TIME > selection.weapon_time + WEAPON_SELECT_TIME;
        let best = if inventory_newer { selection.item_time } else { selection.weapon_time };
        if ps.persistant[3] != 3
            && ps.stats[0] > 0
            && selection.force_time > best
            && selection.force_time + WEAPON_SELECT_TIME < frame.time
        {
            selection.force = field(ps, "fd.forcePowerSelected");
        }
        self.mirror_jk2(&frame.presented);
    }

    fn mirror_jk2(&mut self, presented: &PlayerState) {
        let Some(selection) = self.jk2 else { return };
        if let Some(weapon) = presentation::presented_weapon(selection.weapon) {
            if i32::from(self.weapon_select) != weapon {
                self.set_selected_weapon(weapon as u8);
            }
        }
        self.force_select = if selection.force >= 0 {
            selection.force as u8
        } else {
            presented.field_i32("fd.forcePowerSelected").unwrap_or(0).clamp(0, 17) as u8
        };
        self.inventory_select = if selection.item >= 0 {
            presentation::presented_holdable(selection.item).unwrap_or(0) as u8
        } else {
            inventory_tag_for_item_index(presented.stats[1]).unwrap_or(0)
        };
    }

    fn jk2_command(
        &mut self,
        frame: &Jk2Frame,
        run: impl FnOnce(&mut Jk2Selection) -> Result<Outcome, String>,
        time: fn(&mut Jk2Selection) -> &mut i32,
    ) -> Result<Outcome, String> {
        let selection = self.jk2.as_mut().ok_or("JK2 selection on a JKA connection")?;
        let outcome = run(selection)?;
        if outcome == Outcome::Selected {
            *time(selection) = frame.time;
        }
        if outcome == Outcome::SaberSwitch {
            self.queue_generic_command(1); // CG_Weapon_f: sv_saberswitch
        }
        self.mirror_jk2(&frame.presented);
        Ok(outcome)
    }

    /// CG_NextWeapon_f / CG_PrevWeapon_f.
    pub(crate) fn jk2_cycle_weapon(&mut self, frame: &Jk2Frame, forward: bool) -> Result<Outcome, String> {
        self.jk2_command(frame, |s| {
            select::cycle_weapon(frame.gameplay, &frame.snap, &frame.predicted, &mut s.weapon, forward)
        }, |s| &mut s.weapon_time)
    }

    /// CG_Weapon_f.
    pub(crate) fn jk2_weapon_slot(&mut self, frame: &Jk2Frame, slot: i32) -> Result<Outcome, String> {
        self.jk2_command(frame, |s| {
            select::weapon_slot(frame.gameplay, &frame.snap, &frame.predicted, &mut s.weapon, slot)
        }, |s| &mut s.weapon_time)
    }

    /// TaystJK /weaplast semantics for JK2, using native JK2 weapon IDs and
    /// the pinned MVSDK CG_WeaponSelectable rather than JKA's weapon table.
    pub(crate) fn jk2_last_weapon(&mut self, frame: &Jk2Frame) -> Result<Outcome, String> {
        self.jk2_command(frame, |s| {
            // Match the JKA extension's follow, spectator, and emplacement
            // guards. JK2 PM_SPECTATOR is 3 (JKA uses 4).
            if field(&frame.snap, "pm_flags") & 4096 != 0
                || field(&frame.snap, "pm_type") == 3
                || frame.snap.persistant[3] == 3
                || field(&frame.snap, "emplacedIndex") != 0
            {
                return Ok(Outcome::Ignored);
            }

            // Like JKA's CG_LastWeapon_f, the first invocation starts from
            // the server-equipped weapon, not a possibly pending usercmd.
            let equipped = field(&frame.snap, "weapon");
            if s.last_weapon_select[0] == 0 {
                s.last_weapon_select[0] = equipped;
            }
            if s.last_weapon_select[1] == 0 {
                s.last_weapon_select[1] = equipped;
            }
            if s.last_weapon_select[0] == s.last_weapon_select[1] {
                for candidate in (1..WP_NUM_WEAPONS).rev() {
                    if candidate != s.weapon
                        && select::weapon_selectable(frame.gameplay, &frame.predicted, candidate)?
                    {
                        s.last_weapon_select[1] = candidate;
                        break;
                    }
                }
            }
            if s.last_weapon_select[0] != s.weapon {
                s.last_weapon_select[1] = s.last_weapon_select[0];
                s.last_weapon_select[0] = s.weapon;
            }
            let previous = s.last_weapon_select[1];
            if !select::weapon_selectable(frame.gameplay, &frame.predicted, previous)? {
                return Ok(Outcome::Unchanged);
            }
            s.weapon = previous;
            Ok(Outcome::Selected)
        }, |s| &mut s.weapon_time)
    }

    /// CG_NextForcePower_f / CG_PrevForcePower_f.
    pub(crate) fn jk2_cycle_force(&mut self, frame: &Jk2Frame, forward: bool) -> Result<Outcome, String> {
        self.jk2_command(frame, |s| {
            select::cycle_force(frame.gameplay, &frame.snap, &mut s.force, forward)
        }, |s| &mut s.force_time)
    }

    /// CG_NextInventory_f / CG_PrevInventory_f.
    pub(crate) fn jk2_cycle_inventory(&mut self, frame: &Jk2Frame, forward: bool) -> Result<Outcome, String> {
        self.jk2_command(frame, |s| {
            select::cycle_inventory(frame.gameplay, &frame.snap, &mut s.item, forward)
        }, |s| &mut s.item_time)
    }

    /// cg_event.c EV_NOAMMO for the local client with its raw JK2 eventParm,
    /// then CG_OutOfAmmoChange (which runs regardless of cg_autoswitch 0).
    pub(crate) fn jk2_out_of_ammo(&mut self, frame: &Jk2Frame, parm: i32, autoswitch: i32) -> Result<(), String> {
        let old = if parm != 0 && parm < WP_NUM_WEAPONS {
            field(&frame.snap, "weapon")
        } else if parm != 0 {
            parm - WP_NUM_WEAPONS
        } else {
            0
        };
        self.jk2_command(frame, |s| {
            select::out_of_ammo(frame.gameplay, &frame.predicted, &mut s.weapon, old, autoswitch)
        }, |s| &mut s.weapon_time)
        .map(drop)
    }

    /// CG_ItemPickup's weapon auto-select for a presentation weapon tag.
    pub(crate) fn jk2_pickup(&mut self, frame: &Jk2Frame, presented_tag: i32, autoswitch: i32) {
        let Some(tag) = presentation::wire_weapon(presented_tag) else { return };
        let current = field(&frame.snap, "weapon");
        let better = tag > current && current != WP_SABER;
        let allowed = match autoswitch {
            1 => better && !matches!(tag, WP_TRIP_MINE | WP_DET_PACK | WP_THERMAL | WP_ROCKET_LAUNCHER),
            2 => better,
            _ => false, // 0, and the commented-out upstream level 3
        };
        if !allowed {
            return;
        }
        let emplaced = field(&frame.snap, "emplacedIndex") != 0;
        if let Some(selection) = self.jk2.as_mut() {
            if !emplaced {
                selection.weapon_time = frame.time;
            }
            selection.weapon = tag;
        }
        self.mirror_jk2(&frame.presented);
    }
}

/// jk2mv CL_FinishMove: cgame selections in native JK2 IDs (-1 is sent as
/// 255), and only generic commands JK2 has.
pub(super) fn finish_move(selection: Jk2Selection, cmd: &mut jka_protocol::netchan::UserCmd) {
    cmd.weapon = selection.weapon as u8;
    cmd.force_selection = selection.force as u8;
    cmd.inventory_selection = selection.item as u8;
    cmd.generic_command = presentation::wire_generic_command(cmd.generic_command).unwrap_or(0);
}

#[cfg(test)]
mod tests;
