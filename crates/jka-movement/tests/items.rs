use jka_movement::{bg_item, bg_item_count, byte_to_dir, rotate_around_direction};

#[test]
fn bg_itemlist_is_the_pinned_openjk_table() {
    assert!(bg_item_count() > 40);
    // Index 0 is the NULL item; ET_ITEM modelindex 0 draws nothing.
    assert_eq!(bg_item(0).unwrap().classname, "");
    assert!(bg_item(-1).is_none() && bg_item(bg_item_count()).is_none());
    let blaster = (1..bg_item_count())
        .filter_map(bg_item)
        .find(|item| item.classname == "weapon_blaster")
        .expect("weapon_blaster");
    assert_eq!((blaster.item_type, blaster.tag), (1, 5)); // IT_WEAPON, WP_BLASTER
    let trip = (1..bg_item_count()).filter_map(bg_item).find(|item| item.classname == "weapon_trip_mine").unwrap();
    assert_eq!(trip.world_model2, "models/weapons2/laser_trap/laser_trap_pu.md3");
    for index in [1, 3, 15, 16, 28, 36, 42, 43, 44] {
        println!("ITEM {index}: {:?}", bg_item(index).unwrap());
    }
}

#[test]
fn q_math_event_helpers_match_openjk() {
    // bytedirs[0] and an out-of-range byte (vec3_origin).
    assert_eq!(byte_to_dir(0), [-0.525731, 0.0, 0.850651]);
    assert_eq!(byte_to_dir(1000), [0.0; 3]);
    let axis = rotate_around_direction([1.0, 0.0, 0.0], 90.0);
    for row in axis {
        let len: f32 = row.iter().map(|v| v * v).sum();
        assert!((len - 1.0).abs() < 1e-5);
    }
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    assert!(dot(axis[0], axis[1]).abs() < 1e-5 && dot(axis[0], axis[2]).abs() < 1e-5);
}
