//! Render Studio is an editor for the *existing* isolated Ghoul2 stage, not
//! a second preview renderer. Every model and clip reaches PlayerPresenter.
use super::{App, theme};
use crate::app::render_studio::{StudioBackground, StudioVideoFormat, StudioMediaTab, StudioMediaDrag};
use super::render_studio_layout;
use super::render_studio_spatial::studio_layer_color;
use crate::app::render_studio_timeline::StudioItem;

impl App {
    pub(in crate::app::egui_menu) fn egui_render_studio_page(&mut self, ui: &mut egui::Ui) {
        self.ensure_profile_catalog();
        if self.profile_background_maps.is_none() {
            self.profile_background_maps = Some(Vec::new());
            self.ui_catalog.request(&self.base, self.game.as_deref(),
                crate::app::ui_catalog::CatalogRequest::ProfileBackgroundMaps);
        }
        let width = ui.available_width();
        let height = ui.available_height().max(370.0);
        let (root, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        let Some(panels) = render_studio_layout::compute(width, height) else { return };
        let panel_rect = |p: [f32;4]| egui::Rect::from_min_max(
            root.min + egui::vec2(p[0],p[1]), root.min + egui::vec2(p[2],p[3]));
        let locked = self.render_studio.export.is_some() || self.render_studio.still.is_some();
        // Clicking anywhere on the timeline returns keyboard ownership to
        // the NLE, including after searching the library or naming a project.
        // This is more precise than the whole-context keyboard-ownership flag.
        let timeline_hit=panel_rect(panels.timeline);
        let scene_hit=panel_rect(panels.preview);
        if ui.ctx().input(|i| i.pointer.any_pressed() &&
            !i.pointer.interact_pos().is_some_and(|p|scene_hit.contains(p))) {
            self.render_studio.navigation.selected=false;
            self.set_studio_mouse_look(false);
        }
        if ui.ctx().input(|i| i.pointer.any_pressed() &&
            i.pointer.interact_pos().is_some_and(|p|timeline_hit.contains(p)||scene_hit.contains(p))) {
            ui.ctx().memory_mut(|m| {
                if let Some(id)=m.focused(){m.surrender_focus(id);}
            });
        }
        if ui.ctx().input(|i|i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p|timeline_hit.contains(p))) {
            self.render_studio.cinematic_preview=None;self.profile_preview_key=None;
        }
        if !locked && !ui.ctx().text_edit_focused() {
            if let Some(preview)=self.render_studio.cinematic_preview.as_mut() {
                if ui.ctx().input(|i|i.key_pressed(egui::Key::Space)) {preview.toggle();}
                let (left,right)=ui.ctx().input(|i|(i.key_pressed(egui::Key::ArrowLeft),i.key_pressed(egui::Key::ArrowRight)));
                if left||right {let t=preview.elapsed()+if left {-1.}else{1.}/f64::from(preview.clip.fps);preview.seek(t);}
            }
        }
        // Snapshot state before any mouse edit. Pointer-held edits coalesce into
        // one undo step when released; keyboard edits commit immediately.
        self.render_studio.init_history();
        if !locked && self.render_studio.spatial.drag.is_none() && self.render_studio.cinematic_preview.is_none() {
            // Unlike the global menu's keyboard ownership bit, this specifically
            // checks for a focused edit widget. Keyboard shortcuts still work
            // after clicking a timeline item, even if the page owns keyboard input.
            let typing = ui.ctx().text_edit_focused();
            let (play,left,right,split,mark_in,mark_out) = ui.ctx().input(|i| (
                i.key_pressed(egui::Key::Space),i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),i.key_pressed(egui::Key::K),
                i.key_pressed(egui::Key::I),i.key_pressed(egui::Key::O),
            ));
            let (ctrl,shift,undo,redo,delete)=ui.ctx().input(|i|(
                i.modifiers.command || i.modifiers.ctrl,i.modifiers.shift,
                i.key_pressed(egui::Key::Z),i.key_pressed(egui::Key::Y),
                i.key_pressed(egui::Key::Delete)||i.key_pressed(egui::Key::Backspace)));
            if !typing && ctrl && undo && !shift {
                if self.render_studio.undo_edit(){self.profile_preview_key=None;self.render_studio_update_backdrop();}
            } else if !typing && ctrl && (redo || (shift && undo)) {
                if self.render_studio.redo_edit(){self.profile_preview_key=None;self.render_studio_update_backdrop();}
            } else if !typing && !ctrl && delete {
                if self.render_studio.timeline.delete_selection(){self.profile_preview_key=None;}
            }
            if self.render_studio.handle_clipboard_input(ui.ctx()) {
                self.profile_preview_key=None;
            }
            if !typing && play { self.render_studio.timeline.toggle_play(); }
            if !typing && split {
                let t=self.render_studio.timeline_time();
                let i=self.render_studio.timeline.selected_clip.filter(|&i|
                    self.render_studio.timeline.clips.get(i).is_some_and(|c|t>c.start&&t<c.end()))
                    .or_else(||self.render_studio.timeline.clips.iter().position(|c|t>c.start&&t<c.end()));
                if let Some(i)=i {
                    if self.render_studio.timeline.split_clip(i,t){self.profile_preview_key=None;}
                }
            }
            if !typing && mark_in {let t=self.render_studio.timeline_time();self.render_studio.timeline.set_in(t);}
            if !typing && mark_out {let t=self.render_studio.timeline_time();self.render_studio.timeline.set_out(t);}

            let step = 1.0 / self.render_studio.fps.max(1) as f64;
            if !typing && (left || right) {
                let t = self.render_studio.timeline_time();
                self.render_studio.timeline.seek(t + if left { -step } else { step });
                self.profile_preview_key = None;
            }
        }

        let header = panel_rect(panels.header);
        ui.scope_builder(egui::UiBuilder::new().id_salt("studio_transport").max_rect(header), |ui| {
            egui::Frame::new().fill(egui::Color32::from_rgb(18,25,36))
                .stroke(egui::Stroke::new(1.0,theme::LINE)).inner_margin(egui::Margin::same(7))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.strong("RENDER STUDIO"); ui.separator();
                        let t = self.render_studio.timeline_time();
                        if ui.add_enabled(!locked && self.render_studio.cinematic_preview.is_none(), egui::Button::new(if self.render_studio.timeline.playing {"⏸ Pause"} else {"▶ Play"})).clicked() {
                            self.render_studio.timeline.toggle_play();
                        }
                        if ui.add_enabled(!locked, egui::Button::new("◀ In")).clicked() {
                            let (in_time,_) = self.render_studio.timeline.export_range();
                            self.render_studio.timeline.seek(in_time);
                        }
                        ui.label(format!("{t:.2}s / {:.2}s",self.render_studio.timeline.duration()));
                        ui.separator();
                        if ui.add_enabled(!locked && self.render_studio.cinematic_preview.is_none(), egui::Button::new("+ Camera key")).clicked() {
                            self.capture_studio_preview_camera(t);
                        }
                        if ui.add_enabled(!locked && self.render_studio.spatial.scene_camera.is_none() && self.render_studio.cinematic_preview.is_none(), egui::Button::new("Frame actor"))
                            .on_hover_text("Aim the selected camera key at the active character, resetting its position to the studio framing viewpoint")
                            .clicked() {
                            self.render_studio_frame_actor();
                        }
                        ui.checkbox(&mut self.render_studio.timeline.snap,"Snap 30fps");
                        let (range_in,range_out)=self.render_studio.timeline.export_range();
                        ui.small(format!("IN {range_in:.2} / OUT {range_out:.2}"));
                        ui.separator();
                        if ui.button("RENDER…").clicked() { self.render_studio.cinematic_preview=None;self.profile_preview_key=None;self.render_studio.show_render=true; }
                        if ui.add_enabled(!locked,egui::Button::new("Save As…")).clicked() {
                            self.render_studio.show_save_as=true;
                        }
                        if ui.add_enabled(!locked,egui::Button::new("Open…")).clicked() {
                            self.render_studio.show_open_project=true;
                        }
                        if ui.add_enabled(!locked,egui::Button::new("Save")).clicked() {
                            self.save_render_studio_project();
                        }
                        if ui.add_enabled(!locked,egui::Button::new("Undo")).clicked() {
                            if self.render_studio.undo_edit(){self.profile_preview_key=None;}
                        }
                        if ui.add_enabled(!locked,egui::Button::new("Redo")).clicked() {
                            if self.render_studio.redo_edit(){self.profile_preview_key=None;}
                        }
                    });
                });
        });
        let library = panel_rect(panels.library);
        // A Frame::show() inside a scope_builder(max_rect) still sizes its
        // background from its *content* and can grow across the 3D viewport.
        // Paint a hard-bounded panel instead and keep every child clipped to
        // Taffy's intended library rect. This is a real layout boundary, not
        // extra space reserved for future library content.
        ui.painter().rect_filled(library, 0.0, egui::Color32::from_rgb(16,24,35));
        ui.painter().rect_stroke(library, 0.0,
            egui::Stroke::new(1.0,theme::LINE),egui::StrokeKind::Inside);
        let library_content = library.shrink(9.0);
        ui.scope_builder(egui::UiBuilder::new().id_salt("studio_library")
            .max_rect(library_content), |ui| {
                ui.set_clip_rect(library_content);
                ui.set_max_width(library_content.width());
                ui.set_width(library_content.width());
                ui.heading("MEDIA LIBRARY");
                egui::ScrollArea::vertical().id_salt("studio_library_scroll")
                    .max_height((library_content.height()-42.0).max(80.0))
                    .show(ui,|ui| {
                        self.egui_render_studio_media_library(ui);
                        if panels.inspector[2] <= panels.inspector[0] {
                            ui.separator();
                            ui.collapsing("KEYFRAME INSPECTOR", |ui| self.egui_render_studio_inspector(ui));
                        }
                    });
        });
        let preview = panel_rect(panels.preview);
        let response = ui.interact(preview,ui.make_persistent_id("studio_stage_navigation"),egui::Sense::click_and_drag());
        if !locked {self.set_asset_preview_viewport(preview,ui.ctx().pixels_per_point());}
        let overlay_busy=self.egui_studio_spatial_overlay(ui,preview,locked);
        self.egui_studio_preview_navigation(ui,&response,overlay_busy,locked);
        // The editor intentionally shows no actor outside animation clips.
        // Distinguish that from a camera that has wandered away, and from a
        // failed Ghoul2 pose/draw, instead of leaving an unexplained green box.
        let t = self.render_studio.timeline_time();
        let active = self.render_studio.timeline.active_actors(t);
        let hint = if self.render_studio.cinematic_preview.is_some() {None} else if active.is_empty() {
            Some("No active character clip at the playhead. Drag an animation or scrub onto a clip.")
        } else if self.profile_dynamic_models.is_empty() {
            Some("No character surfaces produced. Check this character model / animation; see console.")
        } else {
            let first = active[0];
            let motion = first.motion_at(t, self.render_studio.timeline.interpolation !=
                crate::app::render_studio_timeline::StudioInterpolation::Linear);
            let center = glam::Vec3::new(
                88.0*self.profile_preview_zoom.clamp(0.45,2.5)+motion.position[0],
                18.0+motion.position[1],
                -22.0+first.lane as f32*26.0+motion.position[2]);
            let camera=self.asset_preview_camera();
            let direction=(center-camera.position).normalize_or_zero();
            (self.render_studio.spatial.scene_camera.is_none() && camera.forward().dot(direction) < 0.8)
                .then_some("Camera is pointed away from the character. Click FRAME ACTOR above.")
        };
        if let Some(hint)=hint {
            ui.painter().text(preview.left_bottom()+egui::vec2(12.0,-12.0),
                egui::Align2::LEFT_BOTTOM,hint,
                egui::FontId::proportional(12.0),egui::Color32::from_rgb(240,220,172));
        }
        ui.painter().rect_stroke(preview,egui::CornerRadius::same(4),
            egui::Stroke::new(1.0,theme::LINE),egui::StrokeKind::Inside);
        ui.painter().text(preview.left_bottom()+egui::vec2(10.0,-31.0),egui::Align2::LEFT_BOTTOM,
            format!("RMB: LOOK  |  WASD: MOVE  |  E/C: UP/DOWN  |  WHEEL: SPEED ({:.0})", self.render_studio.navigation.speed),
            egui::FontId::proportional(10.0),theme::TEXT_DIM);
        if panels.inspector[2] > panels.inspector[0] {
            let inspector = panel_rect(panels.inspector);
            ui.scope_builder(egui::UiBuilder::new().id_salt("studio_inspector").max_rect(inspector), |ui| {
                egui::Frame::new().fill(egui::Color32::from_rgb(16,24,35))
                    .stroke(egui::Stroke::new(1.0,theme::LINE)).inner_margin(egui::Margin::same(9))
                    .show(ui,|ui| {
                        ui.set_width((inspector.width()-18.0).max(80.0));
                        ui.heading("INSPECTOR");
                        egui::ScrollArea::vertical().id_salt("studio_inspector_scroll")
                            .max_height((inspector.height()-60.0).max(80.0))
                            .show(ui,|ui| self.egui_render_studio_inspector(ui));
                    });
            });
        }
        self.egui_render_studio_timeline(ui, panel_rect(panels.timeline), locked);
        if ui.ctx().input(|i|i.pointer.any_released()) {self.render_studio.media_drag=None;}
        self.egui_render_studio_project_dialogs(ui.ctx());
        self.egui_render_studio_model_picker(ui.ctx());
        self.egui_render_studio_render_dialog(ui.ctx());
        self.update_profile_preview();
        self.render_studio.track_edit(ui.ctx().input(|i|i.pointer.primary_down()));
        if !locked && (self.render_studio.timeline.playing || self.render_studio.cinematic_preview.as_ref().is_some_and(|p|p.playing)) {ui.ctx().request_repaint();}
    }

    fn egui_render_studio_media_library(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.render_studio.media_tab,StudioMediaTab::Jka,"JKA Anims");
            ui.selectable_value(&mut self.render_studio.media_tab,StudioMediaTab::Fortnite,"Fortnite Anims");
            ui.selectable_value(&mut self.render_studio.media_tab,StudioMediaTab::Cutscenes,"Single-player Cinematics");
        });
        ui.separator();
        let locked=self.render_studio.export.is_some() || self.render_studio.still.is_some();
        match self.render_studio.media_tab {
            StudioMediaTab::Jka => {
                ui.add(egui::TextEdit::singleline(&mut self.render_studio.animation_search)
                    .hint_text("Search JKA animations…"));
                let needle=self.render_studio.animation_search.trim().to_ascii_lowercase();
                let choices:Vec<String>=(0..1600).filter_map(jka_movement::animation_name)
                    .filter(|name|needle.is_empty()||name.to_ascii_lowercase().contains(&needle))
                    .take(150).map(str::to_owned).collect();
                if choices.is_empty(){ui.small("No matching animations.");}
                for name in choices {
                    self.egui_render_studio_media_tile(ui,StudioMediaDrag {name,emote_id:None,cinematic:None},locked);
                }
            }
            StudioMediaTab::Fortnite => {
                ui.add(egui::TextEdit::singleline(&mut self.render_studio.emote_search)
                    .hint_text("Search Fortnite emotes…"));
                let needle=self.render_studio.emote_search.trim().to_ascii_lowercase();
                match crate::emotes::catalog(&self.base) {
                    Ok(catalog) => {
                        let choices:Vec<StudioMediaDrag>=catalog.iter().filter(|emote|
                            needle.is_empty()||emote.label.to_ascii_lowercase().contains(&needle)
                            ||emote.id.to_ascii_lowercase().contains(&needle))
                            .take(150).map(|emote| {
                                let variant=crate::emotes::default_variant_index(emote);
                                let resolved=emote.variants.get(variant).unwrap_or(emote);
                                StudioMediaDrag {name:crate::emotes::display_title(emote).to_owned(),emote_id:Some(resolved.id.clone()),cinematic:None}
                            }).collect();
                        if choices.is_empty(){ui.small("No matching emotes.");}
                        for choice in choices {self.egui_render_studio_media_tile(ui,choice,locked);}
                    }
                    Err(_)=>{ui.small("Fortnite emote catalog unavailable.");}
                }
            }
            StudioMediaTab::Cutscenes => self.egui_studio_cinematic_library(ui,locked),
        }
        ui.separator();
        ui.small("Drag an animation tile onto a character track. Character names/models and scene background live on the timeline; export settings are under RENDER…");
        if !self.render_studio.status.is_empty(){ui.small(&self.render_studio.status);}
    }

    pub(super) fn egui_render_studio_media_tile(&mut self,ui:&mut egui::Ui,asset:StudioMediaDrag,locked:bool) {
        let (rect,r)=ui.allocate_exact_size(egui::vec2(ui.available_width().max(70.0),38.0),
            if locked {egui::Sense::hover()} else {egui::Sense::click_and_drag()});
        ui.painter().rect_filled(rect.shrink(2.0),4.0,
            if r.hovered() {egui::Color32::from_rgb(45,82,106)}else{egui::Color32::from_rgb(26,40,56)});
        let emblem=if asset.emote_id.is_some(){"♫"}else{"▶"};
        ui.painter().text(rect.left_center()+egui::vec2(12.0,0.0),egui::Align2::LEFT_CENTER,
            emblem,egui::FontId::proportional(16.0),theme::ACCENT);
        ui.painter().with_clip_rect(rect.shrink2(egui::vec2(30.0,0.0))).text(
            rect.left_center()+egui::vec2(38.0,0.0),egui::Align2::LEFT_CENTER,
            &asset.name,egui::FontId::proportional(11.0),egui::Color32::WHITE);
        if r.clicked() {
            if let Some(source)=asset.cinematic.as_ref() {
                ui.ctx().memory_mut(|m|if let Some(id)=m.focused(){m.surrender_focus(id);});
                self.preview_studio_cinematic(source);
            }
        }
        if r.drag_started() {
            self.render_studio.cinematic_preview=None;self.profile_preview_key=None;
            self.render_studio.media_drag=Some(asset.clone());
        }
        let details=asset.cinematic.as_ref().and_then(|source|self.render_studio.cinematics.catalog.as_ref()
            .and_then(|r|r.as_ref().ok()).and_then(|catalog|catalog.clips.iter().find(|clip|clip.source==*source)))
            .map(|clip|format!("{}\n{} / {}\n{} frames, {:.2} FPS, source loopFrames {}\nClick to preview; drag onto a character lane",clip.label,
                clip.source.gla,clip.source.animation,clip.frame_count,clip.fps,clip.loop_frames))
            .unwrap_or_else(||format!("{} - drag onto a character lane",asset.name));
        r.on_hover_text(details);
    }

    fn egui_render_studio_output(&mut self,ui:&mut egui::Ui) {
        let locked=self.render_studio.export.is_some() || self.render_studio.still.is_some();
        ui.add_enabled_ui(!locked, |ui| {
            ui.label(egui::RichText::new("VIDEO").strong().color(theme::ACCENT));
            ui.label(egui::RichText::new("04  EXPORT").strong().color(theme::ACCENT));
            ui.horizontal(|ui| {
                let previous_format = self.render_studio.preset;
                egui::ComboBox::from_id_salt("render_studio_quality")
                    .selected_text(self.render_studio.preset.label())
                    .show_ui(ui, |ui| {
                        for preset in StudioVideoFormat::OPTIONS {
                            ui.selectable_value(&mut self.render_studio.preset, preset, preset.label());
                        }
                    });
                if self.render_studio.preset != previous_format {
                    if self.render_studio.preset == StudioVideoFormat::Gif {
                        // GIF delays are centiseconds, so favor 10-30 fps.
                        self.render_studio.fps = 15;
                    } else if ![24, 30, 60, 120].contains(&self.render_studio.fps) {
                        self.render_studio.fps = 60;
                    }
                    if self.render_studio.preset.supports_alpha() && !previous_format.supports_alpha() {
                        self.render_studio.transparent = true;
                    } else if !self.render_studio.preset.supports_alpha() {
                        self.render_studio.transparent = false;
                    }
                }
                egui::ComboBox::from_id_salt("render_studio_export_fps")
                    .selected_text(format!("{} fps", self.render_studio.fps))
                    .show_ui(ui, |ui| {
                        let fps_options: &[u32] = if self.render_studio.preset == StudioVideoFormat::Gif {
                            &[10, 15, 24, 30]
                        } else {
                            &[24, 30, 60, 120]
                        };
                        for &fps in fps_options {
                            ui.selectable_value(&mut self.render_studio.fps, fps, format!("{fps} fps"));
                        }
                    });
            });
            ui.small(self.render_studio.preset.short_description());
            let can_export_alpha = self.render_studio.preset.supports_alpha();
            ui.add_enabled_ui(can_export_alpha, |ui| {
                ui.checkbox(&mut self.render_studio.transparent, "Transparent video / remove background");
            });
            if !can_export_alpha { self.render_studio.transparent = false; }
            ui.checkbox(&mut self.render_studio.export_depth, "Export depth EXR sequence (32-bit float)");
            if self.render_studio.transparent {
                match self.render_studio.preset {
                    StudioVideoFormat::Gif => ui.small("GIF alpha is 1-bit (threshold 128): soft hair and glow edges become either solid or transparent. GIF loops forever."),
                    StudioVideoFormat::WebmVp9 | StudioVideoFormat::WebmVp9Max => ui.small("True alpha via VP9 WebM. The selected background is excluded. Some desktop players display transparent areas as black."),
                    _ => ui.small("True alpha in ProRes 4444 MOV. The selected background is excluded from capture."),
                };
            } else if self.render_studio.preset == StudioVideoFormat::Gif {
                ui.small("Opaque looping GIF with its selected backdrop. At large window sizes, GIF files may be very large.");
            } else {
                ui.small("Export includes the selected backdrop; H.264 and ProRes 422 are opaque.");
            }
            ui.small("Depth EXRs contain camera-to-surface radial JKA units; empty pixels are 1,000,000.");
            ui.add_space(8.0);
            ui.label(egui::RichText::new("05  STILL SCREENSHOT").strong().color(theme::ACCENT));
            ui.checkbox(&mut self.render_studio.screenshot_alpha, "Transparent PNG (RGBA)");
            ui.checkbox(&mut self.render_studio.screenshot_depth, "Also write 32-bit EXR depth");
            ui.small("Capture the current scrubbed animation pose as a PNG without editor UI.");
            ui.add_space(9.0);
        });
        if locked {
            if self.render_studio.still.is_some() { ui.small("Capturing still screenshot…"); }
            if let Some(export) = self.render_studio.export.as_ref() {
                ui.add(egui::ProgressBar::new(export.frame_index as f32 / export.total_frames.max(1) as f32)
                    .text(format!("{} / {} frames", export.frame_index, export.total_frames)));
            }
            if ui.button("CANCEL EXPORT").clicked() {
                if self.render_studio.still.is_some() {
                    // The in-flight readback can still finish, but no PNG is written.
                    self.render_studio.still = None;
                    self.asset_preview_viewport_key = None;
                    self.render_command(crate::app::RenderCommand::SetAssetPreviewViewport(None));
                } else { self.cancel_render_studio_export(); }
            }
        } else if ui.add_enabled(!self.render_studio.timeline.clips.is_empty(),
            egui::Button::new("EXPORT TIMELINE")).clicked() {
            self.start_render_studio_export();
        }
        if !locked && ui.button("CAPTURE STILL PNG").clicked() {
            self.start_render_studio_screenshot();
        }
        if !self.render_studio.status.is_empty() { ui.small(&self.render_studio.status); }
    }

    fn egui_render_studio_render_dialog(&mut self,ctx:&egui::Context) {
        if !self.render_studio.show_render {return;}
        let mut open=true;
        egui::Window::new("RENDER / OUTPUT").id(egui::Id::new("render_studio_export_panel"))
            .open(&mut open).default_width(440.0).resizable(true).show(ctx,|ui| {
                egui::ScrollArea::vertical().max_height(640.0).show(ui,|ui| {
                    self.egui_render_studio_output(ui);
                });
            });
        self.render_studio.show_render=open;
    }

    fn egui_render_studio_model_picker(&mut self,ctx:&egui::Context) {
        let Some(lane)=self.render_studio.model_picker_lane else{return;};
        let mut open=true;
        let mut selected=None::<String>;
        egui::Window::new(format!("Character {} — Choose model",lane+1))
            .open(&mut open).default_width(430.0).show(ctx,|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.render_studio.model_search)
                    .hint_text("Search player models / skins"));
                let search=self.render_studio.model_search.trim().to_ascii_lowercase();
                if ui.button("Use project default model").clicked(){selected=Some(String::new());}
                let models:Vec<_>=self.profile_models.iter().filter(|entry|
                        search.is_empty() || entry.value.to_ascii_lowercase().contains(&search)
                        ||entry.model_name.to_ascii_lowercase().contains(&search)
                        ||entry.skin_name.to_ascii_lowercase().contains(&search)).collect();
                ui.small(format!("{} models / skins",models.len()));
                let current=self.render_studio.actor_model(lane);
                let row_height=ui.spacing().interact_size.y;
                egui::ScrollArea::vertical().id_salt(("studio_actor_models",lane,search))
                    .max_height(420.0).show_rows(ui,row_height,models.len(),|ui,rows| {
                    for row in rows {
                        let entry=models[row];
                        if ui.add_sized([ui.available_width(),row_height],egui::Button::selectable(
                            current==entry.value,format!("{} / {}",entry.model_name,entry.skin_name)).truncate()).clicked(){
                            selected=Some(entry.value.clone());
                        }
                    }
                });
            });
        if let Some(model)=selected {
            if let Some(slot)=self.render_studio.timeline.lane_models.get_mut(lane) {
                *slot=model;
                self.profile_preview_key=None;
            }
            open=false;
        }
        if !open {self.render_studio.model_picker_lane=None;}
    }

}

impl App {
    /// Hit areas have stable IDs and dragging is always measured against the
    /// pointer's *drag-start* position. Moving a rectangle never feeds back
    /// into its own delta (the old sticky-drag failure).
    fn egui_render_studio_timeline(&mut self, ui: &mut egui::Ui, rect: egui::Rect, locked: bool) {
        use crate::app::render_studio_timeline::{StudioDrag,StudioDragKind,StudioInterpolation};
        let (ctrl,shift)=ui.ctx().input(|input|(input.modifiers.ctrl||input.modifiers.command,input.modifiers.shift));
        // Consume Alt+wheel before the track scroll area can scroll vertically.
        let zoom_wheel=ui.ctx().input_mut(|input| {
            if input.modifiers.alt && input.pointer.hover_pos().is_some_and(|point|rect.contains(point)) {
                let wheel=input.smooth_scroll_delta.y;
                let x=input.pointer.hover_pos().map_or(rect.left(),|point|point.x);
                input.smooth_scroll_delta=egui::Vec2::ZERO;
                Some((x,wheel))
            } else {None}
        });
        if let Some((x,wheel))=zoom_wheel {
            self.render_studio.timeline.zoom_at(x-rect.left()-8.0-132.0,wheel);
        }
        let palette=egui::Color32::from_rgb(16,24,35);
        ui.scope_builder(egui::UiBuilder::new().id_salt("studio_timeline").max_rect(rect), |ui| {
            egui::Frame::new().fill(palette).stroke(egui::Stroke::new(1.0,theme::LINE))
                .inner_margin(egui::Margin::same(8)).show(ui,|ui| {
                    ui.set_width((rect.width()-16.0).max(150.0));
                    ui.horizontal_wrapped(|ui| {
                        ui.strong("TIMELINE");ui.separator();
                        ui.small(format!("{} clips · {} camera keys · {} lanes",
                            self.render_studio.timeline.clips.len(), self.render_studio.timeline.camera.len(),
                            self.render_studio.timeline.lanes));
                        let selected=self.render_studio.timeline.selected_items().len();
                        if selected>1 {ui.strong(format!("{selected} selected"));}
                        ui.separator();ui.label("Zoom").on_hover_text("Alt + scroll to zoom at the pointer");
                        ui.add(egui::Slider::new(&mut self.render_studio.timeline.zoom,24.0..=350.0)
                            .show_value(false));
                        if ui.add_enabled(!locked && self.render_studio.timeline.lanes<12,egui::Button::new("+ Layer")).clicked() {
                            self.render_studio.timeline.lanes+=1;
                            self.render_studio.timeline.lane_models.push(String::new());
                            self.render_studio.status="Added another independently animated character layer.".into();
                        }
                        if ui.add_enabled(!locked,egui::Button::new("+ Motion key")).clicked() {
                            let t=self.render_studio.timeline_time();
                            let i=self.render_studio.timeline.selected_clip.or_else(||self.render_studio.timeline.clips.iter()
                                .position(|c|t>=c.start&&t<=c.end()));
                            if let Some(i)=i {
                                let smooth=self.render_studio.timeline.interpolation!=StudioInterpolation::Linear;
                                let clip=&mut self.render_studio.timeline.clips[i];
                                let mut key=clip.motion_at(t,smooth);key.time=(clip.first_frame-clip.source_first) as f64/clip.source_fps+(t-clip.start).clamp(0.0,clip.duration());
                                let k=clip.add_motion_key(key);
                                self.render_studio.timeline.select_item(StudioItem::Motion(i,k),false,false);
                            }
                        }
                    });
                    let w=ui.available_width().max(160.0);
                    let available_h=(rect.height()-94.0).max(65.0);
                    let lane_h=34.0;
                    let ruler_h=30.0;
                    let rows=self.render_studio.timeline.track_count();
                    let content_h=(ruler_h+rows as f32*lane_h+4.0).max(available_h);
                    egui::ScrollArea::vertical().id_salt("studio_track_scroll")
                        .max_height(available_h).show(ui,|ui| {
                        let (draw,_)=ui.allocate_exact_size(egui::vec2(w,content_h),egui::Sense::hover());
                        let x0=draw.left()+132.0;
                        let right=draw.right();
                        let zoom=self.render_studio.timeline.zoom;
                        let scroll=self.render_studio.timeline.scroll;
                        let to_x=|t:f64|x0+(t as f32-scroll)*zoom;
                        let from_x=|x:f32|((x-x0)/zoom+scroll).max(0.0) as f64;
                        let painter=ui.painter().with_clip_rect(draw);
                        painter.rect_filled(draw,2.0,egui::Color32::from_rgb(11,18,28));
                        for row in 0..rows {
                            let y=draw.top()+ruler_h+row as f32*lane_h;
                            let b=egui::Rect::from_min_size(egui::pos2(draw.left(),y),egui::vec2(draw.width(),lane_h));
                            painter.rect_filled(b,0.0,if row%2==0 {egui::Color32::from_rgb(23,31,43)} else {egui::Color32::from_rgb(19,27,39)});
                            let label=if row==0 {"CAMERA".to_owned()}
                                else if row==1 {"BACKGROUND".to_owned()}
                                else {
                                    let lane=(row-2)/2;
                                    if row%2==1 {format!("  ↳ Motion {:02}",lane+1)}
                                    else {
                                        let model=self.render_studio.timeline.lane_models.get(lane)
                                            .filter(|m|!m.trim().is_empty()).unwrap_or(&self.render_studio.model);
                                        let model=if model.is_empty(){"Choose model"}else{model.as_str()};
                                        let model=model.split('/').next().unwrap_or(model);
                                        format!("✎ {model} · {:02}",lane+1)
                                    }
                                };
                            painter.text(egui::pos2(draw.left()+9.0,y+lane_h/2.0),egui::Align2::LEFT_CENTER,
                                label,egui::FontId::proportional(11.0),if row>=2 {studio_layer_color((row-2)/2)}else{theme::TEXT_DIM});
                            // Layer names act as model selectors. Subtrack labels are informational.
                            if !locked && row>=2 && row%2==0 {
                                let lane=(row-2)/2;
                                let label_rect=egui::Rect::from_min_size(egui::pos2(draw.left(),y),
                                    egui::vec2(128.0,lane_h));
                                let r=ui.interact(label_rect,ui.make_persistent_id(("studio_actor_model",lane)),egui::Sense::click());
                                if r.clicked(){self.render_studio.model_picker_lane=Some(lane);}
                                r.on_hover_text("Click to choose the player model for this character layer");
                            }
                            if !locked && row==1 {
                                let label_rect=egui::Rect::from_min_size(egui::pos2(draw.left(),y),
                                    egui::vec2(128.0,lane_h));
                                let r=ui.interact(label_rect,ui.make_persistent_id("studio_background_label"),egui::Sense::click());
                                if r.clicked(){self.render_studio.selected_background=true;
                                    self.render_studio.timeline.clear_selection();}
                                r.on_hover_text("Edit background in Inspector");
                            }
                            painter.line_segment([egui::pos2(draw.left(),y+lane_h),egui::pos2(right,y+lane_h)],
                                egui::Stroke::new(0.7,theme::LINE));
                        }
                        let bg_y=draw.top()+ruler_h+lane_h;
                        let bgbar=egui::Rect::from_min_max(egui::pos2(x0+2.0,bg_y+5.0),
                            egui::pos2(right-2.0,bg_y+lane_h-5.0));
                        painter.rect_filled(bgbar,4.0,if self.render_studio.selected_background {
                            egui::Color32::from_rgb(109,102,158)}else{egui::Color32::from_rgb(64,62,102)});
                        painter.text(bgbar.left_center()+egui::vec2(8.0,0.0),egui::Align2::LEFT_CENTER,
                            format!("Scene · {}",match self.render_studio.background {
                                StudioBackground::Black=>"Empty / transparent",StudioBackground::Green=>"Green screen",
                                StudioBackground::Blue=>"Blue screen",StudioBackground::Levelshot=>"Levelshot"}),
                            egui::FontId::proportional(10.0),egui::Color32::WHITE);
                        if !locked {
                            let bg=ui.interact(bgbar,ui.make_persistent_id("studio_background_track"),egui::Sense::click());
                            if bg.clicked(){self.render_studio.selected_background=true;
                                self.render_studio.timeline.clear_selection();}
                            bg.context_menu(|ui| {
                                if ui.button("Edit scene background").clicked(){self.render_studio.selected_background=true;ui.close();}
                            });
                        }
                        // Show a destination indicator while dragging a bin asset.
                        if let (Some(asset),Some(pointer))=(self.render_studio.media_drag.as_ref(),
                            ui.ctx().input(|i|i.pointer.latest_pos())) {
                            let row=((pointer.y-draw.top()-ruler_h)/lane_h).floor() as isize;
                            if pointer.x>=x0 && pointer.x<=right && row>=2 && (row as usize)<rows {
                                let lane=crate::app::render_studio_timeline::StudioTimeline::lane_for_row(row as usize).unwrap_or(0);
                                let y=draw.top()+ruler_h+
                                    crate::app::render_studio_timeline::StudioTimeline::character_row(lane) as f32*lane_h;
                                let track=egui::Rect::from_min_size(egui::pos2(x0,y),egui::vec2(right-x0,lane_h));
                                painter.rect_stroke(track.shrink(1.0),3.0,egui::Stroke::new(2.0,theme::ACCENT),egui::StrokeKind::Inside);
                                painter.text(pointer+egui::vec2(12.0,-16.0),egui::Align2::LEFT_CENTER,
                                    format!("Drop {} → character {}",asset.name,lane+1),
                                    egui::FontId::proportional(11.0),egui::Color32::WHITE);
                            }
                        }
                        let bottom=draw.top()+ruler_h+rows as f32*lane_h;
                        painter.line_segment([egui::pos2(x0,draw.top()),egui::pos2(x0,bottom)],egui::Stroke::new(1.0,theme::LINE));
                        let visible=((right-x0)/zoom).max(0.0) as f64;
                        let step=if zoom>=130.0 {0.5}else if zoom>=55.0 {1.0}else{2.0};
                        let first=(scroll as f64/step).floor().max(0.0) as i32;
                        let last=((scroll as f64+visible)/step).ceil() as i32;
                        for tick in first..=last.min(first+500) {
                            let t=tick as f64*step;let x=to_x(t);
                            if x<x0||x>right {continue;}
                            painter.line_segment([egui::pos2(x,draw.top()+ruler_h-7.0),egui::pos2(x,bottom)],
                                egui::Stroke::new(0.7,egui::Color32::from_gray(53)));
                            painter.text(egui::pos2(x+3.0,draw.top()+4.0),egui::Align2::LEFT_TOP,
                                format!("{t:.1}s"),egui::FontId::proportional(10.0),theme::TEXT_DIM);
                        }
                        let (in_time,out_time)=self.render_studio.timeline.export_range();
                        let in_x=to_x(in_time);let out_x=to_x(out_time);
                        let shaded=egui::Rect::from_min_max(egui::pos2(in_x.max(x0),draw.top()+ruler_h-5.0),
                            egui::pos2(out_x.min(right).max(in_x.max(x0)),draw.top()+ruler_h-2.0));
                        painter.rect_filled(shaded,0.0,egui::Color32::from_rgb(97,189,150));
                        let ruler=egui::Rect::from_min_max(egui::pos2(x0,draw.top()),egui::pos2(right,draw.top()+ruler_h));
                        if !locked {
                            let r=ui.interact(ruler,ui.make_persistent_id("studio_ruler"),egui::Sense::click_and_drag());
                            if let Some(pointer)=r.interact_pointer_pos().filter(|_|r.clicked()||r.dragged()) {
                                let near_marker=(pointer.x-in_x).abs()<11.0 || (pointer.x-out_x).abs()<11.0;
                                if !near_marker && self.render_studio.timeline.drag.is_none() {
                                    self.render_studio.timeline.seek(from_x(pointer.x));self.profile_preview_key=None;
                                }
                            }
                        }
                        // Background interaction covers otherwise empty track space. Clip/key
                        // hit rectangles are registered afterwards and take precedence.
                        if !locked {
                            let track_area=egui::Rect::from_min_max(
                                egui::pos2(x0,draw.top()+ruler_h),egui::pos2(right,bottom));
                            let area=ui.interact(track_area,ui.make_persistent_id("studio_empty_track"),egui::Sense::click_and_drag());
                            let point=area.interact_pointer_pos();
                            if area.secondary_clicked() {
                                if let Some(p)=point {
                                    self.render_studio.context_time=Some(from_x(p.x));
                                    let row=((p.y-draw.top()-ruler_h)/lane_h).floor().max(0.0) as usize;
                                    self.render_studio.context_lane=crate::app::render_studio_timeline::StudioTimeline::lane_for_row(row);
                                }
                            }
                            if area.clicked() || area.dragged_by(egui::PointerButton::Primary) {
                                if let Some(p)=point {
                                    self.render_studio.timeline.seek(from_x(p.x));
                                    self.render_studio.timeline.clear_selection();
                                    self.render_studio.selected_background=
                                        ((p.y-draw.top()-ruler_h)/lane_h).floor() as usize==1;
                                    self.profile_preview_key=None;
                                }
                            }
                            area.context_menu(|ui| {
                                let time=self.render_studio.context_time.unwrap_or(self.render_studio.timeline_time());
                                if ui.add_enabled(self.render_studio.clipboard.is_some(),egui::Button::new("Paste at cursor  (Ctrl+V)")).clicked(){
                                    self.render_studio.timeline.seek(time);
                                    // For clips, use the lane under the pointer as destination.
                                    let lane=self.render_studio.context_lane;
                                    if self.render_studio.paste_selection_on_lane(lane){self.profile_preview_key=None;}
                                    ui.close();
                                }
                                if ui.button("Add camera key here").clicked(){
                                    self.capture_studio_preview_camera(time);ui.close();
                                }
                                if ui.button("Set export IN here").clicked(){self.render_studio.timeline.set_in(time);ui.close();}
                                if ui.button("Set export OUT here").clicked(){self.render_studio.timeline.set_out(time);ui.close();}
                                if ui.button("Edit background").clicked(){self.render_studio.selected_background=true;ui.close();}
                            });
                        }
                        // IN / OUT markers sit above ruler scrubbing. Their drag
                        // origin stays fixed even while marker geometry moves.
                        for (kind,time,color) in [
                            (StudioDragKind::In,in_time,egui::Color32::from_rgb(80,214,141)),
                            (StudioDragKind::Out,out_time,egui::Color32::from_rgb(230,175,75))] {
                            let x=to_x(time);
                            if x<x0-10.0||x>right+10.0 {continue;}
                            let marker=egui::Rect::from_center_size(egui::pos2(x,draw.top()+ruler_h/2.0),egui::vec2(17.0,28.0));
                            painter.rect_filled(marker,3.0,color);
                            painter.text(marker.center(),egui::Align2::CENTER_CENTER,
                                if kind==StudioDragKind::In {"I"}else{"O"},egui::FontId::proportional(10.0),egui::Color32::BLACK);
                            if !locked {
                                let r=ui.interact(marker,ui.make_persistent_id(("studio_marker",kind as u8)),egui::Sense::click_and_drag());
                                if r.drag_started() {self.render_studio.timeline.drag=Some(StudioDrag{
                                    kind,index:0,sub_index:0,pointer_x:r.interact_pointer_pos().map_or(x,|p|p.x),initial_scroll:scroll,
                                    initial_time:time,initial_first:0,initial_end:0,initial_start:0.0});}
                                if let Some(d)=self.render_studio.timeline.drag.filter(|d|r.dragged()&&d.kind==kind) {
                                    if let Some(pointer)=r.interact_pointer_pos() {
                                        let t=d.initial_time+f64::from((pointer.x-d.pointer_x)/zoom);
                                        if kind==StudioDragKind::In {self.render_studio.timeline.set_in(t);}
                                        else {self.render_studio.timeline.set_out(t);}
                                    }
                                }
                                if r.drag_stopped(){self.render_studio.timeline.drag=None;}
                                r.on_hover_text(format!("{}: {:.3}s · drag to set export range",if kind==StudioDragKind::In{"IN"}else{"OUT"},time));
                            }
                        }
                        // Stable index-based IDs. Clips are *never* sorted during
                        // a drag, and pointer displacement is never accumulated.
                        let clip_data:Vec<_>=self.render_studio.timeline.clips.iter().enumerate()
                            .map(|(i,c)|(i,c.start,c.end(),c.label().to_owned(),c.lane,c.first_frame,c.end_frame,c.source_fps)).collect();
                        for (i,start,end,label,lane,first_frame,end_frame,fps) in clip_data {
                            if i>=self.render_studio.timeline.clips.len() {continue;}
                            let x1=to_x(start).max(x0);let x2=to_x(end).min(right);
                            if x2<=x1 {continue;}
                            let top=draw.top()+ruler_h+crate::app::render_studio_timeline::StudioTimeline::character_row(lane) as f32*lane_h+4.0;
                            let block=egui::Rect::from_min_max(egui::pos2(x1,top),egui::pos2(x2,top+lane_h-8.0));
                            let selected=self.render_studio.timeline.is_selected(StudioItem::Clip(i));
                            // Tiny clips retain a 28px context-menu target, without
                            // changing the actual painted duration.
                            let hit=egui::Rect::from_center_size(block.center(),
                                egui::vec2(block.width().max(28.0),block.height()));
                            if selected { painter.rect_stroke(block.expand(3.0),5.0,
                                egui::Stroke::new(2.5,egui::Color32::from_rgb(250,220,105)),egui::StrokeKind::Inside); }
                            painter.rect_filled(block,3.0,if selected {egui::Color32::from_rgb(46,151,169)}else{egui::Color32::from_rgb(36,93,124)});
                            painter.rect_stroke(block,3.0,egui::Stroke::new(1.0,if selected {egui::Color32::WHITE}else{theme::LINE}),egui::StrokeKind::Inside);
                            if block.width()>32.0 {
                                painter.with_clip_rect(block.shrink(3.0)).text(
                                    block.left_center()+egui::vec2(8.0,0.0),egui::Align2::LEFT_CENTER,
                                    format!("{} [{}..{}]",label,first_frame,end_frame),
                                    egui::FontId::proportional(10.0),egui::Color32::WHITE);
                            }
                            // The trim handles are separate controls, registered
                            // after the body so they win the edge hit test.
                            if !locked {
                                let r=ui.interact(hit,ui.make_persistent_id(("studio_clip",i)),egui::Sense::click_and_drag());
                                if r.secondary_clicked() {
                                    if !self.render_studio.timeline.is_selected(StudioItem::Clip(i)) {
                                        self.render_studio.timeline.select_item(StudioItem::Clip(i),false,false);
                                    }
                                    self.render_studio.selected_background=false;
                                }
                                if r.clicked() {
                                    self.render_studio.selected_background=false;
                                    self.render_studio.timeline.select_item(StudioItem::Clip(i),ctrl,shift);
                                    if !ctrl && !shift {self.render_studio.timeline.seek(start);}
                                    self.profile_preview_key=None;
                                }
                                if r.drag_started(){self.render_studio.timeline.select_item(StudioItem::Clip(i),false,false);
                                    self.render_studio.timeline.drag=Some(StudioDrag{
                                    kind:StudioDragKind::Clip,index:i,sub_index:0,
                                    pointer_x:r.interact_pointer_pos().map_or(x1,|p|p.x),initial_scroll:scroll,initial_time:start,
                                    initial_first:first_frame,initial_end:end_frame,initial_start:start});}
                                if let Some(d)=self.render_studio.timeline.drag.filter(|d|r.dragged()&&d.kind==StudioDragKind::Clip&&d.index==i) {
                                    if let Some(p)=r.interact_pointer_pos(){
                                        self.render_studio.timeline.move_clip(i,d.initial_time+f64::from((p.x-d.pointer_x)/zoom));
                                        self.render_studio.timeline.selected_clip=Some(i);
                                        self.render_studio.timeline.selected_camera=None;
                                        self.profile_preview_key=None;
                                    }
                                }
                                if r.drag_stopped(){self.render_studio.timeline.drag=None;}
                                let count_before_menu=self.render_studio.timeline.clips.len();
                                r.context_menu(|ui| {
                                    if ui.button("Split at playhead  (K)").clicked(){
                                        let t=self.render_studio.timeline_time();
                                        self.render_studio.timeline.split_clip(i,t);ui.close();
                                        self.profile_preview_key=None;
                                    }
                                    if ui.button("Duplicate after clip").clicked(){
                                        self.render_studio.timeline.duplicate_clip(i);ui.close();
                                    }
                                    if ui.button("Add character motion key").clicked(){
                                        let t=self.render_studio.timeline_time();
                                        let smooth=self.render_studio.timeline.interpolation!=StudioInterpolation::Linear;
                                        if let Some(clip)=self.render_studio.timeline.clips.get_mut(i){
                                            let mut k=clip.motion_at(t,smooth);k.time=(clip.first_frame-clip.source_first) as f64/clip.source_fps+(t-clip.start).clamp(0.0,clip.duration());
                                            let ki=clip.add_motion_key(k);self.render_studio.timeline.select_item(StudioItem::Motion(i,ki),false,false);
                                        }
                                        ui.close();
                                    }
                                    ui.menu_button("Move to layer",|ui| {
                                        for layer in 0..self.render_studio.timeline.lanes {
                                            if ui.button(format!("Layer {}",layer+1)).clicked(){
                                                let original=self.render_studio.timeline.clips[i].lane;
                                                self.render_studio.timeline.clips[i].lane=layer;
                                                let start=self.render_studio.timeline.clips[i].start;
                                                self.render_studio.timeline.move_clip(i,start);
                                                self.render_studio.status=format!("Moved from layer {} to {}",original+1,layer+1);
                                                ui.close();
                                            }
                                        }
                                    });
                                    if ui.button("Copy clip  (Ctrl+C)").clicked(){
                                        if !self.render_studio.timeline.is_selected(StudioItem::Clip(i)) {
                                            self.render_studio.timeline.select_item(StudioItem::Clip(i),false,false);
                                        }
                                        self.render_studio.copy_selection();ui.close();
                                    }
                                    if ui.button("Delete clip").clicked(){
                                        if !self.render_studio.timeline.is_selected(StudioItem::Clip(i)) {
                                            self.render_studio.timeline.select_item(StudioItem::Clip(i),false,false);
                                        }
                                        self.render_studio.timeline.delete_selection();
                                        self.profile_preview_key=None;ui.close();
                                    }
                                });
                                if self.render_studio.timeline.clips.len()<count_before_menu {continue;}
                                if block.width()>26.0 { for (kind,edge_x) in [(StudioDragKind::TrimLeft,x1),(StudioDragKind::TrimRight,x2)] {
                                    painter.rect_filled(egui::Rect::from_center_size(egui::pos2(edge_x,block.center().y),egui::vec2(4.0,block.height()-5.0)),1.0,egui::Color32::WHITE);
                                    let grip=egui::Rect::from_center_size(egui::pos2(edge_x,block.center().y),egui::vec2(10.0,block.height()));
                                    let grip_r=ui.interact(grip,ui.make_persistent_id(("studio_trim",i,kind as u8)),egui::Sense::click_and_drag());
                                    if grip_r.drag_started(){self.render_studio.timeline.drag=Some(StudioDrag{
                                        kind,index:i,sub_index:0,pointer_x:grip_r.interact_pointer_pos().map_or(edge_x,|p|p.x),initial_scroll:scroll,
                                        initial_time:start,initial_first:first_frame,
                                        initial_end:end_frame+self.render_studio.timeline.clips[i].tail_frames,initial_start:start});}
                                    if let Some(d)=self.render_studio.timeline.drag.filter(|d|grip_r.dragged()&&d.kind==kind&&d.index==i) {
                                        if let Some(p)=grip_r.interact_pointer_pos(){
                                            self.render_studio.timeline.trim_clip(i,kind,d.initial_start,d.initial_first,d.initial_end,
                                                f64::from((p.x-d.pointer_x)/zoom));
                                            self.profile_preview_key=None;
                                        }
                                    }
                                    if grip_r.drag_stopped(){self.render_studio.timeline.drag=None;}
                                    let c=&self.render_studio.timeline.clips[i];
                                    grip_r.on_hover_text(format!("{} · frame {} · {:.3}s (drag edge)",
                                        if kind==StudioDragKind::TrimLeft{"IN"}else{"OUT"},
                                        if kind==StudioDragKind::TrimLeft{c.first_frame}else{c.end_frame},
                                        if kind==StudioDragKind::TrimLeft{c.start}else{c.end()}));
                                }}
                            }
                            let _=fps;
                        }
                        // Camera diamonds use exactly the same anchored drag model.
                        let camera_keys:Vec<_>=self.render_studio.timeline.camera.iter().enumerate().map(|(i,k)|(i,k.time)).collect();
                        for (i,time) in camera_keys {
                            let x=to_x(time);if x<x0-12.0||x>right+12.0 {continue;}
                            let y=draw.top()+ruler_h+lane_h/2.0;
                            let selected=self.render_studio.timeline.is_selected(StudioItem::Camera(i));
                            if selected {
                                painter.circle_filled(egui::pos2(x,y),15.0,egui::Color32::from_rgb(248,218,117).gamma_multiply(0.25));
                                painter.circle_stroke(egui::pos2(x,y),14.0,egui::Stroke::new(2.0,egui::Color32::WHITE));
                            }
                            painter.add(egui::Shape::convex_polygon(vec![egui::pos2(x,y-9.0),egui::pos2(x+9.0,y),egui::pos2(x,y+9.0),egui::pos2(x-9.0,y)],
                                if selected {egui::Color32::from_rgb(251,211,117)} else {egui::Color32::from_rgb(206,144,72)},egui::Stroke::new(1.0,egui::Color32::WHITE)));
                            if !locked {
                                let hit=egui::Rect::from_center_size(egui::pos2(x,y),egui::vec2(22.0,28.0));
                                let r=ui.interact(hit,ui.make_persistent_id(("studio_camera",i)),egui::Sense::click_and_drag());
                                if r.clicked(){self.render_studio.timeline.select_item(StudioItem::Camera(i),ctrl,shift);
                                    self.render_studio.selected_background=false;
                                    if !ctrl && !shift {self.render_studio.timeline.seek(time);}self.profile_preview_key=None;}
                                if r.drag_started(){self.render_studio.timeline.select_item(StudioItem::Camera(i),false,false);
                                    self.render_studio.timeline.drag=Some(StudioDrag{
                                    kind:StudioDragKind::Camera,index:i,sub_index:0,
                                    pointer_x:r.interact_pointer_pos().map_or(x,|p|p.x),initial_scroll:scroll,initial_time:time,
                                    initial_first:0,initial_end:0,initial_start:0.0});}
                                if let Some(d)=self.render_studio.timeline.drag.filter(|d|r.dragged()&&d.kind==StudioDragKind::Camera&&d.index==i){
                                    if let Some(p)=r.interact_pointer_pos() {
                                        self.render_studio.timeline.move_camera_key(i,d.initial_time+f64::from((p.x-d.pointer_x)/zoom));
                                        self.render_studio.timeline.selected_camera=Some(i);
                                        let key_time=self.render_studio.timeline.camera[i].time;
                                        self.render_studio.timeline.seek(key_time);
                                        self.profile_preview_key=None;
                                    }
                                }
                                if r.drag_stopped(){self.render_studio.timeline.drag=None;}
                                let count_before_menu=self.render_studio.timeline.camera.len();
                                r.context_menu(|ui| {
                                    if ui.button("Copy camera key").clicked(){
                                        if !self.render_studio.timeline.is_selected(StudioItem::Camera(i)) {
                                            self.render_studio.timeline.select_item(StudioItem::Camera(i),false,false);
                                        }
                                        self.render_studio.copy_selection();ui.close();
                                    }
                                    if ui.button("Delete camera key").clicked(){
                                        if !self.render_studio.timeline.is_selected(StudioItem::Camera(i)) {
                                            self.render_studio.timeline.select_item(StudioItem::Camera(i),false,false);
                                        }
                                        self.render_studio.timeline.delete_selection();
                                        self.profile_preview_key=None;ui.close();
                                    }
                                });
                                if self.render_studio.timeline.camera.len()!=count_before_menu {break;}
                                r.on_hover_text(format!("Camera #{} \u{00b7} {:.3}s \u{00b7} drag to retime \u{00b7} Del removes",i+1,time));
                            }
                        }
                        // Motion-key track. Its diamonds are tied to an animation
                        // clip; times are clip-local so moving a clip moves keys.
                        let keys:Vec<_>=self.render_studio.timeline.clips.iter().enumerate()
                            .flat_map(|(i,c)|c.motion.iter().enumerate().filter_map(move |(k,key)| {
                                let local=key.time-(c.first_frame-c.source_first) as f64/c.source_fps;
                                (local>=-0.00001 && local<=c.duration()+0.00001).then_some((i,k,c.start+local))
                            })).collect();
                        for (number,(i,k,time)) in keys.into_iter().enumerate() {
                            let x=to_x(time);if x<x0-10.0||x>right+10.0 {continue;}
                            let lane=self.render_studio.timeline.clips[i].lane;
                            let y=draw.top()+ruler_h+(crate::app::render_studio_timeline::StudioTimeline::motion_row(lane) as f32+0.5)*lane_h;
                            let selected=self.render_studio.timeline.is_selected(StudioItem::Motion(i,k));
                            let tint=studio_layer_color(lane);
                            if selected {
                                painter.circle_filled(egui::pos2(x,y),13.0,tint.gamma_multiply(0.24));
                                painter.circle_stroke(egui::pos2(x,y),12.5,egui::Stroke::new(2.0,egui::Color32::WHITE));
                            }
                            painter.add(egui::Shape::convex_polygon(vec![egui::pos2(x,y-7.0),egui::pos2(x+7.0,y),egui::pos2(x,y+7.0),egui::pos2(x-7.0,y)],
                                if selected{tint}else{tint.gamma_multiply(0.72)},egui::Stroke::new(1.0,egui::Color32::WHITE)));
                            if !locked {
                                let r=ui.interact(egui::Rect::from_center_size(egui::pos2(x,y),egui::vec2(18.0,22.0)),
                                    ui.make_persistent_id(("studio_motion",i,k)),egui::Sense::click_and_drag());
                                if r.clicked(){self.render_studio.timeline.select_item(StudioItem::Motion(i,k),ctrl,shift);
                                    self.render_studio.selected_background=false;
                                    if !ctrl && !shift {self.render_studio.timeline.seek(time);}self.profile_preview_key=None;}
                                if r.drag_started(){self.render_studio.timeline.select_item(StudioItem::Motion(i,k),false,false);
                                    self.render_studio.timeline.drag=Some(StudioDrag{
                                    kind:StudioDragKind::Motion,index:i,sub_index:k,
                                    pointer_x:r.interact_pointer_pos().map_or(x,|p|p.x),initial_scroll:scroll,initial_time:time,
                                    initial_first:0,initial_end:0,initial_start:0.0});}
                                if let Some(d)=self.render_studio.timeline.drag.filter(|d|
                                    r.dragged()&&d.kind==StudioDragKind::Motion&&d.index==i&&d.sub_index==k) {
                                    if let Some(p)=r.interact_pointer_pos() {
                                        let c=&mut self.render_studio.timeline.clips[i];
                                        let source_offset=(c.first_frame-c.source_first) as f64/c.source_fps;
                                        let before=if k==0 {source_offset}else{c.motion[k-1].time+1.0/30.0};
                                        let after=if k+1>=c.motion.len(){source_offset+c.duration()}else{c.motion[k+1].time-1.0/30.0};
                                        c.motion[k].time=(d.initial_time+f64::from((p.x-d.pointer_x)/zoom)-c.start+source_offset)
                                            .clamp(before,after.max(before));
                                        self.render_studio.timeline.selected_motion=Some((i,k));
                                        self.profile_preview_key=None;
                                    }
                                }
                                if r.drag_stopped(){self.render_studio.timeline.drag=None;}
                                let mut deleted=false;
                                r.context_menu(|ui| {
                                    if ui.button("Copy movement key").clicked(){
                                        if !self.render_studio.timeline.is_selected(StudioItem::Motion(i,k)) {
                                            self.render_studio.timeline.select_item(StudioItem::Motion(i,k),false,false);
                                        }
                                        self.render_studio.copy_selection();ui.close();
                                    }
                                    if ui.button("Delete movement key").clicked(){
                                        if !self.render_studio.timeline.is_selected(StudioItem::Motion(i,k)) {
                                            self.render_studio.timeline.select_item(StudioItem::Motion(i,k),false,false);
                                        }
                                        self.render_studio.timeline.delete_selection();
                                        deleted=true;
                                        self.profile_preview_key=None;ui.close();
                                    }
                                });
                                if deleted {break;}
                                r.on_hover_text(format!("Motion #{} \u{00b7} {:.3}s \u{00b7} drag to retime",number+1,time));
                            }
                        }
                        if let Some(d)=self.render_studio.timeline.drag {
                            if matches!(d.kind,StudioDragKind::TrimLeft|StudioDragKind::TrimRight) {
                                if let Some(c)=self.render_studio.timeline.clips.get(d.index) {
                                    if let Some(p)=ui.ctx().input(|i|i.pointer.interact_pos()) {
                                        let (frame,t)=if d.kind==StudioDragKind::TrimLeft {
                                            (c.first_frame,c.start)
                                        }else{(c.end_frame,c.end())};
                                        let popup=egui::Rect::from_min_size(
                                            egui::pos2((p.x+14.0).min(right-120.0),
                                                (p.y-27.0).max(draw.top())),egui::vec2(116.0,22.0));
                                        painter.rect_filled(popup,4.0,egui::Color32::from_rgb(8,12,19));
                                        painter.text(popup.center(),egui::Align2::CENTER_CENTER,
                                            format!("Frame {frame} · {t:.3}s"),egui::FontId::proportional(11.0),egui::Color32::WHITE);
                                    }
                                }
                            }
                        }
                        let head_x=to_x(self.render_studio.timeline_time());
                        if head_x>=x0&&head_x<=right {
                            painter.line_segment([egui::pos2(head_x,draw.top()),egui::pos2(head_x,bottom)],
                                egui::Stroke::new(2.0,egui::Color32::from_rgb(251,117,112)));
                            painter.add(egui::Shape::convex_polygon(vec![egui::pos2(head_x-6.0,draw.top()),
                                egui::pos2(head_x+6.0,draw.top()),egui::pos2(head_x,draw.top()+9.0)],
                                egui::Color32::from_rgb(251,117,112),egui::Stroke::NONE));
                        }
                        // Continue active pointer capture even if its original
                        // hit rectangle has moved fully outside the viewport.
                        if let Some(d)=self.render_studio.timeline.drag {
                            let (down,pos)=ui.ctx().input(|i|(i.pointer.primary_down(),i.pointer.interact_pos()));
                            if !down {self.render_studio.timeline.drag=None;}
                            else if let Some(pos)=pos {
                                let pan=if pos.x>right-17.0 {0.1}
                                    else if pos.x<x0+17.0 {-0.1}else{0.0};
                                if pan!=0.0 {
                                    self.render_studio.timeline.scroll=(self.render_studio.timeline.scroll+pan).max(0.0);
                                    ui.ctx().request_repaint();
                                }
                                let current_scroll=self.render_studio.timeline.scroll;
                                let shift=f64::from((pos.x-d.pointer_x)/zoom + current_scroll-d.initial_scroll);
                                let desired=d.initial_time+shift;
                                match d.kind {
                                    StudioDragKind::Clip => {
                                        let row=((pos.y-draw.top()-ruler_h)/lane_h).floor() as isize;
                                        let target=if row>=2 && (row as usize)<rows {
                                            crate::app::render_studio_timeline::StudioTimeline::lane_for_row(row as usize)
                                        } else {None};
                                        let target=target.filter(|&lane|lane<self.render_studio.timeline.lanes)
                                            .unwrap_or(self.render_studio.timeline.clips[d.index].lane);
                                        self.render_studio.timeline.move_clip_to_lane(d.index,target,desired);
                                        self.render_studio.timeline.selected_clip=Some(d.index);
                                        self.profile_preview_key=None;
                                    }
                                    StudioDragKind::TrimLeft|StudioDragKind::TrimRight => {
                                        self.render_studio.timeline.trim_clip(d.index,d.kind,d.initial_start,
                                            d.initial_first,d.initial_end,shift);
                                        self.profile_preview_key=None;
                                    }
                                    StudioDragKind::Camera => {
                                        self.render_studio.timeline.move_camera_key(d.index,desired);
                                        if let Some(key)=self.render_studio.timeline.camera.get(d.index){
                                            let time=key.time;self.render_studio.timeline.seek(time);
                                        }
                                        self.profile_preview_key=None;
                                    }
                                    StudioDragKind::Motion => {
                                        if let Some(c)=self.render_studio.timeline.clips.get_mut(d.index){
                                            if d.sub_index<c.motion.len(){
                                                let offset=(c.first_frame-c.source_first) as f64/c.source_fps;
                                                let k=d.sub_index;
                                                let before=if k==0 {offset}else{c.motion[k-1].time+1.0/30.0};
                                                let after=if k+1>=c.motion.len(){offset+c.duration()}else{c.motion[k+1].time-1.0/30.0};
                                                c.motion[k].time=(desired-c.start+offset).clamp(before,after.max(before));
                                                self.profile_preview_key=None;
                                            }
                                        }
                                    }
                                    StudioDragKind::In=>self.render_studio.timeline.set_in(desired),
                                    StudioDragKind::Out=>self.render_studio.timeline.set_out(desired),
                                }
                            }
                        }
                        // Library -> character track drop. Use actual pointer coordinates,
                        // not current selection or playhead, as the destination.
                        if !locked && ui.ctx().input(|i|i.pointer.any_released()) {
                            if let Some(asset)=self.render_studio.media_drag.take() {
                                if let Some(p)=ui.ctx().input(|i|i.pointer.latest_pos()) {
                                    let row=((p.y-draw.top()-ruler_h)/lane_h).floor() as isize;
                                    if p.x>=x0 && p.x<=right && row>=2 && (row as usize)<rows {
                                        let lane=crate::app::render_studio_timeline::StudioTimeline::lane_for_row(row as usize).unwrap_or(0);
                                        self.render_studio_drop_media(asset,from_x(p.x),lane);
                                    } else {self.render_studio.status="Drop an animation onto a character or motion row".into();}
                                }
                            }
                        }
                    });
                    let visible=((w-132.0)/self.render_studio.timeline.zoom).max(0.1);
                    let max_scroll=(self.render_studio.timeline.duration() as f32-visible).max(0.0)+3.0;
                    ui.add(egui::Slider::new(&mut self.render_studio.timeline.scroll,0.0..=max_scroll)
                        .show_value(false).text("Pan timeline"));
                });
        });
    }

    fn egui_render_studio_inspector(&mut self, ui:&mut egui::Ui) {
        use crate::app::render_studio_timeline::StudioInterpolation;
        if let Some(preview)=self.render_studio.cinematic_preview.as_ref() {
            ui.strong("CINEMATIC SOURCE");ui.label(&preview.clip.label);
            ui.small(&preview.clip.source.gla);ui.label(&preview.clip.source.animation);
            ui.label(format!("{} frames · {:.2} FPS",preview.clip.frame_count,preview.clip.fps));
            ui.label(format!("Frames {}..{} (end exclusive)",preview.clip.first_frame,preview.clip.end_frame()));
            ui.label(format!("Source loopFrames: {}",preview.clip.loop_frames));
            ui.small("Place this source on a character lane to trim it, extend/repeat it, or add motion and camera keys.");
            return;
        }
        let locked=self.render_studio.export.is_some()||self.render_studio.still.is_some();
        ui.add_enabled_ui(!locked,|ui| {
            let now=self.render_studio.timeline_time();
            if self.render_studio.selected_background {
                ui.label(egui::RichText::new("SCENE / BACKGROUND").color(theme::ACCENT).strong());
                let mut background=self.render_studio.background;
                egui::ComboBox::from_id_salt("studio_background_inspector")
                    .selected_text(match background {StudioBackground::Black=>"Empty / transparent",
                        StudioBackground::Green=>"Green screen",StudioBackground::Blue=>"Blue screen",
                        StudioBackground::Levelshot=>"Map levelshot (2D)"})
                    .show_ui(ui,|ui| {
                        ui.selectable_value(&mut background,StudioBackground::Black,"Empty / transparent");
                        ui.selectable_value(&mut background,StudioBackground::Green,"Green screen");
                        ui.selectable_value(&mut background,StudioBackground::Blue,"Blue screen");
                        ui.selectable_value(&mut background,StudioBackground::Levelshot,"Map levelshot (2D)");
                    });
                if background!=self.render_studio.background {self.render_studio.background=background;
                    self.render_studio_update_backdrop();}
                if background==StudioBackground::Levelshot {
                    let maps=self.profile_background_maps.clone().unwrap_or_default();
                    let mut chosen=self.render_studio.background_map.clone();
                    egui::ComboBox::from_id_salt("studio_background_map_inspector")
                        .selected_text(if chosen.is_empty(){"Select levelshot…".to_owned()}else{chosen.clone()})
                        .show_ui(ui,|ui|{for map in maps {ui.selectable_value(&mut chosen,map.clone(),map);}});
                    if chosen!=self.render_studio.background_map {
                        self.render_studio.background_map=chosen;self.render_studio_update_backdrop();
                    }
                }
                ui.small("Live 3D BSP staging is not connected to this isolated character renderer yet.");
                return;
            }
            ui.label(egui::RichText::new("INTERPOLATION").color(theme::ACCENT).strong());
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.render_studio.timeline.interpolation,StudioInterpolation::Linear,"Linear");
                ui.selectable_value(&mut self.render_studio.timeline.interpolation,StudioInterpolation::Smooth,"Smooth ease");
                ui.selectable_value(&mut self.render_studio.timeline.interpolation,StudioInterpolation::Spline,"Spline");
            });
            ui.small("Linear: constant velocity. Smooth: ease in/out. Spline: curved camera path with eased actor motion.");
            ui.separator();
            self.egui_studio_camera_rig(ui);
            ui.separator();
            let selected=self.render_studio.timeline.selected_items();
            if selected.len()>1 {
                ui.strong(format!("{} ITEMS SELECTED",selected.len()));
                ui.small("Ctrl+click toggles items. Shift+click extends a time range.");
                if ui.button("Copy selection  (Ctrl+C)").clicked(){self.render_studio.copy_selection();}
                if ui.button("Delete selection  (Del)").clicked(){
                    self.render_studio.timeline.delete_selection();self.profile_preview_key=None;
                }
                return;
            }
            if let Some(i)=self.render_studio.timeline.selected_camera {
                if i<self.render_studio.timeline.camera.len() {
                    ui.label(egui::RichText::new(format!("CAMERA KEY {}",i+1)).color(theme::ACCENT).strong());
                    let following=self.render_studio.timeline.camera_rig.position.enabled;
                    let tracking=self.render_studio.timeline.camera_rig.target.enabled;
                    let reference=self.render_studio.timeline.camera_rig.reference_position;
                    let key=&mut self.render_studio.timeline.camera[i];
                    ui.label(format!("At {:.3}s",key.time));
                    ui.small("Select the preview to fly with WASD and E/C. Hold RMB to aim; wheel changes speed. Add a camera key to save the current view.");
                    let mut changed=false;
                    if following {ui.small(if key.rig_orbit.is_some() {"Additional world movement; keyed orbit controls are above:"}else{"Keyed movement relative to the camera attachment:"});}
                    for (n,label) in ["X","Y","Z"].iter().enumerate() {
                        let mut value=key.position[n]-if following {reference[n]}else{0.};
                        ui.horizontal(|ui| {ui.label(*label);if ui.add(egui::DragValue::new(&mut value).speed(1.0)).changed() {
                            key.position[n]=value+if following {reference[n]}else{0.};changed=true;
                        }});
                    }
                    ui.add_enabled_ui(!tracking,|ui| {
                        changed|=ui.add(egui::Slider::new(&mut key.yaw,-360.0..=360.0).text("Yaw °")).changed();
                        changed|=ui.add(egui::Slider::new(&mut key.pitch,-89.0..=89.0).text("Pitch °")).changed();
                    });
                    if tracking {ui.small("Aim is controlled by Track target above.");}
                    changed|=ui.add(egui::Slider::new(&mut key.fov,10.0..=140.0).text("FOV °")).changed();
                    if changed {self.profile_preview_key=None;}
                    if ui.button("Delete camera key (Del)").clicked(){
                        self.render_studio.timeline.camera.remove(i);
                        self.render_studio.timeline.selected_camera=None;self.profile_preview_key=None;
                    }
                    return;
                }
            }
            if let Some((i,k))=self.render_studio.timeline.selected_motion {
                let number=crate::app::render_studio_spatial::visible_motion_keys(&self.render_studio.timeline)
                    .iter().position(|&(clip,key,_)|clip==i && key==k).map_or(k+1,|n|n+1);
                if let Some(clip)=self.render_studio.timeline.clips.get_mut(i) {
                    if k<clip.motion.len() {
                        ui.label(egui::RichText::new(format!("CHARACTER MOTION KEY #{number}")).color(theme::ACCENT).strong());
                        ui.label(format!("{} · {:.3}s",clip.label(),clip.start+clip.motion[k].time-
                            (clip.first_frame-clip.source_first) as f64/clip.source_fps));
                        let key=&mut clip.motion[k];let mut changed=false;
                        for (n,label) in ["Offset X","Offset Y","Offset Z"].iter().enumerate(){
                            ui.horizontal(|ui|{ui.label(*label);changed|=ui.add(egui::DragValue::new(&mut key.position[n]).speed(1.0)).changed();});
                        }
                        changed|=ui.add(egui::Slider::new(&mut key.yaw,-360.0..=360.0).text("Actor yaw °")).changed();
                        if changed{self.profile_preview_key=None;}
                        if ui.button("Delete motion key").clicked(){
                            clip.motion.remove(k);self.render_studio.timeline.selected_motion=None;
                            self.profile_preview_key=None;
                        }
                        return;
                    }
                }
            }
            if let Some(i)=self.render_studio.timeline.selected_clip {
                if i<self.render_studio.timeline.clips.len() {
                    let (label,first,end,fps,clip_start,clip_end,source_end,tail,mut lane)= {
                        let clip=&self.render_studio.timeline.clips[i];
                        (clip.label().to_owned(),clip.first_frame,clip.end_frame,clip.source_fps,
                            clip.start,clip.end(),clip.source_end,clip.tail_frames,clip.lane)
                    };
                    ui.label(egui::RichText::new("ANIMATION CLIP").color(theme::ACCENT).strong());
                    ui.strong(label);
                    ui.small(format!("Frames [{first}..{end}) · {fps:.2} fps"));
                    if let Some(actor_model)=self.render_studio.timeline.lane_models.get_mut(lane) {
                        ui.label("Actor model for this layer (blank = global model)");
                        ui.add(egui::TextEdit::singleline(actor_model).hint_text("e.g. kyle/default"));
                    }
                    if ui.checkbox(&mut self.render_studio.with_sabers,"Attach selected sabers").changed(){
                        self.profile_preview_key=None;
                    }
                    ui.label(format!("Timeline {clip_start:.3}s — {clip_end:.3}s"));
                    ui.small(format!("Source ends at frame {source_end} · {tail} extended frames"));
                    let mut extra=tail;
                    let next=self.render_studio.timeline.clips.iter().enumerate()
                        .filter(|(j,c)|*j!=i && c.lane==lane && c.start>=clip_end-0.00001)
                        .map(|(_,c)|c.start).fold(f64::INFINITY,f64::min);
                    let max_extra=if next.is_finite() {
                        ((next-clip_start)*fps).floor().max(0.0) as i32-(end-first)
                    } else {360000};
                    if ui.add(egui::DragValue::new(&mut extra).range(0..=max_extra.max(0).min(360000))
                        .speed(1.0).prefix("Extend (frames) ")).changed() {
                        self.render_studio.timeline.clips[i].tail_frames=extra;
                        self.profile_preview_key=None;
                    }
                    ui.horizontal(|ui| {
                        ui.label("After source:");
                        ui.selectable_value(&mut self.render_studio.timeline.clips[i].playback,
                            crate::app::render_studio_timeline::StudioClipPlayback::Hold,"Hold last frame");
                        ui.selectable_value(&mut self.render_studio.timeline.clips[i].playback,
                            crate::app::render_studio_timeline::StudioClipPlayback::Loop,"Repeat");
                    });
                    let mut start=clip_start;
                    if ui.add(egui::DragValue::new(&mut start).speed(0.033).prefix("Start ").suffix(" s")).changed(){
                        self.render_studio.timeline.move_clip(i,start);self.profile_preview_key=None;
                    }
                    egui::ComboBox::from_id_salt("studio_inspector_layer").selected_text(format!("Layer {}",lane+1))
                        .show_ui(ui,|ui|{for layer in 0..self.render_studio.timeline.lanes{ui.selectable_value(&mut lane,layer,format!("Layer {}",layer+1));}});
                    if lane!=self.render_studio.timeline.clips[i].lane {
                        self.render_studio.timeline.clips[i].lane=lane;
                        let t=self.render_studio.timeline.clips[i].start;
                        self.render_studio.timeline.move_clip(i,t);
                    }
                    let (clip_start,clip_end)={let c=&self.render_studio.timeline.clips[i];(c.start,c.end())};
                    if ui.button("Add movement key at playhead").clicked(){
                        let smooth=self.render_studio.timeline.interpolation!=StudioInterpolation::Linear;
                        let clip=&mut self.render_studio.timeline.clips[i];
                        let mut key=clip.motion_at(now,smooth);
                        key.time=(clip.first_frame-clip.source_first) as f64/clip.source_fps+(now-clip.start).clamp(0.0,clip.duration());
                        let k=clip.add_motion_key(key);
                        self.render_studio.timeline.selected_motion=Some((i,k));
                    }
                    if ui.button("Split at playhead  (K)").clicked(){self.render_studio.timeline.split_clip(i,now);self.profile_preview_key=None;}
                    if ui.button("Delete clip").clicked(){
                        self.render_studio.timeline.clips.remove(i);self.render_studio.timeline.selected_clip=None;
                        self.render_studio.timeline.selected_motion=None;self.profile_preview_key=None;
                    }
                    ui.small(format!("Selected clip range {clip_start:.2}–{clip_end:.2}s. Drag its left/right edges to trim non-destructively."));
                    return;
                }
            }
            ui.label(egui::RichText::new("PLAYHEAD").color(theme::ACCENT).strong());
            ui.label(format!("{now:.3} seconds"));
            if ui.button("Camera key at playhead").clicked(){
                self.capture_studio_preview_camera(now);
            }
            ui.separator();
            ui.small("Ctrl+click toggles items. Shift+click selects everything between the anchor and clicked time. Copy, paste and Delete apply to the selection. K splits at the playhead.");
            ui.small("Each animation lane represents a separate character. Overlapping lanes are rendered simultaneously.");
        });
    }
}

impl App {
    /// Project names are sanitized by project_file_name; browsing never permits
    /// paths outside the Render Studio write directory.
    fn egui_render_studio_project_dialogs(&mut self, ctx:&egui::Context) {
        if self.render_studio.show_save_as {
            let mut open=true;
            let mut save=false;
            egui::Window::new("Save Render Studio Project As…")
                .open(&mut open).collapsible(false).resizable(false).show(ctx,|ui| {
                    ui.label("Project name");
                    ui.text_edit_singleline(&mut self.render_studio.project_name);
                    let path=self.game_write_path("render-studio").join(self.render_studio.project_file_name());
                    ui.small(format!("Save to {}",path.display()));
                    if path.exists(){ui.colored_label(egui::Color32::YELLOW,"Existing project: saving will overwrite it.");}
                    if ui.button("Save project").clicked(){save=true;}
                });
            if save {self.save_render_studio_project();open=false;}
            self.render_studio.show_save_as=open;
        }
        if self.render_studio.show_open_project {
            let mut open=true;
            let mut chosen=None::<String>;
            let dir=self.game_write_path("render-studio");
            let mut entries:Vec<_>=std::fs::read_dir(&dir).ok().into_iter().flatten()
                .flatten().filter_map(|e|{
                    let path=e.path();
                    if path.is_file() && path.extension().and_then(|x|x.to_str()).is_some_and(|x|x.eq_ignore_ascii_case("json")) {
                        path.file_stem().and_then(|x|x.to_str()).map(str::to_owned)
                    } else {None}
                }).collect();
            entries.sort();
            egui::Window::new("Open Render Studio Project")
                .open(&mut open).collapsible(false).default_width(420.0).show(ctx,|ui| {
                    ui.small(format!("Folder: {}",dir.display()));
                    if entries.is_empty(){ui.label("No saved JSON projects found.");}
                    egui::ScrollArea::vertical().max_height(330.0).show(ui,|ui| {
                        for entry in entries {
                            if ui.selectable_label(self.render_studio.project_name==entry,format!("{entry}.json")).clicked(){
                                chosen=Some(entry);
                            }
                        }
                    });
                });
            if let Some(name)=chosen {
                self.render_studio.project_name=name;
                self.load_render_studio_project();open=false;
            }
            self.render_studio.show_open_project=open;
        }
    }
}
