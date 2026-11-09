//! HUD declarations shared by live App state and inactive video settings.
use crate::{
    app::App,
    console::{
        cvars::{CvarDef, CvarFlags, CvarType, CvarValue},
        LatchScope,
    },
    ui::VideoSettings,
};

// Each declaration supplies both bindings and both enumeration slices.
macro_rules! video_cvars {
    ($($video:ident, $app:ident, $name:literal, $default:literal, $range:literal,
       $description:literal, $kind:expr, $variant:ident, $field:ident, $convert:expr,
       |$state:ident| $changed:block;)*) => {
        $(pub(crate) static $video: CvarDef<VideoSettings> = CvarDef::new(
            $name, $default, $range, $description, $kind, CvarFlags::ARCHIVE, LatchScope::None,
            |settings| CvarValue::$variant(settings.$field.into()),
            |settings, value| {
                let CvarValue::$variant(value) = value else { unreachable!("validated cvar type") };
                settings.$field = ($convert)(value);
                Ok(())
            });
        pub(super) static $app: CvarDef<App> = $video.bind(
            |app| ($video.get)(&app.video),
            |$state, value| {
                ($video.set)(&mut $state.video, value)?;
                $changed
                Ok(())
            });)*
        pub(super) const VIDEO_CVARS: &[&CvarDef<VideoSettings>] = &[$(&$video),*];
    };
}

video_cvars! {
    DRAW_2D, APP_DRAW_2D, "cg_draw2D", "1", "0|1",
    "TaystJK: 0 hides gameplay 2D (HUD, scoreboard, crosshair, FPS, overlays) for clean screenshots; console, chat entry and menus remain usable.",
    CvarType::Bool, Bool, draw_2d, |value| value,
    |app| { app.mark_config_dirty(); app.publish_ui(); };

    DRAW_HUD, APP_DRAW_HUD, "cg_drawHud", "1", "0|1",
    "TaystJK: 0 hides the main health/shield/ammo/force status HUD; crosshair, movement tools, chat, timer and scoreboard remain.",
    CvarType::Bool, Bool, draw_hud, |value| value,
    |app| { app.mark_config_dirty(); app.publish_ui(); };

    TINT_HUD, APP_TINT_HUD, "cg_tintHud", "1", "0|1",
    "TaystJK/jaPRO team-color tint: native JK2/JKA status HUD red/blue; DinurdoJK-style panel chrome red/blue. Only team modes, not Siege.",
    CvarType::Bool, Bool, tint_hud, |value| value,
    |app| { app.mark_config_dirty(); app.publish_ui(); };

    HUD_STYLE, APP_HUD_STYLE, "cg_hudStyle", "0", "0|1",
    "Native Rust HUD style: 0 DinurdoJK panels, 1 Native (JK2 or JKA retail HUD based on active game).",
    CvarType::Int { min: 0, max: 1 }, Int, hud_style, |value: i32| value as u8,
    |app| { app.mark_config_dirty(); app.publish_ui(); };

    DRAW_FPS, APP_DRAW_FPS, "cg_drawFPS", "1", "0|1|2",
    "FPS overlay: 0 off, 1 simple FPS, 2 detailed diagnostics.",
    CvarType::Int { min: 0, max: 2 }, Int, draw_fps, |value: i32| value as u8,
    |app| { app.mark_config_dirty(); app.publish_ui(); };

    DRAW_TIMER, APP_DRAW_TIMER, "cg_drawTimer", "0", "0|1",
    "TaystJK/OpenJK match timer: elapsed time since level start as M:SS.",
    CvarType::Bool, Bool, draw_timer, |value| value,
    |app| { app.game_timer_shown = None; app.mark_config_dirty(); app.publish_transient_ui(); };
}
