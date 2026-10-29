//! Camera Director UI: add shots from the timeline header, let the Auto
//! Director cut demo footage (route view / route comparison), regenerate
//! shots, edit them as clips on the Camera track (move / trim, Ctrl/Shift
//! multi-select) and in the Inspector.
use super::App;
use super::render_studio_ui::palette as theme;
use super::render_studio_ui::{self as sui, ButtonKind, Icon};
use crate::app::render_studio_auto_director::{DirectorWorkerContext, StudioDirectorPacing};
use crate::app::render_studio_director::{StudioDirectorJob, StudioDirectorPreset, StudioDirectorShot, StudioShotLength,
    StudioRouteJob};
use crate::app::render_studio_timeline::{StudioDemoAnchor, StudioDemoClip, StudioFollowPerspective};

pub(super) const RECORDED_PLAYER: &str = "Recorded player (POV)";
pub(super) const FOLLOWED_PLAYER: &str = "Same as followed player";

/// Minimum shot length while trimming, seconds.
const MIN_SHOT: f64 = 0.1;

fn director_button(ui: &mut egui::Ui, locked: bool) -> egui::Response {
    ui.scope_builder(egui::UiBuilder::new().id(ui.make_persistent_id("studio_director_button")), |ui|
        sui::button(ui, Some(Icon::Camera), "Director", ButtonKind::Normal, !locked)).inner
        .on_hover_text("Cinematic camera shots over demo footage: let the Auto Director cut it, or add shots by hand")
}

/// The panel has its own open state; its nested selectors use egui's popup slot.
fn director_popup<R>(button: &egui::Response, open: &mut bool, content: impl FnOnce(&mut egui::Ui) -> R)
    -> Option<egui::InnerResponse<R>> {
    sui::settings_popup(button, open, |ui| {
        sui::editor_style(ui);
        ui.set_width(360.0);
        egui::ScrollArea::vertical().id_salt("studio_director_menu_scroll")
            .max_height((ui.ctx().content_rect().height() - 100.0).max(180.0)).show(ui, content).inner
    })
}

fn short_label(preset: StudioDirectorPreset) -> &'static str {
    match preset {
        StudioDirectorPreset::PlayerEyes => "Eyes",
        StudioDirectorPreset::RecordedPov => "Recorded",
        StudioDirectorPreset::Establishing => "Establish",
        StudioDirectorPreset::OverShoulder => "Shoulder",
        StudioDirectorPreset::TwoShot => "Two-shot",
        StudioDirectorPreset::SaberCam => "Saber",
        StudioDirectorPreset::VictimPov => "Victim",
        StudioDirectorPreset::VictimBlade => "Blade view",
        StudioDirectorPreset::ThrowCam => "Throw",
        StudioDirectorPreset::FollowCam => "Follow",
        other => other.label(),
    }
}

/// Types whose framing comes from the subject's recorded path (placement or
/// travel direction). Chase and the first-person shots need none.
fn needs_placement(preset: StudioDirectorPreset) -> bool {
    !matches!(preset, StudioDirectorPreset::Chase | StudioDirectorPreset::PlayerEyes | StudioDirectorPreset::RecordedPov)
}

#[derive(Clone, Copy, PartialEq)]
enum ShotAction { Regenerate, Retype(StudioDirectorPreset), Toggle, Front, Duplicate, Delete }

impl App {
    pub(super) fn studio_director_index(&self) -> Option<usize> {
        let id = self.render_studio.director_selected?;
        self.render_studio.timeline.director_shots.iter().position(|s| s.id == id)
    }

    /// Selected shot ids: the multi-selection, else the single primary.
    pub(super) fn studio_director_selected_ids(&self) -> Vec<u64> {
        let shots = &self.render_studio.timeline.director_shots;
        let exists = |id: &u64| shots.iter().any(|s| s.id == *id);
        let Some(primary) = self.render_studio.director_selected.filter(exists) else { return Vec::new() };
        if self.render_studio.director_multi.contains(&primary) {
            self.render_studio.director_multi.iter().copied().filter(exists).collect()
        } else { vec![primary] }
    }

    /// Visible players of the demo under the playhead: (client, name).
    /// All players of the recording at the playhead: (client, name, in view).
    pub(super) fn studio_demo_all_players(&self) -> Vec<(i32, String, bool)> {
        self.render_studio.sources.iter()
            .find(|source| self.render_studio.scene_frame.as_ref().is_some_and(|f| f.outgoing.id == source.id))
            .map(|source| source.playback.demo_all_players())
            .unwrap_or_default()
    }

    pub(super) fn studio_demo_players(&self) -> Vec<(i32, String)> {
        self.render_studio.sources.iter()
            .find(|source| self.render_studio.scene_frame.as_ref().is_some_and(|f| f.outgoing.id == source.id))
            .map(|source| source.playback.demo_visible_players())
            .unwrap_or_default()
    }

    pub(super) fn studio_player_label(players: &[(i32, String)], client: i32) -> String {
        if client < 0 { return "recorded player".into(); }
        players.iter().find(|(id, _)| *id == client)
            .map_or_else(|| format!("client {client}"), |(_, name)| strip_colors(name))
    }

    /// Select a shot: it owns the Inspector until something else is picked.
    pub(super) fn studio_select_director(&mut self, id: u64) {
        self.render_studio.director_selected = Some(id);
        self.render_studio.director_multi = vec![id];
        self.render_studio.director_anchor = Some(id);
        self.render_studio.timeline.clear_selection();
        self.render_studio.timeline.selected_demo = None;
        self.render_studio.selected_background = false;
        self.render_studio.audio.selected = None;
        self.render_studio.audio.selected_track = None;
        self.render_studio.audio.selected_demo_audio = None;
    }

    /// Click on a shot: Ctrl toggles it, Shift selects every shot between the
    /// anchor and this one (inclusive, by start time).
    pub(super) fn studio_click_director(&mut self, id: u64, ctrl: bool, shift: bool) {
        let previous = self.studio_director_selected_ids();
        let shots = &self.render_studio.timeline.director_shots;
        let anchor = self.render_studio.director_anchor.filter(|a| shots.iter().any(|s| s.id == *a));
        let mut selected = if ctrl { previous } else { Vec::new() };
        if let (true, Some(anchor)) = (shift, anchor) {
            let time = |id: u64| shots.iter().find(|s| s.id == id).map_or(0.0, |s| s.start);
            let (a, b) = (time(anchor), time(id));
            let mut range: Vec<&StudioDirectorShot> = shots.iter()
                .filter(|s| s.start >= a.min(b) - 1e-9 && s.start <= a.max(b) + 1e-9).collect();
            range.sort_by(|x, y| x.start.total_cmp(&y.start));
            for shot in range { if !selected.contains(&shot.id) { selected.push(shot.id); } }
        } else if ctrl && selected.contains(&id) {
            selected.retain(|&other| other != id);
        } else if !selected.contains(&id) {
            selected.push(id);
        }
        let primary = if selected.contains(&id) { Some(id) } else { selected.last().copied() };
        match primary {
            Some(primary) => self.studio_select_director(primary),
            None => { self.render_studio.director_selected = None; }
        }
        self.render_studio.director_multi = selected;
        self.render_studio.director_anchor = if shift { anchor.or(Some(id)) } else { Some(id) };
    }

    pub(super) fn studio_select_all_directors(&mut self) {
        let mut shots: Vec<&StudioDirectorShot> = self.render_studio.timeline.director_shots.iter().collect();
        shots.sort_by(|a, b| a.start.total_cmp(&b.start));
        let ids: Vec<u64> = shots.iter().map(|s| s.id).collect();
        if let Some(&first) = ids.first() {
            self.studio_select_director(first);
            self.render_studio.director_multi = ids;
        }
    }

    /// Another selection took the Inspector: drop the shot selection.
    pub(super) fn studio_director_sync_selection(&mut self) {
        let other = !self.render_studio.timeline.selected_items().is_empty()
            || self.render_studio.timeline.selected_demo.is_some()
            || self.render_studio.selected_background
            || self.render_studio.audio.selected.is_some()
            || self.render_studio.audio.selected_track.is_some();
        if other || self.studio_director_index().is_none() {
            self.render_studio.director_selected = None;
            self.render_studio.director_multi.clear();
        }
    }

    pub(super) fn studio_delete_director(&mut self, index: usize) {
        if let Some(id) = self.render_studio.timeline.director_shots.get(index).map(|s| s.id) {
            self.studio_delete_directors(&[id]);
        }
    }

    pub(super) fn studio_delete_directors(&mut self, ids: &[u64]) {
        let before = self.render_studio.timeline.director_shots.len();
        self.render_studio.timeline.director_shots.retain(|s| !ids.contains(&s.id));
        if self.render_studio.timeline.director_shots.len() != before {
            self.render_studio.director_selected = None;
            self.render_studio.director_multi.clear();
            self.profile_preview_key = None;
        }
    }

    /// Shot type chips: three rows of five, follow shots first.
    pub(super) fn studio_preset_picker(ui: &mut egui::Ui, value: &mut StudioDirectorPreset) -> bool {
        let follow = StudioDirectorPreset::FOLLOW;
        let rows: [&[StudioDirectorPreset]; 4] = [&follow[..5], &follow[5..10], &follow[10..], &StudioDirectorPreset::WORLD];
        let mut changed = false;
        for (index, row) in rows.iter().enumerate() {
            if index == 3 { sui::hint(ui, "Placed in the map"); }
            let options: Vec<_> = row.iter().map(|p| (*p, short_label(*p))).collect();
            changed |= sui::segmented(ui, value, &options);
        }
        changed
    }

    /// Timeline header: "Director" menu (Auto Director, new shot, route comparison).
    pub(super) fn egui_studio_director_menu(&mut self, ui: &mut egui::Ui, locked: bool) {
        let button = director_button(ui, locked);
        let mut open = self.render_studio.director_popup_open;
        director_popup(&button, &mut open, |ui| {
                self.egui_studio_auto_director(ui);
                sui::divider(ui);
                sui::group_label(ui, "New shot at the playhead");
                Self::studio_preset_picker(ui, &mut self.render_studio.director_new_preset);
                sui::hint(ui, self.render_studio.director_new_preset.describe());
                sui::prop_segmented(ui, "Length", &mut self.render_studio.director_new_length, &[
                    (StudioShotLength::ToClipEnd, "To clip end"), (StudioShotLength::WholeClip, "Whole clip"),
                    (StudioShotLength::FourSeconds, "4 s")]);
                let players: Vec<(i32, String)> = self.studio_demo_all_players().into_iter().map(|(c, n, _)| (c, n)).collect();
                let mut client = self.render_studio.director_new_client;
                Self::studio_client_menu_row(ui, "Player", &players, &mut client);
                self.render_studio.director_new_client = client;
                let time = self.render_studio.timeline_time();
                let on_demo = self.render_studio.timeline.footage_at(time, None).is_some();
                if !on_demo {
                    sui::callout(ui, "Move the playhead onto demo or animation footage first.", theme::WARNING);
                }
                if sui::button_row(ui, &[(Some(Icon::Plus), "Add at playhead", ButtonKind::Primary, on_demo)]).is_some() {
                    let preset = self.render_studio.director_new_preset;
                    self.studio_add_director_at(preset, client, time, &players);
                    ui.close();
                }
                sui::divider(ui);
                self.egui_studio_route_comparison(ui);
            });
        self.render_studio.director_popup_open = open;
    }

    /// Add a shot and, for path-framed types on the recorded player, place it.
    pub(super) fn studio_add_director_at(&mut self, preset: StudioDirectorPreset, client: i32, time: f64,
        players: &[(i32, String)]) {
        let Some(index) = self.studio_add_director_shot(preset, client, time) else {
            self.render_studio.status = "Not enough demo footage after the playhead for a shot".into();
            return;
        };
        let id = self.render_studio.timeline.director_shots[index].id;
        self.studio_select_director(id);
        self.profile_preview_key = None;
        self.render_studio.status = format!("Added a {} shot on {}", preset.label(), Self::studio_player_label(players, client));
        let stage = self.render_studio.timeline.demo_at(time).is_none();
        if needs_placement(preset) && (client < 0 || stage) {
            self.studio_start_regenerate(vec![id], Some(preset));
        } else if preset.world_anchored() {
            self.render_studio.status.push_str(" · world shots are placed from the recorded player's path, so it orbits until you pick Recorded player");
        }
    }

    /// Demo clips the Auto Director works on: the selected ones, else all.
    pub(super) fn studio_route_view_targets(&self) -> Vec<usize> {
        let selected = self.render_studio.timeline.selected_demo_indices();
        if selected.is_empty() { (0..self.render_studio.timeline.demos.len()).collect() } else { selected }
    }

    fn egui_studio_auto_director(&mut self, ui: &mut egui::Ui) {
        sui::group_label(ui, "Auto director");
        let targets = self.studio_route_view_targets();
        let busy = self.render_studio.director_job.is_some() || self.render_studio.route_job.is_some();
        sui::prop_segmented(ui, "Pacing", &mut self.render_studio.director_pacing, &[
            (StudioDirectorPacing::Calm, "Calm"), (StudioDirectorPacing::Balanced, "Balanced"),
            (StudioDirectorPacing::Energetic, "Energetic")]);
        if self.render_studio.timeline.demos.len() == 1 {
            sui::prop_toggle(ui, "Trim to the run", &mut self.render_studio.director_trim_run)
                .on_hover_text("Trim the clip to the race: 2 s before the start to 2 s after the finish (when the demo has one)");
        }
        let all = self.render_studio.timeline.demos.len();
        sui::hint(ui, &match targets.len() {
            0 => "Drop a demo on the timeline first.".to_owned(),
            n if n == all && self.render_studio.timeline.selected_demo.is_none() =>
                format!("Directs {} demo clip{} (select clips to direct only those).", n, if n == 1 { "" } else { "s" }),
            n => format!("Directs the {n} selected demo clip{}.", if n == 1 { "" } else { "s" }),
        });
        {
            use crate::app::render_studio_auto_director::StudioDirectorFocus as F;
            let players = self.studio_demo_all_players();
            let focus = &mut self.render_studio.director_focus;
            // Inline list, not a combo box or submenu: those are separate
            // popup layers, so picking from them closed this menu.
            let current = match *focus {
                F::Recorded => RECORDED_PLAYER.to_owned(),
                F::BestCombat => "Best fight (any player)".to_owned(),
                F::Player(c) => players.iter().find(|p| p.0 == c).map_or_else(|| format!("Client {c}"), |p| strip_colors(&p.1)),
            };
            egui::CollapsingHeader::new(format!("Focus: {current}")).id_salt("studio_director_focus").show(ui, |ui| {
                ui.selectable_value(focus, F::Recorded, RECORDED_PLAYER)
                    .on_hover_text("Cut around the demo's recorded player");
                ui.selectable_value(focus, F::BestCombat, "Best fight (any player)")
                    .on_hover_text("Follow whoever is in the most intense fight, jumping between players (combat view)");
                for (c, name, _) in &players {
                    ui.selectable_value(focus, F::Player(*c), player_row_text(name));
                }
                if players.is_empty() {
                    ui.label(egui::RichText::new("Move the playhead onto demo footage to list its players")
                        .size(11.5).color(theme::TEXT_FAINT));
                }
            });
        }
        if busy { ui.horizontal(|ui| { ui.spinner(); sui::hint(ui, "Directing…"); }); }
        let ready = !targets.is_empty() && !busy && !self.studio_locked();
        match sui::button_row(ui, &[(Some(Icon::Camera), "Generate route view", ButtonKind::Primary, ready),
            (Some(Icon::Camera), "Generate combat view", ButtonKind::Primary, ready)]) {
            Some(0) => { self.studio_start_route_view(targets); ui.close(); }
            Some(1) => { self.studio_start_combat_view(targets); ui.close(); }
            _ => {}
        }
        sui::prop_toggle(ui, "Slow-mo kills", &mut self.render_studio.director_slowmo)
            .on_hover_text("Combat view: replace the clip's speed ramps with slow motion around kills; holds through a multikill sequence. The result must fit before the next clip.");
        sui::prop_toggle(ui, "Fast downtime", &mut self.render_studio.director_fastforward)
            .on_hover_text("Combat view: replace the clip's speed ramps, speeding up confirmed empty stretches at 2.5x and empty respawn runs immediately at 3x. Nearby fights stay at normal speed.");
        sui::hint(ui, "Route view cuts movement: an establishing opener, tracking on fast straights, pans and fly-bys \
            over big air, overheads in turns, close-ups when slow and a hero finish. Combat view finds the fights \
            (kills, saber hits and clashes, players close together): a two-shot as they close in, over-the-shoulder \
            and reverse angles while trading blows, a kill cam on the victim and a hero angle after the recorder's \
            kills. Generate again for a different cut; R regenerates the selected shots. Ctrl+Z undoes.");
        let clips = self.studio_stage_targets();
        if !self.render_studio.timeline.clips.is_empty() {
            let label = match clips.len() {
                0 => "Generate shots for clip".to_owned(),
                1 => "Generate shots for this clip".to_owned(),
                n => format!("Generate shots for {n} clips"),
            };
            if sui::button_row(ui, &[(Some(Icon::Camera), label.as_str(), ButtonKind::Normal, !clips.is_empty() && !self.studio_locked())]).is_some() {
                self.studio_direct_stage_clips(clips);
                ui.close();
            }
            sui::hint(ui, "Animations and emotes: a shot sequence on the character stage (wide opener, close-ups, low angles, \
                orbits and push-ins, tracking or pans when the actor moves, a pull-out to end). Uses the selected clips, \
                else the clip under the playhead.");
        }
    }

    /// Animation clips the stage Auto Director works on: selected clips, else
    /// the clip under the playhead (lead track first).
    pub(super) fn studio_stage_targets(&self) -> Vec<usize> {
        use crate::app::render_studio_timeline::StudioItem;
        let timeline = &self.render_studio.timeline;
        let selected: Vec<usize> = timeline.selected_items().iter()
            .filter_map(|item| match item { StudioItem::Clip(i) => Some(*i), _ => None }).collect();
        if !selected.is_empty() { return selected; }
        let time = self.render_studio.timeline_time();
        timeline.active_actors(time).first()
            .and_then(|clip| timeline.clips.iter().position(|c| std::ptr::eq(c, *clip))).into_iter().collect()
    }

    /// Distinct recordings on the timeline: (name, game, source length).
    pub(in crate::app) fn studio_route_inputs(&self) -> Vec<(String, String, i32)> {
        let mut out: Vec<(String, String, i32)> = Vec::new();
        for clip in &self.render_studio.timeline.demos {
            if let Some(race) = &clip.comparison {
                for runner in &race.runners {
                    if !out.iter().any(|(n,g,_)| *n==runner.name && *g==runner.game) {
                        out.push((runner.name.clone(),runner.game.clone(),runner.duration_ms));
                    }
                }
                continue;
            }
            if !out.iter().any(|(n, g, _)| *n == clip.name && *g == clip.game) {
                out.push((clip.name.clone(), clip.game.clone(), clip.duration_ms));
            }
        }
        out
    }

    /// Route Comparison: re-author the demo track from 2-32 runs of one course.
    fn egui_studio_route_comparison(&mut self, ui: &mut egui::Ui) {
        sui::group_label(ui, "Route comparison");
        let inputs = self.studio_route_inputs();
        let running = self.render_studio.route_job.is_some() || self.render_studio.director_job.is_some();
        sui::hint(ui, &if inputs.is_empty() {
            "Drop 2-32 demos of the same defrag course on the timeline (any order or position).".to_owned()
        } else {
            format!("{} distinct recordings · supports 2–32", inputs.len())
        });
        let mut choices = vec![(None, "Fastest run (automatic)".to_owned())];
        choices.extend(inputs.iter().map(|(name, game, _)| (Some((name.clone(), game.clone())),
            format!("{} / {name}", if game.is_empty() { "base" } else { game }))));
        let choices: Vec<_> = choices.iter().map(|(key, label)| (key.clone(), label.as_str())).collect();
        sui::prop_combo(ui, "Reference runner", "studio_route_main", &mut self.render_studio.route_options.main, &choices);
        sui::prop_toggle(ui, "Occasional camera orbits", &mut self.render_studio.route_options.moving_camera)
            .on_hover_text("Chase the reference runner, with smooth orbit moves to reveal nearby ghosts. The runner stays in frame.");
        sui::prop_toggle(ui, "Strafe trails", &mut self.render_studio.route_options.replay_trails);
        sui::hint(ui, "The followed runner stays opaque; companions remain visible as ghosts. Replays feature the fastest runner on each different route, with all ghosts moving at actual speeds. The reference line is not repeated.");
        sui::prop_slider(ui, "Route tolerance", egui::Slider::new(&mut self.render_studio.route_threshold, 64.0..=4096.0)
            .logarithmic(true).suffix(" u"))
            .on_hover_text("Route differences sustained for 0.45 s within this distance (map units) count as the same route. \
                Lower tolerance keeps more routes separate for more replays and a longer edit; higher tolerance merges them for a shorter edit. \
                Click Generate route comparison to rebuild the timeline with this value.");
        let main_exists = self.render_studio.route_options.main.as_ref()
            .is_none_or(|(name, game)| inputs.iter().any(|(n, g, _)| n == name && g == game));
        let ready = (2..=32).contains(&inputs.len()) && main_exists && !running && !self.studio_locked();
        if !main_exists { sui::callout(ui, "Choose a main POV recording that is still on the timeline.", theme::WARNING); }
        if !inputs.is_empty() && !(2..=32).contains(&inputs.len()) {
            sui::callout(ui, "Route comparison needs 2–32 distinct recordings on the timeline.", theme::WARNING);
        }
        if running {
            ui.horizontal(|ui| { ui.spinner(); sui::hint(ui, "Waiting for Director analysis…"); });
        }
        if sui::button_row(ui, &[(Some(Icon::Camera), "Generate route comparison", ButtonKind::Primary, ready)]).is_some() {
            self.studio_start_route_job(inputs);
            ui.close();
        }
        sui::hint(ui, "Creates one ghost race: chase → slow down → pause → visible rewind → simultaneous section replay → \
            return to race. Covers complete fork-to-rejoin sections, with a fast, smooth rewind before each alternate route. Replaces the demo track and Director shots; Ctrl+Z restores the original.");
    }

    /// Collision of an already loaded recording (saves reading the BSP).
    fn studio_loaded_collision(&self, name: &str, game: &str) -> Option<jka_movement::CollisionWorld> {
        self.render_studio.sources.iter()
            .find(|s| s.name == name && s.game == game && s.collision.is_some())
            .and_then(|s| s.collision.clone())
    }

    fn studio_director_context(&mut self, name: &str, game: &str) -> DirectorWorkerContext {
        let seed = self.render_studio.director_seed;
        DirectorWorkerContext {
            base: self.base.clone(),
            project_dir: self.studio_project_directory(),
            paths: self.render_studio.director_paths.clone(),
            collision: self.render_studio.director_collision.clone(),
            loaded: self.studio_loaded_collision(name, game),
            fov: self.studio_default_fov(),
            pacing: self.render_studio.director_pacing,
            seed,
            focus: self.render_studio.director_focus,
        }
    }

    pub(in crate::app) fn studio_start_route_job(&mut self, inputs: Vec<(String, String, i32)>) {
        let threshold = self.render_studio.route_threshold;
        let options = self.render_studio.route_options.clone();
        let prior_clips = self.render_studio.timeline.demos.clone();
        let signature = self.render_studio.edit_snapshot().signature();
        let main = options.main.as_ref().map(|(name, game)| inputs.iter().position(|(n, g, _)| n == name && g == game));
        if matches!(main, Some(None)) {
            self.render_studio.status = "Choose a main POV recording that is still on the timeline".into();
            return;
        }
        let main = main.flatten();
        let (name, game) = inputs.get(main.unwrap_or(0)).map(|(n, g, _)| (n.clone(), g.clone())).unwrap_or_default();
        let ctx = self.studio_director_context(&name, &game);
        let media = prior_clips.iter().filter_map(|c|c.source_path.as_ref()
            .map(|path|((c.name.clone(),c.game.clone()),path.clone())))
            .collect::<std::collections::HashMap<_,_>>();
        self.render_studio.director_seed = ctx.seed.wrapping_add(1);
        let viewport = crate::app::render_studio::studio_world_frame_size(
            self.render_studio.capture_size, self.render_studio.demo_viewport);
        let worker = std::thread::Builder::new().name("studio-route-comparison".into()).spawn(move || {
            use crate::app::render_studio_director as director;
            let runs = director::load_route_runs(ctx.base.clone(),ctx.project_dir.clone(),media,inputs)?;
            let mut plan = director::build_route_comparison_plan(&runs, threshold, main)?;
            director::inherit_route_presentation(&mut plan, &prior_clips, &options);
            director::direct_route_plan(&mut plan, ctx.fov, ctx.seed);
            let pov = &runs[main.unwrap_or(0)];
            let world = crate::app::render_studio_auto_director::load_collision(
                &ctx.base, &ctx.collision, &pov.game, &pov.map, ctx.loaded);
            director::bake_route_camera(&mut plan, &runs, world, viewport);
            Ok(plan)
        });
        match worker {
            Ok(worker) => {
                self.render_studio.route_job = Some(StudioRouteJob { worker, signature });
                self.render_studio.status = "Route comparison: reading race segments…".into();
            }
            Err(error) => self.render_studio.status = format!("Route comparison could not start: {error}"),
        }
    }

    /// Commit a finished analysis, atomically and only onto the timeline it
    /// was computed from. A failed analysis never touches the timeline.
    pub(super) fn studio_poll_route_job(&mut self, ctx: &egui::Context) {
        let finished = self.render_studio.route_job.as_ref().is_some_and(|job| job.worker.is_finished());
        if !finished {
            if self.render_studio.route_job.is_some() { ctx.request_repaint_after(std::time::Duration::from_millis(100)); }
            return;
        }
        let job = self.render_studio.route_job.take().unwrap();
        let result = job.worker.join().unwrap_or_else(|_| Err("Route analysis worker panicked".into()));
        let plan = match result {
            Ok(plan) => plan,
            Err(error) => { self.render_studio.status = format!("Route comparison: {error}"); return; }
        };
        if self.render_studio.edit_snapshot().signature() != job.signature {
            self.render_studio.status = "Route comparison discarded: the timeline changed during analysis. Run it again.".into();
            return;
        }
        let timeline = &mut self.render_studio.timeline;
        timeline.clear_selection();
        timeline.selected_demo = None;
        timeline.demos = plan.clips;
        timeline.demo_layer_pinned.clear();
        timeline.demo_layer_hidden.clear();
        timeline.director_shots = plan.shots;
        for (index, shot) in timeline.director_shots.iter_mut().enumerate() { shot.id = index as u64 + 1; }
        timeline.in_point = 0.0;
        timeline.out_point = None;
        timeline.seek(0.0);
        self.render_studio.director_selected = None;
        self.render_studio.director_multi.clear();
        // The edit changes clip borrowers, not the loaded recording assets.
        self.render_studio.scene_frame = None;
        self.render_studio.source_wait = None;
        self.render_studio.spatial.scene_camera=None;
        self.render_studio.spatial.drag=None;
        self.profile_preview_key = None;
        self.render_studio.status = plan.summary;
        self.render_studio.history.dirty = true;
    }

    /// Route view: Auto Director over whole demo clips (no re-cut of the track).
    pub(in crate::app) fn studio_start_route_view(&mut self, targets: Vec<usize>) {
        if self.render_studio.director_job.is_some() { self.render_studio.status = "The Auto Director is still working".into(); return; }
        let clips: Vec<(usize, StudioDemoClip)> = targets.iter()
            .filter_map(|&i| self.render_studio.timeline.demos.get(i).cloned().map(|c| (i, c))).collect();
        let Some((_, first)) = clips.first().cloned() else {
            self.render_studio.status = "Drop a demo on the timeline first".into();
            return;
        };
        let trim = self.render_studio.director_trim_run && self.render_studio.timeline.demos.len() == 1;
        let ctx = self.studio_director_context(&first.name, &first.game);
        self.render_studio.director_seed = ctx.seed.wrapping_add(1);
        let signature = self.render_studio.edit_snapshot().signature();
        let worker = std::thread::Builder::new().name("studio-auto-director".into())
            .spawn(move || crate::app::render_studio_auto_director::route_view_plan(ctx, clips, trim));
        match worker {
            Ok(worker) => {
                self.render_studio.director_job = Some(StudioDirectorJob { worker, signature });
                self.render_studio.status = "Auto Director: reading the player's path…".into();
            }
            Err(error) => self.render_studio.status = format!("Auto Director could not start: {error}"),
        }
    }

    /// Combat view: Auto Director over fights in whole demo clips.
    pub(in crate::app) fn studio_start_combat_view(&mut self, targets: Vec<usize>) {
        if self.render_studio.director_job.is_some() { self.render_studio.status = "The Auto Director is still working".into(); return; }
        let timeline = &self.render_studio.timeline;
        // Slow motion may lengthen a clip only up to the next clip on its layer.
        let clips: Vec<(usize, StudioDemoClip, f64)> = targets.iter().filter_map(|&i| {
            let clip = timeline.demos.get(i)?.clone();
            let limit = timeline.demos.iter().enumerate()
                .filter(|(j, c)| *j != i && c.layer == clip.layer && c.start >= clip.end() - 1e-6)
                .map(|(_, c)| c.start).fold(36000.0, f64::min);
            Some((i, clip, limit))
        }).collect();
        let Some((_, first, _)) = clips.first().cloned() else {
            self.render_studio.status = "Drop a demo on the timeline first".into();
            return;
        };
        let slowmo = self.render_studio.director_slowmo;
        let fast_forward = self.render_studio.director_fastforward;
        let ctx = self.studio_director_context(&first.name, &first.game);
        self.render_studio.director_seed = ctx.seed.wrapping_add(1);
        let signature = self.render_studio.edit_snapshot().signature();
        let worker = std::thread::Builder::new().name("studio-auto-director".into())
            .spawn(move || crate::app::render_studio_auto_director::combat_view_plan(ctx, clips, slowmo, fast_forward));
        match worker {
            Ok(worker) => {
                self.render_studio.director_job = Some(StudioDirectorJob { worker, signature });
                self.render_studio.status = "Auto Director: reading fights from the recording…".into();
            }
            Err(error) => self.render_studio.status = format!("Auto Director could not start: {error}"),
        }
    }

    /// Regenerate shots with a different decision, or (`keep`) re-place them
    /// as that type from the recorded path.
    pub(in crate::app) fn studio_start_regenerate(&mut self, ids: Vec<u64>, keep: Option<StudioDirectorPreset>) {
        // Stage shots (over animation footage) are rebuilt right away.
        let (stage, ids): (Vec<u64>, Vec<u64>) = ids.into_iter().partition(|id| {
            let timeline = &self.render_studio.timeline;
            timeline.director_shots.iter().find(|s| s.id == *id)
                .is_some_and(|s| timeline.demo_at(s.start + s.duration * 0.5).is_none())
        });
        if !stage.is_empty() {
            let done = self.studio_regenerate_stage(&stage, keep);
            self.render_studio.status = match (done, keep) {
                (0, _) => "No animation under that shot to frame".into(),
                (n, Some(preset)) => format!("Placed {n} {} shot{}", preset.label(), if n == 1 { "" } else { "s" }),
                (n, None) => format!("Regenerated {n} shot{}", if n == 1 { "" } else { "s" }),
            };
            if ids.is_empty() { return; }
        }
        if self.render_studio.director_job.is_some() { self.render_studio.status = "The Auto Director is still working".into(); return; }
        let timeline = &self.render_studio.timeline;
        let shots = &timeline.director_shots;
        let items: Vec<_> = ids.iter().filter_map(|id| {
            let shot = shots.iter().find(|s| s.id == *id)?.clone();
            let clip = timeline.demo_at(shot.start + shot.duration * 0.5)?.clone();
            let prev = shots.iter().filter(|s| s.id != shot.id && s.end() <= shot.start + 0.05)
                .max_by(|a, b| a.end().total_cmp(&b.end())).map(|s| s.preset);
            let next = shots.iter().filter(|s| s.id != shot.id && s.start >= shot.end() - 0.05)
                .min_by(|a, b| a.start.total_cmp(&b.start)).map(|s| s.preset);
            Some((shot, clip, [prev, next]))
        }).collect();
        let Some((_, first, _)) = items.first().cloned() else {
            self.render_studio.status = "Regenerate works on Director shots over demo footage".into();
            return;
        };
        let ctx = self.studio_director_context(&first.name, &first.game);
        let signature = self.render_studio.edit_snapshot().signature();
        let worker = std::thread::Builder::new().name("studio-auto-director".into())
            .spawn(move || crate::app::render_studio_auto_director::regenerate_plan(ctx, items, keep));
        match worker {
            Ok(worker) => self.render_studio.director_job = Some(StudioDirectorJob { worker, signature }),
            Err(error) => self.render_studio.status = format!("Auto Director could not start: {error}"),
        }
    }

    pub(super) fn studio_poll_director_job(&mut self, ctx: &egui::Context) {
        let finished = self.render_studio.director_job.as_ref().is_some_and(|job| job.worker.is_finished());
        if !finished {
            if self.render_studio.director_job.is_some() { ctx.request_repaint_after(std::time::Duration::from_millis(60)); }
            return;
        }
        let job = self.render_studio.director_job.take().unwrap();
        let plan = match job.worker.join().unwrap_or_else(|_| Err("Auto Director worker panicked".into())) {
            Ok(plan) => plan,
            Err(error) => { self.render_studio.status = format!("Auto Director: {error}"); return; }
        };
        if self.render_studio.edit_snapshot().signature() != job.signature {
            self.render_studio.status = "Auto Director result discarded: the timeline changed meanwhile. Run it again.".into();
            return;
        }
        let timeline = &mut self.render_studio.timeline;
        for (index, keys) in plan.speed_keys {
            if let Some(clip) = timeline.demos.get(index) {
                let mut slowed = clip.clone();
                slowed.speed_keys = keys;
                let mut check = timeline.demos.clone();
                check[index] = slowed.clone();
                if slowed.valid() && crate::app::render_studio_timeline::StudioTimeline::demo_overlap_valid(&check) {
                    timeline.demos[index] = slowed;
                }
            }
        }
        for (index, in_ms, out_ms) in plan.trims {
            if let Some(clip) = timeline.demos.get(index) {
                let mut trimmed = clip.clone();
                trimmed.in_ms = in_ms;
                trimmed.out_ms = out_ms;
                if trimmed.valid() { timeline.demos[index] = trimmed; }
            }
        }
        let route_view = !plan.added.is_empty();
        if !plan.clear_ranges.is_empty() {
            timeline.director_shots.retain(|s| !plan.clear_ranges.iter()
                .any(|&(a, b)| s.start < b - 1e-6 && s.end() > a + 1e-6));
        }
        for replacement in plan.replaced {
            if let Some(slot) = timeline.director_shots.iter_mut().find(|s| s.id == replacement.id) { *slot = replacement; }
        }
        for mut shot in plan.added {
            shot.id = timeline.next_director_id();
            timeline.director_shots.push(shot);
        }
        if route_view {
            let start = timeline.director_shots.iter().map(|s| s.start).fold(f64::INFINITY, f64::min);
            if start.is_finite() { timeline.seek(start); }
            self.render_studio.director_selected = None;
            self.render_studio.director_multi.clear();
        }
        self.profile_preview_key = None;
        self.render_studio.status = plan.summary;
        self.render_studio.history.dirty = true;
    }

    /// Player picker inside a menu: an inline list (a combo box or submenu is
    /// its own popup layer and closed the menu on pick).
    pub(super) fn studio_client_menu_row(ui: &mut egui::Ui, label: &str, players: &[(i32, String)], client: &mut i32) {
        let current = if *client < 0 { RECORDED_PLAYER.to_owned() } else {
            players.iter().find(|(c, _)| c == client).map_or_else(|| "Player not in view here".to_owned(), |(_, n)| strip_colors(n))
        };
        egui::CollapsingHeader::new(format!("{label}: {current}")).id_salt(("studio_director_client_list", label)).show(ui, |ui| {
            ui.selectable_value(client, -1, RECORDED_PLAYER);
            for (c, name) in players {
                ui.selectable_value(client, *c, player_row_text(name));
            }
            if players.is_empty() {
                ui.label(egui::RichText::new("Move the playhead onto demo footage to list its players")
                    .size(11.5).color(theme::TEXT_FAINT));
            }
        });
    }

    /// Player picker by name. -1 is `none_label` ("Recorded player" for a
    /// subject, "Followed player" for an aim target). Client numbers are not
    /// edited by hand; a player not currently in view keeps its last pick.
    pub(super) fn studio_client_rows(ui: &mut egui::Ui, id: &str, players: &[(i32, String)], client: &mut i32,
        none_label: &str) {
        sui::prop_row(ui, "Player", |ui| {
            let selected = if *client < 0 { egui::WidgetText::from(none_label) } else {
                players.iter().find(|(c, _)| c == client)
                    .map_or_else(|| egui::WidgetText::from("Player not in view here"),
                        |(_, name)| player_row_text(name).into())
            };
            sui::combo_box((id, "players")).width(ui.available_width())
                .wrap_mode(egui::TextWrapMode::Truncate)
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    ui.selectable_value(client, -1, none_label);
                    for (c, name) in players {
                        ui.selectable_value(client, *c, player_row_text(name));
                    }
                    if players.is_empty() {
                        ui.label(egui::RichText::new("Move the playhead onto demo footage to list its players")
                            .size(11.5).color(theme::TEXT_FAINT));
                    }
                });
        });
    }

    /// Director shots as clips on the Camera track. Body drag moves a shot
    /// (or every selected shot), edge drags trim it. Registered before the
    /// camera diamonds, so keys stay clickable on top of a shot.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn egui_studio_director_spans(&mut self, ui: &mut egui::Ui, painter: &egui::Painter,
        row: egui::Rect, x0: f32, right: f32, zoom: f32, scroll: f32, locked: bool) {
        let to_x = |t: f64| x0 + (t as f32 - scroll) * zoom;
        let players = self.studio_demo_players();
        let now = self.render_studio.timeline_time();
        let active = self.render_studio.timeline.director_at(now)
            .or_else(|| self.render_studio.timeline.stage_director_at(now)).map(|s| s.id);
        // Clone only on-screen shots: the loop mutates self, and shots carry path keys.
        let shots: Vec<StudioDirectorShot> = self.render_studio.timeline.director_shots.iter()
            .filter(|s| to_x(s.start).max(x0) < to_x(s.end()).min(right)).cloned().collect();
        let multi_lane = self.render_studio.timeline.lanes > 1;
        let selection = self.studio_director_selected_ids();
        let (ctrl, shift) = ui.input(|i| (i.modifiers.ctrl || i.modifiers.command, i.modifiers.shift));
        let mut action: Option<(Vec<u64>, ShotAction)> = None;
        for shot in &shots {
            let (xa, xb) = (to_x(shot.start), to_x(shot.end()));
            let block = egui::Rect::from_min_max(egui::pos2(xa.max(x0), row.top() + 4.0),
                egui::pos2(xb.min(right), row.bottom() - 4.0));
            if block.width() <= 0.0 { continue; }
            let selected = selection.contains(&shot.id);
            let color = if shot.enabled { sui::CAMERA } else { theme::TEXT_DISABLED };
            painter.rect_filled(block, 3.0, sui::clip_body(color, selected).gamma_multiply(if shot.enabled { 1.0 } else { 0.6 }));
            painter.rect_filled(egui::Rect::from_min_max(block.left_top(), egui::pos2(block.right(), block.top() + 3.0)),
                egui::CornerRadius { nw: 3, ne: 3, sw: 0, se: 0 }, color);
            painter.rect_stroke(block, 3.0, egui::Stroke::new(if selected { 1.5 } else { 1.0 },
                if selected { egui::Color32::WHITE } else { color.gamma_multiply(0.5) }), egui::StrokeKind::Inside);
            if active == Some(shot.id) {
                painter.circle_filled(egui::pos2(block.right() - 8.0, block.center().y + 1.0), 3.0, theme::ACCENT);
            }
            if block.width() > 40.0 {
                let stage = self.render_studio.timeline.demo_at(shot.start + shot.duration * 0.5).is_none();
                let text = if let Some(label)=&shot.comparison_label {label.clone()} else if stage {
                    if multi_lane { format!("{} · character {}", shot.preset.label(), shot.actor_lane + 1) } else { shot.preset.label().to_owned() }
                } else if shot.client < 0 { shot.preset.label().to_owned() }
                    else { format!("{} · {}", shot.preset.label(), Self::studio_player_label(&players, shot.client)) };
                painter.with_clip_rect(block.shrink2(egui::vec2(4.0, 1.0))).text(
                    egui::pos2(block.left() + 7.0, block.center().y + 1.5), egui::Align2::LEFT_CENTER, text,
                    egui::FontId::proportional(11.0), if shot.enabled { theme::TEXT } else { theme::TEXT_FAINT });
            }
            if locked { continue; }
            let base = ui.make_persistent_id(("studio_director_span", shot.id));
            if shot.comparison_label.is_some() {
                let response=ui.interact(block,base,egui::Sense::click());
                if response.clicked() {self.studio_click_director(shot.id,ctrl,shift);}
                response.on_hover_text("Select to edit this phase's camera framing and lens in the Inspector.");
                continue;
            }
            let body = ui.interact(block, base, egui::Sense::click_and_drag());
            let mut grips = Vec::new();
            if block.width() > 18.0 {
                for (edge, x) in [(1u8, xa), (2u8, xb)] {
                    if x < x0 - 1.0 || x > right + 1.0 { continue; }
                    let grip = egui::Rect::from_center_size(egui::pos2(x, block.center().y), egui::vec2(10.0, block.height()));
                    grips.push((edge, ui.interact(grip, base.with(edge), egui::Sense::click_and_drag())
                        .on_hover_cursor(egui::CursorIcon::ResizeHorizontal)));
                }
            }
            if body.secondary_clicked() && !selected { self.studio_select_director(shot.id); }
            for (edge, response) in std::iter::once((0u8, body.clone())).chain(grips) {
                if response.clicked() { self.studio_click_director(shot.id, ctrl, shift); }
                if response.drag_started() {
                    // Dragging a shot of a group moves the group; trims stay single.
                    let group = edge == 0 && selected && selection.len() > 1;
                    if !group { self.studio_select_director(shot.id); }
                    if let Some(p) = response.interact_pointer_pos() {
                        let ids = if group { selection.clone() } else { vec![shot.id] };
                        let starts: Vec<(u64, f64, f64)> = self.render_studio.timeline.director_shots.iter().filter(|s| ids.contains(&s.id))
                            .map(|s| (s.id, s.start, s.duration)).collect();
                        ui.data_mut(|d| d.insert_temp(base, (p.x, starts)));
                    }
                }
                if response.dragged() {
                    if let (Some((px, starts)), Some(p)) =
                        (ui.data(|d| d.get_temp::<(f32, Vec<(u64, f64, f64)>)>(base)), response.interact_pointer_pos()) {
                        let delta = f64::from((p.x - px) / zoom.max(1e-4));
                        for (id, start, duration) in starts {
                            let Some(s) = self.render_studio.timeline.director_shots.iter_mut().find(|s| s.id == id) else { continue };
                            match edge {
                                0 => s.start = (start + delta).clamp(0.0, 36000.0 - duration),
                                1 => {
                                    let end = start + duration;
                                    s.start = (start + delta).clamp(0.0, end - MIN_SHOT);
                                    s.duration = end - s.start;
                                }
                                _ => s.duration = (duration + delta).clamp(MIN_SHOT, 36000.0 - start),
                            }
                        }
                        self.profile_preview_key = None;
                    }
                }
                if edge != 0 {
                    let time = if edge == 1 { shot.start } else { shot.end() };
                    response.on_hover_ui(|ui| { ui.label(format!("{} {:.3}s · drag to trim the shot", if edge == 1 { "Start" } else { "End" }, time)); });
                }
            }
            let _ = body.context_menu(|ui| { sui::editor_style(ui);
                let targets = if selected { selection.clone() } else { vec![shot.id] };
                let plural = targets.len() > 1;
                let count = if plural { format!(" ({} shots)", targets.len()) } else { String::new() };
                if ui.button(format!("Regenerate{count}  (R)")).on_hover_text("Pick a different shot for this moment").clicked() {
                    action = Some((targets.clone(), ShotAction::Regenerate)); ui.close();
                }
                ui.menu_button(format!("Change type{count}"), |ui| {
                    sui::editor_style(ui);
                    for (index, preset) in StudioDirectorPreset::FOLLOW.iter().chain(StudioDirectorPreset::WORLD.iter()).enumerate() {
                        if index == StudioDirectorPreset::FOLLOW.len() { ui.separator(); }
                        if ui.selectable_label(!plural && shot.preset == *preset, preset.label())
                            .on_hover_text(preset.describe()).clicked() {
                            action = Some((targets.clone(), ShotAction::Retype(*preset))); ui.close();
                        }
                    }
                });
                if ui.button(if shot.enabled { "Disable" } else { "Enable" }).clicked() { action = Some((targets.clone(), ShotAction::Toggle)); ui.close(); }
                if ui.button("Bring to front").clicked() { action = Some((targets.clone(), ShotAction::Front)); ui.close(); }
                if !plural && ui.button("Duplicate after").clicked() { action = Some((targets.clone(), ShotAction::Duplicate)); ui.close(); }
                if ui.button(format!("Delete{count}  (Del)")).clicked() { action = Some((targets.clone(), ShotAction::Delete)); ui.close(); }
            });
            body.on_hover_cursor(egui::CursorIcon::Grab).on_hover_ui(|ui| { ui.label(format!(
                "{} on {} · {:.2}s – {:.2}s\nDrag to move, drag edges to trim · Ctrl/Shift+click to multi-select · right-click for options",
                shot.preset.label(), Self::studio_player_label(&players, shot.client), shot.start, shot.end())); });
        }
        if let Some((ids, what)) = action { self.studio_director_action(ids, what); }
    }

    fn studio_director_action(&mut self, ids: Vec<u64>, what: ShotAction) {
        match what {
            ShotAction::Regenerate => self.studio_start_regenerate(ids, None),
            ShotAction::Retype(preset) => {
                let fov = self.studio_default_fov();
                let mut place = Vec::new();
                for shot in self.render_studio.timeline.director_shots.iter_mut().filter(|s| ids.contains(&s.id)) {
                    preset.seed_shot(shot, fov);
                    if needs_placement(preset) && shot.client < 0 { place.push(shot.id); }
                }
                if !place.is_empty() { self.studio_start_regenerate(place, Some(preset)); }
            }
            ShotAction::Toggle => {
                let shots = &mut self.render_studio.timeline.director_shots;
                let enable = !shots.iter().filter(|s| ids.contains(&s.id)).any(|s| s.enabled);
                for shot in shots.iter_mut().filter(|s| ids.contains(&s.id)) { shot.enabled = enable; }
            }
            ShotAction::Front => {
                let shots = &mut self.render_studio.timeline.director_shots;
                let (front, rest): (Vec<_>, Vec<_>) = std::mem::take(shots).into_iter().partition(|s| ids.contains(&s.id));
                *shots = rest;
                shots.extend(front);
            }
            ShotAction::Duplicate => {
                let shots = &mut self.render_studio.timeline.director_shots;
                let Some(source) = shots.iter().find(|s| Some(&s.id) == ids.first()) else { return };
                let mut shot = source.clone();
                shot.id = shots.iter().map(|s| s.id).max().unwrap_or(0) + 1;
                shot.start = shot.end().min(36000.0 - shot.duration);
                let id = shot.id;
                shots.push(shot);
                self.studio_select_director(id);
            }
            ShotAction::Delete => self.studio_delete_directors(&ids),
        }
        self.profile_preview_key = None;
    }

    /// R in the timeline: regenerate the selected shots.
    pub(super) fn studio_regenerate_selected(&mut self) -> bool {
        let ids = self.studio_director_selected_ids();
        if ids.is_empty() { return false; }
        self.studio_start_regenerate(ids, None);
        true
    }

    /// Inspector for the selected Director shot.
    pub(super) fn egui_studio_director_inspector(&mut self, ui: &mut egui::Ui, index: usize) {
        let fps = self.render_studio.fps;
        // Every player of the recording, not only those in view right now.
        let players: Vec<(i32, String)> = self.studio_demo_all_players().into_iter().map(|(c, n, _)| (c, n)).collect();
        let now = self.render_studio.timeline_time();
        let live = self.render_studio.timeline.director_at(now).map(|s| s.id);
        let Some(shot) = self.render_studio.timeline.director_shots.get(index).cloned() else { return };
        if let Some(label)=&shot.comparison_label {
            sui::inspector_title(ui, sui::CAMERA, "Race camera shot", label,
                &format!("{} – {}{}", sui::timecode(shot.start, fps), sui::timecode(shot.end(), fps),
                    if live == Some(shot.id) { " · live at playhead" } else { "" }));
            sui::hint(ui,"Edit this phase's camera below. It keeps following the indexed runner through pauses and rewinds. Director controls the race timing; regenerating the comparison replaces these edits.");
            let before = shot.comparison_camera.unwrap_or_default();
            let mut edited = before;
            race_camera_editor(ui, shot.id, &mut edited, self.render_studio.demo_viewport
                .map_or(16.0/9.0, |v| v[2] as f32/v[3].max(1) as f32));
            if edited != before && edited.valid() {
                self.render_studio.timeline.director_shots[index].comparison_camera = Some(edited);
                self.profile_preview_key = None;
            }
            match sui::button_row(ui, &[(None,"Reset camera",ButtonKind::Normal,shot.comparison_camera.is_some()),
                (None,"Jump to start",ButtonKind::Normal,true)]) {
                Some(0) => {self.render_studio.timeline.director_shots[index].comparison_camera=None;self.profile_preview_key=None;}
                Some(1) => {self.render_studio.timeline.seek(shot.start);self.profile_preview_key=None;}
                _ => {}
            }
            return;
        }
        let selection = self.studio_director_selected_ids();
        let busy = self.render_studio.director_job.is_some();
        sui::inspector_title(ui, sui::CAMERA, "Director shot",
            &format!("{} · {}", shot.preset.label(), Self::studio_player_label(&players, shot.client)),
            &format!("{} – {}{}", sui::timecode(shot.start, fps), sui::timecode(shot.end(), fps),
                if live == Some(shot.id) { " · live at playhead" } else { "" }));
        if selection.len() > 1 {
            sui::callout(ui, &format!("{} shots selected. Edits below apply to this one; these apply to all:", selection.len()),
                theme::ACCENT);
            match sui::button_row(ui, &[(None, "Regenerate all", ButtonKind::Primary, !busy),
                (None, "Delete all", ButtonKind::Danger, true)]) {
                Some(0) => self.studio_director_action(selection.clone(), ShotAction::Regenerate),
                Some(1) => { self.studio_director_action(selection.clone(), ShotAction::Delete); return; }
                _ => {}
            }
        }
        let mut edited = shot.clone();
        let mut retype = None;
        sui::section(ui, ("studio_director_shot", shot.id), "Shot",
            Some("A live shot is evaluated from output time only, so scrubbing and export always agree. It applies only over demo footage; outside it, the keyed camera returns. Later shots win overlaps (Bring to front)."),
            true, |ui| {
            sui::prop_toggle(ui, "Enabled", &mut edited.enabled);
            let mut preset = edited.preset;
            if Self::studio_preset_picker(ui, &mut preset) { retype = Some(preset); }
            sui::hint(ui, preset.describe());
            sui::prop_drag(ui, "Start", egui::DragValue::new(&mut edited.start).speed(0.02).range(0.0..=36000.0).suffix(" s"));
            sui::prop_drag(ui, "Duration", egui::DragValue::new(&mut edited.duration).speed(0.02).range(MIN_SHOT..=3600.0).suffix(" s"));
            if edited.auto { sui::hint(ui, "Chosen by the Auto Director from the player's recorded path."); }
        });
        if let Some(preset) = retype {
            self.studio_director_action(vec![shot.id], ShotAction::Retype(preset));
            return;
        }
        let stage = self.render_studio.timeline.demo_at(shot.start + shot.duration * 0.5).is_none();
        if stage {
            let lanes = self.render_studio.timeline.lanes;
            sui::section(ui, ("studio_director_subject", shot.id), "Subject",
                Some("Over animation footage the shot frames a character track. Body points use JKA proportions (no bone sampling on the stage)."),
                true, |ui| {
                let options: Vec<(usize, String)> = (0..lanes).map(|lane| (lane, format!("Character {}", lane + 1))).collect();
                let options: Vec<(usize, &str)> = options.iter().map(|(lane, label)| (*lane, label.as_str())).collect();
                sui::prop_combo(ui, "Character", ("studio_director_lane", shot.id), &mut edited.actor_lane, &options);
            });
        } else if edited.preset != StudioDirectorPreset::RecordedPov {
        sui::section(ui, ("studio_director_subject", shot.id), "Subject",
            Some("The demo client the camera follows. Players outside the recorded PVS fall back to the recorded POV; nothing is fabricated."),
            true, |ui| {
            Self::studio_client_rows(ui, "studio_director_client", &players, &mut edited.client, RECORDED_PLAYER);
        });
        }
        let world = edited.preset.world_anchored();
        let mut place = false;
        if world {
            sui::section(ui, ("studio_director_world", shot.id), "Placement",
                Some("World shots stand in the map. Positions are placed from the recorded player's path with a clear line of sight; Re-place picks another spot."),
                true, |ui| {
                if let Some(position) = &mut edited.position_override {
                    sui::prop_vec3(ui, "Position", position, 1.0, ["X", "Y", "Z"], true);
                    let mut moving = edited.position_end.is_some();
                    if sui::prop_toggle(ui, "Move during shot", &mut moving).changed() {
                        edited.position_end = moving.then_some(*position);
                    }
                    if let Some(end) = &mut edited.position_end {
                        sui::prop_vec3(ui, "End position", end, 1.0, ["X", "Y", "Z"], true);
                        sui::prop_toggle(ui, "Ease in/out", &mut edited.ease);
                    }
                    let rig = &mut edited.rig;
                    if edited.focus_target.is_none() {
                        sui::prop_toggle(ui, "Follow subject", &mut rig.aim_enabled)
                            .on_hover_text("Pan to keep the subject in frame; off = locked-off camera");
                        if rig.aim_enabled {
                            let anchors: Vec<(StudioDemoAnchor, &str)> = StudioDemoAnchor::ALL.iter().map(|a| (*a, a.label())).collect();
                            sui::prop_combo(ui, "Look at", ("studio_director_world_aim", shot.id), &mut rig.aim_anchor, &anchors);
                        }
                    }
                } else {
                    sui::callout(ui, "Not placed yet: it orbits the subject until placed in the map.", theme::WARNING);
                }
                place = sui::button_row(ui, &[(Some(Icon::Camera), if edited.position_override.is_some() { "Re-place" } else { "Place in map" },
                    ButtonKind::Normal, !busy && edited.client < 0)]).is_some();
            });
        }
        let perspective = edited.preset.perspective();
        if edited.preset != StudioDirectorPreset::RecordedPov && (!world || edited.position_override.is_none()) {
            let rig = &mut edited.rig;
            sui::section(ui, ("studio_director_camera", shot.id), "Camera", None, true, |ui| {
                let anchors: Vec<(StudioDemoAnchor, &str)> = StudioDemoAnchor::ALL.iter().map(|a| (*a, a.label())).collect();
                sui::prop_combo(ui, "Anchor", ("studio_director_anchor", shot.id), &mut rig.position_anchor, &anchors);
                if perspective != StudioFollowPerspective::FirstPerson {
                    sui::prop_slider(ui, "Distance", egui::Slider::new(&mut rig.range, 0.0..=2048.0));
                }
                match perspective {
                    StudioFollowPerspective::Orbit => {
                        sui::prop_slider(ui, "Yaw", egui::Slider::new(&mut rig.yaw_offset, -180.0..=180.0).suffix("°"));
                        sui::prop_slider(ui, "Pitch", egui::Slider::new(&mut rig.pitch_offset, -89.0..=89.0).suffix("°"));
                        sui::prop_slider(ui, "Height", egui::Slider::new(&mut rig.vertical_offset, -512.0..=512.0));
                        sui::prop_toggle(ui, "World angles", &mut rig.orbit_world_angles)
                            .on_hover_text("Angles in world space instead of relative to the player's facing");
                        sui::prop_toggle(ui, "Chase from target", &mut rig.chase_aim)
                            .on_hover_text("Camera behind the anchor on the line from the Look-at player (raised by Height), keeping the anchor (e.g. saber) in the foreground");
                        sui::prop_toggle(ui, "Avoid walls", &mut rig.orbit_collision);
                    }
                    StudioFollowPerspective::ThirdPerson => {
                        sui::prop_slider(ui, "Height", egui::Slider::new(&mut rig.vertical_offset, -512.0..=512.0));
                        sui::prop_slider(ui, "Side", egui::Slider::new(&mut rig.horizontal_offset, -512.0..=512.0));
                        sui::prop_toggle(ui, "Chase motion", &mut rig.motion_direction)
                            .on_hover_text("Orient the chase camera from movement direction (TaystJK angle -1)");
                        ui.add_enabled_ui(!rig.motion_direction, |ui| {
                            sui::prop_slider(ui, "Yaw", egui::Slider::new(&mut rig.yaw_offset, -180.0..=180.0).suffix("°"));
                        });
                        sui::prop_slider(ui, "Pitch", egui::Slider::new(&mut rig.pitch_offset, -89.0..=89.0).suffix("°"));
                    }
                    StudioFollowPerspective::FirstPerson => {
                        sui::prop_slider(ui, "Yaw", egui::Slider::new(&mut rig.yaw_offset, -180.0..=180.0).suffix("°"));
                        sui::prop_slider(ui, "Pitch", egui::Slider::new(&mut rig.pitch_offset, -89.0..=89.0).suffix("°"));
                    }
                }
                sui::prop_vec3(ui, "Offset", &mut rig.position_offset, 0.5, ["X", "Y", "Z"], true);
                sui::prop_toggle(ui, "Facing-relative", &mut rig.position_local);
            });
            if perspective != StudioFollowPerspective::FirstPerson {
                sui::section(ui, ("studio_director_motion", shot.id), "Motion",
                    Some("Moves across the shot, eased by Ease in/out: sweep turns the camera around the subject, End distance dollies in or out, End FOV zooms. Keep subject size makes a dolly zoom."),
                    edited.sweep != 0.0 || edited.range_end.is_some() || edited.fov_end.is_some(), |ui| {
                    sui::prop_drag(ui, "Sweep", egui::DragValue::new(&mut edited.sweep).range(-3600.0..=3600.0).speed(1.0).suffix("°"))
                        .on_hover_text("Degrees the camera turns around the subject across the shot (360 = one full lap)");
                    let mut dolly = edited.range_end.is_some();
                    if sui::prop_toggle(ui, "Dolly", &mut dolly).changed() {
                        edited.range_end = dolly.then_some((edited.rig.range * 0.5).max(32.0));
                        if !dolly { edited.zoom_lock = false; }
                    }
                    if let Some(end) = &mut edited.range_end {
                        sui::prop_slider(ui, "End distance", egui::Slider::new(end, 0.0..=2048.0));
                        sui::prop_toggle(ui, "Keep subject size", &mut edited.zoom_lock)
                            .on_hover_text("Dolly zoom: the field of view follows the distance (vertigo effect)");
                    }
                    if !edited.zoom_lock {
                        let mut zoom = edited.fov_end.is_some();
                        if sui::prop_toggle(ui, "Zoom", &mut zoom).changed() { edited.fov_end = zoom.then_some(edited.fov); }
                        if let Some(end) = &mut edited.fov_end {
                            sui::prop_slider(ui, "End FOV", egui::Slider::new(end, 10.0..=140.0).suffix("°"));
                        }
                    }
                    sui::prop_toggle(ui, "Ease in/out", &mut edited.ease);
                });
            }
            let rig = &mut edited.rig;
            sui::section(ui, ("studio_director_aim", shot.id), "Look-at",
                Some("Aim at a body point on the followed player or another client. Bone points use the presented Ghoul2 pose; a missing bone falls back to the eyes."),
                rig.aim_enabled, |ui| {
                sui::prop_toggle(ui, "Aim at target", &mut rig.aim_enabled);
                if rig.aim_enabled {
                    Self::studio_client_rows(ui, "studio_director_aim_client", &players, &mut rig.aim_client, FOLLOWED_PLAYER);
                    let anchors: Vec<(StudioDemoAnchor, &str)> = StudioDemoAnchor::ALL.iter().map(|a| (*a, a.label())).collect();
                    sui::prop_combo(ui, "Look at", ("studio_director_aim_anchor", shot.id), &mut rig.aim_anchor, &anchors);
                    sui::prop_vec3(ui, "Aim offset", &mut rig.aim_offset, 0.5, ["X", "Y", "Z"], true);
                }
            });
        }
        sui::section(ui, ("studio_director_lens", shot.id), "Lens", None, true, |ui| {
            super::render_studio_camera::studio_lens_editor(ui,("director_lens",shot.id),&mut edited.fov,&mut edited.lens, self.render_studio.demo_viewport.map_or(16.0/9.0,|v|v[2] as f32/v[3].max(1) as f32),edited.focus_target.is_some());
            sui::prop_toggle(ui, "Autofocus", &mut edited.rig.autofocus)
                .on_hover_text("Pull focus to the subject every frame (with an aperture set), also when a look-at point sets the framing");
        });
        if super::render_studio_camera::studio_target_editor(ui,&mut edited.focus_target) && edited.focus_target.is_none() && !world {
            edited.position_override=None;
        }
        if !world {
            if let Some(position)=&mut edited.position_override {
                sui::prop_vec3(ui,"Position (world)",position,1.0,["X","Y","Z"],true);
                if ui.button("Return to live camera position").clicked(){edited.position_override=None;edited.position_end=None;}
            }
        }
        // Commit only valid edits; the snapshot history records the change.
        if edited != shot && edited.valid() {
            self.render_studio.timeline.director_shots[index] = edited;
            self.profile_preview_key = None;
        }
        if place { self.studio_start_regenerate(vec![shot.id], Some(shot.preset)); }
        match sui::button_row(ui, &[(Some(Icon::Camera), "Regenerate", ButtonKind::Primary, !busy),
            (None, "Jump to start", ButtonKind::Normal, true)]) {
            Some(0) => self.studio_director_action(vec![shot.id], ShotAction::Regenerate),
            Some(1) => { self.render_studio.timeline.seek(shot.start); self.profile_preview_key = None; }
            _ => {}
        }
        match sui::button_row(ui, &[(None, "Bring to front", ButtonKind::Normal, index + 1 < self.render_studio.timeline.director_shots.len()),
            (None, "Delete shot  (Del)", ButtonKind::Danger, true)]) {
            Some(0) => self.studio_director_action(vec![shot.id], ShotAction::Front),
            Some(1) => self.studio_delete_director(index),
            _ => {}
        }
    }
}

fn race_camera_editor(ui: &mut egui::Ui, id: u64,
    edit: &mut crate::app::render_studio_race::RaceCameraEdit, aspect: f32) {
    sui::section(ui,("race_camera_framing",id),"Camera",None,true,|ui| {
        sui::prop_slider(ui,"Distance",egui::Slider::new(&mut edit.distance,0.1..=4.0).suffix("×"));
        sui::prop_slider(ui,"Yaw",egui::Slider::new(&mut edit.yaw,-180.0..=180.0).suffix("°"));
        sui::prop_slider(ui,"Pitch",egui::Slider::new(&mut edit.pitch,-80.0..=80.0).suffix("°"));
        sui::prop_slider(ui,"Height",egui::Slider::new(&mut edit.height,-512.0..=512.0));
        sui::prop_slider(ui,"Side",egui::Slider::new(&mut edit.side,-512.0..=512.0));
        sui::hint(ui,"Offsets adjust the planned camera and keep it aimed at the runner. Walls limit the available distance.");
    });
    sui::section(ui,("race_camera_lens",id),"Lens",None,true,|ui| {
        let mut manual_fov=edit.fov.is_some();
        if sui::prop_toggle(ui,"Override FOV",&mut manual_fov).changed() {
            edit.fov=manual_fov.then_some(90.0);
        }
        if let Some(fov)=edit.fov.as_mut() {
            sui::prop_slider(ui,"FOV",egui::Slider::new(fov,10.0..=140.0).suffix("°"));
            sui::prop_value(ui,"35 mm equivalent",&format!("{:.1} mm",crate::renderer::StudioLens::focal_length_35mm(*fov,aspect)));
        }
        let mut manual_lens=edit.lens.is_some();
        if sui::prop_toggle(ui,"Override depth of field",&mut manual_lens).changed() {
            edit.lens=manual_lens.then(crate::renderer::StudioLens::default);
        }
        if let Some(lens)=edit.lens.as_mut() {
            super::render_studio_camera::studio_aperture_editor(ui,lens,true);
        }
    });
}

/// A player name for picker rows: plain, in the readable text colour (game
/// colour codes such as ^0 black vanish on the dark menu).
fn player_row_text(name: &str) -> egui::RichText {
    egui::RichText::new(strip_colors(name)).size(12.0).color(theme::TEXT)
}

/// Plain player name for compact labels (drops ^N color codes).
fn strip_colors(name: &str) -> String {
    let mut out = String::new();
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '^' && chars.peek().is_some_and(|n| n.is_ascii_alphanumeric()) { chars.next(); continue; }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod director_popup_tests {
    use super::*;
    use egui::{Context, Event, Modifiers, PointerButton, RawInput, Rect, pos2, vec2};

    struct State {
        open: bool, main: Option<(String, String)>, ghosts: bool, generated: bool,
        button: Rect, combo: Rect, items: [Rect; 3], toggle: Rect, generate: Rect,
        frame: u64, jitter: bool,
    }

    fn frame(ctx: &Context, state: &mut State, events: Vec<Event>) {
        state.frame += 1;
        let _ = ctx.run_ui(RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
            events, ..Default::default()
        }, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                // The real timeline toolbar adds/removes controls as selections change.
                if state.jitter { for _ in 0..state.frame % 2 { ui.allocate_space(vec2(1.0, 1.0)); } }
                sui::region(ui, Rect::from_min_max(pos2(350.0, 30.0), pos2(790.0, 60.0)),
                    "studio_director_toolbar", egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let button = director_button(ui, false);
                    state.button = button.rect;
                    let mut open = state.open;
                    director_popup(&button, &mut open, |ui| {
                        sui::group_label(ui, "Route comparison");
                        sui::prop_row(ui, "Main POV", |ui| {
                            let combo = sui::combo_box("studio_route_main")
                                .width(ui.available_width())
                                .selected_text(state.main.as_ref().map_or("Fastest run (automatic)", |(n, _)| n.as_str()))
                                .show_ui(ui, |ui| {
                                    for (i, (value, label)) in [(None, "Fastest run (automatic)"),
                                        (Some(("my recording".into(), String::new())), "my recording"),
                                        (Some(("other recording".into(), String::new())), "other recording")]
                                        .into_iter().enumerate() {
                                        state.items[i] = ui.selectable_value(&mut state.main, value, label).rect;
                                    }
                                });
                            state.combo = combo.response.rect;
                        });
                        state.toggle = sui::prop_toggle(ui, "Add POV companion ghosts", &mut state.ghosts).rect;
                        let generate = sui::button(ui, Some(Icon::Camera), "Generate route comparison", ButtonKind::Primary, true);
                        state.generate = generate.rect;
                        if generate.clicked() { state.generated = true; ui.close(); }
                    });
                    state.open = open;
                });
            });
        });
    }

    fn click(ctx: &Context, state: &mut State, pos: egui::Pos2) {
        frame(ctx, state, vec![Event::PointerMoved(pos)]);
        for pressed in [true, false] {
            frame(ctx, state, vec![Event::PointerButton {
                pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE,
            }]);
        }
        frame(ctx, state, vec![]);
    }

    #[test]
    fn director_main_pov_dropdown_keeps_panel_open_until_dismissed_or_generated() {
        for jitter in [false, true] {
            let ctx = Context::default();
            let mut state = State {
                open: false, main: None, ghosts: true, generated: false, button: Rect::NOTHING,
                combo: Rect::NOTHING, items: [Rect::NOTHING; 3], toggle: Rect::NOTHING,
                generate: Rect::NOTHING, frame: 0, jitter,
            };
            frame(&ctx, &mut state, vec![]);
            let pos = state.button.center(); click(&ctx, &mut state, pos);
            assert!(state.open, "jitter={jitter}: opening Director");
            let pos = state.combo.center(); click(&ctx, &mut state, pos);
            assert!(state.open && egui::Popup::is_any_open(&ctx), "jitter={jitter}: opening Main POV must retain Director");
            let pos = state.items[1].center(); click(&ctx, &mut state, pos);
            assert_eq!(state.main.as_ref().unwrap().0, "my recording");
            assert!(state.open && !state.generated, "choosing Main POV must keep settings open without generating");
            let pos = state.combo.center(); click(&ctx, &mut state, pos);
            let pos = state.items[2].center(); click(&ctx, &mut state, pos);
            assert_eq!(state.main.as_ref().unwrap().0, "other recording");
            assert!(state.open, "changing Main POV a second time must retain Director");
            let pos = state.toggle.center(); click(&ctx, &mut state, pos);
            assert!(state.open && !state.ghosts, "settings toggles must retain Director");
            click(&ctx, &mut state, pos2(100.0, 540.0));
            assert!(!state.open, "outside click dismisses Director");
            let pos = state.button.center(); click(&ctx, &mut state, pos);
            let pos = state.generate.center(); click(&ctx, &mut state, pos);
            assert!(state.generated && !state.open, "Generate intentionally closes Director");
            let pos = state.button.center(); click(&ctx, &mut state, pos);
            frame(&ctx, &mut state, vec![Event::Key {
                key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE,
            }]);
            assert!(!state.open, "Escape dismisses Director");
        }
    }
}
