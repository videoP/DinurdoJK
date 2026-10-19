//! The recipe compiler end to end, without any licensed assets: a pre-baked
//! GLA plus one WAV in, a validated bundle out.
use glam::{Quat, Vec3};
use jka_emotes::{compiler::compile, retarget, Bundle};
use serde_json::{json, Value};
use std::{fs, path::PathBuf};

struct Scratch(PathBuf);
impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("djk-compile-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("export/Audio")).unwrap();
        Self(dir)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn silent_wav() -> Vec<u8> {
    let samples = 800u32; // 0.1 s at 8 kHz
    let mut wav = b"RIFF".to_vec();
    wav.extend((36 + samples * 2).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes()); // PCM
    wav.extend(1u16.to_le_bytes()); // mono
    wav.extend(8000u32.to_le_bytes());
    wav.extend(16000u32.to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend((samples * 2).to_le_bytes());
    wav.extend(vec![0u8; samples as usize * 2]);
    wav
}

fn write_recipe(scratch: &Scratch, audio: Value) -> PathBuf {
    let gla = retarget::encode(
        &jka_assets::ghoul2::openjk_default_gla().unwrap(),
        3,
        &[(Quat::IDENTITY, Vec3::ZERO), (Quat::IDENTITY, Vec3::X), (Quat::IDENTITY, Vec3::Y)],
    )
    .unwrap();
    fs::write(scratch.0.join("export/dance.gla"), gla).unwrap();
    fs::write(scratch.0.join("export/Audio/song.wav"), silent_wav()).unwrap();
    let recipe = json!({
        "id": "EID_Test", "name": "Test Dance", "source_root": "export",
        "animation": {"gla": "dance.gla", "fps": 30, "first_frame": 0, "end_frame": 3},
        "loop": true, "audio": audio,
    });
    let path = scratch.0.join("test.recipe.json");
    fs::write(&path, serde_json::to_vec_pretty(&recipe).unwrap()).unwrap();
    path
}

#[test]
fn compiles_a_prebaked_animation_and_audio_into_a_valid_bundle() {
    let scratch = Scratch::new("ok");
    let recipe = write_recipe(&scratch, json!([{"at_ms": 0, "lane": "music", "audience": "both", "wav": "Audio/song.wav"}]));
    let out = scratch.0.join("EID_Test.djk_fnemote");
    compile(&recipe, &out).unwrap();

    let bundle = Bundle::open(&out).unwrap();
    let emote = &bundle.manifest["emotes"][0];
    assert_eq!(emote["name"], "Test Dance");
    assert_eq!(emote["loop"], true);
    assert_eq!(emote["end_frame"], 3);
    // The WAV was copied under a stable member name and is playable from the bundle.
    let wav = emote["audio"][0]["wav"].as_str().unwrap();
    assert!(wav.starts_with("audio/"), "{wav}");
    assert_eq!(bundle.read(wav, 1 << 20).unwrap(), silent_wav());
    // The recipe travels with the bundle for auditing.
    assert!(bundle.read("metadata/recipe.json", 1 << 20).is_ok());
}

#[test]
fn never_overwrites_and_never_reads_outside_the_source_root() {
    let scratch = Scratch::new("reject");
    let out = scratch.0.join("EID_Test.djk_fnemote");

    let recipe = write_recipe(&scratch, json!([{"at_ms": 0, "lane": "music", "audience": "both", "wav": "../escape.wav"}]));
    fs::write(scratch.0.join("escape.wav"), silent_wav()).unwrap();
    assert!(compile(&recipe, &out).is_err(), "a path leaving source_root must be refused");
    assert!(!out.exists(), "a failed compile must not leave a bundle behind");

    let recipe = write_recipe(&scratch, json!([]));
    compile(&recipe, &out).unwrap();
    assert!(compile(&recipe, &out).unwrap_err().contains("already exists"));

    let wrong_extension = scratch.0.join("EID_Test.zip");
    assert!(compile(&recipe, &wrong_extension).is_err());
}
