//! `bg_itemlist` of the game whose filesystem is installed. JK2 items are
//! presented under JKA item indices and tags, but their asset paths (world and
//! view models, icons, pickup sounds) come from the pinned JK2 item list.
use jka_movement::ItemInfo;
use jka_protocol::jk2::presentation;
use std::{collections::HashMap, sync::OnceLock};

const IT_WEAPON: i32 = 1;
const IT_POWERUP: i32 = 5;
const IT_HOLDABLE: i32 = 6;
const IT_TEAM: i32 = 8;
/// Presentation item indices (configstring 27 is mapped to 51 entries).
const PRESENTED_ITEMS: i32 = 64;

struct Jk2Item {
    /// Asset paths from JK2; type and tag in presentation IDs.
    info: ItemInfo,
    icon: String,
    pickup_sound: String,
}

fn jk2_items() -> &'static [Option<Jk2Item>] {
    static TABLE: OnceLock<Vec<Option<Jk2Item>>> = OnceLock::new();
    TABLE.get_or_init(|| {
        (0..PRESENTED_ITEMS)
            .map(|index| {
                let item = jka_movement::jk2::item(presentation::wire_item(index)?)?;
                let mut info = item.info;
                info.tag = match info.item_type {
                    IT_WEAPON => presentation::presented_weapon(info.tag)?,
                    IT_POWERUP | IT_TEAM => presentation::presented_powerup(info.tag)?,
                    IT_HOLDABLE => presentation::presented_holdable(info.tag)?,
                    _ => info.tag,
                };
                Some(Jk2Item { info, icon: item.icon, pickup_sound: item.pickup_sound })
            })
            .collect()
    })
}

fn jk2_item(index: i32) -> Option<&'static Jk2Item> {
    jk2_items().get(usize::try_from(index).ok()?)?.as_ref()
}

fn jk2() -> bool {
    jka_assets::pk3::active_jk2().is_some()
}

/// `bg_itemlist[index]` for a presentation item index.
pub fn item(index: i32) -> Option<ItemInfo> {
    if jk2() {
        jk2_item(index).map(|item| item.info.clone())
    } else {
        jka_movement::bg_item(index)
    }
}

/// `bg_itemlist[index].icon`.
pub fn item_icon(index: i32) -> Option<String> {
    if jk2() {
        jk2_item(index).map(|item| item.icon.clone()).filter(|icon| !icon.is_empty())
    } else {
        jka_movement::bg_item_icon(index)
    }
}

/// `bg_itemlist[index].pickup_sound`.
pub fn item_pickup_sound(index: i32) -> Option<String> {
    if jk2() {
        jk2_item(index).map(|item| item.pickup_sound.clone()).filter(|sound| !sound.is_empty())
    } else {
        jka_movement::bg_item_pickup_sound(index)
    }
}

/// Presentation index range of the active item list.
pub fn item_count() -> i32 {
    if jk2() { PRESENTED_ITEMS } else { jka_movement::bg_item_count() }
}

/// CG_InitG2Weapons: world_model[0] of the last IT_WEAPON item per tag.
pub fn weapon_world_model(weapon: i32) -> Option<String> {
    static JKA: OnceLock<HashMap<i32, String>> = OnceLock::new();
    static JK2: OnceLock<HashMap<i32, String>> = OnceLock::new();
    let table = if jk2() { &JK2 } else { &JKA };
    table
        .get_or_init(|| {
            let mut table = HashMap::new();
            for index in 1..item_count() {
                if let Some(item) = item(index) {
                    if item.item_type == IT_WEAPON && !item.world_model.is_empty() {
                        table.insert(item.tag, item.world_model);
                    }
                }
            }
            table
        })
        .get(&weapon)
        .cloned()
}

/// The item icon for a presentation weapon / holdable tag (CG_Init's
/// BG_FindItemForWeapon / invenIcons), or `None` to keep the HUD's own name.
pub fn tagged_icon(item_type: i32, tag: i32) -> Option<String> {
    if !jk2() {
        return None;
    }
    jk2_items()
        .iter()
        .flatten()
        .filter(|item| item.info.item_type == item_type && item.info.tag == tag && !item.icon.is_empty())
        .map(|item| item.icon.clone())
        .next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jk2_items_keep_jk2_assets_under_presentation_ids() {
        let items = jk2_items();
        let bryar = items
            .iter()
            .flatten()
            .find(|item| item.info.item_type == IT_WEAPON && item.info.tag == 4)
            .expect("JK2 bryar under presentation WP_BRYAR_PISTOL");
        assert_eq!(bryar.info.world_model, "models/weapons2/briar_pistol/briar_pistol_w.glm");
        assert_eq!(bryar.info.view_model, "models/weapons2/briar_pistol/briar_pistol.md3");
        assert_eq!(bryar.icon, "gfx/hud/w_icon_rifle");
        let saber = items
            .iter()
            .flatten()
            .find(|item| item.info.item_type == IT_WEAPON && item.info.tag == 3)
            .unwrap();
        assert_eq!(saber.icon, "gfx/hud/w_icon_lightsaber");
        assert!(items
            .iter()
            .flatten()
            .any(|item| item.info.item_type == IT_HOLDABLE && item.info.tag == 5 && item.icon == "gfx/hud/i_icon_zoom"));
    }
}
