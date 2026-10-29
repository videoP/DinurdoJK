//! Camera position and aim/focus endpoints in the independent Scene view.
use super::{App,render_studio_ui::{self as sui,palette as theme}};
use crate::app::{render_studio_spatial::{StudioSpatialDrag,camera_from_key},
    render_studio_timeline::{StudioCameraKey,StudioCameraSource,StudioDemoPov}};
use glam::Vec3;

#[derive(Clone,Copy)]
enum Owner { Key(usize), Director(usize) }
#[derive(Clone)]
pub(in crate::app) struct StudioCameraHandleDrag {
    owner:Owner,target:bool,spatial:StudioSpatialDrag,key:StudioCameraKey,eye:Vec3,aim:Vec3,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{render_studio_camera::StudioCameraRig,render_studio_timeline::{StudioTimeline,StudioItem},
        render_studio_spatial::translate_item};
    #[test]
    fn target_drives_aim_and_focus_after_position_and_rig_changes() {
        let mut key=StudioCameraKey{position:[0.0;3],focus_target:Some([100.0,80.0,-40.0]),..Default::default()};
        key.lens.aperture=Some(2.8);key.resolve_focus_target();
        let target=Vec3::from_array(key.focus_target.unwrap());
        assert!((key.lens.focus_distance-target.length()).abs()<1e-4);
        assert!(camera_from_key(key).forward().dot(target.normalize())>0.99999);
        let mut timeline=StudioTimeline::default();timeline.camera=vec![key];
        translate_item(&mut timeline,StudioItem::Camera(0),key.position,Vec3::new(20.0,10.0,0.0));
        let moved=timeline.camera[0];
        assert_eq!(moved.focus_target,key.focus_target);
        assert!((moved.lens.focus_distance-Vec3::from_array(moved.position).distance(target)).abs()<1e-4);
        let mut rig=StudioCameraRig::default();rig.position.enabled=true;rig.target.enabled=true;
        let resolved=rig.resolve(moved,Some([40.0,20.0,0.0]),Some([-100.0,0.0,0.0]));
        let delta=target-Vec3::from_array(resolved.position);
        assert!((resolved.lens.focus_distance-delta.length()).abs()<1e-4);
        assert!(camera_from_key(resolved).forward().dot(delta.normalize())>0.99999);
        assert_eq!(resolved.lens.aperture,Some(2.8));
    }
    #[test]
    fn target_keys_interpolate_persist_and_default_for_legacy_projects() {
        let mut timeline=StudioTimeline::default();
        timeline.camera=vec![StudioCameraKey{position:[0.0;3],focus_target:Some([100.0,0.0,0.0]),..Default::default()},
            StudioCameraKey{time:2.0,position:[20.0,0.0,0.0],focus_target:Some([100.0,100.0,0.0]),..Default::default()}];
        let key=timeline.camera_at(1.0);
        assert_eq!(key.focus_target,Some([100.0,50.0,0.0]));
        assert!((key.lens.focus_distance-Vec3::new(90.0,50.0,0.0).length()).abs()<1e-4);
        assert!(key.lens.aperture.is_none());
        let mut json=serde_json::to_value(key).unwrap();
        let loaded:StudioCameraKey=serde_json::from_value(json.clone()).unwrap();assert_eq!(loaded.focus_target,key.focus_target);
        json.as_object_mut().unwrap().remove("focus_target");
        assert!(serde_json::from_value::<StudioCameraKey>(json).unwrap().focus_target.is_none());
    }
    #[test]
    fn director_manual_handles_override_only_the_selected_shot() {
        use crate::app::render_studio_director::{StudioDirectorShot,StudioDirectorPreset};
        let mut shot=StudioDirectorShot::new(1,StudioDirectorPreset::RecordedPov,-1,0.0,4.0);
        shot.position_override=Some([10.0,20.0,30.0]);shot.focus_target=Some([100.0,20.0,30.0]);
        let key=shot.camera_key(1.0,StudioCameraKey::default());
        assert!(matches!(key.source,StudioCameraSource::Demo{pov:StudioDemoPov::Free,..}));
        assert_eq!(key.position,[10.0,20.0,30.0]);assert_eq!(key.lens.focus_distance,90.0);
        assert!(shot.valid());
        let copy:StudioDirectorShot=serde_json::from_str(&serde_json::to_string(&shot).unwrap()).unwrap();
        assert_eq!(copy,shot);
    }
}

impl App {
    pub(super) fn egui_studio_camera_handles(&mut self,ui:&mut egui::Ui,viewport:egui::Rect,locked:bool)->bool {
        if locked || self.render_studio.spatial.scene_camera.is_none() || !self.render_studio.spatial.show_keys {
            self.render_studio.spatial.camera_handle_drag=None;return false;
        }
        let owner=self.studio_director_index().map(Owner::Director)
            .or_else(||self.render_studio.timeline.selected_camera.map(Owner::Key));
        let Some(owner)=owner else{return false};
        let time=self.render_studio.timeline_time();
        let key=match owner {
            Owner::Key(i)=>{let Some(key)=self.render_studio.timeline.camera.get(i) else{return false};*key},
            Owner::Director(i)=>self.render_studio.timeline.director_shots[i].camera_key(time,self.render_studio.timeline.camera_at(time)),
        };
        let demo=matches!(key.source,StudioCameraSource::Demo{..});
        let mut shot=if matches!(owner,Owner::Key(_)) && matches!(key.source,StudioCameraSource::Demo{pov:StudioDemoPov::Free,..}) {
            camera_from_key(key)
        } else if demo {
            self.render_studio.scene_frame.as_ref().map_or_else(||camera_from_key(key),|f|f.outgoing.shot_camera)
        } else {
            let resolved=self.render_studio.camera_rig_runtime.camera.filter(|_|self.render_studio.timeline.camera_rig.active()).unwrap_or(key);
            let mut camera=camera_from_key(resolved);
            if let Owner::Key(i)=owner {
                if let Some(marker)=self.render_studio.spatial.markers.iter().find(|m|m.item==crate::app::render_studio_timeline::StudioItem::Camera(i)) {
                    camera.position=Vec3::from_array(marker.position);
                }
            }
            camera
        };
        let lens=key.lens_for_camera(&mut shot);
        let eye=shot.position;
        let aim=key.focus_target.map(Vec3::from_array).unwrap_or(eye+shot.forward()*lens.focus_distance);
        let observer=self.asset_preview_camera();
        let project=|point:Vec3|observer.project_to_screen(viewport.width().max(1.0) as u32,viewport.height().max(1.0) as u32,point)
            .map(|p|viewport.min+p.to_vec2());
        let painter=ui.painter().with_clip_rect(viewport);
        if let (Some(a),Some(b))=(project(eye),project(aim)) {
            painter.line_segment([a,b],egui::Stroke::new(1.5,sui::CAMERA));
        }
        let mut busy=false;
        for (target,origin,label,tint) in [(false,eye,"Camera position",sui::CAMERA),(true,aim,"Target / focus",theme::ACCENT)] {
            let Some(center)=project(origin).filter(|p|viewport.shrink(12.0).contains(*p)) else{continue};
            painter.circle_stroke(center,8.0,egui::Stroke::new(2.0,tint));
            painter.line_segment([center-egui::vec2(11.0,0.0),center+egui::vec2(11.0,0.0)],egui::Stroke::new(1.0,tint));
            painter.line_segment([center-egui::vec2(0.0,11.0),center+egui::vec2(0.0,11.0)],egui::Stroke::new(1.0,tint));
            painter.text(center+egui::vec2(12.0,12.0),egui::Align2::LEFT_TOP,label,egui::FontId::proportional(11.0),tint);
            for (axis,point,color) in std::iter::once((None,origin,tint)).chain(
                [Vec3::X,Vec3::Y,Vec3::Z].into_iter().enumerate().map(|(axis,direction)| {
                    let scale=(origin-observer.position).dot(observer.forward()).max(1.0)
                        *2.0*(observer.cg_fov().to_radians()*0.5).tan()*0.75/viewport.height().max(1.0);
                    (Some(axis),origin+direction*scale*42.0,[egui::Color32::LIGHT_RED,egui::Color32::LIGHT_GREEN,egui::Color32::LIGHT_BLUE][axis])
                })) {
                let Some(point)=project(point).filter(|p|viewport.shrink(8.0).contains(*p)) else{continue};
                if axis.is_some() {if center.distance(point)<14.0 {continue;}
                    painter.line_segment([center,point],egui::Stroke::new(1.2,color));painter.circle_filled(point,4.0,color);}
                let r=ui.interact(egui::Rect::from_center_size(point,egui::Vec2::splat(if axis.is_some(){13.0}else{22.0})),
                    ui.make_persistent_id(("studio_camera_endpoint",target,axis)),egui::Sense::click_and_drag());
                busy|=r.hovered()||r.is_pointer_button_down_on();
                if r.drag_started() {
                    self.render_studio.timeline.playing=false;
                    let mut authored=key;
                    if demo {authored.position=eye.to_array();authored.yaw=shot.yaw.to_degrees();authored.pitch=shot.pitch.to_degrees();authored.fov=shot.cg_fov();}
                    self.render_studio.spatial.camera_handle_drag=Some(StudioCameraHandleDrag{owner,target,key:authored,eye,aim,
                        spatial:StudioSpatialDrag{camera:observer,viewport,origin,start_pointer:ui.ctx().input(|i|i.pointer.press_origin()).unwrap_or(point),axis,items:vec![]}});
                }
                r.on_hover_text(if target {"Drag the aim and focus point. Small X/Y/Z dots constrain movement to an axis."}
                    else {"Drag camera position; the target stays fixed. Small X/Y/Z dots constrain movement to an axis."});
            }
        }
        if let Some(drag)=self.render_studio.spatial.camera_handle_drag.clone() {
            busy=true;
            if let Some(delta)=ui.ctx().input(|i|i.pointer.interact_pos()).and_then(|p|drag.spatial.delta(p)) {
                let eye=if drag.target {drag.eye}else{(drag.eye+delta).clamp(Vec3::splat(-50000.0),Vec3::splat(50000.0))};
                let target=(if drag.target {drag.aim+delta}else{drag.aim}).clamp(Vec3::splat(-1_000_000.0),Vec3::splat(1_000_000.0));
                match drag.owner {
                    Owner::Key(i)=>if let Some(key)=self.render_studio.timeline.camera.get_mut(i) {
                        *key=drag.key;key.focus_target=Some(target.to_array());
                        if !drag.target {
                            if matches!(key.source,StudioCameraSource::Demo{..}) {
                                key.source=StudioCameraSource::Demo{pov:StudioDemoPov::Free,client:-1};key.position=eye.to_array();
                            } else {key.position=(Vec3::from_array(drag.key.position)+eye-drag.eye).to_array();}
                        }
                        key.resolve_focus_target();key.lens.focus_distance=eye.distance(target).clamp(1.0,65536.0);
                    },
                    Owner::Director(i)=>if let Some(shot)=self.render_studio.timeline.director_shots.get_mut(i) {
                        shot.focus_target=Some(target.to_array());shot.rig.autofocus=false;shot.lens.focus_distance=eye.distance(target).clamp(1.0,65536.0);
                        if !drag.target {shot.position_override=Some(eye.to_array());}
                    },
                }
                self.profile_preview_key=None;ui.ctx().request_repaint();
            }
            if !ui.ctx().input(|i|i.pointer.primary_down()) {self.render_studio.spatial.camera_handle_drag=None;}
        }
        busy
    }
}
