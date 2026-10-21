use super::*;
use crate::cvar_compatibility::{assert_snapshot, digest, Fixture, INPUTS};
use std::fs;

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
    app.set_console_cvar("r_lakeSalt","1").unwrap();
    assert!(app.video.ocean);
    assert_eq!(app.video.ocean_settings.backend,3);
    let resolved=app.video.ocean_settings.resolved_for_gpu();
    assert!(!resolved.blue_water_adapter && !resolved.interaction);
    assert_eq!(resolved.blue_water_caustics_mode,2);
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
