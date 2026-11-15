use super::*;
use std::fs;

#[test]
fn map_identity_preserves_subdirectories_source_kind_and_protocol() {
    assert_eq!(map_key(" MAPS\\MP\\FFA3.BSP ", false), "jka:mp/ffa3.bsp");
    assert_eq!(
        map_key("mp/ffa3", false),
        map_key("maps/mp/ffa3.bsp", false)
    );
    assert_ne!(map_key("mp/ffa3", false), map_key("other/ffa3", false));
    assert_ne!(map_key("mp/ffa3.map", false), map_key("mp/ffa3.bsp", false));
    assert_ne!(map_key("mp/ffa3", false), map_key("mp/ffa3", true));
}

#[test]
fn lighting_snapshot_covers_saved_controls_and_excludes_unrelated_settings() {
    let values = capture(&VideoSettings::default());
    for name in LIGHTING_CVARS {
        assert!(values.contains_key(*name), "missing profile control {name}");
    }
    for name in [
        "r_blueSkyMoonCustom",
        "r_blueSkyMoonIntensity",
        "r_blueSkyRayleigh",
        "r_cloudCoverage",
        "r_cloudBackend",
        "r_blueSkyWorldLighting",
    ] {
        assert!(values.contains_key(name), "missing sky control {name}");
    }
    for name in [
        "r_gamma",
        "r_mode",
        "r_backend",
        "r_gpuTimings",
        "r_bakeThreads",
        "r_blueSkyDiag",
        "r_blueSkyStrikePreview",
        "r_blueSkyCubeAB",
        "r_blueCloudFreeze",
        "r_sunOverride",
        "r_cascadedShadows",
        "r_bakedLocalLights",
        "r_promoteShaderLights",
    ] {
        assert!(
            !values.contains_key(name),
            "profile acquired unrelated/alias cvar {name}"
        );
    }
    validate_values(&values).unwrap();
    let mut unsafe_values = values.clone();
    unsafe_values.insert("exec".into(), "anything.cfg".into());
    assert!(validate_values(&unsafe_values).is_err());
    unsafe_values = values;
    unsafe_values.insert("r_relightmap".into(), "1".into());
    assert!(
        validate_values(&unsafe_values).is_err(),
        "keys must use canonical case for ordered application"
    );
}

#[test]
fn archive_overlay_restores_only_lighting_and_its_derived_shadow_flag() {
    let normal = VideoSettings::default();
    let mut active = normal;
    active.sun_override = true;
    active.relighting_mode = 2;
    active.sun_yaw = 49.0;
    active.baked_sun_removal = 0.91;
    active.blue_water_environment.moon_phase = 0.8;
    active.dynamic_shadows = crate::ui::DynamicShadowsMode::CascadedShadowMapsEntities;
    active.cascaded_shadows = true;
    active.gamma = 2.1;
    active.fps_cap = 333;
    active.picmip = 2;
    copy_lighting(&mut active, &normal);
    assert_eq!(capture(&active), capture(&normal));
    assert_eq!(active.cascaded_shadows, normal.dynamic_shadows.is_csm());
    assert_eq!(active.gamma, 2.1);
    assert_eq!(active.fps_cap, 333);
    assert_eq!(active.picmip, 2);
}

#[test]
fn clock_ticks_do_not_mark_a_profile_modified_but_manual_edits_do() {
    let mut video = VideoSettings::default();
    video.relight_match_system_time = true;
    let saved = capture(&video);
    video.sun_yaw += 12.0;
    video.sun_pitch += 10.0;
    video.blue_water_environment.moon_azimuth += 15.0;
    video.blue_water_environment.moon_custom = true;
    assert!(values_match(&capture(&video), &saved));
    video.blue_water_environment.moon_phase = 0.123;
    assert!(!values_match(&capture(&video), &saved));
    video.blue_water_environment.moon_phase =
        VideoSettings::default().blue_water_environment.moon_phase;
    video.relight_match_system_time = false;
    assert!(!values_match(&capture(&video), &saved));
}

#[test]
fn locked_time_axes_are_transient_but_unlocked_absolute_overrides_are_edits() {
    let mut video = VideoSettings::default();
    video.relight_match_system_time = true;
    let saved = capture(&video);
    video.sun_yaw += 15.0;
    video.sun_pitch -= 8.0;
    video.sun_intensity *= 0.4;
    assert!(values_match(&capture(&video), &saved));
    video.sun_time_locks &= !VideoSettings::SUN_LOCK_YAW;
    assert!(!values_match(&capture(&video), &saved));
    video.sun_time_locks = 7;
    let mut manual = video;
    manual.sun_time_locks = 0;
    let manual_snapshot = capture(&manual);
    manual.sun_pitch += 10.0;
    assert!(!values_match(&capture(&manual), &manual_snapshot));
}

#[test]
fn library_round_trip_map_folders_rename_and_failed_save_preserve_work() {
    let fixture = crate::cvar_compatibility::Fixture::new();
    let root = fixture.0.join("light_profiles");
    let mut library = Library::read(&root).unwrap();
    library.profiles.insert(
        1,
        Profile {
            name: "Warm sunset".into(),
            map: Some(map_key("mp/ffa3", false)),
            values: capture(&VideoSettings::default()),
        },
    );
    library.map_defaults.insert(map_key("mp/ffa3", false), 1);
    library.write(&root).unwrap();
    let old = root.join("mp/ffa3/Warm sunset.json");
    assert!(old.is_file());
    library.profiles.get_mut(&1).unwrap().name = "Soft sunset".into();
    library.write(&root).unwrap();
    let path = root.join("mp/ffa3/Soft sunset.json");
    assert!(!old.exists());
    let saved = fs::read(&path).unwrap();
    let read = Library::read(&root).unwrap();
    assert_eq!(read.profiles[&1].name, "Soft sunset");
    assert_eq!(read.map_defaults["jka:mp/ffa3.bsp"], 1);
    let map = Some("jka:mp/ffa3.bsp");
    assert!(read.check_unique_name("soft SUNSET", map, None).is_err());
    assert!(read.check_unique_name("Soft sunset", map, Some(1)).is_ok());
    assert!(read
        .check_unique_name("Soft sunset", Some("jka:mp/ffa4.bsp"), None)
        .is_ok());
    library.version = 999;
    assert!(library.write(&root).is_err());
    assert_eq!(fs::read(&path).unwrap(), saved);
    let mut invalid: serde_json::Value = serde_json::from_slice(&saved).unwrap();
    invalid["version"] = serde_json::json!(999);
    fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
    assert!(Library::read(&root).is_err());
    library.version = 1;
    library.remove(1);
    assert!(library.map_defaults.is_empty());
    library.write(&root).unwrap();
    assert!(!path.exists());
    assert!(Library::read(&root).unwrap().profiles.is_empty());
}

#[test]
fn profile_paths_stay_relative_and_global_jk2_source_maps_do_not_collide() {
    let mut profile = Profile {
        name: "Warm sunset".into(),
        map: Some(map_key("mp/ffa3", false)),
        values: capture(&VideoSettings::default()),
    };
    assert_eq!(
        storage::relative_path(&profile).unwrap(),
        PathBuf::from("mp/ffa3/Warm sunset.json")
    );
    profile.map = Some(map_key("mp/ffa3", true));
    assert_eq!(
        storage::relative_path(&profile).unwrap(),
        PathBuf::from("_jk2/mp/ffa3/Warm sunset.json")
    );
    profile.map = Some(map_key("mp/ffa3.map", false));
    assert_eq!(
        storage::relative_path(&profile).unwrap(),
        PathBuf::from("_source/mp/ffa3/Warm sunset.json")
    );
    profile.map = None;
    assert_eq!(
        storage::relative_path(&profile).unwrap(),
        PathBuf::from("_global/Warm sunset.json")
    );
    profile.name = "../CON:night".into();
    profile.map = Some("jka:../../CON.map".into());
    let path = storage::relative_path(&profile).unwrap();
    assert!(path
        .components()
        .all(|part| matches!(part, std::path::Component::Normal(_))));
    assert!(!path.to_string_lossy().contains(':'));
    profile.map = Some("jka:_global.bsp".into());
    assert!(storage::relative_path(&profile)
        .unwrap()
        .starts_with("%5Fglobal"));
}

#[test]
fn copied_profile_files_are_discovered_and_untracked_files_are_not_deleted() {
    let fixture = crate::cvar_compatibility::Fixture::new();
    let root = fixture.0.join("light_profiles");
    let mut library = Library::default();
    library.profiles.insert(
        1,
        Profile {
            name: "Sunset".into(),
            map: Some(map_key("mp/ffa3", false)),
            values: capture(&VideoSettings::default()),
        },
    );
    library.write(&root).unwrap();
    let imported = root.join("mp/ffa4/Evening.json");
    fs::create_dir_all(imported.parent().unwrap()).unwrap();
    let mut data: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("mp/ffa3/Sunset.json")).unwrap()).unwrap();
    data["name"] = serde_json::json!("Evening");
    data["map"] = serde_json::json!("jka:mp/ffa4.bsp");
    fs::write(&imported, serde_json::to_vec(&data).unwrap()).unwrap();
    assert_eq!(Library::read(&root).unwrap().profiles.len(), 2);
    library.remove(1);
    library.write(&root).unwrap();
    assert!(imported.is_file());
    let read = Library::read(&root).unwrap();
    assert_eq!(read.profiles.len(), 1);
    assert_eq!(read.profiles[&1].name, "Evening");
}

#[cfg(target_os = "windows")]
#[test]
fn profiles_restore_custom_positions_rollback_failures_and_follow_map_defaults() {
    use winit::platform::windows::EventLoopBuilderExtWindows;
    let event_loop = winit::event_loop::EventLoop::<crate::runtime::UserEvent>::with_user_event()
        .with_any_thread(true)
        .build()
        .unwrap();
    let fixture = crate::cvar_compatibility::Fixture::new();
    let mut app = App::new(
        fixture.0.clone(),
        None,
        crate::scene::MapSource::Bsp("mp/ffa3".into()),
        false,
        event_loop.create_proxy(),
    )
    .unwrap();
    app.config_path = fixture.0.join("DinurdoJK.cfg");
    app.fs_roots.client_install = Some(fixture.0.clone());
    app.relighting_profile_map_loaded("maps/mp/ffa3.bsp", 1);
    let normal = app.video;
    let normal_initialized = app.sun_custom_initialized;
    let mut look = normal;
    look.sun_override = true;
    look.relighting_mode = 1;
    look.relight_time = 19.25;
    look.sun_yaw = 123.0;
    look.sun_pitch = 17.0;
    look.sun_intensity = 250.0;
    look.sun_color = [0.8, 0.6, 0.2];
    look.blue_water_environment.moon_custom = false;
    look.blue_water_environment.moon_elevation = 22.0;
    look.blue_water_environment.moon_azimuth = 45.0;
    look.dynamic_shadows = crate::ui::DynamicShadowsMode::Off;
    look.cascaded_shadows = false;
    let mut library = Library::default();
    library.profiles.insert(
        1,
        Profile {
            name: "Sunset".into(),
            map: app.relighting_profiles.map.clone(),
            values: capture(&look),
        },
    );
    app.commit_relighting_library(library).unwrap();
    let location = fixture.0.join("DinurdoJK/light_profiles");
    assert_eq!(app.relighting_library_path(), location);
    assert!(location.join("mp/ffa3/Sunset.json").is_file());
    app.game = Some(fixture.0.join("some_mod"));
    assert_eq!(
        app.relighting_library_path(),
        location,
        "profiles must not move with fs_game"
    );
    app.game = None;
    app.load_relighting_profile(1).unwrap();
    assert_eq!(
        capture(&app.video),
        capture(&look),
        "time/mode setters must not overwrite custom positions or shadow choices"
    );
    assert!(!app.relighting_profile_modified());
    app.video.gamma = 2.1;
    app.set_console_cvar("r_bakedSunRemoval", "0.67").unwrap();
    assert!(app.relighting_profile_modified());
    app.save_relighting_profile(false).unwrap();
    assert!(!app.relighting_profile_modified());
    app.set_console_cvar("r_bakedSunRemoval", "0.12").unwrap();
    app.load_relighting_profile(1).unwrap();
    assert_eq!(app.video.baked_sun_removal, 0.67);
    app.flush_config();
    let archived = config::load_video_settings(&app.config_path);
    assert_eq!(
        capture(&archived),
        capture(&normal),
        "map lighting must not leak into DinurdoJK.cfg"
    );
    assert_eq!(archived.gamma, 2.1, "unrelated edits must still save");

    let before = capture(&app.video);
    app.relighting_profiles
        .library
        .as_mut()
        .unwrap()
        .profiles
        .get_mut(&1)
        .unwrap()
        .values
        .insert("r_sunPitch".into(), "invalid".into());
    assert!(app.load_relighting_profile(1).is_err());
    assert_eq!(
        capture(&app.video),
        before,
        "a late text-setter error must roll back all earlier changes"
    );
    app.relighting_profiles.library = Some(Library::read(&app.relighting_library_path()).unwrap());

    app.set_relighting_map_default(Some(1)).unwrap();
    app.set_console_cvar("r_bakedSunRemoval", "0.11").unwrap();
    app.relighting_profile_map_loaded("mp/ffa3.bsp", 99);
    assert_eq!(
        app.video.baked_sun_removal, 0.11,
        "same-map renderer upload retains unsaved edits"
    );
    app.relighting_profiles.reload_request = Some(100);
    app.relighting_profile_map_loaded("mp/ffa3.bsp", 100);
    assert_eq!(
        app.video.baked_sun_removal, 0.67,
        "actual same-map re-entry reloads its default"
    );
    app.relighting_profile_map_loaded("mp/ffa4.bsp", 101);
    assert_eq!(capture(&app.video), capture(&normal));
    assert_eq!(app.sun_custom_initialized, normal_initialized);
    assert_eq!(app.video.gamma, 2.1);
    assert!(!app.has_relighting_overlay());
    app.relighting_profile_map_loaded("mp/ffa3.bsp", 102);
    assert_eq!(app.relighting_profiles.active, Some(1));
    assert_eq!(app.video.baked_sun_removal, 0.67);

    app.relighting_profiles.name_input = "Night".into();
    app.save_relighting_profile(true).unwrap();
    let second = app.relighting_profiles.active.unwrap();
    assert_ne!(second, 1);
    app.relighting_profiles.name_input = "Late sunset".into();
    app.rename_relighting_profile(1).unwrap();
    assert_eq!(
        app.relighting_profiles
            .library
            .as_ref()
            .unwrap()
            .map_defaults["jka:mp/ffa3.bsp"],
        1
    );
    app.delete_relighting_profile(1).unwrap();
    assert!(app
        .relighting_profiles
        .library
        .as_ref()
        .unwrap()
        .map_defaults
        .is_empty());
    app.relighting_profiles_disconnected();
    assert_eq!(capture(&app.video), capture(&normal));
    assert!(app.relighting_profiles.map.is_none());
}
