//! Opt-in fixture: validate every newly baked prop track against its actual
//! native mesh and composed variant. Source assets are never checked into Git.
use std::{collections::HashMap, path::Path};

#[test]
#[ignore = "requires DJK_PRESENTATION_DIR and DJK_FN_ASSET_BASE"]
fn all_baked_prop_tracks_match_native_meshes_and_variant_timelines() {
    let directory = std::env::var("DJK_PRESENTATION_DIR").unwrap();
    let base = std::env::var("DJK_FN_ASSET_BASE").unwrap();
    let mut assets = jka_assets::pk3::AssetSearchPath::open(Path::new(&base)).unwrap();
    let mut meshes = HashMap::new();
    let mut tracks = 0;
    let mut skins = 0;
    let mut bundles = 0;
    let mut contacts = 0;
    for file in std::fs::read_dir(directory).unwrap().filter_map(Result::ok) {
        if file.path().extension().is_none_or(|e| e != "djk_fnemote") {
            continue;
        }
        let bundle = jka_emotes::Bundle::open_index(&file.path()).unwrap();
        for entry in jka_emotes::manifest::parse_manifest(&bundle.manifest).unwrap() {
            for variant in std::iter::once(&entry).chain(entry.variants.iter()) {
                if let Some(track) = &variant.contact_track {
                    let clip =
                        jka_emotes::PoseClip::parse(&bundle.read(track, 16 * 1024 * 1024).unwrap())
                            .unwrap();
                    assert_eq!(
                        (clip.frames, clip.bones),
                        ((variant.end_frame - variant.first_frame) as usize, 1),
                        "contact timeline {} {}",
                        file.path().display(),
                        variant.variant_name
                    );
                    assert!(
                        clip.matrices().iter().all(|m| m[2][3] >= -0.001),
                        "contact height {}",
                        file.path().display()
                    );
                    contacts += 1;
                }
                for prop in &variant.props {
                    let clip = jka_emotes::PoseClip::parse(
                        &bundle.read(&prop.track, 16 * 1024 * 1024).unwrap(),
                    )
                    .unwrap();
                    assert_eq!(
                        (clip.frames, clip.bones),
                        ((variant.end_frame - variant.first_frame) as usize, 1),
                        "{} {}",
                        file.path().display(),
                        prop.name
                    );
                    // Authored seconds can extend by half a frame beyond the
                    // nearest-frame resampled duration; never accept a larger drift.
                    for span in &prop.intervals {
                        assert!(
                            span[1] <= clip.frames as f32 / variant.fps + 0.5 / variant.fps + 0.001,
                            "{} {}: notify outside composed timeline",
                            file.path().display(),
                            prop.name
                        );
                    }
                    if prop.model.ends_with(".djk_prop") {
                        if !meshes.contains_key(&prop.model) {
                            let bytes = assets
                                .read(&prop.model, 32 * 1024 * 1024)
                                .unwrap()
                                .unwrap()
                                .bytes;
                            meshes.insert(
                                prop.model.clone(),
                                jka_emotes::prop_mesh::Mesh::parse(&bytes).unwrap(),
                            );
                        }
                        if let Some(track) = &prop.skin_track {
                            let skin = jka_emotes::PoseClip::parse(
                                &bundle.read(track, 32 * 1024 * 1024).unwrap(),
                            )
                            .unwrap();
                            assert_eq!(
                                (skin.frames, skin.bones),
                                (clip.frames, meshes[&prop.model].bones.len()),
                                "{} {}",
                                file.path().display(),
                                prop.name
                            );
                            skins += 1;
                        }
                    } else {
                        assert!(assets
                            .read(&prop.model, 32 * 1024 * 1024)
                            .unwrap()
                            .is_some());
                    }
                    tracks += 1;
                }
            }
        }
        bundles += 1;
    }
    println!("Validated {bundles} bundles, {tracks} attachment tracks, {skins} independent prop skin tracks and {contacts} contact curves");
    assert!(tracks > 0 && skins > 0 && contacts > 0);
}

#[test]
#[ignore = "requires DJK_PRESENTATION_DIR real Black Beatles bundle"]
fn black_beatles_moves_after_its_authored_freeze_intro() {
    let directory = std::env::var("DJK_PRESENTATION_DIR").unwrap();
    let bundle =
        jka_emotes::Bundle::open(&Path::new(&directory).join("EID_FreezeFrame.djk_fnemote"))
            .unwrap();
    let entry = jka_emotes::manifest::parse_manifest(&bundle.manifest)
        .unwrap()
        .remove(0);
    let clip = bundle.pose.as_ref().unwrap();
    let sample = |ms| {
        clip.sample_range(
            ms,
            entry.fps,
            entry.first_frame as usize,
            entry.end_frame as usize,
            entry.loop_start_frame.map(|v| v as usize),
        )
    };
    let a = sample(16000);
    let b = sample(18000);
    let max = a
        .iter()
        .zip(b)
        .flat_map(|(a, b)| {
            a.iter()
                .flatten()
                .zip(b.iter().flatten())
                .map(|(a, b)| (a - b).abs())
                .collect::<Vec<_>>()
        })
        .fold(0.0f32, f32::max);
    assert!(
        max > 0.1,
        "dance stayed static after the authored intro: {max}"
    );
    println!("Black Beatles native pose changes after the 13.6-second source hold: max matrix delta {max}");
}
