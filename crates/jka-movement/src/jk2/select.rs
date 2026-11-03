//! Pinned MVSDK CGame weapon / Force / inventory selection. Selections are
//! native JK2 IDs (`cg.weaponSelect`, `cg.forceSelect`, `cg.itemSelect`, with
//! -1 for none); playerstates are raw wire states of the connection version.
use super::{State, Version};
use crate::ffi;

#[repr(C)]
struct Wire {
    fields: *const u32,
    count: i32,
    stats: *const i32,
    persistant: *const i32,
    ammo: *const i32,
    powerups: *const i32,
}

fn wire(state: &State) -> Wire {
    Wire {
        fields: state.fields.as_ptr(),
        count: state.fields.len() as i32,
        stats: state.stats.as_ptr(),
        persistant: state.persistant.as_ptr(),
        ammo: state.ammo.as_ptr(),
        powerups: state.powerups.as_ptr(),
    }
}

/// What the CGame command did with its selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Returned before touching the selection (follow, emplaced, bad slot).
    Ignored,
    /// The selection display time was updated; the value may be unchanged.
    Selected,
    /// CG_Weapon_f slot 1 on a drawn saber: `sv_saberswitch`.
    SaberSwitch,
    /// Handled without a display-time update.
    Unchanged,
}

fn run(code: i32) -> Result<Outcome, String> {
    match code {
        0 => Ok(Outcome::Ignored),
        1 => Ok(Outcome::Selected),
        2 => Ok(Outcome::SaberSwitch),
        3 => Ok(Outcome::Unchanged),
        _ => Err("JK2 selection received an invalid native playerstate".into()),
    }
}

fn guard(states: &[&State]) -> Result<ffi::NativeGuard, String> {
    if ffi::in_callback() {
        return Err("Cannot run JK2 selection inside a collision callback".into());
    }
    if states.windows(2).any(|pair| pair[0].version != pair[1].version) {
        return Err("JK2 selection states have different versions".into());
    }
    Ok(ffi::NativeGuard::new())
}

pub fn weapon_selectable(gameplay: Version, predicted: &State, weapon: i32) -> Result<bool, String> {
    let _guard = guard(&[predicted])?;
    match unsafe {
        jk2_cg_weapon_selectable(predicted.version as i32, gameplay as i32, &wire(predicted), weapon)
    } {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err("JK2 selection received an invalid native playerstate".into()),
    }
}

/// CG_NextWeapon_f / CG_PrevWeapon_f.
pub fn cycle_weapon(
    gameplay: Version,
    snap: &State,
    predicted: &State,
    select: &mut i32,
    forward: bool,
) -> Result<Outcome, String> {
    let _guard = guard(&[snap, predicted])?;
    run(unsafe {
        jk2_cg_cycle_weapon(
            snap.version as i32,
            gameplay as i32,
            &wire(snap),
            &wire(predicted),
            select,
            i32::from(forward),
        )
    })
}

/// CG_Weapon_f with its `/weapon <slot>` argument.
pub fn weapon_slot(
    gameplay: Version,
    snap: &State,
    predicted: &State,
    select: &mut i32,
    slot: i32,
) -> Result<Outcome, String> {
    let _guard = guard(&[snap, predicted])?;
    run(unsafe {
        jk2_cg_weapon_slot(
            snap.version as i32,
            gameplay as i32,
            &wire(snap),
            &wire(predicted),
            select,
            slot,
        )
    })
}

/// CG_OutOfAmmoChange with `cg_autoswitch`.
pub fn out_of_ammo(
    gameplay: Version,
    predicted: &State,
    select: &mut i32,
    old_weapon: i32,
    autoswitch: i32,
) -> Result<Outcome, String> {
    let _guard = guard(&[predicted])?;
    run(unsafe {
        jk2_cg_out_of_ammo(
            predicted.version as i32,
            gameplay as i32,
            &wire(predicted),
            select,
            old_weapon,
            autoswitch,
        )
    })
}

/// CG_NextForcePower_f / CG_PrevForcePower_f.
pub fn cycle_force(
    gameplay: Version,
    snap: &State,
    select: &mut i32,
    forward: bool,
) -> Result<Outcome, String> {
    let _guard = guard(&[snap])?;
    run(unsafe {
        jk2_cg_cycle_force(snap.version as i32, gameplay as i32, &wire(snap), select, i32::from(forward))
    })
}

/// CG_NextInventory_f / CG_PrevInventory_f.
pub fn cycle_inventory(
    gameplay: Version,
    snap: &State,
    select: &mut i32,
    forward: bool,
) -> Result<Outcome, String> {
    let _guard = guard(&[snap])?;
    run(unsafe {
        jk2_cg_cycle_inventory(snap.version as i32, gameplay as i32, &wire(snap), select, i32::from(forward))
    })
}

/// What the server's WP_InitForcePowers checks a `forcepowers` userinfo with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForceRules {
    pub max_rank: i32,
    /// HasSetSaberOnly(): every non-saber weapon disabled outside Jedi Master.
    pub free_saber: bool,
    /// g_forceBasedTeams side of a red/blue team, otherwise 0.
    pub team_force: i32,
    pub gametype: i32,
    pub force_disable: i32,
}

/// Pinned BG_LegalizedForcePowers. Returns the legal configuration and
/// whether the input already was legal (the server's `warnClient` test).
pub fn legalize_force_powers(
    version: Version,
    gameplay: Version,
    powers: &str,
    rules: ForceRules,
) -> Result<(String, bool), String> {
    if ffi::in_callback() {
        return Err("Cannot legalize JK2 force powers inside a collision callback".into());
    }
    if powers.len() >= 127 || powers.bytes().any(|byte| byte == 0) {
        return Err("Invalid JK2 forcepowers string".into());
    }
    let mut buffer = [0u8; 128];
    buffer[..powers.len()].copy_from_slice(powers.as_bytes());
    let _guard = ffi::NativeGuard::new();
    let result = unsafe {
        jk2_legalize_force_powers(
            version as i32,
            gameplay as i32,
            buffer.as_mut_ptr().cast(),
            buffer.len() as i32,
            rules.max_rank,
            i32::from(rules.free_saber),
            rules.team_force,
            rules.gametype,
            rules.force_disable,
        )
    };
    if result < 0 {
        return Err("JK2 force power legalization failed".into());
    }
    let end = buffer.iter().position(|&byte| byte == 0).unwrap_or(buffer.len());
    Ok((String::from_utf8_lossy(&buffer[..end]).into_owned(), result == 1))
}

unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn jk2_legalize_force_powers(
        version: i32,
        gameplay: i32,
        powers: *mut std::ffi::c_char,
        size: i32,
        max_rank: i32,
        free_saber: i32,
        team_force: i32,
        gametype: i32,
        force_disable: i32,
    ) -> i32;    fn jk2_cg_weapon_selectable(version: i32, gameplay: i32, predicted: *const Wire, weapon: i32) -> i32;
    fn jk2_cg_cycle_weapon(
        version: i32,
        gameplay: i32,
        snap: *const Wire,
        predicted: *const Wire,
        select: *mut i32,
        forward: i32,
    ) -> i32;
    fn jk2_cg_weapon_slot(
        version: i32,
        gameplay: i32,
        snap: *const Wire,
        predicted: *const Wire,
        select: *mut i32,
        slot: i32,
    ) -> i32;
    fn jk2_cg_out_of_ammo(
        version: i32,
        gameplay: i32,
        predicted: *const Wire,
        select: *mut i32,
        old_weapon: i32,
        autoswitch: i32,
    ) -> i32;
    fn jk2_cg_cycle_force(version: i32, gameplay: i32, snap: *const Wire, select: *mut i32, forward: i32) -> i32;
    fn jk2_cg_cycle_inventory(version: i32, gameplay: i32, snap: *const Wire, select: *mut i32, forward: i32) -> i32;
}
