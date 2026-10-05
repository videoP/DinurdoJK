//! OpenJK `CG_Item` (codemp/cgame/cg_ents.c) placement rules.
//!
//! This module decides *what* an `ET_ITEM` submits: which model, where, with
//! which axis, tint, alpha and custom shader. Drawing the resulting MD3 or
//! Ghoul2 model stays with the entity and player presenters. The item table
//! itself is the pinned OpenJK `bg_itemlist`, read through `jka_movement`.

use jka_movement::ItemInfo;

// itemType_t
pub const IT_WEAPON: i32 = 1;
pub const IT_ARMOR: i32 = 3;
pub const IT_HEALTH: i32 = 4;
pub const IT_POWERUP: i32 = 5;
pub const IT_HOLDABLE: i32 = 6;
pub const IT_TEAM: i32 = 8;
// powerup_t
const PW_REDFLAG: i32 = 4;
const PW_BLUEFLAG: i32 = 5;
const PW_NEUTRALFLAG: i32 = 6;
const PW_FORCE_ENLIGHTENED_LIGHT: i32 = 12;
const PW_FORCE_ENLIGHTENED_DARK: i32 = 13;
// holdable_t
const HI_SEEKER: i32 = 1;
const HI_SHIELD: i32 = 2;
const HI_BINOCULARS: i32 = 5;
// weapon_t
const WP_BLASTER: i32 = 5;
const WP_DISRUPTOR: i32 = 6;
const WP_BOWCASTER: i32 = 7;
const WP_REPEATER: i32 = 8;
const WP_DEMP2: i32 = 9;
const WP_FLECHETTE: i32 = 10;
const WP_ROCKET_LAUNCHER: i32 = 11;
const WP_THERMAL: i32 = 12;
const WP_TRIP_MINE: i32 = 13;
const WP_DET_PACK: i32 = 14;
// forceSide
const FORCE_LIGHTSIDE: i32 = 1;
const FORCE_DARKSIDE: i32 = 2;

const EF_DEAD: i32 = 1 << 1;
const EF_NODRAW: i32 = 1 << 8;
const EF_ITEMPLACEHOLDER: i32 = 1 << 23;
const EF_DROPPEDWEAPON: i32 = 1 << 25;

const ITEM_SCALEUP_TIME: i32 = 1000;
pub const ITEM_HOLO_MODEL: &str = "models/map_objects/mp/holo.md3";
const PLACEHOLDER_SHADER: &str = "powerups/placeholder";
const REZ_OUT_SHADER: &str = "powerups/rezout";

/// The per-entity inputs CG_Item reads.
#[derive(Clone, Copy, Debug)]
pub struct ItemInput {
    pub number: u16,
    /// `cent->lerpOrigin`.
    pub origin: [f32; 3],
    /// `cent->currentState.angles` (items do not use lerpAngles).
    pub angles: [f32; 3],
    pub e_flags: i32,
    /// `cg.time`.
    pub time: i32,
    /// `cent->miscTime`, set by EV_ITEM_RESPAWN.
    pub misc_time: i32,
    /// `cg.snap->ps.fd.forceSide` of the viewed player.
    pub force_side: i32,
    /// `cg_weapons[giTag].weaponMidpoint` (R_ModelBounds of world_model[0];
    /// zero for Ghoul2 weapon models, whose bounds OpenJK reports as zero).
    pub weapon_midpoint: [f32; 3],
}

/// One `R_AddRefEntityToScene` from CG_Item.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemDraw {
    pub model: String,
    pub origin: [f32; 3],
    /// Possibly non-normalized: weapons are scaled 1.5, flags 0.7.
    pub axis: [[f32; 3]; 3],
    /// shaderRGBA / 255 as used with RF_RGB_TINT and RF_FORCE_ENT_ALPHA.
    pub rgba: [f32; 4],
    pub custom_shader: Option<&'static str>,
}

/// CG_RegisterItemVisuals `models[0]`. The renderer dispatches on that
/// model's type, so for thermal/trip mine/det pack the `_pu.md3` pickup model
/// is drawn and the Ghoul2 world model (oriented for the hand bolt) is not.
pub fn registered_item_model(item: &ItemInfo) -> &str {
    let pickup_md3 = item.item_type == IT_WEAPON && matches!(item.tag, WP_THERMAL | WP_TRIP_MINE | WP_DET_PACK);
    if pickup_md3 && !item.world_model2.is_empty() { &item.world_model2 } else { &item.world_model }
}

/// CG_GreyItem: enlightenment of the opposing side is shown disabled.
pub fn grey_item(item: &ItemInfo, force_side: i32) -> bool {
    item.item_type == IT_POWERUP
        && match force_side {
            FORCE_LIGHTSIDE => item.tag == PW_FORCE_ENLIGHTENED_DARK,
            FORCE_DARKSIDE => item.tag == PW_FORCE_ENLIGHTENED_LIGHT,
            _ => false,
        }
}

/// cg.autoAngles yaw: one revolution every 2048 ms.
pub fn auto_yaw(time: i32) -> f32 {
    (time & 2047) as f32 * 360.0 / 2048.0
}

/// Where CG_Item plays `mp/itemcone.efx`: the light cone over the pedestal
/// hologram of a placed weapon or powerup, unless it is greyed out. The effect
/// belongs to the FX runtime, so it is not part of [`cg_item`]'s draws.
pub fn item_cone_origin(item: &ItemInfo, input: &ItemInput) -> Option<[f32; 3]> {
    let dropped = input.e_flags & EF_DROPPEDWEAPON != 0;
    let spinning = item.item_type == IT_WEAPON || item.item_type == IT_POWERUP;
    (!item.world_model.is_empty() && spinning && !dropped && !grey_item(item, input.force_side)).then_some(input.origin)
}

/// Everything CG_Item submits for one item, in submission order, except the
/// FX-runtime cone of [`item_cone_origin`].
pub fn cg_item(item: &ItemInfo, input: &ItemInput, angles_to_axis: fn([f32; 3]) -> [[f32; 3]; 3]) -> Vec<ItemDraw> {
    let mut draws = Vec::new();
    let mut e_flags = input.e_flags;
    if e_flags & EF_NODRAW != 0 && e_flags & EF_ITEMPLACEHOLDER != 0 {
        e_flags &= !EF_NODRAW;
    }
    if item.world_model.is_empty() {
        return draws; // bg_itemlist[0] / modelindex 0
    }
    let dropped = e_flags & EF_DROPPEDWEAPON != 0;
    let spinning = item.item_type == IT_WEAPON || item.item_type == IT_POWERUP;
    let grey = grey_item(item, input.force_side);

    // Pedestal hologram under placed (not dropped) weapons and powerups.
    if spinning && !dropped {
        let tint = if grey { 150.0 / 255.0 } else { 1.0 };
        draws.push(ItemDraw {
            model: ITEM_HOLO_MODEL.to_owned(),
            origin: input.origin,
            axis: angles_to_axis(input.angles),
            rgba: [tint, tint, tint, 1.0],
            custom_shader: None,
        });
    }
    if e_flags & EF_NODRAW != 0 {
        return draws;
    }

    let mut origin = input.origin;
    if spinning && !dropped {
        origin[2] += 16.0;
    }
    if (!dropped || item.item_type == IT_POWERUP) && spinning {
        // Items bob up and down continuously.
        let scale = 0.005 + f32::from(input.number) * 0.00001;
        origin[2] += 4.0 + ((input.time + 1000) as f32 * scale).cos() * 4.0;
    } else {
        match (item.item_type, item.tag) {
            (IT_HOLDABLE, HI_SEEKER) => origin[2] += 5.0,
            (IT_HOLDABLE, HI_SHIELD | HI_BINOCULARS) => origin[2] += 2.0,
            (IT_HEALTH, _) => origin[2] += 2.0,
            (IT_ARMOR, _) if item.quantity == 100 => origin[2] += 7.0,
            _ => {}
        }
    }

    // Only weapons and powerups rotate (cg.autoAngles).
    let rotates = (!dropped || item.item_type == IT_POWERUP) && spinning;
    let mut axis = if rotates {
        angles_to_axis([0.0, auto_yaw(input.time), 0.0])
    } else {
        angles_to_axis(input.angles)
    };

    // Weapons have their origin where they attach to player models; offset
    // them so they rotate about their middle.
    if !dropped {
        if item.item_type == IT_WEAPON {
            let mid = input.weapon_midpoint;
            for i in 0..3 {
                origin[i] -= mid[0] * axis[0][i] + mid[1] * axis[1][i] + mid[2] * axis[2][i];
            }
            origin[2] += 8.0; // an extra height boost
        }
    } else {
        origin[2] -= match item.tag {
            WP_BLASTER | WP_REPEATER | WP_THERMAL => 12.0,
            WP_DISRUPTOR => 13.0,
            WP_BOWCASTER | WP_TRIP_MINE | WP_DET_PACK => 16.0,
            WP_DEMP2 => 10.0,
            WP_FLECHETTE => 6.0,
            WP_ROCKET_LAUNCHER => 11.0,
            _ => 8.0,
        };
    }

    let mut draw = ItemDraw {
        model: registered_item_model(item).to_owned(),
        origin,
        axis,
        rgba: [1.0; 4],
        custom_shader: None,
    };

    if grey {
        draw.rgba = [150.0 / 255.0, 150.0 / 255.0, 150.0 / 255.0, 200.0 / 255.0];
        draw.custom_shader = Some(if item.tag == PW_FORCE_ENLIGHTENED_LIGHT {
            "gfx/misc/mp_light_enlight_disable"
        } else {
            "gfx/misc/mp_dark_enlight_disable"
        });
        draws.push(draw);
        return draws;
    }

    if e_flags & EF_ITEMPLACEHOLDER != 0 {
        // Picked up; a dropped item that was picked up is not shown at all.
        if e_flags & EF_DEAD != 0 {
            return draws;
        }
        draw.rgba = [0.0, 200.0 / 255.0, 85.0 / 255.0, 1.0];
        draw.custom_shader = Some(PLACEHOLDER_SHADER);
    }

    // Weapons are presented 1.5x their held size.
    if item.item_type == IT_WEAPON {
        axis = axis.map(|row| row.map(|value| value * 1.5));
        draw.axis = axis;
    }

    let msec = input.time - input.misc_time;
    if item.item_type != IT_TEAM
        && (0..ITEM_SCALEUP_TIME).contains(&msec)
        && e_flags & (EF_ITEMPLACEHOLDER | EF_DROPPEDWEAPON) == 0
    {
        // Just respawned: fade the model in, then draw the rez-out shader
        // over it in the respawn tint.
        let alpha = msec as f32 / ITEM_SCALEUP_TIME as f32;
        let a = ((alpha * 255.0) as i32).max(1);
        draw.rgba[3] = a as f32 / 255.0;
        let mut rez = draw.clone();
        draws.push(draw);
        rez.custom_shader = Some(REZ_OUT_SHADER);
        rez.rgba = [0.0, 200.0 / 255.0, 85.0 / 255.0, 1.0];
        draws.push(rez);
        return draws;
    }

    if item.item_type == IT_TEAM && matches!(item.tag, PW_REDFLAG | PW_BLUEFLAG | PW_NEUTRALFLAG) {
        draw.axis = draw.axis.map(|row| row.map(|value| value * 0.7));
    }
    draws.push(draw);
    draws
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity_yaw(angles: [f32; 3]) -> [[f32; 3]; 3] {
        let (s, c) = angles[1].to_radians().sin_cos();
        [[c, s, 0.0], [-s, c, 0.0], [0.0, 0.0, 1.0]]
    }

    fn item(classname: &str, model: &str, item_type: i32, tag: i32, quantity: i32) -> ItemInfo {
        ItemInfo { classname: classname.into(), world_model: model.into(), world_model2: String::new(), view_model: String::new(), item_type, tag, quantity }
    }

    fn input(time: i32) -> ItemInput {
        ItemInput {
            number: 100,
            origin: [10.0, 20.0, 30.0],
            angles: [0.0, 90.0, 0.0],
            e_flags: 0,
            time,
            misc_time: -100_000,
            force_side: FORCE_LIGHTSIDE,
            weapon_midpoint: [0.0; 3],
        }
    }

    #[test]
    fn placed_weapon_gets_holo_bob_autorotation_height_and_scale() {
        let repeater = item("weapon_repeater", "models/weapons2/heavy_repeater/heavy_repeater_w.glm", IT_WEAPON, 8, 100);
        let draws = cg_item(&repeater, &input(1024), identity_yaw);
        assert_eq!(draws.len(), 2);
        assert_eq!(draws[0].model, ITEM_HOLO_MODEL);
        assert_eq!(draws[0].origin, [10.0, 20.0, 30.0]);
        let bob = 4.0 + ((1024 + 1000) as f32 * (0.005 + 100.0 * 0.00001)).cos() * 4.0;
        assert!((draws[1].origin[2] - (30.0 + 16.0 + bob + 8.0)).abs() < 1e-4);
        // autoAngles yaw 180 at t=1024, scaled 1.5.
        assert!((draws[1].axis[0][0] + 1.5).abs() < 1e-5);
    }

    #[test]
    fn ammo_uses_entity_angles_and_placeholder_tint_after_pickup() {
        let rockets = item("ammo_rockets", "models/items/rockets.md3", 2, 5, 3);
        let mut placed = input(5000);
        let draws = cg_item(&rockets, &placed, identity_yaw);
        assert_eq!(draws.len(), 1);
        assert_eq!(draws[0].origin, [10.0, 20.0, 30.0]);
        assert!((draws[0].axis[0][1] - 1.0).abs() < 1e-5, "yaw 90 from es.angles");

        placed.e_flags = EF_ITEMPLACEHOLDER | EF_NODRAW;
        let draws = cg_item(&rockets, &placed, identity_yaw);
        assert_eq!(draws[0].custom_shader, Some(PLACEHOLDER_SHADER));
        placed.e_flags |= EF_DEAD;
        assert!(cg_item(&rockets, &placed, identity_yaw).is_empty());
    }

    #[test]
    fn respawn_fades_in_with_rez_overlay_and_opposing_enlightenment_greys() {
        let medpak = item("item_medpak_instant", "models/map_objects/mp/medpac.md3", IT_HEALTH, 0, 25);
        let mut respawned = input(5250);
        respawned.misc_time = 5000;
        let draws = cg_item(&medpak, &respawned, identity_yaw);
        assert_eq!(draws.len(), 2);
        assert!((draws[0].rgba[3] - 63.0 / 255.0).abs() < 1e-5);
        assert_eq!(draws[1].custom_shader, Some(REZ_OUT_SHADER));
        assert_eq!(draws[0].origin[2], 32.0, "health sits 2 units up");

        let dark = item("item_force_enlighten_dark", "models/map_objects/mp/dk_enlightenment.md3", IT_POWERUP, 13, 25);
        let draws = cg_item(&dark, &input(0), identity_yaw);
        assert_eq!(draws[0].rgba[0], 150.0 / 255.0, "holo greyed for the light-side viewer");
        assert_eq!(draws[1].custom_shader, Some("gfx/misc/mp_dark_enlight_disable"));
        assert_eq!(draws[1].rgba[3], 200.0 / 255.0);
    }

    #[test]
    fn placed_weapons_and_powerups_get_a_cone_unless_dropped_or_greyed() {
        let repeater = item("weapon_repeater", "x.glm", IT_WEAPON, 8, 100);
        assert_eq!(item_cone_origin(&repeater, &input(0)), Some([10.0, 20.0, 30.0]));
        let mut dropped = input(0);
        dropped.e_flags = EF_DROPPEDWEAPON;
        assert_eq!(item_cone_origin(&repeater, &dropped), None);
        let medpak = item("item_medpak_instant", "models/map_objects/mp/medpac.md3", IT_HEALTH, 0, 25);
        assert_eq!(item_cone_origin(&medpak, &input(0)), None, "only weapons and powerups have a holo");
        let dark = item("item_force_enlighten_dark", "x.md3", IT_POWERUP, 13, 25);
        assert_eq!(item_cone_origin(&dark, &input(0)), None, "greyed for the light-side viewer");
        let light = item("item_force_enlighten_light", "x.md3", IT_POWERUP, 12, 25);
        assert!(item_cone_origin(&light, &input(0)).is_some());
    }

    #[test]
    fn thermal_trip_and_det_pickups_draw_their_pu_md3() {
        let mut trip = item("weapon_trip_mine", "models/weapons2/laser_trap/laser_trap_w.glm", IT_WEAPON, WP_TRIP_MINE, 3);
        trip.world_model2 = "models/weapons2/laser_trap/laser_trap_pu.md3".into();
        assert_eq!(cg_item(&trip, &input(0), identity_yaw)[1].model, "models/weapons2/laser_trap/laser_trap_pu.md3");
        let mut repeater = item("weapon_repeater", "models/weapons2/heavy_repeater/heavy_repeater_w.glm", IT_WEAPON, 8, 100);
        repeater.world_model2 = "unused.md3".into();
        assert!(cg_item(&repeater, &input(0), identity_yaw)[1].model.ends_with("_w.glm"));
    }

    #[test]
    fn nodraw_item_keeps_only_its_pedestal() {
        let repeater = item("weapon_repeater", "x.glm", IT_WEAPON, 8, 100);
        let mut hidden = input(0);
        hidden.e_flags = EF_NODRAW;
        let draws = cg_item(&repeater, &hidden, identity_yaw);
        assert_eq!(draws.len(), 1);
        assert_eq!(draws[0].model, ITEM_HOLO_MODEL);
    }
}
