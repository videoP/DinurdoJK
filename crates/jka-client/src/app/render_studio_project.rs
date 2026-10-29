//! A new document clears project content while retaining editor preferences
//! and the gameplay settings snapshot owned by the open Studio workspace.
use super::{
    render_studio::{RenderStudio, StudioBackground},
    App,
};

impl RenderStudio {
    fn reset_project(&mut self, name: String, model: String, fov: f32) {
        self.timeline = Default::default();
        self.director_paths=Default::default();
        self.timeline.camera[0].fov = fov;
        self.model = model;
        self.with_sabers = false;
        self.background = StudioBackground::Black;
        self.background_map.clear();
        self.audio = Default::default();
        self.race_runtime.clear();
        self.route_job = None;
        self.director_job = None;
        self.route_options = Default::default();
        self.director_focus = Default::default();
        self.director_seed = 1;
        self.director_selected = None;
        self.director_multi.clear();
        self.director_anchor = None;
        self.director_popup_open = false;
        self.pending_demo_drop = None;
        self.pending_demo_queue.clear();
        self.cinematic_preview = None;
        self.emote = None;
        self.stock_animation = "BOTH_STAND1".into();
        self.clip_meta = None;
        self.trim_start = 0;
        self.trim_stop = 0;
        self.playing = false;
        self.playhead = 0.0;
        self.preview_sample_time = None;
        self.source_wait = None;
        self.demo_preview = None;
        self.demo_drag = None;
        self.selected_background = false;
        self.background_popup_open = false;
        self.light_ui = Default::default();
        self.scene_lighting_selected_key = 0;
        self.scene_lighting_popup_open = false;
        self.spatial = Default::default();
        self.navigation = Default::default();
        self.camera_rig_runtime = Default::default();
        self.camera_orbit_edit_runtime = Default::default();
        self.camera_pending_match = None;
        self.hud_editing = false;
        self.hud_selected = None;
        self.hud_drag_origin = None;
        self.hud_scale_origin = None;
        self.media_drag = None;
        self.media_drag_group.clear();
        self.media_drag_duration = None;
        self.library_selection.clear();
        self.library_anchor = None;
        self.context_time = None;
        self.context_lane = None;
        self.context_row = None;
        self.model_picker_lane = None;
        self.show_new_project = false;
        self.show_open_project = false;
        self.show_save_as = false;
        self.show_render = false;
        self.confirm_leave = false;
        self.graphics_settings_open = false;
        self.history = Default::default();
        self.clipboard = None;
        // Metadata arriving for a library preview must not seed a clip into
        // the explicitly empty document on the next frame.
        self.initial_clip_seeded = true;
        self.project_name = name.clone();
        self.saved_project_name = name;
        self.save_as_name.clear();
        self.status = "New project: add demos, animations or audio to the timeline".into();
        self.init_history();
    }
}

impl App {
    pub(in crate::app) fn studio_new_project_name_available(&self, name: &str) -> bool {
        name.chars()
            .take(80)
            .any(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            && !RenderStudio::project_file_name_for(name).eq_ignore_ascii_case(&self.render_studio.project_file_name())
            && !self.studio_project_directory_for(name).exists()
    }

    pub(in crate::app) fn request_new_render_studio_project(&mut self) {
        if self.render_studio.export.is_some() || self.render_studio.still.is_some() {
            return;
        }
        let mut number = 1;
        loop {
            let name = if number == 1 {
                "Untitled".into()
            } else {
                format!("Untitled_{number}")
            };
            if self.studio_new_project_name_available(&name) {
                self.render_studio.new_project_name = name;
                break;
            }
            number += 1;
        }
        self.render_studio.show_open_project = false;
        self.render_studio.show_save_as = false;
        self.render_studio.confirm_leave = false;
        self.render_studio.show_new_project = true;
    }

    pub(in crate::app) fn new_render_studio_project(
        &mut self,
        name: String,
        save_current: bool,
    ) -> bool {
        if self.render_studio.export.is_some() || self.render_studio.still.is_some() {
            return false;
        }
        if !self.studio_new_project_name_available(&name) {
            self.render_studio.status = "Choose an unused name for the new project".into();
            return false;
        }
        if save_current && !self.save_render_studio_project() {
            return false;
        }
        self.set_studio_mouse_look(false);
        self.release_studio_sources();
        crate::emotes::stop();
        let model = self.solo_client_info.model_cvar();
        let fov = self.studio_default_fov();
        self.render_studio
            .reset_project(name.trim().to_owned(), model, fov);
        self.render_studio_update_backdrop();
        self.profile_preview_key = None;
        self.asset_preview_viewport_key = None;
        self.egui_repaint_requested = true;
        true
    }
}

#[cfg(test)]
mod new_project_tests {
    use super::*;
    use crate::app::{
        render_studio_director::{StudioDirectorPreset, StudioDirectorShot, StudioRouteJob},
        render_studio_timeline::StudioDemoClip,
    };

    #[test]
    fn new_project_clears_old_content_jobs_and_undo_without_reusing_source_ids() {
        let mut studio = RenderStudio::default();
        studio.init_history();
        studio.timeline.demos.push(StudioDemoClip { source_path: None,
            comparison: None,
            race_overlays: vec![],
            pov_trail: Default::default(),
            hud_overrides: Default::default(),
            hide_other_players: false,
            layer: 0,
            name: "old".into(),
            game: String::new(),
            start: 0.0,
            duration_ms: 10000,
            in_ms: 0,
            out_ms: 10000,
            speed_keys: vec![],
            transition_in: Default::default(),
            audio: Default::default(),
        });
        studio.timeline.director_shots.push(StudioDirectorShot::new(
            1,
            StudioDirectorPreset::Chase,
            -1,
            0.0,
            4.0,
        ));
        studio
            .pending_demo_queue
            .push_back(("old".into(), String::new(), 0));
        studio.pending_demo_drop = Some(("old".into(), String::new(), 0.0, 0));
        studio.route_job = Some(StudioRouteJob {
            worker: std::thread::spawn(|| Ok(Default::default())),
            signature: "old".into(),
        });
        studio.next_source_id += 50;
        let next_id = studio.next_source_id;
        studio.timeline.seek(7.0);
        studio.track_edit(false);
        studio.reset_project("Fresh".into(), "kyle/default".into(), 90.0);
        assert!(studio.timeline.demos.is_empty() && studio.timeline.clips.is_empty());
        assert!(
            studio.timeline.director_shots.is_empty() && studio.timeline.audio_tracks.is_empty()
        );
        assert!(studio.timeline.light_layers.is_empty());
        assert!(studio.pending_demo_queue.is_empty() && studio.pending_demo_drop.is_none());
        assert!(studio.route_job.is_none() && studio.director_job.is_none());
        assert_eq!(studio.timeline.playhead, 0.0);
        assert_eq!(studio.timeline.camera.len(), 1);
        assert_eq!(studio.timeline.camera[0].fov, 90.0);
        assert_eq!(
            studio.next_source_id, next_id,
            "late source completions must not collide with a new source"
        );
        assert!(
            !studio.undo_edit(),
            "undo must not restore the old project into the new one"
        );
        assert!(studio.initial_clip_seeded);
        studio.set_clip_meta(Some((0, 30, 30.0)));
        assert!(
            studio.timeline.clips.is_empty(),
            "library metadata must not add an unsolicited clip"
        );
    }

    #[test]
    fn new_project_preserves_workspace_profile_and_gameplay_restore_snapshot() {
        let mut studio = RenderStudio::default();
        studio.graphics_profile = "my_quality.cfg".into();
        studio.hud_settings.set("strafe_helper", true);
        studio.graphics_gameplay = Some(crate::app::render_studio_cfg::StudioGraphicsSnapshot {
            values: [("r_hdr".into(), "0".into())].into_iter().collect(),
            was_dirty: true,
        });
        studio.panels.library_width = Some(300.0);
        studio.reset_project("Fresh".into(), "kyle/default".into(), 90.0);
        assert_eq!(studio.graphics_profile, "my_quality.cfg");
        assert!(studio.hud_settings.enabled("strafe_helper"));
        assert_eq!(
            studio.graphics_gameplay.as_ref().unwrap().values["r_hdr"],
            "0"
        );
        assert!(studio.graphics_gameplay.as_ref().unwrap().was_dirty);
        assert_eq!(studio.panels.library_width, Some(300.0));
        assert_eq!(studio.project_file_name(), "Fresh.json");
    }
}
