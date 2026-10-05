//! Race ghost runtime.
use crate::app::{
    playerstate_vec3, race_ghost, race_ghost_web, thread, App, OverlayMode, Path,
    RaceGhostLoadResult,
};

impl App {
    pub(in crate::app) fn race_ghost_context(&self) -> Result<(String, i32, String), String> {
        let map = self
            .game_session
            .as_ref()
            .filter(|session| session.live)
            .ok_or_else(|| {
                "Race ghosts are available while playing a live or local race map.".to_owned()
            })?
            .map_name
            .clone()
            .ok_or_else(|| "Current map is not known yet.".to_owned())?;
        // Use the same playerState source as movement prediction/HUD. Remote
        // play prefers the predicted state; local play is already authoritative.
        let player_state = self
            .live_player_state()
            .ok_or_else(|| "Waiting for the first gameplay snapshot.".to_owned())?;
        let style_id = player_state.stats[crate::japro_cg::STAT_MOVEMENTSTYLE];
        let style = crate::japro_cg::movement_style_slug(style_id)
            .ok_or_else(|| format!("Unknown movement style {style_id}."))?;
        Ok((map, style_id, style.to_owned()))
    }

    pub(in crate::app) fn race_ghost_suggested_course(&self) -> Option<String> {
        let origin = self
            .live_player_state()
            .and_then(|state| playerstate_vec3(state, "origin"))?;
        race_ghost_web::nearest_course(&self.race_ghost_web_courses, origin)
            .map(|course| course.course.clone())
    }

    /// Re-evaluate the closest course start from the player's predicted origin.
    /// This is called when entering the menu (or after a fresh catalog arrives),
    /// not every frame, so a manual course selection remains stable while the
    /// player browses the menu.
    pub(in crate::app) fn select_suggested_race_ghost_course(&mut self) {
        let suggested = self.race_ghost_suggested_course();
        self.race_ghost_web_suggested_course = suggested.clone();
        let selected = suggested.or_else(|| {
            self.race_ghost_web_courses
                .first()
                .map(|course| course.course.clone())
        });
        let Some(selected) = selected else { return };
        if self.race_ghost_web_selected_course.as_deref() != Some(selected.as_str()) {
            self.request_race_ghost_demos(selected);
        }
    }

    pub(in crate::app) fn set_race_ghost_demo_base_url(
        &mut self,
        value: &str,
    ) -> Result<(), String> {
        let normalized = race_ghost_web::normalize_base_url(value)?;
        self.race_ghost_demo_base_url = normalized.clone();
        self.race_ghost_demo_base_url_input = normalized;
        self.race_ghost_web_generation = self.race_ghost_web_generation.wrapping_add(1);
        self.race_ghost_web_courses.clear();
        self.race_ghost_web_demos.clear();
        self.race_ghost_web_selected_course = None;
        self.race_ghost_web_suggested_course = None;
        self.race_ghost_web_active_map = None;
        self.race_ghost_web_active_style = None;
        self.race_ghost_web_catalog_pending = false;
        self.race_ghost_web_demos_pending = false;
        self.race_ghost_web_error = None;
        self.mark_config_dirty();
        Ok(())
    }

    pub(in crate::app) fn request_race_ghost_catalog(&mut self) {
        let (map, style_id, style) = match self.race_ghost_context() {
            Ok(context) => context,
            Err(error) => {
                self.race_ghost_web_error = Some(error);
                self.race_ghost_web_catalog_pending = false;
                return;
            }
        };
        let base_url = match race_ghost_web::normalize_base_url(&self.race_ghost_demo_base_url) {
            Ok(url) => url,
            Err(error) => {
                self.race_ghost_web_error = Some(error);
                return;
            }
        };
        self.race_ghost_web_generation = self.race_ghost_web_generation.wrapping_add(1);
        let generation = self.race_ghost_web_generation;
        self.race_ghost_web_active_map = Some(map.clone());
        self.race_ghost_web_active_style = Some(style.clone());
        self.race_ghost_web_catalog_pending = true;
        self.race_ghost_web_demos_pending = false;
        self.race_ghost_web_courses.clear();
        self.race_ghost_web_demos.clear();
        self.race_ghost_web_selected_course = None;
        self.race_ghost_web_suggested_course = None;
        self.race_ghost_web_error = None;
        let tx = self.race_ghost_web_tx.clone();
        let worker_map = map.clone();
        let worker_style = style.clone();
        if let Err(error) = thread::Builder::new()
            .name(format!(
                "race-ghost-index:{}",
                map.chars().take(20).collect::<String>()
            ))
            .spawn(move || {
                let result =
                    race_ghost_web::fetch_courses(&base_url, &worker_map, style_id, &worker_style);
                let _ = tx.send(race_ghost_web::WebResult::Courses {
                    generation,
                    map: worker_map,
                    style: worker_style,
                    result,
                });
            })
        {
            self.race_ghost_web_catalog_pending = false;
            self.race_ghost_web_error = Some(format!("Could not start catalog worker: {error}"));
        }
    }

    pub(in crate::app) fn request_race_ghost_demos(&mut self, course: String) {
        let (Some(map), Some(style)) = (
            self.race_ghost_web_active_map.clone(),
            self.race_ghost_web_active_style.clone(),
        ) else {
            return;
        };
        let base_url = self.race_ghost_demo_base_url.clone();
        let generation = self.race_ghost_web_generation;
        self.race_ghost_web_selected_course = Some(course.clone());
        self.race_ghost_web_demos.clear();
        self.race_ghost_web_demos_pending = true;
        self.race_ghost_web_error = None;
        let tx = self.race_ghost_web_tx.clone();
        let worker_course = course.clone();
        if let Err(error) = thread::Builder::new()
            .name(format!(
                "race-ghost-list:{}",
                course.chars().take(20).collect::<String>()
            ))
            .spawn(move || {
                let result = race_ghost_web::fetch_demos(&base_url, &map, &worker_course, &style);
                let _ = tx.send(race_ghost_web::WebResult::Demos {
                    generation,
                    map,
                    style,
                    course: worker_course,
                    result,
                });
            })
        {
            self.race_ghost_web_demos_pending = false;
            self.race_ghost_web_error = Some(format!("Could not start demo-list worker: {error}"));
        }
    }

    pub(in crate::app) fn request_remote_race_ghost(&mut self, demo: race_ghost_web::RemoteDemo) {
        let already_loaded = self.game_session.as_ref().is_some_and(|session| {
            session
                .race_ghosts
                .iter()
                .any(|ghost| ghost.track.source_key == demo.url)
        });
        if already_loaded || !self.race_ghost_web_pending_demos.insert(demo.url.clone()) {
            return;
        }
        let generation = self.race_ghost_load_generation;
        let source_key = demo.url.clone();
        let worker_key = source_key.clone();
        let display_name = demo.label.clone();
        let base_url = self.race_ghost_demo_base_url.clone();
        let tx = self.race_ghost_tx.clone();
        let cache_root = self.base.join("cache").join("race_ghosts");
        if let Err(error) = thread::Builder::new()
            .name("race-ghost-web".to_owned())
            .spawn(move || {
                let result = (|| {
                    let bytes =
                        race_ghost_web::fetch_demo_cached(&base_url, &worker_key, &cache_root)?;
                    let demo_name = race_ghost_web::demo_basename(&worker_key);
                    let mut track = race_ghost::parse_track(demo_name, bytes)?;
                    track.source_key = worker_key.clone();
                    track.display_name = display_name;
                    Ok(track)
                })();
                let _ = tx.send(RaceGhostLoadResult {
                    generation,
                    replace_existing: false,
                    source_key: worker_key,
                    result,
                });
            })
        {
            self.race_ghost_web_pending_demos.remove(&source_key);
            self.race_ghost_web_error = Some(format!("Could not start demo download: {error}"));
        }
    }

    pub(in crate::app) fn unload_race_ghost_source(&mut self, source_key: &str) {
        // Removing a pending URL doubles as cancellation: the worker may finish,
        // but poll_race_ghost_loads discards it if the URL is no longer pending.
        self.race_ghost_web_pending_demos.remove(source_key);
        if let Some(session) = self.game_session.as_mut() {
            session
                .race_ghosts
                .retain(|ghost| ghost.track.source_key != source_key);
        }
        self.egui_repaint_requested = true;
    }

    pub(in crate::app) fn clear_race_ghosts(&mut self) {
        self.race_ghost_load_generation = self.race_ghost_load_generation.wrapping_add(1);
        self.race_ghost_web_pending_demos.clear();
        let cleared = self
            .game_session
            .as_mut()
            .map(|session| {
                let count = session.race_ghosts.len();
                session.race_ghosts.clear();
                count
            })
            .unwrap_or(0);
        self.push_console_line(if cleared == 0 {
            "^3RACE GHOST:^7 no ghost was loaded".to_owned()
        } else {
            "^3RACE GHOST:^7 cleared".to_owned()
        });
    }

    pub(in crate::app) fn load_race_ghost_named(&mut self, argument: &str) {
        let Some(session) = self.game_session.as_ref().filter(|session| session.live) else {
            self.push_console_line(
                "^1RACE GHOST:^7 /rGhost is available while playing a live or local race map"
                    .to_owned(),
            );
            return;
        };
        if session.map_name.is_none() {
            self.push_console_line("^1RACE GHOST:^7 current map is not known yet".to_owned());
            return;
        }

        let argument = argument.trim().trim_matches('"');
        let mut name = argument.replace('\\', "/");
        if name
            .get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("demos/"))
        {
            name = name[6..].to_owned();
        }
        let demo_name = if name
            .get(name.len().saturating_sub(6)..)
            .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".dm_26"))
        {
            name[..name.len() - 6].to_owned()
        } else {
            name
        };
        if demo_name.is_empty()
            || Path::new(&demo_name).components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            self.push_console_line("^1RACE GHOST:^7 demo name must stay inside demos/".to_owned());
            return;
        }

        let qpath = format!("demos/{demo_name}.dm_26");
        // Console /rGhost preserves its v1 replacement behavior. Starting one
        // also invalidates any in-flight web additions from the previous set.
        self.race_ghost_load_generation = self.race_ghost_load_generation.wrapping_add(1);
        self.race_ghost_web_pending_demos.clear();
        let generation = self.race_ghost_load_generation;
        let tx = self.race_ghost_tx.clone();
        let source_key = format!("local:{demo_name}");
        let worker_key = source_key.clone();
        let worker_name = demo_name.clone();
        let worker_qpath = qpath.clone();
        let base = self.base.clone();
        let game = self.game.clone();
        self.push_console_line(format!("^3RACE GHOST:^7 loading {qpath}..."));
        if let Err(error) = thread::Builder::new()
            .name(format!(
                "race-ghost:{}",
                demo_name.chars().take(24).collect::<String>()
            ))
            .spawn(move || {
                const MAX_DEMO_FILE_BYTES: usize = 512 * 1024 * 1024;
                let result = (|| {
                    let mut assets =
                        jka_assets::pk3::AssetSearchPath::open_game(&base, game.as_deref())
                            .map_err(|error| format!("RACE GHOST ASSET PATH ERROR: {error}"))?;
                    let asset = assets
                        .read(&worker_qpath, MAX_DEMO_FILE_BYTES)
                        .map_err(|error| format!("RACE GHOST READ ERROR: {error}"))?
                        .ok_or_else(|| format!("RACE GHOST NOT FOUND: {worker_qpath}"))?;
                    let mut track = race_ghost::parse_track(worker_name.clone(), asset.bytes)?;
                    track.source_key = worker_key.clone();
                    track.display_name = worker_name;
                    Ok(track)
                })();
                let _ = tx.send(RaceGhostLoadResult {
                    generation,
                    replace_existing: true,
                    source_key: worker_key,
                    result,
                });
            })
        {
            self.push_console_line(format!("^1RACE GHOST WORKER ERROR:^7 {error}"));
        }
    }

    pub(in crate::app) fn poll_race_ghost_loads(&mut self) {
        while let Ok(done) = self.race_ghost_rx.try_recv() {
            if done.generation != self.race_ghost_load_generation {
                continue;
            }
            if !done.replace_existing && !self.race_ghost_web_pending_demos.remove(&done.source_key)
            {
                // The GUI unchecked this entry while its worker was in flight.
                continue;
            }
            let source_key = done.source_key.clone();
            let remote = !done.replace_existing;
            let message = match done.result {
                Err(error) => Err(error),
                Ok(track) => {
                    let Some(session) = self.game_session.as_mut().filter(|session| session.live)
                    else {
                        continue;
                    };
                    let Some(current_map) = session.map_name.clone() else {
                        continue;
                    };
                    if !current_map.eq_ignore_ascii_case(&track.map_name) {
                        Err(format!(
                            "RACE GHOST MAP MISMATCH: demo is {} but current map is {}",
                            track.map_name, current_map
                        ))
                    } else {
                        let duration = track.duration_ms();
                        let display_name = track.display_name.clone();
                        if done.replace_existing {
                            session.race_ghosts.clear();
                        } else {
                            session
                                .race_ghosts
                                .retain(|ghost| ghost.track.source_key != source_key);
                        }
                        let entity_num = (60_000u16..60_128u16)
                            .find(|candidate| {
                                session
                                    .race_ghosts
                                    .iter()
                                    .all(|ghost| ghost.entity_num != *candidate)
                            })
                            .ok_or_else(|| {
                                "RACE GHOST: too many simultaneous ghosts (128 max)".to_owned()
                            });
                        match entity_num.and_then(|entity_num| {
                            race_ghost::RaceGhost::from_track(
                                track,
                                &session.siege_classes,
                                entity_num,
                            )
                        }) {
                            Ok(ghost) => {
                                session.race_ghosts.push(ghost);
                                Ok(format!(
                                    "RACE GHOST: loaded {display_name} ({:.3}s) for {current_map}; it will appear when your race starts",
                                    f64::from(duration) / 1000.0
                                ))
                            }
                            Err(error) => Err(error),
                        }
                    }
                }
            };
            match message {
                Ok(line) => self.push_console_line(format!("^2{line}")),
                Err(line) => {
                    if remote {
                        self.race_ghost_web_error = Some(line.clone());
                    }
                    self.push_console_line(format!("^1{line}"));
                }
            }
            self.egui_repaint_requested = true;
        }
    }

    pub(in crate::app) fn poll_race_ghost_web(&mut self) {
        let events = self.race_ghost_web_rx.try_iter().collect::<Vec<_>>();
        let mut request_course = None::<String>;
        for event in events {
            match event {
                race_ghost_web::WebResult::Courses {
                    generation,
                    map,
                    style,
                    result,
                } => {
                    if generation != self.race_ghost_web_generation
                        || self.race_ghost_web_active_map.as_deref() != Some(map.as_str())
                        || self.race_ghost_web_active_style.as_deref() != Some(style.as_str())
                    {
                        continue;
                    }
                    self.race_ghost_web_catalog_pending = false;
                    match result {
                        Ok(courses) => {
                            self.race_ghost_web_courses = courses;
                            self.race_ghost_web_error = None;
                            let suggested = self.race_ghost_suggested_course();
                            self.race_ghost_web_suggested_course = suggested.clone();
                            let selected = suggested.or_else(|| {
                                self.race_ghost_web_courses
                                    .first()
                                    .map(|course| course.course.clone())
                            });
                            self.race_ghost_web_selected_course = selected.clone();
                            request_course = selected;
                        }
                        Err(error) => {
                            self.race_ghost_web_courses.clear();
                            self.race_ghost_web_demos.clear();
                            self.race_ghost_web_suggested_course = None;
                            self.race_ghost_web_error = Some(error);
                        }
                    }
                }
                race_ghost_web::WebResult::Demos {
                    generation,
                    map,
                    style,
                    course,
                    result,
                } => {
                    if generation != self.race_ghost_web_generation
                        || self.race_ghost_web_active_map.as_deref() != Some(map.as_str())
                        || self.race_ghost_web_active_style.as_deref() != Some(style.as_str())
                        || self.race_ghost_web_selected_course.as_deref() != Some(course.as_str())
                    {
                        continue;
                    }
                    self.race_ghost_web_demos_pending = false;
                    match result {
                        Ok(demos) => {
                            self.race_ghost_web_demos = demos;
                            self.race_ghost_web_error = None;
                        }
                        Err(error) => {
                            self.race_ghost_web_demos.clear();
                            self.race_ghost_web_error = Some(error);
                        }
                    }
                }
            }
        }
        if let Some(course) = request_course {
            self.request_race_ghost_demos(course);
        }
        if self.overlay == OverlayMode::RaceGhosts {
            self.egui_repaint_requested = true;
        }
    }
}
