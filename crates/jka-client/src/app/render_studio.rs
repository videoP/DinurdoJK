//! Offline character capture stage. Uses the same PlayerPresenter, emote poses,
//! GPU capture readback and encoding profiles as the rest of the client.
//! The stage is intentionally isolated: nothing here spawns network entities.
use crate::app::{App, FrontendPage, Instant, RenderCommand};
use crate::app::demos::video_export::{adjacent_ffmpeg_path, DemoVideoPreset};
use std::{io::Write, path::PathBuf, process::{Child, ChildStdin, Command, Stdio}};
use image::ImageEncoder;
use super::render_studio_camera::{StudioCameraRig, StudioCameraRigRuntime};
use super::render_studio_timeline::{StudioAnimationClip, StudioTimeline, StudioProject, StudioProjectClip, StudioClipPlayback, StudioMotionKey, StudioCameraKey, StudioInterpolation, StudioItem};

/// Render Studio-only formats. The demo exporter deliberately retains its
/// existing preset enum/codec behavior; these additions apply only here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum StudioVideoFormat {
    H264High,
    H264Max,
    ProRes422,
    ProRes444,
    WebmVp9,
    WebmVp9Max,
    Gif,
}

impl StudioVideoFormat {
    pub const OPTIONS: [Self; 7] = [
        Self::H264High, Self::H264Max, Self::ProRes422,
        Self::ProRes444, Self::WebmVp9, Self::WebmVp9Max, Self::Gif,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::H264High => "H264 HIGH",
            Self::H264Max => "H264 MAX",
            Self::ProRes422 => "PRORES 422",
            Self::ProRes444 => "PRORES 4444",
            Self::WebmVp9 => "WEBM VP9 HIGH",
            Self::WebmVp9Max => "WEBM VP9 MAX",
            Self::Gif => "ANIMATED GIF",
        }
    }

    pub fn short_description(self) -> &'static str {
        match self {
            Self::H264High => DemoVideoPreset::H264High.short_description(),
            Self::H264Max => DemoVideoPreset::H264Max.short_description(),
            Self::ProRes422 => DemoVideoPreset::ProRes422.short_description(),
            Self::ProRes444 => DemoVideoPreset::ProRes444.short_description(),
            Self::WebmVp9 => "VP9 CRF 28; compact transparent WebM (optional alpha)",
            Self::WebmVp9Max => "VP9 CRF 18; larger high-quality transparent WebM (optional alpha)",
            Self::Gif => "Looping 256-color GIF; optional 1-bit alpha, best at 10-30 fps",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::H264High | Self::H264Max => "mp4",
            Self::ProRes422 | Self::ProRes444 => "mov",
            Self::WebmVp9 | Self::WebmVp9Max => "webm",
            Self::Gif => "gif",
        }
    }

    pub fn supports_alpha(self) -> bool {
        matches!(self, Self::ProRes444 | Self::WebmVp9 | Self::WebmVp9Max | Self::Gif)
    }

    fn demo_preset(self) -> Option<DemoVideoPreset> {
        match self {
            Self::H264High => Some(DemoVideoPreset::H264High),
            Self::H264Max => Some(DemoVideoPreset::H264Max),
            Self::ProRes422 => Some(DemoVideoPreset::ProRes422),
            Self::ProRes444 => Some(DemoVideoPreset::ProRes444),
            Self::WebmVp9 | Self::WebmVp9Max | Self::Gif => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum StudioBackground {
    Black,
    Green,
    Blue,
    Levelshot,
}

pub(in crate::app) struct RenderStudioExport {
    pub frame_index: u64,
    pub total_frames: u64,
    pub fps: u32,
    pub awaiting_frame: bool,
    pub capture_queued: bool,
    output: PathBuf,
    alpha: bool,
    depth: bool,
    depth_dir: Option<PathBuf>,
    depth_child: Option<Child>,
    depth_stdin: Option<ChildStdin>,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    preset: StudioVideoFormat,
}

impl Drop for RenderStudioExport {
    fn drop(&mut self) {
        self.stdin.take();
        self.depth_stdin.take();
        if let Some(mut child) = self.depth_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// A one-shot still shares the video readback queue, but does not start an
/// encoder or advance the model timeline.
pub(in crate::app) struct RenderStudioStill {
    pub awaiting_frame: bool,
    output: PathBuf,
    alpha: bool,
    depth: bool,
}


/// Editor history uses immutable scene snapshots. A held pointer is one
/// transaction, so a 500-frame drag consumes one undo step, not 500.
#[derive(Clone)]
pub(in crate::app) struct StudioEditSnapshot {
    clips: Vec<StudioAnimationClip>, camera: Vec<StudioCameraKey>, lanes: usize,
    camera_rig: StudioCameraRig,
    lane_models: Vec<String>, in_point: f64, out_point: Option<f64>,
    interpolation: StudioInterpolation, model: String, with_sabers: bool,
    background: StudioBackground, background_map: String,
}
impl StudioEditSnapshot {
    pub(super) fn camera_pose_signature(&self) -> String {
        // Bone samples survive camera-key, offset and damping edits. Only
        // changes to the actual actor pose or attachment invalidate them.
        format!("{:?}{:?}{:?}{:?}{:?}{:?}{:?}",
            self.clips.iter().map(|c|(&c.name,c.emote.as_ref().map(|e|&e.id),c.start,
                c.first_frame,c.end_frame,c.source_fps,c.lane,c.source_first,c.source_end,
                c.tail_frames,c.playback,c.motion.iter().map(|m|(m.time,m.position,m.yaw)).collect::<Vec<_>>()))
                .collect::<Vec<_>>(),self.model,self.lane_models,self.with_sabers,self.interpolation,
            (self.camera_rig.position.lane,self.camera_rig.position.part,&self.camera_rig.position.bone),
            (self.camera_rig.target.lane,self.camera_rig.target.part,&self.camera_rig.target.bone))
    }
    pub(super) fn signature(&self) -> String {
        format!("{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}",
            self.clips.iter().map(|c|(&c.name,c.emote.as_ref().map(|e| &e.id),c.start,
                c.first_frame,c.end_frame,c.source_fps,c.lane,c.source_first,c.source_end,
                c.tail_frames,c.playback,c.motion.iter().map(|m|(m.time,m.position,m.yaw)).collect::<Vec<_>>()))
                .collect::<Vec<_>>(),
            self.camera.iter().map(|k|(k.time,k.position,k.yaw,k.pitch,k.fov,k.rig_orbit)).collect::<Vec<_>>(),
            self.lanes,self.lane_models,self.in_point,self.out_point,self.interpolation,
            self.model,self.with_sabers,self.background as u8,self.background_map,self.camera_rig)
    }
}
#[derive(Default)]
pub(in crate::app) struct StudioEditHistory {
    undo: Vec<StudioEditSnapshot>, redo: Vec<StudioEditSnapshot>,
    baseline: Option<StudioEditSnapshot>, pending: Option<StudioEditSnapshot>,
}
impl StudioEditHistory {
    fn observe(&mut self, current: StudioEditSnapshot, pointer_down: bool) {
        let Some(baseline)=self.baseline.as_ref() else {self.baseline=Some(current);return};
        if pointer_down {
            if self.pending.is_none(){self.pending=Some(baseline.clone());}
            return;
        }
        let before=self.pending.take().unwrap_or_else(||baseline.clone());
        if before.signature()!=current.signature() {
            self.undo.push(before);if self.undo.len()>256 {self.undo.remove(0);}
            self.redo.clear();
        }
        self.baseline=Some(current);
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(in crate::app) enum StudioMediaTab { #[default] Jka, Fortnite, Cutscenes }
#[derive(Clone)]
pub(in crate::app) struct StudioMediaDrag {pub name: String, pub emote_id: Option<String>,
    pub cinematic:Option<super::render_studio_cinematics::StudioCinematicSource>}

#[derive(Clone)]
pub(in crate::app) enum StudioClipboard {
    Clip(StudioAnimationClip), Camera(StudioCameraKey), Motion(StudioMotionKey),
    Group(StudioClipboardGroup),
}
#[derive(Clone)]
pub(in crate::app) struct StudioClipboardGroup {
    origin: f64,
    clips: Vec<StudioAnimationClip>,
    camera: Vec<StudioCameraKey>,
    /// Source lane, absolute timeline time, and the copied movement value.
    motion: Vec<(usize,f64,StudioMotionKey)>,
}
pub(in crate::app) struct RenderStudio {
    pub model: String,
    pub model_search: String,
    pub animation_search: String,
    pub emote_search: String,
    pub cinematics: super::render_studio_cinematics::StudioCinematicLibrary,
    pub cinematic_preview: Option<super::render_studio_cinematics::StudioCinematicPreview>,
    pub stock_animation: String,
    pub emote: Option<crate::emotes::Emote>,
    pub trim_start: i32,
    /// Exclusive end frame in the *source* clip.
    pub trim_stop: i32,
    pub clip_meta: Option<(i32, i32, f32)>,
    pub playhead: f64,
    pub playing: bool,
    pub play_started: Instant,
    pub with_sabers: bool,
    pub background: StudioBackground,
    pub background_map: String,
    pub preset: StudioVideoFormat,
    pub fps: u32,
    pub transparent: bool,
    pub export_depth: bool,
    pub screenshot_alpha: bool,
    pub screenshot_depth: bool,
    pub still: Option<RenderStudioStill>,
    pub export: Option<RenderStudioExport>,
    pub status: String,
    pub timeline: StudioTimeline,
    /// Auto-create a first preview clip once, never re-create it after delete.
    pub initial_clip_seeded: bool,
    pub project_name: String,
    pub show_save_as: bool, pub show_open_project: bool,
    pub show_render: bool, pub model_picker_lane: Option<usize>,
    pub navigation: super::render_studio_navigation::StudioPreviewNavigation,
    pub camera_rig_runtime: StudioCameraRigRuntime,
    pub camera_orbit_edit_runtime: StudioCameraRigRuntime,
    pub camera_pending_match: Option<[f32;3]>,
    /// One shared sample time for all actors, attachment queries and FX in a frame.
    pub preview_sample_time: Option<f64>,
    pub camera_bone_search: [String;2],
    pub spatial: super::render_studio_spatial::StudioSpatialEditor,
    pub media_tab: StudioMediaTab, pub media_drag: Option<StudioMediaDrag>,
    pub selected_background: bool,
    /// Empty-track context menu keeps the right-click location across frames.
    pub context_time: Option<f64>, pub context_lane: Option<usize>,
    pub history: StudioEditHistory, pub clipboard: Option<StudioClipboard>,
}

impl Default for RenderStudio {
    fn default() -> Self {
        Self {
            model: String::new(), model_search: String::new(),
            animation_search: String::new(), emote_search: String::new(),
            cinematics: Default::default(),cinematic_preview:None,
            stock_animation: "BOTH_STAND1".into(), emote: None,
            trim_start: 0, trim_stop: 0, clip_meta: None,
            playhead: 0.0, playing: false, play_started: Instant::now(),
            with_sabers: false, background: StudioBackground::Black,
            background_map: String::new(),
            preset: StudioVideoFormat::H264High, fps: 60,
            transparent: false, export_depth: false,
            screenshot_alpha: true, screenshot_depth: false,
            still: None, export: None, status: String::new(),
            timeline: StudioTimeline::default(), initial_clip_seeded: false,
            project_name: "timeline_project".into(),
            show_save_as: false, show_open_project: false,
            show_render: false, model_picker_lane: None,
            navigation: Default::default(), camera_rig_runtime: Default::default(),camera_orbit_edit_runtime:Default::default(),
            camera_pending_match: None,
            preview_sample_time: None,
            camera_bone_search: Default::default(),
            spatial: Default::default(),
            media_tab: StudioMediaTab::Jka, media_drag: None, selected_background: false,
            context_time: None, context_lane: None,
            history: StudioEditHistory::default(), clipboard: None,
        }
    }
}

impl RenderStudio {
    pub fn actor_center(&self, zoom: f32) -> [f32;3] {
        let time=self.timeline_time();
        let lane=self.timeline.selected_clip.and_then(|i|self.timeline.clips.get(i))
            .map(|clip|clip.lane).or_else(||self.timeline.active_actors(time).first().map(|clip|clip.lane))
            .unwrap_or(0);
        let motion=self.timeline.clip_at_lane(time,lane)
            .map(|clip|clip.motion_at(time,self.timeline.interpolation != StudioInterpolation::Linear))
            .unwrap_or_default();
        [88.0*zoom.clamp(0.45,2.5)+motion.position[0],18.0+motion.position[1],
            -22.0+lane as f32*26.0+motion.position[2]]
    }
    /// Safe basename only: never let a project field select a parent directory.
    pub fn project_file_name(&self) -> String {
        let stem: String = self.project_name.chars().take(80)
            .filter(|c|c.is_ascii_alphanumeric() || *c=='_' || *c=='-').collect();
        format!("{}.json", if stem.is_empty(){"timeline_project"}else{&stem})
    }
    pub fn select_clip(&mut self, name: Option<&str>, emote: Option<crate::emotes::Emote>) {
        if let Some(name) = name { self.stock_animation = name.to_owned(); }
        self.emote = emote;
        self.clip_meta = None;
        self.playing = false;
        self.playhead = 0.0;
    }

    pub fn set_clip_meta(&mut self, meta: Option<(i32, i32, f32)>) {
        let Some((first, end, fps)) = meta else { return; };
        if first >= end || !fps.is_finite() || fps <= 0.0 { return; }
        if self.clip_meta != Some((first, end, fps)) {
            self.clip_meta = Some((first, end, fps));
            self.trim_start = first;
            self.trim_stop = end;
            self.playhead = 0.0;
            self.playing = false;
            // Seed the non-linear animation track once the first clip has
            // resolved its actual frame range. Subsequent source selections
            // are staged in the library until explicitly added to the track.
            if !self.initial_clip_seeded {
                self.initial_clip_seeded=true;
                self.timeline.add_clip(StudioAnimationClip {
                    name: self.stock_animation.clone(), emote: self.emote.clone(),
                    start: 0.0, first_frame: first, end_frame: end,
                    source_fps: f64::from(fps), lane: 0, source_first: first, source_end: end,
                    motion: vec![Default::default()],tail_frames:0,playback:StudioClipPlayback::Hold,
                });
            }
        }
    }

    pub fn add_current_clip_to_timeline(&mut self) -> Result<(), &'static str> {
        let (first, end) = self.clip_range().ok_or("Animation metadata not ready")?;
        let time = self.timeline.current_time(None);
        self.timeline.add_clip(StudioAnimationClip {
            name: self.stock_animation.clone(), emote: self.emote.clone(), start: time,
            first_frame: first, end_frame: end, source_fps: self.clip_fps(),
            lane: self.timeline.selected_clip.and_then(|i|self.timeline.clips.get(i).map(|c|c.lane)).unwrap_or(0),
            source_first: self.clip_meta.map_or(first,|m|m.0), source_end: self.clip_meta.map_or(end,|m|m.1),
            motion: vec![Default::default()],tail_frames:0,playback:StudioClipPlayback::Hold,
        });
        Ok(())
    }

    pub fn timeline_time(&self) -> f64 {
        self.preview_sample_time.unwrap_or_else(||
            self.timeline.current_time(self.export.as_ref().map(|e| (e.frame_index, e.fps))))
    }

    pub fn active_animation(&self) -> (String, Option<crate::emotes::Preview>, Option<(i32,i32)>, i32) {
        if let Some(preview)=self.cinematic_preview.as_ref() {
            let ms=(preview.elapsed()*1000.).round() as i32;
            let mut emote=preview.emote.clone();emote.looping=false;emote.loop_start_frame=None;
            return (preview.clip.source.animation.clone(),Some(crate::emotes::Preview {emote,elapsed_ms:ms}),
                Some((preview.clip.first_frame,preview.clip.end_frame())),ms);
        }
        let time = self.timeline_time();
        if let Some(clip) = self.timeline.clip_at_lane(time,0) {
            return (clip.name.clone(), clip.preview(time), Some((clip.first_frame, clip.end_frame)), clip.pose_time_ms(time));
        }
        ("BOTH_STAND1".into(), None, None, 0)
    }

    pub fn clip_range(&self) -> Option<(i32, i32)> {
        self.clip_meta.map(|(lo, hi, _)| (
            self.trim_start.clamp(lo, hi - 1),
            self.trim_stop.clamp(self.trim_start.clamp(lo, hi - 1) + 1, hi),
        ))
    }

    pub fn clip_fps(&self) -> f64 {
        self.clip_meta.map_or(20.0, |(_, _, fps)| fps as f64)
    }

    pub fn duration_frames(&self) -> f64 {
        self.clip_range().map_or(1.0, |(first, end)| (end - first) as f64).max(1.0)
    }

    pub fn offset_frames(&self) -> f64 {
        if let Some(export) = &self.export {
            (export.frame_index as f64 * self.clip_fps() / export.fps as f64)
                .min((self.duration_frames() - f64::EPSILON).max(0.0))
        } else if self.playing {
            (self.playhead + self.play_started.elapsed().as_secs_f64() * self.clip_fps())
                % self.duration_frames()
        } else {
            self.playhead.clamp(0.0, self.duration_frames() - f64::EPSILON)
        }
    }

    pub fn presentation_time_ms(&self) -> i32 {
        let offset = self.offset_frames();
        let relative = if let Some(emote) = &self.emote {
            (self.trim_start - emote.first_frame).max(0) as f64 + offset
        } else {
            offset
        };
        (relative * 1000.0 / self.clip_fps()).round().clamp(0.0, i32::MAX as f64) as i32
    }

    pub fn emote_preview(&self) -> Option<crate::emotes::Preview> {
        self.emote.clone().map(|emote| crate::emotes::Preview {
            emote,
            elapsed_ms: self.presentation_time_ms(),
        })
    }
}

impl RenderStudio {
    pub fn actor_model(&self, lane: usize) -> &str {
        self.timeline.lane_models.get(lane).filter(|model|!model.is_empty())
            .map_or(self.model.as_str(),String::as_str)
    }
    pub fn edit_snapshot(&self) -> StudioEditSnapshot {
        StudioEditSnapshot {clips:self.timeline.clips.clone(),camera:self.timeline.camera.clone(),
            lanes:self.timeline.lanes,lane_models:self.timeline.lane_models.clone(),
            camera_rig:self.timeline.camera_rig.clone(),
            in_point:self.timeline.in_point,out_point:self.timeline.out_point,
            interpolation:self.timeline.interpolation,model:self.model.clone(),
            with_sabers:self.with_sabers,background:self.background,background_map:self.background_map.clone()}
    }
    fn apply_snapshot(&mut self,s:StudioEditSnapshot) {
        self.cinematic_preview=None;
        self.timeline.clips=s.clips;self.timeline.camera=s.camera;self.timeline.lanes=s.lanes;
        self.timeline.lane_models=s.lane_models;
        self.timeline.camera_rig=s.camera_rig;self.camera_rig_runtime=Default::default();self.camera_pending_match=None;
        self.spatial.drag=None;
        self.timeline.in_point=s.in_point;self.timeline.out_point=s.out_point;
        self.timeline.interpolation=s.interpolation;
        self.model=s.model;self.with_sabers=s.with_sabers;
        self.background=s.background;self.background_map=s.background_map;
        self.timeline.clear_selection();
        self.timeline.drag=None;self.timeline.playing=false;
    }
    pub fn init_history(&mut self) {
        if self.history.baseline.is_none() {self.history.baseline=Some(self.edit_snapshot());}
    }
    pub fn track_edit(&mut self,pointer_down:bool) {
        let s=self.edit_snapshot();self.history.observe(s,pointer_down);
    }
    pub fn undo_edit(&mut self) -> bool {
        let Some(previous)=self.history.undo.pop() else{return false};
        let current=self.edit_snapshot();self.history.redo.push(current);
        self.apply_snapshot(previous);
        self.history.baseline=Some(self.edit_snapshot());self.history.pending=None;
        true
    }
    pub fn redo_edit(&mut self) -> bool {
        let Some(next)=self.history.redo.pop() else{return false};
        let current=self.edit_snapshot();self.history.undo.push(current);
        self.apply_snapshot(next);
        self.history.baseline=Some(self.edit_snapshot());self.history.pending=None;
        true
    }
    /// Consume semantic clipboard commands in order, including when Ctrl was
    /// released before this UI frame. Text fields retain their normal clipboard.
    pub fn handle_clipboard_input(&mut self, ctx: &egui::Context) -> bool {
        if ctx.text_edit_focused() { return false; }
        let commands=ctx.input_mut(|input| {
            let mut commands=Vec::new();
            input.events.retain(|event| {
                let copy=match event {
                    egui::Event::Copy=>true,
                    egui::Event::Paste(_)=>false,
                    _=>return true,
                };
                commands.push(copy);
                false
            });
            commands
        });
        let mut pasted=false;
        for copy in commands {
            if copy {
                self.status=if self.copy_selection() {"Selection copied"} else {"Select a timeline clip or key to copy"}.into();
            } else if self.paste_selection() {
                self.status="Selection pasted".into();
                pasted=true;
            } else {
                self.status="Clipboard empty or no destination clip selected for motion key".into();
            }
        }
        pasted
    }
    pub fn copy_selection(&mut self) -> bool {
        let items=self.timeline.selected_items();
        if items.len()>1 {
            let origin=items.iter().filter_map(|&item|self.timeline.item_time(item)).fold(f64::INFINITY,f64::min);
            let mut group=StudioClipboardGroup {origin,clips:Vec::new(),camera:Vec::new(),motion:Vec::new()};
            for item in &items {
                match *item {
                    StudioItem::Clip(i)=>group.clips.push(self.timeline.clips[i].clone()),
                    StudioItem::Camera(i)=>group.camera.push(self.timeline.camera[i]),
                    StudioItem::Motion(i,k)=>{
                        // A copied clip already contains its movement keys.
                        if !items.contains(&StudioItem::Clip(i)) {
                            group.motion.push((self.timeline.clips[i].lane,self.timeline.item_time(*item).unwrap(),self.timeline.clips[i].motion[k]));
                        }
                    }
                }
            }
            self.clipboard=Some(StudioClipboard::Group(group));return true;
        }
        let value=if let Some((i,k))=self.timeline.selected_motion {
            self.timeline.clips.get(i).and_then(|c|c.motion.get(k)).copied().map(StudioClipboard::Motion)
        } else if let Some(i)=self.timeline.selected_camera {
            self.timeline.camera.get(i).copied().map(StudioClipboard::Camera)
        } else {self.timeline.selected_clip.and_then(|i|self.timeline.clips.get(i)).cloned().map(StudioClipboard::Clip)};
        if value.is_some(){self.clipboard=value;true}else{false}
    }
    pub fn paste_selection(&mut self) -> bool {
        self.paste_selection_on_lane(None)
    }
    pub fn paste_selection_on_lane(&mut self, target_lane: Option<usize>) -> bool {
        let t=self.timeline.current_time(None);
        match self.clipboard.clone() {
            Some(StudioClipboard::Clip(mut c))=>{
                c.start=t;
                if let Some(lane)=target_lane.filter(|&lane|lane<self.timeline.lanes) {c.lane=lane;}
                self.timeline.add_clip(c);true
            }
            Some(StudioClipboard::Camera(mut k))=>{
                k.time=t;self.timeline.add_camera_key(k);true
            }
            Some(StudioClipboard::Motion(mut k))=>{
                let Some(i)=self.timeline.selected_clip else{return false};
                let Some(c)=self.timeline.clips.get_mut(i) else{return false};
                k.time=(c.first_frame-c.source_first) as f64/c.source_fps+(t-c.start).clamp(0.0,c.duration());
                let j=c.add_motion_key(k);self.timeline.select_item(StudioItem::Motion(i,j),false,false);true
            }
            Some(StudioClipboard::Group(group))=>self.paste_group(group,target_lane),
            None=>false,
        }
    }
    fn paste_group(&mut self, group:StudioClipboardGroup, target_lane:Option<usize>) -> bool {
        let min_lane=group.clips.iter().map(|clip|clip.lane).chain(group.motion.iter().map(|key|key.0)).min().unwrap_or(0);
        let max_lane=group.clips.iter().map(|clip|clip.lane).chain(group.motion.iter().map(|key|key.0)).max().unwrap_or(0);
        let lane_delta=target_lane.filter(|&lane|lane+max_lane-min_lane<self.timeline.lanes)
            .map_or(0,|lane|lane as isize-min_lane as isize);
        let mut offset=self.timeline.snap_time(self.timeline.current_time(None)).max(0.0)-group.origin;
        // Find one common offset, so collisions never collapse group spacing.
        loop {
            let mut advance=0.0f64;
            for clip in &group.clips {
                let lane=(clip.lane as isize+lane_delta) as usize;
                let start=clip.start+offset;
                for existing in self.timeline.clips.iter().filter(|existing|existing.lane==lane) {
                    if start<existing.end()-1e-8 && start+clip.duration()>existing.start+1e-8 {
                        advance=advance.max(existing.end()-start);
                    }
                }
            }
            if advance<=1e-8 {break;}
            offset+=advance;
        }
        let copies:Vec<_>=group.clips.iter().cloned().map(|mut clip| {
            clip.start+=offset;clip.lane=(clip.lane as isize+lane_delta) as usize;clip
        }).collect();
        let mut motions=Vec::new();
        // Validate every destination before changing any timeline contents.
        for (lane,time,mut key) in group.motion {
            let lane=(lane as isize+lane_delta) as usize;
            let time=time+offset;
            let Some((i,clip))=self.timeline.clips.iter().chain(copies.iter()).enumerate()
                .find(|(_,clip)|clip.lane==lane && time>=clip.start-1e-8 && time<=clip.end()+1e-8) else {return false};
            key.time=(clip.first_frame-clip.source_first) as f64/clip.source_fps.max(1.0)+(time-clip.start).clamp(0.0,clip.duration());
            motions.push((i,key));
        }
        let mut selected=Vec::new();
        for clip in copies {
            selected.push(StudioItem::Clip(self.timeline.clips.len()));self.timeline.clips.push(clip);
        }
        let mut camera_times=Vec::new();
        for mut key in group.camera {
            key.time+=offset;camera_times.push(key.time);self.timeline.insert_camera_key(key);
        }
        for time in camera_times {
            if let Some(i)=self.timeline.camera.iter().position(|key|(key.time-time).abs()<1e-5) {selected.push(StudioItem::Camera(i));}
        }
        for &(i,key) in &motions {self.timeline.clips[i].add_motion_key(key);}
        for (i,key) in motions {
            if let Some(k)=self.timeline.clips[i].motion.iter().position(|other|(other.time-key.time).abs()<1e-5) {selected.push(StudioItem::Motion(i,k));}
        }
        self.timeline.set_selection(selected);true
    }
}

impl App {
    /// Reframe the currently selected camera key (or author one at the
    /// playhead) toward the active actor. Explicit action: dropping media or
    /// scrubbing must never silently overwrite authored cinematic cameras.
    pub(in crate::app) fn render_studio_frame_actor(&mut self) {
        let time=self.render_studio.timeline_time();
        let lane=self.render_studio.timeline.selected_clip
            .and_then(|i|self.render_studio.timeline.clips.get(i))
            .map(|c|c.lane)
            .or_else(||self.render_studio.timeline.active_actors(time).first().map(|c|c.lane))
            .unwrap_or(0);
        // Camera coordinates are X-forward, Y-up, Z-sideways. Secondary
        // actors are spaced laterally on Z, never stacked vertically on Y.
        let target=self.render_studio.actor_center(self.profile_preview_zoom);
        let eye=[0.0,-11.0,0.0];
        let dx=target[0]-eye[0];
        let dy=target[1]-eye[1];
        let dz=target[2]-eye[2];
        let yaw=(-dz).atan2(dx).to_degrees().rem_euclid(360.0);
        let pitch=dy.atan2((dx*dx+dz*dz).sqrt()).to_degrees();
        if self.render_studio.timeline.camera_rig.active() {
            self.update_studio_camera_rig();
            let visible=self.studio_shot_camera();
            self.render_studio.timeline.begin_camera_edit();
            if let Some(key)=self.render_studio.timeline.selected_camera
                .and_then(|i|self.render_studio.timeline.camera.get_mut(i)) {
                for i in 0..3 {key.position[i]+=eye[i]-visible.position[i];}
                key.yaw=yaw;key.pitch=pitch;
            }
        } else if let Some(key)=self.render_studio.timeline.selected_camera
            .and_then(|i|self.render_studio.timeline.camera.get_mut(i)) {
            key.position=eye;key.yaw=yaw;key.pitch=pitch;
        } else {
            let mut key=self.render_studio.timeline.camera_at(time);
            key.time=time;key.position=eye;key.yaw=yaw;key.pitch=pitch;
            self.render_studio.timeline.add_camera_key(key);
        }
        self.render_studio.timeline.seek(time);
        self.profile_preview_key=None;
        self.render_studio.status=format!("Framed character {} in selected camera key",lane+1);
    }

    /// Resolve source metadata only on an actual drop, not for every tile in the bin.
    /// The library is a source catalogue; the timeline owns placed animation clips.
    pub(in crate::app) fn render_studio_drop_media(&mut self,
        asset: StudioMediaDrag, time: f64, lane: usize) {
        if lane>=self.render_studio.timeline.lanes {return;}
        self.render_studio.cinematic_preview=None;
        let emote = if let Some(source)=asset.cinematic.as_ref() {
            match self.resolve_studio_cinematic(source).and_then(|clip|clip.emote()) {
                Ok(emote)=>Some(emote),Err(error)=>{self.render_studio.status=error;return;}
            }
        } else if let Some(id)=asset.emote_id.as_deref() {
            match crate::emotes::catalog(&self.base) {
                Ok(catalog) => {
                    let found=catalog.iter().flat_map(|root|
                        std::iter::once(root).chain(root.variants.iter()))
                        .find(|candidate| candidate.id==id).cloned();
                    match found {
                        Some(variant) => match crate::emotes::prepare_visuals(variant) {
                            Ok((emote,_)) => Some(emote),
                            Err(err) => {self.render_studio.status=err;return;}
                        },
                        None=>{self.render_studio.status=format!("Emote not found: {id}");return;}
                    }
                }
                Err(err)=>{self.render_studio.status=err.to_string();return;}
            }
        } else {None};
        let (first,end,fps)=if let Some(ref e)=emote {
            (e.first_frame,e.end_frame,f64::from(e.fps))
        } else {
            match self.asset_preview_player_presenter.as_ref()
                .and_then(|presenter|presenter.profile_preview_animation_span(&asset.name)) {
                Some((first,end,fps))=>(first,end,f64::from(fps)),
                None=>{self.render_studio.status=format!("Animation unavailable for current model: {}",asset.name);return;}
            }
        };
        if end<=first || fps<=0.0 {self.render_studio.status="Invalid source animation range".into();return;}
        let playback=if asset.cinematic.is_some() && emote.as_ref().is_some_and(|e|e.looping) {StudioClipPlayback::Loop}else{StudioClipPlayback::Hold};
        let name=asset.cinematic.as_ref().map_or_else(||asset.name.clone(),|source|source.animation.clone());
        let clip=StudioAnimationClip {
            name,start:time,first_frame:first,end_frame:end,
            source_fps:fps,lane,source_first:first,source_end:end,
            emote,motion:vec![StudioMotionKey::default()],tail_frames:0,
            playback,
        };
        self.render_studio.timeline.add_clip(clip);
        self.render_studio.selected_background=false;
        self.render_studio.status=format!("Placed animation on character {}",lane+1);
        self.profile_preview_key=None;
    }

    pub(in crate::app) fn save_render_studio_project(&mut self) {
        let project = StudioProject {
            version: 6,
            model: self.render_studio.model.clone(),
            with_sabers: self.render_studio.with_sabers,
            background: match self.render_studio.background {
                StudioBackground::Black => "black", StudioBackground::Green => "green",
                StudioBackground::Blue => "blue", StudioBackground::Levelshot => "levelshot",
            }.into(),
            background_map: self.render_studio.background_map.clone(),
            clips: self.render_studio.timeline.clips.iter().map(|c| StudioProjectClip {
                name: c.name.clone(),
                cinematic: c.emote.as_ref().and_then(super::render_studio_cinematics::StudioCinematicSource::from_emote),
                emote_id: c.emote.as_ref().filter(|e|super::render_studio_cinematics::StudioCinematicSource::from_emote(e).is_none()).map(|e| e.id.clone()),
                start: c.start, first_frame: c.first_frame, end_frame: c.end_frame,
                source_fps: c.source_fps, lane: c.lane, source_first: Some(c.source_first),
                source_end: Some(c.source_end), motion: c.motion.clone(),
                tail_frames: c.tail_frames, playback: c.playback,
            }).collect(),
            camera: self.render_studio.timeline.camera.clone(),
            camera_rig: self.render_studio.timeline.camera_rig.clone(),
            lanes: self.render_studio.timeline.lanes,
            in_point: self.render_studio.timeline.in_point,
            out_point: self.render_studio.timeline.out_point,
            interpolation: self.render_studio.timeline.interpolation,
            lane_models: self.render_studio.timeline.lane_models.clone(),
        };
        let result = (|| -> Result<PathBuf, String> {
            let dir = self.game_write_path("render-studio");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let path = dir.join(self.render_studio.project_file_name());
            let bytes = serde_json::to_vec_pretty(&project).map_err(|e| e.to_string())?;
            std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
            Ok(path)
        })();
        self.render_studio.status = match result {
            Ok(path) => format!("Saved editable timeline to {}", path.display()),
            Err(err) => format!("Could not save timeline: {err}"),
        };
    }

    pub(in crate::app) fn load_render_studio_project(&mut self) {
        let result = (|| -> Result<(StudioProject, Vec<StudioAnimationClip>), String> {
            let path = self.game_write_path("render-studio").join(self.render_studio.project_file_name());
            let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            if text.len() > 1_000_000 { return Err("Project exceeds 1 MB".into()); }
            let project: StudioProject = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            if !(1..=6).contains(&project.version) { return Err("Unsupported timeline project version".into()); }
            if project.clips.len() > 500 || project.camera.len() > 500 {
                return Err("Project exceeds 500 clips / camera keys".into());
            }
            let mut clips = Vec::<StudioAnimationClip>::new();
            for c in &project.clips {
                if c.lane > 15 || c.motion.len()>1000 || c.tail_frames<0 || c.tail_frames>360000 ||
                    !c.start.is_finite() || c.start < 0.0 || c.start > 36000.0 ||
                    !c.source_fps.is_finite() || c.source_fps <= 0.0 || c.source_fps > 1000.0 ||
                    c.end_frame <= c.first_frame || c.end_frame - c.first_frame > 1_000_000 {
                    return Err("Invalid animation clip in project".into());
                }
                let emote = if let Some(source)=c.cinematic.as_ref() {
                    let clip=self.resolve_studio_cinematic(source)?;
                    if c.emote_id.is_some() || c.first_frame<clip.first_frame || c.end_frame>clip.end_frame()
                        || (c.source_fps-f64::from(clip.fps)).abs()>0.000001
                        || c.source_first.is_some_and(|f|f!=clip.first_frame) || c.source_end.is_some_and(|f|f!=clip.end_frame()) {
                        return Err("Cinematic clip trim is outside its source animation".into());
                    }
                    Some(clip.emote()?)
                } else if let Some(id) = c.emote_id.as_deref() {
                    let catalog = crate::emotes::catalog(&self.base).map_err(|e| e.to_string())?;
                    let variant = catalog.iter().flat_map(|root|
                        std::iter::once(root).chain(root.variants.iter()))
                        .find(|candidate| candidate.id == id)
                        .cloned().ok_or_else(|| format!("Missing Fortnite emote {id}"))?;
                    Some(crate::emotes::prepare_visuals(variant).map_err(|e| e.to_string())?.0)
                } else { None };
                clips.push(StudioAnimationClip {
                    name: c.name.clone(), emote, start: c.start,
                    first_frame: c.first_frame, end_frame: c.end_frame,
                    source_fps: c.source_fps, lane: c.lane,
                    source_first: c.source_first.unwrap_or(c.first_frame).min(c.first_frame),
                    source_end: c.source_end.unwrap_or(c.end_frame).max(c.end_frame),
                    motion: c.motion.clone(),tail_frames:c.tail_frames,playback:c.playback,
                });
            }
            clips.sort_by(|a,b| a.lane.cmp(&b.lane).then_with(||a.start.total_cmp(&b.start)));
            if clips.windows(2).any(|p| p[0].lane==p[1].lane && p[0].end() > p[1].start + 0.00001) {
                return Err("Overlapping animation clips are unsupported".into());
            }
            if project.lanes==0 || project.lanes>16 || !project.camera_rig.valid(project.lanes) || !project.in_point.is_finite() || project.in_point<0.0 ||
                project.out_point.is_some_and(|t|!t.is_finite() || t <= project.in_point || t > 36000.0) ||
                clips.iter().any(|c|c.lane>=project.lanes || c.motion.iter().any(|k|
                    !k.time.is_finite() || k.time<0.0 || k.time>(c.source_end-c.source_first+c.tail_frames) as f64/c.source_fps+0.0001 ||
                    !k.yaw.is_finite() || k.position.iter().any(|v|!v.is_finite()))) ||
                project.camera.iter().any(|k| !k.time.is_finite() || k.time < 0.0 ||
                k.time > 36000.0 || !k.fov.is_finite() || k.fov < 1.0 || k.fov > 176.0 ||
                !k.yaw.is_finite() || !k.pitch.is_finite() || k.rig_orbit.is_some_and(|o|!o.valid()) ||
                k.position.iter().any(|c| !c.is_finite())) {
                return Err("Invalid camera key in project".into());
            }
            Ok((project, clips))
        })();
        match result {
            Ok((project, clips)) => {
                self.render_studio.cinematic_preview=None;
                self.render_studio.timeline.clips = clips;
                self.render_studio.initial_clip_seeded = true;
                self.render_studio.timeline.lanes=project.lanes;
                self.render_studio.timeline.lane_models=project.lane_models;
                self.render_studio.timeline.lane_models.resize(project.lanes,String::new());
                self.render_studio.timeline.in_point=project.in_point;
                self.render_studio.timeline.out_point=project.out_point;
                self.render_studio.timeline.interpolation=project.interpolation;
                self.render_studio.timeline.drag=None;
                self.render_studio.timeline.selected_motion=None;
                self.render_studio.timeline.camera = project.camera;
                self.render_studio.timeline.camera_rig=project.camera_rig;
                self.render_studio.camera_rig_runtime=Default::default();
                self.render_studio.camera_pending_match=None;
                self.render_studio.spatial=Default::default();
                self.render_studio.timeline.camera.sort_by(|a,b|a.time.total_cmp(&b.time));
                self.render_studio.timeline.camera.dedup_by(|a,b|(a.time-b.time).abs()<0.00001);
                self.render_studio.timeline.selected_clip = None;
                self.render_studio.timeline.selected_camera = None;
                self.render_studio.timeline.clear_selection();
                self.render_studio.timeline.seek(0.0);
                self.render_studio.model = project.model;
                self.render_studio.with_sabers = project.with_sabers;
                self.render_studio.background = match project.background.as_str() {
                    "green" => StudioBackground::Green, "blue" => StudioBackground::Blue,
                    "levelshot" => StudioBackground::Levelshot, _ => StudioBackground::Black,
                };
                self.render_studio.background_map = project.background_map;
                self.render_studio_update_backdrop();
                self.profile_preview_key = None;
                self.render_studio.history=StudioEditHistory::default();
                self.render_studio.status = format!("Loaded {}", self.game_write_path("render-studio").join(self.render_studio.project_file_name()).display());
            }
            Err(error) => self.render_studio.status = format!("Could not load timeline: {error}"),
        }
    }
}

impl App {
    pub(in crate::app) fn open_render_studio(&mut self) {
        self.render_studio.cinematic_preview=None;
        self.render_studio.spatial.drag=None;
        self.cancel_render_studio_export();
        self.render_studio.still = None;
        crate::emotes::stop();
        if self.render_studio.model.is_empty() {
            self.render_studio.model = self.solo_client_info.model_cvar();
        }
        self.render_studio.playing = false;
        self.render_studio.playhead = 0.0;
        self.render_studio.timeline.playing = false;
        self.render_studio.status.clear();
        self.frontend_page = FrontendPage::RenderStudio;
        self.ensure_profile_catalog();
        self.sync_profile_preview_mode(true);
        self.profile_preview_key = None;
        self.render_command(RenderCommand::SetAssetPreviewMode(true));
        self.render_studio_update_backdrop();
        self.egui_repaint_requested = true;
    }

    pub(in crate::app) fn render_studio_update_backdrop(&mut self) {
        if self.render_studio.background == StudioBackground::Black {
            self.profile_background_requested = None;
            self.render_command(RenderCommand::SetAssetPreviewCheckerboard);
            return;
        }
        let color: Option<[u8; 4]> = match self.render_studio.background {
            StudioBackground::Black => unreachable!(),
            StudioBackground::Green => Some([0, 177, 48, 255]),
            StudioBackground::Blue => Some([14, 54, 239, 255]),
            StudioBackground::Levelshot => None,
        };
        if let Some(color) = color {
            self.profile_background_requested = None;
            self.render_command(RenderCommand::SetAssetPreviewBackground(Some(([2, 2], color.repeat(4)))));
        } else {
            self.render_command(RenderCommand::SetAssetPreviewBackground(None));
            if !self.render_studio.background_map.is_empty() {
                let map = self.render_studio.background_map.clone();
                self.profile_background_requested = Some(map.clone());
                self.ui_catalog.request(&self.base, self.game.as_deref(),
                    crate::app::ui_catalog::CatalogRequest::ProfileBackground { map_name: map });
            }
        }
    }

    pub(in crate::app) fn start_render_studio_export(&mut self) {
        self.render_studio.cinematic_preview=None;
        if self.render_studio.export.is_some() || self.render_studio.still.is_some() { return; }
        if self.render_studio.timeline.clips.is_empty() {
            self.render_studio.status = "Add an animation to the timeline before exporting".into();
            return;
        }
        let Some(_ffmpeg) = adjacent_ffmpeg_path() else {
            self.render_studio.status = "Place ffmpeg.exe next to DinurdoJK.exe".into();
            return;
        };
        // One frame for each output timestamp in [start, end); no doubled final pose.
        let fps = self.render_studio.fps;
        let (in_time, out_time) = self.render_studio.timeline.export_range();
        let total_frames = ((out_time-in_time) * fps as f64).ceil().max(1.0) as u64;
        if total_frames > 120_000 {
            self.render_studio.status = "Trim the clip to 120,000 output frames or fewer".into();
            return;
        }
        let folder = self.game_write_path("render-studio");
        if let Err(error) = std::fs::create_dir_all(&folder) {
            self.render_studio.status = format!("Output directory: {error}");
            return;
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
        let output = folder.join(format!("studio_{stamp}.{}", self.render_studio.preset.extension()));
        let alpha = self.render_studio.transparent;
        if alpha && !self.render_studio.preset.supports_alpha() {
            self.render_studio.status = "This video format does not support transparency".into();
            return;
        }
        let depth = self.render_studio.export_depth;
        let depth_dir = depth.then(|| folder.join(format!("studio_{stamp}_depth")));
        if let Some(dir) = depth_dir.as_ref() {
            if let Err(error) = std::fs::create_dir_all(dir) {
                self.render_studio.status = format!("Could not create EXR depth directory: {error}");
                return;
            }
        }
        self.render_studio.timeline.seek(in_time);
        self.render_studio.playing = false;
        self.render_studio.export = Some(RenderStudioExport {
            frame_index: 0, total_frames, fps, awaiting_frame: false, capture_queued: false,
            output: output.clone(), child: None, stdin: None, preset: self.render_studio.preset,
            alpha, depth, depth_dir, depth_child: None, depth_stdin: None,
        });
        self.profile_preview_key = None;
        self.render_studio.status = format!("Rendering {total_frames} clean frames to {}", output.display());
        self.push_console_line(format!("^2RENDER STUDIO:^7 {}", output.display()));
        self.egui_repaint_requested = true;
    }

    pub(in crate::app) fn start_render_studio_screenshot(&mut self) {
        self.render_studio.cinematic_preview=None;
        if self.render_studio.export.is_some() || self.render_studio.still.is_some() { return; }
        let folder = self.game_write_path("render-studio");
        if let Err(error) = std::fs::create_dir_all(&folder) {
            self.render_studio.status = format!("Screenshot output directory: {error}");
            return;
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
        let output = folder.join(format!("studio_{stamp}.png"));
        if self.render_studio.screenshot_depth && adjacent_ffmpeg_path().is_none() {
            self.render_studio.status = "ffmpeg.exe required next to game for depth EXR".into();
            return;
        }
        let current_time = self.render_studio.timeline_time();
        self.render_studio.timeline.seek(current_time);
        self.render_studio.playhead = self.render_studio.offset_frames();
        self.render_studio.playing = false;
        self.render_studio.still = Some(RenderStudioStill {
            awaiting_frame: false, output,
            alpha: self.render_studio.screenshot_alpha,
            depth: self.render_studio.screenshot_depth,
        });
        self.profile_preview_key = None;
        self.egui_repaint_requested = true;
    }

    fn set_render_studio_capture_viewport(&mut self) {
        if let Some(window) = self.window.as_ref() {
            let size = window.inner_size();
            let viewport = [0, 0, size.width.max(1), size.height.max(1)];
            if self.asset_preview_viewport_key != Some(viewport) {
                self.asset_preview_viewport_key = Some(viewport);
                self.render_command(RenderCommand::SetAssetPreviewViewport(Some(viewport)));
            }
        }
    }

    fn reset_render_studio_capture_viewport(&mut self) {
        self.asset_preview_viewport_key = None;
        self.render_command(RenderCommand::SetAssetPreviewViewport(None));
        self.egui_repaint_requested = true;
    }

    pub(in crate::app) fn tick_render_studio_export(&mut self) {
        if let Some(still) = self.render_studio.still.as_ref() {
            if still.awaiting_frame { return; }
            if self.frontend_page != FrontendPage::RenderStudio || !self.profile_preview_active {
                self.render_studio.still = None;
                self.reset_render_studio_capture_viewport();
                return;
            }
            let depth = still.depth;
            let alpha = still.alpha;
            self.set_render_studio_capture_viewport();
            self.profile_preview_key = None;
            self.update_profile_preview();
            self.publish_snapshot();
            self.render_command(RenderCommand::CaptureVideoFrame { frame_index: 0, depth, alpha });
            if let Some(still) = self.render_studio.still.as_mut() { still.awaiting_frame = true; }
            return;
        }
        let Some(export) = self.render_studio.export.as_ref() else { return; };
        if export.awaiting_frame { return; }
        if self.frontend_page != FrontendPage::RenderStudio || !self.profile_preview_active {
            self.cancel_render_studio_export();
            return;
        }
        // Capture is full-window at the user's current output resolution, while
        // the interactive stage is an inset viewport. The renderer omits GUI
        // composition from requested video frames, never from normal preview.
        self.set_render_studio_capture_viewport();
        self.profile_preview_key = None;
        self.update_profile_preview();
        self.publish_snapshot();
        let frame_index = self.render_studio.export.as_ref().map_or(0, |e| e.frame_index);
        let (depth, alpha) = self.render_studio.export.as_ref()
            .map_or((false, false), |e| (e.depth, e.alpha));
        self.render_command(RenderCommand::CaptureVideoFrame { frame_index, depth, alpha });
        if let Some(export) = self.render_studio.export.as_mut() {
            export.awaiting_frame = true;
            export.capture_queued = true;
        }
    }

    pub(in crate::app) fn on_render_studio_still_frame(
        &mut self, index: u64, width: u32, height: u32, bgra: Vec<u8>, depth: Option<Vec<u8>>,
    ) {
        let result = (|| -> Result<PathBuf, String> {
            let still = self.render_studio.still.as_ref().ok_or("No pending Render Studio screenshot")?;
            if !still.awaiting_frame || index != 0 { return Err("Unexpected screenshot frame".into()); }
            let output = still.output.clone();
            let alpha = still.alpha;
            let need_depth = still.depth;
            let mut rgba = Vec::with_capacity(bgra.len());
            for px in bgra.chunks_exact(4) {
                rgba.extend_from_slice(&[px[2], px[1], px[0], if alpha { px[3] } else { 255 }]);
            }
            let file = std::fs::File::create(&output).map_err(|e| format!("PNG create: {e}"))?;
            let writer = std::io::BufWriter::new(file);
            image::codecs::png::PngEncoder::new(writer)
                .write_image(&rgba, width, height, image::ExtendedColorType::Rgba8)
                .map_err(|e| format!("PNG encode: {e}"))?;
            if need_depth {
                let pixels = depth.ok_or("Depth capture returned no depth pixels")?;
                let depth_path = output.with_file_name(format!("{}_depth.exr",
                    output.file_stem().and_then(|name| name.to_str()).unwrap_or("studio")));
                let ffmpeg = adjacent_ffmpeg_path().ok_or("ffmpeg.exe required to write depth EXR")?;
                let mut child = Command::new(ffmpeg)
                    .args(["-hide_banner", "-loglevel", "error", "-y",
                        "-f", "rawvideo", "-pix_fmt", "grayf32le"])
                    .arg("-s").arg(format!("{width}x{height}"))
                    .args(["-i", "pipe:0", "-frames:v", "1", "-c:v", "exr", "-compression", "zip16"])
                    .arg(&depth_path)
                    .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped())
                    .spawn().map_err(|e| format!("Depth FFmpeg spawn: {e}"))?;
                let mut stdin = child.stdin.take().ok_or("Depth FFmpeg stdin missing")?;
                stdin.write_all(&pixels).map_err(|e| format!("Depth EXR write: {e}"))?;
                drop(stdin);
                let done = child.wait_with_output().map_err(|e| format!("Depth EXR wait: {e}"))?;
                if !done.status.success() {
                    return Err(format!("Depth EXR encoder: {}", String::from_utf8_lossy(&done.stderr)));
                }
            }
            Ok(output)
        })();
        self.render_studio.still = None;
        self.reset_render_studio_capture_viewport();
        match result {
            Ok(path) => {
                self.render_studio.status = format!("Screenshot saved: {}", path.display());
                self.push_console_path_line(format!("^2{}", self.render_studio.status), path);
            }
            Err(error) => {
                self.render_studio.status = format!("Studio screenshot failed: {error}");
                self.push_console_line(format!("^1{}", self.render_studio.status));
            }
        }
    }

    pub(in crate::app) fn on_render_studio_frame(&mut self, index: u64, width: u32, height: u32, bgra: Vec<u8>, depth_f32le: Option<Vec<u8>>) {
        let result = (|| -> Result<bool, String> {
            let export = self.render_studio.export.as_mut().ok_or("No studio export")?;
            if !export.awaiting_frame || index != export.frame_index {
                return Err(format!("Unexpected studio frame {index}, expected {}", export.frame_index));
            }
            if export.child.is_none() {
                let ffmpeg = adjacent_ffmpeg_path().ok_or("ffmpeg.exe disappeared")?;
                let mut command = Command::new(ffmpeg);
                command.args(["-hide_banner", "-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "bgra"])
                    .arg("-s").arg(format!("{width}x{height}"))
                    .arg("-r").arg(export.fps.to_string())
                    .args(["-i", "pipe:0", "-an"])
                    ;
                match export.preset {
                    StudioVideoFormat::WebmVp9 | StudioVideoFormat::WebmVp9Max => {
                        // VP9 alpha is an auxiliary stream in WebM: libvpx
                        // requires yuva420p and alternate references disabled.
                        // Preserve alpha even on hardware whose default FFmpeg
                        // conversion would otherwise flatten it to black.
                        let filter = if export.alpha {
                            "pad=ceil(iw/2)*2:ceil(ih/2)*2:color=black@0,format=yuva420p"
                        } else {
                            "pad=ceil(iw/2)*2:ceil(ih/2)*2,format=yuv420p"
                        };
                        let crf = if export.preset == StudioVideoFormat::WebmVp9Max { "18" } else { "28" };
                        command.args(["-vf", filter, "-c:v", "libvpx-vp9",
                            "-pix_fmt", if export.alpha { "yuva420p" } else { "yuv420p" },
                            "-auto-alt-ref", "0", "-b:v", "0", "-crf", crf,
                            "-deadline", "good", "-cpu-used", "3", "-row-mt", "1"]);
                    }
                    StudioVideoFormat::Gif => {
                        // A fresh palette each frame permits *streaming* GIF
                        // encoding without retaining the entire animation in
                        // memory. GIF transparency is only binary; smooth alpha
                        // is quantized at a defined 128/255 threshold.
                        command.args(["-vf", "format=rgba,split[pal_in][gif_in];[pal_in]palettegen=stats_mode=single:reserve_transparent=1[pal];[gif_in][pal]paletteuse=new=1:dither=sierra2_4a:alpha_threshold=128",
                            "-gifflags", "+transdiff", "-loop", "0"]);
                    }
                    StudioVideoFormat::ProRes444 if export.alpha => {
                        command.args(["-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2:color=black@0,format=yuva444p10le",
                            "-c:v", "prores_ks", "-profile:v", "4", "-pix_fmt", "yuva444p10le", "-alpha_bits", "16"]);
                    }
                    preset => {
                        // Preserve the original opaque H.264/ProRes encoding
                        // path and its BT.709/range tagging.
                        command.args(["-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2,scale=in_range=full:out_range=tv:out_color_matrix=bt709,setparams=range=limited:color_primaries=bt709:color_trc=iec61966-2-1:colorspace=bt709"])
                            .args(["-color_range", "tv", "-color_primaries", "bt709", "-color_trc", "iec61966-2-1", "-colorspace", "bt709"])
                            .args(preset.demo_preset().expect("Non-demo formats handled above").ffmpeg_video_args());
                    }
                }
                command.arg(&export.output)
                    .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
                let mut child = command.spawn().map_err(|e| format!("FFmpeg spawn: {e}"))?;
                export.stdin = child.stdin.take();
                export.child = Some(child);
            }
            if bgra.len() != width as usize * height as usize * 4 {
                return Err("Studio color capture size mismatch".into());
            }
            export.stdin.as_mut().ok_or("No FFmpeg stdin")?
                .write_all(&bgra).map_err(|e| format!("FFmpeg write frame {index}: {e}"))?;
            if export.depth {
                let pixels = depth_f32le.ok_or("Studio depth readback missing")?;
                if pixels.len() != bgra.len() { return Err("Studio depth readback size mismatch".into()); }
                if export.depth_child.is_none() {
                    let ffmpeg = adjacent_ffmpeg_path().ok_or("ffmpeg.exe not found for depth")?;
                    let output = export.depth_dir.as_ref().ok_or("Missing depth output directory")?
                        .join("depth_%06d.exr");
                    let mut child = Command::new(ffmpeg)
                        .args(["-hide_banner", "-loglevel", "error", "-y",
                            "-f", "rawvideo", "-pix_fmt", "grayf32le"])
                        .arg("-s").arg(format!("{width}x{height}"))
                        .arg("-r").arg(export.fps.to_string())
                        .args(["-i", "pipe:0", "-c:v", "exr", "-compression", "zip16", "-start_number", "0"])
                        .arg(output)
                        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped())
                        .spawn().map_err(|e| format!("Depth encoder spawn: {e}"))?;
                    export.depth_stdin = child.stdin.take();
                    export.depth_child = Some(child);
                }
                export.depth_stdin.as_mut().ok_or("Depth FFmpeg stdin missing")?
                    .write_all(&pixels).map_err(|e| format!("Depth encoder frame {index}: {e}"))?;
            }
            export.frame_index += 1;
            export.awaiting_frame = false;
            export.capture_queued = false;
            Ok(export.frame_index == export.total_frames)
        })();
        match result {
            Ok(true) => self.finish_render_studio_export(),
            Ok(false) => { self.egui_repaint_requested = true; },
            Err(error) => self.fail_render_studio_export(error),
        }
    }

    fn finish_render_studio_export(&mut self) {
        let Some(mut export) = self.render_studio.export.take() else { return; };
        export.stdin.take();
        let result = export.child.take().expect("studio FFmpeg child must exist")
            .wait_with_output();
        export.depth_stdin.take();
        let depth_result = export.depth_child.take().map(|mut child| child.wait_with_output());
        let output = export.output.clone();
        match result {
            Ok(result) if result.status.success() && depth_result.as_ref().is_none_or(|r|
                r.as_ref().is_ok_and(|status| status.status.success())) => {
                self.render_studio.status = format!("Wrote {}{}", output.display(),
                    export.depth_dir.as_ref().map_or(String::new(), |dir| format!(" + depth EXRs in {}", dir.display())));
                self.push_console_path_line(format!("^2{}", self.render_studio.status), output);
            }
            other => {
                let detail = if let Some(Err(error)) = depth_result.as_ref() {
                    error.to_string()
                } else if let Some(Ok(depth)) = depth_result.as_ref().filter(|r|
                    r.as_ref().is_ok_and(|d| !d.status.success())) {
                    String::from_utf8_lossy(&depth.stderr).trim().to_owned()
                } else {
                    match other {
                        Ok(output) => String::from_utf8_lossy(&output.stderr).trim().to_owned(),
                        Err(e) => e.to_string(),
                    }
                };
                let _ = std::fs::remove_file(&output);
                if let Some(dir) = export.depth_dir.as_ref() { let _ = std::fs::remove_dir_all(dir); }
                self.render_studio.status = format!("Render Studio FFmpeg failed: {detail}");
                self.push_console_line(format!("^1{}", self.render_studio.status));
            }
        }
        self.asset_preview_viewport_key = None;
        self.render_command(RenderCommand::SetAssetPreviewViewport(None));
        self.egui_repaint_requested = true;
    }

    pub(in crate::app) fn cancel_render_studio_export(&mut self) {
        let Some(export) = self.render_studio.export.take() else { return; };
        let output = export.output.clone();
        let depth_dir = export.depth_dir.clone();
        drop(export); // closes pipes and terminates the encoders
        let _ = std::fs::remove_file(output);
        if let Some(dir) = depth_dir { let _ = std::fs::remove_dir_all(dir); }
        self.render_studio.status = "Render Studio export cancelled".into();
        self.asset_preview_viewport_key = None;
        self.render_command(RenderCommand::SetAssetPreviewViewport(None));
        self.egui_repaint_requested = true;
    }

    pub(in crate::app) fn fail_render_studio_export(&mut self, error: impl std::fmt::Display) {
        if self.render_studio.still.is_some() {
            self.render_studio.still = None;
            self.reset_render_studio_capture_viewport();
        } else {
            self.cancel_render_studio_export();
        }
        self.render_studio.status = format!("Render Studio export failed: {error}");
        self.push_console_line(format!("^1{}", self.render_studio.status));
    }
}

#[cfg(test)]
mod editor_history_tests {
    use super::*;
    fn group_clip(start:f64) -> StudioAnimationClip {
        StudioAnimationClip {name:"BOTH_STAND1".into(),emote:None,start,
            first_frame:0,end_frame:20,source_fps:20.0,lane:0,source_first:0,source_end:20,
            motion:vec![StudioMotionKey::default()],tail_frames:0,playback:StudioClipPlayback::Hold}
    }
    #[test] fn mixed_group_paste_preserves_spacing_and_undoes_together() {
        let mut studio=RenderStudio::default();
        studio.timeline.clips=vec![group_clip(0.0),group_clip(2.0)];
        studio.timeline.camera.push(StudioCameraKey {time:1.5,fov:80.0,..Default::default()});
        studio.timeline.out_point=Some(10.0);
        studio.timeline.set_selection(vec![StudioItem::Clip(0),StudioItem::Clip(1),StudioItem::Camera(1),StudioItem::Motion(0,0)]);
        assert!(studio.copy_selection());
        studio.timeline.seek(5.0);studio.init_history();
        assert!(studio.paste_selection());
        assert_eq!(studio.timeline.clips[2].start,5.0);
        assert_eq!(studio.timeline.clips[3].start,7.0);
        assert_eq!(studio.timeline.clips[2].motion.len(),1);
        assert_eq!(studio.timeline.camera[2].time,6.5);
        assert_eq!(studio.timeline.camera[2].fov,80.0);
        assert_eq!(studio.timeline.selected_items().len(),3);
        studio.track_edit(false);
        assert!(studio.undo_edit());
        assert_eq!(studio.timeline.clips.len(),2);
        assert_eq!(studio.timeline.camera.len(),2);
        assert!(studio.redo_edit());
        assert_eq!(studio.timeline.clips.len(),4);
    }
    #[test] fn group_paste_resolves_collisions_without_collapsing_offsets() {
        let mut studio=RenderStudio::default();
        studio.timeline.snap=false;
        studio.timeline.clips=vec![group_clip(0.0),group_clip(2.0)];
        studio.timeline.set_selection(vec![StudioItem::Clip(0),StudioItem::Clip(1)]);
        assert!(studio.copy_selection());
        studio.timeline.seek(0.5);
        assert!(studio.paste_selection());
        assert_eq!(studio.timeline.clips[2].start,1.0);
        assert_eq!(studio.timeline.clips[3].start,3.0);
    }
    #[test] fn movement_group_paste_validates_all_destinations_before_editing() {
        let mut studio=RenderStudio::default();
        let mut clip=group_clip(0.0);
        clip.motion.push(StudioMotionKey {time:0.25,position:[12.0,0.0,0.0],..Default::default()});
        studio.timeline.clips=vec![clip];
        studio.timeline.snap=false;
        studio.timeline.out_point=Some(4.0);
        studio.timeline.set_selection(vec![StudioItem::Motion(0,0),StudioItem::Motion(0,1)]);
        assert!(studio.copy_selection());
        studio.timeline.seek(0.5);
        assert!(studio.paste_selection());
        assert_eq!(studio.timeline.clips[0].motion.len(),4);
        assert_eq!(studio.timeline.clips[0].motion[3].time,0.75);
        assert_eq!(studio.timeline.clips[0].motion[3].position,[12.0,0.0,0.0]);
        studio.timeline.seek(0.9);
        assert!(!studio.paste_selection());
        assert_eq!(studio.timeline.clips[0].motion.len(),4);
    }
    #[test]
    fn render_studio_models_resolve_each_layer_and_follow_edits() {
        let mut studio=RenderStudio::default();
        studio.model="kyle/default".into();
        studio.timeline.lane_models=vec!["alora2/blue".into(),"luke/default".into(),String::new()];
        assert_eq!(studio.actor_model(0),"alora2/blue");
        assert_eq!(studio.actor_model(1),"luke/default");
        assert_eq!(studio.actor_model(2),"kyle/default");
        studio.timeline.lane_models[0]="jan/default".into();
        assert_eq!(studio.actor_model(0),"jan/default");
        studio.timeline.lane_models[0].clear();
        assert_eq!(studio.actor_model(0),"kyle/default");
    }
    fn clipboard_frame(studio: &mut RenderStudio, ctx: &egui::Context, events: Vec<egui::Event>) -> bool {
        let mut pasted=false;
        let _=ctx.run_ui(egui::RawInput {events,..Default::default()}, |ui| {
            pasted=studio.handle_clipboard_input(ui.ctx());
        });
        pasted
    }
    #[test]
    fn clipboard_events_work_without_ctrl_held_or_os_clipboard_text() {
        let mut studio=RenderStudio::default();
        let ctx=egui::Context::default();
        let _=ctx.run_ui(Default::default(),|ui| {
            ui.button("Timeline item").request_focus();
        });
        assert!(ctx.memory(|m|m.focused().is_some()));
        assert!(!ctx.text_edit_focused());
        studio.timeline.selected_camera=Some(0);
        studio.timeline.out_point=Some(4.0);
        studio.init_history();
        assert!(!clipboard_frame(&mut studio,&ctx,vec![egui::Event::Copy]));
        studio.timeline.camera[0].fov=90.0;
        studio.timeline.seek(2.0);
        assert!(clipboard_frame(&mut studio,&ctx,vec![egui::Event::Paste(String::new())]));
        assert_eq!(studio.timeline.camera.len(),2);
        assert_eq!(studio.timeline.camera[1].time,2.0);
        assert_eq!(studio.timeline.camera[1].fov,58.0);
        studio.track_edit(false);
        assert!(studio.undo_edit());
        assert_eq!(studio.timeline.camera.len(),1);
        assert!(studio.redo_edit());
        assert_eq!(studio.timeline.camera[1].time,2.0);
    }
    #[test]
    fn clipboard_events_copy_then_paste_clip_in_one_frame() {
        let mut studio=RenderStudio::default();
        let ctx=egui::Context::default();
        studio.timeline.add_clip(StudioAnimationClip {
            name:"BOTH_STAND1".into(),emote:None,start:0.0,
            first_frame:10,end_frame:30,source_fps:20.0,lane:0,
            source_first:0,source_end:40,motion:vec![StudioMotionKey::default()],
            tail_frames:0,playback:StudioClipPlayback::Loop,
        });
        studio.timeline.out_point=Some(4.0);
        studio.timeline.seek(2.0);
        assert!(clipboard_frame(&mut studio,&ctx,vec![egui::Event::Copy,egui::Event::Paste("unrelated OS text".into())]));
        assert_eq!(studio.timeline.clips.len(),2);
        assert_eq!(studio.timeline.clips[0].start,0.0);
        let pasted=&studio.timeline.clips[1];
        assert_eq!(pasted.start,2.0);
        assert_eq!(pasted.first_frame,10);
        assert_eq!(pasted.end_frame,30);
        assert_eq!(pasted.playback,StudioClipPlayback::Loop);

        studio.timeline.selected_motion=Some((1,0));
        studio.timeline.clips[1].motion[0].position=[12.0,3.0,4.0];
        assert!(!clipboard_frame(&mut studio,&ctx,vec![egui::Event::Copy]));
        studio.timeline.seek(2.5);
        assert!(clipboard_frame(&mut studio,&ctx,vec![egui::Event::Paste(String::new())]));
        let motion=&studio.timeline.clips[1].motion[1];
        assert_eq!(motion.time,1.0); // Half a second of source trim plus playhead offset.
        assert_eq!(motion.position,[12.0,3.0,4.0]);
    }
    #[test]
    fn clipboard_events_belong_to_focused_text_fields() {
        let mut studio=RenderStudio::default();
        let ctx=egui::Context::default();
        let id=egui::Id::new("studio_test_search");
        let mut search=String::new();
        let _=ctx.run_ui(Default::default(),|ui| {
            ui.add(egui::TextEdit::singleline(&mut search).id(id)).request_focus();
        });
        studio.timeline.selected_camera=Some(0);
        let _=ctx.run_ui(egui::RawInput {events:vec![egui::Event::Copy,egui::Event::Paste("search".into())],..Default::default()},|ui| {
            assert!(ui.ctx().text_edit_focused());
            assert!(!studio.handle_clipboard_input(ui.ctx()));
            assert!(studio.clipboard.is_none());
            ui.add(egui::TextEdit::singleline(&mut search).id(id));
        });
        assert_eq!(search,"search");
    }
    #[test]
    fn history_coalesces_pointer_drags_and_roundtrips_undo_redo() {
        let mut studio=RenderStudio::default();
        studio.init_history();
        studio.model="kyle/default".into();
        studio.track_edit(true);
        studio.model="jan/default".into();
        studio.track_edit(true);
        assert!(studio.history.undo.is_empty());
        studio.track_edit(false);
        assert_eq!(studio.history.undo.len(),1);
        assert!(studio.undo_edit());
        assert_eq!(studio.model,"");
        assert!(studio.redo_edit());
        assert_eq!(studio.model,"jan/default");
    }
    #[test]
    fn clipboard_clip_targets_other_lane_without_mutating_source_copy() {
        let mut studio=RenderStudio::default();
        studio.timeline.lanes=2;
        studio.timeline.lane_models.push("kyle/default".into());
        studio.timeline.add_clip(StudioAnimationClip {
            name:"BOTH_STAND1".into(),emote:None,start:0.0,
            first_frame:0,end_frame:20,source_fps:20.0,lane:0,
            source_first:0,source_end:20,motion:vec![StudioMotionKey::default()],
            tail_frames:0,playback:StudioClipPlayback::Hold,
        });
        assert!(studio.copy_selection());
        assert!(studio.paste_selection_on_lane(Some(1)));
        assert_eq!(studio.timeline.clips[1].lane,1);
        assert!(studio.paste_selection_on_lane(Some(0)));
        assert_eq!(studio.timeline.clips[2].lane,0);
        assert_eq!(studio.timeline.clips[0].lane,0);
    }
    #[test]
    fn clipboard_camera_key_pastes_at_playhead() {
        let mut studio=RenderStudio::default();
        studio.timeline.selected_camera=Some(0);
        assert!(studio.copy_selection());
        studio.timeline.seek(0.0);
        studio.timeline.camera[0].fov=80.0;
        // Copy remembers the original camera value, not a reference.
        assert!(studio.paste_selection());
        assert_eq!(studio.timeline.camera[0].fov,58.0);
    }
}
