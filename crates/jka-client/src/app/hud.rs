//! Hud.
use crate::app::{
    playerstate_vec3, scene, ui, App, Arc, ConsolePathLinkUi, ConsolePoint, ConsoleSearchMatch,
    ConsoleSelection, DemoViewMode, Duration, HashMap, HudState, Instant, JoinMode, MapLoadingBar,
    MapLoadingState, OverlayMode, PredictionDebugUi, SurfaceInspectorInfo, UiScoreboard,
    UiSnapshot, ET_PLAYER,
};

impl App {
    /// Snap only untouched/default HUD elements when opening the editor. This
    /// changes layout coordinates, not the underlying JKA stock draw routines.
    pub(in crate::app) fn align_hud_defaults_for_editor(&mut self) {
        let Some(window) = self.window.as_ref() else { return; };
        let size = window.inner_size();
        let context = ui::HudRectContext::new(
            self.movement_keys_hud, &self.video, &self.perf, self.threads.len(),
            &self.japro_cg, self.strafe_helper, self.video.draw_timer,
            [self.draw_score_x, self.draw_score_y], self.draw_scores,
        );
        if self.hud_layout.align_default_elements_to_grid(&context, size.width, size.height) {
            self.mark_config_dirty();
            self.publish_ui();
        }
    }

    /// Editor handles follow configuration, including transient components
    /// whose live data may currently be empty (for example jump measurements).
    pub(in crate::app) fn hud_edit_element_enabled(&self, id: ui::HudElementId) -> bool {
        use ui::HudElementId::*;
        if !self.video.draw_2d || self.video.skip_ui { return false; }
        if ui::movement_components::is_movement_component(id) {
            let choice = ui::movement_components::selected(id, self.strafe_helper.flags, self.japro_cg.speedometer.flags);
            return choice != 0 && !(id == StrafeGuide && choice == ui::SHELPER_CINEMATIC);
        }
        match id {
            Health | Shield | Ammo | Force => self.video.draw_hud,
            Score => self.video.draw_hud && self.draw_score != 0,
            MiniScores => self.draw_scores != 0,
            EnemyInfo => self.draw_enemy_info,
            MovementKeys => self.movement_keys_hud.mode != 0,
            Fps => self.video.draw_fps != 0,
            CrosshairName => self.crosshair.names != 0.0,
            RaceTimer => self.japro_cg.race_timer != 0,
            RaceStart => self.japro_cg.race_start,
            Lagometer => self.japro_cg.lagometer.mode != 0,
            SurfaceInspector => false,
            _ => true,
        }
    }

    pub(in crate::app) fn current_lagometer_ui(&self) -> Option<crate::lagometer::Ui> {
        if self.overlay != OverlayMode::HudEdit { return self.lagometer_ui.clone(); }
        if !self.hud_edit_element_enabled(ui::HudElementId::Lagometer) { return None; }
        let mut plot = crate::lagometer::Lagometer::default();
        for i in 0..96 {
            plot.add_frame_info(1000+i*20+8,1000+i*20);
            plot.add_snapshot_info(&crate::lagometer::SnapshotInfo {
                message_num:i, server_time:1000+i*20, command_time:1000+i*20-40,
                ping:35+i%12, flags:0,
            }, false, 50);
        }
        plot.draw(&self.japro_cg.lagometer, &crate::lagometer::DrawInput {
            width_ratio_coef:self.width_ratio_coef(), ..Default::default()
        })
    }

    pub(in crate::app) fn ui_snapshot(&self) -> UiSnapshot {
        // Keep the retained UI snapshot bounded even after a very long console session.
        // Full-screen 4K needs far fewer than 384 atlas-text rows, so this leaves
        // ample scroll context without cloning the entire 10k-line ring every keypress.
        let console_scroll = self.console_scroll.min(self.console_max_scroll());
        let layout = self.console_wrap_layout();
        let total_rows = layout.rows.len();
        let console_end = total_rows.saturating_sub(console_scroll);
        let console_start = console_end.saturating_sub(384);
        let visible_rows = &layout.rows[console_start..console_end];
        let console_lines: Vec<String> = visible_rows.iter().map(|row| {
            super::console_state::console_colored_fragment(
                &self.console_lines[row.line], row.start_col, row.end_col,
            )
        }).collect();
        let console_line_kinds: Vec<crate::logging::ConsoleLineKind> = visible_rows.iter()
            .map(|row| self.console_line_kinds.get(row.line).copied().unwrap_or_default())
            .collect();

        // Selection coordinates remain tied to original messages so a resize
        // never changes what a drag, double-click, or Ctrl+C selects.
        let console_selection = match (self.console_selection_anchor, self.console_selection_focus) {
            (Some(mut start), Some(mut end)) if !visible_rows.is_empty() => {
                if (start.line, start.col) > (end.line, end.col) {
                    std::mem::swap(&mut start, &mut end);
                }
                let first_row = layout.row_for_point(start, false);
                let last_row = layout.row_for_point(end, true);
                if last_row < console_start || first_row >= console_end || first_row > last_row {
                    None
                } else {
                    let begin_row = first_row.max(console_start);
                    let finish_row = last_row.min(console_end - 1);
                    let begin = layout.rows[begin_row];
                    let finish = layout.rows[finish_row];
                    Some(ConsoleSelection {
                        start_line: begin_row - console_start,
                        start_col: if begin_row == first_row {
                            start.col.saturating_sub(begin.start_col)
                        } else { 0 },
                        end_line: finish_row - console_start,
                        end_col: if finish_row == last_row {
                            end.col.saturating_sub(finish.start_col)
                        } else { finish.end_col - finish.start_col },
                    })
                }
            }
            _ => None,
        };

        // Translate search/link ranges into physical row-local spans. Matches
        // can straddle a wrap boundary without losing their highlight or link.
        let mut console_search_matches = Vec::new();
        let mut console_search_active_spans = Vec::new();
        let mut console_path_links = Vec::new();
        let active_search = self.console_search_index
            .and_then(|index| self.console_search_matches.get(index).copied());
        for (display_row, row) in visible_rows.iter().enumerate() {
            let start = row.start_col;
            let end = row.end_col;
            for hit in self.console_search_matches.iter().filter(|hit| hit.line == row.line) {
                let left = hit.start_col.max(start);
                let right = hit.end_col.min(end);
                if left < right {
                    let segment = ConsoleSearchMatch {
                        line: display_row, start_col: left - start, end_col: right - start,
                    };
                    console_search_matches.push(segment);
                    if active_search == Some(*hit) { console_search_active_spans.push(segment); }
                }
            }
            if let Some(links) = self.console_path_links.get(row.line) {
                for link in links {
                    let left = link.start_col.max(start);
                    let right = link.end_col.min(end);
                    if left < right {
                        console_path_links.push(ConsolePathLinkUi {
                            line: display_row, start_col: left - start, end_col: right - start,
                        });
                    }
                }
            }
        }
        let console_search_active = console_search_active_spans.first().copied();

        let (mut chat_lines, mut center_print) = self.transient_ui_snapshot();
        let console_notify_lines = self.console_notify_ui_snapshot();
        let hud = self.current_hud_state();
        let in_studio = self.front_end && self.frontend_page == super::FrontendPage::RenderStudio;
        let mut follow_name = if in_studio {None}else{self.current_follow_name()};
        let mut race_timer = self
            .game_session
            .as_ref()
            .and_then(|session| session.race_timer_ui.clone());
        let mut vote_line = self.vote_hud_line();
        let mut crosshair_target = self.crosshair_target.clone();
        let mut surface_inspector = self.surface_inspector.clone();
        let mut speedometer = self.speedometer_ui.clone();
        let scoreboard = self.visible_scoreboard();
        let scoreboard_focus_client = self.scoreboard_focus_client();
        if self.overlay == OverlayMode::HudEdit {
            self.hud_edit_transient_samples(
                &mut chat_lines,
                &mut center_print,
                &mut follow_name,
                &mut vote_line,
                &mut crosshair_target,
                &mut race_timer,
                &mut speedometer,
            );
            surface_inspector.get_or_insert_with(|| SurfaceInspectorInfo {
                kind: "WORLD SURFACE".to_owned(),
                title: "textures/sample/trace_inspector".to_owned(),
                summary: vec![
                    (
                        "SHADER".to_owned(),
                        "textures/sample/trace_inspector".to_owned(),
                    ),
                    ("DISTANCE".to_owned(), "128.0 units".to_owned()),
                    ("SURFACE".to_owned(), "12345".to_owned()),
                ],
                sections: Vec::new(),
                lines: Vec::new(),
                hit_entity_num: None,
                hit_inline_model: None,
            });
        }
        let (console_suggestions, console_suggest_total, console_suggest_hint) =
            self.console_suggest_snapshot();
        let console_suggest_selected = if self.console_suggest_picked {
            self.console_suggest_index
                .min(console_suggestions.len().saturating_sub(1))
        } else {
            0
        };
        let prediction_debug = self.prediction_debug_ui();
        let force_select = self.force_select_ui(Instant::now());
        let weapon_select = self.weapon_select_ui(Instant::now());
        let inventory_select = self.inventory_select_ui(Instant::now());
        // cg.predictedPlayerState, or the demo/follow equivalent. Never rely on
        // a local key bind: these values are also supplied in DM26 playback.
        let (zoom_scope, rocket_lock) = self.combat_overlay_state();

        let mut snapshot = UiSnapshot {
            studio_race: None,
            frontend_main_scene: self.front_end && matches!(self.frontend_page,
                crate::app::FrontendPage::Main | crate::app::FrontendPage::Play
                | crate::app::FrontendPage::DeveloperTools),
            mode: self.overlay,
            setup_selected: self.setup_selected,
            console_input: self.console_input.clone(),
            console_cursor: self.console_cursor,
            console_status: self.console_status.clone(),
            console_lines,
            console_line_kinds,
            console_scroll: 0,
            console_size: self.console_size,
            console_scale: self.console_scale,
            console_selection,
            console_search_open: self.console_search_open,
            console_search_query: self.console_search_input.clone(),
            console_search_total: self.console_search_matches.len(),
            console_search_index: self.console_search_index,
            console_search_matches,
            console_search_active,
            console_search_active_spans,
            console_path_links,
            console_total_lines: total_rows,
            console_scrolled: console_scroll,
            console_suggest_enabled: self.console_suggest,
            console_suggestions,
            console_suggest_total,
            console_suggest_selected,
            console_suggest_hint,
            console_help_hover: self.console_help_hover && self.overlay == OverlayMode::Console,
            chat_mode: self.chat_mode,
            chat_input: self.chat_input.clone(),
            chat_lines,
            console_notify_lines,
            center_print,
            follow_name,
            game_timer: self.game_timer_text(),
            mini_scores: self.mini_scores_ui(),
            hud_score: self.hud_score_ui(),
            enemy_info: self.enemy_info_ui(),
            siege_hud: self.siege_hud_ui(),
            race_timer,
            race_components: [self.japro_cg.race_timer != 0, self.japro_cg.race_start],
            vote_line,
            scoreboard,
            scoreboard_focus_client,
            demo_timeline: self.demo_timeline_ui(Instant::now()),
            prediction_debug,
            hud,
            team_overlay: self.team_overlay_ui(),
            hud_layout: self.hud_layout,
            studio_hud_virtual_layout: false,
            studio_world_labels: Vec::new(),
            studio_crosshair_name: false,
            extension_hud: self.extensions.placed_widgets(),
            native_hidden: self.extensions.native_hidden,
            extension_images: self.extensions.hud_images.clone(),
            crosshair: if in_studio || (self.model_placement.active && self.overlay == OverlayMode::None) {
                crate::ui::CrosshairSettings { style: 0, ..self.crosshair }
            } else { self.crosshair },
            crosshair_target,
            force_select,
            weapon_select,
            inventory_select,
            zoom_scope,
            rocket_lock,
            icon_atlas_uvs: Vec::new(),
            movement_keys: self.movement_keys_hud,
            strafe_helper: if in_studio {
                let mut helper=self.strafe_helper;
                helper.flags &= !crate::ui::SHELPER_CROSSHAIR;
                helper
            }else{self.strafe_helper},
            movement_hud: self.current_movement_hud_state(),
            speedometer,
            speedometer_settings: self.japro_cg.speedometer,
            lagometer: self.current_lagometer_ui(),
            lagometer_settings: self.japro_cg.lagometer,
            video: self.video,
            perf: self.perf,
            threads: self.threads.clone(),
            surface_inspector,
            startup_splash: !self.startup_first_frame_seen,
            startup_splash_art: !self.front_end,
            // Studio draws its own progress modal; the native fullscreen
            // loading HUD must never replace the editing canvas.
            loading: self.loading.as_ref().map(MapLoadingState::ui),
            shader_compile_progress: None, // Render thread owns worker progress.
            shader_compile_stage: None,
            static_ao_progress: self
                .static_ao_progress
                .map(|(completed, total)| MapLoadingBar {
                    label: "BAKED AO",
                    completed,
                    total,
                    skipped: false,
                }),
        };
        if self.streamer_preview_local {
            crate::streamer::redact_stream_snapshot(&mut snapshot, self.streamer);
        }
        snapshot
    }

    /// Owned, independently filtered presentation for the eventual OBS output.
    /// Consumers must never draw the unfiltered local snapshot into that output.
    #[allow(dead_code)] // OBS surface integration will consume this snapshot.
    pub(in crate::app) fn streamer_ui_snapshot(&self) -> UiSnapshot {
        let mut stream = self.ui_snapshot();
        crate::streamer::redact_stream_snapshot(&mut stream, self.streamer);
        stream
    }

    /// HUD Edit Mode: fill the normally empty transient readouts with sample
    /// content so there is something to grab and to see move. Shared by the full
    /// snapshot and the transient republish so neither wipes the other's samples.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::app) fn hud_edit_transient_samples(
        &self,
        chat_lines: &mut Vec<ui::UiChatLine>,
        center_print: &mut Option<ui::UiCenterPrint>,
        follow_name: &mut Option<String>,
        vote_line: &mut Option<String>,
        crosshair_target: &mut ui::UiCrosshairTarget,
        race_timer: &mut Option<crate::japro_cg::RaceTimerUi>,
        speedometer: &mut Option<crate::speedometer::Ui>,
    ) {
        if chat_lines.is_empty() {
            *chat_lines = [
                "^7Player^7: chat messages appear here",
                "^7Another^7: drag this block anywhere",
            ]
            .map(|text| ui::UiChatLine {
                text: text.to_owned(),
                team: false,
                alpha: 1.0,
            })
            .into();
        }
        center_print.get_or_insert_with(|| ui::UiCenterPrint {
            text: "Center print text".to_owned(),
            alpha: 1.0,
            y_fraction: 0.30,
        });
        follow_name.get_or_insert_with(|| "Followed Player".to_owned());
        vote_line.get_or_insert_with(|| "Vote (30): map ffa_bespin  Yes: 1  No: 0".to_owned());
        if !self.hud_edit_element_enabled(ui::HudElementId::CrosshairName) {
            crosshair_target.name = None;
        } else if crosshair_target.name.is_none() {
            crosshair_target.name = Some(ui::UiCrosshairName {
                text: "Target Player".to_owned(),
                color: [1.0, 1.0, 1.0],
                alpha: 1.0,
            });
        }
        *race_timer = (self.japro_cg.race_timer != 0 || self.japro_cg.race_start).then(|| crate::japro_cg::RaceTimerUi {
            timer_text: if self.japro_cg.race_timer != 0 {"0:12.3\nMax: 640\nAvg: 512".to_owned()} else {String::new()},
            timer_x: self.japro_cg.race_timer_x,
            timer_y: self.japro_cg.race_timer_y,
            size: self.japro_cg.race_timer_size,
            start_text: if self.japro_cg.race_start {"Start: 420".to_owned()} else {String::new()},
            start_x: self.japro_cg.race_start_x,
            start_y: self.japro_cg.race_start_y,
            start_color: [1.0; 3],
        });
        let mut settings = self.japro_cg.speedometer;
        let mut helper = self.strafe_helper.flags;
        ui::movement_components::normalize(&mut helper, &mut settings.flags);
        *speedometer = (settings.flags & ui::movement_components::NATIVE_READOUTS != 0)
            .then(|| crate::speedometer::preview_with_context(&settings, self.japro_cg.lagometer, self.width_ratio_coef()));
    }

    pub(in crate::app) fn prediction_debug_ui(&self) -> Option<PredictionDebugUi> {
        if !self.network.prediction_debug && !self.network.prediction_miss_highlight {
            return None;
        }

        if self
            .game_session
            .as_ref()
            .is_some_and(|session| session.local)
        {
            let mut lines = vec!["SOLO GAME: local physics / snapshot presentation".to_owned()];
            if let Some(server) = self.local_server.as_ref() {
                let view = server.view();
                lines.push(format!(
                    "physics t={} ground={} z={:.3} vZ={:.3} legs={} torso={}",
                    view.command_time,
                    view.ground_entity,
                    view.origin[2],
                    view.velocity[2],
                    view.legs_anim,
                    view.torso_anim,
                ));
            }
            if let Some(session) = self.game_session.as_ref() {
                if let Some(entity) = session.audio_followed_entity.as_ref() {
                    lines.push(format!(
                        "presentation ground={} z={:.3} legs={} torso={}",
                        entity.state.field_i32("groundEntityNum").unwrap_or(1023),
                        entity.origin[2],
                        entity.state.field_i32("legsAnim").unwrap_or(0),
                        entity.state.field_i32("torsoAnim").unwrap_or(0),
                    ));
                }
                if let Some(anim) = session.player_presenter.viewer_anim_debug() {
                    lines.push(format!(
                        "pose t={} call={} legs frame={}/{} blend={:.3} torso frame={}/{} blend={:.3}",
                        anim.pose_time, anim.call_time, anim.legs_frame, anim.legs_old_frame,
                        anim.legs_backlerp, anim.torso_frame, anim.torso_old_frame, anim.torso_backlerp,
                    ));
                }
            }
            lines.push(
                "ground IDs: 1022=world, 1023=none; online ground traces unavailable here"
                    .to_owned(),
            );
            return Some(PredictionDebugUi {
                show_panel: self.network.prediction_debug,
                flash: false,
                threshold: self.network.prediction_miss_threshold,
                last_miss: None,
                lines,
            });
        }

        let frame = self.predictor.prediction_debug_frame();
        let miss = self.predictor.latest_prediction_miss();
        let threshold = self.network.prediction_miss_threshold;
        let flash = self.network.prediction_miss_highlight
            && miss.is_some_and(|miss| {
                miss.length >= threshold && miss.detected_at.elapsed() <= Duration::from_millis(350)
            });
        let mut lines = Vec::new();

        if let Some(frame) = frame {
            let view_error_len = frame.view_error.iter().map(|v| v * v).sum::<f32>().sqrt();
            lines.push(format!(
                "NOW t={} display o=[{:.2} {:.2} {:.2}] vZ={:.2} ground={} pm={} flags={:#x} scale={}",
                frame.server_time,
                frame.display.origin[0], frame.display.origin[1], frame.display.origin[2],
                frame.display.velocity[2], frame.display.ground_entity, frame.display.pm_type,
                frame.display.pm_flags, frame.display.model_scale,
            ));
            lines.push(format!(
                "view correction=[{:.2} {:.2} {:.2}] |corr|={:.2}  committed ground={} legs={} torso={} inAir={}",
                frame.view_error[0], frame.view_error[1], frame.view_error[2], view_error_len,
                frame.committed.ground_entity, frame.committed.legs_anim,
                frame.committed.torso_anim, frame.committed.in_air_anim,
            ));
            if let Some(trace) = frame.ground_trace {
                lines.push(format!(
                    "groundTrace .25: frac={:.4} ent={} ss={} as={} hitZ={:.2} n=[{:.2} {:.2} {:.2}]",
                    trace.fraction, trace.entity, u8::from(trace.start_solid), u8::from(trace.all_solid),
                    trace.hit_end[2], trace.normal[0], trace.normal[1], trace.normal[2],
                ));
            }
            if let Some(trace) = frame.ground_probe {
                lines.push(format!(
                    "groundProbe 64: frac={:.4} ent={} ss={} as={} hitZ={:.2} nZ={:.2}",
                    trace.fraction,
                    trace.entity,
                    u8::from(trace.start_solid),
                    u8::from(trace.all_solid),
                    trace.hit_end[2],
                    trace.normal[2],
                ));
            }
        } else {
            lines.push(
                "prediction frame unavailable (cg_noPredict/follow/no live predictor)".to_owned(),
            );
        }

        if let Some(miss) = miss {
            lines.push(format!(
                "MISS #{} age={}ms len={:.3} delta=[{:.3} {:.3} {:.3}] cmd={}",
                miss.sequence,
                miss.detected_at.elapsed().as_millis(),
                miss.length,
                miss.delta[0],
                miss.delta[1],
                miss.delta[2],
                miss.command_time,
            ));
            lines.push(format!(
                "pred o=[{:.2} {:.2} {:.2}] v=[{:.2} {:.2} {:.2}] ground={} pm={} legs={} scale={}",
                miss.predicted.origin[0],
                miss.predicted.origin[1],
                miss.predicted.origin[2],
                miss.predicted.velocity[0],
                miss.predicted.velocity[1],
                miss.predicted.velocity[2],
                miss.predicted.ground_entity,
                miss.predicted.pm_type,
                miss.predicted.legs_anim,
                miss.predicted.model_scale,
            ));
            lines.push(format!(
                "srv  o=[{:.2} {:.2} {:.2}] v=[{:.2} {:.2} {:.2}] ground={} pm={} legs={} scale={}",
                miss.server.origin[0],
                miss.server.origin[1],
                miss.server.origin[2],
                miss.server.velocity[0],
                miss.server.velocity[1],
                miss.server.velocity[2],
                miss.server.ground_entity,
                miss.server.pm_type,
                miss.server.legs_anim,
                miss.server.model_scale,
            ));
            let trace_line = |label: &str, trace: crate::net::PredictionTraceDebug| {
                format!(
                    "{label}: {:.2}->{:.2} frac={:.4} hitZ={:.2} ent={} ss={} as={} nZ={:.2}",
                    trace.start[2],
                    trace.end[2],
                    trace.fraction,
                    trace.hit_end[2],
                    trace.entity,
                    u8::from(trace.start_solid),
                    u8::from(trace.all_solid),
                    trace.normal[2],
                )
            };
            if let Some(trace) = miss.previous_ground_trace {
                lines.push(trace_line("pre-miss ground .25", trace));
            }
            if let Some(trace) = miss.previous_ground_probe {
                lines.push(trace_line("pre-miss probe 64", trace));
            }
            if let Some(trace) = miss.replay_ground_trace {
                lines.push(trace_line("replay ground .25", trace));
            }
            if let Some(trace) = miss.replay_ground_probe {
                lines.push(trace_line("replay probe 64", trace));
            }
        }

        Some(PredictionDebugUi {
            show_panel: self.network.prediction_debug,
            flash,
            threshold,
            last_miss: miss.map(|miss| miss.length),
            lines,
        })
    }

    /// TaystJK CG_ScoresDown_f: request a `score` only on a fresh +scores press
    /// when the cached request is at least two seconds old. Do NOT call this
    /// periodically while the scoreboard is open or after death/intermission:
    /// servers may share flood protection between `score` and `say`, causing
    /// legitimate chat commands to be silently discarded.
    pub(in crate::app) fn request_scores_if_due(&mut self, clear_stale: bool) {
        let active = self
            .net
            .as_ref()
            .is_some_and(|net| net.state() == jka_protocol::session::ConnectionState::Active);
        if !active {
            return;
        }
        let now = Instant::now();
        let due = self
            .last_scores_request
            .is_none_or(|last| now.saturating_duration_since(last) >= Duration::from_secs(2));
        if !due {
            return;
        }
        if clear_stale {
            self.scoreboard = None;
            self.mark_companion_dirty();
        }
        self.last_scores_request = Some(now);
        if let Some(net) = self.net.as_mut() {
            if let Err(error) = net.session_mut().add_reliable_command(b"score", false) {
                self.push_console_line(format!("^1score request failed: {error}"));
            }
        }
    }

    /// Refresh a demo scoreboard from the pre-indexed reliable score timeline.
    /// Returns true when the active session is a demo (even if the visible
    /// board contents did not change), so callers know not to issue a live
    /// `score` request.
    pub(in crate::app) fn refresh_demo_scoreboard(&mut self, now: Instant) -> bool {
        let Some(session) = self.game_session.as_ref().filter(|session| !session.live) else {
            return false;
        };
        let Some(board) = session.demo_scoreboard_ui(now, self.score_deaths) else {
            return true;
        };
        if self.scoreboard.as_ref() != Some(&board) {
            self.scoreboard = Some(board);
            self.mark_companion_dirty();
            if self.scores_showing {
                self.publish_ui();
            }
        }
        true
    }

    /// cls.state >= CA_CONNECTED on a live server.
    pub(in crate::app) fn live_connected(&self) -> bool {
        self.net
            .as_ref()
            .is_some_and(|net| net.state() >= jka_protocol::session::ConnectionState::Connected)
    }

    /// `sv_hostname` of the server currently joined, live or local.
    pub(in crate::app) fn current_server_hostname(&self) -> Option<String> {
        let info: &[u8] = if let Some(net) = self.net.as_ref() {
            net.session()
                .decoder()
                .configstrings
                .get(&crate::cgame::CS_SERVERINFO)
                .map(Vec::as_slice)
                .unwrap_or_default()
        } else if let Some(server) = self.local_server.as_ref() {
            server
                .configstrings()
                .get(&crate::cgame::CS_SERVERINFO)
                .map(Vec::as_slice)
                .unwrap_or_default()
        } else {
            return None;
        };
        jka_protocol::commands::info_value(info, b"sv_hostname")
            .map(|name| String::from_utf8_lossy(name).into_owned())
            .filter(|name| !name.is_empty())
    }

    pub(in crate::app) fn active_fs_game_name(&self) -> String {
        self.game
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "base".to_owned())
    }

    /// Start a lazy live-server chat-log session. No file is created until the
    /// first CGame chat notice arrives.
    pub(in crate::app) fn begin_chat_log_for_current_connection(&self) {
        if !self.chat_log_enabled {
            return;
        }
        let Some(net) = self.net.as_ref() else { return };
        let started_unix_ms = crate::chat_log::unix_ms_now().saturating_sub(
            net.connection_started()
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
        );
        let hostname = self
            .current_server_hostname()
            .unwrap_or_else(|| net.server_name.clone());
        let map_name = self
            .game_session
            .as_ref()
            .filter(|session| session.live && !session.local)
            .and_then(|session| session.map_name.clone())
            .unwrap_or_default();
        self.chat_log.begin(crate::chat_log::SessionMetadata {
            directory: self.game_write_path("chatlogs"),
            started_unix_ms,
            player_name: self.network.name.clone(),
            server_address: net.session().server(),
            hostname: crate::logging::strip_jka_colors(&hostname),
            fs_game: self.active_fs_game_name(),
            map_name,
        });
    }

    pub(in crate::app) fn update_chat_log_live_metadata(&self, map_name: &str) {
        if !self.chat_log_enabled || self.net.is_none() {
            return;
        }
        let hostname = self.current_server_hostname().unwrap_or_else(|| {
            self.net
                .as_ref()
                .map(|net| net.server_name.clone())
                .unwrap_or_default()
        });
        self.chat_log.update(crate::chat_log::SessionUpdate {
            directory: self.game_write_path("chatlogs"),
            hostname: crate::logging::strip_jka_colors(&hostname),
            fs_game: self.active_fs_game_name(),
            map_name: map_name.to_owned(),
        });
    }

    /// TaystJK/OpenJK CG_DrawTimer core: cg.time - cgs.levelStartTime,
    /// formatted without the optional millisecond extension.
    pub(in crate::app) fn game_timer_text(&self) -> Option<String> {
        if !self.video.draw_timer {
            return None;
        }
        let session = self.game_session.as_ref()?;
        let server_time = session.current_snapshot.as_ref()?.server_time;
        let elapsed_ms = server_time
            .saturating_sub(session.client_game.match_limits().level_start_time)
            .max(0);
        let seconds = elapsed_ms / 1000;
        Some(format!("{}:{:02}", seconds / 60, seconds % 60))
    }


    /// TaystJK CG_DrawHUD score string. The disabled FFA score-bias branch
    /// (`else if (0 && gametype < GT_TEAM)`) is intentionally not reinstated.
    pub(in crate::app) fn hud_score_ui(&self) -> Option<ui::UiHudScore> {
        const GT_DUEL: i32 = 3;
        const GT_POWERDUEL: i32 = 4;
        const GT_TEAM: i32 = 6;
        const GT_SIEGE: i32 = 7;
        if self.overlay == OverlayMode::HudEdit && self.draw_score != 0 && self.video.draw_hud {
            // The HUD editor needs something to drag when no score packet has
            // arrived (or when the spectator/player cannot normally draw it).
            // This placeholder is editor-only; it never escapes into gameplay.
            return Some(ui::UiHudScore {
                lines: vec!["Score: 0".into(), "Team: 0 (+0)".into()],
                offset: [self.draw_score_x, self.draw_score_y],
            });
        }
        const TEAM_RED: i32 = 1;
        let session = self.game_session.as_ref()?;
        let snap = session.current_snapshot.as_ref()?;
        let ps = session.presented_player_state().unwrap_or(&snap.player_state);
        if self.draw_score <= 0 || !self.video.draw_hud
            || ps.field_i32("pm_type") == Some(2) // PM_SPECTATOR
        { return None; }
        let game = &session.client_game;
        let gametype = game.gametype();
        if gametype == GT_POWERDUEL || (self.active_server_mod() == crate::net::mod_support::ServerMod::Japro
            && ps.stats[crate::japro_cg::STAT_RACEMODE] != 0)
        { return None; }
        // Tayst uses the snapshot's PERS_SCORE even while following.
        let score = snap.player_state.persistant[0];
        let limits = game.match_limits();
        let client = usize::try_from(snap.player_state.field_i32("clientNum")?).ok()?;
        let mut lines = Vec::with_capacity(2);
        if gametype == GT_DUEL && limits.fraglimit > 0 {
            if limits.fraglimit > 1 {
                lines.push(format!("Score: {score}/{}", limits.fraglimit));
            } else {
                let (wins, losses) = game.client_duel_record(client)?;
                lines.push(format!("{wins}/{losses}"));
            }
        } else {
            lines.push(format!("Score: {score}"));
            if self.draw_score > 1 && gametype >= GT_TEAM && gametype != GT_SIEGE {
                let team = game.client_info(client, &session.siege_classes)?.team;
                let own = if team == TEAM_RED { limits.scores1 } else { limits.scores2 };
                let other = if team == TEAM_RED { limits.scores2 } else { limits.scores1 };
                let bias = own - other;
                let formatted = if bias > 0 { format!("{own} ({bias:+})") }
                    else if bias < 0 { format!("{own}  ({bias})") }
                    else { format!("{own}      ") };
                lines.push(format!("Team: {formatted}"));
            }
        }
        Some(ui::UiHudScore {
            lines,
            offset: [self.draw_score_x, self.draw_score_y],
        })
    }

    /// TaystJK CG_DrawEnemyInfo. This deliberately chooses the duel opponent,
    /// explicit duelIndex, Jedi Master or protocol duelWinner rather than the
    /// recently attacking player (disabled in TaystJK itself).
    pub(in crate::app) fn enemy_info_ui(&self) -> Option<ui::UiEnemyInfo> {
        if self.overlay == OverlayMode::HudEdit && self.draw_enemy_info {
            return Some(ui::UiEnemyInfo {
                icon_paths: vec!["models/players/kyle/icon_default".into()],
                title: "Dueling".into(), name: Some("^7OPPONENT".into()),
                detail: Some("Score: 0/10".into()), health: Some(100),
            });
        }
        const GT_DUEL: i32 = 3;
        const GT_POWERDUEL: i32 = 4;
        const GT_JEDIMASTER: i32 = 2;
        const TEAM_SPECTATOR: i32 = 3;
        const PMF_FOLLOW: i32 = 4096;
        let session = self.game_session.as_ref()?;
        let snap = session.current_snapshot.as_ref()?;
        let game = &session.client_game;
        let ps = session.presented_player_state().unwrap_or(&snap.player_state);
        let snapshot_ps = &snap.player_state;
        let snapshot_client = usize::try_from(snapshot_ps.field_i32("clientNum")?).ok()?;
        if !self.draw_enemy_info || ps.stats[0] <= 0 || game.gametype() == GT_POWERDUEL
            || (self.active_server_mod() == crate::net::mod_support::ServerMod::Japro
                && ps.stats[crate::japro_cg::STAT_RACEMODE] != 0)
        { return None; }
        // CG_DrawEnemyInfo suppresses the stock panel while cg_specHud is
        // active for a spectator/follow view. The native minimap isn't enabled
        // by this client, so there is no additional cg_drawRadar override.
        if self.japro_cg.spec_hud && (session.live && self.live_spec_hud_eligible()
            || !session.live && (snapshot_ps.persistant[3] == TEAM_SPECTATOR
                || snapshot_ps.field_i32("pm_flags").unwrap_or(0) & PMF_FOLLOW != 0))
        { return None; }
        let viewer = game.client_info(snapshot_client, &session.siege_classes)?;
        let (target, title) = if game.gametype() == GT_JEDIMASTER {
            let master = game.jedi_master();
            if master < 0 {
                return Some(ui::UiEnemyInfo {
                    icon_paths: vec!["gfx/hud/w_icon_lightsaber".to_owned()],
                    title: "Get Saber!".into(), name: None, detail: None, health: None,
                });
            }
            (master, "Jedi Master".to_owned())
        } else if snapshot_ps.field_i32("duelInProgress").unwrap_or(0) != 0 {
            (snapshot_ps.field_i32("duelIndex")?, "Dueling".to_owned())
        } else if game.gametype() == GT_DUEL && viewer.team != TEAM_SPECTATOR {
            let duelists = game.duelists();
            let target = if Some(duelists[0]) == snapshot_ps.field_i32("clientNum") { duelists[1] }
                else if Some(duelists[1]) == snapshot_ps.field_i32("clientNum") { duelists[0] }
                else if Some(duelists[2]) == snapshot_ps.field_i32("clientNum") { duelists[0] }
                else { return None; };
            (target, "Dueling".to_owned())
        } else {
            (game.duel_winner(), format!("Leader: {}", game.match_limits().scores1))
        };
        let client = usize::try_from(target).ok().filter(|&value| value < 32)?;
        let info = game.client_info(client, &session.siege_classes)?;
        let limits = game.match_limits();
        let detail = if (game.gametype() == GT_DUEL || game.gametype() == GT_POWERDUEL)
            && viewer.team != TEAM_SPECTATOR {
            let score = self.scoreboard.as_ref()
                .and_then(|board| board.entries.iter().find(|entry| entry.client == target))
                .map(|entry| entry.score);
            if limits.fraglimit > 1 {
                score.map(|score| format!("Score: {score}/{}", limits.fraglimit))
            } else if limits.fraglimit == 1 {
                game.client_duel_record(client).map(|(wins, losses)| format!("Win/Loss: {wins}/{losses}"))
            } else {
                score.map(|score| format!("Score: {score}"))
            }
        } else { None };
        let duelists = game.duelists();
        let healths = game.duelist_healths();
        // g_showDuelHealths=2 draws the small health bar under the upper-right
        // enemy panel.  No value is invented when the wire field is absent.
        let show_health = game.serverinfo_value(b"g_showDuelHealths")
            .and_then(|value| value.parse::<i32>().ok()).unwrap_or(0) >= 2;
        let health = if show_health && target == duelists[0] { (healths[0] >= 0).then_some(healths[0]) }
            else if show_health && target == duelists[1] { (healths[1] >= 0).then_some(healths[1]) }
            else { None };
        Some(ui::UiEnemyInfo {
            icon_paths: vec![format!(
                "@team-model-icon\t{}\t{}\t{}\t{}\t{}",
                info.model_name, info.skin_name, info.team, info.gametype, u8::from(info.jedi_v_merc)
            )],
            title,
            name: Some(info.name),
            detail,
            health,
        })
    }

    /// TaystJK `cg_drawScores`. Modes 1/2 are the stock/jaPRO upper-right
    /// mini scoreboard; mode 3 is Tayst's centred team/duel HUD. Team scores
    /// come from CS_SCORES1/2, while duel scores require the `scores` command
    /// because Tayst reads each duelist's clientInfo score.
    pub(in crate::app) fn mini_scores_ui(&self) -> Option<ui::UiMiniScores> {
        const GT_DUEL: i32 = 3;
        const GT_TEAM: i32 = 6;
        const GT_SIEGE: i32 = 7;
        const SCORE_NOT_PRESENT: i32 = -9999;

        let mode = self.draw_scores;
        if mode == 0 {
            return None;
        }
        let session = self.game_session.as_ref()?;
        let gametype = session.client_game.gametype();
        let limits = session.client_game.match_limits();
        let score = |value: i32| (value != SCORE_NOT_PRESENT).then_some(value);

        if mode != 3 {
            // CG_DrawMiniScoreboard explicitly skips Siege.
            return (gametype >= GT_TEAM && gametype != GT_SIEGE).then_some(
                ui::UiMiniScores::Team {
                    mode,
                    red: score(limits.scores1),
                    blue: score(limits.scores2),
                },
            );
        }

        // CG_DrawTaystHUD calls CG_DrawTeamHUD for every team gametype,
        // including Siege; this is intentionally different from modes 1/2.
        if gametype >= GT_TEAM {
            return Some(ui::UiMiniScores::Team {
                mode,
                red: score(limits.scores1),
                blue: score(limits.scores2),
            });
        }
        if gametype != GT_DUEL {
            return None;
        }

        // CG_DrawDuelHUD requires a live snapshot, a living viewed player and
        // suppresses the jaPRO race-mode overlay. GT_POWERDUEL is excluded by
        // the GT_DUEL gate above, as in TaystJK.
        let snapshot_ps = &session.current_snapshot.as_ref()?.player_state;
        let ps = session.presented_player_state().unwrap_or(snapshot_ps);
        if ps.stats[0] <= 0 {
            return None;
        }
        let server_mod = session
            .client_game
            .configstring(crate::cgame::CS_SERVERINFO)
            .map(crate::net::mod_support::ServerMod::detect)
            .unwrap_or(crate::net::mod_support::ServerMod::Unknown);
        if server_mod == crate::net::mod_support::ServerMod::Japro
            && ps.stats[crate::japro_cg::STAT_RACEMODE] != 0
        {
            return None;
        }

        let duelists = session.client_game.duelists();
        let d1 = usize::try_from(duelists[0]).ok()?;
        let d2 = usize::try_from(duelists[1]).ok()?;
        let clean_name = |name: String| {
            crate::logging::strip_jka_colors(&name)
                .chars()
                .take(18)
                .collect::<String>()
        };
        let d1_info = session
            .client_game
            .client_info(d1, &session.siege_classes)?;
        let d2_info = session
            .client_game
            .client_info(d2, &session.siege_classes)?;
        let d1_name = clean_name(d1_info.name.clone());
        let d2_name = clean_name(d2_info.name.clone());
        let icon_key = |info: &crate::cgame::ClientInfo| {
            format!(
                "@team-model-icon\t{}\t{}\t{}\t{}\t{}\ngfx/2d/defer",
                info.model_name,
                info.skin_name,
                info.team,
                info.gametype,
                u8::from(info.jedi_v_merc),
            )
        };
        let icon_paths = vec![icon_key(&d1_info), icon_key(&d2_info)];
        let client_score = |client: i32| {
            self.scoreboard
                .as_ref()
                .and_then(|board| board.entries.iter().find(|entry| entry.client == client))
                .map(|entry| entry.score)
                .filter(|value| *value != SCORE_NOT_PRESENT)
        };

        Some(ui::UiMiniScores::Duel {
            icon_paths,
            blue: ui::UiDuelMiniScore {
                name: d1_name,
                score: client_score(duelists[0]),
                model_icon: Some(0),
            },
            red: ui::UiDuelMiniScore {
                name: d2_name,
                score: client_score(duelists[1]),
                model_icon: Some(1),
            },
        })
    }

    /// jaPRO CG_DrawFollow, verbatim: with `cg.snap->ps.pm_flags & PMF_FOLLOW`
    /// (or during demo playback) it draws `cgs.clientinfo[cg.snap->ps.clientNum].name`.
    /// It reads `cg.snap->ps`, never the predicted state: a free spectator's
    /// predicted state can keep a stale PMF_FOLLOW after unfollowing.
    pub(in crate::app) fn current_follow_name(&self) -> Option<String> {
        const PMF_FOLLOW: i32 = 4096;
        let session = self.game_session.as_ref()?;
        let ps = &session.client_game.current_snapshot()?.player_state;
        if ps.field_i32("pm_flags").unwrap_or(0) & PMF_FOLLOW == 0 && session.live {
            return None;
        }
        let snapshot_client = ps.field_i32("clientNum")?;
        let effective_client = if session.live {
            snapshot_client
        } else {
            match self.demo_view_mode {
                DemoViewMode::Authoritative => snapshot_client,
                DemoViewMode::Follow(client) => client,
                DemoViewMode::Free => return None,
            }
        };
        let client_num = usize::try_from(effective_client).ok()?;
        let mut text = session
            .client_game
            .client_name(client_num)
            .filter(|name| !name.is_empty())?;
        // Alternate demo POVs do not have an authoritative PlayerState for the
        // jaPRO race-style subline. Never show the recorded player's style under
        // a different followed player's name.
        if !session.live && effective_client != snapshot_client {
            return Some(text);
        }

        // "Loda - add their movemnt style here": under the name when the server is
        // jaPRO and the *predicted* playerState is in racemode (verbatim). The
        // renderer splits the second line off at '\n'.
        if self.active_server_mod() == crate::net::mod_support::ServerMod::Japro {
            let predicted = self.live_player_state().unwrap_or(ps);
            if predicted.stats[crate::japro_cg::STAT_RACEMODE] != 0 {
                let dueling = predicted.field_i32("duelInProgress").unwrap_or(0) != 0;
                let partner = usize::try_from(predicted.field_i32("duelIndex").unwrap_or(-1))
                    .ok()
                    .and_then(|index| session.client_game.client_name(index))
                    .filter(|name| !name.is_empty());
                text.push('\n');
                text.push_str(&crate::japro_cg::integer_to_race_name(
                    predicted.stats[crate::japro_cg::STAT_MOVEMENTSTYLE],
                    dueling,
                    partner.as_deref(),
                ));
            }
        }
        Some(text)
    }

    /// TaystJK's scoreboard identifies the highlighted score row with
    /// `cg.snap->ps.clientNum`. In live follow mode that field is the followed
    /// client; for this client's alternate demo POVs use the explicitly selected
    /// fake-follow client instead of the recorded POV.
    pub(in crate::app) fn scoreboard_focus_client(&self) -> Option<i32> {
        let session = self.game_session.as_ref()?;
        let snapshot_client = session
            .client_game
            .current_snapshot()?
            .player_state
            .field_i32("clientNum")?;
        if session.live {
            return Some(snapshot_client);
        }
        match self.demo_view_mode {
            DemoViewMode::Authoritative => Some(snapshot_client),
            DemoViewMode::Follow(client) => Some(client),
            DemoViewMode::Free => None,
        }
    }

    /// `CG_DrawOldScoreboard`: explicit +scores, death, and intermission keep
    /// the board fully visible. Intermission is also the dedicated 2D path in
    /// TaystJK, so it must not depend on the +scores key being held.
    pub(in crate::app) fn scoreboard_forced_by_player_state(&self) -> bool {
        const PM_DEAD: i32 = 5;
        let Some(session) = self.game_session.as_ref() else {
            return false;
        };
        // `CG_DrawOldScoreboard`: death does not force the board during a
        // warmup. Explicit +scores still wins because scoreboard_should_show
        // checks scores_showing separately.
        let warmup = session
            .client_game
            .configstring(crate::cgame::CS_WARMUP)
            .and_then(|value| std::str::from_utf8(value).ok())
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(0);
        if warmup != 0 {
            return false;
        }
        let snapshot = session.client_game.current_snapshot();
        let ps = session.presented_player_state()
            .or_else(|| snapshot.map(|snapshot| &snapshot.player_state));
        let pm_type = ps.and_then(|ps| ps.field_i32("pm_type")).unwrap_or(0);
        pm_type == PM_DEAD || pm_type == crate::cgame::PM_INTERMISSION
    }

    pub(in crate::app) fn scoreboard_should_show(&self) -> bool {
        self.scores_showing || self.scoreboard_forced_by_player_state()
    }

    pub(in crate::app) fn visible_scoreboard(&self) -> Option<UiScoreboard> {
        if !self.scoreboard_should_show() {
            return None;
        }
        // The scoreboard is a presentation surface, not a network response.
        // TaystJK draws the header even when cg.numScores == 0 (including
        // immediately after a new score request). A server rejecting/delaying
        // `score` must never make the scoreboard itself disappear.
        let session = self.game_session.as_ref()?;
        let mut board = self.scoreboard.clone().unwrap_or_else(|| {
            let gametype = session.client_game.gametype();
            let limits = session.client_game.match_limits();
            UiScoreboard {
                team_scores: [limits.scores1.max(0), limits.scores2.max(0)],
                ready_clients: 0,
                team_game: gametype >= 6,
                ctf: gametype == 8,
                siege: gametype == 7,
                siege_win_team: session.client_game.siege_win_team(),
                spectator_scores: matches!(gametype, 3 | 4),
                show_deaths: false,
                icon_paths: Vec::new(),
                entries: Vec::new(),
            }
        });

        // TaystJK CG_DrawClientScore does not cache readiness in the `scores`
        // command. It reads cg.snap->ps.stats[STAT_CLIENTS_READY] while drawing
        // every row, so spectators and followed views see the authoritative
        // intermission mask as soon as a snapshot changes. The wire stat is a
        // signed 16-bit value; normalize it back to its low 16 mask bits.
        board.ready_clients = self
            .game_session
            .as_ref()
            .and_then(|session| session.client_game.current_snapshot())
            .map(|snapshot| {
                snapshot.player_state.stats[jka_protocol::server::STAT_CLIENTS_READY] as u16
            })
            .unwrap_or(0);

        Some(board)
    }

    /// Jump-height helper for the renderer. Only a jaPRO server in the SP
    /// movement style ever engages it, and the setting gates all further work.
    pub(in crate::app) fn current_jump_shade(&self) -> crate::jump_shade::JumpShadeState {
        use crate::jump_shade::JumpShadeState;
        if !self.jump_height_shade
            || self.active_server_mod() != crate::net::mod_support::ServerMod::Japro
        {
            return JumpShadeState::Off;
        }
        self.live_player_state()
            .map_or(JumpShadeState::Off, JumpShadeState::from_player_state)
    }

    /// Live: the predicted playerstate; solo: the offline Pmove host.
    /// cg.forceHUDTotalFlashTime: the force bar blinks red every 400 ms for a second.
    pub(in crate::app) fn force_flash_active(&self) -> bool {
        self.game_session
            .as_ref()
            .and_then(|session| session.force_flash_until)
            .is_some_and(|until| {
                let left = until.saturating_duration_since(Instant::now());
                !left.is_zero() && (left.as_millis() / 400) % 2 == 0
            })
    }

    pub(in crate::app) fn hud_state_from_player_state(
        &self,
        ps: &jka_protocol::server::PlayerState,
    ) -> HudState {
        let weapon = ps.field_i32("weapon").unwrap_or(0);
        let ammo = jka_movement::weapon_info(weapon)
            .filter(|(ammo_index, _, _)| *ammo_index > 0)
            .and_then(|(ammo_index, _, _)| ps.ammo.get(ammo_index as usize).copied());
        HudState {
            health: ps.stats[0],
            max_health: ps.stats[8].max(1),
            armor: ps.stats[5],
            force_power: Some(ps.field_i32("fd.forcePower").unwrap_or(0)),
            force_power_max: 100,
            ammo,
            weapon,
            // cg_draw.c shows saberDrawAnimLevel: the server updates it the
            // moment a style is queued, while saberAnimLevel waits out the swing.
            saber_style: ps.field_i32("fd.saberDrawAnimLevel").unwrap_or(0),
            saber_active_style: ps.field_i32("fd.saberAnimLevel").unwrap_or(0),
            force_flash: self.force_flash_active(),
        }
    }

    /// Best available HUD state for a non-authoritative demo POV. dm_26 only
    /// records one complete playerState; remote status comes from recorded tinfo
    /// only, so missing data hides the HUD instead of borrowing the recorder.
    pub(in crate::app) fn demo_fake_hud_state(&self, client: i32) -> Option<HudState> {
        let session = self.game_session.as_ref().filter(|session| !session.live)?;
        // dm_26 records only one complete playerState. For a different POV,
        // use only recorded tinfo for status values; never borrow or fabricate
        // the recorder's health/armor/force. In normal team play this means the
        // HUD is available for teammates when the server sent team-overlay data.
        let snapshot = session.client_game.current_snapshot()?;
        let recorder_team = snapshot.player_state.persistant[3];
        if matches!(recorder_team, 1 | 2) {
            let followed_team = usize::try_from(client)
                .ok()
                .and_then(|client| {
                    session
                        .client_game
                        .client_info(client, &session.siege_classes)
                })
                .map_or(3, |info| info.team);
            if followed_team != recorder_team {
                return None;
            }
        }
        let info = session
            .client_game
            .team_info()
            .iter()
            .find(|info| i32::from(info.client) == client)?;
        let health = info.health.max(0);
        let mut armor = info.armor.max(0);
        let force_power = if session.client_game.is_japro() {
            let mut force = armor % 100;
            if force == 0 {
                force = 100;
                armor = armor.saturating_sub(100);
            }
            armor /= 100;
            Some(force)
        } else {
            None
        };
        Some(HudState {
            health,
            max_health: health.max(100).max(1),
            armor,
            force_power,
            force_power_max: 100,
            // Ammo and saber style are not part of tinfo/remote EntityState.
            ammo: None,
            weapon: info.weapon,
            saber_style: 0,
            saber_active_style: 0,
            force_flash: false,
        })
    }

    pub(in crate::app) fn combat_overlay_state(&self) -> (Option<ui::UiZoomScope>, Option<ui::UiRocketLock>) {
        let Some(session) = self.game_session.as_ref() else { return (None, None); };
        let Some(snapshot) = session.current_snapshot.as_ref() else { return (None, None); };
        if self.demo_view_mode == DemoViewMode::Free && !session.live { return (None, None); }
        let ps = session.presented_player_state().unwrap_or(&snapshot.player_state);
        let time = session.cgame_time;
        // Zoom art belongs to the gun zoom (1) or binoculars (2), not
        // cg_zoomFov's generic +zoom camera FOV mode.
        let zoom_mode = ps.field_i32("zoomMode").unwrap_or(0);
        let binoculars_allowed = !(zoom_mode == 2
            && session.client_game.is_japro()
            && ps.stats[crate::japro_cg::STAT_RACEMODE] != 0);
        let zoom_scope = (zoom_mode == 1 || (zoom_mode == 2 && binoculars_allowed)).then(|| ui::UiZoomScope {
            mode: zoom_mode,
            fov: ps.field_f32("zoomFov").unwrap_or(50.0),
            time,
            yaw_deg: ps.field_f32("viewangles[1]").unwrap_or(0.0),
            // bg_weapons.h AMMO_POWERCELL=3; bg_weapons.c max=300.
            ammo: ps.ammo[3].max(0),
            double_ammo: ps.field_i32("eFlags").unwrap_or(0) & (1<<20) != 0,
            charge_started: (ps.field_i32("weaponstate").unwrap_or(0) == 5)
                .then(|| ps.field_i32("weaponChargeTime").unwrap_or(time)),
        });
        // Render the wheel only when the real server/predicted playerState
        // reports a valid target and an active lock timer.
        let index = ps.field_i32("rocketLockIndex").unwrap_or(1023);
        let start = ps.field_i32("rocketLockTime").unwrap_or(0);
        let rocket_lock = if (0..1023).contains(&index) && start > 0 && time > start
            && ps.persistant[3] != 3 // PERS_TEAM != TEAM_SPECTATOR
        {
            session.presented_entities.iter().find(|e| i32::from(e.number) == index)
                .map(|entity| {
                    let origin = [0, 1, 2].map(|i| ps.field_f32(match i {
                        0 => "origin[0]", 1 => "origin[1]", _ => "origin[2]"
                    }).unwrap_or(0.0));
                    let elapsed = time.saturating_sub(start);
                    let interval = if session.client_game.gametype() == 7 { 150 } else { 75 };
                    ui::UiRocketLock {
                        target: [entity.origin[0], entity.origin[1], entity.origin[2] + 28.0],
                        viewer: origin,
                        wedges: (elapsed / interval).clamp(0, 8) as u8,
                        time,
                    }
                })
        } else { None };
        (zoom_scope, rocket_lock)
    }

    pub(in crate::app) fn current_hud_state(&self) -> Option<HudState> {
        if let Some(ps) = self.live_player_state() {
            return Some(self.hud_state_from_player_state(ps));
        }
        if let Some(session) = self.game_session.as_ref().filter(|session| !session.live) {
            if self.demo_view_mode != DemoViewMode::Free {
                if let DemoViewMode::Follow(client) = self.demo_view_mode {
                    let authoritative = session
                        .current_snapshot
                        .as_ref()
                        .and_then(|snapshot| snapshot.player_state.field_i32("clientNum"));
                    if authoritative != Some(client) {
                        return self.demo_fake_hud_state(client);
                    }
                }
                if let Some(ps) = session.presented_player_state().or_else(|| {
                    session.current_snapshot.as_ref().map(|snapshot| &snapshot.player_state)
                }) {
                    return Some(self.hud_state_from_player_state(ps));
                }
            }
        }
        self.local_server.as_ref().and_then(|player| {
            (player.mode == JoinMode::Player).then(|| {
                let view = player.view();
                HudState {
                    health: view.health,
                    max_health: view.max_health.max(1),
                    armor: view.armor,
                    force_power: Some(view.force_power),
                    force_power_max: view.force_power_max.max(1),
                    ammo: (view.ammo >= 0).then_some(view.ammo),
                    weapon: view.weapon,
                    saber_style: player.saber_style(),
                    saber_active_style: player.saber_active_style(),
                    force_flash: self.force_flash_active(),
                }
            })
        })
    }

    /// TaystJK CG_DrawTeamOverlay data projection. `tinfo` remains the only
    /// remote data source; this function only applies the same client-side
    /// filtering/unpacking/icon lookup metadata used by cg_draw.c.
    pub(in crate::app) fn team_overlay_ui(&self) -> Option<ui::TeamOverlayUi> {
        use crate::cgame::{CS_LOCATIONS, GT_TEAM, TEAM_BLUE, TEAM_RED};

        if self.team_overlay.mode == 0 {
            return None;
        }
        let session = self.game_session.as_ref()?;
        if session.client_game.gametype() < GT_TEAM {
            return None;
        }
        let snapshot = session.client_game.current_snapshot()?;
        let ps = &snapshot.player_state;
        let snapshot_client = ps.field_i32("clientNum").unwrap_or(-1);
        let (local, team) = if !session.live {
            match self.demo_view_mode {
                DemoViewMode::Free => return None,
                DemoViewMode::Follow(client) if client != snapshot_client => {
                    let team = usize::try_from(client)
                        .ok()
                        .and_then(|client| {
                            session
                                .client_game
                                .client_info(client, &session.siege_classes)
                        })
                        .map_or(3, |info| info.team);
                    (client, team)
                }
                _ => (snapshot_client, ps.persistant[3]),
            }
        } else {
            (snapshot_client, ps.persistant[3])
        };
        if team != TEAM_RED && team != TEAM_BLUE {
            return None;
        }
        let japro = session.client_game.is_japro();
        let omit_local = matches!(self.team_overlay.mode, 2 | 4 | 6);

        let mut icon_paths: Vec<String> = Vec::new();
        let add_icon = |icon_paths: &mut Vec<String>, candidates: Vec<String>| -> Option<u16> {
            let candidates: Vec<String> = candidates
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect();
            if candidates.is_empty() {
                return None;
            }
            let key = candidates.join("\n");
            if let Some(index) = icon_paths.iter().position(|existing| existing == &key) {
                return u16::try_from(index).ok();
            }
            let index = u16::try_from(icon_paths.len()).ok()?;
            icon_paths.push(key);
            Some(index)
        };

        // One table walk, not one walk per icon. The overlay may be sampled at
        // very high frame rates while tinfo itself updates comparatively slowly.
        static TEAM_OVERLAY_ITEM_ICONS: std::sync::OnceLock<(
            HashMap<i32, String>,
            HashMap<i32, String>,
        )> = std::sync::OnceLock::new();
        let (weapon_icons, powerup_icons_by_tag) = TEAM_OVERLAY_ITEM_ICONS.get_or_init(|| {
            let mut weapon_icons = HashMap::<i32, String>::new();
            let mut powerup_icons_by_tag = HashMap::<i32, String>::new();
            for index in 1..jka_movement::bg_item_count() {
                let Some(item) = jka_movement::bg_item(index) else {
                    continue;
                };
                let Some(icon) = jka_movement::bg_item_icon(index) else {
                    continue;
                };
                if item.item_type == 1 {
                    weapon_icons.entry(item.tag).or_insert(icon);
                } else if item.item_type == 5 || item.item_type == 8 {
                    powerup_icons_by_tag.entry(item.tag).or_insert(icon);
                }
            }
            (weapon_icons, powerup_icons_by_tag)
        });

        let resolve_location = |raw: &[u8]| -> String {
            let text = crate::cgame::bytes_to_lossless_ascii(raw);
            if let Some(key) = text.strip_prefix('@') {
                if let Some(table) = self.stringed.as_ref() {
                    let localized = crate::cgame::bytes_to_lossless_ascii(table.get(key));
                    if !localized.is_empty() {
                        return localized;
                    }
                }
            }
            text
        };
        // TaystJK computes overlay sizing/hasLocations from every authored map
        // location, not just the locations occupied by the eight displayed players.
        let mut location_width = 0usize;
        let mut has_locations = false;
        for location in 1..64u16 {
            let Some(raw) = session.client_game.configstring(CS_LOCATIONS + location) else {
                continue;
            };
            let text = resolve_location(raw);
            if text.is_empty() {
                continue;
            }
            has_locations = true;
            location_width = location_width.max(ui::visible_jka_chars(&text)).min(16);
        }

        let mut entries = Vec::with_capacity(8);
        // Exactly as TaystJK: clamp the sorted tinfo list to eight before
        // rejecting invalid/opposite-team entries.
        for team_info in session.client_game.team_info().iter().take(8) {
            let client_num = usize::from(team_info.client);
            let Some(info) = session
                .client_game
                .client_info(client_num, &session.siege_classes)
            else {
                continue;
            };
            if info.team != team || (omit_local && i32::try_from(client_num).ok() == Some(local)) {
                continue;
            }

            let mut health = team_info.health;
            let mut armor = team_info.armor;
            let mut force = -1;
            if japro {
                force = armor % 100;
                if force == 0 {
                    force = 100;
                    armor -= 100;
                }
                armor /= 100;
            }
            // Only the recorded/snapshot client owns a complete playerState.
            // A followed remote client's row must remain on tinfo rather than
            // borrowing the recorder's health/armor/force.
            if i32::try_from(client_num).ok() == Some(local) && local == snapshot_client {
                health = ps.stats[0];
                armor = ps.stats[5];
                if japro {
                    force = ps.field_i32("fd.forcePower").unwrap_or(force);
                }
            }
            health = health.max(0);
            armor = armor.max(0);

            let location_index = if (0..64).contains(&team_info.location) {
                team_info.location as u16
            } else {
                0
            };
            let location = session
                .client_game
                .configstring(CS_LOCATIONS + location_index)
                .map(resolve_location)
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| "unknown".to_owned());

            // CG_LoadClientInfo's modelIcon is registered *after*
            // BG_ValidateSkinForTeam. Resolve that exact skin on the render
            // thread where the VFS is available (important for jedi_/rgb skins
            // whose team handling depends on authored model_*.skin files).
            let model_icon_key = format!(
                "@team-model-icon\t{}\t{}\t{}\t{}\t{}",
                info.model_name,
                info.skin_name,
                team,
                info.gametype,
                u8::from(info.jedi_v_merc),
            );
            let model_icon = add_icon(
                &mut icon_paths,
                vec![model_icon_key, "gfx/2d/defer".to_owned()],
            );

            let weapon_icon = if self.team_overlay.weapons {
                weapon_icons
                    .get(&team_info.weapon)
                    .cloned()
                    .and_then(|path| {
                        add_icon(&mut icon_paths, vec![path, "gfx/2d/defer".to_owned()])
                    })
            } else {
                None
            };
            let mut powerup_icons = Vec::new();
            for powerup in 0..32 {
                if (team_info.powerups & (1_i32.wrapping_shl(powerup))) == 0 {
                    continue;
                }
                if let Some(path) = powerup_icons_by_tag.get(&(powerup as i32)).cloned() {
                    if let Some(index) = add_icon(&mut icon_paths, vec![path]) {
                        powerup_icons.push(index);
                    }
                }
            }

            entries.push(ui::TeamOverlayEntry {
                client: team_info.client,
                name: info.name,
                location,
                health,
                armor,
                force,
                model_icon,
                weapon_icon,
                powerup_icons,
            });
        }
        (!entries.is_empty()).then_some(ui::TeamOverlayUi {
            settings: self.team_overlay,
            team,
            location_width,
            has_locations,
            icon_paths,
            entries,
        })
    }

    pub(in crate::app) fn cycle_spawn(&mut self) {
        if self.spawns.is_empty() {
            return;
        }
        self.spawn_index = (self.spawn_index + 1) % self.spawns.len();
        let spawn = self.spawns[self.spawn_index];
        let respawned = if let Some(server) = &mut self.local_server {
            match server.respawn(spawn) {
                Ok(()) => true,
                Err(error) => {
                    self.console_status = error;
                    false
                }
            }
        } else {
            false
        };
        if respawned {
            self.push_local_snapshot(true);
        }
        self.third_person_camera.reset();
        self.update_solo_player_view_and_presentation();
        self.publish_snapshot();
    }

    pub(in crate::app) fn player_names_restricted(&self) -> bool {
        self.server_restrictions()
            .contains(super::restrictions::PLAYER_LABELS)
    }

    /// TaystJK `CG_PlayerLabels` visibility discovery. The original walks and
    /// traces every client during HUD drawing; DinurdoJK samples only the
    /// expensive visibility decision at 30 Hz. Accepted labels are projected
    /// against the final render camera every frame on the render thread.
    pub(in crate::app) fn update_player_name_visibility(&mut self, now: Instant) {
        use crate::cgame::crosshair;
        use jka_movement::TraceWorld;
        const SCAN_INTERVAL: Duration = Duration::from_millis(33);
        const ENTITYNUM_WORLD: i32 = 1022;
        const MAX_CLIENTS: usize = 32;
        const TEAM_SPECTATOR: i32 = 3;
        const EF_DEAD: i32 = 1 << 1;
        const JAPRO_CINFO2_WTTRIBES: i32 = 1 << 4;
        const MAX_LABEL_DISTANCE_SQ: f32 = 3000.0 * 3000.0;

        let overlay_allows_labels = matches!(
            self.overlay,
            OverlayMode::None | OverlayMode::Chat | OverlayMode::Vgs
        );
        let normal_active = self.player_names.mode != 0
            && overlay_allows_labels
            && self.game_session.is_some()
            && !self.player_names_restricted()
            && self.game_session.as_ref().map_or(true, |session| {
                session.client_game.japro_cinfo2() & JAPRO_CINFO2_WTTRIBES == 0
            });
        let ghost_active = overlay_allows_labels
            && (self.race_ghost_name
                || self.race_ghost_velocity_delta
                || self.race_ghost_distance_delta)
            && self
                .game_session
                .as_ref()
                .is_some_and(|session| !session.race_ghosts.is_empty());
        if !normal_active && !ghost_active {
            self.player_name_visible = [false; MAX_CLIENTS];
            self.race_ghost_label_visible = [false; 128];
            return;
        }
        if now.saturating_duration_since(self.player_name_scan_at) < SCAN_INTERVAL {
            return;
        }
        self.player_name_scan_at = now;

        let Some(session) = self.game_session.as_ref() else {
            return;
        };
        let Some(snapshot) = session.client_game.current_snapshot() else {
            return;
        };
        let viewer = snapshot.player_state.field_i32("clientNum").unwrap_or(-1);
        let own = self
            .net
            .as_ref()
            .map_or(viewer, |net| net.session().client_num());
        let start = session.presented_player_state()
            .and_then(|ps| playerstate_vec3(ps, "origin"))
            .or_else(|| playerstate_vec3(&snapshot.player_state, "origin"))
            .unwrap_or_default();
        let viewer_state = session
            .presented_entities
            .iter()
            .find(|entity| i32::from(entity.number) == viewer)
            .map(|entity| &entity.state);
        let solids = crate::net::solid_entities(&session.presented_entities, snapshot.server_time);
        let candidates = if normal_active {
            session
                .presented_entities
                .iter()
                .filter_map(|entity| {
                    let client = usize::from(entity.number);
                    if client >= MAX_CLIENTS
                        || entity.entity_type != ET_PLAYER
                        || i32::from(entity.number) == viewer
                        || i32::from(entity.number) == own
                        || entity.state.field_i32("eFlags").unwrap_or(0) & EF_DEAD != 0
                    {
                        return None;
                    }
                    let team = u16::try_from(client)
                        .ok()
                        .and_then(|client| crate::cgame::CS_PLAYERS.checked_add(client))
                        .and_then(|index| session.client_game.configstring(index))
                        .and_then(|info| jka_protocol::commands::info_value(info, b"t"))
                        .map_or(0, jka_protocol::commands::atoi);
                    if team == TEAM_SPECTATOR
                        || crosshair::is_mind_tricked(&entity.state, viewer, viewer_state)
                    {
                        return None;
                    }
                    let delta = [
                        entity.origin[0] - start[0],
                        entity.origin[1] - start[1],
                        entity.origin[2] - start[2],
                    ];
                    let distance2 = delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2];
                    (distance2 < MAX_LABEL_DISTANCE_SQ).then_some((client, entity.origin))
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let ghost_candidates = if ghost_active {
            session
                .race_ghosts
                .iter()
                .filter_map(|ghost| {
                    let slot = usize::from(ghost.entity_num.saturating_sub(60_000));
                    let sample = ghost.visual_sample?;
                    if slot >= 128 {
                        return None;
                    }
                    let delta = [
                        sample.origin[0] - start[0],
                        sample.origin[1] - start[1],
                        sample.origin[2] - start[2],
                    ];
                    let distance2 = delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2];
                    (distance2 < MAX_LABEL_DISTANCE_SQ).then_some((slot, sample.origin))
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let mut visible = [false; MAX_CLIENTS];
        let mut ghost_visible = [false; 128];
        let Some(world) = self.map_collision.as_mut() else {
            self.player_name_visible = visible;
            self.race_ghost_label_visible = ghost_visible;
            return;
        };
        let mut prediction_world = crate::net::PredictionWorld {
            world,
            solids: &solids,
            client_num: own,
        };
        for (client, end) in candidates {
            let trace = prediction_world.trace(jka_movement::TraceQuery {
                start,
                mins: [0.0; 3],
                maxs: [0.0; 3],
                end,
                pass_entity: own,
                mask: 0x1 | 0x100, // CONTENTS_SOLID | CONTENTS_BODY
            });
            // TaystJK rejects only a world hit here; another body between the
            // viewer and target does not suppress the target's label.
            visible[client] = trace.entity != ENTITYNUM_WORLD;
        }
        for (slot, end) in ghost_candidates {
            let trace = prediction_world.trace(jka_movement::TraceQuery {
                start,
                mins: [0.0; 3],
                maxs: [0.0; 3],
                end,
                pass_entity: own,
                mask: 0x1 | 0x100,
            });
            ghost_visible[slot] = trace.entity != ENTITYNUM_WORLD;
        }
        self.player_name_visible = visible;
        self.race_ghost_label_visible = ghost_visible;
    }

    pub(in crate::app) fn world_player_names(&self) -> Option<ui::UiWorldPlayerNames> {
        const MAX_CLIENTS: usize = 32;
        const TEAM_SPECTATOR: i32 = 3;
        const EF_DEAD: i32 = 1 << 1;
        const GT_TEAM: i32 = 6;
        let overlay_allows_labels = matches!(
            self.overlay,
            OverlayMode::None | OverlayMode::Chat | OverlayMode::Vgs
        );
        if !overlay_allows_labels {
            return None;
        }
        let session = self.game_session.as_ref()?;
        let normal_active = self.player_names.mode != 0
            && !self.player_names_restricted()
            && session.client_game.japro_cinfo2() & (1 << 4) == 0;
        let ghost_active = self.race_ghost_name
            || self.race_ghost_velocity_delta
            || self.race_ghost_distance_delta;
        if !normal_active && !ghost_active {
            return None;
        }

        let mut labels = Vec::new();
        if normal_active {
            let snapshot = session.client_game.current_snapshot()?;
            let viewer_team = snapshot.player_state.persistant[3];
            let gametype = session.client_game.gametype();
            for entity in &session.presented_entities {
                let client = usize::from(entity.number);
                if client >= MAX_CLIENTS
                    || !self.player_name_visible[client]
                    || entity.entity_type != ET_PLAYER
                    || entity.state.field_i32("eFlags").unwrap_or(0) & EF_DEAD != 0
                {
                    continue;
                }
                let team = u16::try_from(client)
                    .ok()
                    .and_then(|client| crate::cgame::CS_PLAYERS.checked_add(client))
                    .and_then(|index| session.client_game.configstring(index))
                    .and_then(|info| jka_protocol::commands::info_value(info, b"t"))
                    .map_or(0, jka_protocol::commands::atoi);
                let Some(name) = session.client_game.client_name(client) else {
                    continue;
                };
                if team == TEAM_SPECTATOR || name.is_empty() {
                    continue;
                }
                let mut anchor = entity.origin;
                anchor[2] += 64.0;
                let (health_fraction, health_color) = if self.player_names.mode > 1 {
                    let health = entity.state.field_i32("health").unwrap_or(0);
                    let max_health = entity.state.field_i32("maxhealth").unwrap_or(0);
                    if max_health > 0 && health > 0 {
                        let teamowner = entity.state.field_i32("teamowner").unwrap_or(0);
                        let color = if teamowner == 0 || gametype < GT_TEAM {
                            [1.0, 1.0, 0.0, 0.4]
                        } else if teamowner == viewer_team {
                            [0.0, 1.0, 0.0, 0.4]
                        } else {
                            [1.0, 0.0, 0.0, 0.4]
                        };
                        (Some(health.max(0) as f32 / max_health as f32), color)
                    } else {
                        (None, [1.0, 1.0, 0.0, 0.4])
                    }
                } else {
                    (None, [1.0, 1.0, 0.0, 0.4])
                };
                labels.push(ui::UiWorldPlayerLabel {
                    anchor: scene::render_position(anchor),
                    text: name,
                    health_fraction,
                    health_color,
                });
            }
        }

        if ghost_active {
            let live = session.race_ghost_live_sample;
            for ghost in &session.race_ghosts {
                let slot = usize::from(ghost.entity_num.saturating_sub(60_000));
                let Some(sample) = ghost.visual_sample else {
                    continue;
                };
                if slot >= self.race_ghost_label_visible.len()
                    || !self.race_ghost_label_visible[slot]
                {
                    continue;
                }

                let mut first_line = String::new();
                if self.race_ghost_name {
                    first_line.push_str(&ghost.track.display_name);
                }
                let mut stats = Vec::with_capacity(2);
                if self.race_ghost_velocity_delta {
                    if let Some(live) = live {
                        let ghost_speed = sample.velocity[0].hypot(sample.velocity[1]);
                        let live_speed = live.velocity[0].hypot(live.velocity[1]);
                        stats.push(format!("dSpeed {:+.0} ups", ghost_speed - live_speed));
                    }
                }
                if self.race_ghost_distance_delta {
                    if let Some(live) = live {
                        let dx = sample.origin[0] - live.origin[0];
                        let dy = sample.origin[1] - live.origin[1];
                        let dz = sample.origin[2] - live.origin[2];
                        stats.push(format!("gap {:.0}u", (dx * dx + dy * dy + dz * dz).sqrt()));
                    }
                }
                if first_line.is_empty() && stats.is_empty() {
                    continue;
                }
                if !stats.is_empty() {
                    if !first_line.is_empty() {
                        first_line.push('\n');
                    }
                    first_line.push_str(&stats.join("  "));
                }
                let mut anchor = sample.origin;
                anchor[2] += 64.0;
                labels.push(ui::UiWorldPlayerLabel {
                    anchor: scene::render_position(anchor),
                    text: first_line,
                    health_fraction: None,
                    health_color: [1.0, 1.0, 1.0, 0.0],
                });
            }
        }

        (!labels.is_empty()).then(|| ui::UiWorldPlayerNames {
            scale: self.player_names.scale,
            labels: Arc::new(labels),
        })
    }
}

/// `CG_DrawGenericTimerBar`: a vertical bar at the lower right that empties as
/// `remaining` (1.0 -> 0.0) runs out. Coordinates are the 640x480 HUD space.
pub(in crate::app) fn generic_timer_bar(
    draws: &mut Vec<crate::fx::draw::ScreenFxDraw>,
    remaining: f32,
) {
    use crate::fx::draw::{FxBlend, FxMaterial, ScreenFxAnchor, ScreenFxDraw};
    const BAR_H: f32 = 50.0;
    const BAR_W: f32 = 10.0;
    const BAR_X: f32 = 640.0 - BAR_W - 120.0;
    const BAR_Y: f32 = 480.0 - BAR_H - 20.0;
    let material = || FxMaterial {
        animation: None,
        texture: None,
        blend: FxBlend::Alpha,
        rgb_vertex: true,
        alpha_vertex: true,
        rgb_const: [1.0; 3],
        alpha_const: 1.0,
        rgb_wave: None,
        alpha_wave: None,
        rgb_one_minus_vertex: false,
        alpha_one_minus_vertex: false,
        tc_mods: Vec::new(),
        shader_time_offset: 0.0,
        shader_clamp_time: None,
        authored_glow: false,
    };
    let mut rect = |rect: [f32; 4], color: [f32; 4]| {
        draws.push(ScreenFxDraw {
            rect,
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            color,
            material: material(),
            anchor: ScreenFxAnchor::Right,
        });
    };
    let percent = (remaining.clamp(0.0, 1.0) * BAR_H).max(0.1);
    let black = [0.0, 0.0, 0.0, 1.0];
    // 1 px outline.
    rect([BAR_X, BAR_Y, BAR_W, 1.0], black);
    rect([BAR_X, BAR_Y + BAR_H - 1.0, BAR_W, 1.0], black);
    rect([BAR_X, BAR_Y, 1.0, BAR_H], black);
    rect([BAR_X + BAR_W - 1.0, BAR_Y, 1.0, BAR_H], black);
    // Remaining time in yellow, the spent part greyed out.
    rect(
        [
            BAR_X + 1.0,
            BAR_Y + 1.0 + (BAR_H - percent),
            BAR_W - 2.0,
            percent - 1.0,
        ],
        [1.0, 1.0, 0.0, 1.0],
    );
    rect(
        [BAR_X + 1.0, BAR_Y + 1.0, BAR_W - 2.0, BAR_H - percent],
        [0.5, 0.5, 0.5, 0.1],
    );
}
