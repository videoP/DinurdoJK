use super::{egui_theme as theme, race_ghost_web::RemoteDemo, App};
use crate::ui::OverlayMode;

impl App {
    pub(super) fn open_race_ghosts(&mut self) {
        self.set_overlay(OverlayMode::RaceGhosts);
        let context = self.race_ghost_context().ok();
        let stale = context.as_ref().is_some_and(|(map, _, style)| {
            self.race_ghost_web_active_map.as_deref() != Some(map.as_str())
                || self.race_ghost_web_active_style.as_deref() != Some(style.as_str())
        });
        if stale || (self.race_ghost_web_courses.is_empty() && !self.race_ghost_web_catalog_pending) {
            self.request_race_ghost_catalog();
        } else if !self.race_ghost_web_catalog_pending {
            // Re-opening the menu is a fresh "where am I?" check. Do not do
            // this continuously while the menu is open, because a manual course
            // choice must not be replaced as the player moves.
            self.select_suggested_race_ghost_course();
        }
    }

    pub(super) fn egui_race_ghosts(&mut self, root: &mut egui::Ui) {
        // Movement style can change in-place via `move`; if that happens while
        // this menu is open, never keep showing the previous style's catalog.
        if let Ok((map, _, style)) = self.race_ghost_context() {
            if self.race_ghost_web_active_map.as_deref() != Some(map.as_str())
                || self.race_ghost_web_active_style.as_deref() != Some(style.as_str())
            {
                // A movement-style or map change invalidates even an in-flight
                // request. request_race_ghost_catalog bumps the generation so
                // the stale worker can finish harmlessly in the background.
                self.request_race_ghost_catalog();
            }
        }

        let mut close = false;
        let mut refresh = false;
        let mut apply_base = false;
        let mut select_course: Option<String> = None;
        let mut toggle_demo: Option<(RemoteDemo, bool)> = None;
        let mut clear_all = false;
        let mut unload_key: Option<String> = None;
        let mut alpha_changed = false;
        let mut visuals_changed = false;

        let context = self.race_ghost_context();
        let loaded = self
            .game_session
            .as_ref()
            .map(|session| {
                session
                    .race_ghosts
                    .iter()
                    .map(|ghost| {
                        (
                            ghost.track.source_key.clone(),
                            ghost.track.display_name.clone(),
                            ghost.track.duration_ms(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        egui::CentralPanel::default().show_inside(root, |ui| {
            ui.horizontal(|ui| {
                ui.heading("RACE GHOSTS");
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Race-synchronized .dm_26 references").weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() { close = true; }
                    if ui.button("Refresh web catalog").clicked() { refresh = true; }
                });
            });
            ui.separator();

            match &context {
                Ok((map, _, style)) => {
                    ui.horizontal(|ui| {
                        theme::glow_label(ui, "CURRENT", 11.5, theme::TEXT_FAINT);
                        ui.label(egui::RichText::new(format!("{map}  /  {style}")).strong());
                    });
                }
                Err(error) => {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Demo base URL");
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.race_ghost_demo_base_url_input)
                        .desired_width(430.0),
                );
                let enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if ui.button("Apply").clicked() || enter { apply_base = true; }
            });
            ui.label(
                egui::RichText::new(format!(
                    "Catalog root: {}/index/",
                    self.race_ghost_demo_base_url.trim_end_matches('/')
                ))
                .weak()
                .small(),
            );

            ui.add_space(10.0);
            ui.columns(2, |columns| {
                let left = &mut columns[0];
                left.heading("Courses");
                if self.race_ghost_web_catalog_pending {
                    left.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Loading map catalog...");
                    });
                } else if self.race_ghost_web_courses.is_empty() {
                    left.label(egui::RichText::new("No courses loaded for this map/style.").weak());
                }
                if let Some(suggested) = self
                    .race_ghost_web_suggested_course
                    .as_deref()
                    .and_then(|course| self.race_ghost_web_courses.iter().find(|entry| entry.course == course))
                {
                    left.label(
                        egui::RichText::new(format!("Suggested from position: {}", suggested.coursename))
                            .color(theme::ACCENT)
                            .small(),
                    );
                }
                let courses = self.race_ghost_web_courses.clone();
                egui::ScrollArea::vertical()
                    .id_salt("race_ghost_courses")
                    .max_height(320.0)
                    .show(left, |ui| {
                    for course in courses {
                        let selected = self.race_ghost_web_selected_course.as_deref() == Some(course.course.as_str());
                        let suggested = self.race_ghost_web_suggested_course.as_deref() == Some(course.course.as_str());
                        let label = if suggested {
                            format!("{}  [suggested]", course.coursename)
                        } else {
                            course.coursename.clone()
                        };
                        let response = ui.selectable_label(selected, label);
                        let response = if suggested {
                            response.on_hover_text("Closest course start area to your current predicted position")
                        } else {
                            response
                        };
                        if response.clicked() && !selected {
                            select_course = Some(course.course);
                        }
                    }
                });

                left.add_space(12.0);
                left.heading("Loaded ghosts");
                if loaded.is_empty() {
                    left.label(egui::RichText::new("No race ghosts loaded.").weak());
                } else {
                    if left.button("Clear all ghosts").clicked() { clear_all = true; }
                    egui::ScrollArea::vertical()
                        .id_salt("race_ghost_loaded")
                        .max_height(180.0)
                        .show(left, |ui| {
                        for (key, label, duration_ms) in &loaded {
                            ui.horizontal(|ui| {
                                if ui.small_button("Unload").clicked() {
                                    unload_key = Some(key.clone());
                                }
                                ui.label(label);
                                ui.label(
                                    egui::RichText::new(format!("{:.3}s", f64::from(*duration_ms) / 1000.0))
                                        .weak()
                                        .small(),
                                );
                            });
                        }
                    });
                }

                let right = &mut columns[1];
                right.heading("Available demos");
                if let Some(course) = self.race_ghost_web_selected_course.as_deref() {
                    right.label(
                        egui::RichText::new(if course.is_empty() {
                            "Unnamed/default course".to_owned()
                        } else {
                            format!("Course: {course}")
                        })
                        .weak()
                        .small(),
                    );
                }
                if self.race_ghost_web_demos_pending {
                    right.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Loading demo list...");
                    });
                }
                if let Some(error) = self.race_ghost_web_error.as_deref() {
                    right.colored_label(egui::Color32::LIGHT_RED, error);
                }
                let demos = self.race_ghost_web_demos.clone();
                if demos.is_empty() && !self.race_ghost_web_demos_pending {
                    right.label(egui::RichText::new("No downloadable demos for this selection.").weak());
                }
                egui::ScrollArea::vertical()
                    .id_salt("race_ghost_available_demos")
                    .max_height(390.0)
                    .show(right, |ui| {
                    for demo in demos {
                        let is_loaded = loaded.iter().any(|(key, _, _)| key == &demo.url);
                        let pending = self.race_ghost_web_pending_demos.contains(&demo.url);
                        let active = is_loaded || pending;
                        let marker = if pending {
                            "[..]"
                        } else if is_loaded {
                            "[x]"
                        } else {
                            "[ ]"
                        };
                        let status = if pending {
                            "  downloading..."
                        } else if is_loaded {
                            "  loaded"
                        } else {
                            ""
                        };
                        let text = format!("{marker} {}{status}", demo.label);
                        let row_width = ui.available_width();
                        let response = ui
                            .add_sized(
                                [row_width, 25.0],
                                egui::Button::new(egui::RichText::new(text).monospace()),
                            )
                            .on_hover_text(demo.url.as_str());
                        if response.clicked() {
                            toggle_demo = Some((demo.clone(), !active));
                        }
                    }
                });

                right.add_space(12.0);
                right.heading("Appearance");
                alpha_changed |= right
                    .add(egui::Slider::new(&mut self.race_ghost_alpha, 0.02..=1.0).text("Ghost opacity"))
                    .changed();
                visuals_changed |= right.checkbox(&mut self.race_ghost_name, "Show archive username").changed();
                visuals_changed |= right.checkbox(&mut self.race_ghost_trail, "Show ghost strafe trail").changed();
                visuals_changed |= right
                    .checkbox(&mut self.race_ghost_velocity_delta, "Show speed delta")
                    .on_hover_text("Horizontal ghost speed minus your speed, in units per second.")
                    .changed();
                visuals_changed |= right
                    .checkbox(&mut self.race_ghost_distance_delta, "Show distance delta")
                    .on_hover_text("Current 3D separation between you and the synchronized ghost.")
                    .changed();
                right.label(
                    egui::RichText::new(
                        "Ghost trails reuse the Strafe Trails radius, opacity mode, draw distance, frustum culling, and angular LOD settings.",
                    )
                    .weak()
                    .small(),
                );
                right.label(
                    egui::RichText::new(
                        "Ghosts are visual-only: no collision, sounds, projectiles, server entities, or gameplay events.",
                    )
                    .weak()
                    .small(),
                );
            });
        });

        if apply_base {
            let value = self.race_ghost_demo_base_url_input.clone();
            match self.set_race_ghost_demo_base_url(&value) {
                Ok(()) => refresh = true,
                Err(error) => self.race_ghost_web_error = Some(error),
            }
        }
        if refresh { self.request_race_ghost_catalog(); }
        if let Some(course) = select_course { self.request_race_ghost_demos(course); }
        if let Some((demo, enabled)) = toggle_demo {
            if enabled {
                self.request_remote_race_ghost(demo);
            } else {
                self.unload_race_ghost_source(&demo.url);
            }
        }
        if let Some(key) = unload_key { self.unload_race_ghost_source(&key); }
        if clear_all { self.clear_race_ghosts(); }
        if alpha_changed {
            if let Some(session) = self.game_session.as_mut() {
                session.race_ghost_alpha = self.race_ghost_alpha;
            }
            self.mark_config_dirty();
        }
        if visuals_changed {
            self.mark_config_dirty();
        }
        if close { self.set_overlay(OverlayMode::None); }
    }
}
