//! Audio-specific Render Studio widgets. Kept separate from the common clip
//! editor, so animation/demo editing can evolve independently.
use super::App;
use super::render_studio_ui::palette as theme;
use super::render_studio_ui::{self as sui, ButtonKind, Icon};
use crate::app::render_studio_audio::{StudioAudioFade, StudioAudioTrack};

impl App {
    pub(in crate::app::egui_menu) fn egui_studio_audio_menu(
        &mut self,
        ui: &mut egui::Ui,
        locked: bool,
    ) {
        let button = sui::button(ui, Some(Icon::Audio), "Audio", ButtonKind::Normal, !locked)
            .on_hover_text("Add an audio track or import a sound file");
        egui::Popup::menu(&button).style(sui::menu_style())
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                sui::editor_style(ui);
                ui.set_width(300.0);
                if sui::button_row(ui, &[(Some(Icon::Plus), "New audio track", ButtonKind::Normal, true)]).is_some() {
                    let n = self.render_studio.timeline.audio_tracks.len();
                    if n < 32 {
                        self.render_studio
                            .timeline
                            .audio_tracks
                            .push(StudioAudioTrack::new(n + 1));
                        self.render_studio.timeline.audio_expanded.push(false);
                    }
                    ui.close();
                }
                sui::divider(ui);
                sui::group_label(ui, "Import WAV / MP3 / FLAC / OGG");
                ui.checkbox(&mut self.render_studio.audio.link_external, "Link to original file (do not copy)")
                    .on_hover_text("Off (default): copy into render-studio/<project>/media/audio. On: reference the original location.");
                if sui::button_row(ui, &[(Some(Icon::Open), "Browse and import at playhead…", ButtonKind::Primary, true)]).is_some() {
                    self.render_studio.file_picker.open(crate::app::render_studio_picker::StudioPickerKind::Audio);
                    ui.close();
                }
                sui::divider(ui);
                sui::hint(ui,"Or paste an absolute path:");
                ui.add(egui::TextEdit::singleline(&mut self.render_studio.audio.import_path)
                    .desired_width(ui.available_width()).hint_text("D:\\Music\\song.mp3"));
                if sui::button_row(ui,&[(None,"Import path",ButtonKind::Normal,true)]).is_some() {
                    let path=self.render_studio.audio.import_path.trim().trim_matches('"').to_owned();
                    self.studio_queue_audio_files(vec![std::path::PathBuf::from(path)]);
                    ui.close();
                }
            });
    }
    /// Audio track header cell: expand chevron, name (click selects the
    /// track) and Mute / Solo toggles at the right edge of the header column.
    pub(in crate::app::egui_menu) fn egui_studio_audio_track_header(
        &mut self,
        ui: &mut egui::Ui,
        draw: egui::Rect,
        y: f32,
        h: f32,
        track: usize,
        locked: bool,
    ) {
        let Some(row) = self.render_studio.timeline.audio_tracks.get(track) else {
            return;
        };
        let (mute, solo) = (row.mute, row.solo);
        let x = draw.left();
        let header_right = x + sui::TRACK_HEADER_W;
        let arrow = egui::Rect::from_center_size(egui::pos2(x + 18.0, y + h * 0.5), egui::vec2(22.0, h));
        let open = self
            .render_studio
            .timeline
            .audio_expanded
            .get(track)
            .copied()
            .unwrap_or(false);
        let hovered = !locked && ui.rect_contains_pointer(arrow);
        sui::paint_icon(
            ui.painter(),
            arrow.center(),
            if open { Icon::ChevronDown } else { Icon::ChevronRight },
            if hovered { theme::TEXT } else { theme::TEXT_FAINT },
        );
        if !locked
            && ui
                .interact(
                    arrow,
                    ui.make_persistent_id(("audio_expand", track)),
                    egui::Sense::click(),
                )
                .on_hover_text(if open { "Hide volume automation" } else { "Show volume automation" })
                .clicked()
        {
            self.render_studio
                .timeline
                .audio_expanded
                .resize(self.render_studio.timeline.audio_tracks.len(), false);
            self.render_studio.timeline.audio_expanded[track] = !open;
        }
        let label_area = egui::Rect::from_min_max(egui::pos2(x + 30.0, y), egui::pos2(header_right - 54.0, y + h));
        let label = ui.interact(
            label_area,
            ui.make_persistent_id(("audio_track_label", track)),
            egui::Sense::click(),
        );
        if label.clicked() {
            self.render_studio.audio.selected_track = Some(track);
            self.render_studio.audio.selected = None;
            self.render_studio.timeline.clear_selection();
            self.render_studio.timeline.selected_demo = None;
            self.render_studio.selected_background = false;
        }
        label.on_hover_text("Select the track to rename, reorder and set its volume in the Inspector");
        for (letter, at, value, color) in [("M", 50.0, mute, theme::WARNING), ("S", 26.0, solo, sui::AUDIO)] {
            let r = egui::Rect::from_center_size(egui::pos2(header_right - at, y + h * 0.5), egui::vec2(20.0, 18.0));
            let hovered = !locked && ui.rect_contains_pointer(r);
            ui.painter().rect_filled(
                r,
                3.0,
                if value {
                    color.gamma_multiply(0.85)
                } else if hovered {
                    theme::CONTROL_HOVER
                } else {
                    theme::CONTROL
                },
            );
            ui.painter().text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                letter,
                egui::FontId::proportional(10.5),
                if value { sui::WELL } else { theme::TEXT_DIM },
            );
            if !locked {
                let response = ui
                    .interact(
                        r,
                        ui.make_persistent_id(("audio_track_toggle", track, letter)),
                        egui::Sense::click(),
                    )
                    .on_hover_text(if letter == "M" { "Mute track" } else { "Solo track" });
                if response.clicked() {
                    if letter == "M" {
                        self.render_studio.timeline.audio_tracks[track].mute = !mute;
                    } else {
                        self.render_studio.timeline.audio_tracks[track].solo = !solo;
                    }
                }
            }
        }
    }
    pub(in crate::app::egui_menu) fn egui_studio_audio_clips(
        &mut self,
        ui: &mut egui::Ui,
        draw: egui::Rect,
        ruler_h: f32,
        lane_h: f32,
        x0: f32,
        right: f32,
        zoom: f32,
        scroll: f32,
        locked: bool,
    ) {
        let to_x = |t: f64| x0 + (t as f32 - scroll) * zoom;
        let mut delete = None;
        let mut split = None;
        let mut key_delete = None;
        for track in 0..self.render_studio.timeline.audio_tracks.len() {
            let row = self.render_studio.timeline.audio_row(track);
            let y = draw.top() + ruler_h + row as f32 * lane_h;
            self.egui_studio_audio_track_header(ui, draw, y, lane_h, track, locked);
            let entries: Vec<_> = self.render_studio.timeline.audio_tracks[track]
                .clips
                .iter()
                .enumerate()
                .map(|(i, c)| (i, c.clone()))
                .collect();
            for (i, c) in entries {
                let raw_left = to_x(c.start);
                let raw_right = to_x(c.end());
                let x1 = raw_left.max(x0);
                let x2 = raw_right.min(right);
                if x2 <= x1 {
                    continue;
                }
                let r = egui::Rect::from_min_max(
                    egui::pos2(x1, y + 4.0),
                    egui::pos2(x2, y + lane_h - 4.0),
                );
                let selected = self.render_studio.audio.selected == Some((track, i));
                let tint = sui::clip_body(sui::AUDIO, selected);
                let painter = ui.painter().with_clip_rect(draw);
                painter.rect_filled(r, 3.0, tint);
                painter.rect_stroke(
                    r,
                    3.0,
                    egui::Stroke::new(
                        if selected { 2.0 } else { 0.6 },
                        if selected {
                            egui::Color32::WHITE
                        } else {
                            theme::LINE
                        },
                    ),
                    egui::StrokeKind::Inside,
                );
                let short = std::path::Path::new(&c.path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Audio");
                if r.width() > 55.0 {
                    painter.with_clip_rect(r.shrink(2.0)).text(
                        r.min + egui::vec2(5.0, 2.0),
                        egui::Align2::LEFT_TOP,
                        short,
                        egui::FontId::proportional(10.0),
                        egui::Color32::WHITE,
                    );
                }
                // Source-anchored waveform: trims change the displayed source
                // portion, never rescale the underlying audio waveform.
                if let Some(source) = self.render_studio.audio.cache.get(&c.path) {
                    let wave_y = r.center().y + 4.0;
                    let max_pixels = r.width().ceil().min(1500.0) as usize;
                    let step = (r.width() / max_pixels.max(1) as f32).max(1.0);
                    let mut wx = r.left();
                    for _ in 0..max_pixels {
                        if wx > r.right() {
                            break;
                        }
                        let at = c.source_in + f64::from((wx - raw_left) / zoom);
                        let w = ((at * source.rate as f64) / 512.0).max(0.0) as usize;
                        if let Some(&(lo, hi)) = source.waveform.get(w) {
                            let x = wx;
                            painter.line_segment(
                                [
                                    egui::pos2(x, wave_y - hi * 6.5),
                                    egui::pos2(x, wave_y - lo * 6.5),
                                ],
                                egui::Stroke::new(
                                    step.min(1.5),
                                    egui::Color32::from_rgb(188, 249, 218),
                                ),
                            );
                        }
                        wx += step;
                    }
                }
                // Fade handles indicate shape/length; actual gain is calculated
                // at output sample timestamps by StudioAudioClip::gain_at.
                for (is_in, length) in [(true, c.fade_in), (false, c.fade_out)] {
                    if length <= 0.0 {
                        continue;
                    }
                    let x = to_x(if is_in {
                        c.start + length
                    } else {
                        c.end() - length
                    })
                    .clamp(r.left(), r.right());
                    let a = egui::pos2(
                        if is_in { r.left() } else { x },
                        if is_in { r.bottom() } else { r.top() },
                    );
                    let b = egui::pos2(
                        if is_in { x } else { r.right() },
                        if is_in { r.top() } else { r.bottom() },
                    );
                    painter.line_segment(
                        [a, b],
                        egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 230, 133)),
                    );
                }
                if locked {
                    continue;
                }
                let body = ui.interact(
                    r,
                    ui.make_persistent_id(("studio_audio_clip", track, i)),
                    egui::Sense::click_and_drag(),
                );
                if body.clicked() {
                    self.render_studio.audio.selected = Some((track, i));
                    self.render_studio.audio.selected_track = Some(track);
                    self.render_studio.audio.selected_key = None;
                    self.render_studio.timeline.clear_selection();
                    self.render_studio.timeline.selected_demo = None;
                    self.render_studio.selected_background = false;
                }
                if body.drag_started() {
                    if let Some(p) = body.interact_pointer_pos() {
                        self.render_studio.audio.drag = Some((track, i, p.x, c.start));
                        self.render_studio.audio.selected = Some((track, i));
                        self.render_studio.audio.selected_track = Some(track);
                    }
                }
                if let Some((t, k, px, start)) = self.render_studio.audio.drag {
                    if t == track && k == i && body.dragged() {
                        if let Some(p) = body.interact_pointer_pos() {
                            self.render_studio.timeline.audio_tracks[track].clips[i].start = (start
                                + (p.x - px) as f64 / zoom as f64)
                                .clamp(0.0, 36000.0 - c.duration());
                        }
                    }
                }
                if body.drag_stopped() {
                    self.render_studio.audio.drag = None;
                }
                if body.secondary_clicked() {
                    self.render_studio.context_time = body.interact_pointer_pos()
                        .map(|p| f64::from((p.x - x0) / zoom + scroll));
                }
                body.context_menu(|ui| { sui::editor_style(ui);
                    if ui.button("Add volume key here").clicked() {
                        let at = self.render_studio.context_time.unwrap_or(self.render_studio.timeline_time());
                        self.studio_add_audio_volume_key_at(track, i, at);
                        ui.close();
                    }
                    if ui.button("Split at playhead").clicked() {
                        split = Some((track, i, self.render_studio.timeline_time()));
                        ui.close();
                    }
                    if ui.button("Delete audio clip").clicked() {
                        delete = Some((track, i));
                        ui.close();
                    }
                });
                // Visible fade handles at upper clip edges: drag horizontally.
                for is_in in [true, false] {
                    let x = to_x(if is_in {
                        c.start + c.fade_in
                    } else {
                        c.end() - c.fade_out
                    })
                    .clamp(r.left(), r.right());
                    let handle = egui::Rect::from_center_size(
                        egui::pos2(x, r.top() + 2.0),
                        egui::vec2(12.0, 14.0),
                    );
                    painter.circle_filled(
                        handle.center(),
                        3.0,
                        egui::Color32::from_rgb(251, 220, 143),
                    );
                    let grip = ui.interact(
                        handle,
                        ui.make_persistent_id(("studio_audio_fade", track, i, is_in)),
                        egui::Sense::click_and_drag(),
                    );
                    if grip.drag_started() {
                        if let Some(p) = grip.interact_pointer_pos() {
                            self.render_studio.audio.fade_drag = Some((
                                track,
                                i,
                                is_in,
                                p.x,
                                if is_in { c.fade_in } else { c.fade_out },
                            ));
                        }
                    }
                    if let Some((t, k, side, px, initial)) = self.render_studio.audio.fade_drag {
                        if (t, k, side) == (track, i, is_in) && grip.dragged() {
                            if let Some(p) = grip.interact_pointer_pos() {
                                let delta = (p.x - px) as f64 / zoom as f64
                                    * if is_in { 1.0 } else { -1.0 };
                                let target =
                                    &mut self.render_studio.timeline.audio_tracks[track].clips[i];
                                target.envelope = None;
                                if is_in {
                                    target.fade_in =
                                        (initial + delta).clamp(0.0, target.duration());
                                } else {
                                    target.fade_out =
                                        (initial + delta).clamp(0.0, target.duration());
                                }
                            }
                        }
                    }
                    if grip.drag_stopped() {
                        self.render_studio.audio.fade_drag = None;
                    }
                    grip.on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
                        .on_hover_text("Drag audio fade length");
                }
                // Independent trim handles at the middle of each edge, away
                // from the upper fade handles. Source samples remain anchored.
                for is_left in [true, false] {
                    let edge_x = if is_left { r.left() } else { r.right() };
                    let handle = egui::Rect::from_center_size(
                        egui::pos2(edge_x, r.center().y + 3.0),
                        egui::vec2(9.0, 13.0),
                    );
                    painter.rect_filled(
                        egui::Rect::from_center_size(handle.center(), egui::vec2(3.0, 10.0)),
                        1.0,
                        egui::Color32::WHITE,
                    );
                    let grip = ui.interact(
                        handle,
                        ui.make_persistent_id(("studio_audio_trim", track, i, is_left)),
                        egui::Sense::click_and_drag(),
                    );
                    if grip.drag_started() {
                        if let Some(p) = grip.interact_pointer_pos() {
                            self.render_studio.audio.trim_drag =
                                Some((track, i, is_left, p.x, c.clone()));
                            self.render_studio.timeline.clear_selection();
                            self.render_studio.selected_background = false;
                            self.render_studio.audio.selected = Some((track, i));
                            self.render_studio.audio.selected_track = Some(track);
                        }
                    }
                    if let Some((t, k, side, px, original)) =
                        self.render_studio.audio.trim_drag.as_ref()
                    {
                        if (*t, *k, *side) == (track, i, is_left) && grip.dragged() {
                            if let Some(p) = grip.interact_pointer_pos() {
                                let shift = (p.x - *px) as f64 / zoom as f64;
                                let maximum = self
                                    .render_studio
                                    .audio
                                    .cache
                                    .get(&c.path)
                                    .map_or(original.source_out, |a| a.duration());
                                let (start, end) = if is_left {
                                    (
                                        (original.source_in + shift).clamp(
                                            (original.source_in - original.start).max(0.0),
                                            original.source_out - 0.001,
                                        ),
                                        original.source_out,
                                    )
                                } else {
                                    (
                                        original.source_in,
                                        (original.source_out + shift).clamp(
                                            original.source_in + 0.001,
                                            maximum.max(original.source_in + 0.001),
                                        ),
                                    )
                                };
                                self.render_studio.timeline.audio_tracks[track].clips[i] =
                                    original.trimmed(start, end);
                            }
                        }
                    }
                    if grip.drag_stopped() {
                        self.render_studio.audio.trim_drag = None;
                    }
                    grip.on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
                        .on_hover_text("Drag to trim audio source (non-destructive)");
                }
                // Volume diamonds live on the expandable subtrack.
                if let Some(subrow) = self.render_studio.timeline.audio_volume_row(track) {
                    let sy = draw.top() + ruler_h + subrow as f32 * lane_h;
                    for (k, key) in c.keys.iter().enumerate() {
                        let kt = c.start + key.time;
                        if !(c.start..=c.end()).contains(&kt) {
                            continue;
                        }
                        let x = to_x(kt);
                        if x < x0 || x > right {
                            continue;
                        }
                        let y = sy + lane_h * (0.75 - 0.5 * (key.gain / 4.0));
                        let pos = egui::pos2(x, y);
                        painter.add(egui::Shape::convex_polygon(
                            vec![
                                pos + egui::vec2(0.0, -6.0),
                                pos + egui::vec2(6.0, 0.0),
                                pos + egui::vec2(0.0, 6.0),
                                pos + egui::vec2(-6.0, 0.0),
                            ],
                            if self.render_studio.audio.selected == Some((track, i))
                                && self.render_studio.audio.selected_key == Some(k)
                            {
                                egui::Color32::YELLOW
                            } else {
                                egui::Color32::from_rgb(145, 219, 168)
                            },
                            egui::Stroke::new(1.0, egui::Color32::WHITE),
                        ));
                        let handle = egui::Rect::from_center_size(pos, egui::vec2(16.0, 18.0));
                        let keyhit = ui.interact(
                            handle,
                            ui.make_persistent_id(("studio_audio_key", track, i, k)),
                            egui::Sense::click_and_drag(),
                        );
                        if keyhit.clicked() {
                            self.render_studio.timeline.clear_selection();
                            self.render_studio.selected_background = false;
                            self.render_studio.audio.selected = Some((track, i));
                            self.render_studio.audio.selected_key = Some(k);
                        }
                        if keyhit.drag_started() {
                            if let Some(p) = keyhit.interact_pointer_pos() {
                                self.render_studio.audio.key_drag =
                                    Some((track, i, k, p.x, p.y, key.time, key.gain));
                                self.render_studio.timeline.clear_selection();
                                self.render_studio.selected_background = false;
                                self.render_studio.audio.selected = Some((track, i));
                                self.render_studio.audio.selected_key = Some(k);
                            }
                        }
                        keyhit.context_menu(|ui| { sui::editor_style(ui);
                            if ui.button("Delete volume key").clicked() {
                                key_delete = Some((track, i, k));
                                ui.close();
                            }
                        });
                        keyhit.on_hover_text(format!(
                            "Volume {:.0}% at {:.3}s",
                            key.gain * 100.0,
                            kt
                        ));
                    }
                }
            }
        }
        if let Some((t, i, k)) = key_delete {
            if let Some(c) = self
                .render_studio
                .timeline
                .audio_tracks
                .get_mut(t)
                .and_then(|r| r.clips.get_mut(i))
            {
                if k < c.keys.len() {
                    c.keys.remove(k);
                    self.render_studio.audio.selected_key = None;
                }
            }
        }
        if let Some((t, i, k, px, py, time, gain)) = self.render_studio.audio.key_drag {
            let (down, p) = ui
                .ctx()
                .input(|input| (input.pointer.primary_down(), input.pointer.interact_pos()));
            if !down {
                self.render_studio.audio.key_drag = None;
            } else if let Some(p) = p {
                if let Some(c) = self
                    .render_studio
                    .timeline
                    .audio_tracks
                    .get_mut(t)
                    .and_then(|row| row.clips.get_mut(i))
                {
                    if k < c.keys.len() {
                        let lo = if k > 0 {
                            c.keys[k - 1].time + 0.001
                        } else {
                            0.0
                        };
                        let hi = if k + 1 < c.keys.len() {
                            c.keys[k + 1].time - 0.001
                        } else {
                            c.duration()
                        };
                        c.keys[k].time =
                            (time + (p.x - px) as f64 / zoom as f64).clamp(lo, hi.max(lo));
                        c.keys[k].gain = (gain + (py - p.y) / lane_h * 8.0).clamp(0.0, 4.0);
                    }
                }
            }
        }
        if let Some((t, i)) = delete {
            self.render_studio.timeline.audio_tracks[t].clips.remove(i);
            self.render_studio.audio.selected = None;
            self.render_studio.audio.selected_key = None;
        }
        if let Some((t, i, at)) = split {
            if let Some((left, right)) = self.render_studio.timeline.audio_tracks[t]
                .clips
                .get(i)
                .and_then(|clip| clip.split_at(at))
            {
                self.render_studio.timeline.audio_tracks[t].clips[i] = left;
                self.render_studio.timeline.audio_tracks[t]
                    .clips
                    .insert(i + 1, right);
                self.render_studio.audio.selected = Some((t, i + 1));
            }
        }
    }
    pub(in crate::app::egui_menu) fn egui_studio_audio_inspector(
        &mut self,
        ui: &mut egui::Ui,
    ) -> bool {
        let Some((t, i)) = self.render_studio.audio.selected else {
            let Some(t) = self.render_studio.audio.selected_track else {
                return false;
            };
            let count = self.render_studio.timeline.audio_tracks.len();
            if t >= count {
                return false;
            }
            let clips = self.render_studio.timeline.audio_tracks[t].clips.len();
            let name = self.render_studio.timeline.audio_tracks[t].name.clone();
            sui::inspector_title(ui, sui::AUDIO, "Audio track", &name, &format!("{clips} clip(s) · track {}", t + 1));
            let track = &mut self.render_studio.timeline.audio_tracks[t];
            sui::section(ui, "studio_insp_audio_track", "Track", None, true, |ui| {
                sui::prop_text(ui, "Name", &mut track.name, "Audio track");
                sui::prop_slider(ui, "Gain", egui::Slider::new(&mut track.gain, 0.0..=4.0).suffix("×"));
                sui::prop_toggle(ui, "Mute", &mut track.mute);
                sui::prop_toggle(ui, "Solo", &mut track.solo);
            });
            self.egui_studio_audio_track_order(ui, t, None);
            return true;
        };
        let Some(track) = self.render_studio.timeline.audio_tracks.get(t) else {
            return false;
        };
        let Some(source_clip) = track.clips.get(i) else {
            return false;
        };
        let path = source_clip.path.clone();
        let now = self.render_studio.timeline_time();
        let track_count = self.render_studio.timeline.audio_tracks.len();
        let source_duration = self
            .render_studio
            .audio
            .cache
            .get(&path)
            .map_or(36000.0, |x| x.duration());
        let file = std::path::Path::new(&path)
            .file_name()
            .and_then(|x| x.to_str())
            .unwrap_or("Audio")
            .to_owned();
        let fps = self.render_studio.fps;
        sui::inspector_title(ui, sui::AUDIO, "Audio clip", &file,
            &format!("{} – {}", sui::timecode(source_clip.start, fps), sui::timecode(source_clip.end(), fps)));
        let selected_key = self.render_studio.audio.selected_key;
        let mut new_key = None;
        let mut delete_key = false;
        let mut remove_clip = false;
        let row = &mut self.render_studio.timeline.audio_tracks[t];
        sui::section(ui, "studio_insp_audio_clip", "Clip",
            Some("Waveform, fades and automation are source aligned. Clips overlap and mix at 48 kHz."), true, |ui| {
            let clip = &mut row.clips[i];
            let duration = clip.duration();
            sui::prop_drag(ui, "Start", egui::DragValue::new(&mut clip.start)
                .speed(0.03)
                .range(0.0..=(36000.0 - duration).max(0.0))
                .suffix(" s"));
            let original = clip.clone();
            let mut source_in = clip.source_in;
            let mut source_out = clip.source_out;
            sui::prop_drag(ui, "Trim IN", egui::DragValue::new(&mut source_in)
                .speed(0.01)
                .range((clip.source_in - clip.start).max(0.0)..=(clip.source_out - 0.001).max(0.0))
                .suffix(" s"));
            sui::prop_drag(ui, "Trim OUT", egui::DragValue::new(&mut source_out)
                .speed(0.01)
                .range((source_in + 0.001)..=source_duration.max(source_in + 0.001))
                .suffix(" s"));
            if source_in != clip.source_in || source_out != clip.source_out {
                *clip = original.trimmed(source_in, source_out);
            }
        });
        sui::section(ui, "studio_insp_audio_mix", "Gain & fades", None, true, |ui| {
            let clip = &mut row.clips[i];
            let old_fades = (clip.fade_in, clip.fade_out, clip.fade_curve);
            let duration = clip.duration();
            sui::prop_slider(ui, "Clip gain", egui::Slider::new(&mut clip.gain, 0.0..=4.0).suffix("×"));
            sui::prop_slider(ui, "Fade in", egui::Slider::new(&mut clip.fade_in, 0.0..=duration).suffix(" s"));
            sui::prop_slider(ui, "Fade out", egui::Slider::new(&mut clip.fade_out, 0.0..=duration).suffix(" s"));
            sui::prop_segmented(ui, "Fade curve", &mut clip.fade_curve,
                &[(StudioAudioFade::Linear, "Linear"), (StudioAudioFade::EqualPower, "Equal power")]);
            if old_fades != (clip.fade_in, clip.fade_out, clip.fade_curve) {
                clip.envelope = None;
            }
            ui.add_space(4.0);
            sui::group_label(ui, "Track");
            sui::prop_text(ui, "Name", &mut row.name, "Audio track");
            sui::prop_slider(ui, "Track gain", egui::Slider::new(&mut row.gain, 0.0..=4.0).suffix("×"));
            sui::prop_toggle(ui, "Mute", &mut row.mute);
            sui::prop_toggle(ui, "Solo", &mut row.solo);
        });
        sui::section(ui, "studio_insp_audio_volume", "Volume automation",
            Some("Volume keys are diamonds in the track's expanded Volume lane; drag them to retime / adjust."), true, |ui| {
            let clip = &mut row.clips[i];
            let duration = clip.duration();
            if let Some(k) = selected_key {
                if let Some(key) = clip.keys.get_mut(k) {
                    sui::prop_drag(ui, &format!("Key {} gain", k + 1),
                        egui::DragValue::new(&mut key.gain).speed(0.01).range(0.0..=4.0).suffix("×"));
                }
            } else {
                sui::hint(ui, &format!("{} volume key(s). Select one in the Volume lane to edit it.", clip.keys.len()));
            }
            match sui::button_row(ui, &[(Some(Icon::AddKey), "Key at playhead", ButtonKind::Normal, true),
                (None, "Delete key", ButtonKind::Normal, selected_key.is_some())]) {
                Some(0) => {
                    let local = (now - clip.start).clamp(0.0, duration);
                    let sample = (clip.start + local).min(clip.end() - 0.00001);
                    let gain = clip.automation_at((sample - clip.start).max(0.0));
                    new_key = Some(clip.add_key(local, gain));
                }
                Some(1) => delete_key = true,
                _ => {}
            }
        });
        if let Some(k) = new_key {
            self.render_studio.audio.selected_key = Some(k);
            self.render_studio.timeline.audio_expanded.resize(track_count, false);
            self.render_studio.timeline.audio_expanded[t] = true;
        }
        if delete_key {
            if let Some(k) = self.render_studio.audio.selected_key.take() {
                let clip = &mut self.render_studio.timeline.audio_tracks[t].clips[i];
                if k < clip.keys.len() {
                    clip.keys.remove(k);
                }
            }
        }
        if sui::button_row(ui, &[(None, "Remove audio clip", ButtonKind::Danger, true)]).is_some() {
            remove_clip = true;
        }
        if remove_clip {
            self.render_studio.timeline.audio_tracks[t].clips.remove(i);
            self.render_studio.audio.selected = None;
            return true;
        }
        self.egui_studio_audio_track_order(ui, t, Some(i));
        true
    }

    /// Reorder / delete actions shared by the track and clip inspectors.
    fn egui_studio_audio_track_order(&mut self, ui: &mut egui::Ui, t: usize, clip: Option<usize>) {
        let count = self.render_studio.timeline.audio_tracks.len();
        ui.add_space(6.0);
        match sui::button_row(ui, &[(None, "Move track up", ButtonKind::Normal, t > 0),
            (None, "Move track down", ButtonKind::Normal, t + 1 < count)]) {
            Some(index) => {
                let other = if index == 0 { t - 1 } else { t + 1 };
                self.render_studio.timeline.audio_tracks.swap(t, other);
                if self.render_studio.timeline.audio_expanded.len() == count {
                    self.render_studio.timeline.audio_expanded.swap(t, other);
                }
                self.render_studio.audio.selected_track = Some(other);
                if let Some(i) = clip {
                    self.render_studio.audio.selected = Some((other, i));
                }
            }
            None => {}
        }
        if sui::button_row(ui, &[(None, "Delete track and its clips", ButtonKind::Danger, true)]).is_some() {
            self.render_studio.timeline.audio_tracks.remove(t);
            if t < self.render_studio.timeline.audio_expanded.len() {
                self.render_studio.timeline.audio_expanded.remove(t);
            }
            self.render_studio.audio.selected = None;
            self.render_studio.audio.selected_track = None;
        }
    }
}

impl App {
    /// Linked demo audio is backed by the native SoundPresenter, not a file
    /// decoder. Source-time keys are evaluated with the demo's Speed remap.
    pub(in crate::app::egui_menu) fn egui_studio_demo_audio_inspector(
        &mut self,ui:&mut egui::Ui,index:usize,playhead:f64,
    ) {
        let Some(clip)=self.render_studio.timeline.demos.get_mut(index) else {return;};
        sui::section(ui,("studio_insp_demo_audio",index),"Linked audio",
            Some("Native game SFX, voices, ambient loops and map music are reconstructed from DM26 events, so there is no PCM waveform. Moving, trimming or retiming the demo keeps its sound attached. Drag the gold handles in the green lane to fade."),
            true,|ui| {
            sui::prop_toggle(ui,"Mute",&mut clip.audio.mute);
            sui::prop_toggle(ui,"Solo",&mut clip.audio.solo);
            sui::prop_slider(ui,"Volume",egui::Slider::new(&mut clip.audio.gain,0.0..=4.0).suffix("×"));
            let duration=clip.duration();
            sui::prop_slider(ui,"Fade in",egui::Slider::new(&mut clip.audio.fade_in,0.0..=duration).suffix(" s"));
            sui::prop_slider(ui,"Fade out",egui::Slider::new(&mut clip.audio.fade_out,0.0..=duration).suffix(" s"));
            sui::prop_segmented(ui,"Fade curve",&mut clip.audio.fade_curve,
                &[(StudioAudioFade::Linear,"Linear"),(StudioAudioFade::EqualPower,"Equal power")]);
            let source=f64::from(clip.source_time_ms(playhead))/1000.0;
            let active=playhead>=clip.start && playhead<clip.end();
            sui::prop_value(ui,"At playhead",&format!("source {:.3}s · gain {:.0}%",source,clip.audio_gain_at(playhead)*100.0));
            let mut to_remove=None;
            for (k,key) in clip.audio.keys.iter_mut().enumerate() {
                ui.push_id(k,|ui| {
                    sui::prop_row(ui,&format!("Key {} · {:.2}s",k+1,key.time),|ui| {
                        ui.spacing_mut().item_spacing.x=4.0;
                        ui.add_sized(egui::vec2((ui.available_width()-30.0).max(40.0),20.0),
                            egui::DragValue::new(&mut key.gain).speed(0.01).range(0.0..=4.0).suffix("×"));
                        if sui::icon_button(ui,Icon::Close,true,false,"Delete volume key").clicked(){to_remove=Some(k);}
                    });
                });
            }
            if let Some(k)=to_remove {clip.audio.keys.remove(k);}
            if sui::button_row(ui,&[(Some(Icon::AddKey),"Volume key at playhead",ButtonKind::Normal,
                active && clip.audio.keys.len()<512)]).is_some() {
                clip.audio.add_key(source,1.0);
            }
        });
    }
}
