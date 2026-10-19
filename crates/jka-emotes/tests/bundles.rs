use glam::{Quat, Vec3};
use jka_emotes::{manifest, pose::PoseClip, retarget, Bundle};
use serde_json::json;
use std::{collections::BTreeMap, path::PathBuf};

#[test]
fn variant_sampling_keeps_neighbors_out_and_loops_after_the_intro() {
    let matrices: Vec<_> = (0..5)
        .map(|i| {
            [
                [1.0, 0.0, 0.0, i as f32],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ]
        })
        .collect();
    let clip = PoseClip::parse(&PoseClip::encode(5, 1, &matrices).unwrap()).unwrap();
    assert_eq!(clip.sample_range(0, 10.0, 1, 3, None)[0][1][3], 1.0);
    assert_eq!(clip.sample_range(150, 10.0, 1, 3, None)[0][1][3], 2.0);
    assert_eq!(clip.sample_range(300, 10.0, 1, 4, Some(2))[0][1][3], 2.0);
    assert!((clip.sample_range(250, 10.0, 1, 4, Some(2))[0][1][3] - 2.5).abs() < 0.001);
}

fn temp(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "djk-emote-{name}-{}-{}.djk_fnemote",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}
fn gla() -> Vec<u8> {
    retarget::encode(
        &jka_assets::ghoul2::openjk_default_gla().unwrap(),
        2,
        &[(Quat::IDENTITY, Vec3::ZERO), (Quat::IDENTITY, Vec3::X)],
    )
    .unwrap()
}
fn wav() -> Vec<u8> {
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&38u32.to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&8000u32.to_le_bytes());
    bytes.extend_from_slice(&16000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&0i16.to_le_bytes());
    bytes
}
fn root() -> serde_json::Value {
    json!({"format":"djk_fnemote","version":1,"emotes":[{"id":"test","name":"Test","gla":"animation.gla","first_frame":0,"end_frame":2,"fps":30,"audio":[{"at_ms":0,"lane":"music","audience":"both","wav":"audio/a.wav"}]}]})
}
fn members() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([
        ("animation.gla".into(), gla()),
        ("audio/a.wav".into(), wav()),
    ])
}

#[test]
fn bundle_round_trip_is_self_contained_and_never_overwrites() {
    let path = temp("roundtrip");
    jka_emotes::write_bundle(&path, &root(), &members()).unwrap();
    let bundle = Bundle::open(&path).unwrap();
    assert_eq!(bundle.read("audio/a.wav", 1024).unwrap(), wav());
    assert!(bundle.read("audio/a.wav", 8).is_err());
    assert!(jka_emotes::write_bundle(&path, &root(), &members()).is_err());
    std::fs::remove_file(path).unwrap();
}
#[test]
fn catalogue_defers_payload_validation_until_playback() {
    use std::io::Write;
    let path = temp("index");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    for (name, bytes) in [
        ("manifest.json", serde_json::to_vec(&root()).unwrap()),
        ("animation.gla", gla()),
        ("audio/a.wav", vec![0; 44]),
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&bytes).unwrap();
    }
    zip.finish().unwrap();
    let index = Bundle::open_index(&path).unwrap();
    assert!(index.pose.is_none());
    assert!(index.validate_for_playback().is_err());
    std::fs::remove_file(path).unwrap();
}
#[test]
fn missing_audio_and_wrong_versions_never_publish_a_bundle() {
    let path = temp("invalid");
    let mut m = members();
    m.remove("audio/a.wav");
    assert!(jka_emotes::write_bundle(&path, &root(), &m).is_err());
    assert!(!path.exists());
    let mut r = root();
    r["version"] = json!(2);
    assert!(jka_emotes::write_bundle(&path, &r, &members()).is_err());
    assert!(!path.exists());
}
#[test]
fn unsafe_paths_and_frame_ranges_are_rejected() {
    for path in ["../x", "/x", "C:/x", "x//y", "x/./y", "x/../y", "x\0y"] {
        assert!(manifest::qpath(path).is_err());
    }
    let mut r = root();
    r["emotes"][0]["end_frame"] = json!(0);
    assert!(manifest::parse_manifest(&r).is_err());
    r = root();
    r["emotes"][0]["audio"][0]["at_ms"] = json!(9999);
    assert!(manifest::parse_manifest(&r).is_err());
    let path = temp("frame-range");
    r = root();
    r["emotes"][0]["end_frame"] = json!(3);
    assert!(jka_emotes::write_bundle(&path, &r, &members()).is_err());
    assert!(!path.exists());
}
#[test]
fn duplicate_zip_names_and_traversal_are_rejected() {
    use std::io::Write;
    for name in ["../escape", "ANIMATION.GLA"] {
        let path = temp("zip-names");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        for (n, b) in [
            ("manifest.json", serde_json::to_vec(&root()).unwrap()),
            ("animation.gla", gla()),
            ("audio/a.wav", wav()),
            (name, vec![0]),
        ] {
            zip.start_file(n, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(&b).unwrap();
        }
        zip.finish().unwrap();
        assert!(Bundle::open(&path).is_err());
        std::fs::remove_file(path).unwrap();
    }
}
#[test]
fn wave_validation_checks_bounds_and_sample_alignment() {
    jka_emotes::validate_wav(&wav()).unwrap();
    let mut w = wav();
    w.pop();
    assert!(jka_emotes::validate_wav(&w).is_err());
    let mut w = wav();
    w[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(jka_emotes::validate_wav(&w).is_err());
}
#[test]
fn pose_sampling_preserves_scale_wraps_and_applies_root_once() {
    let a = [
        [2.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
    ];
    let mut b = a;
    b[0][3] = 10.0;
    let bytes = PoseClip::encode(2, 1, &[a, b]).unwrap();
    let clip = PoseClip::parse(&bytes).unwrap();
    let midpoint = clip.sample(50, 10.0, false)[0];
    assert_eq!(midpoint[1][0], 2.0);
    assert!((midpoint[1][3] - 5.0).abs() < 0.001);
    assert_eq!(clip.sample(200, 10.0, true), clip.sample(0, 10.0, true));
    assert_eq!(clip.sample(999, 10.0, false), clip.sample(100, 10.0, false));
    let mut bad = bytes;
    bad[16..20].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(PoseClip::parse(&bad).is_err());
}

/// A non-file backend: the seam a streaming store would implement.
#[derive(Debug)]
struct MemorySource {
    bytes: std::sync::Arc<Vec<u8>>,
    opens: std::sync::atomic::AtomicUsize,
}
impl jka_emotes::BundleSource for MemorySource {
    fn len(&self) -> u64 {
        self.bytes.len() as u64
    }
    fn open(&self) -> std::io::Result<Box<dyn jka_emotes::source::ReadSeek>> {
        self.opens
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(Box::new(std::io::Cursor::new((*self.bytes).clone())))
    }
    fn key(&self) -> u64 {
        0xfeed
    }
    fn describe(&self) -> String {
        "memory".into()
    }
}

#[test]
fn bundles_open_from_any_source_not_only_files() {
    let path = temp("source");
    jka_emotes::write_bundle(&path, &root(), &members()).unwrap();
    let source = std::sync::Arc::new(MemorySource {
        bytes: std::sync::Arc::new(std::fs::read(&path).unwrap()),
        opens: Default::default(),
    });
    std::fs::remove_file(path).unwrap();

    // Catalogue mode reads the directory and manifest once, nothing else.
    let index = Bundle::from_source(source.clone(), false).unwrap();
    assert_eq!(source.opens.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(index.manifest["emotes"][0]["id"], "test");
    assert!(index.pose.is_none());

    // Playback re-opens, validates the assets and keeps the same cache identity.
    let playable = index.validate_for_playback().unwrap();
    assert_eq!(playable.runtime_prefix(), index.runtime_prefix());
    assert_eq!(playable.read("audio/a.wav", 1024).unwrap(), wav());
}
