use super::*;
use crate::cvar_compatibility::{assert_snapshot, digest, Fixture, INPUTS};
use std::fs;

#[test]
#[ignore = "requires default audio device, EAL_TEST_MAP and EAL_TEST_OPENAL_DLL"]
fn local_game_loads_eal_before_live_toggles_and_sound_restart() {
    use winit::platform::windows::EventLoopBuilderExtWindows;
    let event_loop = winit::event_loop::EventLoop::<crate::runtime::UserEvent>::with_user_event()
        .with_any_thread(true).build().unwrap();
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.0.join("eagle")).unwrap();
    fs::create_dir_all(fixture.0.join("models/players/_humanoid")).unwrap();
    fs::write(fixture.0.join("models/players/_humanoid/animation.cfg"), "BOTH_STAND1 0 1 -1 20\n").unwrap();
    fs::copy(std::env::var_os("EAL_TEST_MAP").expect("EAL_TEST_MAP"), fixture.0.join("eagle/ffa3.eal")).unwrap();
    fs::write(fixture.0.join("DinurdoJK.cfg"), "seta s_steamAudio 0\nseta s_eal 0\n").unwrap();
    let mut app = App::new(fixture.0.clone(), None, crate::scene::MapSource::Bsp("mp/ffa3".into()),
        false, event_loop.create_proxy()).unwrap();
    let server = crate::local_server::LocalServer::new(
        "mp/ffa3", None, None, crate::scene::SpawnPoint::default(), &app.solo_client_info,
        [jka_movement::SaberMovementInfo::equipped_default(), jka_movement::SaberMovementInfo::default()],
        &[], &[], &[], None, None,
    ).unwrap();
    let session = app.build_local_game_session(&server).unwrap();
    let info = session.sound_presenter.as_ref().unwrap().info().unwrap();
    assert_eq!(info.eal_file.as_deref(), Some("eagle/ffa3.eal"));
    assert!(!info.eal_effects_active);
    app.game_session = Some(session);
    for enabled in [true, false, true] {
        app.set_console_cvar("s_eal", if enabled { "1" } else { "0" }).unwrap();
        let info = app.game_session.as_ref().unwrap().sound_presenter.as_ref().unwrap().info().unwrap();
        assert_eq!(info.eal_effects_active, enabled, "{:?}", info.eal_effects_error);
        assert_eq!(info.eal_file.as_deref(), Some("eagle/ffa3.eal"));
    }
    app.set_console_cvar("s_ealHrtf", "1").unwrap();
    assert!(!app.audio.steam_audio);
    let wait_for_hrtf = |app:&App| {
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(2);
        loop {
            let info=app.game_session.as_ref().unwrap().sound_presenter.as_ref().unwrap().info().unwrap();
            if info.eal_hrtf { assert_eq!(info.eal_transport,1);break; }
            assert!(std::time::Instant::now()<deadline,"independent HRTF did not become active: {:?}",info.eal_effects_error);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    wait_for_hrtf(&app);
    app.restart_sound_subsystem();
    wait_for_hrtf(&app);
    let info = app.game_session.as_ref().unwrap().sound_presenter.as_ref().unwrap().info().unwrap();
    assert!(info.eal_effects_active, "{:?}", info.eal_effects_error);
    assert_eq!(info.eal_file.as_deref(), Some("eagle/ffa3.eal"));
    app.flush_config();
    assert!(crate::config::load_audio_settings(&fixture.0.join("DinurdoJK.cfg")).eal_enabled);
    assert!(crate::config::load_audio_settings(&fixture.0.join("DinurdoJK.cfg")).eal_hrtf);
}

#[test]
fn hybrid_breaker_applies_live_and_round_trips_without_changing_sea_state() {
    use winit::platform::windows::EventLoopBuilderExtWindows;
    let event_loop = winit::event_loop::EventLoop::<crate::runtime::UserEvent>::with_user_event()
        .with_any_thread(true).build().unwrap();
    let fixture = Fixture::new();
    let path = fixture.0.join("DinurdoJK.cfg");
    fs::write(&path, "").unwrap();
    let mut app = App::new(fixture.0.clone(), None, crate::scene::MapSource::Bsp("mp/ffa3".into()),
        false, event_loop.create_proxy()).unwrap();
    let waves = app.video.ocean_settings.blue_water_waves;
    assert_eq!(app.video.ocean_settings.hybrid_breaker, 0);
    assert_eq!(crate::app::cvar_registry().find("r_hybridBreaker").unwrap().entry.latch,
        crate::console::LatchScope::None);
    for mode in [2, 1, 0] {
        app.set_console_cvar("r_hybridBreaker", &mode.to_string()).unwrap();
        assert_eq!(app.video.ocean_settings.hybrid_breaker, mode);
        assert_eq!(app.video.ocean_settings.blue_water_waves, waves);
        app.flush_config();
        assert_eq!(crate::config::load_video_settings(&path).ocean_settings.hybrid_breaker, mode);
    }
    assert!(app.set_console_cvar("r_hybridBreaker", "3").is_err());
}

#[test]
fn inline_transform_modes_apply_live_and_round_trip() {
    use crate::ui::InlineTransformMode;
    use winit::platform::windows::EventLoopBuilderExtWindows;
    let event_loop = winit::event_loop::EventLoop::<crate::runtime::UserEvent>::with_user_event()
        .with_any_thread(true).build().unwrap();
    let fixture = Fixture::new();
    let path = fixture.0.join("DinurdoJK.cfg");
    fs::write(&path, "").unwrap();
    let mut app = App::new(fixture.0.clone(), None, crate::scene::MapSource::Bsp("mp/ffa3".into()),
        false, event_loop.create_proxy()).unwrap();
    assert_eq!(app.video.inline_transform_mode, InlineTransformMode::Gpu);
    let definition = crate::app::cvar_registry().find("r_inlineTransform").unwrap();
    assert_eq!(definition.entry.latch, crate::console::LatchScope::None);
    for (value, expected) in [
        ("workers", InlineTransformMode::CpuWorkers),
        ("gpu", InlineTransformMode::Gpu),
        ("cpu", InlineTransformMode::Cpu),
    ] {
        app.set_console_cvar("r_inlineTransform", value).unwrap();
        assert_eq!(app.video.inline_transform_mode, expected);
        assert_eq!(app.console_cvar_value("r_inlineTransform").as_deref(), Some(value));
        app.flush_config();
        assert_eq!(crate::config::load_video_settings(&path).inline_transform_mode, expected);
    }
    app.set_console_cvar("r_inlineGpuTransform", "1").unwrap();
    assert_eq!(app.video.inline_transform_mode, InlineTransformMode::Gpu);
    app.set_console_cvar("r_inlineGpuTransform", "0").unwrap();
    assert_eq!(app.video.inline_transform_mode, InlineTransformMode::Cpu);
    app.set_console_cvar("r_inlineTransform", "workers").unwrap();
    assert!(app.set_console_cvar("r_inlineTransform", "invalid").is_err());
    assert_eq!(app.video.inline_transform_mode, InlineTransformMode::CpuWorkers);
}

#[test]
fn upstream_coast_is_saved_separately_and_preserves_jka_solver_choice() {
    use winit::platform::windows::EventLoopBuilderExtWindows;
    let event_loop=winit::event_loop::EventLoop::<crate::runtime::UserEvent>::with_user_event()
        .with_any_thread(true).build().unwrap();
    let fixture=Fixture::new();let path=fixture.0.join("DinurdoJK.cfg");fs::write(&path,"").unwrap();
    let mut app=App::new(fixture.0.clone(),None,crate::scene::MapSource::Bsp("mp/ffa3".into()),false,event_loop.create_proxy()).unwrap();
    app.set_console_cvar("r_oceanType","3").unwrap();
    app.set_console_cvar("r_coastal","1").unwrap();
    app.set_console_cvar("r_coastalPool","2").unwrap();
    let waves=app.video.ocean_settings.blue_water_waves;
    app.set_console_cvar("r_blueWaterCoast","1").unwrap();
    assert!(!app.video.ocean_settings.blue_water_coast_shelter);
    app.set_console_cvar("r_blueWaterCoastShelter","1").unwrap();
    app.set_console_cvar("r_blueWaterCoastShelter","0").unwrap();
    assert!(app.video.ocean_settings.blue_water_coast && app.coastal_enabled);
    assert_eq!(app.coastal_pool,2);
    app.flush_config();
    assert!(crate::config::load_video_settings(&path).ocean_settings.blue_water_coast);
    assert!(!crate::config::load_video_settings(&path).ocean_settings.blue_water_coast_shelter);
    app.set_console_cvar("r_blueWaterCoast","0").unwrap();
    assert!(app.coastal_enabled);
    assert_eq!(app.video.ocean_settings.blue_water_waves,waves);
    assert_eq!(app.video.ocean_settings.backend,2);
    app.flush_config();
    assert!(!crate::config::load_video_settings(&path).ocean_settings.blue_water_coast);
}

#[test]
fn lake_promotion_switches_live_and_round_trips_without_overwriting_ocean() {
    use winit::platform::windows::EventLoopBuilderExtWindows;
    let event_loop=winit::event_loop::EventLoop::<crate::runtime::UserEvent>::with_user_event()
        .with_any_thread(true).build().unwrap();
    let fixture=Fixture::new();
    let path=fixture.0.join("DinurdoJK.cfg");
    fs::write(&path, "").unwrap();
    let mut app=App::new(fixture.0.clone(),None,crate::scene::MapSource::Bsp("mp/ffa3".into()),
        false,event_loop.create_proxy()).unwrap();
    app.set_console_cvar("r_oceanType","3").unwrap();
    let ocean=app.video.ocean_settings;
    app.set_console_cvar("r_oceanType","4").unwrap();
    app.set_console_cvar("r_lakeWindSpeed","7.5").unwrap();
    app.set_console_cvar("r_lakeDirection","-35").unwrap();
    app.set_console_cvar("r_lakeTransparency","2").unwrap();
    app.set_console_cvar("r_lakeReflections","0.35").unwrap();
    app.set_console_cvar("r_lakeSpeed","1.8").unwrap();
    app.set_console_cvar("r_lakeTint","0.4 0.8 0.9").unwrap();
    app.set_console_cvar("r_reflectionQuality","off").unwrap();
    app.set_console_cvar("r_lakeReflectionMode","2").unwrap();
    assert_eq!(app.video.ocean_settings.lake.reflection_mode,2);
    assert_eq!(app.video.reflection_quality,crate::ui::ReflectionQuality::Off);
    app.set_console_cvar("r_lakeShader","1").unwrap();
    app.set_console_cvar("r_lakeTransmission","0.65").unwrap();
    app.set_console_cvar("r_lakeRefraction","0.02").unwrap();
    app.set_console_cvar("r_lakeOpaqueDepth","12.5").unwrap();
    app.set_console_cvar("r_lakeWake","0.6").unwrap();
    assert_eq!(app.video.ocean_settings.lake.wake_strength,0.6);
    assert_eq!(app.video.ocean_settings.lake.opaque_depth,12.5);
    app.set_console_cvar("r_lakeDepthColor","0.8").unwrap();
    app.set_console_cvar("r_lakeColorDepth","6").unwrap();
    app.set_console_cvar("r_lakeShallowColor","0.3 0.6 0.5").unwrap();
    app.set_console_cvar("r_lakeDeepColor","0.02 0.1 0.2").unwrap();
    assert_eq!(app.video.ocean_settings.lake.depth_color_strength,0.8);
    assert_eq!(app.video.ocean_settings.lake.color_depth,6.0);
    assert_eq!(app.video.ocean_settings.lake.shallow_color,[0.3,0.6,0.5]);
    assert_eq!(app.video.ocean_settings.lake.deep_color,[0.02,0.1,0.2]);
    assert_eq!(app.video.ocean_settings.lake.cheap_transmission,0.65);
    assert_eq!(app.video.ocean_settings.lake.cheap_refraction,0.02);
    app.set_console_cvar("r_lakeReflectionMode","1").unwrap();
    app.set_console_cvar("r_lakeAmplitude","12").unwrap();
    app.set_console_cvar("r_lakeWaterColor","0.2 0.5 0.8").unwrap();
    app.set_console_cvar("r_lakeFogColor","0.1 0.3 0.4").unwrap();
    assert_eq!(app.video.ocean_settings.lake.amplitude,12.0);
    assert_eq!(app.video.ocean_settings.lake.reflections,0.35);
    assert_eq!(app.video.ocean_settings.lake.speed,1.8);
    assert_eq!(app.video.ocean_settings.lake.tint_color,[0.4,0.8,0.9]);
    assert_eq!(app.video.ocean_settings.lake.shader,1);
    assert_eq!(app.video.ocean_settings.lake.reflection_mode,1);
    assert_eq!(app.video.reflection_quality,crate::ui::ReflectionQuality::Off);
    assert_eq!(app.video.ocean_settings.lake.water_color,[0.2,0.5,0.8]);
    assert!(app.video.ocean);
    assert_eq!(app.video.ocean_settings.backend,3);
    let resolved=app.video.ocean_settings.resolved_for_gpu();
    assert!(!resolved.blue_water_adapter && !resolved.interaction);
    assert_eq!(resolved.blue_water_caustics_mode,2);
    assert_eq!(resolved.optics.fog_color,[0.1,0.3,0.4]);
    app.flush_config();
    let saved=crate::config::load_video_settings(&path);
    assert!(saved.ocean);
    assert_eq!(saved.ocean_settings.backend,3);
    assert_eq!(saved.ocean_settings.lake,app.video.ocean_settings.lake);
    app.set_console_cvar("r_oceanType","3").unwrap();
    assert_eq!(app.video.ocean_settings.blue_water_waves,ocean.blue_water_waves);
    assert_eq!(app.video.ocean_settings.hybrid_transparency,ocean.hybrid_transparency);
    assert_eq!(app.video.ocean_settings.hybrid_underwater_mode,ocean.hybrid_underwater_mode);
    app.set_console_cvar("r_oceanType","0").unwrap();
    assert!(!app.video.ocean);
}

#[test]
fn game_launch_screen_opens_preferred_page_without_waiting_for_scene() {
    use crate::app::{FrontendPage, OverlayMode};
    use crate::config::GameLaunchScreen;
    use winit::platform::windows::EventLoopBuilderExtWindows;
    let event_loop = winit::event_loop::EventLoop::<crate::runtime::UserEvent>::with_user_event()
        .with_any_thread(true).build().unwrap();
    let fixture = Fixture::new();
    let path = fixture.0.join("DinurdoJK.cfg");
    for &(screen, name, _) in GameLaunchScreen::OPTIONS {
        fs::write(&path, format!("seta ui_gameLaunchScreen \"{name}\"\n")).unwrap();
        let mut app = App::new(fixture.0.clone(), None,
            crate::scene::MapSource::Bsp("mp/ffa3".into()), false,
            event_loop.create_proxy()).unwrap();
        assert_eq!(app.game_launch_screen, screen);
        assert_eq!(app.frontend_skip_chrome_fade, screen != GameLaunchScreen::Main);
        assert!(app.render.is_none(), "navigation is ready before renderer/scene initialization");
        assert!(app.loading.is_none(), "frontend scene must not block the preferred page");
        if screen == GameLaunchScreen::Settings {
            assert_eq!(app.overlay, OverlayMode::Video);
        } else {
            assert_eq!(app.overlay, OverlayMode::Game);
            let expected = match screen {
                GameLaunchScreen::Main => FrontendPage::Main,
                GameLaunchScreen::Play => FrontendPage::Play,
                GameLaunchScreen::Solo => FrontendPage::SoloGame,
                GameLaunchScreen::Servers => FrontendPage::ServerBrowser,
                GameLaunchScreen::Demo => FrontendPage::PlayDemo,
                GameLaunchScreen::Profile => FrontendPage::Profile,
                GameLaunchScreen::Controls => FrontendPage::Controls,
                GameLaunchScreen::ChatLogs => FrontendPage::ChatLogs,
                GameLaunchScreen::Screenshots => FrontendPage::Screenshots,
                GameLaunchScreen::DeveloperTools => FrontendPage::DeveloperTools,
                GameLaunchScreen::Assets => FrontendPage::AssetViewer,
                GameLaunchScreen::Maps => FrontendPage::MapViewer,
                GameLaunchScreen::Settings => unreachable!(),
            };
            assert_eq!(app.frontend_page, expected);
        }
        if screen != GameLaunchScreen::Main {
            app.open_ui_page(&["main"]);
            assert_eq!(app.frontend_page, FrontendPage::Main);
            assert!(app.frontend_skip_chrome_fade, "returning to Main must not wait for its scene");
        }
        app.set_console_cvar("ui_gameLaunchScreen", "solo").unwrap();
        assert!(app.set_console_cvar("ui_gameLaunchScreen", "missing-page").is_err());
        assert_eq!(app.game_launch_screen, GameLaunchScreen::Solo);
    }
    fs::write(&path, "seta ui_gameLaunchScreen \"setup\"\n").unwrap();
    let app = App::new(fixture.0.clone(), None,
        crate::scene::MapSource::Bsp("mp/ffa3".into()), true,
        event_loop.create_proxy()).unwrap();
    assert!(!app.front_end);
    assert!(app.loading.is_some(), "explicit map launch takes priority");
    assert_eq!(app.overlay, OverlayMode::Game);
}

#[test]
fn cvar_runtime_setters_and_latches_match_baseline() {
    use winit::platform::windows::EventLoopBuilderExtWindows;
    let event_loop = winit::event_loop::EventLoop::<crate::runtime::UserEvent>::with_user_event()
        .with_any_thread(true)
        .build()
        .unwrap();
    let fixture = Fixture::new();
    let directory = fixture.0.clone();
    fs::write(directory.join("DinurdoJK.cfg"), "").unwrap();
    let mut app = App::new(
        directory.clone(),
        None,
        crate::scene::MapSource::Bsp("mp/ffa3".into()),
        false,
        event_loop.create_proxy(),
    )
    .unwrap();
    assert_command_encoding_ab_controls(&mut app);
    let names: Vec<_> = include_str!("../../../testdata/cvars/runtime-matrix.txt")
        .lines()
        .map(|row| row.split_whitespace().next().unwrap())
        .collect();
    let mut snapshot = String::new();
    // The frozen runtime baseline includes two removed prediction cvars in its
    // vector of current values. Reconstruct only their historical entries to
    // compare the remaining cvar behavior without rebaselining every hash.
    let mut old_predict_backend = -1i32;
    let mut old_snap_mode = -1i32;
    for name in &names {
        if *name == "cg_predictbackend" {
            for value in INPUTS { old_predict_backend = jka_protocol::commands::atoi(value.as_bytes()).clamp(-1, 1); }
            continue;
        }
        if *name == "cg_snapmode" {
            for value in INPUTS { old_snap_mode = jka_protocol::commands::atoi(value.as_bytes()).clamp(-1, 4); }
            continue;
        }
        snapshot.push_str(name);
        for value in INPUTS {
            app.config_dirty = false;
            app.japro_camera_userinfo_dirty = false;
            app.japro_net_userinfo_dirty = false;
            app.next_command_time = Some(1.0);
            let result = app.set_console_cvar(name, value);
            let values: Vec<_> = names
                .iter()
                .map(|name| match *name {
                    "cg_predictbackend" => Some(old_predict_backend.to_string()),
                    "cg_snapmode" => Some(old_snap_mode.to_string()),
                    _ => app.console_cvar_value(name),
                })
                .collect();
            snapshot.push(' ');
            snapshot.push_str(&digest(&format!(
                "{result:?}|{values:?}|{}|{}|{}|{}",
                app.config_dirty,
                app.japro_camera_userinfo_dirty,
                app.japro_net_userinfo_dirty,
                app.next_command_time.is_some()
            )));
        }
        snapshot.push('\n');
    }
    assert_snapshot("runtime-matrix.txt", &snapshot);
    let mut snapshot = String::new();
    for name in [
        "r_fullscreen",
        "r_backend",
        "r_customwidth",
        "r_customheight",
        "r_pbr",
        "r_genNormalMaps",
        "r_floatLightmap",
        "fs_allowAssetOverrides",
        "r_picmip",
    ] {
        snapshot.push_str(name);
        for value in INPUTS {
            snapshot.push(' ');
            snapshot.push_str(&digest(&format!(
                "{:?}",
                app.normalize_latched_console_value(name, value)
            )));
        }
        snapshot.push('\n');
    }
    assert_snapshot("latches.txt", &snapshot);
    drop(app);
    drop(event_loop);
}

#[test]
fn removed_prediction_controls_are_not_registered() {
    for name in ["cg_predictBackend", "cg_snapMode"] {
        assert!(crate::app::cvar_registry().find(name).is_none(), "{name} must not select physics");
        assert!(crate::net::NetworkSettings::default().cvar_value(name).is_none());
    }
}

#[test]
fn ocean_enable_is_archived_before_apply() {
    use winit::platform::windows::EventLoopBuilderExtWindows;
    let event_loop = winit::event_loop::EventLoop::<crate::runtime::UserEvent>::with_user_event()
        .with_any_thread(true).build().unwrap();
    let fixture = Fixture::new();
    let path = fixture.0.join("DinurdoJK.cfg");
    fs::write(&path, "seta r_ocean \"0\"\n").unwrap();
    let mut app = App::new(fixture.0.clone(), None,
        crate::scene::MapSource::Bsp("mp/ffa3".into()), false,
        event_loop.create_proxy()).unwrap();
    assert!(!app.applied_ocean);
    // The menu can stage ON while the current world has no ocean resources.
    app.environment_selected = crate::ui::ENV_ROW_OCEAN;
    app.change_environment_setting(1);
    assert!(app.video.ocean);
    app.flush_config();
    assert!(crate::config::load_video_settings(&path).ocean,
        "a pending ocean enable must survive quitting before Apply");
    // The console uses the same saved preference, including immediate OFF.
    app.set_console_cvar("r_ocean", "0").unwrap();
    app.flush_config();
    assert!(!crate::config::load_video_settings(&path).ocean);
    app.set_console_cvar("r_ocean", "1").unwrap();
    app.flush_config();
    assert!(crate::config::load_video_settings(&path).ocean);
}

#[test]
fn stage_collapse_extra_is_live_and_archived() {
    use crate::console::LatchScope;
    let definition = crate::app::cvar_registry()
        .find("r_stageCollapseExtra")
        .expect("renderer stage-collapse cvar registered");
    assert_eq!(definition.entry.latch, LatchScope::None);
    assert!(!crate::ui::VideoSettings::default().stage_collapse_extra);
}

#[test]
fn additive_stage_fusion_is_live_and_archived() {
    use crate::console::{cvars::CvarFlags, LatchScope};
    let definition = crate::app::cvar_registry()
        .find("r_additiveStageFusion")
        .expect("additive-stage fusion cvar registered");
    assert_eq!(definition.entry.latch, LatchScope::None);
    assert!(definition.flags.contains(CvarFlags::ARCHIVE));
    assert!(crate::ui::VideoSettings::default().additive_stage_fusion);
}

fn assert_command_encoding_ab_controls(app: &mut App) {
    use crate::console::{cvars::CvarFlags, LatchScope};
    app.config_dirty = false;
    for (name, baseline, experimental) in [
        ("r_modelStateCache", "0", "1"),
        ("r_dynamicBatchDraws", "1", "0"),
        ("r_worldCompactMinDraws", "1", "2"),
        ("r_modelPrepareThreads", "2", "0"),
        ("r_md3GeometryCache", "1", "0"),
        ("r_staticMd3Instances", "0", "1"),
        ("r_staticMd3Bvh", "0", "1"),
        ("r_occlusionStats", "0", "1"),
        ("r_fxOcclusion", "0", "1"),
        ("r_modelRenderBundles", "0", "1"),
        ("r_encoderFinishThreads", "2", "0"),
        ("r_modelIndexUploadCache", "0", "1"),
        ("r_ghoul2BatchKeyCache", "1", "0"),
    ] {
        let definition = crate::app::cvar_registry().find(name).unwrap();
        assert_eq!(definition.entry.latch, LatchScope::None);
        assert!(!definition.flags.contains(CvarFlags::ARCHIVE));
        assert_eq!(app.console_cvar_value(name).as_deref(), Some(baseline));
        app.set_console_cvar(name, experimental).unwrap();
        assert_eq!(app.console_cvar_value(name).as_deref(), Some(experimental));
        assert!(!app.config_dirty);
        assert!(app.set_console_cvar(name, "invalid").is_err());
        assert_eq!(app.console_cvar_value(name).as_deref(), Some(experimental));
        app.set_console_cvar(name, baseline).unwrap();
    }
    for value in ["0", "3", "-1", "1.5"] {
        assert!(app.set_console_cvar("r_worldCompactMinDraws", value).is_err());
        assert_eq!(app.video.world_compact_min_draws, 1);
    }
    app.set_console_cvar("r_encoderFinishThreads", "1").unwrap();
    assert_eq!(app.video.encoder_finish_threads, 1);
    app.set_console_cvar("r_encoderFinishThreads", "0").unwrap();
    for value in ["3", "-1", "1.5"] {
        assert!(app.set_console_cvar("r_encoderFinishThreads", value).is_err());
        assert_eq!(app.video.encoder_finish_threads, 0);
    }
    for value in ["3", "8", "-1", "1.5"] {
        assert!(app.set_console_cvar("r_modelPrepareThreads", value).is_err());
        assert_eq!(app.video.model_prepare_threads, 2);
    }
}

#[test]
fn movement_components_match_editor_selection_and_archive() {
    use crate::ui::{HudElementId as Id, movement_components as components};
    use winit::platform::windows::EventLoopBuilderExtWindows;
    let event_loop = winit::event_loop::EventLoop::<crate::runtime::UserEvent>::with_user_event()
        .with_any_thread(true).build().unwrap();
    let fixture = Fixture::new();
    let path = fixture.0.join("DinurdoJK.cfg");
    fs::write(&path, "").unwrap();
    let mut app = App::new(fixture.0.clone(), None, crate::scene::MapSource::Bsp("mp/ffa3".into()),
        false, event_loop.create_proxy()).unwrap();
    app.set_console_cvar("cg_speedometer", "0").unwrap();
    app.set_console_cvar("cg_strafeHelper", "0").unwrap();
    for id in [Id::Speedometer, Id::AccelMeter, Id::StrafeGuide, Id::VelocityCompass, Id::SpeedGraph,
        Id::GroundSpeed, Id::JumpHeight, Id::JumpDistance, Id::VerticalSpeed, Id::YawSpeed, Id::SpeedometerJumps] {
        assert!(!app.hud_edit_element_enabled(id), "off {id:?}");
    }
    app.set_console_cvar("cg_speedometer", "65").unwrap();
    app.set_console_cvar("cg_strafeHelper", &(crate::ui::SHELPER_Q3DF_SPEED | crate::ui::SHELPER_Q3DF_ACCEL | crate::ui::SHELPER_WARSOW_HUD).to_string()).unwrap();
    assert_eq!(app.japro_cg.speedometer.flags & 65, 0);
    assert_eq!(components::selected(Id::Speedometer, app.strafe_helper.flags, app.japro_cg.speedometer.flags), 2);
    assert!(app.hud_edit_element_enabled(Id::Speedometer));
    assert!(app.hud_edit_element_enabled(Id::AccelMeter));
    app.set_console_cvar("cg_speedometer", "64").unwrap();
    assert_eq!(app.strafe_helper.flags & crate::ui::SHELPER_Q3DF_ACCEL, 0);
    assert_ne!(app.strafe_helper.flags & crate::ui::SHELPER_Q3DF_SPEED, 0);
    for id in [Id::AccelMeter, Id::StrafeGuide, Id::VelocityCompass, Id::GroundSpeed, Id::JumpHeight,
        Id::JumpDistance, Id::VerticalSpeed, Id::YawSpeed] {
        app.set_console_cvar(id.cvar_name(), "tl 17 -12 1.25").unwrap();
    }
    app.overlay = crate::ui::OverlayMode::HudEdit;
    let snapshot = app.ui_snapshot();
    let native = snapshot.speedometer.as_ref().unwrap();
    assert!(native.texts.is_empty());
    app.flush_config();
    let loaded = crate::config::load_client_presentation_settings(&path);
    for id in [Id::AccelMeter, Id::StrafeGuide, Id::VelocityCompass, Id::GroundSpeed, Id::JumpHeight,
        Id::JumpDistance, Id::VerticalSpeed, Id::YawSpeed] {
        let layout = loaded.hud_layout.element(id);
        assert_eq!(layout.offset, [17.0,-12.0], "{id:?}");
        assert_eq!(layout.scale, 1.25);
    }
    app.set_console_cvar("cg_strafeHelper", "0").unwrap();
    app.set_console_cvar("cg_speedometer", "0").unwrap();
    assert!(app.ui_snapshot().speedometer.is_none());
}

#[test]
fn occlusion_ab_cvars_fail_closed_on_unwired_gpu_modes() {
    use winit::platform::windows::EventLoopBuilderExtWindows;
    let event_loop = winit::event_loop::EventLoop::<crate::runtime::UserEvent>::with_user_event()
        .with_any_thread(true).build().unwrap();
    let fixture = Fixture::new();
    let mut app = App::new(fixture.0.clone(), None,
        crate::scene::MapSource::Bsp("mp/ffa3".into()), false,
        event_loop.create_proxy()).unwrap();
    for name in ["r_staticMd3Bvh", "r_fxOcclusion", "r_md3Occlusion", "r_occlusionStats"] {
        let definition = crate::app::cvar_registry().find(name).unwrap();
        assert!(!definition.flags.contains(crate::console::cvars::CvarFlags::ARCHIVE));
        assert_eq!(definition.entry.latch, crate::console::LatchScope::None);
        assert_eq!(app.console_cvar_value(name).as_deref(), Some("0"));
    }
    app.config_dirty = false;
    for name in ["r_staticMd3Bvh", "r_fxOcclusion", "r_occlusionStats"] {
        app.set_console_cvar(name, "1").unwrap();
        assert_eq!(app.console_cvar_value(name).as_deref(), Some("1"));
        app.set_console_cvar(name, "0").unwrap();
        assert_eq!(app.console_cvar_value(name).as_deref(), Some("0"));
        assert!(!app.config_dirty, "session-only switch must not dirty config");
    }
    assert_eq!(app.console_cvar_value("r_md3Occlusion").as_deref(), Some("0"));
    assert_eq!(app.console_cvar_value("r_fxOcclusion").as_deref(), Some("0"));
    for invalid in ["1", "2", "999", "-1", "nonsense"] {
        assert!(app.set_console_cvar("r_md3Occlusion", invalid).is_err());
    }
    for invalid in ["2", "3", "-1", "nonsense"] {
        assert!(app.set_console_cvar("r_fxOcclusion", invalid).is_err());
    }
    assert_eq!(app.console_cvar_value("r_md3Occlusion").as_deref(), Some("0"));
    assert_eq!(app.console_cvar_value("r_fxOcclusion").as_deref(), Some("0"));
}

#[test]
fn baked_local_controls_apply_live_and_round_trip_independently() {
    use winit::platform::windows::EventLoopBuilderExtWindows;
    let event_loop=winit::event_loop::EventLoop::<crate::runtime::UserEvent>::with_user_event().with_any_thread(true).build().unwrap();
    let fixture=Fixture::new();let path=fixture.0.join("DinurdoJK.cfg");fs::write(&path,"").unwrap();
    let mut app=App::new(fixture.0.clone(),None,crate::scene::MapSource::Bsp("mp/ffa3".into()),false,event_loop.create_proxy()).unwrap();
    let runtime_promotion=(app.video.promote_shader_lights,app.video.promote_shader_glow,app.video.promote_shader_additive,app.video.promote_shader_emissive,app.video.emissive_area_lights,app.video.promote_fx_lights);
    for (name,value) in [("r_bakedLocalLights","1"),("r_bakedLocalGlow","0"),("r_bakedLocalEmissive","0"),("r_bakedLocalAdditive","1"),("r_bakedLocalSurfacelight","1"),("r_bakedLocalFx","1")] {
        assert_eq!(crate::app::cvar_registry().find(name).unwrap().entry.latch,crate::console::LatchScope::None);
        app.set_console_cvar(name,value).unwrap();
    }
    assert!(app.video.baked_local_lights);assert_eq!(app.video.baked_local_sources,2|8|16);
    app.set_console_cvar("r_playerContactShadows","0").unwrap();
    assert!(!app.video.player_contact_shadows);
    assert!(crate::app::cvar_registry().find("r_bakedLocalShadows").is_none(), "master cvar was removed");
    assert_eq!(runtime_promotion,(app.video.promote_shader_lights,app.video.promote_shader_glow,app.video.promote_shader_additive,app.video.promote_shader_emissive,app.video.emissive_area_lights,app.video.promote_fx_lights));
    app.flush_config();let restored=crate::config::load_video_settings(&path);
    assert!(restored.baked_local_lights);assert_eq!(restored.baked_local_sources,2|8|16);
    assert!(!restored.player_contact_shadows);
    assert!(app.set_console_cvar("r_bakedLocalLights","banana").is_err());
    app.set_console_cvar("r_bakedLocalLights","0").unwrap();assert!(!app.video.baked_local_lights);assert_eq!(app.video.baked_local_sources,2|8|16);
    // Selecting a lamp source does not turn on experimental illumination.
    assert!(!app.video.baked_local_lights && app.video.baked_local_sources == 2|8|16);
    app.flush_config();let shadows_only=crate::config::load_video_settings(&path);
    assert!(!shadows_only.baked_local_lights && shadows_only.baked_local_sources == 2|8|16);
}
