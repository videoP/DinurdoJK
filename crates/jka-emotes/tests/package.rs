use glam::{Quat, Vec3};
use serde_json::json;
use std::{collections::BTreeMap, fs, io::Write};

#[test]
fn packed_library_seeks_nested_members_and_keeps_payloads_identical() {
    let root = std::env::temp_dir().join(format!("djk-pk3-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let input = root.join("emotes");
    fs::create_dir_all(&input).unwrap();
    let gla = jka_emotes::retarget::encode(
        &jka_assets::ghoul2::openjk_default_gla().unwrap(),
        1,
        &[(Quat::IDENTITY, Vec3::ZERO)],
    )
    .unwrap();
    let manifest = json!({"format":"djk_fnemote","version":1,"emotes":[{"id":"test","name":"Test","gla":"a.gla","first_frame":0,"end_frame":1,"fps":30}]});
    let bundle = input.join("EID_Test.djk_fnemote");
    jka_emotes::write_bundle(
        &bundle,
        &manifest,
        &BTreeMap::from([("a.gla".into(), gla.clone())]),
    )
    .unwrap();
    fs::write(input.join("favorites.json"), b"personal").unwrap();
    let package = root.join("library.pk3");
    assert_eq!(
        jka_emotes::package::library(&input, None, &package).unwrap(),
        1
    );
    let index =
        jka_emotes::Bundle::open_packed_index(&package, "emotes/eid_test.djk_fnemote").unwrap();
    assert_eq!(index.read("a.gla", gla.len()).unwrap(), gla);
    assert!(index.read("a.gla", 1).is_err());
    index.validate_for_playback().unwrap();
    let mut vfs = jka_assets::pk3::AssetSearchPath::open(&root).unwrap();
    let (path, range) = vfs
        .stored_location("emotes/eid_test.djk_fnemote")
        .unwrap()
        .unwrap();
    let located =
        jka_emotes::Bundle::open_located_index(&path, "emotes/eid_test.djk_fnemote", range)
            .unwrap();
    assert_eq!(index.runtime_prefix(), located.runtime_prefix());
    let zip = zip::ZipArchive::new(fs::File::open(&package).unwrap()).unwrap();
    assert!(!zip.file_names().any(|n| n.ends_with("favorites.json")));
    assert!(jka_emotes::package::library(&input, None, &package).is_err());
    drop(vfs);
    drop(zip);
    fs::remove_file(bundle).unwrap();
    fs::remove_file(input.join("favorites.json")).unwrap();
    fs::remove_dir(input).unwrap();
    fs::remove_file(package).unwrap();
    fs::remove_dir(root).unwrap();
}

#[test]
fn outer_deflate_and_truncated_ranges_are_rejected() {
    let path = std::env::temp_dir().join(format!("djk-pk3-invalid-{}.pk3", std::process::id()));
    let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    zip.start_file(
        "emotes/test.djk_fnemote",
        zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated),
    )
    .unwrap();
    zip.write_all(b"not a bundle").unwrap();
    zip.finish().unwrap();
    assert!(jka_emotes::Bundle::open_packed_index(&path, "emotes/test.djk_fnemote").is_err());
    assert!(jka_emotes::Bundle::open_located_index(
        &path,
        "emotes/test.djk_fnemote",
        Some((u64::MAX, 50))
    )
    .is_err());
    fs::remove_file(path).unwrap();
}
