//! Compose.
use crate::ui::{
    build_background_progress, build_shader_compile_progress, build_center_print, build_chat_history, build_chat_input,
    build_console, build_console_notify, build_crosshair_name, build_crosshair_vertices,
    build_demo_timeline,
    build_follow_indicator, build_force_select, build_weapon_select, build_inventory_select, build_fps_simple, build_game_timer, build_hud,
    build_enemy_info, build_hud_score, build_lagometer, build_loading_screen, build_mini_scores, build_movement_keys, build_perf,
    build_siege_hud,
    build_prediction_debug, build_race_timer, build_reflection_debug_legend, build_scoreboard,
    build_speedometer, build_splash_background, build_strafe_helper, build_team_overlay,
    build_vote, build_extension_hud, draw_placed, HudElementId, OverlayMode, ProportionalFont, UiSnapshot, UiVertex,
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
        build_splash_background(&mut out, width, height, splash_size.filter(|_| ui.startup_splash_art));
        return out;
    }
    if let Some(loading) = &ui.loading {
        build_loading_screen(&mut out, loading, width, height, splash_size);
        if let Some(progress) = &ui.shader_compile_progress {
            build_shader_compile_progress(&mut out, progress, ui.shader_compile_stage.as_deref(), width, height);
        }
        // Keep the console usable during background map preparation/upload.
        // It is intentionally layered over the loading screen rather than
        // replacing it, so load progress remains visible behind the console.
        if ui.mode == OverlayMode::Console {
            build_console(&mut out, ui, width, height);
        }
        return out;
    }
    if ui.video.draw_2d && matches!(
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
    if ui.video.draw_2d && ui.video.reflection_debug {
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
        | OverlayMode::DevRadial
        | OverlayMode::ModelPicker
        | OverlayMode::BmodelPicker
        | OverlayMode::MapMods
        | OverlayMode::EmoteRadial
        | OverlayMode::StrafeTrails
        | OverlayMode::RaceGhosts
        | OverlayMode::Telemarks
        | OverlayMode::ShaderRemap => {}
    }
    // The trace result panel used to be drawn here from `ui.surface_inspector`
    // (`build_surface_inspector`, a static non-interactive readout). It's now
    // the clickable `OverlayMode::Trace` egui menu (`app::egui_trace_menu`);
    // `ui.surface_inspector` still mirrors the selected entry for Ctrl+C, but
    // nothing draws it on the HUD any more.
    if let Some(progress) = &ui.static_ao_progress {
        build_background_progress(&mut out, progress, width, height);
    }
    if let Some(progress) = &ui.shader_compile_progress {
        build_shader_compile_progress(&mut out, progress, ui.shader_compile_stage.as_deref(), width, height);
    }
    out
}

pub(crate) fn gameplay_hud_visible(ui: &UiSnapshot, width: u32, height: u32) -> bool {
    width != 0
        && height != 0
        && !ui.video.skip_ui
        && ui.video.draw_2d
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
    // Mirror TaystJK CG_DrawHUD: only the main status HUD is behind
    // cg_drawHud; movement helpers and other screen overlays are not.
    if ui.video.draw_hud {
        build_hud(out, ui, width, height);
        build_extension_hud(out, &ui.extension_hud, &ui.extension_images, width, height);
    }
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
    let gameplay_overlay = ui.video.draw_2d && matches!(
        ui.mode,
        OverlayMode::None
            | OverlayMode::Chat
            | OverlayMode::Vgs
            | OverlayMode::HudEdit
            | OverlayMode::CameraEdit
            | OverlayMode::MapEdit
    );
    if gameplay_overlay {
        if let Some(race)=&ui.studio_race {super::studio_race::draw(out,race,small_font,width,height);}
        for label in &ui.studio_world_labels {
            super::text::build_studio_label_vertices(out, label, small_font, width, height);
        }
    }
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
        build_weapon_select(out, ui, small_font, width, height);
        build_inventory_select(out, ui, small_font, width, height);
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
        draw_placed(out, ui, HudElementId::KillFeed, width, height, |out| {
            super::killfeed::build_killfeed(out, ui, small_font, width, height);
        });
        draw_placed(out, ui, HudElementId::Chat, width, height, |out| {
            build_chat_history(out, ui, small_font, width, height);
        });
        draw_placed(out, ui, HudElementId::CenterPrint, width, height, |out| {
            build_center_print(out, ui, small_font, width, height);
        });
        build_demo_timeline(out, ui, width, height);
    }
    // TaystJK Con_DrawNotify remains visible while KEYCATCH_MESSAGE is active,
    // but not under the solid console or UI/cgame/menu catchers.
    if ui.video.draw_2d && matches!(ui.mode, OverlayMode::None | OverlayMode::Chat) {
        build_console_notify(out, ui, width, height);
    }
    if ui.video.draw_2d {
        build_prediction_debug(out, ui, width, height);
        // FPS/perf may appear above menus, but is still CG_Draw2D content.
        draw_placed(out, ui, HudElementId::Fps, width, height, |out| {
            match ui.video.draw_fps {
                0 => {}
                1 => build_fps_simple(out, ui, width, height),
                _ => build_perf(out, ui, width, height),
            }
        });
    }
    if gameplay_overlay {
        // TaystJK CG_DrawFPS/CG_DrawTimer share the same right edge. Reuse the
        // FPS HUD transform as well, so moving/scaling FPS cannot leave the
        // timer behind with a different effective right edge.
        draw_placed(out, ui, HudElementId::Fps, width, height, |out| {
            build_game_timer(out, ui, width, height);
        });
        draw_placed(out, ui, HudElementId::EnemyInfo, width, height, |out| {
            build_enemy_info(out, ui, width, height);
        });
        // cg_drawScore is snapshot-dependent: keep it in the transient HUD
        // batch so score and team deltas update on score changes without
        // rebuilding the large static health/force HUD.
        if ui.video.draw_hud {
            draw_placed(out, ui, HudElementId::Score, width, height, |out| {
                build_hud_score(out, ui, width, height);
            });
        }
        draw_placed(out, ui, HudElementId::MiniScores, width, height, |out| {
            build_mini_scores(out, ui, small_font, width, height);
        });
        build_siege_hud(out, ui, width, height);
    }
    if ui.mode == OverlayMode::Chat {
        // TaystJK message entry is a console/notify field, not a child of the
        // cg_chatBox HUD element. Keep it outside draw_placed(Chat) so the HUD
        // editor's chat extent/position affects history only.
        build_chat_input(out, ui, small_font, width, height);
    }
}

#[cfg(test)]
mod draw2d_tests {
    use super::*;

    #[test]
    fn draw2d_gates_gameplay_but_not_engine_console_and_chat_entry() {
        let mut ui = UiSnapshot::default();
        assert!(gameplay_hud_visible(&ui, 1920, 1080));
        ui.video.draw_2d = false;
        assert!(!gameplay_hud_visible(&ui, 1920, 1080));
        assert!(build_vertices(&ui, 1920, 1080, None, None).is_empty());
        let mut stable = Vec::new();
        let mut movement = Vec::new();
        build_dynamic_static_vertices(&mut stable, &ui, 1920, 1080);
        build_movement_vertices(&mut movement, &ui, 1920, 1080);
        assert!(stable.is_empty());
        assert!(movement.is_empty());
        ui.mode = OverlayMode::Console;
        ui.console_input = "cg_draw2D 1".to_string();
        assert!(!build_vertices(&ui, 1920, 1080, None, None).is_empty());
        ui.mode = OverlayMode::Chat;
        ui.chat_input = "screenshot".to_string();
        let mut transient = Vec::new();
        build_transient_vertices(&mut transient, &ui, None, 1920, 1080);
        assert!(!transient.is_empty());
    }

    #[test]
    fn drawhud_only_gates_core_hud_not_entire_draw2d() {
        let mut ui = UiSnapshot::default();
        // The distinction is important: cg_drawHud must not disable the
        // crosshair, scoreboards or movement helpers in CG_Draw2D.
        ui.hud = Some(crate::ui::HudState {
            health: 100,
            max_health: 100,
            armor: 50,
            force_power: Some(75),
            force_power_max: 100,
            ammo: Some(30),
            ..Default::default()
        });
        let mut vertices = Vec::new();
        build_dynamic_static_vertices(&mut vertices, &ui, 1920, 1080);
        assert!(!vertices.is_empty());
        ui.video.draw_hud = false;
        assert!(gameplay_hud_visible(&ui, 1920, 1080));
        vertices.clear();
        build_dynamic_static_vertices(&mut vertices, &ui, 1920, 1080);
        assert!(vertices.is_empty()); // the core status HUD is gated
        ui.game_timer = Some("1:23".to_owned());
        build_transient_vertices(&mut vertices, &ui, None, 1920, 1080);
        assert!(!vertices.is_empty()); // unrelated 2D survives cg_drawHud 0
    }

    #[test]
    fn extension_replaced_native_elements_are_not_built() {
        use crate::extensions::{NativeHud, NativeHudMask};
        let mut ui = UiSnapshot::default();
        ui.hud = Some(crate::ui::HudState {
            health: 100, max_health: 100, armor: 50,
            force_power: Some(75), force_power_max: 100, ammo: Some(30),
            ..Default::default()
        });
        let count = |ui: &UiSnapshot| {
            let mut v = Vec::new();
            build_dynamic_static_vertices(&mut v, ui, 1920, 1080);
            v.len()
        };
        let all = count(&ui);
        ui.native_hidden = NativeHudMask::from_names(&["health".to_owned()]).unwrap();
        let without_health = count(&ui);
        assert!(without_health > 0 && without_health < all);
        ui.native_hidden = NativeHudMask::from_names(
            &NativeHud::ALL.iter().map(|e| e.name().to_owned()).collect::<Vec<_>>()).unwrap();
        assert_eq!(count(&ui), 0);
        // Under cg_drawHud 0 extension widgets are not drawn either, so the
        // native element is not considered replaced.
        ui.video.draw_hud = false;
        assert!(!ui.native_replaced(NativeHud::Speedometer));
    }
}
