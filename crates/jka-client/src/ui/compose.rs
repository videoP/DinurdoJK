//! Compose.
use crate::ui::{
    build_background_progress, build_center_print, build_chat_history, build_chat_input,
    build_console, build_crosshair_name, build_crosshair_vertices, build_demo_timeline,
    build_follow_indicator, build_force_select, build_fps_simple, build_game_timer, build_hud,
    build_lagometer, build_loading_screen, build_mini_scores, build_movement_keys, build_perf,
    build_prediction_debug, build_race_timer, build_reflection_debug_legend, build_scoreboard,
    build_speedometer, build_splash_background, build_strafe_helper, build_team_overlay,
    build_vote, draw_placed, HudElementId, OverlayMode, ProportionalFont, UiSnapshot, UiVertex,
};

pub fn build_vertices(
    ui: &UiSnapshot,
    width: u32,
    height: u32,
    small_font: Option<&ProportionalFont>,
    splash_size: Option<[u32; 2]>,
) -> Vec<UiVertex> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(16_384);
    if ui.startup_splash {
        build_splash_background(&mut out, width, height, splash_size);
        return out;
    }
    if let Some(loading) = &ui.loading {
        build_loading_screen(&mut out, loading, width, height, splash_size);
        // Keep the console usable during background map preparation/upload.
        // It is intentionally layered over the loading screen rather than
        // replacing it, so load progress remains visible behind the console.
        if ui.mode == OverlayMode::Console {
            build_console(&mut out, ui, width, height);
        }
        return out;
    }
    if matches!(
        ui.mode,
        OverlayMode::None
            | OverlayMode::Chat
            | OverlayMode::Vgs
            | OverlayMode::HudEdit
            | OverlayMode::CameraEdit
            | OverlayMode::MapEdit
    ) {
        build_scoreboard(&mut out, ui, small_font, width, height);
    }
    if ui.video.reflection_debug {
        build_reflection_debug_legend(&mut out, ui, width, height);
    }
    match ui.mode {
        OverlayMode::None | OverlayMode::Chat => {}
        OverlayMode::Console => build_console(&mut out, ui, width, height),
        // The Game/Video menus are drawn by egui; see `app::egui_menu`.
        OverlayMode::Video
        | OverlayMode::Game
        | OverlayMode::Vgs
        | OverlayMode::HudEdit
        | OverlayMode::CameraEdit
        | OverlayMode::MapEdit
        | OverlayMode::EntityGraph
        | OverlayMode::Trace
        | OverlayMode::StrafeTrails
        | OverlayMode::RaceGhosts => {}
    }
    // The trace result panel used to be drawn here from `ui.surface_inspector`
    // (`build_surface_inspector`, a static non-interactive readout). It's now
    // the clickable `OverlayMode::Trace` egui menu (`app::egui_trace_menu`);
    // `ui.surface_inspector` still mirrors the selected entry for Ctrl+C, but
    // nothing draws it on the HUD any more.
    if let Some(progress) = &ui.static_ao_progress {
        build_background_progress(&mut out, progress, width, height);
    }
    out
}

pub(in crate::ui) fn gameplay_hud_visible(ui: &UiSnapshot, width: u32, height: u32) -> bool {
    width != 0
        && height != 0
        && !ui.video.skip_ui
        && ui.loading.is_none()
        && matches!(
            ui.mode,
            OverlayMode::None
                | OverlayMode::Chat
                | OverlayMode::Vgs
                | OverlayMode::HudEdit
                | OverlayMode::CameraEdit
                | OverlayMode::MapEdit
        )
}

/// Stable gameplay HUD geometry. This changes when HUD values/settings change,
/// not merely because velocity/view input produced another movement sample.
pub fn build_dynamic_static_vertices(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    width: u32,
    height: u32,
) {
    if !gameplay_hud_visible(ui, width, height) {
        return;
    }
    build_hud(out, ui, width, height);
}

/// Hot movement-driven HUD geometry. Strafehelper and MovementKeys are the only
/// legacy-HUD pieces that need to follow every new input/simulation snapshot.
pub fn build_movement_vertices(out: &mut Vec<UiVertex>, ui: &UiSnapshot, width: u32, height: u32) {
    if !gameplay_hud_visible(ui, width, height) {
        return;
    }
    build_strafe_helper(out, ui, width, height);
    draw_placed(out, ui, HudElementId::MovementKeys, width, height, |out| {
        build_movement_keys(out, ui, width, height);
    });
}

/// Chat history and center-print alpha are periodic presentation changes, not
/// retained UI changes. Rebuild just this small text batch when their fade moves.
pub fn build_transient_vertices(
    out: &mut Vec<UiVertex>,
    ui: &UiSnapshot,
    small_font: Option<&ProportionalFont>,
    width: u32,
    height: u32,
) {
    if width == 0 || height == 0 || ui.video.skip_ui || ui.loading.is_some() {
        return;
    }
    let gameplay_overlay = matches!(
        ui.mode,
        OverlayMode::None
            | OverlayMode::Chat
            | OverlayMode::Vgs
            | OverlayMode::HudEdit
            | OverlayMode::CameraEdit
            | OverlayMode::MapEdit
    );
    if gameplay_hud_visible(ui, width, height) {
        // Colour and name follow the aim target, so they share the small batch
        // that is rebuilt when it changes rather than the stable HUD prefix.
        // Dynamic crosshair placement depends on the render thread's final
        // late-latched camera, so its geometry is appended in the view-dependent
        // UI tail. Static/smart-overridden crosshairs stay in this stable batch.
        if !ui.crosshair_target.dynamic {
            build_crosshair_vertices(out, ui, None, width, height);
        }
        draw_placed(out, ui, HudElementId::CrosshairName, width, height, |out| {
            build_crosshair_name(out, ui, small_font, width, height);
        });
        build_force_select(out, ui, small_font, width, height);
    }
    if gameplay_overlay {
        // Follows the snapshot's client every frame (CG_DrawFollow), so it lives
        // in the batch that is republished on change, not the retained UI.
        draw_placed(out, ui, HudElementId::Follow, width, height, |out| {
            build_follow_indicator(out, ui, small_font, width, height);
        });
        build_team_overlay(out, ui, small_font, width, height);
        build_race_timer(out, ui, small_font, width, height);
        build_speedometer(out, ui, small_font, width, height);
        build_lagometer(out, ui, small_font, width, height);
        draw_placed(out, ui, HudElementId::Vote, width, height, |out| {
            build_vote(out, ui, width, height);
        });
        draw_placed(out, ui, HudElementId::Chat, width, height, |out| {
            build_chat_history(out, ui, small_font, width, height);
        });
        draw_placed(out, ui, HudElementId::CenterPrint, width, height, |out| {
            build_center_print(out, ui, small_font, width, height);
        });
        build_demo_timeline(out, ui, width, height);
    }
    build_prediction_debug(out, ui, width, height);
    // FPS/perf is intentionally visible above menus too, matching the old
    // retained path and submit_ui_overlay ordering.
    draw_placed(out, ui, HudElementId::Fps, width, height, |out| {
        match ui.video.draw_fps {
            0 => {}
            1 => build_fps_simple(out, ui, width, height),
            _ => build_perf(out, ui, width, height),
        }
    });
    if gameplay_overlay {
        // TaystJK CG_DrawFPS/CG_DrawTimer share the same right edge. Reuse the
        // FPS HUD transform as well, so moving/scaling FPS cannot leave the
        // timer behind with a different effective right edge.
        draw_placed(out, ui, HudElementId::Fps, width, height, |out| {
            build_game_timer(out, ui, width, height);
        });
        build_mini_scores(out, ui, small_font, width, height);
    }
    if ui.mode == OverlayMode::Chat {
        // The input box is anchored to the oldest visible chat line, so keep
        // it in the same transient batch as chat history. This lets fades/new
        // messages reposition it without rebuilding the retained UI.
        draw_placed(out, ui, HudElementId::Chat, width, height, |out| {
            build_chat_input(out, ui, small_font, width, height);
        });
    }
}
