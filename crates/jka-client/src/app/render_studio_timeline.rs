//! Deterministic Render Studio NLE timeline model, independent of egui/Taffy.
//! Times are seconds; source end frames are exclusive; motion-key times are
//! relative to the untrimmed source clip.  Output evaluation never depends on UI FPS.
use crate::emotes::{Emote, Preview};
use super::render_studio_camera::{StudioCameraRig,StudioCameraOrbit};
use serde::{Deserialize, Serialize};
use std::time::Instant;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(in crate::app) struct StudioMotionKey {
    /// Time measured from source_first, not the truncated visible clip edge.
    pub time: f64,
    /// Character offset in JKA world units, relative to the preview stage origin.
    pub position: [f32; 3],
    pub yaw: f32,
}
impl Default for StudioMotionKey {
    fn default() -> Self { Self { time: 0.0, position: [0.0; 3], yaw: 0.0 } }
}

#[derive(Clone)]
pub(in crate::app) struct StudioAnimationClip {
    pub name: String,
    pub emote: Option<Emote>,
    pub start: f64,
    pub first_frame: i32,
    pub end_frame: i32,
    pub source_fps: f64,
    pub lane: usize,
    pub source_first: i32,
    pub source_end: i32,
    pub motion: Vec<StudioMotionKey>,
    /// Extra frames after the authored clip. Rendered by holding or looping.
    pub tail_frames: i32,
    pub playback: StudioClipPlayback,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::app) enum StudioClipPlayback { #[default] Hold, Loop }

impl StudioAnimationClip {
    pub fn duration(&self) -> f64 { (f64::from(self.end_frame-self.first_frame+self.tail_frames.max(0))/self.source_fps.max(1.0)).max(0.001) }
    pub fn source_duration(&self) -> f64 { f64::from(self.end_frame-self.first_frame).max(1.0)/self.source_fps.max(1.0) }
    pub fn end(&self) -> f64 { self.start + self.duration() }
    pub fn label(&self) -> &str { self.emote.as_ref().map_or(self.name.as_str(), |e| e.label.as_str()) }
    pub fn pose_time_ms(&self, time: f64) -> i32 {
        let elapsed = (time-self.start).clamp(0.0,(self.duration()-0.00001).max(0.0));
        let native = self.source_duration();
        let local = match self.playback {
            StudioClipPlayback::Hold => elapsed.min((native-0.00001).max(0.0)),
            StudioClipPlayback::Loop => elapsed.rem_euclid(native.max(0.00001)),
        };
        let frame = if let Some(emote) = &self.emote {
            f64::from(self.first_frame - emote.first_frame).max(0.0)
        } else { 0.0 };
        ((frame/self.source_fps.max(1.0)+local)*1000.0).round().clamp(0.0,i32::MAX as f64) as i32
    }
    pub fn preview(&self, time: f64) -> Option<Preview> {
        self.emote.clone().map(|mut emote| {
            // Studio owns cinematic trim/repeat clocks. Freeze at the last pose
            // when a rounded millisecond lands exactly on the source end.
            if super::render_studio_cinematics::StudioCinematicSource::from_emote(&emote).is_some() {
                emote.looping=false;emote.loop_start_frame=None;
            }
            Preview { emote, elapsed_ms: self.pose_time_ms(time) }
        })
    }
    pub fn motion_at(&self, absolute_time: f64, smooth: bool) -> StudioMotionKey {
        let source_offset=(self.first_frame-self.source_first) as f64/self.source_fps.max(1.0);
        // Transform keys are independent of the pose playback mode. An actor
        // can walk while holding its last animation frame, or loop an emote
        // while following a non-looping camera/position path.
        let local=source_offset+(absolute_time-self.start).clamp(0.0,self.duration());
        let Some(first) = self.motion.first().copied() else { return StudioMotionKey::default() };
        if local <= first.time { return first; }
        for keys in self.motion.windows(2) {
            let (a,b)=(keys[0],keys[1]);
            if local <= b.time {
                let mut t=((local-a.time)/(b.time-a.time).max(0.00001)).clamp(0.0,1.0) as f32;
                if smooth { t=t*t*(3.0-2.0*t); }
                let dyaw=(b.yaw-a.yaw+180.0).rem_euclid(360.0)-180.0;
                return StudioMotionKey {time: local,
                    position: std::array::from_fn(|i| a.position[i]+(b.position[i]-a.position[i])*t),
                    yaw: a.yaw+dyaw*t};
            }
        }
        *self.motion.last().unwrap_or(&first)
    }
    pub fn add_motion_key(&mut self, mut key: StudioMotionKey) -> usize {
        let source_offset=(self.first_frame-self.source_first) as f64/self.source_fps.max(1.0);
        key.time=key.time.clamp(source_offset,source_offset+self.duration());
        if let Some(i)=self.motion.iter().position(|k|(k.time-key.time).abs()<0.00001) {
            self.motion[i]=key; return i;
        }
        self.motion.push(key);
        self.motion.sort_by(|a,b|a.time.total_cmp(&b.time));
        self.motion.iter().position(|k|(k.time-key.time).abs()<0.00001).unwrap_or(0)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(in crate::app) struct StudioCameraKey {
    pub time: f64, pub position: [f32;3], pub yaw: f32, pub pitch: f32, pub fov: f32,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub rig_orbit: Option<StudioCameraOrbit>,
}
impl Default for StudioCameraKey {
    fn default() -> Self { Self { time: 0.0, position: [0.0,-11.0,0.0], yaw: 0.0, pitch: 5.0, fov: 58.0,rig_orbit:None } }
}
impl StudioCameraKey {
    /// Rotate the current camera around the actor without resetting its framing.
    pub fn orbit(&mut self, target: [f32;3], delta: [f32;2]) {
        if delta==[0.0,0.0] {return;}
        let target=glam::Vec3::from_array(target);
        let offset=glam::Vec3::from_array(self.position)-target;
        let yaw_delta=delta[0]*0.25;
        let pitch=(self.pitch-delta[1]*0.25).clamp(-89.0,89.0);
        let pitch_delta=pitch-self.pitch;
        self.yaw=(self.yaw+yaw_delta).rem_euclid(360.0);
        let yaw=self.yaw.to_radians();
        let right=glam::Vec3::new(yaw.sin(),0.0,yaw.cos());
        let rotation=glam::Quat::from_axis_angle(right,pitch_delta.to_radians())
            *glam::Quat::from_rotation_y(yaw_delta.to_radians());
        self.position=(target+rotation*offset).to_array();
        self.pitch=pitch;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(in crate::app) enum StudioInterpolation { Linear, #[default] Smooth, Spline }

#[derive(Serialize, Deserialize)]
pub(in crate::app) struct StudioProjectClip {
    pub name: String, pub emote_id: Option<String>, pub start: f64,
    #[serde(default)] pub cinematic: Option<super::render_studio_cinematics::StudioCinematicSource>,
    pub first_frame: i32, pub end_frame: i32, pub source_fps: f64,
    #[serde(default)] pub lane: usize,
    #[serde(default)] pub source_first: Option<i32>,
    #[serde(default)] pub source_end: Option<i32>,
    #[serde(default)] pub motion: Vec<StudioMotionKey>,
    #[serde(default)] pub tail_frames: i32,
    #[serde(default)] pub playback: StudioClipPlayback,
}
#[derive(Serialize, Deserialize)]
pub(in crate::app) struct StudioProject {
    pub version: u32, pub model: String, pub with_sabers: bool,
    pub background: String, pub background_map: String,
    pub clips: Vec<StudioProjectClip>, pub camera: Vec<StudioCameraKey>,
    #[serde(default)] pub camera_rig: StudioCameraRig,
    #[serde(default = "default_lanes")] pub lanes: usize,
    #[serde(default)] pub in_point: f64,
    #[serde(default)] pub out_point: Option<f64>,
    #[serde(default)] pub interpolation: StudioInterpolation,
    #[serde(default)] pub lane_models: Vec<String>,
}
fn default_lanes() -> usize { 1 }

/// Origin snapshots are used for pointer-anchored, non-incremental drags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum StudioDragKind { Clip, TrimLeft, TrimRight, Camera, Motion, In, Out }
#[derive(Clone, Copy, Debug)]
pub(in crate::app) struct StudioDrag {
    pub kind: StudioDragKind, pub index: usize, pub sub_index: usize,
    pub pointer_x: f32, pub initial_scroll: f32, pub initial_time: f64,
    pub initial_first: i32, pub initial_end: i32, pub initial_start: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(in crate::app) enum StudioItem { Clip(usize), Camera(usize), Motion(usize,usize) }

pub(in crate::app) struct StudioTimeline {
    pub clips: Vec<StudioAnimationClip>, pub camera: Vec<StudioCameraKey>,
    pub camera_rig: StudioCameraRig,
    pub lanes: usize,
    pub selected_clip: Option<usize>, pub selected_camera: Option<usize>,
    pub selected_motion: Option<(usize,usize)>,
    pub selection: Vec<StudioItem>,
    selection_anchor: Option<StudioItem>,
    pub playhead: f64, pub playing: bool, pub play_started: Instant,
    pub zoom: f32, pub scroll: f32, pub snap: bool,
    pub in_point: f64, pub out_point: Option<f64>,
    pub interpolation: StudioInterpolation,
    /// Optional per-character model overrides. Empty means global default.
    pub lane_models: Vec<String>,
    pub drag: Option<StudioDrag>,
}
impl Default for StudioTimeline {
    fn default() -> Self { Self {
        clips: Vec::new(), camera: vec![StudioCameraKey::default()], lanes: 1,
        camera_rig: StudioCameraRig::default(),
        selected_clip: None, selected_camera: None, selected_motion: None,
        selection: Vec::new(), selection_anchor: None,
        playhead: 0.0, playing: false, play_started: Instant::now(),
        zoom: 90.0, scroll: 0.0, snap: true, in_point: 0.0, out_point: None,
        interpolation: StudioInterpolation::Smooth, lane_models: vec![String::new()], drag: None,
    }}
}
impl StudioTimeline {
    fn primary_item(&self) -> Option<StudioItem> {
        self.selected_motion.map(|(i,k)|StudioItem::Motion(i,k))
            .or(self.selected_camera.map(StudioItem::Camera))
            .or(self.selected_clip.map(StudioItem::Clip))
    }
    pub fn item_time(&self,item:StudioItem) -> Option<f64> {
        match item {
            StudioItem::Clip(i)=>self.clips.get(i).map(|clip|clip.start),
            StudioItem::Camera(i)=>self.camera.get(i).map(|key|key.time),
            StudioItem::Motion(i,k)=>self.clips.get(i).and_then(|clip|clip.motion.get(k)
                .map(|key|clip.start+key.time-(clip.first_frame-clip.source_first) as f64/clip.source_fps.max(1.0))),
        }
    }
    pub fn selected_items(&self) -> Vec<StudioItem> {
        let Some(primary)=self.primary_item() else {return Vec::new()};
        if self.selection.contains(&primary) {
            self.selection.iter().copied().filter(|&item|self.item_time(item).is_some()).collect()
        } else {self.item_time(primary).map(|_|vec![primary]).unwrap_or_default()}
    }
    pub fn is_selected(&self,item:StudioItem) -> bool {
        let Some(primary)=self.primary_item() else {return false;};
        self.item_time(item).is_some() && if self.selection.contains(&primary) {self.selection.contains(&item)}else{primary==item}
    }
    fn focus_item(&mut self,item:Option<StudioItem>) {
        self.selected_clip=None;self.selected_camera=None;self.selected_motion=None;
        match item {
            Some(StudioItem::Clip(i))=>self.selected_clip=Some(i),
            Some(StudioItem::Camera(i))=>self.selected_camera=Some(i),
            Some(StudioItem::Motion(i,k))=>{self.selected_clip=Some(i);self.selected_motion=Some((i,k));}
            None=>{}
        }
    }
    pub fn clear_selection(&mut self) {
        self.selection.clear();self.selection_anchor=None;self.focus_item(None);
    }
    pub fn set_selection(&mut self,items:Vec<StudioItem>) {
        self.clear_selection();
        for item in items {
            if self.item_time(item).is_some() && !self.selection.contains(&item) {self.selection.push(item);}
        }
        self.selection_anchor=self.selection.first().copied();
        self.focus_item(self.selection.last().copied());
    }
    pub fn select_item(&mut self,item:StudioItem,ctrl:bool,shift:bool) {
        if self.item_time(item).is_none() {return;}
        let previous=self.selected_items();
        let anchor=if previous.is_empty() {item} else {self.selection_anchor.or(self.primary_item()).unwrap_or(item)};
        let mut selected=if ctrl {previous} else {Vec::new()};
        if shift {
            let a=self.item_time(anchor).unwrap_or_else(||self.item_time(item).unwrap());
            let b=self.item_time(item).unwrap();
            let mut candidates:Vec<_>=(0..self.clips.len()).map(StudioItem::Clip)
                .chain((0..self.camera.len()).map(StudioItem::Camera)).collect();
            for (i,clip) in self.clips.iter().enumerate() {
                for (k,_) in clip.motion.iter().enumerate() {
                    let key=StudioItem::Motion(i,k);
                    if self.item_time(key).is_some_and(|t|t>=clip.start-1e-8 && t<=clip.end()+1e-8) {candidates.push(key);}
                }
            }
            for candidate in candidates {
                if self.item_time(candidate).is_some_and(|t|t>=a.min(b)-1e-8 && t<=a.max(b)+1e-8)
                    && !selected.contains(&candidate) {selected.push(candidate);}
            }
        } else if ctrl && selected.contains(&item) {selected.retain(|&other|other!=item);}
        else {selected.push(item);}
        self.selection=selected;
        self.selection_anchor=Some(if shift {anchor}else{item});
        self.focus_item(if self.selection.contains(&item) {Some(item)}else{self.selection.last().copied()});
    }
    /// Keep the time under the pointer fixed while zooming the timeline.
    pub fn zoom_at(&mut self, offset: f32, wheel: f32) {
        let offset=offset.max(0.0);
        let anchor=self.scroll+offset/self.zoom;
        self.zoom=(self.zoom*(wheel*0.003).clamp(-4.0,4.0).exp()).clamp(24.0,350.0);
        self.scroll=(anchor-offset/self.zoom).max(0.0);
    }
    pub fn duration(&self) -> f64 {
        self.clips.iter().map(StudioAnimationClip::end).fold(
            self.camera.last().map_or(0.1, |k| k.time),f64::max)
            .max(self.out_point.unwrap_or(0.1)).max(0.1)
    }
    /// The background is a scene track; each actor owns an animation and motion subtrack.
    pub fn track_count(&self)->usize {2+self.lanes*2}
    pub fn character_row(lane:usize)->usize {2+lane*2}
    pub fn motion_row(lane:usize)->usize {3+lane*2}
    pub fn lane_for_row(row:usize)->Option<usize> {(row>=2).then_some((row-2)/2)}
    pub fn row_is_character(row:usize)->bool {row>=2 && row%2==0}
    /// A cross-track move places the clip in the closest legal gap on its new lane.
    pub fn move_clip_to_lane(&mut self,index:usize,lane:usize,time:f64)->bool {
        if index>=self.clips.len() || lane>=self.lanes {return false;}
        self.clips[index].lane=lane;
        self.move_clip(index,time);
        true
    }
    pub fn export_range(&self) -> (f64,f64) {
        let end=self.out_point.unwrap_or_else(||self.duration()).max(0.001);
        let start=self.in_point.clamp(0.0,(end-0.001).max(0.0));
        (start,end)
    }
    pub fn set_in(&mut self, time: f64) {
        let (_,end)=self.export_range();
        self.in_point=self.snap_time(time).clamp(0.0,(end-0.001).max(0.0));
    }
    pub fn set_out(&mut self, time: f64) {
        self.out_point=Some(self.snap_time(time).clamp(self.in_point+0.001,36000.0));
    }
    pub fn snap_time(&self, time: f64) -> f64 {
        if self.snap { (time*30.0).round()/30.0 } else { time }
    }
    pub fn current_time(&self, export: Option<(u64,u32)>) -> f64 {
        if let Some((frame,fps))=export {
            let (start,end)=self.export_range();
            return (start+frame as f64/fps.max(1) as f64).min(end);
        }
        if self.playing { (self.playhead+self.play_started.elapsed().as_secs_f64()) % self.duration() }
        else { self.playhead.clamp(0.0,self.duration()) }
    }
    pub fn seek(&mut self, seconds: f64) {
        self.playing=false; self.playhead=seconds.clamp(0.0,self.duration());
    }
    pub fn toggle_play(&mut self) {
        if self.playing { self.playhead=self.current_time(None); self.playing=false; }
        else { self.play_started=Instant::now(); self.playing=true; }
    }
    pub fn add_clip(&mut self, mut clip: StudioAnimationClip) {
        clip.lane=clip.lane.min(self.lanes.saturating_sub(1));
        clip.start=self.snap_time(clip.start).max(0.0);
        // Find a free gap on the chosen lane.
        loop {
            let next=self.clips.iter().filter(|c|c.lane==clip.lane && clip.start<c.end()-1e-8 && clip.end()>c.start+1e-8)
                .map(StudioAnimationClip::end).fold(None,|a:Option<f64>,b|Some(a.map_or(b,|v|v.max(b))));
            if let Some(next)=next { clip.start=next; } else { break; }
        }
        let i=self.clips.len(); self.clips.push(clip);
        self.select_item(StudioItem::Clip(i),false,false);
    }
    /// Preserve array identity while moving: egui IDs and selection do not jump.
    /// Overlapping positions are projected to the nearest available gap.
    pub fn move_clip(&mut self, index: usize, time: f64) {
        if index>=self.clips.len() {return;}
        let desired=self.snap_time(time).max(0.0);
        let duration=self.clips[index].duration();
        let lane=self.clips[index].lane;
        let mut obstacles:Vec<(f64,f64)>=self.clips.iter().enumerate()
            .filter(|(i,c)|*i!=index && c.lane==lane).map(|(_,c)|(c.start,c.end())).collect();
        obstacles.sort_by(|a,b|a.0.total_cmp(&b.0));
        let mut begin=0.0; let mut best=None::<(f64,f64)>;
        for (start,end) in obstacles.into_iter().chain(std::iter::once((f64::INFINITY,f64::INFINITY))) {
            if start-begin>=duration-1e-8 {
                let place=desired.clamp(begin,(start-duration).max(begin));
                let distance=(place-desired).abs();
                if best.is_none_or(|(_,old)|distance<old) { best=Some((place,distance)); }
            }
            begin=begin.max(end);
        }
        if let Some((position,_))=best {self.clips[index].start=position;}
    }
    pub fn trim_clip(&mut self, i:usize, kind:StudioDragKind, initial_start:f64,
        initial_first:i32, initial_end:i32, seconds:f64) {
        let Some(c)=self.clips.get(i) else {return};
        let fps=c.source_fps.max(1.0);
        let delta=(seconds*fps).round() as i32;
        let lane=c.lane;
        let prev_end=self.clips.iter().enumerate().filter(|(j,p)|*j!=i&&p.lane==lane&&p.end()<=initial_start+1e-8)
            .map(|(_, clip)| clip.end()).fold(0.0,f64::max);
        let next_start=self.clips.iter().enumerate().filter(|(j,p)|*j!=i&&p.lane==lane&&p.start>=initial_start+(initial_end-initial_first) as f64/fps-1e-8)
            .map(|(_,c)|c.start).fold(f64::INFINITY,f64::min);
        let c=&mut self.clips[i];
        match kind {
            StudioDragKind::TrimLeft => {
                let min_collision=initial_first+((prev_end-initial_start)*fps).ceil() as i32;
                let latest=c.end_frame-1;
                let earliest=c.source_first.max(min_collision).min(latest);
                let frame=(initial_first+delta).clamp(earliest,latest);
                c.first_frame=frame;
                c.start=initial_start+(frame-initial_first) as f64/fps;
                // Motion times are source-relative: trimming never destroys
                // or shifts keys. Extending the edge restores original motion.
            }
            StudioDragKind::TrimRight => {
                let max_collision=if next_start.is_finite() {
                    initial_first+((next_start-initial_start)*fps).floor() as i32
                } else {i32::MAX/4};
                let requested=(initial_end+delta).clamp(initial_first+1,max_collision.max(initial_first+1));
                c.end_frame=requested.min(c.source_end).max(initial_first+1);
                c.tail_frames=(requested-c.end_frame).max(0);
            }
            _=>{}
        }
    }
    /// Split on a source frame boundary and preserve motion interpolation.
    pub fn split_clip(&mut self, i:usize, time:f64) -> bool {
        let Some(c)=self.clips.get(i).cloned() else {return false};
        if time<=c.start+1.0/c.source_fps || time>=c.start+c.source_duration()-1.0/c.source_fps {return false;}
        let frame=(c.first_frame+((time-c.start)*c.source_fps).round() as i32)
            .clamp(c.first_frame+1,c.end_frame-1);
        let cut_time=c.start+(frame-c.first_frame) as f64/c.source_fps;
        let cut_offset=(frame-c.source_first) as f64/c.source_fps;
        let center=c.motion_at(cut_time,self.interpolation==StudioInterpolation::Smooth);
        let mut left=c.clone(); let mut right=c;
        left.end_frame=frame; left.tail_frames=0;
        right.first_frame=frame; right.start=cut_time;
        left.motion.retain(|k|k.time<cut_offset);
        left.motion.push(StudioMotionKey {time:cut_offset,..center});
        right.motion.retain(|k|k.time>cut_offset);
        right.motion.insert(0,StudioMotionKey {time:cut_offset,..center});
        self.clips[i]=left;
        let j=self.clips.len(); self.clips.push(right);
        self.select_item(StudioItem::Clip(j),false,false);
        true
    }
    pub fn duplicate_clip(&mut self,i:usize) {
        let Some(mut copy)=self.clips.get(i).cloned() else {return};
        copy.start=copy.end(); self.add_clip(copy);
    }
    pub fn active_actors(&self,time:f64) -> Vec<&StudioAnimationClip> {
        (0..self.lanes).filter_map(|lane| self.clips.iter()
            .find(|c|c.lane==lane && time>=c.start && time<c.end())).collect()
    }
    pub fn clip_at_lane(&self,time:f64,lane:usize) -> Option<&StudioAnimationClip> {
        self.clips.iter().find(|c| c.lane==lane && time>=c.start && time<c.end())
    }
    pub fn delete_selection(&mut self) -> bool {
        let mut items=self.selected_items();
        if items.is_empty() {return false;}
        // Remove keys first, then clips, descending so indices remain valid.
        items.sort();items.dedup();
        for item in items.into_iter().rev() {
            match item {
                StudioItem::Motion(i,k)=>{if let Some(clip)=self.clips.get_mut(i) {if k<clip.motion.len(){clip.motion.remove(k);}}}
                StudioItem::Camera(i)=>{if i<self.camera.len(){self.camera.remove(i);}}
                StudioItem::Clip(i)=>{if i<self.clips.len(){self.clips.remove(i);}}
            }
        }
        self.clear_selection();self.drag=None;true
    }
    pub fn clip_at(&self,time:f64)->Option<&StudioAnimationClip> {
        // Compatibility with single-actor callers; the multi-actor renderer
        // evaluates active_actors() instead of picking a highest-priority lane.
        self.clip_at_lane(time,0)
    }
    pub fn add_camera_key(&mut self,mut key:StudioCameraKey) {
        key.time=self.snap_time(key.time.max(0.0));
        self.insert_camera_key(key);
    }
    /// Begin navigation from exactly the camera currently displayed. Selecting
    /// an earlier key must not rewind the playhead or restore its old pose.
    pub fn begin_camera_edit(&mut self) {
        let time=self.current_time(None);
        let mut key=self.camera_at(time);
        key.time=time;
        self.seek(time);
        // Navigation must not quantize an interpolated view to another time.
        self.insert_camera_key(key);
    }
    pub fn insert_camera_key(&mut self,key:StudioCameraKey) {
        if let Some(i)=self.camera.iter().position(|k|(k.time-key.time).abs()<1e-5) {
            self.camera[i]=key; self.selected_camera=Some(i);
        } else {
            self.camera.push(key);self.camera.sort_by(|a,b|a.time.total_cmp(&b.time));
            self.selected_camera=self.camera.iter().position(|k|(k.time-key.time).abs()<1e-5);
        }
        self.selected_clip=None;self.selected_motion=None;
        if let Some(i)=self.selected_camera {self.select_item(StudioItem::Camera(i),false,false);}
    }
    pub fn camera_at(&self,time:f64)->StudioCameraKey {
        let mut key=self.camera_transform_at(time);key.rig_orbit=self.orbit_at(time);key
    }
    fn orbit_at(&self,time:f64)->Option<StudioCameraOrbit> {
        let keys:Vec<_>=self.camera.iter().filter_map(|k|k.rig_orbit.map(|o|(k.time,o))).collect();
        let &(first_time,first)=keys.first()?;
        if time<=first_time {return Some(first);}
        let angle=|a:f32,b:f32|(b-a+180.).rem_euclid(360.)-180.;
        for (i,pair) in keys.windows(2).enumerate() {
            let (ta,a)=pair[0];let (tb,b)=pair[1];if time>tb {continue;}
            let (tp,p)=if i>0 {keys[i-1]}else{pair[0]};
            let (tn,n)=keys.get(i+2).copied().unwrap_or(pair[1]);
            let u=((time-ta)/(tb-ta).max(1e-8)).clamp(0.,1.) as f32;
            let mix=|p:f32,a:f32,b:f32,n:f32| {
                if self.interpolation==StudioInterpolation::Spline {
                    let u2=u*u;let u3=u2*u;let dt=(tb-ta) as f32;
                    (2.*u3-3.*u2+1.)*a+(u3-2.*u2+u)*dt*(b-p)/(tb-tp).max(1e-6) as f32
                        +(-2.*u3+3.*u2)*b+(u3-u2)*dt*(n-a)/(tn-ta).max(1e-6) as f32
                } else {let t=if self.interpolation==StudioInterpolation::Smooth {u*u*(3.-2.*u)}else{u};a+(b-a)*t}
            };
            let by=a.yaw+angle(a.yaw,b.yaw);
            return Some(StudioCameraOrbit {
                distance:mix(p.distance,a.distance,b.distance,n.distance).max(0.),
                yaw:(mix(a.yaw-angle(p.yaw,a.yaw),a.yaw,by,by+angle(b.yaw,n.yaw))+180.).rem_euclid(360.)-180.,
                pitch:mix(p.pitch,a.pitch,b.pitch,n.pitch).clamp(-90.,90.),
            });
        }
        keys.last().map(|(_,orbit)|*orbit)
    }
    fn camera_transform_at(&self,time:f64)->StudioCameraKey {
        let Some(first)=self.camera.first().copied() else {return StudioCameraKey::default()};
        if time<=first.time {return first;}
        for (i,keys) in self.camera.windows(2).enumerate() {
            let (a,b)=(keys[0],keys[1]);
            if time<=b.time {
                let mut t=((time-a.time)/(b.time-a.time).max(1e-8)).clamp(0.0,1.0) as f32;
                if self.interpolation==StudioInterpolation::Smooth { t=t*t*(3.0-2.0*t); }
                let mix=|x:f32,y:f32|x+(y-x)*t;
                let dyaw=(b.yaw-a.yaw+180.0).rem_euclid(360.0)-180.0;
                // Time-aware cubic Hermite avoids the stop/start of independent
                // smoothstep segments and produces a curved camera path.
                let position=if self.interpolation==StudioInterpolation::Spline {
                    let prev=self.camera.get(i.wrapping_sub(1)).filter(|_|i>0).copied().unwrap_or(a);
                    let next=self.camera.get(i+2).copied().unwrap_or(b);
                    let dt=(b.time-a.time).max(1e-6) as f32;
                    let u=((time-a.time)/(b.time-a.time).max(1e-8)).clamp(0.0,1.0) as f32;
                    let u2=u*u;let u3=u2*u;
                    let h00=2.0*u3-3.0*u2+1.0;
                    let h10=u3-2.0*u2+u;
                    let h01=-2.0*u3+3.0*u2;
                    let h11=u3-u2;
                    std::array::from_fn(|axis| {
                        let m0=(b.position[axis]-prev.position[axis])/
                            (b.time-prev.time).max(1e-6) as f32;
                        let m1=(next.position[axis]-a.position[axis])/
                            (next.time-a.time).max(1e-6) as f32;
                        h00*a.position[axis]+h10*dt*m0+h01*b.position[axis]+h11*dt*m1
                    })
                } else {std::array::from_fn(|axis|mix(a.position[axis],b.position[axis]))};
                return StudioCameraKey {time,position,
                    yaw:a.yaw+dyaw*t,pitch:mix(a.pitch,b.pitch),fov:mix(a.fov,b.fov),rig_orbit:None};
            }
        }
        *self.camera.last().unwrap_or(&first)
    }
    pub fn move_camera_key(&mut self,index:usize,time:f64) {
        if index>=self.camera.len() {return;}
        let minimum=if index==0 {0.0} else {self.camera[index-1].time+1.0/30.0};
        let maximum=self.camera.get(index+1).map_or(36000.0,|k|k.time-1.0/30.0);
        self.camera[index].time=self.snap_time(time).clamp(minimum,maximum.max(minimum));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn clip(first:i32,end:i32,start:f64)->StudioAnimationClip {
        StudioAnimationClip{name:"BOTH_STAND1".into(),emote:None,start,first_frame:first,end_frame:end,
            source_fps:20.0,lane:0,source_first:0,source_end:100,motion:vec![StudioMotionKey::default()],tail_frames:0,playback:StudioClipPlayback::Hold}
    }
    #[test] fn ctrl_toggle_and_shift_select_by_time_across_tracks() {
        let mut timeline=StudioTimeline::default();
        timeline.clips=vec![clip(0,20,4.0),clip(0,20,1.0)];
        timeline.clips[1].motion[0].time=0.5;
        timeline.camera=vec![StudioCameraKey::default(),StudioCameraKey {time:2.0,..Default::default()},StudioCameraKey {time:5.0,..Default::default()}];
        timeline.select_item(StudioItem::Clip(1),false,false);
        timeline.select_item(StudioItem::Camera(1),true,false);
        assert_eq!(timeline.selected_items().len(),2);
        timeline.select_item(StudioItem::Camera(1),true,false);
        assert_eq!(timeline.selected_items(),vec![StudioItem::Clip(1)]);
        timeline.select_item(StudioItem::Clip(1),false,false);
        timeline.select_item(StudioItem::Clip(0),false,true);
        assert_eq!(timeline.selected_items().len(),5);
        assert!(timeline.is_selected(StudioItem::Motion(1,0)));
        assert!(timeline.is_selected(StudioItem::Camera(1)));
        assert!(!timeline.is_selected(StudioItem::Camera(0)));
        assert!(!timeline.is_selected(StudioItem::Camera(2)));
        timeline.select_item(StudioItem::Camera(0),false,true);
        assert_eq!(timeline.selected_items().len(),2); // Original range anchor stays at 1s.
        timeline.select_item(StudioItem::Clip(0),false,false);
        timeline.select_item(StudioItem::Clip(0),true,false);
        assert!(timeline.selected_items().is_empty());
    }
    #[test] fn group_delete_keeps_unselected_items_and_indices_intact() {
        let mut timeline=StudioTimeline::default();
        timeline.clips=vec![clip(0,20,0.0),clip(0,20,2.0),clip(0,20,4.0)];
        timeline.clips[1].motion=vec![StudioMotionKey::default(),StudioMotionKey {time:0.25,..Default::default()},StudioMotionKey {time:0.5,..Default::default()}];
        timeline.camera=(0..3).map(|i|StudioCameraKey {time:i as f64,..Default::default()}).collect();
        timeline.set_selection(vec![StudioItem::Clip(0),StudioItem::Clip(2),StudioItem::Camera(0),StudioItem::Camera(2),StudioItem::Motion(1,0),StudioItem::Motion(1,2)]);
        assert!(timeline.delete_selection());
        assert_eq!(timeline.clips.len(),1);
        assert_eq!(timeline.clips[0].start,2.0);
        assert_eq!(timeline.clips[0].motion.len(),1);
        assert_eq!(timeline.clips[0].motion[0].time,0.25);
        assert_eq!(timeline.camera.len(),1);
        assert_eq!(timeline.camera[0].time,1.0);
        assert!(timeline.selected_items().is_empty());
        assert!(!timeline.delete_selection());
    }
    #[test] fn orbit_keeps_actor_centered_at_fixed_distance() {
        let target=[110.0,24.0,16.0];
        let mut camera=StudioCameraKey::default();
        let offset=glam::Vec3::from_array(target)-glam::Vec3::from_array(camera.position);
        camera.yaw=(-offset.z).atan2(offset.x).to_degrees();
        camera.pitch=offset.y.atan2(offset.x.hypot(offset.z)).to_degrees();
        let radius=offset.length();
        let original=camera.position;
        for delta in [[120.0,60.0],[-40.0,-2000.0],[800.0,4000.0]] {
            camera.orbit(target,delta);
            let offset=glam::Vec3::from_array(target)-glam::Vec3::from_array(camera.position);
            assert!((offset.length()-radius).abs()<0.001);
            let (yaw,pitch)=(camera.yaw.to_radians(),camera.pitch.to_radians());
            let forward=glam::Vec3::new(yaw.cos()*pitch.cos(),pitch.sin(),-yaw.sin()*pitch.cos());
            assert!(forward.dot(offset.normalize())>0.9999);
            assert!(camera.pitch.abs()<=89.0);
        }
        assert_ne!(camera.position,original);
    }
    #[test] fn orbit_after_pan_preserves_current_distance_angle_and_framing() {
        fn framing(camera:&StudioCameraKey,target:[f32;3])->glam::Vec3 {
            let offset=glam::Vec3::from_array(target)-glam::Vec3::from_array(camera.position);
            let (yaw,pitch)=(camera.yaw.to_radians(),camera.pitch.to_radians());
            let forward=glam::Vec3::new(yaw.cos()*pitch.cos(),pitch.sin(),-yaw.sin()*pitch.cos());
            let right=glam::Vec3::new(yaw.sin(),0.0,yaw.cos());
            glam::Vec3::new(offset.dot(right),offset.dot(right.cross(forward)),offset.dot(forward))
        }
        let target=[88.0,18.0,-22.0];
        let mut camera=StudioCameraKey {position:[-140.0,95.0,180.0],yaw:17.0,pitch:-12.0,..Default::default()};
        let original=camera;
        camera.orbit(target,[0.0,0.0]);
        assert_eq!(camera.position,original.position);
        assert_eq!(camera.yaw,original.yaw);
        assert_eq!(camera.pitch,original.pitch);
        let before=framing(&camera,target);
        camera.orbit(target,[0.1,-0.1]);
        assert!((glam::Vec3::from_array(camera.position)-glam::Vec3::from_array(original.position)).length()<0.5);
        assert!((camera.yaw-17.025).abs()<0.0001);
        assert!((camera.pitch+11.975).abs()<0.0001);
        assert!((framing(&camera,target)-before).length()<0.001);
        // Pan/dolly farther, release, and resume orbit from that new view.
        camera.position[0]-=250.0;camera.position[1]+=80.0;camera.position[2]+=120.0;
        let before=framing(&camera,target);
        for delta in [[120.0,80.0],[-60.0,-160.0],[0.0,0.0]] {
            camera.orbit(target,delta);
            assert!((framing(&camera,target)-before).length()<0.002);
        }
    }
    #[test] fn camera_navigation_starts_at_displayed_view_without_rewinding_or_snapping() {
        let mut timeline=StudioTimeline::default();
        timeline.camera.push(StudioCameraKey {time:4.0,position:[40.0,20.0,80.0],yaw:90.0,pitch:25.0,..Default::default()});
        timeline.selected_camera=Some(0);
        timeline.seek(1.2345);
        let displayed=timeline.camera_at(timeline.playhead);
        timeline.begin_camera_edit();
        let index=timeline.selected_camera.unwrap();
        assert_eq!(timeline.playhead,1.2345);
        assert_eq!(timeline.camera[index].time,1.2345);
        assert_eq!(timeline.camera[index].position,displayed.position);
        assert_eq!(timeline.camera[index].yaw,displayed.yaw);
        assert_eq!(timeline.camera[index].pitch,displayed.pitch);
        timeline.camera[index].position=[-120.0,45.0,210.0];
        timeline.begin_camera_edit();
        assert_eq!(timeline.camera.len(),3);
        assert_eq!(timeline.camera[timeline.selected_camera.unwrap()].position,[-120.0,45.0,210.0]);
    }
    #[test] fn timeline_zoom_preserves_pointer_time_and_clamps() {
        let mut timeline=StudioTimeline::default();
        timeline.scroll=3.0;
        let offset=420.0;
        let anchor=timeline.scroll+offset/timeline.zoom;
        timeline.zoom_at(offset,120.0);
        assert!(timeline.zoom>90.0);
        assert!((timeline.scroll+offset/timeline.zoom-anchor).abs()<0.0001);
        timeline.zoom_at(offset,-120.0);
        assert!((timeline.zoom-90.0).abs()<0.0001);
        timeline.zoom_at(0.0,10000.0);
        assert_eq!(timeline.zoom,350.0);
        timeline.zoom_at(0.0,-10000.0);
        assert_eq!(timeline.zoom,24.0);
        assert!(timeline.scroll>=0.0);
    }
    #[test] fn interpolation_switch() {
        let mut tl=StudioTimeline::default();
        tl.camera=vec![StudioCameraKey {yaw:350.0,..Default::default()},
            StudioCameraKey {time:2.0,yaw:10.0,position:[10.,0.,0.],..Default::default()}];
        assert!((tl.camera_at(1.0).yaw-360.0).abs()<0.01);
        tl.interpolation=StudioInterpolation::Linear;
        assert!((tl.camera_at(0.5).position[0]-2.5).abs()<0.01);
    }
    #[test] fn trim_keeps_endpoint_and_source_frame() {
        let mut tl=StudioTimeline::default();tl.add_clip(clip(0,40,0.0));
        tl.trim_clip(0,StudioDragKind::TrimLeft,0.0,0,40,0.5);
        assert_eq!(tl.clips[0].first_frame,10);
        assert!((tl.clips[0].end()-2.0).abs()<1e-6);
    }
    #[test] fn split_is_contiguous() {
        let mut tl=StudioTimeline::default();tl.add_clip(clip(0,40,0.0));
        assert!(tl.split_clip(0,1.0));assert_eq!(tl.clips.len(),2);
        assert_eq!(tl.clips[0].end_frame,tl.clips[1].first_frame);
        assert!((tl.clips[0].end()-tl.clips[1].start).abs()<1e-6);
    }
    #[test] fn drag_can_cross_neighbor_without_reindexing() {
        let mut tl=StudioTimeline::default();tl.snap=false;
        tl.add_clip(clip(0,20,0.0));tl.add_clip(clip(0,20,1.0));
        tl.move_clip(0,4.0);
        assert_eq!(tl.clips[0].start,4.0);
        assert_eq!(tl.clips[1].start,1.0);
    }
    #[test] fn source_extension_hold_loop_and_trim() {
        let mut tl=StudioTimeline::default();tl.add_clip(clip(0,20,0.0));
        tl.trim_clip(0,StudioDragKind::TrimRight,0.0,0,20,2.0);
        assert_eq!(tl.clips[0].end_frame,60);
        assert_eq!(tl.clips[0].tail_frames,0);
        tl.trim_clip(0,StudioDragKind::TrimRight,0.0,0,60,3.0);
        assert_eq!(tl.clips[0].end_frame,100);
        assert_eq!(tl.clips[0].tail_frames,20);
        let c=&mut tl.clips[0];
        let held=c.pose_time_ms(5.5);
        assert_eq!(held,c.pose_time_ms(5.0));
        c.playback=StudioClipPlayback::Loop;
        assert!(c.pose_time_ms(5.5)<held);
    }
    #[test] fn first_camera_key_is_removable_and_retimable() {
        let mut tl=StudioTimeline::default();
        tl.move_camera_key(0,0.5);
        assert!((tl.camera[0].time-0.5).abs()<1e-6);
        tl.selected_camera=Some(0);
        assert!(tl.delete_selection());
        assert!(tl.camera.is_empty());
        assert_eq!(tl.camera_at(0.0).fov,58.0);
    }
    #[test] fn all_active_lanes_render() {
        let mut tl=StudioTimeline::default();tl.lanes=2;
        tl.add_clip(clip(0,20,0.0));
        let mut c=clip(0,20,0.0);c.lane=1;tl.add_clip(c);
        assert_eq!(tl.active_actors(0.5).len(),2);
        assert_eq!(tl.track_count(),6);
        assert_eq!(StudioTimeline::character_row(1),4);
        assert_eq!(StudioTimeline::motion_row(1),5);
        assert!(tl.move_clip_to_lane(0,1,3.0));
        assert_eq!(tl.clips[0].lane,1);
        assert_eq!(tl.active_actors(0.5).len(),1);
    }
    #[test] fn export_range_offsets_exact_frames() {
        let mut tl=StudioTimeline::default();tl.in_point=2.0;tl.out_point=Some(3.0);
        assert!((tl.current_time(Some((15,30)))-2.5).abs()<1e-6);
    }
}
