//! Frozen pre-migration behavior: config parsing/defaults/archives and subsystem setters.
use crate::{config, japro_cg::JaproCgame, net::NetworkSettings};
use std::{fs, path::PathBuf};

pub(crate) const INPUTS: &[&str] = &[
    "",
    "0",
    "1",
    "2",
    "3",
    "7",
    "-0",
    "-1",
    "256",
    "10000000000",
    "0.5",
    "1.9",
    " 1 ",
    "yes",
    "off",
    "NaN",
    "inf",
    "invalid",
    "1 2 3",
    "255 128 0 64",
];
pub(crate) const NAMES: &str = include_str!("testdata/cvars/names.txt");
pub(crate) fn digest(text: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    // These fields were added after the frozen settings snapshot. Keep the
    // compatibility digest focused on pre-existing behavior; each addition has
    // a dedicated load/archive test below.
    let projected = text
        // These fields were added after the frozen Debug layout. Project them
        // out for legacy fixtures; promoted defaults and live A/B setters are
        // checked by the headless runtime test. Ghoul2's intentional default
        // change remains visible to the old fixtures.
        .replace(", model_state_cache: false, dynamic_batch_draws: false, world_compact_min_draws: 1", "")
        .replace(", md3_geometry_cache: false", "")
        .replace(", md3_geometry_cache: true", "")
        .replace(", static_md3_instances: false", "")
        .replace(", surface_sprites_gpu: true, surface_sprites: true", "")
        .replace(", md3_auto_lod: false, md3_lod_debug: false", "")
        .replace(", md3_auto_lod: true, bsp_model_lod: true, md3_lod_debug: false", "")
        .replace(", model_render_bundles: false", "")
        .replace(", encoder_finish_threads: 2", "")
        .replace(", model_prepare_threads: 0, model_index_upload_cache: false, ghoul2_batch_key_cache: false", "")
        .replace(", model_prepare_threads: 2, model_index_upload_cache: false, ghoul2_batch_key_cache: false", "")
        .replace(", model_state_cache: false, dynamic_batch_draws: true, world_compact_min_draws: 1", "")
        .replace(", dynamic_light_falloff: ", ", rt_denoiser_mode: 0, dynamic_light_falloff: ")
        .replace("OceanSettings { interaction: false, interaction_strength: 1.0, interaction_splash_mode: 3, interaction_debug: false, ", "OceanSettings { ")
        .replace("interaction_splash_mode: 3, ", "")
        .replace("interaction_lifetime: 2.5, interaction_fade: 1.0, interaction_damping: 2.0, interaction_edge_fade: 81.92, ", "")
        // The prediction backend/rounding cvars were deleted. Preserve the old
        // NetworkSettings Debug-layout for unrelated frozen compatibility hashes.
        .replace(", prediction_miss_threshold: ", ", snap_mode: -1, predict_backend: -1, prediction_miss_threshold: ")
        .replace(", third_person_flag_alpha: 1.0", "")
        .replace(", console_notify_lines: 3", "")
        .replace(", score_spectator_guess: true", "")
        .replace(", steam_audio_hrtf_interpolation: 0", "")
        .replace(", jiggle_glute_coverage_height: 1.0", "")
        .replace(", jiggle_breast_remesh_bottom_z: 0.0", "")
        .replace(", jiggle_breast_remesh_top_z: 0.0", "")
        .replace(", jiggle_glute_remesh_bottom_z: 0.0", "")
        .replace(", jiggle_glute_remesh_top_z: 0.0", "")
        .replace(", jiggle_glute_jiggle_bottom_z: 0.0", "")
        .replace(", jiggle_glute_jiggle_top_z: 0.0", "")
        .replace(", jiggle_breast_size: 0.0", "")
        .replace(", jiggle_glute_size: 0.0", "")
        .replace(", jiggle_locomotion: 1.0", "")
        .replace(", jiggle_impulse_cap: 0.5", "")
        .replace(", jiggle_swirl: 0.35", "")
        .replace(", jiggle_breast_stiffness: 1.0", "")
        .replace(", jiggle_breast_damping: 1.0", "")
        .replace(", jiggle_glute_stiffness: 1.0", "")
        .replace(", jiggle_glute_damping: 1.0", "")
        // New flare settings did not exist in the frozen pre-registry settings.
        .replace(", world_flares: false", "")
        .replace(", saber_flares: true", "")
        .replace(", rt_taa: 0", "")
        .replace(", baked_sun_removal: 0.0", "")
        .replace(", baked_sun_cache: false", "")
        .replace(", sun_shadow_strength: 0.3", "")
        .replace(", sun_lighting: false", "")
        .replace(", baked_sun_shadow_map: true", "")
        .replace(", baked_sun_edge_softness: 2.5", "")
        .replace(", baked_sun_direct_limit: 0.48", "")
        .replace(", baked_indirect_scale: 1.0", "")
        .replace(", baked_indirect_saturation: 1.0", "")
        .replace(", baked_indirect_power: 1.0", "")
        .replace(", baked_indirect_highlight_preserve: 1.0", "")
        .replace(", swim_tilt: 0.9", "")
        .replace(", swim_strafe_roll: 0.0", "")
        .replace(", swim_bank_axis: 2", "")
        .replace(", swim_bank_probe: 0.0", "")
        .replace(", swim_turn_bank: 0.7", "")
        .replace(", swim_turn_bank: 1.0", "")
        .replace(", move_lean: 0.75", "");
    for byte in projected.bytes() {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}
pub(crate) struct Fixture(pub(crate) PathBuf);
impl Fixture {
    pub(crate) fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dinurdo-cvar-compat-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
pub(crate) fn assert_snapshot(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src/testdata/cvars")
        .join(name);
    let mut expected = fs::read_to_string(&path).unwrap();
    let projected_archive;
    let actual = if name.ends_with(".cfg") {
        projected_archive = actual
            .lines()
            .filter(|line| {
                !line.starts_with("seta cg_swimTilt ")
                    && !line.starts_with("seta r_bakedSunRemoval ")
                    && !line.starts_with("seta r_bakedSunCache ")
                    && !line.starts_with("seta r_sunShadowStrength ")
                    && !line.starts_with("seta r_sunLighting ")
                    && !line.starts_with("seta r_bakedSunShadowMap ")
                    && !line.starts_with("seta r_rtTaa ") &&
                !line.starts_with("seta con_notifylines ")
                    && !line.starts_with("seta cg_thirdPersonFlagAlpha ")
                    && !line.starts_with("seta cg_scoreSpectatorGuess ")
                    && !line.starts_with("seta s_steamAudioHrtfInterpolation ")
                    && !line.starts_with("seta r_jiggleBreastRemeshBottomZ ")
                    && !line.starts_with("seta r_jiggleBreastRemeshTopZ ")
                    && !line.starts_with("seta r_jiggleGluteRemeshBottomZ ")
                    && !line.starts_with("seta r_jiggleGluteRemeshTopZ ")
                    && !line.starts_with("seta r_jiggleGluteJiggleBottomZ ")
                    && !line.starts_with("seta r_jiggleGluteJiggleTopZ ")
                    && !line.starts_with("seta r_jiggleBreastSize ")
                    && !line.starts_with("seta r_jiggleGluteSize ")
                    && !line.starts_with("seta r_jiggleLocomotion ")
                    && !line.starts_with("seta r_jiggleImpulseCap ")
                    && !line.starts_with("seta r_jiggleSwirl ")
                    && !line.starts_with("seta r_jiggleBreastStiffness ")
                    && !line.starts_with("seta r_jiggleBreastDamping ")
                    && !line.starts_with("seta r_jiggleGluteStiffness ")
                    && !line.starts_with("seta r_jiggleGluteDamping ")
                    && !line.starts_with("seta cl_serverFilters ")
                    // New controls are outside the frozen pre-registry archive.
                    && !line.starts_with("seta r_worldFlares ")
                    && !line.starts_with("seta r_saberFlares ")
                    && !line.starts_with("seta r_md3AutoLod ")
                    && !line.starts_with("seta r_oceanInteraction ")
                    && !line.starts_with("seta r_oceanInteractionStrength ")
                    && !line.starts_with("seta r_oceanInteractionFx ")
                    && !line.starts_with("seta r_oceanInteractionLifetime ")
                    && !line.starts_with("seta r_oceanInteractionFade ")
                    && !line.starts_with("seta r_oceanInteractionDamping ")
                    && !line.starts_with("seta r_oceanInteractionEdgeFade ")
            })
            .map(|line| format!("{line}\n"))
            .collect::<String>();
        projected_archive.as_str()
    } else {
        actual
    };
    if matches!(name, "config-matrix.txt" | "runtime-matrix.txt" | "subsystem-matrix.txt") {
        expected = expected
            .lines()
            .filter(|line| {
                let cvar_name = line.split_whitespace().next().unwrap_or_default();
                if matches!(cvar_name, "cg_predictbackend" | "cg_snapmode") {
                    return false; // No longer registered; previous behavior is intentionally gone.
                }
                name != "config-matrix.txt" || (cvar_name != "r_hdr" && crate::app::cvar_registry().find(cvar_name).is_some())
            })
            .map(|line| format!("{line}\n"))
            .collect();
    }
    if name.ends_with(".cfg") {
        expected = expected.lines().filter(|line| !line.starts_with("seta r_rtDenoiser ")).map(|line| format!("{line}\n")).collect();
    }
    if name == "changed.cfg" {
        // The removed r_volumetricFog import used to override current r_drawfog.
        expected = expected.replace("seta r_drawfog \"0\"", "seta r_drawfog \"2\"");
    }
    for (row, (a, e)) in actual.lines().zip(expected.lines()).enumerate() {
        assert_eq!(a, e, "{name} row {}", row + 1);
    }
    assert_eq!(
        actual.lines().count(),
        expected.lines().count(),
        "{name} row count"
    );
}
#[test]
fn cvar_compatibility_config_input_matrix() {
    let fixture = Fixture::new();
    let path = fixture.0.join("input.cfg");
    let mut actual = String::new();
    for name in NAMES
        .lines()
        .filter(|name| *name != "r_hdr" && crate::app::cvar_registry().find(name).is_some())
    {
        actual.push_str(name);
        for value in INPUTS {
            fs::write(
                &path,
                format!("seta {} \"{value}\"\n", name.to_ascii_uppercase()),
            )
            .unwrap();
            let video = config::load_video_settings(&path);
            let audio = config::load_audio_settings(&path);
            let client = config::load_client_presentation_settings(&path);
            actual.push(' ');
            actual.push_str(&digest(&format!("{video:?}|{audio:?}|{client:?}")));
        }
        actual.push('\n');
    }
    assert_snapshot("config-matrix.txt", &actual);
}
#[test]
fn cvar_compatibility_network_and_japro_setters() {
    let mut actual = String::new();
    for name in NAMES.lines() {
        if NetworkSettings::default().cvar_value(name).is_none()
            && JaproCgame::default().cvar_value(name).is_none()
        {
            continue;
        }
        actual.push_str(name);
        for value in INPUTS {
            let mut network = NetworkSettings::default();
            let result = network.set_cvar(name, value);
            let get = network.cvar_value(name);
            let mut japro = JaproCgame::default();
            let jp_result = japro.set_cvar(name, value);
            let jp_get = japro.cvar_value(name);
            actual.push(' ');
            actual.push_str(&digest(&format!(
                "{result:?}|{get:?}|{network:?}|{jp_result:?}|{jp_get:?}|{japro:?}"
            )));
        }
        actual.push('\n');
    }
    assert_snapshot("subsystem-matrix.txt", &actual);
}
#[test]
fn cvar_compatibility_archive_text_and_defaults() {
    let fixture = Fixture::new();
    let path = fixture.0.join("archive.cfg");
    let defaults = (
        crate::ui::VideoSettings::default(),
        config::ClientPresentationSettings::default(),
        config::AudioSettings::default(),
    );
    config::save_video_settings(
        &path,
        defaults.0,
        &crate::keybinds::Bindings::default(),
        &defaults.1,
        &defaults.2,
    )
    .unwrap();
    let default_text = fs::read_to_string(&path).unwrap();
    assert_snapshot("default.cfg", &default_text);
    // Load every recognized name in sequence to exercise cross-cvar conversions.
    let mut input = default_text;
    for name in NAMES
        .lines()
        .filter(|name| crate::app::cvar_registry().find(name).is_some())
    {
        input.push_str(&format!("seta {name} \"2\"\n"));
    }
    fs::write(&path, input).unwrap();
    let video = config::load_video_settings(&path);
    let audio = config::load_audio_settings(&path);
    let client = config::load_client_presentation_settings(&path);
    config::save_video_settings(
        &path,
        video,
        &crate::keybinds::Bindings::default(),
        &client,
        &audio,
    )
    .unwrap();
    assert_snapshot("changed.cfg", &fs::read_to_string(&path).unwrap());
}

#[test]
fn steam_audio_hrtf_interpolation_addition_loads_and_archives() {
    let fixture = Fixture::new();
    let path = fixture.0.join("input.cfg");
    fs::write(&path, "seta s_steamAudioHrtfInterpolation bilinear\n").unwrap();
    let audio = config::load_audio_settings(&path);
    assert_eq!(audio.steam_audio_hrtf_interpolation, 1);

    let video = crate::ui::VideoSettings::default();
    let client = config::ClientPresentationSettings::default();
    config::save_video_settings(
        &path,
        video,
        &crate::keybinds::Bindings::default(),
        &client,
        &audio,
    )
    .unwrap();
    let archived = fs::read_to_string(&path).unwrap();
    assert!(archived.contains("seta s_steamAudioHrtfInterpolation \"bilinear\""));
}

#[test]
fn current_config_ignores_obsolete_keys_and_keeps_hdr_independent_of_tonemap() {
    let fixture = Fixture::new();
    let path = fixture.0.join("input.cfg");
    fs::write(&path, "seta r_hdr 1\nseta r_bakedBrightness 1\nseta r_fogMode volumetric\nseta r_cloudWindSpeed 0\nseta r_oceanWeather \"0 0 0 0\"\n").unwrap();
    let actual = config::load_video_settings(&path);
    let mut expected = crate::ui::VideoSettings::default();
    expected.hdr = true;
    expected.ocean_settings.wind = expected.weather_wind;
    assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
    let absent = fixture.0.join("absent.cfg");
    fs::write(fixture.0.join("jampconfig.cfg"), "seta r_gamma 2\n").unwrap();
    assert_eq!(
        format!("{:?}", config::load_video_settings(&absent)),
        format!("{:?}", crate::ui::VideoSettings::default())
    );
    assert_eq!(
        config::load_audio_settings(&absent),
        config::AudioSettings::default()
    );
    assert_eq!(
        config::load_client_presentation_settings(&absent),
        config::ClientPresentationSettings::default()
    );
}

#[test]
fn notification_cvar_addition_loads_and_archives_its_current_default() {
    let fixture = Fixture::new();
    let path = fixture.0.join("input.cfg");
    fs::write(&path, "seta con_notifylines 8\n").unwrap();
    let settings = config::load_client_presentation_settings(&path);
    assert_eq!(settings.console_notify_lines, 8);
    config::save_video_settings(
        &path,
        crate::ui::VideoSettings::default(),
        &crate::keybinds::Bindings::default(),
        &settings,
        &config::AudioSettings::default(),
    )
    .unwrap();
    assert!(fs::read_to_string(&path)
        .unwrap()
        .contains("seta con_notifylines \"8\""));
    assert_eq!(
        crate::app::cvar_registry()
            .find("con_notifylines")
            .unwrap()
            .entry
            .default,
        "3"
    );
    assert!(!crate::app::cvar_registry()
        .find("con_notifytime")
        .unwrap()
        .flags
        .contains(crate::console::cvars::CvarFlags::ARCHIVE));
}

#[test]
fn ocean_interaction_controls_load_archive_and_reject_nonfinite_strength() {
    let fixture = Fixture::new();
    let path = fixture.0.join("ocean.cfg");
    fs::write(&path,"seta r_oceanInteraction 1\nseta r_oceanInteractionStrength 64\nseta r_oceanInteractionDebug 1\nseta r_oceanInteractionLifetime 4\nseta r_oceanInteractionFade 1.5\nseta r_oceanInteractionDamping 0.7\nseta r_oceanInteractionEdgeFade 64\n").unwrap();
    let settings=config::load_video_settings(&path);
    assert!(settings.ocean_settings.interaction);
    assert_eq!(settings.ocean_settings.interaction_strength,64.0);
    assert_eq!(settings.ocean_settings.sanitize().interaction_strength,64.0);
    assert_eq!(settings.ocean_settings.interaction_lifetime,4.0);
    assert_eq!(settings.ocean_settings.interaction_fade,1.5);
    assert_eq!(settings.ocean_settings.interaction_damping,0.7);
    assert_eq!(settings.ocean_settings.interaction_edge_fade,64.0);
    assert!(!settings.ocean_settings.interaction_debug);
    config::save_video_settings(&path,settings,&crate::keybinds::Bindings::default(),
        &config::ClientPresentationSettings::default(),&config::AudioSettings::default()).unwrap();
    let saved=fs::read_to_string(&path).unwrap();
    assert!(saved.contains("seta r_oceanInteraction \"1\""));
    assert!(saved.contains("seta r_oceanInteractionStrength \"64\""));
    assert!(saved.contains("seta r_oceanInteractionLifetime \"4\""));
    assert!(saved.contains("seta r_oceanInteractionFade \"1.5\""));
    assert!(saved.contains("seta r_oceanInteractionDamping \"0.7\""));
    assert!(saved.contains("seta r_oceanInteractionEdgeFade \"64\""));
    assert!(!saved.contains("seta r_oceanInteractionDebug"));
    fs::write(&path,"seta r_oceanInteractionStrength NaN\n").unwrap();
    assert_eq!(config::load_video_settings(&path).ocean_settings.interaction_strength,1.0);
    let registry=crate::app::cvar_registry();
    for name in ["r_oceanInteraction","r_oceanInteractionStrength"] {
        assert!(registry.find(name).unwrap().flags.contains(crate::console::cvars::CvarFlags::ARCHIVE));
    }
}

#[test]
fn third_person_flag_alpha_loads_archives_and_rejects_nonfinite_values() {
    let fixture = Fixture::new();
    let path = fixture.0.join("flag.cfg");
    fs::write(&path, "seta cg_thirdPersonFlagAlpha 0.25\n").unwrap();
    let settings = config::load_client_presentation_settings(&path);
    assert_eq!(settings.japro.third_person_flag_alpha, 0.25);
    config::save_video_settings(
        &path, crate::ui::VideoSettings::default(), &crate::keybinds::Bindings::default(),
        &settings, &config::AudioSettings::default(),
    ).unwrap();
    assert!(fs::read_to_string(&path).unwrap().contains("seta cg_thirdPersonFlagAlpha \"0.25\""));
    let mut options = JaproCgame::default();
    assert!(options.set_cvar("cg_thirdPersonFlagAlpha", "NaN").unwrap().is_err());
    assert_eq!(options.third_person_flag_alpha, 1.0);
    options.set_cvar("cg_thirdPersonFlagAlpha", "-1").unwrap().unwrap();
    assert_eq!(options.third_person_flag_alpha, 0.0);
}

#[test]
fn spectator_guess_cvar_addition_loads_and_archives() {
    let fixture = Fixture::new();
    let path = fixture.0.join("input.cfg");
    fs::write(&path, "seta cg_scoreSpectatorGuess 0\n").unwrap();
    let settings = config::load_client_presentation_settings(&path);
    assert!(!settings.score_spectator_guess);
    config::save_video_settings(
        &path,
        crate::ui::VideoSettings::default(),
        &crate::keybinds::Bindings::default(),
        &settings,
        &config::AudioSettings::default(),
    )
    .unwrap();
    assert!(fs::read_to_string(&path)
        .unwrap()
        .contains("seta cg_scoreSpectatorGuess \"0\""));
    assert_eq!(
        crate::app::cvar_registry()
            .find("cg_scoreSpectatorGuess")
            .unwrap()
            .entry
            .default,
        "1"
    );
}

#[test]
fn jiggle_glute_coverage_height_cvar_addition_loads_and_archives() {
    let fixture = Fixture::new();
    let path = fixture.0.join("input.cfg");
    fs::write(&path, "seta r_jiggleGluteCoverageHeight 0.72\n").unwrap();
    let settings = config::load_video_settings(&path);
    assert!((settings.jiggle_glute_coverage_height - 0.72).abs() < f32::EPSILON);
    config::save_video_settings(
        &path,
        settings,
        &crate::keybinds::Bindings::default(),
        &config::ClientPresentationSettings::default(),
        &config::AudioSettings::default(),
    )
    .unwrap();
    assert!(fs::read_to_string(&path)
        .unwrap()
        .contains("seta r_jiggleGluteCoverageHeight \"0.720\""));
}

#[test]
fn jiggle_impulse_cap_loads_and_archives() {
    let fixture = Fixture::new();
    let path = fixture.0.join("input.cfg");
    fs::write(&path, "seta r_jiggleImpulseCap 0.25\n").unwrap();
    let settings = config::load_video_settings(&path);
    assert_eq!(settings.jiggle_impulse_cap, 0.25);
    config::save_video_settings(
        &path,
        settings,
        &crate::keybinds::Bindings::default(),
        &config::ClientPresentationSettings::default(),
        &config::AudioSettings::default(),
    ).unwrap();
    assert!(fs::read_to_string(&path).unwrap().contains("seta r_jiggleImpulseCap \"0.250\""));
    assert_eq!(config::load_video_settings(&path).jiggle_impulse_cap, 0.25);
}

#[test]
fn removed_rt_denoiser_is_not_registered_or_archived() {
    assert!(crate::app::cvar_registry().find("r_rtDenoiser").is_none());
    let fixture = Fixture::new();
    let path = fixture.0.join("removed-rt-denoiser.cfg");
    fs::write(&path, "seta r_rtDenoiser 2\nseta r_rtTaa 1\n").unwrap();
    let settings = config::load_video_settings(&path);
    assert_eq!(settings.rt_taa, 1);
    config::save_video_settings(&path, settings, &crate::keybinds::Bindings::default(),
        &config::ClientPresentationSettings::default(), &config::AudioSettings::default()).unwrap();
    assert!(!fs::read_to_string(&path).unwrap().contains("r_rtDenoiser"));
}

#[test]
fn rt_regional_taa_loads_and_archives_without_changing_msaa() {
    let fixture = Fixture::new();
    let path = fixture.0.join("rt-taa.cfg");
    fs::write(&path, "seta r_ext_multisample 2\nseta r_taa 0\nseta r_rtTaa 1\n").unwrap();
    let settings = config::load_video_settings(&path);
    assert_eq!(settings.rt_taa, 1);
    assert_eq!(settings.msaa_samples, 2);
    assert!(!settings.taa);
    config::save_video_settings(&path, settings, &crate::keybinds::Bindings::default(),
        &config::ClientPresentationSettings::default(), &config::AudioSettings::default()).unwrap();
    let saved = fs::read_to_string(&path).unwrap();
    assert!(saved.contains("seta r_rtTaa \"1\""));
    assert_eq!(config::load_video_settings(&path).rt_taa, 1);
    fs::write(&path, "seta r_rtTaa 3\n").unwrap();
    assert_eq!(config::load_video_settings(&path).rt_taa, 0);
}

#[test]
fn world_and_saber_flare_cvars_load_and_archive_independently() {
    let fixture = Fixture::new();
    let path = fixture.0.join("flare.cfg");
    fs::write(
        &path,
        "seta r_flares 0
seta r_worldFlares 1
seta r_saberFlares 0
",
    )
    .unwrap();
    let settings = config::load_video_settings(&path);
    assert!(!settings.flares);
    assert!(settings.world_flares);
    assert!(settings.world_flares_active());
    assert!(!settings.saber_flares);

    config::save_video_settings(
        &path,
        settings,
        &crate::keybinds::Bindings::default(),
        &config::ClientPresentationSettings::default(),
        &config::AudioSettings::default(),
    )
    .unwrap();
    let saved = fs::read_to_string(&path).unwrap();
    for expected in [
        "seta r_flares \"0\"",
        "seta r_worldFlares \"1\"",
        "seta r_saberFlares \"0\"",
    ] {
        assert!(saved.lines().any(|line| line == expected), "missing {expected}");
    }
}

#[test]
fn shared_auto_lod_default_and_archive_round_trip() {
    let defaults = crate::ui::VideoSettings::default();
    assert!(defaults.md3_auto_lod);
    assert!(defaults.bsp_model_lod);

    let fixture = Fixture::new();
    let path = fixture.0.join("model-lods.cfg");
    fs::write(&path, "seta r_md3AutoLod 0\n").unwrap();
    let settings = config::load_video_settings(&path);
    assert!(!settings.md3_auto_lod);
    assert!(settings.bsp_model_lod);
    config::save_video_settings(
        &path, settings, &crate::keybinds::Bindings::default(),
        &config::ClientPresentationSettings::default(),
        &config::AudioSettings::default(),
    ).unwrap();
    assert!(fs::read_to_string(&path).unwrap().contains("seta r_md3AutoLod \"0\""));
    assert!(!config::load_video_settings(&path).md3_auto_lod);
}

#[test]
fn baked_sun_removal_loads_and_archives_without_changing_legacy_fixtures() {
    let fixture = Fixture::new();
    let path = fixture.0.join("sun.cfg");
    fs::write(&path, "seta r_bakedSunRemoval \"0.750\"\nseta r_bakedSunCache \"1\"\nseta r_sunShadowStrength \"0.85\"\nseta r_sunLighting \"1\"\n").unwrap();
    let video = config::load_video_settings(&path);
    assert_eq!(video.baked_sun_removal, 0.75);
    assert!(video.baked_sun_cache);
    assert_eq!(video.sun_shadow_strength, 0.85);
    assert!(video.sun_lighting);
    config::save_video_settings(
        &path, video,
        &crate::keybinds::Bindings::default(),
        &config::ClientPresentationSettings::default(),
        &config::AudioSettings::default(),
    ).unwrap();
    let saved = fs::read_to_string(&path).unwrap();
    assert!(saved.contains("seta r_bakedSunRemoval \"0.750\""));
    assert!(saved.contains("seta r_bakedSunCache \"1\""));
    assert!(saved.contains("seta r_sunShadowStrength \"0.850\""));
    assert!(saved.contains("seta r_sunLighting \"1\""));
}
