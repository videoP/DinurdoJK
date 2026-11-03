/* Pinned MVSDK CGame selection commands (4c7c463638192aead1ff3de8a88cc5de266bf3a2):
 * cg_weapons.c CG_WeaponSelectable / CG_NextWeapon_f / CG_PrevWeapon_f /
 * CG_Weapon_f / CG_OutOfAmmoChange and cg_main.c CG_Next/PrevForcePower_f,
 * CG_Next/PrevInventory_f. cg.* selection values are passed in and out; the
 * shared BG_CycleForce / BG_CycleInven / weaponData are the compiled SDK's. */
#include "jk2_host_internal.h"

extern int bg_numItems;
void BG_CycleForce(playerState_t *ps, int direction); /* declared by cg_local.h upstream */

static qboolean CG_WeaponSelectable( const playerState_t *predicted, int i ) {
	if (!i)
	{
		return qfalse;
	}
	if (i < 0 || i >= WP_NUM_WEAPONS)
	{ /* host guard: CGame indexes weaponData with its own cg.weaponSelect */
		return qfalse;
	}

	if (predicted->ammo[weaponData[i].ammoIndex] < weaponData[i].energyPerShot &&
		predicted->ammo[weaponData[i].ammoIndex] < weaponData[i].altEnergyPerShot)
	{
		return qfalse;
	}

	if (i == WP_DET_PACK && predicted->ammo[weaponData[i].ammoIndex] < 1 &&
		!predicted->hasDetPackPlanted)
	{
		return qfalse;
	}

	if ( ! (predicted->stats[ STAT_WEAPONS ] & ( 1 << i ) ) ) {
		return qfalse;
	}

	return qtrue;
}

static int load(int version, int gameplay, const jk2_wire *wire, playerState_t *ps) {
	return jk2_select_versions(version, gameplay) && jk2_import_wire(version, wire, ps);
}

int jk2_cg_weapon_selectable(int version, int gameplay, const jk2_wire *predicted, int weapon) {
	playerState_t ps;
	if (!load(version, gameplay, predicted, &ps)) return -1;
	return CG_WeaponSelectable(&ps, weapon);
}

/* CG_NextWeapon_f (forward) / CG_PrevWeapon_f. */
int jk2_cg_cycle_weapon(int version, int gameplay, const jk2_wire *wire, const jk2_wire *predicted_wire,
	int32_t *weaponSelect, int forward) {
	playerState_t snap, predicted;
	int i, original;
	if (!load(version, gameplay, wire, &snap) || !jk2_import_wire(version, predicted_wire, &predicted)) return -1;

	if ( snap.pm_flags & PMF_FOLLOW ) {
		return 0;
	}

	if (snap.emplacedIndex)
	{
		return 0;
	}

	original = *weaponSelect;

	for ( i = 0 ; i < 16 ; i++ ) {
		if (forward) {
			(*weaponSelect)++;
			if ( *weaponSelect == 16 ) {
				*weaponSelect = 0;
			}
		} else {
			(*weaponSelect)--;
			if ( *weaponSelect == -1 ) {
				*weaponSelect = 15;
			}
		}
		if ( CG_WeaponSelectable( &predicted, *weaponSelect ) ) {
			break;
		}
	}
	if ( i == 16 ) {
		*weaponSelect = original;
	}
	return 1;
}

/* CG_Weapon_f. */
int jk2_cg_weapon_slot(int version, int gameplay, const jk2_wire *wire, const jk2_wire *predicted_wire,
	int32_t *weaponSelect, int num) {
	playerState_t snap, predicted;
	if (!load(version, gameplay, wire, &snap) || !jk2_import_wire(version, predicted_wire, &predicted)) return -1;

	if ( snap.pm_flags & PMF_FOLLOW ) {
		return 0;
	}

	if (snap.emplacedIndex)
	{
		return 0;
	}

	if ( num < 1 || num > 15 ) {
		return 0;
	}

	if (num == 1 && snap.weapon == WP_SABER)
	{
		if (snap.weaponTime < 1)
		{
			return 2;
		}
		return 0;
	}

	//rww - hack to make weapon numbers same as single player
	if (num > WP_STUN_BATON)
	{
		num++;
	}
	else
	{
		if (snap.stats[STAT_WEAPONS] & (1 << WP_SABER))
		{
			num = WP_SABER;
		}
		else
		{
			num = WP_STUN_BATON;
		}
	}

	if (num > WP_DET_PACK+1)
	{ //other weapons are off limits due to not actually being weapon weapons
		return 0;
	}

	if (num >= WP_THERMAL)
	{
		int weap, i = 0;

		if (snap.weapon >= WP_THERMAL &&
			snap.weapon <= WP_DET_PACK)
		{
			// already in cycle range so start with next cycle item
			weap = snap.weapon + 1;
		}
		else
		{
			// not in cycle range, so start with thermal detonator
			weap = WP_THERMAL;
		}

		// prevent an endless loop
		while ( i <= 4 )
		{
			if (weap > WP_DET_PACK)
			{
				weap = WP_THERMAL;
			}

			if (CG_WeaponSelectable(&predicted, weap))
			{
				num = weap;
				break;
			}

			weap++;
			i++;
		}
	}

	if (!CG_WeaponSelectable(&predicted, num))
	{
		return 3;
	}

	if ( ! ( snap.stats[STAT_WEAPONS] & ( 1 << num ) ) )
	{
		if (num == WP_SABER)
		{ //don't have saber, try the stun baton on the same slot
			num = WP_STUN_BATON;

			if ( ! ( snap.stats[STAT_WEAPONS] & ( 1 << num ) ) )
			{
				return 1;
			}
		}
		else
		{
			return 1;		// don't have the weapon
		}
	}

	*weaponSelect = num;
	return 1;
}

/* CG_OutOfAmmoChange; `autoswitch` is cg_autoswitch. */
int jk2_cg_out_of_ammo(int version, int gameplay, const jk2_wire *predicted_wire, int32_t *weaponSelect,
	int oldWeapon, int autoswitch) {
	playerState_t predicted;
	int i;
	if (!load(version, gameplay, predicted_wire, &predicted)) return -1;

	for ( i = WP_DET_PACK ; i > 0 ; i-- )	//We don't want the emplaced or turret
	{
		if ( CG_WeaponSelectable( &predicted, i ) )
		{
			if (autoswitch != 1 || (i != WP_TRIP_MINE && i != WP_DET_PACK && i != WP_THERMAL && i != WP_ROCKET_LAUNCHER))
			{
				if (i != oldWeapon)
				{ //don't even do anything if we're just selecting the weapon we already have/had
					*weaponSelect = i;
					break;
				}
			}
		}
	}
	return 1;
}

/* CG_NextForcePower_f (forward) / CG_PrevForcePower_f, on a copy of cg.snap->ps. */
int jk2_cg_cycle_force(int version, int gameplay, const jk2_wire *wire, int32_t *forceSelect, int forward) {
	playerState_t snap;
	if (!load(version, gameplay, wire, &snap)) return -1;

	if (snap.pm_flags & PMF_FOLLOW)
	{
		return 0;
	}

	if (*forceSelect != -1)
	{
		snap.fd.forcePowerSelected = *forceSelect;
	}

	BG_CycleForce(&snap, forward ? 1 : -1);

	if (snap.fd.forcePowersKnown & (1 << snap.fd.forcePowerSelected))
	{
		*forceSelect = snap.fd.forcePowerSelected;
		return 1;
	}
	return 3;
}

/* CG_NextInventory_f (forward) / CG_PrevInventory_f, on a copy of cg.snap->ps. */
int jk2_cg_cycle_inventory(int version, int gameplay, const jk2_wire *wire, int32_t *itemSelect, int forward) {
	playerState_t snap;
	if (!load(version, gameplay, wire, &snap)) return -1;

	if (snap.pm_flags & PMF_FOLLOW)
	{
		return 0;
	}

	if (*itemSelect != -1)
	{
		snap.stats[STAT_HOLDABLE_ITEM] = BG_GetItemIndexByTag(*itemSelect, IT_HOLDABLE);
	}
	if (snap.stats[STAT_HOLDABLE_ITEM] < 0 || snap.stats[STAT_HOLDABLE_ITEM] >= bg_numItems)
	{ /* host guard: BG_CycleInven indexes bg_itemlist with the networked value */
		return -1;
	}
	BG_CycleInven(&snap, forward ? 1 : -1);

	if (snap.stats[STAT_HOLDABLE_ITEM])
	{
		*itemSelect = bg_itemlist[snap.stats[STAT_HOLDABLE_ITEM]].giTag;
		return 1;
	}
	return 3;
}
