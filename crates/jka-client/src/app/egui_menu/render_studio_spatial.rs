//! Editor-only projected keyframe markers and translation handles.
use super::App;
use super::render_studio_ui::palette as theme;
use crate::app::render_studio_spatial::{StudioSpatialDrag,StudioSpatialMarker,StudioMotionPath,
    key_from_camera,stored_position,translate_item};
use glam::Vec3;

fn project(camera:&crate::camera::Camera,viewport:egui::Rect,point:[f32;3])->Option<egui::Pos2> {
    camera.project_to_screen(viewport.width().max(1.) as u32,viewport.height().max(1.) as u32,
        Vec3::from_array(point)).map(|p|viewport.min+p.to_vec2())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spatial_coincident_key_labels_stay_separate_and_in_the_viewport() {
        let viewport=egui::Rect::from_min_size(egui::pos2(250.,100.),egui::vec2(900.,500.));
        for anchor in [viewport.center(),viewport.left_top()+egui::vec2(3.,3.),viewport.right_bottom()-egui::vec2(3.,3.)] {
            let mut occupied=vec![];
            for _ in 0..12 {
                let rect=label_rect(anchor,viewport,&occupied);
                assert!(viewport.contains_rect(rect));
                assert!(!occupied.iter().any(|r:&egui::Rect|r.expand(2.).intersects(rect)));
                occupied.push(rect);
            }
        }
    }
}
pub(super) fn studio_layer_color(lane:usize)->egui::Color32 {
    const COLORS:[[u8;3];16]=[
        [94,224,204],[244,128,191],[120,172,255],[155,227,115],
        [190,148,255],[255,151,118],[211,226,103],[244,108,118],
        [127,230,167],[242,179,110],[221,133,234],[103,205,247],
        [174,199,102],[221,159,164],[154,135,229],[155,220,230],
    ];
    let [r,g,b]=COLORS[lane%COLORS.len()];egui::Color32::from_rgb(r,g,b)
}
fn color(lane:Option<usize>)->egui::Color32 {
    lane.map_or(egui::Color32::from_rgb(244,202,84),studio_layer_color)
}
fn paint_motion_paths<'a>(painter:&egui::Painter,camera:&crate::camera::Camera,viewport:egui::Rect,
    paths:impl Iterator<Item=&'a StudioMotionPath>,occupied:&mut Vec<egui::Rect>) {
    let visible=viewport.shrink2(egui::vec2(5.,5.));
    for path in paths {
        let tint=color(path.lane);
        for (i,points) in path.points.windows(2).enumerate() {
            if path.breaks.binary_search(&(i+1)).is_ok() {continue;}
            if let (Some(a),Some(b))=(project(camera,viewport,points[0]),project(camera,viewport,points[1])) {
                // A subtle dark edge keeps faint cyan readable on the light
                // transparency checkerboard as well as on a rendered scene.
                painter.line_segment([a,b],egui::Stroke::new(2.8,egui::Color32::from_black_alpha(70)));
                painter.line_segment([a,b],egui::Stroke::new(1.4,tint.gamma_multiply(0.65)));
            }
        }
        let mut previous_tick:Option<egui::Pos2>=None;
        for tick in &path.ticks {
            let Some(anchor)=project(camera,viewport,tick.position).filter(|p|visible.contains(*p)) else {continue;};
            // Holds and foreshortened movement can put many seconds in one pixel.
            // Keep the path legible instead of stacking identical ticks and text.
            if previous_tick.is_some_and(|p|p.distance(anchor)<6.) {continue;}
            previous_tick=Some(anchor);
            let tangent=project(camera,viewport,(Vec3::from_array(tick.position)+Vec3::from_array(tick.tangent)).to_array())
                .map_or(egui::Vec2::ZERO,|p|p-anchor).normalized();
            let normal=if tangent.length_sq()>0.5 {egui::vec2(-tangent.y,tangent.x)}else{egui::vec2(0.,1.)};
            painter.line_segment([anchor-normal*4.,anchor+normal*4.],egui::Stroke::new(1.,tint.gamma_multiply(0.65)));
            let galley=painter.layout_no_wrap(format!("{}s",tick.second),egui::FontId::proportional(10.),tint.gamma_multiply(0.8));
            let center=anchor+normal*(8.+galley.size().y*0.5);
            let rect=egui::Rect::from_center_size(center,galley.size());
            if rect.top()<viewport.top()+40. || !visible.contains_rect(rect)
                || occupied.iter().any(|r|r.expand(3.).intersects(rect)) {continue;}
            painter.rect_filled(rect.expand2(egui::vec2(2.,1.)),2.,egui::Color32::from_black_alpha(110));
            painter.galley(rect.min,galley,tint);
            occupied.push(rect);
        }
    }
}
fn label(marker:&StudioSpatialMarker)->String {
    format!("{} #{}",if marker.lane.is_some(){"M"}else{"C"},marker.number)
}
fn label_rect(anchor:egui::Pos2,viewport:egui::Rect,occupied:&[egui::Rect])->egui::Rect {
    let size=egui::vec2(72.,22.);
    let base=egui::pos2(
        (anchor.x+if anchor.x>viewport.center().x {-80.}else{8.})
            .clamp(viewport.left()+4.,(viewport.right()-size.x-4.).max(viewport.left()+4.)),
        (anchor.y-11.).clamp(viewport.top()+42.,(viewport.bottom()-size.y-4.).max(viewport.top()+42.)));
    for n in 0..(occupied.len()*3+24).min(128) {
        let row_index=n/3;
        let row=if row_index%2==0 {(row_index/2) as f32}else{-(((row_index+1)/2) as f32)};
        let column=match n%3 {0=>0.,1=>1.,_=>-1.};
        let min=base+egui::vec2(column*80.,row*28.);
        let min=egui::pos2(min.x.clamp(viewport.left()+4.,(viewport.right()-size.x-4.).max(viewport.left()+4.)),
            min.y.clamp(viewport.top()+42.,(viewport.bottom()-size.y-4.).max(viewport.top()+42.)));
        let rect=egui::Rect::from_min_size(min,size);
        if !occupied.iter().any(|r|r.expand(2.).intersects(rect)) {return rect;}
    }
    egui::Rect::from_min_size(anchor+egui::vec2(8.,-11.),size)
}

impl App {
    /// Returns whether keyframe controls own pointer input this frame.
    pub(super) fn egui_studio_spatial_overlay(&mut self,ui:&mut egui::Ui,viewport:egui::Rect,locked:bool)->bool {
        if locked {self.render_studio.spatial.drag=None;return true;}
        if self.render_studio.cinematic_preview.is_some() {return false;}
        let pointer=ui.ctx().input(|i|i.pointer.interact_pos());
        let mut busy=false;
        if !self.render_studio.spatial.show_keys && !self.render_studio.spatial.show_paths {return busy;}
        self.refresh_studio_spatial_markers();
        if self.render_studio.spatial.paths_pending {ui.ctx().request_repaint();}
        else if let Some(viewport)=self.render_studio.spatial.frame_when_ready.take() {self.frame_studio_spatial_keys(viewport);}
        let markers=self.render_studio.spatial.markers.clone();
        let camera=self.asset_preview_camera();
        let painter=ui.painter().with_clip_rect(viewport);
        // Reserve key labels first so second labels never obscure the controls.
        let mut occupied=Vec::new();
        let labels:Vec<_>=markers.iter().map(|marker| {
            if !self.render_studio.spatial.show_keys {return None;}
            let anchor=project(&camera,viewport,marker.position).filter(|p|viewport.shrink(4.).contains(*p))?;
            let rect=label_rect(anchor,viewport,&occupied);occupied.push(rect);Some((anchor,rect))
        }).collect();
        if self.render_studio.spatial.show_paths {
            paint_motion_paths(&painter,&camera,viewport,self.render_studio.spatial.camera_path.iter()
                .chain(self.render_studio.spatial.motion_paths.iter()),&mut occupied);
        }
        if !self.render_studio.spatial.show_keys {self.render_studio.spatial.drag=None;return busy;}
        for (marker,placement) in markers.iter().zip(labels) {
            let Some((anchor,rect))=placement else {continue;};
            let selected=self.render_studio.timeline.is_selected(marker.item);
            let tint=color(marker.lane);
            painter.circle_filled(anchor,3.,tint);
            painter.line_segment([anchor,rect.left_center()],egui::Stroke::new(1.,tint.gamma_multiply(0.6)));
            painter.rect_filled(rect,4.,if selected {egui::Color32::from_rgb(48,63,72)}else{egui::Color32::from_black_alpha(210)});
            painter.rect_stroke(rect,4.,egui::Stroke::new(if selected{2.}else{1.},if selected {egui::Color32::WHITE}else{tint}),egui::StrokeKind::Inside);
            let icon=rect.left_center()+egui::vec2(11.,0.);
            painter.add(egui::Shape::convex_polygon(vec![icon+egui::vec2(0.,-6.),icon+egui::vec2(6.,0.),
                icon+egui::vec2(0.,6.),icon+egui::vec2(-6.,0.)],tint,egui::Stroke::new(1.,egui::Color32::WHITE)));
            painter.text(icon+egui::vec2(10.,0.),egui::Align2::LEFT_CENTER,label(marker),egui::FontId::proportional(11.),egui::Color32::WHITE);
            let r=ui.interact(rect,ui.make_persistent_id(("studio_spatial_marker",marker.item)),egui::Sense::click_and_drag());
            busy|=r.hovered()||r.is_pointer_button_down_on();
            self.studio_spatial_response(ui,&r,marker,None,viewport,camera);
            let actor=marker.lane.map_or_else(String::new,|lane|format!(" · Character {}",lane+1));
            r.on_hover_text(format!("{}{} · {:.3}s\nDrag in the view plane. Select for X/Y/Z handles. Ctrl/Shift+click selects multiple. Double-click seeks to this key.",label(marker),actor,marker.time));
        }
        // The active key owns a single world-space translation gizmo. Other
        // selected keys move by the same world delta, across camera/motion tracks.
        let active=self.render_studio.timeline.selected_items().into_iter().rev()
            .filter(|item| !matches!(item,crate::app::render_studio_timeline::StudioItem::Camera(_)))
            .find_map(|item|markers.iter().find(|m|m.item==item));
        if let Some(marker)=active {
            if let Some(anchor)=project(&camera,viewport,marker.position).filter(|p|viewport.shrink(10.).contains(*p)) {
                let depth=(Vec3::from_array(marker.position)-camera.position).dot(camera.forward()).max(1.);
                let length=depth*2.*(camera.fov_y_for_viewport(viewport.width() as u32,viewport.height() as u32)*0.5).tan()
                    /viewport.height().max(1.)*66.;
                for (axis,(direction,tint,name)) in [(Vec3::X,egui::Color32::from_rgb(247,102,104),"X"),
                    (Vec3::Y,egui::Color32::from_rgb(114,223,118),"Y"),(Vec3::Z,egui::Color32::from_rgb(103,162,255),"Z")].into_iter().enumerate() {
                    let end=Vec3::from_array(marker.position)+direction*length;
                    let Some(end)=project(&camera,viewport,end.to_array()).filter(|p|viewport.shrink(8.).contains(*p)) else {continue;};
                    if anchor.distance(end)<20. {continue;}
                    painter.line_segment([anchor,end],egui::Stroke::new(2.5,tint));
                    painter.circle_filled(end,6.,tint);
                    painter.text(end+egui::vec2(0.,-11.),egui::Align2::CENTER_BOTTOM,name,egui::FontId::proportional(12.),tint);
                    let r=ui.interact(egui::Rect::from_center_size(end,egui::Vec2::splat(20.)),
                        ui.make_persistent_id(("studio_spatial_axis",marker.item,axis)),egui::Sense::click_and_drag());
                    busy|=r.hovered()||r.is_pointer_button_down_on();
                    self.studio_spatial_response(ui,&r,marker,Some(axis),viewport,camera);
                    r.on_hover_text(format!("Drag along scene {name}{}",if axis==1 {" (up)"}else{""}));
                }
            }
        }
        if let Some(drag)=self.render_studio.spatial.drag.clone() {
            busy=true;
            if let Some(pointer)=pointer {
                if let Some(delta)=drag.delta(pointer) {
                    let mut changed=false;
                    for (item,initial) in &drag.items {
                        let before=stored_position(&self.render_studio.timeline,*item);
                        translate_item(&mut self.render_studio.timeline,*item,*initial,delta);
                        changed|=before!=stored_position(&self.render_studio.timeline,*item);
                    }
                    if changed {self.profile_preview_key=None;ui.ctx().request_repaint();}
                }
            }
            if !ui.ctx().input(|i|i.pointer.primary_down()) {
                self.render_studio.spatial.drag=None;ui.ctx().request_repaint();
            }
        }
        busy
    }

    fn studio_spatial_response(&mut self,ui:&egui::Ui,r:&egui::Response,marker:&StudioSpatialMarker,
        axis:Option<usize>,viewport:egui::Rect,camera:crate::camera::Camera) {
        let (ctrl,shift)=ui.ctx().input(|i|(i.modifiers.ctrl||i.modifiers.command,i.modifiers.shift));
        if r.clicked() {
            self.render_studio.timeline.select_item(marker.item,ctrl,shift);
            self.render_studio.selected_background=false;
        }
        if r.double_clicked() {self.render_studio.timeline.seek(marker.time);self.profile_preview_key=None;}
        if r.drag_started() {
            if !self.render_studio.timeline.is_selected(marker.item) {
                self.render_studio.timeline.select_item(marker.item,ctrl,false);
            }
            self.render_studio.selected_background=false;
            let time=self.render_studio.timeline_time();self.render_studio.timeline.seek(time);
            if self.render_studio.spatial.scene_camera.is_none() {
                self.render_studio.spatial.scene_camera=Some(key_from_camera(camera));
                self.render_studio.spatial.pivot=marker.position;
            }
            let items=self.render_studio.timeline.selected_items().into_iter().filter_map(|item|
                stored_position(&self.render_studio.timeline,item).map(|position|(item,position))).collect();
            if let Some(start_pointer)=ui.ctx().input(|i|i.pointer.press_origin()) {
                self.render_studio.spatial.drag=Some(StudioSpatialDrag {camera,viewport,
                    origin:Vec3::from_array(marker.position),start_pointer,axis,items});
            }
            self.profile_preview_key=None;
        }
    }

}

impl App {
    /// Program viewer header: Shot / Scene view, key and path overlays.
    /// Toggles live in the pane header so nothing covers the 3D image.
    pub(super) fn egui_studio_viewer_tools(&mut self,ui:&mut egui::Ui,viewport:egui::Rect,locked:bool,demo:bool) {
        use super::render_studio_ui::{self as sui,ButtonKind,Icon};
        ui.spacing_mut().item_spacing.x=4.0;
        if let Some(preview)=self.render_studio.cinematic_preview.as_ref() {
            ui.add(egui::Label::new(egui::RichText::new(&preview.clip.label).size(12.5).color(theme::TEXT)).truncate());
            ui.label(egui::RichText::new(format!("frames {}–{} · {:.2} fps",preview.clip.first_frame,
                preview.clip.end_frame()-1,preview.clip.fps)).size(11.5).color(theme::TEXT_FAINT));
            return;
        }
        if demo {
            if sui::button(ui,None,if self.render_studio.hud_editing {"Done HUD"}else{"Edit HUD"},
                if self.render_studio.hud_editing {ButtonKind::Primary}else{ButtonKind::Normal},!locked).clicked() {
                self.render_studio.hud_editing=!self.render_studio.hud_editing;
                if self.render_studio.hud_editing { self.align_studio_defaults_for_editor(); }
                self.render_studio.hud_drag_origin=None;
                self.render_studio.hud_scale_origin=None;
                self.set_studio_mouse_look(false);
            }
            if self.render_studio.hud_editing {return;}
            let scene=self.render_studio.spatial.scene_camera.is_some();
            if sui::button(ui,None,if scene {"Shot view"}else{"Scene view"},ButtonKind::Normal,!locked).clicked() {
                if scene {self.render_studio.spatial.scene_camera=None;} else if let Some(frame)=self.render_studio.scene_frame.as_ref() {
                    let shot=frame.outgoing.shot_camera;
                    let target=shot.position+shot.forward()*128.0;
                    let eye=shot.position-shot.forward()*180.0+glam::Vec3::Y*80.0;
                    let mut camera=key_from_camera(shot);camera.position=eye.to_array();camera.focus_target=Some(target.to_array());camera.resolve_focus_target();
                    camera.focus_target=None;self.render_studio.spatial.scene_camera=Some(camera);
                    self.render_studio.spatial.show_keys=true;
                }
                self.profile_preview_key=None;
            }
            let time=self.render_studio.timeline_time();
            if let Some(shot)=self.render_studio.timeline.director_at(time) {
                ui.label(egui::RichText::new(format!("Director · {}",shot.preset.label())).size(12.0).color(sui::CAMERA))
                    .on_hover_text("A live Director shot drives the camera here (see the Camera track)");
            } else {
                use crate::app::render_studio_timeline::{StudioCameraSource,StudioDemoPov,StudioFollowPerspective};
                let key=self.render_studio.timeline.camera_at(time);
                let text=match key.source {
                    StudioCameraSource::Demo{pov:StudioDemoPov::Follow,client} => format!("{} · {}",
                        match key.demo_follow.perspective {
                            StudioFollowPerspective::FirstPerson=>"Eyes",
                            StudioFollowPerspective::ThirdPerson=>"Third person",
                            StudioFollowPerspective::Orbit=>"Orbit",
                        },
                        if client<0 {"recorded player".to_owned()} else {format!("client {client}")}),
                    StudioCameraSource::Demo{pov:StudioDemoPov::Free,..} => "Free camera".to_owned(),
                    _ => "Recorded POV".to_owned(),
                };
                ui.label(egui::RichText::new(text).size(12.0).color(sui::DEMO));
            }
            return;
        }
        ui.add_enabled_ui(!locked,|ui| {
            let scene=self.render_studio.spatial.scene_camera.is_some();
            let mut mode=scene;
            let changed=ui.allocate_ui(egui::vec2(132.0,24.0),|ui| sui::segmented(ui,&mut mode,&[(false,"Shot"),(true,"Scene")]))
                .response.on_hover_text("Shot: look through the exported camera.
Scene: navigate freely without changing the shot camera or its keys.");
            let _=changed;
            if mode!=scene {
                if mode {self.frame_studio_spatial_keys(viewport);}
                else {self.render_studio.spatial.scene_camera=None;self.render_studio.spatial.drag=None;self.profile_preview_key=None;}
            }
            ui.add_space(6.0);
            if sui::icon_button(ui,Icon::Keys,true,self.render_studio.spatial.show_keys,"Show keyframe markers in the view").clicked() {
                self.render_studio.spatial.show_keys=!self.render_studio.spatial.show_keys;
            }
            let paths=sui::icon_button(ui,Icon::Path,true,self.render_studio.spatial.show_paths,
                "Motion paths. Yellow: camera path; actor paths match their track color, including movement within cinematic clips. Ticks mark whole seconds. Editor only, never exported.");
            if paths.clicked() {self.render_studio.spatial.show_paths=!self.render_studio.spatial.show_paths;}
            if let Some(message)=&self.render_studio.spatial.path_message {paths.on_hover_text(message);}
            let fit=if ui.available_width()<100.0 {
                sui::icon_button(ui,Icon::Frame,true,false,"Fit keys")
            } else {sui::button(ui,None,"Fit keys",ButtonKind::Ghost,true)};
            if fit
                .on_hover_text("Fit the camera, motion keys and actor paths in Scene view").clicked() {
                self.render_studio.spatial.show_keys=true;self.frame_studio_spatial_keys(viewport);
            }
            if self.render_studio.spatial.paths_pending {
                ui.add(egui::Spinner::new().size(12.)).on_hover_text("Tracing animated actor paths...");
            }
        });
    }
}
